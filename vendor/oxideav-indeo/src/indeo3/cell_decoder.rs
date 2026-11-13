//! Indeo 3 plane decoder over the cell-geometry banks (r459).
//!
//! Spec source: `spec/03` (the binary-tree walk, §2–§4), `spec/04
//! §1.1`/`§4`/`§5.3`/`§6` (geometry banks, VQ_NULL, staging window),
//! `spec/05 §2`/`§5` (motion-compensated fetch), `spec/06 §3`/`§4`
//! (the mode-byte stream: family prologues, the six families, the
//! escapes) and `spec/07 §1`/`§2`/`§4` (predictor, codebook
//! application, 7-bit range and the output upshift), as re-cut by the
//! Specifier-20 round; every value-level choice the spec text leaves
//! open is arbitrated against the two staged `IV32` fixtures.
//!
//! One [`PlaneBuffers`] holds a plane's strips as `0xb0`-stride byte
//! buffers with one predictor row above the picture. [`decode_plane`]
//! walks the plane's binary tree — positioning each leaf through the
//! banks — and reconstructs every cell in place.

use super::geometry_bank::{
    PlaneBanks, STRIP_ROW_STRIDE, UNDER_FOUR_CODE, UNDER_FOUR_YPOS, chroma_plane_dims,
};
use super::picture_layer::MotionVector;
use super::staging::{STAGING_BLOCK_COUNT, STAGING_BLOCK_STRIDE, StagingImage};
use super::vq::{DyadDeltaTable, VqArena};

/// The strip pixel buffers of one plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneBuffers {
    /// Plane width in samples (the decoded, padded width for chroma).
    pub width: u32,
    /// Plane height in rows.
    pub height: u32,
    /// The geometry banks of the plane.
    pub banks: PlaneBanks,
    /// One buffer per strip: `(height + 2) * 0xb0` bytes, row `r` of
    /// the picture at `(r + 1) * 0xb0`; row 0 is the predictor row
    /// above the picture and the last row a scratch row below it.
    pub strips: Vec<Vec<u8>>,
}

/// The predictor row above the top of every strip (fixture-arbitrated
/// r451: the strip boundary reads as `0x40`, the 7-bit mid-grey).
pub use super::reconstruct::TOP_OF_STRIP_PREDICTOR;

impl PlaneBuffers {
    /// Allocate a plane's strips, every sample at `fill` and the
    /// predictor row at [`TOP_OF_STRIP_PREDICTOR`].
    pub fn new(width: u32, height: u32, banks: PlaneBanks, fill: u8) -> Self {
        let stride = STRIP_ROW_STRIDE as usize;
        let mut strips = Vec::with_capacity(banks.nstrips as usize);
        for _ in 0..banks.nstrips {
            let mut s = vec![fill; stride * (height as usize + 2)];
            s[..stride].fill(TOP_OF_STRIP_PREDICTOR);
            strips.push(s);
        }
        PlaneBuffers {
            width,
            height,
            banks,
            strips,
        }
    }

    /// The luma plane of a `width × height` picture.
    pub fn luma(width: u32, height: u32) -> Self {
        Self::new(width, height, PlaneBanks::luma(width, height), 0)
    }

    /// A chroma plane of a `width × height` picture.
    pub fn chroma(width: u32, height: u32) -> Self {
        let (cw, ch) = chroma_plane_dims(width, height);
        Self::new(cw, ch, PlaneBanks::chroma(width, height), 0)
    }

    /// The byte index of `(row, col)` inside a strip buffer; `row = -1`
    /// is the predictor row.
    #[inline]
    fn idx(row: i64, col: u32) -> usize {
        ((row + 1) as usize) * STRIP_ROW_STRIDE as usize + col as usize
    }

    /// Sample `(x, y)` of the plane (7-bit internal range), `None`
    /// outside the plane.
    pub fn sample(&self, x: u32, y: u32) -> Option<u8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let strip = (x / self.banks.full_strip) as usize;
        let col = x % self.banks.full_strip;
        self.strips
            .get(strip)
            .and_then(|s| s.get(Self::idx(i64::from(y), col)).copied())
    }

    /// The plane as `out_w × out_h` 8-bit samples (`spec/07 §4.3`:
    /// `(v & 0x7f) << 1`), cropped from the decoded plane.
    pub fn to_pixels(&self, out_w: u32, out_h: u32) -> Vec<u8> {
        // Row by row and strip by strip: each strip's run of a row is
        // contiguous in its buffer. Samples [`Self::sample`] reports as
        // `None` stay 0.
        let mut out = vec![0u8; (out_w * out_h) as usize];
        let full = self.banks.full_strip as usize;
        let (w, h) = (out_w.min(self.width) as usize, out_h.min(self.height));
        if full == 0 || w == 0 {
            return out;
        }
        for (y, row) in (0..h).zip(out.chunks_exact_mut(out_w as usize)) {
            let start = Self::idx(i64::from(y), 0);
            for (run, strip) in row[..w].chunks_mut(full).zip(&self.strips) {
                let src = strip.get(start..).unwrap_or(&[]);
                for (d, &v) in run.iter_mut().zip(src) {
                    *d = (v & 0x7f) << 1;
                }
            }
        }
        out
    }
}

/// Errors of the plane decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CellDecodeError {
    /// The bitstream ended inside the plane.
    StreamExhausted,
    /// A leaf landed on a node under four rows / pixels
    /// (`spec/03 §4.2`, walker return 3).
    UnderFourNode,
    /// The tree descended past heap index 255.
    TreeTooDeep,
    /// The strip slot a leaf named does not exist.
    BadStrip,
    /// A VQ_NULL sub-code the binary faults on.
    BadVqNull,
    /// A mode byte whose nibble-table slot is the fault exit
    /// (`spec/06 §3.2`).
    BadModeByte {
        /// The mode byte.
        mode: u8,
    },
    /// A family whose gate the cell's MC flag fails (`spec/06 §3.4`).
    FamilyGate {
        /// The mode byte.
        mode: u8,
    },
    /// An escape at a position that rejects it (`spec/06 §4.1`).
    BadEscape {
        /// The escape byte.
        escape: u8,
        /// The row position.
        position: u8,
    },
    /// An `0xFB` counter the binary rejects (`spec/06 §4.4`).
    BadCounter {
        /// The counter byte.
        counter: u8,
    },
    /// The two-byte literal still had bit 31 set (`spec/07 §2.3`).
    RangeFault,
    /// The staging window the mode byte selects lies outside the
    /// staging image.
    BadStagingBlock,
    /// An INTER leaf's MV index is outside the plane's MV table.
    BadMvIndex {
        /// The index byte.
        index: u8,
    },
    /// A motion-compensated fetch left the strip buffer.
    McOutOfRange,
    /// A family the fixtures do not exercise (C / D) — reported, not
    /// guessed.
    UnsupportedFamily {
        /// The mode byte.
        mode: u8,
    },
}

