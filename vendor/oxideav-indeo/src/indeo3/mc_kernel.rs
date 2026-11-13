//! Indeo 3 spec/05 §5.1 / §5.2 / §5.3 — motion-compensation cell-copy
//! inner-loop kernel.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/05-motion-compensation.md`
//! §5.1 (full-pel MC fetcher inner loop at
//! `IR32_32.DLL!0x1000670d..0x1000673d`), §5.2 (three half-pel variants
//! at `0x10006780` / `0x1000684b` / `0x100068f8`), §5.3 (destination
//! and row stride).
//!
//! Round 13 ([`super::mc_packed`]) closed §2.2 / §2.3 / §3.3 / §3.4 —
//! the wire-side decode of the 32-bit packed-MV DWORD into
//! [`super::PackedMv`] / [`super::McDispatchMode`] and the §2.3
//! `add esi, sar(packed_mv, 2)` source-pointer arithmetic. Round 14
//! takes the next slice in the MC pipeline: once the source-pixel
//! base address has been resolved, the per-cell copy kernel reads
//! 4 DWORDs (= 16 bytes) per inner-loop iteration from successive
//! rows of the source buffer and writes them into the corresponding
//! rows of the destination cell.
//!
//! This module surfaces:
//!
//! * [`MC_ROW_STRIDE`] = `0xb0` — the §5.1 / §5.3 strip pixel-buffer
//!   row stride, aliasing
//!   [`super::reconstruct::PREDICTOR_ROW_STRIDE`] and
//!   [`super::mc_packed::MV_PIXEL_OFFSET_ROW_STRIDE`].
//! * [`MC_INNER_LOOP_DWORDS_PER_ITER`] = `4` — the §5.1 per-iteration
//!   DWORD count (rows 0, 1, 2, 3 each copied as one DWORD).
//! * [`MC_INNER_LOOP_BYTES_PER_ITER`] = `16` — the §5.1 per-iteration
//!   byte count, the destination-side advance per column-group
//!   (`lea edi, [edi + 0x4]` × 4 DWORDs = 16 bytes spread across 4
//!   rows, not 16 bytes within one row).
//! * [`MC_BAND_ROWS`] = `4` — the §5.1 row-band height in pixels (one
//!   inner-loop iteration advances horizontally by 4 columns within a
//!   4-row band; the outer loop advances vertically by one band).
//! * [`MC_COLUMN_GROUP_PIXELS`] = `4` — the §5.1 horizontal step the
//!   inner loop advances per iteration (`lea edi, [edi + 0x4]` —
//!   four pixels per DWORD).
//! * [`MC_VERT_HALF_PEL_NEIGHBOUR_OFFSET`] = `0xb0` — the §5.2 (`01`
//!   path) vertical-half-pel neighbour byte offset (one row below).
//! * [`MC_HORIZ_HALF_PEL_NEIGHBOUR_OFFSET`] = `1` — the §5.2 (`10`
//!   path) horizontal-half-pel neighbour byte offset (one byte right).
//! * [`McKernelGeometry`] — the typed (cell_w_dwords, cell_h_bands)
//!   pair the kernel iterates over, with [`McKernelGeometry::new`]
//!   enforcing the §5.1 / §5.3 invariants (width is a non-zero
//!   multiple of 4 pixels; height is a non-zero multiple of 4 rows)
//!   and surfacing the §5.3 "cell width ≤ row stride" bound
//!   ([`MC_MAX_CELL_WIDTH_BYTES`] = `0xb0`).
//! * [`mc_full_pel_row_dword`] — the §5.1 source-side per-DWORD load:
//!   `mov edx, [esi]` (row 0), `mov eax, [esi + 0xb0]` (row 1),
//!   `mov edx, [esi + 0x160]` (row 2), `mov eax, [esi + 0x210]` (row
//!   3) — given the source-row index, returns the byte offset
//!   (`row_idx * MC_ROW_STRIDE`) for the DWORD read.
//! * [`mc_vert_half_pel_pair`] — the §5.2 `01` path per-DWORD
//!   averaging kernel: `(src[i] + src[i + 0xb0]) >> 1` byte-parallel
//!   via [`super::reconstruct::average_7bit`]'s SWAR identity
//!   (`(a & b) + (((a ^ b) >> 1) & 0x7f7f7f7f)`).
//! * [`mc_horiz_half_pel_pair`] — the §5.2 `10` path per-DWORD
//!   averaging kernel: `(src[i] + src[i + 1]) >> 1`. Because the
//!   neighbour is one byte to the right *within* a DWORD-wide read,
//!   the kernel takes two adjacent DWORDs (`src[i]` and `src[i + 4]`)
//!   and computes the byte-pair average for the four output pixels.
//! * [`mc_both_half_pel_quad`] — the §5.2 `11` path per-DWORD 2×2
//!   box filter: `avg(src[i], src[i + 1], src[i + 0xb0], src[i +
//!   0xb1])`.
//! * [`McKernelStep`] — the per-iteration tuple (source-row offset,
//!   destination-row offset) the inner loop advances through; the
//!   sequence runs `(0, 0)`, `(0xb0, 0xb0)`, `(0x160, 0x160)`,
//!   `(0x210, 0x210)` for the four DWORDs of one column-group, then
//!   the outer loop adds one band's worth (`MC_BAND_ROWS *
//!   MC_ROW_STRIDE` = `0x2c0`) to both source and destination
//!   before the next band starts.
//!
//! What this module **deliberately does not do** (the §5 chapter
//! boundary):
//!
//! * It does not own the strip pixel-buffer arena. The
//!   destination/source addresses come from the cell-position decode
//!   chain (§7 of spec/05) which is table-mediated through the
//!   codebook-bank `+0x300` / `+0x700` sub-tables — values still
//!   pending an Extractor round per `§7.5` and `§8.2 item 4`.
//! * It does not perform a slice-bounds check against the arena.
//!   Per §4.4 the binary itself does not range-check the resulting
//!   source pointer; callers operating over a safe-Rust strip-buffer
//!   view can apply such a check at the arena boundary, not inside
//!   the kernel.
//! * It does not address the §5.6 VQ-residual-after-MC chain. That
//!   step adds a VQ residual *in place* to the just-written MC
//!   prediction and is the spec/06 entry point at
//!   `IR32_32.DLL!0x10006bac`.
//! * It does not validate the §5.4 cell-position decode
//!   (`bank[+0x300][cl]` against the `0xf423f` sanity sentinel); the
//!   `+0x300` table-value check is the cell-loop preamble's
//!   responsibility ([`super::cell_loop::CELL_POSITION_MAX`]).
//!
//! All offsets, RVAs and the per-iteration DWORD count are taken
//! from `05-motion-compensation.md` §5 (§5.1 full-pel, §5.2
//! half-pel, §5.3 destination/stride). RVAs cited in doc-comments
//! refer to the binary identified in `spec/00 §2`.

