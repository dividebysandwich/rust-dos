//! Indeo 3 byte-level entropy: the per-cell mode-byte stream + the
//! `0xF8..0xFF` RLE escape codes.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/06-entropy.md`.
//!
//! Round 5 lands the byte-level entropy surface that consumes the
//! spec/04 VQ codebook state. spec/06 establishes (via the §1
//! entropy-surface inventory) that Indeo 3 has exactly four
//! bitstream mechanisms and that there is **no Huffman / arithmetic
//! coder and no fixed VLC longer than the 2-bit binary-tree code**.
//! The first three are already modelled (spec/03 §2 binary-tree
//! codes, spec/03 §3.4 / spec/04 §3.1 leaf-byte indices, spec/04 §4
//! VQ_NULL prefix code — surfaced as [`super::VqNullRuntime`]). This
//! module lands the fourth: the per-cell mode-byte stream.
//!
//! What this round covers, mapped to the spec/06 sections:
//!
//! * §2.3 / §3.1 — the mode-byte vocabulary and high/low nibble
//!   split. [`ModeByte::classify`] turns a raw byte into a
//!   [`ModeByteKind`]: a literal dyad index (`0x00..0xF7`, carrying
//!   the high-nibble jump-table selector and low-nibble band /
//!   bit-3 dispatch state) or an RLE escape (`0xF8..0xFF`).
//! * §3.3 — the variable-byte continuation rule. The dyad sum's
//!   sign bit decides whether a continuation byte is consumed;
//!   [`continuation_needed`] models that decoder-driven test on the
//!   §6.4 biased-signed sum.
//! * §3.4 — the four cell-unpacker variants reached from the
//!   spec/04 §2.1 mode bits, surfaced here as
//!   [`super::CellVariant`] re-used for the position-table lookup.
//! * §4.1 / §4.2 — the eight RLE escapes [`RleEscape`] and their
//!   first-position handlers (all eight accepted at a cell start).
//! * §4.3 — the per-position acceptance matrix
//!   ([`PositionClass`] + [`RleEscape::accepted_at`]): which escapes
//!   continue versus fault at each cell / row position.
//! * §4.4 — the `0xFB` counter byte: the category-lookup table
//!   ([`fb_category_table`]) built from the spec's normative seed
//!   ranges, and the counter decomposition [`FbCounter`].
//!
//! What this round deliberately does **not** do (the spec/06 §8
//! boundary with spec/07):
//!
//! * No pixel emission. The arithmetic that combines a dyad delta
//!   with the predictor (`add eax, [esi + 4*edx + 0x400]`, the
//!   `0x7f7f7f7f` mask, the saturation / continuation pixel writes)
//!   is `spec/07`. This module answers only the entropy question:
//!   *which* bytes the stream consumes, and *how* a mode byte and an
//!   escape byte are classified.
//! * No dyad / quad table entry values — those are referenced by RVA
//!   from spec/06 but enumerated in spec/04 §7.1 (Extractor
//!   territory). [`DyadAddress`] only computes the *address* of the
//!   dyad entry from the mode byte's nibbles.
//! * No cell-stack predictor chain, no motion compensation
//!   (`spec/05`), no inter-cell edge fix-up (`spec/07`).
//!
//! The contract: given the bitstream cursor at the start of a cell's
//! mode-byte stream, this module classifies each byte (literal vs
//! escape), tells the caller how many bytes a literal mode byte
//! consumes (1 or 2 via [`continuation_needed`]), and validates an
//! escape against the current position. The pixel writes are the
//! next chapter's job.

use super::CellVariant;

/// Spec/06 §2.3 — the inclusive upper bound of a *literal* mode byte.
/// Bytes `0x00..=0xF7` are literal dyad indices.
pub const LITERAL_MODE_MAX: u8 = 0xF7;

/// Spec/06 §2.3 / §4 — the inclusive lower bound of an RLE escape
/// byte (`cmp dl, 0xF8; jb literal` at `IR32_32.DLL!0x10006bbe`).
pub const RLE_ESCAPE_MIN: u8 = 0xF8;

/// Spec/06 §3.1 — the per-frame-arena band stride a low nibble
/// selects (`shl esi, 0x0B` → low nibble × 2048).
pub const ARENA_BAND_STRIDE: usize = 2048;

/// Spec/06 §3.2 — the dyad-pair primary-table base displacement
/// `+0x400` (`add eax, [esi + 4*edx + 0x400]`).
pub const PRIMARY_TABLE_DISP: usize = 0x400;

/// Spec/06 §3.3 — the secondary-table dyad-word displacement
/// `+0x402` (`add ax, [esi + 4*edx + 0x402]`).
pub const SECONDARY_TABLE_DISP: usize = 0x402;

/// Spec/06 §3.3 / §6.1 — the high-bit XOR mask the continuation path
/// applies (`xor eax, 0x80008000`) to back out the two packed 16-bit
/// halves' sign extension before consulting the secondary table.
pub const CONTINUATION_XOR: u32 = 0x8000_8000;

/// Spec/06 §1.2 — the per-cell unpacker entry RVAs for the four
/// cell-shape variants (cited for provenance; the variant itself is
/// [`super::CellVariant`]).
pub const VARIANT_A_ENTRY: u32 = 0x1000_6bac;
/// Spec/06 §1.2 / §3.4 — variant B (with-edge averaging) entry.
pub const VARIANT_B_ENTRY: u32 = 0x1000_6fe1;
/// Spec/06 §3.4 — variant C (doubled-row) entry.
pub const VARIANT_C_ENTRY: u32 = 0x1000_818e;
/// Spec/06 §3.4 — variant D (fully-doubled) entry, reached via the
/// `0x100072bb` doubled-row handler.
pub const VARIANT_D_ENTRY: u32 = 0x1000_72bb;

/// Spec/06 §2.3 / §3.1 — the structured view of a literal mode byte
/// (`0x00..=0xF7`).
///
/// The per-cell unpacker at `IR32_32.DLL!0x10006bac` splits the byte
/// into a high nibble (bits 4-7) and a low nibble (bits 0-3). The
/// high nibble × 4 selects the entry in one of the two 16-entry jump
/// tables (§3.2); the low nibble × 2048 (`shl esi, 0x0B`) selects the
/// per-frame-arena band sub-table; bit 3 of the low nibble (`test dl,
/// 0x08`) selects which of the two jump tables is consulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiteralMode {
    /// The raw mode byte (`0x00..=0xF7`).
    pub byte: u8,
    /// High nibble (`byte >> 4`, 0..15) — the jump-table index.
    pub high_nibble: u8,
    /// Low nibble (`byte & 0x0F`, 0..15) — band selector + bit-3
    /// jump-table flavour.
    pub low_nibble: u8,
    /// `(high_nibble << 4) >> 2` = `high_nibble * 4` — the byte
    /// displacement into the selected 16-entry jump table
    /// (`sar eax, 0x2` after `and eax, 0xF0`).
    pub jump_table_offset: u8,
    /// `low_nibble << 11` = `low_nibble * 2048` — the per-frame-arena
    /// band sub-table base displacement (`shl esi, 0x0B`).
    pub arena_band_offset: usize,
    /// Bit 3 of the low nibble (`test dl, 0x08`). When set, the
    /// dispatch uses the first jump table (`0x10006bd4`); when clear,
    /// the second (`0x10006c50`). See [`JumpTable`].
    pub low_nibble_bit3: bool,
}

