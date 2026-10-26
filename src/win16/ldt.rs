//! Selectors: the entries of the Win16 process's LDT that 16-bit code
//! uses for its segments, given out from `FIRST` on as Windows 3.1 gave
//! them out. A block of more than 64 KB has a selector for each 64 KB of
//! it, `__AHINCR` (8) apart, so that huge pointers step through it: the
//! first selector's base is the block's and its limit the whole block's,
//! and each next one starts 64 KB further with a limit 64 KB smaller, as
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

/// The selector of LDT entry `index`, at level 3.
pub fn selector(index: u16) -> u16 {
    index << 3 | 7
}

/// The LDT index of `selector`.
pub fn index(selector: u16) -> u16 {
    selector >> 3
}

/// Which LDT entries are taken, a bit each.
#[derive(Clone, Debug, Default)]
pub struct Ldt {
    used: Vec<u64>,
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

    /// Take `count` entries in a row; the first one's selector.
    pub fn alloc(&mut self, count: u16) -> Option<u16> {
        let count = count.max(1);
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
            self.set(i, false);
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

/// Write the descriptor of `selector`. A data or stack segment register
/// that holds it takes the new descriptor, as the DPMI host of Windows
/// reloads them: VBRUN300 grows its DGROUP with GlobalReAlloc and moves
/// SP past the old limit without loading SS again.
pub fn write(cpu: &mut Cpu, system: u32, selector: u16, desc: u64) {
    let at = entry_phys(system, selector);
    cpu.bus.write_32(at, desc as u32);
    cpu.bus.write_32(at + 4, (desc >> 32) as u32);
    if desc & PRESENT == 0 || !cpu.pe() || cpu.v86() {
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

/// A 16-bit segment's descriptor: `base`, `limit`, an access byte.
pub fn segment(base: u32, limit: u32, access: u8) -> u64 {
    descriptor(base, limit, access, 0)
}

/// Point the `count` selectors from `selector` at a block of `size`
/// bytes at `base`, as Windows tiles a huge block.
pub fn set_block(cpu: &mut Cpu, system: u32, selector: u16, base: u32, size: u32, access: u8) {
    let count = tiles(size);
    for i in 0..count {
        let offset = i as u32 * 0x1_0000;
        let limit = size.max(1) - 1 - offset;
        let sel = selector + i * AHINCR;
        write(cpu, system, sel, segment(base + offset, limit, access));
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

crate::state_fields!(Ldt { used });
