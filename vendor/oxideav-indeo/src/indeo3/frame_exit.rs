//! Indeo 3 spec/02 §6.2 — per-frame plane-iteration terminator and
//! output-reconstruction handoff.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/02-picture-layer.md`
//! §6 (the per-plane decode call at `IR32_32.DLL!0x10006538`, its
//! call site at `IR32_32.DLL!0x10004637`, and the `ret 0x1c`
//! seven-argument cdecl stack cleanup at
//! `IR32_32.DLL!0x10006b94`), §6.1 (the per-plane exit established
//! by the binary-tree walk's natural termination), §6.2 (the
//! per-frame exit: after the count-down loop visits all three planes
//! `plane_idx ∈ {2, 1, 0}`, control proceeds to the output-
//! reconstruction stage at `IR32_32.DLL!0x10004644`), and §8 (the
//! `for plane_idx in [2, 1, 0]` plane-iteration summary).
//!
//! Round 8 ([`super::strip_context::PlaneDecodeStatus`]) lands the
//! §6 *per-plane* terminal-status classifier (the `eax` value the
//! per-plane decoder returns: `0` success / `3` malformed). This
//! module owns the *per-frame* layer above it: the disposition that
//! folds the three per-plane statuses into one frame-level outcome
//! — proceed to output-reconstruction when every present plane
//! returned `Ok`, or raise an end-of-frame fault when any plane
//! returned `Malformed` (`IR32_32.DLL!0x10006ba2..0x10006baa`
//! returns the §6 status `3`). The fold short-circuits on the first
//! faulting plane in §8 iteration order, matching the outer parser's
//! "treat any non-zero plane status as an end-of-frame fault" chain.
//!
//! This module surfaces:
//!
//! * [`PLANE_ITERATION_ORDER`] — the §8 `[2, 1, 0]` (U, V, Y)
//!   count-down loop order, with a `const _` cross-check that it is
//!   a permutation of `0..PLANE_COUNT`.
//! * [`PER_PLANE_DECODE_CALL_SITE_RVA`] — the §6 call site
//!   (`IR32_32.DLL!0x10004637`) that invokes the per-plane decoder.
//! * [`PER_PLANE_DECODE_ENTRY_RVA`] — the §6 per-plane decoder
//!   entry (`IR32_32.DLL!0x10006538`).
//! * [`PER_PLANE_DECODE_RET_RVA`] / [`PER_PLANE_DECODE_RET_CLEANUP_BYTES`]
//!   — the §6 `ret 0x1c` at `IR32_32.DLL!0x10006b94` and its
//!   seven-argument cdecl callee stack-cleanup byte count (`0x1c` =
//!   `7 * 4`), with a `const _` cross-check against the §6 argument
//!   count.
//! * [`FRAME_OUTPUT_RECONSTRUCTION_RVA`] — the §6.2 handoff target
//!   (`IR32_32.DLL!0x10004644`) the per-frame parser proceeds to
//!   after the plane loop.
//! * [`FRAME_FAULT_RETURN_RVA`] — the §6 / §6.2 end-of-frame fault
//!   path (`IR32_32.DLL!0x10006ba2`) that returns the §6 status `3`.
//! * [`FrameExitDisposition`] — the typed per-frame outcome
//!   (`ProceedToReconstruction` / `EndOfFrameFault`).
//! * [`FramePlaneStatusFold`] — the typed fold of per-plane statuses
//!   in §8 iteration order into a [`FrameExitDisposition`], carrying
//!   the §8-order index of the first faulting plane.
//!
//! Per the §6 chapter boundary, this module deliberately does not
//! perform the per-plane binary-tree walk itself (owned by the
//! spec/03 macroblock layer), does not classify a single plane's
//! `eax` (owned by [`super::strip_context::PlaneDecodeStatus`]), does
//! not own the §6.1 per-plane payload byte budget (owned by
//! [`super::PlaneByteMap`]), and does not perform the output-
//! reconstruction stage the §6.2 handoff targets (deferred to
//! `spec/07-output-reconstruction.md`).

