//! Complete IV50 picture decoder that reproduces the vendor driver's
//! internal 8-bit planes.
//!
//! [`Iv50Decoder`] takes one compressed frame at a time (one AVI video
//! chunk) and keeps the state that spans frames: the GOP header, three
//! sets of band buffers that rotate by frame type (spec 07, 7.2), the
//! luma band 0 macroblock records that later bands inherit from (spec
//! 03, section 6), the transparency bounding box, and the output planes,
//! which persist because a transparency frame only rebuilds its bounding
//! box (spec 08, 8.3).
//!
//! A frame is decoded in this order: picture start (spec 01), GOP header
//! for intra frames and frame header (spec 02), every band in bitstream
//! order (luma bands, then the bands of chroma plane 1 (V), then those of
//! chroma plane 2 (U)), the transparency trailer, the buffer rotation and
//! the recombination of the bands into the 8-bit planes (spec 08).

mod band;
mod bits;
mod headers;
mod huff;
mod synth;
mod tables;
mod xform;

use band::{B0Rec, BandGeom, BandJob, Bank, TileGeom, build_bank, fill_empty_band};
use bits::PaddedFrame;
use headers::GopHeader;
use huff::Huff;
use synth::{OutPlane, Window};

/// Largest picture area, in luma pixels, the decoder accepts. A custom
/// picture size can ask for up to 8191 by 8191; larger pictures than this
/// are refused before any buffer is allocated.
pub const MAX_PICTURE_PIXELS: u64 = 4096 * 4096;

/// The type of a decoded frame (spec 01, "Frame types").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FrameType {
    /// Type 0: every macroblock is intra; carries the GOP header. Becomes
    /// the reference of the following type 1 frames.
    Intra,
    /// Type 1: predicted from the last type 0 or 1 frame, and becomes the
    /// reference of the following frames in its place.
    Inter,
    /// Type 2: predicted from the last type 2 frame since the last type 0
    /// or 1 frame (else from that frame). Later type 2 and 3 frames
    /// predict from it, type 0 and 1 frames never do.
    DroppableInterScalability,
    /// Type 3: predicted like a type 2 frame and never used as a
    /// reference.
    DroppableInter,
    /// Type 4, or a predicted frame that repeats the previous frame's
    /// number: no picture data; the picture stays as it was.
    Null,
}

/// Errors of [`Iv50Decoder::decode`]. Every one of them means the frame
/// was not decoded, and the planes keep the previous picture. The one
/// exception is an intra frame whose GOP header changes the picture size
/// or the band counts: the decoder takes the new layout before it decodes
/// the bands, which clears the picture, so after such a frame fails
/// there is none until the next intra frame decodes.
///
/// An intra frame decodes in place into the reference buffers, as the
/// DLL's does. When one fails, the planes keep the previous picture, but
/// the bands it decoded before the error stay in the reference, so the
/// inter frames that follow predict from that mix until the next intra
/// frame decodes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Iv50Error {
    /// The start code is not 0x1f or the frame type is 5 to 7.
    BadPictureStart,
    /// A header field holds a value the format doesn't allow.
    BadHeader(&'static str),
    /// The frame ends before its data does.
    Truncated,
    /// A macroblock header doesn't decode.
    BadMbData,
    /// Block data doesn't decode.
    BadBlockData(&'static str),
    /// An inter or NULL frame arrived before any intra frame.
    NoReference,
    /// An inter frame in a stream that declares intra frames only.
    InterInIntraOnlyStream,
    /// A feature the specification leaves undetermined.
    Unsupported(&'static str),
}

impl core::fmt::Display for Iv50Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Iv50Error::BadPictureStart => f.write_str("bad picture start code or frame type"),
            Iv50Error::BadHeader(what) => write!(f, "bad header field: {what}"),
            Iv50Error::Truncated => f.write_str("frame data ends early"),
            Iv50Error::BadMbData => f.write_str("bad macroblock data"),
            Iv50Error::BadBlockData(what) => write!(f, "bad block data: {what}"),
            Iv50Error::NoReference => f.write_str("predicted frame before the first intra frame"),
            Iv50Error::InterInIntraOnlyStream => f.write_str("inter frame in an intra-only stream"),
            Iv50Error::Unsupported(what) => write!(f, "unsupported: {what}"),
        }
    }
}

impl std::error::Error for Iv50Error {}

