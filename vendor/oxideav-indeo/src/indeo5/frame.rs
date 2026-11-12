//! Indeo 5 frame header (frame_type 0..3, except NULL).
//!
//! Spec source: `docs/video/indeo/indeo5/spec/02-gop-and-band-layer.md`
//! §1.9, §1.10, and §2.
//!
//! After the GOP header (INTRA frames) or directly after the
//! picture-start triplet (INTER / droppable frames), the parser reads
//! the frame header. For INTRA frames the parser first consumes the
//! §1.9 GOP-trailing fixed fields + the optional `gop_ext` loop
//! (joined with the frame-header path without re-aligning, `spec/02
//! §1.10`); then, for all non-NULL frames, the §2 frame header proper:
//! a byte alignment, `frame_flags`, the conditional `pic_hdr_size` /
//! `frm_checksum` / `frm_hdr_ext` / `mb_huff_desc` fields, a 3-bit
//! `value5`, and a final byte alignment.
//!
//! [`FrameHeader::parse`] threads these in order, leaving the bit
//! reader byte-aligned at the start of the per-band payload (`spec/02
//! §3`). The Huffman codebook *descriptor* (`mb_huff_desc`) is parsed
//! to the extent the §2.6 grammar pins down (preset id vs. custom
//! row-length table); the canonical VLC construction from the
//! bit-lengths is a later chapter's subject (`spec/02 §6` item 4).

use super::bitreader::{BitReader, BitReaderError};

/// Errors raised while parsing the frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameError {
    /// Underlying bit-reader fault.
    BitReader(BitReaderError),
}

impl From<BitReaderError> for FrameError {
    fn from(e: BitReaderError) -> Self {
        FrameError::BitReader(e)
    }
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FrameError::BitReader(e) => write!(f, "indeo5 frame: {e}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Spec/02 §1.9 — the GOP-trailing fixed fields + optional `gop_ext`,
/// read only for INTRA frames before the §2 frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GopTrailer {
    /// `value1` (8 bits).
    pub value1: u8,
    /// `value2` (8 bits).
    pub value2: u8,
    /// `value3` (3 bits).
    pub value3: u8,
    /// `value4` (4 bits); its bit 3 is `gop_ext_flg`.
    pub value4: u8,
    /// `true` when `value4` bit 3 was set, triggering the `gop_ext`
    /// continuation loop.
    pub gop_ext_present: bool,
}

impl GopTrailer {
    /// Parse the §1.9 trailing block. The `gop_ext` loop reads-and-
    /// discards 16-bit words while the high bit (`& 0x8000`) is set
    /// (`spec/02 §1.9`).
    fn parse(r: &mut BitReader<'_>) -> Result<Self, FrameError> {
        let value1 = r.read(8)? as u8;
        let value2 = r.read(8)? as u8;
        let value3 = r.read(3)? as u8;
        let value4 = r.read(4)? as u8;
        let gop_ext_present = value4 & 0x8 != 0;
        if gop_ext_present {
            // do { word = read(16) } while (word & 0x8000)
            loop {
                let word = r.read(16)?;
                if word & 0x8000 == 0 {
                    break;
                }
            }
        }
        Ok(GopTrailer {
            value1,
            value2,
            value3,
            value4,
            gop_ext_present,
        })
    }
}

/// Spec/02 §2.2 — the decoded 8-bit `frame_flags` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameFlags {
    /// Raw 8-bit value (stored at `[ebx+0xbe]`).
    pub raw: u8,
}

impl FrameFlags {
    /// Bit 0 — if set, a 24-bit `pic_hdr_size` follows (§2.3).
    pub fn pic_hdr_size_present(self) -> bool {
        self.raw & 0x01 != 0
    }
    /// Bit 2 — per-frame scratch flag (writes `[ebx+0x124]`).
    pub fn flag_b2(self) -> bool {
        self.raw & 0x04 != 0
    }
    /// Bit 3 — per-frame flag (writes `[ebx+0x120]`): every MB of the
    /// first luma band codes a qdelta, coded or not (`MbContext::
    /// qdelta_always`).
    pub fn flag_b3(self) -> bool {
        self.raw & 0x08 != 0
    }
    /// Bit 4 — if set, a 16-bit `frm_checksum` follows (§2.4).
    pub fn checksum_present(self) -> bool {
        self.raw & 0x10 != 0
    }
    /// Bit 5 — if set, the `frm_hdr_ext` opaque-extension loop reads
    /// (§2.5).
    pub fn hdr_ext_present(self) -> bool {
        self.raw & 0x20 != 0
    }
    /// Bit 6 — if set, an `mb_huff_desc` descriptor follows (§2.6).
    pub fn mb_huff_present(self) -> bool {
        self.raw & 0x40 != 0
    }
    /// Bit 7 — per-band `band_data_size` present (consumed by the §3
    /// band-header parser, gated by this frame-level flag).
    pub fn band_data_size_present(self) -> bool {
        self.raw & 0x80 != 0
    }
}

