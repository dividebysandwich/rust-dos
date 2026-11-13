//! Indeo 3 plane-level reconstruction executor: the cell-tree-walking
//! driver that turns a [`super::PlaneReconstructPlan`] into an
//! actually-mutated strip pixel buffer.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/07-output-reconstruction.md`
//! §1.4 (VQ_NULL copy-upper), §4.4 (mark-edge), §5.1 (the per-plane
//! strip pixel-buffer accumulator) cross-referenced with `spec/04 §4`
//! (the VQ_NULL sub-codes) and `spec/04 §3.1` / §7.1 (the VQ_DATA
//! arena gate).
//!
//! ## What this module adds
//!
//! The earlier rounds landed each reconstruction primitive in
//! isolation:
//!
//! * [`super::classify_cell_tree`] surveys a plane's cell tree into a
//!   per-unit [`super::PlaneReconstructPlan`] (which unit is VQ_NULL
//!   copy / skip / VQ_DATA / INTER).
//! * [`super::copy_upper_cell`] / [`super::mark_edge_cell`] each execute
//!   *one* VQ_NULL cell over a caller-supplied strip buffer.
//! * [`super::drive_vq_null_copies`] drove only the **copy** subset of a
//!   plan, leaving the mark-edge skip cells, the strip-buffer sizing,
//!   and the deferred-frontier bookkeeping to the caller.
//!
//! This module owns the **whole-plane executor**: given a
//! [`super::PlaneReconstructPlan`] it sizes a single-strip pixel buffer
//! from the plane geometry (`spec/07 §5.1`: row stride
//! [`PREDICTOR_ROW_STRIDE`] = `0xb0`), walks every reconstruction unit
//! in plan order, and dispatches each to its disposition's executor:
//!
//! * **VQ_NULL copy** → [`super::copy_upper_cell`] (a literal
//!   upper-row copy; `spec/07 §1.4`). Table-free.
//! * **VQ_NULL skip** → [`super::mark_edge_cell`] (the §4.4 bit-7
//!   edge-marker write). Table-free.
//! * **VQ_DATA** → recorded as the **first deferred frontier** if no
//!   earlier deferral was seen, then skipped. The leaf indexes the
//!   per-frame codebook arena (`spec/04 §3.1`) whose values are the
//!   `spec/04 §7.1` codebook-bank docs-gap (the §5.2 seed-window
//!   block-format contradiction; see [`super::CodebookSeedArea`]).
//! * **INTER** → likewise recorded / skipped (motion compensation needs
//!   a prior decoded frame; `spec/05`).
//!
//! The result is a [`ReconstructedPlane`]: the mutated strip buffer
//! plus a [`PlaneExecStats`] coverage report — how many units (and
//! pixel bytes) the unblocked subset reconstructed, how many were
//! deferred, and the exact `(x, y, disposition)` of the first deferred
//! unit. This is the genuinely-unblocked depth of single-frame
//! reconstruction: every VQ_NULL unit in a plane is materialised into
//! real strip pixels, and the precise codebook-bank boundary is
//! surfaced for the next Extractor round rather than guessed.
//!
//! ## Why a single strip
//!
//! `spec/07 §5.1` allocates one strip pixel buffer per *strip*, and a
//! plane may decompose into several strips (`spec/02 §4.1`). The cell
//! tree this executor walks is the whole-plane tree produced by
//! [`super::decode_plane_tree`], whose cell coordinates are plane-global
//! and whose row stride is the fixed `0xb0`. We therefore drive a
//! single plane-spanning strip buffer (`plane_height` rows of `0xb0`
//! bytes) so cross-cell predictor continuity (`spec/07 §1.3`) holds
//! across the whole plane in one buffer. The multi-strip *output*
//! tiling stays with [`super::assemble_output`]; this module's job is
//! the pixel synthesis, not the strip-to-frame assembly.

