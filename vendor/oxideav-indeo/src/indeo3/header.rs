//! Indeo 3 frame-header + bitstream-header parser.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/01-file-header.md`.
//! All field offsets, widths, sentinel values, and validation
//! rules below cite sections of that chapter directly.

use core::fmt;

/// The four-byte ASCII tag `"FRMH"` read as a little-endian
/// DWORD (§2.1). Used by the frame-header checksum check.
pub const MAGIC_FRMH: u32 = 0x4652_4d48;

/// The only `dec_version` value the reference decoder accepts
/// (§3.1). Both `IV31` and `IV32` frames carry this constant;
/// the R3.1 / R3.2 distinction is a container-level FOURCC
/// convention, not a bitstream-version field.
pub const REQUIRED_DEC_VERSION: u16 = 0x0020;

/// Total size in bytes of the fixed 16-byte frame header (§2).
pub const FRAME_HEADER_LEN: usize = 16;

/// Total size in bytes of the fixed 48-byte bitstream header (§3).
pub const BITSTREAM_HEADER_LEN: usize = 48;

/// Total size in bytes of the combined header at the start of an
/// Indeo 3 codec frame: 16-byte frame header (§2) + 48-byte
/// bitstream header (§3).
pub const COMBINED_HEADER_LEN: usize = FRAME_HEADER_LEN + BITSTREAM_HEADER_LEN;

/// `frame_flags` bit 1 (mask `0x0002`) — 8-bit YVU9 pixel format.
/// The reference decoder rejects any frame with this bit set
/// (§3.2, error `-100` at `IR32_32.DLL!0x10004285`).
pub const FLAG_YVU9_8BIT: u16 = 0x0002;

/// Sentinel value of `data_size` (§3.3) that marks a NULL / sync
/// frame: 128 bits, i.e. no coded picture payload, the decoder
/// reproduces output from prior-frame state.
pub const NULL_FRAME_DATA_SIZE_BITS: u32 = 0x0000_0080;

/// Codec-supplied picture-dimension envelope inherited from
/// `ICDecompressQuery` (§3.6).
///
/// The per-frame parser itself treats `bsh.height` / `bsh.width`
/// as informational and decodes against the host-supplied
/// `BITMAPINFOHEADER` geometry; this constant records the
/// envelope the host enforces upstream.
pub const MIN_DIMENSION: u16 = 0x0010;
/// Upper width bound from `ICDecompressQuery` (§3.6).
pub const MAX_WIDTH: u16 = 0x0280;
/// Upper height bound from `ICDecompressQuery` (§3.6).
pub const MAX_HEIGHT: u16 = 0x01e0;

/// Errors a structurally valid Indeo 3 header decoder can raise.
///
/// Variants map one-to-one to the validation rules called out in
/// `spec/01-file-header.md` §2.x / §3.x. The reference decoder
/// returns a single opaque error code (`-100`, `0xffffff9c`) for
/// all of these; we surface them individually so callers can
/// distinguish "the buffer was too short" from "the checksum did
/// not match" from "the codec version is wrong".
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum HeaderError {
    /// The supplied buffer was too short for either the 16-byte
    /// frame header (§2) or the 48-byte bitstream header (§3).
    BufferTooShort {
        /// Number of bytes the parser needed to read.
        needed: usize,
        /// Number of bytes actually present in the input buffer.
        actual: usize,
    },
    /// The frame header's `check_sum` field (§2.1) did not match
    /// the recomputed `frame_number ^ unknown1 ^ frame_size ^
    /// 'FRMH'`.
    ChecksumMismatch {
        /// Checksum read from the header (offset `0x08`).
        got: u32,
        /// Checksum the parser recomputed from the surrounding
        /// fields and the `FRMH` constant.
        expected: u32,
    },
    /// The frame header's `frame_size` field (§2.2) was less
    /// than or equal to the 16-byte frame-header size. The
    /// reference parser requires at least one byte of payload
    /// after the frame header.
    FrameSizeTooSmall {
        /// `frame_size` as read from offset `0x0c`.
        frame_size: u32,
    },
    /// The bitstream header's `dec_version` field (§3.1) did not
    /// equal the required value `0x0020`.
    UnsupportedDecVersion {
        /// `dec_version` as read from bsh offset `0x00`.
        got: u16,
    },
    /// The bitstream header's `frame_flags` field (§3.2) had bit
    /// 1 (mask `0x0002`, `YVU9_8BIT`) set. The reference
    /// decoder explicitly rejects this pixel format.
    UnsupportedPixelFormat {
        /// `frame_flags` as read from bsh offset `0x02`.
        flags: u16,
    },
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            HeaderError::BufferTooShort { needed, actual } => write!(
                f,
                "input buffer too short: need {needed} bytes, got {actual}"
            ),
            HeaderError::ChecksumMismatch { got, expected } => write!(
                f,
                "frame header check_sum mismatch: got 0x{got:08x}, expected 0x{expected:08x} (frame_number ^ unknown1 ^ frame_size ^ 'FRMH')"
            ),
            HeaderError::FrameSizeTooSmall { frame_size } => write!(
                f,
                "frame_size 0x{frame_size:08x} is not greater than the 16-byte frame header"
            ),
            HeaderError::UnsupportedDecVersion { got } => write!(
                f,
                "unsupported dec_version 0x{got:04x} (only 0x{REQUIRED_DEC_VERSION:04x} is accepted)"
            ),
            HeaderError::UnsupportedPixelFormat { flags } => write!(
                f,
                "unsupported pixel format: frame_flags 0x{flags:04x} has YVU9_8BIT (bit 1) set"
            ),
        }
    }
}

