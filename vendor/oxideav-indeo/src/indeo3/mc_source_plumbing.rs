//! Indeo 3 spec/05 §4.3 — source-pointer plumbing
//! (the per-plane decoder → cell-state dispatcher stack-frame
//! hand-off that carries the §4.2 ping-pong source / destination
//! slot indices into the §7.2 cell-data DWORD load).
//!
//! Spec source: `docs/video/indeo/indeo3/spec/05-motion-compensation.md`
//! §4.3 — the four-instruction dispatcher fragment at
//! `IR32_32.DLL!0x10006638..0x10006641`:
//!
//! ```text
//! sub eax, edi              ; eax = 16 * cell_slot
//! add eax, [esp + 0x54]      ; + source-slot index
//! mov edx, [esi + 4 * eax]    ; load source cell-data DWORD
//! mov [esp + 0x24], edx       ; save for MC use
//! ```
//!
//! and cross-references: `spec/02 §6` table rows 2-3 (the per-plane
//! decoder's `[esp+0x54]` source-slot / `[esp+0x58]` destination-slot
//! arguments), `spec/05 §4.2` (the ping-pong inversion that fills
//! those two slot-index arguments), `spec/05 §7.2` (the cell-data
//! DWORD load chain and the `[esp+0x24]` / `[esp+0x28]` / `[esp+0x38]`
//! dispatcher scratch slots).
//!
//! Round 16 ([`super::bank_select`]) resolved the §4.2 pair
//! `(dst_slot, src_slot)` from `frame_flags` bit 9 and the
//! plane index. Round 15 ([`super::mc_address`]) resolved the
//! cell-position decoding entry — the §7.2 index arithmetic and the
//! `CellAddrEntry` typed cell-data DWORD load. Round 17
//! ([`super::mc_arena`]) pinned the §4.1 arena geometry the six
//! per-slot base pointers point into. This module owns the §4.3 link
//! between those three: the stack-frame slot offsets the per-plane
//! decoder uses to push its arguments into the dispatcher's frame
//! and the matching dispatcher-scratch slots that hold the resolved
//! cell-data DWORDs.
//!
//! This module surfaces:
//!
//! * [`DECODER_ARG_SRC_SLOT_OFFSET`] (`0x54`) — the byte offset of
//!   the source-slot-index argument within the per-plane decoder's
//!   stack frame (`[esp+0x54]` per `spec/02 §6` table row 2). The
//!   `frame_flags` bit 9 inversion at
//!   `IR32_32.DLL!0x100045e9..0x100045fd` writes this slot.
//! * [`DECODER_ARG_DST_SLOT_OFFSET`] (`0x58`) — the byte offset of
//!   the destination-slot-index argument within the per-plane
//!   decoder's stack frame (`[esp+0x58]` per `spec/02 §6` table row
//!   3). The `frame_flags` bit 9 inversion at
//!   `IR32_32.DLL!0x100045c3..0x100045d4` writes this slot.
//! * [`DISPATCHER_SCRATCH_SRC_DATA_OFFSET`] (`0x24`) — the byte
//!   offset of the source cell-data DWORD scratch slot within the
//!   cell-state dispatcher's frame (`[esp+0x24]` per §4.3 line 4 /
//!   §7.2). The four-instruction §4.3 fragment writes
//!   `strip_ctx_arr[16 * cell_slot + src_slot]` to this slot.
//! * [`DISPATCHER_SCRATCH_DST_DATA_OFFSET`] (`0x28`) — the byte
//!   offset of the destination cell-data DWORD scratch slot
//!   (`[esp+0x28]` per §7.2). The analogous fragment writes
//!   `strip_ctx_arr[16 * cell_slot + dst_slot]` to this slot.
//! * [`DISPATCHER_SCRATCH_EXTRA_OFFSET_OFFSET`] (`0x38`) — the byte
//!   offset of the §7.2 companion `extra_offset` scratch slot
//!   (`[esp+0x38]`). Loaded only for the source role (the `idx_src + 1`
//!   element-index step at `0x1000663e + 4`).
//! * [`STRIP_CTX_ARRAY_ELEMENT_SHIFT`] (`2`) — the `mov edx, [esi +
//!   4 * eax]` left-shift that turns the `eax = 16 * cell_slot +
//!   slot_arg` element index into a byte offset within the
//!   strip-context array.
//! * [`DecoderStackArg`] — typed pick of one of the two per-plane
//!   decoder arguments (`SrcSlot` / `DstSlot`), with
//!   [`DecoderStackArg::byte_offset`] surfacing the chosen offset
//!   within the per-plane decoder's stack frame and
//!   [`DecoderStackArg::role`] tying it to a
//!   [`super::mc_address::CellAddrRole`].
//! * [`DispatcherScratch`] — typed pick of one of the three
//!   cell-state dispatcher scratch slots (`SrcCellData` /
//!   `DstCellData` / `ExtraOffset`), with
//!   [`DispatcherScratch::byte_offset`] / [`DispatcherScratch::role`]
//!   / [`DispatcherScratch::is_source_companion`].
//! * [`SourcePlumbingPair`] — the §4.3 invariant view: the typed
//!   `(decoder_arg, dispatcher_scratch)` pair the dispatcher's
//!   four-instruction fragment turns one into the other. The §4.3
//!   prose's "save for MC use" identity is encoded as
//!   [`SourcePlumbingPair::for_role`].
//! * [`is_self_copy_degenerate`] — the §4.3 closing predicate:
//!   `dst_slot == src_slot` ⇒ self-copy. Returns `true` for any
//!   pair that would degenerate; `false` otherwise. The §4.3 prose
//!   notes "no such frame is observed in the binary"; surfaced
//!   here as a typed test so callers can flag a corpus-anomalous
//!   frame at safe-Rust boundaries.
//!
//! What this module **deliberately does not do** (the §4 chapter
//! boundary):
//!
//! * It does not perform the cell-data DWORD load itself. The
//!   `mov edx, [esi + 4 * eax]` site at `0x10006641` is owned by
//!   [`super::mc_address::CellSubarrayIndex`] /
//!   [`super::mc_address::CellAddrEntry`]; this module surfaces
//!   only the stack-frame slot offsets the load reads from and
//!   writes to.
//! * It does not resolve `(dst_slot, src_slot)` itself. That's
//!   [`super::bank_select::McBankAssignment::resolve`]'s job; this
//!   module accepts an already-resolved pair.
//! * It does not perform the §2.3 source-pointer arithmetic
//!   `add esi, sign_extend(packed >> 2)`. The §4.3 source-pointer
//!   chain (§4.3 second paragraph) ends at the cell-data DWORD
//!   load; the per-cell MV displacement is owned by
//!   [`super::apply_mv_source_offset`].
//! * It does not enforce per-strip bounds. Per §4.4 the binary
//!   itself performs no boundary check on the source-pointer
//!   arithmetic; safe-Rust callers that want bounds-checking apply
//!   it at the slice boundary, not here.
//!
//! All offsets, RVAs, and arithmetic identities are taken from
//! `05-motion-compensation.md` §4.3 / §7.2 and `02-picture-layer.md`
//! §6. RVAs cited in doc-comments refer to the binary identified in
//! `spec/00 §2`.

