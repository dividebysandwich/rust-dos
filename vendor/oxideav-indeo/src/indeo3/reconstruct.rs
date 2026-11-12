//! Indeo 3 output reconstruction: predictor formation + the softSIMD
//! dyad-pair `predictor + delta` add.
//!
//! Spec source: `docs/video/indeo/indeo3/spec/07-output-reconstruction.md`.
//!
//! Round 6 lands the first half of spec/07: the pixel-emission
//! arithmetic that spec/06 (round 5) deferred. spec/06's
//! [`super::continuation_needed`] / [`super::apply_continuation_xor`]
//! answered the *entropy* question — how many bytes a literal mode
//! byte costs and the XOR back-out shape. This module answers the
//! *arithmetic* question: given a predictor pixel-pair and the dyad
//! delta entry the per-frame arena (spec/04 §2.1) holds, how the
//! decoder forms the output pixel-pair DWORD.
//!
//! What this round covers, mapped to the spec/07 sections:
//!
//! * §0 / §1.1 — the predictor address. The predictor for the pixel
//!   about to be written at `[edi]` is `[edi - 0xb0]` (one row above,
//!   same column, in the strip pixel buffer). [`PREDICTOR_ROW_STRIDE`]
//!   is the `0xb0` (176) row stride; [`predictor_offset`] computes the
//!   row-above byte index from a write index.
//! * §1.3 / §9 — the top-of-strip predictor seed. Cells whose
//!   row-above slot falls outside the strip read the zero-initialised
//!   padding, so the top-of-strip predictor is the constant
//!   [`TOP_OF_STRIP_PREDICTOR`] (`0x40`, fixture-arbitrated r451).
//! * §2.1 / §2.3 — the softSIMD dyad-pair add. [`apply_dyad_pair`]
//!   computes `predictor + primary_delta` as a byte-parallel DWORD
//!   add, detects the 16-bit-half overflow sentinel (`jns` on the
//!   full DWORD, then `js` on the low 16-bit half after the secondary
//!   add), runs the continuation fall-back against the secondary-table
//!   word, and faults if the secondary add is still sign-set.
//! * §4.1 / §4.2 — the 7-bit-per-byte range. There is no explicit
//!   saturation; bit 7 of every pixel byte is reserved as the
//!   edge / boundary marker, and the overflow sentinel (bit 15 of
//!   each 16-bit half) doubles as the continuation trigger.
//!   [`SoftSimdSum`] records both halves' overflow state.
//!
//! Round 7 (this round) adds the four cell-shape variant inner-loop
//! emission kernels (spec/07 §2.2 / spec/04 §2.2). Round 6 landed the
//! shared per-position dyad-pair add ([`apply_dyad_pair`]); this round
//! answers *what each variant does with the resulting pixel DWORD* —
//! the per-row store shape that differs between the four variants the
//! codebook DWORD's two mode bits select:
//!
//! * Variant A ([`CellVariant::Plain`], §2.2): direct store, the same
//!   dyad-pair DWORD written to two adjacent rows (`[edi]` and
//!   `[edi + 0xb0]`) — vertical doubling, no saturation.
//! * Variant B ([`CellVariant::WithEdge`], §2.2): per-byte average of
//!   the predictor and the dyad delta with the `0x7f7f7f7f`
//!   7-bit-per-byte clamp ([`average_7bit`]).
//! * Variant C ([`CellVariant::DoubledRow`], §2.2): the variant-B
//!   average written to two adjacent rows (doubled-row store).
//! * Variant D ([`CellVariant::FullyDoubled`], §2.2): the
//!   `and 0xfefefefe; shr 1` per-byte halve ([`halve_fefefefe`])
//!   written to two adjacent rows.
//!
//! [`emit_variant`] runs [`apply_dyad_pair`] then applies the
//! variant-specific store shape, returning a [`VariantEmission`] that
//! names the output DWORD(s) and which rows receive them.
//!
//! What this round deliberately does **not** do (the spec/07 chapter
//! boundary on the output side, plus the multi-cell scope):
//!
//! * No cell-stack walk, no row-band advance, and no inter-cell edge
//!   fix-up (§1.3). The variant kernels here are the per-position store
//!   shape; the outer row/column loop that calls them per cell position
//!   (the `cl` / `ch` counter walk, spec/04 §3.3) is still future work.
//! * No strip-buffer allocation, no plane assembly, no 7-bit→8-bit
//!   upshift, and no YUV→RGB / IF09 conversion (§4.3, §5). Those are
//!   the output-buffer-write stage.
//! * No motion compensation (`spec/05`).
//! * No static dyad delta-table (`.data + 0x1003d088`) value
//!   interpretation beyond what spec/04's [`super::DyadDeltaTable`]
//!   already materialises (§3 here is the same table spec/04 §1.3
//!   vendored).
//!
//! The contract: given a 4-byte predictor DWORD (four horizontally
//! adjacent predictor pixels in softSIMD layout) and the primary /
//! optional secondary dyad entries the arena holds, this module
//! produces the output pixel-pair DWORD (or a fault) exactly as the
//! `add eax, [esi + 4*edx + 0x400]` chain at
//! `IR32_32.DLL!0x10006e0f..0x10006e2e` does.

