//! The 3D trace (`--trace-3d FILE`): what a program asks of DirectDraw,
//! Direct3D 3 to 8 and OpenGL, for finding out what a 3D card has to
//! offer the programs that run on it.
//!
//! Each event is a line of text, its key. FILE gets a key the first time
//! it happens, numbered in order; `FILE.counts` gets every key with the
//! number of times it happened. The events are the calls of DDRAW, D3D8,
//! OPENGL32 and GLU32 with the arguments that tell what was asked (states
//! and their values, formats, sizes, the surfaces involved) and their
//! results, the items an enumeration offered and what the program's
//! callback answered, the 3D DLLs and functions asked for by name through
//! KERNEL32, GDI's pixel format calls, OpenGL functions that aren't
//! there, execute buffers' states, and a line for each different set of
//! states a draw (`draw`) or ProcessVertices (`process`) used, with the
//! texture stages as the card got them. States, formats and enums have
//! their names, without the API's prefix.
//!
//! The trace only reads the machine. It reads guest memory through the
//! running process's page tables, plain RAM only, so it never faults a
//! page in, sets an accessed bit or reads a card's registers. The files
//! are written on the host's clock and when the machine stops: nothing
//! the program sees depends on them. Off, each hook is one test of
//! `Win32::trace3d`.

mod names;

use crate::cpu::Cpu;
use crate::win32::task::Next;
use names::Gl;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How often `FILE.counts` is rewritten while events come, in the host's
/// time, so that a run that is killed leaves counts.
const REWRITE: Duration = Duration::from_secs(2);

pub struct Trace3d {
    path: PathBuf,
    events: BufWriter<File>,
    counts: HashMap<String, u64>,
    order: Vec<String>,
    written: Instant,
    /// The item an enumeration gave the program's callback, by the
    /// task's scratch space, until the callback returns.
    offered: HashMap<u32, String>,
}

impl std::fmt::Debug for Trace3d {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Trace3d")
            .field("path", &self.path)
            .field("events", &self.order.len())
            .finish()
    }
}

impl Trace3d {
    fn create(path: &Path) -> io::Result<Self> {
        Ok(Trace3d {
            path: path.to_path_buf(),
            events: BufWriter::new(File::create(path)?),
            counts: HashMap::new(),
            order: Vec::new(),
            written: Instant::now(),
            offered: HashMap::new(),
        })
    }

    fn record(&mut self, key: String) {
        // Most events have come before: the key is copied only for a new
        // one, which `order` keeps too.
        if let Some(count) = self.counts.get_mut(&key) {
            *count += 1;
        } else {
            let _ = writeln!(self.events, "{:6} {}", self.order.len(), key);
            self.counts.insert(key.clone(), 1);
            self.order.push(key);
        }
        if self.written.elapsed() >= REWRITE {
            self.write();
        }
    }

    /// Write out the events and the counts as they are now.
    fn write(&mut self) {
        let mut text = String::new();
        for key in &self.order {
            text.push_str(&format!("{}\t{}\n", self.counts[key], key));
        }
        let _ = std::fs::write(counts_path(&self.path), text);
        let _ = self.events.flush();
        self.written = Instant::now();
    }
}

impl Drop for Trace3d {
    fn drop(&mut self) {
        self.write();
    }
}

/// `FILE.counts` for the trace `FILE`.
pub fn counts_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".counts");
    PathBuf::from(name)
}

/// Start the trace into `path` (and `path.counts`), replacing them.
pub fn start(cpu: &mut Cpu, path: &Path) -> io::Result<()> {
    cpu.bus.win32.trace3d = Some(Box::new(Trace3d::create(path)?));
    Ok(())
}

/// Write the trace's files as they are now: when the machine stops.
pub fn flush(cpu: &mut Cpu) {
    if let Some(t) = cpu.bus.win32.trace3d.as_deref_mut() {
        t.write();
    }
}

/// Whether the trace is on.
#[inline(always)]
pub fn on(cpu: &Cpu) -> bool {
    cpu.bus.win32.trace3d.is_some()
}

/// Count the event `key`.
pub(crate) fn record(cpu: &mut Cpu, key: String) {
    if let Some(t) = cpu.bus.win32.trace3d.as_deref_mut() {
        t.record(key);
    }
}

// Reading the machine.

/// The word at `at` in the running process, if it is mapped to plain
/// RAM.
fn peek(cpu: &Cpu, at: u32) -> Option<u32> {
    if at == 0 {
        return None;
    }
    if at & 0xFFF > 0xFFC {
        let mut b = [0u8; 4];
        for (i, byte) in b.iter_mut().enumerate() {
            *byte = peek_8(cpu, at.wrapping_add(i as u32))?;
        }
        return Some(u32::from_le_bytes(b));
    }
    let memory = &super::running(cpu)?.memory;
    let Some(phys) = memory.translate(&cpu.bus, at) else {
        // A page in the page file is read from there.
        return memory.read_32(&cpu.bus, at);
    };
    let phys = phys as usize;
    cpu.bus
        .is_plain_ram(phys, 4)
        .then(|| cpu.bus.ram_read::<4>(phys))
}

fn peek_8(cpu: &Cpu, at: u32) -> Option<u8> {
    let memory = &super::running(cpu)?.memory;
    let Some(phys) = memory.translate(&cpu.bus, at) else {
        let mut byte = [0];
        memory.read(&cpu.bus, at, &mut byte)?;
        return Some(byte[0]);
    };
    let phys = phys as usize;
    cpu.bus
        .is_plain_ram(phys, 1)
        .then(|| cpu.bus.ram_read::<1>(phys) as u8)
}

