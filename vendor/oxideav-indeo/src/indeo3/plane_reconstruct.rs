//! Indeo 3 plane-level reconstruction-readiness classifier.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/03-macroblock-layer.md`
//! §3 / §4 (the per-plane cell tree) cross-referenced with
//! `spec/04-vq-codebooks.md` §3 / §4 (the VQ_DATA codebook-bank lookup
//! vs the VQ_NULL copy / mark sub-codes) and `spec/05` (INTER motion
//! compensation).
//!
//! ## What this module adds
//!
//! [`super::decode_frame`] resolves a frame's structure into a
//! [`super::DecodedPlane`] per present plane — a [`super::CellTree`] of
//! INTRA / INTER leaf cells (INTRA cells carrying their VQ sub-tree
//! inline). The earlier rounds could *classify* one leaf at a time, but
//! nothing surveyed a whole plane to answer the practical question a
//! reconstruction driver needs: **which cells can I reconstruct from the
//! genuinely-unblocked, on-disk / table-free paths, and which are gated
//! on the `spec/04 §7.1` codebook-bank docs-gap?**
//!
//! This module owns that survey. It walks a plane's cell tree and maps
//! every reconstruction unit to a [`CellDisposition`]:
//!
//! * **VQ_NULL copy** (`spec/04 §4` sub-code `00`) → [`CellDisposition::VqNullCopy`].
//!   Reconstructed by a literal upper-row copy ([`super::copy_upper_cell`]);
//!   needs **no table input** — fully unblocked.
//! * **VQ_NULL skip** (`spec/04 §4` sub-code `01`) → [`CellDisposition::VqNullSkip`].
//!   Reconstructed by the edge-marker write ([`super::mark_edge_cell`]);
//!   also table-free — fully unblocked.
//! * **VQ_DATA** (`spec/04 §3.1`) → [`CellDisposition::VqDataArena`].
//!   The leaf byte indexes `inner_instance[4*byte]` to fetch the packed
//!   codebook DWORD whose value is the `spec/04 §7.1` codebook-bank
//!   docs-gap (zero on disk; built at codec-init). Deferred.
//! * **INTER** (`spec/03 §3.4`) → [`CellDisposition::InterMc`]. Motion
//!   compensation (`spec/05`); the per-plane packed-MV table is decoded
//!   but the reference-buffer pixels need a prior decoded frame, so
//!   single-frame reconstruction defers these.
//!
//! The aggregate [`PlaneReconstructPlan`] then reports, per plane, how
//! many reconstruction units fall in each disposition — a measured view
//! of exactly how much of the plane the unblocked subset can cover and
//! how much waits on the docs-gap. This turns the structural decode into
//! an actionable reconstruction roadmap without guessing any gated
//! value.

use super::cell_null::{COPY_UPPER_ROW_COUNT, CopyUpperError, CopyUpperGeometry, copy_upper_cell};
use super::frame::DecodedPlane;
use super::macroblock::{Cell, CellTree, VqLeaf, VqNull};
use super::reconstruct::PREDICTOR_ROW_STRIDE;

/// How one reconstruction unit (a leaf cell, or a VQ sub-cell of an
/// INTRA cell) can be reconstructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellDisposition {
    /// VQ_NULL `00` — copy the upper-neighbour row (`spec/04 §4`,
    /// `spec/07 §1.4`). Table-free; reconstructable now via
    /// [`super::copy_upper_cell`].
    VqNullCopy,
    /// VQ_NULL `01` — mark the cell as a boundary / edge
    /// (`spec/04 §4`, `spec/07 §4.4`). Table-free; reconstructable now
    /// via [`super::mark_edge_cell`].
    VqNullSkip,
    /// VQ_NULL `1` — the one-bit "dispatch into the per-byte
    /// unpacker" sub-code (`spec/04 §4` / `§7.3`, `spec/06 §5.2`):
    /// the cell's data is a mode-byte stream read directly from the
    /// bitstream with no codebook-index byte. Gated on the same
    /// per-frame-arena docs-gap as VQ_DATA for its literal dyads
    /// (the static-table-only subset is drivable via
    /// [`super::reconstruct_cell_static`]).
    VqNullUnpacker,
    /// VQ_DATA — the leaf byte indexes the per-frame codebook arena
    /// (`spec/04 §3.1`). Gated on the `spec/04 §7.1` codebook-bank
    /// docs-gap.
    VqDataArena,
    /// INTER — motion compensation against a reference frame
    /// (`spec/03 §3.4`, `spec/05`). Needs a prior decoded frame.
    InterMc,
}

