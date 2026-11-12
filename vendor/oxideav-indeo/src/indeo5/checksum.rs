//! Indeo 5 frame / band checksum: parse-and-store-only.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/08-output-reconstruction.md`
//! §7 (checksum verification).
//!
//! The wiki documents a per-frame `frm_checksum` and a per-band
//! `band_checksum`, both 16-bit and both gated by a presence flag. The
//! Indeo 5 binary **parses and stores** both but **never computes or
//! compares** a corresponding checksum (`spec/08 §7.1`/`§7.2`/`§7.3`):
//! they are documented as "for debugging purposes". The only runtime
//! consumer is a **range validation** — the stored slot must fit in 16
//! bits (`cmp [ebx+0xe4], 0xffff; ja error`, `spec/08 §7.1`), which any
//! 16-bit read trivially satisfies.
//!
//! A clean-room decoder must therefore (`spec/08 §7.4`):
//!
//! 1. consume the correct number of bits per the presence flags,
//! 2. store the parsed value,
//! 3. **not** compute or compare a checksum, and
//! 4. surface the parsed bytes for optional downstream tooling.
//!
//! This module implements exactly that: the presence-flag gates, the
//! 16-bit LSB-first read (`spec/08 §7.1`, via the shared
//! [`BitReader`](crate::indeo5::BitReader)), the store, and the
//! range validation — with no checksum arithmetic.

use crate::indeo5::bitreader::{BitReader, BitReaderError};

/// `spec/08 §7.1` — `frame_flags` bit 4 (mask `0x10`) gates the
/// per-frame `frm_checksum`.
pub const FRAME_CHECKSUM_FLAG: u8 = 0x10;

/// A parsed checksum field (`spec/08 §7`). The value is stored verbatim;
/// the decoder never verifies it (`enforced` is always `false`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChecksumField {
    /// The 16-bit checksum value as read from the stream.
    pub value: u16,
}

impl ChecksumField {
    /// `spec/08 §7.1` — the range validation applied to the stored slot
    /// (`value <= 0xffff`). Always `true` for a 16-bit value; modelled
    /// explicitly to mirror the binary's `cmp .., 0xffff; ja error`
    /// guard on the u32 storage slot.
    #[inline]
    pub fn in_range(self) -> bool {
        u32::from(self.value) <= 0xffff
    }

    /// `spec/08 §7.3`/`§7.4` — whether the decoder enforces this
    /// checksum. Always `false`: the shipping decoder reads and stores
    /// checksums but never compares them against a computed value.
    #[inline]
    pub fn enforced(self) -> bool {
        false
    }
}

/// `spec/08 §7.1` — whether the per-frame `frm_checksum` is present,
/// per `frame_flags` bit 4.
#[inline]
pub fn frame_checksum_present(frame_flags: u8) -> bool {
    frame_flags & FRAME_CHECKSUM_FLAG != 0
}

/// `spec/08 §7.1` — parse the per-frame `frm_checksum`.
///
/// When `frame_flags` bit 4 is clear, no bits are consumed and `None`
/// is returned. When set, a 16-bit LSB-first value is read and returned
/// as a [`ChecksumField`] (store-only; not verified).
pub fn parse_frame_checksum(
    reader: &mut BitReader<'_>,
    frame_flags: u8,
) -> Result<Option<ChecksumField>, BitReaderError> {
    if !frame_checksum_present(frame_flags) {
        return Ok(None);
    }
    let value = reader.read(16)? as u16;
    Ok(Some(ChecksumField { value }))
}

/// `spec/08 §7.2` — parse the per-band `band_checksum`.
///
/// The per-band checksum is gated by a 1-bit `checksum_flag` in the
/// band header (`spec/08 §7.2`, wiki "Band header"). This reads that
/// flag; if set it reads the following 16-bit value. Store-only — the
/// value is never verified.
pub fn parse_band_checksum(
    reader: &mut BitReader<'_>,
) -> Result<Option<ChecksumField>, BitReaderError> {
    let present = reader.read_bit()? != 0;
    if !present {
        return Ok(None);
    }
    let value = reader.read(16)? as u16;
    Ok(Some(ChecksumField { value }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_flag_gate() {
        // spec/08 §7.1: bit 4.
        assert!(frame_checksum_present(0x10));
        assert!(frame_checksum_present(0xff));
        assert!(!frame_checksum_present(0x0f));
        assert!(!frame_checksum_present(0x00));
    }

    #[test]
    fn frame_checksum_absent_consumes_no_bits() {
        // 4-byte prefetch minimum; flags without bit 4 -> no read.
        let data = [0xaa, 0xbb, 0xcc, 0xdd];
        let mut r = BitReader::new(&data).unwrap();
        let before = r.bits_read();
        let cs = parse_frame_checksum(&mut r, 0x00).unwrap();
        assert_eq!(cs, None);
        assert_eq!(r.bits_read(), before);
    }

    #[test]
    fn frame_checksum_reads_16_bits_lsb_first() {
        // Accumulator seeds with the LE dword 0x44332211; low 16 bits
        // = 0x2211 read LSB-first.
        let data = [0x11, 0x22, 0x33, 0x44];
        let mut r = BitReader::new(&data).unwrap();
        let cs = parse_frame_checksum(&mut r, 0x10).unwrap().unwrap();
        assert_eq!(cs.value, 0x2211);
        assert_eq!(r.bits_read(), 16);
        assert!(cs.in_range());
        assert!(!cs.enforced());
    }

    #[test]
    fn band_checksum_flag_zero_skips() {
        // First bit (LSB of 0x11) = 1 -> present; use a byte whose LSB
        // is 0 to exercise the skip path.
        let data = [0x00, 0x00, 0x00, 0x00];
        let mut r = BitReader::new(&data).unwrap();
        let cs = parse_band_checksum(&mut r).unwrap();
        assert_eq!(cs, None);
        assert_eq!(r.bits_read(), 1);
    }

    #[test]
    fn band_checksum_flag_one_reads_value() {
        // LSB of first byte = 1 -> present. After the 1-bit flag, the
        // next 16 bits are read LSB-first.
        // dword = 0x......_01; bit0 = 1 (flag). Remaining accumulator
        // >>1 then 16-bit read.
        let data = [0x01, 0x00, 0x02, 0x00];
        let mut r = BitReader::new(&data).unwrap();
        let cs = parse_band_checksum(&mut r).unwrap().unwrap();
        // accumulator = 0x00020001; after reading flag (bit0=1), acc>>1
        // = 0x00010000, low 16 bits = 0x0000.
        assert_eq!(cs.value, 0x0000);
        assert_eq!(r.bits_read(), 17);
        assert!(!cs.enforced());
    }

    #[test]
    fn checksum_field_always_in_range() {
        // spec/08 §7.1: any 16-bit value fits.
        assert!(ChecksumField { value: 0 }.in_range());
        assert!(ChecksumField { value: 0xffff }.in_range());
    }
}