impl core::fmt::Display for CellDecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "indeo3 cell decoder: {self:?}")
    }
}

impl std::error::Error for CellDecodeError {}

/// Per-plane decode statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct PlaneStats {
    /// Leaf cells positioned.
    pub cells: u32,
    /// Cells with a mode-byte stream (VQ_DATA or the VQ_NULL `1`
    /// sub-code).
    pub coded_cells: u32,
    /// VQ_NULL copy-upper cells.
    pub copy_upper: u32,
    /// VQ_NULL mark-skip cells.
    pub skipped: u32,
    /// INTER leaves (motion-compensated).
    pub inter: u32,
    /// Bytes of the plane payload consumed.
    pub bytes: usize,
    /// Histogram of mode bytes by family letter index (A..F).
    pub families: [u32; 6],
}

/// The codebook the mode byte selects (`spec/06 §3.2` prologues).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodeBase {
    /// The per-frame arena band (`alt_quant` overlay).
    Arena,
    /// The `cb_offset`-biased staging window.
    Staging,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    A,
    B,
    C,
    D,
    E,
    F,
}

/// The inputs a plane decode needs besides the buffers.
pub struct PlaneContext<'a> {
    /// The codec-init staging image.
    pub staging: &'a StagingImage,
    /// The per-frame arena after the `alt_quant` overlay.
    pub arena: &'a VqArena,
    /// The frame's `cb_offset`.
    pub cb_offset: i8,
    /// The static dyad table (its first 1 KB is the predictor LUT
    /// bank set).
    pub lut: &'a DyadDeltaTable,
    /// The plane's motion-vector table.
    pub mvs: &'a [MotionVector],
    /// The reference plane (previous frame) for INTER leaves.
    pub reference: Option<&'a PlaneBuffers>,
}

/// MSB-first bit reader with byte-level reads (`spec/03 §2.1`,
/// `spec/06 §5.1`): the bit accumulator holds the remaining bits of
/// the last byte it loaded, and byte-level reads take whole bytes at
/// the cursor without touching it — so the tree bits left over from
/// before a cell's byte stream continue *after* it.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u8,
    nbits: u8,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader {
            data,
            pos: 0,
            acc: 0,
            nbits: 0,
        }
    }

    fn bit(&mut self) -> Result<u8, CellDecodeError> {
        if self.nbits == 0 {
            self.acc = *self
                .data
                .get(self.pos)
                .ok_or(CellDecodeError::StreamExhausted)?;
            self.pos += 1;
            self.nbits = 8;
        }
        let v = self.acc >> 7;
        self.acc <<= 1;
        self.nbits -= 1;
        Ok(v)
    }

    fn byte(&mut self) -> Result<u8, CellDecodeError> {
        let b = *self
            .data
            .get(self.pos)
            .ok_or(CellDecodeError::StreamExhausted)?;
        self.pos += 1;
        Ok(b)
    }

    fn peek(&self) -> Result<u8, CellDecodeError> {
        self.data
            .get(self.pos)
            .copied()
            .ok_or(CellDecodeError::StreamExhausted)
    }
}

/// A positioned leaf cell.
#[derive(Debug, Clone, Copy)]
struct CellRect {
    strip: usize,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

struct Walker<'a, 'b> {
    r: Reader<'a>,
    bufs: &'b mut PlaneBuffers,
    ctx: &'b PlaneContext<'b>,
    stats: PlaneStats,
    sticky: bool,
}

impl<'a, 'b> Walker<'a, 'b> {
    fn position(&self, cl: u32, ch: u32) -> Result<CellRect, CellDecodeError> {
        if cl > 255 || ch > 255 {
            return Err(CellDecodeError::TreeTooDeep);
        }
        let banks = &self.bufs.banks;
        let slot = banks.full.strip[ch as usize];
        let bank = banks.bank_for_slot(slot);
        let h4 = bank.h4[cl as usize];
        let w4 = bank.w4[ch as usize];
        if h4 == UNDER_FOUR_CODE
            || w4 == UNDER_FOUR_CODE
            || bank.ypos[cl as usize] >= UNDER_FOUR_YPOS
        {
            return Err(CellDecodeError::UnderFourNode);
        }
        if usize::from(slot) >= self.bufs.strips.len() {
            return Err(CellDecodeError::BadStrip);
        }
        Ok(CellRect {
            strip: usize::from(slot),
            x: bank.xpos[ch as usize],
            y: bank.ypos[cl as usize] / STRIP_ROW_STRIDE,
            w: 4 * u32::from(w4),
            h: 4 * u32::from(h4),
        })
    }

    /// `spec/03 §3` MC tree over node `(cl, ch)`.
    fn walk_mc(&mut self, cl: u32, ch: u32) -> Result<(), CellDecodeError> {
        if cl > 255 || ch > 255 {
            // The banks are 256 entries deep; a deeper split is a
            // stream error (and bounds the recursion).
            return Err(CellDecodeError::TreeTooDeep);
        }
        let b0 = self.r.bit()?;
        if b0 == 0 {
            let b1 = self.r.bit()?;
            if b1 == 0 {
                self.walk_mc(cl * 2, ch)?;
                self.walk_mc(cl * 2 + 1, ch)
            } else {
                self.walk_mc(cl, ch * 2)?;
                self.walk_mc(cl, ch * 2 + 1)
            }
        } else {
            let b1 = self.r.bit()?;
            let mv = if b1 == 1 {
                let idx = self.r.byte()?;
                let mv = *self
                    .ctx
                    .mvs
                    .get(usize::from(idx))
                    .ok_or(CellDecodeError::BadMvIndex { index: idx })?;
                Some(mv)
            } else {
                None
            };
            self.walk_vq(cl, ch, mv)
        }
    }

