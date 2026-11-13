//! Indeo 3 (IV31 / IV32) — frame-level reconstruction pass.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/07-output-reconstruction.md`
//! §1.5 (per-plane independence) + §5.1 / §5.2 (the three planes decoded
//! in order into their own strip pixel buffers), built on
//! [`super::exec_plane_plan`].
//!
//! ## What this module adds
//!
//! [`super::decode_frame`] resolves a frame's structure into a
//! [`super::DecodedFrame`] (per-plane [`super::CellTree`]s).
//! [`super::classify_plane`] + [`super::exec_plane_plan`] then take *one*
//! plane to a [`super::ReconstructedPlane`] (its VQ_NULL units
//! materialised into a strip pixel buffer, its VQ_DATA / INTER frontier
//! surfaced).
//!
//! This module threads that per-plane executor across a whole decoded
//! frame. It walks the frame's present planes in `spec/07 §5.2` decode
//! order (the same U, V, Y order the [`super::DecodedFrame::planes`]
//! vector already carries), classifies and executes each, and
//! aggregates the result into a [`ReconstructedFrame`]:
//!
//! * one [`super::ReconstructedPlane`] per present plane (its strip
//!   buffer + per-plane [`super::PlaneExecStats`] + frontier), and
//! * a frame-wide [`FrameReconstructStats`] folding every plane's
//!   coverage so a caller can read, in one place, how much of the whole
//!   frame the genuinely-unblocked subset reconstructed versus how much
//!   waits on the codebook-bank docs-gap / motion compensation.
//!
//! Per `spec/07 §1.5`, the planes are independent — there is no
//! cross-plane prediction, so each plane's strip buffer is reconstructed
//! in isolation and the per-plane results never interact. A NULL frame
//! (`spec/02 §1`) carries no planes and reconstructs to an empty frame.
//!
//! ## The boundary this honours
//!
//! This pass reconstructs the **unblocked subset only** — the two
//! VQ_NULL arms (`spec/07 §1.4` copy-upper + §4.4 mark-edge). VQ_DATA
//! cells (the `spec/04 §7.1` codebook-bank docs-gap) and INTER cells
//! (motion compensation against a reference frame this single-frame pass
//! does not hold) are counted and their first occurrence is surfaced as
//! a per-plane [`super::DeferredFrontier`], but their pixels are left
//! zero. The frame-wide [`FrameReconstructStats::is_fully_reconstructed`]
//! reports whether the frame happened to be entirely VQ_NULL-coded (so
//! the unblocked subset was sufficient) — the common case is `false`
//! until the codebook-bank values land.

use super::frame::DecodedFrame;
use super::frame_assemble::{OutputFrame, OutputPlane};
use super::frame_output::upshift_7bit_to_8bit;
use super::plane_execute::{
    PlaneExecError, ReconstructedPlane, STRIP_ROW_STRIDE, exec_plane_plan,
    exec_plane_plan_with_stream,
};
use super::plane_reconstruct::classify_cell_tree;
use super::vq::DyadDeltaTable;

/// Frame-wide reconstruction coverage, folding every present plane's
/// [`super::PlaneExecStats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FrameReconstructStats {
    /// Present planes reconstructed this pass.
    pub planes: usize,
    /// VQ_NULL copy units reconstructed across all planes.
    pub copy_units: usize,
    /// VQ_NULL skip units reconstructed across all planes.
    pub skip_units: usize,
    /// VQ_NULL unpacker-dispatch units whose static-table subset was
    /// driven with real bitstream bytes (`spec/06 §5.2`;
    /// [`reconstruct_frame_with_stream`] only).
    pub unpacker_static_units: usize,
    /// VQ_NULL unpacker-dispatch units deferred across all planes
    /// (`spec/06 §5.2` hybrid; arena-gated mode-byte stream).
    pub vq_null_unpacker_deferred: usize,
    /// VQ_DATA units deferred across all planes (codebook-bank docs-gap).
    pub vq_data_deferred: usize,
    /// INTER units deferred across all planes (needs a reference frame).
    pub inter_deferred: usize,
    /// Total pixel bytes the unblocked subset wrote across all planes.
    pub bytes_written: usize,
}