use super::PLANE_COUNT;
use super::strip_context::{PLANE_DECODE_STATUS_MALFORMED, PlaneDecodeStatus};

/// Spec/02 §8 — the per-frame plane-iteration order, a count-down
/// over `plane_idx ∈ {2, 1, 0}` (U, then V, then Y).
///
/// The reference decoder's outer loop decrements `plane_idx` from
/// `2` to `0`, so the iteration visits the U plane first and the Y
/// plane last. The §6.2 per-frame exit is reached once this loop has
/// visited all three entries (whether or not a given plane was
/// present per the §2 range check).
pub const PLANE_ITERATION_ORDER: [usize; PLANE_COUNT] = [2, 1, 0];

// The §8 iteration order must be a permutation of every plane index
// `0..PLANE_COUNT`; otherwise a plane would be visited twice or
// skipped entirely. Sum-of-indices is a sufficient permutation check
// for a fixed three-element array of distinct in-range values, paired
// with the distinctness check below.
const _: () = assert!(
    PLANE_ITERATION_ORDER[0] + PLANE_ITERATION_ORDER[1] + PLANE_ITERATION_ORDER[2]
        == (PLANE_COUNT - 1) + (PLANE_COUNT - 2) + (PLANE_COUNT - 3)
);
const _: () = assert!(
    PLANE_ITERATION_ORDER[0] != PLANE_ITERATION_ORDER[1]
        && PLANE_ITERATION_ORDER[1] != PLANE_ITERATION_ORDER[2]
        && PLANE_ITERATION_ORDER[0] != PLANE_ITERATION_ORDER[2]
);
const _: () = assert!(
    PLANE_ITERATION_ORDER[0] < PLANE_COUNT
        && PLANE_ITERATION_ORDER[1] < PLANE_COUNT
        && PLANE_ITERATION_ORDER[2] < PLANE_COUNT
);

/// Spec/02 §6 — the call site (`IR32_32.DLL!0x10004637`) at which
/// the per-frame parser invokes the per-plane decoder.
pub const PER_PLANE_DECODE_CALL_SITE_RVA: u32 = 0x1000_4637;

/// Spec/02 §6 — the per-plane decoder entry point
/// (`IR32_32.DLL!0x10006538`) the call site dispatches into.
pub const PER_PLANE_DECODE_ENTRY_RVA: u32 = 0x1000_6538;

/// Spec/02 §6 — the RVA of the per-plane decoder's `ret 0x1c`
/// (`IR32_32.DLL!0x10006b94`).
pub const PER_PLANE_DECODE_RET_RVA: u32 = 0x1000_6b94;

/// Spec/02 §6 — the number of bytes the per-plane decoder's
/// `ret 0x1c` cleans off the stack (`0x1c` = 28 = `7 * 4`), one DWORD
/// per cdecl argument in the §6 seven-argument call frame.
pub const PER_PLANE_DECODE_RET_CLEANUP_BYTES: u32 = 0x1c;

/// Spec/02 §6 — the number of arguments in the per-plane decode call
/// frame (the §6 push table has seven rows). Surfaced so the
/// `ret 0x1c` cleanup count can be cross-checked against the argument
/// count.
pub const PER_PLANE_DECODE_ARG_COUNT: u32 = 7;

// The `ret 0x1c` callee-cleanup byte count must equal one DWORD per
// cdecl argument: 7 arguments × 4 bytes = 0x1c.
const _: () = assert!(PER_PLANE_DECODE_RET_CLEANUP_BYTES == PER_PLANE_DECODE_ARG_COUNT * 4);

/// Spec/02 §6.2 — the output-reconstruction stage entry
/// (`IR32_32.DLL!0x10004644`) the per-frame parser proceeds to after
/// the §8 plane loop completes without a fault.
pub const FRAME_OUTPUT_RECONSTRUCTION_RVA: u32 = 0x1000_4644;

/// Spec/02 §6 / §6.2 — the end-of-frame fault path
/// (`IR32_32.DLL!0x10006ba2`) that returns the §6 status `3`
/// (`PLANE_DECODE_STATUS_MALFORMED`) up to the per-frame parser.
pub const FRAME_FAULT_RETURN_RVA: u32 = 0x1000_6ba2;

