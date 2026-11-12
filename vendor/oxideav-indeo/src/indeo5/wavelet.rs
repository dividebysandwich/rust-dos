//! Indeo 5 wavelet recomposition.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/06-slant-inverse-transform.md`
//! §3 (synthesis filter), §4 (per-level upsampling + boundary handling).
//!
//! A plane split into wavelet bands carries one low-low band plus three
//! high-frequency bands per decomposition level, each at half the
//! level's width and height. The synthesis rebuilds every 2×2 output
//! quad from the band samples around its position with a separable
//! filter, accumulated in integers and scaled down once at the end:
//!
//! * one axis' low-pass taps put `4·L[k]` on the even output sample and
//!   `2·(L[k] + L[k+1])` on the odd one;
//! * its high-pass taps put `2·(H[k-1] + H[k])` on the even sample and
//!   `H[k-1] − 6·H[k] + H[k+1]` on the odd one.
//!
//! A band's contribution is the product of its vertical and horizontal
//! taps; the four products are summed and shifted right by 6, an
//! arithmetic shift that rounds towards minus infinity, so a flat
//! low-low band of `4·v` gives a flat plane of `v`. The bands follow
//! the stream's band order: band 1 is high-pass vertically and
//! low-pass horizontally, band 2 the reverse, band 3 high-pass on both
//! axes.
//!
//! At the edges a neighbour outside the band repeats the nearest band
//! sample: `k − 1` at the left or top edge reads `k`, `k + 1` at the
//! right or bottom edge reads `k`.
//!
//! FFmpeg's `indeo5` decoder adds `4·(H[k-1] − 6·H[k])` of band 1's
//! own column to the odd rows of its even columns, and `2·` that to
//! the odd rows of its odd columns, where the filter above has none.
//! The frame checksums the encoder stores in each frame side with the
//! filter above: over a 1 527-frame scalable movie it reproduces the
//! stored sum of 249 frames exactly and FFmpeg's of 88, and of the
//! others it comes closer on 1 198, FFmpeg on 240.

/// A band-resolution buffer: `width × height` row-major `i32` samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Band {
    /// Width in samples.
    pub width: usize,
    /// Height in samples.
    pub height: usize,
    /// Row-major samples (`width · height` entries).
    pub data: Vec<i32>,
}

impl Band {
    /// The band cropped / zero-padded to `width × height`.
    pub fn resized(&self, width: usize, height: usize) -> Band {
        let mut data = vec![0i32; width * height];
        for y in 0..height.min(self.height) {
            let n = width.min(self.width);
            data[y * width..y * width + n]
                .copy_from_slice(&self.data[y * self.width..y * self.width + n]);
        }
        Band {
            width,
            height,
            data,
        }
    }

    /// Construct a band from explicit dimensions + data (panics on a
    /// length mismatch — a programming error in the caller).
    pub fn new(width: usize, height: usize, data: Vec<i32>) -> Self {
        assert_eq!(data.len(), width * height, "band data length mismatch");
        Band {
            width,
            height,
            data,
        }
    }

    /// Sample at `(x, y)` (row-major).
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> i32 {
        self.data[y * self.width + x]
    }

    /// Sample at `(x, y)` with each coordinate held to the band: an
    /// index before the first sample or after the last reads the edge
    /// sample.
    #[inline]
    fn edge(&self, x: isize, y: isize) -> i32 {
        let x = x.clamp(0, self.width as isize - 1) as usize;
        let y = y.clamp(0, self.height as isize - 1) as usize;
        self.at(x, y)
    }
}

/// One axis' synthesis taps for output parity `odd` around band
/// sample `k`: up to three `(offset from k, weight)` pairs.
fn taps(high: bool, odd: bool) -> &'static [(isize, i32)] {
    match (high, odd) {
        (false, false) => &[(0, 4)],
        (false, true) => &[(0, 2), (1, 2)],
        (true, false) => &[(-1, 2), (0, 2)],
        (true, true) => &[(-1, 1), (0, -6), (1, 1)],
    }
}

/// The unscaled contribution of `band` to the output sample of parity
/// `(odd_x, odd_y)` above band sample `(k, j)` (column, row).
fn contribution(
    band: &Band,
    high_x: bool,
    high_y: bool,
    (k, j): (isize, isize),
    (odd_x, odd_y): (bool, bool),
) -> i32 {
    let mut sum = 0;
    for &(dy, wy) in taps(high_y, odd_y) {
        for &(dx, wx) in taps(high_x, odd_x) {
            sum += wy * wx * band.edge(k + dx, j + dy);
        }
    }
    sum
}

