//! Indeo 3 spec/03 §5.4 end-of-strip edge fix-up surface.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/03-macroblock-layer.md`
//! §5.4 (strip-edge fix-up) + §5 (slot indexing) + spec/02 §5.1
//! (per-slot plane-role classification) and §5.2 (per-slot field
//! offsets `+0x18` / `+0x1c`).
//!
//! Round 10's [`super::cell_subarray::CellStackTopDispatch`] surfaced
//! the *dispatch* into the §5.4 branch when the destination-slot
//! cell-stack top is zero. This module surfaces the §5.4 fix-up's
//! own per-slot parameters (height stride + width stride) and the
//! per-row byte-copy step, both as descriptors:
//!
//! * [`StripEdgeFixupDims::for_slot`] — the (height, width) pair the
//!   §5.4 loop iterates with, after the per-plane-role `sar 2` divide-
//!   by-4 (luma slots 0/3 → as-stored; chroma slots 1/2/4/5 → divided
//!   by 4 per `IR32_32.DLL!0x10006b5e..0x10006b61`).
//! * [`strip_edge_chroma_shift`] — the §5.4 `sar 2` width / height
//!   shift constant (`STRIP_EDGE_CHROMA_SHIFT` = 2, the chroma
//!   subsampling ratio per `spec/02 §4.1`).
//! * [`strip_edge_row_step`] — the §5.4 per-row pointer-advance step
//!   in bytes (`STRIP_EDGE_ROW_STRIDE` = `0xb0`, the same `0xb0`
//!   stride the per-cell fix-up uses).
//! * [`strip_edge_byte_copy_offsets`] — the §5.4 `mov al, [edi-1];
//!   mov [edi], al` per-row byte read / write offsets relative to the
//!   row cursor (read at `-1`, write at `0`).
//! * [`StripEdgeRowIter::new`] — a non-allocating iterator yielding
//!   the (row_index, read_offset, write_offset) triples the §5.4
//!   loop emits over the strip's full height (chroma-adjusted).
//!
//! What this module **deliberately does not do** (the §5 chapter
//! boundary):
//!
//! * It does not own the strip-context slot bytes the `+0x18` /
//!   `+0x1c` fields are read from. Callers pass the slot's strip
//!   height + strip width as already-loaded `u32`s; the per-slot
//!   field offsets themselves live on [`super::strip_context::
//!   slot_field`] (`STRIP_HEIGHT` / `STRIP_WIDTH`).
//! * It does not own the pixel-buffer arena the read / write offsets
//!   address into. The §5.4 fix-up reads `[edi - 1]` and writes
//!   `[edi]` relative to whichever pixel-buffer-base pointer the
//!   slot's `+0x00..+0x14` table resolved to (spec/03 §5.2); this
//!   module surfaces the *relative* offsets, not the absolute
//!   addresses.
//!
//! Round 18 (this round) closes the §5.4 executor surface that round 11
//! deferred to the caller: [`StripEdgeFixupDims::apply_to_buffer`] runs
//! the per-row `dest[r * 0xb0 + width] = src[r * 0xb0 + width - 1]`
//! rightmost-column duplication on a caller-supplied `&mut [u8]` strip
//! pixel buffer view, walking `strip_height` rows at the
//! [`STRIP_EDGE_ROW_STRIDE`] (`0xb0`) row stride. The function is
//! safe-Rust over a slice and surfaces [`StripEdgeApplyError`] for the
//! three failure modes the §5.4 contract leaves open to the caller
//! (zero-width strip, last-row-write-out-of-buffer, and
//! buffer-too-short).
//!
//! All offsets, field widths, and divide-by-4 disposition are taken
//! from `03-macroblock-layer.md` §5.4. RVAs cited in doc-comments
//! refer to the binary identified in `spec/00 §2`.

use super::cell_subarray::PER_CELL_EDGE_ROW_STRIDE;
use super::strip_context::PlaneRole;

// ---- §5.4 (shift + stride constants) -------------------------------

/// Spec/03 §5.4 — chroma `sar 2` shift constant (`2`).
///
/// `IR32_32.DLL!0x10006b5e..0x10006b61` applies `sar edx, 0x2; sar
/// eax, 0x2` to the strip-height and strip-width fields before
/// driving the §5.4 byte-copy loop, when the slot's plane role is
/// chroma (slot indices 1, 2, 4, 5). The shift amount is the 4:1
/// chroma subsampling ratio established by `spec/02 §4.1`.
pub const STRIP_EDGE_CHROMA_SHIFT: u32 = 2;

/// Spec/03 §5.4 — per-row pointer-advance step in bytes (`0xb0`).
///
/// The §5.4 fix-up walks down the strip's full height one row at a
/// time. The stride between adjacent row starts inside the strip
/// pixel buffer is `0xb0` (= 176 bytes), the same stride the
/// §5.5 per-cell fix-up uses (re-exported as
/// [`super::cell_subarray::PER_CELL_EDGE_ROW_STRIDE`]) and the same
/// stride the reconstruction kernel's predictor address uses
/// ([`super::reconstruct::PREDICTOR_ROW_STRIDE`]).
pub const STRIP_EDGE_ROW_STRIDE: usize = PER_CELL_EDGE_ROW_STRIDE;

