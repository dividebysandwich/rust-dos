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

/// A codebook entry: four luma (or grey) samples, and blue and red
/// chroma.
#[derive(Clone, Copy, Default)]
struct Entry {
    y: [u8; 4],
    u: i8,
    v: i8,
}

impl Entry {
    /// Its four pixels as red, green and blue.
    fn rgb(&self) -> [[u8; 3]; 4] {
        let (u, v) = (i32::from(self.u), i32::from(self.v));
        self.y.map(|y| {
            let y = i32::from(y);
            let c = |x: i32| x.clamp(0, 255) as u8;
            [c(y + 2 * v), c(y - u / 2 - v), c(y + 2 * u)]
        })
    }
}

#[derive(Clone)]
struct Codebooks {
    v4: [Entry; 256],
    v1: [Entry; 256],
}

impl Default for Codebooks {
    fn default() -> Codebooks {
        Codebooks {
            v4: [Entry::default(); 256],
            v1: [Entry::default(); 256],
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
                        block[q] = books.v4[usize::from(e)].rgb();
                    }
                    at += 4;
                } else {
                    let &e = data.get(at).ok_or("V1 block cut short")?;
                    // A V1 entry's sample is a 2 by 2 square of its own.
                    let p = books.v1[usize::from(e)].rgb();
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
        *e = Entry {
            y: [b[0], b[1], b[2], b[3]],
            u: if n == 6 { b[4] as i8 } else { 0 },
            v: if n == 6 { b[5] as i8 } else { 0 },
        };
        at += n;
    }
}
