//! Indeo 5 per-band run-value (rv) tables (`spec/05 §2` + the r338
//! static extraction).
//!
//! Sources: `docs/video/indeo/indeo5/tables/rv_tables_100972f4.*` (the
//! live-dump-located static `.data` array, 9 slots × 332 bytes,
//! staged r338 — see `provenance/11-…` Addendum 3 and the `spec/05 §7`
//! behavioural-resolution note) and `spec/05 §2` for the surrounding
//! mechanism (selection via `rv_tab_sel`, patch pairs, escape).
//!
//! ## Slot layout (r388 decode)
//!
//! Each 332-byte slot splits into the **A-subarray** (72 bytes, bound
//! to `[band+0x198]`) and the **B-subarray** (260 bytes, bound to
//! `[band+0x19c]`), plus a trailing u32 of undetermined role
//! ([`RvSlotData::aux`]). This module implements the decode semantics
//! arbitrated against the staged `IV50` fixtures (byte-exact band
//! exhaustion across both fixtures × three planes):
//!
//! * `A[0]` is a constant `1` (role undetermined); `A[1..]` is the
//!   per-run **magnitude-count array**: `counts[r]` = how many
//!   magnitudes (`1..=counts[r]`) run `r` can pair with. The run
//!   counts sum to 127 usable pairs (`2 × 127 + 2` markers = exactly
//!   the 256-value composite space); the final non-zero byte of `A`
//!   (a lone `1` after the zero tail) is not a run count — it closes
//!   the array — and runs with `counts[r] == 0` are representable
//!   only via the escape path.
//! * `B[vlc]` maps a decoded block-Huffman symbol (`0..=255`) to a
//!   **composite code**: `0` marks the EOB symbol, `1` marks the ESC
//!   symbol, and any other value `c` decodes as a `(run, val)` pair:
//!   composites `base_r..base_r + 2*counts[r]` (with `base_0 = 2`,
//!   `base_{r+1} = base_r + 2*counts[r]`) belong to run `r`, the
//!   negative magnitudes first (`-counts[r]..=-1`) then the positive
//!   ones (`+1..=+counts[r]`): with `mid_r = base_r + counts[r]`,
//!   `c >= mid_r` ⇒ `val = c - mid_r + 1`, `c < mid_r` ⇒ `val = c -
//!   mid_r`. **There is no level-0 entry** — every symbol carries a
//!   non-zero coefficient, matching `spec/05 §2.3`'s level index
//!   (`lidx = 0` is the end-of-block symbol only).
//!
//! The r451 "zero-inclusive" reading (the midpoint as a level-0
//! stuffing entry) is withdrawn: it rested on the all-flat fixture
//! parsed with its two tile phases unaligned (the MB-header phase and
//! the block-stream phase each start on a byte boundary, r459 — see
//! `decode`); aligned, the flat frame is one escape-coded DC and this
//! layout reproduces the 320×240 fixture's luma pixel-exactly.
//!
//! The `rv_tab_corr` band-header pairs (`spec/02 §3.4`, `spec/05
//! §2.4`) are **symbol-slot swaps**: each `(a, b)` pair exchanges
//! `B[a]` and `B[b]` — the kernel's per-entry copies of `run[sym]`
//! and `lidx[sym]` are transposed together, which is exactly the
//! `B` permutation — applied in list order (a slot named twice moves
//! twice). Measured on the 320×240 fixture's Y band: nine pairs
//! starting `(4,220) (4,6)`.

/// One static rv-table slot (the 332-byte records at
/// `IR50_32.DLL!.data 0x100972f4`, stride `0x14c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RvSlotData {
    /// `A[1..]` — per-run magnitude counts (trailing zeros trimmed;
    /// interior zeros are meaningful: runs with no direct pairs).
    pub counts: &'static [u8],
    /// `B` — the vlc-symbol → composite-code permutation.
    pub composites: [u8; 256],
    /// The trailing u32 at slot offset `0x148` (role undetermined —
    /// not consumed by the fixture-arbitrated decode).
    pub aux: u32,
}