use super::cell_null::{
    COPY_UPPER_ROW_COUNT, CopyUpperError, CopyUpperGeometry, MarkEdgeError, MarkEdgeGeometry,
    copy_upper_cell, mark_edge_cell,
};
use super::cell_reconstruct::{
    CellOutcome, CellReconstructError, CellReconstructGeometry, PositionEffect,
    reconstruct_cell_stateful,
};
use super::plane_reconstruct::{CellDisposition, CellPlanEntry, PlaneReconstructPlan};
use super::reconstruct::PREDICTOR_ROW_STRIDE;
use super::vq::DyadDeltaTable;

/// The per-plane strip pixel-buffer row stride (`spec/07 §0 / §5.1`):
/// `0xb0` (176) bytes, aliasing [`PREDICTOR_ROW_STRIDE`] so the two
/// never drift.
pub const STRIP_ROW_STRIDE: usize = PREDICTOR_ROW_STRIDE;

const _: () = assert!(STRIP_ROW_STRIDE == 0xb0);

/// Errors raised while executing a plane's reconstruction plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlaneExecError {
    /// The plane geometry is degenerate: zero width or zero height
    /// (`spec/02 §4.1` rejects a zero-sized plane).
    ZeroGeometry {
        /// `true` if the width is zero, `false` if the height is zero.
        is_width: bool,
    },
    /// A unit's `x` (plus its width) exceeds the `0xb0`-byte strip row
    /// stride — a malformed plan whose cell would overrun the row.
    /// Carries the offending unit's plane coordinates and its right
    /// edge in bytes.
    UnitExceedsRowStride {
        /// Unit top-left x (plane samples).
        x: u32,
        /// Unit top-left y (plane samples).
        y: u32,
        /// The unit's right edge in bytes (`x + w`).
        right_edge: u32,
    },
    /// A VQ_NULL copy unit's [`copy_upper_cell`] drive failed. Carries
    /// the unit's plane coordinates and the underlying error.
    CopyUpper {
        /// Unit top-left x.
        x: u32,
        /// Unit top-left y.
        y: u32,
        /// The underlying copy-upper error.
        source: CopyUpperError,
    },
    /// A VQ_NULL skip unit's [`mark_edge_cell`] drive failed. Carries
    /// the unit's plane coordinates and the underlying error.
    MarkEdge {
        /// Unit top-left x.
        x: u32,
        /// Unit top-left y.
        y: u32,
        /// The underlying mark-edge error.
        source: MarkEdgeError,
    },
    /// An unpacker-dispatch unit's static-subset drive faulted (the
    /// binary's typed error-code-1 / stream-exhaustion faults;
    /// [`exec_plane_plan_with_stream`] only).
    UnpackerCell(UnpackerCellFault),
}

impl core::fmt::Display for PlaneExecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PlaneExecError::ZeroGeometry { is_width } => write!(
                f,
                "indeo3 plane-exec: plane {} is zero",
                if *is_width { "width" } else { "height" }
            ),
            PlaneExecError::UnitExceedsRowStride { x, y, right_edge } => write!(
                f,
                "indeo3 plane-exec: unit at ({x}, {y}) right edge {right_edge} exceeds row stride {STRIP_ROW_STRIDE}"
            ),
            PlaneExecError::CopyUpper { x, y, source } => {
                write!(f, "indeo3 plane-exec: VQ_NULL copy at ({x}, {y}): {source}")
            }
            PlaneExecError::MarkEdge { x, y, source } => {
                write!(f, "indeo3 plane-exec: VQ_NULL skip at ({x}, {y}): {source}")
            }
            PlaneExecError::UnpackerCell(fault) => write!(
                f,
                "indeo3 plane-exec: VQ_NULL unpacker cell at ({}, {}): {}",
                fault.x, fault.y, fault.source
            ),
        }
    }
}

impl std::error::Error for PlaneExecError {}

/// An unpacker-dispatch cell's static-subset drive failed with the
/// binary's typed fault (`spec/06 §4.3` error-code-1 escape fault,
/// stream exhaustion, or an out-of-range seed write), tagged with the
/// unit's plane coordinates. Surfaced by
/// [`exec_plane_plan_with_stream`] via
/// [`PlaneExecError::UnpackerCell`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnpackerCellFault {
    /// Unit top-left x (plane samples).
    pub x: u32,
    /// Unit top-left y (plane samples).
    pub y: u32,
    /// The underlying executor error.
    pub source: CellReconstructError,
}

