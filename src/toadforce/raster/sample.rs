//! Sampling: the colour a texture gives at a coordinate, read from card
//! memory when it is asked for ("Texels" in `docs/toadforce.md`).

use super::mem::Mem;
use super::texel::{self, Rgba};
use crate::toadforce::format::Format;
use crate::toadforce::level1::Texture;

/// How a coordinate outside 0 to 1 is brought to a texel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Address {
    Wrap,
    Mirror,
    Clamp,
    Border,
}

impl Address {
    /// From a stage's `ADDRESSU` or `ADDRESSV`.
    pub fn from_state(value: u32) -> Self {
        match value {
            2 => Address::Mirror,
            3 => Address::Clamp,
            4 => Address::Border,
            _ => Address::Wrap,
        }
    }

    /// The texel of `size` that texel coordinate `at` falls on, or None
    /// for the border.
    #[inline]
    fn texel(self, at: i32, size: i32) -> Option<i32> {
        match self {
            Address::Wrap => Some(at & (size - 1)),
            Address::Mirror => {
                let at = at & (2 * size - 1);
                Some(if at < size { at } else { 2 * size - 1 - at })
            }
            Address::Clamp => Some(at.clamp(0, size - 1)),
            Address::Border => (0..size).contains(&at).then_some(at),
        }
    }
}

/// How texels are combined: the nearest one, or the four around the
/// coordinate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Filter {
    Point,
    Linear,
}

/// How mip levels are used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mip {
    /// The largest level allowed only.
    None,
    /// The nearest level.
    Point,
    /// The two nearest levels, blended.
    Linear,
}

/// A stage's texture with the stage's sampling states.
#[derive(Clone, Debug, PartialEq)]
pub struct Tex {
    pub texture: Texture,
    pub address: [Address; 2],
    pub border: Rgba,
    pub mag: Filter,
    pub min: Filter,
    pub mip: Mip,
    pub lod_bias: f32,
    /// The largest level used (`MAXMIPLEVEL`): 0 for the whole chain.
    pub first_level: u32,
    /// The highest `MAXANISOTROPY` in use, or 1: with more, the level
    /// follows the smaller derivative.
    pub anisotropy: u32,
    /// A fragment whose nearest texel has this pixel value is dropped.
    pub key: Option<u32>,
}

impl Tex {
    /// `texture` sampled at its nearest texel with wrapping: the
    /// states' reset values.
    pub fn new(texture: Texture) -> Self {
        Tex {
            texture,
            address: [Address::Wrap; 2],
            border: [0; 4],
            mag: Filter::Point,
            min: Filter::Point,
            mip: Mip::None,
            lod_bias: 0.0,
            first_level: 0,
            anisotropy: 1,
            key: None,
        }
    }

    /// Whether a sample depends on how many texels a pixel covers.
    pub fn needs_lod(&self) -> bool {
        self.mag != self.min || (self.mip != Mip::None && self.texture.levels > 1)
    }
}

/// Blend `a` and `b` by `f` of 256.
#[inline]
fn lerp(a: Rgba, b: Rgba, f: u32) -> Rgba {
    let mix = |i: usize| ((a[i] as u32 * (256 - f) + b[i] as u32 * f + 128) >> 8) as u8;
    [mix(0), mix(1), mix(2), mix(3)]
}

/// The levels a sample reads: the level, the filter in it, and how much
/// of the next level is blended in, of 256.
struct Levels {
    level: u32,
    filter: Filter,
    next: u32,
}

/// A `Tex` in card memory.
#[derive(Clone, Copy)]
pub struct Sampler<'a> {
    mem: Mem<'a>,
    pub tex: &'a Tex,
}

impl<'a> Sampler<'a> {
    pub fn new(mem: Mem<'a>, tex: &'a Tex) -> Self {
        Sampler { mem, tex }
    }

