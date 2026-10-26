//! Vertex shaders: declarations, which say where a vertex's registers
//! come from in the streams, and functions, vs.1.0 and 1.1 programs,
//! which run here on each vertex (software vertex processing) in place
//! of transform and lighting. A shader without a function is drawn by
//! the fixed-function pipeline, its registers taken as the parts of an
//! FVF vertex. Pixel shaders there are none of: the caps say so, and
//! CreatePixelShader fails.
//!
//! What a function writes goes to the HAL as the pipeline's vertices
//! do: oPos as clip coordinates, oD0 and oD1 as the colors, oT0 to oT7
//! as the texture coordinates and oFog as the vertex fog. oPts has no
//! effect, as the card draws points one pixel wide.

use super::device::{self, Device, this_device};
use super::{D3D_OK, D3DERR_INVALIDCALL};
use crate::cpu::Cpu;

/// A vertex shader: its declaration's tokens and its function's (none
/// for a fixed-function one).
#[derive(Clone, Debug, Default)]
pub struct Shader {
    pub decl: Vec<u32>,
    pub function: Vec<u32>,
}

crate::state_fields!(Shader { decl, function });

/// The first handle of a shader's: handles at or below it are FVF codes.
const HANDLE_BASE: u32 = 0xF000_0000;

/// Whether `h` is a vertex shader's handle (or an FVF code).
pub fn is_handle(h: u32) -> bool {
    h > HANDLE_BASE
}

/// The shader of the handle `h`.
pub fn get_shader(d: &Device, h: u32) -> Option<&Shader> {
    let i = h.checked_sub(HANDLE_BASE + 1)? as usize;
    d.shaders.get(i)?.as_ref()
}

/// The pixel shader of the handle `h`: there are none.
pub fn get_pixel(_: &Device, _: u32) -> Option<()> {
    None
}

// Declarations.

/// D3DVSDT_ types: their sizes in bytes.
pub fn type_size(t: u32) -> Option<u32> {
    Some(match t {
        0 => 4,
        1 => 8,
        2 => 12,
        3 => 16,
        4 | 5 => 4,
        6 => 4,
        7 => 8,
        _ => return None,
    })
}

/// A register a declaration loads from a stream: the stream, the
/// offset in its vertices, the D3DVSDT_ type and the register.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Input {
    pub stream: u32,
    pub offset: u32,
    pub kind: u32,
    pub reg: u32,
}

/// What a declaration says: the inputs, and the constants it loads.
#[derive(Clone, Debug, Default)]
pub struct Decl {
    pub inputs: Vec<Input>,
    pub constants: Vec<(u32, [u32; 4])>,
}

const DECL_END: u32 = 0xFFFF_FFFF;

/// The tokens of a declaration at `at`, up to and with its end: None
/// if it isn't one.
fn read_decl(cpu: &mut Cpu, at: u32) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    let mut p = at;
    loop {
        if out.len() > 0x1_0000 || !cpu.bus.guest_probe(p, 4, false) {
            return None;
        }
        let t = cpu.bus.guest_read_32(p);
        out.push(t);
        p = p.checked_add(4)?;
        if t == DECL_END {
            return Some(out);
        }
        let extra = match t >> 29 {
            4 => 4 * ((t >> 25) & 0xF),
            5 => (t >> 24) & 0x1F,
            _ => 0,
        };
        for _ in 0..extra {
            if !cpu.bus.guest_probe(p, 4, false) {
                return None;
            }
            out.push(cpu.bus.guest_read_32(p));
            p = p.checked_add(4)?;
        }
    }
}

/// Parse a declaration's tokens.
pub fn parse_decl(tokens: &[u32]) -> Option<Decl> {
    let mut d = Decl::default();
    let mut stream = 0;
    let mut offset = 0;
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        i += 1;
        match t >> 29 {
            0 => {}
            1 => {
                stream = t & 0xF;
                offset = 0;
            }
            2 => {
                if t & 0x1000_0000 != 0 {
                    offset += 4 * ((t >> 16) & 0xF);
                } else {
                    let kind = (t >> 16) & 0xF;
                    let reg = t & 0x1F;
                    let size = type_size(kind)?;
                    if reg >= 16 {
                        return None;
                    }
                    d.inputs.push(Input {
                        stream,
                        offset,
                        kind,
                        reg,
                    });
                    offset += size;
                }
            }
            3 => {}
            4 => {
                let count = (t >> 25) & 0xF;
                let addr = t & 0x7F;
                for k in 0..count {
                    let v: [u32; 4] = std::array::from_fn(|c| {
                        tokens.get(i + 4 * k as usize + c).copied().unwrap_or(0)
                    });
                    d.constants.push((addr + k, v));
                }
                i += 4 * count as usize;
            }
            5 => i += ((t >> 24) & 0x1F) as usize,
            7 => break,
            _ => return None,
        }
    }
    Some(d)
}

