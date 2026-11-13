//! Indeo 3 strip-context array + per-plane decode-call signature.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/02-picture-layer.md`
//! §4.1, §4.2, §5, §6, §7.
//!
//! Round 2 (`picture_layer.rs`) decoded the per-plane prelude bytes
//! (`num_vectors` + `mc_vectors[]`) that immediately follow each
//! plane offset stored in the bitstream header. The picture-layer
//! chapter §5 + §6 + §7 sit *between* the prelude consumer and the
//! spec/03 binary-tree walker — they describe the **per-codec-frame
//! state** the binary-tree walker reads + writes:
//!
//! 1. §5 — the strip-context array. A 32-slot, `0x400`-stride scratch
//!    buffer at instance-state offset `+0x414` that holds per-strip
//!    geometry + the binary-tree walker's stack frames. Six of the
//!    32 slots are dispatchable from the per-plane decode call (one
//!    per `(plane, buffer)` pair) and the remaining 26 are scratch
//!    slots the walker pushes intermediate sub-cells onto.
//!
//! 2. §6 — the per-plane decode-call signature. The picture-layer
//!    iterator hands the binary-tree walker (`IR32_32.DLL!0x10006538`)
//!    a 7-argument cdecl frame: bitstream pointer, strip-context array
//!    pointer, slot indices (src + dst), instance-state base, secondary
//!    codebook pointer, and a *luma-vs-chroma-discriminated* codebook
//!    bank pointer (`+0x1a00` for luma planes, `+0x400` for chroma).
//!
//! 3. §7 — the per-plane state at codec-init time. The per-strip
//!    pixel buffers and the strip-context array are allocated once per
//!    `ICDecompressBegin` (not once per frame). The luma strip count
//!    is `ceil(width / 160)`, the chroma strip count is
//!    `ceil(width / 16)` (luma width / chroma subsampling ratio /
//!    chroma strip width × per-strip slot pattern), and the remainder
//!    strip (for non-multiple widths) carries width
//!    `((W-1) mod strip_width) + 1`.
//!
//! What this module **deliberately does not do** (the spec/02 §10
//! chapter boundary):
//!
//! * It does not allocate or own the strip-context array bytes. The
//!   strip-context array is allocated by the codec-init routine (which
//!   spec/02 §7 documents the steps of, without specifying the host
//!   allocator). This module exposes **layout descriptors** + **index
//!   arithmetic** the future codec-init code will consume.
//! * It does not perform the binary-tree walk that writes to the slot's
//!   sub-array (`+0x40+`). That walk is spec/03 (`macroblock.rs`).
//! * It does not perform motion compensation against the pixel buffers
//!   the slot's base pointers refer to. That is spec/05.
//! * The detailed field semantics of the per-slot sub-array beyond
//!   `+0x1c` are intertwined with the binary-tree walker's stack
//!   discipline and are documented as spec/03's subject (see spec/02
//!   §5.2 the table's deferral note).
//!
//! The module is therefore a **pure structural surface**: typed
//! representations of the spec/02 §4-§7 picture-decomposition state
//! that the per-plane decode call needs in order to dispatch correctly.

use super::header::FrameFlags;
use super::picture_layer::{PLANE_COUNT, PLANE_IDX_Y};

// ---- spec/02 §4.1 ---------------------------------------------------
//
// The strip-width constants `LUMA_STRIP_WIDTH` (= 0xa0) and
// `CHROMA_STRIP_WIDTH` (= 0x28) are re-exported from
// `super::macroblock` so the strip-geometry helpers in this module
// share the single canonical constant pair the binary-tree walker
// (round 3, `spec/03`) already uses.

use super::macroblock::{CHROMA_STRIP_WIDTH, LUMA_STRIP_WIDTH};

// ---- spec/02 §5 (strip-context array) -------------------------------

/// Spec/02 §5 — strip-context array stride in bytes (`0x400`).
///
/// Set by the codec init at `IR32_32.DLL!0x10003d2f`. Each slot has
/// 1 KiB of scratch storage for its base pointers, width / height,
/// strip scratch, and per-cell binary-tree sub-array.
pub const STRIP_SLOT_STRIDE: usize = 0x400;

/// Spec/02 §5 — total slot count in the strip-context array (32).
///
/// The init loop at `IR32_32.DLL!0x10003d29..0x10003d35` writes the
/// all-ones sentinel to 32 slots. Only the first six are addressed
/// by the per-plane decode-call dispatcher; the remaining 26 are
/// scratch slots used by the binary-tree walker to hold pushed
/// sub-cells (spec/03's stack discipline).
pub const STRIP_SLOT_COUNT: usize = 32;

/// Spec/02 §5 — dispatchable slot count (2 banks × 3 planes = 6).
pub const DISPATCHABLE_SLOT_COUNT: usize = 6;

/// Spec/02 §5 — all-ones sentinel written to each slot at codec-init
/// (`IR32_32.DLL!0x10003d2f` immediate `0x1869f`).
pub const STRIP_SLOT_SENTINEL: u32 = 0x1869f;

/// Spec/02 §5 — strip-context array offset within the instance state
/// (`+0x414`). The view passed as the 1st argument to the per-plane
/// decode call (`[instance+0x46c]->[0x300c]`) is a flattened pointer
/// into this region.
pub const STRIP_ARRAY_OFFSET_IN_INSTANCE: usize = 0x414;

/// Spec/02 §7 — instance-state block size in bytes (`0x3010`).
///
/// Allocated once per `ICDecompressBegin` at
/// `IR32_32.DLL!0x10003ca9..0x10003cb0` via the host's heap-alloc
/// pointer at `0x10055170`.
pub const INSTANCE_STATE_LEN: usize = 0x3010;

/// Spec/02 §7 — pixel-buffer arena size in bytes (`0x8020`).
///
/// Allocated once per `ICDecompressBegin` at
/// `IR32_32.DLL!0x10003cdc..0x10003ce3`. The storage that each
/// strip-context slot's base pointers refer to.
pub const PIXEL_BUFFER_ARENA_LEN: usize = 0x8020;

/// Spec/02 §6 / §7 — offset within the instance state for the
/// strip-context-array view pointer (`+0x300c`).
///
/// The per-plane decode call's 1st argument is loaded from
/// `[instance+0x46c]->[0x300c]`.
pub const INSTANCE_STRIP_ARRAY_VIEW_PTR: usize = 0x300c;

