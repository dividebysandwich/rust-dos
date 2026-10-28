//! The HAL: Direct3D's drawing on the card, through the Glide layer
//! (`win32::glide::api`), as 3dfx's Direct3D driver drew on a Banshee
//! or Voodoo3.
//!
//! Before each batch of primitives `bind` puts the device's state on
//! the card: the render target and depth buffer are the surfaces' blocks
//! of the card's memory, the texture stages become the texture units'
//! and the color and alpha combine units' functions, and blending, the
//! alpha test, depth and fog become Glide's. What the card can't do is
//! done the nearest way it can, and ValidateDevice says which
//! operations those are. `Card` then takes the pipeline's screen
//! vertices and draws them with Glide's triangle setup.
//!
//! On a Voodoo Graphics or Voodoo2 add-on card (the 3dfx device of
//! `voodoo.rs`) the render target is the card's front or back buffer,
//! drawn into through Glide's render buffer, and the depth buffer is
//! its auxiliary buffer.
//!
//! On a Banshee or Voodoo3 a texture surface in video memory that the
//! card can sample as it is (565, 1555, 4444 or 8-bit paletted, a
//! power-of-two size up to 256, aspect up to 8 to 1, rows without
//! padding) is sampled where it is, with the mipmap levels that follow
//! it the way the card lays them out; its color key is the card's
//! chroma key.
//!
//! Other textures are copied into a heap of the card's memory, a block
//! of video memory the HAL takes from DirectDraw when it first draws
//! (on an add-on card, each texture unit's own memory), as 565, 1555
//! or 4444, power-of-two sizes up to 256 and aspects up to 8 to 1. A
//! texture is copied again when its surface changed (its uniqueness
//! value, which Unlock, Blt and Load change), and the least recently
//! used ones make room when the heap is full. A texture with a source
//! color key, drawn with D3DRENDERSTATE_COLORKEYENABLE, is copied as
//! 1555 with the keyed texels' alpha 0, and the alpha test leaves them
//! out.

use super::super::surface::{self, DDPF_PALETTEINDEXED8, KEY_SRC_BLT, MEM_CARD, Surface};
use super::super::{current_pid, display, state, voodoo};
use super::device::{
    Device, RS_FOGENABLE, RS_FOGTABLEMODE, RS_SPECULARENABLE, RS_TEXTUREFACTOR, RS_TEXTUREMAPBLEND,
    RS_ZENABLE, RS_ZFUNC, RS_ZWRITEENABLE, TA_TEXTURE, TA_TFACTOR, TOP_ADD, TOP_BLENDTEXTUREALPHA,
    TOP_DISABLE, TOP_MODULATE, TOP_SELECTARG1, TOP_SELECTARG2, TSS_ADDRESSU, TSS_ADDRESSV,
    TSS_ALPHAARG1, TSS_ALPHAARG2, TSS_ALPHAOP, TSS_COLORARG1, TSS_COLORARG2, TSS_COLOROP,
    TSS_MAGFILTER, TSS_MINFILTER, TSS_MIPFILTER,
};
use super::pipeline::{self, Rasterizer, ScreenVertex, Setup};
use crate::cpu::Cpu;
use crate::win32::glide::api::{self, TexInfo, TmuVertex, Vertex};
use crate::win32::glide::hw;

/// A texture in the heap: the surface it is of and its uniqueness value
/// then, what kind of copy it is (`variant`), the texture unit whose
/// memory it is in (0 on a Banshee or Voodoo3, whose units share it),
/// where it is, and when it was last drawn with.
#[derive(Clone, Debug, Default)]
pub struct Cached {
    pub surface: u32,
    pub tmu: u32,
    pub uniqueness: u32,
    pub variant: u32,
    pub start: u32,
    pub size: u32,
    pub info: [u32; 4],
    pub keyed: bool,
    pub used: u32,
}

crate::state_fields!(Cached {
    surface,
    tmu,
    uniqueness,
    variant,
    start,
    size,
    info,
    keyed,
    used
});

/// The HAL's state: the system's, as there is one card.
#[derive(Clone, Debug, Default)]
pub struct Hal {
    /// The process whose devices draw, and the texture heap's block of
    /// video memory and its size (0: none). An add-on card has no block:
    /// its texture units' memories are the heap.
    pub pid: u32,
    pub heap: u32,
    pub heap_len: u32,
    pub textures: Vec<Cached>,
    /// A count of binds, for the least recently used texture.
    pub clock: u32,
    /// The palette each texture unit has, by its hash.
    pub palettes: [u32; 2],
    /// The 16-bit shadows of 24- and 32-bit render targets and depth
    /// buffers (`shadow.rs`).
    pub shadows: Vec<super::shadow::Shadow>,
}

crate::state_fields!(Hal {
    pid,
    heap,
    heap_len,
    textures,
    clock,
    palettes,
    shadows
});

fn hal(cpu: &mut Cpu) -> &mut Hal {
    &mut state(cpu).hal
}

// Render states the HAL reads.
pub const RS_ALPHATESTENABLE: u32 = 15;
const RS_SRCBLEND: u32 = 19;
const RS_DESTBLEND: u32 = 20;
pub const RS_ALPHAREF: u32 = 24;
const RS_ALPHAFUNC: u32 = 25;
pub const RS_DITHERENABLE: u32 = 26;
pub const RS_ALPHABLENDENABLE: u32 = 27;
pub const RS_FOGCOLOR: u32 = 34;
const RS_FOGSTART: u32 = 36;
const RS_FOGEND: u32 = 37;
const RS_FOGDENSITY: u32 = 38;
pub const RS_COLORKEYENABLE: u32 = 41;

/// The heap's size at the most, and the least worth having.
const HEAP_MAX: u32 = 4 << 20;
const HEAP_MIN: u32 = 256 << 10;

/// How a batch of primitives is drawn: for each of the card's texture
/// units, the factors that turn a texture coordinate into Glide's (its
/// texture's size in Glide's 256 units) and the stage whose coordinates
/// it takes; whether Z or W carries the vertex fog; and how the
/// specular color is added.
#[derive(Clone, Copy, Debug, Default)]
pub struct Card {
    tmus: [Option<(usize, [f32; 2])>; 2],
    fog_z: bool,
    fog_w: bool,
    specular: Specular,
}

/// How the specular color is added (D3DRENDERSTATE_SPECULARENABLE).
/// The card iterates one color, so without a texture the diffuse and
/// specular colors are added at the vertices. With one, the specular
/// color is a second pass over the batch's triangles, added to what the
/// first drew: blended with the first's source factor and ONE, the fog
/// color black (which scales it by the fog's 1 - f), and where depth
/// was written, only on pixels of equal depth.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Specular {
    #[default]
    Off,
    Vertices,
    Pass {
        source: u32,
        equal: bool,
    },
}

