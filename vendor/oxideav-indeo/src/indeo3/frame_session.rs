//! Indeo 3 multi-frame decode session: the inter-frame state machine
//! that threads one frame's [`super::FrameFinalisation`] forward into
//! the next frame's continuity / bank decision.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/01-file-header.md` §3.2
//! (the `frame_flags` INTRA / INTER dispatch and the bit-9 buffer
//! selector), §3.3 (the NULL-frame `data_size == 0x80` sentinel), §3.6
//! / §4 (the sequence-continuity check against the saved
//! `[instance+0x474]` frame-number slot) and
//! `spec/07-output-reconstruction.md` §6 (frame finalisation: §6.1 the
//! saved-flags / next-frame bank, §6.2 the saved-frame-number / seek
//! classifier, §6.3 the return code, §6.4 the "no explicit buffer
//! rotation" invariant).
//!
//! ## What this module adds
//!
//! Every earlier round landed the per-frame pieces in isolation:
//!
//! * [`super::decode_frame`] resolves *one* frame's structure
//!   (header → picture layer → cell trees).
//! * [`super::reconstruct_frame`] reconstructs *one* frame's unblocked
//!   (VQ_NULL) pixel subset.
//! * [`super::FrameFinalisation`] captures *one* frame's saved-flags /
//!   saved-frame-number slots and its return code.
//! * [`super::FrameContinuity`] / [`super::SavedFrameNumber::classify`]
//!   classify *one* incoming frame number against *one* saved value.
//!
//! None of them threads the saved state across a *sequence*. That
//! threading is the decoder's actual inter-frame contract: the
//! reference decoder keeps the previous frame's `frame_flags` and
//! `frame_number` in instance state (`[instance+0x434]` / `+0x474`,
//! spec/07 §6.1 / §6.2) and consults them when the *next* frame
//! arrives — to pick the reference bank (spec/05 §4.2) and to detect a
//! seek (spec/01 §3.6 / §4). This module owns that state machine.
//!
//! A [`DecodeSession`] is created empty (no frame seen yet). Each
//! incoming frame is presented to [`DecodeSession::admit`], which:
//!
//! 1. Parses the spec/01 header (the cheap part; the full structural
//!    decode is the caller's [`super::decode_frame`] call once the
//!    session has classified the frame).
//! 2. Classifies the frame against the saved state into a
//!    [`FrameAdmission`]:
//!    * **first frame** must be INTRA (spec/01 §3.2: an INTER first
//!      frame is the `-100` input error) — [`FrameAdmission::FirstFrame`]
//!      on success, [`SessionError::FirstFrameNotIntra`] otherwise.
//!    * a **NULL frame** (`data_size == 0x80`, spec/01 §3.3) repeats
//!      the previous output (spec/07 §6.3 return `1`) —
//!      [`FrameAdmission::NullRepeat`].
//!    * a **sequential** INTRA/INTER frame (`frame_number == saved + 1`,
//!      spec/07 §6.2) — [`FrameAdmission::Sequential`].
//!    * a **discontinuous** frame (a seek / gap, spec/01 §3.6) is
//!      admitted only if it is INTRA (the seek path re-validates the
//!      INTRA requirement, spec/01 §4) — [`FrameAdmission::Seek`] on
//!      success, [`SessionError::SeekNotIntra`] otherwise.
//! 3. On a non-error admission, advances the saved state to *this*
//!    frame (spec/07 §6.1 / §6.2 stores), so the next `admit` sees the
//!    updated continuity baseline. The §6.4 invariant holds: there is
//!    no decoder-side buffer rotation — the bank ping-pong is entirely
//!    encoder-driven through each frame's bit-9.
//!
//! The admission carries the [`super::Bank`] the frame reads its
//! previous-frame reference from (spec/07 §6.1: driven by the *saved*
//! frame's bit-9), the [`super::DecodeReturn`] disposition, and the
//! [`super::FrameContinuity`] classification, so a caller can drive a
//! whole IV31 / IV32 sequence's frame-sequencing without re-deriving
//! the inter-frame rules at each step.
//!
//! ## Scope
//!
//! This module owns the *sequencing*, not the pixels. It does not
//! reconstruct a frame (that is [`super::reconstruct_frame`], gated on
//! the spec/04 §7.1 codebook-bank docs-gap for VQ_DATA), nor does it
//! hold the reference pixel banks (the strip arena, spec/07 §5.1). It
//! tracks exactly the two instance-state slots the spec/07 §6
//! finalisation maintains plus the first-frame / seek INTRA gate, which
//! are entirely table-free and decodable now.

