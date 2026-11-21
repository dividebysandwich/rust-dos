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
//! The pages of a module's image go out and come back the same way
//! (`map_pageable`). The system's own pages (stacks, page tables)
//! stay in the pool; those of views of files leave it as
//! `mapping::evict` says.

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

/// Page table entry bits the processor sets when it uses the page, and
/// when it writes it.
const ACCESSED: u32 = 0x020;
const DIRTY: u32 = 0x040;

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
    /// The physical pages of programs' own memory and of modules' images,
    /// which can go out to it, with the address of the page table entry
    /// that maps each.
    pageable: BTreeMap<u32, u32>,
    /// Pages of programs' own memory that the system maps a second time
    /// (`pin`), which stay in the pool until the last of those mappings
    /// goes (`unpin`): how many there are and the page table entry
    /// address `pageable` had for each.
    pinned: BTreeMap<u32, (u32, u32)>,
    /// The page the last search for one to write out stopped at.
    hand: u32,
    /// The linear address of the last page of a view of a file
    /// `mapping::evict` gave back: its next search starts after it.
    pub(crate) view_hand: u32,
    /// The context of the instruction whose page faults the pages in
    /// `held` came in for (`retry`), as a hash.
    retry_key: u64,
    /// The pages that instruction brought in, which stay in the pool
    /// until a page fault of another context (`hold`): physical page and
    /// the address of the page table entry that maps it.
    held: Vec<(u32, u32)>,
}

/// How many pages the page file gave and took since the emulator started:
/// the pages of zeros that got a physical page (on their first touch, or
/// sent out holding zeros), the pages read back from it, and the pages
/// written out to it. And the pages of views of files read from their
/// files, those given back to the pool (`mapping::evict`), and the
/// searches for those (its calls).
#[derive(Clone, Copy, Debug, Default)]
pub struct PagingCounts {
    pub zero_ins: u64,
    pub ins: u64,
    pub outs: u64,
    pub reads: u64,
    pub evicted: u64,
    pub sweeps: u64,
}

/// The page file now, for the debugger's status.
#[derive(Clone, Copy, Debug)]
pub struct PageFileStatus {
    /// The pages it may hold and those it holds.
    pub size: u32,
    pub used: u32,
    /// The physical pages of programs' own memory and of modules' images
    /// that may go out to it.
    pub pageable: usize,
    pub counts: PagingCounts,
}