// The §6.2 handoff target follows the §6 call site in code memory;
// the call site dispatches the per-plane decoder, and only after the
// plane loop completes does control reach the reconstruction stage.
const _: () = assert!(FRAME_OUTPUT_RECONSTRUCTION_RVA > PER_PLANE_DECODE_CALL_SITE_RVA);

/// Spec/02 §6.2 — the per-frame outcome after the §8 plane-iteration
/// loop completes.
///
/// The reference decoder either falls through to the output-
/// reconstruction stage at [`FRAME_OUTPUT_RECONSTRUCTION_RVA`] (every
/// present plane decoded cleanly) or takes the end-of-frame fault at
/// [`FRAME_FAULT_RETURN_RVA`] returning the §6 status
/// [`PLANE_DECODE_STATUS_MALFORMED`] (`3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameExitDisposition {
    /// Every plane returned [`PlaneDecodeStatus::Ok`]; control
    /// proceeds to the §6.2 output-reconstruction stage at
    /// [`FRAME_OUTPUT_RECONSTRUCTION_RVA`].
    ProceedToReconstruction,
    /// At least one plane returned [`PlaneDecodeStatus::Malformed`];
    /// the per-frame parser takes the §6 end-of-frame fault at
    /// [`FRAME_FAULT_RETURN_RVA`] and returns the §6 status `3`.
    EndOfFrameFault,
}

impl FrameExitDisposition {
    /// True iff the disposition is [`ProceedToReconstruction`].
    ///
    /// [`ProceedToReconstruction`]: FrameExitDisposition::ProceedToReconstruction
    pub fn proceeds_to_reconstruction(self) -> bool {
        matches!(self, FrameExitDisposition::ProceedToReconstruction)
    }

    /// True iff the disposition is [`EndOfFrameFault`].
    ///
    /// [`EndOfFrameFault`]: FrameExitDisposition::EndOfFrameFault
    pub fn is_fault(self) -> bool {
        matches!(self, FrameExitDisposition::EndOfFrameFault)
    }

    /// Spec/02 §6.2 — the RVA control reaches for this disposition:
    /// [`FRAME_OUTPUT_RECONSTRUCTION_RVA`] on success or
    /// [`FRAME_FAULT_RETURN_RVA`] on the end-of-frame fault.
    pub fn target_rva(self) -> u32 {
        match self {
            FrameExitDisposition::ProceedToReconstruction => FRAME_OUTPUT_RECONSTRUCTION_RVA,
            FrameExitDisposition::EndOfFrameFault => FRAME_FAULT_RETURN_RVA,
        }
    }

    /// Spec/02 §6 — the integer status the per-frame parser ultimately
    /// returns for this disposition: `0`
    /// ([`super::strip_context::PLANE_DECODE_STATUS_OK`]) on success,
    /// `3` ([`PLANE_DECODE_STATUS_MALFORMED`]) on the fault.
    pub fn frame_status(self) -> i32 {
        match self {
            FrameExitDisposition::ProceedToReconstruction => {
                super::strip_context::PLANE_DECODE_STATUS_OK
            }
            FrameExitDisposition::EndOfFrameFault => PLANE_DECODE_STATUS_MALFORMED,
        }
    }
}

/// Spec/02 §6.2 / §8 — the fold of the three per-plane decode
/// statuses (in §8 `[2, 1, 0]` iteration order) into one per-frame
/// [`FrameExitDisposition`].
///
/// The fold short-circuits on the first plane that returned
/// [`PlaneDecodeStatus::Malformed`], matching the outer parser's
/// "any non-zero plane status is an end-of-frame fault" chain (the
/// per-plane decoder does not return until it has walked its entire
/// plane, so the loop never advances past a faulting plane). The
/// fold records the §8-order index (`0..PLANE_COUNT`) of the first
/// faulting plane so callers can report which plane raised the fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramePlaneStatusFold {
    /// The resolved per-frame disposition.
    pub disposition: FrameExitDisposition,
    /// The position within [`PLANE_ITERATION_ORDER`] (`0..PLANE_COUNT`)
    /// of the first plane that faulted, or `None` when no plane
    /// faulted. This is the §8 iteration index, **not** the
    /// `plane_idx`; the faulting `plane_idx` is
    /// `PLANE_ITERATION_ORDER[iteration_index]`.
    pub first_fault_iteration_index: Option<usize>,
}