/// Whether the device `d` can be drawn by the card now, and the card's
/// state for it: None draws nothing (no card, the render target isn't
/// in its memory, or another process has Glide open).
pub fn bind(cpu: &mut Cpu, d: &Device, setup: &Setup) -> Option<Card> {
    let banshee = hw::banshee(cpu);
    let pid = current_pid(cpu);
    let owner = crate::win32::glide::state(cpu).owner;
    if owner != 0 && owner != pid {
        return None;
    }
    let (width, height) = state(cpu).surface(d.target).map(|s| (s.width, s.height))?;
    if banshee {
        // A 24- or 32-bit target is drawn in its 16-bit shadow.
        let (target_at, target_pitch) = super::shadow::place(cpu, d.target)?;
        super::shadow::drawn(cpu, d.target);
        if !crate::win32::glide::state(cpu).gc.active {
            api::attach(cpu, width, height);
        }
        api::set_render_target(cpu, target_at, target_pitch, width, height);
    } else {
        // The add-on card, open for this process by SetDisplayMode.
        let buffer = voodoo::card_buffer(cpu, d.target)?;
        if buffer > api::BUFFER_BACKBUFFER || owner != pid {
            return None;
        }
        if !crate::win32::glide::state(cpu).gc.active {
            return None;
        }
        if hal(cpu).pid != pid {
            *hal(cpu) = Hal {
                pid,
                ..Hal::default()
            };
        }
        api::render_buffer(cpu, buffer);
    }
    hal(cpu).clock = hal(cpu).clock.wrapping_add(1);

    // The viewport's rectangle.
    let m = &setup.view_map;
    let left = (m.ox + m.bounds[0] * m.sx).max(0.0) as u32;
    let right = ((m.ox + m.bounds[1] * m.sx).max(0.0) as u32).min(width);
    let top = (m.oy - m.bounds[3] * m.sy).max(0.0) as u32;
    let bottom = ((m.oy - m.bounds[2] * m.sy).max(0.0) as u32).min(height);
    // OpenGL's scissor box cuts it down further.
    let [left, top, right, bottom] = match d.scissor {
        Some(s) => [
            left.max(s[0]),
            top.max(s[1]),
            right.min(s[2]),
            bottom.min(s[3]),
        ],
        None => [left, top, right, bottom],
    };
    api::clip_window(cpu, left, top, right.max(left), bottom.max(top));
    api::cull_mode(cpu, api::CULL_DISABLE);
    api::color_mask(cpu, !d.color_off, false);
    api::depth_bias_level(cpu, d.depth_bias);
    api::chromakey_mode(cpu, api::CHROMAKEY_DISABLE);
    let dither = d.state(RS_DITHERENABLE) != 0;
    api::dither_mode(
        cpu,
        if dither {
            api::DITHER_4X4
        } else {
            api::DITHER_DISABLE
        },
    );

    // Fog: the table by W, or the vertices' through W, the card's W
    // being free while depth is by Z. With depth by W the vertices' fog
    // goes through Z.
    let fog_on = d.state(RS_FOGENABLE) != 0;
    let table = d.state(RS_FOGTABLEMODE);
    let zenable = d.state(RS_ZENABLE);
    let (mut fog_z, mut fog_w) = (false, false);
    if fog_on && table != pipeline::FOG_NONE {
        let (start, end) = (d.state_f(RS_FOGSTART), d.state_f(RS_FOGEND));
        let density = d.state_f(RS_FOGDENSITY);
        let entries: [u8; api::FOG_TABLE_SIZE] = std::array::from_fn(|i| {
            let w = fog_table_index_to_w(i as i32);
            let f = pipeline::fog_factor(table, start, end, density, w);
            ((1.0 - f) * 255.0) as u8
        });
        api::fog_table(cpu, &entries);
        api::fog_mode(cpu, api::FOG_WITH_TABLE);
    } else if fog_on && zenable == 2 {
        fog_z = true;
        api::fog_mode(cpu, api::FOG_WITH_ITERATED_Z);
    } else if fog_on {
        fog_w = true;
        api::fog_mode(cpu, api::FOG_WITH_ITERATED_W);
    } else {
        api::fog_mode(cpu, api::FOG_DISABLE);
    }
    api::fog_color_value(cpu, d.state(RS_FOGCOLOR) & 0xFF_FFFF);
    // The texture units' W apart from the card's when that is the fog.
    let hint = if fog_w {
        api::STWHINT_W_DIFF_TMU0 | api::STWHINT_W_DIFF_TMU1
    } else {
        0
    };
    api::hints(cpu, api::HINT_STWHINT, hint);

    // Depth: the render target's depth buffer, Z or W. On an add-on card
    // it is the auxiliary buffer.
    let z = super::device::depth_buffer(cpu, d).and_then(|z| {
        if banshee {
            let place = super::shadow::place(cpu, z);
            super::shadow::drawn(cpu, z);
            place.map(Some)
        } else {
            (voodoo::card_buffer(cpu, z) == Some(api::BUFFER_AUXBUFFER)).then_some(None)
        }
    });
    match z {
        Some(place) if zenable != 0 => {
            if let Some((at, pitch)) = place {
                api::set_depth_buffer(cpu, at, pitch);
            }
            let mode = if zenable == 2 {
                api::DEPTHBUFFER_WBUFFER
            } else {
                api::DEPTHBUFFER_ZBUFFER
            };
            api::depth_buffer_mode(cpu, mode);
            api::depth_buffer_function(cpu, compare(d.state(RS_ZFUNC)));
            api::depth_mask(cpu, d.state(RS_ZWRITEENABLE) != 0);
        }
        _ => {
            api::depth_buffer_mode(cpu, api::DEPTHBUFFER_DISABLE);
            api::depth_mask(cpu, false);
        }
    }

    // Blending.
    let mut source = api::BLEND_ONE;
    if d.state(RS_ALPHABLENDENABLE) != 0 {
        let (src, dst) = blend(d.state(RS_SRCBLEND), d.state(RS_DESTBLEND));
        source = src;
        api::alpha_blend_function(cpu, src, dst, api::BLEND_ONE, api::BLEND_ZERO);
    } else {
        api::alpha_blend_function(cpu, api::BLEND_ONE, api::BLEND_ZERO, api::BLEND_ONE, 0);
    }
    let depth_written = z.is_some() && zenable != 0 && d.state(RS_ZWRITEENABLE) != 0;

    // The texture stages.
    let tmus = hw::board(cpu).map_or(1, |b| b.tmus);
    let key = d.state(RS_COLORKEYENABLE) != 0;
    let texture_of = |cpu: &mut Cpu, stage: usize| -> Option<u32> {
        let iface = *d.textures.get(stage)?;
        if iface == 0 {
            return None;
        }
        let (_, id) = super::super::this(cpu, iface)?;
        state(cpu).surface(id)?;
        Some(id)
    };
    let uses_texture = |stage: u32| {
        let op = d.stage(stage, TSS_COLOROP);
        let aop = d.stage(stage, TSS_ALPHAOP);
        let color = op != TOP_DISABLE
            && (d.stage(stage, TSS_COLORARG1) & 0xF == TA_TEXTURE
                || d.stage(stage, TSS_COLORARG2) & 0xF == TA_TEXTURE);
        let alpha = aop != TOP_DISABLE
            && (d.stage(stage, TSS_ALPHAARG1) & 0xF == TA_TEXTURE
                || d.stage(stage, TSS_ALPHAARG2) & 0xF == TA_TEXTURE);
        color || alpha
    };
    let first = if uses_texture(0) {
        texture_of(cpu, 0)
    } else {
        None
    };
    let second = if first.is_some() && tmus > 1 && d.stage(1, TSS_COLOROP) != TOP_DISABLE {
        if uses_texture(1) {
            texture_of(cpu, 1)
        } else {
            None
        }
    } else {
        None
    };
    let mut card = Card {
        fog_z,
        fog_w,
        ..Card::default()
    };
    let mut keyed = false;
    // Stage 0 is the upstream unit (TMU1) when both are in use, TMU0
    // alone.
    let mut loaded = [false; 2];
    match (first, second) {
        (Some(t0), Some(t1)) => {
            if let Some((scale, k)) = load(cpu, d, 0, t0, 1, key) {
                card.tmus[1] = Some((0, scale));
                keyed |= k;
                loaded[1] = true;
            }
            if let Some((scale, k)) = load(cpu, d, 1, t1, 0, key) {
                card.tmus[0] = Some((1, scale));
                keyed |= k;
                loaded[0] = true;
            }
        }
        (Some(t0), None) => {
            if let Some((scale, k)) = load(cpu, d, 0, t0, 0, key) {
                card.tmus[0] = Some((0, scale));
                keyed |= k;
                loaded[0] = true;
            }
        }
        _ => {}
    }
    for tmu in 0..tmus.min(2) {
        if !loaded[tmu as usize] {
            api::tex_combine(cpu, tmu, 0, 0, 0, 0, false, false);
        }
    }
    let textured = loaded[0];
    // Each unit's own texture coordinates when both draw: without the
    // hint the upstream unit takes the downstream one's.
    if loaded[0] && loaded[1] {
        api::hints(cpu, api::HINT_STWHINT, hint | api::STWHINT_ST_DIFF_TMU1);
    }
    // A keyed texture on the upstream unit alone isn't drawn.
    let keyed = keyed && textured;
    if textured {
        if loaded[1] {
            api::tex_combine(cpu, 1, local(), 0, local(), 0, false, false);
            let (f, factor) = tmu_function(d.stage(1, TSS_COLOROP), d, 1, false);
            let (af, afactor) = tmu_function(d.stage(1, TSS_ALPHAOP), d, 1, true);
            api::tex_combine(cpu, 0, f, factor, af, afactor, false, false);
        } else {
            api::tex_combine(cpu, 0, local(), 0, local(), 0, false, false);
        }
    }
    // D3DTBLEND_MODULATE and MODULATEMASK take the diffuse alpha for a
    // texture without alpha, as Direct3D before 7 has them.
    let legacy = d.version < 7
        && matches!(d.state(RS_TEXTUREMAPBLEND), 2 | 6)
        && d.stage(0, TSS_ALPHAOP) == TOP_SELECTARG1
        && d.stage(0, TSS_ALPHAARG1) & 0xF == TA_TEXTURE;
    let diffuse_alpha = legacy
        && first
            .and_then(|id| state(cpu).surface(id))
            .is_some_and(|s| s.format.a == 0);
    fbi_combine(cpu, d, textured, diffuse_alpha, keyed);
    if d.state(RS_SPECULARENABLE) != 0 {
        card.specular = if textured {
            Specular::Pass {
                source,
                equal: depth_written,
            }
        } else {
            Specular::Vertices
        };
    }

    // The alpha test, and the keyed texels it leaves out: the program's
    // test, unless it passes every alpha (ALWAYS, GREATEREQUAL 0,
    // LESSEQUAL 255). A test that passes alpha 0 and not every alpha
    // can't also leave the keyed texels out, and stays the program's.
    let (func, reference) = (d.state(RS_ALPHAFUNC), d.state(RS_ALPHAREF) & 0xFF);
    let passes_all = func == 8 || (func == 7 && reference == 0) || (func == 4 && reference == 0xFF);
    if d.state(RS_ALPHATESTENABLE) != 0 && !(keyed && passes_all) {
        api::alpha_test_function(cpu, compare(func));
        api::alpha_test_reference_value(cpu, reference);
    } else if keyed {
        api::alpha_test_function(cpu, compare(5));
        api::alpha_test_reference_value(cpu, 0);
    } else {
        api::alpha_test_function(cpu, api::CMP_ALWAYS);
    }
    Some(card)
}

