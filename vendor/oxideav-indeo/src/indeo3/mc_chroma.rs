//! Indeo 3 spec/05 §5.5 — chroma-plane scaling for the MC fetcher.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/05-motion-compensation.md`
//! §5.5 (the four-bullet disposition that pins the MC fetcher's
//! behaviour on the chroma slot indices `1, 2, 4, 5` relative to the
//! luma slot indices `0, 3`) and cross-references: `spec/02 §4.1`
//! (the chroma strip's nominal 40-pixel visible width versus the
//! luma 160), `spec/02 §5.1` (the chroma vs luma slot-index split),
//! `spec/03 §5.2` (the strip's allocated row-stride `0xb0`),
//! `spec/04 §5.3` (the codec-init step that populates the chroma
//! cell sizes into the codebook-bank tables), and `spec/05 §3.3`
//! (the packed-MV `176`-factor as a buffer-allocation constant).
//!
//! Round 11 ([`super::strip_edge`]) lands the strip-edge fix-up
//! parameter surface that already pins the chroma `sar 2` halving
//! at the strip-edge level. Round 14 ([`super::mc_kernel`]) lands
//! the §5.1 / §5.2 inner-loop geometry that the §5.5 disposition
//! re-uses verbatim for chroma. Round 17 ([`super::mc_arena`])
//! lands the per-strip region size; the §5.5 disposition is that
//! the *row stride* is constant across luma and chroma, while the
//! *visible width* and *cell size* shrink 4:1. This module owns
//! §5.5 — the typed surface that (a) the MC inner-loop kernel
//! geometry is plane-role-invariant, (b) the chroma strip's
//! visible width is the luma width divided by `LUMA_PIXEL_PER_CHROMA_PIXEL`,
//! (c) the codebook-bank cell-size populations are pre-subsampled
//! by `LUMA_PIXEL_PER_CHROMA_PIXEL` in each axis (host-side per
//! `spec/04 §5.3`, this module only pins the disposition), and
//! (d) the packed-MV `176`-factor is a buffer-allocation constant,
//! not a plane-resolution constant.
//!
//! This module surfaces:
//!
//! * [`LUMA_PIXEL_PER_CHROMA_PIXEL`] — the §5.5 4:1 subsampling
//!   ratio on each axis (horizontal and vertical).
//! * [`CHROMA_PACKED_MV_FACTOR_IS_BUFFER_STRIDE`] — the §5.5
//!   typed disposition (`= true`) that the §3.3 `176`-factor in
//!   the packed-MV formula is the allocated buffer stride, not a
//!   plane-resolution constant, and applies uniformly to luma and
//!   chroma planes.
//! * [`McPlaneRole`] — a typed surface enum (`Luma` / `Chroma`)
//!   for the §5.1 split between luma slots `0, 3` and chroma slots
//!   `1, 2, 4, 5`. The enum is local to this module so the §5.5
//!   surface does not have to reach into the [`super::strip_context`]
//!   `PlaneRole` (which carries different invariants).
//! * [`McPlaneRole::from_strip_slot_index`] — the §5.1 slot-index
//!   classifier (`0, 3` ⇒ luma; `1, 2, 4, 5` ⇒ chroma; other
//!   indices ⇒ `None`).
//! * [`McPlaneRole::strip_visible_width`] — returns
//!   [`super::LUMA_STRIP_WIDTH`] for [`McPlaneRole::Luma`] and
//!   [`super::CHROMA_STRIP_WIDTH`] for [`McPlaneRole::Chroma`].
//! * [`McPlaneRole::strip_allocated_row_stride`] — returns
//!   [`super::MC_ARENA_ROW_STRIDE`] for both variants (the §5.5
//!   second-bullet "the row stride remains the constant `0xb0`,
//!   the buffer's allocated stride, not the strip's *visible*
//!   width" disposition).
//! * [`McPlaneRole::chroma_cell_size`] — applies the §5.5
//!   third-bullet 4:1 / 4:1 subsampling to a candidate luma
//!   `(width, height)` cell size in pixels and returns the
//!   corresponding chroma `(width, height)` (rounded-down division;
//!   the chroma cell-size populations in the codebook-bank tables
//!   are integer pixel counts).
//! * [`McKernelGeometryIsPlaneRoleInvariant`] — the §5.5 first-
//!   bullet typed disposition (`= true`) that the MC fetcher inner
//!   loop is structurally identical for chroma; the geometry
//!   constants [`super::MC_BAND_BYTE_STRIDE`],
//!   [`super::MC_BAND_ROWS`], [`super::MC_BYTES_PER_DWORD`],
//!   [`super::MC_INNER_LOOP_BYTES_PER_ITER`], and
//!   [`super::MC_INNER_LOOP_DWORDS_PER_ITER`] are plane-role-
//!   invariant.
//! * [`MvPixelOffsetInterpretation`] — the §5.5 fourth-bullet
//!   typed disposition (`LumaOrChromaUniformBufferStride`) that the
//!   packed-MV's `pixel_offset` is interpreted as a
//!   plane-resolution-displacement against the buffer's row
//!   stride, uniformly across luma and chroma planes.
//!
//! What this module **deliberately does not do** (the §5.5 chapter
//! boundary):
//!
//! * It does not perform the codec-init population of the
//!   codebook-bank `+0x000` / `+0x100` sub-tables with chroma cell
//!   sizes. That's host-side per `spec/04 §5.3`; this module only
//!   pins the §5.5 disposition that those populations are 4:1 / 4:1
//!   subsampled when the slot index is chroma.
//! * It does not perform the §5.1 inner-loop reads / writes. The
//!   inner loop is owned by [`super::mc_kernel`] and is itself
//!   plane-role-invariant per the §5.5 first bullet.
//! * It does not perform the §2.3 source-pointer arithmetic. The
//!   `add esi, sign_extend(packed >> 2)` site is owned by
//!   [`super::apply_mv_source_offset`]; this module only pins the
//!   §5.5 fourth-bullet disposition that the formula's `176`-
//!   factor is the buffer-allocated row stride and not a plane-
//!   resolution constant.
//! * It does not derive the luma-vs-chroma slot-index split itself
//!   beyond the §5.1 cross-reference; [`super::strip_context`]'s
//!   [`super::PlaneRole`] owns that split for the strip-context
//!   array dimension, and this module's [`McPlaneRole`] is the
//!   smaller §5.5-scoped surface that classifies a slot index for
//!   the MC fetcher specifically.

