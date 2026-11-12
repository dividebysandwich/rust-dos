//! The lowering: level 1's state as the short list of steps a fragment
//! goes through ("One pipeline description, two back ends" in
//! `docs/toadforce.md`).
//!
//! What a stage means is decided here, once: which stages run, what a
//! stage without a texture does, where the depth test goes. The steps
//! hold no numbers of a draw (factors, references, fog distances),
//! which are in `Values`, so two draws that differ in those have equal
//! steps. The software renderer runs the steps over the pixels of a
//! row (`span.rs`); the OpenGL renderer will print them as a shader.

use super::combine::Op;
use super::sample::{Address, Filter, Mip, Tex};
use super::texel::{self, Rgba};
use crate::toadforce::caps;
use crate::toadforce::format::Format;
use crate::toadforce::level1::states::*;
use crate::toadforce::level1::{Level1, Target, Texture};
use crate::toadforce::regs::Error;

/// Where a step reads or writes a colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Reg {
    Current,
    Temp,
    Diffuse,
    Specular,
    /// `TEXTUREFACTOR`.
    Factor,
    /// The sample of the stage being run.
    Texture,
}

pub const REGS: usize = 6;

/// An argument of a stage operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Arg {
    pub reg: Reg,
    pub complement: bool,
    pub replicate: bool,
}

impl Arg {
    pub const CURRENT: Arg = Arg::plain(Reg::Current);

    pub const fn plain(reg: Reg) -> Self {
        Arg {
            reg,
            complement: false,
            replicate: false,
        }
    }

    /// From a `COLORARG` or `ALPHAARG` value.
    fn from_state(value: u32) -> Self {
        Arg {
            reg: match value & 0xF {
                ARG_DIFFUSE => Reg::Diffuse,
                ARG_TEXTURE => Reg::Texture,
                ARG_TFACTOR => Reg::Factor,
                ARG_SPECULAR => Reg::Specular,
                ARG_TEMP => Reg::Temp,
                _ => Reg::Current,
            },
            complement: value & ARG_COMPLEMENT != 0,
            replicate: value & ARG_ALPHA_REPLICATE != 0,
        }
    }
}

/// A stencil test and what it does to the stencil value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Stencil {
    pub func: u32,
    pub fail: u32,
    pub zfail: u32,
    pub pass: u32,
}

/// Where table fog's distance comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fog {
    /// The factor is the specular colour's alpha.
    Vertex,
    /// `mode` is `FOGTABLEMODE`'s (1 exponential, 2 its square, 3
    /// linear), by depth or by w.
    Table { mode: u32, by_w: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Blend {
    pub src: u32,
    pub dst: u32,
    pub op: u32,
}

/// One thing done to the fragments of a row, in the order of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    /// The depth and stencil tests, and their writes.
    Depth {
        func: u32,
        write: bool,
        stencil: Option<Stencil>,
    },
    /// The vertices' colours into `Diffuse` and `Specular`, and
    /// `Diffuse` into `Current`.
    Colours {
        specular: bool,
    },
    /// The stage's texture at coordinate set slot `slot` into
    /// `Texture`.
    Sample {
        stage: u8,
        slot: u8,
        /// The coordinate the others are divided by.
        projected: Option<u8>,
        cube: bool,
        /// Drop the fragment when its texel is the colour key.
        keyed: bool,
        /// Move the coordinates by the last `Bump`'s offsets, and with
        /// true scale the sample by its luminance.
        bumped: Option<bool>,
    },
    /// The offsets of `BUMPENVMAP` from `Texture`, for the next
    /// `Sample`.
    Bump {
        stage: u8,
        luminance: bool,
    },
    /// A stage operation, on the colour channels, the alpha channel or
    /// both.
    Combine {
        op: Op,
        args: [Arg; 3],
        /// The argument whose alpha the operation blends by or adds.
        by: Option<Arg>,
        colour: bool,
        alpha: bool,
        scale: u8,
        to: Reg,
    },
    SpecularAdd,
    Fog(Fog),
    /// Drop the fragments whose alpha fails `func` against the
    /// reference.
    AlphaTest {
        func: u32,
    },
    /// Dither `Current` for a 5-6-5 target.
    Dither,
    /// `Current` into the target, blended with what is there.
    Write {
        blend: Option<Blend>,
        mask: u8,
    },
}

