//! The 3dfx card drawn again with OpenGL at `voodoo_scale` times its
//! resolution, for the window and the libretro core's hardware rendering
//! (`voodoo_renderer=opengl`). The card records what it draws
//! (`super::mirror`); here each of its colour buffers is the texture of a
//! framebuffer object, all sharing one depth texture, the triangles go
//! through the card's pixel pipeline in a shader (gl/triangle.frag) with
//! OpenGL's depth test and blending, and the front buffer through the
//! card's gamma table is the picture shown. With `voodoo_dither`, what the
//! game draws dithered (triangles and fastfills) is dithered to 5-6-5 as
//! the card does it (`set_dither`). With `voodoo_msaa`, the buffers are
//! multisampled and resolved into their textures before they are shown.
//! The software rasterizer's memory stays what screenshots, recordings and
//! the debugger see, and what the game reads back; it only draws what those
//! can still see (`super::backlog`).

use glow::HasContext;
use crate::video::Frame;
use crate::video::shader::Glsl;
use super::gpu::GpuLink;
use super::mirror::{
    Blit, BlitSource, Command, Convert, Draw, Fill, Frame as Recording, Key, Pixels, Scanout,
    Snapshot, Surface, Texture, Vertex,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

const TRIANGLE_VERT: &str = include_str!("gl/triangle.vert");
const TRIANGLE_FRAG: &str = include_str!("gl/triangle.frag");
const QUAD_VERT: &str = include_str!("gl/quad.vert");
const PIXELS_FRAG: &str = include_str!("gl/pixels.frag");
const DEPTH_FRAG: &str = include_str!("gl/depth.frag");
const FILL_FRAG: &str = include_str!("gl/fill.frag");
const CLUT_FRAG: &str = include_str!("gl/clut.frag");
const BANSHEE_FRAG: &str = include_str!("gl/banshee.frag");
const BLIT_FRAG: &str = include_str!("gl/blit.frag");

mod uniforms;
use uniforms::{Banshee, Blit2d, FillQuad, Locations, Quad, Triangle};

/// Bytes a row of the textures of the card's memory (`CardMemory`) at
/// most: a page (`pages::PAGE`).
const MEMORY_ROW: u32 = 4096;

/// Limits below what the host's OpenGL allows, to try the renderer as a
/// smaller driver runs it: `RUST_DOS_GL_MAX_TEXTURE_SIZE=<texels>` lowers
/// the largest texture it assumes, and `RUST_DOS_GL_NO_BLIT=1` leaves out
/// the program of the 2D operations, as if it didn't compile.
#[derive(Clone, Copy, Debug, Default)]
pub struct Limits {
    pub max_texture_size: Option<u32>,
    pub no_blit: bool,
}

impl Limits {
    pub fn from_env() -> Self {
        Self {
            max_texture_size: std::env::var("RUST_DOS_GL_MAX_TEXTURE_SIZE")
                .ok()
                .and_then(|v| v.trim().parse().ok()),
            no_blit: std::env::var("RUST_DOS_GL_NO_BLIT").is_ok_and(|v| v != "0"),
        }
    }
}

/// The card's depth functions as OpenGL's.
const DEPTH_FUNCS: [u32; 8] =
    [glow::NEVER, glow::LESS, glow::EQUAL, glow::LEQUAL, glow::GREATER, glow::NOTEQUAL, glow::GEQUAL, glow::ALWAYS];
const TEXTURE_MAX_ANISOTROPY: u32 = 0x84FE;
const MAX_TEXTURE_MAX_ANISOTROPY: u32 = 0x84FF;

/// A linked program and its uniforms.
struct Program<U> {
    program: glow::Program,
    uniforms: U,
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
        self.resolve_rows(gl, w, (0, h));
    }

    /// Resolve rows `y0..y1` of the samples, `w` wide, into the texture.
    fn resolve_rows(&self, gl: &glow::Context, w: i32, (y0, y1): (i32, i32)) {
        let Some((samples, _)) = self.samples else { return };
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(samples));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.fbo));
            gl.blit_framebuffer(0, y0, w, y1, 0, y0, w, y1, glow::COLOR_BUFFER_BIT, glow::NEAREST);
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
    levels: Vec<(u32, u32)>,
    level_count: usize,
    params: Option<[u32; 4]>,
    lod: Option<[f32; 2]>,
    anisotropy: Option<f32>,
}

/// How `CardMemory` lays the card's memory out in the layers of a 2D
/// array texture: byte `a` is texel `a % width` of row `a / width % rows`
/// of layer `a / (width * rows)`, all of them powers of two. A row is
/// `MEMORY_ROW` bytes, and a layer as many rows as OpenGL allows, so with
/// textures of 4096 texels or more 16 MB is one layer of 4096 rows. Under
/// a smaller limit (OpenGL 3 promises only 1024) the rows are narrower
/// and the memory takes several layers.
#[derive(Clone, Copy, Debug, PartialEq)]
struct MemoryLayout {
    /// The bytes a row and a layer, as powers of two.
    row_bits: u32,
    layer_bits: u32,
    layers: u32,
}

impl MemoryLayout {
    /// The layout of `size` bytes in textures of at most `most` texels
    /// across and down, with at most `layers` layers.
    fn new(size: u32, most: u32, layers: u32) -> Result<Self, String> {
        let side = 1 << most.max(1).ilog2();
        let width = MEMORY_ROW.min(side);
        let rows = size.div_ceil(width).max(1).next_power_of_two();
        let layer_rows = rows.min(side);
        let layout = Self {
            row_bits: width.ilog2(),
            layer_bits: width.ilog2() + layer_rows.ilog2(),
            layers: rows / layer_rows,
        };
        if layout.layers > layers {
            return Err(format!(
                "OpenGL's textures have at most {most} texels a side and {layers} layers, \
                 too few for the card's memory of {size} bytes"
            ));
        }
        Ok(layout)
    }

    fn width(self) -> u32 {
        1 << self.row_bits
    }

    fn layer_rows(self) -> u32 {
        1 << (self.layer_bits - self.row_bits)
    }

    /// The rows of all the layers.
    fn rows(self) -> u32 {
        self.layers * self.layer_rows()
    }

    /// Rows `first..last` of the memory as the rows of the layers they
    /// are in: each layer's number and its rows `first..last`.
    fn spans(self, first: u32, last: u32) -> impl Iterator<Item = (u32, u32, u32)> {
        let n = self.layer_rows();
        let last = last.min(self.rows());
        (first / n..last.div_ceil(n)).filter_map(move |layer| {
            let (top, bottom) = (first.max(layer * n), last.min(layer * n + n));
            (top < bottom).then(|| (layer, top - layer * n, bottom - layer * n))
        })
    }

    /// `spans` of rows `first..last`, each with the bytes it takes of
    /// those rows' bytes in order.
    fn parts(
        self,
        first: u32,
        last: u32,
    ) -> impl Iterator<Item = (u32, u32, u32, std::ops::Range<usize>)> {
        let (n, w) = (self.layer_rows(), self.width() as usize);
        self.spans(first, last).map(move |(layer, top, bottom)| {
            let from = (layer * n + top - first) as usize * w;
            (layer, top, bottom, from..from + (bottom - top) as usize * w)
        })
    }

    /// What the shaders' `u_mem_layout` takes for memory of `size`
    /// bytes.
    fn uniform(self, size: u32) -> [i32; 3] {
        [
            size.max(1) as i32 - 1,
            self.row_bits as i32,
            self.layer_bits as i32,
        ]
    }
}

/// A Banshee's or Voodoo3's memory as an integer texture array of bytes
/// (`MemoryLayout`), which the picture is composed from.
/// `Command::Memory` brings it the pages the card's memory changed, and
/// 2D operations draw into it (`blit`) through a framebuffer a layer,
/// reading what was there before from a copy of the rows they read
/// (`copy`).
struct CardMemory {
    texture: glow::Texture,
    copy: glow::Texture,
    /// A framebuffer drawing into each layer of `texture`, and of `copy`.
    fbos: Vec<glow::Framebuffer>,
    copy_fbos: Vec<glow::Framebuffer>,
    /// The card's memory in bytes, which the textures have rows for, and
    /// how they hold it.
    size: u32,
    layout: MemoryLayout,
    /// OpenGL's largest texture: texels a side, and layers.
    most: (u32, u32),
}

impl CardMemory {
    fn new(gl: &glow::Context, most: (u32, u32)) -> Result<Self, String> {
        let layout = MemoryLayout::new(MEMORY_ROW, most.0, most.1)?;
        let texture = texture_array(gl)?;
        let copy = match texture_array(gl) {
            Ok(copy) => copy,
            Err(e) => {
                // SAFETY: see `VoodooGl`.
                unsafe { gl.delete_texture(texture) };
                return Err(e);
            }
        };
        let mut memory = Self {
            texture,
            copy,
            fbos: Vec::new(),
            copy_fbos: Vec::new(),
            size: 0,
            layout,
            most,
        };
        // What was made goes again when the memory can't be drawn into.
        if let Err(e) = memory.allocate(gl, MEMORY_ROW) {
            memory.delete(gl);
            return Err(e);
        }
        Ok(memory)
    }

    /// Rows for `size` bytes, their contents not yet known. When OpenGL
    /// can't make them, the memory has no rows (`usable`): nothing goes
    /// into it or is read back, and the card does its 2D operations in
    /// software.
    fn allocate(&mut self, gl: &glow::Context, size: u32) -> Result<(), String> {
        self.size = size;
        let made = MemoryLayout::new(size, self.most.0, self.most.1)
            .and_then(|layout| self.storage(gl, layout).map(|()| layout));
        match made {
            Ok(layout) => {
                self.layout = layout;
                Ok(())
            }
            Err(e) => {
                self.layout.layers = 0;
                Err(e)
            }
        }
    }

    /// Whether the memory has its rows (`allocate`).
    fn usable(&self) -> bool {
        self.layout.layers > 0
    }

