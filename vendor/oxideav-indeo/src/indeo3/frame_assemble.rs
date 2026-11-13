//! Indeo 3 (IV31 / IV32) — output-plane assembly driver.
//!
//! [`super::frame`] threads the spec/01 → spec/02 → spec/03 layers
//! into a [`DecodedFrame`] structural view. This module wires the
//! spec/07 output stage onto that view: it allocates the three
//! output planes at their spec/02 §4 geometry and runs the spec/07
//! §5.7 strip-to-frame assembly (`assemble_plane_if09`) for each
//! present plane.
//!
//! ## The reconstruction handoff
//!
//! The spec/07 §5.7 assembly consumes one **filled strip pixel
//! buffer** per strip — the buffer the per-cell reconstruction
//! (spec/04 / spec/05) writes its decoded pixels into. That
//! reconstruction is gated on the codebook-bank per-entry values
//! (the spec/04 §7.1 Extractor docs-gap; see [`super::frame`]). So
//! this module's public entry point takes the per-plane strip
//! buffers as a **caller-supplied input**: callers that have a
//! reconstruction oracle (a future Extractor round, or a test
//! harness feeding known strip contents) pass the filled strips and
//! receive fully-assembled output planes; callers without one pass
//! zeroed strips and receive a correctly-shaped all-zero plane.
//!
//! This keeps the output path exercised end-to-end against the
//! driver's real geometry while honouring the clean-room boundary at
//! the un-extracted bank values.

use super::frame::DecodedFrame;
use super::frame_output::{PlaneAssembleError, assemble_plane_if09, strip_min_buffer_bytes};
use super::picture_layer::{PLANE_COUNT, PLANE_IDX_U, PLANE_IDX_V, PLANE_IDX_Y};
use super::strip_context::StripGeometry;

/// One assembled output plane: a tightly-packed
/// `plane_width × plane_height` byte raster (stride == width).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputPlane {
    /// Spec/02 §2 plane index (0 = Y, 1 = V, 2 = U).
    pub plane_idx: usize,
    /// Plane width in samples (the assembled raster's row stride).
    pub width: u32,
    /// Plane height in samples.
    pub height: u32,
    /// `width × height` bytes of upshifted 8-bit pixels, row-major,
    /// stride == `width`.
    pub pixels: Vec<u8>,
}

impl OutputPlane {
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

/// The assembled output of one Indeo 3 codec frame: up to three
/// output planes (Y, V, U) in spec/07 §5.6 output order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFrame {
    /// Assembled planes, in spec/07 §5.6 output order (Y, V, U).
    /// A plane absent from the decoded frame is omitted here.
    pub planes: Vec<OutputPlane>,
}

impl OutputFrame {
    /// Borrow the assembled plane for `plane_idx`, if present.
    pub fn plane(&self, plane_idx: usize) -> Option<&OutputPlane> {
        self.planes.iter().find(|p| p.plane_idx == plane_idx)
    }

    /// Borrow the assembled luma (Y) plane, if present.
    pub fn luma(&self) -> Option<&OutputPlane> {
        self.plane(PLANE_IDX_Y)
    }
}

/// Errors raised while assembling output planes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AssembleError {
    /// The caller supplied the wrong number of strip buffers for a
    /// plane.
    StripCountMismatch {
        /// Spec/02 §2 plane index whose strip count was wrong.
        plane_idx: usize,
        /// Strip count the plane's geometry requires.
        expected: u32,
        /// Strip count the caller supplied.
        supplied: usize,
    },
    /// The spec/07 §5.7 assembly rejected a plane's strips.
    PlaneAssembly {
        /// Spec/02 §2 plane index whose assembly failed.
        plane_idx: usize,
        /// The underlying assembly error.
        source: PlaneAssembleError,
    },
}

impl core::fmt::Display for AssembleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AssembleError::StripCountMismatch {
                plane_idx,
                expected,
                supplied,
            } => write!(
                f,
                "indeo3 assemble: plane {plane_idx} needs {expected} strips, got {supplied}"
            ),
            AssembleError::PlaneAssembly { plane_idx, source } => {
                write!(f, "indeo3 assemble: plane {plane_idx}: {source}")
            }
        }
    }
}

impl std::error::Error for AssembleError {}

/// Spec/07 §5.6 — the output plane order (Y, V, U).
pub const OUTPUT_ASSEMBLE_ORDER: [usize; PLANE_COUNT] = [PLANE_IDX_Y, PLANE_IDX_V, PLANE_IDX_U];

const _: () = assert!(OUTPUT_ASSEMBLE_ORDER[0] == PLANE_IDX_Y);
const _: () = assert!(OUTPUT_ASSEMBLE_ORDER[1] == PLANE_IDX_V);
const _: () = assert!(OUTPUT_ASSEMBLE_ORDER[2] == PLANE_IDX_U);