use super::{
    CHROMA_STRIP_WIDTH, LUMA_STRIP_WIDTH, MC_ARENA_ROW_STRIDE, MV_PIXEL_OFFSET_ROW_STRIDE,
};

// ---- §5.5 third-bullet: 4:1 / 4:1 subsampling ratio --------------

/// Spec/05 §5.5 third bullet — the 4:1 horizontal × 4:1 vertical
/// subsampling ratio between a luma pixel and the corresponding
/// chroma pixel. The IV3 codec uses YVU9 (per `spec/01` and
/// `spec/02 §4.1`), where each chroma sample covers a `4 × 4`
/// luma-pixel block.
///
/// Used by [`McPlaneRole::chroma_cell_size`] to scale a luma cell
/// size to its chroma counterpart.
pub const LUMA_PIXEL_PER_CHROMA_PIXEL: u32 = 4;

/// `const _` cross-check that the §5.5 4:1 ratio is consistent
/// with the §2 / §4.1 luma vs chroma strip-width split.
const _: () = assert!(LUMA_STRIP_WIDTH == CHROMA_STRIP_WIDTH * LUMA_PIXEL_PER_CHROMA_PIXEL);

// ---- §5.5 fourth-bullet: packed-MV factor is a buffer constant ---

/// Spec/05 §5.5 fourth bullet — typed disposition that the §3.3
/// packed-MV `176`-factor (i.e. [`super::MV_PIXEL_OFFSET_ROW_STRIDE`])
/// is the allocated buffer row stride and not a plane-resolution
/// constant. The disposition is therefore that the packed-MV
/// formula applies uniformly to luma and chroma planes.
///
/// Surfaces the disposition as a typed `const`-`true` flag so the
/// §3.3 packing-formula site has a greppable cross-reference to
/// the §5.5 chroma-plane disposition.
pub const CHROMA_PACKED_MV_FACTOR_IS_BUFFER_STRIDE: bool = true;

