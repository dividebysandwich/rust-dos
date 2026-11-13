//! Indeo 3 spec/05 §5.1 / §5.2 / §7.2 + spec/03 §5.5 — the
//! motion-compensation cell-copy executor and the per-cell boundary
//! fix-up.
//!
//! Spec sources:
//!
//! * `docs/video/indeo/indeo3/spec/05-motion-compensation.md` §5.1
//!   (full-pel MC fetcher inner loop at
//!   `IR32_32.DLL!0x1000670d..0x1000673d`), §5.2 (the three half-pel
//!   inner-loop variants at `0x10006780` / `0x1000684b` /
//!   `0x100068f8`), §5.3 (destination and stride), §7.2 (the
//!   `[esp+0x34]` boundary-fix-up reduction
//!   `cell_offset = bank[+0x700][cl] sar 2 + extra_offset + ch`).
//! * `docs/video/indeo/indeo3/spec/03-macroblock-layer.md` §5.5 (the
//!   per-cell edge fix-up loop at `IR32_32.DLL!0x10006574..0x100065a3`).
//! * `docs/video/indeo/indeo3/spec/07-output-reconstruction.md` §1.2
//!   (the per-row `add [esp + 0x34], 0xb0` advance of the same
//!   scratch slot) and §1.3 (the fix-up's predictor-continuity role:
//!   it keeps the next cell's `[edi - 0xb0]` predictor reads valid
//!   across cell boundaries).
//!
//! Round 14 ([`super::mc_kernel`]) pinned the inner-loop *shape*
//! (row stride, band geometry, the three per-DWORD averaging
//! kernels) and round 15 ([`super::mc_address`]) resolved the §7.2
//! destination / source byte addresses — but deliberately deferred
//! both the §7.2 `[esp+0x34]` boundary-fix-up reduction and the
//! actual buffer-mutating copy. This module executes both stages
//! over a caller-supplied strip pixel buffer:
//!
//! * [`boundary_fixup_dst_cell_offset`] — the §7.2 reduction that
//!   produces the `[esp+0x34]` scratch value
//!   ([`BOUNDARY_FIXUP_SCRATCH_OFFSET`] = `0x34`,
//!   [`BOUNDARY_FIXUP_AUX_SHIFT`] = `2` for the `sar 2` of
//!   `bank[+0x700][cl]`), with [`advance_boundary_fixup_row`]
//!   running the spec/07 §1.2 per-row `+= 0xb0` advance
//!   ([`BOUNDARY_FIXUP_ROW_ADVANCE`]).
//! * [`mc_copy_cell`] — the §5.1 / §5.2 cell copy proper: walks the
//!   cell's 4-row bands and 4-pixel column groups in the inner-loop
//!   order (rows 0+1 read then written, rows 2+3 read then written,
//!   columns advancing within a band, bands advancing down) and
//!   applies the four-way [`McDispatchMode`] filter per DWORD via
//!   the round-14 averaging kernels.
//! * [`mc_copy_cell_mv`] — the same copy driven by a [`PackedMv`]:
//!   derives the §2.2 dispatch mode and the §2.3
//!   `src = dst + sar(packed_mv, 2)` source base in one step.
//! * [`apply_per_cell_edge_fixup`] — the spec/03 §5.5 inter-cell
//!   edge fix-up loop: per iteration it exchanges one DWORD across
//!   the cell boundary (previous cell's bottom-right edge at
//!   `[esi + 0x24]` into the next cell's `[edi - 4]`, the next
//!   cell's top edge at `[edi]` into the previous cell's
//!   `[esi + 0x28]`), advancing both pointers by the row stride
//!   `0xb0` and decrementing the height counter by 4 per iteration
//!   (do-while, `edx -= 4; while (edx > 0)`).
//!
//! What this module **deliberately does not do** (chapter
//! boundaries):
//!
//! * It does not own the `bank[+0x000]` / `+0x100` / `+0x200` /
//!   `+0x300` / `+0x700` codebook-bank LUT values — those per-entry
//!   values are §7.5 Extractor territory; callers pass the resolved
//!   `cell_pos_aux` / `extra_offset` / geometry inputs.
//! * It does not range-check the MV against the strip region the
//!   way the binary doesn't (§4.4 — "no explicit boundary check");
//!   the safe-Rust bound applied here is the *buffer* bound, exactly
//!   the arena-edge guard [`super::mc_kernel`]'s chapter notes
//!   assign to the caller side. The strip-region classification
//!   stays with [`super::mv_source_offset_in_strip_region`].
//! * It does not apply the §5.6 VQ residual over the just-written
//!   prediction — that is the spec/06 per-byte unpacker entry at
//!   `IR32_32.DLL!0x10006bac` ([`super::McToVqHandoff`] territory).
//! * It does not run the spec/03 §5.4 end-of-strip fix-up (owned by
//!   [`super::strip_edge`]); §5.5's inter-cell loop here is the
//!   *other* branch of the [`super::CellStackTopDispatch`] fork.
//!
//! All offsets, iteration orders, and counter semantics are taken
//! from `05-motion-compensation.md` §5 / §7.2, `03-macroblock-layer.md`
//! §5.5, and `07-output-reconstruction.md` §1.2 / §1.3. RVAs cited
//! in doc-comments refer to the binary identified in `spec/00 §2`.

use super::cell_subarray::{
    PER_CELL_EDGE_HEIGHT_STEP, PER_CELL_EDGE_PREV_BR_NEXT_OFFSET, PER_CELL_EDGE_PREV_BR_OFFSET,
    PER_CELL_EDGE_ROW_STRIDE,
};
use super::mc_kernel::{
    MC_BAND_ROWS, MC_BYTES_PER_DWORD, MC_ROW_STRIDE, McKernelGeometry, mc_both_half_pel_quad,
    mc_horiz_half_pel_pair, mc_vert_half_pel_pair,
};
use super::mc_packed::{McDispatchMode, PackedMv};

// ---- §7.2 boundary-fix-up reduction ---------------------------------