/// The reconstruction frontier: the first reconstruction unit the
/// executor had to defer (VQ_DATA on the codebook-bank docs-gap, or
/// INTER on a missing reference frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeferredFrontier {
    /// Unit top-left x (plane samples).
    pub x: u32,
    /// Unit top-left y (plane samples).
    pub y: u32,
    /// Why the unit was deferred (VQ_DATA arena or INTER).
    pub disposition: CellDisposition,
    /// The plan-entry index (0-based) of the deferred unit.
    pub entry_index: usize,
}

/// Per-plane reconstruction coverage after executing a plan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlaneExecStats {
    /// VQ_NULL copy units reconstructed through [`copy_upper_cell`].
    pub copy_units: usize,
    /// VQ_NULL skip units reconstructed through [`mark_edge_cell`].
    pub skip_units: usize,
    /// VQ_NULL unpacker-dispatch units whose **static-table subset**
    /// was driven with real bitstream bytes through
    /// [`reconstruct_cell_stateful`] (`spec/06 §5.2` + the
    /// [`CellPlanEntry::data_cursor`] anchor) and completed without
    /// hitting an arena-gated literal.
    pub unpacker_static_units: usize,
    /// VQ_NULL unpacker-dispatch units deferred (`spec/06 §5.2`
    /// hybrid; arena-gated mode-byte stream, or no byte-exact
    /// stream anchor for the unit).
    pub vq_null_unpacker_deferred: usize,
    /// VQ_DATA units deferred (codebook-bank docs-gap).
    pub vq_data_deferred: usize,
    /// INTER units deferred (needs a reference frame).
    pub inter_deferred: usize,
    /// Total pixel bytes the unblocked subset wrote into the strip
    /// (copy-upper copies + mark-edge marks).
    pub bytes_written: usize,
}

impl PlaneExecStats {
    /// Units reconstructed now (VQ_NULL copy + skip + the
    /// static-subset unpacker cells).
    pub fn reconstructed(&self) -> usize {
        self.copy_units + self.skip_units + self.unpacker_static_units
    }

    /// Units deferred (VQ_DATA + INTER).
    pub fn deferred(&self) -> usize {
        self.vq_null_unpacker_deferred + self.vq_data_deferred + self.inter_deferred
    }

    /// Total units the executor visited.
    pub fn total(&self) -> usize {
        self.reconstructed() + self.deferred()
    }

    /// `true` if every unit was reconstructed (no deferrals) and the
    /// plane carried at least one unit.
    pub fn is_fully_reconstructed(&self) -> bool {
        self.deferred() == 0 && self.total() > 0
    }
}

/// The result of executing a plane's reconstruction plan: the mutated
/// strip pixel buffer plus the coverage report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconstructedPlane {
    /// Spec/02 §2 plane index (0 = Y, 1 = V, 2 = U).
    pub plane_idx: usize,
    /// Plane width in samples.
    pub plane_width: u32,
    /// Plane height in samples.
    pub plane_height: u32,
    /// The plane-spanning strip pixel buffer (`plane_height` rows of
    /// [`STRIP_ROW_STRIDE`] bytes), with every VQ_NULL unit's pixels
    /// materialised. VQ_DATA / INTER regions stay zero.
    pub strip: Vec<u8>,
    /// Coverage statistics.
    pub stats: PlaneExecStats,
    /// The first deferred unit, or `None` if the plane was fully
    /// reconstructed from the unblocked subset.
    pub frontier: Option<DeferredFrontier>,
}

impl ReconstructedPlane {
    /// Borrow the strip pixel-buffer row at plane-row `y` (the visible
    /// `plane_width` bytes), or `None` if `y` is out of range.
    pub fn strip_row(&self, y: u32) -> Option<&[u8]> {
        if y >= self.plane_height {
            return None;
        }
        let start = y as usize * STRIP_ROW_STRIDE;
        let w = self.plane_width as usize;
        self.strip.get(start..start + w)
    }
}

