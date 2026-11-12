//! Indeo 5 inverse-Slant butterfly primitives (`spec/06 §1`/`§2`).
//!
//! Spec source: `docs/video/indeo/indeo5/spec/06-slant-inverse-transform.md`.
//!
//! Indeo 5's per-block inverse Slant transform is **fused** with the
//! coefficient dispatch (`spec/05 §3`): each of the 192 dispatch
//! handlers at `IR50_32.DLL!0x1001fec7..0x10020e04` carries one
//! fragment of the butterfly. The fragments reduce to three SWAR
//! (SIMD-within-a-register) building blocks over **paired 16-bit
//! coefficients** packed `(hi << 16) | (lo & 0xffff)` (`spec/06 §0`):
//!
//! 1. **Pair-load** — a 32-bit load of two adjacent coefficients.
//! 2. **Pair-shift-and-add** — a 1-bit / 2-bit / 17-bit right-rotate
//!    followed by a paired add (`spec/06 §1.2`/`§1.4`): `ror 1`
//!    halves both halves, `ror 2` quarters them, `ror 0x11` swaps the
//!    halves and shifts by 1 (the transpose-and-divide primitive for
//!    the column pass).
//! 3. **Pair-normalise** — `and reg, 0x7ffc7ffc`, clearing the
//!    rotate's LSB artefacts and confining each half to a 15-bit
//!    signed range (the integer normalisation of the Slant
//!    butterfly). The dequantisation-fused cluster uses
//!    `and reg, 0xfff8fff8` instead (`spec/06 §2.2`/`§5.1`).
//!
//! This module lands the primitives, the `spec/06 §2.1` handler
//! cluster taxonomy, the `§2.3` page-0 handler-to-slot scan table,
//! and the representative B0a / B1a fragment kernels. **Docs-gap
//! (spec/06 §6 items 2/3/7):** the complete per-handler enumeration
//! for the B0c/B1a/B1b/B1c clusters, the page-1 (column-pass)
//! handler-to-slot mapping, and the 4×4-block variants are not yet
//! staged — the end-to-end per-block transform is gated on them.

use super::gop::TransformId;

/// Spec/06 §0 — the paired saturation / range-clamp mask applied at
/// every butterfly handler tail.
pub const PAIR_NORM_MASK: u32 = 0x7ffc_7ffc;

/// Spec/06 §2.2 / §5.1 — the dequantisation-fused cluster's mask
/// (preserves the upper 13 bits of each half, clearing the bottom 3).
pub const DEQUANT_FUSED_MASK: u32 = 0xfff8_fff8;

/// Spec/05 §1.4 / spec/06 §0 — the per-block coefficient buffer: 8
/// rows × 8 columns of 16-bit signed values, 16 bytes per row.
pub const BLOCK_ROW_STRIDE: usize = 0x10;

/// Spec/06 §0 — bytes per 8×8 coefficient block.
pub const BLOCK_BYTES: usize = 128;

/// Pack two adjacent 16-bit signed coefficients into the SWAR pair
/// convention `(hi << 16) | (lo & 0xffff)` (`spec/06 §0`).
pub fn pair_pack(lo: i16, hi: i16) -> u32 {
    ((hi as u16 as u32) << 16) | (lo as u16 as u32)
}

/// Unpack a SWAR pair to `(lo, hi)`.
pub fn pair_unpack(pair: u32) -> (i16, i16) {
    ((pair & 0xffff) as u16 as i16, (pair >> 16) as u16 as i16)
}

/// Spec/06 §1.2 — the 1-bit pair-rotate (`ror reg, 1`): divides both
/// halves by 2 with the discarded LSBs landing in the positions the
/// [`PAIR_NORM_MASK`] clears.
pub fn pair_ror1(pair: u32) -> u32 {
    pair.rotate_right(1)
}

/// Spec/06 §1.4 — the 2-bit pair-rotate (`ror reg, 2`): the
/// divide-by-4 primitive of the deeper butterfly stages.
pub fn pair_ror2(pair: u32) -> u32 {
    pair.rotate_right(2)
}