/// Spec/05 §7.2 — the dispatcher-scratch byte offset of the
/// boundary-fix-up cell offset (`[esp+0x34]`).
///
/// The §7.2 chain stores
/// `cell_offset = bank[+0x700][cl] sar 2 + extra_offset + ch` into
/// this slot ("→ `dst_cell_offset = [esp+0x34]` for boundary-fix-up
/// use"); the spec/07 §1.2 per-row outer-loop tail then advances the
/// same slot by the row stride (`add [esp + 0x34], 0xb0` at
/// `IR32_32.DLL!0x10006fc0..0x10006fdb`). Distinct from the three
/// [`super::DispatcherScratch`] cell-data slots (`0x24` / `0x28` /
/// `0x38`).
pub const BOUNDARY_FIXUP_SCRATCH_OFFSET: usize = 0x34;

/// Spec/05 §7.2 — the arithmetic right-shift applied to the
/// `bank[+0x700][cl]` cell-position aux DWORD inside the
/// boundary-fix-up reduction (`sar 2`).
///
/// The shift mirrors the packed-MV pixel-offset recovery
/// ([`super::MV_PIXEL_OFFSET_SHIFT`]): both quantities carry a
/// byte offset scaled by 4 in their stored form.
pub const BOUNDARY_FIXUP_AUX_SHIFT: u32 = 2;

/// Spec/07 §1.2 — the per-row advance added to the `[esp+0x34]`
/// boundary-fix-up offset by the outer-loop tail
/// (`add [esp + 0x34], 0xb0`). Aliases the strip pixel-buffer row
/// stride [`MC_ROW_STRIDE`].
pub const BOUNDARY_FIXUP_ROW_ADVANCE: usize = MC_ROW_STRIDE;

/// Spec/05 §7.2 — the boundary-fix-up reduction that produces the
/// `[esp+0x34]` scratch value:
///
/// ```text
/// cell_offset = bank[+0x700][cl] sar 2 + extra_offset + ch
/// ```
///
/// * `cell_pos_aux` — the `bank[+0x700][cl]` cell-position aux DWORD
///   (signed; the `sar` is arithmetic per §3.2's "the `sar` is
///   arithmetic, not logical" disposition for the sibling packed-MV
///   shift).
/// * `extra_offset` — the §7.2 `strip_ctx_arr[idx_src + 1]`
///   companion DWORD (the `[esp+0x38]` scratch,
///   [`super::CellAddrEntry::extra_offset`]).
/// * `ch` — the cell-state byte at dispatcher entry (§7.1).
///
/// The result is widened to `i64` so no input combination can wrap;
/// the binary performs the same sum in 32-bit registers but the
/// encoder-produced operands stay far below the wrap point (§7.4's
/// `0xf423f` position sentinel bounds the table side).
pub const fn boundary_fixup_dst_cell_offset(cell_pos_aux: i32, extra_offset: u32, ch: u8) -> i64 {
    (cell_pos_aux >> BOUNDARY_FIXUP_AUX_SHIFT) as i64 + extra_offset as i64 + ch as i64
}

/// Spec/07 §1.2 — advance the `[esp+0x34]` boundary-fix-up offset by
/// one row (`add [esp + 0x34], 0xb0` in the per-row outer-loop tail
/// at `IR32_32.DLL!0x10006fc0..0x10006fdb`).
pub const fn advance_boundary_fixup_row(dst_cell_offset: i64) -> i64 {
    dst_cell_offset + BOUNDARY_FIXUP_ROW_ADVANCE as i64
}

// ---- §5.1 / §5.2 cell-copy executor ---------------------------------

/// Spec/05 §5.1 / §5.2 — failure modes of the safe-Rust buffer-bound
/// check [`mc_copy_cell`] applies at the arena edge (the binary
/// itself performs no such check per §4.4; the strip allocation's
/// padding absorbs in-bounds-by-construction encoder output).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McCopyError {
    /// The destination cell's last row would extend past the end of
    /// the supplied buffer.
    DstOutOfBounds {
        /// Minimum buffer length the destination walk requires.
        required: usize,
        /// Supplied buffer length.
        supplied: usize,
    },
    /// The source cell's reads (including the §5.2 half-pel
    /// neighbour row / column) would extend past the end of the
    /// supplied buffer.
    SrcOutOfBounds {
        /// Minimum buffer length the source walk requires.
        required: usize,
        /// Supplied buffer length.
        supplied: usize,
    },
    /// The packed-MV displacement would move the source base below
    /// the start of the buffer (the §2.3 `dst + sar(packed_mv, 2)`
    /// went negative).
    MvUnderflow {
        /// Destination cell base the MV was applied to.
        dst_cell_base: usize,
        /// The §2.3 signed pixel offset recovered from the MV.
        pixel_offset: i32,
    },
}

impl core::fmt::Display for McCopyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            McCopyError::DstOutOfBounds { required, supplied } => write!(
                f,
                "spec/05 §5.1 / §5.3: destination cell walk requires {required} byte(s); \
                 buffer has {supplied}"
            ),
            McCopyError::SrcOutOfBounds { required, supplied } => write!(
                f,
                "spec/05 §5.1 / §5.2 / §4.4: source cell walk requires {required} byte(s); \
                 buffer has {supplied}"
            ),
            McCopyError::MvUnderflow {
                dst_cell_base,
                pixel_offset,
            } => write!(
                f,
                "spec/05 §2.3: MV pixel offset {pixel_offset} moves the source base below \
                 the buffer start (dst cell base {dst_cell_base})"
            ),
        }
    }
}

impl std::error::Error for McCopyError {}

/// Read one little-endian DWORD (§5.1's `mov edx, [esi + …]`).
/// Callers pre-validate bounds; the slice index cannot fail.
fn read_dword(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

/// Write one little-endian DWORD (§5.1's `mov [edi + …], edx`).
fn write_dword(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + MC_BYTES_PER_DWORD].copy_from_slice(&v.to_le_bytes());
}

