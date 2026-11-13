//! Indeo 3 spec/05 §1 — per-plane packed-MV table layout and population.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/05-motion-compensation.md`
//! §1 (MV-table layout and population) and §1.3 (INTER-leaf indexing).
//!
//! Every plane's payload begins with a `num_vectors` u32 followed by
//! `num_vectors` `(vertical, horizontal)` signed-byte pairs
//! (`spec/02 §3.1` / §3.2, already surfaced by
//! [`super::picture_layer::PictureLayer`]). The per-plane parser
//! decodes each pair into a 32-bit packed-MV word and writes it into
//! the per-plane table at `inner_instance[0x000 + 4*i]` for
//! `i ∈ [0, num_vectors)`. The first 1024 bytes of the inner-instance
//! state therefore carry up to 256 packed-MV entries; the same arena
//! is reinterpreted by VQ_DATA leaves as the per-plane codebook-entry
//! table per `spec/04 §3.1`, with the `ecx` flag bits at the
//! downstream dispatcher (`spec/03 §3.4` / `§4.1`) disambiguating the
//! two interpretations.
//!
//! At an INTER leaf the binary-tree walker fetches one byte from the
//! bitstream (`IR32_32.DLL!0x100065f2..0x10006607`), shifts it left
//! by 2, adds the inner-instance pointer, and dereferences to fetch
//! the packed-MV word into the cell-state scratch slot at
//! `[esp + 0x44]`. The shift-by-2 / blind-index / no-bounds-check
//! disposition (§1.4) is intrinsic: a `num_vectors > 255` entry
//! would never be reachable, and a `num_vectors < 256` table is
//! never zeroed at its tail.
//!
//! This module surfaces:
//!
//! * The table base and per-entry stride within the inner-instance
//!   state ([`MV_TABLE_BASE_OFFSET`], [`MV_TABLE_ENTRY_SIZE`]).
//! * The 1024-byte / 256-entry byte-indexable bound the leaf-byte
//!   shift establishes ([`MV_TABLE_BYTES`],
//!   [`MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES`]).
//! * The per-frame-flags parser arm enumeration (§1.2 table)
//!   linking each of the four `(half-pel-horiz, half-pel-vert)`
//!   bit-pair combinations to its write-site RVA
//!   ([`MvTableParserArm`]).
//! * The per-entry byte-offset helper
//!   ([`mv_table_entry_byte_offset`]) and the validated MV-index
//!   byte → table-byte-offset accessor ([`MvIndexFetch::for_index`])
//!   modelling the INTER-leaf `shl eax, 0x2; add eax, inner_instance`
//!   sequence in §1.3.
//! * The §1.4 bound-disposition classifier ([`MvIndexValidity`])
//!   that callers can use to detect "this index addresses an entry
//!   the parser actually wrote", without the binary itself performing
//!   any such check.
//!
//! What this module **deliberately does not do** (the §1 / §2 chapter
//! boundary):
//!
//! * It does not own the bitstream input bytes. The `(vertical,
//!   horizontal)` raw-byte pairs live on
//!   [`super::picture_layer::MotionVector`]; this module's
//!   pre-population helper [`MvIndexFetch::for_index`] models the
//!   table-side read of an already-populated entry.
//! * It does not decode the packed-MV word's bit layout (§3.4 — bottom
//!   2 bits = filter mode, upper 30 bits = signed strip-pixel byte
//!   offset). That is §3's subject and stays out of scope here.
//! * It does not dispatch the four MC fetchers (§5.1 / §5.2). The
//!   four-way mode dispatch on the packed-MV's low 2 bits is §2.2's
//!   subject; here we only carry the parser-arm enumeration as a
//!   layout descriptor.
//! * It does not validate `num_vectors` against any byte-indexable
//!   bound at parse time. The binary does not; the encoder is
//!   responsible. [`MvIndexValidity`] is the read-side classifier
//!   only.
//!
//! All offsets, field widths, RVAs and bound dispositions are taken
//! from `05-motion-compensation.md` §1 (§1.1 / §1.2 / §1.3 / §1.4).
//! RVAs cited in doc-comments refer to the binary identified in
//! `spec/00 §2`.

