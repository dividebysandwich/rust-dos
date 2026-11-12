//! Indeo 5 — the measured inverse Slant transform kernel
//! (`spec/06 §1`/`§2`, Extractor round 13).
//!
//! Spec source: `docs/video/indeo/indeo5/spec/06-slant-inverse-transform.md`
//! §1.2–§1.5 (the 8-point butterfly recurrence, its integer semantics
//! and rounding biases), §1.3 (the separable 2D composite with the
//! second pass's DC pre-bias and exact halving), §2.3 (the kernel
//! variants: 2D, the two single-axis forms, the 4-point family), §2.5
//! (the four scan-order permutation tables at `.data 0x10098528`,
//! staged in `tables/scan_tables_10098528.csv`), and
//! `tables/transform_rounding_10098500.csv` (the three rounding-bias
//! constants) / `tables/slant_basis_8.csv` (the recovered basis, used
//! here as test vectors).
//!
//! The staged recurrence was validated upstream against every one of
//! the 460 live kernel invocations of a real fixture decode
//! (`spec/06 §2.6`), so this module transcribes the §1.2 arithmetic
//! *verbatim*: signed 16-bit lanes, wraparound adds (no saturation),
//! sign-propagating shifts, round-half-up biases added before each
//! shift. A decoder must reproduce the integer recurrence, not the
//! closed-form basis (`spec/06 §1.4`).
//!
//! The 4-point kernel family is staged only as a constraint
//! (`spec/06 §2.3`: "no `>>3` stage at all — only the `(5, 2)` stage
//! and the halving"); [`inverse_slant_4`] realises that constraint
//! with the 8-point kernel's own `(5, 2)` stage shape and is marked
//! **provisional** pending a quantised-fixture verification (the
//! `spec/06 §5.4` dequant docs-gap currently blocks the byte-sum
//! oracle on the coded 4×4 chroma bands).

/// Spec/06 §1.2 — stage-1 rounding bias (`.data 0x10098500`, four
/// identical 16-bit lanes of value 4; paired shift `>> 3`).
pub const STAGE1_ROTATE_BIAS: i16 = 4;

/// Spec/06 §1.2 — stage-2 rounding bias (`.data 0x10098508`, value 2;
/// paired shift `>> 2`).
pub const STAGE2_ROTATE_BIAS: i16 = 2;

/// Spec/06 §1.3 — the second pass's DC pre-bias (`.data 0x10098510`,
/// value 1): added to the second pass's `X[0]` input so that pass's
/// trailing `>> 1` is an exact round-to-nearest halving.
pub const SECOND_PASS_DC_BIAS: i16 = 1;

#[inline]
fn m(v: i16, k: i16) -> i16 {
    // §1.5: all products are built from shifts/adds on 16-bit lanes
    // with wraparound; modulo 2^16 the composition is equivalent to a
    // wrapping multiply.
    v.wrapping_mul(k)
}

/// Spec/06 §1.2 — one 8-point inverse pass (first-pass form): eight
/// sequency-ordered coefficients in, eight spatial samples out.
///
/// Every add/subtract wraps modulo 2^16 and every shift is
/// arithmetic, with the rounding bias added before the shift
/// (`spec/06 §1.5`).
pub fn inverse_slant_8(x: &[i16; 8]) -> [i16; 8] {
    let a = m(x[1], 4)
        .wrapping_add(m(x[3], 7))
        .wrapping_add(STAGE1_ROTATE_BIAS)
        >> 3;
    let b = m(x[1], 7)
        .wrapping_sub(m(x[3], 4))
        .wrapping_add(STAGE1_ROTATE_BIAS)
        >> 3;

    let t = a.wrapping_add(x[2]);
    let u = a.wrapping_sub(x[2]);
    let p = x[0].wrapping_add(b);
    let q = x[0].wrapping_sub(b);
    let r = x[4].wrapping_add(x[5]);
    let s = x[4].wrapping_sub(x[5]);
    let v = x[7].wrapping_add(x[6]);
    let w = x[7].wrapping_sub(x[6]);

    let c = m(t, 5)
        .wrapping_add(m(w, 2))
        .wrapping_add(STAGE2_ROTATE_BIAS)
        >> 2;
    let d = m(t, 2)
        .wrapping_sub(m(w, 5))
        .wrapping_add(STAGE2_ROTATE_BIAS)
        >> 2;
    let e = m(u, 5)
        .wrapping_add(m(v, 2))
        .wrapping_add(STAGE2_ROTATE_BIAS)
        >> 2;
    let f = m(u, 2)
        .wrapping_sub(m(v, 5))
        .wrapping_add(STAGE2_ROTATE_BIAS)
        >> 2;

    let pr = p.wrapping_add(r);
    let pmr = p.wrapping_sub(r);
    let qs = q.wrapping_add(s);
    let qms = q.wrapping_sub(s);

    [
        pr.wrapping_add(c),
        pmr.wrapping_add(d),
        pmr.wrapping_sub(d),
        pr.wrapping_sub(c),
        qs.wrapping_add(e),
        qms.wrapping_add(f),
        qms.wrapping_sub(f),
        qs.wrapping_sub(e),
    ]
}

