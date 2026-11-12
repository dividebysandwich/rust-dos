//! Indeo 5 motion-compensated coefficient fetcher kernels.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/07-motion-compensation.md`
//! §4.4 (per-band reference buffers), §5.2 (the per-MV-mode kernels),
//! §5.3 (interpolation filter), §5.5 (residual-add semantics), §5.6
//! (the no-MC transform-id gate).
//!
//! Indeo 5's inter prediction operates on the **band-coefficient
//! layer**, not on pixels (`spec/07 §4.4`): the MC fetcher reads signed
//! 16-bit coefficients from the reference band buffer at the
//! MV-displaced position and **adds** them into the current band
//! buffer's same-position coefficients (`spec/07 §5.5`):
//!
//! ```text
//! destination[i] += interpolated_reference[i + MV]
//! ```
//!
//! The final pixel is then `wavelet_synth(decoded_coeff + predicted
//! coeff)` — the recomposition (`spec/06 §3`) bridges the augmented
//! coefficient buffer to the plane output.
//!
//! Four kernels cover the `(half_pel_x, half_pel_y)` cases (`spec/07
//! §5.2`), selected by [`McMode`]:
//!
//! * **full-pel** — direct add, no interpolation;
//! * **half-pel X** — average with the next column, `(a + b) >> 1`;
//! * **half-pel Y** — average with the next row;
//! * **2D half-pel** — average of the four surrounding samples,
//!   `(a + b + c + d) >> 2`.
//!
//! The filter is a plain two-tap unweighted arithmetic average
//! (`spec/07 §5.3` — the packed `paddw` + `psraw 1` / `psraw 2`
//! sequence, no multi-tap filter, no rounding bias); the coefficient
//! adds wrap in the signed-16-bit lanes (packed `paddw`). Boundary
//! handling is implicit (`spec/07 §5.4`): the caller supplies buffers
//! padded per `spec/06 §4.2` so displaced reads stay in-bounds — this
//! module bounds-checks and reports a violation instead of reading
//! past the slice.

use crate::indeo5::mv::McMode;

/// `spec/07 §5.6` — the transform-id flag mask the fetcher tests to
/// decide whether an MB participates in inter prediction (`test eax,
/// 0xc; je skip`): if bits 2 and 3 are both clear the MB is "no-MC"
/// (fully intra within an inter band) and the destination is left
/// unchanged.
pub const MC_TRANSFORM_FLAG_MASK: u32 = 0xc;

/// `spec/07 §5.6` — whether an MB's transform-id enables the
/// prediction-add step.
#[inline]
pub fn mb_uses_mc(transform_id: u32) -> bool {
    transform_id & MC_TRANSFORM_FLAG_MASK != 0
}

/// Errors the MC block fetch can raise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McError {
    /// The MV-displaced source rectangle (including the +1 column/row
    /// the half-pel kernels read) falls outside the reference buffer.
    /// In the binary this cannot happen for an in-range encoder MV
    /// against the `spec/06 §4.2` padded buffer (`spec/07 §5.4`); a
    /// clean-room decoder surfaces it as an error instead of reading
    /// out of bounds.
    SourceOutOfBounds,
    /// The destination rectangle falls outside the destination buffer.
    DestOutOfBounds,
}

impl core::fmt::Display for McError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            McError::SourceOutOfBounds => f.write_str(
                "indeo5 mc: MV-displaced source rectangle outside the reference band buffer (spec/07 §5.4 padding contract)",
            ),
            McError::DestOutOfBounds => f.write_str(
                "indeo5 mc: destination rectangle outside the current band buffer (spec/07 §5.5)",
            ),
        }
    }
}

impl std::error::Error for McError {}

/// A signed-16-bit band-coefficient buffer view (`spec/07 §4.4`):
/// row-major `stride × height` samples, `width <= stride`.
#[derive(Debug, Clone, Copy)]
pub struct BandView<'a> {
    /// The coefficient samples (`stride * height` entries).
    pub data: &'a [i16],
    /// Row stride in samples.
    pub stride: usize,
}

/// `spec/07 §5.3` — the two-tap unweighted average `(a + b) >> 1`
/// (packed `paddw` + `psraw 1`; wrapping 16-bit lanes, no rounding
/// bias).
#[inline]
fn avg2(a: i16, b: i16) -> i16 {
    (a as i32).wrapping_add(b as i32).wrapping_shr(1) as i16
}

