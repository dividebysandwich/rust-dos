//! Indeo 3 spec/05 §4.1 — strip pixel-buffer arena geometry.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/05-motion-compensation.md`
//! §4.1 (the `0x8020`-byte heap-allocated arena, populated once per
//! `ICDecompressBegin`, divided into per-strip regions of fixed
//! height and row-stride `0xb0`, with each strip-context slot's six
//! base pointers `[ctx+0x00..+0x14]` acting as six aliases of the
//! same per-strip region), and `spec/02 §7` (the codec-init
//! allocation routine at `IR32_32.DLL!0x10003cdc..0x10003ce3`).
//! Cross-references: `spec/02 §5.2` (the per-slot base-pointer field
//! offsets) and `spec/03 §5.2` (the "six aliases of the strip's
//! pixel buffer" identity).
//!
//! Round 8 ([`super::strip_context`]) surfaced the strip-context
//! slot layout up to and including `+0x1c` (strip width), and
//! constants for the arena size and the six base-pointer field
//! offsets (`BASE_PTR_0` .. `BASE_PTR_5`). Round 15
//! ([`super::mc_address`]) surfaced the cell-position decoding
//! entry that produces the per-cell `dst_cell_data` / `src_cell_data`
//! DWORDs the MC fetcher consumes — *given* a strip-context slot
//! pointer. Round 16 ([`super::bank_select`]) resolved the §4.2
//! ping-pong bank pick that determines *which* strip-context slot
//! the per-plane decoder reads from / writes to. This module
//! (round 17) fills the §4.1 link between those two pieces: the
//! arena into which the six base pointers point, and the typed
//! pick of which of the six aliases to consume.
//!
//! This module surfaces:
//!
//! * [`MC_ARENA_LEN`] (`0x8020`) — the heap-allocated arena size
//!   per `ICDecompressBegin` (the allocator at
//!   `IR32_32.DLL!0x10003cdc`). Re-exported as
//!   `super::PIXEL_BUFFER_ARENA_LEN` with the typed alias here for
//!   the §4.1 surface.
//! * [`MC_ARENA_ROW_STRIDE`] (`0xb0`) — the byte stride between
//!   successive rows of a strip's pixel buffer (§4.1 / §5.3 / `spec/03
//!   §5.2`). Aliases [`super::mc_kernel::MC_ROW_STRIDE`] with a
//!   `const _` cross-check.
//! * [`STRIP_PIXEL_BUFFER_ALIAS_COUNT`] (`6`) — the number of
//!   per-slot base-pointer fields at `[ctx+0x00..+0x14]` (also
//!   re-exported as [`super::STRIP_SLOT_BASE_PTR_COUNT`]).
//! * [`StripPixelBufferAlias`] — a typed pick of one of the six
//!   aliases (`Base0` .. `Base5`), with
//!   [`StripPixelBufferAlias::from_index`] / [`as_index`] /
//!   [`slot_relative_byte_offset`] surfacing the chosen alias's
//!   byte offset within the strip-context slot.
//! * [`strip_region_bytes`] — the per-strip region size in bytes
//!   (= [`MC_ARENA_ROW_STRIDE`] × `plane_height_pixels`), per the
//!   §4.1 worked example.
//! * [`StripArenaCapacity`] — the typed result of comparing one
//!   strip region's byte size against the arena's total
//!   [`MC_ARENA_LEN`], pinning the §4.1 footnote arithmetic for
//!   any caller that wants to flag the "region does not fit in
//!   arena" case at safe-Rust boundaries. The decoder does not
//!   itself check this; per §4.1 the arena allocator and the host's
//!   heap are responsible.
//! * [`StripArenaCapacity::for_plane_height`] —
//!   constructor that runs the §4.1 worked example for any
//!   `plane_height_pixels`.
//! * [`base_pointer_aliases_equal`] — the §4.1 / `spec/03 §5.2`
//!   "six pointers are aliases of the same per-strip region"
//!   invariant, expressed as a per-slot read of the six fields and
//!   a check that they hold the same 32-bit value. Useful for
//!   black-box assertions over a decoded strip-context slot byte
//!   slice.
//!
//! What this module **deliberately does not do** (the §4 chapter
//! boundary):
//!
//! * It does not perform the heap allocation itself. The
//!   `IR32_32.DLL!0x10003cdc..0x10003ce3` call site is a host
//!   `LocalAlloc` (or equivalent); on safe-Rust callers this is the
//!   pixel-buffer slice the [`super::mc_kernel`] kernels operate
//!   over, sized by the caller.
//! * It does not enforce per-strip bounds at MC-fetcher time. Per
//!   §4.4 the binary itself performs no `pixel_offset` range-check
//!   on the §2.3 source-pointer arithmetic; safe-Rust callers that
//!   want bounds-checking apply it at the slice boundary, not here.
//!   [`StripArenaCapacity`] only flags the *static* "does one
//!   strip's worth of pixels even fit in the arena" question, not
//!   the per-cell question.
//! * It does not own or populate the strip-context slot's six
//!   base-pointer fields. The codec-init routine at
//!   `IR32_32.DLL!0x10003edc..0x10003f3a` writes those six fields;
//!   this module surfaces the field offsets only.
//! * It does not perform the §4.2 ping-pong bank pick or the §4.3
//!   source/destination slot inversion. Those are owned by
//!   [`super::bank_select`].
//! * It does not own the arena's per-frame contents. The pixel
//!   bytes within the arena are written by the MC fetcher
//!   ([`super::mc_kernel`]) and the dyad-emission kernel
//!   ([`super::reconstruct`]); this module surfaces only the arena
//!   geometry.
//!
//! All offsets, RVAs, sizes, and arithmetic identities are taken
//! from `05-motion-compensation.md` §4.1 / §4.4, `02-picture-layer.md`
//! §7, and `03-macroblock-layer.md` §5.2. RVAs cited in
//! doc-comments refer to the binary identified in `spec/00 §2`.

