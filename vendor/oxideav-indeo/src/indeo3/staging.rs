//! Indeo 3 codebook staging image (`spec/04 §5.2`, Extractor
//! round 17).
//!
//! Spec source: `docs/video/indeo/indeo3/spec/04-vq-codebooks.md`
//! §5.2/§5.2.2/§5.2.3 (the corrected codec-init derivation) and the
//! staged ground truth `tables/03-vq-staging-blocks.csv` /
//! `tables/03-vq-staging-words.csv`.
//!
//! The routine at `IR32_32.DLL!0x10006308` does **not** fill the
//! per-frame arena: it builds a separate 0x18000-byte **codebook
//! staging image** on the heap (stashing its base at
//! `.data 0x1004d25a`), and the per-frame `alt_quant[]` overlay
//! (`spec/04 §6`, [`super::VqArena::apply_alt_quant`]) later copies
//! whole 1 KB sub-tables out of it. The image tiles as:
//!
//! | Image offset | Size | Role |
//! | ------------ | ---- | ---- |
//! | `+0x0000..+0xbfff`  | 48 KB | 24 blocks × 0x800 — the codebook table set |
//! | `+0xc000..+0x17fff` | 48 KB | 24 blocks × 0x800 — the byte-replicated table set |
//!
//! with two 256-DWORD sub-tables per 0x800 block (`+0x000` and
//! `+0x400`). The build is three passes (`spec/04 §5.2`):
//!
//! 1. **Prefill** — every DWORD from byte offset 4 through 0x17ffc is
//!    set to its own byte offset from the image base (DWORD 0 is not
//!    written).
//! 2. **Seeded words** — seed-area block `q` writes pair `k`'s
//!    §5.2.2 word into word `k` of both halves of image block `q`,
//!    the byte-replicated form into the `+0xc000` set, and its
//!    bit-31 complement into the `+0xc400` sub-table.
//! 3. **Expansion** — `d²` further words (`d = |E|`) over ordered
//!    pairs `(i, j)`: `word = (B[i] << 16) + sext16(B[j])` as a
//!    32-bit sum, `j` outermost for `E ≥ 0` and `i` outermost for
//!    `E < 0`. The `+0xc000` sub-table receives the `j`-indexed
//!    replication; the `+0xc400` sub-table the `i`-indexed one, drawn
//!    from the bit-31-complemented seeded form exactly when `E < 0`.
//!
//! Word indices from `count + d²` through 255 keep the prefill —
//! self-referential byte offsets, not codebook data.
//!
//! The builder below reproduces the staged
//! `tables/03-vq-staging-words.csv` for **all 24 × 256 × 4 words with
//! zero mismatches** (pinned by the FNV-1a digest + spot rows in the
//! tests).

use super::codebook_seed::CodebookSeedArea;

/// Spec/04 §5.2 — the staging image's byte length.
pub const STAGING_IMAGE_LEN: usize = 0x18000;

/// Spec/04 §5.2 — blocks per table set (matches the seed area's 24
/// blocks).
pub const STAGING_BLOCK_COUNT: usize = 24;

/// Spec/04 §5.2 — per-block stride within the image.
pub const STAGING_BLOCK_STRIDE: usize = 0x800;

/// Spec/04 §5.2 — the byte-replicated table set's offset.
pub const STAGING_ALT_SET_OFFSET: usize = 0xc000;

/// Spec/04 §5.2 — each 0x800 block holds two 256-DWORD sub-tables at
/// `+0x000` and `+0x400`.
pub const STAGING_SUB_TABLE_LEN: usize = 0x400;

/// The 0x18000-byte codebook staging image the codec-init routine at
/// `IR32_32.DLL!0x10006308` builds from the seed area
/// ([`CodebookSeedArea`]), `spec/04 §5.2`.
#[derive(Clone, PartialEq, Eq)]
pub struct StagingImage {
    bytes: Vec<u8>,
}

impl core::fmt::Debug for StagingImage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StagingImage")
            .field("len", &self.bytes.len())
            .finish()
    }
}

