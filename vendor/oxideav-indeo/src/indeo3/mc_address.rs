//! Indeo 3 spec/05 §5.4 / §7.2 — cell-position decoding entry
//! (the cell-state dispatcher's index-arithmetic chain that
//! resolves the per-cell destination and source pixel-buffer
//! addresses the MC fetcher's inner loop consumes).
//!
//! Spec source: `docs/video/indeo/indeo3/spec/05-motion-compensation.md`
//! §5.4 (destination derivation via the strip-context per-cell
//! sub-array + `bank[+0x700][cl]`), §7.2 (the
//! `idx_dst = 16 * cell_slot + dst_slot` /
//! `idx_src = 16 * cell_slot + src_slot` cell-state dispatcher
//! arithmetic, the `dst_addr = dst_cell_data + cell_pos_aux` /
//! `src_addr = src_cell_data + cell_pos_aux + (packed_MV >> 2)`
//! composition), and §4.3 (the `[esp+0x54]` / `[esp+0x58]`
//! source-slot / destination-slot arguments to the per-plane
//! decoder).
//!
//! Round 14 ([`super::mc_kernel`]) closed §5.1 / §5.2 / §5.3 —
//! the per-DWORD copy / averaging kernels and the
//! [`super::McKernelGeometry`] (cell-width × cell-height) typed
//! shape. The inputs *to* those kernels are two pixel-buffer
//! byte addresses (`dst_addr`, `src_addr`) and a column-group /
//! row-band count. Round 15 takes the next slice of the pipeline:
//! the **cell-position decoding entry** — how the cell-state
//! dispatcher at `IR32_32.DLL!0x10006615..0x100066dc` builds those
//! addresses from
//!
//! * the per-plane decoder's destination-slot / source-slot
//!   arguments (`[esp+0x58]` / `[esp+0x54]` per §4.3 and
//!   `spec/02 §6` table rows 2-3),
//! * the per-cell-state slot index `cell_slot = bank[+0x200][ch]`
//!   (§7.2's first equation, the `bank+0x200` 256-entry sub-table
//!   already pinned by [`super::cell_loop::SLOT_INDEX_LUT`]),
//! * the per-cell sub-array DWORDs at
//!   `[strip_ctx + 16 * cell_slot + slot_arg]`
//!   (the "destination cell-data DWORD" / "source cell-data
//!   DWORD" pair the dispatcher loads into `[esp+0x28]` /
//!   `[esp+0x24]`, per §4.3 and §7.2),
//! * the per-cell auxiliary offset `bank[+0x700][cl]` (already
//!   pinned by [`super::cell_loop::CELL_POSITION_TABLE`]), and
//! * the §2.3 packed-MV signed pixel-offset
//!   (already pinned by [`super::mc_packed::PackedMv::pixel_offset`]).
//!
//! This module surfaces:
//!
//! * [`CELL_SLOT_STRIDE`] = `16` — the §7.2 / §4.3
//!   `shl eax, 0x4` (at `IR32_32.DLL!0x10006615`) that multiplies
//!   the per-cell-state slot index by 16 to form the per-cell
//!   sub-array stride.
//! * [`CELL_SLOT_INDEX_MAX`] = `15` — the upper bound on
//!   `cell_slot` derivable from the `shl eax, 0x4` immediately
//!   following a one-byte load. The §7.2 phrasing ("cell-slot
//!   index 0..15") confirms this is intended; the binary does
//!   not range-check the loaded byte.
//! * [`CellSlotBase`] — the typed result of
//!   `cell_slot << 4 = 16 * cell_slot`, with a constructor that
//!   takes the raw `bank[+0x200][ch]` byte and returns the cell-
//!   slot base index. Constrained to `[0, 255 * 16]` by the
//!   one-byte slot-index width.
//! * [`CellSubarrayIndex`] — the typed result of
//!   `idx_dst = 16 * cell_slot + dst_slot` /
//!   `idx_src = 16 * cell_slot + src_slot`, with the
//!   [`CellSubarrayIndex::dst`] / [`CellSubarrayIndex::src`]
//!   constructors. Used as the per-cell sub-array element index
//!   for the `mov edx, [esi + 4 * eax]` load at
//!   `IR32_32.DLL!0x10006641`.
//! * [`CellAddrEntry`] — the destination / source cell-data DWORD
//!   loaded by the dispatcher (`[esp+0x28]` for destination,
//!   `[esp+0x24]` for source), tagged with the role (`Dest` /
//!   `Src`) to keep the two-bank §4.2 / §4.3 plumbing
//!   distinguishable at the type level. The `extra_offset` field
//!   captures the §7.2 `[esp+0x38]` companion DWORD (loaded from
//!   `strip_ctx_arr[idx_src + 1]`) used by the §5.5 boundary fix-up.
//! * [`mc_dest_address`] — the §5.4 / §7.2
//!   `dst_addr = dst_cell_data + cell_pos_aux` composition; returns
//!   `None` on `u32` wrap (safe-Rust safety net; per §4.4 the
//!   binary itself does not bounds-check).
//! * [`mc_source_address`] — the §5.4 / §7.2
//!   `src_addr = src_cell_data + cell_pos_aux + sign_extend(packed_MV >> 2)`
//!   composition. Composes [`mc_dest_address`] with
//!   [`super::apply_mv_source_offset`]; returns `None` on either
//!   safe-Rust wrap or signed underflow of the MV step.
//! * [`McCellAddressPair`] — the (dst, src) pair the MC fetcher
//!   consumes at inner-loop entry, with a single
//!   [`McCellAddressPair::resolve`] constructor that runs the
//!   complete §7.2 chain (`cell_slot << 4` → per-cell sub-array
//!   indices → `dst_addr` / `src_addr` composition with the MV
//!   displacement applied to `src_addr` only). Returns
//!   [`McAddressError`] on any wrap.
//!
//! What this module **deliberately does not do** (the §5.4 / §7
//! chapter boundary):
//!
//! * It does not own the codebook bank's `+0x200` slot-index LUT
//!   or `+0x700` cell-position-aux LUT bytes. The per-entry values
//!   of those tables are pending an Extractor round per
//!   `spec/05 §7.5` and `§8.2 item 4`. This module accepts a
//!   pre-resolved `bank_slot_index_byte` (= `bank[+0x200][ch]`)
//!   and a pre-resolved `cell_pos_aux` (= `bank[+0x700][cl]`) as
//!   raw inputs.
//! * It does not own the strip-context per-cell sub-array DWORDs.
//!   Those are populated by the pre-frame cell-stack setup
//!   (`spec/03 §6` open question 4) and surfaced via
//!   [`super::cell_subarray`]'s read-side indexing arithmetic.
//!   This module accepts the destination / source cell-data
//!   DWORDs as inputs.
//! * It does not perform the §7.2 `cell_offset = bank[+0x700][cl]
//!   sar 2 + extra_offset + ch` reduction (the `[esp+0x34]`
//!   composite used by the §5.5 boundary fix-up). That arithmetic
//!   feeds the boundary-fix-up byte loop, not the MC fetcher
//!   inner-loop entry, and is out of scope for the §5.4 entry-
//!   point surface this module pins.
//! * It does not perform the §7.3 `(x, y, w, h)` recovery from
//!   the `dst_addr` byte address back into pixel coordinates.
//!   That decomposition is a reverse mapping required only for
//!   external readers (e.g. a renderer wanting to know where in
//!   the frame the cell lives); the decoder itself treats
//!   `dst_addr` as an opaque byte address and writes through it.
//! * It does not allocate, own, or bounds-check the strip pixel-
//!   buffer arena. Per §4.4 the binary itself performs no source-
//!   pointer range-check; safe-Rust callers that operate over an
//!   explicit pixel-buffer slice apply the check at the slice
//!   boundary, not here.
//! * It does not perform the §4.2 `frame_flags` bit 9 source /
//!   destination slot inversion. That decision is the per-plane
//!   decoder's job at `IR32_32.DLL!0x100045b1..0x100045fd`; this
//!   module accepts the *resolved* `(dst_slot, src_slot)` pair as
//!   inputs.
//!
//! All offsets, RVAs, and arithmetic identities are taken from
//! `05-motion-compensation.md` §4.3 / §5.4 / §7.2. RVAs cited in
//! doc-comments refer to the binary identified in `spec/00 §2`.

