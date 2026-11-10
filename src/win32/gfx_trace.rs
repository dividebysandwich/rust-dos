//! The 3D trace: what a program asks of DirectDraw, Direct3D 3 to 8 and
//! OpenGL, for deciding what a 3D card has to offer. Off unless the
//! environment variable `RUST_DOS_3D_TRACE` names a file.
//!
//! Each event is a line of text, the key. The file gets a key the first
//! time it happens, numbered in order; `FILE.counts` gets every key with
//! the number of times it happened, rewritten every two seconds of the
//! host's time while events come. The events are the calls of DDRAW,
//! D3D8, OPENGL32 and GLU32 with the arguments that tell what was asked
//! (states and their values, formats, sizes, flags) and their results,
//! the 3D DLLs and functions asked for by name through KERNEL32, GDI's
//! pixel format calls, and a line for each different set of states a
//! draw used (`draw`), which shows the texture stages as the card got
//! them.
//!
//! `RUST_DOS_3D_TRACE_NO_WBUFFER=1`, with the trace on, takes
//! D3DPRASTERCAPS_WBUFFER out of the HAL's capabilities, so that a run
//! with it and one without show whether a program asks for a W buffer
//! only when it is offered. Nothing else the trace does changes what
//! the program sees.

use crate::cpu::Cpu;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

struct Trace {
    path: String,
    events: File,
    counts: HashMap<String, u64>,
    order: Vec<String>,
    written: Instant,
}

impl Trace {
    fn write_counts(&mut self) {
        let mut text = String::new();
        for key in &self.order {
            text.push_str(&format!("{}\t{}\n", self.counts[key], key));
        }
        let _ = std::fs::write(format!("{}.counts", self.path), text);
        let _ = self.events.flush();
        self.written = Instant::now();
    }
}

fn trace() -> Option<&'static Mutex<Trace>> {
    static TRACE: OnceLock<Option<Mutex<Trace>>> = OnceLock::new();
    TRACE
        .get_or_init(|| {
            let path = std::env::var("RUST_DOS_3D_TRACE").ok()?;
            if path.is_empty() {
                return None;
            }
            let events = File::create(&path).ok()?;
            Some(Mutex::new(Trace {
                path,
                events,
                counts: HashMap::new(),
                order: Vec::new(),
                written: Instant::now(),
            }))
        })
        .as_ref()
}

/// Whether the trace is on.
pub fn on() -> bool {
    trace().is_some()
}

/// Whether the HAL leaves the W buffer out of its capabilities.
pub fn hide_w_buffer() -> bool {
    static HIDE: OnceLock<bool> = OnceLock::new();
    *HIDE.get_or_init(|| {
        let hide = on() && std::env::var("RUST_DOS_3D_TRACE_NO_WBUFFER").is_ok_and(|v| v == "1");
        if on() {
            record(format!("config w_buffer_cap_offered={}", !hide));
        }
        hide
    })
}

/// Count the event `key`.
pub fn record(key: String) {
    let Some(trace) = trace() else {
        return;
    };
    let mut t = trace.lock().unwrap_or_else(|e| e.into_inner());
    let count = t.counts.entry(key.clone()).or_insert(0);
    *count += 1;
    if *count == 1 {
        let n = t.order.len();
        let _ = writeln!(t.events, "{:6} {}", n, key);
        t.order.push(key);
    }
    if t.written.elapsed() >= Duration::from_secs(2) {
        t.write_counts();
    }
}

/// The word at `at` in the running process, without a page fault.
fn peek(cpu: &Cpu, at: u32) -> Option<u32> {
    if at == 0 {
        return None;
    }
    super::peek_32(cpu, at)
}

fn peek_bytes(cpu: &Cpu, at: u32, len: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(len + 3);
    for i in (0..len).step_by(4) {
        bytes.extend_from_slice(&peek(cpu, at.wrapping_add(i as u32))?.to_le_bytes());
    }
    bytes.truncate(len);
    Some(bytes)
}

