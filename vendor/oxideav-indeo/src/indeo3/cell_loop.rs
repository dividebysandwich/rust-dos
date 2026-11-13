//! Indeo 3 outer per-cell row/column loop (`cl`/`ch` counter walk).
//!
//! Spec source: `docs/video/indeo/indeo3/spec/04-vq-codebooks.md`
//! §3.3 (codebook-bank base selection at cell time) and §1.1 (the
//! sub-table layout the lookup hits), with the sanity-check / fault
//! disposition pinned by §5 of `spec/05-motion-compensation.md`.
//!
//! Round 7 (`reconstruct::emit_variant`) lands the per-position dyad
//! kernel — given a 4-byte predictor DWORD plus a primary/secondary
//! delta pair, it produces the output pixel DWORD(s) the four cell
//! variants store. Round 8 (`strip_context`) lands the strip-context
//! slot's geometry. The piece that bridges them is the **outer
//! per-cell row/column loop** — the binary's
//! `IR32_32.DLL!0x1000665e..0x100066cc` preamble that:
//!
//! 1. Picks between the **primary** codebook bank sub-tables (`bank +
//!    0x000..+0x6ff` / `+0x700..+0xaff`) and the **`+0xb00` mirror**
//!    used by intra-context-without-stack cells (§3.3 step 1, the
//!    `cmp [esi + 4*eax + 0x40], 0; jne` fork at `0x1000665e`).
//! 2. Loads the **cell-position offset** the row/column walk advances
//!    against: `cell_position = bank[+0x300 + 4*cl]` (4 bytes). The
//!    loaded DWORD is **sanity-checked** against the constant
//!    `0xf423f`; any value `>= 0xf423f` faults to the malformed-
//!    bitstream branch at `0x10006b97` (per §3.3 step 2 and `spec/05`
//!    §5).
//! 3. Loads the **`cl` row-counter** the inner variant kernels
//!    decrement: `new_cl = bank[+0x000 + cl]` (1 byte). The new `cl`
//!    becomes the row-loop counter (`dec cl` in
//!    [`super::reconstruct::emit_variant`]'s call site, §2.2 / §3.3).
//! 4. **Clears the intra-context flag** (`ecx &= 0xbfffffff` at
//!    `0x10006670`) — the per-cell state machine resets the
//!    cell-stack-empty bit after the dispatch so the next cell starts
//!    clean.
//!
//! The §3.3 chapter boundary: this module ends at "the cell has a
//! row-loop counter `cl_inner`, a cell-position offset within the
//! strip buffer, and the codebook-bank-side mirror choice for the
//! inner variant kernel to consult". The per-byte unpacker dispatch
//! at `IR32_32.DLL!0x10006bac` (the mode-byte high-nibble jump table,
//! `spec/06`) and the inner column advance (`add edi, [esp+0x20]`,
//! `spec/07` §2.2's row-store shape) are *downstream* of this loop's
//! preamble.
//!
//! What this module **deliberately does not do**:
//!
//! * It does not allocate or own the cell-geometry bank bytes —
//!   `spec/04` §1.1 documents the bank as a 4.75 KB
//!   `inner_instance + 0x400` / `+0x1a00` view, populated at codec
//!   init by `IR32_32.DLL!0x100038f0`. Per-entry contents are
//!   Extractor territory (§7.1). This module operates on a
//!   `&[u8]` slice the caller hands in (or on the raw `cl`-indexed
//!   bytes for fine-grain unit tests).
//! * It does not drive the per-row variant kernel — round 7's
//!   [`super::emit_variant`] handles each `(cell_row, cell_col)`
//!   position. The loop's *progression* (decrement `cl_inner`,
//!   advance `edi` by `0xb0`, advance `edi` by `[esp+0x20]` for the
//!   next column) is recorded as [`CellRowAdvance`] so the caller
//!   can step the strip pixel-buffer cursor exactly as the binary
//!   does without this module reaching into the strip allocator.
//! * It does not perform the §3.4 VQ_DATA / VQ_NULL fork
//!   (`test ecx, 0x800000` at `0x100066cc`) — that's downstream of
//!   the bank-lookup; the caller pairs the [`CellLoopPreamble`]
//!   outcome with the [`super::CodebookEntry`] (round 4) or the
//!   [`super::VqNullRuntime`] (round 4) it already decoded.

use super::PREDICTOR_ROW_STRIDE;
use super::vq::{ARENA_BAND_LEN, ARENA_HALF_LEN};

// ---- spec/04 §1.1 (cell-geometry bank layout) -----------------------

/// Spec/04 §1.1 — total bytes per cell-geometry bank (`0x1300`, =
/// 4.75 KB). Each plane's bank (luma at `inner_instance + 0x1a00`,
/// chroma at `inner_instance + 0x400`) is this wide.
pub const CELL_BANK_LEN: usize = 0x1300;

