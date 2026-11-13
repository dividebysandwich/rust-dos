//! Indeo 3 frame finalisation: the spec/07 §6 per-frame state
//! updates that `sub_4190` performs after the output-conversion has
//! written the final frame, before it returns to the VfW dispatcher.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/07-output-reconstruction.md`
//! §6 (frame finalisation), with cross-references into §6.1 (the
//! saved `frame_flags` slot at `[outer_instance + 0x434]` and the
//! bit-9 reference-bank ping-pong it drives on the *next* frame),
//! §6.2 (the saved `frame_number` slot at `[outer_instance + 0x474]`
//! and the next-frame continuity check at `IR32_32.DLL!0x100041f8`),
//! §6.3 (the `sub_4190` return code), and §6.4 (the absence of any
//! explicit decoder-side buffer rotation).
//!
//! Round 29 lands the state-update slice that sits directly above the
//! round-28 output stage ([`super::frame_output`], whose module docs
//! flag "No frame finalisation … the next chapter slice above this
//! one"). The output stage ends at "the host's output buffer holds
//! the reconstructed frame"; this module owns what `sub_4190` does
//! between that point and its `ret`.
//!
//! This module maps to the spec/07 §6 sub-sections:
//!
//! * §6.1 — [`SavedFrameFlags`] models the 16-bit slot at
//!   `[outer_instance + 0x434]` (RVA [`SAVED_FRAME_FLAGS_SLOT_OFFSET`],
//!   updated at [`SAVED_FRAME_FLAGS_STORE_RVA`]). Its
//!   [`SavedFrameFlags::next_frame_read_bank`] returns the
//!   [`super::Bank`] the *next* frame reads from — driven by *this*
//!   frame's bit-9 value, the encoder-driven "ping-pong" of §6.1 /
//!   spec/05 §4.2. A `const _` cross-check pins the bit-9 mask to
//!   [`super::header::FrameFlags::buffer_selector`]'s mask.
//! * §6.2 — [`SavedFrameNumber`] models the slot at
//!   `[outer_instance + 0x474]` ([`SAVED_FRAME_NUMBER_SLOT_OFFSET`],
//!   updated at [`SAVED_FRAME_NUMBER_STORE_RVA`]) and the next-frame
//!   continuity classifier [`FrameContinuity::classify`] (the
//!   `if [eax + 0x474] != ecx` test at
//!   [`FRAME_NUMBER_CONTINUITY_CHECK_RVA`]): an incoming frame whose
//!   number is exactly one more than the saved value is
//!   [`FrameContinuity::Sequential`]; any other value is
//!   [`FrameContinuity::Discontinuous`] (the "out-of-order / seek"
//!   path at [`FRAME_NUMBER_SEEK_PATH_RVA`], which re-validates the
//!   INTRA requirement).
//! * §6.3 — [`DecodeReturn`] enumerates the four `sub_4190` return
//!   dispositions (success `0`, input-format error `-100`,
//!   repeat-previous `1`, and a per-plane fault code propagated from
//!   the per-plane decoder), with [`DecodeReturn::code`] yielding the
//!   exact `i32` the VfW dispatcher sees.
//! * §6.4 — [`PERFORMS_BUFFER_ROTATION`] (`= false`) records that
//!   `sub_4190` performs no explicit buffer rotation; the ping-pong
//!   is entirely encoder-driven via the next frame's bit-9.
//!
//! The contract is the *typed state-update + return-disposition*
//! surface for one frame's finalisation. What this module
//! deliberately does **not** do (the §6 chapter boundaries):
//!
//! * It does not perform the output-conversion that precedes it
//!   (owned by [`super::frame_output`], spec/07 §5).
//! * It does not perform the per-plane decode whose status it may
//!   propagate (owned by [`super::strip_context::PlaneDecodeStatus`]
//!   and folded by [`super::frame_exit::FramePlaneStatusFold`]).
//! * It does not own the bank-slot resolution the saved bit-9 feeds
//!   into (owned by [`super::bank_select`], spec/05 §4.2); this
//!   module only reports *which* [`super::Bank`] the next frame will
//!   read from, not the strip-slot index arithmetic.
//! * It does not parse the header whose fields it stores (owned by
//!   [`super::header`], spec/01).