/// One plane of the decoded picture.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Iv50Plane<'a> {
    /// Visible width in samples.
    pub width: usize,
    /// Visible height in samples.
    pub height: usize,
    /// Distance between rows in `data`.
    pub stride: usize,
    /// The samples, row `r` starting at `r * stride`.
    pub data: &'a [u8],
}

impl Iv50Plane<'_> {
    /// Row `r` of the visible picture, `width` samples; `r` must be below
    /// `height`.
    pub fn row(&self, r: usize) -> &[u8] {
        &self.data[r * self.stride..r * self.stride + self.width]
    }
}

/// The geometry of one plane: its size and its bands.
#[derive(Debug, Clone)]
struct PlaneLayout {
    width: usize,
    height: usize,
    bands: Vec<BandGeom>,
    /// Samples in the plane's band area, margins included.
    area_len: usize,
}

/// Samples before the first band and after the last one of a plane
/// area. Band reads that leave the bands (spec 07, 7.5; the neighbour
/// rules of spec 08, 8.6) land here and read zero.
const MARGIN: usize = 8192 + 512;
/// The gap the DLL leaves after each band: 0x400 bytes (spec 08, 8.2).
const BAND_GAP: usize = 0x200;

/// Stateful IV50 decoder.
pub struct Iv50Decoder {
    gop: Option<GopHeader>,
    layout: Vec<PlaneLayout>,
    /// Three buffer sets, each one plane area per plane.
    sets: [Vec<Vec<i16>>; 3],
    /// Indices into `sets`: W (decoded into), R1, R2, and whether R2
    /// holds a usable frame (spec 07, 7.2).
    w: usize,
    r1: usize,
    r2: usize,
    r2_valid: bool,
    b0: Vec<B0Rec>,
    prev_number: Option<u32>,
    /// Output planes in bitstream order: Y, V, U.
    out: Vec<OutPlane>,
    /// Transparency bounding box (y, x, h, w).
    bbox: Option<(usize, usize, usize, usize)>,
    bank: Box<Bank>,
    /// Huffman tables of the macroblock and block layers.
    mb_huffs: Vec<Huff>,
    blk_huffs: Vec<Huff>,
    have_picture: bool,
}

impl Default for Iv50Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for Iv50Decoder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Iv50Decoder")
            .field("gop", &self.gop)
            .field("have_picture", &self.have_picture)
            .finish_non_exhaustive()
    }
}

impl Iv50Decoder {
    /// A decoder waiting for its first intra frame.
    pub fn new() -> Self {
        Iv50Decoder {
            gop: None,
            layout: Vec::new(),
            sets: [Vec::new(), Vec::new(), Vec::new()],
            w: 0,
            r1: 1,
            r2: 2,
            r2_valid: false,
            b0: Vec::new(),
            prev_number: None,
            out: Vec::new(),
            bbox: None,
            bank: build_bank(),
            mb_huffs: Vec::new(),
            blk_huffs: Vec::new(),
            have_picture: false,
        }
    }

    /// Picture size from the last GOP header, or `None` before the first
    /// intra frame.
    pub fn dimensions(&self) -> Option<(usize, usize)> {
        self.gop
            .as_ref()
            .map(|g| (g.width as usize, g.height as usize))
    }