/// `const _` cross-check that the §3.3 `176`-factor used by the
/// packed-MV pixel-offset formula is exactly the buffer-allocated
/// row stride [`super::MC_ARENA_ROW_STRIDE`], confirming the §5.5
/// fourth-bullet disposition at the typed surface.
const _: () = assert!(MV_PIXEL_OFFSET_ROW_STRIDE as usize == MC_ARENA_ROW_STRIDE);

// ---- §5.5 first-bullet: kernel-geometry plane-role-invariance ----

/// Spec/05 §5.5 first bullet — typed disposition that the MC
/// fetcher's inner-loop geometry is identical for luma and chroma
/// planes. The four constants
/// [`super::MC_BAND_BYTE_STRIDE`] / [`super::MC_BAND_ROWS`] /
/// [`super::MC_BYTES_PER_DWORD`] /
/// [`super::MC_INNER_LOOP_BYTES_PER_ITER`] /
/// [`super::MC_INNER_LOOP_DWORDS_PER_ITER`] are therefore not
/// parameterised on plane role.
///
/// Surfaces the disposition as a typed `const`-`true` flag.
pub const MC_KERNEL_GEOMETRY_IS_PLANE_ROLE_INVARIANT: bool = true;

/// Spec/05 §5.5 first-bullet typed-surface alias for callers that
/// want a documented-shape symbol at the kernel-call site (the
/// `MC_KERNEL_GEOMETRY_IS_PLANE_ROLE_INVARIANT` const above is
/// shorter; this re-export is the long-form name the §5.5 surface
/// is referenced by in upstream call sites).
pub use MC_KERNEL_GEOMETRY_IS_PLANE_ROLE_INVARIANT as McKernelGeometryIsPlaneRoleInvariant;

// ---- §5.5 fourth-bullet: packed-MV interpretation surface --------

/// Spec/05 §5.5 fourth-bullet typed-surface enum.
///
/// The packed-MV's high-30-bits `pixel_offset` field is *always*
/// interpreted as `vert * 176 + horiz`, where the `176` is the
/// buffer-allocated row stride and not the plane's visible width.
/// The enum carries the single documented variant; the absence of
/// per-plane variants is itself the §5.5 disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MvPixelOffsetInterpretation {
    /// Uniform buffer-stride interpretation: the `176`-factor in
    /// the §3.3 packed-MV formula is
    /// [`super::MV_PIXEL_OFFSET_ROW_STRIDE`] regardless of whether
    /// the destination slot is a luma slot or a chroma slot.
    LumaOrChromaUniformBufferStride,
}

impl MvPixelOffsetInterpretation {
    /// Returns the §3.3 packed-MV row-stride factor (i.e.
    /// [`super::MV_PIXEL_OFFSET_ROW_STRIDE`]) regardless of the
    /// variant. The method exists so the §5.5 fourth-bullet
    /// disposition is callable from a typed-surface site.
    pub const fn pixel_offset_row_stride(self) -> i32 {
        MV_PIXEL_OFFSET_ROW_STRIDE
    }
}

// ---- §5.1 / §5.5 slot-index classifier --------------------------

/// Spec/05 §5.1 + §5.5 typed-surface enum for the MC fetcher's
/// plane-role classification of a strip-context slot index.
///
/// Local to this module so the §5.5 chroma-scaling surface does
/// not couple to [`super::strip_context`]'s [`super::PlaneRole`]
/// (which carries the strip-context array dimension's invariants;
/// the §5.5 surface only needs the §5.1 split between luma and
/// chroma slot indices for the MC fetcher).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum McPlaneRole {
    /// Luma slot index — the §5.1 indices `0` and `3`.
    Luma,
    /// Chroma slot index — the §5.1 indices `1`, `2`, `4`, `5`.
    Chroma,
}

