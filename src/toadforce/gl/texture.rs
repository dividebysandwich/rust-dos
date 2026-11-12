//! A texture's images in card memory as the texels the host keeps.
//!
//! The host's texture has what `raster::texel` decodes: four bytes a
//! texel, whatever the format, with the palette looked up and the
//! blocks taken apart by the software renderer's own decoder. A format
//! so reads the same in both renderers ("Texels" in
//! `docs/toadforce.md`). A texture with a colour key has a second
//! image, a byte a texel, that marks the texels whose pixel is the key:
//! the key is compared with the pixel, which the decoded texel no
//! longer is.

use crate::toadforce::format::Format;
use crate::toadforce::level1::Texture;
use crate::toadforce::raster::texel;

/// The `N` bytes at `at`, or zeroes when they aren't all in the
/// memory, as `raster::mem::Mem::read` gives them.
fn read<const N: usize>(memory: &[u8], at: usize) -> [u8; N] {
    at.checked_add(N)
        .and_then(|end| memory.get(at..end))
        .map_or([0; N], |bytes| bytes.try_into().unwrap())
}

/// One image of a texture as the host keeps it.
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// Blue, green, red and alpha of each texel, a row after another.
    pub texels: Vec<u8>,
    /// For a texture with a colour key, 255 for each texel whose pixel
    /// has the key's value and 0 for the others.
    pub keyed: Option<Vec<u8>>,
}

/// Whether the colour key of a texture of `format` can be told from
/// its pixels. A block format has no pixel values.
pub fn has_key(format: Format) -> bool {
    format.block_bytes().is_none()
}

/// The image of `face` of `level` of `t`, from card memory.
pub fn decode(t: &Texture, level: u32, face: u32, memory: &[u8]) -> Image {
    let (width, height) = t.level_size(level);
    let (offset, pitch) = t.image(level, face);
    let (offset, pitch) = (offset as usize, pitch as usize);
    let mut texels = vec![0u8; (width * height * 4) as usize];
    let key = t.key.filter(|_| has_key(t.format));
    let mut keyed = key.map(|_| vec![0u8; (width * height) as usize]);
    if let Some(size) = t.format.block_bytes() {
        let size = size as usize;
        for by in 0..height.div_ceil(4) {
            for bx in 0..width.div_ceil(4) {
                let at = offset + by as usize * pitch + bx as usize * size;
                let block = if size == 8 {
                    texel::block(t.format, &read::<8>(memory, at))
                } else {
                    texel::block(t.format, &read::<16>(memory, at))
                };
                for (i, texel) in block.iter().enumerate() {
                    let (x, y) = (bx * 4 + i as u32 % 4, by * 4 + i as u32 / 4);
                    if x < width && y < height {
                        let to = ((y * width + x) * 4) as usize;
                        texels[to..to + 4].copy_from_slice(texel);
                    }
                }
            }
        }
    } else {
        let bytes = t.format.bytes() as usize;
        let defined = t.format.defined_bits();
        let palette = t.palette.unwrap_or(0) as usize;
        for y in 0..height {
            for x in 0..width {
                let at = offset + y as usize * pitch + x as usize * bytes;
                // A pixel whose bytes aren't all in the memory is 0.
                let value = match bytes {
                    1 => read::<1>(memory, at)[0] as u32,
                    2 => u16::from_le_bytes(read(memory, at)) as u32,
                    _ => u32::from_le_bytes(read(memory, at)),
                };
                let texel = if t.format == Format::P8 {
                    read::<4>(memory, palette + value as usize * 4)
                } else {
                    texel::decode(t.format, value)
                };
                let i = (y * width + x) as usize;
                texels[i * 4..i * 4 + 4].copy_from_slice(&texel);
                if let (Some(key), Some(keyed)) = (key, &mut keyed) {
                    keyed[i] = if value & defined == key { 255 } else { 0 };
                }
            }
        }
    }
    Image {
        width,
        height,
        texels,
        keyed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toadforce::raster::mem::Mem;
    use crate::toadforce::raster::sample::{Sampler, Tex};

    /// Every format's image is what the software sampler reads texel
    /// by texel, with a palette, a pitch wider than a row, and an image
    /// that ends past the memory.
    #[test]
    fn an_image_has_the_texels_the_software_sampler_reads() {
        use Format::*;
        let formats = [
            P8, A8, L8, A8L8, R5G6B5, X1R5G5B5, A1R5G5B5, A4R4G4B4, X8R8G8B8, A8R8G8B8, V8U8,
            L6V5U5, X8L8V8U8, DXT1, DXT2, DXT3, DXT4, DXT5,
        ];
        for format in formats {
            let mut memory: Vec<u8> = (0..4096u32)
                .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
                .collect();
            let line = format.line_bytes(8) as u32;
            let texture = Texture {
                format,
                width: 8,
                height: 4,
                levels: 2,
                palette: (format == P8).then_some(1024),
                key: Some(memory[0] as u32),
                images: vec![(0, line + 4), (4096 - line - 2, line)],
                ..Texture::default()
            };
            let images = [0, 1].map(|level| decode(&texture, level, 0, &memory));
            let tex = Tex::new(texture.clone());
            let sampler = Sampler::new(Mem::new(&mut memory), &tex);
            for (level, image) in images.iter().enumerate() {
                assert_eq!((image.width, image.height), (8 >> level, 4 >> level));
                for y in 0..image.height {
                    for x in 0..image.width {
                        let i = ((y * image.width + x) * 4) as usize;
                        let want = sampler.fetch(level as u32, 0, x, y);
                        assert_eq!(image.texels[i..i + 4], want, "{format:?} {level} {x} {y}");
                    }
                }
                assert_eq!(image.keyed.is_some(), has_key(format));
            }
            // The first pixel has the key's value in a one-byte format.
            if format.bytes() == 1 {
                assert_eq!(images[0].keyed.as_ref().unwrap()[0], 255);
            }
        }
    }
}