/// The byte length of a plane-spanning strip buffer for the given
/// height (`plane_height` rows of [`STRIP_ROW_STRIDE`] bytes).
///
/// One extra row is **not** added: the top-of-strip predictor for
/// row-0 cells reads the codec-init zero-fill (`spec/07 §1.3`), and the
/// classifier never emits a copy-upper unit at `y == 0` (a copy-upper
/// at the top of the strip is the `spec/07 §1.4`
/// upper-neighbour-above-buffer malformed case the executor rejects via
/// [`copy_upper_cell`]).
pub fn plane_strip_len(plane_height: u32) -> usize {
    plane_height as usize * STRIP_ROW_STRIDE
}

/// The byte offset of a unit's top-left pixel within the strip buffer:
/// `y * 0xb0 + x` (`spec/07 §1.1` strip-pixel addressing).
fn unit_top_left_offset(entry: &CellPlanEntry) -> usize {
    entry.y as usize * STRIP_ROW_STRIDE + entry.x as usize
}

/// The unit's width in column groups (pixels / 4), rounded up so a
/// width that is not a multiple of 4 still covers its partial group
/// (`spec/07 §1.4` walks 4-pixel column groups).
fn unit_width_dwords(entry: &CellPlanEntry) -> usize {
    (entry.w as usize).div_ceil(4)
}

/// The unit's row count, clamped to the `spec/07 §1.4`
/// [`COPY_UPPER_ROW_COUNT`] band height (the VQ_NULL executors fill at
/// most four rows per invocation; taller VQ_NULL cells are emitted as
/// stacked four-row bands, but the classifier's cell heights are
/// `N ∈ {4, 8}` and a height of 8 is two bands — handled by the
/// per-band loop in [`exec_plane_plan`]).
fn unit_band_rows(remaining_h: u32) -> usize {
    (remaining_h as usize).clamp(1, COPY_UPPER_ROW_COUNT)
}

/// Spec/07 §1.4 / §4.4 + §5.1 — execute a plane's reconstruction plan
/// into a freshly-allocated strip pixel buffer.
///
/// Allocates a `plane_height × 0xb0` strip buffer (zero-filled, the
/// `spec/07 §1.3` top-of-strip seed), walks the plan's units in order,
/// and drives each VQ_NULL unit through its executor:
///
/// * [`CellDisposition::VqNullCopy`] → [`copy_upper_cell`], filling up
///   to four rows per band (an 8-row cell is two bands).
/// * [`CellDisposition::VqNullSkip`] → [`mark_edge_cell`], or-setting
///   bit 7 over the cell's own bytes.
/// * [`CellDisposition::VqNullUnpacker`] / [`CellDisposition::VqDataArena`]
///   / [`CellDisposition::InterMc`] → counted and recorded as the
///   deferred frontier (first occurrence), then skipped.
///
/// Returns a [`ReconstructedPlane`] with the mutated strip and coverage
/// stats, or the first [`PlaneExecError`] an executor raises.
pub fn exec_plane_plan(plan: &PlaneReconstructPlan) -> Result<ReconstructedPlane, PlaneExecError> {
    exec_plane_plan_inner(plan, None)
}

/// Spec/06 §5.2 + `spec/07 §5.1` — [`exec_plane_plan`] with the plane's
/// **input bitstream** attached, so unpacker-dispatch VQ_NULL cells with
/// a byte-exact stream anchor can drive their static-table subset with
/// real bitstream bytes.
///
/// Only the plan's *first* data-bearing unit has a byte-exact anchor
/// ([`PlaneReconstructPlan::first_data_anchor`]) — later anchors are
/// lower bounds because the structural walk cannot consume arena-gated
/// mode bytes — so exactly that unit (when it is an unpacker-dispatch
/// cell) is driven through [`reconstruct_cell_stateful`]:
///
/// * a static-subset completion (all escapes / high-nibble-0 literals)
///   reconstructs the cell for real
///   ([`PlaneExecStats::unpacker_static_units`]);
/// * an arena-gated literal defers it to the frontier exactly as
///   before;
/// * the binary's typed faults (mis-positioned escape, stream
///   exhaustion) surface as [`PlaneExecError::UnpackerCell`].
///
/// Every other unit behaves exactly as in [`exec_plane_plan`].
pub fn exec_plane_plan_with_stream(
    plan: &PlaneReconstructPlan,
    input: &[u8],
    table: &DyadDeltaTable,
) -> Result<ReconstructedPlane, PlaneExecError> {
    exec_plane_plan_inner(plan, Some((input, table)))
}

