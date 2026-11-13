//! Indeo 3 VQ codebook materialisation (per-plane banks + per-frame
//! arena + static delta tables).
//!
//! Spec source: `docs/video/indeo/indeo3/spec/04-vq-codebooks.md`.
//!
//! Round 4 lands the codebook system that the spec/03 VQ_DATA leaf
//! indices reference. It materialises the static + per-frame
//! codebook resources spec/04 documents and resolves a packed
//! codebook entry into the structural pieces the downstream
//! per-cell unpacker consumes:
//!
//! * §1.3 / §2.3 — the static dyad-mode delta table at
//!   `.data + 0x1003d088` (8 KB, 16 banks × 512 B), vendored
//!   verbatim from `tables/region_1003d088.hex` and indexed
//!   `(high_nibble << 9) + col` per the dyad handler at
//!   `IR32_32.DLL!0x10006c2c`. The audit-noted bank-15 row
//!   restriction (§1.3) is surfaced as [`DYAD_BANK15_VALID_ROWS`].
//! * §2.1 — the packed codebook DWORD format: mode bit 0 / bit 1
//!   selecting one of four cell-unpacker variants, and bits 2..31
//!   as a signed (`sar 2`) byte offset into the per-frame arena.
//!   [`CodebookEntry::decode`] performs that split.
//! * §5.1 — the static codebook seed table at `.data + 0x1003ed4c`
//!   (258 B, 129 byte-pairs) and the codec-init routine at
//!   `IR32_32.DLL!0x10006262` that packs each pair into a
//!   512-scaled DWORD ([`seed_dispatch_entries`]), plus the
//!   materialisation of the codec-init-built cell-state dispatch
//!   tables ([`SeedDispatchTables`]) that the spec/04 §5.1 init
//!   function writes from that seed. Audit/00 §4 established that
//!   `DllMain` runs Path 1 (`0x10006262`), so this reproduces the
//!   actually-executed packing for the three **low-half**-stream
//!   tables (`0x1003f24c`, `0x1003f94c`, `0x1003f950`). The three
//!   **high-half**-stream tables (`0x1003f44c`, `0x1003fd4c`,
//!   `0x1003fd50`) source from seed offset `+0x100`; only the single
//!   in-bounds pair is determinable from the 258-byte extract
//!   (audit/00 §2.2) — the rest is a deferred DOCS-GAP.
//! * §1.2 / §6 — the per-frame VQ arena layout
//!   (`*(inner_instance + 0x3004) + 0x800..+0x8800`, 16 bands ×
//!   2 KB) and the `alt_quant[]` band-selection overlay at
//!   `IR32_32.DLL!0x1000646a` ([`VqArena::apply_alt_quant`]).
//! * §4 — the VQ_NULL runtime sub-bit semantics
//!   ([`VqNullRuntime`]).
//!
//! What this round deliberately does **not** do (the spec/04
//! chapter boundary, §0 / §8):
//!
//! * No per-byte mode-byte unpacking, dyad-pair → pixel-pair
//!   expansion, or RLE escape codes — those start at the per-byte
//!   unpacker entry `IR32_32.DLL!0x10006bac` and are
//!   `spec/06-entropy.md`'s subject. spec/04 explicitly defers the
//!   QUAD-mode escape table at `.data + 0x1004ccd4` to spec/06
//!   (§1.3) and ends at "dispatched into the unpacker with the
//!   base pointers loaded" (§8).
//! * No actual pixel reconstruction or 7-bit→8-bit upshift —
//!   `spec/07`.
//! * No motion compensation — `spec/05`.
//!
//! The contract this module provides is the *materialised codebook
//! state* plus the *index → structural-delta resolution* that
//! spec/04 pins down; the structural-delta → pixel emission is the
//! next chapter's job.

/// Spec/04 §1.3 — the static dyad-mode delta table is 8 KB.
pub const DYAD_TABLE_LEN: usize = 8192;

/// Spec/04 §1.3 / §2.3 — the dyad table is 16 high-nibble banks,
/// each 512 bytes (`high_nibble << 9` stride at the handler).
pub const DYAD_BANK_COUNT: usize = 16;

/// Spec/04 §2.3 — per-bank stride in bytes (`<< 9`).
pub const DYAD_BANK_STRIDE: usize = 512;

/// Spec/04 §1.3 audit-correction (audit/00 §3.2) — bank 15 of the
/// dyad table only has 65 of its 128 pair-rows populated (the
/// `region_1003d088_dyads.csv` analysis shows pair-rows 65..127 are
/// all-zero). The `(high_nibble << 9) + row*4 + col` indexing for
/// `high_nibble == 15` must therefore restrict the row-index subset
/// to the first [`DYAD_BANK15_VALID_ROWS`] rows.
pub const DYAD_BANK15_VALID_ROWS: usize = 65;

/// Spec/04 §6 — the codebook-band region the per-frame overlay
/// writes. The §6.1 overlay starts at `arena + 0x800` and advances
/// `edi` by `0x800` per band for 16 bands, so it spans
/// `0x800..0x8800` (32 KB). We size the arena to that span so the
/// overlay's full output range is addressable.
///
/// NOTE — spec contradiction (reported as a DOCS-GAP): §1.2 states
/// the heap block is `0x8020` bytes with the `cb_offset`-biased
/// base-pointer slot read at `[arena + 0x8000]`, which is
/// incompatible with a 16-band region running to `0x8800` (band 15's
/// The corrected layout (round 17, `tables/03-vq-staging-words.meta`)
/// is exactly the binary's 0x8020-byte allocation: the 32 KB band
/// region at `+0x0000..+0x7fff` (16 bands × 2 KB, band `i` at
/// `+0x800·i`), the `+0x8000` staging-pointer slot and the register
/// stash in the 0x20-byte tail. We model the band region in-buffer;
/// the `cb_offset`-biased staging base the binary stashes at
/// `+0x8000` is returned by [`VqArena::apply_alt_quant`].
pub const ARENA_LEN: usize = 0x8020;

/// Spec/04 §1.2 / §6.3 (corrected, Extractor round 17) — the
/// per-band codebook region starts at the **arena base** (band `i`
/// at `+0x800·i`, no leading skip; the earlier `+0x800` bands offset
/// is withdrawn as arithmetically impossible — it would put band 15
/// past the 0x8020-byte allocation).
pub const ARENA_BANDS_OFFSET: usize = 0;

/// Spec/04 §1.2 — there are 16 per-band codebook sub-tables (one per
/// `alt_quant[]` byte).
pub const ARENA_BAND_COUNT: usize = 16;

/// Spec/04 §1.2 / §2.1 — each per-band sub-table is 2 KB (1 KB
/// primary + 1 KB secondary).
pub const ARENA_BAND_LEN: usize = 0x800;

/// Spec/04 §2.1 — the primary / secondary halves are each 1 KB
/// (256 codebook DWORDs).
pub const ARENA_HALF_LEN: usize = 0x400;

