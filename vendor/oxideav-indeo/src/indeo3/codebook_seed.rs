//! Indeo 3 per-frame codebook seed area (`spec/04 §5.2`, settled).
//!
//! Spec source: `docs/video/indeo/indeo3/spec/04-vq-codebooks.md`
//! §5.2/§5.2.1 (Extractor round 16 — the seed-area block grammar,
//! reconciled) and the staged block/pair tables
//! `tables/seed_blocks_1004d26a.csv` / `tables/seed_pairs_1004d26a.csv`.
//!
//! The static area at `.data 0x1004d26a` is the master copy of the
//! VQ codebook content. The codec-init routine at
//! `IR32_32.DLL!0x10006308` expands it into the 0x18000-byte
//! **codebook staging image** ([`super::StagingImage`], `spec/04
//! §5.2`), and the per-frame `alt_quant[]` overlay
//! ([`super::VqArena::apply_alt_quant`], `spec/04 §6`) then copies
//! whole 1 KB sub-tables out of that image into the per-frame arena.
//!
//! ## The settled grammar (`spec/04 §5.2.1`)
//!
//! ```text
//! block := count_byte           ; UNSIGNED count of byte-pairs
//!          pair[count]          ; each pair is two signed bytes (a, b)
//!          expand_byte          ; SIGNED; drives the §5.2.3 expansion
//! area  := block* 0x00          ; a zero count byte terminates
//! ```
//!
//! The next block's header is at `offset + 2·count + 2`. The count is
//! **unsigned** — the first block's is `0xC3` = 195, and reading it
//! signed desynchronises the parse at block 0. Walking the grammar
//! from `0x1004d26a` terminates cleanly on the zero count byte at
//! `0x1004e6ec`: **24 blocks, 2601 pairs, 5251 bytes**. (This
//! supersedes the r43x DOCS-GAP report: the audit's zero-gap-record
//! reading of the same bytes is withdrawn by the round-16
//! reconciliation, which watched the codec's own arena filler consume
//! the area.)
//!
//! Each pair `(a, b)` seeds the 32-bit staging word of `spec/04
//! §5.2.2` ([`SeedPair::seeded_word`]); the trailing signed
//! `expand_byte` `E` gives the §5.2.3 expansion dimension `d = |E|`
//! and, by its sign, the ordered-pair emission order.

/// Spec/04 §5.2 — the codec-init seed pointer is stashed at
/// `.data + 0x1004d25a`; the raw block table begins at
/// `.data + 0x1004d26a` (16 bytes past the stash slot).
pub const SEED_AREA_VMA: u32 = 0x1004_d26a;

/// Spec/04 §5.2.1 — the block table terminates at the first count
/// byte equal to `0`.
pub const BLOCK_TERMINATOR: u8 = 0;

/// Spec/04 §5.2.2 — the `0x8000` bias that recentres the signed
/// 16-bit pair value onto the unsigned range (the codebook's natural
/// zero sits mid-range).
pub const SEED_WORD_BIAS: u16 = 0x8000;

/// Spec/04 §5.2 step 2 — the per-block destination advance within the
/// staging image (`0x800` bytes per block).
pub const BLOCK_DEST_ADVANCE: usize = 0x800;

/// Spec/04 §5.2.1 — the settled seed-area extent: 24 blocks, 2601
/// pairs, 5251 bytes (terminator at offset 5250 = VMA `0x1004e6ec`).
pub const SEED_BLOCK_COUNT: usize = 24;

/// Spec/04 §5.2.1 — total pairs across the 24 blocks.
pub const SEED_PAIR_TOTAL: usize = 2601;

/// Spec/04 §5.2.1 — the area's byte length including the terminator.
pub const SEED_AREA_LEN: usize = 5251;

// Vendored, verbatim, from the docs clean-room staging (the settled
// round-16 block/pair/expand tables re-serialised to the raw wire
// bytes; byte-identical over its first 4096 bytes to the raw window
// extract `tables/region_1004d26a.hex`). A copy lives inside the
// crate so the published crate is self-contained. The file carries a
// `#`-prefixed provenance header; `parse_hex_bytes` skips comment
// lines.
const SEED_AREA_HEX: &str = include_str!("data/codebook_seed_1004d26a.hex");