#[cfg(test)]
use super::strip_context::STRIP_SLOT_STRIDE;
use super::strip_context::{PIXEL_BUFFER_ARENA_LEN, STRIP_SLOT_BASE_PTR_COUNT, slot_field};

// ---- §4.1 (arena geometry constants) -------------------------------

/// Spec/05 §4.1 / `spec/02 §7` — strip pixel-buffer arena size in
/// bytes (`0x8020`).
///
/// The arena is heap-allocated once per `ICDecompressBegin` at
/// `IR32_32.DLL!0x10003cdc..0x10003ce3`. Aliases
/// [`super::PIXEL_BUFFER_ARENA_LEN`] with a `const _` cross-check
/// to keep the §4.1 surface self-contained.
pub const MC_ARENA_LEN: usize = PIXEL_BUFFER_ARENA_LEN;

/// `const _` cross-check that [`MC_ARENA_LEN`] tracks the
/// [`super::PIXEL_BUFFER_ARENA_LEN`] surface owned by
/// `strip_context`.
const _: () = assert!(MC_ARENA_LEN == PIXEL_BUFFER_ARENA_LEN);

/// `const _` cross-check that the arena's hex value matches the
/// `IR32_32.DLL!0x10003cdc` allocator's immediate.
const _: () = assert!(MC_ARENA_LEN == 0x8020);

/// Spec/05 §4.1 / §5.1 / §5.3 / `spec/03 §5.2` — byte stride between
/// successive rows of a strip's pixel buffer (`0xb0` = 176 bytes).
///
/// The same constant drives the MC fetcher inner-loop (§5.1, surfaced
/// as [`super::mc_kernel::MC_ROW_STRIDE`]) and the packed-MV
/// pixel-offset arithmetic (§3.3, surfaced as
/// [`super::mc_packed::MV_PIXEL_OFFSET_ROW_STRIDE`]).
pub const MC_ARENA_ROW_STRIDE: usize = 0xb0;