/// Spec/04 §6.2 (corrected) — both `alt_quant[]` nibbles index the
/// staging image's blocks at the same 0x800-byte stride (the earlier
/// stride-128 "overlapping primary" reading is withdrawn).
pub const STAGING_SELECT_STRIDE: usize = 0x800;

/// Spec/04 §5.1 — the static seed table is 258 bytes (129 byte-pairs;
/// pairs 0..127 from offsets 0..255, pair 128 from offsets 256..257).
pub const SEED_TABLE_LEN: usize = 258;

/// Spec/04 §5.1 — number of byte-pairs the seed walker reads from the
/// low-half stream (`0x1003ed4c(,ecx,2)`), `ecx` running 0..127.
pub const SEED_PAIR_COUNT: usize = 128;

// Vendored, verbatim, from the docs clean-room table extracts. These
// are copies of `docs/video/indeo/indeo3/tables/region_*.hex` placed
// inside the crate so the published crate is self-contained (an
// `include_str!` of a path outside the crate root would not survive
// `cargo package`). Each file carries a `#`-prefixed provenance
// header; [`parse_hex_bytes`] skips comment lines.
const DYAD_TABLE_HEX: &str = include_str!("data/dyad_delta_1003d088.hex");
const SEED_TABLE_HEX: &str = include_str!("data/seed_dispatch_1003ed4c.hex");

/// Parse a whitespace-separated lower-hex byte dump, skipping any
/// line that begins with `#` (the vendored-file provenance header).
fn parse_hex_bytes(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        for tok in line.split_whitespace() {
            // Each token is exactly two hex digits.
            if let Ok(b) = u8::from_str_radix(tok, 16) {
                out.push(b);
            }
        }
    }
    out
}

/// Spec/04 §1.3 / §2.3 — the static dyad-mode delta table at
/// `.data + 0x1003d088`.
///
/// The dyad-mode handler at `IR32_32.DLL!0x10006c2c` reads
/// `[edx + eax + 0x1003d088]` where `eax = high_nibble << 9` and
/// `edx` is the per-row cell column offset. The byte read is a
/// pixel-pair delta written into the cell's pixel buffer. The
/// per-entry values are clean-room extracted; this type only
/// provides the indexing structure (the delta → pixel emission is
/// spec/06).
#[derive(Debug, Clone)]
pub struct DyadDeltaTable {
    bytes: Box<[u8; DYAD_TABLE_LEN]>,
}

impl DyadDeltaTable {
    /// Materialise the table from the vendored clean-room extract.
    ///
    /// Panics only if the vendored data file is the wrong length,
    /// which is a build-time invariant (the file is committed
    /// alongside this source).
    pub fn load() -> Self {
        let parsed = parse_hex_bytes(DYAD_TABLE_HEX);
        assert_eq!(
            parsed.len(),
            DYAD_TABLE_LEN,
            "vendored dyad delta table must be exactly {DYAD_TABLE_LEN} bytes"
        );
        let mut bytes = Box::new([0u8; DYAD_TABLE_LEN]);
        bytes.copy_from_slice(&parsed);
        DyadDeltaTable { bytes }
    }

    /// Raw table bytes (8 KB, VMA order).
    pub fn as_bytes(&self) -> &[u8; DYAD_TABLE_LEN] {
        &self.bytes
    }

    /// Spec/04 §2.3 — look up a delta byte by `(high_nibble, col)`.
    ///
    /// `high_nibble` is `(input_byte >> 4)` (0..15) and `col` is the
    /// per-row cell column offset (0..511 within the bank). Returns
    /// `None` if `col >= 512` (out of bank), or if
    /// `high_nibble == 15` and `col` addresses one of the unpopulated
    /// rows beyond [`DYAD_BANK15_VALID_ROWS`] (audit/00 §3.2): bank
    /// 15 is laid out as 128 pair-rows of 4 bytes, of which only the
    /// first 65 are populated, so a `col` of `row*4 + c` with
    /// `row >= 65` is rejected.
    pub fn delta(&self, high_nibble: u8, col: usize) -> Option<u8> {
        if high_nibble as usize >= DYAD_BANK_COUNT || col >= DYAD_BANK_STRIDE {
            return None;
        }
        if high_nibble as usize == DYAD_BANK_COUNT - 1 {
            let row = col / 4;
            if row >= DYAD_BANK15_VALID_ROWS {
                return None;
            }
        }
        let idx = (high_nibble as usize) * DYAD_BANK_STRIDE + col;
        Some(self.bytes[idx])
    }

    /// Spec/04 §2.3 — the 16-bank base offset for a high nibble
    /// (`high_nibble << 9`). Returns `None` for `high_nibble >= 16`.
    pub fn bank_base(high_nibble: u8) -> Option<usize> {
        if high_nibble as usize >= DYAD_BANK_COUNT {
            return None;
        }
        Some((high_nibble as usize) * DYAD_BANK_STRIDE)
    }

    /// Spec/07 §3.1 — the static-table index for the high-nibble-0
    /// row-band-advance handler (`IR32_32.DLL!0x10006c14`):
    /// `table_index = (high_nibble << 9) + row_position * 4 +
    /// column_offset`.
    ///
    /// Each high nibble selects a 512-byte sub-table; within it the row
    /// position × 4 plus the column offset selects one delta byte. The
    /// within-bank column is therefore `row_position * 4 +
    /// column_offset`; [`Self::delta`] applies the bank-15 row
    /// restriction. Returns `None` when the within-bank column would
    /// exceed the 512-byte bank stride.
    pub fn row_band_column(row_position: usize, column_offset: usize) -> Option<usize> {
        let col = row_position.checked_mul(4)?.checked_add(column_offset)?;
        if col >= DYAD_BANK_STRIDE {
            return None;
        }
        Some(col)
    }

    /// Spec/07 §3.1 / §3.2 — look up the signed delta byte for the
    /// high-nibble-0 row-band-advance handler.
    ///
    /// `high_nibble` selects the 512-byte sub-table; `row_position` and
    /// `column_offset` form the within-bank column per
    /// [`Self::row_band_column`]. The returned byte is a **signed
    /// delta** (spec/07 §3.2 — the codec-init `xor ah, -0x80` bias
    /// applies). Returns `None` when the index is out of range (or in
    /// bank 15's unpopulated rows, per [`Self::delta`]).
    pub fn row_band_delta(
        &self,
        high_nibble: u8,
        row_position: usize,
        column_offset: usize,
    ) -> Option<i8> {
        let col = Self::row_band_column(row_position, column_offset)?;
        self.delta(high_nibble, col).map(|b| b as i8)
    }
}

/// Spec/07 §3.2 — the result of the high-nibble-0 row-band-advance
/// handler (`IR32_32.DLL!0x10006c14..0x10006c34`).
///
/// Unlike a normal cell-data write, this handler writes a single signed
/// delta byte into the **predictor slot** at `[edi - 0xb0]` (the
/// row-above byte that seeds the next row's prediction), rather than
/// emitting a 2-pixel dyad at the current write position. The byte is
/// the static-table delta read from `.data + 0x1003d088`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowBandSeed {
    /// The signed delta byte read from the static dyad table.
    pub delta: i8,
    /// The byte index of the predictor slot the handler writes
    /// (`write_index - 0xb0`).
    pub predictor_slot: usize,
}