impl McPlaneRole {
    /// Spec/05 §5.1 — classify a strip-context slot index into the
    /// §5.5 luma vs chroma split. Returns `None` for slot indices
    /// outside the §5.1 `0..=5` range.
    ///
    /// The luma indices are `0` and `3` (per `spec/02 §5.1`'s slot
    /// layout: slot `0` is the luma slot for the first Y plane and
    /// slot `3` is the luma slot for the second Y plane on a YVU9
    /// frame with both Y / V / U planes present). All other in-
    /// range indices (`1`, `2`, `4`, `5`) are chroma.
    pub const fn from_strip_slot_index(slot: u32) -> Option<Self> {
        match slot {
            0 | 3 => Some(McPlaneRole::Luma),
            1 | 2 | 4 | 5 => Some(McPlaneRole::Chroma),
            _ => None,
        }
    }

    /// Returns the strip's *visible* width in pixels for this role.
    /// Per the §5.5 first / second bullets, this is
    /// [`super::LUMA_STRIP_WIDTH`] for luma and
    /// [`super::CHROMA_STRIP_WIDTH`] for chroma.
    pub const fn strip_visible_width(self) -> u32 {
        match self {
            McPlaneRole::Luma => LUMA_STRIP_WIDTH,
            McPlaneRole::Chroma => CHROMA_STRIP_WIDTH,
        }
    }

    /// Returns the strip's *allocated* row stride in bytes for this
    /// role. Per the §5.5 second bullet, the allocated row stride
    /// is the constant [`super::MC_ARENA_ROW_STRIDE`] (`= 0xb0`)
    /// for both luma and chroma — the chroma plane's smaller
    /// visible width does **not** propagate into the row stride.
    pub const fn strip_allocated_row_stride(self) -> usize {
        let _ = self;
        MC_ARENA_ROW_STRIDE
    }

    /// Returns the §5.5 third-bullet 4:1 subsampling ratio (each
    /// axis) when this role is [`McPlaneRole::Chroma`], and `1`
    /// when this role is [`McPlaneRole::Luma`]. Used by
    /// [`Self::chroma_cell_size`].
    pub const fn cell_size_subsampling_ratio(self) -> u32 {
        match self {
            McPlaneRole::Luma => 1,
            McPlaneRole::Chroma => LUMA_PIXEL_PER_CHROMA_PIXEL,
        }
    }

    /// Returns whether this role corresponds to a luma slot.
    pub const fn is_luma(self) -> bool {
        matches!(self, McPlaneRole::Luma)
    }

    /// Returns whether this role corresponds to a chroma slot.
    pub const fn is_chroma(self) -> bool {
        matches!(self, McPlaneRole::Chroma)
    }

    /// Spec/05 §5.5 third bullet — apply the 4:1 / 4:1 subsampling
    /// to a luma cell-size pair `(width_pixels, height_pixels)`,
    /// returning the corresponding chroma cell-size pair.
    ///
    /// The returned pair is the integer-pixel cell size that the
    /// codec-init step populates into the codebook-bank tables for
    /// chroma planes (per `spec/04 §5.3`, host-side; this method
    /// surfaces the §5.5 arithmetic, not the population itself).
    ///
    /// Returns `None` if either luma dimension is not an exact
    /// multiple of [`LUMA_PIXEL_PER_CHROMA_PIXEL`]; the §5.5 bullet
    /// is "the cell sizes encoded in the codebook-bank tables are
    /// subsampled by the 4:1 horizontal × 4:1 vertical ratio",
    /// which only makes sense for exact multiples (a half-pixel
    /// chroma cell would not round-trip through the integer cell-
    /// width counters in the codec-init population).
    pub const fn chroma_cell_size(luma_width: u32, luma_height: u32) -> Option<(u32, u32)> {
        let r = LUMA_PIXEL_PER_CHROMA_PIXEL;
        if !luma_width.is_multiple_of(r) || !luma_height.is_multiple_of(r) {
            return None;
        }
        Some((luma_width / r, luma_height / r))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        MC_BAND_BYTE_STRIDE, MC_BAND_ROWS, MC_BYTES_PER_DWORD, MC_INNER_LOOP_BYTES_PER_ITER,
        MC_INNER_LOOP_DWORDS_PER_ITER,
    };
    use super::*;

