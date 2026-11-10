//! The DXT1 to DXT5 texture formats (S3 Texture Compression): blocks of
//! 4x4 pixels, each with two 5-6-5 colours and two bits a pixel that
//! pick one of them or a colour between them, and in DXT2 to DXT5 alpha
//! before them: 4 bits a pixel in DXT2 and DXT3, and in DXT4 and DXT5
//! two 8-bit values and three bits a pixel that pick one of them or a
//! value between them. DXT2 and DXT4 hold colours already multiplied by
//! their alpha.
//!
//! Two sets of rules decode them, in exact integer arithmetic:
//!
//! - `Rules::S3tc`, the format's specification, as Microsoft's DirectX
//!   documentation and the cards that read DXT textures have it.
//! - `Rules::DirectDraw`, what DirectDraw's own decoder does when a blit
//!   copies a DXT surface to an RGB one. A probe program run on Windows
//!   98 SE with DirectX 7 showed it (the probe and its output are
//!   described in `docs/builtin-windows-architecture.md`, "DXT
//!   surfaces"); Windows 11's DirectDraw decodes the same. It differs
//!   from the specification in three ways: a block of DXT2 to DXT5 whose
//!   first colour isn't greater than its second has three colours and
//!   black, as in DXT1, where the specification has four colours; the
//!   six or eight alpha values of DXT4 and DXT5 run from the second
//!   value to the first, so that code 2 is the one nearest the second;
//!   and DXT2 and DXT4 keep each colour channel at most the alpha.

/// A DXT format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Dxt1,
    Dxt2,
    Dxt3,
    Dxt4,
    Dxt5,
}

/// Which decoder's rules to follow (see the module's comment).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rules {
    S3tc,
    DirectDraw,
}

impl Format {
    /// The format of a DirectDraw FOURCC code, if it is one of these.
    pub fn from_fourcc(code: u32) -> Option<Format> {
        Some(match &code.to_le_bytes() {
            b"DXT1" => Format::Dxt1,
            b"DXT2" => Format::Dxt2,
            b"DXT3" => Format::Dxt3,
            b"DXT4" => Format::Dxt4,
            b"DXT5" => Format::Dxt5,
            _ => return None,
        })
    }

    /// The bytes of a block of 4x4 pixels.
    pub fn block_bytes(self) -> usize {
        match self {
            Format::Dxt1 => 8,
            _ => 16,
        }
    }

    /// Whether its colours are multiplied by its alpha.
    pub fn premultiplied(self) -> bool {
        matches!(self, Format::Dxt2 | Format::Dxt4)
    }

    /// The bytes of a row of blocks of a surface `width` pixels wide.
    pub fn row_bytes(self, width: u32) -> usize {
        width.div_ceil(4).max(1) as usize * self.block_bytes()
    }

    /// The bytes of a surface of `width` by `height` pixels: whole
    /// blocks, at least one.
    pub fn size(self, width: u32, height: u32) -> usize {
        self.row_bytes(width) * height.div_ceil(4).max(1) as usize
    }
}

/// A decoded pixel: red, green, blue and alpha, 8 bits each.
pub type Rgba = [u8; 4];

/// A 5-6-5 colour widened to 8 bits a channel, each channel's top bits
/// repeated below it, so that 31 and 63 become 255.
fn widen(c: u16) -> [u32; 3] {
    let (r, g, b) = ((c >> 11) as u32, (c >> 5 & 0x3F) as u32, (c & 0x1F) as u32);
    [r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2]
}

/// The four colours a colour block's codes pick, alpha 255 but for
/// DXT1's transparent black.
fn palette(block: &[u8], format: Format, rules: Rules) -> [Rgba; 4] {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let (a, b) = (widen(c0), widen(c1));
    let mix = |f: &dyn Fn(u32, u32) -> u32| -> Rgba {
        [
            f(a[0], b[0]) as u8,
            f(a[1], b[1]) as u8,
            f(a[2], b[2]) as u8,
            255,
        ]
    };
    let four = c0 > c1 || (format != Format::Dxt1 && rules == Rules::S3tc);
    let (third, fourth) = if four {
        // A third of the way from one to the other, to the nearest.
        (
            mix(&|a, b| (2 * a + b + 1) / 3),
            mix(&|a, b| (a + 2 * b + 1) / 3),
        )
    } else {
        // Half way, rounded down, and black, transparent in DXT1.
        let alpha = if format == Format::Dxt1 { 0 } else { 255 };
        (mix(&|a, b| (a + b) / 2), [0, 0, 0, alpha])
    };
    [mix(&|a, _| a), mix(&|_, b| b), third, fourth]
}