/// Spec/04 §1.1 — byte offset of the **`cl`-indexed row-counter LUT**
/// within a primary cell-geometry bank (`bank + 0x000`, 256 bytes).
/// The `mov cl, [eax + edi]` at `IR32_32.DLL!0x100066b0` reads the
/// 1-byte entry `bank[CL_ROW_COUNTER_LUT + cl]` and replaces `cl`
/// with it (§3.3 step 3).
pub const CL_ROW_COUNTER_LUT: usize = 0x000;

/// Spec/04 §1.1 — byte offset of the **`ch`-indexed control LUT**
/// within a primary cell-geometry bank (`bank + 0x100`, 256 bytes).
/// Read at `IR32_32.DLL!0x10006691` (`mov ch, [eax + edi + 0x100]`)
/// after `rol ecx, 0x18` has rotated `ch` into the low byte. The
/// resulting `ch` value drives the next iteration's slot-index
/// dispatch (`spec/03 §3.3`).
pub const CH_CONTROL_LUT: usize = 0x100;

/// Spec/04 §1.1 — byte offset of the **slot-index LUT** within a
/// primary cell-geometry bank (`bank + 0x200`, 256 bytes). The
/// `mov al, [eax + edx + 0x200]` at `IR32_32.DLL!0x10006615` reads
/// `bank[SLOT_INDEX_LUT + ch]` and the result is multiplied by 16 to
/// produce a slot stride for the strip-context array
/// (`spec/02 §5.2`).
pub const SLOT_INDEX_LUT: usize = 0x200;

/// Spec/04 §1.1 — byte offset of the **cell-data DWORD table** within
/// a primary cell-geometry bank (`bank + 0x300`, 256 × 4 B = 1 KB).
/// The `mov esi, [edi + 4*eax + 0x300]` at `IR32_32.DLL!0x10006669`
/// reads the 4-byte cell-data entry indexed by `cl`. The DWORD is
/// the **cell-position offset** the row/column walk advances against
/// (§3.3 step 2).
pub const CELL_DATA_TABLE: usize = 0x300;

/// Spec/04 §1.1 — byte offset of the **cell-position DWORD table**
/// within a primary cell-geometry bank (`bank + 0x700`, 256 × 4 B).
/// The `mov edx, [eax + edi + 0x700]` at `IR32_32.DLL!0x10006698`
/// reads the per-cell-state cell-position DWORD that addresses the
/// strip pixel buffer.
pub const CELL_POSITION_TABLE: usize = 0x700;

/// Spec/04 §1.1 / §3.3 step 1 — byte offset of the **`+0xb00`
/// mirror** sub-tables within a cell-geometry bank (`bank + 0xb00`,
/// 0x800 bytes). Selected by the cell-stack-empty fork at
/// `IR32_32.DLL!0x10006658..0x1000665e` for intra-context-without-
/// stack cells.
pub const MIRROR_TABLE_OFFSET: usize = 0xb00;

/// Spec/04 §3.3 step 2 / `spec/05` §5 — the cell-position-offset
/// sanity-check constant. The DWORD loaded from `bank[+0x300 + 4*cl]`
/// is compared `cmp esi, 0xf423f` at `IR32_32.DLL!0x10006676`; any
/// value `>= 0xf423f` (= 999,999 decimal) triggers a malformed-
/// bitstream fault at `0x10006b97` (the `jge` is taken).
pub const CELL_POSITION_MAX: u32 = 0xf423f;

/// Spec/04 §3.3 step 4 — the bitmask that **clears the intra-context
/// flag** (bit 30 of the `ecx` cell-state register). The instruction
/// `and ecx, 0xbfffffff` at `IR32_32.DLL!0x10006670` zeroes the bit
/// after the codebook-bank-side dispatch so the next cell-state
/// transition is unaffected.
pub const INTRA_CONTEXT_CLEAR_MASK: u32 = 0xbfffffff;

/// Spec/04 §3.3 step 4 — the intra-context flag bit itself
/// (bit 30, mask `0x40000000`). Set by `IR32_32.DLL!0x10006684`
/// (`or ecx, 0x40000000`) when an INTRA leaf enters VQ_TREE
/// (`spec/03 §3.3`), and cleared by [`INTRA_CONTEXT_CLEAR_MASK`].
pub const INTRA_CONTEXT_FLAG: u32 = 0x40000000;

// ---- spec/04 §3.3 (codebook-bank-side mirror choice) ----------------

/// Spec/04 §3.3 step 1 — which sub-table half of the cell-geometry
/// bank the per-cell dispatcher consults.
///
/// The fork at `IR32_32.DLL!0x10006658..0x1000665e` reads the
/// strip-context slot's cell-stack top
/// (`[strip_ctx + 0x40 + 4*slot_idx]`) and:
///
/// * If the cell-stack top is **non-zero** (the cell was reached via
///   a push from a parent split — i.e. it's a normal in-strip cell),
///   the dispatcher uses the **primary** sub-tables at `bank + 0x000`
///   and onward. → [`CodebookBankView::Primary`].
/// * If the cell-stack top is **zero** (this cell is the *first*
///   cell of a strip's MC_TREE walk — there is no parent split on
///   the stack), the dispatcher uses the **`+0xb00` mirror** sub-
///   tables. → [`CodebookBankView::Mirror`].
///
/// The mirror tables encode different per-cell-state →
/// cell-position mappings than the primary tables (the entry
/// semantics are §7.1's Extractor territory).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodebookBankView {
    /// Cell-stack top is non-zero → use the primary sub-tables at
    /// `bank + 0x000`.
    Primary,
    /// Cell-stack top is zero → use the `+0xb00` mirror sub-tables
    /// (intra-context-without-stack case).
    Mirror,
}

