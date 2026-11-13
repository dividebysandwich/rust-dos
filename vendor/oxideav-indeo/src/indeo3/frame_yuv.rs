//! Indeo 3 (IV31 / IV32) — full-resolution YUV frame producer.
//!
//! [`super::frame_assemble`] runs the spec/07 §5.7 strip-to-frame
//! assembly, producing an [`OutputFrame`] whose three planes are at
//! their **native** geometry: the Y plane at full picture resolution
//! and the V / U planes at their 4:1:0-subsampled resolution (one
//! quarter of the luma width *and* height, per `spec/07 §5.5`). That
//! `OutputFrame` is exactly the IF09 / YVU9 passthrough layout
//! (`spec/07 §5.6`): chroma is left subsampled because the IF09 host
//! consumes the planar YVU 4:1:0 buffer directly.
//!
//! This module wires the **next** spec/07 stage onto that view: the
//! §5.5 box-filter chroma upsampler ([`super::upsample_chroma_4x4`]).
//! `spec/07 §5.5` documents that the *output-conversion* stage (the
//! §5.4 YUV → RGB path) upsamples chroma to luma resolution by
//! "replicating each chroma sample to a 4×4 block of luma positions
//! … plain box-filter chroma upsampling: there is no chroma
//! interpolation, no edge-aware reconstruction". [`upsample_frame`]
//! applies that replication to the V and U planes of an
//! [`OutputFrame`], yielding a [`YuvFrame`] whose three planes are all
//! at full luma resolution.
//!
//! ## Why this is the §5.4-RGB-independent half of the conversion stage
//!
//! `spec/07 §5.4` (the YUV → RGB matrix) is gated on the
//! `0x1004cxxx` YUV → RGB LUTs, which `audit/00 §3.3` found are
//! **zero on disk** and runtime-populated by an undetermined RVA
//! (`audit/00 §6.1`) — a reported docs-gap. The §5.5 chroma
//! upsampling, by contrast, is a pure geometric box-filter with **no
//! table input**: §5.5 fully determines it ("integer division by 4
//! (or equivalently shift by 2)"). So a full-resolution YUV frame —
//! the exact three-plane, luma-resolution input the §5.4 matrix would
//! consume per pixel — can be produced *without* the blocked LUTs.
//! [`YuvFrame`] is that input, surfaced as a typed result so a future
//! round (once the §5.4 LUTs are extracted) only has to add the
//! per-pixel matrix multiply on top.
//!
//! ## The reconstruction handoff (unchanged)
//!
//! Like [`super::assemble_output`], this module consumes
//! caller-supplied strip pixel buffers: the per-cell reconstruction
//! that fills them is gated on the spec/04 §7.1 codebook-bank
//! docs-gap. A test harness (or a future reconstruction oracle)
//! supplies the filled strips; this module threads them through the
//! §5.7 assembly and the §5.5 upsample into a full-resolution YUV
//! frame.

use super::frame::DecodedFrame;
use super::frame_assemble::{AssembleError, OutputFrame, OutputPlane, assemble_output};
use super::frame_output::{CHROMA_UPSAMPLE_FACTOR, ChromaUpsampleError, upsample_chroma_4x4};
use super::picture_layer::{PLANE_IDX_U, PLANE_IDX_V, PLANE_IDX_Y};

/// One full-resolution plane of a [`YuvFrame`]: a tightly-packed
/// `width × height` byte raster (stride == width) of 8-bit pixels.
///
/// Unlike [`OutputPlane`], every plane of a [`YuvFrame`] — Y, V, and
/// U — is at full luma resolution: the V / U planes have been
/// box-upsampled from their 4:1:0 subsampled geometry (`spec/07
/// §5.5`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct YuvPlane {
    /// Spec/02 §2 plane index (0 = Y, 1 = V, 2 = U).
    pub plane_idx: usize,
    /// Plane width in samples (full luma resolution; == the raster's
    /// row stride).
    pub width: u32,
    /// Plane height in samples (full luma resolution).
    pub height: u32,
    /// `width × height` bytes of 8-bit pixels, row-major, stride ==
    /// `width`.
    pub pixels: Vec<u8>,
}

impl YuvPlane {
    /// Borrow the pixel row at `y` (0-based), or `None` if out of
    /// range.
    pub fn row(&self, y: u32) -> Option<&[u8]> {
        if y >= self.height {
            return None;
        }
        let w = self.width as usize;
        let start = y as usize * w;
        Some(&self.pixels[start..start + w])
    }
}

/// A full-resolution YUV frame: up to three planes (Y, V, U in
/// `spec/07 §5.6` output order), all at full luma resolution.
///
/// This is the three-plane, luma-resolution surface the `spec/07
/// §5.4` YUV → RGB matrix consumes one pixel at a time (R, G, B from
/// `(Y[r][c], V[r][c], U[r][c])`). The chroma planes have been
/// box-upsampled per `spec/07 §5.5`; the Y plane is carried through
/// unchanged from the §5.7 assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct YuvFrame {
    /// Full-resolution planes in `spec/07 §5.6` output order (Y, V,
    /// U). A plane absent from the source [`OutputFrame`] is omitted.
    pub planes: Vec<YuvPlane>,
}