fn peek_bytes(cpu: &Cpu, at: u32, len: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(len + 3);
    for i in (0..len).step_by(4) {
        bytes.extend_from_slice(&peek(cpu, at.wrapping_add(i as u32))?.to_le_bytes());
    }
    bytes.truncate(len);
    Some(bytes)
}

/// The string at `at`, up to 128 characters.
fn peek_str(cpu: &Cpu, at: u32) -> Option<String> {
    let mut s = String::new();
    for i in 0..128 {
        match peek_8(cpu, at.checked_add(i)?)? {
            0 => return Some(s),
            b => s.push(b as char),
        }
    }
    Some(s)
}

fn guid(cpu: &Cpu, at: u32) -> String {
    if at == 0 {
        return "NULL".to_string();
    }
    let Some(b) = peek_bytes(cpu, at, 16) else {
        return "?".to_string();
    };
    format!(
        "{{{:08X}-{:04X}-{:04X}-{}}}",
        u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        u16::from_le_bytes([b[4], b[5]]),
        u16::from_le_bytes([b[6], b[7]]),
        b[8..]
            .iter()
            .map(|x| format!("{:02X}", x))
            .collect::<String>()
    )
}

/// The object of the COM interface `this` of the built-in `dll`, read
/// as `com::interface_of` reads it, without its probe.
fn object_of(cpu: &Cpu, dll: &str, this: u32) -> Option<u32> {
    interface_of(cpu, dll, this).map(|(_, id)| id)
}

/// The same with the interface's vtable, as its place in the DLL's
/// table (`com::interface_of`'s).
fn interface_of(cpu: &Cpu, dll: &str, this: u32) -> Option<(usize, u32)> {
    let vtable = peek(cpu, this)?;
    let index = super::stubs::DLLS.iter().position(|d| d.name == dll)?;
    let table = (0..super::stubs::DLLS[index].vtables.len())
        .find(|&i| super::stubs::vtable_address(index, i) == vtable)?;
    Some((table, peek(cpu, this.wrapping_add(4))?))
}

/// The vertex type DrawPrimitive and DrawIndexedPrimitive of the
/// Direct3D device `this` were given: IDirect3DDevice2's D3DVERTEXTYPE,
/// or an FVF code from IDirect3DDevice3 on (`draw::vertex_format`).
fn vertex_type(cpu: &Cpu, this: u32, vt: u32) -> String {
    let version = interface_of(cpu, "DDRAW.DLL", this)
        .map_or(7, |(vtable, _)| super::ddraw::d3d::device::version(vtable));
    if version > 2 {
        return names::fvf(vt);
    }
    match vt {
        1 => "VERTEX".to_string(),
        2 => "LVERTEX".to_string(),
        3 => "TLVERTEX".to_string(),
        _ => names::number(vt),
    }
}

/// DirectDraw's surface `id`: its size, format and caps.
fn surface(cpu: &Cpu, id: u32) -> String {
    match cpu.bus.win32.ddraw.surface(id) {
        Some(s) => format!(
            "{}x{} {} {}",
            s.width,
            s.height,
            names::pixel_format(
                s.format.flags,
                s.format.fourcc,
                s.format.bits,
                [s.format.r, s.format.g, s.format.b, s.format.a]
            ),
            names::ddscaps(s.caps[0])
        ),
        None => "none".to_string(),
    }
}

/// The surface of the DirectDraw interface `this`.
fn surface_iface(cpu: &Cpu, this: u32) -> String {
    match object_of(cpu, "DDRAW.DLL", this) {
        Some(id) => surface(cpu, id),
        None if this == 0 => "NULL".to_string(),
        None => "?".to_string(),
    }
}

/// The depth buffer attached to DirectDraw's surface `id`.
fn z_buffer(cpu: &Cpu, id: u32) -> Option<u32> {
    let ddraw = &cpu.bus.win32.ddraw;
    ddraw.surface(id)?.attached.iter().map(|a| a.0).find(|&a| {
        ddraw
            .surface(a)
            .is_some_and(|z| z.caps[0] & super::ddraw::surface::DDSCAPS_ZBUFFER != 0)
    })
}

/// The render target and depth buffer of Direct3D's device `id` (of
/// DirectDraw's objects).
fn device_buffers(cpu: &Cpu, id: u32) -> String {
    let device = cpu
        .bus
        .win32
        .ddraw
        .object(id)
        .and_then(|o| o.d3d.as_ref())
        .and_then(|d| d.device.as_ref());
    let Some(d) = device else {
        return "device=?".to_string();
    };
    let depth = if d.version == 8 {
        Some(d.depth).filter(|&z| z != 0)
    } else {
        z_buffer(cpu, d.target)
    };
    format!(
        "target=[{}] depth=[{}]",
        surface(cpu, d.target),
        depth.map_or("none".to_string(), |z| surface(cpu, z))
    )
}

/// Direct3D 8's surface `id`: its size, format and usage.
fn surface8(cpu: &Cpu, id: u32) -> String {
    let s = cpu
        .bus
        .win32
        .d3d8
        .object(id)
        .and_then(|o| o.surface.as_ref());
    match s {
        Some(s) => format!(
            "{}x{} {} usage=0x{:X} {}",
            s.width,
            s.height,
            names::d3d_format(s.format),
            s.usage,
            names::pool(s.pool)
        ),
        None => "none".to_string(),
    }
}

