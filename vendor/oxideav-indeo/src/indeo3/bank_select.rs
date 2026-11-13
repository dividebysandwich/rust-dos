//! Indeo 3 spec/05 §4.2 — `frame_flags` bit 9 ping-pong bank
//! selection (the per-plane decoder's destination / source slot
//! inversion that drives the §5 motion-compensation "previous
//! frame" reference).
//!
//! Spec source: `docs/video/indeo/indeo3/spec/05-motion-compensation.md`
//! §4.2 — "Bank selection by `frame_flags` bit 9" (the parser at
//! `IR32_32.DLL!0x100045b1..0x100045fd` builds the two strip-context
//! slot-index arguments the per-plane decode call pushes as `[esp+0x54]`
//! and `[esp+0x58]` per `spec/02 §6` table rows 2-3).
//!
//! Rounds 8 ([`super::strip_context::strip_slot_index`]) and 15
//! ([`super::mc_address`]) covered the two ends of this surface but
//! left the §4.2 inversion itself unwrapped:
//!
//! * `strip_slot_index(plane_idx, buffer_selector)` returns *one*
//!   slot index — the destination slot the per-plane decoder writes
//!   into.
//! * [`super::mc_address::McCellAddressPair::resolve`] accepts the
//!   `(dst_slot, src_slot)` pair *already resolved* — its doc
//!   explicitly notes that "the §4.2 `frame_flags` bit 9 source /
//!   destination slot inversion is the per-plane decoder's job
//!   at `IR32_32.DLL!0x100045b1..0x100045fd`".
//!
//! This module owns the missing middle — the typed mapping
//! `(frame_flags bit 9, plane_idx) → (dst_slot, src_slot)` and
//! the few constants the §4.2 mapping rests on.
//!
//! ## The §4.2 inversion (verbatim spec extract)
//!
//! Per §4.2's parser-text walk:
//!
//! * **`frame_flags` bit 9 = 0** (primary buffer in use): the
//!   destination slot is `plane_idx + 3` (slots 3, 4, 5 for Y, V, U
//!   respectively); the source slot is `plane_idx` (slots 0, 1, 2).
//! * **`frame_flags` bit 9 = 1** (secondary buffer in use): the
//!   destination slot is `plane_idx` (slots 0, 1, 2); the source slot
//!   is `plane_idx + 3` (slots 3, 4, 5).
//!
//! In both cases the two slot indices differ by exactly
//! [`BANK_INVERSION_DELTA`] (`= 3`) and are the
//! same-plane / opposite-bank pair. The destination is the bank
//! the *current* frame writes into; the source is the bank the
//! *previous* frame wrote into (= what the *next* frame will read
//! from when this frame's bit 9 flips).
//!
//! ## What this module owns
//!
//! * [`BANK_INVERSION_DELTA`] (`= 3`) — the per-plane slot-index
//!   delta between the primary and secondary banks. Aliases
//!   `[`PRIMARY_BANK_SLOTS`][plane_idx] - [`SECONDARY_BANK_SLOTS`][plane_idx]`
//!   for any valid `plane_idx`; surfaced as a named constant so
//!   the §4.2 "the two slot indices are inverted" identity has a
//!   single symbol the source / destination resolvers cite.
//! * [`Bank`] — the two-variant enum naming the §5.1 primary /
//!   secondary banks, with a [`Bank::from_buffer_selector`]
//!   constructor that decodes `frame_flags` bit 9 (per §4.2's
//!   `test ch, 0x2` test at `IR32_32.DLL!0x100045b1`; the
//!   `ch` byte holds the high byte of `frame_flags`, so `& 0x2`
//!   tests the 16-bit-`frame_flags` bit 9 mask `0x0200`).
//! * [`McBankAssignment`] — the resolved `(dst_slot, src_slot)`
//!   pair per `(frame_flags bit 9, plane_idx)`. Carries the
//!   destination [`Bank`] (the bank the current frame writes into)
//!   for callers that need to thread the bank choice further (e.g.
//!   reporting / debug surfaces); the source bank is always the
//!   inverse.
//! * [`McBankAssignment::resolve`] — the entry point that runs
//!   the §4.2 mapping for a `(FrameFlags, plane_idx)` pair, with
//!   the same `plane_idx >= PLANE_COUNT` guard
//!   [`super::strip_context::strip_slot_index`] applies.
//! * [`McBankAssignment::is_self_copy`] — defensive predicate
//!   (always `false` for a well-formed result) for callers that
//!   want to assert the §4.2 ping-pong invariant. The §4.2
//!   parser-text exit "if a frame chose to read and write within
//!   the same bank, the two arguments would be identical, and
//!   the MC copy would degenerate into a self-copy" is the
//!   negative-existence-result the binary tolerates but does not
//!   produce.
//!
//! ## What this module deliberately does not do (chapter boundary)
//!
//! * It does not perform the strip-context-slot **read** at
//!   `IR32_32.DLL!0x10006638..0x10006641`. That index arithmetic
//!   (`16 * cell_slot + slot_idx`) is [`super::mc_address`]'s
//!   [`super::mc_address::CellSubarrayIndex`].
//! * It does not load the strip-context per-cell sub-array DWORDs
//!   themselves. Those are populated by the pre-frame cell-stack
//!   setup (spec/03 §6 open question 4) and reached via
//!   [`super::cell_subarray`].
//! * It does not perform the §5.1 / §4.4 source-pointer
//!   range-check. Per §4.4 the binary itself performs no check;
//!   safe-Rust callers that operate over an explicit pixel-buffer
//!   slice enforce the bound at the slice boundary.
//! * It does not own the per-frame bank-state machine that flips
//!   bit 9 across frames. The encoder is responsible for the bit-9
//!   sequence; the decoder just consults the per-frame value.
//!   `spec/05 §4.2` notes the expected behaviour ("`frame_flags`
//!   bit 9 flipping between adjacent frames") but the decoder
//!   does not validate it.
//!
//! ## RVA crosswalk
//!
//! The four §4.2 parser sites (per the spec text):
//!
//! | Site RVA | Computes | Result |
//! | -------- | -------- | ------ |
//! | `IR32_32.DLL!0x100045b1` | `test ch, 0x2` on `frame_flags` high byte | branch select |
//! | `IR32_32.DLL!0x100045c3..0x100045d4` | destination slot from bit 9 | written to `[esp+0x58]` ⇒ `dst_slot` |
//! | `IR32_32.DLL!0x100045d7` | `test ch, 0x2` again (same flag, new context) | branch select |
//! | `IR32_32.DLL!0x100045e9..0x100045fd` | source slot from bit 9 | written to `[esp+0x54]` ⇒ `src_slot` |
//!
//! The two `test ch, 0x2` sites consult the same bit; the inversion
//! is wired by the **opposite branch arms** (the bit-set branch on
//! the destination computation matches the bit-clear branch on the
//! source computation, and vice versa). This module folds the
//! two branches into the single [`McBankAssignment::resolve`] call.

