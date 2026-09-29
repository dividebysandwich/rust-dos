//! The machine's memory as RetroAchievements addresses it, for the
//! frontend's rcheevos: the MS-DOS map DOSBox Pure publishes, with the
//! game's conventional memory at 0 (see achievements/memory.rs). It moves
//! with the first program, so it is published again whenever it does.

use std::ffi::c_void;
use std::ptr;

use rust_dos::achievements::memory::MemoryMap;
use rust_dos::bus::Bus;

use crate::ffi::*;

const CONVENTIONAL_END: usize = 0xA_0000;
const OS_BASE: usize = 0x10_0000;
const EXPANDED_BASE: usize = 0x20_0000;
/// The map has 64 MB from A0000h on.
const EXPANDED_MAX: usize = 0x400_0000;

/// The regions of memory `ram_len` bytes long with the game's from
/// `game_start`: where each is in RAM, where in the map, and its length.
pub fn regions(game_start: usize, ram_len: usize) -> Vec<(usize, usize, usize)> {
    let game_start = game_start.min(CONVENTIONAL_END);
    let mut regions = vec![(game_start, 0, CONVENTIONAL_END - game_start)];
    if game_start > 0 {
        regions.push((0, OS_BASE, game_start));
    }
    if ram_len > CONVENTIONAL_END {
        regions.push((CONVENTIONAL_END, EXPANDED_BASE, (ram_len - CONVENTIONAL_END).min(EXPANDED_MAX)));
    }
    regions
}

/// The map the frontend was last given.
#[derive(Default)]
pub struct Published {
    map: Option<MemoryMap>,
    /// Kept for the frontend, which may read them until the next map.
    descriptors: Vec<retro_memory_descriptor>,
}

impl Published {
    /// The map as it is now, published if it changed (or `forget` was
    /// called). Returns whether it was.
    pub fn refresh(&mut self, bus: &mut Bus, env: retro_environment_t) -> bool {
        let map = MemoryMap::of(bus.ram(), bus.boot.is_some());
        if self.map == Some(map) {
            return false;
        }
        self.map = Some(map);
        let ram = bus.ram_mut();
        let base = ram.as_mut_ptr() as *mut c_void;
        self.descriptors = regions(map.game_start, ram.len())
            .into_iter()
            .map(|(offset, start, len)| retro_memory_descriptor {
                flags: RETRO_MEMDESC_SYSTEM_RAM,
                ptr: base,
                offset,
                start,
                select: 0,
                disconnect: 0,
                len,
                addrspace: ptr::null(),
            })
            .collect();
        let map = retro_memory_map { descriptors: self.descriptors.as_ptr(), num_descriptors: self.descriptors.len() as u32 };
        // SAFETY: SET_MEMORY_MAPS takes a retro_memory_map; the descriptors
        // live in `self` and point into the machine's RAM, which is never
        // reallocated.
        unsafe { env(RETRO_ENVIRONMENT_SET_MEMORY_MAPS, &map as *const _ as *mut c_void) };
        true
    }

    /// Publish the map again at the next refresh, as after a state is
    /// loaded.
    pub fn forget(&mut self) {
        self.map = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_game_is_at_zero_and_the_rest_above() {
        let ram = 16 << 20;
        assert_eq!(
            regions(0x2A00, ram),
            [(0x2A00, 0, 0xA0000 - 0x2A00), (0, 0x100000, 0x2A00), (0xA0000, 0x200000, ram - 0xA0000)]
        );
        // A booted system's conventional memory is all the game's.
        assert_eq!(regions(0, ram), [(0, 0, 0xA0000), (0xA0000, 0x200000, ram - 0xA0000)]);
        // 64 MB of the memory above.
        assert_eq!(regions(0x2A00, 128 << 20)[2].2, 0x400_0000);
    }
}