fn surface8_iface(cpu: &Cpu, this: u32) -> String {
    if this == 0 {
        return "NULL".to_string();
    }
    object_of(cpu, "D3D8.DLL", this).map_or("?".to_string(), |id| surface8(cpu, id))
}

/// DDPIXELFORMAT at `at` as a name.
fn pixel_format_at(cpu: &Cpu, at: u32) -> String {
    let w: Vec<u32> = (0..8)
        .map(|i| peek(cpu, at.wrapping_add(4 * i)).unwrap_or(0))
        .collect();
    names::pixel_format(w[1], w[2], w[3], [w[4], w[5], w[6], w[7]])
}

/// DDSURFACEDESC or DDSURFACEDESC2 at `at`: the fields its flags say
/// are there.
fn surface_desc_at(cpu: &Cpu, at: u32) -> String {
    let w = |o: u32| peek(cpu, at.wrapping_add(o)).unwrap_or(0);
    let flags = w(4);
    let mut s = Vec::new();
    if flags & 0x6 != 0 {
        s.push(format!("{}x{}", w(12), w(8)));
    }
    if flags & 0x1000 != 0 {
        s.push(pixel_format_at(cpu, at.wrapping_add(72)));
    }
    if flags & 0x1 != 0 {
        let mut caps = names::ddscaps(w(104));
        if w(0) >= 124 && w(108) != 0 {
            caps += &format!(" caps2=0x{:X}", w(108));
        }
        s.push(caps);
    }
    if flags & 0x20 != 0 {
        s.push(format!("backbuffers={}", w(20)));
    }
    if flags & 0x2_0000 != 0 {
        s.push(format!("mips={}", w(24)));
    }
    if flags & 0x40 != 0 {
        s.push(format!("zbits={}", w(24)));
    }
    if flags & 0x1_0000 != 0 {
        s.push("cksrcblt".to_string());
    }
    if flags & 0x800 != 0 {
        s.push("lpsurface".to_string());
    }
    s.join(" ")
}

/// D3DPRESENT_PARAMETERS at `at`.
fn present_params(cpu: &Cpu, at: u32) -> String {
    let w: Vec<u32> = (0..13)
        .map(|i| peek(cpu, at.wrapping_add(4 * i)).unwrap_or(0))
        .collect();
    format!(
        "[{}x{} {} backbuffers={} ms={} swap={} windowed={} autodepth={} {} flags=0x{:X}]",
        w[0],
        w[1],
        names::d3d_format(w[2]),
        w[3],
        w[4],
        w[5],
        w[7],
        w[8],
        names::d3d_format(w[9]),
        w[10]
    )
}

/// PIXELFORMATDESCRIPTOR at `at`.
fn pfd(cpu: &Cpu, at: u32) -> String {
    let Some(b) = peek_bytes(cpu, at, 28) else {
        return "pfd[?]".to_string();
    };
    format!(
        "pfd[flags=0x{:X} type={} color={} alpha={} accum={} depth={} stencil={}]",
        u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
        b[8],
        b[9],
        b[16],
        b[18],
        b[23],
        b[24]
    )
}

/// The arguments `which` of `args`, as numbers, in parentheses.
fn pick(args: &[u32], which: &[usize]) -> String {
    let list: Vec<String> = which
        .iter()
        .map(|&i| args.get(i).map_or("?".to_string(), |&a| names::number(a)))
        .collect();
    format!("({})", list.join(","))
}

/// A COM method's result.
fn hresult(result: Option<u32>) -> String {
    match result {
        Some(0) => "OK".to_string(),
        Some(r) => format!("0x{:08X}", r),
        None => "-".to_string(),
    }
}

// The calls.

/// A call of a built-in function, after it returned `result` (None for
/// one that called back into the program, which returns later).
pub(crate) fn call(cpu: &mut Cpu, module: &str, name: &str, args: &[u32], result: Option<u32>) {
    // Short argument lists read as zeros past their end.
    let mut padded = [0u32; 16];
    for (to, from) in padded.iter_mut().zip(args) {
        *to = *from;
    }
    let args = &padded[..];
    let detail = match module {
        "DDRAW" | "D3D8" => com_detail(cpu, module, name, args, result),
        "OPENGL32" => Some(gl_detail(cpu, name, args, result)),
        "GLU32" => Some(String::new()),
        "KERNEL32" => kernel_detail(cpu, name, args, result),
        "GDI32" => gdi_detail(cpu, name, args, result),
        _ => None,
    };
    if let Some(detail) = detail {
        let key = match module {
            "KERNEL32" => format!("KERNEL32.{}", detail),
            _ => format!("{}.{}{}", module, name, detail),
        };
        record(cpu, key);
    }
}

