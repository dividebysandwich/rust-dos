//! Indeo 3 spec/05 §7.3 — reverse decomposition of the cell's
//! `(x, y, w, h)` from the cell-state dispatcher's outputs.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/05-motion-compensation.md`
//! §7.3 (the four-quantity "the (x, y, w, h) of the cell, decoded"
//! recipe at the end of the §7 position-stack decode chapter), with
//! cross-references to §7.2 (the four codebook-bank sub-tables —
//! `bank[+0x000]`, `+0x100`, `+0x200`, `+0x300`, `+0x700` — that
//! mediate the forward `(edi, esi, ch, cl) → byte_address` mapping),
//! §7.1 (the `(edi, esi, ch, cl)` path-encoding register state),
//! `spec/03 §5.2` (the strip-context slot's `[+0x00]` base pointer
//! and the `0xb0` row stride), and `spec/05 §3.3` (the `176`-byte
//! buffer row stride that aliases the §7.3 `cell_x = dst_addr mod
//! 0xb0` modulus).
//!
//! Round 15 ([`super::mc_address`]) closed the §5.4 / §7.2 forward
//! address-composition chain: cell-slot index → per-cell sub-array
//! load → `dst_addr` / `src_addr` byte offsets. Round 15's module
//! deliberately did **not** perform the §7.3 reverse mapping back
//! into pixel coordinates ("That decomposition is a reverse mapping
//! required only for external readers (e.g. a renderer wanting to
//! know where in the frame the cell lives); the decoder itself
//! treats `dst_addr` as an opaque byte address and writes through
//! it."). This module owns the §7.3 reverse decomposition as a
//! typed external-reader surface.
//!
//! Three reverse-mapping facets sit here:
//!
//! 1. **Cell shape from the codebook-bank LUTs.** Per §7.3, the
//!    cell's pixel width and height are direct functions of the
//!    `(ch, cl)` cell-state-byte registers via the `bank[+0x000]`
//!    column-group LUT and the `ch >> 24` row-band-count extraction:
//!
//!    ```text
//!    cell_w = cl_inner * 4    ; cl_inner = bank[+0x000][cl]
//!    cell_h = row_band_count * 4
//!    ```
//!
//!    The `* 4` factors are §5.1's [`super::MC_COLUMN_GROUP_PIXELS`]
//!    (the four pixels per column-group inner-loop iteration) and
//!    [`super::MC_BAND_ROWS`] (the four rows per row-band band-
//!    stride).
//!
//! 2. **Cell top-left coordinates from `dst_addr`.** Per §7.3, the
//!    cell's `(cell_x, cell_y)` top-left pixel coordinates are
//!    recovered from the destination byte address via the strip
//!    pixel-buffer's row stride [`super::MC_ROW_STRIDE`] = `0xb0`:
//!
//!    ```text
//!    cell_x = dst_addr mod 0xb0
//!    cell_y = (dst_addr - strip_base) / 0xb0
//!    ```
//!
//!    where `strip_base` is the strip-context slot's `[+0x00]` base
//!    pointer the per-plane decoder hands in via the §5 / §6 plumbing.
//!
//! 3. **The (w, h, x, y) tuple as a typed shape descriptor.** The
//!    four quantities together describe a rectangular region of the
//!    strip's pixel buffer. The §7.3 paragraph names this a "cell";
//!    this module surfaces a typed [`CellRect`] for it, with
//!    width / height constrained to `[1, MAX]` where `MAX` is set
//!    by the §5.1 `MC_MAX_CELL_WIDTH_BYTES` / §5.3 row-band stride.
//!
//! This module surfaces:
//!
//! * [`CELL_PIXELS_PER_COLUMN_GROUP`] = `4` — the §7.3 `cl_inner * 4`
//!   factor, aliasing the §5.1 [`super::MC_COLUMN_GROUP_PIXELS`]
//!   surface with a `const _` cross-check.
//! * [`CELL_PIXELS_PER_ROW_BAND`] = `4` — the §7.3 `row_band_count *
//!   4` factor, aliasing the §5.1 [`super::MC_BAND_ROWS`] surface
//!   with a `const _` cross-check.
//! * [`cell_width_from_column_group_count`] — the §7.3
//!   `cl_inner → cell_w = cl_inner * 4` mapping, returning `None` on
//!   a `cl_inner` of zero (degenerate empty cell) or on overflow of
//!   the `u32` product (defensive; the §5.3 row-stride bound
//!   ([`super::MC_MAX_CELL_WIDTH_BYTES`] = `0xb0` = `176`) limits
//!   `cl_inner` to at most 44, well within `u32`).
//! * [`cell_height_from_row_band_count`] — the §7.3
//!   `row_band_count → cell_h = row_band_count * 4` mapping with the
//!   same defensive overflow check.
//! * [`row_band_count_from_ch_register`] — the §7.3 "extracted via
//!   `ecx >> 24`" derivation: the initial `ch` value (the
//!   accumulated V_SPLIT cell-state byte from §7.1) has its
//!   row-band-count carried in its upper byte; this helper applies
//!   the `>> 24` shift on a 32-bit register value.
//! * [`CellCoords`] — a typed `(cell_x, cell_y)` pixel-coordinate
//!   pair (within the strip's pixel buffer, not the whole frame).
//! * [`cell_coords_from_dst_addr`] — the §7.3 modular decomposition
//!   `dst_addr → (cell_x, cell_y)` against
//!   [`super::MC_ROW_STRIDE`]. Returns `None` if `dst_addr <
//!   strip_base` (a contract violation by the caller; the binary's
//!   forward arithmetic always produces `dst_addr >= strip_base`).
//! * [`CellRect`] — the full `(cell_x, cell_y, cell_w, cell_h)`
//!   shape descriptor, constructed via [`CellRect::from_parts`] from
//!   the four sub-pieces.
//! * [`CellRectDecodeError`] — the typed failure surface for the
//!   reverse decomposition (zero column-group count, zero row-band
//!   count, dst-address-below-strip-base, integer overflow of the
//!   pixel-product arithmetic).
//! * [`reverse_decompose`] — the single-call §7.3 reverse mapping
//!   `(dst_addr, strip_base, cl_inner, row_band_count) → CellRect`
//!   composing the three sub-facets in one entry point.
//!
//! What this module **deliberately does not do** (the §7.3 chapter
//! boundary):
//!
//! * It does not own the per-entry values of `bank[+0x000][cl]`
//!   (the `cl_inner` column-group LUT) — those table values are
//!   §7.5 Extractor territory ("populated by the codec-init routine
//!   at `IR32_32.DLL!0x10006308` from the seed table at `.data +
//!   0x1003ed4c..+0x1003ee4d`"). This module accepts a
//!   pre-resolved `cl_inner` byte as input.
//! * It does not bridge the `(cell_x, cell_y)` strip-pixel-buffer
//!   coordinates into whole-frame coordinates. The strip-to-frame
//!   assembly is `spec/07 §5.7` territory; an external reader that
//!   wants whole-frame coordinates composes `frame_y = strip_y_in_frame
//!   + cell_y` itself at the strip-context layer.
//! * It does not validate that the `(cell_x, cell_y, cell_w,
//!   cell_h)` rectangle fits within the strip's visible width
//!   ([`super::LUMA_STRIP_WIDTH`] = `160` or
//!   [`super::CHROMA_STRIP_WIDTH`] = `40`). The strip pixel buffer
//!   has the same allocated row stride [`super::MC_ROW_STRIDE`] =
//!   `0xb0` regardless of plane role per §5.5; visible-width
//!   classification is a plane-role question this module leaves to
//!   [`super::McPlaneRole::strip_visible_width`].
//! * It does not reverse-engineer the codebook-bank `+0x300` / `+0x700`
//!   sub-table values from a `dst_addr` observation. The forward
//!   `(ch, cl) → (cell_x, cell_y)` mapping is table-mediated (§7.4);
//!   the §7.3 reverse mapping uses arithmetic against the strip's
//!   `0xb0` row stride and the per-strip base pointer, both of
//!   which are decoder-side state independent of the codebook
//!   tables.
//!
//! All identities, immediates, and arithmetic come from
//! `05-motion-compensation.md` §7.3 with the cross-references named
//! above.