/// Spec/05 §5.1 / §5.2 — produce one output DWORD from the source
/// base offset `off`, applying the four-way dispatch-mode filter:
///
/// * [`McDispatchMode::FullPel`] — straight DWORD read (§5.1).
/// * [`McDispatchMode::VerticalHalfPel`] — average with the DWORD
///   one row below (`[esi + 0xb0]`, §5.2 `01` path).
/// * [`McDispatchMode::HorizontalHalfPel`] — average with the
///   one-byte-right neighbour, formed by splicing the next DWORD
///   (`[esi + 0x4]`, §5.2 `10` path).
/// * [`McDispatchMode::BothHalfPel`] — 2×2 unweighted box filter
///   over the four neighbours (§5.2 `11` path).
fn fetch_source_dword(buf: &[u8], mode: McDispatchMode, off: usize) -> u32 {
    match mode {
        McDispatchMode::FullPel => read_dword(buf, off),
        McDispatchMode::VerticalHalfPel => {
            mc_vert_half_pel_pair(read_dword(buf, off), read_dword(buf, off + MC_ROW_STRIDE))
        }
        McDispatchMode::HorizontalHalfPel => mc_horiz_half_pel_pair(
            read_dword(buf, off),
            read_dword(buf, off + MC_BYTES_PER_DWORD),
        ),
        McDispatchMode::BothHalfPel => mc_both_half_pel_quad(
            read_dword(buf, off),
            read_dword(buf, off + MC_BYTES_PER_DWORD),
            read_dword(buf, off + MC_ROW_STRIDE),
            read_dword(buf, off + MC_ROW_STRIDE + MC_BYTES_PER_DWORD),
        ),
    }
}

/// Spec/05 §5.1 / §5.2 / §5.3 — execute the MC cell copy over a
/// strip pixel buffer.
///
/// Copies a `geometry`-shaped cell whose destination top-left byte
/// is `dst_off` from the source top-left byte `src_off` (both
/// offsets into the same strip pixel-buffer view `buf`, row stride
/// [`MC_ROW_STRIDE`] = `0xb0`), applying the `mode` filter per
/// DWORD.
///
/// Iteration order matches the §5.1 inner loop: within one 4-row
/// band and one 4-pixel column group, rows 0 and 1 are read then
/// written, then rows 2 and 3 are read then written
/// (`mov edx, [esi]; mov eax, [esi + 0xb0]; mov [edi], edx;
/// mov [edi + 0xb0], eax; …`); column groups advance left to right
/// within the band (`lea edi, [edi + 0x4]`; `dec cl; jne`), and
/// bands advance top to bottom (`sub ecx, 0x1000000; jae`). For
/// overlapping source / destination ranges the read-before-write
/// pairing therefore reproduces the binary's dataflow.
///
/// The §5.2 half-pel modes read one extra source row below the cell
/// (vertical / both) and one extra source DWORD to the right of the
/// cell's last column group (horizontal / both) — the §4.4 padding
/// reads; the bound check accounts for them.
///
/// Returns the number of pixel bytes written
/// (`width_pixels * height_pixels`).
pub fn mc_copy_cell(
    buf: &mut [u8],
    dst_off: usize,
    src_off: usize,
    geometry: McKernelGeometry,
    mode: McDispatchMode,
) -> Result<usize, McCopyError> {
    let width_bytes = geometry.width_pixels();
    let height_rows = geometry.height_pixels();

    // §5.3: the destination walk's last byte is the end of the last
    // row.
    let dst_required = dst_off + (height_rows - 1) * MC_ROW_STRIDE + width_bytes;
    if buf.len() < dst_required {
        return Err(McCopyError::DstOutOfBounds {
            required: dst_required,
            supplied: buf.len(),
        });
    }

    // §5.2 / §4.4: vertical-filter modes read one row below the
    // cell's last row; horizontal-filter modes read one DWORD past
    // the cell's last column group.
    let extra_rows = if mode.applies_vertical_half_pel() {
        1
    } else {
        0
    };
    let extra_bytes = if mode.applies_horizontal_half_pel() {
        MC_BYTES_PER_DWORD
    } else {
        0
    };
    let src_required =
        src_off + (height_rows - 1 + extra_rows) * MC_ROW_STRIDE + width_bytes + extra_bytes;
    if buf.len() < src_required {
        return Err(McCopyError::SrcOutOfBounds {
            required: src_required,
            supplied: buf.len(),
        });
    }

    for band in 0..geometry.row_bands() {
        let band_row = band * MC_BAND_ROWS;
        for group in 0..geometry.column_groups() {
            let col = group * MC_BYTES_PER_DWORD;
            // §5.1 read/write pairing: rows (0, 1) then rows (2, 3).
            for pair in 0..(MC_BAND_ROWS / 2) {
                let row_a = band_row + 2 * pair;
                let row_b = row_a + 1;
                let src_a = src_off + row_a * MC_ROW_STRIDE + col;
                let src_b = src_off + row_b * MC_ROW_STRIDE + col;
                let d_a = fetch_source_dword(buf, mode, src_a);
                let d_b = fetch_source_dword(buf, mode, src_b);
                write_dword(buf, dst_off + row_a * MC_ROW_STRIDE + col, d_a);
                write_dword(buf, dst_off + row_b * MC_ROW_STRIDE + col, d_b);
            }
        }
    }

    Ok(width_bytes * height_rows)
}

/// Spec/05 §2.2 / §2.3 / §5 — execute the MC cell copy driven by a
/// packed MV: derives the four-way dispatch mode from the MV's low
/// two bits and the source base from the §2.3
/// `src = dst + sar(packed_mv, 2)` displacement, then runs
/// [`mc_copy_cell`].
pub fn mc_copy_cell_mv(
    buf: &mut [u8],
    dst_off: usize,
    mv: PackedMv,
    geometry: McKernelGeometry,
) -> Result<usize, McCopyError> {
    let src_off = mv.source_address(dst_off).ok_or(McCopyError::MvUnderflow {
        dst_cell_base: dst_off,
        pixel_offset: mv.pixel_offset(),
    })?;
    mc_copy_cell(buf, dst_off, src_off, geometry, mv.mode())
}

// ---- spec/03 §5.5 per-cell edge fix-up executor ----------------------