fn local() -> u32 {
    api::COMBINE_FUNCTION_LOCAL
}

/// D3DCMP_ as Glide's comparisons: one less.
fn compare(func: u32) -> u32 {
    func.clamp(1, 8) - 1
}

/// D3DBLEND_ source and destination factors as the card's.
fn blend(src: u32, dst: u32) -> (u32, u32) {
    const ZERO: u32 = 0;
    const SRC_ALPHA: u32 = 1;
    const COLOR: u32 = 2;
    const DST_ALPHA: u32 = 3;
    const ONE: u32 = 4;
    const OM_SRC_ALPHA: u32 = 5;
    const OM_COLOR: u32 = 6;
    const OM_DST_ALPHA: u32 = 7;
    const SATURATE: u32 = 0xF;
    match src {
        // BOTHSRCALPHA and BOTHINVSRCALPHA set both.
        12 => return (SRC_ALPHA, OM_SRC_ALPHA),
        13 => return (OM_SRC_ALPHA, SRC_ALPHA),
        _ => {}
    }
    let s = match src {
        1 => ZERO,
        5 => SRC_ALPHA,
        6 => OM_SRC_ALPHA,
        7 => DST_ALPHA,
        8 => OM_DST_ALPHA,
        9 => COLOR,
        10 => OM_COLOR,
        11 => SATURATE,
        // SRCCOLOR and INVSRCCOLOR: the card has neither as a source
        // factor.
        4 => ZERO,
        _ => ONE,
    };
    let d = match dst {
        2 => ONE,
        3 => COLOR,
        4 => OM_COLOR,
        5 => SRC_ALPHA,
        6 => OM_SRC_ALPHA,
        7 => DST_ALPHA,
        8 => OM_DST_ALPHA,
        // DESTCOLOR and INVDESTCOLOR: neither as a destination factor.
        9 => ONE,
        _ => ZERO,
    };
    (s, d)
}

/// The function of the texture unit that has stage 1's texture, with
/// stage 0's result from the other unit: for its color, or its alpha.
fn tmu_function(op: u32, d: &Device, stage: u32, alpha: bool) -> (u32, u32) {
    let (a1, a2) = if alpha {
        (d.stage(stage, TSS_ALPHAARG1), d.stage(stage, TSS_ALPHAARG2))
    } else {
        (d.stage(stage, TSS_COLORARG1), d.stage(stage, TSS_COLORARG2))
    };
    let select = |arg: u32| {
        if arg & 0xF == TA_TEXTURE {
            (api::COMBINE_FUNCTION_LOCAL, api::COMBINE_FACTOR_NONE)
        } else {
            (api::COMBINE_FUNCTION_SCALE_OTHER, api::COMBINE_FACTOR_ONE)
        }
    };
    match op {
        TOP_DISABLE => (api::COMBINE_FUNCTION_SCALE_OTHER, api::COMBINE_FACTOR_ONE),
        TOP_SELECTARG1 => select(a1),
        TOP_SELECTARG2 => select(a2),
        TOP_ADD => (
            api::COMBINE_FUNCTION_SCALE_OTHER_ADD_LOCAL,
            api::COMBINE_FACTOR_ONE,
        ),
        TOP_BLENDTEXTUREALPHA => (
            api::COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL_ADD_LOCAL,
            api::COMBINE_FACTOR_ONE_MINUS_LOCAL_ALPHA,
        ),
        // MODULATE and the rest.
        _ => (api::COMBINE_FUNCTION_SCALE_OTHER, api::COMBINE_FACTOR_LOCAL),
    }
}

