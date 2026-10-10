//! The 3dfx card drawn again with OpenGL at `voodoo_scale` times its
//! resolution, for the window and the libretro core's hardware rendering
//! (`voodoo_renderer=opengl`). The card records what it draws
//! (`super::mirror`); here each of its colour buffers is the texture of a
//! framebuffer object, all sharing one depth texture, the triangles go
//! through the card's pixel pipeline in a shader (gl/triangle.frag) with
//! OpenGL's depth test and blending, and the front buffer through the
//! card's gamma table is the picture shown. With `voodoo_msaa`, the buffers
//! are multisampled and resolved into their textures before they are shown.
//! The software rasterizer's memory stays what screenshots, recordings and
//! the debugger see, and what the game reads back; it only draws what those
//! can still see (`super::backlog`).

use glow::HasContext;
use crate::video::Frame;
use crate::video::shader::Glsl;
use super::mirror::{Command, Draw, Fill, Frame as Recording, Pixels, Snapshot, Texture, Vertex};
use std::collections::HashMap;

const TRIANGLE_VERT: &str = include_str!("gl/triangle.vert");
const TRIANGLE_FRAG: &str = include_str!("gl/triangle.frag");
const QUAD_VERT: &str = include_str!("gl/quad.vert");
const PIXELS_FRAG: &str = include_str!("gl/pixels.frag");
const DEPTH_FRAG: &str = include_str!("gl/depth.frag");
const CLUT_FRAG: &str = include_str!("gl/clut.frag");

const TRIANGLE_UNIFORMS: &[&str] = &[
    "u_size", "u_fbzcp", "u_fbz", "u_alpha", "u_fog", "u_color0", "u_color1", "u_chroma", "u_zacolor", "u_fogcolor",
    "u_stipple", "u_yorigin", "u_fogblend", "u_fogdelta", "u_scale", "u_units", "u_config", "u_tmode0", "u_tmode1",
    "u_tsize0", "u_tsize1", "u_tlod0", "u_tlod1", "u_tdetail0", "u_tdetail1", "u_constant_depth",
];
const QUAD_UNIFORMS: &[&str] = &["u_rect", "u_size", "u_scale"];

/// The card's depth functions as OpenGL's.
const DEPTH_FUNCS: [u32; 8] =
    [glow::NEVER, glow::LESS, glow::EQUAL, glow::LEQUAL, glow::GREATER, glow::NOTEQUAL, glow::GEQUAL, glow::ALWAYS];
const TEXTURE_MAX_ANISOTROPY: u32 = 0x84FE;
const MAX_TEXTURE_MAX_ANISOTROPY: u32 = 0x84FF;

/// A linked program and its uniforms.
struct Program {
    program: glow::Program,
    uniforms: HashMap<&'static str, glow::UniformLocation>,
}

impl Program {
    fn at(&self, name: &str) -> Option<&glow::UniformLocation> {
        self.uniforms.get(name)
    }
}

/// A colour buffer: its texture and the framebuffer object drawing into
/// it, or with multisampling, the one with the samples that is resolved
/// into it.
struct Target {
    fbo: glow::Framebuffer,
    color: glow::Texture,
    samples: Option<(glow::Framebuffer, glow::Renderbuffer)>,
}

impl Target {
    /// The framebuffer to draw into.
    fn draw_fbo(&self) -> glow::Framebuffer {
        self.samples.map_or(self.fbo, |(fbo, _)| fbo)
    }

    /// Resolve the samples into the texture.
    fn resolve(&self, gl: &glow::Context, (w, h): (i32, i32)) {
        let Some((samples, _)) = self.samples else { return };
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(samples));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.fbo));
            gl.blit_framebuffer(0, 0, w, h, 0, 0, w, h, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }

    fn delete(self, gl: &glow::Context) {
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.delete_framebuffer(self.fbo);
            gl.delete_texture(self.color);
            if let Some((fbo, buffer)) = self.samples {
                gl.delete_framebuffer(fbo);
                gl.delete_renderbuffer(buffer);
            }
        }
    }
}

/// The depth buffer the colour buffers share: a texture, or with
/// multisampling a renderbuffer of samples.
#[derive(Clone, Copy)]
enum Depth {
    Texture(glow::Texture),
    Samples(glow::Renderbuffer),
}

/// Pixels frame buffer writes left, waiting to go into a buffer: RGBA of
/// the card's size, alpha where there is one, and the rectangle they are
/// in (left, right, top, bottom).
struct Staging {
    rgba: Vec<u8>,
    dirty: Option<[u32; 4]>,
}

/// Unfiltered texels can still blend mip levels when supported anisotropy
/// is enabled, while magnification remains nearest-neighbour.
fn sampling_filters(mode: u32, unfiltered: bool, anisotropy: Option<f32>) -> (u32, u32) {
    if unfiltered {
        let min = if anisotropy.is_some_and(|n| n > 1.0) {
            glow::NEAREST_MIPMAP_LINEAR
        } else {
            glow::NEAREST_MIPMAP_NEAREST
        };
        (min, glow::NEAREST)
    } else {
        let min = if mode & 2 != 0 { glow::LINEAR_MIPMAP_NEAREST } else { glow::NEAREST_MIPMAP_NEAREST };
        let mag = if mode & 4 != 0 { glow::LINEAR } else { glow::NEAREST };
        (min, mag)
    }
}

/// A texture of the card's, and the sampling it was last set up for.
struct GlTexture {
    texture: glow::Texture,
    params: Option<[u32; 4]>,
    lod: Option<[f32; 2]>,
    anisotropy: Option<f32>,
}

