//! Microsoft Video 1 (CRAM, Windows' `msvidc32.dll`), written from the
//! format's public description. A picture is cut into blocks of 4 by 4
//! pixels, coded from the bottom left: left to right along a row of
//! blocks, the rows from the bottom up, as a bottom-up DIB stores them.
//! A frame is a list of codes, each two bytes A and B, then the colours
//! the code needs:
//!
//! - B from 0x84 to 0x87: ((B - 0x84) << 8 | A) blocks are skipped, and
//!   keep the pixels they had.
//! - B under 0x80: the 16 bits B << 8 | A choose between two colours for
//!   each pixel, or between two of eight, a pair for each quarter.
//! - Any other: the whole block is one colour.
//!
//! The 8-bit form codes palette indexes, a byte each: after a B under
//! 0x80 come two colours, after a B of 0x90 or more eight, and a block of
//! one colour is A. The 16-bit form codes RGB555 colours, a word each:
//! after a B under 0x80 come two, and six more when the first has its
//! top bit set; a block of one colour is the word B << 8 | A.
//!
//! In a block, bit n of the 16 is the pixel n % 4 from the left in row
//! n / 4 from the bottom. A set bit takes the first colour of its pair,
//! a clear one the second. With eight colours the bottom left quarter
//! has the first pair, the bottom right the second, the top left the
//! third and the top right the fourth.

/// What a frame's colours are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    /// Palette indexes, of an 8-bit stream.
    Indexed,
    /// RGB555, of a 16-bit stream.
    Rgb555,
}

impl Depth {
    /// The depth of a stream of `bits` bits a pixel, which only 8 and 16
    /// are.
    pub fn of(bits: u16) -> Option<Depth> {
        match bits {
            8 => Some(Depth::Indexed),
            16 => Some(Depth::Rgb555),
            _ => None,
        }
    }
}

/// Reads the codes of a frame.
struct Codes<'a> {
    frame: &'a [u8],
    at: usize,
}

impl Codes<'_> {
    fn byte(&mut self) -> Option<u8> {
        let b = *self.frame.get(self.at)?;
        self.at += 1;
        Some(b)
    }

    fn word(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes([self.byte()?, self.byte()?]))
    }

    /// The next colour: a byte or a word, without the word's top bit.
    fn colour(&mut self, depth: Depth) -> Option<u16> {
        match depth {
            Depth::Indexed => self.byte().map(u16::from),
            Depth::Rgb555 => self.word().map(|w| w & 0x7FFF),
        }
    }
}

/// A block's 16 pixels, by `flags`, from the pairs in `colours`: one
/// pair, or four, one for each quarter.
fn pixels(flags: u16, colours: &[u16]) -> [u16; 16] {
    std::array::from_fn(|n| {
        let (x, y) = (n % 4, n / 4);
        let pair = if colours.len() == 8 {
            2 * (x / 2 + 2 * (y / 2))
        } else {
            0
        };
        colours[pair + (flags >> n & 1 == 0) as usize]
    })
}

/// Decodes `frame`, of a picture `width` by `height`, block by block:
/// `block(x, y, pixels)` gets each block the frame codes, at `x` blocks
/// from the left and `y` from the bottom, with its 16 colours in rows
/// from the bottom. Blocks the frame skips are not passed. A picture
/// whose size isn't a multiple of 4 has its last, partial, column and row
/// of blocks left out, as the encoder leaves them.
///
/// An error for a frame that ends before its last block: the blocks
/// before have been passed.
pub fn decode(
    frame: &[u8],
    depth: Depth,
    width: u32,
    height: u32,
    mut block: impl FnMut(usize, usize, &[u16; 16]),
) -> Result<(), String> {
    let (across, up) = (width as usize / 4, height as usize / 4);
    let total = across * up;
    let mut codes = Codes { frame, at: 0 };
    let short = |n: usize| format!("the frame ends at block {n} of {total}");
    let mut n = 0;
    while n < total {
        let (Some(a), Some(b)) = (codes.byte(), codes.byte()) else {
            return Err(short(n));
        };
        let (x, y) = (n % across, n / across);
        if b & 0xFC == 0x84 {
            n += ((b as usize - 0x84) << 8) | a as usize;
            continue;
        }
        let flags = u16::from_le_bytes([a, b]);
        let one = match depth {
            Depth::Indexed => (0x80..0x90).contains(&b).then_some(a as u16),
            Depth::Rgb555 => (b >= 0x80).then_some(flags & 0x7FFF),
        };
        if let Some(colour) = one {
            block(x, y, &[colour; 16]);
            n += 1;
            continue;
        }
        // Eight colours: B's 0x90, or the first word's top bit.
        let eight = match depth {
            Depth::Indexed => b >= 0x90,
            Depth::Rgb555 => frame.get(codes.at + 1).is_some_and(|&hi| hi & 0x80 != 0),
        };
        let used = if eight { 8 } else { 2 };
        let mut colours = [0u16; 8];
        for c in &mut colours[..used] {
            *c = codes.colour(depth).ok_or_else(|| short(n))?;
        }
        block(x, y, &pixels(flags, &colours[..used]));
        n += 1;
    }
    Ok(())
}

