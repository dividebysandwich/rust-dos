//! Indeo 3 stateful multi-frame decoder: the session-driven driver that
//! turns a sequence of IV31 / IV32 codec frames into a sequence of
//! reconstructed output frames, holding the previous frame so NULL /
//! repeat frames re-emit it.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/01-file-header.md` §3
//! (header / NULL-frame / continuity), `spec/02` (picture layer),
//! `spec/03` (cell trees), `spec/07-output-reconstruction.md` §1.5 /
//! §5.2 (frame reconstruction) and §6 (finalisation / repeat-previous).
//!
//! ## What this module adds
//!
//! [`super::DecodeSession`] classifies each frame's *sequencing* (first
//! / sequential / NULL-repeat / seek) and tracks the spec/07 §6 saved
//! slots, but it produces no pixels. [`super::decode_frame`] /
//! [`super::reconstruct_frame`] produce one frame's structure / pixels
//! but hold no inter-frame state. This module joins the two: an
//! [`Indeo3Decoder`] owns a [`super::DecodeSession`] *and* the previous
//! [`super::ReconstructedFrame`], so it can implement the spec/07 §6.3
//! repeat-previous semantics — a NULL frame's output *is* the prior
//! frame's reconstruction, re-emitted byte-for-byte.
//!
//! [`Indeo3Decoder::decode`] runs the full per-frame pipeline:
//!
//! 1. Admit the frame through the session ([`super::DecodeSession::admit`]):
//!    classify continuity, enforce the first-frame / seek INTRA gate,
//!    detect NULL frames, resolve the reference bank.
//! 2. For a **picture-carrying** frame (first / sequential / seek),
//!    structurally decode it ([`super::decode_frame_with_selector`]
//!    against the admission's read bank) and reconstruct the unblocked
//!    (VQ_NULL) subset ([`super::reconstruct_frame`]). The result
//!    becomes the new "previous frame".
//! 3. For a **NULL-repeat** frame, re-emit the held previous frame's
//!    reconstruction (spec/07 §6.3 return `1`). A NULL frame before any
//!    picture frame is impossible — the session rejects a NULL first
//!    frame as the §3.2 input error — so the held frame always exists
//!    when a NULL frame is admitted.
//!
//! The returned [`DecodedOutput`] bundles the [`super::AdmittedFrame`]
//! classification with a borrow of the reconstructed frame, so a caller
//! can drive a whole sequence and pull each frame's
//! [`super::OutputFrame`] via [`super::ReconstructedFrame::to_output_frame`].
//!
//! ## Scope
//!
//! This decoder reconstructs only the genuinely-unblocked (VQ_NULL)
//! subset of each picture frame — the VQ_DATA / INTER regions stay black
//! pending the spec/04 §7.1 codebook-bank docs-gap, exactly as
//! [`super::reconstruct_frame`] does. Its contribution is the
//! inter-frame *sequencing* + repeat-previous output, which is entirely
//! table-free and decodable now. It does not own the host buffer
//! handoff (the VfW `ICDecompress` envelope) nor the YUV→RGB conversion
//! (spec/07 §5.4 LUT docs-gap).

use super::Bank;
use super::frame::{FrameDecodeError, decode_frame};
use super::frame_assemble::OutputFrame;
use super::frame_reconstruct::{FrameReconstructError, ReconstructedFrame, reconstruct_frame};
use super::frame_session::{AdmittedFrame, DecodeSession, FrameAdmission, SessionError};
use super::frame_yuv::{YuvError, YuvFrame, upsample_frame};

/// Errors raised while decoding one frame through an [`Indeo3Decoder`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecoderError {
    /// The frame's sequencing was rejected by the session (first-frame
    /// or seek INTRA requirement, or a malformed header).
    Session(SessionError),
    /// The frame's structural decode (spec/01 → spec/03) failed.
    Decode(FrameDecodeError),
    /// The frame's reconstruction (spec/07) failed.
    Reconstruct(FrameReconstructError),
}

