//! Indeo 3 spec/05 §2.2 / §2.3 / §3.3 / §3.4 — packed-MV bit-layout
//! decode and four-way mode dispatch.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/05-motion-compensation.md`
//! §2.2 (the four-way mode dispatch), §2.3 (source-pointer arithmetic),
//! §3.3 (the packing formula `176 * vert + horiz`) and §3.4 (the
//! packed-MV byte layout `bits 31..2 = pixel_offset`, `bit 1 = horiz
//! half-pel`, `bit 0 = vert half-pel`).
//!
//! Round 12 ([`super::mc_table`]) closed the §1 chapter — table layout
//! and INTER-leaf indexing up to but not including the table
//! dereference. This module owns the next slice of the pipeline: given
//! the already-fetched 32-bit packed-MV word, decompose it into the
//! signed strip-pixel byte offset and the half-pel filter-mode selector
//! the §2.2 dispatcher branches on, and surface the §2.3
//! `src_addr = dst_cell_base + sign_extend(packed_MV >> 2)`
//! source-pointer arithmetic.
//!
//! This module surfaces:
//!
//! * [`MV_PIXEL_OFFSET_SHIFT`] = `2` — the §2.3 / §3.4 arithmetic
//!   right-shift the dispatcher applies to recover the signed pixel
//!   byte offset from the upper 30 bits.
//! * [`MV_VERT_HALFPEL_BIT`] = `0x1` / [`MV_HORIZ_HALFPEL_BIT`] = `0x2`
//!   / [`MV_MODE_BITS_MASK`] = `0x3` — the §3.4 low-two-bit field
//!   labels and combined mask matching `test edx, 0x1` / `test edx,
//!   0x2` at `IR32_32.DLL!0x100066e0..0x100066ee`.
//! * [`MV_PIXEL_OFFSET_ROW_STRIDE`] = `176` (`0xb0`) — the §3.3
//!   `11 * vert << 4 = 176 * vert` row-stride constant aliasing
//!   [`super::reconstruct::PREDICTOR_ROW_STRIDE`].
//! * [`PackedMv`] — the typed view over a packed-MV DWORD with
//!   `pixel_offset()` (signed `sar 2`) and `mode()` returning
//!   [`McDispatchMode`]. (The §3.3 `(vert, horiz)` pair is *not*
//!   re-decomposed here: the §2.3 dispatcher uses the combined
//!   offset directly, and any decomposition would require fixing a
//!   division convention the spec does not pin down — see the §3.3
//!   note on rare `|horiz| ≥ row_stride/2` ambiguity.)
//! * [`McDispatchMode`] — the §2.2 four-way fork
//!   (`FullPel` / `VerticalHalfPel` / `HorizontalHalfPel` /
//!   `BothHalfPel`) with each variant carrying its inner-loop RVA
//!   (`0x1000670d` / `0x10006780` / `0x1000684b` / `0x100068f8`).
//! * [`apply_mv_source_offset`] — the §2.3 sign-extending
//!   `add esi, edx` that produces the source-pixel address from the
//!   destination cell base and the packed MV.
//! * [`pack_mv_components`] — the constructive inverse of
//!   [`PackedMv::pixel_offset`] / [`PackedMv::mode`], surfacing the
//!   §3.3 closing-arithmetic write `((176*vert + horiz) << 2) |
//!   (horiz_lsb << 1) | vert_lsb` so round-trip tests and fuzz
//!   harnesses can build packed words from `(vert, horiz, vert_lsb,
//!   horiz_lsb)` directly.
//!
//! What this module **deliberately does not do** (the §3 / §5 chapter
//! boundary):
//!
//! * It does not consume the bitstream. The MV-index byte read and the
//!   table dereference live in [`super::mc_table`] / a future
//!   table-read helper; this module operates on the already-fetched
//!   32-bit DWORD.
//! * It does not perform the §5.1 / §5.2 / §5.3 cell copy. The
//!   per-row byte-pair averaging filter and the `0xb0`-stride
//!   destination walk belong with the strip pixel-buffer surface and
//!   are still future work.
//! * It does not validate the resulting source-pixel address against
//!   any strip-buffer bound. Per §4.4 the binary performs no such
//!   check; callers that wish to bound a well-formed stream do it at
//!   the strip-arena view, not here.
//!
//! All offsets, field widths, RVAs and bit assignments are taken from
//! `05-motion-compensation.md` §2 (§2.2 / §2.3), §3 (§3.3 / §3.4) and
//! cross-checked against §1 for the source / destination of the table
//! word being decoded. RVAs cited in doc-comments refer to the binary
//! identified in `spec/00 §2`.

