//! File mappings' memory (CreateFileMapping, MapViewOfFile): as on
//! Windows 95, a mapping has one place in the shared arena, the same in
//! every process, and its views are its pages mapped there in each
//! process that maps it. Its pages are the system's, not a process's:
//! they go back to the pool when the mapping's object is gone and no view
//! of it is left.
//!
//! A mapping of a file takes no memory when it is made. As on Windows 95,
//! its pages are read from the file when they are first touched: a page
//! fault in a view (`fault_in`), or a system call reading or writing the
//! view (`page_in`, from the bus's accessors). When the pool runs low,
//! the pages not used lately leave it (`evict`), as Windows 98 pages
//! out a mapped file: one no view wrote is read from the file again when
//! it is needed, a written one of a mapping that may write its file is
//! written to the file first, and a written one of a copy-on-write
//! mapping (PAGE_WRITECOPY), whose file keeps what it had, goes to the
//! page file and comes back from there. Pages read from files leave the
//! last part of the pool (`RESERVE`) to the rest of the system: once the
//! pool is down to it, a page fault on a view evicts before it reads. A
//! mapping of the swap file has all its pages from the start, or with
//! SEC_RESERVE those VirtualAlloc commits: nothing else holds their
//! contents.

use std::collections::BTreeMap;

use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::win32::memory::{
    self, PAGE, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY, protection,
    protection_bits,
};

/// Where mappings are put in the shared arena: above what the processes
/// take there for their threads' TIBs, which they take from its bottom,
/// and below the built-in DLLs.
pub const ARENA: (u32, u32) = (0xA000_0000, 0xBF00_0000);

/// Page table entry bits the processor sets: accessed, and dirty.
const ACCESSED: u32 = 0x20;
const DIRTY: u32 = 0x40;

/// How many pages `evict` gives back at a time, at least: one scan of the
/// mappings' pages makes room for the next few faults.
const EVICTED: usize = 16;

/// The share of the pages read from files `evict` gives back at a time
/// when that is more than `EVICTED`: one in this many, so that the scans
/// cost a few pages' work each however many pages are read.
const EVICTED_SHARE: usize = 16;

/// The share of the pool pages read from files leave free: one in this
/// many of its pages, for other allocations (VirtualAlloc, heaps, stacks,
/// page tables), which get one at once while it lasts.
const RESERVE: u32 = 8;

#[derive(Clone, Debug, Default)]
pub struct Section {
    /// Its kernel object.
    pub object: u32,
    /// Its address in the shared arena and its size.
    pub base: u32,
    pub size: u32,
    /// The protection it was made with (a PAGE_ value), and whether its
    /// pages are only reserved until VirtualAlloc commits them
    /// (SEC_RESERVE).
    pub protect: u32,
    pub reserve: bool,
    /// Each page's physical page, or 0 where none is committed, or for a
    /// mapping of a file where the page wasn't read from it yet or is in
    /// the page file.
    pub pages: Vec<u32>,
    /// The pages of a copy-on-write mapping of a file that a view wrote
    /// and that left the pool, by their index in `pages`: they hold the
    /// program's copy, not the file's bytes, and go to the page file
    /// whenever they leave the pool again, written since or not
    /// (`evict`). Each has its place in the page file while it is there
    /// (`PageFile::keep`), and None while it is in the pool.
    pub copies: BTreeMap<u32, Option<u32>>,
    /// The file it maps, as the object of the program's handle of it,
    /// which the mapping holds, and its entry in DOS's file table; or
    /// None for a mapping of the swap file.
    pub file: Option<(u32, u16)>,
    /// Its views: the process's id, the view's address, and the
    /// protection the view gives its pages (a PAGE_ value).
    pub views: Vec<(u32, u32, u32)>,
}

impl Section {
    /// The end of its place: its pages' end.
    pub fn end(&self) -> u32 {
        self.base + self.pages.len() as u32 * PAGE
    }

    /// Whether the process `pid` has a view of it.
    pub fn viewed_by(&self, pid: u32) -> bool {
        self.views.iter().any(|&(p, _, _)| p == pid)
    }

    /// The protection the pages have in the process `pid`, which all its
    /// views share: writable if one of them may write. None without a
    /// view.
    fn protection_in(&self, pid: u32) -> Option<u32> {
        let mut protections = self
            .views
            .iter()
            .filter(|&&(p, _, _)| p == pid)
            .map(|&(_, _, protect)| protect)
            .peekable();
        protections.peek()?;
        Some(protections.fold(PAGE_NOACCESS, |kept, protect| {
            if protect == PAGE_READWRITE || kept == PAGE_READWRITE {
                PAGE_READWRITE
            } else if protect == PAGE_READONLY {
                PAGE_READONLY
            } else {
                kept
            }
        }))
    }
}