impl FrameReconstructStats {
    /// Units reconstructed now across the frame (VQ_NULL copy + skip
    /// + static-subset unpacker cells).
    pub fn reconstructed(&self) -> usize {
        self.copy_units + self.skip_units + self.unpacker_static_units
    }

    /// Units deferred across the frame (VQ_NULL unpacker + VQ_DATA +
    /// INTER).
    pub fn deferred(&self) -> usize {
        self.vq_null_unpacker_deferred + self.vq_data_deferred + self.inter_deferred
    }

    /// Total reconstruction units visited across the frame.
    pub fn total(&self) -> usize {
        self.reconstructed() + self.deferred()
    }

    /// `true` if every unit across every present plane was reconstructed
    /// from the unblocked subset (no VQ_DATA / INTER deferrals) and the
    /// frame carried at least one unit. For most real frames this is
    /// `false` until the codebook-bank values land.
    pub fn is_fully_reconstructed(&self) -> bool {
        self.deferred() == 0 && self.total() > 0
    }
}

/// The reconstruction result for a whole decoded frame: one
/// [`super::ReconstructedPlane`] per present plane plus the frame-wide
/// coverage fold.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReconstructedFrame {
    /// One reconstructed plane per present plane, in the
    /// [`super::DecodedFrame::planes`] decode order (U, V, Y). Empty for
    /// a NULL frame.
    pub planes: Vec<ReconstructedPlane>,
    /// Frame-wide coverage fold.
    pub stats: FrameReconstructStats,
}

impl ReconstructedFrame {
    /// Borrow the reconstructed plane for `plane_idx` (0 = Y, 1 = V,
    /// 2 = U), if it was present and reconstructed this frame.
    pub fn plane(&self, plane_idx: usize) -> Option<&ReconstructedPlane> {
        self.planes.iter().find(|p| p.plane_idx == plane_idx)
    }

    /// `true` if no plane was reconstructed (a NULL frame, or a frame
    /// whose planes were all skipped / absent).
    pub fn is_empty(&self) -> bool {
        self.planes.is_empty()
    }

    /// Spec/07 §4.3 / §5.6 — assemble this reconstructed frame's
    /// plane-spanning strip buffers into an [`OutputFrame`] of
    /// tightly-packed 8-bit output planes.
    ///
    /// Each [`ReconstructedPlane`]'s `0xb0`-stride strip buffer is walked
    /// row by row; the `plane_width` visible bytes of each row are
    /// upshifted ([`upshift_7bit_to_8bit`]: `(b & 0x7f) << 1`, clearing
    /// the §4.4 edge-marker sentinel) into the output raster. Regions
    /// that stayed zero (the deferred VQ_DATA / INTER units) upshift to
    /// `0` — black — so the output frame is correctly shaped with the
    /// unblocked subset's pixels in place and the deferred regions left
    /// black until the codebook-bank values land.
    ///
    /// The output planes are in [`Self::planes`] order (U, V, Y as
    /// decoded); callers wanting the §5.6 Y, V, U *output* order use
    /// [`OutputFrame::plane`] by index.
    pub fn to_output_frame(&self) -> OutputFrame {
        let planes = self.planes.iter().map(upshift_plane).collect();
        OutputFrame { planes }
    }
}

/// Spec/07 §4.3 / §5.7 — upshift one reconstructed plane's strip buffer
/// into a tightly-packed 8-bit [`OutputPlane`].
///
/// Walks `plane_height` rows of the `0xb0`-stride strip buffer, copying
/// the `plane_width` visible bytes of each row through
/// [`upshift_7bit_to_8bit`] into a `width × height` raster (stride ==
/// width). A row whose visible span runs past the strip buffer (a
/// degenerate / truncated buffer) is zero-padded rather than panicking.
fn upshift_plane(plane: &ReconstructedPlane) -> OutputPlane {
    let w = plane.plane_width as usize;
    let h = plane.plane_height as usize;
    let mut pixels = vec![0u8; w * h];
    for y in 0..h {
        let src_start = y * STRIP_ROW_STRIDE;
        let dst_start = y * w;
        for x in 0..w {
            if let Some(&b) = plane.strip.get(src_start + x) {
                pixels[dst_start + x] = upshift_7bit_to_8bit(b);
            }
        }
    }
    OutputPlane {
        plane_idx: plane.plane_idx,
        width: plane.plane_width,
        height: plane.plane_height,
        pixels,
    }
}

