//! Level 1, fixed-function 3D: the state its commands set, and those
//! commands (chapter 9 of `docs/toadforce-spec.md`).
//!
//! `decode` checks a command against everything but the state, as the
//! level 0 decoder does, and `Level1::apply` checks the rest and carries
//! it out. A command is rejected whole: one bad pair of a
//! `SET_RENDER_STATE` sets none of its states.

pub mod states;

use super::caps;
use super::format::{Format, Rect, SURFACE_WORDS, Surface};
use super::regs::Error;
use states::{RENDER_STATES, STAGE_STATES, STAGES};
use std::collections::BTreeMap;

pub const SET_RENDER_STATE: u16 = 0x0010;
pub const SET_STAGE_STATE: u16 = 0x0011;
pub const SET_TRANSFORM: u16 = 0x0012;
pub const SET_VIEWPORT: u16 = 0x0013;
pub const SET_SCISSOR: u16 = 0x0014;
pub const SET_MATERIAL: u16 = 0x0015;
pub const SET_LIGHT: u16 = 0x0016;
pub const ENABLE_LIGHT: u16 = 0x0017;
pub const SET_CLIP_PLANE: u16 = 0x0018;
pub const SET_RENDER_TARGET: u16 = 0x0020;
pub const CLEAR: u16 = 0x0021;
pub const DEFINE_TEXTURE: u16 = 0x0022;
pub const FREE_TEXTURE: u16 = 0x0023;
pub const SET_TEXTURE: u16 = 0x0024;

/// The transforms `SET_TRANSFORM` names: four world matrices (the
/// second to fourth for vertex blending), the view, the projection, and
/// one for each texture coordinate set.
pub const TRANSFORM_WORLD: u32 = 0;
pub const TRANSFORM_VIEW: u32 = 4;
pub const TRANSFORM_PROJECTION: u32 = 5;
pub const TRANSFORM_TEXTURE: u32 = 6;
pub const TRANSFORMS: usize = 14;

pub const LIGHTS: usize = caps::LIGHTS as usize;
pub const CLIP_PLANES: usize = caps::CLIP_PLANES as usize;

/// `CLEAR`'s flags.
pub const CLEAR_COLOUR: u32 = 1 << 0;
pub const CLEAR_DEPTH: u32 = 1 << 1;
pub const CLEAR_STENCIL: u32 = 1 << 2;
/// `DEFINE_TEXTURE`'s flags: bit 0, a cube map of six faces.
pub const TEXTURE_CUBE: u32 = 1 << 0;
/// `DEFINE_TEXTURE`'s flags: bit 1, the texture has a colour key.
pub const TEXTURE_KEYED: u32 = 1 << 1;
/// `DEFINE_TEXTURE`'s palette argument and `SET_RENDER_TARGET`'s depth
/// format when there is none.
pub const NONE: u32 = 0xFFFF_FFFF;

/// The kinds of light.
pub const LIGHT_POINT: u32 = 1;
pub const LIGHT_SPOT: u32 = 2;
pub const LIGHT_DIRECTIONAL: u32 = 3;

const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// The part of the render target that draws reach, and the depths they
/// are given.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Viewport {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub min_z: f32,
    pub max_z: f32,
}

crate::state_fields!(Viewport {
    x,
    y,
    width,
    height,
    min_z,
    max_z,
});

/// The colours of the surface that is lit: red, green, blue and alpha
/// of each, and the specular power.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Material {
    pub diffuse: [f32; 4],
    pub ambient: [f32; 4],
    pub specular: [f32; 4],
    pub emissive: [f32; 4],
    pub power: f32,
}

crate::state_fields!(Material {
    diffuse,
    ambient,
    specular,
    emissive,
    power,
});

/// A light, as Direct3D describes one.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Light {
    /// `LIGHT_POINT`, `LIGHT_SPOT` or `LIGHT_DIRECTIONAL`; 0 in a slot
    /// never set.
    pub kind: u32,
    pub diffuse: [f32; 4],
    pub specular: [f32; 4],
    pub ambient: [f32; 4],
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub range: f32,
    pub falloff: f32,
    pub attenuation: [f32; 3],
    pub theta: f32,
    pub phi: f32,
    /// A spot light's formula: 0 Direct3D's (two cones and a falloff),
    /// 1 OpenGL's (the cosine to the power `falloff`, cut off at `phi`).
    pub spot_model: u32,
}

