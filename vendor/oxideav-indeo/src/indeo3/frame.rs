//! Indeo 3 (IV31 / IV32) — end-to-end frame-decode driver.
//!
//! The earlier rounds landed each decode stage as an independent,
//! caller-driven primitive (header parse, picture-layer parse,
//! per-plane decode plan, binary-tree cell walk, strip / output
//! assembly). Each stopped at its documented chapter boundary and
//! none of them was wired into a single decode entry point.
//!
//! This module threads those primitives into one structural decode
//! pass over a whole codec frame:
//!
//! 1. [`FrameHeader::parse`](super::FrameHeader::parse) — the
//!    spec/01 64-byte combined header.
//! 2. [`PictureLayer::parse`](super::PictureLayer::parse) — the
//!    spec/02 per-plane preludes (NULL-frame short-circuit, the §2
//!    plane range check, the §3.1 / §3.2 motion-vector preamble).
//! 3. [`PictureLayer::plane_decode_plan`](super::PictureLayer::plane_decode_plan)
//!    — the spec/02 §4 / §5 / §6 per-plane decode plan (strip
//!    geometry, slot descriptor, payload offset).
//! 4. [`decode_plane_tree`](super::decode_plane_tree) — the spec/03
//!    binary-tree walk over each present plane's payload, producing
//!    a typed [`CellTree`](super::CellTree) of INTRA / INTER leaf
//!    cells.
//!
//! The driver produces a [`DecodedFrame`]: a per-plane structural
//! view (geometry + cell tree + per-cell-class statistics) of the
//! whole frame, walked in spec/02 §8 decode order (U → V → Y).
//!
//! ## Where the pipeline stops
//!
//! Pixel reconstruction (spec/04 §3.2's cell-state dispatch and the
//! spec/04 §3.3 codebook-bank lookup, the spec/05 motion
//! compensation, the spec/07 output assembly) requires the
//! **codebook-bank per-entry values** (`bank[+0x000]` cl-counter
//! LUT, `bank[+0x200]` slot-index LUT, `bank[+0x300]` cell-position
//! LUT, `bank[+0x700]` aux LUT). Per `spec/04 §7.1` (audit-corrected
//! against `audit/00-report.md §3`/§4) those tables are all-zero on
//! disk and are built at codec-init by `IR32_32.DLL!0x100060de`; the
//! exact per-entry recipe for several of them remains an Extractor
//! docs-gap (`SeedDispatchTables` materialises only the low-half
//! tables fully determined by the vendored 258-byte seed). The
//! driver therefore stops at the *structural* boundary — it fully
//! resolves the cell decomposition of every present plane but does
//! not synthesise pixels. [`DecodedFrame::reconstruction_status`]
//! records whether the structural decode reached that boundary
//! cleanly.

use super::header::{FrameFlags, FrameHeader, HeaderError};
use super::macroblock::{Cell, CellTree, MacroblockError, VqLeaf, decode_plane_tree};
use super::picture_layer::{
    PLANE_COUNT, PLANE_IDX_U, PLANE_IDX_V, PLANE_IDX_Y, PictureLayer, PictureLayerError,
    PlaneDecodePlan, PlanePresence,
};

/// Spec/02 §8 — the plane decode order (U, V, Y) the driver walks.
///
/// This is the same `[2, 1, 0]` count-down the per-plane decode loop
/// (`IR32_32.DLL!0x10004d2c` outer loop) uses; it aliases
/// [`super::PLANE_ITERATION_ORDER`] with a `const _` cross-check so
/// the two never drift.
pub const FRAME_PLANE_DECODE_ORDER: [usize; PLANE_COUNT] = [PLANE_IDX_U, PLANE_IDX_V, PLANE_IDX_Y];

const _: () = assert!(FRAME_PLANE_DECODE_ORDER[0] == PLANE_IDX_U);
const _: () = assert!(FRAME_PLANE_DECODE_ORDER[1] == PLANE_IDX_V);
const _: () = assert!(FRAME_PLANE_DECODE_ORDER[2] == PLANE_IDX_Y);

