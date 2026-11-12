//! A draw's steps printed as a GLSL fragment shader ("One pipeline
//! description, two back ends" in `docs/toadforce.md`).
//!
//! The shader keeps the registers of `span.rs` as vectors of four
//! integers, 0 to 255, in the order red, green, blue, alpha, and each
//! step is the formula `combine.rs` has for it in the same whole
//! numbers. What isn't in whole numbers there isn't here either: the
//! colours' and coordinates' interpolation, the texture filter, the
//! bump offsets and the fog factor.
//!
//! The depth and stencil tests, the blend and the write masks are the
//! host's fixed state, set for each run (`state.rs`), so the steps that
//! name them print nothing and two draws that differ in them alone
//! share a program.

use crate::toadforce::level1::states::STAGES;
use crate::toadforce::raster::combine::{DITHER, Op};
use crate::toadforce::raster::pipeline::{Arg, Fog, Reg, SLOTS, Step};
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
    v_z = a_pos.z;
}
";

/// What of a draw's steps its program is made from: the steps with
/// what is the host's fixed state taken out.
pub fn key(steps: &[Step]) -> Vec<Step> {
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

/// The fragment shader of the steps `steps`.
pub fn fragment(steps: &[Step]) -> String {
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
noperspective in float v_z;
out vec4 o_color;
ivec4 mul(ivec4 a, ivec4 b) {
    return (a * b + 127) / 255;
}
",
    );
    let mut body = String::from(
        "void main() {
    ivec4 cur = ivec4(0);
    ivec4 tmp = ivec4(0);
    ivec4 dif = ivec4(0);
    ivec4 spc = ivec4(0);
    ivec4 fac = ivec4(0);
    ivec4 tex = ivec4(0);
    vec2 bump = vec2(0.0);
    float lum = 1.0;
",
    );
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
                bias: at("u_bias"),
                factor: at("u_factor"),
                alpha_ref: at("u_alpha_ref"),
                pack: at("u_pack"),
                fog_colour: at("u_fog_colour"),
                fog: at("u_fog"),
                bump: std::array::from_fn(|stage| at(&format!("u_bump{stage}"))),
                lum: std::array::from_fn(|stage| at(&format!("u_lum{stage}"))),
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
        let a = key(&[depth, colours, write(0xF)]);
        let b = key(&[colours, write(0x7)]);
        assert_eq!(a, b);
        assert_ne!(a, key(&[colours, Step::Dither, write(0xF)]));
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
        let source = fragment(&[step]);
        assert!(source.contains("ivec4 a = tex;"));
        assert!(source.contains("ivec4 b = ivec4((255 - dif).a);"));
        // An argument the operation doesn't read is nothing.
        assert!(source.contains("ivec4 c = ivec4(0);"));
        assert!(source.contains("ivec4 r = mul(a, b) * 2;"));
        assert!(source.contains("tmp.rgb = r.rgb;"));
        assert!(!source.contains("tmp.a = r.a;"));
    }
}