// Every `unsafe` below is a call into OpenGL on the context `GlScreen`
// keeps current.
pub struct VoodooGl {
    scale: u32,
    /// Samples a pixel with multisampling, or 1.
    samples: u32,
    /// The requested anisotropy factor.
    anisotropy: u32,
    /// The effective factor when the OpenGL extension is available.
    texture_anisotropy: Option<f32>,
    /// The buffers' size in the card's pixels.
    width: u32,
    height: u32,
    /// The colour buffers by word offset in the card's memory, and the
    /// depth texture they share if the card has an auxiliary buffer.
    targets: HashMap<u32, Target>,
    depth: Option<Depth>,
    /// Frame buffer writes waiting: a colour buffer's, or with None the
    /// auxiliary buffer's.
    staging: HashMap<Option<u32>, Staging>,
    staging_texture: glow::Texture,
    textures: HashMap<u32, GlTexture>,
    /// The gamma table as 256 entries a component, and the table it is.
    lut: glow::Texture,
    lut_of: Option<[u32; 33]>,
    /// The picture shown, and its size.
    composite: Option<(Target, (u32, u32))>,
    triangle: Program,
    pixels: Program,
    depth_program: Program,
    clut_program: Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    /// No attributes: the quads make their corners from the vertex number.
    quad_vao: glow::VertexArray,
    /// What the card shows: its front buffer, and its gamma table.
    front: Option<u32>,
    clut: [u32; 33],
}

impl VoodooGl {
    /// Draw at `scale` times the card's size with `samples` a pixel (as
    /// many as OpenGL has at most), and the requested texture anisotropy.
    pub fn new(
        gl: &glow::Context,
        glsl: Glsl,
        scale: u32,
        samples: u32,
        anisotropy: u32,
    ) -> Result<Self, String> {
        if glsl == Glsl::Es300 {
            return Err("OpenGL ES has no noperspective interpolation".to_string());
        }
        let attributes = [(0, "a_pos"), (1, "a_color"), (2, "a_zw"), (3, "a_tex0"), (4, "a_tex1")];
        let triangle = compile(gl, glsl, TRIANGLE_VERT, TRIANGLE_FRAG, &attributes, TRIANGLE_UNIFORMS)?;
        let pixels = compile(gl, glsl, QUAD_VERT, PIXELS_FRAG, &[], QUAD_UNIFORMS)?;
        let depth_program = compile(gl, glsl, QUAD_VERT, DEPTH_FRAG, &[], QUAD_UNIFORMS)?;
        let clut_program = compile(gl, glsl, QUAD_VERT, CLUT_FRAG, &[], QUAD_UNIFORMS)?;
        // SAFETY: see `VoodooGl`.
        unsafe {
            for (program, samplers) in [
                (&triangle, &[("u_tex0", 1), ("u_tex1", 2)][..]),
                (&pixels, &[("u_src", 0)][..]),
                (&depth_program, &[("u_src", 0)][..]),
                (&clut_program, &[("u_src", 0), ("u_lut", 1)][..]),
            ] {
                gl.use_program(Some(program.program));
                for &(name, unit) in samplers {
                    gl.uniform_1_i32(gl.get_uniform_location(program.program, name).as_ref(), unit);
                }
            }
            gl.use_program(None);

            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let stride = std::mem::size_of::<Vertex>() as i32;
            for (index, size, offset) in [(0, 2, 0), (1, 4, 8), (2, 2, 24), (3, 3, 32), (4, 3, 44)] {
                gl.enable_vertex_attrib_array(index);
                gl.vertex_attrib_pointer_f32(index, size, glow::FLOAT, false, stride, offset);
            }
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            let quad_vao = gl.create_vertex_array()?;
            let staging_texture = texture(gl, glow::NEAREST)?;
            let lut = texture(gl, glow::NEAREST)?;
            let samples = samples.clamp(1, gl.get_parameter_i32(glow::MAX_SAMPLES).max(1) as u32);
            let extensions = gl.supported_extensions();
            let texture_anisotropy = (anisotropy > 1
                && extensions
                    .iter()
                    .any(|e| e == "GL_EXT_texture_filter_anisotropic" || e == "GL_ARB_texture_filter_anisotropic"))
            .then(|| {
                // SAFETY: see `VoodooGl`.
                (anisotropy as f32).min(gl.get_parameter_f32(MAX_TEXTURE_MAX_ANISOTROPY))
            });
            Ok(Self {
                scale: scale.max(1),
                samples,
                anisotropy,
                texture_anisotropy,
                width: 0,
                height: 0,
                targets: HashMap::new(),
                depth: None,
                staging: HashMap::new(),
                staging_texture,
                textures: HashMap::new(),
                lut,
                lut_of: None,
                composite: None,
                triangle,
                pixels,
                depth_program,
                clut_program,
                vao,
                vbo,
                quad_vao,
                front: None,
                clut: [0; 33],
            })
        }
    }

    pub fn scale(&self) -> u32 {
        self.scale
    }

    /// The samples a pixel asked for, which OpenGL may have fewer of.
    pub fn samples(&self) -> u32 {
        self.samples
    }

    pub fn anisotropy(&self) -> u32 {
        self.anisotropy
    }

    /// Give everything back to OpenGL.
    pub fn destroy(mut self, gl: &glow::Context) {
        self.drop_targets(gl);
        // SAFETY: see `VoodooGl`.
        unsafe {
            for (_, t) in self.textures.drain() {
                gl.delete_texture(t.texture);
            }
            if let Some((target, _)) = self.composite.take() {
                target.delete(gl);
            }
            gl.delete_texture(self.staging_texture);
            gl.delete_texture(self.lut);
            for program in [&self.triangle, &self.pixels, &self.depth_program, &self.clut_program] {
                gl.delete_program(program.program);
            }
            gl.delete_vertex_array(self.vao);
            gl.delete_vertex_array(self.quad_vao);
            gl.delete_buffer(self.vbo);
        }
    }

    fn drop_targets(&mut self, gl: &glow::Context) {
        // SAFETY: see `VoodooGl`.
        unsafe {
            for (_, target) in self.targets.drain() {
                target.delete(gl);
            }
            match self.depth.take() {
                Some(Depth::Texture(depth)) => gl.delete_texture(depth),
                Some(Depth::Samples(depth)) => gl.delete_renderbuffer(depth),
                None => {}
            }
        }
    }

