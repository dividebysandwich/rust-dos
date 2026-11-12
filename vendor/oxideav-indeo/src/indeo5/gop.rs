//! Indeo 5 GOP header (frame_type == 0 / INTRA).
//!
//! Spec source: `docs/video/indeo/indeo5/spec/02-gop-and-band-layer.md`
//! §1.
//!
//! The GOP header follows the 16-bit picture-start triplet on INTRA
//! frames (`spec/01 §3.5`). It declares the parameters that govern the
//! whole group-of-pictures: subsampling, wavelet decomposition levels
//! and the derived per-plane band counts, picture dimensions (preset
//! or custom, overriding the format descriptor), a per-band `band_info`
//! descriptor array, and an optional transparency colour.
//!
//! [`GopHeader::parse`] consumes the determinable GOP fields from the
//! bit cursor [`super::PictureStart::parse`] leaves at bit 16, stopping
//! at the §1.9 GOP-trailing / extension block (which the spec joins
//! with the frame-header path — `spec/02 §1.10` — and is parsed by the
//! frame-header module). The fields this module deliberately leaves to
//! later passes are documented inline and in the round report:
//!
//! * `lock_word` (§1.3) is a documented open question — the GOP parser
//!   in the binary contains no `read(32)` for it (`spec/02 §6` item 1),
//!   so this module does not consume it and surfaces only the gate bit.
//! * The §1.9 GOP-trailing fixed fields and the `gop_ext` loop are
//!   parsed at the frame-header boundary (`spec/02 §1.10`).

use super::bitreader::{BitReader, BitReaderError};
use super::pic_size;

/// Spec/02 §1.5 — accepted combined `decomp_levels` values. Any other
/// value is rejected with parser sentinel `2`.
const DECOMP_LEVELS_ACCEPTED: [u32; 4] = [0, 1, 2, 6];

/// Errors raised while parsing the GOP header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GopError {
    /// Spec/02 §1.5 — the 3-bit `decomp_levels` field held a forbidden
    /// value (`> 2 && != 6`), parser sentinel `2`
    /// (`IR50_32.DLL!0x1002365c`).
    BadDecompLevels {
        /// The 3-bit combined value found.
        found: u32,
    },
    /// Spec/02 §1.6 — `pic_size_id` selected an unused zero-dimension
    /// table slot (`12..=14`), yielding a `0x0` picture.
    ZeroPictureSize {
        /// The 4-bit `pic_size_id` found.
        pic_size_id: u32,
    },
    /// Spec/02 §1.8 — the transparency block's low-3 alignment bits
    /// were non-zero, parser sentinel `0xe` (`IR50_32.DLL!0x1002409d`).
    BadTransparencyAlignment {
        /// The 4-bit transparency field found.
        found: u32,
    },
    /// Underlying bit-reader fault.
    BitReader(BitReaderError),
}

impl From<BitReaderError> for GopError {
    fn from(e: BitReaderError) -> Self {
        GopError::BitReader(e)
    }
}

impl core::fmt::Display for GopError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GopError::BadDecompLevels { found } => write!(
                f,
                "indeo5 gop: forbidden decomp_levels {found} (accepted: 0,1,2,6) (spec/02 §1.5)"
            ),
            GopError::ZeroPictureSize { pic_size_id } => write!(
                f,
                "indeo5 gop: pic_size_id {pic_size_id} maps to a 0x0 picture (spec/02 §1.6)"
            ),
            GopError::BadTransparencyAlignment { found } => write!(
                f,
                "indeo5 gop: transparency alignment bits non-zero in {found:#x} (spec/02 §1.8)"
            ),
            GopError::BitReader(e) => write!(f, "indeo5 gop: {e}"),
        }
    }
}

impl std::error::Error for GopError {}