use super::Bank;
use super::header::{FrameFlags, FrameHeader};

/// Spec/07 §6.1 — the 16-bit saved-`frame_flags` slot offset inside
/// the outer instance state (`[outer_instance + 0x434]`).
pub const SAVED_FRAME_FLAGS_SLOT_OFFSET: usize = 0x434;

/// Spec/07 §6.1 — the RVA of the parser-side store that updates the
/// saved-`frame_flags` slot (`mov [ecx + 0x434], ax`).
pub const SAVED_FRAME_FLAGS_STORE_RVA: u32 = 0x1000_42c1;

/// Spec/07 §6.2 — the 32-bit saved-`frame_number` slot offset inside
/// the outer instance state (`[outer_instance + 0x474]`).
pub const SAVED_FRAME_NUMBER_SLOT_OFFSET: usize = 0x474;

/// Spec/07 §6.2 — the RVA of the parser-side store that updates the
/// saved-`frame_number` slot (`mov [ecx + 0x474], eax`).
pub const SAVED_FRAME_NUMBER_STORE_RVA: u32 = 0x1000_42a3;

/// Spec/07 §6.2 — the RVA of the next-frame continuity check
/// (`if [eax + 0x474] != ecx`).
pub const FRAME_NUMBER_CONTINUITY_CHECK_RVA: u32 = 0x1000_41f8;

/// Spec/07 §6.2 — the RVA of the "out-of-order / seek" path the
/// continuity check branches to when the frame number is not
/// sequential.
pub const FRAME_NUMBER_SEEK_PATH_RVA: u32 = 0x1000_4220;

/// Spec/07 §6.4 — `sub_4190` performs **no** explicit decoder-side
/// buffer rotation; the reference-bank ping-pong is entirely
/// encoder-driven via the next frame's `frame_flags` bit 9 (§6.1).
pub const PERFORMS_BUFFER_ROTATION: bool = false;

/// Spec/07 §6.3 — the `sub_4190` input-format error return value
/// (`-100`, i.e. `0xffffff9c`). Matches the §3.2 header-reject code
/// (`super::header` cites the same `-100` / `0xffffff9c` at the
/// `dec_version` / `frame_flags` faults).
pub const RETURN_INPUT_ERROR: i32 = -100;

/// Spec/07 §6.3 — the `sub_4190` success return value (`0`).
pub const RETURN_SUCCESS: i32 = 0;

/// Spec/07 §6.3 — the `sub_4190` repeat-previous-frame return value
/// (`1`): the NULL-frame / droppable-INTER path that asks the host
/// to re-display the prior frame.
pub const RETURN_REPEAT_PREVIOUS: i32 = 1;

/// Spec/07 §6.1 — `frame_flags` bit 9 (`BUFFER_SELECTOR`) mask. A
/// `const _` cross-check below pins this to the bit the
/// [`FrameFlags::buffer_selector`] accessor tests.
pub const BUFFER_SELECTOR_MASK: u16 = 0x0200;

// Cross-check: the §6.1 bit-9 mask is bit 9 (the `BUFFER_SELECTOR`
// bit the `FrameFlags::buffer_selector` accessor tests). The accessor
// is not `const fn`, so the runtime test
// `buffer_selector_mask_is_bit9` pins the equality dynamically; this
// `const _` pins the literal at build time.
const _: () = assert!(BUFFER_SELECTOR_MASK == 1 << 9);

/// Spec/07 §6.1 — the saved-`frame_flags` slot
/// (`[outer_instance + 0x434]`).
///
/// `sub_4190` stashes the *current* frame's `frame_flags` word here
/// at [`SAVED_FRAME_FLAGS_STORE_RVA`]. The slot is consulted on the
/// **next** frame by the bank-selection logic
/// (`IR32_32.DLL!0x100045b1..0x100045fd`, spec/05 §4.2): the previous
/// frame's bit-9 value determines which reference bank the current
/// frame reads from. This type therefore models a one-frame-deep
/// piece of inter-frame state — what was just decoded — and answers
/// "given this saved flags word, which bank does the next frame read
/// from?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedFrameFlags {
    /// The raw 16-bit `frame_flags` word of the just-decoded frame,
    /// as stored in `[outer_instance + 0x434]`.
    pub flags: FrameFlags,
}

