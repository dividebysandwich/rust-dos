//! The software devices: Direct3D's Ramp and RGB emulation, which draw
//! on the processor into any surface, in system or video memory, on
//! any display adapter. Windows 98's DirectX lists them beside the HAL
//! and before it, and a program that asks for one by its GUID gets one
//! even on a machine without a 3D card.
//!
//! Both draw the same way here: the Ramp device's color model
//! (D3DCOLOR_MONO) is what it reports, but it draws in RGB, as Wine's
//! does. A batch of primitives is collected (`Raster`) and drawn at its
//! end: the part of the render target and of its depth buffer the
//! primitives cover is read, drawn into and written back, so a batch
//! costs two copies of the pixels it touches.
//!
//! What it draws: triangles with the top-left fill rule and pixel
//! centres on integer coordinates, lines and points; the depth test,
//! its bias and the write in 16- or 32-bit depth buffers; one texture
//! stage with every operation up to D3DTOP_BLENDCURRENTALPHA, point or
//! bilinear filtering, a level of detail worked out at each pixel of a
//! triangle with its bias, point or linear mip filtering, the address
//! modes, source color keys and palettes with alpha; specular, the
//! alpha test, vertex and table fog and alpha blending with every
//! factor. Not dithering, and not stencil, which its description
//! doesn't offer.

use super::super::super::surface::{
    self, DDPF_ALPHAPIXELS, DDPF_PALETTEINDEXED8, DDPF_RGB, DDPF_ZBUFFER, KEY_SRC_BLT, Surface,
};
use super::super::super::{Format, state};
use super::super::device::{
    Device, RS_DESTBLEND, RS_FOGDENSITY, RS_FOGENABLE, RS_FOGEND, RS_FOGSTART, RS_FOGTABLEMODE,
    RS_SPECULARENABLE, RS_SRCBLEND, RS_TEXTUREFACTOR, RS_TEXTUREMAPBLEND, RS_TEXTUREPERSPECTIVE,
    RS_ZENABLE, RS_ZFUNC, RS_ZWRITEENABLE, TA_TEXTURE, TOP_DISABLE, TOP_SELECTARG1, TSS_ADDRESSU,
    TSS_ADDRESSV, TSS_ALPHAARG1, TSS_ALPHAARG2, TSS_ALPHAOP, TSS_BORDERCOLOR, TSS_COLORARG1,
    TSS_COLORARG2, TSS_COLOROP, TSS_MAGFILTER, TSS_MINFILTER, TSS_MIPFILTER, TSS_MIPMAPLODBIAS,
};
use super::super::pipeline::{self, Rasterizer, ScreenVertex, Setup};
use super::{
    Description, Fill, RS_ALPHABLENDENABLE, RS_ALPHAREF, RS_ALPHATESTENABLE, RS_COLORKEYENABLE,
    RS_FOGCOLOR,
};
use crate::cpu::Cpu;
use std::sync::Arc;

/// The textures a software device draws with at once.
pub const UNITS: u32 = 1;

/// The band the pipeline clips what a software device draws to. The
/// rasterizer clips to the viewport itself, so this only keeps the
/// arithmetic of a triangle's edges in range.
pub const VERTEX_BAND: [f32; 4] = [-16384.0, -16384.0, 16384.0, 16384.0];

/// The largest width and height of a texture. Nothing here limits it;
/// this is what the description reports.
pub const TEXTURE_MAX: u32 = 2048;

/// D3DCOLOR_MONO and D3DCOLOR_RGB: the color models of the Ramp and the
/// RGB device, which a software device is made as.
pub const MODEL_MONO: u32 = 1;
pub const MODEL_RGB: u32 = 2;

const DDBD_16: u32 = 0x400;
const DDBD_24: u32 = 0x200;
const DDBD_32: u32 = 0x100;

/// D3DTOP_DISABLE to D3DTOP_BLENDCURRENTALPHA: the stage operations it
/// draws.
const LAST_OP: u32 = 16;

/// D3DTOP_SELECTARG1 and SELECTARG2, which read one argument, and
/// BLENDTEXTUREALPHA and BLENDTEXTUREALPHAPM, which read the texture's
/// alpha whatever their arguments are.
const TOP_SELECTARG2: u32 = 3;
const TOP_BLENDTEXTUREALPHA: u32 = 13;
const TOP_BLENDTEXTUREALPHAPM: u32 = 15;

/// D3DRENDERSTATE_ZBIAS.
const RS_ZBIAS: u32 = 47;

/// D3DTFP_LINEAR: the mip filter that blends two levels.
const MIP_LINEAR: u32 = 3;

/// DDPCAPS_ALPHA: a palette whose entries' fourth bytes are alpha.
const DDPCAPS_ALPHA: u32 = 0x400;

/// Whether the half of a stage that runs `op` on `args` reads the
/// texture.
fn reads_texture(op: u32, args: [u32; 2]) -> bool {
    let arg = |a: u32| a & 0xF == TA_TEXTURE;
    match op {
        TOP_DISABLE => false,
        TOP_SELECTARG1 => arg(args[0]),
        TOP_SELECTARG2 => arg(args[1]),
        TOP_BLENDTEXTUREALPHA | TOP_BLENDTEXTUREALPHAPM => true,
        _ => arg(args[0]) || arg(args[1]),
    }
}

pub fn can_do(op: u32) -> bool {
    (1..=LAST_OP).contains(&op)
}