impl CodebookBankView {
    /// Spec/04 §3.3 step 1 — pick the bank view from the cell-stack
    /// top at `[strip_ctx + 0x40 + 4*slot_idx]`.
    ///
    /// `cell_stack_top == 0` → `Mirror`; any other value → `Primary`.
    pub fn from_cell_stack_top(cell_stack_top: u32) -> Self {
        if cell_stack_top == 0 {
            CodebookBankView::Mirror
        } else {
            CodebookBankView::Primary
        }
    }

    /// The byte offset to add to a primary-bank base pointer to
    /// reach the start of this view's sub-tables. `Primary` adds 0;
    /// `Mirror` adds [`MIRROR_TABLE_OFFSET`] (`0xb00`).
    pub fn bank_base_offset(self) -> usize {
        match self {
            CodebookBankView::Primary => 0,
            CodebookBankView::Mirror => MIRROR_TABLE_OFFSET,
        }
    }

    /// True iff this is the `+0xb00` mirror view (intra-context-
    /// without-stack).
    pub fn is_mirror(self) -> bool {
        matches!(self, CodebookBankView::Mirror)
    }
}

// ---- spec/04 §3.3 (per-cell preamble outcome) ----------------------

/// Spec/04 §3.3 — terminal outcome of the per-cell preamble.
///
/// The preamble at `IR32_32.DLL!0x1000665e..0x10006670` runs the
/// four lookups (mirror choice, cell-position-offset load + sanity
/// check, `cl`-counter load, intra-context flag clear) in a single
/// branchless sequence. The only conditional path is the
/// `cmp esi, 0xf423f; jge 0x10006b97` sanity-check fault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellLoopPreamble {
    /// Successful preamble — the row-counter, cell-position offset,
    /// chosen bank view, and post-clear `ecx` are ready for the
    /// inner variant kernel.
    Ready(CellLoopState),
    /// The cell-position DWORD failed the `>= 0xf423f` sanity check;
    /// the decoder branches to `IR32_32.DLL!0x10006b97`, the
    /// malformed-bitstream return path
    /// ([`super::PlaneDecodeStatus::Malformed`]).
    CellPositionFault {
        /// The faulting DWORD, as loaded from `bank[+0x300 + 4*cl]`.
        offset: u32,
    },
}

impl CellLoopPreamble {
    /// True iff the preamble produced a [`CellLoopState`]
    /// (i.e. the cell-position sanity check passed).
    pub fn is_ready(&self) -> bool {
        matches!(self, CellLoopPreamble::Ready(_))
    }

    /// Borrow the inner state on success.
    pub fn state(&self) -> Option<&CellLoopState> {
        if let CellLoopPreamble::Ready(s) = self {
            Some(s)
        } else {
            None
        }
    }
}

/// Spec/04 §3.3 — the resolved state the inner variant kernel needs.
///
/// `cl_inner` is the row-loop counter (`dec cl` decrements it until
/// it reaches zero), `cell_position_offset` is the byte offset within
/// the strip pixel buffer where the cell's pixels are stored
/// (relative to the slot's base pointer at `[ctx+0x00]`, spec/02
/// §5.2), `bank_view` records the mirror choice, and `ecx_post_clear`
/// is the `ecx` register's value after `and ecx, 0xbfffffff` —
/// preserved so the §3.4 VQ_DATA / VQ_NULL fork
/// (`test ecx, 0x800000`) can be applied without recomputing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellLoopState {
    /// The row-loop counter loaded from `bank[+0x000 + cl_in]`. The
    /// inner variant kernel calls `emit_variant` while `cl_inner > 0`,
    /// decrementing once per row.
    pub cl_inner: u8,
    /// The cell-position offset loaded from `bank[+0x300 + 4*cl_in]`.
    /// Strictly less than [`CELL_POSITION_MAX`] (`0xf423f`) on the
    /// `Ready` branch; the inner kernel adds this to the slot's base
    /// pointer to locate the first cell byte.
    pub cell_position_offset: u32,
    /// Which bank view supplied the lookups (primary vs `+0xb00`
    /// mirror).
    pub bank_view: CodebookBankView,
    /// The `ecx` cell-state register after `and ecx, 0xbfffffff`
    /// (intra-context flag clear). Bit 30 is guaranteed clear; bits
    /// 29..0 and 31 are preserved from the caller-supplied `ecx_in`.
    pub ecx_post_clear: u32,
}