// ---- §3.3 / §3.4 layout constants ----------------------------------

/// Spec/05 §3.4 — `bit 0` mask within a packed-MV DWORD.
///
/// Set iff the cell uses vertical half-pel interpolation (`(p, p+row)`
/// average per §2.2 row `01`). Equal to the post-`sar` LSB of the
/// vertical component in arm `0x100044a0` (vertical-only) and
/// `0x100043bb` (both); always zero in the horizontal-only and
/// full-pel arms.
pub const MV_VERT_HALFPEL_BIT: u32 = 0x1;

/// Spec/05 §3.4 — `bit 1` mask within a packed-MV DWORD.
///
/// Set iff the cell uses horizontal half-pel interpolation
/// (`(p, p+1)` average per §2.2 row `10`). Equal to the post-`sar`
/// LSB of the horizontal component in arm `0x10004433`
/// (horizontal-only) and `0x100043bb` (both); always zero in the
/// vertical-only and full-pel arms.
pub const MV_HORIZ_HALFPEL_BIT: u32 = 0x2;

/// Spec/05 §3.4 — combined `bits 1..0` half-pel mode mask (`0x3`).
///
/// The §2.2 dispatcher tests these two bits to fork between the four
/// MC inner loops (`test edx, 0x1; jne ...; test edx, 0x2; jne ...`
/// at `IR32_32.DLL!0x100066e0..0x100066ee`).
pub const MV_MODE_BITS_MASK: u32 = MV_VERT_HALFPEL_BIT | MV_HORIZ_HALFPEL_BIT;

/// Spec/05 §2.3 / §3.4 — `sar` amount the dispatcher applies to
/// recover the signed strip-pixel byte offset from the packed-MV
/// DWORD (`2`).
///
/// `sar edx, 0x2` at `IR32_32.DLL!0x100066f3` (full-pel arm) and the
/// matching site in each half-pel sibling drops the two mode bits and
/// leaves a signed integer-pixel byte offset that is added to the
/// destination cell base to compute the source-pixel address.
pub const MV_PIXEL_OFFSET_SHIFT: u32 = 2;

/// Spec/05 §3.3 — row-stride constant used to fold the vertical
/// displacement into the packed-MV's pixel-byte offset (`176` =
/// `0xb0`).
///
/// The §3.3 packing arithmetic computes `(vert + 10*vert) << 4 =
/// 176 * vert` before adding the horizontal component, matching the
/// `0xb0` row stride of the strip pixel buffer
/// ([`super::reconstruct::PREDICTOR_ROW_STRIDE`]). The two values are
/// the same physical constant: one expresses "rows down in the
/// destination buffer", the other expresses "rows down between MV
/// source and destination". They MUST agree.
pub const MV_PIXEL_OFFSET_ROW_STRIDE: i32 = super::reconstruct::PREDICTOR_ROW_STRIDE as i32;

// ---- §2.2 four-way MC dispatch -------------------------------------