/// The numbers of a draw that the steps use.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Values {
    pub factor: Rgba,
    pub alpha_ref: u32,
    pub fog_colour: Rgba,
    pub fog_start: f32,
    pub fog_end: f32,
    pub fog_density: f32,
    /// What is taken from a fragment's depth value.
    pub z_bias: u32,
    pub stencil_ref: u32,
    pub stencil_mask: u32,
    pub stencil_write: u32,
    /// For each stage, the bump matrix (00, 01, 10, 11) and the
    /// luminance scale and offset.
    pub bump: [[f32; 6]; STAGES],
}

/// How primitives become pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    /// Pixel centres at half coordinates.
    pub half_centre: bool,
    /// `CULLMODE`: 1 none, 2 clockwise, 3 counterclockwise.
    pub cull: u32,
    /// `FILLMODE`: 1 points, 2 lines, 3 filled.
    pub fill: u32,
    pub flat: bool,
    /// The last vertex of a primitive gives its flat colour.
    pub flat_last: bool,
    pub last_pixel: bool,
    pub point_size: f32,
    pub point_min: f32,
    pub point_max: f32,
    pub point_sprite: bool,
}

/// What a slot's coordinates are: where they come from, and the
/// texture matrix they go through. Two stages with the same share a
/// slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Coord {
    /// The vertex's coordinate set.
    pub set: u8,
    /// `TEXCOORDINDEX`'s upper half: 0 for the set, or what the card
    /// makes the coordinates from.
    pub generate: u8,
    /// How many coordinates the texture matrix gives, 0 for none.
    pub count: u8,
    /// The stage whose texture matrix it is, 0 without one.
    pub stage: u8,
}

/// The largest number of coordinate sets a draw interpolates: one for
/// each texture it samples.
pub const SLOTS: usize = caps::TEXTURES_AT_ONCE as usize;

/// Everything a draw's primitives are drawn with.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawState {
    pub steps: Vec<Step>,
    pub values: Values,
    pub target: Target,
    /// The pixels a draw may write: left, top, right, bottom, the last
    /// two outside.
    pub clip: [i32; 4],
    /// Each stage's texture, where a step samples it.
    pub tex: [Option<Tex>; STAGES],
    /// The coordinate set of each slot in use, which is all a vertex
    /// in screen space gives a slot.
    pub sets: Vec<u8>,
    /// What each slot's coordinates are for a vertex the card
    /// transforms.
    pub coords: Vec<Coord>,
    pub shape: Shape,
    /// Whether `Specular` is read.
    pub specular: bool,
    /// The draw reads card memory it also writes, and so runs on one
    /// thread.
    pub serial: bool,
}

/// A state's `DrawState`, kept beside the state while it is the
/// state's. It is no part of the state: two states are equal with or
/// without it, a copy starts without, and a saved state has none.
#[derive(Default)]
pub struct Lowered(Option<std::sync::Arc<DrawState>>);

impl Lowered {
    /// The state changed.
    pub fn forget(&mut self) {
        self.0 = None;
    }

    /// The lowered form of `l`, which holds this.
    pub fn of(l: &mut Level1) -> Result<std::sync::Arc<DrawState>, Error> {
        if let Some(state) = &l.lowered.0 {
            return Ok(state.clone());
        }
        let state = std::sync::Arc::new(lower(l)?);
        l.lowered.0 = Some(state.clone());
        Ok(state)
    }
}

impl Clone for Lowered {
    fn clone(&self) -> Self {
        Lowered(None)
    }
}

impl PartialEq for Lowered {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for Lowered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "Lowered(kept)"
        } else {
            "Lowered(none)"
        })
    }
}

impl crate::savestate::State for Lowered {
    fn save(&self, _: &mut crate::savestate::Writer) {}

    fn load(&mut self, _: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        self.forget();
        Ok(())
    }
}

