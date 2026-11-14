//! A Win32 process's memory: physical pages from the pool all processes
//! share (`Pool`), a block of extended memory taken from the XMS pool when
//! the first process starts, and the page tables that put them at the
//! linear addresses Windows 95 has its parts at. The first megabyte (DOS,
//! the BIOS and video memory) is there as it is in physical memory, for
//! the system only.
//!
//! The memory a program commits for itself (VirtualAlloc, heaps) is
//! backed by the page file (`PageFile`), as on Windows 98: a page
//! committed there takes no physical page until it is first touched,
//! when a page fault gives it a zeroed one (`page_in`). When the pool
//! has no page left, one of those pages that wasn't touched lately goes
//! out to the page file (`page_out`), and comes back on its next touch.
//! So a program can commit as much as the pool and the page file hold
//! together, and touch it all, which is what GlobalMemoryStatus reports.
//! The system's own pages (images, stacks, page tables, file mappings)
//! stay in the pool.

use std::collections::BTreeMap;
use std::ops::Bound;

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
const PROTECTION_MASK: u32 = 7 << PROTECTION_SHIFT;

/// The entry of a committed page of a program's own memory that has no
/// physical page now: not present, with this bit (which the processor
/// doesn't look at in an entry that isn't present) and the page's
/// protection. Its upper 20 bits are its place in the page file plus 1,
/// or 0 for a page that is all zeros and was never written out.
pub const PAGED: u32 = 0x080;

/// Page table entry bit the processor sets when it uses the page.
const ACCESSED: u32 = 0x020;

/// Whether the page table entry `entry` is of a committed page in the
/// page file (`PAGED`).
pub fn is_paged(entry: u32) -> bool {
    entry & (PRESENT | PAGED) == PAGED
}

