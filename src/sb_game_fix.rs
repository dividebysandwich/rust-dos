//! Fixes for games whose own Sound Blaster driver plays them badly on every
//! card, real ones too (`sb_game_fix`). rust-dos recognises such a driver
//! by its code once the game has loaded it, and patches that code in
//! memory before it runs, so the game sounds as it was meant to. Nothing
//! changes for a program whose code doesn't match.
//!
//! The check runs when a program resets the DSP (a write to base + 6),
//! which a driver does before it sets anything up. It looks through
//! conventional memory for each known driver's first piece of code, and
//! patches only a copy where every piece is there as the game shipped it.
//!
//! Prehistorik 2 (Titus, 1993) mixes four channels of MOD music and its
//! sound effects into 8-bit unsigned blocks for DSP command 14h. It turns
//! each signed sample into an unsigned 0-63, with its centre at 32, scales
//! that by the note's volume through a table of `volume * sample / 64`,
//! and adds the channels to a block that starts at 0. So a channel's
//! centre moves with its volume, and a silent channel adds 0 where a
//! playing one adds about 16 or 32: every note that starts, stops or
//! changes volume moves the output's centre line, which is the click at
//! the start of every note, and the output sits far below 128. The patch
//! scales around the centre instead (`(sample - 32) * volume / 64`), starts
//! every block at 128, and stores the sound effects (which are copied into
//! the block as they are) around 128 too. The driver's ramp at the end of
//! a sample, which made up for the jump to 0, goes: a channel that ends
//! now just stops adding to the centre.
//!
//! Blues Brothers: Jukebox Adventure (Titus, 1993) has the same sound
//! engine and the same fault. Its driver isn't here yet: it needs its own
//! entry in `DRIVERS`, with its own offsets, made from the game's code.

/// Bytes at an offset in the driver's code segment.
pub struct Bytes {
    pub at: usize,
    pub bytes: &'static [u8],
}

/// A known driver: the code that identifies it, and what to change.
pub struct Driver {
    pub game: &'static str,
    /// Code that has to be there, unchanged, besides the patches' own.
    pub checks: &'static [Bytes],
    /// What to change: the bytes there now, and what replaces them (of
    /// the same length).
    pub patches: &'static [(Bytes, &'static [u8])],
}

/// Prehistorik 2's driver, by offset in its code segment (the game's
/// `CS`, the segment of its IRQ handler at 204Dh).
const PREHISTORIK_2: Driver = Driver {
    game: "Prehistorik 2",
    checks: &[
        // The rest of the loop that builds the volume table.
        Bytes {
            at: 0x1EA3,
            bytes: &[
                0xFE, 0xC5, 0x80, 0xFD, 0x40, 0x75, 0xE8, 0xFE, 0xC1, 0x80, 0xF9, 0x41, 0x75, 0xDF,
            ],
        },
        // The IRQ handler's loop that mixes a channel into the block:
        // lodsb es:, xlat, add [di],al, and the step through the sample.
        Bytes {
            at: 0x21E6,
            bytes: &[
                0x26, 0xAC, 0xD7, 0x00, 0x05, 0x47, 0x03, 0xD5, 0x8A, 0xC6, 0x32, 0xF6, 0x98, 0x03,
                0xF0, 0x3B, 0xF4, 0x77, 0x04, 0xE2, 0xEB,
            ],
        },
        // The IRQ handler's start: push ax, bx, cx, dx.
        Bytes {
            at: 0x204D,
            bytes: &[0x50, 0x53, 0x51, 0x52],
        },
    ],
    patches: &[
        // The volume table, 65 volumes by 64 samples: from
        //   mov al,cl / mul ch / shr ax,1 (six times) / stosb
        // (volume * sample / 64) to
        //   mov al,ch / sub al,20h / imul cl / shl ax,1 (twice) /
        //   mov al,ah / stosb
        // ((sample - 32) * volume / 64, signed).
        (
            Bytes {
                at: 0x1E92,
                bytes: &[
                    0x8A, 0xC1, 0xF6, 0xE5, 0xD1, 0xE8, 0xD1, 0xE8, 0xD1, 0xE8, 0xD1, 0xE8, 0xD1,
                    0xE8, 0xD1, 0xE8, 0xAA,
                ],
            },
            &[
                0x8A, 0xC5, 0x2C, 0x20, 0xF6, 0xE9, 0xD1, 0xE0, 0xD1, 0xE0, 0x8A, 0xC4, 0xAA, 0x90,
                0x90, 0x90, 0x90,
            ],
        ),
        // The sound effects, converted once as they are loaded: from
        // add al,80h / shr al,1 / shr al,1 (0 to 63) to
        // sar al,1 / sar al,1 / add al,80h (96 to 159).
        (
            Bytes {
                at: 0x1F42,
                bytes: &[0x04, 0x80, 0xD0, 0xE8, 0xD0, 0xE8],
            },
            &[0xD0, 0xF8, 0xD0, 0xF8, 0x04, 0x80],
        ),
        // The rest of a block after an effect ends: from
        //   sub ax,0A8h / neg ax / mov cx,ax / xor ax,ax / shr cx,1 / rep stosw
        // to
        //   mov cx,0A8h / sub cx,ax / shr cx,1 / mov ax,8080h / rep stosw / nop
        (
            Bytes {
                at: 0x20FB,
                bytes: &[
                    0x2D, 0xA8, 0x00, 0xF7, 0xD8, 0x8B, 0xC8, 0x33, 0xC0, 0xD1, 0xE9, 0xF3, 0xAB,
                ],
            },
            &[
                0xB9, 0xA8, 0x00, 0x2B, 0xC8, 0xD1, 0xE9, 0xB8, 0x80, 0x80, 0xF3, 0xAB, 0x90,
            ],
        ),
        // A block without an effect: from xor ax,ax / mov cx,54h to
        // mov ax,8080h / mov cl,54h (CX is 0 here: JCXZ led to it).
        (
            Bytes {
                at: 0x2112,
                bytes: &[0x33, 0xC0, 0xB9, 0x54, 0x00, 0xF3, 0xAB],
            },
            &[0xB8, 0x80, 0x80, 0xB1, 0x54, 0xF3, 0xAB],
        ),
        // The ramp down after a sample that doesn't loop: jump past it.
        (
            Bytes {
                at: 0x222A,
                bytes: &[0x8A, 0x45, 0xFF, 0x0A, 0xC0, 0x74, 0x08],
            },
            &[0xEB, 0x0D, 0x90, 0x90, 0x90, 0x90, 0x90],
        ),
    ],
};