crate::state_fields!(Light {
    kind,
    diffuse,
    specular,
    ambient,
    position,
    direction,
    range,
    falloff,
    attenuation,
    theta,
    phi,
    spot_model,
});

/// A depth buffer: as wide and high as the colour surface it goes with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Depth {
    pub offset: u32,
    pub pitch: u32,
    pub format: Format,
}

impl Default for Depth {
    fn default() -> Self {
        Depth {
            offset: 0,
            pitch: 0,
            format: Format::D16,
        }
    }
}

crate::state_fields!(Depth {
    offset,
    pitch,
    format,
});

/// What draws and `CLEAR` write to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Target {
    pub colour: Surface,
    pub depth: Option<Depth>,
}

crate::state_fields!(Target { colour, depth });

impl Target {
    /// The depth buffer as a surface.
    pub fn depth_surface(&self) -> Option<Surface> {
        self.depth.map(|depth| Surface {
            offset: depth.offset,
            pitch: depth.pitch,
            width: self.colour.width,
            height: self.colour.height,
            format: depth.format,
        })
    }
}

/// A texture: where its images are in card memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture {
    pub format: Format,
    /// The size of level 0. Each level after it has half the width and
    /// height of the one before, and at least 1.
    pub width: u32,
    pub height: u32,
    pub levels: u32,
    pub cube: bool,
    /// The offset of the 256 words of a `P8` texture's palette.
    pub palette: Option<u32>,
    /// The pixel value that is left out of a draw with
    /// `COLORKEYENABLE` on.
    pub key: Option<u32>,
    /// The offset and pitch of each image: level 0 first, and for a
    /// cube map the six faces of a level together.
    pub images: Vec<(u32, u32)>,
}

impl Default for Texture {
    fn default() -> Self {
        Texture {
            format: Format::P8,
            width: 0,
            height: 0,
            levels: 0,
            cube: false,
            palette: None,
            key: None,
            images: Vec::new(),
        }
    }
}

crate::state_fields!(Texture {
    format,
    width,
    height,
    levels,
    cube,
    palette,
    key,
    images,
});

impl Texture {
    pub fn faces(&self) -> u32 {
        if self.cube { 6 } else { 1 }
    }

    /// The width and height of `level`.
    pub fn level_size(&self, level: u32) -> (u32, u32) {
        ((self.width >> level).max(1), (self.height >> level).max(1))
    }

    /// The offset and pitch of `face` of `level`.
    pub fn image(&self, level: u32, face: u32) -> (u32, u32) {
        self.images[(level * self.faces() + face) as usize]
    }
}

/// A command of level 1, decoded: its arguments checked against the
/// level's limits and card memory's size, and not yet against the
/// state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command<'a> {
    /// Pairs of a state's number and its value.
    RenderState(&'a [u32]),
    StageState {
        stage: usize,
        pairs: &'a [u32],
    },
    Transform {
        which: usize,
        matrix: [f32; 16],
    },
    Viewport(Viewport),
    Scissor {
        enable: bool,
        rect: [u32; 4],
    },
    Material(Material),
    Light {
        slot: usize,
        light: Light,
    },
    EnableLight {
        slot: usize,
        on: bool,
    },
    ClipPlane {
        index: usize,
        plane: [f32; 4],
    },
    RenderTarget(Option<Target>),
    Clear {
        flags: u32,
        colour: u32,
        z: f32,
        stencil: u32,
        /// Left, top, width and height of each rectangle; none for the
        /// whole target.
        rects: &'a [u32],
    },
    DefineTexture {
        handle: u32,
        format: Format,
        width: u32,
        height: u32,
        levels: u32,
        cube: bool,
        palette: Option<u32>,
        key: Option<u32>,
        /// The offset and pitch of each image.
        images: &'a [u32],
    },
    FreeTexture(u32),
    SetTexture {
        stage: usize,
        /// 0 for none.
        handle: u32,
    },
}

