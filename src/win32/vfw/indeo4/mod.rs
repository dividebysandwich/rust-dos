//! Indeo Video Interactive 4 (IV41), the codec of Intel's that Windows
//! 98 installs (IR41_32.AX and IR41_32.DLL): a picture in YVU9 (a luma
//! plane, and two chroma planes of a quarter of its width and height),
//! each plane cut into tiles of macroblocks, coded as transformed and
//! quantized blocks with motion compensation from the frame before. The
//! luma plane can be split into four Haar wavelet bands ("scalability").
//!
//! The decoder follows FFmpeg's (libavcodec/indeo4.c and ivi.c, LGPL 2.1
//! or later):
//!
//! Copyright (c) 2009-2011 Maxim Poliakovski
//!
//! Every size and offset comes from the file, so each is checked before
//! it is used: a frame that doesn't decode is an error, and the picture
//! stays the one before, as with the other codecs (`codec.rs`).

mod dsp;
mod tables;

use dsp::Transform;
use std::sync::{Arc, OnceLock};

/// A Huffman codebook's descriptor: rows of codes, each of `xbits` bits
/// after a prefix of its row's number of ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Desc {
    pub rows: u8,
    pub xbits: [u8; 16],
}

/// A run-value table: what each symbol of a block's codes means, and
/// the symbols that end a block and escape to a run and value coded
/// on their own.
#[derive(Clone, Copy, Debug)]
pub struct RvMap {
    pub eob: u8,
    pub esc: u8,
    pub run: [u8; 256],
    pub val: [i8; 256],
}

/// The frame types.
const INTRA: u32 = 0;
const INTRA1: u32 = 1;
const INTER: u32 = 2;
const BIDIR: u32 = 3;
const NULL_FIRST: u32 = 5;
const NULL_LAST: u32 = 6;

/// The longest code.
const VLC_BITS: u32 = 13;

/// The JPEG zigzag scan.
static ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// The scans by the band header's number: 0 to 4 for 8x8 blocks, 5 to
/// 9 for 4x4 ones, and the rest as FFmpeg guesses them.
fn scan(index: u32) -> &'static [u8] {
    use tables::*;
    match index {
        0 | 4 => &ZIGZAG,
        1 => &ALTERNATE_SCAN_8X8,
        2 => &HORIZONTAL_SCAN_8X8,
        3 => &VERTICAL_SCAN_8X8,
        5 | 9 => &DIRECT_SCAN_4X4,
        6 => &ALTERNATE_SCAN_4X4,
        7 => &VERTICAL_SCAN_4X4,
        8 => &HORIZONTAL_SCAN_4X4,
        _ => &HORIZONTAL_SCAN_8X8,
    }
}

/// Why a frame didn't decode.
pub type Error = String;

fn bad<T>(what: &str) -> Result<T, Error> {
    Err(what.to_string())
}

/// A reader of bits, from the low bit of each byte up, as Indeo's are
/// packed. Past the end it reads zeros.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Bits<'a> {
        Bits { data, pos: 0 }
    }

    /// The next 32 bits or fewer, without reading them.
    fn peek(&self, n: u32) -> u32 {
        let byte = self.pos >> 3;
        let mut word = [0u8; 8];
        let end = (byte + 8).min(self.data.len());
        if byte < end {
            word[..end - byte].copy_from_slice(&self.data[byte..end]);
        }
        let v = u64::from_le_bytes(word) >> (self.pos & 7);
        (v & ((1u64 << n) - 1)) as u32
    }

    fn read(&mut self, n: u32) -> u32 {
        let v = self.peek(n);
        self.pos += n as usize;
        v
    }

    fn bit(&mut self) -> bool {
        self.read(1) != 0
    }

    fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }

    fn count(&self) -> usize {
        self.pos
    }

    fn left(&self) -> isize {
        (self.data.len() * 8) as isize - self.pos as isize
    }
}

/// A Huffman decoding table: for the next 13 bits as the reader peeks
/// them, the symbol and the code's length, 0 where no code starts so.
struct Vlc {
    table: Vec<(u16, u8)>,
}

impl Vlc {
    /// The codebook of a descriptor, None if it has a code too long.
    fn new(desc: &Desc) -> Option<Vlc> {
        let mut table = vec![(0u16, 0u8); 1 << VLC_BITS];
        let mut pos = 0u32;
        for i in 0..desc.rows as u32 {
            let xbits = desc.xbits[i as usize] as u32;
            let not_last = u32::from(i != desc.rows as u32 - 1);
            let prefix = ((1u32 << i) - 1) << (xbits + not_last);
            for j in 0..1u32 << xbits {
                if pos >= 256 {
                    break;
                }
                let len = (i + xbits + not_last).max(1);
                if len > VLC_BITS {
                    return None;
                }
                // The code is read from its high bit first, which the
                // reader gives as the low bit of what it peeks.
                let code = prefix | j;
                let reversed = code.reverse_bits() >> (32 - len);
                for k in 0..1u32 << (VLC_BITS - len) {
                    table[(reversed | k << len) as usize] = (pos as u16, len as u8);
                }
                pos += 1;
            }
        }
        Some(Vlc { table })
    }

    fn read(&self, b: &mut Bits) -> Result<u32, Error> {
        let (sym, len) = self.table[b.peek(VLC_BITS) as usize];
        if len == 0 {
            return bad("invalid code");
        }
        b.skip(len as usize);
        Ok(sym as u32)
    }
}

/// The predefined codebooks: the 8 of macroblocks, then the 8 of
/// blocks.
fn static_vlcs() -> &'static [Arc<Vlc>; 16] {
    static TABLES: OnceLock<[Arc<Vlc>; 16]> = OnceLock::new();
    TABLES.get_or_init(|| {
        std::array::from_fn(|i| {
            let desc = if i < 8 {
                &tables::MB_HUFF[i]
            } else {
                &tables::BLK_HUFF[i - 8]
            };
            Arc::new(Vlc::new(desc).expect("the predefined codebooks are valid"))
        })
    })
}

