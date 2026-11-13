//! Indeo 3 per-cell sub-array indexing (the cell-stack at `[+0x40+]`
//! within each strip-context slot).
//!
//! Spec source: `docs/video/indeo/indeo3/spec/03-macroblock-layer.md`
//! §5.1 (field map), §5.3 (cell-stack at `[+0x40..]`), §5.5 (per-cell
//! edge fix-up).
//!
//! Round 8 (`strip_context.rs`) defined the strip-context slot's
//! 1 KiB layout up to and including offset `+0x1c` (strip width). The
//! remainder of each 1 KiB slot, starting at byte offset `+0x40`,
//! holds the **cell-stack**: a 4-byte-per-entry array indexed by the
//! binary-tree walker's `(slot_idx, cell_idx)` pair. §5.3 enumerates
//! three read sites within `IR32_32.DLL!0x10006538` that consult this
//! array; §5.5 documents two further byte-offset constants
//! (`+0x24`, `+0x28`) used by the inter-cell edge fix-up that runs
//! whenever the cell-stack top is non-zero.
//!
//! The cell-stack is **read-only** during the per-frame decode
//! (`spec/03 §5.3`): the binary's `IR32_32.DLL!0x10006538` never
//! writes to `[+0x40+]`; the pre-frame setup populates the array
//! with per-strip cell pixel-buffer offsets. The pre-population
//! mechanism itself is `spec/03 §6` open question item 4
//! (`cell-stack pre-population`); this module surfaces only the
//! **read-side indexing arithmetic**, which is fully specified by
//! §5.1's table row for `+0x40+`.
//!
//! What this module **deliberately does not do** (the spec/03 §5
//! chapter boundary):
//!
//! * It does not own the cell-stack bytes. Like the strip-context
//!   slot itself, the cell-stack is allocated by the codec-init
//!   routine; this module exposes index arithmetic only.
//! * It does not pre-populate cell-stack entries. The pre-frame
//!   population mechanism is `spec/03 §6` open question 4.
//! * It does not perform the per-cell edge fix-up byte loop. That
//!   loop (§5.5's `eax = [edi]; ebx = [esi + 0x24]; [edi - 0x4] = ebx;
//!   [esi + 0x28] = eax; esi += 0xb0; edi += 0xb0; edx -= 4`) is the
//!   pixel-buffer-side work that follows the cell-stack top probe;
//!   this module surfaces only the two byte-offset constants
//!   (`+0x24`, `+0x28`) the loop consumes.
//! * It does not decode the cell-stack entries' contents. Each entry
//!   is a 4-byte pointer/offset whose interpretation is the
//!   pre-population routine's job (and §6 open question 4); the
//!   per-frame decoder only checks `entry == 0` (end-of-strip,
//!   §5.3 read at `0x10006ab5`) or `entry != 0` (inter-cell fix-up,
//!   §5.5).
//!
//! All offsets, field widths, and bounds are taken from §5.1 / §5.3 /
//! §5.5 of `03-macroblock-layer.md`. The RVAs cited in doc-comments
//! refer to the binary identified in `spec/00 §2`.

use super::strip_context::{STRIP_SLOT_COUNT, STRIP_SLOT_STRIDE, slot_field};

// ---- spec/03 §5.1 / §5.3 (cell-stack entry geometry) ---------------

/// Spec/03 §5.1 — cell-stack entry size in bytes (`4`, a 32-bit DWORD).
///
/// The cell-stack at `[+0x40..]` is indexed via `[ecx + 4*ebx + 0x40]`
/// throughout the binary; the `4*ebx` factor is this constant. Each
/// entry is a 4-byte cell-data pointer per §5.3.
pub const CELL_STACK_ENTRY_SIZE: usize = 4;

/// Spec/03 §5 / §5.1 — first byte of the cell-stack within a slot.
///
/// Re-exported as a convenience alongside the index helpers in this
/// module; the underlying constant is owned by
/// [`super::strip_context::slot_field::CELL_SUBARRAY_BEGIN`]
/// (`0x40`).
pub const CELL_STACK_BEGIN_OFFSET: usize = slot_field::CELL_SUBARRAY_BEGIN;