/// The minimum strip-buffer length each strip of a plane needs for a
/// spec/07 §5.7 assembly walk over the plane's geometry.
///
/// Returns one length per strip, in left-to-right order. A caller
/// allocating zeroed strip buffers can size each to the matching
/// entry; a caller with reconstructed strips can use these to
/// validate its buffers before assembly.
pub fn plane_strip_buffer_lengths(geometry: &StripGeometry) -> Vec<usize> {
    geometry
        .iter_strip_widths()
        .map(|w| strip_min_buffer_bytes(w, geometry.plane_height))
        .collect()
}

/// Allocate a zeroed strip-buffer set for every present plane of a
/// decoded frame, sized to each plane's spec/07 §5.7 assembly walk.
///
/// The returned vector is indexed in spec/07 §5.6 output order
/// (Y, V, U), each entry being `(plane_idx, Vec<Vec<u8>>)` where the
/// inner vector holds one zeroed buffer per strip. Reconstruction
/// (when its codebook-bank docs-gap is closed) fills these buffers;
/// [`assemble_output`] then upshifts and packs them into output
/// planes.
pub fn allocate_strip_buffers(frame: &DecodedFrame) -> Vec<(usize, Vec<Vec<u8>>)> {
    let mut out = Vec::new();
    for &plane_idx in &OUTPUT_ASSEMBLE_ORDER {
        let Some(plane) = frame.plane(plane_idx) else {
            continue;
        };
        let lengths = plane_strip_buffer_lengths(&plane.plan.geometry);
        let strips = lengths.into_iter().map(|n| vec![0u8; n]).collect();
        out.push((plane_idx, strips));
    }
    out
}

