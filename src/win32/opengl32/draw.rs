//! Drawing: glBegin and glEnd, the vertex arrays, glClear and
//! glReadPixels.
//!
//! Before it draws, a context puts its state on its Direct3D device
//! (`apply`): the matrices as the world and projection transforms, the
//! viewport, render states and texture stages, and the scissor box,
//! depth bias and color mask the HAL takes from the device beside
//! Direct3D 7's. The vertices then go through Direct3D's pipeline as
//! DrawIndexedPrimitive's would, with the indices of glDrawElements.
//! OpenGL's primitives are assembled here (`primitives`), with the
//! vertex flat shading takes the colors of where OpenGL has it, which
//! is not where Direct3D has it.
//!
//! With GL_LIGHTING on, each vertex is lit here as it is given
//! (`shade.rs`), and reaches the pipeline with its lit color and
//! Direct3D's lighting off. With two-sided lighting a vertex has a
//! front and a back color, and each polygon takes the ones of the side
//! it shows (`back_faces`).

use super::super::ddraw::d3d::device::{
    self as inner_device, RS_ALPHAFUNC, RS_BLENDOP, RS_CLIPPING, RS_COLORWRITEENABLE, RS_CULLMODE,
    RS_DESTBLEND, RS_FILLMODE, RS_FOGDENSITY, RS_FOGENABLE, RS_FOGEND, RS_FOGSTART,
    RS_FOGTABLEMODE, RS_FOGVERTEXMODE, RS_LIGHTING, RS_SHADEMODE, RS_SPECULARENABLE, RS_SRCBLEND,
    RS_ZENABLE, RS_ZFUNC, RS_ZWRITEENABLE, TA_CURRENT, TA_DIFFUSE, TA_TEXTURE, TOP_ADD,
    TOP_BLENDTEXTUREALPHA, TOP_DISABLE, TOP_MODULATE, TOP_SELECTARG1, TOP_SELECTARG2, TSS_ADDRESSU,
    TSS_ADDRESSV, TSS_ALPHAARG1, TSS_ALPHAARG2, TSS_ALPHAOP, TSS_COLORARG1, TSS_COLORARG2,
    TSS_COLOROP, TSS_MAGFILTER, TSS_MINFILTER, TSS_MIPFILTER, TSS_TEXCOORDINDEX,
};
use super::super::ddraw::d3d::hal::{
    RS_ALPHABLENDENABLE, RS_ALPHAREF, RS_ALPHATESTENABLE, RS_COLORKEYENABLE, RS_DITHERENABLE,
    RS_FOGCOLOR,
};
use super::super::ddraw::d3d::pipeline::{
    self, CULL_CCW, CULL_CW, FVF_DIFFUSE, FVF_XYZ, IDENTITY, Layout, Matrix, Prim, Vertex,
};
use super::super::ddraw::d3d::{draw as d3d_draw, hal, states};
use super::light::Lighting;
use super::matrix::{self, mul};
use super::shade::{self, Input};
use super::state::{
    GL_BACK, GL_CW, GL_EXP, GL_EXP2, GL_FLAT, GL_FRONT_AND_BACK, GL_LINE, GL_POINT,
};
use super::texture::{self, GL_ADD, GL_CLAMP, GL_DECAL, GL_LINEAR, GL_REPLACE};
use super::{
    Array, CAP_ALPHA_TEST, CAP_BLEND, CAP_COLOR_MATERIAL, CAP_CULL_FACE, CAP_DEPTH_TEST,
    CAP_DITHER, CAP_FOG, CAP_LIGHTING, CAP_NORMALIZE, CAP_POLYGON_OFFSET_FILL, CAP_SCISSOR_TEST,
    Context, GL_INVALID_ENUM, GL_INVALID_OPERATION, GL_INVALID_VALUE, ImmVertex, Settings, UNITS,
    ctx,
    current, error, signed_unit, state,
};
use crate::cpu::Cpu;

// Primitives.
const GL_POINTS: u32 = 0;
const GL_LINES: u32 = 1;
const GL_LINE_LOOP: u32 = 2;
const GL_LINE_STRIP: u32 = 3;
const GL_TRIANGLES: u32 = 4;
const GL_TRIANGLE_STRIP: u32 = 5;
const GL_TRIANGLE_FAN: u32 = 6;
const GL_QUADS: u32 = 7;
const GL_QUAD_STRIP: u32 = 8;
const GL_POLYGON: u32 = 9;

// Types of arrays and indices.
const GL_BYTE: u32 = 0x1400;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_SHORT: u32 = 0x1402;
const GL_UNSIGNED_SHORT: u32 = 0x1403;
const GL_INT: u32 = 0x1404;
const GL_UNSIGNED_INT: u32 = 0x1405;
const GL_FLOAT: u32 = 0x1406;
const GL_DOUBLE: u32 = 0x140A;

// glClear's buffers.
const GL_DEPTH_BUFFER_BIT: u32 = 0x100;
const GL_STENCIL_BUFFER_BIT: u32 = 0x400;
const GL_COLOR_BUFFER_BIT: u32 = 0x4000;

// Formats glReadPixels writes.
const GL_RGB: u32 = 0x1907;
const GL_RGBA: u32 = 0x1908;
const GL_BGR_EXT: u32 = 0x80E0;
const GL_BGRA_EXT: u32 = 0x80E1;
const GL_DEPTH_COMPONENT: u32 = 0x1902;

/// Each vertex as the pipeline takes it: a position, a diffuse color
/// and two sets of two texture coordinates.
fn layout() -> Layout {
    Layout::of(FVF_XYZ | FVF_DIFFUSE | 2 << 8).expect("a valid FVF")
}

/// OpenGL's depth from -1 to 1 as Direct3D's from 0 to 1: z' = (z +
/// w) / 2, applied after the projection.
const DEPTH_HALF: Matrix = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 0.5, 0.0, //
    0.0, 0.0, 0.5, 1.0,
];

/// Direct3D's compare function of OpenGL's (GL_NEVER to GL_ALWAYS).
fn compare(func: u32) -> u32 {
    func.wrapping_sub(0x0200).min(7) + 1
}

/// Direct3D's blend factor of OpenGL's.
fn blend_factor(f: u32) -> u32 {
    match f {
        0 => 1,
        1 => 2,
        // GL_SRC_COLOR to GL_SRC_ALPHA_SATURATE.
        0x0300 => 3,
        0x0301 => 4,
        0x0302 => 5,
        0x0303 => 6,
        0x0304 => 7,
        0x0305 => 8,
        0x0306 => 9,
        0x0307 => 10,
        0x0308 => 11,
        _ => 2,
    }
}

/// Direct3D 8's D3DCOLORWRITEENABLE bits of glColorMask's red, green,
/// blue and alpha.
fn color_write(mask: [bool; 4]) -> u32 {
    mask.iter()
        .enumerate()
        .fold(0, |bits, (i, &on)| bits | (on as u32) << i)
}

/// D3DCOLOR (ARGB) of red, green, blue and alpha from 0 to 1.
fn argb(c: [f32; 4]) -> u32 {
    pipeline::color(c)
}

/// What lighting the vertices of one draw take from their context. The
/// modelview matrix and the lights can't change between glBegin and
/// glEnd. It borrows the lights and materials: glVertex makes one for
/// each vertex.
struct Lit<'a> {
    lighting: &'a Lighting,
    modelview: Matrix,
    normals: Option<[f32; 9]>,
    normalize: bool,
    color_material: bool,
}

