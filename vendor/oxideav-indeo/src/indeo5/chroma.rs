//! Indeo 5 output-stage chroma subsampling and upsampling.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/08-output-reconstruction.md`
//! §3.5 (chroma upsampling) and §5 (chroma subsampling ratio +
//! positioning).
//!
//! Indeo 5's chroma planes are carried subsampled relative to luma. Two
//! ratios exist (`spec/08 §5.1`), selected by the GOP `gop_flags` bit 1:
//!
//! * **4:1:0** (`YVU9`-equivalent, the dominant `chroma_levels = 0`
//!   mode) — each chroma plane is `ceil(luma_w / 4) × ceil(luma_h / 4)`
//!   samples.
//! * **4:2:0** (`YV12`/`I420` mode) — each chroma plane is
//!   `ceil(luma_w / 2) × ceil(luma_h / 2)` samples.
//!
//! At output time each chroma sample is replicated to a
//! `scale × scale` block of luma-resolution positions (`spec/08 §3.5`).
//! The upsampling is a plain **box-filter replication** — no
//! interpolation, no centre-vs-cosited correction (`spec/08 §5.2`: the
//! chroma sample sits at the **top-left** of its luma block):
//!
//! ```text
//! chroma_index = (luma_y >> shift) * chroma_stride + (luma_x >> shift)
//! ```
//!
//! with `shift = 2` for 4:1:0 and `shift = 1` for 4:2:0. This module
//! computes the subsampled chroma dimensions (`spec/08 §5.1`) and the
//! box-filter upsample to luma resolution (`spec/08 §3.5`/`§5.2`).

use crate::indeo5::output::OutputPlane;

/// `spec/08 §5.1` — the chroma subsampling ratio selected by the GOP
/// `gop_flags` bit 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaSubsampling {
    /// 4:1:0 (`YVU9`-equivalent): chroma is `ceil(luma / 4)` per axis.
    /// The dominant Indeo 5 mode (`chroma_levels = 0`, `gop_flags`
    /// bit 1 = 0).
    Yvu9,
    /// 4:2:0 (`YV12`/`I420`): chroma is `ceil(luma / 2)` per axis
    /// (`gop_flags` bit 1 = 1).
    Yv12,
}

impl ChromaSubsampling {
    /// `spec/08 §5.1` — the per-axis luma→chroma downscale shift
    /// (`2` for 4:1:0, `1` for 4:2:0). A luma coordinate maps to its
    /// chroma coordinate via `luma >> shift`.
    #[inline]
    pub fn shift(self) -> u32 {
        match self {
            ChromaSubsampling::Yvu9 => 2,
            ChromaSubsampling::Yv12 => 1,
        }
    }

    /// `spec/08 §5.1` — the per-axis subsampled chroma dimension from a
    /// luma dimension: `ceil(luma / scale)` = `(luma + scale - 1) >>
    /// shift` (the `+3`/`+1` rounding-up bias at
    /// `IR50_32.DLL!0x100340bc`).
    #[inline]
    pub fn chroma_dim(self, luma_dim: u32) -> u32 {
        let shift = self.shift();
        let scale = 1u32 << shift;
        (luma_dim + scale - 1) >> shift
    }

    /// Both subsampled chroma dimensions `(width, height)` for a luma
    /// plane of `(luma_w, luma_h)` (`spec/08 §5.1`).
    #[inline]
    pub fn chroma_dims(self, luma_w: u32, luma_h: u32) -> (u32, u32) {
        (self.chroma_dim(luma_w), self.chroma_dim(luma_h))
    }
}