/// Spec/06 §1.3 / §2.3 — the second-pass form of the 8-point run:
/// `X[0]` is pre-incremented by [`SECOND_PASS_DC_BIAS`] and each of
/// the eight outputs is arithmetically shifted right by 1. This is
/// both the 2D composite's second pass and the whole of the
/// single-axis (1D row / 1D column) kernels.
pub fn inverse_slant_8_second_pass(x: &[i16; 8]) -> [i16; 8] {
    let mut biased = *x;
    biased[0] = biased[0].wrapping_add(SECOND_PASS_DC_BIAS);
    let out = inverse_slant_8(&biased);
    out.map(|v| v >> 1)
}

/// Spec/06 §1.3 / §2.2 — the separable 2D inverse Slant over one 8×8
/// block in raster order (`block[8*row + col]`; the input holds the
/// vertical-sequency × horizontal-sequency coefficient grid, the
/// output holds spatial samples).
///
/// Pass 1 runs the §1.2 recurrence along the vertical axis (the
/// binary's "across the eight rows" lane runs); pass 2 runs the
/// second-pass form along the horizontal axis, carrying the DC
/// pre-bias and the final halving, so the composite DC gain is ½.
pub fn inverse_slant_2d_8x8(block: &mut [i16; 64]) {
    // Pass 1 — one 8-point run per column.
    for col in 0..8 {
        let mut lane = [0i16; 8];
        for (row, l) in lane.iter_mut().enumerate() {
            *l = block[8 * row + col];
        }
        let out = inverse_slant_8(&lane);
        for (row, o) in out.iter().enumerate() {
            block[8 * row + col] = *o;
        }
    }
    // Pass 2 — one second-pass-form run per row.
    for row in 0..8 {
        let mut lane = [0i16; 8];
        lane.copy_from_slice(&block[8 * row..8 * row + 8]);
        let out = inverse_slant_8_second_pass(&lane);
        block[8 * row..8 * row + 8].copy_from_slice(&out);
    }
}

/// Spec/06 §2.3 — the single-axis row-Slant over one 8×8 block:
/// one second-pass-form run per row (the horizontal axis carries the
/// transform; the vertical axis is untouched).
pub fn inverse_slant_row_8x8(block: &mut [i16; 64]) {
    for row in 0..8 {
        let mut lane = [0i16; 8];
        lane.copy_from_slice(&block[8 * row..8 * row + 8]);
        let out = inverse_slant_8_second_pass(&lane);
        block[8 * row..8 * row + 8].copy_from_slice(&out);
    }
}

/// Spec/06 §2.3 — the single-axis column-Slant over one 8×8 block:
/// one second-pass-form run per column.
pub fn inverse_slant_col_8x8(block: &mut [i16; 64]) {
    for col in 0..8 {
        let mut lane = [0i16; 8];
        for (row, l) in lane.iter_mut().enumerate() {
            *l = block[8 * row + col];
        }
        let out = inverse_slant_8_second_pass(&lane);
        for (row, o) in out.iter().enumerate() {
            block[8 * row + col] = *o;
        }
    }
}