/// The 16 alpha values of a DXT4 or DXT5 alpha block, in pixel order.
fn interpolated_alpha(block: &[u8], rules: Rules) -> [u8; 16] {
    let (a0, a1) = (block[0] as u32, block[1] as u32);
    // The values between run from the first to the second, or under
    // DirectDraw's rules from the second to the first; each is rounded
    // to the nearest.
    let (from, to) = match rules {
        Rules::S3tc => (a0, a1),
        Rules::DirectDraw => (a1, a0),
    };
    let between = |steps: u32, i: u32| ((steps - i) * from + i * to + steps / 2) / steps;
    let mut values = [a0, a1, 0, 0, 0, 0, 0, 255];
    if a0 > a1 {
        for i in 1..7 {
            values[i as usize + 1] = between(7, i);
        }
    } else {
        for i in 1..5 {
            values[i as usize + 1] = between(5, i);
        }
    }
    let mut bits = [0u8; 8];
    bits[..6].copy_from_slice(&block[2..8]);
    let bits = u64::from_le_bytes(bits);
    std::array::from_fn(|i| values[(bits >> (3 * i) & 7) as usize] as u8)
}

/// The 16 alpha values of a DXT2 or DXT3 alpha block: 4 bits each, the
/// low ones first.
fn explicit_alpha(block: &[u8]) -> [u8; 16] {
    std::array::from_fn(|i| (block[i / 2] >> (4 * (i % 2)) & 0xF) * 17)
}

/// The 16 pixels of a block, row by row.
pub fn decode_block(format: Format, rules: Rules, block: &[u8]) -> [Rgba; 16] {
    let (alpha, colour) = match format {
        Format::Dxt1 => (None, &block[..8]),
        Format::Dxt2 | Format::Dxt3 => (Some(explicit_alpha(block)), &block[8..16]),
        Format::Dxt4 | Format::Dxt5 => (Some(interpolated_alpha(block, rules)), &block[8..16]),
    };
    let colours = palette(colour, format, rules);
    let codes = u32::from_le_bytes([colour[4], colour[5], colour[6], colour[7]]);
    let clamp = format.premultiplied() && rules == Rules::DirectDraw;
    std::array::from_fn(|i| {
        let mut p = colours[(codes >> (2 * i) & 3) as usize];
        if let Some(alpha) = &alpha {
            p[3] = alpha[i];
            if clamp {
                for c in &mut p[..3] {
                    *c = (*c).min(alpha[i]);
                }
            }
        }
        p
    })
}