    /// The pixel value at (`x`, `y`) of `face` of `level`, both inside
    /// it. The format isn't a block format.
    #[inline]
    fn pixel(&self, level: u32, face: u32, x: u32, y: u32) -> u32 {
        let t = &self.tex.texture;
        let (offset, pitch) = t.image(level, face);
        let bytes = t.format.bytes();
        let at = offset as usize + y as usize * pitch as usize + (x * bytes) as usize;
        self.mem.pixel(at, bytes)
    }

    /// The texel at (`x`, `y`) of `face` of `level`, both inside it.
    #[inline]
    pub fn fetch(&self, level: u32, face: u32, x: u32, y: u32) -> Rgba {
        let t = &self.tex.texture;
        if let Some(size) = t.format.block_bytes() {
            let (offset, pitch) = t.image(level, face);
            let at = offset as usize
                + (y / 4) as usize * pitch as usize
                + (x / 4) as usize * size as usize;
            let block = if size == 8 {
                texel::block(t.format, &self.mem.read::<8>(at))
            } else {
                texel::block(t.format, &self.mem.read::<16>(at))
            };
            return block[(y % 4 * 4 + x % 4) as usize];
        }
        let value = self.pixel(level, face, x, y);
        if t.format == Format::P8 {
            let palette = t.palette.unwrap_or(0) as usize;
            return self.mem.read(palette + value as usize * 4);
        }
        texel::decode(t.format, value)
    }

    /// The texel of `level` at texel coordinates (`x`, `y`) that may be
    /// outside it.
    #[inline]
    fn at(&self, level: u32, face: u32, x: i32, y: i32) -> Rgba {
        let (w, h) = self.tex.texture.level_size(level);
        match (
            self.tex.address[0].texel(x, w as i32),
            self.tex.address[1].texel(y, h as i32),
        ) {
            (Some(x), Some(y)) => self.fetch(level, face, x as u32, y as u32),
            _ => self.tex.border,
        }
    }

    /// (`u`, `v`) in 1/256 of a texel of `level`, `shift` of them to
    /// the left and up.
    #[inline]
    fn fixed(&self, level: u32, u: f32, v: f32, shift: f64) -> (i64, i64) {
        let (w, h) = self.tex.texture.level_size(level);
        let limit = (1i64 << 31) as f64;
        let to = |c: f32, size: u32| {
            let at = (c as f64 * size as f64 * 256.0 - shift).floor();
            // A coordinate that is none is texel 0.
            if at.is_nan() {
                0
            } else {
                at.clamp(-limit, limit) as i64
            }
        };
        (to(u, w), to(v, h))
    }

    /// One level at (`u`, `v`), in texture coordinates where 0 to 1 is
    /// the texture, through `filter`.
    fn level(&self, level: u32, face: u32, u: f32, v: f32, filter: Filter) -> Rgba {
        match filter {
            Filter::Point => {
                let (x, y) = self.fixed(level, u, v, 0.0);
                self.at(level, face, (x >> 8) as i32, (y >> 8) as i32)
            }
            Filter::Linear => {
                // A texel's centre is half a texel inside it.
                let (x, y) = self.fixed(level, u, v, 128.0);
                let (fx, fy) = ((x & 255) as u32, (y & 255) as u32);
                let (x, y) = ((x >> 8) as i32, (y >> 8) as i32);
                let (x1, y1) = (x.wrapping_add(1), y.wrapping_add(1));
                let top = lerp(self.at(level, face, x, y), self.at(level, face, x1, y), fx);
                let bottom = lerp(
                    self.at(level, face, x, y1),
                    self.at(level, face, x1, y1),
                    fx,
                );
                lerp(top, bottom, fy)
            }
        }
    }

