//! The HAL on a ToadForce (`crate::toadforce`): Direct3D's drawing as
//! the card's level 1 commands. The driver here builds the words a
//! guest's driver would put in the command ring and hands them to the
//! card (`Bus::toadforce_submit`).
//!
//! Before each batch of primitives `bind` works out the state the card
//! needs for the device and sends what differs from what it sent last:
//! the render target and its depth buffer, which are the surfaces'
//! blocks of card memory, the viewport, the render states and the stage
//! states, which the card numbers as Direct3D does, and the textures.
//! Nothing here decides how a state draws: that is the card's
//! (`docs/toadforce-spec.md`).
//!
//! A draw of vertices that aren't in screen space goes to the card as
//! the program gave it (`draw_geometry`): the card has the device's
//! matrices, lights, material and clip planes, and transforms, lights,
//! clips and culls the vertices itself. The device's map from clip
//! space to the screen, which may be Direct3D 5's viewport with its
//! own scale, is folded into the projection matrix the card gets, so
//! the card's viewport is only the rectangle it draws into.
//!
//! The card also makes each stage's texture coordinates as the
//! stage's `TEXCOORDINDEX` and `TEXTURETRANSFORMFLAGS` say, generated
//! and through the texture matrix, which the pipeline doesn't do
//! (#950).
//!
//! The pipeline keeps the draws the card can't take (`can_transform`):
//! blended and tweened vertices, clip status and extents the program
//! reads back, a depth range outside 0 to 1, lights and values the
//! card's commands lack, and screen vertices, which need no transform.
//! Execute buffers and ProcessVertices are the pipeline's too. For
//! those the pipeline has transformed, lit, clipped and culled the
//! vertices, so the card gets screen vertices with the two sets of
//! texture coordinates the pipeline works out, one for each of the
//! first two stages. `Raster` collects them into `DRAW` commands and
//! hands the card all of a batch's at once, which the card draws on
//! its threads.
//!
//! A texture surface in video memory whose format and size the card
//! takes is sampled where it is: `DEFINE_TEXTURE` names the surface's
//! block and those of its mipmap levels, and the card reads them when
//! it draws, so nothing is copied and nothing goes stale. Other
//! textures (in system memory, palettized, of a size that is no power
//! of two, or in a format the card lacks) are copied into blocks of
//! video memory the HAL takes from DirectDraw, as they are when the
//! card takes their format and as 8-8-8-8 otherwise. A copy is made
//! again when its surface changed, and the least recently used ones
//! make room when video memory is full.

use super::super::super::surface::{
    self, DDPF_ALPHAPIXELS, DDPF_FOURCC, DDPF_PALETTEINDEXED8, DDPF_RGB, DDPF_ZBUFFER, KEY_SRC_BLT,
    MEM_VIDEO,
};
use super::super::super::{Format as DdFormat, current_pid, display, state};
use super::super::device::{
    Device, RS_TEXTUREMAPBLEND, TA_TEXTURE, TOP_DISABLE, TOP_SELECTARG1, TSS_ALPHAARG1,
    TSS_ALPHAOP, TSS_COLOROP,
};
use super::super::pipeline::{
    self, LIGHT_GLSPOT, Layout, Matrix, Rasterizer, SHADE_FLAT, ScreenVertex, Setup, Vertex,
};
use super::{Description, Fill};
use crate::cpu::Cpu;
use crate::toadforce::format::{Format, Rect, Surface as CardSurface};
use crate::toadforce::level1::states::{self, Def};
use crate::toadforce::level1::{
    self, CLIP_PLANES, Depth, LIGHTS, Light, Material, TRANSFORMS, Target, Texture, Viewport,
    encode,
};
use crate::toadforce::{caps, regs};
use crate::win32::screen;

/// The textures a draw has at once: the sets of texture coordinates
/// the pipeline gives a screen vertex. The card takes more
/// (`caps::TEXTURES_AT_ONCE`), which the device gets when every draw
/// that uses them is one the card transforms itself (#948).
pub const UNITS: u32 = 2;

/// The band the pipeline clips what the card draws to. The card takes
/// positions to 32768 pixels either side of 0.
pub const VERTEX_BAND: [f32; 4] = [-16384.0, -16384.0, 16384.0, 16384.0];

/// The vertex layout of a `DRAW`: position with 1 / w, diffuse and
/// specular colour, and two sets of two texture coordinates.
const FVF: u32 = 0x004 | 0x040 | 0x080 | 0x200;
const VERTEX_WORDS: usize = 10;
/// The most vertices one `DRAW` holds, a command being 65535 words at
/// the most: a multiple of two and of three.
const DRAW_VERTICES: usize = 6000;

const DDPF_ALPHA: u32 = 0x2;
const DDPF_STENCILBUFFER: u32 = 0x4000;
const DDPF_LUMINANCE: u32 = 0x2_0000;
const DDPF_BUMPLUMINANCE: u32 = 0x4_0000;
const DDPF_BUMPDUDV: u32 = 0x8_0000;

/// A copy of a texture in card memory: the surface it is of and its
/// uniqueness value then, what kind of copy it is (`variant`), its
/// block of video memory, when it was last drawn with, and the copy as
/// the card's commands name it.
#[derive(Clone, Debug, Default)]
pub struct Cached {
    pub surface: u32,
    pub uniqueness: u32,
    pub variant: u32,
    pub start: u32,
    pub used: u32,
    pub texture: Texture,
}

crate::state_fields!(Cached {
    surface,
    uniqueness,
    variant,
    start,
    used,
    texture
});

/// What the HAL keeps between draws.
#[derive(Clone, Debug, Default)]
pub struct Hal {
    /// The process whose devices draw: the copies are in its video
    /// memory.
    pub pid: u32,
    pub textures: Vec<Cached>,
    /// A count of binds, for the least recently used copy.
    pub clock: u32,
    /// What the card has of the HAL's state, as far as the HAL knows.
    /// None after a state was loaded, and before the first draw: the
    /// card is then set to level 1's defaults and told everything.
    pub sent: Option<Sent>,
    /// The `Undrawn` reasons logged so far, one bit each.
    pub logged: u32,
}

crate::state_fields!(Hal {
    pid,
    textures,
    clock
} skip { sent, logged });

/// The state the HAL gives the card.
#[derive(Clone, Debug, PartialEq)]
pub struct Sent {
    /// The render states of `render_states`, in its order.
    render: Vec<u32>,
    /// The stage states of `states::STAGE`, in its order, a stage after
    /// another.
    stages: Vec<u32>,
    target: Option<Target>,
    viewport: Viewport,
    /// The texture each unit's handle is defined as, and whether the
    /// unit's stage has it.
    textures: [Option<Texture>; UNITS as usize],
    bound: [bool; UNITS as usize],
    geometry: Geometry,
}

/// What the HAL gives the card for the vertices the card transforms
/// and lights.
#[derive(Clone, Debug, PartialEq)]
struct Geometry {
    transforms: [Matrix; TRANSFORMS],
    material: Material,
    lights: [Light; LIGHTS],
    /// Bit n: light n is on.
    on: u32,
    planes: [[f32; 4]; CLIP_PLANES],
}

impl Sent {
    /// The card after a write to `LEVEL`.
    fn reset() -> Sent {
        Sent {
            render: render_states()
                .map(|id| states::render(id).map_or(0, |def| def.reset))
                .collect(),
            stages: (0..states::STAGES)
                .flat_map(|stage| {
                    states::STAGE
                        .iter()
                        .map(move |def| states::stage_reset(def, stage))
                })
                .collect(),
            target: None,
            viewport: Viewport::default(),
            textures: Default::default(),
            bound: [false; UNITS as usize],
            geometry: Geometry {
                transforms: [pipeline::IDENTITY; TRANSFORMS],
                material: Material::default(),
                lights: [Light::default(); LIGHTS],
                on: 0,
                planes: [[0.0; 4]; CLIP_PLANES],
            },
        }
    }
}

/// The render states of the card's transform and lighting, which a
/// draw of screen vertices leaves at their reset values.
const GEOMETRY: [u32; 13] = [
    states::RS_RANGEFOGENABLE,
    states::RS_CLIPPING,
    states::RS_LIGHTING,
    states::RS_AMBIENT,
    states::RS_FOGVERTEXMODE,
    states::RS_COLORVERTEX,
    states::RS_LOCALVIEWER,
    states::RS_NORMALIZENORMALS,
    states::RS_DIFFUSEMATERIALSOURCE,
    states::RS_SPECULARMATERIALSOURCE,
    states::RS_AMBIENTMATERIALSOURCE,
    states::RS_EMISSIVEMATERIALSOURCE,
    states::RS_CLIPPLANEENABLE,
];

/// The render states the HAL sets: the rasteriser's, and `GEOMETRY`.
/// The others stay at their reset values.
fn render_states() -> impl Iterator<Item = u32> {
    RASTER.into_iter().chain(GEOMETRY)
}

