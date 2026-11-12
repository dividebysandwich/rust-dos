//! Indeo 5 multi-frame decode session.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/01 §3`, `spec/07 §1/§4`,
//! `spec/08 §6/§8`.
//!
//! [`Indeo5Decoder`] carries the inter-frame state a single-frame call
//! cannot: the GOP header from the last INTRA frame (INTER frames
//! carry no GOP, `spec/01 §3.5`), the previous frame's per-band
//! coefficient buffers (the `spec/07 §1.2` reference workspace the
//! band-coefficient-layer predictor reads), the previous
//! `frame_number` (the `spec/01 §3.4` duplicate-number NULL
//! soft-correction), and the held host buffer a NULL frame re-emits
//! (`spec/08 §6.4`).
//!
//! Per-frame reference-promotion follows the `spec/08 §8.1` rotation
//! table ([`super::reference_rotation`]): INTRA / INTER promote the
//! just-decoded band buffers to the reference; DROPPABLE_INTER_SCAL
//! promotes with the chroma-pair swap; DROPPABLE_INTER and NULL do
//! not promote (`spec/07 §1.5` — no later frame references a
//! droppable frame). The [`super::RefSlots`] token model mirrors the
//! `spec/07 §1.2`/`§1.3` slot dispatches alongside.
//!
//! The reachable decode subset is the structural one documented in
//! [`super::decode_intra_picture`]: coded-block coefficient data and
//! preset-codebook VLC fields surface as
//! [`DecodeFrontier`](super::DecodeFrontier) records rather than
//! pixels; the motion-compensated predictor copy (skip-inherited and
//! explicitly-decoded MVs over the band-coefficient layer) is real.

use super::ChecksumStatus;
use super::decode::{
    BandReconstruction, DecodeError, DecodeFrontier, DecodeStats, decode_payload, output_format,
    reconstruction_checksums,
};
use super::finalise::{ReferenceRotation, reference_rotation};
use super::format::OutputFormat;
use super::gop::GopHeader;
use super::header::FrameType;
use super::pack::HostBuffer;
use super::picture::{PictureError, PictureHeader};
use super::refbuf::RefSlots;
use super::wavelet::Band;
use super::{AssembleError, assemble_frame};

/// Errors raised by the multi-frame session.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionError {
    /// The first frame of a session must be INTRA (`spec/01 §3.2` —
    /// nothing to predict from or repeat).
    FirstFrameNotIntra {
        /// The frame type found.
        found: FrameType,
    },
    /// Single-frame decode fault.
    Decode(DecodeError),
}

impl From<DecodeError> for SessionError {
    fn from(e: DecodeError) -> Self {
        SessionError::Decode(e)
    }
}
impl From<PictureError> for SessionError {
    fn from(e: PictureError) -> Self {
        SessionError::Decode(DecodeError::Picture(e))
    }
}
impl From<AssembleError> for SessionError {
    fn from(e: AssembleError) -> Self {
        SessionError::Decode(DecodeError::Assemble(e))
    }
}

impl core::fmt::Display for SessionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SessionError::FirstFrameNotIntra { found } => write!(
                f,
                "indeo5 session: first frame must be INTRA, got {found:?} (spec/01 §3.2)"
            ),
            SessionError::Decode(e) => write!(f, "indeo5 session: {e}"),
        }
    }
}

impl std::error::Error for SessionError {}

/// One decoded frame out of the session.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SessionOutput {
    /// The frame type (after the `spec/01 §3.4` soft-correction).
    pub frame_type: FrameType,
    /// `true` when this was a NULL frame re-emitting the previous
    /// output byte-for-byte (`spec/08 §6.4`).
    pub repeated_previous: bool,
    /// The host output format.
    pub format: OutputFormat,
    /// The coded luma dimensions `(width, height)` of this frame
    /// (`spec/02 §1.6`). Carried through NULL / INTER frames from the
    /// governing INTRA GOP so a consumer can shape the planar
    /// [`output`](Self::output) into a strided surface without
    /// re-parsing the header.
    pub dimensions: (u32, u32),
    /// The assembled planar host buffer.
    pub output: HostBuffer,
    /// Gated elements encountered this frame (empty for NULL).
    pub frontiers: Vec<DecodeFrontier>,
    /// Structural counts for this frame (zeros for NULL).
    pub stats: DecodeStats,
    /// Per-band decoded-coefficient work list + `spec/08 §7`
    /// reconstruction-checksum status (empty for NULL / repeat frames).
    pub bands: Vec<BandReconstruction>,
    /// The recomputed-vs-stored frame checksum (`spec/08 §7.1`,
    /// [`ChecksumStatus::Absent`] for NULL).
    pub frame_checksum: ChecksumStatus,
    /// `false` when parsing stopped at an unskippable frontier.
    pub parse_complete: bool,
}