/// Spec/03 §5.5 — failure modes of the safe-Rust bound check
/// [`apply_per_cell_edge_fixup`] applies at the buffer edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PerCellEdgeFixupError {
    /// A zero cell height would underflow the §5.5 do-while counter
    /// (`edx -= 4; while (edx > 0)` runs its body at least once).
    ZeroHeight,
    /// The next cell's offset is below 4 — the `[edi - 0x4]` write
    /// would land before the buffer start.
    NextOffsetUnderflow {
        /// The supplied next-cell byte offset.
        next_cell_off: usize,
    },
    /// The previous cell's `[esi + 0x24]` / `[esi + 0x28]` walk
    /// would extend past the end of the supplied buffer.
    PrevOutOfBounds {
        /// Minimum buffer length the previous-cell walk requires.
        required: usize,
        /// Supplied buffer length.
        supplied: usize,
    },
    /// The next cell's `[edi]` walk would extend past the end of
    /// the supplied buffer.
    NextOutOfBounds {
        /// Minimum buffer length the next-cell walk requires.
        required: usize,
        /// Supplied buffer length.
        supplied: usize,
    },
}

impl core::fmt::Display for PerCellEdgeFixupError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PerCellEdgeFixupError::ZeroHeight => write!(
                f,
                "spec/03 §5.5: the edge fix-up's do-while height counter requires a \
                 non-zero cell height"
            ),
            PerCellEdgeFixupError::NextOffsetUnderflow { next_cell_off } => write!(
                f,
                "spec/03 §5.5: next-cell offset {next_cell_off} leaves no room for the \
                 [edi - 0x4] write"
            ),
            PerCellEdgeFixupError::PrevOutOfBounds { required, supplied } => write!(
                f,
                "spec/03 §5.5: previous-cell edge walk requires {required} byte(s); \
                 buffer has {supplied}"
            ),
            PerCellEdgeFixupError::NextOutOfBounds { required, supplied } => write!(
                f,
                "spec/03 §5.5: next-cell edge walk requires {required} byte(s); \
                 buffer has {supplied}"
            ),
        }
    }
}

impl std::error::Error for PerCellEdgeFixupError {}

/// Spec/03 §5.5 — execute the per-cell (inter-cell) edge fix-up at
/// `IR32_32.DLL!0x10006574..0x100065a3` over a strip pixel buffer.
///
/// `prev_cell_off` is the previous cell's pointer (`esi`) and
/// `next_cell_off` the next cell's pointer (`edi`) at loop entry;
/// `height_px` is the cell height the `edx` counter starts from.
/// Each iteration runs the §5.5 body:
///
/// ```text
/// eax = [edi]                 ; top edge of next cell
/// ebx = [esi + 0x24]          ; previous cell's bottom-right edge
/// [edi - 0x4] = ebx           ; write to next cell's top-left
/// [esi + 0x28] = eax          ; write to previous cell's bottom-right-adjacent
/// esi += 0xb0                 ; row stride
/// edi += 0xb0
/// edx -= 4                    ; height -= 4
/// while (edx > 0): repeat
/// ```
///
/// The do-while shape means the body runs `ceil(height_px / 4)`
/// times (at least once); both pointers advance one row stride
/// ([`PER_CELL_EDGE_ROW_STRIDE`] = `0xb0`) per iteration while the
/// counter steps by [`PER_CELL_EDGE_HEIGHT_STEP`] (= 4). Per
/// spec/07 §1.3 the copies keep the next cell's `[edi - 0xb0]`
/// predictor reads valid across the cell boundary.
///
/// Returns the number of iterations executed.
pub fn apply_per_cell_edge_fixup(
    buf: &mut [u8],
    prev_cell_off: usize,
    next_cell_off: usize,
    height_px: u32,
) -> Result<u32, PerCellEdgeFixupError> {
    if height_px == 0 {
        return Err(PerCellEdgeFixupError::ZeroHeight);
    }
    if next_cell_off < MC_BYTES_PER_DWORD {
        return Err(PerCellEdgeFixupError::NextOffsetUnderflow { next_cell_off });
    }

    // do-while with `edx -= 4; while (edx > 0)` → ceil(height / 4)
    // iterations.
    let iterations = height_px.div_ceil(PER_CELL_EDGE_HEIGHT_STEP);
    let last = (iterations as usize - 1) * PER_CELL_EDGE_ROW_STRIDE;

    let prev_required =
        prev_cell_off + last + PER_CELL_EDGE_PREV_BR_NEXT_OFFSET + MC_BYTES_PER_DWORD;
    if buf.len() < prev_required {
        return Err(PerCellEdgeFixupError::PrevOutOfBounds {
            required: prev_required,
            supplied: buf.len(),
        });
    }
    let next_required = next_cell_off + last + MC_BYTES_PER_DWORD;
    if buf.len() < next_required {
        return Err(PerCellEdgeFixupError::NextOutOfBounds {
            required: next_required,
            supplied: buf.len(),
        });
    }

    let mut prev = prev_cell_off;
    let mut next = next_cell_off;
    for _ in 0..iterations {
        let next_top = read_dword(buf, next);
        let prev_br = read_dword(buf, prev + PER_CELL_EDGE_PREV_BR_OFFSET);
        write_dword(buf, next - MC_BYTES_PER_DWORD, prev_br);
        write_dword(buf, prev + PER_CELL_EDGE_PREV_BR_NEXT_OFFSET, next_top);
        prev += PER_CELL_EDGE_ROW_STRIDE;
        next += PER_CELL_EDGE_ROW_STRIDE;
    }

    Ok(iterations)
}

// ---- consistency assertions ----------------------------------------

const _: () = {
    // §7.2: the boundary-fix-up scratch slot is distinct from the
    // three §4.3 dispatcher cell-data scratch slots (0x24 / 0x28 /
    // 0x38).
    assert!(
        BOUNDARY_FIXUP_SCRATCH_OFFSET
            != super::mc_source_plumbing::DISPATCHER_SCRATCH_SRC_DATA_OFFSET
    );
    assert!(
        BOUNDARY_FIXUP_SCRATCH_OFFSET
            != super::mc_source_plumbing::DISPATCHER_SCRATCH_DST_DATA_OFFSET
    );
    assert!(
        BOUNDARY_FIXUP_SCRATCH_OFFSET
            != super::mc_source_plumbing::DISPATCHER_SCRATCH_EXTRA_OFFSET_OFFSET
    );
    // §7.2 / §3.2: the aux shift equals the packed-MV pixel-offset
    // shift — both stored forms scale the byte offset by 4.
    assert!(BOUNDARY_FIXUP_AUX_SHIFT == super::mc_packed::MV_PIXEL_OFFSET_SHIFT);
    // spec/07 §1.2: the per-row advance is the strip row stride.
    assert!(BOUNDARY_FIXUP_ROW_ADVANCE == MC_ROW_STRIDE);
    assert!(BOUNDARY_FIXUP_ROW_ADVANCE == 0xb0);
    // spec/03 §5.5: the fix-up's row stride and the MC row stride
    // agree.
    assert!(PER_CELL_EDGE_ROW_STRIDE == MC_ROW_STRIDE);
};