/// `const _` cross-check that [`MC_ARENA_ROW_STRIDE`] equals the
/// [`super::mc_kernel::MC_ROW_STRIDE`] surface.
const _: () = assert!(MC_ARENA_ROW_STRIDE == super::mc_kernel::MC_ROW_STRIDE);

/// `const _` cross-check that [`MC_ARENA_ROW_STRIDE`] equals the
/// [`super::reconstruct::PREDICTOR_ROW_STRIDE`] surface (the same
/// per-row stride drives the dyad emission kernel).
const _: () = assert!(MC_ARENA_ROW_STRIDE == super::reconstruct::PREDICTOR_ROW_STRIDE);

/// Spec/05 §4.1 / `spec/02 §5.2` — number of per-slot base-pointer
/// fields the §4.1 prose refers to as "six aliases of the strip's
/// pixel buffer".
///
/// Mirrors [`super::STRIP_SLOT_BASE_PTR_COUNT`] for callers that
/// want the constant by its §4.1 name.
pub const STRIP_PIXEL_BUFFER_ALIAS_COUNT: usize = STRIP_SLOT_BASE_PTR_COUNT;

/// `const _` cross-check.
const _: () = assert!(STRIP_PIXEL_BUFFER_ALIAS_COUNT == STRIP_SLOT_BASE_PTR_COUNT);
const _: () = assert!(STRIP_PIXEL_BUFFER_ALIAS_COUNT == 6);

// ---- §4.1 / `spec/02 §5.2` (typed alias pick) ----------------------

/// Spec/05 §4.1 / `spec/02 §5.2` — typed pick of one of the six
/// strip-context-slot base-pointer aliases.
///
/// Per §4.1, "the six pointers are aliases of the same per-strip
/// region" — at codec-init time the routine at
/// `IR32_32.DLL!0x10003edc..0x10003f3a` writes the *same* per-strip
/// region pointer into all six `+0x00..+0x14` fields. The decoder's
/// MC fetcher, dyad emission kernel, and edge fix-up loops each
/// consult one of the six aliases depending on the role:
///
/// | Alias    | Slot byte offset | Role per `spec/02 §5.2`  |
/// |----------|------------------|--------------------------|
/// | `Base0`  | `+0x00`          | start of strip's pixel buffer |
/// | `Base1`  | `+0x04`          | top-edge prediction context   |
/// | `Base2`  | `+0x08`          | (slot-relative offset)        |
/// | `Base3`  | `+0x0c`          | (slot-relative offset)        |
/// | `Base4`  | `+0x10`          | (slot-relative offset)        |
/// | `Base5`  | `+0x14`          | (slot-relative offset)        |
///
/// Per §4.1 / `spec/03 §5.2`, all six fields hold the same byte
/// address into the arena. The §4.1 prose describes them as
/// "aliases" precisely because of this; the per-role distinction
/// is documented but does not introduce a per-role pointer value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripPixelBufferAlias {
    /// `+0x00` — base ptr 0 (start of strip's pixel buffer).
    Base0,
    /// `+0x04` — base ptr 1 (top-edge prediction context).
    Base1,
    /// `+0x08` — base ptr 2.
    Base2,
    /// `+0x0c` — base ptr 3.
    Base3,
    /// `+0x10` — base ptr 4.
    Base4,
    /// `+0x14` — base ptr 5.
    Base5,
}