fn com_detail(
    cpu: &Cpu,
    module: &str,
    name: &str,
    args: &[u32],
    result: Option<u32>,
) -> Option<String> {
    let method = name.rsplit_once('_').map_or(name, |(_, m)| m);
    let r = hresult(result);
    let d8 = module == "D3D8";
    let a = |which: &[usize]| pick(args, which);
    let fmt = |i: usize| names::d3d_format(args[i]);
    let detail = match method {
        "AddRef" | "Release" => return None,
        // Polled every frame, with nothing to tell.
        "GetRenderState"
        | "GetTextureStageState"
        | "Lock"
        | "Unlock"
        | "LockRect"
        | "UnlockRect"
        | "GetSurfaceDesc"
        | "GetDesc"
        | "GetLevelDesc" => format!(" = {}", r),
        "QueryInterface" => format!(" {} = {}", guid(cpu, args[1]), r),
        "SetRenderState" => format!(
            "({},{}) = {}",
            names::render_state(args[1]).map_or_else(|| names::number(args[1]), str::to_string),
            names::render_value(args[1], args[2]),
            r
        ),
        "SetLightState" => format!(
            "({},{}) = {}",
            names::light_state(args[1]).map_or_else(|| names::number(args[1]), str::to_string),
            names::light_value(args[1], args[2]),
            r
        ),
        "LightEnable" => format!("{} = {}", a(&[1, 2]), r),
        "SetTextureStageState" => format!(
            "({},{},{}) = {}",
            args[1],
            names::stage_state(args[2]).map_or_else(|| names::number(args[2]), str::to_string),
            names::stage_value(args[2], args[3], d8),
            r
        ),
        "SetTransform" | "MultiplyTransform" => {
            format!("({}) = {}", names::transform(args[1]), r)
        }
        "SetClipPlane" => format!("{} = {}", a(&[1]), r),
        "SetTexture" if d8 => format!("({},{}) = {}", args[1], (args[2] != 0) as u32, r),
        "SetTexture" => {
            let texture = match args[2] {
                0 => "none".to_string(),
                t => object_of(cpu, "DDRAW.DLL", t).map_or("?".to_string(), |id| {
                    cpu.bus
                        .win32
                        .ddraw
                        .surface(id)
                        .map_or("?".to_string(), |s| {
                            names::pixel_format(
                                s.format.flags,
                                s.format.fourcc,
                                s.format.bits,
                                [s.format.r, s.format.g, s.format.b, s.format.a],
                            )
                        })
                }),
            };
            format!("({},{}) = {}", args[1], texture, r)
        }
        "SetLight" => {
            let (index, at) = if name.starts_with("IDirect3DLight_") {
                (None, args[1].wrapping_add(4))
            } else {
                (Some(args[1]), args[2])
            };
            let kind = peek(cpu, at).unwrap_or(0);
            let kind = match kind {
                1 => "POINT".to_string(),
                2 => "SPOT".to_string(),
                3 => "DIRECTIONAL".to_string(),
                4 => "PARALLELPOINT".to_string(),
                k => names::number(k),
            };
            match index {
                Some(i) => format!("({},{}) = {}", i, kind, r),
                None => format!("({}) = {}", kind, r),
            }
        }
        "DrawPrimitive"
        | "DrawIndexedPrimitive"
        | "DrawPrimitiveStrided"
        | "DrawIndexedPrimitiveStrided"
            if !d8 =>
        {
            format!(
                "({},{}) = {}",
                names::primitive(args[1]),
                vertex_type(cpu, args[0], args[2]),
                r
            )
        }
        "DrawPrimitive"
        | "DrawIndexedPrimitive"
        | "DrawPrimitiveUP"
        | "DrawIndexedPrimitiveUP"
        | "DrawPrimitiveVB"
        | "DrawIndexedPrimitiveVB" => format!("({}) = {}", names::primitive(args[1]), r),
        "ProcessVertices" if d8 => format!("(flags=0x{:X}) = {}", args[5], r),
        "ProcessVertices" | "ProcessVerticesStrided" => {
            format!("(op=0x{:X},flags=0x{:X}) = {}", args[1], args[7], r)
        }
        "SetVertexShader" if args[1] < 0x1_0000 => {
            format!("({}) = {}", names::fvf(args[1]), r)
        }
        "SetVertexShader" => format!("(shader) = {}", r),
        "CreateVertexBuffer" if d8 => format!(
            "(usage=0x{:X},{},{}) = {}",
            args[2],
            names::fvf(args[3]),
            names::pool(args[4]),
            r
        ),
        "CreateVertexBuffer" => {
            let w = |o: u32| peek(cpu, args[1].wrapping_add(o)).unwrap_or(0);
            format!("(caps=0x{:X},{}) = {}", w(4), names::fvf(w(8)), r)
        }
        "CreateIndexBuffer" => format!(
            "(usage=0x{:X},{},{}) = {}",
            args[2],
            fmt(3),
            names::pool(args[4]),
            r
        ),
        "CreateTexture" => format!(
            "({}x{},levels={},usage=0x{:X},{},{}) = {}",
            args[1],
            args[2],
            args[3],
            args[4],
            fmt(5),
            names::pool(args[6]),
            r
        ),
        "CreateCubeTexture" => format!(
            "({},levels={},usage=0x{:X},{},{}) = {}",
            args[1],
            args[2],
            args[3],
            fmt(4),
            names::pool(args[5]),
            r
        ),
        "CreateVolumeTexture" => format!(
            "({}x{}x{},levels={},usage=0x{:X},{},{}) = {}",
            args[1],
            args[2],
            args[3],
            args[4],
            args[5],
            fmt(6),
            names::pool(args[7]),
            r
        ),
        "CreateRenderTarget" => format!(
            "({}x{},{},ms={},lockable={}) = {}",
            args[1],
            args[2],
            fmt(3),
            args[4],
            args[5],
            r
        ),
        "CreateDepthStencilSurface" => {
            format!(
                "({}x{},{},ms={}) = {}",
                args[1],
                args[2],
                fmt(3),
                args[4],
                r
            )
        }
        "CreateImageSurface" => format!("({}x{},{}) = {}", args[1], args[2], fmt(3), r),
        "CheckDeviceFormat" => format!(
            "({},{},{},usage=0x{:X},rtype={},{}) = {}",
            args[1],
            args[2],
            fmt(3),
            args[4],
            args[5],
            fmt(6),
            r
        ),
        "CheckDeviceType" => format!(
            "({},{},{},{},windowed={}) = {}",
            args[1],
            args[2],
            fmt(3),
            fmt(4),
            args[5],
            r
        ),
        "CheckDepthStencilMatch" => format!(
            "({},{},{},{},{}) = {}",
            args[1],
            args[2],
            fmt(3),
            fmt(4),
            fmt(5),
            r
        ),
        "CheckMultiSampleType" => format!(
            "({},{},{},windowed={},ms={}) = {}",
            args[1],
            args[2],
            fmt(3),
            args[4],
            args[5],
            r
        ),
        "GetDeviceCaps" if !name.starts_with("Device8") => format!("{} = {}", a(&[1, 2]), r),
        "CreateDevice" if d8 => format!(
            "({},{},behavior=0x{:X}) {} = {}",
            args[1],
            args[2],
            args[4],
            present_params(cpu, args[5]),
            r
        ),
        // (this, guid, target, out): Direct3D 2, 3 and 7.
        "CreateDevice" => format!(
            " {} on [{}] = {}",
            guid(cpu, args[1]),
            surface_iface(cpu, args[2]),
            r
        ),
        "Reset" => format!(" {} = {}", present_params(cpu, args[1]), r),
        "ValidateDevice" => {
            let passes = peek(cpu, args[1]);
            format!(" passes={:?} = {}", passes, r)
        }
        "CreateSurface" => {
            // (this, desc, out, outer): what was asked, and what was made.
            let made = match result {
                Some(0) => peek(cpu, args[2]).map_or("?".to_string(), |s| surface_iface(cpu, s)),
                _ => "nothing".to_string(),
            };
            format!(
                " [{}] made [{}] = {}",
                surface_desc_at(cpu, args[1]),
                made,
                r
            )
        }
        "AddAttachedSurface" => format!(
            " [{}] gets [{}] = {}",
            surface_iface(cpu, args[0]),
            surface_iface(cpu, args[1]),
            r
        ),
        "DeleteAttachedSurface" => format!(
            " [{}] loses [{}] flags=0x{:X} = {}",
            surface_iface(cpu, args[0]),
            if args[2] == 0 {
                "all".to_string()
            } else {
                surface_iface(cpu, args[2])
            },
            args[1],
            r
        ),
        "SetRenderTarget" if d8 => format!(
            " target=[{}] depth=[{}] = {}",
            surface8_iface(cpu, args[1]),
            surface8_iface(cpu, args[2]),
            r
        ),
        "SetRenderTarget" => {
            let depth = object_of(cpu, "DDRAW.DLL", args[1])
                .and_then(|id| z_buffer(cpu, id))
                .map_or("none".to_string(), |z| surface(cpu, z));
            format!(
                " target=[{}] depth=[{}] flags=0x{:X} = {}",
                surface_iface(cpu, args[1]),
                depth,
                args[2],
                r
            )
        }
        "Clear" | "Clear2" => clear_detail(cpu, name, d8, args, &r),
        "Blt" => format!(
            " [{}] <- [{}] flags=0x{:X} = {}",
            surface_kind(cpu, args[0]),
            surface_kind(cpu, args[2]),
            args[4],
            r
        ),
        "BltFast" => format!(
            " [{}] <- [{}] trans=0x{:X} = {}",
            surface_kind(cpu, args[0]),
            surface_kind(cpu, args[3]),
            args[5],
            r
        ),
        "SetDisplayMode" => format!("{} = {}", a(&[1, 2, 3]), r),
        // The lists, whose items and answers come as events of their own.
        "EnumZBufferFormats" => format!(" {} = {}", guid(cpu, args[1]), r),
        "EnumDisplayModes" => format!("(flags=0x{:X}) = {}", args[1], r),
        "SetCooperativeLevel" => format!("(0x{:X}) = {}", args[2], r),
        _ => format!(" = {}", r),
    };
    Some(detail)
}

