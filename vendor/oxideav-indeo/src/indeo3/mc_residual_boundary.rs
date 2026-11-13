//! Indeo 3 spec/05 §5.6 — boundary between the MC fetcher and the
//! VQ residual path.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/05-motion-compensation.md`
//! §5.6 (the two-paragraph disposition that pins the chapter-end of
//! the MC fetcher's inner loop and the chapter-start of the VQ
//! residual path) and cross-references: `spec/04 §3.4` (the
//! per-byte unpacker dispatch entry at `IR32_32.DLL!0x10006bac`),
//! `spec/04 §7.5` (the shared INTER / VQ_DATA leaf-byte table that
//! lets an INTER-flagged cell chain into a VQ_DATA residual via
//! the cell-state byte's chained-flag arithmetic), and `spec/05
//! §5.1` (the MC fetcher inner loop whose final write is the
//! `mov [edi + 0x20c], eax` at `IR32_32.DLL!0x10006732`).
//!
//! Round 14 ([`super::mc_kernel`]) lands the §5.1 / §5.2 inner-loop
//! geometry whose final write this module's [`MC_FETCHER_LAST_WRITE_DST_OFFSET`]
//! pins. Round 20 ([`super::mc_chroma`]) lands the §5.5 chroma-
//! scaling surface that closes the chapter's per-plane disposition.
//! Round 21 (this module) owns §5.6 — the typed surface that pins
//! the chapter boundary itself: the MC chapter ends at the final
//! row-3 write of the inner loop, and the next instruction the
//! spec/06 entropy chapter documents is the per-byte mode read at
//! `IR32_32.DLL!0x10006bac`.
//!
//! This module surfaces:
//!
//! * [`MC_FETCHER_LAST_WRITE_RVA`] — the RVA of the final inner-loop
//!   write (`mov [edi + 0x20c], eax` at
//!   `IR32_32.DLL!0x10006732`).
//! * [`MC_FETCHER_LAST_WRITE_DST_OFFSET`] — the row-3 destination
//!   byte offset (`0x20c`) on the final inner-loop write, equal to
//!   the §5.1 row-3 immediate `0x210` minus the §5.1 `lea edi,
//!   [edi + 0x4]` mid-loop column advance.
//! * [`VQ_RESIDUAL_DISPATCH_RVA`] — the spec/04 §3.4 / §7.5 per-byte
//!   unpacker dispatch entry at `IR32_32.DLL!0x10006bac` where
//!   spec/06 picks up the residual-application chain.
//! * [`MC_CHAPTER_LAST_DST_ROW_INDEX`] — the §5.1 inner loop's
//!   last-written row index (`3`, the fourth row of a 4-row band).
//! * [`MC_INNER_LOOP_BAND_ROWS_ALIAS`] — typed `const`-alias of
//!   [`super::MC_BAND_ROWS`] with a `const _` cross-check that the
//!   final dst row index is exactly `MC_BAND_ROWS - 1`.
//! * [`McCellDisposition`] — typed surface for the §5.6 "what
//!   happens after the MC copy" classification: the cell is either
//!   pure prediction (no residual chains) or prediction-then-
//!   residual (the cell's chained-flag arithmetic per `spec/04
//!   §7.5` re-enters VQ_TREE for the residual).
//! * [`ResidualApplication`] — typed surface for the §5.6 first
//!   paragraph "the residual is added *in place* to the just-written
//!   prediction" disposition (vs the spec/04 §3 direct-intra VQ
//!   path where no MC pre-write exists).
//! * [`McToVqHandoff`] — composite typed surface bundling the MC-
//!   chapter terminator with the spec/06 start point, used by
//!   callers that need to document the chapter boundary at a single
//!   call site.
//! * [`McToVqHandoff::for_disposition`] — returns the typed
//!   handoff matching the cell's [`McCellDisposition`] (the
//!   `PredictionThenResidual` disposition yields the populated
//!   handoff; the `PredictionOnly` disposition yields `None` because
//!   the MC chapter's last write is also the cell's last write in
//!   that case).
//! * [`shares_destination_buffer`] — `const`-`true` predicate
//!   surfacing the §5.6 first-paragraph disposition that the MC
//!   prediction and the VQ residual share the same destination
//!   buffer (the residual is added in place; no copy between two
//!   buffers).
//!
//! What this module **deliberately does not do** (the §5.6 chapter
//! boundary):
//!
//! * It does not perform the MC fetcher inner-loop reads / writes
//!   themselves. The four inner-loop writes are owned by
//!   [`super::mc_kernel`]; this module only pins the disposition
//!   that the last of the four is the chapter terminator.
//! * It does not perform the per-byte mode read at
//!   `IR32_32.DLL!0x10006bac`. That entry point is owned by
//!   [`super::entropy`]'s `0x10006bac` dispatch (per `spec/04 §3.4`,
//!   the mode-byte high-nibble jump table); this module only pins
//!   the RVA at which spec/06's chapter begins.
//! * It does not perform the VQ residual addition itself. The
//!   `add ah, [byte ptr]` site is the spec/06 unpacker territory;
//!   this module only pins the §5.6 first-paragraph disposition
//!   that the addition is in-place over the MC prediction.
//! * It does not classify a cell-state byte as chained or not
//!   chained. That classification is the `spec/04 §7.5`
//!   chained-flag arithmetic territory; this module accepts a
//!   pre-classified [`McCellDisposition`] from the caller.
//! * It does not own the §5.1 inner-loop row layout. The four
//!   per-row destination offsets are owned by
//!   [`super::MC_FULL_PEL_ROW_OFFSETS`]; this module re-uses the
//!   final entry through [`MC_FETCHER_LAST_WRITE_DST_OFFSET`] but
//!   does not re-derive the row layout.

