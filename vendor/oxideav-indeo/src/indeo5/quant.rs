//! Indeo 5 quantisation reversal by table (`spec/05 §2.3`, `spec/06
//! §5`; Extractor round 15 + Validator round 16).
//!
//! Spec source: `docs/video/indeo/indeo5/spec/05-coefficient-decode.md`
//! §2.3 (the measured reconstruction read: 1 524 / 1 524 coefficient
//! reads of the 320×240 fixture obey one rule), `spec/06 §5.1`/`§5.2`/
//! `§5.4` (the `band_glob_quant` → matrix-bank → per-step
//! reconstruction-table chain and where `mb_qdelta` joins), and the
//! staged numeric tables `tables/quant_base_1008cf00` (12 base
//! matrices), `tables/quant_scale_1008d200` (288 per-level scales),
//! `tables/quant_matrices_1007b000` (the regenerated runtime bank, used
//! here as the cross-check oracle) and `tables/recon_lut_formula` (the
//! per-step reconstruction values).
//!
//! The quantiser **never multiplies a coefficient**. It selects a
//! step `b` per block position and the decoded level index `lidx`
//! addresses a reconstruction table for that step:
//!
//! ```text
//! b       = bank[g][c][q_mb][scan[pos]]
//! bank[g][c][q][n] = clamp((base[g][c][n] · scale[g][c][q]) >> 8, 1, 255)
//! k       = (lidx + 1) >> 1                       (|level|)
//! r(k, b) = k·b + ⌊b/2⌋ − (b & 1, if b > 1)
//! coef    = +r(k, b) for odd lidx, −r(k, b) for even lidx; lidx = 0 = EOB
//! ```
//!
//! where `g` is the matrix group (groups 0..4 are the 8×8 sets, group
//! 5 the 4×4 set), `c` the class (only the second class, `c = 1`, was
//! observed on the fixtures — `spec/06 §5.2`), `q_mb = band_glob_quant
//! + mb_qdelta` the effective per-MB quantiser (24 levels; what 24..31
//! selects "is not established", so the lookup saturates at 23), and
//! `scan[pos]` the raster slot of the scan position.
//!
//! The crate's rv-table decode already carries each symbol as a
//! signed level `val` (`spec/05 §2.2` composite decode, r388/r451):
//! `lidx = 2·|val| − (val > 0)`, i.e. `k = |val|` and the sign is
//! `val`'s sign — [`dequant_level`] takes that signed level directly.

include!("quant_data.rs");

/// Quantiser levels per `(group, class)` matrix set (`spec/06 §5.1`).
pub const QUANT_LEVELS: usize = 24;

/// Matrix groups (`spec/06 §5.1`: groups 0..4 are 8×8 sets, group 5
/// the 4×4 set).
pub const QUANT_GROUPS: usize = 6;

/// The 4×4 matrix group.
pub const GROUP_4X4: usize = 5;

/// Class of the reconstruction-table pointer set the fixtures were
/// measured to use (`spec/06 §5.2`: every one of the 900 macroblock
/// fetches went through the second-class `+0x60` set; the meaning of
/// the class bit itself "is not established").
pub const OBSERVED_CLASS: usize = 1;

/// `tables/quant_matrices_1007b000` — regenerate matrix `(g, c, q)` of
/// the runtime `.sdata 0x1007b000` bank from the two on-disk tables
/// through the `ICOpen`-time fill arithmetic:
///
/// `bank[n] = clamp((base[g][c][n] · scale[g][c][q]) >> 8, 1, 255)`
///
/// For the 4×4 group the fill writes 16 entries and zeroes bytes
/// 16..63, then the spread pass (`0x10001290`) copies byte `i` to byte
/// `i + (i & !3)` for `i = 15` down to `4` (every matrix whose byte 63
/// is zero), leaving the 4×4 matrix on 8-byte rows with bytes 4..7 and
/// 12..15 keeping their pre-spread values. The result is the CSV's
/// final form.
pub fn quant_matrix(group: usize, class: usize, quant: usize) -> [u8; 64] {
    let base = &QUANT_BASE[group][class];
    let scale = QUANT_SCALE[group][class][quant];
    let mut m = [0u8; 64];
    let live = if group == GROUP_4X4 { 16 } else { 64 };
    for n in 0..live {
        let v = (u32::from(base[n]) * scale) >> 8;
        m[n] = v.clamp(1, 255) as u8;
    }
    if m[63] == 0 {
        for i in (4..=15).rev() {
            m[i + (i & !3)] = m[i];
        }
    }
    m
}

/// `spec/06 §5.1` — the matrix group a band's blocks quantise with,
/// chosen by the band's position in its plane: the 4×4 block-size
/// variant is group 5; the single band of a plane without wavelet
/// decomposition is group 0; in a plane split into wavelet bands,
/// band `n` quantises with group `n + 1` (group 1 is the low-low
/// band's 2D set, group 2 the next band's, which the row Slant
/// transforms, group 3 the column-Slant band's and group 4 the flat
/// set of the band without a transform).
pub fn quant_group(blk_size: u32, band_idx: usize, plane_bands: usize) -> usize {
    if blk_size == 4 {
        GROUP_4X4
    } else if plane_bands <= 1 {
        0
    } else {
        (band_idx + 1).min(GROUP_4X4 - 1)
    }
}

