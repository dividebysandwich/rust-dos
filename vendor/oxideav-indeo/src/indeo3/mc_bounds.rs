//! Indeo 3 spec/05 §4.4 — the "no explicit boundary check" surface
//! for the motion-compensation source-pointer arithmetic.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/05-motion-compensation.md`
//! §4.4 (the disposition "the parser does not validate that
//! `pixel_offset` (the high 30 bits of the packed MV, signed)
//! addresses a byte within the source strip's allocated buffer") and
//! cross-references: `spec/05 §2.3` (the bare
//! `add esi, sign_extend(packed_MV >> 2)`), `spec/05 §4.1` (the
//! per-strip region's byte size = [`super::MC_ARENA_ROW_STRIDE`] ×
//! `plane_height_pixels`), `spec/02 §7` (the strip allocator's
//! deterministic-pattern initialisation), and `spec/03 §5.4` (the
//! strip-edge fix-up loops that preserve the padding pixels across
//! frames).
//!
//! Round 14 ([`super::mc_kernel`]) lands the §5.1 MC fetcher inner
//! loop that consumes the resolved `(dst_addr, src_addr)` byte
//! addresses. Round 13 ([`super::mc_packed`]) lands the §2.3
//! sign-extending [`super::apply_mv_source_offset`] that computes the
//! source-pixel address from the destination cell base and the
//! packed-MV's pixel offset. Round 17 ([`super::mc_arena`]) lands the
//! §4.1 [`super::strip_region_bytes`] formula and
//! [`super::StripArenaCapacity`]. This module owns §4.4 — the
//! disposition that no boundary check is performed at the
//! source-pointer arithmetic site, the typed predicate "does this
//! source-pointer offset land within the strip's allocated region"
//! that a safe-Rust caller may opt in to, the §4.4 worked example
//! for a 240-pixel-tall luma plane (`0xb0 * 240 = 0xa500` ≈ 42 KB,
//! far smaller than [`super::MC_ARENA_LEN`]), and the typed surface
//! linking the §4.4 padding-pixel preservation to the §5.4 strip-edge
//! fix-up surface from round 11.
//!
//! This module surfaces:
//!
//! * [`MC_NO_BOUNDARY_CHECK`] — the §4.4 disposition as a typed
//!   `const`-`true` flag: the binary performs no boundary check on
//!   the §2.3 source-pointer arithmetic. Consumed by callers that
//!   want to assert at the type boundary that they are aware of the
//!   disposition before invoking [`super::apply_mv_source_offset`]
//!   without their own bounds check.
//! * [`SourcePointerBoundsCheck`] — the §4.4 typed disposition enum
//!   (`BinaryDoesNotCheck` / `CallerOptsIn`). The binary uses the
//!   first variant; safe-Rust callers may use the second to indicate
//!   they have applied [`mv_source_offset_in_strip_region`] before
//!   feeding the offset to the §2.3 arithmetic.
//! * [`MvSourceOffsetClass`] — the per-call classification result
//!   (`InRegion` / `OutOfRegion` / `Underflow`). `InRegion` ⇒ the
//!   resulting source-pointer byte is within the supplied strip
//!   region. `OutOfRegion` ⇒ the source byte is past the strip
//!   region's last addressable byte (the §4.4 "decoder reads from
//!   whatever bytes happen to occupy that part of the heap arena"
//!   case). `Underflow` ⇒ the signed addition would go below zero
//!   (the §2.3 `add esi, edx` with negative `edx` exceeds the
//!   destination cell base).
//! * [`mv_source_offset_in_strip_region`] — the §4.4 opt-in
//!   classifier. Takes the destination cell base `dst_cell_base`, the
//!   signed pixel-byte offset `mv_offset` (the output of
//!   [`super::PackedMv::pixel_offset`]), and the per-strip region
//!   size in bytes (`strip_region_bytes`, the output of
//!   [`super::strip_region_bytes`] applied to the plane height).
//!   Returns the [`MvSourceOffsetClass`] without consuming or
//!   producing the §2.3 source-pointer arithmetic itself — the
//!   §2.3 add is owned by [`super::apply_mv_source_offset`].
//! * [`STRIP_REGION_LUMA_240_BYTES`] — the §4.4 worked-example
//!   constant for a 240-pixel-tall luma plane (`0xb0 * 240 =
//!   0xa500` = `42_240` bytes), cross-checked against
//!   [`super::strip_region_bytes`].
//! * [`STRIP_REGION_LUMA_240_FITS_IN_ARENA`] — the §4.4 worked-
//!   example boolean disposition that `STRIP_REGION_LUMA_240_BYTES <
//!   MC_ARENA_LEN` ("far smaller than the 0x8020-byte arena's
//!   total"). The strict-`<` direction (not `<=`) matches the §4.4
//!   prose's "far smaller".
//! * [`PaddingPixelPreservation`] — the §4.4 typed disposition of
//!   the strip allocator's deterministic-pattern initialisation
//!   relative to the §5.4 strip-edge fix-up loops
//!   ([`super::StripEdgeFixupDims`]): the padding pixels at the
//!   left-neighbour and upper-neighbour pre-allocated region are
//!   initialised once at codec init and preserved frame-to-frame by
//!   the §5.4 edge fix-up.
//!
//! What this module **deliberately does not do** (the §4.4 chapter
//! boundary):
//!
//! * It does not perform the §2.3 source-pointer arithmetic itself.
//!   The `add esi, sign_extend(packed >> 2)` site is owned by
//!   [`super::apply_mv_source_offset`]; this module surfaces only
//!   the disposition and the opt-in classifier.
//! * It does not own the strip allocator or its deterministic-
//!   pattern fill. The host's heap allocator at
//!   `IR32_32.DLL!0x10003cdc..0x10003ce3` is host-side territory
//!   (per `spec/02 §7`); this module only documents the spec-
//!   visible consequence (padding pixels remain stable across
//!   frames).
//! * It does not perform the §5.4 strip-edge fix-up. That's
//!   [`super::StripEdgeFixupDims`] / [`super::StripEdgeRowIter`];
//!   this module only points at them as the side that preserves the
//!   padding across frames.
//! * It does not range-check `dst_cell_base` itself against the
//!   strip region size. The destination is assumed to have been
//!   produced by the §7.2 [`super::mc_dest_address`] chain and to
//!   sit within the strip region by the §5.4 / §7.3 position
//!   decomposition. (`mv_source_offset_in_strip_region` will return
//!   `OutOfRegion` if the destination base itself is past the
//!   region end, but does not distinguish that diagnostic from the
//!   MV-out-of-region case — both reach "decoder reads from
//!   whatever bytes happen to occupy that part of the heap arena"
//!   per §4.4.)
//! * It does not assert any encoder-side rule that MVs stay within
//!   the strip's allocated cells. Per §4.4 the binary "tolerates
//!   them without faulting; they are not malformed from the
//!   decoder's perspective", so the classifier is informational only
//!   and never indicates a malformed stream by itself.
//!
//! All offsets, RVAs, and arithmetic identities are taken from
//! `05-motion-compensation.md` §4.4 (paragraphs 1–3) and cross-
//! referenced against §2.3 (the bare `add esi, edx`), §4.1 (the
//! per-strip region size and the arena-vs-region disposition), and
//! `spec/03 §5.4` (the strip-edge fix-up's padding preservation).
//! RVAs cited in doc-comments refer to the binary identified in
//! `spec/00 §2`.