/// A blit's surface: its format and caps, without the size, which
/// changes with every sprite.
fn surface_kind(cpu: &Cpu, this: u32) -> String {
    if this == 0 {
        return "NULL".to_string();
    }
    let Some(s) = object_of(cpu, "DDRAW.DLL", this).and_then(|id| cpu.bus.win32.ddraw.surface(id))
    else {
        return "?".to_string();
    };
    format!(
        "{} {}",
        names::pixel_format(
            s.format.flags,
            s.format.fourcc,
            s.format.bits,
            [s.format.r, s.format.g, s.format.b, s.format.a]
        ),
        names::ddscaps(s.caps[0])
    )
}

/// Clear and Clear2 of Direct3D's devices and viewports, with the
/// buffers they clear.
fn clear_detail(cpu: &Cpu, name: &str, d8: bool, args: &[u32], r: &str) -> String {
    // IDirect3DViewport's Clear has no color, depth or stencil value.
    let plain = name.ends_with("Viewport_Clear");
    let values = if plain {
        String::new()
    } else {
        format!(
            " color=0x{:X} z={} stencil={}",
            args[4],
            names::float(args[5]),
            args[6]
        )
    };
    let buffers = if d8 {
        object_of(cpu, "D3D8.DLL", args[0])
            .and_then(|id| cpu.bus.win32.d3d8.object(id))
            .and_then(|o| o.dev.as_ref())
            .map_or("device=?".to_string(), |d| {
                format!(
                    "target=[{}] depth=[{}]",
                    surface8(cpu, d.target),
                    surface8(cpu, d.depth)
                )
            })
    } else {
        let id = object_of(cpu, "DDRAW.DLL", args[0]);
        let o = id.and_then(|id| cpu.bus.win32.ddraw.object(id));
        let d3d = o.and_then(|o| o.d3d.as_ref());
        match (d3d.and_then(|d| d.viewport.as_ref()), id) {
            (Some(v), _) => device_buffers(cpu, v.device),
            (None, Some(id)) => device_buffers(cpu, id),
            (None, None) => "device=?".to_string(),
        }
    };
    format!(
        "(rects={},{}{}) {} = {}",
        args[1],
        names::clear_flags(args[3]),
        values,
        buffers,
        r
    )
}