/// The color and alpha combine units for stage 0's operations: the
/// texture units' result is the texture, the iterated color the diffuse
/// color. `diffuse_alpha` makes stage 0's alpha the diffuse one.
/// `keyed`: the texture is a keyed copy, whose keyed texels have alpha 0
/// and the others their alpha's top bit (1 for a texture without
/// alpha), and the alpha test leaves out what comes to 0.
fn fbi_combine(cpu: &mut Cpu, d: &Device, textured: bool, diffuse_alpha: bool, keyed: bool) {
    let it = api::COMBINE_LOCAL_ITERATED;
    let (other_tex, other_it) = (api::COMBINE_OTHER_TEXTURE, api::COMBINE_OTHER_ITERATED);
    let arg_is_texture = |arg: u32| textured && arg & 0xF == TA_TEXTURE;
    // D3DTA_TFACTOR: the texture factor, the card's constant color.
    let arg_is_factor = |arg: u32| arg & 0xF == TA_TFACTOR;
    let other_of = |arg: u32| {
        if arg_is_texture(arg) {
            other_tex
        } else if arg_is_factor(arg) {
            api::COMBINE_OTHER_CONSTANT
        } else {
            other_it
        }
    };
    let local_of = |arg: u32| {
        if arg_is_factor(arg) {
            api::COMBINE_LOCAL_CONSTANT
        } else {
            it
        }
    };
    // The function, its factor, the local and the other color.
    let pick = |op: u32, a1: u32, a2: u32| -> (u32, u32, u32, u32) {
        use api::*;
        let scale = COMBINE_FUNCTION_SCALE_OTHER;
        let any_texture = arg_is_texture(a1) || arg_is_texture(a2);
        if matches!(op, TOP_SELECTARG1 | TOP_SELECTARG2) {
            let arg = if op == TOP_SELECTARG1 { a1 } else { a2 };
            return (scale, COMBINE_FACTOR_ONE, it, other_of(arg));
        }
        if !any_texture {
            if (arg_is_factor(a1) || arg_is_factor(a2)) && matches!(op, TOP_MODULATE | TOP_ADD | 10)
            {
                // The diffuse color and the texture factor, or the factor
                // twice: the first argument is the other color and the
                // second the local one, so SUBTRACT is the first less
                // the second.
                let (f, factor) = match op {
                    TOP_ADD => (COMBINE_FUNCTION_SCALE_OTHER_ADD_LOCAL, COMBINE_FACTOR_ONE),
                    10 => (COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL, COMBINE_FACTOR_ONE),
                    _ => (scale, COMBINE_FACTOR_LOCAL),
                };
                return (f, factor, local_of(a2), other_of(a1));
            }
            // The diffuse color, which is the current one here too.
            return (scale, COMBINE_FACTOR_ONE, it, other_it);
        }
        // The argument that isn't the texture is the local color.
        let rest = if arg_is_texture(a1) { a2 } else { a1 };
        match op {
            TOP_ADD => (
                COMBINE_FUNCTION_SCALE_OTHER_ADD_LOCAL,
                COMBINE_FACTOR_ONE,
                local_of(rest),
                other_tex,
            ),
            // SUBTRACT: the texture less the diffuse color.
            10 => (
                COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL,
                COMBINE_FACTOR_ONE,
                local_of(rest),
                other_tex,
            ),
            // BLENDDIFFUSEALPHA.
            12 => (
                COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL_ADD_LOCAL,
                COMBINE_FACTOR_LOCAL_ALPHA,
                local_of(rest),
                other_tex,
            ),
            TOP_BLENDTEXTUREALPHA => (
                COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL_ADD_LOCAL,
                COMBINE_FACTOR_TEXTURE_ALPHA,
                local_of(rest),
                other_tex,
            ),
            // MODULATE, MODULATE2X and 4X, and the rest.
            _ => (scale, COMBINE_FACTOR_LOCAL, local_of(rest), other_tex),
        }
    };
    let (a1, a2) = (d.stage(0, TSS_COLORARG1), d.stage(0, TSS_COLORARG2));
    let alpha_op = if diffuse_alpha {
        TOP_SELECTARG2
    } else {
        d.stage(0, TSS_ALPHAOP)
    };
    let (aa1, aa2) = (d.stage(0, TSS_ALPHAARG1), d.stage(0, TSS_ALPHAARG2));
    if [a1, a2, aa1, aa2].into_iter().any(arg_is_factor) {
        api::constant_color_value(cpu, d.state(RS_TEXTUREFACTOR));
    }
    let (f, factor, local, other) = pick(d.stage(0, TSS_COLOROP), a1, a2);
    api::color_combine(cpu, f, factor, local, other, false);
    let (f, factor, local, other) = pick(alpha_op, aa1, aa2);
    // The alpha unit's factor is alpha already.
    let factor = match factor {
        api::COMBINE_FACTOR_LOCAL => api::COMBINE_FACTOR_LOCAL_ALPHA,
        f => f,
    };
    // Direct3D leaves a keyed texel out whatever the stage does with
    // alpha. When the alpha isn't the texture's scaled, it becomes the
    // texture's scaled by a factor, which is 0 for a keyed texel and,
    // as the texels the alpha test keeps have alpha 1, the stage's
    // alpha for the others: 1 for ADD and BLENDTEXTUREALPHA of the
    // texture, one less the local alpha for SUBTRACT, the selected
    // alpha for SELECTARG (the diffuse alpha of MODULATE with a texture
    // without alpha, DECALALPHA, ADD before Direct3D 7), and the local
    // alpha for the rest. The card's alpha test compares the alpha unit's
    // other alpha, the texture's then, so a test the program sets on the
    // stage's alpha sees the texture's instead.
    let scales_texture =
        f == api::COMBINE_FUNCTION_SCALE_OTHER && other == api::COMBINE_OTHER_TEXTURE;
    let (f, factor, local, other) = if keyed && !scales_texture {
        use api::*;
        let (factor, local) = match (f, factor, other) {
            (COMBINE_FUNCTION_SCALE_OTHER_ADD_LOCAL, _, COMBINE_OTHER_TEXTURE)
            | (COMBINE_FUNCTION_BLEND, COMBINE_FACTOR_TEXTURE_ALPHA, COMBINE_OTHER_TEXTURE) => {
                (COMBINE_FACTOR_ONE, local)
            }
            (COMBINE_FUNCTION_SCALE_OTHER_MINUS_LOCAL, _, COMBINE_OTHER_TEXTURE) => {
                (COMBINE_FACTOR_ONE_MINUS_LOCAL_ALPHA, local)
            }
            (COMBINE_FUNCTION_SCALE_OTHER, COMBINE_FACTOR_ONE, COMBINE_OTHER_CONSTANT) => {
                (COMBINE_FACTOR_LOCAL_ALPHA, COMBINE_LOCAL_CONSTANT)
            }
            _ => (COMBINE_FACTOR_LOCAL_ALPHA, local),
        };
        (
            COMBINE_FUNCTION_SCALE_OTHER,
            factor,
            local,
            COMBINE_OTHER_TEXTURE,
        )
    } else {
        (f, factor, local, other)
    };
    api::alpha_combine(cpu, f, factor, local, other, false);
}

/// Whether the card can do a stage's operation, for ValidateDevice.
pub fn can_do(op: u32) -> bool {
    matches!(op, 1..=7 | 10 | 12 | 13)
}

/// The W of fog table entry `i`, as Glide's utility library has it.
fn fog_table_index_to_w(i: i32) -> f32 {
    let e = 3.0 + (i >> 2) as f64;
    (2f64.powf(e) as f32) / (8 - (i & 3)) as f32
}

// Textures.