use super::{MC_BAND_ROWS, MC_COLUMN_GROUP_PIXELS, MC_ROW_STRIDE};

// ---- §7.3 (cell-width / cell-height factors) ------------------------

/// Spec/05 §7.3 — the `cl_inner * 4` factor for cell pixel width.
///
/// Per §7.3 "cell_w = cl_inner * 4" with `cl_inner = bank[+0x000][cl]`
/// (the codebook-bank column-group LUT). The `4` is §5.1's per-
/// column-group pixel count, aliased here so the §7.3 reverse-
/// decomposition surface does not have to reach into the §5.1
/// inner-loop kernel constants directly.
pub const CELL_PIXELS_PER_COLUMN_GROUP: u32 = MC_COLUMN_GROUP_PIXELS as u32;

/// `const _` cross-check: the §7.3 column-group factor equals the
/// §5.1 [`super::MC_COLUMN_GROUP_PIXELS`] surface.
const _: () = assert!(CELL_PIXELS_PER_COLUMN_GROUP == MC_COLUMN_GROUP_PIXELS as u32);

/// Spec/05 §7.3 — the `row_band_count * 4` factor for cell pixel
/// height.
///
/// Per §7.3 "cell_h = 4 * row_band_count". The `4` is §5.1's per-
/// row-band row count, aliased here so the §7.3 reverse-decomposition
/// surface does not have to reach into the §5.1 inner-loop kernel
/// constants directly.
pub const CELL_PIXELS_PER_ROW_BAND: u32 = MC_BAND_ROWS as u32;

