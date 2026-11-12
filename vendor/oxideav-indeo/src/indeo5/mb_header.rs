//! Indeo 5 per-macroblock header (`spec/03 §4`).
//!
//! Spec source: `docs/video/indeo/indeo5/spec/03-tile-and-macroblock-layer.md`
//! §4 plus the `spec/04 §3.4` symbol-to-signed-value mapping.
//!
//! Each macroblock of a coded tile is prefixed by a small
//! variable-width header. **Field order (fixture-arbitrated, r388):**
//! the CBP precedes the qdelta VLC — the wiki "Block header" annex
//! order (`value31`/`value32` before `value33`), the only order under
//! which the staged `IV50` fixtures decode to byte-exact band
//! exhaustion. `spec/03 §4.5`'s qdelta-first summary table does not
//! decode the fixtures and is reported as an erratum.
//!
//! | Field         | Width  | Condition                                |
//! | ------------- | ------ | ---------------------------------------- |
//! | `mb_coded`    | 1 bit  | always (`0` = coded, `1` = **skipped**)  |
//! | `cbp`         | 4 bits | coded AND `blocks_per_mb == 4`           |
//! | `block_coded` | 1 bit  | coded AND `blocks_per_mb == 1`           |
//! | `mb_qdelta`   | VLC    | `qdelta_present` AND coded               |
//! | `mv_x_delta`  | VLC    | inter tile AND coded                     |
//! | `mv_y_delta`  | VLC    | inter tile AND coded                     |
//!
//! The qdelta / MV VLCs decode through the **MB-Huffman** codebook
//! (the frame-level `mb_huff_desc` selection — the Indeo 4 wiki
//! annex D describes Table A as coding "quant delta and motion vector
//! delta signals"), and the decoded symbol folds to a signed value via
//! the shared level zig-zag table read as an offset around `+0x80`
//! (`spec/04 §3.4`, [`super::build_level_table`]): symbol `0 → 0`,
//! `1 → +1`, `2 → -1`, `3 → +2`, …

use super::bitreader::{BitReader, BitReaderError};
use super::codebook::{Codebook, CodebookError};
use super::level_table::{LEVEL_TABLE_LEN, level_value};

/// Spec/03 §5 / spec/06 §5.2 — the effective per-MB quantiser valid
/// range bound (`0..=31`).
pub const MAX_QUANT: u8 = 31;

/// Errors raised while parsing a per-MB header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MbHeaderError {
    /// A VLC field failed to decode against the codebook.
    Vlc(CodebookError),
    /// Underlying bit-reader fault.
    BitReader(BitReaderError),
    /// A decoded VLC symbol exceeded the 256-entry level zig-zag
    /// table's index space (`spec/04 §3.4`).
    SymbolOutOfRange {
        /// The decoded symbol.
        symbol: u32,
    },
}

impl From<CodebookError> for MbHeaderError {
    fn from(e: CodebookError) -> Self {
        MbHeaderError::Vlc(e)
    }
}

impl From<BitReaderError> for MbHeaderError {
    fn from(e: BitReaderError) -> Self {
        MbHeaderError::BitReader(e)
    }
}

impl core::fmt::Display for MbHeaderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MbHeaderError::Vlc(e) => write!(f, "indeo5 mb header: vlc: {e}"),
            MbHeaderError::BitReader(e) => write!(f, "indeo5 mb header: {e}"),
            MbHeaderError::SymbolOutOfRange { symbol } => write!(
                f,
                "indeo5 mb header: vlc symbol {symbol} exceeds the level table (spec/04 §3.4)"
            ),
        }
    }
}

impl std::error::Error for MbHeaderError {}

/// Spec/03 §4.2 — the three per-MB-qdelta modes selected by the band
/// record's `[band+0x2c]` (`qdelta_present`, `band_flags` bit 2) and
/// `[band+0x30]` (`qdelta_inherit`, bit 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QdeltaMode {
    /// `[band+0x2c] == 0` — no qdelta; the MB inherits the band's
    /// `band_glob_quant` directly.
    Absent,
    /// `[band+0x2c] == 1 && [band+0x30] == 0` — an explicit per-MB
    /// qdelta VLC is read.
    Explicit,
    /// `[band+0x30] == 1` — the per-MB qdelta is inherited from the
    /// parent band's per-MB-qdelta map (no bits read here; the
    /// inheritance-source walk is the `spec/03 §4.2`-deferred path).
    Inherit,
}

