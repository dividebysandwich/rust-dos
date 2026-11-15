//! A draw's steps printed as a GLSL fragment shader ("One pipeline
//! description, two back ends" in `docs/toadforce.md`).
//!
//! The shader keeps the registers of `span.rs` as vectors of four
//! integers, 0 to 255, in the order red, green, blue, alpha, and each
//! step is the formula `combine.rs` has for it in the same whole
//! numbers. What isn't in whole numbers there isn't here either: the
//! colours' and coordinates' interpolation, the bump offsets and the
//! fog factor.
//!
//! A sample of a texture is `sample.rs`'s too (`SAMPLING`): the level
//! of detail from how the coordinates change over a pixel, the levels
//! and the filter it picks, the coordinates in 1/256 of a texel, the
//! address modes, and the four texels blended by weights of 8 bits in
//! whole numbers, each read with `texelFetch`. The host's own filter
//! takes none of those steps as the card does: it rounds a weight
//! where the card cuts it off, magnifies up to another level of
//! detail, and picks the texel of a coordinate on a texel's edge by
//! its own rule. Two samples are still the host's own (`Key::native`):
//! a cube map's, and an anisotropic one where the host has that
//! filter, which reads more texels than the card's stand-in for it.
//!
//! The depth and stencil tests, the blend and the write masks are the
//! host's fixed state, set for each run (`state.rs`), so the steps that
//! name them print nothing and two draws that differ in them alone
//! share a program.

use crate::toadforce::level1::states::STAGES;
use crate::toadforce::raster::combine::{DITHER, Op};
use crate::toadforce::raster::pipeline::{Arg, DrawState, Fog, Reg, SLOTS, Step};
use crate::toadforce::raster::sample::Tex;
use crate::voodoo::gl::Locations;
use glow::HasContext;
use std::fmt::Write;

/// The texture unit of stage `stage`'s texture, and of the texture
/// that marks its colour-keyed texels.
pub fn unit(stage: usize) -> u32 {
    stage as u32
}

pub fn key_unit(stage: usize) -> u32 {
    (STAGES + stage) as u32
}

/// The vertex attributes, at these locations: position (x and y in
/// pixels of the target with the centre of a pixel at its half, depth,
/// 1 / w), the two colours as the card has them (blue, green, red,
/// alpha, each 0 to 255), and a coordinate set for each slot.
pub const ATTRIBUTES: [(u32, &str); 3 + SLOTS] = [
    (0, "a_pos"),
    (1, "a_diffuse"),
    (2, "a_specular"),
    (3, "a_tex0"),
    (4, "a_tex1"),
    (5, "a_tex2"),
    (6, "a_tex3"),
];

/// The attributes of a program that transforms: those, and the
/// normal.
pub const ATTRIBUTES_TNL: [(u32, &str); 4 + SLOTS] = [
    (0, "a_pos"),
    (1, "a_diffuse"),
    (2, "a_specular"),
    (3, "a_tex0"),
    (4, "a_tex1"),
    (5, "a_tex2"),
    (6, "a_tex3"),
    (7, "a_normal"),
];

/// The vertex shader of every program. Dividing the position by 1 / w
/// has the host interpolate the colours and the coordinates with
/// perspective correction, as `setup.rs` does; depth goes linearly
/// over the screen there, which the host's own depth does.
///
/// The depth bias is taken from the depth here, as a part of the
/// buffer's range, and the host rounds the result to the buffer's
/// steps: the steps `span.rs` takes from the rounded depth. The host's
/// own polygon offset counts in a unit of the host's choosing, which
/// isn't a step of a 16-bit buffer on every host.
pub const VERTEX: &str = "
uniform vec2 u_scale;
uniform float u_bias;
in vec4 a_pos;
in vec4 a_diffuse;
in vec4 a_specular;
in vec4 a_tex0;
in vec4 a_tex1;
in vec4 a_tex2;
in vec4 a_tex3;
out vec4 v_diffuse;
out vec4 v_specular;
out vec4 v_tex[4];
noperspective out vec4 v_stw[4];
noperspective out float v_q;
noperspective out float v_z;
void main() {
    float w = 1.0 / a_pos.w;
    vec2 at = a_pos.xy * u_scale - 1.0;
    gl_Position = vec4(at * w, ((a_pos.z - u_bias) * 2.0 - 1.0) * w, w);
    v_diffuse = a_diffuse.zyxw;
    v_specular = a_specular.zyxw;
    v_tex[0] = a_tex0;
    v_tex[1] = a_tex1;
    v_tex[2] = a_tex2;
    v_tex[3] = a_tex3;
    // The coordinates times 1 / w and 1 / w itself go linearly over
    // the screen, as `setup.rs` has them (`Prim::tex` and `Prim::q`).
    v_stw[0] = a_tex0 * a_pos.w;
    v_stw[1] = a_tex1 * a_pos.w;
    v_stw[2] = a_tex2 * a_pos.w;
    v_stw[3] = a_tex3 * a_pos.w;
    v_q = a_pos.w;
    v_z = a_pos.z;
}
";