/// Stateful multi-frame Indeo 5 decoder (`spec/01 §3` + `spec/07
/// §1/§4` + `spec/08 §6/§8`).
#[derive(Debug, Clone, Default)]
pub struct Indeo5Decoder {
    /// The GOP from the last INTRA frame (INTER frames reuse it).
    gop: Option<GopHeader>,
    /// Previous frame number (`spec/01 §3.4` soft-correction input).
    prev_frame_number: Option<u8>,
    /// The reference per-plane per-band coefficient buffers
    /// (`spec/07 §1.2`).
    reference: Vec<Vec<Band>>,
    /// The last emitted output (NULL frames re-emit it).
    held: Option<(OutputFormat, HostBuffer)>,
    /// The `spec/07 §1.2`/`§1.3` buffer-slot rotation model.
    slots: RefSlots,
}

impl Indeo5Decoder {
    /// A fresh session (next frame must be INTRA).
    pub fn new() -> Self {
        Self::default()
    }

    /// The `spec/07 §1.2` slot-rotation state (bookkeeping mirror).
    pub fn ref_slots(&self) -> &RefSlots {
        &self.slots
    }

    /// Decode the next frame of the sequence.
    pub fn decode(&mut self, bitstream: &[u8]) -> Result<SessionOutput, SessionError> {
        let (header, mut r) = PictureHeader::parse_with_reader(bitstream, self.prev_frame_number)?;
        let frame_type = header.frame_type();

        // First-frame gate: only INTRA can start a session
        // (spec/01 §3.2).
        if self.gop.is_none() && frame_type != FrameType::Intra {
            return Err(SessionError::FirstFrameNotIntra { found: frame_type });
        }

        self.slots.pre_decode(frame_type);

        let out = match frame_type {
            FrameType::Null => {
                // spec/08 §6.4 — no coded payload; re-emit the held
                // output. The first-frame gate guarantees a held
                // buffer exists.
                let (format, buffer) = self
                    .held
                    .clone()
                    .expect("first-frame gate guarantees a held output");
                let g = self
                    .gop
                    .as_ref()
                    .expect("first-frame gate guarantees a held GOP");
                SessionOutput {
                    frame_type,
                    repeated_previous: true,
                    format,
                    dimensions: (g.width, g.height),
                    output: buffer,
                    frontiers: Vec::new(),
                    stats: DecodeStats::default(),
                    bands: Vec::new(),
                    frame_checksum: ChecksumStatus::Absent,
                    parse_complete: true,
                }
            }
            FrameType::Intra => {
                let gop = header.gop.clone().expect("INTRA carries a GOP");
                let frame = header.frame.as_ref().expect("INTRA carries a frame header");
                let payload = decode_payload(&mut r, &gop, frame, None)?;
                let format = output_format(&gop);
                let output = assemble_frame(
                    &payload.recon[0],
                    &payload.recon[1],
                    &payload.recon[2],
                    format,
                )?;
                let dimensions = (gop.width, gop.height);
                let (bands, frame_checksum) =
                    reconstruction_checksums(&output, frame.frm_checksum, &payload.blocks);
                self.gop = Some(gop);
                self.promote(frame_type, payload.bands);
                self.held = Some((format, output.clone()));
                SessionOutput {
                    frame_type,
                    repeated_previous: false,
                    format,
                    dimensions,
                    output,
                    frontiers: payload.frontiers,
                    stats: payload.stats,
                    bands,
                    frame_checksum,
                    parse_complete: payload.parse_complete,
                }
            }
            FrameType::Inter | FrameType::DroppableInter | FrameType::DroppableInterScalability => {
                let gop = self.gop.clone().expect("first-frame gate held a GOP");
                let frame = header
                    .frame
                    .as_ref()
                    .expect("predicted frame carries a frame header");
                let payload = decode_payload(&mut r, &gop, frame, Some(&self.reference))?;
                let format = output_format(&gop);
                let output = assemble_frame(
                    &payload.recon[0],
                    &payload.recon[1],
                    &payload.recon[2],
                    format,
                )?;
                let dimensions = (gop.width, gop.height);
                let (bands, frame_checksum) =
                    reconstruction_checksums(&output, frame.frm_checksum, &payload.blocks);
                self.promote(frame_type, payload.bands);
                self.held = Some((format, output.clone()));
                SessionOutput {
                    frame_type,
                    repeated_previous: false,
                    format,
                    dimensions,
                    output,
                    frontiers: payload.frontiers,
                    stats: payload.stats,
                    bands,
                    frame_checksum,
                    parse_complete: payload.parse_complete,
                }
            }
        };

        self.slots.post_decode(frame_type);
        self.prev_frame_number = Some(header.start.frame_number);
        Ok(out)
    }