    /// Draw `recording`; whether the picture may have changed with it.
    pub fn run(&mut self, gl: &glow::Context, recording: Recording) -> bool {
        let changed = !recording.commands.is_empty() || self.front != recording.front || self.clut != recording.clut;
        for command in &recording.commands {
            match command {
                Command::Resync(snapshot) => self.resync(gl, snapshot),
                Command::Draw(draw) => {
                    self.flush_staging(gl, Some(draw.state.dest));
                    self.flush_staging(gl, None);
                    self.draw(gl, draw);
                }
                Command::Fill(fill) => {
                    self.flush_staging(gl, Some(fill.dest));
                    self.flush_staging(gl, None);
                    self.fill(gl, fill);
                }
                Command::Pixels(pixels) => self.stage(pixels),
                Command::Texture(texture) => self.upload(gl, texture),
                Command::FreeTexture(id) => {
                    if let Some(t) = self.textures.remove(id) {
                        // SAFETY: see `VoodooGl`.
                        unsafe { gl.delete_texture(t.texture) };
                    }
                }
            }
        }
        let keys: Vec<_> = self.staging.keys().copied().collect();
        for key in keys {
            self.flush_staging(gl, key);
        }
        self.front = recording.front;
        self.clut = recording.clut;
        restore(gl);
        changed
    }