use super::mc_address::CellAddrRole;

// ---- §4.3 / spec/02 §6 (decoder stack-frame arg offsets) -----------

/// Spec/05 §4.3 / `spec/02 §6` table row 2 — byte offset of the
/// **source-slot-index** argument within the per-plane decoder's
/// stack frame (`[esp+0x54]`).
///
/// The `frame_flags` bit 9 inversion at
/// `IR32_32.DLL!0x100045e9..0x100045fd` writes this slot before the
/// per-plane decoder calls into the cell-state dispatcher. The
/// dispatcher reads it back at `0x1000663b`
/// (`add eax, [esp + 0x54]`).
pub const DECODER_ARG_SRC_SLOT_OFFSET: usize = 0x54;

/// Spec/05 §4.3 / `spec/02 §6` table row 3 — byte offset of the
/// **destination-slot-index** argument within the per-plane decoder's
/// stack frame (`[esp+0x58]`).
///
/// The `frame_flags` bit 9 inversion at
/// `IR32_32.DLL!0x100045c3..0x100045d4` writes this slot. The
/// dispatcher reads it back at `0x10006637`'s analogue
/// (`add eax, [esp + 0x58]`) for the destination chain.
pub const DECODER_ARG_DST_SLOT_OFFSET: usize = 0x58;

