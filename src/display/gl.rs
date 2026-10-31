//! The picture drawn with OpenGL 3, as it is or through a CRT look.

#[cfg(test)]
#[path = "gl/present_tests.rs"]
mod present_tests;
mod xbrz;

use rust_dos::voodoo::gl::{SharedGl, VoodooGl};
use rust_dos::voodoo::gpu::GpuLink;
use std::cell::RefCell;
use std::rc::Rc;
use crate::config::Filter;
use crate::video::Frame;
use crate::video::mono::Monochrome;
use rust_dos::config_ui::Layer;
use crate::video::shader::{self, CrtSettings, Glsl, Shader};
use glow::HasContext;
use super::PresentRows;
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
    glow: Option<glow::UniformLocation>,
    line: Option<glow::UniformLocation>,
    overlay_enabled: Option<glow::UniformLocation>,
    unflipped: Option<glow::UniformLocation>,
}

// Every `unsafe` below is a call into OpenGL, whose context `open` makes
// current on this thread and which stays current for the life of the
// `GlScreen`: nothing else in the program makes one.
pub struct GlScreen {
    // Dropped in this order: the functions, the context, then the window.
    // The 3dfx card shares the functions and its renderer (`voodoo_link`)
    // until this ends, which ends the renderer.
    gl: Rc<glow::Context>,
    _context: GLContext,
    window: Window,
    glsl: Glsl,
    /// Empty: the vertex shader makes its triangle from the vertex number.
    vao: glow::VertexArray,
    texture: glow::Texture,
    /// The texture's size; nothing before the first frame.
    texture_size: (u32, u32),
    /// Composed frame pixels per uploaded texel.
    texture_scale: u32,
    /// Subsampled rows when a doubled VGA picture is shown at half size.
    rgba: Vec<u8>,
    /// Composed pixels that differ from the native xBRZ base.
    xbrz_overlay: Option<glow::Texture>,
    xbrz_overlay_size: Option<(u32, u32)>,
    xbrz_overlay_uploaded_revision: Option<u32>,
    xbrz_overlay_visible: bool,
    xbrz_frame: shader::XbrzFrame,
    xbrz_uploaded_revision: Option<u32>,
    xbrz_gpu: Option<Result<xbrz::XbrzGpu, String>>,
    #[cfg(test)]
    fail_overlay_upload: bool,
    /// The looks compiled so far, or why one doesn't compile.
    programs: HashMap<Shader, Result<Program, String>>,
    /// The look drawn with: the one chosen, or none if it doesn't compile.
    active: Shader,
    /// 1 for a colour tube's mask in front of the CRT looks, 0 for a
    /// monochrome tube's none (`u_mask`).
    mask: f32,
    /// The CRT look's own settings.
    crt: CrtSettings,
    /// Whether the CRT look's lines follow the 3dfx card's scale, or stay
    /// at the card's own size.
    scale_lines: bool,
    /// The scaling filter without a look.
    filter: Filter,
    renderer: String,
    /// The 3dfx card drawn with OpenGL, and why it can't be if it can't.
    voodoo: Rc<RefCell<Option<VoodooGl>>>,
    voodoo_failed: Option<String>,
    /// Whether the log said that a Banshee's 2D operations stay on the
    /// CPU (`VoodooGl::gpu_2d_problem`).
    gpu_2d_logged: bool,
    /// What screenshots and recordings of the look are drawn with, made
    /// the first time one is (`capture`).
    capture: Option<Capture>,
    /// The texture of the layer over the picture (a manual's page), made
    /// the first time there is one, and the layer in it.
    layer: Option<glow::Texture>,
    layer_generation: Option<u64>,
    /// The 3D scene the picture is shown in, with `[vr]`.
    #[cfg(feature = "vr")]
    stage: Option<Box<super::stage::Stage>>,
    /// The look drawn without the tube's curve: the scene's screen has
    /// its own shape.
    flat: bool,
}

/// The scene goes first: the headset's thread draws with a context of the
/// window's.
#[cfg(feature = "vr")]
impl Drop for GlScreen {
    fn drop(&mut self) {
        drop(self.stage.take());
        // A link the card still has finds no renderer from now on.
        self.drop_voodoo();
    }
}

/// A picture to draw through the look away from the window, and the
/// framebuffer it is drawn into and read back from.
struct Capture {
    source: glow::Texture,
    target: glow::Texture,
    framebuffer: glow::Framebuffer,
    /// The target's size; nothing before the first capture.
    size: (u32, u32),
    /// The pixels read back, four bytes a pixel, bottom row first.
    rgba: Vec<u8>,
}