/// The vertex shader of a program that transforms: a vertex as the
/// program gave it, through `u_all` to the clip coordinates that put
/// it on the screen where `geometry::Setup::project` does. The host
/// cuts the triangles, at the planes of its own for the eye and the
/// target's edges, and at `gl_ClipDistance` for the near and the far
/// plane, and rounds the positions of what is left to its own grid,
/// as the card cuts first and rounds what is left. A position rounded
/// here would move a triangle that crosses the eye's plane: the
/// nearer a pixel of it is than the vertex, the further.
/// What it interpolates is then right on the pieces only when it is
/// interpolated with perspective correction, so the values that go
/// linearly over the screen aren't outputs here: `fragment_tnl` makes
/// them from `gl_FragCoord.w`, which is 1 / w.
///
/// `u_all` is the card's matrix row by row, which OpenGL reads as its
/// transpose: the product with a column is the card's row vector
/// times its matrix. `u_x`, `u_y` and `u_z` are `Transform`'s, with
/// the far plane's flag after `u_z`'s two, and `u_shift` is what goes
/// from a position of the card to the host's pixels. `u_eye` takes a
/// vertex to camera space for the fog of `u_vfog` (start, end,
/// density, and the mode, 0 for none, plus 4 for fog by the distance
/// from the eye), which is rounded to a byte as the card's is.
///
/// With `u_lights` of 0 and above the colours are `raster::light`'s
/// for the front of a triangle, from that many lights: `u_lpos` (the
/// position, and the kind), `u_laxis` (and the range), `u_ldiffuse`
/// (and the falloff), `u_lspecular` (and the spot's formula),
/// `u_lambient`, `u_latt` (the attenuation) and `u_lcone` (the
/// cosines of the inner and the outer half angle); the material's
/// colours in `u_mat` (diffuse, specular, ambient, emissive) from the
/// sources of `u_lsrc`; the ambient light and the material's power in
/// `u_lmisc`; and in `u_lflags` whether there is specular light,
/// whether it goes into the diffuse colour, whether the eye is where
/// it is and not infinitely far, and whether normals are made of
/// length 1 after `u_normals` took them to camera space. The powers
/// are the host's `pow`, where the card has its own series.
pub const VERTEX_TNL: &str = "
uniform vec2 u_scale;
uniform float u_bias;
uniform mat4 u_all;
uniform vec2 u_x;
uniform vec2 u_y;
uniform vec3 u_z;
uniform float u_shift;
uniform mat4 u_eye;
uniform vec4 u_vfog;
uniform int u_lights;
uniform vec4 u_lpos[8];
uniform vec4 u_laxis[8];
uniform vec4 u_ldiffuse[8];
uniform vec4 u_lspecular[8];
uniform vec4 u_lambient[8];
uniform vec4 u_latt[8];
uniform vec4 u_lcone[8];
uniform vec4 u_mat[4];
uniform vec4 u_lmisc;
uniform ivec4 u_lsrc;
uniform ivec4 u_lflags;
uniform mat3 u_normals;
in vec4 a_pos;
in vec4 a_diffuse;
in vec4 a_specular;
in vec4 a_tex0;
in vec4 a_tex1;
in vec4 a_tex2;
in vec4 a_tex3;
in vec3 a_normal;
out vec4 v_diffuse;
out vec4 v_specular;
out vec4 v_tex[4];
out float v_zw;
invariant gl_Position;
// `math::pow_unit`.
float pow_unit(float x, float y) {
    if (!(y > 0.0)) {
        return 1.0;
    }
    if (!(x > 0.0)) {
        return 0.0;
    }
    return x >= 1.0 ? 1.0 : pow(x, y);
}
// `geometry::normalize`.
vec3 unit(vec3 a) {
    float len = length(a);
    return len > 0.0 && !isinf(len) ? a / len : a;
}
vec4 source(int from, vec4 material, vec4 diffuse, vec4 specular) {
    return from == 1 ? diffuse : from == 2 ? specular : material;
}
// `Lighting::light` for the vertex at `eye` with the normal `n`.
void light(vec3 eye, vec3 n, vec4 own_diffuse, vec4 own_specular) {
    vec4 m_diffuse = source(u_lsrc.x, u_mat[0], own_diffuse, own_specular);
    vec4 m_specular = source(u_lsrc.y, u_mat[1], own_diffuse, own_specular);
    vec4 m_ambient = source(u_lsrc.z, u_mat[2], own_diffuse, own_specular);
    vec4 m_emissive = source(u_lsrc.w, u_mat[3], own_diffuse, own_specular);
    vec3 ambient = u_lmisc.xyz;
    vec3 lit = vec3(0.0);
    vec3 shine = vec3(0.0);
    vec3 to_eye = u_lflags.z != 0 ? unit(-eye) : vec3(0.0, 0.0, -1.0);
    for (int i = 0; i < u_lights; i++) {
        float kind = u_lpos[i].w;
        vec3 axis = u_laxis[i].xyz;
        vec3 to = -axis;
        float reach = 1.0;
        if (kind != 3.0) {
            to = u_lpos[i].xyz - eye;
            float d = length(to);
            if (d > u_laxis[i].w) {
                continue;
            }
            vec3 a = u_latt[i].xyz;
            float under = a.x + a.y * d + a.z * d * d;
            to = unit(to);
            reach = under > 0.0 ? 1.0 / under : 1.0;
        }
        float cone = 1.0;
        if (kind == 2.0) {
            float rho = -dot(to, axis);
            float inner = u_lcone[i].x;
            float outer = u_lcone[i].y;
            float falloff = u_ldiffuse[i].w;
            if (rho <= outer) {
                cone = 0.0;
            } else if (u_lspecular[i].w == 1.0) {
                cone = pow_unit(rho, falloff);
            } else if (rho > inner || inner <= outer) {
                cone = 1.0;
            } else {
                cone = pow_unit((rho - outer) / (inner - outer), falloff);
            }
        }
        float factor = reach * cone;
        if (!(factor > 0.0) || isinf(factor)) {
            continue;
        }
        ambient += u_lambient[i].xyz * factor;
        float facing = dot(n, to);
        if (!(facing > 0.0)) {
            continue;
        }
        lit += u_ldiffuse[i].xyz * facing * factor;
        if (u_lflags.x != 0) {
            float glint = dot(n, unit(to + to_eye));
            if (glint > 0.0) {
                shine += u_lspecular[i].xyz * pow_unit(glint, u_lmisc.w) * factor;
            }
        }
    }
    vec3 d = m_emissive.rgb + m_ambient.rgb * ambient + m_diffuse.rgb * lit;
    vec3 s = vec3(0.0);
    if (u_lflags.y != 0) {
        d += m_specular.rgb * shine;
    } else {
        s = m_specular.rgb * shine;
    }
    // Bytes, as the card rounds a vertex's colours.
    v_diffuse = floor(clamp(vec4(d, m_diffuse.a), 0.0, 1.0) * 255.0 + 0.5);
    v_specular = floor(clamp(vec4(s, 1.0), 0.0, 1.0) * 255.0 + 0.5);
}
void main() {
    vec4 c = u_all * vec4(a_pos.xyz, 1.0);
    float w = c.w;
    // The host's x and y times w, of the screen's
    // x[0] + (c.x / w + 1) * x[1] and y[0] + (1 - c.y / w) * y[1].
    vec2 at = vec2(
        ((u_x.x + u_x.y + u_shift) * u_scale.x - 1.0) * w + c.x * u_x.y * u_scale.x,
        ((u_y.x + u_y.y + u_shift) * u_scale.y - 1.0) * w - c.y * u_y.y * u_scale.y);
    // Depth is z[0] + c.z / w * z[1], less the bias.
    gl_Position = vec4(at, ((u_z.x - u_bias) * 2.0 - 1.0) * w + 2.0 * u_z.y * c.z, w);
    gl_ClipDistance[0] = c.z;
    gl_ClipDistance[1] = u_z.z != 0.0 ? w - c.z : 1.0;
    v_diffuse = a_diffuse.zyxw;
    v_specular = a_specular.zyxw;
    vec3 e = vec3(0.0);
    if (u_lights >= 0 || u_vfog.w != 0.0) {
        e = (u_eye * vec4(a_pos.xyz, 1.0)).xyz;
    }
    if (u_lights >= 0) {
        vec3 n = u_normals * a_normal;
        if (u_lflags.w != 0) {
            n = unit(n);
        }
        light(e, n, v_diffuse / 255.0, v_specular / 255.0);
    }
    if (u_vfog.w != 0.0) {
        float d = u_vfog.w >= 4.0 ? length(e) : abs(e.z);
        float mode = u_vfog.w >= 4.0 ? u_vfog.w - 4.0 : u_vfog.w;
        float f = mode == 1.0 ? exp(-u_vfog.z * d)
            : mode == 2.0 ? exp(-(u_vfog.z * d) * (u_vfog.z * d))
            : (u_vfog.y - d) / (u_vfog.y - u_vfog.x);
        // No number, as at the end of a fog that starts where it
        // ends, is no fog.
        f = isnan(f) ? 1.0 : clamp(f, 0.0, 1.0);
        v_specular.a = floor(f * 255.0 + 0.5);
    }
    v_tex[0] = a_tex0;
    v_tex[1] = a_tex1;
    v_tex[2] = a_tex2;
    v_tex[3] = a_tex3;
    v_zw = u_z.x * w + u_z.y * c.z;
}
";