/// A codebook as a header chooses it: a predefined one (the default, 7,
/// when it isn't coded), or one the header describes.
fn read_huffman(b: &mut Bits, coded: bool, blocks: bool) -> Result<Arc<Vlc>, Error> {
    let base = if blocks { 8 } else { 0 };
    if !coded {
        return Ok(static_vlcs()[base + 7].clone());
    }
    let sel = b.read(3) as usize;
    if sel != 7 {
        return Ok(static_vlcs()[base + sel].clone());
    }
    let rows = b.read(4) as u8;
    if rows == 0 {
        return bad("empty custom Huffman table");
    }
    let mut desc = Desc {
        rows,
        xbits: [0; 16],
    };
    for x in &mut desc.xbits[..rows as usize] {
        *x = b.read(4) as u8;
    }
    Vlc::new(&desc)
        .map(Arc::new)
        .ok_or_else(|| "invalid custom Huffman table".to_string())
}

/// The value of a code whose sign is its low bit.
fn to_signed(v: u32) -> i32 {
    let v = v as i32;
    ((v >> 1) ^ (v & 1).wrapping_neg()).wrapping_neg()
}

/// A motion vector of the luma band scaled to a band of another size.
fn scale_mv(mv: i32, scale: i32) -> i32 {
    (mv + i32::from(mv > 0) + (scale - 1)) >> scale
}

/// A macroblock's information.
#[derive(Clone, Copy, Debug, Default)]
struct Mb {
    xpos: i32,
    ypos: i32,
    offset: usize,
    /// 0 intra, 1 inter, 2 backward, 3 bidirectional.
    kind: u8,
    cbp: u8,
    q_delta: i8,
    mv_x: i8,
    mv_y: i8,
    b_mv_x: i8,
    b_mv_y: i8,
}

#[derive(Clone, Debug, Default)]
struct Tile {
    xpos: i32,
    ypos: i32,
    width: i32,
    height: i32,
    mb_size: i32,
    mbs: Vec<Mb>,
}

/// What a band header sets, which inter frames keep from the frame
/// before.
#[derive(Clone)]
struct Params {
    mb_size: i32,
    blk_size: i32,
    halfpel: bool,
    inherit_mv: bool,
    inherit_qdelta: bool,
    glob_quant: i32,
    transform: Option<Transform>,
    scan: Option<&'static [u8]>,
    quant_mat: usize,
    rvmap_sel: usize,
    corr: Vec<(u8, u8)>,
}

impl Default for Params {
    fn default() -> Params {
        Params {
            mb_size: 0,
            blk_size: 0,
            halfpel: false,
            inherit_mv: false,
            inherit_qdelta: false,
            glob_quant: 0,
            transform: None,
            scan: None,
            quant_mat: 0,
            rvmap_sel: 8,
            corr: Vec::new(),
        }
    }
}

/// A wavelet band of a plane: its size, its buffers (the frame being
/// decoded and the references, by `Decoder::buffers`' numbers) and its
/// tiles.
#[derive(Default)]
struct Band {
    plane: u32,
    number: u32,
    width: i32,
    height: i32,
    pitch: usize,
    aheight: usize,
    bufs: [Vec<i16>; 4],
    params: Params,
    vlc: Option<Arc<Vlc>>,
    tiles: Vec<Tile>,
}

impl Band {
    fn size(&self) -> usize {
        self.pitch * self.aheight
    }
}

#[derive(Default)]
struct Plane {
    width: i32,
    height: i32,
    bands: Vec<Band>,
}

/// The layout of the pictures, which a frame that changes it makes the
/// decoder start again with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Layout {
    width: i32,
    height: i32,
    tile_width: i32,
    tile_height: i32,
    luma_bands: u32,
    chroma_bands: u32,
}

/// A picture: the luma plane, and the two chroma planes of a quarter
/// of its width and height (rounded up), each a byte a pixel.
#[derive(Clone, Debug, Default)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl Picture {
    pub fn chroma_width(&self) -> usize {
        self.width.div_ceil(4)
    }
}