/// The pixels of the rectangle (`left`, `top`) to (`right`, `bottom`)
/// of a surface `width` pixels wide whose blocks are `data`, row by row.
/// The rectangle may start and end inside blocks; a block `data` doesn't
/// hold decodes as transparent black.
pub fn decode_rect(
    format: Format,
    rules: Rules,
    data: &[u8],
    width: u32,
    [left, top, right, bottom]: [u32; 4],
) -> Vec<Rgba> {
    if left >= right || top >= bottom {
        return Vec::new();
    }
    let bytes = format.block_bytes();
    let row_bytes = format.row_bytes(width);
    let (first, last) = ((left / 4) as usize, ((right - 1) / 4) as usize);
    let w = (right - left) as usize;
    let mut out = vec![[0u8; 4]; w * (bottom - top) as usize];
    for by in top / 4..=(bottom - 1) / 4 {
        let rows = (by * 4).max(top)..(by * 4 + 4).min(bottom);
        for bx in first..=last {
            let at = by as usize * row_bytes + bx * bytes;
            let Some(block) = data.get(at..at + bytes) else {
                continue;
            };
            let pixels = decode_block(format, rules, block);
            let columns = (bx as u32 * 4).max(left)..(bx as u32 * 4 + 4).min(right);
            for y in rows.clone() {
                for x in columns.clone() {
                    let to = (y - top) as usize * w + (x - left) as usize;
                    out[to] = pixels[(y % 4 * 4 + x % 4) as usize];
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A DXT1 block: its two colours and its 16 two-bit codes, the first
    /// pixel's lowest.
    fn colour_block(c0: u16, c1: u16, codes: u32) -> [u8; 8] {
        let mut b = [0u8; 8];
        b[..2].copy_from_slice(&c0.to_le_bytes());
        b[2..4].copy_from_slice(&c1.to_le_bytes());
        b[4..].copy_from_slice(&codes.to_le_bytes());
        b
    }

    /// Codes 0, 1, 2, 3 in each row.
    const CODES: u32 = 0xE4E4_E4E4;

    #[test]
    fn colours_widen_by_repeating_their_top_bits() {
        assert_eq!(widen(0xFFFF), [255, 255, 255]);
        assert_eq!(widen(0x0843), [8, 8, 24]);
        assert_eq!(widen(0x8410), [132, 130, 132]);
    }

    /// Hand-decoded: 0xFFE0 is (255, 255, 0) and 0x0843 is (8, 8, 24);
    /// a third of the way is (2 * 255 + 8 + 1) / 3 = 173 and
    /// (255 + 2 * 8 + 1) / 3 = 90, and (0 + 2 * 24 + 1) / 3 = 16.
    #[test]
    fn a_four_colour_block_has_the_colours_a_third_and_two_thirds_between() {
        let block = colour_block(0xFFE0, 0x0843, CODES);
        for rules in [Rules::S3tc, Rules::DirectDraw] {
            let p = decode_block(Format::Dxt1, rules, &block);
            assert_eq!(
                &p[..4],
                &[
                    [255, 255, 0, 255],
                    [8, 8, 24, 255],
                    [173, 173, 8, 255],
                    [90, 90, 16, 255]
                ]
            );
            assert_eq!(&p[4..8], &p[..4]);
        }
    }

    /// 0x001F is blue and 0xF800 red: half way is 127 of each (255 / 2
    /// rounded down), and code 3 is transparent black.
    #[test]
    fn a_dxt1_block_whose_first_colour_isnt_greater_has_three_and_transparent_black() {
        let block = colour_block(0x001F, 0xF800, CODES);
        let p = decode_block(Format::Dxt1, Rules::DirectDraw, &block);
        assert_eq!(
            &p[..4],
            &[
                [0, 0, 255, 255],
                [255, 0, 0, 255],
                [127, 0, 127, 255],
                [0, 0, 0, 0]
            ]
        );
        // Equal colours are the three-colour kind too.
        let p = decode_block(
            Format::Dxt1,
            Rules::S3tc,
            &colour_block(0x1234, 0x1234, CODES),
        );
        assert_eq!(p[3], [0, 0, 0, 0]);
    }

    fn dxt3_block(alpha: [u8; 8], colour: [u8; 8]) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[..8].copy_from_slice(&alpha);
        b[8..].copy_from_slice(&colour);
        b
    }

    #[test]
    fn dxt3_alpha_is_four_bits_a_pixel_low_nibble_first() {
        let alpha = [0x10, 0x32, 0x54, 0x76, 0x98, 0xBA, 0xDC, 0xFE];
        let block = dxt3_block(alpha, colour_block(0xFFE0, 0x0843, CODES));
        let p = decode_block(Format::Dxt3, Rules::DirectDraw, &block);
        let alphas: Vec<u8> = p.iter().map(|p| p[3]).collect();
        let expected: Vec<u8> = (0..16).map(|i| i * 17).collect();
        assert_eq!(alphas, expected);
        assert_eq!(p[2], [173, 173, 8, 34]);
    }

    /// DirectDraw decodes a DXT3 block whose first colour isn't greater
    /// as DXT1 would, but with the alpha block's alpha on black; the
    /// specification has four colours.
    #[test]
    fn a_three_colour_dxt3_block_differs_between_the_rules() {
        let block = dxt3_block([0xFF; 8], colour_block(0x001F, 0xF800, CODES));
        let p = decode_block(Format::Dxt3, Rules::DirectDraw, &block);
        assert_eq!(&p[2..4], &[[127, 0, 127, 255], [0, 0, 0, 255]]);
        // (2 * 0 + 255 + 1) / 3 = 85, (2 * 255 + 0 + 1) / 3 = 170.
        let p = decode_block(Format::Dxt3, Rules::S3tc, &block);
        assert_eq!(&p[2..4], &[[85, 0, 170, 255], [170, 0, 85, 255]]);
    }

    /// Codes 0 to 7 for the first eight pixels and again for the rest.
    fn dxt5_block(a0: u8, a1: u8) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0] = a0;
        b[1] = a1;
        b[2..8].copy_from_slice(&[0x88, 0xC6, 0xFA, 0x88, 0xC6, 0xFA]);
        b[8..].copy_from_slice(&colour_block(0xFFFF, 0x0000, 0));
        b
    }

    fn alphas(format: Format, rules: Rules, block: &[u8]) -> Vec<u8> {
        decode_block(format, rules, block)[..8]
            .iter()
            .map(|p| p[3])
            .collect()
    }

    /// Eight values between 201 and 37: (6 * 201 + 37 + 3) / 7 = 178 is
    /// code 2 in the specification and code 7 under DirectDraw, whose
    /// code 2 is (201 + 6 * 37 + 3) / 7 = 60.
    #[test]
    fn dxt5_has_eight_alpha_values_when_the_first_is_greater() {
        let block = dxt5_block(201, 37);
        assert_eq!(
            alphas(Format::Dxt5, Rules::S3tc, &block),
            [201, 37, 178, 154, 131, 107, 84, 60]
        );
        assert_eq!(
            alphas(Format::Dxt5, Rules::DirectDraw, &block),
            [201, 37, 60, 84, 107, 131, 154, 178]
        );
    }

    /// Six values between 37 and 201, then 0 and 255: code 2 is
    /// (4 * 37 + 201 + 2) / 5 = 70 in the specification, and
    /// (37 + 4 * 201 + 2) / 5 = 168 under DirectDraw.
    #[test]
    fn dxt5_has_six_alpha_values_and_0_and_255_otherwise() {
        let block = dxt5_block(37, 201);
        assert_eq!(
            alphas(Format::Dxt5, Rules::S3tc, &block),
            [37, 201, 70, 103, 135, 168, 0, 255]
        );
        assert_eq!(
            alphas(Format::Dxt5, Rules::DirectDraw, &block),
            [37, 201, 168, 135, 103, 70, 0, 255]
        );
        let equal = dxt5_block(90, 90);
        assert_eq!(
            alphas(Format::Dxt5, Rules::DirectDraw, &equal),
            [90, 90, 90, 90, 90, 90, 0, 255]
        );
    }

    /// DirectDraw keeps DXT2's and DXT4's colours at most their alpha:
    /// premultiplied colours can't be more.
    #[test]
    fn premultiplied_colours_stay_at_most_the_alpha_under_directdraw() {
        let alpha = [0x10, 0x32, 0x54, 0x76, 0x98, 0xBA, 0xDC, 0xFE];
        let block = dxt3_block(alpha, colour_block(0xFFE0, 0x0843, CODES));
        let p = decode_block(Format::Dxt2, Rules::DirectDraw, &block);
        assert_eq!(
            &p[..4],
            &[
                [0, 0, 0, 0],
                [8, 8, 17, 17],
                [34, 34, 8, 34],
                [51, 51, 16, 51]
            ]
        );
        let p = decode_block(Format::Dxt2, Rules::S3tc, &block);
        assert_eq!(p[1], [8, 8, 24, 17]);
    }

    #[test]
    fn a_rectangle_can_cut_blocks() {
        // Four blocks of 8x8 pixels, each of one colour (code 0).
        let mut data = Vec::new();
        for c in [0xF800u16, 0x07E0, 0x001F, 0xFFFF] {
            data.extend_from_slice(&colour_block(c, 0, 0));
        }
        let p = decode_rect(Format::Dxt1, Rules::DirectDraw, &data, 8, [2, 3, 7, 6]);
        assert_eq!(p.len(), 5 * 3);
        let red = [255, 0, 0, 255];
        let green = [0, 255, 0, 255];
        let blue = [0, 0, 255, 255];
        let white = [255, 255, 255, 255];
        assert_eq!(&p[..5], &[red, red, green, green, green]);
        assert_eq!(&p[5..10], &[blue, blue, white, white, white]);
        assert_eq!(&p[10..], &[blue, blue, white, white, white]);
    }

    #[test]
    fn sizes_are_whole_blocks() {
        assert_eq!(Format::Dxt1.size(8, 8), 32);
        assert_eq!(Format::Dxt5.size(8, 8), 64);
        assert_eq!(Format::Dxt1.size(1, 1), 8);
        assert_eq!(Format::Dxt3.size(6, 2), 32);
        assert_eq!(
            Format::from_fourcc(u32::from_le_bytes(*b"DXT4")),
            Some(Format::Dxt4)
        );
        assert_eq!(Format::from_fourcc(u32::from_le_bytes(*b"UYVY")), None);
    }
}