use super::strip_context::INSTANCE_STATE_LEN;

// ---- §1.2 (table base, stride, and arena byte length) --------------

/// Spec/05 §1.2 — byte offset of the per-plane packed-MV table within
/// the 0x3010-byte inner-instance state block.
///
/// The parser writes each decoded packed-MV word at
/// `inner_instance[0x000 + 4*i]` per
/// `IR32_32.DLL!0x10004426 / 0x10004493 / 0x10004510 / 0x10004572`.
/// The table base is `0x000`, the first byte of the inner-instance
/// state itself.
pub const MV_TABLE_BASE_OFFSET: usize = 0x000;

/// Spec/05 §1.2 — per-entry stride in bytes (`4`, a 32-bit packed-MV
/// word).
///
/// Each `(vertical, horizontal)` byte pair from the bitstream is
/// scaled and packed into a single little-endian DWORD; consecutive
/// entries are 4 bytes apart per the `[ecx + 4*edx]` indexing the
/// parser uses (`edx` = loop index `i`, `ecx` = inner-instance
/// pointer).
pub const MV_TABLE_ENTRY_SIZE: usize = 4;

/// Spec/05 §1.4 — total byte length of the MV-table arena (`1024`).
///
/// The downstream dispatcher (`spec/03 §3.4` / `§4.1`) reads any of
/// `inner_instance[0x000..0x3ff]` as a packed-MV word; the leaf-byte
/// `shl eax, 0x2` (§1.3) restricts the addressable entries to
/// `MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES * MV_TABLE_ENTRY_SIZE` =
/// `0x400` = 1024 bytes. The same 1024-byte region is reinterpreted
/// as the per-plane VQ codebook-entry table on VQ_DATA leaves
/// (`spec/04 §3.1`).
pub const MV_TABLE_BYTES: usize = 0x400;

/// Spec/05 §1.3 / §1.4 — maximum number of MV-table entries reachable
/// via the one-byte INTER-leaf MV index (`256`).
///
/// The leaf-byte read at `IR32_32.DLL!0x100065f4 / 0x100065f8` loads
/// a single byte into the low 8 bits of `eax` and uses it as the
/// table index. The `shl eax, 0x2` (§1.3) scales it by
/// [`MV_TABLE_ENTRY_SIZE`]; the maximum reachable index is therefore
/// `255` and the maximum reachable byte offset within the table is
/// `255 * 4 + 3 = 1023 = MV_TABLE_BYTES - 1`. The encoder is
/// responsible (per §1.4) for keeping `num_vectors <= 256`.
pub const MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES: usize = 256;

/// Spec/05 §1.3 — left-shift amount the INTER-leaf MV-index byte is
/// scaled by before being added to the inner-instance pointer (`2`).
///
/// `shl eax, 0x2` at `IR32_32.DLL!0x100065fc` multiplies the
/// MV-index byte by the [`MV_TABLE_ENTRY_SIZE`].
pub const MV_INDEX_SCALE_SHIFT: u32 = 2;

// ---- §1.2 (parser arm enumeration) ---------------------------------

