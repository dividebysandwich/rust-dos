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

/// A pixel's samples: its luma, its block's blue and red chroma, which
/// are signed bytes, and the kind of entry it came from (`COLOUR`,
/// `GREY_V4` or `GREY_V1`), which iccvid.dll's 8-bit output tells apart.
type Sample = [u8; 4];

/// A sample of a colour codebook's entry.
const COLOUR: u8 = 0;
/// A sample of a grey codebook's entry (chunks 0x24 to 0x27), of the V4
/// codebook or the V1 one.
const GREY_V4: u8 = 1;
const GREY_V1: u8 = 2;

/// A codebook entry: the samples of its four pixels, each a luma (or
/// grey) sample with the entry's chroma pair, so that a block only
/// copies them.
type Entry = [Sample; 4];

/// The entry of luma samples `y`, blue and red chroma `u` and `v`, and
/// kind `kind`.
fn entry(y: [u8; 4], u: u8, v: u8, kind: u8) -> Entry {
    y.map(|y| [y, u, v, kind])
}

/// The red, green and blue of a pixel's samples, as FFmpeg's decoder
/// makes them.
fn rgb([y, u, v, _]: Sample) -> [u8; 3] {
    let (y, u, v) = (i32::from(y), i32::from(u as i8), i32::from(v as i8));
    let c = |x: i32| x.clamp(0, 255) as u8;
    [c(y + 2 * v), c(y - u / 2 - v), c(y + 2 * u)]
}

#[derive(Clone)]
struct Codebooks {
    v4: [Entry; 256],
    v1: [Entry; 256],
}

impl Default for Codebooks {
    fn default() -> Codebooks {
        Codebooks {
            v4: [[[0; 4]; 4]; 256],
            v1: [[[0; 4]; 4]; 256],
        }
    }
}

/// A Cinepak decoder: the picture, as rows of samples from the top, in
/// whole blocks, and each strip's codebooks.
#[derive(Default)]
pub struct Decoder {
    width: usize,
    height: usize,
    pixels: Vec<Sample>,
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
    pub fn row(&self, y: usize) -> impl Iterator<Item = [u8; 3]> + '_ {
        self.pixels[y * self.width..][..self.width]
            .iter()
            .map(|&s| rgb(s))
    }

    /// The top left `width` by `height` of the picture as iccvid.dll
    /// decodes it to 8 bits, at `zoom` times its size (1 or 2): rows of
    /// indexes of `palette_8` from the top, `width * zoom` each. None if
    /// the picture is smaller.
    pub fn indexes(&self, width: usize, height: usize, zoom: usize) -> Option<Vec<u8>> {
        if width > self.width || height > self.height || !(1..=2).contains(&zoom) {
            return None;
        }
        let w = width * zoom;
        let mut out = vec![0; w * height * zoom];
        for (y, row) in out.chunks_exact_mut(w.max(1)).enumerate() {
            let sy = y / zoom;
            let line = &self.pixels[sy * self.width..];
            for (x, o) in row.iter_mut().enumerate() {
                let mut s = line[x / zoom];
                // At twice the size, iccvid.dll draws the lower half of a
                // grey V1 block with its upper two samples again.
                if zoom == 2 && s[3] == GREY_V1 && sy % 4 >= 2 {
                    s = self.pixels[(sy - 2) * self.width + x / zoom];
                }
                *o = index_8(s, x, y, zoom);
            }
        }
        Some(out)
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
            self.pixels = vec![[0; 4]; w * h];
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
                let mut block = [[[0u8; 4]; 4]; 4];
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
        let (u, v, kind) = match (n, id & 2) {
            (6, _) => (b[4], b[5], COLOUR),
            (_, 0) => (0, 0, GREY_V4),
            _ => (0, 0, GREY_V1),
        };
        *e = entry([b[0], b[1], b[2], b[3]], u, v, kind);
        at += n;
    }
}

// iccvid.dll's 8-bit output.
//
// Asked for 8 bits (ICM_DECOMPRESS_GET_PALETTE, then ICDecompress to an
// 8-bit DIB), Windows 98's Cinepak codec gives indexes of a palette of
// its own and dithers to it itself, with a 4x4 ordered dither. Its
// palette is a lattice in YUV: entries 10 to 230 are colours of 16 luma
// levels and 8 levels each of the two chroma samples, those outside RGB
// left out or merged (`LATTICE_COLORS`); 231 to 245 are (0, 0, 1) to
// (0, 0, 15), which no pixel takes; the rest are the system palette's
// 20 static colours.
//
// A pixel's luma sample and its block's chroma pair each take a level
// by a threshold of its own, `Y_STEPS`, `U_STEPS` and `V_STEPS` (the
// level is how many of the steps are at or below the sample, the chroma
// read as signed bytes). Each of the four sets of steps is a phase of
// the dither, and the phase of the pixel `x` from the left and `y` from
// the top is `PHASE[y % 4][x % 4]`. The three levels make an index into
// `LATTICE`, which gives the palette entry. At twice the size, which
// the codec also decodes to in 8 bits, each sample covers 2x2 pixels
// and the phase goes by the pixel drawn.
//
// Grey codebook entries (chunks 0x24 to 0x27) take the greys of the
// palette the codec gives a grey movie, even in a colour one, where
// those entries are lattice colours: `grey_8`. A movie of 8 bits gets
// that palette of 236 greys.
//
// The codec does this with tables of 256 entries for each sample, which
// give the four phases' levels together, and works each codebook entry
// out for the phases of the pixels it covers when the codebook is read.
// These are those tables, read from its code, and its palette, which
// ICM_DECOMPRESS_GET_PALETTE gives and MCIAVI realizes. They match
// pixel for pixel what Windows 98 SE draws (`tests`).