    /// Apply the `spec/08 §8.1` reference-promotion decision to the
    /// just-decoded band buffers.
    fn promote(&mut self, frame_type: FrameType, mut bands: Vec<Vec<Band>>) {
        match reference_rotation(frame_type) {
            ReferenceRotation::Promote => {
                self.reference = bands;
            }
            ReferenceRotation::PromoteWithChromaSwap => {
                // spec/08 §8.1 — swap the chroma-plane pair, then
                // promote (the DROPPABLE_INTER_SCAL path).
                if bands.len() == 3 {
                    bands.swap(1, 2);
                }
                self.reference = bands;
            }
            ReferenceRotation::NoPromote => {
                // spec/07 §1.5 droppable invariant / NULL: the
                // reference is untouched.
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LSB-first bit packer mirroring the reader's bit order.
    struct BitWriter {
        bits: Vec<u8>,
    }
    impl BitWriter {
        fn new() -> Self {
            BitWriter { bits: Vec::new() }
        }
        fn put(&mut self, value: u32, n: u32) {
            for i in 0..n {
                self.bits.push(((value >> i) & 1) as u8);
            }
        }
        fn put_codeword(&mut self, code: u32, len: u8) {
            for i in (0..len).rev() {
                self.bits.push(((code >> i) & 1) as u8);
            }
        }
        fn align(&mut self) {
            while !self.bits.len().is_multiple_of(8) {
                self.bits.push(0);
            }
        }
        fn finish(mut self) -> Vec<u8> {
            self.align();
            let mut out = Vec::new();
            for chunk in self.bits.chunks(8) {
                let mut byte = 0u8;
                for (i, &b) in chunk.iter().enumerate() {
                    byte |= b << i;
                }
                out.push(byte);
            }
            while out.len() < 8 {
                out.push(0);
            }
            out
        }
    }

    /// Minimal custom-size INTRA header: `dim`x`dim` luma, YVU9
    /// (quarter-res chroma), no decomposition, one band per plane,
    /// all-empty bands.
    fn intra_frame(w: &mut BitWriter, frame_number: u8, dim: u32) {
        w.put(0x1f, 5);
        w.put(0, 3); // INTRA
        w.put(frame_number as u32, 8);
        w.put(0x00, 8); // gop_flags
        w.put(0, 3); // decomp
        w.put(15, 4); // pic_size_id = custom
        w.put((dim << 13) | dim, 26); // height, width
        w.put(0b000000, 6); // luma band_info
        w.put(0b000000, 6); // chroma band_info
        w.put(0, 8); // GOP trailer
        w.put(0, 8);
        w.put(0, 3);
        w.put(0, 4);
        w.align();
        w.put(0x00, 8); // frame_flags
        w.put(0, 3); // value5
        w.align();
        // Three empty bands.
        for _ in 0..3 {
            w.put(0x01, 8);
            w.align();
        }
    }

    fn intra_32(w: &mut BitWriter, frame_number: u8) {
        intra_frame(w, frame_number, 32);
    }

    /// An INTER frame (frame_type 1) with all-empty bands: pure
    /// reference carry.
    fn inter_empty(frame_number: u8) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put(0x1f, 5);
        w.put(1, 3); // INTER
        w.put(frame_number as u32, 8);
        w.put(0x00, 8); // frame_flags
        w.put(0, 3); // value5
        w.align();
        for _ in 0..3 {
            w.put(0x01, 8);
            w.align();
        }
        w.finish()
    }

    const OUT_32: usize = 32 * 32 + 2 * 8 * 8;

    #[test]
    fn first_frame_must_be_intra() {
        let mut dec = Indeo5Decoder::new();
        let err = dec.decode(&inter_empty(1)).unwrap_err();
        assert_eq!(
            err,
            SessionError::FirstFrameNotIntra {
                found: FrameType::Inter
            }
        );
    }

    #[test]
    fn intra_inter_null_sequence() {
        let mut dec = Indeo5Decoder::new();

        // Frame 0: INTRA, all-empty bands -> mid-grey.
        let mut w = BitWriter::new();
        intra_32(&mut w, 0);
        let f0 = dec.decode(&w.finish()).expect("intra");
        assert_eq!(f0.frame_type, FrameType::Intra);
        assert!(!f0.repeated_previous);
        assert_eq!(f0.output.data.len(), OUT_32);
        assert!(f0.output.data.iter().all(|&b| b == 128));

        // Frame 1: INTER, all-empty bands -> carries the reference.
        let f1 = dec.decode(&inter_empty(1)).expect("inter");
        assert_eq!(f1.frame_type, FrameType::Inter);
        assert!(f1.parse_complete);
        assert!(f1.frontiers.is_empty());
        assert_eq!(f1.output.data, f0.output.data);

        // Frame 2: NULL -> byte-for-byte repeat.
        let mut w = BitWriter::new();
        w.put(0x1f, 5);
        w.put(4, 3); // NULL
        w.put(2, 8);
        let f2 = dec.decode(&w.finish()).expect("null");
        assert!(f2.repeated_previous);
        assert_eq!(f2.output.data, f1.output.data);
    }

    #[test]
    fn duplicate_frame_number_soft_corrects_to_null_repeat() {
        let mut dec = Indeo5Decoder::new();
        let mut w = BitWriter::new();
        intra_32(&mut w, 5);
        dec.decode(&w.finish()).expect("intra");
        // An INTER frame re-using frame_number 5 soft-corrects to
        // NULL (spec/01 §3.4) and repeats the previous output.
        let out = dec.decode(&inter_empty(5)).expect("soft null");
        assert_eq!(out.frame_type, FrameType::Null);
        assert!(out.repeated_previous);
    }

    #[test]
    fn inter_skip_mbs_carry_reference() {
        let mut dec = Indeo5Decoder::new();
        let mut w = BitWriter::new();
        intra_32(&mut w, 0);
        let f0 = dec.decode(&w.finish()).expect("intra");

        // INTER frame whose luma band is coded with every MB skipped
        // (zero inherited MVs -> predictor copy), chroma bands empty.
        let mut w = BitWriter::new();
        w.put(0x1f, 5);
        w.put(1, 3);
        w.put(1, 8);
        w.put(0x00, 8); // frame_flags
        w.put(0, 3);
        w.align();
        w.put(0x00, 8); // luma band_flags: non-empty
        w.put(0, 1); // checksum_flag
        w.put(10, 5); // band_glob_quant
        w.align();
        w.put(0, 1); // tile value24
        w.put(0, 1); // value25 -> implicit
        w.align(); // MB-header phase starts byte-aligned (r459)
        for _ in 0..(2 * 2) {
            w.put(1, 1); // 32x32 tile, mb 16 -> 4 MBs, all skipped
        }
        w.align();
        for _ in 0..2 {
            w.put(0x01, 8); // empty chroma bands
            w.align();
        }
        let f1 = dec.decode(&w.finish()).expect("inter skip");
        assert!(f1.parse_complete);
        assert!(f1.frontiers.is_empty());
        assert_eq!(f1.stats.mbs_skipped, 4);
        assert_eq!(f1.output.data, f0.output.data);
    }

    #[test]
    fn inter_coded_mb_with_mv_applies_mc_copy() {
        // Drive the spec/07 predictor copy with an explicit MV via a
        // CUSTOM frame-level MB codebook of one 8-extra-bit row (256
        // symbols). Under the recentred zig-zag fold, symbol 242 maps
        // to -121, so use a 256x256 luma band and code the LAST MB
        // (at 240,240) so the displaced fetch stays in-bounds — and
        // no later skipped MB inherits the MV.
        use crate::indeo5::Codebook;
        let cb = Codebook::build(&[8]).unwrap();
        let sym = 242u32; // recentred zig-zag: 242 -> -121

        let mut dec = Indeo5Decoder::new();
        let mut w = BitWriter::new();
        intra_frame(&mut w, 0, 256);
        dec.decode(&w.finish()).expect("intra");

        // INTER: the frame header carries the custom mb_huff_desc
        // (frame_flags bit 6); a 16x16 MB grid; MBs 0..254 skipped,
        // MB 255 (at 240,240) coded with CBP 0 + MV deltas.
        // src = 240 - 121 = 119: in-bounds.
        let mut w = BitWriter::new();
        w.put(0x1f, 5);
        w.put(1, 3);
        w.put(1, 8);
        w.put(0x40, 8); // frame_flags: mb_huff_desc present
        w.put(7, 3); // custom descriptor
        w.put(1, 4); // num_rows = 1
        w.put(8, 4); // xbits[0] = 8
        w.put(0, 3); // value5
        w.align();
        w.put(0x00, 8); // luma band_flags: non-empty, defaults
        w.put(0, 1); // checksum_flag
        w.put(10, 5); // band_glob_quant
        w.align();
        w.put(0, 1); // tile value24
        w.put(0, 1); // value25 -> implicit
        w.align(); // MB-header phase starts byte-aligned (r459)
        // MB-header phase: MBs 0..254 skipped.
        for _ in 0..255 {
            w.put(1, 1);
        }
        // MB 255: coded; CBP first, then the MV VLC pair (x, y).
        w.put(0, 1);
        w.put(0b0000, 4); // CBP: no AC
        let cw = cb.codeword(sym).unwrap();
        w.put_codeword(cw.code, cw.length);
        w.put_codeword(cw.code, cw.length);
        w.align();
        for _ in 0..2 {
            w.put(0x01, 8); // empty chroma bands
            w.align();
        }
        let f1 = dec.decode(&w.finish()).expect("inter mv");
        assert!(f1.parse_complete, "custom MB codebook decodes the MVs");
        assert!(f1.frontiers.is_empty());
        assert_eq!(f1.stats.mbs, 256);
        assert_eq!(f1.stats.mbs_skipped, 255);
        assert_eq!(f1.stats.mbs_coded_no_ac, 1);
        // Reference is all-zero, so the displaced copy is also zero:
        // output stays mid-grey — the point is the MC path decodes.
        assert!(f1.output.data.iter().all(|&b| b == 128));
    }

    #[test]
    fn droppable_inter_does_not_promote_reference() {
        let mut dec = Indeo5Decoder::new();
        let mut w = BitWriter::new();
        intra_32(&mut w, 0);
        dec.decode(&w.finish()).expect("intra");
        let reference_before = dec.reference.clone();

        // DROPPABLE_INTER (frame_type 3), all-empty bands.
        let mut w = BitWriter::new();
        w.put(0x1f, 5);
        w.put(3, 3);
        w.put(1, 8);
        w.put(0x00, 8);
        w.put(0, 3);
        w.align();
        for _ in 0..3 {
            w.put(0x01, 8);
            w.align();
        }
        dec.decode(&w.finish()).expect("droppable");
        // spec/07 §1.5 droppable invariant: the reference is
        // untouched.
        assert_eq!(dec.reference, reference_before);
    }

    #[test]
    fn preset_block_codebook_band_decodes() {
        // A coded inter band whose blk_huff_desc selects preset 1
        // (formerly mis-modelled as Kraft-anomalous) now builds and
        // the MB walk decodes: 4 skipped MBs, reference carry.
        let mut dec = Indeo5Decoder::new();
        let mut w = BitWriter::new();
        intra_32(&mut w, 0);
        dec.decode(&w.finish()).expect("intra");

        let mut w = BitWriter::new();
        w.put(0x1f, 5);
        w.put(1, 3);
        w.put(1, 8);
        w.put(0x00, 8);
        w.put(0, 3);
        w.align();
        w.put(0x80, 8); // luma band: blk_huff_present
        w.put(1, 3); // blk_huff_desc: preset id 1
        w.put(0, 1); // checksum_flag
        w.put(10, 5); // band_glob_quant
        w.align();
        w.put(0, 1); // tile: implicit size, coded
        w.put(0, 1);
        w.align(); // MB-header phase starts byte-aligned (r459)
        for _ in 0..4 {
            w.put(1, 1); // all 4 MBs skipped
        }
        w.align();
        for _ in 0..2 {
            w.put(0x01, 8); // empty chroma bands
            w.align();
        }
        let f = dec.decode(&w.finish()).expect("preset band decodes");
        assert!(f.parse_complete);
        assert!(f.frontiers.is_empty());
        assert_eq!(f.stats.mbs_skipped, 4);
    }
}