/// The string at `at`, up to 128 characters, if its pages are there.
fn peek_str(cpu: &Cpu, at: u32) -> Option<String> {
    let mut s = String::new();
    for i in 0..32 {
        for b in peek(cpu, at.wrapping_add(4 * i))?.to_le_bytes() {
            if b == 0 {
                return Some(s);
            }
            s.push(b as char);
        }
    }
    Some(s)
}

fn guid(cpu: &Cpu, at: u32) -> String {
    let Some(b) = peek_bytes(cpu, at, 16) else {
        return "?".to_string();
    };
    format!(
        "{:08X}-{:04X}-{:04X}-{}",
        u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        u16::from_le_bytes([b[4], b[5]]),
        u16::from_le_bytes([b[6], b[7]]),
        b[8..]
            .iter()
            .map(|x| format!("{:02X}", x))
            .collect::<String>()
    )
}

/// A value as the trace shows it: small numbers in decimal, the rest in
/// hexadecimal.
fn v(x: u32) -> String {
    if x < 0x1_0000 {
        x.to_string()
    } else {
        format!("0x{:X}", x)
    }
}

/// A float argument of OpenGL's, which is often a whole enum.
fn f(x: u32) -> String {
    let x = f32::from_bits(x);
    if x.fract() == 0.0 && x.abs() < 1e9 {
        v(x as i64 as u32)
    } else {
        x.to_string()
    }
}

fn fourcc(x: u32) -> String {
    if x == 0 {
        return "0".to_string();
    }
    let b = x.to_le_bytes();
    if b.iter().all(|c| c.is_ascii_graphic()) {
        b.iter().map(|&c| c as char).collect()
    } else {
        format!("0x{:X}", x)
    }
}

/// DDPIXELFORMAT at `at`: flags, FourCC, bits and masks.
fn pixel_format(cpu: &Cpu, at: u32) -> String {
    let w: Vec<u32> = (0..8).map(|i| peek(cpu, at + 4 * i).unwrap_or(0)).collect();
    format!(
        "[fl=0x{:X} 4cc={} bits={} r=0x{:X} g=0x{:X} b=0x{:X} a=0x{:X}]",
        w[1],
        fourcc(w[2]),
        w[3],
        w[4],
        w[5],
        w[6],
        w[7]
    )
}

/// DDSURFACEDESC or DDSURFACEDESC2 at `at`.
fn surface_desc(cpu: &Cpu, at: u32) -> String {
    let w = |o: u32| peek(cpu, at + o).unwrap_or(0);
    let (size, flags) = (w(0), w(4));
    let mut s = format!("flags=0x{:X} caps=0x{:X}", flags, w(104));
    if size >= 124 {
        s += &format!("/0x{:X}", w(108));
    }
    if flags & 0x6 != 0 {
        s += &format!(" {}x{}", w(12), w(8));
    }
    if flags & 0x20 != 0 {
        s += &format!(" backbuffers={}", w(20));
    }
    if flags & 0x2_0000 != 0 {
        s += &format!(" mips={}", w(24));
    }
    if flags & 0x40 != 0 {
        s += &format!(" zbits={}", w(24));
    }
    if flags & 0x1000 != 0 {
        s += &format!(" pf={}", pixel_format(cpu, at + 72));
    }
    if flags & 0x1_0000 != 0 {
        s += " cksrcblt";
    }
    s
}

