//! The OpenGL host renderer ("The OpenGL renderer" in
//! `docs/toadforce.md`): the card's triangles drawn by the host's GPU,
//! behind `gpu::Host`.
//!
//! - **Targets.** Each surface the card draws into has a host texture:
//!   8 bits a channel for the 32-bit formats, 5-6-5 for `R5G6B5` (see
//!   `Colour`), and the host's 16-bit depth or 24-bit depth with
//!   stencil for the depth formats. A texture is filled from card
//!   memory when the card's pages say its copy is stale, and read back
//!   into the card's own bytes. Row 0 of the surface is row 0 of the
//!   texture, so the host's window coordinates are the card's and
//!   nothing is turned over.
//! - **Textures** are decoded by the software renderer's decoders
//!   (`texture.rs`) and kept as 8 bits a channel, by the texture's
//!   description and the stamps of the memory it was read from. The
//!   host's sampler does the addressing and the filtering.
//! - **Programs.** The fragment shader is printed from the draw's
//!   steps (`shader.rs`), and kept by them. The depth and stencil
//!   tests, the blend, the write masks and the clip rectangle are the
//!   host's fixed state.
//! - **Vertices** come from the card in screen space. They are rounded
//!   to 1/256 of a pixel as `raster::setup` rounds them, and moved by
//!   half a pixel where the card's pixel centres are at whole
//!   coordinates, since the host's are at half.
//!
//! What it doesn't draw as the software renderer does, it declines
//! (`Host::takes`), and the software renderer draws that.
//!
//! The host is made, used and dropped on the thread whose OpenGL
//! context it was given, while that context is current.

mod shader;
mod texture;

#[cfg(test)]
mod tests;

use super::format::{Format, Surface};
use super::gpu::{self, Batch, Clear, Host, Pages, Stamp};
use super::level1::Texture;
use super::raster::pipeline::{self, Blend, DrawState, Shape, Step, Values};
use super::raster::sample::{Address, Filter, Mip, Tex};
use super::raster::span;
use super::raster::texel::{self, Rgba};
use super::raster::vertex::ScreenVertex;
use crate::video::shader::Glsl;
use crate::voodoo::gl::{Program, compile};
use glow::HasContext;
use shader::Uniforms;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

const TEXTURE_MAX_ANISOTROPY: u32 = 0x84FE;
const MAX_TEXTURE_MAX_ANISOTROPY: u32 = 0x84FF;

/// The bytes of a vertex as the host gets it: the position, the two
/// colours, and four coordinate sets (`shader::ATTRIBUTES`).
const VERTEX_BYTES: usize = 88;

/// The texture unit textures are bound to while they are filled, past
/// the stages' units (`shader::unit`, `shader::key_unit`).
const SCRATCH_UNIT: u32 = 2 * super::level1::states::STAGES as u32;

/// How the host keeps a colour buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Colour {
    /// `A8R8G8B8`, as it is.
    Argb,
    /// `X8R8G8B8`. The host's alpha holds the pixel's unused byte, so
    /// a read-back gives it back; a blend reads the destination's
    /// alpha as 1, and a draw writes 0 there as the software renderer
    /// does.
    Xrgb,
    /// `R5G6B5` in the host's 5-6-5 format: the host rounds what a
    /// draw writes to the format's bits, as the card does.
    Packed,
    /// `R5G6B5` in 8 bits a channel, on a host whose 5-6-5 format is
    /// missing or fails `ToadForceGl::packed_fault`. The colours are
    /// reduced when they are read back, so blended layers keep 8 bits
    /// until then.
    Wide,
}

/// A surface as the host has it.
struct Copy {
    surface: Surface,
    texture: glow::Texture,
    /// What the pages held when the texture was last what card memory
    /// has.
    stamp: Stamp,
}

/// The sampling states of a stage, as last set on a host texture.
#[derive(Clone, Copy, PartialEq)]
struct Sampling {
    address: [Address; 2],
    border: Rgba,
    mag: Filter,
    min: Filter,
    mip: Mip,
    lod_bias: u32,
    first_level: u32,
    anisotropy: u32,
}

impl Sampling {
    fn of(tex: &Tex) -> Self {
        Sampling {
            address: tex.address,
            border: tex.border,
            mag: tex.mag,
            min: tex.min,
            mip: tex.mip,
            lod_bias: tex.lod_bias.to_bits(),
            first_level: tex.first_level,
            anisotropy: tex.anisotropy,
        }
    }
}

/// A texture as the host has it.
struct Texels {
    texture: Texture,
    gl: glow::Texture,
    /// The texture that marks the texels with the colour key's value,
    /// for a texture with a key.
    key: Option<glow::Texture>,
    /// The bytes of card memory it was read from, and their stamps.
    ranges: Vec<((u32, u32), Stamp)>,
    sampling: Option<Sampling>,
    key_sampling: Option<Sampling>,
    /// The batch that last sampled it.
    used: u64,
}

/// The card's compare functions (`D3DCMP`) as OpenGL's. A value that
/// names none passes everything, as `combine::compare` has it.
fn compare(func: u32) -> u32 {
    match func {
        1 => glow::NEVER,
        2 => glow::LESS,
        3 => glow::EQUAL,
        4 => glow::LEQUAL,
        5 => glow::GREATER,
        6 => glow::NOTEQUAL,
        7 => glow::GEQUAL,
        _ => glow::ALWAYS,
    }
}

/// A stencil operation (`D3DSTENCILOP`) as OpenGL's.
fn stencil_op(op: u32) -> u32 {
    match op {
        2 => glow::ZERO,
        3 => glow::REPLACE,
        4 => glow::INCR,
        5 => glow::DECR,
        6 => glow::INVERT,
        7 => glow::INCR_WRAP,
        8 => glow::DECR_WRAP,
        _ => glow::KEEP,
    }
}

/// A blend factor (`D3DBLEND`) as OpenGL's. A target without alpha
/// reads as alpha 1 (`combine::factor` on what `texel::decode` gives),
/// whatever the host's texture holds there.
fn blend_factor(factor: u32, alpha: bool) -> u32 {
    match factor {
        1 => glow::ZERO,
        3 => glow::SRC_COLOR,
        4 => glow::ONE_MINUS_SRC_COLOR,
        5 => glow::SRC_ALPHA,
        6 => glow::ONE_MINUS_SRC_ALPHA,
        7 if alpha => glow::DST_ALPHA,
        8 if alpha => glow::ONE_MINUS_DST_ALPHA,
        8 => glow::ZERO,
        9 => glow::DST_COLOR,
        10 => glow::ONE_MINUS_DST_COLOR,
        11 if alpha => glow::SRC_ALPHA_SATURATE,
        11 => glow::ZERO,
        _ => glow::ONE,
    }
}

