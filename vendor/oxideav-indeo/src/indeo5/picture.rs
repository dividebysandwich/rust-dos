//! Indeo 5 picture-header front door — thread the whole header stack.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/01-file-header.md` and
//! `spec/02-gop-and-band-layer.md`.
//!
//! [`PictureHeader::parse`] is the single entry that drives the
//! `spec/01 §3` picture-start triplet, the `spec/02 §1` GOP header
//! (INTRA only), and the `spec/02 §2` frame header, dispatching by
//! frame type per `spec/01 §3.5`:
//!
//! * **INTRA** (frame_type 0): picture-start → GOP header → frame
//!   header (with the §1.9 GOP trailer).
//! * **INTER / droppable** (frame_type 1..3): picture-start → frame
//!   header (no GOP header, no GOP trailer).
//! * **NULL** (frame_type 4, including the §3.4 soft-correction):
//!   picture-start only; no further bitstream is consumed and the
//!   decoder repeats the previous frame.
//!
//! The format-descriptor preamble ([`super::FormatDescriptor`]) is a
//! session-level structure validated once at decode-begin, separate
//! from the per-frame bitstream; [`PictureHeader::parse`] takes the
//! per-frame `lpInput` bitstream only.

use super::frame::{FrameError, FrameHeader};
use super::gop::{GopError, GopHeader};
use super::header::{FrameType, HeaderError, PictureStart};

/// Errors raised while parsing a full picture header.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PictureError {
    /// Fault in the `spec/01 §3` picture-start triplet.
    Header(HeaderError),
    /// Fault in the `spec/02 §1` GOP header.
    Gop(GopError),
    /// Fault in the `spec/02 §2` frame header.
    Frame(FrameError),
}

impl From<HeaderError> for PictureError {
    fn from(e: HeaderError) -> Self {
        PictureError::Header(e)
    }
}
impl From<GopError> for PictureError {
    fn from(e: GopError) -> Self {
        PictureError::Gop(e)
    }
}
impl From<FrameError> for PictureError {
    fn from(e: FrameError) -> Self {
        PictureError::Frame(e)
    }
}

impl core::fmt::Display for PictureError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PictureError::Header(e) => write!(f, "indeo5 picture: {e}"),
            PictureError::Gop(e) => write!(f, "indeo5 picture: {e}"),
            PictureError::Frame(e) => write!(f, "indeo5 picture: {e}"),
        }
    }
}

impl std::error::Error for PictureError {}

/// Spec/01 + spec/02 — a fully parsed Indeo 5 picture header.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PictureHeader {
    /// The `spec/01 §3` picture-start triplet.
    pub start: PictureStart,
    /// The `spec/02 §1` GOP header (INTRA frames only).
    pub gop: Option<GopHeader>,
    /// The `spec/02 §2` frame header (all non-NULL frames).
    pub frame: Option<FrameHeader>,
}

impl PictureHeader {
    /// Parse the whole picture header from a per-frame bitstream.
    ///
    /// `prev_frame_number` is the previously decoded frame's number for
    /// the §3.4 duplicate-`frame_number` soft-correction (`None` for
    /// the first frame of a session).
    pub fn parse(bitstream: &[u8], prev_frame_number: Option<u8>) -> Result<Self, PictureError> {
        Ok(Self::parse_with_reader(bitstream, prev_frame_number)?.0)
    }

    /// Like [`PictureHeader::parse`], but also return the bit reader,
    /// positioned at the first per-band payload byte (`spec/02 §2.8`
    /// frame-header alignment exit) so the per-band decode
    /// (`spec/02 §4.4`) can continue from the same cursor. For a NULL
    /// frame the reader sits after the picture-start triplet.
    pub fn parse_with_reader<'a>(
        bitstream: &'a [u8],
        prev_frame_number: Option<u8>,
    ) -> Result<(Self, super::bitreader::BitReader<'a>), PictureError> {
        let (start, mut r) = PictureStart::parse(bitstream, prev_frame_number)?;

        let header = match start.frame_type {
            FrameType::Null => PictureHeader {
                start,
                gop: None,
                frame: None,
            },
            FrameType::Intra => {
                let gop = GopHeader::parse(&mut r)?;
                let frame = FrameHeader::parse(&mut r, true)?;
                PictureHeader {
                    start,
                    gop: Some(gop),
                    frame: Some(frame),
                }
            }
            FrameType::Inter | FrameType::DroppableInter | FrameType::DroppableInterScalability => {
                let frame = FrameHeader::parse(&mut r, false)?;
                PictureHeader {
                    start,
                    gop: None,
                    frame: Some(frame),
                }
            }
        };
        Ok((header, r))
    }

    /// The parsed frame type (after the §3.4 soft-correction).
    pub fn frame_type(&self) -> FrameType {
        self.start.frame_type
    }

    /// `true` when this is a NULL frame (no coded payload; repeat the
    /// previous frame at output).
    pub fn is_null(&self) -> bool {
        matches!(self.start.frame_type, FrameType::Null)
    }

    /// The coded luma dimensions, available only on INTRA frames (the
    /// GOP header carries them and overrides the format descriptor).
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        self.gop.as_ref().map(|g| (g.width, g.height))
    }
}