/// The stage's sampling states around `texture`.
fn tex(l: &Level1, stage: usize) -> Option<Tex> {
    let texture = l.texture(stage)?.clone();
    let filter = |value: u32| {
        if value >= 2 {
            Filter::Linear
        } else {
            Filter::Point
        }
    };
    let min = l.stage(stage, TSS_MINFILTER);
    Some(Tex {
        address: [
            Address::from_state(l.stage(stage, TSS_ADDRESSU)),
            Address::from_state(l.stage(stage, TSS_ADDRESSV)),
        ],
        border: texel::from_word(l.stage(stage, TSS_BORDERCOLOR)),
        mag: filter(l.stage(stage, TSS_MAGFILTER)),
        min: filter(min),
        mip: match l.stage(stage, TSS_MIPFILTER) {
            0 => Mip::None,
            1 => Mip::Point,
            _ => Mip::Linear,
        },
        lod_bias: f32::from_bits(l.stage(stage, TSS_MIPMAPLODBIAS)),
        first_level: l.stage(stage, TSS_MAXMIPLEVEL),
        anisotropy: if min == 3 {
            l.stage(stage, TSS_MAXANISOTROPY)
                .clamp(1, caps::ANISOTROPY_MAX)
        } else {
            1
        },
        key: if l.state(RS_COLORKEYENABLE) != 0 {
            texture.key
        } else {
            None
        },
        texture,
    })
}

/// One of a stage's two operations: what it computes, and from what.
#[derive(Clone, Copy, PartialEq)]
struct Half {
    op: Op,
    args: [Arg; 3],
    by: Option<Arg>,
    scale: u8,
}

impl Half {
    /// The fragment's value as it is.
    const PASS: Half = Half {
        op: Op::SelectArg1,
        args: [Arg::CURRENT; 3],
        by: None,
        scale: 1,
    };

    fn reads(&self, reg: Reg) -> bool {
        let reads = self.op.reads();
        (0..3).any(|i| reads[i] && self.args[i].reg == reg) || self.by.is_some_and(|a| a.reg == reg)
    }
}

/// The colour (`alpha` false) or alpha operation of `stage`, or None
/// where the stage leaves that half of the fragment alone.
fn half(l: &Level1, stage: usize, alpha: bool) -> Option<Half> {
    let ids = if alpha {
        [
            TSS_ALPHAOP,
            TSS_ALPHAARG1,
            TSS_ALPHAARG2,
            TSS_ALPHAARG0,
            TSS_ALPHA_SCALE,
        ]
    } else {
        [
            TSS_COLOROP,
            TSS_COLORARG1,
            TSS_COLORARG2,
            TSS_COLORARG0,
            TSS_COLOR_SCALE,
        ]
    };
    let value = l.stage(stage, ids[0]);
    let args = [1, 2, 3].map(|i| Arg::from_state(l.stage(stage, ids[i])));
    let mut half = Half {
        op: Op::from_state(value)?,
        args,
        by: match value {
            12 => Some(Arg::plain(Reg::Diffuse)),
            13 | 15 => Some(Arg::plain(Reg::Texture)),
            14 => Some(Arg::plain(Reg::Factor)),
            16 => Some(Arg::CURRENT),
            18..=21 => Some(Arg {
                replicate: false,
                ..args[0]
            }),
            _ => None,
        },
        scale: l.stage(stage, ids[4]) as u8,
    };
    if alpha && !half.op.for_alpha() {
        half.op = Op::SelectArg1;
        half.by = None;
    }
    Some(half)
}

