//! Sampling: the colour a texture gives at a coordinate, read from card
//! memory when it is asked for ("Texels" in `docs/toadforce.md`).

use super::mem::Mem;
use super::texel::{self, Rgba};
use crate::toadforce::caps;
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

/// A texel's four channels in the four 16-bit fields of a word, for
/// `lerp_wide`.
#[inline(always)]
fn wide(c: Rgba) -> u64 {
    let v = u32::from_le_bytes(c) as u64;
    let v = (v | v << 16) & 0x0000_FFFF_0000_FFFF;
    (v | v << 8) & 0x00FF_00FF_00FF_00FF
}

/// `wide` of what `texel::decode` makes of a pixel, for the formats
/// most textures have, without the bytes in between: field by field,
/// with the top bits repeated as `texel::widen` does.
mod wide_of {
    #[inline(always)]
    fn fields(b: u64, g: u64, r: u64, a: u64) -> u64 {
        b | g << 16 | r << 32 | a << 48
    }

    #[inline(always)]
    pub fn r5g6b5(v: u32) -> u64 {
        let v = v as u64;
        let (b, g, r) = (v & 0x1F, v >> 5 & 0x3F, v >> 11 & 0x1F);
        fields(b << 3 | b >> 2, g << 2 | g >> 4, r << 3 | r >> 2, 255)
    }

    /// `alpha` for `A1R5G5B5`; without it the texel is opaque.
    #[inline(always)]
    pub fn r5g5b5(v: u32, alpha: bool) -> u64 {
        let v = v as u64;
        let (b, g, r) = (v & 0x1F, v >> 5 & 0x1F, v >> 10 & 0x1F);
        let a = if !alpha || v & 0x8000 != 0 { 255 } else { 0 };
        fields(b << 3 | b >> 2, g << 3 | g >> 2, r << 3 | r >> 2, a)
    }

    #[inline(always)]
    pub fn a4r4g4b4(v: u32) -> u64 {
        let v = v as u64;
        // A field of 4 bits twice over is 17 times it.
        fields(v & 0xF, v >> 4 & 0xF, v >> 8 & 0xF, v >> 12 & 0xF) * 17
    }

    /// `alpha` for `A8R8G8B8`; without it the texel is opaque.
    #[inline(always)]
    pub fn r8g8b8(v: u32, alpha: bool) -> u64 {
        super::wide(if alpha { v } else { v | 0xFF00_0000 }.to_le_bytes())
    }
}

#[inline(always)]
fn narrow(v: u64) -> Rgba {
    let v = (v | v >> 8) & 0x0000_FFFF_0000_FFFF;
    ((v | v >> 16) as u32).to_le_bytes()
}

/// `lerp` of all four channels at once. A channel's sum is at most
/// 255 * 256 + 128, so no field carries into the next.
#[inline(always)]
fn lerp_wide(a: u64, b: u64, f: u32) -> u64 {
    let f = f as u64;
    ((a * (256 - f) + b * f + 0x0080_0080_0080_0080) >> 8) & 0x00FF_00FF_00FF_00FF
}

/// The levels a sample reads: the level, the filter in it, and how much
/// of the next level is blended in, of 256.
struct Levels {
    level: u32,
    filter: Filter,
    next: u32,
}

/// How a texture's pixels become texels: what the format says, found
/// out once for a sampler.
#[derive(Clone, Copy, PartialEq)]
enum Texels {
    /// Pixels of a format `texel::decode` reads.
    Plain,
    /// Indices into the palette at this offset.
    Palette(usize),
    /// Blocks of four by four, of this many bytes.
    Blocks(u32),
}

/// Where one image of a texture is and how large.
#[derive(Clone, Copy, Default)]
struct Image {
    offset: usize,
    pitch: usize,
    width: i32,
    height: i32,
}

/// The most levels a texture of the largest size has.
const LEVELS: usize = caps::TEXTURE_MAX.ilog2() as usize + 1;

