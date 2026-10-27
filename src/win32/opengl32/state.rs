//! OpenGL's state that isn't matrices, textures or vertices: glEnable
//! and the settings of blending, the tests, culling, fog, the viewport
//! and the rest, which `draw.rs` puts on the inner device as it draws;
//! glGetError, glGetString and glGet.

use super::wgl::{EXTENSIONS_AT, RENDERER_AT, VENDOR_AT, VERSION_AT};
use super::{
    CAP_ALPHA_TEST, CAP_BLEND, CAP_CLIP_PLANES, CAP_COLOR_MATERIAL, CAP_CULL_FACE, CAP_DEPTH_TEST,
    CAP_DITHER, CAP_FOG, CAP_LIGHTING, CAP_LINE_SMOOTH, CAP_LINE_STIPPLE, CAP_LOGIC_OP,
    CAP_NORMALIZE, CAP_POINT_SMOOTH, CAP_POLYGON_OFFSET_FILL, CAP_POLYGON_OFFSET_LINE,
    CAP_POLYGON_OFFSET_POINT, CAP_POLYGON_SMOOTH, CAP_POLYGON_STIPPLE, CAP_SCISSOR_TEST,
    CAP_STENCIL_TEST, CAP_TEXTURE_GEN, Context, GL_INVALID_ENUM, GL_INVALID_VALUE, GL_NO_ERROR,
    MAX_TEXTURE_SIZE, MODELVIEW_DEPTH, PROJECTION_DEPTH, TEXTURE_DEPTH, ctx, d, error, f,
    read_floats,
};
use crate::cpu::Cpu;

// Comparisons.
pub const GL_NEVER: u32 = 0x0200;
pub const GL_LESS: u32 = 0x0201;
pub const GL_ALWAYS: u32 = 0x0207;

// Blending factors.
pub const GL_ZERO: u32 = 0;
pub const GL_ONE: u32 = 1;
pub const GL_SRC_COLOR: u32 = 0x0300;
pub const GL_SRC_ALPHA_SATURATE: u32 = 0x0308;

// Faces and modes.
pub const GL_FRONT: u32 = 0x0404;
pub const GL_BACK: u32 = 0x0405;
pub const GL_FRONT_AND_BACK: u32 = 0x0408;
pub const GL_CW: u32 = 0x0900;
pub const GL_CCW: u32 = 0x0901;
pub const GL_POINT: u32 = 0x1B00;
pub const GL_LINE: u32 = 0x1B01;
pub const GL_FILL: u32 = 0x1B02;
pub const GL_FLAT: u32 = 0x1D00;
pub const GL_SMOOTH: u32 = 0x1D01;
pub const GL_BACK_BUFFER: u32 = 0x0405;

// Fog.
pub const GL_EXP: u32 = 0x0800;
pub const GL_EXP2: u32 = 0x0801;
pub const GL_LINEAR: u32 = 0x2601;
const GL_FOG_DENSITY: u32 = 0x0B62;
const GL_FOG_START: u32 = 0x0B63;
const GL_FOG_END: u32 = 0x0B64;
const GL_FOG_MODE: u32 = 0x0B65;
const GL_FOG_COLOR: u32 = 0x0B66;
const GL_FOG_INDEX: u32 = 0x0B61;

// Capabilities.
const GL_BLEND: u32 = 0x0BE2;
const GL_ALPHA_TEST: u32 = 0x0BC0;
const GL_DEPTH_TEST: u32 = 0x0B71;
const GL_CULL_FACE: u32 = 0x0B44;
const GL_POLYGON_OFFSET_FILL: u32 = 0x8037;
const GL_POLYGON_OFFSET_LINE: u32 = 0x2A02;
const GL_POLYGON_OFFSET_POINT: u32 = 0x2A01;
const GL_SCISSOR_TEST: u32 = 0x0C11;
const GL_FOG: u32 = 0x0B60;
const GL_DITHER: u32 = 0x0BD0;
const GL_LIGHTING: u32 = 0x0B50;
const GL_STENCIL_TEST: u32 = 0x0B90;
const GL_COLOR_MATERIAL: u32 = 0x0B57;
const GL_NORMALIZE: u32 = 0x0BA1;
const GL_LINE_SMOOTH: u32 = 0x0B20;
const GL_POINT_SMOOTH: u32 = 0x0B10;
const GL_POLYGON_SMOOTH: u32 = 0x0B41;
const GL_LINE_STIPPLE: u32 = 0x0B24;
const GL_POLYGON_STIPPLE: u32 = 0x0B42;
const GL_LOGIC_OP: u32 = 0x0BF1;
const GL_COLOR_LOGIC_OP: u32 = 0x0BF2;
pub const GL_TEXTURE_2D: u32 = 0x0DE1;
const GL_TEXTURE_1D: u32 = 0x0DE0;
const GL_AUTO_NORMAL: u32 = 0x0D80;