/// Load stage `stage`'s texture, the surface `id`, into the heap and
/// map it on texture unit `tmu`, with the stage's filtering and
/// addressing: the factors of its coordinates, and whether it is keyed.
fn load(
    cpu: &mut Cpu,
    d: &Device,
    stage: u32,
    id: u32,
    tmu: u32,
    key: bool,
) -> Option<([f32; 2], bool)> {
    // A texture the device drew into is read from its surface.
    super::shadow::expose(cpu, id);
    let (start, info, keyed) = match in_place(cpu, id, key) {
        Some(p) => {
            if p.info.format == api::TEXFMT_P_8 {
                let pal = palette(cpu, id);
                let hash = hash_words(&pal);
                if hal(cpu).palettes[tmu as usize] != hash {
                    api::tex_download_palette(cpu, tmu, &pal, 0, 255);
                    hal(cpu).palettes[tmu as usize] = hash;
                }
            }
            if let Some(color) = p.key {
                api::chromakey_value(cpu, color);
                api::chromakey_mode(cpu, api::CHROMAKEY_ENABLE);
            }
            (p.start, p.info, false)
        }
        None => {
            let entry = cached(cpu, id, key, tmu)?;
            let info = TexInfo {
                small_lod: entry.info[0],
                large_lod: entry.info[1],
                aspect: entry.info[2],
                format: entry.info[3],
            };
            (entry.start, info, entry.keyed)
        }
    };
    let mip = d.stage(stage, TSS_MIPFILTER) > 1 && info.small_lod > info.large_lod;
    api::tex_mipmap_mode(
        cpu,
        tmu,
        if mip {
            api::MIPMAP_NEAREST
        } else {
            api::MIPMAP_DISABLE
        },
        false,
    );
    api::tex_source(cpu, tmu, start, api::MIPMAPLEVELMASK_BOTH, &info);
    let filter = |v: u32| {
        if v >= 2 {
            api::TEXTUREFILTER_BILINEAR
        } else {
            api::TEXTUREFILTER_POINT_SAMPLED
        }
    };
    api::tex_filter_mode(
        cpu,
        tmu,
        filter(d.stage(stage, TSS_MINFILTER)),
        filter(d.stage(stage, TSS_MAGFILTER)),
    );
    let clamp = |v: u32| {
        if v == 3 || v == 4 {
            api::TEXTURECLAMP_CLAMP
        } else {
            api::TEXTURECLAMP_WRAP
        }
    };
    api::tex_clamp_mode(
        cpu,
        tmu,
        clamp(d.stage(stage, TSS_ADDRESSU)),
        clamp(d.stage(stage, TSS_ADDRESSV)),
    );
    if info.format == api::TEXFMT_P_8 {
        let pal = palette(cpu, id);
        let hash = hash_words(&pal);
        if hal(cpu).palettes[tmu as usize] != hash {
            api::tex_download_palette(cpu, tmu, &pal, 0, 255);
            hal(cpu).palettes[tmu as usize] = hash;
        }
    }
    let (w, h) = api::level_size(info.aspect, 0);
    Some(([w as f32, h as f32], keyed))
}

/// A texture surface in the card's memory, sampled where it is: the
/// address of its top level, and the color its color key is, for the
/// card's chroma key.
struct InPlace {
    info: TexInfo,
    start: u32,
    key: Option<u32>,
}

/// The surface `id` as a texture where it is, as 3dfx's driver drew
/// textures in video memory: on a Banshee or Voodoo3, whose texture
/// units read all of its memory, for a size and format the card
/// samples, rows without padding. Its mipmap levels are the ones
/// DirectDraw put where the card looks for them, each right after the
/// one before (a Banshee has one base address for all of them).
fn in_place(cpu: &mut Cpu, id: u32, key: bool) -> Option<InPlace> {
    if !hw::banshee(cpu) {
        return None;
    }
    let (_, pitch) = surface::card_memory(cpu, id)?;
    let s = state(cpu).surface(id)?.clone();
    let (w, h) = (s.width, s.height);
    if !w.is_power_of_two() || !h.is_power_of_two() || w.max(h) > 256 || w.max(h) < 2 {
        return None;
    }
    if w > 8 * h || h > 8 * w {
        return None;
    }
    let f = &s.format;
    let paletted = f.flags & DDPF_PALETTEINDEXED8 != 0;
    let format = match (paletted, f.bits, f.r, f.a) {
        (true, 8, _, _) => api::TEXFMT_P_8,
        (false, 16, 0xF800, 0) => api::TEXFMT_RGB_565,
        (false, 16, 0x0F00, 0xF000) => api::TEXFMT_ARGB_4444,
        (false, 16, 0x7C00, _) => api::TEXFMT_ARGB_1555,
        _ => return None,
    };
    let bpp = f.bits / 8;
    if pitch != w * bpp {
        return None;
    }
    let large_lod = 8 - w.max(h).trailing_zeros();
    let aspect = if w >= h {
        3 - (w / h).trailing_zeros()
    } else {
        3 + (h / w).trailing_zeros()
    };
    let start = surface::card_memory(cpu, id)?.0;
    let mut small_lod = large_lod;
    let mut next = start;
    let (mut level, mut lod) = (id, large_lod);
    while let Some((addr, p)) = surface::card_memory(cpu, level) {
        let (lw, lh) = api::level_size(aspect, lod);
        let fits = state(cpu)
            .surface(level)
            .is_some_and(|l| (l.width, l.height) == (lw, lh));
        if !fits || p != lw * bpp || addr != next {
            break;
        }
        // The card's level sizes, at least 4 texels.
        next = addr + (lw * lh).max(4) * bpp;
        small_lod = lod;
        if lod == api::LOD_1 {
            break;
        }
        match super::texture::next_level(cpu, level) {
            Some(n) => (level, lod) = (n, lod + 1),
            None => break,
        }
    }
    let keyed = key && s.key_set & 1 << KEY_SRC_BLT != 0;
    let key = keyed.then(|| {
        let raw = s.keys[KEY_SRC_BLT].0;
        if paletted {
            palette(cpu, id)[(raw & 0xFF) as usize]
        } else {
            expanded(raw, &s)
        }
    });
    Some(InPlace {
        info: TexInfo {
            small_lod,
            large_lod,
            aspect,
            format,
        },
        start,

        key,
    })
}

/// A 16-bit texel as the texture unit makes it 8 bits a channel: each
/// channel's bits again below them. 0RGB.
fn expanded(raw: u32, s: &Surface) -> u32 {
    let channel = |mask: u32| {
        // A mask past the texel's 16 bits is the program's mistake.
        let mask = mask & 0xFFFF;
        if mask == 0 {
            return 0;
        }
        let bits = mask.count_ones();
        let v = (raw & mask) >> mask.trailing_zeros();
        let mut out = 0;
        let mut have = 0;
        while have < 8 {
            out = out << bits | v;
            have += bits;
        }
        out >> (have - 8)
    };
    let f = &s.format;
    channel(f.r) << 16 | channel(f.g) << 8 | channel(f.b)
}

fn hash_words(words: &[u32]) -> u32 {
    let mut h = 0x811C_9DC5u32;
    for w in words {
        h = (h ^ w).wrapping_mul(0x0100_0193);
    }
    h | 1
}

/// The surface `id`'s palette, 0RGB each, or black.
fn palette(cpu: &mut Cpu, id: u32) -> [u32; 256] {
    let mut out = [0; 256];
    let pal = state(cpu).surface(id).map_or(0, |s| s.palette);
    let Some((_, pid)) = (pal != 0).then(|| super::super::this(cpu, pal)).flatten() else {
        return out;
    };
    let Some(p) = state(cpu).object(pid).and_then(|o| o.palette.as_ref()) else {
        return out;
    };
    for (o, [r, g, b, _]) in out.iter_mut().zip(p.entries.as_chunks::<4>().0) {
        *o = (*r as u32) << 16 | (*g as u32) << 8 | *b as u32;
    }
    out
}

/// The heap's copy of the surface `id`, made if it has none that is
/// up to date.
fn cached(cpu: &mut Cpu, id: u32, key: bool, tmu: u32) -> Option<Cached> {
    let s = state(cpu).surface(id)?.clone();
    let keyed = key && s.key_set & 1 << KEY_SRC_BLT != 0;
    let paletted = s.format.flags & DDPF_PALETTEINDEXED8 != 0;
    let variant = if paletted {
        hash_words(&palette(cpu, id)) ^ keyed as u32
    } else {
        keyed as u32
    };
    let (unit, room) = if hw::banshee(cpu) {
        if !heap(cpu) {
            return None;
        }
        let h = hal(cpu);
        (0, (h.heap, h.heap_len))
    } else {
        (tmu, (0, hw::texture_memory(cpu, tmu).1))
    };
    let clock = hal(cpu).clock;
    if let Some(c) = hal(cpu).textures.iter_mut().find(|c| {
        c.surface == id && c.tmu == unit && c.uniqueness == s.uniqueness && c.variant == variant
    }) {
        c.used = clock;
        return Some(c.clone());
    }
    hal(cpu)
        .textures
        .retain(|c| c.surface != id || c.tmu != unit);
    let image = image(cpu, id, keyed)?;
    let size = (image.data.len() as u32).next_multiple_of(16);
    let start = place(cpu, unit, room, size)?;
    api::tex_download_mipmap(
        cpu,
        unit,
        start,
        api::MIPMAPLEVELMASK_BOTH,
        &image.info,
        &image.data,
    );
    let c = Cached {
        surface: id,
        tmu: unit,
        uniqueness: s.uniqueness,
        variant,
        start,
        size,
        info: [
            image.info.small_lod,
            image.info.large_lod,
            image.info.aspect,
            image.info.format,
        ],
        keyed,
        used: clock,
    };
    hal(cpu).textures.push(c.clone());
    Some(c)
}