/// The rasteriser's render states the HAL sets.
const RASTER: [u32; 39] = [
    states::RS_ZENABLE,
    states::RS_FILLMODE,
    states::RS_SHADEMODE,
    states::RS_ZWRITEENABLE,
    states::RS_ALPHATESTENABLE,
    states::RS_LASTPIXEL,
    states::RS_SRCBLEND,
    states::RS_DESTBLEND,
    states::RS_CULLMODE,
    states::RS_ZFUNC,
    states::RS_ALPHAREF,
    states::RS_ALPHAFUNC,
    states::RS_DITHERENABLE,
    states::RS_ALPHABLENDENABLE,
    states::RS_FOGENABLE,
    states::RS_SPECULARENABLE,
    states::RS_FOGCOLOR,
    states::RS_FOGTABLEMODE,
    states::RS_FOGSTART,
    states::RS_FOGEND,
    states::RS_FOGDENSITY,
    states::RS_COLORKEYENABLE,
    states::RS_ZBIAS,
    states::RS_STENCILENABLE,
    states::RS_STENCILFAIL,
    states::RS_STENCILZFAIL,
    states::RS_STENCILPASS,
    states::RS_STENCILFUNC,
    states::RS_STENCILREF,
    states::RS_STENCILMASK,
    states::RS_STENCILWRITEMASK,
    states::RS_TEXTUREFACTOR,
    states::RS_POINTSIZE,
    states::RS_POINTSIZE_MIN,
    states::RS_POINTSIZE_MAX,
    states::RS_COLORWRITEENABLE,
    states::RS_BLENDOP,
    states::RS_PIXEL_CENTRE,
    states::RS_FOG_BY_W,
];

/// Why the card leaves a draw, or a part of it, undone.
#[derive(Clone, Copy, Debug)]
enum Undrawn {
    /// The render target isn't a surface in card memory of a format
    /// and size the card draws into.
    Target,
    /// The depth buffer isn't one the card takes under this target.
    Depth,
    /// A texture is neither one the card samples nor one the HAL can
    /// copy.
    Texture,
}

fn hal(cpu: &mut Cpu) -> &mut Hal {
    &mut state(cpu).hal_toadforce
}

/// Log, the first time for each reason, that the card can't draw with
/// the surface `id` as the program asked.
fn undrawn(cpu: &mut Cpu, why: Undrawn, id: u32) {
    let bit = 1 << why as u32;
    if hal(cpu).logged & bit != 0 {
        return;
    }
    hal(cpu).logged |= bit;
    let (w, h, bits) = state(cpu)
        .surface(id)
        .map_or((0, 0, 0), |s| (s.width, s.height, s.format.bits));
    let what = match why {
        Undrawn::Target => format!(
            "the render target ({w}x{h}, {bits} bits) isn't one the card draws \
             into: draws into it are not drawn"
        ),
        Undrawn::Depth => format!(
            "the depth buffer ({w}x{h}, {bits} bits) isn't one the card takes: \
             draws with it are drawn without the depth test"
        ),
        Undrawn::Texture => format!(
            "a texture ({w}x{h}, {bits} bits) is neither one the card samples nor \
             one the HAL copies: draws with it are drawn without it"
        ),
    };
    cpu.bus
        .log_string(&format!("[W32] DDRAW: Direct3D HAL (ToadForce): {what}"));
}

/// Whether the display adapter is a ToadForce that has level 1.
pub fn there(cpu: &Cpu) -> bool {
    cpu.bus
        .toadforce
        .as_ref()
        .is_some_and(|card| card.level_max() >= 1)
}

// What the device reports.

/// The stage operations the card draws: all of Direct3D 7's but
/// PREMODULATE.
pub fn can_do(op: u32) -> bool {
    states::Kind::Op.takes(op) && op <= 24
}

fn rgb(bits: u32, a: u32, r: u32, g: u32, b: u32) -> DdFormat {
    DdFormat {
        flags: DDPF_RGB | if a != 0 { DDPF_ALPHAPIXELS } else { 0 },
        fourcc: 0,
        bits,
        r,
        g,
        b,
        a,
    }
}

/// The texture formats, the ones a program is likeliest to want first.
fn texture_formats() -> Vec<DdFormat> {
    let fourcc = |name: &[u8; 4]| DdFormat {
        flags: DDPF_FOURCC,
        fourcc: u32::from_le_bytes(*name),
        ..DdFormat::default()
    };
    let other = |flags: u32, bits: u32, r: u32, g: u32, b: u32, a: u32| DdFormat {
        flags,
        fourcc: 0,
        bits,
        r,
        g,
        b,
        a,
    };
    vec![
        rgb(16, 0, 0xF800, 0x07E0, 0x001F),
        rgb(16, 0x8000, 0x7C00, 0x03E0, 0x001F),
        rgb(16, 0xF000, 0x0F00, 0x00F0, 0x000F),
        rgb(32, 0xFF00_0000, 0xFF_0000, 0xFF00, 0xFF),
        rgb(32, 0, 0xFF_0000, 0xFF00, 0xFF),
        rgb(16, 0, 0x7C00, 0x03E0, 0x001F),
        other(DDPF_RGB | DDPF_PALETTEINDEXED8, 8, 0, 0, 0, 0),
        fourcc(b"DXT1"),
        fourcc(b"DXT2"),
        fourcc(b"DXT3"),
        fourcc(b"DXT4"),
        fourcc(b"DXT5"),
        other(DDPF_ALPHA, 8, 0, 0, 0, 0xFF),
        other(DDPF_LUMINANCE, 8, 0xFF, 0, 0, 0),
        other(DDPF_LUMINANCE | DDPF_ALPHAPIXELS, 16, 0xFF, 0, 0, 0xFF00),
        // The bump formats: du, dv and luminance where red, green and
        // blue are.
        other(DDPF_BUMPDUDV, 16, 0xFF, 0xFF00, 0, 0),
        other(
            DDPF_BUMPDUDV | DDPF_BUMPLUMINANCE,
            16,
            0x1F,
            0x3E0,
            0xFC00,
            0,
        ),
        other(
            DDPF_BUMPDUDV | DDPF_BUMPLUMINANCE,
            32,
            0xFF,
            0xFF00,
            0xFF_0000,
            0,
        ),
    ]
}

/// The depth buffer formats: 16 bits, and 24 bits above 8 of stencil or
/// of nothing. A depth buffer's format has its stencil bits where red
/// is, its depth mask where green is and its stencil mask where blue
/// is.
fn z_formats() -> Vec<DdFormat> {
    let z = |flags: u32, bits: u32, stencil: u32, mask: u32, stencil_mask: u32| DdFormat {
        flags: DDPF_ZBUFFER | flags,
        fourcc: 0,
        bits,
        r: stencil,
        g: mask,
        b: stencil_mask,
        a: 0,
    };
    vec![
        z(0, 16, 0, 0xFFFF, 0),
        z(DDPF_STENCILBUFFER, 32, 8, 0xFFFF_FF00, 0xFF),
        z(0, 32, 0, 0xFFFF_FF00, 0),
    ]
}

/// What the device description says of the card.
pub fn description() -> Description {
    Description {
        // DDBD_16 and DDBD_32. Depth buffers aren't DDBD_24: a program
        // that picks 24 from this makes a depth buffer of 3 bytes a
        // pixel, which the card has no format for. Its 24 bits of depth
        // are in a 32-bit buffer (`z_formats`).
        target_depths: 0x400 | 0x100,
        z_depths: 0x400 | 0x100,
        texture_max: caps::TEXTURE_MAX,
        // Any ratio a texture of the largest size can have, as the
        // cards of the time with no limit reported it.
        texture_aspect: caps::TEXTURE_MAX,
        // D3DPRASTERCAPS_: anisotropic filtering, table fog by w and by
        // z.
        raster: 0x2_0000 | 0x10_0000 | 0x20_0000,
        texture: 0,
        // D3DPTFILTERCAPS_: anisotropic minification and magnification,
        // and a linear filter between mipmap levels.
        filter: 0x400 | 0x400_0000 | 0x2_0000,
        // D3DPTADDRESSCAPS_: the border color.
        address: 0x8,
        // D3DPBLENDCAPS_: every factor.
        blend: 0x1FFF,
        guard_band: VERTEX_BAND,
        // D3DSTENCILCAPS_: all eight operations.
        stencil: 0xFF,
        // D3DTEXOPCAPS_: operations 1 to 24 but PREMODULATE (17).
        ops: 0xFF_FFFF & !(1 << 16),
        stages: caps::STAGES,
        anisotropy: caps::ANISOTROPY_MAX,
        textures: texture_formats(),
        z_formats: z_formats(),
    }
}

/// The card's format for a render target of `f`.
fn target_format(f: &DdFormat) -> Option<Format> {
    if f.flags & DDPF_RGB == 0 || f.flags & (DDPF_PALETTEINDEXED8 | DDPF_FOURCC) != 0 {
        return None;
    }
    match (f.bits, f.r, f.g, f.b, f.a) {
        (16, 0xF800, 0x07E0, 0x001F, 0) => Some(Format::R5G6B5),
        (32, 0xFF_0000, 0xFF00, 0xFF, 0) => Some(Format::X8R8G8B8),
        (32, 0xFF_0000, 0xFF00, 0xFF, 0xFF00_0000) => Some(Format::A8R8G8B8),
        _ => None,
    }
}