impl FramePlaneStatusFold {
    /// Spec/02 §6.2 / §8 — fold the per-plane statuses, supplied in
    /// [`PLANE_ITERATION_ORDER`] order (entry `0` is the U plane, the
    /// first plane visited; entry `2` is the Y plane, the last), into
    /// the per-frame disposition.
    pub fn from_iteration_order(statuses: [PlaneDecodeStatus; PLANE_COUNT]) -> Self {
        let mut first_fault_iteration_index = None;
        for (iteration_index, status) in statuses.iter().enumerate() {
            if !status.is_ok() {
                first_fault_iteration_index = Some(iteration_index);
                break;
            }
        }
        let disposition = match first_fault_iteration_index {
            None => FrameExitDisposition::ProceedToReconstruction,
            Some(_) => FrameExitDisposition::EndOfFrameFault,
        };
        Self {
            disposition,
            first_fault_iteration_index,
        }
    }

    /// Spec/02 §6.2 / §8 — fold the per-plane statuses supplied in
    /// `plane_idx` order (entry `0` is Y, entry `1` is V, entry `2`
    /// is U) into the per-frame disposition, re-ordering them into
    /// §8 iteration order first.
    ///
    /// The resolved [`disposition`](Self::disposition) is independent
    /// of the supplied ordering (the fault is order-agnostic), but
    /// the [`first_fault_iteration_index`](Self::first_fault_iteration_index)
    /// reflects §8 visitation order, so this constructor maps the
    /// `plane_idx`-ordered input through [`PLANE_ITERATION_ORDER`]
    /// before folding.
    pub fn from_plane_idx_order(statuses_by_plane_idx: [PlaneDecodeStatus; PLANE_COUNT]) -> Self {
        let in_iteration_order = [
            statuses_by_plane_idx[PLANE_ITERATION_ORDER[0]],
            statuses_by_plane_idx[PLANE_ITERATION_ORDER[1]],
            statuses_by_plane_idx[PLANE_ITERATION_ORDER[2]],
        ];
        Self::from_iteration_order(in_iteration_order)
    }

    /// Spec/02 §8 — the `plane_idx` of the first faulting plane (the
    /// value of [`PLANE_ITERATION_ORDER`] at
    /// [`first_fault_iteration_index`](Self::first_fault_iteration_index)),
    /// or `None` when the frame proceeded to reconstruction.
    pub fn first_fault_plane_idx(&self) -> Option<usize> {
        self.first_fault_iteration_index
            .map(|i| PLANE_ITERATION_ORDER[i])
    }