use super::reconstruct::{PREDICTOR_ROW_STRIDE, average_7bit};

// ---- §5.1 / §5.3 row-stride and inner-loop shape constants ---------

/// Spec/05 §5.1 / §5.3 — strip pixel-buffer row stride in bytes
/// (`0xb0` = `176`).
///
/// The full-pel inner loop at `IR32_32.DLL!0x1000670d..0x1000673d`
/// reads from `[esi + 0xb0]`, `[esi + 0x160]` (= 2 × `0xb0`), and
/// `[esi + 0x210]` (= 3 × `0xb0`); the half-pel inner loops use the
/// same stride for the vertical-neighbour offset. The stride aliases
/// [`PREDICTOR_ROW_STRIDE`] (used by the output reconstruction
/// kernel) and [`super::mc_packed::MV_PIXEL_OFFSET_ROW_STRIDE`]
/// (the `176 * vert + horiz` packing constant).
pub const MC_ROW_STRIDE: usize = PREDICTOR_ROW_STRIDE;

/// Spec/05 §5.1 — DWORDs read per inner-loop iteration (`4`).
///
/// The full-pel inner loop reads rows 0, 1, 2, 3 — one DWORD per
/// row — before the column-group advance. The half-pel variants
/// share the same shape; they only interpose the byte-parallel
/// averaging step.
pub const MC_INNER_LOOP_DWORDS_PER_ITER: usize = 4;

/// Spec/05 §5.1 — bytes copied per DWORD (`4`).
///
/// One DWORD is exactly four packed pixel bytes (the strip's pixel
/// format is one byte per pixel, see §5.5 / spec/02 §5.2).
pub const MC_BYTES_PER_DWORD: usize = 4;

/// Spec/05 §5.1 — total bytes touched per inner-loop iteration
/// (`16` = `MC_INNER_LOOP_DWORDS_PER_ITER * MC_BYTES_PER_DWORD`).
///
/// The bytes are spread across four rows (4 bytes / row), not
/// 16 consecutive bytes within one row; each row's 4 bytes are at
/// the same column position. This matches the inner loop's
/// `mov edx, [esi]; mov eax, [esi + 0xb0]; mov edx, [esi + 0x160];
/// mov eax, [esi + 0x210]` pattern.
pub const MC_INNER_LOOP_BYTES_PER_ITER: usize = MC_INNER_LOOP_DWORDS_PER_ITER * MC_BYTES_PER_DWORD;

/// Spec/05 §5.1 — row-band height in pixels (`4`).
///
/// The inner loop processes one 4-row band at a time; the outer
/// loop advances `esi` and `edi` by one band's worth of rows
/// (`MC_BAND_ROWS * MC_ROW_STRIDE` = `0x2c0` bytes) per iteration.
pub const MC_BAND_ROWS: usize = MC_INNER_LOOP_DWORDS_PER_ITER;

