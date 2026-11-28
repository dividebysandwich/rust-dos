//! Selectors: the entries of the Win16 process's LDT that 16-bit code
//! uses for its segments, given out from `FIRST` on as Windows 3.1 gave
//! them out. A single selector freed is given out again before the
//! others, the last one freed first, as Windows 98's free list gives
//! them: a program started again after it ended then gets other
//! selectors than it had, which a probe on Windows 98 SE showed (#2280).
//! A block of more than 64 KB has a selector for each 64 KB of it,
//! `__AHINCR` (8) apart, so that huge pointers step through it: the first
//! selector's base is the block's and its limit the whole block's, and
//! each next one starts 64 KB further with a limit 64 KB smaller, as
//! Windows sets them.
//!
//! The LDT is the system block's (system.rs), in physical memory the
//! process holds, so its entries are written there directly.

use crate::cpu::{Cpu, Seg};
use crate::dpmi::descriptor;

/// The first LDT index given out: those below are the system's flat
/// selectors and the threads' TIB selectors (`MAX_TASK_THREADS` of them).
pub const FIRST: u16 = 0x80;
/// The selector increment of a huge block, and its shift.
pub const AHINCR: u16 = 8;
pub const AHSHIFT: u16 = 3;

/// Access bytes: present, level 3, data read/write or code readable, both
/// accessed.
pub const ACCESS_DATA: u8 = 0xF3;
pub const ACCESS_CODE: u8 = 0xFB;

/// How many LDT entries the Win16 process of a machine at `end` of a
/// link keeps out of use (`Ldt::reserve`): none without a link. Two
/// machines with the same built-in Windows would otherwise give a program
/// the same selectors, and a program can tell the two ends of a link
/// apart by them: Metal Marines takes the far pointer the other side
/// sends that equals its own for its own message coming back (#661). A
/// room member reserves its index (1 to 200); an instance not in a room
/// 256 to 319 entries by its settings and random bytes, which never
/// equals a member's.
pub fn reserved_for(end: Option<crate::net::LinkEnd>) -> u16 {
    use crate::net::LinkEnd;
    match end {
        None => 0,
        Some(LinkEnd::Member(index)) => index as u16,
        Some(LinkEnd::NoRoom(hash)) => 256 + (hash % 64) as u16,
    }
}

/// The selector of LDT entry `index`, at level 3.
pub fn selector(index: u16) -> u16 {
    index << 3 | 7
}

/// The LDT index of `selector`.
pub fn index(selector: u16) -> u16 {
    selector >> 3
}

/// The W32 version whose states keep the entries freed (`Ldt::freed`).
pub const FREED_SAVED: u16 = 117;

/// Which LDT entries are taken, a bit each, and the order the free ones
/// were freed in.
#[derive(Clone, Debug, Default)]
pub struct Ldt {
    used: Vec<u64>,
    /// The entries freed, the last one at the end. One taken again since
    /// stays in it until `alloc` comes to it.
    freed: crate::savestate::Since<FREED_SAVED, Vec<u16>>,
}

impl Ldt {
    fn taken(&self, i: u16) -> bool {
        self.used
            .get(i as usize / 64)
            .is_some_and(|w| w & 1 << (i % 64) != 0)
    }

    fn set(&mut self, i: u16, on: bool) {
        let word = i as usize / 64;
        if self.used.len() <= word {
            self.used.resize(word + 1, 0);
        }
        if on {
            self.used[word] |= 1 << (i % 64);
        } else {
            self.used[word] &= !(1 << (i % 64));
        }
    }

    /// Keep the `count` entries from `FIRST` out of use, so that every
    /// selector given out after comes `count` entries later. Their
    /// descriptors stay empty. A machine linked to another reserves some
    /// (`reserved_for`), as a real PC's drivers and the programs that ran
    /// before take selectors another PC's don't.
    pub fn reserve(&mut self, count: u16) {
        for i in FIRST..FIRST + count {
            self.set(i, true);
        }
    }

    /// Take `count` entries in a row; the first one's selector. A single
    /// entry is the one freed last that is still free, if there is one,
    /// and otherwise, as a run is, the first free one from `FIRST`.
    ///
    /// Windows 98 keeps its free selectors in a list and takes the one
    /// freed last. MCLIB.DLL, Memphis Math's library, caches its client's
    /// data by the client's SS, and a MATHPATH.EXE started after another
    /// ended found the first one's SS again and its freed data with it.
    pub fn alloc(&mut self, count: u16) -> Option<u16> {
        let count = count.max(1);
        if count == 1 {
            while let Some(i) = self.freed.pop() {
                if !self.taken(i) {
                    self.set(i, true);
                    return Some(selector(i));
                }
            }
        }
        let end = crate::win32::system::LDT_ENTRIES as u16;
        // More than the table holds, which would also wrap `start + count`.
        if count > end {
            return None;
        }
        let mut start = FIRST;
        while start + count <= end {
            match (start..start + count).find(|&i| self.taken(i)) {
                Some(busy) => start = busy + 1,
                None => {
                    for i in start..start + count {
                        self.set(i, true);
                    }
                    return Some(selector(start));
                }
            }
        }
        None
    }