/// What a software device's description says.
pub fn description() -> Description {
    let rgb = |bits: u32, a: u32, r: u32, g: u32, b: u32| Format {
        flags: DDPF_RGB | if a != 0 { DDPF_ALPHAPIXELS } else { 0 },
        fourcc: 0,
        bits,
        r,
        g,
        b,
        a,
    };
    Description {
        target_depths: DDBD_16 | DDBD_24 | DDBD_32,
        z_depths: DDBD_16 | DDBD_32,
        texture_max: TEXTURE_MAX,
        texture_aspect: TEXTURE_MAX,
        raster: 0,
        texture: 0,
        filter: 0,
        address: 0,
        // D3DPBLENDCAPS_: the destination's alpha and color and their
        // inverses, and the two that set both factors.
        blend: 0x40 | 0x80 | 0x100 | 0x200 | 0x800 | 0x1000,
        // No guard band: a program clips to the viewport.
        guard_band: [0.0; 4],
        stencil: 0,
        ops: (1 << LAST_OP) - 1,
        stages: 0,
        anisotropy: 1,
        // The formats Wine's IDirect3DDevice2 lists, in its order.
        textures: vec![
            rgb(16, 0, 0x7C00, 0x03E0, 0x001F),
            rgb(16, 0, 0xF800, 0x07E0, 0x001F),
            rgb(16, 0x8000, 0x7C00, 0x03E0, 0x001F),
            rgb(16, 0xF000, 0x0F00, 0x00F0, 0x000F),
            rgb(32, 0xFF00_0000, 0xFF_0000, 0xFF00, 0xFF),
            rgb(32, 0, 0xFF_0000, 0xFF00, 0xFF),
            rgb(8, 0, 0xE0, 0x1C, 0x03),
            Format {
                flags: DDPF_RGB | DDPF_PALETTEINDEXED8,
                bits: 8,
                ..Format::default()
            },
        ],
        z_formats: vec![
            Format {
                flags: DDPF_ZBUFFER,
                bits: 16,
                g: 0xFFFF,
                ..Format::default()
            },
            Format {
                flags: DDPF_ZBUFFER,
                bits: 32,
                g: 0xFFFF_FFFF,
                ..Format::default()
            },
        ],
    }
}

/// Whether a surface of `f` can be a software device's render target:
/// 16, 24 or 32-bit RGB, wherever it is.
pub fn takes_target(f: &Format) -> bool {
    f.flags & DDPF_RGB != 0
        && f.flags & DDPF_PALETTEINDEXED8 == 0
        && matches!(f.bits, 16 | 24 | 32)
        && f.r != 0
}

/// Whether one of `f` can be its depth buffer: 16 or 32 bits.
pub fn takes_depth(f: &Format) -> bool {
    matches!(f.bits, 16 | 32)
}

/// A decoded texture: its levels from the largest, ARGB, and for a
/// keyed one which texels are the key's.
#[derive(Debug, Default)]
pub struct Texture {
    levels: Vec<Level>,
}

#[derive(Debug, Default)]
struct Level {
    width: u32,
    height: u32,
    texels: Vec<u32>,
    keyed: Vec<bool>,
}

/// A decoded texture kept for the next batch: the surface, what it was
/// decoded from (`version`), and when it was last drawn with.
#[derive(Clone, Debug)]
struct Kept {
    surface: u32,
    version: u64,
    used: u64,
    texture: Arc<Texture>,
}

/// What the software devices keep between batches: the textures they
/// decoded. A saved state leaves it out; a texture is decoded again.
#[derive(Clone, Debug, Default)]
pub struct Hal {
    kept: Vec<Kept>,
    clock: u64,
}

/// The decoded textures kept at most.
const KEPT: usize = 64;

/// How a batch is drawn, as `bind` worked it out from the device.
#[derive(Clone, Copy, Debug, Default)]
pub struct Card {
    target: u32,
    /// The depth buffer when the depth test is on, 0 otherwise.
    depth: u32,
    /// The viewport's rectangle: left, top, right, bottom (exclusive).
    clip: [i32; 4],
    z_func: u32,
    z_write: bool,
    /// D3DRENDERSTATE_ZBIAS, 0 to 16: steps of a 16-bit depth buffer
    /// taken off a pixel's depth before the test.
    z_bias: u32,
    /// The alpha test's function and reference, if it is on.
    alpha_test: Option<(u32, u32)>,
    /// The blend's source and destination factors, if it is on.
    blend: Option<(u32, u32)>,
    /// The texture's surface, 0 for none, and whether its key is used.
    texture: u32,
    keyed: bool,
    /// Stage 0's color and alpha: the operation and its two arguments.
    color: [u32; 3],
    alpha: [u32; 3],
    /// The alpha is the diffuse color's (Direct3D 5's MODULATE with a
    /// texture without alpha).
    diffuse_alpha: bool,
    factor: u32,
    specular: bool,
    fog: Fog,
    fog_color: u32,
    mag_linear: bool,
    min_linear: bool,
    mip: bool,
    /// The mip filter blends the two levels nearest the pixel's.
    mip_linear: bool,
    /// D3DTSS_MIPMAPLODBIAS, added to each pixel's level of detail.
    lod_bias: f32,
    address: [u32; 2],
    border: u32,
    perspective: bool,
}

/// The fog: none, the vertices' (the specular color's alpha), or the
/// table's by depth: its mode, start, end and density.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Fog {
    #[default]
    Off,
    Vertex,
    Table(u32, f32, f32, f32),
}