/// The kinds of OpenGL's arguments, for the calls whose arguments the
/// trace writes.
fn gl_args(name: &str) -> &'static [(usize, Gl)] {
    use Gl::*;
    match name {
        "glEnable"
        | "glDisable"
        | "glIsEnabled"
        | "glEnableClientState"
        | "glDisableClientState"
        | "glMatrixMode"
        | "glShadeModel"
        | "glCullFace"
        | "glFrontFace"
        | "glDepthFunc"
        | "glGetIntegerv"
        | "glGetFloatv"
        | "glGetBooleanv"
        | "glGetDoublev"
        | "glDrawBuffer"
        | "glReadBuffer"
        | "glActiveTextureARB"
        | "glClientActiveTextureARB"
        | "glClipPlane"
        | "glBlendEquationEXT"
        | "glFogfv"
        | "glLightModelfv" => &[(0, Enum)],
        "glBegin" | "glDrawArrays" => &[(0, Mode)],
        "glDrawElements" => &[(0, Mode), (2, Enum)],
        "glDepthMask" | "glLockArraysEXT" => &[(0, Int)],
        "glBlendFunc" => &[(0, Factor), (1, Factor)],
        "glHint" | "glPolygonMode" | "glColorMaterial" | "glFogi" | "glLightModeli"
        | "glTexEnvfv" | "glTexEnviv" | "glTexGenfv" | "glTexParameterfv" | "glTexParameteriv"
        | "glLightfv" | "glMaterialfv" => &[(0, Enum), (1, Enum)],
        "glPixelStorei" => &[(0, Enum), (1, Int)],
        "glTexParameteri" | "glTexEnvi" | "glTexGeni" | "glStencilOp" => {
            &[(0, Enum), (1, Enum), (2, Enum)]
        }
        "glAlphaFunc" => &[(0, Enum), (1, Float)],
        "glStencilFunc" => &[(0, Enum), (1, Int), (2, Int)],
        "glLightf" | "glMaterialf" => &[(0, Enum), (1, Enum), (2, Float)],
        "glLighti" => &[(0, Enum), (1, Enum), (2, Int)],
        "glTexEnvf" | "glTexParameterf" | "glTexGenf" => &[(0, Enum), (1, Enum), (2, FloatEnum)],
        "glFogf" | "glLightModelf" => &[(0, Enum), (1, FloatEnum)],
        "glTexImage1D" => &[(0, Enum), (2, Enum), (3, Int), (5, Enum), (6, Enum)],
        "glCopyTexImage2D" => &[(0, Enum), (2, Enum), (5, Int), (6, Int)],
        "glTexSubImage2D" => &[(0, Enum), (6, Enum), (7, Enum)],
        "glColorTableEXT" => &[(0, Enum), (1, Enum), (2, Int), (3, Enum), (4, Enum)],
        "glCompressedTexImage2DARB" => &[(0, Enum), (2, Enum), (3, Int), (4, Int)],
        _ => &[],
    }
}

fn gl_detail(cpu: &Cpu, name: &str, args: &[u32], result: Option<u32>) -> String {
    match name {
        "glGetString" => {
            let s = result.and_then(|at| peek_str(cpu, at)).unwrap_or_default();
            format!("({}) = \"{}\"", names::gl(args[0], Gl::Enum), s)
        }
        "glTexImage2D" => format!(
            "({},level0={},{},{}x{},border={},{},{})",
            names::gl(args[0], Gl::Enum),
            (args[1] == 0) as u32,
            names::gl(args[2], Gl::Enum),
            args[3],
            args[4],
            args[5],
            names::gl(args[6], Gl::Enum),
            names::gl(args[7], Gl::Enum)
        ),
        "wglGetProcAddress" => {
            let s = peek_str(cpu, args[0]).unwrap_or_default();
            format!("(\"{}\") found={}", s, result.unwrap_or(0) != 0)
        }
        "wglChoosePixelFormat" => format!(
            " {} = {}",
            pfd(cpu, args[1]),
            names::number(result.unwrap_or(0))
        ),
        "wglSetPixelFormat" => format!(
            "({}) = {}",
            names::number(args[1]),
            names::number(result.unwrap_or(0))
        ),
        "wglCreateContext" | "wglMakeCurrent" => {
            format!(" = {}", (result.unwrap_or(0) != 0) as u32)
        }
        _ => {
            let kinds = gl_args(name);
            if kinds.is_empty() {
                return String::new();
            }
            let list: Vec<String> = kinds.iter().map(|&(i, k)| names::gl(args[i], k)).collect();
            format!("({})", list.join(","))
        }
    }
}