/// The entry of a page in the page file at `slot` (0 for a page of
/// zeros) with the protection `protect`.
fn paged_entry(slot: u32, protect: u32) -> u32 {
    slot << 12 | PAGED | protection_bits(protect) & PROTECTION_MASK
}

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
    Some(if is_paged(entry) {
        tagged
    } else if entry & PRESENT == 0 {
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
    /// The page file behind the programs' own memory.
    pub page_file: PageFile,
}

/// The page file: where the committed pages of programs' own memory that
/// have no physical page are (`PAGED`). Windows 98 lets its swap file
/// grow as far as the disk allows by default; this one may hold twice as
/// many pages as the machine has, what GlobalMemoryStatus has always
/// reported as its size.
#[derive(Clone, Debug, Default)]
pub struct PageFile {
    /// The pages it may hold.
    pub size: u32,
    /// The committed pages it holds: those of zeros and those written to
    /// it.
    pub used: u32,
    /// The contents of the pages written to it, by place; an empty one is
    /// a free place, which `free_slots` lists.
    slots: Vec<Vec<u8>>,
    free_slots: Vec<u32>,
    /// The physical pages of programs' own memory, which can go out to
    /// it, with the address of the page table entry that maps each.
    pageable: BTreeMap<u32, u32>,
    /// Pages of programs' own memory that the system maps a second time
    /// (`pin`), which stay in the pool until the last of those mappings
    /// goes (`unpin`): how many there are and the page table entry
    /// address `pageable` had for each.
    pinned: BTreeMap<u32, (u32, u32)>,
    /// The page the last search for one to write out stopped at.
    hand: u32,
}

/// How many pages the page file gave and took since the emulator started:
/// the pages of zeros that got a physical page (on their first touch, or
/// sent out holding zeros), the pages read back from it, and the pages
/// written out to it.
#[derive(Clone, Copy, Debug, Default)]
pub struct PagingCounts {
    pub zero_ins: u64,
    pub ins: u64,
    pub outs: u64,
}

/// The page file now, for the debugger's status.
#[derive(Clone, Copy, Debug)]
pub struct PageFileStatus {
    /// The pages it may hold and those it holds.
    pub size: u32,
    pub used: u32,
    /// The physical pages of programs' own memory that may go out to it.
    pub pageable: usize,
    pub counts: PagingCounts,
}

/// The page file's state and counts, if the built-in Windows runs.
pub fn page_file_status(bus: &Bus) -> Option<PageFileStatus> {
    let page_file = &bus.win32.pool.as_ref()?.page_file;
    Some(PageFileStatus {
        size: page_file.size,
        used: page_file.used,
        pageable: page_file.pageable.len(),
        counts: bus.win32.paging,
    })
}

impl PageFile {
    /// The pages it has room for.
    pub fn room(&self) -> u32 {
        self.size.saturating_sub(self.used)
    }

    /// Keep `data`, a page's contents: its place plus 1, as an entry has
    /// it.
    fn store(&mut self, data: Vec<u8>) -> u32 {
        match self.free_slots.pop() {
            Some(slot) => {
                self.slots[slot as usize] = data;
                slot + 1
            }
            None => {
                self.slots.push(data);
                self.slots.len() as u32
            }
        }
    }

    /// The contents at `slot` (an entry's, 0 for zeros).
    fn contents(&self, slot: u32) -> Option<&[u8]> {
        slot.checked_sub(1)
            .and_then(|s| self.slots.get(s as usize))
            .map(Vec::as_slice)
    }

    /// Give the place `slot` (an entry's) back, with its contents.
    fn take(&mut self, slot: u32) -> Option<Vec<u8>> {
        let s = slot.checked_sub(1)?;
        let data = std::mem::take(self.slots.get_mut(s as usize)?);
        self.free_slots.push(s);
        Some(data)
    }

    /// A committed page that was in it is gone, or has a physical page
    /// now: the entry it had was `entry`.
    fn release(&mut self, entry: u32) {
        self.take(entry >> 12);
        self.used = self.used.saturating_sub(1);
    }
}

impl Pool {
    /// Take the largest free block of extended memory, if it is at least
    /// `min` bytes, with a page file twice the size of the machine's
    /// memory.
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
            page_file: PageFile {
                size: end / PAGE * 2,
                ..PageFile::default()
            },
            ..Pool::default()
        })
    }

    /// Give the physical page `phys` back.
    fn give(&mut self, phys: u32) {
        self.page_file.pageable.remove(&phys);
        self.page_file.pinned.remove(&phys);
        self.free.push(phys);
    }

    /// The pages a program can commit: those left in the pool and the
    /// page file's room.
    pub fn commit_room(&self) -> u32 {
        self.available() / PAGE + self.page_file.room()
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

    /// The page tables at `cr3` alone, without the reserved ranges: for
    /// reading and writing another process's memory (`read`, `write`).
    pub fn tables(cr3: u32) -> Memory {
        Memory {
            cr3,
            ..Memory::default()
        }
    }

    /// Give every page it has back to the pool: the pages mapped, the
    /// page tables and the directory.
    pub fn release(&self, bus: &mut Bus) {
        let Some(pool) = bus.win32.pool.as_ref() else {
            return;
        };
        let (base, len) = (pool.base, pool.len);
        let in_pool = |phys: u32| (base..base + len).contains(&phys);
        let mut pages = Vec::new();
        let mut paged = Vec::new();
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
                if is_paged(entry) {
                    paged.push(entry);
                } else if entry != 0 && in_pool(entry & !0xFFF) && !bus.win32.shared(entry & !0xFFF)
                {
                    pages.push(entry & !0xFFF);
                }
            }
            pages.push(table);
        }
        pages.push(self.cr3);
        if let Some(pool) = bus.win32.pool.as_mut() {
            for phys in pages {
                pool.give(phys);
            }
            for entry in paged {
                pool.page_file.release(entry);
            }
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
    /// back to the pool unless a file mapping has it, or to the page file.
    /// Its physical address, if one was there.
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
        if is_paged(entry) {
            if let Some(pool) = bus.win32.pool.as_mut() {
                pool.page_file.release(entry);
            }
            return None;
        }
        let phys = entry & !0xFFF;
        let shared = bus.win32.shared(phys);
        if let Some(pool) = bus.win32.pool.as_mut()
            && pool.has(phys)
            && !shared
        {
            pool.give(phys);
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
    /// value) for the program's own memory: a zeroed page from the pool,
    /// which can go out to the page file (`page_out`) as a page brought
    /// in from there can. Otherwise a pool full of such pages would leave
    /// none to give a page in the page file when it is touched.
    pub fn commit_page(&mut self, bus: &mut Bus, lin: u32, protect: u32) -> Option<()> {
        let phys = self.page(bus)?;
        if self
            .set_entry(bus, lin, phys | protection_bits(protect))
            .is_none()
        {
            free_page(bus, phys);
            return None;
        }
        if let Some(at) = self.entry_at(bus, lin)
            && let Some(pool) = bus.win32.pool.as_mut()
        {
            pool.page_file.pageable.insert(phys, at as u32);
        }
        Some(())
    }

    /// Commit the `len` bytes at `lin` (whole pages, none of them
    /// committed) for the program's own memory, with the protection
    /// `protect`: in the page file, where they take no physical page
    /// until they are touched, or while it is full, in the pool. None,
    /// with none of them committed, when the two together haven't room.
    pub fn commit_paged(&mut self, bus: &mut Bus, lin: u32, len: u32, protect: u32) -> Option<()> {
        let pages = len / PAGE;
        if bus.win32.pool.as_ref()?.commit_room() < pages {
            return None;
        }
        for page in (lin..lin + len).step_by(PAGE as usize) {
            let pool = bus.win32.pool.as_mut()?;
            let done = if pool.page_file.room() > 0 {
                pool.page_file.used += 1;
                let done = self.set_entry(bus, page, paged_entry(0, protect));
                if done.is_none()
                    && let Some(pool) = bus.win32.pool.as_mut()
                {
                    pool.page_file.used -= 1;
                }
                done
            } else {
                self.commit_page(bus, page, protect)
            };
            if done.is_none() {
                for done in (lin..page).step_by(PAGE as usize) {
                    self.unmap(bus, done);
                }
                return None;
            }
        }
        Some(())
    }

    /// Give the committed page at `lin` the protection `protect`, its
    /// physical page (or place in the page file) and accessed and dirty
    /// bits kept. False if it isn't committed.
    pub fn set_protection(&mut self, bus: &mut Bus, lin: u32, protect: u32) -> bool {
        match self.entry(bus, lin) {
            Some(entry) if is_paged(entry) => {
                let kept = entry & !PROTECTION_MASK;
                let tag = protection_bits(protect) & PROTECTION_MASK;
                self.set_entry(bus, lin, kept | tag).is_some()
            }
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
        self.entry_at(bus, lin).map(|at| bus.read_32(at))
    }

    /// The physical address of the page table entry of `lin`, if its page
    /// table is there.
    pub fn entry_at(&self, bus: &Bus, lin: u32) -> Option<usize> {
        let table = bus.read_32((self.cr3 + 4 * (lin >> 22)) as usize);
        (table & PRESENT != 0).then(|| ((table & !0xFFF) + 4 * ((lin >> 12) & 0x3FF)) as usize)
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

    /// Back `len` bytes at `lin` (page aligned) with zeroed pages. When
    /// there aren't pages for all of them, the pages it took go back to
    /// the pool and none of the range is backed.
    pub fn commit(&mut self, bus: &mut Bus, lin: u32, len: u32, flags: u32) -> Option<()> {
        for page in (lin..lin + len).step_by(PAGE as usize) {
            let mapped = self.page(bus).is_some_and(|phys| {
                let mapped = self.map(bus, page, phys, flags).is_some();
                if !mapped {
                    free_page(bus, phys);
                }
                mapped
            });
            if !mapped {
                for done in (lin..page).step_by(PAGE as usize) {
                    self.unmap(bus, done);
                }
                return None;
            }
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

    /// Where the bytes of the page at `lin` are now, if the program can
    /// reach them or they are in the page file.
    fn place(&self, bus: &Bus, lin: u32) -> Option<Place> {
        let at = self.entry_at(bus, lin)?;
        let entry = bus.read_32(at);
        if entry & PRESENT != 0 {
            Some(Place::Ram(entry & !0xFFF))
        } else if is_paged(entry) {
            Some(Place::Paged(at, entry))
        } else {
            None
        }
    }

    /// Fill `data` from `lin`, without a page fault: the bytes of pages in
    /// the page file are read from it. None where a page isn't there.
    pub fn read(&self, bus: &Bus, lin: u32, data: &mut [u8]) -> Option<()> {
        for (offset, run) in Bus::page_runs(lin, data.len()) {
            let at = lin.wrapping_add(offset as u32);
            let part = &mut data[offset..offset + run];
            match self.place(bus, at)? {
                Place::Ram(phys) => bus.read_into((phys | at & 0xFFF) as usize, part),
                Place::Paged(_, entry) => {
                    let page_file = &bus.win32.pool.as_ref()?.page_file;
                    match page_file.contents(entry >> 12) {
                        Some(page) => {
                            let from = (at & 0xFFF) as usize;
                            part.copy_from_slice(&page[from..from + run]);
                        }
                        None => part.fill(0),
                    }
                }
            }
        }
        Some(())
    }

    /// Write `data` at `lin`, which is mapped or in the page file, whose
    /// copy is written there: no page fault, and no physical page taken.
    pub fn write(&self, bus: &mut Bus, lin: u32, data: &[u8]) -> Option<()> {
        for (offset, run) in Bus::page_runs(lin, data.len()) {
            let at = lin.wrapping_add(offset as u32);
            let part = &data[offset..offset + run];
            match self.place(bus, at)? {
                Place::Ram(phys) => bus.write_run((phys | at & 0xFFF) as usize, part),
                Place::Paged(pte, entry) => {
                    let page_file = &mut bus.win32.pool.as_mut()?.page_file;
                    let mut slot = entry >> 12;
                    if slot == 0 {
                        slot = page_file.store(vec![0; PAGE as usize]);
                        bus.write_32(pte, entry & 0xFFF | slot << 12);
                    }
                    let page_file = &mut bus.win32.pool.as_mut()?.page_file;
                    let page = &mut page_file.slots[slot as usize - 1];
                    let from = (at & 0xFFF) as usize;
                    page[from..from + run].copy_from_slice(part);
                }
            }
        }
        Some(())
    }

    pub fn write_32(&self, bus: &mut Bus, lin: u32, value: u32) -> Option<()> {
        self.write(bus, lin, &value.to_le_bytes())
    }

    /// The double word at `lin`, if it is mapped or in the page file.
    pub fn read_32(&self, bus: &Bus, lin: u32) -> Option<u32> {
        let mut bytes = [0; 4];
        self.read(bus, lin, &mut bytes)?;
        Some(u32::from_le_bytes(bytes))
    }

    /// Set the `len` bytes at `lin` to zero, as `write` does, except that
    /// a page in the page file that is all zeros stays as it is, and one
    /// zeroed whole becomes such a page again: a block of a heap zeroed
    /// for the program takes no physical page until it is touched.
    pub fn zero(&self, bus: &mut Bus, lin: u32, len: u32) {
        for (offset, run) in Bus::page_runs(lin, len as usize) {
            let at = lin.wrapping_add(offset as u32);
            match self.place(bus, at) {
                Some(Place::Ram(phys)) => {
                    let from = (phys | at & 0xFFF) as usize;
                    bus.fill_ram(from..from + run, 0);
                }
                Some(Place::Paged(_, entry)) if entry >> 12 == 0 => {}
                Some(Place::Paged(pte, entry)) if run == PAGE as usize => {
                    if let Some(pool) = bus.win32.pool.as_mut() {
                        pool.page_file.take(entry >> 12);
                        bus.write_32(pte, entry & 0xFFF);
                    }
                }
                Some(Place::Paged(..)) => {
                    self.write(bus, at, &[0; PAGE as usize][..run]);
                }
                None => {}
            }
        }
    }

    /// Whether the program may reach the page at `lin`, as a touch would
    /// find it: it is mapped, or in the page file with a protection that
    /// lets the program touch it, so the touch brings it in.
    pub fn reachable(&self, bus: &Bus, lin: u32) -> bool {
        let entry = self.entry(bus, lin).unwrap_or(0);
        entry & PRESENT != 0 || is_paged(entry) && protection(entry) != Some(PAGE_NOACCESS)
    }
}

/// Where a page's bytes are: in its physical page, or in the page file
/// (the address of its entry, and the entry).
enum Place {
    Ram(u32),
    Paged(usize, u32),
}

/// A zeroed physical page from the pool. When it has none left, a page
/// of a program's own memory goes out to the page file to make one.
pub fn pool_page(bus: &mut Bus) -> Option<u32> {
    let page = match take_page(bus) {
        Some(page) => page,
        None if page_out(bus) => take_page(bus)?,
        None => return None,
    };
    bus.fill_ram(page as usize..(page + PAGE) as usize, 0);
    Some(page)
}

/// A physical page from the pool, as it is.
fn take_page(bus: &mut Bus) -> Option<u32> {
    let pool = bus.win32.pool.as_mut()?;
    match pool.free.pop() {
        Some(page) => Some(page),
        None if pool.next + PAGE <= pool.len => {
            pool.next += PAGE;
            Some(pool.base + pool.next - PAGE)
        }
        None => None,
    }
}

/// Give the physical page `phys` back to the pool.
pub fn free_page(bus: &mut Bus, phys: u32) {
    if let Some(pool) = bus.win32.pool.as_mut()
        && pool.has(phys)
    {
        pool.give(phys);
    }
}

/// Write a page of a program's own memory out to the page file, to give
/// its physical page back to the pool: the first, from where the last
/// search stopped, that no program has touched since the search before
/// (its entry's accessed bit, which the search clears). False if the
/// page file is full or there is no such page. The TLB is stale after
/// it (`Win32::tlb_stale`).
pub fn page_out(bus: &mut Bus) -> bool {
    let Some(pool) = bus.win32.pool.as_ref() else {
        return false;
    };
    if pool.page_file.room() == 0 {
        return false;
    }
    // Two rounds: the first may only clear the accessed bits.
    let mut left = 2 * pool.page_file.pageable.len();
    let mut hand = pool.page_file.hand;
    let victim = loop {
        if left == 0 {
            return false;
        }
        left -= 1;
        let pageable = &bus.win32.pool.as_ref().expect("checked").page_file.pageable;
        let next = pageable
            .range((Bound::Excluded(hand), Bound::Unbounded))
            .next()
            .or_else(|| pageable.iter().next());
        let Some((&phys, &at)) = next else {
            return false;
        };
        hand = phys;
        let entry = bus.read_32(at as usize);
        // A page made PAGE_NOACCESS keeps its physical page in an entry
        // that isn't present, and goes out as any other, with that
        // protection: VirtualProtect may give it another later.
        if entry == 0 || is_paged(entry) || entry & !0xFFF != phys {
            // Its entry has changed: not the page's any more.
            let pool = bus.win32.pool.as_mut().expect("checked");
            pool.page_file.pageable.remove(&phys);
            continue;
        }
        if entry & ACCESSED != 0 {
            bus.write_32(at as usize, entry & !ACCESSED);
            continue;
        }
        break (phys, at, entry);
    };
    let (phys, at, entry) = victim;
    let mut data = vec![0; PAGE as usize];
    bus.read_into(phys as usize, &mut data);
    let protect = protection(entry).unwrap_or(PAGE_READWRITE);
    let pool = bus.win32.pool.as_mut().expect("checked");
    pool.page_file.hand = hand;
    let slot = if data.iter().all(|&b| b == 0) {
        0
    } else {
        pool.page_file.store(data)
    };
    pool.page_file.used += 1;
    pool.give(phys);
    bus.write_32(at as usize, paged_entry(slot, protect));
    bus.win32.tlb_stale = true;
    bus.win32.paging.outs += 1;
    true
}

/// What `page_in` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageIn {
    /// The page is there now.
    Done,
    /// The page is in the page file, but there is no physical page for
    /// it.
    Full,
    /// It isn't a page in the page file that the program may touch.
    No,
}

/// Give the page at `lin` of the page tables at `cr3` its physical page
/// if it is in the page file and its protection lets the program touch
/// it: a zeroed one, or its contents read back. A page goes out to make
/// room when the pool has none left (`page_out`), and the TLB is stale
/// then (`Win32::tlb_stale`).
pub fn page_in(bus: &mut Bus, cr3: u32, lin: u32) -> PageIn {
    if cr3 == 0 || bus.win32.pool.is_none() {
        return PageIn::No;
    }
    let table = bus.read_32((cr3 + 4 * (lin >> 22)) as usize);
    if table & PRESENT == 0 {
        return PageIn::No;
    }
    let at = ((table & !0xFFF) + 4 * ((lin >> 12) & 0x3FF)) as usize;
    let entry = bus.read_32(at);
    let protect = match protection(entry) {
        Some(protect) if is_paged(entry) && protect != PAGE_NOACCESS => protect,
        _ => return PageIn::No,
    };
    // The page leaves the page file first, which makes room there for
    // the page that goes out for it when the pool is empty.
    let pool = bus.win32.pool.as_mut().expect("checked");
    let data = pool.page_file.take(entry >> 12);
    pool.page_file.used -= 1;
    let Some(phys) = pool_page(bus) else {
        let pool = bus.win32.pool.as_mut().expect("checked");
        let slot = data.map_or(0, |data| pool.page_file.store(data));
        pool.page_file.used += 1;
        bus.write_32(at, entry & 0xFFF | slot << 12);
        return PageIn::Full;
    };
    if let Some(data) = data {
        bus.load_bytes(phys as usize, &data);
        bus.win32.paging.ins += 1;
    } else {
        bus.win32.paging.zero_ins += 1;
    }
    bus.write_32(at, phys | protection_bits(protect));
    let pool = bus.win32.pool.as_mut().expect("checked");
    pool.page_file.pageable.insert(phys, at as u32);
    PageIn::Done
}

/// The physical page of the present page at `lin` of the page tables at
/// `cr3`.
fn present_page(bus: &Bus, cr3: u32, lin: u32) -> Option<u32> {
    let table = bus.read_32((cr3 + 4 * (lin >> 22)) as usize);
    if table & PRESENT == 0 {
        return None;
    }
    let entry = bus.read_32(((table & !0xFFF) + 4 * ((lin >> 12) & 0x3FF)) as usize);
    (entry & PRESENT != 0).then_some(entry & !0xFFF)
}

/// Keep the page at `lin` of the page tables at `cr3` in the pool until
/// as many `unpin`s as `pin`s have been made: give it its physical page
/// if it is in the page file, and send it there no more meanwhile. For a
/// page the system maps at a second place, whose entry there `page_out`
/// wouldn't change.
pub fn pin(bus: &mut Bus, cr3: u32, lin: u32) {
    page_in(bus, cr3, lin);
    let Some(phys) = present_page(bus, cr3, lin) else {
        return;
    };
    let Some(file) = bus.win32.pool.as_mut().map(|p| &mut p.page_file) else {
        return;
    };
    if let Some(at) = file.pageable.remove(&phys) {
        file.pinned.insert(phys, (1, at));
    } else if let Some((count, _)) = file.pinned.get_mut(&phys) {
        *count += 1;
    }
}

/// One of the second mappings `pin` was made for has gone: the page at
/// `lin` of the page tables at `cr3` can go out to the page file again
/// once none is left.
pub fn unpin(bus: &mut Bus, cr3: u32, lin: u32) {
    let Some(phys) = present_page(bus, cr3, lin) else {
        return;
    };
    let Some(file) = bus.win32.pool.as_mut().map(|p| &mut p.page_file) else {
        return;
    };
    let Some((count, at)) = file.pinned.get_mut(&phys) else {
        return;
    };
    *count -= 1;
    if *count == 0 {
        let at = *at;
        file.pinned.remove(&phys);
        file.pageable.insert(phys, at);
    }
}

/// Write `data` at `lin` in the memory of the process `pid`, through its
/// own page tables, whichever process runs and whether or not a service
/// is running: for work the system finishes later, outside the call that
/// asked for it. Bytes on pages the process hasn't mapped are left out.
pub fn write_process_bytes(bus: &mut Bus, pid: u32, lin: u32, data: &[u8]) {
    let Some(p) = bus.win32.processes.iter().position(|p| p.id == pid) else {
        return;
    };
    let memory = Memory::tables(bus.win32.processes[p].memory.cr3);
    for (offset, run) in Bus::page_runs(lin, data.len()) {
        let at = lin.wrapping_add(offset as u32);
        let _ = memory.write(bus, at, &data[offset..offset + run]);
    }
}

crate::state_fields!(Pool {
    base,
    len,
    next,
    free,
    page_file
});
crate::state_fields!(PageFile {
    size,
    used,
    slots,
    free_slots,
    pageable,
    pinned,
    hand
});
crate::state_fields!(Region {
    start,
    len,
    protect,
    kind
});
crate::state_fields!(Memory { cr3, regions });

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::Cpu;
    use crate::cpu::paging::GuestPaging;

    /// A process's page tables on a machine whose pool has `pages` pages
    /// left, with a page file of `file` pages.
    fn machine(pages: u32, file: u32) -> (Cpu, Memory) {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.win32.pool = Pool::take(&mut cpu.bus, 0x40_0000);
        let memory = Memory::new(&mut cpu.bus).expect("a page directory");
        let pool = cpu.bus.win32.pool.as_mut().expect("a pool");
        pool.len = pool.next + pages * PAGE;
        pool.page_file.size = file;
        (cpu, memory)
    }

    fn file(cpu: &Cpu) -> &PageFile {
        &cpu.bus.win32.pool.as_ref().expect("a pool").page_file
    }

    /// The `n`th page of the program's memory.
    fn page(n: u32) -> u32 {
        PRIVATE + n * PAGE
    }

    /// Touch the page at `lin` as the program does: bring it in, as the
    /// page fault does, and fill it with `byte` where it is now.
    fn touch(cpu: &mut Cpu, memory: &Memory, lin: u32, byte: u8) {
        if memory.translate(&cpu.bus, lin).is_none() {
            assert_eq!(page_in(&mut cpu.bus, memory.cr3, lin), PageIn::Done);
        }
        let phys = memory.translate(&cpu.bus, lin).expect("present") as usize;
        cpu.bus.fill_ram(phys..phys + PAGE as usize, byte);
    }

    /// The `len` bytes at `lin`, wherever they are.
    fn bytes(cpu: &Cpu, memory: &Memory, lin: u32, len: usize) -> Vec<u8> {
        let mut data = vec![0xEE; len];
        memory.read(&cpu.bus, lin, &mut data).expect("committed");
        data
    }

    /// More pages touched than the pool has: some go out to the page
    /// file, and every page reads back what was written to it, whether it
    /// is in the pool or the page file, and again after each has come
    /// back in.
    #[test]
    fn pages_sent_to_the_page_file_keep_their_bytes() {
        let (mut cpu, mut memory) = machine(4, 32);
        memory
            .commit_paged(&mut cpu.bus, page(0), 8 * PAGE, PAGE_READWRITE)
            .expect("room in the page file");
        for n in 0..8 {
            touch(&mut cpu, &memory, page(n), n as u8 + 1);
        }
        let out = (0..8)
            .filter(|&n| memory.translate(&cpu.bus, page(n)).is_none())
            .count();
        assert!(out >= 5, "only {out} pages went out");
        assert!(cpu.bus.win32.tlb_stale, "pages went out");
        for n in 0..8 {
            let all = bytes(&cpu, &memory, page(n), PAGE as usize);
            assert!(all.iter().all(|&b| b == n as u8 + 1), "page {n}");
        }
        for n in (0..8).rev() {
            if memory.translate(&cpu.bus, page(n)).is_none() {
                assert_eq!(page_in(&mut cpu.bus, memory.cr3, page(n)), PageIn::Done);
            }
            let phys = memory.translate(&cpu.bus, page(n)).expect("in") as usize;
            assert_eq!(cpu.bus.read_8(phys + 0x123), n as u8 + 1, "page {n}");
        }
    }

    /// The system's write across a page in the pool and one in the page
    /// file that was never touched: the first part goes to the physical
    /// page, the second to the page file, which takes no physical page
    /// for it, and a read across both gives the bytes back.
    #[test]
    fn a_write_across_a_page_in_the_pool_and_one_in_the_page_file() {
        let (mut cpu, mut memory) = machine(8, 32);
        memory
            .commit_paged(&mut cpu.bus, page(0), 2 * PAGE, PAGE_READWRITE)
            .expect("room");
        touch(&mut cpu, &memory, page(0), 0x11);
        let available = cpu.bus.win32.pool.as_ref().unwrap().available();
        let data = [1, 2, 3, 4, 5, 6, 7, 8];
        let at = page(1) - 4;
        memory.write(&mut cpu.bus, at, &data).expect("committed");
        assert_eq!(cpu.bus.win32.pool.as_ref().unwrap().available(), available);
        let entry = memory.entry(&cpu.bus, page(1)).expect("an entry");
        assert!(is_paged(entry) && entry >> 12 != 0, "{entry:08X}");
        assert_eq!(bytes(&cpu, &memory, at, 8), data);
        assert_eq!(bytes(&cpu, &memory, page(1) + 4, 4), [0; 4]);
        assert_eq!(page_in(&mut cpu.bus, memory.cr3, page(1)), PageIn::Done);
        let phys = memory.translate(&cpu.bus, page(1)).expect("in") as usize;
        assert_eq!(cpu.bus.read_32(phys), 0x0807_0605);
        assert_eq!(file(&cpu).free_slots.len(), file(&cpu).slots.len());
    }

    /// A service's write through the page tables of more pages than the
    /// pool has, each brought in on the way (`Bus::guest_write_bytes`):
    /// the pages that go out to make room for the next keep what was
    /// written to them.
    #[test]
    fn a_service_writes_more_pages_than_the_pool_has() {
        let (mut cpu, mut memory) = machine(3, 32);
        memory
            .commit_paged(&mut cpu.bus, page(0), 6 * PAGE, PAGE_READWRITE)
            .expect("room");
        let data: Vec<u8> = (0..6 * PAGE - 10).map(|i| (i / PAGE + 1) as u8).collect();
        // The built-in Windows runs with the A20 gate open.
        cpu.bus.set_a20(true);
        cpu.bus.guest_paging = Some(GuestPaging {
            cr3: memory.cr3,
            user: true,
            write_protect: false,
            pse: false,
        });
        cpu.bus.guest_write_bytes(page(0) + 5, &data);
        assert!(!cpu.bus.guest_faulted());
        cpu.bus.guest_paging = None;
        assert_eq!(bytes(&cpu, &memory, page(0) + 5, data.len()), data);
    }

    /// Pages decommitted, or of a process that ended, give their places in
    /// the page file back, and their physical pages to the pool.
    #[test]
    fn decommitted_pages_free_their_places_in_the_page_file() {
        for end in [false, true] {
            let (mut cpu, mut memory) = machine(3, 32);
            memory
                .commit_paged(&mut cpu.bus, page(0), 6 * PAGE, PAGE_READWRITE)
                .expect("room");
            for n in 0..6 {
                touch(&mut cpu, &memory, page(n), 0x40 + n as u8);
            }
            assert!(file(&cpu).used > 0);
            if end {
                memory.release(&mut cpu.bus);
            } else {
                for n in 0..6 {
                    memory.unmap(&mut cpu.bus, page(n));
                }
            }
            let f = file(&cpu);
            assert_eq!(f.used, 0, "process ended: {end}");
            assert_eq!(f.free_slots.len(), f.slots.len(), "process ended: {end}");
            assert!(f.pageable.is_empty(), "process ended: {end}");
        }
    }

    /// A page pinned twice stays in the pool while pages go out around
    /// it, and can go out again only after both pins are undone.
    #[test]
    fn a_pinned_page_stays_until_its_last_pin_goes() {
        let (mut cpu, mut memory) = machine(3, 32);
        memory
            .commit_paged(&mut cpu.bus, page(0), 5 * PAGE, PAGE_READWRITE)
            .expect("room");
        touch(&mut cpu, &memory, page(0), 1);
        pin(&mut cpu.bus, memory.cr3, page(0));
        pin(&mut cpu.bus, memory.cr3, page(0));
        let phys = memory.translate(&cpu.bus, page(0)).expect("in");
        for n in 1..5 {
            touch(&mut cpu, &memory, page(n), 2);
        }
        assert_eq!(memory.translate(&cpu.bus, page(0)), Some(phys));
        unpin(&mut cpu.bus, memory.cr3, page(0));
        assert!(!file(&cpu).pageable.contains_key(&(phys & !0xFFF)));
        unpin(&mut cpu.bus, memory.cr3, page(0));
        assert!(file(&cpu).pageable.contains_key(&(phys & !0xFFF)));
        assert!(file(&cpu).pinned.is_empty());
        for n in 1..5 {
            touch(&mut cpu, &memory, page(n), 3);
        }
        assert_eq!(memory.translate(&cpu.bus, page(0)), None, "went out");
        assert_eq!(bytes(&cpu, &memory, page(0), 4), [1; 4]);
    }
}
