//! Texels: what a texture's bytes in card memory are as colours
//! ("Texels" in `docs/toadforce.md`).
//!
//! A texel is four channels of 8 bits. Formats with fewer bits are
//! widened by repeating their top bits, and DXT blocks are decoded by
//! the fixed integer formulas of `video::dxt` (its S3TC rules), which
//! the OpenGL renderer uses too: it uploads what this module decodes.

use crate::toadforce::format::Format;
use crate::video::dxt;

/// A colour of 8 bits a channel, as its word's bytes: blue, green, red,
/// alpha. A texel of a bump format has du where red is, dv where green
/// is (both signed) and its luminance where blue is.
pub type Rgba = [u8; 4];

pub const B: usize = 0;
pub const G: usize = 1;
pub const R: usize = 2;
pub const A: usize = 3;

/// A colour in one word (alpha in bits 24 to 31, red in 16 to 23, green
/// in 8 to 15, blue in 0 to 7) as channels.
pub fn from_word(argb: u32) -> Rgba {
    argb.to_le_bytes()
}

pub fn to_word(c: Rgba) -> u32 {
    u32::from_le_bytes(c)
}

/// A field of `bits` bits (1 to 8) widened to 8 by repeating it from
/// the top.
#[inline]
pub fn widen(value: u32, bits: u32) -> u8 {
    debug_assert!((1..=8).contains(&bits) && value < 1 << bits);
    let mut out = value << (8 - bits);
    let mut filled = bits;
    while filled < 8 {
        out |= out >> filled;
        filled *= 2;
    }
    out as u8
}

/// A signed field of `bits` bits as a signed 8-bit number of the same
/// fraction of its range: -16 of 5 bits is -128.
#[inline]
fn widen_signed(value: u32, bits: u32) -> u8 {
    (value << (8 - bits)) as u8
}

#[inline]
fn rgb565(v: u32) -> Rgba {
    [
        widen(v & 0x1F, 5),
        widen(v >> 5 & 0x3F, 6),
        widen(v >> 11 & 0x1F, 5),
        255,
    ]
}

/// The texel a pixel `v` of `format` is. `format` isn't a block format
/// or `P8`, whose texels come from `block` and the palette.
#[inline(always)]
pub fn decode(format: Format, v: u32) -> Rgba {
    use Format::*;
    match format {
        R5G6B5 => rgb565(v),
        X8R8G8B8 => from_word(v | 0xFF00_0000),
        A8R8G8B8 => from_word(v),
        X1R5G5B5 | A1R5G5B5 => [
            widen(v & 0x1F, 5),
            widen(v >> 5 & 0x1F, 5),
            widen(v >> 10 & 0x1F, 5),
            if format == X1R5G5B5 || v & 0x8000 != 0 {
                255
            } else {
                0
            },
        ],
        A4R4G4B4 => [
            widen(v & 0xF, 4),
            widen(v >> 4 & 0xF, 4),
            widen(v >> 8 & 0xF, 4),
            widen(v >> 12 & 0xF, 4),
        ],
        A8 => [255, 255, 255, v as u8],
        L8 => [v as u8, v as u8, v as u8, 255],
        A8L8 => [v as u8, v as u8, v as u8, (v >> 8) as u8],
        V8U8 => [255, (v >> 8) as u8, v as u8, 255],
        L6V5U5 => [
            widen(v >> 10 & 0x3F, 6),
            widen_signed(v >> 5 & 0x1F, 5),
            widen_signed(v & 0x1F, 5),
            255,
        ],
        X8L8V8U8 => [(v >> 16) as u8, (v >> 8) as u8, v as u8, 255],
        // A depth buffer sampled as a texture has no defined colour.
        P8 | DXT1 | DXT2 | DXT3 | DXT4 | DXT5 | D16 | D24X8 | D24S8 => [0, 0, 0, 255],
    }
}

/// Texel `i` (0 to 15, a row of four after another) of a block of
/// `format`, from the block's 8 or 16 bytes, by the S3TC rules of
/// `video::dxt`. A format that isn't a block format has no colour.
#[inline]
pub fn block_texel(format: Format, bytes: &[u8], i: usize) -> Rgba {
    let format = match format {
        Format::DXT1 => dxt::Format::Dxt1,
        Format::DXT2 => dxt::Format::Dxt2,
        Format::DXT3 => dxt::Format::Dxt3,
        Format::DXT4 => dxt::Format::Dxt4,
        Format::DXT5 => dxt::Format::Dxt5,
        _ => return [0, 0, 0, 255],
    };
    let [r, g, b, a] = dxt::decode_texel(format, dxt::Rules::S3tc, bytes, i);
    [b, g, r, a]
}