impl std::error::Error for HeaderError {}

/// Typed view of `frame_flags` (bsh+0x02, §3.2).
///
/// Each accessor below corresponds to one of the named flag bits
/// the reference parser explicitly tests. Bits 6, 7, and 10..=15
/// are not tested by the parser (see §6 deferred items) — callers
/// that need their raw value can read [`FrameFlags::bits`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameFlags(pub u16);

impl FrameFlags {
    /// Raw 16-bit `frame_flags` value as it appeared in the
    /// bitstream header (little-endian on disk; this is the
    /// host-order value after the load).
    pub fn bits(self) -> u16 {
        self.0
    }

    /// Bit 0 — `PERIODIC_INTRA`: the frame is a periodic INTRA
    /// (key) frame.
    pub fn periodic_intra(self) -> bool {
        self.0 & 0x0001 != 0
    }

    /// Bit 1 — `YVU9_8BIT`: 8-bit YVU9 pixel format.
    /// **Unsupported by the reference decoder**; parsing a
    /// header with this bit set returns
    /// [`HeaderError::UnsupportedPixelFormat`].
    pub fn yvu9_8bit(self) -> bool {
        self.0 & FLAG_YVU9_8BIT != 0
    }

    /// Bit 2 — `INTRA`: the frame is an INTRA (key) frame
    /// (alone, or in combination with bit 0).
    pub fn intra(self) -> bool {
        self.0 & 0x0004 != 0
    }

    /// True when neither bit 0 nor bit 2 is set — i.e. the frame
    /// is an INTER frame and the parser will apply the §3.6
    /// sequence-continuity check against the previously decoded
    /// `frame_number`.
    pub fn is_inter(self) -> bool {
        self.0 & 0x0005 == 0
    }

    /// Bit 3 — `NEXT_INTRA_HINT`: the next frame in the
    /// sequence is an INTRA frame.
    pub fn next_intra_hint(self) -> bool {
        self.0 & 0x0008 != 0
    }

    /// Bit 4 — `MV_HALFPEL_HORIZ`: horizontal motion vectors
    /// carry half-pel resolution.
    pub fn mv_halfpel_horiz(self) -> bool {
        self.0 & 0x0010 != 0
    }

    /// Bit 5 — `MV_HALFPEL_VERT`: vertical motion vectors carry
    /// half-pel resolution.
    pub fn mv_halfpel_vert(self) -> bool {
        self.0 & 0x0020 != 0
    }

    /// Bit 8 — `DROPPABLE_INTER`: the decoder may skip this
    /// frame under buffer pressure.
    pub fn droppable_inter(self) -> bool {
        self.0 & 0x0100 != 0
    }

    /// Bit 9 — `BUFFER_SELECTOR`: 0 = primary reference buffer,
    /// 1 = secondary reference buffer.
    pub fn buffer_selector(self) -> bool {
        self.0 & 0x0200 != 0
    }
}