/// A `Tex` in card memory.
#[derive(Clone, Copy)]
pub struct Sampler<'a> {
    mem: Mem<'a>,
    pub tex: &'a Tex,
    texels: Texels,
    /// The bytes of a pixel.
    bytes: u32,
    /// The levels of face 0, which is the whole texture unless it is a
    /// cube map.
    images: [Image; LEVELS],
}

impl<'a> Sampler<'a> {
    pub fn new(mem: Mem<'a>, tex: &'a Tex) -> Self {
        let t = &tex.texture;
        let texels = match t.format.block_bytes() {
            Some(size) => Texels::Blocks(size),
            None if t.format == Format::P8 => Texels::Palette(t.palette.unwrap_or(0) as usize),
            None => Texels::Plain,
        };
        let mut images = [Image::default(); LEVELS];
        for (level, image) in images.iter_mut().enumerate().take(t.levels as usize) {
            *image = Self::image_of(t, level as u32, 0);
        }
        Sampler {
            mem,
            tex,
            texels,
            bytes: t.format.bytes(),
            images,
        }
    }

    fn image_of(t: &Texture, level: u32, face: u32) -> Image {
        let (offset, pitch) = t.image(level, face);
        let (width, height) = t.level_size(level);
        Image {
            offset: offset as usize,
            pitch: pitch as usize,
            width: width as i32,
            height: height as i32,
        }
    }

    /// The image of `face` of `level`. `images` holds zeroes past the
    /// texture's levels, so a level it doesn't have goes to
    /// `Texture::image`, which panics.
    #[inline]
    fn image(&self, level: u32, face: u32) -> Image {
        if face == 0 && level < self.tex.texture.levels && (level as usize) < LEVELS {
            self.images[level as usize]
        } else {
            Self::image_of(&self.tex.texture, level, face)
        }
    }

    /// The pixel value at (`x`, `y`) of `image`, both inside it. The
    /// format isn't a block format.
    #[inline]
    fn value(&self, image: &Image, x: u32, y: u32) -> u32 {
        let at = image.offset + y as usize * image.pitch + (x * self.bytes) as usize;
        self.mem.pixel(at, self.bytes)
    }

    /// The texel at (`x`, `y`) of `image`, both inside it.
    #[inline]
    fn texel(&self, image: &Image, x: u32, y: u32) -> Rgba {
        match self.texels {
            Texels::Plain => texel::decode(self.tex.texture.format, self.value(image, x, y)),
            Texels::Palette(palette) => self
                .mem
                .read(palette + self.value(image, x, y) as usize * 4),
            Texels::Blocks(size) => {
                let at = image.offset
                    + (y / 4) as usize * image.pitch
                    + (x / 4) as usize * size as usize;
                let i = (y % 4 * 4 + x % 4) as usize;
                let format = self.tex.texture.format;
                if size == 8 {
                    texel::block_texel(format, &self.mem.read::<8>(at), i)
                } else {
                    texel::block_texel(format, &self.mem.read::<16>(at), i)
                }
            }
        }
    }

    /// The texel at (`x`, `y`) of `face` of `level`, both inside it.
    #[inline]
    pub fn fetch(&self, level: u32, face: u32, x: u32, y: u32) -> Rgba {
        self.texel(&self.image(level, face), x, y)
    }

    /// The texel of `image` in column `x` and row `y`, each None for
    /// the border.
    #[inline]
    fn at(&self, image: &Image, x: Option<i32>, y: Option<i32>) -> Rgba {
        match (x, y) {
            (Some(x), Some(y)) => self.texel(image, x as u32, y as u32),
            _ => self.tex.border,
        }
    }

    /// (`u`, `v`) in 1/256 of a texel of `image`, `shift` of them to
    /// the left and up.
    #[inline]
    fn fixed(image: &Image, u: f32, v: f32, shift: f64) -> (i64, i64) {
        const LIMIT: f64 = (1i64 << 31) as f64;
        let to = |c: f32, size: i32| {
            let at = (c as f64 * size as f64 * 256.0 - shift).clamp(-LIMIT, LIMIT);
            // Rounded down: the cast rounds toward 0, which is one too
            // high below 0 unless the number is whole. A coordinate
            // that is none casts to 0, texel 0.
            let whole = at as i64;
            whole - (whole as f64 > at) as i64
        };
        (to(u, image.width), to(v, image.height))
    }