use super::{CONTINUATION_XOR, CellVariant};

/// Spec/07 §0 / §1.1 — the strip pixel buffer's row stride, `0xb0`
/// (176) bytes. The predictor for `[edi]` is at `[edi - 0xb0]`.
pub const PREDICTOR_ROW_STRIDE: usize = 0xb0;

/// Spec/07 §1.3 / §7.4 — the top-of-strip predictor seed,
/// **fixture-arbitrated to `0x40`** (r451, overturning the
/// provisional zero-fill reading): the real all-intra IV32 fixture's
/// chroma planes reconstruct as uniform neutral `128` (internal
/// `0x40`) through repeat-row-above chains from the boundary, and its
/// luma top row is byte-exactly `0x40 + delta` for the first cell's
/// staged codebook delta (`0x40 - 54 = 10` → output `20`, the
/// reference decode's exact first-row value). The strip buffer is
/// therefore seeded with the internal 7-bit mid-range value `0x40`,
/// not zero — spec/07 §7.4's open question resolves to "the encoder
/// assumes a mid-range boundary".
pub const TOP_OF_STRIP_PREDICTOR: u8 = 0x40;

/// Spec/07 §4.2 — the internal pixel range is 7 bits per byte
/// (`0..=0x7f`). Bit 7 of every pixel byte is reserved as the
/// edge / boundary marker (set by the VQ_NULL `01` mark-skip path and
/// the `0xF8`/`0xF9`/`0xFA` RLE escapes, spec/06 §4.2).
pub const PIXEL_VALUE_MAX: u8 = 0x7f;

/// Spec/07 §4.2 — the edge / boundary marker bit (bit 7) reserved on
/// every internal pixel byte.
pub const EDGE_MARKER_BIT: u8 = 0x80;

/// Spec/07 §2.3 / §4.1 — the per-16-bit-half overflow sentinel mask.
///
/// A softSIMD DWORD holds two 16-bit halves, each carrying a 2-pixel
/// dyad-pair. Bit 15 of each half is the overflow / continuation
/// sentinel: after the `add eax, [primary]`, a set bit 15 in either
/// half means the primary table could not represent the delta in one
/// byte and the continuation path is taken (`xor eax, 0x80008000`).
pub const HALF_SENTINEL_MASK: u32 = 0x8000_8000;

/// Spec/07 §1.1 — the byte offset of the predictor for a write at byte
/// index `write_index` within the strip pixel buffer.
///
/// The predictor is the pixel one row above (`[edi - 0xb0]`). Returns
/// `None` when `write_index < PREDICTOR_ROW_STRIDE`, i.e. the write is
/// in the strip's top row and the predictor falls into the
/// pre-allocated padding region (where the seed is the constant
/// [`TOP_OF_STRIP_PREDICTOR`] per §1.3).
pub fn predictor_offset(write_index: usize) -> Option<usize> {
    write_index.checked_sub(PREDICTOR_ROW_STRIDE)
}

/// Spec/07 §2.3 / §4.1 — the overflow state of a softSIMD DWORD add.
///
/// After `predictor + primary_delta`, each 16-bit half's bit-15
/// sentinel is checked independently. The decoder's `jns` at
/// `IR32_32.DLL!0x10006e16` tests bit 31 (the *high* half's sentinel);
/// the low half's sentinel is consulted by the subsequent `js` after
/// the 16-bit `add ax` in the continuation path. This type records
/// both so the per-position kernel can reproduce the exact branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SoftSimdSum {
    /// The raw 32-bit sum (`predictor + primary_delta`), wrapping.
    pub raw: u32,
    /// Bit 15 of the low 16-bit half (`raw & 0x0000_8000`).
    pub low_half_overflow: bool,
    /// Bit 15 of the high 16-bit half (`raw & 0x8000_0000`, i.e.
    /// bit 31). The `jns 0x10006e2f` tests this bit.
    pub high_half_overflow: bool,
}