    /// The buffers as they are in the card's memory, in its layout.
    fn resync(&mut self, gl: &glow::Context, snapshot: &Snapshot) {
        self.drop_targets(gl);
        self.staging.clear();
        let layout = &snapshot.layout;
        (self.width, self.height) = (layout.width.max(1), layout.height.max(1));
        let (w, h) = self.scaled();
        // SAFETY: see `VoodooGl`.
        unsafe {
            self.depth = layout.aux.and_then(|_| {
                if self.samples > 1 {
                    let depth = gl.create_renderbuffer().ok()?;
                    gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth));
                    let samples = self.samples as i32;
                    gl.renderbuffer_storage_multisample(glow::RENDERBUFFER, samples, glow::DEPTH24_STENCIL8, w, h);
                    gl.bind_renderbuffer(glow::RENDERBUFFER, None);
                    return Some(Depth::Samples(depth));
                }
                let depth = texture(gl, glow::NEAREST).ok()?;
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::DEPTH24_STENCIL8 as i32,
                    w,
                    h,
                    0,
                    glow::DEPTH_STENCIL,
                    glow::UNSIGNED_INT_24_8,
                    glow::PixelUnpackData::Slice(None),
                );
                Some(Depth::Texture(depth))
            });
            gl.disable(glow::SCISSOR_TEST);
            gl.color_mask(true, true, true, true);
            gl.depth_mask(true);
            for &offs in &layout.color {
                if self.targets.contains_key(&offs) {
                    continue;
                }
                if let Ok(target) = target(gl, (w, h), self.samples, self.depth) {
                    gl.clear_color(0.0, 0.0, 0.0, 1.0);
                    gl.clear_depth_f64(1.0);
                    gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
                    self.targets.insert(offs, target);
                }
            }
            gl.bind_texture(glow::TEXTURE_2D, Some(self.staging_texture));
            let none = glow::PixelUnpackData::Slice(None);
            let (w, h) = (self.width as i32, self.height as i32);
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0, glow::RGBA, glow::UNSIGNED_BYTE, none);
        }
        for (&offs, pixels) in layout.color.iter().zip(&snapshot.color) {
            self.stage_rows(Some(offs), pixels);
            self.flush_staging(gl, Some(offs));
        }
        if let Some(aux) = &snapshot.aux {
            self.stage_rows(None, aux);
            self.flush_staging(gl, None);
        }
    }

    fn scaled(&self) -> (i32, i32) {
        ((self.width * self.scale) as i32, (self.height * self.scale) as i32)
    }

    /// The staging area of a buffer.
    fn staging(&mut self, key: Option<u32>) -> &mut Staging {
        let size = (self.width * self.height * 4) as usize;
        self.staging.entry(key).or_insert_with(|| Staging { rgba: vec![0; size], dirty: None })
    }

    /// The whole of a buffer's pixels, `width * height` of them.
    fn stage_rows(&mut self, key: Option<u32>, pixels: &[u16]) {
        let (w, h) = (self.width, self.height);
        let staging = self.staging(key);
        for (out, &value) in staging.rgba.chunks_exact_mut(4).zip(pixels) {
            out.copy_from_slice(&rgba(key, value));
        }
        staging.dirty = Some([0, w, 0, h]);
    }

    fn stage(&mut self, pixels: &Pixels) {
        let (w, h) = (self.width, self.height);
        if pixels.y >= h || pixels.x >= w {
            return;
        }
        let key = pixels.dest;
        let x1 = (pixels.x + pixels.values.len() as u32).min(w);
        let staging = self.staging(key);
        let row = (pixels.y * w) as usize;
        for (x, &value) in (pixels.x..x1).zip(&pixels.values) {
            let at = (row + x as usize) * 4;
            staging.rgba[at..at + 4].copy_from_slice(&rgba(key, value));
        }
        let rect = [pixels.x, x1, pixels.y, pixels.y + 1];
        staging.dirty = Some(match staging.dirty {
            None => rect,
            Some([l, r, t, b]) => [l.min(rect[0]), r.max(rect[1]), t.min(rect[2]), b.max(rect[3])],
        });
    }

    /// Draw the staged pixels of a buffer into it.
    fn flush_staging(&mut self, gl: &glow::Context, key: Option<u32>) {
        let w = self.width;
        let scale = self.scale;
        let fbo = match key {
            Some(offs) => self.targets.get(&offs).map(Target::draw_fbo),
            None => self.depth.and(self.targets.values().next().map(Target::draw_fbo)),
        };
        let Some(staging) = self.staging.get_mut(&key) else { return };
        let Some([x0, x1, y0, y1]) = staging.dirty.take() else { return };
        let Some(fbo) = fbo else { return };
        let program = if key.is_some() { &self.pixels } else { &self.depth_program };
        let (sw, sh) = ((self.width * scale) as i32, (self.height * scale) as i32);
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.staging_texture));
            let rows = &staging.rgba[(y0 * w * 4) as usize..(y1 * w * 4) as usize];
            let pixels = glow::PixelUnpackData::Slice(Some(rows));
            let (y, h) = (y0 as i32, (y1 - y0) as i32);
            gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, y, w as i32, h, glow::RGBA, glow::UNSIGNED_BYTE, pixels);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.viewport(0, 0, sw, sh);
            gl.disable(glow::BLEND);
            gl.disable(glow::SCISSOR_TEST);
            if key.is_some() {
                gl.disable(glow::DEPTH_TEST);
                gl.color_mask(true, true, true, false);
            } else {
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(glow::ALWAYS);
                gl.depth_mask(true);
                gl.color_mask(false, false, false, false);
            }
            gl.use_program(Some(program.program));
            gl.uniform_4_f32(program.at("u_rect"), x0 as f32, y0 as f32, x1 as f32, y1 as f32);
            gl.uniform_2_f32(program.at("u_size"), self.width as f32, self.height as f32);
            gl.uniform_1_f32(program.at("u_scale"), scale as f32);
            gl.bind_vertex_array(Some(self.quad_vao));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        }
        for y in y0..y1 {
            for x in x0..x1 {
                staging.rgba[((y * w + x) * 4 + 3) as usize] = 0;
            }
        }
    }

    fn upload(&mut self, gl: &glow::Context, texture: &Texture) {
        if !self.textures.contains_key(&texture.id) {
            let Ok(t) = self::texture(gl, glow::NEAREST) else { return };
            self.textures.insert(texture.id, GlTexture { texture: t, params: None, lod: None, anisotropy: None });
        }
        let Some(entry) = self.textures.get_mut(&texture.id) else { return };
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(entry.texture));
            for (i, level) in texture.levels.iter().enumerate() {
                let bytes: Vec<u8> =
                    level.argb.iter().flat_map(|&c| [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]).collect();
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    i as i32,
                    glow::RGBA8 as i32,
                    level.width as i32,
                    level.height as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&bytes)),
                );
            }
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_BASE_LEVEL, 0);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, texture.levels.len() as i32 - 1);
            entry.params = None;
            entry.lod = None;
        }
    }

    fn draw(&mut self, gl: &glow::Context, draw: &Draw) {
        let s = &draw.state;
        let Some(target) = self.targets.get(&s.dest) else { return };
        let fbz = s.fbz_mode;
        let depth_test = fbz & (1 << 4) != 0;
        let func = ((fbz >> 5) & 7) as usize;
        if depth_test && func == 0 {
            return;
        }
        let alpha_planes = fbz & (1 << 18) != 0;
        let has_depth = s.aux && !alpha_planes && self.depth.is_some();
        let depth_write = has_depth && fbz & (1 << 10) != 0;
        let constant_depth = has_depth && depth_test && fbz & (1 << 20) != 0;
        let rgb = fbz & (1 << 9) != 0;
        let (w, h) = self.scaled();
        let scale = self.scale as f32;
        let p = &self.triangle;
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.draw_fbo()));
            gl.viewport(0, 0, w, h);
            self.clip(gl, s.clip);
            gl.disable(glow::STENCIL_TEST);
            if has_depth && (depth_test || depth_write) {
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(if depth_test { DEPTH_FUNCS[func] } else { glow::ALWAYS });
                gl.depth_mask(depth_write);
            } else {
                gl.disable(glow::DEPTH_TEST);
            }
            gl.color_mask(rgb, rgb, rgb, alpha_planes && fbz & (1 << 10) != 0);
            let am = s.alpha_mode;
            if am & (1 << 4) != 0 {
                let alpha = |f: u32| if f == 4 { glow::ONE } else { glow::ZERO };
                gl.enable(glow::BLEND);
                gl.blend_func_separate(
                    source_factor((am >> 8) & 15),
                    dest_factor((am >> 12) & 15),
                    alpha((am >> 16) & 15),
                    alpha((am >> 20) & 15),
                );
            } else {
                gl.disable(glow::BLEND);
            }

            gl.use_program(Some(p.program));
            gl.uniform_2_f32(p.at("u_size"), self.width as f32, self.height as f32);
            for (name, value) in [
                ("u_fbzcp", s.fbz_color_path),
                ("u_fbz", s.fbz_mode),
                ("u_alpha", s.alpha_mode),
                ("u_fog", s.fog_mode),
                ("u_color0", s.color0),
                ("u_color1", s.color1),
                ("u_chroma", s.chroma_key),
                ("u_zacolor", s.za_color),
                ("u_fogcolor", s.fog_color),
                ("u_stipple", s.stipple),
                ("u_yorigin", s.yorigin),
                ("u_config", s.send_config.unwrap_or(0)),
            ] {
                gl.uniform_1_i32(p.at(name), value as i32);
            }
            gl.uniform_1_i32_slice(p.at("u_fogblend"), &s.fogblend.map(|v| v as i32));
            gl.uniform_1_i32_slice(p.at("u_fogdelta"), &s.fogdelta.map(|v| v as i32));
            gl.uniform_1_f32(p.at("u_scale"), scale);
            let mut units = 0;
            for (unit, t) in s.tmu.iter().enumerate() {
                let Some(t) = t else { continue };
                let Some(texture) = self.textures.get_mut(&t.texture) else { continue };
                units |= 1 << unit;
                gl.active_texture(glow::TEXTURE1 + unit as u32);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture.texture));
                if let Some(anisotropy) = self.texture_anisotropy {
                    if texture.anisotropy != Some(anisotropy) {
                        gl.tex_parameter_f32(glow::TEXTURE_2D, TEXTURE_MAX_ANISOTROPY, anisotropy);
                        texture.anisotropy = Some(anisotropy);
                    }
                }
                let wrap = |clamp: bool| if clamp { glow::CLAMP_TO_EDGE } else { glow::REPEAT };
                let (min, mag) = sampling_filters(t.mode, t.unfiltered, self.texture_anisotropy);
                let params = [wrap(t.mode & 0x40 != 0), wrap(t.mode & 0x80 != 0), min, mag];
                if texture.params != Some(params) {
                    let names = [glow::TEXTURE_WRAP_S, glow::TEXTURE_WRAP_T, glow::TEXTURE_MIN_FILTER, glow::TEXTURE_MAG_FILTER];
                    for (name, value) in names.into_iter().zip(params) {
                        gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
                    }
                    texture.params = Some(params);
                }
                let first = t.first_level as f32;
                let lod = [t.lodmin as f32 / 256.0 - first, t.lodmax as f32 / 256.0 - first];
                if texture.lod != Some(lod) {
                    gl.tex_parameter_f32(glow::TEXTURE_2D, glow::TEXTURE_MIN_LOD, lod[0]);
                    gl.tex_parameter_f32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LOD, lod[1]);
                    texture.lod = Some(lod);
                }
                let [mode, size, lods, detail] = if unit == 0 {
                    ["u_tmode0", "u_tsize0", "u_tlod0", "u_tdetail0"]
                } else {
                    ["u_tmode1", "u_tsize1", "u_tlod1", "u_tdetail1"]
                };
                gl.uniform_1_i32(p.at(mode), t.mode as i32);
                gl.uniform_3_f32(p.at(size), t.width as f32, t.height as f32, t.lodbias as f32 / 256.0);
                gl.uniform_3_i32(p.at(lods), t.lodmin, t.lodmax, t.lodbias);
                gl.uniform_3_i32(p.at(detail), t.detailbias, t.detailmax, t.detailscale as i32);
            }
            if s.send_config.is_some() && units & 1 != 0 {
                units |= 4;
            }
            gl.uniform_1_i32(p.at("u_units"), units);
            gl.uniform_1_i32(p.at("u_constant_depth"), constant_depth as i32);

            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, vertex_bytes(&draw.vertices), glow::STREAM_DRAW);
            if constant_depth && depth_write {
                // The card compares zaColor but stores the interpolated
                // depth. GL uses gl_FragDepth for both: record which samples
                // pass with stencil, then write their real depth separately.
                // Keep overlapping triangles in order, including with MSAA.
                gl.enable(glow::STENCIL_TEST);
                for first in (0..draw.vertices.len() as i32).step_by(3) {
                    gl.stencil_mask(0xFF);
                    gl.clear_stencil(0);
                    gl.clear(glow::STENCIL_BUFFER_BIT);
                    gl.stencil_func(glow::ALWAYS, 1, 0xFF);
                    gl.stencil_op(glow::KEEP, glow::KEEP, glow::REPLACE);
                    gl.depth_func(DEPTH_FUNCS[func]);
                    gl.depth_mask(false);
                    gl.color_mask(rgb, rgb, rgb, false);
                    gl.uniform_1_i32(p.at("u_constant_depth"), 1);
                    gl.draw_arrays(glow::TRIANGLES, first, 3);

                    gl.stencil_mask(0);
                    gl.stencil_func(glow::EQUAL, 1, 0xFF);
                    gl.stencil_op(glow::KEEP, glow::KEEP, glow::KEEP);
                    gl.depth_func(glow::ALWAYS);
                    gl.depth_mask(true);
                    gl.color_mask(false, false, false, false);
                    gl.uniform_1_i32(p.at("u_constant_depth"), 0);
                    gl.draw_arrays(glow::TRIANGLES, first, 3);
                }
                gl.disable(glow::STENCIL_TEST);
                gl.stencil_mask(0xFF);
            } else {
                gl.draw_arrays(glow::TRIANGLES, 0, draw.vertices.len() as i32);
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.active_texture(glow::TEXTURE0);
        }
    }

    /// Draw only into `clip` (left, right, top, bottom in the card's
    /// pixels), or everywhere.
    fn clip(&self, gl: &glow::Context, clip: Option<[u32; 4]>) {
        let s = self.scale as i32;
        // SAFETY: see `VoodooGl`.
        unsafe {
            match clip {
                Some([l, r, t, b]) => {
                    let (l, r) = (l.min(self.width) as i32, r.min(self.width) as i32);
                    let (t, b) = (t.min(self.height) as i32, b.min(self.height) as i32);
                    gl.enable(glow::SCISSOR_TEST);
                    gl.scissor(l * s, t * s, (r - l).max(0) * s, (b - t).max(0) * s);
                }
                None => gl.disable(glow::SCISSOR_TEST),
            }
        }
    }

    fn fill(&mut self, gl: &glow::Context, fill: &Fill) {
        let [l, r, t, b] = fill.rect;
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.disable(glow::BLEND);
            self.clip(gl, Some([l, r, t, b]));
            let target = self.targets.get(&fill.dest).or_else(|| self.targets.values().next());
            let Some(target) = target else { return };
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.draw_fbo()));
            if let Some(color) = fill.color {
                gl.color_mask(true, true, true, false);
                let c = |shift: u32| ((color >> shift) & 0xFF) as f32 / 255.0;
                gl.clear_color(c(16), c(8), c(0), 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
            }
            if let Some(aux) = fill.aux {
                if fill.alpha_planes {
                    gl.color_mask(false, false, false, true);
                    gl.clear_color(0.0, 0.0, 0.0, (aux & 0xFF) as f32 / 255.0);
                    gl.clear(glow::COLOR_BUFFER_BIT);
                } else if self.depth.is_some() {
                    gl.depth_mask(true);
                    gl.clear_depth_f64(aux as f64 / 65535.0);
                    gl.clear(glow::DEPTH_BUFFER_BIT);
                }
            }
        }
    }

    /// The picture the card shows, with `screen`'s pixels over it where
    /// they differ from `base` (the settings window, messages): a texture
    /// and its size, or None if the card shows nothing.
    pub fn composite(&mut self, gl: &glow::Context, screen: &Frame, base: &Frame) -> Option<(glow::Texture, (u32, u32))> {
        let (w, h) = self.scaled();
        let front = self.targets.get(&self.front?)?;
        front.resolve(gl, (w, h));
        let front = front.color;
        let size = (w as u32, h as u32);
        // SAFETY: see `VoodooGl`.
        unsafe {
            if self.composite.as_ref().is_none_or(|(_, s)| *s != size) {
                if let Some((old, _)) = self.composite.take() {
                    old.delete(gl);
                }
                self.composite = Some((target(gl, size_i32(size), 1, None).ok()?, size));
            }
            if self.lut_of != Some(self.clut) {
                let table = lut(&self.clut);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.lut));
                let pixels = glow::PixelUnpackData::Slice(Some(&table));
                gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, 256, 1, 0, glow::RGBA, glow::UNSIGNED_BYTE, pixels);
                self.lut_of = Some(self.clut);
            }
            let (composite, _) = self.composite.as_ref()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(composite.fbo));
            gl.viewport(0, 0, w, h);
            gl.disable(glow::BLEND);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::SCISSOR_TEST);
            gl.color_mask(true, true, true, true);
            let p = &self.clut_program;
            gl.use_program(Some(p.program));
            gl.uniform_4_f32(p.at("u_rect"), 0.0, 0.0, self.width as f32, self.height as f32);
            gl.uniform_2_f32(p.at("u_size"), self.width as f32, self.height as f32);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.lut));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(front));
            gl.bind_vertex_array(Some(self.quad_vao));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);

            // What is drawn over the machine's picture.
            let fits = (screen.width, screen.height) == (self.width, self.height)
                && (base.width, base.height) == (self.width, self.height);
            if fits && screen.rgb != base.rgb {
                let over: Vec<u8> = screen
                    .rgb
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .zip(base.rgb.as_chunks::<3>().0)
                    .flat_map(|(&[r, g, b], old)| if [r, g, b] != *old { [r, g, b, 0xFF] } else { [0; 4] })
                    .collect();
                gl.bind_texture(glow::TEXTURE_2D, Some(self.staging_texture));
                let pixels = glow::PixelUnpackData::Slice(Some(&over));
                let (nw, nh) = (self.width as i32, self.height as i32);
                gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, nw, nh, glow::RGBA, glow::UNSIGNED_BYTE, pixels);
                let p = &self.pixels;
                gl.use_program(Some(p.program));
                gl.uniform_4_f32(p.at("u_rect"), 0.0, 0.0, self.width as f32, self.height as f32);
                gl.uniform_2_f32(p.at("u_size"), self.width as f32, self.height as f32);
                gl.uniform_1_f32(p.at("u_scale"), self.scale as f32);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            }
            restore(gl);
            Some((composite.color, size))
        }
    }
}