/// What a picture is decoded to: the layout of a DIB's pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// 8 bits, the frame's palette indexes as they are.
    Indexes,
    /// 16 bits, RGB555, or RGB565 when `green6`.
    Rgb16 { green6: bool },
    /// 24 bits, blue, green and red.
    Rgb24,
    /// 32 bits, blue, green, red and a zero.
    Rgb32,
}

impl Output {
    pub fn bits(self) -> u16 {
        match self {
            Output::Indexes => 8,
            Output::Rgb16 { .. } => 16,
            Output::Rgb24 => 24,
            Output::Rgb32 => 32,
        }
    }

    /// Whether a stream of `depth` can be decoded to this: anything but
    /// an RGB555 one to palette indexes.
    pub fn takes(self, depth: Depth) -> bool {
        self != Output::Indexes || depth == Depth::Indexed
    }
}

/// A colour of `depth` as blue, green and red: an index through
/// `palette` (black past its end), RGB555 with each 5 bits widened to 8.
fn bgr(colour: u16, depth: Depth, palette: &[[u8; 3]]) -> [u8; 3] {
    match depth {
        Depth::Indexed => palette.get(colour as usize).copied().unwrap_or_default(),
        Depth::Rgb555 => {
            let five = |shift: u16| {
                let c = (colour >> shift & 0x1F) as u8;
                c << 3 | c >> 2
            };
            [five(0), five(5), five(10)]
        }
    }
}

/// A DIB that a frame is decoded onto, in place: the blocks a frame
/// skips keep what the DIB had.
pub struct Target<'a> {
    pub pixels: &'a mut [u8],
    pub width: u32,
    pub height: u32,
    pub output: Output,
    /// Whether its rows run from the top (a negative biHeight).
    pub top_down: bool,
}