impl LiteralMode {
    /// Spec/06 §3.1 — split a literal mode byte into its dispatch
    /// fields. The caller must have already established that
    /// `byte <= LITERAL_MODE_MAX` (else it is an RLE escape).
    pub fn from_byte(byte: u8) -> Self {
        let high_nibble = byte >> 4;
        let low_nibble = byte & 0x0F;
        LiteralMode {
            byte,
            high_nibble,
            low_nibble,
            // `and eax, 0xF0` keeps `high_nibble << 4`; `sar eax, 2`
            // yields `high_nibble << 2` = high_nibble * 4.
            jump_table_offset: high_nibble << 2,
            arena_band_offset: (low_nibble as usize) << 11,
            low_nibble_bit3: low_nibble & 0x08 != 0,
        }
    }

    /// Spec/06 §3.1 / §3.2 — which of the two 16-entry jump tables the
    /// dispatch consults, given the low nibble's bit 3.
    pub fn jump_table(self) -> JumpTable {
        if self.low_nibble_bit3 {
            JumpTable::First
        } else {
            JumpTable::Second
        }
    }

    /// Spec/06 §3.2 — resolve this mode byte's full dispatch outcome by
    /// indexing the bit-3-selected jump table at this byte's high
    /// nibble (`jmp [4 * high_nibble + base]`). Combines
    /// [`jump_table`](Self::jump_table) (table selection) and
    /// [`JumpTable::entry`] (per-high-nibble target) into the single
    /// dispatch the per-cell unpacker performs at
    /// `IR32_32.DLL!0x10006bd4` / `0x10006c50`.
    pub fn dispatch_entry(self) -> JumpTableEntry {
        self.jump_table().entry(self.high_nibble)
    }

    /// Spec/06 §3.2 — `true` when this mode byte indexes a fault slot
    /// (target `0x10007a96` → `0x1000854b`, error code 1) of the
    /// bit-3-selected jump table. An encoder is forbidden from emitting
    /// such a byte in the variant-A flavour.
    pub fn is_fault(self) -> bool {
        self.dispatch_entry().is_fault()
    }
}

/// Spec/06 §3.2 — the two 16-entry mode-byte jump tables.
///
/// They share most entries, differing only at indices where the
/// "bit 3 of low nibble" distinction is structurally meaningful
/// (§3.1). Selection is by the low nibble's bit 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpTable {
    /// `0x10006bd4` — selected when low-nibble bit 3 is **set**
    /// (`jmp [eax + 0x10006bd4]`).
    First,
    /// `0x10006c50` — selected when low-nibble bit 3 is **clear**
    /// (the `je 0x10006c4a` fall-through path).
    Second,
}

impl JumpTable {
    /// The dispatch-table base RVA in `IR32_32.DLL`.
    pub fn base_rva(self) -> u32 {
        match self {
            JumpTable::First => 0x1000_6bd4,
            JumpTable::Second => 0x1000_6c50,
        }
    }

    /// Spec/06 §3.2 — resolve the 4-byte entry at index `high_nibble`
    /// (`[base + 4 * high_nibble]`) into a typed handler classification.
    ///
    /// The two tables are indexed identically (`sar eax, 0x2` after
    /// `and eax, 0xF0` yields `high_nibble * 4`); they differ only at
    /// the indices where the §3.1 bit-3 distinction is structurally
    /// meaningful (high nibbles `0x0`, `0x3`, `0xA`). `high_nibble`
    /// values above `0xF` saturate the 4-bit index and are masked to
    /// `0x0..=0xF` so a caller passing a raw byte's high nibble cannot
    /// run off the 16-entry table.
    pub fn entry(self, high_nibble: u8) -> JumpTableEntry {
        let hn = high_nibble & 0x0F;
        match self {
            JumpTable::First => match hn {
                0x0 => JumpTableEntry::Handler(0x1000_6c14),
                0x1 => JumpTableEntry::Handler(0x1000_6c90),
                0x2 => JumpTableEntry::Fault,
                0x3 => JumpTableEntry::Handler(0x1000_6c14),
                0x4 => JumpTableEntry::Handler(0x1000_72bb),
                0x5..=0x9 => JumpTableEntry::Fault,
                0xA => JumpTableEntry::Handler(0x1000_6c14),
                0xB => JumpTableEntry::Handler(0x1000_771c),
                0xC => JumpTableEntry::Handler(0x1000_7710),
                _ => JumpTableEntry::Fault, // 0xD..=0xF
            },
            JumpTable::Second => match hn {
                0x0 => JumpTableEntry::Handler(0x1000_6c9c),
                0x1 => JumpTableEntry::Handler(0x1000_6c90),
                0x2 => JumpTableEntry::Fault,
                0x3 => JumpTableEntry::Handler(0x1000_72c7),
                0x4 => JumpTableEntry::Handler(0x1000_72bb),
                // "various"; the per-entry targets are not enumerated
                // at the bitstream level, so we do not invent them.
                // Corrected (round 17): both tables carry the fault
                // target at every one of these five slots.
                0x5..=0x9 => JumpTableEntry::Fault,
                0xA => JumpTableEntry::Handler(0x1000_7a9b),
                0xB => JumpTableEntry::Handler(0x1000_771c),
                0xC => JumpTableEntry::Handler(0x1000_7710),
                _ => JumpTableEntry::Fault, // 0xD..=0xF
            },
        }
    }
}

/// Spec/06 §3.2 — the classification of a single 16-entry jump-table
/// slot (`[0x10006bd4 + 4N]` / `[0x10006c50 + 4N]`).
///
/// The slot holds a code address; this enum records what category of
/// handler that address belongs to at the bitstream level. The
/// per-pixel handler bodies are spec/07's subject — this only pins
/// the dispatch outcome (accept-and-which-handler / fault / not-pinned)
/// so a caller routing a mode byte knows which slots an encoder is
/// forbidden from indexing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JumpTableEntry {
    /// A real handler at the given `IR32_32.DLL` RVA. The mode byte is
    /// accepted; the handler emits cell pixels (spec/07).
    Handler(u32),
    /// The slot points at the fault handler `0x10007a96` →
    /// `0x1000854b`, which returns error code 1. An encoder is
    /// forbidden from emitting a mode byte that indexes here for the
    /// variant-A flavour.
    Fault,
}

impl JumpTableEntry {
    /// `true` if the slot routes to the fault handler (`0x10007a96`).
    pub fn is_fault(self) -> bool {
        matches!(self, JumpTableEntry::Fault)
    }

    /// The handler RVA, when the slot is a real handler.
    pub fn handler_rva(self) -> Option<u32> {
        match self {
            JumpTableEntry::Handler(rva) => Some(rva),
            _ => None,
        }
    }
}

