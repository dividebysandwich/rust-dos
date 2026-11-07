//! Cinepak (CVID), the codec Windows 95 and 98 install as `iccvid.dll`.
//!
//! A frame is a header and strips of rows. Each strip has two codebooks
//! of 256 entries, V1 and V4, each entry four luma samples and a chroma
//! pair (or four grey samples), and codes the strip's blocks of 4 by 4
//! pixels: a V1 block takes one entry, each of its samples covering 2 by
//! 2 pixels; a V4 block takes four, one for each quarter. An inter
//! frame's blocks may be skipped, keeping the pixels of the frame
//! before, and its codebook chunks may replace only some entries.
//!
//! A strip keeps its codebooks from one frame to the next. When a
//! frame's flags have bit 0 clear, each strip after the first starts
//! with the codebooks the strip above it ended with. This is how FFmpeg's
//! `cinepak.c` decodes, which the movies' pictures were compared with.

/// The most strips a frame has, as FFmpeg's decoder takes.
const MAX_STRIPS: usize = 32;

/// A codebook entry: its four pixels as red, green and blue, made from
/// four luma (or grey) samples and a chroma pair when the codebook is
/// read, as FFmpeg's decoder does, so that a block only copies them.
type Entry = [[u8; 3]; 4];

/// The entry of luma samples `y` and blue and red chroma `u` and `v`.
fn entry(y: [u8; 4], u: i8, v: i8) -> Entry {
    let (u, v) = (i32::from(u), i32::from(v));
    y.map(|y| {
        let y = i32::from(y);
        let c = |x: i32| x.clamp(0, 255) as u8;
        [c(y + 2 * v), c(y - u / 2 - v), c(y + 2 * u)]
    })
}

#[derive(Clone)]
struct Codebooks {
    v4: [Entry; 256],
    v1: [Entry; 256],
}

impl Default for Codebooks {
    fn default() -> Codebooks {
        Codebooks {
            v4: [[[0; 3]; 4]; 256],
            v1: [[[0; 3]; 4]; 256],
        }
    }
}

/// A Cinepak decoder: the picture, as rows of red, green and blue from
/// the top, in whole blocks, and each strip's codebooks.
#[derive(Default)]
pub struct Decoder {
    width: usize,
    height: usize,
    pixels: Vec<[u8; 3]>,
    strips: Vec<Codebooks>,
}

fn be16(d: &[u8], at: usize) -> usize {
    usize::from(u16::from_be_bytes([d[at], d[at + 1]]))
}

fn be24(d: &[u8], at: usize) -> usize {
    (usize::from(d[at]) << 16) | be16(d, at + 1)
}

/// The flag words of a chunk, read 32 bits at a time from the top bit.
struct Flags {
    word: u32,
    mask: u32,
}

impl Flags {
    /// The next flag, reading a new word from `data` at `at` when the
    /// last is used up.
    fn next(&mut self, data: &[u8], at: &mut usize) -> Result<bool, String> {
        self.mask >>= 1;
        if self.mask == 0 {
            let w = data.get(*at..*at + 4).ok_or("flags cut short")?;
            self.word = u32::from_be_bytes([w[0], w[1], w[2], w[3]]);
            self.mask = 0x8000_0000;
            *at += 4;
        }
        Ok(self.word & self.mask != 0)
    }
}

impl Decoder {
    /// The picture's size: the frames' size in whole blocks.
    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// The pixels of row `y`, red, green and blue from the left.
    pub fn row(&self, y: usize) -> &[[u8; 3]] {
        &self.pixels[y * self.width..][..self.width]
    }

    /// Decode a frame onto the picture. A frame larger than `max` on a
    /// side is refused. A frame that fails partway keeps what it drew
    /// and the codebook entries it read, as with FFmpeg's decoder.
    pub fn decode(&mut self, frame: &[u8], max: usize) -> Result<(), String> {
        if frame.len() < 10 {
            return Err("frame header cut short".to_string());
        }
        let flags = frame[0];
        let end = be24(frame, 1).min(frame.len());
        let (w, h) = (be16(frame, 4), be16(frame, 6));
        if w == 0 || h == 0 || w > max || h > max {
            return Err(format!("a {w}x{h} frame"));
        }
        let (w, h) = (w.next_multiple_of(4), h.next_multiple_of(4));
        if (w, h) != (self.width, self.height) {
            self.width = w;
            self.height = h;
            self.pixels = vec![[0; 3]; w * h];
        }
        let count = be16(frame, 8).min(MAX_STRIPS);
        if self.strips.len() < count {
            self.strips.resize(count, Codebooks::default());
        }
        let mut at = 10;
        let mut y0 = 0;
        for i in 0..count {
            let s = frame.get(at..at + 12).filter(|_| at + 12 <= end);
            let s = s.ok_or_else(|| format!("strip {i} cut short"))?;
            // A y of 0 at the top is the bottom of the strip above, and
            // the bottom is then a height.
            let (mut y1, mut y2) = (be16(s, 4), be16(s, 8));
            if y1 == 0 {
                y1 = y0;
                y2 += y0;
            }
            let (x1, x2) = (be16(s, 6), be16(s, 10));
            let size = be24(s, 1)
                .checked_sub(12)
                .ok_or_else(|| format!("strip {i} of a bad size"))?;
            at += 12;
            let data = &frame[at..(at + size).min(end)];
            if i > 0 && flags & 1 == 0 {
                self.strips[i] = self.strips[i - 1].clone();
            }
            // FFmpeg's decoder refuses a strip that isn't inside the
            // frame, or covers nothing.
            if x1 >= x2 || y1 >= y2 || x2 > w || y2 > h {
                return Err(format!("strip {i} of {x1},{y1} to {x2},{y2}"));
            }
            self.strip(i, data, (x1, y1, x2, y2))?;
            at += data.len();
            y0 = y2;
        }
        Ok(())
    }