use super::header::FrameFlags;
use super::picture_layer::PLANE_COUNT;
use super::strip_context::{PRIMARY_BANK_SLOTS, SECONDARY_BANK_SLOTS};

/// Spec/05 §4.2 — per-plane slot-index delta between the primary and
/// secondary banks (`= 3`).
///
/// Equals `PRIMARY_BANK_SLOTS[plane_idx] - SECONDARY_BANK_SLOTS[plane_idx]`
/// for any valid `plane_idx`; the spec text states this directly
/// ("the destination slot is `plane_idx + 3`" vs "the destination
/// slot is `plane_idx`"). Surfaced here so the §4.2 "the two slot
/// indices are inverted" identity has a single named constant the
/// source / destination resolvers cite and a `const _` cross-check
/// in the module's tests guarantees it stays in sync with the two
/// per-plane tables.
pub const BANK_INVERSION_DELTA: usize = 3;

/// Spec/05 §4.2 — the two strip-context-array banks the per-plane
/// decoder partitions the six dispatchable slots into.
///
/// Mirrors `spec/02 §5.1`'s `PRIMARY_BANK_SLOTS` / `SECONDARY_BANK_SLOTS`
/// pair: `Primary` covers slots 3, 4, 5; `Secondary` covers slots 0,
/// 1, 2. The per-frame choice between the two is controlled by
/// `frame_flags` bit 9 (`BUFFER_SELECTOR`) per §4.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bank {
    /// The primary bank (slots 3, 4, 5 for Y, V, U). Active when
    /// `frame_flags` bit 9 is **clear**.
    Primary,
    /// The secondary bank (slots 0, 1, 2 for Y, V, U). Active when
    /// `frame_flags` bit 9 is **set**.
    Secondary,
}

