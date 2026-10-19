//! A Win32 process's memory: physical pages from the pool all processes
//! share (`Pool`), a block of extended memory taken from the XMS pool when
//! the first process starts, and the page tables that put them at the
//! linear addresses Windows 95 has its parts at. The first megabyte (DOS,
//! the BIOS and video memory) is there as it is in physical memory, for
//! the system only.

use crate::bus::Bus;

pub const PAGE: u32 = 0x1000;

/// Page table entry bits.
pub const PRESENT: u32 = 0x001;
pub const WRITABLE: u32 = 0x002;
pub const USER: u32 = 0x004;

/// Windows 95's arenas: the private one of each process from 4 MB to
/// 2 GB, the shared one (system DLLs, memory-mapped files) to 3 GB, the
/// system's above.
pub const PRIVATE: u32 = 0x0040_0000;
pub const SHARED: u32 = 0x8000_0000;
pub const SYSTEM: u32 = 0xC000_0000;

/// Linear address space is given out in 64 KB pieces, as by Windows.
pub const GRANULARITY: u32 = 0x1_0000;

/// Page protections (VirtualAlloc's PAGE_ values): one of these bits.
pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_READONLY: u32 = 0x02;
pub const PAGE_READWRITE: u32 = 0x04;
pub const PAGE_WRITECOPY: u32 = 0x08;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub const PAGE_EXECUTE_WRITECOPY: u32 = 0x80;

/// Where a page table entry keeps the page's protection: the bits the
/// processor leaves to the system, as the bit number of its PAGE_ value.
const PROTECTION_SHIFT: u32 = 9;

/// Region types (VirtualQuery's MEM_ values).
pub const MEM_PRIVATE: u32 = 0x2_0000;
pub const MEM_MAPPED: u32 = 0x4_0000;
pub const MEM_IMAGE: u32 = 0x100_0000;

/// A reserved range of linear address space.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Region {
    pub start: u32,
    pub len: u32,
    /// The protection it was reserved with (0 for the system's own), and
    /// its type: MEM_PRIVATE, MEM_MAPPED for a view of a file mapping, or
    /// MEM_IMAGE for a module's image.
    pub protect: u32,
    pub kind: u32,
}

impl Region {
    pub fn end(&self) -> u32 {
        self.start + self.len
    }

    pub fn contains(&self, lin: u32) -> bool {
        (self.start..self.end()).contains(&lin)
    }
}

/// The entry bits for a committed page with the protection `protect` (a
/// PAGE_ value): there for the program to read, and to write where the
/// protection allows it, or with PAGE_NOACCESS not there at all; and the
/// protection itself, which VirtualQuery gives back.
pub fn protection_bits(protect: u32) -> u32 {
    let base = protect & 0xFF;
    let hardware = match base {
        PAGE_NOACCESS => 0,
        PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY => {
            PRESENT | USER | WRITABLE
        }
        _ => PRESENT | USER,
    };
    hardware | base.trailing_zeros().min(7) << PROTECTION_SHIFT
}

/// The protection of the page whose entry is `entry`, if it is committed.
/// Pages the system put there without one (the program's image, stacks)
/// have the protection their bits give.
pub fn protection(entry: u32) -> Option<u32> {
    if entry == 0 {
        return None;
    }
    let tagged = 1 << (entry >> PROTECTION_SHIFT & 7);
    Some(if entry & PRESENT == 0 {
        PAGE_NOACCESS
    } else if tagged != PAGE_NOACCESS {
        tagged
    } else if entry & USER == 0 {
        PAGE_NOACCESS
    } else if entry & WRITABLE != 0 {
        PAGE_READWRITE
    } else {
        PAGE_READONLY
    })
}

/// The physical pages the processes take theirs from.
#[derive(Clone, Debug, Default)]
pub struct Pool {
    /// The block of extended memory: its physical address and length.
    pub base: u32,
    pub len: u32,
    /// The offset of the first page in it never used yet, and the pages
    /// given back.
    next: u32,
    free: Vec<u32>,
}