impl Target<'_> {
    /// The bytes of a row of the DIB, padded to 4 bytes.
    fn stride(&self) -> usize {
        (self.width as usize * self.output.bits() as usize).div_ceil(32) * 4
    }

    /// Decodes `frame` of `depth` onto the DIB, with `palette` the
    /// colours of an 8-bit stream as blue, green and red. An error for a
    /// DIB too small for its size, for a frame cut short (the blocks
    /// before its end are drawn), and for an RGB555 stream to palette
    /// indexes.
    pub fn decode(
        &mut self,
        frame: &[u8],
        depth: Depth,
        palette: &[[u8; 3]],
    ) -> Result<(), String> {
        if !self.output.takes(depth) {
            return Err("a 16-bit stream has no palette indexes".to_string());
        }
        let stride = self.stride();
        if self.pixels.len() < stride * self.height as usize {
            return Err("the picture's buffer is too small".to_string());
        }
        let (output, height, top_down) = (self.output, self.height as usize, self.top_down);
        let bytes = output.bits() as usize / 8;
        let pixels = &mut *self.pixels;
        decode(frame, depth, self.width, self.height, |bx, by, colours| {
            for (n, &colour) in colours.iter().enumerate() {
                let (x, y) = (4 * bx + n % 4, 4 * by + n / 4);
                let row = if top_down { height - 1 - y } else { y };
                let at = row * stride + x * bytes;
                let out = &mut pixels[at..at + bytes];
                match output {
                    Output::Indexes => out[0] = colour as u8,
                    Output::Rgb16 { green6 } => {
                        let word = if depth == Depth::Rgb555 && !green6 {
                            colour
                        } else {
                            let [b, g, r] = bgr(colour, depth, palette).map(u16::from);
                            if green6 {
                                (r >> 3) << 11 | (g >> 2) << 5 | b >> 3
                            } else {
                                (r >> 3) << 10 | (g >> 3) << 5 | b >> 3
                            }
                        };
                        out.copy_from_slice(&word.to_le_bytes());
                    }
                    Output::Rgb24 | Output::Rgb32 => {
                        out[..3].copy_from_slice(&bgr(colour, depth, palette));
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes `frame` onto an 8-bit DIB of `width` by `height` filled
    /// with `fill`, and gives its rows from the bottom.
    fn indexes(frame: &[u8], width: u32, height: u32, fill: u8) -> Vec<Vec<u8>> {
        let stride = (width as usize).div_ceil(4) * 4;
        let mut pixels = vec![fill; stride * height as usize];
        let mut target = Target {
            pixels: &mut pixels,
            width,
            height,
            output: Output::Indexes,
            top_down: false,
        };
        target.decode(frame, Depth::Indexed, &[]).unwrap();
        pixels
            .chunks(stride)
            .map(|row| row[..width as usize].to_vec())
            .collect()
    }

    #[test]
    fn an_8_bit_block_of_two_colours_takes_the_first_for_a_set_bit() {
        // Flags 0x0F21: bits 0, 5, 8, 9, 10 and 11 set.
        let rows = indexes(&[0x21, 0x0F, 7, 9], 4, 4, 0);
        assert_eq!(
            rows,
            [[7, 9, 9, 9], [9, 7, 9, 9], [7, 7, 7, 7], [9, 9, 9, 9]]
        );
    }

    #[test]
    fn an_8_bit_block_of_eight_colours_has_a_pair_for_each_quarter() {
        // B of 0x90 or more: eight colours. Flags 0x90F0 set bits 4 to
        // 7, the second row from the bottom, and bits 12 and 15, the top
        // row's first and last pixels.
        let frame = [0xF0, 0x90, 1, 2, 3, 4, 5, 6, 7, 8];
        let rows = indexes(&frame, 4, 4, 0);
        assert_eq!(
            rows,
            [[2, 2, 4, 4], [1, 1, 3, 3], [6, 6, 8, 8], [5, 6, 8, 7]]
        );
    }

    #[test]
    fn an_8_bit_block_of_one_colour_and_skips_keep_the_pixels_there() {
        // A picture of 3 by 2 blocks, 12 by 8, filled with 0xEE: the
        // first block one colour (B 0x80, A 0x11), then a skip of 3
        // (B 0x84, A 3), a block of one colour (B 0x8F), and a skip of
        // the last.
        let frame = [0x11, 0x80, 3, 0x84, 0x22, 0x8F, 1, 0x84];
        let rows = indexes(&frame, 12, 8, 0xEE);
        for (y, row) in rows.iter().enumerate() {
            let want: Vec<u8> = (0..12)
                .map(|x| match (x / 4, y / 4) {
                    (0, 0) => 0x11,
                    (1, 1) => 0x22,
                    _ => 0xEE,
                })
                .collect();
            assert_eq!(row, &want, "row {y}");
        }
    }

    #[test]
    fn a_frame_cut_short_keeps_the_blocks_before_its_end() {
        let stride = 8;
        let mut pixels = vec![0; stride * 4];
        let mut target = Target {
            pixels: &mut pixels,
            width: 8,
            height: 4,
            output: Output::Indexes,
            top_down: false,
        };
        // One block of colour 5, then a two-colour block without colours.
        let e = target.decode(&[5, 0x80, 0, 0], Depth::Indexed, &[]);
        assert_eq!(e, Err("the frame ends at block 1 of 2".to_string()));
        assert_eq!(&pixels[..8], &[5, 5, 5, 5, 0, 0, 0, 0]);
    }

    /// Decodes a 16-bit `frame` of 4 by 4 to `output`, rows from the
    /// bottom.
    fn rgb(frame: &[u8], output: Output) -> Vec<u8> {
        let mut pixels = vec![0; 4 * output.bits() as usize / 2];
        let mut target = Target {
            pixels: &mut pixels,
            width: 4,
            height: 4,
            output,
            top_down: false,
        };
        target.decode(frame, Depth::Rgb555, &[]).unwrap();
        pixels
    }

    #[test]
    fn a_16_bit_block_of_one_colour_is_its_word_without_the_top_bit() {
        // 0xFC00 | 0x8000: red at full, the word's top bit set.
        let pixels = rgb(&[0x00, 0xFC], Output::Rgb16 { green6: false });
        assert!(pixels.chunks(2).all(|p| p == [0x00, 0x7C]), "{pixels:?}");
        let pixels = rgb(&[0x00, 0xFC], Output::Rgb24);
        assert!(pixels.chunks(3).all(|p| p == [0, 0, 0xFF]), "{pixels:?}");
    }

    #[test]
    fn a_16_bit_block_of_eight_colours_is_marked_by_the_first_colours_top_bit() {
        // Flags 0x7FFF: each pixel but the top right one the first colour
        // of its quarter's pair. The first colour has the top bit, so
        // there are eight words.
        let colours: [u16; 8] = [0x801F, 0, 0x03E0, 0, 0x7C00, 0, 0x7FFF, 0];
        let mut frame = vec![0xFF, 0x7F];
        frame.extend(colours.iter().flat_map(|c| c.to_le_bytes()));
        let pixels = rgb(&frame, Output::Rgb32);
        let px = |x: usize, y: usize| &pixels[(y * 4 + x) * 4..][..4];
        assert_eq!(px(0, 0), [0xFF, 0, 0, 0]); // blue, bottom left
        assert_eq!(px(3, 1), [0, 0xFF, 0, 0]); // green, bottom right
        assert_eq!(px(1, 2), [0, 0, 0xFF, 0]); // red, top left
        assert_eq!(px(2, 3), [0xFF, 0xFF, 0xFF, 0]); // white, top right
        // Without the top bit, two colours: blue for the set bits.
        let pixels = rgb(&[0xFF, 0x00, 0x1F, 0x00, 0x00, 0x7C], Output::Rgb24);
        let row = |y: usize| &pixels[y * 12..][..12];
        assert_eq!(row(0), [0xFF, 0, 0].repeat(4));
        assert_eq!(row(3), [0, 0, 0xFF].repeat(4));
    }

    #[test]
    fn indexes_go_through_the_palette_to_rgb_and_rows_can_run_from_the_top() {
        let palette = [[0, 0, 0], [0x10, 0x20, 0x30], [0xF8, 0xFC, 0xF8]];
        // Two colours, 1 for the bottom row, 2 for the rest.
        let frame = [0x0F, 0x00, 1, 2];
        let mut pixels = vec![0; 4 * 4 * 2];
        let mut target = Target {
            pixels: &mut pixels,
            width: 4,
            height: 4,
            output: Output::Rgb16 { green6: true },
            top_down: true,
        };
        target.decode(&frame, Depth::Indexed, &palette).unwrap();
        // RGB565 of 0x30, 0x20, 0x10, the bottom row: the last in memory.
        let bottom = (0x30u16 >> 3) << 11 | (0x20 >> 2) << 5 | 0x10 >> 3;
        let top = 0xFFFFu16;
        assert_eq!(&pixels[24..], bottom.to_le_bytes().repeat(4));
        assert_eq!(&pixels[..8], top.to_le_bytes().repeat(4));
    }

    #[test]
    fn a_16_bit_stream_has_no_8_bit_output() {
        let mut pixels = vec![0; 16];
        let mut target = Target {
            pixels: &mut pixels,
            width: 4,
            height: 4,
            output: Output::Indexes,
            top_down: false,
        };
        assert!(target.decode(&[0, 0x80], Depth::Rgb555, &[]).is_err());
    }
}