#[cfg(test)]
mod tests {
    use super::super::frame_output::upshift_7bit_to_8bit;
    use super::super::mc_packed::pack_mv_components;
    use super::super::strip_context::PIXEL_BUFFER_ARENA_LEN;
    use super::*;

    // ---- §7.2 boundary-fix-up reduction ---------------------------

    #[test]
    fn boundary_fixup_scratch_slot_is_0x34() {
        // §7.2: "dst_cell_offset = [esp+0x34] for boundary-fix-up
        // use".
        assert_eq!(BOUNDARY_FIXUP_SCRATCH_OFFSET, 0x34);
    }

    #[test]
    fn boundary_fixup_reduction_composes_three_terms() {
        // §7.2: cell_offset = aux sar 2 + extra_offset + ch.
        assert_eq!(boundary_fixup_dst_cell_offset(0x40, 0x10, 0x02), 0x22);
        assert_eq!(boundary_fixup_dst_cell_offset(0, 0, 0), 0);
        assert_eq!(boundary_fixup_dst_cell_offset(4, 1, 1), 3);
    }

    #[test]
    fn boundary_fixup_aux_shift_is_arithmetic() {
        // §3.2 (sibling disposition): the shift is `sar`, not `shr` —
        // negative aux DWORDs keep their sign.
        assert_eq!(boundary_fixup_dst_cell_offset(-4, 0, 0), -1);
        assert_eq!(boundary_fixup_dst_cell_offset(-1, 0, 0), -1);
        assert_eq!(boundary_fixup_dst_cell_offset(i32::MIN, 0, 0), -0x2000_0000);
    }

    #[test]
    fn boundary_fixup_no_wrap_at_extremes() {
        // The i64 widening keeps the worst-case operands exact.
        let v = boundary_fixup_dst_cell_offset(i32::MAX, u32::MAX, u8::MAX);
        assert_eq!(v, (i32::MAX >> 2) as i64 + u32::MAX as i64 + u8::MAX as i64);
    }

    #[test]
    fn boundary_fixup_row_advance_is_row_stride() {
        // spec/07 §1.2: `add [esp + 0x34], 0xb0`.
        assert_eq!(BOUNDARY_FIXUP_ROW_ADVANCE, 0xb0);
        assert_eq!(advance_boundary_fixup_row(0), 0xb0);
        assert_eq!(advance_boundary_fixup_row(0xb0), 0x160);
        let mut off = boundary_fixup_dst_cell_offset(0x40, 0, 0);
        for _ in 0..4 {
            off = advance_boundary_fixup_row(off);
        }
        assert_eq!(off, 0x10 + 4 * 0xb0);
    }

    // ---- §5.1 full-pel copy ---------------------------------------

    /// A deterministic 7-bit test pattern, distinct per (row, col).
    fn pattern(row: usize, col: usize) -> u8 {
        ((row * 19 + col * 7 + 3) & 0x7f) as u8
    }

    /// Fill `rows` rows × `cols` cols at `base` with [`pattern`].
    fn fill_pattern(buf: &mut [u8], base: usize, rows: usize, cols: usize) {
        for r in 0..rows {
            for c in 0..cols {
                buf[base + r * MC_ROW_STRIDE + c] = pattern(r, c);
            }
        }
    }

    #[test]
    fn full_pel_copies_cell_exactly() {
        // §5.1: the full-pel fetcher reproduces the source cell
        // byte-for-byte at the destination.
        let mut buf = vec![0u8; 32 * MC_ROW_STRIDE];
        let src = 0;
        let dst = 16 * MC_ROW_STRIDE + 8;
        fill_pattern(&mut buf, src, 8, 8);
        let written = mc_copy_cell(
            &mut buf,
            dst,
            src,
            McKernelGeometry::new(8, 8).unwrap(),
            McDispatchMode::FullPel,
        )
        .unwrap();
        assert_eq!(written, 64);
        for r in 0..8 {
            for c in 0..8 {
                assert_eq!(buf[dst + r * MC_ROW_STRIDE + c], pattern(r, c));
            }
        }
    }

    #[test]
    fn full_pel_preserves_untouched_bytes() {
        // §5.3: only the cell's `width × height` raster is written;
        // bytes outside it (stride padding, neighbour cells) stay.
        let mut buf = vec![0x55u8; 16 * MC_ROW_STRIDE];
        let src = 0;
        let dst = 8 * MC_ROW_STRIDE;
        fill_pattern(&mut buf, src, 4, 4);
        mc_copy_cell(
            &mut buf,
            dst,
            src,
            McKernelGeometry::new(4, 4).unwrap(),
            McDispatchMode::FullPel,
        )
        .unwrap();
        // One byte right of the cell, one row below the cell.
        assert_eq!(buf[dst + 4], 0x55);
        assert_eq!(buf[dst + 4 * MC_ROW_STRIDE], 0x55);
        // Row padding between src rows is untouched.
        assert_eq!(buf[src + 4], 0x55);
    }

    #[test]
    fn full_pel_minimum_cell() {
        // §2.4 / §5.1: the 4×4 minimum cell — one band, one group.
        let mut buf = vec![0u8; 8 * MC_ROW_STRIDE];
        fill_pattern(&mut buf, 0, 4, 4);
        let written = mc_copy_cell(
            &mut buf,
            4 * MC_ROW_STRIDE + 4,
            0,
            McKernelGeometry::new(4, 4).unwrap(),
            McDispatchMode::FullPel,
        )
        .unwrap();
        assert_eq!(written, 16);
        for r in 0..4 {
            for c in 0..4 {
                assert_eq!(
                    buf[4 * MC_ROW_STRIDE + 4 + r * MC_ROW_STRIDE + c],
                    pattern(r, c)
                );
            }
        }
    }