/// The inputs of `fragment` that go linearly over the screen, and
/// what `fragment_tnl` has in their place.
const LINEAR: &str = "noperspective in vec4 v_stw[4];
noperspective in float v_q;
noperspective in float v_z;
";
const LINEAR_TNL: &str = "in float v_zw;
";
const MAIN: &str = "void main() {
";
const MAIN_TNL: &str = "void main() {
    float v_q = gl_FragCoord.w;
    vec4 v_stw[4];
    for (int i = 0; i < 4; i++) {
        v_stw[i] = v_tex[i] * v_q;
    }
    float v_z = v_zw * v_q;
";

/// The fragment shader of `key` for `VERTEX_TNL`: `fragment`'s, with
/// 1 / w from the host's own, and the coordinates times it and the
/// depth from what the host interpolates with perspective correction
/// times it.
pub fn fragment_tnl(key: &Key) -> String {
    fragment(key)
        .replacen(LINEAR, LINEAR_TNL, 1)
        .replacen(MAIN, MAIN_TNL, 1)
}

/// `sample.rs` in GLSL, for the samples the shader takes itself. A
/// stage's numbers come in its uniforms: `ts` is the size of level 0,
/// `MAXMIPLEVEL` and the last level; `tm` the magnification and the
/// minification filter (1 linear), the mip filter (0 none, 1 the
/// nearest level, 2 two levels blended) and whether the level of
/// detail is needed; `ta` the address modes; `tl` the level of
/// detail's bias and the square of the anisotropy; `tb` the border.
const SAMPLING: &str = "
// `Address::texel`: -1 for the border.
int tf_address(int at, int size, int mode) {
    if (mode == 0) return at & (size - 1);
    if (mode == 1) {
        at = at & (2 * size - 1);
        return at < size ? at : 2 * size - 1 - at;
    }
    if (mode == 2) return clamp(at, 0, size - 1);
    return at >= 0 && at < size ? at : -1;
}
// `Sampler::fixed`, without the shift, which is a whole number the
// caller takes off: a coordinate in 1/256 of a texel, rounded down.
// A power of two times the coordinate is exact. A texture drawn a
// texel to a pixel from whole coordinates has every pixel on the edge
// of two texels, where the card's planes give the edge itself and the
// host's interpolation a value a little to either side of it. A
// sixteenth of a step is added so that such a value is on the card's
// side of the edge.
int tf_fixed(float c, int size) {
    float at = c * float(size) * 256.0 + 0.0625;
    return int(floor(clamp(at, -2147483648.0, 2147483520.0)));
}
ivec4 tf_texel(sampler2D s, int level, int x, int y, ivec4 border) {
    if (x < 0 || y < 0) return border;
    return ivec4(texelFetch(s, ivec2(x, y), level) * 255.0 + 0.5);
}
// `lerp`.
ivec4 tf_lerp(ivec4 a, ivec4 b, int f) {
    return (a * (256 - f) + b * f + 128) >> 8;
}
// `Sampler::level`.
ivec4 tf_level(sampler2D s, int level, vec2 at, ivec2 size0, bool linear, ivec2 mode,
               ivec4 border) {
    ivec2 size = max(size0 >> level, ivec2(1));
    ivec2 p = ivec2(tf_fixed(at.x, size.x), tf_fixed(at.y, size.y));
    if (!linear) {
        int x = tf_address(p.x >> 8, size.x, mode.x);
        int y = tf_address(p.y >> 8, size.y, mode.y);
        return tf_texel(s, level, x, y, border);
    }
    // A texel's centre is half a texel inside it.
    p -= 128;
    ivec2 f = p & 255;
    p = p >> 8;
    int x0 = tf_address(p.x, size.x, mode.x);
    int x1 = tf_address(p.x + 1, size.x, mode.x);
    int y0 = tf_address(p.y, size.y, mode.y);
    int y1 = tf_address(p.y + 1, size.y, mode.y);
    ivec4 a = tf_texel(s, level, x0, y0, border);
    ivec4 b = tf_texel(s, level, x1, y0, border);
    ivec4 c = tf_texel(s, level, x0, y1, border);
    ivec4 d = tf_texel(s, level, x1, y1, border);
    return tf_lerp(tf_lerp(a, b, f.x), tf_lerp(c, d, f.x), f.y);
}
// The level of detail of `span.rs`: half the logarithm of the square
// of the texels a pixel covers along its longer side, or with
// anisotropy along the shorter one as far as the ratio allows. The
// differences of a pixel's neighbours give the sizes to a few parts
// in a million, where the card's planes give a texture drawn one
// texel, or two or four, to a pixel its exact size, and the filters
// and the levels change over at just those sizes. The level of detail
// is rounded to 1/4096 so those sizes are exact here too.
float tf_lod(float rx, float ry, float aniso2, int needed) {
    float m = max(min(rx, ry), max(rx, ry) / aniso2);
    if (needed == 0 || !(m > 0.0)) return -1.0e30;
    return floor(0.5 * log2(m) * 4096.0 + 0.5) / 4096.0;
}
// `Sampler::levels`: the level, whether its filter is linear, and how
// much of the next level is blended in, of 256.
ivec3 tf_levels(float lod, ivec4 ts, ivec4 tm, float bias) {
    int last = ts.w;
    int first = min(ts.z, last);
    lod += bias;
    if (!(lod > 0.0)) return ivec3(first, tm.x, 0);
    if (tm.z == 0) return ivec3(first, tm.y, 0);
    if (tm.z == 1) return ivec3(clamp(int(min(lod + 0.5, 64.0)), first, last), tm.y, 0);
    float l = clamp(lod, float(first), float(last));
    int level = int(l);
    return ivec3(level, tm.y, level == last ? 0 : int(fract(l) * 256.0));
}
// `Sampler::sample`.
ivec4 tf_sample(sampler2D s, ivec3 lv, vec2 at, ivec2 size0, ivec2 mode, ivec4 border) {
    ivec4 near = tf_level(s, lv.x, at, size0, lv.y != 0, mode, border);
    if (lv.z == 0) return near;
    return tf_lerp(near, tf_level(s, lv.x + 1, at, size0, lv.y != 0, mode, border), lv.z);
}
// `Sampler::keyed`, on the texture that marks the key's texels.
bool tf_keyed(sampler2D k, int level, vec2 at, ivec2 size0, ivec2 mode) {
    ivec2 size = max(size0 >> level, ivec2(1));
    int x = tf_address(tf_fixed(at.x, size.x) >> 8, size.x, mode.x);
    int y = tf_address(tf_fixed(at.y, size.y) >> 8, size.y, mode.y);
    return x >= 0 && y >= 0 && texelFetch(k, ivec2(x, y), level).r > 0.5;
}
";