/// Spec/03 §5.4 — per-row byte read offset relative to the row's
/// destination cursor (`-1`).
///
/// The `mov al, [edi - 1]` at the body of the §5.4 loop reads the
/// rightmost source byte one position *before* the destination write
/// cursor.
pub const STRIP_EDGE_BYTE_READ_OFFSET: i32 = -1;

/// Spec/03 §5.4 — per-row byte write offset relative to the row's
/// destination cursor (`0`).
///
/// The `mov [edi], al` at the body of the §5.4 loop writes the
/// loaded source byte to the destination cursor itself.
pub const STRIP_EDGE_BYTE_WRITE_OFFSET: i32 = 0;

// ---- §5.4 (per-slot dimensions) ------------------------------------

/// Spec/03 §5.4 — strip-edge fix-up height + width after per-plane-
/// role chroma `sar 2` divide.
///
/// Constructed by [`Self::for_slot`] from the destination slot's
/// `+0x18` strip-height and `+0x1c` strip-width fields and the slot
/// index. Slots 0 and 3 (luma, per `spec/02 §5.1`) preserve the
/// fields verbatim; slots 1, 2, 4, 5 (chroma, per `spec/02 §5.1`)
/// apply `sar 2` to each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StripEdgeFixupDims {
    /// Per-iteration height (in pixels), after chroma divide-by-4 if
    /// applicable. `IR32_32.DLL!0x10006b5e`'s `sar edx, 0x2`
    /// destination.
    pub strip_height: u32,
    /// Per-iteration width (in pixels), after chroma divide-by-4 if
    /// applicable. `IR32_32.DLL!0x10006b61`'s `sar eax, 0x2`
    /// destination.
    pub strip_width: u32,
    /// Plane-role classification of the destination slot (carried
    /// through from `super::strip_context::PlaneRole::for_slot`).
    pub plane_role: PlaneRole,
}

impl StripEdgeFixupDims {
    /// Spec/03 §5.4 — build the fix-up dimensions for a destination
    /// slot's strip-height + strip-width fields.
    ///
    /// Reads `strip_height` from the slot's `+0x18` field and
    /// `strip_width` from `+0x1c`, then divides each by 4 via `sar 2`
    /// when [`PlaneRole::for_slot(slot_idx)`] reports
    /// [`PlaneRole::Chroma`]. Returns `None` for an out-of-range
    /// `slot_idx` (only the 32 strip-context slots are addressable
    /// per `spec/02 §5`); the caller is expected to have validated
    /// `slot_idx < STRIP_SLOT_COUNT` before reaching the §5.4 fix-up,
    /// but this method enforces it as a safety net.
    ///
    /// Luma slots (`slot_idx ∈ {0, 3}`) preserve the fields verbatim
    /// per the static-analysis branch in `03-macroblock-layer.md`
    /// §5.4 "Luma path". Chroma slots
    /// (`slot_idx ∈ {1, 2, 4, 5}`) apply the `sar 2` divide per the
    /// "Chroma path" branch. Scratch slots (`slot_idx ∈ {6..31}`)
    /// are not dispatched into the §5.4 fix-up by the per-plane
    /// decode call (spec/02 §5.1 limits dispatchable slots to 0..5),
    /// so `Scratch`-role slots yield `None` here; that signals "the
    /// §5.4 fix-up does not apply to this slot" to the caller rather
    /// than executing an undefined divide-or-preserve disposition.
    pub fn for_slot(slot_idx: usize, strip_height: u32, strip_width: u32) -> Option<Self> {
        let plane_role = PlaneRole::for_slot(slot_idx);
        let (h, w) = match plane_role {
            PlaneRole::Luma => (strip_height, strip_width),
            PlaneRole::Chroma => (
                strip_height >> STRIP_EDGE_CHROMA_SHIFT,
                strip_width >> STRIP_EDGE_CHROMA_SHIFT,
            ),
            PlaneRole::Scratch => return None,
        };
        Some(Self {
            strip_height: h,
            strip_width: w,
            plane_role,
        })
    }

    /// Spec/03 §5.4 — the per-row iterator the §5.4 byte-copy loop
    /// would walk. Yields one row index per strip row, from `0`
    /// (top) up to `strip_height - 1` (bottom).
    pub fn row_iter(self) -> StripEdgeRowIter {
        StripEdgeRowIter::new(self.strip_height)
    }

    /// Spec/03 §5.4 — true iff the destination slot is luma (the
    /// `ebx == 0 || ebx == 3` branch in the static analysis).
    pub fn is_luma(self) -> bool {
        matches!(self.plane_role, PlaneRole::Luma)
    }