impl<'a> Lit<'a> {
    /// The context's, None with GL_LIGHTING off.
    fn of(c: &'a Context) -> Option<Lit<'a>> {
        if c.caps & CAP_LIGHTING == 0 {
            return None;
        }
        let modelview = c.modelview.last().copied().unwrap_or(IDENTITY);
        Some(Lit {
            lighting: &c.lighting,
            modelview,
            normals: shade::normal_matrix(&modelview),
            normalize: c.caps & CAP_NORMALIZE != 0,
            color_material: c.caps & CAP_COLOR_MATERIAL != 0,
        })
    }

    /// The front and back colors (ARGB) of the vertex at `position` in
    /// object coordinates, with the normal `normal` and the current
    /// color `color`.
    fn colors(&self, position: [f32; 4], normal: [f32; 3], color: [f32; 4]) -> [u32; 2] {
        let input = Input {
            position: matrix::apply(&self.modelview, position),
            normal: shade::eye_normal(self.normals.as_ref(), normal, self.normalize),
            color: self.color_material.then_some(color),
        };
        shade::shade(self.lighting, &input).map(argb)
    }
}

/// Whether the context's vertices have back colors of their own, which
/// they have with GL_LIGHTING on and two-sided lighting.
fn two_sided(c: &Context) -> bool {
    c.caps & CAP_LIGHTING != 0 && c.lighting.two_side
}

/// The front and back colors of a vertex: lit by `lit`, or the color
/// `color` with lighting off.
fn vertex_colors(
    lit: Option<&Lit>,
    position: [f32; 4],
    normal: [f32; 3],
    color: [f32; 4],
) -> [u32; 2] {
    match lit {
        Some(l) => l.colors(position, normal, color),
        None => [argb(color); 2],
    }
}

/// The scissor box as the target's rectangle (left, top, right,
/// bottom).
fn scissor_rect(c: &Settings) -> [u32; 4] {
    let [x, y, w, h] = c.scissor;
    let height = c.height as i32;
    let left = x.clamp(0, c.width as i32);
    let right = (x + w.max(0)).clamp(0, c.width as i32);
    let top = (height - (y + h.max(0))).clamp(0, height);
    let bottom = (height - y).clamp(0, height);
    [left as u32, top as u32, right as u32, bottom as u32]
}

/// The texture stage states of unit `u` drawing with `t` in the
/// environment `mode`, None for a unit that doesn't texture.
fn stage_states(mode: u32, t: &super::Texture) -> Vec<(u32, u32)> {
    let alpha = t.alpha;
    let (color_op, alpha_op) = match mode {
        GL_REPLACE => (
            TOP_SELECTARG1,
            if alpha {
                TOP_SELECTARG1
            } else {
                TOP_SELECTARG2
            },
        ),
        GL_DECAL => (
            if alpha {
                TOP_BLENDTEXTUREALPHA
            } else {
                TOP_SELECTARG1
            },
            TOP_SELECTARG2,
        ),
        GL_ADD => (TOP_ADD, if alpha { TOP_MODULATE } else { TOP_SELECTARG2 }),
        // GL_MODULATE, and GL_BLEND the nearest way the card has.
        _ => (
            TOP_MODULATE,
            if alpha { TOP_MODULATE } else { TOP_SELECTARG2 },
        ),
    };
    let linear = |f: u32| f == GL_LINEAR || f == 0x2701 || f == 0x2703;
    let mipmapped = (0x2700..=0x2703).contains(&t.min_filter);
    let complete = !t.levels.is_empty() && t.defined.count_ones() as usize == t.levels.len();
    let mip = if !mipmapped || !complete || t.levels.len() < 2 {
        1
    } else if t.min_filter >= 0x2702 {
        3
    } else {
        2
    };
    let address = |w: u32| if w == GL_CLAMP { 3 } else { 1 };
    vec![
        (TSS_COLOROP, color_op),
        (TSS_COLORARG1, TA_TEXTURE),
        (TSS_COLORARG2, TA_CURRENT),
        (TSS_ALPHAOP, alpha_op),
        (TSS_ALPHAARG1, TA_TEXTURE),
        (TSS_ALPHAARG2, TA_CURRENT),
        (TSS_MAGFILTER, if linear(t.mag_filter) { 2 } else { 1 }),
        (TSS_MINFILTER, if linear(t.min_filter) { 2 } else { 1 }),
        (TSS_MIPFILTER, mip),
        (TSS_ADDRESSU, address(t.wrap_s)),
        (TSS_ADDRESSV, address(t.wrap_t)),
    ]
}

/// A context's state as its Direct3D device's: what `put` puts on the
/// device before a draw. Made from what the context's draws are made
/// with and nothing else, so it holds while that has its stamp
/// (`applied_of`).
#[derive(Clone, Debug, Default)]
pub struct Applied {
    inner: u32,
    render: Vec<(u32, u32)>,
    stages: Vec<(u32, u32, u32)>,
    textures: [u32; UNITS],
    world: Matrix,
    projection: Matrix,
    viewport: inner_device::Viewport7,
    scissor: Option<[u32; 4]>,
    depth_bias: i16,
    color_off: bool,
}

crate::state_fields!(Applied {
    inner,
    render,
    stages,
    textures,
    world,
    projection,
    viewport,
    scissor,
    depth_bias,
    color_off
});

impl Applied {
    /// Whether a draw with `other` is drawn as one with this: the same
    /// to the last bit.
    fn same(&self, other: &Applied) -> bool {
        let bits = |m: &Matrix| m.map(f32::to_bits);
        self.inner == other.inner
            && self.render == other.render
            && self.stages == other.stages
            && self.textures == other.textures
            && bits(&self.world) == bits(&other.world)
            && bits(&self.projection) == bits(&other.projection)
            && same_viewport(&self.viewport, &other.viewport)
            && self.scissor == other.scissor
            && self.depth_bias == other.depth_bias
            && self.color_off == other.color_off
    }
}

/// The device's state for the draws of the context `c` as it is now,
/// None when its viewport is off its buffers and nothing is drawn. It
/// is made once for each stamp of the context: a draw that follows
/// another with nothing set between them finds it made.
fn applied_of(c: &mut Context) -> Option<&Applied> {
    let stamp = c.stamp();
    if c.applied.as_ref().is_none_or(|(made, _)| *made != stamp) {
        let made = applied(c);
        c.applied = Some((stamp, made));
    }
    c.applied.as_ref()?.1.as_ref()
}

/// The device's state for drawing what a context draws with `c`: None
/// when its viewport is off its buffers, and nothing is drawn.
fn applied(c: &Settings) -> Option<Applied> {
    let (viewport, fit) = super::wgl::inner_viewport(c)?;
    let on = |cap: u32| c.caps & cap != 0;
    let depth = on(CAP_DEPTH_TEST);
    let cull = if !on(CAP_CULL_FACE) {
        // D3DCULL_NONE.
        1
    } else {
        // Faces that are clockwise on the screen are the back ones when
        // the front ones are counter-clockwise.
        let back = c.cull_face == GL_BACK;
        let ccw_front = c.front_face != GL_CW;
        if back == ccw_front { CULL_CW } else { CULL_CCW }
    };
    let fog_mode = match c.fog_mode {
        GL_EXP => pipeline::FOG_EXP,
        GL_EXP2 => pipeline::FOG_EXP2,
        _ => pipeline::FOG_LINEAR,
    };
    let fill = match c.polygon_mode {
        GL_POINT => 1,
        GL_LINE => 2,
        _ => 3,
    };
    let render = vec![
        (RS_ZENABLE, depth as u32),
        (RS_ZWRITEENABLE, (depth && c.depth_mask) as u32),
        (RS_ZFUNC, compare(c.depth_func)),
        (RS_ALPHATESTENABLE, on(CAP_ALPHA_TEST) as u32),
        (
            RS_ALPHAREF,
            (c.alpha_ref.clamp(0.0, 1.0) * 255.0 + 0.5) as u32,
        ),
        (RS_ALPHAFUNC, compare(c.alpha_func)),
        (RS_ALPHABLENDENABLE, on(CAP_BLEND) as u32),
        (RS_SRCBLEND, blend_factor(c.blend[0])),
        (RS_DESTBLEND, blend_factor(c.blend[1])),
        (RS_DITHERENABLE, on(CAP_DITHER) as u32),
        (RS_COLORKEYENABLE, 0),
        (RS_SPECULARENABLE, 0),
        (RS_FOGENABLE, on(CAP_FOG) as u32),
        (RS_FOGVERTEXMODE, fog_mode),
        (RS_FOGTABLEMODE, pipeline::FOG_NONE),
        (RS_FOGSTART, c.fog[1].to_bits()),
        (RS_FOGEND, c.fog[2].to_bits()),
        (RS_FOGDENSITY, c.fog[0].to_bits()),
        (RS_FOGCOLOR, argb(c.fog_color)),
        (RS_CULLMODE, cull),
        (RS_FILLMODE, fill),
        (RS_SHADEMODE, if c.shade_model == GL_FLAT { 1 } else { 2 }),
        (RS_LIGHTING, 0),
        (RS_CLIPPING, 1),
        // A card that writes some channels of a color and not others
        // takes the color mask from here (`color_off` below is the
        // mask of one that writes all of them or none).
        (RS_COLORWRITEENABLE, color_write(c.color_mask)),
        // D3DBLENDOP_ADD: OpenGL 1.1 has no other.
        (RS_BLENDOP, 1),
    ];

    // The units that texture: the first ones, each with a texture that
    // has its level 0.
    let mut stages = Vec::new();
    let mut textures = [0; UNITS];
    let mut stage = 0;
    for unit in 0..UNITS {
        if !c.texture_2d[unit] {
            continue;
        }
        let Some(t) = texture::texture(c, c.bound[unit]) else {
            continue;
        };
        if t.iface == 0 || t.defined & 1 == 0 {
            continue;
        }
        for (s, v) in stage_states(c.env_mode[unit], t) {
            stages.push((stage, s, v));
        }
        stages.push((stage, TSS_TEXCOORDINDEX, unit as u32));
        textures[stage as usize] = t.iface;
        stage += 1;
    }
    if stage == 0 {
        // The vertices' color as it is.
        stages.extend([
            (0, TSS_COLOROP, TOP_SELECTARG1),
            (0, TSS_COLORARG1, TA_DIFFUSE),
            (0, TSS_ALPHAOP, TOP_SELECTARG1),
            (0, TSS_ALPHAARG1, TA_DIFFUSE),
        ]);
        stage = 1;
    }
    for s in stage..UNITS as u32 {
        stages.push((s, TSS_COLOROP, TOP_DISABLE));
        stages.push((s, TSS_ALPHAOP, TOP_DISABLE));
    }

    // Polygon offset: the units, and the factor as if the slope were
    // one step, as the card adds one bias to a whole triangle.
    let depth_bias = if on(CAP_POLYGON_OFFSET_FILL) {
        (c.offset[0] + c.offset[1]).round().clamp(-32768.0, 32767.0) as i16
    } else {
        0
    };
    let projection = c.projection.last().copied().unwrap_or(IDENTITY);
    Some(Applied {
        inner: c.inner,
        render,
        stages,
        textures,
        world: c.modelview.last().copied().unwrap_or(IDENTITY),
        projection: mul(&fit, &mul(&DEPTH_HALF, &projection)),
        viewport,
        scissor: on(CAP_SCISSOR_TEST).then(|| scissor_rect(c)),
        depth_bias,
        color_off: !c.color_mask[..3].iter().any(|&m| m),
    })
}

/// Put the state `a` on its device. False if the device is gone.
fn put(cpu: &mut Cpu, a: &Applied) -> bool {
    let Some(d) = inner_device::get(cpu, a.inner) else {
        return false;
    };
    // Most draws follow one with the same state. Only what differs is
    // written: a write gives the device a new stamp, and the HAL then
    // binds the card's state anew (`inner_device::Device`).
    for &(s, v) in &a.render {
        if d.render[s as usize] != v {
            d.render[s as usize] = v;
        }
    }
    for &(stage, s, v) in &a.stages {
        let at = (stage * 32 + s) as usize;
        if d.stages[at] != v {
            d.stages[at] = v;
        }
    }
    for (which, matrix) in [(1, a.world), (2, IDENTITY), (3, a.projection)] {
        if d.transforms[which].map(f32::to_bits) != matrix.map(f32::to_bits) {
            d.transforms[which] = matrix;
        }
    }
    if !same_viewport(&d.viewport, &a.viewport) {
        d.viewport = a.viewport;
    }
    if d.scissor != a.scissor {
        d.scissor = a.scissor;
    }
    if d.depth_bias != a.depth_bias {
        d.depth_bias = a.depth_bias;
    }
    if d.color_off != a.color_off {
        d.color_off = a.color_off;
    }
    if !d.gl_pixel_centres {
        d.gl_pixel_centres = true;
    }
    let held: Vec<u32> = d.textures.iter().take(UNITS).copied().collect();
    for (stage, &iface) in a.textures.iter().enumerate() {
        if held[stage] != iface {
            states::set_stage_texture(cpu, a.inner, stage as u32, iface);
        }
    }
    true
}

/// Whether two viewports are the same to the last bit.
fn same_viewport(a: &inner_device::Viewport7, b: &inner_device::Viewport7) -> bool {
    states::viewport_words(a) == states::viewport_words(b)
}

/// The primitives of `kind` (OpenGL's) that the vertices `order`
/// (places in the vertices) make. Each takes its flat color from the
/// vertex the OpenGL 1.1 specification gives (table 2.6): the last
/// vertex of a line, of an independent triangle and of a quad, the
/// third of each triangle of a strip or fan, and the first of a
/// polygon. Direct3D takes the first vertex of a line or triangle and
/// the second of a fan's triangle.
fn primitives(kind: u32, order: &[u32]) -> Vec<Prim> {
    let n = order.len();
    let mut out = Vec::new();
    match kind {
        GL_POINTS => out.extend(order.iter().map(|&i| Prim::Point(i))),
        GL_LINES => out.extend(
            order
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[a, b]| Prim::Line(a, b, b)),
        ),
        GL_LINE_STRIP | GL_LINE_LOOP => {
            out.extend(order.windows(2).map(|w| Prim::Line(w[0], w[1], w[1])));
            // A loop's last line goes back to the first vertex, and
            // takes its color.
            if kind == GL_LINE_LOOP && n >= 2 {
                out.push(Prim::Line(order[n - 1], order[0], order[0]));
            }
        }
        GL_TRIANGLES => out.extend(
            order
                .as_chunks::<3>()
                .0
                .iter()
                .map(|&[a, b, c]| Prim::Triangle([a, b, c], c, 7)),
        ),
        // A quad strip's vertices are a triangle strip's, two triangles
        // to a quad, both with the quad's last vertex's color. The
        // diagonal between them is no edge of the quad, so wireframe
        // leaves it out: the first triangle's second edge, the second's
        // first.
        GL_TRIANGLE_STRIP | GL_QUAD_STRIP => {
            let quads = kind == GL_QUAD_STRIP;
            let n = if quads { n & !1 } else { n };
            for i in 0..n.saturating_sub(2) {
                let (a, b, c) = (order[i], order[i + 1], order[i + 2]);
                // Every other triangle the other way round, so that all
                // wind as the first does.
                let v = if i % 2 == 0 { [a, b, c] } else { [b, a, c] };
                let (provoking, edges) = match (quads, i % 2) {
                    (true, 0) => (order[i + 3], 5),
                    (true, _) => (c, 6),
                    (false, _) => (c, 7),
                };
                out.push(Prim::Triangle(v, provoking, edges));
            }
        }
        // A polygon's wireframe is its outline: of the triangles of the
        // fan it is cut into, only the first's first edge and the last's
        // third are on it, besides each one's second.
        GL_TRIANGLE_FAN | GL_POLYGON => {
            for i in 1..n.saturating_sub(1) {
                let (provoking, edges) = if kind == GL_POLYGON {
                    let first = if i == 1 { 1 } else { 0 };
                    let last = if i + 2 == n { 4 } else { 0 };
                    (order[0], 2 | first | last)
                } else {
                    (order[i + 1], 7)
                };
                out.push(Prim::Triangle(
                    [order[0], order[i], order[i + 1]],
                    provoking,
                    edges,
                ));
            }
        }
        // Two triangles to a quad, without the diagonal in wireframe.
        GL_QUADS => {
            for &[a, b, c, d] in order.as_chunks::<4>().0 {
                out.push(Prim::Triangle([a, b, c], d, 3));
                out.push(Prim::Triangle([a, c, d], d, 6));
            }
        }
        _ => {}
    }
    out
}