fn exec_plane_plan_inner(
    plan: &PlaneReconstructPlan,
    stream: Option<(&[u8], &DyadDeltaTable)>,
) -> Result<ReconstructedPlane, PlaneExecError> {
    if plan.plane_width == 0 {
        return Err(PlaneExecError::ZeroGeometry { is_width: true });
    }
    if plan.plane_height == 0 {
        return Err(PlaneExecError::ZeroGeometry { is_width: false });
    }

    let mut strip = vec![0u8; plane_strip_len(plan.plane_height)];
    let mut stats = PlaneExecStats::default();
    let mut frontier: Option<DeferredFrontier> = None;
    // The single unit whose stream anchor is byte-exact (spec/06
    // §5.1: the first data-bearing unit of the plane).
    let exact_anchor = plan.first_data_anchor();

    for (entry_index, entry) in plan.entries.iter().enumerate() {
        let right_edge = entry.x.saturating_add(entry.w);
        if (right_edge as usize) > STRIP_ROW_STRIDE {
            return Err(PlaneExecError::UnitExceedsRowStride {
                x: entry.x,
                y: entry.y,
                right_edge,
            });
        }

        match entry.disposition {
            CellDisposition::VqNullCopy => {
                exec_copy_unit(&mut strip, entry, &mut stats)?;
                stats.copy_units += 1;
            }
            CellDisposition::VqNullSkip => {
                exec_skip_unit(&mut strip, entry, &mut stats)?;
                stats.skip_units += 1;
            }
            CellDisposition::VqNullUnpacker => {
                // spec/06 §5.2 — the hybrid "VQ-data without
                // leaf-byte" cell. With the input stream attached
                // and a byte-exact anchor, drive the static-table
                // subset; otherwise (or on an arena-gated literal)
                // defer to the frontier like VQ_DATA.
                let driven = match (stream, exact_anchor) {
                    (Some((input, table)), Some((anchor_index, cursor)))
                        if anchor_index == entry_index && cursor <= input.len() =>
                    {
                        Some(exec_unpacker_unit(
                            &mut strip,
                            entry,
                            &input[cursor..],
                            table,
                            &mut stats,
                        )?)
                    }
                    _ => None,
                };
                if !matches!(driven, Some(true)) {
                    stats.vq_null_unpacker_deferred += 1;
                    record_frontier(&mut frontier, entry, entry_index);
                }
            }
            CellDisposition::VqDataArena => {
                stats.vq_data_deferred += 1;
                record_frontier(&mut frontier, entry, entry_index);
            }
            CellDisposition::InterMc => {
                stats.inter_deferred += 1;
                record_frontier(&mut frontier, entry, entry_index);
            }
        }
    }

    Ok(ReconstructedPlane {
        plane_idx: plan.plane_idx,
        plane_width: plan.plane_width,
        plane_height: plan.plane_height,
        strip,
        stats,
        frontier,
    })
}

/// Drive one unpacker-dispatch unit's static-table subset over the
/// strip with real stream bytes. Returns `Ok(true)` when the cell
/// resolved (complete / terminated / consumed-by-carry) and
/// `Ok(false)` when it hit an arena-gated literal (the caller defers
/// it); typed executor faults map to
/// [`PlaneExecError::UnpackerCell`].
fn exec_unpacker_unit(
    strip: &mut [u8],
    entry: &CellPlanEntry,
    stream: &[u8],
    table: &DyadDeltaTable,
    stats: &mut PlaneExecStats,
) -> Result<bool, PlaneExecError> {
    let geometry = CellReconstructGeometry {
        width_dwords: (entry.w as usize).div_ceil(4),
        source_rows: (entry.h as usize).max(1),
        top_left_offset: entry.y as usize * STRIP_ROW_STRIDE + entry.x as usize,
    };
    let run =
        reconstruct_cell_stateful(strip, geometry, stream, table, false).map_err(|source| {
            PlaneExecError::UnpackerCell(UnpackerCellFault {
                x: entry.x,
                y: entry.y,
                source,
            })
        })?;
    match run.outcome {
        CellOutcome::Complete(effects) => {
            stats.unpacker_static_units += 1;
            // Row-band seeds write one predictor byte each
            // (spec/07 §3.2).
            stats.bytes_written += effects
                .iter()
                .filter(|e| matches!(e, PositionEffect::RowBandSeed { .. }))
                .count();
            Ok(true)
        }
        CellOutcome::SkippedByCarry => {
            stats.unpacker_static_units += 1;
            Ok(true)
        }
        CellOutcome::DeferredArena { .. } => Ok(false),
    }
}