/// The device `d`'s state for a batch.
pub fn bind(cpu: &mut Cpu, d: &Device, setup: &Setup) -> Option<Card> {
    let (width, height) = state(cpu)
        .surface(d.target)
        .map(|s| (s.width as i32, s.height as i32))?;
    let m = &setup.view_map;
    let left = (m.ox + m.bounds[0] * m.sx).max(0.0) as i32;
    let right = ((m.ox + m.bounds[1] * m.sx).max(0.0) as i32).min(width);
    let top = (m.oy - m.bounds[3] * m.sy).max(0.0) as i32;
    let bottom = ((m.oy - m.bounds[2] * m.sy).max(0.0) as i32).min(height);
    let depth = match d.state(RS_ZENABLE) {
        0 => 0,
        _ => super::super::device::depth_buffer(cpu, d).unwrap_or(0),
    };
    let texture = {
        let color = reads_texture(
            d.stage(0, TSS_COLOROP),
            [d.stage(0, TSS_COLORARG1), d.stage(0, TSS_COLORARG2)],
        );
        let alpha = d.stage(0, TSS_COLOROP) != TOP_DISABLE
            && reads_texture(
                d.stage(0, TSS_ALPHAOP),
                [d.stage(0, TSS_ALPHAARG1), d.stage(0, TSS_ALPHAARG2)],
            );
        let iface = d.textures.first().copied().unwrap_or(0);
        match (color || alpha, iface) {
            (true, iface) if iface != 0 => super::super::super::this(cpu, iface)
                .map(|(_, id)| id)
                .filter(|&id| state(cpu).surface(id).is_some())
                .unwrap_or(0),
            _ => 0,
        }
    };
    let surface = state(cpu).surface(texture).cloned();
    let keyed = d.state(RS_COLORKEYENABLE) != 0
        && surface
            .as_ref()
            .is_some_and(|s| s.key_set & 1 << KEY_SRC_BLT != 0);
    // D3DTBLEND_MODULATE and MODULATEMASK take the diffuse alpha for a
    // texture without alpha, as Direct3D before 7 has them.
    let diffuse_alpha = d.version < 7
        && matches!(d.state(RS_TEXTUREMAPBLEND), 2 | 6)
        && d.stage(0, TSS_ALPHAOP) == TOP_SELECTARG1
        && d.stage(0, TSS_ALPHAARG1) & 0xF == TA_TEXTURE
        && surface.as_ref().is_some_and(|s| s.format.a == 0);
    let fog = match (d.state(RS_FOGENABLE), d.state(RS_FOGTABLEMODE)) {
        (0, _) => Fog::Off,
        (_, pipeline::FOG_NONE) => Fog::Vertex,
        (_, mode) => Fog::Table(
            mode,
            d.state_f(RS_FOGSTART),
            d.state_f(RS_FOGEND),
            d.state_f(RS_FOGDENSITY),
        ),
    };
    Some(Card {
        target: d.target,
        depth,
        clip: [left, top, right, bottom],
        z_func: d.state(RS_ZFUNC),
        z_write: d.state(RS_ZWRITEENABLE) != 0,
        // As the ToadForce's HAL takes it, with OpenGL's offset.
        z_bias: (d.state(RS_ZBIAS) as i64 + d.depth_bias as i64).clamp(0, 16) as u32,
        alpha_test: (d.state(RS_ALPHATESTENABLE) != 0).then(|| {
            (
                d.state(super::super::device::RS_ALPHAFUNC),
                d.state(RS_ALPHAREF) & 0xFF,
            )
        }),
        blend: (d.state(RS_ALPHABLENDENABLE) != 0)
            .then(|| (d.state(RS_SRCBLEND), d.state(RS_DESTBLEND))),
        texture,
        keyed,
        color: [TSS_COLOROP, TSS_COLORARG1, TSS_COLORARG2].map(|t| d.stage(0, t)),
        alpha: [TSS_ALPHAOP, TSS_ALPHAARG1, TSS_ALPHAARG2].map(|t| d.stage(0, t)),
        diffuse_alpha,
        factor: d.state(RS_TEXTUREFACTOR),
        specular: d.state(RS_SPECULARENABLE) != 0,
        fog,
        fog_color: d.state(RS_FOGCOLOR),
        // D3DTFG_LINEAR and D3DTFN_LINEAR, and a mip filter.
        mag_linear: d.stage(0, TSS_MAGFILTER) == 2,
        min_linear: d.stage(0, TSS_MINFILTER) == 2,
        mip: d.stage(0, TSS_MIPFILTER) > 1,
        mip_linear: d.stage(0, TSS_MIPFILTER) == MIP_LINEAR,
        lod_bias: f32::from_bits(d.stage(0, TSS_MIPMAPLODBIAS)),
        address: [d.stage(0, TSS_ADDRESSU), d.stage(0, TSS_ADDRESSV)],
        border: d.stage(0, TSS_BORDERCOLOR),
        perspective: d.version >= 7 || d.state(RS_TEXTUREPERSPECTIVE) != 0,
    })
}

/// A collected primitive.
enum Prim {
    Triangle([ScreenVertex; 3]),
    Line([ScreenVertex; 2]),
    Point(ScreenVertex),
}

/// The software device as the pipeline's rasterizer: it collects a
/// batch's primitives and draws them at its end.
pub struct Raster<'a> {
    cpu: &'a mut Cpu,
    card: Card,
    prims: Vec<Prim>,
}

impl Raster<'_> {
    pub fn new(cpu: &mut Cpu, card: Card) -> Raster<'_> {
        Raster {
            cpu,
            card,
            prims: Vec::new(),
        }
    }

    /// The end of the batch: its primitives are drawn.
    pub fn finish(self) {
        draw(self.cpu, &self.card, &self.prims);
    }
}

impl Rasterizer for Raster<'_> {
    fn triangle(&mut self, v: &[ScreenVertex; 3]) {
        self.prims.push(Prim::Triangle(*v));
    }

    fn line(&mut self, v: &[ScreenVertex; 2]) {
        self.prims.push(Prim::Line(*v));
    }

    fn point(&mut self, v: &ScreenVertex) {
        self.prims.push(Prim::Point(*v));
    }
}

/// A rectangle of a surface's pixels read for drawing: where it is, and
/// its bytes, row after row.
struct Pixels {
    surface: Surface,
    x0: i32,
    y0: i32,
    width: i32,
    bpp: usize,
    bytes: Vec<u8>,
}

impl Pixels {
    fn read(cpu: &mut Cpu, surface: Surface, rect: [i32; 4]) -> Pixels {
        let bpp = surface.bytes_per_pixel().max(1) as usize;
        let [x0, y0, x1, y1] = rect;
        let (width, height) = (x1 - x0, y1 - y0);
        let row = width as usize * bpp;
        let mut bytes = vec![0; row * height as usize];
        let at = y0 as u32 * surface.pitch + x0 as u32 * bpp as u32;
        surface::read_rows(cpu, &surface, at, row, &mut bytes);
        Pixels {
            surface,
            x0,
            y0,
            width,
            bpp,
            bytes,
        }
    }

    fn write(&self, cpu: &mut Cpu) {
        let row = self.width as usize * self.bpp;
        let at = self.y0 as u32 * self.surface.pitch + self.x0 as u32 * self.bpp as u32;
        surface::write_rows(cpu, &self.surface, at, row, &self.bytes);
    }

    fn offset(&self, x: i32, y: i32) -> usize {
        ((y - self.y0) * self.width + (x - self.x0)) as usize * self.bpp
    }

    fn get(&self, x: i32, y: i32) -> u32 {
        let at = self.offset(x, y);
        let mut v = [0u8; 4];
        v[..self.bpp].copy_from_slice(&self.bytes[at..at + self.bpp]);
        u32::from_le_bytes(v)
    }

    fn set(&mut self, x: i32, y: i32, raw: u32) {
        let at = self.offset(x, y);
        let bpp = self.bpp;
        self.bytes[at..at + bpp].copy_from_slice(&raw.to_le_bytes()[..bpp]);
    }
}

/// A color as four channels from 0 to 1: red, green, blue, alpha.
type Color = [f32; 4];

fn color_of(argb: u32) -> Color {
    let [b, g, r, a] = argb.to_le_bytes();
    [r, g, b, a].map(|c| c as f32 / 255.0)
}

fn argb_of(c: Color) -> u32 {
    let [r, g, b, a] = c.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32);
    a << 24 | r << 16 | g << 8 | b
}