/// KERNEL32's loads of 3D DLLs and lookups of 3D functions by name.
fn kernel_detail(cpu: &Cpu, name: &str, args: &[u32], result: Option<u32>) -> Option<String> {
    let at = match name {
        "GetProcAddress" => args[1],
        "LoadLibraryA" | "LoadLibraryExA" | "GetModuleHandleA" => args[0],
        _ => return None,
    };
    // An ordinal.
    if at < 0x1_0000 {
        return None;
    }
    let s = peek_str(cpu, at)?;
    let wanted = if name == "GetProcAddress" {
        // Exports keep their case: glBegin and wglCreateContext are
        // OpenGL's, GlobalAlloc and DirectSoundCreate aren't 3D.
        ["gl", "wgl", "Direct3D", "DirectDraw", "D3D", "d3d"]
            .iter()
            .any(|p| s.starts_with(p))
    } else {
        // The file's name, without its folder: GLIDE2X.DLL and GLU32.DLL,
        // not ENGLISH.DLL.
        let lower = s.to_ascii_lowercase();
        let file = lower.rsplit(['\\', '/']).next().unwrap_or(&lower);
        file.starts_with("gl")
            || ["opengl", "d3d", "ddraw", "3dfx"]
                .iter()
                .any(|p| file.contains(p))
    };
    wanted.then(|| format!("{}(\"{}\") found={}", name, s, result.unwrap_or(0) != 0))
}

fn gdi_detail(cpu: &Cpu, name: &str, args: &[u32], result: Option<u32>) -> Option<String> {
    let result = names::number(result.unwrap_or(0));
    match name {
        "ChoosePixelFormat" => Some(format!(" {} = {}", pfd(cpu, args[1]), result)),
        "SetPixelFormat" => Some(format!("({}) = {}", names::number(args[1]), result)),
        "DescribePixelFormat" if args[1] != 0 && args[3] != 0 => Some(format!(
            "({}) {}",
            names::number(args[1]),
            pfd(cpu, args[3])
        )),
        _ => None,
    }
}

/// An OpenGL function OPENGL32 doesn't have was called.
pub(crate) fn gl_missing(cpu: &mut Cpu, name: &str) {
    record(cpu, format!("OPENGL32 not implemented: {}", name));
}

/// An execute buffer's state instruction: D3DOP_STATETRANSFORM (6),
/// STATELIGHT (7) or STATERENDER (8), and the state and value.
pub(crate) fn execute_state(cpu: &mut Cpu, op: u8, s: u32, value: u32) {
    let key = match op {
        6 => format!("EXECUTE STATETRANSFORM({},{})", names::transform(s), value),
        7 => format!(
            "EXECUTE STATELIGHT({},{})",
            names::light_state(s).map_or_else(|| names::number(s), str::to_string),
            names::light_value(s, value)
        ),
        8 => format!(
            "EXECUTE STATERENDER({},{})",
            names::render_state(s).map_or_else(|| names::number(s), str::to_string),
            names::render_value(s, value)
        ),
        _ => format!("EXECUTE op={}({},{})", op, s, value),
    };
    record(cpu, key);
}

/// What the pipeline's setup is for: a draw, or ProcessVertices.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pass {
    Draw,
    Process,
}

/// A draw or ProcessVertices of a device of `version`: the vertices'
/// FVF, and the states that say how they are drawn. `render` and
/// `stages` are the device's render states by number and its texture
/// stages' states (32 a stage); `textures` says which stages have a
/// texture, `lights` how many lights are on.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw(
    cpu: &mut Cpu,
    pass: Pass,
    version: u32,
    fvf: u32,
    render: &[u32],
    stages: &[u32],
    textures: &[bool],
    lights: usize,
) {
    let r = |s: u32| names::render_value(s, render.get(s as usize).copied().unwrap_or(0));
    let mut key = format!(
        "{} v{} fvf={} z={},{},{} alphatest={},{} blend={},{},{},{} fog={},{},{},{} \
         specular={} stencil={} lighting={}/{} vertexblend={},{} clipplanes={} wrap0={} \
         pointsprite={} zbias={}",
        match pass {
            Pass::Draw => "draw",
            Pass::Process => "process",
        },
        version,
        names::fvf(fvf),
        r(7),
        r(14),
        r(23),
        r(15),
        r(25),
        r(27),
        r(19),
        r(20),
        r(171),
        r(28),
        r(35),
        r(140),
        r(48),
        r(29),
        r(52),
        r(137),
        lights,
        r(151),
        r(167),
        r(152),
        r(128),
        r(156),
        r(47)
    );
    for stage in 0..8 {
        let t = |k: usize| stages.get(stage * 32 + k).copied().unwrap_or(0);
        // D3DTOP_DISABLE ends the cascade.
        if t(1) <= 1 {
            break;
        }
        let op = |x: u32| names::stage_op(x).map_or_else(|| names::number(x), str::to_string);
        key += &format!(
            " | s{} tex={} c={}({},{}) a={}({},{}) tci={} ttf={}",
            stage,
            textures.get(stage).copied().unwrap_or(false) as u32,
            op(t(1)),
            names::stage_arg(t(2)),
            names::stage_arg(t(3)),
            op(t(4)),
            names::stage_arg(t(5)),
            names::stage_arg(t(6)),
            names::stage_value(11, t(11), false),
            names::stage_value(24, t(24), false)
        );
        if matches!(t(1), 22..=24) {
            key += &format!(
                " bump=[{},{},{},{}] l={},{}",
                names::float(t(7)),
                names::float(t(8)),
                names::float(t(9)),
                names::float(t(10)),
                names::float(t(22)),
                names::float(t(23))
            );
        }
        if t(26) != 0 || t(27) != 0 || t(28) != 1 && t(28) != 0 {
            key += &format!(
                " arg0={},{} result={}",
                names::stage_arg(t(26)),
                names::stage_arg(t(27)),
                names::stage_arg(t(28))
            );
        }
    }
    record(cpu, key);
}