/// Spec/05 §1.2 — which of the four parser arms writes the per-plane
/// MV-table, selected by `frame_flags` bits 4 (`MV_HALFPEL_HORIZ`)
/// and 5 (`MV_HALFPEL_VERT`).
///
/// All four arms write through the same `[ecx + 4*edx]` indexing into
/// `inner_instance[0x000 + 4*i]`; they differ in the per-component
/// pre-scaling applied before packing into the 32-bit table entry
/// (full-pel pairs are stored as-is post-scale; half-pel arms apply a
/// `<<= 1` to the half-pel component before adding the strip-pixel
/// byte offset). The packing formula itself is §3's subject; this
/// enum surfaces only the arm-selection table from §1.2 so callers
/// can label a written entry with its parser-arm provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MvTableParserArm {
    /// Both half-pel bits clear (`frame_flags & 0x30 == 0x00`).
    ///
    /// Write site: `IR32_32.DLL!0x10004572`. Arm body:
    /// `0x1000451d..0x10004572`.
    FullPel,
    /// Half-pel horizontal only (`frame_flags & 0x30 == 0x10`).
    ///
    /// Write site: `IR32_32.DLL!0x10004493`. Arm body:
    /// `0x10004433..0x10004493`.
    HalfPelHorizontal,
    /// Half-pel vertical only (`frame_flags & 0x30 == 0x20`).
    ///
    /// Write site: `IR32_32.DLL!0x10004510`. Arm body:
    /// `0x100044a0..0x10004510`.
    HalfPelVertical,
    /// Both half-pel bits set (`frame_flags & 0x30 == 0x30`).
    ///
    /// Write site: `IR32_32.DLL!0x10004426`. Arm body:
    /// `0x100043bb..0x10004426`.
    HalfPelBoth,
}

/// Spec/02 §3.3 — `MV_HALFPEL_HORIZ` bit-mask within `frame_flags`
/// (`0x10`).
///
/// Re-exposed here as the dispatch input for
/// [`MvTableParserArm::from_frame_flags`].
pub const MV_HALFPEL_HORIZ: u32 = 0x10;

/// Spec/02 §3.3 — `MV_HALFPEL_VERT` bit-mask within `frame_flags`
/// (`0x20`).
///
/// Re-exposed here as the dispatch input for
/// [`MvTableParserArm::from_frame_flags`].
pub const MV_HALFPEL_VERT: u32 = 0x20;

/// Spec/05 §1.2 — combined `frame_flags` half-pel-pair mask
/// (`MV_HALFPEL_HORIZ | MV_HALFPEL_VERT` = `0x30`).
pub const MV_HALFPEL_MASK: u32 = MV_HALFPEL_HORIZ | MV_HALFPEL_VERT;

impl MvTableParserArm {
    /// Spec/05 §1.2 — pick the parser arm dispatched for a given
    /// `frame_flags` value (only bits 4 and 5 are consulted).
    ///
    /// All other bits of `frame_flags` (the §3.3 zoom / VQ-bank /
    /// inter bits etc.) are ignored; the parser-arm dispatch is
    /// purely on the half-pel pair.
    pub const fn from_frame_flags(frame_flags: u32) -> Self {
        match frame_flags & MV_HALFPEL_MASK {
            0x00 => Self::FullPel,
            MV_HALFPEL_HORIZ => Self::HalfPelHorizontal,
            MV_HALFPEL_VERT => Self::HalfPelVertical,
            _ => Self::HalfPelBoth,
        }
    }

    /// Spec/05 §1.2 — RVA of the `mov [ecx + 4*edx], eax` write site
    /// for this parser arm. Useful for cross-referencing a written
    /// entry back to the static-analysis citation.
    pub const fn write_site_rva(self) -> u32 {
        match self {
            Self::FullPel => 0x10004572,
            Self::HalfPelHorizontal => 0x10004493,
            Self::HalfPelVertical => 0x10004510,
            Self::HalfPelBoth => 0x10004426,
        }
    }

    /// Spec/05 §1.2 — true iff this arm applies the half-pel `<<= 1`
    /// to the **horizontal** displacement component before packing.
    pub const fn applies_half_pel_horizontal(self) -> bool {
        matches!(self, Self::HalfPelHorizontal | Self::HalfPelBoth)
    }

    /// Spec/05 §1.2 — true iff this arm applies the half-pel `<<= 1`
    /// to the **vertical** displacement component before packing.
    pub const fn applies_half_pel_vertical(self) -> bool {
        matches!(self, Self::HalfPelVertical | Self::HalfPelBoth)
    }
}