/// Spec/02 §6 — offset within the instance state for the secondary
/// codebook pointer (`+0x3004`).
///
/// The per-plane decode call's 6th argument (`cb_offset`-biased
/// secondary codebook pointer; see spec/01 §3.4 for `cb_offset`).
pub const INSTANCE_SECONDARY_CODEBOOK_PTR: usize = 0x3004;

/// Spec/02 §6 — luma codebook bank offset within the instance state
/// (`+0x1a00`).
///
/// Selected by the per-plane decode call's codebook-bank discriminant
/// when `plane_idx == 0` (luma), at
/// `IR32_32.DLL!0x100045a3..0x100045a9`.
pub const INSTANCE_LUMA_CODEBOOK_BANK: usize = 0x1a00;

/// Spec/02 §6 — chroma codebook bank offset within the instance state
/// (`+0x400`).
///
/// Selected by the per-plane decode call's codebook-bank discriminant
/// when `plane_idx != 0` (chroma), at
/// `IR32_32.DLL!0x1000458d..0x10004593`.
pub const INSTANCE_CHROMA_CODEBOOK_BANK: usize = 0x400;

// ---- spec/02 §5.2 (per-slot field layout) ---------------------------

/// Spec/02 §5.2 — number of per-slot base-pointer fields (6, one each
/// at `+0x00`, `+0x04`, `+0x08`, `+0x0c`, `+0x10`, `+0x14`).
pub const STRIP_SLOT_BASE_PTR_COUNT: usize = 6;

/// Spec/02 §5.2 — per-slot field offsets, in bytes, from the slot's
/// own start.
///
/// Established by the initialiser at
/// `IR32_32.DLL!0x10003edc..0x10003f3a`. Fields beyond `+0x1c`
/// (strip-scratch and the per-cell sub-array) are documented as
/// spec/03's subject and surfaced here only as range constants.
pub mod slot_field {
    /// `+0x00` — base ptr 0 (start of strip's pixel buffer,
    /// slot-relative).
    pub const BASE_PTR_0: usize = 0x00;
    /// `+0x04` — base ptr 1 (slot-relative offset for top-edge
    /// prediction).
    pub const BASE_PTR_1: usize = 0x04;
    /// `+0x08` — base ptr 2 (slot-relative offset).
    pub const BASE_PTR_2: usize = 0x08;
    /// `+0x0c` — base ptr 3 (slot-relative offset).
    pub const BASE_PTR_3: usize = 0x0c;
    /// `+0x10` — base ptr 4 (slot-relative offset).
    pub const BASE_PTR_4: usize = 0x10;
    /// `+0x14` — base ptr 5 (slot-relative offset).
    pub const BASE_PTR_5: usize = 0x14;
    /// `+0x18` — strip height in pixels. Initialised once from the
    /// plane height at `IR32_32.DLL!0x10003f37..0x10003f3a` and never
    /// rewritten (spec/02 §4.4).
    pub const STRIP_HEIGHT: usize = 0x18;
    /// `+0x1c` — strip width in pixels. Default `0xa0` / `0x28` per
    /// plane class, overridden to the remainder width for the last
    /// strip per spec/02 §4.1.
    pub const STRIP_WIDTH: usize = 0x1c;
    /// First byte of the strip-scratch region (spec/03's subject).
    pub const STRIP_SCRATCH_BEGIN: usize = 0x20;
    /// One-past-the-last byte of the strip-scratch region.
    pub const STRIP_SCRATCH_END: usize = 0x40;
    /// First byte of the per-cell binary-tree sub-array (spec/03's
    /// stack discipline).
    pub const CELL_SUBARRAY_BEGIN: usize = 0x40;
}

// ---- spec/02 §5.1 (slot index discipline) ---------------------------

/// Spec/02 §5.1 — primary buffer slot indices, indexed by plane_idx.
///
/// In use when `frame_flags` bit 9 (`BUFFER_SELECTOR`) is **clear**
/// (= 0, primary). Computed by the parser at
/// `IR32_32.DLL!0x100045b1..0x100045fd` as `plane_idx + 3`.
pub const PRIMARY_BANK_SLOTS: [usize; PLANE_COUNT] = [3, 4, 5];

/// Spec/02 §5.1 — secondary buffer slot indices, indexed by plane_idx.
///
/// In use when `frame_flags` bit 9 (`BUFFER_SELECTOR`) is **set**
/// (= 1, secondary). Computed by the parser as `plane_idx`.
pub const SECONDARY_BANK_SLOTS: [usize; PLANE_COUNT] = [0, 1, 2];

/// Spec/02 §5.1 / §6 — plane classification a strip-context slot
/// belongs to.
///
/// The per-plane decode call's luma / chroma branches at
/// `IR32_32.DLL!0x10006acd` and `IR32_32.DLL!0x10006acf` distinguish
/// slots 0 and 3 (luma) from the remaining slots (chroma). Slots
/// 6..=31 are scratch slots (spec/03's stack discipline) and have no
/// plane association.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneRole {
    /// Slot belongs to the luma (Y) plane.
    Luma,
    /// Slot belongs to a chroma (U or V) plane.
    Chroma,
    /// Slot is a scratch slot (slot index ≥ 6); no plane association.
    Scratch,
}

impl PlaneRole {
    /// Spec/02 §5.1 / §6 — classify a slot index by plane role.
    ///
    /// Slot index 0 (Y / secondary) and slot index 3 (Y / primary)
    /// are luma. Slot indices 1, 2, 4, 5 are chroma. Indices ≥ 6 are
    /// scratch.
    pub fn for_slot(slot_idx: usize) -> Self {
        match slot_idx {
            0 | 3 => PlaneRole::Luma,
            1 | 2 | 4 | 5 => PlaneRole::Chroma,
            _ => PlaneRole::Scratch,
        }
    }

    /// True iff this role is luma.
    pub fn is_luma(self) -> bool {
        matches!(self, PlaneRole::Luma)
    }

    /// True iff this role is a real chroma plane (U or V).
    pub fn is_chroma(self) -> bool {
        matches!(self, PlaneRole::Chroma)
    }
}