    /// `spec/03 §4` VQ tree over node `(cl, ch)` of an MC-tree leaf.
    fn walk_vq(
        &mut self,
        cl: u32,
        ch: u32,
        mv: Option<MotionVector>,
    ) -> Result<(), CellDecodeError> {
        if cl > 255 || ch > 255 {
            return Err(CellDecodeError::TreeTooDeep);
        }
        let b0 = self.r.bit()?;
        if b0 == 0 {
            let b1 = self.r.bit()?;
            if b1 == 0 {
                self.walk_vq(cl * 2, ch, mv)?;
                self.walk_vq(cl * 2 + 1, ch, mv)
            } else {
                self.walk_vq(cl, ch * 2, mv)?;
                self.walk_vq(cl, ch * 2 + 1, mv)
            }
        } else {
            let b1 = self.r.bit()?;
            let cell = self.position(cl, ch)?;
            self.stats.cells += 1;
            if let Some(mv) = mv {
                self.stats.inter += 1;
                self.mc_copy(&cell, mv)?;
            }
            if b1 == 1 {
                // VQ_DATA.
                self.unpack(&cell, mv.is_some())
            } else {
                // VQ_NULL sub-code prefix (spec/04 §4).
                let s0 = self.r.bit()?;
                if s0 == 1 {
                    self.unpack(&cell, mv.is_some())
                } else {
                    let s1 = self.r.bit()?;
                    if s1 == 0 {
                        self.stats.copy_upper += 1;
                        // An INTER cell keeps its motion-compensated
                        // content (fixture-arbitrated r459: every
                        // chroma plane of the inter frames).
                        if mv.is_none() {
                            self.copy_upper(&cell);
                        }
                    } else {
                        self.stats.skipped += 1;
                    }
                    Ok(())
                }
            }
        }
    }

    fn strip_mut(&mut self, strip: usize) -> &mut Vec<u8> {
        &mut self.bufs.strips[strip]
    }

    /// `spec/04 §4` copy-upper: every row of the cell takes the row
    /// above it (the predictor chain reduces to the row above the
    /// cell).
    fn copy_upper(&mut self, cell: &CellRect) {
        let s = self.strip_mut(cell.strip);
        let src = PlaneBuffers::idx(i64::from(cell.y) - 1, cell.x);
        let w = cell.w as usize;
        // The source row lies above the cell, so no copy overwrites it.
        for r in 0..cell.h as usize {
            let dst = PlaneBuffers::idx(i64::from(cell.y) + r as i64, cell.x);
            s.copy_within(src..src + w, dst);
        }
    }

    /// `spec/05 §5`/`§6` motion-compensated fetch from the reference
    /// plane into the cell: the packed MV's high 30 bits are a signed
    /// byte offset (`176 · vert + horiz`) into the strip buffer, its
    /// low two bits select the half-pel filters (bit 0 vertical, bit 1
    /// horizontal; truncating averages masked to 7 bits).
    fn mc_copy(&mut self, cell: &CellRect, mv: MotionVector) -> Result<(), CellDecodeError> {
        let reference = self.ctx.reference.ok_or(CellDecodeError::McOutOfRange)?;
        let rs = reference
            .strips
            .get(cell.strip)
            .ok_or(CellDecodeError::McOutOfRange)?;
        let packed = mv.packed_mv();
        let offset = i64::from(packed >> 2);
        let vert_half = packed & 1 != 0;
        let horiz_half = packed & 2 != 0;
        let stride = STRIP_ROW_STRIDE as usize;
        let (w, h) = (cell.w as usize, cell.h as usize);
        if w == 0 || h == 0 {
            return Ok(());
        }
        // The fetch addresses grow with the row and the column, so the
        // whole cell is in range when its first and last fetches are.
        // Checking both before writing anything leaves the cell
        // untouched on a fault, as a per-sample check would.
        let first = PlaneBuffers::idx(i64::from(cell.y), cell.x) as i64 + offset;
        let last = PlaneBuffers::idx(i64::from(cell.y) + h as i64 - 1, cell.x) as i64
            + offset
            + (w - 1) as i64
            + if vert_half { stride as i64 } else { 0 }
            + i64::from(horiz_half);
        if first < 0 || last >= rs.len() as i64 {
            return Err(CellDecodeError::McOutOfRange);
        }
        let avg = |a: u8, b: u8| (u16::from(a) + u16::from(b)) >> 1;
        let s = &mut self.bufs.strips[cell.strip];
        for r in 0..h {
            let d0 = PlaneBuffers::idx(i64::from(cell.y) + r as i64, cell.x);
            let s0 = (d0 as i64 + offset) as usize;
            let dst = &mut s[d0..d0 + w];
            let a = &rs[s0..s0 + w];
            match (vert_half, horiz_half) {
                (false, false) => {
                    for (d, &p) in dst.iter_mut().zip(a) {
                        *d = p & 0x7f;
                    }
                }
                (true, false) => {
                    let b = &rs[s0 + stride..s0 + stride + w];
                    for ((d, &p), &q) in dst.iter_mut().zip(a).zip(b) {
                        *d = (avg(p, q) & 0x7f) as u8;
                    }
                }
                (false, true) => {
                    let b = &rs[s0 + 1..s0 + 1 + w];
                    for ((d, &p), &q) in dst.iter_mut().zip(a).zip(b) {
                        *d = (avg(p, q) & 0x7f) as u8;
                    }
                }
                (true, true) => {
                    let a1 = &rs[s0 + 1..s0 + 1 + w];
                    let b = &rs[s0 + stride..s0 + stride + w];
                    let b1 = &rs[s0 + stride + 1..s0 + stride + 1 + w];
                    for (i, d) in dst.iter_mut().enumerate() {
                        let top = avg(a[i], a1[i]);
                        let bottom = avg(b[i], b1[i]);
                        *d = (((top + bottom) >> 1) & 0x7f) as u8;
                    }
                }
            }
        }
        Ok(())
    }