impl super::Win32 {
    /// Whether the physical page `phys` is a file mapping's, which a
    /// process's memory gives back to the pool only with the mapping.
    pub fn shared(&self, phys: u32) -> bool {
        self.sections
            .iter()
            .flatten()
            .any(|s| s.pages.contains(&phys))
    }

    /// The page of a file mapping that a view of the process `pid` has at
    /// `lin` and that is read from the file when it is touched: its
    /// section, its index there and the protection the view gives it.
    /// Whether the process's page tables have it yet is the caller's to
    /// check.
    pub fn file_page(&self, pid: u32, lin: u32) -> Option<(usize, usize, u32)> {
        self.sections.iter().enumerate().find_map(|(s, section)| {
            let section = section.as_ref()?;
            if section.file.is_none() || !(section.base..section.end()).contains(&lin) {
                return None;
            }
            let protect = section.protection_in(pid)?;
            Some((s, ((lin - section.base) / PAGE) as usize, protect))
        })
    }
}

/// The protection of the page at `lin` of the process `process` if it is
/// a page of a view of a file that its page tables don't have yet, which
/// is committed all the same: the first touch reads it.
pub fn unread_protection(bus: &Bus, process: &super::Process, lin: u32) -> Option<u32> {
    if process.memory.entry(bus, lin).unwrap_or(0) != 0 {
        return None;
    }
    bus.win32
        .file_page(process.id, lin)
        .map(|(_, _, protect)| protect)
        .filter(|&protect| protect != PAGE_NOACCESS)
}

/// Read the pages of views of files not read yet in the `len` bytes at
/// `lin` of the running process, for a service that hands them to work
/// that reads them later without a page fault (a wave device playing a
/// block). False when one needs room the pool doesn't have: the service
/// stops with a page fault there and runs again once it is read
/// (`page_in`). Other pages are left as they are.
pub fn read_ahead(bus: &mut Bus, lin: u32, len: u32) -> bool {
    if len == 0 {
        return true;
    }
    let first = lin & !(PAGE - 1);
    let last = lin.saturating_add(len - 1) & !(PAGE - 1);
    for page in (first..=last).step_by(PAGE as usize) {
        let unread = bus
            .win32
            .processes
            .get(bus.win32.running)
            .is_some_and(|p| unread_protection(bus, p, page).is_some());
        if unread {
            bus.guest_read_8(page);
            if bus.guest_faulted() {
                return false;
            }
        }
    }
    true
}

/// What `page_in` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageIn {
    /// The page is there now.
    Done,
    /// The page is one to read, but the pool has no page for it, or for
    /// its page table, or is down to what is kept for the rest (`RESERVE`).
    Full,
    /// It isn't a page of a view of a file waiting to be read.
    No,
}

/// The pages of the pool that pages read from files leave free.
fn reserve(bus: &Bus) -> u32 {
    bus.win32
        .pool
        .as_ref()
        .map_or(0, |p| p.len / PAGE / RESERVE)
}

/// Make room in the pool before a page fault takes a page from it, when
/// it is down to its last few (`EVICTED`): a `sweep`. Whether any page
/// went; the TLB is then stale. Every process must be in the list.
pub fn make_room(bus: &mut Bus) -> bool {
    let left = bus
        .win32
        .pool
        .as_ref()
        .map_or(u32::MAX, |p| p.available() / PAGE);
    left < EVICTED as u32 && sweep(bus)
}

/// Give back to the pool a batch of the pages that no program touched
/// since the last sweep, of both kinds: those of views of files
/// (`evict`) and those of programs' own memory and modules' images,
/// which go out to the page file (`memory::page_out_unused`); and clear
/// the accessed bits of the others. A page untouched from one sweep to
/// the next goes, whichever kind it is, so neither kind keeps pages the
/// other needs. Whether any went; the TLB is then stale. Every process
/// must be in the list.
pub fn sweep(bus: &mut Bus) -> bool {
    let views = evict(bus, false);
    let pageable = bus
        .win32
        .pool
        .as_ref()
        .map_or(0, |p| p.page_file.pageable_len());
    let own = memory::page_out_unused(bus, (pageable / EVICTED_SHARE).max(EVICTED));
    views || own > 0
}