/// `const _` cross-check: the two argument offsets are 4 bytes
/// apart (one DWORD). The per-plane decoder pushes them as adjacent
/// arguments in the standard cdecl 4-byte slot stride.
const _: () = assert!(DECODER_ARG_DST_SLOT_OFFSET == DECODER_ARG_SRC_SLOT_OFFSET + 4);

// ---- §4.3 / §7.2 (dispatcher scratch-slot offsets) -----------------

/// Spec/05 §4.3 line 4 / §7.2 — byte offset of the **source
/// cell-data DWORD** scratch slot within the cell-state dispatcher's
/// stack frame (`[esp+0x24]`).
///
/// The §4.3 fragment writes
/// `strip_ctx_arr[16 * cell_slot + src_slot]` to this slot via
/// `mov [esp + 0x24], edx` after the indexed load at `0x10006641`.
pub const DISPATCHER_SCRATCH_SRC_DATA_OFFSET: usize = 0x24;

/// Spec/05 §7.2 — byte offset of the **destination cell-data DWORD**
/// scratch slot within the cell-state dispatcher's stack frame
/// (`[esp+0x28]`).
///
/// The dispatcher writes
/// `strip_ctx_arr[16 * cell_slot + dst_slot]` to this slot via the
/// analogous fragment for the destination chain. Consumed at the
/// MC fetcher's inner-loop entry by `add esi, [esp + 0x28]` at
/// `0x100066dc` (or the dst-pointer composition's equivalent).
pub const DISPATCHER_SCRATCH_DST_DATA_OFFSET: usize = 0x28;

/// Spec/05 §7.2 — byte offset of the **`extra_offset`** scratch
/// slot within the cell-state dispatcher's stack frame
/// (`[esp+0x38]`).
///
/// Loaded only for the source role (the `idx_src + 1` element-index
/// step at `0x1000663e + 4`). Used by the §5.5 boundary fix-up; per
/// §7.2 it does not participate in the §4.3 source-pointer chain
/// itself.
pub const DISPATCHER_SCRATCH_EXTRA_OFFSET_OFFSET: usize = 0x38;

/// `const _` cross-check: the source and destination cell-data
/// scratch slots are 4 bytes apart (one DWORD).
const _: () = assert!(DISPATCHER_SCRATCH_DST_DATA_OFFSET == DISPATCHER_SCRATCH_SRC_DATA_OFFSET + 4);

/// Spec/05 §4.3 line 3 / §7.2 — the `mov edx, [esi + 4 * eax]`
/// left-shift that turns the `eax = 16 * cell_slot + slot_arg`
/// element index into a byte offset within the strip-context array.
///
/// Equal to `log2(sizeof(u32)) = 2`. Mirrors
/// [`super::mc_address::CellSubarrayIndex::byte_offset`]'s `* 4`.
pub const STRIP_CTX_ARRAY_ELEMENT_SHIFT: u32 = 2;

/// `const _` cross-check: the element shift matches `log2(4)`.
const _: () = assert!(STRIP_CTX_ARRAY_ELEMENT_SHIFT == 2);
const _: () = assert!(1usize << STRIP_CTX_ARRAY_ELEMENT_SHIFT == 4);

// ---- typed surface ------------------------------------------------

/// Spec/05 §4.3 / `spec/02 §6` — typed pick of one of the two
/// per-plane decoder arguments the cell-state dispatcher reads
/// (`[esp+0x54]` source-slot or `[esp+0x58]` destination-slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderStackArg {
    /// The source-slot-index argument at [`DECODER_ARG_SRC_SLOT_OFFSET`]
    /// (`[esp+0x54]`). Read by the dispatcher's source chain at
    /// `0x1000663b`.
    SrcSlot,
    /// The destination-slot-index argument at
    /// [`DECODER_ARG_DST_SLOT_OFFSET`] (`[esp+0x58]`). Read by the
    /// dispatcher's destination chain.
    DstSlot,
}