impl core::fmt::Display for DecoderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DecoderError::Session(e) => write!(f, "indeo3 decoder: {e}"),
            DecoderError::Decode(e) => write!(f, "indeo3 decoder: {e}"),
            DecoderError::Reconstruct(e) => write!(f, "indeo3 decoder: {e}"),
        }
    }
}

impl std::error::Error for DecoderError {}

impl From<SessionError> for DecoderError {
    fn from(e: SessionError) -> Self {
        DecoderError::Session(e)
    }
}

impl From<FrameDecodeError> for DecoderError {
    fn from(e: FrameDecodeError) -> Self {
        DecoderError::Decode(e)
    }
}

impl From<FrameReconstructError> for DecoderError {
    fn from(e: FrameReconstructError) -> Self {
        DecoderError::Reconstruct(e)
    }
}

/// The result of decoding one frame through an [`Indeo3Decoder`]: the
/// session admission plus a borrow of the reconstructed output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodedOutput<'a> {
    /// The session's sequencing classification for this frame.
    pub admission: AdmittedFrame,
    /// `true` if this frame's output was *re-emitted* from the previous
    /// frame (a NULL / repeat frame, spec/07 §6.3), rather than freshly
    /// reconstructed. For a picture-carrying frame this is `false`.
    pub repeated_previous: bool,
    /// The reconstructed frame: freshly reconstructed for a
    /// picture-carrying frame, or the held previous frame re-emitted for
    /// a NULL / repeat frame.
    pub frame: &'a ReconstructedFrame,
}

impl DecodedOutput<'_> {
    /// The reference [`Bank`] this frame read its previous-frame data
    /// from (delegates to the admission).
    pub fn read_bank(&self) -> Bank {
        self.admission.read_bank
    }

    /// `true` if this frame begins a fresh INTRA reference chain (first
    /// frame or seek).
    pub fn is_resync_point(&self) -> bool {
        self.admission.admission.is_resync_point()
    }

    /// Spec/07 §4.3 / §5.6 — assemble this frame's reconstructed strip
    /// buffers into an [`OutputFrame`] of tightly-packed 8-bit planes
    /// (the unblocked subset's pixels in place, deferred regions black).
    /// Convenience passthrough to [`ReconstructedFrame::to_output_frame`].
    pub fn to_output_frame(&self) -> OutputFrame {
        self.frame.to_output_frame()
    }

    /// Spec/07 §5.5 / §5.6 — produce a full-luma-resolution
    /// [`YuvFrame`] from this frame: the §5.7 strip-to-frame assembly
    /// followed by the §5.5 4:1:0 chroma box-upsample. The one-call
    /// path from a decoded frame to a displayable three-plane YUV
    /// surface (the §5.4-RGB-independent half of output conversion).
    pub fn to_yuv_frame(&self) -> Result<YuvFrame, YuvError> {
        upsample_frame(&self.frame.to_output_frame())
    }
}

/// A stateful Indeo 3 (IV31 / IV32) multi-frame decoder.
///
/// Owns the [`DecodeSession`] (inter-frame sequencing state) and the
/// previous [`ReconstructedFrame`] (for the spec/07 §6.3 repeat-previous
/// path). Create with [`Indeo3Decoder::new`]; feed frames in order with
/// [`Indeo3Decoder::decode`].
#[derive(Debug, Clone, Default)]
pub struct Indeo3Decoder {
    session: DecodeSession,
    previous: Option<ReconstructedFrame>,
}

impl Indeo3Decoder {
    /// Create a fresh decoder with no frames decoded yet.
    pub fn new() -> Self {
        Indeo3Decoder {
            session: DecodeSession::new(),
            previous: None,
        }
    }

    /// `true` if no frame has been decoded yet (the next frame is the
    /// first and must be INTRA).
    pub fn is_empty(&self) -> bool {
        self.session.is_empty()
    }

    /// Borrow the previously-decoded reconstructed frame, if any.
    pub fn previous_frame(&self) -> Option<&ReconstructedFrame> {
        self.previous.as_ref()
    }

