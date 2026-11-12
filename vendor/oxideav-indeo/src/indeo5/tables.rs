//! Indeo 5 extracted static data tables (`.data` / `.rdata` regions).
//!
//! Spec source: `docs/video/indeo/indeo5/spec/` (chapters 05/06/08) with
//! the audit-corrected value-level claims in
//! `docs/video/indeo/indeo5/audit/00-report.md` and the byte evidence in
//! `docs/video/indeo/indeo5/tables/region_*` (Extractor round 9,
//! `provenance/09-extractor-static-tables.md`; Auditor round 10,
//! `provenance/10-auditor-static-tables-crosscheck.md`).
//!
//! These are **numeric data tables** transcribed from the extracted
//! on-disk byte regions — not derived from any implementation code. Each
//! table below carries its extraction site (VMA + `region_*` artefact)
//! and the spec section whose value-level claim the audit confirmed
//! against the byte evidence.
//!
//! The tables feed the (still-gated) per-block coefficient / inverse
//! Slant path (`spec/05`/`spec/06 §2`) and the wavelet-synthesis
//! recomposition kernel (`spec/08 §3.2`); they are vendored here as the
//! documented inputs those paths will consume when the gated dispatch
//! tables are extracted, and are exercised structurally in the tests
//! below.

/// `spec/05 §4.1` (audit-corrected, `audit/00 §2.5`/§4) — the
/// per-codebook `vlcEnd` state-flag table at `.data 0x10097eb8`
/// (site 5, `tables/region_10097eb8_u32.csv` DWORDs 0..3). The u32-
/// indexed loads at `IR50_32.DLL!0x1001f465` / `0x1001f65b` read this
/// 4-entry table as the per-block state-register end-of-block seed.
pub const VLC_END: [u32; 4] = [2, 4, 8, 12];

/// `spec/08 §3.2` (audit-corrected, `audit/00 §2.6`) — the three
/// wavelet-synthesis `pmullw` constants at `.data 0x10098438` /
/// `0x10098440` / `0x10098448` (site 6, `tables/region_10098438.hex`),
/// each a four-lane i16 LE MMX qword carrying the single value below.
///
/// The audit refutes the wiki's LeGall 5/3 `{1, 2, 1}` / `{1, 2, -6, 2,
/// 1}` reading at the value level: the actual operands are `{6}`,
/// `{-7}`, `{42}` — a fixed-point scaled-and-rounded form of the
/// synthesis filter (with the adjacent `{128}` rounding-bias qword at
/// `+0x18` consistent with a `paddw` + `psraw 8` averaging tail). The
/// exact filter cannot be named from static analysis alone
/// (`spec/08 §9.2`); the values are vendored verbatim.
pub const WAVELET_SYNTH_CONSTANTS: [i16; 3] = [6, -7, 42];

/// `spec/08 §3.2` — the rounding-bias MMX qword at `.data 0x10098450`
/// (`+0x18` past the synthesis constants), a four-lane i16 `{128}` used
/// by the synthesis kernel's `paddw` + `psraw 8` averaging tail
/// (`audit/00 §2.6`).
pub const WAVELET_SYNTH_ROUND_BIAS: i16 = 128;

/// `spec/06 §5.1` (audit-corrected, `audit/00 §2.5`/§4) — the length of
/// the per-codebook dequantiser FP scale table transcribed below.
pub const DEQUANT_SCALE_LEN: usize = 60;