/// `const _` cross-check: the §7.3 row-band factor equals the §5.1
/// [`super::MC_BAND_ROWS`] surface.
const _: () = assert!(CELL_PIXELS_PER_ROW_BAND == MC_BAND_ROWS as u32);

// ---- §7.3 (cell-width / cell-height reverse helpers) ----------------

/// Spec/05 §7.3 — `cl_inner → cell_w = cl_inner * 4`.
///
/// Returns `None` on a `cl_inner` of zero (degenerate empty cell;
/// the §7.4 codebook-bank populates `bank[+0x000][cl]` with at
/// least 1 for any valid `(ch, cl)` combination per the §2.4
/// minimum-cell-size constraint from `spec/03 §2.4`) or on overflow
/// of the `u32` product.
pub const fn cell_width_from_column_group_count(cl_inner: u8) -> Option<u32> {
    if cl_inner == 0 {
        return None;
    }
    (cl_inner as u32).checked_mul(CELL_PIXELS_PER_COLUMN_GROUP)
}

/// Spec/05 §7.3 — `row_band_count → cell_h = row_band_count * 4`.
///
/// Returns `None` on a zero `row_band_count` (degenerate empty
/// cell; same §2.4 minimum-cell-size disposition) or on overflow
/// of the `u32` product.
pub const fn cell_height_from_row_band_count(row_band_count: u8) -> Option<u32> {
    if row_band_count == 0 {
        return None;
    }
    (row_band_count as u32).checked_mul(CELL_PIXELS_PER_ROW_BAND)
}

/// Spec/05 §7.3 — extract the row-band count from the initial `ch`
/// register value.
///
/// Per §7.3 the row-band count is "the initial value of `ch`'s upper
/// byte (extracted via `ecx >> 24` after the `sub ecx, 0x1000000`
/// decrement loop terminates)". This helper applies the `>> 24`
/// shift to a 32-bit register snapshot and returns the upper byte
/// as a `u8`.
pub const fn row_band_count_from_ch_register(ch_register: u32) -> u8 {
    (ch_register >> 24) as u8
}

