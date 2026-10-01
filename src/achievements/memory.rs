//! The machine's memory as the MS-DOS achievement sets address it.
//! rcheevos's MS-DOS map, as DOSBox Pure fills it:
//!
//! ```text
//! 000000-09FFFF  the conventional memory the game has, from where
//!                programs start (DOS and the BIOS's below it are left
//!                out, so the game is at the same address whatever DOS
//!                keeps)
//! 100000-19FFFF  the conventional memory below that
//! 200000-41FFFF  the memory from A0000h on: upper and extended memory
//! ```
//!
//! DOSBox Pure's programs start at segment 186h, where a program run from
//! the prompt has its environment (a 256-byte block at least) and its
//! PSP at 198h, 120h bytes in. Here programs start elsewhere, so the
//! game's memory is from 120h bytes below the first program's PSP: the
//! game, its data and what it allocates are at the addresses the sets
//! expect. A system booted from a disk has all conventional memory at 0.

use crate::dos_data::{HIGH, LOW, SYSVARS};
use crate::mcb::{MCB_M, MCB_Z};

/// The first MCB, from the word below the List of Lists where DOS keeps
/// it: the one of either layout DOS has (`dos_data::Layout`).
fn first_mcb(ram: &[u8]) -> u16 {
    let at = crate::dos_data::address(SYSVARS) - 2;
    let word = ram.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    match word {
        Some(seg) if seg == HIGH.first_mcb => seg,
        _ => LOW.first_mcb,
    }
}

/// Where DOSBox Pure's first program has its PSP, from the start of the
/// game's memory.
pub const PSP_OFFSET: usize = 0x120;
/// The end of conventional memory.
const CONVENTIONAL_END: usize = 0xA_0000;
/// Where the memory below the game's is.
const OS_BASE: u32 = 0x10_0000;
/// Where the memory from A0000h is, and the end of the map (64 MB of it).
const EXPANDED_BASE: u32 = 0x20_0000;
const MAP_END: u32 = 0x420_0000;

/// Where the game's memory starts in the machine's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryMap {
    /// The physical address of the game's memory's start; 0 for a booted
    /// system, whose conventional memory is all the game's.
    pub game_start: usize,
}

impl MemoryMap {
    /// The map for memory `ram` as it is: `booted` a system booted from a
    /// disk.
    pub fn of(ram: &[u8], booted: bool) -> Self {
        if booted {
            return Self { game_start: 0 };
        }
        let game_start = match first_program(ram) {
            Some(psp) => (psp as usize * 16).saturating_sub(PSP_OFFSET),
            // No program: where one would go, with the environment DOSBox
            // Pure gives it.
            None => first_mcb(ram) as usize * 16,
        };
        Self {
            game_start: game_start.min(CONVENTIONAL_END),
        }
    }

    /// The physical address of the sets' `address`, if it is in memory.
    pub fn physical(&self, address: u32) -> Option<usize> {
        let game_len = (CONVENTIONAL_END - self.game_start) as u32;
        if address < game_len {
            Some(self.game_start + address as usize)
        } else if (OS_BASE..OS_BASE + self.game_start as u32).contains(&address)
            && self.game_start > 0
        {
            Some((address - OS_BASE) as usize)
        } else if (EXPANDED_BASE..MAP_END).contains(&address) {
            Some(CONVENTIONAL_END + (address - EXPANDED_BASE) as usize)
        } else {
            None
        }
    }

    pub fn peek(&self, ram: &[u8], address: u32) -> u8 {
        self.physical(address)
            .and_then(|a| ram.get(a).copied())
            .unwrap_or(0)
    }
}

/// The PSP of the first program in the memory control blocks' chain: the
/// first block its owner's PSP is in.
fn first_program(ram: &[u8]) -> Option<u16> {
    let word = |at: usize| {
        ram.get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    };
    let mut seg = first_mcb(ram);
    for _ in 0..1000 {
        let at = seg as usize * 16;
        let signature = *ram.get(at)?;
        if signature != MCB_M && signature != MCB_Z {
            return None;
        }
        let owner = word(at + 1)?;
        let size = word(at + 3)?;
        if owner == seg.wrapping_add(1) {
            return Some(owner);
        }
        if signature == MCB_Z {
            return None;
        }
        seg = seg.checked_add(1)?.checked_add(size)?;
        if seg as usize * 16 >= CONVENTIONAL_END {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mcb(ram: &mut [u8], seg: u16, signature: u8, owner: u16, size: u16) {
        let at = seg as usize * 16;
        ram[at] = signature;
        ram[at + 1..at + 3].copy_from_slice(&owner.to_le_bytes());
        ram[at + 3..at + 5].copy_from_slice(&size.to_le_bytes());
    }

    #[test]
    fn the_game_is_where_dosbox_pure_has_it() {
        let mut ram = vec![0u8; 0x20_0000];
        // An environment of 10 paragraphs, then the program's block.
        let psp = LOW.first_mcb + 1 + 10 + 1;
        mcb(&mut ram, LOW.first_mcb, MCB_M, psp, 10);
        mcb(&mut ram, psp - 1, MCB_Z, psp, 0x9FFF - psp);
        ram[psp as usize * 16 + 0x80] = 0x42;
        ram[0x400] = 0x11;
        ram[0xA_0000 + 0x6_0000] = 0x77;
        let map = MemoryMap::of(&ram, false);
        assert_eq!(map.game_start, psp as usize * 16 - 0x120);
        // The PSP at 120h, as in DOSBox Pure.
        assert_eq!(map.peek(&ram, 0x120 + 0x80), 0x42);
        // DOS's memory at 100000h, and the memory above A0000h at 200000h.
        assert_eq!(map.peek(&ram, 0x10_0400), 0x11);
        assert_eq!(map.peek(&ram, 0x26_0000), 0x77);
        assert_eq!(map.peek(&ram, 0x1F_0000), 0, "padding");
        assert_eq!(map.physical(0xA_0000 - map.game_start as u32), None);

        // At the prompt, the free block.
        let mut ram = vec![0u8; 0x10_0000];
        mcb(&mut ram, LOW.first_mcb, MCB_Z, 0, 0x9FFF - LOW.first_mcb);
        assert_eq!(MemoryMap::of(&ram, false).game_start, 0xFFF0);
        // A booted system's is all of it.
        let booted = MemoryMap::of(&ram, true);
        assert_eq!(
            (booted.physical(0x400), booted.physical(0x10_0400)),
            (Some(0x400), None)
        );
    }
}