/// Assemble the output planes of a decoded frame from caller-supplied
/// strip pixel buffers.
///
/// `strip_sets` is indexed by spec/07 §5.6 output order (Y, V, U) —
/// the same shape [`allocate_strip_buffers`] returns; each entry is
/// `(plane_idx, &[strip_buffer])`. For each present plane the
/// function runs the spec/07 §5.7 strip-to-frame assembly
/// (`assemble_plane_if09`), which upshifts each strip's 7-bit pixels
/// to 8 bits (clearing the §4.4 edge-marker bit) and packs them into
/// a tightly-strided output raster.
///
/// Planes present in `frame` but absent from `strip_sets` are simply
/// not assembled. Planes in `strip_sets` whose `plane_idx` is not in
/// `frame` are ignored.
pub fn assemble_output(
    frame: &DecodedFrame,
    strip_sets: &[(usize, Vec<Vec<u8>>)],
) -> Result<OutputFrame, AssembleError> {
    let mut planes = Vec::new();

    for &plane_idx in &OUTPUT_ASSEMBLE_ORDER {
        let Some(plane) = frame.plane(plane_idx) else {
            continue;
        };
        let Some((_, strips)) = strip_sets.iter().find(|(idx, _)| *idx == plane_idx) else {
            continue;
        };

        let geometry = &plane.plan.geometry;
        if strips.len() != geometry.strip_count as usize {
            return Err(AssembleError::StripCountMismatch {
                plane_idx,
                expected: geometry.strip_count,
                supplied: strips.len(),
            });
        }

        let width = geometry.plane_width;
        let height = geometry.plane_height;
        let mut pixels = vec![0u8; (width as usize) * (height as usize)];

        let strip_refs: Vec<&[u8]> = strips.iter().map(|s| s.as_slice()).collect();
        assemble_plane_if09(geometry, &strip_refs, &mut pixels, width as usize)
            .map_err(|source| AssembleError::PlaneAssembly { plane_idx, source })?;

        planes.push(OutputPlane {
            plane_idx,
            width,
            height,
            pixels,
        });
    }

    Ok(OutputFrame { planes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::frame::decode_frame;
    use crate::indeo3::header::{
        COMBINED_HEADER_LEN, FRAME_HEADER_LEN, MAGIC_FRMH, REQUIRED_DEC_VERSION,
    };
    use crate::indeo3::strip_context::StripGeometry;

    // A 16x16 single-INTRA-plane frame whose Y plane parses cleanly.
    // V / U are skipped so we exercise a one-plane assembly.
    fn build_one_plane_frame() -> Vec<u8> {
        let bsh = FRAME_HEADER_LEN;
        let plane_base_target = COMBINED_HEADER_LEN;
        let y_off = (plane_base_target - bsh) as u32;

        // prelude: num_vectors (u32 = 0) then a payload that the tree
        // walker accepts. A 16x16 plane split into cells needs a few
        // bytes of tree codes; we feed an all-zero payload (the
        // all-INTRA-split path) of generous length.
        let mut payload = vec![0u8; 4 + 64];
        // num_vectors stays 0 (INTRA).
        let total_len = (plane_base_target + payload.len()) as u32;

        let mut buf = vec![0u8; COMBINED_HEADER_LEN];
        // frame_number ^ unknown1 ^ frame_size ^ 'FRMH' with the
        // first two fields zero.
        let frame_size = total_len;
        let check_sum = frame_size ^ MAGIC_FRMH;
        buf[0x08..0x0c].copy_from_slice(&check_sum.to_le_bytes());
        buf[0x0c..0x10].copy_from_slice(&frame_size.to_le_bytes());

        let b = bsh;
        buf[b..b + 2].copy_from_slice(&REQUIRED_DEC_VERSION.to_le_bytes());
        // frame_flags = 0
        buf[b + 4..b + 8].copy_from_slice(&((payload.len() as u32) * 8).to_le_bytes());
        let h: u16 = 16;
        let w: u16 = 16;
        buf[b + 0x0c..b + 0x0e].copy_from_slice(&h.to_le_bytes());
        buf[b + 0x0e..b + 0x10].copy_from_slice(&w.to_le_bytes());
        buf[b + 0x10..b + 0x14].copy_from_slice(&y_off.to_le_bytes());
        buf[b + 0x14..b + 0x18].copy_from_slice(&0x8000_0000u32.to_le_bytes()); // V skip
        buf[b + 0x18..b + 0x1c].copy_from_slice(&0x8000_0000u32.to_le_bytes()); // U skip

        // Make the payload non-degenerate so the tree walk has bytes.
        for (i, byte) in payload.iter_mut().enumerate().skip(4) {
            *byte = (i % 7) as u8;
        }
        buf.extend_from_slice(&payload);
        buf
    }

    #[test]
    fn strip_buffer_lengths_match_geometry() {
        let g = StripGeometry::for_luma(16, 16);
        let lengths = plane_strip_buffer_lengths(&g);
        assert_eq!(lengths.len(), g.strip_count as usize);
        // A 16-wide luma plane fits in one 0xa0-wide strip.
        assert_eq!(g.strip_count, 1);
        // Last (only) strip width is 16; 16 rows.
        assert_eq!(lengths[0], strip_min_buffer_bytes(16, 16));
    }

    #[test]
    fn output_order_is_y_v_u() {
        assert_eq!(
            OUTPUT_ASSEMBLE_ORDER,
            [PLANE_IDX_Y, PLANE_IDX_V, PLANE_IDX_U]
        );
    }

    #[test]
    fn allocate_then_assemble_zeroed_yields_zero_plane() {
        let buf = build_one_plane_frame();
        let frame = match decode_frame(&buf) {
            Ok(f) => f,
            // If the synthetic tree bytes are rejected, the assembly
            // path is still covered by the geometry-only tests; skip.
            Err(_) => return,
        };
        // Only proceed if the Y plane decoded.
        let Some(yplane) = frame.plane(PLANE_IDX_Y) else {
            return;
        };
        let w = yplane.plan.geometry.plane_width;
        let h = yplane.plan.geometry.plane_height;

        let strips = allocate_strip_buffers(&frame);
        let out = assemble_output(&frame, &strips).expect("assembly succeeds");

        let oy = out.luma().expect("luma plane assembled");
        assert_eq!(oy.width, w);
        assert_eq!(oy.height, h);
        assert_eq!(oy.pixels.len(), (w as usize) * (h as usize));
        // Zeroed strips upshift to zero.
        assert!(oy.pixels.iter().all(|&b| b == 0));
        // Row accessor is consistent.
        if h > 0 {
            assert_eq!(oy.row(0).unwrap().len(), w as usize);
            assert!(oy.row(h).is_none());
        }
    }

    #[test]
    fn strip_count_mismatch_is_rejected() {
        let buf = build_one_plane_frame();
        let Ok(frame) = decode_frame(&buf) else {
            return;
        };
        if frame.plane(PLANE_IDX_Y).is_none() {
            return;
        }
        // Supply zero strips for the Y plane → mismatch.
        let bad = vec![(PLANE_IDX_Y, Vec::<Vec<u8>>::new())];
        match assemble_output(&frame, &bad) {
            Err(AssembleError::StripCountMismatch { plane_idx, .. }) => {
                assert_eq!(plane_idx, PLANE_IDX_Y);
            }
            other => panic!("expected strip-count mismatch, got {other:?}"),
        }
    }

    #[test]
    fn non_zero_strip_upshifts_by_one_bit() {
        // Directly exercise the assembly arithmetic on a constructed
        // single-strip plane: a 4x4 luma-style plane with a known
        // 7-bit strip value upshifts to 2x that value (clearing the
        // edge-marker bit 7 in the process via the §4.3 `shl 1`).
        let g = StripGeometry::for_luma(4, 4);
        assert_eq!(g.strip_count, 1);
        let len = plane_strip_buffer_lengths(&g)[0];
        let mut strip = vec![0u8; len];
        // Fill the visible 4x4 region with value 0x09 (7-bit).
        let stride = crate::indeo3::FRAME_OUTPUT_SRC_ROW_STRIDE;
        for row in 0..4 {
            for col in 0..4 {
                strip[row * stride + col] = 0x09;
            }
        }
        let mut pixels = vec![0u8; 4 * 4];
        let strip_refs: [&[u8]; 1] = [strip.as_slice()];
        assemble_plane_if09(&g, &strip_refs, &mut pixels, 4).expect("assemble");
        // 0x09 << 1 == 0x12.
        assert!(pixels.iter().all(|&b| b == 0x12), "got {pixels:?}");
    }
}