/// Spec/06 §3.1 — the four categories of high-nibble behaviour
/// (the §3.1 "High nibble / Action" table), as a bitstream-level
/// selector. The actual per-pixel handler bodies are spec/07; this
/// only records what the nibble selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighNibbleAction {
    /// High nibble `0x0` — dyad-pair from the per-frame primary table
    /// (the low nibble × 2048 selects the band). The common
    /// INTRA-frame VQ payload case (§3.1).
    DyadPrimary,
    /// High nibble `0x1` — single-pixel fill via the `ch`-counter
    /// "skip current pair" mechanism (§3.1).
    SinglePixelFill,
    /// High nibble `0x2..=0xF` — other per-mode behaviours (QUAD,
    /// doubled-row, row-band advance) whose detailed semantics are
    /// spec/07 (§3.1 / §3.2). Several of these slots are faults; for
    /// the precise per-(table, high-nibble) dispatch outcome including
    /// fault detection, use [`LiteralMode::dispatch_entry`] /
    /// [`JumpTable::entry`] rather than this coarse §3.1 category.
    Other,
}

impl HighNibbleAction {
    /// Spec/06 §3.1 — classify the high nibble.
    pub fn from_high_nibble(high_nibble: u8) -> Self {
        match high_nibble {
            0x0 => HighNibbleAction::DyadPrimary,
            0x1 => HighNibbleAction::SinglePixelFill,
            _ => HighNibbleAction::Other,
        }
    }
}

/// Spec/06 §2.3 / §3.1 / §4 — the classification of a single
/// mode byte read from the bitstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeByteKind {
    /// `0x00..=0xF7` — a literal dyad index (§2.3 / §3.1).
    Literal(LiteralMode),
    /// `0xF8..=0xFF` — an RLE escape (§2.3 / §4).
    Escape(RleEscape),
}

/// Spec/06 §3 — a classified mode byte plus convenience accessors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeByte {
    /// The raw byte.
    pub byte: u8,
    /// Its classification.
    pub kind: ModeByteKind,
}

impl ModeByte {
    /// Spec/06 §2.3 / §3.1 — classify a mode byte (`cmp dl, 0xF8;
    /// jb literal` at `IR32_32.DLL!0x10006bbe`).
    pub fn classify(byte: u8) -> Self {
        let kind = if byte >= RLE_ESCAPE_MIN {
            ModeByteKind::Escape(
                RleEscape::from_byte(byte).expect("byte >= 0xF8 is a valid escape"),
            )
        } else {
            ModeByteKind::Literal(LiteralMode::from_byte(byte))
        };
        ModeByte { byte, kind }
    }

    /// `true` if the byte is a literal dyad index (`0x00..=0xF7`).
    pub fn is_literal(self) -> bool {
        matches!(self.kind, ModeByteKind::Literal(_))
    }

    /// `true` if the byte is an RLE escape (`0xF8..=0xFF`).
    pub fn is_escape(self) -> bool {
        matches!(self.kind, ModeByteKind::Escape(_))
    }
}

/// Spec/06 §4 — the eight RLE escape codes (`0xF8..=0xFF`).
///
/// The wiki names (`wiki/Indeo_3.wiki` §"Run-length codes") are
/// recorded on each variant's doc-comment for orientation. The
/// per-handler behaviour summarised here is the §4.2 description; the
/// pixel-buffer side effects (edge-bit setting, row strides) are
/// spec/07.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RleEscape {
    /// `0xF8` (RLE_ESC_F8) — edge-marker for the cell; sets bit 7 of
    /// pixel-buffer bytes (`0x100085C3`). The wiki notes it "seems
    /// to be never used" — encoders prefer the more compact VQ_NULL
    /// "01" sub-code (§4.5).
    F8,
    /// `0xF9` (RLE_ESC_F9) — toggles `ecx` bit 16 (half-pel-skip
    /// flag) then marks-and-skips (`0x10006cec`). The wiki's
    /// "this block and the next one" two-cell variant (§4.6).
    F9,
    /// `0xFA` (RLE_ESC_FA) — walks the cell setting bit 7 per row,
    /// decrementing the row counter (`0x10006cf2`).
    Fa,
    /// `0xFB` (RLE_ESC_FB) — reads a counter byte at `[ebp + 1]`,
    /// advances `ebp` by 2, looks the counter up in the §4.4
    /// category table, and skips `(counter & 0x1F) + 1` cells
    /// (copy or mark-skipped per bit 5). The only escape that
    /// consumes an extra bitstream byte (`0x10006d14`).
    Fb,
    /// `0xFC` (RLE_ESC_FC) — toggles `ecx` bit 16 and skips the rest
    /// of this cell *and* the next cell (`0x10006ddc`).
    Fc,
    /// `0xFD` (RLE_ESC_FD) — skips all remaining rows of the current
    /// cell (advances `edi`, decrements the row counter to
    /// termination) (`0x10006dea`).
    Fd,
    /// `0xFE` (RLE_ESC_FE) — skips lines 1 and 2 of this block:
    /// advances `edi` by two row strides without emitting deltas
    /// (`0x10006f80`).
    Fe,
    /// `0xFF` (RLE_ESC_FF) — skips line 1 of this block: advances
    /// `edi` by one row stride (`0x10006f02`).
    Ff,
}

impl RleEscape {
    /// Spec/06 §4 — classify an escape byte. Returns `None` for any
    /// byte below [`RLE_ESCAPE_MIN`] (i.e. a literal mode byte).
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0xF8 => Some(RleEscape::F8),
            0xF9 => Some(RleEscape::F9),
            0xFA => Some(RleEscape::Fa),
            0xFB => Some(RleEscape::Fb),
            0xFC => Some(RleEscape::Fc),
            0xFD => Some(RleEscape::Fd),
            0xFE => Some(RleEscape::Fe),
            0xFF => Some(RleEscape::Ff),
            _ => None,
        }
    }

    /// The escape byte value (`0xF8..=0xFF`).
    pub fn byte(self) -> u8 {
        match self {
            RleEscape::F8 => 0xF8,
            RleEscape::F9 => 0xF9,
            RleEscape::Fa => 0xFA,
            RleEscape::Fb => 0xFB,
            RleEscape::Fc => 0xFC,
            RleEscape::Fd => 0xFD,
            RleEscape::Fe => 0xFE,
            RleEscape::Ff => 0xFF,
        }
    }

    /// Spec/06 §4.4 — how many extra bitstream bytes the escape
    /// consumes *beyond* the escape byte itself. Only `0xFB` reads a
    /// counter byte (`mov dl, [ebp + 1]; add ebp, 2`); every other
    /// escape advances `ebp` by 1 (the escape byte alone).
    pub fn extra_bytes(self) -> usize {
        match self {
            RleEscape::Fb => 1,
            _ => 0,
        }
    }

    /// Spec/06 §4.3 — is this escape accepted at the given position
    /// within the cell, or does it fault to error code 1?
    ///
    /// The acceptance matrix (computed from the §4.3 position-anchored
    /// dispatch tables):
    ///
    /// * `0xFB`, `0xFC`, `0xFD` — accepted at **every** position
    ///   (they imply "stop emitting deltas in this cell").
    /// * `0xFE`, `0xFF` — "skip N rows" codes accepted only at
    ///   row-start positions; they narrow as the row advances.
    /// * `0xF8`, `0xF9`, `0xFA` — start-of-cell-only; accepted only
    ///   at the first position of a cell.
    ///
    /// The §4.3 narrowing is per *continuation index* within a row:
    /// `0xFE` survives through continuation 1, `0xFF` faults beyond
    /// the first position, and `0xFD` survives one continuation
    /// further than `0xFE`. See [`PositionClass`].
    pub fn accepted_at(self, position: PositionClass) -> bool {
        match position {
            // First position of a cell (row 0 or row 1 first): all
            // eight escapes are accepted (§4.3 row "first").
            PositionClass::CellFirst | PositionClass::RowFirst => true,
            // First continuation (§4.3 "continuation 1"): the
            // F/F/F/C/C/C/C/F pattern — F8/F9/FA fault, FB/FC/FD/FE
            // continue, FF faults.
            PositionClass::Continuation1 => matches!(
                self,
                RleEscape::Fb | RleEscape::Fc | RleEscape::Fd | RleEscape::Fe
            ),
            // Second continuation (§4.3 "continuation 2"): FE now
            // faults too — F/F/F/C/C/C/F/F.
            PositionClass::Continuation2 => {
                matches!(self, RleEscape::Fb | RleEscape::Fc | RleEscape::Fd)
            }
            // Third continuation (§4.3 "continuation 3"): FD also
            // faults — only FB/FC survive — F/F/F/C/C/F/F/F.
            PositionClass::Continuation3 => matches!(self, RleEscape::Fb | RleEscape::Fc),
        }
    }
}