/// Put the page at `lin` of a view of a file mapping in the page tables of
/// the running process, whose page directory is at `cr3`, reading it from
/// the file if no process has read it yet. With only the bus, for the
/// system's own reads and writes of the view: nothing is evicted, since a
/// process's memory may be out of the list while it runs (`with_process`),
/// which a page fault (`fault_in`) never finds.
pub fn page_in(bus: &mut Bus, cr3: u32, lin: u32) -> PageIn {
    let running = bus.win32.running;
    let ours = bus
        .win32
        .processes
        .get(running)
        .is_some_and(|p| cr3 != 0 && p.memory.cr3 == cr3);
    if !ours {
        return PageIn::No;
    }
    let keep = reserve(bus);
    read_in(bus, running, lin, keep)
}

/// `page_in` for the process at `index` in the list, taking a page for the
/// file's contents only while the pool has more than `keep` left. A
/// process out of the list while the system works on it
/// (`with_process`) has an empty memory there, and gets nothing.
fn read_in(bus: &mut Bus, index: usize, lin: u32, keep: u32) -> PageIn {
    let Some(process) = bus.win32.processes.get(index) else {
        return PageIn::No;
    };
    let page = lin & !(PAGE - 1);
    if process.memory.cr3 == 0 || process.memory.entry(bus, page).unwrap_or(0) != 0 {
        return PageIn::No;
    }
    let Some((s, i, protect)) = bus.win32.file_page(process.id, page) else {
        return PageIn::No;
    };
    if protect == PAGE_NOACCESS {
        return PageIn::No;
    }
    let section = bus.win32.sections[s].as_ref().expect("found");
    let mut phys = section.pages[i];
    if phys == 0 {
        let (Some((_, sft)), size) = (section.file, section.size) else {
            return PageIn::No;
        };
        let left = bus.win32.pool.as_ref().map_or(0, |p| p.available() / PAGE);
        if left <= keep {
            return PageIn::Full;
        }
        let Some(new) = memory::pool_page(bus) else {
            return PageIn::Full;
        };
        let section = bus.win32.sections[s].as_mut().expect("found");
        if let Some(slot) = section.copies.get_mut(&(i as u32)).and_then(Option::take) {
            // A written page of a copy-on-write view, back from the page
            // file.
            let pool = bus.win32.pool.as_mut().expect("a page came from it");
            if let Some(data) = pool.page_file.give_back(slot) {
                bus.load_bytes(new as usize, &data);
            }
            bus.win32.paging.ins += 1;
        } else {
            let offset = i as u32 * PAGE;
            let len = (size - offset).min(PAGE) as usize;
            // A file that can't be read any more leaves the page zeroed.
            if let Some(data) = crate::interrupts::int21::read_file_at(bus, sft, offset, len) {
                bus.load_bytes(new as usize, &data);
            }
            bus.win32.paging.reads += 1;
        }
        bus.win32.sections[s].as_mut().expect("found").pages[i] = new;
        phys = new;
    }
    let mut process = std::mem::take(&mut bus.win32.processes[index]);
    let mapped = process
        .memory
        .set_entry(bus, page, phys | protection_bits(protect) | ACCESSED);
    bus.win32.processes[index] = process;
    match mapped {
        Some(()) => PageIn::Done,
        None => PageIn::Full,
    }
}

/// A page fault at `lin` in the running process: if it is on a page of a
/// view of a file not read yet, read it, making room for it when the pool
/// is low, and the program goes on. Whether it was one.
pub fn fault_in(cpu: &mut Cpu, lin: u32) -> bool {
    let running = cpu.bus.win32.running;
    let ours = cpu
        .bus
        .win32
        .processes
        .get(running)
        .is_some_and(|p| cpu.cr3 != 0 && p.memory.cr3 == cpu.cr3);
    ours && fault_in_process(cpu, running, lin)
}

/// `fault_in` for the process at `index` in the list, running or not:
/// for the system's work on a view's pages through the page tables
/// (VirtualProtect, ReadProcessMemory). Every process must be in the
/// list.
pub fn fault_in_process(cpu: &mut Cpu, index: usize, lin: u32) -> bool {
    let done = read_making_room(&mut cpu.bus, index, lin);
    // Reading it, or making room for it, may have sent pages out or
    // cleared accessed bits, whether or not it came in.
    if std::mem::take(&mut cpu.bus.win32.tlb_stale) {
        cpu.tlb.flush();
    } else if done && index == cpu.bus.win32.running {
        cpu.tlb.flush_page(lin);
    }
    done
}