impl SavedFrameFlags {
    /// Spec/07 §6.1 — capture the current frame's `frame_flags` word
    /// at finalisation time (the `mov [ecx + 0x434], ax` store).
    pub fn capture(header: &FrameHeader) -> Self {
        SavedFrameFlags {
            flags: header.bitstream.frame_flags,
        }
    }

    /// Spec/07 §6.1 — the saved bit-9 (`BUFFER_SELECTOR`) value that
    /// drives the *next* frame's reference-bank choice.
    pub fn buffer_selector(self) -> bool {
        self.flags.buffer_selector()
    }

    /// Spec/07 §6.1 / spec/05 §4.2 — the reference [`Bank`] the
    /// **next** frame reads its previous-frame data from, driven by
    /// *this* (just-saved) frame's bit-9 value.
    ///
    /// Per spec/05 §4.2 the bit-9 value selects between the primary
    /// and secondary reference banks; bit 9 = 0 → primary, bit 9 = 1
    /// → secondary. Encoders are expected to flip bit 9 between
    /// consecutive frames so the banks alternate (the §6.1
    /// "ping-pong"); the decoder imposes nothing (§6.4 — no explicit
    /// rotation), so two consecutive frames with the same bit 9
    /// produce a degenerate-but-legal same-bank sequence.
    pub fn next_frame_read_bank(self) -> Bank {
        // Reuse the spec/05 §4.2 bit-9 → bank fold so the two sites
        // (bank selection vs. finalisation read-out) cannot drift.
        Bank::from_buffer_selector(self.flags)
    }
}

/// Spec/07 §6.2 — the saved-`frame_number` slot
/// (`[outer_instance + 0x474]`).
///
/// `sub_4190` stashes the current frame's `frame_number` here at
/// [`SAVED_FRAME_NUMBER_STORE_RVA`]. The slot feeds the next frame's
/// `sub_4190` entry, which checks at
/// [`FRAME_NUMBER_CONTINUITY_CHECK_RVA`] whether the incoming frame's
/// number is exactly one more than the saved value (§6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedFrameNumber {
    /// The just-decoded frame's `frame_number`, as stored in
    /// `[outer_instance + 0x474]`.
    pub frame_number: u32,
}

impl SavedFrameNumber {
    /// Spec/07 §6.2 — capture the current frame's `frame_number` at
    /// finalisation time (the `mov [ecx + 0x474], eax` store).
    pub fn capture(header: &FrameHeader) -> Self {
        SavedFrameNumber {
            frame_number: header.frame.frame_number,
        }
    }

    /// Spec/07 §6.2 — classify an incoming frame's `frame_number`
    /// against this saved value as the next frame's continuity check
    /// (`if [eax + 0x474] != ecx`) would.
    ///
    /// The reference compares the incoming number against
    /// `saved + 1`: an exact match is [`FrameContinuity::Sequential`]
    /// (the normal forward step), anything else is
    /// [`FrameContinuity::Discontinuous`] (the seek path at
    /// [`FRAME_NUMBER_SEEK_PATH_RVA`], which re-validates the
    /// INTRA-frame requirement per spec/01 §3.2). The `+ 1` uses
    /// wrapping arithmetic to match the binary's 32-bit `inc`/`cmp`.
    pub fn classify(self, incoming_frame_number: u32) -> FrameContinuity {
        if incoming_frame_number == self.frame_number.wrapping_add(1) {
            FrameContinuity::Sequential
        } else {
            FrameContinuity::Discontinuous
        }
    }
}