impl CellLoopState {
    /// Spec/04 §3.4 — does this cell's state set the VQ_DATA flag
    /// (bit 31, mask `0x80000000`)? The fork at
    /// `IR32_32.DLL!0x100066cc` tests this bit after the preamble
    /// (the `rol ecx, 0x18` rotated it to position 7).
    pub fn vq_data_flag(self) -> bool {
        self.ecx_post_clear & 0x80000000 != 0
    }
}

// ---- spec/04 §3.3 (the bank-lookup primitives) ----------------------

/// Spec/04 §1.1 / §3.3 step 3 — read the `cl`-indexed row counter
/// from the bank: `bank[CL_ROW_COUNTER_LUT + cl]`.
///
/// `bank` is the cell-geometry-bank slice as passed in the per-plane
/// decode call's 7th argument (`spec/02 §6` — `inner_instance +
/// 0x1a00` for luma, `inner_instance + 0x400` for chroma), already
/// adjusted for the [`CodebookBankView`] choice.
///
/// Returns `None` if `bank.len() <= cl as usize` (the per-bank size
/// invariant requires `bank.len() >= CELL_BANK_LEN`; this check
/// fails closed for unit-test slices shorter than that).
pub fn read_cl_row_counter(bank: &[u8], cl: u8) -> Option<u8> {
    bank.get(CL_ROW_COUNTER_LUT + cl as usize).copied()
}