// Client states.
pub const GL_VERTEX_ARRAY: u32 = 0x8074;
pub const GL_NORMAL_ARRAY: u32 = 0x8075;
pub const GL_COLOR_ARRAY: u32 = 0x8076;
const GL_INDEX_ARRAY: u32 = 0x8077;
pub const GL_TEXTURE_COORD_ARRAY: u32 = 0x8078;
const GL_EDGE_FLAG_ARRAY: u32 = 0x8079;

/// The bit of `caps` that glEnable(`cap`) sets: Some(0) for one that is
/// accepted and has no effect, None for none.
fn cap_bit(cap: u32) -> Option<u32> {
    Some(match cap {
        GL_BLEND => CAP_BLEND,
        GL_ALPHA_TEST => CAP_ALPHA_TEST,
        GL_DEPTH_TEST => CAP_DEPTH_TEST,
        GL_CULL_FACE => CAP_CULL_FACE,
        GL_POLYGON_OFFSET_FILL => CAP_POLYGON_OFFSET_FILL,
        GL_POLYGON_OFFSET_LINE => CAP_POLYGON_OFFSET_LINE,
        GL_POLYGON_OFFSET_POINT => CAP_POLYGON_OFFSET_POINT,
        GL_SCISSOR_TEST => CAP_SCISSOR_TEST,
        GL_FOG => CAP_FOG,
        GL_DITHER => CAP_DITHER,
        GL_LIGHTING => CAP_LIGHTING,
        GL_STENCIL_TEST => CAP_STENCIL_TEST,
        GL_COLOR_MATERIAL => CAP_COLOR_MATERIAL,
        GL_NORMALIZE => CAP_NORMALIZE,
        GL_LINE_SMOOTH => CAP_LINE_SMOOTH,
        GL_POINT_SMOOTH => CAP_POINT_SMOOTH,
        GL_POLYGON_SMOOTH => CAP_POLYGON_SMOOTH,
        GL_LINE_STIPPLE => CAP_LINE_STIPPLE,
        GL_POLYGON_STIPPLE => CAP_POLYGON_STIPPLE,
        GL_LOGIC_OP | GL_COLOR_LOGIC_OP => CAP_LOGIC_OP,
        0x3000..=0x3005 => CAP_CLIP_PLANES,
        0x0C60..=0x0C63 => CAP_TEXTURE_GEN,
        // Lights, evaluators' maps, GL_TEXTURE_1D: kept nowhere.
        0x4000..=0x4007 | 0x0D90..=0x0DB8 | GL_AUTO_NORMAL | GL_TEXTURE_1D => 0,
        _ => return None,
    })
}

fn set_cap(cpu: &mut Cpu, cap: u32, on: bool) {
    let Some(c) = ctx(cpu) else {
        return;
    };
    if cap == GL_TEXTURE_2D {
        let unit = c.active_unit as usize;
        c.texture_2d[unit] = on;
        return;
    }
    match cap_bit(cap) {
        Some(bit) if on => c.caps |= bit,
        Some(bit) => c.caps &= !bit,
        None => error(cpu, GL_INVALID_ENUM),
    }
}

/// glEnable(cap).
pub fn enable(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_cap(cpu, args[0], true);
    Some(0)
}

/// glDisable(cap).
pub fn disable(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_cap(cpu, args[0], false);
    Some(0)
}