/// The 16 texels of a block of `format`, a row of four after another,
/// from the block's 8 or 16 bytes.
pub fn block(format: Format, bytes: &[u8]) -> [Rgba; 16] {
    std::array::from_fn(|i| block_texel(format, bytes, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_widen_by_repeating_their_top_bits() {
        assert_eq!(widen(0x1F, 5), 0xFF);
        assert_eq!(widen(0x10, 5), 0x84);
        assert_eq!(widen(0x01, 5), 0x08);
        assert_eq!(widen(0x3F, 6), 0xFF);
        assert_eq!(widen(0x20, 6), 0x82);
        assert_eq!(widen(0xA, 4), 0xAA);
        assert_eq!(widen(1, 1), 0xFF);
        assert_eq!(widen(0, 1), 0);
        assert_eq!(widen(0x5A, 8), 0x5A);
        assert_eq!(widen_signed(0x10, 5) as i8, -128);
        assert_eq!(widen_signed(0x0F, 5) as i8, 120);
        assert_eq!(widen_signed(0x1F, 5) as i8, -8);
    }

    #[test]
    fn each_plain_format_gives_its_channels() {
        use Format::*;
        let word = |f, v| to_word(decode(f, v));
        assert_eq!(word(R5G6B5, 0xF800), 0xFFFF_0000);
        assert_eq!(word(R5G6B5, 0x07E0), 0xFF00_FF00);
        assert_eq!(word(R5G6B5, 0x001F), 0xFF00_00FF);
        assert_eq!(word(X8R8G8B8, 0x0012_3456), 0xFF12_3456);
        assert_eq!(word(A8R8G8B8, 0x8012_3456), 0x8012_3456);
        assert_eq!(word(X1R5G5B5, 0x7C00), 0xFFFF_0000);
        assert_eq!(word(A1R5G5B5, 0x7C00), 0x00FF_0000);
        assert_eq!(word(A1R5G5B5, 0x83E0), 0xFF00_FF00);
        assert_eq!(word(A4R4G4B4, 0x8F0A), 0x88FF_00AA);
        assert_eq!(word(A8, 0x40), 0x40FF_FFFF);
        assert_eq!(word(L8, 0x40), 0xFF40_4040);
        assert_eq!(word(A8L8, 0x8040), 0x8040_4040);
        // du in red's place, dv in green's, luminance in blue's.
        assert_eq!(word(V8U8, 0x80_7F), 0xFF7F_80FF);
        assert_eq!(word(X8L8V8U8, 0x0040_FE02), 0xFF02_FE40);
        assert_eq!(word(L6V5U5, 0x3F << 10 | 0x10 << 5 | 0x0F), 0xFF78_80FF);
    }

    #[test]
    fn a_dxt1_block_has_four_colours_or_three_and_a_hole() {
        // Red above blue: two colours between them, by thirds.
        let mut bytes = [0u8; 8];
        bytes[..2].copy_from_slice(&0xF800u16.to_le_bytes());
        bytes[2..4].copy_from_slice(&0x001Fu16.to_le_bytes());
        // Texels 0 to 3 pick colours 0 to 3; the rest pick 0.
        bytes[4] = 0b11_10_01_00;
        let texels = block(Format::DXT1, &bytes);
        assert_eq!(to_word(texels[0]), 0xFFFF_0000);
        assert_eq!(to_word(texels[1]), 0xFF00_00FF);
        assert_eq!(to_word(texels[2]), 0xFFAA_0055);
        assert_eq!(to_word(texels[3]), 0xFF55_00AA);
        assert_eq!(to_word(texels[15]), 0xFFFF_0000);
        // The other way round: their mean, and a transparent texel.
        bytes[..2].copy_from_slice(&0x001Fu16.to_le_bytes());
        bytes[2..4].copy_from_slice(&0xF800u16.to_le_bytes());
        let texels = block(Format::DXT1, &bytes);
        assert_eq!(to_word(texels[2]), 0xFF7F_007F);
        assert_eq!(to_word(texels[3]), 0);
    }

    #[test]
    fn dxt3_and_dxt5_blocks_have_their_alphas_beside_four_colours() {
        let mut bytes = [0u8; 16];
        // Blue above red in value order would be three colours in
        // DXT1; here it is four all the same.
        bytes[8..10].copy_from_slice(&0x001Fu16.to_le_bytes());
        bytes[10..12].copy_from_slice(&0xF800u16.to_le_bytes());
        bytes[12] = 0b11_10_01_00;
        // DXT3: 4 bits a texel.
        bytes[0] = 0xF0;
        bytes[7] = 0x80;
        let texels = block(Format::DXT3, &bytes);
        assert_eq!(to_word(texels[0]), 0x0000_00FF);
        assert_eq!(to_word(texels[1]), 0xFFFF_0000);
        assert_eq!(to_word(texels[2]), 0x0055_00AA);
        assert_eq!(to_word(texels[3]), 0x00AA_0055);
        assert_eq!(texels[15][A], 0x88);
        assert_eq!(block(Format::DXT2, &bytes), texels);

        // DXT5: 255 above 0 gives six steps between; texel n picks
        // alpha n for the first eight.
        bytes[..8].fill(0);
        bytes[0] = 255;
        bytes[1] = 0;
        let picks: u64 = (0..8).map(|i| i << (3 * i)).sum();
        bytes[2..8].copy_from_slice(&picks.to_le_bytes()[..6]);
        let texels = block(Format::DXT5, &bytes);
        let alphas: Vec<u8> = texels[..8].iter().map(|t| t[A]).collect();
        assert_eq!(alphas, [255, 0, 219, 182, 146, 109, 73, 36]);
        // 0 below 255: four steps, then 0 and 255.
        bytes[0] = 0;
        bytes[1] = 255;
        let texels = block(Format::DXT4, &bytes);
        let alphas: Vec<u8> = texels[..8].iter().map(|t| t[A]).collect();
        assert_eq!(alphas, [0, 255, 51, 102, 153, 204, 0, 255]);
    }
}