use super::{MC_BAND_ROWS, MC_FULL_PEL_ROW_OFFSETS};

// ---- §5.6 second paragraph: MC-chapter terminator ----------------

/// Spec/05 §5.6 second paragraph — the RVA of the MC fetcher's
/// final inner-loop write (`mov [edi + 0x20c], eax` at
/// `IR32_32.DLL!0x10006732`), the last instruction the MC chapter
/// owns for the cell.
///
/// The next instruction the spec/06 entropy chapter documents is
/// the per-byte mode read at [`VQ_RESIDUAL_DISPATCH_RVA`] when the
/// cell's chained-flag arithmetic (`spec/04 §7.5`) re-enters
/// VQ_TREE for a residual; otherwise the cell's traversal moves on
/// to the next binary-tree node and the §5.6 chapter boundary is
/// implicit.
pub const MC_FETCHER_LAST_WRITE_RVA: u32 = 0x1000_6732;

/// Spec/05 §5.6 second paragraph — the destination byte offset on
/// the MC fetcher's final inner-loop write (`mov [edi + 0x20c],
/// eax`).
///
/// Equal to the §5.1 row-3 source-read immediate `0x210` minus the
/// `lea edi, [edi + 0x4]` mid-loop column advance the inner loop
/// performs between the row-1 store and the row-2 / row-3 stores.
/// Surfacing this as a named constant lets a caller cross-reference
/// the §5.6 chapter terminator against the §5.1 row-offset
/// constants without re-deriving the `lea` adjustment.
pub const MC_FETCHER_LAST_WRITE_DST_OFFSET: u32 = 0x20c;

/// Spec/05 §5.6 / §5.1 — the row index of the MC fetcher's
/// final inner-loop write. The inner loop processes four rows
/// per iteration (`spec/05 §5.1`'s 4-row band), and the final
/// write is the fourth row at row index `3`.
///
/// `const _`-cross-checked against [`MC_INNER_LOOP_BAND_ROWS_ALIAS`]
/// (`= MC_BAND_ROWS`) so any future change to the band height would
/// flag this constant for review.
pub const MC_CHAPTER_LAST_DST_ROW_INDEX: u32 = 3;