impl SoftSimdSum {
    /// Spec/07 §2.1 — form `predictor + primary_delta` as a wrapping
    /// 32-bit add and record both halves' sentinel bits.
    pub fn add(predictor: u32, primary_delta: u32) -> Self {
        let raw = predictor.wrapping_add(primary_delta);
        SoftSimdSum {
            raw,
            low_half_overflow: raw & 0x0000_8000 != 0,
            high_half_overflow: raw & 0x8000_0000 != 0,
        }
    }

    /// Spec/07 §2.3 — `true` if *either* 16-bit half has its bit-15
    /// sentinel set, i.e. the documented "continuation needed"
    /// condition (§2.3 / §4.1). This is the per-half semantic; the
    /// literal `jns` instruction tests only [`Self::high_half_overflow`]
    /// (see [`jns_taken`]).
    pub fn any_half_overflow(self) -> bool {
        self.low_half_overflow || self.high_half_overflow
    }
}

/// Spec/07 §2.1 / §2.3 — the literal `jns 0x10006e2f` test at
/// `IR32_32.DLL!0x10006e16`: the jump (no continuation) is taken when
/// the 32-bit sum's sign bit (bit 31, the high half's sentinel) is
/// **clear**. This mirrors [`super::continuation_needed`] exactly —
/// kept here so the reconstruction kernel can document the instruction
/// it reproduces without reaching across modules.
pub fn jns_taken(sum: u32) -> bool {
    sum & 0x8000_0000 == 0
}

/// Spec/07 §2 — the outcome of applying a dyad-pair codebook entry to
/// the predictor at one cell position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DyadOutcome {
    /// The primary-table add stayed in range (the high-half sentinel
    /// was clear, `jns` taken). The output pixel-pair DWORD is the raw
    /// sum; no continuation byte was consumed.
    Primary {
        /// The output pixel-pair DWORD written to `[edi]`.
        pixels: u32,
    },
    /// The primary-table add overflowed; the continuation byte drove a
    /// secondary-table add that stayed in range. The output DWORD is
    /// the post-`xor`/post-secondary-add value; one continuation byte
    /// was consumed (`inc ebp`).
    Continuation {
        /// The output pixel-pair DWORD written to `[edi]`.
        pixels: u32,
    },
    /// Both the primary and the secondary add left the low half's sign
    /// bit set; the decoder faults to error code 2 at
    /// `IR32_32.DLL!0x1000855f` (§2.3 step 3, §4.1). An encoder that
    /// emits such a sequence produces a bitstream the decoder rejects.
    Fault,
}

/// Spec/07 §2.1 / §2.3 / §4.1 — apply a dyad-pair codebook entry to a
/// predictor DWORD, reproducing the inner-loop body at
/// `IR32_32.DLL!0x10006e0f..0x10006e2e`:
///
/// ```text
/// mov eax, [edi - 0xb0]              ; predictor DWORD
/// add eax, [esi + 4*edx + 0x400]     ; predictor + primary delta
/// jns 0x10006e2f                     ; if no overflow → Primary
/// mov dl, [ebp + 0x1]                ; read continuation byte
/// xor eax, 0x80008000                ; flip the two half-high bits
/// add ax, [esi + 4*edx + 0x402]      ; add secondary 16-bit word
/// js  0x1000855f                     ; still negative → Fault
/// inc ebp                            ; consume continuation byte
/// mov [edi], eax                     ; → Continuation
/// ```
///
/// `predictor` is the 4-byte row-above DWORD (`[edi - 0xb0]`).
/// `primary_delta` is the per-frame-arena primary-table DWORD at
/// `[esi + 4*edx + 0x400]` (spec/04 §2.1; the arena lookup itself is
/// the caller's job). `secondary_word` is the low-16-bit secondary
/// word at `[esi + 4*edx + 0x402]`, consulted only on a continuation.
///
/// Returns a [`DyadOutcome`] describing which path the decoder took
/// and the resulting pixel-pair DWORD; the caller stores `pixels` to
/// `[edi]` and, on [`DyadOutcome::Continuation`], advances the
/// bitstream cursor by one (`inc ebp`).
pub fn apply_dyad_pair(predictor: u32, primary_delta: u32, secondary_word: u16) -> DyadOutcome {
    // `add eax, [primary]`.
    let sum = predictor.wrapping_add(primary_delta);
    // `jns 0x10006e2f`: no high-half overflow → take the primary path.
    if jns_taken(sum) {
        return DyadOutcome::Primary { pixels: sum };
    }
    // Continuation path. `xor eax, 0x80008000` backs out the two
    // half-high sentinel bits before the secondary add (§2.3 step 1).
    let backed_out = sum ^ CONTINUATION_XOR;
    // `add ax, [secondary]`: a *16-bit* add affecting only the low
    // half (§2.3 step 2). The high half is preserved.
    let low = (backed_out & 0x0000_ffff) as u16;
    let new_low = low.wrapping_add(secondary_word);
    let combined = (backed_out & 0xffff_0000) | (new_low as u32);
    // `js 0x1000855f`: if the low half's sign bit (bit 15) is still
    // set after the secondary add, fault to error code 2 (§2.3 step 3,
    // §4.1). The `js` here tests the 16-bit result's sign, i.e. bit 15
    // of the low half.
    if new_low & 0x8000 != 0 {
        return DyadOutcome::Fault;
    }
    // `inc ebp` (continuation byte consumed); `mov [edi], eax`.
    DyadOutcome::Continuation { pixels: combined }
}