    /// The column and row of the texel of `image` nearest (`u`, `v`),
    /// each None for the border.
    #[inline]
    fn nearest(&self, image: &Image, u: f32, v: f32) -> (Option<i32>, Option<i32>) {
        let (x, y) = Self::fixed(image, u, v, 0.0);
        let [across, down] = self.tex.address;
        (
            across.texel((x >> 8) as i32, image.width),
            down.texel((y >> 8) as i32, image.height),
        )
    }

    /// One level at (`u`, `v`), in texture coordinates where 0 to 1 is
    /// the texture, through `filter`.
    #[inline]
    fn level(&self, level: u32, face: u32, u: f32, v: f32, filter: Filter) -> Rgba {
        let image = self.image(level, face);
        match filter {
            Filter::Point => {
                let (x, y) = self.nearest(&image, u, v);
                if self.texels == Texels::Plain
                    && let (Some(x), Some(y)) = (x, y)
                {
                    return narrow(self.plain(&image, [(x, y)])[0]);
                }
                self.at(&image, x, y)
            }
            Filter::Linear => {
                // A texel's centre is half a texel inside it.
                let (x, y) = Self::fixed(&image, u, v, 128.0);
                let (fx, fy) = ((x & 255) as u32, (y & 255) as u32);
                let (x, y) = ((x >> 8) as i32, (y >> 8) as i32);
                let [across, down] = self.tex.address;
                let (x0, x1) = (
                    across.texel(x, image.width),
                    across.texel(x.wrapping_add(1), image.width),
                );
                let (y0, y1) = (
                    down.texel(y, image.height),
                    down.texel(y.wrapping_add(1), image.height),
                );
                if self.texels == Texels::Plain
                    && let (Some(x0), Some(x1), Some(y0), Some(y1)) = (x0, x1, y0, y1)
                {
                    let t = self.plain(&image, [(x0, y0), (x1, y0), (x0, y1), (x1, y1)]);
                    return narrow(lerp_wide(
                        lerp_wide(t[0], t[1], fx),
                        lerp_wide(t[2], t[3], fx),
                        fy,
                    ));
                }
                let top = lerp(self.at(&image, x0, y0), self.at(&image, x1, y0), fx);
                let bottom = lerp(self.at(&image, x0, y1), self.at(&image, x1, y1), fx);
                lerp(top, bottom, fy)
            }
        }
    }

    /// The texels of `image` at `places` (column, row), all inside it,
    /// as `wide` has them. The texture's pixels are `Texels::Plain`.
    ///
    /// What a sample of most textures comes to. The format is looked
    /// at once for the lot, and each format's arm has its own copy of
    /// the reads and the decoding, with the pixel's size known.
    #[inline]
    fn plain<const K: usize>(&self, image: &Image, places: [(i32, i32); K]) -> [u64; K] {
        use Format::*;
        match self.tex.texture.format {
            R5G6B5 => self.pixels::<2, K>(image, places, wide_of::r5g6b5),
            A4R4G4B4 => self.pixels::<2, K>(image, places, wide_of::a4r4g4b4),
            A1R5G5B5 => self.pixels::<2, K>(image, places, |v| wide_of::r5g5b5(v, true)),
            X1R5G5B5 => self.pixels::<2, K>(image, places, |v| wide_of::r5g5b5(v, false)),
            A8R8G8B8 => self.pixels::<4, K>(image, places, |v| wide_of::r8g8b8(v, true)),
            X8R8G8B8 => self.pixels::<4, K>(image, places, |v| wide_of::r8g8b8(v, false)),
            format => {
                let decode = |v| wide(texel::decode(format, v));
                match self.bytes {
                    1 => self.pixels::<1, K>(image, places, decode),
                    2 => self.pixels::<2, K>(image, places, decode),
                    _ => self.pixels::<4, K>(image, places, decode),
                }
            }
        }
    }