fn size_i32((w, h): (u32, u32)) -> (i32, i32) {
    (w as i32, h as i32)
}

/// The card's source blend factors as OpenGL's.
fn source_factor(f: u32) -> u32 {
    match f {
        1 => glow::SRC_ALPHA,
        2 => glow::DST_COLOR,
        3 => glow::DST_ALPHA,
        4 => glow::ONE,
        5 => glow::ONE_MINUS_SRC_ALPHA,
        6 => glow::ONE_MINUS_DST_COLOR,
        7 => glow::ONE_MINUS_DST_ALPHA,
        15 => glow::SRC_ALPHA_SATURATE,
        _ => glow::ZERO,
    }
}

/// The card's destination blend factors as OpenGL's. 15, the colour
/// before fog, OpenGL hasn't: the colour after it stands in.
fn dest_factor(f: u32) -> u32 {
    match f {
        1 => glow::SRC_ALPHA,
        2 | 15 => glow::SRC_COLOR,
        3 => glow::DST_ALPHA,
        4 => glow::ONE,
        5 => glow::ONE_MINUS_SRC_ALPHA,
        6 => glow::ONE_MINUS_SRC_COLOR,
        7 => glow::ONE_MINUS_DST_ALPHA,
        _ => glow::ZERO,
    }
}