/// Spec/02 §1.2 — chroma subsampling mode, from `gop_flags` bit 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subsampling {
    /// Bit clear — YVU9 (4:1:0); chroma is quarter-resolution in each
    /// axis (`(dim + 3) >> 2`).
    Yvu9,
    /// Bit set — YV12 (4:2:0); chroma is half-resolution in each axis
    /// (`(dim + 1) >> 1`).
    Yv12,
}

impl Subsampling {
    /// Chroma width for a given luma width (`spec/02 §1.6`).
    pub fn chroma_width(self, width: u32) -> u32 {
        match self {
            Subsampling::Yv12 => (width + 1) >> 1,
            Subsampling::Yvu9 => (width + 3) >> 2,
        }
    }

    /// Chroma height for a given luma height (`spec/02 §1.6`).
    pub fn chroma_height(self, height: u32) -> u32 {
        match self {
            Subsampling::Yv12 => (height + 1) >> 1,
            Subsampling::Yvu9 => (height + 3) >> 2,
        }
    }
}

/// Spec/02 §1.1 — the decoded 8-bit `gop_flags` field, with the named
/// bits surfaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GopFlags {
    /// Raw 8-bit value (stored at `[ebx+0xbd]`).
    pub raw: u8,
}

impl GopFlags {
    /// Bit 0 — if set, a 16-bit `gop_hdr_size` follows (§1.2 of spec).
    pub fn hdr_size_present(self) -> bool {
        self.raw & 0x01 != 0
    }
    /// Bit 1 — subsampling selector (§1.2).
    pub fn subsampling(self) -> Subsampling {
        if self.raw & 0x02 != 0 {
            Subsampling::Yv12
        } else {
            Subsampling::Yvu9
        }
    }
    /// Bit 3 — gates the §1.8 transparency block.
    pub fn transparency_present(self) -> bool {
        self.raw & 0x08 != 0
    }
    /// Bit 5 — `lock_word_present` (§1.3; the field itself is a
    /// documented open question and not consumed here).
    pub fn lock_word_present(self) -> bool {
        self.raw & 0x20 != 0
    }
    /// Bit 6 — if set, a 2-bit `slice_size_id` follows (§1.4).
    pub fn slice_size_present(self) -> bool {
        self.raw & 0x40 != 0
    }
}

/// Spec/02 §1.5 — wavelet decomposition levels + the derived per-plane
/// band counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecompLevels {
    /// `luma_levels` = bits 1:0 of the combined field (0, 1, or 2).
    pub luma_levels: u32,
    /// `chroma_levels` = bit 2 of the combined field (0 or 1).
    pub chroma_levels: u32,
    /// `luma_levels * 3 + 1` — number of luma bands (1, 4, or 7).
    pub luma_bands: u32,
    /// `chroma_levels * 3 + 1` — number of chroma bands (1 or 4).
    pub chroma_bands: u32,
}

impl DecompLevels {
    /// Derive from the 3-bit combined field, validating it against the
    /// accepted set `{0, 1, 2, 6}` (`spec/02 §1.5`).
    pub fn from_combined(combined: u32) -> Result<Self, GopError> {
        if !DECOMP_LEVELS_ACCEPTED.contains(&combined) {
            return Err(GopError::BadDecompLevels { found: combined });
        }
        let luma_levels = combined & 0x3;
        let chroma_levels = (combined >> 2) & 0x1;
        Ok(DecompLevels {
            luma_levels,
            chroma_levels,
            luma_bands: luma_levels * 3 + 1,
            chroma_bands: chroma_levels * 3 + 1,
        })
    }
}