    /// The textures and framebuffers for `layout`.
    fn storage(&mut self, gl: &glow::Context, layout: MemoryLayout) -> Result<(), String> {
        let (w, h) = (layout.width() as i32, layout.layer_rows() as i32);
        let layers = layout.layers as usize;
        let (format, kind) = (glow::RED_INTEGER, glow::UNSIGNED_BYTE);
        // SAFETY: see `VoodooGl`.
        unsafe {
            for (texture, fbos) in [
                (self.texture, &mut self.fbos),
                (self.copy, &mut self.copy_fbos),
            ] {
                let none = glow::PixelUnpackData::Slice(None);
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(texture));
                let (target, r8ui) = (glow::TEXTURE_2D_ARRAY, glow::R8UI as i32);
                gl.tex_image_3d(target, 0, r8ui, w, h, layers as i32, 0, format, kind, none);
                for fbo in fbos.drain(layers.min(fbos.len())..) {
                    gl.delete_framebuffer(fbo);
                }
                while fbos.len() < layers {
                    fbos.push(gl.create_framebuffer()?);
                }
                for (layer, &fbo) in fbos.iter().enumerate() {
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                    let (target, at, layer) =
                        (glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, layer as i32);
                    gl.framebuffer_texture_layer(target, at, Some(texture), 0, layer);
                    let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
                    if status != glow::FRAMEBUFFER_COMPLETE {
                        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                        return Err(format!(
                            "the card's memory can't be drawn into ({status:04X}h)"
                        ));
                    }
                }
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            }
        }
        Ok(())
    }

    /// The row byte `a` is in.
    fn row(&self, a: u32) -> u32 {
        a >> self.layout.row_bits
    }

    /// Copy rows `first..last` into the copy the 2D operations read.
    fn copy_rows(&self, gl: &glow::Context, first: u32, last: u32) {
        let w = self.layout.width() as i32;
        for (layer, top, bottom) in self.layout.spans(first, last) {
            let (top, bottom) = (top as i32, bottom as i32);
            let layer = layer as usize;
            // SAFETY: see `VoodooGl`.
            unsafe {
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.fbos[layer]));
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.copy_fbos[layer]));
                let (bits, nearest) = (glow::COLOR_BUFFER_BIT, glow::NEAREST);
                gl.blit_framebuffer(0, top, w, bottom, 0, top, w, bottom, bits, nearest);
            }
        }
        // SAFETY: see `VoodooGl`.
        unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, None) };
    }

    /// Bytes from byte `at` on (which starts a page) into `out`, whole
    /// pages. False, with `out` as it was, if the texture hasn't them all.
    fn read(&self, gl: &glow::Context, at: u32, out: &mut [u8]) -> bool {
        let w = self.layout.width() as usize;
        let (first, rows) = (at as usize / w, out.len() / w);
        let whole = (at as usize).is_multiple_of(w) && out.len().is_multiple_of(w);
        if !whole || first + rows > self.layout.rows() as usize {
            return false;
        }
        let (format, kind) = (glow::RED_INTEGER, glow::UNSIGNED_BYTE);
        let parts = self.layout.parts(first as u32, (first + rows) as u32);
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            for (layer, top, bottom, bytes) in parts {
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(self.fbos[layer as usize]));
                gl.read_buffer(glow::COLOR_ATTACHMENT0);
                let pixels = glow::PixelPackData::Slice(Some(&mut out[bytes]));
                let (y, h) = (top as i32, (bottom - top) as i32);
                gl.read_pixels(0, y, w as i32, h, format, kind, pixels);
            }
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
        }
        true
    }

    /// `bytes` of a memory of `size` bytes, from byte `at` on, which
    /// starts a page.
    fn put(&mut self, gl: &glow::Context, size: u32, at: u32, bytes: &[u8]) {
        if size != self.size
            && let Err(e) = self.allocate(gl, size)
        {
            eprintln!("[DISPLAY] {e}: 2D operations stay on the CPU");
            return;
        }
        let layout = self.layout;
        let w = layout.width() as usize;
        let first = at as usize / w;
        let rows = (size as usize).div_ceil(w).saturating_sub(first);
        let bytes = &bytes[..bytes.len().min(rows * w)];
        let full = bytes.len() / w;
        let put = |first: usize, data: &[u8]| {
            let last = first + data.len() / w;
            for (layer, top, bottom, bytes) in layout.parts(first as u32, last as u32) {
                let pixels = glow::PixelUnpackData::Slice(Some(&data[bytes]));
                let (y, h, z) = (top as i32, (bottom - top) as i32, layer as i32);
                let (target, format, kind) = (
                    glow::TEXTURE_2D_ARRAY,
                    glow::RED_INTEGER,
                    glow::UNSIGNED_BYTE,
                );
                // SAFETY: see `VoodooGl`.
                unsafe {
                    gl.tex_sub_image_3d(target, 0, 0, y, z, w as i32, h, 1, format, kind, pixels)
                };
            }
        };
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.texture));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        }
        if full > 0 {
            put(first, &bytes[..full * w]);
        }
        if bytes.len() > full * w {
            let mut last = bytes[full * w..].to_vec();
            last.resize(w, 0);
            put(first + full, &last);
        }
        // SAFETY: see `VoodooGl`.
        unsafe { gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4) };
    }

    fn delete(&mut self, gl: &glow::Context) {
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.delete_texture(self.texture);
            gl.delete_texture(self.copy);
            for fbo in self.fbos.drain(..).chain(self.copy_fbos.drain(..)) {
                gl.delete_framebuffer(fbo);
            }
        }
    }
}

/// What composes a Banshee's or Voodoo3's picture (`compose`): the
/// program, the card's memory, its cursor's planes and its CLUT as
/// textures, and the texture of what the frame has over the picture.
/// The program of the 2D operations (`blit`), or why there is none: the
/// picture is composed without it, and the card does its 2D operations
/// in software.
struct BansheeGl {
    program: Program<Banshee>,
    blit: Result<Program<Blit2d>, String>,
    memory: CardMemory,
    cursor: glow::Texture,
    clut: glow::Texture,
    screen: Option<(glow::Texture, (u32, u32))>,
}

impl BansheeGl {
    fn new(gl: &glow::Context, glsl: Glsl, limits: Limits) -> Result<Self, String> {
        // SAFETY: see `VoodooGl`.
        let (side, layers) = unsafe {
            let side = gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE).max(1) as u32;
            let layers = gl.get_parameter_i32(glow::MAX_ARRAY_TEXTURE_LAYERS).max(1) as u32;
            let side = limits.max_texture_size.map_or(side, |m| m.clamp(1, side));
            (side, layers)
        };
        // All of the card's memory has to fit, not only what it has now.
        MemoryLayout::new(super::banshee::MEMORY as u32, side, layers)?;
        let program: Program<Banshee> = compile(gl, glsl, QUAD_VERT, BANSHEE_FRAG, &[])?;
        let blit: Result<Program<Blit2d>, String> = if limits.no_blit {
            Err("RUST_DOS_GL_NO_BLIT is set".to_string())
        } else {
            compile(gl, glsl, QUAD_VERT, BLIT_FRAG, &[])
        };
        // SAFETY: see `VoodooGl`.
        unsafe {
            if let Ok(blit) = &blit {
                gl.use_program(Some(blit.program));
                for (name, unit) in [("u_mem", 0), ("u_buf", 1)] {
                    gl.uniform_1_i32(gl.get_uniform_location(blit.program, name).as_ref(), unit);
                }
            }
            gl.use_program(Some(program.program));
            for (name, unit) in [
                ("u_mem", 0),
                ("u_desk_buf", 1),
                ("u_ovl_buf", 2),
                ("u_cursor", 3),
                ("u_clut", 4),
            ] {
                gl.uniform_1_i32(
                    gl.get_uniform_location(program.program, name).as_ref(),
                    unit,
                );
            }
            gl.use_program(None);
            let cursor = texture(gl, glow::NEAREST)?;
            let (format, kind) = (glow::RED_INTEGER, glow::UNSIGNED_BYTE);
            let zeros = glow::PixelUnpackData::Slice(Some(&[0u8; 1024]));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::R8UI as i32,
                16,
                64,
                0,
                format,
                kind,
                zeros,
            );
            let clut = texture(gl, glow::NEAREST)?;
            let zeros = glow::PixelUnpackData::Slice(Some(&[0u8; 512 * 4]));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                512,
                1,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                zeros,
            );
            Ok(Self {
                program,
                blit,
                memory: CardMemory::new(gl, (side, layers))?,
                cursor,
                clut,
                screen: None,
            })
        }
    }

    fn destroy(&mut self, gl: &glow::Context) {
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.delete_program(self.program.program);
            if let Ok(blit) = &self.blit {
                gl.delete_program(blit.program);
            }
            self.memory.delete(gl);
            for t in [self.cursor, self.clut] {
                gl.delete_texture(t);
            }
            if let Some((t, _)) = self.screen.take() {
                gl.delete_texture(t);
            }
        }
    }
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
    /// Whether to dither where the game dithers (`set_dither`).
    dither: bool,
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
    texture_bytes: Vec<u8>,
    /// The gamma table as 256 entries a component, and the table it is.
    lut: glow::Texture,
    lut_of: Option<[u32; 33]>,
    /// The picture shown, and its size.
    composite: Option<(Target, (u32, u32))>,
    triangle: Program<Triangle>,
    pixels: Program<Quad>,
    depth_program: Program<Quad>,
    clut_program: Program<Quad>,
    fill_program: Program<FillQuad>,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    /// No attributes: the quads make their corners from the vertex number.
    quad_vao: glow::VertexArray,
    /// What the card shows: its front buffer, and its gamma table.
    front: Option<u32>,
    clut: [u32; 33],
    /// A Banshee's picture instead, which `banshee` composes (or the
    /// reason it can't), without the memory, which is in `banshee`'s
    /// textures.
    scanout: Option<Box<Scanout>>,
    banshee: Result<BansheeGl, String>,
    /// Commands the card handed over before the frame ended
    /// (`run_early`) since the last frame.
    ran_early: bool,
}

/// The renderer and its context as the card reaches them between frames
/// (`gpu::GpuLink`), shared with the front end that shows the picture.
/// The renderer is None while there is none.
pub struct SharedGl {
    pub gl: Rc<glow::Context>,
    pub renderer: Rc<RefCell<Option<VoodooGl>>>,
}

impl GpuLink for SharedGl {
    fn ready(&self) -> bool {
        let renderer = self.renderer.borrow();
        let ready = |b: &BansheeGl| b.blit.is_ok() && b.memory.usable();
        renderer.as_ref().is_some_and(|r| r.banshee.as_ref().is_ok_and(ready))
    }

    fn run(&mut self, commands: Vec<Command>) {
        if let Some(renderer) = self.renderer.borrow_mut().as_mut() {
            renderer.run_early(&self.gl, &commands);
        }
    }

    fn read(&mut self, at: u32, out: &mut [u8]) -> bool {
        let renderer = self.renderer.borrow();
        renderer.as_ref().is_some_and(|r| r.read_memory(&self.gl, at, out))
    }

    fn read_buffer(&mut self, dest: Option<u32>, rows: (u32, u32), out: &mut [u16]) -> bool {
        let renderer = self.renderer.borrow();
        renderer
            .as_ref()
            .is_some_and(|r| r.read_buffer(&self.gl, dest, rows, out))
    }
}

impl VoodooGl {
    /// Draw at `scale` times the card's size with `samples` a pixel (as
    /// many as OpenGL has at most), and the requested texture anisotropy,
    /// within the `Limits` the environment sets.
    pub fn new(
        gl: &glow::Context,
        glsl: Glsl,
        scale: u32,
        samples: u32,
        anisotropy: u32,
    ) -> Result<Self, String> {
        Self::with_limits(gl, glsl, scale, samples, anisotropy, Limits::from_env())
    }