fn blend_op(op: u32) -> u32 {
    match op {
        2 => glow::FUNC_SUBTRACT,
        3 => glow::FUNC_REVERSE_SUBTRACT,
        4 => glow::MIN,
        5 => glow::MAX,
        _ => glow::FUNC_ADD,
    }
}

fn overlap(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// The pixels of `s` in `memory`, a row after another with nothing
/// between them.
fn rows(s: &Surface, memory: &[u8]) -> Vec<u8> {
    let len = (s.width * s.format.bytes()) as usize;
    let mut out = Vec::with_capacity(len * s.height as usize);
    for y in 0..s.height {
        let at = s.at(0, y);
        out.extend_from_slice(&memory[at..at + len]);
    }
    out
}

/// `pixels`, as `rows` gives them, put where `s` is in `memory`.
fn put_rows(s: &Surface, pixels: &[u8], memory: &mut [u8]) {
    let len = (s.width * s.format.bytes()) as usize;
    for (y, row) in pixels.chunks_exact(len).enumerate() {
        let at = s.at(0, y as u32);
        memory[at..at + len].copy_from_slice(row);
    }
}

/// A position as the software renderer rounds it, to 1/256 of a pixel
/// (`setup::snap`).
fn snap(v: f32) -> f32 {
    if !v.is_finite() {
        return 0.0;
    }
    let limit = super::raster::setup::LIMIT;
    ((v.clamp(-limit, limit) as f64 * 256.0).round() / 256.0) as f32
}

/// The triangles of a run as the host's vertices, added to `out`.
/// Gives how many vertices there are.
fn vertices(shape: &Shape, stride: usize, from: &[ScreenVertex], out: &mut Vec<u8>) -> usize {
    // The host's pixel centres are at half coordinates.
    let shift = if shape.half_centre { 0.0 } else { 0.5 };
    let mut count = 0;
    for triangle in from.chunks_exact(stride) {
        // A flat-shaded triangle has the colours of its fourth vertex.
        let colours = triangle.get(3).unwrap_or(&triangle[0]);
        for v in &triangle[..3] {
            let colours = if shape.flat { colours } else { v };
            for number in [snap(v.x) + shift, snap(v.y) + shift, v.z, v.rhw] {
                out.extend_from_slice(&number.to_ne_bytes());
            }
            out.extend_from_slice(&colours.diffuse);
            out.extend_from_slice(&colours.specular);
            for number in v.tex[..pipeline::SLOTS].iter().flatten() {
                out.extend_from_slice(&number.to_ne_bytes());
            }
            count += 1;
        }
    }
    count
}

/// The OpenGL host renderer.
pub struct ToadForceGl {
    gl: Rc<glow::Context>,
    glsl: Glsl,
    /// Whether `R5G6B5` targets are in the host's 5-6-5 format.
    packed: bool,
    /// The highest anisotropy the host filters with, or 0 where it
    /// has none.
    anisotropy: f32,
    /// The framebuffer the draws go to, and the one a depth buffer is
    /// read from, which has no colour.
    target: glow::Framebuffer,
    depth_only: glow::Framebuffer,
    array: glow::VertexArray,
    buffer: glow::Buffer,
    copies: Vec<Copy>,
    textures: Vec<Texels>,
    /// The programs by `shader::key` of the steps they draw.
    programs: HashMap<Vec<Step>, Program<Uniforms>>,
    /// The keys whose program the host didn't compile.
    failed: HashSet<Vec<Step>>,
    /// Counted up with every batch.
    batch: u64,
    /// For the status: the runs drawn, and the surfaces and textures
    /// copied from card memory.
    runs: u64,
    uploads: u64,
    decodes: u64,
    /// The host has no framebuffer the card's surfaces fit: nothing is
    /// taken.
    broken: bool,
}

impl ToadForceGl {
    /// A host renderer on `gl`, a desktop OpenGL context of version
    /// 3.2 or above that is current.
    pub fn new(gl: Rc<glow::Context>, glsl: Glsl) -> Result<Self, String> {
        if glsl != Glsl::Gl150 {
            return Err("the ToadForce's OpenGL renderer needs desktop OpenGL 3.2".to_string());
        }
        // SAFETY: the caller's context is current on this thread.
        let mut host = unsafe {
            let extensions = gl.supported_extensions();
            let anisotropy = if extensions.contains("GL_EXT_texture_filter_anisotropic")
                || extensions.contains("GL_ARB_texture_filter_anisotropic")
            {
                gl.get_parameter_f32(MAX_TEXTURE_MAX_ANISOTROPY)
            } else {
                0.0
            };
            let version = gl.version();
            let packed = (version.major, version.minor) >= (4, 1)
                || extensions.contains("GL_ARB_ES2_compatibility");
            let target = gl.create_framebuffer()?;
            let depth_only = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(depth_only));
            gl.draw_buffer(glow::NONE);
            gl.read_buffer(glow::NONE);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            let array = gl.create_vertex_array()?;
            let buffer = gl.create_buffer()?;
            gl.bind_vertex_array(Some(array));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            let stride = VERTEX_BYTES as i32;
            gl.vertex_attrib_pointer_f32(0, 4, glow::FLOAT, false, stride, 0);
            // The colours' bytes as numbers from 0 to 255.
            gl.vertex_attrib_pointer_f32(1, 4, glow::UNSIGNED_BYTE, false, stride, 16);
            gl.vertex_attrib_pointer_f32(2, 4, glow::UNSIGNED_BYTE, false, stride, 20);
            for slot in 0..pipeline::SLOTS as u32 {
                let offset = 24 + 16 * slot as i32;
                gl.vertex_attrib_pointer_f32(3 + slot, 4, glow::FLOAT, false, stride, offset);
            }
            for (index, _) in shader::ATTRIBUTES {
                gl.enable_vertex_attrib_array(index);
            }
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            ToadForceGl {
                gl,
                glsl,
                packed,
                anisotropy,
                target,
                depth_only,
                array,
                buffer,
                copies: Vec::new(),
                textures: Vec::new(),
                programs: HashMap::new(),
                failed: HashSet::new(),
                batch: 0,
                runs: 0,
                uploads: 0,
                decodes: 0,
                broken: false,
            }
        };
        if host.packed
            && let Some(fault) = host.packed_fault()
        {
            eprintln!(
                "[TOADFORCE] the host's 5-6-5 targets don't hold what the card's do ({fault}): \
                 16-bit targets are drawn with 8 bits a channel"
            );
            host.packed = false;
        }
        host.forget();
        Ok(host)
    }

    /// Whether `R5G6B5` targets are in the host's 5-6-5 format.
    pub fn packed(&self) -> bool {
        self.packed
    }

    /// Keep `R5G6B5` targets in 8 bits a channel from now on, as on a
    /// host whose 5-6-5 format fails the test.
    pub fn keep_wide(&mut self) {
        self.forget();
        self.packed = false;
    }

    /// What the host's 5-6-5 format gets wrong, or None where it holds
    /// what the card's does: a target filled from card memory reads
    /// back as it was, a colour written reads back as `span::pack` has
    /// it, and a blend's result is rounded to the nearest value the
    /// format holds. The format being there doesn't say so: drivers
    /// have got packed formats wrong, some put an 8-bit format in its
    /// place, and some cut a blend's result off where the card rounds
    /// it.
    fn packed_fault(&mut self) -> Option<&'static str> {
        const SIDE: u32 = 4;
        let colour = Surface {
            offset: 0,
            pitch: SIDE * 2,
            width: SIDE,
            height: SIDE,
            format: Format::R5G6B5,
        };
        let target = super::level1::Target {
            colour,
            depth: None,
        };
        let mut memory = vec![0u8; (SIDE * SIDE * 2) as usize];
        let pages = Pages::default();
        let pixel =
            |memory: &[u8], i: usize| u16::from_le_bytes([memory[2 * i], memory[2 * i + 1]]);
        // One triangle over every pixel, in one colour.
        let corner = |x: f32, y: f32, diffuse: Rgba| ScreenVertex {
            x,
            y,
            diffuse,
            ..ScreenVertex::default()
        };
        let far = 2.0 * SIDE as f32;
        let triangle = |c: Rgba| {
            [
                corner(0.0, 0.0, c),
                corner(far, 0.0, c),
                corner(0.0, far, c),
            ]
        };
        let state = |blend: Option<Blend>| {
            Arc::new(DrawState {
                steps: vec![
                    Step::Colours { specular: false },
                    Step::Write { blend, mask: 0xF },
                ],
                values: Values::default(),
                target,
                clip: [0, 0, SIDE as i32, SIDE as i32],
                tex: Default::default(),
                sets: Vec::new(),
                coords: Vec::new(),
                shape: Shape {
                    half_centre: false,
                    cull: 1,
                    fill: 3,
                    flat: false,
                    flat_last: false,
                    last_pixel: false,
                    point_size: 1.0,
                    point_min: 1.0,
                    point_max: 1.0,
                    point_sprite: false,
                },
                specular: false,
                serial: false,
            })
        };
        // The destination's pixels: a ramp of every field.
        let ramp: Vec<u16> = (0..SIDE * SIDE)
            .map(|i| ((i * 2) << 11 | (i * 4) << 5 | (31 - i * 2)) as u16)
            .collect();
        let fill = |memory: &mut [u8]| {
            for (i, value) in ramp.iter().enumerate() {
                memory[2 * i..2 * i + 2].copy_from_slice(&value.to_le_bytes());
            }
        };
        let blend = Blend {
            src: 5,
            dst: 6,
            op: 1,
        };
        // Channel values each side of where the 5 and the 6 bits
        // round, in different company.
        let values = [0u8, 4, 5, 12, 13, 127, 128, 131, 132, 250, 251, 255];
        for (i, &v) in values.iter().enumerate() {
            let c: Rgba = [v, values[(i + 5) % values.len()], 255 - v, 128];
            for blend in [None, Some(blend)] {
                // A new copy of the target each time, from memory.
                self.forget();
                fill(&mut memory);
                let vertices = triangle(c);
                let runs = [gpu::Run {
                    state: state(blend),
                    vertices: 0..3,
                }];
                let batch = Batch {
                    target,
                    runs: &runs,
                    vertices: &vertices,
                };
                self.draw(&batch, &memory, &pages);
                if self.broken || !self.read_back(&colour, &mut memory) {
                    return Some("it is no render target");
                }
                for (i, &old) in ramp.iter().enumerate() {
                    let got = pixel(&memory, i) as u32;
                    let ok = match blend {
                        None => got == span::pack(Format::R5G6B5, c),
                        // The blend of the fields as the host has
                        // them, each of its own range. A result near
                        // the middle between two values, or near one,
                        // says nothing: the host's arithmetic may be
                        // a little off either way.
                        Some(_) => [(11, 31, texel::R), (5, 63, texel::G), (0, 31, texel::B)]
                            .into_iter()
                            .all(|(shift, max, ch)| {
                                let alpha = c[texel::A] as f64 / 255.0;
                                let src = c[ch] as f64 / 255.0;
                                let dst = (old as u32 >> shift & max) as f64 / max as f64;
                                let ideal = (src * alpha + dst * (1.0 - alpha)) * max as f64;
                                let part = ideal.fract();
                                let clear =
                                    (0.2..=0.4).contains(&part) || (0.6..=0.8).contains(&part);
                                !clear || (got >> shift & max) == ideal.round() as u32
                            }),
                    };
                    if !ok {
                        return Some(match blend {
                            None => "a colour written reads back as another",
                            Some(_) => "a blend isn't rounded to the nearest value",
                        });
                    }
                }
            }
        }
        // A target read back unchanged.
        self.forget();
        fill(&mut memory);
        let vertices = triangle([0; 4]);
        let mut hidden = (*state(None)).clone();
        hidden.clip = [0, 0, 0, 0];
        let runs = [gpu::Run {
            state: Arc::new(hidden),
            vertices: 0..3,
        }];
        let batch = Batch {
            target,
            runs: &runs,
            vertices: &vertices,
        };
        self.draw(&batch, &memory, &pages);
        let mut back = vec![0u8; memory.len()];
        let same = self.read_back(&colour, &mut back) && back == memory;
        (!same).then_some("a target doesn't read back as it was filled")
    }

    /// How the host keeps a colour buffer of `format`.
    fn colour(&self, format: Format) -> Colour {
        match format {
            Format::A8R8G8B8 => Colour::Argb,
            Format::R5G6B5 if self.packed => Colour::Packed,
            Format::R5G6B5 => Colour::Wide,
            _ => Colour::Xrgb,
        }
    }

    /// The host's format for a texture of a surface of `format`: the
    /// internal format, and the format and the type of the pixels it
    /// is filled from and read back as.
    fn formats(&self, format: Format) -> (u32, u32, u32) {
        match format {
            Format::D16 => (
                glow::DEPTH_COMPONENT16,
                glow::DEPTH_COMPONENT,
                glow::UNSIGNED_SHORT,
            ),
            Format::D24X8 | Format::D24S8 => (
                glow::DEPTH24_STENCIL8,
                glow::DEPTH_STENCIL,
                glow::UNSIGNED_INT_24_8,
            ),
            Format::R5G6B5 if self.packed => (glow::RGB565, glow::RGB, glow::UNSIGNED_SHORT_5_6_5),
            _ => (glow::RGBA8, glow::BGRA, glow::UNSIGNED_BYTE),
        }
    }

    /// The state every use of the context here starts from.
    ///
    /// # Safety
    ///
    /// The context is current.
    unsafe fn enter(&self) {
        let gl = &*self.gl;
        // SAFETY: the caller's.
        unsafe {
            gl.active_texture(glow::TEXTURE0 + SCRATCH_UNIT);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            gl.disable(glow::CULL_FACE);
            // The card's dither is the shader's.
            gl.disable(glow::DITHER);
            gl.disable(glow::TEXTURE_CUBE_MAP_SEAMLESS);
            // Depth outside 0 to 1 is clamped, as `span.rs` clamps it,
            // and cuts nothing off.
            gl.enable(glow::DEPTH_CLAMP);
        }
    }

    /// OpenGL's reset state again, for whoever uses the context next.
    ///
    /// # Safety
    ///
    /// The context is current.
    unsafe fn leave(&self) {
        let gl = &*self.gl;
        // SAFETY: the caller's.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.use_program(None);
            gl.active_texture(glow::TEXTURE0);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
            gl.enable(glow::DITHER);
            for state in [
                glow::DEPTH_CLAMP,
                glow::DEPTH_TEST,
                glow::STENCIL_TEST,
                glow::BLEND,
                glow::SCISSOR_TEST,
            ] {
                gl.disable(state);
            }
            gl.color_mask(true, true, true, true);
            gl.depth_mask(true);
            gl.stencil_mask(!0);
            gl.depth_func(glow::LESS);
            gl.blend_func(glow::ONE, glow::ZERO);
            gl.blend_equation(glow::FUNC_ADD);
        }
    }

    /// Drop the copies of other surfaces, and the textures, that share
    /// a byte with `surfaces`: what the host draws there isn't counted
    /// in the card's pages (`Host::draw`).
    ///
    /// # Safety
    ///
    /// The context is current.
    unsafe fn drop_shared(&mut self, surfaces: &[Surface]) {
        let gl = &*self.gl;
        let ranges: Vec<(u32, u32)> = surfaces.iter().map(gpu::range).collect();
        let shares = |r: (u32, u32)| ranges.iter().any(|&t| overlap(t, r));
        self.copies.retain(|c| {
            let keep = surfaces.contains(&c.surface) || !shares(gpu::range(&c.surface));
            if !keep {
                // SAFETY: the texture is this context's.
                unsafe { gl.delete_texture(c.texture) };
            }
            keep
        });
        self.textures.retain(|t| {
            let keep = !t.ranges.iter().any(|&(r, _)| shares(r));
            if !keep {
                // SAFETY: the textures are this context's.
                unsafe {
                    gl.delete_texture(t.gl);
                    if let Some(key) = t.key {
                        gl.delete_texture(key);
                    }
                }
            }
            keep
        });
    }

    /// The host's texture of `surface`, filled from `memory` if the
    /// host has none or a stale one.
    ///
    /// # Safety
    ///
    /// The context is current, in the state `enter` leaves.
    unsafe fn copy(&mut self, surface: &Surface, memory: &[u8], pages: &Pages) -> glow::Texture {
        let range = gpu::range(surface);
        let (internal, format, kind) = self.formats(surface.format);
        let wide = self.colour(surface.format) == Colour::Wide;
        let gl = &*self.gl;
        let (w, h) = (surface.width as i32, surface.height as i32);
        // SAFETY: the caller's.
        unsafe {
            let at = match self.copies.iter().position(|c| c.surface == *surface) {
                Some(at) => {
                    if !pages.changed(range, &mut self.copies[at].stamp) {
                        return self.copies[at].texture;
                    }
                    gl.bind_texture(glow::TEXTURE_2D, Some(self.copies[at].texture));
                    at
                }
                None => {
                    let texture = gl.create_texture().expect("an OpenGL texture");
                    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    let none = glow::PixelUnpackData::Slice(None);
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        internal as i32,
                        w,
                        h,
                        0,
                        format,
                        kind,
                        none,
                    );
                    for filter in [glow::TEXTURE_MIN_FILTER, glow::TEXTURE_MAG_FILTER] {
                        gl.tex_parameter_i32(glow::TEXTURE_2D, filter, glow::NEAREST as i32);
                    }
                    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, 0);
                    self.copies.push(Copy {
                        surface: *surface,
                        texture,
                        stamp: Stamp::default(),
                    });
                    self.copies.len() - 1
                }
            };
            self.uploads += 1;
            let mut pixels = rows(surface, memory);
            if wide {
                pixels = pixels
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .flat_map(|p| texel::decode(Format::R5G6B5, u16::from_le_bytes(*p) as u32))
                    .collect();
            }
            let data = glow::PixelUnpackData::Slice(Some(&pixels));
            gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, w, h, format, kind, data);
            self.copies[at].stamp = pages.stamp(range);
            self.copies[at].texture
        }
    }

    /// Make `colour` and `depth` what the draws go to. False where the
    /// host has no such framebuffer.
    ///
    /// # Safety
    ///
    /// The context is current.
    unsafe fn attach(
        &mut self,
        colour: glow::Texture,
        depth: Option<(glow::Texture, Format)>,
    ) -> bool {
        let gl = &*self.gl;
        // SAFETY: the caller's.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.target));
            let attach = |point: u32, texture: Option<glow::Texture>| {
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, point, glow::TEXTURE_2D, texture, 0);
            };
            attach(glow::COLOR_ATTACHMENT0, Some(colour));
            attach(glow::DEPTH_STENCIL_ATTACHMENT, None);
            attach(glow::DEPTH_ATTACHMENT, None);
            match depth {
                Some((texture, Format::D16)) => attach(glow::DEPTH_ATTACHMENT, Some(texture)),
                Some((texture, _)) => attach(glow::DEPTH_STENCIL_ATTACHMENT, Some(texture)),
                None => {}
            }
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                eprintln!(
                    "[TOADFORCE] the host has no framebuffer for the card's target: \
                     the software renderer draws"
                );
                self.broken = true;
            }
        }
        !self.broken
    }

    /// The program of `steps`, compiled if it is new. None for one
    /// the host doesn't compile, which `takes` declines from then on.
    ///
    /// # Safety
    ///
    /// The context is current.
    unsafe fn program(&mut self, steps: &[Step]) -> Option<&Program<Uniforms>> {
        let key = shader::key(steps);
        if self.failed.contains(&key) {
            return None;
        }
        if !self.programs.contains_key(&key) {
            let fragment = shader::fragment(&key);
            let made = compile::<Uniforms>(
                &self.gl,
                self.glsl,
                shader::VERTEX,
                &fragment,
                &shader::ATTRIBUTES,
            );
            match made {
                Ok(program) => {
                    self.programs.insert(key.clone(), program);
                }
                Err(e) => {
                    let e = e.replacen("a 3dfx shader", "a ToadForce shader", 1);
                    eprintln!("[TOADFORCE] {e}: the software renderer draws this state");
                    self.failed.insert(key);
                    return None;
                }
            }
        }
        self.programs.get(&key)
    }

    /// The host's texture of `texture`, decoded from `memory` if the
    /// host has none or a stale one: its place in `textures`.
    ///
    /// # Safety
    ///
    /// The context is current, in the state `enter` leaves.
    unsafe fn texels(&mut self, texture: &Texture, memory: &[u8], pages: &Pages) -> usize {
        let gl = &*self.gl;
        // A unit no stage samples, for the textures filled here.
        // SAFETY: the caller's.
        unsafe { gl.active_texture(glow::TEXTURE0 + SCRATCH_UNIT) };
        let at = match self.textures.iter().position(|t| t.texture == *texture) {
            Some(at) => {
                let t = &mut self.textures[at];
                t.used = self.batch;
                let mut stale = false;
                for (range, stamp) in &mut t.ranges {
                    stale |= pages.changed(*range, stamp);
                }
                if !stale {
                    return at;
                }
                at
            }
            None => {
                let mut ranges = Vec::new();
                let clamp = |at: u64| at.min(u32::MAX as u64) as u32;
                pipeline::texture_reads(texture, |r| {
                    ranges.push(((clamp(r.0), clamp(r.1)), Stamp::default()));
                });
                let keyed = texture.key.is_some() && texture::has_key(texture.format);
                // SAFETY: the caller's.
                let (gl_texture, key) = unsafe {
                    let make = || gl.create_texture().expect("an OpenGL texture");
                    (make(), keyed.then(make))
                };
                self.textures.push(Texels {
                    texture: texture.clone(),
                    gl: gl_texture,
                    key,
                    ranges,
                    sampling: None,
                    key_sampling: None,
                    used: self.batch,
                });
                self.textures.len() - 1
            }
        };
        self.decodes += 1;
        let t = &mut self.textures[at];
        let kind = if texture.cube {
            glow::TEXTURE_CUBE_MAP
        } else {
            glow::TEXTURE_2D
        };
        // SAFETY: the caller's.
        unsafe {
            let last = texture.levels as i32 - 1;
            for level in 0..texture.levels {
                for face in 0..texture.faces() {
                    let image = texture::decode(texture, level, face, memory);
                    let (w, h) = (image.width as i32, image.height as i32);
                    let to = if texture.cube {
                        glow::TEXTURE_CUBE_MAP_POSITIVE_X + face
                    } else {
                        glow::TEXTURE_2D
                    };
                    let level = level as i32;
                    gl.bind_texture(kind, Some(t.gl));
                    let data = glow::PixelUnpackData::Slice(Some(&image.texels));
                    let (format, bytes) = (glow::BGRA, glow::UNSIGNED_BYTE);
                    gl.tex_image_2d(to, level, glow::RGBA8 as i32, w, h, 0, format, bytes, data);
                    if let (Some(key), Some(keyed)) = (t.key, &image.keyed) {
                        gl.bind_texture(kind, Some(key));
                        let data = glow::PixelUnpackData::Slice(Some(keyed));
                        let (format, bytes) = (glow::RED, glow::UNSIGNED_BYTE);
                        gl.tex_image_2d(to, level, glow::R8 as i32, w, h, 0, format, bytes, data);
                    }
                }
            }
            for texture in std::iter::once(t.gl).chain(t.key) {
                gl.bind_texture(kind, Some(texture));
                gl.tex_parameter_i32(kind, glow::TEXTURE_MAX_LEVEL, last);
            }
        }
        for (range, stamp) in &mut t.ranges {
            *stamp = pages.stamp(*range);
        }
        at
    }

    /// Put the texture at `at` of `textures` on `stage`'s units, with
    /// the stage's sampling states.
    ///
    /// # Safety
    ///
    /// The context is current.
    unsafe fn bind(&mut self, at: usize, stage: usize, tex: &Tex) {
        let gl = &*self.gl;
        let anisotropy = self.anisotropy;
        let t = &mut self.textures[at];
        let kind = if tex.texture.cube {
            glow::TEXTURE_CUBE_MAP
        } else {
            glow::TEXTURE_2D
        };
        let now = Sampling::of(tex);
        let mipped = tex.mip != Mip::None && tex.texture.levels > 1;
        let wrap = |address: Address| match address {
            Address::Wrap => glow::REPEAT,
            Address::Mirror => glow::MIRRORED_REPEAT,
            Address::Clamp => glow::CLAMP_TO_EDGE,
            Address::Border => glow::CLAMP_TO_BORDER,
        };
        let last = tex.texture.levels - 1;
        let first = tex.first_level.min(last);
        // What the texture and the texture of its key share: where a
        // coordinate falls, and the levels.
        let common = |nearest: bool| {
            // SAFETY: the caller's, with the texture bound.
            unsafe {
                gl.tex_parameter_i32(kind, glow::TEXTURE_WRAP_S, wrap(tex.address[0]) as i32);
                gl.tex_parameter_i32(kind, glow::TEXTURE_WRAP_T, wrap(tex.address[1]) as i32);
                let filter = |filter: Filter| match filter {
                    Filter::Linear if !nearest => glow::LINEAR,
                    _ => glow::NEAREST,
                };
                let min = match (filter(tex.min), tex.mip) {
                    (min, _) if !mipped => min,
                    (glow::LINEAR, Mip::Linear) if !nearest => glow::LINEAR_MIPMAP_LINEAR,
                    (glow::LINEAR, _) => glow::LINEAR_MIPMAP_NEAREST,
                    (_, Mip::Linear) if !nearest => glow::NEAREST_MIPMAP_LINEAR,
                    _ => glow::NEAREST_MIPMAP_NEAREST,
                };
                gl.tex_parameter_i32(kind, glow::TEXTURE_MIN_FILTER, min as i32);
                gl.tex_parameter_i32(kind, glow::TEXTURE_MAG_FILTER, filter(tex.mag) as i32);
                // `MAXMIPLEVEL` limits the level the pixel's size
                // picks (`Sampler::levels`). Without mip levels in use
                // it is the one level sampled.
                let (base, least) = if mipped { (0, first) } else { (first, 0) };
                gl.tex_parameter_i32(kind, glow::TEXTURE_BASE_LEVEL, base as i32);
                gl.tex_parameter_f32(kind, glow::TEXTURE_MIN_LOD, least as f32);
                gl.tex_parameter_f32(kind, glow::TEXTURE_LOD_BIAS, tex.lod_bias);
            }
        };
        // SAFETY: the caller's.
        unsafe {
            gl.active_texture(glow::TEXTURE0 + shader::unit(stage));
            gl.bind_texture(kind, Some(t.gl));
            if t.sampling != Some(now) {
                common(false);
                let border = tex.border.map(|c| c as f32 / 255.0);
                let border = [
                    border[texel::R],
                    border[texel::G],
                    border[texel::B],
                    border[texel::A],
                ];
                gl.tex_parameter_f32_slice(kind, glow::TEXTURE_BORDER_COLOR, &border);
                if anisotropy > 0.0 {
                    let most = (tex.anisotropy as f32).clamp(1.0, anisotropy);
                    gl.tex_parameter_f32(kind, TEXTURE_MAX_ANISOTROPY, most);
                }
                t.sampling = Some(now);
            }
            if let Some(key) = t.key.filter(|_| tex.key.is_some()) {
                gl.active_texture(glow::TEXTURE0 + shader::key_unit(stage));
                gl.bind_texture(kind, Some(key));
                if t.key_sampling != Some(now) {
                    // The nearest texel, and no key outside a border.
                    common(true);
                    gl.tex_parameter_f32_slice(kind, glow::TEXTURE_BORDER_COLOR, &[0.0; 4]);
                    t.key_sampling = Some(now);
                }
            }
        }
    }

    /// The host's fixed state for a run with `state`: the tests, the
    /// blend, the masks and the clip rectangle. False
    /// for a run that has no pixels.
    ///
    /// # Safety
    ///
    /// The context is current.
    unsafe fn fixed(&self, state: &DrawState) -> bool {
        let gl = &*self.gl;
        let surface = &state.target.colour;
        let left = state.clip[0].max(0);
        let top = state.clip[1].max(0);
        let right = state.clip[2].min(surface.width as i32);
        let bottom = state.clip[3].min(surface.height as i32);
        if left >= right || top >= bottom {
            return false;
        }
        let colour = self.colour(surface.format);
        let depth = state.steps.iter().find_map(|step| match *step {
            Step::Depth {
                func,
                write,
                stencil,
            } => Some((func, write, stencil)),
            _ => None,
        });
        let write = state.steps.iter().find_map(|step| match *step {
            Step::Write { blend, mask } => Some((blend, mask)),
            _ => None,
        });
        let values = &state.values;
        // SAFETY: the caller's.
        unsafe {
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(left, top, right - left, bottom - top);
            match depth {
                Some((func, write, stencil)) => {
                    gl.enable(glow::DEPTH_TEST);
                    gl.depth_func(compare(func));
                    gl.depth_mask(write);
                    match stencil {
                        Some(s) => {
                            gl.enable(glow::STENCIL_TEST);
                            let reference = values.stencil_ref as i32;
                            gl.stencil_func(compare(s.func), reference, values.stencil_mask);
                            gl.stencil_op(
                                stencil_op(s.fail),
                                stencil_op(s.zfail),
                                stencil_op(s.pass),
                            );
                            gl.stencil_mask(values.stencil_write);
                        }
                        None => gl.disable(glow::STENCIL_TEST),
                    }
                }
                None => {
                    gl.disable(glow::DEPTH_TEST);
                    gl.disable(glow::STENCIL_TEST);
                }
            }
            let Some((blend, mask)) = write else {
                gl.color_mask(false, false, false, false);
                gl.disable(glow::BLEND);
                return true;
            };
            let on = |bit: u8| mask >> bit & 1 != 0;
            // The alpha of a target that has none: the unused byte,
            // which a draw makes 0, or nothing the card ever sees.
            let alpha = match colour {
                Colour::Argb => on(3),
                Colour::Xrgb => true,
                Colour::Packed | Colour::Wide => false,
            };
            gl.color_mask(on(0), on(1), on(2), alpha);
            let has_alpha = colour == Colour::Argb;
            let (src, dst, op) = match blend {
                Some(b) => (
                    blend_factor(b.src, has_alpha),
                    blend_factor(b.dst, has_alpha),
                    blend_op(b.op),
                ),
                None => (glow::ONE, glow::ZERO, glow::FUNC_ADD),
            };
            if colour == Colour::Xrgb {
                gl.enable(glow::BLEND);
                gl.blend_func_separate(src, dst, glow::ZERO, glow::ZERO);
                gl.blend_equation_separate(op, glow::FUNC_ADD);
            } else if blend.is_some() {
                gl.enable(glow::BLEND);
                gl.blend_func(src, dst);
                gl.blend_equation(op);
            } else {
                gl.disable(glow::BLEND);
            }
        }
        true
    }

    /// The numbers of a run's state, to the program in use. With
    /// `pack` the shader rounds the colour to a 5-6-5 target's bits.
    ///
    /// # Safety
    ///
    /// The context is current, with the program of `u` in use.
    unsafe fn uniforms(gl: &glow::Context, u: &Uniforms, state: &DrawState, pack: bool) {
        let v = &state.values;
        let surface = &state.target.colour;
        let rgba = |c: Rgba| [texel::R, texel::G, texel::B, texel::A].map(|ch| c[ch] as i32);
        // SAFETY: the caller's.
        unsafe {
            let scale = [2.0 / surface.width as f32, 2.0 / surface.height as f32];
            gl.uniform_2_f32(u.scale.as_ref(), scale[0], scale[1]);
            gl.uniform_1_i32(u.pack.as_ref(), pack as i32);
            // The bias in steps of the depth buffer, of its range.
            let (bias, steps) = match state.target.depth.map(|d| d.format) {
                Some(Format::D16) => (v.z_bias, 0xFFFF),
                _ => (v.z_bias >> 8, 0xFF_FFFF),
            };
            gl.uniform_1_f32(u.bias.as_ref(), bias as f32 / steps as f32);
            if u.factor.is_some() {
                let [r, g, b, a] = rgba(v.factor);
                gl.uniform_4_i32(u.factor.as_ref(), r, g, b, a);
            }
            if u.alpha_ref.is_some() {
                gl.uniform_1_i32(u.alpha_ref.as_ref(), v.alpha_ref as i32);
            }
            if u.fog_colour.is_some() {
                let [r, g, b, _] = rgba(v.fog_colour);
                gl.uniform_3_i32(u.fog_colour.as_ref(), r, g, b);
            }
            if u.fog.is_some() {
                gl.uniform_3_f32(u.fog.as_ref(), v.fog_start, v.fog_end, v.fog_density);
            }
            for (stage, m) in v.bump.iter().enumerate() {
                if u.bump[stage].is_some() {
                    gl.uniform_4_f32(u.bump[stage].as_ref(), m[0], m[1], m[2], m[3]);
                }
                if u.lum[stage].is_some() {
                    gl.uniform_2_f32(u.lum[stage].as_ref(), m[4], m[5]);
                }
            }
        }
    }

    /// Delete the textures no draw has sampled for a while, once there
    /// are many: a program's freed textures stay here until then.
    ///
    /// # Safety
    ///
    /// The context is current.
    unsafe fn prune(&mut self) {
        const MANY: usize = 4096;
        const BATCHES: u64 = 1000;
        if self.textures.len() <= MANY {
            return;
        }
        let (gl, batch) = (&*self.gl, self.batch);
        self.textures.retain(|t| {
            let keep = t.used + BATCHES >= batch;
            if !keep {
                // SAFETY: the textures are this context's.
                unsafe {
                    gl.delete_texture(t.gl);
                    if let Some(key) = t.key {
                        gl.delete_texture(key);
                    }
                }
            }
            keep
        });
    }
}