    /// Spec/03 §5.4 — true iff the destination slot is chroma (the
    /// `ebx ∈ {1, 2, 4, 5}` branch in the static analysis).
    pub fn is_chroma(self) -> bool {
        matches!(self.plane_role, PlaneRole::Chroma)
    }
}

// ---- §5.4 (per-row iteration) --------------------------------------

/// Spec/03 §5.4 — per-row iteration descriptor.
///
/// The §5.4 fix-up walks `strip_height` rows top-to-bottom, advancing
/// the pixel cursor by [`STRIP_EDGE_ROW_STRIDE`] (`0xb0`) bytes per
/// row, and on each row reads one byte at offset `-1` and writes one
/// byte at offset `0` relative to the row's leading cursor position.
/// This iterator emits, for each row, the row index (`0`-based) plus
/// the (signed) read / write offsets relative to the row's leading
/// cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StripEdgeRowIter {
    next_row: u32,
    remaining: u32,
}

impl StripEdgeRowIter {
    /// Spec/03 §5.4 — build a row iterator for a strip of `height`
    /// rows.
    ///
    /// A `height == 0` strip yields zero iterations (the §5.4 loop's
    /// `while (rows_remaining)` test fails at the first check).
    pub fn new(height: u32) -> Self {
        Self {
            next_row: 0,
            remaining: height,
        }
    }
}

/// Spec/03 §5.4 — one row's worth of per-row state yielded by the
/// [`StripEdgeRowIter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StripEdgeRow {
    /// Row index, `0` for top row, `strip_height - 1` for bottom row.
    pub row_index: u32,
    /// Byte offset (relative to the strip pixel-buffer base for the
    /// fix-up's destination slot) of this row's leading cursor.
    /// Equals `row_index * STRIP_EDGE_ROW_STRIDE`.
    pub row_cursor_byte_offset: usize,
    /// Byte offset (relative to this row's leading cursor) of the
    /// `mov al, [edi - 1]` read. Always equals
    /// [`STRIP_EDGE_BYTE_READ_OFFSET`].
    pub read_offset: i32,
    /// Byte offset (relative to this row's leading cursor) of the
    /// `mov [edi], al` write. Always equals
    /// [`STRIP_EDGE_BYTE_WRITE_OFFSET`].
    pub write_offset: i32,
}

impl Iterator for StripEdgeRowIter {
    type Item = StripEdgeRow;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let row_index = self.next_row;
        let row_cursor_byte_offset = (row_index as usize) * STRIP_EDGE_ROW_STRIDE;
        self.next_row += 1;
        self.remaining -= 1;
        Some(StripEdgeRow {
            row_index,
            row_cursor_byte_offset,
            read_offset: STRIP_EDGE_BYTE_READ_OFFSET,
            write_offset: STRIP_EDGE_BYTE_WRITE_OFFSET,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining as usize, Some(self.remaining as usize))
    }
}

impl ExactSizeIterator for StripEdgeRowIter {}

// ---- helpers (re-export-friendly accessor functions) ---------------

/// Spec/03 §5.4 — re-export accessor for [`STRIP_EDGE_CHROMA_SHIFT`].
pub const fn strip_edge_chroma_shift() -> u32 {
    STRIP_EDGE_CHROMA_SHIFT
}

/// Spec/03 §5.4 — re-export accessor for [`STRIP_EDGE_ROW_STRIDE`].
pub const fn strip_edge_row_step() -> usize {
    STRIP_EDGE_ROW_STRIDE
}

/// Spec/03 §5.4 — the (read, write) per-row byte-copy offset pair.
pub const fn strip_edge_byte_copy_offsets() -> (i32, i32) {
    (STRIP_EDGE_BYTE_READ_OFFSET, STRIP_EDGE_BYTE_WRITE_OFFSET)
}

// ---- §5.4 (per-row byte-copy executor) -----------------------------