/// The lengths `opcode`'s commands can have, if level 1 adds the
/// command: (the least, the words that repeat after it). A length is
/// the least plus a whole number of repeats, and exactly the least when
/// nothing repeats.
pub fn lengths(opcode: u16) -> Option<(usize, usize)> {
    Some(match opcode {
        SET_RENDER_STATE => (3, 2),
        SET_STAGE_STATE => (4, 2),
        SET_TRANSFORM => (18, 0),
        SET_VIEWPORT => (7, 0),
        SET_SCISSOR => (6, 0),
        SET_MATERIAL => (18, 0),
        SET_LIGHT => (29, 0),
        ENABLE_LIGHT => (3, 0),
        SET_CLIP_PLANE => (6, 0),
        SET_RENDER_TARGET => (9, 0),
        CLEAR => (5, 4),
        DEFINE_TEXTURE => (11, 2),
        FREE_TEXTURE => (2, 0),
        SET_TEXTURE => (3, 0),
        _ => return None,
    })
}

/// The finite numbers `words` hold.
fn floats<const N: usize>(words: &[u32]) -> Result<[f32; N], Error> {
    let mut out = [0.0; N];
    for (value, &word) in out.iter_mut().zip(words) {
        *value = f32::from_bits(word);
        if !value.is_finite() {
            return Err(Error::BadValue);
        }
    }
    Ok(out)
}

fn below(value: u32, limit: usize) -> Result<usize, Error> {
    ((value as usize) < limit)
        .then_some(value as usize)
        .ok_or(Error::BadValue)
}

fn flag(value: u32) -> Result<bool, Error> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::BadValue),
    }
}

/// Whether a rectangle's left, top, width and height keep it within the
/// largest render target, as a viewport's and a scissor's must.
fn in_target_max(rect: &[u32]) -> bool {
    let side = |from: u32, size: u32| from as u64 + size as u64 <= caps::TARGET_MAX as u64;
    side(rect[0], rect[2]) && side(rect[1], rect[3])
}

/// A texture's handle: 1 to `caps::TEXTURE_HANDLES`.
fn handle(value: u32) -> Result<u32, Error> {
    (1..=caps::TEXTURE_HANDLES)
        .contains(&value)
        .then_some(value)
        .ok_or(Error::BadValue)
}