impl GlScreen {
    /// Make a window of `size` with an OpenGL 3 context: 3.2 core, which is
    /// what macOS has, or else 3.0 or later with the older functions. A
    /// `hidden` window isn't shown, and its context draws all the same.
    pub fn open(
        video: &VideoSubsystem,
        title: &str,
        (width, height): (u32, u32),
        hidden: bool,
    ) -> Result<Self, NoGl> {
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
        let mut builder = video.window(title, width, height);
        builder.position_centered().opengl().allow_highdpi();
        if hidden {
            builder.hidden();
        }
        let window = builder
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
                // Frame rows are packed, four bytes a pixel.
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
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
        // The vendor too: on a laptop with two graphics chips it says
        // which one the driver gave the program.
        // SAFETY: see `GlScreen`.
        let (version_name, device_name, vendor) = unsafe {
            (
                gl.get_parameter_string(glow::VERSION),
                gl.get_parameter_string(glow::RENDERER),
                gl.get_parameter_string(glow::VENDOR),
            )
        };
        let renderer =
            format!("OpenGL {} on {} ({}), {:?}", version_name, device_name, vendor, glsl);
        Ok(Self {
            gl: Rc::new(gl),
            _context: context,
            window,
            glsl,
            vao,
            texture,
            texture_size: (0, 0),
            texture_scale: 1,
            rgba: Vec::new(),
            xbrz_overlay: None,
            xbrz_overlay_size: None,
            xbrz_overlay_uploaded_revision: None,
            xbrz_overlay_visible: false,
            xbrz_frame: shader::XbrzFrame::default(),
            xbrz_uploaded_revision: None,
            xbrz_gpu: None,
            #[cfg(test)]
            fail_overlay_upload: false,
            programs: HashMap::from([(Shader::None, Ok(plain))]),
            active: Shader::None,
            mask: 1.0,
            crt: CrtSettings::default(),
            scale_lines: true,
            filter: Filter::Nearest,
            renderer,
            voodoo: Rc::new(RefCell::new(None)),
            voodoo_failed: (glsl == Glsl::Es300).then(|| "OpenGL ES can't draw it".to_string()),
            gpu_2d_logged: false,
            capture: None,
            layer: None,
            layer_generation: None,
            #[cfg(feature = "vr")]
            stage: None,
            flat: false,
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
        if shader == Shader::Xbrz && self.xbrz_gpu.is_none() {
            let cache = xbrz::XbrzGpu::new(gl, glsl);
            if let Err(error) = &cache {
                eprintln!("[DISPLAY] xBRZ cache unavailable: {error}; using the direct shader");
            }
            self.xbrz_gpu = Some(cache);
        }
        let compiled = self.programs.entry(shader).or_insert_with(|| compile(gl, glsl, shader));
        let result = match compiled {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("The {} shader doesn't work here: {}", shader.describe(), e)),
        };
        let active = if result.is_ok() { shader } else { Shader::None };
        if active != self.active {
            self.xbrz_frame.invalidate();
        }
        self.active = active;
        self.filter = filter;
        let mipmaps = shader::needs_mipmaps(self.active);
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            self.set_filter();
            // Without its mipmap the texture would read as black.
            if mipmaps && self.texture_size != (0, 0) {
                self.gl.generate_mipmap(glow::TEXTURE_2D);
            }
        }
        result
    }