/// Spec/02 §2.6 — a Huffman-codebook descriptor (shared format with
/// the band-header's `blk_huff_desc`, §3.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HuffDesc {
    /// A 3-bit preset id `0..=6` selecting one of the seven preset
    /// codebooks (`[ebx + 0x1f0 + id*0x510]`).
    Preset {
        /// The preset id.
        id: u32,
    },
    /// A custom descriptor (id == 7): `num_rows` 4-bit count followed
    /// by per-row 4-bit bit-length values.
    Custom {
        /// The per-row bit-length values (`num_rows` entries).
        row_lengths: Vec<u8>,
    },
}

impl HuffDesc {
    /// Parse a Huffman descriptor (`spec/02 §2.6` / §3.6) from a bit
    /// reader. Reads the 3-bit id; for id == 7 reads the custom
    /// row-length table. Shared by the frame-header `mb_huff_desc` and
    /// the band-header `blk_huff_desc`.
    pub fn parse_bits(r: &mut BitReader<'_>) -> Result<Self, BitReaderError> {
        let id = r.read(3)?;
        if id == 7 {
            let num_rows = r.read(4)?;
            let mut row_lengths = Vec::with_capacity(num_rows as usize);
            for _ in 0..num_rows {
                row_lengths.push(r.read(4)? as u8);
            }
            Ok(HuffDesc::Custom { row_lengths })
        } else {
            Ok(HuffDesc::Preset { id })
        }
    }
}

/// Spec/02 §2 — the parsed frame header.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FrameHeader {
    /// The §1.9 GOP-trailing block (present for INTRA frames only).
    pub gop_trailer: Option<GopTrailer>,
    /// The 8-bit `frame_flags` (§2.2).
    pub flags: FrameFlags,
    /// `pic_hdr_size` (§2.3, 24 bits) when present.
    pub pic_hdr_size: Option<u32>,
    /// `frm_checksum` (§2.4, 16 bits) when present.
    pub frm_checksum: Option<u16>,
    /// `mb_huff_desc` (§2.6) when present.
    pub mb_huff_desc: Option<HuffDesc>,
    /// `value5` (§2.7, 3 bits).
    pub value5: u8,
}