    /// The luma plane of the current picture.
    pub fn y(&self) -> Option<Iv50Plane<'_>> {
        self.plane(0)
    }

    /// The U (Cb) plane of the current picture.
    pub fn u(&self) -> Option<Iv50Plane<'_>> {
        self.plane(2)
    }

    /// The V (Cr) plane of the current picture.
    pub fn v(&self) -> Option<Iv50Plane<'_>> {
        self.plane(1)
    }

    fn plane(&self, i: usize) -> Option<Iv50Plane<'_>> {
        if !self.have_picture {
            return None;
        }
        let p = &self.out[i];
        Some(Iv50Plane {
            width: p.width,
            height: p.height,
            stride: p.stride,
            data: &p.data,
        })
    }

    /// Decode one compressed frame. Returns the frame's type; the
    /// picture is then in [`y`](Self::y), [`u`](Self::u) and
    /// [`v`](Self::v). A NULL frame leaves the picture as it was.
    pub fn decode(&mut self, data: &[u8]) -> Result<FrameType, Iv50Error> {
        if data.is_empty() {
            // An empty chunk carries no frame: the picture stays.
            if !self.have_picture {
                return Err(Iv50Error::NoReference);
            }
            return Ok(FrameType::Null);
        }
        let frame = PaddedFrame::new(data);
        let mut r = frame.reader_at(0);
        let start = headers::read_picture_start(&mut r)?;
        let mut ftype = start.frame_type;
        // The repeated-number rule (spec 01): a non-intra frame with the
        // previous frame's number is a NULL frame.
        if ftype != 0 && self.prev_number == Some(start.frame_number) {
            ftype = 4;
        }
        self.prev_number = Some(start.frame_number);
        if ftype == 4 {
            if !self.have_picture {
                return Err(Iv50Error::NoReference);
            }
            return Ok(FrameType::Null);
        }
        if ftype == 0 {
            let gop = headers::read_gop(&mut r)?;
            if gop.width as u64 * gop.height as u64 > MAX_PICTURE_PIXELS {
                return Err(Iv50Error::Unsupported(
                    "picture area above 4096 x 4096 pixels",
                ));
            }
            self.set_gop(gop);
        } else {
            let Some(gop) = &self.gop else {
                return Err(Iv50Error::NoReference);
            };
            if gop.intra_only {
                return Err(Iv50Error::InterInIntraOnlyStream);
            }
            if !self.have_picture {
                return Err(Iv50Error::NoReference);
            }
        }
        let fh = headers::read_frame_header(&mut r)?;
        let saved = (self.w, self.r1, self.r2, self.r2_valid);
        // Buffer rotation before decoding (spec 07, 7.2).
        match ftype {
            0 => {
                std::mem::swap(&mut self.w, &mut self.r1);
                self.r2_valid = false;
            }
            1 => self.r2_valid = false,
            _ => {
                if self.r2_valid {
                    std::mem::swap(&mut self.r1, &mut self.r2);
                }
            }
        }
        let decoded = self.w;
        match self.decode_bands(&frame, r.byte_pos(), ftype, &fh) {
            Ok(rects) => {
                if !rects.is_empty() {
                    self.set_bbox(&rects);
                }
            }
            Err(e) => {
                (self.w, self.r1, self.r2, self.r2_valid) = saved;
                return Err(e);
            }
        }
        // Rotation after decoding.
        match ftype {
            0 | 1 => std::mem::swap(&mut self.w, &mut self.r1),
            2 => {
                if self.r2_valid {
                    std::mem::swap(&mut self.r1, &mut self.r2);
                }
                std::mem::swap(&mut self.w, &mut self.r2);
                self.r2_valid = true;
            }
            _ => {
                if self.r2_valid {
                    std::mem::swap(&mut self.r1, &mut self.r2);
                }
            }
        }
        self.recombine(decoded);
        self.have_picture = true;
        Ok(match ftype {
            0 => FrameType::Intra,
            1 => FrameType::Inter,
            2 => FrameType::DroppableInterScalability,
            _ => FrameType::DroppableInter,
        })
    }

    /// Take a new GOP header. A change of picture size or band counts
    /// rebuilds and clears every buffer (spec 02, 1.3).
    fn set_gop(&mut self, gop: GopHeader) {
        let rebuild = match &self.gop {
            Some(old) => {
                old.width != gop.width
                    || old.height != gop.height
                    || old.luma_bands != gop.luma_bands
                    || old.chroma_bands != gop.chroma_bands
            }
            None => true,
        };
        self.layout = build_layout(&gop);
        let b0_count = self.layout[0].bands[0]
            .tiles
            .iter()
            .map(|t| t.cols * t.rows)
            .sum();
        if rebuild {
            for set in &mut self.sets {
                *set = self.layout.iter().map(|p| vec![0i16; p.area_len]).collect();
            }
            (self.w, self.r1, self.r2, self.r2_valid) = (0, 1, 2, false);
            self.b0 = vec![B0Rec::default(); b0_count];
            self.out = self
                .layout
                .iter()
                .map(|p| OutPlane::new(p.width, p.height))
                .collect();
            self.bbox = None;
            self.have_picture = false;
        } else {
            self.b0.resize(b0_count, B0Rec::default());
        }
        self.gop = Some(gop);
    }

    /// A cached Huffman table for a descriptor, from `cache`. Only one
    /// index of a cache is in use at a time, so any entry can make room.
    fn huff_index(cache: &mut Vec<Huff>, rows: &[u8]) -> Result<usize, Iv50Error> {
        if let Some(i) = cache.iter().position(|h| h.rows() == rows) {
            return Ok(i);
        }
        let h = Huff::new(rows).ok_or(Iv50Error::BadHeader("Huffman descriptor"))?;
        if cache.len() >= 16 {
            cache.swap_remove(0);
        }
        cache.push(h);
        Ok(cache.len() - 1)
    }

    /// Decode every band of the frame into buffer set W, starting at
    /// byte `pos`. Returns the transparency rectangles.
    fn decode_bands(
        &mut self,
        frame: &PaddedFrame,
        mut pos: usize,
        ftype: u32,
        fh: &headers::FrameHeader,
    ) -> Result<Vec<(u32, u32, u32, u32)>, Iv50Error> {
        let mb_idx = Self::huff_index(&mut self.mb_huffs, &fh.mb_rows)?;
        let gop = self.gop.as_ref().expect("GOP checked by the caller");
        let b0_mb_size = gop.luma_info[0].mb_size;
        let transparency = gop.transparency;
        let b0 = &self.layout[0].bands[0];
        let b0_drifts =
            b0.info.mb_size == b0.info.blk_size && b0.tiles.iter().any(|t| t.cols % 2 == 1);
        for plane in 0..3 {
            for bi in 0..self.layout[plane].bands.len() {
                if pos >= frame.len() {
                    return Err(Iv50Error::Truncated);
                }
                let mut r = frame.reader_at(pos);
                let hdr = headers::read_band_header(&mut r, fh.flags)?;
                if hdr.empty() {
                    let g = &self.layout[plane].bands[bi];
                    fill_empty_band(&mut self.sets[self.w][plane], g, bi);
                    pos += 1;
                    continue;
                }
                let blk_idx = Self::huff_index(&mut self.blk_huffs, &hdr.blk_rows)?;
                // Split the sets so the current one is mutable and the
                // reference readable.
                let (cur, refp) = split_sets(&mut self.sets, self.w, self.r1);
                let job = BandJob {
                    geom: &self.layout[plane].bands[bi],
                    hdr: &hdr,
                    is_b0: plane == 0 && bi == 0,
                    frame_type: ftype,
                    frame_flags: fh.flags,
                    b0_mb_size,
                    b0_drifts,
                    mb_huff: &self.mb_huffs[mb_idx],
                    blk_huff: &self.blk_huffs[blk_idx],
                    bank: &self.bank,
                    cur: &mut cur[plane],
                    refp: &refp[plane],
                    b0: &mut self.b0,
                };
                job.decode(&mut r)?;
                pos = match hdr.size {
                    Some(s) => pos + s as usize,
                    None => r.byte_pos(),
                };
            }
        }
        if transparency {
            let mut r = frame.reader_at(pos);
            let rects = headers::read_transparency_rects(&mut r, fh.flags);
            if r.overrun() {
                return Err(Iv50Error::Truncated);
            }
            return Ok(rects);
        }
        Ok(Vec::new())
    }

    /// Store the union of the frame's transparency rectangles (spec 08,
    /// 8.9).
    fn set_bbox(&mut self, rects: &[(u32, u32, u32, u32)]) {
        let x0 = rects.iter().map(|r| r.0).min().unwrap_or(0) as usize;
        let y0 = rects.iter().map(|r| r.1).min().unwrap_or(0) as usize;
        let x1 = rects.iter().map(|r| r.0 + r.2).max().unwrap_or(0) as usize;
        let y1 = rects.iter().map(|r| r.1 + r.3).max().unwrap_or(0) as usize;
        self.bbox = Some((y0, x0, y1 - y0, x1 - x0));
    }

    /// Recombine the bands of buffer set `set` into the output planes
    /// over the frame's window (spec 08, 8.1 and 8.3).
    fn recombine(&mut self, set: usize) {
        let gop = self.gop.as_ref().expect("GOP present");
        let (pw, ph) = (gop.width as usize, gop.height as usize);
        let (mut y0, mut x0, mut h, mut w) = (0usize, 0usize, ph, pw);
        if gop.transparency
            && let Some((by, bx, bh, bw)) = self.bbox
        {
            let (wy, wx, wh, ww) = (0usize, 0usize, ph, pw);
            y0 = wy.max(by);
            x0 = wx.max(bx);
            h = if wy + wh < by + bh {
                (wh + wy).saturating_sub(y0)
            } else {
                (by + bh).saturating_sub(y0)
            };
            w = if wx + ww < bx + bw {
                (ww + wx).saturating_sub(x0)
            } else {
                (bx + bw).saturating_sub(x0)
            };
            h = h.max(1);
            w = w.max(1);
            // A bounding box that starts below or right of the picture
            // gives a window outside the planes. The DLL would write
            // past its planes there; this decoder writes nothing.
            if y0 >= ph || x0 >= pw {
                return;
            }
        }
        for plane in 0..3 {
            let win = if plane == 0 {
                Window { y: y0, x: x0, h, w }
            } else {
                let (yc, xc) = (y0 >> 2, x0 >> 2);
                Window {
                    y: yc,
                    x: xc,
                    h: ((y0 + 3 + h) >> 2) - yc,
                    w: ((x0 + 3 + w) >> 2) - xc,
                }
            };
            let lay = &self.layout[plane];
            let area = &self.sets[set][plane];
            // One or four bands: the GOP header reader refuses seven.
            if lay.bands.len() == 1 {
                synth::one_band(area, &lay.bands[0], win, &mut self.out[plane]);
            } else {
                synth::four_band(area, &lay.bands, win, &mut self.out[plane]);
            }
        }
    }
}