// Registers of a fixed-function declaration (D3DVSDE_).
pub const REG_POSITION: u32 = 0;
pub const REG_NORMAL: u32 = 3;
pub const REG_DIFFUSE: u32 = 5;
pub const REG_SPECULAR: u32 = 6;
pub const REG_TEXCOORD0: u32 = 7;

/// Whether a declaration can be drawn by the fixed-function pipeline:
/// a normal of three floats, colors of D3DCOLOR.
fn fixed_function_ok(d: &Decl) -> bool {
    d.inputs.iter().all(|i| match i.reg {
        REG_NORMAL => i.kind == 2,
        REG_DIFFUSE | REG_SPECULAR => i.kind == 4,
        _ => true,
    })
}

/// An input's value from the bytes of its vertex, as four floats.
pub fn fetch(kind: u32, b: &[u8]) -> [f32; 4] {
    let f = |k: usize| {
        b.get(4 * k..4 * k + 4)
            .map_or(0.0, |x| f32::from_le_bytes([x[0], x[1], x[2], x[3]]))
    };
    let s = |k: usize| {
        b.get(2 * k..2 * k + 2)
            .map_or(0.0, |x| i16::from_le_bytes([x[0], x[1]]) as f32)
    };
    let u = |k: usize| b.get(k).copied().unwrap_or(0) as f32;
    match kind {
        0 => [f(0), 0.0, 0.0, 1.0],
        1 => [f(0), f(1), 0.0, 1.0],
        2 => [f(0), f(1), f(2), 1.0],
        3 => [f(0), f(1), f(2), f(3)],
        // D3DCOLOR: B, G, R, A in memory, as red, green, blue, alpha.
        4 => [u(2) / 255.0, u(1) / 255.0, u(0) / 255.0, u(3) / 255.0],
        5 => [u(0), u(1), u(2), u(3)],
        6 => [s(0), s(1), 0.0, 1.0],
        7 => [s(0), s(1), s(2), s(3)],
        _ => [0.0, 0.0, 0.0, 1.0],
    }
}

// Functions.

const VS_1_0: u32 = 0xFFFE_0100;
const VS_1_1: u32 = 0xFFFE_0101;
const END: u32 = 0x0000_FFFF;
const COMMENT: u32 = 0xFFFE;

/// The operands each opcode has, the destination's with them.
fn operands(op: u32) -> Option<usize> {
    Some(match op {
        0 => 0,
        1 | 6 | 7 | 14 | 15 | 16 | 19 | 78 | 79 => 2,
        2 | 3 | 5 | 8..=13 | 17 | 20..=24 => 3,
        4 | 18 => 4,
        81 => 5,
        _ => return None,
    })
}

/// The tokens of a function at `at`, up to and with its end: None if it
/// isn't a vs.1.0 or 1.1 one.
fn read_function(cpu: &mut Cpu, at: u32) -> Option<Vec<u32>> {
    if !cpu.bus.guest_probe(at, 4, false) {
        return None;
    }
    let version = cpu.bus.guest_read_32(at);
    if version != VS_1_0 && version != VS_1_1 {
        return None;
    }
    let mut out = vec![version];
    let mut p = at.checked_add(4)?;
    loop {
        if out.len() > 0x4_0000 || !cpu.bus.guest_probe(p, 4, false) {
            return None;
        }
        let t = cpu.bus.guest_read_32(p);
        out.push(t);
        p = p.checked_add(4)?;
        if t == END {
            return Some(out);
        }
        let extra = if t & 0xFFFF == COMMENT {
            (t >> 16) & 0x7FFF
        } else {
            operands(t & 0xFFFF)? as u32
        };
        for _ in 0..extra {
            if !cpu.bus.guest_probe(p, 4, false) {
                return None;
            }
            out.push(cpu.bus.guest_read_32(p));
            p = p.checked_add(4)?;
        }
    }
}

