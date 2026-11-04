//! Indeo 4's inverse transforms, motion compensation and Haar wavelet
//! recomposition, after FFmpeg's libavcodec/ivi_dsp.c (LGPL 2.1 or
//! later):
//!
//! Copyright (c) 2009-2011 Maxim Poliakovski
//!
//! A transform takes a block of coefficients, `n` by `n` in rows, and
//! gives the block of pixels in the same order, each truncated to 16
//! bits as the band buffers keep them. Its arithmetic wraps, as C's does
//! on the machines the codec ran on; the coefficients are kept small
//! enough by the caller that it never needs to.

/// A transform's two functions: of a block with its coefficients, and
/// of one whose only coefficient is the DC one.
#[derive(Clone, Copy)]
pub struct Transform {
    pub inverse: fn(&[i32], &[u8; 8], &mut [i16]),
    pub dc: fn(i32, usize, &mut [i16]),
    /// Whether it transforms both ways, and so intra blocks predict
    /// their DC coefficient.
    pub two_d: bool,
    pub size: usize,
}

const fn t(
    inverse: fn(&[i32], &[u8; 8], &mut [i16]),
    dc: fn(i32, usize, &mut [i16]),
    two_d: bool,
    size: usize,
) -> Option<Transform> {
    Some(Transform {
        inverse,
        dc,
        two_d,
        size,
    })
}

/// The transforms by the band header's number: None for the DCTs and
/// "no transform 4x4", which no decoder known has.
pub static TRANSFORMS: [Option<Transform>; 18] = [
    t(inverse_haar_8x8, dc_haar_2d, true, 8),
    t(row_haar8, dc_haar_2d, false, 8),
    t(col_haar8, dc_haar_2d, false, 8),
    t(put_pixels_8x8, put_dc_pixel_8x8, true, 8),
    t(inverse_slant_8x8, dc_slant_2d, true, 8),
    t(row_slant8, dc_row_slant, true, 8),
    t(col_slant8, dc_col_slant, true, 8),
    None,
    None,
    None,
    t(inverse_haar_4x4, dc_haar_2d, true, 4),
    t(inverse_slant_4x4, dc_slant_2d, true, 4),
    None,
    t(row_haar4, dc_haar_2d, false, 4),
    t(col_haar4, dc_haar_2d, false, 4),
    t(row_slant4, dc_row_slant, false, 4),
    t(col_slant4, dc_col_slant, false, 4),
    None,
];

/// The 8-point inverse Haar transform of `s`, each output through
/// `c`. The order of the inputs is FFmpeg's INV_HAAR8's.
#[inline]
fn haar8(s: [i32; 8], c: fn(i32) -> i32) -> [i32; 8] {
    let bfly = |a: i32, b: i32| (a.wrapping_add(b) >> 1, a.wrapping_sub(b) >> 1);
    let [s1, s5, s3, s7, s2, s4, s6, s8] = s;
    let (t1, t5) = bfly(s1.wrapping_mul(2), s5.wrapping_mul(2));
    let (t1, t3) = bfly(t1, s3);
    let (t5, t7) = bfly(t5, s7);
    let (t1, t2) = bfly(t1, s2);
    let (t3, t4) = bfly(t3, s4);
    let (t5, t6) = bfly(t5, s6);
    let (t7, t8) = bfly(t7, s8);
    [t1, t2, t3, t4, t5, t6, t7, t8].map(c)
}

/// The 4-point inverse Haar transform, as INV_HAAR4.
#[inline]
fn haar4(s: [i32; 4]) -> [i32; 4] {
    let bfly = |a: i32, b: i32| (a.wrapping_add(b) >> 1, a.wrapping_sub(b) >> 1);
    let [s1, s3, s5, s7] = s;
    let (t0, t1) = bfly(s1, s3);
    let (d1, d2) = bfly(t0, s5);
    let (d3, d4) = bfly(t1, s7);
    [d1, d2, d3, d4]
}

fn same(x: i32) -> i32 {
    x
}

fn halve(x: i32) -> i32 {
    x.wrapping_add(1) >> 1
}