    /// Resolve the 1 KB primary / secondary code tables for a mode
    /// byte's base and band.
    fn tables(&self, base: CodeBase, band: u8) -> Result<(&'b [u8], &'b [u8]), CellDecodeError> {
        match base {
            CodeBase::Staging => {
                let block = i64::from(self.ctx.cb_offset) + i64::from(band);
                if block < 0 || block as usize >= STAGING_BLOCK_COUNT {
                    return Err(CellDecodeError::BadStagingBlock);
                }
                let off = block as usize * STAGING_BLOCK_STRIDE;
                let bytes = self.ctx.staging.as_bytes();
                Ok((&bytes[off..off + 0x400], &bytes[off + 0x400..off + 0x800]))
            }
            CodeBase::Arena => {
                let p = VqArena::band_primary_offset(usize::from(band))
                    .ok_or(CellDecodeError::BadStagingBlock)?;
                let s = VqArena::band_secondary_offset(usize::from(band))
                    .ok_or(CellDecodeError::BadStagingBlock)?;
                let bytes = &self.ctx.arena.as_bytes()[..];
                Ok((&bytes[p..p + 0x400], &bytes[s..s + 0x400]))
            }
        }
    }

    /// The per-cell mode-byte stream (`spec/06 §3`).
    fn unpack(&mut self, cell: &CellRect, mc: bool) -> Result<(), CellDecodeError> {
        self.stats.coded_cells += 1;
        let mode = self.r.byte()?;
        let hi = mode >> 4;
        let lo = mode & 0x0f;
        // spec/06 §3.2: the bit-3-set table's slots 0/3/10 rewrite the
        // predictor row through the LUT bank and re-dispatch through
        // the other table.
        let (base, rows, lut, band) = if lo & 8 != 0 {
            match hi {
                // The LUT rewrite uses bank `lo & 7`; the codebook band
                // keeps the full nibble (staging block 8.. = the even
                // seed sets, fixture-arbitrated r459).
                0x0 => (CodeBase::Staging, 1u8, true, lo),
                0x3 => (CodeBase::Staging, 2, true, lo),
                0xA => (CodeBase::Staging, 3, true, lo),
                0x1 => (CodeBase::Arena, 1, false, lo),
                0x4 => (CodeBase::Arena, 2, false, lo),
                0xB => (CodeBase::Staging, 2, false, lo),
                0xC => (CodeBase::Arena, 2, false, lo),
                _ => return Err(CellDecodeError::BadModeByte { mode }),
            }
        } else {
            match hi {
                0x0 => (CodeBase::Staging, 1, false, lo),
                0x1 => (CodeBase::Arena, 1, false, lo),
                0x3 => (CodeBase::Staging, 2, false, lo),
                0x4 => (CodeBase::Arena, 2, false, lo),
                0xA => (CodeBase::Staging, 3, false, lo),
                0xB => (CodeBase::Staging, 2, false, lo),
                0xC => (CodeBase::Arena, 2, false, lo),
                _ => return Err(CellDecodeError::BadModeByte { mode }),
            }
        };
        // Family by prologue class and MC flag (spec/06 §3.4).
        let family = match (rows, hi, mc) {
            (1, _, false) => Family::A,
            (1, _, true) => Family::B,
            (3, _, false) => Family::E,
            (3, _, true) => Family::F,
            (2, 0x3 | 0x4, false) => Family::C,
            (2, 0xB | 0xC, true) => Family::D,
            _ => return Err(CellDecodeError::FamilyGate { mode }),
        };
        self.stats.families[family as usize] += 1;
        if lut {
            self.lut_rewrite(cell, band & 7);
        }

        let (primary, secondary) = self.tables(base, band)?;
        match family {
            Family::A | Family::B => self.body_4x4(cell, primary, secondary, family == Family::B),
            Family::E | Family::F => self.body_8x8(cell, primary, secondary, family == Family::F),
            Family::C | Family::D => Err(CellDecodeError::UnsupportedFamily { mode }),
        }
    }

    /// `spec/06 §3.4` band-nibble-≥8 rewrite of the row above the cell
    /// through LUT bank `band` of `.data 0x1003d088`.
    fn lut_rewrite(&mut self, cell: &CellRect, band: u8) {
        let table = self.ctx.lut.as_bytes();
        let bank = &table[usize::from(band) * 128..usize::from(band) * 128 + 128];
        let s = self.strip_mut(cell.strip);
        let src = PlaneBuffers::idx(i64::from(cell.y) - 1, cell.x);
        for i in (0..cell.w as usize).rev() {
            let v = s[src + i];
            s[src + i] = bank[usize::from(v & 0x7f)] | (v & 0x80);
        }
    }

    /// One literal code (`spec/07 §2.1`/`§2.3`): the raw `pred +
    /// table[b]` decides the one-byte / two-byte form by its bit 31 (a
    /// seed word's `0x8000` bias always sets it); the pixels then take
    /// the word's two dyads — the high lane from `b`, and for the
    /// two-byte form the low lane from the continuation byte `c`'s
    /// high word — each applied through [`apply_dyad`].
    fn literal(&mut self, table: &[u8], b: u8, pred: u32) -> Result<u32, CellDecodeError> {
        let e = usize::from(b) * 4;
        let word = u32::from_le_bytes(table[e..e + 4].try_into().unwrap());
        let sum = pred.wrapping_add(word);
        let (lo_dyad, hi_dyad) = if sum & 0x8000_0000 == 0 {
            // One-byte form: `(B[i] << 16) + sext16(B[j])`.
            let lo = word as u16;
            let borrow = u16::from(lo & 0x8000 != 0);
            let hi = ((word >> 16) as u16).wrapping_add(borrow);
            (lo, hi)
        } else {
            let c = self.r.byte()?;
            let ce = usize::from(c) * 4 + 2;
            let c_hi = u16::from_le_bytes(table[ce..ce + 2].try_into().unwrap());
            (
                c_hi.wrapping_sub(0x8000),
                ((word >> 16) as u16).wrapping_sub(0x8000),
            )
        };
        let out_lo = apply_dyad(pred as u16, lo_dyad);
        let out_hi = apply_dyad((pred >> 16) as u16, hi_dyad);
        let out = (u32::from(out_hi) << 16) | u32::from(out_lo);
        if out & 0x8080_8080 != 0 {
            return Err(CellDecodeError::RangeFault);
        }
        Ok(out)
    }