/// Spec/05 §2.2 — the four-way MC dispatch fork on the packed-MV's
/// bottom two bits.
///
/// The dispatcher at `IR32_32.DLL!0x100066e0` tests `bit 0` first
/// (vertical-half-pel branch) then `bit 1` (horizontal-half-pel
/// branch); the resulting four paths each have a distinct inner loop
/// at the RVAs listed below. The full-pel path falls through, the
/// half-pel paths apply a 1-D `(a + b) >> 1` byte-parallel average,
/// and the both-half-pel path applies a 2×2 box filter
/// (`(a + b + c + d) >> 2`-ish, byte-lane carry-stripped — see §2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McDispatchMode {
    /// `bits 1..0 = 00` — full-pel, no half-pel filter applied.
    ///
    /// Inner loop at `IR32_32.DLL!0x1000670d` (the `0x100066f8` ->
    /// `0x1000670d` fork). Source byte is read straight from
    /// `[esi]` after the §2.3 `add esi, sar(packed_mv, 2)`.
    FullPel,
    /// `bits 1..0 = 01` — vertical half-pel only.
    ///
    /// Inner loop at `IR32_32.DLL!0x10006780` (the `0x10006759` ->
    /// `0x10006780` fork). Each output byte is `(src[i] + src[i +
    /// 0xb0]) >> 1`, byte-parallel with the `0x7f7f7f7f` carry mask.
    VerticalHalfPel,
    /// `bits 1..0 = 10` — horizontal half-pel only.
    ///
    /// Inner loop at `IR32_32.DLL!0x1000684b` (the `0x1000682e` ->
    /// `0x1000684b` fork). Each output byte is `(src[i] + src[i +
    /// 1]) >> 1`, byte-parallel.
    HorizontalHalfPel,
    /// `bits 1..0 = 11` — both half-pel (2×2 box filter).
    ///
    /// Inner loop at `IR32_32.DLL!0x100068f8` (the `0x100068db` ->
    /// `0x100068f8` fork). Each output byte is the 2×2 unweighted
    /// average of `src[i]`, `src[i+1]`, `src[i+0xb0]`,
    /// `src[i+0xb1]`.
    BothHalfPel,
}

impl McDispatchMode {
    /// Spec/05 §2.2 — recover the four-way dispatch from the low two
    /// bits of a packed-MV DWORD. All bits other than `MV_MODE_BITS_MASK`
    /// are ignored.
    pub const fn from_packed_mv(packed: u32) -> Self {
        match packed & MV_MODE_BITS_MASK {
            0b00 => Self::FullPel,
            0b01 => Self::VerticalHalfPel,
            0b10 => Self::HorizontalHalfPel,
            _ => Self::BothHalfPel,
        }
    }

    /// Spec/05 §2.2 — the encoded `bits 1..0` for this dispatch mode.
    /// Useful as the constructive inverse of [`Self::from_packed_mv`].
    pub const fn mode_bits(self) -> u32 {
        match self {
            Self::FullPel => 0b00,
            Self::VerticalHalfPel => 0b01,
            Self::HorizontalHalfPel => 0b10,
            Self::BothHalfPel => 0b11,
        }
    }

    /// Spec/05 §2.2 — RVA of the inner-loop entry for this dispatch
    /// mode. Useful for cross-referencing the §2.2 fork table back to
    /// the static-analysis citation.
    pub const fn inner_loop_rva(self) -> u32 {
        match self {
            Self::FullPel => 0x1000670d,
            Self::VerticalHalfPel => 0x10006780,
            Self::HorizontalHalfPel => 0x1000684b,
            Self::BothHalfPel => 0x100068f8,
        }
    }

    /// Spec/05 §2.2 — true iff this mode applies a vertical
    /// half-pel filter (averaging `src[i]` with `src[i + row_stride]`).
    pub const fn applies_vertical_half_pel(self) -> bool {
        matches!(self, Self::VerticalHalfPel | Self::BothHalfPel)
    }

    /// Spec/05 §2.2 — true iff this mode applies a horizontal
    /// half-pel filter (averaging `src[i]` with `src[i + 1]`).
    pub const fn applies_horizontal_half_pel(self) -> bool {
        matches!(self, Self::HorizontalHalfPel | Self::BothHalfPel)
    }

    /// Spec/05 §2.2 — true iff this mode applies any half-pel filter.
    pub const fn is_half_pel(self) -> bool {
        !matches!(self, Self::FullPel)
    }
}