/// Recompose a single 2D decomposition level (`spec/06 §4.1`).
///
/// Given the low-low band and the level's three high-frequency bands
/// (each `(W/2) × (H/2)`, the same dimensions; others are fitted to the
/// low-low band), produce the `W × H` plane: every 2×2 quad above band
/// sample `(k, j)` is the shifted sum of the four bands' filter taps
/// (module docs).
pub fn recompose_level(ll: &Band, hl: &Band, lh: &Band, hh: &Band) -> Band {
    let bw = ll.width;
    let bh = ll.height;
    let fit =
        |b: &Band| -> Option<Band> { ((b.width, b.height) != (bw, bh)).then(|| b.resized(bw, bh)) };
    let (hl_f, lh_f, hh_f) = (fit(hl), fit(lh), fit(hh));
    let hl = hl_f.as_ref().unwrap_or(hl);
    let lh = lh_f.as_ref().unwrap_or(lh);
    let hh = hh_f.as_ref().unwrap_or(hh);

    let full_w = bw * 2;
    let mut out = vec![0i32; full_w * bh * 2];
    for j in 0..bh {
        for k in 0..bw {
            let at = (k as isize, j as isize);
            for (oy, odd_y) in [(0, false), (1, true)] {
                for (ox, odd_x) in [(0, false), (1, true)] {
                    let parity = (odd_x, odd_y);
                    let p = contribution(ll, false, false, at, parity)
                        + contribution(hl, false, true, at, parity)
                        + contribution(lh, true, false, at, parity)
                        + contribution(hh, true, true, at, parity);
                    out[(2 * j + oy) * full_w + 2 * k + ox] = p >> 6;
                }
            }
        }
    }
    Band::new(full_w, bh * 2, out)
}

/// The three high-frequency bands of one decomposition level, in the
/// stream's band order. The low-low band is the level's first band;
/// for the innermost level it is an explicit band, for outer levels
/// the recomposed result of the level below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelBands {
    /// Band 1: high-pass vertically, low-pass horizontally.
    pub hl: Band,
    /// Band 2: low-pass vertically, high-pass horizontally.
    pub lh: Band,
    /// Band 3: high-pass on both axes.
    pub hh: Band,
}