/// The decoder: the planes and what the frames before left.
#[derive(Default)]
pub struct Decoder {
    layout: Layout,
    planes: [Plane; 3],
    frame_type: u32,
    prev_frame_type: u32,
    in_q: bool,
    mb_vlc: Option<Arc<Vlc>>,
    blk_vlc: Option<Arc<Vlc>>,
    rvmaps: Option<Box<[RvMap; 9]>>,
    /// The buffers' roles: where the frame goes, its reference, and the
    /// second reference of a bidirectional frame.
    dst: usize,
    reference: usize,
    b_reference: usize,
    invalid: [bool; 4],
    /// A frame that came after an intra frame in its chunk, which a
    /// NULL_LAST frame shows.
    pending: Option<Picture>,
    picture: Option<Picture>,
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder {
            reference: 1,
            b_reference: 3,
            ..Decoder::default()
        }
    }

    /// The picture the frames so far show.
    pub fn picture(&self) -> Option<&Picture> {
        self.picture.as_ref()
    }

    /// Decodes one frame.
    pub fn decode(&mut self, data: &[u8]) -> Result<(), Error> {
        if self.rvmaps.is_none() {
            self.rvmaps = Some(Box::new(tables::RVMAPS));
        }
        let (shown, _) = self.frame(data, true)?;
        if let Some(p) = shown {
            self.picture = Some(p);
        }
        Ok(())
    }

    /// Decodes the frame in `data`: the picture it shows, if any, and
    /// how many bits it took. `outer` is false for a frame inside an
    /// intra frame's chunk.
    fn frame(&mut self, data: &[u8], outer: bool) -> Result<(Option<Picture>, usize), Error> {
        let mut b = Bits::new(data);
        self.picture_header(&mut b)?;
        if self.frame_type == NULL_LAST {
            return Ok((self.pending.take(), b.count()));
        }
        if self.planes[0].bands.is_empty() {
            return bad("no picture layout yet");
        }
        self.switch_buffers();
        let nonnull = self.frame_type < NULL_FIRST;
        if nonnull {
            self.invalid[self.dst] = true;
            for p in 0..3 {
                for i in 0..self.planes[p].bands.len() {
                    self.band(&mut b, p, i)?;
                }
            }
            self.invalid[self.dst] = false;
        } else {
            if self.layout.luma_bands != 1 {
                return bad("a null frame in scalable mode");
            }
            if self
                .planes
                .iter()
                .any(|p| p.bands[0].bufs[self.dst].is_empty())
            {
                return bad("a null frame before any picture");
            }
        }
        if self.invalid[self.dst] {
            return bad("the frame's buffer is invalid");
        }
        if !nonnull {
            return Ok((None, b.count()));
        }
        let picture = self.output();
        if outer && self.frame_type == INTRA {
            // The version string, then padding to 64 bits; then maybe
            // the inter frame that goes with a bidirectional sequence,
            // which a NULL_LAST frame shows.
            while b.read(8) != 0 {
                if b.left() < 8 {
                    return bad("unterminated version string");
                }
            }
            let left = b.count() & 0x18;
            b.skip(64 - left);
            if b.left() > 18 && b.peek(21) == 0xB_FFF8 {
                let at = b.count() / 8;
                let rest = &data[at.min(data.len())..];
                self.pending = None;
                if let Ok((p, _)) = self.frame(rest, false) {
                    self.pending = p;
                }
            }
        }
        Ok((Some(picture), b.count()))
    }

    fn picture_header(&mut self, b: &mut Bits) -> Result<(), Error> {
        if b.read(18) != 0x3_FFF8 {
            return bad("no picture start code");
        }
        self.prev_frame_type = self.frame_type;
        self.frame_type = b.read(3);
        if self.frame_type == 7 {
            return bad("invalid frame type");
        }
        let _transparency = b.bit();
        if b.bit() {
            return bad("sync bit set");
        }
        if b.bit() {
            b.skip(24); // the data size
        }
        if self.frame_type >= NULL_FIRST {
            return Ok(());
        }
        if b.bit() {
            b.skip(32); // the key lock word
        }
        let size = b.read(3);
        let (height, width) = if size == 7 {
            (b.read(16) as i32, b.read(16) as i32)
        } else {
            let s = size as usize * 2;
            (tables::PIC_SIZES[s + 1] as i32, tables::PIC_SIZES[s] as i32)
        };
        let tiled = b.bit();
        let scale = |def: i32, f: u32| if f == 15 { def } else { (f as i32 + 1) << 5 };
        let (tile_height, tile_width) = if tiled {
            let h = scale(height, b.read(4));
            (h, scale(width, b.read(4)))
        } else {
            (height, width)
        };
        if b.read(2) != 0 {
            return bad("chroma other than YVU9");
        }
        let luma_bands = subdivision(b);
        let chroma_bands = if luma_bands != 0 { subdivision(b) } else { 0 };
        if !(1..=4096).contains(&width) || !(1..=4096).contains(&height) {
            return bad("picture size out of range");
        }
        if !(luma_bands == 1 && chroma_bands == 1 || luma_bands == 4 && chroma_bands == 1) {
            return bad("unsupported band subdivision");
        }
        let layout = Layout {
            width,
            height,
            tile_width,
            tile_height,
            luma_bands,
            chroma_bands,
        };
        if layout != self.layout {
            self.init(layout)?;
        }
        if b.bit() {
            b.skip(20); // the frame number
        }
        if b.bit() {
            b.skip(8); // the decoding time estimate
        }
        let coded = b.bit();
        self.mb_vlc = Some(read_huffman(b, coded, false)?);
        let coded = b.bit();
        self.blk_vlc = Some(read_huffman(b, coded, true)?);
        let _rvmap_sel = if b.bit() { b.read(3) } else { 8 };
        let _in_imf = b.bit();
        self.in_q = b.bit();
        let _glob_quant = b.read(5);
        if b.bit() {
            b.skip(3);
        }
        if b.bit() {
            b.skip(16); // the checksum
        }
        while b.bit() {
            if b.left() < 10 {
                return bad("picture header extension runs out");
            }
            b.skip(8);
        }
        let _bad_blocks = b.bit();
        b.align();
        Ok(())
    }

    /// Starts again with the planes, bands and tiles of `layout`.
    fn init(&mut self, layout: Layout) -> Result<(), Error> {
        // As FFmpeg, the layout is kept only once it is set up.
        self.layout = Layout::default();
        let scalable = layout.luma_bands != 1;
        for p in 0..3 {
            let (w, h, n) = if p == 0 {
                (layout.width, layout.height, layout.luma_bands)
            } else {
                (
                    (layout.width + 3) >> 2,
                    (layout.height + 3) >> 2,
                    layout.chroma_bands,
                )
            };
            let (bw, bh) = if n == 1 {
                (w, h)
            } else {
                ((w + 1) >> 1, (h + 1) >> 1)
            };
            let align = if p == 0 { 16 } else { 8 };
            let pitch = (bw as usize).next_multiple_of(align);
            let aheight = (bh as usize).next_multiple_of(align);
            let plane = &mut self.planes[p];
            plane.width = w;
            plane.height = h;
            plane.bands = (0..n)
                .map(|i| Band {
                    plane: p as u32,
                    number: i,
                    width: bw,
                    height: bh,
                    pitch,
                    aheight,
                    params: Params {
                        mb_size: if p == 0 {
                            if scalable { 8 } else { 16 }
                        } else {
                            4
                        },
                        blk_size: if p == 0 { 8 } else { 4 },
                        ..Params::default()
                    },
                    ..Band::default()
                })
                .collect();
        }
        self.invalid = [false; 4];
        for p in 0..3 {
            let mut tw = if p == 0 {
                layout.tile_width
            } else {
                (layout.tile_width + 3) >> 2
            };
            let mut th = if p == 0 {
                layout.tile_height
            } else {
                (layout.tile_height + 3) >> 2
            };
            if p == 0 && layout.luma_bands == 4 {
                if tw % 2 != 0 || th % 2 != 0 {
                    return bad("odd tiles");
                }
                tw >>= 1;
                th >>= 1;
            }
            if tw <= 0 || th <= 0 {
                return bad("empty tiles");
            }
            for i in 0..self.planes[p].bands.len() {
                let band = &self.planes[p].bands[i];
                let mut tiles = Vec::new();
                let mb = band.params.mb_size;
                for y in (0..band.height).step_by(th as usize) {
                    for x in (0..band.width).step_by(tw as usize) {
                        let width = (band.width - x).min(tw);
                        let height = (band.height - y).min(th);
                        let count = ((width + mb - 1) / mb) * ((height + mb - 1) / mb);
                        tiles.push(Tile {
                            xpos: x,
                            ypos: y,
                            width,
                            height,
                            mb_size: mb,
                            mbs: vec![Mb::default(); count as usize],
                        });
                    }
                }
                if p != 0 || i != 0 {
                    let reference = &self.planes[0].bands[0].tiles;
                    if reference.len() < tiles.len()
                        || tiles
                            .iter()
                            .zip(reference)
                            .any(|(t, r)| t.mbs.len() != r.mbs.len())
                    {
                        return bad("tiles don't match the luma band's");
                    }
                }
                self.planes[p].bands[i].tiles = tiles;
            }
        }
        self.layout = layout;
        Ok(())
    }

    fn switch_buffers(&mut self) {
        let is_ref = |t: u32| matches!(t, INTRA | INTRA1 | INTER);
        let (prev, now) = (is_ref(self.prev_frame_type), is_ref(self.frame_type));
        if prev && now {
            std::mem::swap(&mut self.dst, &mut self.reference);
        } else if prev {
            std::mem::swap(&mut self.reference, &mut self.b_reference);
            std::mem::swap(&mut self.dst, &mut self.reference);
        }
    }

    /// Decodes the band `i` of the plane `p`.
    fn band(&mut self, b: &mut Bits, p: usize, i: usize) -> Result<(), Error> {
        let mut band = std::mem::take(&mut self.planes[p].bands[i]);
        let result = self.decode_band(b, &mut band);
        self.planes[p].bands[i] = band;
        result
    }

    fn decode_band(&mut self, b: &mut Bits, band: &mut Band) -> Result<(), Error> {
        let size = band.size();
        let (dst, ref1, ref2) = if self.frame_type == BIDIR {
            (self.dst, self.b_reference, Some(self.reference))
        } else {
            (self.dst, self.reference, None)
        };
        for k in [Some(dst), Some(ref1), ref2].into_iter().flatten() {
            if self.layout.luma_bands <= 1 && k == 2 {
                return bad("no buffer for the band");
            }
            if band.bufs[k].len() != size {
                band.bufs[k] = vec![0; size];
            }
        }
        self.band_header(b, band)?;
        let rvmaps = self.rvmaps.as_mut().expect("made in decode");
        let map = &mut rvmaps[band.params.rvmap_sel];
        apply_corrections(map, &band.params.corr, false);
        let map_copy = *map;
        let result = self.band_tiles(b, band, &map_copy, dst, ref1, ref2);
        let rvmaps = self.rvmaps.as_mut().expect("made in decode");
        apply_corrections(&mut rvmaps[band.params.rvmap_sel], &band.params.corr, true);
        b.align();
        result
    }

    fn band_header(&mut self, b: &mut Bits, band: &mut Band) -> Result<(), Error> {
        let plane = b.read(2);
        let number = b.read(4);
        if plane != band.plane || number != band.number {
            return bad("band headers out of order");
        }
        if b.bit() {
            return bad("an empty band");
        }
        let mut params = band.params.clone();
        let old_blk_size = params.blk_size;
        if b.bit() {
            b.skip(16); // the header's size
        }
        let halfpel = b.read(2);
        if halfpel >= 2 {
            return bad("motion vector resolution");
        }
        params.halfpel = halfpel == 1;
        if b.bit() {
            b.skip(16); // the checksum
        }
        let indx = b.read(2);
        if indx == 3 {
            return bad("block size");
        }
        params.mb_size = 16 >> indx;
        params.blk_size = 8 >> (indx >> 1);
        params.inherit_mv = b.bit();
        params.inherit_qdelta = b.bit();
        params.glob_quant = b.read(5) as i32;
        if !b.bit() || self.frame_type == INTRA {
            let id = b.read(5) as usize;
            let Some(transform) = dsp::TRANSFORMS.get(id).copied().flatten() else {
                return bad("unsupported transform");
            };
            if id < 10 && params.blk_size < 8 {
                return bad("transform size");
            }
            if params.blk_size as usize != transform.size {
                return bad("transform and block sizes differ");
            }
            params.transform = Some(transform);
            let scan_index = b.read(4);
            if scan_index == 15 {
                return bad("custom scan");
            }
            let four = scan_index > 4 && scan_index < 10;
            if four != (params.blk_size == 4) {
                return bad("scan and block sizes differ");
            }
            params.scan = Some(scan(scan_index));
            let quant_mat = b.read(5) as usize;
            if quant_mat >= tables::QUANT_INDEX_TO_TAB.len() {
                return bad("quantization matrix");
            }
            params.quant_mat = quant_mat;
        } else if old_blk_size != params.blk_size {
            return bad("block size differs from the inherited one");
        }
        if tables::QUANT_INDEX_TO_TAB[params.quant_mat] > 4 && params.blk_size == 4 {
            band.params.quant_mat = 0;
            return bad("quantization matrix for 4x4 blocks");
        }
        let Some(scan) = params.scan else {
            return bad("no scan");
        };
        if scan.len() != (params.blk_size * params.blk_size) as usize {
            return bad("scan and block sizes differ");
        }
        if params.transform.map(|t| t.size) != Some(params.blk_size as usize) {
            return bad("transform and block sizes differ");
        }
        let vlc = if b.bit() {
            read_huffman(b, true, true)?
        } else {
            self.blk_vlc.clone().ok_or("no block codebook")?
        };
        params.rvmap_sel = if b.bit() { b.read(3) as usize } else { 8 };
        params.corr.clear();
        if b.bit() {
            let n = b.read(8);
            if n > 61 {
                return bad("too many corrections");
            }
            for _ in 0..n {
                let a = b.read(8) as u8;
                params.corr.push((a, b.read(8) as u8));
            }
        }
        b.align();
        band.params = params;
        band.vlc = Some(vlc);
        Ok(())
    }

    fn band_tiles(
        &mut self,
        b: &mut Bits,
        band: &mut Band,
        map: &RvMap,
        dst: usize,
        ref1: usize,
        ref2: Option<usize>,
    ) -> Result<(), Error> {
        let mut pos = b.count();
        let luma_mb = if band.plane == 0 && band.number == 0 {
            band.params.mb_size
        } else {
            self.planes[0].bands[0].params.mb_size
        };
        let mv_scale = (luma_mb >> 3) - (band.params.mb_size >> 3);
        let mut out = std::mem::take(&mut band.bufs[dst]);
        let result = (|| {
            for t in 0..band.tiles.len() {
                if band.tiles[t].mb_size != band.params.mb_size {
                    return bad("macroblock sizes differ");
                }
                let reference = if band.plane == 0 && band.number == 0 {
                    None
                } else {
                    Some(self.planes[0].bands[0].tiles[t].mbs.clone())
                };
                if b.bit() {
                    empty_tile(band, t, reference.as_deref(), mv_scale, &mut out, ref1)?;
                } else {
                    let size = tile_data_size(b);
                    if size == 0 {
                        return bad("tile data size is zero");
                    }
                    self.mb_info(b, band, t, reference.as_deref(), mv_scale)?;
                    blocks(b, band, t, map, &mut out, ref1, ref2)?;
                    if (b.count() - pos) >> 3 != size {
                        return bad("tile data size mismatch");
                    }
                    pos += size << 3;
                }
            }
            Ok(())
        })();
        band.bufs[dst] = out;
        result
    }

    fn mb_info(
        &self,
        b: &mut Bits,
        band: &mut Band,
        t: usize,
        reference: Option<&[Mb]>,
        mv_scale: i32,
    ) -> Result<(), Error> {
        let mb_vlc = self.mb_vlc.as_ref().ok_or("no macroblock codebook")?;
        let prm = &band.params;
        let mb_size = prm.mb_size;
        let pitch = band.pitch as i32;
        let bufsize = band.size() as i32;
        let blks_per_mb = if prm.mb_size != prm.blk_size { 4 } else { 1 };
        let type_bits = if self.frame_type == BIDIR { 2 } else { 1 };
        let luma0 = band.plane == 0 && band.number == 0;
        let tile = &mut band.tiles[t];
        let count =
            ((tile.width + mb_size - 1) / mb_size) * ((tile.height + mb_size - 1) / mb_size);
        if count as usize != tile.mbs.len() {
            return bad("macroblock count mismatch");
        }
        let inherited = |r: &Mb| -> (i8, i8) {
            if mv_scale != 0 {
                (
                    scale_mv(r.mv_x as i32, mv_scale) as i8,
                    scale_mv(r.mv_y as i32, mv_scale) as i8,
                )
            } else {
                (r.mv_x, r.mv_y)
            }
        };
        let (mut mv_x, mut mv_y) = (0i32, 0i32);
        let mut offs = tile.ypos * pitch + tile.xpos;
        let mut k = 0;
        for y in (tile.ypos..tile.ypos + tile.height).step_by(mb_size as usize) {
            let mut mb_offset = offs;
            for x in (tile.xpos..tile.xpos + tile.width).step_by(mb_size as usize) {
                let r = reference.and_then(|r| r.get(k)).copied();
                let mut mb = Mb {
                    xpos: x,
                    ypos: y,
                    offset: mb_offset as usize,
                    ..Mb::default()
                };
                if b.left() < 1 {
                    return bad("macroblock information runs out");
                }
                if b.bit() {
                    if self.frame_type == INTRA {
                        return bad("an empty macroblock in an intra frame");
                    }
                    mb.kind = 1;
                    mb.cbp = 0;
                    if luma0 && self.in_q {
                        mb.q_delta = to_signed(mb_vlc.read(b)?) as i8;
                    }
                    if let (true, Some(r)) = (prm.inherit_mv, r) {
                        (mb.mv_x, mb.mv_y) = inherited(&r);
                    }
                } else {
                    if prm.inherit_mv {
                        let Some(r) = r else {
                            return bad("no reference macroblock");
                        };
                        mb.kind = r.kind;
                    } else if matches!(self.frame_type, INTRA | INTRA1) {
                        mb.kind = 0;
                    } else {
                        mb.kind = b.read(type_bits) as u8;
                    }
                    mb.cbp = b.read(blks_per_mb) as u8;
                    if prm.inherit_qdelta {
                        if let Some(r) = r {
                            mb.q_delta = r.q_delta;
                        }
                    } else if mb.cbp != 0 || luma0 && self.in_q {
                        mb.q_delta = to_signed(mb_vlc.read(b)?) as i8;
                    }
                    if mb.kind != 0 {
                        if prm.inherit_mv {
                            if let Some(r) = r {
                                (mb.mv_x, mb.mv_y) = inherited(&r);
                            }
                        } else {
                            mv_y = mv_y.wrapping_add(to_signed(mb_vlc.read(b)?));
                            mv_x = mv_x.wrapping_add(to_signed(mb_vlc.read(b)?));
                            mb.mv_x = mv_x as i8;
                            mb.mv_y = mv_y as i8;
                            if mb.kind == 3 {
                                mv_y = mv_y.wrapping_add(to_signed(mb_vlc.read(b)?));
                                mv_x = mv_x.wrapping_add(to_signed(mb_vlc.read(b)?));
                                mb.b_mv_x = mv_x.wrapping_neg() as i8;
                                mb.b_mv_y = mv_y.wrapping_neg() as i8;
                            }
                        }
                        if mb.kind == 2 {
                            mb.b_mv_x = mb.mv_x.wrapping_neg();
                            mb.b_mv_y = mb.mv_y.wrapping_neg();
                            mb.mv_x = 0;
                            mb.mv_y = 0;
                        }
                    }
                }
                let s = u32::from(prm.halfpel);
                if mb.kind != 0 {
                    let (mx, my) = (mb.mv_x as i32, mb.mv_y as i32);
                    let first = x + (mx >> s) + (y + (my >> s)) * pitch;
                    let last = x + ((mx + s as i32) >> s) + mb_size - 1
                        + (y + mb_size - 1 + ((my + s as i32) >> s)) * pitch;
                    if first < 0 || last > bufsize - 1 {
                        return bad("motion vector outside the reference");
                    }
                }
                tile.mbs[k] = mb;
                k += 1;
                mb_offset += mb_size;
            }
            offs += mb_size * pitch;
        }
        b.align();
        Ok(())
    }

    /// The decoded planes as a picture.
    fn output(&self) -> Picture {
        let (w, h) = (self.layout.width as usize, self.layout.height as usize);
        let mut y = vec![0u8; w * h];
        let luma = &self.planes[0];
        if self.layout.luma_bands == 4 {
            let pitch = luma.bands[0].pitch;
            let bands: [&[i16]; 4] =
                std::array::from_fn(|i| luma.bands[i].bufs[self.dst].as_slice());
            // The recomposition writes pixel pairs: to an even size.
            let (ew, eh) = (w.next_multiple_of(2), h.next_multiple_of(2));
            let mut even = vec![0u8; ew * eh];
            if bands.iter().all(|b| b.len() >= pitch * eh / 2) {
                dsp::recompose_haar(bands, pitch, w, h, &mut even, ew);
            }
            for row in 0..h {
                y[row * w..row * w + w].copy_from_slice(&even[row * ew..row * ew + w]);
            }
        } else {
            plane_out(&luma.bands[0], self.dst, w, h, &mut y);
        }
        let (cw, ch) = (w.div_ceil(4), h.div_ceil(4));
        let mut u = vec![0u8; cw * ch];
        let mut v = vec![0u8; cw * ch];
        // Plane 1 is V and plane 2 is U: the planes of YVU9.
        plane_out(&self.planes[2].bands[0], self.dst, cw, ch, &mut u);
        plane_out(&self.planes[1].bands[0], self.dst, cw, ch, &mut v);
        Picture {
            width: w,
            height: h,
            y,
            u,
            v,
        }
    }
}