/// Spec/05 §5.6 typed alias of [`super::MC_BAND_ROWS`] (`= 4`),
/// surfacing the §5.1 band height as the upper bound of the
/// §5.6 last-write row index.
pub const MC_INNER_LOOP_BAND_ROWS_ALIAS: u32 = MC_BAND_ROWS as u32;

/// `const _` cross-check that the §5.6 last-write row index is
/// exactly the band-row count minus one (i.e. the §5.1 band's
/// fourth and last row).
const _: () = assert!(MC_CHAPTER_LAST_DST_ROW_INDEX + 1 == MC_INNER_LOOP_BAND_ROWS_ALIAS);

/// `const _` cross-check that the §5.6 last-write destination
/// byte offset agrees with the §5.1 row-3 entry of
/// [`super::MC_FULL_PEL_ROW_OFFSETS`] minus the §5.1 mid-loop
/// `lea edi, [edi + 0x4]` column advance.
const _: () = {
    let row3: usize = MC_FULL_PEL_ROW_OFFSETS[MC_CHAPTER_LAST_DST_ROW_INDEX as usize];
    let lea_adjustment: usize = 0x4;
    assert!(row3 - lea_adjustment == MC_FETCHER_LAST_WRITE_DST_OFFSET as usize);
};

// ---- §5.6 first paragraph: spec/06 chapter start point -----------

/// Spec/05 §5.6 first paragraph — the RVA of the per-byte mode
/// read at `IR32_32.DLL!0x10006bac` where the spec/06 entropy
/// chapter picks up after the MC chapter's terminator.
///
/// Per `spec/04 §3.4`, this is the per-cell unpacker dispatch
/// entry; per `spec/04 §7.5`, the entry is reached only when the
/// cell's chained-flag arithmetic re-enters VQ_TREE for a residual
/// (otherwise the binary-tree walker advances to the next node
/// without entering the unpacker).
pub const VQ_RESIDUAL_DISPATCH_RVA: u32 = 0x1000_6bac;

/// `const _` cross-check that the spec/06 chapter starts strictly
/// after the MC chapter terminates (the [`MC_FETCHER_LAST_WRITE_RVA`]
/// comes before [`VQ_RESIDUAL_DISPATCH_RVA`] in code memory).
const _: () = assert!(MC_FETCHER_LAST_WRITE_RVA < VQ_RESIDUAL_DISPATCH_RVA);

// ---- §5.6 first paragraph: in-place residual disposition ----------

/// Spec/05 §5.6 first paragraph — `const`-`true` predicate
/// surfacing the disposition that the MC prediction and the VQ
/// residual share the same destination buffer (the residual is
/// added in place over the just-written prediction; no copy
/// between two buffers, no per-cell intermediate).
pub const fn shares_destination_buffer() -> bool {
    true
}

// ---- §5.6 typed surface: cell disposition ------------------------

/// Spec/05 §5.6 typed surface enum for the cell's "what happens
/// after the MC copy" classification.
///
/// The §5.6 first paragraph distinguishes two cases:
///
/// * [`McCellDisposition::PredictionOnly`] — the cell's
///   chained-flag arithmetic (`spec/04 §7.5`) does *not* re-enter
///   VQ_TREE. The MC fetcher's inner loop writes the cell's pixel
///   data, the binary-tree walker advances to the next node, and
///   no per-cell residual is applied. The chapter terminator at
///   [`MC_FETCHER_LAST_WRITE_RVA`] is also the cell's terminator.
/// * [`McCellDisposition::PredictionThenResidual`] — the cell is
///   INTER-flagged at the MC_TREE level but the chained-flag
///   arithmetic re-enters VQ_TREE (per `spec/04 §7.5`'s shared
///   INTER / VQ_DATA leaf-byte table). After the MC fetcher writes
///   the prediction, the spec/06 unpacker entry at
///   [`VQ_RESIDUAL_DISPATCH_RVA`] adds the VQ residual in place
///   over the just-written prediction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum McCellDisposition {
    /// Pure-prediction INTER cell — no residual chains; the MC
    /// fetcher's inner loop is the cell's last write.
    PredictionOnly,
    /// INTER cell whose chained-flag arithmetic re-enters VQ_TREE
    /// for a residual — the spec/06 unpacker adds the residual in
    /// place over the MC prediction.
    PredictionThenResidual,
}

