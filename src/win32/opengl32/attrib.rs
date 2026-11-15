//! glPushAttrib and glPopAttrib: the server's attribute stack (OpenGL
//! 1.1, section 6.1.12 and table 6.4's groups).
//!
//! A push keeps every group the context has state for, and the pop puts
//! back those of the mask the push was given. The groups this DLL keeps
//! nothing of, such as the accumulation buffer, evaluators, hints and
//! the pixel transfer modes, are pushed and popped with nothing in
//! them.

use super::light::Lighting;
use super::{
    CAP_ALPHA_TEST, CAP_BLEND, CAP_CLIP_PLANES, CAP_COLOR_MATERIAL, CAP_CULL_FACE, CAP_DEPTH_TEST,
    CAP_DITHER, CAP_FOG, CAP_LIGHTING, CAP_LINE_SMOOTH, CAP_LINE_STIPPLE, CAP_LOGIC_OP,
    CAP_NORMALIZE, CAP_POINT_SMOOTH, CAP_POLYGON_OFFSET_FILL, CAP_POLYGON_OFFSET_LINE,
    CAP_POLYGON_OFFSET_POINT, CAP_POLYGON_SMOOTH, CAP_POLYGON_STIPPLE, CAP_SCISSOR_TEST,
    CAP_STENCIL_TEST, CAP_TEXTURE_GEN, Context, GL_INVALID_OPERATION, GL_STACK_OVERFLOW,
    GL_STACK_UNDERFLOW, Texture, UNITS, ctx, current, error, state,
};
use crate::cpu::Cpu;

/// How deep the stack is (GL_MAX_ATTRIB_STACK_DEPTH).
pub const ATTRIB_DEPTH: usize = 16;

// The groups' bits.
const GL_CURRENT_BIT: u32 = 0x1;
const GL_POINT_BIT: u32 = 0x2;
const GL_LINE_BIT: u32 = 0x4;
const GL_POLYGON_BIT: u32 = 0x8;
const GL_PIXEL_MODE_BIT: u32 = 0x20;
pub const GL_LIGHTING_BIT: u32 = 0x40;
const GL_FOG_BIT: u32 = 0x80;
const GL_DEPTH_BUFFER_BIT: u32 = 0x100;
const GL_STENCIL_BUFFER_BIT: u32 = 0x400;
const GL_VIEWPORT_BIT: u32 = 0x800;
const GL_TRANSFORM_BIT: u32 = 0x1000;
pub const GL_ENABLE_BIT: u32 = 0x2000;
const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
const GL_LIST_BIT: u32 = 0x2_0000;
const GL_TEXTURE_BIT: u32 = 0x4_0000;
const GL_SCISSOR_BIT: u32 = 0x8_0000;

/// A texture's parameters, as the texture group keeps those of the
/// textures bound: its name, minification and magnification filters and
/// wrap modes.
pub type TexParams = [u32; 5];

/// What a push keeps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attrib {
    pub mask: u32,
    pub caps: u32,
    pub texture_2d: [bool; UNITS],
    pub color: [f32; 4],
    pub tex_coord: [[f32; 4]; UNITS],
    pub normal: [f32; 3],
    pub lighting: Lighting,
    pub shade_model: u32,
    pub matrix_mode: u32,
    pub fog_mode: u32,
    pub fog: [f32; 3],
    pub fog_color: [f32; 4],
    pub depth_func: u32,
    pub depth_mask: bool,
    pub clear_depth: f32,
    pub alpha_func: u32,
    pub alpha_ref: f32,
    pub blend: [u32; 2],
    pub draw_buffer: u32,
    pub color_mask: [bool; 4],
    pub clear_color: [f32; 4],
    pub cull_face: u32,
    pub front_face: u32,
    pub polygon_mode: u32,
    pub offset: [f32; 2],
    pub scissor: [i32; 4],
    pub viewport: [i32; 4],
    pub depth_range: [f32; 2],
    pub point_size: f32,
    pub line_width: f32,
    pub read_buffer: u32,
    pub clear_stencil: u32,
    pub active_unit: u32,
    pub env_mode: [u32; UNITS],
    pub env_color: [[f32; 4]; UNITS],
    pub bound: [u32; UNITS],
    pub tex_params: [TexParams; UNITS],
    pub list_base: u32,
}