// ---- §3.4 packed-MV view -------------------------------------------

/// Spec/05 §3.4 — typed view over a 32-bit packed-MV DWORD as read
/// from the per-plane MV table (`[inner_instance + 4*i]`).
///
/// The wire encoding is:
///
/// ```text
/// bits 31..2 : pixel_offset, signed, = 176 * vert + horiz
/// bit  1     : horizontal half-pel filter flag
/// bit  0     : vertical half-pel filter flag
/// ```
///
/// The dispatcher does *not* validate that the pixel_offset stays
/// inside any particular strip buffer (per §4.4); callers that need
/// such a bound apply it at the strip-arena view. This struct simply
/// exposes the wire decomposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackedMv {
    /// Raw little-endian DWORD as fetched from
    /// `inner_instance[4*i]`.
    pub raw: u32,
}

impl PackedMv {
    /// Spec/05 §1.3 / §3.4 — build a typed view from the raw DWORD.
    pub const fn from_raw(raw: u32) -> Self {
        Self { raw }
    }

    /// Spec/05 §3.4 — `bit 0`: vertical-half-pel filter flag.
    pub const fn vert_half_pel_bit(self) -> bool {
        (self.raw & MV_VERT_HALFPEL_BIT) != 0
    }

    /// Spec/05 §3.4 — `bit 1`: horizontal-half-pel filter flag.
    pub const fn horiz_half_pel_bit(self) -> bool {
        (self.raw & MV_HORIZ_HALFPEL_BIT) != 0
    }

    /// Spec/05 §2.2 — four-way MC dispatch mode this packed MV
    /// targets, derived from `bits 1..0` per §3.4.
    pub const fn mode(self) -> McDispatchMode {
        McDispatchMode::from_packed_mv(self.raw)
    }

    /// Spec/05 §2.3 / §3.4 — recover the signed strip-pixel byte
    /// offset from `bits 31..2` via the dispatcher's
    /// `sar edx, 0x2` at `IR32_32.DLL!0x100066f3`.
    ///
    /// The shift is arithmetic, so a packed MV with `bit 31` set
    /// decodes to a negative offset (the source pixel sits before
    /// the destination cell base in the strip-buffer linear
    /// addressing). The low two bits are discarded by the shift —
    /// they belong to the mode-bit field, not to the offset.
    pub const fn pixel_offset(self) -> i32 {
        (self.raw as i32) >> MV_PIXEL_OFFSET_SHIFT
    }

    /// Spec/05 §2.3 — apply this MV's pixel offset to a destination
    /// cell base address, returning the source-pixel address the MC
    /// fetcher reads from. Matches the `add esi, edx` at
    /// `IR32_32.DLL!0x100066f6` (full-pel arm) and the matching site
    /// in each half-pel sibling.
    ///
    /// The arithmetic is byte-addressing throughout; no separate row
    /// and column components are added. Returns `None` if the
    /// signed addition would underflow `0` (i.e. the source address
    /// would be negative). Per §4.4 the binary performs no such
    /// check, so callers that want the unchecked behaviour can use
    /// [`Self::pixel_offset`] directly.
    pub const fn source_address(self, dst_cell_base: usize) -> Option<usize> {
        apply_mv_source_offset(dst_cell_base, self.pixel_offset())
    }
}

// ---- §2.3 source-pointer arithmetic --------------------------------

/// Spec/05 §2.3 — apply a signed pixel byte offset to a destination
/// cell base address, returning the source-pixel address.
///
/// Models `add esi, edx` at the §2.3 fork tail. Returns `None` when
/// the resulting `(dst + offset) as i64` is negative (i.e. would
/// require a memory address before `0`). Per §4.4 the binary itself
/// performs no such check; this function clamps purely so callers in
/// a safe-Rust strip-buffer view can detect the wire-malformed case
/// without panicking.
pub const fn apply_mv_source_offset(dst_cell_base: usize, offset: i32) -> Option<usize> {
    let signed = dst_cell_base as i64 + offset as i64;
    if signed < 0 {
        None
    } else {
        Some(signed as usize)
    }
}