    /// Give back `count` entries from `selector`'s.
    pub fn free(&mut self, selector: u16, count: u16) {
        let first = index(selector);
        for i in first..first.saturating_add(count.max(1)) {
            if self.taken(i) {
                self.set(i, false);
                self.freed.push(i);
            }
        }
        // Entries taken again by a run, and freed again, would be in the
        // list twice: keep it no longer than the table.
        if self.freed.len() > crate::win32::system::LDT_ENTRIES as usize {
            let mut seen = Ldt::default();
            let mut kept: Vec<u16> = Vec::new();
            for &i in self.freed.iter().rev() {
                if !self.taken(i) && !seen.taken(i) {
                    seen.set(i, true);
                    kept.push(i);
                }
            }
            kept.reverse();
            *self.freed = kept;
        }
    }

    /// Whether `selector` is one given out.
    pub fn is_allocated(&self, selector: u16) -> bool {
        selector & 4 != 0 && index(selector) >= FIRST && self.taken(index(selector))
    }

    /// Whether the `count` entries after `selector`'s are free, for a
    /// block that grows in place.
    pub fn free_after(&self, selector: u16, count: u16) -> bool {
        let first = index(selector) + 1;
        let end = crate::win32::system::LDT_ENTRIES as u16;
        first as u32 + count as u32 <= end as u32 && (first..first + count).all(|i| !self.taken(i))
    }

    /// Take the entries after `selector`'s (see `free_after`).
    pub fn take_after(&mut self, selector: u16, count: u16) {
        let first = index(selector) + 1;
        for i in first..first + count {
            self.set(i, true);
        }
    }
}

/// The physical address of `selector`'s entry in the LDT of the process
/// whose system block is at `system`.
fn entry_phys(system: u32, selector: u16) -> usize {
    (system + crate::win32::system::LDT_OFFSET + (selector & !7) as u32) as usize
}

/// The descriptor of `selector` in the running Win16 process's LDT.
pub fn read(cpu: &Cpu, system: u32, selector: u16) -> u64 {
    let at = entry_phys(system, selector);
    cpu.bus.read_32(at) as u64 | (cpu.bus.read_32(at + 4) as u64) << 32
}

/// Write the descriptor bits of `selector` and nothing else: no segment
/// register changes. Used where Windows drops a selector without the
/// DPMI free path (GlobalReAlloc shrinking its block).
pub fn write_desc(cpu: &mut Cpu, system: u32, selector: u16, desc: u64) {
    let at = entry_phys(system, selector);
    cpu.bus.write_32(at, desc as u32);
    cpu.bus.write_32(at + 4, (desc >> 32) as u32);
}

/// Write the descriptor of `selector`. A data or stack segment register
/// that holds it takes the new descriptor, as the DPMI host of Windows
/// reloads them: VBRUN300 grows its DGROUP with GlobalReAlloc and moves
/// SP past the old limit without loading SS again. A descriptor of 0
/// frees the selector, and the data segment registers that hold it are
/// zeroed, as DPMI's function 0001h does and Windows does when it frees
/// a block: Johnny Castaway's GlobalFree leaves the block's selector in
/// ES, which its next function pushes and pops. A context saved before
/// the free gets 0 for it when it is loaded (`system::load_context`).
pub fn write(cpu: &mut Cpu, system: u32, selector: u16, desc: u64) {
    write_desc(cpu, system, selector, desc);
    if !cpu.pe() || cpu.v86() {
        return;
    }
    if desc == 0 {
        for seg in [Seg::ES, Seg::DS, Seg::FS, Seg::GS] {
            if cpu.seg_cache(seg).selector | 3 == selector | 3 {
                let _ = cpu.load_segment(seg, 0);
            }
        }
        return;
    }
    if desc & PRESENT == 0 {
        return;
    }
    for seg in [Seg::ES, Seg::SS, Seg::DS, Seg::FS, Seg::GS] {
        let held = cpu.seg_cache(seg).selector;
        if held | 3 == selector | 3 {
            let _ = cpu.load_segment(seg, held);
        }
    }
}

/// A descriptor's present bit.
const PRESENT: u64 = 1 << 47;
/// A descriptor's D/B bit: 32-bit code, or a big stack.
const BIG: u64 = 1 << 54;

/// A 16-bit segment's descriptor: `base`, `limit`, an access byte.
pub fn segment(base: u32, limit: u32, access: u8) -> u64 {
    descriptor(base, limit, access, 0)
}

/// A segment's descriptor, 32-bit (D/B set) when `big` is.
pub fn segment_sized(base: u32, limit: u32, access: u8, big: bool) -> u64 {
    let desc = segment(base, limit, access);
    if big { desc | BIG } else { desc }
}