    #[test]
    fn full_pel_self_copy_is_identity() {
        // §4.2's degenerate same-bank case: dst == src leaves the
        // buffer unchanged.
        let mut buf = vec![0u8; 8 * MC_ROW_STRIDE];
        fill_pattern(&mut buf, 0, 8, 8);
        let before = buf.clone();
        mc_copy_cell(
            &mut buf,
            0,
            0,
            McKernelGeometry::new(8, 8).unwrap(),
            McDispatchMode::FullPel,
        )
        .unwrap();
        assert_eq!(buf, before);
    }

    // ---- §5.2 half-pel variants ------------------------------------

    #[test]
    fn vertical_half_pel_averages_row_pairs() {
        // §5.2 `01`: output row r = floor((src[r] + src[r+1]) / 2)
        // per byte — the extra row below the cell is read.
        let mut buf = vec![0u8; 16 * MC_ROW_STRIDE];
        let src = 0;
        let dst = 8 * MC_ROW_STRIDE;
        fill_pattern(&mut buf, src, 5, 4); // 4 rows + 1 neighbour row
        mc_copy_cell(
            &mut buf,
            dst,
            src,
            McKernelGeometry::new(4, 4).unwrap(),
            McDispatchMode::VerticalHalfPel,
        )
        .unwrap();
        for r in 0..4 {
            for c in 0..4 {
                let expect = (pattern(r, c) as u16 + pattern(r + 1, c) as u16) / 2;
                assert_eq!(
                    buf[dst + r * MC_ROW_STRIDE + c],
                    expect as u8,
                    "row {r} col {c}"
                );
            }
        }
    }

    #[test]
    fn horizontal_half_pel_averages_column_pairs() {
        // §5.2 `10`: output col c = floor((src[c] + src[c+1]) / 2)
        // per byte — the extra DWORD right of the cell is read.
        let mut buf = vec![0u8; 16 * MC_ROW_STRIDE];
        let src = 0;
        let dst = 8 * MC_ROW_STRIDE;
        fill_pattern(&mut buf, src, 4, 8); // 4 cols + neighbour DWORD
        mc_copy_cell(
            &mut buf,
            dst,
            src,
            McKernelGeometry::new(4, 4).unwrap(),
            McDispatchMode::HorizontalHalfPel,
        )
        .unwrap();
        for r in 0..4 {
            for c in 0..4 {
                let expect = (pattern(r, c) as u16 + pattern(r, c + 1) as u16) / 2;
                assert_eq!(
                    buf[dst + r * MC_ROW_STRIDE + c],
                    expect as u8,
                    "row {r} col {c}"
                );
            }
        }
    }

    #[test]
    fn both_half_pel_is_2x2_box() {
        // §5.2 `11`: 2×2 unweighted average, composed horizontal-
        // pair-first then vertical (two floor-rounded steps).
        let mut buf = vec![0u8; 16 * MC_ROW_STRIDE];
        let src = 0;
        let dst = 8 * MC_ROW_STRIDE;
        fill_pattern(&mut buf, src, 5, 8);
        mc_copy_cell(
            &mut buf,
            dst,
            src,
            McKernelGeometry::new(4, 4).unwrap(),
            McDispatchMode::BothHalfPel,
        )
        .unwrap();
        for r in 0..4 {
            for c in 0..4 {
                let top = (pattern(r, c) as u16 + pattern(r, c + 1) as u16) / 2;
                let bot = (pattern(r + 1, c) as u16 + pattern(r + 1, c + 1) as u16) / 2;
                let expect = (top + bot) / 2;
                assert_eq!(
                    buf[dst + r * MC_ROW_STRIDE + c],
                    expect as u8,
                    "row {r} col {c}"
                );
            }
        }
    }

    #[test]
    fn half_pel_uniform_source_is_fixed_point() {
        // All four modes leave a uniform 7-bit source value unchanged
        // (every neighbour pair averages to itself).
        for mode in [
            McDispatchMode::FullPel,
            McDispatchMode::VerticalHalfPel,
            McDispatchMode::HorizontalHalfPel,
            McDispatchMode::BothHalfPel,
        ] {
            let mut buf = vec![0x3au8; 16 * MC_ROW_STRIDE];
            mc_copy_cell(
                &mut buf,
                8 * MC_ROW_STRIDE,
                0,
                McKernelGeometry::new(8, 8).unwrap(),
                mode,
            )
            .unwrap();
            assert!(
                buf.iter().all(|&b| b == 0x3a),
                "mode {mode:?} altered a uniform buffer"
            );
        }
    }

    // ---- §2.2 / §2.3 MV-driven entry --------------------------------

    #[test]
    fn mv_driven_copy_matches_direct_call_per_mode() {
        // §2.2 / §2.3: pack (vert, horiz) + mode bits, decode through
        // PackedMv, and confirm the copy equals the direct-call form.
        let geometry = McKernelGeometry::new(8, 8).unwrap();
        let dst = 16 * MC_ROW_STRIDE + 16;
        for (vert, horiz, vert_lsb, horiz_lsb, mode) in [
            (-8i32, 4i32, 0u32, 0u32, McDispatchMode::FullPel),
            (-8, 4, 1, 0, McDispatchMode::VerticalHalfPel),
            (-8, 4, 0, 1, McDispatchMode::HorizontalHalfPel),
            (-8, 4, 1, 1, McDispatchMode::BothHalfPel),
        ] {
            let packed = pack_mv_components(vert, horiz, vert_lsb, horiz_lsb);
            let mv = PackedMv::from_raw(packed);
            assert_eq!(mv.mode(), mode);

            let mut via_mv = vec![0u8; 32 * MC_ROW_STRIDE];
            fill_pattern(&mut via_mv, 0, 24, 32);
            let mut direct = via_mv.clone();

            mc_copy_cell_mv(&mut via_mv, dst, mv, geometry).unwrap();
            let src = (dst as i64 + (vert as i64 * MC_ROW_STRIDE as i64) + horiz as i64) as usize;
            mc_copy_cell(&mut direct, dst, src, geometry, mode).unwrap();
            assert_eq!(via_mv, direct, "mode {mode:?}");
        }
    }