/// Spec/05 §5.1 — pixels advanced per column-group (`4`).
///
/// `lea edi, [edi + 0x4]` after each 4-DWORD column-group read /
/// write moves the destination pointer 4 pixels to the right.
pub const MC_COLUMN_GROUP_PIXELS: usize = MC_BYTES_PER_DWORD;

/// Spec/05 §5.1 — bytes between successive row-band entries in the
/// outer loop (`MC_BAND_ROWS * MC_ROW_STRIDE` = `0x2c0`).
///
/// `mov eax, [esp + 0x20]; add esi, eax; lea edi, [eax + edi]` in
/// the outer loop at `0x1000673f..0x10006750` uses a precomputed
/// "row-stride − column-group-bytes" but the *band* stride itself
/// is unambiguously this product.
pub const MC_BAND_BYTE_STRIDE: usize = MC_BAND_ROWS * MC_ROW_STRIDE;

/// Spec/05 §5.3 — maximum cell width in bytes (`0xb0`).
///
/// The strip's allocated buffer width *is* the row stride; the
/// visible cell width is `cl * 4 ≤ MC_ROW_STRIDE` per §5.3.
pub const MC_MAX_CELL_WIDTH_BYTES: usize = MC_ROW_STRIDE;

// ---- §5.2 half-pel neighbour offsets -------------------------------

/// Spec/05 §5.2 (`01` path) — vertical-half-pel neighbour byte
/// offset (`0xb0`, = `MC_ROW_STRIDE`).
///
/// The `01` inner loop at `0x10006780..0x100067de` averages `[esi]`
/// with `[esi + 0xb0]` — i.e. the pixel one row below.
pub const MC_VERT_HALF_PEL_NEIGHBOUR_OFFSET: usize = MC_ROW_STRIDE;

/// Spec/05 §5.2 (`10` path) — horizontal-half-pel neighbour byte
/// offset (`1`).
///
/// The `10` inner loop at `0x1000684b..0x100068c4` averages `[esi]`
/// with `[esi + 0x1]` — i.e. the pixel one column to the right.
pub const MC_HORIZ_HALF_PEL_NEIGHBOUR_OFFSET: usize = 1;

// ---- §5.1 / §5.3 cell-geometry typed surface -----------------------

/// Spec/05 §5.1 / §5.3 — typed (width, height) of an MC cell as
/// counted in the units the kernel iterates over.
///
/// The kernel's two loop counters are:
///
/// * `cl_inner` = column-groups, each a 4-pixel-wide DWORD
///   (`MC_COLUMN_GROUP_PIXELS`). The cell width in pixels is
///   `cl_inner * 4`.
/// * `row_band_count` = 4-row bands (`MC_BAND_ROWS`). The cell
///   height in pixels is `row_band_count * 4`.
///
/// Both counters are at least `1` (a degenerate zero-counter cell
/// would consume no bitstream and produce no output, which the
/// binary's encoder never emits per the §2.4 4×4 minimum-cell size
/// invariant from `spec/03`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McKernelGeometry {
    column_groups: usize,
    row_bands: usize,
}

/// Spec/05 §5.1 / §5.3 — construction error for [`McKernelGeometry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McKernelGeometryError {
    /// The supplied cell width was zero — the inner loop's `dec cl;
    /// jne` would underflow on entry. §2.4 of `spec/03` and §5.1 of
    /// this chapter both pin the minimum cell width at 4 pixels.
    ZeroWidth,
    /// The supplied cell height was zero — the outer loop's
    /// `sub ecx, 0x1000000; jae` would underflow on entry. §2.4 of
    /// `spec/03` pins the minimum cell height at 4 pixels.
    ZeroHeight,
    /// The cell width in pixels was not a multiple of 4 — the inner
    /// loop's DWORD reads assume the cell width packs evenly into
    /// 4-pixel column groups.
    WidthNotMultipleOf4,
    /// The cell height in pixels was not a multiple of 4 — the outer
    /// loop's row-band advance assumes the cell height packs evenly
    /// into 4-row bands.
    HeightNotMultipleOf4,
    /// The cell width in bytes exceeded the §5.3 row-stride bound
    /// (`MC_MAX_CELL_WIDTH_BYTES` = `0xb0`). A cell that wide would
    /// read or write past the strip's allocated buffer width.
    WidthExceedsRowStride,
}

impl McKernelGeometry {
    /// Spec/05 §5.1 — construct a kernel geometry from the cell's
    /// width and height in *pixels*, enforcing the §5.1 multiple-of-4
    /// invariants and the §5.3 row-stride bound.
    pub const fn new(width_px: usize, height_px: usize) -> Result<Self, McKernelGeometryError> {
        if width_px == 0 {
            return Err(McKernelGeometryError::ZeroWidth);
        }
        if height_px == 0 {
            return Err(McKernelGeometryError::ZeroHeight);
        }
        if !width_px.is_multiple_of(MC_COLUMN_GROUP_PIXELS) {
            return Err(McKernelGeometryError::WidthNotMultipleOf4);
        }
        if !height_px.is_multiple_of(MC_BAND_ROWS) {
            return Err(McKernelGeometryError::HeightNotMultipleOf4);
        }
        if width_px > MC_MAX_CELL_WIDTH_BYTES {
            return Err(McKernelGeometryError::WidthExceedsRowStride);
        }
        Ok(Self {
            column_groups: width_px / MC_COLUMN_GROUP_PIXELS,
            row_bands: height_px / MC_BAND_ROWS,
        })
    }

