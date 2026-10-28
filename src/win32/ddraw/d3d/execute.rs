//! Execute buffers: IDirect3DExecuteBuffer and IDirect3DDevice's
//! Execute, Direct3D 3 and 5's way of drawing. An execute buffer holds
//! vertices and instructions: PROCESSVERTICES puts vertices through
//! transform and lighting (or takes them as they are) into the device's
//! vertex array for the buffer, and POINT, LINE and TRIANGLE draw with
//! the vertices of that array; the others set states and matrices, load
//! textures and branch on the buffer's status. The instructions are as
//! Wine's executebuffer.c runs them, which this is ported from.

use super::super::{
    DDERR_ALREADYINITIALIZED, DDERR_INVALIDOBJECT, DDERR_INVALIDPARAMS, DDERR_UNSUPPORTED,
};
use super::device::{self, HANDLE_MATRIX, HANDLE_TEXTURE, device_of, get};
use super::draw::setup_in;
use super::pipeline::{self, Layout, Prim, Processed};
use super::{
    D3D_OK, D3DERR_EXECUTE_LOCKED, D3DERR_EXECUTE_NOT_LOCKED, D3dObject, probe, read_bytes,
    read_dwords, write_dwords,
};
use crate::cpu::Cpu;
use crate::win32::{com, kernel32};

/// An execute buffer.
#[derive(Clone, Debug, Default)]
pub struct ExecuteBuffer {
    /// The device it was made by.
    pub device: u32,
    /// D3DEXECUTEBUFFERDESC's caps and size, and where it is: the
    /// program's memory, or pages of DirectDraw's (`owns`).
    pub caps: u32,
    pub size: u32,
    pub memory: u32,
    pub owns: bool,
    pub locked: bool,
    /// D3DEXECUTEDATA's words after its size: the vertices' offset and
    /// count, the instructions' offset and length, the homogeneous
    /// vertices' offset, and D3DSTATUS (flags, status, extent).
    pub data: [u32; 11],
}

crate::state_fields!(ExecuteBuffer {
    device,
    caps,
    size,
    memory,
    owns,
    locked,
    data
});

// D3DEXECUTEBUFFERDESC's flags.
const D3DDEB_BUFSIZE: u32 = 0x1;
const D3DDEB_CAPS: u32 = 0x2;
const D3DDEB_LPDATA: u32 = 0x4;

/// The most an execute buffer holds.
const MAX_SIZE: u32 = 0x100_0000;

fn buffer_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    let (_, id) = super::of(cpu, this).ok_or(DDERR_INVALIDOBJECT)?;
    get_buffer(cpu, id).ok_or(DDERR_INVALIDOBJECT)?;
    Ok(id)
}

fn get_buffer(cpu: &mut Cpu, id: u32) -> Option<&mut ExecuteBuffer> {
    super::object(cpu, id)?.execute.as_mut()
}

/// The execute buffer `e` goes: its memory with it, if it was
/// DirectDraw's.
pub fn destroyed(cpu: &mut Cpu, e: ExecuteBuffer) {
    if e.owns && e.memory != 0 {
        kernel32::free_pages(cpu, e.memory);
    }
}

/// CreateExecuteBuffer(this, desc, out, outer).
pub fn create_execute_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, desc, out, outer] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !com::write_out(cpu, out, 0) {
        return Some(DDERR_INVALIDPARAMS);
    }
    if outer != 0 {
        return Some(com::CLASS_E_NOAGGREGATION);
    }
    if !probe(cpu, desc, 20, false) || cpu.bus.guest_read_32(desc) != 20 {
        return Some(DDERR_INVALIDPARAMS);
    }
    let [_, flags, caps, size, data] = read_dwords::<5>(cpu, desc);
    if flags & D3DDEB_BUFSIZE == 0 || size == 0 || size > MAX_SIZE {
        return Some(DDERR_INVALIDPARAMS);
    }
    let (memory, owns) = if flags & D3DDEB_LPDATA != 0 && data != 0 {
        if !probe(cpu, data, size, true) {
            return Some(DDERR_INVALIDPARAMS);
        }
        (data, false)
    } else {
        let Some(at) = kernel32::alloc_pages(cpu, size) else {
            return Some(com::E_OUTOFMEMORY);
        };
        (at, true)
    };
    let dd = get(cpu, id).map_or(0, |d| d.ddraw);
    let object = D3dObject {
        execute: Some(ExecuteBuffer {
            device: id,
            caps: if flags & D3DDEB_CAPS != 0 { caps } else { 0 },
            size,
            memory,
            owns,
            ..ExecuteBuffer::default()
        }),
        ..D3dObject::default()
    };
    Some(
        match super::add(cpu, dd, object, super::super::V_EXECBUF, out) {
            Ok(_) => D3D_OK,
            Err(e) => {
                if owns {
                    kernel32::free_pages(cpu, memory);
                }
                e
            }
        },
    )
}

