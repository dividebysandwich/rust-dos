//! The 2D microcode Rendition's Windows 95 driver loads (`v10002d.uc`,
//! which xf86-video-rendition also uses): the commands it takes from the
//! FIFO, carried out as it would.
//!
//! The driver (`v1000.dll`, which builds the commands in DMA buffers)
//! starts a small loader first, which takes a command, the context store
//! area, a word and the microcode's entry from the FIFO and runs it. Command
//! numbers are xf86-video-rendition's `cmd2d.h`; the layouts are what the
//! driver builds. Positions and sizes are packed x (or width) high, y (or
//! height) low; colours are the pixel's value repeated across the word.
//! Raster operations are 4 bits: the result for (source or pattern,
//! destination) = (1,1) in bit 3, (1,0) bit 2, (0,1) bit 1, (0,0) bit 0,
//! applied to each bit of a pixel.

/// The commands, by number.
pub mod op {
    pub const NOP: u32 = 0x00;
    /// A rectangle in the scan's colour (BSCAN_SOLID).
    pub const RECT: u32 = 0x01;
    /// Answer through the output FIFO.
    pub const RESPOND: u32 = 0x02;
    /// An 8x8 two-colour brush into a slot: its rows, one a byte, the
    /// first in the top byte of the first word.
    pub const LOAD_MONO_BRUSH: u32 = 0x04;
    pub const SYNC_AND_RESPOND: u32 = 0x08;
    /// A copy within the surface: rop, source, size, destination.
    pub const SCREEN_BLT: u32 = 0x0C;
    /// A one-bit image onto the surface: 1 bits in one colour, 0 bits in
    /// the other.
    pub const BITBLT_MS_MONO: u32 = 0x16;
    /// The surface drawn on: size, depth and format, base, bytes a line,
    /// stride code.
    /// The end of a scan.
    pub const END_SCAN: u32 = 0x12;
    /// A scan of rectangles in a colour, with a raster operation.
    pub const BEGIN_SCAN_SOLID: u32 = 0x13;
    pub const SETUP: u32 = 0x20;
    pub const SET_PIXEL: u32 = 0x22;
    /// Text: one-bit glyphs at their positions in a colour, clipped, over
    /// an opaque rectangle if bit 16 is set.
    pub const DRAW_GLYPHS: u32 = 0x23;
    pub const SET_CLIPPING: u32 = 0x24;
    /// A copy between two surfaces of memory.
    pub const DD_SCREEN_BLT: u32 = 0x25;
    pub const RECT_SOLID_ROP: u32 = 0x29;
    pub const RECT_MONO_BRUSH_ROP: u32 = 0x2A;
    pub const RECT_SOLID: u32 = 0x30;
}

/// How many words the command at the start of `words` takes, if they hold
/// enough of it to tell: `Some(None)` when more words are needed, `None`
/// when the command isn't one this knows. `bpp` is the surface's depth.
pub fn length(words: &[u32], bpp: u32) -> Option<Option<usize>> {
    let need = |n: usize| if words.len() >= n { Some(n) } else { None };
    let word = |i: usize| words.get(i).copied();
    Some(match words[0] & 0xFFFF {
        op::NOP | op::RESPOND | op::SYNC_AND_RESPOND | op::END_SCAN => Some(1),
        // Scans with brushes.
        0x0D | 0x0E => Some(2),
        op::RECT | op::LOAD_MONO_BRUSH | op::SET_PIXEL | op::SET_CLIPPING | op::BEGIN_SCAN_SOLID | 0x10 | 0x11 => {
            Some(3)
        }
        // Spans of a scan line: a count, then the spans.
        0x0F => word(1).map(|n| 2 + n as usize),
        0x03 | 0x18 => Some(5),
        0x14 => Some(8),
        0x17 => Some(10),
        op::RECT_SOLID_ROP | op::RECT_SOLID => Some(4),
        op::SCREEN_BLT => Some(5),
        op::SETUP => Some(6),
        op::DD_SCREEN_BLT => Some(9),
        // A colour brush: 8x8 pixels.
        0x05 => Some(1 + 2 * bpp as usize),
        op::RECT_MONO_BRUSH_ROP => word(1).map(|w| if w & 0xFFFF == 1 { 7 } else { 6 }),
        op::BITBLT_MS_MONO => word(5).map(|wh| 6 + mono_rows(wh).0 * mono_rows(wh).1),
        op::DRAW_GLYPHS => glyphs_length(words),
        _ => return None,
    }
    .and_then(need))
}