/// 16-byte frame header (§2) preceding the bitstream header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeaderPreamble {
    /// `frame_number` — sequential frame counter from offset
    /// `0x00`, starting at 0 for the first frame an encoder
    /// emits (§2).
    pub frame_number: u32,
    /// `unknown1` — reserved DWORD at offset `0x04`. Read by the
    /// parser but never range-checked (§2, deferred item §6.1).
    pub unknown1: u32,
    /// `check_sum` — identification checksum at offset `0x08`.
    /// Validated in `parse` (§2.1) — by the time you hold a
    /// [`FrameHeader`], this value matched the recomputed XOR.
    pub check_sum: u32,
    /// `frame_size` — total codec-frame size in bytes, measured
    /// from byte 0 of the frame header (§2).
    pub frame_size: u32,
}

/// 48-byte bitstream header (§3) starting at byte `0x10` of the
/// codec frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct BitstreamHeader {
    /// `dec_version` (bsh+0x00, §3.1). Always equals
    /// [`REQUIRED_DEC_VERSION`] for a successfully parsed
    /// header.
    pub dec_version: u16,
    /// `frame_flags` (bsh+0x02, §3.2) — see [`FrameFlags`].
    pub frame_flags: FrameFlags,
    /// `data_size` (bsh+0x04, §3.3) — bitstream payload length
    /// in **bits** (not bytes). The sentinel
    /// [`NULL_FRAME_DATA_SIZE_BITS`] marks a NULL / sync frame.
    pub data_size: u32,
    /// `cb_offset` (bsh+0x08, §3.4) — signed codebook selection
    /// bias used by VQ modes 1 and 4. The reference parser
    /// sign-extends the byte at the read site, so this is i8.
    pub cb_offset: i8,
    /// `reserved1` (bsh+0x09, §3) — ignored by the decoder.
    pub reserved1: u8,
    /// `checksum` (bsh+0x0a, §3.5) — optional payload checksum.
    /// Known encoders set it to 0; the reference parser does not
    /// validate it. Callers MUST tolerate any value here.
    pub checksum: u16,
    /// `height` (bsh+0x0c, §3.6) — coded picture height in luma
    /// pixels.
    pub height: u16,
    /// `width` (bsh+0x0e, §3.6) — coded picture width in luma
    /// pixels.
    pub width: u16,
    /// `y_offset` (bsh+0x10, §3.7) — byte offset from the start
    /// of the bitstream header to the Y-plane data.
    pub y_offset: u32,
    /// `v_offset` (bsh+0x14, §3.7) — byte offset to V-plane
    /// data. (Plane order in the header is Y, V, U, consistent
    /// with the codec's YVU 4:1:0 internal pixel format.)
    pub v_offset: u32,
    /// `u_offset` (bsh+0x18, §3.7) — byte offset to U-plane
    /// data.
    pub u_offset: u32,
    /// `reserved2` (bsh+0x1c, §3.8) — ignored by the decoder.
    pub reserved2: u32,
    /// `alt_quant[16]` (bsh+0x20, §3.9) — per-frame VQ codebook
    /// indices. Each byte encodes a pair of 4-bit table
    /// indices: high nibble = primary table, low nibble =
    /// secondary table.
    pub alt_quant: [u8; 16],
}

impl BitstreamHeader {
    /// True iff `data_size == NULL_FRAME_DATA_SIZE_BITS` (§3.3).
    /// On null frames the reference decoder skips the
    /// `alt_quant[]` codebook rebuild and reproduces output
    /// from prior-frame state.
    pub fn is_null_frame(&self) -> bool {
        self.data_size == NULL_FRAME_DATA_SIZE_BITS
    }
}

/// Combined typed view of the 64-byte codec-frame header (frame
/// header + bitstream header) at offset 0 of an Indeo 3 frame.
///
/// Construct via [`FrameHeader::parse`], which performs every
/// validation called out in `spec/01-file-header.md` §§2.1, 2.2,
/// 3.1, and 3.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct FrameHeader {
    /// The 16-byte preamble at offset 0 (§2).
    pub frame: FrameHeaderPreamble,
    /// The 48-byte bitstream header at offset `0x10` (§3).
    pub bitstream: BitstreamHeader,
}