impl QdeltaMode {
    /// Derive the mode from the two band flags (`spec/03 §4.2`).
    pub fn from_band_flags(qdelta_present: bool, qdelta_inherit: bool) -> Self {
        if !qdelta_present {
            QdeltaMode::Absent
        } else if qdelta_inherit {
            QdeltaMode::Inherit
        } else {
            QdeltaMode::Explicit
        }
    }
}

/// Spec/03 §4.3 — the coded-block-pattern of one coded macroblock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cbp {
    /// Case B — four-block MB: one bit per block, LSB-first in block
    /// raster order (bit 0 = block 0 top-left … bit 3 = block 3
    /// bottom-right). A `1` means the block is coded (AC coefficients
    /// follow); a `0` means DC-only.
    FourBlock(u8),
    /// Case A — single-block MB: the 1-bit `block_coded` flag.
    /// **Fixture-arbitrated sense (r388):** bit `1` = the block is
    /// coded (AC follows), bit `0` = no AC data — the *opposite* of
    /// the `spec/03 §4.3` case-A pseudocode (which does not decode the
    /// staged fixtures' chroma bands; reported as an erratum).
    SingleBlock {
        /// `true` when the single block carries no AC residual.
        dc_only: bool,
    },
}

impl Cbp {
    /// `true` when block `block_idx` carries AC coefficient data.
    pub fn block_coded(&self, block_idx: u32) -> bool {
        match self {
            Cbp::FourBlock(bits) => bits & (1 << block_idx) != 0,
            Cbp::SingleBlock { dc_only } => !dc_only,
        }
    }

    /// Number of coded (AC-carrying) blocks.
    pub fn coded_blocks(&self, blocks_per_mb: u32) -> u32 {
        (0..blocks_per_mb).filter(|&b| self.block_coded(b)).count() as u32
    }
}

/// The per-tile context the per-MB header parse needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MbContext {
    /// The band's qdelta mode (`spec/03 §4.2`).
    pub qdelta_mode: QdeltaMode,
    /// `true` for an inter tile carrying explicit MVs (`frame_type !=
    /// 0` AND the per-band MV-inheritance flag clear, `spec/03 §4.4`).
    pub explicit_mv: bool,
    /// Blocks per macroblock (1 or 4, `spec/03 §3.1`).
    pub blocks_per_mb: u32,
    /// `true` for the first band of the luma plane in a frame whose
    /// flags set bit 3: every MB then codes a qdelta, a skipped MB and
    /// one without a coded block too, whatever the band's qdelta mode.
    /// Otherwise an `Explicit` band codes it only for an MB with a
    /// coded block, and a skipped MB codes none.
    pub qdelta_always: bool,
}

/// One parsed per-MB header (`spec/03 §4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MbHeader {
    /// `true` when the 1-bit `mb_coded` flag was `1` — the MB is
    /// skipped and no further bits were consumed (`spec/03 §4.1`).
    pub skipped: bool,
    /// The decoded signed per-MB quantiser delta (`Explicit` mode
    /// only).
    pub qdelta: Option<i8>,
    /// The decoded signed MV delta pair `(mv_x_delta, mv_y_delta)`
    /// (explicit-MV inter tiles only).
    pub mv_delta: Option<(i8, i8)>,
    /// The coded-block pattern (`None` for a skipped MB).
    pub cbp: Option<Cbp>,
}

/// Decode one VLC and fold the symbol to a signed value via the level
/// zig-zag table read as an offset around `+0x80` (`spec/04 §3.4`):
/// symbol `0 → 0`, `1 → +1`, `2 → -1`, `3 → +2`, …
fn decode_signed_vlc(
    r: &mut BitReader<'_>,
    codebook: &Codebook,
    level_table: &[i8; LEVEL_TABLE_LEN],
) -> Result<i8, MbHeaderError> {
    let symbol = codebook.decode(r)?;
    if symbol as usize >= LEVEL_TABLE_LEN {
        return Err(MbHeaderError::SymbolOutOfRange { symbol });
    }
    // The table byte, re-centred: unsigned byte minus 0x80.
    let raw = level_value(level_table, symbol as u8) as u8;
    Ok(raw.wrapping_sub(0x80) as i8)
}