/// Spec/07 §1.1 / §0 — pack four horizontally adjacent predictor
/// pixel bytes into the little-endian softSIMD DWORD the inner loop
/// loads with `mov eax, [edi - 0xb0]`.
///
/// The four bytes are laid out low-to-high: `bytes[0]` is the leftmost
/// pixel (the low byte of the DWORD), matching the x86 little-endian
/// `mov eax, [mem]` load of four consecutive buffer bytes.
pub fn pack_predictor(bytes: [u8; 4]) -> u32 {
    u32::from_le_bytes(bytes)
}

/// Spec/07 §2.4 — unpack an output pixel-pair / quad DWORD back into
/// its four pixel bytes in raster (left-to-right) order, the inverse
/// of [`pack_predictor`]. The dyad iteration order (`wiki/Indeo_3.wiki`
/// §"VQ data codes") writes pixels left-to-right within the row, so
/// `bytes[0]` is the leftmost emitted pixel.
pub fn unpack_pixels(dword: u32) -> [u8; 4] {
    dword.to_le_bytes()
}

/// Spec/07 §4.2 — the per-byte 7-bit clamp mask `0x7f7f7f7f` applied by
/// the averaging variants (B / C) before storing the DWORD (spec/04
/// §2.2: "`and 0x7f7f7f7f` mask is applied to the DWORD result before
/// storing"). Dropping bit 7 of each byte keeps the result in the
/// internal 7-bit-per-byte pixel range and clears the edge-marker bit.
pub const CLAMP_7BIT_MASK: u32 = 0x7f7f_7f7f;

/// Spec/07 §2.2 — the per-byte carry-strip mask `0xfefefefe` applied by
/// the fully-doubled variant (D) before the `shr 1` halve. Clearing
/// bit 0 of each byte before the shared right-shift prevents the low
/// bit of one byte bleeding into the high bit of the next during the
/// byte-parallel DWORD `shr`.
pub const HALVE_CARRY_MASK: u32 = 0xfefe_fefe;

/// Spec/07 §2.2 (variant B / C) — the per-byte average of two pixel
/// DWORDs with the `0x7f7f7f7f` 7-bit clamp.
///
/// The averaging variants form `(predictor + delta)` (already done by
/// [`apply_dyad_pair`]); the variant-specific step is a per-byte
/// average against the predictor, clamped to 7 bits per byte. The
/// byte-parallel average without inter-byte carry bleed is the SWAR
/// identity `(a & b) + (((a ^ b) >> 1) & 0x7f7f7f7f)`; masking the
/// half-sum with `0x7f7f7f7f` both performs the per-byte `>> 1` carry
/// strip and applies the variant's documented `and 0x7f7f7f7f` clamp
/// (spec/04 §2.2).
pub fn average_7bit(a: u32, b: u32) -> u32 {
    (a & b).wrapping_add(((a ^ b) >> 1) & CLAMP_7BIT_MASK)
}

/// Spec/07 §2.2 (variant D) — the `and 0xfefefefe; shr 1` per-byte
/// halve.
///
/// The fully-doubled variant clears bit 0 of each byte (so the right
/// shift cannot pull the low bit of one byte into the high bit of the
/// next) and then shifts the whole DWORD right by 1, halving every byte
/// in parallel. This is the literal disassembled sequence at the
/// variant-D inner loop (`and 0xfefefefe; shr 1`).
pub fn halve_fefefefe(value: u32) -> u32 {
    (value & HALVE_CARRY_MASK) >> 1
}