    /// Decode strip `i`'s chunks, which cover `x1..x2` and `y1..y2`.
    fn strip(
        &mut self,
        i: usize,
        data: &[u8],
        rect: (usize, usize, usize, usize),
    ) -> Result<(), String> {
        let mut at = 0;
        while at + 4 <= data.len() {
            let id = data[at];
            let size = be24(data, at + 1)
                .checked_sub(4)
                .ok_or("chunk of a bad size")?;
            at += 4;
            let chunk = &data[at..(at + size).min(data.len())];
            match id {
                0x20 | 0x21 | 0x24 | 0x25 => codebook(&mut self.strips[i].v4, id, chunk),
                0x22 | 0x23 | 0x26 | 0x27 => codebook(&mut self.strips[i].v1, id, chunk),
                0x30..=0x32 => return self.vectors(i, id, chunk, rect),
                _ => {}
            }
            at += chunk.len();
        }
        Err("a strip with no blocks".to_string())
    }

    /// Draw a strip's blocks: chunk 0x30 has a flag for each block, V1
    /// or V4; 0x31 also one before it, whether it is coded or kept from
    /// the frame before; in 0x32 every block is V1.
    fn vectors(
        &mut self,
        i: usize,
        id: u8,
        data: &[u8],
        (x1, y1, x2, y2): (usize, usize, usize, usize),
    ) -> Result<(), String> {
        let books = &self.strips[i];
        let mut flags = Flags { word: 0, mask: 0 };
        let mut at = 0;
        for y in (y1..y2).step_by(4) {
            for x in (x1..x2).step_by(4) {
                if id & 1 != 0 && !flags.next(data, &mut at)? {
                    continue;
                }
                let v4 = id & 2 == 0 && flags.next(data, &mut at)?;
                // The 2 by 2 squares of the block, each an entry's
                // pixels laid out two above two.
                let mut block = [[[0u8; 3]; 4]; 4];
                if v4 {
                    let n = data.get(at..at + 4).ok_or("V4 block cut short")?;
                    for (q, &e) in n.iter().enumerate() {
                        block[q] = books.v4[usize::from(e)];
                    }
                    at += 4;
                } else {
                    let &e = data.get(at).ok_or("V1 block cut short")?;
                    // A V1 entry's sample is a 2 by 2 square of its own.
                    let p = books.v1[usize::from(e)];
                    for (q, s) in block.iter_mut().zip(p) {
                        *q = [s; 4];
                    }
                    at += 1;
                }
                for (q, square) in block.iter().enumerate() {
                    let (qx, qy) = (x + (q & 1) * 2, y + (q >> 1) * 2);
                    for (k, &px) in square.iter().enumerate() {
                        let (px_x, px_y) = (qx + (k & 1), qy + (k >> 1));
                        if px_x < self.width && px_y < self.height {
                            self.pixels[px_y * self.width + px_x] = px;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// Read a codebook chunk: bit 2 of `id` makes its entries grey (4 bytes,
/// no chroma), and bit 0 has a flag word before each 32 entries saying
/// which of them it replaces.
fn codebook(book: &mut [Entry; 256], id: u8, data: &[u8]) {
    let n = if id & 4 != 0 { 4 } else { 6 };
    let mut flags = Flags { word: 0, mask: 0 };
    let mut at = 0;
    for e in book.iter_mut() {
        if id & 1 != 0 {
            match flags.next(data, &mut at) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => break,
            }
        }
        let Some(b) = data.get(at..at + n) else {
            break;
        };
        let (u, v) = if n == 6 {
            (b[4] as i8, b[5] as i8)
        } else {
            (0, 0)
        };
        *e = entry([b[0], b[1], b[2], b[3]], u, v);
        at += n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chunk, or a frame's or a strip's header and body: `id`, then
    /// the size with the header's `header` bytes, in 24 bits.
    fn sized(id: u8, header: &[u8], body: &[u8]) -> Vec<u8> {
        let size = (4 + header.len() + body.len()) as u32;
        let mut out = vec![id];
        out.extend_from_slice(&size.to_be_bytes()[1..]);
        out.extend_from_slice(header);
        out.extend_from_slice(body);
        out
    }

    /// A strip of rows `y1..y2`, `width` wide, of `chunks`.
    fn strip(y1: u16, y2: u16, width: u16, chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut header = Vec::new();
        for v in [y1, 0, y2, width] {
            header.extend_from_slice(&v.to_be_bytes());
        }
        sized(0x11, &header, &chunks.concat())
    }

    /// A frame of `width` by `height` with frame flags `flags`.
    fn frame(flags: u8, width: u16, height: u16, strips: &[Vec<u8>]) -> Vec<u8> {
        let mut header = Vec::new();
        for v in [width, height, strips.len() as u16] {
            header.extend_from_slice(&v.to_be_bytes());
        }
        sized(flags, &header, &strips.concat())
    }

    /// A grey V1 codebook chunk whose entries are each one grey level.
    fn grey_v1(levels: &[u8]) -> Vec<u8> {
        let body: Vec<u8> = levels.iter().flat_map(|&l| [l; 4]).collect();
        sized(0x26, &[], &body)
    }

    /// The grey level of the pixel at `x`, `y`.
    fn grey(d: &Decoder, x: usize, y: usize) -> u8 {
        let [r, g, b] = d.row(y)[x];
        assert!(r == g && g == b, "{x},{y}: {:?}", [r, g, b]);
        r
    }

    #[test]
    fn strips_take_the_codebooks_above_only_when_flag_bit_0_is_clear() {
        let mut d = Decoder::default();
        // Two strips of a 4 by 8 picture, the lower with no codebook.
        let block = sized(0x32, &[], &[0]);
        let top = |chunks: &[Vec<u8>]| strip(0, 4, 4, chunks);
        let bottom = strip(4, 8, 4, std::slice::from_ref(&block));
        d.decode(
            &frame(
                0,
                4,
                8,
                &[top(&[grey_v1(&[200]), block.clone()]), bottom.clone()],
            ),
            64,
        )
        .unwrap();
        assert_eq!((grey(&d, 0, 0), grey(&d, 0, 7)), (200, 200));
        // Bit 0 set: the lower strip keeps its own.
        d.decode(
            &frame(
                1,
                4,
                8,
                &[top(&[grey_v1(&[50]), block.clone()]), bottom.clone()],
            ),
            64,
        )
        .unwrap();
        assert_eq!((grey(&d, 0, 0), grey(&d, 0, 7)), (50, 200));
        // Bit 0 clear: it starts from the upper strip's again.
        d.decode(
            &frame(0, 4, 8, &[top(std::slice::from_ref(&block)), bottom]),
            64,
        )
        .unwrap();
        assert_eq!((grey(&d, 0, 0), grey(&d, 0, 7)), (50, 50));
    }

    #[test]
    fn selective_codebooks_and_skipped_blocks_keep_what_was_there() {
        let mut d = Decoder::default();
        let key = strip(
            0,
            4,
            12,
            &[grey_v1(&[10, 20, 30]), sized(0x32, &[], &[0, 1, 2])],
        );
        d.decode(&frame(0, 12, 4, &[key]), 64).unwrap();
        let row = |d: &Decoder| [0, 4, 8].map(|x| grey(d, x, 3));
        assert_eq!(row(&d), [10, 20, 30]);
        // Entry 2 replaced, entry 0 kept. Block 0 skipped, block 1 V1
        // entry 2, block 2 V1 entry 0: flags 0, 1 0, 1 0.
        let update = sized(0x27, &[], &[0x20, 0, 0, 0, 90, 90, 90, 90]);
        let blocks = sized(0x31, &[], &[0x50, 0, 0, 0, 2, 0]);
        d.decode(&frame(1, 12, 4, &[strip(0, 4, 12, &[update, blocks])]), 64)
            .unwrap();
        assert_eq!(row(&d), [10, 90, 10]);
    }

    #[test]
    fn a_v4_block_takes_an_entry_for_each_quarter() {
        let mut d = Decoder::default();
        // Entry q's samples are 40 q, 40 q + 10, 40 q + 20, 40 q + 30.
        let body: Vec<u8> = (0..16).map(|i| i * 10).collect();
        let books = sized(0x24, &[], &body);
        let block = sized(0x30, &[], &[0x80, 0, 0, 0, 0, 1, 2, 3]);
        d.decode(&frame(0, 4, 4, &[strip(0, 4, 4, &[books, block])]), 64)
            .unwrap();
        for y in 0..4 {
            for x in 0..4 {
                let (q, k) = (y / 2 * 2 + x / 2, y % 2 * 2 + x % 2);
                assert_eq!(grey(&d, x, y), (q * 40 + k * 10) as u8, "{x},{y}");
            }
        }
    }

    #[test]
    fn a_strip_outside_the_frame_is_refused() {
        let mut d = Decoder::default();
        let block = sized(0x32, &[], &[0]);
        let wide = strip(0, 4, 8, &[grey_v1(&[200]), block]);
        assert!(d.decode(&frame(0, 4, 4, &[wide]), 64).is_err());
    }
}