    /// `new` within `limits`.
    pub fn with_limits(
        gl: &glow::Context,
        glsl: Glsl,
        scale: u32,
        samples: u32,
        anisotropy: u32,
        limits: Limits,
    ) -> Result<Self, String> {
        if glsl == Glsl::Es300 {
            return Err("OpenGL ES has no noperspective interpolation".to_string());
        }
        let attributes = [(0, "a_pos"), (1, "a_color"), (2, "a_zw"), (3, "a_tex0"), (4, "a_tex1")];
        let triangle: Program<Triangle> =
            compile(gl, glsl, TRIANGLE_VERT, TRIANGLE_FRAG, &attributes)?;
        let fill_program: Program<FillQuad> = compile(gl, glsl, QUAD_VERT, FILL_FRAG, &[])?;
        let pixels: Program<Quad> = compile(gl, glsl, QUAD_VERT, PIXELS_FRAG, &[])?;
        let depth_program: Program<Quad> = compile(gl, glsl, QUAD_VERT, DEPTH_FRAG, &[])?;
        let clut_program: Program<Quad> = compile(gl, glsl, QUAD_VERT, CLUT_FRAG, &[])?;
        let banshee = BansheeGl::new(gl, glsl, limits);
        // SAFETY: see `VoodooGl`.
        unsafe {
            for (program, samplers) in [
                (triangle.program, &[("u_tex0", 1), ("u_tex1", 2)][..]),
                (pixels.program, &[("u_src", 0)][..]),
                (depth_program.program, &[("u_src", 0)][..]),
                (clut_program.program, &[("u_src", 0), ("u_lut", 1)][..]),
            ] {
                gl.use_program(Some(program));
                for &(name, unit) in samplers {
                    gl.uniform_1_i32(gl.get_uniform_location(program, name).as_ref(), unit);
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
                dither: true,
                width: 0,
                height: 0,
                targets: HashMap::new(),
                depth: None,
                staging: HashMap::new(),
                staging_texture,
                textures: HashMap::new(),
                texture_bytes: Vec::new(),
                lut,
                lut_of: None,
                composite: None,
                triangle,
                pixels,
                depth_program,
                clut_program,
                fill_program,
                vao,
                vbo,
                quad_vao,
                front: None,
                clut: [0; 33],
                scanout: None,
                banshee,
                ran_early: false,
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

    /// Dither to 5-6-5 as the card does where the game turns dithering on
    /// (`voodoo_dither`, on when the renderer starts), or keep 8 bits a
    /// colour. It applies to what is drawn from then on.
    pub fn set_dither(&mut self, on: bool) {
        self.dither = on;
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
            for program in [
                self.triangle.program,
                self.pixels.program,
                self.depth_program.program,
                self.clut_program.program,
                self.fill_program.program,
            ] {
                gl.delete_program(program);
            }
            gl.delete_vertex_array(self.vao);
            gl.delete_vertex_array(self.quad_vao);
            gl.delete_buffer(self.vbo);
        }
        if let Ok(banshee) = &mut self.banshee {
            banshee.destroy(gl);
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

    /// Why a Banshee's picture can't be composed here, if it can't.
    pub fn banshee_problem(&self) -> Option<&str> {
        self.banshee.as_ref().err().map(String::as_str)
    }

    /// Why a Banshee's 2D operations can't be done here (the card then
    /// does them in software), if its picture can be composed and they
    /// can't.
    pub fn gpu_2d_problem(&self) -> Option<&str> {
        self.banshee.as_ref().ok()?.blit.as_ref().err().map(String::as_str)
    }

    /// Draw `recording`; whether the picture may have changed with it.
    pub fn run(&mut self, gl: &glow::Context, mut recording: Recording) -> bool {
        let mut changed = std::mem::take(&mut self.ran_early)
            || !recording.commands.is_empty()
            || self.front != recording.front
            || self.clut != recording.clut;
        changed |= recording.scanout != self.scanout;
        self.scanout = recording.scanout.take();
        self.run_commands(gl, &recording.commands);
        self.front = recording.front;
        self.clut = recording.clut;
        changed
    }

    /// Do `commands`, part of a frame's recording that the card hands
    /// over before the frame ends (`gpu`): the picture may change with
    /// them, which the frame's `run` says.
    pub fn run_early(&mut self, gl: &glow::Context, commands: &[Command]) {
        self.ran_early |= !commands.is_empty();
        self.run_commands(gl, commands);
    }

    /// Bytes of the card's memory from byte `at` on (which starts a
    /// page) as the renderer has them, into `out`. False, with `out` as
    /// it was, if it hasn't them: it keeps no copy of the memory, or one
    /// too small.
    pub fn read_memory(&self, gl: &glow::Context, at: u32, out: &mut [u8]) -> bool {
        self.banshee.as_ref().is_ok_and(|b| b.memory.read(gl, at, out))
    }

    /// Rows `top..bottom` of a buffer's pixels as drawn, at the card's
    /// resolution, row by row from the top (`GpuLink::read_buffer`):
    /// colour buffer `key` in 5-6-5, or with None the depth buffer's
    /// 16-bit values. Each of the card's pixels is the sample nearest the
    /// middle of its block of `scale` by `scale`, which holds the pixel
    /// the card would draw there: an average would mix the edges of what
    /// was drawn. The colours are picked out on the GPU, so only the
    /// card's pixels come back. False if there is no such buffer, or
    /// `out` isn't the rows' size.
    pub fn read_buffer(
        &self,
        gl: &glow::Context,
        key: Option<u32>,
        (top, bottom): (u32, u32),
        out: &mut [u16],
    ) -> bool {
        let (w, h) = (self.width as usize, self.height as usize);
        let target = match key {
            Some(offs) => self.targets.get(&offs),
            None => self.depth.and(self.targets.values().next()),
        };
        let rows = (bottom as usize).saturating_sub(top as usize);
        let Some(target) =
            target.filter(|_| bottom as usize <= h && rows > 0 && out.len() == w * rows)
        else {
            return false;
        };
        let scale = self.scale.max(1) as usize;
        let (sw, sh) = self.scaled();
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
            if key.is_some() {
                // Only the rows asked for: a game reading the buffer
                // reads it a band at a time (`lfb_band`).
                let band = (top as i32 * scale as i32, bottom as i32 * scale as i32);
                target.resolve_rows(gl, sw, band);
                let mut rgba = vec![0u8; w * rows * 4];
                let (width, height) = (w as i32, rows as i32);
                // A blit without filtering to the card's size takes the
                // sample at the middle of each block.
                let picked = if scale > 1 {
                    let Some(picked) = pick(
                        gl,
                        target.fbo,
                        (sw, top as i32 * scale as i32),
                        scale as i32,
                        (width, height),
                    ) else {
                        restore(gl);
                        return false;
                    };
                    Some(picked)
                } else {
                    None
                };
                gl.bind_framebuffer(
                    glow::READ_FRAMEBUFFER,
                    Some(picked.map_or(target.fbo, |(fbo, _)| fbo)),
                );
                let y = if picked.is_some() { 0 } else { top as i32 };
                let pixels = glow::PixelPackData::Slice(Some(&mut rgba));
                gl.read_pixels(0, y, width, height, glow::RGBA, glow::UNSIGNED_BYTE, pixels);
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
                if let Some((fbo, texture)) = picked {
                    gl.delete_framebuffer(fbo);
                    gl.delete_texture(texture);
                }
                // The colours are 5-6-5 values widened by repeating their
                // top bits, which the top bits give back.
                for (value, rgba) in out.iter_mut().zip(rgba.as_chunks::<4>().0) {
                    let [r, g, b] = [rgba[0], rgba[1], rgba[2]].map(u16::from);
                    *value = (r >> 3) << 11 | (g >> 2) << 5 | b >> 3;
                }
            } else {
                let band = ((top as usize * scale) as i32, (rows * scale) as i32);
                let Some(depth) = self.read_depth(gl, target, (sw, sh), band) else {
                    restore(gl);
                    return false;
                };
                let middle = |x: usize, y: usize| {
                    (y * scale + scale / 2) * sw as usize + x * scale + scale / 2
                };
                // Depth is the card's value over 65535 (`triangle.frag`).
                for (i, value) in out.iter_mut().enumerate() {
                    *value = (depth[middle(i % w, i / w)] * 65535.0)
                        .round()
                        .clamp(0.0, 65535.0) as u16;
                }
            }
            restore(gl);
        }
        true
    }

    /// The depth buffer's values in the band of `height` rows from row
    /// `y` up of a buffer of size `(w, h)`: with multisampling through a
    /// framebuffer of one sample a pixel made for the purpose.
    ///
    /// # Safety
    /// See `VoodooGl`.
    unsafe fn read_depth(
        &self,
        gl: &glow::Context,
        target: &Target,
        (w, h): (i32, i32),
        (y, height): (i32, i32),
    ) -> Option<Vec<f32>> {
        let mut depth = vec![0f32; w as usize * height as usize];
        // SAFETY: the caller's.
        unsafe {
            let resolved = match target.samples {
                None => None,
                Some((samples, _)) => {
                    let texture = texture(gl, glow::NEAREST).ok()?;
                    let none = glow::PixelUnpackData::Slice(None);
                    let format = glow::DEPTH24_STENCIL8 as i32;
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        format,
                        w,
                        h,
                        0,
                        glow::DEPTH_STENCIL,
                        glow::UNSIGNED_INT_24_8,
                        none,
                    );
                    let Ok(fbo) = gl.create_framebuffer() else {
                        gl.delete_texture(texture);
                        return None;
                    };
                    gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(fbo));
                    let attach = glow::DEPTH_STENCIL_ATTACHMENT;
                    gl.framebuffer_texture_2d(
                        glow::DRAW_FRAMEBUFFER,
                        attach,
                        glow::TEXTURE_2D,
                        Some(texture),
                        0,
                    );
                    gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(samples));
                    let (top, bottom) = (y, y + height);
                    gl.blit_framebuffer(
                        0,
                        top,
                        w,
                        bottom,
                        0,
                        top,
                        w,
                        bottom,
                        glow::DEPTH_BUFFER_BIT,
                        glow::NEAREST,
                    );
                    Some((fbo, texture))
                }
            };
            gl.bind_framebuffer(
                glow::READ_FRAMEBUFFER,
                Some(resolved.map_or(target.fbo, |(fbo, _)| fbo)),
            );
            let pixels = glow::PixelPackData::Slice(Some(bytemuck_f32(&mut depth)));
            gl.read_pixels(0, y, w, height, glow::DEPTH_COMPONENT, glow::FLOAT, pixels);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            if let Some((fbo, texture)) = resolved {
                gl.delete_framebuffer(fbo);
                gl.delete_texture(texture);
            }
        }
        Some(depth)
    }

    fn run_commands(&mut self, gl: &glow::Context, commands: &[Command]) {
        for command in commands {
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
                Command::Memory { size, at, bytes } => {
                    if let Ok(banshee) = &mut self.banshee {
                        banshee.memory.put(gl, *size, *at, bytes);
                    }
                }
                Command::Blit(blit) => {
                    if let BlitSource::Buffer { buffer, .. } = blit.src {
                        self.flush_staging(gl, Some(buffer));
                    }
                    self.blit(gl, blit);
                }
            }
        }
        let keys: Vec<_> = self.staging.keys().copied().collect();
        for key in keys {
            self.flush_staging(gl, key);
        }
        restore(gl);
    }