/// Spec/07 §1.5 / §5.2 — reconstruct the unblocked (VQ_NULL) subset of
/// every present plane of a decoded frame.
///
/// Walks `frame.planes` in decode order, classifies each plane's cell
/// tree ([`classify_cell_tree`]), runs the whole-plane executor
/// ([`exec_plane_plan`]) over it, and folds the per-plane coverage into
/// a frame-wide [`FrameReconstructStats`]. Returns a
/// [`ReconstructedFrame`] carrying each plane's mutated strip buffer +
/// coverage, or the first [`FrameReconstructError`] a plane executor
/// raises (tagging which plane failed).
///
/// A NULL frame (or a frame with no present planes) reconstructs to an
/// empty [`ReconstructedFrame`] with zeroed stats.
pub fn reconstruct_frame(
    frame: &DecodedFrame,
) -> Result<ReconstructedFrame, FrameReconstructError> {
    reconstruct_frame_inner(frame, None)
}

/// [`reconstruct_frame`] with the frame's **input bitstream** attached
/// (`spec/06 §5.2`): each plane's first data-bearing unit, when it is
/// an unpacker-dispatch VQ_NULL cell, is driven through the
/// static-table mode-byte executor with real bitstream bytes (see
/// [`exec_plane_plan_with_stream`]). `input` is the same buffer that
/// was passed to [`super::decode_frame`] — the recorded stream anchors
/// are absolute offsets into it.
pub fn reconstruct_frame_with_stream(
    frame: &DecodedFrame,
    input: &[u8],
    table: &DyadDeltaTable,
) -> Result<ReconstructedFrame, FrameReconstructError> {
    reconstruct_frame_inner(frame, Some((input, table)))
}

fn reconstruct_frame_inner(
    frame: &DecodedFrame,
    stream: Option<(&[u8], &DyadDeltaTable)>,
) -> Result<ReconstructedFrame, FrameReconstructError> {
    let mut planes = Vec::with_capacity(frame.planes.len());
    let mut stats = FrameReconstructStats::default();

    for decoded in &frame.planes {
        let plan = classify_cell_tree(decoded.plane_idx, &decoded.tree);
        let recon = match stream {
            Some((input, table)) => exec_plane_plan_with_stream(&plan, input, table),
            None => exec_plane_plan(&plan),
        }
        .map_err(|source| FrameReconstructError {
            plane_idx: decoded.plane_idx,
            source,
        })?;

        stats.planes += 1;
        stats.copy_units += recon.stats.copy_units;
        stats.skip_units += recon.stats.skip_units;
        stats.unpacker_static_units += recon.stats.unpacker_static_units;
        stats.vq_null_unpacker_deferred += recon.stats.vq_null_unpacker_deferred;
        stats.vq_data_deferred += recon.stats.vq_data_deferred;
        stats.inter_deferred += recon.stats.inter_deferred;
        stats.bytes_written += recon.stats.bytes_written;

        planes.push(recon);
    }

    Ok(ReconstructedFrame { planes, stats })
}

/// A plane executor failure during a frame reconstruction pass, tagged
/// with the plane that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameReconstructError {
    /// Spec/02 §2 plane index (0 = Y, 1 = V, 2 = U) whose executor
    /// failed.
    pub plane_idx: usize,
    /// The underlying plane-executor error.
    pub source: PlaneExecError,
}

impl core::fmt::Display for FrameReconstructError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "indeo3 frame-reconstruct: plane {}: {}",
            self.plane_idx, self.source
        )
    }
}