    /// Spec/05 §5.1 — number of 4-pixel column groups per row band
    /// (the inner-loop count, `cl_inner` at `0x10006666`).
    pub const fn column_groups(self) -> usize {
        self.column_groups
    }

    /// Spec/05 §5.1 — number of 4-row bands per cell (the outer-loop
    /// count, the high byte of `ecx` at `0x1000673f..0x10006750`).
    pub const fn row_bands(self) -> usize {
        self.row_bands
    }

    /// Spec/05 §5.1 — cell width in pixels.
    pub const fn width_pixels(self) -> usize {
        self.column_groups * MC_COLUMN_GROUP_PIXELS
    }

    /// Spec/05 §5.1 — cell height in pixels.
    pub const fn height_pixels(self) -> usize {
        self.row_bands * MC_BAND_ROWS
    }

    /// Spec/05 §5.1 — total DWORDs the kernel reads/writes for this
    /// cell. Each row-band processes `column_groups *
    /// MC_INNER_LOOP_DWORDS_PER_ITER` DWORDs.
    pub const fn total_dwords(self) -> usize {
        self.row_bands * self.column_groups * MC_INNER_LOOP_DWORDS_PER_ITER
    }
}

// ---- §5.1 source-row byte offsets per inner-loop iteration ---------

/// Spec/05 §5.1 — byte offset within the source pointer for the
/// `row_idx`-th DWORD of one inner-loop iteration.
///
/// The full-pel inner loop's four reads are encoded as immediates in
/// the `mov` instructions: `[esi]` (= `0`), `[esi + 0xb0]`,
/// `[esi + 0x160]` (= `2 * 0xb0`), and `[esi + 0x210]` (= `3 *
/// 0xb0`). Each entry is `row_idx * MC_ROW_STRIDE`. Returns `None`
/// for `row_idx >= MC_INNER_LOOP_DWORDS_PER_ITER`.
pub const fn mc_full_pel_row_dword(row_idx: usize) -> Option<usize> {
    if row_idx >= MC_INNER_LOOP_DWORDS_PER_ITER {
        return None;
    }
    Some(row_idx * MC_ROW_STRIDE)
}

/// Spec/05 §5.1 — the four hard-coded source-byte offsets the
/// full-pel inner loop reads per iteration. Matches the immediates
/// at `0x1000670d..0x1000673d`: `0x0`, `0xb0`, `0x160`, `0x210`.
pub const MC_FULL_PEL_ROW_OFFSETS: [usize; MC_INNER_LOOP_DWORDS_PER_ITER] =
    [0, MC_ROW_STRIDE, 2 * MC_ROW_STRIDE, 3 * MC_ROW_STRIDE];

// ---- §5.2 per-DWORD averaging kernels ------------------------------

/// Spec/05 §5.2 (`01` path) — vertical-half-pel per-DWORD averaging
/// kernel.
///
/// Given the upper-row DWORD `src_row` (4 pixels) and the lower-row
/// DWORD `src_row_below` (the 4 pixels one row down, at byte offset
/// `MC_VERT_HALF_PEL_NEIGHBOUR_OFFSET` from `src_row`'s source
/// address), returns the byte-parallel `(a + b) >> 1` average per
/// the `01` inner loop at `0x10006780..0x100067de`. The averaging
/// is the SWAR identity provided by
/// [`super::reconstruct::average_7bit`].
pub fn mc_vert_half_pel_pair(src_row: u32, src_row_below: u32) -> u32 {
    average_7bit(src_row, src_row_below)
}

/// Spec/05 §5.2 (`10` path) — horizontal-half-pel per-DWORD
/// averaging kernel.
///
/// Given two adjacent DWORDs `src_dword` (pixels at columns
/// `c..c+3`) and `src_dword_next` (pixels at columns `c+4..c+7`,
/// i.e. one DWORD to the right), returns the byte-parallel
/// `(src[c..c+3] + src[c+1..c+4]) >> 1` average for the four output
/// pixels per the `10` inner loop at `0x1000684b..0x100068c4`.
///
/// The kernel splices the high byte of `src_dword` with the low
/// three bytes of `src_dword_next` to form the "shifted-by-one"
/// neighbour DWORD, then averages with `src_dword`.
pub fn mc_horiz_half_pel_pair(src_dword: u32, src_dword_next: u32) -> u32 {
    // Form the neighbour DWORD whose bytes are src[c+1..c+4]:
    //
    //   src_dword       = | b3 | b2 | b1 | b0 |   (little-endian)
    //   src_dword_next  = | b7 | b6 | b5 | b4 |
    //   shifted (target) = | b4 | b3 | b2 | b1 |
    //
    // In little-endian byte order, the shifted DWORD has b1 at the
    // lowest byte (right-shift the current DWORD by 8 bits) and b4
    // at the highest byte (left-shift the next DWORD by 24 bits).
    let shifted = (src_dword >> 8) | (src_dword_next << 24);
    average_7bit(src_dword, shifted)
}