/// Spec/07 §3.1 / §3.2 — apply the high-nibble-0 row-band-advance
/// handler to a strip pixel buffer.
///
/// The handler reads a signed delta byte from the static dyad table at
/// `(high_nibble << 9) + row_position*4 + column_offset` (spec/07 §3.1)
/// and writes it into the predictor slot one row above the current
/// write position (`[edi - 0xb0]`, spec/07 §3.2), seeding the next
/// row's prediction. The stored byte is the delta with its `0x80`
/// sign-bias re-applied for the 7-bit-per-byte pixel range (the value
/// the buffer holds is the `xor`-biased unsigned form; spec/07 §3.2 /
/// §4).
///
/// Returns the [`RowBandSeed`] (delta + slot) on success, or `None`
/// when the table index is out of range or the predictor slot falls
/// above the strip (write within the first `0xb0`-byte row — there is
/// no row above to seed; spec/07 §1.3).
pub fn apply_row_band_seed(
    table: &DyadDeltaTable,
    strip: &mut [u8],
    write_index: usize,
    high_nibble: u8,
    row_position: usize,
    column_offset: usize,
) -> Option<RowBandSeed> {
    let delta = table.row_band_delta(high_nibble, row_position, column_offset)?;
    // `[edi - 0xb0]` — the predictor slot one row above (spec/07 §3.2).
    // The first row of a strip has no row above (spec/07 §1.3).
    let predictor_slot = write_index.checked_sub(super::PREDICTOR_ROW_STRIDE)?;
    if predictor_slot >= strip.len() {
        return None;
    }
    // The buffer stores the `0x80`-biased unsigned form (spec/07 §3.2 /
    // §4 — bit 7 of every pixel byte is the edge marker; the signed
    // delta is biased onto the unsigned storage range).
    strip[predictor_slot] = (delta as u8) ^ SEED_SIGN_BIAS_BYTE;
    Some(RowBandSeed {
        delta,
        predictor_slot,
    })
}

/// Spec/07 §3.2 / §4 — the `0x80` sign-bias re-applied when a signed
/// dyad delta is stored into the 7-bit-per-byte strip buffer (`xor ah,
/// -0x80` at `IR32_32.DLL!0x10006345`).
const SEED_SIGN_BIAS_BYTE: u8 = 0x80;

/// Spec/04 §2.1 — one of the four cell-unpacker variants selected by
/// the packed codebook DWORD's two mode bits.
///
/// The variant only controls *how* the dyad-pair is applied (direct
/// vs averaged vs row-doubled); the per-pixel emission body lives in
/// spec/06. We surface the variant so the spec/06 dispatcher can
/// route to the correct handler without re-decoding the mode bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellVariant {
    /// `(bit0, bit1) = (0, 0)` — "plain" dyad emission
    /// (`IR32_32.DLL!0x100066fb`). Direct DWORD store, no
    /// saturation (§2.2).
    Plain,
    /// `(bit0, bit1) = (1, 0)` — "with-edge" dyad emission
    /// (`IR32_32.DLL!0x10006759`). Averaging variant with the
    /// `0x7f7f7f7f` clamp (§2.2).
    WithEdge,
    /// `(bit0, bit1) = (0, 1)` — "doubled-row" emission
    /// (`IR32_32.DLL!0x1000682e`). Row-doubling + averaging.
    DoubledRow,
    /// `(bit0, bit1) = (1, 1)` — "fully-doubled" emission
    /// (`IR32_32.DLL!0x100068db`). Two-step averaging with the
    /// `0xfefefefe; shr 1` step.
    FullyDoubled,
}

impl CellVariant {
    /// Spec/04 §2.1 — derive the variant from the two low mode bits.
    pub fn from_mode_bits(bit0: bool, bit1: bool) -> Self {
        match (bit0, bit1) {
            (false, false) => CellVariant::Plain,
            (true, false) => CellVariant::WithEdge,
            (false, true) => CellVariant::DoubledRow,
            (true, true) => CellVariant::FullyDoubled,
        }
    }

    /// Whether this variant applies the `0x7f7f7f7f` 7-bit-per-byte
    /// clamp before storing (§2.2 — every variant except `Plain`).
    pub fn clamps_to_7bit(self) -> bool {
        !matches!(self, CellVariant::Plain)
    }
}

/// Spec/04 §2.1 / §3.1 — a decoded packed codebook entry.
///
/// At a VQ_DATA leaf the decoder reads one byte, indexes
/// `inner_instance[4*byte]` to fetch a packed 4-byte word, and
/// dispatches on it (§3.1). This type is the structured view of that
/// word: the two mode bits select the [`CellVariant`], and bits 2..31
/// are an arithmetic-shifted (signed) byte offset into the per-frame
/// arena (`sar edx, 0x2` at `IR32_32.DLL!0x100066f8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodebookEntry {
    /// The raw packed 4-byte word, little-endian as stored.
    pub packed: u32,
    /// Mode bit 0 (`packed & 0x1`).
    pub mode_bit0: bool,
    /// Mode bit 1 (`packed & 0x2`).
    pub mode_bit1: bool,
    /// The cell-unpacker variant selected by the two mode bits.
    pub variant: CellVariant,
    /// Bits 2..31 read as a signed arithmetic shift (`sar 2`):
    /// the byte offset into the per-frame arena that addresses the
    /// delta entry. Signed to preserve the relative-offset sign
    /// (§2.1).
    pub arena_offset: i32,
}

impl CodebookEntry {
    /// Spec/04 §2.1 — decode a packed codebook DWORD.
    pub fn decode(packed: u32) -> Self {
        let mode_bit0 = packed & 0x0000_0001 != 0;
        let mode_bit1 = packed & 0x0000_0002 != 0;
        // `sar edx, 0x2`: arithmetic right shift of the 32-bit word
        // (signed) drops the two mode bits and sign-extends.
        let arena_offset = (packed as i32) >> 2;
        CodebookEntry {
            packed,
            mode_bit0,
            mode_bit1,
            variant: CellVariant::from_mode_bits(mode_bit0, mode_bit1),
            arena_offset,
        }
    }
}

/// Spec/04 §5.1 — one entry of the static codebook seed-dispatch
/// table, packed by the codec-init routine at
/// `IR32_32.DLL!0x10006262`.
///
/// For each `ecx` in 0..127 the routine reads a low byte `al` and a
/// high byte `bl` from the seed table, packs `(al << 8) + bl`, and
/// scales by 512 (`<< 9`) before writing it into the six destination
/// dispatch tables. The bytes are read as signed (`movsx`, §5.1),
/// so we surface both the signed source bytes and the packed/scaled
/// DWORD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedEntry {
    /// `al` — the low source byte, read signed (`movsx`).
    pub lo: i8,
    /// `bl` — the high source byte, read signed (`movsx`).
    pub hi: i8,
    /// `(al << 8) + bl`, then `<< 9` (the scaled dispatch DWORD the
    /// init routine writes). Computed in `i32` to preserve the
    /// signed packing.
    pub packed: i32,
}

