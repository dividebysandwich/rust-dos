//! Indeo 3 VQ_NULL copy-upper executor: the only decode path where the
//! predictor row is consumed *without* a delta add — a pure copy of the
//! upper-neighbour row into the current cell's pixel buffer.
//!
//! Spec source:
//! `docs/video/indeo/indeo3/spec/07-output-reconstruction.md` §1.4
//! (VQ_NULL copy-upper predictor source), cross-referenced from
//! `docs/video/indeo/indeo3/spec/04-vq-codebooks.md` §4 (VQ_NULL
//! semantics).
//!
//! Round 30's [`super::emit_cell_chain`] drives the *delta* decode path:
//! every position there forms a `predictor + delta` sum. §1.4 documents
//! the one exception. When the binary-tree walker reaches a VQ_NULL leaf
//! whose first two sub-code bits are `0`, `0` (`spec/04 §4`), the body at
//! `IR32_32.DLL!0x100069f4..0x10006a2d` copies the row at `[edi - 0xb0]`
//! — the upper neighbour — into the cell's pixel buffer for up to four
//! rows, with no delta application:
//!
//! ```text
//! 100069cc: sub esi, 0xb0      ; esi = upper-neighbour row (= edi - 0xb0)
//! 100069f4: ...                ; copy [esi] to [edi], [edi+0xb0],
//!                              ;                [edi+0x15c], [edi+0x20c]
//!                              ; advancing edi += 4 per column group
//! ```
//!
//! The §1.4 four destination offsets are relative to the *current*
//! column-group write cursor `edi`: rows 0 and 1 sit at the plain row
//! strides `+0x000` and `+0x0b0`, while rows 2 and 3 sit at `+0x15c`
//! (`= 2*0xb0 - 4`) and `+0x20c` (`= 3*0xb0 - 4`). The `-4` on the lower
//! two rows folds the body's interleaved `edi += 4` column advance into
//! the stored displacements: the lower band is written one column group
//! ahead of the upper band, so its absolute byte offsets are four bytes
//! smaller than the naive `row * 0xb0` would give. This module models the
//! observable result — each of the four destination rows receives a
//! byte-identical copy of the upper-neighbour row at the matching column
//! group — by iterating column group by column group and writing
//! `row * 0xb0` aligned offsets, which is byte-for-byte equivalent to the
//! binary's interleaved cursor walk.
//!
//! What this module **deliberately does not do** (the §1.4 / §4 chapter
//! boundary):
//!
//! * It does not read the bitstream. The VQ_NULL leaf-type fork and the
//!   two sub-code bits (`spec/04 §4` at `IR32_32.DLL!0x100069d4` /
//!   `0x100069f2`) are resolved by the entropy walker (`spec/06`); this
//!   module is invoked only once that fork has selected the `00`
//!   copy-upper sub-code.
//! * The `01` mark-edge sub-code (the `0x10006a2f..0x10006a55` body that
//!   sets the §4.4 / §7 [`super::EDGE_MARKER_BIT`] on the cell's pixel
//!   bytes) is the other non-degenerate VQ_NULL arm: round 32 adds
//!   [`mark_edge_cell`] alongside [`copy_upper_cell`]. The copy-upper body
//!   *copies* the upper-neighbour row; the mark-edge body *or-sets* bit 7
//!   over the cell's own pixel positions, leaving the low 7 bits intact.
//! * It does not perform the "first bit `1`" VQ-data-without-index
//!   dispatch to the per-byte unpacker (`0x10006bac`); that re-enters the
//!   delta path (`spec/06 §3.4`).
//! * It does not perform the §1.3 cross-cell predictor continuity / the
//!   top-of-strip zero seed setup — it reads whatever the buffer holds at
//!   `[edi - 0xb0]`, exactly as [`super::emit_cell_chain`] does.

use super::reconstruct::{EDGE_MARKER_BIT, PREDICTOR_ROW_STRIDE};

/// Spec/07 §1.4 — the number of destination rows the copy-upper body
/// writes per column group (`[edi]`, `[edi+0xb0]`, `[edi+0x15c]`,
/// `[edi+0x20c]`).
pub const COPY_UPPER_ROW_COUNT: usize = 4;

/// Spec/07 §1.4 — the column-group width, in bytes, the copy-upper body
/// advances `edi` by per iteration (`edi += 4`). One DWORD = four
/// horizontally adjacent pixels in softSIMD layout.
pub const COPY_UPPER_COLUMN_GROUP_BYTES: usize = 4;