/// The drivers rust-dos knows.
pub const DRIVERS: &[Driver] = &[PREHISTORIK_2];

/// Whether `ram` holds `b` at `base`.
fn has(ram: &[u8], base: usize, b: &Bytes) -> bool {
    ram.get(base + b.at..base + b.at + b.bytes.len()) == Some(b.bytes)
}

/// Find a known driver, as the game loaded it and not yet patched, in
/// `ram` (conventional memory): the driver and the linear address of its
/// code segment.
pub fn find(ram: &[u8]) -> Option<(&'static Driver, usize)> {
    DRIVERS.iter().find_map(|driver| {
        let (first, _) = &driver.patches[0];
        ram.windows(first.bytes.len())
            .enumerate()
            .filter(|(at, window)| *window == first.bytes && *at >= first.at)
            .map(|(at, _)| at - first.at)
            .find(|&base| {
                driver.checks.iter().all(|b| has(ram, base, b))
                    && driver.patches.iter().all(|(old, _)| has(ram, base, old))
            })
            .map(|base| (driver, base))
    })
}

/// The bytes to write for `driver` at `base`: (linear address, byte).
pub fn writes(driver: &Driver, base: usize) -> impl Iterator<Item = (usize, u8)> + '_ {
    driver.patches.iter().flat_map(move |(old, new)| {
        debug_assert_eq!(old.bytes.len(), new.len());
        new.iter()
            .enumerate()
            .map(move |(i, &byte)| (base + old.at + i, byte))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A copy of the driver's pieces at `base` in 640 KB of zeros.
    fn memory(driver: &Driver, base: usize) -> Vec<u8> {
        let mut ram = vec![0u8; 0xA0000];
        for b in driver
            .checks
            .iter()
            .chain(driver.patches.iter().map(|(old, _)| old))
        {
            ram[base + b.at..base + b.at + b.bytes.len()].copy_from_slice(b.bytes);
        }
        ram
    }

    #[test]
    fn finds_the_driver_wherever_it_is_loaded() {
        for base in [0x1_2340, 0x8_FF00] {
            let ram = memory(&PREHISTORIK_2, base);
            let (driver, at) = find(&ram).unwrap();
            assert_eq!((driver.game, at), ("Prehistorik 2", base));
        }
    }

    #[test]
    fn leaves_code_that_differs_in_any_piece() {
        let base = 0x2_0000;
        for b in PREHISTORIK_2
            .checks
            .iter()
            .chain(PREHISTORIK_2.patches.iter().map(|(old, _)| old))
        {
            let mut ram = memory(&PREHISTORIK_2, base);
            ram[base + b.at + b.bytes.len() - 1] ^= 1;
            assert!(find(&ram).is_none(), "found with {:04X} changed", b.at);
        }
        assert!(find(&vec![0u8; 0xA0000]).is_none());
    }

    #[test]
    fn patches_once() {
        let base = 0x3_0000;
        let mut ram = memory(&PREHISTORIK_2, base);
        let (driver, at) = find(&ram).unwrap();
        for (addr, byte) in writes(driver, at).collect::<Vec<_>>() {
            ram[addr] = byte;
        }
        for (old, new) in driver.patches {
            assert_eq!(&ram[base + old.at..base + old.at + new.len()], *new);
        }
        assert!(find(&ram).is_none(), "a patched driver is found again");
    }

    #[test]
    fn patches_keep_their_lengths() {
        for driver in DRIVERS {
            for (old, new) in driver.patches {
                assert_eq!(
                    old.bytes.len(),
                    new.len(),
                    "{} at {:04X}",
                    driver.game,
                    old.at
                );
            }
        }
    }
}