impl DecoderStackArg {
    /// The byte offset of this argument within the per-plane
    /// decoder's stack frame.
    pub const fn byte_offset(self) -> usize {
        match self {
            DecoderStackArg::SrcSlot => DECODER_ARG_SRC_SLOT_OFFSET,
            DecoderStackArg::DstSlot => DECODER_ARG_DST_SLOT_OFFSET,
        }
    }

    /// The [`CellAddrRole`] this argument feeds into. The
    /// dispatcher's four-instruction §4.3 fragment turns a
    /// `SrcSlot` argument into a `Src` cell-data scratch entry; a
    /// `DstSlot` argument turns into a `Dest` cell-data scratch
    /// entry.
    pub const fn role(self) -> CellAddrRole {
        match self {
            DecoderStackArg::SrcSlot => CellAddrRole::Src,
            DecoderStackArg::DstSlot => CellAddrRole::Dest,
        }
    }

    /// The companion [`DispatcherScratch`] cell-data slot this
    /// argument's dispatcher fragment writes to.
    pub const fn dispatcher_scratch(self) -> DispatcherScratch {
        match self {
            DecoderStackArg::SrcSlot => DispatcherScratch::SrcCellData,
            DecoderStackArg::DstSlot => DispatcherScratch::DstCellData,
        }
    }
}

/// Spec/05 §4.3 line 4 / §7.2 — typed pick of one of the three
/// cell-state dispatcher scratch slots
/// (`[esp+0x24]` source cell-data, `[esp+0x28]` destination
/// cell-data, or `[esp+0x38]` extra-offset companion).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatcherScratch {
    /// The source cell-data DWORD scratch slot at
    /// [`DISPATCHER_SCRATCH_SRC_DATA_OFFSET`] (`[esp+0x24]`).
    /// Holds `strip_ctx_arr[16 * cell_slot + src_slot]` after the
    /// §4.3 fragment runs.
    SrcCellData,
    /// The destination cell-data DWORD scratch slot at
    /// [`DISPATCHER_SCRATCH_DST_DATA_OFFSET`] (`[esp+0x28]`).
    /// Holds `strip_ctx_arr[16 * cell_slot + dst_slot]`.
    DstCellData,
    /// The §7.2 `extra_offset` companion scratch slot at
    /// [`DISPATCHER_SCRATCH_EXTRA_OFFSET_OFFSET`] (`[esp+0x38]`).
    /// Loaded only for the source role's `idx_src + 1` element
    /// step; consumed by the §5.5 boundary fix-up.
    ExtraOffset,
}

impl DispatcherScratch {
    /// The byte offset of this scratch slot within the cell-state
    /// dispatcher's stack frame.
    pub const fn byte_offset(self) -> usize {
        match self {
            DispatcherScratch::SrcCellData => DISPATCHER_SCRATCH_SRC_DATA_OFFSET,
            DispatcherScratch::DstCellData => DISPATCHER_SCRATCH_DST_DATA_OFFSET,
            DispatcherScratch::ExtraOffset => DISPATCHER_SCRATCH_EXTRA_OFFSET_OFFSET,
        }
    }

    /// The [`CellAddrRole`] this scratch slot carries. Both the
    /// source cell-data slot (`[esp+0x24]`) and the extra-offset
    /// companion (`[esp+0x38]`) carry the `Src` role; the
    /// destination cell-data slot (`[esp+0x28]`) carries `Dest`.
    pub const fn role(self) -> CellAddrRole {
        match self {
            DispatcherScratch::SrcCellData => CellAddrRole::Src,
            DispatcherScratch::DstCellData => CellAddrRole::Dest,
            DispatcherScratch::ExtraOffset => CellAddrRole::Src,
        }
    }

    /// `true` for the source-side companion scratch slot
    /// (`[esp+0x38]`) — the §7.2 `idx_src + 1` element-index step
    /// that distinguishes the source chain from the destination
    /// chain. `false` for the other two slots.
    pub const fn is_source_companion(self) -> bool {
        matches!(self, DispatcherScratch::ExtraOffset)
    }
}