/// Parse a whitespace-separated lower-hex byte dump, skipping any line
/// that begins with `#` (the vendored-file provenance header).
fn parse_hex_bytes(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        for tok in line.split_whitespace() {
            if let Ok(b) = u8::from_str_radix(tok, 16) {
                out.push(b);
            }
        }
    }
    out
}

/// Spec/04 §5.2.1 — one signed byte-pair `(a, b)` from a block's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedPair {
    /// The first (low) byte of the pair.
    pub a: i8,
    /// The second (high) byte of the pair.
    pub b: i8,
}

impl SeedPair {
    /// Spec/04 §5.2.2 — the pair read as a signed 16-bit quantity:
    /// `b · 256 + a`.
    pub fn value(self) -> i16 {
        (i16::from(self.b) << 8).wrapping_add(i16::from(self.a))
    }

    /// Spec/04 §5.2.2 — the 32-bit word a seeded pair becomes in the
    /// staging image's `+0x000` / `+0x400` sub-tables:
    ///
    /// ```text
    /// word = ((( b · 256 + a ) + 0x8000) & 0xFFFF) << 16
    /// ```
    pub fn seeded_word(self) -> u32 {
        let biased = (self.value() as u16).wrapping_add(SEED_WORD_BIAS);
        u32::from(biased) << 16
    }

    /// Spec/04 §5.2.2 — the byte-replicated form for the `+0xc000`
    /// table set: the pair's two bytes emitted as `(b, b, a, a)` from
    /// most to least significant, **accumulated with 32-bit adds** so
    /// a negative low byte borrows into the byte above it.
    pub fn replicated_word(self) -> u32 {
        let a = i32::from(self.a);
        let b = i32::from(self.b);
        ((b << 24) + (b << 16) + (a << 8) + a) as u32
    }
}

/// Spec/04 §5.2.1 — one variable-length block of the `0x1004d26a`
/// seed area: a count `N`, `N` signed byte-pairs, and the signed
/// expansion byte `E`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedBlock {
    /// Byte offset of this block's count byte within the seed area.
    pub offset: usize,
    /// The count `N` (number of byte-pairs in the block body;
    /// unsigned — values above 127 occur).
    pub count: u8,
    /// The `N` signed byte-pairs of the block body, in stream order.
    pub pairs: Vec<SeedPair>,
    /// The signed trailing expansion byte `E` (`spec/04 §5.2.3`):
    /// `d = |E|` seed words feed the `d²` ordered-pair expansion, and
    /// the sign of `E` transposes the emission order.
    pub expand: i8,
}

impl SeedBlock {
    /// The number of bytes this block occupies in the seed area
    /// (`1` count byte + `2·N` body bytes + `1` expansion byte).
    pub fn encoded_len(&self) -> usize {
        2 + 2 * self.count as usize
    }

    /// `d = |E|` — the §5.2.3 expansion dimension.
    pub fn expand_dim(&self) -> usize {
        self.expand.unsigned_abs() as usize
    }

    /// The per-block seeded staging words, in pair order
    /// (`spec/04 §5.2.2`).
    pub fn seeded_words(&self) -> Vec<u32> {
        self.pairs.iter().map(|p| p.seeded_word()).collect()
    }
}

/// Errors raised by the seed-area walk (`spec/04 §5.2.1`). The
/// vendored area parses cleanly; these fire only for caller-supplied
/// synthetic tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SeedAreaError {
    /// A block's body (pairs + expansion byte) ran past the supplied
    /// buffer before a terminator was seen.
    Truncated {
        /// The offset of the truncated block's count byte.
        block_offset: usize,
    },
    /// The buffer ended with no terminating zero count byte.
    MissingTerminator,
}

impl core::fmt::Display for SeedAreaError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SeedAreaError::Truncated { block_offset } => write!(
                f,
                "spec/04 §5.2.1: seed block at offset {block_offset} runs past the buffer"
            ),
            SeedAreaError::MissingTerminator => {
                f.write_str("spec/04 §5.2.1: seed area has no zero-count terminator")
            }
        }
    }
}

impl std::error::Error for SeedAreaError {}