/// What a program is made from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    /// The draw's steps with what is the host's fixed state taken out.
    pub steps: Vec<Step>,
    /// A bit for each stage whose texture the host's own filter
    /// samples (`native`).
    pub native: u8,
}

impl Key {
    pub fn native(&self, stage: usize) -> bool {
        self.native >> stage & 1 != 0
    }
}

/// Whether the host's own filter samples `tex`: a cube map, whose
/// face the host picks, and an anisotropic sample where the host has
/// the filter (`anisotropic`).
pub fn native(tex: &Tex, anisotropic: bool) -> bool {
    tex.texture.cube || (anisotropic && tex.anisotropy > 1)
}

/// The key of the program that draws `state`.
pub fn key(state: &DrawState, anisotropic: bool) -> Key {
    let mut native_stages = 0;
    for (stage, tex) in state.tex.iter().enumerate() {
        if tex.as_ref().is_some_and(|tex| native(tex, anisotropic)) {
            native_stages |= 1 << stage;
        }
    }
    Key {
        steps: steps_key(&state.steps),
        native: native_stages,
    }
}

fn steps_key(steps: &[Step]) -> Vec<Step> {
    steps
        .iter()
        .filter_map(|step| match *step {
            Step::Depth { .. } => None,
            Step::Write { .. } => Some(Step::Write {
                blend: None,
                mask: 0,
            }),
            other => Some(other),
        })
        .collect()
}