impl MbHeader {
    /// Parse one per-MB header in the fixture-arbitrated field order
    /// (skip, CBP, qdelta, MV pair — see the module docs).
    ///
    /// `codebook` is the **MB-Huffman** codebook (the frame-level
    /// `mb_huff_desc` selection, `spec/02 §2.6`) shared by the qdelta
    /// and MV-delta VLCs; `level_table` is the shared zig-zag fold
    /// (`spec/04 §3.4`, [`super::build_level_table`]).
    pub fn parse(
        r: &mut BitReader<'_>,
        ctx: &MbContext,
        codebook: &Codebook,
        level_table: &[i8; LEVEL_TABLE_LEN],
    ) -> Result<Self, MbHeaderError> {
        // §4.1 — the 1-bit MB-coded flag (1 = skipped).
        if r.read_bit()? == 1 {
            let qdelta = if ctx.qdelta_always {
                Some(decode_signed_vlc(r, codebook, level_table)?)
            } else {
                None
            };
            return Ok(MbHeader {
                skipped: true,
                qdelta,
                mv_delta: None,
                cbp: None,
            });
        }

        // §4.3 — CBP: 4-bit field (case B) or 1-bit flag (case A).
        let cbp = if ctx.blocks_per_mb == 4 {
            Some(Cbp::FourBlock(r.read(4)? as u8))
        } else {
            // r388 fixture-arbitrated sense: 1 = coded, 0 = no AC.
            Some(Cbp::SingleBlock {
                dc_only: r.read_bit()? == 0,
            })
        };

        // §4.2 — conditional per-MB quantiser delta (after the CBP),
        // present for an MB with a coded block, or for every MB under
        // `qdelta_always`.
        let coded = cbp.is_some_and(|c| c.coded_blocks(ctx.blocks_per_mb) > 0);
        let qdelta = match ctx.qdelta_mode {
            QdeltaMode::Explicit if coded || ctx.qdelta_always => {
                Some(decode_signed_vlc(r, codebook, level_table)?)
            }
            _ => None,
        };

        // §4.4 — conditional MV delta pair (x then y).
        let mv_delta = if ctx.explicit_mv {
            let dx = decode_signed_vlc(r, codebook, level_table)?;
            let dy = decode_signed_vlc(r, codebook, level_table)?;
            Some((dx, dy))
        } else {
            None
        };

        Ok(MbHeader {
            skipped: false,
            qdelta,
            mv_delta,
            cbp,
        })
    }
}