/// A one-bit image's lines and words a line, for its size: whole bytes a
/// line, padded to words.
fn mono_rows(wh: u32) -> (usize, usize) {
    let (w, h) = ((wh >> 16) as usize, (wh & 0xFFFF) as usize);
    (h, w.div_ceil(8).div_ceil(4))
}

/// The words of a glyph record whose first word is `header`: its size
/// and position, then its lines of whole bytes packed into words.
fn glyph_words(header: u32) -> usize {
    let (w, h) = ((header >> 16) as usize, (header & 0xFFFF) as usize);
    2 + (h * w.div_ceil(8)).div_ceil(4)
}

/// DRAW_GLYPHS's words: the header (with an opaque rectangle and its
/// colour first if bit 16 is set), the clipping rectangle, the number of
/// characters, then if there are any the colour and the glyph records up
/// to a zero word.
fn glyphs_length(words: &[u32]) -> Option<usize> {
    let mut at = 1 + if words[0] & 0x1_0000 != 0 { 3 } else { 0 };
    if *words.get(at + 2)? == 0 {
        return Some(at + 3);
    }
    at += 4;
    loop {
        let header = *words.get(at)?;
        if header == 0 {
            return Some(at + 1);
        }
        at += glyph_words(header);
    }
}

/// A 4-bit raster operation on source (or pattern) `s` and destination
/// `d`, bit by bit.
pub fn rop(code: u32, s: u32, d: u32) -> u32 {
    let mut r = 0;
    if code & 8 != 0 {
        r |= s & d;
    }
    if code & 4 != 0 {
        r |= s & !d;
    }
    if code & 2 != 0 {
        r |= !s & d;
    }
    if code & 1 != 0 {
        r |= !s & !d;
    }
    r
}

/// A rectangle, the far edges excluded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

crate::state_fields!(Rect { x0, y0, x1, y1 });

/// A packed position or size: x (or width) high, y (or height) low.
fn xy(word: u32) -> (i32, i32) {
    ((word >> 16) as i16 as i32, word as i16 as i32)
}

fn pack(x: i32, y: i32) -> u32 {
    (x as u32) << 16 | y as u32 & 0xFFFF
}

#[derive(Clone, Debug)]
pub struct Engine {
    /// The loader's four words are still to come.
    pub loader: bool,
    pub base: u32,
    pub pitch: u32,
    /// Bytes a pixel.
    pub bytes: u32,
    pub width: u32,
    pub height: u32,
    pub clip: Rect,
    /// Two-colour brushes by slot: 8 rows of 8 pixels, the leftmost in a
    /// row's top bit.
    pub mono_brushes: Vec<u64>,
    /// The scan's raster operation and colour.
    pub scan_rop: u32,
    pub scan_colour: u32,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            loader: false,
            base: 0,
            pitch: 0,
            bytes: 1,
            width: 0,
            height: 0,
            clip: Rect::default(),
            mono_brushes: vec![0; 256],
            scan_rop: 0xC,
            scan_colour: 0,
        }
    }
}

crate::state_fields!(Engine { loader, base, pitch, bytes, width, height, clip, mono_brushes, scan_rop, scan_colour });

impl Engine {
    /// Bits a pixel, for commands whose length depends on it.
    pub fn bpp(&self) -> u32 {
        self.bytes * 8
    }