/// A staged pixel: a 5-6-5 colour as RGB, or a depth as its low and high
/// bytes.
fn rgba(key: Option<u32>, value: u16) -> [u8; 4] {
    match key {
        Some(_) => {
            let (r, g, b) = ((value >> 11) as u8, (value >> 5 & 0x3F) as u8, (value & 0x1F) as u8);
            [r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2, 0xFF]
        }
        None => [value as u8, (value >> 8) as u8, 0, 0xFF],
    }
}

/// The gamma table as 256 RGBA entries a component takes its value from.
fn lut(clut: &[u32; 33]) -> Vec<u8> {
    super::gamma_table(clut).iter().flat_map(|&[r, g, b]| [r, g, b, 0xFF]).collect()
}

fn vertex_bytes(vertices: &[Vertex]) -> &[u8] {
    // SAFETY: a Vertex is repr(C) and all f32, so it has no padding, and
    // any bytes may be read as u8.
    unsafe { std::slice::from_raw_parts(vertices.as_ptr().cast::<u8>(), std::mem::size_of_val(vertices)) }
}

/// Put back what the rest of the display expects of OpenGL's state.
fn restore(gl: &glow::Context) {
    // SAFETY: see `VoodooGl`.
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.disable(glow::BLEND);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::STENCIL_TEST);
        gl.disable(glow::SCISSOR_TEST);
        gl.color_mask(true, true, true, true);
        gl.depth_mask(true);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_vertex_array(None);
        gl.use_program(None);
    }
}

/// A texture with `filter`, clamped at its edges, bound.
fn texture(gl: &glow::Context, filter: u32) -> Result<glow::Texture, String> {
    // SAFETY: see `VoodooGl`.
    unsafe {
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        Ok(texture)
    }
}