/// Spec/07 §2.2 — the result of applying one cell-shape variant's
/// inner-loop store at a single cell position.
///
/// Every variant first runs the shared [`apply_dyad_pair`] add; the
/// variant then decides *what* to store and *to how many rows*. This
/// type captures both: `outcome` is the dyad-pair add result (so a
/// caller can detect the [`DyadOutcome::Fault`] / continuation-byte
/// cases), and `rows` is the list of output DWORDs to store at
/// successive `0xb0`-stride row offsets from the destination pointer
/// `[edi]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantEmission {
    /// The shared per-position dyad-pair add outcome ([`apply_dyad_pair`]).
    /// On [`DyadOutcome::Fault`] the variant store does not run and
    /// `rows` is empty.
    pub outcome: DyadOutcome,
    /// The output pixel-pair DWORD(s) to store, in row order. `rows[i]`
    /// is written at `[edi + i * 0xb0]`. The plain / doubled-row /
    /// fully-doubled variants emit two rows (vertical doubling); the
    /// with-edge variant emits one row.
    pub rows: RowEmission,
}

/// A small inline row list (max 2 entries — no variant emits more than
/// two rows per dyad-pair position per spec/07 §2.2). Kept as a fixed
/// array + length so the module stays allocation-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RowEmission {
    storage: [u32; 2],
    len: usize,
}

impl RowEmission {
    fn push(&mut self, v: u32) {
        self.storage[self.len] = v;
        self.len += 1;
    }

    /// The stored output DWORDs in row order.
    pub fn as_slice(&self) -> &[u32] {
        &self.storage[..self.len]
    }