/// Recompose a whole plane from its wavelet bands (`spec/06 §3.4`
/// bottom-up multi-level synthesis).
///
/// * `ll` is the innermost low-low band (the only band whose `ll`
///   quadrant is carried explicitly).
/// * `levels` lists the decomposition levels from **innermost to
///   outermost** — `levels[0]` pairs with `ll` to form the inner-LL,
///   whose result then becomes the `ll` quadrant for `levels[1]`, and so
///   on. The list length equals the plane's decomposition-level count
///   (`spec/02 §1.5`: 0, 1, or 2).
///
/// For a 0-level plane (`levels` empty) the single `ll` band *is* the
/// plane (`spec/06 §3.4`: "no wavelet recomposition is performed"). Each
/// listed level doubles both plane axes (`spec/06 §4.1`). Only the
/// one-level split has a reference to check against: streams with two
/// levels are not known to exist.
pub fn recompose_plane(ll: &Band, levels: &[LevelBands]) -> Band {
    let mut current = ll.clone();
    // §3.4 bottom-up: combine the running low-low band with each level's
    // three high-frequency bands, innermost level first. A recomposed
    // low-low band is twice its input's size, which for a plane whose
    // dimensions are not multiples of `2^levels` can differ by one from
    // the next level's `ceil`-sized triple: fit it to the triple.
    for level in levels {
        if (current.width, current.height) != (level.hl.width, level.hl.height) {
            current = current.resized(level.hl.width, level.hl.height);
        }
        current = recompose_level(&current, &level.hl, &level.lh, &level.hh);
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, v: i32) -> Band {
        Band::new(w, h, vec![v; w * h])
    }

    #[test]
    fn recompose_level_flat_low_low_quarters() {
        // A flat low-low band of 4·v and zero high bands give a flat
        // plane of v: each axis' low-pass taps sum to 4, and the sum is
        // shifted down by 6.
        let zero = flat(2, 2, 0);
        let plane = recompose_level(&flat(2, 2, 160), &zero, &zero, &zero);
        assert_eq!((plane.width, plane.height), (4, 4));
        assert!(plane.data.iter().all(|&v| v == 40), "{:?}", plane.data);
    }

    #[test]
    fn recompose_level_rounds_towards_minus_infinity() {
        let zero = flat(1, 1, 0);
        // -448 · 16 >> 6 = -112 (studio black once biased).
        let plane = recompose_level(&flat(1, 1, -448), &zero, &zero, &zero);
        assert!(plane.data.iter().all(|&v| v == -112));
        // -1 · 16 = -16, >> 6 = -1, not 0.
        let plane = recompose_level(&flat(1, 1, -1), &zero, &zero, &zero);
        assert!(plane.data.iter().all(|&v| v == -1));
    }

    #[test]
    fn band_two_is_horizontal_high_pass() {
        // A single sample of band 2 (high-pass horizontally) in a 3×1
        // band: the even outputs beside it take 2·4 = 8 per tap, the odd
        // output above it -6·4 = -24, its neighbours' odd outputs 1·4.
        let zero = flat(3, 1, 0);
        let mut lh = flat(3, 1, 0);
        lh.data[1] = 64;
        let plane = recompose_level(&zero, &zero, &lh, &zero);
        // Even row: x = 0,1 | 2,3 | 4,5 above band samples 0, 1, 2.
        let row: Vec<i32> = (0..6).map(|x| plane.at(x, 0)).collect();
        assert_eq!(row, vec![0, 4, 8, -24, 8, 4]);
    }

    #[test]
    fn band_one_is_vertical_high_pass() {
        // The same sample in band 1 acts down the columns instead, alike
        // in both columns of the quad.
        let zero = flat(1, 3, 0);
        let mut hl = flat(1, 3, 0);
        hl.data[1] = 64;
        let plane = recompose_level(&zero, &hl, &zero, &zero);
        for x in 0..2 {
            let column: Vec<i32> = (0..6).map(|y| plane.at(x, y)).collect();
            assert_eq!(column, vec![0, 4, 8, -24, 8, 4], "x = {x}");
        }
    }

    #[test]
    fn edges_repeat_the_nearest_sample() {
        // A flat high-pass band gives every odd output the same value
        // (1 - 6 + 1 taps), the edge ones too: the repeated edge sample
        // stands in for the missing neighbour.
        let zero = flat(4, 1, 0);
        let plane = recompose_level(&zero, &zero, &flat(4, 1, 16), &zero);
        for x in (1..8).step_by(2) {
            assert_eq!(plane.at(x, 0), (-4 * 16 * 4) >> 6, "x = {x}");
        }
    }

    #[test]
    fn recompose_level_dimensions() {
        let b = flat(3, 2, 0);
        let plane = recompose_level(&b, &b, &b, &b);
        assert_eq!((plane.width, plane.height), (6, 4));
        assert_eq!(plane.data.len(), 24);
    }

    #[test]
    fn band_accessor() {
        let b = Band::new(2, 2, vec![1, 2, 3, 4]);
        assert_eq!(b.at(0, 0), 1);
        assert_eq!(b.at(1, 0), 2);
        assert_eq!(b.at(0, 1), 3);
        assert_eq!(b.at(1, 1), 4);
    }

    #[test]
    fn recompose_plane_zero_levels_is_identity() {
        // A 0-level plane is just its single LL band.
        let ll = Band::new(3, 2, vec![1, 2, 3, 4, 5, 6]);
        let plane = recompose_plane(&ll, &[]);
        assert_eq!(plane, ll);
    }

    #[test]
    fn recompose_plane_one_level_matches_recompose_level() {
        let ll = flat(2, 2, 160);
        let zero = flat(2, 2, 0);
        let level = LevelBands {
            hl: zero.clone(),
            lh: zero.clone(),
            hh: zero.clone(),
        };
        let plane = recompose_plane(&ll, &[level]);
        let direct = recompose_level(&ll, &zero, &zero, &zero);
        assert_eq!(plane, direct);
        assert_eq!((plane.width, plane.height), (4, 4));
    }

    #[test]
    fn recompose_plane_two_levels_doubles_twice() {
        // Each level takes its low-low band at 4 times the scale of its
        // output, so an innermost flat band of 16·v comes out as a flat
        // plane of v, 4 times its size on each axis.
        let ll = flat(2, 2, 32);
        let level = |n: usize| LevelBands {
            hl: flat(n, n, 0),
            lh: flat(n, n, 0),
            hh: flat(n, n, 0),
        };
        let plane = recompose_plane(&ll, &[level(2), level(4)]);
        assert_eq!((plane.width, plane.height), (8, 8));
        assert!(plane.data.iter().all(|&v| v == 2), "{:?}", plane.data);
    }
}
