//! Indeo 5 per-tile data-size header (the wiki `value24..value27`).
//!
//! Spec source: `docs/video/indeo/indeo5/spec/03-tile-and-macroblock-layer.md`
//! §2 (parser `IR50_32.DLL!0x10025320`).
//!
//! Each coded band is subdivided into tiles (`spec/02 §4`,
//! [`super::TileGrid`]); every tile begins on a whole-byte boundary
//! within the band payload (`spec/03 §0`) and opens with a 4-stage
//! variable-length header signalling how many bytes of coded block
//! data follow:
//!
//! | Stage | Field     | Width  | Condition          |
//! | ----- | --------- | ------ | ------------------ |
//! | 1     | `value24` | 1 bit  | always             |
//! | 2     | `value25` | 1 bit  | `value24 == 0`     |
//! | 3     | `value26` | 8 bits | `value25 == 1`     |
//! | 4     | `value27` | 24 bits| `value26 == 0xFF`  |
//!
//! The four paths form a strict prefix code (`spec/03 §2.6`): empty
//! tile (1 bit), implicit size = the remainder of the band payload
//! (2 bits), explicit 8-bit byte count (10 bits, sizes `0..=254`),
//! and the 24-bit extended byte count (34 bits, up to `0xFFFFFF`).
//!
//! **Documented tension (`spec/03 §2.4` vs `§2.8`)**: §2.4 states the
//! explicit count covers "the per-tile data that follows the per-tile
//! header (not including the header bits themselves)", while the §2.8
//! reconciliation check compares the *parser's consumed bit count*
//! against `8 * tile_data_size` and then advances the byte cursor by
//! the same count — an operational reading in which the count spans
//! the whole tile. This module stores the raw field and exposes the
//! §2.8 reconciliation as [`explicit_size_matches`] without inventing
//! a third semantic; the frame driver documents which reading it
//! applies. Reported as a docs-gap for a Specifier/Auditor pass.

use super::bitreader::{BitReader, BitReaderError};
use super::header::FrameType;

/// Spec/03 §2.4 — the stage-3 sentinel selecting the 24-bit stage 4.
pub const TILE_SIZE_ESCAPE: u32 = 0xff;

/// Spec/03 §2.5 — maximum representable per-tile byte count
/// (24 bits).
pub const MAX_TILE_DATA_SIZE: u32 = 0xff_ffff;

/// Spec/03 §2.6 — the per-tile data-size field, after the 4-stage
/// prefix decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileDataSize {
    /// `value24 == 1` — the tile carries no coded block data
    /// (`spec/03 §2.2`). For an inter frame with the per-tile
    /// predictor active this triggers the MV-inheritance fast path
    /// (`spec/03 §2.7`); for an intra frame it is the plain "no coded
    /// blocks" short-circuit.
    Empty,
    /// `value25 == 0` — the per-tile data implicitly extends to the
    /// end of the band payload (`spec/03 §2.3`); the decoder is
    /// committed to parsing every block of the tile.
    Implicit,
    /// `value26 < 0xFF` or `value27` — an explicit byte count
    /// (`spec/03 §2.4`/`§2.5`), enabling the skip-to-next-tile
    /// error-recovery path without parsing the coefficient stream.
    Explicit(u32),
}

impl TileDataSize {
    /// `true` when the tile carries coded block data that a decoder
    /// must (implicit) or may (explicit) parse.
    pub fn carries_data(self) -> bool {
        !matches!(self, TileDataSize::Empty)
    }

    /// Header width in bits for this path (`spec/03 §2.6` summary
    /// table): 1 / 2 / 10 / 34.
    pub fn header_bits(self) -> u32 {
        match self {
            TileDataSize::Empty => 1,
            TileDataSize::Implicit => 2,
            TileDataSize::Explicit(n) if n < TILE_SIZE_ESCAPE => 10,
            TileDataSize::Explicit(_) => 34,
        }
    }
}

/// Spec/03 §2 — one parsed per-tile header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileHeader {
    /// The decoded 4-stage data-size field.
    pub size: TileDataSize,
    /// The `spec/03 §2.7` per-tile-skip predictor flag (`[ebp-0x24]`):
    /// whether an `Empty` tile takes the inter-tile MV-inheritance
    /// fast path (`true`) or the plain no-coded-blocks short-circuit
    /// (`false`). Always `false` on intra frames (`spec/03 §2.7`,
    /// `IR50_32.DLL!0x10025446`).
    pub predictor_active: bool,
}

impl TileHeader {
    /// Parse the 4-stage per-tile data-size header from a byte-aligned
    /// reader positioned at the tile start (`spec/03 §2.1`).
    ///
    /// `predictor_active` is the pre-computed §2.7 context flag — see
    /// [`tile_predictor_active`].
    pub fn parse(r: &mut BitReader<'_>, predictor_active: bool) -> Result<Self, BitReaderError> {
        // §2.2 stage 1 — value24.
        let value24 = r.read_bit()?;
        if value24 == 1 {
            return Ok(TileHeader {
                size: TileDataSize::Empty,
                predictor_active,
            });
        }

        // §2.3 stage 2 — value25.
        let value25 = r.read_bit()?;
        if value25 == 0 {
            return Ok(TileHeader {
                size: TileDataSize::Implicit,
                predictor_active,
            });
        }

        // §2.4 stage 3 — value26 (8 bits).
        let value26 = r.read(8)?;
        if value26 != TILE_SIZE_ESCAPE {
            return Ok(TileHeader {
                size: TileDataSize::Explicit(value26),
                predictor_active,
            });
        }

        // §2.5 stage 4 — value27 (24 bits).
        let value27 = r.read(24)?;
        Ok(TileHeader {
            size: TileDataSize::Explicit(value27),
            predictor_active,
        })
    }