/// How many of the triangles `primitives` makes of `kind` make one
/// OpenGL polygon: two for a quad, all of them for a polygon.
fn polygon_triangles(kind: u32) -> usize {
    match kind {
        GL_QUADS | GL_QUAD_STRIP => 2,
        GL_POLYGON => usize::MAX,
        _ => 1,
    }
}

/// The determinant of a triangle's clip coordinates x, y and w: twice
/// its area in normalized device coordinates times the three w.
fn clip_det(a: [f64; 4], b: [f64; 4], d: [f64; 4]) -> f64 {
    a[0] * (b[1] * d[3] - d[1] * b[3]) - b[0] * (a[1] * d[3] - d[1] * a[3])
        + d[0] * (a[1] * b[3] - b[1] * a[3])
}

/// The vertices for the primitives `prims` of the context `c`, which
/// come `per` triangles to a polygon, when the vertices have the back
/// colors `backs`: `vertices` and then the same with the back colors,
/// with each polygon whose back shows taking the second ones.
///
/// Section 2.13.1 chooses by the sign of the polygon's area in window
/// coordinates. With all its corners in front of the eye (w > 0) that
/// is the sum of its triangles' areas in normalized device coordinates.
/// With one behind, each triangle takes the sign of the determinant of
/// its corners' clip coordinates x, y and w, which is the side that
/// shows of the part of it in front of the eye.
fn back_faces(
    c: &Context,
    vertices: &[Vertex],
    backs: &[u32],
    prims: &mut [Prim],
    per: usize,
) -> Vec<Vertex> {
    let modelview = c.modelview.last().copied().unwrap_or(IDENTITY);
    let projection = c.projection.last().copied().unwrap_or(IDENTITY);
    let m = mul(&projection, &modelview);
    let clip: Vec<[f64; 4]> = vertices
        .iter()
        .map(|v| matrix::apply(&m, v.position).map(|x| x as f64))
        .collect();
    let ccw_front = c.front_face != GL_CW;
    let n = vertices.len() as u32;
    let mut out = vertices.to_vec();
    out.extend(vertices.iter().zip(backs).map(|(v, &b)| Vertex {
        diffuse: Some(b),
        ..*v
    }));
    let shows_back = |area: f64| if ccw_front { area < 0.0 } else { area > 0.0 };
    for polygon in prims.chunks_mut(per) {
        let corners = |p: &Prim| match *p {
            Prim::Triangle(t, ..) => Some(t.map(|i| clip[i as usize])),
            _ => None,
        };
        let in_front = polygon
            .iter()
            .filter_map(corners)
            .all(|t| t.iter().all(|v| v[3] > 0.0));
        let area: f64 = polygon
            .iter()
            .filter_map(corners)
            .map(|[a, b, d]| clip_det(a, b, d) / (a[3] * b[3] * d[3]))
            .sum();
        for p in polygon.iter_mut() {
            let Some([a, b, d]) = corners(p) else {
                continue;
            };
            let back = if in_front {
                shows_back(area)
            } else {
                shows_back(clip_det(a, b, d))
            };
            if let Prim::Triangle(t, provoking, _) = p
                && back
            {
                *t = t.map(|i| i + n);
                *provoking += n;
            }
        }
    }
    out
}