impl Bank {
    /// Decode the §4.2 `frame_flags` bit 9 into a typed bank
    /// selector.
    ///
    /// Per §4.2 the parser tests `ch & 0x2` at
    /// `IR32_32.DLL!0x100045b1`, where `ch` is the high byte of
    /// the 16-bit `frame_flags` value loaded by the prior
    /// `mov cx, word ptr [eax + 0x2]` (= `frame_flags`); the
    /// `0x2` mask on the high byte corresponds to the
    /// 16-bit-`frame_flags` mask `0x0200`. This constructor
    /// accepts the typed [`FrameFlags`] surface
    /// ([`super::header::FrameFlags::buffer_selector`]) and folds
    /// the convention.
    pub fn from_buffer_selector(flags: FrameFlags) -> Self {
        if flags.buffer_selector() {
            Bank::Secondary
        } else {
            Bank::Primary
        }
    }

    /// The opposite bank (`Primary` ⇔ `Secondary`).
    ///
    /// Per §4.2 the source bank is always the **inverse** of the
    /// destination bank for any given frame; the two are linked
    /// rigidly through this swap.
    pub fn opposite(self) -> Self {
        match self {
            Bank::Primary => Bank::Secondary,
            Bank::Secondary => Bank::Primary,
        }
    }

    /// The slot index this bank assigns to a given `plane_idx`.
    ///
    /// Returns `None` if `plane_idx >= PLANE_COUNT` (only the
    /// three legal indices `PLANE_IDX_Y`, `PLANE_IDX_V`,
    /// `PLANE_IDX_U` are addressable).
    pub fn slot_for_plane(self, plane_idx: usize) -> Option<usize> {
        if plane_idx >= PLANE_COUNT {
            return None;
        }
        Some(match self {
            Bank::Primary => PRIMARY_BANK_SLOTS[plane_idx],
            Bank::Secondary => SECONDARY_BANK_SLOTS[plane_idx],
        })
    }

    /// True iff this is the primary bank.
    pub fn is_primary(self) -> bool {
        matches!(self, Bank::Primary)
    }

    /// True iff this is the secondary bank.
    pub fn is_secondary(self) -> bool {
        matches!(self, Bank::Secondary)
    }
}

/// Spec/05 §4.2 — the resolved `(dst_slot, src_slot)` pair the
/// per-plane decode-call pushes as `[esp+0x58]` and `[esp+0x54]`.
///
/// The destination slot is the strip-context slot the *current*
/// frame writes into; the source slot is the slot the *previous*
/// frame wrote into (= the MC "previous frame" reference). Per
/// §4.2 the two are always **opposite-bank, same-plane** — never
/// equal for a well-formed frame.
///
/// The destination [`Bank`] is also carried so callers that want
/// to thread the bank choice further can do so without re-decoding
/// `frame_flags`. The source bank is always
/// [`Bank::opposite`] of the destination bank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McBankAssignment {
    /// The plane index this assignment is for (= 0, 1, 2 for Y,
    /// V, U).
    pub plane_idx: usize,
    /// The destination bank (the bank the current frame writes
    /// into).
    pub dst_bank: Bank,
    /// The strip-context slot index the current frame writes into
    /// (the `[esp+0x58]` argument).
    pub dst_slot: usize,
    /// The strip-context slot index the MC fetcher reads from
    /// (the `[esp+0x54]` argument).
    pub src_slot: usize,
}