// ---- §7.3 (cell-coordinate reverse decomposition) -------------------

/// Spec/05 §7.3 — typed `(cell_x, cell_y)` pixel-coordinate pair
/// within the strip's pixel buffer.
///
/// `cell_x` is in `[0, MC_ROW_STRIDE - 1]` (the §5.5 / §5.1 strip
/// row stride is the allocated `0xb0` regardless of plane role);
/// `cell_y` is in `[0, strip_height - 1]` where `strip_height` is
/// the §4.1 picture-decomposition strip-height the per-plane
/// decoder computes.
///
/// Coordinates are relative to the strip's pixel buffer (i.e. row
/// 0 column 0 of the strip), not the whole frame. The strip-to-
/// frame assembly is `spec/07 §5.7` territory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellCoords {
    /// The cell top-left's column within the strip pixel buffer,
    /// in pixels (= `dst_addr mod 0xb0` per §7.3).
    pub cell_x: u32,
    /// The cell top-left's row within the strip pixel buffer, in
    /// pixels (= `(dst_addr - strip_base) / 0xb0` per §7.3).
    pub cell_y: u32,
}

/// Spec/05 §7.3 — modular reverse decomposition
/// `dst_addr → (cell_x, cell_y)` against [`super::MC_ROW_STRIDE`]
/// = `0xb0`.
///
/// Per §7.3 the destination byte address recovered by the forward
/// composition chain ([`super::McCellAddressPair::resolve`])
/// satisfies `dst_addr = strip_base + cell_y * 0xb0 + cell_x` with
/// `cell_x` in `[0, 0xb0)`. The reverse mapping is therefore a
/// single division-with-remainder of `(dst_addr - strip_base)` by
/// `MC_ROW_STRIDE`.
///
/// Returns `None` if `dst_addr < strip_base` (a caller-side
/// contract violation; the binary's forward arithmetic always
/// produces `dst_addr >= strip_base` because the `dst_cell_data`
/// DWORD loaded from the strip-context per-cell sub-array is the
/// strip's base byte pointer plus the per-cell offset).
pub const fn cell_coords_from_dst_addr(dst_addr: usize, strip_base: usize) -> Option<CellCoords> {
    let Some(offset) = dst_addr.checked_sub(strip_base) else {
        return None;
    };
    let stride = MC_ROW_STRIDE;
    let cell_y = (offset / stride) as u32;
    let cell_x = (offset % stride) as u32;
    Some(CellCoords { cell_x, cell_y })
}

// ---- §7.3 (full (x, y, w, h) rectangle) -----------------------------

/// Spec/05 §7.3 — the full `(cell_x, cell_y, cell_w, cell_h)`
/// shape descriptor recovered from the §7.2 forward outputs.
///
/// The four quantities together describe a rectangular region of
/// the strip's pixel buffer that the MC fetcher writes to (the
/// destination) or reads from (the source, with the §2.3 packed-MV
/// displacement applied). Per §7.3, the (w, h) decode is *direct*
/// from `(ch, cl)`; the (x, y) decode is *table-mediated* via the
/// codebook-bank sub-tables (§7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRect {
    /// The cell's top-left pixel coordinates within the strip's
    /// pixel buffer.
    pub coords: CellCoords,
    /// The cell's pixel width (`cl_inner * 4` per §7.3).
    pub width: u32,
    /// The cell's pixel height (`row_band_count * 4` per §7.3).
    pub height: u32,
}