impl Host for ToadForceGl {
    fn name(&self) -> &str {
        "OpenGL"
    }

    fn takes(&self, state: &DrawState) -> bool {
        use Format::*;
        if self.broken
            || state.serial
            || !matches!(state.target.colour.format, R5G6B5 | X8R8G8B8 | A8R8G8B8)
        {
            return false;
        }
        for step in &state.steps {
            match *step {
                Step::Sample {
                    stage,
                    cube,
                    keyed,
                    bumped,
                    ..
                } => {
                    let Some(tex) = &state.tex[stage as usize] else {
                        return false;
                    };
                    // The bump offsets move a cube map's coordinates on
                    // the face they picked, which the host's cube
                    // sampler has no way to say.
                    if cube && bumped.is_some() {
                        return false;
                    }
                    // A block format's pixels have no value to compare
                    // with the key.
                    if keyed && !texture::has_key(tex.texture.format) {
                        return false;
                    }
                    // With `MAXMIPLEVEL` the software renderer still
                    // magnifies by the pixel's size in texels of level
                    // 0; the host goes by the level it limits to, and
                    // the two filters would change over elsewhere.
                    let first = tex.first_level.min(tex.texture.levels.saturating_sub(1));
                    if first > 0 && tex.mag != tex.min {
                        return false;
                    }
                }
                // `SRCALPHASAT` for the destination is OpenGL 3.3's.
                Step::Write {
                    blend: Some(Blend { dst: 11, .. }),
                    ..
                } => return false,
                _ => {}
            }
        }
        self.failed.is_empty() || !self.failed.contains(&shader::key(&state.steps))
    }