use super::Bank;
use super::frame_finalise::{DecodeReturn, FrameContinuity, FrameFinalisation};
use super::header::{FrameHeader, HeaderError};

/// How an incoming frame was admitted to the session, relative to the
/// previously-decoded frame's saved state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameAdmission {
    /// The first frame of the session. Per spec/01 §3.2 it must be
    /// INTRA (an INTER first frame is rejected with the `-100` input
    /// error). There is no saved state to compare against, so no
    /// continuity classification applies.
    FirstFrame,
    /// A normal sequential frame: `frame_number == saved + 1`
    /// (spec/07 §6.2). The frame may be INTRA or INTER.
    Sequential,
    /// A NULL / sync frame (`data_size == 0x80`, spec/01 §3.3): no
    /// coded picture data; the host re-displays the previous output
    /// (spec/07 §6.3 return `1`). The saved `frame_number` is still
    /// advanced so the next frame's continuity baseline is this NULL
    /// frame's number.
    NullRepeat,
    /// A discontinuous frame (a seek / gap: `frame_number != saved + 1`,
    /// spec/01 §3.6). Admitted only because it is INTRA — the seek path
    /// re-validates the INTRA requirement (spec/01 §4) before
    /// continuing.
    Seek,
}

impl FrameAdmission {
    /// `true` if this frame carries coded picture data that the caller
    /// should structurally decode + reconstruct. A [`Self::NullRepeat`]
    /// frame does not (the previous output is re-displayed).
    pub fn carries_picture(self) -> bool {
        !matches!(self, FrameAdmission::NullRepeat)
    }

    /// `true` if this frame begins a fresh reference chain (an INTRA
    /// key frame the decoder can resynchronise on): the first frame or
    /// a seek (both INTRA by construction).
    pub fn is_resync_point(self) -> bool {
        matches!(self, FrameAdmission::FirstFrame | FrameAdmission::Seek)
    }
}

/// The fully-classified outcome of admitting one frame to the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct AdmittedFrame {
    /// How the frame was admitted relative to the saved state.
    pub admission: FrameAdmission,
    /// The [`super::Bank`] this frame reads its previous-frame
    /// reference data from (spec/07 §6.1 / spec/05 §4.2): driven by the
    /// *saved* (previous) frame's bit-9. For the first frame there is
    /// no previous frame; the read bank is the frame's own bit-9 bank
    /// (the encoder picks a starting bank for the INTRA frame).
    pub read_bank: Bank,
    /// The spec/07 §6.3 return disposition the decoder reports for this
    /// frame (`Success`, `RepeatPrevious` for a NULL frame, …).
    pub return_value: DecodeReturn,
    /// The spec/07 §6.2 continuity classification against the saved
    /// frame number (`None` for the first frame: no baseline yet).
    pub continuity: Option<FrameContinuity>,
    /// The frame's own `frame_number` (spec/01 §2.1).
    pub frame_number: u32,
}

impl AdmittedFrame {
    /// Convenience: did this admission carry coded picture data the
    /// caller should decode? (Delegates to [`FrameAdmission::carries_picture`].)
    pub fn carries_picture(self) -> bool {
        self.admission.carries_picture()
    }
}

/// Errors raised while admitting a frame to a [`DecodeSession`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionError {
    /// The frame's spec/01 header could not be parsed.
    Header(HeaderError),
    /// The first frame of a session was not INTRA. Per spec/01 §3.2 the
    /// decoder rejects an INTER first frame with the `-100` input
    /// error (there is no reference frame for INTER prediction to read
    /// from). Carries the offending frame's `frame_flags` word.
    FirstFrameNotIntra {
        /// The raw `frame_flags` of the rejected first frame.
        frame_flags: u16,
    },
    /// A discontinuous (seek) frame was not INTRA. Per spec/01 §4 the
    /// seek path re-validates the INTRA requirement: a non-INTRA frame
    /// whose number breaks sequence has no valid reference and is
    /// rejected. Carries the saved + incoming frame numbers and the
    /// offending `frame_flags`.
    SeekNotIntra {
        /// The previously-saved frame number.
        saved_frame_number: u32,
        /// The incoming (out-of-sequence) frame number.
        incoming_frame_number: u32,
        /// The raw `frame_flags` of the rejected seek frame.
        frame_flags: u16,
    },
}