impl CellRect {
    /// Spec/05 §7.3 — assemble a [`CellRect`] from its four parts:
    /// the cell coordinates, the column-group count `cl_inner`, and
    /// the row-band count.
    ///
    /// Returns [`CellRectDecodeError::ZeroColumnGroupCount`] /
    /// [`CellRectDecodeError::ZeroRowBandCount`] on a degenerate
    /// zero in either factor (the §2.4 minimum-cell-size constraint
    /// forbids a zero-cell encoding) and
    /// [`CellRectDecodeError::DimensionOverflow`] on `u32` overflow
    /// of the pixel-product arithmetic.
    pub const fn from_parts(
        coords: CellCoords,
        cl_inner: u8,
        row_band_count: u8,
    ) -> Result<Self, CellRectDecodeError> {
        let Some(width) = cell_width_from_column_group_count(cl_inner) else {
            if cl_inner == 0 {
                return Err(CellRectDecodeError::ZeroColumnGroupCount);
            }
            return Err(CellRectDecodeError::DimensionOverflow);
        };
        let Some(height) = cell_height_from_row_band_count(row_band_count) else {
            if row_band_count == 0 {
                return Err(CellRectDecodeError::ZeroRowBandCount);
            }
            return Err(CellRectDecodeError::DimensionOverflow);
        };
        Ok(Self {
            coords,
            width,
            height,
        })
    }
}

/// Spec/05 §7.3 — typed failure surface for the reverse
/// decomposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CellRectDecodeError {
    /// `dst_addr < strip_base` — caller passed an address before
    /// the strip's base pointer; the binary's forward arithmetic
    /// never produces this and the reverse mapping has no
    /// meaningful answer.
    DestAddressBelowStripBase,
    /// `cl_inner == 0` — the column-group LUT byte was zero, which
    /// the §2.4 minimum-cell-size constraint forbids in a valid
    /// encoding.
    ZeroColumnGroupCount,
    /// `row_band_count == 0` — the upper byte of the initial `ch`
    /// register was zero, which the §2.4 minimum-cell-size
    /// constraint forbids in a valid encoding.
    ZeroRowBandCount,
    /// `cl_inner * 4` or `row_band_count * 4` overflowed `u32`.
    /// Defensive; the §5.3 row-stride bound limits `cl_inner` to
    /// 44 in any valid encoding.
    DimensionOverflow,
}