/// Spec/02 §5.1 — strip-context slot index for `(plane_idx, buffer)`.
///
/// Returns the slot index in `0..DISPATCHABLE_SLOT_COUNT`. The
/// `buffer_selector` flag is `frame_flags` bit 9 — `true` means
/// secondary (slots 0..2), `false` means primary (slots 3..5).
///
/// Returns `None` if `plane_idx >= PLANE_COUNT` — the only legal
/// values are `PLANE_IDX_Y`, `PLANE_IDX_V`, `PLANE_IDX_U`.
pub fn strip_slot_index(plane_idx: usize, buffer_selector: bool) -> Option<usize> {
    if plane_idx >= PLANE_COUNT {
        return None;
    }
    if buffer_selector {
        Some(SECONDARY_BANK_SLOTS[plane_idx])
    } else {
        Some(PRIMARY_BANK_SLOTS[plane_idx])
    }
}

/// Spec/02 §5.2 — descriptor of one strip-context slot.
///
/// Records the slot's index within the strip-context array, the
/// plane role it covers, the per-strip dimensions (width is the §4.1
/// per-plane constant or the remainder width; height is the plane
/// height), and the byte offset of the slot's start within the
/// strip-context array (= `slot_idx * STRIP_SLOT_STRIDE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StripSlotDescriptor {
    /// Slot index within the strip-context array (0..6 for
    /// dispatchable slots; 6..32 for scratch).
    pub slot_idx: usize,
    /// Plane role this slot covers.
    pub role: PlaneRole,
    /// Byte offset of the slot's start within the strip-context
    /// array (= `slot_idx * STRIP_SLOT_STRIDE`).
    pub array_byte_offset: usize,
    /// Strip width in plane samples (spec/02 §4.1, written to
    /// `[ctx+0x1c]`).
    pub strip_width: u32,
    /// Strip height in plane samples (spec/02 §4.4, written to
    /// `[ctx+0x18]` and equal to the plane height).
    pub strip_height: u32,
}

impl StripSlotDescriptor {
    /// Build the descriptor for a dispatchable slot.
    ///
    /// Returns `None` if `plane_idx >= PLANE_COUNT`. The strip
    /// `width` / `height` are the spec/02 §4.1 / §4.4 per-plane
    /// values (the chroma planes' width is the luma value divided by
    /// the 4:1 subsampling ratio; the height is the plane height).
    pub fn for_dispatch(
        plane_idx: usize,
        buffer_selector: bool,
        strip_width: u32,
        strip_height: u32,
    ) -> Option<Self> {
        let slot_idx = strip_slot_index(plane_idx, buffer_selector)?;
        let role = PlaneRole::for_slot(slot_idx);
        Some(StripSlotDescriptor {
            slot_idx,
            role,
            array_byte_offset: slot_idx * STRIP_SLOT_STRIDE,
            strip_width,
            strip_height,
        })
    }

    /// Byte offset, within the strip-context array, of the slot's
    /// `STRIP_WIDTH` field (`[slot + 0x1c]`).
    pub fn strip_width_field_offset(&self) -> usize {
        self.array_byte_offset + slot_field::STRIP_WIDTH
    }

    /// Byte offset, within the strip-context array, of the slot's
    /// `STRIP_HEIGHT` field (`[slot + 0x18]`).
    pub fn strip_height_field_offset(&self) -> usize {
        self.array_byte_offset + slot_field::STRIP_HEIGHT
    }
}

// ---- spec/02 §4.1 / §4.2 (strip geometry) ---------------------------

/// Spec/02 §4.1 — round-up division by the strip width.
///
/// `ceil(width / strip_width)`. The reference parser computes this
/// as `(width + (strip_width-1)) / strip_width` (`(width + 0x9f) /
/// 0xa0` for luma at `IR32_32.DLL!0x10003d6b..0x10003d73`).
fn ceil_div(width: u32, strip_width: u32) -> u32 {
    debug_assert!(strip_width != 0);
    width.div_ceil(strip_width)
}

/// Spec/02 §4.1 — width of the last (rightmost) strip when the
/// picture width is not a multiple of the strip width.
///
/// Formula `((width - 1) mod strip_width) + 1`, computed at
/// `IR32_32.DLL!0x10003f53..0x10003f6c`. For widths that **are** a
/// multiple of the strip width the result equals the strip width
/// itself (no special case).
fn last_strip_width(picture_width: u32, strip_width: u32) -> u32 {
    debug_assert!(picture_width > 0);
    debug_assert!(strip_width != 0);
    ((picture_width - 1) % strip_width) + 1
}

/// Spec/02 §4.1 / §4.2 — per-plane strip geometry.
///
/// Records the strip count, the strip width (or widths, for the
/// remainder case), and the plane height.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripGeometry {
    /// Plane role (`Luma` or `Chroma`).
    pub role: PlaneRole,
    /// Plane width in samples.
    pub plane_width: u32,
    /// Plane height in samples.
    pub plane_height: u32,
    /// Strip width constant for this plane class (`0xa0` for luma,
    /// `0x28` for chroma).
    pub strip_width: u32,
    /// `ceil(plane_width / strip_width)`.
    pub strip_count: u32,
    /// Width of the last (rightmost) strip — `strip_width` for
    /// pictures whose width is a multiple of `strip_width`,
    /// otherwise `((plane_width - 1) mod strip_width) + 1`.
    pub last_strip_width: u32,
}

impl StripGeometry {
    /// Spec/02 §4.1 — build the luma-plane geometry from
    /// `(plane_width, plane_height)`.
    pub fn for_luma(plane_width: u32, plane_height: u32) -> Self {
        Self::build(PlaneRole::Luma, plane_width, plane_height, LUMA_STRIP_WIDTH)
    }

    /// Spec/02 §4.1 — build the chroma-plane geometry from
    /// `(plane_width, plane_height)`. The chroma plane width is the
    /// luma width divided by the 4:1 subsampling ratio.
    pub fn for_chroma(plane_width: u32, plane_height: u32) -> Self {
        Self::build(
            PlaneRole::Chroma,
            plane_width,
            plane_height,
            CHROMA_STRIP_WIDTH,
        )
    }

    fn build(role: PlaneRole, plane_width: u32, plane_height: u32, strip_width: u32) -> Self {
        let (strip_count, last_w) = if plane_width == 0 {
            (0, 0)
        } else {
            (
                ceil_div(plane_width, strip_width),
                last_strip_width(plane_width, strip_width),
            )
        };
        StripGeometry {
            role,
            plane_width,
            plane_height,
            strip_width,
            strip_count,
            last_strip_width: last_w,
        }
    }

    /// True when the picture width divides evenly into strips —
    /// `last_strip_width == strip_width`.
    pub fn is_aligned(&self) -> bool {
        self.strip_count == 0 || self.last_strip_width == self.strip_width
    }