impl McBankAssignment {
    /// Run the §4.2 ping-pong mapping for a `(FrameFlags,
    /// plane_idx)` pair.
    ///
    /// Returns `None` if `plane_idx >= PLANE_COUNT` (only the
    /// three legal indices are addressable; the same bound
    /// [`super::strip_context::strip_slot_index`] applies).
    ///
    /// Mirrors the parser at
    /// `IR32_32.DLL!0x100045b1..0x100045fd`. For any well-formed
    /// `(flags, plane_idx)`, the result satisfies
    /// `src_slot == dst_slot ± BANK_INVERSION_DELTA` (the §4.2
    /// "inverted" identity).
    pub fn resolve(flags: FrameFlags, plane_idx: usize) -> Option<Self> {
        if plane_idx >= PLANE_COUNT {
            return None;
        }
        let dst_bank = Bank::from_buffer_selector(flags);
        let src_bank = dst_bank.opposite();
        let dst_slot = dst_bank.slot_for_plane(plane_idx)?;
        let src_slot = src_bank.slot_for_plane(plane_idx)?;
        Some(McBankAssignment {
            plane_idx,
            dst_bank,
            dst_slot,
            src_slot,
        })
    }

    /// The source bank ([`Bank::opposite`] of [`Self::dst_bank`]).
    ///
    /// Convenience accessor; identical to
    /// `self.dst_bank.opposite()`.
    pub fn src_bank(self) -> Bank {
        self.dst_bank.opposite()
    }

    /// True if the resolved `(dst_slot, src_slot)` pair is a
    /// degenerate self-copy (the two arguments are equal).
    ///
    /// Per §4.2: "if a frame chose to read and write within the
    /// same bank, the two arguments would be identical, and the
    /// MC copy would degenerate into a self-copy. No such frame
    /// is observed in the binary." A well-formed
    /// [`Self::resolve`] result therefore always returns `false`
    /// here; the predicate is exposed for safe-Rust callers that
    /// want to assert the invariant on input or after applying a
    /// hypothetical custom mapping.
    pub fn is_self_copy(self) -> bool {
        self.dst_slot == self.src_slot
    }

    /// The absolute slot-index delta between the two arguments
    /// (`|dst_slot - src_slot|`).
    ///
    /// For any [`Self::resolve`] result this is exactly
    /// [`BANK_INVERSION_DELTA`] (`= 3`); the §4.2 invariant is
    /// expressible as `self.slot_delta() ==
    /// BANK_INVERSION_DELTA`.
    pub fn slot_delta(self) -> usize {
        self.dst_slot.abs_diff(self.src_slot)
    }
}

#[cfg(test)]
mod tests {
    use super::super::header::FrameFlags;
    use super::super::picture_layer::{PLANE_COUNT, PLANE_IDX_U, PLANE_IDX_V, PLANE_IDX_Y};
    use super::super::strip_context::{PRIMARY_BANK_SLOTS, SECONDARY_BANK_SLOTS};
    use super::{BANK_INVERSION_DELTA, Bank, McBankAssignment};

    // ---- BANK_INVERSION_DELTA cross-checks ----

    #[test]
    fn inversion_delta_equals_primary_minus_secondary_y() {
        // Spec/05 §4.2 — the "+3" in "destination slot is plane_idx + 3"
        // is BANK_INVERSION_DELTA. Verify on the Y plane.
        assert_eq!(
            PRIMARY_BANK_SLOTS[PLANE_IDX_Y] - SECONDARY_BANK_SLOTS[PLANE_IDX_Y],
            BANK_INVERSION_DELTA,
        );
    }

    #[test]
    fn inversion_delta_equals_primary_minus_secondary_v() {
        assert_eq!(
            PRIMARY_BANK_SLOTS[PLANE_IDX_V] - SECONDARY_BANK_SLOTS[PLANE_IDX_V],
            BANK_INVERSION_DELTA,
        );
    }

    #[test]
    fn inversion_delta_equals_primary_minus_secondary_u() {
        assert_eq!(
            PRIMARY_BANK_SLOTS[PLANE_IDX_U] - SECONDARY_BANK_SLOTS[PLANE_IDX_U],
            BANK_INVERSION_DELTA,
        );
    }

