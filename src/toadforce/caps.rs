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
pub const NEWEST_LEVEL: u32 = 1;
/// The highest level that is released, and so frozen. The levels above
/// it are being built: their tables and commands still change, and a
/// machine's card doesn't offer them.
pub const RELEASED_LEVEL: u32 = 0;

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

/// Entry 6: the largest width and height of a render target.
pub const TARGET_MAX: u32 = 2048;
/// Entry 7: the formats a render target's colour surface takes.
pub const TARGET_FORMATS: u32 = 0x1C;
/// Entry 8: the formats its depth buffer takes.
pub const DEPTH_FORMATS: u32 = 0x38_0000;
/// Entry 9: the formats a texture takes.
pub const TEXTURE_FORMATS: u32 = 0x7_FFFE;
/// Entry 10: the largest width and height of a texture.
pub const TEXTURE_MAX: u32 = 2048;
/// Entry 11: the stages that can have a texture in one draw.
pub const TEXTURES_AT_ONCE: u32 = 4;
/// Entry 12: the blend stages.
pub const STAGES: u32 = 8;
/// Entry 13: the highest texture handle.
pub const TEXTURE_HANDLES: u32 = 4095;
/// Entry 14: the lights.
pub const LIGHTS: u32 = 8;
/// Entry 15: the clip planes.
pub const CLIP_PLANES: u32 = 6;
/// Entry 16: the world matrices a vertex can be blended between.
pub const BLEND_MATRICES: u32 = 4;
/// Entry 17: the texture coordinate sets of a vertex.
pub const COORD_SETS: u32 = 8;
/// Entry 18: the highest `MAXANISOTROPY`.
pub const ANISOTROPY_MAX: u32 = 8;
/// Entry 19: the largest point size, in pixels.
pub const POINT_SIZE_MAX: u32 = 64;
/// Entry 20: the stage operations, bit n for operation n: all from
/// `DISABLE` (1) to `LERP` (26) but `PREMODULATE` (17).
pub const STAGE_OPS: u32 = 0x07FD_FFFE;

/// Level 1, fixed-function 3D: level 0's entries and entries 6 to 20.
const LEVEL_1: [u32; 20] = [
    SURFACE_MAX,
    BLIT_FORMATS,
    MODE_FORMATS,
    REFRESH_MIN,
    REFRESH_MAX,
    TARGET_MAX,
    TARGET_FORMATS,
    DEPTH_FORMATS,
    TEXTURE_FORMATS,
    TEXTURE_MAX,
    TEXTURES_AT_ONCE,
    STAGES,
    TEXTURE_HANDLES,
    LIGHTS,
    CLIP_PLANES,
    BLEND_MATRICES,
    COORD_SETS,
    ANISOTROPY_MAX,
    POINT_SIZE_MAX,
    STAGE_OPS,
];

/// The entries of `level`'s table, from entry 1 on.
fn table(level: u32) -> &'static [u32] {
    match level {
        0 => &LEVEL_0,
        1 => &LEVEL_1,
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
        assert_eq!(FROZEN.len(), RELEASED_LEVEL as usize + 1);
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