// ---- §3.3 constructive packer (for round-trip / fuzz) --------------

/// Spec/05 §3.3 — pack `(vert, horiz, vert_lsb, horiz_lsb)` into the
/// packed-MV DWORD per the §3.3 closing arithmetic
/// (`IR32_32.DLL!0x100043f6..0x10004426` for the both-half-pel arm,
/// the matching sites in each sibling arm for the others).
///
/// The closing arithmetic is:
///
/// ```text
/// packed = ((176 * vert + horiz) << 2)
///        | ((horiz_lsb & 1) << 1)
///        | ((vert_lsb  & 1) << 0)
/// ```
///
/// This is the constructive inverse of [`PackedMv::pixel_offset`] and
/// [`PackedMv::mode`]: any sequence of `(vert, horiz, vert_lsb,
/// horiz_lsb)` that the parser arms can emit can be reconstructed
/// here, and the resulting DWORD round-trips through
/// [`PackedMv::from_raw`].
///
/// `vert` is the post-`sar` signed vertical component, `horiz` is the
/// post-`sar` signed horizontal component, and the two `_lsb` values
/// are the `& 1` of the post-shift components in the half-pel arms
/// (always zero in the full-pel arm). Only `bit 0` of each `_lsb` is
/// consulted; higher bits are masked off.
pub const fn pack_mv_components(vert: i32, horiz: i32, vert_lsb: u32, horiz_lsb: u32) -> u32 {
    let pixel_offset = MV_PIXEL_OFFSET_ROW_STRIDE * vert + horiz;
    let high = (pixel_offset as u32) << MV_PIXEL_OFFSET_SHIFT;
    let h_bit = (horiz_lsb & 1) << 1;
    let v_bit = vert_lsb & 1;
    high | h_bit | v_bit
}

// ---- consistency assertions ----------------------------------------