    #[test]
    fn inversion_delta_value_is_three() {
        // The literal "+3" from spec/05 §4.2 prose, surfaced as a
        // named constant.
        assert_eq!(BANK_INVERSION_DELTA, 3);
    }

    // ---- Bank constructor ----

    #[test]
    fn bank_from_buffer_selector_clear_is_primary() {
        // Spec/05 §4.2 — frame_flags bit 9 = 0 ⇒ primary buffer in use.
        // Bit 9 = mask 0x0200; build a FrameFlags without that bit.
        let flags = FrameFlags(0x0000);
        assert_eq!(Bank::from_buffer_selector(flags), Bank::Primary);
    }

    #[test]
    fn bank_from_buffer_selector_set_is_secondary() {
        // Spec/05 §4.2 — frame_flags bit 9 = 1 ⇒ secondary buffer.
        let flags = FrameFlags(0x0200);
        assert_eq!(Bank::from_buffer_selector(flags), Bank::Secondary);
    }

    #[test]
    fn bank_from_buffer_selector_other_bits_irrelevant() {
        // §4.2 only consults bit 9. Other bits set should not change
        // the result. Use a mix of bits 0..15 with bit 9 clear ⇒
        // Primary.
        let flags = FrameFlags(0xfdff); // all bits set except bit 9
        assert_eq!(Bank::from_buffer_selector(flags), Bank::Primary);
        // ... and with bit 9 set ⇒ Secondary.
        let flags = FrameFlags(0xffff);
        assert_eq!(Bank::from_buffer_selector(flags), Bank::Secondary);
    }

    #[test]
    fn bank_opposite_swaps_primary_and_secondary() {
        assert_eq!(Bank::Primary.opposite(), Bank::Secondary);
        assert_eq!(Bank::Secondary.opposite(), Bank::Primary);
    }

    #[test]
    fn bank_opposite_is_involution() {
        // f(f(x)) == x for both inputs.
        assert_eq!(Bank::Primary.opposite().opposite(), Bank::Primary);
        assert_eq!(Bank::Secondary.opposite().opposite(), Bank::Secondary);
    }

    #[test]
    fn bank_predicates_partition_the_two_variants() {
        assert!(Bank::Primary.is_primary());
        assert!(!Bank::Primary.is_secondary());
        assert!(Bank::Secondary.is_secondary());
        assert!(!Bank::Secondary.is_primary());
    }

    // ---- Bank::slot_for_plane ----

    #[test]
    fn bank_slot_for_plane_primary_matches_spec_table() {
        // Spec/02 §5.1 — PRIMARY_BANK_SLOTS = [3, 4, 5].
        assert_eq!(Bank::Primary.slot_for_plane(PLANE_IDX_Y), Some(3));
        assert_eq!(Bank::Primary.slot_for_plane(PLANE_IDX_V), Some(4));
        assert_eq!(Bank::Primary.slot_for_plane(PLANE_IDX_U), Some(5));
    }

    #[test]
    fn bank_slot_for_plane_secondary_matches_spec_table() {
        // Spec/02 §5.1 — SECONDARY_BANK_SLOTS = [0, 1, 2].
        assert_eq!(Bank::Secondary.slot_for_plane(PLANE_IDX_Y), Some(0));
        assert_eq!(Bank::Secondary.slot_for_plane(PLANE_IDX_V), Some(1));
        assert_eq!(Bank::Secondary.slot_for_plane(PLANE_IDX_U), Some(2));
    }

    #[test]
    fn bank_slot_for_plane_rejects_out_of_range_plane_idx() {
        // PLANE_COUNT == 3; any index >= 3 is rejected.
        for plane_idx in PLANE_COUNT..(PLANE_COUNT + 4) {
            assert_eq!(Bank::Primary.slot_for_plane(plane_idx), None);
            assert_eq!(Bank::Secondary.slot_for_plane(plane_idx), None);
        }
    }