    /// Scale the bound texture as the look and the filter want.
    fn set_filter(&self) {
        let (min, mag) = match (shader::needs_mipmaps(self.active), self.filter) {
            (true, _) => (glow::LINEAR_MIPMAP_LINEAR, glow::LINEAR),
            (false, Filter::Nearest) => (glow::NEAREST, glow::NEAREST),
            (false, Filter::Linear) => (glow::LINEAR, glow::LINEAR),
        };
        // SAFETY: see `GlScreen`.
        unsafe {
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, min as i32);
            self.gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, mag as i32);
        }
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

    /// Whether the CRT look's lines follow the 3dfx card's scale.
    pub fn set_scale_lines(&mut self, on: bool) {
        self.scale_lines = on;
    }

    /// Prepare the same native input for the window and shader captures.
    fn prepare_xbrz(
        &mut self,
        screen: &Frame,
        base: Option<&Frame>,
        rows: PresentRows,
        mono: Monochrome,
    ) -> Result<bool, String> {
        let mut prepared = std::mem::take(&mut self.xbrz_frame);
        let (visible, changed, _) = if let Some(candidates) = rows.candidates {
            prepared.prepare_rows(screen, base, mono, candidates.clone(), candidates)
        } else {
            prepared.prepare(screen, base, mono, rows.changed)
        };
        let scale = if base.unwrap_or(screen).doubled { 2 } else { 1 };
        let source_changed = self.xbrz_uploaded_revision != Some(prepared.source_revision());
        if source_changed {
            let sequential = self
                .xbrz_uploaded_revision
                .is_some_and(|revision| revision.wrapping_add(1) == prepared.source_revision())
                && self.texture_size == (prepared.source.width, prepared.source.height);
            let rows = if sequential {
                prepared.source_rows()
            } else {
                0..prepared.source.height as usize
            };
            let partial = sequential
                && prepared.source_damage().rects()
                    != [[0, 0, prepared.source.width, prepared.source.height]];
            if partial {
                self.upload_xbrz_regions(&prepared);
            } else {
                self.upload(&prepared.source, rows, None);
            }
            self.xbrz_uploaded_revision = Some(prepared.source_revision());
        }
        self.texture_scale = scale as u32;
        self.xbrz_frame = prepared;
        // Exercise fallback after the source texture has been replaced.
        #[cfg(test)]
        if std::mem::take(&mut self.fail_overlay_upload) {
            return Err("injected overlay allocation failure".into());
        }
        let revision = self.xbrz_frame.overlay_revision();
        let overlay_changed = visible
            && (self.xbrz_overlay_uploaded_revision != Some(revision)
                || self.xbrz_overlay_size != Some((screen.width, screen.height)));
        if overlay_changed {
            let texture = match self.xbrz_overlay {
                Some(texture) => texture,
                None => {
                    // SAFETY: this screen owns the current context.
                    let texture = unsafe { self.gl.create_texture()? };
                    self.xbrz_overlay = Some(texture);
                    texture
                }
            };
            let gl = &self.gl;
            // SAFETY: this screen owns the current context and texture.
            unsafe {
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::NEAREST as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::NEAREST as i32,
                );
                if self.xbrz_overlay_size == Some((screen.width, screen.height)) {
                    // Missed revisions may change rows outside the latest
                    // band, so recover with a complete upload.
                    let rows = if self
                        .xbrz_overlay_uploaded_revision
                        .is_some_and(|old| old.wrapping_add(1) == revision)
                    {
                        self.xbrz_frame.overlay_rows()
                    } else {
                        0..screen.height as usize
                    };
                    let row_bytes = screen.width as usize * 4;
                    let pixels =
                        &self.xbrz_frame.overlay.rgba[rows.start * row_bytes..rows.end * row_bytes];
                    gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        0,
                        rows.start as i32,
                        screen.width as i32,
                        rows.len() as i32,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(pixels)),
                    );
                } else {
                    gl.tex_image_2d(
                        glow::TEXTURE_2D, 0, glow::RGBA8 as i32, screen.width as i32,
                        screen.height as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(&self.xbrz_frame.overlay.rgba)),
                    );
                    self.xbrz_overlay_size = Some((screen.width, screen.height));
                }
                gl.active_texture(glow::TEXTURE0);
            }
            self.xbrz_overlay_uploaded_revision = Some(revision);
        }
        self.xbrz_overlay_visible = visible;
        Ok(changed || source_changed || overlay_changed)
    }

    /// Show the frame, using a clean base for xBRZ when one is available.
    /// Without a base, xBRZ filters the already composed and tinted frame.
    pub fn present(
        &mut self,
        frame: &Frame,
        base: Option<&Frame>,
        rows: impl Into<PresentRows>,
        display: (u32, u32),
        layer: Option<&Layer>,
        mono: Monochrome,
    ) -> Result<(), String> {
        let rows = rows.into();
        if self.active == Shader::Xbrz {
            let damaged = !rows.changed.is_empty();
            let changed = self.prepare_xbrz(frame, base, rows.clone(), mono)?;
            if !changed && !damaged {
                self.xbrz_overlay_visible = false;
                #[cfg(feature = "vr")]
                self.render_stage();
                return Ok(());
            }
        } else {
            self.xbrz_frame.invalidate();
            self.texture_scale = self.upload(frame, rows.changed, layer);
        }
        self.draw(self.texture, self.texture_size, display, layer, 1.0);
        self.xbrz_overlay_visible = false;
        Ok(())
    }

    fn upload_xbrz_regions(&mut self, prepared: &shader::XbrzFrame) {
        let gl = &self.gl;
        let source = &prepared.source;
        // SAFETY: this screen owns the context and source texture. Each
        // rectangle lies within the prepared source; reset unpack stride
        // before another uploader uses this context.
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, source.width as i32);
            for &[x, y, w, h] in prepared.source_damage().rects() {
                let start = ((y * source.width + x) * 4) as usize;
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    x as i32,
                    y as i32,
                    w as i32,
                    h as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&source.rgba[start..])),
                );
            }
            gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
        }
        if let Some(Ok(cache)) = &mut self.xbrz_gpu {
            cache.invalidate_regions(prepared.source_damage(), (source.width, source.height));
        }
    }

    fn upload(
        &mut self,
        frame: &Frame,
        rows: std::ops::Range<usize>,
        layer: Option<&Layer>,
    ) -> u32 {
        let gl = &self.gl;
        // Nearest scaling preserves the picture with a quarter of the
        // upload bytes. xBRZ recovers native pixels during preparation.
        let half = frame.doubled
            && layer.is_none()
            && self.active == Shader::None
            && self.filter == Filter::Nearest;
        let (scale, rows) = if half { (2, rows.start / 2..rows.end.div_ceil(2)) } else { (1, rows) };
        let size = (frame.width / scale, frame.height / scale);
        let (width, height) = (size.0 as i32, size.1 as i32);
        let rows = if size != self.texture_size { 0..size.1 as usize } else { rows };
        if !rows.is_empty() || size != self.texture_size {
            self.xbrz_uploaded_revision = None;
            if let Some(Ok(cache)) = &mut self.xbrz_gpu {
                cache.invalidate();
            }
        }
        let row_bytes = frame.width as usize * 4;
        let pixels = if half {
            let w = size.0 as usize;
            self.rgba.resize(rows.len() * w * 4, 0);
            for (y, out) in rows.clone().zip(self.rgba.chunks_exact_mut(w * 4)) {
                let src = frame.rgba[2 * y * row_bytes..(2 * y + 1) * row_bytes]
                    .as_chunks::<8>().0;
                for (rgba, pair) in out.as_chunks_mut::<4>().0.iter_mut().zip(src) {
                    rgba.copy_from_slice(&pair[..4]);
                }
            }
            &self.rgba[..]
        } else {
            &frame.rgba[rows.start * row_bytes..rows.end * row_bytes]
        };
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            if size != self.texture_size {
                let none = glow::PixelUnpackData::Slice(None);
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    width,
                    height,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    none,
                );
                self.texture_size = size;
            }
            let pixels = glow::PixelUnpackData::Slice(Some(pixels));
            let (y, h) = (rows.start as i32, (rows.end - rows.start) as i32);
            gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, y, width, h, glow::RGBA, glow::UNSIGNED_BYTE, pixels);
            if shader::needs_mipmaps(self.active) {
                gl.generate_mipmap(glow::TEXTURE_2D);
            }
        }
        scale
    }

    /// Whether the 3dfx card can be drawn with OpenGL here, or why not.
    pub fn voodoo_problem(&self) -> Option<&str> {
        self.voodoo_failed.as_deref()
    }

    /// The 3dfx card's OpenGL settings, if it is drawn.
    pub fn voodoo_scale(&self) -> Option<(u32, u32, u32)> {
        let voodoo = self.voodoo.borrow();
        voodoo.as_ref().map(|v| (v.scale(), v.samples(), v.anisotropy()))
    }

    /// Stop drawing the 3dfx card.
    pub fn drop_voodoo(&mut self) {
        let voodoo = self.voodoo.borrow_mut().take();
        if let Some(voodoo) = voodoo {
            voodoo.destroy(&self.gl);
        }
    }

    /// The renderer for the 3dfx card to do its 2D operations with
    /// between frames (`voodoo::gpu`).
    pub fn voodoo_link(&self) -> Box<dyn GpuLink> {
        Box::new(SharedGl {
            gl: self.gl.clone(),
            renderer: self.voodoo.clone(),
        })
    }

    /// Draw what the 3dfx card recorded, at `scale` times its size with
    /// `samples` a pixel and `anisotropy` for textures, dithering as the
    /// card does with `dither` (`voodoo_dither`). The error says why OpenGL
    /// can't; whether the picture may have changed.
    pub fn run_voodoo(
        &mut self,
        recording: rust_dos::voodoo::mirror::Frame,
        scale: u32,
        samples: u32,
        anisotropy: u32,
        dither: bool,
    ) -> Result<bool, String> {
        if self.voodoo_scale().is_some_and(|now| now != (scale, samples, anisotropy)) {
            self.drop_voodoo();
        }
        if self.voodoo.borrow().is_none() {
            if let Some(problem) = &self.voodoo_failed {
                return Err(problem.clone());
            }
            match VoodooGl::new(&self.gl, self.glsl, scale, samples, anisotropy) {
                Ok(voodoo) => *self.voodoo.borrow_mut() = Some(voodoo),
                Err(e) => {
                    eprintln!("[DISPLAY] {}", e);
                    self.voodoo_failed = Some(e.clone());
                    return Err(e);
                }
            }
        }
        // A Banshee's picture needs its compositor, which may not compile
        // here: the software renderer draws it from then on.
        let problem = recording
            .scanout
            .as_ref()
            .and_then(|_| self.voodoo.borrow().as_ref()?.banshee_problem().map(str::to_string));
        if let Some(problem) = problem {
            eprintln!("[DISPLAY] {}", problem);
            self.voodoo_failed = Some(problem.clone());
            self.drop_voodoo();
            return Err(problem);
        }
        // Without the 2D operations' program, a Banshee does them in
        // software, which the log says the first time it draws.
        if recording.scanout.is_some() && !self.gpu_2d_logged {
            let voodoo = self.voodoo.borrow();
            if let Some(problem) = voodoo.as_ref().and_then(|v| v.gpu_2d_problem()) {
                eprintln!("[DISPLAY] 2D operations stay on the CPU: {problem}");
                self.gpu_2d_logged = true;
            }
        }
        Ok(self.voodoo.borrow_mut().as_mut().is_some_and(|voodoo| {
            voodoo.set_dither(dither);
            voodoo.run(&self.gl, recording)
        }))
    }

    /// The 3dfx card's picture as OpenGL last composed it, read back at
    /// the card's resolution (`VoodooGl::read_picture`).
    pub fn voodoo_picture(&mut self) -> Option<Frame> {
        self.voodoo.borrow().as_ref()?.read_picture(&self.gl)
    }

    /// Show the 3dfx card's picture, with what `screen` has over `base`
    /// (the settings window, messages) on it, letterboxed at `display`
    /// proportions. False if it has none to show.
    pub fn present_voodoo(&mut self, screen: &Frame, base: &Frame, display: (u32, u32)) -> bool {
        let composed = self.voodoo.borrow_mut().as_mut().and_then(|voodoo| {
            let (texture, size) = voodoo.composite(&self.gl, screen, base)?;
            Some((texture, size, voodoo.scale()))
        });
        let Some((texture, size, scale)) = composed else { return false };
        self.xbrz_frame.invalidate();
        let line = shader::line_scale(self.scale_lines.then_some(scale));
        // SAFETY: see `GlScreen`.
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            self.set_filter();
            if shader::needs_mipmaps(self.active) {
                self.gl.generate_mipmap(glow::TEXTURE_2D);
            }
        }
        self.draw(texture, size, display, None, line);
        true
    }

    /// Draw `texture`, a picture of `size` pixels, into the window with the
    /// look, letterboxed at `display` proportions, and show it. With the 3D
    /// scene, it goes on the scene's screen instead. `line` is the texels a
    /// CRT line spans (`u_line`).
    fn draw(&mut self, texture: glow::Texture, size: (u32, u32), display: (u32, u32), layer: Option<&Layer>, line: f32) {
        #[cfg(feature = "vr")]
        if let Some(stage) = &mut self.stage {
            let target = stage.screen_size(display);
            if stage.begin_screen(&self.gl, target) {
                self.flat = true;
                self.compose(texture, size, display, layer, target, line);
                self.flat = false;
                if let Some(stage) = &mut self.stage {
                    stage.end_screen(&self.gl);
                }
            }
            // SAFETY: see `GlScreen`.
            unsafe { self.gl.bind_framebuffer(glow::FRAMEBUFFER, None) };
            self.render_stage();
            return;
        }
        let drawable = self.window.drawable_size();
        self.compose(texture, size, display, layer, drawable, line);
        self.window.gl_swap_window();
    }

    /// Draw `texture` as `draw` does into the framebuffer bound, of
    /// `(dw, dh)` pixels.
    fn compose(&mut self, texture: glow::Texture, size: (u32, u32), display: (u32, u32), layer: Option<&Layer>, (dw, dh): (u32, u32), line: f32) {
        let gl = &self.gl;
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.viewport(0, 0, dw as i32, dh as i32);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
        if dw > 0 && dh > 0 {
            let (x, y, w, h) = super::letterbox((dw, dh), display);
            // OpenGL counts rows from the bottom.
            self.draw_look(texture, size, (x as i32, (dh - y - h) as i32, w, h), line);
            if let Some(layer) = layer {
                let scale = self.texture_scale as f32;
                let (sx, sy) = (
                    w as f32 / (size.0.max(1) as f32 * scale),
                    h as f32 / (size.1.max(1) as f32 * scale),
                );
                let (lx, ly, lw, lh) = layer.rect;
                let top = y as f32 + ly * sy;
                let at = ((x as f32 + lx * sx) as i32, (dh as f32 - top - lh * sy) as i32);
                self.draw_layer(layer, (at.0, at.1, (lw * sx) as u32, (lh * sy) as u32));
            }
        }
    }

    /// Draw `layer` as it is, smoothly scaled, into the `x, y, width,
    /// height` of the framebuffer (y from the bottom).
    fn draw_layer(&mut self, layer: &Layer, rect: (i32, i32, u32, u32)) {
        let gl = &self.gl;
        let Some(Ok(plain)) = self.programs.get(&Shader::None) else { return };
        let picture = &layer.picture;
        // SAFETY: see `GlScreen`.
        unsafe {
            let texture = match self.layer {
                Some(texture) => texture,
                None => match gl.create_texture() {
                    Ok(texture) => {
                        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                        for (name, value) in [
                            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                            (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                            (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                        ] {
                            gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
                        }
                        self.layer = Some(texture);
                        texture
                    }
                    Err(_) => return,
                },
            };
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            if self.layer_generation != Some(layer.generation) {
                let pixels = glow::PixelUnpackData::Slice(Some(&picture.rgba));
                let (w, h) = (picture.width as i32, picture.height as i32);
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0, glow::RGBA, glow::UNSIGNED_BYTE, pixels);
                self.layer_generation = Some(layer.generation);
            }
            let (x, y, w, h) = rect;
            gl.viewport(x, y, w as i32, h as i32);
            // The full-screen triangle can extend beyond the viewport;
            // clip it to the layer rectangle to prevent bleed.
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(x, y, w as i32, h as i32);
            gl.use_program(Some(plain.program));
            gl.uniform_2_f32(plain.source.as_ref(), picture.width as f32, picture.height as f32);
            gl.uniform_2_f32(plain.output.as_ref(), w as f32, h as f32);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.disable(glow::SCISSOR_TEST);
            // The look's own texture, as `select` left it.
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
        }
    }

    /// Draw `texture`, a picture of `size` pixels, with the look into the
    /// `x, y, width, height` of the framebuffer bound (y from the bottom).
    fn draw_look(
        &mut self,
        texture: glow::Texture,
        size: (u32, u32),
        (x, y, w, h): (i32, i32, u32, u32),
        line: f32,
    ) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        if self.active == Shader::Xbrz
            && w >= size.0
            && h >= size.1
            && let Some(Ok(cache)) = &mut self.xbrz_gpu
            && cache.supports(size)
        {
            // The 3dfx compositor can repaint its texture without an upload.
            if texture != self.texture {
                cache.invalidate();
            }
            // SAFETY: this screen owns the context and empty VAO.
            unsafe { self.gl.bind_vertex_array(Some(self.vao)) };
            let overlay = self.xbrz_overlay.filter(|_| self.xbrz_overlay_visible);
            match cache.draw(&self.gl, texture, size, (x, y, w, h), overlay) {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("[DISPLAY] xBRZ cache unavailable: {error}; using the direct shader");
                    if let Some(Ok(cache)) = self.xbrz_gpu.take() {
                        cache.destroy(&self.gl);
                    }
                    self.xbrz_gpu = Some(Err(error));
                }
            }
        }
        let gl = &self.gl;
        // `select` compiled the active look.
        let Some(Ok(program)) = self.programs.get(&self.active) else { return };
        // SAFETY: see `GlScreen`.
        unsafe {
            gl.viewport(x, y, w as i32, h as i32);
            gl.use_program(Some(program.program));
            gl.uniform_2_f32(program.source.as_ref(), size.0 as f32, size.1 as f32);
            gl.uniform_2_f32(program.output.as_ref(), w as f32, h as f32);
            gl.uniform_1_f32(program.mask.as_ref(), self.mask);
            let [cx, cy] = if self.flat { [0.0, 0.0] } else { self.active.curvature(self.crt) };
            gl.uniform_2_f32(program.curvature.as_ref(), cx, cy);
            gl.uniform_1_f32(program.glow.as_ref(), self.active.glow(self.crt));
            gl.uniform_1_f32(program.line.as_ref(), line);
            gl.uniform_1_i32(program.overlay_enabled.as_ref(), self.xbrz_overlay_visible as i32);
            if self.xbrz_overlay_visible {
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, self.xbrz_overlay);
            }
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
    }

    /// Show the picture in the 3D scene of `settings`, from now on. What is
    /// worth saying about it: the scene or headset that can't be had.
    #[cfg(feature = "vr")]
    pub fn open_stage(&mut self, settings: &rust_dos::vr::VrSettings) -> Result<Vec<String>, String> {
        let (stage, notes) = super::stage::Stage::new(&self.gl, self.glsl, settings, &self.window, &self._context)?;
        self.stage = Some(Box::new(stage));
        Ok(notes)
    }

    /// Take on the `[vr]` settings: the scene shown, closed with `mode`
    /// off, or changed. What is worth saying about it.
    #[cfg(feature = "vr")]
    pub fn apply_stage(&mut self, settings: &rust_dos::vr::VrSettings) -> Vec<String> {
        if settings.mode == rust_dos::vr::VrMode::Off {
            if let Some(stage) = self.stage.take() {
                stage.close(&self.gl);
            }
            return Vec::new();
        }
        match &mut self.stage {
            Some(stage) => stage.apply(&self.gl, settings, &self.window, &self._context),
            None => self.open_stage(settings).unwrap_or_else(|e| vec![format!("[VR] No 3D scene: {}", e)]),
        }
    }

    /// The 3D scene's news: the headset's, and a scene read since, which
    /// takes the place of the one shown.
    #[cfg(feature = "vr")]
    pub fn poll_stage(&mut self) -> Vec<String> {
        match &mut self.stage {
            Some(stage) => stage.poll(&self.gl),
            None => Vec::new(),
        }
    }

    #[cfg(feature = "vr")]
    pub fn stage(&self) -> Option<&super::stage::Stage> {
        self.stage.as_deref()
    }

    #[cfg(feature = "vr")]
    pub fn stage_mut(&mut self) -> Option<&mut super::stage::Stage> {
        self.stage.as_deref_mut()
    }

    /// The pictures the VFX1's eyes see, to the headset.
    #[cfg(feature = "vr")]
    pub fn show_vfx1_eyes(&mut self, left: &Frame, right: &Frame, width: f32) {
        if let Some(stage) = &mut self.stage {
            stage.show_eyes(&self.gl, left, right, width);
        }
    }

    /// Draw the 3D scene again, with the picture its screen has, and show
    /// it: the viewer may have moved.
    #[cfg(feature = "vr")]
    pub fn render_stage(&mut self) {
        let drawable = self.window.drawable_size();
        if let Some(stage) = &mut self.stage {
            stage.render(&self.gl, drawable);
            self.window.gl_swap_window();
        }
    }

    /// The size the window shows a picture of `display` proportions at,
    /// without the black bars around it, if it shows it through a look.
    pub fn capture_size(&self, display: (u32, u32)) -> Option<(u32, u32)> {
        let (_, _, w, h) = super::letterbox(self.window.drawable_size(), display);
        (self.active != Shader::None && w > 0 && h > 0).then_some((w, h))
    }

    /// `frame` drawn with the look as the window shows a picture of
    /// `display` proportions, at the size it shows it (`capture_size`),
    /// for a screenshot or a recording. None without a look, or if OpenGL
    /// can't draw it away from the window. It costs drawing the picture
    /// again and reading it back, so only captures ask for it.
    pub fn capture(
        &mut self,
        frame: &Frame,
        base: &Frame,
        mono: Monochrome,
        display: (u32, u32),
    ) -> Option<Frame> {
        let (w, h) = self.capture_size(display)?;
        if self.capture.is_none() {
            match self.make_capture() {
                Ok(capture) => self.capture = Some(capture),
                Err(e) => eprintln!("[DISPLAY] Captures can't show the shader: {}", e),
            }
        }
        self.capture.as_ref()?;
        let native = self.active == Shader::Xbrz;
        if native {
            let prepared =
                self.prepare_xbrz(frame, Some(base), (0..frame.height as usize).into(), mono);
            // Captures may add overlays outside the producer's next band.
            self.xbrz_frame.invalidate();
            prepared.ok()?;
        }
        let mut capture = self.capture.take()?;
        let gl = &self.gl;
        capture.rgba.resize(w as usize * h as usize * 4, 0);
        // SAFETY: see `GlScreen`.
        let complete = unsafe {
            let rgba8 = glow::RGBA8 as i32;
            if !native {
                gl.bind_texture(glow::TEXTURE_2D, Some(capture.source));
                let pixels = glow::PixelUnpackData::Slice(Some(&frame.rgba));
                let (fw, fh) = (frame.width as i32, frame.height as i32);
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    rgba8,
                    fw,
                    fh,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    pixels,
                );
                self.set_filter();
                if shader::needs_mipmaps(self.active) {
                    gl.generate_mipmap(glow::TEXTURE_2D);
                }
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(capture.framebuffer));
            if capture.size != (w, h) {
                gl.bind_texture(glow::TEXTURE_2D, Some(capture.target));
                let none = glow::PixelUnpackData::Slice(None);
                let (tw, th) = (w as i32, h as i32);
                gl.tex_image_2d(glow::TEXTURE_2D, 0, rgba8, tw, th, 0, glow::RGBA, glow::UNSIGNED_BYTE, none);
                let target = Some(capture.target);
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, target, 0);
                capture.size = (w, h);
            }
            let complete = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            if complete {
                let (texture, size) = if native {
                    (self.texture, self.texture_size)
                } else {
                    (capture.source, (frame.width, frame.height))
                };
                self.draw_look(texture, size, (0, 0, w, h), 1.0);
                self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
                let pixels = glow::PixelPackData::Slice(Some(&mut capture.rgba));
                self.gl.read_pixels(
                    0, 0, w as i32, h as i32, glow::RGBA, glow::UNSIGNED_BYTE, pixels,
                );
            }
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            complete
        };
        self.xbrz_overlay_visible = false;
        // Rows from the top, four bytes a pixel.
        let shaded = complete.then(|| {
            let mut frame = Frame::new(w, h);
            let rows = frame.rgba.chunks_exact_mut(w as usize * 4)
                .zip(capture.rgba.chunks_exact(w as usize * 4).rev());
            for (to, from) in rows {
                for (out, pixel) in to.as_chunks_mut::<4>().0.iter_mut()
                    .zip(from.as_chunks::<4>().0)
                {
                    *out = [pixel[0], pixel[1], pixel[2], 255];
                }
            }
            frame
        });
        self.capture = Some(capture);
        shaded
    }

    /// The textures and framebuffer of captures of the look.
    fn make_capture(&self) -> Result<Capture, String> {
        let gl = &self.gl;
        // SAFETY: see `GlScreen`.
        unsafe {
            let source = gl.create_texture()?;
            let target = gl.create_texture()?;
            for texture in [source, target] {
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            }
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
            let framebuffer = gl.create_framebuffer()?;
            Ok(Capture { source, target, framebuffer, size: (0, 0), rgba: Vec::new() })
        }
    }
}