// Enumerations, whose callbacks a task (task.rs) calls.

/// A step of the enumeration task with scratch space at `scratch` and
/// the list name `list` ran: at a step after the first (`step`), the
/// callback returned `value` for the item given to it last; `next` is
/// what the step does now, which `describe` turns into the item it
/// gives the callback, from the callback's arguments.
pub(crate) fn enumerated(
    cpu: &mut Cpu,
    list: &str,
    scratch: u32,
    step: u16,
    value: u32,
    next: &Next,
    describe: impl FnOnce(&Cpu, &[u32]) -> String,
) {
    let Some(t) = cpu.bus.win32.trace3d.as_deref_mut() else {
        return;
    };
    // A list that ended without its callback returning (one that
    // faulted, or a process that went) left its item; a task starting
    // with the same scratch space has nothing to answer yet.
    if step == 0 {
        t.offered.remove(&scratch);
    }
    if step != 0
        && let Some(item) = t.offered.remove(&scratch)
    {
        let answer = match value {
            0 => "CANCEL".to_string(),
            1 => "OK".to_string(),
            v => names::number(v),
        };
        t.record(format!("{} -> {}", item, answer));
    }
    if let Next::Call { target, args, .. } = next
        && *target != 0
    {
        let item = format!("{} item {}", list, describe(cpu, args));
        if let Some(t) = cpu.bus.win32.trace3d.as_deref_mut() {
            t.offered.insert(scratch, item);
        }
    }
}

/// DirectDrawEnumerate's devices: (guid, description, name, context).
pub(crate) fn ddraw_device_item(cpu: &Cpu, args: &[u32]) -> String {
    format!(
        "{} \"{}\" \"{}\"",
        guid(cpu, args[0]),
        peek_str(cpu, args[1]).unwrap_or_default(),
        peek_str(cpu, args[2]).unwrap_or_default()
    )
}

/// An item with a DDSURFACEDESC at `args[at]`.
pub(crate) fn surface_desc_item(cpu: &Cpu, args: &[u32], at: usize) -> String {
    format!("[{}]", surface_desc_at(cpu, args[at]))
}

/// An item that is a DDPIXELFORMAT at `args[0]`.
pub(crate) fn pixel_format_item(cpu: &Cpu, args: &[u32]) -> String {
    pixel_format_at(cpu, args[0])
}

/// IDirect3D to 3's EnumDevices: (guid, description, name, HAL
/// description, HEL description, context), D3DDEVICEDESC's caps of the
/// HAL.
pub(crate) fn d3d_device_item(cpu: &Cpu, args: &[u32]) -> String {
    let w = |o: u32| peek(cpu, args[3].wrapping_add(o)).unwrap_or(0);
    format!(
        "{} \"{}\" devcaps=0x{:X} raster=0x{:X} texcaps=0x{:X} texblend=0x{:X} \
         depths=0x{:X} zdepths=0x{:X}",
        guid(cpu, args[0]),
        peek_str(cpu, args[2]).unwrap_or_default(),
        w(12),
        w(108),
        w(132),
        w(140),
        w(156),
        w(160)
    )
}

/// IDirect3D7's EnumDevices: (description, name, D3DDEVICEDESC7,
/// context).
pub(crate) fn d3d_device7_item(cpu: &Cpu, args: &[u32]) -> String {
    let w = |o: u32| peek(cpu, args[2].wrapping_add(o)).unwrap_or(0);
    format!(
        "\"{}\" devcaps=0x{:X} raster=0x{:X} texcaps=0x{:X} texblend=0x{:X} \
         depths=0x{:X} zdepths=0x{:X}",
        peek_str(cpu, args[1]).unwrap_or_default(),
        w(0),
        w(68),
        w(92),
        w(100),
        w(116),
        w(120)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointers_to_values_are_not_named_as_enums() {
        assert!(matches!(gl_args("glFogfv"), [(0, Gl::Enum)]));
        assert!(matches!(gl_args("glLightModelfv"), [(0, Gl::Enum)]));
        assert!(matches!(
            gl_args("glLightfv"),
            [(0, Gl::Enum), (1, Gl::Enum)]
        ));
    }

    #[test]
    fn an_event_is_written_once_and_counted_each_time() {
        let dir = std::env::temp_dir().join(format!("trace3d-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.trace");
        let mut t = Trace3d::create(&path).unwrap();
        for key in ["a", "b", "a", "a"] {
            t.record(key.to_string());
        }
        drop(t);
        let events = std::fs::read_to_string(&path).unwrap();
        assert_eq!(events, "     0 a\n     1 b\n");
        let counts = std::fs::read_to_string(counts_path(&path)).unwrap();
        assert_eq!(counts, "3\ta\n1\tb\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