impl McCellDisposition {
    /// Returns whether this disposition triggers a residual
    /// application after the MC fetcher's final write.
    pub const fn requires_residual(self) -> bool {
        matches!(self, McCellDisposition::PredictionThenResidual)
    }

    /// Returns the disposition's corresponding [`ResidualApplication`]
    /// flag.
    pub const fn residual_application(self) -> ResidualApplication {
        match self {
            McCellDisposition::PredictionOnly => ResidualApplication::None,
            McCellDisposition::PredictionThenResidual => ResidualApplication::InPlaceOverPrediction,
        }
    }
}

// ---- §5.6 typed surface: residual-application disposition --------

/// Spec/05 §5.6 first paragraph — typed surface for the "how is
/// the residual applied" classification.
///
/// The §5.6 disposition is that, when a residual exists, it is
/// added *in place* to the prediction buffer (no per-cell
/// intermediate copy). When no residual exists (the
/// [`McCellDisposition::PredictionOnly`] case), the
/// [`ResidualApplication::None`] variant carries that absence.
///
/// The variant is separate from the [`McCellDisposition`] enum
/// because callers consuming the §5.6 first-paragraph surface
/// (e.g. an audit walker) may want to distinguish "no residual,
/// just MC prediction" from "MC prediction followed by an
/// in-place residual" without re-deriving the cell-state byte's
/// chained-flag arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResidualApplication {
    /// No residual application — the MC fetcher's final write is
    /// the cell's terminal write.
    None,
    /// In-place residual addition over the just-written MC
    /// prediction; the prediction and the residual share the same
    /// destination buffer per the §5.6 first paragraph.
    InPlaceOverPrediction,
}

impl ResidualApplication {
    /// Returns whether the residual application is "none" (the
    /// pure-prediction disposition).
    pub const fn is_none(self) -> bool {
        matches!(self, ResidualApplication::None)
    }

    /// Returns whether the residual application is the §5.6
    /// in-place addition over the prediction.
    pub const fn is_in_place(self) -> bool {
        matches!(self, ResidualApplication::InPlaceOverPrediction)
    }
}

// ---- §5.6 composite handoff surface -------------------------------

/// Spec/05 §5.6 composite handoff surface — bundles the MC-chapter
/// terminator RVA with the spec/06 start RVA, used by callers that
/// need to document the chapter boundary at a single call site.
///
/// Constructed via [`McToVqHandoff::for_disposition`] from a
/// pre-classified [`McCellDisposition`]; the
/// [`McCellDisposition::PredictionOnly`] case returns `None` (the
/// MC chapter's last write is also the cell's last write, so no
/// handoff to spec/06 occurs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct McToVqHandoff {
    /// The MC fetcher's final inner-loop write RVA — the last
    /// instruction the MC chapter owns for the cell.
    pub mc_terminator_rva: u32,
    /// The spec/06 entry RVA — the per-byte mode read at
    /// `IR32_32.DLL!0x10006bac` where the residual path begins.
    pub vq_residual_dispatch_rva: u32,
}