    /// `plain` for pixels of `N` bytes, each made a texel by `decode`.
    #[inline(always)]
    fn pixels<const N: usize, const K: usize>(
        &self,
        image: &Image,
        places: [(i32, i32); K],
        decode: impl Fn(u32) -> u64,
    ) -> [u64; K] {
        let at = places.map(|(x, y)| image.offset + y as usize * image.pitch + x as usize * N);
        let value = |bytes: [u8; N]| {
            let mut word = [0; 4];
            word[..N].copy_from_slice(&bytes);
            u32::from_le_bytes(word)
        };
        match self.mem.read_all::<N, K>(at) {
            Some(pixels) => pixels.map(|bytes| decode(value(bytes))),
            // Pixels outside the memory read as 0, each by itself.
            None => at.map(|at| decode(value(self.mem.read(at)))),
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
        // `MAXMIPLEVEL` limits the level the pixel's size picks, as in
        // Direct3D: it doesn't move it. The level of detail is measured
        // in texels of level 0.
        let (level, next) = match tex.mip {
            Mip::None => (first, 0),
            Mip::Point => (((lod + 0.5) as u32).clamp(first, last), 0),
            Mip::Linear => {
                let lod = lod.clamp(first as f32, last as f32);
                let level = lod as u32;
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
        let image = self.image(self.levels(lod).level, face);
        match self.nearest(&image, u, v) {
            (Some(x), Some(y)) => {
                let defined = self.tex.texture.format.defined_bits();
                self.value(&image, x as u32, y as u32) & defined == key
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
    fn the_wide_decoders_give_what_decode_gives() {
        let same = |format: Format, v: u32, got: u64| {
            assert_eq!(got, wide(texel::decode(format, v)), "{format:?} {v:#x}");
        };
        for v in 0..=0xFFFF {
            same(Format::R5G6B5, v, wide_of::r5g6b5(v));
            same(Format::A4R4G4B4, v, wide_of::a4r4g4b4(v));
            same(Format::A1R5G5B5, v, wide_of::r5g5b5(v, true));
            same(Format::X1R5G5B5, v, wide_of::r5g5b5(v, false));
        }
        for i in 0..=0xFFFFu32 {
            // Every value of each byte, in different company.
            let v = i.wrapping_mul(0x9E37_79B9) ^ i << 16;
            same(Format::A8R8G8B8, v, wide_of::r8g8b8(v, true));
            same(Format::X8R8G8B8, v, wide_of::r8g8b8(v, false));
        }
    }

    #[test]
    fn a_linear_sample_blends_as_its_channels_do_one_by_one() {
        // The four channels in one word against `lerp`, for every
        // weight and the channel values at and around the ends.
        let values = [0u8, 1, 2, 127, 128, 129, 254, 255];
        for f in 0..256 {
            for &a in &values {
                for &b in &values {
                    let (x, y) = ([a, b, 255 - a, 0], [b, a, 255, 255 - b]);
                    assert_eq!(narrow(lerp_wide(wide(x), wide(y), f)), lerp(x, y, f));
                }
            }
        }
    }

    #[test]
    fn a_sample_of_plain_pixels_is_what_its_texels_give_one_by_one() {
        use Format::*;
        let formats = [
            L8, A8L8, R5G6B5, A4R4G4B4, A1R5G5B5, X1R5G5B5, A8R8G8B8, X8R8G8B8, X8L8V8U8,
        ];
        for format in formats {
            let n = format.bytes() as usize;
            // A 4x4 texture whose last pixel is past the memory's end,
            // so a sample with it reads its pixels one at a time.
            let mut memory: Vec<u8> = (0..15 * n).map(|i| (i * 89 + 7) as u8).collect();
            let mut tex = Tex::new(Texture {
                format,
                width: 4,
                height: 4,
                levels: 1,
                images: vec![(0, 4 * n as u32)],
                ..Texture::default()
            });
            let mem = Mem::new(&mut memory);
            for filter in [Filter::Point, Filter::Linear] {
                tex.mag = filter;
                let s = Sampler::new(mem, &tex);
                let texel = |x: i64, y: i64| s.fetch(0, 0, (x & 3) as u32, (y & 3) as u32);
                for (u, v) in [(0.1, 0.2), (0.5, 0.5), (0.9, 0.95), (-0.3, 1.7)] {
                    let shift = if filter == Filter::Linear { 128.0 } else { 0.0 };
                    let to = |c: f32| (c as f64 * 1024.0 - shift).floor() as i64;
                    let (x, y) = (to(u), to(v));
                    let (fx, fy) = ((x & 255) as u32, (y & 255) as u32);
                    let (x, y) = (x >> 8, y >> 8);
                    let want = match filter {
                        Filter::Point => texel(x, y),
                        Filter::Linear => lerp(
                            lerp(texel(x, y), texel(x + 1, y), fx),
                            lerp(texel(x, y + 1), texel(x + 1, y + 1), fx),
                            fy,
                        ),
                    };
                    let got = s.sample(0, u, v, f32::NEG_INFINITY);
                    assert_eq!(got, want, "{format:?} {filter:?} ({u}, {v})");
                }
            }
        }
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
        // A bias of one level, and the chain cut at level 1: the levels
        // the pixel's size picks below 1 are 1, the others stay.
        tex.lod_bias = 1.0;
        assert_eq!(at(&tex, 1.0), 0xFF00_00FF);
        tex.lod_bias = 0.0;
        tex.first_level = 1;
        assert_eq!(at(&tex, 0.5), 0xFF00_00FF);
        assert_eq!(at(&tex, 1.4), 0xFF00_00FF);
        assert_eq!(at(&tex, 2.0), 0xFF00_00FF);
        assert_eq!(at(&tex, 4.0), 0xFFFF_FFFF);
        tex.mip = Mip::Linear;
        assert_eq!(at(&tex, 1.4), 0xFF00_00FF);
        tex.mip = Mip::Point;
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

    /// The sampler as it was before it kept each level's place and read
    /// plain pixels by format: everything from the texture by itself,
    /// one texel and one channel at a time. `differs_in_nothing` holds
    /// the sampler to it.
    mod reference {
        use super::*;

        fn texel(mem: Mem, t: &Texture, level: u32, face: u32, x: u32, y: u32) -> Rgba {
            let (offset, pitch) = t.image(level, face);
            if let Some(size) = t.format.block_bytes() {
                let at = offset as usize
                    + (y / 4) as usize * pitch as usize
                    + (x / 4) as usize * size as usize;
                let i = (y % 4 * 4 + x % 4) as usize;
                return if size == 8 {
                    texel::block_texel(t.format, &mem.read::<8>(at), i)
                } else {
                    texel::block_texel(t.format, &mem.read::<16>(at), i)
                };
            }
            let value = pixel(mem, t, level, face, x, y);
            if t.format == Format::P8 {
                return mem.read(t.palette.unwrap_or(0) as usize + value as usize * 4);
            }
            texel::decode(t.format, value)
        }

        fn pixel(mem: Mem, t: &Texture, level: u32, face: u32, x: u32, y: u32) -> u32 {
            let (offset, pitch) = t.image(level, face);
            let bytes = t.format.bytes();
            mem.pixel(offset as usize + y as usize * pitch as usize + (x * bytes) as usize, bytes)
        }

        fn at(mem: Mem, tex: &Tex, level: u32, face: u32, x: i32, y: i32) -> Rgba {
            let (w, h) = tex.texture.level_size(level);
            match (
                tex.address[0].texel(x, w as i32),
                tex.address[1].texel(y, h as i32),
            ) {
                (Some(x), Some(y)) => texel(mem, &tex.texture, level, face, x as u32, y as u32),
                _ => tex.border,
            }
        }

        fn fixed(tex: &Tex, level: u32, u: f32, v: f32, shift: f64) -> (i64, i64) {
            let (w, h) = tex.texture.level_size(level);
            let limit = (1i64 << 31) as f64;
            let to = |c: f32, size: u32| {
                let at = (c as f64 * size as f64 * 256.0 - shift).floor();
                if at.is_nan() {
                    0
                } else {
                    at.clamp(-limit, limit) as i64
                }
            };
            (to(u, w), to(v, h))
        }

        fn level(mem: Mem, tex: &Tex, level: u32, face: u32, u: f32, v: f32, f: Filter) -> Rgba {
            let at = |x, y| at(mem, tex, level, face, x, y);
            match f {
                Filter::Point => {
                    let (x, y) = fixed(tex, level, u, v, 0.0);
                    at((x >> 8) as i32, (y >> 8) as i32)
                }
                Filter::Linear => {
                    let (x, y) = fixed(tex, level, u, v, 128.0);
                    let (fx, fy) = ((x & 255) as u32, (y & 255) as u32);
                    let (x, y) = ((x >> 8) as i32, (y >> 8) as i32);
                    let (x1, y1) = (x.wrapping_add(1), y.wrapping_add(1));
                    let top = lerp(at(x, y), at(x1, y), fx);
                    let bottom = lerp(at(x, y1), at(x1, y1), fx);
                    lerp(top, bottom, fy)
                }
            }
        }

        pub fn sample(s: &Sampler, mem: Mem, face: u32, u: f32, v: f32, lod: f32) -> Rgba {
            let l = s.levels(lod);
            let near = level(mem, s.tex, l.level, face, u, v, l.filter);
            if l.next == 0 {
                return near;
            }
            let far = level(mem, s.tex, l.level + 1, face, u, v, l.filter);
            lerp(near, far, l.next)
        }

        pub fn keyed(s: &Sampler, mem: Mem, face: u32, u: f32, v: f32, lod: f32) -> bool {
            let Some(key) = s.tex.key else {
                return false;
            };
            let t = &s.tex.texture;
            let level = s.levels(lod).level;
            let (x, y) = fixed(s.tex, level, u, v, 0.0);
            let (w, h) = t.level_size(level);
            match (
                s.tex.address[0].texel((x >> 8) as i32, w as i32),
                s.tex.address[1].texel((y >> 8) as i32, h as i32),
            ) {
                (Some(x), Some(y)) => {
                    pixel(mem, t, level, face, x as u32, y as u32) & t.format.defined_bits() == key
                }
                _ => false,
            }
        }
    }

    /// A xorshift generator, so the test is the same on every run.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        fn pick<T: Copy>(&mut self, of: &[T]) -> T {
            of[self.below(of.len() as u64) as usize]
        }

        /// A coordinate: one of the odd ones, a whole number of 1/256
        /// of a texel of a level or the float either side of it, or
        /// any.
        fn coordinate(&mut self, size: u32) -> f32 {
            let odd = [
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
                0.0,
                -0.0,
                1.0,
                -1.0,
                -2.0,
                0.5,
                1e10,
                -1e10,
                f32::MAX,
                f32::MIN,
                f32::MIN_POSITIVE,
                -f32::MIN_POSITIVE,
                // Just past what 2^31 / 256 texels of the largest
                // size come to, and just short of it.
                8388608.0 / 2048.0,
                -8388608.0 / 2048.0,
                8388607.9 / 2048.0,
                -8388607.9 / 2048.0,
            ];
            match self.below(4) {
                0 => self.pick(&odd),
                1 | 2 => {
                    let size = (size >> self.below(12)).max(1) as f32;
                    let whole = (self.below(4096) as i64 - 2048) as f32 / (size * 256.0);
                    match self.below(3) {
                        0 => whole,
                        1 => whole.next_down(),
                        _ => whole.next_up(),
                    }
                }
                _ => (self.next() as i64 as f64 / i64::MAX as f64 * 3.0) as f32,
            }
        }
    }

    #[test]
    fn differs_in_nothing_from_reading_each_texel_by_itself() {
        use Format::*;
        let formats = [
            P8, R5G6B5, X8R8G8B8, A8R8G8B8, X1R5G5B5, A1R5G5B5, A4R4G4B4, A8, L8, A8L8, DXT1,
            DXT2, DXT3, DXT4, DXT5, V8U8, L6V5U5, X8L8V8U8,
        ];
        let addresses = [
            Address::Wrap,
            Address::Mirror,
            Address::Clamp,
            Address::Border,
        ];
        let filters = [Filter::Point, Filter::Linear];
        let mut r = Random(0x9E37_79B9_7F4A_7C15);
        for _ in 0..3000 {
            let format = r.pick(&formats);
            let cube = r.below(4) == 0;
            // Sides of 1 to 2048, some long and thin for more than 12
            // levels' worth of a sampler's own list.
            let width = 1u32 << r.below(12);
            let height = if cube {
                width
            } else if width > 64 {
                1 << r.below(2)
            } else {
                1 << r.below(7)
            };
            if cube && width > 64 {
                continue;
            }
            let faces = if cube { 6 } else { 1 };
            let levels = 1 + r.below((width.max(height).ilog2() + 1) as u64) as u32;
            let mut images = Vec::new();
            let mut end = r.below(64) as u32;
            for i in 0..levels * faces {
                let (w, h) = ((width >> (i / faces)).max(1), (height >> (i / faces)).max(1));
                let pitch = format.line_bytes(w) as u32 + r.below(3) as u32 * 4;
                images.push((end, pitch));
                end += format.image_bytes(w, h, pitch) as u32 + r.below(16) as u32;
            }
            let palette = (format == P8).then(|| end + r.below(64) as u32);
            end = palette.map_or(end, |at| at + 1024);
            // Most of the time all of the texture is in the memory;
            // else it stops somewhere in it, and the rest reads as 0.
            let len = match r.below(4) {
                0 => r.below(end as u64 + 1) as usize,
                _ => end as usize,
            };
            let mut memory: Vec<u8> = (0..len).map(|_| r.next() as u8).collect();
            let mem = Mem::new(&mut memory);
            let mut tex = Tex::new(Texture {
                format,
                width,
                height,
                levels,
                cube,
                palette,
                images,
                ..Texture::default()
            });
            tex.address = [r.pick(&addresses), r.pick(&addresses)];
            tex.border = (r.next() as u32).to_le_bytes();
            tex.mag = r.pick(&filters);
            tex.min = r.pick(&filters);
            tex.mip = r.pick(&[Mip::None, Mip::Point, Mip::Linear]);
            tex.lod_bias = r.pick(&[0.0, 0.5, -1.0]);
            tex.first_level = r.below(levels as u64) as u32;
            if format.block_bytes().is_none() {
                tex.key = Some(r.next() as u32 & format.defined_bits() & r.pick(&[0, !0]));
            }
            let s = Sampler::new(mem, &tex);
            for _ in 0..100 {
                let face = r.below(faces as u64) as u32;
                let (u, v) = (r.coordinate(width), r.coordinate(height));
                let lod = match r.below(4) {
                    0 => r.pick(&[f32::NAN, f32::NEG_INFINITY, f32::INFINITY, 0.0]),
                    _ => (r.below(1600) as f32 - 200.0) / 100.0,
                };
                assert_eq!(
                    s.sample(face, u, v, lod),
                    reference::sample(&s, mem, face, u, v, lod),
                    "{tex:?} face {face} at ({u}, {v}) lod {lod}"
                );
                assert_eq!(
                    s.keyed(face, u, v, lod),
                    reference::keyed(&s, mem, face, u, v, lod),
                    "{tex:?} face {face} at ({u}, {v}) lod {lod}"
                );
            }
        }
    }
}