/// The number of bands of a plane, by its subdivision code: 1, 4, or 0
/// for one not supported.
fn subdivision(b: &mut Bits) -> u32 {
    match b.read(2) {
        3 => 1,
        2 => {
            let mut ok = true;
            for _ in 0..4 {
                ok &= b.read(2) == 3;
            }
            if ok { 4 } else { 0 }
        }
        _ => 0,
    }
}

/// A tile's size in bytes: one byte, or 255 and three more.
fn tile_data_size(b: &mut Bits) -> usize {
    let mut len = 0;
    if b.bit() {
        len = b.read(8);
        if len == 255 {
            len = b.read(24);
        }
    }
    b.align();
    len as usize
}

/// Swaps the run-value table's pairs of entries a band header lists,
/// or swaps them back (`undo`, in reverse order).
fn apply_corrections(map: &mut RvMap, corr: &[(u8, u8)], undo: bool) {
    let mut swap = |&(a, b): &(u8, u8)| {
        let (a, b) = (a as usize, b as usize);
        map.run.swap(a, b);
        map.val.swap(a, b);
        let (a, b) = (a as u8, b as u8);
        if a == map.eob || b == map.eob {
            map.eob ^= a ^ b;
        }
        if a == map.esc || b == map.esc {
            map.esc ^= a ^ b;
        }
    };
    if undo {
        corr.iter().rev().for_each(&mut swap);
    } else {
        corr.iter().for_each(&mut swap);
    }
}