/// `spec/06 §5.1` (audit-corrected, `audit/00 §2.5`/§4) — the
/// per-codebook dequantiser scale table (IEEE 754 binary64) starting at
/// `.data 0x10097ed8` (site 5-wide, `tables/region_10097eb8_wide_u32.csv`
/// DWORDs 8..127), stored as the raw little-endian bit patterns for a
/// byte-exact transcription. Use [`dequant_scale`] to reinterpret an
/// entry as `f64`.
///
/// The first entry `0.38196…` is consumed by `fmull 0x10097ed8` at
/// `IR50_32.DLL!0x1002a0d7`; entries `1..49` carry varied values in
/// `[0.5, 1.7]`; entries `49..` are the default-fill value `0.99`
/// (bit pattern `0x3fefae147ae147ae`). The exact
/// `band_glob_quant`→scale index relationship rides the gated per-block
/// state-register path (`spec/06 §6` item 1).
pub const DEQUANT_SCALE_BITS: [u64; DEQUANT_SCALE_LEN] = [
    0x3fd8722191a02d60,
    0x3ffb333333333333,
    0x3fe6666666666666,
    0x3fe6666666666666,
    0x3ff0000000000000,
    0x3feccccccccccccd,
    0x3ff0000000000000,
    0x3ff199999999999a,
    0x3feccccccccccccd,
    0x3feccccccccccccd,
    0x3feccccccccccccd,
    0x3fe999999999999a,
    0x3fe999999999999a,
    0x3feccccccccccccd,
    0x3fe999999999999a,
    0x3fe999999999999a,
    0x3feccccccccccccd,
    0x3fe999999999999a,
    0x3fe999999999999a,
    0x3fe6666666666666,
    0x3fe3333333333333,
    0x3fe6666666666666,
    0x3fe6666666666666,
    0x3fe0000000000000,
    0x3fe6666666666666,
    0x3ff4cccccccccccd,
    0x3fe6666666666666,
    0x3feccccccccccccd,
    0x3feccccccccccccd,
    0x3fe999999999999a,
    0x3feccccccccccccd,
    0x3feccccccccccccd,
    0x3feccccccccccccd,
    0x3fe999999999999a,
    0x3feccccccccccccd,
    0x3ff0000000000000,
    0x3ff0000000000000,
    0x3ff0000000000000,
    0x3ff0000000000000,
    0x3feccccccccccccd,
    0x3ff0000000000000,
    0x3fe999999999999a,
    0x3ff0000000000000,
    0x3ff0000000000000,
    0x3feccccccccccccd,
    0x3feccccccccccccd,
    0x3ff0000000000000,
    0x3feccccccccccccd,
    0x3fe999999999999a,
    0x3fefae147ae147ae,
    0x3fefae147ae147ae,
    0x3fefae147ae147ae,
    0x3fefae147ae147ae,
    0x3fefae147ae147ae,
    0x3fefae147ae147ae,
    0x3fefae147ae147ae,
    0x3fefae147ae147ae,
    0x3fefae147ae147ae,
    0x3fefae147ae147ae,
    0x3fefae147ae147ae,
];

/// `spec/06 §5.1` — the default-fill dequantiser scale (`0.99`, bit
/// pattern `0x3fefae147ae147ae`) that occupies the unused table slots
/// from entry 49 onward.
pub const DEQUANT_SCALE_DEFAULT_BITS: u64 = 0x3fefae147ae147ae;

/// `spec/06 §5.1` — reinterpret a [`DEQUANT_SCALE_BITS`] entry as an
/// `f64`. Returns `None` for an out-of-range index.
#[inline]
pub fn dequant_scale(index: usize) -> Option<f64> {
    DEQUANT_SCALE_BITS
        .get(index)
        .map(|&bits| f64::from_bits(bits))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vlc_end_values() {
        // spec/05 §4.1 audit-corrected: {2, 4, 8, 12}.
        assert_eq!(VLC_END, [2, 4, 8, 12]);
    }

    #[test]
    fn wavelet_synth_constants_values() {
        // spec/08 §3.2 audit-corrected: {6, -7, 42} (region_10098438.hex
        // = 06 00 / f9 ff / 2a 00 as four-lane i16 LE).
        assert_eq!(WAVELET_SYNTH_CONSTANTS, [6, -7, 42]);
        // -7 as i16 LE is 0xfff9.
        assert_eq!((WAVELET_SYNTH_CONSTANTS[1] as u16), 0xfff9);
        assert_eq!(WAVELET_SYNTH_ROUND_BIAS, 128);
    }

    #[test]
    fn dequant_scale_len_and_bounds() {
        assert_eq!(DEQUANT_SCALE_BITS.len(), DEQUANT_SCALE_LEN);
        assert_eq!(DEQUANT_SCALE_LEN, 60);
        assert!(dequant_scale(59).is_some());
        assert!(dequant_scale(60).is_none());
    }

    #[test]
    fn dequant_scale_first_entry_is_golden_conjugate() {
        // spec/06 §5.1: first entry 0.38196… consumed by fmull at
        // 0x1002a0d7. (3 - sqrt(5)) / 2 = 0.3819660112501051.
        let v = dequant_scale(0).unwrap();
        assert!((v - 0.381_966_011_250_105_1).abs() < 1e-15, "got {v}");
    }

    #[test]
    fn dequant_scale_varied_middle_values_in_range() {
        // spec/06 §5.1: entries 1..49 in [0.5, 1.7].
        for i in 1..49 {
            let v = dequant_scale(i).unwrap();
            assert!(
                (0.5..=1.7).contains(&v),
                "entry {i} = {v} out of [0.5, 1.7]"
            );
        }
    }

    #[test]
    fn dequant_scale_default_fill_is_099() {
        // spec/06 §5.1: entries 49.. are the 0.99 default fill.
        assert_eq!(DEQUANT_SCALE_DEFAULT_BITS, 0x3fefae147ae147ae);
        for (i, &bits) in DEQUANT_SCALE_BITS.iter().enumerate().skip(49) {
            assert_eq!(bits, DEQUANT_SCALE_DEFAULT_BITS, "entry {i}");
            let v = f64::from_bits(bits);
            assert!((v - 0.99).abs() < 1e-12, "entry {i} = {v}");
        }
    }
}