impl std::error::Error for FrameReconstructError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::frame::decode_frame;
    use crate::indeo3::header::{
        COMBINED_HEADER_LEN, FRAME_HEADER_LEN, MAGIC_FRMH, NULL_FRAME_DATA_SIZE_BITS,
        REQUIRED_DEC_VERSION,
    };

    // Build a minimal valid combined header (mirrors frame.rs's test
    // helper) so we can decode a real DecodedFrame and feed it to
    // reconstruct_frame. This keeps the frame-level test honest — it
    // exercises the real classify → exec path over a genuinely-decoded
    // plane rather than a fabricated one.
    #[allow(clippy::too_many_arguments)]
    fn build_header(
        width: u16,
        height: u16,
        data_size_bits: u32,
        flags: u16,
        y_off: u32,
        v_off: u32,
        u_off: u32,
        total_len: u32,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; COMBINED_HEADER_LEN];
        let frame_number: u32 = 0;
        let unknown1: u32 = 0;
        let frame_size: u32 = total_len;
        let check_sum = frame_number ^ unknown1 ^ frame_size ^ MAGIC_FRMH;
        buf[0x00..0x04].copy_from_slice(&frame_number.to_le_bytes());
        buf[0x04..0x08].copy_from_slice(&unknown1.to_le_bytes());
        buf[0x08..0x0c].copy_from_slice(&check_sum.to_le_bytes());
        buf[0x0c..0x10].copy_from_slice(&frame_size.to_le_bytes());

        let b = FRAME_HEADER_LEN;
        // dec_version, flags, data_size.
        buf[b..b + 2].copy_from_slice(&REQUIRED_DEC_VERSION.to_le_bytes());
        buf[b + 2..b + 4].copy_from_slice(&flags.to_le_bytes());
        buf[b + 4..b + 8].copy_from_slice(&data_size_bits.to_le_bytes());
        buf[b + 0x0c..b + 0x0e].copy_from_slice(&height.to_le_bytes());
        buf[b + 0x0e..b + 0x10].copy_from_slice(&width.to_le_bytes());
        buf[b + 0x10..b + 0x14].copy_from_slice(&y_off.to_le_bytes());
        buf[b + 0x14..b + 0x18].copy_from_slice(&v_off.to_le_bytes());
        buf[b + 0x18..b + 0x1c].copy_from_slice(&u_off.to_le_bytes());
        buf
    }

    #[test]
    fn null_frame_reconstructs_to_empty() {
        // data_size == NULL sentinel → spec/02 §1 NULL frame (no planes).
        let buf = build_header(
            64,
            64,
            NULL_FRAME_DATA_SIZE_BITS,
            0,
            0,
            0,
            0,
            COMBINED_HEADER_LEN as u32,
        );
        let frame = decode_frame(&buf).expect("null frame decodes");
        let recon = reconstruct_frame(&frame).expect("reconstruct");
        assert!(recon.is_empty());
        assert_eq!(recon.stats.total(), 0);
        assert!(!recon.stats.is_fully_reconstructed());
        assert_eq!(recon.stats.planes, 0);
    }

    #[test]
    fn skipped_planes_reconstruct_to_empty() {
        // All plane offsets negative → spec/02 §2 skips every plane.
        let neg = 0x8000_0000u32;
        let buf = build_header(64, 64, 4096, 0, neg, neg, neg, COMBINED_HEADER_LEN as u32);
        let frame = decode_frame(&buf).expect("decodes");
        let recon = reconstruct_frame(&frame).expect("reconstruct");
        assert!(recon.is_empty());
        assert_eq!(recon.stats.planes, 0);
    }

    #[test]
    fn decoded_plane_reconstructs_or_surfaces_frontier() {
        // A tiny single-luma-plane frame (mirrors frame.rs's structural
        // test). Whichever cell classes the synthetic payload decodes
        // to, reconstruct_frame must thread through without panic and
        // return a typed result: either every unit reconstructed
        // (fully) or a deferred frontier on the first VQ_DATA / INTER.
        let bsh = FRAME_HEADER_LEN;
        let mut payload = vec![0u8; 8];
        for (i, b) in payload.iter_mut().enumerate().skip(4) {
            *b = (i as u8).wrapping_mul(0x11);
        }
        let plane_base_target = COMBINED_HEADER_LEN;
        let y_off = (plane_base_target - bsh) as u32;
        let total_len = (plane_base_target + payload.len()) as u32;
        let mut buf = build_header(
            4,
            4,
            (payload.len() as u32) * 8,
            0,
            y_off,
            0x8000_0000,
            0x8000_0000,
            total_len,
        );
        buf.extend_from_slice(&payload);

        // The structural decode may succeed or surface a deterministic
        // macroblock error; only when it decodes do we reconstruct.
        if let Ok(frame) = decode_frame(&buf) {
            let recon = reconstruct_frame(&frame).expect("reconstruct does not fail");
            // Coverage is internally consistent: reconstructed + deferred
            // == total, and each plane's frontier is set iff that plane
            // deferred at least one unit.
            assert_eq!(
                recon.stats.reconstructed() + recon.stats.deferred(),
                recon.stats.total()
            );
            for plane in &recon.planes {
                let plane_deferred = plane.stats.deferred() > 0;
                assert_eq!(plane.frontier.is_some(), plane_deferred);
            }
        }
    }

    #[test]
    fn to_output_frame_upshifts_strip_pixels() {
        use crate::indeo3::PLANE_IDX_Y;
        use crate::indeo3::ReconstructedPlane;
        use crate::indeo3::plane_execute::{PlaneExecStats, plane_strip_len};
        use crate::indeo3::reconstruct::EDGE_MARKER_BIT;

        // Build a reconstructed plane by hand with a known strip pattern:
        // row 0 holds 0x01, 0x02 with the edge marker set on the first
        // (so the upshift must clear bit 7 → (0x81 & 0x7f) << 1 = 0x02,
        // and 0x02 << 1 = 0x04).
        let mut strip = vec![0u8; plane_strip_len(2)];
        strip[0] = 0x01 | EDGE_MARKER_BIT;
        strip[1] = 0x02;
        // Row 1 (offset 0xb0) holds 0x10, 0x20.
        strip[STRIP_ROW_STRIDE] = 0x10;
        strip[STRIP_ROW_STRIDE + 1] = 0x20;
        let plane = ReconstructedPlane {
            plane_idx: PLANE_IDX_Y,
            plane_width: 2,
            plane_height: 2,
            strip,
            stats: PlaneExecStats::default(),
            frontier: None,
        };
        let frame = ReconstructedFrame {
            planes: vec![plane],
            stats: FrameReconstructStats::default(),
        };
        let output = frame.to_output_frame();
        let op = output.plane(PLANE_IDX_Y).expect("plane");
        assert_eq!(op.width, 2);
        assert_eq!(op.height, 2);
        // Row 0: edge marker cleared then <<1: 0x02, 0x04.
        assert_eq!(op.row(0), Some(&[0x02u8, 0x04][..]));
        // Row 1: 0x10<<1=0x20, 0x20<<1=0x40.
        assert_eq!(op.row(1), Some(&[0x20u8, 0x40][..]));
    }

    #[test]
    fn to_output_frame_of_empty_is_empty() {
        let frame = ReconstructedFrame {
            planes: vec![],
            stats: FrameReconstructStats::default(),
        };
        let output = frame.to_output_frame();
        assert!(output.planes.is_empty());
    }

    #[test]
    fn stats_fold_aggregates_units() {
        let stats = FrameReconstructStats {
            planes: 2,
            copy_units: 3,
            skip_units: 2,
            unpacker_static_units: 1,
            vq_null_unpacker_deferred: 2,
            vq_data_deferred: 4,
            inter_deferred: 1,
            bytes_written: 256,
        };
        assert_eq!(stats.reconstructed(), 6);
        assert_eq!(stats.deferred(), 7);
        assert_eq!(stats.total(), 13);
        assert!(!stats.is_fully_reconstructed());
    }
}