/// Whether the ranges `a` and `b` of card memory share a byte.
fn overlap(a: (u64, u64), b: (u64, u64)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// Whether the range `r` of card memory shares a byte with what a draw
/// into `target` writes: its colour buffer or its depth buffer.
pub fn writes(target: &Target, r: (u64, u64)) -> bool {
    let colour = (target.colour.offset as u64, target.colour.end());
    overlap(r, colour)
        || target
            .depth_surface()
            .is_some_and(|d| overlap(r, (d.offset as u64, d.end())))
}

/// The bytes of card memory a draw that samples `t` reads: its
/// palette and its images.
pub fn texture_reads(t: &Texture, mut f: impl FnMut((u64, u64))) {
    if let Some(palette) = t.palette {
        f((palette as u64, palette as u64 + 1024));
    }
    for level in 0..t.levels {
        let (w, h) = t.level_size(level);
        for face in 0..t.faces() {
            let (offset, pitch) = t.image(level, face);
            f((
                offset as u64,
                offset as u64 + t.format.image_bytes(w, h, pitch),
            ));
        }
    }
}

/// The bytes of card memory a draw with `state` reads besides its
/// target, added to `reads`: its textures.
pub fn reads(state: &DrawState, reads: &mut Vec<(u32, u32)>) {
    let clamp = |at: u64| at.min(u32::MAX as u64) as u32;
    for t in state.tex.iter().flatten() {
        texture_reads(&t.texture, |r| reads.push((clamp(r.0), clamp(r.1))));
    }
}

/// Lower the state `l`. `BadState` when it has no render target, or
/// samples more textures than the card has at once.
pub fn lower(l: &Level1) -> Result<DrawState, Error> {
    let target = l.target.ok_or(Error::BadState)?;
    let mut steps = Vec::new();
    let mut texs: [Option<Tex>; STAGES] = Default::default();
    let mut coords: Vec<Coord> = Vec::new();
    let mut keyed_any = false;
    let mut bumped: Option<bool> = None;
    let mut stage_steps = Vec::new();

    // The stage's number names its states and its texture alike.
    #[allow(clippy::needless_range_loop)]
    for stage in 0..STAGES {
        let cop = l.stage(stage, TSS_COLOROP);
        if cop == OP_DISABLE {
            break;
        }
        let texture = tex(l, stage);
        let mut sample = |steps: &mut Vec<Step>, bumped: Option<bool>| -> Result<(), Error> {
            let t = texture.clone().ok_or(Error::BadState)?;
            let index = l.stage(stage, TSS_TEXCOORDINDEX);
            let flags = l.stage(stage, TSS_TEXTURETRANSFORMFLAGS);
            let count = (flags & 0xFF) as u8;
            let coord = Coord {
                set: (index & 0xFFFF) as u8,
                generate: (index >> 16) as u8,
                count,
                stage: if count > 0 { stage as u8 } else { 0 },
            };
            let slot = match coords.iter().position(|&c| c == coord) {
                Some(slot) => slot,
                None => {
                    coords.push(coord);
                    coords.len() - 1
                }
            };
            let keyed = t.key.is_some();
            keyed_any |= keyed;
            steps.push(Step::Sample {
                stage: stage as u8,
                slot: slot as u8,
                projected: (flags & TRANSFORM_PROJECTED != 0).then(|| match flags & 0xFF {
                    0 => 3,
                    count => count as u8 - 1,
                }),
                cube: t.texture.cube,
                keyed,
                bumped,
            });
            texs[stage] = Some(t);
            Ok(())
        };

        // The two bump operations move the next stage's coordinates and
        // leave the fragment alone.
        if cop == 22 || cop == 23 {
            if texture.is_some() {
                sample(&mut stage_steps, None)?;
                stage_steps.push(Step::Bump {
                    stage: stage as u8,
                    luminance: cop == 23,
                });
                bumped = Some(cop == 23);
            } else {
                bumped = None;
            }
            continue;
        }

        // An operation that reads a texture the stage hasn't got passes
        // the fragment through.
        let usable = |half: Option<Half>| match half {
            Some(h) if h.reads(Reg::Texture) && texture.is_none() => Some(Half::PASS),
            other => other,
        };
        let colour = usable(half(l, stage, false)).unwrap_or(Half::PASS);
        let alpha = usable(half(l, stage, true));
        let dot3 = colour.op == Op::DotProduct3;
        let reads_texture =
            colour.reads(Reg::Texture) || (!dot3 && alpha.is_some_and(|a| a.reads(Reg::Texture)));
        let offsets = bumped.take();
        if reads_texture {
            sample(&mut stage_steps, offsets)?;
        }
        let to = if l.stage(stage, TSS_RESULTARG) == ARG_TEMP {
            Reg::Temp
        } else {
            Reg::Current
        };
        let combine = |h: Half, colour: bool, alpha: bool| Step::Combine {
            op: h.op,
            args: h.args,
            by: h.by,
            colour,
            alpha,
            scale: h.scale,
            to,
        };
        if dot3 || alpha == Some(colour) {
            // The dot product goes to every channel, alpha too.
            stage_steps.push(combine(colour, true, true));
        } else {
            // The colour first: it may read the alpha this stage is
            // about to change.
            stage_steps.push(combine(colour, true, false));
            if let Some(alpha) = alpha {
                stage_steps.push(combine(alpha, false, true));
            }
        }
    }
    if coords.len() > SLOTS || texs.iter().flatten().count() > SLOTS {
        return Err(Error::BadState);
    }

    let on = |id: u32| l.state(id) != 0;
    let fog = on(RS_FOGENABLE).then(|| match l.state(RS_FOGTABLEMODE) {
        0 => Fog::Vertex,
        mode => Fog::Table {
            mode,
            by_w: on(RS_FOG_BY_W),
        },
    });
    let alpha_func = l.state(RS_ALPHAFUNC);
    let alpha_test = on(RS_ALPHATESTENABLE) && alpha_func != 8;
    let depth_format = target.depth.map(|d| d.format);
    let z_on = on(RS_ZENABLE) && depth_format.is_some();
    let stencil_on = on(RS_STENCILENABLE) && depth_format == Some(Format::D24S8);
    let depth = (z_on || stencil_on).then(|| Step::Depth {
        func: if z_on { l.state(RS_ZFUNC) } else { 8 },
        write: z_on && on(RS_ZWRITEENABLE),
        stencil: stencil_on.then(|| Stencil {
            func: l.state(RS_STENCILFUNC),
            fail: l.state(RS_STENCILFAIL),
            zfail: l.state(RS_STENCILZFAIL),
            pass: l.state(RS_STENCILPASS),
        }),
    });
    let mask = (l.state(RS_COLORWRITEENABLE) & 0xF) as u8;
    let kills = alpha_test || keyed_any;
    let specular_add = on(RS_SPECULARENABLE);
    let reads_specular = stage_steps.iter().any(|step| match step {
        Step::Combine { op, args, by, .. } => {
            let reads = op.reads();
            (0..3).any(|i| reads[i] && args[i].reg == Reg::Specular)
                || by.is_some_and(|a| a.reg == Reg::Specular)
        }
        _ => false,
    });
    let specular = specular_add || fog == Some(Fog::Vertex) || reads_specular;

    // Nothing drops a fragment between its depth test and its colour:
    // the test goes first, and saves the colour of what fails it.
    if !kills {
        steps.extend(depth);
    }
    if mask != 0 || kills {
        steps.push(Step::Colours { specular });
        steps.append(&mut stage_steps);
        if specular_add {
            steps.push(Step::SpecularAdd);
        }
        steps.extend(fog.map(Step::Fog));
        if alpha_test {
            steps.push(Step::AlphaTest { func: alpha_func });
        }
    } else {
        texs = Default::default();
        coords.clear();
    }
    if kills {
        steps.extend(depth);
    }
    if mask != 0 {
        if on(RS_DITHERENABLE) && target.colour.format == Format::R5G6B5 {
            steps.push(Step::Dither);
        }
        let blend = on(RS_ALPHABLENDENABLE).then(|| {
            let (src, dst) = match l.state(RS_SRCBLEND) {
                12 => (5, 6),
                13 => (6, 5),
                src => (src, l.state(RS_DESTBLEND)),
            };
            Blend {
                src,
                dst,
                op: l.state(RS_BLENDOP),
            }
        });
        steps.push(Step::Write { blend, mask });
    }

    let mut values = Values {
        factor: texel::from_word(l.state(RS_TEXTUREFACTOR)),
        alpha_ref: l.state(RS_ALPHAREF) & 0xFF,
        fog_colour: texel::from_word(l.state(RS_FOGCOLOR)),
        fog_start: l.state_f(RS_FOGSTART),
        fog_end: l.state_f(RS_FOGEND),
        fog_density: l.state_f(RS_FOGDENSITY),
        z_bias: l.state(RS_ZBIAS)
            << if depth_format == Some(Format::D16) {
                0
            } else {
                8
            },
        stencil_ref: l.state(RS_STENCILREF) & 0xFF,
        stencil_mask: l.state(RS_STENCILMASK) & 0xFF,
        stencil_write: l.state(RS_STENCILWRITEMASK) & 0xFF,
        bump: [[0.0; 6]; STAGES],
    };
    for (stage, bump) in values.bump.iter_mut().enumerate() {
        let m = TSS_BUMPENVMAT00;
        let ids = [
            m,
            m + 1,
            m + 2,
            m + 3,
            TSS_BUMPENVLSCALE,
            TSS_BUMPENVLOFFSET,
        ];
        for (value, id) in bump.iter_mut().zip(ids) {
            *value = f32::from_bits(l.stage(stage, id));
        }
    }

    // The viewport, the target and the scissor rectangle together.
    let v = &l.viewport;
    let mut clip = [
        v.x as i32,
        v.y as i32,
        (v.x + v.width).min(target.colour.width) as i32,
        (v.y + v.height).min(target.colour.height) as i32,
    ];
    if l.scissor_on {
        let [left, top, width, height] = l.scissor;
        clip[0] = clip[0].max(left.min(i32::MAX as u32) as i32);
        clip[1] = clip[1].max(top.min(i32::MAX as u32) as i32);
        clip[2] = clip[2].min((left as u64 + width as u64).min(i32::MAX as u64) as i32);
        clip[3] = clip[3].min((top as u64 + height as u64).min(i32::MAX as u64) as i32);
    }

    // What the draw writes against what it reads.
    let colour = (target.colour.offset as u64, target.colour.end());
    let mut serial = target
        .depth_surface()
        .is_some_and(|d| overlap(colour, (d.offset as u64, d.end())));
    for t in texs.iter().flatten() {
        texture_reads(&t.texture, |r| serial |= writes(&target, r));
    }

    let size = |id: u32| l.state_f(id).clamp(0.0, caps::POINT_SIZE_MAX as f32);
    Ok(DrawState {
        steps,
        values,
        target,
        clip,
        tex: texs,
        sets: coords.iter().map(|c| c.set).collect(),
        coords,
        shape: Shape {
            half_centre: on(RS_PIXEL_CENTRE),
            cull: l.state(RS_CULLMODE),
            fill: l.state(RS_FILLMODE),
            flat: l.state(RS_SHADEMODE) == 1,
            flat_last: on(RS_FLAT_VERTEX),
            last_pixel: on(RS_LASTPIXEL),
            point_size: size(RS_POINTSIZE),
            point_min: size(RS_POINTSIZE_MIN),
            point_max: size(RS_POINTSIZE_MAX),
            point_sprite: on(RS_POINTSPRITEENABLE),
        },
        specular,
        serial,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toadforce::format::Surface;
    use crate::toadforce::level1::{Depth, Texture, Viewport};

    /// A state with a 64x32 target with a `D24S8` buffer, and a
    /// viewport of all of it.
    fn state() -> Level1 {
        Level1 {
            target: Some(Target {
                colour: Surface {
                    offset: 0x1000,
                    pitch: 128,
                    width: 64,
                    height: 32,
                    format: Format::R5G6B5,
                },
                depth: Some(Depth {
                    offset: 0x4000,
                    pitch: 256,
                    format: Format::D24S8,
                }),
            }),
            viewport: Viewport {
                width: 64,
                height: 32,
                max_z: 1.0,
                ..Viewport::default()
            },
            ..Level1::default()
        }
    }

    /// Bind a 4x4 texture at `offset` to `stage`.
    fn bind(l: &mut Level1, stage: usize, offset: u32) {
        let handle = stage as u32 + 1;
        l.textures.insert(
            handle,
            Texture {
                format: Format::A8R8G8B8,
                width: 4,
                height: 4,
                levels: 1,
                images: vec![(offset, 16)],
                ..Texture::default()
            },
        );
        l.bound[stage] = handle;
    }

    const PASS: Step = Step::Combine {
        op: Op::SelectArg1,
        args: [Arg::CURRENT; 3],
        by: None,
        colour: true,
        alpha: true,
        scale: 1,
        to: Reg::Current,
    };
    const WRITE: Step = Step::Write {
        blend: None,
        mask: 0xF,
    };
    const TEXTURE: Arg = Arg::plain(Reg::Texture);
    const COLOURS: Step = Step::Colours { specular: false };

    #[test]
    fn a_draw_needs_a_render_target() {
        assert_eq!(lower(&Level1::default()), Err(Error::BadState));
    }

    #[test]
    fn a_stage_without_its_texture_passes_the_fragment_through() {
        // After a reset stage 0 modulates the texture with the diffuse
        // colour, and has no texture.
        let s = lower(&state()).unwrap();
        assert_eq!(s.steps, [COLOURS, PASS, WRITE]);
        assert_eq!(s.clip, [0, 0, 64, 32]);
        assert!(s.tex.iter().all(Option::is_none));
        assert!(!s.serial && !s.specular);
    }

    #[test]
    fn a_textured_stage_samples_and_runs_its_two_operations() {
        let mut l = state();
        bind(&mut l, 0, 0x8000);
        l.stages[0][TSS_TEXCOORDINDEX as usize] = 5;
        let s = lower(&l).unwrap();
        let sample = Step::Sample {
            stage: 0,
            slot: 0,
            projected: None,
            cube: false,
            keyed: false,
            bumped: None,
        };
        let colour = Step::Combine {
            op: Op::Modulate,
            args: [TEXTURE, Arg::CURRENT, Arg::CURRENT],
            by: None,
            colour: true,
            alpha: false,
            scale: 1,
            to: Reg::Current,
        };
        let alpha = Step::Combine {
            op: Op::SelectArg1,
            args: [TEXTURE, Arg::CURRENT, Arg::CURRENT],
            by: None,
            colour: false,
            alpha: true,
            scale: 1,
            to: Reg::Current,
        };
        assert_eq!(s.steps, [COLOURS, sample, colour, alpha, WRITE]);
        assert_eq!(s.sets, [5]);
        assert!(s.tex[0].is_some());
    }

    #[test]
    fn the_depth_test_goes_first_unless_a_fragment_can_be_dropped() {
        let mut l = state();
        l.render[RS_ZENABLE as usize] = 1;
        let depth = Step::Depth {
            func: 4,
            write: true,
            stencil: None,
        };
        assert_eq!(lower(&l).unwrap().steps, [depth, COLOURS, PASS, WRITE]);
        // With an alpha test the depth is written only for what passes
        // it.
        l.render[RS_ALPHATESTENABLE as usize] = 1;
        l.render[RS_ALPHAFUNC as usize] = 5;
        let test = Step::AlphaTest { func: 5 };
        assert_eq!(
            lower(&l).unwrap().steps,
            [COLOURS, PASS, test, depth, WRITE]
        );
        // A test that always passes is none.
        l.render[RS_ALPHAFUNC as usize] = 8;
        assert_eq!(lower(&l).unwrap().steps, [depth, COLOURS, PASS, WRITE]);
        // With no colour written, only the depth is left.
        l.render[RS_COLORWRITEENABLE as usize] = 0;
        assert_eq!(lower(&l).unwrap().steps, [depth]);
        // The stencil test rides with it, without the depth test too.
        l.render[RS_ZENABLE as usize] = 0;
        l.render[RS_STENCILENABLE as usize] = 1;
        l.render[RS_STENCILPASS as usize] = 3;
        let stencil = Step::Depth {
            func: 8,
            write: false,
            stencil: Some(Stencil {
                func: 8,
                fail: 1,
                zfail: 1,
                pass: 3,
            }),
        };
        assert_eq!(lower(&l).unwrap().steps, [stencil]);
    }

    #[test]
    fn what_follows_the_stages_comes_in_the_cards_order() {
        let mut l = state();
        for id in [
            RS_SPECULARENABLE,
            RS_FOGENABLE,
            RS_DITHERENABLE,
            RS_ALPHABLENDENABLE,
        ] {
            l.render[id as usize] = 1;
        }
        l.render[RS_SRCBLEND as usize] = 13;
        l.render[RS_COLORWRITEENABLE as usize] = 7;
        let s = lower(&l).unwrap();
        let blend = Blend {
            src: 6,
            dst: 5,
            op: 1,
        };
        assert_eq!(
            s.steps,
            [
                Step::Colours { specular: true },
                PASS,
                Step::SpecularAdd,
                Step::Fog(Fog::Vertex),
                Step::Dither,
                Step::Write {
                    blend: Some(blend),
                    mask: 7
                },
            ]
        );
        l.render[RS_FOGTABLEMODE as usize] = 3;
        l.render[RS_FOG_BY_W as usize] = 1;
        let fog = Fog::Table {
            mode: 3,
            by_w: true,
        };
        assert!(lower(&l).unwrap().steps.contains(&Step::Fog(fog)));
    }

    #[test]
    fn a_bump_stage_moves_the_next_stages_coordinates() {
        let mut l = state();
        bind(&mut l, 0, 0x8000);
        bind(&mut l, 1, 0x9000);
        l.stages[0][TSS_COLOROP as usize] = 23;
        l.stages[1][TSS_COLOROP as usize] = OP_SELECT_ARG1;
        l.stages[1][TSS_COLORARG1 as usize] = ARG_TEXTURE;
        l.stages[1][TSS_ALPHAOP as usize] = OP_SELECT_ARG1;
        l.stages[1][TSS_ALPHAARG1 as usize] = ARG_TEXTURE;
        let s = lower(&l).unwrap();
        let sample = |stage: u8, bumped| Step::Sample {
            stage,
            slot: stage,
            projected: None,
            cube: false,
            keyed: false,
            bumped,
        };
        let select = Step::Combine {
            op: Op::SelectArg1,
            args: [TEXTURE, Arg::CURRENT, Arg::CURRENT],
            by: None,
            colour: true,
            alpha: true,
            scale: 1,
            to: Reg::Current,
        };
        let bump = Step::Bump {
            stage: 0,
            luminance: true,
        };
        assert_eq!(
            s.steps,
            [
                COLOURS,
                sample(0, None),
                bump,
                sample(1, Some(true)),
                select,
                WRITE
            ]
        );
        assert_eq!(s.sets, [0, 1]);
    }

    #[test]
    fn five_textures_are_one_too_many_and_a_texture_in_the_target_is_serial() {
        let mut l = state();
        let textured = |l: &mut Level1, stage: usize, offset: u32| {
            bind(l, stage, offset);
            l.stages[stage][TSS_COLOROP as usize] = OP_MODULATE;
            l.stages[stage][TSS_COLORARG1 as usize] = ARG_TEXTURE;
        };
        for stage in 0..4 {
            textured(&mut l, stage, 0x8000 + 0x100 * stage as u32);
        }
        let s = lower(&l).unwrap();
        assert_eq!(s.tex.iter().flatten().count(), 4);
        assert!(!s.serial);
        textured(&mut l, 4, 0x9000);
        assert_eq!(lower(&l), Err(Error::BadState));
        // A texture inside the colour surface, and one inside the depth
        // buffer.
        l.stages[4][TSS_COLOROP as usize] = OP_DISABLE;
        bind(&mut l, 3, 0x1800);
        assert!(lower(&l).unwrap().serial);
        bind(&mut l, 3, 0x4010);
        assert!(lower(&l).unwrap().serial);
        bind(&mut l, 3, 0x8300);
        assert!(!lower(&l).unwrap().serial);
    }

    #[test]
    fn the_scissor_and_the_target_bound_the_viewport() {
        let mut l = state();
        l.viewport.x = 8;
        l.viewport.width = 100;
        assert_eq!(lower(&l).unwrap().clip, [8, 0, 64, 32]);
        l.scissor_on = true;
        l.scissor = [16, 4, 8, 100];
        assert_eq!(lower(&l).unwrap().clip, [16, 4, 24, 32]);
    }
}
