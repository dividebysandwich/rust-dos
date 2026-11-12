//! Indeo 5 format-descriptor preamble + picture-start triplet.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/01-file-header.md`.
//!
//! Every Indeo 5 codec frame begins with two layers (`spec/01 §0`):
//!
//! 1. A fixed **format-descriptor preamble** (`spec/01 §2`): a magic
//!    word plus the coded picture dimensions, carried in the host's
//!    `BITMAPINFOHEADER` extra-bytes. The decoder validates it once at
//!    `ICM_DECOMPRESS_BEGIN` and re-checks the magic per frame.
//!    [`FormatDescriptor::parse`] models the validator at
//!    `IR50_32.DLL!0x100364d0`-`0x10036544`.
//! 2. A bit-packed **picture-start triplet** (`spec/01 §3`): a 5-bit
//!    picture-start code, a 3-bit frame-type, and an 8-bit
//!    frame-number, read LSB-first. [`PictureStart::parse`] models the
//!    parser at `IR50_32.DLL!0x10023310`, including the §3.4
//!    duplicate-`frame_number` soft-correction to NULL.
//!
//! This module is the entry point of the Indeo 5 decode stack — the
//! `spec/02` GOP / frame / band headers parse from the same bit cursor
//! [`PictureStart::parse`] leaves at bit 16.

use super::bitreader::{BitReader, BitReaderError};

/// Spec/01 §2.1 — the canonical format magic word (`0x86753090`,
/// on-disk bytes `90 30 75 86`). The decoder normalises the alternate
/// form to this value in place.
pub const MAGIC_CANONICAL: u32 = 0x8675_3090;

/// Spec/01 §2.1 — the alternate accepted magic word (`0x68570309`),
/// each canonical byte's nibbles swapped. Accepted on input and
/// rewritten to [`MAGIC_CANONICAL`].
pub const MAGIC_ALTERNATE: u32 = 0x6857_0309;

/// Spec/01 §2.2 — minimum coded width, from `.rdata 0x1008cee8`.
pub const MIN_WIDTH: u32 = 4;

/// Spec/01 §2.2 — minimum coded height, from `.rdata 0x1008ceec`.
pub const MIN_HEIGHT: u32 = 4;

/// Spec/01 §3.2 — the required 5-bit picture-start code value.
pub const PICTURE_START_CODE: u32 = 0x1f;

/// Errors raised while parsing the Indeo 5 file header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HeaderError {
    /// Spec/01 §2 — the format descriptor was shorter than the 0x14
    /// bytes the validator reads.
    DescriptorTooShort {
        /// Bytes available.
        available: usize,
    },
    /// Spec/01 §2.1 — neither accepted magic word matched
    /// (`IR50_32.DLL!0x10046540`, `ICERR_BADFORMAT`).
    BadMagic {
        /// The DWORD found at offset 0x00.
        found: u32,
    },
    /// Spec/01 §2.2 — `width` or `height` below the minimum.
    DimensionTooSmall {
        /// Parsed width.
        width: u32,
        /// Parsed height.
        height: u32,
    },
    /// Spec/01 §2.2 — `width` or `height` not a multiple of 4.
    DimensionMisaligned {
        /// Parsed width.
        width: u32,
        /// Parsed height.
        height: u32,
    },
    /// Spec/01 §3.2 — the 5-bit picture-start code was not `0x1F`
    /// (parser sentinel `0xf`, `IR50_32.DLL!0x10023377`).
    BadPictureStartCode {
        /// The 5-bit value found.
        found: u32,
    },
    /// Spec/01 §3.3 — the 3-bit frame-type was `>= 5`
    /// (parser sentinel `0xf`, `IR50_32.DLL!0x100233ae`).
    IllegalFrameType {
        /// The 3-bit value found.
        found: u32,
    },
    /// Underlying bit-reader fault while consuming the triplet.
    BitReader(BitReaderError),
}

impl From<BitReaderError> for HeaderError {
    fn from(e: BitReaderError) -> Self {
        HeaderError::BitReader(e)
    }
}