/// The heap there, taken from video memory if it isn't (or video
/// memory was lost since).
fn heap(cpu: &mut Cpu) -> bool {
    let h = hal(cpu).clone();
    let pid = current_pid(cpu);
    if h.heap_len != 0 {
        let there = state(cpu)
            .display
            .blocks
            .iter()
            .any(|&(at, len)| at == h.heap && len >= h.heap_len);
        if there && h.pid == pid {
            return true;
        }
        if there {
            display::free(cpu, h.heap);
        }
    }
    let (_, free) = display::video_memory(cpu);
    let len = (free / 2).min(HEAP_MAX) & !0xFFFF;
    let h = hal(cpu);
    h.textures.clear();
    h.heap_len = 0;
    h.palettes = [0; 2];
    if len < HEAP_MIN {
        return false;
    }
    let Ok(at) = display::alloc(cpu, len) else {
        return false;
    };
    let h = hal(cpu);
    h.heap = at;
    h.heap_len = len;
    h.pid = pid;
    true
}

/// Room for `size` bytes in texture unit `unit`'s heap, `room` (its
/// start and length), the least recently used textures there going
/// until there is: its card address.
fn place(cpu: &mut Cpu, unit: u32, room: (u32, u32), size: u32) -> Option<u32> {
    let h = hal(cpu);
    let (heap, heap_len) = room;
    if size > heap_len {
        return None;
    }
    loop {
        let mut used: Vec<(u32, u32)> = h
            .textures
            .iter()
            .filter(|c| c.tmu == unit)
            .map(|c| (c.start, c.size))
            .collect();
        used.sort();
        let mut at = heap;
        for (start, len) in used {
            if start >= at + size {
                break;
            }
            at = at.max(start + len);
        }
        if at + size <= heap + heap_len {
            return Some(at);
        }
        let oldest = h
            .textures
            .iter()
            .enumerate()
            .filter(|(_, c)| c.tmu == unit)
            .min_by_key(|(_, c)| c.used)
            .map(|(i, _)| i)?;
        h.textures.remove(oldest);
    }
}

/// A device is made: on a Banshee or Voodoo3 the heap is taken now,
/// before the program fills video memory with its textures, as Unreal
/// does until there is no more room. Taken at the first draw, there is
/// none left for it then.
pub fn created(cpu: &mut Cpu) {
    if hw::banshee(cpu) {
        heap(cpu);
    }
}

/// The device `gone` goes: if it was the last, the heap goes back to
/// video memory.
pub fn release(cpu: &mut Cpu, gone: u32) {
    let devices = state(cpu).ids(|o| o.d3d.as_ref().is_some_and(|d| d.device.is_some()));
    if devices.iter().any(|&id| id != gone) {
        return;
    }
    super::shadow::forget_all(cpu);
    let h = hal(cpu).clone();
    if h.heap_len != 0 && h.pid == current_pid(cpu) {
        display::free(cpu, h.heap);
    }
    *hal(cpu) = Hal::default();
    if hw::banshee(cpu) && crate::win32::glide::state(cpu).owner == 0 {
        api::set_render_target(cpu, 0, 0, 0, 0);
    }
}

/// A process ended: the heap it had is gone with its video memory.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    // The shadows of its surfaces go even when it never had the heap,
    // as when video memory was too full for one.
    let gone: Vec<u32> = {
        let s = state(cpu);
        s.hal
            .shadows
            .iter()
            .filter(|sh| s.object(sh.surface).is_none_or(|o| o.pid == pid))
            .map(|sh| sh.at)
            .collect()
    };
    for &at in &gone {
        display::free(cpu, at);
    }
    hal(cpu).shadows.retain(|sh| !gone.contains(&sh.at));
    if hal(cpu).pid == pid {
        let h = hal(cpu).clone();
        if h.heap_len != 0 {
            display::free(cpu, h.heap);
        }
        for sh in &h.shadows {
            display::free(cpu, sh.at);
        }
        *hal(cpu) = Hal::default();
    }
}

/// A texture as the card takes it.
struct Image {
    info: TexInfo,
    data: Vec<u8>,
}

/// The largest power of two up to `n`, 256 at the most.
fn pow2(n: u32) -> u32 {
    let n = n.clamp(1, 256);
    1 << (31 - n.leading_zeros())
}

/// The surface `id` and its mipmap levels as a texture of the card.
fn image(cpu: &mut Cpu, id: u32, keyed: bool) -> Option<Image> {
    let s = state(cpu).surface(id)?.clone();
    let (mut w, mut h) = (pow2(s.width), pow2(s.height));
    // A texture unit whose largest level is the 1x1 one (LODMIN 8) is
    // off, which is how Glide turns one off, so a 1x1 texture is drawn
    // as a 2x2 one.
    if w.max(h) == 1 {
        (w, h) = (2, 2);
    }
    if w > 8 * h {
        h = w / 8;
    }
    if h > 8 * w {
        w = h / 8;
    }
    let exact = w == s.width && h == s.height;
    let mut levels = vec![id];
    if exact {
        let mut at = id;
        while let Some(next) = super::texture::next_level(cpu, at) {
            let n = levels.len() as u32;
            let fits = state(cpu)
                .surface(next)
                .is_some_and(|l| l.width == (w >> n).max(1) && l.height == (h >> n).max(1));
            if !fits || n >= 9 || (w >> n) == 0 && (h >> n) == 0 {
                break;
            }
            levels.push(next);
            at = next;
        }
    }
    let big = w.max(h);
    let large_lod = 8 - big.trailing_zeros();
    let aspect = if w >= h {
        3 - (w / h).trailing_zeros()
    } else {
        3 + (h / w).trailing_zeros()
    };
    let small_lod = (large_lod + levels.len() as u32 - 1).min(api::LOD_1);
    let levels = &levels[..(small_lod - large_lod + 1) as usize];
    let format = texture_format(&s, keyed);
    let pal = palette(cpu, id);
    let mut data = Vec::new();
    for (n, &level) in levels.iter().enumerate() {
        let l = state(cpu).surface(level)?.clone();
        let (lw, lh) = api::level_size(aspect, large_lod + n as u32);
        let src = level_texels(cpu, &l);
        for y in 0..lh {
            for x in 0..lw {
                let sx = (x * l.width / lw).min(l.width.saturating_sub(1));
                let sy = (y * l.height / lh).min(l.height.saturating_sub(1));
                let raw = src.get((sy * l.width + sx) as usize).copied().unwrap_or(0);
                let key_hit = keyed && {
                    let (low, high) = l.keys[KEY_SRC_BLT];
                    (low..=high.max(low)).contains(&raw)
                };
                encode(&mut data, format, raw, &l, &pal, key_hit);
            }
        }
    }
    Some(Image {
        info: TexInfo {
            small_lod,
            large_lod,
            aspect,
            format,
        },
        data,
    })
}