/// Spec/03 §5.4 — failure modes the byte-copy executor surfaces when
/// the caller's pixel-buffer view cannot host the per-row
/// rightmost-column duplication.
///
/// The §5.4 spec describes the byte-copy in terms of an `edi` cursor
/// positioned at the rightmost column of the strip. In a safe-Rust
/// slice view, that cursor's read at `[edi - 1]` and write at
/// `[edi]` correspond to row-relative byte offsets of `width - 1`
/// (read source: the rightmost stored pixel) and `width` (write
/// destination: the padding byte one position to the right of the
/// rightmost stored pixel). These offsets, scaled by the per-row
/// stride [`STRIP_EDGE_ROW_STRIDE`] (`0xb0`), index into the
/// caller's pixel buffer. The three failure modes below capture the
/// out-of-bounds cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StripEdgeApplyError {
    /// The dims report a zero `strip_width`; the §5.4 fix-up cannot
    /// derive a "one-byte-before-rightmost" read offset because there
    /// is no rightmost stored pixel. The spec's `mov al, [edi - 1]`
    /// load presumes a non-empty row; a zero-width strip is a
    /// caller-side construction error.
    ZeroWidthStrip,
    /// The caller's pixel buffer has fewer than `strip_height * 0xb0`
    /// bytes available, so at least one row's `width`-relative cursor
    /// would address past the slice's end. Carries the required
    /// byte count + the supplied byte count for diagnostic purposes.
    BufferTooShort {
        /// Number of bytes the §5.4 walk requires (`strip_height *
        /// STRIP_EDGE_ROW_STRIDE`).
        required_bytes: usize,
        /// Number of bytes the caller actually supplied.
        supplied_bytes: usize,
    },
    /// The strip's stored `strip_width` is at least the per-row
    /// [`STRIP_EDGE_ROW_STRIDE`] (`0xb0` = 176). The §5.4 fix-up
    /// would write at row-relative offset `strip_width` (the
    /// position one byte past the rightmost stored pixel), which
    /// must lie strictly inside the same row's allocated stride. The
    /// strip allocator at `spec/03 §5.2` documents `0xb0` as the
    /// strip pixel buffer's allocated row stride (visible width fits
    /// strictly within the stride, with the trailing bytes serving
    /// as boundary padding) — the §5.4 destination position is part
    /// of that padding region. `strip_width >= 0xb0` therefore
    /// signals a malformed dimension pair: it leaves no padding
    /// position for the duplicated byte.
    WidthExceedsRowStride {
        /// The supplied `strip_width` (in pixels).
        supplied_width: u32,
        /// The fixed [`STRIP_EDGE_ROW_STRIDE`] (`0xb0`).
        row_stride: usize,
    },
}

impl core::fmt::Display for StripEdgeApplyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            StripEdgeApplyError::ZeroWidthStrip => f.write_str(
                "spec/03 §5.4: zero-width strip — `mov al, [edi - 1]` source position is undefined",
            ),
            StripEdgeApplyError::BufferTooShort {
                required_bytes,
                supplied_bytes,
            } => write!(
                f,
                "spec/03 §5.4: pixel-buffer slice has {supplied_bytes} bytes; \
                 fix-up requires at least {required_bytes} (strip_height × 0xb0)"
            ),
            StripEdgeApplyError::WidthExceedsRowStride {
                supplied_width,
                row_stride,
            } => write!(
                f,
                "spec/03 §5.4: strip_width = {supplied_width} reaches or exceeds row stride 0x{row_stride:x} \
                 (spec/03 §5.2 keeps visible width strictly within the allocated stride)"
            ),
        }
    }
}

impl std::error::Error for StripEdgeApplyError {}