/// `spec/08 §3.5`/`§5.2` — box-filter-replicate a subsampled chroma
/// plane up to luma resolution.
///
/// Every luma-resolution output position `(x, y)` reads the chroma
/// sample at `(x >> shift, y >> shift)` (`spec/08 §5.2` top-left-cosited
/// box filter — no interpolation). The output plane is `luma_w × luma_h`
/// pixels. The chroma sub-plane must already be the subsampled
/// resolution `subsampling.chroma_dims(luma_w, luma_h)`.
///
/// Returns `None` if the supplied chroma plane's dimensions do not match
/// the expected subsampled dimensions for the requested luma size.
pub fn upsample_chroma(
    chroma: &OutputPlane,
    subsampling: ChromaSubsampling,
    luma_w: u32,
    luma_h: u32,
) -> Option<OutputPlane> {
    let (cw, ch) = subsampling.chroma_dims(luma_w, luma_h);
    if chroma.width != cw || chroma.height != ch {
        return None;
    }
    let shift = subsampling.shift();
    let mut pixels = Vec::with_capacity((luma_w * luma_h) as usize);
    for y in 0..luma_h {
        let cy = y >> shift;
        let crow = (cy * cw) as usize;
        for x in 0..luma_w {
            let cx = (x >> shift) as usize;
            pixels.push(chroma.pixels[crow + cx]);
        }
    }
    Some(OutputPlane {
        width: luma_w,
        height: luma_h,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_per_ratio() {
        assert_eq!(ChromaSubsampling::Yvu9.shift(), 2);
        assert_eq!(ChromaSubsampling::Yv12.shift(), 1);
    }

    #[test]
    fn chroma_dims_4_1_0() {
        // spec/08 §5.1: ceil(luma/4).
        let s = ChromaSubsampling::Yvu9;
        assert_eq!(s.chroma_dims(176, 144), (44, 36));
        assert_eq!(s.chroma_dims(352, 288), (88, 72));
        // ceil rounding: 5 -> ceil(5/4) = 2.
        assert_eq!(s.chroma_dim(5), 2);
        assert_eq!(s.chroma_dim(4), 1);
    }

    #[test]
    fn chroma_dims_4_2_0() {
        // spec/08 §5.1: ceil(luma/2).
        let s = ChromaSubsampling::Yv12;
        assert_eq!(s.chroma_dims(176, 144), (88, 72));
        assert_eq!(s.chroma_dim(5), 3); // ceil(5/2)
        assert_eq!(s.chroma_dim(4), 2);
    }

    #[test]
    fn upsample_4_1_0_replicates_4x4() {
        // 1 chroma sample -> 4x4 luma block, all identical (spec/08 §3.5).
        let chroma = OutputPlane {
            width: 1,
            height: 1,
            pixels: vec![200],
        };
        let up = upsample_chroma(&chroma, ChromaSubsampling::Yvu9, 4, 4).unwrap();
        assert_eq!((up.width, up.height), (4, 4));
        assert!(up.pixels.iter().all(|&p| p == 200));
    }

    #[test]
    fn upsample_4_1_0_two_samples_step_at_boundary() {
        // 2x1 chroma -> 8x4 luma: first 4 cols = sample0, next 4 = sample1
        // (spec/08 §5.2 top-left box filter, sharp step at 4-pixel edge).
        let chroma = OutputPlane {
            width: 2,
            height: 1,
            pixels: vec![10, 20],
        };
        let up = upsample_chroma(&chroma, ChromaSubsampling::Yvu9, 8, 4).unwrap();
        for y in 0..4 {
            for x in 0..8 {
                let want = if x < 4 { 10 } else { 20 };
                assert_eq!(up.at(x, y), want, "at ({x},{y})");
            }
        }
    }

    #[test]
    fn upsample_4_2_0_replicates_2x2() {
        // 2x2 chroma -> 4x4 luma via 2x2 replication.
        let chroma = OutputPlane {
            width: 2,
            height: 2,
            pixels: vec![1, 2, 3, 4],
        };
        let up = upsample_chroma(&chroma, ChromaSubsampling::Yv12, 4, 4).unwrap();
        // Top-left 2x2 quadrant = 1, top-right = 2, bottom-left = 3, etc.
        assert_eq!(up.at(0, 0), 1);
        assert_eq!(up.at(1, 1), 1);
        assert_eq!(up.at(2, 0), 2);
        assert_eq!(up.at(0, 2), 3);
        assert_eq!(up.at(3, 3), 4);
    }

    #[test]
    fn upsample_rejects_wrong_chroma_dims() {
        let chroma = OutputPlane {
            width: 2,
            height: 2,
            pixels: vec![0; 4],
        };
        // For luma 4x4 at 4:1:0 the chroma should be 1x1, not 2x2.
        assert!(upsample_chroma(&chroma, ChromaSubsampling::Yvu9, 4, 4).is_none());
    }

    #[test]
    fn upsample_odd_luma_uses_ceil_chroma() {
        // luma 5x5 at 4:1:0 -> chroma 2x2; the trailing luma row/col
        // reads the second chroma sample (spec/08 §5.1 ceil).
        let chroma = OutputPlane {
            width: 2,
            height: 2,
            pixels: vec![7, 8, 9, 10],
        };
        let up = upsample_chroma(&chroma, ChromaSubsampling::Yvu9, 5, 5).unwrap();
        assert_eq!((up.width, up.height), (5, 5));
        // (4,4) -> chroma (1,1) = 10.
        assert_eq!(up.at(4, 4), 10);
        // (0,0) -> chroma (0,0) = 7.
        assert_eq!(up.at(0, 0), 7);
    }
}