impl McToVqHandoff {
    /// Returns the §5.6 handoff for a cell whose disposition is
    /// [`McCellDisposition::PredictionThenResidual`]; returns
    /// `None` for [`McCellDisposition::PredictionOnly`] because
    /// the MC chapter's terminator is also the cell's terminator
    /// in that case (no handoff to spec/06).
    pub const fn for_disposition(disposition: McCellDisposition) -> Option<Self> {
        match disposition {
            McCellDisposition::PredictionOnly => None,
            McCellDisposition::PredictionThenResidual => Some(Self {
                mc_terminator_rva: MC_FETCHER_LAST_WRITE_RVA,
                vq_residual_dispatch_rva: VQ_RESIDUAL_DISPATCH_RVA,
            }),
        }
    }

    /// Returns the RVA delta between the spec/06 entry and the MC
    /// chapter terminator. Positive by construction (the
    /// [`const _` cross-check at module level pins
    /// `MC_FETCHER_LAST_WRITE_RVA < VQ_RESIDUAL_DISPATCH_RVA`]).
    pub const fn rva_delta(self) -> u32 {
        self.vq_residual_dispatch_rva - self.mc_terminator_rva
    }
}

#[cfg(test)]
mod tests {
    use super::super::{MC_BAND_ROWS, MC_FULL_PEL_ROW_OFFSETS, McDispatchMode, PackedMv};
    use super::*;

    // ---- MC_FETCHER_LAST_WRITE_RVA -------------------------------

    #[test]
    fn mc_fetcher_last_write_rva_matches_spec() {
        // §5.6 second paragraph: the RVA `IR32_32.DLL!0x10006732`
        // is the final `mov [edi + 0x20c], eax` of the inner loop.
        assert_eq!(MC_FETCHER_LAST_WRITE_RVA, 0x1000_6732);
    }

    #[test]
    fn mc_fetcher_last_write_rva_falls_inside_inner_loop_range() {
        // §5.1 spec text places the inner loop at
        // `0x1000670d`..`0x1000673d`. The terminator must lie
        // within that range. Use `black_box` so the compile-time
        // value does not collapse to a tautological assertion.
        let rva = core::hint::black_box(MC_FETCHER_LAST_WRITE_RVA);
        let lo = core::hint::black_box(0x1000_670d_u32);
        let hi = core::hint::black_box(0x1000_673d_u32);
        assert!(rva >= lo);
        assert!(rva <= hi);
    }

    // ---- MC_FETCHER_LAST_WRITE_DST_OFFSET -------------------------

    #[test]
    fn mc_fetcher_last_write_dst_offset_matches_spec() {
        // §5.1 inner loop: the row-3 store uses `[edi + 0x20c]`
        // after the mid-loop `lea edi, [edi + 0x4]`.
        assert_eq!(MC_FETCHER_LAST_WRITE_DST_OFFSET, 0x20c);
    }

    #[test]
    fn mc_fetcher_last_write_dst_offset_equals_row3_minus_lea() {
        // §5.1: the row-3 source-read uses `[esi + 0x210]`; the
        // dst-write uses `[edi + 0x20c]` because the inner loop
        // does `lea edi, [edi + 0x4]` between row-1's store and
        // row-2 / row-3's stores. Verify the bookkeeping.
        let row3_no_lea: usize = MC_FULL_PEL_ROW_OFFSETS[MC_CHAPTER_LAST_DST_ROW_INDEX as usize];
        let lea_adjustment: usize = 0x4;
        assert_eq!(
            row3_no_lea - lea_adjustment,
            MC_FETCHER_LAST_WRITE_DST_OFFSET as usize
        );
    }

    // ---- MC_CHAPTER_LAST_DST_ROW_INDEX ----------------------------

    #[test]
    fn mc_chapter_last_dst_row_index_is_band_height_minus_one() {
        // §5.1 inner loop emits four rows per iteration; the last
        // is row index 3.
        assert_eq!(MC_CHAPTER_LAST_DST_ROW_INDEX + 1, MC_BAND_ROWS as u32);
    }

    #[test]
    fn mc_chapter_last_dst_row_index_aliases_to_band_rows_alias() {
        assert_eq!(MC_INNER_LOOP_BAND_ROWS_ALIAS, MC_BAND_ROWS as u32);
    }