impl FrameHeader {
    /// Parse the combined 64-byte header at the start of an
    /// Indeo 3 codec frame.
    ///
    /// `input` is the codec's input buffer (the `lpInput` field
    /// of the VfW `ICDECOMPRESS` struct, per §1); only the first
    /// [`COMBINED_HEADER_LEN`] bytes are consumed.
    ///
    /// Validations performed (in order, each citing the spec
    /// section that motivates it):
    ///
    /// 1. The buffer is at least [`COMBINED_HEADER_LEN`] bytes
    ///    long (§2 + §3).
    /// 2. `check_sum == frame_number ^ unknown1 ^ frame_size ^
    ///    'FRMH'` (§2.1).
    /// 3. `frame_size > 16` (§2.2).
    /// 4. `dec_version == 0x0020` (§3.1).
    /// 5. `frame_flags` bit 1 (`YVU9_8BIT`) is clear (§3.2).
    pub fn parse(input: &[u8]) -> Result<Self, HeaderError> {
        if input.len() < COMBINED_HEADER_LEN {
            return Err(HeaderError::BufferTooShort {
                needed: COMBINED_HEADER_LEN,
                actual: input.len(),
            });
        }

        // §2 — fixed 16-byte frame header at offset 0. All four
        // fields are u32 little-endian.
        let frame_number = read_u32_le(&input[0x00..0x04]);
        let unknown1 = read_u32_le(&input[0x04..0x08]);
        let check_sum = read_u32_le(&input[0x08..0x0c]);
        let frame_size = read_u32_le(&input[0x0c..0x10]);

        // §2.1 — identification checksum.
        let expected = frame_number ^ unknown1 ^ frame_size ^ MAGIC_FRMH;
        if check_sum != expected {
            return Err(HeaderError::ChecksumMismatch {
                got: check_sum,
                expected,
            });
        }

        // §2.2 — frame_size must be strictly greater than the
        // 16-byte frame header.
        if (frame_size as usize) <= FRAME_HEADER_LEN {
            return Err(HeaderError::FrameSizeTooSmall { frame_size });
        }

        // §3 — 48-byte bitstream header begins at offset 0x10.
        let bsh = &input[FRAME_HEADER_LEN..FRAME_HEADER_LEN + BITSTREAM_HEADER_LEN];

        let dec_version = read_u16_le(&bsh[0x00..0x02]);
        if dec_version != REQUIRED_DEC_VERSION {
            return Err(HeaderError::UnsupportedDecVersion { got: dec_version });
        }

        let frame_flags_raw = read_u16_le(&bsh[0x02..0x04]);
        if frame_flags_raw & FLAG_YVU9_8BIT != 0 {
            return Err(HeaderError::UnsupportedPixelFormat {
                flags: frame_flags_raw,
            });
        }

        let data_size = read_u32_le(&bsh[0x04..0x08]);
        let cb_offset = bsh[0x08] as i8;
        let reserved1 = bsh[0x09];
        let checksum = read_u16_le(&bsh[0x0a..0x0c]);
        let height = read_u16_le(&bsh[0x0c..0x0e]);
        let width = read_u16_le(&bsh[0x0e..0x10]);
        let y_offset = read_u32_le(&bsh[0x10..0x14]);
        let v_offset = read_u32_le(&bsh[0x14..0x18]);
        let u_offset = read_u32_le(&bsh[0x18..0x1c]);
        let reserved2 = read_u32_le(&bsh[0x1c..0x20]);

        let mut alt_quant = [0u8; 16];
        alt_quant.copy_from_slice(&bsh[0x20..0x30]);

        Ok(FrameHeader {
            frame: FrameHeaderPreamble {
                frame_number,
                unknown1,
                check_sum,
                frame_size,
            },
            bitstream: BitstreamHeader {
                dec_version,
                frame_flags: FrameFlags(frame_flags_raw),
                data_size,
                cb_offset,
                reserved1,
                checksum,
                height,
                width,
                y_offset,
                v_offset,
                u_offset,
                reserved2,
                alt_quant,
            },
        })
    }
}

/// Decode a per-byte `alt_quant[]` entry into its (primary,
/// secondary) 4-bit table-index pair (§3.9).
///
/// Returns `(primary_index, secondary_index)`. The primary index
/// is the high nibble; the secondary is the low nibble.
pub fn alt_quant_indices(byte: u8) -> (u8, u8) {
    let primary = (byte & 0xf0) >> 4;
    let secondary = byte & 0x0f;
    (primary, secondary)
}

#[inline]
fn read_u16_le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

