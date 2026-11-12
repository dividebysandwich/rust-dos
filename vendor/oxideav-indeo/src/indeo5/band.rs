//! Indeo 5 band header (per coded band).
//!
//! Spec source: `docs/video/indeo/indeo5/spec/02-gop-and-band-layer.md`
//! §3.
//!
//! Each coded band carries its own header at the start of the band's
//! payload (`spec/02 §3`). Unlike the GOP / frame headers, the band
//! header starts on a whole-byte boundary (it re-initialises its own
//! bit accumulator from the per-band data pointer), so
//! [`BandHeader::parse`] takes a fresh [`BitReader`] over the band's
//! bytes.
//!
//! The header signals band emptiness (an early-exit fast path),
//! motion-vector inheritance, optional `qdelta` presence, an optional
//! run-value table correction array, an optional rv-table selector, an
//! optional block-Huffman descriptor, an optional checksum, the
//! mandatory 5-bit global quantiser, and an optional opaque extension.
//!
//! The `band_data_size` field (§3.2) is gated by a **frame-level**
//! flag (`frame_flags` bit 7, `spec/02 §3.2`), not a band-level flag,
//! so [`BandHeader::parse`] takes that gate as a parameter.

use super::bitreader::{BitReader, BitReaderError};
use super::frame::HuffDesc;

/// Spec/02 §3.4 — the maximum accepted `num_rv_corr` (`> 61` rejected).
pub const MAX_RV_CORR: u32 = 61;

/// Spec/02 §3.5 — the default `rv_tab_sel` when the selector bit is
/// clear.
pub const DEFAULT_RV_TAB_SEL: u32 = 8;

/// Errors raised while parsing the band header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BandError {
    /// Spec/02 §3.4 — `num_rv_corr` exceeded [`MAX_RV_CORR`]; the
    /// parser takes the early-exit at `IR50_32.DLL!0x1001e0c3`.
    TooManyRvCorr {
        /// The `num_rv_corr` value found.
        found: u32,
    },
    /// Underlying bit-reader fault.
    BitReader(BitReaderError),
}

impl From<BitReaderError> for BandError {
    fn from(e: BitReaderError) -> Self {
        BandError::BitReader(e)
    }
}

impl core::fmt::Display for BandError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BandError::TooManyRvCorr { found } => write!(
                f,
                "indeo5 band: num_rv_corr {found} exceeds the maximum {MAX_RV_CORR} (spec/02 §3.4)"
            ),
            BandError::BitReader(e) => write!(f, "indeo5 band: {e}"),
        }
    }
}

impl std::error::Error for BandError {}

/// Spec/02 §3.1 — the decoded 8-bit `band_flags` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandFlags {
    /// Raw 8-bit value (stored at `[band+0x3c]`).
    pub raw: u8,
}

impl BandFlags {
    /// Bit 0 — empty band (no coded data; early-exit fast path §3.3).
    pub fn empty_band(self) -> bool {
        self.raw & 0x01 != 0
    }
    /// Bit 1 — motion-vector inheritance mode for this band.
    pub fn mv_inherit(self) -> bool {
        self.raw & 0x02 != 0
    }
    /// Bit 2 — per-block qdelta values present.
    pub fn qdelta_present(self) -> bool {
        self.raw & 0x04 != 0
    }
    /// Bit 3 — qdelta inherited from the parent band.
    pub fn qdelta_inherit(self) -> bool {
        self.raw & 0x08 != 0
    }
    /// Bit 4 — the rv-table correction array follows (§3.4).
    pub fn rv_corr_present(self) -> bool {
        self.raw & 0x10 != 0
    }
    /// Bit 5 — the opaque-extension loop reads (§3.9).
    pub fn hdr_ext_present(self) -> bool {
        self.raw & 0x20 != 0
    }
    /// Bit 6 — a 3-bit `rv_tab_sel` follows (§3.5).
    pub fn rv_sel_present(self) -> bool {
        self.raw & 0x40 != 0
    }
    /// Bit 7 — an explicit `blk_huff_desc` follows (§3.6).
    pub fn blk_huff_present(self) -> bool {
        self.raw & 0x80 != 0
    }
}