impl StripPixelBufferAlias {
    /// Spec/05 §4.1 / `spec/02 §5.2` — construct from a numeric
    /// alias index in `0..STRIP_PIXEL_BUFFER_ALIAS_COUNT`.
    ///
    /// Returns `None` if `index >= STRIP_PIXEL_BUFFER_ALIAS_COUNT`.
    pub const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Base0),
            1 => Some(Self::Base1),
            2 => Some(Self::Base2),
            3 => Some(Self::Base3),
            4 => Some(Self::Base4),
            5 => Some(Self::Base5),
            _ => None,
        }
    }

    /// The numeric alias index in `0..STRIP_PIXEL_BUFFER_ALIAS_COUNT`.
    pub const fn as_index(self) -> usize {
        match self {
            Self::Base0 => 0,
            Self::Base1 => 1,
            Self::Base2 => 2,
            Self::Base3 => 3,
            Self::Base4 => 4,
            Self::Base5 => 5,
        }
    }

    /// The slot-relative byte offset of this alias's field within
    /// the strip-context slot (one of
    /// [`slot_field::BASE_PTR_0`] .. [`slot_field::BASE_PTR_5`]).
    pub const fn slot_relative_byte_offset(self) -> usize {
        match self {
            Self::Base0 => slot_field::BASE_PTR_0,
            Self::Base1 => slot_field::BASE_PTR_1,
            Self::Base2 => slot_field::BASE_PTR_2,
            Self::Base3 => slot_field::BASE_PTR_3,
            Self::Base4 => slot_field::BASE_PTR_4,
            Self::Base5 => slot_field::BASE_PTR_5,
        }
    }
}

// ---- §4.1 (per-strip region size) ----------------------------------

/// Spec/05 §4.1 — per-strip region size in bytes for a plane of
/// height `plane_height_pixels`.
///
/// Per §4.1, "the arena is divided into per-strip regions of fixed
/// height (the plane height) and width equal to the strip's
/// allocated row stride `0xb0`". The per-strip region size is
/// therefore [`MC_ARENA_ROW_STRIDE`] × `plane_height_pixels`. The
/// computation is performed in `u64` to remove any wraparound
/// concern at the type boundary; per the §4.1 worked example,
/// `plane_height_pixels = 240` gives `0xb0 * 240 = 0xa500`.
pub const fn strip_region_bytes(plane_height_pixels: u32) -> u64 {
    (MC_ARENA_ROW_STRIDE as u64) * (plane_height_pixels as u64)
}

// ---- §4.1 (arena-capacity comparison) ------------------------------

/// Spec/05 §4.1 — typed result of comparing one strip region's
/// byte size against the arena's total [`MC_ARENA_LEN`].
///
/// Per the §4.1 footnote (the worked example for a 240-pixel-tall
/// luma plane), the per-strip region byte size can exceed
/// [`MC_ARENA_LEN`] when the plane is tall enough; the §4.1
/// disposition is that the arena allocator (the host's heap) and
/// the per-strip slot's [`MC_ARENA_ROW_STRIDE`] × strip-height
/// allocation each play a role, and the §4.1 "smaller than" claim
/// in the original prose is an arena-vs-row-stride observation, not
/// a strict-fits-or-fails check at runtime.
///
/// The `region_bytes` field is the §4.1 worked-example value (=
/// [`strip_region_bytes`] applied to the plane height); the
/// `fits_in_arena` field is the per-§4.1 `region_bytes <=
/// MC_ARENA_LEN` predicate that the binary itself does not assert.
/// Safe-Rust callers can use this to flag malformed plane heights
/// at construction time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StripArenaCapacity {
    /// Plane height in pixels — the `plane_height_pixels` argument
    /// to [`Self::for_plane_height`].
    pub plane_height_pixels: u32,
    /// Per-strip region size in bytes (= [`MC_ARENA_ROW_STRIDE`] ×
    /// `plane_height_pixels`).
    pub region_bytes: u64,
    /// Whether `region_bytes <= MC_ARENA_LEN`.
    pub fits_in_arena: bool,
}

impl StripArenaCapacity {
    /// Spec/05 §4.1 — run the §4.1 worked example for the supplied
    /// `plane_height_pixels`.
    pub const fn for_plane_height(plane_height_pixels: u32) -> Self {
        let region_bytes = strip_region_bytes(plane_height_pixels);
        Self {
            plane_height_pixels,
            region_bytes,
            fits_in_arena: region_bytes <= MC_ARENA_LEN as u64,
        }
    }
}