/// Spec/06 §5.2 — the effective per-MB quantiser:
/// `band_glob_quant + mb_qdelta`, clamped to the `0..=31` valid range.
pub fn effective_mb_quant(band_glob_quant: u8, qdelta: i8) -> u8 {
    (i32::from(band_glob_quant) + i32::from(qdelta)).clamp(0, i32::from(MAX_QUANT)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo5::level_table::build_level_table;

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
        /// Emit a codeword MSB-first (stream order).
        fn put_codeword(&mut self, code: u32, len: u8) {
            for i in (0..len).rev() {
                self.bits.push(((code >> i) & 1) as u8);
            }
        }
        fn finish(mut self) -> Vec<u8> {
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

    /// A tiny single-row codebook: four symbols, each two extra bits
    /// (`xx`, no prefix).
    fn flat_codebook() -> Codebook {
        Codebook::build(&[2]).unwrap()
    }

    fn intra_ctx(blocks_per_mb: u32, qdelta_mode: QdeltaMode) -> MbContext {
        MbContext {
            qdelta_mode,
            explicit_mv: false,
            blocks_per_mb,
            qdelta_always: false,
        }
    }

    #[test]
    fn qdelta_mode_derivation() {
        assert_eq!(
            QdeltaMode::from_band_flags(false, false),
            QdeltaMode::Absent
        );
        assert_eq!(
            QdeltaMode::from_band_flags(true, false),
            QdeltaMode::Explicit
        );
        assert_eq!(QdeltaMode::from_band_flags(true, true), QdeltaMode::Inherit);
        // qdelta_inherit without qdelta_present: no qdelta at all.
        assert_eq!(QdeltaMode::from_band_flags(false, true), QdeltaMode::Absent);
    }

    #[test]
    fn skipped_mb_consumes_one_bit() {
        let mut w = BitWriter::new();
        w.put(1, 1); // mb_coded = 1 -> skipped
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let table = build_level_table();
        let cb = flat_codebook();
        let ctx = intra_ctx(4, QdeltaMode::Explicit);
        let mb = MbHeader::parse(&mut r, &ctx, &cb, &table).unwrap();
        assert!(mb.skipped);
        assert_eq!(mb.cbp, None);
        assert_eq!(r.bits_read(), 1);
    }

    #[test]
    fn coded_mb_four_block_cbp() {
        let mut w = BitWriter::new();
        w.put(0, 1); // coded
        w.put(0b1010, 4); // CBP: blocks 1 and 3 coded
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let table = build_level_table();
        let cb = flat_codebook();
        let ctx = intra_ctx(4, QdeltaMode::Absent);
        let mb = MbHeader::parse(&mut r, &ctx, &cb, &table).unwrap();
        assert!(!mb.skipped);
        assert_eq!(mb.qdelta, None);
        assert_eq!(mb.mv_delta, None);
        let cbp = mb.cbp.unwrap();
        assert!(!cbp.block_coded(0));
        assert!(cbp.block_coded(1));
        assert!(!cbp.block_coded(2));
        assert!(cbp.block_coded(3));
        assert_eq!(cbp.coded_blocks(4), 2);
        assert_eq!(r.bits_read(), 5);
    }

    #[test]
    fn coded_mb_single_block_flag() {
        // Case A (r388 sense): bit 1 = coded, 0 = DC-only.
        let mut w = BitWriter::new();
        w.put(0, 1); // coded
        w.put(0, 1); // block_coded = 0 -> DC-only
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let table = build_level_table();
        let cb = flat_codebook();
        let ctx = intra_ctx(1, QdeltaMode::Absent);
        let mb = MbHeader::parse(&mut r, &ctx, &cb, &table).unwrap();
        let cbp = mb.cbp.unwrap();
        assert_eq!(cbp, Cbp::SingleBlock { dc_only: true });
        assert!(!cbp.block_coded(0));
        assert_eq!(cbp.coded_blocks(1), 0);
    }

    #[test]
    fn explicit_qdelta_after_cbp_zig_zag_fold() {
        // Field order: CBP first, then the qdelta VLC. Symbol 3 folds
        // to +2 under the recentred zig-zag (0, +1, -1, +2, ...).
        let table = build_level_table();
        let cb = flat_codebook();
        let cw = cb.codeword(3).unwrap();
        let mut w = BitWriter::new();
        w.put(0, 1); // coded
        w.put(0b0001, 4); // CBP
        w.put_codeword(cw.code, cw.length); // qdelta VLC
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let ctx = intra_ctx(4, QdeltaMode::Explicit);
        let mb = MbHeader::parse(&mut r, &ctx, &cb, &table).unwrap();
        assert_eq!(mb.qdelta, Some(2));
    }

    #[test]
    fn zig_zag_fold_recentres_around_0x80() {
        // spec/04 §3.4 mapping via the +0x80 recentre: 0 -> 0,
        // 1 -> +1, 2 -> -1, 3 -> +2, 4 -> -2.
        let table = build_level_table();
        let cb = flat_codebook();
        let expect = [0i8, 1, -1, 2];
        for (sym, &want) in expect.iter().enumerate() {
            let cw = cb.codeword(sym as u32).unwrap();
            let mut w = BitWriter::new();
            w.put(0, 1);
            w.put(0b1000, 4);
            w.put_codeword(cw.code, cw.length);
            let bytes = w.finish();
            let mut r = BitReader::new(&bytes).unwrap();
            let ctx = intra_ctx(4, QdeltaMode::Explicit);
            let mb = MbHeader::parse(&mut r, &ctx, &cb, &table).unwrap();
            assert_eq!(mb.qdelta, Some(want), "symbol {sym}");
        }
    }

    #[test]
    fn explicit_qdelta_needs_a_coded_block() {
        // An MB without a coded block codes no qdelta, unless every MB
        // of the band does (the first luma band under frame flag bit 3),
        // a skipped one too.
        let table = build_level_table();
        let cb = flat_codebook();
        let cw = cb.codeword(3).unwrap();
        let mut w = BitWriter::new();
        w.put(0, 1); // coded
        w.put(0b0000, 4); // CBP
        w.put(1, 1); // the next MB: skipped
        w.put_codeword(cw.code, cw.length);
        let bytes = w.finish();

        let mut r = BitReader::new(&bytes).unwrap();
        let ctx = intra_ctx(4, QdeltaMode::Explicit);
        let mb = MbHeader::parse(&mut r, &ctx, &cb, &table).unwrap();
        assert_eq!(mb.qdelta, None);
        assert_eq!(r.bits_read(), 5);
        let mb = MbHeader::parse(&mut r, &ctx, &cb, &table).unwrap();
        assert!(mb.skipped);
        assert_eq!(mb.qdelta, None);

        let mut r = BitReader::new(&bytes).unwrap();
        let ctx = MbContext {
            qdelta_always: true,
            ..intra_ctx(4, QdeltaMode::Absent)
        };
        let mb = MbHeader::parse(&mut r, &ctx, &cb, &table).unwrap();
        // Absent mode: the coded MB still reads none ...
        assert_eq!(mb.qdelta, None);
        // ... but a skipped MB of that band does.
        let mb = MbHeader::parse(&mut r, &ctx, &cb, &table).unwrap();
        assert!(mb.skipped);
        assert_eq!(mb.qdelta, Some(2));
    }

    #[test]
    fn inherit_mode_reads_no_qdelta_bits() {
        let mut w = BitWriter::new();
        w.put(0, 1); // coded
        w.put(0b1111, 4); // CBP directly (no qdelta VLC)
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let table = build_level_table();
        let cb = flat_codebook();
        let ctx = intra_ctx(4, QdeltaMode::Inherit);
        let mb = MbHeader::parse(&mut r, &ctx, &cb, &table).unwrap();
        assert_eq!(mb.qdelta, None);
        assert_eq!(mb.cbp, Some(Cbp::FourBlock(0b1111)));
        assert_eq!(r.bits_read(), 5);
    }

    #[test]
    fn inter_mv_delta_pair_order() {
        // Field order: CBP, then mv_x, mv_y.
        let table = build_level_table();
        let cb = flat_codebook();
        let cw1 = cb.codeword(1).unwrap();
        let cw3 = cb.codeword(3).unwrap();
        let mut w = BitWriter::new();
        w.put(0, 1); // coded
        w.put(0b0101, 4); // CBP
        w.put_codeword(cw1.code, cw1.length); // mv_x symbol 1 -> +1
        w.put_codeword(cw3.code, cw3.length); // mv_y symbol 3 -> +2
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let ctx = MbContext {
            qdelta_mode: QdeltaMode::Absent,
            explicit_mv: true,
            blocks_per_mb: 4,
            qdelta_always: false,
        };
        let mb = MbHeader::parse(&mut r, &ctx, &cb, &table).unwrap();
        assert_eq!(mb.mv_delta, Some((1, 2)));
        assert_eq!(mb.cbp, Some(Cbp::FourBlock(0b0101)));
    }

    #[test]
    fn effective_quant_clamps() {
        // spec/06 §5.2 — band_glob_quant + qdelta clamped to 0..=31.
        assert_eq!(effective_mb_quant(16, 4), 20);
        assert_eq!(effective_mb_quant(16, -4), 12);
        assert_eq!(effective_mb_quant(30, 5), 31);
        assert_eq!(effective_mb_quant(2, -5), 0);
        assert_eq!(effective_mb_quant(31, 0), 31);
    }
}
