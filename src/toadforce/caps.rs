//! The capability table of each feature level, which `CAPS` shows
//! (chapter 3 of `docs/toadforce-spec.md`).
//!
//! A level is frozen once it is released: a driver that asks for it gets
//! these values from every later version of the emulator. The tests at
//! the end hold a copy of each table, so that a change to one fails
//! them. New capabilities make a new level.

/// The entries `CAPS` has room for after its count.
pub const ENTRIES: usize = 191;

/// The highest level this version of the card can have.
pub const NEWEST_LEVEL: u32 = 0;

/// Entry 1: the largest width and height of a surface in `FILL` and
/// `BLT`, and of a mode.
pub const SURFACE_MAX: u32 = 4096;
/// Entry 2: the formats `FILL` and `BLT` take, bit n for format code n.
pub const BLIT_FORMATS: u32 = 0x1E;
/// Entry 3: the formats a mode takes, the same way.
pub const MODE_FORMATS: u32 = 0x0E;
/// Entries 4 and 5: the lowest and highest `MODE_REFRESH`.
pub const REFRESH_MIN: u32 = 50;
pub const REFRESH_MAX: u32 = 200;

/// Level 0, display and 2D: entries 1 to 5.
const LEVEL_0: [u32; 5] = [
    SURFACE_MAX,
    BLIT_FORMATS,
    MODE_FORMATS,
    REFRESH_MIN,
    REFRESH_MAX,
];

/// The entries of `level`'s table, from entry 1 on.
fn table(level: u32) -> &'static [u32] {
    match level {
        0 => &LEVEL_0,
        _ => &[],
    }
}

/// `CAPS[index]` at `level`: the count of the entries after it at index
/// 0, then the entries, and 0 past them.
pub fn read(level: u32, index: usize) -> u32 {
    let table = table(level);
    match index {
        0 => table.len() as u32,
        _ => table.get(index - 1).copied().unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The released tables, copied by hand. A test that fails here means
    /// a released level changed: make a new level instead.
    const FROZEN: [&[u32]; 1] = [&[4096, 0x1E, 0x0E, 50, 200]];

    #[test]
    fn released_levels_keep_their_tables() {
        assert_eq!(FROZEN.len(), NEWEST_LEVEL as usize + 1);
        for (level, frozen) in FROZEN.iter().enumerate() {
            let level = level as u32;
            assert_eq!(read(level, 0), frozen.len() as u32, "level {level}");
            for (i, &value) in frozen.iter().enumerate() {
                assert_eq!(read(level, i + 1), value, "level {level}, entry {}", i + 1);
            }
            assert_eq!(read(level, frozen.len() + 1), 0);
            assert_eq!(read(level, ENTRIES), 0);
        }
    }

    #[test]
    fn a_higher_level_keeps_the_entries_of_the_ones_below() {
        for level in 1..=NEWEST_LEVEL {
            let below = table(level - 1);
            assert_eq!(&table(level)[..below.len()], below, "level {level}");
        }
        assert!(table(NEWEST_LEVEL).len() <= ENTRIES);
    }
}