/// Draws that wait to go to Direct3D as one: the primitives of
/// glBegin and glEnd pairs and array draws that follow each other
/// with the same state.
///
/// A program that draws a triangle or a quad at a time (Oni: 2700
/// glEnd a frame for 3450 triangles) would otherwise pay Direct3D's
/// work for a draw each time: the pipeline's setup, the card's state,
/// a command. The primitives and their order are those the program
/// gave, so the picture is the one it would be drawn one by one.
///
/// They hold everything they are drawn with by value: the device's
/// state (`Applied`), and vertices that the texture matrices and the
/// lights were applied to as they were given. So the program may set
/// states after them and they are still drawn as they were. What they
/// hold by name is the textures and the buffers, and what they change
/// is the buffers: `flush_waiting` draws them before any function
/// runs that isn't known to leave those alone (`keeps_waiting`).
#[derive(Clone, Debug, Default)]
pub struct Waiting {
    /// The context that draws them, and its stamp when the last of
    /// them was added: a draw of the context with that stamp has their
    /// state.
    context: u32,
    stamp: u64,
    applied: Applied,
    vertices: Vec<Vertex>,
    prims: Vec<Prim>,
}

/// The vertices that wait at most: a draw that would take them past
/// this goes after a flush.
const WAITING_VERTICES: usize = 8192;

// A saved state has the vertices as OpenGL's are (position, color and
// a set of coordinates for each unit) and each primitive as six words.
// The stamp isn't saved: a loaded context has another.
impl crate::savestate::State for Waiting {
    fn save(&self, w: &mut crate::savestate::Writer) {
        self.context.save(w);
        self.applied.save(w);
        let vertices: Vec<ImmVertex> = self
            .vertices
            .iter()
            .map(|v| ImmVertex {
                position: v.position,
                color: v.diffuse.unwrap_or(0),
                back: 0,
                tex: std::array::from_fn(|u| v.tex[u]),
            })
            .collect();
        vertices.save(w);
        let prims: Vec<[u32; 6]> = self
            .prims
            .iter()
            .map(|p| match *p {
                Prim::Point(a) => [0, a, 0, 0, 0, 0],
                Prim::Line(a, b, flat) => [1, a, b, 0, flat, 0],
                Prim::Triangle([a, b, c], flat, edges) => [2, a, b, c, flat, edges as u32],
            })
            .collect();
        prims.save(w);
    }

    fn load(&mut self, r: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        self.context.load(r)?;
        self.applied.load(r)?;
        let mut vertices: Vec<ImmVertex> = Vec::new();
        vertices.load(r)?;
        self.vertices = vertices.iter().map(waiting_vertex).collect();
        let mut prims: Vec<[u32; 6]> = Vec::new();
        prims.load(r)?;
        self.prims = prims
            .iter()
            .map(|&[kind, a, b, c, flat, edges]| match kind {
                0 => Prim::Point(a),
                1 => Prim::Line(a, b, flat),
                _ => Prim::Triangle([a, b, c], flat, edges as u8),
            })
            .collect();
        self.stamp = 0;
        Ok(())
    }
}

/// Draws that wait, of every kind of primitive, and whether two are
/// the same but for the stamp: for the test of their saved state.
#[cfg(test)]
pub fn waiting_for_tests() -> Waiting {
    let vertex = |x: f32| {
        let mut v = Vertex {
            position: [x, 2.0, 3.0, 1.0],
            diffuse: Some(0x8040_2010),
            ..Vertex::default()
        };
        v.tex[1] = [0.25, 0.5, 0.0, 1.0];
        v
    };
    Waiting {
        context: 0,
        stamp: 77,
        applied: Applied {
            inner: 5,
            render: vec![(7, 1)],
            stages: vec![(0, 1, 4)],
            textures: std::array::from_fn(|u| u as u32 + 9),
            world: std::array::from_fn(|i| i as f32),
            projection: IDENTITY,
            scissor: Some([1, 2, 3, 4]),
            depth_bias: -3,
            color_off: true,
            ..Applied::default()
        },
        vertices: (0..4).map(|i| vertex(i as f32)).collect(),
        prims: vec![
            Prim::Point(3),
            Prim::Line(0, 1, 1),
            Prim::Triangle([0, 1, 2], 2, 5),
        ],
    }
}

#[cfg(test)]
pub fn same_waiting(a: &Waiting, b: &Waiting) -> bool {
    a.context == b.context
        && a.applied.same(&b.applied)
        && a.vertices == b.vertices
        && a.prims == b.prims
}

/// A saved waiting vertex as the pipeline takes it.
fn waiting_vertex(v: &ImmVertex) -> Vertex {
    let mut out = Vertex {
        position: v.position,
        diffuse: Some(v.color),
        ..Vertex::default()
    };
    out.tex[..UNITS].copy_from_slice(&v.tex);
    out
}

/// The process of the context the draws `w` wait for, None when the
/// context is gone.
pub fn waiting_pid(w: &Waiting, s: &super::State) -> Option<u32> {
    let c = s.contexts.get(w.context as usize)?.as_ref()?;
    Some(c.pid)
}

/// Draw what waits (`Waiting`), if anything does.
pub fn flush_waiting(cpu: &mut Cpu) {
    let Some(w) = state(cpu).waiting.take() else {
        return;
    };
    let put = {
        let _profile = crate::profile::zone(crate::profile::Zone::OpenGlApply);
        put(cpu, &w.applied)
    };
    if put {
        let inner = w.applied.inner;
        let _ = d3d_draw::draw_prims(cpu, inner, 8, &layout(), &w.vertices, &w.prims, 0);
    }
}