    #[test]
    fn mv_underflow_is_reported() {
        // §2.3: a displacement that moves the source base below the
        // buffer start surfaces as MvUnderflow.
        let mut buf = vec![0u8; 8 * MC_ROW_STRIDE];
        let mv = PackedMv::from_raw(pack_mv_components(-4, 0, 0, 0));
        let err = mc_copy_cell_mv(&mut buf, 0, mv, McKernelGeometry::new(4, 4).unwrap());
        assert_eq!(
            err,
            Err(McCopyError::MvUnderflow {
                dst_cell_base: 0,
                pixel_offset: -4 * MC_ROW_STRIDE as i32,
            })
        );
    }

    // ---- bound-check error paths ------------------------------------

    #[test]
    fn dst_out_of_bounds_is_reported() {
        let mut buf = vec![0u8; 4 * MC_ROW_STRIDE];
        let err = mc_copy_cell(
            &mut buf,
            2 * MC_ROW_STRIDE,
            0,
            McKernelGeometry::new(4, 4).unwrap(),
            McDispatchMode::FullPel,
        );
        assert_eq!(
            err,
            Err(McCopyError::DstOutOfBounds {
                required: 2 * MC_ROW_STRIDE + 3 * MC_ROW_STRIDE + 4,
                supplied: 4 * MC_ROW_STRIDE,
            })
        );
    }

    #[test]
    fn src_out_of_bounds_full_pel_is_reported() {
        let mut buf = vec![0u8; 4 * MC_ROW_STRIDE];
        let err = mc_copy_cell(
            &mut buf,
            0,
            2 * MC_ROW_STRIDE,
            McKernelGeometry::new(4, 4).unwrap(),
            McDispatchMode::FullPel,
        );
        assert_eq!(
            err,
            Err(McCopyError::SrcOutOfBounds {
                required: 2 * MC_ROW_STRIDE + 3 * MC_ROW_STRIDE + 4,
                supplied: 4 * MC_ROW_STRIDE,
            })
        );
    }

    #[test]
    fn src_bound_accounts_for_half_pel_neighbour_row() {
        // §5.2 `01` reads one row below the cell: a buffer exactly
        // big enough for full-pel fails the vertical-mode bound.
        let exact_full_pel = 3 * MC_ROW_STRIDE + 4;
        let mut buf = vec![0u8; exact_full_pel];
        let geometry = McKernelGeometry::new(4, 4).unwrap();
        assert!(mc_copy_cell(&mut buf, 0, 0, geometry, McDispatchMode::FullPel).is_ok());
        assert_eq!(
            mc_copy_cell(&mut buf, 0, 0, geometry, McDispatchMode::VerticalHalfPel),
            Err(McCopyError::SrcOutOfBounds {
                required: 4 * MC_ROW_STRIDE + 4,
                supplied: exact_full_pel,
            })
        );
    }

    #[test]
    fn src_bound_accounts_for_half_pel_neighbour_dword() {
        // §5.2 `10` reads one DWORD right of the cell's last column
        // group.
        let exact_full_pel = 3 * MC_ROW_STRIDE + 4;
        let mut buf = vec![0u8; exact_full_pel];
        let geometry = McKernelGeometry::new(4, 4).unwrap();
        assert_eq!(
            mc_copy_cell(&mut buf, 0, 0, geometry, McDispatchMode::HorizontalHalfPel),
            Err(McCopyError::SrcOutOfBounds {
                required: 3 * MC_ROW_STRIDE + 8,
                supplied: exact_full_pel,
            })
        );
    }

    #[test]
    fn copy_error_display_cites_spec_sections() {
        let d = McCopyError::DstOutOfBounds {
            required: 10,
            supplied: 5,
        }
        .to_string();
        assert!(d.contains("spec/05"), "{d}");
        let s = McCopyError::SrcOutOfBounds {
            required: 10,
            supplied: 5,
        }
        .to_string();
        assert!(s.contains("§4.4"), "{s}");
        let m = McCopyError::MvUnderflow {
            dst_cell_base: 0,
            pixel_offset: -1,
        }
        .to_string();
        assert!(m.contains("§2.3"), "{m}");
    }

    // ---- spec/03 §5.5 per-cell edge fix-up ---------------------------

    #[test]
    fn edge_fixup_exchanges_boundary_dwords() {
        // §5.5 body, single iteration (height 4): the previous
        // cell's `[esi + 0x24]` DWORD lands at `[edi - 4]`, the next
        // cell's `[edi]` DWORD lands at `[esi + 0x28]`.
        let mut buf = vec![0u8; 4 * MC_ROW_STRIDE];
        let prev = 8;
        let next = 0x40;
        buf[prev + PER_CELL_EDGE_PREV_BR_OFFSET..prev + PER_CELL_EDGE_PREV_BR_OFFSET + 4]
            .copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
        buf[next..next + 4].copy_from_slice(&[0x55, 0x66, 0x77, 0x08]);
        let iters = apply_per_cell_edge_fixup(&mut buf, prev, next, 4).unwrap();
        assert_eq!(iters, 1);
        assert_eq!(&buf[next - 4..next], &[0x11, 0x22, 0x33, 0x44]);
        assert_eq!(
            &buf[prev + PER_CELL_EDGE_PREV_BR_NEXT_OFFSET
                ..prev + PER_CELL_EDGE_PREV_BR_NEXT_OFFSET + 4],
            &[0x55, 0x66, 0x77, 0x08]
        );
    }