/// Whether `cap` is on: glIsEnabled's, and glGet's of a capability.
fn enabled(c: &Context, cap: u32) -> Option<bool> {
    let unit = c.client_unit as usize;
    Some(match cap {
        GL_TEXTURE_2D => c.texture_2d[c.active_unit as usize],
        GL_VERTEX_ARRAY => c.vertex_array.enabled,
        GL_COLOR_ARRAY => c.color_array.enabled,
        GL_NORMAL_ARRAY => c.normal_array.enabled,
        GL_TEXTURE_COORD_ARRAY => c.tex_array[unit].enabled,
        GL_INDEX_ARRAY | GL_EDGE_FLAG_ARRAY => false,
        _ => {
            let bit = cap_bit(cap)?;
            bit != 0 && c.caps & bit != 0
        }
    })
}

/// glIsEnabled(cap).
pub fn is_enabled(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    match enabled(c, args[0]) {
        Some(on) => Some(on as u32),
        None => {
            error(cpu, GL_INVALID_ENUM);
            Some(0)
        }
    }
}

fn set_client_state(cpu: &mut Cpu, array: u32, on: bool) {
    let Some(c) = ctx(cpu) else {
        return;
    };
    let unit = c.client_unit as usize;
    match array {
        GL_VERTEX_ARRAY => c.vertex_array.enabled = on,
        GL_COLOR_ARRAY => c.color_array.enabled = on,
        GL_NORMAL_ARRAY => c.normal_array.enabled = on,
        GL_TEXTURE_COORD_ARRAY => c.tex_array[unit].enabled = on,
        GL_INDEX_ARRAY | GL_EDGE_FLAG_ARRAY => {}
        _ => error(cpu, GL_INVALID_ENUM),
    }
}

/// glEnableClientState(array).
pub fn enable_client_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_client_state(cpu, args[0], true);
    Some(0)
}

/// glDisableClientState(array).
pub fn disable_client_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_client_state(cpu, args[0], false);
    Some(0)
}

fn blend_factor(v: u32) -> bool {
    v <= GL_ONE || (GL_SRC_COLOR..=GL_SRC_ALPHA_SATURATE).contains(&v)
}