/// Spec/06 §4.3 — the position class within a cell that selects which
/// per-position escape dispatch table is consulted.
///
/// The original decoder has a distinct dispatch table per (variant,
/// row, continuation) tuple (§4.1 lists 24 tables), but the
/// *acceptance pattern* is shared: every "first position" accepts all
/// eight escapes, and the pattern narrows identically across
/// continuations regardless of variant or row (§4.3 / §7.6). This
/// enum collapses the position to the acceptance class that drives
/// [`RleEscape::accepted_at`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionClass {
    /// The very first position of a cell (variant A row 0 first,
    /// dispatch base `0x100068ec`). All eight escapes accepted.
    CellFirst,
    /// The first position of a non-initial row (variant A row 1
    /// first, base `0x10006c18`). All eight escapes accepted.
    RowFirst,
    /// First continuation within a row (base `0x10006a60` /
    /// `0x10006c18`-row equivalents). F8/F9/FA and FF fault.
    Continuation1,
    /// Second continuation within a row (base `0x10006adc`). FE also
    /// faults.
    Continuation2,
    /// Third continuation within a row (base `0x10006b64`). FD also
    /// faults; only FB/FC survive.
    Continuation3,
}

impl PositionClass {
    /// Spec/06 §4.3 — the variant-A row-0 dispatch-base RVA for this
    /// position class (cited for provenance). Row-1 and the B/C/D
    /// variants have their own bases (§4.1) but the same acceptance
    /// pattern.
    pub fn variant_a_row0_base_rva(self) -> u32 {
        match self {
            PositionClass::CellFirst => 0x1000_68ec,
            PositionClass::RowFirst => 0x1000_6c18,
            PositionClass::Continuation1 => 0x1000_6a60,
            PositionClass::Continuation2 => 0x1000_6adc,
            PositionClass::Continuation3 => 0x1000_6b64,
        }
    }
}

/// Spec/06 §3.2 — the per-frame-arena dyad-pair address a literal
/// mode byte selects, before the dyad value is looked up.
///
/// The dyad-pair DWORD lives at `esi + 4*edx + 0x400` where `esi =
/// low_nibble << 11` (the band base) and `edx` is the per-row dyad
/// column index. The continuation word is at `+0x402` in the
/// secondary table. This type computes those byte offsets relative to
/// the band base; the dyad *value* lookup (against the spec/04 arena)
/// and the pixel write are spec/07.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DyadAddress {
    /// The arena band base offset (`low_nibble << 11`).
    pub band_base: usize,
    /// The primary-table dyad DWORD offset
    /// (`band_base + 4*col + 0x400`).
    pub primary_offset: usize,
    /// The secondary-table dyad word offset
    /// (`band_base + 4*col + 0x402`), consulted only on a
    /// continuation (§3.3).
    pub secondary_offset: usize,
}

impl DyadAddress {
    /// Spec/06 §3.2 / §3.3 — compute the dyad-pair addresses for a
    /// literal mode byte at dyad column `col` within the row.
    pub fn new(mode: LiteralMode, col: usize) -> Self {
        let band_base = mode.arena_band_offset;
        let primary_offset = band_base + 4 * col + PRIMARY_TABLE_DISP;
        let secondary_offset = band_base + 4 * col + SECONDARY_TABLE_DISP;
        DyadAddress {
            band_base,
            primary_offset,
            secondary_offset,
        }
    }
}

/// Spec/06 §3.3 / §6.4 — the variable-byte continuation test.
///
/// After a literal mode byte's primary-table dyad is added to the
/// predictor (`add eax, [esi + 4*edx + 0x400]`), the decoder checks
/// the sum's sign bit (`jns done`). If the high bit of the 32-bit sum
/// is set, the primary table could not represent the delta in one
/// byte and a continuation byte is consumed from `[ebp + 1]` (§3.3).
///
/// `dyad_sum` is the post-add 32-bit value (predictor + primary-table
/// dyad). Returns `true` when a continuation byte must be read. This
/// is the decoder-driven half of the variable-byte encoding (the
/// encoder's choice is §7.7, out of scope for the decoder).
pub fn continuation_needed(dyad_sum: u32) -> bool {
    // `jns` is taken (no continuation) when the sign bit (bit 31) is
    // clear; the continuation is read when bit 31 is set.
    dyad_sum & 0x8000_0000 != 0
}

/// Spec/06 §3.3 — apply the continuation high-bit XOR
/// (`xor eax, 0x80008000`) that backs out the primary-table sign
/// extension before the secondary-table word is added.
pub fn apply_continuation_xor(dyad_sum: u32) -> u32 {
    dyad_sum ^ CONTINUATION_XOR
}

/// Spec/06 §1.2 / §3.3 — the per-row continuation-byte lookahead
/// offset.
///
/// When a literal mode byte's primary-table dyad overflows
/// ([`continuation_needed`] is `true`), the decoder reads the
/// continuation byte at a fixed **positive** offset from the
/// bitstream cursor `ebp` — *not* at `[ebp]`. The offset depends on
/// which row of the cell is being emitted, because the per-row
/// continuation read sites in the binary load their second byte at a
/// row-specific displacement:
///
/// | Row index | Read site (variant B row-1 tail, §1.2) | Offset |
/// | --------- | -------------------------------------- | ------ |
/// | 0         | `IR32_32.DLL!0x10006e18`               | `+1`   |
/// | 1         | `IR32_32.DLL!0x10006e91`               | `+2`   |
/// | 2         | `IR32_32.DLL!0x10006f17`               | `+3`   |
/// | 3         | `IR32_32.DLL!0x10006f98`               | `+4`   |
///
/// The §3.3 wording: "the per-row continuation reads its second byte
/// at a fixed positive offset from `ebp` depending on which row of the
/// cell is being emitted (row 0: `+1`; row 1: `+2`; row 2: `+3`;
/// row 3: `+4`)". This is the `mov dl, [ebp + N]` displacement; the
/// cursor itself advances by one extra byte (`inc ebp`) per
/// continuation consumed (§3.3), so the displacement equals
/// `row_index + 1` — one more than the number of `inc ebp` advances
/// the earlier rows of the same dyad-pair have already issued.
///
/// A cell has at most four rows (`spec/03 §2.4`), so the offset is
/// bounded by [`MAX_ROW_LOOKAHEAD_OFFSET`] (`= 4`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowLookahead {
    /// The 0-based row index within the cell (`0..=3`).
    pub row_index: usize,
    /// The `[ebp + N]` displacement at which the continuation byte for
    /// this row is read (`row_index + 1`).
    pub continuation_offset: usize,
    /// The §1.2 read-site RVA for this row's continuation lookahead
    /// (cited for provenance).
    pub read_site_rva: u32,
}