    fn status(&self) -> serde_json::Value {
        serde_json::json!({
            "batches": self.batch,
            "runs": self.runs,
            "surface_uploads": self.uploads,
            "texture_uploads": self.decodes,
            "programs": self.programs.len(),
            "failed_programs": self.failed.len(),
            "surfaces": self.copies.len(),
            "textures": self.textures.len(),
            "packed_565": self.packed,
        })
    }

    fn draw(&mut self, batch: &Batch, memory: &[u8], pages: &Pages) {
        self.batch += 1;
        self.runs += batch.runs.len() as u64;
        let colour = batch.target.colour;
        let depth = batch.target.depth_surface();
        let surfaces: Vec<Surface> = std::iter::once(colour).chain(depth).collect();
        // SAFETY: the context is current (see the module comment), and
        // every object used is this host's.
        unsafe {
            self.enter();
            self.drop_shared(&surfaces);
            let target = self.copy(&colour, memory, pages);
            let depth = depth.map(|d| (self.copy(&d, memory, pages), d.format));
            if !self.attach(target, depth) {
                self.leave();
                return;
            }
            let gl = Rc::clone(&self.gl);
            gl.viewport(0, 0, colour.width as i32, colour.height as i32);
            // Every run's vertices in one buffer.
            let mut bytes = Vec::new();
            let mut firsts = Vec::with_capacity(batch.runs.len());
            let mut first = 0;
            for run in batch.runs {
                let from = &batch.vertices[run.vertices.clone()];
                let count = vertices(&run.state.shape, run.stride(), from, &mut bytes);
                firsts.push((first as i32, count as i32));
                first += count;
            }
            gl.bind_vertex_array(Some(self.array));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &bytes, glow::STREAM_DRAW);
            for (run, (first, count)) in batch.runs.iter().zip(firsts) {
                let state = &*run.state;
                if count == 0 || !self.fixed(state) {
                    continue;
                }
                for (stage, tex) in state.tex.iter().enumerate() {
                    if let Some(tex) = tex {
                        let at = self.texels(&tex.texture, memory, pages);
                        self.bind(at, stage, tex);
                    }
                }
                let Some(program) = self.program(&state.steps) else {
                    continue;
                };
                gl.use_program(Some(program.program));
                // A write without a blend into a 5-6-5 target.
                let plain = state
                    .steps
                    .iter()
                    .any(|step| matches!(step, Step::Write { blend: None, .. }));
                let pack = plain && colour.format == Format::R5G6B5;
                Self::uniforms(&gl, &program.uniforms, state, pack);
                gl.draw_arrays(glow::TRIANGLES, first, count);
            }
            self.prune();
            self.leave();
        }
    }

    fn clear(&mut self, clear: &Clear, _: &[u8], pages: &Pages) -> bool {
        let surface = clear.surface;
        let range = gpu::range(&surface);
        // Only a copy that is what card memory holds, or newer.
        let Some(at) = self.copies.iter().position(|c| c.surface == surface) else {
            return false;
        };
        if self.broken || pages.changed(range, &mut self.copies[at].stamp) {
            return false;
        }
        let depth_format = surface.format.in_mask(super::caps::DEPTH_FORMATS);
        let d16 = surface.format == Format::D16;
        // The depth a clear writes, and whether it is one the host's
        // clear writes exactly: its conversion of the nearest and the
        // farthest depth is certain, and of the others it isn't.
        let (depth, most) = if d16 {
            (clear.value, 0xFFFF)
        } else {
            (clear.value >> 8, 0xFF_FFFF)
        };
        let writes_depth = depth_format && (d16 || clear.mask & 0xFFFF_FF00 != 0);
        let writes_stencil = depth_format && !d16 && clear.mask & 0xFF != 0;
        let exact = !writes_depth || depth == 0 || depth == most;
        if depth_format && !exact && !clear.whole() {
            return false;
        }
        let texture = self.copies[at].texture;
        let colour = self.colour(surface.format);
        let (_, format, kind) = self.formats(surface.format);
        // SAFETY: the context is current (see the module comment), and
        // every object used is this host's.
        unsafe {
            self.enter();
            self.drop_shared(&[surface]);
            let gl = Rc::clone(&self.gl);
            if depth_format && !exact {
                // Every bit of the pixels, as the pixels themselves.
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                for rect in clear.rects {
                    let bytes = &clear.value.to_le_bytes()[..surface.format.bytes() as usize];
                    let pixels = bytes.repeat((rect.width * rect.height) as usize);
                    let data = glow::PixelUnpackData::Slice(Some(&pixels));
                    let (x, y) = (rect.left as i32, rect.top as i32);
                    let (w, h) = (rect.width as i32, rect.height as i32);
                    gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, x, y, w, h, format, kind, data);
                }
                self.leave();
                return true;
            }
            let bits = if depth_format {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.depth_only));
                let point = if d16 {
                    glow::DEPTH_ATTACHMENT
                } else {
                    glow::DEPTH_STENCIL_ATTACHMENT
                };
                let other = if d16 {
                    glow::DEPTH_STENCIL_ATTACHMENT
                } else {
                    glow::DEPTH_ATTACHMENT
                };
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, other, glow::TEXTURE_2D, None, 0);
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    point,
                    glow::TEXTURE_2D,
                    Some(texture),
                    0,
                );
                gl.depth_mask(true);
                gl.stencil_mask(0xFF);
                gl.clear_depth_f64(if depth == 0 { 0.0 } else { 1.0 });
                gl.clear_stencil((clear.value & 0xFF) as i32);
                let depth = if writes_depth {
                    glow::DEPTH_BUFFER_BIT
                } else {
                    0
                };
                let stencil = if writes_stencil {
                    glow::STENCIL_BUFFER_BIT
                } else {
                    0
                };
                depth | stencil
            } else {
                if !self.attach(texture, None) {
                    self.leave();
                    return false;
                }
                // The pixel value's fields, each of its own range.
                let v = clear.value;
                let rgba = match colour {
                    Colour::Packed => [
                        (v >> 11 & 31) as f32 / 31.0,
                        (v >> 5 & 63) as f32 / 63.0,
                        (v & 31) as f32 / 31.0,
                        1.0,
                    ],
                    Colour::Wide => {
                        let c = texel::decode(Format::R5G6B5, v);
                        [texel::R, texel::G, texel::B, texel::A].map(|ch| c[ch] as f32 / 255.0)
                    }
                    Colour::Argb | Colour::Xrgb => {
                        [v >> 16, v >> 8, v, v >> 24].map(|c| (c & 0xFF) as f32 / 255.0)
                    }
                };
                gl.color_mask(true, true, true, colour != Colour::Wide);
                gl.clear_color(rgba[0], rgba[1], rgba[2], rgba[3]);
                glow::COLOR_BUFFER_BIT
            };
            gl.enable(glow::SCISSOR_TEST);
            for rect in clear.rects {
                let (x, y) = (rect.left as i32, rect.top as i32);
                gl.scissor(x, y, rect.width as i32, rect.height as i32);
                gl.clear(bits);
            }
            self.leave();
        }
        true
    }

    fn read_back(&mut self, surface: &Surface, memory: &mut [u8]) -> bool {
        let Some(copy) = self.copies.iter().find(|c| c.surface == *surface) else {
            return false;
        };
        let texture = copy.texture;
        let (_, format, kind) = self.formats(surface.format);
        let depth_format = surface.format.in_mask(super::caps::DEPTH_FORMATS);
        let wide = !depth_format && self.colour(surface.format) == Colour::Wide;
        let (w, h) = (surface.width as i32, surface.height as i32);
        let bytes = if wide { 4 } else { surface.format.bytes() };
        let mut pixels = vec![0u8; (surface.width * surface.height * bytes) as usize];
        // SAFETY: the context is current (see the module comment), and
        // every object used is this host's.
        unsafe {
            self.enter();
            if depth_format {
                let gl = &*self.gl;
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.depth_only));
                let d16 = surface.format == Format::D16;
                let (point, other) = if d16 {
                    (glow::DEPTH_ATTACHMENT, glow::DEPTH_STENCIL_ATTACHMENT)
                } else {
                    (glow::DEPTH_STENCIL_ATTACHMENT, glow::DEPTH_ATTACHMENT)
                };
                gl.framebuffer_texture_2d(glow::FRAMEBUFFER, other, glow::TEXTURE_2D, None, 0);
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    point,
                    glow::TEXTURE_2D,
                    Some(texture),
                    0,
                );
            } else if !self.attach(texture, None) {
                self.leave();
                return false;
            } else {
                self.gl.read_buffer(glow::COLOR_ATTACHMENT0);
            }
            let data = glow::PixelPackData::Slice(Some(&mut pixels));
            self.gl.read_pixels(0, 0, w, h, format, kind, data);
            self.leave();
        }
        if wide {
            pixels = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|c| (span::pack(Format::R5G6B5, *c) as u16).to_le_bytes())
                .collect();
        }
        put_rows(surface, &pixels, memory);
        true
    }

    fn forget(&mut self) {
        let gl = &*self.gl;
        // SAFETY: the context is current (see the module comment), and
        // the textures are this host's.
        unsafe {
            for copy in self.copies.drain(..) {
                gl.delete_texture(copy.texture);
            }
            for t in self.textures.drain(..) {
                gl.delete_texture(t.gl);
                if let Some(key) = t.key {
                    gl.delete_texture(key);
                }
            }
        }
    }
}

impl Drop for ToadForceGl {
    fn drop(&mut self) {
        self.forget();
        let gl = &*self.gl;
        // SAFETY: the context is current (see the module comment), and
        // the objects are this host's.
        unsafe {
            for (_, program) in self.programs.drain() {
                gl.delete_program(program.program);
            }
            gl.delete_framebuffer(self.target);
            gl.delete_framebuffer(self.depth_only);
            gl.delete_vertex_array(self.array);
            gl.delete_buffer(self.buffer);
        }
    }
}