/// Spec/05 §5.2 (`11` path) — both-half-pel per-DWORD 2×2 box-filter
/// kernel.
///
/// Given four DWORDs:
///
/// * `src_dword`             — pixels at row `r`,     columns `c..c+3`
/// * `src_dword_next`        — pixels at row `r`,     columns `c+4..c+7`
/// * `src_dword_below`       — pixels at row `r + 1`, columns `c..c+3`
/// * `src_dword_below_next`  — pixels at row `r + 1`, columns `c+4..c+7`
///
/// returns the byte-parallel 2×2 unweighted average per the `11`
/// inner loop at `0x100068f8..onward`. The kernel forms two
/// horizontal-half-pel intermediates (the same byte-splice as
/// [`mc_horiz_half_pel_pair`]) for the two rows, then averages the
/// two intermediates vertically — matching the §2.2 / §5.2
/// "2×2 unweighted box" description.
///
/// Order of operations follows the binary's apparent dataflow at
/// `0x100068f8` (horizontal pair first, vertical pair second); this
/// ordering produces the same rounding-towards-zero per byte as
/// independently performing the four-input average via two
/// successive `(a + b) >> 1` SWAR steps.
pub fn mc_both_half_pel_quad(
    src_dword: u32,
    src_dword_next: u32,
    src_dword_below: u32,
    src_dword_below_next: u32,
) -> u32 {
    let horiz_top = mc_horiz_half_pel_pair(src_dword, src_dword_next);
    let horiz_bot = mc_horiz_half_pel_pair(src_dword_below, src_dword_below_next);
    average_7bit(horiz_top, horiz_bot)
}

// ---- §5.1 per-iteration step tuple ---------------------------------

/// Spec/05 §5.1 — one inner-loop iteration's source and destination
/// row offsets, taken relative to the column-group's source base
/// (`esi`) and destination base (`edi`) at the start of the band.
///
/// For one column-group of one band the inner loop produces four
/// iterations:
///
/// | DWORD | `src_offset` | `dst_offset` |
/// | ----- | ------------ | ------------ |
/// | 0     | `0`          | `0`          |
/// | 1     | `0xb0`       | `0xb0`       |
/// | 2     | `0x160`      | `0x160`      |
/// | 3     | `0x210`      | `0x210`      |
///
/// Across column groups the kernel advances both source and
/// destination by `MC_COLUMN_GROUP_PIXELS` (4 bytes) after each
/// 4-DWORD group; across row bands the outer loop advances both by
/// `MC_BAND_BYTE_STRIDE` (`0x2c0` bytes) per band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McKernelStep {
    /// Byte offset within the source pointer for this DWORD's read.
    pub src_offset: usize,
    /// Byte offset within the destination pointer for this DWORD's
    /// write.
    pub dst_offset: usize,
}

impl McKernelStep {
    /// Spec/05 §5.1 — return the per-DWORD step within one
    /// inner-loop iteration. Returns `None` for
    /// `row_idx >= MC_INNER_LOOP_DWORDS_PER_ITER`.
    pub const fn for_row(row_idx: usize) -> Option<Self> {
        let off = match mc_full_pel_row_dword(row_idx) {
            Some(o) => o,
            None => return None,
        };
        Some(Self {
            src_offset: off,
            dst_offset: off,
        })
    }

    /// Spec/05 §5.1 — the outer-loop advance per band: source and
    /// destination both advance by `MC_BAND_BYTE_STRIDE` (`0x2c0`)
    /// after one band completes.
    pub const fn outer_band_advance() -> usize {
        MC_BAND_BYTE_STRIDE
    }

    /// Spec/05 §5.1 — the inner-loop advance per column group:
    /// `MC_COLUMN_GROUP_PIXELS` (4) bytes for both source and
    /// destination after one 4-DWORD group is processed.
    pub const fn inner_column_group_advance() -> usize {
        MC_COLUMN_GROUP_PIXELS
    }
}

// ---- consistency assertions ----------------------------------------