    #[test]
    fn edge_fixup_advances_one_row_per_iteration() {
        // §5.5: `esi += 0xb0; edi += 0xb0; edx -= 4` — height 16 runs
        // four iterations, one row apart.
        let mut buf = vec![0u8; 8 * MC_ROW_STRIDE];
        let prev = 0;
        let next = 0x50;
        for it in 0..4usize {
            let row = it * PER_CELL_EDGE_ROW_STRIDE;
            buf[prev + row + PER_CELL_EDGE_PREV_BR_OFFSET] = 0x10 + it as u8;
            buf[next + row] = 0x60 + it as u8;
        }
        let iters = apply_per_cell_edge_fixup(&mut buf, prev, next, 16).unwrap();
        assert_eq!(iters, 4);
        for it in 0..4usize {
            let row = it * PER_CELL_EDGE_ROW_STRIDE;
            assert_eq!(buf[next + row - 4], 0x10 + it as u8, "iteration {it}");
            assert_eq!(
                buf[prev + row + PER_CELL_EDGE_PREV_BR_NEXT_OFFSET],
                0x60 + it as u8,
                "iteration {it}"
            );
        }
    }

    #[test]
    fn edge_fixup_do_while_rounds_height_up() {
        // §5.5: `edx -= 4; while (edx > 0)` — heights 1..=4 run one
        // iteration, 5..=8 run two.
        for h in 1..=4u32 {
            let mut buf = vec![0u8; 2 * MC_ROW_STRIDE];
            assert_eq!(apply_per_cell_edge_fixup(&mut buf, 0, 0x40, h), Ok(1));
        }
        for h in 5..=8u32 {
            let mut buf = vec![0u8; 3 * MC_ROW_STRIDE];
            assert_eq!(apply_per_cell_edge_fixup(&mut buf, 0, 0x40, h), Ok(2));
        }
    }

    #[test]
    fn edge_fixup_rejects_zero_height() {
        let mut buf = vec![0u8; MC_ROW_STRIDE];
        assert_eq!(
            apply_per_cell_edge_fixup(&mut buf, 0, 0x40, 0),
            Err(PerCellEdgeFixupError::ZeroHeight)
        );
    }

    #[test]
    fn edge_fixup_rejects_next_offset_underflow() {
        // §5.5's `[edi - 0x4]` write needs `next >= 4`.
        let mut buf = vec![0u8; MC_ROW_STRIDE];
        assert_eq!(
            apply_per_cell_edge_fixup(&mut buf, 0x40, 3, 4),
            Err(PerCellEdgeFixupError::NextOffsetUnderflow { next_cell_off: 3 })
        );
    }

    #[test]
    fn edge_fixup_rejects_out_of_bounds_walks() {
        let mut buf = vec![0u8; 0x40];
        // prev walk reaches prev + 0x28 + 4 > len.
        assert_eq!(
            apply_per_cell_edge_fixup(&mut buf, 0x20, 0x10, 4),
            Err(PerCellEdgeFixupError::PrevOutOfBounds {
                required: 0x20 + PER_CELL_EDGE_PREV_BR_NEXT_OFFSET + 4,
                supplied: 0x40,
            })
        );
        // next walk reaches next + 4 > len (prev walk fits).
        let mut buf2 = vec![0u8; 0x40];
        assert_eq!(
            apply_per_cell_edge_fixup(&mut buf2, 0x00, 0x3e, 4),
            Err(PerCellEdgeFixupError::NextOutOfBounds {
                required: 0x3e + 4,
                supplied: 0x40,
            })
        );
    }

    #[test]
    fn edge_fixup_error_display_cites_spec_sections() {
        for e in [
            PerCellEdgeFixupError::ZeroHeight,
            PerCellEdgeFixupError::NextOffsetUnderflow { next_cell_off: 1 },
            PerCellEdgeFixupError::PrevOutOfBounds {
                required: 9,
                supplied: 1,
            },
            PerCellEdgeFixupError::NextOutOfBounds {
                required: 9,
                supplied: 1,
            },
        ] {
            assert!(e.to_string().contains("spec/03 §5.5"), "{e}");
        }
    }

    // ---- end-to-end fixture: MV → MC copy → fix-up → 8-bit output ---

    #[test]
    fn fixture_mv_prediction_reaches_8bit_output() {
        // End-to-end over a spec/02 §7-sized arena fixture: pack a
        // packed-MV DWORD (§3.3), decode + dispatch it (§2.2 / §2.3),
        // run the §5.1 cell copy from a reference region into a
        // destination cell, run the spec/03 §5.5 inter-cell fix-up,
        // then push the predicted cell through the spec/07 §4.3
        // upshift — actual 8-bit pixel output from a motion-
        // compensated prediction.
        let mut arena = vec![0u8; PIXEL_BUFFER_ARENA_LEN];

        // Reference region (the §4.1 / §4.2 source bank): rows 0..9
        // of an 8-wide gradient at the top of the arena.
        fill_pattern(&mut arena, 0, 9, 12);

        // Destination cell 16 rows down, same column: vert = -16,
        // horiz = 0, full-pel.
        let dst = 16 * MC_ROW_STRIDE;
        let geometry = McKernelGeometry::new(8, 8).unwrap();
        let mv = PackedMv::from_raw(pack_mv_components(-16, 0, 0, 0));
        let written = mc_copy_cell_mv(&mut arena, dst, mv, geometry).unwrap();
        assert_eq!(written, 64);

        // A second 8×8 cell to the right of the first; §5.5 inter-
        // cell fix-up between them keeps the boundary DWORDs
        // exchanged for predictor continuity (spec/07 §1.3).
        let next = dst + 8;
        let iters = apply_per_cell_edge_fixup(&mut arena, dst, next, 8).unwrap();
        assert_eq!(iters, 2);

        // §4.3 output upshift: the predicted cell's 7-bit bytes reach
        // the 8-bit output range as exactly 2 × the prediction.
        for r in 0..8 {
            for c in 0..8 {
                let predicted = arena[dst + r * MC_ROW_STRIDE + c];
                let out = upshift_7bit_to_8bit(predicted);
                assert_eq!(out, (predicted & 0x7f) << 1, "row {r} col {c}");
                if c < 4 {
                    // Columns 0..3 are untouched by the fix-up's
                    // `[edi - 4]` write into columns 4..7 of this
                    // cell's right edge; they still equal the MV-
                    // displaced reference, so the 8-bit output is
                    // exactly twice the reference pattern.
                    assert_eq!(out, pattern(r, c) << 1, "row {r} col {c}");
                }
            }
        }
    }
}