/// Spec/05 §7.3 — run the complete reverse decomposition in one
/// entry point.
///
/// Inputs:
///
/// * `dst_addr` — the destination byte address recovered by
///   [`super::McCellAddressPair::resolve`].
/// * `strip_base` — the strip-context slot's `[+0x00]` base byte
///   pointer (per `spec/03 §5.2`).
/// * `cl_inner` — `bank[+0x000][cl]`, the per-`cl` column-group
///   count from the codebook-bank LUT (§7.5 Extractor territory;
///   passed as a pre-resolved byte).
/// * `row_band_count` — the initial `ch` register's upper byte (§7.1
///   path-encoding state, recoverable via
///   [`row_band_count_from_ch_register`]).
///
/// Returns the [`CellRect`] on success.
pub fn reverse_decompose(
    dst_addr: usize,
    strip_base: usize,
    cl_inner: u8,
    row_band_count: u8,
) -> Result<CellRect, CellRectDecodeError> {
    let coords = cell_coords_from_dst_addr(dst_addr, strip_base)
        .ok_or(CellRectDecodeError::DestAddressBelowStripBase)?;
    CellRect::from_parts(coords, cl_inner, row_band_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::{MC_BAND_ROWS, MC_COLUMN_GROUP_PIXELS, MC_ROW_STRIDE};

    // ---- §7.3 (factor constants) -----------------------------------

    #[test]
    fn column_group_factor_is_four() {
        // §7.3: cell_w = cl_inner * 4. The 4 is §5.1's
        // MC_COLUMN_GROUP_PIXELS.
        assert_eq!(CELL_PIXELS_PER_COLUMN_GROUP, 4);
        assert_eq!(CELL_PIXELS_PER_COLUMN_GROUP, MC_COLUMN_GROUP_PIXELS as u32);
    }

    #[test]
    fn row_band_factor_is_four() {
        // §7.3: cell_h = row_band_count * 4. The 4 is §5.1's
        // MC_BAND_ROWS.
        assert_eq!(CELL_PIXELS_PER_ROW_BAND, 4);
        assert_eq!(CELL_PIXELS_PER_ROW_BAND, MC_BAND_ROWS as u32);
    }

    // ---- §7.3 (cell_width_from_column_group_count) -----------------

    #[test]
    fn cell_width_one_column_group() {
        // cl_inner = 1 ⇒ 4-pixel-wide cell.
        assert_eq!(cell_width_from_column_group_count(1), Some(4));
    }

    #[test]
    fn cell_width_full_strip_width() {
        // §5.3 row-stride bound MC_MAX_CELL_WIDTH_BYTES = 0xb0;
        // 0xb0 / 4 = 44 column groups ⇒ 176-pixel-wide cell.
        assert_eq!(cell_width_from_column_group_count(44), Some(176));
    }

    #[test]
    fn cell_width_typical_chroma_cell() {
        // Chroma strips at full strip width are 40 pixels =
        // 10 column groups (§5.5 4:1 subsampling).
        assert_eq!(cell_width_from_column_group_count(10), Some(40));
    }

    #[test]
    fn cell_width_zero_column_group_rejected() {
        // §2.4: minimum cell size is non-zero in both dimensions.
        assert_eq!(cell_width_from_column_group_count(0), None);
    }

    #[test]
    fn cell_width_max_byte_does_not_overflow() {
        // 0xff * 4 = 1020, well within u32.
        assert_eq!(cell_width_from_column_group_count(0xff), Some(0xff * 4));
    }

    // ---- §7.3 (cell_height_from_row_band_count) --------------------

    #[test]
    fn cell_height_one_row_band() {
        // row_band_count = 1 ⇒ 4-pixel-tall cell.
        assert_eq!(cell_height_from_row_band_count(1), Some(4));
    }

    #[test]
    fn cell_height_typical_strip_height() {
        // Indeo 3 strips are 40-row at the picture-decomposition
        // table per spec/02 §4.1 ⇒ 10 row bands at 4 rows each.
        assert_eq!(cell_height_from_row_band_count(10), Some(40));
    }

    #[test]
    fn cell_height_zero_row_band_rejected() {
        assert_eq!(cell_height_from_row_band_count(0), None);
    }

    #[test]
    fn cell_height_max_byte_does_not_overflow() {
        assert_eq!(cell_height_from_row_band_count(0xff), Some(0xff * 4));
    }

    // ---- §7.3 (row_band_count_from_ch_register) --------------------

    #[test]
    fn row_band_count_extracts_upper_byte_of_ch_register() {
        // §7.3 / §7.1: row band count = ch register >> 24.
        // ch = 0x0A_00_00_00 ⇒ row band count = 0x0A = 10.
        assert_eq!(row_band_count_from_ch_register(0x0A_00_00_00), 10);
    }

    #[test]
    fn row_band_count_ignores_lower_bytes() {
        // Lower three bytes of ch carry V_SPLIT accumulation state
        // (§7.1's `add ch, ch`); the upper byte alone is the row-
        // band count per §7.3.
        assert_eq!(row_band_count_from_ch_register(0x0A_FF_FF_FF), 10);
    }

    #[test]
    fn row_band_count_zero_for_zero_ch() {
        assert_eq!(row_band_count_from_ch_register(0), 0);
    }

    #[test]
    fn row_band_count_max_byte_extraction() {
        assert_eq!(row_band_count_from_ch_register(0xFF_00_00_00), 0xFF);
    }

    // ---- §7.3 (cell_coords_from_dst_addr) --------------------------

    #[test]
    fn coords_at_strip_base_is_zero_zero() {
        // dst_addr == strip_base ⇒ (0, 0).
        let c = cell_coords_from_dst_addr(0x10_0000, 0x10_0000).unwrap();
        assert_eq!(c.cell_x, 0);
        assert_eq!(c.cell_y, 0);
    }

    #[test]
    fn coords_one_row_below_base() {
        // dst_addr = strip_base + MC_ROW_STRIDE ⇒ (0, 1).
        let c = cell_coords_from_dst_addr(0x10_0000 + MC_ROW_STRIDE, 0x10_0000).unwrap();
        assert_eq!(c.cell_x, 0);
        assert_eq!(c.cell_y, 1);
    }

    #[test]
    fn coords_within_first_row() {
        // dst_addr = strip_base + 4 ⇒ (4, 0).
        let c = cell_coords_from_dst_addr(0x10_0000 + 4, 0x10_0000).unwrap();
        assert_eq!(c.cell_x, 4);
        assert_eq!(c.cell_y, 0);
    }

    #[test]
    fn coords_arbitrary_strip_position() {
        // dst_addr = strip_base + 7 * MC_ROW_STRIDE + 16
        // ⇒ (16, 7).
        let strip_base = 0x20_0000;
        let dst_addr = strip_base + 7 * MC_ROW_STRIDE + 16;
        let c = cell_coords_from_dst_addr(dst_addr, strip_base).unwrap();
        assert_eq!(c.cell_x, 16);
        assert_eq!(c.cell_y, 7);
    }

    #[test]
    fn coords_at_last_column_of_strip_row() {
        // dst_addr = strip_base + MC_ROW_STRIDE - 1
        // ⇒ (MC_ROW_STRIDE - 1, 0) = (175, 0).
        let strip_base = 0x30_0000;
        let dst_addr = strip_base + MC_ROW_STRIDE - 1;
        let c = cell_coords_from_dst_addr(dst_addr, strip_base).unwrap();
        assert_eq!(c.cell_x, (MC_ROW_STRIDE - 1) as u32);
        assert_eq!(c.cell_x, 175);
        assert_eq!(c.cell_y, 0);
    }

    #[test]
    fn coords_below_strip_base_returns_none() {
        // §7.3 caller-contract: dst_addr >= strip_base.
        assert_eq!(cell_coords_from_dst_addr(0x10_0000, 0x10_0001), None);
    }

    // ---- §7.3 (CellRect::from_parts) -------------------------------

    #[test]
    fn rect_assembles_full_strip_cell() {
        // Full-strip 176×40 cell at strip origin.
        let coords = CellCoords {
            cell_x: 0,
            cell_y: 0,
        };
        let rect = CellRect::from_parts(coords, 44, 10).unwrap();
        assert_eq!(rect.coords, coords);
        assert_eq!(rect.width, 176);
        assert_eq!(rect.height, 40);
    }

    #[test]
    fn rect_assembles_typical_intra_cell() {
        // Typical 16×16 INTRA cell (4 column groups × 4 row bands).
        let coords = CellCoords {
            cell_x: 32,
            cell_y: 8,
        };
        let rect = CellRect::from_parts(coords, 4, 4).unwrap();
        assert_eq!(rect.coords.cell_x, 32);
        assert_eq!(rect.coords.cell_y, 8);
        assert_eq!(rect.width, 16);
        assert_eq!(rect.height, 16);
    }

    #[test]
    fn rect_rejects_zero_column_group_count() {
        let coords = CellCoords {
            cell_x: 0,
            cell_y: 0,
        };
        assert_eq!(
            CellRect::from_parts(coords, 0, 4),
            Err(CellRectDecodeError::ZeroColumnGroupCount)
        );
    }

    #[test]
    fn rect_rejects_zero_row_band_count() {
        let coords = CellCoords {
            cell_x: 0,
            cell_y: 0,
        };
        assert_eq!(
            CellRect::from_parts(coords, 4, 0),
            Err(CellRectDecodeError::ZeroRowBandCount)
        );
    }

    // ---- §7.3 (reverse_decompose end-to-end) -----------------------

    #[test]
    fn reverse_decompose_full_chain_strip_origin() {
        // strip_base = 0x10_0000, dst_addr = strip_base ⇒ (0, 0);
        // cl_inner = 4, row_band_count = 4 ⇒ 16×16 cell at origin.
        let r = reverse_decompose(0x10_0000, 0x10_0000, 4, 4).unwrap();
        assert_eq!(r.coords.cell_x, 0);
        assert_eq!(r.coords.cell_y, 0);
        assert_eq!(r.width, 16);
        assert_eq!(r.height, 16);
    }

    #[test]
    fn reverse_decompose_arbitrary_position_and_size() {
        // 8×8 cell at strip column 8, row 4.
        let strip_base = 0x20_0000;
        let dst_addr = strip_base + 4 * MC_ROW_STRIDE + 8;
        let r = reverse_decompose(dst_addr, strip_base, 2, 2).unwrap();
        assert_eq!(r.coords.cell_x, 8);
        assert_eq!(r.coords.cell_y, 4);
        assert_eq!(r.width, 8);
        assert_eq!(r.height, 8);
    }

    #[test]
    fn reverse_decompose_propagates_below_base_error() {
        assert_eq!(
            reverse_decompose(0x10_0000, 0x10_0001, 4, 4),
            Err(CellRectDecodeError::DestAddressBelowStripBase)
        );
    }

    #[test]
    fn reverse_decompose_propagates_zero_column_group_error() {
        assert_eq!(
            reverse_decompose(0x10_0000, 0x10_0000, 0, 4),
            Err(CellRectDecodeError::ZeroColumnGroupCount)
        );
    }

    #[test]
    fn reverse_decompose_propagates_zero_row_band_error() {
        assert_eq!(
            reverse_decompose(0x10_0000, 0x10_0000, 4, 0),
            Err(CellRectDecodeError::ZeroRowBandCount)
        );
    }

    // ---- Cross-module identities -----------------------------------

    #[test]
    fn modulus_aligns_with_mc_row_stride() {
        // §7.3 / §5.5: the modulus used in cell_x recovery is the
        // strip-buffer row stride, not the visible width.
        let strip_base = 0x40_0000;
        let dst_addr = strip_base + MC_ROW_STRIDE - 1;
        let c = cell_coords_from_dst_addr(dst_addr, strip_base).unwrap();
        // cell_x = MC_ROW_STRIDE - 1 (still on the same row, not
        // wrapped to the next).
        assert_eq!(c.cell_x, (MC_ROW_STRIDE - 1) as u32);
        assert_eq!(c.cell_y, 0);
    }

    #[test]
    fn forward_reverse_round_trip_at_arbitrary_coords() {
        // Compose dst_addr from (cell_x, cell_y) and round-trip
        // back through the §7.3 reverse mapping.
        let strip_base = 0x50_0000_usize;
        let cell_x = 24;
        let cell_y = 9;
        let dst_addr = strip_base + (cell_y as usize) * MC_ROW_STRIDE + cell_x as usize;
        let c = cell_coords_from_dst_addr(dst_addr, strip_base).unwrap();
        assert_eq!(c.cell_x, cell_x);
        assert_eq!(c.cell_y, cell_y);
    }

    #[test]
    fn cell_width_consistent_with_column_group_pixels() {
        // The §7.3 cell_w factor IS the §5.1 column-group pixel
        // count; this test pins the alias.
        assert_eq!(
            cell_width_from_column_group_count(1).unwrap() as usize,
            MC_COLUMN_GROUP_PIXELS
        );
    }

    #[test]
    fn cell_height_consistent_with_band_rows() {
        // The §7.3 cell_h factor IS the §5.1 band-rows count.
        assert_eq!(
            cell_height_from_row_band_count(1).unwrap() as usize,
            MC_BAND_ROWS
        );
    }
}