/// The command `words` hold, header first, with an opcode and a length
/// `lengths` allows, on a card with `memory` bytes. Values are checked
/// before ranges, as at level 0.
pub fn decode(opcode: u16, words: &[u32], memory: u32) -> Result<Command<'_>, Error> {
    Ok(match opcode {
        SET_RENDER_STATE => {
            let pairs = &words[1..];
            for pair in pairs.as_chunks::<2>().0 {
                let def = states::render(pair[0]).ok_or(Error::BadValue)?;
                if !def.kind.takes(pair[1]) {
                    return Err(Error::BadValue);
                }
            }
            Command::RenderState(pairs)
        }
        SET_STAGE_STATE => {
            let stage = below(words[1], STAGES)?;
            let pairs = &words[2..];
            for pair in pairs.as_chunks::<2>().0 {
                let def = states::stage(pair[0]).ok_or(Error::BadValue)?;
                if !def.kind.takes(pair[1]) {
                    return Err(Error::BadValue);
                }
            }
            Command::StageState { stage, pairs }
        }
        SET_TRANSFORM => Command::Transform {
            which: below(words[1], TRANSFORMS)?,
            matrix: floats(&words[2..])?,
        },
        SET_VIEWPORT => {
            let [x, y, width, height] = words[1..5] else {
                unreachable!()
            };
            let [min_z, max_z] = floats(&words[5..])?;
            let depths = 0.0 <= min_z && min_z <= max_z && max_z <= 1.0;
            if !in_target_max(&words[1..5]) || !depths {
                return Err(Error::BadValue);
            }
            Command::Viewport(Viewport {
                x,
                y,
                width,
                height,
                min_z,
                max_z,
            })
        }
        SET_SCISSOR => {
            let enable = flag(words[1])?;
            if !in_target_max(&words[2..6]) {
                return Err(Error::BadValue);
            }
            Command::Scissor {
                enable,
                rect: [words[2], words[3], words[4], words[5]],
            }
        }
        SET_MATERIAL => {
            let v: [f32; 17] = floats(&words[1..])?;
            let four = |at: usize| [v[at], v[at + 1], v[at + 2], v[at + 3]];
            Command::Material(Material {
                diffuse: four(0),
                ambient: four(4),
                specular: four(8),
                emissive: four(12),
                power: v[16],
            })
        }
        SET_LIGHT => {
            let slot = below(words[1], LIGHTS)?;
            let kind = words[2];
            let v: [f32; 25] = floats(&words[3..28])?;
            let spot_model = words[28];
            if !(LIGHT_POINT..=LIGHT_DIRECTIONAL).contains(&kind) || spot_model > 1 {
                return Err(Error::BadValue);
            }
            let four = |at: usize| [v[at], v[at + 1], v[at + 2], v[at + 3]];
            let three = |at: usize| [v[at], v[at + 1], v[at + 2]];
            Command::Light {
                slot,
                light: Light {
                    kind,
                    diffuse: four(0),
                    specular: four(4),
                    ambient: four(8),
                    position: three(12),
                    direction: three(15),
                    range: v[18],
                    falloff: v[19],
                    attenuation: three(20),
                    theta: v[23],
                    phi: v[24],
                    spot_model,
                },
            }
        }
        ENABLE_LIGHT => Command::EnableLight {
            slot: below(words[1], LIGHTS)?,
            on: flag(words[2])?,
        },
        SET_CLIP_PLANE => Command::ClipPlane {
            index: below(words[1], CLIP_PLANES)?,
            plane: floats(&words[2..])?,
        },
        SET_RENDER_TARGET => {
            // A colour surface with no format takes the target away.
            if words[1 + SURFACE_WORDS - 1] == NONE {
                return Ok(Command::RenderTarget(None));
            }
            let colour = Surface::values(&words[1..], caps::TARGET_FORMATS)?;
            if colour.width > caps::TARGET_MAX || colour.height > caps::TARGET_MAX {
                return Err(Error::BadValue);
            }
            let depth = match words[7] {
                NONE => None,
                code => {
                    let format = Format::from_code(code)
                        .filter(|f| f.in_mask(caps::DEPTH_FORMATS))
                        .ok_or(Error::BadValue)?;
                    let pitch = words[8];
                    if (pitch as u64) < format.line_bytes(colour.width) {
                        return Err(Error::BadValue);
                    }
                    Some(Depth {
                        offset: words[6],
                        pitch,
                        format,
                    })
                }
            };
            let target = Target { colour, depth };
            colour.in_memory(memory)?;
            if let Some(depth) = target.depth_surface() {
                depth.in_memory(memory)?;
            }
            Command::RenderTarget(Some(target))
        }
        CLEAR => {
            let flags = words[1];
            let [z] = floats(&words[3..4])?;
            let stencil = words[4];
            let all = CLEAR_COLOUR | CLEAR_DEPTH | CLEAR_STENCIL;
            if flags == 0 || flags & !all != 0 || !(0.0..=1.0).contains(&z) || stencil > 0xFF {
                return Err(Error::BadValue);
            }
            // A rectangle with no pixel is a bad range on any target.
            let rects = &words[5..];
            if rects
                .as_chunks::<4>()
                .0
                .iter()
                .any(|r| r[2] == 0 || r[3] == 0)
            {
                return Err(Error::BadRange);
            }
            Command::Clear {
                flags,
                colour: words[2],
                z,
                stencil,
                rects,
            }
        }
        DEFINE_TEXTURE => {
            let handle = handle(words[1])?;
            let format = Format::from_code(words[2])
                .filter(|f| f.in_mask(caps::TEXTURE_FORMATS))
                .ok_or(Error::BadValue)?;
            let (width, height, levels, flags) = (words[3], words[4], words[5], words[6]);
            let side = |n: u32| n.is_power_of_two() && n <= caps::TEXTURE_MAX;
            if !side(width) || !side(height) || flags & !(TEXTURE_CUBE | TEXTURE_KEYED) != 0 {
                return Err(Error::BadValue);
            }
            let cube = flags & TEXTURE_CUBE != 0;
            // A key is a pixel value, which a block has none of.
            let key = match flags & TEXTURE_KEYED != 0 {
                true if format.block_bytes().is_some() => return Err(Error::BadValue),
                true if words[8] & !format.defined_bits() != 0 => return Err(Error::BadValue),
                true => Some(words[8]),
                false => None,
            };
            let most = width.max(height).ilog2() + 1;
            let images = &words[9..];
            let faces = if cube { 6 } else { 1 };
            if !(1..=most).contains(&levels)
                || images.len() as u32 != 2 * levels * faces
                || cube && width != height
            {
                return Err(Error::BadValue);
            }
            // A palette goes with `P8` and with nothing else.
            let palette = match (words[7], format == Format::P8) {
                (NONE, false) => None,
                (offset, true) if offset != NONE => Some(offset),
                _ => return Err(Error::BadValue),
            };
            let size = |level: u32| ((width >> level).max(1), (height >> level).max(1));
            for (i, image) in images.as_chunks::<2>().0.iter().enumerate() {
                let (w, _) = size(i as u32 / faces);
                if (image[1] as u64) < format.line_bytes(w) {
                    return Err(Error::BadValue);
                }
            }
            for (i, image) in images.as_chunks::<2>().0.iter().enumerate() {
                let (w, h) = size(i as u32 / faces);
                let end = image[0] as u64 + format.image_bytes(w, h, image[1]);
                if end > memory as u64 {
                    return Err(Error::BadRange);
                }
            }
            if palette.is_some_and(|offset| offset as u64 + 1024 > memory as u64) {
                return Err(Error::BadRange);
            }
            Command::DefineTexture {
                handle,
                format,
                width,
                height,
                levels,
                cube,
                palette,
                key,
                images,
            }
        }
        FREE_TEXTURE => Command::FreeTexture(handle(words[1])?),
        SET_TEXTURE => Command::SetTexture {
            stage: below(words[1], STAGES)?,
            handle: match words[2] {
                0 => 0,
                value => handle(value)?,
            },
        },
        _ => unreachable!("an opcode with lengths has a decoder"),
    })
}