/// The paging counts since the emulator started, kept after the
/// built-in Windows has given its memory back.
pub fn paging_counts(bus: &Bus) -> PagingCounts {
    bus.win32.paging
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
    /// How many physical pages of programs' own memory and of modules'
    /// images may go out to it.
    pub fn pageable_len(&self) -> usize {
        self.pageable.len()
    }

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

    /// Take in a page that no page table entry keeps the place of (a
    /// written page of a copy-on-write view of a file, `mapping::evict`):
    /// its place, 0 for a page of zeros, or None when it is full.
    pub(crate) fn keep(&mut self, data: Vec<u8>) -> Option<u32> {
        if self.room() == 0 {
            return None;
        }
        self.used += 1;
        Some(if data.iter().all(|&b| b == 0) {
            0
        } else {
            self.store(data)
        })
    }

    /// Give back the page `keep` took in at `slot`: its contents, None
    /// for a page of zeros.
    pub(crate) fn give_back(&mut self, slot: u32) -> Option<Vec<u8>> {
        self.used = self.used.saturating_sub(1);
        self.take(slot)
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

    /// Lend the pages of the block never used yet back to the XMS pool,
    /// for a DOS program the system runs while the Windows programs wait
    /// (dosvm.rs): on Windows 98 the VMM gives a DOS machine its DPMI and
    /// XMS memory from the memory the system has free. The bytes lent,
    /// which `take_back` takes back.
    pub fn lend(bus: &mut Bus) -> u32 {
        let end = bus.ram().len() as u32;
        let Bus { win32, xms, .. } = bus;
        let Some(pool) = win32.pool.as_mut() else {
            return 0;
        };
        // The block keeps a page, so that it stays one.
        let keep = pool.next.max(PAGE);
        if pool.len <= keep {
            return 0;
        }
        xms.release_dpmi(pool.base);
        let kept = xms.take_dpmi_at(pool.base, keep, end);
        debug_assert!(kept, "the pool's own memory was free");
        let lent = pool.len - keep;
        pool.len = keep;
        lent
    }

    /// Take back the `bytes` that `lend` lent, if all of it is free
    /// again: false if a program kept some of it, and the block stays as
    /// it is.
    pub fn take_back(bus: &mut Bus, bytes: u32) -> bool {
        let end = bus.ram().len() as u32;
        let Bus { win32, xms, .. } = bus;
        let Some(pool) = win32.pool.as_mut() else {
            return bytes == 0;
        };
        if bytes == 0 {
            return true;
        }
        xms.release_dpmi(pool.base);
        if xms.take_dpmi_at(pool.base, pool.len + bytes, end) {
            pool.len += bytes;
            return true;
        }
        let kept = xms.take_dpmi_at(pool.base, pool.len, end);
        debug_assert!(kept, "the pool's own memory was free");
        false
    }

    /// Give the physical page `phys` back.
    fn give(&mut self, phys: u32) {
        self.page_file.pageable.remove(&phys);
        self.page_file.pinned.remove(&phys);
        self.page_file.held.retain(|&(held, _)| held != phys);
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
        self.make_pageable(bus, lin, phys);
        Some(())
    }

    /// Map the physical page `phys`, which `page` gave, at `lin` with the
    /// entry bits `flags`, as a page of a module's image: it can go out
    /// to the page file as the program's own memory does, with its
    /// protection. Windows 95 pages images out too, so a program can load
    /// images larger than the memory left beside them, as a game's DLL of
    /// 10 MB of bitmaps on a machine of 16 MB; a page that stayed in the
    /// pool would leave none for the program's own pages to come back
    /// in. Windows reads a page of code or read-only data from the image
    /// file again, and sends only written pages to the swap file. Here
    /// every page goes to the page file, which keeps what the loader
    /// wrote to it (relocations, bound imports) and needs the file no
    /// more.
    pub fn map_pageable(&mut self, bus: &mut Bus, lin: u32, phys: u32, flags: u32) -> Option<()> {
        self.map(bus, lin, phys, flags)?;
        self.make_pageable(bus, lin, phys);
        Some(())
    }

    /// Let the physical page `phys`, mapped at `lin`, go out to the page
    /// file (`page_out`).
    fn make_pageable(&self, bus: &mut Bus, lin: u32, phys: u32) {
        if let Some(at) = self.entry_at(bus, lin)
            && let Some(pool) = bus.win32.pool.as_mut()
        {
            pool.page_file.pageable.insert(phys, at as u32);
        }
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
        let i = self.regions.binary_search_by_key(&start, |r| r.start).ok()?;
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
        let fits = |start: u64| start >= lo as u64 && start + len as u64 <= hi as u64;
        let start = match at {
            Some(at) if at % GRANULARITY == 0 && fits(at as u64) => {
                // The regions are in order and don't overlap: only the
                // first one that ends after `at` can be in the way.
                let i = self.regions.partition_point(|r| r.end() <= at);
                match self.regions.get(i) {
                    Some(r) if (r.start as u64) < at as u64 + len as u64 => return None,
                    _ => at,
                }
            }
            Some(_) => return None,
            None => {
                // The lowest free place: `lo`, or the first multiple of
                // 64 KB after a region, where the gap up to the next
                // region is long enough. A program that makes thousands
                // of regions (a texture's every mipmap level is one)
                // takes one pass over them for each. The pass starts at
                // the first region that ends after `lo`, so the shared
                // arena's search doesn't go over every private region.
                // A region can still end before the candidate: one that
                // fits in the rest of the 64 KB the one before it ends in.
                let mut start = lo as u64;
                let first = self.regions.partition_point(|r| r.end() <= lo);
                for r in &self.regions[first..] {
                    if r.end() as u64 <= start {
                        continue;
                    }
                    if start + len as u64 <= r.start as u64 {
                        break;
                    }
                    start = (r.end() as u64).next_multiple_of(GRANULARITY as u64);
                }
                if !fits(start) {
                    return None;
                }
                start as u32
            }
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
            Some(Place::Ram(entry & !0xFFF, at))
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
                Place::Ram(phys, _) => bus.read_into((phys | at & 0xFFF) as usize, part),
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

    /// Fill `data` from `lin` as a debugger sees it, without a page
    /// fault and with one walk of the tables for each page: the bytes of
    /// a present page and of one that PAGE_NOACCESS keeps come from its
    /// physical page with no device's read side effects, and those of a
    /// page in the page file from there. The bytes of a page that isn't
    /// committed stay as they were.
    pub fn peek(&self, bus: &Bus, lin: u32, data: &mut [u8]) {
        for (offset, run) in Bus::page_runs(lin, data.len()) {
            let at = lin.wrapping_add(offset as u32);
            let part = &mut data[offset..offset + run];
            let Some(entry) = self.entry(bus, at).filter(|&entry| entry != 0) else {
                continue;
            };
            if entry & PRESENT == 0 && is_paged(entry) {
                let page_file = bus.win32.pool.as_ref().map(|pool| &pool.page_file);
                match page_file.and_then(|page_file| page_file.contents(entry >> 12)) {
                    Some(page) => {
                        let from = (at & 0xFFF) as usize;
                        part.copy_from_slice(&page[from..from + run]);
                    }
                    None => part.fill(0),
                }
            } else {
                let phys = (entry & !0xFFF | at & 0xFFF) as usize;
                for (i, byte) in part.iter_mut().enumerate() {
                    *byte = bus.peek_8(phys + i);
                }
            }
        }
    }

    /// Write `data` at `lin`, which is mapped or in the page file, whose
    /// copy is written there: no page fault, and no physical page taken.
    pub fn write(&self, bus: &mut Bus, lin: u32, data: &[u8]) -> Option<()> {
        for (offset, run) in Bus::page_runs(lin, data.len()) {
            let at = lin.wrapping_add(offset as u32);
            let part = &data[offset..offset + run];
            match self.place(bus, at)? {
                Place::Ram(phys, pte) => {
                    bus.write_run((phys | at & 0xFFF) as usize, part);
                    written(bus, pte);
                }
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
                Some(Place::Ram(phys, pte)) => {
                    let from = (phys | at & 0xFFF) as usize;
                    bus.fill_ram(from..from + run, 0);
                    written(bus, pte);
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

    /// Where a debugger finds the page at `lin` when it isn't present: in
    /// the page file, or with PAGE_NOACCESS in the physical page its entry
    /// keeps. None for a page that is present or isn't committed.
    pub fn kept(&self, bus: &Bus, lin: u32) -> Option<Kept> {
        let entry = self.entry(bus, lin)?;
        if entry == 0 || entry & PRESENT != 0 {
            None
        } else if is_paged(entry) {
            Some(Kept::PageFile)
        } else {
            Some(Kept::Ram(entry & !0xFFF))
        }
    }

    /// Whether the page at `lin` is one of a program's own memory, which
    /// goes out to the page file and comes back at another physical
    /// address: it is in the page file now, or its physical page is one
    /// `page_out` may take (or a pinned one, which it may take once the
    /// pins are gone).
    pub fn moves(&self, bus: &Bus, lin: u32) -> bool {
        let Some(entry) = self.entry(bus, lin).filter(|&e| e != 0) else {
            return false;
        };
        let Some(pool) = bus.win32.pool.as_ref() else {
            return false;
        };
        let phys = entry & !0xFFF;
        is_paged(entry)
            || pool.page_file.pageable.contains_key(&phys)
            || pool.page_file.pinned.contains_key(&phys)
    }

    /// The pages in the page file, as runs of pages next to each other
    /// that are all pages of zeros or all have their contents there: at
    /// most `limit` runs, and whether there are more.
    pub fn paged_ranges(&self, bus: &Bus, limit: usize) -> (Vec<PagedRange>, bool) {
        let mut ranges: Vec<PagedRange> = Vec::new();
        for pde in 0..1024u32 {
            let table = bus.read_32((self.cr3 + 4 * pde) as usize);
            if table & PRESENT == 0 {
                continue;
            }
            for pte in 0..1024u32 {
                let entry = bus.read_32(((table & !0xFFF) + 4 * pte) as usize);
                if !is_paged(entry) {
                    continue;
                }
                let (lin, zeros) = (pde << 22 | pte << 12, entry >> 12 == 0);
                let next = |r: &PagedRange| {
                    r.zeros == zeros && r.start.wrapping_add(r.pages * PAGE) == lin
                };
                if let Some(last) = ranges.last_mut().filter(|r| next(r)) {
                    last.pages += 1;
                } else if ranges.len() == limit {
                    return (ranges, true);
                } else {
                    ranges.push(PagedRange {
                        start: lin,
                        pages: 1,
                        zeros,
                    });
                }
            }
        }
        (ranges, false)
    }

    /// Whether the program may reach the page at `lin`, as a touch would
    /// find it: it is mapped, or in the page file with a protection that
    /// lets the program touch it, so the touch brings it in.
    pub fn reachable(&self, bus: &Bus, lin: u32) -> bool {
        let entry = self.entry(bus, lin).unwrap_or(0);
        entry & PRESENT != 0 || is_paged(entry) && protection(entry) != Some(PAGE_NOACCESS)
    }
}

/// Where a page's bytes are: in its physical page (and the address of
/// its entry), or in the page file (the address of its entry, and the
/// entry).
enum Place {
    Ram(u32, usize),
    Paged(usize, u32),
}

/// The system wrote the present page whose entry is at `pte`: set its
/// accessed and dirty bits, as the processor does for a write through
/// the tables, so the page isn't taken for one that holds what it was
/// read with (`mapping::evict`).
fn written(bus: &mut Bus, pte: usize) {
    let entry = bus.read_32(pte);
    if entry & (ACCESSED | DIRTY) != ACCESSED | DIRTY {
        bus.write_32(pte, entry | ACCESSED | DIRTY);
    }
}

/// Where a debugger finds a committed page the processor's tables don't
/// have present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    /// In the physical page at this address: a page made PAGE_NOACCESS,
    /// which keeps its physical page in an entry that isn't present.
    Ram(u32),
    /// In the page file.
    PageFile,
}

/// A run of pages in the page file, for the debugger: the linear address
/// of the first, how many there are, and whether they are pages of zeros
/// with no contents kept (committed and never written, or sent out
/// holding zeros).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PagedRange {
    pub start: u32,
    pub pages: u32,
    pub zeros: bool,
}

/// A zeroed physical page from the pool. When it has none left, a page
/// of programs' own memory or modules' images goes out to the page file
/// to make one.
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
            // The TLB is stale: see `page_out_unused`.
            bus.write_32(at as usize, entry & !ACCESSED);
            bus.win32.tlb_stale = true;
            continue;
        }
        break (phys, at, entry);
    };
    let (phys, at, entry) = victim;
    bus.win32.pool.as_mut().expect("checked").page_file.hand = hand;
    send_out(bus, phys, at, entry);
    true
}

/// Write up to `most` pages of programs' own memory and modules' images
/// that no program has touched since the last search (their entries'
/// accessed bits, which it clears on the others) out to the page file,
/// in one pass over them that starts where the last search stopped
/// (`hand`, which it moves to the last page it sent): with
/// `mapping::evict`, a batch of the pages not used lately, of both
/// kinds. How many went. The TLB is stale after it when it sent a page
/// or cleared an accessed bit: the processor sets the bit only when it
/// walks the tables, so a page whose translation stayed in the TLB
/// would look unused at the next search however often the program
/// touched it.
pub fn page_out_unused(bus: &mut Bus, most: usize) -> usize {
    let Some(pool) = bus.win32.pool.as_ref() else {
        return 0;
    };
    let file = &pool.page_file;
    let after = file
        .pageable
        .range((Bound::Excluded(file.hand), Bound::Unbounded));
    let pages: Vec<(u32, u32)> = after
        .chain(file.pageable.range(..=file.hand))
        .map(|(&p, &a)| (p, a))
        .collect();
    let mut room = file.room() as usize;
    let mut sent = 0;
    for (phys, at) in pages {
        let entry = bus.read_32(at as usize);
        if entry == 0 || is_paged(entry) || entry & !0xFFF != phys {
            let pool = bus.win32.pool.as_mut().expect("checked");
            pool.page_file.pageable.remove(&phys);
            continue;
        }
        if entry & ACCESSED != 0 {
            bus.write_32(at as usize, entry & !ACCESSED);
            bus.win32.tlb_stale = true;
        } else if sent < most && room > 0 {
            send_out(bus, phys, at, entry);
            bus.win32.pool.as_mut().expect("checked").page_file.hand = phys;
            sent += 1;
            room -= 1;
        }
    }
    sent
}

/// Write the page of programs' own memory at `phys`, mapped by the entry
/// at `at`, which is `entry`, out to the page file, and give its physical
/// page back to the pool. The page file has room.
fn send_out(bus: &mut Bus, phys: u32, at: u32, entry: u32) {
    let mut data = vec![0; PAGE as usize];
    bus.read_into(phys as usize, &mut data);
    let protect = protection(entry).unwrap_or(PAGE_READWRITE);
    let pool = bus.win32.pool.as_mut().expect("has room");
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
    // Accessed, as the access that faulted is about to make it: the next
    // search for a page to write out passes it once before taking it,
    // and doesn't send it straight back when the instruction that wants
    // it needs another page that is out too.
    bus.write_32(at, phys | protection_bits(protect) | ACCESSED);
    let pool = bus.win32.pool.as_mut().expect("checked");
    pool.page_file.pageable.insert(phys, at as u32);
    PageIn::Done
}

/// Whether the page at `lin` of the page tables at `cr3` is a committed
/// page in the page file (`PAGED`).
pub fn is_paged_at(bus: &Bus, cr3: u32, lin: u32) -> bool {
    let table = bus.read_32((cr3 + 4 * (lin >> 22)) as usize);
    table & PRESENT != 0
        && is_paged(bus.read_32(((table & !0xFFF) + 4 * ((lin >> 12) & 0x3FF)) as usize))
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

/// A page fault of the instruction (or service) whose context hashes to
/// `key` is to be served. One instruction may need several pages at once
/// (its code and its operands, each across a page boundary, and the
/// stack), and runs again from its start after each fault: a page it got
/// in that went out again for the next would bring it back for ever. So
/// the pages it brings in stay (`hold`) while its faults go on, and are
/// let go at the first fault of another context, which an instruction
/// that ran, or a string instruction that moved on, has. With every page
/// it needs held, the pool is too small for it: the fault fails, with a
/// report in the log (`out_of_memory`), and nothing goes round.
pub fn retry(bus: &mut Bus, key: u64) {
    let Some(file) = bus.win32.pool.as_mut().map(|p| &mut p.page_file) else {
        return;
    };
    if file.retry_key == key {
        return;
    }
    file.retry_key = key;
    for (phys, at) in std::mem::take(&mut file.held) {
        file.let_go(phys, at);
    }
}

/// The most pages `hold` keeps for one context. An instruction needs at
/// most six at once (its bytes, two memory operands or one and the
/// stack, each across a page boundary), and these are more. A service
/// can fault more often with the same registers, since its arguments
/// are in memory, and two calls of it, each needing other pages, may
/// have the same context: the pages beyond these let the oldest go, so
/// that a run of such calls doesn't hold the pool's pages one by one
/// until none can go.
const HOLDS: usize = 16;

impl PageFile {
    /// `hold` lets go of the page `phys`, whose entry is at `at`.
    fn let_go(&mut self, phys: u32, at: u32) {
        if let Some((count, pinned_at)) = self.pinned.get_mut(&phys)
            && *pinned_at == at
        {
            *count -= 1;
            if *count == 0 {
                self.pinned.remove(&phys);
                self.pageable.insert(phys, at);
            }
        }
    }
}

/// Keep the present page at `lin` of the page tables at `cr3`, which a
/// page fault of the instruction `retry` was told of brought in, in the
/// pool until that instruction's faults end: out of `page_out`'s reach,
/// and of `mapping::evict`'s for a page of a view of a file. The last
/// `HOLDS` pages are kept.
pub fn hold(bus: &mut Bus, cr3: u32, lin: u32) {
    let table = bus.read_32((cr3 + 4 * (lin >> 22)) as usize);
    if table & PRESENT == 0 {
        return;
    }
    let at = (table & !0xFFF) + 4 * ((lin >> 12) & 0x3FF);
    let entry = bus.read_32(at as usize);
    if entry & PRESENT == 0 {
        return;
    }
    let phys = entry & !0xFFF;
    let Some(file) = bus.win32.pool.as_mut().map(|p| &mut p.page_file) else {
        return;
    };
    if file.held.contains(&(phys, at)) {
        return;
    }
    if file.pageable.remove(&phys).is_some() {
        file.pinned.insert(phys, (1, at));
    } else if let Some((count, pinned_at)) = file.pinned.get_mut(&phys)
        && *pinned_at == at
    {
        *count += 1;
    }
    file.held.push((phys, at));
    if file.held.len() > HOLDS {
        let (phys, at) = file.held.remove(0);
        file.let_go(phys, at);
    }
}

/// Whether the physical page `phys` is held for the instruction that
/// faults (`hold`).
pub fn held(bus: &Bus, phys: u32) -> bool {
    bus.win32
        .pool
        .as_ref()
        .is_some_and(|p| p.page_file.held.iter().any(|&(held, _)| held == phys))
}

/// Report in the log that the page at `lin` can't come in: no page of the
/// pool can make room for it.
pub fn out_of_memory(bus: &mut Bus, lin: u32) {
    let Some(pool) = bus.win32.pool.as_ref() else {
        return;
    };
    let file = &pool.page_file;
    let held = file.held.len();
    let held_pinned = file
        .held
        .iter()
        .filter(|(p, _)| file.pinned.contains_key(p));
    let pinned = file.pinned.len() - held_pinned.count();
    let line = format!(
        "[W32] Out of memory: the page at {lin:08X} can't come in. Of the pool's {} pages, \
         {held} are held for the instruction that faulted, {pinned} are pinned by the \
         system, {} of programs' own memory and modules' images may go out, and the rest \
         are the system's own; the page file has room for {} more",
        pool.len / PAGE,
        file.pageable.len(),
        file.room(),
    );
    bus.log_string(&line);
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
    hand,
    view_hand,
    retry_key,
    held
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
    fn machine(pages: u32, file: u32) -> (Box<Cpu>, Memory) {
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

    /// The pool lends the pages it never used to the XMS pool for a DOS
    /// program the system runs, and takes them back once all of them are
    /// free again; while the program keeps one, the pool stays smaller.
    #[test]
    fn lends_its_unused_pages_and_takes_them_back() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.win32.pool = Pool::take(&mut cpu.bus, 0x40_0000);
        let _memory = Memory::new(&mut cpu.bus).expect("a page directory");
        let end = cpu.bus.ram().len() as u32;
        let pool = cpu.bus.win32.pool.as_ref().expect("a pool");
        let (base, len, next) = (pool.base, pool.len, pool.next);
        assert!(next > 0, "the page directory came from the pool");
        let (_, free) = cpu.bus.xms.dpmi_free(end);
        let lent = Pool::lend(&mut cpu.bus);
        assert_eq!(lent, len - next);
        assert_eq!(cpu.bus.xms.dpmi_free(end).1, free + lent);
        assert_eq!(cpu.bus.win32.pool.as_ref().unwrap().len, next);
        // A program keeps a page of it.
        assert!(cpu.bus.xms.take_dpmi_at(base + next, PAGE, end));
        assert!(!Pool::take_back(&mut cpu.bus, lent));
        assert_eq!(cpu.bus.win32.pool.as_ref().unwrap().len, next);
        cpu.bus.xms.release_dpmi(base + next);
        assert!(Pool::take_back(&mut cpu.bus, lent));
        assert_eq!(cpu.bus.win32.pool.as_ref().unwrap().len, len);
        assert_eq!(cpu.bus.xms.dpmi_free(end).1, free);
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

    /// An image of more pages than the pool has, mapped page by page as
    /// the loader maps one and then written whole: its pages go out to
    /// the page file to make room for the next, every byte reads back,
    /// and a page of the program's own memory still comes in after it.
    /// Each page that went out comes back read-only, as it was mapped.
    #[test]
    fn an_image_larger_than_the_pool_is_paged() {
        let (mut cpu, mut memory) = machine(4, 32);
        let image = page(16);
        for n in 0..8 {
            let phys = memory.page(&mut cpu.bus).expect("a page, one going out");
            memory
                .map_pageable(&mut cpu.bus, image + n * PAGE, phys, USER)
                .expect("mapped");
        }
        let data: Vec<u8> = (0..8 * PAGE).map(|i| (i / PAGE + 1) as u8).collect();
        memory.write(&mut cpu.bus, image, &data).expect("written");
        memory
            .commit_paged(&mut cpu.bus, page(0), PAGE, PAGE_READWRITE)
            .expect("room");
        touch(&mut cpu, &memory, page(0), 0x5A);
        assert_eq!(bytes(&cpu, &memory, image, data.len()), data);
        let out: Vec<u32> = (0..8)
            .map(|n| image + n * PAGE)
            .filter(|&lin| memory.translate(&cpu.bus, lin).is_none())
            .collect();
        assert!(out.len() >= 4, "only {} pages of the image went out", out.len());
        for lin in out {
            let entry = memory.entry(&cpu.bus, lin).expect("an entry");
            assert!(is_paged(entry), "{lin:08X}: {entry:08X}");
            assert_eq!(page_in(&mut cpu.bus, memory.cr3, lin), PageIn::Done);
            let entry = memory.entry(&cpu.bus, lin).expect("an entry");
            assert_eq!(entry & (PRESENT | WRITABLE), PRESENT, "{lin:08X}: {entry:08X}");
            assert_eq!(protection(entry), Some(PAGE_READONLY), "{lin:08X}: {entry:08X}");
        }
        assert_eq!(bytes(&cpu, &memory, page(0), 4), [0x5A; 4]);
        assert_eq!(bytes(&cpu, &memory, image, data.len()), data);
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

    /// The faults of an instruction that needs the pages at `need` at
    /// once, as the processor runs it: it reaches them in that order,
    /// setting each one's accessed bit, and the first that isn't there
    /// faults, comes in as the built-in Windows brings it (`retry`,
    /// `page_in`, `hold`), and the instruction runs again from its start.
    /// How many faults it took, or the page that couldn't come in. It
    /// gives up after 64 faults.
    fn run_instruction(cpu: &mut Cpu, memory: &Memory, need: &[u32]) -> Result<u32, u32> {
        for faults in 0..64 {
            let missing = need.iter().copied().find(|&lin| {
                let at = memory.entry_at(&cpu.bus, lin).expect("a page table");
                let entry = cpu.bus.read_32(at);
                if entry & PRESENT == 0 {
                    return true;
                }
                cpu.bus.write_32(at, entry | ACCESSED);
                false
            });
            let Some(lin) = missing else {
                return Ok(faults);
            };
            retry(&mut cpu.bus, 0x1115);
            if page_in(&mut cpu.bus, memory.cr3, lin) != PageIn::Done {
                return Err(lin);
            }
            hold(&mut cpu.bus, memory.cr3, lin);
        }
        panic!("the instruction faults for ever");
    }

    /// An instruction whose code is on a page in the pool and whose two
    /// operands are on pages in the page file, on a pool with room for its
    /// three pages and with one to spare: each fault brings one in, and the
    /// pages it brought stay until it runs, so it runs after two faults,
    /// whatever page the search for one to send out starts from. With
    /// room for two pages only, the fault that can't be served fails
    /// instead of sending out a page the instruction needs.
    #[test]
    fn an_instruction_needing_pages_sent_out_runs_once_they_fit() {
        // The page table of the program's memory takes one of the pool's
        // pages.
        for (pool, fits) in [(5, true), (4, true), (3, false)] {
            for start in 0..6 {
                let (mut cpu, mut memory) = machine(pool, 32);
                memory
                    .commit_paged(&mut cpu.bus, page(0), 8 * PAGE, PAGE_READWRITE)
                    .expect("room");
                // Touched in an order that leaves the search's hand at a
                // different place each time; the operands touched first,
                // so they are out when the code is touched last.
                for n in (0..6).map(|n| 2 + (n + start) % 6) {
                    touch(&mut cpu, &memory, page(n), n as u8);
                }
                touch(&mut cpu, &memory, page(0), 0xA0);
                touch(&mut cpu, &memory, page(1), 0xA1);
                for n in 2..8 {
                    touch(&mut cpu, &memory, page(n), n as u8);
                }
                let code = page(7);
                let operands = [page(0), page(1)];
                assert!(
                    operands
                        .iter()
                        .all(|&p| memory.translate(&cpu.bus, p).is_none())
                );
                let result = run_instruction(&mut cpu, &memory, &[code, operands[0], operands[1]]);
                if fits {
                    assert!(result.is_ok(), "pool {pool}, start {start}: {result:X?}");
                    assert_eq!(bytes(&cpu, &memory, page(0), 4), [0xA0; 4]);
                    assert_eq!(bytes(&cpu, &memory, page(1), 4), [0xA1; 4]);
                } else {
                    assert!(result.is_err(), "pool {pool}, start {start}: ran");
                }
            }
        }
    }

    /// The physical page of the present page at `lin`.
    fn phys_of(cpu: &Cpu, memory: &Memory, lin: u32) -> u32 {
        memory.translate(&cpu.bus, lin).expect("present") & !0xFFF
    }

    /// A search that clears accessed bits makes the TLB stale, though it
    /// sends no page out: the processor sets a bit again only when it
    /// walks the tables, and a translation left in the TLB would let the
    /// program use the page without the next search seeing it.
    #[test]
    fn a_search_that_clears_accessed_bits_makes_the_tlb_stale() {
        let (mut cpu, mut memory) = machine(8, 32);
        memory
            .commit_paged(&mut cpu.bus, page(0), 4 * PAGE, PAGE_READWRITE)
            .expect("room");
        for n in 0..4 {
            touch(&mut cpu, &memory, page(n), n as u8);
        }
        cpu.bus.win32.tlb_stale = false;
        assert_eq!(page_out_unused(&mut cpu.bus, 4), 0);
        assert!(cpu.bus.win32.tlb_stale);
        for n in 0..4 {
            let entry = memory.entry(&cpu.bus, page(n)).expect("an entry");
            assert_eq!(entry & ACCESSED, 0, "page {n}: {entry:08X}");
        }
    }

    /// The search for pages not used lately goes on after the last page
    /// it sent out: a page that came back in since, touched after the
    /// others, isn't the next to go just because its physical page is
    /// the lowest.
    #[test]
    fn the_search_for_unused_pages_goes_on_from_where_it_stopped() {
        let (mut cpu, mut memory) = machine(8, 32);
        memory
            .commit_paged(&mut cpu.bus, page(0), 4 * PAGE, PAGE_READWRITE)
            .expect("room");
        for n in 0..4 {
            touch(&mut cpu, &memory, page(n), n as u8);
        }
        let mut by_phys: Vec<u32> = (0..4).collect();
        by_phys.sort_by_key(|&n| phys_of(&cpu, &memory, page(n)));
        let (first, second) = (page(by_phys[0]), page(by_phys[1]));
        page_out_unused(&mut cpu.bus, 0);
        assert_eq!(page_out_unused(&mut cpu.bus, 1), 1);
        assert!(memory.translate(&cpu.bus, first).is_none(), "the first went");
        // Back in, at the physical page it had, and then untouched as the
        // others are until the next search.
        assert_eq!(page_in(&mut cpu.bus, memory.cr3, first), PageIn::Done);
        page_out_unused(&mut cpu.bus, 0);
        assert_eq!(page_out_unused(&mut cpu.bus, 1), 1);
        assert!(memory.translate(&cpu.bus, first).is_some(), "it went again");
        assert!(memory.translate(&cpu.bus, second).is_none(), "the next stayed");
    }

    /// The pages held for one context stay at `HOLDS`: the oldest may go
    /// to the page file again once more are held, the latest may not.
    #[test]
    fn the_pages_held_for_one_context_are_bounded() {
        let pages = HOLDS as u32 + 8;
        let (mut cpu, mut memory) = machine(64, 64);
        memory
            .commit_paged(&mut cpu.bus, page(0), pages * PAGE, PAGE_READWRITE)
            .expect("room");
        retry(&mut cpu.bus, 0x1115);
        for n in 0..pages {
            assert_eq!(page_in(&mut cpu.bus, memory.cr3, page(n)), PageIn::Done);
            hold(&mut cpu.bus, memory.cr3, page(n));
        }
        assert_eq!(file(&cpu).held.len(), HOLDS);
        for n in 0..pages {
            let phys = phys_of(&cpu, &memory, page(n));
            let kept = n >= pages - HOLDS as u32;
            assert_eq!(held(&cpu.bus, phys), kept, "page {n}");
            assert_eq!(file(&cpu).pageable.contains_key(&phys), !kept, "page {n}");
        }
        retry(&mut cpu.bus, 0x1116);
        assert!(file(&cpu).held.is_empty() && file(&cpu).pinned.is_empty());
        assert_eq!(file(&cpu).pageable.len(), pages as usize);
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

    /// The machine's processor with paging on and the page tables of
    /// `memory`, as it is while the process runs: what the debugger's
    /// reads and writes walk.
    fn run_in(cpu: &mut Cpu, memory: &Memory) {
        cpu.cr0 |= crate::cpu::CR0_PE | crate::cpu::CR0_PG;
        cpu.cr3 = memory.cr3;
    }

    /// The page file's counts and what it and the pool hold.
    fn state(cpu: &Cpu) -> (u64, u64, u64, u32, usize, u32) {
        let counts = cpu.bus.win32.paging;
        let pool = cpu.bus.win32.pool.as_ref().expect("a pool");
        let f = &pool.page_file;
        (
            counts.zero_ins,
            counts.ins,
            counts.outs,
            f.used,
            f.pageable.len(),
            pool.commit_room(),
        )
    }

    /// The debugger's write to a page that went out to the page file
    /// changes the page's contents there. The page stays there, with no
    /// page in or out for it, and comes in with the bytes written when
    /// the program touches it.
    #[test]
    fn the_debuggers_write_to_a_page_sent_out_comes_in_with_it() {
        use crate::win32::{kept_page, peek_paged, poke_paged};
        let (mut cpu, mut memory) = machine(4, 32);
        memory
            .commit_paged(&mut cpu.bus, page(0), 8 * PAGE, PAGE_READWRITE)
            .expect("room in the page file");
        for n in 0..8 {
            touch(&mut cpu, &memory, page(n), n as u8 + 1);
        }
        run_in(&mut cpu, &memory);
        let out = (0..8)
            .find(|&n| memory.translate(&cpu.bus, page(n)).is_none())
            .expect("a page went out");
        let before = state(&cpu);
        let at = page(out) + 0x10;
        assert_eq!(kept_page(&cpu, at), Some(Kept::PageFile));
        poke_paged(&mut cpu, at, &[0xA1, 0xA2, 0xA3]).expect("in the page file");
        let mut read = [0; 5];
        peek_paged(&cpu, at - 1, &mut read).expect("in the page file");
        let old = out as u8 + 1;
        assert_eq!(read, [old, 0xA1, 0xA2, 0xA3, old]);
        assert_eq!(kept_page(&cpu, at), Some(Kept::PageFile), "it stays there");
        assert_eq!(state(&cpu), before);
        assert_eq!(page_in(&mut cpu.bus, memory.cr3, at), PageIn::Done);
        let phys = memory.translate(&cpu.bus, at).expect("in") as usize;
        assert_eq!(
            cpu.bus.read_32(phys - 1),
            u32::from_le_bytes([old, 0xA1, 0xA2, 0xA3])
        );
        assert_eq!(kept_page(&cpu, at), None, "present");
    }

    /// A committed page nobody touched has its room in the page file
    /// from the commit on, so the debugger's write to it finds a place
    /// for its contents also when the page file is full: the room taken
    /// and what a program can still commit stay as they were.
    #[test]
    fn the_debuggers_write_to_an_untouched_page_takes_no_room_in_a_full_page_file() {
        use crate::win32::{peek_paged, poke_paged};
        let (mut cpu, mut memory) = machine(2, 4);
        memory
            .commit_paged(&mut cpu.bus, page(0), 4 * PAGE, PAGE_READWRITE)
            .expect("room in the page file");
        run_in(&mut cpu, &memory);
        assert_eq!(file(&cpu).room(), 0, "full");
        let before = state(&cpu);
        let data: Vec<u8> = (0..PAGE + 8).map(|i| i as u8 | 1).collect();
        poke_paged(&mut cpu, page(1) - 4, &data).expect("committed");
        let mut read = vec![0; data.len()];
        peek_paged(&cpu, page(1) - 4, &mut read).expect("committed");
        assert_eq!(read, data);
        assert_eq!(state(&cpu), before);
        assert_eq!(
            file(&cpu).slots.len(),
            3,
            "the three pages written have contents"
        );
        // A page that isn't committed is refused, and so is a write that
        // reaches one.
        assert_eq!(poke_paged(&mut cpu, page(4), &[1]), None);
        assert_eq!(crate::win32::kept_page(&cpu, page(4)), None);
    }

    /// Where the debugger finds pages the processor's tables don't have
    /// present, and which pages it keeps by their linear address: a page
    /// made PAGE_NOACCESS in the physical page its entry keeps, pages in
    /// the page file whatever their protection, which the debugger's
    /// write leaves as it is.
    #[test]
    fn the_debugger_finds_pages_that_are_not_present_and_keeps_their_protection() {
        use crate::win32::{kept_page, page_moves, poke_paged};
        let (mut cpu, mut memory) = machine(8, 32);
        memory
            .commit_paged(&mut cpu.bus, page(0), 3 * PAGE, PAGE_READWRITE)
            .expect("room");
        run_in(&mut cpu, &memory);
        // In memory, then made PAGE_NOACCESS.
        touch(&mut cpu, &memory, page(0), 0x11);
        let phys = memory.translate(&cpu.bus, page(0)).expect("in");
        assert!(page_moves(&cpu, page(0)), "a page of the program's own");
        assert_eq!(kept_page(&cpu, page(0)), None, "present");
        assert!(memory.set_protection(&mut cpu.bus, page(0), PAGE_NOACCESS));
        assert_eq!(kept_page(&cpu, page(0) + 5), Some(Kept::Ram(phys)));
        assert!(page_moves(&cpu, page(0)));
        // In the page file, read-only and PAGE_NOACCESS.
        assert!(memory.set_protection(&mut cpu.bus, page(1), PAGE_READONLY));
        assert!(memory.set_protection(&mut cpu.bus, page(2), PAGE_NOACCESS));
        for (n, protect) in [(1, PAGE_READONLY), (2, PAGE_NOACCESS)] {
            assert_eq!(kept_page(&cpu, page(n)), Some(Kept::PageFile));
            assert!(page_moves(&cpu, page(n)));
            poke_paged(&mut cpu, page(n) + 8, &[0x77]).expect("in the page file");
            let entry = memory.entry(&cpu.bus, page(n)).expect("an entry");
            assert!(is_paged(entry), "page {n} stays in the page file");
            assert_eq!(protection(entry), Some(protect), "page {n}");
            assert_eq!(bytes(&cpu, &memory, page(n) + 8, 1), [0x77]);
        }
        // The system's own pages don't move (one of the first megabyte),
        // nor does one that isn't committed.
        assert!(!page_moves(&cpu, 0x1000));
        assert!(!page_moves(&cpu, page(3)));
        assert_eq!(kept_page(&cpu, page(3)), None);
        // With paging off the tables aren't walked.
        cpu.cr0 &= !crate::cpu::CR0_PG;
        assert_eq!(kept_page(&cpu, page(1)), None);
        assert!(!page_moves(&cpu, page(1)));
    }

    /// The pages in the page file as runs: pages of zeros and pages with
    /// contents apart, pages in memory left out.
    #[test]
    fn the_pages_in_the_page_file_are_listed_as_runs() {
        let (mut cpu, mut memory) = machine(8, 32);
        memory
            .commit_paged(&mut cpu.bus, page(0), 6 * PAGE, PAGE_READWRITE)
            .expect("room");
        memory
            .write(&mut cpu.bus, page(2), &[1])
            .expect("committed");
        touch(&mut cpu, &memory, page(4), 0x44);
        let run = |start, pages, zeros| PagedRange {
            start,
            pages,
            zeros,
        };
        let all = [
            run(page(0), 2, true),
            run(page(2), 1, false),
            run(page(3), 1, true),
            run(page(5), 1, true),
        ];
        assert_eq!(memory.paged_ranges(&cpu.bus, 100), (all.to_vec(), false));
        assert_eq!(memory.paged_ranges(&cpu.bus, 4), (all.to_vec(), false));
        assert_eq!(memory.paged_ranges(&cpu.bus, 2), (all[..2].to_vec(), true));
    }

    /// Where the old search put a region: the lowest of `lo` and the
    /// multiples of 64 KB after each region that leaves room for it.
    fn lowest_free_reference(regions: &[Region], len: u32, lo: u32, hi: u32) -> Option<u32> {
        let free = |start: u64| {
            let end = start + len as u64;
            start >= lo as u64
                && end <= hi as u64
                && regions
                    .iter()
                    .all(|r| end <= r.start as u64 || start >= r.end() as u64)
        };
        std::iter::once(lo as u64)
            .chain(
                regions
                    .iter()
                    .map(|r| (r.end() as u64).next_multiple_of(GRANULARITY as u64)),
            )
            .filter(|&start| free(start))
            .min()
            .map(|start| start as u32)
    }

    /// Regions reserved and released in a mixed order, of sizes that
    /// leave gaps of every kind, some with the place asked for and some in
    /// the arenas below and above: each goes where the search over every
    /// pair of a candidate and a region put it, and none overlaps another.
    #[test]
    fn regions_go_to_the_lowest_free_place() {
        let mut memory = Memory::default();
        let mut seed = 0x1234_5678u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let (lo, hi) = (PRIVATE, PRIVATE + 0x400_0000);
        let mut held = Vec::new();
        for step in 0..3000 {
            if next() % 3 == 0 && !held.is_empty() {
                let i = next() as usize % held.len();
                let start = held.swap_remove(i);
                assert!(memory.unreserve(start).is_some(), "step {step}");
                continue;
            }
            let len = (next() % 40 + 1) * PAGE * if next() % 8 == 0 { 16 } else { 1 };
            let rounded = len.next_multiple_of(PAGE);
            if next() % 5 == 0 {
                let at = lo + (next() % 0x400) * GRANULARITY;
                let free = memory
                    .regions
                    .iter()
                    .all(|r| at + rounded <= r.start || at >= r.end())
                    && at + rounded <= hi;
                let got =
                    memory.reserve_region(Some(at), len, lo, hi, PAGE_READWRITE, MEM_PRIVATE);
                assert_eq!(got, free.then_some(at), "step {step}: at {at:#x}");
                held.extend(got);
                continue;
            }
            // Some go to the arenas below and above, so that each search
            // has regions of another arena on either side of its own.
            let (lo, hi) = match next() % 8 {
                0 => (GRANULARITY, lo),
                1 => (hi, hi + 0x100_0000),
                _ => (lo, hi),
            };
            let want = lowest_free_reference(&memory.regions, rounded, lo, hi);
            let got = memory.reserve_region(None, len, lo, hi, PAGE_READWRITE, MEM_PRIVATE);
            assert_eq!(got, want, "step {step}: {len:#x} bytes");
            held.extend(got);
        }
        assert!(
            memory.regions.len() > 100,
            "{} regions",
            memory.regions.len()
        );
        for pair in memory.regions.windows(2) {
            assert!(pair[0].end() <= pair[1].start, "{pair:?}");
        }
    }
}