/// Spec/02 §1.7 — a single per-band `band_info` descriptor.
///
/// The core 6-bit field packs `mv_res`, `mb_size_id`, `blk_size_id`,
/// `trans_flg`, and a 2-bit end marker; when `trans_flg` is set an
/// extra 2-bit `ext_trans` selects an explicit transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandInfo {
    /// Bit 0 — motion-vector resolution: `false` = fullpel, `true` =
    /// halfpel.
    pub mv_halfpel: bool,
    /// Bit 1 — macroblock-size selector.
    pub mb_size_id: u32,
    /// Bit 2 — block-size selector (`false` = 8×8, `true` = 4×4).
    pub blk_size_id: u32,
    /// Derived macroblock size (`mb_size_table[mb_size_id |
    /// blk_size_id << 1]`).
    pub mb_size: u32,
    /// Derived block size (`blk_size_table[...]`).
    pub blk_size: u32,
    /// Block-size log2 (3 for 8×8, 2 for 4×4).
    pub block_log2: u32,
    /// Selected transform id (`ext_trans` when explicit, else the
    /// default/standard transform).
    pub transform_id: TransformId,
}

/// Spec/02 §1.7 — the `mb_size` lookup table indexed by the 2-bit
/// `(mb_size_id | blk_size_id << 1)` selector.
pub const MB_SIZE_TABLE: [u32; 4] = [0x10, 0x08, 0x08, 0x04];

/// Spec/02 §1.7 — the `blk_size` lookup table, same index.
pub const BLK_SIZE_TABLE: [u32; 4] = [0x08, 0x08, 0x04, 0x04];

/// Spec/02 §1.7 — the explicit `ext_trans` transform selector (read
/// only when `trans_flg` is set), plus the standard-transform default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformId {
    /// `ext_trans == 0` — 2D Slant.
    Slant2d,
    /// `ext_trans == 1` — Row Slant.
    SlantRow,
    /// `ext_trans == 2` — Column Slant.
    SlantColumn,
    /// `ext_trans == 3` — no transform.
    None,
    /// `trans_flg` clear — the band uses its standard transform for its
    /// frequency content; the concrete transform is resolved per-band
    /// position at reconstruction time (`spec/02 §1.7` `[band+0x47]`
    /// fallback; the LL→2D / HL→Row / LH→Column / HH→none mapping is a
    /// later chapter's subject).
    Standard,
}

impl TransformId {
    fn from_ext_trans(ext_trans: u32) -> Self {
        match ext_trans {
            0 => TransformId::Slant2d,
            1 => TransformId::SlantRow,
            2 => TransformId::SlantColumn,
            _ => TransformId::None,
        }
    }
}

impl BandInfo {
    /// Parse one `band_info` descriptor (`spec/02 §1.7`).
    fn parse(r: &mut BitReader<'_>) -> Result<Self, GopError> {
        let core = r.read(6)?;
        let mv_halfpel = core & 0x01 != 0;
        let mb_size_id = (core >> 1) & 0x01;
        let blk_size_id = (core >> 2) & 0x01;
        let trans_flg = (core >> 3) & 0x01 != 0;

        let table_index = (mb_size_id | (blk_size_id << 1)) as usize;
        let mb_size = MB_SIZE_TABLE[table_index];
        let blk_size = BLK_SIZE_TABLE[table_index];
        let block_log2 = if blk_size == 8 { 3 } else { 2 };

        let transform_id = if trans_flg {
            let ext_trans = r.read(2)?;
            TransformId::from_ext_trans(ext_trans)
        } else {
            TransformId::Standard
        };

        Ok(BandInfo {
            mv_halfpel,
            mb_size_id,
            blk_size_id,
            mb_size,
            blk_size,
            block_log2,
            transform_id,
        })
    }
}

/// Spec/02 §1.8 — the optional transparency colour block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transparency {
    /// The three transparency-colour component bytes (`[ebx+0x150..]`),
    /// present only when the §1.8 `color_flg` (bit 3 of the 4-bit
    /// field) was set.
    pub color: Option<[u8; 3]>,
}