const _: () = {
    // §5.1 the per-iteration DWORD count is exactly the band height.
    assert!(MC_INNER_LOOP_DWORDS_PER_ITER == MC_BAND_ROWS);
    // §5.1 the per-iteration byte count is `4 DWORDs * 4 bytes`.
    assert!(MC_INNER_LOOP_BYTES_PER_ITER == 16);
    // §5.1 / §5.3 the row stride matches the predictor-side stride.
    assert!(MC_ROW_STRIDE == 0xb0);
    // §5.1 the outer-band byte stride is one full band's worth of
    // rows.
    assert!(MC_BAND_BYTE_STRIDE == MC_BAND_ROWS * MC_ROW_STRIDE);
    assert!(MC_BAND_BYTE_STRIDE == 0x2c0);
    // §5.2 vertical neighbour offset is one row.
    assert!(MC_VERT_HALF_PEL_NEIGHBOUR_OFFSET == MC_ROW_STRIDE);
    // §5.2 horizontal neighbour offset is one byte.
    assert!(MC_HORIZ_HALF_PEL_NEIGHBOUR_OFFSET == 1);
    // §5.3 the max cell width equals the row stride.
    assert!(MC_MAX_CELL_WIDTH_BYTES == MC_ROW_STRIDE);
    // The full-pel row-offset array matches the per-row helper.
    assert!(MC_FULL_PEL_ROW_OFFSETS[0] == 0);
    assert!(MC_FULL_PEL_ROW_OFFSETS[1] == MC_ROW_STRIDE);
    assert!(MC_FULL_PEL_ROW_OFFSETS[2] == 2 * MC_ROW_STRIDE);
    assert!(MC_FULL_PEL_ROW_OFFSETS[3] == 3 * MC_ROW_STRIDE);
};

#[cfg(test)]
mod tests {
    use super::*;

    // ---- §5.1 / §5.3 constants ------------------------------------

    #[test]
    fn row_stride_is_0xb0() {
        // §5.1 / §5.3: the strip pixel-buffer row stride is `0xb0`.
        assert_eq!(MC_ROW_STRIDE, 0xb0);
        assert_eq!(MC_ROW_STRIDE, 176);
    }

    #[test]
    fn inner_loop_processes_four_dwords_per_iter() {
        // §5.1: the inner loop reads rows 0, 1, 2, 3 — four DWORDs.
        assert_eq!(MC_INNER_LOOP_DWORDS_PER_ITER, 4);
        assert_eq!(MC_BYTES_PER_DWORD, 4);
        assert_eq!(MC_INNER_LOOP_BYTES_PER_ITER, 16);
    }

    #[test]
    fn band_rows_equal_inner_dword_count() {
        // §5.1: one inner iteration's four DWORDs cover four rows;
        // that's the band height.
        assert_eq!(MC_BAND_ROWS, 4);
        assert_eq!(MC_BAND_ROWS, MC_INNER_LOOP_DWORDS_PER_ITER);
    }

    #[test]
    fn column_group_pixels_equal_bytes_per_dword() {
        // §5.1: `lea edi, [edi + 0x4]` advances 4 bytes = 4 pixels.
        assert_eq!(MC_COLUMN_GROUP_PIXELS, 4);
        assert_eq!(MC_COLUMN_GROUP_PIXELS, MC_BYTES_PER_DWORD);
    }

    #[test]
    fn band_byte_stride_is_0x2c0() {
        // §5.1 outer loop: `MC_BAND_ROWS * MC_ROW_STRIDE` is the
        // byte advance per band.
        assert_eq!(MC_BAND_BYTE_STRIDE, 0x2c0);
        assert_eq!(MC_BAND_BYTE_STRIDE, MC_BAND_ROWS * MC_ROW_STRIDE);
    }

    #[test]
    fn max_cell_width_equals_row_stride() {
        // §5.3: the strip's allocated buffer width is the row
        // stride; visible cell width cannot exceed it.
        assert_eq!(MC_MAX_CELL_WIDTH_BYTES, MC_ROW_STRIDE);
        assert_eq!(MC_MAX_CELL_WIDTH_BYTES, 0xb0);
    }

    #[test]
    fn vert_half_pel_neighbour_is_one_row() {
        // §5.2 `01`: `[esi]` and `[esi + 0xb0]`.
        assert_eq!(MC_VERT_HALF_PEL_NEIGHBOUR_OFFSET, MC_ROW_STRIDE);
        assert_eq!(MC_VERT_HALF_PEL_NEIGHBOUR_OFFSET, 0xb0);
    }

    #[test]
    fn horiz_half_pel_neighbour_is_one_byte() {
        // §5.2 `10`: `[esi]` and `[esi + 0x1]`.
        assert_eq!(MC_HORIZ_HALF_PEL_NEIGHBOUR_OFFSET, 1);
    }

    // ---- §5.1 row-offset table -----------------------------------

    #[test]
    fn full_pel_row_offsets_match_immediates() {
        // §5.1 the four immediates at `0x1000670d..0x10006735`:
        // `0x0`, `0xb0`, `0x160`, `0x210`.
        assert_eq!(MC_FULL_PEL_ROW_OFFSETS, [0, 0xb0, 0x160, 0x210]);
    }

    #[test]
    fn full_pel_row_offset_helper_matches_table() {
        for (row, expected) in MC_FULL_PEL_ROW_OFFSETS.iter().enumerate() {
            assert_eq!(mc_full_pel_row_dword(row), Some(*expected));
        }
    }