/// The format the card takes a surface's texels in.
fn texture_format(s: &Surface, keyed: bool) -> u32 {
    let f = &s.format;
    if keyed {
        return api::TEXFMT_ARGB_1555;
    }
    if f.flags & DDPF_PALETTEINDEXED8 != 0 {
        return api::TEXFMT_RGB_565;
    }
    match (f.bits, f.r, f.a) {
        (16, 0xF800, _) => api::TEXFMT_RGB_565,
        (16, 0x0F00, 0xF000) => api::TEXFMT_ARGB_4444,
        (16, 0x7C00, _) => api::TEXFMT_ARGB_1555,
        (_, _, a) if a != 0 => api::TEXFMT_ARGB_4444,
        _ => api::TEXFMT_RGB_565,
    }
}

/// A level's texels as raw values, row after row.
fn level_texels(cpu: &mut Cpu, s: &Surface) -> Vec<u32> {
    let bpp = s.bytes_per_pixel().max(1);
    let mut row = vec![0u8; (s.width * bpp) as usize];
    let mut out = Vec::with_capacity((s.width * s.height) as usize);
    for y in 0..s.height {
        surface::read_bytes(cpu, s, y * s.pitch, &mut row);
        for x in 0..s.width as usize {
            let b = &row[x * bpp as usize..(x + 1) * bpp as usize];
            let mut v = [0u8; 4];
            v[..b.len().min(4)].copy_from_slice(&b[..b.len().min(4)]);
            out.push(u32::from_le_bytes(v));
        }
    }
    out
}

/// A raw texel of `s` as ARGB, 8 bits each.
fn argb(raw: u32, s: &Surface, pal: &[u32; 256]) -> u32 {
    let f = &s.format;
    if f.flags & DDPF_PALETTEINDEXED8 != 0 {
        return 0xFF00_0000 | pal[(raw & 0xFF) as usize];
    }
    let channel = |mask: u32, default: u32| {
        if mask == 0 {
            return default;
        }
        let v = (raw & mask) >> mask.trailing_zeros();
        let max = mask >> mask.trailing_zeros();
        (v as u64 * 255 / max as u64) as u32
    };
    channel(f.a, 255) << 24 | channel(f.r, 0) << 16 | channel(f.g, 0) << 8 | channel(f.b, 0)
}

/// Put a texel in `format` at the end of `data`.
fn encode(data: &mut Vec<u8>, format: u32, raw: u32, s: &Surface, pal: &[u32; 256], keyed: bool) {
    let same = match format {
        api::TEXFMT_RGB_565 => s.format.bits == 16 && s.format.r == 0xF800,
        api::TEXFMT_ARGB_4444 => s.format.bits == 16 && s.format.a == 0xF000,
        api::TEXFMT_ARGB_1555 => s.format.bits == 16 && s.format.a == 0x8000 && !keyed,
        _ => false,
    };
    let v = if same {
        raw as u16
    } else {
        let c = argb(raw, s, pal);
        let [b, g, r, a] = c.to_le_bytes().map(u32::from);
        (match format {
            api::TEXFMT_RGB_565 => (r >> 3) << 11 | (g >> 2) << 5 | b >> 3,
            api::TEXFMT_ARGB_4444 => (a >> 4) << 12 | (r >> 4) << 8 | (g >> 4) << 4 | b >> 4,
            _ => {
                let alpha = if keyed { 0 } else { (a >= 0x80) as u32 };
                alpha << 15 | (r >> 3) << 10 | (g >> 3) << 5 | b >> 3
            }
        }) as u16
    };
    data.extend_from_slice(&v.to_le_bytes());
}

// Drawing.

/// The band on the screen the pipeline clips what the card draws to:
/// left, top, right and bottom. The card's vertex registers hold 12.4
/// fixed point, -2048 to 2047.9375, and a coordinate past either end
/// wraps round to the other, so a triangle clipped at 2048 would have
/// its vertices there drawn at -2048. A pixel in from those ends
/// leaves room for the half pixel lines are drawn out by.
pub const VERTEX_BAND: [f32; 4] = [-2047.0, -2047.0, 2047.0, 2047.0];

/// The card as the pipeline's rasterizer.
pub struct Raster<'a> {
    pub cpu: &'a mut Cpu,
    pub card: Card,
    /// The triangles of the specular pass, their colors the specular
    /// ones.
    pub second: Vec<[Vertex; 3]>,
}

impl Raster<'_> {
    fn vertex(&self, v: &ScreenVertex) -> Vertex {
        let rhw = if v.rhw.is_finite() && v.rhw > 0.0 {
            v.rhw
        } else {
            1.0
        };
        let [mut b, mut g, mut r, a] = v.diffuse.to_le_bytes().map(f32::from);
        if self.card.specular == Specular::Vertices {
            let [sb, sg, sr, _] = v.specular.to_le_bytes().map(f32::from);
            (r, g, b) = (
                (r + sr).min(255.0),
                (g + sg).min(255.0),
                (b + sb).min(255.0),
            );
        }
        // How much of the fog color: 1 less the vertex's fog factor, in
        // Z's top 8 bits or as W's integer part.
        let fog = 1.0 - (v.specular >> 24) as f32 / 255.0;
        let ooz = if self.card.fog_z {
            fog * 65535.0
        } else {
            v.z.clamp(0.0, 1.0) * 65535.0
        };
        let oow = if self.card.fog_w { fog * 255.0 } else { rhw };
        let mut tmu = [TmuVertex::default(); 2];
        for (t, slot) in tmu.iter_mut().zip(self.card.tmus) {
            if let Some((stage, [sw, sh])) = slot {
                let [u, vv] = v.tex[stage];
                *t = TmuVertex {
                    sow: u * sw * rhw,
                    tow: vv * sh * rhw,
                    oow: rhw,
                };
            } else {
                t.oow = rhw;
            }
        }
        Vertex {
            x: v.x,
            y: v.y,
            z: v.z,
            r,
            g,
            b,
            ooz,
            a,
            oow,

            tmu,
        }
    }
}

impl Raster<'_> {
    /// The card as the rasterizer of a batch, as `bind` set it up.
    pub fn new(cpu: &mut Cpu, card: Card) -> Raster<'_> {
        Raster {
            cpu,
            card,
            second: Vec::new(),
        }
    }

    /// The end of the batch: its specular pass, if it has one.
    pub fn finish(self) {
        let Specular::Pass { source, equal } = self.card.specular else {
            return;
        };
        if self.second.is_empty() {
            return;
        }
        let cpu = self.cpu;
        api::color_combine(
            cpu,
            api::COMBINE_FUNCTION_SCALE_OTHER,
            api::COMBINE_FACTOR_ONE,
            api::COMBINE_LOCAL_ITERATED,
            api::COMBINE_OTHER_ITERATED,
            false,
        );
        api::alpha_blend_function(cpu, source, api::BLEND_ONE, api::BLEND_ONE, api::BLEND_ZERO);
        api::fog_color_value(cpu, 0);
        if equal {
            api::depth_buffer_function(cpu, compare(3));
            api::depth_mask(cpu, false);
        }
        for [a, b, c] in &self.second {
            api::draw_triangle(cpu, a, b, c);
        }
    }
}

impl Rasterizer for Raster<'_> {
    fn triangle(&mut self, v: &[ScreenVertex; 3]) {
        let [a, b, c] = v.map(|v| self.vertex(&v));
        api::draw_triangle(self.cpu, &a, &b, &c);
        if matches!(self.card.specular, Specular::Pass { .. })
            && v.iter().any(|v| v.specular & 0xFF_FFFF != 0)
        {
            let lit = |mut p: Vertex, s: &ScreenVertex| {
                let [sb, sg, sr, _] = s.specular.to_le_bytes().map(f32::from);
                (p.r, p.g, p.b) = (sr, sg, sb);
                p
            };
            self.second
                .push([lit(a, &v[0]), lit(b, &v[1]), lit(c, &v[2])]);
        }
    }

    fn line(&mut self, v: &[ScreenVertex; 2]) {
        let [a, b] = v.map(|v| self.vertex(&v));
        api::draw_line(self.cpu, &a, &b);
    }

    fn point(&mut self, v: &ScreenVertex) {
        let p = self.vertex(v);
        api::draw_point(self.cpu, &p);
    }
}