/// Whether `data` starts like an IV50 frame: a valid picture start and,
/// for an intra frame, a GOP header and frame header that parse (a
/// header that asks for an unsupported feature still counts), or for a
/// predicted frame a frame header that fits in `data`.
pub(crate) fn looks_like_iv50(data: &[u8]) -> bool {
    let frame = PaddedFrame::new(data);
    let mut r = frame.reader_at(0);
    let Ok(start) = headers::read_picture_start(&mut r) else {
        return false;
    };
    if start.frame_type == 4 {
        return true;
    }
    if start.frame_type == 0 {
        match headers::read_gop(&mut r) {
            Ok(_) => {}
            Err(Iv50Error::Unsupported(_)) => return true,
            Err(_) => return false,
        }
    }
    headers::read_frame_header(&mut r).is_ok()
}

/// Borrow set `a` mutably and set `b` immutably.
fn split_sets(
    sets: &mut [Vec<Vec<i16>>; 3],
    a: usize,
    b: usize,
) -> (&mut Vec<Vec<i16>>, &Vec<Vec<i16>>) {
    assert_ne!(a, b);
    let [s0, s1, s2] = sets;
    match (a, b) {
        (0, 1) => (s0, s1),
        (0, 2) => (s0, s2),
        (1, 0) => (s1, s0),
        (1, 2) => (s1, s2),
        (2, 0) => (s2, s0),
        _ => (s2, s1),
    }
}