/// One decoded rv-table entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RvEntry {
    /// End-of-block sentinel (composite `0`).
    Eob,
    /// Escape sentinel (composite `1`) — three more VLC symbols
    /// follow (`run`, `lindex_lo`, `lindex_hi`).
    Esc,
    /// A regular `(run, val)` pair: `run` zero coefficients, then the
    /// signed value `val`.
    Val {
        /// Zero-run before the value.
        run: u8,
        /// The signed (still-quantised) value.
        val: i16,
    },
}

/// Errors raised by the rv-table mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RvTableError {
    /// `rv_tab_sel` outside `0..=8`.
    BadSelector {
        /// The selector found.
        rv_tab_sel: u32,
    },
}

impl core::fmt::Display for RvTableError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RvTableError::BadSelector { rv_tab_sel } => write!(
                f,
                "indeo5 rv-table: selector {rv_tab_sel} outside 0..=8 (spec/02 §3.5)"
            ),
        }
    }
}

impl std::error::Error for RvTableError {}

/// One band's active rv-table: a static slot selected by
/// `rv_tab_sel`, with the band header's correction pairs applied as
/// entry swaps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RvTable {
    /// The (possibly swap-patched) vlc → composite permutation.
    composites: [u8; 256],
    /// Composite → entry decode table (256 entries).
    decode: [RvEntry; 256],
}

impl RvTable {
    /// Build the active table for a band: select slot `rv_tab_sel`
    /// (`8` = default slot) and apply the `rv_tab_corr` swap pairs in
    /// order (`spec/02 §3.4`).
    pub fn for_band(rv_tab_sel: u32, rv_tab_corr: &[(u8, u8)]) -> Result<Self, RvTableError> {
        let slot = RV_TABLE_SLOTS
            .get(rv_tab_sel as usize)
            .ok_or(RvTableError::BadSelector { rv_tab_sel })?;
        let mut composites = slot.composites;
        for &(a, b) in rv_tab_corr {
            composites.swap(a as usize, b as usize);
        }
        // Composite decode table from the counts.
        let mut decode = [RvEntry::Eob; 256];
        decode[1] = RvEntry::Esc;
        let mut base: u32 = 2;
        for (run, &cnt) in slot.counts.iter().enumerate() {
            let cnt = u32::from(cnt);
            let mid = base + cnt;
            for c in base..(base + 2 * cnt).min(256) {
                // Negative magnitudes below the midpoint, positive
                // ones from it up — no level-0 entry (r459 pixel
                // arbitration; see the module docs).
                let val = if c >= mid {
                    c as i16 - mid as i16 + 1
                } else {
                    c as i16 - mid as i16
                };
                decode[c as usize] = RvEntry::Val {
                    run: run as u8,
                    val,
                };
            }
            base += 2 * cnt;
            if base >= 256 {
                break;
            }
        }
        Ok(RvTable { composites, decode })
    }

    /// Look up a decoded block-Huffman symbol. Symbols outside the
    /// 256-entry composite space return `None` (observed only in the
    /// over-256 tail of wide custom codebooks; their mapping is a
    /// reported docs-gap).
    pub fn lookup(&self, vlc: u32) -> Option<RvEntry> {
        let c = *self.composites.get(vlc as usize)?;
        Some(self.decode[c as usize])
    }

    /// The vlc symbol currently mapped to EOB (composite `0`).
    pub fn eob_symbol(&self) -> u32 {
        self.composites.iter().position(|&c| c == 0).unwrap_or(0) as u32
    }

    /// The vlc symbol currently mapped to ESC (composite `1`).
    pub fn esc_symbol(&self) -> u32 {
        self.composites.iter().position(|&c| c == 1).unwrap_or(0) as u32
    }
}

/// The escape path's aggregated level index:
/// `lindex = lindex_lo | (lindex_hi << 6)` (`spec/05 §4.2`).
pub fn escape_lindex(lindex_lo: u32, lindex_hi: u32) -> u32 {
    (lindex_lo & 0x3f) | ((lindex_hi & 0x3f) << 6)
}