/// Draw `kind` primitives (OpenGL's) of `vertices`, in the order
/// `order` or as they are. `backs` are the vertices' back colors when
/// they have their own, with two-sided lighting.
///
/// They join the draws that wait if those have the context's state,
/// and wait themselves otherwise, after those are drawn.
fn draw(
    cpu: &mut Cpu,
    kind: u32,
    vertices: &[Vertex],
    backs: Option<&[u32]>,
    order: Option<&[u32]>,
) {
    if vertices.is_empty() {
        return;
    }
    let _profile = crate::profile::zone(crate::profile::Zone::OpenGl);
    let Some((index, _)) = current(cpu) else {
        return;
    };
    let st = state(cpu);
    let Some(Some(c)) = st.contexts.get_mut(index) else {
        return;
    };
    if c.target == 0 {
        return;
    }
    let faces = !(c.caps & CAP_CULL_FACE != 0 && c.cull_face == GL_FRONT_AND_BACK);
    let all: Vec<u32>;
    let order = match order {
        Some(o) => o,
        None => {
            all = (0..vertices.len() as u32).collect();
            &all
        }
    };
    let triangles = matches!(
        kind,
        GL_TRIANGLES | GL_TRIANGLE_STRIP | GL_TRIANGLE_FAN | GL_QUADS | GL_QUAD_STRIP | GL_POLYGON
    );
    if triangles && !faces {
        return;
    }
    let mut prims = primitives(kind, order);
    if prims.is_empty() {
        return;
    }
    // Points and lines take the front colors.
    let sided = match backs {
        Some(backs) if triangles => Some(back_faces(
            c,
            vertices,
            backs,
            &mut prims,
            polygon_triangles(kind),
        )),
        _ => None,
    };
    let vertices = sided.as_deref().unwrap_or(vertices);
    let stamp = c.stamp();
    // The draws that wait take this one if they are the context's and
    // have its state: its stamp, or the state made anew and equal.
    if let Some(w) = &mut st.waiting
        && w.context == index as u32
        && w.vertices.len() + vertices.len() <= WAITING_VERTICES
        && (w.stamp == stamp || applied_of(c).is_some_and(|a| a.same(&w.applied)))
    {
        let first = w.vertices.len() as u32;
        w.stamp = stamp;
        w.vertices.extend_from_slice(vertices);
        w.prims.extend(prims.iter().map(|p| match *p {
            Prim::Point(a) => Prim::Point(a + first),
            Prim::Line(a, b, flat) => Prim::Line(a + first, b + first, flat + first),
            Prim::Triangle(t, flat, edges) => {
                Prim::Triangle(t.map(|i| i + first), flat + first, edges)
            }
        }));
        c.drawn = true;
        return;
    }
    let Some(applied) = applied_of(c).cloned() else {
        return;
    };
    c.drawn = true;
    let waiting = Waiting {
        context: index as u32,
        stamp,
        applied,
        vertices: vertices.to_vec(),
        prims,
    };
    flush_waiting(cpu);
    state(cpu).waiting = Some(waiting);
}

/// Whether a primitive is one there is.
fn valid_kind(kind: u32) -> bool {
    kind <= GL_POLYGON
}

// Immediate mode.

/// glBegin(mode).
pub fn begin(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !valid_kind(args[0]) {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    if c.begin.is_some() {
        error(cpu, GL_INVALID_OPERATION);
        return Some(0);
    }
    c.begin = Some(args[0]);
    c.vertices.clear();
    Some(0)
}

/// glEnd().
pub fn end(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let profile = crate::profile::zone(crate::profile::Zone::OpenGlEnd);
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    let Some(kind) = c.begin.take() else {
        error(cpu, GL_INVALID_OPERATION);
        return Some(0);
    };
    let gathered = std::mem::take(&mut c.vertices);
    let matrices = texture_matrices(c);
    let sided = two_sided(c);
    let vertices: Vec<Vertex> = gathered
        .iter()
        .map(|v| {
            let mut out = Vertex {
                position: v.position,
                diffuse: Some(v.color),
                ..Vertex::default()
            };
            for (u, m) in matrices.iter().enumerate() {
                out.tex[u] = texture_coords(m, v.tex[u]);
            }
            out
        })
        .collect();
    let backs: Option<Vec<u32>> = sided.then(|| gathered.iter().map(|v| v.back).collect());
    drop(profile);
    draw(cpu, kind, &vertices, backs.as_deref(), None);
    Some(0)
}

/// The texture matrices of the context's units that aren't the
/// identity.
fn texture_matrices(c: &Context) -> [Option<Matrix>; UNITS] {
    std::array::from_fn(|u| {
        c.texture_matrix[u]
            .last()
            .copied()
            .filter(|m| *m != IDENTITY)
    })
}

/// Texture coordinates through a texture matrix, and divided by q.
fn texture_coords(m: &Option<Matrix>, t: [f32; 4]) -> [f32; 4] {
    let t = match m {
        Some(m) => matrix::apply(m, t),
        None => t,
    };
    if t[3] != 1.0 && t[3] != 0.0 {
        [t[0] / t[3], t[1] / t[3], t[2] / t[3], 1.0]
    } else {
        t
    }
}

/// A vertex of glVertex: one with the current color and texture
/// coordinates, between glBegin and glEnd.
pub fn vertex(cpu: &mut Cpu, position: [f32; 4]) {
    let _profile = crate::profile::zone(crate::profile::Zone::OpenGlVertex);
    let Some(c) = ctx(cpu) else {
        return;
    };
    if c.begin.is_none() {
        return;
    }
    let [color, back] = vertex_colors(Lit::of(c).as_ref(), position, c.normal, c.color);
    let v = ImmVertex {
        position: homogeneous(position),
        color,
        back,
        tex: c.tex_coord,
    };
    c.vertices.push(v);
}

/// A position with w divided in, as the pipeline takes (x, y, z, 1).
fn homogeneous(p: [f32; 4]) -> [f32; 4] {
    if p[3] != 1.0 && p[3] != 0.0 {
        [p[0] / p[3], p[1] / p[3], p[2] / p[3], 1.0]
    } else {
        p
    }
}

// Vertex arrays.

/// The bytes of one value of `kind`.
fn type_size(kind: u32) -> Option<u32> {
    Some(match kind {
        GL_BYTE | GL_UNSIGNED_BYTE => 1,
        GL_SHORT | GL_UNSIGNED_SHORT => 2,
        GL_INT | GL_UNSIGNED_INT | GL_FLOAT => 4,
        GL_DOUBLE => 8,
        _ => return None,
    })
}

/// Set an array's pointer: `sizes` are the sizes it may have.
fn pointer(cpu: &mut Cpu, which: fn(&mut Context) -> &mut Array, args: [u32; 4], sizes: &[u32]) {
    let [size, kind, stride, at] = args;
    if !sizes.contains(&size) || (stride as i32) < 0 {
        error(cpu, GL_INVALID_VALUE);
        return;
    }
    if type_size(kind).is_none() {
        error(cpu, GL_INVALID_ENUM);
        return;
    }
    if let Some(c) = ctx(cpu) {
        let a = which(c);
        a.size = size;
        a.kind = kind;
        a.stride = stride;
        a.at = at;
    }
}

/// glVertexPointer(size, type, stride, pointer).
pub fn vertex_pointer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = [args[0], args[1], args[2], args[3]];
    pointer(cpu, |c| &mut c.vertex_array, a, &[2, 3, 4]);
    Some(0)
}

/// glColorPointer(size, type, stride, pointer).
pub fn color_pointer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = [args[0], args[1], args[2], args[3]];
    pointer(cpu, |c| &mut c.color_array, a, &[3, 4]);
    Some(0)
}

/// glTexCoordPointer(size, type, stride, pointer), for the client's
/// active unit.
pub fn tex_coord_pointer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = [args[0], args[1], args[2], args[3]];
    pointer(
        cpu,
        |c| &mut c.tex_array[c.client_unit as usize],
        a,
        &[1, 2, 3, 4],
    );
    Some(0)
}

/// glNormalPointer(type, stride, pointer): of signed types only.
pub fn normal_pointer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if ![GL_BYTE, GL_SHORT, GL_INT, GL_FLOAT, GL_DOUBLE].contains(&args[0]) {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    let a = [3, args[0], args[1], args[2]];
    pointer(cpu, |c| &mut c.normal_array, a, &[3]);
    Some(0)
}