/// The card's format for a depth buffer of `f`. A 32-bit one is the
/// card's whatever masks the program gave it: programs make them with
/// depth in the low 24 bits, in the high 24 and in all 32, and the card
/// keeps its depth in the high 24 for each. Only a program that locks
/// its depth buffer could tell.
fn depth_format(f: &DdFormat) -> Option<Format> {
    if f.flags & DDPF_ZBUFFER == 0 {
        return None;
    }
    match f.bits {
        16 => Some(Format::D16),
        32 if f.flags & DDPF_STENCILBUFFER != 0 => Some(Format::D24S8),
        32 => Some(Format::D24X8),
        _ => None,
    }
}

/// Whether a surface of `format` can be a render target of the card: in
/// video memory, and 5-6-5 or 8-8-8 with or without alpha.
pub fn takes_target(format: &DdFormat, video: bool) -> bool {
    video && target_format(format).is_some()
}

pub fn takes_depth(format: &DdFormat, video: bool) -> bool {
    video && depth_format(format).is_some()
}

/// The card's format for a texture of `f`, if it samples one.
fn texture_format(f: &DdFormat) -> Option<Format> {
    if f.flags & DDPF_FOURCC != 0 {
        return match &f.fourcc.to_le_bytes() {
            b"DXT1" => Some(Format::DXT1),
            b"DXT2" => Some(Format::DXT2),
            b"DXT3" => Some(Format::DXT3),
            b"DXT4" => Some(Format::DXT4),
            b"DXT5" => Some(Format::DXT5),
            _ => None,
        };
    }
    if f.flags & DDPF_BUMPDUDV != 0 {
        return match (f.bits, f.r, f.g, f.b) {
            (16, 0xFF, 0xFF00, 0) => Some(Format::V8U8),
            (16, 0x1F, 0x3E0, 0xFC00) => Some(Format::L6V5U5),
            (32, 0xFF, 0xFF00, 0xFF_0000) => Some(Format::X8L8V8U8),
            _ => None,
        };
    }
    if f.flags & DDPF_LUMINANCE != 0 {
        return match (f.bits, f.r, f.a) {
            (8, 0xFF, 0) => Some(Format::L8),
            (16, 0xFF, 0xFF00) => Some(Format::A8L8),
            _ => None,
        };
    }
    if f.flags & DDPF_PALETTEINDEXED8 != 0 {
        // A palette is DirectDraw's object, which the card can't read:
        // these are copied with their colours.
        return None;
    }
    if f.flags & DDPF_RGB == 0 {
        return (f.flags & DDPF_ALPHA != 0 && f.bits == 8).then_some(Format::A8);
    }
    match (f.bits, f.r, f.g, f.b, f.a) {
        (16, 0xF800, 0x07E0, 0x001F, 0) => Some(Format::R5G6B5),
        (16, 0x7C00, 0x03E0, 0x001F, 0) => Some(Format::X1R5G5B5),
        (16, 0x7C00, 0x03E0, 0x001F, 0x8000) => Some(Format::A1R5G5B5),
        (16, 0x0F00, 0x00F0, 0x000F, 0xF000) => Some(Format::A4R4G4B4),
        (32, 0xFF_0000, 0xFF00, 0xFF, 0) => Some(Format::X8R8G8B8),
        (32, 0xFF_0000, 0xFF00, 0xFF, 0xFF00_0000) => Some(Format::A8R8G8B8),
        _ => None,
    }
}

// The card's state.

/// Hand the card the commands in `words`.
fn submit(cpu: &mut Cpu, words: &[u32]) {
    if !words.is_empty() {
        cpu.bus.toadforce_submit(words);
    }
}

/// What the HAL knows the card to have, with the card at level 1: a
/// card at another level, or one the HAL knows nothing of, starts from
/// the level's defaults.
fn sent(cpu: &mut Cpu) -> Sent {
    let level = cpu.bus.toadforce.as_ref().map_or(0, |card| card.level());
    match hal(cpu).sent.take() {
        Some(sent) if level == 1 => sent,
        _ => {
            cpu.bus.toadforce_register_write(regs::LEVEL, 1);
            Sent::reset()
        }
    }
}

/// The surface `id` in card memory: where it is and its pitch.
fn in_card(cpu: &mut Cpu, id: u32) -> Option<(u32, u32)> {
    let s = state(cpu).surface(id)?;
    (s.memory == MEM_VIDEO && !s.lost).then_some((s.bits, s.pitch))
}

/// The device's render target as the card takes it, with the depth
/// buffer if the card takes that too. None when the card can't draw
/// into the target.
fn target(cpu: &mut Cpu, d: &Device) -> Option<Target> {
    let memory = screen::memory_size(cpu);
    let colour = in_card(cpu, d.target).and_then(|(at, pitch)| {
        let s = state(cpu).surface(d.target)?;
        let format = target_format(&s.format)?;
        let words = [at, pitch, s.width, s.height, format.code()];
        CardSurface::parse(&words, caps::TARGET_FORMATS, memory).ok()
    });
    let Some(colour) = colour.filter(|c| c.width.max(c.height) <= caps::TARGET_MAX) else {
        undrawn(cpu, Undrawn::Target, d.target);
        return None;
    };
    let mut target = Target {
        colour,
        depth: None,
    };
    if let Some(z) = super::super::device::depth_buffer(cpu, d) {
        let depth = in_card(cpu, z).and_then(|(offset, pitch)| {
            let s = state(cpu).surface(z)?;
            let format = depth_format(&s.format)?;
            (s.width >= colour.width && s.height >= colour.height).then_some(Depth {
                offset,
                pitch,
                format,
            })
        });
        let with = Target { depth, ..target };
        // The card has the last word on what it takes.
        if depth.is_some() && takes(cpu, |w| encode::render_target(w, Some(&with))) {
            target = with;
        } else {
            undrawn(cpu, Undrawn::Depth, z);
        }
    }
    takes(cpu, |w| encode::render_target(w, Some(&target))).then_some(target)
}

/// Whether the card takes the command `build` writes, as far as its
/// arguments go: what `decode` checks before a command runs.
fn takes(cpu: &mut Cpu, build: impl FnOnce(&mut Vec<u32>)) -> bool {
    let mut words = Vec::new();
    build(&mut words);
    let memory = screen::memory_size(cpu);
    let opcode = (words[0] & 0xFFFF) as u16;
    level1::decode(opcode, &words, memory).is_ok()
}

/// A state's value for the card: the device's, or the state's reset
/// value when the device's is none the card takes.
fn checked(def: &Def, value: u32) -> u32 {
    if def.kind.takes(value) {
        value
    } else {
        def.reset
    }
}

/// The value of the render state `def` for the device `d`. With
/// `lit` the card transforms and lights the draw's vertices;
/// without, they are the pipeline's screen vertices.
fn render_value(d: &Device, def: &Def, setup: &Setup, depth: Option<Format>, lit: bool) -> u32 {
    use states::*;
    if !lit && GEOMETRY.contains(&def.id) {
        return def.reset;
    }
    let lighting = setup.lighting.as_ref();
    let source = |which: usize| lighting.map_or(0, |l| l.sources[which]);
    let value = match def.id {
        RS_CULLMODE if lit => setup.cull,
        RS_FILLMODE if lit => setup.fill,
        RS_SHADEMODE if lit => 2 - (setup.shade == SHADE_FLAT) as u32,
        RS_CLIPPING => setup.clip as u32,
        RS_LIGHTING => lighting.is_some() as u32,
        RS_AMBIENT => lighting.map_or(0, |l| pipeline::color(l.ambient)),
        RS_COLORVERTEX => lighting.is_some_and(|l| l.color_vertex) as u32,
        RS_LOCALVIEWER => lighting.is_some_and(|l| l.local_viewer) as u32,
        RS_NORMALIZENORMALS => lighting.is_some_and(|l| l.normalize) as u32,
        RS_DIFFUSEMATERIALSOURCE => source(0),
        RS_SPECULARMATERIALSOURCE => source(1),
        RS_AMBIENTMATERIALSOURCE => source(2),
        RS_EMISSIVEMATERIALSOURCE => source(3),
        RS_FOGVERTEXMODE => setup.fog.map_or(0, |f| f.mode),
        RS_RANGEFOGENABLE => setup.fog.is_some_and(|f| f.range) as u32,
        RS_CLIPPLANEENABLE => (1 << setup.planes.len().min(CLIP_PLANES)) - 1,
        // The pipeline culls, makes the lines and points of the other
        // fill modes, and gives a flat primitive's vertices one color.
        RS_CULLMODE => 1,
        RS_FILLMODE => 3,
        RS_SHADEMODE => 2,
        // Depth by w is depth by z on this card.
        RS_ZENABLE => (d.state(RS_ZENABLE) != 0 && depth.is_some()) as u32,
        RS_STENCILENABLE => (d.state(def.id) != 0 && depth == Some(Format::D24S8)) as u32,
        RS_ALPHAREF | RS_STENCILREF => d.state(def.id) & 0xFF,
        // OpenGL's depth bias is in steps of a 16-bit depth buffer.
        RS_ZBIAS => (d.state(def.id) as i64 + d.depth_bias as i64).clamp(0, 16) as u32,
        // Direct3D 7 has neither state. A device of version 8 has both:
        // Direct3D 8 sets them, and OpenGL puts its color mask, a
        // channel at a time, and the blend operation it always has
        // there before each draw.
        RS_COLORWRITEENABLE if d.version == 8 => d.state(def.id) & 0xF,
        RS_COLORWRITEENABLE => 0xF,
        RS_BLENDOP if d.version != 8 => 1,
        RS_PIXEL_CENTRE => d.gl_pixel_centres as u32,
        // Table fog goes by w when the projection gives a w that is
        // the distance from the eye, as Direct3D decides it.
        RS_FOG_BY_W => {
            let p = &setup.projection;
            ([p[3], p[7], p[11], p[15]] != [0.0, 0.0, 0.0, 1.0]) as u32
        }
        // Points come out of the pipeline as the card's own, a pixel
        // each.
        RS_POINTSIZE | RS_POINTSIZE_MIN | RS_POINTSIZE_MAX => def.reset,
        id => d.state(id),
    };
    checked(def, value)
}