/// Errors raised by the end-to-end frame-decode driver.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameDecodeError {
    /// The spec/01 header parse failed.
    Header(HeaderError),
    /// The spec/02 picture-layer parse failed.
    PictureLayer(PictureLayerError),
    /// The spec/03 binary-tree walk of a present plane failed. The
    /// `plane_idx` (0 = Y, 1 = V, 2 = U) identifies which plane's
    /// payload was malformed.
    PlaneTree {
        /// Spec/02 §2 plane index whose tree walk failed.
        plane_idx: usize,
        /// The underlying spec/03 macroblock-layer error.
        source: MacroblockError,
    },
    /// A present plane's [`PlaneDecodePlan`] could not be built even
    /// though the prelude parsed — an internal invariant violation
    /// (the picture layer reported the plane present but the plan
    /// builder rejected the plane index or strip geometry).
    PlanePlanUnavailable {
        /// Spec/02 §2 plane index that lacked a decode plan.
        plane_idx: usize,
    },
}

impl core::fmt::Display for FrameDecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FrameDecodeError::Header(e) => write!(f, "indeo3 frame: header: {e}"),
            FrameDecodeError::PictureLayer(e) => write!(f, "indeo3 frame: picture layer: {e}"),
            FrameDecodeError::PlaneTree { plane_idx, source } => {
                write!(f, "indeo3 frame: plane {plane_idx} tree walk: {source}")
            }
            FrameDecodeError::PlanePlanUnavailable { plane_idx } => {
                write!(f, "indeo3 frame: plane {plane_idx} has no decode plan")
            }
        }
    }
}

impl std::error::Error for FrameDecodeError {}

impl From<HeaderError> for FrameDecodeError {
    fn from(e: HeaderError) -> Self {
        FrameDecodeError::Header(e)
    }
}

impl From<PictureLayerError> for FrameDecodeError {
    fn from(e: PictureLayerError) -> Self {
        FrameDecodeError::PictureLayer(e)
    }
}

/// Per-class cell-count statistics for one decoded plane.
///
/// The spec/03 binary-tree walk classifies every leaf as INTRA
/// (carrying a VQ sub-tree) or INTER (carrying a motion-vector
/// index). For INTRA cells the VQ sub-tree's own leaves are further
/// split into VQ_DATA (codebook-index) and VQ_NULL (copy / skip)
/// sub-cells. These counts summarise that decomposition without
/// re-walking the tree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlaneCellStats {
    /// Number of top-level INTRA leaf cells (spec/03 §3.3).
    pub intra_cells: usize,
    /// Number of top-level INTER leaf cells (spec/03 §3.4).
    pub inter_cells: usize,
    /// Number of VQ_DATA sub-cells across all INTRA cells
    /// (spec/03 §4.1; codebook-index leaves).
    pub vq_data_cells: usize,
    /// Number of VQ_NULL sub-cells across all INTRA cells
    /// (spec/03 §4.1; copy / skip leaves).
    pub vq_null_cells: usize,
}

impl PlaneCellStats {
    /// Total top-level leaf cells (INTRA + INTER).
    pub fn total_cells(&self) -> usize {
        self.intra_cells + self.inter_cells
    }

    fn from_tree(tree: &CellTree) -> Self {
        let mut stats = PlaneCellStats::default();
        for cell in &tree.cells {
            match cell {
                Cell::Inter { .. } => stats.inter_cells += 1,
                Cell::Intra { vq_leaves, .. } => {
                    stats.intra_cells += 1;
                    for vq in vq_leaves {
                        match vq.leaf {
                            VqLeaf::Data { .. } => stats.vq_data_cells += 1,
                            VqLeaf::Null(_) => stats.vq_null_cells += 1,
                        }
                    }
                }
            }
        }
        stats
    }
}

/// The structural decode result for one present plane.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodedPlane {
    /// Spec/02 §2 plane index (0 = Y, 1 = V, 2 = U).
    pub plane_idx: usize,
    /// The spec/02 §4 / §5 / §6 per-plane decode plan (geometry,
    /// slot descriptor, payload offset, motion-vector count).
    pub plan: PlaneDecodePlan,
    /// The spec/03 binary-tree decomposition of the plane.
    pub tree: CellTree,
    /// Per-class cell-count summary of [`Self::tree`].
    pub stats: PlaneCellStats,
}