impl CellDisposition {
    /// `true` if this disposition can be reconstructed *now* from the
    /// genuinely-unblocked (table-free / on-disk) paths — i.e. the two
    /// VQ_NULL sub-codes. VQ_DATA waits on the codebook-bank docs-gap;
    /// INTER waits on a reference frame.
    pub fn is_unblocked(self) -> bool {
        matches!(
            self,
            CellDisposition::VqNullCopy | CellDisposition::VqNullSkip
        )
    }
}

/// One reconstruction unit's geometry + disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellPlanEntry {
    /// Top-left x in plane samples.
    pub x: u32,
    /// Top-left y in plane samples.
    pub y: u32,
    /// Width in plane samples.
    pub w: u32,
    /// Height in plane samples.
    pub h: u32,
    /// How this unit is reconstructed.
    pub disposition: CellDisposition,
    /// The absolute input-buffer offset of the unit's mode-byte
    /// stream, for data-bearing units
    /// ([`VqCell::data_cursor`](super::macroblock::VqCell::data_cursor)):
    /// byte-exact for the plane's *first* data-bearing unit, a lower
    /// bound for later ones (the structural walk does not consume
    /// arena-gated mode bytes). `None` for INTER / VQ_NULL copy /
    /// VQ_NULL skip units.
    pub data_cursor: Option<usize>,
}

/// Per-disposition reconstruction-unit counts for one plane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DispositionCounts {
    /// VQ_NULL copy cells (table-free; reconstructable now).
    pub vq_null_copy: usize,
    /// VQ_NULL skip cells (table-free; reconstructable now).
    pub vq_null_skip: usize,
    /// VQ_NULL unpacker-dispatch cells (`spec/06 §5.2` hybrid;
    /// mode-byte-stream driven, arena-gated for literal dyads).
    pub vq_null_unpacker: usize,
    /// VQ_DATA cells (codebook-bank docs-gap).
    pub vq_data_arena: usize,
    /// INTER cells (motion compensation; needs a reference frame).
    pub inter_mc: usize,
}

impl DispositionCounts {
    /// Total reconstruction units across all dispositions.
    pub fn total(&self) -> usize {
        self.vq_null_copy
            + self.vq_null_skip
            + self.vq_null_unpacker
            + self.vq_data_arena
            + self.inter_mc
    }

    /// Units reconstructable now from the unblocked (VQ_NULL) subset.
    pub fn unblocked(&self) -> usize {
        self.vq_null_copy + self.vq_null_skip
    }

    /// Units gated on a docs-gap or a reference frame (VQ_NULL
    /// unpacker-dispatch + VQ_DATA + INTER).
    pub fn deferred(&self) -> usize {
        self.vq_null_unpacker + self.vq_data_arena + self.inter_mc
    }

    fn record(&mut self, disposition: CellDisposition) {
        match disposition {
            CellDisposition::VqNullCopy => self.vq_null_copy += 1,
            CellDisposition::VqNullSkip => self.vq_null_skip += 1,
            CellDisposition::VqNullUnpacker => self.vq_null_unpacker += 1,
            CellDisposition::VqDataArena => self.vq_data_arena += 1,
            CellDisposition::InterMc => self.inter_mc += 1,
        }
    }
}

/// The reconstruction-readiness plan for one decoded plane: the
/// per-unit dispositions plus the aggregate counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneReconstructPlan {
    /// Spec/02 §2 plane index (0 = Y, 1 = V, 2 = U).
    pub plane_idx: usize,
    /// Plane width in samples (`spec/03 §3.1`).
    pub plane_width: u32,
    /// Plane height in samples.
    pub plane_height: u32,
    /// One entry per reconstruction unit, in cell-tree walk order
    /// (INTRA cells expand into their VQ sub-cells).
    pub entries: Vec<CellPlanEntry>,
    /// Aggregate per-disposition counts.
    pub counts: DispositionCounts,
}

impl PlaneReconstructPlan {
    /// `true` if every reconstruction unit in this plane is reconstructable
    /// now from the unblocked subset (no VQ_DATA / INTER units).
    pub fn is_fully_unblocked(&self) -> bool {
        self.counts.deferred() == 0 && self.counts.total() > 0
    }