/// Record the first deferred unit as the reconstruction frontier;
/// later deferrals leave the frontier unchanged (it marks where the
/// unblocked path first stops).
fn record_frontier(frontier: &mut Option<DeferredFrontier>, entry: &CellPlanEntry, idx: usize) {
    if frontier.is_none() {
        *frontier = Some(DeferredFrontier {
            x: entry.x,
            y: entry.y,
            disposition: entry.disposition,
            entry_index: idx,
        });
    }
}

/// Drive one VQ_NULL copy unit through [`copy_upper_cell`], one
/// four-row band at a time (an 8-row cell is two bands).
fn exec_copy_unit(
    strip: &mut [u8],
    entry: &CellPlanEntry,
    stats: &mut PlaneExecStats,
) -> Result<(), PlaneExecError> {
    let width_dwords = unit_width_dwords(entry);
    let mut row = 0u32;
    while row < entry.h {
        let band_rows = unit_band_rows(entry.h - row);
        let band_entry = CellPlanEntry {
            y: entry.y + row,
            ..*entry
        };
        let geometry = CopyUpperGeometry {
            width_dwords,
            row_count: band_rows,
            top_left_offset: unit_top_left_offset(&band_entry),
        };
        let band_stats =
            copy_upper_cell(strip, geometry).map_err(|source| PlaneExecError::CopyUpper {
                x: entry.x,
                y: band_entry.y,
                source,
            })?;
        stats.bytes_written += band_stats.bytes_copied;
        row += band_rows as u32;
    }
    Ok(())
}