/// The 8-point inverse slant transform, as IVI_INV_SLANT8, with its
/// inputs in the order the macro takes them.
#[inline]
fn slant8(s: [i32; 8], c: fn(i32) -> i32) -> [i32; 8] {
    let [s1, s4, s8, s5, s2, s6, s3, s7] = s;
    let bfly = |a: i32, b: i32| (a.wrapping_add(b), a.wrapping_sub(b));
    // IVI_IREFLECT: a, b = 1/2, 5/4.
    let ireflect = |a: i32, b: i32| {
        let o1 = (a.wrapping_add(b.wrapping_mul(2)).wrapping_add(2) >> 2).wrapping_add(a);
        let o2 = (a.wrapping_mul(2).wrapping_sub(b).wrapping_add(2) >> 2).wrapping_sub(b);
        (o1, o2)
    };
    // IVI_SLANT_PART4: a, b = 1/2, 7/8.
    let o1 = s5.wrapping_add(s4.wrapping_mul(4).wrapping_sub(s5).wrapping_add(4) >> 3);
    let o2 = s4.wrapping_add(
        s4.wrapping_neg()
            .wrapping_sub(s5.wrapping_mul(4))
            .wrapping_add(4)
            >> 3,
    );
    let (t4, t5) = (o1, o2);
    let (t1, t5) = bfly(s1, t5);
    let (t2, t6) = bfly(s2, s6);
    let (t7, t3) = bfly(s7, s3);
    let (t4, t8) = bfly(t4, s8);
    let (t1, t2) = bfly(t1, t2);
    let (t4, t3) = ireflect(t4, t3);
    let (t5, t6) = bfly(t5, t6);
    let (t8, t7) = ireflect(t8, t7);
    let (t1, t4) = bfly(t1, t4);
    let (t2, t3) = bfly(t2, t3);
    let (t5, t8) = bfly(t5, t8);
    let (t6, t7) = bfly(t6, t7);
    [t1, t2, t3, t4, t5, t6, t7, t8].map(c)
}

/// IVI_INV_SLANT4, with its inputs in the order the macro takes them.
#[inline]
fn slant4(s: [i32; 4], c: fn(i32) -> i32) -> [i32; 4] {
    let [s1, s4, s2, s3] = s;
    let bfly = |a: i32, b: i32| (a.wrapping_add(b), a.wrapping_sub(b));
    let (t1, t2) = bfly(s1, s2);
    let t4 = (s4.wrapping_add(s3.wrapping_mul(2)).wrapping_add(2) >> 2).wrapping_add(s4);
    let t3 = (s4.wrapping_mul(2).wrapping_sub(s3).wrapping_add(2) >> 2).wrapping_sub(s3);
    let (t1, t4) = bfly(t1, t4);
    let (t2, t3) = bfly(t2, t3);
    [t1, t2, t3, t4].map(c)
}

fn column<const N: usize>(b: &[i32], i: usize) -> [i32; N] {
    std::array::from_fn(|k| b[k * N + i])
}

fn row<const N: usize>(b: &[i32], i: usize) -> [i32; N] {
    std::array::from_fn(|k| b[i * N + k])
}

fn store_row<const N: usize>(out: &mut [i16], i: usize, v: [i32; N]) {
    for (k, x) in v.into_iter().enumerate() {
        out[i * N + k] = x as i16;
    }
}

fn store_column<const N: usize>(out: &mut [i16], i: usize, v: [i32; N]) {
    for (k, x) in v.into_iter().enumerate() {
        out[k * N + i] = x as i16;
    }
}

fn inverse_haar_8x8(input: &[i32], flags: &[u8; 8], out: &mut [i16]) {
    let mut tmp = [0i32; 64];
    for i in 0..8 {
        if flags[i] != 0 {
            // The first four inputs are scaled up in the left half.
            let shift = u32::from(i & 4 == 0);
            let mut s = column::<8>(input, i);
            for v in &mut s[..4] {
                *v = v.wrapping_shl(shift);
            }
            for (k, v) in haar8(s, same).into_iter().enumerate() {
                tmp[k * 8 + i] = v;
            }
        }
    }
    for i in 0..8 {
        let r = row::<8>(&tmp, i);
        let v = if r.iter().all(|&x| x == 0) {
            [0; 8]
        } else {
            haar8(r, same)
        };
        store_row(out, i, v);
    }
}

fn row_haar8(input: &[i32], _: &[u8; 8], out: &mut [i16]) {
    for i in 0..8 {
        let r = row::<8>(input, i);
        let v = if r.iter().all(|&x| x == 0) {
            [0; 8]
        } else {
            haar8(r, same)
        };
        store_row(out, i, v);
    }
}

fn col_haar8(input: &[i32], flags: &[u8; 8], out: &mut [i16]) {
    for (i, &flag) in flags.iter().enumerate() {
        let v = if flag != 0 {
            haar8(column::<8>(input, i), same)
        } else {
            [0; 8]
        };
        store_column(out, i, v);
    }
}

fn inverse_haar_4x4(input: &[i32], flags: &[u8; 8], out: &mut [i16]) {
    let mut tmp = [0i32; 16];
    for i in 0..4 {
        if flags[i] != 0 {
            let shift = u32::from(i & 2 == 0);
            let mut s = column::<4>(input, i);
            for v in &mut s[..2] {
                *v = v.wrapping_shl(shift);
            }
            for (k, v) in haar4(s).into_iter().enumerate() {
                tmp[k * 4 + i] = v;
            }
        }
    }
    for i in 0..4 {
        let r = row::<4>(&tmp, i);
        let v = if r.iter().all(|&x| x == 0) {
            [0; 4]
        } else {
            haar4(r)
        };
        store_row(out, i, v);
    }
}