    /// Number of output rows this position emits.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether no rows were emitted (the [`DyadOutcome::Fault`] case).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Spec/07 §2.2 / spec/04 §2.2 — apply one cell-shape variant's
/// inner-loop store at a single cell position.
///
/// Runs the shared [`apply_dyad_pair`] add against the predictor, then
/// applies the variant-specific output shape:
///
/// * [`CellVariant::Plain`] (variant A, `IR32_32.DLL!0x1000670d`):
///   direct DWORD store written to **two** adjacent rows (`[edi]` and
///   `[edi + 0xb0]`) — vertical doubling, no saturation.
/// * [`CellVariant::WithEdge`] (variant B, `IR32_32.DLL!0x10006780`):
///   per-byte [`average_7bit`] of the predictor and the dyad-pair
///   result, written to **one** row.
/// * [`CellVariant::DoubledRow`] (variant C, `IR32_32.DLL!0x1000684b`):
///   the variant-B average written to **two** adjacent rows
///   (doubled-row store).
/// * [`CellVariant::FullyDoubled`] (variant D,
///   `IR32_32.DLL!0x100068f8`): the [`halve_fefefefe`]
///   (`and 0xfefefefe; shr 1`) halve of the dyad-pair result, written
///   to **two** adjacent rows.
///
/// `predictor` is the row-above DWORD (`[edi - 0xb0]`), `primary_delta`
/// the per-frame arena primary entry, and `secondary_word` the
/// secondary-table word consulted only on a continuation (see
/// [`apply_dyad_pair`]). On [`DyadOutcome::Fault`] no rows are emitted.
pub fn emit_variant(
    variant: CellVariant,
    predictor: u32,
    primary_delta: u32,
    secondary_word: u16,
) -> VariantEmission {
    let outcome = apply_dyad_pair(predictor, primary_delta, secondary_word);
    let pixels = match outcome {
        DyadOutcome::Primary { pixels } | DyadOutcome::Continuation { pixels } => pixels,
        DyadOutcome::Fault => {
            return VariantEmission {
                outcome,
                rows: RowEmission::default(),
            };
        }
    };

    let mut rows = RowEmission::default();
    match variant {
        // Variant A (plain): direct store, written to two rows.
        CellVariant::Plain => {
            rows.push(pixels);
            rows.push(pixels);
        }
        // Variant B (with-edge averaging): per-byte average with the
        // 7-bit clamp, one row.
        CellVariant::WithEdge => {
            rows.push(average_7bit(predictor, pixels));
        }
        // Variant C (doubled-row): the with-edge average, two rows.
        CellVariant::DoubledRow => {
            let avg = average_7bit(predictor, pixels);
            rows.push(avg);
            rows.push(avg);
        }
        // Variant D (fully-doubled): the `and 0xfefefefe; shr 1` halve,
        // two rows.
        CellVariant::FullyDoubled => {
            let halved = halve_fefefefe(pixels);
            rows.push(halved);
            rows.push(halved);
        }
    }

    VariantEmission { outcome, rows }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn predictor_stride_and_seed_constants() {
        assert_eq!(PREDICTOR_ROW_STRIDE, 0xb0);
        assert_eq!(PREDICTOR_ROW_STRIDE, 176);
        assert_eq!(TOP_OF_STRIP_PREDICTOR, 0x40);
        assert_eq!(PIXEL_VALUE_MAX, 0x7f);
        assert_eq!(EDGE_MARKER_BIT, 0x80);
        assert_eq!(HALF_SENTINEL_MASK, 0x8000_8000);
    }

    #[test]
    fn predictor_offset_is_row_above() {
        // A write at byte 0xb0 reads the predictor at byte 0.
        assert_eq!(predictor_offset(0xb0), Some(0));
        // A write at byte 0x160 (2 rows down) reads byte 0xb0.
        assert_eq!(predictor_offset(0x160), Some(0xb0));
        // Top-row writes (< stride) have no in-buffer predictor: the
        // seed is the constant 0x00 (§1.3).
        assert_eq!(predictor_offset(0), None);
        assert_eq!(predictor_offset(PREDICTOR_ROW_STRIDE - 1), None);
    }

    #[test]
    fn softsimd_sum_records_both_half_sentinels() {
        // No overflow in either half.
        let s = SoftSimdSum::add(0x0001_0001, 0x0002_0002);
        assert_eq!(s.raw, 0x0003_0003);
        assert!(!s.low_half_overflow);
        assert!(!s.high_half_overflow);
        assert!(!s.any_half_overflow());

        // Low-half-only overflow (bit 15 of the low half set).
        let s = SoftSimdSum::add(0x0000_7fff, 0x0000_0001);
        assert_eq!(s.raw, 0x0000_8000);
        assert!(s.low_half_overflow);
        assert!(!s.high_half_overflow);
        assert!(s.any_half_overflow());

        // High-half-only overflow (bit 31 set).
        let s = SoftSimdSum::add(0x7fff_0000, 0x0001_0000);
        assert_eq!(s.raw, 0x8000_0000);
        assert!(!s.low_half_overflow);
        assert!(s.high_half_overflow);
        assert!(s.any_half_overflow());
    }

    #[test]
    fn jns_taken_matches_high_half_sentinel() {
        // jns is taken (no continuation) when bit 31 is clear.
        assert!(jns_taken(0x0000_0000));
        assert!(jns_taken(0x7fff_ffff));
        assert!(!jns_taken(0x8000_0000));
        assert!(!jns_taken(0xffff_ffff));
        // It is exactly the inverse of spec/06's continuation_needed.
        for v in [0x0000_0000u32, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff] {
            assert_eq!(jns_taken(v), !super::super::continuation_needed(v));
        }
    }

    #[test]
    fn primary_path_when_high_half_in_range() {
        // predictor + primary stays with bit 31 clear → Primary, raw
        // sum, no continuation byte.
        let out = apply_dyad_pair(0x0010_0010, 0x0005_0005, 0xffff);
        assert_eq!(
            out,
            DyadOutcome::Primary {
                pixels: 0x0015_0015
            }
        );
    }

    #[test]
    fn primary_path_ignores_secondary_word() {
        // When the primary add does not overflow, the secondary word is
        // never consulted (the continuation byte is not consumed).
        let a = apply_dyad_pair(0x0001_0001, 0x0001_0001, 0x0000);
        let b = apply_dyad_pair(0x0001_0001, 0x0001_0001, 0x7fff);
        assert_eq!(a, b);
        assert_eq!(
            a,
            DyadOutcome::Primary {
                pixels: 0x0002_0002
            }
        );
    }

    #[test]
    fn continuation_path_backs_out_and_adds_secondary() {
        // Force a high-half overflow so the continuation path runs.
        // predictor 0x7fff_0000 + primary 0x0001_0000 = 0x8000_0000:
        // bit 31 set → continuation.
        // xor 0x80008000 → 0x0000_8000. Low half = 0x8000.
        // add ax, secondary: pick secondary so the low half clears its
        // sign bit. 0x8000 + 0x8000 = 0x0000 (wrapping), bit 15 clear.
        let out = apply_dyad_pair(0x7fff_0000, 0x0001_0000, 0x8000);
        // backed_out high half = 0x0000; low half 0x8000 + 0x8000 = 0.
        assert_eq!(
            out,
            DyadOutcome::Continuation {
                pixels: 0x0000_0000
            }
        );
    }

    #[test]
    fn continuation_preserves_high_half() {
        // High half after xor must survive the 16-bit secondary add.
        // predictor 0x1234_0000 + primary 0x8000_0000 = 0x9234_0000:
        // bit 31 set → continuation. xor 0x80008000 → 0x1234_8000.
        // low 0x8000 + secondary 0x8001 = 0x0001 (bit 15 clear).
        let out = apply_dyad_pair(0x1234_0000, 0x8000_0000, 0x8001);
        assert_eq!(
            out,
            DyadOutcome::Continuation {
                pixels: 0x1234_0001
            }
        );
    }

    #[test]
    fn fault_when_secondary_still_sign_set() {
        // Continuation path, but the secondary add leaves bit 15 set →
        // Fault (error code 2).
        // predictor 0x7fff_0000 + primary 0x0001_0000 = 0x8000_0000.
        // xor → 0x0000_8000, low = 0x8000. secondary 0x0001:
        // 0x8000 + 0x0001 = 0x8001, bit 15 still set → Fault.
        let out = apply_dyad_pair(0x7fff_0000, 0x0001_0000, 0x0001);
        assert_eq!(out, DyadOutcome::Fault);
    }

    #[test]
    fn pack_and_unpack_predictor_round_trip() {
        let bytes = [0x12u8, 0x34, 0x56, 0x70];
        let dword = pack_predictor(bytes);
        // Little-endian: leftmost pixel is the low byte.
        assert_eq!(dword, 0x7056_3412);
        assert_eq!(unpack_pixels(dword), bytes);
    }

    #[test]
    fn realistic_in_range_dyad_pair() {
        // Two adjacent predictor pixels 0x20, 0x30 in the low half; the
        // primary delta adds 0x05, 0x07. softSIMD: low half holds
        // (pixel1<<8)|pixel0 = 0x3020; delta 0x0705 → 0x3725. Stays in
        // 7-bit-per-byte range (no bit-7 carry), no overflow.
        let predictor = pack_predictor([0x20, 0x30, 0x00, 0x00]);
        // primary delta in matching softSIMD layout.
        let primary = pack_predictor([0x05, 0x07, 0x00, 0x00]);
        let out = apply_dyad_pair(predictor, primary, 0xffff);
        match out {
            DyadOutcome::Primary { pixels } => {
                let p = unpack_pixels(pixels);
                assert_eq!(p[0], 0x25);
                assert_eq!(p[1], 0x37);
                // Pixel values stay within the 7-bit range.
                assert!(p[0] <= PIXEL_VALUE_MAX);
                assert!(p[1] <= PIXEL_VALUE_MAX);
            }
            other => panic!("expected Primary, got {other:?}"),
        }
    }

    #[test]
    fn variant_masks_match_spec() {
        // The two documented per-byte masks (spec/04 §2.2, spec/07 §2.2).
        assert_eq!(CLAMP_7BIT_MASK, 0x7f7f_7f7f);
        assert_eq!(HALVE_CARRY_MASK, 0xfefe_fefe);
    }

    #[test]
    fn average_7bit_is_per_byte_floor_average() {
        // Per-byte average with no inter-byte carry bleed. Each byte
        // pair averages independently, rounding toward zero.
        let a = pack_predictor([0x10, 0x40, 0x7f, 0x00]);
        let b = pack_predictor([0x20, 0x42, 0x01, 0x7f]);
        let avg = unpack_pixels(average_7bit(a, b));
        // floor((0x10 + 0x20) / 2) = 0x18.
        assert_eq!(avg[0], 0x18);
        // floor((0x40 + 0x42) / 2) = 0x41.
        assert_eq!(avg[1], 0x41);
        // floor((0x7f + 0x01) / 2) = 0x40.
        assert_eq!(avg[2], 0x40);
        // floor((0x00 + 0x7f) / 2) = 0x3f.
        assert_eq!(avg[3], 0x3f);
        // Every output byte stays within the 7-bit pixel range.
        for v in avg {
            assert!(v <= PIXEL_VALUE_MAX);
        }
    }

    #[test]
    fn average_7bit_clamps_bit7() {
        // For equal inputs the average is exact (the xor term is zero).
        assert_eq!(average_7bit(0xffff_ffff, 0xffff_ffff), 0xffff_ffff);
        // For mixed inputs the `0x7f7f7f7f` mask strips the carry out of
        // bit 7: (0x80 ^ 0x00) >> 1 = 0x40, & 0x7f = 0x40.
        let avg = unpack_pixels(average_7bit(0x8080_8080, 0x0000_0000));
        for v in avg {
            assert_eq!(v, 0x40);
        }
    }

    #[test]
    fn halve_fefefefe_is_per_byte_halve() {
        // `and 0xfefefefe; shr 1`: clear bit 0 of each byte, then halve
        // the whole DWORD. No low bit bleeds across byte boundaries.
        let v = pack_predictor([0x81, 0x40, 0xff, 0x01]);
        let halved = unpack_pixels(halve_fefefefe(v));
        // 0x81 & 0xfe = 0x80, >> 1 = 0x40.
        assert_eq!(halved[0], 0x40);
        // 0x40 & 0xfe = 0x40, >> 1 = 0x20.
        assert_eq!(halved[1], 0x20);
        // 0xff & 0xfe = 0xfe, >> 1 = 0x7f.
        assert_eq!(halved[2], 0x7f);
        // 0x01 & 0xfe = 0x00, >> 1 = 0x00.
        assert_eq!(halved[3], 0x00);
        // No bleed across bytes: byte 0's cleared low bit does not reach
        // byte 1's high bit.
        assert_eq!(halve_fefefefe(0x0000_0001), 0x0000_0000);
        assert_eq!(halve_fefefefe(0x0000_0100), 0x0000_0000);
    }

    #[test]
    fn variant_plain_writes_two_identical_rows() {
        // Variant A: direct store, same DWORD to both rows.
        let predictor = pack_predictor([0x20, 0x30, 0x00, 0x00]);
        let primary = pack_predictor([0x05, 0x07, 0x00, 0x00]);
        let em = emit_variant(CellVariant::Plain, predictor, primary, 0xffff);
        assert_eq!(
            em.outcome,
            DyadOutcome::Primary {
                pixels: 0x0000_3725
            }
        );
        assert_eq!(em.rows.len(), 2);
        assert_eq!(em.rows.as_slice(), &[0x0000_3725, 0x0000_3725]);
    }

    #[test]
    fn variant_with_edge_averages_one_row() {
        // Variant B: per-byte average of predictor and dyad result, one
        // row. dyad result = [0x25, 0x37, 0, 0]; average with predictor
        // [0x20, 0x30, 0, 0]: (0x20+0x25)/2 = 0x22, (0x30+0x37)/2 = 0x33.
        let predictor = pack_predictor([0x20, 0x30, 0x00, 0x00]);
        let primary = pack_predictor([0x05, 0x07, 0x00, 0x00]);
        let em = emit_variant(CellVariant::WithEdge, predictor, primary, 0xffff);
        assert_eq!(em.rows.len(), 1);
        let row = unpack_pixels(em.rows.as_slice()[0]);
        assert_eq!(row[0], (0x20 + 0x25) / 2);
        assert_eq!(row[1], (0x30 + 0x37) / 2);
    }

    #[test]
    fn variant_doubled_row_averages_two_rows() {
        // Variant C: the with-edge average, written to two rows.
        let predictor = pack_predictor([0x20, 0x30, 0x00, 0x00]);
        let primary = pack_predictor([0x05, 0x07, 0x00, 0x00]);
        let b = emit_variant(CellVariant::WithEdge, predictor, primary, 0xffff);
        let c = emit_variant(CellVariant::DoubledRow, predictor, primary, 0xffff);
        assert_eq!(c.rows.len(), 2);
        // Both rows equal the variant-B single-row result.
        assert_eq!(c.rows.as_slice()[0], b.rows.as_slice()[0]);
        assert_eq!(c.rows.as_slice()[1], b.rows.as_slice()[0]);
    }

    #[test]
    fn variant_fully_doubled_halves_two_rows() {
        // Variant D: `and 0xfefefefe; shr 1` of the dyad result, two
        // rows. dyad result = [0x44, 0x66, 0, 0]; halve = [0x22, 0x33].
        let predictor = pack_predictor([0x40, 0x60, 0x00, 0x00]);
        let primary = pack_predictor([0x04, 0x06, 0x00, 0x00]);
        let em = emit_variant(CellVariant::FullyDoubled, predictor, primary, 0xffff);
        assert_eq!(em.rows.len(), 2);
        let row = unpack_pixels(em.rows.as_slice()[0]);
        assert_eq!(row[0], 0x44 >> 1);
        assert_eq!(row[1], 0x66 >> 1);
        assert_eq!(em.rows.as_slice()[0], em.rows.as_slice()[1]);
    }

    #[test]
    fn variant_fault_emits_no_rows() {
        // On a dyad-pair fault, every variant emits zero rows and
        // propagates the Fault outcome.
        for variant in [
            CellVariant::Plain,
            CellVariant::WithEdge,
            CellVariant::DoubledRow,
            CellVariant::FullyDoubled,
        ] {
            let em = emit_variant(variant, 0x7fff_0000, 0x0001_0000, 0x0001);
            assert_eq!(em.outcome, DyadOutcome::Fault);
            assert!(em.rows.is_empty());
            assert_eq!(em.rows.len(), 0);
        }
    }

    #[test]
    fn variant_propagates_continuation_outcome() {
        // The variant store runs on the continuation pixels and the
        // outcome reports Continuation (so the caller advances the
        // bitstream cursor).
        let em = emit_variant(CellVariant::Plain, 0x1234_0000, 0x8000_0000, 0x8001);
        assert_eq!(
            em.outcome,
            DyadOutcome::Continuation {
                pixels: 0x1234_0001
            }
        );
        assert_eq!(em.rows.as_slice(), &[0x1234_0001, 0x1234_0001]);
    }
}