/// The dither phase of the pixel `x` from the left and `y` from the top
/// is `PHASE[y % 4][x % 4]`.
const PHASE: [[usize; 4]; 4] = [[0, 1, 2, 3], [2, 3, 0, 1], [1, 2, 3, 0], [3, 0, 1, 2]];

/// The first palette entry of the lattice.
const LATTICE_FIRST: usize = 10;

/// The palette's entries 10 to 230, as red, green and blue.
#[rustfmt::skip]
const LATTICE_COLORS: [[u8; 3]; 221] = [
    [2, 2, 2], [25, 25, 25], [71, 2, 25], [25, 13, 71],
    [0, 81, 0], [46, 58, 0], [92, 35, 0], [143, 10, 0],
    [0, 69, 46], [46, 46, 46], [92, 23, 46], [0, 58, 92],
    [46, 35, 92], [92, 12, 92], [0, 44, 149], [46, 21, 149],
    [0, 26, 220], [46, 3, 220], [21, 102, 21], [67, 79, 21],
    [113, 56, 21], [164, 30, 21], [219, 2, 21], [21, 90, 67],
    [67, 67, 67], [113, 44, 67], [164, 19, 67], [21, 79, 113],
    [67, 56, 113], [113, 33, 113], [164, 7, 113], [21, 64, 170],
    [67, 41, 170], [113, 18, 170], [21, 47, 241], [67, 24, 241],
    [113, 1, 241], [41, 121, 41], [87, 98, 41], [133, 75, 41],
    [183, 50, 41], [239, 22, 41], [41, 110, 87], [87, 87, 87],
    [133, 64, 87], [183, 39, 87], [239, 11, 87], [41, 98, 133],
    [87, 75, 133], [133, 52, 133], [183, 27, 133], [41, 84, 190],
    [87, 61, 190], [133, 38, 190], [183, 13, 190], [3, 181, 9],
    [60, 153, 9], [106, 130, 9], [152, 107, 9], [202, 81, 9],
    [3, 169, 60], [60, 140, 60], [106, 117, 60], [152, 94, 60],
    [202, 69, 60], [3, 157, 106], [60, 129, 106], [106, 106, 106],
    [152, 83, 106], [202, 57, 106], [3, 146, 152], [60, 117, 152],
    [106, 94, 152], [152, 71, 152], [202, 46, 152], [3, 131, 209],
    [60, 103, 209], [106, 80, 209], [152, 57, 209], [202, 32, 209],
    [20, 199, 27], [77, 171, 27], [123, 148, 27], [169, 125, 27],
    [220, 99, 27], [20, 186, 77], [77, 158, 77], [123, 135, 77],
    [169, 112, 77], [220, 87, 77], [20, 175, 123], [77, 146, 123],
    [123, 123, 123], [169, 100, 123], [220, 75, 123], [20, 163, 169],
    [77, 135, 169], [123, 112, 169], [169, 89, 169], [220, 64, 169],
    [20, 149, 226], [77, 121, 226], [123, 98, 226], [169, 75, 226],
    [220, 49, 226], [37, 216, 44], [94, 187, 44], [140, 164, 44],
    [186, 141, 44], [237, 116, 44], [37, 203, 94], [94, 175, 94],
    [140, 152, 94], [186, 129, 94], [237, 103, 94], [37, 192, 140],
    [94, 163, 140], [140, 140, 140], [186, 117, 140], [237, 92, 140],
    [37, 180, 186], [94, 152, 186], [140, 129, 186], [186, 106, 186],
    [237, 80, 186], [37, 166, 243], [94, 138, 243], [140, 115, 243],
    [186, 92, 243], [237, 66, 243], [53, 246, 4], [110, 217, 4],
    [156, 194, 4], [202, 171, 4], [253, 146, 4], [53, 232, 60],
    [110, 203, 60], [156, 180, 60], [202, 157, 60], [253, 132, 60],
    [53, 219, 110], [110, 191, 110], [156, 168, 110], [202, 145, 110],
    [253, 120, 110], [53, 208, 156], [110, 179, 156], [156, 156, 156],
    [202, 133, 156], [253, 108, 156], [53, 196, 202], [110, 168, 202],
    [156, 145, 202], [202, 122, 202], [253, 97, 202], [126, 233, 19],
    [172, 210, 19], [218, 187, 19], [69, 247, 75], [126, 219, 75],
    [172, 196, 75], [218, 173, 75], [69, 235, 126], [126, 206, 126],
    [172, 183, 126], [218, 160, 126], [69, 223, 172], [126, 195, 172],
    [172, 172, 172], [218, 149, 172], [69, 212, 218], [126, 183, 218],
    [172, 160, 218], [218, 137, 218], [140, 247, 34], [186, 224, 34],
    [232, 201, 34], [140, 233, 89], [186, 210, 89], [232, 187, 89],
    [83, 249, 140], [140, 221, 140], [186, 198, 140], [232, 175, 140],
    [83, 238, 186], [140, 209, 186], [186, 186, 186], [232, 163, 186],
    [83, 226, 232], [140, 198, 232], [186, 175, 232], [232, 152, 232],
    [200, 238, 48], [246, 215, 48], [154, 247, 103], [200, 224, 103],
    [246, 201, 103], [154, 234, 154], [200, 211, 154], [246, 188, 154],
    [97, 251, 200], [154, 223, 200], [200, 200, 200], [246, 177, 200],
    [97, 240, 246], [154, 211, 246], [200, 188, 246], [246, 165, 246],
    [213, 251, 61], [213, 237, 116], [167, 247, 167], [213, 224, 167],
    [167, 236, 213], [213, 213, 213], [225, 250, 129], [225, 237, 179],
    [179, 248, 225], [225, 225, 225], [237, 249, 191], [237, 237, 237],
    [248, 248, 248],
];
/// The palette entry of the luma level `l` and chroma levels `u` and
/// `v`: `LATTICE[l << 6 | u << 3 | v]`.
#[rustfmt::skip]
const LATTICE: [u8; 1024] = [
    10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
    10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
    10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
    10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10, 10,
    11, 11, 11, 11, 12, 12, 12, 12, 11, 11, 11, 11, 12, 12, 12, 12,
    11, 11, 11, 11, 12, 12, 12, 12, 11, 11, 11, 11, 12, 12, 12, 12,
    11, 11, 11, 11, 12, 12, 12, 12, 13, 13, 13, 13, 12, 12, 12, 12,
    13, 13, 13, 13, 13, 12, 12, 12, 13, 13, 13, 13, 13, 13, 12, 12,
    14, 14, 14, 15, 16, 17, 17, 17, 14, 14, 14, 15, 16, 17, 17, 17,
    14, 14, 14, 15, 16, 17, 17, 17, 14, 14, 14, 15, 16, 17, 17, 17,
    18, 18, 18, 19, 20, 17, 17, 17, 21, 21, 21, 22, 23, 23, 23, 17,
    24, 24, 24, 25, 23, 23, 23, 23, 26, 26, 26, 27, 27, 27, 23, 23,
    28, 28, 28, 29, 30, 31, 32, 32, 28, 28, 28, 29, 30, 31, 32, 32,
    28, 28, 28, 29, 30, 31, 32, 32, 28, 28, 28, 29, 30, 31, 32, 32,
    33, 33, 33, 34, 35, 36, 32, 32, 37, 37, 37, 38, 39, 40, 40, 40,
    41, 41, 41, 42, 43, 40, 40, 40, 44, 44, 44, 45, 46, 46, 46, 40,
    47, 47, 47, 48, 49, 50, 51, 51, 47, 47, 47, 48, 49, 50, 51, 51,
    47, 47, 47, 48, 49, 50, 51, 51, 47, 47, 47, 48, 49, 50, 51, 51,
    52, 52, 52, 53, 54, 55, 56, 56, 57, 57, 57, 58, 59, 60, 56, 56,
    61, 61, 61, 62, 63, 64, 64, 64, 61, 61, 61, 62, 63, 64, 64, 64,
    65, 65, 66, 67, 68, 69, 69, 69, 65, 65, 66, 67, 68, 69, 69, 69,
    65, 65, 66, 67, 68, 69, 69, 69, 70, 70, 71, 72, 73, 74, 74, 74,
    75, 75, 76, 77, 78, 79, 79, 79, 80, 80, 81, 82, 83, 84, 84, 84,
    85, 85, 86, 87, 88, 89, 89, 89, 85, 85, 86, 87, 88, 89, 89, 89,
    90, 90, 91, 92, 93, 94, 94, 94, 90, 90, 91, 92, 93, 94, 94, 94,
    90, 90, 91, 92, 93, 94, 94, 94, 95, 95, 96, 97, 98, 99, 99, 99,
    100, 100, 101, 102, 103, 104, 104, 104, 105, 105, 106, 107, 108, 109, 109, 109,
    110, 110, 111, 112, 113, 114, 114, 114, 110, 110, 111, 112, 113, 114, 114, 114,
    115, 115, 116, 117, 118, 119, 119, 119, 115, 115, 116, 117, 118, 119, 119, 119,
    115, 115, 116, 117, 118, 119, 119, 119, 120, 120, 121, 122, 123, 124, 124, 124,
    125, 125, 126, 127, 128, 129, 129, 129, 130, 130, 131, 132, 133, 134, 134, 134,
    135, 135, 136, 137, 138, 139, 139, 139, 135, 135, 136, 137, 138, 139, 139, 139,
    140, 140, 141, 142, 143, 144, 144, 144, 140, 140, 141, 142, 143, 144, 144, 144,
    145, 145, 146, 147, 148, 149, 149, 149, 150, 150, 151, 152, 153, 154, 154, 154,
    155, 155, 156, 157, 158, 159, 159, 159, 160, 160, 161, 162, 163, 164, 164, 164,
    160, 160, 161, 162, 163, 164, 164, 164, 160, 160, 161, 162, 163, 164, 164, 164,
    165, 165, 165, 166, 167, 167, 167, 167, 168, 165, 165, 166, 167, 167, 167, 167,
    168, 168, 169, 170, 171, 171, 171, 171, 172, 172, 173, 174, 175, 175, 175, 175,
    176, 176, 177, 178, 179, 179, 179, 179, 180, 180, 181, 182, 183, 183, 183, 183,
    180, 180, 181, 182, 183, 183, 183, 183, 180, 180, 181, 182, 183, 183, 183, 183,
    184, 184, 184, 185, 186, 186, 186, 186, 184, 184, 184, 185, 186, 186, 186, 186,
    190, 187, 187, 188, 189, 189, 189, 189, 190, 190, 191, 192, 193, 193, 193, 193,
    194, 194, 195, 196, 197, 197, 197, 197, 198, 198, 199, 200, 201, 201, 201, 201,
    198, 198, 199, 200, 201, 201, 201, 201, 198, 198, 199, 200, 201, 201, 201, 201,
    204, 202, 202, 202, 203, 203, 203, 203, 204, 204, 202, 202, 203, 203, 203, 203,
    204, 204, 204, 205, 206, 206, 206, 206, 210, 207, 207, 208, 209, 209, 209, 209,
    210, 210, 211, 212, 213, 213, 213, 213, 214, 214, 215, 216, 217, 217, 217, 217,
    214, 214, 215, 216, 217, 217, 217, 217, 214, 214, 215, 216, 217, 217, 217, 217,
    218, 218, 218, 218, 218, 218, 218, 218, 220, 218, 218, 218, 218, 218, 218, 218,
    220, 220, 219, 219, 219, 219, 219, 219, 220, 220, 220, 221, 221, 221, 221, 221,
    222, 222, 222, 223, 223, 223, 223, 223, 222, 222, 222, 223, 223, 223, 223, 223,
    222, 222, 222, 223, 223, 223, 223, 223, 222, 222, 222, 223, 223, 223, 223, 223,
    224, 224, 224, 224, 224, 224, 224, 224, 224, 224, 224, 224, 224, 224, 224, 224,
    226, 224, 224, 224, 224, 224, 224, 224, 226, 226, 225, 225, 225, 225, 225, 225,
    226, 226, 226, 227, 227, 227, 227, 227, 226, 226, 226, 227, 227, 227, 227, 227,
    226, 226, 226, 227, 227, 227, 227, 227, 226, 226, 226, 227, 227, 227, 227, 227,
    228, 228, 228, 228, 228, 228, 228, 228, 228, 228, 228, 228, 228, 228, 228, 228,
    228, 228, 228, 228, 228, 228, 228, 228, 228, 228, 228, 228, 228, 228, 228, 228,
    229, 229, 229, 229, 229, 229, 229, 229, 229, 229, 229, 229, 229, 229, 229, 229,
    229, 229, 229, 229, 229, 229, 229, 229, 229, 229, 229, 229, 229, 229, 229, 229,
    230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230,
    230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230,
    230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230,
    230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230, 230,
];
/// The steps of a luma sample's 16 levels, for each phase.
const Y_STEPS: [[i16; 15]; 4] = [
    [
        11, 33, 55, 75, 94, 113, 130, 147, 162, 177, 192, 205, 218, 230, 242,
    ],
    [
        20, 42, 63, 83, 102, 120, 137, 153, 169, 183, 197, 210, 223, 235, 246,
    ],
    [
        6, 29, 51, 71, 91, 109, 127, 143, 159, 174, 189, 202, 215, 228, 239,
    ],
    [
        15, 38, 59, 79, 98, 116, 134, 150, 165, 180, 194, 208, 220, 232, 244,
    ],
];
/// The steps of a blue chroma sample's 8 levels, for each phase.
const U_STEPS: [[i16; 7]; 4] = [
    [-88, -59, -33, -9, 14, 40, 73],
    [-94, -65, -38, -14, 9, 34, 66],
    [-82, -54, -28, -5, 18, 46, 80],
    [-101, -71, -43, -18, 5, 29, 59],
];
/// The steps of a red chroma sample's 8 levels, for each phase.
const V_STEPS: [[i16; 7]; 4] = [
    [-80, -46, -18, 5, 28, 54, 82],
    [-66, -34, -9, 14, 38, 65, 94],
    [-73, -40, -14, 9, 33, 59, 88],
    [-59, -29, -5, 18, 43, 71, 101],
];