fn row_haar4(input: &[i32], _: &[u8; 8], out: &mut [i16]) {
    for i in 0..4 {
        let r = row::<4>(input, i);
        let v = if r.iter().all(|&x| x == 0) {
            [0; 4]
        } else {
            haar4(r)
        };
        store_row(out, i, v);
    }
}

fn col_haar4(input: &[i32], flags: &[u8; 8], out: &mut [i16]) {
    for (i, &flag) in flags[..4].iter().enumerate() {
        let v = if flag != 0 {
            haar4(column::<4>(input, i))
        } else {
            [0; 4]
        };
        store_column(out, i, v);
    }
}

fn fill(out: &mut [i16], n: usize, value: i16) {
    out[..n * n].fill(value);
}

fn dc_haar_2d(dc: i32, n: usize, out: &mut [i16]) {
    fill(out, n, (dc >> 3) as i16);
}

fn put_pixels_8x8(input: &[i32], _: &[u8; 8], out: &mut [i16]) {
    for (o, &i) in out[..64].iter_mut().zip(input) {
        *o = i as i16;
    }
}

fn put_dc_pixel_8x8(dc: i32, _: usize, out: &mut [i16]) {
    out[..64].fill(0);
    out[0] = dc as i16;
}

fn inverse_slant_8x8(input: &[i32], flags: &[u8; 8], out: &mut [i16]) {
    let mut tmp = [0i32; 64];
    for i in 0..8 {
        if flags[i] != 0 {
            for (k, v) in slant8(column::<8>(input, i), same).into_iter().enumerate() {
                tmp[k * 8 + i] = v;
            }
        }
    }
    for i in 0..8 {
        let r = row::<8>(&tmp, i);
        let v = if r.iter().all(|&x| x == 0) {
            [0; 8]
        } else {
            slant8(r, halve)
        };
        store_row(out, i, v);
    }
}

fn inverse_slant_4x4(input: &[i32], flags: &[u8; 8], out: &mut [i16]) {
    let mut tmp = [0i32; 16];
    for i in 0..4 {
        if flags[i] != 0 {
            for (k, v) in slant4(column::<4>(input, i), same).into_iter().enumerate() {
                tmp[k * 4 + i] = v;
            }
        }
    }
    for i in 0..4 {
        let r = row::<4>(&tmp, i);
        let v = if r.iter().all(|&x| x == 0) {
            [0; 4]
        } else {
            slant4(r, halve)
        };
        store_row(out, i, v);
    }
}

fn dc_slant_2d(dc: i32, n: usize, out: &mut [i16]) {
    fill(out, n, (dc.wrapping_add(1) >> 1) as i16);
}

fn row_slant8(input: &[i32], _: &[u8; 8], out: &mut [i16]) {
    for i in 0..8 {
        let r = row::<8>(input, i);
        let v = if r.iter().all(|&x| x == 0) {
            [0; 8]
        } else {
            slant8(r, halve)
        };
        store_row(out, i, v);
    }
}

fn dc_row_slant(dc: i32, n: usize, out: &mut [i16]) {
    out[..n * n].fill(0);
    out[..n].fill((dc.wrapping_add(1) >> 1) as i16);
}

fn col_slant8(input: &[i32], flags: &[u8; 8], out: &mut [i16]) {
    for (i, &flag) in flags.iter().enumerate() {
        let v = if flag != 0 {
            slant8(column::<8>(input, i), halve)
        } else {
            [0; 8]
        };
        store_column(out, i, v);
    }
}

fn dc_col_slant(dc: i32, n: usize, out: &mut [i16]) {
    out[..n * n].fill(0);
    for y in 0..n {
        out[y * n] = (dc.wrapping_add(1) >> 1) as i16;
    }
}

fn row_slant4(input: &[i32], _: &[u8; 8], out: &mut [i16]) {
    for i in 0..4 {
        let r = row::<4>(input, i);
        let v = if r.iter().all(|&x| x == 0) {
            [0; 4]
        } else {
            slant4(r, halve)
        };
        store_row(out, i, v);
    }
}

fn col_slant4(input: &[i32], flags: &[u8; 8], out: &mut [i16]) {
    for (i, &flag) in flags[..4].iter().enumerate() {
        let v = if flag != 0 {
            slant4(column::<4>(input, i), halve)
        } else {
            [0; 4]
        };
        store_column(out, i, v);
    }
}