// ---- §1.3 (INTER-leaf indexing) ------------------------------------

/// Spec/05 §1.3 — byte offset of MV-table entry `i` within the
/// inner-instance state.
///
/// Equals `MV_TABLE_BASE_OFFSET + i * MV_TABLE_ENTRY_SIZE`. The
/// addressable indices are `[0, MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES)`;
/// indices beyond that bound are not reachable from the one-byte
/// MV-index path and would alias into other inner-instance state
/// regions (the cell-stack bases at `+0x400+`, the codebook-bank
/// pointers, …). Returns `None` for an unreachable index.
pub const fn mv_table_entry_byte_offset(index: usize) -> Option<usize> {
    if index >= MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES {
        return None;
    }
    Some(MV_TABLE_BASE_OFFSET + index * MV_TABLE_ENTRY_SIZE)
}

/// Spec/05 §1.4 — read-side classification of an INTER-leaf MV index
/// byte against a plane's `num_vectors` count.
///
/// The binary itself performs no such check (per §1.4); this enum is
/// purely a caller convenience for diagnostics and a fixture-validator
/// surface. Callers that wish to bound a well-formed stream can
/// dispatch on the returned variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MvIndexValidity {
    /// `index < num_vectors`: addresses an entry the parser actually
    /// wrote during the current frame.
    WrittenThisFrame,
    /// `num_vectors <= index < MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES`:
    /// addresses a tail entry the parser did not touch this frame.
    /// Per §1.4 the binary reads whatever residual content the prior
    /// frame left there; a well-formed bitstream never emits this
    /// path. The variant is informational only.
    StaleTailEntry,
    /// `index >= MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES`: not reachable
    /// from the one-byte MV-index path; the caller is invoking the
    /// helper with an out-of-range index. Distinguished from
    /// [`Self::StaleTailEntry`] to flag a misuse vs a stream-content
    /// disposition.
    OutOfRange,
}

impl MvIndexValidity {
    /// Spec/05 §1.4 — classify an MV-index byte against a plane's
    /// `num_vectors` count.
    pub const fn classify(index: usize, num_vectors: u32) -> Self {
        if index >= MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES {
            Self::OutOfRange
        } else if (index as u64) < (num_vectors as u64) {
            Self::WrittenThisFrame
        } else {
            Self::StaleTailEntry
        }
    }

    /// Spec/05 §1.4 — true iff the index addresses an entry the parser
    /// wrote during the current frame (the only stream-well-formed
    /// disposition).
    pub const fn is_written_this_frame(self) -> bool {
        matches!(self, Self::WrittenThisFrame)
    }
}

/// Spec/05 §1.3 — INTER-leaf fetch descriptor: the parameters the
/// `IR32_32.DLL!0x100065f2..0x10006607` sequence assembles before
/// dereferencing the table entry.
///
/// The sequence:
///
/// ```text
/// xor eax, eax            ; clear
/// mov al, [ebp]           ; mv index byte
/// inc ebp                 ; advance bitstream cursor
/// shl eax, 0x2            ; * 4
/// add eax, [esp + 0x60]   ; + inner_instance
/// or  ecx, 0x80000000     ; mark "leaf-byte present"
/// mov eax, [eax]          ; packed-MV word
/// mov [esp + 0x44], eax   ; into the cell-state scratch slot
/// ```
///
/// This descriptor surfaces the `index`, the
/// [`table_byte_offset`](Self::table_byte_offset) the table is read
/// from, and the [`validity`](Self::validity) classification —
/// everything the §1.3 sequence assembles *up to but not including*
/// the dereference itself. The dereference (loading the packed-MV
/// word) and the §3-§5 packed-MV decoding sit downstream of this
/// surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MvIndexFetch {
    /// MV index byte read from the bitstream by `mov al, [ebp]`.
    pub index: u8,
    /// Byte offset of the addressed entry within the inner-instance
    /// state, after `shl eax, 0x2`. Always
    /// `index * MV_TABLE_ENTRY_SIZE` since
    /// [`MV_TABLE_BASE_OFFSET`] is zero.
    pub table_byte_offset: usize,
    /// `frame_flags` half-pel disposition this entry was written
    /// under (the parser arm that ran during the frame's prelude).
    pub parser_arm: MvTableParserArm,
    /// §1.4 validity classification against the plane's
    /// `num_vectors` count.
    pub validity: MvIndexValidity,
}