    /// Families A / B: 4×4 blocks, row-group major, one code per row.
    fn body_4x4(
        &mut self,
        cell: &CellRect,
        primary: &[u8],
        secondary: &[u8],
        own_predictor: bool,
    ) -> Result<(), CellDecodeError> {
        let groups = cell.h / 4;
        let cols = cell.w / 4;
        let total = groups * cols;
        let mut block = 0u32;
        // A pending run of predictor / marker positions (0xFB).
        let mut run_fill = 0u32;
        let mut run_mark = 0u32;
        while block < total {
            let g = block / cols;
            let c = block % cols;
            let y0 = i64::from(cell.y) + 4 * i64::from(g);
            let x0 = cell.x + 4 * c;
            if run_fill > 0 {
                run_fill -= 1;
                self.fill_rows(cell.strip, y0, x0, 0, 4, own_predictor);
                block += 1;
                continue;
            }
            if run_mark > 0 {
                run_mark -= 1;
                block += 1;
                continue;
            }
            let mut pos = 0u8;
            while pos < 4 {
                if self.sticky {
                    // A sticky 0xFA / 0xFD repeats without a read.
                    let b = self.r.peek()?;
                    if b == 0xFA {
                        pos = 4;
                        continue;
                    }
                    if b == 0xFD {
                        self.fill_rows(cell.strip, y0, x0, pos, 4, own_predictor);
                        pos = 4;
                        continue;
                    }
                }
                let b = self.r.byte()?;
                match b {
                    0x00..=0xF7 => {
                        let table = if pos.is_multiple_of(2) {
                            secondary
                        } else {
                            primary
                        };
                        let pred =
                            self.pred_dword(cell.strip, y0 + i64::from(pos), x0, own_predictor);
                        let v = self.literal(table, b, pred)?;
                        self.store_dword(cell.strip, y0 + i64::from(pos), x0, v);
                        pos += 1;
                    }
                    0xF8 => {
                        if pos != 0 {
                            return Err(CellDecodeError::BadEscape {
                                escape: b,
                                position: pos,
                            });
                        }
                        let v = self.r.byte()? & 0x7f;
                        let w = u32::from_le_bytes([v | 0x80, v | 0x80, v | 0x80, v | 0x80]);
                        self.store_dword(cell.strip, y0, x0, w);
                        pos += 1;
                    }
                    0xF9 | 0xFA => {
                        if pos != 0 {
                            return Err(CellDecodeError::BadEscape {
                                escape: b,
                                position: pos,
                            });
                        }
                        if b == 0xF9 {
                            self.sticky = !self.sticky;
                        }
                        if self.sticky {
                            self.r.pos -= 1;
                        }
                        pos = 4;
                    }
                    0xFB => {
                        let counter = self.r.byte()?;
                        let (fill, mark) = fb_counter(counter)?;
                        if fill > 0 {
                            let extra = if self.sticky {
                                self.sticky = false;
                                1
                            } else {
                                0
                            };
                            self.fill_rows(cell.strip, y0, x0, pos, 4, own_predictor);
                            run_fill = (fill + extra).saturating_sub(1);
                        } else {
                            run_mark = mark.saturating_sub(1);
                        }
                        pos = 4;
                    }
                    0xFC | 0xFD => {
                        if b == 0xFC {
                            self.sticky = !self.sticky;
                        }
                        self.fill_rows(cell.strip, y0, x0, pos, 4, own_predictor);
                        if self.sticky {
                            self.r.pos -= 1;
                        }
                        pos = 4;
                    }
                    0xFE => {
                        if pos > 1 {
                            return Err(CellDecodeError::BadEscape {
                                escape: b,
                                position: pos,
                            });
                        }
                        self.fill_rows(cell.strip, y0, x0, pos, 3, own_predictor);
                        pos = 3;
                    }
                    0xFF => {
                        if pos != 0 {
                            return Err(CellDecodeError::BadEscape {
                                escape: b,
                                position: pos,
                            });
                        }
                        self.fill_rows(cell.strip, y0, x0, 0, 2, own_predictor);
                        pos = 2;
                    }
                }
            }
            block += 1;
        }
        Ok(())
    }

    /// Families E / F: 8×8 blocks, four codes per block, each producing
    /// a horizontally doubled row stored as a `(avg, cur)` row pair.
    fn body_8x8(
        &mut self,
        cell: &CellRect,
        primary: &[u8],
        secondary: &[u8],
        own_predictor: bool,
    ) -> Result<(), CellDecodeError> {
        let groups = cell.h / 8;
        let cols = cell.w / 8;
        let total = groups * cols;
        let mut block = 0u32;
        let mut run_fill = 0u32;
        let mut run_mark = 0u32;
        while block < total {
            let g = block / cols;
            let c = block % cols;
            let y0 = i64::from(cell.y) + 8 * i64::from(g);
            let x0 = cell.x + 8 * c;
            if run_fill > 0 {
                run_fill -= 1;
                self.fill_doubled(cell.strip, y0, x0, 0, 4, own_predictor);
                block += 1;
                continue;
            }
            if run_mark > 0 {
                run_mark -= 1;
                block += 1;
                continue;
            }
            let mut pos = 0u8;
            while pos < 4 {
                if self.sticky {
                    let b = self.r.peek()?;
                    if b == 0xFA {
                        pos = 4;
                        continue;
                    }
                    if b == 0xFD {
                        self.fill_doubled(cell.strip, y0, x0, pos, 4, own_predictor);
                        pos = 4;
                        continue;
                    }
                }
                let b = self.r.byte()?;
                match b {
                    0x00..=0xF7 => {
                        let table = if pos.is_multiple_of(2) {
                            secondary
                        } else {
                            primary
                        };
                        let prev_row = y0 + 2 * i64::from(pos) - 1;
                        if own_predictor {
                            // Family F: the doubled deltas add to the
                            // motion-compensated content of both rows
                            // of the pair, pixel by pixel.
                            let row = y0 + 2 * i64::from(pos);
                            let pred = self.pred_dword_doubled(cell.strip, row - 1, x0, true);
                            let v = self.literal(table, b, pred)?;
                            let vb = v.to_le_bytes();
                            let pb = pred.to_le_bytes();
                            let d = [
                                vb[0].wrapping_sub(pb[0]),
                                vb[1].wrapping_sub(pb[1]),
                                vb[2].wrapping_sub(pb[2]),
                                vb[3].wrapping_sub(pb[3]),
                            ];
                            self.add_doubled_deltas(cell.strip, row, x0, d);
                        } else {
                            let pred = self.pred_dword_doubled(cell.strip, prev_row, x0, false);
                            let v = self.literal(table, b, pred)?;
                            self.store_doubled(cell.strip, y0 + 2 * i64::from(pos), x0, v);
                        }
                        pos += 1;
                    }
                    0xF9 | 0xFA => {
                        if pos != 0 {
                            return Err(CellDecodeError::BadEscape {
                                escape: b,
                                position: pos,
                            });
                        }
                        if b == 0xF9 {
                            self.sticky = !self.sticky;
                        }
                        if self.sticky {
                            self.r.pos -= 1;
                        }
                        pos = 4;
                    }
                    0xFB => {
                        let counter = self.r.byte()?;
                        let (fill, mark) = fb_counter(counter)?;
                        if fill > 0 {
                            let extra = if self.sticky {
                                self.sticky = false;
                                1
                            } else {
                                0
                            };
                            self.fill_doubled(cell.strip, y0, x0, pos, 4, own_predictor);
                            run_fill = (fill + extra).saturating_sub(1);
                        } else {
                            run_mark = mark.saturating_sub(1);
                        }
                        pos = 4;
                    }
                    0xFC | 0xFD => {
                        if b == 0xFC {
                            self.sticky = !self.sticky;
                        }
                        self.fill_doubled(cell.strip, y0, x0, pos, 4, own_predictor);
                        if self.sticky {
                            self.r.pos -= 1;
                        }
                        pos = 4;
                    }
                    0xFE => {
                        if pos > 1 {
                            return Err(CellDecodeError::BadEscape {
                                escape: b,
                                position: pos,
                            });
                        }
                        self.fill_doubled(cell.strip, y0, x0, pos, 3, own_predictor);
                        pos = 3;
                    }
                    0xFF => {
                        if pos != 0 {
                            return Err(CellDecodeError::BadEscape {
                                escape: b,
                                position: pos,
                            });
                        }
                        self.fill_doubled(cell.strip, y0, x0, 0, 2, own_predictor);
                        pos = 2;
                    }
                    0xF8 => {
                        return Err(CellDecodeError::BadEscape {
                            escape: b,
                            position: pos,
                        });
                    }
                }
            }
            block += 1;
        }
        Ok(())
    }