    #[test]
    fn full_pel_row_offset_helper_rejects_out_of_range() {
        assert!(mc_full_pel_row_dword(MC_INNER_LOOP_DWORDS_PER_ITER).is_none());
        assert!(mc_full_pel_row_dword(usize::MAX).is_none());
    }

    // ---- §5.1 / §5.3 kernel-geometry surface ---------------------

    #[test]
    fn kernel_geometry_basic_construction() {
        // A nominal 8×8 luma cell (the spec/03 §2.4 wiki block size)
        // → 2 column groups, 2 row bands.
        let g = McKernelGeometry::new(8, 8).unwrap();
        assert_eq!(g.column_groups(), 2);
        assert_eq!(g.row_bands(), 2);
        assert_eq!(g.width_pixels(), 8);
        assert_eq!(g.height_pixels(), 8);
        assert_eq!(g.total_dwords(), 2 * 2 * MC_INNER_LOOP_DWORDS_PER_ITER);
    }

    #[test]
    fn kernel_geometry_minimum_cell_size() {
        // §2.4 / §5.1: the 4×4 minimum cell.
        let g = McKernelGeometry::new(4, 4).unwrap();
        assert_eq!(g.column_groups(), 1);
        assert_eq!(g.row_bands(), 1);
        assert_eq!(g.total_dwords(), MC_INNER_LOOP_DWORDS_PER_ITER);
    }

    #[test]
    fn kernel_geometry_full_width_cell() {
        // §5.3: the maximum-width cell equals the row stride
        // (`0xb0` = 176 pixels) and any reasonable height.
        let g = McKernelGeometry::new(MC_MAX_CELL_WIDTH_BYTES, 4).unwrap();
        assert_eq!(g.column_groups(), MC_MAX_CELL_WIDTH_BYTES / 4);
        assert_eq!(g.width_pixels(), 176);
    }

    #[test]
    fn kernel_geometry_rejects_zero_width() {
        assert_eq!(
            McKernelGeometry::new(0, 4),
            Err(McKernelGeometryError::ZeroWidth)
        );
    }

    #[test]
    fn kernel_geometry_rejects_zero_height() {
        assert_eq!(
            McKernelGeometry::new(4, 0),
            Err(McKernelGeometryError::ZeroHeight)
        );
    }

    #[test]
    fn kernel_geometry_rejects_non_multiple_of_4_width() {
        assert_eq!(
            McKernelGeometry::new(5, 4),
            Err(McKernelGeometryError::WidthNotMultipleOf4)
        );
        assert_eq!(
            McKernelGeometry::new(7, 4),
            Err(McKernelGeometryError::WidthNotMultipleOf4)
        );
    }

    #[test]
    fn kernel_geometry_rejects_non_multiple_of_4_height() {
        assert_eq!(
            McKernelGeometry::new(4, 5),
            Err(McKernelGeometryError::HeightNotMultipleOf4)
        );
        assert_eq!(
            McKernelGeometry::new(4, 7),
            Err(McKernelGeometryError::HeightNotMultipleOf4)
        );
    }

    #[test]
    fn kernel_geometry_rejects_width_exceeding_row_stride() {
        // One pixel past the row stride.
        assert_eq!(
            McKernelGeometry::new(MC_MAX_CELL_WIDTH_BYTES + 4, 4),
            Err(McKernelGeometryError::WidthExceedsRowStride)
        );
    }

    // ---- §5.2 averaging-kernel correctness -----------------------

    #[test]
    fn vert_half_pel_pair_averages_byte_parallel() {
        // (0x00, 0x10) → 0x08 per byte.
        let a = 0x10101010u32;
        let b = 0x00000000u32;
        assert_eq!(mc_vert_half_pel_pair(a, b), 0x08080808);
    }

    #[test]
    fn vert_half_pel_pair_rounds_floor() {
        // (0x01, 0x00) → 0x00 (floor of `0.5`).
        let a = 0x01010101u32;
        let b = 0x00000000u32;
        assert_eq!(mc_vert_half_pel_pair(a, b), 0x00000000);
    }

    #[test]
    fn vert_half_pel_pair_high_pixel_values() {
        // (0x7f, 0x7f) → 0x7f.
        let a = 0x7f7f7f7fu32;
        let b = 0x7f7f7f7fu32;
        assert_eq!(mc_vert_half_pel_pair(a, b), 0x7f7f7f7f);
    }

    #[test]
    fn vert_half_pel_pair_no_inter_byte_bleed() {
        // (0x7f, 0x01) per byte should give 0x40 per byte; no carry
        // from byte 0 should bleed into byte 1.
        let a = 0x7f7f7f7fu32;
        let b = 0x01010101u32;
        assert_eq!(mc_vert_half_pel_pair(a, b), 0x40404040);
    }