    /// Iterate the per-strip widths in left-to-right order. Every
    /// strip except possibly the last has width `strip_width`; the
    /// last has width `last_strip_width`.
    pub fn iter_strip_widths(&self) -> impl Iterator<Item = u32> + '_ {
        (0..self.strip_count).map(|i| {
            if i + 1 == self.strip_count {
                self.last_strip_width
            } else {
                self.strip_width
            }
        })
    }
}

// ---- spec/02 §6 (per-plane decode call) -----------------------------

/// Spec/02 §6 — terminal status of a per-plane decode call.
///
/// The per-plane decoder at `IR32_32.DLL!0x10006538` returns an
/// integer status (`eax`) that the outer parser stores at
/// `[ebp-0x8]`. The two values observed are:
///
/// - `0` — success; the binary-tree walk consumed the plane and
///   produced reconstructed strip pixels.
/// - `3` — malformed bitstream; treated as an end-of-frame fault
///   (`IR32_32.DLL!0x10006ba2..0x10006baa`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneDecodeStatus {
    /// Plane decoded successfully (`eax == 0`).
    Ok,
    /// Plane decode raised a malformed-bitstream fault
    /// (`eax == 3`).
    Malformed,
}

/// Spec/02 §6 — the integer status code value the reference decoder
/// returns on a malformed bitstream (`3`).
pub const PLANE_DECODE_STATUS_MALFORMED: i32 = 3;
/// Spec/02 §6 — the integer status code value the reference decoder
/// returns on success (`0`).
pub const PLANE_DECODE_STATUS_OK: i32 = 0;

impl PlaneDecodeStatus {
    /// Spec/02 §6 — classify the `eax` value the per-plane decoder
    /// returns. Any non-zero, non-`3` value falls back to `Malformed`
    /// to match the outer parser's "treat any non-zero as fault"
    /// semantics.
    pub fn from_eax(eax: i32) -> Self {
        if eax == PLANE_DECODE_STATUS_OK {
            PlaneDecodeStatus::Ok
        } else {
            PlaneDecodeStatus::Malformed
        }
    }

    /// True iff the status is `Ok`.
    pub fn is_ok(self) -> bool {
        matches!(self, PlaneDecodeStatus::Ok)
    }
}

/// Spec/02 §6 — typed view of the seven cdecl arguments to the
/// per-plane decode call (`IR32_32.DLL!0x10006538`).
///
/// The arguments are pushed right-to-left; `arg1` is the first one
/// pushed (and the *first* one read inside the callee). Field roles
/// follow the spec/02 §6 table verbatim, with the codebook-bank
/// discriminant at §6 resolved to the per-plane offset constant.
///
/// **What this struct does not do.** It does not model the actual
/// pointer values (those live in host memory) — it models the
/// *byte-offset* discriminants the dispatcher applies, so a future
/// caller that holds a real instance-state buffer can resolve each
/// argument by adding the offset to its base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerPlaneDecodeCall {
    /// Plane index (0 = Y, 1 = V, 2 = U) — drives the codebook-bank
    /// discriminant + the slot-index computation.
    pub plane_idx: usize,
    /// Buffer-selector bit (`frame_flags` bit 9) — drives the slot
    /// bank selection (primary vs secondary).
    pub buffer_selector: bool,
    /// Spec/02 §6, 1st argument — offset within instance state for
    /// the strip-context-array view pointer (`+0x300c`, constant).
    pub strip_array_view_offset: usize,
    /// Spec/02 §6, 2nd argument — strip-context source slot index
    /// (used for inter-plane reference reads).
    pub slot_idx_src: usize,
    /// Spec/02 §6, 3rd argument — strip-context destination slot
    /// index (where the per-plane decoder writes its strips).
    pub slot_idx_dst: usize,
    /// Spec/02 §6, 4th argument — caller-supplied byte offset of the
    /// first byte of the plane's binary-tree / VQ bitstream payload
    /// (= `plane_base + 4 + 2*num_vectors`, from spec/02 §3.4 and
    /// the round-2 prelude parser's `bitstream_offset`).
    pub bitstream_payload_offset: usize,
    /// Spec/02 §6, 5th argument — instance-state base pointer (no
    /// offset; the caller passes `[instance+0x46c]` directly).
    pub instance_state_base_offset: usize,
    /// Spec/02 §6, 6th argument — offset within instance state for
    /// the secondary codebook pointer (`+0x3004`, constant).
    pub secondary_codebook_offset: usize,
    /// Spec/02 §6, 7th argument — offset within instance state for
    /// the codebook bank (`+0x1a00` for luma, `+0x400` for chroma).
    pub codebook_bank_offset: usize,
}

impl PerPlaneDecodeCall {
    /// Spec/02 §6 — build the typed argument frame for the per-plane
    /// decode call.
    ///
    /// `plane_idx` is the spec/02 §2 plane index (`PLANE_IDX_Y`,
    /// `PLANE_IDX_V`, `PLANE_IDX_U`). `bitstream_payload_offset` is
    /// the round-2 prelude parser's
    /// [`PlanePrelude::bitstream_offset`](super::picture_layer::PlanePrelude::bitstream_offset).
    ///
    /// Returns `None` if `plane_idx >= PLANE_COUNT` (the only legal
    /// values are the three plane-index constants).
    ///
    /// `slot_idx_src` and `slot_idx_dst` are set to the same value
    /// per spec/02 §10 item 3 (the only call path observed in the
    /// binary computes them identically); callers wanting to model a
    /// hypothetical inter-bank read may override either field after
    /// construction.
    pub fn for_plane(
        plane_idx: usize,
        flags: FrameFlags,
        bitstream_payload_offset: usize,
    ) -> Option<Self> {
        Self::for_plane_and_buffer(plane_idx, flags.buffer_selector(), bitstream_payload_offset)
    }