/// Spec/06 §1.4 — the 17-bit pair-rotate (`ror reg, 0x11`): swaps the
/// two halves and right-shifts by 1 — the **transpose-and-divide**
/// primitive that re-aligns a column pair into row-pair orientation.
pub fn pair_ror17(pair: u32) -> u32 {
    pair.rotate_right(0x11)
}

/// Paired add (`spec/06 §1.2` step 2). The binary uses a plain 32-bit
/// `add`; the masks keep a guard bit clear in each half so the SWAR
/// sum cannot carry between halves in-spec (`spec/06 §1.4`).
pub fn pair_add(a: u32, b: u32) -> u32 {
    a.wrapping_add(b)
}

/// Spec/06 §1.2 step 3 — the pair-normalise mask.
pub fn pair_normalise(pair: u32) -> u32 {
    pair & PAIR_NORM_MASK
}

/// Spec/06 §2.1 — the per-handler rotate signature observed per
/// cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairRotate {
    /// No rotate (pure pass-through store).
    None,
    /// `ror 0x10` — the 16-bit half-store swap (A1 cluster).
    Ror16,
    /// `ror 1` — row-pass stage 0/2.
    Ror1,
    /// `ror 2` — column-pass stage 0.
    Ror2,
    /// `ror 0x11` — the transpose stages.
    Ror17,
    /// `ror 2` then `ror 0x11` combined (B1b cluster).
    Ror2Then17,
}

/// Spec/06 §2.1 — the eight observable handler clusters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerCluster {
    /// Pass-through (load and store, no transform) — the HH-band
    /// no-transform path.
    A0,
    /// 16-bit half-store (no transform, 4×4 mode).
    A1,
    /// Row-pass stage 0 (1-bit rotate).
    B0a,
    /// Row-pass stage 1 (17-bit rotate, transpose).
    B0b,
    /// Row-pass stage 2 with multi-row combine (1-bit rotate +
    /// dequantisation-fused `0xfff8fff8` mask).
    B0c,
    /// Column-pass stage 0 (2-bit rotate).
    B1a,
    /// Column-pass stage 1 (2-bit + 17-bit rotates combined).
    B1b,
    /// Column-pass tail (17-bit rotate only, 1D Slant variant).
    B1c,
}

impl HandlerCluster {
    /// The cluster's handler RVA range (`spec/06 §2.1` table).
    pub fn rva_range(self) -> (u32, u32) {
        match self {
            HandlerCluster::A0 => (0x1001_fec7, 0x1001_ff1a),
            HandlerCluster::A1 => (0x1002_009b, 0x1002_0132),
            HandlerCluster::B0a => (0x1002_0238, 0x1002_02c7),
            HandlerCluster::B0b => (0x1002_02d6, 0x1002_04fa),
            HandlerCluster::B0c => (0x1002_05ba, 0x1002_08b8),
            HandlerCluster::B1a => (0x1002_0945, 0x1002_0aa9),
            HandlerCluster::B1b => (0x1002_0abc, 0x1002_0d65),
            HandlerCluster::B1c => (0x1002_0d6b, 0x1002_0e04),
        }
    }

    /// The cluster's pair-rotate amount (`spec/06 §2.1`).
    pub fn rotate(self) -> PairRotate {
        match self {
            HandlerCluster::A0 => PairRotate::None,
            HandlerCluster::A1 => PairRotate::Ror16,
            HandlerCluster::B0a | HandlerCluster::B0c => PairRotate::Ror1,
            HandlerCluster::B0b | HandlerCluster::B1c => PairRotate::Ror17,
            HandlerCluster::B1a => PairRotate::Ror2,
            HandlerCluster::B1b => PairRotate::Ror2Then17,
        }
    }

    /// `true` for the row-pass clusters of the separable 2D Slant
    /// (`spec/06 §1.3`).
    pub fn is_row_pass(self) -> bool {
        matches!(
            self,
            HandlerCluster::B0a | HandlerCluster::B0b | HandlerCluster::B0c
        )
    }

    /// `true` for the column-pass clusters.
    pub fn is_column_pass(self) -> bool {
        matches!(
            self,
            HandlerCluster::B1a | HandlerCluster::B1b | HandlerCluster::B1c
        )
    }
}