use super::mc_packed::PackedMv;

// ---- §7.2 / §4.3 (cell-slot stride and bounds) ---------------------

/// Spec/05 §7.2 / §4.3 — `shl eax, 0x4` immediate at
/// `IR32_32.DLL!0x10006615` (`16`).
///
/// The per-cell-state slot index (loaded as one byte from
/// `bank[+0x200][ch]`) is multiplied by 16 to form the per-cell
/// sub-array's `cell_slot * 16` base index. The destination and
/// source slot indices (each in `0..6` per
/// [`super::DISPATCHABLE_SLOT_COUNT`]) are then added to this base
/// to form the final per-cell sub-array element indices `idx_dst`
/// / `idx_src` consumed by the `mov edx, [esi + 4 * eax]` load at
/// `0x10006641`.
pub const CELL_SLOT_STRIDE: usize = 16;

/// Spec/05 §7.2 — upper bound on the slot index `cell_slot`
/// returned by `bank[+0x200][ch]` (`15`).
///
/// The §7.2 prose ("cell-slot index 0..15") matches the
/// dispatcher's `shl eax, 0x4` step: a one-byte index times 16 is
/// at most `255 * 16 = 4080`, but the chosen slot stride and the
/// §3.1 / §5 strip-context sub-array layout constrain the
/// per-plane in-use range to `[0, 15]`. Values beyond 15 still
/// produce a valid load (the per-cell sub-array fills the whole
/// `+0x40..+0x400` region of a strip-context slot; see
/// [`super::CELL_STACK_MAX_ENTRIES`] = `240`), but only the first
/// 16 cell-slot indices map to entries the pre-frame cell-stack
/// setup populates with cell-data DWORDs.
pub const CELL_SLOT_INDEX_MAX: u8 = 15;