/// Spec/02 §3 — the parsed band header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BandHeader {
    /// The 8-bit `band_flags` (§3.1).
    pub flags: BandFlags,
    /// `true` when the band is empty (§3.3 early exit). When empty,
    /// all subsequent fields are `None`/default and `band_glob_quant`
    /// was not read.
    pub empty: bool,
    /// `band_data_size` (§3.2, 24 bits) when the frame-level gate and
    /// the band was non-empty.
    pub band_data_size: Option<u32>,
    /// rv-table correction pairs (`(lookup, value)` bytes, §3.4) when
    /// `rv_corr_present`.
    pub rv_tab_corr: Vec<(u8, u8)>,
    /// The effective `rv_tab_sel` (§3.5): the read value, or
    /// [`DEFAULT_RV_TAB_SEL`] when the selector bit was clear.
    pub rv_tab_sel: u32,
    /// `blk_huff_desc` (§3.6) when `blk_huff_present`; `None` means the
    /// band uses the default descriptor.
    pub blk_huff_desc: Option<HuffDesc>,
    /// `band_checksum` (§3.7, 16 bits) when the 1-bit checksum flag was
    /// set.
    pub band_checksum: Option<u16>,
    /// `band_glob_quant` (§3.8, 5 bits) — the per-band global quantiser
    /// scale (`0..=31`). `None` for an empty band.
    pub band_glob_quant: Option<u8>,
}