impl StripEdgeFixupDims {
    /// Spec/03 §5.4 — execute the per-row rightmost-column byte
    /// duplication on a caller-supplied strip-pixel-buffer slice.
    ///
    /// For each row `r` in `0..strip_height`, the function reads the
    /// byte at position `r * 0xb0 + (strip_width - 1)` (the rightmost
    /// stored pixel, equivalent to the spec's `[edi - 1]` load) and
    /// writes that byte to position `r * 0xb0 + strip_width` (the
    /// padding byte just past the rightmost stored pixel, equivalent
    /// to the spec's `[edi]` store). This realises the "across-strip-
    /// boundary pixel duplication" the §5.4 prose describes
    /// ("copies the rightmost column of pixels from `[edi-1]` to
    /// `[edi]` ... for the strip's full height").
    ///
    /// The `pixel_buffer` slice is the strip-context slot's pixel-
    /// buffer base (per `spec/03 §5.2`'s `+0x00..+0x14` pointer
    /// table) viewed as a flat byte slice. The function does not
    /// allocate, does not perform interior mutability, and uses one
    /// per-row read + write per iteration (matching the spec's
    /// per-row `mov al, [edi - 1]; mov [edi], al` pair).
    ///
    /// Errors are reported via [`StripEdgeApplyError`]. The four
    /// short-circuit conditions are:
    ///
    /// * `strip_height == 0` — the §5.4 spec's `while (rows_remaining)`
    ///   guard exits at the first check and the function returns
    ///   `Ok(0)` (zero rows walked) without touching the buffer.
    /// * `strip_width == 0` — yields
    ///   [`StripEdgeApplyError::ZeroWidthStrip`] because the
    ///   `[edi - 1]` source position falls before the row's leading
    ///   cursor.
    /// * `strip_width >= STRIP_EDGE_ROW_STRIDE` — yields
    ///   [`StripEdgeApplyError::WidthExceedsRowStride`] because the
    ///   write at row-relative offset `strip_width` would land inside
    ///   the next row's leading cursor, violating the `spec/03 §5.2`
    ///   "strip width sits strictly inside the allocated row stride"
    ///   invariant.
    /// * The slice has fewer bytes than `strip_height *
    ///   STRIP_EDGE_ROW_STRIDE` — yields
    ///   [`StripEdgeApplyError::BufferTooShort`] with the required +
    ///   supplied byte counts.
    ///
    /// Returns `Ok(rows_walked)` on success, where `rows_walked`
    /// equals `strip_height` (the iterator emits one byte-copy per
    /// row top-to-bottom).
    pub fn apply_to_buffer(self, pixel_buffer: &mut [u8]) -> Result<u32, StripEdgeApplyError> {
        if self.strip_height == 0 {
            return Ok(0);
        }
        if self.strip_width == 0 {
            return Err(StripEdgeApplyError::ZeroWidthStrip);
        }
        let width = self.strip_width as usize;
        if width >= STRIP_EDGE_ROW_STRIDE {
            return Err(StripEdgeApplyError::WidthExceedsRowStride {
                supplied_width: self.strip_width,
                row_stride: STRIP_EDGE_ROW_STRIDE,
            });
        }
        let required_bytes = (self.strip_height as usize)
            .checked_mul(STRIP_EDGE_ROW_STRIDE)
            .ok_or(StripEdgeApplyError::BufferTooShort {
                required_bytes: usize::MAX,
                supplied_bytes: pixel_buffer.len(),
            })?;
        if pixel_buffer.len() < required_bytes {
            return Err(StripEdgeApplyError::BufferTooShort {
                required_bytes,
                supplied_bytes: pixel_buffer.len(),
            });
        }

        // §5.4 per-row body: read `[edi - 1]`, write `[edi]`. With the
        // row's leading cursor at `r * 0xb0`, `edi` for the rightmost-
        // column slot sits at `r * 0xb0 + strip_width`; the read at
        // `edi - 1` is therefore at `r * 0xb0 + strip_width - 1` and
        // the write at `edi` is at `r * 0xb0 + strip_width`.
        for row in 0..self.strip_height {
            let row_base = (row as usize) * STRIP_EDGE_ROW_STRIDE;
            let read_at = row_base + width - 1;
            let write_at = row_base + width;
            pixel_buffer[write_at] = pixel_buffer[read_at];
        }
        Ok(self.strip_height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::strip_context::STRIP_SLOT_COUNT;

    // ---- §5.4 (shift + stride constants) ---------------------------

    #[test]
    fn strip_edge_chroma_shift_matches_spec() {
        // §5.4 / spec/02 §4.1: chroma is 4:1 subsampled →
        // `sar edx, 0x2` / `sar eax, 0x2`.
        assert_eq!(STRIP_EDGE_CHROMA_SHIFT, 2);
        assert_eq!(strip_edge_chroma_shift(), 2);
        // Worked example: 160 luma → 40 chroma.
        assert_eq!(160u32 >> STRIP_EDGE_CHROMA_SHIFT, 40);
        // 480 luma → 120 chroma.
        assert_eq!(480u32 >> STRIP_EDGE_CHROMA_SHIFT, 120);
    }

    #[test]
    fn strip_edge_row_stride_matches_b0() {
        // §5.4 walks rows at the `0xb0` stride (= 176 bytes).
        assert_eq!(STRIP_EDGE_ROW_STRIDE, 0xb0);
        assert_eq!(STRIP_EDGE_ROW_STRIDE, 176);
        assert_eq!(strip_edge_row_step(), 0xb0);
    }

    #[test]
    fn strip_edge_row_stride_matches_per_cell_fixup_stride() {
        // §5.4 and §5.5 share the `0xb0` row stride (the strip's
        // allocated row stride, not the picture's external stride).
        assert_eq!(STRIP_EDGE_ROW_STRIDE, PER_CELL_EDGE_ROW_STRIDE);
    }

    #[test]
    fn strip_edge_byte_copy_offsets_match_spec() {
        // §5.4: `mov al, [edi - 1]; mov [edi], al`.
        assert_eq!(STRIP_EDGE_BYTE_READ_OFFSET, -1);
        assert_eq!(STRIP_EDGE_BYTE_WRITE_OFFSET, 0);
        assert_eq!(strip_edge_byte_copy_offsets(), (-1, 0));
        // The write cursor is one byte to the *right* of the source
        // (the rightmost-column duplication direction).
        assert_eq!(
            STRIP_EDGE_BYTE_WRITE_OFFSET - STRIP_EDGE_BYTE_READ_OFFSET,
            1
        );
    }

    // ---- §5.4 (per-slot dimensions) --------------------------------

    #[test]
    fn for_slot_luma_preserves_fields_verbatim() {
        // §5.4 luma path: `ebx == 0 || ebx == 3`; no `sar 2` applied.
        let dims = StripEdgeFixupDims::for_slot(0, 240, 160).unwrap();
        assert_eq!(dims.strip_height, 240);
        assert_eq!(dims.strip_width, 160);
        assert_eq!(dims.plane_role, PlaneRole::Luma);
        assert!(dims.is_luma());
        assert!(!dims.is_chroma());

        let dims = StripEdgeFixupDims::for_slot(3, 240, 160).unwrap();
        assert_eq!(dims.strip_height, 240);
        assert_eq!(dims.strip_width, 160);
        assert!(dims.is_luma());
    }

    #[test]
    fn for_slot_chroma_divides_both_fields_by_4() {
        // §5.4 chroma path: `ebx ∈ {1, 2, 4, 5}`; `sar 2` on both.
        for slot in [1usize, 2, 4, 5] {
            let dims = StripEdgeFixupDims::for_slot(slot, 240, 160).unwrap();
            assert_eq!(dims.strip_height, 60, "slot={}", slot);
            assert_eq!(dims.strip_width, 40, "slot={}", slot);
            assert_eq!(dims.plane_role, PlaneRole::Chroma);
            assert!(!dims.is_luma());
            assert!(dims.is_chroma());
        }
    }

    #[test]
    fn for_slot_scratch_returns_none() {
        // Scratch slots (6..31) are not dispatched into the §5.4
        // fix-up by the per-plane decode call.
        for slot in [6usize, 10, 31] {
            assert_eq!(
                StripEdgeFixupDims::for_slot(slot, 240, 160),
                None,
                "slot={}",
                slot
            );
        }
    }

    #[test]
    fn for_slot_chroma_handles_remainder_strip_widths() {
        // Worked example: a luma strip-width of 0xa0 (= 160) yields
        // 40 chroma; a remainder strip with luma width 80 yields 20
        // chroma; a remainder with luma width 64 yields 16 chroma.
        for (luma_w, expected_chroma_w) in [(160u32, 40u32), (80, 20), (64, 16), (4, 1)] {
            let dims = StripEdgeFixupDims::for_slot(1, 240, luma_w).unwrap();
            assert_eq!(dims.strip_width, expected_chroma_w, "luma_w={}", luma_w);
        }
    }

    #[test]
    fn for_slot_chroma_truncates_with_sar() {
        // `sar 2` on a value not a multiple of 4 truncates toward
        // zero for non-negative inputs (same as `>> 2` for `u32`).
        let dims = StripEdgeFixupDims::for_slot(1, 7, 17).unwrap();
        assert_eq!(dims.strip_height, 1); // 7 >> 2 = 1
        assert_eq!(dims.strip_width, 4); // 17 >> 2 = 4
    }

    #[test]
    fn for_slot_rejects_out_of_range_slot_via_scratch_role() {
        // PlaneRole::for_slot already classifies slot 32+ as Scratch;
        // for_slot inherits the same disposition.
        assert_eq!(
            StripEdgeFixupDims::for_slot(STRIP_SLOT_COUNT, 240, 160),
            None
        );
        assert_eq!(StripEdgeFixupDims::for_slot(usize::MAX, 240, 160), None);
    }

    // ---- §5.4 (per-row iteration) ----------------------------------

    #[test]
    fn row_iter_zero_height_yields_nothing() {
        let iter = StripEdgeRowIter::new(0);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        let rows: Vec<_> = iter.collect();
        assert!(rows.is_empty());
    }

    #[test]
    fn row_iter_single_row_yields_one() {
        let rows: Vec<_> = StripEdgeRowIter::new(1).collect();
        assert_eq!(rows.len(), 1);
        let r = rows[0];
        assert_eq!(r.row_index, 0);
        assert_eq!(r.row_cursor_byte_offset, 0);
        assert_eq!(r.read_offset, -1);
        assert_eq!(r.write_offset, 0);
    }

    #[test]
    fn row_iter_strides_rows_by_b0() {
        // 4 rows at stride 0xb0: cursor offsets 0, 0xb0, 0x160, 0x210.
        let rows: Vec<_> = StripEdgeRowIter::new(4).collect();
        assert_eq!(rows.len(), 4);
        for (k, row) in rows.iter().enumerate() {
            assert_eq!(row.row_index as usize, k);
            assert_eq!(row.row_cursor_byte_offset, k * STRIP_EDGE_ROW_STRIDE);
            assert_eq!(row.read_offset, STRIP_EDGE_BYTE_READ_OFFSET);
            assert_eq!(row.write_offset, STRIP_EDGE_BYTE_WRITE_OFFSET);
        }
        assert_eq!(rows[1].row_cursor_byte_offset, 0xb0);
        assert_eq!(rows[2].row_cursor_byte_offset, 0x160);
        assert_eq!(rows[3].row_cursor_byte_offset, 0x210);
    }

    #[test]
    fn row_iter_exhaustion_reports_size_hint_correctly() {
        let mut iter = StripEdgeRowIter::new(3);
        assert_eq!(iter.size_hint(), (3, Some(3)));
        let _ = iter.next();
        assert_eq!(iter.size_hint(), (2, Some(2)));
        let _ = iter.next();
        let _ = iter.next();
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert!(iter.next().is_none());
    }

    #[test]
    fn row_iter_is_exact_size() {
        // The strip-edge fix-up walks a known number of rows; the
        // iterator must report it exactly so callers can pre-size
        // any per-row buffers they keep.
        let iter = StripEdgeRowIter::new(120);
        assert_eq!(iter.len(), 120);
    }

    #[test]
    fn dims_row_iter_walks_chroma_height_after_divide() {
        // A chroma slot whose stored height is 240 walks 60 rows
        // (after `sar 2`).
        let dims = StripEdgeFixupDims::for_slot(1, 240, 160).unwrap();
        let rows: Vec<_> = dims.row_iter().collect();
        assert_eq!(rows.len(), 60);
        assert_eq!(rows.first().unwrap().row_index, 0);
        assert_eq!(rows.last().unwrap().row_index, 59);
    }

    #[test]
    fn dims_row_iter_walks_luma_height_at_full_resolution() {
        // A luma slot whose stored height is 240 walks 240 rows.
        let dims = StripEdgeFixupDims::for_slot(0, 240, 160).unwrap();
        let rows: Vec<_> = dims.row_iter().collect();
        assert_eq!(rows.len(), 240);
        assert_eq!(rows.last().unwrap().row_index, 239);
        assert_eq!(
            rows.last().unwrap().row_cursor_byte_offset,
            239 * STRIP_EDGE_ROW_STRIDE
        );
    }

    // ---- §5.4 (per-row byte-copy executor) -------------------------

    #[test]
    fn apply_to_buffer_zero_height_returns_zero_rows() {
        // §5.4: zero-row strip walks no rows. The buffer is untouched.
        let dims = StripEdgeFixupDims {
            strip_height: 0,
            strip_width: 160,
            plane_role: PlaneRole::Luma,
        };
        let mut buf = vec![0xaau8; STRIP_EDGE_ROW_STRIDE];
        let rows = dims.apply_to_buffer(&mut buf).unwrap();
        assert_eq!(rows, 0);
        // Buffer untouched.
        for b in &buf {
            assert_eq!(*b, 0xaa);
        }
    }

    #[test]
    fn apply_to_buffer_zero_width_errors() {
        // §5.4: zero-width strip has no `[edi - 1]` source.
        let dims = StripEdgeFixupDims {
            strip_height: 4,
            strip_width: 0,
            plane_role: PlaneRole::Luma,
        };
        let mut buf = vec![0u8; 4 * STRIP_EDGE_ROW_STRIDE];
        assert_eq!(
            dims.apply_to_buffer(&mut buf),
            Err(StripEdgeApplyError::ZeroWidthStrip)
        );
    }

    #[test]
    fn apply_to_buffer_width_at_stride_errors() {
        // §5.4 / §5.2: visible width must sit strictly inside the
        // 0xb0 row stride; width == stride leaves no padding column
        // for the duplicated byte.
        let dims = StripEdgeFixupDims {
            strip_height: 1,
            strip_width: STRIP_EDGE_ROW_STRIDE as u32,
            plane_role: PlaneRole::Luma,
        };
        let mut buf = vec![0u8; STRIP_EDGE_ROW_STRIDE];
        assert_eq!(
            dims.apply_to_buffer(&mut buf),
            Err(StripEdgeApplyError::WidthExceedsRowStride {
                supplied_width: STRIP_EDGE_ROW_STRIDE as u32,
                row_stride: STRIP_EDGE_ROW_STRIDE,
            })
        );
    }

    #[test]
    fn apply_to_buffer_too_short_errors() {
        // §5.4 walks `strip_height * 0xb0` bytes; a slice with fewer
        // bytes yields BufferTooShort with both counts.
        let dims = StripEdgeFixupDims {
            strip_height: 4,
            strip_width: 160,
            plane_role: PlaneRole::Luma,
        };
        let supplied = 3 * STRIP_EDGE_ROW_STRIDE;
        let mut buf = vec![0u8; supplied];
        assert_eq!(
            dims.apply_to_buffer(&mut buf),
            Err(StripEdgeApplyError::BufferTooShort {
                required_bytes: 4 * STRIP_EDGE_ROW_STRIDE,
                supplied_bytes: supplied,
            })
        );
    }

    #[test]
    fn apply_to_buffer_single_row_duplicates_byte() {
        // §5.4 per-row body: read `[r*0xb0 + width - 1]`, write
        // `[r*0xb0 + width]`. For a single-row luma strip of
        // width 160, the byte at offset 159 (the rightmost stored
        // pixel) gets copied to offset 160 (the padding slot).
        let dims = StripEdgeFixupDims {
            strip_height: 1,
            strip_width: 160,
            plane_role: PlaneRole::Luma,
        };
        let mut buf = vec![0u8; STRIP_EDGE_ROW_STRIDE];
        buf[159] = 0x5a;
        buf[160] = 0x00; // padding slot before the walk
        let rows = dims.apply_to_buffer(&mut buf).unwrap();
        assert_eq!(rows, 1);
        assert_eq!(buf[159], 0x5a, "source byte preserved");
        assert_eq!(
            buf[160], 0x5a,
            "destination padding byte takes the rightmost-column value"
        );
    }

    #[test]
    fn apply_to_buffer_chroma_strip_walks_quarter_rows() {
        // §5.4 chroma path: a strip with stored height 240 and width
        // 160 walks 60 rows at width 40 after the `sar 2` divide
        // (per StripEdgeFixupDims::for_slot's chroma branch).
        let dims = StripEdgeFixupDims::for_slot(1, 240, 160).unwrap();
        assert_eq!(dims.strip_height, 60);
        assert_eq!(dims.strip_width, 40);
        let mut buf = vec![0u8; 60 * STRIP_EDGE_ROW_STRIDE];
        // Seed each row's rightmost-stored byte (offset width-1=39
        // within the row) with a row-distinct sentinel.
        for r in 0..60usize {
            buf[r * STRIP_EDGE_ROW_STRIDE + 39] = (r + 1) as u8;
        }
        let rows = dims.apply_to_buffer(&mut buf).unwrap();
        assert_eq!(rows, 60);
        // Each row's offset 40 (the padding slot) now mirrors offset 39.
        for r in 0..60usize {
            assert_eq!(
                buf[r * STRIP_EDGE_ROW_STRIDE + 40],
                (r + 1) as u8,
                "row={r} padding-slot mirrors the rightmost-column byte"
            );
        }
    }

    #[test]
    fn apply_to_buffer_leaves_non_padding_bytes_untouched() {
        // §5.4 writes ONE byte per row (the padding slot at offset
        // `width`). Every other byte in the buffer is left as the
        // caller supplied it.
        let dims = StripEdgeFixupDims {
            strip_height: 4,
            strip_width: 160,
            plane_role: PlaneRole::Luma,
        };
        let mut buf = vec![0xeeu8; 4 * STRIP_EDGE_ROW_STRIDE];
        // Place a unique byte at each row's rightmost-stored slot.
        for r in 0..4usize {
            buf[r * STRIP_EDGE_ROW_STRIDE + 159] = 0x11 + (r as u8);
        }
        let _ = dims.apply_to_buffer(&mut buf).unwrap();
        for r in 0..4usize {
            for c in 0..STRIP_EDGE_ROW_STRIDE {
                let expected = match c {
                    // The rightmost-stored slot preserved.
                    159 => 0x11 + (r as u8),
                    // The padding slot now mirrors offset 159.
                    160 => 0x11 + (r as u8),
                    // Everything else stays at the 0xee fill.
                    _ => 0xee,
                };
                assert_eq!(
                    buf[r * STRIP_EDGE_ROW_STRIDE + c],
                    expected,
                    "row={r} col={c}"
                );
            }
        }
    }

    #[test]
    fn apply_to_buffer_accepts_oversize_buffer() {
        // §5.4 is happy with a buffer that has more bytes than the
        // strict minimum; only the first `strip_height * 0xb0` bytes
        // are touched.
        let dims = StripEdgeFixupDims {
            strip_height: 2,
            strip_width: 40,
            plane_role: PlaneRole::Chroma,
        };
        let oversize = 10 * STRIP_EDGE_ROW_STRIDE;
        let mut buf = vec![0xa5u8; oversize];
        buf[39] = 0x77;
        buf[STRIP_EDGE_ROW_STRIDE + 39] = 0x88;
        let rows = dims.apply_to_buffer(&mut buf).unwrap();
        assert_eq!(rows, 2);
        assert_eq!(buf[40], 0x77, "row 0 padding byte");
        assert_eq!(buf[STRIP_EDGE_ROW_STRIDE + 40], 0x88, "row 1 padding byte");
        // Beyond `strip_height * 0xb0` the buffer is untouched.
        for b in buf.iter().skip(2 * STRIP_EDGE_ROW_STRIDE) {
            assert_eq!(*b, 0xa5);
        }
    }

    #[test]
    fn apply_to_buffer_via_for_slot_walks_luma_full_height() {
        // §5.4 luma path: a slot 0 with stored height 480 walks 480
        // rows; the function returns the same count as
        // dims.strip_height.
        let dims = StripEdgeFixupDims::for_slot(0, 480, 160).unwrap();
        assert_eq!(dims.strip_height, 480);
        assert_eq!(dims.strip_width, 160);
        let mut buf = vec![0u8; 480 * STRIP_EDGE_ROW_STRIDE];
        let rows = dims.apply_to_buffer(&mut buf).unwrap();
        assert_eq!(rows, 480);
    }

    #[test]
    fn apply_error_display_messages_cite_spec() {
        // Diagnostic surface: every variant cites spec/03 §5.4 by
        // name so callers debugging a corrupt strip have an
        // unambiguous source citation.
        let e1 = StripEdgeApplyError::ZeroWidthStrip;
        assert!(format!("{e1}").contains("spec/03 §5.4"));
        let e2 = StripEdgeApplyError::BufferTooShort {
            required_bytes: 256,
            supplied_bytes: 128,
        };
        let s2 = format!("{e2}");
        assert!(s2.contains("spec/03 §5.4"));
        assert!(s2.contains("256"));
        assert!(s2.contains("128"));
        let e3 = StripEdgeApplyError::WidthExceedsRowStride {
            supplied_width: 200,
            row_stride: STRIP_EDGE_ROW_STRIDE,
        };
        let s3 = format!("{e3}");
        assert!(s3.contains("spec/03 §5.4"));
        assert!(s3.contains("200"));
    }
}