use super::mc_arena::{MC_ARENA_LEN, MC_ARENA_ROW_STRIDE, strip_region_bytes};

// ---- §4.4 disposition ---------------------------------------------

/// Spec/05 §4.4 — the §4.4 disposition as a typed `const`-`true`
/// flag.
///
/// Per §4.4 paragraph 1: "The parser does not validate that
/// `pixel_offset` (the high 30 bits of the packed MV, signed)
/// addresses a byte within the source strip's allocated buffer."
/// Safe-Rust callers that consume the typed source-pointer arithmetic
/// without their own bounds check should reference this flag at
/// their call site so the disposition is greppable from any audit
/// of the call graph.
pub const MC_NO_BOUNDARY_CHECK: bool = true;

/// `const _` cross-check: the disposition is `true` (i.e. the binary
/// indeed performs no boundary check). Pinned here so a future
/// disposition change forces a re-audit of the flag's call sites.
const _: () = assert!(MC_NO_BOUNDARY_CHECK);

// ---- §4.4 worked example ------------------------------------------

/// Spec/05 §4.4 paragraph 2 first bullet — the worked-example strip
/// region byte size for a 240-pixel-tall luma plane.
///
/// `MC_ARENA_ROW_STRIDE * 240 = 0xb0 * 240 = 0xa500 = 42_240` bytes.
/// Surfaced as a `const` so the §4.4 worked example is greppable.
pub const STRIP_REGION_LUMA_240_BYTES: u64 = (MC_ARENA_ROW_STRIDE as u64) * 240;