/// Spec/03 §5.1 — maximum number of cell-stack entries that fit in
/// one strip-context slot.
///
/// Derived in §5 ("Sizes and ranges within `+0x40+` for the cell-stack
/// are bounded by the slot stride `0x400`, giving a maximum of
/// `(0x400 - 0x40) / 4 = 240` cell-stack entries"). The MC_TREE depth
/// required to reach a 4×4 luma cell from a 160-pixel-wide strip is
/// `log2(160/4) ≈ 5.3` H-splits and `log2(plane_height/4)` V-splits,
/// well within this bound for any plane that complies with the
/// spec/02 §4.1 dimension limits.
pub const CELL_STACK_MAX_ENTRIES: usize =
    (STRIP_SLOT_STRIDE - CELL_STACK_BEGIN_OFFSET) / CELL_STACK_ENTRY_SIZE;

// ---- spec/03 §5.5 (per-cell edge fix-up field offsets) -------------

/// Spec/03 §5.5 — byte offset within the previous cell's pixel-buffer
/// pointer where the per-cell edge fix-up reads the previous cell's
/// bottom-right edge.
///
/// Cited site: `IR32_32.DLL!0x1000658b` reads `[esi + 0x24]` into
/// `ebx` and then stores that DWORD to the next cell's
/// `[edi - 0x4]` position (per §5.5's
/// `ebx = [esi + 0x24]; [edi - 0x4] = ebx`).
pub const PER_CELL_EDGE_PREV_BR_OFFSET: usize = 0x24;

/// Spec/03 §5.5 — byte offset within the previous cell's pixel-buffer
/// pointer where the per-cell edge fix-up writes the next cell's
/// top-edge DWORD.
///
/// Cited site: `IR32_32.DLL!0x10006594` writes `eax` (the next cell's
/// top edge previously loaded from `[edi]`) to `[esi + 0x28]` (per
/// §5.5's `[esi + 0x28] = eax`).
pub const PER_CELL_EDGE_PREV_BR_NEXT_OFFSET: usize = 0x28;

/// Spec/03 §5.5 — pixel-buffer row stride used by the per-cell edge
/// fix-up loop's `esi += 0xb0; edi += 0xb0` advance, in bytes.
///
/// The same `0xb0` (= 176) row stride is consumed by the inner cell
/// emission kernel via [`super::reconstruct::PREDICTOR_ROW_STRIDE`].
pub const PER_CELL_EDGE_ROW_STRIDE: usize = 0xb0;

/// Spec/03 §5.5 — height decrement per row pair of the per-cell edge
/// fix-up loop's `edx -= 4` step, in pixels.
///
/// `edx` enters the loop as the cell height in pixels; each iteration
/// processes a 4-row group (because cell heights are constrained to
/// multiples of 4 by spec/02 §4 / spec/03 §2.4 minimum-cell size).
pub const PER_CELL_EDGE_HEIGHT_STEP: u32 = 4;

// ---- spec/03 §5.3 (read-site enumeration) ---------------------------

/// Spec/03 §5.3 — the three read sites within `IR32_32.DLL!0x10006538`
/// that consult a strip-context slot's cell-stack.
///
/// The cell-stack is read-only during the per-frame decode; only the
/// pre-frame setup populates the array. Every load is by the
/// `[ctx + 4*<idx> + 0x40]` pattern enumerated here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellStackReadSite {
    /// Read at `IR32_32.DLL!0x1000656c` — the source slot's stack top
    /// is loaded into `edi` for the entry-time consistency check
    /// (`cmp edi, edi; je 0x100065a5` at `0x10006570..0x10006572`).
    SourceSlotTop,
    /// Read at `IR32_32.DLL!0x10006ab5` — the destination slot's stack
    /// top is loaded into `edi` for the inter-cell edge fix-up
    /// dispatch. `entry == 0` → strip-edge fix-up (§5.4); `entry != 0`
    /// → per-cell edge fix-up (§5.5).
    DestSlotTop,
    /// Read at `IR32_32.DLL!0x10006651` — `cmp [esi + 4*eax + 0x40], 0`
    /// checks whether the cell at the current `(ch, slot_idx)`
    /// position has a non-null pixel buffer. Non-null → INTER cell
    /// (motion-compensation reads from this buffer); null → INTRA
    /// cell (the codebook bank is offset by `+0xb00` to use the
    /// mirror sub-tables, per §4.2 and `cell_loop.rs`).
    CellPositionProbe,
}