// ---- §4.1 / `spec/03 §5.2` (alias-equality invariant) --------------

/// Spec/05 §4.1 / `spec/03 §5.2` — check that all six base-pointer
/// aliases within the supplied strip-context slot hold the same
/// 32-bit value.
///
/// Per the §4.1 prose ("the six pointers are aliases of the same
/// per-strip region") and the `spec/03 §5.2` matching identity, a
/// well-formed strip-context slot at any time during a decode
/// session has the same 32-bit value at each of `+0x00`, `+0x04`,
/// `+0x08`, `+0x0c`, `+0x10`, `+0x14`. This check is a no-op for
/// the decoder itself (which reads from the role-specific field on
/// each site) but is useful for safe-Rust callers that materialise
/// a slot from arbitrary bytes and want to flag a malformed slot
/// early.
///
/// Returns `None` if `slot_bytes.len() < slot_field::BASE_PTR_5 + 4`
/// — i.e. the supplied slice does not extend through the last
/// base-pointer field.
///
/// The bytes are interpreted as little-endian per `spec/00 §3`
/// ("All multi-byte fields are little-endian"). The function does
/// not check the slot's own size against [`STRIP_SLOT_STRIDE`]; it
/// only checks the six base-pointer fields.
pub fn base_pointer_aliases_equal(slot_bytes: &[u8]) -> Option<bool> {
    let required_len = slot_field::BASE_PTR_5 + 4;
    if slot_bytes.len() < required_len {
        return None;
    }
    let base0 = read_u32_le(slot_bytes, slot_field::BASE_PTR_0);
    let base1 = read_u32_le(slot_bytes, slot_field::BASE_PTR_1);
    let base2 = read_u32_le(slot_bytes, slot_field::BASE_PTR_2);
    let base3 = read_u32_le(slot_bytes, slot_field::BASE_PTR_3);
    let base4 = read_u32_le(slot_bytes, slot_field::BASE_PTR_4);
    let base5 = read_u32_le(slot_bytes, slot_field::BASE_PTR_5);
    Some(base0 == base1 && base1 == base2 && base2 == base3 && base3 == base4 && base4 == base5)
}