/// Spec/05 §4.3 — the typed `(decoder_arg, dispatcher_scratch)` pair
/// the dispatcher's four-instruction fragment turns one into the
/// other.
///
/// The §4.3 fragment encodes a single relation: the per-plane
/// decoder pushes a slot index at `DECODER_ARG_*_SLOT_OFFSET`, the
/// dispatcher loads
/// `strip_ctx_arr[16 * cell_slot + slot_arg]` into the matching
/// `DISPATCHER_SCRATCH_*_DATA_OFFSET`, and the MC fetcher consumes
/// the cell-data DWORD from there. This struct names that relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePlumbingPair {
    decoder_arg: DecoderStackArg,
    dispatcher_scratch: DispatcherScratch,
}

impl SourcePlumbingPair {
    /// Build the §4.3 pair for the given role. The source role
    /// returns `(SrcSlot, SrcCellData)`; the destination role
    /// returns `(DstSlot, DstCellData)`. The §7.2 companion
    /// [`DispatcherScratch::ExtraOffset`] is not part of this pair
    /// — it's the source role's second load, not the §4.3
    /// fragment's primary write.
    pub const fn for_role(role: CellAddrRole) -> Self {
        match role {
            CellAddrRole::Src => Self {
                decoder_arg: DecoderStackArg::SrcSlot,
                dispatcher_scratch: DispatcherScratch::SrcCellData,
            },
            CellAddrRole::Dest => Self {
                decoder_arg: DecoderStackArg::DstSlot,
                dispatcher_scratch: DispatcherScratch::DstCellData,
            },
        }
    }

    /// The per-plane decoder's argument the dispatcher reads.
    pub const fn decoder_arg(self) -> DecoderStackArg {
        self.decoder_arg
    }

    /// The dispatcher's scratch slot the cell-data DWORD is
    /// written to.
    pub const fn dispatcher_scratch(self) -> DispatcherScratch {
        self.dispatcher_scratch
    }

    /// The role both sides of the pair carry.
    pub const fn role(self) -> CellAddrRole {
        self.decoder_arg.role()
    }
}

/// Spec/05 §4.3 closing predicate — `true` if the two slot indices
/// would degenerate the MC copy into a same-bank self-copy.
///
/// Per §4.3 paragraph 2: "if a frame chose to read and write within
/// the same bank, the two arguments would be identical, and the MC
/// copy would degenerate into a self-copy. No such frame is observed
/// in the binary." This predicate surfaces the test as a typed
/// helper for callers that want to flag a corpus-anomalous frame at
/// safe-Rust boundaries.
///
/// Returns `false` for any pair produced by a well-formed §4.2
/// bank inversion (where the two slots differ by
/// [`super::BANK_INVERSION_DELTA`] = 3).
pub const fn is_self_copy_degenerate(dst_slot: u32, src_slot: u32) -> bool {
    dst_slot == src_slot
}

#[cfg(test)]
mod tests {
    use super::super::bank_select::{BANK_INVERSION_DELTA, McBankAssignment};
    use super::super::header::FrameFlags;
    use super::super::mc_address::CellAddrRole;
    use super::super::picture_layer::{PLANE_COUNT, PLANE_IDX_U, PLANE_IDX_V, PLANE_IDX_Y};
    use super::{
        DECODER_ARG_DST_SLOT_OFFSET, DECODER_ARG_SRC_SLOT_OFFSET,
        DISPATCHER_SCRATCH_DST_DATA_OFFSET, DISPATCHER_SCRATCH_EXTRA_OFFSET_OFFSET,
        DISPATCHER_SCRATCH_SRC_DATA_OFFSET, DecoderStackArg, DispatcherScratch,
        STRIP_CTX_ARRAY_ELEMENT_SHIFT, SourcePlumbingPair, is_self_copy_degenerate,
    };

    // ---- decoder-frame arg offsets ----

    #[test]
    fn decoder_arg_src_slot_offset_matches_spec() {
        // Spec/05 §4.3 line 2 — `add eax, [esp + 0x54]`.
        assert_eq!(DECODER_ARG_SRC_SLOT_OFFSET, 0x54);
    }