    /// Spec/01 §3 + spec/07 §1.5 / §5.2 / §6 — decode one frame.
    ///
    /// Admits the frame through the session (sequencing + INTRA gate +
    /// NULL detection), then:
    ///
    /// * **picture-carrying** (first / sequential / seek): structurally
    ///   decodes the frame against the admission's read bank and
    ///   reconstructs its unblocked subset, storing the result as the
    ///   new previous frame.
    /// * **NULL-repeat**: re-emits the held previous frame (spec/07 §6.3
    ///   return `1`).
    ///
    /// Returns a [`DecodedOutput`] borrowing the reconstructed frame, or
    /// a [`DecoderError`]. A rejected frame leaves the decoder state
    /// (session baseline + held frame) unchanged.
    pub fn decode(&mut self, input: &[u8]) -> Result<DecodedOutput<'_>, DecoderError> {
        let admitted = self.session.admit(input)?;

        match admitted.admission {
            FrameAdmission::NullRepeat => {
                // The session guarantees a NULL frame is never the first
                // frame (a NULL first frame is the §3.2 input error), so
                // a previous frame always exists here. Re-emit it.
                debug_assert!(
                    self.previous.is_some(),
                    "session admitted a NULL-repeat frame with no previous frame"
                );
                let frame = self
                    .previous
                    .as_ref()
                    .expect("NULL-repeat frame has a held previous frame");
                Ok(DecodedOutput {
                    admission: admitted,
                    repeated_previous: true,
                    frame,
                })
            }
            _ => {
                // The structural decode uses the frame's *own* bit-9
                // buffer selector (which bank the per-plane decoder
                // writes/reads through, spec/02 §3.2 / §5.1); the
                // admission's `read_bank` (driven by the *previous*
                // frame's bit-9, spec/07 §6.1) is the inter-frame
                // reference-bank report, surfaced on the admission.
                let decoded = decode_frame(input)?;
                let reconstructed = reconstruct_frame(&decoded)?;
                self.previous = Some(reconstructed);
                let frame = self
                    .previous
                    .as_ref()
                    .expect("just-stored reconstructed frame");
                Ok(DecodedOutput {
                    admission: admitted,
                    repeated_previous: false,
                    frame,
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

    const INTRA: u16 = 0x0004;
    const BUF_SEL: u16 = 0x0200;

    /// Build a frame whose three planes are all skipped (high bit set),
    /// so the structural decode produces a `DecodedFrame` with no planes
    /// — the reconstruction is an empty frame. This keeps the decoder
    /// test focused on the inter-frame sequencing + repeat-previous path
    /// (the per-plane reconstruction is exercised by
    /// frame_reconstruct.rs / plane_execute.rs).
    fn skipped_frame(frame_number: u32, frame_flags: u16, data_size_bits: u32) -> Vec<u8> {
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
        // All three plane offsets negative → every plane skipped.
        let neg = 0x8000_0000u32;
        buf[b + 0x10..b + 0x14].copy_from_slice(&neg.to_le_bytes());
        buf[b + 0x14..b + 0x18].copy_from_slice(&neg.to_le_bytes());
        buf[b + 0x18..b + 0x1c].copy_from_slice(&neg.to_le_bytes());
        buf
    }

    const DATA: u32 = 4096;

    #[test]
    fn new_decoder_is_empty() {
        let d = Indeo3Decoder::new();
        assert!(d.is_empty());
        assert!(d.previous_frame().is_none());
    }

    #[test]
    fn first_inter_frame_is_rejected() {
        let mut d = Indeo3Decoder::new();
        let err = d.decode(&skipped_frame(0, 0x0000, DATA)).unwrap_err();
        assert!(matches!(
            err,
            DecoderError::Session(SessionError::FirstFrameNotIntra { .. })
        ));
        // Decoder unchanged.
        assert!(d.is_empty());
        assert!(d.previous_frame().is_none());
    }

    #[test]
    fn first_intra_frame_decodes_and_is_held() {
        let mut d = Indeo3Decoder::new();
        let out = d.decode(&skipped_frame(0, INTRA, DATA)).expect("first");
        assert_eq!(out.admission.admission, FrameAdmission::FirstFrame);
        assert!(!out.repeated_previous);
        assert!(out.frame.is_empty()); // all planes skipped
        assert!(out.is_resync_point());
        // Held as previous.
        assert!(!d.is_empty());
        assert!(d.previous_frame().is_some());
    }

    #[test]
    fn null_frame_repeats_previous_output() {
        let mut d = Indeo3Decoder::new();
        d.decode(&skipped_frame(0, INTRA, DATA)).expect("first");
        let prev_ptr = d.previous_frame().cloned();
        // Frame 1 is NULL → re-emits the previous reconstruction.
        let out = d
            .decode(&skipped_frame(1, 0x0000, NULL_FRAME_DATA_SIZE_BITS))
            .expect("null repeat");
        assert_eq!(out.admission.admission, FrameAdmission::NullRepeat);
        assert!(out.repeated_previous);
        assert_eq!(out.admission.return_value.code(), 1);
        // The re-emitted frame equals the held previous frame.
        assert_eq!(Some(out.frame.clone()), prev_ptr);
    }

    #[test]
    fn sequential_frames_each_replace_previous() {
        let mut d = Indeo3Decoder::new();
        let o0 = d.decode(&skipped_frame(0, INTRA, DATA)).expect("0");
        assert!(!o0.repeated_previous);
        let o1 = d.decode(&skipped_frame(1, 0x0000, DATA)).expect("1");
        assert_eq!(o1.admission.admission, FrameAdmission::Sequential);
        assert!(!o1.repeated_previous);
    }

    #[test]
    fn read_bank_follows_previous_frame_bit9() {
        let mut d = Indeo3Decoder::new();
        // First frame bit-9 set → next frame reads secondary.
        d.decode(&skipped_frame(0, INTRA | BUF_SEL, DATA))
            .expect("first");
        let out = d.decode(&skipped_frame(1, 0x0000, DATA)).expect("seq");
        assert_eq!(out.read_bank(), Bank::Secondary);
    }

    #[test]
    fn seek_to_inter_is_rejected_and_keeps_previous() {
        let mut d = Indeo3Decoder::new();
        d.decode(&skipped_frame(0, INTRA, DATA)).expect("first");
        let held = d.previous_frame().cloned();
        // Seek (frame 10) carrying INTER → rejected.
        let err = d.decode(&skipped_frame(10, 0x0000, DATA)).unwrap_err();
        assert!(matches!(
            err,
            DecoderError::Session(SessionError::SeekNotIntra { .. })
        ));
        // The held previous frame is unchanged.
        assert_eq!(d.previous_frame().cloned(), held);
    }

    #[test]
    fn output_and_yuv_convenience_passthroughs() {
        let mut d = Indeo3Decoder::new();
        let out = d.decode(&skipped_frame(0, INTRA, DATA)).expect("first");
        // Skipped planes → empty output / yuv, but both calls succeed
        // and agree with the underlying ReconstructedFrame.
        let output = out.to_output_frame();
        assert_eq!(output, out.frame.to_output_frame());
        assert!(output.planes.is_empty());
        let yuv = out.to_yuv_frame().expect("yuv assembles");
        assert!(yuv.planes.is_empty());
    }

    #[test]
    fn full_sequence_drives_output() {
        // INTRA(0) → NULL(1, repeat) → INTER(2) → seek INTRA(20).
        let mut d = Indeo3Decoder::new();

        let o0 = d.decode(&skipped_frame(0, INTRA, DATA)).expect("0");
        assert_eq!(o0.admission.admission, FrameAdmission::FirstFrame);

        let o1 = d
            .decode(&skipped_frame(1, 0x0000, NULL_FRAME_DATA_SIZE_BITS))
            .expect("1");
        assert!(o1.repeated_previous);

        let o2 = d.decode(&skipped_frame(2, 0x0000, DATA)).expect("2");
        assert_eq!(o2.admission.admission, FrameAdmission::Sequential);
        assert!(!o2.repeated_previous);

        let o3 = d.decode(&skipped_frame(20, INTRA, DATA)).expect("20");
        assert_eq!(o3.admission.admission, FrameAdmission::Seek);
        assert!(o3.is_resync_point());
    }
}