/// The value of stage `stage`'s state `def` for the device `d`.
/// `diffuse_alpha` gives stage 0's alpha the diffuse one (see `bind`),
/// and `lit` is `render_value`'s.
fn stage_value(d: &Device, at: u32, def: &Def, diffuse_alpha: bool, lit: bool) -> u32 {
    use states::*;
    let value = d.stage(at, def.id);
    let value = match def.id {
        // The card makes the coordinates of a vertex it transforms as
        // the device's states say.
        TSS_TEXCOORDINDEX | TSS_TEXTURETRANSFORMFLAGS if lit => value,
        // The pipeline gives each of the first stages its coordinates.
        TSS_TEXCOORDINDEX => at.min(UNITS - 1),
        TSS_TEXTURETRANSFORMFLAGS => 0,
        TSS_ALPHAARG1 if at == 0 && diffuse_alpha => ARG_DIFFUSE,
        // The device has Direct3D 7's filter values: the two cubic
        // magnification filters are the linear one here, and the
        // anisotropic one is the card's third. Direct3D 8's are the
        // card's already, but for its two cubic filters (4 and 5),
        // which are the linear one here too.
        TSS_MAGFILTER if d.version != 8 => match value {
            0 | 1 => 1,
            5 => 3,
            _ => 2,
        },
        TSS_MAGFILTER | TSS_MINFILTER if value > 3 => 2,
        TSS_MAGFILTER | TSS_MINFILTER => value.clamp(1, 3),
        // The device keeps Direct3D 7's mipmap filters, from 1
        // (D3DTFP_NONE), on every version: Direct3D 8's states add 1
        // to theirs (`d3d8::states::inner_stage`).
        TSS_MIPFILTER => value.clamp(1, 3) - 1,
        TSS_MAXANISOTROPY => value.clamp(1, caps::ANISOTROPY_MAX),
        TSS_COLOR_SCALE | TSS_ALPHA_SCALE => 1,
        // Direct3D 7 has no third argument and no result register.
        TSS_COLORARG0 | TSS_ALPHAARG0 | TSS_RESULTARG if d.version != 8 => {
            stage_reset(def, at as usize)
        }
        _ => value,
    };
    if def.kind.takes(value) {
        value
    } else {
        stage_reset(def, at as usize)
    }
}

/// How a batch of primitives is drawn: nothing the card doesn't have in
/// its own state.
#[derive(Clone, Copy, Debug, Default)]
pub struct Card {
    /// The render target, for the count of what was drawn of the
    /// picture shown.
    target: (u32, u32, u32),
    /// The rectangle drawn into: left, top, right, bottom.
    clip: [u32; 4],
}

/// Whether the device `d` can be drawn by the card now. The card's
/// state is then the device's, for screen vertices.
pub fn bind(cpu: &mut Cpu, d: &Device, setup: &Setup) -> Option<Card> {
    bind_as(cpu, d, setup, false)
}

/// `bind`, with `lit` for vertices the card transforms and lights:
/// the card then has the device's matrices, lights, material and clip
/// planes too, which `can_transform` found it to take.
fn bind_as(cpu: &mut Cpu, d: &Device, setup: &Setup, lit: bool) -> Option<Card> {
    let target = target(cpu, d)?;
    let pid = current_pid(cpu);
    if hal(cpu).pid != pid {
        // The copies are in the video memory of the process that drew
        // last, which may still be running: their blocks go back.
        forget(cpu, |_| true);
        hal(cpu).pid = pid;
    }
    hal(cpu).clock = hal(cpu).clock.wrapping_add(1);
    let mut sent = sent(cpu);
    let mut words = Vec::new();

    if sent.target != Some(target) {
        encode::render_target(&mut words, Some(&target));
        sent.target = Some(target);
    }

    // The viewport's rectangle, which the card clips to, cut down by
    // OpenGL's scissor box.
    let (width, height) = (target.colour.width, target.colour.height);
    let m = &setup.view_map;
    let left = (m.ox + m.bounds[0] * m.sx).max(0.0) as u32;
    let right = ((m.ox + m.bounds[1] * m.sx).max(0.0) as u32).min(width);
    let top = (m.oy - m.bounds[3] * m.sy).max(0.0) as u32;
    let bottom = ((m.oy - m.bounds[2] * m.sy).max(0.0) as u32).min(height);
    let [left, top, right, bottom] = match d.scissor {
        Some(s) => [
            left.max(s[0]),
            top.max(s[1]),
            right.min(s[2]),
            bottom.min(s[3]),
        ],
        None => [left, top, right, bottom],
    };
    let (left, top) = (left.min(width), top.min(height));
    let mut viewport = Viewport {
        x: left,
        y: top,
        width: right.saturating_sub(left),
        height: bottom.saturating_sub(top),
        min_z: 0.0,
        max_z: 1.0,
    };
    let lit = lit && viewport.width != 0 && viewport.height != 0;
    if lit {
        // The card's viewport is the rectangle it draws into, which
        // the scissor box may have cut down, and the depths are the
        // device's.
        (viewport.min_z, viewport.max_z) = (m.oz, m.oz + m.sz);
    }
    if sent.viewport != viewport {
        encode::viewport(&mut words, &viewport);
        sent.viewport = viewport;
    }
    if lit {
        // The card puts clip x and y of -1 to 1 on its viewport. The
        // device's map from clip space to the screen comes first, as
        // a last step of the projection that undoes the card's.
        let (w, h) = (viewport.width as f32, viewport.height as f32);
        let mut fold = pipeline::IDENTITY;
        fold[0] = 2.0 * m.sx / w;
        fold[5] = 2.0 * m.sy / h;
        fold[12] = 2.0 * (m.ox - left as f32) / w - 1.0;
        fold[13] = 2.0 * (top as f32 - m.oy) / h + 1.0;
        let projection = pipeline::multiply(&setup.projection, &fold);
        let g = &mut sent.geometry;
        let mut transforms = vec![
            (level1::TRANSFORM_WORLD, setup.world),
            (level1::TRANSFORM_VIEW, setup.view),
            (level1::TRANSFORM_PROJECTION, projection),
        ];
        for stage in matrix_stages(d) {
            let matrix = d.transforms[TS_TEXTURE0 + stage as usize];
            transforms.push((level1::TRANSFORM_TEXTURE + stage, matrix));
        }
        for (which, matrix) in transforms {
            if g.transforms[which as usize] != matrix {
                encode::transform(&mut words, which, &matrix);
                g.transforms[which as usize] = matrix;
            }
        }
        if let Some(l) = &setup.lighting {
            let m = &l.material;
            let material = Material {
                diffuse: m.diffuse,
                ambient: m.ambient,
                specular: m.specular,
                emissive: m.emissive,
                power: m.power,
            };
            if g.material != material {
                encode::material(&mut words, &material);
                g.material = material;
            }
            for (slot, light) in l.lights.iter().enumerate() {
                let light = card_light(light);
                if g.lights[slot] != light {
                    encode::light(&mut words, slot as u32, &light);
                    g.lights[slot] = light;
                }
            }
            let on = (1u32 << l.lights.len()) - 1;
            for slot in 0..LIGHTS as u32 {
                if (g.on ^ on) >> slot & 1 != 0 {
                    encode::enable_light(&mut words, slot, on >> slot & 1 != 0);
                }
            }
            g.on = on;
        }
        for (i, plane) in setup.planes.iter().take(CLIP_PLANES).enumerate() {
            if g.planes[i] != *plane {
                encode::clip_plane(&mut words, i as u32, plane);
                g.planes[i] = *plane;
            }
        }
    }

    // The textures of the stages in use.
    let stages = (0..caps::STAGES)
        .take_while(|&s| d.stage(s, TSS_COLOROP) != TOP_DISABLE)
        .count() as u32;
    let mut first_alpha = None;
    let key = d.state(super::RS_COLORKEYENABLE) != 0;
    for unit in 0..UNITS {
        let surface = (unit < stages)
            .then(|| texture_of(cpu, d, unit as usize))
            .flatten();
        let texture = surface.and_then(|id| {
            let texture = texture(cpu, id, key);
            if texture.is_none() {
                undrawn(cpu, Undrawn::Texture, id);
            }
            texture
        });
        if unit == 0 {
            first_alpha = surface
                .and_then(|id| state(cpu).surface(id))
                .map(|s| s.format.a != 0);
        }
        let (i, handle) = (unit as usize, unit + 1);
        match texture {
            Some(texture) => {
                if sent.textures[i].as_ref() != Some(&texture) {
                    encode::define_texture(&mut words, handle, &texture);
                    // A handle defined anew stays in its stage.
                    sent.textures[i] = Some(texture);
                }
                if !sent.bound[i] {
                    encode::set_texture(&mut words, unit, handle);
                    sent.bound[i] = true;
                }
            }
            None if sent.bound[i] => {
                encode::set_texture(&mut words, unit, 0);
                sent.bound[i] = false;
            }
            None => {}
        }
    }

    // D3DTBLEND_MODULATE and MODULATEMASK take the diffuse alpha for a
    // texture without alpha, as Direct3D before 7 has them.
    let diffuse_alpha = d.version < 7
        && matches!(d.state(RS_TEXTUREMAPBLEND), 2 | 6)
        && d.stage(0, TSS_ALPHAOP) == TOP_SELECTARG1
        && d.stage(0, TSS_ALPHAARG1) & 0xF == TA_TEXTURE
        && first_alpha == Some(false);

    // The states that differ from the card's.
    let depth = target.depth.map(|depth| depth.format);
    let mut pairs = Vec::new();
    for (have, id) in sent.render.iter_mut().zip(render_states()) {
        let Some(def) = states::render(id) else {
            continue;
        };
        let want = render_value(d, def, setup, depth, lit);
        if *have != want {
            pairs.push((id, want));
            *have = want;
        }
    }
    if !pairs.is_empty() {
        encode::render_state(&mut words, &pairs);
    }
    let per_stage = states::STAGE.len();
    for stage in 0..caps::STAGES {
        pairs.clear();
        let have = &mut sent.stages[stage as usize * per_stage..][..per_stage];
        for (have, def) in have.iter_mut().zip(states::STAGE) {
            let want = stage_value(d, stage, def, diffuse_alpha, lit);
            if *have != want {
                pairs.push((def.id, want));
                *have = want;
            }
        }
        if !pairs.is_empty() {
            encode::stage_state(&mut words, stage, &pairs);
        }
    }
    submit(cpu, &words);
    hal(cpu).sent = Some(sent);
    Some(Card {
        target: (
            target.colour.offset,
            target.colour.pitch,
            target.colour.format.bytes(),
        ),
        clip: [left, top, right.max(left), bottom.max(top)],
    })
}