/// `tables/recon_lut_formula` — the reconstruction value of magnitude
/// index `k` (1-based) at step `b`: `k·b + ⌊b/2⌋ − odd`, with `odd =
/// b & 1` when `b > 1` and `0` when `b = 1`.
#[inline]
pub fn recon_value(k: u32, b: u8) -> i32 {
    let b32 = i32::from(b);
    let odd = if b > 1 { b32 & 1 } else { 0 };
    k as i32 * b32 + (b32 >> 1) - odd
}

/// `spec/05 §2.3` — dequantise one signed level at step `b`: the
/// placed coefficient is `±r(|level|, b)` with `level`'s sign,
/// truncated to 16 bits (the kernel stores the low 16 bits of the
/// table dword). A zero level stays zero (no coefficient).
#[inline]
pub fn dequant_level(level: i16, b: u8) -> i16 {
    if level == 0 {
        return 0;
    }
    let k = level.unsigned_abs() as u32;
    let r = recon_value(k, b);
    let v = if level < 0 { -r } else { r };
    v as i16
}

/// One band's resolved step-matrix set: the `(group, class)` pair plus
/// a per-quantiser cache of the 64-entry step matrices, so each block
/// resolves its `q_mb` matrix once.
#[derive(Debug, Clone)]
pub struct BandQuant {
    group: usize,
    class: usize,
    cache: [Option<[u8; 64]>; QUANT_LEVELS],
}

impl BandQuant {
    /// Build the set for band `band_idx` of a plane of `plane_bands`
    /// bands, coded in `blk_size` blocks.
    pub fn new(blk_size: u32, band_idx: usize, plane_bands: usize) -> Self {
        BandQuant {
            group: quant_group(blk_size, band_idx, plane_bands),
            class: OBSERVED_CLASS,
            cache: [None; QUANT_LEVELS],
        }
    }

    /// The matrix group in use.
    pub fn group(&self) -> usize {
        self.group
    }

    /// The step matrix for effective per-MB quantiser `q_mb`
    /// (saturated to the 24-level range).
    pub fn steps(&mut self, q_mb: u8) -> &[u8; 64] {
        let q = (q_mb as usize).min(QUANT_LEVELS - 1);
        self.cache[q].get_or_insert_with(|| quant_matrix(self.group, self.class, q))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recon_formula_head_of_table() {
        // tables/recon_lut_formula: step 1 -> k; step 2 -> 3, 5, 7.
        for k in 1..=8 {
            assert_eq!(recon_value(k, 1), k as i32);
        }
        assert_eq!(recon_value(1, 2), 3);
        assert_eq!(recon_value(2, 2), 5);
        assert_eq!(recon_value(3, 2), 7);
        // odd b > 1: k*b + b/2 - 1.
        assert_eq!(recon_value(1, 3), 3);
        assert_eq!(recon_value(2, 5), 11);
    }

    #[test]
    fn dequant_sign_and_zero() {
        assert_eq!(dequant_level(0, 9), 0);
        assert_eq!(dequant_level(1, 1), 1);
        assert_eq!(dequant_level(-1, 1), -1);
        assert_eq!(dequant_level(-224, 1), -224);
        assert_eq!(dequant_level(3, 4), 14);
        assert_eq!(dequant_level(-3, 4), -14);
    }

    #[test]
    fn matrix_head_matches_staged_bank() {
        // tables/quant_matrices_1007b000.csv rows (0,0,0), (0,0,9) and
        // (0,1,9) — spot values.
        let m = quant_matrix(0, 0, 0);
        assert_eq!(&m[..8], &[1, 1, 1, 1, 1, 1, 1, 1]);
        assert_eq!(m[15], 2);
        let m = quant_matrix(0, 0, 9);
        assert_eq!(&m[..8], &[2, 3, 3, 3, 4, 4, 4, 5]);
        assert_eq!(m[63], 6);
        let m = quant_matrix(0, 1, 9);
        assert_eq!(&m[..8], &[1, 2, 2, 3, 3, 4, 4, 4]);
        assert_eq!(m[63], 13);
    }

    #[test]
    fn four_by_four_group_spreads_to_eight_byte_rows() {
        let m = quant_matrix(GROUP_4X4, 0, 0);
        // Rows on 8-byte strides; bytes 4..7 keep the pre-spread row 1.
        assert_eq!(&m[8..12], &m[4..8]);
        assert_eq!(&m[28..32], &[0, 0, 0, 0]);
        assert_eq!(&m[32..], &[0u8; 32]);
    }

    #[test]
    fn group_selection() {
        assert_eq!(quant_group(4, 0, 1), GROUP_4X4);
        assert_eq!(quant_group(8, 0, 1), 0);
        // A plane of four wavelet bands: the low-low band takes the
        // second 2D set, the others follow in band order.
        assert_eq!(quant_group(8, 0, 4), 1);
        assert_eq!(quant_group(8, 1, 4), 2);
        assert_eq!(quant_group(8, 2, 4), 3);
        assert_eq!(quant_group(8, 3, 4), 4);
    }

    #[test]
    fn band_quant_cache_saturates_at_23() {
        let mut bq = BandQuant::new(8, 0, 1);
        let a = *bq.steps(23);
        let b = *bq.steps(31);
        assert_eq!(a, b);
        assert_eq!(bq.group(), 0);
    }
}