impl YuvFrame {
    /// Borrow the full-resolution plane for `plane_idx`, if present.
    pub fn plane(&self, plane_idx: usize) -> Option<&YuvPlane> {
        self.planes.iter().find(|p| p.plane_idx == plane_idx)
    }

    /// Borrow the luma (Y) plane, if present.
    pub fn luma(&self) -> Option<&YuvPlane> {
        self.plane(PLANE_IDX_Y)
    }

    /// Borrow the (full-resolution) V plane, if present.
    pub fn chroma_v(&self) -> Option<&YuvPlane> {
        self.plane(PLANE_IDX_V)
    }

    /// Borrow the (full-resolution) U plane, if present.
    pub fn chroma_u(&self) -> Option<&YuvPlane> {
        self.plane(PLANE_IDX_U)
    }
}

/// Errors raised while producing a full-resolution [`YuvFrame`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum YuvError {
    /// The underlying spec/07 §5.7 strip-to-frame assembly failed.
    Assemble(AssembleError),
    /// The spec/07 §5.5 chroma box-upsample of a plane failed.
    ChromaUpsample {
        /// Spec/02 §2 plane index whose upsample failed (V or U).
        plane_idx: usize,
        /// The underlying upsampler error.
        source: ChromaUpsampleError,
    },
}

impl core::fmt::Display for YuvError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            YuvError::Assemble(e) => write!(f, "indeo3 yuv: {e}"),
            YuvError::ChromaUpsample { plane_idx, source } => {
                write!(f, "indeo3 yuv: plane {plane_idx} chroma upsample: {source}")
            }
        }
    }
}

impl std::error::Error for YuvError {}

impl From<AssembleError> for YuvError {
    fn from(e: AssembleError) -> Self {
        YuvError::Assemble(e)
    }
}

/// Box-upsample the V and U planes of an [`OutputFrame`] to full luma
/// resolution, producing a [`YuvFrame`] (`spec/07 §5.5`).
///
/// The Y plane (if present) is carried through verbatim — it is
/// already at full resolution. Each present chroma plane (V, U) is
/// box-filtered: every chroma sample is replicated into a
/// [`CHROMA_UPSAMPLE_FACTOR`]×[`CHROMA_UPSAMPLE_FACTOR`] block of
/// output positions, so the upsampled plane is `chroma_width × 4` by
/// `chroma_height × 4` samples (`spec/07 §5.5`'s "4×4 block").
///
/// Planes are emitted in `spec/07 §5.6` output order (Y, V, U),
/// matching the source [`OutputFrame::planes`] order.
pub fn upsample_frame(output: &OutputFrame) -> Result<YuvFrame, YuvError> {
    let mut planes = Vec::with_capacity(output.planes.len());

    for plane in &output.planes {
        let yuv_plane = match plane.plane_idx {
            // Luma is already at full resolution — copy through.
            PLANE_IDX_Y => YuvPlane {
                plane_idx: plane.plane_idx,
                width: plane.width,
                height: plane.height,
                pixels: plane.pixels.clone(),
            },
            // V / U are 4:1:0 subsampled — box-upsample 4×4.
            _ => upsample_chroma_plane(plane)?,
        };
        planes.push(yuv_plane);
    }

    Ok(YuvFrame { planes })
}

/// Decode a frame's structure and produce a full-resolution
/// [`YuvFrame`] from caller-supplied strip pixel buffers.
///
/// This is the [`upsample_frame`]∘[`assemble_output`] composition: it
/// runs the spec/07 §5.7 strip-to-frame assembly over `strip_sets`
/// (the same shape [`super::allocate_strip_buffers`] returns) and then
/// the spec/07 §5.5 chroma box-upsample, yielding a three-plane
/// luma-resolution frame.
///
/// See [`assemble_output`] for the `strip_sets` contract; the strip
/// buffers are caller-supplied because the per-cell reconstruction
/// that fills them is gated on the spec/04 §7.1 codebook docs-gap.
pub fn assemble_yuv(
    frame: &DecodedFrame,
    strip_sets: &[(usize, Vec<Vec<u8>>)],
) -> Result<YuvFrame, YuvError> {
    let output = assemble_output(frame, strip_sets)?;
    upsample_frame(&output)
}