/// Spec/07 §1.4 — the four destination byte displacements (relative to
/// the current column-group write cursor) the copy-upper body stores to,
/// exactly as they appear in the binary.
///
/// Rows 0 / 1 are the plain row strides `+0x000` / `+0x0b0`; rows 2 / 3
/// are `+0x15c` / `+0x20c`, which fold the body's interleaved `edi += 4`
/// advance into the displacement (`0x15c == 2*0xb0 - 4`,
/// `0x20c == 3*0xb0 - 4`). See the module docs for why the row-aligned
/// `row * 0xb0` walk this module performs is byte-for-byte equivalent.
pub const COPY_UPPER_RAW_ROW_OFFSETS: [usize; COPY_UPPER_ROW_COUNT] = [0x000, 0x0b0, 0x15c, 0x20c];

// Cross-check the raw §1.4 displacements against the row stride: rows 0/1
// are plain multiples, rows 2/3 carry the `-4` column-advance fold.
const _: () = assert!(COPY_UPPER_RAW_ROW_OFFSETS[0] == 0);
const _: () = assert!(COPY_UPPER_RAW_ROW_OFFSETS[1] == PREDICTOR_ROW_STRIDE);
const _: () = assert!(
    COPY_UPPER_RAW_ROW_OFFSETS[2] == 2 * PREDICTOR_ROW_STRIDE - COPY_UPPER_COLUMN_GROUP_BYTES
);
const _: () = assert!(
    COPY_UPPER_RAW_ROW_OFFSETS[3] == 3 * PREDICTOR_ROW_STRIDE - COPY_UPPER_COLUMN_GROUP_BYTES
);

/// The three VQ_NULL sub-codes the `spec/04 §4` leaf-type fork selects
/// from the next one-or-two bitstream bits.
///
/// This module executes only [`VqNullSubCode::CopyUpper`]; the other two
/// are surfaced so a caller routing the `spec/04 §4` fork has a typed
/// discriminant rather than a bare bit pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VqNullSubCode {
    /// First bit `1` (`IR32_32.DLL!0x100069d4`): dispatch the next byte to
    /// the per-byte unpacker (`0x10006bac`) as a VQ-data-without-index
    /// mode byte. Re-enters the `spec/06 §3.4` delta path.
    VqDataNoIndex,
    /// First bit `0`, second bit `0` (`0x100069f4`): the copy-upper body
    /// this module executes — a pure copy of the upper-neighbour row.
    CopyUpper,
    /// First bit `0`, second bit `1` (`0x10006a2f`): the mark-edge body —
    /// sets the §4.4 / §7 [`super::EDGE_MARKER_BIT`] on the cell's bytes,
    /// executed by [`mark_edge_cell`].
    MarkEdge,
}

impl VqNullSubCode {
    /// Decode the `spec/04 §4` sub-code from the leading VQ_NULL bits.
    ///
    /// `first_bit` is the bit consumed at `0x100069d4`; `second_bit` is
    /// the bit consumed at `0x100069f2`, consulted only when `first_bit`
    /// is `0`. Returns the typed sub-code.
    pub const fn from_bits(first_bit: bool, second_bit: bool) -> Self {
        if first_bit {
            VqNullSubCode::VqDataNoIndex
        } else if second_bit {
            VqNullSubCode::MarkEdge
        } else {
            VqNullSubCode::CopyUpper
        }
    }

    /// `true` only for [`VqNullSubCode::CopyUpper`] — the copy-upper
    /// sub-code [`copy_upper_cell`] executes.
    pub const fn is_copy_upper(self) -> bool {
        matches!(self, VqNullSubCode::CopyUpper)
    }

    /// `true` only for [`VqNullSubCode::MarkEdge`] — the mark-edge
    /// sub-code [`mark_edge_cell`] executes.
    pub const fn is_mark_edge(self) -> bool {
        matches!(self, VqNullSubCode::MarkEdge)
    }
}