    /// Spec/02 §6 — sibling constructor that takes the spec/02 §3.2 / §5.1
    /// buffer-selector bit directly, without round-tripping through
    /// [`FrameFlags`].
    ///
    /// Used by [`PlaneDecodePlan::to_decode_call`](super::picture_layer::PlaneDecodePlan::to_decode_call)
    /// to bridge the picture-layer plan (which already carries the buffer-
    /// selector bit per spec/02 §5.1) to this §6 call frame without
    /// reconstructing the full `frame_flags` u16. The two constructors
    /// produce identical structures for any
    /// `(plane_idx, FrameFlags::buffer_selector() == buffer_selector,
    /// bitstream_payload_offset)` triple.
    ///
    /// Returns `None` under the same condition as
    /// [`Self::for_plane`]: `plane_idx >= PLANE_COUNT`.
    ///
    /// `slot_idx_src` and `slot_idx_dst` are set to the same value per
    /// spec/02 §10 item 3 (the only call path observed in the binary
    /// computes them identically).
    pub fn for_plane_and_buffer(
        plane_idx: usize,
        buffer_selector: bool,
        bitstream_payload_offset: usize,
    ) -> Option<Self> {
        if plane_idx >= PLANE_COUNT {
            return None;
        }
        let slot_idx = strip_slot_index(plane_idx, buffer_selector)?;
        let codebook_bank_offset = if plane_idx == PLANE_IDX_Y {
            INSTANCE_LUMA_CODEBOOK_BANK
        } else {
            INSTANCE_CHROMA_CODEBOOK_BANK
        };
        Some(PerPlaneDecodeCall {
            plane_idx,
            buffer_selector,
            strip_array_view_offset: INSTANCE_STRIP_ARRAY_VIEW_PTR,
            slot_idx_src: slot_idx,
            slot_idx_dst: slot_idx,
            bitstream_payload_offset,
            instance_state_base_offset: 0,
            secondary_codebook_offset: INSTANCE_SECONDARY_CODEBOOK_PTR,
            codebook_bank_offset,
        })
    }

    /// Spec/02 §6 — the plane-role classification for this call's
    /// destination slot.
    pub fn plane_role(&self) -> PlaneRole {
        PlaneRole::for_slot(self.slot_idx_dst)
    }
}

// ---- spec/02 §7 (codec-init strip-context allocation) ---------------

/// Spec/02 §7 — count of luma strip-context slots required by
/// `ICDecompressBegin` for a picture of luma width `W`.
///
/// `ceil(W / 160)`. For `W <= 160` the count is 1.
pub fn luma_strip_slot_count(plane_width: u32) -> u32 {
    if plane_width == 0 {
        0
    } else {
        ceil_div(plane_width, LUMA_STRIP_WIDTH)
    }
}

/// Spec/02 §7 — count of chroma strip-context slots required by
/// `ICDecompressBegin` for a picture of luma width `W`.
///
/// `ceil(W / 16)` per the spec's "each chroma strip count = ceil(width
/// / 16) per the chroma subsampling, with strip width 0x28 = 40"
/// wording (§7 item 4). For a chroma plane whose own width is
/// `chroma_width = luma_width / 4`, this equals
/// `ceil(chroma_width / 4)` of the chroma 0x28 strip width, but the
/// init routine derives it from the luma width with the constant
/// 16 divisor — the formula this function implements.
pub fn chroma_strip_slot_count(luma_width: u32) -> u32 {
    if luma_width == 0 {
        0
    } else {
        ceil_div(luma_width, 16)
    }
}

/// Spec/02 §7 — chroma plane height (rounded down to a multiple of 4
/// per the `& -0x4` mask at `IR32_32.DLL!0x10003f94`).
///
/// The chroma plane height is the luma height divided by 4 (the 4:1
/// chroma subsampling ratio), then aligned down to a multiple of 4
/// by the bitwise mask the init routine applies.
pub fn chroma_plane_height(luma_height: u32) -> u32 {
    (luma_height / 4) & !0x3
}

