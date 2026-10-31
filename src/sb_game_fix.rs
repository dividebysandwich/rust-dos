//! Fixes for games whose own Sound Blaster driver plays them badly on every
//! card, real ones too (`sb_game_fix`). rust-dos recognises such a driver
//! by its code once the game has loaded it, and patches that code in
//! memory before it runs, so the game sounds as it was meant to. Nothing
//! changes for a program whose code doesn't match.
//!
//! The check runs when a program starts a reset of the DSP (a write of 1
//! to base + 6), which a driver does before it sets anything up. It looks
//! through conventional memory for each known driver's first piece of
//! code, and patches only a copy where every piece is there as the game
//! shipped it and whose IRQ handler isn't in the interrupt table yet. A
//! driver that is already set up, as in a save state from a run without
//! the fix, has built its volume table and converted its effects the old
//! way, and patching the rest would make the mix wrap around; it stays
//! as it is until the game starts again.
//!
//! The drivers known are those of two Titus games of 1993 with the same
//! sound engine: Prehistorik 2 and Blues Brothers: Jukebox Adventure. The
//! engine mixes four channels of MOD music and the sound effects into
//! 8-bit unsigned blocks for DSP command 14h. It turns each signed sample
//! into an unsigned 0-63, with its centre at 32, scales that by the
//! note's volume through a table of `volume * sample / 64`, and adds the
//! channels to a block that starts at 0. So a channel's centre moves with
//! its volume, and a silent channel adds 0 where a playing one adds up to
//! 32: every note that starts, stops or changes volume moves the output's
//! centre line, which is the click at the start of every note, and the
//! output sits far below 128. The patches:
//!
//! - The volume table's loop runs the sample from -32 to 31 instead of
//!   0 to 63 and multiplies signed, so the table holds
//!   `(sample - 32) * volume / 64` in the same places.
//! - Every block starts at 128 instead of 0.
//! - The sound effects, which the driver copies into the block as they
//!   are, are stored around 128 instead of 32 when they are loaded.
//! - What the driver did when a sample ends, to make up for the jump to
//!   0 (Prehistorik 2 ramps the block down, Jukebox Adventure holds a
//!   value derived from the mix so far), goes: an ended channel now just
//!   adds nothing to the centre.
//!
//! Jukebox Adventure's MOD player also stops on effect Bxx (position
//! jump): before it calls its handler for the effect, a leftover
//! `cmp bx,0Bh / je $` jumps to itself, in the IRQ handler with
//! interrupts off, so the machine hangs the first time a song loops that
//! way. The patch takes the jump out, and the game's own handler runs.

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
    /// Where its IRQ handler starts.
    pub irq_handler: usize,
    /// What to change: the bytes there now, and what replaces them (of
    /// the same length). The first is what the search looks for.
    pub patches: &'static [(Bytes, &'static [u8])],
}

/// The IRQ handler's loop that mixes a channel into the block: lodsb es:,
/// xlat, add [di],al, and the step through the sample. Both games have it.
const MIX_LOOP: &[u8] = &[
    0x26, 0xAC, 0xD7, 0x00, 0x05, 0x47, 0x03, 0xD5, 0x8A, 0xC6, 0x32, 0xF6, 0x98, 0x03, 0xF0, 0x3B,
    0xF4, 0x77, 0x04, 0xE2, 0xEB,
];

/// A block without a sound effect: from xor ax,ax / mov cx,54h / rep
/// stosw to mov ax,8080h / mov cl,54h / rep stosw. CX is 0 there: a JCXZ
/// leads to it.
const SILENCE: (&[u8], &[u8]) = (
    &[0x33, 0xC0, 0xB9, 0x54, 0x00, 0xF3, 0xAB],
    &[0xB8, 0x80, 0x80, 0xB1, 0x54, 0xF3, 0xAB],
);