/// Spec/06 §2.3 — one 4-point inverse pass (first-pass form). The
/// staged constraint is that the 4-point kernels have "no `>>3` stage
/// at all — only the `(5, 2)` stage and the halving"; the recurrence
/// below realises that constraint with the same `(5, 2)` rotation
/// pair, rounding bias, and butterfly shape as the 8-point kernel's
/// second stage, giving the sequency-ordered 4-point basis
/// `[1,1,1,1]`, `[5,2,−2,−5]/4`, `[1,−1,−1,1]`, `[2,−5,5,−2]/4`.
/// **Provisional**: the flat fixture exercises only its DC column
/// (which verifies byte-sum-exactly through the frame checksum); the
/// AC shape awaits a quantised-fixture verification once the
/// `spec/06 §5.4` dequant gap closes.
pub fn inverse_slant_4(x: &[i16; 4]) -> [i16; 4] {
    let c = m(x[1], 5)
        .wrapping_add(m(x[3], 2))
        .wrapping_add(STAGE2_ROTATE_BIAS)
        >> 2;
    let d = m(x[1], 2)
        .wrapping_sub(m(x[3], 5))
        .wrapping_add(STAGE2_ROTATE_BIAS)
        >> 2;
    let p = x[0].wrapping_add(x[2]);
    let q = x[0].wrapping_sub(x[2]);
    [
        p.wrapping_add(c),
        q.wrapping_add(d),
        q.wrapping_sub(d),
        p.wrapping_sub(c),
    ]
}

/// Spec/06 §1.3 / §2.3 — the second-pass form of the 4-point run:
/// `X[0]` pre-biased by [`SECOND_PASS_DC_BIAS`], each output halved.
pub fn inverse_slant_4_second_pass(x: &[i16; 4]) -> [i16; 4] {
    let mut biased = *x;
    biased[0] = biased[0].wrapping_add(SECOND_PASS_DC_BIAS);
    let out = inverse_slant_4(&biased);
    out.map(|v| v >> 1)
}

/// The separable 2D 4-point inverse Slant over one 4×4 block in
/// raster order (`block[4*row + col]`): pass 1 down columns, pass 2
/// (second-pass form) across rows — the same composite shape as
/// [`inverse_slant_2d_8x8`].
pub fn inverse_slant_2d_4x4(block: &mut [i16; 16]) {
    for col in 0..4 {
        let mut lane = [0i16; 4];
        for (row, l) in lane.iter_mut().enumerate() {
            *l = block[4 * row + col];
        }
        let out = inverse_slant_4(&lane);
        for (row, o) in out.iter().enumerate() {
            block[4 * row + col] = *o;
        }
    }
    for row in 0..4 {
        let mut lane = [0i16; 4];
        lane.copy_from_slice(&block[4 * row..4 * row + 4]);
        let out = inverse_slant_4_second_pass(&lane);
        block[4 * row..4 * row + 4].copy_from_slice(&out);
    }
}

/// The single-axis 4-point row Slant (second-pass form per row).
pub fn inverse_slant_row_4x4(block: &mut [i16; 16]) {
    for row in 0..4 {
        let mut lane = [0i16; 4];
        lane.copy_from_slice(&block[4 * row..4 * row + 4]);
        let out = inverse_slant_4_second_pass(&lane);
        block[4 * row..4 * row + 4].copy_from_slice(&out);
    }
}

/// The single-axis 4-point column Slant (second-pass form per
/// column).
pub fn inverse_slant_col_4x4(block: &mut [i16; 16]) {
    for col in 0..4 {
        let mut lane = [0i16; 4];
        for (row, l) in lane.iter_mut().enumerate() {
            *l = block[4 * row + col];
        }
        let out = inverse_slant_4_second_pass(&lane);
        for (row, o) in out.iter().enumerate() {
            block[4 * row + col] = *o;
        }
    }
}

/// Spec/06 §2.5 — the four coefficient scan tables at
/// `.data 0x10098528` (64-byte stride array), staged in
/// `tables/scan_tables_10098528.csv`. `table[scan_position]` is the
/// raster index inside the block (`row = n >> 3`, `col = n & 7` for
/// the 8×8 tables; `n >> 2` / `n & 3` for the 4×4 table, whose first
/// 16 entries are the live ones).
pub const SCAN_ZIGZAG_8X8: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Spec/06 §2.5 — the column-major scan (`0x10098568`), used by a 1D
/// transform variant (the no-transform axis is scanned first).
pub const SCAN_COLUMN_8X8: [u8; 64] = [
    0, 8, 16, 24, 32, 40, 48, 56, 1, 9, 17, 25, 33, 41, 49, 57, 2, 10, 18, 26, 34, 42, 50, 58, 3,
    11, 19, 27, 35, 43, 51, 59, 4, 12, 20, 28, 36, 44, 52, 60, 5, 13, 21, 29, 37, 45, 53, 61, 6,
    14, 22, 30, 38, 46, 54, 62, 7, 15, 23, 31, 39, 47, 55, 63,
];

/// Spec/06 §2.5 — the row-major identity scan (`0x100985a8`), used by
/// the other 1D variant and the no-transform variant.
pub const SCAN_RASTER_8X8: [u8; 64] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49,
    50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63,
];