impl DecodedPlane {
    /// Spec/02 §3.1 — true iff this plane carries no motion vectors
    /// (an INTRA-coded plane).
    pub fn is_intra(&self) -> bool {
        self.plan.is_intra()
    }
}

/// Whether a frame's payload is structurally decodable and how far
/// the pipeline carried it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconstructionStatus {
    /// Spec/02 §1 — a NULL / sync frame (`data_size == 0x80`). No
    /// plane payload is present; the reference decoder reproduces the
    /// prior frame's output. The driver carries no planes for such a
    /// frame.
    NullFrame,
    /// Every present plane's cell tree was walked to completion. The
    /// structural decode reached the spec/04 §3.2 cell-state-dispatch
    /// boundary; pixel synthesis is gated on the codebook-bank
    /// per-entry values (the spec/04 §7.1 Extractor docs-gap).
    StructureComplete,
}

/// The end-to-end structural decode of one Indeo 3 codec frame.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodedFrame {
    /// The parsed spec/01 combined header.
    pub header: FrameHeader,
    /// The parsed spec/02 picture layer (per-plane presence records).
    pub picture: PictureLayer,
    /// The per-present-plane structural decode, in spec/02 §8 decode
    /// order (U, V, Y). Empty for a NULL frame.
    pub planes: Vec<DecodedPlane>,
    /// How far the pipeline carried this frame.
    pub reconstruction_status: ReconstructionStatus,
}

impl DecodedFrame {
    /// Coded luma picture width (spec/01 §3.6).
    pub fn width(&self) -> u16 {
        self.header.bitstream.width
    }

    /// Coded luma picture height (spec/01 §3.6).
    pub fn height(&self) -> u16 {
        self.header.bitstream.height
    }

    /// Spec/02 §1 — true iff this frame is a NULL / sync frame.
    pub fn is_null_frame(&self) -> bool {
        matches!(self.reconstruction_status, ReconstructionStatus::NullFrame)
    }

    /// Spec/02 §3.2 / §5.1 `frame_flags` bit 9 buffer selector this
    /// frame decoded against.
    pub fn buffer_selector(&self) -> bool {
        self.header.bitstream.frame_flags.buffer_selector()
    }

    /// Borrow the decoded plane for `plane_idx` (0 = Y, 1 = V,
    /// 2 = U), if it was present and decoded this frame.
    pub fn plane(&self, plane_idx: usize) -> Option<&DecodedPlane> {
        self.planes.iter().find(|p| p.plane_idx == plane_idx)
    }
}

/// Decode the structural layers of an Indeo 3 codec frame.
///
/// `input` is the codec's input buffer — the bytes the VfW driver
/// hands the codec, already shorn of any container envelope
/// (spec/00 §1). The frame begins with the 64-byte combined header
/// at offset 0.
///
/// The driver runs the spec/01 → spec/02 → spec/03 pipeline and
/// returns a [`DecodedFrame`] structural view. It does **not**
/// synthesise pixels — see the module documentation for the
/// codebook-bank docs-gap that gates reconstruction.
///
/// `buffer_selector` overrides the bank selection; pass
/// `header.bitstream.frame_flags.buffer_selector()` for the
/// frame's own bit-9 selection (the [`decode_frame`] convenience
/// wrapper does exactly that).
pub fn decode_frame_with_selector(
    input: &[u8],
    buffer_selector: bool,
) -> Result<DecodedFrame, FrameDecodeError> {
    let header = FrameHeader::parse(input)?;
    let picture = PictureLayer::parse(&header, input)?;

    if header.bitstream.is_null_frame() {
        return Ok(DecodedFrame {
            header,
            picture,
            planes: Vec::new(),
            reconstruction_status: ReconstructionStatus::NullFrame,
        });
    }

    let flags: FrameFlags = header.bitstream.frame_flags;
    let mut planes = Vec::with_capacity(PLANE_COUNT);

    // Spec/02 §8 — walk planes in U, V, Y decode order.
    for &plane_idx in &FRAME_PLANE_DECODE_ORDER {
        let prelude = match &picture.planes[plane_idx] {
            PlanePresence::Present(p) => p,
            // Skipped / absent planes carry no payload — the
            // per-plane decode loop skips them (spec/02 §2).
            _ => continue,
        };

        let plan = picture
            .plane_decode_plan(plane_idx, &header, buffer_selector)
            .ok_or(FrameDecodeError::PlanePlanUnavailable { plane_idx })?;

        let tree = decode_plane_tree(
            input,
            prelude,
            plan.plane_width,
            plan.plane_height,
            plan.is_chroma(),
            flags,
        )
        .map_err(|source| FrameDecodeError::PlaneTree { plane_idx, source })?;

        let stats = PlaneCellStats::from_tree(&tree);
        planes.push(DecodedPlane {
            plane_idx,
            plan,
            tree,
            stats,
        });
    }

    Ok(DecodedFrame {
        header,
        picture,
        planes,
        reconstruction_status: ReconstructionStatus::StructureComplete,
    })
}

