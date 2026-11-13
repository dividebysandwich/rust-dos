//! Inverse transforms (spec 06, sections 1 and 2).
//!
//! All arithmetic is on 16-bit values with wrapping. The code computes in
//! `i32` and cuts to 16 bits before every right shift and at the end:
//! additions, subtractions and multiplications commute with the cut, so
//! only the shifts need the 16-bit value.

/// Cut to 16 bits, two's complement.
#[inline(always)]
fn w(v: i32) -> i32 {
    v as i16 as i32
}

/// The eight-point slant transform with bias `b` (spec 06, 1.1).
#[inline(always)]
fn slant8(x: [i32; 8], b: i32) -> [i32; 8] {
    let x0 = x[0] + b;
    let t0 = w(7 * x[1] - 4 * x[3] + 4) >> 3;
    let t1 = w(4 * x[1] + 7 * x[3] + 4) >> 3;
    let e0 = x0 + t0;
    let e1 = x0 - t0;
    let p = e0 + (x[4] + x[5]);
    let q = e0 - (x[4] + x[5]);
    let s_ = e1 + (x[4] - x[5]);
    let r = e1 - (x[4] - x[5]);
    let u = t1 + x[2];
    let s = t1 - x[2];
    let ww = x[7] - x[6];
    let v = x[7] + x[6];
    let a = w(5 * u + 2 * ww + 2) >> 2;
    let bb = w(2 * u - 5 * ww + 2) >> 2;
    let c = w(5 * s + 2 * v + 2) >> 2;
    let d = w(2 * s - 5 * v + 2) >> 2;
    [p + a, q + bb, q - bb, p - a, s_ + c, r + d, r - d, s_ - c]
}

/// The four-point slant transform with bias `b` (spec 06, 1.2).
#[inline(always)]
fn slant4(x: [i32; 4], b: i32) -> [i32; 4] {
    let x0 = x[0] + b;
    let a = w(5 * x[1] + 2 * x[3] + 2) >> 2;
    let bb = w(2 * x[1] - 5 * x[3] + 2) >> 2;
    let e0 = x0 + x[2];
    let e1 = x0 - x[2];
    [e0 + a, e1 + bb, e1 - bb, e0 - a]
}

/// The final scaling of the last pass: cut to 16 bits and shift right
/// by one (spec 06, 1.3).
#[inline(always)]
fn fin(v: i32) -> i16 {
    (w(v) >> 1) as i16
}

/// Apply transform type `t` (0 to 4) to the coefficient array `c`
/// (8-wide, row-major) and write the block to `out` at `off` with row
/// pitch `pitch`.
pub(crate) fn inverse(t: u32, c: &[i16; 64], out: &mut [i16], off: usize, pitch: usize) {
    match t {
        0 => slant8x8(c, out, off, pitch),
        1 => {
            // Rows only.
            for r in 0..8 {
                let x = std::array::from_fn(|k| c[r * 8 + k] as i32);
                let y = slant8(x, 1);
                let o = &mut out[off + r * pitch..off + r * pitch + 8];
                for k in 0..8 {
                    o[k] = fin(y[k]);
                }
            }
        }
        2 => {
            // Columns only.
            for k in 0..8 {
                let x = std::array::from_fn(|r| c[r * 8 + k] as i32);
                let y = slant8(x, 1);
                for r in 0..8 {
                    out[off + r * pitch + k] = fin(y[r]);
                }
            }
        }
        3 => {
            for r in 0..8 {
                out[off + r * pitch..off + r * pitch + 8].copy_from_slice(&c[r * 8..r * 8 + 8]);
            }
        }
        _ => slant4x4(c, out, off, pitch),
    }
}

fn slant8x8(c: &[i16; 64], out: &mut [i16], off: usize, pitch: usize) {
    // Columns with b = 0, cut to 16 bits.
    let mut m = [0i32; 64];
    for k in 0..8 {
        let x = std::array::from_fn(|r| c[r * 8 + k] as i32);
        let y = slant8(x, 0);
        for r in 0..8 {
            m[r * 8 + k] = w(y[r]);
        }
    }
    // Rows with b = 1, then the final shift.
    for r in 0..8 {
        let x = std::array::from_fn(|k| m[r * 8 + k]);
        let y = slant8(x, 1);
        let o = &mut out[off + r * pitch..off + r * pitch + 8];
        for k in 0..8 {
            o[k] = fin(y[k]);
        }
    }
}

fn slant4x4(c: &[i16; 64], out: &mut [i16], off: usize, pitch: usize) {
    let mut m = [0i32; 16];
    for k in 0..4 {
        let x = std::array::from_fn(|r| c[r * 8 + k] as i32);
        let y = slant4(x, 0);
        for r in 0..4 {
            m[r * 4 + k] = w(y[r]);
        }
    }
    for r in 0..4 {
        let x = std::array::from_fn(|k| m[r * 4 + k]);
        let y = slant4(x, 1);
        let o = &mut out[off + r * pitch..off + r * pitch + 4];
        for k in 0..4 {
            o[k] = fin(y[k]);
        }
    }
}

/// Fill an `n` by `n` block with `v`.
#[inline]
pub(crate) fn fill(out: &mut [i16], off: usize, pitch: usize, n: usize, v: i16) {
    for r in 0..n {
        out[off + r * pitch..off + r * pitch + n].fill(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_only_8x8_is_constant() {
        for d in [-300i16, -1, 0, 1, 5, 200, 32767] {
            let mut c = [0i16; 64];
            c[0] = d;
            let mut out = vec![0i16; 64];
            inverse(0, &c, &mut out, 0, 8);
            let want = (d.wrapping_add(1)) >> 1;
            assert!(out.iter().all(|&v| v == want), "d = {d}");
            let mut out4 = vec![0i16; 16];
            inverse(4, &c, &mut out4, 0, 4);
            assert!(out4.iter().all(|&v| v == want), "4x4 d = {d}");
        }
    }

    #[test]
    fn row_transform_touches_only_its_row() {
        let mut c = [0i16; 64];
        c[0] = 10;
        let mut out = vec![0i16; 64];
        inverse(1, &c, &mut out, 0, 8);
        assert!(out[..8].iter().all(|&v| v == 5));
        assert!(out[8..].iter().all(|&v| v == 0));
    }

    #[test]
    fn eight_point_matches_formulas() {
        // x1 = 8: t0 = (56 + 4) >> 3 = 7, t1 = (32 + 4) >> 3 = 4;
        // u = s = 4: a = (20 + 2) >> 2 = 5, bb = (8 + 2) >> 2 = 2,
        // c = 5, d = 2.
        let y = slant8([0, 8, 0, 0, 0, 0, 0, 0], 0);
        assert_eq!(y, [12, 9, 5, 2, -2, -5, -9, -12]);
    }
}