impl core::fmt::Display for HeaderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HeaderError::DescriptorTooShort { available } => write!(
                f,
                "indeo5 header: format descriptor of {available} bytes is shorter than the 0x14-byte minimum (spec/01 §2)"
            ),
            HeaderError::BadMagic { found } => write!(
                f,
                "indeo5 header: format magic {found:#010x} is neither {MAGIC_CANONICAL:#010x} nor {MAGIC_ALTERNATE:#010x} (spec/01 §2.1)"
            ),
            HeaderError::DimensionTooSmall { width, height } => write!(
                f,
                "indeo5 header: dimensions {width}x{height} below minimum {MIN_WIDTH}x{MIN_HEIGHT} (spec/01 §2.2)"
            ),
            HeaderError::DimensionMisaligned { width, height } => write!(
                f,
                "indeo5 header: dimensions {width}x{height} not both multiples of 4 (spec/01 §2.2)"
            ),
            HeaderError::BadPictureStartCode { found } => write!(
                f,
                "indeo5 header: picture-start code {found:#x} != {PICTURE_START_CODE:#x} (spec/01 §3.2)"
            ),
            HeaderError::IllegalFrameType { found } => write!(
                f,
                "indeo5 header: frame_type {found} >= 5 is illegal (spec/01 §3.3)"
            ),
            HeaderError::BitReader(e) => write!(f, "indeo5 header: {e}"),
        }
    }
}

impl std::error::Error for HeaderError {}

/// Spec/01 §2 — the validated format-descriptor preamble.
///
/// Only the wire-format-relevant fields (magic + dimensions) are
/// surfaced; offsets `0x14+` are codec-internal session scratch
/// (`spec/01 §2.3`) and not part of the wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct FormatDescriptor {
    /// Coded picture width in pixels (offset 0x10).
    pub width: u32,
    /// Coded picture height in pixels (offset 0x0c).
    pub height: u32,
    /// `true` when the descriptor carried the alternate magic
    /// ([`MAGIC_ALTERNATE`]); the validator rewrites it to
    /// [`MAGIC_CANONICAL`] in place (`spec/01 §2.1`).
    pub magic_normalised: bool,
}

impl FormatDescriptor {
    /// Parse and validate the format descriptor from the start of a
    /// byte slice (the bytes immediately past the host's 40-byte
    /// `BITMAPINFOHEADER`). Mirrors the validator at
    /// `IR50_32.DLL!0x100364d0`-`0x10036544`.
    ///
    /// Note the **height-before-width** field order (`spec/01 §2.2`):
    /// `height` at 0x0c, `width` at 0x10.
    pub fn parse(desc: &[u8]) -> Result<Self, HeaderError> {
        if desc.len() < 0x14 {
            return Err(HeaderError::DescriptorTooShort {
                available: desc.len(),
            });
        }
        let magic = u32::from_le_bytes([desc[0], desc[1], desc[2], desc[3]]);
        let magic_normalised = match magic {
            MAGIC_CANONICAL => false,
            MAGIC_ALTERNATE => true,
            other => return Err(HeaderError::BadMagic { found: other }),
        };
        let height = u32::from_le_bytes([desc[0xc], desc[0xd], desc[0xe], desc[0xf]]);
        let width = u32::from_le_bytes([desc[0x10], desc[0x11], desc[0x12], desc[0x13]]);
        if width < MIN_WIDTH || height < MIN_HEIGHT {
            return Err(HeaderError::DimensionTooSmall { width, height });
        }
        if width & 0x3 != 0 || height & 0x3 != 0 {
            return Err(HeaderError::DimensionMisaligned { width, height });
        }
        Ok(FormatDescriptor {
            width,
            height,
            magic_normalised,
        })
    }
}

/// Spec/01 §3.3 — frame type, the 3-bit field after the picture-start
/// code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    /// 0 — key frame; carries a GOP header (`spec/02 §1`).
    Intra,
    /// 1 — predicted frame; no GOP header.
    Inter,
    /// 2 — predicted frame, droppable in scalability mode.
    DroppableInterScalability,
    /// 3 — predicted frame, droppable (temporal scalability).
    DroppableInter,
    /// 4 — no coded payload after the picture header (`spec/01 §4`).
    Null,
}