    /// The levels a sample reads where a pixel covers 2 to the power
    /// of `lod` texels of level 0 along its longer side.
    #[inline]
    fn levels(&self, lod: f32) -> Levels {
        let tex = self.tex;
        let last = tex.texture.levels - 1;
        let first = tex.first_level.min(last);
        let lod = lod + tex.lod_bias;
        // A number that is none comes from a coordinate that was none.
        if lod.is_nan() || lod <= 0.0 {
            return Levels {
                level: first,
                filter: tex.mag,
                next: 0,
            };
        }
        let span = (last - first) as f32;
        let (level, next) = match tex.mip {
            Mip::None => (first, 0),
            Mip::Point => (first + (lod + 0.5).min(span) as u32, 0),
            Mip::Linear => {
                let lod = lod.min(span);
                let level = first + lod as u32;
                let next = (lod.fract() * 256.0) as u32;
                (level, if level == last { 0 } else { next })
            }
        };
        Levels {
            level,
            filter: tex.min,
            next,
        }
    }

    /// The sample at (`u`, `v`) of `face`. `lod` is as for `levels`;
    /// anything not above 0 magnifies.
    pub fn sample(&self, face: u32, u: f32, v: f32, lod: f32) -> Rgba {
        let l = self.levels(lod);
        let near = self.level(l.level, face, u, v, l.filter);
        if l.next == 0 {
            return near;
        }
        lerp(near, self.level(l.level + 1, face, u, v, l.filter), l.next)
    }

    /// Whether the texel nearest (`u`, `v`), in the level a sample
    /// there reads first, has the colour key's value.
    pub fn keyed(&self, face: u32, u: f32, v: f32, lod: f32) -> bool {
        let Some(key) = self.tex.key else {
            return false;
        };
        let t = &self.tex.texture;
        let level = self.levels(lod).level;
        let (x, y) = self.fixed(level, u, v, 0.0);
        let (w, h) = t.level_size(level);
        match (
            self.tex.address[0].texel((x >> 8) as i32, w as i32),
            self.tex.address[1].texel((y >> 8) as i32, h as i32),
        ) {
            (Some(x), Some(y)) => {
                self.pixel(level, face, x as u32, y as u32) & t.format.defined_bits() == key
            }
            _ => false,
        }
    }
}