/// A colour buffer of `size` with `samples` a pixel and `depth` attached,
/// the framebuffer to draw into bound.
fn target(gl: &glow::Context, (w, h): (i32, i32), samples: u32, depth: Option<Depth>) -> Result<Target, String> {
    // SAFETY: see `VoodooGl`.
    unsafe {
        let color = texture(gl, glow::NEAREST)?;
        let none = glow::PixelUnpackData::Slice(None);
        gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0, glow::RGBA, glow::UNSIGNED_BYTE, none);
        let fbo = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(color), 0);
        let mut target = Target { fbo, color, samples: None };
        if samples > 1 {
            let buffer = gl.create_renderbuffer()?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(buffer));
            gl.renderbuffer_storage_multisample(glow::RENDERBUFFER, samples as i32, glow::RGBA8, w, h);
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);
            let fbo = gl.create_framebuffer()?;
            target.samples = Some((fbo, buffer));
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::RENDERBUFFER, Some(buffer));
        }
        match depth {
            Some(Depth::Texture(depth)) => {
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::DEPTH_STENCIL_ATTACHMENT, glow::TEXTURE_2D, Some(depth), 0)
            }
            Some(Depth::Samples(depth)) => {
                gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_STENCIL_ATTACHMENT, glow::RENDERBUFFER, Some(depth))
            }
            None => {}
        }
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        if status != glow::FRAMEBUFFER_COMPLETE {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            target.delete(gl);
            return Err(format!("the framebuffer is incomplete ({:04X}h)", status));
        }
        gl.viewport(0, 0, w, h);
        Ok(target)
    }
}