/// Spec/06 §2.4 — which handler families a band's transform variant
/// exercises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchUse {
    /// Page-0 row-pass butterfly handlers.
    pub row_pass: bool,
    /// Page-1 column-pass butterfly handlers.
    pub column_pass: bool,
    /// The A0/A1 pure-store handlers.
    pub no_transform: bool,
}

/// Spec/06 §2.4 — map a band's transform id to the dispatch families
/// it exercises. `Standard` resolves per band frequency at
/// reconstruction time (`spec/02 §1.7`); callers resolve it before
/// asking (`None` here).
pub fn dispatch_use(transform: TransformId) -> Option<DispatchUse> {
    match transform {
        TransformId::Slant2d => Some(DispatchUse {
            row_pass: true,
            column_pass: true,
            no_transform: false,
        }),
        TransformId::SlantRow => Some(DispatchUse {
            row_pass: true,
            column_pass: false,
            no_transform: false,
        }),
        TransformId::SlantColumn => Some(DispatchUse {
            row_pass: false,
            column_pass: true,
            no_transform: false,
        }),
        TransformId::None => Some(DispatchUse {
            row_pass: false,
            column_pass: false,
            no_transform: true,
        }),
        TransformId::Standard => None,
    }
}

/// One `spec/06 §2.3` page-0 handler-to-slot entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page0Handler {
    /// Handler RVA.
    pub rva: u32,
    /// Write offset within the block buffer (`mov [edi+esi+N]`).
    pub write_offset: u8,
    /// The `(row, first-column)` of the coefficient pair the handler
    /// owns (each handler covers a 2-coefficient pair).
    pub row: u8,
    /// First column of the pair (0 or 2... columns 0/2 within the
    /// 4-pair row split; the pair covers `(row, col)`-`(row, col+1)`).
    pub col: u8,
    /// The pair-rotate the handler applies.
    pub rotate: PairRotate,
}

/// Spec/06 §2.3 — the page-0 (row-pass) handler-to-slot mapping for
/// the B0a/B0b clusters, transcribed verbatim: uniform raster-order
/// coverage of the 8-row × 4-pair block; rows 0..3 use the `ror 1`
/// row-pass schedule, rows 4..7 the `ror 0x11` column-swap schedule.
pub const PAGE0_ROW_PASS_HANDLERS: [Page0Handler; 16] = [
    Page0Handler {
        rva: 0x1002_0238,
        write_offset: 0x00,
        row: 0,
        col: 0,
        rotate: PairRotate::Ror1,
    },
    Page0Handler {
        rva: 0x1002_024b,
        write_offset: 0x04,
        row: 0,
        col: 2,
        rotate: PairRotate::Ror1,
    },
    Page0Handler {
        rva: 0x1002_025e,
        write_offset: 0x08,
        row: 1,
        col: 0,
        rotate: PairRotate::Ror1,
    },
    Page0Handler {
        rva: 0x1002_0273,
        write_offset: 0x0c,
        row: 1,
        col: 2,
        rotate: PairRotate::Ror1,
    },
    Page0Handler {
        rva: 0x1002_0286,
        write_offset: 0x10,
        row: 2,
        col: 0,
        rotate: PairRotate::Ror1,
    },
    Page0Handler {
        rva: 0x1002_029b,
        write_offset: 0x14,
        row: 2,
        col: 2,
        rotate: PairRotate::Ror1,
    },
    Page0Handler {
        rva: 0x1002_02ae,
        write_offset: 0x18,
        row: 3,
        col: 0,
        rotate: PairRotate::Ror1,
    },
    Page0Handler {
        rva: 0x1002_02c3,
        write_offset: 0x1c,
        row: 3,
        col: 2,
        rotate: PairRotate::Ror1,
    },
    Page0Handler {
        rva: 0x1002_02d6,
        write_offset: 0x20,
        row: 4,
        col: 0,
        rotate: PairRotate::Ror17,
    },
    Page0Handler {
        rva: 0x1002_02ec,
        write_offset: 0x24,
        row: 4,
        col: 2,
        rotate: PairRotate::Ror17,
    },
    Page0Handler {
        rva: 0x1002_0301,
        write_offset: 0x28,
        row: 5,
        col: 0,
        rotate: PairRotate::Ror17,
    },
    Page0Handler {
        rva: 0x1002_0318,
        write_offset: 0x2c,
        row: 5,
        col: 2,
        rotate: PairRotate::Ror17,
    },
    Page0Handler {
        rva: 0x1002_032d,
        write_offset: 0x30,
        row: 6,
        col: 0,
        rotate: PairRotate::Ror17,
    },
    Page0Handler {
        rva: 0x1002_0344,
        write_offset: 0x34,
        row: 6,
        col: 2,
        rotate: PairRotate::Ror17,
    },
    Page0Handler {
        rva: 0x1002_0359,
        write_offset: 0x38,
        row: 7,
        col: 0,
        rotate: PairRotate::Ror17,
    },
    Page0Handler {
        rva: 0x1002_0370,
        write_offset: 0x3c,
        row: 7,
        col: 2,
        rotate: PairRotate::Ror17,
    },
];