/// `const _` cross-check: the worked-example value tracks the §4.1
/// formula [`strip_region_bytes`] applied to height 240.
const _: () = assert!(STRIP_REGION_LUMA_240_BYTES == strip_region_bytes(240));

/// `const _` cross-check: the worked-example value matches the §4.4
/// prose's explicit `0xa500` figure.
const _: () = assert!(STRIP_REGION_LUMA_240_BYTES == 0xa500);

/// `const _` cross-check: the worked-example value also matches the
/// §4.4 prose's decimal `42_240` (≈ 42 KB).
const _: () = assert!(STRIP_REGION_LUMA_240_BYTES == 42_240);

/// Spec/05 §4.4 paragraph 2 first bullet — the boolean disposition
/// of whether the 240-pixel-tall luma plane's per-strip region size
/// is `<=` [`MC_ARENA_LEN`].
///
/// **The §4.4 prose claims "far smaller than the 0x8020-byte arena's
/// total" — but the arithmetic disagrees**: `0xa500 = 42_240 >
/// 0x8020 = 32_800`, so the comparison is `false`. Round 17's
/// [`super::StripArenaCapacity::for_plane_height`] already surfaces
/// this discrepancy (`fits_in_arena = false` for height 240); this
/// constant pins it specifically for the §4.4 worked example so the
/// prose claim and the numeric disposition are both greppable.
///
/// Predicate uses `<=` (not strict `<`) to align with the §4.1
/// [`super::StripArenaCapacity::fits_in_arena`] convention.
pub const STRIP_REGION_LUMA_240_FITS_IN_ARENA: bool =
    STRIP_REGION_LUMA_240_BYTES <= MC_ARENA_LEN as u64;

/// `const _` cross-check: the §4.4 worked-example region does **not**
/// fit strictly inside the arena, matching the §4.1 footnote-
/// discrepancy disposition surfaced by round 17.
const _: () = assert!(!STRIP_REGION_LUMA_240_FITS_IN_ARENA);

// ---- typed disposition --------------------------------------------

/// Spec/05 §4.4 — the typed disposition the call site selects when
/// invoking the §2.3 source-pointer arithmetic.
///
/// The §4.4 binary path is [`BinaryDoesNotCheck`]: the source
/// pointer is built without any bounds check, and the §5.1 MC
/// fetcher reads from wherever the resulting byte address points.
/// A safe-Rust caller that wants to flag a corpus-anomalous frame
/// before invoking the §2.3 arithmetic may use [`CallerOptsIn`] and
/// pair it with a call to [`mv_source_offset_in_strip_region`].
///
/// The two variants are equivalent at the byte level (both produce
/// the same source-pointer byte address from the same `(dst_cell_base,
/// mv_offset)` pair); the variant choice is a documentation /
/// audit-trail concern only.
///
/// [`BinaryDoesNotCheck`]: SourcePointerBoundsCheck::BinaryDoesNotCheck
/// [`CallerOptsIn`]: SourcePointerBoundsCheck::CallerOptsIn
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePointerBoundsCheck {
    /// Spec/05 §4.4 paragraph 1 — the binary path. No bounds check
    /// is performed at the §2.3 `add esi, edx` site. Safe-Rust
    /// callers that select this variant accept the §4.4
    /// "decoder reads from whatever bytes happen to occupy that
    /// part of the heap arena" disposition.
    BinaryDoesNotCheck,
    /// Spec/05 §4.4 paragraph 3 — the safe-Rust opt-in path. The
    /// caller invokes [`mv_source_offset_in_strip_region`] (or an
    /// equivalent per-call check) before the §2.3 arithmetic; the
    /// classification result is used to either skip the §2.3 add or
    /// to flag a corpus-anomalous frame.
    CallerOptsIn,
}