// Textures.

/// The surface of the texture in stage `stage`.
fn texture_of(cpu: &mut Cpu, d: &Device, stage: usize) -> Option<u32> {
    let iface = *d.textures.get(stage)?;
    if iface == 0 {
        return None;
    }
    let (_, id) = super::super::super::this(cpu, iface)?;
    state(cpu).surface(id)?;
    Some(id)
}

/// The surface `id` and the mipmap levels after it that halve each time,
/// as the card's levels do.
fn chain(cpu: &mut Cpu, id: u32) -> Vec<u32> {
    let mut levels = vec![id];
    let Some((mut w, mut h)) = state(cpu).surface(id).map(|s| (s.width, s.height)) else {
        return levels;
    };
    let mut at = id;
    while (w > 1 || h > 1)
        && let Some(next) = super::super::texture::next_level(cpu, at)
    {
        (w, h) = ((w / 2).max(1), (h / 2).max(1));
        let fits = state(cpu)
            .surface(next)
            .is_some_and(|l| (l.width, l.height) == (w, h));
        if !fits {
            break;
        }
        levels.push(next);
        at = next;
    }
    levels
}

/// The surface `id` as a texture of the card: where it is, or the
/// HAL's copy of it. `key`: D3DRENDERSTATE_COLORKEYENABLE is on.
fn texture(cpu: &mut Cpu, id: u32, key: bool) -> Option<Texture> {
    in_place(cpu, id).or_else(|| cached(cpu, id, key))
}

/// The surface `id` as a texture where it is: in card memory, in a
/// format the card samples, a power of two each way, with the levels
/// that are there the same way.
fn in_place(cpu: &mut Cpu, id: u32) -> Option<Texture> {
    let (offset, pitch) = in_card(cpu, id)?;
    let (width, height, f, key) = {
        let s = state(cpu).surface(id)?;
        let key = (s.key_set & 1 << KEY_SRC_BLT != 0).then_some(s.keys[KEY_SRC_BLT].0);
        (s.width, s.height, s.format, key)
    };
    let format = texture_format(&f)?;
    let block = format.block_bytes();
    // A block format's surface has its size where the pitch is.
    let pitch_of = |pitch: u32, width: u32| match block {
        Some(_) => format.line_bytes(width) as u32,
        None => pitch,
    };
    let mut images = vec![(offset, pitch_of(pitch, width))];
    for (n, level) in chain(cpu, id).into_iter().enumerate().skip(1) {
        let Some((offset, pitch)) = in_card(cpu, level) else {
            break;
        };
        images.push((offset, pitch_of(pitch, (width >> n).max(1))));
    }
    let texture = Texture {
        format,
        width,
        height,
        levels: images.len() as u32,
        cube: false,
        palette: None,
        key: key
            .filter(|_| block.is_none())
            .map(|key| key & format.defined_bits()),
        images,
    };
    takes(cpu, |w| encode::define_texture(w, 1, &texture)).then_some(texture)
}

fn hash_words(words: &[u32]) -> u32 {
    let mut h = 0x811C_9DC5u32;
    for w in words {
        h = (h ^ w).wrapping_mul(0x0100_0193);
    }
    h | 1
}

/// The HAL's copy of the surface `id`, made if it has none that is up
/// to date. `key`: the copy leaves out the texels of the surface's
/// color key, for D3DRENDERSTATE_COLORKEYENABLE.
fn cached(cpu: &mut Cpu, id: u32, key: bool) -> Option<Texture> {
    let (uniqueness, paletted, key) = {
        let s = state(cpu).surface(id)?;
        let paletted =
            s.format.flags & DDPF_PALETTEINDEXED8 != 0 && s.format.flags & DDPF_FOURCC == 0;
        let key = match key && s.key_set & 1 << KEY_SRC_BLT != 0 {
            true => [1, s.keys[KEY_SRC_BLT].0, s.keys[KEY_SRC_BLT].1],
            false => [0; 3],
        };
        (s.uniqueness, paletted, key)
    };
    // What a copy has besides the surface's pixels: a palettized
    // texture's has the palette's colors, and a keyed copy the color key.
    let mut variant = key.to_vec();
    if paletted {
        variant.extend(super::voodoo::palette(cpu, id));
    }
    let variant = hash_words(&variant);
    let clock = hal(cpu).clock;
    if let Some(c) = hal(cpu)
        .textures
        .iter_mut()
        .find(|c| c.surface == id && c.uniqueness == uniqueness && c.variant == variant)
    {
        c.used = clock;
        return Some(c.texture.clone());
    }
    forget(cpu, |c| c.surface == id);
    let image = image(cpu, id, key[0] != 0)?;
    let start = room(cpu, image.data.len() as u32)?;
    screen::write(cpu, start, &image.data);
    let mut texture = image.texture;
    for (offset, _) in &mut texture.images {
        *offset += start;
    }
    if !takes(cpu, |w| encode::define_texture(w, 1, &texture)) {
        display::free(cpu, start);
        return None;
    }
    hal(cpu).textures.push(Cached {
        surface: id,
        uniqueness,
        variant,
        start,
        used: clock,
        texture: texture.clone(),
    });
    Some(texture)
}

/// Drop the copies `gone` picks, and give their blocks back.
fn forget(cpu: &mut Cpu, gone: impl Fn(&Cached) -> bool) {
    let (drop, keep): (Vec<Cached>, Vec<Cached>) = std::mem::take(&mut hal(cpu).textures)
        .into_iter()
        .partition(|c| gone(c));
    hal(cpu).textures = keep;
    for c in drop {
        display::free(cpu, c.start);
    }
}