/// Spec/04 §5.1 — build the 128 packed seed-dispatch entries from the
/// static seed table at `.data + 0x1003ed4c`.
///
/// The init routine (`IR32_32.DLL!0x10006262`) reads the low-half
/// stream `0x1003ed4c(,ecx,2)` / `0x1003ed4d(,ecx,2)` for `ecx` in
/// 0..127. Each iteration packs `eax = (al << 8) + bl` then
/// `eax <<= 9`. The bytes are signed (`movsx`). The returned vector
/// holds the 128 entries in `ecx` order.
pub fn seed_dispatch_entries() -> Vec<SeedEntry> {
    let raw = parse_hex_bytes(SEED_TABLE_HEX);
    assert_eq!(
        raw.len(),
        SEED_TABLE_LEN,
        "vendored seed table must be exactly {SEED_TABLE_LEN} bytes"
    );
    let mut out = Vec::with_capacity(SEED_PAIR_COUNT);
    for ecx in 0..SEED_PAIR_COUNT {
        let lo = raw[2 * ecx] as i8;
        let hi = raw[2 * ecx + 1] as i8;
        // `eax = (al << 8) + bl`, then `eax <<= 9`. The reference
        // packs `al` into the high byte and `bl` into the low byte
        // of a 16-bit value, then scales by 512. Sign comes from
        // the `movsx` reads.
        let packed16 = ((lo as i32) << 8) + (hi as i32);
        let packed = packed16 << 9;
        out.push(SeedEntry { lo, hi, packed });
    }
    out
}

/// Spec/04 §5.1 — number of dispatch records the codec-init routine
/// writes into each cell-state dispatch table (128, one per seed
/// pair; `ecx` decrements `0x7f → 0`).
pub const SEED_DISPATCH_RECORDS: usize = SEED_PAIR_COUNT;

/// Spec/04 §5.1 — the codec-init-built cell-state dispatch tables
/// rooted at `.data + 0x1003f24c` and its siblings.
///
/// Audit/00 §3.1 confirmed the six destination tables
/// (`0x1003f24c`, `0x1003f44c`, `0x1003f950`, `0x1003f94c`,
/// `0x1003fd50`, `0x1003fd4c`) are **zero on disk** and are built at
/// codec-init time by the static-table init function entered at
/// `IR32_32.DLL!0x100060de`. Audit/00 §4 further established that
/// `DllMain` calls that function once with `arg = 1`, so the
/// **actually-executed** population is Path 1 at
/// `IR32_32.DLL!0x10006262` — the path spec/04 §5.1 quotes
/// (`eax = (al << 8) + bl`, then `eax <<= 9`, the same packing
/// [`seed_dispatch_entries`] performs).
///
/// A clean-room decoder cannot load these tables as static numeric
/// blobs (they are zero on disk); it must reproduce the init
/// function's arithmetic. This type does that for the three
/// destination tables whose source is the **low-half** seed stream
/// (`0x1003ed4c(,ecx,2)` / `0x1003ed4d(,ecx,2)`), which is fully
/// determined by the vendored 258-byte seed:
///
/// * [`Self::table_f24c`] — `0x1003f24c`, 4-byte stride, one packed
///   DWORD per record (spec/04 §5.1 row 1).
/// * [`Self::table_f94c`] — `0x1003f94c` / `0x1003f950`, the 8-byte-
///   stride table whose two halves are written by the `0x1003f94c`
///   (`+0x0`) and `0x1003f950` (`+0x4`) stores. Both halves receive
///   the **same** packed DWORD (spec/04 §5.1 rows 2 + 3), so each
///   8-byte record is `[packed, packed]`.
///
/// The three **high-half**-stream tables (`0x1003f44c`,
/// `0x1003fd50`, `0x1003fd4c`; spec/04 §5.1 rows 4–6) are sourced
/// from `0x1003ee4c(,ecx,2)` = seed offset `+0x100`. Audit/00 §2.2
/// flags that the 258-byte extract only covers offsets `0x100..0x101`
/// (one in-bounds pair), so the high-half stream's record layout for
/// `ecx > 0` falls outside the extracted bytes and is left deferred
/// (see [`Self::high_half_pair0`] for the single in-bounds pair and
/// the module / report DOCS-GAP note).
#[derive(Clone)]
pub struct SeedDispatchTables {
    /// `0x1003f24c` — 128 packed DWORDs (4-byte stride).
    f24c: Box<[i32; SEED_DISPATCH_RECORDS]>,
    /// `0x1003f94c` / `0x1003f950` — 128 8-byte records, each
    /// `[packed_lo, packed_hi]` where both equal the seed pair's
    /// packed DWORD.
    f94c: Box<[[i32; 2]; SEED_DISPATCH_RECORDS]>,
}

impl core::fmt::Debug for SeedDispatchTables {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SeedDispatchTables")
            .field("records", &SEED_DISPATCH_RECORDS)
            .finish_non_exhaustive()
    }
}

impl Default for SeedDispatchTables {
    fn default() -> Self {
        Self::build()
    }
}

impl SeedDispatchTables {
    /// Spec/04 §5.1 — reproduce the codec-init Path-1 population of
    /// the low-half-stream cell-state dispatch tables from the
    /// vendored seed.
    pub fn build() -> Self {
        let entries = seed_dispatch_entries();
        let mut f24c = Box::new([0i32; SEED_DISPATCH_RECORDS]);
        let mut f94c = Box::new([[0i32; 2]; SEED_DISPATCH_RECORDS]);
        for (i, e) in entries.iter().enumerate() {
            // `0x1003f24c(,ecx,4) = packed` (4-byte stride; spec row 1).
            f24c[i] = e.packed;
            // `0x1003f950(,ecx,8) = packed` (`+0x4` half) and
            // `0x1003f94c(,ecx,8) = packed` (`+0x0` half) — both
            // halves of the 8-byte record get the same DWORD (spec
            // rows 2 + 3).
            f94c[i] = [e.packed, e.packed];
        }
        SeedDispatchTables { f24c, f94c }
    }

    /// Spec/04 §5.1 row 1 — the `0x1003f24c` 4-byte-stride dispatch
    /// table (128 packed DWORDs in `ecx` order).
    pub fn table_f24c(&self) -> &[i32; SEED_DISPATCH_RECORDS] {
        &self.f24c
    }

    /// Spec/04 §5.1 rows 2 + 3 — the `0x1003f94c` / `0x1003f950`
    /// 8-byte-stride dispatch table (128 records of `[+0x0, +0x4]`,
    /// both halves equal to the record's packed DWORD).
    pub fn table_f94c(&self) -> &[[i32; 2]; SEED_DISPATCH_RECORDS] {
        &self.f94c
    }