impl Pool {
    /// Take the largest free block of extended memory, if it is at least
    /// `min` bytes.
    pub fn take(bus: &mut Bus, min: u32) -> Option<Pool> {
        let end = bus.ram().len() as u32;
        let (largest, _) = bus.xms.dpmi_free(end);
        if largest < min {
            return None;
        }
        let base = bus.xms.take_dpmi(largest, end)?;
        Some(Pool {
            base,
            len: largest,
            ..Pool::default()
        })
    }

    /// Give the block back to the XMS pool.
    pub fn release(&self, bus: &mut Bus) {
        bus.xms.release_dpmi(self.base);
    }

    /// Whether the physical address `phys` is in the pool.
    fn has(&self, phys: u32) -> bool {
        (self.base..self.base + self.len).contains(&phys)
    }

    /// The bytes of the pages it has left.
    pub fn available(&self) -> u32 {
        self.len - self.next + self.free.len() as u32 * PAGE
    }
}

#[derive(Clone, Debug, Default)]
pub struct Memory {
    /// The page directory's physical address.
    pub cr3: u32,
    /// Reserved linear address ranges, in order.
    regions: Vec<Region>,
}

impl Memory {
    /// An empty page directory with the first megabyte mapped for the
    /// system, its pages from the pool.
    pub fn new(bus: &mut Bus) -> Option<Memory> {
        let mut memory = Memory::default();
        memory.cr3 = memory.page(bus)?;
        for page in 0..0x100 {
            memory.map(bus, page * PAGE, page * PAGE, WRITABLE)?;
        }
        Some(memory)
    }

    /// Give every page it has back to the pool: the pages mapped, the
    /// page tables and the directory.
    pub fn release(&self, bus: &mut Bus) {
        let Some(pool) = bus.win32.pool.as_ref() else {
            return;
        };
        let pool = pool.clone();
        let mut pages = Vec::new();
        for pde in 0..1024u32 {
            let table = bus.read_32((self.cr3 + 4 * pde) as usize);
            if table & PRESENT == 0 {
                continue;
            }
            let table = table & !0xFFF;
            for pte in 0..1024u32 {
                // (Committed pages the program may not reach have their
                // physical page in an entry that isn't present.)
                let entry = bus.read_32((table + 4 * pte) as usize);
                if entry != 0 && pool.has(entry & !0xFFF) && !bus.win32.shared(entry & !0xFFF) {
                    pages.push(entry & !0xFFF);
                }
            }
            pages.push(table);
        }
        pages.push(self.cr3);
        if let Some(pool) = bus.win32.pool.as_mut() {
            pool.free.extend(pages);
        }
    }

    /// A zeroed physical page.
    pub fn page(&mut self, bus: &mut Bus) -> Option<u32> {
        pool_page(bus)
    }

    /// `count` zeroed physical pages in a row, from memory never used yet.
    pub fn pages(&mut self, bus: &mut Bus, count: u32) -> Option<u32> {
        let pool = bus.win32.pool.as_mut()?;
        let len = count * PAGE;
        if pool.next + len > pool.len {
            return None;
        }
        let first = pool.base + pool.next;
        pool.next += len;
        bus.fill_ram(first as usize..(first + len) as usize, 0);
        Some(first)
    }

    /// Take the page at `lin` away (committed, if not present), giving it
    /// back to the pool unless a file mapping has it. Its physical
    /// address, if one was there.
    pub fn unmap(&mut self, bus: &mut Bus, lin: u32) -> Option<u32> {
        let table = bus.read_32((self.cr3 + 4 * (lin >> 22)) as usize);
        if table & PRESENT == 0 {
            return None;
        }
        let pte = ((table & !0xFFF) + 4 * ((lin >> 12) & 0x3FF)) as usize;
        let entry = bus.read_32(pte);
        if entry == 0 {
            return None;
        }
        bus.write_32(pte, 0);
        let phys = entry & !0xFFF;
        let shared = bus.win32.shared(phys);
        if let Some(pool) = bus.win32.pool.as_mut()
            && pool.has(phys)
            && !shared
        {
            pool.free.push(phys);
        }
        Some(phys)
    }