    /// The plane's **first data-bearing unit** and its byte-exact
    /// mode-byte stream anchor (`spec/06 §5.1` / `§5.2`): the entry
    /// index plus the absolute input-buffer offset where its stream
    /// begins. Only the first such unit's anchor is exact (the
    /// structural walk does not consume arena-gated mode bytes, so
    /// later anchors are lower bounds); this is the seam an
    /// interleaved reconstruction walk starts from.
    pub fn first_data_anchor(&self) -> Option<(usize, usize)> {
        self.entries
            .iter()
            .enumerate()
            .find_map(|(i, e)| e.data_cursor.map(|c| (i, c)))
    }
}

/// Spec/03 + spec/04 §3 / §4 — classify a plane's cell tree into a
/// per-unit reconstruction-readiness plan.
///
/// Walks `tree.cells` in tree-walk order. An INTER leaf contributes one
/// [`CellDisposition::InterMc`] unit. An INTRA leaf expands into its VQ
/// sub-tree: each [`VqLeaf::Null`] sub-cell maps to
/// [`CellDisposition::VqNullCopy`] / [`CellDisposition::VqNullSkip`] and
/// each [`VqLeaf::Data`] sub-cell to [`CellDisposition::VqDataArena`].
pub fn classify_cell_tree(plane_idx: usize, tree: &CellTree) -> PlaneReconstructPlan {
    let mut entries = Vec::new();
    let mut counts = DispositionCounts::default();

    for cell in &tree.cells {
        match cell {
            Cell::Inter { x, y, w, h, .. } => {
                let disposition = CellDisposition::InterMc;
                counts.record(disposition);
                entries.push(CellPlanEntry {
                    x: *x,
                    y: *y,
                    w: *w,
                    h: *h,
                    disposition,
                    data_cursor: None,
                });
            }
            Cell::Intra { vq_leaves, .. } => {
                for vq in vq_leaves {
                    let disposition = match vq.leaf {
                        VqLeaf::Null(VqNull::Copy) => CellDisposition::VqNullCopy,
                        VqLeaf::Null(VqNull::Skip) => CellDisposition::VqNullSkip,
                        VqLeaf::Null(VqNull::Unpacker) => CellDisposition::VqNullUnpacker,
                        VqLeaf::Data { .. } => CellDisposition::VqDataArena,
                    };
                    counts.record(disposition);
                    entries.push(CellPlanEntry {
                        x: vq.x,
                        y: vq.y,
                        w: vq.w,
                        h: vq.h,
                        disposition,
                        data_cursor: vq.data_cursor,
                    });
                }
            }
        }
    }

    PlaneReconstructPlan {
        plane_idx,
        plane_width: tree.plane_width,
        plane_height: tree.plane_height,
        entries,
        counts,
    }
}

/// Convenience wrapper: classify a [`DecodedPlane`]'s tree.
pub fn classify_plane(plane: &DecodedPlane) -> PlaneReconstructPlan {
    classify_cell_tree(plane.plane_idx, &plane.tree)
}

/// Errors raised while driving the unblocked VQ_NULL cells of a plane
/// over a strip pixel buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlaneReconstructError {
    /// A VQ_NULL copy cell's [`copy_upper_cell`] drive failed. Carries
    /// the unit's plane coordinates and the underlying error.
    CopyUpper {
        /// Unit top-left x.
        x: u32,
        /// Unit top-left y.
        y: u32,
        /// The underlying copy-upper error.
        source: CopyUpperError,
    },
}

impl core::fmt::Display for PlaneReconstructError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PlaneReconstructError::CopyUpper { x, y, source } => {
                write!(f, "indeo3 plane: VQ_NULL copy cell at ({x}, {y}): {source}")
            }
        }
    }
}

impl std::error::Error for PlaneReconstructError {}

/// Outcome of driving a plane's unblocked VQ_NULL copy cells.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct VqNullDriveStats {
    /// VQ_NULL copy cells that were driven through [`copy_upper_cell`].
    pub copy_cells_driven: usize,
    /// VQ_NULL skip cells (their edge-marker write is left to the caller
    /// via [`super::mark_edge_cell`]; counted here for completeness).
    pub skip_cells: usize,
    /// Total pixel bytes written by the copy-upper drives.
    pub bytes_copied: usize,
}