/// The geometry of one VQ_NULL copy-upper cell.
///
/// `width_dwords` is the cell width in column groups (= width-in-pixels /
/// 4, matching [`super::CellEmitGeometry::width_dwords`]). `row_count` is
/// the number of destination rows to fill; the binary's body fills up to
/// [`COPY_UPPER_ROW_COUNT`] (4), but a shorter cell fills fewer.
/// `top_left_offset` is the byte offset of the cell's top-left pixel
/// within the strip buffer (the initial `edi`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyUpperGeometry {
    /// Cell width in column groups (pixels / 4).
    pub width_dwords: usize,
    /// Number of destination rows to fill (`1..=COPY_UPPER_ROW_COUNT`).
    pub row_count: usize,
    /// Byte offset of the cell's top-left pixel within the strip buffer.
    pub top_left_offset: usize,
}

/// The error modes the copy-upper executor surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CopyUpperError {
    /// `width_dwords` is zero (a degenerate cell with no column groups).
    ZeroWidth,
    /// `row_count` is zero or exceeds [`COPY_UPPER_ROW_COUNT`]. Carries
    /// the supplied count.
    InvalidRowCount {
        /// The supplied (out-of-range) row count.
        supplied: usize,
    },
    /// The cell's top row falls at the very top of the strip, so the
    /// `[edi - 0xb0]` upper-neighbour read would precede the buffer start.
    /// Unlike the delta path (which substitutes the §1.3 zero seed), the
    /// §1.4 copy is a literal row copy: a missing source row is a
    /// malformed-geometry condition the caller must avoid by placing the
    /// cell below the strip's padding region.
    UpperNeighbourAboveBuffer,
    /// A read or write region would land outside the strip buffer. Carries
    /// the offending exclusive end offset and the buffer length.
    OutOfBounds {
        /// The exclusive end byte offset the access would require.
        end: usize,
        /// The supplied buffer length.
        buffer_len: usize,
    },
}

impl core::fmt::Display for CopyUpperError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CopyUpperError::ZeroWidth => {
                write!(f, "spec/07 §1.4: copy-upper cell width is zero")
            }
            CopyUpperError::InvalidRowCount { supplied } => write!(
                f,
                "spec/07 §1.4: copy-upper row count {supplied} not in 1..={COPY_UPPER_ROW_COUNT}"
            ),
            CopyUpperError::UpperNeighbourAboveBuffer => write!(
                f,
                "spec/07 §1.4: copy-upper upper-neighbour row [edi - 0xb0] precedes strip buffer start"
            ),
            CopyUpperError::OutOfBounds { end, buffer_len } => write!(
                f,
                "spec/07 §1.4: copy-upper access end {end} exceeds strip buffer length {buffer_len}"
            ),
        }
    }
}

/// The result of a copy-upper cell emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct CopyUpperStats {
    /// The number of destination rows written (= `geometry.row_count`).
    pub rows_written: usize,
    /// The number of column groups copied per row (= `geometry.width_dwords`).
    pub column_groups: usize,
    /// The total number of pixel bytes copied
    /// (`rows_written * column_groups * 4`).
    pub bytes_copied: usize,
}