    /// Spec/04 §5.1 rows 4–6 — the single in-bounds high-half seed
    /// pair (`0x1003ee4c` / `0x1003ee4d` = seed offset `0x100` /
    /// `0x101`), packed by the same `((lo << 8) + hi) << 9` formula.
    ///
    /// Returns `None` when the vendored seed is shorter than
    /// `0x102` bytes. The high-half stream's records for `ecx > 0`
    /// would read past the 258-byte extract (audit/00 §2.2), so only
    /// pair 0 is determinable; the rest is a deferred DOCS-GAP (see
    /// the module docs).
    pub fn high_half_pair0() -> Option<SeedEntry> {
        let raw = parse_hex_bytes(SEED_TABLE_HEX);
        let lo = *raw.get(0x100)? as i8;
        let hi = *raw.get(0x101)? as i8;
        let packed = (((lo as i32) << 8) + (hi as i32)) << 9;
        Some(SeedEntry { lo, hi, packed })
    }
}

/// Spec/04 §1.2 / §6 — the per-frame VQ codebook arena
/// (`*(inner_instance + 0x3004) + 0x000..+0x8000`).
///
/// The `+0x800..+0x8800` range holds 16 bands × 2 KB; each band is a
/// 1 KB primary table followed by a 1 KB secondary table. The arena
/// is rebuilt per non-NULL frame from a static seed window by
/// [`VqArena::apply_alt_quant`] (the §6 overlay). On NULL frames the
/// overlay is skipped and the arena retains its previous contents
/// (§6.4).
#[derive(Clone)]
pub struct VqArena {
    bytes: Box<[u8; ARENA_LEN]>,
}

impl core::fmt::Debug for VqArena {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VqArena")
            .field("len", &ARENA_LEN)
            .finish_non_exhaustive()
    }
}

impl Default for VqArena {
    fn default() -> Self {
        Self::new()
    }
}

impl VqArena {
    /// Allocate a zeroed arena (matching the heap allocation +
    /// zero-init at `IR32_32.DLL!0x10003cdc` before the first
    /// overlay).
    pub fn new() -> Self {
        VqArena {
            bytes: Box::new([0u8; ARENA_LEN]),
        }
    }

    /// The raw arena bytes.
    pub fn as_bytes(&self) -> &[u8; ARENA_LEN] {
        &self.bytes
    }

    /// Mutable access to the raw arena bytes — the seam through which
    /// a caller (or test) supplies arena content while the `spec/04
    /// §7.1` / `§5.2` value extraction stays gapped. The layout
    /// contract is [`arena_primary_offset`](super::arena_primary_offset)
    /// / [`arena_secondary_offset`](super::arena_secondary_offset)
    /// (spec/07 §2.1 / §2.3).
    pub fn bytes_mut(&mut self) -> &mut [u8; ARENA_LEN] {
        &mut self.bytes
    }

    /// Spec/04 §6.3 — the byte offset of band `i`'s primary table
    /// (`0x800 + 0x800*i`), or `None` for `i >= 16`.
    pub fn band_primary_offset(band: usize) -> Option<usize> {
        if band >= ARENA_BAND_COUNT {
            return None;
        }
        Some(ARENA_BANDS_OFFSET + ARENA_BAND_LEN * band)
    }

    /// Spec/04 §6.3 (corrected) — the byte offset of band `i`'s
    /// secondary table (`0x800·i + 0x400`), or `None` for `i >= 16`.
    pub fn band_secondary_offset(band: usize) -> Option<usize> {
        Self::band_primary_offset(band).map(|p| p + ARENA_HALF_LEN)
    }

    /// Spec/04 §6 (corrected, Extractor rounds 16/17) — rebuild the
    /// per-band codebook tables from the codec-init **staging image**
    /// ([`super::StagingImage`], `spec/04 §5.2`) according to
    /// `alt_quant[16]` and the `cb_offset` bias.
    ///
    /// The overlay applies the global staging-window bias once
    /// (`staging + cb_offset · 0x800`, §6.3), then per band `i`:
    ///
    /// * `alt_quant[i] == 0` → skip the band (it keeps its previous
    ///   contents; `0x00` is the only encoding that skips, §6.2).
    /// * else copy 1 KB from staging block `(q >> 4) + cb_offset`
    ///   into the band's primary half (`+0x800·i`) and 1 KB from
    ///   staging block `(q & 0x0f) + cb_offset` into its secondary
    ///   half (`+0x800·i + 0x400`). Both nibbles index the same
    ///   block array at the same 0x800 stride (§6.2 corrected), and
    ///   only a block's `+0x000` sub-table is ever a copy source.
    ///
    /// The binary's `[src+0x3fc] != [dst+0x3fc]` dirty-check is a
    /// copy-elision optimisation with no semantic effect, so we copy
    /// unconditionally. Returns the biased staging offset
    /// (`cb_offset << 11`) the reference stashes at `arena + 0x8000`
    /// (§1.2), or an error if a selected block falls outside the
    /// staging image (the routine performs no bounds check; encoders
    /// must keep `nibble + cb_offset` within the 24 blocks, §6.2).
    pub fn apply_alt_quant(
        &mut self,
        staging: &super::StagingImage,
        alt_quant: &[u8; ARENA_BAND_COUNT],
        cb_offset: i8,
    ) -> Result<i64, VqError> {
        // §6.3: the window shift is applied once, before the per-band
        // loop — a signed whole-block bias.
        let bias = (cb_offset as i64) << 11;

        for (band, &q) in alt_quant.iter().enumerate() {
            if q == 0 {
                // §6.2 — leave both halves as the previous frame left
                // them.
                continue;
            }
            let (primary_idx, secondary_idx) = nibble_split(q);

            let p_dst = Self::band_primary_offset(band).expect("band < 16");
            copy_staging_block(staging, bias, primary_idx, &mut self.bytes[..], p_dst, band)?;

            let s_dst = Self::band_secondary_offset(band).expect("band < 16");
            copy_staging_block(
                staging,
                bias,
                secondary_idx,
                &mut self.bytes[..],
                s_dst,
                band,
            )?;
        }

        Ok(bias)
    }
}

/// Copy one staging block's `+0x000` sub-table (1 KB) into an arena
/// half, range-checking the biased source window (`spec/04 §6.1`).
fn copy_staging_block(
    staging: &super::StagingImage,
    bias: i64,
    nibble: u8,
    arena: &mut [u8],
    dst_off: usize,
    band: usize,
) -> Result<(), VqError> {
    let src_off = bias + (i64::from(nibble)) * (STAGING_SELECT_STRIDE as i64);
    if src_off < 0 {
        return Err(VqError::SeedWindowOutOfRange {
            band,
            src_offset: src_off,
            seed_len: staging.as_bytes().len(),
        });
    }
    let start = src_off as usize;
    let end = start + ARENA_HALF_LEN;
    let Some(src) = staging.as_bytes().get(start..end) else {
        return Err(VqError::SeedWindowOutOfRange {
            band,
            src_offset: src_off,
            seed_len: staging.as_bytes().len(),
        });
    };
    arena[dst_off..dst_off + ARENA_HALF_LEN].copy_from_slice(src);
    Ok(())
}