/// Spec/06 §2.2 — the representative B0a-cluster fragment
/// (`IR50_32.DLL!0x10020238` + tail `0x10020384`): load two input
/// pairs, `ror 1` each, paired sum, normalise.
pub fn b0a_fragment(input_a: u32, input_b: u32) -> u32 {
    pair_normalise(pair_add(pair_ror1(input_a), pair_ror1(input_b)))
}

/// Spec/06 §2.2 — the representative B1a-cluster (column-pass)
/// fragment (`IR50_32.DLL!0x10020abc`): pair-add each input with its
/// row-above pair, `ror 2` both sums, paired sum, normalise.
pub fn b1a_fragment(col_a: u32, col_b: u32, above_a: u32, above_b: u32) -> u32 {
    let a = pair_ror2(pair_add(col_a, above_a));
    let b = pair_ror2(pair_add(col_b, above_b));
    pair_normalise(pair_add(a, b))
}

/// Spec/06 §2.1 — the A0 pass-through fragment: the coefficient pair
/// is stored byte-for-byte (HH band / no-transform path).
pub fn a0_fragment(coeff_pair: u32) -> u32 {
    coeff_pair
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_pack_round_trip() {
        assert_eq!(pair_unpack(pair_pack(-3, 7)), (-3, 7));
        assert_eq!(pair_pack(1, 2), 0x0002_0001);
        assert_eq!(pair_pack(-1, -1), 0xffff_ffff);
        assert_eq!(pair_unpack(0x8000_7fff), (0x7fff, -0x8000));
    }

    #[test]
    fn ror1_halves_both_lanes_after_mask() {
        // For even, non-negative lane values the ror-1 + mask sequence
        // is an exact divide-by-2 in each lane (spec/06 §1.2).
        let pair = pair_pack(100, 64);
        let (lo, hi) = pair_unpack(pair_normalise(pair_ror1(pair)));
        assert_eq!((lo, hi), (48, 32)); // mask clears the 2 LSBs: 50 -> 48
        // A multiple of 8 survives exactly.
        let pair = pair_pack(80, 8);
        let (lo, hi) = pair_unpack(pair_normalise(pair_ror1(pair)));
        assert_eq!((lo, hi), (40, 4));
    }

    #[test]
    fn ror17_swaps_and_halves() {
        // spec/06 §1.4 — ror 0x11 swaps halves and shifts by 1: the
        // new low half is the old high half shifted right by 1 (for
        // in-range even values, after the mask cleanup).
        let pair = pair_pack(0, 64); // lo = 0, hi = 64
        let (lo, hi) = pair_unpack(pair_normalise(pair_ror17(pair)));
        assert_eq!((lo, hi), (32, 0)); // halves swapped, value halved
    }

    #[test]
    fn ror16_swaps_exactly() {
        let pair = pair_pack(0x1234, 0x0abc);
        let swapped = pair.rotate_right(16);
        assert_eq!(pair_unpack(swapped), (0x0abc, 0x1234));
    }

    #[test]
    fn normalise_masks_lsb_pair_and_sign_guard() {
        assert_eq!(pair_normalise(0xffff_ffff), 0x7ffc_7ffc);
        assert_eq!(PAIR_NORM_MASK, 0x7ffc_7ffc);
        assert_eq!(DEQUANT_FUSED_MASK, 0xfff8_fff8);
    }

    #[test]
    fn b0a_fragment_is_halved_sum() {
        // (a >> 1) + (b >> 1) per lane, mask-rounded.
        let a = pair_pack(64, 128);
        let b = pair_pack(32, 64);
        let (lo, hi) = pair_unpack(b0a_fragment(a, b));
        assert_eq!((lo, hi), (48, 96));
    }

    #[test]
    fn b1a_fragment_is_quarter_of_four_sum() {
        // ((col + above) >> 2) summed across the two pairs, per lane.
        let col_a = pair_pack(64, 0);
        let above_a = pair_pack(64, 0);
        let col_b = pair_pack(32, 0);
        let above_b = pair_pack(32, 0);
        let (lo, hi) = pair_unpack(b1a_fragment(col_a, col_b, above_a, above_b));
        assert_eq!((lo, hi), (48, 0)); // 128/4 + 64/4 = 32 + 16
    }

    #[test]
    fn a0_fragment_pass_through() {
        assert_eq!(a0_fragment(0xdead_beef), 0xdead_beef);
    }

    #[test]
    fn cluster_taxonomy_ranges_and_rotates() {
        // spec/06 §2.1 table.
        assert_eq!(HandlerCluster::A0.rva_range(), (0x1001_fec7, 0x1001_ff1a));
        assert_eq!(HandlerCluster::A0.rotate(), PairRotate::None);
        assert_eq!(HandlerCluster::A1.rotate(), PairRotate::Ror16);
        assert_eq!(HandlerCluster::B0a.rotate(), PairRotate::Ror1);
        assert_eq!(HandlerCluster::B0b.rotate(), PairRotate::Ror17);
        assert_eq!(HandlerCluster::B0c.rotate(), PairRotate::Ror1);
        assert_eq!(HandlerCluster::B1a.rotate(), PairRotate::Ror2);
        assert_eq!(HandlerCluster::B1b.rotate(), PairRotate::Ror2Then17);
        assert_eq!(HandlerCluster::B1c.rotate(), PairRotate::Ror17);
        assert!(HandlerCluster::B0a.is_row_pass());
        assert!(!HandlerCluster::B0a.is_column_pass());
        assert!(HandlerCluster::B1b.is_column_pass());
        assert!(!HandlerCluster::A0.is_row_pass());
    }

    #[test]
    fn page0_table_raster_coverage() {
        // spec/06 §2.3 — write offsets ascend by 4; rows 0..3 use
        // ror 1, rows 4..7 use ror 0x11; each row carries two pairs.
        for (i, h) in PAGE0_ROW_PASS_HANDLERS.iter().enumerate() {
            assert_eq!(h.write_offset as usize, i * 4);
            assert_eq!(h.row as usize, i / 2);
            assert_eq!(h.col, if i % 2 == 0 { 0 } else { 2 });
            let expected = if h.row < 4 {
                PairRotate::Ror1
            } else {
                PairRotate::Ror17
            };
            assert_eq!(h.rotate, expected);
        }
        assert_eq!(PAGE0_ROW_PASS_HANDLERS[0].rva, 0x1002_0238);
        assert_eq!(PAGE0_ROW_PASS_HANDLERS[15].rva, 0x1002_0370);
    }

    #[test]
    fn dispatch_use_per_transform() {
        // spec/06 §2.4.
        let d = dispatch_use(TransformId::Slant2d).unwrap();
        assert!(d.row_pass && d.column_pass && !d.no_transform);
        let d = dispatch_use(TransformId::SlantRow).unwrap();
        assert!(d.row_pass && !d.column_pass);
        let d = dispatch_use(TransformId::SlantColumn).unwrap();
        assert!(!d.row_pass && d.column_pass);
        let d = dispatch_use(TransformId::None).unwrap();
        assert!(d.no_transform && !d.row_pass && !d.column_pass);
        assert!(dispatch_use(TransformId::Standard).is_none());
    }

    #[test]
    fn block_layout_constants() {
        assert_eq!(BLOCK_ROW_STRIDE, 0x10);
        assert_eq!(BLOCK_BYTES, 128);
    }
}