/// glEdgeFlagPointer, glIndexPointer: kept nowhere, as nothing draws
/// with them.
pub fn ignore_pointer(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// glInterleavedArrays(format, stride, pointer).
pub fn interleaved_arrays(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [format, stride, at] = [args[0], args[1], args[2]];
    // Each format: texture coordinates, color (size, type), normal and
    // vertex sizes, as the table of the specification has them.
    let (t, color, n, v): (u32, Option<(u32, u32)>, bool, u32) = match format {
        0x2A20 => (0, None, false, 2),
        0x2A21 => (0, None, false, 3),
        0x2A22 => (0, Some((4, GL_UNSIGNED_BYTE)), false, 2),
        0x2A23 => (0, Some((4, GL_UNSIGNED_BYTE)), false, 3),
        0x2A24 => (0, Some((3, GL_FLOAT)), false, 3),
        0x2A25 => (0, None, true, 3),
        0x2A26 => (0, Some((4, GL_FLOAT)), true, 3),
        0x2A27 => (2, None, false, 3),
        0x2A28 => (4, None, false, 4),
        0x2A29 => (2, Some((4, GL_UNSIGNED_BYTE)), false, 3),
        0x2A2A => (2, Some((3, GL_FLOAT)), false, 3),
        0x2A2B => (2, None, true, 3),
        0x2A2C => (2, Some((4, GL_FLOAT)), true, 3),
        0x2A2D => (4, Some((4, GL_FLOAT)), true, 4),
        _ => {
            error(cpu, GL_INVALID_ENUM);
            return Some(0);
        }
    };
    let color_bytes = color.map_or(0, |(s, k)| if k == GL_FLOAT { 4 * s } else { 4 });
    let size = 4 * t + color_bytes + if n { 12 } else { 0 } + 4 * v;
    let stride = if stride == 0 { size } else { stride };
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    let unit = c.client_unit as usize;
    let mut offset = 0;
    c.tex_array[unit] = Array {
        enabled: t > 0,
        size: t,
        kind: GL_FLOAT,
        stride,
        at: at + offset,
    };
    offset += 4 * t;
    c.color_array = match color {
        Some((s, k)) => Array {
            enabled: true,
            size: s,
            kind: k,
            stride,
            at: at + offset,
        },
        None => Array::default(),
    };
    offset += color_bytes;
    c.normal_array = Array {
        enabled: n,
        size: 3,
        kind: GL_FLOAT,
        stride,
        at: at + offset,
    };
    offset += if n { 12 } else { 0 };
    c.vertex_array = Array {
        enabled: true,
        size: v,
        kind: GL_FLOAT,
        stride,
        at: at + offset,
    };
    Some(0)
}

/// An array's bytes for the elements `first` to `last`, and the stride
/// they are at in them. None if they aren't all in memory.
fn array_bytes(cpu: &mut Cpu, a: &Array, first: u32, last: u32) -> Option<(Vec<u8>, usize)> {
    let elem = a.size * type_size(a.kind)?;
    let stride = if a.stride == 0 { elem } else { a.stride };
    let start = a.at.checked_add(first.checked_mul(stride)?)?;
    let len = ((last - first) as usize) * stride as usize + elem as usize;
    if a.at == 0 || len > 0x1000_0000 || !cpu.bus.guest_probe(start, len, false) {
        return None;
    }
    let mut bytes = vec![0u8; len];
    cpu.bus.guest_read_bytes(start, &mut bytes);
    Some((bytes, stride as usize))
}

/// The `size` values of `kind` at the start of `b`, as floats; colors
/// of integers are 0 to 1.
fn values(b: &[u8], kind: u32, size: u32, normalize: bool) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (i, o) in out.iter_mut().enumerate().take(size as usize) {
        *o = match kind {
            GL_FLOAT => {
                let at = 4 * i;
                f32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
            }
            GL_DOUBLE => {
                let at = 8 * i;
                let mut d = [0u8; 8];
                d.copy_from_slice(&b[at..at + 8]);
                f64::from_le_bytes(d) as f32
            }
            GL_UNSIGNED_BYTE => {
                let v = b[i] as f32;
                if normalize { v / 255.0 } else { v }
            }
            GL_BYTE => {
                let v = b[i] as i8 as f32;
                if normalize { v / 127.0 } else { v }
            }
            GL_UNSIGNED_SHORT => {
                let v = u16::from_le_bytes([b[2 * i], b[2 * i + 1]]) as f32;
                if normalize { v / 65535.0 } else { v }
            }
            GL_SHORT => {
                let v = i16::from_le_bytes([b[2 * i], b[2 * i + 1]]) as f32;
                if normalize { v / 32767.0 } else { v }
            }
            GL_UNSIGNED_INT => {
                let v = u32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
                if normalize {
                    v as f32 / u32::MAX as f32
                } else {
                    v as f32
                }
            }
            _ => {
                let v = i32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
                if normalize {
                    v as f32 / i32::MAX as f32
                } else {
                    v as f32
                }
            }
        };
    }
    out
}

/// A normal of an array as table 2.6 converts it: signed integers to
/// (2c + 1) / (2^b - 1).
fn normal_values(b: &[u8], kind: u32) -> [f32; 3] {
    let v = values(b, kind, 3, false);
    let bits = match kind {
        GL_BYTE => 8,
        GL_SHORT => 16,
        GL_INT => 32,
        _ => return [v[0], v[1], v[2]],
    };
    [v[0], v[1], v[2]].map(|x| signed_unit(x as f64, bits))
}

/// The vertices `first` to `last` of the current context's arrays,
/// with the current color, normal and texture coordinates where an
/// array is off, lit with GL_LIGHTING on; and their back colors with
/// two-sided lighting. The texture coordinates go through the texture
/// matrices when `transform` is set; glArrayElement's don't, as glEnd
/// puts them through. With `used`, only the vertices it marks (counted
/// from `first`) are lit: the others are drawn by no primitive. None
/// without a vertex array.
fn gather(
    cpu: &mut Cpu,
    first: u32,
    last: u32,
    transform: bool,
    used: Option<&[bool]>,
) -> Option<(Vec<Vertex>, Option<Vec<u32>>)> {
    let c = ctx(cpu)?;
    let (va, ca, na, ta) = (c.vertex_array, c.color_array, c.normal_array, c.tex_array);
    let lighting = c.caps & CAP_LIGHTING != 0;
    if !va.enabled || last < first {
        return None;
    }
    let (vb, vs) = array_bytes(cpu, &va, first, last)?;
    let colors = if ca.enabled {
        array_bytes(cpu, &ca, first, last)
    } else {
        None
    };
    // Normals matter only to lighting.
    let normals = if na.enabled && lighting {
        array_bytes(cpu, &na, first, last)
    } else {
        None
    };
    let texs: [Option<(Vec<u8>, usize)>; UNITS] = std::array::from_fn(|u| {
        if ta[u].enabled {
            array_bytes(cpu, &ta[u], first, last)
        } else {
            None
        }
    });
    let c = ctx(cpu)?;
    let (color, normal, tex) = (c.color, c.normal, c.tex_coord);
    let matrices = if transform {
        texture_matrices(c)
    } else {
        [None; UNITS]
    };
    let lit = Lit::of(c);
    let n = (last - first + 1) as usize;
    let mut out = Vec::with_capacity(n);
    let mut backs = two_sided(c).then(|| Vec::with_capacity(n));
    for i in 0..n {
        let mut p = values(&vb[i * vs..], va.kind, va.size, false);
        if va.size < 4 {
            p[3] = 1.0;
        }
        if va.size < 3 {
            p[2] = 0.0;
        }
        let rgba = match &colors {
            Some((b, s)) => {
                let mut v = values(&b[i * s..], ca.kind, ca.size, true);
                if ca.size < 4 {
                    v[3] = 1.0;
                }
                v
            }
            None => color,
        };
        let normal = match &normals {
            Some((b, s)) => normal_values(&b[i * s..], na.kind),
            None => normal,
        };
        let lit = lit.as_ref().filter(|_| used.is_none_or(|u| u[i]));
        let [diffuse, back] = vertex_colors(lit, p, normal, rgba);
        if let Some(backs) = &mut backs {
            backs.push(back);
        }
        let mut v = Vertex {
            position: homogeneous(p),
            diffuse: Some(diffuse),
            ..Vertex::default()
        };
        for u in 0..UNITS {
            let t = match &texs[u] {
                Some((b, s)) => {
                    let mut t = values(&b[i * s..], ta[u].kind, ta[u].size, false);
                    t[3] = if ta[u].size == 4 { t[3] } else { 1.0 };
                    t
                }
                None => tex[u],
            };
            v.tex[u] = texture_coords(&matrices[u], t);
        }
        out.push(v);
    }
    Some((out, backs))
}