/// Initialize(this, device, desc): it was.
pub fn initialize(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DDERR_ALREADYINITIALIZED)
}

/// Lock(this, desc): where the buffer is, and its size and caps.
pub fn lock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, desc] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(buffer_of(cpu, this));
    if !probe(cpu, desc, 20, true) || cpu.bus.guest_read_32(desc) != 20 {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(e) = get_buffer(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if e.locked {
        return Some(D3DERR_EXECUTE_LOCKED);
    }
    e.locked = true;
    let words = [
        D3DDEB_BUFSIZE | D3DDEB_CAPS | D3DDEB_LPDATA,
        e.caps,
        e.size,
        e.memory,
    ];
    write_dwords(cpu, desc + 4, &words);
    Some(D3D_OK)
}

/// Unlock(this).
pub fn unlock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(buffer_of(cpu, args[0]));
    let Some(e) = get_buffer(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if !e.locked {
        return Some(D3DERR_EXECUTE_NOT_LOCKED);
    }
    e.locked = false;
    Some(D3D_OK)
}

/// D3DEXECUTEDATA's size.
const DATA_SIZE: u32 = 48;

/// SetExecuteData(this, data).
pub fn set_execute_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(buffer_of(cpu, this));
    if !probe(cpu, at, DATA_SIZE, false) || cpu.bus.guest_read_32(at) != DATA_SIZE {
        return Some(DDERR_INVALIDPARAMS);
    }
    let data = read_dwords::<11>(cpu, at + 4);
    if let Some(e) = get_buffer(cpu, id) {
        e.data = data;
    }
    Some(D3D_OK)
}

/// GetExecuteData(this, data).
pub fn get_execute_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(buffer_of(cpu, this));
    if !probe(cpu, at, DATA_SIZE, true) || cpu.bus.guest_read_32(at) != DATA_SIZE {
        return Some(DDERR_INVALIDPARAMS);
    }
    let data = get_buffer(cpu, id).map_or([0; 11], |e| e.data);
    write_dwords(cpu, at + 4, &data);
    Some(D3D_OK)
}

/// Validate(this, offset, callback, context, reserved): Windows' never
/// did.
pub fn validate(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DDERR_UNSUPPORTED)
}

/// Optimize(this, flags).
pub fn optimize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(buffer_of(cpu, args[0]));
    Some(D3D_OK)
}

// The instructions.
const D3DOP_POINT: u8 = 1;
const D3DOP_LINE: u8 = 2;
const D3DOP_TRIANGLE: u8 = 3;
const D3DOP_MATRIXLOAD: u8 = 4;
const D3DOP_MATRIXMULTIPLY: u8 = 5;
const D3DOP_STATETRANSFORM: u8 = 6;
const D3DOP_STATELIGHT: u8 = 7;
const D3DOP_STATERENDER: u8 = 8;
const D3DOP_PROCESSVERTICES: u8 = 9;
const D3DOP_TEXTURELOAD: u8 = 10;
const D3DOP_EXIT: u8 = 11;
const D3DOP_BRANCHFORWARD: u8 = 12;
const D3DOP_SETSTATUS: u8 = 14;

// D3DPROCESSVERTICES_ operations.
const PV_OPMASK: u32 = 0x7;
const PV_TRANSFORMLIGHT: u32 = 0;
const PV_TRANSFORM: u32 = 1;
const PV_UPDATEEXTENTS: u32 = 0x8;

/// The most instructions an Execute runs: a branch can't go back, so
/// a buffer's run ends; this bounds one that is all empty instructions.
const MAX_INSTRUCTIONS: u32 = 0x10_0000;

/// Execute(this, buffer, viewport, flags): run the buffer's
/// instructions, drawing in the viewport.
pub fn execute(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, buffer, viewport, _] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    let Ok(bid) = buffer_of(cpu, buffer) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let Ok(vid) = super::viewport::viewport_of(cpu, viewport) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let Some(e) = get_buffer(cpu, bid).cloned() else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if e.locked {
        return Some(D3DERR_EXECUTE_LOCKED);
    }
    let version = device::version(vtable);
    let bytes = read_bytes(cpu, e.memory, e.size);
    let mut run = Run {
        id,
        version,
        viewport: vid,
        bytes,
        vertices: vec![Processed::default(); e.data[1].min(0x1_0000) as usize],
        data: e.data,
        setup: None,
    };
    run.go(cpu);
    if let Some(e) = get_buffer(cpu, bid) {
        e.data = run.data;
    }
    Some(D3D_OK)
}