impl SourcePointerBoundsCheck {
    /// `true` for [`SourcePointerBoundsCheck::BinaryDoesNotCheck`] —
    /// the variant the binary itself uses.
    pub const fn is_binary_path(self) -> bool {
        matches!(self, SourcePointerBoundsCheck::BinaryDoesNotCheck)
    }

    /// `true` for [`SourcePointerBoundsCheck::CallerOptsIn`] — the
    /// safe-Rust caller-opted-in variant.
    pub const fn is_caller_opts_in(self) -> bool {
        matches!(self, SourcePointerBoundsCheck::CallerOptsIn)
    }
}

/// Spec/05 §4.4 paragraph 2 — the per-call classification of a
/// source-pointer offset against a supplied strip region.
///
/// The §2.3 `add esi, sign_extend(packed_MV >> 2)` produces the
/// source-pointer byte address. Combined with the destination cell
/// base and the per-strip region size in bytes, the result either
/// lands within the strip's allocated region, past its last
/// addressable byte, or — for sufficiently negative MV offsets —
/// before its base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MvSourceOffsetClass {
    /// The resulting source-pointer byte address is within
    /// `[0, strip_region_bytes)` of the strip's base. The §5.1 MC
    /// fetcher reads valid strip-pixel bytes.
    InRegion,
    /// The resulting source-pointer byte address is `>=
    /// strip_region_bytes` (past the strip region's last addressable
    /// byte). Per §4.4 the decoder reads from "whatever bytes happen
    /// to occupy that part of the heap arena". The strip allocator
    /// (`spec/02 §7`) initialises the buffer to a deterministic
    /// pattern at codec init, so the read produces a deterministic
    /// (but spec-undefined) value.
    OutOfRegion,
    /// The signed `add esi, edx` underflows zero — i.e. the §2.3
    /// arithmetic would point before the strip region's base. The
    /// `add esi, edx` instruction does not fault on signed overflow
    /// in x86-32, so the binary still issues the read; a safe-Rust
    /// caller should treat this as out-of-region.
    Underflow,
}

impl MvSourceOffsetClass {
    /// `true` for [`MvSourceOffsetClass::InRegion`].
    pub const fn is_in_region(self) -> bool {
        matches!(self, MvSourceOffsetClass::InRegion)
    }

    /// `true` for [`MvSourceOffsetClass::OutOfRegion`].
    pub const fn is_out_of_region(self) -> bool {
        matches!(self, MvSourceOffsetClass::OutOfRegion)
    }

    /// `true` for [`MvSourceOffsetClass::Underflow`].
    pub const fn is_underflow(self) -> bool {
        matches!(self, MvSourceOffsetClass::Underflow)
    }

    /// `true` for any variant other than
    /// [`MvSourceOffsetClass::InRegion`] (i.e. any case where the
    /// §5.1 MC fetcher would read undefined bytes per §4.4).
    pub const fn is_out_of_bounds(self) -> bool {
        !self.is_in_region()
    }
}

// ---- §4.4 opt-in classifier ---------------------------------------