/// glArrayElement(i): a vertex of the arrays, between glBegin and
/// glEnd.
pub fn array_element(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let i = args[0];
    let Some((vertices, backs)) = gather(cpu, i, i, false, None) else {
        return Some(0);
    };
    let Some(&v) = vertices.first() else {
        return Some(0);
    };
    let color = v.diffuse.unwrap_or(0xFFFF_FFFF);
    let back = backs.and_then(|b| b.first().copied()).unwrap_or(color);
    let Some(c) = ctx(cpu) else {
        return Some(0);
    };
    if c.begin.is_some() {
        c.vertices.push(ImmVertex {
            position: v.position,
            color,
            back,
            tex: [v.tex[0], v.tex[1]],
        });
    }
    Some(0)
}

/// glDrawArrays(mode, first, count).
pub fn draw_arrays(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [kind, first, count] = [args[0], args[1], args[2]];
    if !valid_kind(kind) {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    if (count as i32) < 0 {
        error(cpu, GL_INVALID_VALUE);
        return Some(0);
    }
    if count == 0 || ctx(cpu).is_some_and(|c| c.begin.is_some()) {
        return Some(0);
    }
    // A range past the end of the address space is a program's error.
    let Some(last) = first.checked_add(count - 1) else {
        return Some(0);
    };
    if let Some((v, backs)) = gather(cpu, first, last, true, None) {
        draw(cpu, kind, &v, backs.as_deref(), None);
    }
    Some(0)
}

/// glDrawElements(mode, count, type, indices).
pub fn draw_elements(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [kind, count, index_kind, at] = [args[0], args[1], args[2], args[3]];
    if !valid_kind(kind) {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    if (count as i32) < 0 {
        error(cpu, GL_INVALID_VALUE);
        return Some(0);
    }
    let size = match index_kind {
        GL_UNSIGNED_BYTE => 1,
        GL_UNSIGNED_SHORT => 2,
        GL_UNSIGNED_INT => 4,
        _ => {
            error(cpu, GL_INVALID_ENUM);
            return Some(0);
        }
    };
    if count == 0 || ctx(cpu).is_some_and(|c| c.begin.is_some()) {
        return Some(0);
    }
    let len = count as usize * size;
    if at == 0 || !cpu.bus.guest_probe(at, len, false) {
        return Some(0);
    }
    let mut bytes = vec![0u8; len];
    cpu.bus.guest_read_bytes(at, &mut bytes);
    let indices: Vec<u32> = match size {
        1 => bytes.iter().map(|&b| b as u32).collect(),
        2 => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| u16::from_le_bytes(b) as u32)
            .collect(),
        _ => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| u32::from_le_bytes(b))
            .collect(),
    };
    let first = indices.iter().copied().min().unwrap_or(0);
    let last = indices.iter().copied().max().unwrap_or(0);
    // A range far wider than the indices is a program's error.
    if last - first > 0x10_0000 {
        return Some(0);
    }
    let mut used = vec![false; (last - first + 1) as usize];
    for &i in &indices {
        used[(i - first) as usize] = true;
    }
    let Some((v, backs)) = gather(cpu, first, last, true, Some(&used)) else {
        return Some(0);
    };
    let order: Vec<u32> = indices.iter().map(|i| i - first).collect();
    draw(cpu, kind, &v, backs.as_deref(), Some(&order));
    Some(0)
}

/// glLockArraysEXT(first, count) and glUnlockArraysEXT(): the arrays
/// are read as they are drawn either way.
pub fn lock_arrays(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// glRectf(x1, y1, x2, y2) and the others, as floats.
fn rect(cpu: &mut Cpu, r: [f32; 4]) {
    let Some(c) = ctx(cpu) else {
        return;
    };
    if c.begin.is_some() {
        error(cpu, GL_INVALID_OPERATION);
        return;
    }
    let matrices = texture_matrices(c);
    let tex: [[f32; 4]; UNITS] =
        std::array::from_fn(|u| texture_coords(&matrices[u], c.tex_coord[u]));
    // A rectangle is a polygon of four vertices, lit as glVertex's are.
    let lit = Lit::of(c);
    let corners = [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]];
    let mut backs = Vec::with_capacity(4);
    let vertices = corners.map(|[x, y]| {
        let position = [x, y, 0.0, 1.0];
        let [color, back] = vertex_colors(lit.as_ref(), position, c.normal, c.color);
        backs.push(back);
        let mut v = Vertex {
            position,
            diffuse: Some(color),
            ..Vertex::default()
        };
        v.tex[..UNITS].copy_from_slice(&tex);
        v
    });
    let sided = two_sided(c);
    draw(
        cpu,
        GL_POLYGON,
        &vertices,
        sided.then_some(&backs[..]),
        None,
    );
}

/// glRectf(x1, y1, x2, y2).
pub fn rectf(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    rect(cpu, std::array::from_fn(|i| super::f(args, i)));
    Some(0)
}

/// glRectd(x1, y1, x2, y2).
pub fn rectd(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    rect(cpu, std::array::from_fn(|i| super::d(args, 2 * i) as f32));
    Some(0)
}

/// glRecti(x1, y1, x2, y2).
pub fn recti(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    rect(cpu, std::array::from_fn(|i| args[i] as i32 as f32));
    Some(0)
}

/// glRects(x1, y1, x2, y2): shorts, whose words' top halves the caller
/// may leave as they were.
pub fn rects(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    rect(cpu, std::array::from_fn(|i| args[i] as u16 as i16 as f32));
    Some(0)
}

/// glRectfv(v1, v2).
pub fn rectfv(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let (Some(a), Some(b)) = (
        super::read_floats(cpu, args[0], 2),
        super::read_floats(cpu, args[1], 2),
    ) {
        rect(cpu, [a[0], a[1], b[0], b[1]]);
    }
    Some(0)
}

// Clearing.

/// Clear the context at `index`'s color buffer, depth buffer, or both,
/// in the scissor box if it is on and `whole` isn't asked for.
fn clear_in(cpu: &mut Cpu, index: usize, color: bool, depth: bool, whole: bool) {
    let Some(Some(c)) = state(cpu).contexts.get(index) else {
        return;
    };
    if c.target == 0 {
        return;
    }
    let rects = if !whole && c.caps & CAP_SCISSOR_TEST != 0 {
        let [l, t, r, b] = scissor_rect(c);
        vec![[l as i32, t as i32, r as i32, b as i32]]
    } else {
        Vec::new()
    };
    let fill_color = (color && c.color_mask[..3].iter().any(|&m| m)).then(|| argb(c.clear_color));
    let fill_depth = (depth && c.depth_mask).then_some(c.clear_depth.clamp(0.0, 1.0));
    let inner = c.inner;
    if !rects.is_empty() && rects[0][0] >= rects[0][2] {
        return;
    }
    let Some(d) = inner_device::get(cpu, inner).cloned() else {
        return;
    };
    let fill = hal::Fill {
        target: fill_color,
        depth: fill_depth,
        stencil: None,
    };
    hal::clear(cpu, &d, &rects, fill);
}

/// Clear the whole of the context at `index`'s buffers, as new ones
/// are.
pub fn clear_buffers(cpu: &mut Cpu, index: usize, color: bool, depth: bool) {
    clear_in(cpu, index, color, depth, true);
}

/// glClear(mask).
pub fn clear(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mask = args[0];
    if mask & !(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT | GL_STENCIL_BUFFER_BIT | 0x200) != 0 {
        error(cpu, GL_INVALID_VALUE);
        return Some(0);
    }
    let Some((index, _)) = current(cpu) else {
        return Some(0);
    };
    if ctx(cpu).is_some_and(|c| c.begin.is_some()) {
        error(cpu, GL_INVALID_OPERATION);
        return Some(0);
    }
    clear_in(
        cpu,
        index,
        mask & GL_COLOR_BUFFER_BIT != 0,
        mask & GL_DEPTH_BUFFER_BIT != 0,
        false,
    );
    if let Some(c) = ctx(cpu) {
        c.drawn = true;
    }
    Some(0)
}

// Finishing.

/// glFlush() and glFinish(): a single-buffered context's drawing is
/// shown.
pub fn flush(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let Some((index, _)) = current(cpu) else {
        return Some(0);
    };
    let show = state(cpu).contexts[index]
        .as_ref()
        .is_some_and(|c| c.drawn && !super::wgl::double_buffered(c.format));
    if show {
        super::wgl::present(cpu, index);
    }
    Some(0)
}