impl BandHeader {
    /// Parse a band header from a fresh byte-aligned reader over the
    /// band's payload bytes.
    ///
    /// `frame_band_data_size_present` is the frame-level `frame_flags`
    /// bit 7 (`spec/02 §3.2`) that gates the `band_data_size` read.
    pub fn parse(
        r: &mut BitReader<'_>,
        frame_band_data_size_present: bool,
    ) -> Result<Self, BandError> {
        // §3.1 band_flags.
        let flags = BandFlags {
            raw: r.read(8)? as u8,
        };

        // §3.3 empty-band early exit: band occupies only the flags byte.
        if flags.empty_band() {
            return Ok(BandHeader {
                flags,
                empty: true,
                band_data_size: None,
                rv_tab_corr: Vec::new(),
                rv_tab_sel: DEFAULT_RV_TAB_SEL,
                blk_huff_desc: None,
                band_checksum: None,
                band_glob_quant: None,
            });
        }

        // §3.2 band_data_size (24 bits), gated by the frame-level flag.
        let band_data_size = if frame_band_data_size_present {
            Some(r.read(24)?)
        } else {
            None
        };

        // §3.4 num_rv_corr + rv_tab_corr pairs (conditional bit 4).
        let mut rv_tab_corr = Vec::new();
        if flags.rv_corr_present() {
            let num_rv_corr = r.read(8)?;
            if num_rv_corr > MAX_RV_CORR {
                return Err(BandError::TooManyRvCorr { found: num_rv_corr });
            }
            rv_tab_corr.reserve(num_rv_corr as usize);
            for _ in 0..num_rv_corr {
                let lookup = r.read(8)? as u8;
                let value = r.read(8)? as u8;
                rv_tab_corr.push((lookup, value));
            }
        }

        // §3.5 rv_tab_sel (3 bits, conditional bit 6; default 8).
        let rv_tab_sel = if flags.rv_sel_present() {
            r.read(3)?
        } else {
            DEFAULT_RV_TAB_SEL
        };

        // §3.6 blk_huff_desc (conditional bit 7).
        let blk_huff_desc = if flags.blk_huff_present() {
            Some(HuffDesc::parse_bits(r)?)
        } else {
            None
        };

        // §3.7 checksum_flag (1 bit) + band_checksum (16 bits).
        let checksum_flag = r.read_bit()? != 0;
        let band_checksum = if checksum_flag {
            Some(r.read(16)? as u16)
        } else {
            None
        };

        // §3.8 band_glob_quant (5 bits).
        let band_glob_quant = Some(r.read(5)? as u8);

        // §3.9 band_hdr_ext (conditional bit 5): align(8) then
        // do { len = read(8); skip len bytes } while (read(1)).
        if flags.hdr_ext_present() {
            r.align()?;
            loop {
                let len = r.read(8)?;
                for _ in 0..len {
                    r.read(8)?;
                }
                if r.read_bit()? == 0 {
                    break;
                }
            }
        }

        Ok(BandHeader {
            flags,
            empty: false,
            band_data_size,
            rv_tab_corr,
            rv_tab_sel,
            blk_huff_desc,
            band_checksum,
            band_glob_quant,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn band_flags_bits() {
        let f = BandFlags { raw: 0xff };
        assert!(f.empty_band());
        assert!(f.mv_inherit());
        assert!(f.qdelta_present());
        assert!(f.qdelta_inherit());
        assert!(f.rv_corr_present());
        assert!(f.hdr_ext_present());
        assert!(f.rv_sel_present());
        assert!(f.blk_huff_present());
    }

    #[test]
    fn parse_empty_band() {
        // band_flags bit0 set -> empty, only the flags byte consumed.
        let mut w = BitWriter::new();
        w.put(0x01, 8);
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let bh = BandHeader::parse(&mut r, false).unwrap();
        assert!(bh.empty);
        assert!(bh.band_glob_quant.is_none());
        assert_eq!(bh.rv_tab_sel, DEFAULT_RV_TAB_SEL);
    }

    #[test]
    fn parse_minimal_band() {
        // band_flags=0 (non-empty, no optional fields), checksum_flag=0,
        // band_glob_quant=17.
        let mut w = BitWriter::new();
        w.put(0x00, 8); // band_flags
        w.put(0, 1); // checksum_flag
        w.put(17, 5); // band_glob_quant
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let bh = BandHeader::parse(&mut r, false).unwrap();
        assert!(!bh.empty);
        assert_eq!(bh.band_glob_quant, Some(17));
        assert_eq!(bh.rv_tab_sel, DEFAULT_RV_TAB_SEL);
        assert!(bh.band_data_size.is_none());
    }

    #[test]
    fn parse_band_with_data_size() {
        // frame gate set -> 24-bit band_data_size after flags.
        let mut w = BitWriter::new();
        w.put(0x00, 8); // band_flags
        w.put(0x00abcd, 24); // band_data_size
        w.put(0, 1); // checksum_flag
        w.put(5, 5); // band_glob_quant
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let bh = BandHeader::parse(&mut r, true).unwrap();
        assert_eq!(bh.band_data_size, Some(0x00abcd));
        assert_eq!(bh.band_glob_quant, Some(5));
    }

    #[test]
    fn parse_band_with_rv_corr() {
        // band_flags bit4 (rv_corr_present). num=2, pairs (1,2),(3,4).
        let mut w = BitWriter::new();
        w.put(0x10, 8); // band_flags bit4
        w.put(2, 8); // num_rv_corr
        w.put(1, 8);
        w.put(2, 8);
        w.put(3, 8);
        w.put(4, 8);
        w.put(0, 1); // checksum_flag
        w.put(0, 5); // band_glob_quant
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let bh = BandHeader::parse(&mut r, false).unwrap();
        assert_eq!(bh.rv_tab_corr, vec![(1, 2), (3, 4)]);
    }

    #[test]
    fn parse_band_rv_corr_too_many() {
        let mut w = BitWriter::new();
        w.put(0x10, 8); // band_flags bit4
        w.put(62, 8); // num_rv_corr > 61
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        assert!(matches!(
            BandHeader::parse(&mut r, false),
            Err(BandError::TooManyRvCorr { found: 62 })
        ));
    }

    #[test]
    fn parse_band_with_rv_tab_sel() {
        // band_flags bit6 (rv_sel_present); rv_tab_sel=3.
        let mut w = BitWriter::new();
        w.put(0x40, 8); // band_flags bit6
        w.put(3, 3); // rv_tab_sel
        w.put(0, 1); // checksum_flag
        w.put(0, 5); // band_glob_quant
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let bh = BandHeader::parse(&mut r, false).unwrap();
        assert_eq!(bh.rv_tab_sel, 3);
    }

    #[test]
    fn parse_band_with_checksum() {
        let mut w = BitWriter::new();
        w.put(0x00, 8); // band_flags
        w.put(1, 1); // checksum_flag set
        w.put(0xdead, 16); // band_checksum
        w.put(0, 5); // band_glob_quant
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let bh = BandHeader::parse(&mut r, false).unwrap();
        assert_eq!(bh.band_checksum, Some(0xdead));
    }

    #[test]
    fn parse_band_with_blk_huff_preset() {
        // band_flags bit7 (blk_huff_present); preset id 2.
        let mut w = BitWriter::new();
        w.put(0x80, 8); // band_flags bit7
        w.put(2, 3); // huff id 2 (preset)
        w.put(0, 1); // checksum_flag
        w.put(0, 5); // band_glob_quant
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let bh = BandHeader::parse(&mut r, false).unwrap();
        assert_eq!(bh.blk_huff_desc, Some(HuffDesc::Preset { id: 2 }));
    }

    #[test]
    fn parse_band_with_hdr_ext() {
        // band_flags bit5 (hdr_ext_present). After glob_quant the ext:
        // align(8), then len=1 skip 1 byte, terminator bit 0.
        let mut w = BitWriter::new();
        w.put(0x20, 8); // band_flags bit5
        w.put(0, 1); // checksum_flag
        w.put(7, 5); // band_glob_quant
        w.align(); // ext align(8)
        w.put(1, 8); // ext len 1
        w.put(0xaa, 8); // skipped byte
        w.put(0, 1); // terminator -> stop
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let bh = BandHeader::parse(&mut r, false).unwrap();
        assert_eq!(bh.band_glob_quant, Some(7));
        assert!(bh.flags.hdr_ext_present());
    }
}