impl MvIndexFetch {
    /// Spec/05 §1.3 — build the INTER-leaf fetch descriptor for a
    /// given MV-index byte against a plane's `num_vectors` count and
    /// the frame's parser arm.
    pub const fn for_index(index: u8, num_vectors: u32, frame_flags: u32) -> Self {
        let parser_arm = MvTableParserArm::from_frame_flags(frame_flags);
        let table_byte_offset = (index as usize) * MV_TABLE_ENTRY_SIZE;
        let validity = MvIndexValidity::classify(index as usize, num_vectors);
        Self {
            index,
            table_byte_offset,
            parser_arm,
            validity,
        }
    }

    /// Spec/05 §1.3 — true iff the addressed entry was written this
    /// frame (the only stream-well-formed disposition).
    pub const fn is_well_formed(self) -> bool {
        self.validity.is_written_this_frame()
    }
}

// ---- consistency assertions ----------------------------------------

// Spec/05 §1.2 — the MV-table arena fits within the inner-instance
// state block, exactly fills its byte-indexable bound, and matches the
// `shl eax, 0x2` per-entry-stride scaling.
const _: () = {
    assert!(MV_TABLE_BASE_OFFSET + MV_TABLE_BYTES <= INSTANCE_STATE_LEN);
    assert!(MV_TABLE_BYTES == MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES * MV_TABLE_ENTRY_SIZE);
    assert!(1usize << MV_INDEX_SCALE_SHIFT == MV_TABLE_ENTRY_SIZE);
};

#[cfg(test)]
mod tests {
    use super::*;

    // ---- §1.2 layout constants -------------------------------------

    #[test]
    fn mv_table_base_is_zero() {
        // §1.2: `inner_instance[0x000 + 4*i]`.
        assert_eq!(MV_TABLE_BASE_OFFSET, 0);
    }

    #[test]
    fn mv_table_entry_size_is_four_bytes() {
        // §1.2: `[ecx + 4*edx]` — each entry is a 32-bit packed-MV word.
        assert_eq!(MV_TABLE_ENTRY_SIZE, 4);
        // §1.3: `shl eax, 0x2` matches.
        assert_eq!(1usize << MV_INDEX_SCALE_SHIFT, MV_TABLE_ENTRY_SIZE);
    }

    #[test]
    fn mv_table_byte_capacity_is_one_kib() {
        // §1.4: arena spans `inner_instance[0x000..0x3ff]` →
        // 1024 bytes total.
        assert_eq!(MV_TABLE_BYTES, 0x400);
        assert_eq!(MV_TABLE_BYTES, 1024);
        // Capacity = max indexable entries * entry stride.
        assert_eq!(
            MV_TABLE_BYTES,
            MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES * MV_TABLE_ENTRY_SIZE
        );
    }

    #[test]
    fn mv_table_max_indexable_entries_is_256() {
        // §1.3 / §1.4: one-byte MV index → 256 reachable entries.
        assert_eq!(MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES, 256);
        assert_eq!(MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES, u8::MAX as usize + 1);
    }

    #[test]
    fn mv_table_fits_within_inner_instance_state() {
        // §1.2 anchors the arena at offset 0 of the inner-instance
        // state; the state itself is 0x3010 bytes.
        let arena_end = MV_TABLE_BASE_OFFSET + MV_TABLE_BYTES;
        assert!(arena_end <= INSTANCE_STATE_LEN);
        assert_eq!(arena_end, 0x400);
        assert_eq!(INSTANCE_STATE_LEN, 0x3010);
    }