/// What a function wrote for a vertex.
#[derive(Clone, Copy, Debug)]
pub struct Output {
    pub position: [f32; 4],
    pub colors: [[f32; 4]; 2],
    pub tex: [[f32; 4]; 8],
    pub fog: f32,
}

impl Default for Output {
    fn default() -> Output {
        Output {
            position: [0.0, 0.0, 0.0, 1.0],
            colors: [[1.0; 4], [0.0; 4]],
            tex: [[0.0, 0.0, 0.0, 1.0]; 8],
            fog: 1.0,
        }
    }
}

// Register types.
const R_TEMP: u32 = 0;
const R_INPUT: u32 = 1;
const R_CONST: u32 = 2;
const R_ADDR: u32 = 3;
const R_RASTOUT: u32 = 4;
const R_ATTROUT: u32 = 5;
const R_TEXCRDOUT: u32 = 6;

/// A vertex being run through a function.
struct Machine<'a> {
    v: &'a [[f32; 4]; 16],
    c: &'a [[f32; 4]],
    /// The constants DEF gave: the register and its value.
    defs: Vec<(usize, [f32; 4])>,
    r: [[f32; 4]; 12],
    a0: i32,
    out: Output,
}

impl Machine<'_> {
    /// A source operand's value, `row` registers past the one it names
    /// (for the matrix operations).
    fn src(&self, t: u32, row: u32) -> [f32; 4] {
        let kind = (t >> 28) & 7;
        let num = (t & 0x7FF) + row;
        let relative = t & 0x2000 != 0;
        let raw = match kind {
            R_TEMP => self.r.get(num as usize).copied().unwrap_or_default(),
            R_INPUT => self.v.get(num as usize).copied().unwrap_or_default(),
            R_CONST => {
                let n = if relative {
                    (num as i32).wrapping_add(self.a0)
                } else {
                    num as i32
                };
                usize::try_from(n)
                    .ok()
                    .and_then(|n| {
                        let def = self.defs.iter().rev().find(|d| d.0 == n);
                        def.map(|d| d.1).or_else(|| self.c.get(n).copied())
                    })
                    .unwrap_or_default()
            }
            R_ADDR => [self.a0 as f32; 4],
            _ => [0.0; 4],
        };
        let swizzle = (t >> 16) & 0xFF;
        let mut v: [f32; 4] = std::array::from_fn(|i| raw[(swizzle >> (2 * i) & 3) as usize]);
        if (t >> 24) & 0xF == 1 {
            v = v.map(|x| -x);
        }
        v
    }

    /// Write `value` to the destination operand `t`, the components its
    /// mask has.
    fn dst(&mut self, t: u32, value: [f32; 4]) {
        let kind = (t >> 28) & 7;
        let num = (t & 0x7FF) as usize;
        let mask = (t >> 16) & 0xF;
        let reg: &mut [f32; 4] = match kind {
            R_TEMP if num < 12 => &mut self.r[num],
            R_ADDR => {
                // vs.1.1's MOV to a0 rounds down.
                if mask & 1 != 0 {
                    self.a0 = value[0].floor() as i32;
                }
                return;
            }
            R_RASTOUT => match num {
                0 => &mut self.out.position,
                1 => {
                    if mask & 1 != 0 {
                        self.out.fog = value[0];
                    }
                    return;
                }
                _ => return,
            },
            R_ATTROUT if num < 2 => &mut self.out.colors[num],
            R_TEXCRDOUT if num < 8 => &mut self.out.tex[num],
            _ => return,
        };
        for (i, r) in reg.iter_mut().enumerate() {
            if mask & 1 << i != 0 {
                *r = value[i];
            }
        }
    }
}