    #[test]
    fn bank_slot_for_plane_primary_minus_secondary_is_inversion_delta() {
        // The §4.2 "+3" identity, per plane.
        for plane_idx in 0..PLANE_COUNT {
            let primary = Bank::Primary.slot_for_plane(plane_idx).unwrap();
            let secondary = Bank::Secondary.slot_for_plane(plane_idx).unwrap();
            assert_eq!(primary - secondary, BANK_INVERSION_DELTA);
        }
    }

    // ---- McBankAssignment::resolve ----

    #[test]
    fn resolve_primary_y_writes_slot_3_reads_slot_0() {
        // Spec/05 §4.2 — frame_flags bit 9 clear ⇒
        // dst = plane_idx + 3 = 3 for Y; src = plane_idx = 0.
        let flags = FrameFlags(0x0000);
        let a = McBankAssignment::resolve(flags, PLANE_IDX_Y).unwrap();
        assert_eq!(a.plane_idx, PLANE_IDX_Y);
        assert_eq!(a.dst_bank, Bank::Primary);
        assert_eq!(a.dst_slot, 3);
        assert_eq!(a.src_slot, 0);
    }

    #[test]
    fn resolve_secondary_y_writes_slot_0_reads_slot_3() {
        // Spec/05 §4.2 — frame_flags bit 9 set ⇒ dst = plane_idx,
        // src = plane_idx + 3. The two slots invert.
        let flags = FrameFlags(0x0200);
        let a = McBankAssignment::resolve(flags, PLANE_IDX_Y).unwrap();
        assert_eq!(a.plane_idx, PLANE_IDX_Y);
        assert_eq!(a.dst_bank, Bank::Secondary);
        assert_eq!(a.dst_slot, 0);
        assert_eq!(a.src_slot, 3);
    }

    #[test]
    fn resolve_primary_v_writes_slot_4_reads_slot_1() {
        let flags = FrameFlags(0x0000);
        let a = McBankAssignment::resolve(flags, PLANE_IDX_V).unwrap();
        assert_eq!(a.dst_slot, 4);
        assert_eq!(a.src_slot, 1);
    }

    #[test]
    fn resolve_secondary_v_writes_slot_1_reads_slot_4() {
        let flags = FrameFlags(0x0200);
        let a = McBankAssignment::resolve(flags, PLANE_IDX_V).unwrap();
        assert_eq!(a.dst_slot, 1);
        assert_eq!(a.src_slot, 4);
    }

    #[test]
    fn resolve_primary_u_writes_slot_5_reads_slot_2() {
        let flags = FrameFlags(0x0000);
        let a = McBankAssignment::resolve(flags, PLANE_IDX_U).unwrap();
        assert_eq!(a.dst_slot, 5);
        assert_eq!(a.src_slot, 2);
    }

    #[test]
    fn resolve_secondary_u_writes_slot_2_reads_slot_5() {
        let flags = FrameFlags(0x0200);
        let a = McBankAssignment::resolve(flags, PLANE_IDX_U).unwrap();
        assert_eq!(a.dst_slot, 2);
        assert_eq!(a.src_slot, 5);
    }

    #[test]
    fn resolve_rejects_out_of_range_plane_idx() {
        let flags = FrameFlags(0x0000);
        for plane_idx in PLANE_COUNT..(PLANE_COUNT + 4) {
            assert!(McBankAssignment::resolve(flags, plane_idx).is_none());
        }
    }

    #[test]
    fn resolve_src_bank_equals_dst_bank_opposite() {
        // For every (flags-bit-9, plane) combination, src_bank is
        // dst_bank's opposite. Quartet exercise.
        for raw in [0x0000u16, 0x0200u16] {
            let flags = FrameFlags(raw);
            for plane_idx in 0..PLANE_COUNT {
                let a = McBankAssignment::resolve(flags, plane_idx).unwrap();
                assert_eq!(a.src_bank(), a.dst_bank.opposite());
            }
        }
    }