/// Box-upsample one chroma [`OutputPlane`] (V or U) to full luma
/// resolution (`spec/07 §5.5`).
fn upsample_chroma_plane(plane: &OutputPlane) -> Result<YuvPlane, YuvError> {
    let up_w = plane.width as usize * CHROMA_UPSAMPLE_FACTOR;
    let up_h = plane.height as usize * CHROMA_UPSAMPLE_FACTOR;

    let mut pixels = vec![0u8; up_w * up_h];
    // The source raster is tightly packed (stride == width per
    // `OutputPlane`); the destination is tightly packed at the
    // upsampled width.
    upsample_chroma_4x4(
        &plane.pixels,
        plane.width,
        plane.height,
        plane.width as usize,
        &mut pixels,
        up_w,
    )
    .map_err(|source| YuvError::ChromaUpsample {
        plane_idx: plane.plane_idx,
        source,
    })?;

    Ok(YuvPlane {
        plane_idx: plane.plane_idx,
        width: up_w as u32,
        height: up_h as u32,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::frame_assemble::OutputPlane;

    /// Build a synthetic `OutputFrame` with a full-res Y plane and
    /// subsampled V / U planes, all with known constant fills.
    fn build_output_frame(
        luma_w: u32,
        luma_h: u32,
        y_fill: u8,
        v_fill: u8,
        u_fill: u8,
    ) -> OutputFrame {
        let cw = luma_w / 4;
        let ch = luma_h / 4;
        OutputFrame {
            planes: vec![
                OutputPlane {
                    plane_idx: PLANE_IDX_Y,
                    width: luma_w,
                    height: luma_h,
                    pixels: vec![y_fill; (luma_w * luma_h) as usize],
                },
                OutputPlane {
                    plane_idx: PLANE_IDX_V,
                    width: cw,
                    height: ch,
                    pixels: vec![v_fill; (cw * ch) as usize],
                },
                OutputPlane {
                    plane_idx: PLANE_IDX_U,
                    width: cw,
                    height: ch,
                    pixels: vec![u_fill; (cw * ch) as usize],
                },
            ],
        }
    }

    #[test]
    fn luma_plane_is_carried_through_unchanged() {
        let of = build_output_frame(16, 16, 0x42, 0x10, 0x20);
        let yuv = upsample_frame(&of).expect("upsample");
        let y = yuv.luma().expect("luma present");
        // Luma keeps its full resolution and contents.
        assert_eq!(y.width, 16);
        assert_eq!(y.height, 16);
        assert_eq!(y.pixels.len(), 16 * 16);
        assert!(y.pixels.iter().all(|&b| b == 0x42));
    }

    #[test]
    fn chroma_planes_upsample_to_luma_resolution() {
        let of = build_output_frame(16, 16, 0x42, 0x10, 0x20);
        let yuv = upsample_frame(&of).expect("upsample");

        // 16/4 = 4-wide chroma upsamples to 16-wide full resolution.
        let v = yuv.chroma_v().expect("V present");
        assert_eq!(v.width, 16);
        assert_eq!(v.height, 16);
        assert_eq!(v.pixels.len(), 16 * 16);
        assert!(v.pixels.iter().all(|&b| b == 0x10));

        let u = yuv.chroma_u().expect("U present");
        assert_eq!(u.width, 16);
        assert_eq!(u.height, 16);
        assert!(u.pixels.iter().all(|&b| b == 0x20));
    }

    #[test]
    fn box_filter_replicates_each_sample_into_a_4x4_block() {
        // A 1-sample-wide chroma plane with a single value upsamples
        // to a 4×4 constant block; a 2×1 chroma plane with two
        // distinct values upsamples to two horizontally-adjacent
        // 4×4 blocks.
        let of = OutputFrame {
            planes: vec![OutputPlane {
                plane_idx: PLANE_IDX_V,
                width: 2,
                height: 1,
                pixels: vec![0x05, 0x0a],
            }],
        };
        let yuv = upsample_frame(&of).expect("upsample");
        let v = yuv.chroma_v().expect("V present");
        assert_eq!(v.width, 8); // 2 * 4
        assert_eq!(v.height, 4); // 1 * 4
        // First 4 columns of every row are 0x05; last 4 are 0x0a.
        for y in 0..4 {
            let row = v.row(y).expect("row");
            assert_eq!(&row[0..4], &[0x05; 4]);
            assert_eq!(&row[4..8], &[0x0a; 4]);
        }
    }

    #[test]
    fn output_order_is_preserved_y_v_u() {
        let of = build_output_frame(16, 16, 1, 2, 3);
        let yuv = upsample_frame(&of).expect("upsample");
        let order: Vec<usize> = yuv.planes.iter().map(|p| p.plane_idx).collect();
        assert_eq!(order, vec![PLANE_IDX_Y, PLANE_IDX_V, PLANE_IDX_U]);
    }

    #[test]
    fn luma_only_frame_yields_single_plane() {
        let of = OutputFrame {
            planes: vec![OutputPlane {
                plane_idx: PLANE_IDX_Y,
                width: 8,
                height: 8,
                pixels: vec![0x7f; 64],
            }],
        };
        let yuv = upsample_frame(&of).expect("upsample");
        assert_eq!(yuv.planes.len(), 1);
        assert!(yuv.chroma_v().is_none());
        assert!(yuv.chroma_u().is_none());
        assert_eq!(yuv.luma().unwrap().width, 8);
    }
}