/// The prediction of an `n` by `n` block from `src` at `at`, rows
/// `pitch` apart: whole pixels (`kind` 0) or half ones, across (1),
/// down (2) or both (3). None if it would read outside `src`.
pub fn predict(src: &[i16], at: usize, pitch: usize, n: usize, kind: u32) -> Option<[i16; 64]> {
    let extra = (kind & 1) as usize + if kind > 1 { pitch } else { 0 };
    if at + (n - 1) * pitch + n + extra > src.len() {
        return None;
    }
    let mut out = [0i16; 64];
    for i in 0..n {
        let r = at + i * pitch;
        for j in 0..n {
            let a = src[r + j] as i32;
            let v = match kind {
                0 => a,
                1 => (a + src[r + j + 1] as i32) >> 1,
                2 => (a + src[r + pitch + j] as i32) >> 1,
                _ => {
                    (a + src[r + j + 1] as i32
                        + src[r + pitch + j] as i32
                        + src[r + pitch + j + 1] as i32)
                        >> 2
                }
            };
            out[i * n + j] = v as i16;
        }
    }
    Some(out)
}

/// Writes or adds (`add`) the `n` by `n` block `block` into `dst` at
/// `at`, rows `pitch` apart. False if it doesn't fit.
pub fn put(dst: &mut [i16], at: usize, pitch: usize, n: usize, block: &[i16], add: bool) -> bool {
    if at + (n - 1) * pitch + n > dst.len() {
        return false;
    }
    for i in 0..n {
        let row = &mut dst[at + i * pitch..at + i * pitch + n];
        let src = &block[i * n..i * n + n];
        if add {
            for (d, &s) in row.iter_mut().zip(src) {
                *d = d.wrapping_add(s);
            }
        } else {
            row.copy_from_slice(src);
        }
    }
    true
}

/// The average of two predictions, as ff_ivi_mc_avg's: the first, plus
/// the second (in 16 bits), halved.
pub fn average(a: &[i16; 64], b: &[i16; 64], n: usize) -> [i16; 64] {
    let mut out = [0i16; 64];
    for i in 0..n * n {
        out[i] = a[i].wrapping_add(b[i]) >> 1;
    }
    out
}

/// Recomposes a luma plane of four Haar wavelet bands (LL, HL, LH and
/// HH, `pitch` apart) into `dst`, `width` by `height` rounded up to
/// even, as pixels with their bias of 128 back.
pub fn recompose_haar(
    bands: [&[i16]; 4],
    pitch: usize,
    width: usize,
    height: usize,
    dst: &mut [u8],
    dst_pitch: usize,
) {
    for y in (0..height).step_by(2) {
        let row = y / 2 * pitch;
        for (indx, x) in (0..width).step_by(2).enumerate() {
            let b: [i32; 4] = std::array::from_fn(|i| bands[i][row + indx] as i32);
            let p = [
                (b[0] + b[1] + b[2] + b[3] + 2) >> 2,
                (b[0] + b[1] - b[2] - b[3] + 2) >> 2,
                (b[0] - b[1] + b[2] - b[3] + 2) >> 2,
                (b[0] - b[1] - b[2] + b[3] + 2) >> 2,
            ];
            let at = y * dst_pitch + x;
            dst[at] = (p[0] + 128).clamp(0, 255) as u8;
            dst[at + 1] = (p[1] + 128).clamp(0, 255) as u8;
            dst[at + dst_pitch] = (p[2] + 128).clamp(0, 255) as u8;
            dst[at + dst_pitch + 1] = (p[3] + 128).clamp(0, 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dc_coefficient_alone_fills_the_block() {
        // The 2D Haar transform of a DC of 64 (scaled up by 2 in the
        // column pass) is 8 everywhere, as dc_haar_2d's 64 >> 3.
        let mut input = [0i32; 64];
        input[0] = 64;
        let mut flags = [0u8; 8];
        flags[0] = 1;
        let mut out = [0i16; 64];
        inverse_haar_8x8(&input, &flags, &mut out);
        let mut dc = [0i16; 64];
        dc_haar_2d(64, 8, &mut dc);
        assert_eq!(out, dc);
        assert!(out.iter().all(|&v| v == 8));
    }

    #[test]
    fn slant_of_a_dc_matches_its_dc_function() {
        let mut input = [0i32; 64];
        input[0] = 20;
        let mut flags = [0u8; 8];
        flags[0] = 1;
        let mut out = [0i16; 64];
        inverse_slant_8x8(&input, &flags, &mut out);
        let mut dc = [0i16; 64];
        dc_slant_2d(20, 8, &mut dc);
        assert_eq!(out, dc);
    }

    #[test]
    fn half_pel_prediction_averages_neighbours() {
        let src: Vec<i16> = (0..100).collect();
        let p = predict(&src, 0, 10, 4, 3).unwrap();
        // (0 + 1 + 10 + 11) >> 2.
        assert_eq!(p[0], 5);
        assert!(predict(&src, 96, 10, 4, 0).is_none());
    }
}