fn read_u32_le(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- §4.1 arena-geometry constants ----

    #[test]
    fn arena_len_matches_strip_context() {
        assert_eq!(MC_ARENA_LEN, 0x8020);
        assert_eq!(MC_ARENA_LEN, PIXEL_BUFFER_ARENA_LEN);
    }

    #[test]
    fn arena_row_stride_matches_kernel_and_predictor() {
        assert_eq!(MC_ARENA_ROW_STRIDE, 0xb0);
        assert_eq!(MC_ARENA_ROW_STRIDE, super::super::mc_kernel::MC_ROW_STRIDE);
        assert_eq!(
            MC_ARENA_ROW_STRIDE,
            super::super::reconstruct::PREDICTOR_ROW_STRIDE
        );
    }

    #[test]
    fn alias_count_is_six() {
        assert_eq!(STRIP_PIXEL_BUFFER_ALIAS_COUNT, 6);
        assert_eq!(STRIP_PIXEL_BUFFER_ALIAS_COUNT, STRIP_SLOT_BASE_PTR_COUNT);
    }

    // ---- StripPixelBufferAlias ----

    #[test]
    fn alias_round_trips_through_index() {
        for i in 0..STRIP_PIXEL_BUFFER_ALIAS_COUNT {
            let alias = StripPixelBufferAlias::from_index(i).expect("legal index");
            assert_eq!(alias.as_index(), i);
        }
    }

    #[test]
    fn alias_rejects_out_of_range_index() {
        assert!(StripPixelBufferAlias::from_index(6).is_none());
        assert!(StripPixelBufferAlias::from_index(7).is_none());
        assert!(StripPixelBufferAlias::from_index(usize::MAX).is_none());
    }

    #[test]
    fn alias_byte_offsets_match_slot_field_constants() {
        assert_eq!(
            StripPixelBufferAlias::Base0.slot_relative_byte_offset(),
            slot_field::BASE_PTR_0
        );
        assert_eq!(
            StripPixelBufferAlias::Base1.slot_relative_byte_offset(),
            slot_field::BASE_PTR_1
        );
        assert_eq!(
            StripPixelBufferAlias::Base2.slot_relative_byte_offset(),
            slot_field::BASE_PTR_2
        );
        assert_eq!(
            StripPixelBufferAlias::Base3.slot_relative_byte_offset(),
            slot_field::BASE_PTR_3
        );
        assert_eq!(
            StripPixelBufferAlias::Base4.slot_relative_byte_offset(),
            slot_field::BASE_PTR_4
        );
        assert_eq!(
            StripPixelBufferAlias::Base5.slot_relative_byte_offset(),
            slot_field::BASE_PTR_5
        );
    }

    #[test]
    fn alias_byte_offsets_are_dword_aligned_4_apart() {
        // Per spec/02 §5.2 the six fields are consecutive 4-byte slots.
        for i in 1..STRIP_PIXEL_BUFFER_ALIAS_COUNT {
            let prev = StripPixelBufferAlias::from_index(i - 1)
                .unwrap()
                .slot_relative_byte_offset();
            let curr = StripPixelBufferAlias::from_index(i)
                .unwrap()
                .slot_relative_byte_offset();
            assert_eq!(
                curr - prev,
                4,
                "alias {i} byte offset should be 4 past {prev:#x}"
            );
        }
    }

    #[test]
    fn last_alias_byte_offset_fits_within_slot() {
        // Through the last base-pointer DWORD (`+0x14 + 4 = +0x18`),
        // the slot still has its remaining fields available.
        let last = StripPixelBufferAlias::Base5.slot_relative_byte_offset();
        assert!(last + 4 <= STRIP_SLOT_STRIDE);
    }

    // ---- strip_region_bytes ----

    #[test]
    fn strip_region_bytes_for_worked_example() {
        // §4.1 worked example: 240-pixel-tall luma plane → 0xb0 * 240 = 0xa500.
        assert_eq!(strip_region_bytes(240), 0xa500);
    }

    #[test]
    fn strip_region_bytes_zero_height_is_zero() {
        assert_eq!(strip_region_bytes(0), 0);
    }

    #[test]
    fn strip_region_bytes_does_not_wrap_on_max_u32_height() {
        // u64 arithmetic; should not panic or overflow.
        let big = strip_region_bytes(u32::MAX);
        assert_eq!(big, (MC_ARENA_ROW_STRIDE as u64) * (u32::MAX as u64));
    }

    // ---- StripArenaCapacity ----

    #[test]
    fn arena_capacity_worked_example_does_not_fit() {
        // §4.1 worked example: 0xa500 bytes for 240px plane;
        // the arena is 0x8020 bytes.
        let cap = StripArenaCapacity::for_plane_height(240);
        assert_eq!(cap.plane_height_pixels, 240);
        assert_eq!(cap.region_bytes, 0xa500);
        // 0xa500 > 0x8020 → does NOT fit; surfaces the §4.1 footnote
        // discrepancy. Safe-Rust callers can flag this; the decoder
        // does not.
        assert!(!cap.fits_in_arena);
    }

    #[test]
    fn arena_capacity_small_plane_fits() {
        // A short plane (e.g. height 16) easily fits.
        let cap = StripArenaCapacity::for_plane_height(16);
        assert_eq!(cap.region_bytes, (MC_ARENA_ROW_STRIDE as u64) * 16);
        assert!(cap.fits_in_arena);
    }

    #[test]
    fn arena_capacity_boundary_height() {
        // Largest plane_height where MC_ARENA_ROW_STRIDE * height <= MC_ARENA_LEN.
        // 0x8020 / 0xb0 = 186.something → 186 fits, 187 does not.
        assert_eq!(MC_ARENA_LEN / MC_ARENA_ROW_STRIDE, 186);
        let cap_fits = StripArenaCapacity::for_plane_height(186);
        assert!(cap_fits.fits_in_arena);
        assert_eq!(cap_fits.region_bytes, 186 * (MC_ARENA_ROW_STRIDE as u64));
        let cap_not = StripArenaCapacity::for_plane_height(187);
        assert!(!cap_not.fits_in_arena);
    }

    #[test]
    fn arena_capacity_zero_plane_height_fits() {
        let cap = StripArenaCapacity::for_plane_height(0);
        assert_eq!(cap.region_bytes, 0);
        assert!(cap.fits_in_arena);
    }

    // ---- base_pointer_aliases_equal ----

    #[test]
    fn aliases_equal_for_well_formed_slot() {
        // Build a slot where all six base pointers hold 0xdeadbeef.
        let mut slot = vec![0u8; STRIP_SLOT_STRIDE];
        let val = 0xdeadbeef_u32.to_le_bytes();
        for off in [
            slot_field::BASE_PTR_0,
            slot_field::BASE_PTR_1,
            slot_field::BASE_PTR_2,
            slot_field::BASE_PTR_3,
            slot_field::BASE_PTR_4,
            slot_field::BASE_PTR_5,
        ] {
            slot[off..off + 4].copy_from_slice(&val);
        }
        assert_eq!(base_pointer_aliases_equal(&slot), Some(true));
    }

    #[test]
    fn aliases_unequal_for_malformed_slot() {
        // One alias differs.
        let mut slot = vec![0u8; STRIP_SLOT_STRIDE];
        let val = 0xdeadbeef_u32.to_le_bytes();
        for off in [
            slot_field::BASE_PTR_0,
            slot_field::BASE_PTR_1,
            slot_field::BASE_PTR_2,
            slot_field::BASE_PTR_3,
            slot_field::BASE_PTR_4,
            slot_field::BASE_PTR_5,
        ] {
            slot[off..off + 4].copy_from_slice(&val);
        }
        // Corrupt Base3.
        let bad = 0xfeedfaceu32.to_le_bytes();
        slot[slot_field::BASE_PTR_3..slot_field::BASE_PTR_3 + 4].copy_from_slice(&bad);
        assert_eq!(base_pointer_aliases_equal(&slot), Some(false));
    }

    #[test]
    fn aliases_equal_returns_none_for_short_slice() {
        let short = vec![0u8; slot_field::BASE_PTR_5 + 3];
        assert_eq!(base_pointer_aliases_equal(&short), None);
        // Zero-length is also short.
        assert_eq!(base_pointer_aliases_equal(&[]), None);
    }

    #[test]
    fn aliases_equal_accepts_slice_truncated_just_after_last_field() {
        // Exactly through Base5 + 4 is enough.
        let mut slot = vec![0u8; slot_field::BASE_PTR_5 + 4];
        // All zeros → all equal.
        assert_eq!(base_pointer_aliases_equal(&slot), Some(true));
        // Set Base0 to 1 → no longer equal.
        slot[slot_field::BASE_PTR_0..slot_field::BASE_PTR_0 + 4]
            .copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(base_pointer_aliases_equal(&slot), Some(false));
    }

    // ---- cross-checks with neighbour modules ----

    #[test]
    fn arena_row_stride_matches_mv_pixel_offset_row_stride() {
        assert_eq!(
            MC_ARENA_ROW_STRIDE,
            super::super::mc_packed::MV_PIXEL_OFFSET_ROW_STRIDE as usize
        );
    }

    #[test]
    fn arena_row_stride_matches_per_cell_edge_row_stride() {
        assert_eq!(
            MC_ARENA_ROW_STRIDE,
            super::super::cell_subarray::PER_CELL_EDGE_ROW_STRIDE
        );
    }
}