    /// True iff the fold resolved to
    /// [`FrameExitDisposition::ProceedToReconstruction`].
    pub fn proceeds_to_reconstruction(&self) -> bool {
        self.disposition.proceeds_to_reconstruction()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::strip_context::PLANE_DECODE_STATUS_OK;
    use crate::indeo3::{PLANE_IDX_U, PLANE_IDX_V, PLANE_IDX_Y};

    // ---- §8 iteration order ------------------------------------------

    #[test]
    fn iteration_order_is_u_v_y_countdown() {
        assert_eq!(PLANE_ITERATION_ORDER, [2, 1, 0]);
        // The §2 plane-index constants: U = 2, V = 1, Y = 0.
        assert_eq!(PLANE_ITERATION_ORDER[0], PLANE_IDX_U);
        assert_eq!(PLANE_ITERATION_ORDER[1], PLANE_IDX_V);
        assert_eq!(PLANE_ITERATION_ORDER[2], PLANE_IDX_Y);
    }

    #[test]
    fn iteration_order_is_a_permutation_of_all_planes() {
        let mut seen = [false; PLANE_COUNT];
        for &idx in &PLANE_ITERATION_ORDER {
            assert!(idx < PLANE_COUNT);
            assert!(!seen[idx], "plane {idx} visited twice");
            seen[idx] = true;
        }
        assert!(seen.iter().all(|&s| s), "not every plane visited");
    }

    // ---- §6 RVA / cleanup constants ----------------------------------

    #[test]
    fn rva_constants_match_spec() {
        assert_eq!(PER_PLANE_DECODE_CALL_SITE_RVA, 0x1000_4637);
        assert_eq!(PER_PLANE_DECODE_ENTRY_RVA, 0x1000_6538);
        assert_eq!(PER_PLANE_DECODE_RET_RVA, 0x1000_6b94);
        assert_eq!(FRAME_OUTPUT_RECONSTRUCTION_RVA, 0x1000_4644);
        assert_eq!(FRAME_FAULT_RETURN_RVA, 0x1000_6ba2);
    }

    #[test]
    fn ret_cleanup_is_one_dword_per_argument() {
        assert_eq!(PER_PLANE_DECODE_RET_CLEANUP_BYTES, 0x1c);
        assert_eq!(PER_PLANE_DECODE_ARG_COUNT, 7);
        assert_eq!(
            PER_PLANE_DECODE_RET_CLEANUP_BYTES,
            PER_PLANE_DECODE_ARG_COUNT * 4
        );
    }

    #[test]
    fn entry_precedes_ret_and_fault_in_code_memory() {
        // The per-plane decoder body (entry `0x10006538`) precedes both
        // the `ret 0x1c` (`0x10006b94`, the non-faulting return) and
        // the fault block (`0x10006ba2`, which sets the status `3`
        // before its own return). `black_box` so the const ordering is
        // verified at run time, not folded away by the optimiser.
        let entry = core::hint::black_box(PER_PLANE_DECODE_ENTRY_RVA);
        let ret = core::hint::black_box(PER_PLANE_DECODE_RET_RVA);
        let fault = core::hint::black_box(FRAME_FAULT_RETURN_RVA);
        assert!(entry < ret);
        assert!(entry < fault);
        // The fault block follows the primary `ret 0x1c` in the §6
        // listing (`0x10006ba2..0x10006baa` is the trailing fault
        // return after the main cleanup at `0x10006b94`).
        assert!(fault > ret);
    }

    #[test]
    fn reconstruction_target_after_call_site() {
        // §6.2 — the parser proceeds to reconstruction after the call
        // site returns. `black_box` so the const ordering is verified
        // at run time.
        let recon = core::hint::black_box(FRAME_OUTPUT_RECONSTRUCTION_RVA);
        let call_site = core::hint::black_box(PER_PLANE_DECODE_CALL_SITE_RVA);
        assert!(recon > call_site);
    }

    // ---- FrameExitDisposition ----------------------------------------

    #[test]
    fn proceed_disposition_targets_reconstruction() {
        let d = FrameExitDisposition::ProceedToReconstruction;
        assert!(d.proceeds_to_reconstruction());
        assert!(!d.is_fault());
        assert_eq!(d.target_rva(), FRAME_OUTPUT_RECONSTRUCTION_RVA);
        assert_eq!(d.frame_status(), PLANE_DECODE_STATUS_OK);
    }

    #[test]
    fn fault_disposition_targets_fault_return() {
        let d = FrameExitDisposition::EndOfFrameFault;
        assert!(d.is_fault());
        assert!(!d.proceeds_to_reconstruction());
        assert_eq!(d.target_rva(), FRAME_FAULT_RETURN_RVA);
        assert_eq!(d.frame_status(), PLANE_DECODE_STATUS_MALFORMED);
        assert_eq!(d.frame_status(), 3);
    }

    // ---- FramePlaneStatusFold ----------------------------------------

    #[test]
    fn all_ok_proceeds_to_reconstruction() {
        let fold = FramePlaneStatusFold::from_iteration_order([
            PlaneDecodeStatus::Ok,
            PlaneDecodeStatus::Ok,
            PlaneDecodeStatus::Ok,
        ]);
        assert!(fold.proceeds_to_reconstruction());
        assert_eq!(
            fold.disposition,
            FrameExitDisposition::ProceedToReconstruction
        );
        assert_eq!(fold.first_fault_iteration_index, None);
        assert_eq!(fold.first_fault_plane_idx(), None);
    }

    #[test]
    fn first_plane_fault_short_circuits_at_index_zero() {
        // The U plane (iteration index 0, plane_idx 2) faults.
        let fold = FramePlaneStatusFold::from_iteration_order([
            PlaneDecodeStatus::Malformed,
            PlaneDecodeStatus::Ok,
            PlaneDecodeStatus::Ok,
        ]);
        assert!(fold.disposition.is_fault());
        assert_eq!(fold.first_fault_iteration_index, Some(0));
        assert_eq!(fold.first_fault_plane_idx(), Some(PLANE_IDX_U));
    }

    #[test]
    fn last_plane_fault_records_index_two() {
        // Only the Y plane (iteration index 2, plane_idx 0) faults.
        let fold = FramePlaneStatusFold::from_iteration_order([
            PlaneDecodeStatus::Ok,
            PlaneDecodeStatus::Ok,
            PlaneDecodeStatus::Malformed,
        ]);
        assert!(fold.disposition.is_fault());
        assert_eq!(fold.first_fault_iteration_index, Some(2));
        assert_eq!(fold.first_fault_plane_idx(), Some(PLANE_IDX_Y));
    }

    #[test]
    fn fold_short_circuits_at_first_of_multiple_faults() {
        // Both the V plane (iteration index 1) and the Y plane
        // (iteration index 2) fault; the fold reports the first.
        let fold = FramePlaneStatusFold::from_iteration_order([
            PlaneDecodeStatus::Ok,
            PlaneDecodeStatus::Malformed,
            PlaneDecodeStatus::Malformed,
        ]);
        assert!(fold.disposition.is_fault());
        assert_eq!(fold.first_fault_iteration_index, Some(1));
        assert_eq!(fold.first_fault_plane_idx(), Some(PLANE_IDX_V));
    }

    #[test]
    fn plane_idx_order_constructor_reorders_into_iteration_order() {
        // Supplied in plane_idx order [Y, V, U]; only Y (plane_idx 0)
        // faults. In §8 iteration order Y is visited last (iteration
        // index 2).
        let by_plane_idx = [
            PlaneDecodeStatus::Malformed, // plane_idx 0 = Y
            PlaneDecodeStatus::Ok,        // plane_idx 1 = V
            PlaneDecodeStatus::Ok,        // plane_idx 2 = U
        ];
        let fold = FramePlaneStatusFold::from_plane_idx_order(by_plane_idx);
        assert!(fold.disposition.is_fault());
        assert_eq!(fold.first_fault_iteration_index, Some(2));
        assert_eq!(fold.first_fault_plane_idx(), Some(PLANE_IDX_Y));
    }

    #[test]
    fn plane_idx_and_iteration_order_agree_on_disposition() {
        // The disposition is order-agnostic: any input with a fault
        // yields EndOfFrameFault regardless of which constructor is
        // used.
        let by_plane_idx = [
            PlaneDecodeStatus::Ok,        // Y
            PlaneDecodeStatus::Ok,        // V
            PlaneDecodeStatus::Malformed, // U
        ];
        let via_plane_idx = FramePlaneStatusFold::from_plane_idx_order(by_plane_idx);
        // The same set re-expressed in iteration order [U, V, Y].
        let via_iteration = FramePlaneStatusFold::from_iteration_order([
            PlaneDecodeStatus::Malformed, // U
            PlaneDecodeStatus::Ok,        // V
            PlaneDecodeStatus::Ok,        // Y
        ]);
        assert_eq!(via_plane_idx.disposition, via_iteration.disposition);
        assert_eq!(
            via_plane_idx.first_fault_iteration_index,
            via_iteration.first_fault_iteration_index
        );
        // U is iteration index 0 in both.
        assert_eq!(via_plane_idx.first_fault_plane_idx(), Some(PLANE_IDX_U));
    }
}