// ---- §7.2 / §4.3 (typed slot-index surface) ------------------------

/// Spec/05 §7.2 — the `cell_slot * 16` base used to index the
/// per-cell sub-array.
///
/// Built from the one-byte `bank[+0x200][ch]` lookup by
/// [`CellSlotBase::from_bank_byte`]; combined with a §4.3
/// `(dst_slot, src_slot)` argument to form a
/// [`CellSubarrayIndex`].
///
/// The `byte_index` field is the post-`shl 0x4` value (= the
/// `bank` byte multiplied by 16), in `[0, 4080]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellSlotBase {
    byte_index: u32,
}

impl CellSlotBase {
    /// Spec/05 §7.2 — apply the `shl eax, 0x4` step at
    /// `IR32_32.DLL!0x10006615` to the raw one-byte
    /// `bank[+0x200][ch]` lookup.
    ///
    /// The §7.2 prose constrains the meaningful range to `[0, 15]`;
    /// values beyond that still produce a valid base index by the
    /// binary's arithmetic but are guaranteed to address an
    /// entry the pre-frame cell-stack setup did not populate
    /// (`spec/03 §6` open question 4).
    pub const fn from_bank_byte(bank_byte: u8) -> Self {
        Self {
            byte_index: (bank_byte as u32) << 4,
        }
    }

    /// True iff the source `bank_byte` was within the §7.2
    /// "cell-slot index 0..15" meaningful range.
    pub const fn is_within_meaningful_range(self) -> bool {
        self.byte_index <= (CELL_SLOT_INDEX_MAX as u32) << 4
    }

    /// The post-`shl 0x4` base index (= `16 * cell_slot`).
    pub const fn base_index(self) -> u32 {
        self.byte_index
    }
}

// ---- §7.2 (per-cell sub-array element index) -----------------------

/// Spec/05 §7.2 / §4.3 — typed per-cell sub-array element index.
///
/// The cell-state dispatcher composes
///
/// ```text
/// idx_dst = 16 * cell_slot + dst_slot     ; → [esp+0x28]
/// idx_src = 16 * cell_slot + src_slot     ; → [esp+0x24]
/// ```
///
/// at `IR32_32.DLL!0x10006638..0x10006641`. Each resolved index is
/// consumed by `mov edx, [esi + 4 * eax]` to load the four-byte
/// cell-data DWORD (the destination or source cell pixel-buffer
/// base pointer) into the dispatcher's scratch slots.
///
/// The `role` field records whether the index targets the
/// destination or source slot — the two halves of the §4.2 ping-
/// pong reference scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellSubarrayIndex {
    element_index: u32,
    role: CellAddrRole,
}

/// Spec/05 §4.2 / §4.3 — destination vs source cell-data role.
///
/// The §4.2 ping-pong scheme writes into the current frame's bank
/// (the destination slot) and reads from the previous frame's
/// bank (the source slot). The two slot indices arrive at the
/// cell-state dispatcher as separate `[esp+0x54]` / `[esp+0x58]`
/// arguments per `spec/02 §6` table rows 2-3; this enum keeps the
/// dispatcher's two scratch-slot writes (`[esp+0x24]` source,
/// `[esp+0x28]` destination) typed apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellAddrRole {
    /// The destination cell-data DWORD (current frame's bank,
    /// dispatcher scratch slot `[esp+0x28]`). Written through the
    /// MC fetcher's `edi` pointer.
    Dest,
    /// The source cell-data DWORD (previous frame's bank,
    /// dispatcher scratch slot `[esp+0x24]`). Read through the
    /// MC fetcher's `esi` pointer.
    Src,
}

impl CellSubarrayIndex {
    /// Spec/05 §7.2 — `idx_dst = 16 * cell_slot + dst_slot`.
    pub const fn dst(cell_slot: CellSlotBase, dst_slot: u32) -> Self {
        Self {
            element_index: cell_slot.byte_index + dst_slot,
            role: CellAddrRole::Dest,
        }
    }

    /// Spec/05 §7.2 — `idx_src = 16 * cell_slot + src_slot`.
    pub const fn src(cell_slot: CellSlotBase, src_slot: u32) -> Self {
        Self {
            element_index: cell_slot.byte_index + src_slot,
            role: CellAddrRole::Src,
        }
    }

    /// The resolved element index used by `mov edx, [esi + 4 * eax]`
    /// at `IR32_32.DLL!0x10006641`.
    pub const fn element_index(self) -> u32 {
        self.element_index
    }