/// Spec/06 §1.2 / §3.3 — the maximum per-row continuation lookahead
/// offset (`+4`, the row-3 displacement). A cell has at most four rows
/// (`spec/03 §2.4`).
pub const MAX_ROW_LOOKAHEAD_OFFSET: usize = 4;

impl RowLookahead {
    /// Spec/06 §1.2 / §3.3 — resolve the continuation-byte lookahead
    /// for the given 0-based cell row index. Returns `None` for a row
    /// index `>= 4` (no cell exceeds four rows; `spec/03 §2.4`).
    pub fn for_row(row_index: usize) -> Option<Self> {
        let read_site_rva = match row_index {
            0 => 0x1000_6e18,
            1 => 0x1000_6e91,
            2 => 0x1000_6f17,
            3 => 0x1000_6f98,
            _ => return None,
        };
        Some(RowLookahead {
            row_index,
            continuation_offset: row_index + 1,
            read_site_rva,
        })
    }
}

/// Spec/06 §4.4 — the per-byte category looked up for a `0xFB`
/// counter byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FbCategory {
    /// Category `0x00` — counters `0x00`, `0x20`, `0x40..=0xFF`.
    /// **Invalid** (corrected, round 17): the handler at
    /// `0x10006d34` jumps directly to the plane decoder's error exit
    /// at `0x1000854b`, which returns error code 1 — the binary does
    /// not tolerate these counters.
    Zero,
    /// Category `0x04` — counter `0x01..=0x1F` (bit 5 clear): a run
    /// of `counter` dyad positions, each repeating the pixel row
    /// above it. Handler `0x10006d39`.
    Copy,
    /// Category `0x08` — counter `0x21..=0x3F` (bit 5 set): a run of
    /// `counter - 0x20` dyad positions, each edge-marked (bit 7 set)
    /// rather than written. Handler `0x10006d97`.
    MarkSkipped,
}

impl FbCategory {
    /// Spec/06 §4.4 — the category byte value (`0x00` / `0x04` /
    /// `0x08`) the table holds.
    pub fn value(self) -> u8 {
        match self {
            FbCategory::Zero => 0x00,
            FbCategory::Copy => 0x04,
            FbCategory::MarkSkipped => 0x08,
        }
    }

    /// Spec/06 §4.4 — the 3-way dispatch handler RVA for this
    /// category (cited for provenance).
    pub fn handler_rva(self) -> u32 {
        match self {
            FbCategory::Zero => 0x1000_6d34,
            FbCategory::Copy => 0x1000_6d39,
            FbCategory::MarkSkipped => 0x1000_6d97,
        }
    }
}

/// Spec/06 §4.4 — build the 256-byte `0xFB` counter-byte category
/// table.
///
/// The table at `.data + 0x1004ccd4` is a heap-resident copy made at
/// codec attach time (`IR32_32.DLL!0x100060e6`) from the static seed
/// at `.data + 0x1003ef4c`. The destination region is all-zero on
/// disk (confirmed by `tables/region_1004ccd4.meta`), so the table is
/// reconstructed from the spec's normative seed ranges (§4.4,
/// audit-confirmed against the on-disk source bytes):
///
/// | Counter byte range | Category value |
/// | ------------------ | -------------- |
/// | `0x00`             | `0x00`         |
/// | `0x01..=0x1F`      | `0x04`         |
/// | `0x20`             | `0x00`         |
/// | `0x21..=0x3F`      | `0x08`         |
/// | `0x40..=0xFF`      | `0x00`         |
///
/// Because the ranges are normative spec text, the table is built
/// from them rather than vendored as a (all-zero on disk) binary
/// region.
pub fn fb_category_table() -> [u8; 256] {
    let mut table = [0u8; 256];
    for (counter, slot) in table.iter_mut().enumerate() {
        *slot = fb_category(counter as u8).value();
    }
    table
}

/// Spec/06 §4.4 — classify a single `0xFB` counter byte into its
/// category directly from the normative seed ranges.
pub fn fb_category(counter: u8) -> FbCategory {
    match counter {
        0x00 => FbCategory::Zero,
        0x01..=0x1F => FbCategory::Copy,
        0x20 => FbCategory::Zero,
        0x21..=0x3F => FbCategory::MarkSkipped,
        // 0x40..=0xFF reserved → category 0 (§4.4).
        _ => FbCategory::Zero,
    }
}

/// Spec/06 §4.4 (corrected, round 17) — the decomposition of a
/// `0xFB` counter byte.
///
/// * bits 0..4 (`counter & 0x1F`) — the **run length in emitted dyad
///   positions**, `1..=31`, with **no off-by-one**: each category's
///   handler decrements a saved copy of the counter and leaves the
///   run on the first negative result.
/// * bit 5 (`counter & 0x20`) — the disposition: clear = each
///   position repeats the pixel row above it (category `0x04`), set
///   = each position is edge-marked instead of written (category
///   `0x08`, which strips bit 5 by subtracting `0x20` first).
/// * bits 6..7 — **must be zero**: a counter of `0x40..=0xFF` maps to
///   category `0x00` and is rejected (the binary does not tolerate
///   it), as are the zero-run-length counters `0x00` / `0x20`.
///
/// Both runs are bounded by the cell as well as by the counter: a
/// counter that has not reached zero when the cell's rows are
/// exhausted diverts to the shared cell-completion paths rather than
/// returning to the per-cell mode-byte loop (§4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FbCounter {
    /// The raw counter byte.
    pub byte: u8,
    /// `counter & 0x1F` — the run length in emitted dyad positions
    /// (exact; the earlier `+1` reading is withdrawn). Zero only for
    /// the invalid counters `0x00` / `0x20`.
    pub run_length: u8,
    /// `counter & 0x20 == 0` — each run position repeats the row
    /// above (`true`, category `0x04`) versus is edge-marked
    /// (`false`, category `0x08`).
    pub repeats_row_above: bool,
    /// The category the §4.4 table maps this counter to.
    pub category: FbCategory,
}

impl FbCounter {
    /// Spec/06 §4.4 — decompose a `0xFB` counter byte.
    pub fn decode(byte: u8) -> Self {
        FbCounter {
            byte,
            run_length: byte & 0x1F,
            repeats_row_above: byte & 0x20 == 0,
            category: fb_category(byte),
        }
    }

    /// `false` for the counters the binary rejects (`0x00`, `0x20`,
    /// and anything with bits 6..7 set — all category `0x00`, the
    /// plane decoder's error-code-1 return).
    pub fn is_valid(self) -> bool {
        self.category != FbCategory::Zero
    }
}