    /// The pixel at (`x`, `y`) of the surface at `base`, `pitch` bytes a
    /// line.
    fn at(&self, base: u32, pitch: u32, x: i32, y: i32) -> usize {
        (base as i64 + y as i64 * pitch as i64 + x as i64 * self.bytes as i64) as usize
    }

    fn get(&self, vram: &[u8], at: usize) -> u32 {
        let n = vram.len();
        (0..self.bytes as usize).map(|i| (vram[(at + i) % n] as u32) << (8 * i)).sum()
    }

    fn set(&self, vram: &mut [u8], at: usize, value: u32) {
        let n = vram.len();
        for i in 0..self.bytes as usize {
            vram[(at + i) % n] = (value >> (8 * i)) as u8;
        }
    }

    fn mask(&self) -> u32 {
        if self.bytes >= 4 { u32::MAX } else { (1 << (8 * self.bytes)) - 1 }
    }

    /// Whether (`x`, `y`) is inside the clipping rectangle and the surface.
    fn visible(&self, x: i32, y: i32) -> bool {
        let c = &self.clip;
        x >= c.x0 && x < c.x1 && y >= c.y0 && y < c.y1 && x >= 0 && y >= 0
    }

    /// `value` combined with the surface's pixel at (`x`, `y`) by `code`,
    /// if it is visible.
    fn plot(&self, vram: &mut [u8], x: i32, y: i32, code: u32, value: u32) {
        if !self.visible(x, y) {
            return;
        }
        let at = self.at(self.base, self.pitch, x, y);
        let d = self.get(vram, at);
        self.set(vram, at, rop(code, value, d) & self.mask());
    }

    /// A rectangle at `pos` of size `size`, each pixel from `pixel`.
    fn fill(&self, vram: &mut [u8], pos: u32, size: u32, code: u32, pixel: impl Fn(i32, i32) -> Option<u32>) {
        let ((x, y), (w, h)) = (xy(pos), xy(size));
        let c = &self.clip;
        for py in y.max(c.y0).max(0)..(y + h).min(c.y1) {
            for px in x.max(c.x0).max(0)..(x + w).min(c.x1) {
                if let Some(value) = pixel(px, py) {
                    self.plot(vram, px, py, code, value);
                }
            }
        }
    }