    #[test]
    fn resolve_is_never_self_copy() {
        // §4.2 — "the two slot indices are inverted ... a frame that
        // chose to read and write within the same bank ... is not
        // observed in the binary". Verify for all six legal
        // (flags-bit-9, plane) combinations.
        for raw in [0x0000u16, 0x0200u16] {
            let flags = FrameFlags(raw);
            for plane_idx in 0..PLANE_COUNT {
                let a = McBankAssignment::resolve(flags, plane_idx).unwrap();
                assert!(!a.is_self_copy());
            }
        }
    }

    #[test]
    fn resolve_slot_delta_is_bank_inversion_delta() {
        // §4.2 invariant: |dst_slot - src_slot| == BANK_INVERSION_DELTA
        // for every well-formed resolve result.
        for raw in [0x0000u16, 0x0200u16] {
            let flags = FrameFlags(raw);
            for plane_idx in 0..PLANE_COUNT {
                let a = McBankAssignment::resolve(flags, plane_idx).unwrap();
                assert_eq!(a.slot_delta(), BANK_INVERSION_DELTA);
            }
        }
    }

    #[test]
    fn resolve_dst_slot_agrees_with_strip_slot_index() {
        // The destination half of resolve() must agree with the
        // round-8 `strip_slot_index(plane_idx, buffer_selector)`
        // for every (flags-bit-9, plane) pair. The "buffer_selector"
        // arg of strip_slot_index is the typed bit-9 read.
        use super::super::strip_context::strip_slot_index;
        for raw in [0x0000u16, 0x0200u16] {
            let flags = FrameFlags(raw);
            for plane_idx in 0..PLANE_COUNT {
                let a = McBankAssignment::resolve(flags, plane_idx).unwrap();
                let expected = strip_slot_index(plane_idx, flags.buffer_selector()).unwrap();
                assert_eq!(a.dst_slot, expected);
            }
        }
    }

    #[test]
    fn resolve_src_slot_is_strip_slot_index_with_inverted_buffer_selector() {
        // The source half is the destination of the *opposite*
        // frame_flags-bit-9 value — i.e. what the previous frame
        // wrote into. Verify the dual identity holds.
        use super::super::strip_context::strip_slot_index;
        for raw in [0x0000u16, 0x0200u16] {
            let flags = FrameFlags(raw);
            let inverted = !flags.buffer_selector();
            for plane_idx in 0..PLANE_COUNT {
                let a = McBankAssignment::resolve(flags, plane_idx).unwrap();
                let expected = strip_slot_index(plane_idx, inverted).unwrap();
                assert_eq!(a.src_slot, expected);
            }
        }
    }

    // ---- Symmetry: flipping the frame between two frames inverts ----
    //      the (dst, src) pair (the ping-pong identity).

    #[test]
    fn ping_pong_two_frame_dst_becomes_src_on_next_frame() {
        // §4.2 — if frame N writes into slot S, frame N+1 (with
        // bit 9 flipped) reads from slot S.
        for plane_idx in 0..PLANE_COUNT {
            let frame_n = McBankAssignment::resolve(FrameFlags(0x0000), plane_idx).unwrap();
            let frame_np1 = McBankAssignment::resolve(FrameFlags(0x0200), plane_idx).unwrap();
            // Frame N's dst_slot must equal frame N+1's src_slot.
            assert_eq!(frame_n.dst_slot, frame_np1.src_slot);
            // And vice versa: frame N's src_slot equals frame N+1's
            // dst_slot.
            assert_eq!(frame_n.src_slot, frame_np1.dst_slot);
        }
    }

    #[test]
    fn ping_pong_two_frame_banks_swap() {
        // The bank pair (dst_bank, src_bank) of frame N is the
        // (src_bank, dst_bank) of frame N+1.
        for plane_idx in 0..PLANE_COUNT {
            let frame_n = McBankAssignment::resolve(FrameFlags(0x0000), plane_idx).unwrap();
            let frame_np1 = McBankAssignment::resolve(FrameFlags(0x0200), plane_idx).unwrap();
            assert_eq!(frame_n.dst_bank, frame_np1.src_bank());
            assert_eq!(frame_n.src_bank(), frame_np1.dst_bank);
        }
    }
}