/// Spec/06 §3.2 (round 17) — which codebook base a mode-byte
/// handler's prologue selects for the cell: the per-frame arena band
/// (`spec/04 §1.2`) or the `cb_offset`-biased staging-image pointer
/// held in the arena's `+0x8000` slot. This is the choice `spec/04
/// §2.1` describes as `base + offset` versus `static_base + offset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodebookBase {
    /// The per-frame arena band selected by the mode byte's low
    /// nibble.
    ArenaBand,
    /// The `cb_offset`-biased codebook staging image
    /// ([`super::StagingImage`]).
    StagingImage,
}

/// Spec/06 §3.2 (round 17) — the non-canonical handler prologues.
/// The seven non-fault targets that are not the canonical dyad path
/// are not distinct unpackers: each is a short prologue that (1)
/// charges the cell's row budget (high nibbles `0x0`/`0x1` charge
/// one row; `0x3`/`0x4`/`0xA`/`0xB`/`0xC` charge two) and (2)
/// selects the codebook base, before joining a shared body. Returns
/// `(row_charge, base)` for a pinned prologue RVA, `None` for RVAs
/// outside the table. (The shared bodies themselves remain unstaged;
/// the `0x72xx` bodies additionally require a cell-shape state bit
/// clear and the `0x77xx` bodies require it set, and `0x10007a9b`
/// sets a further state bit of its own.)
pub fn handler_prologue(rva: u32) -> Option<(u8, CodebookBase)> {
    Some(match rva {
        0x1000_6c90 => (1, CodebookBase::ArenaBand),
        0x1000_6c9c => (1, CodebookBase::StagingImage),
        0x1000_72bb => (2, CodebookBase::ArenaBand),
        0x1000_72c7 => (2, CodebookBase::StagingImage),
        0x1000_7710 => (2, CodebookBase::ArenaBand),
        0x1000_771c => (2, CodebookBase::StagingImage),
        0x1000_7a9b => (2, CodebookBase::StagingImage),
        _ => return None,
    })
}