fn dp3(a: [f32; 4], b: [f32; 4]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn dp4(a: [f32; 4], b: [f32; 4]) -> f32 {
    dp3(a, b) + a[3] * b[3]
}

fn splat(x: f32) -> [f32; 4] {
    [x; 4]
}

/// Run the function `f` (its tokens after the version) on a vertex with
/// the inputs `v` and the constants `c`.
pub fn run(f: &[u32], v: &[[f32; 4]; 16], c: &[[f32; 4]]) -> Output {
    let mut m = Machine {
        v,
        c,
        defs: Vec::new(),
        r: [[0.0; 4]; 12],
        a0: 0,
        out: Output::default(),
    };
    let mut i = 1;
    while i < f.len() {
        let t = f[i];
        i += 1;
        if t == END {
            break;
        }
        let op = t & 0xFFFF;
        if op == COMMENT {
            i += ((t >> 16) & 0x7FFF) as usize;
            continue;
        }
        let Some(n) = operands(op) else {
            break;
        };
        let args = &f[i.min(f.len())..(i + n).min(f.len())];
        i += n;
        if args.len() < n || n == 0 {
            continue;
        }
        let d = args[0];
        // DEF: a constant register the function gives itself.
        if op == 81 {
            let v = std::array::from_fn(|k| f32::from_bits(args[1 + k]));
            m.defs.push(((d & 0x7FF) as usize, v));
            continue;
        }
        let s = |k: usize, row: u32| m.src(args[k], row);
        let value = match op {
            1 => s(1, 0),
            2 => {
                let (a, b) = (s(1, 0), s(2, 0));
                std::array::from_fn(|k| a[k] + b[k])
            }
            3 => {
                let (a, b) = (s(1, 0), s(2, 0));
                std::array::from_fn(|k| a[k] - b[k])
            }
            4 => {
                let (a, b, c) = (s(1, 0), s(2, 0), s(3, 0));
                std::array::from_fn(|k| a[k] * b[k] + c[k])
            }
            5 => {
                let (a, b) = (s(1, 0), s(2, 0));
                std::array::from_fn(|k| a[k] * b[k])
            }
            6 => {
                let x = s(1, 0)[3];
                splat(if x == 1.0 { 1.0 } else { 1.0 / x })
            }
            7 => {
                let x = s(1, 0)[3].abs();
                splat(if x == 1.0 { 1.0 } else { 1.0 / x.sqrt() })
            }
            8 => splat(dp3(s(1, 0), s(2, 0))),
            9 => splat(dp4(s(1, 0), s(2, 0))),
            10..=13 => {
                let (a, b) = (s(1, 0), s(2, 0));
                std::array::from_fn(|k| match op {
                    10 => a[k].min(b[k]),
                    11 => a[k].max(b[k]),
                    12 => (a[k] < b[k]) as u32 as f32,
                    _ => (a[k] >= b[k]) as u32 as f32,
                })
            }
            // EXP and LOG: full precision, in every component.
            14 => splat(s(1, 0)[3].exp2()),
            15 => splat(s(1, 0)[3].abs().log2()),
            // EXPP and LOGP: the parts of the partial precision ones.
            78 => {
                let x = s(1, 0)[3];
                let fl = x.floor();
                [fl.exp2(), x - fl, x.exp2(), 1.0]
            }
            79 => {
                let x = s(1, 0)[3].abs();
                if x == 0.0 {
                    [f32::NEG_INFINITY, 1.0, f32::NEG_INFINITY, 1.0]
                } else {
                    let e = x.log2().floor();
                    [e, x / e.exp2(), x.log2(), 1.0]
                }
            }
            16 => {
                let a = s(1, 0);
                let power = a[3].clamp(-128.0, 128.0);
                let diffuse = a[0].max(0.0);
                let specular = if a[0] > 0.0 && a[1] > 0.0 {
                    a[1].powf(power)
                } else {
                    0.0
                };
                [1.0, diffuse, specular, 1.0]
            }
            17 => {
                let (a, b) = (s(1, 0), s(2, 0));
                [1.0, a[1] * b[1], a[2], b[3]]
            }
            18 => {
                let (a, b, c) = (s(1, 0), s(2, 0), s(3, 0));
                std::array::from_fn(|k| a[k] * (b[k] - c[k]) + c[k])
            }
            19 => s(1, 0).map(|x| x - x.floor()),
            20..=24 => {
                let a = s(1, 0);
                let (rows, three) = match op {
                    20 => (4, false),
                    21 => (3, false),
                    22 => (4, true),
                    23 => (3, true),
                    _ => (2, true),
                };
                let mut out = [0.0; 4];
                for (r, o) in out.iter_mut().enumerate().take(rows) {
                    let row = s(2, r as u32);
                    *o = if three { dp3(a, row) } else { dp4(a, row) };
                }
                out
            }
            _ => continue,
        };
        m.dst(d, value);
    }
    m.out
}

// The methods.

/// CreateVertexShader(this, declaration, function, out, usage).
pub fn create_vertex_shader(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, decl_at, function_at, out, _] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) || decl_at == 0 {
        return Some(D3DERR_INVALIDCALL);
    }
    let Some(decl) = read_decl(cpu, decl_at) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let Some(parsed) = parse_decl(&decl) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let function = if function_at != 0 {
        let Some(f) = read_function(cpu, function_at) else {
            return Some(D3DERR_INVALIDCALL);
        };
        f
    } else {
        if !fixed_function_ok(&parsed) {
            return Some(D3DERR_INVALIDCALL);
        }
        Vec::new()
    };
    let Some(d) = device::get(cpu, id) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let shader = Shader { decl, function };
    let i = match d.shaders.iter().position(Option::is_none) {
        Some(i) => {
            d.shaders[i] = Some(shader);
            i
        }
        None => {
            d.shaders.push(Some(shader));
            d.shaders.len() - 1
        }
    };
    cpu.bus.guest_write_32(out, HANDLE_BASE + 1 + i as u32);
    Some(D3D_OK)
}