    #[test]
    fn decoder_arg_dst_slot_offset_matches_spec() {
        // Spec/02 §6 table row 3 / §4.3 paragraph 2 — `[esp+0x58]`.
        assert_eq!(DECODER_ARG_DST_SLOT_OFFSET, 0x58);
    }

    #[test]
    fn decoder_arg_offsets_are_one_dword_apart() {
        // Two adjacent cdecl args ⇒ 4-byte stride.
        assert_eq!(DECODER_ARG_DST_SLOT_OFFSET - DECODER_ARG_SRC_SLOT_OFFSET, 4);
    }

    // ---- dispatcher scratch-slot offsets ----

    #[test]
    fn dispatcher_scratch_src_data_offset_matches_spec() {
        // Spec/05 §4.3 line 4 — `mov [esp + 0x24], edx`.
        assert_eq!(DISPATCHER_SCRATCH_SRC_DATA_OFFSET, 0x24);
    }

    #[test]
    fn dispatcher_scratch_dst_data_offset_matches_spec() {
        // Spec/05 §7.2 — `[esp+0x28]` destination cell-data DWORD.
        assert_eq!(DISPATCHER_SCRATCH_DST_DATA_OFFSET, 0x28);
    }

    #[test]
    fn dispatcher_scratch_extra_offset_offset_matches_spec() {
        // Spec/05 §7.2 — `[esp+0x38]` source-only companion.
        assert_eq!(DISPATCHER_SCRATCH_EXTRA_OFFSET_OFFSET, 0x38);
    }

    #[test]
    fn dispatcher_scratch_cell_data_slots_are_one_dword_apart() {
        // Spec/05 §4.3 / §7.2 — `[esp+0x24]` and `[esp+0x28]` are
        // adjacent DWORD slots, mirroring the §7.2 element-index
        // arithmetic.
        assert_eq!(
            DISPATCHER_SCRATCH_DST_DATA_OFFSET - DISPATCHER_SCRATCH_SRC_DATA_OFFSET,
            4
        );
    }

    #[test]
    fn dispatcher_scratch_offsets_partition_three_distinct_slots() {
        // The three scratch slots are pairwise distinct.
        let a = DISPATCHER_SCRATCH_SRC_DATA_OFFSET;
        let b = DISPATCHER_SCRATCH_DST_DATA_OFFSET;
        let c = DISPATCHER_SCRATCH_EXTRA_OFFSET_OFFSET;
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }

    // ---- element-index shift ----

    #[test]
    fn strip_ctx_array_element_shift_matches_dword_size() {
        // Spec/05 §4.3 line 3 — `mov edx, [esi + 4 * eax]` ⇒ shift 2.
        assert_eq!(STRIP_CTX_ARRAY_ELEMENT_SHIFT, 2);
        assert_eq!(1u32 << STRIP_CTX_ARRAY_ELEMENT_SHIFT, 4);
    }

    // ---- DecoderStackArg ----

    #[test]
    fn decoder_stack_arg_src_byte_offset() {
        assert_eq!(
            DecoderStackArg::SrcSlot.byte_offset(),
            DECODER_ARG_SRC_SLOT_OFFSET
        );
    }

    #[test]
    fn decoder_stack_arg_dst_byte_offset() {
        assert_eq!(
            DecoderStackArg::DstSlot.byte_offset(),
            DECODER_ARG_DST_SLOT_OFFSET
        );
    }

    #[test]
    fn decoder_stack_arg_src_role_is_src() {
        assert_eq!(DecoderStackArg::SrcSlot.role(), CellAddrRole::Src);
    }

    #[test]
    fn decoder_stack_arg_dst_role_is_dest() {
        assert_eq!(DecoderStackArg::DstSlot.role(), CellAddrRole::Dest);
    }

    #[test]
    fn decoder_stack_arg_src_to_dispatcher_scratch() {
        assert_eq!(
            DecoderStackArg::SrcSlot.dispatcher_scratch(),
            DispatcherScratch::SrcCellData,
        );
    }