    /// `true` when this header short-circuits the tile into the
    /// `spec/03 §2.7`/`§4.4` MV-inheritance fast path (empty tile with
    /// the predictor context flag set on an inter frame).
    pub fn takes_inheritance_fast_path(&self) -> bool {
        matches!(self.size, TileDataSize::Empty) && self.predictor_active
    }
}

/// Spec/03 §2.7 — the per-tile-skip predictor context flag.
///
/// The parser reads the frame-level `[frame+0xec]` byte array (one
/// byte per tile, raster order) before the size stages; the flag is
/// forced clear for INTRA frames regardless of the array content
/// (`IR50_32.DLL!0x10025446`: `mov ecx, [frame+0xb8]; test ecx, ecx;
/// jbe short_circuit` — `frame_type == 0` takes the short-circuit).
pub fn tile_predictor_active(frame_type: FrameType, ec_flag_nonzero: bool) -> bool {
    match frame_type {
        FrameType::Intra | FrameType::Null => false,
        FrameType::Inter | FrameType::DroppableInter | FrameType::DroppableInterScalability => {
            ec_flag_nonzero
        }
    }
}

/// Spec/03 §2.8 — the explicit-size reconciliation check performed
/// after the per-MB grid walk:
///
/// ```text
/// if (consumed_bits != tile_data_size_bits && tile_data_size != 0)
///     goto err_return;   // parser returns 0 (failure)
/// ```
///
/// `consumed_bits` is the parser's cumulative bit consumption for the
/// tile; the encoder-supplied byte count must reconcile exactly
/// (`tile_data_size_bits = 8 * tile_data_size`). A zero size skips
/// the comparison (the implicit-size path trusts the block-grid
/// termination instead).
pub fn explicit_size_matches(consumed_bits: u64, tile_data_size: u32) -> bool {
    tile_data_size == 0 || consumed_bits == u64::from(tile_data_size) * 8
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

    #[test]
    fn empty_tile_one_bit() {
        let mut w = BitWriter::new();
        w.put(1, 1); // value24 = 1
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let th = TileHeader::parse(&mut r, false).unwrap();
        assert_eq!(th.size, TileDataSize::Empty);
        assert_eq!(r.bits_read(), 1);
        assert_eq!(th.size.header_bits(), 1);
        assert!(!th.size.carries_data());
        assert!(!th.takes_inheritance_fast_path());
    }

    #[test]
    fn empty_tile_with_predictor_takes_fast_path() {
        let mut w = BitWriter::new();
        w.put(1, 1);
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let th = TileHeader::parse(&mut r, true).unwrap();
        assert!(th.takes_inheritance_fast_path());
    }

    #[test]
    fn implicit_size_two_bits() {
        let mut w = BitWriter::new();
        w.put(0, 1); // value24 = 0
        w.put(0, 1); // value25 = 0 -> implicit
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let th = TileHeader::parse(&mut r, false).unwrap();
        assert_eq!(th.size, TileDataSize::Implicit);
        assert_eq!(r.bits_read(), 2);
        assert_eq!(th.size.header_bits(), 2);
        assert!(th.size.carries_data());
    }

    #[test]
    fn explicit_eight_bit_size() {
        let mut w = BitWriter::new();
        w.put(0, 1); // value24
        w.put(1, 1); // value25 = 1 -> explicit
        w.put(0x7b, 8); // value26 = 123 bytes
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let th = TileHeader::parse(&mut r, false).unwrap();
        assert_eq!(th.size, TileDataSize::Explicit(123));
        assert_eq!(r.bits_read(), 10);
        assert_eq!(th.size.header_bits(), 10);
    }

    #[test]
    fn explicit_escape_to_twenty_four_bits() {
        let mut w = BitWriter::new();
        w.put(0, 1); // value24
        w.put(1, 1); // value25
        w.put(TILE_SIZE_ESCAPE, 8); // value26 = 0xFF sentinel
        w.put(0x012345, 24); // value27
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let th = TileHeader::parse(&mut r, false).unwrap();
        assert_eq!(th.size, TileDataSize::Explicit(0x012345));
        assert_eq!(r.bits_read(), 34);
        assert_eq!(th.size.header_bits(), 34);
    }

    #[test]
    fn explicit_254_stays_in_stage_three() {
        // 0xFE is the largest stage-3 literal (spec/03 §2.4).
        let mut w = BitWriter::new();
        w.put(0, 1);
        w.put(1, 1);
        w.put(0xfe, 8);
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes).unwrap();
        let th = TileHeader::parse(&mut r, false).unwrap();
        assert_eq!(th.size, TileDataSize::Explicit(0xfe));
        assert_eq!(r.bits_read(), 10);
    }

    #[test]
    fn predictor_forced_off_for_intra() {
        // spec/03 §2.7 — frame_type == 0 forces the short-circuit.
        assert!(!tile_predictor_active(FrameType::Intra, true));
        assert!(!tile_predictor_active(FrameType::Null, true));
        assert!(tile_predictor_active(FrameType::Inter, true));
        assert!(!tile_predictor_active(FrameType::Inter, false));
        assert!(tile_predictor_active(FrameType::DroppableInter, true));
        assert!(tile_predictor_active(
            FrameType::DroppableInterScalability,
            true
        ));
    }

    #[test]
    fn size_reconciliation_check() {
        // spec/03 §2.8 — exact match required for a non-zero size.
        assert!(explicit_size_matches(8 * 123, 123));
        assert!(!explicit_size_matches(8 * 123 + 1, 123));
        assert!(!explicit_size_matches(0, 123));
        // Zero size skips the comparison.
        assert!(explicit_size_matches(999, 0));
    }
}