/// Spec/02 §1 — the parsed GOP header (the determinable subset through
/// the band_info array + transparency block).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GopHeader {
    /// The 8-bit `gop_flags` (§1.1).
    pub flags: GopFlags,
    /// Incremented `slice_size_id` (`raw + 1`), or `None` when absent
    /// (§1.4). The slice size in pixels is `32 << slice_size_id`.
    pub slice_size_id: Option<u32>,
    /// Wavelet decomposition levels + band counts (§1.5).
    pub decomp: DecompLevels,
    /// Luma picture width (preset table or custom read; §1.6).
    pub width: u32,
    /// Luma picture height (§1.6).
    pub height: u32,
    /// Chroma plane width, derived from subsampling (§1.6).
    pub chroma_width: u32,
    /// Chroma plane height (§1.6).
    pub chroma_height: u32,
    /// Per-band `band_info` descriptors for the luma plane
    /// (`luma_bands` entries; §1.7).
    pub luma_band_info: Vec<BandInfo>,
    /// Per-band `band_info` descriptors for the chroma plane
    /// (`chroma_bands` entries; §1.7).
    pub chroma_band_info: Vec<BandInfo>,
    /// The optional transparency block (§1.8).
    pub transparency: Transparency,
}

impl GopHeader {
    /// Parse the GOP header from a bit reader positioned at bit 16
    /// (immediately after the `spec/01 §3` picture-start triplet, on an
    /// INTRA frame). The reader is left at the start of the §1.9
    /// GOP-trailing block.
    pub fn parse(r: &mut BitReader<'_>) -> Result<Self, GopError> {
        // §1.1 gop_flags.
        let flags = GopFlags {
            raw: r.read(8)? as u8,
        };

        // §1.2 gop_hdr_size (read and discard).
        if flags.hdr_size_present() {
            r.read(16)?;
        }

        // §1.3 lock_word — open question; the GOP parser does not
        // consume a read(32) for it (spec/02 §6 item 1). Nothing read.

        // §1.4 slice_size_id (2 bits, incremented by 1).
        let slice_size_id = if flags.slice_size_present() {
            Some(r.read(2)? + 1)
        } else {
            None
        };

        // §1.5 decomp_levels (3 bits) + band-count derivation.
        let decomp = DecompLevels::from_combined(r.read(3)?)?;

        // §1.6 pic_size_id (4 bits) + optional custom dimensions.
        let pic_size_id = r.read(4)?;
        let (width, height) = if pic_size_id == pic_size::PIC_SIZE_ID_CUSTOM {
            // 26-bit read: high 13 bits = height, low 13 bits = width.
            let combined = r.read(26)?;
            let height = combined >> 13;
            let width = combined & 0x1fff;
            (width, height)
        } else {
            pic_size::lookup(pic_size_id).ok_or(GopError::ZeroPictureSize { pic_size_id })?
        };

        // §1.6 chroma dimensions from subsampling.
        let subsampling = flags.subsampling();
        let chroma_width = subsampling.chroma_width(width);
        let chroma_height = subsampling.chroma_height(height);

        // §1.7 band_info array: luma plane then chroma plane.
        let mut luma_band_info = Vec::with_capacity(decomp.luma_bands as usize);
        for _ in 0..decomp.luma_bands {
            luma_band_info.push(BandInfo::parse(r)?);
        }
        let mut chroma_band_info = Vec::with_capacity(decomp.chroma_bands as usize);
        for _ in 0..decomp.chroma_bands {
            chroma_band_info.push(BandInfo::parse(r)?);
        }

        // §1.8 transparency block (conditional on gop_flags bit 3).
        let transparency = if flags.transparency_present() {
            let field = r.read(4)?;
            if field & 0x7 != 0 {
                return Err(GopError::BadTransparencyAlignment { found: field });
            }
            let color_flg = field & 0x8 != 0;
            let color = if color_flg {
                let packed = r.read(24)?;
                Some([
                    (packed & 0xff) as u8,
                    ((packed >> 8) & 0xff) as u8,
                    ((packed >> 16) & 0xff) as u8,
                ])
            } else {
                None
            };
            Transparency { color }
        } else {
            Transparency { color: None }
        };

        Ok(GopHeader {
            flags,
            slice_size_id,
            decomp,
            width,
            height,
            chroma_width,
            chroma_height,
            luma_band_info,
            chroma_band_info,
            transparency,
        })
    }