    #[test]
    fn decoder_stack_arg_dst_to_dispatcher_scratch() {
        assert_eq!(
            DecoderStackArg::DstSlot.dispatcher_scratch(),
            DispatcherScratch::DstCellData,
        );
    }

    // ---- DispatcherScratch ----

    #[test]
    fn dispatcher_scratch_src_cell_data_byte_offset() {
        assert_eq!(
            DispatcherScratch::SrcCellData.byte_offset(),
            DISPATCHER_SCRATCH_SRC_DATA_OFFSET
        );
    }

    #[test]
    fn dispatcher_scratch_dst_cell_data_byte_offset() {
        assert_eq!(
            DispatcherScratch::DstCellData.byte_offset(),
            DISPATCHER_SCRATCH_DST_DATA_OFFSET
        );
    }

    #[test]
    fn dispatcher_scratch_extra_offset_byte_offset() {
        assert_eq!(
            DispatcherScratch::ExtraOffset.byte_offset(),
            DISPATCHER_SCRATCH_EXTRA_OFFSET_OFFSET
        );
    }

    #[test]
    fn dispatcher_scratch_src_role_is_src() {
        assert_eq!(DispatcherScratch::SrcCellData.role(), CellAddrRole::Src);
    }

    #[test]
    fn dispatcher_scratch_dst_role_is_dest() {
        assert_eq!(DispatcherScratch::DstCellData.role(), CellAddrRole::Dest);
    }

    #[test]
    fn dispatcher_scratch_extra_role_is_src() {
        // §7.2: extra_offset is loaded from idx_src + 1; carries Src role.
        assert_eq!(DispatcherScratch::ExtraOffset.role(), CellAddrRole::Src);
    }

    #[test]
    fn dispatcher_scratch_is_source_companion_only_for_extra_offset() {
        assert!(!DispatcherScratch::SrcCellData.is_source_companion());
        assert!(!DispatcherScratch::DstCellData.is_source_companion());
        assert!(DispatcherScratch::ExtraOffset.is_source_companion());
    }

    // ---- SourcePlumbingPair ----

    #[test]
    fn source_plumbing_pair_for_src_role_pairs_0x54_with_0x24() {
        let p = SourcePlumbingPair::for_role(CellAddrRole::Src);
        assert_eq!(p.decoder_arg(), DecoderStackArg::SrcSlot);
        assert_eq!(p.dispatcher_scratch(), DispatcherScratch::SrcCellData);
        assert_eq!(p.role(), CellAddrRole::Src);
        assert_eq!(p.decoder_arg().byte_offset(), 0x54);
        assert_eq!(p.dispatcher_scratch().byte_offset(), 0x24);
    }

    #[test]
    fn source_plumbing_pair_for_dest_role_pairs_0x58_with_0x28() {
        let p = SourcePlumbingPair::for_role(CellAddrRole::Dest);
        assert_eq!(p.decoder_arg(), DecoderStackArg::DstSlot);
        assert_eq!(p.dispatcher_scratch(), DispatcherScratch::DstCellData);
        assert_eq!(p.role(), CellAddrRole::Dest);
        assert_eq!(p.decoder_arg().byte_offset(), 0x58);
        assert_eq!(p.dispatcher_scratch().byte_offset(), 0x28);
    }

    #[test]
    fn source_plumbing_pair_decoder_arg_and_scratch_share_role() {
        for role in [CellAddrRole::Src, CellAddrRole::Dest] {
            let p = SourcePlumbingPair::for_role(role);
            assert_eq!(p.decoder_arg().role(), role);
            assert_eq!(p.dispatcher_scratch().role(), role);
        }
    }

    #[test]
    fn source_plumbing_pair_src_and_dst_decoder_offsets_4_apart() {
        let src = SourcePlumbingPair::for_role(CellAddrRole::Src);
        let dst = SourcePlumbingPair::for_role(CellAddrRole::Dest);
        assert_eq!(
            dst.decoder_arg().byte_offset() - src.decoder_arg().byte_offset(),
            4
        );
    }