/// Compile and link a program with its attributes at their locations, and
/// find its uniforms.
fn compile(
    gl: &glow::Context,
    glsl: Glsl,
    vertex: &str,
    fragment: &str,
    attributes: &[(u32, &str)],
    uniforms: &[&'static str],
) -> Result<Program, String> {
    // SAFETY: see `VoodooGl`.
    unsafe {
        let program = gl.create_program()?;
        let mut stages = Vec::new();
        let mut problem = None;
        for (kind, source) in [(glow::VERTEX_SHADER, vertex), (glow::FRAGMENT_SHADER, fragment)] {
            let stage = gl.create_shader(kind)?;
            gl.shader_source(stage, &format!("{}{}", glsl.preamble(), source));
            gl.compile_shader(stage);
            gl.attach_shader(program, stage);
            stages.push(stage);
            if !gl.get_shader_compile_status(stage) {
                problem = Some(format!("a 3dfx shader doesn't compile: {}", gl.get_shader_info_log(stage)));
                break;
            }
        }
        if problem.is_none() {
            for &(index, name) in attributes {
                gl.bind_attrib_location(program, index, name);
            }
            gl.bind_frag_data_location(program, 0, "o_color");
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                problem = Some(format!("a 3dfx shader doesn't link: {}", gl.get_program_info_log(program)));
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
        let mut found = HashMap::new();
        for &name in uniforms {
            let location =
                gl.get_uniform_location(program, name).or_else(|| gl.get_uniform_location(program, &format!("{}[0]", name)));
            if let Some(location) = location {
                found.insert(name, location);
            }
        }
        Ok(Program { program, uniforms: found })
    }
}

#[cfg(test)]
mod tests {
    use super::sampling_filters;

    #[test]
    #[ignore = "requires a desktop OpenGL context"]
    fn constant_depth_comparison_writes_interpolated_depth() {
        use super::*;
        use crate::voodoo::mirror::{DrawState, Layout};
        let sdl = sdl2::init().unwrap();
        let video = sdl.video().unwrap();
        let attrs = video.gl_attr();
        attrs.set_context_profile(sdl2::video::GLProfile::Core);
        attrs.set_context_version(3, 3);
        let window = video.window("Depth regression", 8, 8).opengl().hidden().build().unwrap();
        let _context = window.gl_create_context().unwrap();
        // SAFETY: this thread owns the current SDL OpenGL context.
        let gl = unsafe { glow::Context::from_loader_function(|s| video.gl_get_proc_address(s).cast()) };
        for samples in [1, 4] {
            let mut renderer = VoodooGl::new(&gl, Glsl::Gl150, 1, samples, 1).unwrap();
            let state = DrawState {
                dest: 0, fbz_color_path: 0, fbz_mode: (1 << 4) | (5 << 5) | (1 << 9) | (1 << 10) | (1 << 20),
                alpha_mode: 0, fog_mode: 0, za_color: 0, chroma_key: 0,
                color0: 0, color1: 0, fog_color: 0, stipple: 0, yorigin: 7,
                clip: None, aux: true, fogblend: [0; 64], fogdelta: [0; 64], send_config: None, tmu: [None; 2],
            };
            // Both triangles compare against zero. The second must see
            // the first's interpolated depth (20000), not its reference (0).
            let vertices = [(20000.0, [255.0, 0.0, 0.0, 255.0]), (30000.0, [0.0, 255.0, 0.0, 255.0])]
                .into_iter().flat_map(|(z, color)| {
                    [[0.0, 0.0], [16.0, 0.0], [0.0, 16.0]].map(|pos| Vertex { pos, color, zw: [z, 0.0], ..Vertex::default() })
                }).collect();
            renderer.run(&gl, Recording {
                commands: vec![
                    Command::Resync(Box::new(Snapshot {
                        layout: Layout { width: 8, height: 8, rowpixels: 8, color: vec![0, 64], aux: Some(128) },
                        color: vec![vec![0; 64]; 2], aux: Some(vec![0xFFFF; 64]),
                    })),
                    Command::Draw(Box::new(Draw { state, vertices })),
                ],
                front: Some(0), output: true, width: 8, height: 8, clut: [0; 33],
            });
            let target = &renderer.targets[&0];
            target.resolve(&gl, (8, 8));
            let mut pixel = [0; 4];
            // SAFETY: the context and framebuffer belong to this thread.
            unsafe {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.fbo));
                gl.read_pixels(2, 2, 1, 1, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut pixel)));
                assert_eq!(&pixel[..3], &[0, 255, 0], "samples={samples}");
                if samples == 1 {
                    let mut depth = [0u8; 4];
                    gl.read_pixels(2, 2, 1, 1, glow::DEPTH_COMPONENT, glow::FLOAT, glow::PixelPackData::Slice(Some(&mut depth)));
                    assert!((f32::from_ne_bytes(depth) * 65535.0 - 30000.0).abs() < 1.0);
                }
                assert_eq!(gl.get_error(), glow::NO_ERROR);
            }
            renderer.destroy(&gl);
        }
    }

    /// Replay a saved Glide scene without changing the running game.
    /// RUST_DOS_VOODOO_STATE selects the state; images go under target/tmp.
    #[test]
    #[ignore = "requires a saved Glide scene and a desktop OpenGL context"]
    fn replay_saved_glide_scene() {
        use super::*;
        use crate::cpu::Cpu;
        use crate::exec::{NoHook, run_batch};
        use crate::savestate::{machine, slots};
        use std::{fs, path::PathBuf};

        let path = std::env::var_os("RUST_DOS_VOODOO_STATE").expect("RUST_DOS_VOODOO_STATE");
        let (header, state) = slots::decode(&fs::read(path).unwrap()).unwrap();
        let settings = slots::machine_settings(&header.machine, &Default::default());
        let mut cpu = Cpu::with_memory(PathBuf::from("."), header.memsize);
        crate::hardware::configure(&mut cpu, &settings, crate::keylayout::Layout::us());
        machine::load(&mut cpu, &state).unwrap();
        cpu.bus.voodoo.as_mut().unwrap().set_mirror(true);
        cpu.bus.voodoo.as_mut().unwrap().set_software_picture(true);

        let sdl = sdl2::init().unwrap();
        let video = sdl.video().unwrap();
        let attrs = video.gl_attr();
        attrs.set_context_profile(sdl2::video::GLProfile::Core);
        attrs.set_context_version(3, 3);
        let window = video.window("Glide replay", 640, 480).opengl().hidden().build().unwrap();
        let _context = window.gl_create_context().unwrap();
        // SAFETY: this thread owns the current SDL OpenGL context.
        let gl = unsafe { glow::Context::from_loader_function(|s| video.gl_get_proc_address(s).cast()) };
        let samples = std::env::var("RUST_DOS_VOODOO_SAMPLES").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
        let mut renderer = VoodooGl::new(&gl, Glsl::Gl150, 1, samples, 1).unwrap();
        let save = |name: &str, rgb: &[u8], w, h| {
            let file = fs::File::create(format!("target/tmp/{name}.png")).unwrap();
            let mut encoder = png::Encoder::new(file, w, h);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header().unwrap().write_image_data(rgb).unwrap();
        };
        for frame in 0..60 {
            if frame != 0 {
                cpu.bus.start_batch(cpu.bus.clock.icount + 1_600_000);
                run_batch(&mut cpu, &mut NoHook, false);
            }
            let card = cpu.bus.voodoo.as_mut().unwrap();
            let recording = card.take_mirror().unwrap();
            let fills: Vec<_> = recording.commands.iter().filter_map(|c| match c {
                Command::Fill(f) => Some(f), _ => None,
            }).collect();
            println!("frame {frame}, front {:?}, commands {}, fills {:?}", recording.front, recording.commands.len(), fills);
            renderer.run(&gl, recording);
            if frame % 10 == 0 || frame == 59 {
                let (w, h) = (card.fbi.width, card.fbi.height);
                card.frame_buffer();
                card.prepare_display();
                let mut reference = vec![0; (w * h * 3) as usize];
                card.render(&mut reference, w as usize);
                save(&format!("glide-{frame}-software"), &reference, w, h);
                let target = &renderer.targets[&renderer.front.unwrap()];
                target.resolve(&gl, (w as i32, h as i32));
                let mut rendered = vec![0; reference.len()];
                // GL's lower row is the card's top row (the shader's convention).
                unsafe {
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.fbo));
                    gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
                    gl.read_pixels(0, 0, w as i32, h as i32, glow::RGB, glow::UNSIGNED_BYTE,
                        glow::PixelPackData::Slice(Some(&mut rendered)));
                    assert_eq!(gl.get_error(), glow::NO_ERROR);
                }
                save(&format!("glide-{frame}-opengl"), &rendered, w, h);
                let bad = reference.chunks_exact(3).zip(rendered.chunks_exact(3))
                    .filter(|(a, b)| a.iter().zip(*b).any(|(a, b)| a.abs_diff(*b) > 32)).count();
                println!("frame {frame}: {bad} pixels differ by more than 32");
            }
        }
        renderer.destroy(&gl);
    }

    #[test]
    fn unfiltered_blends_mip_levels_only_with_supported_anisotropy() {
        for mode in [0, 2, 4, 6] {
            for anisotropy in [None, Some(1.0)] {
                assert_eq!(sampling_filters(mode, true, anisotropy), (glow::NEAREST_MIPMAP_NEAREST, glow::NEAREST));
            }
            for anisotropy in [2.0, 4.0, 8.0, 16.0] {
                assert_eq!(sampling_filters(mode, true, Some(anisotropy)), (glow::NEAREST_MIPMAP_LINEAR, glow::NEAREST));
            }
        }
    }

    #[test]
    fn default_sampling_keeps_the_games_filters() {
        for anisotropy in [None, Some(1.0), Some(16.0)] {
            for (mode, min, mag) in [
                (0, glow::NEAREST_MIPMAP_NEAREST, glow::NEAREST),
                (2, glow::LINEAR_MIPMAP_NEAREST, glow::NEAREST),
                (4, glow::NEAREST_MIPMAP_NEAREST, glow::LINEAR),
                (6, glow::LINEAR_MIPMAP_NEAREST, glow::LINEAR),
            ] {
                assert_eq!(sampling_filters(mode, false, anisotropy), (min, mag));
            }
        }
    }
}