/// Spec/06 §3.4 — the per-cell unpacker entry RVA for a cell-shape
/// variant (cited for provenance; cross-references the spec/04 §2.1
/// mode bits surfaced as [`super::CellVariant`]).
pub fn variant_entry_rva(variant: CellVariant) -> u32 {
    match variant {
        CellVariant::Plain => VARIANT_A_ENTRY,
        CellVariant::WithEdge => VARIANT_B_ENTRY,
        CellVariant::DoubledRow => VARIANT_C_ENTRY,
        CellVariant::FullyDoubled => VARIANT_D_ENTRY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_vs_escape_boundary() {
        // 0xF7 is the last literal; 0xF8 is the first escape (§2.3).
        assert!(ModeByte::classify(0xF7).is_literal());
        assert!(ModeByte::classify(0xF8).is_escape());
        assert!(ModeByte::classify(0x00).is_literal());
        assert!(ModeByte::classify(0xFF).is_escape());
        assert_eq!(LITERAL_MODE_MAX, 0xF7);
        assert_eq!(RLE_ESCAPE_MIN, 0xF8);
    }

    #[test]
    fn literal_mode_nibble_split() {
        // 0x35: high nibble 3, low nibble 5.
        let m = LiteralMode::from_byte(0x35);
        assert_eq!(m.high_nibble, 3);
        assert_eq!(m.low_nibble, 5);
        // jump_table_offset = high_nibble * 4 = 12.
        assert_eq!(m.jump_table_offset, 12);
        // arena_band_offset = low_nibble << 11 = 5 * 2048 = 10240.
        assert_eq!(m.arena_band_offset, 5 * ARENA_BAND_STRIDE);
        // low nibble 5 = 0b0101: bit 3 clear.
        assert!(!m.low_nibble_bit3);
        assert_eq!(m.jump_table(), JumpTable::Second);
    }

    #[test]
    fn literal_mode_bit3_selects_first_table() {
        // 0x08: low nibble 8 = 0b1000, bit 3 set.
        let m = LiteralMode::from_byte(0x08);
        assert!(m.low_nibble_bit3);
        assert_eq!(m.jump_table(), JumpTable::First);
        assert_eq!(m.jump_table().base_rva(), 0x1000_6bd4);
        // 0x07: bit 3 clear.
        let m = LiteralMode::from_byte(0x07);
        assert!(!m.low_nibble_bit3);
        assert_eq!(m.jump_table(), JumpTable::Second);
        assert_eq!(m.jump_table().base_rva(), 0x1000_6c50);
    }

    #[test]
    fn high_nibble_action_categories() {
        assert_eq!(
            HighNibbleAction::from_high_nibble(0x0),
            HighNibbleAction::DyadPrimary
        );
        assert_eq!(
            HighNibbleAction::from_high_nibble(0x1),
            HighNibbleAction::SinglePixelFill
        );
        for hn in 0x2..=0xF {
            assert_eq!(
                HighNibbleAction::from_high_nibble(hn),
                HighNibbleAction::Other
            );
        }
    }

    #[test]
    fn first_jump_table_entries_match_spec_3_2() {
        // §3.2 column "Entry at 0x10006bd4 + 4N".
        use JumpTableEntry::*;
        let t = JumpTable::First;
        assert_eq!(t.entry(0x0), Handler(0x1000_6c14));
        assert_eq!(t.entry(0x1), Handler(0x1000_6c90));
        assert_eq!(t.entry(0x2), Fault);
        assert_eq!(t.entry(0x3), Handler(0x1000_6c14));
        assert_eq!(t.entry(0x4), Handler(0x1000_72bb));
        for hn in 0x5..=0x9 {
            assert_eq!(t.entry(hn), Fault, "first[{hn:#x}]");
        }
        assert_eq!(t.entry(0xA), Handler(0x1000_6c14));
        assert_eq!(t.entry(0xB), Handler(0x1000_771c));
        assert_eq!(t.entry(0xC), Handler(0x1000_7710));
        for hn in 0xD..=0xF {
            assert_eq!(t.entry(hn), Fault, "first[{hn:#x}]");
        }
    }

    #[test]
    fn second_jump_table_entries_match_spec_3_2() {
        // §3.2 column "Entry at 0x10006c50 + 4N".
        use JumpTableEntry::*;
        let t = JumpTable::Second;
        assert_eq!(t.entry(0x0), Handler(0x1000_6c9c));
        assert_eq!(t.entry(0x1), Handler(0x1000_6c90));
        assert_eq!(t.entry(0x2), Fault);
        assert_eq!(t.entry(0x3), Handler(0x1000_72c7));
        assert_eq!(t.entry(0x4), Handler(0x1000_72bb));
        // Corrected (round 17): the second table's 0x5..=0x9 slots
        // carry the fault target too — "various" is withdrawn.
        for hn in 0x5..=0x9 {
            assert_eq!(t.entry(hn), Fault, "second[{hn:#x}]");
        }
        assert_eq!(t.entry(0xA), Handler(0x1000_7a9b));
        assert_eq!(t.entry(0xB), Handler(0x1000_771c));
        assert_eq!(t.entry(0xC), Handler(0x1000_7710));
        for hn in 0xD..=0xF {
            assert_eq!(t.entry(hn), Fault, "second[{hn:#x}]");
        }
    }

    #[test]
    fn jump_table_entry_index_masks_to_four_bits() {
        // A caller passing a raw nibble above 0xF (defensive) must not
        // run off the 16-entry table: 0x10 wraps to index 0x0.
        assert_eq!(JumpTable::First.entry(0x10), JumpTable::First.entry(0x0));
        assert_eq!(JumpTable::Second.entry(0x1F), JumpTable::Second.entry(0xF));
    }

    #[test]
    fn shared_entries_identical_across_both_tables() {
        // §3.2: high nibbles 0x1, 0x2, 0x4, 0xB, 0xC, and the
        // 0xD..=0xF tail resolve identically in both tables.
        for hn in [0x1u8, 0x2, 0x4, 0xB, 0xC, 0xD, 0xE, 0xF] {
            assert_eq!(
                JumpTable::First.entry(hn),
                JumpTable::Second.entry(hn),
                "shared[{hn:#x}]"
            );
        }
    }

    #[test]
    fn divergent_entries_differ_per_bit3() {
        // §3.2: high nibbles 0x0, 0x3, 0xA diverge between the tables.
        for hn in [0x0u8, 0x3, 0xA] {
            assert_ne!(
                JumpTable::First.entry(hn),
                JumpTable::Second.entry(hn),
                "divergent[{hn:#x}]"
            );
        }
    }

    #[test]
    fn dispatch_entry_combines_bit3_and_high_nibble() {
        // 0x08: high nibble 0, low nibble 8 (bit 3 set) → First table,
        // index 0 → 0x10006c14.
        assert_eq!(
            LiteralMode::from_byte(0x08).dispatch_entry(),
            JumpTableEntry::Handler(0x1000_6c14)
        );
        // 0x00: high nibble 0, low nibble 0 (bit 3 clear) → Second
        // table, index 0 → 0x10006c9c.
        assert_eq!(
            LiteralMode::from_byte(0x00).dispatch_entry(),
            JumpTableEntry::Handler(0x1000_6c9c)
        );
        // 0x20: high nibble 2 → fault in both flavours.
        assert!(LiteralMode::from_byte(0x20).is_fault());
        assert!(LiteralMode::from_byte(0x28).is_fault());
    }

    #[test]
    fn jump_table_entry_accessors() {
        assert!(JumpTableEntry::Fault.is_fault());
        assert!(!JumpTableEntry::Handler(0x1000_6c14).is_fault());
        assert_eq!(
            JumpTableEntry::Handler(0x1000_6c14).handler_rva(),
            Some(0x1000_6c14)
        );
        assert_eq!(JumpTableEntry::Fault.handler_rva(), None);
    }

    #[test]
    fn classify_literal_carries_mode() {
        match ModeByte::classify(0x0A).kind {
            ModeByteKind::Literal(m) => {
                assert_eq!(m.byte, 0x0A);
                assert_eq!(m.high_nibble, 0);
                assert_eq!(m.low_nibble, 0xA);
            }
            ModeByteKind::Escape(_) => panic!("0x0A must be literal"),
        }
    }

    #[test]
    fn all_eight_escapes_round_trip() {
        for byte in 0xF8..=0xFF {
            let e = RleEscape::from_byte(byte).unwrap();
            assert_eq!(e.byte(), byte);
        }
        assert_eq!(RleEscape::from_byte(0xF7), None);
    }

    #[test]
    fn only_fb_consumes_extra_byte() {
        assert_eq!(RleEscape::Fb.extra_bytes(), 1);
        for byte in 0xF8..=0xFF {
            let e = RleEscape::from_byte(byte).unwrap();
            if e == RleEscape::Fb {
                assert_eq!(e.extra_bytes(), 1);
            } else {
                assert_eq!(e.extra_bytes(), 0);
            }
        }
    }

    #[test]
    fn first_position_accepts_all_escapes() {
        for byte in 0xF8..=0xFF {
            let e = RleEscape::from_byte(byte).unwrap();
            assert!(e.accepted_at(PositionClass::CellFirst), "{byte:#x}");
            assert!(e.accepted_at(PositionClass::RowFirst), "{byte:#x}");
        }
    }

    #[test]
    fn continuation1_acceptance_matrix() {
        // §4.3 row "continuation 1": F/F/F/C/C/C/C/F.
        let p = PositionClass::Continuation1;
        assert!(!RleEscape::F8.accepted_at(p));
        assert!(!RleEscape::F9.accepted_at(p));
        assert!(!RleEscape::Fa.accepted_at(p));
        assert!(RleEscape::Fb.accepted_at(p));
        assert!(RleEscape::Fc.accepted_at(p));
        assert!(RleEscape::Fd.accepted_at(p));
        assert!(RleEscape::Fe.accepted_at(p));
        assert!(!RleEscape::Ff.accepted_at(p));
    }

    #[test]
    fn continuation2_drops_fe() {
        // §4.3 row "continuation 2": F/F/F/C/C/C/F/F.
        let p = PositionClass::Continuation2;
        assert!(RleEscape::Fb.accepted_at(p));
        assert!(RleEscape::Fc.accepted_at(p));
        assert!(RleEscape::Fd.accepted_at(p));
        assert!(!RleEscape::Fe.accepted_at(p));
        assert!(!RleEscape::Ff.accepted_at(p));
        assert!(!RleEscape::F8.accepted_at(p));
    }

    #[test]
    fn continuation3_drops_fd_too() {
        // §4.3 row "continuation 3": F/F/F/C/C/F/F/F.
        let p = PositionClass::Continuation3;
        assert!(RleEscape::Fb.accepted_at(p));
        assert!(RleEscape::Fc.accepted_at(p));
        assert!(!RleEscape::Fd.accepted_at(p));
        assert!(!RleEscape::Fe.accepted_at(p));
    }

    #[test]
    fn fbfcfd_accepted_at_every_position() {
        // §4.3: FB, FC, FD continue at every position.
        for p in [
            PositionClass::CellFirst,
            PositionClass::RowFirst,
            PositionClass::Continuation1,
            PositionClass::Continuation2,
        ] {
            assert!(RleEscape::Fb.accepted_at(p));
            assert!(RleEscape::Fc.accepted_at(p));
        }
        // FD survives through continuation 2 but faults at 3.
        assert!(RleEscape::Fd.accepted_at(PositionClass::Continuation2));
        assert!(!RleEscape::Fd.accepted_at(PositionClass::Continuation3));
        // FB/FC survive even continuation 3.
        assert!(RleEscape::Fb.accepted_at(PositionClass::Continuation3));
        assert!(RleEscape::Fc.accepted_at(PositionClass::Continuation3));
    }

    #[test]
    fn dyad_address_layout() {
        let m = LiteralMode::from_byte(0x05); // low nibble 5 → band 5
        let a = DyadAddress::new(m, 3);
        assert_eq!(a.band_base, 5 * ARENA_BAND_STRIDE);
        assert_eq!(a.primary_offset, 5 * 2048 + 4 * 3 + 0x400);
        assert_eq!(a.secondary_offset, 5 * 2048 + 4 * 3 + 0x402);
        // primary and secondary differ by 2 (the +0x400 vs +0x402).
        assert_eq!(a.secondary_offset - a.primary_offset, 2);
    }

    #[test]
    fn continuation_needed_on_sign_bit() {
        // High bit clear → no continuation (jns taken).
        assert!(!continuation_needed(0x0000_0001));
        assert!(!continuation_needed(0x7FFF_FFFF));
        // High bit set → continuation read.
        assert!(continuation_needed(0x8000_0000));
        assert!(continuation_needed(0xFFFF_FFFF));
    }

    #[test]
    fn continuation_xor_backs_out_sign() {
        assert_eq!(CONTINUATION_XOR, 0x8000_8000);
        // XOR flips both packed-word high bits.
        assert_eq!(apply_continuation_xor(0x0000_0000), 0x8000_8000);
        assert_eq!(apply_continuation_xor(0x8000_8000), 0x0000_0000);
    }

    #[test]
    fn fb_category_table_matches_spec_ranges() {
        let t = fb_category_table();
        for (c, &v) in t.iter().enumerate() {
            let expect = match c {
                0x00 => 0x00,
                0x01..=0x1F => 0x04,
                0x20 => 0x00,
                0x21..=0x3F => 0x08,
                _ => 0x00, // 0x40..=0xFF reserved → category 0 (§4.4).
            };
            assert_eq!(v, expect, "counter {c:#x}");
        }
    }

    #[test]
    fn fb_category_classifier() {
        assert_eq!(fb_category(0x00), FbCategory::Zero);
        assert_eq!(fb_category(0x01), FbCategory::Copy);
        assert_eq!(fb_category(0x1F), FbCategory::Copy);
        assert_eq!(fb_category(0x20), FbCategory::Zero);
        assert_eq!(fb_category(0x21), FbCategory::MarkSkipped);
        assert_eq!(fb_category(0x3F), FbCategory::MarkSkipped);
        assert_eq!(fb_category(0x40), FbCategory::Zero);
        assert_eq!(fb_category(0xFF), FbCategory::Zero);
        // Category byte values.
        assert_eq!(FbCategory::Zero.value(), 0x00);
        assert_eq!(FbCategory::Copy.value(), 0x04);
        assert_eq!(FbCategory::MarkSkipped.value(), 0x08);
    }

    #[test]
    fn fb_counter_decode() {
        // §4.4 corrected: run length is counter & 0x1F exactly — no
        // off-by-one. 0x03 -> a 3-position repeat-row-above run.
        let c = FbCounter::decode(0x03);
        assert_eq!(c.run_length, 3);
        assert!(c.repeats_row_above);
        assert!(c.is_valid());
        assert_eq!(c.category, FbCategory::Copy);

        // 0x25: bit 5 set -> a 5-position edge-mark run.
        let c = FbCounter::decode(0x25);
        assert_eq!(c.run_length, 5);
        assert!(!c.repeats_row_above);
        assert!(c.is_valid());
        assert_eq!(c.category, FbCategory::MarkSkipped);

        // 0x00 / 0x20: zero run length — rejected (category 0x00 is
        // the plane decoder's error return).
        let c = FbCounter::decode(0x00);
        assert_eq!(c.run_length, 0);
        assert!(!c.is_valid());
        assert_eq!(c.category, FbCategory::Zero);
        assert!(!FbCounter::decode(0x20).is_valid());

        // 0xC0: bits 6..7 set — rejected, not tolerated (§4.4
        // corrected).
        let c = FbCounter::decode(0xC0);
        assert!(!c.is_valid());
        assert_eq!(c.category, FbCategory::Zero);
    }

    #[test]
    fn handler_prologues_match_spec_3_2() {
        use CodebookBase::*;
        assert_eq!(handler_prologue(0x1000_6c90), Some((1, ArenaBand)));
        assert_eq!(handler_prologue(0x1000_6c9c), Some((1, StagingImage)));
        assert_eq!(handler_prologue(0x1000_72bb), Some((2, ArenaBand)));
        assert_eq!(handler_prologue(0x1000_72c7), Some((2, StagingImage)));
        assert_eq!(handler_prologue(0x1000_7710), Some((2, ArenaBand)));
        assert_eq!(handler_prologue(0x1000_771c), Some((2, StagingImage)));
        assert_eq!(handler_prologue(0x1000_7a9b), Some((2, StagingImage)));
        // The canonical dyad path and fault target are not prologues.
        assert_eq!(handler_prologue(0x1000_6c14), None);
        assert_eq!(handler_prologue(0x1000_7a96), None);
    }

    #[test]
    fn variant_entry_rvas() {
        assert_eq!(variant_entry_rva(CellVariant::Plain), VARIANT_A_ENTRY);
        assert_eq!(variant_entry_rva(CellVariant::WithEdge), VARIANT_B_ENTRY);
        assert_eq!(variant_entry_rva(CellVariant::DoubledRow), VARIANT_C_ENTRY);
        assert_eq!(
            variant_entry_rva(CellVariant::FullyDoubled),
            VARIANT_D_ENTRY
        );
    }

    #[test]
    fn fb_handler_rvas() {
        assert_eq!(FbCategory::Zero.handler_rva(), 0x1000_6d34);
        assert_eq!(FbCategory::Copy.handler_rva(), 0x1000_6d39);
        assert_eq!(FbCategory::MarkSkipped.handler_rva(), 0x1000_6d97);
    }

    #[test]
    fn position_class_base_rvas() {
        assert_eq!(
            PositionClass::CellFirst.variant_a_row0_base_rva(),
            0x1000_68ec
        );
        assert_eq!(
            PositionClass::Continuation1.variant_a_row0_base_rva(),
            0x1000_6a60
        );
        assert_eq!(
            PositionClass::Continuation3.variant_a_row0_base_rva(),
            0x1000_6b64
        );
    }

    #[test]
    fn row_lookahead_offsets_match_spec_1_2() {
        // §1.2 / §3.3: row 0 → +1, row 1 → +2, row 2 → +3, row 3 → +4.
        for (row, expect) in [(0, 1), (1, 2), (2, 3), (3, 4)] {
            let l = RowLookahead::for_row(row).expect("row 0..=3 is valid");
            assert_eq!(l.row_index, row);
            assert_eq!(l.continuation_offset, expect, "row {row}");
            // The displacement is exactly one more than the row index.
            assert_eq!(l.continuation_offset, row + 1);
        }
    }

    #[test]
    fn row_lookahead_read_site_rvas() {
        // §1.2 "Cross-row escape lookahead" read-site RVAs.
        assert_eq!(RowLookahead::for_row(0).unwrap().read_site_rva, 0x1000_6e18);
        assert_eq!(RowLookahead::for_row(1).unwrap().read_site_rva, 0x1000_6e91);
        assert_eq!(RowLookahead::for_row(2).unwrap().read_site_rva, 0x1000_6f17);
        assert_eq!(RowLookahead::for_row(3).unwrap().read_site_rva, 0x1000_6f98);
    }

    #[test]
    fn row_lookahead_rejects_out_of_range_rows() {
        // A cell has at most four rows (spec/03 §2.4); row 4+ is invalid.
        assert!(RowLookahead::for_row(4).is_none());
        assert!(RowLookahead::for_row(255).is_none());
        // The max offset constant equals the row-3 displacement.
        assert_eq!(MAX_ROW_LOOKAHEAD_OFFSET, 4);
        assert_eq!(
            RowLookahead::for_row(3).unwrap().continuation_offset,
            MAX_ROW_LOOKAHEAD_OFFSET
        );
    }
}