    // ---- LUMA_PIXEL_PER_CHROMA_PIXEL ----

    #[test]
    fn luma_pixel_per_chroma_pixel_is_4() {
        assert_eq!(LUMA_PIXEL_PER_CHROMA_PIXEL, 4);
    }

    #[test]
    fn luma_pixel_per_chroma_pixel_consistent_with_strip_widths() {
        // §5.5 third bullet — the 4:1 ratio is consistent with the
        // luma vs chroma strip widths.
        assert_eq!(
            LUMA_STRIP_WIDTH,
            CHROMA_STRIP_WIDTH * LUMA_PIXEL_PER_CHROMA_PIXEL
        );
    }

    // ---- CHROMA_PACKED_MV_FACTOR_IS_BUFFER_STRIDE ----

    #[test]
    fn chroma_packed_mv_factor_is_buffer_stride_holds() {
        // §5.5 fourth-bullet disposition: the packed-MV `176`-factor
        // is the buffer-allocated row stride.
        assert!(core::hint::black_box(
            CHROMA_PACKED_MV_FACTOR_IS_BUFFER_STRIDE
        ));
    }

    #[test]
    fn packed_mv_factor_matches_arena_row_stride() {
        assert_eq!(MV_PIXEL_OFFSET_ROW_STRIDE as usize, MC_ARENA_ROW_STRIDE);
    }

    // ---- MC_KERNEL_GEOMETRY_IS_PLANE_ROLE_INVARIANT ----

    #[test]
    fn mc_kernel_geometry_invariant_flag_is_true() {
        assert!(core::hint::black_box(
            MC_KERNEL_GEOMETRY_IS_PLANE_ROLE_INVARIANT
        ));
        assert!(core::hint::black_box(McKernelGeometryIsPlaneRoleInvariant));
    }

    #[test]
    fn mc_kernel_geometry_constants_are_plane_role_invariant() {
        // §5.5 first-bullet documentation invariant — the kernel
        // geometry constants are not parameterised on plane role.
        // (The assertions are tautological by construction; their
        // purpose is to lock the §5.5 disposition into a
        // greppable test.)
        let _ = MC_BAND_BYTE_STRIDE;
        let _ = MC_BAND_ROWS;
        let _ = MC_BYTES_PER_DWORD;
        let _ = MC_INNER_LOOP_BYTES_PER_ITER;
        let _ = MC_INNER_LOOP_DWORDS_PER_ITER;
        // Cross-checks against derived equalities the existing
        // kernel module already asserts.
        assert_eq!(
            MC_INNER_LOOP_BYTES_PER_ITER,
            MC_INNER_LOOP_DWORDS_PER_ITER * MC_BYTES_PER_DWORD,
        );
    }

    // ---- MvPixelOffsetInterpretation ----

    #[test]
    fn mv_pixel_offset_interpretation_single_variant() {
        let v = MvPixelOffsetInterpretation::LumaOrChromaUniformBufferStride;
        assert_eq!(
            v,
            MvPixelOffsetInterpretation::LumaOrChromaUniformBufferStride
        );
    }

    #[test]
    fn mv_pixel_offset_interpretation_returns_buffer_stride() {
        let v = MvPixelOffsetInterpretation::LumaOrChromaUniformBufferStride;
        assert_eq!(v.pixel_offset_row_stride(), MV_PIXEL_OFFSET_ROW_STRIDE);
        assert_eq!(v.pixel_offset_row_stride(), 0xb0);
    }

    // ---- McPlaneRole::from_strip_slot_index ----

    #[test]
    fn from_strip_slot_index_luma_slots() {
        assert_eq!(
            McPlaneRole::from_strip_slot_index(0),
            Some(McPlaneRole::Luma)
        );
        assert_eq!(
            McPlaneRole::from_strip_slot_index(3),
            Some(McPlaneRole::Luma)
        );
    }