    // ---- §1.2 parser arm dispatch ----------------------------------

    #[test]
    fn parser_arm_full_pel_when_both_clear() {
        // §1.2 row 4: `0x10` and `0x20` both clear → full-pel arm.
        assert_eq!(
            MvTableParserArm::from_frame_flags(0),
            MvTableParserArm::FullPel
        );
        assert_eq!(
            MvTableParserArm::from_frame_flags(0).write_site_rva(),
            0x10004572
        );
    }

    #[test]
    fn parser_arm_half_pel_horizontal_when_only_bit4_set() {
        // §1.2 row 2: `0x10` set, `0x20` clear → horizontal half-pel only.
        assert_eq!(
            MvTableParserArm::from_frame_flags(MV_HALFPEL_HORIZ),
            MvTableParserArm::HalfPelHorizontal
        );
        assert_eq!(
            MvTableParserArm::from_frame_flags(MV_HALFPEL_HORIZ).write_site_rva(),
            0x10004493
        );
    }

    #[test]
    fn parser_arm_half_pel_vertical_when_only_bit5_set() {
        // §1.2 row 3: `0x10` clear, `0x20` set → vertical half-pel only.
        assert_eq!(
            MvTableParserArm::from_frame_flags(MV_HALFPEL_VERT),
            MvTableParserArm::HalfPelVertical
        );
        assert_eq!(
            MvTableParserArm::from_frame_flags(MV_HALFPEL_VERT).write_site_rva(),
            0x10004510
        );
    }

    #[test]
    fn parser_arm_half_pel_both_when_both_set() {
        // §1.2 row 1: both `0x10` and `0x20` set → both half-pel.
        assert_eq!(
            MvTableParserArm::from_frame_flags(MV_HALFPEL_MASK),
            MvTableParserArm::HalfPelBoth
        );
        assert_eq!(
            MvTableParserArm::from_frame_flags(MV_HALFPEL_MASK).write_site_rva(),
            0x10004426
        );
    }

    #[test]
    fn parser_arm_ignores_bits_outside_half_pel_mask() {
        // Only bits 4 + 5 of `frame_flags` participate in arm
        // selection; other bits (zoom / VQ-bank / inter / etc.) must
        // not perturb the dispatch.
        let arm_full = MvTableParserArm::from_frame_flags(0xffff_ffcf);
        assert_eq!(arm_full, MvTableParserArm::FullPel);
        let arm_horiz = MvTableParserArm::from_frame_flags(0xffff_ffdf);
        assert_eq!(arm_horiz, MvTableParserArm::HalfPelHorizontal);
        let arm_vert = MvTableParserArm::from_frame_flags(0xffff_ffef);
        assert_eq!(arm_vert, MvTableParserArm::HalfPelVertical);
        let arm_both = MvTableParserArm::from_frame_flags(0xffff_ffff);
        assert_eq!(arm_both, MvTableParserArm::HalfPelBoth);
    }

    #[test]
    fn parser_arm_half_pel_predicates_match_arm_definition() {
        assert!(!MvTableParserArm::FullPel.applies_half_pel_horizontal());
        assert!(!MvTableParserArm::FullPel.applies_half_pel_vertical());
        assert!(MvTableParserArm::HalfPelHorizontal.applies_half_pel_horizontal());
        assert!(!MvTableParserArm::HalfPelHorizontal.applies_half_pel_vertical());
        assert!(!MvTableParserArm::HalfPelVertical.applies_half_pel_horizontal());
        assert!(MvTableParserArm::HalfPelVertical.applies_half_pel_vertical());
        assert!(MvTableParserArm::HalfPelBoth.applies_half_pel_horizontal());
        assert!(MvTableParserArm::HalfPelBoth.applies_half_pel_vertical());
    }