/// Prehistorik 2's driver (PRE2.SQZ as CDRUN.COM unpacks it), by offset
/// in its code segment: the game's `CS`, whose IRQ handler is at 204Dh.
const PREHISTORIK_2: Driver = Driver {
    game: "Prehistorik 2",
    checks: &[
        // The end of the volume table's loops: jne / inc cl / cmp cl,41h /
        // jne.
        Bytes {
            at: 0x1EA8,
            bytes: &[0x75, 0xE8, 0xFE, 0xC1, 0x80, 0xF9, 0x41, 0x75, 0xDF],
        },
        Bytes {
            at: 0x21E6,
            bytes: MIX_LOOP,
        },
        // The IRQ handler's start: push ax, bx, cx, dx.
        Bytes {
            at: 0x204D,
            bytes: &[0x50, 0x53, 0x51, 0x52],
        },
    ],
    irq_handler: 0x204D,
    patches: &[
        // The volume table: from
        //   xor ch,ch / mov al,cl / mul ch / shr ax,1 (6 times) / stosb /
        //   inc ch / cmp ch,40h
        // to
        //   mov ch,-20h / mov al,cl / imul ch / sar ax,1 (6 times) /
        //   stosb / inc ch / cmp ch,20h
        (
            Bytes {
                at: 0x1E90,
                bytes: &[
                    0x32, 0xED, 0x8A, 0xC1, 0xF6, 0xE5, 0xD1, 0xE8, 0xD1, 0xE8, 0xD1, 0xE8, 0xD1,
                    0xE8, 0xD1, 0xE8, 0xD1, 0xE8, 0xAA, 0xFE, 0xC5, 0x80, 0xFD, 0x40,
                ],
            },
            &[
                0xB5, 0xE0, 0x8A, 0xC1, 0xF6, 0xED, 0xD1, 0xF8, 0xD1, 0xF8, 0xD1, 0xF8, 0xD1, 0xF8,
                0xD1, 0xF8, 0xD1, 0xF8, 0xAA, 0xFE, 0xC5, 0x80, 0xFD, 0x20,
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
        //   sub ax,0A8h / neg ax / mov cx,ax / xor ax,ax / shr cx,1 /
        //   rep stosw
        // to
        //   mov cx,0A8h / sub cx,ax / shr cx,1 / mov ax,8080h /
        //   rep stosw / nop
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
        (
            Bytes {
                at: 0x2112,
                bytes: SILENCE.0,
            },
            SILENCE.1,
        ),
        // The ramp down after a sample that doesn't loop: from
        // mov al,[di-1] / or al,al / je to a jump past it (jmp +0Dh).
        (
            Bytes {
                at: 0x222A,
                bytes: &[0x8A, 0x45, 0xFF, 0x0A, 0xC0, 0x74, 0x08],
            },
            &[0xEB, 0x0D, 0x90, 0x90, 0x90, 0x90, 0x90],
        ),
    ],
};

/// Blues Brothers: Jukebox Adventure's driver (BLUES.EXE as PKLITE
/// unpacks it), by offset in its code segment, whose IRQ handler is at
/// 444Bh. The same engine assembled for the 286.
const JUKEBOX_ADVENTURE: Driver = Driver {
    game: "Blues Brothers: Jukebox Adventure",
    checks: &[
        Bytes {
            at: 0x4289,
            bytes: &[0x75, 0xF1, 0xFE, 0xC1, 0x80, 0xF9, 0x41, 0x75, 0xE8],
        },
        Bytes {
            at: 0x461E,
            bytes: MIX_LOOP,
        },
        Bytes {
            at: 0x444B,
            bytes: &[0x50, 0x53, 0x51, 0x52],
        },
    ],
    irq_handler: 0x444B,
    patches: &[
        // The volume table: from
        //   xor ch,ch / mov al,cl / mul ch / shr ax,6 / stosb / inc ch /
        //   cmp ch,40h
        // to
        //   mov ch,-20h / mov al,cl / imul ch / sar ax,6 / stosb / inc ch /
        //   cmp ch,20h
        (
            Bytes {
                at: 0x427A,
                bytes: &[
                    0x32, 0xED, 0x8A, 0xC1, 0xF6, 0xE5, 0xC1, 0xE8, 0x06, 0xAA, 0xFE, 0xC5, 0x80,
                    0xFD, 0x40,
                ],
            },
            &[
                0xB5, 0xE0, 0x8A, 0xC1, 0xF6, 0xED, 0xC1, 0xF8, 0x06, 0xAA, 0xFE, 0xC5, 0x80, 0xFD,
                0x20,
            ],
        ),
        // The sound effects: from add al,80h / shr al,2 to
        // sar al,2 / add al,80h.
        (
            Bytes {
                at: 0x4335,
                bytes: &[0x04, 0x80, 0xC0, 0xE8, 0x02],
            },
            &[0xC0, 0xF8, 0x02, 0x04, 0x80],
        ),
        // The rest of a block after an effect ends, which ramped down from
        // the effect's last byte to 0: from mov al,es:[di-1] / or al,al /
        // je to mov al,80h and a jump to its rep stosb (jmp +0Dh).
        (
            Bytes {
                at: 0x4501,
                bytes: &[0x26, 0x8A, 0x45, 0xFF, 0x0A, 0xC0, 0x74, 0x09],
            },
            &[0xB0, 0x80, 0xEB, 0x0D, 0x90, 0x90, 0x90, 0x90],
        ),
        (
            Bytes {
                at: 0x4552,
                bytes: SILENCE.0,
            },
            SILENCE.1,
        ),
        // A sample that doesn't loop ends: the value the channel adds for
        // the rest of the block, and holds until its next note, from
        // mov al,[di-1] / xlat / shr al,1 to xor al,al.
        (
            Bytes {
                at: 0x4669,
                bytes: &[0x8A, 0x45, 0xFF, 0xD7, 0xD0, 0xE8],
            },
            &[0x32, 0xC0, 0x90, 0x90, 0x90, 0x90],
        ),
        // Effect Bxx: cmp bx,0Bh / je $ loses the je.
        (
            Bytes {
                at: 0x47B7,
                bytes: &[0x83, 0xFB, 0x0B, 0x74, 0xFB],
            },
            &[0x83, 0xFB, 0x0B, 0x90, 0x90],
        ),
    ],
};

/// The drivers rust-dos knows.
pub const DRIVERS: &[Driver] = &[PREHISTORIK_2, JUKEBOX_ADVENTURE];

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
        // A match's index in what follows `first.at` is the base it would
        // be at.
        memchr::memmem::find_iter(ram.get(first.at..)?, first.bytes)
            .find(|&base| {
                driver.checks.iter().all(|b| has(ram, base, b))
                    && driver.patches.iter().all(|(old, _)| has(ram, base, old))
                    && !set_up(ram, base + driver.irq_handler)
            })
            .map(|base| (driver, base))
    })
}