    #[test]
    fn from_strip_slot_index_chroma_slots() {
        assert_eq!(
            McPlaneRole::from_strip_slot_index(1),
            Some(McPlaneRole::Chroma)
        );
        assert_eq!(
            McPlaneRole::from_strip_slot_index(2),
            Some(McPlaneRole::Chroma)
        );
        assert_eq!(
            McPlaneRole::from_strip_slot_index(4),
            Some(McPlaneRole::Chroma)
        );
        assert_eq!(
            McPlaneRole::from_strip_slot_index(5),
            Some(McPlaneRole::Chroma)
        );
    }

    #[test]
    fn from_strip_slot_index_out_of_range() {
        assert_eq!(McPlaneRole::from_strip_slot_index(6), None);
        assert_eq!(McPlaneRole::from_strip_slot_index(7), None);
        assert_eq!(McPlaneRole::from_strip_slot_index(42), None);
        assert_eq!(McPlaneRole::from_strip_slot_index(u32::MAX), None);
    }

    #[test]
    fn from_strip_slot_index_full_in_range_coverage() {
        // §5.1 — every index in `0..=5` classifies as exactly one
        // of luma or chroma; never both, never neither.
        for slot in 0u32..=5 {
            let role =
                McPlaneRole::from_strip_slot_index(slot).expect("§5.1 in-range slot must classify");
            assert!(role.is_luma() ^ role.is_chroma());
        }
    }

    // ---- McPlaneRole::strip_visible_width ----

    #[test]
    fn strip_visible_width_luma_is_160() {
        assert_eq!(McPlaneRole::Luma.strip_visible_width(), LUMA_STRIP_WIDTH);
        assert_eq!(McPlaneRole::Luma.strip_visible_width(), 160);
    }

    #[test]
    fn strip_visible_width_chroma_is_40() {
        assert_eq!(
            McPlaneRole::Chroma.strip_visible_width(),
            CHROMA_STRIP_WIDTH
        );
        assert_eq!(McPlaneRole::Chroma.strip_visible_width(), 40);
    }

    // ---- McPlaneRole::strip_allocated_row_stride ----

    #[test]
    fn strip_allocated_row_stride_luma_is_0xb0() {
        assert_eq!(
            McPlaneRole::Luma.strip_allocated_row_stride(),
            MC_ARENA_ROW_STRIDE,
        );
        assert_eq!(McPlaneRole::Luma.strip_allocated_row_stride(), 0xb0);
    }

    #[test]
    fn strip_allocated_row_stride_chroma_is_0xb0() {
        // §5.5 second bullet — the row stride remains `0xb0` for
        // chroma, *not* the visible width 40.
        assert_eq!(
            McPlaneRole::Chroma.strip_allocated_row_stride(),
            MC_ARENA_ROW_STRIDE,
        );
        assert_eq!(McPlaneRole::Chroma.strip_allocated_row_stride(), 0xb0);
    }

    #[test]
    fn strip_allocated_row_stride_equal_across_roles() {
        // §5.5 second bullet documentation invariant.
        assert_eq!(
            McPlaneRole::Luma.strip_allocated_row_stride(),
            McPlaneRole::Chroma.strip_allocated_row_stride(),
        );
    }

    // ---- McPlaneRole::cell_size_subsampling_ratio ----

    #[test]
    fn cell_size_subsampling_ratio_luma_is_1() {
        assert_eq!(McPlaneRole::Luma.cell_size_subsampling_ratio(), 1);
    }

    #[test]
    fn cell_size_subsampling_ratio_chroma_is_4() {
        assert_eq!(
            McPlaneRole::Chroma.cell_size_subsampling_ratio(),
            LUMA_PIXEL_PER_CHROMA_PIXEL,
        );
        assert_eq!(McPlaneRole::Chroma.cell_size_subsampling_ratio(), 4);
    }

    // ---- McPlaneRole::is_luma / is_chroma ----

    #[test]
    fn is_luma_disjoint_from_is_chroma() {
        assert!(McPlaneRole::Luma.is_luma());
        assert!(!McPlaneRole::Luma.is_chroma());
        assert!(!McPlaneRole::Chroma.is_luma());
        assert!(McPlaneRole::Chroma.is_chroma());
    }