/// The capabilities each group keeps the enables of; the enable group
/// keeps all of them.
const GROUP_CAPS: &[(u32, u32)] = &[
    (GL_LIGHTING_BIT, CAP_LIGHTING | CAP_COLOR_MATERIAL),
    (GL_TRANSFORM_BIT, CAP_NORMALIZE | CAP_CLIP_PLANES),
    (GL_FOG_BIT, CAP_FOG),
    (GL_DEPTH_BUFFER_BIT, CAP_DEPTH_TEST),
    (GL_STENCIL_BUFFER_BIT, CAP_STENCIL_TEST),
    (
        GL_COLOR_BUFFER_BIT,
        CAP_ALPHA_TEST | CAP_BLEND | CAP_DITHER | CAP_LOGIC_OP,
    ),
    (
        GL_POLYGON_BIT,
        CAP_CULL_FACE
            | CAP_POLYGON_OFFSET_FILL
            | CAP_POLYGON_OFFSET_LINE
            | CAP_POLYGON_OFFSET_POINT
            | CAP_POLYGON_SMOOTH
            | CAP_POLYGON_STIPPLE,
    ),
    (GL_SCISSOR_BIT, CAP_SCISSOR_TEST),
    (GL_POINT_BIT, CAP_POINT_SMOOTH),
    (GL_LINE_BIT, CAP_LINE_SMOOTH | CAP_LINE_STIPPLE),
    (GL_TEXTURE_BIT, CAP_TEXTURE_GEN),
];

/// The parameters of the texture `name`, if there is one: the
/// context's default texture, or one of `shared`, its group's.
fn tex_params(c: &Context, shared: &[Texture], name: u32) -> TexParams {
    match super::texture::find(c, shared, name) {
        Some(t) => [name, t.min_filter, t.mag_filter, t.wrap_s, t.wrap_t],
        None => [name, 0, 0, 0, 0],
    }
}

/// What a push of `mask` keeps of `c`, whose group's textures are
/// `shared`.
fn save(c: &Context, shared: &[Texture], mask: u32) -> Attrib {
    Attrib {
        mask,
        caps: c.caps,
        texture_2d: c.texture_2d,
        color: c.color,
        tex_coord: c.tex_coord,
        normal: c.normal,
        lighting: c.lighting,
        shade_model: c.shade_model,
        matrix_mode: c.matrix_mode,
        fog_mode: c.fog_mode,
        fog: c.fog,
        fog_color: c.fog_color,
        depth_func: c.depth_func,
        depth_mask: c.depth_mask,
        clear_depth: c.clear_depth,
        alpha_func: c.alpha_func,
        alpha_ref: c.alpha_ref,
        blend: c.blend,
        draw_buffer: c.draw_buffer,
        color_mask: c.color_mask,
        clear_color: c.clear_color,
        cull_face: c.cull_face,
        front_face: c.front_face,
        polygon_mode: c.polygon_mode,
        offset: c.offset,
        scissor: c.scissor,
        viewport: c.viewport,
        depth_range: c.depth_range,
        point_size: c.point_size,
        line_width: c.line_width,
        read_buffer: c.read_buffer,
        clear_stencil: c.clear_stencil,
        active_unit: c.active_unit,
        env_mode: c.env_mode,
        env_color: c.env_color,
        bound: c.bound,
        tex_params: std::array::from_fn(|u| tex_params(c, shared, c.bound[u])),
        list_base: c.lists.base,
    }
}

/// Put back in `c` the groups `a` was pushed with, but the bound
/// textures' parameters, which aren't the context's alone
/// (`texture::restore_params`).
fn restore(c: &mut Context, a: &Attrib) {
    let m = a.mask;
    let on = |bit: u32| m & bit != 0;
    let mut caps = 0;
    if on(GL_ENABLE_BIT) {
        caps = u32::MAX;
    } else {
        for &(bit, group) in GROUP_CAPS {
            if on(bit) {
                caps |= group;
            }
        }
    }
    c.caps = c.caps & !caps | a.caps & caps;
    if on(GL_ENABLE_BIT) || on(GL_TEXTURE_BIT) {
        c.texture_2d = a.texture_2d;
    }
    if on(GL_ENABLE_BIT) || on(GL_LIGHTING_BIT) {
        c.lighting.enabled = a.lighting.enabled;
    }
    if on(GL_CURRENT_BIT) {
        c.color = a.color;
        c.tex_coord = a.tex_coord;
        c.normal = a.normal;
    }
    if on(GL_LIGHTING_BIT) {
        c.lighting = a.lighting;
        c.shade_model = a.shade_model;
    }
    if on(GL_TRANSFORM_BIT) {
        c.matrix_mode = a.matrix_mode;
    }
    if on(GL_FOG_BIT) {
        c.fog_mode = a.fog_mode;
        c.fog = a.fog;
        c.fog_color = a.fog_color;
    }
    if on(GL_DEPTH_BUFFER_BIT) {
        c.depth_func = a.depth_func;
        c.depth_mask = a.depth_mask;
        c.clear_depth = a.clear_depth;
    }
    if on(GL_STENCIL_BUFFER_BIT) {
        c.clear_stencil = a.clear_stencil;
    }
    if on(GL_COLOR_BUFFER_BIT) {
        c.alpha_func = a.alpha_func;
        c.alpha_ref = a.alpha_ref;
        c.blend = a.blend;
        c.draw_buffer = a.draw_buffer;
        c.color_mask = a.color_mask;
        c.clear_color = a.clear_color;
    }
    if on(GL_POLYGON_BIT) {
        c.cull_face = a.cull_face;
        c.front_face = a.front_face;
        c.polygon_mode = a.polygon_mode;
        c.offset = a.offset;
    }
    if on(GL_SCISSOR_BIT) {
        c.scissor = a.scissor;
    }
    if on(GL_VIEWPORT_BIT) {
        c.viewport = a.viewport;
        c.depth_range = a.depth_range;
    }
    if on(GL_POINT_BIT) {
        c.point_size = a.point_size;
    }
    if on(GL_LINE_BIT) {
        c.line_width = a.line_width;
    }
    if on(GL_PIXEL_MODE_BIT) {
        c.read_buffer = a.read_buffer;
    }
    if on(GL_LIST_BIT) {
        c.lists.base = a.list_base;
    }
    if on(GL_TEXTURE_BIT) {
        c.active_unit = a.active_unit;
        c.env_mode = a.env_mode;
        c.env_color = a.env_color;
        c.bound = a.bound;
    }
}