/// glBlendFunc(source, destination).
pub fn blend_func(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (s, dst) = (args[0], args[1]);
    if !blend_factor(s) || !blend_factor(dst) {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    if let Some(c) = ctx(cpu) {
        c.blend = [s, dst];
    }
    Some(0)
}

fn comparison(v: u32) -> bool {
    (GL_NEVER..=GL_ALWAYS).contains(&v)
}

/// glAlphaFunc(func, reference).
pub fn alpha_func(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !comparison(args[0]) {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    if let Some(c) = ctx(cpu) {
        c.alpha_func = args[0];
        c.alpha_ref = f(args, 1).clamp(0.0, 1.0);
    }
    Some(0)
}

/// glDepthFunc(func).
pub fn depth_func(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !comparison(args[0]) {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    if let Some(c) = ctx(cpu) {
        c.depth_func = args[0];
    }
    Some(0)
}

/// glDepthMask(flag).
pub fn depth_mask(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(c) = ctx(cpu) {
        c.depth_mask = args[0] & 0xFF != 0;
    }
    Some(0)
}

/// glDepthRange(near, far), doubles.
pub fn depth_range(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(c) = ctx(cpu) {
        c.depth_range = [
            d(args, 0).clamp(0.0, 1.0) as f32,
            d(args, 2).clamp(0.0, 1.0) as f32,
        ];
    }
    Some(0)
}

/// glCullFace(mode).
pub fn cull_face(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if ![GL_FRONT, GL_BACK, GL_FRONT_AND_BACK].contains(&args[0]) {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    if let Some(c) = ctx(cpu) {
        c.cull_face = args[0];
    }
    Some(0)
}

/// glFrontFace(mode).
pub fn front_face(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if ![GL_CW, GL_CCW].contains(&args[0]) {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    if let Some(c) = ctx(cpu) {
        c.front_face = args[0];
    }
    Some(0)
}

/// glPolygonOffset(factor, units).
pub fn polygon_offset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(c) = ctx(cpu) {
        c.offset = [f(args, 0), f(args, 1)];
    }
    Some(0)
}

/// glPolygonMode(face, mode): the same for both faces, the card's
/// triangles having no face of their own.
pub fn polygon_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if ![GL_POINT, GL_LINE, GL_FILL].contains(&args[1]) {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    if let Some(c) = ctx(cpu) {
        c.polygon_mode = args[1];
    }
    Some(0)
}

/// glShadeModel(mode).
pub fn shade_model(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if ![GL_FLAT, GL_SMOOTH].contains(&args[0]) {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    if let Some(c) = ctx(cpu) {
        c.shade_model = args[0];
    }
    Some(0)
}

/// The rectangle x, y, width, height of `args`, or an error for a
/// negative size.
fn rect(cpu: &mut Cpu, args: &[u32]) -> Option<[i32; 4]> {
    let r = [
        args[0] as i32,
        args[1] as i32,
        args[2] as i32,
        args[3] as i32,
    ];
    if r[2] < 0 || r[3] < 0 {
        error(cpu, GL_INVALID_VALUE);
        return None;
    }
    Some(r)
}

/// glScissor(x, y, width, height).
pub fn scissor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(r) = rect(cpu, args)
        && let Some(c) = ctx(cpu)
    {
        c.scissor = r;
    }
    Some(0)
}

/// glViewport(x, y, width, height).
pub fn viewport(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(r) = rect(cpu, args)
        && let Some(c) = ctx(cpu)
    {
        c.viewport = [r[0], r[1], r[2].min(MAX_VIEWPORT), r[3].min(MAX_VIEWPORT)];
    }
    Some(0)
}

/// The largest viewport.
const MAX_VIEWPORT: i32 = 2048;

/// glColorMask(red, green, blue, alpha).
pub fn color_mask(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(c) = ctx(cpu) {
        c.color_mask = std::array::from_fn(|i| args[i] & 0xFF != 0);
    }
    Some(0)
}

/// glClearColor(red, green, blue, alpha).
pub fn clear_color(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(c) = ctx(cpu) {
        c.clear_color = std::array::from_fn(|i| f(args, i).clamp(0.0, 1.0));
    }
    Some(0)
}

/// glClearDepth(depth), a double.
pub fn clear_depth(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(c) = ctx(cpu) {
        c.clear_depth = d(args, 0).clamp(0.0, 1.0) as f32;
    }
    Some(0)
}

/// glClearStencil(s): there is no stencil buffer.
pub fn clear_stencil(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(c) = ctx(cpu) {
        c.clear_stencil = args[0];
    }
    Some(0)
}

/// glStencilFunc, glStencilMask and glStencilOp: the pixel formats have
/// no stencil buffer, so the test passes.
pub fn stencil_func(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

pub fn stencil_mask(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

pub fn stencil_op(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// glHint(target, mode).
pub fn hint(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// Set the fog parameter `p` to `v` (the first of its values).
fn fog(cpu: &mut Cpu, p: u32, v: &[f32]) {
    let Some(c) = ctx(cpu) else {
        return;
    };
    let first = v.first().copied().unwrap_or(0.0);
    match p {
        GL_FOG_MODE => {
            let mode = first as u32;
            if [GL_LINEAR, GL_EXP, GL_EXP2].contains(&mode) {
                c.fog_mode = mode;
            } else {
                error(cpu, GL_INVALID_ENUM);
            }
        }
        GL_FOG_DENSITY => c.fog[0] = first,
        GL_FOG_START => c.fog[1] = first,
        GL_FOG_END => c.fog[2] = first,
        GL_FOG_COLOR if v.len() >= 4 => {
            c.fog_color = [v[0], v[1], v[2], v[3]].map(|x| x.clamp(0.0, 1.0))
        }
        GL_FOG_INDEX => {}
        _ => error(cpu, GL_INVALID_ENUM),
    }
}

/// glFogf(pname, param).
pub fn fogf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    fog(cpu, args[0], &[f(args, 1)]);
    Some(0)
}

/// glFogi(pname, param).
pub fn fogi(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    fog(cpu, args[0], &[args[1] as i32 as f32]);
    Some(0)
}

/// How many values the fog parameter `p` has.
fn fog_values(p: u32) -> usize {
    if p == GL_FOG_COLOR { 4 } else { 1 }
}

/// glFogfv(pname, params).
pub fn fogfv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let p = args[0];
    if let Some(v) = read_floats(cpu, args[1], fog_values(p)) {
        fog(cpu, p, &v);
    }
    Some(0)
}

/// glFogiv(pname, params): the color's integers from the most negative
/// to the most positive as -1 to 1.
pub fn fogiv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (p, at) = (args[0], args[1]);
    let n = fog_values(p);
    if at == 0 || !cpu.bus.guest_probe(at, n * 4, false) {
        return Some(0);
    }
    let v: Vec<f32> = (0..n as u32)
        .map(|i| {
            let x = cpu.bus.guest_read_32(at + 4 * i) as i32;
            if p == GL_FOG_COLOR {
                x as f32 / i32::MAX as f32
            } else {
                x as f32
            }
        })
        .collect();
    fog(cpu, p, &v);
    Some(0)
}

/// glLineWidth(width).
pub fn line_width(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(c) = ctx(cpu) {
        c.line_width = f(args, 0);
    }
    Some(0)
}

/// glPointSize(size).
pub fn point_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(c) = ctx(cpu) {
        c.point_size = f(args, 0);
    }
    Some(0)
}

/// glDrawBuffer(mode): drawing is into the back buffer of a double
/// buffered format, the front of the other.
pub fn draw_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(c) = ctx(cpu) {
        c.draw_buffer = args[0];
    }
    Some(0)
}

/// glReadBuffer(mode).
pub fn read_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(c) = ctx(cpu) {
        c.read_buffer = args[0];
    }
    Some(0)
}

// The pixel store.
const GL_UNPACK_ROW_LENGTH: u32 = 0x0CF2;
const GL_UNPACK_SKIP_ROWS: u32 = 0x0CF3;
const GL_UNPACK_SKIP_PIXELS: u32 = 0x0CF4;
const GL_UNPACK_ALIGNMENT: u32 = 0x0CF5;
const GL_PACK_ROW_LENGTH: u32 = 0x0D02;
const GL_PACK_SKIP_ROWS: u32 = 0x0D03;
const GL_PACK_SKIP_PIXELS: u32 = 0x0D04;
const GL_PACK_ALIGNMENT: u32 = 0x0D05;

fn pixel_store(cpu: &mut Cpu, p: u32, v: i32) {
    let Some(c) = ctx(cpu) else {
        return;
    };
    let align = matches!(p, GL_UNPACK_ALIGNMENT | GL_PACK_ALIGNMENT);
    if v < 0 || align && ![1, 2, 4, 8].contains(&v) {
        error(cpu, GL_INVALID_VALUE);
        return;
    }
    let v = v as u32;
    match p {
        GL_UNPACK_ALIGNMENT => c.unpack[0] = v,
        GL_UNPACK_ROW_LENGTH => c.unpack[1] = v,
        GL_UNPACK_SKIP_ROWS => c.unpack[2] = v,
        GL_UNPACK_SKIP_PIXELS => c.unpack[3] = v,
        GL_PACK_ALIGNMENT => c.pack[0] = v,
        GL_PACK_ROW_LENGTH => c.pack[1] = v,
        GL_PACK_SKIP_ROWS => c.pack[2] = v,
        GL_PACK_SKIP_PIXELS => c.pack[3] = v,
        // Swapping bytes and the LSB first of bitmaps.
        0x0CF0 | 0x0CF1 | 0x0D00 | 0x0D01 => {}
        _ => error(cpu, GL_INVALID_ENUM),
    }
}

/// glPixelStorei(pname, param).
pub fn pixel_storei(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    pixel_store(cpu, args[0], args[1] as i32);
    Some(0)
}

/// glPixelStoref(pname, param).
pub fn pixel_storef(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    pixel_store(cpu, args[0], f(args, 1).round() as i32);
    Some(0)
}

/// glGetError(): the first error since the last call.
pub fn get_error(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let Some(c) = ctx(cpu) else {
        return Some(GL_NO_ERROR);
    };
    Some(std::mem::take(&mut c.error))
}

// glGetString's names.
const GL_VENDOR: u32 = 0x1F00;
const GL_RENDERER: u32 = 0x1F01;
const GL_VERSION: u32 = 0x1F02;
const GL_EXTENSIONS: u32 = 0x1F03;

/// glGetString(name): NULL without a current context, as Wine has it.
pub fn get_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(at) = ctx(cpu).map(|c| c.strings) else {
        return Some(0);
    };
    let offset = match args[0] {
        GL_VENDOR => VENDOR_AT,
        GL_RENDERER => RENDERER_AT,
        GL_VERSION => VERSION_AT,
        GL_EXTENSIONS => EXTENSIONS_AT,
        _ => {
            error(cpu, GL_INVALID_ENUM);
            return Some(0);
        }
    };
    Some(at + offset)
}

/// The values glGet gives for `p`, and whether they are colors, which
/// glGetIntegerv takes from 0 to 1 to the whole range of integers.
fn values(cpu: &mut Cpu, p: u32) -> Option<(Vec<f64>, bool)> {
    let tmus = {
        let dd = ctx(cpu)?.dd;
        super::super::ddraw::d3d::hal(cpu, dd).unwrap_or(1)
    };
    let c = ctx(cpu)?;
    let unit = c.active_unit as usize;
    let one = |v: f64| Some((vec![v], false));
    let ints = |v: &[i32]| Some((v.iter().map(|&x| x as f64).collect(), false));
    let floats = |v: &[f32]| v.iter().map(|&x| x as f64).collect::<Vec<_>>();
    let matrix = |m: &[f32; 16]| Some((floats(m), false));
    if let Some(on) = enabled(c, p) {
        return one(on as u32 as f64);
    }
    match p {
        // GL_CURRENT_COLOR, GL_CURRENT_TEXTURE_COORDS, GL_CURRENT_NORMAL.
        0x0B00 => Some((floats(&c.color), true)),
        0x0B03 => Some((floats(&c.tex_coord[unit]), false)),
        0x0B02 => Some((floats(&c.normal), false)),
        // GL_POINT_SIZE, GL_LINE_WIDTH.
        0x0B11 => one(c.point_size as f64),
        0x0B21 => one(c.line_width as f64),
        // GL_POLYGON_MODE, GL_CULL_FACE_MODE, GL_FRONT_FACE,
        // GL_SHADE_MODEL.
        0x0B40 => ints(&[c.polygon_mode as i32, c.polygon_mode as i32]),
        0x0B45 => one(c.cull_face as f64),
        0x0B46 => one(c.front_face as f64),
        0x0B54 => one(c.shade_model as f64),
        // Fog.
        GL_FOG_DENSITY => one(c.fog[0] as f64),
        GL_FOG_START => one(c.fog[1] as f64),
        GL_FOG_END => one(c.fog[2] as f64),
        GL_FOG_MODE => one(c.fog_mode as f64),
        GL_FOG_COLOR => Some((floats(&c.fog_color), true)),
        // GL_DEPTH_RANGE, GL_DEPTH_WRITEMASK, GL_DEPTH_CLEAR_VALUE,
        // GL_DEPTH_FUNC.
        0x0B70 => Some((floats(&c.depth_range), true)),
        0x0B72 => one(c.depth_mask as u32 as f64),
        0x0B73 => Some((vec![c.clear_depth as f64], true)),
        0x0B74 => one(c.depth_func as f64),
        // GL_MATRIX_MODE and the stacks.
        0x0BA0 => one(c.matrix_mode as f64),
        0x0BA3 => one(c.modelview.len() as f64),
        0x0BA4 => one(c.projection.len() as f64),
        0x0BA5 => one(c.texture_matrix[unit].len() as f64),
        0x0BA6 => matrix(c.modelview.last()?),
        0x0BA7 => matrix(c.projection.last()?),
        0x0BA8 => matrix(c.texture_matrix[unit].last()?),
        // GL_VIEWPORT, GL_SCISSOR_BOX.
        0x0BA2 => ints(&c.viewport),
        0x0C10 => ints(&c.scissor),
        // GL_ALPHA_TEST_FUNC, GL_ALPHA_TEST_REF.
        0x0BC1 => one(c.alpha_func as f64),
        0x0BC2 => Some((vec![c.alpha_ref as f64], true)),
        // GL_BLEND_DST, GL_BLEND_SRC.
        0x0BE0 => one(c.blend[1] as f64),
        0x0BE1 => one(c.blend[0] as f64),
        // GL_DRAW_BUFFER, GL_READ_BUFFER, GL_DOUBLEBUFFER.
        0x0C01 => one(c.draw_buffer as f64),
        0x0C02 => one(c.read_buffer as f64),
        0x0C32 => one(super::wgl::double_buffered(c.format) as u32 as f64),
        // GL_COLOR_CLEAR_VALUE, GL_COLOR_WRITEMASK.
        0x0C22 => Some((floats(&c.clear_color), true)),
        0x0C23 => Some((c.color_mask.map(|m| m as u32 as f64).to_vec(), false)),
        // The pixel store.
        GL_UNPACK_ALIGNMENT => one(c.unpack[0] as f64),
        GL_UNPACK_ROW_LENGTH => one(c.unpack[1] as f64),
        GL_UNPACK_SKIP_ROWS => one(c.unpack[2] as f64),
        GL_UNPACK_SKIP_PIXELS => one(c.unpack[3] as f64),
        GL_PACK_ALIGNMENT => one(c.pack[0] as f64),
        GL_PACK_ROW_LENGTH => one(c.pack[1] as f64),
        GL_PACK_SKIP_ROWS => one(c.pack[2] as f64),
        GL_PACK_SKIP_PIXELS => one(c.pack[3] as f64),
        // GL_MAX_LIGHTS, GL_MAX_CLIP_PLANES, GL_MAX_TEXTURE_SIZE,
        // GL_MAX_PIXEL_MAP_TABLE, GL_MAX_ATTRIB_STACK_DEPTH,
        // GL_MAX_MODELVIEW_STACK_DEPTH, GL_MAX_NAME_STACK_DEPTH,
        // GL_MAX_PROJECTION_STACK_DEPTH, GL_MAX_TEXTURE_STACK_DEPTH,
        // GL_MAX_VIEWPORT_DIMS, GL_MAX_CLIENT_ATTRIB_STACK_DEPTH.
        0x0D31 => one(8.0),
        0x0D32 => one(6.0),
        0x0D33 => one(MAX_TEXTURE_SIZE as f64),
        0x0D34 => one(256.0),
        0x0D35 => one(16.0),
        0x0D36 => one(MODELVIEW_DEPTH as f64),
        0x0D37 => one(64.0),
        0x0D38 => one(PROJECTION_DEPTH as f64),
        0x0D39 => one(TEXTURE_DEPTH as f64),
        0x0D3A => ints(&[MAX_VIEWPORT, MAX_VIEWPORT]),
        0x0D3B => one(16.0),
        // GL_SUBPIXEL_BITS, and the bits of the buffers: 5-6-5 color,
        // 16 of depth, no alpha, stencil or accumulation.
        0x0D50 => one(4.0),
        0x0D52 | 0x0D54 => one(5.0),
        0x0D53 => one(6.0),
        0x0D55 | 0x0D57..=0x0D5B => one(0.0),
        0x0D56 => one(16.0),
        // GL_TEXTURE_BINDING_2D.
        0x8069 => one(c.bound[unit] as f64),
        // GL_POLYGON_OFFSET_FACTOR, GL_POLYGON_OFFSET_UNITS.
        0x8038 => one(c.offset[0] as f64),
        0x2A00 => one(c.offset[1] as f64),
        // The arrays: size, type, stride.
        0x807A => one(c.vertex_array.size as f64),
        0x807B => one(c.vertex_array.kind as f64),
        0x807C => one(c.vertex_array.stride as f64),
        0x807E => one(c.normal_array.kind as f64),
        0x807F => one(c.normal_array.stride as f64),
        0x8081 => one(c.color_array.size as f64),
        0x8082 => one(c.color_array.kind as f64),
        0x8083 => one(c.color_array.stride as f64),
        0x8088 => one(c.tex_array[c.client_unit as usize].size as f64),
        0x8089 => one(c.tex_array[c.client_unit as usize].kind as f64),
        0x808A => one(c.tex_array[c.client_unit as usize].stride as f64),
        // GL_ACTIVE_TEXTURE_ARB, GL_CLIENT_ACTIVE_TEXTURE_ARB,
        // GL_MAX_TEXTURE_UNITS_ARB.
        0x84E0 => one((super::texture::GL_TEXTURE0 + c.active_unit) as f64),
        0x84E1 => one((super::texture::GL_TEXTURE0 + c.client_unit) as f64),
        0x84E2 => one(tmus as f64),
        _ => None,
    }
}

/// The values of `p` for glGet, or the error for one there isn't.
fn get(cpu: &mut Cpu, p: u32) -> Option<(Vec<f64>, bool)> {
    ctx(cpu)?;
    let v = values(cpu, p);
    if v.is_none() {
        error(cpu, GL_INVALID_ENUM);
    }
    v
}

/// glGetIntegerv(pname, params).
pub fn get_integerv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((v, color)) = get(cpu, args[0]) else {
        return Some(0);
    };
    let at = args[1];
    if at == 0 || !cpu.bus.guest_probe(at, v.len() * 4, true) {
        return Some(0);
    }
    for (i, x) in v.iter().enumerate() {
        let n = if color {
            (x.clamp(-1.0, 1.0) * i32::MAX as f64) as i32
        } else {
            x.round() as i32
        };
        cpu.bus.guest_write_32(at + 4 * i as u32, n as u32);
    }
    Some(0)
}