    /// The byte offset of this entry within the per-cell sub-array
    /// (= `element_index * 4`, since each entry is a 4-byte DWORD).
    pub const fn byte_offset(self) -> u32 {
        self.element_index * 4
    }

    /// Whether this index targets the destination or source slot.
    pub const fn role(self) -> CellAddrRole {
        self.role
    }
}

// ---- §5.4 / §7.2 (cell-data DWORD entry) ---------------------------

/// Spec/05 §5.4 / §7.2 — typed cell-data DWORD as loaded from the
/// per-cell sub-array, plus the §7.2 `extra_offset` companion.
///
/// The dispatcher loads the cell-data DWORD with
///
/// ```text
/// mov edx, [esi + 4 * eax]      ; the cell pixel-buffer base
/// ```
///
/// where `[esi]` is the strip-context base pointer and `4 * eax` is
/// the post-shift byte offset built from [`CellSubarrayIndex::byte_offset`].
/// For the *source* role the dispatcher additionally loads
///
/// ```text
/// extra_offset = strip_ctx_arr[idx_src + 1]   ; → [esp+0x38]
/// ```
///
/// at `0x1000663e + 4` (the +1 element-index step), used by the
/// §5.5 boundary fix-up. For the *destination* role no such
/// companion is consumed.
///
/// Cell-data DWORDs are byte offsets into the strip's pixel-buffer
/// arena; they share the same `usize` representation as the source-
/// address arithmetic in [`super::apply_mv_source_offset`] so the
/// composition chain doesn't need a width conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellAddrEntry {
    cell_data_ptr: usize,
    extra_offset: Option<usize>,
    role: CellAddrRole,
}

impl CellAddrEntry {
    /// Construct a destination cell-data entry. The destination
    /// branch does not consume an `extra_offset` (the §7.2 +1
    /// element-index load is source-only).
    pub const fn dst(cell_data_ptr: usize) -> Self {
        Self {
            cell_data_ptr,
            extra_offset: None,
            role: CellAddrRole::Dest,
        }
    }

    /// Construct a source cell-data entry, carrying the §7.2
    /// `[esp+0x38]` companion DWORD (loaded from
    /// `strip_ctx_arr[idx_src + 1]`).
    pub const fn src(cell_data_ptr: usize, extra_offset: usize) -> Self {
        Self {
            cell_data_ptr,
            extra_offset: Some(extra_offset),
            role: CellAddrRole::Src,
        }
    }

    /// The cell-data DWORD itself (a byte pointer / offset into
    /// the strip's pixel-buffer arena).
    pub const fn cell_data_ptr(self) -> usize {
        self.cell_data_ptr
    }

    /// The §7.2 `[esp+0x38]` companion DWORD, present only for the
    /// source role.
    pub const fn extra_offset(self) -> Option<usize> {
        self.extra_offset
    }

    /// The role this entry was loaded for.
    pub const fn role(self) -> CellAddrRole {
        self.role
    }
}

// ---- §5.4 / §7.2 (address composition) -----------------------------

/// Spec/05 §5.4 / §7.2 — error from the address-composition chain.
///
/// All variants reflect safe-Rust `usize` wrap or signed `i32`
/// underflow. Per §4.4 the binary itself performs no bounds-check;
/// returning `Err` rather than panicking lets safe-Rust callers
/// reject malformed wire input rather than producing an in-range
/// but meaningless byte offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McAddressError {
    /// `dst_addr = dst_cell_data + cell_pos_aux` overflowed `usize`.
    DestAddressOverflow,
    /// `src_addr = src_cell_data + cell_pos_aux` overflowed `usize`
    /// before the MV displacement was applied.
    SrcAddressOverflow,
    /// `src_addr + sign_extend(packed_MV >> 2)` either overflowed
    /// `usize` (positive displacement) or underflowed (negative
    /// displacement); the §2.3 / §4.4 source-pointer arithmetic
    /// returned `None` from [`super::apply_mv_source_offset`].
    SrcMvDisplacementInvalid,
    /// The address arithmetic was attempted on a [`CellAddrEntry`]
    /// whose role does not match the requested operation
    /// (e.g. asking for a destination address from a source-role
    /// entry, or vice versa).
    RoleMismatch,
}

/// Spec/05 §5.4 / §7.2 — compose `dst_addr = dst_cell_data + cell_pos_aux`.
///
/// `cell_pos_aux` is `bank[+0x700][cl]` (already pinned by
/// [`super::CELL_POSITION_TABLE`]).
///
/// Returns `None` on `usize` wrap (the `Err(DestAddressOverflow)`
/// case at the [`McCellAddressPair::resolve`] layer). Per §4.4 the
/// binary itself does not range-check, but safe-Rust over an
/// explicit pixel-buffer slice does need this check; it lives at
/// this composition site rather than buried inside the kernel.
pub const fn mc_dest_address(dst_entry: CellAddrEntry, cell_pos_aux: usize) -> Option<usize> {
    dst_entry.cell_data_ptr.checked_add(cell_pos_aux)
}