fn reg(reg: Reg) -> &'static str {
    match reg {
        Reg::Current => "cur",
        Reg::Temp => "tmp",
        Reg::Diffuse => "dif",
        Reg::Specular => "spc",
        Reg::Factor => "fac",
        Reg::Texture => "tex",
    }
}

/// An argument as an expression.
fn arg(arg: Arg) -> String {
    let mut e = reg(arg.reg).to_string();
    if arg.complement {
        e = format!("(255 - {e})");
    }
    if arg.replicate {
        e = format!("ivec4({e}.a)");
    }
    e
}

/// `combine::channel` for the four channels at once, on the arguments
/// `a`, `b` and `c` and the alpha `t`.
fn channel(op: Op) -> &'static str {
    match op {
        Op::SelectArg1 | Op::DotProduct3 => "a",
        Op::SelectArg2 => "b",
        Op::Modulate => "mul(a, b)",
        Op::Modulate2x => "mul(a, b) * 2",
        Op::Modulate4x => "mul(a, b) * 4",
        Op::Add => "a + b",
        Op::AddSigned => "a + b - 128",
        Op::AddSigned2x => "(a + b - 128) * 2",
        Op::Subtract => "a - b",
        Op::AddSmooth => "a + b - mul(a, b)",
        Op::BlendAlpha => "(a * t + b * (255 - t) + 127) / 255",
        Op::BlendAlphaPm => "a + mul(b, ivec4(255 - t))",
        Op::ModulateAlphaAddColor => "a + mul(ivec4(t), b)",
        Op::ModulateColorAddAlpha => "mul(a, b) + t",
        Op::ModulateInvAlphaAddColor => "mul(ivec4(255 - t), b) + a",
        Op::ModulateInvColorAddAlpha => "mul(255 - a, b) + t",
        Op::MultiplyAdd => "c + mul(a, b)",
        Op::Lerp => "(c * a + (255 - c) * b + 127) / 255",
    }
}

/// A compare function (`D3DCMP`) of `value` against `reference`, as
/// the condition a fragment fails by. None for one every value passes.
fn fails(func: u32, value: &str, reference: &str) -> Option<String> {
    let op = match func {
        1 => return Some("true".to_string()),
        2 => ">=",
        3 => "!=",
        4 => ">",
        5 => "<=",
        6 => "==",
        7 => "<",
        _ => return None,
    };
    Some(format!("{value} {op} {reference}"))
}