    // ---- VQ_RESIDUAL_DISPATCH_RVA ---------------------------------

    #[test]
    fn vq_residual_dispatch_rva_matches_spec() {
        // §5.6 first paragraph + `spec/04 §3.4`: the per-byte
        // unpacker entry sits at `IR32_32.DLL!0x10006bac`.
        assert_eq!(VQ_RESIDUAL_DISPATCH_RVA, 0x1000_6bac);
    }

    #[test]
    fn vq_residual_dispatch_rva_strictly_after_mc_terminator() {
        // Use `black_box` so the const ordering is checked at run
        // time rather than collapsed into a tautological assertion.
        let mc = core::hint::black_box(MC_FETCHER_LAST_WRITE_RVA);
        let vq = core::hint::black_box(VQ_RESIDUAL_DISPATCH_RVA);
        assert!(mc < vq);
    }

    #[test]
    fn vq_residual_dispatch_rva_delta_is_nonzero() {
        let delta = VQ_RESIDUAL_DISPATCH_RVA - MC_FETCHER_LAST_WRITE_RVA;
        assert!(delta > 0);
        // The chapter boundary is more than a handful of bytes;
        // the §5.6 second paragraph indicates an entire
        // intervening section of binary code between the MC inner
        // loop and the spec/06 unpacker.
        assert!(delta >= 0x100);
    }

    // ---- shares_destination_buffer --------------------------------

    #[test]
    fn shares_destination_buffer_pins_in_place_disposition() {
        // §5.6 first paragraph: the residual is added in place over
        // the prediction; the prediction and residual share the
        // same destination buffer.
        assert!(shares_destination_buffer());
    }

    // ---- McCellDisposition predicates ----------------------------

    #[test]
    fn prediction_only_does_not_require_residual() {
        assert!(!McCellDisposition::PredictionOnly.requires_residual());
    }

    #[test]
    fn prediction_then_residual_requires_residual() {
        assert!(McCellDisposition::PredictionThenResidual.requires_residual());
    }

    #[test]
    fn cell_disposition_residual_application_mapping() {
        assert_eq!(
            McCellDisposition::PredictionOnly.residual_application(),
            ResidualApplication::None
        );
        assert_eq!(
            McCellDisposition::PredictionThenResidual.residual_application(),
            ResidualApplication::InPlaceOverPrediction
        );
    }

    #[test]
    fn cell_disposition_variants_are_distinct() {
        assert_ne!(
            McCellDisposition::PredictionOnly,
            McCellDisposition::PredictionThenResidual
        );
    }

    // ---- ResidualApplication predicates ---------------------------

    #[test]
    fn residual_application_none_is_none() {
        assert!(ResidualApplication::None.is_none());
        assert!(!ResidualApplication::None.is_in_place());
    }

    #[test]
    fn residual_application_in_place_is_in_place() {
        assert!(!ResidualApplication::InPlaceOverPrediction.is_none());
        assert!(ResidualApplication::InPlaceOverPrediction.is_in_place());
    }

    #[test]
    fn residual_application_variants_are_distinct() {
        assert_ne!(
            ResidualApplication::None,
            ResidualApplication::InPlaceOverPrediction
        );
    }

    // ---- McToVqHandoff::for_disposition ---------------------------

    #[test]
    fn handoff_prediction_only_returns_none() {
        assert!(McToVqHandoff::for_disposition(McCellDisposition::PredictionOnly).is_none());
    }

    #[test]
    fn handoff_prediction_then_residual_returns_populated() {
        let handoff =
            McToVqHandoff::for_disposition(McCellDisposition::PredictionThenResidual).unwrap();
        assert_eq!(handoff.mc_terminator_rva, MC_FETCHER_LAST_WRITE_RVA);
        assert_eq!(handoff.vq_residual_dispatch_rva, VQ_RESIDUAL_DISPATCH_RVA);
    }