    /// Write the page table entry of `lin`, making its page table if
    /// there is none yet.
    pub fn set_entry(&mut self, bus: &mut Bus, lin: u32, entry: u32) -> Option<()> {
        let pde = (self.cr3 + 4 * (lin >> 22)) as usize;
        let mut table = bus.read_32(pde);
        if table & PRESENT == 0 {
            // Directory entries allow everything; the pages' entries say
            // what each allows.
            table = self.page(bus)? | PRESENT | WRITABLE | USER;
            bus.write_32(pde, table);
        }
        let pte = ((table & !0xFFF) + 4 * ((lin >> 12) & 0x3FF)) as usize;
        bus.write_32(pte, entry);
        Some(())
    }

    /// Commit the page at `lin` with the protection `protect` (a PAGE_
    /// value): a zeroed page from the pool.
    pub fn commit_page(&mut self, bus: &mut Bus, lin: u32, protect: u32) -> Option<()> {
        let phys = self.page(bus)?;
        if self
            .set_entry(bus, lin, phys | protection_bits(protect))
            .is_none()
        {
            if let Some(pool) = bus.win32.pool.as_mut() {
                pool.free.push(phys);
            }
            return None;
        }
        Some(())
    }

    /// Give the committed page at `lin` the protection `protect`, its
    /// physical page and accessed and dirty bits kept. False if it isn't
    /// committed.
    pub fn set_protection(&mut self, bus: &mut Bus, lin: u32, protect: u32) -> bool {
        match self.entry(bus, lin) {
            Some(entry) if entry != 0 => {
                let kept = entry & (!0xFFF | 0x60);
                self.set_entry(bus, lin, kept | protection_bits(protect))
                    .is_some()
            }
            _ => false,
        }
    }

    /// The page table entry of `lin`, if its page table is there.
    pub fn entry(&self, bus: &Bus, lin: u32) -> Option<u32> {
        let table = bus.read_32((self.cr3 + 4 * (lin >> 22)) as usize);
        (table & PRESENT != 0)
            .then(|| bus.read_32(((table & !0xFFF) + 4 * ((lin >> 12) & 0x3FF)) as usize))
    }

    /// The reserved ranges, in order.
    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    /// The reserved range `lin` is in.
    pub fn region(&self, lin: u32) -> Option<&Region> {
        let i = self.regions.partition_point(|r| r.end() <= lin);
        self.regions.get(i).filter(|r| r.contains(lin))
    }

    /// Take the reservation that starts at `start` away. Its length, if
    /// there was one.
    pub fn unreserve(&mut self, start: u32) -> Option<u32> {
        let i = self.regions.iter().position(|r| r.start == start)?;
        Some(self.regions.remove(i).len)
    }

    /// Map the physical page `phys` at `lin`, with the entry bits `flags`
    /// (present added), making its page table if there is none yet.
    pub fn map(&mut self, bus: &mut Bus, lin: u32, phys: u32, flags: u32) -> Option<()> {
        self.set_entry(bus, lin, phys & !0xFFF | flags | PRESENT)
    }

    /// The physical address `lin` is mapped to, if it is.
    pub fn translate(&self, bus: &Bus, lin: u32) -> Option<u32> {
        let table = bus.read_32((self.cr3 + 4 * (lin >> 22)) as usize);
        if table & PRESENT == 0 {
            return None;
        }
        let entry = bus.read_32(((table & !0xFFF) + 4 * ((lin >> 12) & 0x3FF)) as usize);
        (entry & PRESENT != 0).then_some(entry & !0xFFF | lin & 0xFFF)
    }

    /// Back `len` bytes at `lin` (page aligned) with zeroed pages.
    pub fn commit(&mut self, bus: &mut Bus, lin: u32, len: u32, flags: u32) -> Option<()> {
        for page in (lin..lin + len).step_by(PAGE as usize) {
            let phys = self.page(bus)?;
            self.map(bus, page, phys, flags)?;
        }
        Some(())
    }

    /// Reserve `len` bytes of linear address space in `lo..hi`: at `at`,
    /// None if that isn't free there, or with no `at` at the lowest free
    /// place. Returns where.
    pub fn reserve(&mut self, at: Option<u32>, len: u32, lo: u32, hi: u32) -> Option<u32> {
        let len = len.checked_add(GRANULARITY - 1)? & !(GRANULARITY - 1);
        self.reserve_region(at, len, lo, hi, 0, MEM_PRIVATE)
    }