/// The fragment shader of `key`.
pub fn fragment(key: &Key) -> String {
    let steps = &key.steps;
    let mut head = String::from(
        "
uniform ivec4 u_factor;
uniform int u_alpha_ref;
uniform int u_pack;
uniform ivec3 u_fog_colour;
uniform vec3 u_fog;
in vec4 v_diffuse;
in vec4 v_specular;
in vec4 v_tex[4];
",
    );
    head.push_str(LINEAR);
    head.push_str(
        "out vec4 o_color;
ivec4 mul(ivec4 a, ivec4 b) {
    return (a * b + 127) / 255;
}
",
    );
    head.push_str(SAMPLING);
    let mut body = String::from(MAIN);
    body.push_str(
        "    ivec4 cur = ivec4(0);
    ivec4 tmp = ivec4(0);
    ivec4 dif = ivec4(0);
    ivec4 spc = ivec4(0);
    ivec4 fac = ivec4(0);
    ivec4 tex = ivec4(0);
    vec2 bump = vec2(0.0);
    float lum = 1.0;
",
    );
    // How the slots' coordinates and 1 / w change over a pixel, taken
    // before a step can drop the fragment: a neighbour's difference is
    // of no use after that.
    let mut slots = [false; SLOTS];
    for step in steps {
        if let Step::Sample { stage, slot, .. } = *step
            && !key.native(stage as usize)
            && !std::mem::replace(&mut slots[slot as usize], true)
        {
            let _ = writeln!(
                body,
                "    vec4 sdx{slot} = dFdx(v_stw[{slot}]);\n    \
                 vec4 sdy{slot} = dFdy(v_stw[{slot}]);"
            );
        }
    }
    if slots.contains(&true) {
        body.push_str("    vec2 qd = vec2(dFdx(v_q), dFdy(v_q));\n");
    }
    // `write!` to a String can't fail.
    for step in steps {
        match *step {
            Step::Depth { .. } => {}
            Step::Colours { specular } => {
                body.push_str("    dif = ivec4(clamp(v_diffuse, 0.0, 255.0) + 0.5);\n");
                if specular {
                    body.push_str("    spc = ivec4(clamp(v_specular, 0.0, 255.0) + 0.5);\n");
                }
                body.push_str("    cur = dif;\n    fac = u_factor;\n");
            }
            Step::Sample {
                stage,
                slot,
                projected,
                keyed,
                bumped,
                ..
            } if !key.native(stage as usize) => {
                let _ = writeln!(
                    head,
                    "uniform sampler2D u_tex{stage};
uniform ivec4 u_ts{stage};
uniform ivec4 u_tm{stage};
uniform ivec2 u_ta{stage};
uniform vec2 u_tl{stage};
uniform ivec4 u_tb{stage};"
                );
                // What the coordinates are divided by, and how it
                // changes over a pixel: 1 / w, or the coordinate a
                // projected texture names.
                let (den, den_d) = match projected {
                    Some(k) => (
                        format!("s[{k}]"),
                        format!("vec2(sdx{slot}[{k}], sdy{slot}[{k}])"),
                    ),
                    None => ("v_q".to_string(), "qd".to_string()),
                };
                let _ = write!(
                    body,
                    "    {{
        vec4 s = v_stw[{slot}];
        float inv = 1.0 / {den};
        vec2 dd = {den_d};
        vec2 at = s.xy * inv;
        vec2 size = vec2(u_ts{stage}.xy);
        vec2 ax = (sdx{slot}.xy - at * dd.x) * inv * size;
        vec2 ay = (sdy{slot}.xy - at * dd.y) * inv * size;
        float lod = tf_lod(dot(ax, ax), dot(ay, ay), u_tl{stage}.y, u_tm{stage}.w);
        ivec3 lv = tf_levels(lod, u_ts{stage}, u_tm{stage}, u_tl{stage}.x);
"
                );
                if bumped.is_some() {
                    body.push_str("        at += bump;\n");
                }
                if keyed {
                    let _ = writeln!(head, "uniform sampler2D u_key{stage};");
                    let _ = writeln!(
                        body,
                        "        if (tf_keyed(u_key{stage}, lv.x, at, u_ts{stage}.xy, \
                         u_ta{stage})) discard;"
                    );
                }
                let _ = writeln!(
                    body,
                    "        tex = tf_sample(u_tex{stage}, lv, at, u_ts{stage}.xy, u_ta{stage}, \
                     u_tb{stage});"
                );
                if bumped == Some(true) {
                    body.push_str("        tex.rgb = ivec3(vec3(tex.rgb) * lum + 0.5);\n");
                }
                body.push_str("    }\n");
            }
            Step::Sample {
                stage,
                slot,
                projected,
                cube,
                keyed,
                bumped,
            } => {
                let kind = if cube { "samplerCube" } else { "sampler2D" };
                let _ = writeln!(head, "uniform {kind} u_tex{stage};");
                let at = if cube { "xyz" } else { "xy" };
                let _ = write!(body, "    {{\n        vec4 c = v_tex[{slot}];\n");
                let _ = match projected {
                    Some(k) => writeln!(body, "        vec{} at = c.{at} / c[{k}];", at.len()),
                    None => writeln!(body, "        vec{} at = c.{at};", at.len()),
                };
                if bumped.is_some() {
                    body.push_str("        at += bump;\n");
                }
                if keyed {
                    let _ = writeln!(head, "uniform {kind} u_key{stage};");
                    let _ = writeln!(
                        body,
                        "        if (texture(u_key{stage}, at).r > 0.5) discard;"
                    );
                }
                let _ = writeln!(
                    body,
                    "        tex = ivec4(texture(u_tex{stage}, at) * 255.0 + 0.5);"
                );
                if bumped == Some(true) {
                    body.push_str("        tex.rgb = ivec3(vec3(tex.rgb) * lum + 0.5);\n");
                }
                body.push_str("    }\n");
            }
            Step::Bump { stage, luminance } => {
                let _ = writeln!(head, "uniform vec4 u_bump{stage};");
                let _ = writeln!(head, "uniform vec2 u_lum{stage};");
                // The offsets are signed bytes where red and green are.
                let _ = write!(
                    body,
                    "    {{
        vec2 d = vec2(tex.rg - 256 * (tex.rg / 128)) / 128.0;
        bump = vec2(u_bump{stage}.x * d.x + u_bump{stage}.z * d.y,
                    u_bump{stage}.y * d.x + u_bump{stage}.w * d.y);
"
                );
                let _ = match luminance {
                    true => writeln!(
                        body,
                        "        lum = float(tex.b) / 255.0 * u_lum{stage}.x + u_lum{stage}.y;
        lum = clamp(lum, 0.0, 1.0);"
                    ),
                    false => writeln!(body, "        lum = 1.0;"),
                };
                body.push_str("    }\n");
            }
            Step::Combine {
                op,
                args,
                by,
                colour,
                alpha,
                scale,
                to,
            } => {
                let reads = op.reads();
                let get = |k: usize| match reads[k] {
                    true => arg(args[k]),
                    false => "ivec4(0)".to_string(),
                };
                let _ = write!(
                    body,
                    "    {{
        ivec4 a = {};
        ivec4 b = {};
        ivec4 c = {};
        int t = {};
",
                    get(0),
                    get(1),
                    get(2),
                    by.map_or("0".to_string(), |by| format!("{}.a", arg(by))),
                );
                if op == Op::DotProduct3 {
                    body.push_str(
                        "        ivec3 n = (a.rgb - 128) * (b.rgb - 128);
        int s = n.r + n.g + n.b;
        ivec4 r = ivec4(s <= 0 ? 0 : min((4 * s + 127) / 255, 255));
",
                    );
                } else {
                    let _ = writeln!(body, "        ivec4 r = {};", channel(op));
                }
                let _ = writeln!(body, "        r = clamp(r * {scale}, 0, 255);");
                let to = reg(to);
                if colour {
                    let _ = writeln!(body, "        {to}.rgb = r.rgb;");
                }
                if alpha {
                    let _ = writeln!(body, "        {to}.a = r.a;");
                }
                body.push_str("    }\n");
            }
            Step::SpecularAdd => {
                body.push_str("    cur.rgb = min(cur.rgb + spc.rgb, 255);\n");
            }
            Step::Fog(fog) => {
                let factor = match fog {
                    Fog::Vertex => "spc.a".to_string(),
                    Fog::Table { mode, by_w } => {
                        let d = match by_w {
                            true => "1.0 / gl_FragCoord.w",
                            false => "clamp(v_z, 0.0, 1.0)",
                        };
                        // Start, end and density.
                        let f = match mode {
                            3 => "(u_fog.y - d) / (u_fog.y - u_fog.x)",
                            1 => "exp(-max(u_fog.z * d, 0.0))",
                            _ => "exp(-(u_fog.z * d) * (u_fog.z * d))",
                        };
                        let _ = writeln!(body, "    float d = {d};");
                        format!("int(clamp({f}, 0.0, 1.0) * 255.0 + 0.5)")
                    }
                };
                let _ = write!(
                    body,
                    "    int f = {factor};
    cur.rgb = (cur.rgb * f + u_fog_colour * (255 - f) + 127) / 255;
"
                );
            }
            Step::AlphaTest { func } => {
                if let Some(fails) = fails(func, "cur.a", "u_alpha_ref") {
                    let _ = writeln!(body, "    if ({fails}) discard;");
                }
            }
            Step::Dither => {
                let matrix: Vec<String> = DITHER.iter().map(|d| d.to_string()).collect();
                let _ = write!(
                    body,
                    "    {{
        const int matrix[16] = int[16]({});
        ivec2 p = ivec2(gl_FragCoord.xy) & 3;
        int d = matrix[p.y * 4 + p.x];
        ivec2 five = min((cur.rb + d / 2) >> 3, 31);
        int six = min((cur.g + d / 4) >> 2, 63);
        cur.rb = five << 3 | five >> 2;
        cur.g = six << 2 | six >> 4;
    }}
",
                    matrix.join(", ")
                );
            }
            // A colour that goes to a 5-6-5 target as it is, without a
            // blend, is rounded here to the nearest the target holds
            // (`span::pack`), so it doesn't hang on how the host
            // rounds. The host rounds what it blends.
            Step::Write { .. } => body.push_str(
                "    if (u_pack != 0) {
        ivec2 five = (cur.rb * 31 + 127) / 255;
        int six = (cur.g * 63 + 127) / 255;
        cur.rb = five << 3 | five >> 2;
        cur.g = six << 2 | six >> 4;
    }
",
            ),
        }
    }
    body.push_str("    o_color = vec4(cur) / 255.0;\n}\n");
    head + &body
}

