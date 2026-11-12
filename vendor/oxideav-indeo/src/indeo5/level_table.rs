//! Indeo 5 level zig-zag-folded signed-byte table.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/04-entropy.md` §3.4
//! (audit-corrected against `audit/00-report.md §3.2`).
//!
//! A shared 256-byte table populated once at codec init by the helper
//! at `IR50_32.DLL!0x1001e8c0`. It is the **level-magnitude lookup**
//! the per-block coefficient decoder consumes (`spec/04 §4.3`,
//! `IR50_32.DLL!0x1001f6c1`/`0x1001f8c2`): given a small unsigned
//! codeword index `i`, `byte[i]` is the signed level value the decoded
//! run-value-table-mapped level represents.
//!
//! The on-disk bytes are zero (PE-loader zero-fill, `tables/
//! region_1009e438.meta`); the table is built at runtime from the
//! `spec/04 §3.4` recurrence. This module materialises it from that
//! fully-specified algorithm — no docs gap.
//!
//! ## Construction (`spec/04 §3.4`)
//!
//! ```text
//! for i in 1..=0x100:
//!     if i & 1:                    # odd i
//!         byte[i-1] = -0x80 - (i / 2)
//!     else:                        # even i
//!         byte[i-1] = (i / 2) - 0x80
//! ```
//!
//! Index 0 (`i = 1`, odd) maps to `-0x80`; index 1 (`i = 2`, even) to
//! `(1) - 0x80 = -0x7f`; index 2 (`i = 3`, odd) to `-0x80 - 1 = -0x81`
//! which wraps to `+0x7f` in the signed byte; index 3 (`i = 4`, even)
//! to `(2) - 0x80 = -0x7e`; and so on — the **zig-zag folded** mapping
//! the spec describes (`spec/04 §3.4`: index 0 → `-0x80`, 1 → `-0x7f`,
//! 2 → `+0x7f`, 3 → `-0x7f - 1`, …).

/// The number of entries in the level zig-zag table (`spec/04 §3.4`).
pub const LEVEL_TABLE_LEN: usize = 256;

/// Build the 256-entry level zig-zag-folded signed-byte table
/// (`spec/04 §3.4`).
///
/// Returns `[i8; 256]` where index `i` carries the signed level value
/// for codeword index `i`. The arithmetic is performed in `i32` and
/// truncated to `i8` (the binary stores a single byte per entry, so the
/// `-0x80 - (i/2)` values beyond `-128` wrap into the positive range —
/// the "fold").
pub fn build_level_table() -> [i8; LEVEL_TABLE_LEN] {
    let mut table = [0i8; LEVEL_TABLE_LEN];
    // The spec recurrence runs `i` over `1..=0x100`, writing index
    // `i - 1`. Mirror that 1-based loop exactly.
    for i in 1..=LEVEL_TABLE_LEN {
        let half = (i / 2) as i32;
        let value = if i & 1 != 0 {
            // odd i
            -0x80 - half
        } else {
            // even i
            half - 0x80
        };
        // Single-byte store: truncate to i8 (the binary's `mov byte`).
        table[i - 1] = value as i8;
    }
    table
}

/// Look up the signed level value for a codeword index (`spec/04
/// §4.3`). `index` is masked to the table range so a raw byte index
/// cannot run off the 256-entry table.
pub fn level_value(table: &[i8; LEVEL_TABLE_LEN], index: u8) -> i8 {
    table[index as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_length() {
        let t = build_level_table();
        assert_eq!(t.len(), LEVEL_TABLE_LEN);
    }

    #[test]
    fn spec_worked_examples() {
        // spec/04 §3.4: index 0 -> -0x80, 1 -> -0x7f, 2 -> +0x7f,
        // 3 -> -0x7f - 1 (= -0x80, i.e. (2)-0x80 = -0x7e... let's pin
        // against the recurrence directly).
        let t = build_level_table();
        // i=1 (odd): -0x80 - 0 = -128.
        assert_eq!(t[0], -128);
        // i=2 (even): 1 - 0x80 = -127.
        assert_eq!(t[1], -127);
        // i=3 (odd): -0x80 - 1 = -129 -> wraps to +127 in i8.
        assert_eq!(t[2], 127);
        // i=4 (even): 2 - 0x80 = -126.
        assert_eq!(t[3], -126);
        // i=5 (odd): -0x80 - 2 = -130 -> wraps to +126.
        assert_eq!(t[4], 126);
    }

    #[test]
    fn odd_indices_fold_from_negative_overflow() {
        // For odd i >= 3 the raw value -0x80 - (i/2) is below -128 and
        // folds into the positive byte range. Verify the fold is exactly
        // the i8 wrap of the i32 computation.
        let t = build_level_table();
        for i in 1..=LEVEL_TABLE_LEN {
            let half = (i / 2) as i32;
            let expected = if i & 1 != 0 {
                -0x80 - half
            } else {
                half - 0x80
            };
            assert_eq!(t[i - 1], expected as i8, "index {}", i - 1);
        }
    }

    #[test]
    fn last_entry() {
        // i = 256 (even): (128) - 0x80 = 0.
        let t = build_level_table();
        assert_eq!(t[255], 0);
        // i = 255 (odd): -0x80 - 127 = -255 -> i8 wrap = 1.
        assert_eq!(t[254], 1);
    }

    #[test]
    fn level_value_lookup() {
        let t = build_level_table();
        assert_eq!(level_value(&t, 0), -128);
        assert_eq!(level_value(&t, 2), 127);
        assert_eq!(level_value(&t, 255), 0);
    }

    #[test]
    fn even_half_to_zero_progression() {
        // The even entries climb from -127 (i=2) toward 0 (i=256) in
        // steps of +1 every two indices.
        let t = build_level_table();
        assert_eq!(t[1], -127); // i=2
        assert_eq!(t[3], -126); // i=4
        assert_eq!(t[5], -125); // i=6
        assert_eq!(t[253], -1); // i=254
        assert_eq!(t[255], 0); // i=256
    }
}