/// Spec/07 §1.4 — execute the VQ_NULL copy-upper body over a
/// caller-supplied strip pixel buffer.
///
/// Walks the cell column group by column group. For each column group,
/// reads the upper-neighbour DWORD at `[edi - 0xb0]` and writes it
/// byte-identically into each of the cell's `row_count` destination rows
/// at `edi + row * 0xb0`. This reproduces the §1.4 body's observable
/// effect — the cell is emitted identical to its upper neighbour at every
/// position — via a row-aligned walk equivalent to the binary's
/// interleaved cursor advance (see module docs).
///
/// Returns a [`CopyUpperStats`] on success, or a typed [`CopyUpperError`]
/// for a degenerate geometry, a top-of-strip source, or an out-of-bounds
/// access. On an out-of-bounds error the buffer is left mutated up to but
/// not including the offending access.
pub fn copy_upper_cell(
    buffer: &mut [u8],
    geometry: CopyUpperGeometry,
) -> Result<CopyUpperStats, CopyUpperError> {
    if geometry.width_dwords == 0 {
        return Err(CopyUpperError::ZeroWidth);
    }
    if geometry.row_count == 0 || geometry.row_count > COPY_UPPER_ROW_COUNT {
        return Err(CopyUpperError::InvalidRowCount {
            supplied: geometry.row_count,
        });
    }
    // The §1.4 source is the row at `[edi - 0xb0]`. For a cell whose top
    // row is in the strip's top padding band the delta path substitutes
    // the §1.3 zero seed, but the copy-upper body performs a literal copy
    // of an actual buffer row, so a source above the buffer is malformed.
    if geometry.top_left_offset < PREDICTOR_ROW_STRIDE {
        return Err(CopyUpperError::UpperNeighbourAboveBuffer);
    }

    for dword in 0..geometry.width_dwords {
        let write_cursor = geometry.top_left_offset + dword * COPY_UPPER_COLUMN_GROUP_BYTES;
        let src_off = write_cursor - PREDICTOR_ROW_STRIDE;
        let src_end = src_off + COPY_UPPER_COLUMN_GROUP_BYTES;
        if src_end > buffer.len() {
            return Err(CopyUpperError::OutOfBounds {
                end: src_end,
                buffer_len: buffer.len(),
            });
        }
        // Snapshot the upper-neighbour DWORD before any write (a cell
        // immediately below another cell in the same column never overlaps
        // its own source, but snapshotting keeps the copy order-independent
        // and matches the binary's single `mov eax, [esi]` per group).
        let mut src = [0u8; COPY_UPPER_COLUMN_GROUP_BYTES];
        src.copy_from_slice(&buffer[src_off..src_end]);

        for row in 0..geometry.row_count {
            let store_off = write_cursor + row * PREDICTOR_ROW_STRIDE;
            let store_end = store_off + COPY_UPPER_COLUMN_GROUP_BYTES;
            if store_end > buffer.len() {
                return Err(CopyUpperError::OutOfBounds {
                    end: store_end,
                    buffer_len: buffer.len(),
                });
            }
            buffer[store_off..store_end].copy_from_slice(&src);
        }
    }

    Ok(CopyUpperStats {
        rows_written: geometry.row_count,
        column_groups: geometry.width_dwords,
        bytes_copied: geometry.row_count * geometry.width_dwords * COPY_UPPER_COLUMN_GROUP_BYTES,
    })
}

/// The geometry of one VQ_NULL mark-edge cell.
///
/// The mark-edge body (`spec/04 §4` at `IR32_32.DLL!0x10006a2f`) walks the
/// cell's own pixel positions and or-sets bit 7 ([`EDGE_MARKER_BIT`]) on
/// each. Its geometry is the same cell shape the rest of the decoder uses:
/// `width_dwords` column groups (= width-in-pixels / 4) by `row_count`
/// rows, with each row at the [`PREDICTOR_ROW_STRIDE`] (`0xb0`) stride.
///
/// Unlike [`CopyUpperGeometry`], there is no upper-neighbour read, so the
/// cell may legitimately sit at the very top of the strip — bit 7 is set on
/// the cell's own bytes, never on a row above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkEdgeGeometry {
    /// Cell width in column groups (pixels / 4).
    pub width_dwords: usize,
    /// Number of cell rows to mark (`1..=COPY_UPPER_ROW_COUNT`).
    pub row_count: usize,
    /// Byte offset of the cell's top-left pixel within the strip buffer.
    pub top_left_offset: usize,
}

/// The error modes the mark-edge executor surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MarkEdgeError {
    /// `width_dwords` is zero (a degenerate cell with no column groups).
    ZeroWidth,
    /// `row_count` is zero or exceeds [`COPY_UPPER_ROW_COUNT`]. Carries
    /// the supplied count.
    InvalidRowCount {
        /// The supplied (out-of-range) row count.
        supplied: usize,
    },
    /// A marked region would land outside the strip buffer. Carries the
    /// offending exclusive end offset and the buffer length.
    OutOfBounds {
        /// The exclusive end byte offset the access would require.
        end: usize,
        /// The supplied buffer length.
        buffer_len: usize,
    },
}

impl core::fmt::Display for MarkEdgeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MarkEdgeError::ZeroWidth => {
                write!(f, "spec/04 §4: mark-edge cell width is zero")
            }
            MarkEdgeError::InvalidRowCount { supplied } => write!(
                f,
                "spec/04 §4: mark-edge row count {supplied} not in 1..={COPY_UPPER_ROW_COUNT}"
            ),
            MarkEdgeError::OutOfBounds { end, buffer_len } => write!(
                f,
                "spec/04 §4: mark-edge access end {end} exceeds strip buffer length {buffer_len}"
            ),
        }
    }
}