/// Spec/04 §5.2 — the parsed per-frame codebook seed area at
/// `.data + 0x1004d26a` (the producer side of the staging-image
/// build, [`super::StagingImage`]).
#[derive(Debug, Clone)]
pub struct CodebookSeedArea {
    raw: Vec<u8>,
    blocks: Vec<SeedBlock>,
    terminator_offset: usize,
}

impl CodebookSeedArea {
    /// Materialise the seed area from the vendored clean-room bytes
    /// and walk the settled §5.2.1 block grammar.
    pub fn load() -> Self {
        let raw = parse_hex_bytes(SEED_AREA_HEX);
        Self::from_bytes(raw).expect("vendored seed area parses (spec/04 §5.2.1)")
    }

    /// Parse a caller-supplied raw seed buffer with the §5.2.1
    /// grammar: count byte + `2·count` body bytes + expansion byte,
    /// repeated until a zero count byte.
    pub fn from_bytes(raw: Vec<u8>) -> Result<Self, SeedAreaError> {
        let mut blocks = Vec::new();
        let mut i = 0usize;
        loop {
            let Some(&count) = raw.get(i) else {
                return Err(SeedAreaError::MissingTerminator);
            };
            if count == BLOCK_TERMINATOR {
                return Ok(CodebookSeedArea {
                    raw,
                    blocks,
                    terminator_offset: i,
                });
            }
            let body_start = i + 1;
            let body_len = 2 * count as usize;
            // Body plus the trailing expansion byte must fit.
            if body_start + body_len + 1 > raw.len() {
                return Err(SeedAreaError::Truncated { block_offset: i });
            }
            let mut pairs = Vec::with_capacity(count as usize);
            for k in 0..count as usize {
                let a = raw[body_start + 2 * k] as i8;
                let b = raw[body_start + 2 * k + 1] as i8;
                pairs.push(SeedPair { a, b });
            }
            let expand = raw[body_start + body_len] as i8;
            blocks.push(SeedBlock {
                offset: i,
                count,
                pairs,
                expand,
            });
            i = body_start + body_len + 1;
        }
    }

    /// The raw seed-area bytes (VMA order).
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// The parsed §5.2.1 blocks (in stream order).
    pub fn blocks(&self) -> &[SeedBlock] {
        &self.blocks
    }