/// `spec/07 §5.2` — the four-sample average `(a + b + c + d) >> 2` of
/// the 2D half-pel kernel (`paddw` ×3 + `psraw 2`).
///
/// The binary's packed `paddw` chain wraps per 16-bit lane at every
/// add; this is reproduced by folding each add through `i16` before
/// the shift.
#[inline]
fn avg4(a: i16, b: i16, c: i16, d: i16) -> i16 {
    let ab = (a as i32).wrapping_add(b as i32) as i16;
    let abc = (ab as i32).wrapping_add(c as i32) as i16;
    let abcd = (abc as i32).wrapping_add(d as i32) as i16;
    (abcd as i32).wrapping_shr(2) as i16
}

/// `spec/07 §5.2`/`§5.5` — motion-compensate one block: fetch the
/// `width × height` reference rectangle at `(src_x, src_y)` (the
/// MV-displaced block origin), interpolate per `mode`, and **add** the
/// result into the destination rectangle at `(dst_x, dst_y)` (the
/// residual-add semantics; wrapping 16-bit adds, mirroring the packed
/// `paddw`).
///
/// The half-pel kernels read one extra column (`HalfPelX`), one extra
/// row (`HalfPelY`), or both (`HalfPelXY`) past the rectangle; the
/// reference view must cover that extent (`spec/07 §5.4` padding
/// contract) or [`McError::SourceOutOfBounds`] is returned.
#[allow(clippy::too_many_arguments)]
pub fn mc_add_block(
    dst: &mut [i16],
    dst_stride: usize,
    dst_x: usize,
    dst_y: usize,
    src: BandView<'_>,
    src_x: i32,
    src_y: i32,
    width: usize,
    height: usize,
    mode: McMode,
) -> Result<(), McError> {
    // Extra source extent read by the interpolating kernels.
    let (extra_x, extra_y) = match mode {
        McMode::FullPel => (0usize, 0usize),
        McMode::HalfPelX => (1, 0),
        McMode::HalfPelY => (0, 1),
        McMode::HalfPelXY => (1, 1),
    };

    // Source bounds (spec/07 §5.4 — normally guaranteed by padding).
    if src_x < 0 || src_y < 0 {
        return Err(McError::SourceOutOfBounds);
    }
    let (sx, sy) = (src_x as usize, src_y as usize);
    let src_rows = src.data.len().checked_div(src.stride).unwrap_or(0);
    if sx + width + extra_x > src.stride || sy + height + extra_y > src_rows {
        return Err(McError::SourceOutOfBounds);
    }

    // Destination bounds.
    let dst_rows = dst.len().checked_div(dst_stride).unwrap_or(0);
    if dst_x + width > dst_stride || dst_y + height > dst_rows {
        return Err(McError::DestOutOfBounds);
    }

    for row in 0..height {
        let s0 = (sy + row) * src.stride + sx;
        let s1 = s0 + src.stride; // next row (HalfPelY / XY only)
        let d0 = (dst_y + row) * dst_stride + dst_x;
        for col in 0..width {
            let pred = match mode {
                // spec/07 §5.2 full-pel: movq + paddw.
                McMode::FullPel => src.data[s0 + col],
                // half-pel X: average with the next column.
                McMode::HalfPelX => avg2(src.data[s0 + col], src.data[s0 + col + 1]),
                // half-pel Y: average with the next row.
                McMode::HalfPelY => avg2(src.data[s0 + col], src.data[s1 + col]),
                // 2D half-pel: average of four surrounding samples.
                McMode::HalfPelXY => avg4(
                    src.data[s0 + col],
                    src.data[s0 + col + 1],
                    src.data[s1 + col],
                    src.data[s1 + col + 1],
                ),
            };
            // spec/07 §5.5 residual add (wrapping, like paddw).
            dst[d0 + col] = dst[d0 + col].wrapping_add(pred);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4x4 reference with distinct values, stride 5 (one pad column +
    /// one pad row so the half-pel kernels have their +1 extent).
    fn reference() -> Vec<i16> {
        let mut v = vec![0i16; 5 * 5];
        for y in 0..5 {
            for x in 0..5 {
                v[y * 5 + x] = (10 * y + x) as i16;
            }
        }
        v
    }

    #[test]
    fn no_mc_gate() {
        // spec/07 §5.6: transform-id bits 2/3 clear -> no MC.
        assert!(!mb_uses_mc(0x0));
        assert!(!mb_uses_mc(0x3));
        assert!(mb_uses_mc(0x4));
        assert!(mb_uses_mc(0x8));
        assert!(mb_uses_mc(0xc));
    }

    #[test]
    fn full_pel_adds_reference() {
        let refdata = reference();
        let src = BandView {
            data: &refdata,
            stride: 5,
        };
        let mut dst = vec![100i16; 2 * 2];
        mc_add_block(&mut dst, 2, 0, 0, src, 1, 1, 2, 2, McMode::FullPel).unwrap();
        // ref (1,1)=11 (2,1)=12 (1,2)=21 (2,2)=22, added onto 100.
        assert_eq!(dst, vec![111, 112, 121, 122]);
    }

    #[test]
    fn half_pel_x_averages_columns() {
        let refdata = reference();
        let src = BandView {
            data: &refdata,
            stride: 5,
        };
        let mut dst = vec![0i16; 2];
        mc_add_block(&mut dst, 2, 0, 0, src, 0, 0, 2, 1, McMode::HalfPelX).unwrap();
        // (ref[0]+ref[1])>>1 = (0+1)>>1 = 0; (ref[1]+ref[2])>>1 = 1.
        assert_eq!(dst, vec![0, 1]);
    }

    #[test]
    fn half_pel_y_averages_rows() {
        let refdata = reference();
        let src = BandView {
            data: &refdata,
            stride: 5,
        };
        let mut dst = vec![0i16; 2];
        mc_add_block(&mut dst, 2, 0, 0, src, 0, 0, 2, 1, McMode::HalfPelY).unwrap();
        // (ref(0,0)+ref(0,1))>>1 = (0+10)>>1 = 5; (1+11)>>1 = 6.
        assert_eq!(dst, vec![5, 6]);
    }

    #[test]
    fn half_pel_xy_averages_four() {
        let refdata = reference();
        let src = BandView {
            data: &refdata,
            stride: 5,
        };
        let mut dst = vec![0i16; 1];
        mc_add_block(&mut dst, 1, 0, 0, src, 0, 0, 1, 1, McMode::HalfPelXY).unwrap();
        // (0 + 1 + 10 + 11) >> 2 = 22 >> 2 = 5.
        assert_eq!(dst, vec![5]);
    }

    #[test]
    fn negative_average_truncates_toward_negative_infinity() {
        // psraw is an arithmetic shift: (-1 + -2) >> 1 = -3 >> 1 = -2.
        assert_eq!(avg2(-1, -2), -2);
        // (-1 -2 -3 -4) >> 2 = -10 >> 2 = -3 (arithmetic).
        assert_eq!(avg4(-1, -2, -3, -4), -3);
    }

    #[test]
    fn adds_wrap_like_paddw() {
        // Residual add wraps per 16-bit lane (packed paddw).
        let refdata = vec![i16::MAX; 4];
        let src = BandView {
            data: &refdata,
            stride: 2,
        };
        let mut dst = vec![1i16; 1];
        mc_add_block(&mut dst, 1, 0, 0, src, 0, 0, 1, 1, McMode::FullPel).unwrap();
        assert_eq!(dst, vec![i16::MIN]); // 1 + 32767 wraps.
    }

    #[test]
    fn source_bounds_checked_per_mode() {
        let refdata = reference();
        let src = BandView {
            data: &refdata,
            stride: 5,
        };
        let mut dst = vec![0i16; 4 * 4];
        // Full-pel 4x4 at (1,1) fits in the 5x5 reference…
        assert!(mc_add_block(&mut dst, 4, 0, 0, src, 1, 1, 4, 4, McMode::FullPel).is_ok());
        // …but the XY kernel needs the +1 extent, exceeding 5x5.
        assert_eq!(
            mc_add_block(&mut dst, 4, 0, 0, src, 1, 1, 4, 4, McMode::HalfPelXY),
            Err(McError::SourceOutOfBounds)
        );
        // Negative displacement is out of bounds.
        assert_eq!(
            mc_add_block(&mut dst, 4, 0, 0, src, -1, 0, 2, 2, McMode::FullPel),
            Err(McError::SourceOutOfBounds)
        );
    }

    #[test]
    fn dest_bounds_checked() {
        let refdata = reference();
        let src = BandView {
            data: &refdata,
            stride: 5,
        };
        let mut dst = vec![0i16; 2 * 2];
        assert_eq!(
            mc_add_block(&mut dst, 2, 1, 0, src, 0, 0, 2, 2, McMode::FullPel),
            Err(McError::DestOutOfBounds)
        );
    }

    #[test]
    fn error_display_cites_spec() {
        assert!(McError::SourceOutOfBounds.to_string().contains("spec/07"));
        assert!(McError::DestOutOfBounds.to_string().contains("spec/07"));
    }
}