    // ---- McPlaneRole::chroma_cell_size ----

    #[test]
    fn chroma_cell_size_4_by_4_luma_to_1_by_1_chroma() {
        assert_eq!(McPlaneRole::chroma_cell_size(4, 4), Some((1, 1)));
    }

    #[test]
    fn chroma_cell_size_16_by_16_luma_to_4_by_4_chroma() {
        assert_eq!(McPlaneRole::chroma_cell_size(16, 16), Some((4, 4)));
    }

    #[test]
    fn chroma_cell_size_160_by_240_luma_to_40_by_60_chroma() {
        // §5.5 worked example — luma strip width 160 / chroma
        // strip width 40, luma height 240 / chroma height 60.
        assert_eq!(McPlaneRole::chroma_cell_size(160, 240), Some((40, 60)));
    }

    #[test]
    fn chroma_cell_size_non_multiple_width_returns_none() {
        // §5.5 third-bullet integer-multiple constraint.
        assert_eq!(McPlaneRole::chroma_cell_size(3, 4), None);
        assert_eq!(McPlaneRole::chroma_cell_size(5, 8), None);
    }

    #[test]
    fn chroma_cell_size_non_multiple_height_returns_none() {
        assert_eq!(McPlaneRole::chroma_cell_size(4, 3), None);
        assert_eq!(McPlaneRole::chroma_cell_size(8, 5), None);
    }

    #[test]
    fn chroma_cell_size_zero_passes_through() {
        // Zero is an exact multiple; the resulting chroma size is
        // also zero. The §5.5 disposition does not exclude zero-
        // sized cells at this layer.
        assert_eq!(McPlaneRole::chroma_cell_size(0, 0), Some((0, 0)));
        assert_eq!(McPlaneRole::chroma_cell_size(0, 4), Some((0, 1)));
        assert_eq!(McPlaneRole::chroma_cell_size(4, 0), Some((1, 0)));
    }

    // ---- cross-module sanity ----

    #[test]
    fn classifier_of_chroma_subsampling_uniform_in_both_axes() {
        // §5.5 third bullet — the 4:1 ratio applies to *both*
        // horizontal and vertical axes. The subsampling-ratio
        // method returns a single scalar shared by both.
        let r = McPlaneRole::Chroma.cell_size_subsampling_ratio();
        let (w, h) = McPlaneRole::chroma_cell_size(r * 7, r * 11)
            .expect("multiple of the ratio always succeeds");
        assert_eq!(w, 7);
        assert_eq!(h, 11);
    }

    #[test]
    fn chroma_visible_width_equals_luma_divided_by_ratio() {
        // §5.5 second / third bullets jointly imply the visible
        // width follows the cell-size subsampling.
        assert_eq!(
            McPlaneRole::Chroma.strip_visible_width(),
            McPlaneRole::Luma.strip_visible_width()
                / McPlaneRole::Chroma.cell_size_subsampling_ratio(),
        );
    }

    #[test]
    fn row_stride_independent_of_visible_width_for_chroma() {
        // §5.5 second-bullet disposition cross-check — the
        // allocated row stride and the visible width are
        // structurally independent for the chroma role.
        assert_ne!(
            McPlaneRole::Chroma.strip_allocated_row_stride() as u32,
            McPlaneRole::Chroma.strip_visible_width(),
        );
    }

    #[test]
    fn packed_mv_pixel_offset_interpretation_disposition() {
        // §5.5 fourth-bullet disposition cross-check — the typed
        // interpretation enum's single-variant disposition is the
        // §5.5 prose's "applies uniformly to luma and chroma
        // planes" claim.
        let v = MvPixelOffsetInterpretation::LumaOrChromaUniformBufferStride;
        assert_eq!(v.pixel_offset_row_stride(), 0xb0);
        assert_eq!(
            v.pixel_offset_row_stride() as usize,
            McPlaneRole::Luma.strip_allocated_row_stride(),
        );
        assert_eq!(
            v.pixel_offset_row_stride() as usize,
            McPlaneRole::Chroma.strip_allocated_row_stride(),
        );
    }
}