/// How many of `steps` are at or below `v`.
fn level(steps: &[i16], v: i16) -> usize {
    steps.iter().take_while(|&&s| s <= v).count()
}

/// The palette index iccvid.dll gives the pixel `x` from the left and
/// `y` from the top of its picture at `zoom` times the size, of samples
/// `s`, when it decodes to 8 bits.
fn index_8([l, u, v, kind]: Sample, x: usize, y: usize, zoom: usize) -> u8 {
    let phase = match (kind, zoom) {
        (COLOUR, _) => PHASE[y % 4][x % 4],
        // A grey sample takes the grey of its own palette (`grey_8`),
        // undithered for V4's at the picture's size, else by where the
        // pixel is in the square the sample covers.
        (GREY_V4, 1) => 0,
        (GREY_V1, 2) => (x + [0, 2, 1, 3][y % 4]) % 4,
        _ => y % 2 * 2 + x % 2,
    };
    if kind != COLOUR {
        return grey_8(l, phase);
    }
    let l = level(&Y_STEPS[phase], i16::from(l));
    let u = level(&U_STEPS[phase], i16::from(u as i8));
    let v = level(&V_STEPS[phase], i16::from(v as i8));
    LATTICE[l << 6 | u << 3 | v]
}

/// The index iccvid.dll gives a grey sample `l` in the dither phase
/// `phase`: one of the 236 greys of the palette it gives a grey movie
/// (`palette_8`), phases 1 and 2 a little above and below the nearest.
/// It does so in a colour movie too, whose palette has other colours
/// there: the codec takes a colour movie's codebooks to be in colour.
fn grey_8(l: u8, phase: usize) -> u8 {
    let offset = [128, 836, -580, 128][phase];
    (10 + (i32::from(l) * 236 + offset).div_euclid(256)).clamp(10, 245) as u8
}