/// The result of a mark-edge cell emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct MarkEdgeStats {
    /// The number of cell rows marked (= `geometry.row_count`).
    pub rows_marked: usize,
    /// The number of column groups marked per row (= `geometry.width_dwords`).
    pub column_groups: usize,
    /// The total number of pixel bytes marked
    /// (`rows_marked * column_groups * 4`).
    pub bytes_marked: usize,
}

/// Spec/04 §4 — execute the VQ_NULL `01` mark-edge body over a
/// caller-supplied strip pixel buffer.
///
/// The body at `IR32_32.DLL!0x10006a2f..0x10006a55` walks the cell's own
/// pixel positions and sets the high bit ([`EDGE_MARKER_BIT`] = `0x80`) of
/// each, marking the cell as an edge / boundary cell. This module models
/// that as an or-set of bit 7 over each of the cell's `row_count` rows ×
/// `width_dwords` column groups, walking rows at the
/// [`PREDICTOR_ROW_STRIDE`] (`0xb0`) per-row stride and bytes
/// left-to-right within each row.
///
/// The low 7 bits of each marked byte are preserved (the body sets bit 7
/// with an or, it does not overwrite the pixel value): per `spec/07 §4.2`
/// the marker is a sentinel layered on top of the existing 7-bit content,
/// consumed downstream during output reconstruction (the `shl 1` upshift
/// discards it).
///
/// Returns a [`MarkEdgeStats`] on success, or a typed [`MarkEdgeError`]
/// for a degenerate geometry or an out-of-bounds access. On an
/// out-of-bounds error the buffer is left mutated up to but not including
/// the offending access.
pub fn mark_edge_cell(
    buffer: &mut [u8],
    geometry: MarkEdgeGeometry,
) -> Result<MarkEdgeStats, MarkEdgeError> {
    if geometry.width_dwords == 0 {
        return Err(MarkEdgeError::ZeroWidth);
    }
    if geometry.row_count == 0 || geometry.row_count > COPY_UPPER_ROW_COUNT {
        return Err(MarkEdgeError::InvalidRowCount {
            supplied: geometry.row_count,
        });
    }

    let row_bytes = geometry.width_dwords * COPY_UPPER_COLUMN_GROUP_BYTES;
    for row in 0..geometry.row_count {
        let row_off = geometry.top_left_offset + row * PREDICTOR_ROW_STRIDE;
        let row_end = row_off + row_bytes;
        if row_end > buffer.len() {
            return Err(MarkEdgeError::OutOfBounds {
                end: row_end,
                buffer_len: buffer.len(),
            });
        }
        for byte in &mut buffer[row_off..row_end] {
            *byte |= EDGE_MARKER_BIT;
        }
    }

    Ok(MarkEdgeStats {
        rows_marked: geometry.row_count,
        column_groups: geometry.width_dwords,
        bytes_marked: geometry.row_count * row_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRIDE: usize = PREDICTOR_ROW_STRIDE;

    #[test]
    fn copy_upper_constants() {
        assert_eq!(COPY_UPPER_ROW_COUNT, 4);
        assert_eq!(COPY_UPPER_COLUMN_GROUP_BYTES, 4);
        assert_eq!(COPY_UPPER_RAW_ROW_OFFSETS, [0x000, 0x0b0, 0x15c, 0x20c]);
        // The §1.4 -4 fold on the lower band.
        assert_eq!(COPY_UPPER_RAW_ROW_OFFSETS[2], 2 * STRIDE - 4);
        assert_eq!(COPY_UPPER_RAW_ROW_OFFSETS[3], 3 * STRIDE - 4);
    }

    #[test]
    fn subcode_from_bits() {
        // First bit 1 → VQ-data-no-index regardless of second bit.
        assert_eq!(
            VqNullSubCode::from_bits(true, false),
            VqNullSubCode::VqDataNoIndex
        );
        assert_eq!(
            VqNullSubCode::from_bits(true, true),
            VqNullSubCode::VqDataNoIndex
        );
        // First 0, second 0 → copy-upper.
        assert_eq!(
            VqNullSubCode::from_bits(false, false),
            VqNullSubCode::CopyUpper
        );
        // First 0, second 1 → mark-edge.
        assert_eq!(
            VqNullSubCode::from_bits(false, true),
            VqNullSubCode::MarkEdge
        );
    }

    #[test]
    fn subcode_is_copy_upper_predicate() {
        assert!(VqNullSubCode::CopyUpper.is_copy_upper());
        assert!(!VqNullSubCode::MarkEdge.is_copy_upper());
        assert!(!VqNullSubCode::VqDataNoIndex.is_copy_upper());
    }

    #[test]
    fn zero_width_rejected() {
        let mut buf = vec![0u8; STRIDE * 8];
        let g = CopyUpperGeometry {
            width_dwords: 0,
            row_count: 4,
            top_left_offset: STRIDE,
        };
        assert_eq!(copy_upper_cell(&mut buf, g), Err(CopyUpperError::ZeroWidth));
    }

    #[test]
    fn invalid_row_count_rejected() {
        let mut buf = vec![0u8; STRIDE * 8];
        let g0 = CopyUpperGeometry {
            width_dwords: 1,
            row_count: 0,
            top_left_offset: STRIDE,
        };
        assert_eq!(
            copy_upper_cell(&mut buf, g0),
            Err(CopyUpperError::InvalidRowCount { supplied: 0 })
        );
        let g5 = CopyUpperGeometry {
            width_dwords: 1,
            row_count: 5,
            top_left_offset: STRIDE,
        };
        assert_eq!(
            copy_upper_cell(&mut buf, g5),
            Err(CopyUpperError::InvalidRowCount { supplied: 5 })
        );
    }

    #[test]
    fn top_of_strip_source_rejected() {
        // A cell whose top row is within the first row stride has no
        // in-buffer upper neighbour to copy.
        let mut buf = vec![0u8; STRIDE * 8];
        let g = CopyUpperGeometry {
            width_dwords: 1,
            row_count: 4,
            top_left_offset: STRIDE - 4,
        };
        assert_eq!(
            copy_upper_cell(&mut buf, g),
            Err(CopyUpperError::UpperNeighbourAboveBuffer)
        );
    }

    #[test]
    fn single_column_four_row_copy() {
        // Seed an upper-neighbour row one stride above the cell; the copy
        // should replicate it into all four destination rows.
        let mut buf = vec![0u8; STRIDE * 6];
        let top = 2 * STRIDE;
        let seed = [0x11u8, 0x22, 0x33, 0x44];
        // Upper neighbour is at `top - STRIDE`.
        buf[(top - STRIDE)..(top - STRIDE + 4)].copy_from_slice(&seed);
        let g = CopyUpperGeometry {
            width_dwords: 1,
            row_count: 4,
            top_left_offset: top,
        };
        let stats = copy_upper_cell(&mut buf, g).unwrap();
        assert_eq!(stats.rows_written, 4);
        assert_eq!(stats.column_groups, 1);
        assert_eq!(stats.bytes_copied, 16);
        for row in 0..4 {
            let off = top + row * STRIDE;
            assert_eq!(&buf[off..off + 4], &seed, "row {row} mismatch");
        }
    }

    #[test]
    fn multi_column_copy_iterates_left_to_right() {
        // An 8-pixel-wide cell is two column groups; each copies its own
        // matching upper-neighbour DWORD.
        let mut buf = vec![0u8; STRIDE * 4];
        let top = STRIDE;
        let left = [0xa0u8, 0xa1, 0xa2, 0xa3];
        let right = [0xb0u8, 0xb1, 0xb2, 0xb3];
        buf[(top - STRIDE)..(top - STRIDE + 4)].copy_from_slice(&left);
        buf[(top - STRIDE + 4)..(top - STRIDE + 8)].copy_from_slice(&right);
        let g = CopyUpperGeometry {
            width_dwords: 2,
            row_count: 2,
            top_left_offset: top,
        };
        copy_upper_cell(&mut buf, g).unwrap();
        for row in 0..2 {
            let off = top + row * STRIDE;
            assert_eq!(&buf[off..off + 4], &left, "left col, row {row}");
            assert_eq!(&buf[off + 4..off + 8], &right, "right col, row {row}");
        }
    }

    #[test]
    fn partial_row_count_leaves_lower_rows_untouched() {
        // A 2-row cell must not write the rows-2/3 band.
        let mut buf = vec![0u8; STRIDE * 6];
        let top = STRIDE;
        let seed = [0x07u8, 0x08, 0x09, 0x0a];
        buf[(top - STRIDE)..(top - STRIDE + 4)].copy_from_slice(&seed);
        // Mark the row-2 / row-3 destinations with a sentinel.
        for row in 2..4 {
            let off = top + row * STRIDE;
            buf[off..off + 4].copy_from_slice(&[0xee; 4]);
        }
        let g = CopyUpperGeometry {
            width_dwords: 1,
            row_count: 2,
            top_left_offset: top,
        };
        copy_upper_cell(&mut buf, g).unwrap();
        assert_eq!(&buf[top..top + 4], &seed);
        assert_eq!(&buf[top + STRIDE..top + STRIDE + 4], &seed);
        // Untouched lower band keeps the sentinel.
        for row in 2..4 {
            let off = top + row * STRIDE;
            assert_eq!(
                &buf[off..off + 4],
                &[0xee; 4],
                "row {row} should be untouched"
            );
        }
    }

    #[test]
    fn out_of_bounds_write_surfaced() {
        // A cell whose lower-band write exceeds the buffer surfaces a typed
        // error rather than panicking.
        let mut buf = vec![0u8; STRIDE + 4]; // only room for the source + row 0
        let g = CopyUpperGeometry {
            width_dwords: 1,
            row_count: 4,
            top_left_offset: STRIDE,
        };
        let r = copy_upper_cell(&mut buf, g);
        assert!(matches!(r, Err(CopyUpperError::OutOfBounds { .. })));
    }

    #[test]
    fn copy_is_identical_to_upper_neighbour_invariant() {
        // The §1.4 disposition: the cell is emitted identical to the
        // upper-neighbour row at every position. Verify across a 3-column,
        // 4-row cell with a distinct byte per source column.
        let mut buf = vec![0u8; STRIDE * 6];
        let top = STRIDE;
        let upper = top - STRIDE;
        for c in 0..12usize {
            buf[upper + c] = (0x40 + c) as u8;
        }
        let g = CopyUpperGeometry {
            width_dwords: 3,
            row_count: 4,
            top_left_offset: top,
        };
        copy_upper_cell(&mut buf, g).unwrap();
        for row in 0..4 {
            for c in 0..12usize {
                assert_eq!(
                    buf[top + row * STRIDE + c],
                    (0x40 + c) as u8,
                    "row {row} col {c}"
                );
            }
        }
    }

    #[test]
    fn error_display_cites_spec() {
        assert!(CopyUpperError::ZeroWidth.to_string().contains("§1.4"));
        assert!(
            CopyUpperError::InvalidRowCount { supplied: 9 }
                .to_string()
                .contains("§1.4")
        );
        assert!(
            CopyUpperError::UpperNeighbourAboveBuffer
                .to_string()
                .contains("§1.4")
        );
        assert!(
            CopyUpperError::OutOfBounds {
                end: 10,
                buffer_len: 4
            }
            .to_string()
            .contains("§1.4")
        );
    }

    #[test]
    fn subcode_is_mark_edge_predicate() {
        assert!(VqNullSubCode::MarkEdge.is_mark_edge());
        assert!(!VqNullSubCode::CopyUpper.is_mark_edge());
        assert!(!VqNullSubCode::VqDataNoIndex.is_mark_edge());
    }

    #[test]
    fn mark_edge_zero_width_rejected() {
        let mut buf = vec![0u8; STRIDE * 4];
        let g = MarkEdgeGeometry {
            width_dwords: 0,
            row_count: 4,
            top_left_offset: 0,
        };
        assert_eq!(mark_edge_cell(&mut buf, g), Err(MarkEdgeError::ZeroWidth));
    }

    #[test]
    fn mark_edge_invalid_row_count_rejected() {
        let mut buf = vec![0u8; STRIDE * 4];
        let g0 = MarkEdgeGeometry {
            width_dwords: 1,
            row_count: 0,
            top_left_offset: 0,
        };
        assert_eq!(
            mark_edge_cell(&mut buf, g0),
            Err(MarkEdgeError::InvalidRowCount { supplied: 0 })
        );
        let g5 = MarkEdgeGeometry {
            width_dwords: 1,
            row_count: 5,
            top_left_offset: 0,
        };
        assert_eq!(
            mark_edge_cell(&mut buf, g5),
            Err(MarkEdgeError::InvalidRowCount { supplied: 5 })
        );
    }

    #[test]
    fn mark_edge_sets_bit7_over_cell() {
        // A 2-column (8-pixel), 4-row cell at the strip top. The mark-edge
        // body or-sets bit 7 over every cell byte and leaves everything
        // else untouched.
        let mut buf = vec![0u8; STRIDE * 6];
        let top = 0usize; // mark-edge has no upper-neighbour read, so top is fine.
        let g = MarkEdgeGeometry {
            width_dwords: 2,
            row_count: 4,
            top_left_offset: top,
        };
        let stats = mark_edge_cell(&mut buf, g).unwrap();
        assert_eq!(stats.rows_marked, 4);
        assert_eq!(stats.column_groups, 2);
        assert_eq!(stats.bytes_marked, 32);
        for row in 0..4 {
            let off = top + row * STRIDE;
            for c in 0..8 {
                assert_eq!(buf[off + c], EDGE_MARKER_BIT, "cell byte row {row} col {c}");
            }
            // The byte just past the cell width is untouched.
            assert_eq!(buf[off + 8], 0, "byte past cell row {row}");
        }
    }

    #[test]
    fn mark_edge_preserves_low_seven_bits() {
        // The body or-sets bit 7; the existing 7-bit pixel content survives.
        let mut buf = vec![0u8; STRIDE * 2];
        let top = STRIDE;
        let content = [0x01u8, 0x42, 0x7f, 0x00];
        buf[top..top + 4].copy_from_slice(&content);
        let g = MarkEdgeGeometry {
            width_dwords: 1,
            row_count: 1,
            top_left_offset: top,
        };
        mark_edge_cell(&mut buf, g).unwrap();
        for (i, &orig) in content.iter().enumerate() {
            assert_eq!(buf[top + i], orig | EDGE_MARKER_BIT);
            // Low 7 bits unchanged.
            assert_eq!(buf[top + i] & !EDGE_MARKER_BIT, orig);
        }
    }

    #[test]
    fn mark_edge_already_marked_is_idempotent() {
        // An or-set is idempotent: re-marking an already-marked cell is a
        // no-op on the bytes.
        let mut buf = vec![0u8; STRIDE * 2];
        let top = 0usize;
        buf[top..top + 4].copy_from_slice(&[EDGE_MARKER_BIT | 0x11; 4]);
        let g = MarkEdgeGeometry {
            width_dwords: 1,
            row_count: 1,
            top_left_offset: top,
        };
        mark_edge_cell(&mut buf, g).unwrap();
        assert_eq!(&buf[top..top + 4], &[EDGE_MARKER_BIT | 0x11; 4]);
    }

    #[test]
    fn mark_edge_partial_row_count_leaves_lower_rows_untouched() {
        let mut buf = vec![0u8; STRIDE * 6];
        let top = 0usize;
        let g = MarkEdgeGeometry {
            width_dwords: 1,
            row_count: 2,
            top_left_offset: top,
        };
        mark_edge_cell(&mut buf, g).unwrap();
        for row in 0..2 {
            let off = top + row * STRIDE;
            assert_eq!(&buf[off..off + 4], &[EDGE_MARKER_BIT; 4]);
        }
        for row in 2..4 {
            let off = top + row * STRIDE;
            assert_eq!(&buf[off..off + 4], &[0u8; 4], "row {row} must be untouched");
        }
    }

    #[test]
    fn mark_edge_out_of_bounds_surfaced() {
        // A cell whose row 1 would exceed the buffer surfaces a typed error.
        let mut buf = vec![0u8; STRIDE + 2]; // room for row 0 (4 bytes) but not row 1.
        let g = MarkEdgeGeometry {
            width_dwords: 1,
            row_count: 2,
            top_left_offset: 0,
        };
        let r = mark_edge_cell(&mut buf, g);
        assert!(matches!(r, Err(MarkEdgeError::OutOfBounds { .. })));
        // Row 0 was marked before the out-of-bounds abort on row 1.
        assert_eq!(&buf[0..4], &[EDGE_MARKER_BIT; 4]);
    }

    #[test]
    fn mark_edge_error_display_cites_spec() {
        assert!(MarkEdgeError::ZeroWidth.to_string().contains("§4"));
        assert!(
            MarkEdgeError::InvalidRowCount { supplied: 9 }
                .to_string()
                .contains("§4")
        );
        assert!(
            MarkEdgeError::OutOfBounds {
                end: 10,
                buffer_len: 4
            }
            .to_string()
            .contains("§4")
        );
    }
}