impl core::fmt::Display for SessionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SessionError::Header(e) => write!(f, "indeo3 session: header: {e}"),
            SessionError::FirstFrameNotIntra { frame_flags } => write!(
                f,
                "spec/01 §3.2: first frame is INTER (frame_flags {frame_flags:#06x}); \
                 an INTER first frame has no reference (return -100)"
            ),
            SessionError::SeekNotIntra {
                saved_frame_number,
                incoming_frame_number,
                frame_flags,
            } => write!(
                f,
                "spec/01 §4: seek frame {incoming_frame_number} (saved {saved_frame_number}) is \
                 not INTRA (frame_flags {frame_flags:#06x}); a discontinuous non-INTRA frame has \
                 no valid reference"
            ),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<HeaderError> for SessionError {
    fn from(e: HeaderError) -> Self {
        SessionError::Header(e)
    }
}

/// The inter-frame state a [`DecodeSession`] carries between frames:
/// the spec/07 §6.1 / §6.2 saved slots of the previously-admitted
/// frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SavedState {
    finalisation: FrameFinalisation,
}

/// A multi-frame Indeo 3 decode session.
///
/// Holds the spec/07 §6 saved instance-state slots (the previous
/// frame's `frame_flags` at `[instance+0x434]` and `frame_number` at
/// `[instance+0x474]`) and applies the spec/01 §3.2 / §3.6 / §4
/// admission rules to each incoming frame. Create with
/// [`DecodeSession::new`]; feed frames with [`DecodeSession::admit`].
#[derive(Debug, Clone, Default)]
pub struct DecodeSession {
    saved: Option<SavedState>,
}

impl DecodeSession {
    /// Create a fresh session with no frames admitted yet.
    pub fn new() -> Self {
        DecodeSession { saved: None }
    }

    /// `true` if no frame has been admitted yet (the next frame is the
    /// first, which must be INTRA).
    pub fn is_empty(&self) -> bool {
        self.saved.is_none()
    }

    /// The previously-admitted frame's saved `frame_number`
    /// (spec/07 §6.2 `[instance+0x474]`), or `None` if the session is
    /// empty.
    pub fn saved_frame_number(&self) -> Option<u32> {
        self.saved
            .map(|s| s.finalisation.saved_frame_number.frame_number)
    }

    /// The [`super::Bank`] the *next* frame would read from, driven by
    /// the previously-admitted frame's bit-9 (spec/07 §6.1). `None` if
    /// the session is empty.
    pub fn next_read_bank(&self) -> Option<Bank> {
        self.saved
            .map(|s| s.finalisation.saved_flags.next_frame_read_bank())
    }

    /// Spec/01 §3.2 / §3.3 / §3.6 / §4 + spec/07 §6 — admit one frame.
    ///
    /// Parses `input`'s spec/01 header, classifies the frame against the
    /// session's saved state, and — on a successful admission — advances
    /// the saved state to this frame (the spec/07 §6.1 / §6.2 stores).
    ///
    /// The returned [`AdmittedFrame`] carries the admission kind, the
    /// reference [`super::Bank`] the frame reads from, the spec/07 §6.3
    /// return disposition, and the spec/07 §6.2 continuity
    /// classification. A rejected frame ([`SessionError`]) leaves the
    /// saved state untouched (the reference decoder does not advance its
    /// continuity baseline past an input-error frame).
    pub fn admit(&mut self, input: &[u8]) -> Result<AdmittedFrame, SessionError> {
        let header = FrameHeader::parse(input)?;
        let flags = header.bitstream.frame_flags;
        let frame_number = header.frame.frame_number;
        let is_null = header.bitstream.is_null_frame();

        let admitted = match self.saved {
            None => self.admit_first(&header, flags, is_null)?,
            Some(saved) => self.admit_subsequent(saved, flags, frame_number, is_null)?,
        };

        // Spec/07 §6.1 / §6.2 — advance the saved instance-state slots to
        // this frame. The stores happen during header processing
        // (before decode success is known, spec/07 §6 note), so even a
        // NULL / repeat frame updates the continuity baseline.
        let return_value = admitted.return_value;
        self.saved = Some(SavedState {
            finalisation: FrameFinalisation::finalise(&header, return_value),
        });

        Ok(admitted)
    }