    #[test]
    fn write_site_rvas_are_all_distinct() {
        // §1.2 table has four distinct write-site RVAs; a duplicate
        // would mean we misread the static analysis.
        let arms = [
            MvTableParserArm::FullPel,
            MvTableParserArm::HalfPelHorizontal,
            MvTableParserArm::HalfPelVertical,
            MvTableParserArm::HalfPelBoth,
        ];
        let mut rvas: Vec<u32> = arms.iter().map(|a| a.write_site_rva()).collect();
        rvas.sort_unstable();
        rvas.dedup();
        assert_eq!(rvas.len(), 4);
    }

    // ---- §1.3 entry byte offset ------------------------------------

    #[test]
    fn entry_byte_offset_for_index_zero() {
        // §1.3: entry 0 sits at the arena's base.
        assert_eq!(mv_table_entry_byte_offset(0), Some(0));
    }

    #[test]
    fn entry_byte_offset_for_max_indexable_index() {
        // §1.3: entry 255 sits at `255 * 4` = `0x3fc` (the last
        // packed-MV slot inside the 1 KiB arena).
        assert_eq!(mv_table_entry_byte_offset(255), Some(0x3fc));
        // The four bytes of that entry exactly fill the arena tail.
        assert_eq!(0x3fc + MV_TABLE_ENTRY_SIZE, MV_TABLE_BYTES);
    }

    #[test]
    fn entry_byte_offset_rejects_unreachable_index() {
        // §1.4: index 256 would require a 9-bit MV index; the binary
        // can never reach it.
        assert_eq!(mv_table_entry_byte_offset(256), None);
        assert_eq!(mv_table_entry_byte_offset(usize::MAX), None);
    }

    #[test]
    fn entry_byte_offsets_are_dense_stride_four() {
        // §1.2 / §1.3: consecutive entries are 4 bytes apart.
        for i in 0..MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES {
            let off = mv_table_entry_byte_offset(i).unwrap();
            assert_eq!(off, i * MV_TABLE_ENTRY_SIZE);
        }
    }

    // ---- §1.4 validity classification ------------------------------

    #[test]
    fn validity_within_num_vectors_is_written_this_frame() {
        // §1.4: index < num_vectors → addresses a written entry.
        assert_eq!(
            MvIndexValidity::classify(0, 1),
            MvIndexValidity::WrittenThisFrame
        );
        assert_eq!(
            MvIndexValidity::classify(99, 100),
            MvIndexValidity::WrittenThisFrame
        );
        assert_eq!(
            MvIndexValidity::classify(255, 256),
            MvIndexValidity::WrittenThisFrame
        );
    }

    #[test]
    fn validity_at_or_past_num_vectors_is_stale_tail() {
        // §1.4: `num_vectors <= index < 256` → stale tail content.
        assert_eq!(
            MvIndexValidity::classify(1, 1),
            MvIndexValidity::StaleTailEntry
        );
        assert_eq!(
            MvIndexValidity::classify(100, 99),
            MvIndexValidity::StaleTailEntry
        );
        assert_eq!(
            MvIndexValidity::classify(255, 0),
            MvIndexValidity::StaleTailEntry
        );
    }

    #[test]
    fn validity_intra_frame_marks_every_index_as_stale_tail() {
        // §1.4: `num_vectors == 0` (INTRA frame) → every byte-indexable
        // index hits stale tail content; a well-formed INTRA stream
        // emits no INTER leaf, so this path is purely informational.
        for i in 0..MV_TABLE_MAX_BYTE_INDEXABLE_ENTRIES {
            assert_eq!(
                MvIndexValidity::classify(i, 0),
                MvIndexValidity::StaleTailEntry
            );
        }
    }

    #[test]
    fn validity_past_byte_indexable_bound_is_out_of_range() {
        // §1.4: index >= 256 is unreachable via a one-byte MV index.
        assert_eq!(
            MvIndexValidity::classify(256, 256),
            MvIndexValidity::OutOfRange
        );
        assert_eq!(
            MvIndexValidity::classify(usize::MAX, u32::MAX),
            MvIndexValidity::OutOfRange
        );
    }