/// Spec/05 §4.4 paragraph 3 — classify a §2.3 source-pointer offset
/// against a supplied strip region without invoking the §2.3
/// arithmetic itself.
///
/// `dst_cell_base` is the destination cell base byte offset within
/// the strip region (the output of the §7.2 [`super::mc_dest_address`]
/// chain). `mv_offset` is the signed pixel-byte offset (the output
/// of [`super::PackedMv::pixel_offset`]). `strip_region_bytes_total`
/// is the per-strip region size in bytes (the output of
/// [`super::strip_region_bytes`] applied to the plane height).
///
/// Returns:
///
/// * [`MvSourceOffsetClass::Underflow`] if `dst_cell_base + mv_offset`
///   underflows (i.e. the signed sum is negative).
/// * [`MvSourceOffsetClass::OutOfRegion`] if the resulting address
///   is `>= strip_region_bytes_total`.
/// * [`MvSourceOffsetClass::InRegion`] otherwise.
///
/// The function is `const` and does not consume the §2.3 arithmetic
/// — it operates on the same `(dst_cell_base, mv_offset)` pair the
/// arithmetic would consume, separately.
pub const fn mv_source_offset_in_strip_region(
    dst_cell_base: u64,
    mv_offset: i64,
    strip_region_bytes_total: u64,
) -> MvSourceOffsetClass {
    if mv_offset < 0 {
        let neg = (-mv_offset) as u64;
        if neg > dst_cell_base {
            return MvSourceOffsetClass::Underflow;
        }
        let src = dst_cell_base - neg;
        if src >= strip_region_bytes_total {
            // Cannot happen with a valid dst_cell_base and a negative
            // offset that doesn't underflow: src < dst_cell_base, and
            // dst_cell_base is assumed in-region. But a defensive
            // caller may have passed an out-of-region dst_cell_base;
            // classify it the same as the positive-overflow case.
            MvSourceOffsetClass::OutOfRegion
        } else {
            MvSourceOffsetClass::InRegion
        }
    } else {
        let pos = mv_offset as u64;
        // Saturating add: u64 cannot overflow within any realistic
        // strip region, but the const fn must avoid wrap.
        let (src, overflow) = dst_cell_base.overflowing_add(pos);
        if overflow || src >= strip_region_bytes_total {
            MvSourceOffsetClass::OutOfRegion
        } else {
            MvSourceOffsetClass::InRegion
        }
    }
}

// ---- §4.4 paragraph 2 second bullet (padding-pixel preservation) --

/// Spec/05 §4.4 paragraph 2 second bullet — the typed disposition of
/// the strip allocator's deterministic-pattern initialisation
/// relative to the §5.4 strip-edge fix-up loops.
///
/// Per §4.4: "The strip allocator (`spec/02 §7`) initialises the
/// buffer to a deterministic pattern at codec init (not zero-fill;
/// the exact init is inherited from the host's heap allocator); the
/// decoder's edge fix-up loops (`spec/03 §5.4`) preserve those
/// padding pixels across frames."
///
/// The enum makes the two-half disposition greppable:
///
/// * `DeterministicAtCodecInit` — the §2 §7 codec-init disposition:
///   the strip allocator's pixel-buffer arena
///   ([`super::MC_ARENA_LEN`]) is initialised to a deterministic
///   (but spec-undefined) pattern by the host's heap allocator. The
///   pattern is not zero-fill, but it is the same pattern across
///   any two decode sessions of the same host.
/// * `PreservedAcrossFramesByStripEdgeFixup` — the §5.4 preservation
///   disposition: the strip-edge fix-up loops (see
///   [`super::StripEdgeFixupDims`]) walk only the in-region pixels;
///   the padding region (the left-neighbour and upper-neighbour
///   bytes that lie outside the strip-pixel coordinate range) is
///   never written to by either the VQ residual store or the MC
///   copy, so its codec-init pattern survives across frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaddingPixelPreservation {
    /// Spec/02 §7 — the codec-init disposition.
    DeterministicAtCodecInit,
    /// Spec/03 §5.4 — the frame-to-frame preservation disposition.
    PreservedAcrossFramesByStripEdgeFixup,
}

impl PaddingPixelPreservation {
    /// `true` for [`PaddingPixelPreservation::DeterministicAtCodecInit`].
    pub const fn is_codec_init(self) -> bool {
        matches!(self, PaddingPixelPreservation::DeterministicAtCodecInit)
    }