/// The state the commands of level 1 set.
#[derive(Clone, Debug, PartialEq)]
pub struct Level1 {
    /// The render states, by number. Numbers no state has stay 0.
    pub render: [u32; RENDER_STATES],
    /// The stage states of each stage, by number.
    pub stages: [[u32; STAGE_STATES]; STAGES],
    pub transforms: [[f32; 16]; TRANSFORMS],
    pub viewport: Viewport,
    pub scissor_on: bool,
    /// Left, top, width and height.
    pub scissor: [u32; 4],
    pub material: Material,
    pub lights: [Light; LIGHTS],
    /// Bit n: light n is on.
    pub lights_on: u32,
    pub clip_planes: [[f32; 4]; CLIP_PLANES],
    pub target: Option<Target>,
    /// The textures defined, by handle.
    pub textures: BTreeMap<u32, Texture>,
    /// The handle of each stage's texture, 0 for none.
    pub bound: [u32; STAGES],
}

crate::state_fields!(Level1 {
    render,
    stages,
    transforms,
    viewport,
    scissor_on,
    scissor,
    material,
    lights,
    lights_on,
    clip_planes,
    target,
    textures,
    bound,
});

impl Default for Level1 {
    /// The state after a reset, and after a write to `LEVEL`.
    fn default() -> Self {
        let mut render = [0; RENDER_STATES];
        for def in states::RENDER {
            render[def.id as usize] = def.reset;
        }
        let mut stages = [[0; STAGE_STATES]; STAGES];
        for (stage, values) in stages.iter_mut().enumerate() {
            for def in states::STAGE {
                values[def.id as usize] = states::stage_reset(def, stage);
            }
        }
        Level1 {
            render,
            stages,
            transforms: [IDENTITY; TRANSFORMS],
            viewport: Viewport::default(),
            scissor_on: false,
            scissor: [0; 4],
            material: Material::default(),
            lights: [Light::default(); LIGHTS],
            lights_on: 0,
            clip_planes: [[0.0; 4]; CLIP_PLANES],
            target: None,
            textures: BTreeMap::new(),
            bound: [0; STAGES],
        }
    }
}