    #[test]
    fn validity_classify_caps_at_byte_indexable_bound_even_for_max_num_vectors() {
        // u32 num_vectors can be > 256, but no MV-index byte can ever
        // reach those entries; the classifier still bounds by index.
        assert_eq!(
            MvIndexValidity::classify(100, u32::MAX),
            MvIndexValidity::WrittenThisFrame
        );
        assert_eq!(
            MvIndexValidity::classify(255, u32::MAX),
            MvIndexValidity::WrittenThisFrame
        );
    }

    #[test]
    fn validity_is_written_this_frame_predicate() {
        assert!(MvIndexValidity::WrittenThisFrame.is_written_this_frame());
        assert!(!MvIndexValidity::StaleTailEntry.is_written_this_frame());
        assert!(!MvIndexValidity::OutOfRange.is_written_this_frame());
    }

    // ---- §1.3 INTER-leaf fetch descriptor --------------------------

    #[test]
    fn fetch_for_index_zero_in_full_pel_frame() {
        // §1.3: index 0 → table offset 0; full-pel arm; well-formed
        // for any frame with `num_vectors >= 1`.
        let f = MvIndexFetch::for_index(0, 1, 0);
        assert_eq!(f.index, 0);
        assert_eq!(f.table_byte_offset, 0);
        assert_eq!(f.parser_arm, MvTableParserArm::FullPel);
        assert_eq!(f.validity, MvIndexValidity::WrittenThisFrame);
        assert!(f.is_well_formed());
    }

    #[test]
    fn fetch_for_max_index_in_half_pel_both_frame() {
        // §1.3: index 255 → table offset 0x3fc; both-half-pel arm;
        // well-formed only for `num_vectors == 256`.
        let f = MvIndexFetch::for_index(255, 256, MV_HALFPEL_MASK);
        assert_eq!(f.table_byte_offset, 0x3fc);
        assert_eq!(f.parser_arm, MvTableParserArm::HalfPelBoth);
        assert_eq!(f.validity, MvIndexValidity::WrittenThisFrame);
        // Confirm the addressed entry's four bytes fit within the arena.
        assert!(f.table_byte_offset + MV_TABLE_ENTRY_SIZE <= MV_TABLE_BYTES);
    }

    #[test]
    fn fetch_records_stale_tail_for_index_past_num_vectors() {
        // §1.4: index 200 with num_vectors = 100 → stale-tail read.
        let f = MvIndexFetch::for_index(200, 100, MV_HALFPEL_HORIZ);
        assert_eq!(f.table_byte_offset, 200 * MV_TABLE_ENTRY_SIZE);
        assert_eq!(f.parser_arm, MvTableParserArm::HalfPelHorizontal);
        assert_eq!(f.validity, MvIndexValidity::StaleTailEntry);
        assert!(!f.is_well_formed());
    }

    #[test]
    fn fetch_byte_offset_matches_helper_for_every_index() {
        // §1.3: the per-entry byte-offset helper and the fetch
        // descriptor must agree on the table offset for every
        // reachable index.
        for i in 0..=u8::MAX {
            let f = MvIndexFetch::for_index(i, 256, 0);
            let helper = mv_table_entry_byte_offset(i as usize).unwrap();
            assert_eq!(f.table_byte_offset, helper);
        }
    }

    #[test]
    fn fetch_parser_arm_tracks_frame_flags() {
        // §1.2 + §1.3: arm selection in the descriptor must match the
        // standalone parser-arm dispatch for the same flags.
        for flags in [
            0u32,
            MV_HALFPEL_HORIZ,
            MV_HALFPEL_VERT,
            MV_HALFPEL_MASK,
            0xdead_beef,
        ] {
            let f = MvIndexFetch::for_index(0, 0, flags);
            assert_eq!(f.parser_arm, MvTableParserArm::from_frame_flags(flags));
        }
    }
}