    #[test]
    fn handoff_rva_delta_is_positive_and_matches_constants() {
        let handoff =
            McToVqHandoff::for_disposition(McCellDisposition::PredictionThenResidual).unwrap();
        assert_eq!(
            handoff.rva_delta(),
            VQ_RESIDUAL_DISPATCH_RVA - MC_FETCHER_LAST_WRITE_RVA
        );
        assert!(handoff.rva_delta() > 0);
    }

    #[test]
    fn handoff_struct_is_copy() {
        let h = McToVqHandoff::for_disposition(McCellDisposition::PredictionThenResidual).unwrap();
        let h2 = h; // Copy
        assert_eq!(h, h2);
    }

    // ---- Cross-module sanity --------------------------------------

    #[test]
    fn mc_terminator_offset_uses_existing_row_offset_table() {
        // Sanity: the §5.6 chapter terminator's dst offset is
        // recoverable from the existing `MC_FULL_PEL_ROW_OFFSETS`
        // table without re-deriving the offsets.
        let row3: usize = MC_FULL_PEL_ROW_OFFSETS[3];
        assert_eq!(row3, 0x210);
        // After the `lea edi, [edi + 0x4]`, the `[edi + 0x20c]`
        // store reaches the same memory.
        assert_eq!(row3 - 0x4, MC_FETCHER_LAST_WRITE_DST_OFFSET as usize);
    }

    #[test]
    fn mc_terminator_applies_to_all_dispatch_modes() {
        // §5.6 chapter boundary applies regardless of which §2.2
        // four-way dispatch mode was taken — all four MC paths
        // emit a final inner-loop write before relinquishing the
        // cell. Construct a PackedMv for each mode and verify the
        // mode field is recoverable (the §5.6 surface itself is
        // mode-agnostic; this is a sanity assertion that the §2.2
        // four-way fork does not affect the §5.6 chapter end).
        let modes = [
            McDispatchMode::FullPel,
            McDispatchMode::VerticalHalfPel,
            McDispatchMode::HorizontalHalfPel,
            McDispatchMode::BothHalfPel,
        ];
        for m in modes {
            // Construct a raw word with this mode in the low 2
            // bits; the chapter terminator is independent of mode.
            let raw: u32 = m as u32;
            let mv = PackedMv::from_raw(raw);
            assert_eq!(mv.mode(), m);
            // §5.6 surface stays constant regardless of which
            // mode was selected.
            assert_eq!(MC_FETCHER_LAST_WRITE_RVA, 0x1000_6732);
        }
    }

    #[test]
    fn handoff_round_trip_over_both_dispositions() {
        // Every well-formed McCellDisposition value either yields
        // None (PredictionOnly) or a populated handoff
        // (PredictionThenResidual) whose constants match the
        // module's two RVAs exactly.
        for d in [
            McCellDisposition::PredictionOnly,
            McCellDisposition::PredictionThenResidual,
        ] {
            match McToVqHandoff::for_disposition(d) {
                None => assert_eq!(d, McCellDisposition::PredictionOnly),
                Some(h) => {
                    assert_eq!(d, McCellDisposition::PredictionThenResidual);
                    assert_eq!(h.mc_terminator_rva, MC_FETCHER_LAST_WRITE_RVA);
                    assert_eq!(h.vq_residual_dispatch_rva, VQ_RESIDUAL_DISPATCH_RVA);
                }
            }
        }
    }

    #[test]
    fn residual_application_distinguishes_chapter_boundary_paths() {
        // The §5.6 first paragraph splits the post-MC chain into
        // exactly two paths: the chapter ends here (None) or the
        // chapter ends here and spec/06 picks up with an in-place
        // residual (InPlaceOverPrediction). No third path exists.
        let variants = [
            ResidualApplication::None,
            ResidualApplication::InPlaceOverPrediction,
        ];
        // Verify the typed surface has exactly two variants by
        // round-tripping each one.
        for v in variants {
            assert!(v.is_none() ^ v.is_in_place());
        }
    }
}