impl FrameHeader {
    /// Parse the frame header from a bit reader positioned after the
    /// GOP header (INTRA) or the picture-start triplet (INTER /
    /// droppable). `is_intra` selects whether the §1.9 GOP-trailing
    /// block is read first. The reader is left byte-aligned at the
    /// start of the per-band payload (`spec/02 §2.8`).
    pub fn parse(r: &mut BitReader<'_>, is_intra: bool) -> Result<Self, FrameError> {
        // §1.9 GOP-trailing (INTRA only, no re-align before it per
        // §1.10).
        let gop_trailer = if is_intra {
            Some(GopTrailer::parse(r)?)
        } else {
            None
        };

        // §2.1 pre-frame-header alignment.
        r.align()?;

        // §2.2 frame_flags.
        let flags = FrameFlags {
            raw: r.read(8)? as u8,
        };

        // §2.3 pic_hdr_size (24 bits, conditional).
        let pic_hdr_size = if flags.pic_hdr_size_present() {
            Some(r.read(24)?)
        } else {
            None
        };

        // §2.4 frm_checksum (16 bits, conditional).
        let frm_checksum = if flags.checksum_present() {
            Some(r.read(16)? as u16)
        } else {
            None
        };

        // §2.5 frm_hdr_ext (variable, conditional): a length-prefixed
        // opaque-extension loop, skipped.
        if flags.hdr_ext_present() {
            loop {
                let len = r.read(8)?;
                for _ in 0..len {
                    r.read(8)?;
                }
                if len == 0 {
                    break;
                }
            }
        }

        // §2.6 mb_huff_desc (conditional).
        let mb_huff_desc = if flags.mb_huff_present() {
            Some(HuffDesc::parse_bits(r)?)
        } else {
            None
        };

        // §2.7 value5 (3 bits).
        let value5 = r.read(3)? as u8;

        // §2.8 frame-header alignment exit.
        r.align()?;

        Ok(FrameHeader {
            gop_trailer,
            flags,
            pic_hdr_size,
            frm_checksum,
            mb_huff_desc,
            value5,
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
    fn frame_flags_bits() {
        let f = FrameFlags { raw: 0xd1 };
        // 0xd1 = 1101_0001
        assert!(f.pic_hdr_size_present()); // bit0
        assert!(!f.flag_b2()); // bit2
        assert!(!f.flag_b3()); // bit3
        assert!(f.checksum_present()); // bit4
        assert!(!f.hdr_ext_present()); // bit5
        assert!(f.mb_huff_present()); // bit6
        assert!(f.band_data_size_present()); // bit7
    }

    #[test]
    fn parse_minimal_inter_frame_header() {
        // INTER: no GOP trailer. frame_flags=0 (no optional fields),
        // value5=0.
        let mut w = BitWriter::new();
        w.put(0x00, 8); // frame_flags
        w.put(0, 3); // value5
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let fh = FrameHeader::parse(&mut r, false).unwrap();
        assert!(fh.gop_trailer.is_none());
        assert_eq!(fh.flags.raw, 0);
        assert!(fh.pic_hdr_size.is_none());
        assert!(fh.frm_checksum.is_none());
        assert!(fh.mb_huff_desc.is_none());
        // Reader left byte-aligned.
        assert_eq!(r.bits_read() % 8, 0);
    }

    #[test]
    fn parse_frame_header_with_pic_hdr_size_and_checksum() {
        // frame_flags bit0 (pic_hdr_size) + bit4 (checksum).
        let mut w = BitWriter::new();
        w.put(0x11, 8); // bits 0 and 4
        w.put(0x012345, 24); // pic_hdr_size
        w.put(0xbeef, 16); // frm_checksum
        w.put(0, 3); // value5
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let fh = FrameHeader::parse(&mut r, false).unwrap();
        assert_eq!(fh.pic_hdr_size, Some(0x012345));
        assert_eq!(fh.frm_checksum, Some(0xbeef));
    }

    #[test]
    fn parse_frame_header_mb_huff_preset() {
        // bit6 mb_huff_present; preset id 3.
        let mut w = BitWriter::new();
        w.put(0x40, 8);
        w.put(3, 3); // huff id 3 (preset)
        w.put(0, 3); // value5
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let fh = FrameHeader::parse(&mut r, false).unwrap();
        assert_eq!(fh.mb_huff_desc, Some(HuffDesc::Preset { id: 3 }));
    }

    #[test]
    fn parse_frame_header_mb_huff_custom() {
        // bit6 mb_huff_present; id 7 (custom), num_rows=2, lengths 4,5.
        let mut w = BitWriter::new();
        w.put(0x40, 8);
        w.put(7, 3); // huff id 7 -> custom
        w.put(2, 4); // num_rows
        w.put(4, 4); // row length 0
        w.put(5, 4); // row length 1
        w.put(0, 3); // value5
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let fh = FrameHeader::parse(&mut r, false).unwrap();
        assert_eq!(
            fh.mb_huff_desc,
            Some(HuffDesc::Custom {
                row_lengths: vec![4, 5]
            })
        );
    }

    #[test]
    fn parse_frame_header_hdr_ext_skipped() {
        // bit5 hdr_ext_present; ext loop: len=2 (skip 2 bytes), len=0.
        let mut w = BitWriter::new();
        w.put(0x20, 8); // frame_flags bit5
        w.put(2, 8); // ext len 2
        w.put(0xaa, 8); // skipped
        w.put(0xbb, 8); // skipped
        w.put(0, 8); // ext len 0 -> terminate
        w.put(0, 3); // value5
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let fh = FrameHeader::parse(&mut r, false).unwrap();
        assert_eq!(fh.flags.raw, 0x20);
    }

    #[test]
    fn parse_intra_frame_header_with_gop_trailer() {
        // INTRA: GOP trailer first. value1=1, value2=2, value3=3,
        // value4=0 (no gop_ext), then align, frame_flags=0, value5=0.
        let mut w = BitWriter::new();
        w.put(1, 8); // value1
        w.put(2, 8); // value2
        w.put(3, 3); // value3
        w.put(0, 4); // value4 (gop_ext_flg clear)
        // §2.1 align happens in parser. Emit alignment padding to match.
        w.align();
        w.put(0x00, 8); // frame_flags
        w.put(0, 3); // value5
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let fh = FrameHeader::parse(&mut r, true).unwrap();
        let tr = fh.gop_trailer.unwrap();
        assert_eq!(tr.value1, 1);
        assert_eq!(tr.value2, 2);
        assert_eq!(tr.value3, 3);
        assert!(!tr.gop_ext_present);
    }

    #[test]
    fn parse_gop_trailer_with_ext() {
        // value4 bit3 set -> gop_ext loop. Two words: first 0x8001
        // (continue), second 0x0002 (stop).
        let mut w = BitWriter::new();
        w.put(0, 8); // value1
        w.put(0, 8); // value2
        w.put(0, 3); // value3
        w.put(0x8, 4); // value4 with bit3 -> gop_ext_flg
        w.put(0x8001, 16); // word, high bit set -> continue
        w.put(0x0002, 16); // word, high bit clear -> stop
        w.align();
        w.put(0x00, 8); // frame_flags
        w.put(0, 3); // value5
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let fh = FrameHeader::parse(&mut r, true).unwrap();
        assert!(fh.gop_trailer.unwrap().gop_ext_present);
    }
}