/// Spec/05 §5.4 / §7.2 — compose
/// `src_addr = src_cell_data + cell_pos_aux + sign_extend(packed_MV >> 2)`.
///
/// Composes the §5.4 `src_base = src_cell_data + cell_pos_aux`
/// step (`usize` checked-add) with the §2.3 / §3.4
/// [`super::apply_mv_source_offset`] sign-extending MV displacement.
///
/// Returns `None` on either the `usize` wrap of the first step or
/// the signed under/overflow of the second.
pub fn mc_source_address(
    src_entry: CellAddrEntry,
    cell_pos_aux: usize,
    packed_mv: PackedMv,
) -> Option<usize> {
    let src_base = src_entry.cell_data_ptr.checked_add(cell_pos_aux)?;
    super::mc_packed::apply_mv_source_offset(src_base, packed_mv.pixel_offset())
}

// ---- §7.2 (dst / src address pair) ---------------------------------

/// Spec/05 §7.2 — the resolved (dst, src) byte-address pair the MC
/// fetcher inner loop consumes at entry.
///
/// `dst_addr` is composed from
/// `dst_cell_data + bank[+0x700][cl]`; `src_addr` is composed from
/// `src_cell_data + bank[+0x700][cl] + sign_extend(packed_MV >> 2)`.
/// Both are 32-bit byte offsets into the strip pixel-buffer arena.
///
/// The two addresses are typically distinct (the §4.2 ping-pong
/// scheme reads from one bank and writes to the other); the only
/// case in which they coincide is the degenerate self-copy
/// `dst_slot == src_slot` + identity MV (`packed_mv = 0`), which
/// `spec/05 §8.2 item 8` notes as a valid but uncommon encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McCellAddressPair {
    /// The destination byte address the MC fetcher writes to
    /// (`edi` at inner-loop entry).
    pub dst_addr: usize,
    /// The source byte address the MC fetcher reads from
    /// (`esi` at inner-loop entry, post-MV displacement).
    pub src_addr: usize,
}

impl McCellAddressPair {
    /// Spec/05 §7.2 — run the complete §5.4 / §7.2 address chain.
    ///
    /// Inputs:
    ///
    /// * `dst_entry` — the destination cell-data DWORD
    ///   (`CellAddrEntry::dst`, loaded from the per-cell sub-array
    ///   at `idx_dst = 16 * cell_slot + dst_slot`).
    /// * `src_entry` — the source cell-data DWORD
    ///   (`CellAddrEntry::src`, loaded from the per-cell sub-array
    ///   at `idx_src = 16 * cell_slot + src_slot`).
    /// * `cell_pos_aux` — `bank[+0x700][cl]`, the per-cell-state
    ///   intra-cell offset.
    /// * `packed_mv` — the packed-MV DWORD already fetched from
    ///   `inner_instance[4 * mv_index]` (or `0` for VQ_DATA cells
    ///   that have no MV displacement).
    ///
    /// Returns the (dst, src) byte-address pair on success.
    /// Returns [`McAddressError::RoleMismatch`] if `dst_entry` is
    /// not a destination entry or `src_entry` is not a source
    /// entry.
    pub fn resolve(
        dst_entry: CellAddrEntry,
        src_entry: CellAddrEntry,
        cell_pos_aux: usize,
        packed_mv: PackedMv,
    ) -> Result<Self, McAddressError> {
        if !matches!(dst_entry.role, CellAddrRole::Dest) {
            return Err(McAddressError::RoleMismatch);
        }
        if !matches!(src_entry.role, CellAddrRole::Src) {
            return Err(McAddressError::RoleMismatch);
        }
        let dst_addr =
            mc_dest_address(dst_entry, cell_pos_aux).ok_or(McAddressError::DestAddressOverflow)?;
        let src_base = src_entry
            .cell_data_ptr
            .checked_add(cell_pos_aux)
            .ok_or(McAddressError::SrcAddressOverflow)?;
        let src_addr = super::mc_packed::apply_mv_source_offset(src_base, packed_mv.pixel_offset())
            .ok_or(McAddressError::SrcMvDisplacementInvalid)?;
        Ok(Self { dst_addr, src_addr })
    }