/// Write `data` at `lin` in the memory of the process `pid`, through its
/// own page tables, whichever process runs and whether or not a service
/// is running: for work the system finishes later, outside the call that
/// asked for it (a wave block's flags, an ASPI command's results). A page
/// of a view of a file that isn't in memory, not read yet or given back
/// to the pool, is read in first, as the program's own write would read
/// it. Bytes on pages the process hasn't mapped are left out.
pub fn write_process_bytes(cpu: &mut Cpu, pid: u32, lin: u32, data: &[u8]) {
    let Some(p) = cpu.bus.win32.processes.iter().position(|p| p.id == pid) else {
        return;
    };
    let memory = memory::Memory::tables(cpu.bus.win32.processes[p].memory.cr3);
    for (offset, run) in Bus::page_runs(lin, data.len()) {
        let at = lin.wrapping_add(offset as u32);
        let part = &data[offset..offset + run];
        if memory.write(&mut cpu.bus, at, part).is_none() && fault_in_process(cpu, p, at) {
            let _ = memory.write(&mut cpu.bus, at, part);
        }
    }
}

/// `fault_in_process` without the TLB, which may be stale after it
/// (`Win32::tlb_stale`).
fn read_making_room(bus: &mut Bus, index: usize, lin: u32) -> bool {
    let mut keep = reserve(bus);
    // A page and its page table may each need room; a few rounds are
    // plenty, and stop a page evicted again before it is mapped from
    // going round for ever.
    let mut swept = false;
    for _ in 0..4 {
        match read_in(bus, index, lin, keep) {
            PageIn::Done => return true,
            PageIn::No => return false,
            PageIn::Full => {
                // Pages not used since the last sweep go first, a batch of
                // them (`sweep`), but one sweep a fault: a second would find
                // the pages the first left unused. After that, and once the
                // reserve is used up, a page of a program's own memory goes
                // out, and only with none of those, any view's page that
                // isn't held for the instruction (`memory::hold`).
                let left = bus.win32.pool.as_ref().map_or(0, |p| p.available() / PAGE);
                if !std::mem::replace(&mut swept, true) && sweep(bus) {
                    bus.win32.tlb_stale = true;
                } else if keep == 0 || left == 0 {
                    if !memory::page_out(bus) && !evict(bus, true) {
                        memory::out_of_memory(bus, lin);
                        return false;
                    }
                    bus.win32.tlb_stale = true;
                }
                // With what room evicting made, or with nothing left to
                // evict, the page may take from the reserve.
                keep = 0;
            }
        }
    }
    false
}