    #[inline]
    fn pred_dword(&self, strip: usize, y: i64, x: u32, own: bool) -> u32 {
        let s = &self.bufs.strips[strip];
        let i = PlaneBuffers::idx(if own { y } else { y - 1 }, x);
        u32::from_le_bytes(s[i..i + 4].try_into().unwrap())
    }

    #[inline]
    fn store_dword(&mut self, strip: usize, y: i64, x: u32, v: u32) {
        let s = &mut self.bufs.strips[strip];
        let i = PlaneBuffers::idx(y, x);
        s[i..i + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// Fill rows `from..to` of a 4-wide column block with each row's
    /// predictor (the row above, or the own content for B / D / F).
    fn fill_rows(&mut self, strip: usize, y0: i64, x0: u32, from: u8, to: u8, own: bool) {
        for p in from..to {
            let y = y0 + i64::from(p);
            let v = self.pred_dword(strip, y, x0, own);
            self.store_dword(strip, y, x0, v);
        }
    }

    /// The doubled-family predictor: every other byte of the row above
    /// (the doubled pixels' left halves).
    #[inline]
    fn pred_dword_doubled(&self, strip: usize, y: i64, x: u32, own: bool) -> u32 {
        let s = &self.bufs.strips[strip];
        let i = PlaneBuffers::idx(if own { y + 1 } else { y }, x);
        let p: &[u8; 7] = s[i..i + 7].try_into().unwrap();
        u32::from_le_bytes([p[0], p[2], p[4], p[6]])
    }

    /// Store a decoded 4-byte row doubled horizontally as rows
    /// `y + 1` (the row) and `y` (the average with the row above).
    fn store_doubled(&mut self, strip: usize, y: i64, x: u32, v: u32) {
        let b = v.to_le_bytes();
        let mut cur = [0u8; 8];
        for j in 0..4 {
            cur[2 * j] = b[j];
            cur[2 * j + 1] = b[j];
        }
        // The row pair's first row averages the row above with the
        // decoded row — except at the top of the strip, where the row
        // above is the predictor padding and the decoded row repeats
        // (fixture-arbitrated r451 / r459: every E cell of the
        // all-intra fixture, at the strip top and below).
        let repeat = y == 0;
        let s = &mut self.bufs.strips[strip];
        let above = PlaneBuffers::idx(y - 1, x);
        let i0 = PlaneBuffers::idx(y, x);
        let i1 = PlaneBuffers::idx(y + 1, x);
        let cur = u64::from_le_bytes(cur);
        let first = if repeat {
            cur
        } else {
            // Bytewise `((prev & 0x7f) + (cur & 0x7f)) >> 1`: the 7-bit
            // sums fit their bytes, and the final mask drops the bit
            // each byte's shift takes from the byte above it.
            let prev = u64::from_le_bytes(s[above..above + 8].try_into().unwrap());
            (((prev & LOW7) + (cur & LOW7)) >> 1) & LOW7
        };
        s[i0..i0 + 8].copy_from_slice(&first.to_le_bytes());
        s[i1..i1 + 8].copy_from_slice(&cur.to_le_bytes());
    }

    /// Family F: add per-pixel deltas `(d0, d0, d1, d1, d2, d2, d3, d3)`
    /// to the two rows of a pair over their own (motion-compensated)
    /// content.
    fn add_doubled_deltas(&mut self, strip: usize, y: i64, x: u32, d: [u8; 4]) {
        // Bytewise `(s + d) & 0x7f`: the low seven bits of a sum only
        // depend on the low seven bits of its terms, whose sums fit
        // their bytes.
        let d = u64::from_le_bytes([d[0], d[0], d[1], d[1], d[2], d[2], d[3], d[3]]) & LOW7;
        let s = &mut self.bufs.strips[strip];
        for row in [y, y + 1] {
            let i = PlaneBuffers::idx(row, x);
            let px = &mut s[i..i + 8];
            let v = u64::from_le_bytes((&*px).try_into().unwrap());
            px.copy_from_slice(&(((v & LOW7) + d) & LOW7).to_le_bytes());
        }
    }

    fn fill_doubled(&mut self, strip: usize, y0: i64, x0: u32, from: u8, to: u8, own: bool) {
        if own {
            // Predictor fills keep the motion-compensated content.
            return;
        }
        for p in from..to {
            let y = y0 + 2 * i64::from(p);
            let pred = self.pred_dword_doubled(strip, y - 1, x0, own);
            self.store_doubled(strip, y, x0, pred);
        }
    }
}

/// The low seven bits of each byte of a `u64`.
const LOW7: u64 = 0x7f7f_7f7f_7f7f_7f7f;

/// Apply one dyad (a 16-bit lane `b·256 + a`, `a` and `b` signed with
/// the low byte's borrow folded into `b`) to a lane of two predictor
/// pixels. **Fixture-arbitrated (r459):** each pixel takes its delta
/// rounded toward zero to an even value — `p + d − sign(d)·(d & 1)` —
/// which fits every one of the 404 luma samples of the all-intra
/// fixture's first frame that the plain `p + d` add misses on 64.
/// The mechanism behind the rounding is not in the spec (docs ask).
#[inline]
fn apply_dyad(pred: u16, dyad: u16) -> u16 {
    let a = i32::from((dyad & 0xff) as u8 as i8);
    let mut b = i32::from((dyad >> 8) as u8 as i8);
    if a < 0 {
        b += 1;
    }
    let p0 = i32::from(pred & 0xff);
    let p1 = i32::from(pred >> 8);
    let o0 = (p0 + a) & 0xff;
    let o1 = (p1 + b) & 0xff;
    ((o1 << 8) | o0) as u16
}

/// `spec/06 §4.4` counter byte: `(fill positions, mark positions)`.
fn fb_counter(counter: u8) -> Result<(u32, u32), CellDecodeError> {
    let n = u32::from(counter & 0x1f);
    if n == 0 || counter & 0xc0 != 0 {
        return Err(CellDecodeError::BadCounter { counter });
    }
    if counter & 0x20 != 0 {
        Ok((0, n))
    } else {
        Ok((n, 0))
    }
}

/// Decode one plane's payload into `bufs` (`spec/03 §3` root at heap
/// index `(1, 1)`), returning the plane statistics.
pub fn decode_plane(
    payload: &[u8],
    bufs: &mut PlaneBuffers,
    ctx: &PlaneContext<'_>,
) -> Result<PlaneStats, CellDecodeError> {
    let mut w = Walker {
        r: Reader::new(payload),
        bufs,
        ctx,
        stats: PlaneStats::default(),
        sticky: false,
    };
    w.walk_mc(1, 1)?;
    let mut stats = w.stats;
    stats.bytes = w.r.pos;
    // spec/03 §5.4 end-of-strip fix-up: duplicate the last column.
    let width = w.bufs.width;
    let full = w.bufs.banks.full_strip;
    let last = w.bufs.banks.last_strip;
    let height = w.bufs.height;
    let n = w.bufs.strips.len();
    for (i, s) in w.bufs.strips.iter_mut().enumerate() {
        let sw = if i + 1 == n { last } else { full };
        if sw == 0 || sw >= STRIP_ROW_STRIDE {
            continue;
        }
        for y in 0..height {
            let idx = PlaneBuffers::idx(i64::from(y), sw);
            s[idx] = s[idx - 1];
        }
    }
    let _ = width;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    //! Equivalence pins for the slice-based `mc_copy` and `to_pixels`:
    //! each is checked against a straightforward per-sample model of
    //! the same rule. The staged `IV32` fixtures carry full-pel
    //! vectors only, so the half-pel arms are pinned here.

    use super::super::codebook_seed::CodebookSeedArea;
    use super::*;

    /// Deterministic xorshift byte source.
    struct Rng(u64);

    impl Rng {
        fn step(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: u64) -> u64 {
            self.step() % n
        }

        fn fill(&mut self, bufs: &mut PlaneBuffers) {
            for s in &mut bufs.strips {
                for b in s.iter_mut() {
                    *b = self.step() as u8;
                }
            }
        }
    }

    fn mv(vert: i16, horiz: i16, vert_half: bool, horiz_half: bool) -> MotionVector {
        MotionVector {
            vertical_raw: 0,
            horizontal_raw: 0,
            vertical_scaled: vert,
            horizontal_scaled: horiz,
            vertical_halfpel_bit: u8::from(vert_half),
            horizontal_halfpel_bit: u8::from(horiz_half),
        }
    }

    /// Per-sample model of `spec/05 §5`/`§6`: every fetch bounds
    /// checked on its own; a fault leaves the destination untouched.
    fn mc_model(
        dst: &mut PlaneBuffers,
        reference: &PlaneBuffers,
        cell: &CellRect,
        mv: MotionVector,
    ) -> Result<(), CellDecodeError> {
        let rs = &reference.strips[cell.strip];
        let packed = mv.packed_mv();
        let offset = i64::from(packed >> 2);
        let (vh, hh) = (packed & 1 != 0, packed & 2 != 0);
        let stride = i64::from(STRIP_ROW_STRIDE);
        let fetch = |i: i64| -> Result<u32, CellDecodeError> {
            usize::try_from(i)
                .ok()
                .and_then(|i| rs.get(i))
                .map(|&v| u32::from(v))
                .ok_or(CellDecodeError::McOutOfRange)
        };
        let mut out = Vec::new();
        for r in 0..i64::from(cell.h) {
            for c in 0..cell.w {
                let d = PlaneBuffers::idx(i64::from(cell.y) + r, cell.x + c) as i64;
                let s = d + offset;
                let v = match (vh, hh) {
                    (false, false) => fetch(s)?,
                    (true, false) => (fetch(s)? + fetch(s + stride)?) >> 1,
                    (false, true) => (fetch(s)? + fetch(s + 1)?) >> 1,
                    (true, true) => {
                        let a = (fetch(s)? + fetch(s + 1)?) >> 1;
                        let b = (fetch(s + stride)? + fetch(s + stride + 1)?) >> 1;
                        (a + b) >> 1
                    }
                };
                out.push((d as usize, (v & 0x7f) as u8));
            }
        }
        let s = &mut dst.strips[cell.strip];
        for (d, v) in out {
            s[d] = v;
        }
        Ok(())
    }

    /// The codec tables a [`PlaneContext`] borrows (built once per
    /// test; `mc_copy` reads none of them).
    struct Tables {
        staging: StagingImage,
        arena: VqArena,
        lut: DyadDeltaTable,
    }

    impl Tables {
        fn new() -> Self {
            Tables {
                staging: StagingImage::build(&CodebookSeedArea::load()),
                arena: VqArena::new(),
                lut: DyadDeltaTable::load(),
            }
        }
    }

    /// Run the decoder's `mc_copy` on `dst`.
    fn mc_fast(
        t: &Tables,
        dst: &mut PlaneBuffers,
        reference: &PlaneBuffers,
        cell: &CellRect,
        mv: MotionVector,
    ) -> Result<(), CellDecodeError> {
        let ctx = PlaneContext {
            staging: &t.staging,
            arena: &t.arena,
            cb_offset: 0,
            lut: &t.lut,
            mvs: &[],
            reference: Some(reference),
        };
        let mut w = Walker {
            r: Reader::new(&[]),
            bufs: dst,
            ctx: &ctx,
            stats: PlaneStats::default(),
            sticky: false,
        };
        w.mc_copy(cell, mv)
    }

    /// Both implementations from the same starting buffers: same
    /// result, same buffers afterwards (untouched on a fault).
    fn check(
        t: &Tables,
        start: &PlaneBuffers,
        reference: &PlaneBuffers,
        cell: &CellRect,
        mv: MotionVector,
    ) -> Result<(), CellDecodeError> {
        let mut want = start.clone();
        let want_r = mc_model(&mut want, reference, cell, mv);
        let mut got = start.clone();
        let got_r = mc_fast(t, &mut got, reference, cell, mv);
        assert_eq!(got_r, want_r, "{cell:?} {mv:?}");
        assert!(got == want, "{cell:?} {mv:?}: planes differ");
        if got_r.is_err() {
            assert!(&got == start, "{cell:?} {mv:?}: written on a fault");
        }
        got_r
    }

    #[test]
    fn mc_copy_matches_per_sample_model_in_every_half_pel_mode() {
        let mut rng = Rng(0x1d0_3e3e_c0de);
        // 176 wide: a 160 strip and a 16 strip.
        let mut start = PlaneBuffers::luma(176, 144);
        let mut reference = start.clone();
        rng.fill(&mut start);
        rng.fill(&mut reference);
        let t = Tables::new();
        let mut tally = [[0u32; 2]; 4];
        let rounds = if cfg!(miri) { 200 } else { 4000 };
        for _ in 0..rounds {
            let strip = rng.below(2) as usize;
            let strip_w = if strip == 0 { 160 } else { 16 };
            let w = 4 * (1 + rng.below(strip_w / 4)) as u32;
            let h = 4 * (1 + rng.below(8)) as u32;
            let cell = CellRect {
                strip,
                x: 4 * rng.below(u64::from((strip_w as u32 - w) / 4 + 1)) as u32,
                y: 4 * rng.below(u64::from((144 - h) / 4 + 1)) as u32,
                w,
                h,
            };
            let mode = rng.below(4);
            let v = mv(
                rng.below(41) as i16 - 20,
                rng.below(81) as i16 - 40,
                mode & 1 != 0,
                mode & 2 != 0,
            );
            let ok = check(&t, &start, &reference, &cell, v).is_ok();
            tally[mode as usize][usize::from(ok)] += 1;
        }
        for (mode, t) in tally.iter().enumerate() {
            assert!(t[0] > 0 && t[1] > 0, "mode {mode}: faults/ok {t:?}");
        }
    }

    #[test]
    fn mc_copy_range_edges_are_exact() {
        let mut rng = Rng(0xbead_f00d);
        let t = Tables::new();
        let mut start = PlaneBuffers::luma(176, 144);
        let mut reference = start.clone();
        rng.fill(&mut start);
        rng.fill(&mut reference);
        let stride = i64::from(STRIP_ROW_STRIDE);
        let len = reference.strips[0].len() as i64;
        for mode in 0..4u8 {
            let (vh, hh) = (mode & 1 != 0, mode & 2 != 0);
            // First fetch: the cell at (0, 0) reaches back to index 0
            // (the predictor row) but not to -1.
            let top = CellRect {
                strip: 0,
                x: 0,
                y: 0,
                w: 8,
                h: 4,
            };
            let first = PlaneBuffers::idx(0, 0) as i64;
            let at = |o: i64| mv(0, o as i16, vh, hh);
            assert_eq!(check(&t, &start, &reference, &top, at(-first)), Ok(()));
            assert_eq!(
                check(&t, &start, &reference, &top, at(-first - 1)),
                Err(CellDecodeError::McOutOfRange),
                "mode {mode}"
            );
            // Last fetch: the bottom-right cell of the strip reaches
            // the buffer's last byte but not one past it.
            let bottom = CellRect {
                strip: 0,
                x: 152,
                y: 136,
                w: 8,
                h: 8,
            };
            let last = PlaneBuffers::idx(143, 152) as i64
                + 7
                + if vh { stride } else { 0 }
                + i64::from(hh);
            let o = len - 1 - last;
            assert_eq!(check(&t, &start, &reference, &bottom, at(o)), Ok(()));
            assert_eq!(
                check(&t, &start, &reference, &bottom, at(o + 1)),
                Err(CellDecodeError::McOutOfRange),
                "mode {mode}"
            );
        }
    }

    #[test]
    fn to_pixels_matches_sample_with_crop_and_padding() {
        let mut rng = Rng(0xc0ff_ee11);
        // A 168-wide picture: 44-wide chroma plane (two strips, 40 +
        // 4) cropped to a 42-wide output; 120 high: 32 rows to 30.
        for mut bufs in [
            PlaneBuffers::chroma(168, 120),
            PlaneBuffers::luma(176, 144),
            PlaneBuffers::luma(16, 16),
        ] {
            rng.fill(&mut bufs);
            let (pw, ph) = (bufs.width, bufs.height);
            for (ow, oh) in [
                (pw, ph),
                (pw - 2, ph - 2),
                (pw + 5, ph + 3),
                (1, 1),
                (0, 4),
                (4, 0),
                (41u32.min(pw), 3),
            ] {
                let mut want = Vec::new();
                for y in 0..oh {
                    for x in 0..ow {
                        want.push((bufs.sample(x, y).unwrap_or(0) & 0x7f) << 1);
                    }
                }
                assert_eq!(bufs.to_pixels(ow, oh), want, "{pw}x{ph} -> {ow}x{oh}");
            }
        }
    }
}