/// A plane's band 0 with its bias of 128 back, `w` by `h`.
fn plane_out(band: &Band, buf: usize, w: usize, h: usize, dst: &mut [u8]) {
    let src = &band.bufs[buf];
    if src.len() < band.pitch * h.saturating_sub(1) + w {
        return;
    }
    for y in 0..h {
        let row = &src[y * band.pitch..y * band.pitch + w];
        for (d, &s) in dst[y * w..y * w + w].iter_mut().zip(row) {
            *d = (s as i32 + 128).clamp(0, 255) as u8;
        }
    }
}

/// The motion compensation of one block: the prediction from `ref1`
/// (or from `ref2` alone, or the average of the two), put into `out`
/// or added to what is there.
#[allow(clippy::too_many_arguments)]
fn motion(
    band: &Band,
    out: &mut [i16],
    offs: usize,
    ref1: usize,
    ref2: Option<usize>,
    mv: (i32, i32, u32),
    mv2: Option<(i32, i32, u32)>,
    first: bool,
    add: bool,
) -> Result<(), Error> {
    let n = band.params.blk_size as usize;
    let pitch = band.pitch as i32;
    let at = |(x, y, _): (i32, i32, u32)| -> Result<usize, Error> {
        let r = offs as i32 + y * pitch + x;
        usize::try_from(r).map_err(|_| "motion vector before the buffer".to_string())
    };
    let pred = |buf: usize, v: (i32, i32, u32)| -> Result<[i16; 64], Error> {
        dsp::predict(&band.bufs[buf], at(v)?, band.pitch, n, v.2)
            .ok_or_else(|| "motion vector past the buffer".to_string())
    };
    let block = match (first, mv2) {
        (true, None) => pred(ref1, mv)?,
        (false, Some(v2)) => pred(ref2.ok_or("no second reference")?, v2)?,
        (true, Some(v2)) => dsp::average(
            &pred(ref1, mv)?,
            &pred(ref2.ok_or("no second reference")?, v2)?,
            n,
        ),
        (false, None) => return Ok(()),
    };
    if !dsp::put(out, offs, band.pitch, n, &block, add) {
        return bad("block past the buffer");
    }
    Ok(())
}