/// Drive a plane's **VQ_NULL copy** units through [`copy_upper_cell`]
/// over a caller-supplied strip pixel buffer (`spec/07 §1.4`).
///
/// This is the genuinely-unblocked half of plane reconstruction: VQ_NULL
/// copy cells need only a literal upper-row copy, with no codebook-bank
/// table. The `strip` buffer is the per-plane accumulator
/// (`spec/07 §5.1`); its row stride is [`PREDICTOR_ROW_STRIDE`]. Each
/// copy cell's `top_left_offset` is derived from its `(x, y)` plane
/// coordinates as `y * 0xb0 + x`.
///
/// VQ_DATA, INTER, and VQ_NULL skip units are **not** driven (the first
/// two are gated; the skip body is the caller's [`super::mark_edge_cell`]
/// call). Returns the [`VqNullDriveStats`], or the first
/// [`PlaneReconstructError`] a copy-upper drive raises.
pub fn drive_vq_null_copies(
    strip: &mut [u8],
    plan: &PlaneReconstructPlan,
) -> Result<VqNullDriveStats, PlaneReconstructError> {
    let mut stats = VqNullDriveStats::default();

    for entry in &plan.entries {
        match entry.disposition {
            CellDisposition::VqNullCopy => {
                // The cell width in column groups (pixels / 4); the
                // copy-upper body works in 4-pixel column groups
                // (spec/07 §1.4). A cell width that is not a multiple of
                // 4 rounds up to cover the partial group.
                let width_dwords = (entry.w as usize).div_ceil(4);
                // The copy-upper body fills up to COPY_UPPER_ROW_COUNT
                // rows; a taller cell is covered in row_count-bounded
                // bands (here we drive the first band, matching the §1.4
                // single-invocation shape; multi-band tiling is the
                // caller's loop).
                let row_count = (entry.h as usize).clamp(1, COPY_UPPER_ROW_COUNT);
                let top_left_offset = entry.y as usize * PREDICTOR_ROW_STRIDE + entry.x as usize;
                let geometry = CopyUpperGeometry {
                    width_dwords,
                    row_count,
                    top_left_offset,
                };
                let cell_stats = copy_upper_cell(strip, geometry).map_err(|source| {
                    PlaneReconstructError::CopyUpper {
                        x: entry.x,
                        y: entry.y,
                        source,
                    }
                })?;
                stats.copy_cells_driven += 1;
                stats.bytes_copied += cell_stats.bytes_copied;
            }
            CellDisposition::VqNullSkip => stats.skip_cells += 1,
            CellDisposition::VqNullUnpacker
            | CellDisposition::VqDataArena
            | CellDisposition::InterMc => {}
        }
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::macroblock::{Cell, CellTree, VqCell, VqLeaf, VqNull};

    fn intra_cell(x: u32, y: u32, w: u32, h: u32, leaves: Vec<VqLeaf>) -> Cell {
        let vq_leaves = leaves
            .into_iter()
            .enumerate()
            .map(|(i, leaf)| VqCell {
                x: x + i as u32 * 4,
                y,
                w: 4,
                h,
                leaf,
                data_cursor: None,
            })
            .collect();
        Cell::Intra {
            x,
            y,
            w,
            h,
            vq_leaves,
        }
    }

    #[test]
    fn classify_counts_each_disposition() {
        let tree = CellTree {
            plane_width: 16,
            plane_height: 8,
            cells: vec![
                intra_cell(
                    0,
                    0,
                    8,
                    4,
                    vec![
                        VqLeaf::Null(VqNull::Copy),
                        VqLeaf::Null(VqNull::Skip),
                        VqLeaf::Data { codebook_index: 7 },
                    ],
                ),
                Cell::Inter {
                    x: 8,
                    y: 0,
                    w: 8,
                    h: 4,
                    mv_index: 3,
                },
            ],
        };
        let plan = classify_cell_tree(0, &tree);
        assert_eq!(plan.counts.vq_null_copy, 1);
        assert_eq!(plan.counts.vq_null_skip, 1);
        assert_eq!(plan.counts.vq_data_arena, 1);
        assert_eq!(plan.counts.inter_mc, 1);
        assert_eq!(plan.counts.total(), 4);
        assert_eq!(plan.counts.unblocked(), 2);
        assert_eq!(plan.counts.deferred(), 2);
        assert_eq!(plan.entries.len(), 4);
        assert!(!plan.is_fully_unblocked());
    }

    #[test]
    fn first_data_anchor_finds_the_first_data_bearing_unit() {
        // Two VQ_NULL units (no anchor), then a VQ_DATA unit with a
        // recorded stream anchor, then another data unit — the plan
        // reports the FIRST anchor only (later ones are lower
        // bounds; spec/06 §5.1).
        let mk = |leaf, cursor| VqCell {
            x: 0,
            y: 0,
            w: 4,
            h: 4,
            leaf,
            data_cursor: cursor,
        };
        let tree = CellTree {
            plane_width: 16,
            plane_height: 4,
            cells: vec![Cell::Intra {
                x: 0,
                y: 0,
                w: 16,
                h: 4,
                vq_leaves: vec![
                    mk(VqLeaf::Null(VqNull::Copy), None),
                    mk(VqLeaf::Null(VqNull::Unpacker), Some(9)),
                    mk(VqLeaf::Data { codebook_index: 1 }, Some(11)),
                ],
            }],
        };
        let plan = classify_cell_tree(0, &tree);
        assert_eq!(plan.counts.vq_null_unpacker, 1);
        assert_eq!(plan.first_data_anchor(), Some((1, 9)));
        assert_eq!(plan.entries[2].data_cursor, Some(11));

        // A plane with no data-bearing units has no anchor.
        let tree = CellTree {
            plane_width: 8,
            plane_height: 4,
            cells: vec![intra_cell(0, 0, 8, 4, vec![VqLeaf::Null(VqNull::Copy)])],
        };
        assert_eq!(classify_cell_tree(0, &tree).first_data_anchor(), None);
    }

    #[test]
    fn all_vq_null_plane_is_fully_unblocked() {
        let tree = CellTree {
            plane_width: 8,
            plane_height: 4,
            cells: vec![intra_cell(
                0,
                0,
                8,
                4,
                vec![VqLeaf::Null(VqNull::Copy), VqLeaf::Null(VqNull::Skip)],
            )],
        };
        let plan = classify_cell_tree(0, &tree);
        assert!(plan.is_fully_unblocked());
        assert_eq!(plan.counts.deferred(), 0);
    }

    #[test]
    fn disposition_unblocked_flag() {
        assert!(CellDisposition::VqNullCopy.is_unblocked());
        assert!(CellDisposition::VqNullSkip.is_unblocked());
        assert!(!CellDisposition::VqDataArena.is_unblocked());
        assert!(!CellDisposition::InterMc.is_unblocked());
    }

    #[test]
    fn drive_vq_null_copies_fills_strip_from_upper_row() {
        // One VQ_NULL copy cell at (0, 4): its top-left is at byte
        // 4*0xb0; the upper-neighbour row (row 3) is seeded with a known
        // pattern that the copy must replicate downward.
        let stride = PREDICTOR_ROW_STRIDE;
        let mut strip = vec![0u8; stride * 12];
        // Seed the row directly above the cell (row 3) with a pattern.
        let upper_row_start = 3 * stride;
        for (i, b) in strip[upper_row_start..upper_row_start + 8]
            .iter_mut()
            .enumerate()
        {
            *b = 0x40 + i as u8;
        }
        let tree = CellTree {
            plane_width: 8,
            plane_height: 8,
            cells: vec![intra_cell(0, 4, 8, 4, vec![VqLeaf::Null(VqNull::Copy)])],
        };
        // The single VqCell spans x=0,w=4 by the helper; widen the leaf
        // by classifying then driving. The helper makes a 4-wide sub-cell.
        let plan = classify_cell_tree(0, &tree);
        let stats = drive_vq_null_copies(&mut strip, &plan).expect("drive");
        assert_eq!(stats.copy_cells_driven, 1);
        assert!(stats.bytes_copied > 0);
        // The cell's first row (row 4) now equals the upper row (row 3).
        let cell_row_start = 4 * stride;
        assert_eq!(
            &strip[cell_row_start..cell_row_start + 4],
            &strip[upper_row_start..upper_row_start + 4]
        );
    }

    #[test]
    fn drive_reports_skip_cells_without_writing() {
        let stride = PREDICTOR_ROW_STRIDE;
        let mut strip = vec![0u8; stride * 8];
        let tree = CellTree {
            plane_width: 4,
            plane_height: 4,
            cells: vec![intra_cell(0, 0, 4, 4, vec![VqLeaf::Null(VqNull::Skip)])],
        };
        let plan = classify_cell_tree(0, &tree);
        let stats = drive_vq_null_copies(&mut strip, &plan).expect("drive");
        assert_eq!(stats.skip_cells, 1);
        assert_eq!(stats.copy_cells_driven, 0);
        assert_eq!(stats.bytes_copied, 0);
    }
}