impl FrameType {
    /// Map the 3-bit field value to a [`FrameType`]. Values `5..=7`
    /// are illegal (`spec/01 §3.3`).
    pub fn from_bits(value: u32) -> Result<Self, HeaderError> {
        match value {
            0 => Ok(FrameType::Intra),
            1 => Ok(FrameType::Inter),
            2 => Ok(FrameType::DroppableInterScalability),
            3 => Ok(FrameType::DroppableInter),
            4 => Ok(FrameType::Null),
            other => Err(HeaderError::IllegalFrameType { found: other }),
        }
    }

    /// The 3-bit wire value for this frame type.
    pub fn to_bits(self) -> u32 {
        match self {
            FrameType::Intra => 0,
            FrameType::Inter => 1,
            FrameType::DroppableInterScalability => 2,
            FrameType::DroppableInter => 3,
            FrameType::Null => 4,
        }
    }

    /// `true` for INTRA frames, which carry a GOP header
    /// (`spec/01 §3.5`).
    pub fn carries_gop_header(self) -> bool {
        matches!(self, FrameType::Intra)
    }

    /// `true` for the three predicted (INTER-family) frame types.
    pub fn is_predicted(self) -> bool {
        matches!(
            self,
            FrameType::Inter | FrameType::DroppableInterScalability | FrameType::DroppableInter
        )
    }
}

/// Spec/01 §3 — the parsed picture-start triplet (PSC + frame_type +
/// frame_number), plus the [`BitReader`] positioned at the next field
/// (bit 16) for the downstream GOP / frame / band parsers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureStart {
    /// Parsed frame type, after the §3.4 soft-correction.
    pub frame_type: FrameType,
    /// Parsed 8-bit frame number (low byte).
    pub frame_number: u8,
    /// `true` when the §3.4 duplicate-`frame_number` continuity check
    /// re-classified a predicted frame as NULL.
    pub soft_corrected_to_null: bool,
}