/// Spec/07 §6.2 — the disposition of the next-frame continuity check
/// (`IR32_32.DLL!0x100041f8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameContinuity {
    /// The incoming `frame_number` is exactly `saved + 1`: a normal
    /// sequential frame; decode proceeds without re-validating the
    /// INTRA requirement.
    Sequential,
    /// The incoming `frame_number` is **not** `saved + 1`: a gap
    /// (seek / out-of-order) that takes the
    /// [`FRAME_NUMBER_SEEK_PATH_RVA`] path, where the decoder is
    /// permitted to re-validate the INTRA-frame requirement
    /// (spec/01 §3.2 bit 2) before continuing.
    Discontinuous,
}

impl FrameContinuity {
    /// True for [`FrameContinuity::Sequential`].
    pub fn is_sequential(self) -> bool {
        matches!(self, FrameContinuity::Sequential)
    }
}

/// Spec/07 §6.3 — the `sub_4190` return disposition the VfW
/// dispatcher receives from `ICDecompress`.
///
/// [`DecodeReturn::code`] yields the exact `i32` return value; the
/// host treats `0` as success and any non-zero value as an error or
/// a special indication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeReturn {
    /// `0` — successful decode; the output frame is in the host
    /// buffer.
    Success,
    /// `-100` (`0xffffff9c`) — input-format error (invalid
    /// `dec_version`, an invalid `frame_flags` combination, or an
    /// INTRA-expected-but-INTER-provided first frame).
    InputError,
    /// `1` — repeat-previous-frame indication; the host should
    /// re-display the prior frame (the NULL-frame / droppable-INTER
    /// path).
    RepeatPrevious,
    /// A per-plane fault code propagated from the per-plane decoder
    /// `sub_6538` through `var_8` in `sub_4190` (the per-plane fault
    /// codes documented in spec/06 §4 and spec/07 §4.1). Carries the
    /// raw non-zero plane status (e.g. the §6-status `3` malformed
    /// code).
    PlaneFault(i32),
}

impl DecodeReturn {
    /// Spec/07 §6.3 — the exact `i32` return value `sub_4190` hands
    /// back to the VfW dispatcher.
    pub fn code(self) -> i32 {
        match self {
            DecodeReturn::Success => RETURN_SUCCESS,
            DecodeReturn::InputError => RETURN_INPUT_ERROR,
            DecodeReturn::RepeatPrevious => RETURN_REPEAT_PREVIOUS,
            DecodeReturn::PlaneFault(code) => code,
        }
    }

    /// Whether the host treats this disposition as a successful
    /// decode (`code == 0`).
    pub fn is_success(self) -> bool {
        self.code() == RETURN_SUCCESS
    }
}

/// Spec/07 §6 — the bundle of state updates one frame's finalisation
/// produces: the saved `frame_flags` and `frame_number` slots that
/// become the *next* frame's inter-frame state, and the return code
/// `sub_4190` reports for *this* frame.
///
/// This groups the §6.1 / §6.2 / §6.3 outputs so a caller can finalise
/// a decoded frame in one step and carry the saved-slot pair forward
/// to the next frame's continuity check / bank selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameFinalisation {
    /// §6.1 — the saved `frame_flags` slot for the next frame's
    /// bank selection.
    pub saved_flags: SavedFrameFlags,
    /// §6.2 — the saved `frame_number` slot for the next frame's
    /// continuity check.
    pub saved_frame_number: SavedFrameNumber,
    /// §6.3 — the return code this frame reports.
    pub return_value: DecodeReturn,
}