    /// Classify the *first* frame of a session (no saved state).
    fn admit_first(
        &self,
        header: &FrameHeader,
        flags: super::header::FrameFlags,
        is_null: bool,
    ) -> Result<AdmittedFrame, SessionError> {
        // A NULL first frame has no previous output to repeat, and an
        // INTER first frame has no reference — both are the §3.2 input
        // error. The reference decoder requires the first frame to be a
        // real INTRA key frame.
        if is_null || !flags.intra() {
            return Err(SessionError::FirstFrameNotIntra {
                frame_flags: flags.bits(),
            });
        }
        // The first frame reads from its own bit-9 bank: there is no
        // previous frame, so the encoder's chosen starting bank governs.
        let read_bank = Bank::from_buffer_selector(flags);
        Ok(AdmittedFrame {
            admission: FrameAdmission::FirstFrame,
            read_bank,
            return_value: DecodeReturn::Success,
            continuity: None,
            frame_number: header.frame.frame_number,
        })
    }

    /// Classify a frame that follows a previously-admitted one.
    fn admit_subsequent(
        &self,
        saved: SavedState,
        flags: super::header::FrameFlags,
        frame_number: u32,
        is_null: bool,
    ) -> Result<AdmittedFrame, SessionError> {
        // Spec/07 §6.1 — the reference bank this frame reads from is
        // driven by the *saved* (previous) frame's bit-9, regardless of
        // this frame's own continuity.
        let read_bank = saved.finalisation.saved_flags.next_frame_read_bank();
        let continuity = saved.finalisation.saved_frame_number.classify(frame_number);

        // Spec/01 §3.3 — a NULL frame repeats the previous output
        // (spec/07 §6.3 return 1) regardless of continuity.
        if is_null {
            return Ok(AdmittedFrame {
                admission: FrameAdmission::NullRepeat,
                read_bank,
                return_value: DecodeReturn::RepeatPrevious,
                continuity: Some(continuity),
                frame_number,
            });
        }

        match continuity {
            FrameContinuity::Sequential => Ok(AdmittedFrame {
                admission: FrameAdmission::Sequential,
                read_bank,
                return_value: DecodeReturn::Success,
                continuity: Some(continuity),
                frame_number,
            }),
            FrameContinuity::Discontinuous => {
                // Spec/01 §4 — the seek path re-validates the INTRA
                // requirement. A discontinuous INTER frame has no valid
                // reference and is rejected.
                if !flags.intra() {
                    return Err(SessionError::SeekNotIntra {
                        saved_frame_number: saved.finalisation.saved_frame_number.frame_number,
                        incoming_frame_number: frame_number,
                        frame_flags: flags.bits(),
                    });
                }
                Ok(AdmittedFrame {
                    admission: FrameAdmission::Seek,
                    read_bank,
                    return_value: DecodeReturn::Success,
                    continuity: Some(continuity),
                    frame_number,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::header::{
        COMBINED_HEADER_LEN, FRAME_HEADER_LEN, MAGIC_FRMH, NULL_FRAME_DATA_SIZE_BITS,
        REQUIRED_DEC_VERSION,
    };

    /// Build a minimal valid combined header for a frame of 64×64 with
    /// the given `frame_number`, `frame_flags`, and `data_size` (bits).
    /// The frame's three plane offsets are all skipped (high bit set)
    /// so the structural decode — which this module does not run — is
    /// irrelevant; only the spec/01 header fields matter here.
    fn make_frame(frame_number: u32, frame_flags: u16, data_size_bits: u32) -> Vec<u8> {
        let mut buf = vec![0u8; COMBINED_HEADER_LEN];
        let unknown1: u32 = 0;
        let frame_size: u32 = COMBINED_HEADER_LEN as u32;
        let check_sum = frame_number ^ unknown1 ^ frame_size ^ MAGIC_FRMH;
        buf[0x00..0x04].copy_from_slice(&frame_number.to_le_bytes());
        buf[0x04..0x08].copy_from_slice(&unknown1.to_le_bytes());
        buf[0x08..0x0c].copy_from_slice(&check_sum.to_le_bytes());
        buf[0x0c..0x10].copy_from_slice(&frame_size.to_le_bytes());
        let b = FRAME_HEADER_LEN;
        buf[b..b + 2].copy_from_slice(&REQUIRED_DEC_VERSION.to_le_bytes());
        buf[b + 2..b + 4].copy_from_slice(&frame_flags.to_le_bytes());
        buf[b + 4..b + 8].copy_from_slice(&data_size_bits.to_le_bytes());
        buf[b + 0x0c..b + 0x0e].copy_from_slice(&64u16.to_le_bytes());
        buf[b + 0x0e..b + 0x10].copy_from_slice(&64u16.to_le_bytes());
        buf
    }

    const INTRA: u16 = 0x0004;
    const BUF_SEL: u16 = 0x0200;
    const DATA: u32 = 4096; // a non-null bit count

    #[test]
    fn empty_session_has_no_saved_state() {
        let s = DecodeSession::new();
        assert!(s.is_empty());
        assert_eq!(s.saved_frame_number(), None);
        assert_eq!(s.next_read_bank(), None);
    }

    #[test]
    fn first_frame_must_be_intra() {
        let mut s = DecodeSession::new();
        // INTER first frame → rejected with the §3.2 input error.
        let inter = make_frame(0, 0x0000, DATA);
        let err = s.admit(&inter).unwrap_err();
        assert_eq!(
            err,
            SessionError::FirstFrameNotIntra {
                frame_flags: 0x0000
            }
        );
        // The session stays empty — the baseline is not advanced past an
        // input-error frame.
        assert!(s.is_empty());
    }

    #[test]
    fn null_first_frame_is_rejected() {
        let mut s = DecodeSession::new();
        let null = make_frame(0, INTRA, NULL_FRAME_DATA_SIZE_BITS);
        let err = s.admit(&null).unwrap_err();
        assert!(matches!(err, SessionError::FirstFrameNotIntra { .. }));
        assert!(s.is_empty());
    }

    #[test]
    fn first_intra_frame_is_admitted_and_sets_baseline() {
        let mut s = DecodeSession::new();
        let intra = make_frame(0, INTRA, DATA);
        let a = s.admit(&intra).expect("first INTRA admits");
        assert_eq!(a.admission, FrameAdmission::FirstFrame);
        assert_eq!(a.return_value, DecodeReturn::Success);
        assert_eq!(a.continuity, None);
        assert_eq!(a.frame_number, 0);
        assert!(a.carries_picture());
        assert!(a.admission.is_resync_point());
        // Baseline advanced.
        assert!(!s.is_empty());
        assert_eq!(s.saved_frame_number(), Some(0));
        // bit-9 clear → next frame reads primary.
        assert_eq!(s.next_read_bank(), Some(Bank::Primary));
    }

    #[test]
    fn first_frame_read_bank_follows_own_bit9() {
        let mut s = DecodeSession::new();
        let intra = make_frame(0, INTRA | BUF_SEL, DATA);
        let a = s.admit(&intra).expect("admits");
        // First frame has no previous frame; its read bank is its own
        // bit-9 bank.
        assert_eq!(a.read_bank, Bank::Secondary);
    }

    #[test]
    fn sequential_inter_frame_is_admitted() {
        let mut s = DecodeSession::new();
        s.admit(&make_frame(0, INTRA, DATA)).expect("first");
        // frame 1 is INTER and sequential.
        let a = s.admit(&make_frame(1, 0x0000, DATA)).expect("seq inter");
        assert_eq!(a.admission, FrameAdmission::Sequential);
        assert_eq!(a.continuity, Some(FrameContinuity::Sequential));
        assert_eq!(a.return_value, DecodeReturn::Success);
        assert!(a.carries_picture());
        assert!(!a.admission.is_resync_point());
        assert_eq!(s.saved_frame_number(), Some(1));
    }

    #[test]
    fn read_bank_follows_previous_frame_bit9() {
        let mut s = DecodeSession::new();
        // First frame bit-9 set → the NEXT frame reads secondary.
        s.admit(&make_frame(0, INTRA | BUF_SEL, DATA))
            .expect("first");
        let a = s.admit(&make_frame(1, 0x0000, DATA)).expect("seq");
        assert_eq!(a.read_bank, Bank::Secondary);
        // This (frame 1) bit-9 clear → frame 2 reads primary.
        let a = s.admit(&make_frame(2, 0x0000, DATA)).expect("seq");
        assert_eq!(a.read_bank, Bank::Primary);
    }

    #[test]
    fn null_frame_repeats_previous_output() {
        let mut s = DecodeSession::new();
        s.admit(&make_frame(0, INTRA, DATA)).expect("first");
        // A NULL frame at number 1 (sequential) repeats the prior output.
        let a = s
            .admit(&make_frame(1, 0x0000, NULL_FRAME_DATA_SIZE_BITS))
            .expect("null repeat");
        assert_eq!(a.admission, FrameAdmission::NullRepeat);
        assert_eq!(a.return_value, DecodeReturn::RepeatPrevious);
        assert_eq!(a.return_value.code(), 1);
        assert!(!a.carries_picture());
        // The continuity baseline still advances to the NULL frame.
        assert_eq!(s.saved_frame_number(), Some(1));
    }

    #[test]
    fn null_frame_advances_baseline_even_out_of_sequence() {
        let mut s = DecodeSession::new();
        s.admit(&make_frame(0, INTRA, DATA)).expect("first");
        // A NULL frame whose number breaks sequence is still admitted
        // (NULL repeats output regardless of continuity) and advances
        // the baseline.
        let a = s
            .admit(&make_frame(9, 0x0000, NULL_FRAME_DATA_SIZE_BITS))
            .expect("null");
        assert_eq!(a.admission, FrameAdmission::NullRepeat);
        assert_eq!(a.continuity, Some(FrameContinuity::Discontinuous));
        assert_eq!(s.saved_frame_number(), Some(9));
    }

    #[test]
    fn seek_to_intra_frame_is_admitted() {
        let mut s = DecodeSession::new();
        s.admit(&make_frame(0, INTRA, DATA)).expect("first");
        // Jump to frame 10 (a gap) carrying INTRA → admitted as a seek.
        let a = s.admit(&make_frame(10, INTRA, DATA)).expect("seek intra");
        assert_eq!(a.admission, FrameAdmission::Seek);
        assert_eq!(a.continuity, Some(FrameContinuity::Discontinuous));
        assert_eq!(a.return_value, DecodeReturn::Success);
        assert!(a.admission.is_resync_point());
        assert_eq!(s.saved_frame_number(), Some(10));
    }

    #[test]
    fn seek_to_inter_frame_is_rejected() {
        let mut s = DecodeSession::new();
        s.admit(&make_frame(0, INTRA, DATA)).expect("first");
        // Jump to frame 10 carrying INTER → no valid reference, rejected.
        let err = s.admit(&make_frame(10, 0x0000, DATA)).unwrap_err();
        assert_eq!(
            err,
            SessionError::SeekNotIntra {
                saved_frame_number: 0,
                incoming_frame_number: 10,
                frame_flags: 0x0000,
            }
        );
        // The baseline is NOT advanced past the rejected frame.
        assert_eq!(s.saved_frame_number(), Some(0));
    }

    #[test]
    fn periodic_intra_counts_as_intra_for_seek() {
        let mut s = DecodeSession::new();
        s.admit(&make_frame(0, INTRA, DATA)).expect("first");
        // Bit 0 (PERIODIC_INTRA) alone does NOT set bit 2 (INTRA);
        // per spec/01 §3.2 the INTRA gate tests bit 2 specifically.
        // A periodic-only seek frame must therefore be rejected.
        let err = s.admit(&make_frame(5, 0x0001, DATA)).unwrap_err();
        assert!(matches!(err, SessionError::SeekNotIntra { .. }));
        // But a frame carrying both bit 0 and bit 2 is INTRA → admitted.
        let a = s.admit(&make_frame(5, 0x0005, DATA)).expect("intra seek");
        assert_eq!(a.admission, FrameAdmission::Seek);
    }

    #[test]
    fn malformed_header_is_typed_error() {
        let mut s = DecodeSession::new();
        let err = s.admit(&[0u8; 4]).unwrap_err();
        assert!(matches!(err, SessionError::Header(_)));
        assert!(s.is_empty());
    }

    #[test]
    fn full_sequence_threads_state() {
        // INTRA(0) → INTER(1) → NULL(2) → INTER(3) → seek INTRA(20).
        let mut s = DecodeSession::new();

        let a0 = s.admit(&make_frame(0, INTRA, DATA)).expect("0");
        assert_eq!(a0.admission, FrameAdmission::FirstFrame);

        let a1 = s.admit(&make_frame(1, 0x0000, DATA)).expect("1");
        assert_eq!(a1.admission, FrameAdmission::Sequential);

        let a2 = s
            .admit(&make_frame(2, 0x0000, NULL_FRAME_DATA_SIZE_BITS))
            .expect("2");
        assert_eq!(a2.admission, FrameAdmission::NullRepeat);

        let a3 = s.admit(&make_frame(3, 0x0000, DATA)).expect("3");
        assert_eq!(a3.admission, FrameAdmission::Sequential);

        let a4 = s.admit(&make_frame(20, INTRA, DATA)).expect("20");
        assert_eq!(a4.admission, FrameAdmission::Seek);

        assert_eq!(s.saved_frame_number(), Some(20));
    }
}