/// Whether the descriptor `desc` has the D/B bit.
pub fn is_big(desc: u64) -> bool {
    desc & BIG != 0
}

/// Point the `count` selectors from `selector` at a block of `size`
/// bytes at `base`, as Windows tiles a huge block; 32-bit ones when
/// `big` is, as a block whose program set the D bit stays when Windows
/// moves or resizes it (it sets the base and limit with DPMI, which keep
/// the rest of the descriptor).
pub fn set_block(
    cpu: &mut Cpu,
    system: u32,
    selector: u16,
    base: u32,
    size: u32,
    access: u8,
    big: bool,
) {
    let count = tiles(size);
    for i in 0..count {
        let offset = i as u32 * 0x1_0000;
        let limit = size.max(1) - 1 - offset;
        let sel = selector + i * AHINCR;
        write(cpu, system, sel, segment_sized(base + offset, limit, access, big));
    }
}

/// How many selectors a block of `size` bytes takes.
pub fn tiles(size: u32) -> u16 {
    size.max(1).div_ceil(0x1_0000).min(0x2000) as u16
}

/// The base of a descriptor.
pub fn base_of(desc: u64) -> u32 {
    crate::cpu::Descriptor(desc).base()
}

/// The limit of a descriptor, in bytes.
pub fn limit_of(desc: u64) -> u32 {
    crate::cpu::Descriptor(desc).limit()
}

crate::state_fields!(Ldt { used, freed });

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::LinkEnd;

    #[test]
    fn linked_machines_reserve_entries_no_other_end_does() {
        assert_eq!(reserved_for(None), 0);
        // Room members: their indices, which the relay gives once each.
        assert_eq!(reserved_for(Some(LinkEnd::Member(1))), 1);
        assert_eq!(reserved_for(Some(LinkEnd::Member(200))), 200);
        // Outside a room: past every member's, whatever the number.
        for hash in [0, 63, 64, u32::MAX] {
            assert!((256..320).contains(&reserved_for(Some(LinkEnd::NoRoom(hash)))));
        }
        let mut ldt = Ldt::default();
        ldt.reserve(3);
        assert_eq!(ldt.alloc(1), Some(selector(FIRST + 3)));
        assert!(!ldt.is_allocated(selector(FIRST + 4)));
    }

    #[test]
    fn the_selector_freed_last_comes_back_first() {
        // As Windows 98 SE gives them: six blocks, the 2nd and the 4th
        // freed, then two allocated get the 4th and then the 2nd.
        let mut ldt = Ldt::default();
        let six: Vec<u16> = (0..6).map(|_| ldt.alloc(1).unwrap()).collect();
        ldt.free(six[1], 1);
        ldt.free(six[3], 1);
        assert_eq!(ldt.alloc(1), Some(six[3]));
        assert_eq!(ldt.alloc(1), Some(six[1]));
        // With none freed, the first free entry.
        assert_eq!(ldt.alloc(1), Some(selector(FIRST + 6)));
    }

    #[test]
    fn a_run_takes_the_first_free_entries() {
        let mut ldt = Ldt::default();
        let a = ldt.alloc(1).unwrap();
        let b = ldt.alloc(1).unwrap();
        ldt.free(a, 1);
        // A run of two doesn't fit in the one entry freed.
        assert_eq!(ldt.alloc(2), Some(selector(FIRST + 2)));
        // The entry freed is still given out singly.
        assert_eq!(ldt.alloc(1), Some(a));
        // An entry freed and then taken by a run isn't given out again.
        ldt.free(b, 1);
        ldt.free(selector(FIRST + 2), 2);
        assert_eq!(ldt.alloc(3), Some(b));
        assert_eq!(ldt.alloc(1), Some(selector(FIRST + 4)));
    }

    #[test]
    fn the_freed_list_stays_within_the_table_and_keeps_its_order() {
        // A huge block allocated and freed over and over takes the same
        // run each time and puts its entries on the list each time.
        let mut ldt = Ldt::default();
        let a = ldt.alloc(1).unwrap();
        let b = ldt.alloc(1).unwrap();
        ldt.free(a, 1);
        for _ in 0..5000 {
            let run = ldt.alloc(2).unwrap();
            assert_eq!(run, selector(FIRST + 2));
            ldt.free(run, 2);
        }
        let entries = crate::win32::system::LDT_ENTRIES as usize;
        assert!(ldt.freed.len() <= entries, "{}", ldt.freed.len());
        // The run's entries, the last freed first, then the one freed
        // before them all, then the first free entry.
        assert_eq!(ldt.alloc(1), Some(selector(FIRST + 3)));
        assert_eq!(ldt.alloc(1), Some(selector(FIRST + 2)));
        assert_eq!(ldt.alloc(1), Some(a));
        assert_eq!(ldt.alloc(1), Some(selector(FIRST + 4)));
        assert!(ldt.is_allocated(b));
    }
}