/// D3DPRESENT_PARAMETERS at `at`.
fn present_params(cpu: &Cpu, at: u32) -> String {
    let w: Vec<u32> = (0..13)
        .map(|i| peek(cpu, at + 4 * i).unwrap_or(0))
        .collect();
    format!(
        "pp[{}x{} fmt={} n={} ms={} swap={} windowed={} autodepth={} dfmt={} flags=0x{:X}]",
        w[0], w[1], w[2], w[3], w[4], w[5], w[7], w[8], w[9], w[10]
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

/// The arguments `which` of `args`, in parentheses.
fn pick(args: &[u32], which: &[usize]) -> String {
    let list: Vec<String> = which
        .iter()
        .map(|&i| args.get(i).map_or("?".to_string(), |&a| v(a)))
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

/// A call of a built-in function, after it returned `result`.
pub fn call(cpu: &Cpu, module: &str, name: &str, args: &[u32], result: Option<u32>) {
    // Short argument lists read as zeros past their end.
    let mut padded = [0u32; 16];
    for (to, from) in padded.iter_mut().zip(args) {
        *to = *from;
    }
    let args = &padded[..];
    let detail = match module {
        "DDRAW" | "D3D8" => com_detail(cpu, module, name, args, result),
        "OPENGL32" => gl_detail(cpu, name, args, result),
        "GLU32" => Some(String::new()),
        "KERNEL32" => return kernel_call(cpu, name, args, result),
        "GDI32" => match name {
            "ChoosePixelFormat" => Some(format!(
                "{} = {}",
                pfd(cpu, args[1]),
                v(result.unwrap_or(0))
            )),
            "SetPixelFormat" => Some(format!("({}) = {}", v(args[1]), v(result.unwrap_or(0)))),
            "DescribePixelFormat" if args[1] != 0 && args[3] != 0 => {
                Some(format!("({}) {}", v(args[1]), pfd(cpu, args[3])))
            }
            _ => None,
        },
        _ => None,
    };
    if let Some(detail) = detail {
        record(format!("{}.{}{}", module, name, detail));
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
        "QueryInterface" => format!(" {{{}}} = {}", guid(cpu, args[1]), r),
        "SetRenderState" | "SetLightState" | "LightEnable" => format!("{} = {}", a(&[1, 2]), r),
        "SetTextureStageState" => format!("{} = {}", a(&[1, 2, 3]), r),
        "SetTransform" | "MultiplyTransform" | "SetClipPlane" => format!("{} = {}", a(&[1]), r),
        "SetTexture" => format!("({},{}) = {}", args[1], (args[2] != 0) as u32, r),
        "SetLight" => {
            let (index, at) = if name.starts_with("IDirect3DLight_") {
                (None, args[1] + 4)
            } else {
                (Some(args[1]), args[2])
            };
            let kind = peek(cpu, at).unwrap_or(0);
            format!("({:?} type={}) = {}", index, kind, r)
        }
        "DrawPrimitive"
        | "DrawIndexedPrimitive"
        | "DrawPrimitiveStrided"
        | "DrawIndexedPrimitiveStrided"
            if !d8 =>
        {
            format!("(prim={},vt=0x{:X}) = {}", args[1], args[2], r)
        }
        "DrawPrimitive"
        | "DrawIndexedPrimitive"
        | "DrawPrimitiveUP"
        | "DrawIndexedPrimitiveUP"
        | "DrawPrimitiveVB"
        | "DrawIndexedPrimitiveVB" => format!("(prim={}) = {}", args[1], r),
        "SetVertexShader" => format!("(0x{:X}) = {}", args[1], r),
        "CreateVertexBuffer" if d8 => format!("{} = {}", a(&[2, 3, 4]), r),
        "CreateVertexBuffer" => {
            let w = |o: u32| peek(cpu, args[1] + o).unwrap_or(0);
            format!("(caps=0x{:X} fvf=0x{:X}) = {}", w(4), w(8), r)
        }
        "CreateIndexBuffer" => format!("{} = {}", a(&[2, 3, 4]), r),
        "CreateTexture" => format!("{} = {}", a(&[1, 2, 3, 4, 5, 6]), r),
        "CreateCubeTexture" => format!("{} = {}", a(&[1, 2, 3, 4, 5]), r),
        "CreateVolumeTexture" => format!("{} = {}", a(&[1, 2, 3, 4, 5, 6, 7]), r),
        "CreateRenderTarget" => format!("{} = {}", a(&[1, 2, 3, 4, 5]), r),
        "CreateDepthStencilSurface" => format!("{} = {}", a(&[1, 2, 3, 4]), r),
        "CreateImageSurface" => format!("{} = {}", a(&[1, 2, 3]), r),
        "CheckDeviceFormat" => format!("{} = {}", a(&[1, 2, 3, 4, 5, 6]), r),
        "CheckDeviceType" | "CheckDepthStencilMatch" => {
            format!("{} = {}", a(&[1, 2, 3, 4, 5]), r)
        }
        "CheckMultiSampleType" => format!("{} = {}", a(&[1, 2, 3, 4]), r),
        "GetDeviceCaps" if !name.starts_with("Device8") => format!("{} = {}", a(&[1, 2]), r),
        "CreateDevice" if d8 => {
            format!("{} {} = {}", a(&[1, 2, 4]), present_params(cpu, args[5]), r)
        }
        "CreateDevice" => format!(" {{{}}} = {}", guid(cpu, args[1]), r),
        "Reset" => format!(" {} = {}", present_params(cpu, args[1]), r),
        "ValidateDevice" => {
            let passes = peek(cpu, args[1]);
            format!(" passes={:?} = {}", passes, r)
        }
        "CreateSurface" => format!(" {} = {}", surface_desc(cpu, args[1]), r),
        "SetDisplayMode" => format!("{} = {}", a(&[1, 2, 3]), r),
        "SetCooperativeLevel" => format!("(0x{:X}) = {}", args[2], r),
        _ => format!(" = {}", r),
    };
    Some(detail)
}

fn gl_detail(cpu: &Cpu, name: &str, args: &[u32], result: Option<u32>) -> Option<String> {
    let a = |which: &[usize]| pick(args, which);
    let fl = |which: &[usize]| {
        let list: Vec<String> = which.iter().map(|&i| f(args[i])).collect();
        format!("({})", list.join(","))
    };
    let detail = match name {
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
        | "glBegin"
        | "glDrawBuffer"
        | "glReadBuffer"
        | "glDepthMask"
        | "glActiveTextureARB"
        | "glClientActiveTextureARB"
        | "glClipPlane"
        | "glDrawArrays"
        | "glLockArraysEXT" => a(&[0]),
        "glGetString" => {
            let s = result.and_then(|at| peek_str(cpu, at)).unwrap_or_default();
            format!("({}) = \"{}\"", v(args[0]), s)
        }
        "glBlendFunc" | "glHint" | "glPolygonMode" | "glPixelStorei" | "glColorMaterial"
        | "glFogi" | "glLightModeli" | "glTexParameteri" => a(&[0, 1, 2]),
        "glStencilOp" | "glTexEnvi" | "glTexGeni" => a(&[0, 1, 2]),
        "glAlphaFunc" | "glStencilFunc" | "glLightf" | "glLightfv" | "glLighti" | "glMaterialf"
        | "glMaterialfv" | "glDrawElements" | "glTexEnvfv" | "glTexEnviv" | "glTexGenfv"
        | "glTexParameterfv" | "glTexParameteriv" | "glFogfv" | "glLightModelfv" => a(&[0, 1]),
        "glTexEnvf" | "glTexParameterf" | "glTexGenf" => {
            format!("({},{},{})", v(args[0]), v(args[1]), f(args[2]))
        }
        "glFogf" | "glLightModelf" => format!("({},{})", v(args[0]), fl(&[1])),
        "glTexImage2D" => format!(
            "(target={} level0={} internal={} {}x{} border={} format={} type={})",
            v(args[0]),
            (args[1] == 0) as u32,
            v(args[2]),
            args[3],
            args[4],
            args[5],
            v(args[6]),
            v(args[7])
        ),
        "glTexImage1D" => a(&[0, 2, 3, 5, 6]),
        "glCopyTexImage2D" => a(&[0, 2, 5, 6]),
        "glTexSubImage2D" => a(&[0, 6, 7]),
        "glColorTableEXT" => a(&[0, 1, 2, 3, 4]),
        "glCompressedTexImage2DARB" => a(&[0, 2, 3, 4]),
        "wglGetProcAddress" => {
            let s = peek_str(cpu, args[0]).unwrap_or_default();
            format!("(\"{}\") found={}", s, result.unwrap_or(0) != 0)
        }
        "wglChoosePixelFormat" => format!(" {} = {}", pfd(cpu, args[1]), v(result.unwrap_or(0))),
        "wglSetPixelFormat" => format!("({}) = {}", v(args[1]), v(result.unwrap_or(0))),
        "wglCreateContext" | "wglMakeCurrent" => {
            format!(" = {}", (result.unwrap_or(0) != 0) as u32)
        }
        _ => String::new(),
    };
    Some(detail)
}

fn kernel_call(cpu: &Cpu, name: &str, args: &[u32], result: Option<u32>) {
    let at = match name {
        "GetProcAddress" => args[1],
        "LoadLibraryA" | "LoadLibraryExA" | "GetModuleHandleA" => args[0],
        _ => return,
    };
    if at < 0x1_0000 {
        return;
    }
    let Some(s) = peek_str(cpu, at) else {
        return;
    };
    let lower = s.to_ascii_lowercase();
    let wanted = if name == "GetProcAddress" {
        ["gl", "wgl", "direct", "d3d"]
            .iter()
            .any(|p| lower.starts_with(p))
    } else {
        ["opengl", "d3d", "ddraw", "glide", "3dfx", "gl"]
            .iter()
            .any(|p| lower.contains(p))
    };
    if wanted {
        let found = result.unwrap_or(0) != 0;
        record(format!("KERNEL32.{}(\"{}\") found={}", name, s, found));
    }
}

/// A draw: the device's version, the vertices' FVF, and the states that
/// say how it is drawn.
pub fn draw(
    version: u32,
    fvf: u32,
    render: &[u32],
    stages: &[u32],
    textures: &[bool],
    lights: usize,
) {
    if !on() {
        return;
    }
    let r = |s: usize| render.get(s).copied().unwrap_or(0);
    // ZENABLE, ZWRITEENABLE, ZFUNC; ALPHATESTENABLE, ALPHAFUNC;
    // ALPHABLENDENABLE, SRCBLEND, DESTBLEND; FOGENABLE, FOGTABLEMODE,
    // FOGVERTEXMODE, RANGEFOGENABLE; SPECULARENABLE; STENCILENABLE;
    // LIGHTING; VERTEXBLEND; CLIPPLANEENABLE; WRAP0; BLENDOP;
    // INDEXEDVERTEXBLENDENABLE; POINTSPRITEENABLE; ZBIAS.
    let mut s = format!(
        "draw v{} fvf=0x{:X} z={},{},{} at={},{} ab={},{},{} fog={},{},{},{} spec={} stencil={} light={}/{} vblend={} clipplanes=0x{:X} wrap0=0x{:X} blendop={} ivb={} psprite={} zbias={}",
        version,
        fvf,
        r(7),
        r(14),
        r(23),
        r(15),
        r(25),
        r(27),
        r(19),
        r(20),
        r(28),
        r(35),
        r(140),
        r(48),
        r(29),
        r(52),
        r(137),
        lights,
        r(151),
        r(152),
        r(128),
        r(171),
        r(167),
        r(156),
        r(47)
    );
    for stage in 0..8 {
        let t = |k: usize| stages.get(stage * 32 + k).copied().unwrap_or(0);
        // COLOROP 1 (DISABLE ends the cascade).
        if t(1) == 1 || t(1) == 0 {
            break;
        }
        s += &format!(
            " | s{} tex={} c={}({},{}) a={}({},{}) tci=0x{:X} ttf={}",
            stage,
            textures.get(stage).copied().unwrap_or(false) as u32,
            t(1),
            t(2),
            t(3),
            t(4),
            t(5),
            t(6),
            t(11),
            t(24)
        );
        if matches!(t(1), 22..=24) {
            s += &format!(
                " bump=[{},{},{},{}] l={},{}",
                f(t(7)),
                f(t(8)),
                f(t(9)),
                f(t(10)),
                f(t(22)),
                f(t(23))
            );
        }
        if t(26) != 0 || t(27) != 0 || t(28) != 0 {
            s += &format!(" arg0={},{} result={}", t(26), t(27), t(28));
        }
    }
    record(s);
}