/// A block of `len` bytes of video memory for a copy, the least
/// recently used copies going until there is one. A copy this bind
/// took for another stage stays: its block is where the card samples
/// that stage's texture.
fn room(cpu: &mut Cpu, len: u32) -> Option<u32> {
    let clock = hal(cpu).clock;
    loop {
        if let Ok(at) = display::alloc(cpu, len.max(4)) {
            return Some(at);
        }
        let textures = &hal(cpu).textures;
        let oldest = textures
            .iter()
            .map(|c| c.used)
            .filter(|&u| u != clock)
            .min()?;
        forget(cpu, |c| c.used == oldest);
    }
}

/// A copy before it has a place: the texture with its images' offsets
/// from the start of `data`.
struct Image {
    texture: Texture,
    data: Vec<u8>,
}

/// The surface `id` and its mipmap levels as a copy for the card: its
/// own bytes when the card takes its format and size, and 8-8-8-8 at
/// the next power of two otherwise. `keyed`: the copy has the surface's
/// color key.
fn image(cpu: &mut Cpu, id: u32, keyed: bool) -> Option<Image> {
    // A surface in video memory, which the queued draws may draw on, is
    // read through `screen::read`, which has them drawn first.
    let s = state(cpu).surface(id)?.clone();
    let pow2 = s.width.is_power_of_two() && s.height.is_power_of_two();
    let fits = s.width.max(s.height) <= caps::TEXTURE_MAX;
    let mut data = Vec::new();
    if let Some(format) = texture_format(&s.format).filter(|_| pow2 && fits) {
        // The surface's bytes, each level's rows without padding.
        let mut images = Vec::new();
        for (n, level) in chain(cpu, id).into_iter().enumerate() {
            let l = state(cpu).surface(level)?.clone();
            let (w, h) = ((s.width >> n).max(1), (s.height >> n).max(1));
            let line = format.line_bytes(w) as u32;
            let lines = format.lines(h);
            // A block format's surface is its blocks, one after
            // another.
            let step = if l.compressed() { line } else { l.pitch };
            images.push((data.len() as u32, line));
            let mut row = vec![0u8; line as usize];
            for y in 0..lines {
                surface::read_bytes(cpu, &l, y * step, &mut row);
                data.extend_from_slice(&row);
            }
        }
        let key = (keyed && format.block_bytes().is_none())
            .then(|| s.keys[KEY_SRC_BLT].0 & format.defined_bits());
        return Some(Image {
            texture: Texture {
                format,
                width: s.width,
                height: s.height,
                levels: images.len() as u32,
                cube: false,
                palette: None,
                key,
                images,
            },
            data,
        });
    }
    if s.compressed() {
        return None;
    }
    // Converted: the nearest texel of the surface for each of the
    // copy's. In a keyed copy the texels of the color key are 0, which
    // no other texel of the copy is.
    let size = |n: u32| {
        n.clamp(1, caps::TEXTURE_MAX)
            .next_power_of_two()
            .min(caps::TEXTURE_MAX)
    };
    let (w, h) = (size(s.width), size(s.height));
    let levels = if pow2 && fits {
        chain(cpu, id)
    } else {
        vec![id]
    };
    let pal = super::voodoo::palette(cpu, id);
    let mut images = Vec::new();
    for (n, level) in levels.into_iter().enumerate() {
        let l = state(cpu).surface(level)?.clone();
        let (lw, lh) = ((w >> n).max(1), (h >> n).max(1));
        let src = super::voodoo::level_texels(cpu, &l);
        images.push((data.len() as u32, lw * 4));
        for y in 0..lh {
            for x in 0..lw {
                let sx = (x * l.width / lw).min(l.width.saturating_sub(1));
                let sy = (y * l.height / lh).min(l.height.saturating_sub(1));
                let raw = src.get((sy * l.width + sx) as usize).copied().unwrap_or(0);
                let (low, high) = l.keys[KEY_SRC_BLT];
                let texel = if keyed && (low..=high.max(low)).contains(&raw) {
                    0
                } else {
                    super::voodoo::argb(raw, &l, &pal).max(1)
                };
                data.extend_from_slice(&texel.to_le_bytes());
            }
        }
    }
    Some(Image {
        texture: Texture {
            format: Format::A8R8G8B8,
            width: w,
            height: h,
            levels: images.len() as u32,
            cube: false,
            palette: None,
            key: keyed.then_some(0),
            images,
        },
        data,
    })
}

// The devices' lives.

/// A device is made: the card goes to level 1.
pub fn created(cpu: &mut Cpu) {
    let sent = sent(cpu);
    hal(cpu).sent = Some(sent);
}

/// The device `gone` goes: with the last one the copies' blocks go back
/// to video memory, and the card lets go of the surfaces.
pub fn release(cpu: &mut Cpu, gone: u32) {
    let devices = state(cpu).ids(|o| o.d3d.as_ref().is_some_and(|d| d.device.is_some()));
    if devices.iter().any(|&id| id != gone) || !there(cpu) {
        return;
    }
    // A process that ended or lost its video memory has no copies left,
    // so the blocks given back are the copies' still.
    forget(cpu, |_| true);
    let known = hal(cpu).sent.is_some();
    *hal(cpu) = Hal::default();
    if known {
        // The level's defaults: no render target and no texture.
        cpu.bus.toadforce_register_write(regs::LEVEL, 1);
    }
}

/// A process ended: the blocks of its copies go back to video memory,
/// which gives back only its surfaces' itself.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    // What it had queued is drawn: the pictures it leaves are whole.
    screen::settle(cpu);
    if hal(cpu).pid == pid {
        forget(cpu, |_| true);
        hal(cpu).pid = 0;
    }
}

/// Video memory was lost: the copies' blocks went with it, and may be
/// other surfaces' now, so the copies are dropped without giving them
/// back. The textures are copied again when they are next drawn with.
pub fn video_memory_lost(cpu: &mut Cpu) {
    // The queued draws are for the surfaces that were there.
    screen::settle(cpu);
    hal(cpu).textures.clear();
}

// Drawing.

/// The card as the pipeline's rasterizer: the batch's primitives as
/// `DRAW` commands, handed to the card together at its end.
pub struct Raster<'a> {
    pub cpu: &'a mut Cpu,
    card: Card,
    /// The commands so far.
    words: Vec<u32>,
    /// The primitive of the command being filled (0 for none), and
    /// where its header is.
    primitive: u32,
    header: usize,
}

impl Raster<'_> {
    pub fn new(cpu: &mut Cpu, card: Card) -> Raster<'_> {
        Raster {
            cpu,
            card,
            words: Vec::new(),
            primitive: 0,
            header: 0,
        }
    }

    /// Close the command being filled: its length and count are known.
    fn close(&mut self) {
        if self.primitive == 0 {
            return;
        }
        let len = self.words.len() - self.header;
        let count = (len - 4) / VERTEX_WORDS;
        self.words[self.header] = crate::toadforce::command::header(level1::DRAW, len);
        self.words[self.header + 3] = count as u32;
        self.primitive = 0;
    }

    /// Add `vertices` to a command of `primitive`, a new one if the one
    /// being filled is of another or full.
    fn add(&mut self, primitive: u32, vertices: &[ScreenVertex]) {
        let full = self.words.len() - self.header >= 4 + DRAW_VERTICES * VERTEX_WORDS;
        if self.primitive != primitive || full {
            self.close();
            self.primitive = primitive;
            self.header = self.words.len();
            self.words.extend([0, primitive, FVF, 0]);
        }
        for v in vertices {
            self.words.extend([
                v.x.to_bits(),
                v.y.to_bits(),
                v.z.to_bits(),
                v.rhw.to_bits(),
                v.diffuse,
                v.specular,
                v.tex[0][0].to_bits(),
                v.tex[0][1].to_bits(),
                v.tex[1][0].to_bits(),
                v.tex[1][1].to_bits(),
            ]);
        }
    }

    /// The end of the batch: the card draws it.
    pub fn finish(mut self) {
        self.close();
        if self.words.is_empty() {
            return;
        }
        submit(self.cpu, &self.words);
        drew(self.cpu, &self.card);
    }
}

/// The card drew into the rectangle of `card`, in its memory itself,
/// past `screen::write`.
fn drew(cpu: &mut Cpu, card: &Card) {
    let [left, top, right, bottom] = card.clip;
    let (at, pitch, bytes) = card.target;
    let rect = crate::win32::gdi32::Rect {
        left: left as i32,
        top: top as i32,
        right: right as i32,
        bottom: bottom as i32,
    };
    screen::drew_rects(cpu, at, pitch, bytes, &[rect]);
}

// Vertices the card transforms and lights.

/// D3DTRANSFORMSTATE_TEXTURE0, the first of the device's texture
/// matrices.
const TS_TEXTURE0: usize = 16;

/// The words of a command at most, with room to spare under the
/// 65535 its length field holds.
const COMMAND_WORDS: usize = 60000;

/// The stages whose texture coordinates go through their stage's
/// texture matrix.
fn matrix_stages(d: &Device) -> impl Iterator<Item = u32> + '_ {
    (0..caps::STAGES)
        .filter(|&stage| d.stage(stage, states::TSS_TEXTURETRANSFORMFLAGS) & 0xFF != 0)
}