/// A raw pixel of `f` as ARGB: a channel without a mask is 0, alpha 255.
fn decode(raw: u32, f: &Format) -> u32 {
    let channel = |mask: u32, default: u32| {
        if mask == 0 {
            return default;
        }
        let max = mask >> mask.trailing_zeros();
        let v = (raw & mask) >> mask.trailing_zeros();
        (v as u64 * 255 / max as u64) as u32
    };
    let a = if f.flags & DDPF_ALPHAPIXELS != 0 {
        channel(f.a, 255)
    } else {
        255
    };
    a << 24 | channel(f.r, 0) << 16 | channel(f.g, 0) << 8 | channel(f.b, 0)
}

/// ARGB as a raw pixel of `f`.
fn encode(argb: u32, f: &Format) -> u32 {
    let channel = |mask: u32, v: u32| {
        if mask == 0 {
            return 0;
        }
        let max = mask >> mask.trailing_zeros();
        (((v as u64 * max as u64 + 127) / 255) as u32) << mask.trailing_zeros()
    };
    let [b, g, r, a] = argb.to_le_bytes().map(u32::from);
    channel(f.r, r) | channel(f.g, g) | channel(f.b, b) | channel(f.a, a)
}

/// A depth buffer's mask: the format's, or all its bits.
fn depth_mask(f: &Format) -> u32 {
    match (f.g, f.bits) {
        (0, 32) => u32::MAX,
        (0, bits) => (1 << bits) - 1,
        (mask, _) => mask,
    }
}

/// A depth from 0 to 1 as a raw value of a depth buffer of `mask`.
fn depth_raw(z: f32, mask: u32) -> u32 {
    let max = mask >> mask.trailing_zeros();
    ((z.clamp(0.0, 1.0) as f64 * max as f64 + 0.5) as u32) << mask.trailing_zeros()
}

/// `bias` steps of a 16-bit depth buffer as a raw value of a depth
/// buffer of `mask`: a step of a wider one is the same share of its
/// range.
fn depth_bias(bias: u32, mask: u32) -> u32 {
    let max = mask >> mask.trailing_zeros();
    let bits = 32 - max.leading_zeros();
    (bias << bits.saturating_sub(16)) << mask.trailing_zeros()
}

fn compare(func: u32, value: u32, reference: u32) -> bool {
    match func {
        1 => false,
        2 => value < reference,
        3 => value == reference,
        4 => value <= reference,
        5 => value > reference,
        6 => value != reference,
        7 => value >= reference,
        _ => true,
    }
}

/// The surface `id`'s palette as ARGB, or black. The entries' alpha is
/// their fourth byte in a palette made with DDPCAPS_ALPHA, which the
/// description's D3DPTEXTURECAPS_ALPHAPALETTE offers, and 255 in any
/// other.
fn palette(cpu: &mut Cpu, id: u32) -> [u32; 256] {
    let pal = state(cpu).surface(id).map_or(0, |s| s.palette);
    let Some((_, pid)) = (pal != 0)
        .then(|| super::super::super::this(cpu, pal))
        .flatten()
    else {
        return [0xFF00_0000; 256];
    };
    match state(cpu).object(pid).and_then(|o| o.palette.as_ref()) {
        Some(p) => argb_entries(p.caps, &p.entries),
        None => [0xFF00_0000; 256],
    }
}

/// A palette's entries (red, green, blue and flags each) as ARGB: the
/// flags byte is the alpha if `caps` has DDPCAPS_ALPHA, else 255.
fn argb_entries(caps: u32, entries: &[u8]) -> [u32; 256] {
    let mut out = [0xFF00_0000; 256];
    let alpha = caps & DDPCAPS_ALPHA != 0;
    for (o, [r, g, b, a]) in out.iter_mut().zip(entries.as_chunks::<4>().0) {
        let a = if alpha { *a as u32 } else { 0xFF };
        *o = a << 24 | (*r as u32) << 16 | (*g as u32) << 8 | *b as u32;
    }
    out
}

/// The texture of the surface `id`, decoded or as kept.
fn texture(cpu: &mut Cpu, id: u32, keyed: bool) -> Option<Arc<Texture>> {
    let mut levels = vec![id];
    while levels.len() < 16 {
        let Some(next) = super::super::texture::next_level(cpu, *levels.last()?) else {
            break;
        };
        levels.push(next);
    }
    // What the texture is decoded from: each level's pixels, the
    // palette and whether the key is used.
    let mut version = 0xCBF2_9CE4_8422_2325u64;
    let mut mix = |v: u64| version = (version ^ v).wrapping_mul(0x100_0000_01B3);
    for &level in &levels {
        let s = state(cpu).surface(level)?;
        mix(level as u64);
        mix(s.uniqueness as u64);
        mix(s.keys[KEY_SRC_BLT].0 as u64 | (s.keys[KEY_SRC_BLT].1 as u64) << 32);
    }
    let pal = palette(cpu, id);
    for p in pal {
        mix(p as u64);
    }
    mix(keyed as u64);
    let hal = &mut state(cpu).hal_software;
    hal.clock += 1;
    let clock = hal.clock;
    if let Some(k) = hal
        .kept
        .iter_mut()
        .find(|k| k.surface == id && k.version == version)
    {
        k.used = clock;
        return Some(k.texture.clone());
    }
    for &level in &levels {
        super::super::shadow::expose(cpu, level);
    }
    let mut decoded = Texture::default();
    for &level in &levels {
        let s = state(cpu).surface(level)?.clone();
        let raw = super::voodoo::level_texels(cpu, &s);
        let (low, high) = s.keys[KEY_SRC_BLT];
        let paletted = s.format.flags & DDPF_PALETTEINDEXED8 != 0;
        let texels = raw
            .iter()
            .map(|&r| match paletted {
                true => pal[(r & 0xFF) as usize],
                false => decode(r, &s.format),
            })
            .collect();
        let keyed = match keyed {
            true => raw
                .iter()
                .map(|r| (low..=high.max(low)).contains(r))
                .collect(),
            false => Vec::new(),
        };
        decoded.levels.push(Level {
            width: s.width.max(1),
            height: s.height.max(1),
            texels,
            keyed,
        });
    }
    let texture = Arc::new(decoded);
    let hal = &mut state(cpu).hal_software;
    hal.kept.retain(|k| k.surface != id);
    if hal.kept.len() >= KEPT
        && let Some(oldest) = (0..hal.kept.len()).min_by_key(|&i| hal.kept[i].used)
    {
        hal.kept.swap_remove(oldest);
    }
    hal.kept.push(Kept {
        surface: id,
        version,
        used: clock,
        texture: texture.clone(),
    });
    Some(texture)
}