/// The face of a cube map the direction (`x`, `y`, `z`) points at, and
/// the coordinates on it.
pub fn cube_face(x: f32, y: f32, z: f32) -> (u32, f32, f32) {
    let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
    let (face, u, v, major) = if ax >= ay && ax >= az {
        if x >= 0.0 {
            (0, -z, -y, ax)
        } else {
            (1, z, -y, ax)
        }
    } else if ay >= az {
        if y >= 0.0 {
            (2, x, z, ay)
        } else {
            (3, x, -z, ay)
        }
    } else if z >= 0.0 {
        (4, x, -y, az)
    } else {
        (5, -x, -y, az)
    };
    // No direction, or one that is no number.
    if major.is_nan() || major == 0.0 {
        return (0, 0.5, 0.5);
    }
    (face, (u / major + 1.0) / 2.0, (v / major + 1.0) / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toadforce::raster::texel::to_word;

    /// A 4x4 `A8R8G8B8` texture whose texel (x, y) is 10h * x in red and
    /// 10h * y in green, with a 2x2 and a 1x1 level of flat colours.
    fn texture() -> (Vec<u8>, Tex) {
        let mut memory = vec![0u8; 4096];
        for y in 0..4u32 {
            for x in 0..4u32 {
                let at = (y * 16 + x * 4) as usize;
                let word = 0xFF00_0000 | (x * 0x10) << 16 | (y * 0x10) << 8;
                memory[at..at + 4].copy_from_slice(&word.to_le_bytes());
            }
        }
        for i in 0..4 {
            memory[256 + i * 4..260 + i * 4].copy_from_slice(&0xFF00_00FFu32.to_le_bytes());
        }
        memory[512..516].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        let texture = Texture {
            format: Format::A8R8G8B8,
            width: 4,
            height: 4,
            levels: 3,
            images: vec![(0, 16), (256, 8), (512, 4)],
            ..Texture::default()
        };
        (memory, Tex::new(texture))
    }

    /// A sample with the texture magnified.
    fn near(s: &Sampler, u: f32, v: f32) -> u32 {
        to_word(s.sample(0, u, v, f32::NEG_INFINITY))
    }

    #[test]
    fn a_point_sample_is_the_texel_the_coordinate_is_in() {
        let (mut memory, tex) = texture();
        let s = Sampler::new(Mem::new(&mut memory), &tex);
        assert_eq!(near(&s, 0.0, 0.0), 0xFF00_0000);
        assert_eq!(near(&s, 0.26, 0.74), 0xFF10_2000);
        assert_eq!(near(&s, 0.999, 0.999), 0xFF30_3000);
        // A coordinate that is no number is texel 0.
        assert_eq!(near(&s, f32::NAN, 0.3), 0xFF00_1000);
    }

    #[test]
    fn coordinates_outside_follow_the_address_mode() {
        let (mut memory, mut tex) = texture();
        let mem = Mem::new(&mut memory);
        let red = |tex: &Tex, u: f32| (near(&Sampler::new(mem, tex), u, 0.0) >> 16) & 0xFF;
        // Wrapping: 1.26 is 0.26, -0.24 is 0.76.
        assert_eq!(red(&tex, 1.26), 0x10);
        assert_eq!(red(&tex, -0.24), 0x30);
        tex.address = [Address::Mirror; 2];
        assert_eq!(red(&tex, 1.01), 0x30);
        assert_eq!(red(&tex, 1.99), 0x00);
        assert_eq!(red(&tex, -0.01), 0x00);
        tex.address = [Address::Clamp; 2];
        assert_eq!(red(&tex, 7.0), 0x30);
        assert_eq!(red(&tex, -7.0), 0x00);
        assert_eq!(red(&tex, 1e30), 0x30);
        tex.address = [Address::Border; 2];
        tex.border = [1, 2, 3, 4];
        assert_eq!(near(&Sampler::new(mem, &tex), 1.01, 0.0), 0x0403_0201);
        assert_eq!(red(&tex, 0.99), 0x30);
    }

    #[test]
    fn a_linear_sample_weighs_the_four_texels_around_it() {
        let (mut memory, mut tex) = texture();
        tex.mag = Filter::Linear;
        tex.address = [Address::Clamp; 2];
        let s = Sampler::new(Mem::new(&mut memory), &tex);
        // At a texel's centre: the texel.
        assert_eq!(near(&s, 0.375, 0.375), 0xFF10_1000);
        // Half way between texels 1 and 2 across, a quarter down from
        // row 1 to row 2.
        assert_eq!(near(&s, 0.5, 0.4375), 0xFF18_1400);
        // At the edge the texel outside is the clamped one.
        assert_eq!(near(&s, 0.0, 0.0), 0xFF00_0000);
    }

    #[test]
    fn the_level_follows_the_texels_a_pixel_covers() {
        let (mut memory, mut tex) = texture();
        let mem = Mem::new(&mut memory);
        let at =
            |tex: &Tex, rho: f32| to_word(Sampler::new(mem, tex).sample(0, 0.1, 0.1, rho.log2()));
        // Without mip mapping, level 0 however small the texture is.
        assert!(!tex.needs_lod());
        assert_eq!(at(&tex, 4.0), 0xFF00_0000);
        tex.mip = Mip::Point;
        assert!(tex.needs_lod());
        assert_eq!(at(&tex, 1.0), 0xFF00_0000);
        assert_eq!(at(&tex, 1.4), 0xFF00_0000);
        assert_eq!(at(&tex, 1.5), 0xFF00_00FF);
        assert_eq!(at(&tex, 2.0), 0xFF00_00FF);
        assert_eq!(at(&tex, 4.0), 0xFFFF_FFFF);
        assert_eq!(at(&tex, 400.0), 0xFFFF_FFFF);
        // A bias of one level, and the chain cut at level 1.
        tex.lod_bias = 1.0;
        assert_eq!(at(&tex, 1.0), 0xFF00_00FF);
        tex.lod_bias = 0.0;
        tex.first_level = 1;
        assert_eq!(at(&tex, 0.5), 0xFF00_00FF);
        assert_eq!(at(&tex, 2.0), 0xFFFF_FFFF);
        // Between two levels, their blend.
        tex.first_level = 0;
        tex.mip = Mip::Linear;
        assert_eq!(at(&tex, 2.0), 0xFF00_00FF);
        assert_eq!(at(&tex, 2.0f32.powf(1.5)), 0xFF80_80FF);
    }

    #[test]
    fn a_palette_a_colour_key_and_blocks_are_read_from_card_memory() {
        let mut memory = vec![0u8; 4096];
        // A 2x1 P8 texture with indices 1 and 2, its palette at 1024.
        memory[0] = 1;
        memory[1] = 2;
        memory[1028..1032].copy_from_slice(&0x8011_2233u32.to_le_bytes());
        memory[1032..1036].copy_from_slice(&0xFF44_5566u32.to_le_bytes());
        let mut tex = Tex::new(Texture {
            format: Format::P8,
            width: 2,
            height: 1,
            levels: 1,
            palette: Some(1024),
            images: vec![(0, 2)],
            ..Texture::default()
        });
        let mem = Mem::new(&mut memory);
        let s = Sampler::new(mem, &tex);
        assert_eq!(to_word(s.fetch(0, 0, 0, 0)), 0x8011_2233);
        assert_eq!(to_word(s.fetch(0, 0, 1, 0)), 0xFF44_5566);
        assert!(!s.keyed(0, 0.75, 0.5, 0.0));
        // The key is an index, and leaves the texel's colour alone.
        tex.key = Some(2);
        let s = Sampler::new(mem, &tex);
        assert!(s.keyed(0, 0.75, 0.5, 0.0));
        assert!(!s.keyed(0, 0.25, 0.5, 0.0));
        assert_eq!(to_word(s.fetch(0, 0, 1, 0)), 0xFF44_5566);

        // An 8x4 DXT1 texture: two blocks, the second all its colour 1.
        let mut memory = vec![0u8; 64];
        memory[8..10].copy_from_slice(&0xF800u16.to_le_bytes());
        memory[10..12].copy_from_slice(&0x001Fu16.to_le_bytes());
        memory[12..16].fill(0x55);
        let tex = Tex::new(Texture {
            format: Format::DXT1,
            width: 8,
            height: 4,
            levels: 1,
            images: vec![(0, 16)],
            ..Texture::default()
        });
        let s = Sampler::new(Mem::new(&mut memory), &tex);
        assert_eq!(to_word(s.fetch(0, 0, 3, 3)), 0xFF00_0000);
        assert_eq!(to_word(s.fetch(0, 0, 4, 0)), 0xFF00_00FF);
        assert_eq!(to_word(s.fetch(0, 0, 7, 3)), 0xFF00_00FF);
    }

    #[test]
    fn a_direction_picks_its_cube_face() {
        assert_eq!(cube_face(1.0, 0.0, 0.0), (0, 0.5, 0.5));
        assert_eq!(cube_face(-2.0, 0.0, 0.0), (1, 0.5, 0.5));
        assert_eq!(cube_face(0.0, 3.0, 0.0), (2, 0.5, 0.5));
        assert_eq!(cube_face(0.0, -1.0, 0.0), (3, 0.5, 0.5));
        assert_eq!(cube_face(0.0, 0.0, 1.0), (4, 0.5, 0.5));
        assert_eq!(cube_face(0.0, 0.0, -1.0), (5, 0.5, 0.5));
        assert_eq!(cube_face(1.0, 1.0, 1.0), (0, 0.0, 0.0));
        assert_eq!(cube_face(0.5, -0.5, 1.0), (4, 0.75, 0.75));
        assert_eq!(cube_face(0.0, 0.0, 0.0), (0, 0.5, 0.5));
        assert_eq!(cube_face(f32::NAN, 0.0, 0.0), (0, 0.5, 0.5));
    }
}