/// Spec/02 §4 — chroma plane width (luma width divided by the 4:1
/// chroma subsampling ratio).
///
/// The picture-decomposition table in `spec/02-picture-layer.md` §4
/// defines the chroma plane dimensions as `(width/4) × (height/4)`.
/// Unlike [`chroma_plane_height`] the §7 codec-init routine does
/// **not** apply a multiple-of-4 alignment mask to the width — the
/// strip-width chain (chroma `0x28` = 40) carries the
/// per-strip-width remainder accounting (`((W-1) mod strip_width) +
/// 1` per §4.1) without requiring the plane width itself to be
/// aligned.
pub fn chroma_plane_width(luma_width: u32) -> u32 {
    luma_width / 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indeo3::header::FrameFlags;
    use crate::indeo3::picture_layer::{PLANE_IDX_U, PLANE_IDX_V};

    // ---- spec/02 §5.1 (slot index discipline) -----------------------

    #[test]
    fn primary_bank_slot_indices_match_spec_table() {
        // spec/02 §5.1 table: primary bank → slots 3, 4, 5 for Y, V, U.
        assert_eq!(PRIMARY_BANK_SLOTS[PLANE_IDX_Y], 3);
        assert_eq!(PRIMARY_BANK_SLOTS[PLANE_IDX_V], 4);
        assert_eq!(PRIMARY_BANK_SLOTS[PLANE_IDX_U], 5);
    }

    #[test]
    fn secondary_bank_slot_indices_match_spec_table() {
        // spec/02 §5.1 table: secondary bank → slots 0, 1, 2 for Y, V, U.
        assert_eq!(SECONDARY_BANK_SLOTS[PLANE_IDX_Y], 0);
        assert_eq!(SECONDARY_BANK_SLOTS[PLANE_IDX_V], 1);
        assert_eq!(SECONDARY_BANK_SLOTS[PLANE_IDX_U], 2);
    }

    #[test]
    fn strip_slot_index_resolves_both_banks() {
        // Secondary bank (bit 9 = 1 = true).
        assert_eq!(strip_slot_index(PLANE_IDX_Y, true), Some(0));
        assert_eq!(strip_slot_index(PLANE_IDX_V, true), Some(1));
        assert_eq!(strip_slot_index(PLANE_IDX_U, true), Some(2));
        // Primary bank (bit 9 = 0 = false).
        assert_eq!(strip_slot_index(PLANE_IDX_Y, false), Some(3));
        assert_eq!(strip_slot_index(PLANE_IDX_V, false), Some(4));
        assert_eq!(strip_slot_index(PLANE_IDX_U, false), Some(5));
    }

    #[test]
    fn strip_slot_index_rejects_out_of_range_plane() {
        assert_eq!(strip_slot_index(PLANE_COUNT, true), None);
        assert_eq!(strip_slot_index(usize::MAX, false), None);
    }

    #[test]
    fn plane_role_for_slot_classifies_six_dispatchable_slots() {
        // Spec/02 §5.1 + §6: slots 0 (Y/sec) and 3 (Y/pri) are luma.
        assert_eq!(PlaneRole::for_slot(0), PlaneRole::Luma);
        assert_eq!(PlaneRole::for_slot(3), PlaneRole::Luma);
        // Slots 1 (V/sec), 2 (U/sec), 4 (V/pri), 5 (U/pri) are chroma.
        assert_eq!(PlaneRole::for_slot(1), PlaneRole::Chroma);
        assert_eq!(PlaneRole::for_slot(2), PlaneRole::Chroma);
        assert_eq!(PlaneRole::for_slot(4), PlaneRole::Chroma);
        assert_eq!(PlaneRole::for_slot(5), PlaneRole::Chroma);
        // Slots 6..32 are scratch.
        assert_eq!(PlaneRole::for_slot(6), PlaneRole::Scratch);
        assert_eq!(PlaneRole::for_slot(31), PlaneRole::Scratch);
    }

    // ---- spec/02 §5 (strip-context array constants) -----------------

    #[test]
    fn strip_array_layout_constants_match_spec_table() {
        // Spec/02 §5: stride 0x400, 32 slots, 6 dispatchable.
        assert_eq!(STRIP_SLOT_STRIDE, 0x400);
        assert_eq!(STRIP_SLOT_COUNT, 32);
        assert_eq!(DISPATCHABLE_SLOT_COUNT, 6);
        // Sentinel + instance offsets.
        assert_eq!(STRIP_SLOT_SENTINEL, 0x1869f);
        assert_eq!(STRIP_ARRAY_OFFSET_IN_INSTANCE, 0x414);
        assert_eq!(INSTANCE_STATE_LEN, 0x3010);
        assert_eq!(PIXEL_BUFFER_ARENA_LEN, 0x8020);
        // §6 sub-field offsets.
        assert_eq!(INSTANCE_STRIP_ARRAY_VIEW_PTR, 0x300c);
        assert_eq!(INSTANCE_SECONDARY_CODEBOOK_PTR, 0x3004);
        assert_eq!(INSTANCE_LUMA_CODEBOOK_BANK, 0x1a00);
        assert_eq!(INSTANCE_CHROMA_CODEBOOK_BANK, 0x400);
    }

    #[test]
    fn slot_field_offsets_match_spec_table() {
        // Spec/02 §5.2: six base pointers at +0x00, +4, +8, +c, +10, +14.
        assert_eq!(slot_field::BASE_PTR_0, 0x00);
        assert_eq!(slot_field::BASE_PTR_1, 0x04);
        assert_eq!(slot_field::BASE_PTR_2, 0x08);
        assert_eq!(slot_field::BASE_PTR_3, 0x0c);
        assert_eq!(slot_field::BASE_PTR_4, 0x10);
        assert_eq!(slot_field::BASE_PTR_5, 0x14);
        // Strip height / width at +0x18 / +0x1c.
        assert_eq!(slot_field::STRIP_HEIGHT, 0x18);
        assert_eq!(slot_field::STRIP_WIDTH, 0x1c);
        // Strip-scratch and per-cell sub-array.
        assert_eq!(slot_field::STRIP_SCRATCH_BEGIN, 0x20);
        assert_eq!(slot_field::STRIP_SCRATCH_END, 0x40);
        assert_eq!(slot_field::CELL_SUBARRAY_BEGIN, 0x40);
        // The base-pointer count matches the table's row count.
        assert_eq!(STRIP_SLOT_BASE_PTR_COUNT, 6);
    }

    #[test]
    fn strip_slot_descriptor_records_offsets_and_role() {
        // Y / primary buffer → slot 3, luma, at byte offset 3 * 0x400.
        let d = StripSlotDescriptor::for_dispatch(PLANE_IDX_Y, false, 160, 120).unwrap();
        assert_eq!(d.slot_idx, 3);
        assert_eq!(d.role, PlaneRole::Luma);
        assert_eq!(d.array_byte_offset, 3 * STRIP_SLOT_STRIDE);
        assert_eq!(d.strip_width, 160);
        assert_eq!(d.strip_height, 120);
        assert_eq!(d.strip_width_field_offset(), 3 * STRIP_SLOT_STRIDE + 0x1c);
        assert_eq!(d.strip_height_field_offset(), 3 * STRIP_SLOT_STRIDE + 0x18);

        // U / secondary buffer → slot 2, chroma, at byte offset 2 * 0x400.
        let d = StripSlotDescriptor::for_dispatch(PLANE_IDX_U, true, 40, 30).unwrap();
        assert_eq!(d.slot_idx, 2);
        assert_eq!(d.role, PlaneRole::Chroma);
        assert_eq!(d.array_byte_offset, 2 * STRIP_SLOT_STRIDE);
    }

    #[test]
    fn strip_slot_descriptor_rejects_out_of_range_plane() {
        assert!(StripSlotDescriptor::for_dispatch(PLANE_COUNT, false, 160, 120).is_none());
    }

    // ---- spec/02 §4.1 / §4.2 (strip geometry) -----------------------

    #[test]
    fn luma_strip_geometry_aligned_widths_match_spec_table() {
        // Spec/02 §4.2 informative table — single strip for W <= 160.
        let g = StripGeometry::for_luma(160, 120);
        assert_eq!(g.strip_count, 1);
        assert_eq!(g.last_strip_width, 160);
        assert!(g.is_aligned());

        // Two aligned strips for W = 320.
        let g = StripGeometry::for_luma(320, 240);
        assert_eq!(g.strip_count, 2);
        assert_eq!(g.last_strip_width, 160);
        assert!(g.is_aligned());

        // Three aligned strips for W = 480.
        let g = StripGeometry::for_luma(480, 360);
        assert_eq!(g.strip_count, 3);
        assert_eq!(g.last_strip_width, 160);

        // Four aligned strips for W = 640.
        let g = StripGeometry::for_luma(640, 480);
        assert_eq!(g.strip_count, 4);
        assert_eq!(g.last_strip_width, 160);
    }

    #[test]
    fn luma_strip_geometry_remainder_widths_match_spec_table() {
        // Spec/02 §4.2 — remainder strip widths.
        // 161..=320 → 2 strips, last = W - 160.
        let g = StripGeometry::for_luma(176, 144);
        assert_eq!(g.strip_count, 2);
        assert_eq!(g.last_strip_width, 176 - 160);
        assert!(!g.is_aligned());

        let g = StripGeometry::for_luma(240, 180);
        assert_eq!(g.strip_count, 2);
        assert_eq!(g.last_strip_width, 240 - 160);

        // 321..=480 → 3 strips, last = W - 320.
        let g = StripGeometry::for_luma(352, 288);
        assert_eq!(g.strip_count, 3);
        assert_eq!(g.last_strip_width, 352 - 320);

        // 481..=640 → 4 strips, last = W - 480.
        let g = StripGeometry::for_luma(576, 432);
        assert_eq!(g.strip_count, 4);
        assert_eq!(g.last_strip_width, 576 - 480);
    }

    #[test]
    fn chroma_strip_geometry_uses_28_strip_width() {
        // Chroma width for a 640-wide luma picture is 160; chroma
        // strip width is 0x28 = 40, so 160 / 40 = 4 strips, aligned.
        let g = StripGeometry::for_chroma(160, 120);
        assert_eq!(g.strip_count, 4);
        assert_eq!(g.last_strip_width, 40);
        assert_eq!(g.role, PlaneRole::Chroma);
        assert!(g.is_aligned());

        // Chroma width 44 (luma 176 / 4) → 2 strips, last = 4.
        let g = StripGeometry::for_chroma(44, 36);
        assert_eq!(g.strip_count, 2);
        assert_eq!(g.last_strip_width, 44 - 40);
    }

    #[test]
    fn strip_geometry_iter_widths_lists_all_strips() {
        // Three aligned 160-wide strips.
        let g = StripGeometry::for_luma(480, 360);
        let widths: Vec<u32> = g.iter_strip_widths().collect();
        assert_eq!(widths, vec![160, 160, 160]);

        // Two strips, last is 16 wide.
        let g = StripGeometry::for_luma(176, 144);
        let widths: Vec<u32> = g.iter_strip_widths().collect();
        assert_eq!(widths, vec![160, 16]);

        // Width 0 → empty.
        let g = StripGeometry::for_luma(0, 0);
        let widths: Vec<u32> = g.iter_strip_widths().collect();
        assert_eq!(widths, Vec::<u32>::new());
        assert_eq!(g.strip_count, 0);
    }

    // ---- spec/02 §6 (per-plane decode call signature) ---------------

    #[test]
    fn per_plane_decode_call_luma_primary() {
        // frame_flags bit 9 = 0 → primary bank → Y at slot 3.
        let flags = FrameFlags(0x0000);
        assert!(!flags.buffer_selector());
        let call = PerPlaneDecodeCall::for_plane(PLANE_IDX_Y, flags, 0x1234).unwrap();
        assert_eq!(call.plane_idx, PLANE_IDX_Y);
        assert!(!call.buffer_selector);
        assert_eq!(call.slot_idx_src, 3);
        assert_eq!(call.slot_idx_dst, 3);
        assert_eq!(call.bitstream_payload_offset, 0x1234);
        assert_eq!(call.codebook_bank_offset, INSTANCE_LUMA_CODEBOOK_BANK);
        assert_eq!(call.strip_array_view_offset, INSTANCE_STRIP_ARRAY_VIEW_PTR);
        assert_eq!(
            call.secondary_codebook_offset,
            INSTANCE_SECONDARY_CODEBOOK_PTR
        );
        assert_eq!(call.plane_role(), PlaneRole::Luma);
    }

    #[test]
    fn per_plane_decode_call_luma_secondary() {
        // frame_flags bit 9 = 1 → secondary bank → Y at slot 0.
        let flags = FrameFlags(0x0200);
        assert!(flags.buffer_selector());
        let call = PerPlaneDecodeCall::for_plane(PLANE_IDX_Y, flags, 0xc0de).unwrap();
        assert!(call.buffer_selector);
        assert_eq!(call.slot_idx_src, 0);
        assert_eq!(call.slot_idx_dst, 0);
        assert_eq!(call.codebook_bank_offset, INSTANCE_LUMA_CODEBOOK_BANK);
        assert_eq!(call.plane_role(), PlaneRole::Luma);
    }

    #[test]
    fn per_plane_decode_call_chroma_picks_400_bank() {
        // Both chroma planes use the +0x400 codebook bank.
        let flags = FrameFlags(0x0000);
        for &plane_idx in &[PLANE_IDX_V, PLANE_IDX_U] {
            let call = PerPlaneDecodeCall::for_plane(plane_idx, flags, 0x0).unwrap();
            assert_eq!(call.codebook_bank_offset, INSTANCE_CHROMA_CODEBOOK_BANK);
            assert_eq!(call.plane_role(), PlaneRole::Chroma);
        }
    }

    #[test]
    fn per_plane_decode_call_chroma_secondary_buffer() {
        // V / secondary → slot 1, chroma.
        // U / secondary → slot 2, chroma.
        let flags = FrameFlags(0x0200);
        let call = PerPlaneDecodeCall::for_plane(PLANE_IDX_V, flags, 0).unwrap();
        assert_eq!(call.slot_idx_dst, 1);
        assert_eq!(call.codebook_bank_offset, INSTANCE_CHROMA_CODEBOOK_BANK);
        let call = PerPlaneDecodeCall::for_plane(PLANE_IDX_U, flags, 0).unwrap();
        assert_eq!(call.slot_idx_dst, 2);
        assert_eq!(call.codebook_bank_offset, INSTANCE_CHROMA_CODEBOOK_BANK);
    }

    #[test]
    fn per_plane_decode_call_src_eq_dst() {
        // Spec/02 §10 item 3: the only call path observed in the
        // binary sets src == dst. The builder reflects this.
        let flags = FrameFlags(0x0000);
        for plane_idx in 0..PLANE_COUNT {
            let call = PerPlaneDecodeCall::for_plane(plane_idx, flags, 0).unwrap();
            assert_eq!(call.slot_idx_src, call.slot_idx_dst);
        }
    }

    #[test]
    fn per_plane_decode_call_rejects_out_of_range() {
        assert!(PerPlaneDecodeCall::for_plane(PLANE_COUNT, FrameFlags(0x0000), 0).is_none());
        assert!(PerPlaneDecodeCall::for_plane(usize::MAX, FrameFlags(0x0200), 0x1234).is_none());
    }

    #[test]
    fn for_plane_and_buffer_matches_for_plane_for_every_legal_input() {
        // Spec/02 §6 — the bool-direct constructor must produce the
        // same call frame as the FrameFlags constructor for every
        // (plane_idx, buffer_selector, bitstream_payload_offset)
        // triple, since `for_plane` delegates to it.
        for &flags_raw in &[0x0000_u16, 0x0200_u16, 0x0205_u16, 0x0210_u16] {
            let flags = FrameFlags(flags_raw);
            let buffer_selector = flags.buffer_selector();
            for plane_idx in 0..PLANE_COUNT {
                for &payload in &[0_usize, 0x1234, 0xdead_beef, usize::MAX] {
                    let via_flags = PerPlaneDecodeCall::for_plane(plane_idx, flags, payload)
                        .expect("for_plane Some");
                    let via_bool = PerPlaneDecodeCall::for_plane_and_buffer(
                        plane_idx,
                        buffer_selector,
                        payload,
                    )
                    .expect("for_plane_and_buffer Some");
                    assert_eq!(via_flags, via_bool);
                }
            }
        }
    }

    #[test]
    fn for_plane_and_buffer_rejects_out_of_range() {
        // Spec/02 §6 — same rejection condition as `for_plane`.
        assert!(PerPlaneDecodeCall::for_plane_and_buffer(PLANE_COUNT, false, 0).is_none());
        assert!(PerPlaneDecodeCall::for_plane_and_buffer(PLANE_COUNT, true, 0).is_none());
        assert!(PerPlaneDecodeCall::for_plane_and_buffer(usize::MAX, false, 0x1234).is_none());
    }

    // ---- spec/02 §6 (plane-decode status) ---------------------------

    #[test]
    fn plane_decode_status_classifies_eax() {
        // Spec/02 §6: eax == 0 → Ok; eax == 3 → Malformed.
        assert_eq!(PlaneDecodeStatus::from_eax(0), PlaneDecodeStatus::Ok);
        assert_eq!(PlaneDecodeStatus::from_eax(3), PlaneDecodeStatus::Malformed);
        // Any other non-zero value falls back to Malformed.
        assert_eq!(PlaneDecodeStatus::from_eax(1), PlaneDecodeStatus::Malformed);
        assert_eq!(
            PlaneDecodeStatus::from_eax(-1),
            PlaneDecodeStatus::Malformed
        );
        assert_eq!(
            PlaneDecodeStatus::from_eax(0x7fffffff),
            PlaneDecodeStatus::Malformed
        );
        assert!(PlaneDecodeStatus::Ok.is_ok());
        assert!(!PlaneDecodeStatus::Malformed.is_ok());
    }

    // ---- spec/02 §7 (codec-init allocation arithmetic) --------------

    #[test]
    fn luma_strip_slot_count_uses_ceil_div_160() {
        // Spec/02 §7 item 3 — luma strip count = ceil(width / 160).
        assert_eq!(luma_strip_slot_count(0), 0);
        assert_eq!(luma_strip_slot_count(1), 1);
        assert_eq!(luma_strip_slot_count(160), 1);
        assert_eq!(luma_strip_slot_count(161), 2);
        assert_eq!(luma_strip_slot_count(320), 2);
        assert_eq!(luma_strip_slot_count(321), 3);
        assert_eq!(luma_strip_slot_count(480), 3);
        assert_eq!(luma_strip_slot_count(640), 4);
    }

    #[test]
    fn chroma_strip_slot_count_uses_ceil_div_16() {
        // Spec/02 §7 item 4 — chroma strip count = ceil(luma_width / 16).
        assert_eq!(chroma_strip_slot_count(0), 0);
        assert_eq!(chroma_strip_slot_count(1), 1);
        assert_eq!(chroma_strip_slot_count(16), 1);
        assert_eq!(chroma_strip_slot_count(17), 2);
        assert_eq!(chroma_strip_slot_count(160), 10);
        assert_eq!(chroma_strip_slot_count(640), 40);
    }

    #[test]
    fn chroma_plane_height_aligns_down_to_multiple_of_4() {
        // Spec/02 §7 item 4 — chroma height = (luma_height / 4) & -4.
        assert_eq!(chroma_plane_height(0), 0);
        assert_eq!(chroma_plane_height(4), 1 & !0x3); // = 0
        assert_eq!(chroma_plane_height(16), 4);
        assert_eq!(chroma_plane_height(17), 4); // 17/4 = 4, aligned.
        assert_eq!(chroma_plane_height(20), 5 & !0x3); // = 4
        assert_eq!(chroma_plane_height(120), 30 & !0x3); // = 28
        assert_eq!(chroma_plane_height(480), 120);
    }

    #[test]
    fn chroma_plane_width_divides_luma_by_four_no_alignment() {
        // Spec/02 §4 picture-decomposition table —
        // chroma_plane_width = luma_width / 4.
        // No multiple-of-4 alignment mask is applied (in contrast
        // to chroma_plane_height, where §7 item 4's & -0x4 mask
        // floors the height).
        assert_eq!(chroma_plane_width(0), 0);
        assert_eq!(chroma_plane_width(4), 1);
        assert_eq!(chroma_plane_width(16), 4);
        // 17 → 17/4 = 4 (integer truncation only; no mask).
        assert_eq!(chroma_plane_width(17), 4);
        // 18 → 18/4 = 4. Without the alignment mask the value
        // stays at 4 (the mask would also produce 4 here).
        assert_eq!(chroma_plane_width(18), 4);
        // 22 → 22/4 = 5. Critically not aligned to 4 — confirms
        // the helper does not apply the §7 height mask.
        assert_eq!(chroma_plane_width(22), 5);
        assert_eq!(chroma_plane_width(160), 40);
        assert_eq!(chroma_plane_width(320), 80);
        assert_eq!(chroma_plane_width(640), 160);
    }

    // ---- internal helpers -------------------------------------------

    #[test]
    fn ceil_div_matches_parser_formula() {
        // Spec/02 §4.1 — (W + (S-1)) / S = ceil(W / S).
        assert_eq!(ceil_div(1, 160), 1);
        assert_eq!(ceil_div(160, 160), 1);
        assert_eq!(ceil_div(161, 160), 2);
        assert_eq!(ceil_div(176, 160), 2);
        assert_eq!(ceil_div(320, 160), 2);
        assert_eq!(ceil_div(321, 160), 3);
    }

    #[test]
    fn last_strip_width_matches_parser_formula() {
        // Spec/02 §4.1 — ((W-1) mod S) + 1.
        assert_eq!(last_strip_width(160, 160), 160); // aligned
        assert_eq!(last_strip_width(161, 160), 1);
        assert_eq!(last_strip_width(176, 160), 16);
        assert_eq!(last_strip_width(240, 160), 80);
        assert_eq!(last_strip_width(320, 160), 160); // aligned again
    }
}