/// Whether a vector in the interrupt table of `ram` points at the linear
/// address `handler`.
fn set_up(ram: &[u8], handler: usize) -> bool {
    ram.get(..0x400).is_some_and(|table| {
        table.as_chunks::<4>().0.iter().any(|&[o0, o1, s0, s1]| {
            let offset = u16::from_le_bytes([o0, o1]) as usize;
            let segment = u16::from_le_bytes([s0, s1]) as usize;
            (segment << 4) + offset == handler
        })
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
    fn finds_each_driver_wherever_it_is_loaded() {
        for driver in DRIVERS {
            for base in [0x1_2340, 0x8_0000] {
                let ram = memory(driver, base);
                let (found, at) = find(&ram).unwrap();
                assert_eq!((found.game, at), (driver.game, base));
            }
        }
    }

    #[test]
    fn leaves_code_that_differs_in_any_piece() {
        let base = 0x2_0000;
        for driver in DRIVERS {
            for b in driver
                .checks
                .iter()
                .chain(driver.patches.iter().map(|(old, _)| old))
            {
                let mut ram = memory(driver, base);
                ram[base + b.at + b.bytes.len() - 1] ^= 1;
                assert!(
                    find(&ram).is_none(),
                    "{} found with {:04X} changed",
                    driver.game,
                    b.at
                );
            }
        }
        assert!(find(&vec![0u8; 0xA0000]).is_none());
    }

    #[test]
    fn leaves_a_driver_that_is_already_set_up() {
        let base = 0x5_DA00;
        for driver in DRIVERS {
            let mut ram = memory(driver, base);
            // IRQ 7's vector (int 0Fh) at the handler, as 5DA0:offset.
            let vector = 0x0F * 4;
            let offset = driver.irq_handler as u16;
            ram[vector..vector + 2].copy_from_slice(&offset.to_le_bytes());
            ram[vector + 2..vector + 4].copy_from_slice(&0x5DA0u16.to_le_bytes());
            assert!(find(&ram).is_none(), "{} found once set up", driver.game);
        }
    }

    #[test]
    fn patches_once() {
        let base = 0x3_0000;
        for driver in DRIVERS {
            let mut ram = memory(driver, base);
            let (found, at) = find(&ram).unwrap();
            for (addr, byte) in writes(found, at).collect::<Vec<_>>() {
                ram[addr] = byte;
            }
            for (old, new) in driver.patches {
                assert_eq!(&ram[base + old.at..base + old.at + new.len()], *new);
            }
            assert!(
                find(&ram).is_none(),
                "{} is found again once patched",
                driver.game
            );
        }
    }

    #[test]
    fn patches_keep_their_lengths_and_stay_apart() {
        for driver in DRIVERS {
            let mut pieces: Vec<_> = driver
                .checks
                .iter()
                .chain(driver.patches.iter().map(|(old, _)| old))
                .collect();
            pieces.sort_by_key(|b| b.at);
            for pair in pieces.windows(2) {
                assert!(
                    pair[0].at + pair[0].bytes.len() <= pair[1].at,
                    "{} overlaps at {:04X}",
                    driver.game,
                    pair[1].at
                );
            }
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