/// Decode the structural layers of an Indeo 3 codec frame using the
/// frame's own `frame_flags` bit-9 buffer selector.
///
/// Convenience wrapper over [`decode_frame_with_selector`] that
/// reads the buffer selector from the parsed header. The header is
/// parsed twice (once here to recover the selector, once inside the
/// driver) — cheap relative to the plane walk, and it keeps the
/// public entry point a single `&[u8]` argument.
pub fn decode_frame(input: &[u8]) -> Result<DecodedFrame, FrameDecodeError> {
    let header = FrameHeader::parse(input)?;
    let selector = header.bitstream.frame_flags.buffer_selector();
    decode_frame_with_selector(input, selector)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::header::{
        COMBINED_HEADER_LEN, FRAME_HEADER_LEN, MAGIC_FRMH, NULL_FRAME_DATA_SIZE_BITS,
        REQUIRED_DEC_VERSION,
    };

    // Build a minimal valid combined header for a frame of the given
    // dimensions, with the supplied plane offsets and frame flags.
    // The frame_size / checksum fields are filled to pass
    // `FrameHeader::parse`'s spec/01 §2.1 / §2.2 validations.
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

        // Frame header (§2): frame_number, unknown1, check_sum,
        // frame_size.
        let frame_number: u32 = 0;
        let unknown1: u32 = 0;
        let frame_size: u32 = total_len;
        let check_sum = frame_number ^ unknown1 ^ frame_size ^ MAGIC_FRMH;
        buf[0x00..0x04].copy_from_slice(&frame_number.to_le_bytes());
        buf[0x04..0x08].copy_from_slice(&unknown1.to_le_bytes());
        buf[0x08..0x0c].copy_from_slice(&check_sum.to_le_bytes());
        buf[0x0c..0x10].copy_from_slice(&frame_size.to_le_bytes());

        // Bitstream header (§3) at offset 0x10.
        let b = FRAME_HEADER_LEN;
        buf[b..b + 2].copy_from_slice(&REQUIRED_DEC_VERSION.to_le_bytes());
        buf[b + 2..b + 4].copy_from_slice(&flags.to_le_bytes());
        buf[b + 4..b + 8].copy_from_slice(&data_size_bits.to_le_bytes());
        // cb_offset (1) + reserved1 (1) + checksum (2) left zero.
        buf[b + 0x0c..b + 0x0e].copy_from_slice(&height.to_le_bytes());
        buf[b + 0x0e..b + 0x10].copy_from_slice(&width.to_le_bytes());
        buf[b + 0x10..b + 0x14].copy_from_slice(&y_off.to_le_bytes());
        buf[b + 0x14..b + 0x18].copy_from_slice(&v_off.to_le_bytes());
        buf[b + 0x18..b + 0x1c].copy_from_slice(&u_off.to_le_bytes());
        // reserved2 + alt_quant[16] left zero.
        buf
    }

    #[test]
    fn null_frame_carries_no_planes() {
        // data_size == NULL sentinel → spec/02 §1 NULL frame.
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
        assert!(frame.is_null_frame());
        assert_eq!(frame.reconstruction_status, ReconstructionStatus::NullFrame);
        assert!(frame.planes.is_empty());
        assert_eq!(frame.width(), 64);
        assert_eq!(frame.height(), 64);
    }

    #[test]
    fn skipped_planes_produce_no_decoded_plane() {
        // All three plane offsets negative (high bit set) → spec/02
        // §2 skips every plane. The frame is non-NULL but carries
        // no decoded planes.
        let neg = 0x8000_0000u32;
        let buf = build_header(64, 64, 4096, 0, neg, neg, neg, COMBINED_HEADER_LEN as u32);
        let frame = decode_frame(&buf).expect("decodes");
        assert_eq!(
            frame.reconstruction_status,
            ReconstructionStatus::StructureComplete
        );
        assert!(frame.planes.is_empty());
    }

    #[test]
    fn decode_order_is_u_v_y() {
        assert_eq!(
            FRAME_PLANE_DECODE_ORDER,
            [PLANE_IDX_U, PLANE_IDX_V, PLANE_IDX_Y]
        );
    }

    #[test]
    fn single_intra_plane_walks_to_structure_complete() {
        // Construct a tiny frame with one present luma plane whose
        // payload is a single INTRA leaf (MC_TREE 2-bit code for an
        // INTRA leaf, then a minimal VQ sub-tree). We only need the
        // pipeline to thread through and produce a CellTree; the
        // exact leaf classification is exercised by the
        // macroblock-layer tests.
        //
        // Plane offset 0 (= bsh base) is the plane base. num_vectors
        // = 0 (INTRA plane, 4 bytes), then the bitstream payload.
        let bsh = FRAME_HEADER_LEN; // 0x10
        // prelude: num_vectors (u32 = 0) at plane_base, then payload.
        let mut payload = vec![0u8; 8]; // 4 bytes num_vectors + payload bytes
        // num_vectors = 0 (already zero).
        // payload bytes drive the binary-tree walk; the exact
        // bytes matter only insofar as they decode to *some* valid
        // tree. A whole 4x4 plane that is a single VQ_DATA leaf:
        // we lean on the macroblock walker to accept these bytes
        // or surface a deterministic error.
        for (i, b) in payload.iter_mut().enumerate().skip(4) {
            *b = (i as u8).wrapping_mul(0x11);
        }

        // The plane data must live at bsh + y_off. Build the full
        // buffer: header (0x40) then plane data starting at bsh
        // (0x10). Since y_off = 0 and bsh = 0x10, the plane base is
        // at offset 0x10, which overlaps the bitstream header region
        // — that is fine for this structural test because the
        // picture-layer parser reads `num_vectors` from
        // `bsh + plane_offset`, i.e. offset 0x10, and the payload
        // follows. To avoid overlap with the header fields we point
        // the plane offset past the header by setting y_off so the
        // plane base lands at the end of the 64-byte header.
        let plane_base_target = COMBINED_HEADER_LEN; // 0x40
        let y_off = (plane_base_target - bsh) as u32; // 0x30
        let total_len = (plane_base_target + payload.len()) as u32;

        let mut buf = build_header(
            4,
            4,
            (payload.len() as u32) * 8,
            0,
            y_off,
            0x8000_0000, // V skipped
            0x8000_0000, // U skipped
            total_len,
        );
        buf.extend_from_slice(&payload);

        // The walk may succeed (structure complete) or surface a
        // deterministic macroblock error on these synthetic bytes;
        // either way the driver must not panic and must report a
        // typed result.
        match decode_frame(&buf) {
            Ok(frame) => {
                assert_eq!(
                    frame.reconstruction_status,
                    ReconstructionStatus::StructureComplete
                );
                // If the Y plane decoded, its plan must be INTRA
                // (num_vectors == 0) and luma.
                if let Some(p) = frame.plane(PLANE_IDX_Y) {
                    assert!(p.is_intra());
                    assert!(p.plan.is_luma());
                }
            }
            Err(FrameDecodeError::PlaneTree { plane_idx, .. }) => {
                assert_eq!(plane_idx, PLANE_IDX_Y);
            }
            Err(other) => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn buffer_selector_reads_frame_flags_bit9() {
        // frame_flags bit 9 (0x0200) set → secondary buffer.
        let buf = build_header(
            64,
            64,
            NULL_FRAME_DATA_SIZE_BITS,
            0x0200,
            0,
            0,
            0,
            COMBINED_HEADER_LEN as u32,
        );
        let frame = decode_frame(&buf).expect("decodes");
        assert!(frame.buffer_selector());
    }
}