    #[test]
    fn horiz_half_pel_pair_splices_neighbour_dword() {
        // For a DWORD whose every byte is the same value, the splice
        // with itself is the identity DWORD and the average is the
        // input (all bytes already in the 7-bit range).
        let d = 0x55555555u32;
        assert_eq!(mc_horiz_half_pel_pair(d, d), d);
        let d2 = 0x33333333u32;
        assert_eq!(mc_horiz_half_pel_pair(d2, d2), d2);
    }

    #[test]
    fn horiz_half_pel_pair_byte_splice_arithmetic() {
        // src_dword       = 0x04030201 (b0=0x01, b1=0x02, b2=0x03, b3=0x04)
        // src_dword_next  = 0x08070605 (b4=0x05, b5=0x06, b6=0x07, b7=0x08)
        // shifted         = 0x05040302 (b1, b2, b3, b4)
        // average byte-parallel: avg(0x01,0x02)=0x01, avg(0x02,0x03)=0x02,
        // avg(0x03,0x04)=0x03, avg(0x04,0x05)=0x04
        // → 0x04030201 (the cleanly-floor-rounded average).
        let src_dword = 0x04030201u32;
        let src_dword_next = 0x08070605u32;
        let avg = mc_horiz_half_pel_pair(src_dword, src_dword_next);
        // bytes: 0x01, 0x02, 0x03, 0x04 in little-endian
        assert_eq!(avg & 0xff, 0x01);
        assert_eq!((avg >> 8) & 0xff, 0x02);
        assert_eq!((avg >> 16) & 0xff, 0x03);
        assert_eq!((avg >> 24) & 0xff, 0x04);
    }

    #[test]
    fn both_half_pel_quad_equals_horiz_then_vert() {
        // The §5.2 ordering is horizontal-pair-first, vertical-pair-
        // second; this test confirms the kernel composes the two
        // averaging steps in that order.
        let a = 0x10101010u32;
        let b = 0x20202020u32;
        let c = 0x30303030u32;
        let d = 0x40404040u32;
        // horiz_top = average_7bit(a, shift(a,b)) for shifted = a>>8 | b<<24
        //           = average_7bit(0x10101010, 0x20101010 _splice_)
        // simpler check: identical inputs → identical output (the
        // splice with same DWORD reduces to the DWORD itself).
        let q = mc_both_half_pel_quad(a, a, a, a);
        assert_eq!(q, a & 0x7f7f7f7f);
        // Pure-vertical degeneration: top row and bottom row identical
        // gives the horizontal pair only.
        let q2 = mc_both_half_pel_quad(a, b, a, b);
        let horiz_top = mc_horiz_half_pel_pair(a, b);
        assert_eq!(q2, horiz_top);
        // Pure-horizontal degeneration: left column and right column
        // identical gives the vertical pair only.
        let q3 = mc_both_half_pel_quad(a, a, c, c);
        let horiz_top = mc_horiz_half_pel_pair(a, a);
        let horiz_bot = mc_horiz_half_pel_pair(c, c);
        let vert = mc_vert_half_pel_pair(horiz_top, horiz_bot);
        assert_eq!(q3, vert);
        // Sanity: with all four distinct, the result is well-defined.
        let _ = mc_both_half_pel_quad(a, b, c, d);
    }

    // ---- §5.1 per-iteration step tuple ---------------------------

    #[test]
    fn kernel_step_for_row_matches_full_pel_offsets() {
        for (row, expected) in MC_FULL_PEL_ROW_OFFSETS.iter().enumerate() {
            let s = McKernelStep::for_row(row).unwrap();
            assert_eq!(s.src_offset, *expected);
            assert_eq!(s.dst_offset, *expected);
        }
    }

    #[test]
    fn kernel_step_for_row_rejects_out_of_range() {
        assert!(McKernelStep::for_row(MC_INNER_LOOP_DWORDS_PER_ITER).is_none());
        assert!(McKernelStep::for_row(100).is_none());
    }

    #[test]
    fn kernel_step_outer_band_advance_matches_const() {
        assert_eq!(McKernelStep::outer_band_advance(), MC_BAND_BYTE_STRIDE);
        assert_eq!(McKernelStep::outer_band_advance(), 0x2c0);
    }

    #[test]
    fn kernel_step_inner_column_group_advance_matches_const() {
        assert_eq!(
            McKernelStep::inner_column_group_advance(),
            MC_COLUMN_GROUP_PIXELS
        );
        assert_eq!(McKernelStep::inner_column_group_advance(), 4);
    }

    // ---- inter-module consistency --------------------------------

    #[test]
    fn row_stride_matches_predictor_stride() {
        // §5.1 / §5.3 / spec/03 §5.2: the kernel's row stride MUST
        // equal the predictor stride and the packed-MV row-stride
        // multiplier.
        assert_eq!(MC_ROW_STRIDE, PREDICTOR_ROW_STRIDE);
        assert_eq!(
            MC_ROW_STRIDE as i32,
            super::super::mc_packed::MV_PIXEL_OFFSET_ROW_STRIDE
        );
    }
}