/// Where a program's uniforms are. One the program doesn't use is
/// None, and setting it does nothing.
pub struct Uniforms {
    pub scale: Option<glow::UniformLocation>,
    /// `VERTEX_TNL`'s: `u_all`, `u_x`, `u_y`, `u_z`, `u_shift`,
    /// `u_eye` and `u_vfog`, and the lighting's.
    pub all: Option<glow::UniformLocation>,
    pub x: Option<glow::UniformLocation>,
    pub y: Option<glow::UniformLocation>,
    pub z: Option<glow::UniformLocation>,
    pub shift: Option<glow::UniformLocation>,
    pub eye: Option<glow::UniformLocation>,
    pub vfog: Option<glow::UniformLocation>,
    pub lights: Option<glow::UniformLocation>,
    /// `u_lpos`, `u_laxis`, `u_ldiffuse`, `u_lspecular`, `u_lambient`,
    /// `u_latt` and `u_lcone`.
    pub lamps: [Option<glow::UniformLocation>; 7],
    pub mat: Option<glow::UniformLocation>,
    pub lmisc: Option<glow::UniformLocation>,
    pub lsrc: Option<glow::UniformLocation>,
    pub lflags: Option<glow::UniformLocation>,
    pub normals: Option<glow::UniformLocation>,
    /// The depth bias, as a part of the depth buffer's range.
    pub bias: Option<glow::UniformLocation>,
    pub factor: Option<glow::UniformLocation>,
    pub alpha_ref: Option<glow::UniformLocation>,
    /// Whether the colour is rounded to a 5-6-5 target's bits.
    pub pack: Option<glow::UniformLocation>,
    pub fog_colour: Option<glow::UniformLocation>,
    pub fog: Option<glow::UniformLocation>,
    pub bump: [Option<glow::UniformLocation>; STAGES],
    pub lum: [Option<glow::UniformLocation>; STAGES],
    /// A stage's numbers for `SAMPLING`: `ts`, `tm`, `ta`, `tl`, `tb`.
    pub sampling: [[Option<glow::UniformLocation>; 5]; STAGES],
}