    /// A 2D operation in the card's memory (`mirror::Blit`).
    fn blit(&mut self, gl: &glow::Context, blit: &Blit) {
        let scaled = self.scaled();
        let buffer = match blit.src {
            BlitSource::Buffer { buffer, .. } => {
                let Some(target) = self.targets.get(&buffer) else { return };
                target.resolve(gl, scaled);
                Some(target.color)
            }
            _ => None,
        };
        let Ok(banshee) = &mut self.banshee else { return };
        let Ok(p) = &banshee.blit else { return };
        let memory = &banshee.memory;
        let layout = memory.layout;
        let rows = |(a, b): (u32, u32)| (memory.row(a), memory.row(b.saturating_sub(1)) + 1);
        // What the blit reads, as it was before it: the source, and the
        // destination only for its key.
        if blit.dst_key.is_some() {
            let (first, last) = rows(blit.dst_span());
            memory.copy_rows(gl, first, last);
        }
        if let Some(span) = blit.src_span() {
            let (first, last) = rows(span);
            memory.copy_rows(gl, first, last);
        }
        let i = |v: u32| v as i32;
        let key = |k: &Option<Key>| match k {
            Some(k) => ([i(k.low), i(k.high), 1], k.masks.map(i)),
            None => ([0; 3], [0; 3]),
        };
        let (src_key, src_masks) = key(&blit.src_key);
        let (dst_key, dst_masks) = key(&blit.dst_key);
        let (kind, plane, rect, fill) = match &blit.src {
            BlitSource::Fill(v) => (0, None, [0; 4], *v),
            BlitSource::Memory { plane, rect } => (1, Some(*plane), *rect, 0),
            BlitSource::Buffer { rect, .. } => (2, None, *rect, 0),
        };
        let src = plane.map_or([kind, 0, 0, 1], |p| [kind, i(p.start), i(p.stride), i(p.bpp)]);
        let convert = match blit.convert {
            Convert::None => 0,
            Convert::Widen => 1,
            Convert::Narrow => 2,
            Convert::ClearTop => 3,
        };
        let (w, h) = (layout.width() as i32, layout.layer_rows() as i32);
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.viewport(0, 0, w, h);
            gl.disable(glow::BLEND);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::SCISSOR_TEST);
            gl.color_mask(true, true, true, true);
            gl.use_program(Some(p.program));
            let u = &p.uniforms;
            u.u_size.set(gl, [w as f32, h as f32]);
            u.u_mem_layout.set(gl, layout.uniform(memory.size));
            let d = &blit.dst;
            u.u_dst.set(gl, [i(d.start), i(d.stride.max(1)), i(d.bpp.clamp(1, 4))]);
            u.u_area.set(gl, blit.area);
            u.u_src.set(gl, src);
            u.u_src_rect.set(gl, rect);
            u.u_fill.set(gl, i(fill));
            let [mx, my] = blit.mirror.map(i32::from);
            u.u_mode.set(gl, [mx, my, convert, self.scale as i32]);
            u.u_src_key.set(gl, src_key);
            u.u_src_masks.set(gl, src_masks);
            u.u_dst_key.set(gl, dst_key);
            u.u_dst_masks.set(gl, dst_masks);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(memory.copy));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(buffer.unwrap_or(self.lut)));
            gl.bind_vertex_array(Some(self.quad_vao));
            // The layer drawn into last: most rectangles stay in it.
            let mut bound = None;
            for &r in &blit.rects {
                let (first, last) = rows(blit.dst.span(r));
                u.u_clip.set(gl, r);
                for (layer, top, bottom) in layout.spans(first, last) {
                    if bound != Some(layer) {
                        let fbo = memory.fbos[layer as usize];
                        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                        u.u_layer.set(gl, layer as i32);
                        bound = Some(layer);
                    }
                    u.u_rect.set(gl, [0.0, top as f32, w as f32, bottom as f32]);
                    gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                }
            }
            gl.active_texture(glow::TEXTURE0);
        }
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
        for (out, &value) in staging.rgba.as_chunks_mut::<4>().0.iter_mut().zip(pixels) {
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
            program.uniforms.u_rect.set(gl, [x0 as f32, y0 as f32, x1 as f32, y1 as f32]);
            program.uniforms.u_size.set(gl, [self.width as f32, self.height as f32]);
            program.uniforms.u_scale.set(gl, scale as f32);
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
        if let std::collections::hash_map::Entry::Vacant(e) = self.textures.entry(texture.id) {
            let Ok(t) = self::texture(gl, glow::NEAREST) else { return };
            e.insert(GlTexture {
                texture: t,
                levels: Vec::new(),
                level_count: 0,
                params: None,
                lod: None,
                anisotropy: None,
            });
        }
        let Some(entry) = self.textures.get_mut(&texture.id) else { return };
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(entry.texture));
            if entry.levels.len() < texture.levels.len() {
                entry.levels.resize(texture.levels.len(), (0, 0));
            }
            for (i, level) in texture.levels.iter().enumerate() {
                self.texture_bytes.clear();
                self.texture_bytes.extend(
                    level
                        .argb
                        .iter()
                        .flat_map(|&c| [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]),
                );
                let size = (level.width, level.height);
                if entry.levels[i] == size {
                    gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        i as i32,
                        0,
                        0,
                        level.width as i32,
                        level.height as i32,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(&self.texture_bytes)),
                    );
                } else {
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        i as i32,
                        glow::RGBA8 as i32,
                        level.width as i32,
                        level.height as i32,
                        0,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(&self.texture_bytes)),
                    );
                    entry.levels[i] = size;
                }
            }
            if entry.level_count != texture.levels.len() {
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAX_LEVEL,
                    texture.levels.len() as i32 - 1,
                );
                entry.level_count = texture.levels.len();
            }
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
            p.uniforms.u_size.set(gl, [self.width as f32, self.height as f32]);
            for (uniform, value) in [
                (&p.uniforms.u_fbzcp, s.fbz_color_path),
                (&p.uniforms.u_fbz, s.fbz_mode),
                (&p.uniforms.u_alpha, s.alpha_mode),
                (&p.uniforms.u_fog, s.fog_mode),
                (&p.uniforms.u_color0, s.color0),
                (&p.uniforms.u_color1, s.color1),
                (&p.uniforms.u_chroma, s.chroma_key),
                (&p.uniforms.u_chroma_range, s.chroma_range),
                (&p.uniforms.u_zacolor, s.za_color),
                (&p.uniforms.u_fogcolor, s.fog_color),
                (&p.uniforms.u_stipple, s.stipple),
                (&p.uniforms.u_yorigin, s.yorigin),
                (&p.uniforms.u_config, s.send_config.unwrap_or(0)),
            ] {
                uniform.set(gl, value as i32);
            }
            p.uniforms.u_fogblend.set(gl, s.fogblend);
            p.uniforms.u_fogdelta.set(gl, s.fogdelta);
            p.uniforms.u_scale.set(gl, scale);
            p.uniforms.u_dither.set(gl, self.dither as i32);
            let mut units = 0;
            for (unit, t) in s.tmu.iter().enumerate() {
                let Some(t) = t else { continue };
                let Some(texture) = self.textures.get_mut(&t.texture) else { continue };
                units |= 1 << unit;
                gl.active_texture(glow::TEXTURE1 + unit as u32);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture.texture));
                if let Some(anisotropy) = self.texture_anisotropy
                    && texture.anisotropy != Some(anisotropy)
                {
                    gl.tex_parameter_f32(glow::TEXTURE_2D, TEXTURE_MAX_ANISOTROPY, anisotropy);
                    texture.anisotropy = Some(anisotropy);
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
                let u = &p.uniforms;
                let (mode, size, lods, detail) = [
                    (&u.u_tmode0, &u.u_tsize0, &u.u_tlod0, &u.u_tdetail0),
                    (&u.u_tmode1, &u.u_tsize1, &u.u_tlod1, &u.u_tdetail1),
                ][unit];
                mode.set(gl, t.mode as i32);
                size.set(gl, [t.width as f32, t.height as f32, t.lodbias as f32 / 256.0]);
                lods.set(gl, [t.lodmin, t.lodmax, t.lodbias]);
                detail.set(gl, [t.detailbias, t.detailmax, t.detailscale as i32]);
            }
            if s.send_config.is_some() && units & 1 != 0 {
                units |= 4;
            }
            p.uniforms.u_units.set(gl, units);
            p.uniforms.u_constant_depth.set(gl, constant_depth as i32);

            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, vertex_bytes(&draw.vertices), glow::STREAM_DRAW);
            if constant_depth && depth_write {
                // The card compares zaColor but stores the interpolated
                // depth. GL uses gl_FragDepth for both: record which samples
                // pass with stencil, then write their real depth separately.
                // Keep overlapping triangles in order, including with MSAA:
                // each gets its own reference, so the buffer is cleared only
                // every 255 triangles.
                gl.enable(glow::STENCIL_TEST);
                for (n, first) in (0..draw.vertices.len() as i32).step_by(3).enumerate() {
                    let reference = (n % 255) as i32 + 1;
                    gl.stencil_mask(0xFF);
                    if reference == 1 {
                        gl.clear_stencil(0);
                        gl.clear(glow::STENCIL_BUFFER_BIT);
                    }
                    gl.stencil_func(glow::ALWAYS, reference, 0xFF);
                    gl.stencil_op(glow::KEEP, glow::KEEP, glow::REPLACE);
                    gl.depth_func(DEPTH_FUNCS[func]);
                    gl.depth_mask(false);
                    gl.color_mask(rgb, rgb, rgb, false);
                    p.uniforms.u_constant_depth.set(gl, 1);
                    gl.draw_arrays(glow::TRIANGLES, first, 3);

                    gl.stencil_mask(0);
                    gl.stencil_func(glow::EQUAL, reference, 0xFF);
                    gl.stencil_op(glow::KEEP, glow::KEEP, glow::KEEP);
                    gl.depth_func(glow::ALWAYS);
                    gl.depth_mask(true);
                    gl.color_mask(false, false, false, false);
                    p.uniforms.u_constant_depth.set(gl, 0);
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
            if let Some(pattern) =
                fill.pattern.as_deref().filter(|_| self.dither && fill.color.is_some())
            {
                // The colour as the card dithers it, through the clip.
                gl.color_mask(true, true, true, false);
                gl.disable(glow::DEPTH_TEST);
                let (w, h) = self.scaled();
                gl.viewport(0, 0, w, h);
                let p = &self.fill_program;
                let size = (self.width as f32, self.height as f32);
                gl.use_program(Some(p.program));
                p.uniforms.u_rect.set(gl, [0.0, 0.0, size.0, size.1]);
                p.uniforms.u_size.set(gl, [size.0, size.1]);
                p.uniforms.u_scale.set(gl, self.scale as f32);
                p.uniforms.u_pattern.set(gl, *pattern);
                gl.bind_vertex_array(Some(self.quad_vao));
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            } else if let Some(color) = fill.color {
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
        if self.scanout.is_some() {
            return self.compose(gl, screen, base);
        }
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
            p.uniforms.u_rect.set(gl, [0.0, 0.0, self.width as f32, self.height as f32]);
            p.uniforms.u_size.set(gl, [self.width as f32, self.height as f32]);
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.lut));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(front));
            gl.bind_vertex_array(Some(self.quad_vao));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            self.draw_over(
                gl,
                screen,
                base,
                self.staging_texture,
                (self.width, self.height),
            );
            restore(gl);
            Some((composite.color, size))
        }
    }

    /// The picture last composed (`composite`), read back from the GPU
    /// at the card's resolution: of each block of `scale` by `scale`
    /// pixels, their average. Rows from the top, RGBA.
    pub fn read_picture(&self, gl: &glow::Context) -> Option<Frame> {
        let (target, (w, h)) = self.composite.as_ref()?;
        let (w, h) = (*w, *h);
        let scale = self.scale.max(1);
        let mut rgba = vec![0u8; w as usize * h as usize * 4];
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(target.fbo));
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
            let pixels = glow::PixelPackData::Slice(Some(&mut rgba));
            let (rw, rh) = size_i32((w, h));
            gl.read_pixels(0, 0, rw, rh, glow::RGBA, glow::UNSIGNED_BYTE, pixels);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
        }
        // The composed picture's first row is the picture's top. Each of
        // the card's pixels is the average of its block of scale by scale,
        // as the player sees it at that size.
        let (fw, fh) = (w / scale, h / scale);
        let mut frame = Frame::new(fw, fh);
        let area = scale * scale;
        let mut sums = vec![[0u32; 3]; fw as usize];
        for y in 0..fh {
            sums.fill([0; 3]);
            for row in y * scale..(y + 1) * scale {
                let from = (row * w) as usize * 4;
                for (x, sum) in sums.iter_mut().enumerate() {
                    let at = from + x * scale as usize * 4;
                    for px in rgba[at..at + scale as usize * 4].as_chunks::<4>().0 {
                        sum.iter_mut().zip(px).for_each(|(s, &c)| *s += c as u32);
                    }
                }
            }
            let to = (y * fw) as usize * 4;
            let out = &mut frame.rgba[to..to + fw as usize * 4];
            for (px, sum) in out.as_chunks_mut::<4>().0.iter_mut().zip(&sums) {
                let [r, g, b] = sum.map(|s| ((s + area / 2) / area) as u8);
                *px = [r, g, b, 0xFF];
            }
        }
        Some(frame)
    }

    /// Draw over the picture being composed, of `size` of the card's
    /// pixels, `screen`'s pixels where they differ from `base`, through
    /// `texture`, as big as the picture.
    fn draw_over(
        &self,
        gl: &glow::Context,
        screen: &Frame,
        base: &Frame,
        texture: glow::Texture,
        size: (u32, u32),
    ) {
        let fits = (screen.width, screen.height) == size && (base.width, base.height) == size;
        if !fits || screen.rgba == base.rgba {
            return;
        }
        let over: Vec<u8> = screen
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(base.rgba.as_chunks::<4>().0)
            .flat_map(|(&[r, g, b, _], old)| {
                if [r, g, b] != old[..3] {
                    [r, g, b, 0xFF]
                } else {
                    [0; 4]
                }
            })
            .collect();
        let (w, h) = (size.0 as f32, size.1 as f32);
        // SAFETY: see `VoodooGl`.
        unsafe {
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            let pixels = glow::PixelUnpackData::Slice(Some(&over));
            let (nw, nh) = size_i32(size);
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                nw,
                nh,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                pixels,
            );
            let p = &self.pixels;
            gl.use_program(Some(p.program));
            p.uniforms.u_rect.set(gl, [0.0, 0.0, w, h]);
            p.uniforms.u_size.set(gl, [w, h]);
            p.uniforms.u_scale.set(gl, self.scale as f32);
            gl.bind_vertex_array(Some(self.quad_vao));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        }
    }

    /// A Banshee's or Voodoo3's picture (`Scanout`), composed as its
    /// video processor does: the desktop, the overlay and the cursor, each
    /// surface from the buffer the renderer draws or from the memory that
    /// came with the scanout, at `scale` times the picture's size (the
    /// desktop's, twice as wide when its pixels are wide). Then what
    /// `screen` has over `base`, as `composite`.
    fn compose(
        &mut self,
        gl: &glow::Context,
        screen: &Frame,
        base: &Frame,
    ) -> Option<(glow::Texture, (u32, u32))> {
        let scaled = self.scaled();
        let s = self.scanout.as_deref()?;
        let banshee = self.banshee.as_mut().ok()?;
        let across = if s.wide { 2 } else { 1 };
        let (w, h) = (s.width.max(1) * across, s.height.max(1));
        let size = (w * self.scale, h * self.scale);
        let buffer = |surface: &Surface| surface.buffer.and_then(|b| self.targets.get(&b));
        let desktop = buffer(&s.desktop);
        let overlay = s.overlay.as_ref().and_then(|o| buffer(&o.surface));
        for target in [desktop, overlay].into_iter().flatten() {
            target.resolve(gl, scaled);
        }
        let (desktop, overlay) = (desktop.map(|t| t.color), overlay.map(|t| t.color));
        // SAFETY: see `VoodooGl`.
        unsafe {
            if self.composite.as_ref().is_none_or(|(_, old)| *old != size) {
                if let Some((old, _)) = self.composite.take() {
                    old.delete(gl);
                }
                self.composite = Some((target(gl, size_i32(size), 1, None).ok()?, size));
            }
            if banshee
                .screen
                .as_ref()
                .is_none_or(|(_, old)| *old != (w, h))
            {
                if let Some((old, _)) = banshee.screen.take() {
                    gl.delete_texture(old);
                }
                let t = texture(gl, glow::NEAREST).ok()?;
                let none = glow::PixelUnpackData::Slice(None);
                let (nw, nh) = size_i32((w, h));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    nw,
                    nh,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    none,
                );
                banshee.screen = Some((t, (w, h)));
            }
            // The CLUT, and the cursor's planes.
            let clut: Vec<u8> = s
                .clut
                .iter()
                .flat_map(|&c| [(c >> 16) as u8, (c >> 8) as u8, c as u8, 0xFF])
                .collect();
            gl.bind_texture(glow::TEXTURE_2D, Some(banshee.clut));
            let pixels = glow::PixelUnpackData::Slice(Some(&clut));
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                512,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                pixels,
            );
            if let Some(cursor) = s.cursor.as_ref().filter(|c| c.pattern.len() == 1024) {
                gl.bind_texture(glow::TEXTURE_2D, Some(banshee.cursor));
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                let pixels = glow::PixelUnpackData::Slice(Some(&cursor.pattern));
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    16,
                    64,
                    glow::RED_INTEGER,
                    glow::UNSIGNED_BYTE,
                    pixels,
                );
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            }

            let (composite, _) = self.composite.as_ref()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(composite.fbo));
            gl.viewport(0, 0, size.0 as i32, size.1 as i32);
            gl.disable(glow::BLEND);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::SCISSOR_TEST);
            gl.color_mask(true, true, true, true);
            let p = &banshee.program;
            gl.use_program(Some(p.program));
            let u = &p.uniforms;
            u.u_rect.set(gl, [0.0, 0.0, w as f32, h as f32]);
            u.u_size.set(gl, [w as f32, h as f32]);
            u.u_scale.set(gl, self.scale as i32);
            u.u_across.set(gl, across as i32);
            let i = |v: u32| v as i32;
            let memory = &banshee.memory;
            u.u_mem_layout.set(gl, memory.layout.uniform(memory.size));
            let d = &s.desktop;
            let bypass = d.bypass as i32;
            u.u_desk.set(gl, [i(d.format), i(d.clut), bypass, desktop.is_some() as i32]);
            let start = i(d.start);
            u.u_desk_layout.set(gl, [i(d.stride), d.tiled as i32, s.half as i32, start]);
            match &s.overlay {
                Some(o) => {
                    let v = &o.surface;
                    let bypass = v.bypass as i32;
                    u.u_ovl.set(gl, [i(v.format), i(v.clut), bypass, overlay.is_some() as i32]);
                    let start = i(v.start);
                    u.u_ovl_layout.set(gl, [i(v.stride), v.tiled as i32, 0, start]);
                    u.u_ovl_rect.set(gl, o.rect.map(i));
                    u.u_ovl_step.set(gl, [i(o.dudx), i(o.dvdy), i(o.v_offset)]);
                }
                None => u.u_ovl.set(gl, [0, 0, 0, -1]),
            }
            match &s.cursor {
                Some(c) => {
                    u.u_cur.set(gl, [c.x, c.y, 1, c.x11 as i32]);
                    let rgb = |c: u32| {
                        [
                            (c >> 16 & 0xFF) as f32 / 255.0,
                            (c >> 8 & 0xFF) as f32 / 255.0,
                            (c & 0xFF) as f32 / 255.0,
                        ]
                    };
                    u.u_cur_c0.set(gl, rgb(c.colors[0]));
                    u.u_cur_c1.set(gl, rgb(c.colors[1]));
                }
                None => u.u_cur.set(gl, [0, 0, 0, 0]),
            }
            // Every sampler has a texture of its kind, used or not: the
            // memory on unit 0, 2D textures on the others.
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(memory.texture));
            let units = [
                desktop.unwrap_or(self.lut),
                overlay.unwrap_or(self.lut),
                banshee.cursor,
                banshee.clut,
            ];
            for (unit, texture) in (1..).zip(units) {
                gl.active_texture(glow::TEXTURE0 + unit);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            }
            gl.bind_vertex_array(Some(self.quad_vao));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            let screen_texture = banshee.screen.as_ref()?.0;
            self.draw_over(gl, screen, base, screen_texture, (w, h));
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