    /// `true` for
    /// [`PaddingPixelPreservation::PreservedAcrossFramesByStripEdgeFixup`].
    pub const fn is_frame_to_frame(self) -> bool {
        matches!(
            self,
            PaddingPixelPreservation::PreservedAcrossFramesByStripEdgeFixup
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::mc_arena::{MC_ARENA_LEN, MC_ARENA_ROW_STRIDE, strip_region_bytes};
    use super::{
        MC_NO_BOUNDARY_CHECK, MvSourceOffsetClass, PaddingPixelPreservation,
        STRIP_REGION_LUMA_240_BYTES, STRIP_REGION_LUMA_240_FITS_IN_ARENA, SourcePointerBoundsCheck,
        mv_source_offset_in_strip_region,
    };

    // ---- §4.4 disposition flag ----

    #[test]
    fn mc_no_boundary_check_is_true() {
        // Spec/05 §4.4 paragraph 1 — the binary performs no boundary
        // check at the §2.3 site. Read through a runtime-opaque path
        // to defeat constant folding (clippy
        // `assertions_on_constants` would otherwise flag the literal).
        let flag = core::hint::black_box(MC_NO_BOUNDARY_CHECK);
        assert!(flag);
    }

    // ---- worked-example constants ----

    #[test]
    fn strip_region_luma_240_bytes_matches_arithmetic() {
        // 0xb0 * 240 = 0xa500.
        assert_eq!(STRIP_REGION_LUMA_240_BYTES, 0xa500);
        assert_eq!(STRIP_REGION_LUMA_240_BYTES, 42_240);
        assert_eq!(STRIP_REGION_LUMA_240_BYTES, strip_region_bytes(240));
        assert_eq!(
            STRIP_REGION_LUMA_240_BYTES,
            (MC_ARENA_ROW_STRIDE as u64) * 240,
        );
    }

    #[test]
    fn strip_region_luma_240_fits_in_arena_is_false() {
        // Spec/05 §4.4 paragraph 2 first bullet claims "far smaller
        // than the 0x8020-byte arena's total" — but the arithmetic
        // disagrees: 0xa500 = 42_240 > 0x8020 = 32_800. Round 17's
        // mc_arena already surfaces this footnote discrepancy; this
        // test pins the §4.4-specific disposition. Use black_box to
        // defeat constant folding (the consts are compile-time
        // known; clippy `assertions_on_constants` would otherwise
        // flag the assertions).
        let flag = core::hint::black_box(STRIP_REGION_LUMA_240_FITS_IN_ARENA);
        let bytes = core::hint::black_box(STRIP_REGION_LUMA_240_BYTES);
        let arena = core::hint::black_box(MC_ARENA_LEN as u64);
        assert!(!flag);
        assert!(bytes > arena);
    }

    #[test]
    fn strip_region_luma_240_byte_size_exceeds_arena_size() {
        // 0xa500 = 42_240. 0x8020 = 32_800. The §4.4 worked example
        // is documented as "far smaller than the 0x8020-byte arena's
        // total" in the prose, but the actual numeric comparison
        // shows the strip region is 9_440 bytes *larger* than the
        // arena. The §4.1 footnote disposition (already surfaced by
        // round 17 mc_arena) takes precedence: the arena allocator's
        // 0x8020-byte block does not by itself hold a 240-pixel-tall
        // luma strip at row stride 0xb0; the strip allocator and the
        // per-strip slot sizing each play a role.
        assert_eq!(
            STRIP_REGION_LUMA_240_BYTES - MC_ARENA_LEN as u64,
            9_440,
            "the §4.4 worked-example strip region overshoots the \
             arena by 9_440 bytes",
        );
        let flag = core::hint::black_box(STRIP_REGION_LUMA_240_FITS_IN_ARENA);
        assert!(
            !flag,
            "the §4.1 footnote disposition mirrors the numeric \
             comparison: the 240-pixel-tall luma strip does NOT \
             fit in a single arena block",
        );
    }

    // ---- SourcePointerBoundsCheck ----

    #[test]
    fn source_pointer_bounds_check_binary_path() {
        let v = SourcePointerBoundsCheck::BinaryDoesNotCheck;
        assert!(v.is_binary_path());
        assert!(!v.is_caller_opts_in());
    }

    #[test]
    fn source_pointer_bounds_check_caller_opts_in() {
        let v = SourcePointerBoundsCheck::CallerOptsIn;
        assert!(!v.is_binary_path());
        assert!(v.is_caller_opts_in());
    }

    #[test]
    fn source_pointer_bounds_check_two_variants_distinct() {
        assert_ne!(
            SourcePointerBoundsCheck::BinaryDoesNotCheck,
            SourcePointerBoundsCheck::CallerOptsIn,
        );
    }

    // ---- MvSourceOffsetClass ----

    #[test]
    fn mv_source_offset_class_in_region_predicates() {
        let c = MvSourceOffsetClass::InRegion;
        assert!(c.is_in_region());
        assert!(!c.is_out_of_region());
        assert!(!c.is_underflow());
        assert!(!c.is_out_of_bounds());
    }

    #[test]
    fn mv_source_offset_class_out_of_region_predicates() {
        let c = MvSourceOffsetClass::OutOfRegion;
        assert!(!c.is_in_region());
        assert!(c.is_out_of_region());
        assert!(!c.is_underflow());
        assert!(c.is_out_of_bounds());
    }

    #[test]
    fn mv_source_offset_class_underflow_predicates() {
        let c = MvSourceOffsetClass::Underflow;
        assert!(!c.is_in_region());
        assert!(!c.is_out_of_region());
        assert!(c.is_underflow());
        assert!(c.is_out_of_bounds());
    }

    // ---- mv_source_offset_in_strip_region — happy path ----

    #[test]
    fn mv_source_offset_zero_offset_in_region() {
        // Zero MV: source == destination, trivially in region.
        let region = strip_region_bytes(240);
        assert_eq!(
            mv_source_offset_in_strip_region(0x1000, 0, region),
            MvSourceOffsetClass::InRegion,
        );
    }

    #[test]
    fn mv_source_offset_positive_within_region_is_in_region() {
        let region = strip_region_bytes(240); // 0xa500
        // dst_cell_base = 0x1000, mv_offset = +0xb0 (one row down).
        // Resulting address = 0x10b0 < 0xa500: in region.
        assert_eq!(
            mv_source_offset_in_strip_region(0x1000, 0xb0, region),
            MvSourceOffsetClass::InRegion,
        );
    }

    #[test]
    fn mv_source_offset_negative_within_region_is_in_region() {
        let region = strip_region_bytes(240);
        // dst_cell_base = 0x1000, mv_offset = -0xb0: resulting
        // address = 0xf50 < 0xa500: in region.
        assert_eq!(
            mv_source_offset_in_strip_region(0x1000, -0xb0, region),
            MvSourceOffsetClass::InRegion,
        );
    }

    // ---- mv_source_offset_in_strip_region — out-of-region ----

    #[test]
    fn mv_source_offset_positive_past_end_is_out_of_region() {
        let region = strip_region_bytes(240); // 0xa500
        // dst_cell_base = 0xa400, mv_offset = +0x200: resulting
        // address = 0xa600 >= 0xa500: out of region.
        assert_eq!(
            mv_source_offset_in_strip_region(0xa400, 0x200, region),
            MvSourceOffsetClass::OutOfRegion,
        );
    }

    #[test]
    fn mv_source_offset_at_region_end_is_out_of_region() {
        let region = strip_region_bytes(240); // 0xa500
        // address == region size is out of region (>= predicate).
        assert_eq!(
            mv_source_offset_in_strip_region(0xa500, 0, region),
            MvSourceOffsetClass::OutOfRegion,
        );
    }

    #[test]
    fn mv_source_offset_one_past_region_end_is_out_of_region() {
        let region = strip_region_bytes(240); // 0xa500
        assert_eq!(
            mv_source_offset_in_strip_region(0xa4ff, 1, region),
            MvSourceOffsetClass::OutOfRegion,
        );
    }

    #[test]
    fn mv_source_offset_one_under_region_end_is_in_region() {
        let region = strip_region_bytes(240); // 0xa500
        // address 0xa4ff < 0xa500: in region.
        assert_eq!(
            mv_source_offset_in_strip_region(0xa4fe, 1, region),
            MvSourceOffsetClass::InRegion,
        );
    }

    // ---- mv_source_offset_in_strip_region — underflow ----

    #[test]
    fn mv_source_offset_negative_past_zero_is_underflow() {
        let region = strip_region_bytes(240);
        // dst_cell_base = 0x100, mv_offset = -0x200: would underflow.
        assert_eq!(
            mv_source_offset_in_strip_region(0x100, -0x200, region),
            MvSourceOffsetClass::Underflow,
        );
    }

    #[test]
    fn mv_source_offset_negative_just_at_zero_is_in_region() {
        let region = strip_region_bytes(240);
        // dst_cell_base = 0x100, mv_offset = -0x100: lands at 0.
        assert_eq!(
            mv_source_offset_in_strip_region(0x100, -0x100, region),
            MvSourceOffsetClass::InRegion,
        );
    }

    #[test]
    fn mv_source_offset_negative_one_past_zero_is_underflow() {
        let region = strip_region_bytes(240);
        // dst_cell_base = 0x100, mv_offset = -0x101: just over.
        assert_eq!(
            mv_source_offset_in_strip_region(0x100, -0x101, region),
            MvSourceOffsetClass::Underflow,
        );
    }

    // ---- mv_source_offset_in_strip_region — zero-size region ----

    #[test]
    fn mv_source_offset_zero_size_region_is_always_out_of_region() {
        // strip_region_bytes(0) == 0: every address is `>= 0`, hence
        // out of region, except the underflow case (which still
        // classifies as Underflow first).
        let region = 0u64;
        assert_eq!(
            mv_source_offset_in_strip_region(0, 0, region),
            MvSourceOffsetClass::OutOfRegion,
        );
        assert_eq!(
            mv_source_offset_in_strip_region(0, 1, region),
            MvSourceOffsetClass::OutOfRegion,
        );
        assert_eq!(
            mv_source_offset_in_strip_region(0, -1, region),
            MvSourceOffsetClass::Underflow,
        );
    }

    // ---- mv_source_offset_in_strip_region — saturating add ----

    #[test]
    fn mv_source_offset_u64_max_addition_does_not_panic() {
        // Pathological large positive offset must not panic; classify
        // as OutOfRegion via overflow flag.
        let region = strip_region_bytes(240);
        assert_eq!(
            mv_source_offset_in_strip_region(u64::MAX, i64::MAX, region),
            MvSourceOffsetClass::OutOfRegion,
        );
    }

    // ---- PaddingPixelPreservation ----

    #[test]
    fn padding_pixel_preservation_codec_init_predicates() {
        let v = PaddingPixelPreservation::DeterministicAtCodecInit;
        assert!(v.is_codec_init());
        assert!(!v.is_frame_to_frame());
    }

    #[test]
    fn padding_pixel_preservation_frame_to_frame_predicates() {
        let v = PaddingPixelPreservation::PreservedAcrossFramesByStripEdgeFixup;
        assert!(!v.is_codec_init());
        assert!(v.is_frame_to_frame());
    }

    #[test]
    fn padding_pixel_preservation_two_variants_distinct() {
        assert_ne!(
            PaddingPixelPreservation::DeterministicAtCodecInit,
            PaddingPixelPreservation::PreservedAcrossFramesByStripEdgeFixup,
        );
    }

    // ---- cross-module sanity ----

    #[test]
    fn worked_example_uses_canonical_row_stride() {
        // Documentation invariant: the §4.4 worked example uses the
        // §4.1 row-stride constant verbatim.
        assert_eq!(
            STRIP_REGION_LUMA_240_BYTES,
            (MC_ARENA_ROW_STRIDE as u64) * 240,
        );
    }

    #[test]
    fn worked_example_classifier_zero_mv_in_region() {
        // A zero-MV copy from a destination at the strip-region
        // midpoint should classify as in-region for the §4.4 worked
        // example.
        let region = STRIP_REGION_LUMA_240_BYTES;
        let mid = region / 2;
        assert_eq!(
            mv_source_offset_in_strip_region(mid, 0, region),
            MvSourceOffsetClass::InRegion,
        );
    }
}