/// Fold an escape-path `lindex` to a signed value by the level
/// zig-zag convention extended past the 256-entry table (`spec/04
/// §3.4` recentred: `0 → 0`, odd `n → +(n+1)/2`, even `n → -n/2`).
/// Provisional: the staged fixtures exercise the escape path only
/// twice, which pins the *bit structure* (three VLCs) but not the
/// value fold — reported as an open item.
pub fn escape_value(lindex: u32) -> i16 {
    if lindex == 0 {
        0
    } else if lindex % 2 == 1 {
        (lindex.div_ceil(2)) as i16
    } else {
        -((lindex / 2) as i16)
    }
}

/// The scan-position advance after one decoded symbol:
/// `pos += run + 1` (the wiki "Block data" identity; `pos` starts at
/// `-1`).
pub fn run_advance(pos: i32, run: u8) -> i32 {
    pos + i32::from(run) + 1
}

include!("rv_table_data.rs");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_permutations_with_markers() {
        for (i, slot) in RV_TABLE_SLOTS.iter().enumerate() {
            let mut seen = [false; 256];
            for &c in &slot.composites {
                assert!(!seen[c as usize], "slot {i}: duplicate composite {c}");
                seen[c as usize] = true;
            }
            // Exactly one EOB and one ESC marker.
            assert_eq!(slot.composites.iter().filter(|&&c| c == 0).count(), 1);
            assert_eq!(slot.composites.iter().filter(|&&c| c == 1).count(), 1);
        }
    }

    #[test]
    fn counts_cover_the_composite_space() {
        // Σ counts == 128 per slot, of which the closing `1` after
        // the zero tail is not a run count: 127 run pairs × 2 signs +
        // the 2 markers fill the 256-value composite space exactly.
        for (i, slot) in RV_TABLE_SLOTS.iter().enumerate() {
            let total: u32 = slot.counts.iter().map(|&c| u32::from(c)).sum();
            assert_eq!(total, 128, "slot {i}");
            assert_eq!(*slot.counts.last().unwrap(), 1, "slot {i}");
            let runs: u32 = slot.counts[..slot.counts.len() - 1]
                .iter()
                .map(|&c| u32::from(c))
                .sum();
            assert_eq!(runs, 127, "slot {i}");
            // Every composite 2..=255 decodes to a non-zero level.
            let t = RvTable::for_band(i as u32, &[]).unwrap();
            for c in 2..256usize {
                match t.decode[c] {
                    RvEntry::Val { val, .. } => assert_ne!(val, 0, "slot {i} composite {c}"),
                    other => panic!("slot {i} composite {c}: {other:?}"),
                }
            }
        }
    }

    #[test]
    fn slot0_marker_positions() {
        // Slot 0: EOB at vlc 5, ESC at vlc 2 (B[5] == 0, B[2] == 1 in
        // the staged bytes).
        let t = RvTable::for_band(0, &[]).unwrap();
        assert_eq!(t.eob_symbol(), 5);
        assert_eq!(t.esc_symbol(), 2);
        assert_eq!(t.lookup(5), Some(RvEntry::Eob));
        assert_eq!(t.lookup(2), Some(RvEntry::Esc));
    }

    #[test]
    fn slot0_interval_decode() {
        // Slot 0 counts start [40, 14, ...]: run-0 interval spans
        // composites 2..=81 with midpoint 42, run-1 spans 82..=109
        // with midpoint 96. Staged B: B[0]=42, B[1]=41, B[3]=43,
        // B[4]=40, B[8]=96, B[9]=95. The midpoint is +1, the entry
        // below it -1 (no level-0 entry).
        let t = RvTable::for_band(0, &[]).unwrap();
        assert_eq!(t.lookup(0), Some(RvEntry::Val { run: 0, val: 1 }));
        assert_eq!(t.lookup(1), Some(RvEntry::Val { run: 0, val: -1 }));
        assert_eq!(t.lookup(3), Some(RvEntry::Val { run: 0, val: 2 }));
        assert_eq!(t.lookup(4), Some(RvEntry::Val { run: 0, val: -2 }));
        assert_eq!(t.lookup(8), Some(RvEntry::Val { run: 1, val: 1 }));
        assert_eq!(t.lookup(9), Some(RvEntry::Val { run: 1, val: -1 }));
    }

    #[test]
    fn slot4_fixture_y_band_mapping() {
        // The 320x240 fixture's Y band uses rv_tab_sel = 4: EOB rides
        // the 1-bit codeword (vlc 0); counts [89, 11, ...] put run
        // 0's midpoint (+1) at composite 91 and run 1's at 191. The
        // pixel oracle pins vlc 1 → +1, 2 → -1, 3 → +2, 10 → -3
        // (r459: every vertical-edge block of the fixture's top row).
        let t = RvTable::for_band(4, &[]).unwrap();
        assert_eq!(t.lookup(0), Some(RvEntry::Eob));
        assert_eq!(t.lookup(34), Some(RvEntry::Esc));
        assert_eq!(t.lookup(1), Some(RvEntry::Val { run: 0, val: 1 }));
        assert_eq!(t.lookup(2), Some(RvEntry::Val { run: 0, val: -1 }));
        assert_eq!(t.lookup(3), Some(RvEntry::Val { run: 0, val: 2 }));
        assert_eq!(t.lookup(10), Some(RvEntry::Val { run: 0, val: -3 }));
        assert_eq!(t.lookup(4), Some(RvEntry::Val { run: 1, val: 1 }));
        // The fixture's nine swap pairs move the top-row vertical-edge
        // symbols onto short codewords: 7 → (1,-5), 15 → (5,-2),
        // 8 → (9,-1), 6 → (13,-1), 220 → (1,+1) — each pinned
        // pixel-exactly by the fixture's top row of blocks.
        let corr = [
            (4, 220),
            (4, 6),
            (8, 61),
            (7, 80),
            (5, 23),
            (12, 59),
            (13, 171),
            (15, 100),
            (18, 103),
        ];
        let t = RvTable::for_band(4, &corr).unwrap();
        assert_eq!(t.lookup(34), Some(RvEntry::Esc));
        assert_eq!(t.lookup(4), Some(RvEntry::Val { run: 0, val: -2 }));
        assert_eq!(t.lookup(7), Some(RvEntry::Val { run: 1, val: -5 }));
        assert_eq!(t.lookup(15), Some(RvEntry::Val { run: 5, val: -2 }));
        assert_eq!(t.lookup(8), Some(RvEntry::Val { run: 9, val: -1 }));
        assert_eq!(t.lookup(6), Some(RvEntry::Val { run: 13, val: -1 }));
        assert_eq!(t.lookup(220), Some(RvEntry::Val { run: 1, val: 1 }));
    }

    #[test]
    fn default_slot8_markers() {
        let t = RvTable::for_band(super::super::band::DEFAULT_RV_TAB_SEL, &[]).unwrap();
        assert_eq!(t.eob_symbol(), 4);
        assert_eq!(t.esc_symbol(), 11);
    }

    #[test]
    fn corrections_swap_entries() {
        let base = RvTable::for_band(0, &[]).unwrap();
        let t = RvTable::for_band(0, &[(0, 5)]).unwrap();
        // vlc 0 and 5 exchanged: EOB moved to 0.
        assert_eq!(t.lookup(0), Some(RvEntry::Eob));
        assert_eq!(t.lookup(5), base.lookup(0));
        assert_eq!(t.eob_symbol(), 0);
    }

    #[test]
    fn bad_selector_rejected() {
        assert_eq!(
            RvTable::for_band(9, &[]),
            Err(RvTableError::BadSelector { rv_tab_sel: 9 })
        );
    }

    #[test]
    fn escape_helpers() {
        assert_eq!(escape_lindex(0x3f, 0x3f), 0xfff);
        assert_eq!(escape_lindex(0, 1), 0x40);
        assert_eq!(escape_value(0), 0);
        assert_eq!(escape_value(1), 1);
        assert_eq!(escape_value(2), -1);
        assert_eq!(escape_value(0xfff), 2048);
        assert_eq!(run_advance(-1, 0), 0);
        assert_eq!(run_advance(3, 2), 6);
    }

    #[test]
    fn aux_tail_values_recorded() {
        // The undetermined trailing u32 per slot, pinned so a future
        // semantic assignment is diffable against the staged bytes.
        let aux: Vec<u32> = RV_TABLE_SLOTS.iter().map(|s| s.aux).collect();
        assert_eq!(aux, [65, 65, 65, 16, 42, 26, 50, 51, 0]);
    }
}
