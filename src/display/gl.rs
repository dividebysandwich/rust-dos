//! The picture drawn with OpenGL 3, as it is or through a CRT look.

use crate::config::Filter;
use crate::video::Frame;
use crate::video::shader::{self, CrtSettings, Glsl, Shader};
use glow::HasContext;
use sdl2::VideoSubsystem;
use sdl2::video::{GLContext, GLProfile, SwapInterval, Window};
use std::collections::HashMap;

/// Why there is no OpenGL 3 to draw with, and the window if one was made
/// for it, which SDL's renderer can take instead.
pub struct NoGl {
    pub window: Option<Window>,
    pub reason: String,
}

/// A linked look and where its uniforms are.
struct Program {
    program: glow::Program,
    source: Option<glow::UniformLocation>,
    output: Option<glow::UniformLocation>,
    mask: Option<glow::UniformLocation>,
    curvature: Option<glow::UniformLocation>,
}

// Every `unsafe` below is a call into OpenGL, whose context `open` makes
// current on this thread and which stays current for the life of the
// `GlScreen`: nothing else in the program makes one.
pub struct GlScreen {
    // Dropped in this order: the functions, the context, then the window.
    gl: glow::Context,
    _context: GLContext,
    window: Window,
    glsl: Glsl,
    /// Empty: the vertex shader makes its triangle from the vertex number.
    vao: glow::VertexArray,
    texture: glow::Texture,
    /// The texture's size; nothing before the first frame.
    texture_size: (u32, u32),
    /// The looks compiled so far, or why one doesn't compile.
    programs: HashMap<Shader, Result<Program, String>>,
    /// The look drawn with: the one chosen, or none if it doesn't compile.
    active: Shader,
    /// 1 for a colour tube's mask in front of the CRT looks, 0 for a
    /// monochrome tube's none (`u_mask`).
    mask: f32,
    /// The CRT look's own settings.
    crt: CrtSettings,
    renderer: String,
}

impl GlScreen {
    /// Make a window of `size` with an OpenGL 3 context: 3.2 core, which is
    /// what macOS has, or else 3.0 or later with the older functions.
    pub fn open(video: &VideoSubsystem, title: &str, (width, height): (u32, u32)) -> Result<Self, NoGl> {
        let no_gl = |window, reason: String| NoGl { window, reason };
        // It has no OpenGL, and SDL would refuse the window.
        if video.current_video_driver() == "dummy" {
            return Err(no_gl(None, "the dummy video driver".to_string()));
        }
        let attr = video.gl_attr();
        attr.set_context_profile(GLProfile::Core);
        attr.set_context_version(3, 2);
        if cfg!(target_os = "macos") {
            attr.set_context_flags().forward_compatible().set();
        }
        attr.set_double_buffer(true);
        attr.set_depth_size(0);
        let window = video
            .window(title, width, height)
            .position_centered()
            .opengl()
            .allow_highdpi()
            .build()
            .map_err(|e| no_gl(None, e.to_string()))?;
        let context = match window.gl_create_context() {
            Ok(context) => context,
            Err(core) => {
                attr.set_context_profile(GLProfile::Compatibility);
                attr.set_context_version(3, 0);
                attr.set_context_flags().set();
                match window.gl_create_context() {
                    Ok(context) => context,
                    Err(e) => return Err(no_gl(Some(window), format!("{}; {}", core, e))),
                }
            }
        };
        // SAFETY: the context just made is current.
        let gl = unsafe { glow::Context::from_loader_function(|name| video.gl_get_proc_address(name).cast()) };
        let version = gl.version();
        let Some(glsl) = Glsl::for_gl(version.major, version.minor, version.is_embedded) else {
            let reason = format!("OpenGL {}.{} is older than 3.0", version.major, version.minor);
            return Err(no_gl(Some(window), reason));
        };
        // The emulator paces its frames itself, as with SDL's renderer.
        let _ = video.gl_set_swap_interval(SwapInterval::Immediate);

        // SAFETY: see `GlScreen`.
        let objects = unsafe {
            gl.create_vertex_array().and_then(|vao| {
                let texture = gl.create_texture()?;
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
                // Frame rows are packed, three bytes a pixel.
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                Ok((vao, texture))
            })
        };
        let (vao, texture) = match objects {
            Ok(objects) => objects,
            Err(e) => return Err(no_gl(Some(window), e)),
        };
        // Every picture needs the plain look, if only as the fallback.
        let plain = match compile(&gl, glsl, Shader::None) {
            Ok(plain) => plain,
            Err(e) => return Err(no_gl(Some(window), e)),
        };
        // SAFETY: see `GlScreen`.
        let renderer = unsafe {
            format!(
                "OpenGL {} on {}, {:?}",
                gl.get_parameter_string(glow::VERSION),
                gl.get_parameter_string(glow::RENDERER),
                glsl
            )
        };
        Ok(Self {
            gl,
            _context: context,
            window,
            glsl,
            vao,
            texture,
            texture_size: (0, 0),
            programs: HashMap::from([(Shader::None, Ok(plain))]),
            active: Shader::None,
            mask: 1.0,
            crt: CrtSettings::default(),
            renderer,
        })
    }

    /// What draws the picture, for the log.
    pub fn renderer(&self) -> &str {
        &self.renderer
    }

    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn window_mut(&mut self) -> &mut Window {
        &mut self.window
    }