    /// Slice size in pixels per axis (`32 << slice_size_id`), or the
    /// whole picture (one slice) when `slice_size_id` is absent
    /// (`spec/02 §1.6`).
    pub fn slice_size(&self) -> Option<u32> {
        self.slice_size_id.map(|id| 32u32 << id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small bit-stream builder that packs fields LSB-first to drive
    /// the parser. Mirrors the reader's bit order.
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
        fn finish(mut self) -> Vec<u8> {
            // Pad to a multiple of 8, then ensure >= 4 bytes for the
            // reader's DWORD prefetch.
            while !self.bits.len().is_multiple_of(8) {
                self.bits.push(0);
            }
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
    fn decomp_levels_band_counts() {
        assert_eq!(
            DecompLevels::from_combined(0).unwrap(),
            DecompLevels {
                luma_levels: 0,
                chroma_levels: 0,
                luma_bands: 1,
                chroma_bands: 1
            }
        );
        assert_eq!(DecompLevels::from_combined(2).unwrap().luma_bands, 7);
        let d6 = DecompLevels::from_combined(6).unwrap();
        assert_eq!(d6.luma_bands, 7);
        assert_eq!(d6.chroma_bands, 4);
    }

    #[test]
    fn decomp_levels_rejects_forbidden() {
        assert!(matches!(
            DecompLevels::from_combined(3),
            Err(GopError::BadDecompLevels { found: 3 })
        ));
        assert!(matches!(
            DecompLevels::from_combined(5),
            Err(GopError::BadDecompLevels { found: 5 })
        ));
    }

    #[test]
    fn subsampling_chroma_dims() {
        assert_eq!(Subsampling::Yvu9.chroma_width(352), 88);
        assert_eq!(Subsampling::Yvu9.chroma_height(288), 72);
        assert_eq!(Subsampling::Yv12.chroma_width(352), 176);
        assert_eq!(Subsampling::Yv12.chroma_height(288), 144);
    }

    #[test]
    fn band_info_mb_blk_tables() {
        // index 0 (mb_size_id=0, blk_size_id=0) -> mb 16, blk 8.
        assert_eq!(MB_SIZE_TABLE[0], 0x10);
        assert_eq!(BLK_SIZE_TABLE[0], 0x08);
        // index 3 -> mb 4, blk 4.
        assert_eq!(MB_SIZE_TABLE[3], 0x04);
        assert_eq!(BLK_SIZE_TABLE[3], 0x04);
    }

    /// Build a minimal INTRA GOP header: gop_flags=0 (no extras, YVU9),
    /// decomp=0 (1 luma + 1 chroma band), pic_size_id=5 (CIF 352x288),
    /// then one luma band_info (no trans_flg) and one chroma band_info.
    fn minimal_gop() -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put(0x00, 8); // gop_flags
        w.put(0, 3); // decomp_levels = 0
        w.put(5, 4); // pic_size_id = 5 (CIF)
        w.put(0b000000, 6); // luma band_info, trans_flg clear
        w.put(0b000000, 6); // chroma band_info
        w.finish()
    }

    #[test]
    fn parse_minimal_gop() {
        let bytes = minimal_gop();
        let mut r = BitReader::new(&bytes).unwrap();
        let gop = GopHeader::parse(&mut r).unwrap();
        assert_eq!(gop.width, 352);
        assert_eq!(gop.height, 288);
        assert_eq!(gop.chroma_width, 88);
        assert_eq!(gop.chroma_height, 72);
        assert_eq!(gop.decomp.luma_bands, 1);
        assert_eq!(gop.luma_band_info.len(), 1);
        assert_eq!(gop.chroma_band_info.len(), 1);
        assert_eq!(gop.luma_band_info[0].transform_id, TransformId::Standard);
        assert!(gop.transparency.color.is_none());
        assert_eq!(gop.slice_size_id, None);
    }

    #[test]
    fn parse_gop_with_ext_transform() {
        // gop_flags=0, decomp=0, pic_size_id=5, luma band with
        // trans_flg set + ext_trans=1 (Row Slant), chroma band plain.
        let mut w = BitWriter::new();
        w.put(0x00, 8);
        w.put(0, 3);
        w.put(5, 4);
        // band core: trans_flg = bit3. core=0b001000 = 0x08.
        w.put(0b001000, 6);
        w.put(1, 2); // ext_trans = 1 -> Row Slant
        w.put(0b000000, 6); // chroma band
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let gop = GopHeader::parse(&mut r).unwrap();
        assert_eq!(gop.luma_band_info[0].transform_id, TransformId::SlantRow);
    }

    #[test]
    fn parse_gop_custom_dimensions() {
        // pic_size_id=15 -> custom: height 200, width 100.
        let mut w = BitWriter::new();
        w.put(0x00, 8); // gop_flags
        w.put(0, 3); // decomp_levels = 0
        w.put(15, 4); // pic_size_id = custom
        let combined = (200u32 << 13) | 100u32;
        w.put(combined, 26);
        w.put(0b000000, 6); // luma band
        w.put(0b000000, 6); // chroma band
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let gop = GopHeader::parse(&mut r).unwrap();
        assert_eq!(gop.width, 100);
        assert_eq!(gop.height, 200);
    }

    #[test]
    fn parse_gop_slice_size() {
        // gop_flags bit6 set -> slice_size_id present.
        let mut w = BitWriter::new();
        w.put(0x40, 8); // gop_flags: slice_size_present
        w.put(1, 2); // slice_size_id raw=1 -> stored 2
        w.put(0, 3); // decomp
        w.put(5, 4); // pic_size_id
        w.put(0b000000, 6);
        w.put(0b000000, 6);
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let gop = GopHeader::parse(&mut r).unwrap();
        assert_eq!(gop.slice_size_id, Some(2));
        assert_eq!(gop.slice_size(), Some(32 << 2));
    }

    #[test]
    fn parse_gop_transparency_with_color() {
        // gop_flags bit3 set -> transparency. field=0x8 (color_flg,
        // alignment ok), then 24-bit color.
        let mut w = BitWriter::new();
        w.put(0x08, 8); // gop_flags: transparency_present
        w.put(0, 3); // decomp
        w.put(5, 4); // pic_size_id
        w.put(0b000000, 6); // luma band
        w.put(0b000000, 6); // chroma band
        w.put(0x8, 4); // transparency field: color_flg, align ok
        w.put(0x123456, 24); // transp color
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let gop = GopHeader::parse(&mut r).unwrap();
        assert_eq!(gop.transparency.color, Some([0x56, 0x34, 0x12]));
    }

    #[test]
    fn parse_gop_transparency_bad_alignment() {
        let mut w = BitWriter::new();
        w.put(0x08, 8); // transparency_present
        w.put(0, 3);
        w.put(5, 4);
        w.put(0b000000, 6);
        w.put(0b000000, 6);
        w.put(0x3, 4); // alignment bits non-zero
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        assert!(matches!(
            GopHeader::parse(&mut r),
            Err(GopError::BadTransparencyAlignment { .. })
        ));
    }

    #[test]
    fn parse_gop_chroma_decomposed() {
        // decomp=6 -> 7 luma bands, 4 chroma bands.
        let mut w = BitWriter::new();
        w.put(0x00, 8);
        w.put(6, 3); // decomp = 6
        w.put(5, 4); // pic_size_id
        for _ in 0..7 {
            w.put(0b000000, 6);
        }
        for _ in 0..4 {
            w.put(0b000000, 6);
        }
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let gop = GopHeader::parse(&mut r).unwrap();
        assert_eq!(gop.luma_band_info.len(), 7);
        assert_eq!(gop.chroma_band_info.len(), 4);
    }
}