/// DeleteVertexShader(this, handle): the device draws with none if it
/// was the one.
pub fn delete_vertex_shader(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, handle] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    if get_shader(&d, handle).is_none() {
        return Some(D3DERR_INVALIDCALL);
    }
    let Some(d) = device::get(cpu, id) else {
        return Some(D3DERR_INVALIDCALL);
    };
    d.shaders[(handle - HANDLE_BASE - 1) as usize] = None;
    if d.vertex_shader == handle {
        d.vertex_shader = 0;
    }
    Some(D3D_OK)
}

/// Give a shader's tokens: their size at `size_at`, and the tokens at
/// `at` if it has room for them.
fn give_tokens(cpu: &mut Cpu, tokens: &[u32], at: u32, size_at: u32) -> u32 {
    if size_at == 0 || !cpu.bus.guest_probe(size_at, 4, true) {
        return D3DERR_INVALIDCALL;
    }
    let size = tokens.len() as u32 * 4;
    if at == 0 {
        cpu.bus.guest_write_32(size_at, size);
        return D3D_OK;
    }
    if cpu.bus.guest_read_32(size_at) < size || !cpu.bus.guest_probe(at, size as usize, true) {
        return D3DERR_INVALIDCALL;
    }
    for (i, t) in tokens.iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, *t);
    }
    cpu.bus.guest_write_32(size_at, size);
    D3D_OK
}

/// GetVertexShaderDeclaration(this, handle, data, size).
pub fn get_declaration(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, handle, at, size_at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    let Some(s) = get_shader(&d, handle) else {
        return Some(D3DERR_INVALIDCALL);
    };
    Some(give_tokens(cpu, &s.decl, at, size_at))
}

/// GetVertexShaderFunction(this, handle, data, size): none (0 bytes)
/// for a fixed-function shader.
pub fn get_function(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, handle, at, size_at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    let Some(s) = get_shader(&d, handle) else {
        return Some(D3DERR_INVALIDCALL);
    };
    Some(give_tokens(cpu, &s.function, at, size_at))
}

/// The constants the shader `handle`'s declaration loads, put in the
/// device's registers: SetVertexShader does it.
pub fn load_constants(d: &mut Device, handle: u32) {
    let Some(decl) = get_shader(d, handle).map(|s| s.decl.clone()) else {
        return;
    };
    let Some(parsed) = parse_decl(&decl) else {
        return;
    };
    for (reg, v) in parsed.constants {
        if let Some(c) = d.constants.get_mut(reg as usize) {
            *c = v;
        }
    }
}

/// CreatePixelShader(this, function, out): the card has none.
pub fn create_pixel_shader(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(this_device(cpu, args[0]));
    Some(D3DERR_INVALIDCALL)
}

/// DeletePixelShader(this, handle).
pub fn delete_pixel_shader(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(this_device(cpu, args[0]));
    Some(D3DERR_INVALIDCALL)
}

/// GetPixelShaderFunction(this, handle, data, size).
pub fn get_pixel_function(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(this_device(cpu, args[0]));
    Some(D3DERR_INVALIDCALL)
}