    /// The look the picture is drawn with.
    pub fn active(&self) -> Shader {
        self.active
    }

    /// Draw with `shader`, and without one scale the picture with
    /// `filter`. A look that doesn't compile leaves the picture plain.
    pub fn select(&mut self, shader: Shader, filter: Filter) -> Result<(), String> {
        let (gl, glsl) = (&self.gl, self.glsl);
        let compiled = self.programs.entry(shader).or_insert_with(|| compile(gl, glsl, shader));
        let result = match compiled {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("The {} shader doesn't work here: {}", shader.describe(), e)),
        };
        self.active = if result.is_ok() { shader } else { Shader::None };
        let mipmaps = shader::needs_mipmaps(self.active);
        let (min, mag) = match (mipmaps, filter) {
            (true, _) => (glow::LINEAR_MIPMAP_LINEAR, glow::LINEAR),
            (false, Filter::Nearest) => (glow::NEAREST, glow::NEAREST),
            (false, Filter::Linear) => (glow::LINEAR, glow::LINEAR),
        };
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, min as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, mag as i32);
            // Without its mipmap the texture would read as black.
            if mipmaps && self.texture_size != (0, 0) {
                gl.generate_mipmap(glow::TEXTURE_2D);
            }
        }
        result
    }

    /// Whether the CRT looks put a colour tube's mask in front of the
    /// picture, or show a monochrome tube, whose phosphor is one colour.
    pub fn set_color_mask(&mut self, on: bool) {
        self.mask = if on { 1.0 } else { 0.0 };
    }

    /// Show the CRT look with `crt`.
    pub fn set_crt(&mut self, crt: CrtSettings) {
        self.crt = crt;
    }

    /// Show `frame`, letterboxed at `display` proportions.
    pub fn present(&mut self, frame: &Frame, display: (u32, u32)) {
        let gl = &self.gl;
        let size = (frame.width, frame.height);
        let (width, height) = (frame.width as i32, frame.height as i32);
        let (dw, dh) = self.window.drawable_size();
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            if size != self.texture_size {
                let none = glow::PixelUnpackData::Slice(None);
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGB8 as i32,
                    width,
                    height,
                    0,
                    glow::RGB,
                    glow::UNSIGNED_BYTE,
                    none,
                );
                self.texture_size = size;
            }
            let pixels = glow::PixelUnpackData::Slice(Some(&frame.rgb));
            gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, width, height, glow::RGB, glow::UNSIGNED_BYTE, pixels);
            if shader::needs_mipmaps(self.active) {
                gl.generate_mipmap(glow::TEXTURE_2D);
            }

            gl.viewport(0, 0, dw as i32, dh as i32);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            // `select` compiled the active look.
            if let Some(Ok(program)) = self.programs.get(&self.active)
                && dw > 0
                && dh > 0
            {
                let (x, y, w, h) = super::letterbox((dw, dh), display);
                // OpenGL counts rows from the bottom.
                gl.viewport(x as i32, (dh - y - h) as i32, w as i32, h as i32);
                gl.use_program(Some(program.program));
                gl.uniform_2_f32(program.source.as_ref(), frame.width as f32, frame.height as f32);
                gl.uniform_2_f32(program.output.as_ref(), w as f32, h as f32);
                gl.uniform_1_f32(program.mask.as_ref(), self.mask);
                let [cx, cy] = self.active.curvature(self.crt);
                gl.uniform_2_f32(program.curvature.as_ref(), cx, cy);
                gl.bind_vertex_array(Some(self.vao));
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
        }
        self.window.gl_swap_window();
    }
}

/// Compile and link a look. The error is the first line of the log; the
/// whole log goes to the terminal.
fn compile(gl: &glow::Context, glsl: Glsl, shader: Shader) -> Result<Program, String> {
    let (vertex, fragment) = shader::sources(shader, glsl);
    let failed = |what: &str, log: String| {
        eprintln!("[DISPLAY] The {} shader {}:\n{}", shader.name(), what, log);
        let first = log.lines().find(|line| !line.trim().is_empty()).unwrap_or("no log");
        format!("{} ({})", what, first.trim())
    };
    // SAFETY: see `GlScreen`.
    unsafe {
        let program = gl.create_program()?;
        let mut stages = Vec::new();
        let mut problem = None;
        for (kind, source) in [(glow::VERTEX_SHADER, vertex), (glow::FRAGMENT_SHADER, fragment)] {
            let stage = gl.create_shader(kind)?;
            gl.shader_source(stage, &source);
            gl.compile_shader(stage);
            gl.attach_shader(program, stage);
            stages.push(stage);
            if !gl.get_shader_compile_status(stage) {
                problem = Some(failed("doesn't compile", gl.get_shader_info_log(stage)));
                break;
            }
        }
        if problem.is_none() {
            if glsl != Glsl::Es300 {
                gl.bind_frag_data_location(program, 0, "o_color");
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                problem = Some(failed("doesn't link", gl.get_program_info_log(program)));
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
        gl.use_program(Some(program));
        gl.uniform_1_i32(gl.get_uniform_location(program, "u_frame").as_ref(), 0);
        Ok(Program {
            program,
            source: gl.get_uniform_location(program, "u_source"),
            output: gl.get_uniform_location(program, "u_output"),
            mask: gl.get_uniform_location(program, "u_mask"),
            curvature: gl.get_uniform_location(program, "u_curvature"),
        })
    }
}