    /// The command `words`, on the card's memory `vram`; answers go to
    /// `output`. Whether it was one this carries out.
    pub fn run(&mut self, words: &[u32], vram: &mut [u8], output: &mut std::collections::VecDeque<u32>) -> bool {
        let rop_high = words[0] >> 16 & 0xF;
        match words[0] & 0xFFFF {
            op::NOP => {}
            op::RESPOND | op::SYNC_AND_RESPOND => output.push_back(0),
            op::SETUP => {
                let (w, h) = xy(words[1]);
                self.width = w as u32;
                self.height = h as u32;
                self.bytes = (words[2] >> 16).div_ceil(8).max(1);
                self.base = words[3];
                self.pitch = words[4];
                self.clip = Rect { x0: 0, y0: 0, x1: w, y1: h };
            }
            op::SET_CLIPPING => {
                let ((x0, y0), (x1, y1)) = (xy(words[1]), xy(words[2]));
                self.clip = Rect { x0, y0, x1, y1 };
            }
            op::RECT_SOLID => self.fill(vram, words[2], words[3], 0xC, |_, _| Some(words[1])),
            op::BEGIN_SCAN_SOLID => {
                self.scan_rop = words[1] & 0xF;
                self.scan_colour = words[2];
            }
            op::END_SCAN => {}
            op::RECT => {
                let colour = self.scan_colour;
                self.fill(vram, words[1], words[2], self.scan_rop, |_, _| Some(colour));
            }
            op::RECT_SOLID_ROP => self.fill(vram, words[2], words[3], rop_high, |_, _| Some(words[1])),
            op::SET_PIXEL => {
                let (x, y) = xy(words[2]);
                self.plot(vram, x, y, rop_high, words[1]);
            }
            op::LOAD_MONO_BRUSH => {
                let slot = (words[0] >> 16 & 0xFF) as usize;
                self.mono_brushes[slot] = (words[1] as u64) << 32 | words[2] as u64;
            }
            op::RECT_MONO_BRUSH_ROP => {
                let slot = (words[1] >> 16 & 0xFF) as usize;
                let pattern = self.mono_brushes[slot];
                let bit = |x: i32, y: i32| pattern >> (63 - ((y & 7) * 8 + (x & 7))) & 1 != 0;
                if words[1] & 0xFFFF == 1 {
                    let (one, zero) = (words[2], words[3]);
                    self.fill(vram, words[4], words[5], rop_high, |x, y| Some(if bit(x, y) { one } else { zero }));
                } else {
                    let colour = words[2];
                    self.fill(vram, words[3], words[4], rop_high, |x, y| bit(x, y).then_some(colour));
                }
            }
            op::BITBLT_MS_MONO => {
                let code = words[1] & 0xF;
                let (one, zero) = (words[2], words[3]);
                let ((x, y), (w, h)) = (xy(words[4]), xy(words[5]));
                let (_, per_row) = mono_rows(words[5]);
                let bits = &words[6..];
                for row in 0..h {
                    let line = &bits[row as usize * per_row..][..per_row];
                    for col in 0..w {
                        let byte = line[col as usize / 32].to_le_bytes()[(col as usize % 32) / 8];
                        let set = byte & (0x80 >> (col % 8)) != 0;
                        self.plot(vram, x + col, y + row, code, if set { one } else { zero });
                    }
                }
            }
            op::SCREEN_BLT => {
                let (base, pitch) = (self.base, self.pitch);
                self.blit(vram, words[1] & 0xF, (base, pitch, words[2]), words[3], (base, pitch, words[4]));
            }
            op::DD_SCREEN_BLT => {
                let strides = words[7];
                let src = (words[5], super::draw::stride(strides & 0xFF), words[2]);
                let dst = (words[6], super::draw::stride(strides >> 8 & 0xFF), words[4]);
                self.blit(vram, words[1] & 0xF, src, words[3], dst);
            }
            op::DRAW_GLYPHS => self.glyphs(words, vram),
            _ => return false,
        }
        true
    }