/// Spec/04 §6.2 — split an `alt_quant[]` byte into (primary, secondary)
/// nibble indices (high nibble = primary, low nibble = secondary).
/// Mirrors `header::alt_quant_indices` but is kept local to the VQ
/// module to make the §6 overlay self-describing.
fn nibble_split(byte: u8) -> (u8, u8) {
    ((byte & 0xf0) >> 4, byte & 0x0f)
}

/// Spec/04 §4 — the runtime VQ_NULL sub-bit interpretation.
///
/// spec/03 already resolves the tree-level VQ_NULL leaf into
/// [`super::VqNull`] (`Copy` / `Skip`) by reading a 2-bit sub-code.
/// At reconstruction time (spec/04 §4) the reference reads the
/// sub-bits one at a time from the bit accumulator and recognises a
/// third, anomalous "first-bit-1" path that dispatches into the
/// per-byte unpacker. This enum surfaces all three runtime
/// interpretations so the spec/07 reconstruction layer can act on
/// them; the §3 tree walk only models the two well-formed ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VqNullRuntime {
    /// First bit 0, second bit 0 → copy the upper-neighbour row
    /// into the cell (`IR32_32.DLL!0x100069f4`).
    CopyUpper,
    /// First bit 0, second bit 1 → mark the cell as a boundary /
    /// edge cell by setting bit 7 of the cell pixel bytes
    /// (`IR32_32.DLL!0x10006a2f`).
    MarkBoundary,
    /// First bit 1 → dispatch into the per-byte unpacker treating
    /// the next bitstream byte as a mode byte
    /// (`IR32_32.DLL!0x10006bac`). spec/04 §7.3 flags this as an
    /// open question ("VQ-data without leaf-byte"); we surface it
    /// rather than fault, deferring its emission semantics to
    /// spec/06.
    UnpackerDispatch,
}

impl VqNullRuntime {
    /// Spec/04 §4 — classify a VQ_NULL sub-bit pair.
    ///
    /// `first_bit` is consumed at `IR32_32.DLL!0x100069d4`. If it is
    /// 1, `second_bit` is ignored ([`VqNullRuntime::UnpackerDispatch`]).
    /// If it is 0, `second_bit` (consumed at `0x100069f2`) selects
    /// copy-upper (0) vs mark-boundary (1).
    pub fn classify(first_bit: bool, second_bit: bool) -> Self {
        if first_bit {
            VqNullRuntime::UnpackerDispatch
        } else if second_bit {
            VqNullRuntime::MarkBoundary
        } else {
            VqNullRuntime::CopyUpper
        }
    }
}

/// Errors raised while materialising the spec/04 VQ codebook state.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum VqError {
    /// Spec/04 §6 — an `alt_quant[]` band's source window (after the
    /// `cb_offset` bias and the nibble-stride multiply) fell outside
    /// the supplied static seed buffer.
    SeedWindowOutOfRange {
        /// The band (0..15) whose source window was out of range.
        band: usize,
        /// The computed source byte offset (may be negative after a
        /// negative `cb_offset` bias).
        src_offset: i64,
        /// The length of the static seed buffer the caller supplied.
        seed_len: usize,
    },
}

impl core::fmt::Display for VqError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            VqError::SeedWindowOutOfRange {
                band,
                src_offset,
                seed_len,
            } => write!(
                f,
                "alt_quant band {band} source window at offset {src_offset} \
                 is outside the {seed_len}-byte static seed buffer"
            ),
        }
    }
}