/// Compile and link a look. The error is the first line of the log; the
/// whole log goes to the terminal.
fn compile(gl: &glow::Context, glsl: Glsl, shader: Shader) -> Result<Program, String> {
    compile_sources(gl, glsl, shader.name(), shader::sources(shader, glsl))
}

fn compile_sources(
    gl: &glow::Context,
    glsl: Glsl,
    name: &str,
    (vertex, fragment): (String, String),
) -> Result<Program, String> {
    let failed = |what: &str, log: String| {
        eprintln!("[DISPLAY] The {name} shader {what}:\n{log}");
        let first = log.lines().find(|line| !line.trim().is_empty()).unwrap_or("no log");
        format!("{} ({})", what, first.trim())
    };
    // SAFETY: see `GlScreen`.
    unsafe {
        let program = gl.create_program()?;
        let mut stages = Vec::new();
        let mut problem = None;
        for (kind, source) in [(glow::VERTEX_SHADER, vertex), (glow::FRAGMENT_SHADER, fragment)] {
            let stage = match gl.create_shader(kind) {
                Ok(stage) => stage,
                Err(error) => {
                    problem = Some(error);
                    break;
                }
            };
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
        gl.uniform_1_i32(gl.get_uniform_location(program, "u_overlay").as_ref(), 1);
        gl.uniform_1_i32(gl.get_uniform_location(program, "u_corners").as_ref(), 2);
        Ok(Program {
            program,
            source: gl.get_uniform_location(program, "u_source"),
            output: gl.get_uniform_location(program, "u_output"),
            mask: gl.get_uniform_location(program, "u_mask"),
            curvature: gl.get_uniform_location(program, "u_curvature"),
            glow: gl.get_uniform_location(program, "u_glow"),
            line: gl.get_uniform_location(program, "u_line"),
            overlay_enabled: gl.get_uniform_location(program, "u_overlay_enabled"),
            unflipped: gl.get_uniform_location(program, "u_unflipped"),
        })
    }
}