impl Locations for Uniforms {
    fn resolve(gl: &glow::Context, program: glow::Program) -> Self {
        // SAFETY: the program was linked on this context, which is
        // current.
        unsafe {
            let at = |name: &str| gl.get_uniform_location(program, name);
            // The samplers read their units from now on.
            gl.use_program(Some(program));
            for stage in 0..STAGES {
                let tex = at(&format!("u_tex{stage}"));
                gl.uniform_1_i32(tex.as_ref(), unit(stage) as i32);
                let key = at(&format!("u_key{stage}"));
                gl.uniform_1_i32(key.as_ref(), key_unit(stage) as i32);
            }
            Uniforms {
                scale: at("u_scale"),
                all: at("u_all"),
                x: at("u_x"),
                y: at("u_y"),
                z: at("u_z"),
                shift: at("u_shift"),
                eye: at("u_eye"),
                vfog: at("u_vfog"),
                lights: at("u_lights"),
                lamps: ["lpos", "laxis", "ldiffuse", "lspecular", "lambient", "latt", "lcone"]
                    .map(|name| at(&format!("u_{name}"))),
                mat: at("u_mat"),
                lmisc: at("u_lmisc"),
                lsrc: at("u_lsrc"),
                lflags: at("u_lflags"),
                normals: at("u_normals"),
                bias: at("u_bias"),
                factor: at("u_factor"),
                alpha_ref: at("u_alpha_ref"),
                pack: at("u_pack"),
                fog_colour: at("u_fog_colour"),
                fog: at("u_fog"),
                bump: std::array::from_fn(|stage| at(&format!("u_bump{stage}"))),
                lum: std::array::from_fn(|stage| at(&format!("u_lum{stage}"))),
                sampling: std::array::from_fn(|stage| {
                    ["ts", "tm", "ta", "tl", "tb"].map(|name| at(&format!("u_{name}{stage}")))
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_leaves_out_what_is_fixed_state() {
        let depth = Step::Depth {
            func: 4,
            write: true,
            stencil: None,
        };
        let write = |mask| Step::Write { blend: None, mask };
        let colours = Step::Colours { specular: false };
        let a = steps_key(&[depth, colours, write(0xF)]);
        let b = steps_key(&[colours, write(0x7)]);
        assert_eq!(a, b);
        assert_ne!(a, steps_key(&[colours, Step::Dither, write(0xF)]));
    }

    #[test]
    fn a_transforming_programs_fragment_makes_the_linear_inputs() {
        let key = Key {
            steps: vec![Step::Colours { specular: false }],
            native: 0,
        };
        let (plain, tnl) = (fragment(&key), fragment_tnl(&key));
        assert!(plain.contains(LINEAR) && plain.contains(MAIN));
        assert!(!tnl.contains("noperspective"));
        assert!(tnl.contains(LINEAR_TNL) && tnl.contains(MAIN_TNL));
    }

    #[test]
    fn a_stage_prints_its_formula_on_its_arguments() {
        let step = Step::Combine {
            op: Op::Modulate2x,
            args: [
                Arg::plain(Reg::Texture),
                Arg {
                    reg: Reg::Diffuse,
                    complement: true,
                    replicate: true,
                },
                Arg::CURRENT,
            ],
            by: None,
            colour: true,
            alpha: false,
            scale: 1,
            to: Reg::Temp,
        };
        let source = fragment(&Key {
            steps: vec![step],
            native: 0,
        });
        assert!(source.contains("ivec4 a = tex;"));
        assert!(source.contains("ivec4 b = ivec4((255 - dif).a);"));
        // An argument the operation doesn't read is nothing.
        assert!(source.contains("ivec4 c = ivec4(0);"));
        assert!(source.contains("ivec4 r = mul(a, b) * 2;"));
        assert!(source.contains("tmp.rgb = r.rgb;"));
        assert!(!source.contains("tmp.a = r.a;"));
    }
}