/// The last device went: the decoded textures go with it.
pub fn release(cpu: &mut Cpu, gone: u32) {
    let devices = state(cpu).ids(|o| o.d3d.as_ref().is_some_and(|d| d.device.is_some()));
    if devices.iter().all(|&id| id == gone) {
        state(cpu).hal_software = Hal::default();
    }
}

/// What a batch draws into and with.
struct Target<'a> {
    card: &'a Card,
    color: Pixels,
    depth: Option<(Pixels, u32)>,
    texture: Option<Arc<Texture>>,
    /// The rectangle drawn into: left, top, right, bottom (exclusive).
    clip: [i32; 4],
}

/// The values a pixel is drawn with, from its primitive's vertices:
/// its depth, its diffuse and specular colors, and the texture's
/// coordinates and what they are divided by, each times rhw where the
/// texture is drawn in perspective.
#[derive(Clone, Copy, Default)]
struct Frag([f32; VALUES]);

const Z: usize = 0;
const DIFFUSE: usize = 1;
const SPECULAR: usize = 5;
const UVQ: usize = 9;
const VALUES: usize = 12;

impl Frag {
    fn of(v: &ScreenVertex, perspective: bool) -> Frag {
        let w = if perspective { v.rhw } else { 1.0 };
        let t = v.tex[0];
        let mut f = [0.0; VALUES];
        f[Z] = v.z;
        f[DIFFUSE..DIFFUSE + 4].copy_from_slice(&color_of(v.diffuse));
        f[SPECULAR..SPECULAR + 4].copy_from_slice(&color_of(v.specular));
        f[UVQ..UVQ + 3].copy_from_slice(&[t[0] * w, t[1] * w, t[3] * w]);
        Frag(f)
    }

    fn z(&self) -> f32 {
        self.0[Z]
    }

    fn diffuse(&self) -> Color {
        std::array::from_fn(|i| self.0[DIFFUSE + i])
    }

    fn specular(&self) -> Color {
        std::array::from_fn(|i| self.0[SPECULAR + i])
    }

    /// The texture's coordinates at the pixel.
    fn uv(&self) -> (f32, f32) {
        let [u, v, q] = [0, 1, 2].map(|i| self.0[UVQ + i]);
        if q == 0.0 {
            return (0.0, 0.0);
        }
        (u / q, v / q)
    }
}

/// Draw the primitives of a batch.
fn draw(cpu: &mut Cpu, card: &Card, prims: &[Prim]) {
    let Some(target) = state(cpu).surface(card.target).cloned() else {
        return;
    };
    let clip = [
        card.clip[0].max(0),
        card.clip[1].max(0),
        card.clip[2].min(target.width as i32),
        card.clip[3].min(target.height as i32),
    ];
    // The pixels the primitives may touch.
    let mut bounds = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
    let mut add = |v: &ScreenVertex| {
        if v.x.is_finite() && v.y.is_finite() {
            bounds[0] = bounds[0].min(v.x.floor() as i32 - 1);
            bounds[1] = bounds[1].min(v.y.floor() as i32 - 1);
            bounds[2] = bounds[2].max(v.x.ceil() as i32 + 2);
            bounds[3] = bounds[3].max(v.y.ceil() as i32 + 2);
        }
    };
    for p in prims {
        match p {
            Prim::Triangle(v) => v.iter().for_each(&mut add),
            Prim::Line(v) => v.iter().for_each(&mut add),
            Prim::Point(v) => add(v),
        }
    }
    let rect = [
        bounds[0].max(clip[0]),
        bounds[1].max(clip[1]),
        bounds[2].min(clip[2]),
        bounds[3].min(clip[3]),
    ];
    if rect[0] >= rect[2] || rect[1] >= rect[3] {
        return;
    }
    super::super::shadow::expose(cpu, card.target);
    let color = Pixels::read(cpu, target, rect);
    let depth = match state(cpu).surface(card.depth).cloned() {
        Some(z) if card.depth != 0 => {
            super::super::shadow::expose(cpu, card.depth);
            let mask = depth_mask(&z.format);
            Some((Pixels::read(cpu, z, rect), mask))
        }
        _ => None,
    };
    let texture = match card.texture {
        0 => None,
        id => texture(cpu, id, card.keyed),
    };
    let mut t = Target {
        card,
        color,
        depth,
        texture,
        clip: rect,
    };
    for p in prims {
        match p {
            Prim::Triangle(v) => triangle(&mut t, v),
            Prim::Line(v) => line(&mut t, v),
            Prim::Point(v) => {
                let (x, y) = (v.x.round() as i32, v.y.round() as i32);
                shade(&mut t, x, y, &Frag::of(v, card.perspective), 0.0);
            }
        }
    }
    t.color.write(cpu);
    if let Some((z, _)) = &t.depth
        && card.z_write
    {
        z.write(cpu);
        surface::changed(cpu, card.depth);
    }
}

/// The texture's level of detail at a pixel of a triangle whose values
/// are `planes`, `f` there: the base level's texels a step of one pixel
/// across or down covers, the larger, as a power of 2, with the bias
/// added. Above 0 the texels are smaller than the pixel (minified).
fn lod_at(t: &Target, planes: &[[f32; 3]; VALUES], f: &Frag) -> f32 {
    let Some(texture) = &t.texture else {
        return 0.0;
    };
    let base = &texture.levels[0];
    let q = f.0[UVQ + 2];
    if q == 0.0 {
        return 0.0;
    }
    // The derivatives of u / q and v / q, in texels, along x and y.
    let d = |value: usize, axis: usize, size: u32| {
        let (n, dn, dq) = (f.0[value], planes[value][axis], planes[UVQ + 2][axis]);
        (dn * q - n * dq) / (q * q) * size as f32
    };
    let step = |axis: usize| {
        let du = d(UVQ, axis, base.width);
        let dv = d(UVQ + 1, axis, base.height);
        du * du + dv * dv
    };
    let rho = step(0).max(step(1));
    if !rho.is_finite() || rho <= 0.0 {
        return t.card.lod_bias;
    }
    0.5 * rho.log2() + t.card.lod_bias
}