impl PictureStart {
    /// Parse the picture-start triplet from a fresh bit reader over
    /// the per-frame bitstream (`lpInput`).
    ///
    /// `prev_frame_number` is the previously decoded frame's number
    /// (`None` for the first frame of a session). When the new frame
    /// is predicted (`frame_type 1..3`) and its number equals the
    /// previous one, the §3.4 continuity check soft-corrects the frame
    /// to NULL (`IR50_32.DLL!0x100233ed`-`0x10023400`).
    ///
    /// Returns the parsed triplet and the bit reader left at bit 16.
    pub fn parse<'a>(
        bitstream: &'a [u8],
        prev_frame_number: Option<u8>,
    ) -> Result<(Self, BitReader<'a>), HeaderError> {
        let mut r = BitReader::new(bitstream)?;
        let psc = r.read(5)?;
        if psc != PICTURE_START_CODE {
            return Err(HeaderError::BadPictureStartCode { found: psc });
        }
        let ft_bits = r.read(3)?;
        let mut frame_type = FrameType::from_bits(ft_bits)?;
        let frame_number = r.read(8)? as u8;

        // Spec/01 §3.4 — duplicate-frame_number soft-correction.
        let mut soft_corrected_to_null = false;
        if frame_type.is_predicted()
            && let Some(prev) = prev_frame_number
            && prev == frame_number
        {
            frame_type = FrameType::Null;
            soft_corrected_to_null = true;
        }

        Ok((
            PictureStart {
                frame_type,
                frame_number,
                soft_corrected_to_null,
            },
            r,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(magic: u32, width: u32, height: u32) -> Vec<u8> {
        let mut d = vec![0u8; 0x14];
        d[0..4].copy_from_slice(&magic.to_le_bytes());
        d[0xc..0x10].copy_from_slice(&height.to_le_bytes());
        d[0x10..0x14].copy_from_slice(&width.to_le_bytes());
        d
    }

    #[test]
    fn descriptor_canonical_magic() {
        let d = descriptor(MAGIC_CANONICAL, 352, 288);
        let fd = FormatDescriptor::parse(&d).unwrap();
        assert_eq!(fd.width, 352);
        assert_eq!(fd.height, 288);
        assert!(!fd.magic_normalised);
    }

    #[test]
    fn descriptor_alternate_magic_normalises() {
        let d = descriptor(MAGIC_ALTERNATE, 160, 120);
        let fd = FormatDescriptor::parse(&d).unwrap();
        assert!(fd.magic_normalised);
        assert_eq!(fd.width, 160);
    }

    #[test]
    fn descriptor_bad_magic() {
        let d = descriptor(0xdead_beef, 352, 288);
        assert_eq!(
            FormatDescriptor::parse(&d),
            Err(HeaderError::BadMagic { found: 0xdead_beef })
        );
    }

    #[test]
    fn descriptor_dimension_too_small() {
        let d = descriptor(MAGIC_CANONICAL, 0, 288);
        assert!(matches!(
            FormatDescriptor::parse(&d),
            Err(HeaderError::DimensionTooSmall { .. })
        ));
    }

    #[test]
    fn descriptor_misaligned() {
        let d = descriptor(MAGIC_CANONICAL, 354, 288);
        assert!(matches!(
            FormatDescriptor::parse(&d),
            Err(HeaderError::DimensionMisaligned { .. })
        ));
    }

    #[test]
    fn descriptor_too_short() {
        let d = vec![0u8; 0x10];
        assert_eq!(
            FormatDescriptor::parse(&d),
            Err(HeaderError::DescriptorTooShort { available: 0x10 })
        );
    }

    /// Build a picture-start bitstream: PSC=0x1f (5b), frame_type (3b),
    /// frame_number (8b), all LSB-first, then padding.
    fn picture_bits(frame_type: u32, frame_number: u8) -> Vec<u8> {
        // byte0: low 5 bits PSC=0x1f, high 3 bits = frame_type
        let byte0 = (frame_type << 5) as u8 | (PICTURE_START_CODE as u8);
        vec![byte0, frame_number, 0, 0, 0]
    }

    #[test]
    fn picture_start_intra() {
        let bits = picture_bits(0, 0);
        let (ps, r) = PictureStart::parse(&bits, None).unwrap();
        assert_eq!(ps.frame_type, FrameType::Intra);
        assert_eq!(ps.frame_number, 0);
        assert!(!ps.soft_corrected_to_null);
        assert_eq!(r.bits_read(), 16);
    }

    #[test]
    fn picture_start_null() {
        let bits = picture_bits(4, 7);
        let (ps, _) = PictureStart::parse(&bits, None).unwrap();
        assert_eq!(ps.frame_type, FrameType::Null);
    }

    #[test]
    fn picture_start_bad_psc() {
        // PSC != 0x1f
        let bits = vec![0x1e, 0, 0, 0, 0];
        assert!(matches!(
            PictureStart::parse(&bits, None),
            Err(HeaderError::BadPictureStartCode { found: 0x1e })
        ));
    }

    #[test]
    fn picture_start_illegal_frame_type() {
        let bits = picture_bits(5, 0);
        assert!(matches!(
            PictureStart::parse(&bits, None),
            Err(HeaderError::IllegalFrameType { found: 5 })
        ));
    }

    #[test]
    fn picture_start_soft_correction() {
        // INTER (1) repeating frame_number 7 -> soft-corrected to NULL.
        let bits = picture_bits(1, 7);
        let (ps, _) = PictureStart::parse(&bits, Some(7)).unwrap();
        assert_eq!(ps.frame_type, FrameType::Null);
        assert!(ps.soft_corrected_to_null);
        assert_eq!(ps.frame_number, 7);
    }

    #[test]
    fn picture_start_no_soft_correction_when_distinct() {
        let bits = picture_bits(1, 8);
        let (ps, _) = PictureStart::parse(&bits, Some(7)).unwrap();
        assert_eq!(ps.frame_type, FrameType::Inter);
        assert!(!ps.soft_corrected_to_null);
    }

    #[test]
    fn intra_never_soft_corrected() {
        // INTRA is not predicted; even a repeated number stays INTRA.
        let bits = picture_bits(0, 0);
        let (ps, _) = PictureStart::parse(&bits, Some(0)).unwrap();
        assert_eq!(ps.frame_type, FrameType::Intra);
    }
}