/// Give back to the pool pages of mappings of files that no process has
/// touched since the last call (their entries' accessed bits, which it
/// clears): the pages no view wrote first, which are read from the file
/// again when they are needed, then the written ones, each kind in
/// address order from where the last call stopped
/// (`PageFile::view_hand`). As the `last_resort`, once no page of a
/// program's own memory can go to the page file, touched pages may go
/// too, after the others. A written page is written to the file first
/// when the mapping may write it (PAGE_READWRITE), and stays if that
/// fails; one of a copy-on-write mapping (PAGE_WRITECOPY) goes to the
/// page file, as it does every time it leaves after that
/// (`Section::copies`), and stays while the page file is full. A call
/// gives back a batch (`EVICTED`, or a share of the pages, `EVICTED_SHARE`)
/// so that the scan it makes of the mappings' pages serves many faults.
/// Pages stay that a process gave another protection (VirtualProtect),
/// which reading them again would undo, pages a wave device of a process
/// with a view plays from, which the device reads without a page fault,
/// and pages held for the instruction that faults (`memory::hold`).
/// Whether it gave any back. Every process must be in the list, and the
/// TLB is the caller's to flush.
pub fn evict(bus: &mut Bus, last_resort: bool) -> bool {
    // The candidates: a page, where the processes that have it map it, and
    // whether one of them wrote it.
    let mut unused = Vec::new();
    let mut used = Vec::new();
    for (s, section) in bus.win32.sections.iter().enumerate() {
        let Some(section) = section else {
            continue;
        };
        if section.file.is_none() || section.protect == PAGE_NOACCESS {
            continue;
        }
        let viewers: Vec<(u32, &memory::Memory, Option<u32>)> = bus
            .win32
            .processes
            .iter()
            .filter(|p| section.viewed_by(p.id))
            .map(|p| (p.id, &p.memory, section.protection_in(p.id)))
            .collect();
        for (i, &phys) in section.pages.iter().enumerate() {
            if phys == 0 {
                continue;
            }
            let lin = section.base + i as u32 * PAGE;
            let entries: Vec<(usize, Option<u32>)> = viewers
                .iter()
                .filter_map(|&(_, m, protect)| Some((m.entry_at(bus, lin)?, protect)))
                .filter(|&(at, _)| bus.read_32(at) & !0xFFF == phys)
                .collect();
            let changed = entries
                .iter()
                .any(|&(at, protect)| protection(bus.read_32(at)) != protect);
            let playing = viewers
                .iter()
                .any(|&(pid, _, _)| crate::win32::winmm::wave::plays_from(bus, pid, lin, PAGE));
            if changed || playing || memory::held(bus, phys) {
                continue;
            }
            let entries: Vec<usize> = entries.into_iter().map(|(at, _)| at).collect();
            let wrote = section.copies.contains_key(&(i as u32))
                || entries.iter().any(|&at| bus.read_32(at) & DIRTY != 0);
            let list = if entries.iter().any(|&at| bus.read_32(at) & ACCESSED != 0) {
                &mut used
            } else {
                &mut unused
            };
            list.push((lin, s, i, entries, wrote));
        }
    }
    // With the bits cleared the TLB is stale (`memory::page_out_unused`).
    for (_, _, _, entries, _) in &used {
        for &at in entries {
            let entry = bus.read_32(at);
            bus.write_32(at, entry & !ACCESSED);
        }
    }
    if !used.is_empty() {
        bus.win32.tlb_stale = true;
    }
    let count = ((unused.len() + used.len()) / EVICTED_SHARE).max(EVICTED);
    if !last_resort {
        used.clear();
    }
    // Each list's pages no view wrote first, then the written ones, each
    // from the hand on, then from the start up to the hand.
    let hand = bus.win32.pool.as_ref().map_or(0, |p| p.page_file.view_hand);
    unused.sort_by_key(|&(lin, _, _, _, wrote)| (wrote, lin <= hand, lin));
    used.sort_by_key(|&(lin, _, _, _, wrote)| (wrote, lin <= hand, lin));
    bus.win32.paging.sweeps += 1;
    let mut evicted = 0;
    for (lin, s, i, entries, wrote) in unused.into_iter().chain(used) {
        if evicted == count {
            break;
        }
        if wrote && !save_written(bus, s, i) {
            continue;
        }
        for at in entries {
            bus.write_32(at, 0);
        }
        let pages = &mut bus.win32.sections[s].as_mut().expect("found").pages;
        let phys = std::mem::take(&mut pages[i]);
        memory::free_page(bus, phys);
        if let Some(pool) = bus.win32.pool.as_mut() {
            pool.page_file.view_hand = lin;
        }
        bus.win32.paging.evicted += 1;
        evicted += 1;
    }
    evicted > 0
}

/// Keep what a view wrote to the page at index `i` of the section `s`,
/// which is leaving the pool: in the file for a mapping that may write
/// it, in the page file for a copy-on-write one. Whether it was kept.
fn save_written(bus: &mut Bus, s: usize, i: usize) -> bool {
    let section = bus.win32.sections[s].as_ref().expect("found");
    let (Some((_, sft)), protect, size) = (section.file, section.protect, section.size) else {
        return false;
    };
    let phys = section.pages[i];
    let offset = i as u32 * PAGE;
    match protect {
        PAGE_READWRITE => {
            let len = (size - offset).min(PAGE) as usize;
            let mut data = vec![0; len];
            bus.read_into(phys as usize, &mut data);
            write_file_at(bus, sft, offset, &data)
        }
        PAGE_WRITECOPY => {
            let mut data = vec![0; PAGE as usize];
            bus.read_into(phys as usize, &mut data);
            let Some(pool) = bus.win32.pool.as_mut() else {
                return false;
            };
            let Some(slot) = pool.page_file.keep(data) else {
                return false;
            };
            let section = bus.win32.sections[s].as_mut().expect("found");
            section.copies.insert(i as u32, Some(slot));
            bus.win32.paging.outs += 1;
            true
        }
        _ => false,
    }
}

/// Write `data` at `offset` of DOS's open file `sft`, leaving its position
/// as it was. Whether all of it was written.
pub(crate) fn write_file_at(bus: &mut Bus, sft: u16, offset: u32, data: &[u8]) -> bool {
    let disk = &mut bus.disk;
    let Ok(position) = disk.seek_file(sft, 0, 1) else {
        return false;
    };
    let written = disk.seek_file(sft, offset as i64, 0).is_ok()
        && disk
            .write_file(sft, data)
            .is_ok_and(|n| n as usize == data.len());
    let _ = disk.seek_file(sft, position as i64, 0);
    written
}

crate::state_fields!(Section {
    object,
    base,
    size,
    protect,
    reserve,
    pages,
    copies,
    file,
    views
});