// Clearing.

/// Fill `rects` of the 16-bit surface `s` (`id`) in a Banshee's or
/// Voodoo3's memory with `value` by the card's fastfill, as the Voodoo3's
/// HAL clears: the 3D engine fills it after what it drew before, and with
/// the OpenGL renderer the GPU does, where the software renderer would
/// otherwise draw all that first to have the memory to write into. A
/// depth buffer gets `value` as zaColor's depth, a render target as
/// color1, the 5-6-5 value's channels widened and not dithered, which
/// the fill takes back to `value`. False, with nothing done, while the
/// card isn't set up for the HAL's drawing (Glide closed or another
/// process's, or no render target or depth buffer of the HAL's given to
/// it yet) or the surface isn't in its memory.
fn card_fill(cpu: &mut Cpu, s: &Surface, id: u32, rects: &[[i32; 4]], value: u16) -> bool {
    use crate::voodoo::banshee::render3d::{
        AUX_BUFFER_ADDR, AUX_BUFFER_STRIDE, COL_BUFFER_ADDR, COL_BUFFER_STRIDE,
    };
    use crate::voodoo::regs::{
        CLIP_LEFT_RIGHT, CLIP_LOW_Y_HIGH_Y, COLOR1, FASTFILL_CMD, FBZ_MODE, ZA_COLOR,
    };
    const ENRECTCLIP: u32 = 1;
    const RGBWRMASK: u32 = 1 << 9;
    const ZAWRMASK: u32 = 1 << 10;
    const DRAWBUFFER: u32 = 3 << 14;
    if !hw::banshee(cpu) {
        return false;
    }
    let pid = current_pid(cpu);
    let glide = crate::win32::glide::state(cpu);
    let (owner, g) = (glide.owner, glide.gc.clone());
    if !g.active || (owner != 0 && owner != pid) {
        return false;
    }
    let depth = s.format.flags & surface::DDPF_ZBUFFER != 0;
    // The registers the fill takes the place of, put back after it.
    let (at_reg, stride_reg, value_reg, saved) = if depth {
        (AUX_BUFFER_ADDR, AUX_BUFFER_STRIDE, ZA_COLOR, (g.target[2], g.target[3], g.za_color))
    } else {
        (COL_BUFFER_ADDR, COL_BUFFER_STRIDE, COLOR1, (g.target[0], g.target[1], g.color1))
    };
    if saved.1 == 0 {
        return false;
    }
    let Some((at, pitch)) = surface::card_memory(cpu, id) else {
        return false;
    };
    let (fbz, fill) = if depth {
        (ENRECTCLIP | ZAWRMASK, g.za_color & !0xFFFF | value as u32)
    } else {
        let v = value as u32;
        let (r, gr, b) = ((v >> 11) & 0x1F, (v >> 5) & 0x3F, v & 0x1F);
        let rgb = (r << 3 | r >> 2) << 16 | (gr << 2 | gr >> 4) << 8 | (b << 3 | b >> 2);
        (ENRECTCLIP | RGBWRMASK | g.fbz_mode & DRAWBUFFER, rgb)
    };
    hw::write(cpu, hw::ALL, at_reg, at);
    hw::write(cpu, hw::ALL, stride_reg, pitch);
    hw::write(cpu, hw::ALL, FBZ_MODE, fbz);
    hw::write(cpu, hw::ALL, value_reg, fill);
    let whole = [[0, 0, s.width as i32, s.height as i32]];
    for r in if rects.is_empty() { &whole[..] } else { rects } {
        let x = |v: i32| v.clamp(0, s.width as i32) as u32;
        let y = |v: i32| v.clamp(0, s.height as i32) as u32;
        let (left, right, top, bottom) = (x(r[0]), x(r[2]), y(r[1]), y(r[3]));
        if left >= right || top >= bottom {
            continue;
        }
        hw::write(cpu, hw::ALL, CLIP_LEFT_RIGHT, left << 16 | right);
        hw::write(cpu, hw::ALL, CLIP_LOW_Y_HIGH_Y, top << 16 | bottom);
        hw::write(cpu, hw::ALL, FASTFILL_CMD, 1);
    }
    hw::write(cpu, hw::ALL, at_reg, saved.0);
    hw::write(cpu, hw::ALL, stride_reg, saved.1);
    hw::write(cpu, hw::ALL, FBZ_MODE, g.fbz_mode);
    hw::write(cpu, hw::ALL, value_reg, saved.2);
    hw::write(cpu, hw::ALL, CLIP_LEFT_RIGHT, g.clip_left_right);
    hw::write(cpu, hw::ALL, CLIP_LOW_Y_HIGH_Y, g.clip_bottom_top);
    true
}

/// Fill rectangles (left, top, right, bottom) of the device `d`'s render
/// target with `color` (ARGB) and of its depth buffer with `z`, as
/// `target` and `depth` say, or the whole of them with no rectangles.
pub fn clear(
    cpu: &mut Cpu,
    d: &Device,
    rects: &[[i32; 4]],
    target: Option<u32>,
    depth: Option<f32>,
) {
    let fill = |cpu: &mut Cpu, id: u32, value: u16| {
        let Some(s) = state(cpu).surface(id).cloned() else {
            return;
        };
        if s.format.bits != 16 {
            return;
        }
        let whole = [[0, 0, s.width as i32, s.height as i32]];
        let rects = if rects.is_empty() { &whole[..] } else { rects };
        if s.memory == MEM_CARD {
            // The add-on card's buffers, by its fastfill.
            let list: Vec<_> = rects
                .iter()
                .map(|r| crate::win32::gdi32::Rect {
                    left: r[0],
                    top: r[1],
                    right: r[2],
                    bottom: r[3],
                })
                .collect();
            voodoo::fill(cpu, &s, &list, value as u32);
            surface::changed(cpu, id);
            return;
        }
        if card_fill(cpu, &s, id, rects, value) {
            surface::changed(cpu, id);
            return;
        }

        for r in rects {
            let x0 = r[0].clamp(0, s.width as i32) as u32;
            let x1 = r[2].clamp(0, s.width as i32) as u32;
            let y0 = r[1].clamp(0, s.height as i32) as u32;
            let y1 = r[3].clamp(0, s.height as i32) as u32;
            if x1 <= x0 {
                continue;
            }
            let row: Vec<u8> = (x0..x1).flat_map(|_| value.to_le_bytes()).collect();
            for y in y0..y1 {
                surface::write_bytes(cpu, &s, y * s.pitch + x0 * 2, &row);
            }
        }
        surface::changed(cpu, id);
    };
    // A 24- or 32-bit surface takes the exact value, its shadow the
    // 16-bit one.
    let shadow = super::shadow::fill;
    if let Some(c) = target
        && let Some(s) = state(cpu).surface(d.target).cloned()
    {
        let (exact, v) = super::shadow::color_values(&s, c);
        if !shadow(cpu, d.target, rects, exact, v) {
            fill(cpu, d.target, v);
        }
    }
    if let (Some(z), Some(zid)) = (depth, super::device::depth_buffer(cpu, d))
        && let Some(s) = state(cpu).surface(zid).cloned()
    {
        let (exact, v) = super::shadow::depth_values(&s, z);
        if !shadow(cpu, zid, rects, exact, v) {
            fill(cpu, zid, v);
        }
    }
}