/// An Execute under way.
struct Run {
    id: u32,
    version: u32,
    viewport: u32,
    /// The buffer's bytes.
    bytes: Vec<u8>,
    /// The vertex array, and the setup its vertices were processed with.
    vertices: Vec<Processed>,
    setup: Option<pipeline::Setup>,
    /// D3DEXECUTEDATA's words after the size.
    data: [u32; 11],
}

impl Run {
    fn u16_at(&self, at: usize) -> u32 {
        self.bytes
            .get(at..at + 2)
            .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as u32)
    }

    fn u32_at(&self, at: usize) -> u32 {
        self.bytes
            .get(at..at + 4)
            .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn go(&mut self, cpu: &mut Cpu) {
        let mut at = self.data[2] as usize;
        for _ in 0..MAX_INSTRUCTIONS {
            if at.saturating_add(4) > self.bytes.len() {
                return;
            }
            let current = at;
            let (op, size, count) = (
                self.bytes[at],
                self.bytes[at + 1] as usize,
                self.u16_at(at + 2) as usize,
            );
            at += 4;
            match op {
                D3DOP_POINT | D3DOP_LINE | D3DOP_TRIANGLE => {
                    self.draw(cpu, op, at, size, count);
                    at += count * size;
                }
                D3DOP_MATRIXLOAD | D3DOP_MATRIXMULTIPLY => {
                    for i in 0..count {
                        self.matrix(cpu, op, at + i * size);
                    }
                    at += count * size;
                }
                D3DOP_STATETRANSFORM | D3DOP_STATELIGHT | D3DOP_STATERENDER => {
                    for i in 0..count {
                        let (s, v) = (self.u32_at(at + i * size), self.u32_at(at + i * size + 4));
                        self.state(cpu, op, s, v);
                    }
                    at += count * size;
                }
                D3DOP_PROCESSVERTICES => {
                    for i in 0..count {
                        self.process(cpu, at + i * size);
                    }
                    at += count * size;
                }
                D3DOP_TEXTURELOAD => {
                    for i in 0..count {
                        let (dest, src) =
                            (self.u32_at(at + i * size), self.u32_at(at + i * size + 4));
                        let d = get(cpu, self.id).cloned().unwrap_or_default();
                        let surface = |h: u32| d.handle(h, HANDLE_TEXTURE).map(|h| h.object);
                        if let (Some(dest), Some(src)) = (surface(dest), surface(src)) {
                            let _ = super::texture::load_surfaces(cpu, dest, src);
                        }
                    }
                    at += count * size;
                }
                D3DOP_EXIT => return,
                D3DOP_BRANCHFORWARD => {
                    let mut jumped = false;
                    for i in 0..count {
                        let b = at + i * size;
                        let (mask, value, negate, offset) = (
                            self.u32_at(b),
                            self.u32_at(b + 4),
                            self.u32_at(b + 8),
                            self.u32_at(b + 12),
                        );
                        let taken = (self.data[6] & mask == value) != (negate != 0);
                        if taken && offset != 0 {
                            at = current.saturating_add(offset as usize);
                            jumped = true;
                            break;
                        }
                    }
                    if !jumped {
                        at += count * size;
                    }
                }
                D3DOP_SETSTATUS => {
                    for i in 0..count {
                        for k in 0..6 {
                            self.data[5 + k] = self.u32_at(at + i * size + 4 * k);
                        }
                    }
                    at += count * size;
                }
                _ => at += count * size,
            }
        }
    }

    fn matrix(&self, cpu: &mut Cpu, op: u8, at: usize) {
        let Some(d) = get(cpu, self.id) else {
            return;
        };
        let m = |h: u32| d.handle(h, HANDLE_MATRIX).map(|h| h.matrix);
        let dest = self.u32_at(at);
        let value = if op == D3DOP_MATRIXLOAD {
            m(self.u32_at(at + 4))
        } else {
            // Wine's multiply_matrix(dest, src1, src2): src2 times src1
            // as Direct3D multiplies.
            match (m(self.u32_at(at + 4)), m(self.u32_at(at + 8))) {
                (Some(a), Some(b)) => Some(pipeline::multiply(&b, &a)),
                _ => None,
            }
        };
        if let (Some(v), Some(h)) = (value, d.handle_mut(dest, HANDLE_MATRIX)) {
            h.matrix = v;
            for i in 0..3 {
                if d.bound[i] == dest {
                    d.transforms[i + 1] = v;
                }
            }
        }
    }

    fn state(&mut self, cpu: &mut Cpu, op: u8, s: u32, v: u32) {
        match op {
            D3DOP_STATETRANSFORM => {
                let Some(d) = get(cpu, self.id) else {
                    return;
                };
                if !(1..=3).contains(&s) {
                    return;
                }
                if let Some(m) = d.handle(v, HANDLE_MATRIX).map(|h| h.matrix) {
                    d.transforms[s as usize] = m;
                    d.bound[s as usize - 1] = v;
                }
            }
            D3DOP_STATELIGHT => {
                let this = self.device_iface(cpu);
                super::states::set_light_state(cpu, &[this, s, v]);
            }
            _ => {
                let this = self.device_iface(cpu);
                super::states::set_render_state(cpu, &[this, s, v]);
            }
        }
    }

    /// The device's interface of the version Execute was called through.
    fn device_iface(&self, cpu: &mut Cpu) -> u32 {
        let v = match self.version {
            1 => super::super::V_DEVICE1,
            2 => super::super::V_DEVICE2,
            _ => super::super::V_DEVICE3,
        };
        super::super::iface(cpu, self.id, v)
    }

    /// D3DPROCESSVERTICES at `at`: (flags, start, dest, count).
    fn process(&mut self, cpu: &mut Cpu, at: usize) {
        let flags = self.u32_at(at);
        let start = self.u16_at(at + 4);
        let dest = self.u16_at(at + 6) as usize;
        let count = self.u32_at(at + 8);
        let (fvf, lit) = match flags & PV_OPMASK {
            PV_TRANSFORMLIGHT => (pipeline::FVF_VERTEX, true),
            PV_TRANSFORM => (pipeline::FVF_LVERTEX, false),
            _ => (pipeline::FVF_TLVERTEX, false),
        };
        let Some(layout) = Layout::of(fvf) else {
            return;
        };
        let lighting = if lit {
            0
        } else {
            super::draw::D3DDP_DONOTLIGHT
        };
        let Some(mut setup) =
            setup_in(cpu, self.id, self.version, &layout, lighting, self.viewport)
        else {
            return;
        };
        if !lit {
            setup.lighting = None;
        }
        let first = self.data[0] as usize + start as usize * layout.size as usize;
        let mut input = Vec::new();
        for i in 0..count as usize {
            let b = first + i * layout.size as usize;
            match self.bytes.get(b..b + layout.size as usize) {
                Some(v) => input.push(layout.read(v)),
                None => break,
            }
        }
        let out = pipeline::process(&setup, &input, &layout);
        let (intersection, union) = pipeline::clip_union(&out);
        self.data[6] |= union | intersection << 12;
        if flags & PV_UPDATEEXTENTS != 0
            && let Some(e) = pipeline::extents(&setup, &out)
        {
            let r = |v: f32| v as i32 as u32;
            self.data[7] = r(e[0]);
            self.data[8] = r(e[2]);
            self.data[9] = r(e[1]);
            self.data[10] = r(e[3]);
        }
        if let Some(d) = get(cpu, self.id) {
            d.stats[4] = d.stats[4].wrapping_add(out.len() as u32);
        }
        for (i, v) in out.into_iter().enumerate() {
            if let Some(slot) = self.vertices.get_mut(dest + i) {
                *slot = v;
            }
        }
        self.setup = Some(setup);
    }

    /// POINT, LINE or TRIANGLE: `count` of them, `size` bytes each.
    fn draw(&mut self, cpu: &mut Cpu, op: u8, at: usize, size: usize, count: usize) {
        let mut prims = Vec::new();
        for i in 0..count {
            let b = at + i * size;
            match op {
                D3DOP_POINT => {
                    let (n, first) = (self.u16_at(b), self.u16_at(b + 2));
                    prims.extend((first..first + n).map(Prim::Point));
                }
                D3DOP_LINE => {
                    let (a, z) = (self.u16_at(b), self.u16_at(b + 2));
                    prims.push(Prim::Line(a, z, a));
                }
                _ => {
                    let v = [self.u16_at(b), self.u16_at(b + 2), self.u16_at(b + 4)];
                    // D3DTRIANGLE_EDGEENABLE1 to 3: the wireframe's edges.
                    let edges = (self.u16_at(b + 6) >> 8 & 7) as u8;
                    prims.push(Prim::Triangle(v, v[0], edges));
                }
            }
        }
        let setup = match &self.setup {
            Some(s) => s.clone(),
            None => {
                let layout = Layout::of(pipeline::FVF_TLVERTEX).unwrap_or_default();
                match setup_in(cpu, self.id, self.version, &layout, 0, self.viewport) {
                    Some(s) => s,
                    None => return,
                }
            }
        };
        super::draw::draw_processed(cpu, self.id, &setup, &self.vertices, &prims);
    }
}