/// Spec/06 §2.5 — the 4×4 diagonal zig-zag (`0x100985e8`, first 16
/// entries live).
pub const SCAN_ZIGZAG_4X4: [u8; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// The 4×4 row-major identity scan (the no-transform variant needs no
/// permutation, mirroring [`SCAN_RASTER_8X8`]).
pub const SCAN_RASTER_4X4: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// Spec/06 §2.5 — place a scan-position-ordered coefficient stream
/// into an 8×8 raster block: `block[table[pos]] = coeffs[pos]`.
pub fn place_scan_8x8(coeffs: &[i16], table: &[u8; 64]) -> [i16; 64] {
    let mut block = [0i16; 64];
    for (pos, &v) in coeffs.iter().enumerate().take(64) {
        block[table[pos] as usize] = v;
    }
    block
}

/// Spec/06 §2.5 — place a scan-position-ordered coefficient stream
/// into a 4×4 raster block (`row = n >> 2`, `col = n & 3`).
pub fn place_scan_4x4(coeffs: &[i16], table: &[u8; 16]) -> [i16; 16] {
    let mut block = [0i16; 16];
    for (pos, &v) in coeffs.iter().enumerate().take(16) {
        block[table[pos] as usize] = v;
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The recovered basis (`tables/slant_basis_8.csv`): sample `i` of
    /// coefficient `k`'s contribution is `b[k][i] / denominator[k]`.
    /// Vendored as test vectors; the recurrence must reproduce each
    /// row (scaled so the rationals are exact and the rounding biases
    /// vanish relative to the magnitude).
    const BASIS_DEN: [i32; 8] = [1, 8, 4, 32, 1, 1, 4, 4];
    const BASIS: [[i32; 8]; 8] = [
        [1, 1, 1, 1, 1, 1, 1, 1],
        [12, 9, 5, 2, -2, -5, -9, -12],
        [5, 2, -2, -5, -5, -2, 2, 5],
        [19, -2, -30, -51, 51, 30, 2, -19],
        [1, -1, -1, 1, 1, -1, -1, 1],
        [1, -1, -1, 1, -1, 1, 1, -1],
        [-2, 5, -5, 2, 2, -5, 5, -2],
        [2, -5, 5, -2, 2, -5, 5, -2],
    ];

    #[test]
    fn recurrence_reproduces_recovered_basis() {
        // Feed X[k] = 32 (an exact multiple of every denominator) and
        // check x[i] = 32 * b[k][i] / den[k] exactly: at that scale
        // every intermediate rational is an integer, so the
        // round-half-up biases add nothing.
        for k in 0..8 {
            let mut x = [0i16; 8];
            x[k] = 32;
            let out = inverse_slant_8(&x);
            for i in 0..8 {
                let expect = 32 * BASIS[k][i] / BASIS_DEN[k];
                assert_eq!(
                    i32::from(out[i]),
                    expect,
                    "basis row {k} sample {i}: got {} want {expect}",
                    out[i]
                );
            }
        }
    }

    #[test]
    fn basis_rows_are_sequency_ordered() {
        // spec/06 §1.4: sign changes across the 8 samples == the
        // coefficient index; even rows symmetric, odd rows
        // antisymmetric.
        for (k, row) in BASIS.iter().enumerate() {
            let changes = row.windows(2).filter(|w| w[0] * w[1] < 0).count();
            assert_eq!(changes, k, "row {k} sign changes");
            for i in 0..4 {
                if k % 2 == 0 {
                    assert_eq!(row[i], row[7 - i], "row {k} symmetric");
                } else {
                    assert_eq!(row[i], -row[7 - i], "row {k} antisymmetric");
                }
            }
        }
    }

    #[test]
    fn near_orthogonality_and_the_1_3_exception() {
        // spec/06 §1.4: all basis pairs orthogonal except (1, 3) with
        // inner product -21/64. Work in 32nds so everything is exact.
        let scaled: Vec<[i64; 8]> = (0..8)
            .map(|k| {
                let mut r = [0i64; 8];
                for i in 0..8 {
                    r[i] = i64::from(BASIS[k][i]) * 32 / i64::from(BASIS_DEN[k]);
                }
                r
            })
            .collect();
        for a in 0..8 {
            for b in (a + 1)..8 {
                let dot: i64 = (0..8).map(|i| scaled[a][i] * scaled[b][i]).sum();
                if (a, b) == (1, 3) {
                    // -21/64 * 32 * 32 = -336.
                    assert_eq!(dot, -336);
                } else {
                    assert_eq!(dot, 0, "rows {a},{b} not orthogonal");
                }
            }
        }
    }

    #[test]
    fn dc_gain_of_2d_composite_is_half() {
        // spec/06 §1.3: X[0][0] = 2n reconstructs to constant n.
        for n in [-500i16, -112, -1, 0, 1, 63, 200] {
            let mut block = [0i16; 64];
            block[0] = n.wrapping_mul(2);
            inverse_slant_2d_8x8(&mut block);
            assert!(
                block.iter().all(|&v| v == n),
                "DC {} did not reconstruct flat {n}: {:?}",
                2 * n,
                &block[..8]
            );
        }
        // Odd DC: the exact round-to-nearest halving of the second
        // pass ((2n+1) >> 1 after the +1 pre-bias) — e.g. -223 -> -111.
        let mut block = [0i16; 64];
        block[0] = -223;
        inverse_slant_2d_8x8(&mut block);
        assert!(block.iter().all(|&v| v == -111));
    }

    #[test]
    fn second_pass_dc_bias_makes_halving_round_to_nearest() {
        // 1D second-pass form on a pure-DC lane: X0 = v emits
        // (v + 1) >> 1 everywhere.
        for v in [-224i16, -113, 0, 7, 254] {
            let mut x = [0i16; 8];
            x[0] = v;
            let out = inverse_slant_8_second_pass(&x);
            assert!(out.iter().all(|&o| o == (v.wrapping_add(1)) >> 1));
        }
    }

    #[test]
    fn separability_row_then_col_matches_2d() {
        // spec/06 §1.3: the 2D transform is exactly separable —
        // pass 1 down columns then the second-pass form across rows.
        // Cross-check the composite entry against manually chaining
        // the two public 1D forms on a scattering of blocks.
        let mut lcg = 0x2545_f491u32;
        for _ in 0..64 {
            let mut block = [0i16; 64];
            for v in block.iter_mut() {
                lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *v = (lcg >> 20) as i16 % 256;
            }
            let mut manual = block;
            for col in 0..8 {
                let mut lane = [0i16; 8];
                for (row, l) in lane.iter_mut().enumerate() {
                    *l = manual[8 * row + col];
                }
                let out = inverse_slant_8(&lane);
                for (row, o) in out.iter().enumerate() {
                    manual[8 * row + col] = *o;
                }
            }
            inverse_slant_row_8x8(&mut manual);
            inverse_slant_2d_8x8(&mut block);
            assert_eq!(manual, block);
        }
    }

    #[test]
    fn scan_tables_are_permutations() {
        for table in [&SCAN_ZIGZAG_8X8, &SCAN_COLUMN_8X8, &SCAN_RASTER_8X8] {
            let mut seen = [false; 64];
            for &n in table.iter() {
                assert!(!seen[n as usize]);
                seen[n as usize] = true;
            }
            assert!(seen.iter().all(|&s| s));
        }
        let mut seen = [false; 16];
        for &n in SCAN_ZIGZAG_4X4.iter() {
            assert!(!seen[n as usize]);
            seen[n as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn column_scan_is_transposed_raster() {
        for (pos, &n) in SCAN_COLUMN_8X8.iter().enumerate() {
            let (r, c) = (pos / 8, pos % 8);
            assert_eq!(n as usize, c * 8 + r);
        }
    }

    #[test]
    fn place_scan_zigzag_head() {
        // First five zig-zag positions land at raster 0, 1, 8, 16, 9.
        let block = place_scan_8x8(&[10, 20, 30, 40, 50], &SCAN_ZIGZAG_8X8);
        assert_eq!(block[0], 10);
        assert_eq!(block[1], 20);
        assert_eq!(block[8], 30);
        assert_eq!(block[16], 40);
        assert_eq!(block[9], 50);
        assert_eq!(block.iter().filter(|&&v| v != 0).count(), 5);
    }

    #[test]
    fn wraparound_is_modulo_2_16() {
        // spec/06 §1.5: a coefficient large enough to push 5·t past
        // 32767 wraps rather than clamping.
        let mut x = [0i16; 8];
        x[2] = 30_000; // t = A + X[2] = 30000; 5t wraps.
        let out = inverse_slant_8(&x);
        let t = 30_000i16;
        let c = t.wrapping_mul(5).wrapping_add(STAGE2_ROTATE_BIAS) >> 2;
        assert_eq!(out[0], c);
    }
}