impl Level1 {
    /// A render state's value.
    pub fn state(&self, id: u32) -> u32 {
        self.render[id as usize]
    }

    /// A render state's value, of a state that holds a number.
    pub fn state_f(&self, id: u32) -> f32 {
        f32::from_bits(self.render[id as usize])
    }

    /// A stage state's value.
    pub fn stage(&self, stage: usize, id: u32) -> u32 {
        self.stages[stage][id as usize]
    }

    /// The texture of `stage`, if it has one.
    pub fn texture(&self, stage: usize) -> Option<&Texture> {
        self.textures.get(&self.bound[stage])
    }

    /// Carry out `command` in `memory`, card memory. The spans of card
    /// memory it wrote go to `drawn`.
    pub fn apply(
        &mut self,
        command: Command,
        memory: &mut [u8],
        drawn: &mut Vec<(u32, u32)>,
    ) -> Result<(), Error> {
        match command {
            Command::RenderState(pairs) => {
                for pair in pairs.as_chunks::<2>().0 {
                    self.render[pair[0] as usize] = pair[1];
                }
            }
            Command::StageState { stage, pairs } => {
                for pair in pairs.as_chunks::<2>().0 {
                    self.stages[stage][pair[0] as usize] = pair[1];
                }
            }
            Command::Transform { which, matrix } => self.transforms[which] = matrix,
            Command::Viewport(viewport) => self.viewport = viewport,
            Command::Scissor { enable, rect } => {
                self.scissor_on = enable;
                self.scissor = rect;
            }
            Command::Material(material) => self.material = material,
            Command::Light { slot, light } => self.lights[slot] = light,
            Command::EnableLight { slot, on } => {
                // A light that was never set has nothing to shine with.
                if on && self.lights[slot].kind == 0 {
                    return Err(Error::BadState);
                }
                self.lights_on = self.lights_on & !(1 << slot) | (on as u32) << slot;
            }
            Command::ClipPlane { index, plane } => self.clip_planes[index] = plane,
            Command::RenderTarget(target) => self.target = target,
            Command::Clear {
                flags,
                colour,
                z,
                stencil,
                rects,
            } => self.clear(flags, colour, z, stencil, rects, memory, drawn)?,
            Command::DefineTexture {
                handle,
                format,
                width,
                height,
                levels,
                cube,
                palette,
                key,
                images,
            } => {
                let texture = Texture {
                    format,
                    width,
                    height,
                    levels,
                    cube,
                    palette,
                    key,
                    images: images
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|i| (i[0], i[1]))
                        .collect(),
                };
                self.textures.insert(handle, texture);
            }
            Command::FreeTexture(handle) => {
                self.textures.remove(&handle).ok_or(Error::BadHandle)?;
                for bound in &mut self.bound {
                    if *bound == handle {
                        *bound = 0;
                    }
                }
            }
            Command::SetTexture { stage, handle } => {
                if handle != 0 && !self.textures.contains_key(&handle) {
                    return Err(Error::BadHandle);
                }
                self.bound[stage] = handle;
            }
        }
        Ok(())
    }

    /// `CLEAR`: write the colour, the depth and the stencil value to
    /// the rectangles of the render target, each to the buffer its flag
    /// names.
    #[allow(clippy::too_many_arguments)]
    fn clear(
        &self,
        flags: u32,
        colour: u32,
        z: f32,
        stencil: u32,
        rects: &[u32],
        memory: &mut [u8],
        drawn: &mut Vec<(u32, u32)>,
    ) -> Result<(), Error> {
        // Without a target there is nothing to check the rectangles
        // against. With one, ranges come before the state, as chapter 5
        // orders them.
        let target = self.target.ok_or(Error::BadState)?;
        let whole = [0, 0, target.colour.width, target.colour.height];
        let rects = if rects.is_empty() { &whole } else { rects };
        let rects: Vec<Rect> = rects
            .as_chunks::<4>()
            .0
            .iter()
            .map(|r| Rect::parse(r))
            .collect();
        if !rects.iter().all(|rect| target.colour.holds(rect)) {
            return Err(Error::BadRange);
        }
        let depth = target.depth_surface();
        let has_stencil = depth.is_some_and(|d| d.format == Format::D24S8);
        // A buffer the target lacks can't be cleared.
        if flags & CLEAR_DEPTH != 0 && depth.is_none() || flags & CLEAR_STENCIL != 0 && !has_stencil
        {
            return Err(Error::BadState);
        }
        for rect in &rects {
            if flags & CLEAR_COLOUR != 0 {
                let value = target.colour.format.pack(colour);
                super::blit::fill(memory, &target.colour, rect, value);
                drawn.push(target.colour.span(rect));
            }
            let Some(depth) = depth.filter(|_| flags & (CLEAR_DEPTH | CLEAR_STENCIL) != 0) else {
                continue;
            };
            // The bits of a depth pixel each flag writes.
            let (value, mask) = depth_bits(depth.format, flags, z, stencil);
            if mask == depth.format.defined_bits() || mask == 0xFFFF_FFFF {
                super::blit::fill(memory, &depth, rect, value);
            } else {
                let len = rect.width as usize * 4;
                for y in rect.top..rect.top + rect.height {
                    let at = depth.at(rect.left, y);
                    for pixel in memory[at..at + len].as_chunks_mut::<4>().0 {
                        let old = u32::from_le_bytes(*pixel);
                        *pixel = (old & !mask | value & mask).to_le_bytes();
                    }
                }
            }
            drawn.push(depth.span(rect));
        }
        Ok(())
    }
}