/// Decodes the blocks of the tile `t`'s macroblocks into `out`.
fn blocks(
    b: &mut Bits,
    band: &Band,
    t: usize,
    map: &RvMap,
    out: &mut [i16],
    ref1: usize,
    ref2: Option<usize>,
) -> Result<(), Error> {
    let prm = &band.params;
    let vlc = band.vlc.as_ref().ok_or("no block codebook")?;
    let transform = prm.transform.ok_or("no transform")?;
    let scan = prm.scan.ok_or("no scan")?;
    let blk = prm.blk_size as usize;
    let pitch = band.pitch;
    let num_blocks = if prm.mb_size as usize != blk { 4 } else { 1 };
    let quant_tab = tables::QUANT_INDEX_TO_TAB[prm.quant_mat] as usize;
    let (intra_base, inter_base): (&[u16], &[u16]) = if blk == 8 {
        (
            &tables::QUANT_8X8_INTRA[quant_tab],
            &tables::QUANT_8X8_INTER[quant_tab],
        )
    } else {
        (
            &tables::QUANT_4X4_INTRA[quant_tab.min(4)],
            &tables::QUANT_4X4_INTER[quant_tab.min(4)],
        )
    };
    let mut prev_dc = 0i32;
    let halfpel = u32::from(prm.halfpel);
    for mb in &band.tiles[t].mbs {
        let intra = mb.kind == 0;
        let quant = (prm.glob_quant + mb.q_delta as i32).clamp(0, 31) as u32;
        // The vectors in whole pixels and their half-pixel kinds.
        let split = |x: i8, y: i8| -> (i32, i32, u32) {
            let (x, y) = (x as i32, y as i32);
            if prm.halfpel {
                (x >> 1, y >> 1, (((y & 1) << 1) | (x & 1)) as u32)
            } else {
                (x, y, 0)
            }
        };
        let (mv, mv2) = if intra {
            ((0, 0, 0), None)
        } else {
            for (x, y) in [(mb.mv_x, mb.mv_y), (mb.b_mv_x, mb.b_mv_y)]
                .into_iter()
                .take(if matches!(mb.kind, 2 | 3) { 2 } else { 1 })
                .skip(if mb.kind == 2 { 1 } else { 0 })
            {
                let (dx, dy) = ((x as i32) >> halfpel, (y as i32) >> halfpel);
                let (cx, cy) = ((x as i32) & halfpel as i32, (y as i32) & halfpel as i32);
                if mb.xpos + dx < 0
                    || mb.xpos + dx + prm.mb_size + cx > pitch as i32
                    || mb.ypos + dy < 0
                    || mb.ypos + dy + prm.mb_size + cy > band.aheight as i32
                {
                    return bad("motion vector out of bounds");
                }
            }
            let second = matches!(mb.kind, 2 | 3).then(|| split(mb.b_mv_x, mb.b_mv_y));
            (split(mb.mv_x, mb.mv_y), second)
        };
        let first = mb.kind != 2;
        let mut cbp = mb.cbp;
        let mut offs = mb.offset;
        for k in 0..num_blocks {
            if k & 1 == 1 {
                offs += blk;
            } else if k == 2 {
                offs = offs - blk + blk * pitch;
            }
            if offs + (blk - 1) * pitch + blk > band.size() {
                return bad("block past the buffer");
            }
            if cbp & 1 != 0 {
                let base = if intra { intra_base } else { inter_base };
                let mut coeffs = [0i32; 64];
                let mut flags = [0u8; 8];
                let n = blk * blk;
                let mut scan_pos: i32 = -1;
                let mut sym;
                loop {
                    sym = vlc.read(b)?;
                    if sym == map.eob as u32 {
                        break;
                    }
                    let (run, val) = if sym == map.esc as u32 {
                        let run = vlc.read(b)? as i32 + 1;
                        let lo = vlc.read(b)?;
                        let hi = vlc.read(b)?;
                        (run, to_signed((hi << 6) | lo))
                    } else {
                        if sym >= 256 {
                            return bad("invalid symbol");
                        }
                        (map.run[sym as usize] as i32, map.val[sym as usize] as i32)
                    };
                    scan_pos += run;
                    if scan_pos >= n as i32 || scan_pos < 0 {
                        break;
                    }
                    let pos = scan[scan_pos as usize] as usize;
                    let q = (base[pos] as u32 * quant) >> 9;
                    let mut val = val;
                    if q > 1 {
                        let sign: i32 = if val > 0 { 1 } else { -1 };
                        val = val
                            .wrapping_mul(q as i32)
                            .wrapping_add(sign.wrapping_mul((((q ^ 1) - 1) >> 1) as i32));
                    }
                    // Kept within what a 16-bit picture can use, so that
                    // the transforms can't overflow on a damaged frame.
                    coeffs[pos] = val.clamp(-(1 << 20), 1 << 20);
                    flags[pos & (blk - 1)] |= u8::from(val != 0);
                    if scan_pos > n as i32 {
                        break;
                    }
                }
                if scan_pos < 0 || scan_pos >= n as i32 && sym != map.eob as u32 {
                    return bad("corrupt block");
                }
                if intra && transform.two_d {
                    prev_dc = prev_dc.wrapping_add(coeffs[0]).clamp(-(1 << 20), 1 << 20);
                    coeffs[0] = prev_dc;
                    flags[0] |= u8::from(prev_dc != 0);
                }
                let mut px = [0i16; 64];
                (transform.inverse)(&coeffs[..n], &flags, &mut px);
                dsp::put(out, offs, pitch, blk, &px[..n], false);
                if !intra {
                    motion(band, out, offs, ref1, ref2, mv, mv2, first, true)?;
                }
            } else if intra {
                let mut px = [0i16; 64];
                (transform.dc)(prev_dc, blk, &mut px);
                dsp::put(out, offs, pitch, blk, &px[..blk * blk], false);
            } else {
                motion(band, out, offs, ref1, ref2, mv, mv2, first, false)?;
            }
            cbp >>= 1;
        }
    }
    b.align();
    Ok(())
}