impl CellStackReadSite {
    /// True iff this read site's `entry == 0` outcome branches to the
    /// strip-edge fix-up at `0x10006b4b..0x10006b80` (§5.4).
    ///
    /// Only [`Self::DestSlotTop`] has this branch (`test edi, edi;
    /// je 0x10006b4b` at `0x10006ab9..0x10006abb`).
    pub fn zero_means_strip_edge(self) -> bool {
        matches!(self, CellStackReadSite::DestSlotTop)
    }

    /// True iff this read site's `entry == 0` outcome selects the
    /// `+0xb00` codebook-bank mirror sub-tables (§4.2 INTRA-without-
    /// stack case).
    ///
    /// Only [`Self::CellPositionProbe`] has this disposition; the
    /// other two read sites use `entry == 0` as a dispatch signal
    /// rather than a bank-view selector.
    pub fn zero_means_mirror_bank(self) -> bool {
        matches!(self, CellStackReadSite::CellPositionProbe)
    }

    /// True iff the read load uses the destination-slot indexing
    /// pattern (`[ecx + 4*ebx + 0x40]` with `ecx` = destination slot
    /// base) versus the source-slot pattern.
    ///
    /// The wire-level effect is identical (both load 4 bytes at
    /// `slot_base + 0x40 + 4*idx`); this flag exists to mirror the
    /// binary's two-register convention (`esi` = source slot,
    /// `ecx` / `edi` = destination slot).
    pub fn uses_dest_slot_base(self) -> bool {
        matches!(
            self,
            CellStackReadSite::DestSlotTop | CellStackReadSite::CellPositionProbe
        )
    }
}

// ---- spec/03 §5.1 / §5.3 (index arithmetic) ------------------------

/// Spec/03 §5.1 — byte offset of cell-stack entry `entry_idx` within
/// a strip-context slot (relative to the slot's own start).
///
/// Returns `slot-relative + 0x40 + 4 * entry_idx`. Returns `None` if
/// `entry_idx >= CELL_STACK_MAX_ENTRIES` (the §5 derived 240-entry
/// bound). The result is always `< STRIP_SLOT_STRIDE` (1 KiB) on
/// success.
pub fn cell_stack_slot_offset(entry_idx: usize) -> Option<usize> {
    if entry_idx >= CELL_STACK_MAX_ENTRIES {
        return None;
    }
    Some(CELL_STACK_BEGIN_OFFSET + CELL_STACK_ENTRY_SIZE * entry_idx)
}

/// Spec/03 §5.1 + spec/02 §5 — byte offset of cell-stack entry
/// `(slot_idx, entry_idx)` within the strip-context array's full
/// byte view.
///
/// Returns `slot_idx * STRIP_SLOT_STRIDE + 0x40 + 4 * entry_idx`.
/// Returns `None` if `slot_idx >= STRIP_SLOT_COUNT` (the 32-slot
/// strip-context array bound from `spec/02 §5`) or if
/// `entry_idx >= CELL_STACK_MAX_ENTRIES` (the §5 derived 240-entry
/// bound).
pub fn cell_stack_array_offset(slot_idx: usize, entry_idx: usize) -> Option<usize> {
    if slot_idx >= STRIP_SLOT_COUNT {
        return None;
    }
    let within_slot = cell_stack_slot_offset(entry_idx)?;
    Some(slot_idx * STRIP_SLOT_STRIDE + within_slot)
}

// ---- spec/03 §5.3 + §5.5 (cell-stack top dispatch) -----------------