/// Build a full picture bitstream for round-trip checks: the
/// picture-start triplet plus a caller-supplied tail.
#[cfg(test)]
fn picture_prefix(frame_type: u32, frame_number: u8) -> Vec<u8> {
    let byte0 = (frame_type << 5) as u8 | (super::PICTURE_START_CODE as u8);
    vec![byte0, frame_number]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LSB-first bit packer that prepends the picture-start triplet.
    struct PicBuilder {
        bits: Vec<u8>,
    }
    impl PicBuilder {
        fn new(frame_type: u32, frame_number: u8) -> Self {
            let mut b = PicBuilder { bits: Vec::new() };
            // PSC (5) + frame_type (3) + frame_number (8).
            b.put(super::super::PICTURE_START_CODE, 5);
            b.put(frame_type, 3);
            b.put(frame_number as u32, 8);
            b
        }
        fn put(&mut self, value: u32, n: u32) {
            for i in 0..n {
                self.bits.push(((value >> i) & 1) as u8);
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

    #[test]
    fn null_frame_no_payload() {
        let bits = picture_prefix(4, 3);
        let mut full = bits;
        full.extend_from_slice(&[0, 0, 0]); // pad for prefetch
        let ph = PictureHeader::parse(&full, None).unwrap();
        assert!(ph.is_null());
        assert!(ph.gop.is_none());
        assert!(ph.frame.is_none());
    }

    #[test]
    fn soft_corrected_null() {
        // INTER repeating frame_number 5 -> NULL, no payload parsed.
        let bits = picture_prefix(1, 5);
        let mut full = bits;
        full.extend_from_slice(&[0, 0, 0]);
        let ph = PictureHeader::parse(&full, Some(5)).unwrap();
        assert!(ph.is_null());
        assert_eq!(ph.frame_type(), FrameType::Null);
    }

    #[test]
    fn intra_full_stack() {
        // INTRA: GOP (gop_flags=0, decomp=0, pic_size_id=5, 1+1 bands)
        // then GOP trailer + frame header.
        let mut b = PicBuilder::new(0, 0);
        // --- GOP header ---
        b.put(0x00, 8); // gop_flags
        b.put(0, 3); // decomp_levels
        b.put(5, 4); // pic_size_id (CIF)
        b.put(0b000000, 6); // luma band_info
        b.put(0b000000, 6); // chroma band_info
        // --- GOP trailer (§1.9) ---
        b.put(0, 8); // value1
        b.put(0, 8); // value2
        b.put(0, 3); // value3
        b.put(0, 4); // value4 (no gop_ext)
        b.align(); // §2.1 frame-header align
        // --- frame header ---
        b.put(0x00, 8); // frame_flags
        b.put(0, 3); // value5
        let full = b.finish();
        let ph = PictureHeader::parse(&full, None).unwrap();
        assert_eq!(ph.frame_type(), FrameType::Intra);
        assert_eq!(ph.dimensions(), Some((352, 288)));
        let gop = ph.gop.as_ref().unwrap();
        assert_eq!(gop.luma_band_info.len(), 1);
        assert!(ph.frame.as_ref().unwrap().gop_trailer.is_some());
    }

    #[test]
    fn inter_frame_no_gop() {
        // INTER: frame header only, frame_number distinct from prev.
        let mut b = PicBuilder::new(1, 9);
        b.put(0x00, 8); // frame_flags
        b.put(0, 3); // value5
        let full = b.finish();
        let ph = PictureHeader::parse(&full, Some(8)).unwrap();
        assert_eq!(ph.frame_type(), FrameType::Inter);
        assert!(ph.gop.is_none());
        assert!(ph.frame.is_some());
        assert_eq!(ph.dimensions(), None);
    }

    #[test]
    fn intra_with_pic_hdr_size() {
        let mut b = PicBuilder::new(0, 0);
        b.put(0x00, 8); // gop_flags
        b.put(0, 3); // decomp
        b.put(5, 4); // pic_size_id
        b.put(0b000000, 6);
        b.put(0b000000, 6);
        // GOP trailer
        b.put(0, 8);
        b.put(0, 8);
        b.put(0, 3);
        b.put(0, 4);
        b.align();
        // frame header: pic_hdr_size present (bit0).
        b.put(0x01, 8);
        b.put(0x00cafe, 24); // pic_hdr_size
        b.put(0, 3); // value5
        let full = b.finish();
        let ph = PictureHeader::parse(&full, None).unwrap();
        assert_eq!(ph.frame.as_ref().unwrap().pic_hdr_size, Some(0x00cafe));
    }
}