/// A depth from 0 to 1 as a depth buffer of `format` stores it: rounded
/// to the nearest of its steps.
pub fn depth_value(format: Format, z: f32) -> u32 {
    let steps = if format == Format::D16 {
        0xFFFF
    } else {
        0xFF_FFFF
    };
    (z as f64 * steps as f64).round() as u32
}

/// What `CLEAR` writes to a pixel of a depth buffer of `format`, and
/// the bits of the pixel it writes.
fn depth_bits(format: Format, flags: u32, z: f32, stencil: u32) -> (u32, u32) {
    let depth = depth_value(format, z);
    if format == Format::D16 {
        return (depth, 0xFFFF);
    }
    let (mut value, mut mask) = (0, 0);
    if flags & CLEAR_DEPTH != 0 {
        value |= depth << 8;
        mask |= 0xFFFF_FF00;
    }
    if flags & CLEAR_STENCIL != 0 {
        value |= stencil;
        mask |= 0xFF;
    }
    // The unused byte of `D24X8` is written with the depth.
    if format == Format::D24X8 {
        mask = 0xFFFF_FFFF;
    }
    (value, mask)
}

/// The words of level 1's commands, for the emulator's own drivers and
/// the tests.
pub mod encode {
    use super::super::command::header;
    use super::*;

    pub fn render_state(out: &mut Vec<u32>, pairs: &[(u32, u32)]) {
        out.push(header(SET_RENDER_STATE, 1 + 2 * pairs.len()));
        out.extend(pairs.iter().flat_map(|&(id, value)| [id, value]));
    }

    pub fn stage_state(out: &mut Vec<u32>, stage: u32, pairs: &[(u32, u32)]) {
        out.extend([header(SET_STAGE_STATE, 2 + 2 * pairs.len()), stage]);
        out.extend(pairs.iter().flat_map(|&(id, value)| [id, value]));
    }