/// A light of the device as the card has one.
fn card_light(l: &pipeline::Light) -> Light {
    Light {
        // Direct3D 5's second spot light is a spot light.
        kind: if l.kind == LIGHT_GLSPOT {
            level1::LIGHT_SPOT
        } else {
            l.kind
        },
        diffuse: l.diffuse,
        specular: l.specular,
        ambient: l.ambient,
        position: l.position,
        direction: l.direction,
        // A range of 0 is no limit.
        range: if l.range > 0.0 { l.range } else { f32::MAX },
        falloff: l.falloff,
        attenuation: l.attenuation,
        theta: l.theta,
        phi: l.phi,
        spot_model: 0,
    }
}

/// Whether the card can transform and light a draw with `setup` as
/// the pipeline would: its state then holds everything the pipeline
/// goes by, in values the card's commands take.
fn can_transform(d: &Device, setup: &Setup) -> bool {
    let finite = |numbers: &[f32]| numbers.iter().all(|n| n.is_finite());
    let m = &setup.view_map;
    // The card cuts at z of 0 to w, and its depths are 0 to 1.
    let depths = m.bounds[4] == 0.0
        && m.bounds[5] == 1.0
        && (0.0..=1.0).contains(&m.oz)
        && (0.0..=1.0).contains(&(m.oz + m.sz))
        && m.sz >= 0.0;
    // A texture matrix no stage uses doesn't go to the card.
    let textures = matrix_stages(d).map(|stage| &d.transforms[TS_TEXTURE0 + stage as usize]);
    let matrices = [setup.world, setup.view, setup.projection]
        .iter()
        .chain(textures)
        .all(|matrix| finite(matrix));
    let lighting = setup.lighting.as_ref().is_none_or(|l| {
        let m = &l.material;
        l.lights.len() <= LIGHTS
            && finite(&[m.diffuse, m.ambient, m.specular, m.emissive].concat())
            && m.power.is_finite()
            && l.lights.iter().all(|light| {
                let l = card_light(light);
                (level1::LIGHT_POINT..=level1::LIGHT_DIRECTIONAL).contains(&l.kind)
                    && finite(&[l.diffuse, l.specular, l.ambient].concat())
                    && finite(&[l.position, l.direction, l.attenuation].concat())
                    && finite(&[l.range, l.falloff, l.theta, l.phi])
            })
    });
    // The states the card gets as the device has them.
    let stages = (0..caps::STAGES).all(|stage| {
        [states::TSS_TEXCOORDINDEX, states::TSS_TEXTURETRANSFORMFLAGS]
            .iter()
            .all(|&id| {
                let def = states::STAGE.iter().find(|def| def.id == id);
                def.is_some_and(|def| def.kind.takes(d.stage(stage, id)))
            })
    });
    let modes = (1..=3).contains(&setup.cull)
        && (1..=3).contains(&setup.fill)
        && setup.fog.is_none_or(|f| f.mode <= 3)
        && setup
            .lighting
            .as_ref()
            .is_none_or(|l| l.sources.iter().all(|&source| source <= 2));
    setup.blend.is_none()
        && stages
        && modes
        && depths
        && finite(&[m.ox, m.oy, m.sx, m.sy])
        && matrices
        && lighting
        && setup.planes.iter().all(|plane| finite(plane))
}

/// The card's layout for vertices of `layout`, and the words of one.
fn card_layout(layout: &Layout) -> (u32, usize) {
    let mut fvf = 0x002;
    let mut words = 3;
    for (present, bit, size) in [
        (layout.normal.is_some(), 0x010, 3),
        (layout.diffuse.is_some(), 0x040, 1),
        (layout.specular.is_some(), 0x080, 1),
    ] {
        if present {
            fvf |= bit;
            words += size;
        }
    }
    fvf |= (layout.textures as u32) << 8;
    for (set, &(_, numbers)) in layout.tex.iter().enumerate().take(layout.textures) {
        // Two numbers are size 0, three 1, four 2 and one 3.
        fvf |= [3, 0, 1, 2][numbers.clamp(1, 4) as usize - 1] << (16 + 2 * set);
        words += numbers.clamp(1, 4) as usize;
    }
    (fvf, words)
}

/// The words of the vertex `v` in the card's layout for `layout`.
fn card_vertex(words: &mut Vec<u32>, layout: &Layout, v: &Vertex) {
    words.extend(v.position[..3].iter().map(|n| n.to_bits()));
    if layout.normal.is_some() {
        words.extend(v.normal.iter().map(|n| n.to_bits()));
    }
    if layout.diffuse.is_some() {
        words.push(v.diffuse.unwrap_or(0xFFFF_FFFF));
    }
    if layout.specular.is_some() {
        words.push(v.specular.unwrap_or(0xFF00_0000));
    }
    for (set, &(_, numbers)) in layout.tex.iter().enumerate().take(layout.textures) {
        let numbers = numbers.clamp(1, 4) as usize;
        words.extend(v.tex[set][..numbers].iter().map(|n| n.to_bits()));
    }
}

/// The vertices of each primitive of a draw of `kind` over `n`
/// vertices, as a list: every point by itself, every line's two and
/// every triangle's three, the one that gives a flat triangle its
/// colour first. With them the list's kind and the vertices of one.
fn as_list(kind: u32, n: usize) -> (u32, usize, Vec<usize>) {
    use pipeline::{LINELIST, LINESTRIP, POINTLIST, TRIANGLELIST, TRIANGLESTRIP};
    let mut list = Vec::new();
    match kind {
        POINTLIST => list.extend(0..n),
        LINELIST => list.extend(0..n - n % 2),
        LINESTRIP => (1..n).for_each(|i| list.extend([i - 1, i])),
        TRIANGLELIST => list.extend(0..n - n % 3),
        // Every other triangle of a strip is wound the other way.
        TRIANGLESTRIP => (2..n).for_each(|i| match i % 2 {
            0 => list.extend([i - 2, i - 1, i]),
            _ => list.extend([i - 2, i, i - 1]),
        }),
        _ => (2..n).for_each(|i| list.extend([i - 1, i, 0])),
    }
    match kind {
        POINTLIST => (POINTLIST, 1, list),
        LINELIST | LINESTRIP => (LINELIST, 2, list),
        _ => (TRIANGLELIST, 3, list),
    }
}

/// Whether the environment asks for every draw to be transformed and
/// lit by the pipeline (`RUST_DOS_TOADFORCE_TNL=pipeline`): the way to
/// tell what the card's transform and lighting changes in a scene, by
/// drawing it both ways.
fn pipeline_transforms() -> bool {
    static PIPELINE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PIPELINE.get_or_init(|| {
        std::env::var("RUST_DOS_TOADFORCE_TNL").is_ok_and(|v| v.eq_ignore_ascii_case("pipeline"))
    })
}

/// The draw commands for `kind` primitives of `vertices` in the order
/// `order`, whose places are all among them, or as they are: one
/// command where it has at most `limit` words, and otherwise lists of
/// the draw's primitives in commands of at most that.
fn geometry_words(
    layout: &Layout,
    vertices: &[Vertex],
    kind: u32,
    order: Option<&[u32]>,
    limit: usize,
) -> Vec<u32> {
    let count = vertices.len();
    let (fvf, size) = card_layout(layout);
    let mut words = Vec::new();
    let mut all = Vec::new();
    // The draw in one command, where it fits: all the vertices of a
    // draw without indices, and of an indexed one those the indices
    // name, each once, since a program often draws a few triangles
    // of a large buffer.
    let whole = match order {
        None => {
            let fits = count <= 0x1_0000 && 5 + count * size <= limit;
            if fits {
                for v in vertices {
                    card_vertex(&mut all, layout, v);
                }
                encode::draw(&mut words, kind, fvf, count as u32, &all);
            }
            fits
        }
        Some(order) => {
            let mut places = vec![u32::MAX; count];
            let mut named = 0u32;
            let mut indexed = Vec::with_capacity(order.len());
            for &i in order {
                let place = &mut places[i as usize];
                if *place == u32::MAX {
                    *place = named;
                    named += 1;
                    // Past what one command holds, the rest isn't
                    // needed.
                    if named as usize * size <= limit {
                        card_vertex(&mut all, layout, &vertices[i as usize]);
                    }
                }
                indexed.push(*place as u16);
            }
            let fits = named <= 0x1_0000 && 5 + all.len() + indexed.len().div_ceil(2)
                <= limit
                && all.len() == named as usize * size;
            if fits {
                encode::draw_indexed(&mut words, kind, fvf, named, &all, &indexed);
            }
            fits
        }
    };
    if !whole {
        // A list of its primitives, in commands of as many as fit,
        // each with the vertices its primitives name.
        let (kind, each, list) = as_list(kind, order.map_or(count, |o| o.len()));
        let place = |i: usize| order.map_or(i, |o| o[i] as usize);
        all.clear();
        let mut indexed: Vec<u16> = Vec::new();
        let mut places = std::collections::HashMap::new();
        let flush = |words: &mut Vec<u32>, all: &mut Vec<u32>, indexed: &mut Vec<u16>| {
            if !indexed.is_empty() {
                let count = (all.len() / size) as u32;
                encode::draw_indexed(words, kind, fvf, count, all, indexed);
            }
            all.clear();
            indexed.clear();
        };
        for primitive in list.chunks_exact(each) {
            let room = 5 + all.len() + each * size + (indexed.len() + each).div_ceil(2);
            if room > limit {
                flush(&mut words, &mut all, &mut indexed);
                places.clear();
            }
            for &i in primitive {
                let at = place(i);
                let next = places.len() as u16;
                let index = *places.entry(at).or_insert_with(|| {
                    card_vertex(&mut all, layout, &vertices[at]);
                    next
                });
                indexed.push(index);
            }
        }
        flush(&mut words, &mut all, &mut indexed);
    }
    words
}