    /// Reserve `len` bytes for a module's image, as `reserve` does.
    pub fn reserve_image(&mut self, at: Option<u32>, len: u32, lo: u32, hi: u32) -> Option<u32> {
        let len = len.checked_add(GRANULARITY - 1)? & !(GRANULARITY - 1);
        self.reserve_region(at, len, lo, hi, 0, MEM_IMAGE)
    }

    /// Give back the reservation that starts at `start` and the pages
    /// committed in it. Its length, if there was one.
    pub fn release_region(&mut self, bus: &mut Bus, start: u32) -> Option<u32> {
        let len = self.region(start).filter(|r| r.start == start)?.len;
        for page in (start..start + len).step_by(PAGE as usize) {
            self.unmap(bus, page);
        }
        self.unreserve(start)
    }

    /// Reserve `len` bytes (whole pages) as `reserve` does, the region
    /// starting at a multiple of 64 KB, with the protection and type
    /// VirtualQuery says it has.
    pub fn reserve_region(
        &mut self,
        at: Option<u32>,
        len: u32,
        lo: u32,
        hi: u32,
        protect: u32,
        kind: u32,
    ) -> Option<u32> {
        let len = len.checked_add(PAGE - 1)? & !(PAGE - 1);
        if len == 0 {
            return None;
        }
        let free = |start: u32| {
            let end = start as u64 + len as u64;
            start >= lo
                && end <= hi as u64
                && self
                    .regions
                    .iter()
                    .all(|r| end <= r.start as u64 || start as u64 >= r.end() as u64)
        };
        let start = match at {
            Some(at) if at % GRANULARITY == 0 && free(at) => at,
            Some(_) => return None,
            None => std::iter::once(lo)
                .chain(
                    self.regions
                        .iter()
                        .map(|r| (r.end() as u64).next_multiple_of(GRANULARITY as u64) as u32),
                )
                .filter(|&start| free(start))
                .min()?,
        };
        let i = self.regions.partition_point(|r| r.start < start);
        let region = Region {
            start,
            len,
            protect,
            kind,
        };
        self.regions.insert(i, region);
        Some(start)
    }

    /// Write `data` at `lin`, which is mapped.
    pub fn write(&self, bus: &mut Bus, lin: u32, data: &[u8]) -> Option<()> {
        let mut done = 0;
        while done < data.len() {
            let at = lin.wrapping_add(done as u32);
            let run = ((PAGE - (at & 0xFFF)) as usize).min(data.len() - done);
            let phys = self.translate(bus, at)? as usize;
            bus.write_run(phys, &data[done..done + run]);
            done += run;
        }
        Some(())
    }

    pub fn write_32(&self, bus: &mut Bus, lin: u32, value: u32) -> Option<()> {
        self.write(bus, lin, &value.to_le_bytes())
    }

    /// The double word at `lin` (a multiple of 4), if it is mapped.
    pub fn read_32(&self, bus: &Bus, lin: u32) -> Option<u32> {
        Some(bus.read_32(self.translate(bus, lin)? as usize))
    }
}

/// A zeroed physical page from the pool.
pub fn pool_page(bus: &mut Bus) -> Option<u32> {
    let pool = bus.win32.pool.as_mut()?;
    let page = match pool.free.pop() {
        Some(page) => page,
        None if pool.next + PAGE <= pool.len => {
            pool.next += PAGE;
            pool.base + pool.next - PAGE
        }
        None => return None,
    };
    bus.fill_ram(page as usize..(page + PAGE) as usize, 0);
    Some(page)
}

/// Give the physical page `phys` back to the pool.
pub fn free_page(bus: &mut Bus, phys: u32) {
    if let Some(pool) = bus.win32.pool.as_mut()
        && pool.has(phys)
    {
        pool.free.push(phys);
    }
}

crate::state_fields!(Pool {
    base,
    len,
    next,
    free
});
crate::state_fields!(Region {
    start,
    len,
    protect,
    kind
});
crate::state_fields!(Memory { cr3, regions });