/// Spec/03 §5.3 + §5.4 / §5.5 — terminal disposition of a cell-stack
/// top load at the destination-slot read site (§5.3 read at
/// `0x10006ab5`).
///
/// The destination slot's stack top, loaded into `edi`, is tested by
/// `test edi, edi; je 0x10006b4b` at `0x10006ab9..0x10006abb`. The
/// two branches are §5.4 (strip-edge fix-up at end of strip) and §5.5
/// (per-cell edge fix-up between adjacent cells in the same strip).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellStackTopDispatch {
    /// Stack top is zero — the walker is at the end of a strip and
    /// runs the §5.4 strip-edge fix-up at
    /// `IR32_32.DLL!0x10006b4b..0x10006b80`.
    StripEdgeFixup,
    /// Stack top is non-zero — the walker runs the §5.5 per-cell edge
    /// fix-up at `IR32_32.DLL!0x10006574..0x100065a3` against the
    /// pixel-buffer pointers `[ctx + 4*ebx]` (`esi`) and
    /// `[ctx + 4*ebx + 0x40]` (`edi`).
    InterCellFixup {
        /// The non-zero cell-stack top DWORD value (a cell-data
        /// pointer per §5.3). Carried through so callers can pass it
        /// to the §5.5 byte-loop entry as `edi`.
        cell_data_ptr: u32,
    },
}

impl CellStackTopDispatch {
    /// Spec/03 §5.3 + §5.5 — classify a destination-slot cell-stack-
    /// top DWORD into the §5.4 vs §5.5 branch.
    pub fn from_dest_slot_top(cell_stack_top: u32) -> Self {
        if cell_stack_top == 0 {
            CellStackTopDispatch::StripEdgeFixup
        } else {
            CellStackTopDispatch::InterCellFixup {
                cell_data_ptr: cell_stack_top,
            }
        }
    }

    /// True iff this dispatch is the §5.4 strip-edge fix-up branch
    /// (cell-stack top was zero).
    pub fn is_strip_edge(self) -> bool {
        matches!(self, CellStackTopDispatch::StripEdgeFixup)
    }

    /// True iff this dispatch is the §5.5 inter-cell fix-up branch
    /// (cell-stack top was non-zero).
    pub fn is_inter_cell(self) -> bool {
        matches!(self, CellStackTopDispatch::InterCellFixup { .. })
    }