/// glGetFloatv(pname, params).
pub fn get_floatv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((v, _)) = get(cpu, args[0]) else {
        return Some(0);
    };
    let at = args[1];
    if at == 0 || !cpu.bus.guest_probe(at, v.len() * 4, true) {
        return Some(0);
    }
    for (i, x) in v.iter().enumerate() {
        cpu.bus
            .guest_write_32(at + 4 * i as u32, (*x as f32).to_bits());
    }
    Some(0)
}

/// glGetDoublev(pname, params).
pub fn get_doublev(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((v, _)) = get(cpu, args[0]) else {
        return Some(0);
    };
    let at = args[1];
    if at == 0 || !cpu.bus.guest_probe(at, v.len() * 8, true) {
        return Some(0);
    }
    for (i, x) in v.iter().enumerate() {
        let bits = x.to_bits();
        cpu.bus.guest_write_32(at + 8 * i as u32, bits as u32);
        cpu.bus
            .guest_write_32(at + 8 * i as u32 + 4, (bits >> 32) as u32);
    }
    Some(0)
}

/// glGetBooleanv(pname, params).
pub fn get_booleanv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((v, _)) = get(cpu, args[0]) else {
        return Some(0);
    };
    let at = args[1];
    if at == 0 || !cpu.bus.guest_probe(at, v.len(), true) {
        return Some(0);
    }
    let bytes: Vec<u8> = v.iter().map(|&x| (x != 0.0) as u8).collect();
    cpu.bus.guest_write_bytes(at, &bytes);
    Some(0)
}

/// glGetPointerv(pname, params): the arrays' addresses.
pub fn get_pointerv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    let unit = c.client_unit as usize;
    let p = match args[0] {
        0x808E => c.vertex_array.at,
        0x808F => c.normal_array.at,
        0x8090 => c.color_array.at,
        0x8092 => c.tex_array[unit].at,
        _ => {
            error(cpu, GL_INVALID_ENUM);
            return Some(0);
        }
    };
    if args[1] != 0 && cpu.bus.guest_probe(args[1], 4, true) {
        cpu.bus.guest_write_32(args[1], p);
    }
    Some(0)
}