/// The palette iccvid.dll gives for its 8-bit output, as COLORREFs, with
/// the static colours `statics` (the first ten, then the last ten): for a
/// colour movie the lattice, for a grey one (a format of 8 bits) 236
/// greys from 0 to 255.
pub fn palette_8(statics: &[u32; 20], grey: bool) -> Vec<u32> {
    let mut colors = vec![0u32; 256];
    colors[..10].copy_from_slice(&statics[..10]);
    colors[246..].copy_from_slice(&statics[10..]);
    if grey {
        for (k, c) in colors[10..246].iter_mut().enumerate() {
            let v = (118 + 256 * k as u32) / 236;
            *c = v | v << 8 | v << 16;
        }
        return colors;
    }
    for (c, &[r, g, b]) in colors[LATTICE_FIRST..].iter_mut().zip(&LATTICE_COLORS) {
        *c = u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16;
    }
    for (k, c) in colors[231..246].iter_mut().enumerate() {
        *c = (k as u32 + 1) << 16;
    }
    colors
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
        let [r, g, b] = d.row(y).nth(x).unwrap();
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

    /// The 16 blocks of a strip 64 pixels wide: the even ones V1 of entry
    /// k, the odd ones V4 of entries k to k + 3 (from 16).
    fn sixteen_blocks() -> Vec<u8> {
        let mut flags = 0u32;
        let mut body = Vec::new();
        for k in 0..16u8 {
            if k & 1 != 0 {
                flags |= 1 << (31 - k);
                body.extend((0..4).map(|j| (k + j) % 16));
            } else {
                body.push(k);
            }
        }
        let mut data = flags.to_be_bytes().to_vec();
        data.extend(body);
        sized(0x30, &[], &data)
    }

    /// The frame of the probe that read Windows 98's grey references: 64
    /// by 8, the upper strip of grey codebooks (chunks 0x24 and 0x26), the
    /// lower of the same luma in colour with chroma 0 (0x20 and 0x22),
    /// entry i's luma 16 i, 16 i + 4, 16 i + 8 and 16 i + 12.
    fn grey_and_colour_frame() -> Vec<u8> {
        let luma: Vec<[u8; 4]> = (0..16u8)
            .map(|i| [0, 4, 8, 12].map(|d| (i * 16).saturating_add(d)))
            .collect();
        let grey: Vec<u8> = luma.concat();
        let colour: Vec<u8> = luma
            .iter()
            .flat_map(|y| [&y[..], &[0, 0]].concat())
            .collect();
        let upper = [sized(0x24, &[], &grey), sized(0x26, &[], &grey)];
        let lower = [sized(0x20, &[], &colour), sized(0x22, &[], &colour)];
        frame(
            1,
            64,
            8,
            &[
                strip(0, 4, 64, &[&upper[..], &[sixteen_blocks()]].concat()),
                strip(4, 8, 64, &[&lower[..], &[sixteen_blocks()]].concat()),
            ],
        )
    }

    /// What ICDecompress of iccvid.dll gave on Windows 98 SE for
    /// `grey_and_colour_frame` to an 8-bit DIB of its size: the first 16
    /// pixels of each row from the top. Rows 0 to 3 are of grey entries,
    /// which take the codec's greys though the palette is the lattice's.
    const GREY_1X: [[u8; 16]; 8] = [
        [
            10, 13, 14, 16, 25, 28, 40, 43, 40, 42, 43, 46, 54, 58, 69, 73,
        ],
        [
            10, 10, 11, 14, 32, 36, 47, 51, 37, 40, 40, 43, 62, 65, 76, 80,
        ],
        [
            17, 20, 21, 24, 54, 58, 69, 73, 47, 50, 51, 53, 84, 87, 99, 102,
        ],
        [
            15, 17, 18, 21, 62, 65, 76, 80, 44, 47, 48, 51, 91, 95, 106, 110,
        ],
        [
            10, 10, 10, 10, 11, 11, 19, 11, 11, 11, 19, 11, 19, 19, 34, 34,
        ],
        [
            10, 10, 10, 10, 11, 11, 19, 19, 19, 11, 19, 11, 34, 34, 34, 34,
        ],
        [
            10, 11, 10, 11, 19, 34, 34, 34, 11, 19, 19, 19, 34, 53, 53, 77,
        ],
        [
            10, 10, 10, 11, 19, 34, 34, 53, 19, 19, 19, 19, 53, 53, 77, 77,
        ],
    ];

    /// The same at twice the size: the first 32 pixels of each row.
    const GREY_2X: [[u8; 32]; 16] = [
        [
            10, 13, 10, 10, 14, 16, 11, 14, 25, 28, 28, 31, 40, 42, 43, 46, 40, 42, 37, 40, 43, 46,
            40, 43, 54, 57, 58, 61, 69, 72, 73, 75,
        ],
        [
            10, 10, 10, 13, 11, 14, 14, 16, 22, 25, 26, 28, 37, 40, 40, 43, 37, 40, 40, 42, 40, 43,
            43, 46, 51, 54, 55, 58, 66, 69, 70, 73,
        ],
        [
            13, 10, 10, 10, 16, 11, 14, 14, 32, 35, 36, 39, 47, 50, 51, 53, 42, 37, 40, 40, 46, 40,
            43, 43, 62, 64, 65, 68, 76, 79, 80, 83,
        ],
        [
            10, 10, 13, 10, 14, 14, 16, 11, 29, 32, 33, 36, 44, 47, 48, 51, 40, 40, 42, 37, 43, 43,
            46, 40, 59, 62, 63, 65, 74, 76, 77, 80,
        ],
        [
            10, 13, 10, 10, 14, 16, 11, 14, 54, 57, 58, 61, 69, 72, 73, 75, 40, 42, 37, 40, 43, 46,
            40, 43, 84, 87, 87, 90, 99, 101, 102, 105,
        ],
        [
            10, 10, 10, 13, 11, 14, 14, 16, 51, 54, 55, 58, 66, 69, 70, 73, 37, 40, 40, 42, 40, 43,
            43, 46, 81, 84, 85, 87, 96, 99, 99, 102,
        ],
        [
            13, 10, 10, 10, 16, 11, 14, 14, 62, 64, 65, 68, 76, 79, 80, 83, 42, 37, 40, 40, 46, 40,
            43, 43, 91, 94, 95, 98, 106, 109, 110, 112,
        ],
        [
            10, 10, 13, 10, 14, 14, 16, 11, 59, 62, 63, 65, 74, 76, 77, 80, 40, 40, 42, 37, 43, 43,
            46, 40, 88, 91, 92, 95, 103, 106, 107, 110,
        ],
        [
            10, 10, 10, 10, 10, 10, 10, 10, 11, 10, 11, 11, 11, 11, 19, 11, 11, 11, 19, 11, 19, 11,
            19, 11, 19, 19, 34, 19, 34, 34, 34, 34,
        ],
        [
            10, 10, 10, 10, 10, 10, 10, 10, 11, 11, 11, 11, 19, 11, 19, 11, 19, 11, 11, 11, 19, 11,
            19, 11, 19, 19, 19, 19, 34, 34, 34, 34,
        ],
        [
            10, 10, 10, 10, 10, 10, 10, 10, 11, 11, 11, 11, 11, 19, 19, 19, 11, 19, 11, 11, 11, 19,
            11, 19, 19, 34, 34, 34, 34, 53, 34, 53,
        ],
        [
            10, 10, 10, 10, 10, 10, 10, 10, 11, 11, 11, 11, 19, 19, 19, 19, 11, 11, 11, 19, 11, 19,
            11, 19, 19, 34, 19, 34, 34, 34, 34, 53,
        ],
        [
            10, 10, 11, 10, 11, 10, 11, 10, 19, 19, 34, 19, 34, 34, 34, 34, 19, 11, 19, 19, 19, 19,
            19, 19, 53, 34, 53, 53, 77, 53, 77, 77,
        ],
        [
            11, 10, 10, 10, 11, 10, 11, 10, 19, 19, 19, 19, 34, 34, 34, 34, 19, 19, 19, 11, 19, 19,
            19, 19, 53, 53, 53, 53, 77, 53, 77, 53,
        ],
        [
            10, 11, 10, 10, 10, 11, 10, 11, 19, 34, 34, 34, 34, 53, 34, 53, 11, 19, 19, 19, 19, 19,
            19, 19, 53, 53, 53, 53, 77, 77, 77, 77,
        ],
        [
            10, 10, 10, 11, 10, 11, 10, 11, 19, 34, 19, 34, 34, 34, 34, 53, 19, 19, 11, 19, 19, 19,
            19, 19, 53, 53, 53, 77, 77, 77, 77, 77,
        ],
    ];

    /// The first `n` pixels of each row of `indexes`, rows `width` long.
    fn left<const N: usize>(indexes: &[u8], width: usize) -> Vec<[u8; N]> {
        indexes
            .chunks_exact(width)
            .map(|row| row[..N].try_into().unwrap())
            .collect()
    }

    #[test]
    fn grey_and_colour_entries_give_windows_98s_8_bit_indexes() {
        let mut d = Decoder::default();
        d.decode(&grey_and_colour_frame(), 64).unwrap();
        let one = d.indexes(64, 8, 1).unwrap();
        assert_eq!(left::<16>(&one, 64), GREY_1X);
        let two = d.indexes(64, 8, 2).unwrap();
        assert_eq!(left::<32>(&two, 128), GREY_2X);
    }

    #[test]
    fn indexes_are_refused_for_a_size_the_picture_lacks() {
        let mut d = Decoder::default();
        d.decode(&grey_and_colour_frame(), 64).unwrap();
        assert_eq!(d.indexes(65, 8, 1), None);
        assert_eq!(d.indexes(64, 8, 3), None);
        assert_eq!(d.indexes(0, 0, 1), Some(Vec::new()));
        assert_eq!(Decoder::default().indexes(0, 0, 2), Some(Vec::new()));
    }

    #[test]
    fn the_8_bit_palette_is_windows_98s() {
        // The statics of Windows' default palette, and entries of the
        // palette ICM_DECOMPRESS_GET_PALETTE gave on Windows 98 SE.
        let statics = crate::win32::gdi32::static_colors();
        let colour = palette_8(statics, false);
        let rgb = |c: u32| [c as u8, (c >> 8) as u8, (c >> 16) as u8];
        assert_eq!(colour[..10], statics[..10]);
        assert_eq!(colour[246..], statics[10..]);
        assert_eq!(rgb(colour[10]), [2, 2, 2]);
        assert_eq!(rgb(colour[11]), [25, 25, 25]);
        assert_eq!(rgb(colour[12]), [71, 2, 25]);
        assert_eq!(rgb(colour[13]), [25, 13, 71]);
        assert_eq!(rgb(colour[100]), [20, 175, 123]);
        assert_eq!(rgb(colour[231]), [0, 0, 1]);
        assert_eq!(rgb(colour[245]), [0, 0, 15]);
        let grey = palette_8(statics, true);
        assert_eq!(rgb(grey[10]), [0, 0, 0]);
        assert_eq!(rgb(grey[11]), [1, 1, 1]);
        assert_eq!(rgb(grey[245]), [255, 255, 255]);
    }

    /// The probe's frame of colour entries, 32 by 16: a V4 and a V1
    /// codebook of 32 entries spread over luma and both chroma samples,
    /// the even blocks V1 of entry k, the odd ones V4 of entries k to
    /// k + 3 (from 32).
    fn colour_frame() -> Vec<u8> {
        let v4: Vec<u8> = (0..32u32)
            .flat_map(|i| {
                let y = [0, 1, 2, 3].map(|k| ((i * 29 + k * 67 + 5) % 256) as u8);
                let c = [(i * 53 + 17) % 256, (i * 97 + 201) % 256].map(|c| c as u8);
                [&y[..], &c[..]].concat()
            })
            .collect();
        let v1: Vec<u8> = (0..32u32)
            .flat_map(|i| {
                let y = [0, 1, 2, 3].map(|k| ((i * 31 + k * 59 + 90) % 256) as u8);
                let c = [(i * 71 + 3) % 256, (i * 43 + 77) % 256].map(|c| c as u8);
                [&y[..], &c[..]].concat()
            })
            .collect();
        let mut flags = 0u32;
        let mut body = Vec::new();
        for b in 0..32u8 {
            if b & 1 != 0 {
                flags |= 1 << (31 - b);
                body.extend((0..4).map(|j| (b + j) % 32));
            } else {
                body.push(b);
            }
        }
        let mut blocks = flags.to_be_bytes().to_vec();
        blocks.extend(body);
        let chunks = [
            sized(0x20, &[], &v4),
            sized(0x22, &[], &v1),
            sized(0x30, &[], &blocks),
        ];
        frame(0, 32, 16, &[strip(0, 16, 32, &chunks)])
    }

    /// What ICDecompress of iccvid.dll gave on Windows 98 SE for
    /// `colour_frame` to an 8-bit DIB of its size, rows from the top.
    const COLOUR_1X: [[u8; 32]; 16] = [
        [
            56, 56, 159, 129, 23, 64, 44, 110, 140, 115, 218, 218, 47, 141, 99, 193, 223, 223, 13,
            13, 160, 222, 201, 230, 12, 12, 51, 51, 218, 10, 228, 14,
        ],
        [
            56, 56, 159, 129, 183, 229, 214, 10, 140, 140, 218, 218, 224, 11, 230, 32, 223, 222,
            13, 10, 13, 57, 46, 113, 12, 12, 51, 32, 51, 144, 90, 165,
        ],
        [
            213, 223, 10, 12, 47, 165, 99, 193, 10, 11, 28, 28, 130, 226, 182, 230, 38, 58, 136,
            132, 203, 12, 228, 14, 119, 119, 203, 203, 10, 50, 24, 80,
        ],
        [
            223, 223, 10, 12, 224, 14, 230, 32, 10, 11, 28, 47, 13, 80, 27, 113, 41, 58, 107, 132,
            51, 144, 65, 184, 119, 119, 203, 203, 129, 221, 180, 230,
        ],
        [
            61, 41, 135, 135, 10, 31, 21, 85, 117, 117, 218, 205, 45, 111, 69, 144, 223, 217, 12,
            10, 90, 190, 153, 227, 11, 10, 47, 28, 201, 230, 222, 13,
        ],
        [
            61, 61, 135, 135, 154, 223, 180, 229, 142, 117, 205, 205, 223, 10, 228, 17, 223, 217,
            10, 10, 10, 28, 20, 54, 11, 10, 28, 28, 40, 114, 61, 160,
        ],
        [
            214, 214, 10, 10, 44, 137, 51, 167, 10, 11, 29, 29, 90, 204, 158, 227, 46, 46, 114,
            114, 201, 230, 222, 13, 95, 120, 190, 190, 228, 31, 11, 47,
        ],
        [
            214, 214, 10, 10, 215, 13, 224, 17, 10, 10, 29, 29, 10, 28, 12, 73, 46, 46, 114, 139,
            40, 114, 61, 161, 95, 120, 190, 210, 94, 186, 115, 226,
        ],
        [
            45, 45, 137, 112, 230, 17, 11, 47, 129, 104, 213, 213, 19, 77, 64, 139, 204, 204, 10,
            10, 85, 180, 118, 203, 10, 10, 38, 42, 145, 226, 211, 10,
        ],
        [
            62, 45, 137, 112, 94, 186, 145, 220, 129, 129, 213, 197, 196, 230, 223, 10, 204, 204,
            10, 10, 229, 26, 10, 30, 10, 10, 38, 38, 14, 65, 33, 106,
        ],
        [
            200, 216, 230, 230, 19, 102, 40, 139, 230, 230, 17, 32, 85, 180, 93, 203, 14, 28, 90,
            90, 145, 226, 198, 10, 107, 107, 200, 200, 223, 23, 230, 44,
        ],
        [
            200, 216, 230, 230, 178, 230, 217, 12, 230, 230, 17, 32, 229, 26, 10, 49, 28, 28, 65,
            90, 14, 65, 37, 126, 112, 107, 182, 200, 64, 183, 110, 214,
        ],
        [
            32, 32, 94, 94, 227, 12, 230, 26, 110, 110, 198, 198, 11, 48, 17, 99, 188, 188, 230,
            230, 57, 130, 89, 183, 230, 230, 46, 23, 135, 214, 10, 37,
        ],
        [
            32, 32, 94, 94, 89, 164, 110, 198, 135, 110, 198, 198, 142, 224, 189, 230, 202, 188,
            230, 230, 222, 13, 229, 27, 230, 230, 40, 23, 13, 61, 130, 214,
        ],
        [
            186, 186, 228, 230, 11, 48, 17, 94, 230, 230, 24, 24, 37, 160, 88, 183, 15, 29, 67, 67,
            110, 222, 10, 37, 89, 114, 183, 183, 13, 89, 44, 135,
        ],
        [
            186, 186, 228, 230, 142, 224, 193, 230, 230, 230, 24, 24, 222, 13, 229, 40, 15, 15, 67,
            92, 10, 61, 130, 222, 89, 114, 183, 183, 183, 229, 214, 13,
        ],
    ];

    /// The same at twice the size: the top left 16 by 8.
    const COLOUR_2X: [[u8; 16]; 8] = [
        [
            56, 56, 56, 56, 159, 129, 159, 129, 23, 13, 89, 88, 44, 44, 135, 110,
        ],
        [
            56, 56, 56, 56, 159, 129, 159, 129, 23, 13, 89, 64, 44, 44, 135, 110,
        ],
        [
            56, 56, 56, 56, 129, 159, 129, 159, 164, 183, 229, 229, 214, 214, 10, 10,
        ],
        [
            56, 56, 56, 56, 129, 159, 129, 159, 183, 183, 229, 229, 214, 214, 10, 13,
        ],
        [
            223, 213, 223, 223, 12, 10, 12, 10, 47, 47, 165, 141, 99, 99, 193, 193,
        ],
        [
            223, 223, 223, 213, 12, 10, 12, 10, 66, 47, 141, 141, 99, 99, 193, 193,
        ],
        [
            213, 223, 223, 223, 10, 12, 10, 12, 224, 224, 11, 14, 230, 230, 32, 32,
        ],
        [
            223, 223, 213, 223, 10, 12, 10, 12, 224, 224, 11, 14, 230, 230, 32, 32,
        ],
    ];

    #[test]
    fn colour_entries_give_windows_98s_8_bit_indexes() {
        let mut d = Decoder::default();
        d.decode(&colour_frame(), 64).unwrap();
        let one = d.indexes(32, 16, 1).unwrap();
        assert_eq!(left::<32>(&one, 32), COLOUR_1X);
        let two = d.indexes(32, 16, 2).unwrap();
        assert_eq!(left::<16>(&two, 64)[..8], COLOUR_2X);
    }
}