    pub fn transform(out: &mut Vec<u32>, which: u32, matrix: &[f32; 16]) {
        out.extend([header(SET_TRANSFORM, 18), which]);
        out.extend(matrix.iter().map(|v| v.to_bits()));
    }

    pub fn viewport(out: &mut Vec<u32>, v: &Viewport) {
        let (min_z, max_z) = (v.min_z.to_bits(), v.max_z.to_bits());
        out.extend([header(SET_VIEWPORT, 7), v.x, v.y, v.width, v.height]);
        out.extend([min_z, max_z]);
    }

    pub fn scissor(out: &mut Vec<u32>, enable: bool, rect: [u32; 4]) {
        out.extend([header(SET_SCISSOR, 6), enable as u32]);
        out.extend(rect);
    }

    pub fn material(out: &mut Vec<u32>, m: &Material) {
        out.push(header(SET_MATERIAL, 18));
        let colours = [m.diffuse, m.ambient, m.specular, m.emissive];
        out.extend(colours.iter().flatten().map(|v| v.to_bits()));
        out.push(m.power.to_bits());
    }

    pub fn light(out: &mut Vec<u32>, slot: u32, l: &Light) {
        out.extend([header(SET_LIGHT, 29), slot, l.kind]);
        let colours = [l.diffuse, l.specular, l.ambient];
        out.extend(colours.iter().flatten().map(|v| v.to_bits()));
        let vectors = [l.position, l.direction];
        out.extend(vectors.iter().flatten().map(|v| v.to_bits()));
        out.extend([l.range, l.falloff].map(f32::to_bits));
        out.extend(l.attenuation.map(f32::to_bits));
        out.extend([l.theta, l.phi].map(f32::to_bits));
        out.push(l.spot_model);
    }

    pub fn enable_light(out: &mut Vec<u32>, slot: u32, on: bool) {
        out.extend([header(ENABLE_LIGHT, 3), slot, on as u32]);
    }

    pub fn clip_plane(out: &mut Vec<u32>, index: u32, plane: &[f32; 4]) {
        out.extend([header(SET_CLIP_PLANE, 6), index]);
        out.extend(plane.map(f32::to_bits));
    }

    pub fn render_target(out: &mut Vec<u32>, target: Option<&Target>) {
        out.push(header(SET_RENDER_TARGET, 9));
        let Some(target) = target else {
            out.extend([0, 0, 0, 0, NONE, 0, NONE, 0]);
            return;
        };
        let c = &target.colour;
        out.extend([c.offset, c.pitch, c.width, c.height, c.format.code()]);
        match target.depth {
            Some(depth) => out.extend([depth.offset, depth.format.code(), depth.pitch]),
            None => out.extend([0, NONE, 0]),
        }
    }

    pub fn clear(
        out: &mut Vec<u32>,
        flags: u32,
        colour: u32,
        z: f32,
        stencil: u32,
        rects: &[Rect],
    ) {
        out.push(header(CLEAR, 5 + 4 * rects.len()));
        out.extend([flags, colour, z.to_bits(), stencil]);
        for r in rects {
            out.extend([r.left, r.top, r.width, r.height]);
        }
    }

    pub fn define_texture(out: &mut Vec<u32>, handle: u32, t: &Texture) {
        out.push(header(DEFINE_TEXTURE, 9 + 2 * t.images.len()));
        let cube = if t.cube { TEXTURE_CUBE } else { 0 };
        let keyed = if t.key.is_some() { TEXTURE_KEYED } else { 0 };
        out.extend([handle, t.format.code(), t.width, t.height, t.levels]);
        out.extend([cube | keyed, t.palette.unwrap_or(NONE), t.key.unwrap_or(0)]);
        out.extend(t.images.iter().flat_map(|&(offset, pitch)| [offset, pitch]));
    }

    pub fn free_texture(out: &mut Vec<u32>, handle: u32) {
        out.extend([header(FREE_TEXTURE, 2), handle]);
    }

    pub fn set_texture(out: &mut Vec<u32>, stage: u32, handle: u32) {
        out.extend([header(SET_TEXTURE, 3), stage, handle]);
    }
}

#[cfg(test)]
mod tests;