#[inline]
fn read_u32_le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Per-field cursor used by [`build_valid_header`] to keep
    /// the test helper under clippy's `too_many_arguments`
    /// threshold.
    #[derive(Clone, Copy)]
    struct HeaderFields {
        frame_number: u32,
        unknown1: u32,
        frame_size: u32,
        frame_flags: u16,
        data_size_bits: u32,
        cb_offset: i8,
        reserved1: u8,
        bsh_checksum: u16,
        height: u16,
        width: u16,
        y_offset: u32,
        v_offset: u32,
        u_offset: u32,
        reserved2: u32,
        alt_quant: [u8; 16],
    }

    impl HeaderFields {
        /// Defaults matching a structurally minimal NULL frame
        /// with arbitrary plane offsets.
        fn minimal() -> Self {
            Self {
                frame_number: 0,
                unknown1: 0,
                frame_size: 0x4000,
                frame_flags: 0,
                data_size_bits: NULL_FRAME_DATA_SIZE_BITS,
                cb_offset: 0,
                reserved1: 0,
                bsh_checksum: 0,
                height: 16,
                width: 16,
                y_offset: 0x30,
                v_offset: 0x40,
                u_offset: 0x50,
                reserved2: 0,
                alt_quant: [0; 16],
            }
        }
    }

    /// Build a 64-byte minimal-but-valid Indeo 3 header where
    /// the §2.1 checksum matches and `frame_flags` is clear.
    fn build_valid_header(f: HeaderFields) -> [u8; COMBINED_HEADER_LEN] {
        let HeaderFields {
            frame_number,
            unknown1,
            frame_size,
            frame_flags,
            data_size_bits,
            cb_offset,
            reserved1,
            bsh_checksum,
            height,
            width,
            y_offset,
            v_offset,
            u_offset,
            reserved2,
            alt_quant,
        } = f;
        let mut buf = [0u8; COMBINED_HEADER_LEN];
        let check_sum = frame_number ^ unknown1 ^ frame_size ^ MAGIC_FRMH;

        buf[0x00..0x04].copy_from_slice(&frame_number.to_le_bytes());
        buf[0x04..0x08].copy_from_slice(&unknown1.to_le_bytes());
        buf[0x08..0x0c].copy_from_slice(&check_sum.to_le_bytes());
        buf[0x0c..0x10].copy_from_slice(&frame_size.to_le_bytes());

        let bsh = &mut buf[FRAME_HEADER_LEN..];
        bsh[0x00..0x02].copy_from_slice(&REQUIRED_DEC_VERSION.to_le_bytes());
        bsh[0x02..0x04].copy_from_slice(&frame_flags.to_le_bytes());
        bsh[0x04..0x08].copy_from_slice(&data_size_bits.to_le_bytes());
        bsh[0x08] = cb_offset as u8;
        bsh[0x09] = reserved1;
        bsh[0x0a..0x0c].copy_from_slice(&bsh_checksum.to_le_bytes());
        bsh[0x0c..0x0e].copy_from_slice(&height.to_le_bytes());
        bsh[0x0e..0x10].copy_from_slice(&width.to_le_bytes());
        bsh[0x10..0x14].copy_from_slice(&y_offset.to_le_bytes());
        bsh[0x14..0x18].copy_from_slice(&v_offset.to_le_bytes());
        bsh[0x18..0x1c].copy_from_slice(&u_offset.to_le_bytes());
        bsh[0x1c..0x20].copy_from_slice(&reserved2.to_le_bytes());
        bsh[0x20..0x30].copy_from_slice(&alt_quant);

        buf
    }

    #[test]
    fn parses_minimal_intra_frame() {
        let buf = build_valid_header(HeaderFields {
            frame_flags: 0x0005,    // PERIODIC_INTRA | INTRA
            data_size_bits: 0x1000, // 4096 bits
            height: 240,
            width: 320,
            ..HeaderFields::minimal()
        });
        let hdr = FrameHeader::parse(&buf).expect("minimal intra header must parse");

        assert_eq!(hdr.frame.frame_number, 0);
        assert_eq!(hdr.frame.unknown1, 0);
        assert_eq!(hdr.frame.frame_size, 0x4000);
        assert_eq!(hdr.bitstream.dec_version, REQUIRED_DEC_VERSION);
        assert_eq!(hdr.bitstream.data_size, 0x1000);
        assert_eq!(hdr.bitstream.height, 240);
        assert_eq!(hdr.bitstream.width, 320);
        assert_eq!(hdr.bitstream.y_offset, 0x30);
        assert_eq!(hdr.bitstream.v_offset, 0x40);
        assert_eq!(hdr.bitstream.u_offset, 0x50);

        let flags = hdr.bitstream.frame_flags;
        assert!(flags.periodic_intra());
        assert!(flags.intra());
        assert!(!flags.is_inter());
        assert!(!flags.yvu9_8bit());
        assert!(!hdr.bitstream.is_null_frame());
    }

    #[test]
    fn rejects_buffer_shorter_than_64_bytes() {
        let buf = [0u8; 63];
        let err = FrameHeader::parse(&buf).unwrap_err();
        assert_eq!(
            err,
            HeaderError::BufferTooShort {
                needed: COMBINED_HEADER_LEN,
                actual: 63,
            }
        );
    }

    #[test]
    fn rejects_checksum_mismatch() {
        let mut buf = build_valid_header(HeaderFields {
            frame_number: 7,
            frame_flags: 0x0005,
            data_size_bits: 0x1000,
            height: 240,
            width: 320,
            ..HeaderFields::minimal()
        });
        // Flip a byte inside the checksum word so the §2.1
        // recompute disagrees.
        buf[0x08] ^= 0xff;
        match FrameHeader::parse(&buf) {
            Err(HeaderError::ChecksumMismatch { got, expected }) => {
                assert_ne!(got, expected);
                // The expected value is the XOR of the other
                // three header dwords with the FRMH magic and
                // does not depend on what we wrote into `got`.
                assert_eq!(expected, 7u32 ^ 0x4000u32 ^ MAGIC_FRMH);
            }
            other => panic!("expected ChecksumMismatch, got {other:?}"),
        }
    }

    #[test]
    fn rejects_frame_size_at_or_below_header_length() {
        // §2.2 — strictly greater than 16 required.
        for too_small in 0u32..=(FRAME_HEADER_LEN as u32) {
            let buf = build_valid_header(HeaderFields {
                frame_number: 1,
                unknown1: 2,
                frame_size: too_small,
                ..HeaderFields::minimal()
            });
            match FrameHeader::parse(&buf) {
                Err(HeaderError::FrameSizeTooSmall { frame_size }) => {
                    assert_eq!(frame_size, too_small);
                }
                other => {
                    panic!("expected FrameSizeTooSmall for frame_size={too_small}, got {other:?}")
                }
            }
        }
    }

    #[test]
    fn accepts_frame_size_just_above_header_length() {
        let buf = build_valid_header(HeaderFields {
            frame_number: 1,
            unknown1: 2,
            frame_size: (FRAME_HEADER_LEN as u32) + 1,
            ..HeaderFields::minimal()
        });
        let hdr = FrameHeader::parse(&buf).expect("frame_size = 17 must parse");
        assert_eq!(hdr.frame.frame_size, 17);
    }

    #[test]
    fn rejects_unsupported_dec_version() {
        let mut buf = build_valid_header(HeaderFields::minimal());
        // Overwrite dec_version with 0x0030.
        buf[FRAME_HEADER_LEN..FRAME_HEADER_LEN + 2].copy_from_slice(&0x0030u16.to_le_bytes());
        match FrameHeader::parse(&buf) {
            Err(HeaderError::UnsupportedDecVersion { got }) => assert_eq!(got, 0x0030),
            other => panic!("expected UnsupportedDecVersion, got {other:?}"),
        }
    }

    #[test]
    fn rejects_yvu9_8bit_pixel_format() {
        let buf = build_valid_header(HeaderFields {
            frame_flags: FLAG_YVU9_8BIT,
            ..HeaderFields::minimal()
        });
        match FrameHeader::parse(&buf) {
            Err(HeaderError::UnsupportedPixelFormat { flags }) => {
                assert_eq!(flags, FLAG_YVU9_8BIT);
            }
            other => panic!("expected UnsupportedPixelFormat, got {other:?}"),
        }
    }

    #[test]
    fn null_frame_sentinel_round_trips() {
        let buf = build_valid_header(HeaderFields {
            frame_number: 42,
            frame_size: 0x40,
            ..HeaderFields::minimal()
        });
        let hdr = FrameHeader::parse(&buf).expect("null frame must parse");
        assert!(hdr.bitstream.is_null_frame());
        assert!(hdr.bitstream.frame_flags.is_inter());
        assert!(!hdr.bitstream.frame_flags.periodic_intra());
        assert!(!hdr.bitstream.frame_flags.intra());
    }

    #[test]
    fn frame_flags_decode_named_bits() {
        // Set every named bit at once.
        let raw = 0x0001 | FLAG_YVU9_8BIT | 0x0004 | 0x0008 | 0x0010 | 0x0020 | 0x0100 | 0x0200;
        let f = FrameFlags(raw);
        assert_eq!(f.bits(), raw);
        assert!(f.periodic_intra());
        assert!(f.yvu9_8bit());
        assert!(f.intra());
        assert!(f.next_intra_hint());
        assert!(f.mv_halfpel_horiz());
        assert!(f.mv_halfpel_vert());
        assert!(f.droppable_inter());
        assert!(f.buffer_selector());
        assert!(!f.is_inter());

        // INTER frames have neither bit 0 nor bit 2.
        let inter = FrameFlags(0x0200);
        assert!(inter.is_inter());
        assert!(!inter.periodic_intra());
        assert!(!inter.intra());
        assert!(inter.buffer_selector());
    }

    #[test]
    fn cb_offset_is_signed() {
        // §3.4 — the parser sign-extends the byte at the read
        // site, so 0xff must surface as -1.
        let buf = build_valid_header(HeaderFields {
            cb_offset: -1,
            ..HeaderFields::minimal()
        });
        let hdr = FrameHeader::parse(&buf).expect("cb_offset = -1 must parse");
        assert_eq!(hdr.bitstream.cb_offset, -1);
    }

    #[test]
    fn alt_quant_round_trips_and_splits() {
        // §3.9 — the parser preserves the 16 bytes verbatim;
        // the high nibble selects the primary table, the low
        // nibble the secondary.
        let mut tbl = [0u8; 16];
        for (i, slot) in tbl.iter_mut().enumerate() {
            *slot = ((i as u8) << 4) | (15 - i as u8);
        }
        let buf = build_valid_header(HeaderFields {
            alt_quant: tbl,
            ..HeaderFields::minimal()
        });
        let hdr = FrameHeader::parse(&buf).expect("must parse");
        assert_eq!(hdr.bitstream.alt_quant, tbl);

        for (i, byte) in tbl.iter().enumerate() {
            let (primary, secondary) = alt_quant_indices(*byte);
            assert_eq!(primary, i as u8);
            assert_eq!(secondary, 15 - i as u8);
        }
    }

    #[test]
    fn bsh_checksum_and_reserved_fields_are_tolerated() {
        // §3.5 / §3.8 / §6 — the parser does not validate the
        // bitstream `checksum`, `reserved1`, `reserved2`, or
        // the frame header's `unknown1`. We must surface them
        // verbatim regardless of value.
        let buf = build_valid_header(HeaderFields {
            frame_number: 0x1234_5678,
            unknown1: 0xdead_beef,
            frame_flags: 0x0005,
            cb_offset: -42,
            reserved1: 0xaa,
            bsh_checksum: 0xcafe,
            height: 240,
            width: 320,
            reserved2: 0xfeed_face,
            ..HeaderFields::minimal()
        });
        let hdr = FrameHeader::parse(&buf).expect("must parse");
        assert_eq!(hdr.frame.unknown1, 0xdead_beef);
        assert_eq!(hdr.bitstream.reserved1, 0xaa);
        assert_eq!(hdr.bitstream.checksum, 0xcafe);
        assert_eq!(hdr.bitstream.reserved2, 0xfeed_face);
        assert_eq!(hdr.bitstream.cb_offset, -42);
    }

    #[test]
    fn header_size_constants_match_spec() {
        // §2 + §3 byte-map sanity.
        assert_eq!(FRAME_HEADER_LEN, 16);
        assert_eq!(BITSTREAM_HEADER_LEN, 48);
        assert_eq!(COMBINED_HEADER_LEN, 64);
    }

    #[test]
    fn frmh_magic_constant_matches_spec() {
        // §2.1 — 'FRMH' read as a little-endian DWORD.
        assert_eq!(MAGIC_FRMH, u32::from_le_bytes(*b"HMRF"));
        // The reading-order spelling 'F','R','M','H' = the same
        // numeric DWORD because the on-disk byte sequence is
        // reversed.
        let frmh = *b"FRMH";
        let on_disk = *b"HMRF";
        assert_eq!(MAGIC_FRMH, u32::from_be_bytes(frmh));
        assert_eq!(MAGIC_FRMH, u32::from_le_bytes(on_disk));
    }
}