    /// The byte offset of the terminating zero count byte.
    pub fn terminator_offset(&self) -> usize {
        self.terminator_offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendored_area_has_settled_extent() {
        // spec/04 §5.2.1: 24 blocks, 2601 pairs, 5251 bytes,
        // terminator at offset 5250.
        let area = CodebookSeedArea::load();
        assert_eq!(area.raw().len(), SEED_AREA_LEN);
        assert_eq!(area.blocks().len(), SEED_BLOCK_COUNT);
        let pair_total: usize = area.blocks().iter().map(|b| b.pairs.len()).sum();
        assert_eq!(pair_total, SEED_PAIR_TOTAL);
        assert_eq!(area.terminator_offset(), SEED_AREA_LEN - 1);
    }

    #[test]
    fn vendored_pair_counts_match_staged_table() {
        // tables/seed_blocks_1004d26a.csv: two identical descending
        // groups of eight, then 128 and seven 79s.
        let area = CodebookSeedArea::load();
        let counts: Vec<u8> = area.blocks().iter().map(|b| b.count).collect();
        assert_eq!(
            counts,
            vec![
                195, 159, 133, 115, 101, 93, 87, 77, 195, 159, 133, 115, 101, 93, 87, 77, 128, 79,
                79, 79, 79, 79, 79, 79
            ]
        );
    }

    #[test]
    fn vendored_expand_bytes_match_staged_table() {
        // tables/03-vq-staging-blocks.csv: 7 9 10 11 12 12 12 13
        // twice, then -11 and seven -13s (245 / 243 unsigned).
        let area = CodebookSeedArea::load();
        let expands: Vec<i8> = area.blocks().iter().map(|b| b.expand).collect();
        assert_eq!(
            expands,
            vec![
                7, 9, 10, 11, 12, 12, 12, 13, 7, 9, 10, 11, 12, 12, 12, 13, -11, -13, -13, -13,
                -13, -13, -13, -13
            ]
        );
    }

    #[test]
    fn window_prefix_matches_raw_extract() {
        // The first 16 bytes of the raw window extract
        // (tables/region_1004d26a.meta):
        // c3 00 00 02 02 fe fe ff 03 01 fd 03 ff fd 01 04.
        let area = CodebookSeedArea::load();
        let expect = [
            0xc3, 0x00, 0x00, 0x02, 0x02, 0xfe, 0xfe, 0xff, 0x03, 0x01, 0xfd, 0x03, 0xff, 0xfd,
            0x01, 0x04,
        ];
        assert_eq!(&area.raw()[..16], &expect);
    }

    #[test]
    fn seeded_words_match_staged_head() {
        // tables/seed_pairs_1004d26a.csv block 0: (0,0)->0x80000000,
        // (2,2)->0x82020000, (-2,-2)->0x7dfe0000, (-1,3)->0x82ff0000,
        // (1,-3)->0x7d010000.
        let area = CodebookSeedArea::load();
        let words = area.blocks()[0].seeded_words();
        assert_eq!(words[0], 0x8000_0000);
        assert_eq!(words[1], 0x8202_0000);
        assert_eq!(words[2], 0x7dfe_0000);
        assert_eq!(words[3], 0x82ff_0000);
        assert_eq!(words[4], 0x7d01_0000);
    }

    #[test]
    fn replicated_word_borrows_across_bytes() {
        // spec/04 §5.2.2: (b, b, a, a) accumulated with 32-bit adds.
        assert_eq!(SeedPair { a: 0, b: 0 }.replicated_word(), 0x0000_0000);
        assert_eq!(SeedPair { a: 2, b: 2 }.replicated_word(), 0x0202_0202);
        // Negative bytes borrow: (-2, -2) -> 0xfdfdfdfe.
        assert_eq!(SeedPair { a: -2, b: -2 }.replicated_word(), 0xfdfd_fdfe);
        // Mixed pair (-1, 3) -> (3, 3, -1, -1) with the borrow chain:
        // 0x0302ffff + ... = 0x0302feff (staged w_c000 of block 0
        // word 3).
        assert_eq!(SeedPair { a: -1, b: 3 }.replicated_word(), 0x0302_feff);
    }

    #[test]
    fn synthetic_block_walk_and_terminator() {
        // count=2, two pairs, expand byte, terminator, trailing junk.
        let raw = vec![2, 10, 20, 30, 40, 3, 0, 99];
        let area = CodebookSeedArea::from_bytes(raw).expect("parse");
        assert_eq!(area.blocks().len(), 1);
        let b = &area.blocks()[0];
        assert_eq!(b.count, 2);
        assert_eq!(
            b.pairs,
            vec![SeedPair { a: 10, b: 20 }, SeedPair { a: 30, b: 40 }]
        );
        assert_eq!(b.expand, 3);
        assert_eq!(b.encoded_len(), 6);
        assert_eq!(area.terminator_offset(), 6);
    }

    #[test]
    fn truncated_and_unterminated_tables_are_errors() {
        // count=5 but only 2 body bytes: truncated.
        assert_eq!(
            CodebookSeedArea::from_bytes(vec![5, 1, 2]).unwrap_err(),
            SeedAreaError::Truncated { block_offset: 0 }
        );
        // Complete block but no terminator.
        assert_eq!(
            CodebookSeedArea::from_bytes(vec![1, 1, 2, 3]).unwrap_err(),
            SeedAreaError::MissingTerminator
        );
        // Empty buffer.
        assert_eq!(
            CodebookSeedArea::from_bytes(Vec::new()).unwrap_err(),
            SeedAreaError::MissingTerminator
        );
    }

    #[test]
    fn unsigned_count_reads_first_block_whole() {
        // The count is unsigned (spec/04 §5.2.1): 0xC3 = 195, not
        // -61. The first block must span 392 bytes.
        let area = CodebookSeedArea::load();
        let first = &area.blocks()[0];
        assert_eq!(first.offset, 0);
        assert_eq!(first.count, 195);
        assert_eq!(first.encoded_len(), 392);
        assert_eq!(area.blocks()[1].offset, 392);
    }
}