/// A tile with no data: its macroblocks inherit what the luma band's
/// have, and it is the reference moved by their vectors, or copied.
fn empty_tile(
    band: &mut Band,
    t: usize,
    reference: Option<&[Mb]>,
    mv_scale: i32,
    out: &mut [i16],
    ref1: usize,
) -> Result<(), Error> {
    let prm = band.params.clone();
    let mb_size = prm.mb_size;
    let pitch = band.pitch as i32;
    let clear_first = band.plane == 0 && band.number == 0;
    let aheight = band.aheight as i32;
    let tile = &mut band.tiles[t];
    let count = ((tile.width + mb_size - 1) / mb_size) * ((tile.height + mb_size - 1) / mb_size);
    if count as usize != tile.mbs.len() {
        return bad("macroblock count mismatch");
    }
    let mut need_mc = false;
    let mut offs = tile.ypos * pitch + tile.xpos;
    let mut k = 0;
    for y in (tile.ypos..tile.ypos + tile.height).step_by(mb_size as usize) {
        let mut mb_offset = offs;
        for x in (tile.xpos..tile.xpos + tile.width).step_by(mb_size as usize) {
            let mb = &mut tile.mbs[k];
            mb.xpos = x;
            mb.ypos = y;
            mb.offset = mb_offset as usize;
            mb.kind = 1;
            mb.cbp = 0;
            if clear_first {
                mb.q_delta = prm.glob_quant as i8;
                mb.mv_x = 0;
                mb.mv_y = 0;
            }
            if let Some(r) = reference.and_then(|r| r.get(k)) {
                if prm.inherit_qdelta {
                    mb.q_delta = r.q_delta;
                }
                if prm.inherit_mv {
                    if mv_scale != 0 {
                        mb.mv_x = scale_mv(r.mv_x as i32, mv_scale) as i8;
                        mb.mv_y = scale_mv(r.mv_y as i32, mv_scale) as i8;
                    } else {
                        mb.mv_x = r.mv_x;
                        mb.mv_y = r.mv_y;
                    }
                    need_mc |= mb.mv_x != 0 || mb.mv_y != 0;
                    let s = u32::from(prm.halfpel);
                    let (dx, dy) = ((mb.mv_x as i32) >> s, (mb.mv_y as i32) >> s);
                    let (cx, cy) = ((mb.mv_x as i32) & s as i32, (mb.mv_y as i32) & s as i32);
                    if x + dx < 0
                        || x + dx + mb_size + cx > pitch
                        || y + dy < 0
                        || y + dy + mb_size + cy > aheight
                    {
                        return bad("motion vector out of bounds");
                    }
                }
            }
            k += 1;
            mb_offset += mb_size;
        }
        offs += mb_size * pitch;
    }
    let band = &*band;
    let tile = &band.tiles[t];
    if prm.inherit_mv && need_mc {
        let blk = prm.blk_size as usize;
        let num_blocks = if mb_size as usize != blk { 4 } else { 1 };
        for mb in &tile.mbs {
            let (mut x, mut y) = (mb.mv_x as i32, mb.mv_y as i32);
            let kind = if prm.halfpel {
                let k = (((y & 1) << 1) | (x & 1)) as u32;
                x >>= 1;
                y >>= 1;
                k
            } else {
                0
            };
            for k in 0..num_blocks {
                let o = mb.offset + blk * ((k & 1) + usize::from(k & 2 != 0) * band.pitch);
                motion(band, out, o, ref1, None, (x, y, kind), None, true, false)?;
            }
        }
    } else {
        let src = &band.bufs[ref1];
        for row in 0..tile.height as usize {
            let at = (tile.ypos as usize + row) * band.pitch + tile.xpos as usize;
            let w = tile.width as usize;
            if at + w > src.len() || at + w > out.len() {
                return bad("tile past the buffer");
            }
            out[at..at + w].copy_from_slice(&src[at..at + w]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_come_from_the_low_bit_up() {
        let mut b = Bits::new(&[0xF8, 0xFF, 0x03]);
        assert_eq!(b.read(18), 0x3_FFF8);
        assert_eq!(b.read(8), 0);
        assert_eq!(b.left(), -2);
    }

    #[test]
    fn codes_are_read_from_their_high_bit() {
        // Descriptor {2 rows: 1 bit, 1 bit}: codes 00, 01 (row 0, with
        // its 0) and 10, 11 (row 1, the last, no 0).
        let desc = Desc {
            rows: 2,
            xbits: [1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        };
        let vlc = Vlc::new(&desc).unwrap();
        // The bits 1, 1 (symbol 3) then 0, 1 (symbol 1), low bit first.
        let mut b = Bits::new(&[0b1011]);
        assert_eq!(vlc.read(&mut b).unwrap(), 3);
        assert_eq!(vlc.read(&mut b).unwrap(), 1);
    }

    #[test]
    fn signed_codes_have_their_sign_in_the_low_bit() {
        assert_eq!([0, 1, 2, 3, 4].map(to_signed), [0, 1, -1, 2, -2]);
    }

    #[test]
    fn the_predefined_codebooks_build() {
        assert_eq!(static_vlcs().len(), 16);
    }

    #[test]
    fn a_frame_without_a_start_code_is_refused() {
        let mut d = Decoder::new();
        assert!(d.decode(&[0; 16]).is_err());
        assert!(d.picture().is_none());
        // A null frame before any picture.
        assert!(d.decode(&[0xF8, 0xFF, 0x17, 0]).is_err());
    }
}