/// Spec/04 §1.1 / §3.3 step 2 — read the `cl`-indexed cell-position
/// DWORD from the bank: `bank[CELL_DATA_TABLE + 4*cl]` interpreted
/// little-endian.
///
/// Returns `None` if the bank slice is too short to contain the
/// 4-byte entry.
pub fn read_cell_position_dword(bank: &[u8], cl: u8) -> Option<u32> {
    let start = CELL_DATA_TABLE + 4 * cl as usize;
    let bytes = bank.get(start..start + 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

/// Spec/04 §3.3 — full per-cell preamble: pick the bank view from
/// the cell-stack top, then run the cell-position-offset + `cl`-
/// counter lookups and the intra-context-flag clear.
///
/// `bank_primary` is the cell-geometry-bank slice at its primary base
/// (`inner_instance + 0x1a00` for luma, `inner_instance + 0x400` for
/// chroma); the mirror sub-tables at `+0xb00` are assumed to sit at
/// `bank_primary[MIRROR_TABLE_OFFSET..]` (i.e. the same slice covers
/// both halves, as the binary expects — the bank is 4.75 KB in one
/// contiguous block).
///
/// `cell_stack_top` is the 32-bit DWORD at `[strip_ctx + 0x40 +
/// 4*slot_idx]`; `cl_in` is the cell-state `cl` byte at the time of
/// the dispatch; `ecx_in` is the cell-state `ecx` register.
///
/// Returns [`CellLoopPreamble::CellPositionFault`] if the loaded
/// cell-position DWORD is `>= 0xf423f`. Returns
/// [`CellLoopPreamble::Ready`] otherwise.
pub fn dispatch_cell_preamble(
    bank_primary: &[u8],
    cell_stack_top: u32,
    cl_in: u8,
    ecx_in: u32,
) -> CellLoopPreamble {
    let bank_view = CodebookBankView::from_cell_stack_top(cell_stack_top);
    let base = bank_view.bank_base_offset();
    // Step 2: cell-position offset (4-byte) with sanity check.
    let pos_dword = match bank_primary
        .get(base + CELL_DATA_TABLE + 4 * cl_in as usize..)
        .and_then(|s| s.get(..4))
        .and_then(|s| s.try_into().ok().map(u32::from_le_bytes))
    {
        Some(v) => v,
        None => {
            // Out-of-bounds slice → cannot proceed; treat as fault.
            return CellLoopPreamble::CellPositionFault { offset: 0 };
        }
    };
    if pos_dword >= CELL_POSITION_MAX {
        return CellLoopPreamble::CellPositionFault { offset: pos_dword };
    }
    // Step 3: new `cl` from `bank[+0x000 + cl]`.
    let cl_inner = match bank_primary.get(base + CL_ROW_COUNTER_LUT + cl_in as usize) {
        Some(&b) => b,
        None => {
            return CellLoopPreamble::CellPositionFault { offset: pos_dword };
        }
    };
    // Step 4: clear intra-context flag.
    let ecx_post_clear = ecx_in & INTRA_CONTEXT_CLEAR_MASK;
    CellLoopPreamble::Ready(CellLoopState {
        cl_inner,
        cell_position_offset: pos_dword,
        bank_view,
        ecx_post_clear,
    })
}

// ---- spec/04 §3.3 + §2.2 (row/column advance) ----------------------

/// Spec/04 §3.3 / §2.2 — one step of the outer per-cell row/column
/// walk after a row is emitted.
///
/// The inner variant kernel (`reconstruct::emit_variant`) emits the
/// row's pixel DWORD(s) at the current `edi` (and optionally
/// `edi + 0xb0` for the two-row variants). After the row store, the
/// outer loop:
///
/// * Decrements the row-counter (`dec cl`).
/// * Advances `edi` by [`PREDICTOR_ROW_STRIDE`] (`0xb0`) when more
///   rows remain in this cell-column.
/// * When `cl` reaches zero, advances `edi` by the per-column step
///   stored at `[esp+0x20]` (the cell-column-advance value).
///
/// This type is the structural advance descriptor — given a current
/// `(cl_inner, edi)` and the per-column step, it returns the next
/// `(cl_inner, edi)` plus a flag stating whether the row was the
/// last in the current cell-column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRowAdvance {
    /// The new value of the row counter after the `dec cl`. Zero
    /// signals "this was the last row of the current cell-column"
    /// and the caller advances `edi` by the per-column step.
    pub cl_after: u8,
    /// The new value of the `edi` write cursor. When `cl_after > 0`,
    /// this is `edi + PREDICTOR_ROW_STRIDE`; when `cl_after == 0`,
    /// this is `edi + cell_column_step`.
    pub edi_after: usize,
    /// True iff the row was the last in the current cell-column
    /// (the `dec cl` set ZF). When true, the per-column step was
    /// applied to `edi_after`.
    pub end_of_column: bool,
}

/// Spec/04 §3.3 / §2.2 — advance the outer loop by one row.
///
/// `cl_before` is the row counter before the `dec`; `edi_before` is
/// the write cursor at the time the row was stored;
/// `cell_column_step` is the per-column advance value the binary
/// loads from `[esp+0x20]` at preamble time (the exact value is
/// variant-dependent — `0x4` for the four-byte DWORD store of the
/// plain variant; the larger doubled-stride variants supply a wider
/// step).
///
/// Returns `None` when `cl_before == 0` (the caller should never
/// have called the inner kernel in that state).
pub fn advance_row(
    cl_before: u8,
    edi_before: usize,
    cell_column_step: usize,
) -> Option<CellRowAdvance> {
    if cl_before == 0 {
        return None;
    }
    let cl_after = cl_before - 1;
    if cl_after == 0 {
        Some(CellRowAdvance {
            cl_after,
            edi_after: edi_before.saturating_add(cell_column_step),
            end_of_column: true,
        })
    } else {
        Some(CellRowAdvance {
            cl_after,
            edi_after: edi_before.saturating_add(PREDICTOR_ROW_STRIDE),
            end_of_column: false,
        })
    }
}

/// Spec/04 §3.3 — iterate the per-cell row positions an inner kernel
/// will hit, starting at `cl_inner` and stepping with [`advance_row`]
/// until the column ends.
///
/// Returns the list of `(cl_value, edi)` pairs that an inner variant
/// kernel would call `reconstruct::emit_variant` at, in order. The
/// final entry has `cl_value == 1` (the last row before `dec cl`
/// zeroes the counter); the caller advances `edi` by
/// `cell_column_step` after the final row's store.
///
/// `cell_column_step` is the per-column advance value
/// ([`CellRowAdvance::edi_after`]). The returned vector has exactly
/// `cl_inner as usize` entries (one per row in the column).
pub fn iterate_column_rows(
    cl_inner: u8,
    edi_start: usize,
    cell_column_step: usize,
) -> Vec<(u8, usize)> {
    let mut out = Vec::with_capacity(cl_inner as usize);
    let mut cl = cl_inner;
    let mut edi = edi_start;
    while cl > 0 {
        out.push((cl, edi));
        let adv = match advance_row(cl, edi, cell_column_step) {
            Some(a) => a,
            None => break,
        };
        cl = adv.cl_after;
        edi = adv.edi_after;
    }
    out
}

// ---- spec/04 §3.3 — sanity that this module's constants line up ----

// Cross-check: the cell-geometry bank's primary sub-table sizes sum
// to `MIRROR_TABLE_OFFSET` (the `+0xb00` mirror starts immediately
// after the last primary sub-table per §1.1).
const _: () = {
    // 256 + 256 + 256 = 0x300 (the three byte-LUTs at +0x000 / +0x100
    // / +0x200), plus 256 × 4 = 0x400 (the cell-data DWORD table at
    // +0x300), plus 256 × 4 = 0x400 (the cell-position DWORD table at
    // +0x700) = 0xb00.
    let three_byte_luts = 3 * 256;
    let cell_data = 256 * 4;
    let cell_position = 256 * 4;
    assert!(three_byte_luts + cell_data + cell_position == MIRROR_TABLE_OFFSET);
};

// Cross-check: the four sub-table base offsets are correctly ordered
// without gaps from `0x000` up through `0xaff`.
const _: () = {
    assert!(CL_ROW_COUNTER_LUT == 0x000);
    assert!(CH_CONTROL_LUT == CL_ROW_COUNTER_LUT + 256);
    assert!(SLOT_INDEX_LUT == CH_CONTROL_LUT + 256);
    assert!(CELL_DATA_TABLE == SLOT_INDEX_LUT + 256);
    assert!(CELL_POSITION_TABLE == CELL_DATA_TABLE + 256 * 4);
    assert!(MIRROR_TABLE_OFFSET == CELL_POSITION_TABLE + 256 * 4);
};

// Cross-check: the per-frame arena's per-band sub-table size lines up
// with the arena's stated half-length (round 4's [`ARENA_HALF_LEN`]
// = 0x800 = 2 KiB). Two halves per band × 16 bands = the full
// `ARENA_BAND_LEN * 16` sub-region the round-7 variant kernels
// emit into.
const _: () = {
    assert!(ARENA_HALF_LEN * 2 == ARENA_BAND_LEN);
};

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_bank() -> Vec<u8> {
        // Build a tiny synthetic bank of the right size with
        // deterministic byte values so the lookups are easy to
        // predict. Bytes are `i.wrapping_mul(7).wrapping_add(13)`.
        (0..CELL_BANK_LEN)
            .map(|i| (i.wrapping_mul(7).wrapping_add(13)) as u8)
            .collect()
    }

    #[test]
    fn bank_layout_constants_match_spec_table() {
        // Spec/04 §1.1 table: byte-LUTs at +0x000 / +0x100 / +0x200,
        // DWORD tables at +0x300 / +0x700, mirror at +0xb00, total
        // 0x1300.
        assert_eq!(CL_ROW_COUNTER_LUT, 0x000);
        assert_eq!(CH_CONTROL_LUT, 0x100);
        assert_eq!(SLOT_INDEX_LUT, 0x200);
        assert_eq!(CELL_DATA_TABLE, 0x300);
        assert_eq!(CELL_POSITION_TABLE, 0x700);
        assert_eq!(MIRROR_TABLE_OFFSET, 0xb00);
        assert_eq!(CELL_BANK_LEN, 0x1300);
    }

    #[test]
    fn sanity_check_constants_match_spec() {
        // Spec/04 §3.3 step 2 (and spec/05 §5): the cell-position
        // sanity-check limit is 0xf423f (= 999_999 decimal).
        assert_eq!(CELL_POSITION_MAX, 0xf423f);
        assert_eq!(CELL_POSITION_MAX, 999_999);
        // Spec/04 §3.3 step 4: intra-context clear mask is the
        // complement of bit 30.
        assert_eq!(INTRA_CONTEXT_CLEAR_MASK, !INTRA_CONTEXT_FLAG);
        assert_eq!(INTRA_CONTEXT_FLAG, 0x40000000);
    }

    #[test]
    fn codebook_bank_view_picks_mirror_when_stack_empty() {
        // Cell-stack top zero → mirror.
        assert_eq!(
            CodebookBankView::from_cell_stack_top(0),
            CodebookBankView::Mirror
        );
        assert!(CodebookBankView::Mirror.is_mirror());
        assert_eq!(CodebookBankView::Mirror.bank_base_offset(), 0xb00);

        // Any non-zero value → primary.
        for v in [1u32, 0x1869f, 0xdead_beef, u32::MAX] {
            assert_eq!(
                CodebookBankView::from_cell_stack_top(v),
                CodebookBankView::Primary
            );
        }
        assert!(!CodebookBankView::Primary.is_mirror());
        assert_eq!(CodebookBankView::Primary.bank_base_offset(), 0);
    }

    #[test]
    fn read_cl_row_counter_reads_byte_at_index() {
        let bank = synth_bank();
        // Synth: `bank[0]` = 13; `bank[1]` = 20; ...
        assert_eq!(read_cl_row_counter(&bank, 0), Some(13));
        assert_eq!(read_cl_row_counter(&bank, 1), Some(20));
        // `bank[255]` = 255 * 7 + 13 (mod 256).
        let expected = (255u32 * 7 + 13) as u8;
        assert_eq!(read_cl_row_counter(&bank, 255), Some(expected));
    }

    #[test]
    fn read_cl_row_counter_rejects_short_slice() {
        let short = vec![0u8; 100];
        // 100 < 256: indices >= 100 fail.
        assert_eq!(read_cl_row_counter(&short, 99), Some(0));
        assert_eq!(read_cl_row_counter(&short, 100), None);
        assert_eq!(read_cl_row_counter(&short, 255), None);
    }

    #[test]
    fn read_cell_position_dword_reads_le_four_bytes() {
        let mut bank = vec![0u8; CELL_BANK_LEN];
        // Plant a known LE DWORD at +0x300 + 4*5 = 0x314.
        let cl: u8 = 5;
        let value: u32 = 0xdead_beef;
        let off = CELL_DATA_TABLE + 4 * cl as usize;
        bank[off..off + 4].copy_from_slice(&value.to_le_bytes());
        assert_eq!(read_cell_position_dword(&bank, cl), Some(value));
        // Out-of-range cl in a full-sized bank: still in-bounds
        // because the DWORD table is 256 × 4 = 1 KiB.
        assert!(read_cell_position_dword(&bank, 255).is_some());
    }

    #[test]
    fn dispatch_cell_preamble_picks_mirror_view_when_stack_empty() {
        let mut bank = vec![0u8; CELL_BANK_LEN];
        // Plant a small valid cell-position DWORD in the mirror's
        // cell-data table (mirror base 0xb00, +0x300 = 0xe00,
        // cl = 3 → 0xe0c).
        let cl: u8 = 3;
        let pos: u32 = 0x10;
        let off = MIRROR_TABLE_OFFSET + CELL_DATA_TABLE + 4 * cl as usize;
        bank[off..off + 4].copy_from_slice(&pos.to_le_bytes());
        // Plant a row counter at the mirror's `+0x000 + cl`.
        bank[MIRROR_TABLE_OFFSET + cl as usize] = 7;

        let pre = dispatch_cell_preamble(&bank, /* cell_stack_top */ 0, cl, 0);
        let state = pre.state().expect("ready");
        assert_eq!(state.bank_view, CodebookBankView::Mirror);
        assert_eq!(state.cl_inner, 7);
        assert_eq!(state.cell_position_offset, pos);
    }

    #[test]
    fn dispatch_cell_preamble_picks_primary_view_when_stack_nonzero() {
        let mut bank = vec![0u8; CELL_BANK_LEN];
        let cl: u8 = 11;
        let pos: u32 = 0xa0;
        let off = CELL_DATA_TABLE + 4 * cl as usize;
        bank[off..off + 4].copy_from_slice(&pos.to_le_bytes());
        bank[cl as usize] = 4;

        let pre = dispatch_cell_preamble(&bank, /* cell_stack_top */ 0x1234, cl, 0);
        let state = pre.state().expect("ready");
        assert_eq!(state.bank_view, CodebookBankView::Primary);
        assert_eq!(state.cl_inner, 4);
        assert_eq!(state.cell_position_offset, pos);
    }

    #[test]
    fn dispatch_cell_preamble_clears_intra_context_flag() {
        let mut bank = vec![0u8; CELL_BANK_LEN];
        bank[CELL_DATA_TABLE..CELL_DATA_TABLE + 4].copy_from_slice(&1u32.to_le_bytes());
        bank[0] = 1;
        // ecx with bit 30 set + bit 31 set + bits 0..29 set.
        let ecx_in: u32 = 0xffffffff;
        let pre = dispatch_cell_preamble(&bank, 1, 0, ecx_in);
        let state = pre.state().expect("ready");
        // bit 30 cleared; everything else preserved.
        assert_eq!(state.ecx_post_clear, ecx_in & !INTRA_CONTEXT_FLAG);
        assert_eq!(state.ecx_post_clear & INTRA_CONTEXT_FLAG, 0);
        // bit 31 preserved → VQ_DATA flag still set.
        assert!(state.vq_data_flag());
    }

    #[test]
    fn dispatch_cell_preamble_faults_on_out_of_range_position() {
        let mut bank = vec![0u8; CELL_BANK_LEN];
        // Cell-position == 0xf423f → fault (`>=`, not `>`).
        let cl: u8 = 0;
        let pos: u32 = CELL_POSITION_MAX;
        bank[CELL_DATA_TABLE..CELL_DATA_TABLE + 4].copy_from_slice(&pos.to_le_bytes());
        bank[0] = 1;
        let pre = dispatch_cell_preamble(&bank, 1, cl, 0);
        assert_eq!(pre, CellLoopPreamble::CellPositionFault { offset: pos });
        assert!(!pre.is_ready());

        // One above also faults.
        let pos: u32 = CELL_POSITION_MAX + 1;
        bank[CELL_DATA_TABLE..CELL_DATA_TABLE + 4].copy_from_slice(&pos.to_le_bytes());
        let pre = dispatch_cell_preamble(&bank, 1, cl, 0);
        assert_eq!(pre, CellLoopPreamble::CellPositionFault { offset: pos });

        // One below is fine.
        let pos: u32 = CELL_POSITION_MAX - 1;
        bank[CELL_DATA_TABLE..CELL_DATA_TABLE + 4].copy_from_slice(&pos.to_le_bytes());
        let pre = dispatch_cell_preamble(&bank, 1, cl, 0);
        assert!(pre.is_ready());
    }

    #[test]
    fn vq_data_flag_reads_bit_31() {
        let s = CellLoopState {
            cl_inner: 1,
            cell_position_offset: 0,
            bank_view: CodebookBankView::Primary,
            ecx_post_clear: 0x80000000,
        };
        assert!(s.vq_data_flag());
        let s = CellLoopState {
            cl_inner: 1,
            cell_position_offset: 0,
            bank_view: CodebookBankView::Primary,
            ecx_post_clear: 0x7fffffff,
        };
        assert!(!s.vq_data_flag());
    }

    #[test]
    fn advance_row_advances_by_row_stride_when_more_rows_remain() {
        // cl 4 → 3, edi advances by PREDICTOR_ROW_STRIDE (0xb0),
        // not end-of-column.
        let adv = advance_row(4, 0x100, 0x4).unwrap();
        assert_eq!(adv.cl_after, 3);
        assert_eq!(adv.edi_after, 0x100 + PREDICTOR_ROW_STRIDE);
        assert!(!adv.end_of_column);

        // cl 2 → 1, still mid-column.
        let adv = advance_row(2, 0x200, 0x4).unwrap();
        assert_eq!(adv.cl_after, 1);
        assert_eq!(adv.edi_after, 0x200 + PREDICTOR_ROW_STRIDE);
        assert!(!adv.end_of_column);
    }

    #[test]
    fn advance_row_advances_by_column_step_at_end_of_column() {
        // cl 1 → 0, end-of-column: edi advances by the per-column
        // step (the variant-dependent value at [esp+0x20]).
        let adv = advance_row(1, 0x300, 0x4).unwrap();
        assert_eq!(adv.cl_after, 0);
        assert_eq!(adv.edi_after, 0x300 + 0x4);
        assert!(adv.end_of_column);
    }

    #[test]
    fn advance_row_rejects_zero_counter() {
        // Caller bug: never call advance with cl_before == 0.
        assert!(advance_row(0, 0x100, 0x4).is_none());
    }

    #[test]
    fn iterate_column_rows_lists_every_row() {
        // A 4-row cell column at edi 0x100, column step 4. The walk
        // should hit cl values 4, 3, 2, 1 at edi 0x100, 0x1b0, 0x260,
        // 0x310 — i.e. four rows spaced by PREDICTOR_ROW_STRIDE.
        let rows = iterate_column_rows(4, 0x100, 4);
        assert_eq!(rows.len(), 4);
        assert_eq!(
            rows,
            vec![
                (4u8, 0x100usize),
                (3u8, 0x100 + PREDICTOR_ROW_STRIDE),
                (2u8, 0x100 + 2 * PREDICTOR_ROW_STRIDE),
                (1u8, 0x100 + 3 * PREDICTOR_ROW_STRIDE),
            ]
        );
    }

    #[test]
    fn iterate_column_rows_handles_single_row_columns() {
        // cl == 1 → exactly one (cl, edi) pair.
        let rows = iterate_column_rows(1, 0x200, 0x4);
        assert_eq!(rows, vec![(1u8, 0x200usize)]);
    }

    #[test]
    fn iterate_column_rows_handles_empty_column() {
        // cl == 0 → no rows emitted.
        let rows = iterate_column_rows(0, 0x200, 0x4);
        assert!(rows.is_empty());
    }

    #[test]
    fn iterate_column_rows_for_full_8_row_cell() {
        // The spec/04 §2.2 8×8 cell layout emits 8 rows per column.
        let rows = iterate_column_rows(8, 0, 0x10);
        assert_eq!(rows.len(), 8);
        // Edi values are 0, 0xb0, 0x160, 0x210, 0x2c0, 0x370, 0x420,
        // 0x4d0 — eight `0xb0`-stride rows.
        let expected_edi: Vec<usize> = (0..8).map(|i| i * PREDICTOR_ROW_STRIDE).collect();
        let actual_edi: Vec<usize> = rows.iter().map(|(_, e)| *e).collect();
        assert_eq!(actual_edi, expected_edi);
        // Cl values descend from 8 to 1.
        let expected_cl: Vec<u8> = (1..=8).rev().collect();
        let actual_cl: Vec<u8> = rows.iter().map(|(c, _)| *c).collect();
        assert_eq!(actual_cl, expected_cl);
    }

    #[test]
    fn dispatch_then_iterate_round_trip() {
        // End-to-end: a synthetic bank with a planted 5-row column at
        // cl=10 in the mirror view, iterate the column.
        let mut bank = vec![0u8; CELL_BANK_LEN];
        let cl: u8 = 10;
        let pos: u32 = 0x200;
        let off = MIRROR_TABLE_OFFSET + CELL_DATA_TABLE + 4 * cl as usize;
        bank[off..off + 4].copy_from_slice(&pos.to_le_bytes());
        bank[MIRROR_TABLE_OFFSET + cl as usize] = 5;

        let state = dispatch_cell_preamble(&bank, 0, cl, 0)
            .state()
            .copied()
            .expect("ready");
        assert_eq!(state.bank_view, CodebookBankView::Mirror);
        assert_eq!(state.cl_inner, 5);
        assert_eq!(state.cell_position_offset, pos);

        let rows = iterate_column_rows(state.cl_inner, state.cell_position_offset as usize, 8);
        assert_eq!(rows.len(), 5);
        // First row starts at the cell-position offset; last row
        // ends 4 row-strides later.
        assert_eq!(rows[0].1, pos as usize);
        assert_eq!(
            rows.last().unwrap().1,
            pos as usize + 4 * PREDICTOR_ROW_STRIDE
        );
    }
}