/// glPushAttrib(mask).
pub fn push_attrib(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((index, _)) = current(cpu) else {
        return Some(0);
    };
    let s = state(cpu);
    let Some(Some(c)) = s.contexts.get_mut(index) else {
        return Some(0);
    };
    let e = if c.begin.is_some() {
        GL_INVALID_OPERATION
    } else if c.attribs.len() >= ATTRIB_DEPTH {
        GL_STACK_OVERFLOW
    } else {
        let a = save(c, s.lists.textures(c.lists.group), args[0]);
        c.attribs.push(a);
        return Some(0);
    };
    error(cpu, e);
    Some(0)
}

/// glPopAttrib().
pub fn pop_attrib(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    if c.begin.is_some() {
        error(cpu, GL_INVALID_OPERATION);
        return Some(0);
    }
    match c.attribs.pop() {
        Some(a) => {
            restore(c, &a);
            if a.mask & GL_TEXTURE_BIT != 0 {
                super::texture::restore_params(cpu, &a.tex_params);
            }
        }
        None => error(cpu, GL_STACK_UNDERFLOW),
    }
    Some(0)
}

crate::state_fields!(Attrib {
    mask,
    caps,
    texture_2d,
    color,
    tex_coord,
    normal,
    lighting,
    shade_model,
    matrix_mode,
    fog_mode,
    fog,
    fog_color,
    depth_func,
    depth_mask,
    clear_depth,
    alpha_func,
    alpha_ref,
    blend,
    draw_buffer,
    color_mask,
    clear_color,
    cull_face,
    front_face,
    polygon_mode,
    offset,
    scissor,
    viewport,
    depth_range,
    point_size,
    line_width,
    read_buffer,
    clear_stencil,
    active_unit,
    env_mode,
    env_color,
    bound,
    tex_params,
    list_base
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pop_puts_back_only_the_groups_pushed() {
        let mut c = super::super::new_context(0, 0, 1);
        let a = save(&c, &[], GL_LIGHTING_BIT);
        c.caps |= CAP_LIGHTING | CAP_BLEND;
        c.lighting.enabled = 0b101;
        c.lighting.two_side = true;
        c.shade_model = 0x1D00;
        c.blend = [0x302, 0x303];
        restore(&mut c, &a);
        assert_eq!(c.caps & CAP_LIGHTING, 0);
        assert_eq!(c.lighting.enabled, 0);
        assert!(!c.lighting.two_side);
        assert_eq!(c.shade_model, 0x1D01);
        // The blending enable and factors aren't the lighting group's.
        assert_ne!(c.caps & CAP_BLEND, 0);
        assert_eq!(c.blend, [0x302, 0x303]);
    }

    #[test]
    fn the_enable_group_keeps_every_enable_and_nothing_else() {
        let mut c = super::super::new_context(0, 0, 1);
        let a = save(&c, &[], GL_ENABLE_BIT);
        c.caps |= CAP_LIGHTING | CAP_BLEND | CAP_NORMALIZE;
        c.lighting.enabled = 0xFF;
        c.texture_2d[0] = true;
        c.lighting.local_viewer = true;
        restore(&mut c, &a);
        assert_eq!(c.caps, super::super::CAP_DITHER);
        assert_eq!(c.lighting.enabled, 0);
        assert!(!c.texture_2d[0]);
        // A lighting parameter isn't an enable.
        assert!(c.lighting.local_viewer);
    }
}