/// Band sizes, buffer layout and tiles for a GOP header (spec 02,
/// section 4; spec 08, 8.2).
fn build_layout(gop: &GopHeader) -> Vec<PlaneLayout> {
    let (lw, lh) = (gop.width as usize, gop.height as usize);
    let (cw, ch) = gop.chroma_size();
    let (cw, ch) = (cw as usize, ch as usize);
    let mut layouts = Vec::with_capacity(3);
    for plane in 0..3 {
        let (pw, ph, infos) = if plane == 0 {
            (lw, lh, &gop.luma_info)
        } else {
            (cw, ch, &gop.chroma_info)
        };
        let n = infos.len();
        let mut bands = Vec::with_capacity(n);
        let mut off = MARGIN;
        for (bi, info) in infos.iter().enumerate() {
            // One or four bands (seven-band planes are refused by the GOP
            // header reader).
            let (bw, bh) = if n == 1 { (pw, ph) } else { (pw >> 1, ph >> 1) };
            let (bw, bh) = (bw.max(1), bh.max(1));
            let pitch = ((2 * bw + 31) & !31) / 2;
            let rows = bh.next_multiple_of(16);
            let matrix = if plane == 0 {
                if n == 1 { 0 } else { bi + 1 }
            } else if n == 1 {
                5
            } else {
                bi + 6
            };
            // One tile covering the band (multi-tile streams are refused
            // by the GOP header reader).
            let mb = info.mb_size as usize;
            let tiles = vec![TileGeom {
                x: 0,
                y: 0,
                cols: bw.div_ceil(mb),
                rows: bh.div_ceil(mb),
                copy_width: pitch,
                rec_base: 0,
            }];
            bands.push(BandGeom {
                height: bh,
                pitch,
                offset: off,
                info: *info,
                matrix,
                tiles,
            });
            off += rows * pitch + BAND_GAP;
        }
        layouts.push(PlaneLayout {
            width: pw,
            height: ph,
            bands,
            area_len: off + MARGIN,
        });
    }
    layouts
}

#[cfg(test)]
mod tests;