    /// True iff `dst_addr == src_addr` — the §8.2 item 8 identity-
    /// MV self-copy degenerate case.
    pub const fn is_self_copy(self) -> bool {
        self.dst_addr == self.src_addr
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::{DISPATCHABLE_SLOT_COUNT, pack_mv_components};

    // ---- §7.2 / §4.3 (slot-stride constants) -----------------------

    #[test]
    fn cell_slot_stride_matches_shl_4_immediate() {
        // §7.2 / §4.3: `shl eax, 0x4` at `IR32_32.DLL!0x10006615`.
        assert_eq!(CELL_SLOT_STRIDE, 16);
        assert_eq!(CELL_SLOT_STRIDE, 1 << 4);
    }

    #[test]
    fn cell_slot_index_max_matches_section_text() {
        // §7.2: "cell-slot index 0..15".
        assert_eq!(CELL_SLOT_INDEX_MAX, 15);
    }

    #[test]
    fn cell_slot_stride_exceeds_dispatchable_slot_count() {
        // §4.2 / `spec/02 §5.1`: only 6 strip-context slots are
        // dispatchable, so any `dst_slot` / `src_slot` fits in the
        // 16-byte cell-slot stride with room to spare. Compared
        // value-to-value to keep the assertion non-constant for
        // `clippy::assertions_on_constants`.
        let dispatchable = DISPATCHABLE_SLOT_COUNT;
        let stride = CELL_SLOT_STRIDE;
        assert!(dispatchable < stride);
        // The concrete present-day values:
        assert_eq!(dispatchable, 6);
        assert_eq!(stride, 16);
    }

    // ---- §7.2 (CellSlotBase shape) ---------------------------------

    #[test]
    fn cell_slot_base_zero_byte_is_zero_base() {
        let b = CellSlotBase::from_bank_byte(0);
        assert_eq!(b.base_index(), 0);
        assert!(b.is_within_meaningful_range());
    }

    #[test]
    fn cell_slot_base_maximum_meaningful_byte() {
        // `bank[+0x200][ch] = 15` is the upper bound of the §7.2
        // meaningful range; base index = `15 * 16 = 240`.
        let b = CellSlotBase::from_bank_byte(CELL_SLOT_INDEX_MAX);
        assert_eq!(b.base_index(), 240);
        assert!(b.is_within_meaningful_range());
    }

    #[test]
    fn cell_slot_base_one_past_meaningful_is_out_of_range() {
        // The first byte past the §7.2 range is `16`; the
        // arithmetic still produces a valid base index, but the
        // §7.2 prose flags this as outside the pre-populated
        // cell-stack region.
        let b = CellSlotBase::from_bank_byte(16);
        assert_eq!(b.base_index(), 16 * 16);
        assert!(!b.is_within_meaningful_range());
    }

    #[test]
    fn cell_slot_base_max_byte_arithmetic() {
        // The §7.2 arithmetic on a one-byte index never overflows
        // `u32`: `255 << 4 = 4080`, well within `u32`.
        let b = CellSlotBase::from_bank_byte(0xff);
        assert_eq!(b.base_index(), 0xff * 16);
        assert_eq!(b.base_index(), 0xff0);
        assert!(!b.is_within_meaningful_range());
    }

    // ---- §7.2 (CellSubarrayIndex composition) ----------------------

    #[test]
    fn subarray_index_dst_role() {
        // `cell_slot = 5`, `dst_slot = 3` → element index `5*16+3 = 83`.
        let cell_slot = CellSlotBase::from_bank_byte(5);
        let idx = CellSubarrayIndex::dst(cell_slot, 3);
        assert_eq!(idx.element_index(), 5 * 16 + 3);
        assert_eq!(idx.byte_offset(), (5 * 16 + 3) * 4);
        assert_eq!(idx.role(), CellAddrRole::Dest);
    }

    #[test]
    fn subarray_index_src_role() {
        // `cell_slot = 5`, `src_slot = 0` → element index `5*16+0 = 80`.
        let cell_slot = CellSlotBase::from_bank_byte(5);
        let idx = CellSubarrayIndex::src(cell_slot, 0);
        assert_eq!(idx.element_index(), 5 * 16);
        assert_eq!(idx.byte_offset(), 5 * 16 * 4);
        assert_eq!(idx.role(), CellAddrRole::Src);
    }

    #[test]
    fn subarray_index_dst_minus_src_is_slot_delta() {
        // The §4.2 ping-pong scheme makes `dst_slot - src_slot`
        // the per-frame inversion delta. The element-index delta
        // is identical because both indices share the same
        // `16 * cell_slot` base.
        let cell_slot = CellSlotBase::from_bank_byte(7);
        let dst_idx = CellSubarrayIndex::dst(cell_slot, 4);
        let src_idx = CellSubarrayIndex::src(cell_slot, 1);
        assert_eq!(
            dst_idx.element_index() - src_idx.element_index(),
            4 - 1,
            "dst_slot - src_slot should equal the element-index delta"
        );
    }

    #[test]
    fn subarray_index_byte_offset_is_element_times_4() {
        // §7.2: the per-cell sub-array entries are 4-byte DWORDs;
        // the post-shift `mov edx, [esi + 4 * eax]` load uses
        // `eax * 4` as the byte offset.
        let cell_slot = CellSlotBase::from_bank_byte(2);
        let idx = CellSubarrayIndex::dst(cell_slot, 5);
        assert_eq!(idx.byte_offset(), idx.element_index() * 4);
    }

    // ---- §5.4 / §7.2 (CellAddrEntry shape) -------------------------

    #[test]
    fn cell_addr_entry_dst_has_no_extra_offset() {
        // §7.2: the `[esp+0x38]` extra-offset load (the +1
        // element-index sibling) is source-role only.
        let e = CellAddrEntry::dst(0x0000_1000);
        assert_eq!(e.cell_data_ptr(), 0x0000_1000);
        assert_eq!(e.extra_offset(), None);
        assert_eq!(e.role(), CellAddrRole::Dest);
    }

    #[test]
    fn cell_addr_entry_src_carries_extra_offset() {
        let e = CellAddrEntry::src(0x0000_2000, 0x0000_0040);
        assert_eq!(e.cell_data_ptr(), 0x0000_2000);
        assert_eq!(e.extra_offset(), Some(0x0000_0040));
        assert_eq!(e.role(), CellAddrRole::Src);
    }

    // ---- §5.4 / §7.2 (address composition) -------------------------

    #[test]
    fn mc_dest_address_basic() {
        // `dst_cell_data = 0x1000`, `cell_pos_aux = 0x40`
        // → `dst_addr = 0x1040`.
        let dst = CellAddrEntry::dst(0x0000_1000);
        assert_eq!(mc_dest_address(dst, 0x40), Some(0x0000_1040));
    }

    #[test]
    fn mc_dest_address_overflow_returns_none() {
        // `dst_cell_data` near `usize::MAX` + `cell_pos_aux` > 0
        // wraps; safe-Rust returns `None`.
        let dst = CellAddrEntry::dst(usize::MAX - 0x10);
        assert_eq!(mc_dest_address(dst, 0x20), None);
    }

    #[test]
    fn mc_source_address_identity_mv() {
        // Identity MV: `packed_mv = 0` → no displacement.
        // `src_addr = src_cell_data + cell_pos_aux + 0`.
        let src = CellAddrEntry::src(0x0000_2000, 0x0000_0040);
        let mv = PackedMv::from_raw(0);
        assert_eq!(mc_source_address(src, 0x80, mv), Some(0x0000_2080));
    }

    #[test]
    fn mc_source_address_positive_displacement() {
        // Build a packed MV with vert = 1, horiz = 0, full-pel.
        // `pixel_offset = 1 * 0xb0 = 0xb0`.
        let packed = pack_mv_components(1, 0, 0, 0);
        let mv = PackedMv::from_raw(packed);
        let src = CellAddrEntry::src(0x0000_2000, 0x0000_0010);
        // src_base = 0x2000 + 0x100 = 0x2100; +0xb0 = 0x21b0.
        assert_eq!(mc_source_address(src, 0x100, mv), Some(0x0000_21b0));
    }

    #[test]
    fn mc_source_address_negative_displacement() {
        // Build a packed MV with vert = -1, horiz = 0, full-pel.
        // `pixel_offset = -0xb0`.
        let packed = pack_mv_components(-1, 0, 0, 0);
        let mv = PackedMv::from_raw(packed);
        let src = CellAddrEntry::src(0x0000_3000, 0);
        // src_base = 0x3000 + 0x100 = 0x3100; -0xb0 = 0x3050.
        assert_eq!(mc_source_address(src, 0x100, mv), Some(0x0000_3050));
    }

    #[test]
    fn mc_source_address_overflow_in_base_returns_none() {
        let src = CellAddrEntry::src(usize::MAX - 0x10, 0);
        let mv = PackedMv::from_raw(0);
        assert_eq!(mc_source_address(src, 0x20, mv), None);
    }

    #[test]
    fn mc_source_address_signed_underflow_returns_none() {
        // src_base = 0x10, negative MV that underflows.
        let packed = pack_mv_components(-100, 0, 0, 0); // -100 * 0xb0 = -17600
        let mv = PackedMv::from_raw(packed);
        let src = CellAddrEntry::src(0x100, 0);
        // src_base = 0x100; pixel_offset = -17600 = -0x44c0;
        // 0x100 + (-0x44c0) underflows.
        assert_eq!(mc_source_address(src, 0, mv), None);
    }

    // ---- §7.2 (McCellAddressPair::resolve) -------------------------

    #[test]
    fn resolve_pair_full_chain_identity_mv() {
        // dst_slot = 3 (primary bank), src_slot = 0 (secondary),
        // cell_slot = 2.
        let cell_slot = CellSlotBase::from_bank_byte(2);
        let dst_idx = CellSubarrayIndex::dst(cell_slot, 3);
        let src_idx = CellSubarrayIndex::src(cell_slot, 0);
        assert_eq!(dst_idx.element_index(), 2 * 16 + 3);
        assert_eq!(src_idx.element_index(), 2 * 16);

        let dst_entry = CellAddrEntry::dst(0x0001_0000);
        let src_entry = CellAddrEntry::src(0x0002_0000, 0);
        let mv = PackedMv::from_raw(0);
        let pair = McCellAddressPair::resolve(dst_entry, src_entry, 0x40, mv).unwrap();
        assert_eq!(pair.dst_addr, 0x0001_0040);
        assert_eq!(pair.src_addr, 0x0002_0040);
        assert!(!pair.is_self_copy());
    }

    #[test]
    fn resolve_pair_with_positive_mv_displacement() {
        let dst_entry = CellAddrEntry::dst(0x0001_0000);
        let src_entry = CellAddrEntry::src(0x0002_0000, 0);
        // Vertical = 2 rows down, horiz = 4, full-pel:
        // pixel_offset = 2*0xb0 + 4 = 0x164.
        let packed = pack_mv_components(2, 4, 0, 0);
        let mv = PackedMv::from_raw(packed);
        let pair = McCellAddressPair::resolve(dst_entry, src_entry, 0x100, mv).unwrap();
        assert_eq!(pair.dst_addr, 0x0001_0100);
        assert_eq!(pair.src_addr, 0x0002_0100 + 0x164);
    }

    #[test]
    fn resolve_pair_self_copy_when_slots_and_mv_identity() {
        // §8.2 item 8: dst_slot == src_slot + packed_mv == 0
        // → dst_addr == src_addr.
        let dst_entry = CellAddrEntry::dst(0x0003_0000);
        let src_entry = CellAddrEntry::src(0x0003_0000, 0);
        let mv = PackedMv::from_raw(0);
        let pair = McCellAddressPair::resolve(dst_entry, src_entry, 0x20, mv).unwrap();
        assert_eq!(pair.dst_addr, pair.src_addr);
        assert!(pair.is_self_copy());
    }

    #[test]
    fn resolve_pair_rejects_swapped_roles_dst_with_src_first() {
        // Passing a src-role entry as `dst_entry` is a type-level
        // mismatch caught by [`McAddressError::RoleMismatch`].
        let bad_dst = CellAddrEntry::src(0, 0);
        let ok_src = CellAddrEntry::src(0, 0);
        let mv = PackedMv::from_raw(0);
        assert_eq!(
            McCellAddressPair::resolve(bad_dst, ok_src, 0, mv),
            Err(McAddressError::RoleMismatch)
        );
    }

    #[test]
    fn resolve_pair_rejects_swapped_roles_src_with_dst_second() {
        let ok_dst = CellAddrEntry::dst(0);
        let bad_src = CellAddrEntry::dst(0);
        let mv = PackedMv::from_raw(0);
        assert_eq!(
            McCellAddressPair::resolve(ok_dst, bad_src, 0, mv),
            Err(McAddressError::RoleMismatch)
        );
    }

    #[test]
    fn resolve_pair_propagates_dest_overflow() {
        let dst_entry = CellAddrEntry::dst(usize::MAX - 0x10);
        let src_entry = CellAddrEntry::src(0, 0);
        let mv = PackedMv::from_raw(0);
        assert_eq!(
            McCellAddressPair::resolve(dst_entry, src_entry, 0x20, mv),
            Err(McAddressError::DestAddressOverflow)
        );
    }

    #[test]
    fn resolve_pair_propagates_src_overflow() {
        let dst_entry = CellAddrEntry::dst(0);
        let src_entry = CellAddrEntry::src(usize::MAX - 0x10, 0);
        let mv = PackedMv::from_raw(0);
        assert_eq!(
            McCellAddressPair::resolve(dst_entry, src_entry, 0x20, mv),
            Err(McAddressError::SrcAddressOverflow)
        );
    }

    #[test]
    fn resolve_pair_propagates_mv_underflow() {
        let dst_entry = CellAddrEntry::dst(0x0001_0000);
        let src_entry = CellAddrEntry::src(0x100, 0);
        // Underflow: pixel_offset = -100 * 0xb0 = -17600 = -0x44c0.
        let packed = pack_mv_components(-100, 0, 0, 0);
        let mv = PackedMv::from_raw(packed);
        assert_eq!(
            McCellAddressPair::resolve(dst_entry, src_entry, 0, mv),
            Err(McAddressError::SrcMvDisplacementInvalid)
        );
    }

    // ---- Cross-module consistency ----------------------------------

    #[test]
    fn cell_slot_stride_consistent_with_cell_subarray_module() {
        // §5.1 / §7.2: the per-cell sub-array stride is 4 bytes per
        // entry (per [`super::cell_subarray::CELL_STACK_ENTRY_SIZE`]).
        // The `idx * 4` byte-offset arithmetic in
        // [`CellSubarrayIndex::byte_offset`] mirrors that constant.
        use crate::indeo3::CELL_STACK_ENTRY_SIZE;
        let cell_slot = CellSlotBase::from_bank_byte(1);
        let idx = CellSubarrayIndex::dst(cell_slot, 0);
        assert_eq!(
            idx.byte_offset() as usize,
            (idx.element_index() as usize) * CELL_STACK_ENTRY_SIZE
        );
    }
}