/// Draw a triangle: the pixels whose centres are inside it, and those on
/// its top and left edges. The colors and the depth are linear on the
/// screen, and so are the texture's coordinates times rhw where they are
/// drawn in perspective (`Frag::uv`).
fn triangle(t: &mut Target, v: &[ScreenVertex; 3]) {
    let mut v = *v;
    let area = (v[1].x - v[0].x) * (v[2].y - v[0].y) - (v[2].x - v[0].x) * (v[1].y - v[0].y);
    if !area.is_finite() || area == 0.0 {
        return;
    }
    if area < 0.0 {
        v.swap(1, 2);
    }
    let area = area.abs();
    let perspective = t.card.perspective;
    let frags = v.map(|v| Frag::of(&v, perspective));
    // The edges opposite each vertex, a to b, as functions of the pixel
    // that are positive inside: x and y factors and a constant. A pixel
    // centre on an edge is in on a top or left edge.
    let edges = [(1, 2), (2, 0), (0, 1)].map(|(a, b)| {
        let (a, b) = (&v[a], &v[b]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let top_left = dy < 0.0 || (dy == 0.0 && dx > 0.0);
        ([-dy, dx, dy * a.x - dx * a.y], top_left)
    });
    // Each value as a plane over the screen: the vertices' values weighed
    // by the edge functions over the area.
    let planes: [[f32; 3]; VALUES] = std::array::from_fn(|k| {
        std::array::from_fn(|c| (0..3).map(|i| edges[i].0[c] * frags[i].0[k]).sum::<f32>() / area)
    });
    let min_x = v.iter().map(|v| v.x).fold(f32::MAX, f32::min).ceil() as i32;
    let max_x = v.iter().map(|v| v.x).fold(f32::MIN, f32::max).floor() as i32;
    let min_y = v.iter().map(|v| v.y).fold(f32::MAX, f32::min).ceil() as i32;
    let max_y = v.iter().map(|v| v.y).fold(f32::MIN, f32::max).floor() as i32;
    let [cl, ct, cr, cb] = t.clip;
    for y in min_y.max(ct)..=max_y.min(cb - 1) {
        let fy = y as f32;
        for x in min_x.max(cl)..=max_x.min(cr - 1) {
            let fx = x as f32;
            let inside = edges.iter().all(|(e, top_left)| {
                let d = e[0] * fx + e[1] * fy + e[2];
                d > 0.0 || (d == 0.0 && *top_left)
            });
            if !inside {
                continue;
            }
            let f = Frag(std::array::from_fn(|k| {
                planes[k][0] * fx + planes[k][1] * fy + planes[k][2]
            }));
            let lod = lod_at(t, &planes, &f);
            shade(t, x, y, &f, lod);
        }
    }
}

/// Draw a line: a pixel for each step along its longer axis, the last
/// left out.
fn line(t: &mut Target, v: &[ScreenVertex; 2]) {
    let perspective = t.card.perspective;
    let (a, b) = (Frag::of(&v[0], perspective), Frag::of(&v[1], perspective));
    let (dx, dy) = (v[1].x - v[0].x, v[1].y - v[0].y);
    let steps = dx.abs().max(dy.abs()).round() as i32;
    if !(0..=1 << 16).contains(&steps) {
        return;
    }
    for i in 0..steps.max(1) {
        let s = if steps == 0 {
            0.0
        } else {
            i as f32 / steps as f32
        };
        let (x, y) = (
            (v[0].x + dx * s).round() as i32,
            (v[0].y + dy * s).round() as i32,
        );
        let f = Frag(std::array::from_fn(|k| a.0[k] * (1.0 - s) + b.0[k] * s));
        shade(t, x, y, &f, 0.0);
    }
}

/// A texel of `level` at the integer coordinates `x`, `y`, as the
/// address modes take them there: None outside a border mode's texture.
fn texel(level: &Level, card: &Card, x: i32, y: i32) -> Option<(u32, bool)> {
    let address = |c: i32, size: i32, mode: u32| match mode {
        // D3DTADDRESS_MIRROR.
        2 => {
            let m = c.rem_euclid(2 * size);
            Some(if m >= size { 2 * size - 1 - m } else { m })
        }
        // CLAMP.
        3 => Some(c.clamp(0, size - 1)),
        // BORDER.
        4 => (0..size).contains(&c).then_some(c),
        // WRAP.
        _ => Some(c.rem_euclid(size)),
    };
    let (w, h) = (level.width as i32, level.height as i32);
    let x = address(x, w, card.address[0])?;
    let y = address(y, h, card.address[1])?;
    let i = (y * w + x) as usize;
    Some((
        level.texels[i],
        level.keyed.get(i).copied().unwrap_or(false),
    ))
}

/// The texture's color at `u`, `v` in its level `level`, filtered as
/// for texels smaller than the pixels (`minified`) or not: None where
/// the key leaves it out.
fn sample(
    texture: &Texture,
    (level, minified): (usize, bool),
    card: &Card,
    u: f32,
    v: f32,
) -> Option<Color> {
    let level = &texture.levels[level.min(texture.levels.len() - 1)];
    let (fu, fv) = (u * level.width as f32, v * level.height as f32);
    if !fu.is_finite() || !fv.is_finite() {
        return Some([0.0; 4]);
    }
    let border = color_of(card.border);
    let linear = if minified {
        card.min_linear
    } else {
        card.mag_linear
    };
    let nearest = texel(level, card, fu.floor() as i32, fv.floor() as i32);
    if nearest.is_some_and(|(_, keyed)| keyed) {
        return None;
    }
    if !linear {
        return Some(nearest.map_or(border, |(c, _)| color_of(c)));
    }
    let (x, y) = (fu - 0.5, fv - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (ax, ay) = (x - x0, y - y0);
    let (x0, y0) = (x0 as i32, y0 as i32);
    let mut sum = [0.0f32; 4];
    let mut weight = 0.0;
    for (dx, dy, w) in [
        (0, 0, (1.0 - ax) * (1.0 - ay)),
        (1, 0, ax * (1.0 - ay)),
        (0, 1, (1.0 - ax) * ay),
        (1, 1, ax * ay),
    ] {
        let c = match texel(level, card, x0 + dx, y0 + dy) {
            Some((_, true)) => continue,
            Some((c, false)) => color_of(c),
            None => border,
        };
        for (s, c) in sum.iter_mut().zip(c) {
            *s += c * w;
        }
        weight += w;
    }
    if weight <= 0.0 {
        return None;
    }
    Some(sum.map(|s| s / weight))
}

/// The texture's color at `u`, `v` for the level of detail `lod`
/// (`lod_at`): the base level without a mip filter or where the texels
/// aren't smaller than the pixel, else the nearest level, or with a
/// linear mip filter the two levels around `lod` weighed by its
/// fraction. None where the key leaves it out.
fn sample_lod(texture: &Texture, lod: f32, card: &Card, u: f32, v: f32) -> Option<Color> {
    let minified = lod > 0.0;
    if !card.mip || !minified {
        return sample(texture, (0, minified), card, u, v);
    }
    let last = (texture.levels.len() - 1) as f32;
    if !card.mip_linear {
        let level = lod.round().min(last) as usize;
        return sample(texture, (level, true), card, u, v);
    }
    let low = lod.floor().min(last);
    let high = (low + 1.0).min(last);
    let a = sample(texture, (low as usize, true), card, u, v)?;
    let w = (lod - low).clamp(0.0, 1.0);
    if high == low || w == 0.0 {
        return Some(a);
    }
    let Some(b) = sample(texture, (high as usize, true), card, u, v) else {
        return Some(a);
    };
    Some(std::array::from_fn(|i| a[i] * (1.0 - w) + b[i] * w))
}

/// D3DTA_ arguments.
const TA_DIFFUSE: u32 = 0;
const TA_CURRENT: u32 = 1;
const TA_TFACTOR: u32 = 3;
const TA_SPECULAR: u32 = 4;
const TA_COMPLEMENT: u32 = 0x10;
const TA_ALPHAREPLICATE: u32 = 0x20;

/// Stage 0's result: its color's operation for the channels `range`.
#[allow(clippy::too_many_arguments)]
fn operate(
    op: u32,
    args: [u32; 2],
    texture: Color,
    diffuse: Color,
    specular: Color,
    factor: Color,
    channels: std::ops::Range<usize>,
    out: &mut Color,
) {
    let arg = |a: u32| -> Color {
        let mut c = match a & 0xF {
            TA_DIFFUSE | TA_CURRENT => diffuse,
            TA_TEXTURE => texture,
            TA_TFACTOR => factor,
            TA_SPECULAR => specular,
            _ => diffuse,
        };
        if a & TA_COMPLEMENT != 0 {
            c = c.map(|v| 1.0 - v);
        }
        if a & TA_ALPHAREPLICATE != 0 {
            c = [c[3]; 4];
        }
        c
    };
    let (a1, a2) = (arg(args[0]), arg(args[1]));
    for i in channels {
        let (x, y) = (a1[i], a2[i]);
        out[i] = match op {
            1 => diffuse[i],
            2 => x,
            3 => y,
            4 => x * y,
            5 => 2.0 * x * y,
            6 => 4.0 * x * y,
            7 => x + y,
            8 => x + y - 0.5,
            9 => 2.0 * (x + y - 0.5),
            10 => x - y,
            11 => x + y - x * y,
            12 => x * diffuse[3] + y * (1.0 - diffuse[3]),
            13 => x * texture[3] + y * (1.0 - texture[3]),
            14 => x * factor[3] + y * (1.0 - factor[3]),
            15 => x + y * (1.0 - texture[3]),
            16 => x * diffuse[3] + y * (1.0 - diffuse[3]),
            _ => diffuse[i],
        }
        .clamp(0.0, 1.0);
    }
}

/// A D3DBLEND_ factor for the source `s` over the destination `d`.
fn blend_factor(f: u32, s: Color, d: Color) -> Color {
    match f {
        1 => [0.0; 4],
        3 => s,
        4 => s.map(|v| 1.0 - v),
        5 => [s[3]; 4],
        6 => [1.0 - s[3]; 4],
        7 => [d[3]; 4],
        8 => [1.0 - d[3]; 4],
        9 => d,
        10 => d.map(|v| 1.0 - v),
        11 => {
            let f = s[3].min(1.0 - d[3]);
            [f, f, f, 1.0]
        }
        _ => [1.0; 4],
    }
}

/// Draw the pixel `x`, `y` with `f`, with the texture at the level of
/// detail `lod` (`lod_at`).
fn shade(t: &mut Target, x: i32, y: i32, f: &Frag, lod: f32) {
    let [cl, ct, cr, cb] = t.clip;
    if x < cl || x >= cr || y < ct || y >= cb {
        return;
    }
    let card = t.card;
    // The depth test, with the bias taken off the pixel's depth.
    let mut depth = None;
    if let Some((z, mask)) = &t.depth {
        let value = depth_raw(f.z(), *mask);
        let value = value.saturating_sub(depth_bias(card.z_bias, *mask)) & *mask;
        let stored = z.get(x, y) & *mask;
        if !compare(card.z_func, value, stored) {
            return;
        }
        depth = Some(value | z.get(x, y) & !*mask);
    }
    // The texture stage. A half of it that reads a texture the stage
    // hasn't got passes the diffuse color on, as the ToadForce's does.
    let diffuse = f.diffuse();
    let mut out = diffuse;
    let tex = match &t.texture {
        Some(texture) => {
            let (u, v) = f.uv();
            let Some(tex) = sample_lod(texture, lod, card, u, v) else {
                return;
            };
            Some(tex)
        }
        None => None,
    };
    if card.color[0] != TOP_DISABLE {
        let factor = color_of(card.factor);
        let spec = f.specular();
        for (half, channels) in [(card.color, 0..3), (card.alpha, 3..4)] {
            let args = [half[1], half[2]];
            if tex.is_none() && reads_texture(half[0], args) {
                continue;
            }
            let tex = tex.unwrap_or([0.0; 4]);
            operate(
                half[0], args, tex, diffuse, spec, factor, channels, &mut out,
            );
        }
        if card.diffuse_alpha {
            out[3] = diffuse[3];
        }
    }
    if card.specular {
        for (o, s) in out.iter_mut().zip(f.specular()).take(3) {
            *o = (*o + s).min(1.0);
        }
    }
    if let Some((func, reference)) = card.alpha_test
        && !compare(
            func,
            (out[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u32,
            reference,
        )
    {
        return;
    }
    let fog = match card.fog {
        Fog::Off => None,
        Fog::Vertex => Some(f.specular()[3]),
        Fog::Table(mode, start, end, density) => {
            Some(pipeline::fog_factor(mode, start, end, density, f.z()))
        }
    };
    if let Some(k) = fog {
        let c = color_of(card.fog_color);
        for (o, c) in out.iter_mut().zip(c).take(3) {
            *o = k * *o + (1.0 - k) * c;
        }
    }
    let format = t.color.surface.format;
    let raw = t.color.get(x, y);
    if let Some((src, dst)) = card.blend {
        let d = color_of(decode(raw, &format));
        let (sf, df) = match src {
            // D3DBLEND_BOTHSRCALPHA and BOTHINVSRCALPHA.
            12 => (blend_factor(5, out, d), blend_factor(6, out, d)),
            13 => (blend_factor(6, out, d), blend_factor(5, out, d)),
            _ => (blend_factor(src, out, d), blend_factor(dst, out, d)),
        };
        out = std::array::from_fn(|i| out[i] * sf[i] + d[i] * df[i]);
    }
    let bits = (1u64 << format.bits.min(32)) - 1;
    let mask = (format.r | format.g | format.b | format.a) as u64 & bits;
    let written = encode(argb_of(out), &format);
    t.color.set(
        x,
        y,
        (written as u64 & mask | raw as u64 & !mask & bits) as u32,
    );
    if let (Some(value), Some((z, _))) = (depth, &mut t.depth)
        && card.z_write
    {
        z.set(x, y, value);
    }
}

/// Fill rectangles of the device `d`'s render target and depth buffer,
/// all of them with no rectangles.
pub fn clear(cpu: &mut Cpu, d: &Device, rects: &[[i32; 4]], fill: Fill) {
    let fill_surface = |cpu: &mut Cpu, id: u32, value: u32| {
        let Some(s) = state(cpu).surface(id).cloned() else {
            return;
        };
        super::super::shadow::expose(cpu, id);
        let whole = [[0, 0, s.width as i32, s.height as i32]];
        let rects = if rects.is_empty() { &whole[..] } else { rects };
        let bpp = s.bytes_per_pixel().max(1) as usize;
        for rect in rects {
            let [l, t, r, b] = [
                rect[0].max(0),
                rect[1].max(0),
                rect[2].min(s.width as i32),
                rect[3].min(s.height as i32),
            ];
            if l >= r || t >= b {
                continue;
            }
            let row: Vec<u8> = (l..r)
                .flat_map(|_| value.to_le_bytes()[..bpp].to_vec())
                .collect();
            let bytes = row.repeat((b - t) as usize);
            let at = t as u32 * s.pitch + l as u32 * bpp as u32;
            surface::write_rows(cpu, &s, at, row.len(), &bytes);
        }
        surface::changed(cpu, id);
    };
    if let Some(color) = fill.target
        && let Some(f) = state(cpu).surface(d.target).map(|s| s.format)
    {
        fill_surface(cpu, d.target, encode(color, &f));
    }
    if let Some(depth) = fill.depth
        && let Some(z) = super::super::device::depth_buffer(cpu, d)
        && let Some(f) = state(cpu).surface(z).map(|s| s.format)
    {
        fill_surface(cpu, z, depth_raw(depth, depth_mask(&f)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_go_through_their_format_and_back() {
        let rgb565 = Format {
            flags: DDPF_RGB,
            bits: 16,
            r: 0xF800,
            g: 0x07E0,
            b: 0x001F,
            ..Format::default()
        };
        assert_eq!(encode(0xFFFF_FFFF, &rgb565), 0xFFFF);
        assert_eq!(encode(0xFF00_FF00, &rgb565), 0x07E0);
        assert_eq!(decode(0xF800, &rgb565), 0xFFFF_0000);
        for raw in [0u32, 0x1234, 0xFFFF, 0x8410] {
            assert_eq!(encode(decode(raw, &rgb565), &rgb565), raw);
        }
    }

    #[test]
    fn a_depth_takes_the_whole_of_its_mask() {
        assert_eq!(depth_raw(1.0, 0xFFFF), 0xFFFF);
        assert_eq!(depth_raw(0.0, 0xFFFF), 0);
        assert_eq!(depth_raw(0.5, 0xFFFF), 0x8000);
        assert_eq!(depth_raw(2.0, 0xFFFF_FF00), 0xFFFF_FF00);
    }

    #[test]
    fn a_palette_with_alpha_gives_its_entries_alpha() {
        // DDPCAPS_8BIT, with DDPCAPS_ALPHA and without.
        let entries = [0x11, 0x22, 0x33, 0x40, 0xAA, 0xBB, 0xCC, 0x00];
        let with = argb_entries(0x4 | DDPCAPS_ALPHA, &entries);
        assert_eq!(with[..2], [0x4011_2233, 0x00AA_BBCC]);
        let without = argb_entries(0x4, &entries);
        assert_eq!(without[..2], [0xFF11_2233, 0xFFAA_BBCC]);
        // Entries the palette hasn't got are opaque black.
        assert_eq!(with[2], 0xFF00_0000);
    }

    #[test]
    fn a_depth_bias_is_in_steps_of_a_16_bit_buffer() {
        assert_eq!(depth_bias(16, 0xFFFF), 16);
        // 24 bits above 8 of stencil, and 32 bits.
        assert_eq!(depth_bias(1, 0xFFFF_FF00), 0x1_0000);
        assert_eq!(depth_bias(1, u32::MAX), 0x1_0000);
    }

    #[test]
    fn a_linear_mip_filter_blends_the_two_nearest_levels() {
        let level = |size: u32, argb: u32| Level {
            width: size,
            height: size,
            texels: vec![argb; (size * size) as usize],
            keyed: Vec::new(),
        };
        let texture = Texture {
            levels: vec![level(4, 0xFFFF_0000), level(2, 0xFF00_00FF)],
        };
        let red = Some([1.0, 0.0, 0.0, 1.0]);
        let linear = Card {
            mip: true,
            mip_linear: true,
            ..Card::default()
        };
        let c = sample_lod(&texture, 0.25, &linear, 0.5, 0.5).unwrap();
        assert_eq!(c, [0.75, 0.0, 0.25, 1.0]);
        // Texels as big as the pixels or bigger: the base level.
        assert_eq!(sample_lod(&texture, -1.0, &linear, 0.5, 0.5), red);
        // The nearest level with a point mip filter, the base without
        // one.
        let point = Card {
            mip_linear: false,
            ..linear
        };
        assert_eq!(sample_lod(&texture, 0.25, &point, 0.5, 0.5), red);
        let c = sample_lod(&texture, 0.75, &point, 0.5, 0.5);
        assert_eq!(c, Some([0.0, 0.0, 1.0, 1.0]));
        let none = Card {
            mip: false,
            ..point
        };
        assert_eq!(sample_lod(&texture, 0.75, &none, 0.5, 0.5), red);
    }
}