    /// Borrow the cell-data pointer on the inter-cell branch; `None`
    /// on the strip-edge branch.
    pub fn cell_data_ptr(self) -> Option<u32> {
        match self {
            CellStackTopDispatch::InterCellFixup { cell_data_ptr } => Some(cell_data_ptr),
            CellStackTopDispatch::StripEdgeFixup => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- §5 / §5.1 (cell-stack entry geometry constants) ------------

    #[test]
    fn cell_stack_entry_size_matches_4_byte_dword() {
        // §5.1: each entry is a 4-byte cell-data pointer; the
        // indexing pattern is `[ecx + 4*ebx + 0x40]`.
        assert_eq!(CELL_STACK_ENTRY_SIZE, 4);
    }

    #[test]
    fn cell_stack_begin_offset_matches_slot_field_constant() {
        // §5.1: the cell-stack starts at slot byte offset `+0x40`,
        // immediately after the strip-scratch region.
        assert_eq!(CELL_STACK_BEGIN_OFFSET, 0x40);
        assert_eq!(CELL_STACK_BEGIN_OFFSET, slot_field::CELL_SUBARRAY_BEGIN);
    }

    #[test]
    fn cell_stack_max_entries_matches_spec_derivation() {
        // §5: "(0x400 - 0x40) / 4 = 240 cell-stack entries".
        assert_eq!(CELL_STACK_MAX_ENTRIES, 240);
        assert_eq!(
            CELL_STACK_MAX_ENTRIES,
            (STRIP_SLOT_STRIDE - CELL_STACK_BEGIN_OFFSET) / CELL_STACK_ENTRY_SIZE
        );
    }

    // ---- §5.5 (per-cell edge fix-up byte-offset constants) ----------

    #[test]
    fn per_cell_edge_byte_offsets_match_spec() {
        // §5.5: `ebx = [esi + 0x24]` then `[esi + 0x28] = eax`.
        assert_eq!(PER_CELL_EDGE_PREV_BR_OFFSET, 0x24);
        assert_eq!(PER_CELL_EDGE_PREV_BR_NEXT_OFFSET, 0x28);
        // The two are adjacent 4-byte DWORDs (read site then write
        // site one DWORD later).
        assert_eq!(
            PER_CELL_EDGE_PREV_BR_NEXT_OFFSET - PER_CELL_EDGE_PREV_BR_OFFSET,
            CELL_STACK_ENTRY_SIZE
        );
    }

    #[test]
    fn per_cell_edge_row_stride_matches_spec_b0() {
        // §5.5: `esi += 0xb0; edi += 0xb0` per row pair iteration.
        assert_eq!(PER_CELL_EDGE_ROW_STRIDE, 0xb0);
    }

    #[test]
    fn per_cell_edge_height_step_matches_spec() {
        // §5.5: `edx -= 4` per loop iteration.
        assert_eq!(PER_CELL_EDGE_HEIGHT_STEP, 4);
    }

    // ---- §5.1 + spec/02 §5 (index arithmetic) -----------------------

    #[test]
    fn cell_stack_slot_offset_at_zero_is_subarray_begin() {
        // Entry 0 sits at the first cell-subarray byte, +0x40.
        assert_eq!(cell_stack_slot_offset(0), Some(0x40));
    }

    #[test]
    fn cell_stack_slot_offset_strides_by_4_bytes() {
        // Entry k sits at +0x40 + 4k.
        for k in 0..16 {
            assert_eq!(cell_stack_slot_offset(k), Some(0x40 + 4 * k));
        }
        // Last valid entry sits at +0x40 + 4*239 = +0x3FC, one DWORD
        // before the slot stride 0x400.
        assert_eq!(
            cell_stack_slot_offset(CELL_STACK_MAX_ENTRIES - 1),
            Some(0x3fc)
        );
    }

    #[test]
    fn cell_stack_slot_offset_rejects_out_of_bounds() {
        // Entry 240 would be at +0x400 = STRIP_SLOT_STRIDE, off the
        // end of this slot. §5 caps the cell-stack at 240 entries.
        assert_eq!(cell_stack_slot_offset(CELL_STACK_MAX_ENTRIES), None);
        assert_eq!(cell_stack_slot_offset(241), None);
        assert_eq!(cell_stack_slot_offset(usize::MAX), None);
    }

    #[test]
    fn cell_stack_slot_offset_is_always_within_slot_stride() {
        // Every valid entry's offset is < STRIP_SLOT_STRIDE.
        for k in 0..CELL_STACK_MAX_ENTRIES {
            let off = cell_stack_slot_offset(k).unwrap();
            assert!(off < STRIP_SLOT_STRIDE);
            assert!(off >= CELL_STACK_BEGIN_OFFSET);
        }
    }

    #[test]
    fn cell_stack_array_offset_combines_slot_and_entry() {
        // Slot 0, entry 0 → +0x40.
        assert_eq!(cell_stack_array_offset(0, 0), Some(0x40));
        // Slot 1, entry 0 → +0x400 + 0x40 = +0x440.
        assert_eq!(cell_stack_array_offset(1, 0), Some(0x440));
        // Slot 3 (Y/primary), entry 5 → 3*0x400 + 0x40 + 20 = 0xc54.
        assert_eq!(cell_stack_array_offset(3, 5), Some(0xc54));
        // Slot 31 (last scratch slot), last valid entry.
        let expected = 31 * STRIP_SLOT_STRIDE + 0x40 + 4 * (CELL_STACK_MAX_ENTRIES - 1);
        assert_eq!(
            cell_stack_array_offset(31, CELL_STACK_MAX_ENTRIES - 1),
            Some(expected)
        );
    }

    #[test]
    fn cell_stack_array_offset_rejects_out_of_range_slot() {
        // Slot 32 is past the 32-slot strip-context array (spec/02
        // §5).
        assert_eq!(cell_stack_array_offset(STRIP_SLOT_COUNT, 0), None);
        assert_eq!(cell_stack_array_offset(usize::MAX, 0), None);
    }

    #[test]
    fn cell_stack_array_offset_rejects_out_of_range_entry() {
        // Slot in range, entry past the 240-entry bound.
        assert_eq!(cell_stack_array_offset(0, CELL_STACK_MAX_ENTRIES), None);
        assert_eq!(cell_stack_array_offset(0, 1000), None);
        // Even a valid slot can't bypass the entry bound.
        assert_eq!(cell_stack_array_offset(5, 240), None);
    }

    #[test]
    fn cell_stack_array_offset_is_within_array_byte_range() {
        // Every valid (slot, entry) maps to a byte offset
        // < STRIP_SLOT_COUNT * STRIP_SLOT_STRIDE
        // (= 32 * 1024 = 32 KiB, the full strip-context array).
        let array_len = STRIP_SLOT_COUNT * STRIP_SLOT_STRIDE;
        for slot in 0..STRIP_SLOT_COUNT {
            for entry in [0usize, 1, 16, 100, CELL_STACK_MAX_ENTRIES - 1] {
                let off = cell_stack_array_offset(slot, entry).unwrap();
                assert!(off < array_len, "slot={} entry={}", slot, entry);
            }
        }
    }

    // ---- §5.3 (read-site enumeration) -------------------------------

    #[test]
    fn read_site_source_slot_top_does_not_branch_to_strip_edge() {
        // §5.3 read at 0x1000656c — used for the entry-time
        // consistency check, not the §5.4/§5.5 fix-up dispatch.
        let site = CellStackReadSite::SourceSlotTop;
        assert!(!site.zero_means_strip_edge());
        assert!(!site.zero_means_mirror_bank());
        assert!(!site.uses_dest_slot_base());
    }

    #[test]
    fn read_site_dest_slot_top_branches_to_strip_edge_on_zero() {
        // §5.3 read at 0x10006ab5 + §5.4/§5.5 dispatch.
        let site = CellStackReadSite::DestSlotTop;
        assert!(site.zero_means_strip_edge());
        assert!(!site.zero_means_mirror_bank());
        assert!(site.uses_dest_slot_base());
    }

    #[test]
    fn read_site_cell_position_probe_selects_mirror_bank_on_zero() {
        // §5.3 read at 0x10006651 + §4.2 bank-view selection.
        let site = CellStackReadSite::CellPositionProbe;
        assert!(!site.zero_means_strip_edge());
        assert!(site.zero_means_mirror_bank());
        assert!(site.uses_dest_slot_base());
    }

    // ---- §5.3 + §5.4 / §5.5 (cell-stack top dispatch) ---------------

    #[test]
    fn dispatch_zero_top_is_strip_edge_branch() {
        let d = CellStackTopDispatch::from_dest_slot_top(0);
        assert_eq!(d, CellStackTopDispatch::StripEdgeFixup);
        assert!(d.is_strip_edge());
        assert!(!d.is_inter_cell());
        assert_eq!(d.cell_data_ptr(), None);
    }

    #[test]
    fn dispatch_nonzero_top_is_inter_cell_branch() {
        let d = CellStackTopDispatch::from_dest_slot_top(0xdead_beef);
        assert_eq!(
            d,
            CellStackTopDispatch::InterCellFixup {
                cell_data_ptr: 0xdead_beef,
            }
        );
        assert!(!d.is_strip_edge());
        assert!(d.is_inter_cell());
        assert_eq!(d.cell_data_ptr(), Some(0xdead_beef));
    }

    #[test]
    fn dispatch_minimum_nonzero_is_inter_cell() {
        // Boundary check: even the smallest non-zero DWORD takes the
        // inter-cell branch.
        let d = CellStackTopDispatch::from_dest_slot_top(1);
        assert!(d.is_inter_cell());
        assert_eq!(d.cell_data_ptr(), Some(1));
    }

    #[test]
    fn dispatch_max_nonzero_is_inter_cell() {
        let d = CellStackTopDispatch::from_dest_slot_top(u32::MAX);
        assert!(d.is_inter_cell());
        assert_eq!(d.cell_data_ptr(), Some(u32::MAX));
    }
}