/// Draw `kind` primitives of `vertices`, which aren't in screen space,
/// in the order `order` or as they are: the card transforms and lights
/// them. False when the card can't draw the device, or can't as the
/// pipeline would: the pipeline then does.
pub fn draw_geometry(
    cpu: &mut Cpu,
    d: &Device,
    setup: &Setup,
    layout: &Layout,
    vertices: &[Vertex],
    kind: u32,
    order: Option<&[u32]>,
) -> bool {
    let count = vertices.len();
    if pipeline_transforms()
        || !can_transform(d, setup)
        || order.is_some_and(|o| o.iter().any(|&i| i as usize >= count))
    {
        return false;
    }
    let Some(card) = bind_as(cpu, d, setup, true) else {
        return false;
    };
    if card.clip[0] == card.clip[2] || card.clip[1] == card.clip[3] {
        return true;
    }
    let words = geometry_words(layout, vertices, kind, order, COMMAND_WORDS);
    if !words.is_empty() {
        submit(cpu, &words);
        drew(cpu, &card);
    }
    true
}

impl Rasterizer for Raster<'_> {
    fn triangle(&mut self, v: &[ScreenVertex; 3]) {
        self.add(4, v);
    }

    fn line(&mut self, v: &[ScreenVertex; 2]) {
        self.add(2, v);
    }

    fn point(&mut self, v: &ScreenVertex) {
        self.add(1, std::slice::from_ref(v));
    }
}

/// Fill rectangles (left, top, right, bottom) of the device `d`'s render
/// target and depth buffer as `fill` says, or the whole target with no
/// rectangles: the card's `CLEAR`.
pub fn clear(cpu: &mut Cpu, d: &Device, rects: &[[i32; 4]], fill: Fill) {
    let Some(target) = target(cpu, d) else {
        return;
    };
    let (width, height) = (target.colour.width as i32, target.colour.height as i32);
    let list: Vec<Rect> = rects
        .iter()
        .map(|r| [r[0].max(0), r[1].max(0), r[2].min(width), r[3].min(height)])
        .filter(|r| r[0] < r[2] && r[1] < r[3])
        .map(|r| Rect {
            left: r[0] as u32,
            top: r[1] as u32,
            width: (r[2] - r[0]) as u32,
            height: (r[3] - r[1]) as u32,
        })
        .collect();
    // Rectangles all past the target clear nothing; an empty list is
    // the whole of each buffer.
    if list.is_empty() && !rects.is_empty() {
        return;
    }
    let depth = target.depth.map(|depth| depth.format);
    let mut flags = 0;
    if fill.target.is_some() {
        flags |= level1::CLEAR_COLOUR;
    }
    if fill.depth.is_some() && depth.is_some() {
        flags |= level1::CLEAR_DEPTH;
    }
    if fill.stencil.is_some() && depth == Some(Format::D24S8) {
        flags |= level1::CLEAR_STENCIL;
    }
    if flags == 0 {
        return;
    }
    let z = fill.depth.unwrap_or(1.0);
    let z = if z.is_finite() {
        z.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let mut sent = sent(cpu);
    let mut words = Vec::new();
    if sent.target != Some(target) {
        encode::render_target(&mut words, Some(&target));
        sent.target = Some(target);
    }
    encode::clear(
        &mut words,
        flags,
        fill.target.unwrap_or(0),
        z,
        fill.stencil.unwrap_or(0) & 0xFF,
        &list,
    );
    submit(cpu, &words);
    hal(cpu).sent = Some(sent);
    let c = target.colour;
    if flags & level1::CLEAR_COLOUR != 0 {
        let whole = [crate::win32::gdi32::Rect {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        }];
        let drawn: Vec<_> = list
            .iter()
            .map(|r| crate::win32::gdi32::Rect {
                left: r.left as i32,
                top: r.top as i32,
                right: (r.left + r.width) as i32,
                bottom: (r.top + r.height) as i32,
            })
            .collect();
        let drawn = if drawn.is_empty() { &whole[..] } else { &drawn };
        screen::drew_rects(cpu, c.offset, c.pitch, c.format.bytes(), drawn);
        surface::changed(cpu, d.target);
    }
    if flags & (level1::CLEAR_DEPTH | level1::CLEAR_STENCIL) != 0
        && let Some(z) = super::super::device::depth_buffer(cpu, d)
    {
        surface::changed(cpu, z);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toadforce::command;
    use crate::toadforce::level1::{Command, Indices, Vertices};

    /// The triangles the commands in `words` draw, each as its
    /// vertices' x coordinates, which number the vertices here.
    fn triangles(words: &[u32]) -> Vec<[u32; 3]> {
        let mut found = Vec::new();
        let mut at = 0;
        while at < words.len() {
            let len = command::length(words[at]);
            let Ok(command::Command::Level1(Command::Draw(draw))) =
                command::decode(&words[at..at + len], 1, 1 << 24)
            else {
                panic!("not a draw at word {at}");
            };
            at += len;
            let Vertices::Inline(vertices) = draw.vertices else {
                panic!("vertices in a buffer");
            };
            let x = |i: usize| f32::from_bits(vertices[i * draw.fvf.words]) as u32;
            let order: Vec<usize> = match draw.indices {
                Indices::None => (0..draw.vertex_count()).collect(),
                Indices::Inline { words, count } => (0..count as usize)
                    .map(|i| (words[i / 2] >> (16 * (i % 2)) & 0xFFFF) as usize)
                    .collect(),
                Indices::Buffer { .. } => panic!("indices in a buffer"),
            };
            // As the card takes a list, a strip and a fan
            // (`raster::pieces`), turned so that the vertex a flat
            // triangle gets its colour from is first.
            let n = order.len();
            let of = |t: [usize; 3]| t.map(|i| x(order[i]));
            match draw.primitive {
                4 => found.extend((0..n / 3).map(|i| of([3 * i, 3 * i + 1, 3 * i + 2]))),
                5 => found.extend((2..n).map(|i| match i % 2 {
                    0 => of([i - 2, i - 1, i]),
                    _ => of([i - 2, i, i - 1]),
                })),
                6 => found.extend((2..n).map(|i| of([i - 1, i, 0]))),
                kind => panic!("primitive {kind}"),
            }
        }
        found
    }

    fn numbered(n: usize) -> Vec<Vertex> {
        (0..n)
            .map(|i| Vertex {
                position: [i as f32, 0.0, 0.0, 1.0],
                ..Vertex::default()
            })
            .collect()
    }

    #[test]
    fn a_draw_too_large_for_one_command_goes_as_lists_of_the_same_triangles() {
        let layout = Layout::of(0x002).unwrap();
        let vertices = numbered(40);
        let order: Vec<u32> = (0..60).map(|i| (i * 7 % 40) as u32).collect();
        for kind in [
            pipeline::TRIANGLELIST,
            pipeline::TRIANGLESTRIP,
            pipeline::TRIANGLEFAN,
        ] {
            for order in [None, Some(&order[..])] {
                let whole = geometry_words(&layout, &vertices, kind, order, COMMAND_WORDS);
                assert_eq!(command::length(whole[0]), whole.len(), "one command");
                // A command of 30 words holds few triangles.
                let split = geometry_words(&layout, &vertices, kind, order, 30);
                assert!(command::framed(&split));
                let mut at = 0;
                let mut commands = 0;
                while at < split.len() {
                    assert!(command::length(split[at]) <= 30);
                    at += command::length(split[at]);
                    commands += 1;
                }
                assert!(commands > 3, "{commands} commands");
                assert_eq!(triangles(&split), triangles(&whole), "kind {kind}");
            }
        }
    }

    #[test]
    fn an_indexed_draw_sends_the_vertices_its_indices_name() {
        let layout = Layout::of(0x002).unwrap();
        let vertices = numbered(1000);
        let order = [998, 3, 500, 3, 500, 999];
        let words = geometry_words(
            &layout,
            &vertices,
            pipeline::TRIANGLELIST,
            Some(&order),
            COMMAND_WORDS,
        );
        // The header, the kind, the layout and the two counts, four
        // vertices of three words and six indices in three.
        assert_eq!(words.len(), 5 + 4 * 3 + 3);
        assert_eq!(triangles(&words), [[998, 3, 500], [3, 500, 999]]);
    }
}