impl StagingImage {
    /// Build the staging image from a parsed seed area (`spec/04
    /// §5.2` passes 1–3).
    pub fn build(area: &CodebookSeedArea) -> Self {
        let words = STAGING_IMAGE_LEN / 4;
        let mut img: Vec<u32> = Vec::with_capacity(words);
        // Pass 1 — prefill: DWORD w (w > 0) holds its own byte
        // offset; DWORD 0 is not written (left zero).
        img.push(0);
        for w in 1..words {
            img.push((w * 4) as u32);
        }

        for (q, block) in area.blocks().iter().enumerate().take(STAGING_BLOCK_COUNT) {
            let base = q * STAGING_BLOCK_STRIDE / 4;
            let alt = (STAGING_ALT_SET_OFFSET + q * STAGING_BLOCK_STRIDE) / 4;
            let half = STAGING_SUB_TABLE_LEN / 4;

            // Pass 2 — seeded words.
            for (k, pair) in block.pairs.iter().enumerate() {
                let w = pair.seeded_word();
                img[base + k] = w;
                img[base + half + k] = w;
                let r = pair.replicated_word();
                img[alt + k] = r;
                img[alt + half + k] = r ^ 0x8000_0000;
            }

            // Pass 3 — the d² expansion over ordered pairs.
            let d = block.expand_dim();
            let negative = block.expand < 0;
            let mut idx = block.pairs.len();
            let mut emit = |i: usize, j: usize, idx: usize| {
                let bi = block.pairs[i].value();
                let bj = block.pairs[j].value();
                let w = ((i32::from(bi) << 16).wrapping_add(i32::from(bj))) as u32;
                img[base + idx] = w;
                img[base + half + idx] = w;
                img[alt + idx] = block.pairs[j].replicated_word();
                img[alt + half + idx] = if negative {
                    block.pairs[i].replicated_word() ^ 0x8000_0000
                } else {
                    block.pairs[i].replicated_word()
                };
            };
            if negative {
                for i in 0..d {
                    for j in 0..d {
                        emit(i, j, idx);
                        idx += 1;
                    }
                }
            } else {
                for j in 0..d {
                    for i in 0..d {
                        emit(i, j, idx);
                        idx += 1;
                    }
                }
            }
        }

        let mut bytes = Vec::with_capacity(STAGING_IMAGE_LEN);
        for w in img {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        StagingImage { bytes }
    }

    /// The raw image bytes (little-endian DWORDs).
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The DWORD at byte offset `off` (`off + 4 <= 0x18000`).
    pub fn word_at(&self, off: usize) -> Option<u32> {
        let end = off.checked_add(4)?;
        let s = self.bytes.get(off..end)?;
        Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    /// Word `idx` (0..=255) of staging block `q`'s `+0x000` sub-table.
    pub fn block_word(&self, q: usize, idx: usize) -> Option<u32> {
        if q >= STAGING_BLOCK_COUNT || idx >= 256 {
            return None;
        }
        self.word_at(q * STAGING_BLOCK_STRIDE + 4 * idx)
    }

    /// The fixture-arbitrated per-row delta application (r451): one
    /// stream byte (plus an optional continuation byte) turns a
    /// predictor DWORD into an output DWORD through staging block
    /// `q`'s sub-tables, with the exact `spec/07 §2.1`/`§2.3`
    /// softSIMD arithmetic:
    ///
    /// 1. `sum = pred + word[+0x400 + 4·byte]` (32-bit wrapping).
    /// 2. Bit 31 clear → `sum` is the output (one-byte form).
    /// 3. Bit 31 set → a continuation byte is required:
    ///    `sum ^= 0x80008000`, then the 16-bit word at
    ///    `+0x402 + 4·continuation` is added to the low half only;
    ///    a still-set bit 31 is the binary's error-code-2 range
    ///    fault.
    ///
    /// **Byte-exact against the real IV32 fixture**: with the strip
    /// boundary predictor `0x40404040`, block 0 and the stream bytes
    /// `[6c 6c]` produce `0x0A0A0A0A` (the reference decode's exact
    /// top row, output `20`), and the follow-up byte `[d3]` produces
    /// `0x08080808` (output `16`) — see `tests/indeo3_fixtures.rs`.
    pub fn row_delta(
        &self,
        block: usize,
        byte: u8,
        continuation: Option<u8>,
        pred: u32,
    ) -> Option<RowDeltaOutcome> {
        if block >= STAGING_BLOCK_COUNT {
            return None;
        }
        let base = block * STAGING_BLOCK_STRIDE + STAGING_SUB_TABLE_LEN;
        let delta = self.word_at(base + 4 * usize::from(byte))?;
        let sum = pred.wrapping_add(delta);
        if sum & 0x8000_0000 == 0 {
            return Some(RowDeltaOutcome::Complete {
                value: sum,
                used_continuation: false,
            });
        }
        let Some(cont) = continuation else {
            return Some(RowDeltaOutcome::NeedsContinuation);
        };
        let x = sum ^ 0x8000_8000;
        let off = base + 4 * usize::from(cont) + 2;
        let s16 = u16::from_le_bytes([*self.bytes.get(off)?, *self.bytes.get(off + 1)?]);
        let lo = (x as u16).wrapping_add(s16);
        let out = (x & 0xffff_0000) | u32::from(lo);
        if out & 0x8000_0000 != 0 {
            return Some(RowDeltaOutcome::RangeFault);
        }
        Some(RowDeltaOutcome::Complete {
            value: out,
            used_continuation: true,
        })
    }
}

/// The outcome of one [`StagingImage::row_delta`] application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowDeltaOutcome {
    /// The output DWORD.
    Complete {
        /// The `pred + delta` result (post-continuation when used).
        value: u32,
        /// `true` when the two-byte form ran (`spec/06 §3.3`).
        used_continuation: bool,
    },
    /// Bit 31 set after the primary add and no continuation byte was
    /// supplied — the caller must consume the next stream byte and
    /// retry (`spec/06 §3.3`).
    NeedsContinuation,
    /// The `spec/07 §2.3` step-3 range fault (error code 2).
    RangeFault,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> StagingImage {
        StagingImage::build(&CodebookSeedArea::load())
    }

    /// FNV-1a 64 over the image bytes — pinned against the digest of
    /// the staged `tables/03-vq-staging-words.csv` ground truth
    /// (identical serialisation; zero word mismatches).
    fn fnv1a64(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    #[test]
    fn image_matches_staged_ground_truth_digest() {
        let img = image();
        assert_eq!(img.as_bytes().len(), STAGING_IMAGE_LEN);
        // Digest of the full staged 24 x 256 x 4 word content
        // (tables/03-vq-staging-words.csv re-serialised as the image's
        // little-endian DWORDs).
        assert_eq!(fnv1a64(img.as_bytes()), 0x60d8_ce28_421b_3ef5);
    }

    #[test]
    fn seeded_head_of_block_0() {
        // tables/03-vq-staging-words.csv block 0 words 0..5, all four
        // families.
        let img = image();
        assert_eq!(img.block_word(0, 0), Some(0x8000_0000));
        assert_eq!(img.block_word(0, 1), Some(0x8202_0000));
        assert_eq!(img.block_word(0, 2), Some(0x7dfe_0000));
        assert_eq!(img.block_word(0, 3), Some(0x82ff_0000));
        assert_eq!(img.block_word(0, 4), Some(0x7d01_0000));
        // +0x400 half mirrors the seeded words.
        assert_eq!(img.word_at(0x400), Some(0x8000_0000));
        assert_eq!(img.word_at(0x404), Some(0x8202_0000));
        // Replicated set + bit-31 complement.
        assert_eq!(img.word_at(0xc000), Some(0x0000_0000));
        assert_eq!(img.word_at(0xc004), Some(0x0202_0202));
        assert_eq!(img.word_at(0xc008), Some(0xfdfd_fdfe));
        assert_eq!(img.word_at(0xc400), Some(0x8000_0000));
        assert_eq!(img.word_at(0xc404), Some(0x8202_0202));
        assert_eq!(img.word_at(0xc408), Some(0x7dfd_fdfe));
    }

    #[test]
    fn expansion_and_prefill_boundaries() {
        let area = CodebookSeedArea::load();
        let img = StagingImage::build(&area);
        // Block 0: count 195, d = 7 -> expansion words 195..=243,
        // first prefill word at index 244 (staged
        // 03-vq-staging-blocks.csv column first_prefill_word_index).
        let b0 = &area.blocks()[0];
        assert_eq!(b0.count, 195);
        assert_eq!(b0.expand_dim(), 7);
        let first_prefill = 195 + 49;
        assert_eq!(first_prefill, 244);
        // The prefill value is the word's own byte offset.
        assert_eq!(
            img.block_word(0, first_prefill),
            Some((4 * first_prefill) as u32)
        );
        // The last expansion word (index 243) is codebook data, not
        // prefill: expansion of (i=6, j=6) with j outermost (E=7>=0)
        // -> B[6] in both halves.
        let b6 = b0.pairs[6].value();
        let want = ((i32::from(b6) << 16).wrapping_add(i32::from(b6))) as u32;
        assert_eq!(img.block_word(0, 243), Some(want));
    }

    #[test]
    fn negative_expand_blocks_transpose_and_flip() {
        // Block 16: count 128, E = -11 -> i outermost, and the
        // +0xc400 expansion words carry the bit-31-complemented
        // i-indexed replication. Expansion word 0 (index 128) is
        // (i=0, j=0): pair 0 of block 16.
        let area = CodebookSeedArea::load();
        let img = StagingImage::build(&area);
        let blk = &area.blocks()[16];
        assert_eq!(blk.count, 128);
        assert_eq!(blk.expand, -11);
        let p0 = blk.pairs[0];
        let want = ((i32::from(p0.value()) << 16).wrapping_add(i32::from(p0.value()))) as u32;
        assert_eq!(img.block_word(16, 128), Some(want));
        let alt = STAGING_ALT_SET_OFFSET + 16 * STAGING_BLOCK_STRIDE;
        assert_eq!(
            img.word_at(alt + 4 * 128),
            Some(p0.replicated_word()),
            "+0xc000 expansion is the j-indexed replication"
        );
        assert_eq!(
            img.word_at(alt + STAGING_SUB_TABLE_LEN + 4 * 128),
            Some(p0.replicated_word() ^ 0x8000_0000),
            "+0xc400 expansion flips bit 31 for E < 0"
        );
    }

    #[test]
    fn word_0_is_unwritten_zero_when_not_seeded() {
        // The prefill skips DWORD 0; block 0's pair 0 then seeds it.
        // A synthetic empty-ish area leaves it zero.
        let area = CodebookSeedArea::from_bytes(vec![1, 3, 3, 1, 0]).expect("parse");
        let img = StagingImage::build(&area);
        // Block 0 word 0 seeded by pair (3,3).
        assert_eq!(
            img.block_word(0, 0),
            Some(super::super::codebook_seed::SeedPair { a: 3, b: 3 }.seeded_word())
        );
        // Block 1 was never written: word 0 keeps the prefill.
        assert_eq!(img.block_word(1, 0), Some(0x800));
        // And the very first DWORD of an unseeded image stays 0:
        let empty = CodebookSeedArea::from_bytes(vec![0]).expect("parse");
        let img2 = StagingImage::build(&empty);
        assert_eq!(img2.word_at(0), Some(0));
        assert_eq!(img2.word_at(4), Some(4));
        assert_eq!(img2.word_at(0x17ffc), Some(0x17ffc));
    }

    #[test]
    fn out_of_range_accessors() {
        let img = image();
        assert_eq!(img.word_at(STAGING_IMAGE_LEN), None);
        assert_eq!(img.word_at(STAGING_IMAGE_LEN - 3), None);
        assert_eq!(img.block_word(24, 0), None);
        assert_eq!(img.block_word(0, 256), None);
    }
}