    #[test]
    fn source_plumbing_pair_src_and_dst_scratch_offsets_4_apart() {
        let src = SourcePlumbingPair::for_role(CellAddrRole::Src);
        let dst = SourcePlumbingPair::for_role(CellAddrRole::Dest);
        assert_eq!(
            dst.dispatcher_scratch().byte_offset() - src.dispatcher_scratch().byte_offset(),
            4
        );
    }

    // ---- is_self_copy_degenerate ----

    #[test]
    fn is_self_copy_degenerate_returns_true_for_equal_slots() {
        // The §4.3 closing predicate's degenerate case.
        for slot in 0..6u32 {
            assert!(is_self_copy_degenerate(slot, slot));
        }
    }

    #[test]
    fn is_self_copy_degenerate_returns_false_for_distinct_slots() {
        // Any pair with dst != src is non-degenerate.
        assert!(!is_self_copy_degenerate(0, 1));
        assert!(!is_self_copy_degenerate(3, 0));
        assert!(!is_self_copy_degenerate(5, 2));
    }

    #[test]
    fn is_self_copy_degenerate_false_for_all_bank_inversion_pairs() {
        // Spec/05 §4.2 + §4.3 — every pair the §4.2 bank inversion
        // produces has |dst - src| == BANK_INVERSION_DELTA = 3,
        // therefore none degenerate.
        for plane_idx in 0..PLANE_COUNT {
            for bit_9_set in [false, true] {
                let raw = if bit_9_set { 0x0200u16 } else { 0x0000u16 };
                let assignment = McBankAssignment::resolve(FrameFlags(raw), plane_idx).unwrap();
                let dst = assignment.dst_slot as u32;
                let src = assignment.src_slot as u32;
                assert!(!is_self_copy_degenerate(dst, src));
                assert_eq!(dst.abs_diff(src), BANK_INVERSION_DELTA as u32);
            }
        }
    }

    #[test]
    fn is_self_copy_degenerate_matches_assignment_predicate() {
        // McBankAssignment::is_self_copy() should agree with this
        // standalone predicate over every legal (bit-9, plane) pair.
        for plane_idx in [PLANE_IDX_Y, PLANE_IDX_V, PLANE_IDX_U] {
            for bit_9_set in [false, true] {
                let raw = if bit_9_set { 0x0200u16 } else { 0x0000u16 };
                let assignment = McBankAssignment::resolve(FrameFlags(raw), plane_idx).unwrap();
                let dst = assignment.dst_slot as u32;
                let src = assignment.src_slot as u32;
                assert_eq!(is_self_copy_degenerate(dst, src), assignment.is_self_copy());
            }
        }
    }

    // ---- cross-module agreement ----

    #[test]
    fn dispatcher_scratch_offsets_disjoint_from_decoder_arg_offsets() {
        // The dispatcher's scratch slots (low offsets) live in a
        // different frame region from the per-plane decoder's
        // arguments (higher offsets, beyond the dispatcher's
        // saved-ebp/local-vars region). This is a documentation
        // invariant; assert all three scratch offsets are strictly
        // lower than both arg offsets.
        for scratch in [
            DispatcherScratch::SrcCellData,
            DispatcherScratch::DstCellData,
            DispatcherScratch::ExtraOffset,
        ] {
            for arg in [DecoderStackArg::SrcSlot, DecoderStackArg::DstSlot] {
                assert!(scratch.byte_offset() < arg.byte_offset());
            }
        }
    }

    #[test]
    fn source_plumbing_pair_self_copy_predicate_consistency() {
        // For a well-formed §4.2 inversion the pair-for-role
        // composition trivially produces matching roles on both
        // sides; the self-copy predicate is about the slot indices
        // (not the roles), so confirm the predicate is decoupled
        // from SourcePlumbingPair::role(). For Y plane bit-9=clear:
        // dst=3, src=0 → not degenerate.
        let assignment = McBankAssignment::resolve(FrameFlags(0x0000), PLANE_IDX_Y).unwrap();
        assert!(!is_self_copy_degenerate(
            assignment.dst_slot as u32,
            assignment.src_slot as u32
        ));
        // Hypothetical self-copy (encoder anomaly).
        assert!(is_self_copy_degenerate(3, 3));
    }
}
