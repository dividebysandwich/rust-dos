//! What the eyes see wearing the VFX1 on the desk (`[vfx1]`): each eye its
//! own picture of the game's, in front of it wherever the head turns, on
//! black, as the headset's two little screens were.

use crate::video::shader::Glsl;
use glam::{Mat4, Vec3};

/// How far ahead the pictures are, in metres. Each eye's is straight
/// ahead of it, the head's way, so that they look as far away as the
/// headset's screens did, whichever way a headset's own eyes are turned.
const DISTANCE: f32 = 3.0;
use glow::HasContext;

const VERTEX: &str = "
uniform mat4 u_head_to_clip;
uniform vec3 u_center;
uniform vec2 u_half;
out vec2 v_uv;
void main() {
    vec2 corner = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1)) * 2.0 - 1.0;
    v_uv = vec2(corner.x + 1.0, 1.0 - corner.y) * 0.5;
    gl_Position = u_head_to_clip * vec4(u_center + vec3(corner * u_half, 0.0), 1.0);
}
";

const FRAGMENT: &str = "
uniform sampler2D u_picture;
uniform float u_fade;
uniform bool u_linear;
in vec2 v_uv;
out vec4 o_color;
void main() {
    vec3 c = texture(u_picture, v_uv).rgb * u_fade;
    // Where the framebuffer encodes sRGB itself, it is given linear light.
    if (u_linear) {
        c = mix(c / 12.92, pow((c + 0.055) / 1.055, vec3(2.4)), step(0.04045, c));
    }
    o_color = vec4(c, 1.0);
}
";

pub struct Visor {
    program: glow::Program,
    /// None of its own: the corners come from the vertex's index.
    vao: glow::VertexArray,
}

impl Visor {
    pub fn new(gl: &glow::Context, glsl: Glsl) -> Result<Self, String> {
        // SAFETY: see `GlScreen`: this thread's context is current.
        unsafe {
            let program = gl.create_program()?;
            let mut stages = Vec::new();
            let mut problem = None;
            for (kind, source) in [
                (glow::VERTEX_SHADER, VERTEX),
                (glow::FRAGMENT_SHADER, FRAGMENT),
            ] {
                let stage = gl.create_shader(kind)?;
                gl.shader_source(stage, &format!("{}{}", glsl.preamble(), source));
                gl.compile_shader(stage);
                gl.attach_shader(program, stage);
                stages.push(stage);
                if !gl.get_shader_compile_status(stage) {
                    problem = Some(format!(
                        "the VFX1's shader doesn't compile: {}",
                        gl.get_shader_info_log(stage)
                    ));
                    break;
                }
            }
            if problem.is_none() {
                gl.bind_frag_data_location(program, 0, "o_color");
                gl.link_program(program);
                if !gl.get_program_link_status(program) {
                    problem = Some(format!(
                        "the VFX1's shader doesn't link: {}",
                        gl.get_program_info_log(program)
                    ));
                }
            }
            for stage in stages {
                gl.detach_shader(program, stage);
                gl.delete_shader(stage);
            }
            if let Some(problem) = problem {
                gl.delete_program(program);
                return Err(problem);
            }
            let vao = gl.create_vertex_array()?;
            Ok(Visor { program, vao })
        }
    }

    /// Draw an eye into `framebuffer` (`area` of it, from the bottom left):
    /// black, and its `picture` 4:3 and `width` degrees across straight
    /// ahead of the eye at `eye` in the head's frame, which `head_to_clip`
    /// takes to the eye's clip space, at `fade` of its brightness.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        gl: &glow::Context,
        framebuffer: glow::Framebuffer,
        area: (u32, u32),
        srgb: bool,
        head_to_clip: Mat4,
        eye: Vec3,
        picture: Option<glow::Texture>,
        width: f32,
        fade: f32,
    ) {
        let across = DISTANCE * (width.to_radians() / 2.0).tan();
        let half = [across, across * 0.75];
        let center = eye + Vec3::NEG_Z * DISTANCE;
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.viewport(0, 0, area.0 as i32, area.1 as i32);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::BLEND);
            gl.disable(glow::CULL_FACE);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            if let Some(picture) = picture {
                if srgb {
                    gl.enable(glow::FRAMEBUFFER_SRGB);
                }
                gl.use_program(Some(self.program));
                let at = |name: &str| gl.get_uniform_location(self.program, name);
                gl.uniform_matrix_4_f32_slice(
                    at("u_head_to_clip").as_ref(),
                    false,
                    &head_to_clip.to_cols_array(),
                );
                gl.uniform_3_f32(at("u_center").as_ref(), center.x, center.y, center.z);
                gl.uniform_2_f32(at("u_half").as_ref(), half[0], half[1]);
                gl.uniform_1_f32(at("u_fade").as_ref(), fade.clamp(0.0, 1.0));
                gl.uniform_1_i32(at("u_linear").as_ref(), i32::from(srgb));
                gl.uniform_1_i32(at("u_picture").as_ref(), 0);
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(picture));
                gl.bind_vertex_array(Some(self.vao));
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                gl.bind_vertex_array(None);
                gl.bind_texture(glow::TEXTURE_2D, None);
                gl.use_program(None);
                gl.disable(glow::FRAMEBUFFER_SRGB);
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }

    pub fn delete(self, gl: &glow::Context) {
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
        }
    }
}