/// A framebuffer with a colour texture of `size`, which gets the band of
/// `fbo` (a buffer `width` wide, at `scale` times the card's size) from
/// row `y` up, one sample of each block of `scale` by `scale`: the one
/// at its middle, as a blit without filtering takes it.
///
/// # Safety
/// See `VoodooGl`.
unsafe fn pick(
    gl: &glow::Context,
    fbo: glow::Framebuffer,
    (width, y): (i32, i32),
    scale: i32,
    (w, h): (i32, i32),
) -> Option<(glow::Framebuffer, glow::Texture)> {
    // SAFETY: the caller's.
    unsafe {
        let texture = texture(gl, glow::NEAREST).ok()?;
        let none = glow::PixelUnpackData::Slice(None);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            w,
            h,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            none,
        );
        let Ok(picked) = gl.create_framebuffer() else {
            gl.delete_texture(texture);
            return None;
        };
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(picked));
        gl.framebuffer_texture_2d(
            glow::DRAW_FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(texture),
            0,
        );
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(fbo));
        gl.blit_framebuffer(
            0,
            y,
            width,
            y + h * scale,
            0,
            0,
            w,
            h,
            glow::COLOR_BUFFER_BIT,
            glow::NEAREST,
        );
        Some((picked, texture))
    }
}

/// `values` as the bytes they are, for OpenGL to read into.
fn bytemuck_f32(values: &mut [f32]) -> &mut [u8] {
    // SAFETY: any bytes are a valid f32, and the slice covers exactly
    // the values' memory.
    unsafe {
        std::slice::from_raw_parts_mut(
            values.as_mut_ptr().cast::<u8>(),
            std::mem::size_of_val(values),
        )
    }
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
    texture_of(gl, glow::TEXTURE_2D, filter)
}