const _: () = {
    // §3.4 mask bits are disjoint and exhaustive over `bits 1..0`.
    assert!(MV_VERT_HALFPEL_BIT == 0x1);
    assert!(MV_HORIZ_HALFPEL_BIT == 0x2);
    assert!(MV_VERT_HALFPEL_BIT & MV_HORIZ_HALFPEL_BIT == 0);
    assert!(MV_MODE_BITS_MASK == 0x3);
    // §2.3 / §3.4 shift recovers the same number of bits as the mask
    // covers.
    assert!((1u32 << MV_PIXEL_OFFSET_SHIFT) == MV_MODE_BITS_MASK + 1);
    // §3.3 row-stride matches the reconstruction-side predictor
    // stride; the two constants are aliases of the same physical
    // value.
    assert!(MV_PIXEL_OFFSET_ROW_STRIDE == 0xb0);
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::reconstruct::PREDICTOR_ROW_STRIDE;

    // ---- §3.4 layout constants -------------------------------------

    #[test]
    fn mode_bits_are_disjoint() {
        // §3.4: bit 0 = vert, bit 1 = horiz; their union is the
        // §2.2 dispatch mask.
        assert_eq!(MV_VERT_HALFPEL_BIT, 0x1);
        assert_eq!(MV_HORIZ_HALFPEL_BIT, 0x2);
        assert_eq!(MV_MODE_BITS_MASK, 0x3);
        assert_eq!(
            MV_VERT_HALFPEL_BIT | MV_HORIZ_HALFPEL_BIT,
            MV_MODE_BITS_MASK
        );
        assert_eq!(MV_VERT_HALFPEL_BIT & MV_HORIZ_HALFPEL_BIT, 0);
    }

    #[test]
    fn pixel_offset_shift_matches_mode_mask_width() {
        // §2.3 `sar edx, 0x2` discards exactly the two mode bits.
        assert_eq!(MV_PIXEL_OFFSET_SHIFT, 2);
        assert_eq!(1u32 << MV_PIXEL_OFFSET_SHIFT, MV_MODE_BITS_MASK + 1);
    }

    #[test]
    fn row_stride_aliases_predictor_stride() {
        // §3.3 `176 * vert` shares the physical row stride with the
        // §1.1 `[edi - 0xb0]` predictor address. The two MUST agree.
        assert_eq!(MV_PIXEL_OFFSET_ROW_STRIDE, 0xb0);
        assert_eq!(MV_PIXEL_OFFSET_ROW_STRIDE, 176);
        assert_eq!(MV_PIXEL_OFFSET_ROW_STRIDE as usize, PREDICTOR_ROW_STRIDE);
    }

    // ---- §2.2 four-way mode dispatch -------------------------------

    #[test]
    fn dispatch_full_pel_when_both_bits_clear() {
        // §2.2 row 1: `00` → full-pel.
        assert_eq!(McDispatchMode::from_packed_mv(0), McDispatchMode::FullPel);
        assert_eq!(McDispatchMode::FullPel.mode_bits(), 0b00);
        assert_eq!(McDispatchMode::FullPel.inner_loop_rva(), 0x1000670d);
        assert!(!McDispatchMode::FullPel.is_half_pel());
        assert!(!McDispatchMode::FullPel.applies_vertical_half_pel());
        assert!(!McDispatchMode::FullPel.applies_horizontal_half_pel());
    }

    #[test]
    fn dispatch_vertical_half_pel_when_bit0_set() {
        // §2.2 row 2: `01` → vertical-only.
        let mode = McDispatchMode::from_packed_mv(MV_VERT_HALFPEL_BIT);
        assert_eq!(mode, McDispatchMode::VerticalHalfPel);
        assert_eq!(mode.mode_bits(), 0b01);
        assert_eq!(mode.inner_loop_rva(), 0x10006780);
        assert!(mode.is_half_pel());
        assert!(mode.applies_vertical_half_pel());
        assert!(!mode.applies_horizontal_half_pel());
    }

    #[test]
    fn dispatch_horizontal_half_pel_when_bit1_set() {
        // §2.2 row 3: `10` → horizontal-only.
        let mode = McDispatchMode::from_packed_mv(MV_HORIZ_HALFPEL_BIT);
        assert_eq!(mode, McDispatchMode::HorizontalHalfPel);
        assert_eq!(mode.mode_bits(), 0b10);
        assert_eq!(mode.inner_loop_rva(), 0x1000684b);
        assert!(mode.is_half_pel());
        assert!(!mode.applies_vertical_half_pel());
        assert!(mode.applies_horizontal_half_pel());
    }

    #[test]
    fn dispatch_both_half_pel_when_both_bits_set() {
        // §2.2 row 4: `11` → 2×2 box.
        let mode = McDispatchMode::from_packed_mv(MV_MODE_BITS_MASK);
        assert_eq!(mode, McDispatchMode::BothHalfPel);
        assert_eq!(mode.mode_bits(), 0b11);
        assert_eq!(mode.inner_loop_rva(), 0x100068f8);
        assert!(mode.is_half_pel());
        assert!(mode.applies_vertical_half_pel());
        assert!(mode.applies_horizontal_half_pel());
    }

    #[test]
    fn dispatch_ignores_pixel_offset_bits() {
        // §2.2 dispatcher tests *only* `bits 1..0` of the packed MV;
        // the upper 30 bits (the pixel-byte offset) must not perturb
        // the dispatch.
        let high_only = 0xffff_fffc;
        assert_eq!(
            McDispatchMode::from_packed_mv(high_only),
            McDispatchMode::FullPel
        );
        let high_plus_vert = high_only | MV_VERT_HALFPEL_BIT;
        assert_eq!(
            McDispatchMode::from_packed_mv(high_plus_vert),
            McDispatchMode::VerticalHalfPel
        );
        let high_plus_horiz = high_only | MV_HORIZ_HALFPEL_BIT;
        assert_eq!(
            McDispatchMode::from_packed_mv(high_plus_horiz),
            McDispatchMode::HorizontalHalfPel
        );
        let high_plus_both = high_only | MV_MODE_BITS_MASK;
        assert_eq!(
            McDispatchMode::from_packed_mv(high_plus_both),
            McDispatchMode::BothHalfPel
        );
    }

    #[test]
    fn dispatch_inner_loop_rvas_are_distinct() {
        // §2.2 table has four distinct inner-loop entries.
        let modes = [
            McDispatchMode::FullPel,
            McDispatchMode::VerticalHalfPel,
            McDispatchMode::HorizontalHalfPel,
            McDispatchMode::BothHalfPel,
        ];
        let mut seen = [0u32; 4];
        for (i, m) in modes.iter().enumerate() {
            seen[i] = m.inner_loop_rva();
        }
        // O(n^2) on n=4 — trivial.
        for i in 0..seen.len() {
            for j in (i + 1)..seen.len() {
                assert_ne!(
                    seen[i], seen[j],
                    "inner-loop RVAs for {:?} and {:?} collide",
                    modes[i], modes[j]
                );
            }
        }
    }

    #[test]
    fn dispatch_mode_bits_roundtrip() {
        // `mode_bits` is the constructive inverse of `from_packed_mv`
        // restricted to the mode-bit field.
        for m in [
            McDispatchMode::FullPel,
            McDispatchMode::VerticalHalfPel,
            McDispatchMode::HorizontalHalfPel,
            McDispatchMode::BothHalfPel,
        ] {
            assert_eq!(McDispatchMode::from_packed_mv(m.mode_bits()), m);
        }
    }

    // ---- §2.3 / §3.4 packed-MV decomposition -----------------------

    #[test]
    fn pixel_offset_is_signed_sar() {
        // §2.3 `sar edx, 0x2` is arithmetic; bit 31 of the raw DWORD
        // therefore sign-extends into the recovered pixel offset.
        let positive = PackedMv::from_raw(0x0000_0010);
        assert_eq!(positive.pixel_offset(), 4);
        // Top bit set + zero mode bits: a -1 pixel offset
        // (`0xfffffffc as i32 >> 2 = -1`).
        let minus_one = PackedMv::from_raw(0xffff_fffc);
        assert_eq!(minus_one.pixel_offset(), -1);
        // A large negative offset.
        let large_neg = PackedMv::from_raw(0x8000_0000);
        assert_eq!(large_neg.pixel_offset(), i32::MIN / 4);
        // Pure mode bits → zero pixel offset.
        let pure_modes = PackedMv::from_raw(MV_MODE_BITS_MASK);
        assert_eq!(pure_modes.pixel_offset(), 0);
    }

    #[test]
    fn pack_mv_components_produces_expected_pixel_offset() {
        // §3.3: the packer combines (vert, horiz) into a single
        // signed pixel byte offset = 176 * vert + horiz, then OR-s
        // the two mode bits. PackedMv::pixel_offset recovers the
        // combined value exactly.
        for vert in [-3, -1, 0, 1, 3, 5] {
            for horiz in [-100i32, -1, 0, 1, 100] {
                let packed = pack_mv_components(vert, horiz, 0, 0);
                let mv = PackedMv::from_raw(packed);
                assert_eq!(
                    mv.pixel_offset(),
                    MV_PIXEL_OFFSET_ROW_STRIDE * vert + horiz,
                    "pixel_offset mismatch for ({vert}, {horiz})"
                );
            }
        }
    }

    #[test]
    fn mode_bits_survive_packing() {
        // §3.3 closing arithmetic ORs the two LSBs into the packed
        // word; the resulting `mode()` MUST report what was packed.
        for (v_lsb, h_lsb, expected) in [
            (0, 0, McDispatchMode::FullPel),
            (1, 0, McDispatchMode::VerticalHalfPel),
            (0, 1, McDispatchMode::HorizontalHalfPel),
            (1, 1, McDispatchMode::BothHalfPel),
        ] {
            let raw = pack_mv_components(3, 7, v_lsb, h_lsb);
            let mv = PackedMv::from_raw(raw);
            assert_eq!(mv.mode(), expected);
            assert_eq!(mv.vert_half_pel_bit(), v_lsb == 1);
            assert_eq!(mv.horiz_half_pel_bit(), h_lsb == 1);
            // Pixel offset is unchanged by the mode-bit packing.
            assert_eq!(mv.pixel_offset(), MV_PIXEL_OFFSET_ROW_STRIDE * 3 + 7);
        }
    }

    #[test]
    fn pack_mv_components_round_trips_for_full_pel() {
        // The full-pel arm packs with both LSBs cleared; for
        // representative (vert, horiz) the result MUST round-trip
        // through PackedMv::from_raw / pixel_offset / mode.
        for vert in -3..=3i32 {
            for horiz in -10..=10i32 {
                let packed = pack_mv_components(vert, horiz, 0, 0);
                let mv = PackedMv::from_raw(packed);
                assert_eq!(mv.mode(), McDispatchMode::FullPel);
                assert_eq!(mv.pixel_offset(), MV_PIXEL_OFFSET_ROW_STRIDE * vert + horiz);
            }
        }
    }

    // ---- §2.3 source-pointer arithmetic ----------------------------

    #[test]
    fn source_address_adds_pixel_offset_to_base() {
        // §2.3: `src = dst_cell_base + sign_extend(packed_MV >> 2)`.
        let base = 0x1_0000usize;
        // Positive offset.
        let mv_pos = PackedMv::from_raw(pack_mv_components(0, 16, 0, 0));
        assert_eq!(mv_pos.source_address(base), Some(base + 16));
        // Negative offset (vert = -1, horiz = 0 → -176).
        let mv_neg = PackedMv::from_raw(pack_mv_components(-1, 0, 0, 0));
        assert_eq!(mv_neg.source_address(base), Some(base - 176));
    }

    #[test]
    fn source_address_clamps_negative_underflow() {
        // §4.4 says the binary does not bounds-check; we offer a
        // None for the safe-Rust caller that wants to detect a
        // wire-malformed offset before computing it.
        let base = 100usize;
        let mv = PackedMv::from_raw(pack_mv_components(-1, 0, 0, 0)); // -176
        assert_eq!(mv.source_address(base), None);
    }

    #[test]
    fn apply_mv_source_offset_handles_zero_offset() {
        assert_eq!(apply_mv_source_offset(0, 0), Some(0));
        assert_eq!(apply_mv_source_offset(0x1234, 0), Some(0x1234));
    }

    #[test]
    fn apply_mv_source_offset_handles_negative_into_zero_base() {
        // dst_cell_base = 0, offset = -1 would land at usize "max" if
        // we wrapped; we explicitly return None instead.
        assert_eq!(apply_mv_source_offset(0, -1), None);
    }

    // ---- raw / mode roundtrip --------------------------------------

    #[test]
    fn raw_is_preserved_verbatim() {
        // `PackedMv::from_raw` is a pure newtype wrapper; the raw
        // DWORD MUST survive untouched.
        for raw in [
            0u32,
            1,
            2,
            3,
            0x1000_0000,
            0xffff_fffc,
            0xffff_ffff,
            MV_MODE_BITS_MASK,
            0xdead_beef,
        ] {
            assert_eq!(PackedMv::from_raw(raw).raw, raw);
        }
    }

    #[test]
    fn pack_then_decode_round_trips_modes_at_zero_offset() {
        // For pixel_offset = 0 the pack/unpack is degenerate but the
        // mode bits MUST still survive intact.
        for (v_lsb, h_lsb) in [(0u32, 0u32), (1, 0), (0, 1), (1, 1)] {
            let raw = pack_mv_components(0, 0, v_lsb, h_lsb);
            let mv = PackedMv::from_raw(raw);
            assert_eq!(mv.pixel_offset(), 0);
            assert_eq!(mv.vert_half_pel_bit(), v_lsb == 1);
            assert_eq!(mv.horiz_half_pel_bit(), h_lsb == 1);
        }
    }
}