impl std::error::Error for VqError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dyad_table_loads_8kib_and_first_bytes_match_extract() {
        let t = DyadDeltaTable::load();
        assert_eq!(t.as_bytes().len(), DYAD_TABLE_LEN);
        // First 16 bytes per tables/region_1003d088.meta:
        // 00 02 02 04 04 06 06 08 08 0a 0a 0c 0c 0e 0e 10.
        let expect = [
            0x00, 0x02, 0x02, 0x04, 0x04, 0x06, 0x06, 0x08, 0x08, 0x0a, 0x0a, 0x0c, 0x0c, 0x0e,
            0x0e, 0x10,
        ];
        assert_eq!(&t.as_bytes()[..16], &expect);
    }

    #[test]
    fn dyad_bank_indexing_uses_512_byte_stride() {
        let t = DyadDeltaTable::load();
        // bank 0 col 0 is the first table byte.
        assert_eq!(t.delta(0, 0), Some(t.as_bytes()[0]));
        // bank 1 col 0 is at byte 512.
        assert_eq!(t.delta(1, 0), Some(t.as_bytes()[512]));
        // bank base helper agrees.
        assert_eq!(DyadDeltaTable::bank_base(0), Some(0));
        assert_eq!(DyadDeltaTable::bank_base(1), Some(512));
        assert_eq!(DyadDeltaTable::bank_base(15), Some(15 * 512));
        assert_eq!(DyadDeltaTable::bank_base(16), None);
    }

    #[test]
    fn dyad_out_of_bank_rejected() {
        let t = DyadDeltaTable::load();
        assert_eq!(t.delta(16, 0), None);
        assert_eq!(t.delta(0, DYAD_BANK_STRIDE), None);
    }

    #[test]
    fn dyad_bank15_row_restriction_per_audit() {
        let t = DyadDeltaTable::load();
        // Bank 15, row 64 (col 256) is the last populated row.
        assert!(t.delta(15, (DYAD_BANK15_VALID_ROWS - 1) * 4).is_some());
        // Bank 15, row 65 (col 260) and beyond are rejected.
        assert_eq!(t.delta(15, DYAD_BANK15_VALID_ROWS * 4), None);
        assert_eq!(t.delta(15, 511), None);
        // Other banks have no such restriction.
        assert!(t.delta(14, 511).is_some());
    }

    #[test]
    fn codebook_entry_mode_bits_and_offset() {
        // packed = 0 → plain variant, offset 0.
        let e = CodebookEntry::decode(0);
        assert_eq!(e.variant, CellVariant::Plain);
        assert!(!e.mode_bit0 && !e.mode_bit1);
        assert_eq!(e.arena_offset, 0);

        // bit0 set, offset bits = 0b101 → packed 0b10101 = 0x15.
        let e = CodebookEntry::decode(0x15);
        assert!(e.mode_bit0 && !e.mode_bit1);
        assert_eq!(e.variant, CellVariant::WithEdge);
        // 0x15 >> 2 (arithmetic) = 5.
        assert_eq!(e.arena_offset, 5);

        // both bits set.
        let e = CodebookEntry::decode(0x3);
        assert!(e.mode_bit0 && e.mode_bit1);
        assert_eq!(e.variant, CellVariant::FullyDoubled);
        assert_eq!(e.arena_offset, 0);

        // bit1 only.
        let e = CodebookEntry::decode(0x2);
        assert!(!e.mode_bit0 && e.mode_bit1);
        assert_eq!(e.variant, CellVariant::DoubledRow);
    }

    #[test]
    fn codebook_entry_offset_is_signed_arithmetic_shift() {
        // High bit set → negative arena offset (sar preserves sign).
        let e = CodebookEntry::decode(0xffff_fffc); // -4 as i32
        assert_eq!(e.arena_offset, -1); // -4 >> 2 == -1
        assert!(!e.mode_bit0 && !e.mode_bit1);
    }

    #[test]
    fn cell_variant_clamp_flag() {
        assert!(!CellVariant::Plain.clamps_to_7bit());
        assert!(CellVariant::WithEdge.clamps_to_7bit());
        assert!(CellVariant::DoubledRow.clamps_to_7bit());
        assert!(CellVariant::FullyDoubled.clamps_to_7bit());
    }

    #[test]
    fn seed_entries_pack_and_scale_per_5_1() {
        let entries = seed_dispatch_entries();
        assert_eq!(entries.len(), SEED_PAIR_COUNT);
        // First seed pair per tables/region_1003ed4c.meta: 19 80.
        // lo = 0x19 = 25, hi = 0x80 = -128 (signed).
        let e0 = entries[0];
        assert_eq!(e0.lo, 25);
        assert_eq!(e0.hi, -128);
        // packed16 = (25 << 8) + (-128) = 6400 - 128 = 6272.
        // packed = 6272 << 9 = 3_211_264.
        assert_eq!(e0.packed, ((25i32 << 8) - 128) << 9);
        assert_eq!(e0.packed, 3_211_264);
    }

    #[test]
    fn seed_entries_signed_high_byte() {
        let entries = seed_dispatch_entries();
        // Second pair: 19 81 → lo 25, hi 0x81 = -127.
        assert_eq!(entries[1].lo, 25);
        assert_eq!(entries[1].hi, -127);
    }

    #[test]
    fn seed_dispatch_tables_f24c_mirrors_packed_entries() {
        // Spec/04 §5.1 row 1: the 0x1003f24c 4-byte-stride table holds
        // one packed DWORD per seed pair, in ecx order.
        let tables = SeedDispatchTables::build();
        let entries = seed_dispatch_entries();
        assert_eq!(tables.table_f24c().len(), SEED_DISPATCH_RECORDS);
        assert_eq!(SEED_DISPATCH_RECORDS, 128);
        for (i, e) in entries.iter().enumerate() {
            assert_eq!(tables.table_f24c()[i], e.packed);
        }
        // First record matches the known first pair (19 80).
        assert_eq!(tables.table_f24c()[0], 3_211_264);
    }

    #[test]
    fn seed_dispatch_table_f94c_both_halves_equal_packed() {
        // Spec/04 §5.1 rows 2 + 3: the 8-byte-stride table's +0x0 and
        // +0x4 halves both receive the same packed DWORD.
        let tables = SeedDispatchTables::build();
        let entries = seed_dispatch_entries();
        assert_eq!(tables.table_f94c().len(), SEED_DISPATCH_RECORDS);
        for (i, e) in entries.iter().enumerate() {
            let rec = tables.table_f94c()[i];
            assert_eq!(rec[0], e.packed, "0x1003f94c half (+0x0)");
            assert_eq!(rec[1], e.packed, "0x1003f950 half (+0x4)");
        }
    }

    #[test]
    fn seed_dispatch_high_half_pair0_packs_offset_0x100() {
        // Spec/04 §5.1 rows 4–6 / audit/00 §2.2: only the single
        // in-bounds high-half pair (seed offset 0x100 / 0x101) is
        // determinable. Pair 128 of the seed is (154, 52) = signed
        // (-102, +52) per audit/00 §2.2.
        let raw = parse_hex_bytes(SEED_TABLE_HEX);
        assert_eq!(raw[0x100], 154);
        assert_eq!(raw[0x101], 52);
        let pair = SeedDispatchTables::high_half_pair0().expect("seed has 0x102 bytes");
        assert_eq!(pair.lo, -102);
        assert_eq!(pair.hi, 52);
        // packed = ((-102 << 8) + 52) << 9.
        assert_eq!(pair.packed, (((-102i32) << 8) + 52) << 9);
    }

    #[test]
    fn arena_band_offsets_match_corrected_6_3() {
        // Round-17 correction: band i at arena + 0x800*i, no leading
        // skip.
        assert_eq!(VqArena::band_primary_offset(0), Some(0));
        assert_eq!(VqArena::band_secondary_offset(0), Some(0x400));
        assert_eq!(VqArena::band_primary_offset(1), Some(0x800));
        assert_eq!(VqArena::band_primary_offset(15), Some(0x800 * 15));
        assert_eq!(VqArena::band_secondary_offset(15), Some(0x800 * 15 + 0x400));
        assert_eq!(VqArena::band_primary_offset(16), None);
        assert_eq!(VqArena::band_secondary_offset(16), None);
        // The band region ends exactly at 0x8000; the 0x20-byte tail
        // holds the +0x8000 pointer slot and register stash (§1.2).
        assert_eq!(
            VqArena::band_secondary_offset(15).unwrap() + ARENA_HALF_LEN,
            0x8000
        );
        assert_eq!(ARENA_LEN, 0x8020);
    }

    fn staging() -> super::super::StagingImage {
        super::super::StagingImage::build(&super::super::CodebookSeedArea::load())
    }

    #[test]
    fn alt_quant_overlay_copies_primary_and_secondary() {
        let img = staging();
        let mut arena = VqArena::new();
        let mut alt = [0u8; 16];
        // Band 0 uses staging block 2 (high nibble) for its primary
        // half and block 1 (low nibble) for its secondary: byte 0x21.
        alt[0] = 0x21;
        let bias = arena.apply_alt_quant(&img, &alt, 0).unwrap();
        assert_eq!(bias, 0);
        // Primary half of band 0 == staging block 2's +0x000
        // sub-table (only a block's +0x000 half is ever a source).
        let p_dst = VqArena::band_primary_offset(0).unwrap();
        assert_eq!(
            &arena.as_bytes()[p_dst..p_dst + ARENA_HALF_LEN],
            &img.as_bytes()[2 * STAGING_SELECT_STRIDE..2 * STAGING_SELECT_STRIDE + ARENA_HALF_LEN]
        );
        // Secondary half == staging block 1's +0x000 sub-table.
        let s_dst = VqArena::band_secondary_offset(0).unwrap();
        assert_eq!(
            &arena.as_bytes()[s_dst..s_dst + ARENA_HALF_LEN],
            &img.as_bytes()[STAGING_SELECT_STRIDE..STAGING_SELECT_STRIDE + ARENA_HALF_LEN]
        );
    }

    #[test]
    fn alt_quant_zero_band_is_skipped() {
        let img = staging();
        let mut arena = VqArena::new();
        // Pre-mark band 1's region so we can detect an unwanted write.
        let p1 = VqArena::band_primary_offset(1).unwrap();
        for b in &mut arena.bytes[p1..p1 + ARENA_BAND_LEN] {
            *b = 0x55;
        }
        let mut alt = [0u8; 16];
        alt[0] = 0x11; // band 0 active
        // band 1 stays 0 -> skipped.
        arena.apply_alt_quant(&img, &alt, 0).unwrap();
        // Band 1 untouched (still 0x55).
        assert!(
            arena.as_bytes()[p1..p1 + ARENA_BAND_LEN]
                .iter()
                .all(|&b| b == 0x55)
        );
    }

    #[test]
    fn alt_quant_cb_offset_bias_applied_once() {
        // cb_offset = 8 shifts the window by 8 whole blocks: nibble
        // pair (1, 0) then addresses blocks 9 and 8 (§6.2).
        let img = staging();
        let mut arena = VqArena::new();
        let mut alt = [0u8; 16];
        alt[0] = 0x10;
        let bias = arena.apply_alt_quant(&img, &alt, 8).unwrap();
        assert_eq!(bias, 8 << 11);
        let p_dst = VqArena::band_primary_offset(0).unwrap();
        let p_src = 9 * STAGING_SELECT_STRIDE;
        assert_eq!(
            &arena.as_bytes()[p_dst..p_dst + ARENA_HALF_LEN],
            &img.as_bytes()[p_src..p_src + ARENA_HALF_LEN]
        );
        let s_dst = VqArena::band_secondary_offset(0).unwrap();
        let s_src = 8 * STAGING_SELECT_STRIDE;
        assert_eq!(
            &arena.as_bytes()[s_dst..s_dst + ARENA_HALF_LEN],
            &img.as_bytes()[s_src..s_src + ARENA_HALF_LEN]
        );
    }

    #[test]
    fn alt_quant_out_of_range_staging_window_errors() {
        // §6.2: a cb_offset that pushes nibble + cb_offset outside the
        // 24 staging blocks addresses outside the image; the binary
        // performs no bounds check, we surface a typed error. Block
        // index 15 + cb_offset 33 = block 48 -> byte 0x18000, past the
        // image end.
        let img = staging();
        let mut arena = VqArena::new();
        let mut alt = [0u8; 16];
        alt[0] = 0x0f;
        let err = arena.apply_alt_quant(&img, &alt, 33).unwrap_err();
        match err {
            VqError::SeedWindowOutOfRange { band, .. } => assert_eq!(band, 0),
        }
        // A negative bias below block 0 likewise errors.
        let mut alt2 = [0u8; 16];
        alt2[0] = 0x11;
        let err = arena.apply_alt_quant(&img, &alt2, -2).unwrap_err();
        match err {
            VqError::SeedWindowOutOfRange { band, .. } => assert_eq!(band, 0),
        }
    }

    #[test]
    fn vq_null_runtime_classification() {
        assert_eq!(
            VqNullRuntime::classify(false, false),
            VqNullRuntime::CopyUpper
        );
        assert_eq!(
            VqNullRuntime::classify(false, true),
            VqNullRuntime::MarkBoundary
        );
        // first bit 1 → unpacker dispatch regardless of second bit.
        assert_eq!(
            VqNullRuntime::classify(true, false),
            VqNullRuntime::UnpackerDispatch
        );
        assert_eq!(
            VqNullRuntime::classify(true, true),
            VqNullRuntime::UnpackerDispatch
        );
    }

    #[test]
    fn nibble_split_matches_header_helper() {
        assert_eq!(nibble_split(0x00), (0, 0));
        assert_eq!(nibble_split(0xab), (0xa, 0xb));
        assert_eq!(nibble_split(0xf0), (0xf, 0));
        assert_eq!(nibble_split(0x0f), (0, 0xf));
    }

    #[test]
    fn row_band_column_packs_row_times_4_plus_col() {
        // Spec/07 §3.1: within-bank column = row_position*4 + column.
        assert_eq!(DyadDeltaTable::row_band_column(0, 0), Some(0));
        assert_eq!(DyadDeltaTable::row_band_column(0, 3), Some(3));
        assert_eq!(DyadDeltaTable::row_band_column(1, 0), Some(4));
        assert_eq!(DyadDeltaTable::row_band_column(2, 1), Some(9));
        // Last in-bank column (row 127, col 3 = 511).
        assert_eq!(DyadDeltaTable::row_band_column(127, 3), Some(511));
        // One past the bank stride is rejected.
        assert_eq!(DyadDeltaTable::row_band_column(128, 0), None);
    }

    #[test]
    fn row_band_delta_reads_signed_static_table_byte() {
        let t = DyadDeltaTable::load();
        // Bank 0 row 0 col 0 = first table byte (0x00) as signed.
        assert_eq!(t.row_band_delta(0, 0, 0), Some(0));
        // Bank 0 row 0 col 1 = table[1] = 0x02.
        assert_eq!(t.row_band_delta(0, 0, 1), Some(2));
        // Bank 1 row 0 col 0 = table[512].
        assert_eq!(t.row_band_delta(1, 0, 0), Some(t.as_bytes()[512] as i8));
        // Out-of-bank high nibble rejected.
        assert_eq!(t.row_band_delta(16, 0, 0), None);
        // Bank 15 beyond the populated rows rejected (row 65+).
        assert_eq!(t.row_band_delta(15, DYAD_BANK15_VALID_ROWS, 0), None);
    }

    #[test]
    fn apply_row_band_seed_writes_biased_byte_into_predictor_slot() {
        let t = DyadDeltaTable::load();
        // Strip large enough for a write index two rows in.
        let mut strip = vec![0u8; 4 * super::super::PREDICTOR_ROW_STRIDE];
        let write_index = 2 * super::super::PREDICTOR_ROW_STRIDE + 5;
        // Bank 0 row 0 col 1 → delta 0x02 (signed +2).
        let seed = apply_row_band_seed(&t, &mut strip, write_index, 0, 0, 1).unwrap();
        assert_eq!(seed.delta, 2);
        assert_eq!(
            seed.predictor_slot,
            write_index - super::super::PREDICTOR_ROW_STRIDE
        );
        // The stored byte is the 0x80-biased form (spec/07 §3.2 / §4):
        // (2 as u8) ^ 0x80 = 0x82.
        assert_eq!(strip[seed.predictor_slot], 0x82);
    }

    #[test]
    fn apply_row_band_seed_rejects_top_of_strip_write() {
        let t = DyadDeltaTable::load();
        let mut strip = vec![0u8; 4 * super::super::PREDICTOR_ROW_STRIDE];
        // Write within the first row has no row above → None (§1.3).
        assert!(apply_row_band_seed(&t, &mut strip, 5, 0, 0, 0).is_none());
    }
}