/// A 2D array texture, unfiltered and clamped at its edges, bound.
fn texture_array(gl: &glow::Context) -> Result<glow::Texture, String> {
    texture_of(gl, glow::TEXTURE_2D_ARRAY, glow::NEAREST)
}

/// A texture of `target` with `filter`, clamped at its edges, bound.
fn texture_of(gl: &glow::Context, target: u32, filter: u32) -> Result<glow::Texture, String> {
    // SAFETY: see `VoodooGl`.
    unsafe {
        let texture = gl.create_texture()?;
        gl.bind_texture(target, Some(texture));
        for (name, value) in [
            (glow::TEXTURE_MIN_FILTER, filter),
            (glow::TEXTURE_MAG_FILTER, filter),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(target, name, value as i32);
        }
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
fn compile<U: Locations>(
    gl: &glow::Context,
    glsl: Glsl,
    vertex: &str,
    fragment: &str,
    attributes: &[(u32, &str)],
) -> Result<Program<U>, String> {
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
        Ok(Program { program, uniforms: U::resolve(gl, program) })
    }
}

#[cfg(test)]
mod tests {
    use super::sampling_filters;

    #[test]
    #[ignore = "requires a desktop OpenGL context"]
    fn texture_updates_replace_pixels_and_resize_mip_storage() {
        use super::*;
        use crate::voodoo::mirror::Level;
        let sdl = sdl2::init().unwrap();
        let video = sdl.video().unwrap();
        let attrs = video.gl_attr();
        attrs.set_context_profile(sdl2::video::GLProfile::Core);
        attrs.set_context_version(3, 3);
        let window = video
            .window("Texture regression", 8, 8)
            .opengl()
            .hidden()
            .build()
            .unwrap();
        let _context = window.gl_create_context().unwrap();
        // SAFETY: this thread owns the current SDL OpenGL context.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| video.gl_get_proc_address(name).cast())
        };
        let mut renderer = VoodooGl::new(&gl, Glsl::Gl150, 1, 1, 1).unwrap();
        for (width, height, count, color) in [
            (4, 4, 2, 0xFF11_2233u32),
            (4, 4, 2, 0x8044_5566),
            (2, 2, 1, 0x4077_8899),
            (4, 4, 2, 0xFFAA_BBCC),
        ] {
            let levels: Vec<_> = (0..count)
                .map(|i| Level {
                    width: width >> i,
                    height: height >> i,
                    argb: vec![color; ((width >> i) * (height >> i)) as usize],
                })
                .collect();
            renderer.upload(
                &gl,
                &Texture {
                    id: 7,
                    levels: levels.clone(),
                },
            );
            let entry = &renderer.textures[&7];
            // SAFETY: the context and texture belong to this thread.
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D, Some(entry.texture));
                assert_eq!(
                    gl.get_tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL),
                    count - 1
                );
                for (i, level) in levels.iter().enumerate() {
                    let mut pixels = vec![0; (level.width * level.height * 4) as usize];
                    gl.get_tex_image(
                        glow::TEXTURE_2D,
                        i as i32,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelPackData::Slice(Some(&mut pixels)),
                    );
                    let expected = [
                        (color >> 16) as u8,
                        (color >> 8) as u8,
                        color as u8,
                        (color >> 24) as u8,
                    ];
                    assert!(
                        pixels
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .all(|pixel| *pixel == expected)
                    );
                }
                assert_eq!(gl.get_error(), glow::NO_ERROR);
            }
        }
        renderer.destroy(&gl);
    }

    #[test]
    #[ignore = "requires a desktop OpenGL context"]
    fn constant_depth_comparison_writes_interpolated_depth() {
        use super::*;
        use crate::voodoo::mirror::{DrawState, Layout};
        use crate::voodoo::raster::Stride;
        let t = test_context("Depth regression");
        let gl = t.gl;
        for samples in [1, 4] {
            let mut renderer = VoodooGl::new(&gl, Glsl::Gl150, 1, samples, 1).unwrap();
            let state = DrawState {
                dest: 0, fbz_color_path: 0, fbz_mode: (1 << 4) | (5 << 5) | (1 << 9) | (1 << 10) | (1 << 20),
                alpha_mode: 0, fog_mode: 0, za_color: 0, chroma_key: 0, chroma_range: 0,
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
                        layout: Layout {
                            width: 8,
                            height: 8,
                            stride: Stride::linear(8),
                            aux_stride: Stride::linear(8),
                            color: vec![0, 64],
                            aux: Some(128),
                        },
                        color: vec![vec![0; 64]; 2], aux: Some(vec![0xFFFF; 64]),
                    })),
                    Command::Draw(Box::new(Draw { state, vertices })),
                ],
                front: Some(0), output: true, width: 8, height: 8, clut: [0; 33], scanout: None,
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
    /// RUST_DOS_VOODOO_DRIVE_C redirects its C: image to a scratch disk.
    #[test]
    #[ignore = "requires a saved Glide scene and a desktop OpenGL context"]
    fn replay_saved_glide_scene() {
        use super::*;
        use crate::cpu::Cpu;
        use crate::exec::{NoHook, run_batch};
        use crate::savestate::{machine, slots};
        use std::{fs, path::PathBuf};

        let path = std::env::var_os("RUST_DOS_VOODOO_STATE").expect("RUST_DOS_VOODOO_STATE");
        let (header, state) = slots::decode(&fs::read(&path).unwrap()).unwrap();
        let settings = slots::machine_settings(&header.machine, &Default::default());
        let mut cpu = Cpu::with_memory(PathBuf::from("."), header.memsize);
        crate::hardware::configure(&mut cpu, &settings, crate::keylayout::Layout::us());
        if let Some(image) = std::env::var_os("RUST_DOS_VOODOO_DRIVE_C") {
            cpu.bus.disk.folders_from_state.push((crate::disk::DRIVE_C, image.into()));
        }
        // A booted system's state wants its disk as it was: a copy beside it.
        crate::savestate::disks::offer_copies(&mut cpu, std::path::Path::new(&path));
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
                let mut rgba = vec![0; (w * h * 4) as usize];
                card.render(&mut rgba, w as usize);
                let reference: Vec<_> = rgba.as_chunks::<4>().0.iter()
                    .flat_map(|&[r, g, b, _]| [r, g, b]).collect();
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
                // The target holds the colours before the gamma table, which the
                // presenting pass applies; the software picture has it applied.
                let table = super::super::gamma_table(&renderer.clut);
                for rgb in rendered.as_chunks_mut::<3>().0 {
                    *rgb = [table[rgb[0] as usize][0], table[rgb[1] as usize][1], table[rgb[2] as usize][2]];
                }
                save(&format!("glide-{frame}-opengl"), &rendered, w, h);
                let bad = reference.as_chunks::<3>().0.iter().zip(rendered.as_chunks::<3>().0)
                    .filter(|(a, b)| a.iter().zip(*b).any(|(a, b)| a.abs_diff(*b) > 32)).count();
                println!("frame {frame}: {bad} pixels differ by more than 32");
            }
        }
        renderer.destroy(&gl);
    }

    /// A Banshee's picture composed with OpenGL is the software one, pixel
    /// for pixel: desktops of every format, linear and tiled, the overlay
    /// in YUV, 8 bits and 5-6-5 with and without scaling, the cursor in
    /// both modes, and surfaces that are buffers the renderer draws. So
    /// is it with the memory in many small textures, and without the 2D
    /// operations' program.
    #[test]
    #[ignore = "requires a desktop OpenGL context"]
    fn banshee_composition_matches_the_software_picture() {
        use super::*;
        use crate::voodoo::banshee::render3d::{
            COL_BUFFER_ADDR, COL_BUFFER_STRIDE, LEFT_OVERLAY_BUF,
        };
        use crate::voodoo::banshee::*;
        use crate::voodoo::regs::SWAPBUFFER_CMD;
        use crate::voodoo::{Board, Now, Voodoo};

        let sdl = sdl2::init().unwrap();
        let video = sdl.video().unwrap();
        let attrs = video.gl_attr();
        attrs.set_context_profile(sdl2::video::GLProfile::Core);
        attrs.set_context_version(3, 3);
        let window = video
            .window("Banshee composition", 8, 8)
            .opengl()
            .hidden()
            .build()
            .unwrap();
        let _context = window.gl_create_context().unwrap();
        // SAFETY: this thread owns the current SDL OpenGL context.
        let gl =
            unsafe { glow::Context::from_loader_function(|s| video.gl_get_proc_address(s).cast()) };

        // A 160x100 desktop at 1000h, memory and CLUT of noise. It is
        // narrow, so the picture shows each pixel twice across.
        let (w, h) = (160usize, 100usize);
        let card = |cfg: u32, stride: u32, overlay: Option<(u32, u32, [u32; 2])>| {
            let mut v = Voodoo::with_workers(Board::Voodoo3, 0);
            let now = Now::default();
            let mut seed = 0x1234_5678u32;
            let mut noise = move || {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                seed >> 24
            };
            for a in 0..0x10_0000 {
                v.fbi.ram.set_byte(a, noise() as u8);
            }
            let b = v.banshee.as_mut().unwrap();
            b.crtc[0x01] = (w / 8 - 1) as u8;
            b.crtc[0x12] = (h - 1) as u8;
            for c in b.clut.iter_mut() {
                *c = noise() << 16 | noise() << 8 | noise();
            }
            let reg = |v: &mut Voodoo, r: u32, value: u32| v.write(r, value, u32::MAX, now);
            reg(&mut v, VGA_INIT0, VGAINIT0_EXTENDED_SHIFT_OUT);
            reg(&mut v, VID_PROC_CFG, VIDPROC_ENABLE | cfg);
            reg(&mut v, VID_DESKTOP_START_ADDR, 0x1000);
            reg(&mut v, HW_CUR_PAT_ADDR, 0x2_0000);
            reg(&mut v, HW_CUR_LOC, (64 + 30) | (64 + 40) << 16);
            reg(&mut v, HW_CUR_C0, 0x12_3456);
            reg(&mut v, HW_CUR_C1, 0xAB_CDEF);
            let mut strides = stride;
            if let Some((addr, overlay_stride, [start, end])) = overlay {
                v.banshee.as_mut().unwrap().overlay_addr = addr;
                strides |= overlay_stride << 16;
                reg(&mut v, VID_OVERLAY_START_COORDS, start);
                reg(&mut v, VID_OVERLAY_END_SCREEN_COORDS, end);
                reg(&mut v, VID_OVERLAY_DUDX, 0x8_0000);
                reg(&mut v, VID_OVERLAY_DVDY, 0x6_0000);
                reg(&mut v, VID_OVERLAY_DVDY_OFFSET, 0x3_0000);
            }
            reg(&mut v, VID_DESKTOP_OVERLAY_STRIDE, strides);
            v
        };
        let at = |x: u32, y: u32| x | y << 12;
        let scaled = VIDPROC_H_SCALE | VIDPROC_V_SCALE;
        let cases: Vec<(&str, Voodoo)> = vec![
            ("8 bits, linear", card(0, 200, None)),
            (
                "5-6-5, tiled, CLUT high",
                card(
                    1 << 18 | VIDPROC_DESKTOP_TILED | VIDPROC_DESKTOP_CLUT_SEL,
                    3,
                    None,
                ),
            ),
            (
                "24 bits, linear, half mode",
                card(2 << 18 | VIDPROC_HALF_MODE, 496, None),
            ),
            (
                "32 bits, tiled, cursor",
                card(3 << 18 | VIDPROC_DESKTOP_TILED | VIDPROC_CURSOR, 5, None),
            ),
            (
                "YUYV overlay, scaled, cursor",
                card(
                    1 << 18
                        | VIDPROC_DESKTOP_CLUT_BYPASS
                        | VIDPROC_OVERLAY_ENABLE
                        | 5 << 21
                        | scaled
                        | VIDPROC_CURSOR,
                    320,
                    Some((0x4_0000, 400, [at(10, 20), at(120, 90)])),
                ),
            ),
            (
                "UYVY overlay, tiled, whole screen, X11 cursor",
                card(
                    1 << 18
                        | VIDPROC_OVERLAY_ENABLE
                        | 6 << 21
                        | VIDPROC_OVERLAY_TILED
                        | VIDPROC_CURSOR
                        | VIDPROC_CURSOR_X11,
                    320,
                    Some((0x6_0000, 4, [at(0, 0), at(159, 99)])),
                ),
            ),
            (
                "8-bit overlay, CLUT high, scaled",
                card(
                    VIDPROC_OVERLAY_ENABLE | VIDPROC_OVERLAY_CLUT_SEL | scaled,
                    160,
                    Some((0x8_0000, 160, [at(40, 10), at(170, 120)])),
                ),
            ),
            (
                "5-6-5 overlay, linear, bypassed",
                card(
                    1 << 18 | VIDPROC_OVERLAY_ENABLE | 1 << 21 | VIDPROC_OVERLAY_CLUT_BYPASS,
                    320,
                    Some((0x8_0000, 320, [at(5, 5), at(100, 60)])),
                ),
            ),
            ("8 bits, 480 pixels across, not widened", {
                let mut v = card(0, 480, None);
                // 60 characters of 8 pixels.
                v.banshee.as_mut().unwrap().crtc[0x01] = 59;
                v
            }),
        ];
        let mut cases = cases;
        // A 3D buffer the overlay shows, as it is and scaled, and one the
        // desktop is.
        let reg3d = |v: &mut Voodoo, r: usize, value: u32| {
            v.write(0x20_0000 + 4 * r as u32, value, u32::MAX, Now::default())
        };
        for (name, scaling) in [
            ("5-6-5 overlay from a 3D buffer", 0),
            ("5-6-5 overlay from a 3D buffer, scaled", scaled),
        ] {
            let mut overlay_buffer = card(
                1 << 18 | VIDPROC_OVERLAY_ENABLE | 1 << 21 | VIDPROC_CURSOR | scaling,
                320,
                Some((0, 320, [at(8, 4), at(150, 95)])),
            );
            reg3d(&mut overlay_buffer, COL_BUFFER_ADDR, 0xA_0000);
            reg3d(&mut overlay_buffer, COL_BUFFER_STRIDE, 320);
            reg3d(&mut overlay_buffer, LEFT_OVERLAY_BUF, 0xA_0000);
            reg3d(&mut overlay_buffer, SWAPBUFFER_CMD, 0);
            cases.push((name, overlay_buffer));
        }
        let mut desktop_buffer = card(1 << 18 | VIDPROC_DESKTOP_TILED, 3, None);
        reg3d(&mut desktop_buffer, COL_BUFFER_ADDR, 0x1000);
        reg3d(&mut desktop_buffer, COL_BUFFER_STRIDE, 1 << 15 | 3);
        cases.push(("tiled 5-6-5 desktop that is a 3D buffer", desktop_buffer));

        let buffers = cases.len() - 3;
        for (n, (name, mut v)) in cases.into_iter().enumerate() {
            v.set_mirror(true);
            v.set_software_picture(false);
            let recording = v.take_mirror().unwrap();
            let s = recording.scanout.as_deref().expect("a scanout");
            let from_buffer = s.desktop.buffer.is_some()
                || s.overlay
                    .as_ref()
                    .is_some_and(|o| o.surface.buffer.is_some());
            assert_eq!(from_buffer, n >= buffers, "{name}: drawn by the renderer");
            // The picture's size: the desktop's, twice as wide when it is
            // less than 400 pixels wide.
            let (w, h) = v.picture_size();
            assert_eq!(s.wide, w == 2 * 160, "{name}: widened");
            let (w, h) = (w as usize, h as usize);
            // The picture as composed by renderers within each of
            // `limits`: the host's OpenGL; textures of 256 texels, which
            // lay the memory out in 256 layers of 64 KB; and no program
            // for 2D operations.
            let limits = [
                Limits::default(),
                Limits {
                    max_texture_size: Some(256),
                    no_blit: false,
                },
                Limits {
                    max_texture_size: None,
                    no_blit: true,
                },
            ];
            let mut pictures = Vec::new();
            for limits in limits {
                let mut renderer =
                    VoodooGl::with_limits(&gl, Glsl::Gl150, 1, 1, 1, limits).unwrap();
                assert!(renderer.banshee_problem().is_none(), "{name}: {limits:?}");
                assert_eq!(renderer.gpu_2d_problem().is_some(), limits.no_blit);
                renderer.run(&gl, recording.clone());
                // Something drawn over the picture (a message) goes on top.
                let base = Frame::new(w as u32, h as u32);
                let mut screen = base.clone();
                screen.rgba[..3].copy_from_slice(&[1, 2, 3]);
                let (_, size) = renderer.composite(&gl, &screen, &base).unwrap();
                assert_eq!(size, (w as u32, h as u32));
                let mut composed = vec![0; w * h * 4];
                // SAFETY: the context and framebuffer belong to this thread.
                unsafe {
                    let fbo = renderer.composite.as_ref().unwrap().0.fbo;
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                    gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
                    let pixels = glow::PixelPackData::Slice(Some(&mut composed));
                    let (format, kind) = (glow::RGBA, glow::UNSIGNED_BYTE);
                    gl.read_pixels(0, 0, w as i32, h as i32, format, kind, pixels);
                    assert_eq!(gl.get_error(), glow::NO_ERROR);
                }
                renderer.destroy(&gl);
                pictures.push((limits, composed));
            }
            v.set_software_picture(true);
            v.prepare_display();
            let mut reference = vec![0; w * h * 4];
            v.render(&mut reference, w);
            reference[..3].copy_from_slice(&[1, 2, 3]);
            for (limits, composed) in pictures {
                let bad: Vec<usize> = (0..w * h)
                    .filter(|&i| composed[i * 4..i * 4 + 3] != reference[i * 4..i * 4 + 3])
                    .collect();
                if let Some(&i) = bad.first() {
                    panic!(
                        "{name}, {limits:?}: {} pixels differ, the first at ({}, {}): {:?} composed, {:?} in software",
                        bad.len(),
                        i % w,
                        i / w,
                        &composed[i * 4..i * 4 + 3],
                        &reference[i * 4..i * 4 + 3],
                    );
                }
            }
        }
    }

    /// 16 MB is one layer of 4096 rows of 4096 bytes where OpenGL allows
    /// textures that big, and narrower rows in several layers where it
    /// doesn't; the layers take rows in order.
    #[test]
    fn memory_layout_fits_the_texture_limits() {
        use super::MemoryLayout;
        let mb16 = 16 << 20;
        let layout = |most, layers| MemoryLayout::new(mb16, most, layers).unwrap();
        for (most, width, rows, layers) in [
            (16384, 4096, 4096, 1),
            (4096, 4096, 4096, 1),
            (3000, 2048, 2048, 4),
            (1024, 1024, 1024, 16),
            (256, 256, 256, 256),
        ] {
            let l = layout(most, 2048);
            assert_eq!((l.width(), l.layer_rows(), l.layers), (width, rows, layers), "{most}");
            assert_eq!(l.width() * l.rows(), mb16);
        }
        assert!(MemoryLayout::new(mb16, 1024, 8).is_err());
        // A small memory takes one layer of as many rows as it needs.
        let small = MemoryLayout::new(4096, 1024, 8).unwrap();
        assert_eq!((small.width(), small.layer_rows(), small.layers), (1024, 4, 1));

        let l = layout(1024, 2048);
        let spans: Vec<_> = l.spans(1000, 3100).collect();
        assert_eq!(spans, [(0, 1000, 1024), (1, 0, 1024), (2, 0, 1024), (3, 0, 28)]);
        assert_eq!(l.spans(5, 5).count(), 0);
        assert_eq!(l.spans(16380, 20000).collect::<Vec<_>>(), [(15, 1020, 1024)]);
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

    /// A hidden window's desktop OpenGL 3.3 context, current on this
    /// thread, with the window and SDL that keep it.
    struct TestContext {
        gl: glow::Context,
        _context: sdl2::video::GLContext,
        _window: sdl2::video::Window,
        _video: sdl2::VideoSubsystem,
        _sdl: sdl2::Sdl,
    }

    fn test_context(title: &str) -> TestContext {
        let sdl = sdl2::init().unwrap();
        let video = sdl.video().unwrap();
        let attrs = video.gl_attr();
        attrs.set_context_profile(sdl2::video::GLProfile::Core);
        attrs.set_context_version(3, 3);
        let window = video.window(title, 8, 8).opengl().hidden().build().unwrap();
        let context = window.gl_create_context().unwrap();
        // SAFETY: this thread owns the current SDL OpenGL context.
        let gl =
            unsafe { glow::Context::from_loader_function(|s| video.gl_get_proc_address(s).cast()) };
        TestContext {
            gl,
            _context: context,
            _window: window,
            _video: video,
            _sdl: sdl,
        }
    }

    const BASE: usize = 0xD000_0000;

    fn w(bus: &mut crate::bus::Bus, reg: u32, value: u32) {
        bus.write_32(BASE + reg as usize, value);
    }

    /// A Voodoo Graphics set up as Glide sets it for 640x480, with the Y
    /// origin at row 479, recording for OpenGL.
    fn dither_card() -> crate::bus::Bus {
        let mut bus = crate::bus::Bus::new(std::path::PathBuf::from("."));
        bus.set_cycles_per_ms(1000);
        bus.configure_voodoo(Some(crate::voodoo::Board::Standard));
        let cfg = |bus: &mut crate::bus::Bus, reg: u32, value: u32| {
            bus.io_write_wide(0xCF8, 0x8000_0000 | reg, 4);
            bus.io_write_wide(0xCFC, value, 4);
        };
        cfg(&mut bus, 0x40, 1);
        // fbiInit1 to 3: 640 pixel rows, the second buffer after 150
        // pages, the Y origin. Then the timing, and the output on.
        w(&mut bus, 0x214, 10 << 4);
        w(&mut bus, 0x218, 150 << 11);
        w(&mut bus, 0x21C, 479 << 22);
        w(&mut bus, 0x208, 2 << 16 | 40);
        w(&mut bus, 0x220, ((800 - 96 - 2) << 16) | (96 - 1));
        w(&mut bus, 0x224, (525 - 2) << 16 | 2);
        w(&mut bus, 0x20C, 479 << 16 | 639);
        cfg(&mut bus, 0xC0, 0);
        w(&mut bus, 0x210, 1);
        cfg(&mut bus, 0x40, 0);
        // The clip rectangle: the screen.
        w(&mut bus, 0x118, 640);
        w(&mut bus, 0x11C, 480);
        bus.voodoo.as_mut().unwrap().set_mirror(true);
        bus
    }

    /// Draw with fbzMode's `fbz` bits and the colour writes: a fastfill of
    /// the screen with a colour between 5-6-5 steps, then two triangles
    /// making a rectangle of 256 by 64 pixels at (64, 32). With `ramp`,
    /// its red rises and its green falls by 1 a pixel across and its blue
    /// rises by 4 a row; without, it is one colour between steps.
    fn draw_scene(bus: &mut crate::bus::Bus, fbz: u32, ramp: bool) {
        w(bus, 0x110, (1 << 9) | fbz);
        w(bus, 0x148, 0x0084_4A6B);
        w(bus, 0x124, 0);
        w(bus, 0x104, 0);
        let (x0, y0, x1, y1) = (64, 32, 320, 96);
        for verts in [
            [(x0, y0), (x1, y0), (x0, y1)],
            [(x1, y0), (x1, y1), (x0, y1)],
        ] {
            let (ax, ay) = verts[0];
            let (dx, dy) = if ramp { (ax - x0, ay - y0) } else { (0, 0) };
            // 12.12, half way between two values, so that both renderers
            // take the same integer part.
            let start = |v: i32| ((v << 12) | 0x800) as u32;
            let (r, g, b) = if ramp {
                (dx, 255 - dx, 4 * dy)
            } else {
                (123, 61, 149)
            };
            for (reg, v) in [(0x020, r), (0x024, g), (0x028, b), (0x030, 255)] {
                w(bus, reg, start(v));
            }
            let slopes = if ramp {
                [1, -1, 0, 0, 0, 0, 4, 0]
            } else {
                [0; 8]
            };
            for (reg, v) in [0x040, 0x044, 0x048, 0x050, 0x060, 0x064, 0x068, 0x070]
                .into_iter()
                .zip(slopes)
            {
                w(bus, reg, (v << 12) as u32);
            }
            for (i, (x, y)) in verts.into_iter().enumerate() {
                w(bus, 0x008 + 8 * i as u32, (x * 16) as u32);
                w(bus, 0x00C + 8 * i as u32, (y * 16) as u32);
            }
            w(bus, 0x080, 0);
        }
    }

    /// A 5-6-5 pixel widened as the card's picture widens it.
    fn widen(v: u16) -> [u8; 3] {
        let (r, g, b) = ((v >> 11) as u8, (v >> 5 & 0x3F) as u8, (v & 0x1F) as u8);
        [r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2]
    }

    /// What the software rasterizer and OpenGL at `scale` (with dithering
    /// `on`) draw of `draw_scene`: the card's 640x480 pixels, and the
    /// buffer's RGB, rows from the top.
    fn both_pictures(
        gl: &glow::Context,
        fbz: u32,
        ramp: bool,
        on: bool,
        scale: u32,
    ) -> (Vec<u16>, Vec<[u8; 3]>) {
        use super::*;
        let mut bus = dither_card();
        let card = bus.voodoo.as_mut().unwrap();
        let resync = card.take_mirror().unwrap();
        draw_scene(&mut bus, fbz, ramp);
        let card = bus.voodoo.as_mut().unwrap();
        let recording = card.take_mirror().unwrap();
        // The software picture through frame buffer reads of the front
        // buffer, which see everything drawn.
        w(&mut bus, 0x114, 0);
        let mut software = Vec::with_capacity(640 * 480);
        for y in 0..480 {
            for x in (0..640).step_by(2) {
                let pair = bus.read_32(BASE + 0x40_0000 + y * 2048 + x * 2);
                software.extend([pair as u16, (pair >> 16) as u16]);
            }
        }
        let mut renderer = VoodooGl::new(gl, Glsl::Gl150, scale, 1, 1).unwrap();
        renderer.set_dither(on);
        renderer.run(gl, resync);
        renderer.run(gl, recording);
        let (w, h) = renderer.scaled();
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        // SAFETY: the context and framebuffer belong to this thread.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(renderer.targets[&0].fbo));
            let pixels = glow::PixelPackData::Slice(Some(&mut rgba));
            gl.read_pixels(0, 0, w, h, glow::RGBA, glow::UNSIGNED_BYTE, pixels);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            assert_eq!(gl.get_error(), glow::NO_ERROR);
        }
        renderer.destroy(gl);
        (
            software,
            rgba.as_chunks::<4>()
                .0
                .iter()
                .map(|&[r, g, b, _]| [r, g, b])
                .collect(),
        )
    }

    /// The pixels where `opengl` at `scale` isn't the card's `software`
    /// picture widened, as (x, y, card's, OpenGL's), and how many.
    fn differences(software: &[u16], opengl: &[[u8; 3]], scale: usize) -> (usize, Vec<String>) {
        let mut bad = Vec::new();
        for (i, &v) in software.iter().enumerate() {
            let (x, y) = (i % 640, i / 640);
            for (sx, sy) in (0..scale).flat_map(|sy| (0..scale).map(move |sx| (sx, sy))) {
                let got = opengl[(y * scale + sy) * 640 * scale + x * scale + sx];
                if got != widen(v) {
                    bad.push(format!("({x}, {y}) {:?} {:?}", widen(v), got));
                }
            }
        }
        (bad.len(), bad.into_iter().take(8).collect())
    }

    #[test]
    #[ignore = "requires a desktop OpenGL context"]
    fn dithering_matches_the_software_rasterizer() {
        let t = test_context("Dither");
        let gl = &t.gl;
        let (dither, dither2, y_origin) = (1 << 8, 1 << 11, 1 << 17);
        // At 1x, the ramp's every value at every place in the matrices is
        // the card's, for the 4x4 and 2x2 matrices and both Y origins.
        for (fbz, what) in [
            (dither, "4x4"),
            (dither | y_origin, "4x4, Y origin at the bottom"),
            (dither | dither2, "2x2"),
            (dither | dither2 | y_origin, "2x2, Y origin at the bottom"),
        ] {
            let (software, opengl) = both_pictures(gl, fbz, true, true, 1);
            assert_eq!(differences(&software, &opengl, 1), (0, vec![]), "{what}");
        }
        // Above 1x a card pixel's dither covers a block of the buffer's.
        for fbz in [dither, dither | dither2 | y_origin] {
            let (software, opengl) = both_pictures(gl, fbz, false, true, 2);
            assert_eq!(
                differences(&software, &opengl, 2),
                (0, vec![]),
                "2x, fbzMode {fbz:X}h"
            );
        }

        // A game that doesn't dither keeps 8 bits a colour, which the
        // card's 5-6-5 pixels are the top bits of.
        let (plain, opengl) = both_pictures(gl, 0, true, true, 1);
        let truncated =
            |&[r, g, b]: &[u8; 3]| (r as u16 >> 3) << 11 | (g as u16 >> 2) << 5 | b as u16 >> 3;
        assert!(opengl.iter().map(truncated).eq(plain.iter().copied()));
        assert!(
            differences(&plain, &opengl, 1).0 > 10_000,
            "8 bits, not 5-6-5"
        );
        // With voodoo_dither off, so does a game that dithers.
        let (dithered, undithered) = both_pictures(gl, dither, true, false, 1);
        assert!(undithered == opengl);
        assert_ne!(dithered, plain);
    }

    /// Time OpenGL drawing many dithered Gouraud triangles at 2x, with
    /// voodoo_dither on and off, alternately. `cargo test --release`.
    #[test]
    #[ignore = "a benchmark; requires a desktop OpenGL context"]
    fn benchmark_dithering() {
        use super::*;
        use crate::voodoo::mirror::{DrawState, Layout};
        use crate::voodoo::raster::Stride;
        let t = test_context("Dither benchmark");
        let gl = &t.gl;
        let state = DrawState {
            dest: 0,
            fbz_color_path: 0,
            fbz_mode: (1 << 8) | (1 << 9),
            alpha_mode: 0,
            fog_mode: 0,
            za_color: 0,
            chroma_key: 0,
            chroma_range: 0,
            color0: 0,
            color1: 0,
            fog_color: 0,
            stipple: 0,
            yorigin: 479,
            clip: None,
            aux: false,
            fogblend: [0; 64],
            fogdelta: [0; 64],
            send_config: None,
            tmu: [None; 2],
        };
        // 200 screen-sized quads a frame, 61 million card pixels and four
        // times as many buffer pixels at 2x.
        let vertices: Vec<Vertex> = (0..200)
            .flat_map(|i| {
                let c = |x: f32, y: f32| {
                    [
                        (x + i as f32) % 256.0,
                        y * 255.0 / 480.0,
                        255.0 - x * 0.3,
                        255.0,
                    ]
                };
                let v = |x: f32, y: f32| Vertex {
                    pos: [x, y],
                    color: c(x, y),
                    ..Vertex::default()
                };
                let (a, b, c, d) = (v(0.0, 0.0), v(640.0, 0.0), v(0.0, 480.0), v(640.0, 480.0));
                [a, b, c, b, d, c]
            })
            .collect();
        let resync = Recording {
            commands: vec![Command::Resync(Box::new(Snapshot {
                layout: Layout {
                    width: 640,
                    height: 480,
                    stride: Stride::linear(640),
                    aux_stride: Stride::linear(640),
                    color: vec![0],
                    aux: None,
                },
                color: vec![vec![0; 640 * 480]],
                aux: None,
            }))],
            front: Some(0),
            output: true,
            width: 640,
            height: 480,
            clut: [0; 33],
            scanout: None,
        };
        let frame = Recording {
            commands: vec![Command::Draw(Box::new(Draw { state, vertices }))],
            ..resync.clone()
        };
        let mut renderer = VoodooGl::new(gl, Glsl::Gl150, 2, 1, 1).unwrap();
        renderer.run(gl, resync);
        let mut times = [Vec::new(), Vec::new()];
        for round in 0..6 {
            for on in [true, false] {
                renderer.set_dither(on);
                // SAFETY: the context belongs to this thread.
                unsafe { gl.finish() };
                let start = std::time::Instant::now();
                for _ in 0..10 {
                    renderer.run(gl, frame.clone());
                }
                // SAFETY: as above.
                unsafe { gl.finish() };
                let ms = start.elapsed().as_secs_f64() * 100.0;
                if round > 0 {
                    times[on as usize].push(ms);
                }
            }
        }
        for (on, ms) in times.iter().enumerate().rev() {
            let mean = ms.iter().sum::<f64>() / ms.len() as f64;
            println!(
                "dither {}: {mean:.2} ms a frame {ms:.2?}",
                if on == 1 { "on" } else { "off" }
            );
        }
        renderer.destroy(gl);
    }
}