    /// A rectangle of size `size` copied from (base, pitch, position)
    /// `src` to `dst`, combined by `code`, through a copy so overlapping
    /// ones come out whole. The destination is clipped when it is the
    /// surface.
    fn blit(&self, vram: &mut [u8], code: u32, src: (u32, u32, u32), size: u32, dst: (u32, u32, u32)) {
        let (w, h) = xy(size);
        let ((sx, sy), (dx, dy)) = (xy(src.2), xy(dst.2));
        let on_surface = dst.0 == self.base;
        let mut pixels = Vec::with_capacity((w.max(0) * h.max(0)) as usize);
        for y in 0..h {
            for x in 0..w {
                pixels.push(self.get(vram, self.at(src.0, src.1, sx + x, sy + y)));
            }
        }
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (dx + x, dy + y);
                if on_surface && !self.visible(px, py) {
                    continue;
                }
                let at = self.at(dst.0, dst.1, px, py);
                let d = self.get(vram, at);
                self.set(vram, at, rop(code, pixels[(y * w + x) as usize], d) & self.mask());
            }
        }
    }

    /// DRAW_GLYPHS: the opaque rectangle filled if there is one, then each
    /// glyph's 1 bits in the colour, within the clipping rectangle the
    /// command gives instead of the one set. Its rectangles are Windows'
    /// RECTs read a doubleword at a time: y high, x low.
    fn glyphs(&mut self, words: &[u32], vram: &mut [u8]) {
        let rect = |tl: u32, br: u32| {
            let ((y0, x0), (y1, x1)) = (xy(tl), xy(br));
            Rect { x0, y0, x1, y1 }
        };
        let saved = self.clip;
        let mut at = if words[0] & 0x1_0000 != 0 { 4 } else { 1 };
        self.clip = rect(words[at], words[at + 1]);
        if at == 4 {
            let r = rect(words[1], words[2]);
            let (pos, size) = (pack(r.x0, r.y0), pack(r.x1 - r.x0, r.y1 - r.y0));
            self.fill(vram, pos, size, 0xC, |_, _| Some(words[3]));
        }
        if words[at + 2] != 0 {
            let colour = words[at + 3];
            at += 4;
            while words[at] != 0 {
                let ((w, h), (x, y)) = (xy(words[at]), xy(words[at + 1]));
                let bytes: Vec<u8> = words[at + 2..at + glyph_words(words[at])].iter().flat_map(|v| v.to_le_bytes()).collect();
                let per_row = (w as usize).div_ceil(8);
                for row in 0..h {
                    for col in 0..w {
                        let byte = bytes[row as usize * per_row + col as usize / 8];
                        if byte & (0x80 >> (col % 8)) != 0 {
                            self.plot(vram, x + col, y + row, 0xC, colour);
                        }
                    }
                }
                at += glyph_words(words[at]);
            }
        }
        self.clip = saved;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_operations_by_their_truth_tables() {
        assert_eq!(rop(0xC, 0b1100, 0b1010), 0b1100, "copy");
        assert_eq!(rop(0x8, 0b1100, 0b1010), 0b1000, "and");
        assert_eq!(rop(0xE, 0b1100, 0b1010), 0b1110, "or");
        assert_eq!(rop(0x6, 0b1100, 0b1010), 0b0110, "xor");
        assert_eq!(rop(0x5, 0, 0b1010) & 0xF, 0b0101, "not the destination");
    }

    // A text run as the driver sends one: the clipping rectangle, 7
    // characters, white, two glyphs (a space has none), the end.
    #[test]
    fn a_glyph_run_ends_at_its_zero_word() {
        let run = [
            0x23, 0x0026_005B, 0x0033_0086, 7, 0xFFFF_FFFF, 0x0005_0009, 0x005F_0028, 1, 2, 3, 0x0001_0006, 0x0066_002B,
            4, 5, 0, 0x24,
        ];
        assert_eq!(length(&run, 16), Some(Some(15)));
        assert_eq!(length(&run[..10], 16), Some(None));
        // Only the opaque rectangle: no characters.
        assert_eq!(length(&[0x1_0023, 1, 2, 3, 4, 5, 0, 0x30], 16), Some(Some(7)));
    }

    #[test]
    fn a_mono_image_takes_whole_bytes_a_line_padded_to_words() {
        // 16x16: two bytes a line, a word each.
        let mut words = vec![0x1_0016, 0xC, 0xFFFF, 0, 0, 0x0010_0010];
        words.extend([0u32; 16]);
        assert_eq!(length(&words, 16), Some(Some(22)));
    }

    #[test]
    fn a_fill_is_clipped() {
        let mut e = Engine::default();
        let mut vram = vec![0u8; 64 * 64 * 2];
        let mut out = Default::default();
        e.run(&[op::SETUP, 0x0040_0040, 0x0010_0004, 0, 128, 0], &mut vram, &mut out);
        e.run(&[op::SET_CLIPPING, 0x0002_0002, 0x0004_0004], &mut vram, &mut out);
        e.run(&[op::RECT_SOLID, 0x1234_1234, 0, 0x0040_0040], &mut vram, &mut out);
        assert_eq!(&vram[128 * 2 + 4..128 * 2 + 8], &[0x34, 0x12, 0x34, 0x12]);
        assert_eq!(&vram[128 * 2 + 8..128 * 2 + 10], &[0, 0]);
        assert_eq!(&vram[0..2], &[0, 0]);
    }
}