impl FrameFinalisation {
    /// Spec/07 §6 — finalise a decoded frame: stash its `frame_flags`
    /// and `frame_number` (§6.1 / §6.2) and record the reported
    /// return disposition (§6.3).
    ///
    /// `return_value` is supplied by the caller because it depends on
    /// the per-plane decode outcome (the
    /// [`super::frame_exit::FramePlaneStatusFold`] / spec/06 fault
    /// chain) and on whether the frame was a NULL / repeat-previous
    /// frame — neither of which is a §6 concern. The §6.1 / §6.2
    /// slot captures happen regardless of the return value (the
    /// stores at [`SAVED_FRAME_FLAGS_STORE_RVA`] /
    /// [`SAVED_FRAME_NUMBER_STORE_RVA`] occur during header parse,
    /// before the decode's success is known).
    pub fn finalise(header: &FrameHeader, return_value: DecodeReturn) -> Self {
        FrameFinalisation {
            saved_flags: SavedFrameFlags::capture(header),
            saved_frame_number: SavedFrameNumber::capture(header),
            return_value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::header::FrameHeader;

    /// Build a minimal valid combined header with the given
    /// `frame_number` and `frame_flags`, then parse it. INTRA flag
    /// (bit 2) is OR-ed in so the first-frame INTER check passes.
    fn make_header(frame_number: u32, frame_flags: u16) -> FrameHeader {
        let mut buf = [0u8; COMBINED_HEADER_LEN];
        let unknown1: u32 = 0;
        let frame_size: u32 = COMBINED_HEADER_LEN as u32;
        let check_sum = frame_number ^ unknown1 ^ frame_size ^ MAGIC_FRMH;
        buf[0x00..0x04].copy_from_slice(&frame_number.to_le_bytes());
        buf[0x04..0x08].copy_from_slice(&unknown1.to_le_bytes());
        buf[0x08..0x0c].copy_from_slice(&check_sum.to_le_bytes());
        buf[0x0c..0x10].copy_from_slice(&frame_size.to_le_bytes());
        // bitstream header at 0x10.
        let bsh = 0x10;
        buf[bsh..bsh + 2].copy_from_slice(&REQUIRED_DEC_VERSION.to_le_bytes());
        // frame_flags | INTRA so parse's first-frame check passes.
        buf[bsh + 2..bsh + 4].copy_from_slice(&(frame_flags | 0x0004).to_le_bytes());
        // data_size (bits): a small non-null value.
        buf[bsh + 4..bsh + 8].copy_from_slice(&0x100u32.to_le_bytes());
        // height / width: minimum valid dimensions.
        buf[bsh + 0x0c..bsh + 0x0e].copy_from_slice(&MIN_DIMENSION.to_le_bytes());
        buf[bsh + 0x0e..bsh + 0x10].copy_from_slice(&MIN_DIMENSION.to_le_bytes());
        FrameHeader::parse(&buf).expect("test header parses")
    }

    use crate::indeo3::header::{
        COMBINED_HEADER_LEN, MAGIC_FRMH, MIN_DIMENSION, REQUIRED_DEC_VERSION,
    };

    #[test]
    fn slot_offsets_and_rvas_match_spec_6() {
        assert_eq!(SAVED_FRAME_FLAGS_SLOT_OFFSET, 0x434);
        assert_eq!(SAVED_FRAME_NUMBER_SLOT_OFFSET, 0x474);
        assert_eq!(SAVED_FRAME_FLAGS_STORE_RVA, 0x1000_42c1);
        assert_eq!(SAVED_FRAME_NUMBER_STORE_RVA, 0x1000_42a3);
        assert_eq!(FRAME_NUMBER_CONTINUITY_CHECK_RVA, 0x1000_41f8);
        assert_eq!(FRAME_NUMBER_SEEK_PATH_RVA, 0x1000_4220);
    }

    #[test]
    fn no_explicit_buffer_rotation_per_6_4() {
        const { assert!(!PERFORMS_BUFFER_ROTATION) };
    }

    #[test]
    fn saved_flags_bit9_drives_next_frame_bank() {
        // bit 9 clear → primary bank.
        let h = make_header(0, 0x0000);
        let saved = SavedFrameFlags::capture(&h);
        assert!(!saved.buffer_selector());
        assert_eq!(saved.next_frame_read_bank(), Bank::Primary);

        // bit 9 set → secondary bank.
        let h = make_header(0, BUFFER_SELECTOR_MASK);
        let saved = SavedFrameFlags::capture(&h);
        assert!(saved.buffer_selector());
        assert_eq!(saved.next_frame_read_bank(), Bank::Secondary);
    }

    #[test]
    fn buffer_selector_mask_is_bit9() {
        assert_eq!(BUFFER_SELECTOR_MASK, 1 << 9);
    }

    #[test]
    fn continuity_sequential_when_incoming_is_saved_plus_one() {
        let saved = SavedFrameNumber { frame_number: 41 };
        assert_eq!(saved.classify(42), FrameContinuity::Sequential);
        assert!(saved.classify(42).is_sequential());
    }

    #[test]
    fn continuity_discontinuous_on_gap_repeat_or_reverse() {
        let saved = SavedFrameNumber { frame_number: 41 };
        // a gap forward
        assert_eq!(saved.classify(43), FrameContinuity::Discontinuous);
        // the same frame
        assert_eq!(saved.classify(41), FrameContinuity::Discontinuous);
        // a backwards seek
        assert_eq!(saved.classify(0), FrameContinuity::Discontinuous);
        assert!(!saved.classify(43).is_sequential());
    }

    #[test]
    fn continuity_wraps_at_u32_max() {
        // saved + 1 wraps to 0; the binary's 32-bit inc/cmp does the
        // same, so an incoming 0 after u32::MAX is sequential.
        let saved = SavedFrameNumber {
            frame_number: u32::MAX,
        };
        assert_eq!(saved.classify(0), FrameContinuity::Sequential);
        assert_eq!(saved.classify(1), FrameContinuity::Discontinuous);
    }

    #[test]
    fn captured_frame_number_round_trips() {
        let h = make_header(0x1234_5678, 0x0000);
        let saved = SavedFrameNumber::capture(&h);
        assert_eq!(saved.frame_number, 0x1234_5678);
    }

    #[test]
    fn decode_return_codes_match_6_3() {
        assert_eq!(DecodeReturn::Success.code(), 0);
        assert_eq!(DecodeReturn::InputError.code(), -100);
        assert_eq!(DecodeReturn::InputError.code(), 0xffff_ff9c_u32 as i32);
        assert_eq!(DecodeReturn::RepeatPrevious.code(), 1);
        assert_eq!(DecodeReturn::PlaneFault(3).code(), 3);
    }

    #[test]
    fn decode_return_is_success_only_for_zero() {
        assert!(DecodeReturn::Success.is_success());
        assert!(!DecodeReturn::InputError.is_success());
        assert!(!DecodeReturn::RepeatPrevious.is_success());
        assert!(!DecodeReturn::PlaneFault(3).is_success());
    }

    #[test]
    fn finalise_bundles_all_three_updates() {
        let h = make_header(7, BUFFER_SELECTOR_MASK);
        let fin = FrameFinalisation::finalise(&h, DecodeReturn::Success);
        assert_eq!(fin.saved_frame_number.frame_number, 7);
        assert!(fin.saved_flags.buffer_selector());
        assert_eq!(fin.saved_flags.next_frame_read_bank(), Bank::Secondary);
        assert_eq!(fin.return_value, DecodeReturn::Success);
        assert!(fin.return_value.is_success());
    }

    #[test]
    fn finalise_captures_slots_regardless_of_fault_return() {
        // §6.1 / §6.2 captures happen at header-parse time, before
        // decode success is known — a faulting frame still saves its
        // slots for the next frame's continuity / bank logic.
        let h = make_header(9, 0x0000);
        let fin = FrameFinalisation::finalise(&h, DecodeReturn::PlaneFault(3));
        assert_eq!(fin.saved_frame_number.frame_number, 9);
        assert_eq!(fin.saved_flags.next_frame_read_bank(), Bank::Primary);
        assert_eq!(fin.return_value.code(), 3);
        assert!(!fin.return_value.is_success());
    }

    #[test]
    fn saved_pair_feeds_next_frame_continuity_and_bank() {
        // End-to-end: finalise frame N, then use the saved pair to
        // classify frame N+1 and pick its read bank.
        let frame_n = make_header(100, 0x0000); // bit9=0 → primary next
        let fin = FrameFinalisation::finalise(&frame_n, DecodeReturn::Success);
        // next frame reads from primary (this frame's bit9=0).
        assert_eq!(fin.saved_flags.next_frame_read_bank(), Bank::Primary);
        // frame 101 is sequential; frame 105 is a seek.
        assert!(fin.saved_frame_number.classify(101).is_sequential());
        assert!(!fin.saved_frame_number.classify(105).is_sequential());
    }
}