/// glReadPixels(x, y, width, height, format, type, pixels).
pub fn read_pixels(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [x, y, width, height, format, kind, at] = [
        args[0], args[1], args[2], args[3], args[4], args[5], args[6],
    ];
    let (w, h) = (width as i32, height as i32);
    if w < 0 || h < 0 {
        error(cpu, GL_INVALID_VALUE);
        return Some(0);
    }
    let components = match format {
        GL_RGB | GL_BGR_EXT => 3,
        GL_RGBA | GL_BGRA_EXT => 4,
        GL_DEPTH_COMPONENT => {
            // No depth is read back: the program's buffer is left as it
            // is.
            return Some(0);
        }
        _ => {
            error(cpu, GL_INVALID_ENUM);
            return Some(0);
        }
    };
    if kind != GL_UNSIGNED_BYTE {
        error(cpu, GL_INVALID_ENUM);
        return Some(0);
    }
    if w == 0 || h == 0 || at == 0 {
        return Some(0);
    }
    let pack = ctx(cpu).map_or([4, 0, 0, 0], |c| c.pack);
    let row_pixels = if pack[1] > 0 { pack[1] } else { width };
    // The program's buffer first: sizes it can't hold aren't read.
    let place = || {
        let stride = row_pixels
            .checked_mul(components)?
            .checked_next_multiple_of(pack[0].max(1))?;
        let skip = pack[2]
            .checked_mul(stride)?
            .checked_add(pack[3].checked_mul(components)?)?;
        let len = stride
            .checked_mul(height - 1)?
            .checked_add(width.checked_mul(components)?)?;
        Some((stride, at.checked_add(skip)?, len))
    };
    let Some((stride, start, len)) = place() else {
        return Some(0);
    };
    if !cpu.bus.guest_probe(start, len as usize, true) {
        return Some(0);
    }
    let Some(pixels) = texture::read_frame(cpu, x as i32, y as i32, width, height) else {
        return Some(0);
    };
    let mut row = Vec::with_capacity((width * components) as usize);
    for r in 0..height {
        row.clear();
        for p in &pixels[(r * width) as usize..((r + 1) * width) as usize] {
            let [red, g, b, a] = *p;
            match format {
                GL_RGB => row.extend_from_slice(&[red, g, b]),
                GL_BGR_EXT => row.extend_from_slice(&[b, g, red]),
                GL_BGRA_EXT => row.extend_from_slice(&[b, g, red, a]),
                _ => row.extend_from_slice(&[red, g, b, a]),
            }
        }
        cpu.bus.guest_write_bytes(start + r * stride, &row);
    }
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_functions_map_in_order() {
        // GL_NEVER is D3DCMP_NEVER, GL_LEQUAL D3DCMP_LESSEQUAL.
        assert_eq!(compare(0x0200), 1);
        assert_eq!(compare(0x0203), 4);
        assert_eq!(compare(0x0207), 8);
    }

    #[test]
    fn a_viewport_past_the_left_edge_keeps_its_place() {
        let mut c = super::super::new_context(0, 0, 1);
        c.width = 640;
        c.height = 480;
        c.viewport = [-100, 0, 700, 480];
        let (v, fit) = super::super::wgl::inner_viewport(&c).expect("on the buffers");
        assert_eq!((v.x, v.width), (0, 600));
        // OpenGL's left edge, x -1, is 100 pixels left of the buffers.
        let x = matrix::apply(&fit, [-1.0, 0.0, 0.0, 1.0])[0];
        let pixel = (x + 1.0) / 2.0 * v.width as f32 + v.x as f32;
        assert!((pixel + 100.0).abs() < 1e-3);
        // One inside the buffers is drawn as it is.
        c.viewport = [10, 20, 300, 200];
        let (v, fit) = super::super::wgl::inner_viewport(&c).expect("on the buffers");
        assert_eq!((v.x, v.y, v.width, v.height), (10, 260, 300, 200));
        assert_eq!(fit, IDENTITY);
    }

    /// The vertex each primitive takes its flat color from.
    fn provoking(kind: u32, n: u32) -> Vec<u32> {
        let order: Vec<u32> = (0..n).collect();
        primitives(kind, &order)
            .iter()
            .map(|p| match *p {
                Prim::Point(i) => i,
                Prim::Line(_, _, p) | Prim::Triangle(_, p, _) => p,
            })
            .collect()
    }

    #[test]
    fn flat_colors_come_from_the_vertices_opengl_gives() {
        // Table 2.6 of the OpenGL 1.1 specification, counting from 0.
        assert_eq!(provoking(GL_POINTS, 3), [0, 1, 2]);
        assert_eq!(provoking(GL_LINES, 5), [1, 3]);
        assert_eq!(provoking(GL_LINE_STRIP, 4), [1, 2, 3]);
        assert_eq!(provoking(GL_LINE_LOOP, 3), [1, 2, 0]);
        assert_eq!(provoking(GL_TRIANGLES, 7), [2, 5]);
        assert_eq!(provoking(GL_TRIANGLE_STRIP, 5), [2, 3, 4]);
        assert_eq!(provoking(GL_TRIANGLE_FAN, 5), [2, 3, 4]);
        assert_eq!(provoking(GL_QUADS, 9), [3, 3, 7, 7]);
        assert_eq!(provoking(GL_QUAD_STRIP, 7), [3, 3, 5, 5]);
        assert_eq!(provoking(GL_POLYGON, 5), [0, 0, 0]);
    }

    #[test]
    fn primitives_keep_their_winding() {
        assert_eq!(
            primitives(GL_TRIANGLE_STRIP, &[0, 1, 2, 3]),
            [
                Prim::Triangle([0, 1, 2], 2, 7),
                Prim::Triangle([2, 1, 3], 3, 7),
            ]
        );
        assert_eq!(
            primitives(GL_QUADS, &[0, 1, 2, 3]),
            [
                Prim::Triangle([0, 1, 2], 3, 3),
                Prim::Triangle([0, 2, 3], 3, 6),
            ]
        );
        // Too few vertices make nothing, a loop of one no line.
        assert!(primitives(GL_LINE_LOOP, &[0]).is_empty());
        assert!(primitives(GL_TRIANGLE_FAN, &[0, 1]).is_empty());
    }

    #[test]
    fn wireframe_leaves_out_the_diagonals_of_quads_and_polygons() {
        let edges = |kind: u32, n: u32| -> Vec<u8> {
            let order: Vec<u32> = (0..n).collect();
            primitives(kind, &order)
                .iter()
                .map(|p| match *p {
                    Prim::Triangle(_, _, e) => e,
                    _ => 0,
                })
                .collect()
        };
        assert_eq!(edges(GL_QUADS, 4), [3, 6]);
        assert_eq!(edges(GL_QUAD_STRIP, 6), [5, 6, 5, 6]);
        assert_eq!(edges(GL_POLYGON, 3), [7]);
        assert_eq!(edges(GL_POLYGON, 5), [3, 2, 6]);
        // A strip's and a fan's triangles are each drawn whole.
        assert_eq!(edges(GL_TRIANGLE_STRIP, 4), [7, 7]);
        assert_eq!(edges(GL_TRIANGLE_FAN, 4), [7, 7]);
    }

    #[test]
    fn a_polygon_shows_one_side_whatever_its_triangles_do() {
        let c = super::super::new_context(0, 0, 1);
        // A concave quad, counterclockwise as a whole, whose second
        // triangle (0, 2, 3) is clockwise.
        let corners = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.5, 0.4]];
        let vertices: Vec<Vertex> = corners
            .iter()
            .map(|&[x, y]| Vertex {
                position: [x, y, 0.0, 1.0],
                ..Vertex::default()
            })
            .collect();
        let backs = [0; 4];
        let showing = |kind: u32| {
            let mut prims = primitives(kind, &[0, 1, 2, 3]);
            back_faces(&c, &vertices, &backs, &mut prims, polygon_triangles(kind));
            prims
                .iter()
                .map(|p| match *p {
                    Prim::Triangle(t, ..) => t[0] >= 4,
                    _ => false,
                })
                .collect::<Vec<_>>()
        };
        // Section 2.13.1: the quad's area, positive, shows the front of
        // both its triangles.
        assert_eq!(showing(GL_QUADS), [false, false]);
        assert_eq!(showing(GL_POLYGON), [false, false]);
        // As separate triangles each shows its own side.
        assert_eq!(showing(GL_TRIANGLE_FAN), [false, true]);
    }

    #[test]
    fn depth_goes_from_minus_one_to_zero() {
        let near = matrix::apply(&DEPTH_HALF, [0.0, 0.0, -1.0, 1.0]);
        let far = matrix::apply(&DEPTH_HALF, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(near[2], 0.0);
        assert_eq!(far[2], 1.0);
    }
}