/// Drive one VQ_NULL skip unit through [`mark_edge_cell`], one
/// four-row band at a time.
fn exec_skip_unit(
    strip: &mut [u8],
    entry: &CellPlanEntry,
    stats: &mut PlaneExecStats,
) -> Result<(), PlaneExecError> {
    let width_dwords = unit_width_dwords(entry);
    let mut row = 0u32;
    while row < entry.h {
        let band_rows = unit_band_rows(entry.h - row);
        let band_entry = CellPlanEntry {
            y: entry.y + row,
            ..*entry
        };
        let geometry = MarkEdgeGeometry {
            width_dwords,
            row_count: band_rows,
            top_left_offset: unit_top_left_offset(&band_entry),
        };
        let band_stats =
            mark_edge_cell(strip, geometry).map_err(|source| PlaneExecError::MarkEdge {
                x: entry.x,
                y: band_entry.y,
                source,
            })?;
        stats.bytes_written += band_stats.bytes_marked;
        row += band_rows as u32;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::macroblock::{Cell, CellTree, VqCell, VqLeaf, VqNull};
    use crate::indeo3::plane_reconstruct::classify_cell_tree;
    use crate::indeo3::reconstruct::EDGE_MARKER_BIT;

    fn intra_cell(x: u32, y: u32, w: u32, h: u32, leaves: Vec<(u32, VqLeaf)>) -> Cell {
        let vq_leaves = leaves
            .into_iter()
            .map(|(lx, leaf)| VqCell {
                x: lx,
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
    fn strip_len_is_height_times_stride() {
        assert_eq!(plane_strip_len(4), 4 * 0xb0);
        assert_eq!(plane_strip_len(0), 0);
    }

    #[test]
    fn copy_unit_fills_from_upper_row() {
        // One VQ_NULL copy cell at (0, 4), 4 wide, 4 tall: its upper
        // neighbour (row 3) is seeded and the copy must replicate it.
        let tree = CellTree {
            plane_width: 4,
            plane_height: 8,
            cells: vec![intra_cell(
                0,
                4,
                4,
                4,
                vec![(0, VqLeaf::Null(VqNull::Copy))],
            )],
        };
        let plan = classify_cell_tree(0, &tree);
        let entry = plan.entries[0];
        // Seed row 3 (the upper neighbour) of a freshly-sized strip.
        let mut strip = vec![0u8; plane_strip_len(plan.plane_height)];
        let upper = 3 * STRIP_ROW_STRIDE;
        for (i, b) in strip[upper..upper + 4].iter_mut().enumerate() {
            *b = 0x50 + i as u8;
        }
        // Drive the copy over the seeded buffer via the unit path.
        let mut stats = PlaneExecStats::default();
        exec_copy_unit(&mut strip, &entry, &mut stats).expect("copy");
        let cell_row = 4 * STRIP_ROW_STRIDE;
        assert_eq!(&strip[cell_row..cell_row + 4], &strip[upper..upper + 4]);
        assert!(stats.bytes_written > 0);
    }

    #[test]
    fn skip_unit_sets_edge_marker_bit() {
        let tree = CellTree {
            plane_width: 4,
            plane_height: 4,
            cells: vec![intra_cell(
                0,
                0,
                4,
                4,
                vec![(0, VqLeaf::Null(VqNull::Skip))],
            )],
        };
        let plan = classify_cell_tree(0, &tree);
        let recon = exec_plane_plan(&plan).expect("exec");
        assert_eq!(recon.stats.skip_units, 1);
        assert_eq!(recon.stats.copy_units, 0);
        // The cell's bytes carry bit 7 set.
        assert_eq!(recon.strip[0] & EDGE_MARKER_BIT, EDGE_MARKER_BIT);
        assert!(recon.stats.is_fully_reconstructed());
        assert!(recon.frontier.is_none());
    }

    #[test]
    fn vq_data_unit_is_deferred_with_frontier() {
        let tree = CellTree {
            plane_width: 8,
            plane_height: 4,
            cells: vec![intra_cell(
                0,
                0,
                8,
                4,
                vec![
                    (0, VqLeaf::Null(VqNull::Skip)),
                    (4, VqLeaf::Data { codebook_index: 9 }),
                ],
            )],
        };
        let plan = classify_cell_tree(0, &tree);
        let recon = exec_plane_plan(&plan).expect("exec");
        assert_eq!(recon.stats.skip_units, 1);
        assert_eq!(recon.stats.vq_data_deferred, 1);
        assert!(!recon.stats.is_fully_reconstructed());
        let frontier = recon.frontier.expect("frontier set");
        assert_eq!(frontier.disposition, CellDisposition::VqDataArena);
        assert_eq!(frontier.x, 4);
        assert_eq!(frontier.entry_index, 1);
    }

    #[test]
    fn inter_unit_is_deferred() {
        let tree = CellTree {
            plane_width: 8,
            plane_height: 4,
            cells: vec![Cell::Inter {
                x: 0,
                y: 0,
                w: 8,
                h: 4,
                mv_index: 2,
            }],
        };
        let plan = classify_cell_tree(0, &tree);
        let recon = exec_plane_plan(&plan).expect("exec");
        assert_eq!(recon.stats.inter_deferred, 1);
        let frontier = recon.frontier.expect("frontier");
        assert_eq!(frontier.disposition, CellDisposition::InterMc);
    }

    #[test]
    fn eight_row_copy_cell_drives_two_bands() {
        // An 8-row VQ_NULL copy cell at (0, 8) is two four-row bands.
        let tree = CellTree {
            plane_width: 4,
            plane_height: 16,
            cells: vec![intra_cell(
                0,
                8,
                4,
                8,
                vec![(0, VqLeaf::Null(VqNull::Copy))],
            )],
        };
        let plan = classify_cell_tree(0, &tree);
        let recon = exec_plane_plan(&plan).expect("exec");
        assert_eq!(recon.stats.copy_units, 1);
        // 8 rows × 1 column-group × 4 bytes = 32 bytes written.
        assert_eq!(recon.stats.bytes_written, 8 * 4);
    }

    #[test]
    fn zero_geometry_is_rejected() {
        let tree = CellTree {
            plane_width: 0,
            plane_height: 4,
            cells: vec![],
        };
        let plan = classify_cell_tree(0, &tree);
        assert_eq!(
            exec_plane_plan(&plan),
            Err(PlaneExecError::ZeroGeometry { is_width: true })
        );
    }

    #[test]
    fn stream_drive_resolves_first_unpacker_unit() {
        use crate::indeo3::macroblock::{Cell, CellTree, VqCell, VqLeaf, VqNull};
        use crate::indeo3::vq::DyadDeltaTable;

        // A plane with one unpacker-dispatch cell whose stream anchor
        // points at a 0xFD escape (skip all remaining rows) — a
        // static-subset cell that resolves with real bytes.
        let input = [0u8, 0u8, 0xFD, 0u8];
        let tree = CellTree {
            plane_width: 8,
            plane_height: 4,
            cells: vec![Cell::Intra {
                x: 0,
                y: 0,
                w: 8,
                h: 4,
                vq_leaves: vec![VqCell {
                    x: 0,
                    y: 0,
                    w: 8,
                    h: 4,
                    leaf: VqLeaf::Null(VqNull::Unpacker),
                    data_cursor: Some(2),
                }],
            }],
        };
        let plan = classify_cell_tree(0, &tree);
        let table = DyadDeltaTable::load();

        // Without the stream the unit defers.
        let recon = exec_plane_plan(&plan).expect("plan executes");
        assert_eq!(recon.stats.vq_null_unpacker_deferred, 1);
        assert_eq!(recon.stats.unpacker_static_units, 0);
        assert!(recon.frontier.is_some());

        // With the stream the static subset resolves the cell.
        let recon = exec_plane_plan_with_stream(&plan, &input, &table).expect("stream drive");
        assert_eq!(recon.stats.unpacker_static_units, 1);
        assert_eq!(recon.stats.vq_null_unpacker_deferred, 0);
        assert!(recon.frontier.is_none());
        assert!(recon.stats.is_fully_reconstructed());

        // An arena-gated literal at the anchor still defers.
        let gated = [0u8, 0u8, 0x30, 0u8];
        let recon = exec_plane_plan_with_stream(&plan, &gated, &table).expect("stream drive");
        assert_eq!(recon.stats.unpacker_static_units, 0);
        assert_eq!(recon.stats.vq_null_unpacker_deferred, 1);
        assert!(recon.frontier.is_some());

        // A truncated stream (0xFB with no counter byte) surfaces the
        // binary's typed fault through the plane executor.
        let hostile = [0u8, 0u8, 0xFB];
        let err = exec_plane_plan_with_stream(&plan, &hostile, &table).unwrap_err();
        assert!(matches!(err, PlaneExecError::UnpackerCell(_)));
    }

    #[test]
    fn unit_past_row_stride_is_rejected() {
        // A unit whose right edge exceeds 0xb0 is a malformed plan.
        let entry = CellPlanEntry {
            x: 0xae,
            y: 4,
            w: 8,
            h: 4,
            disposition: CellDisposition::VqNullCopy,
            data_cursor: None,
        };
        let plan = PlaneReconstructPlan {
            plane_idx: 0,
            plane_width: 0xb0,
            plane_height: 8,
            entries: vec![entry],
            counts: Default::default(),
        };
        let err = exec_plane_plan(&plan).unwrap_err();
        assert!(matches!(
            err,
            PlaneExecError::UnitExceedsRowStride {
                right_edge: 0xb6,
                ..
            }
        ));
    }

    #[test]
    fn strip_row_borrows_visible_width() {
        let tree = CellTree {
            plane_width: 4,
            plane_height: 4,
            cells: vec![intra_cell(
                0,
                0,
                4,
                4,
                vec![(0, VqLeaf::Null(VqNull::Skip))],
            )],
        };
        let plan = classify_cell_tree(0, &tree);
        let recon = exec_plane_plan(&plan).expect("exec");
        assert_eq!(recon.strip_row(0).map(|r| r.len()), Some(4));
        assert_eq!(recon.strip_row(4), None);
    }
}
