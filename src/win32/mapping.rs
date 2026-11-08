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
//! view (`page_in`, from the bus's accessors). The pages of a read-only
//! mapping are the file's, so when the pool runs low the ones not used
//! lately go back to it (`evict`), to be read again when they are needed.
//! Pages read from files leave the last part of the pool (`RESERVE`) to
//! the rest of the system, which can't make room by evicting them: once
//! the pool is down to it, a page fault on a view evicts before it reads.
//! A mapping of the swap file has all its pages from the start, or with
//! SEC_RESERVE those VirtualAlloc commits: nothing else holds their
//! contents.

use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::win32::memory::{
    self, PAGE, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE, protection, protection_bits,
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
/// many of its pages. Other allocations (VirtualAlloc, heaps, stacks,
/// page tables) take pages from the pool with no way to evict, so pages
/// read from files that filled it would make them fail.
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
    /// mapping of a file where the page wasn't read from it yet.
    pub pages: Vec<u32>,
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
        let offset = i as u32 * PAGE;
        let len = (size - offset).min(PAGE) as usize;
        // A file that can't be read any more leaves the page zeroed.
        if let Some(data) = crate::interrupts::int21::read_file_at(bus, sft, offset, len) {
            bus.load_bytes(new as usize, &data);
        }
        bus.win32.sections[s].as_mut().expect("found").pages[i] = new;
        phys = new;
    }
    let mut process = std::mem::take(&mut bus.win32.processes[index]);
    let mapped = process
        .memory
        .set_entry(bus, page, phys | protection_bits(protect));
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
    let mut keep = reserve(&cpu.bus);
    // A page and its page table may each need room; a few rounds are
    // plenty, and stop a page evicted again before it is mapped from
    // going round for ever.
    for _ in 0..4 {
        match read_in(&mut cpu.bus, index, lin, keep) {
            PageIn::Done => {
                if index == cpu.bus.win32.running {
                    cpu.tlb.flush_page(lin);
                }
                return true;
            }
            PageIn::No => return false,
            PageIn::Full => {
                if evict(&mut cpu.bus) {
                    cpu.tlb.flush();
                } else if keep == 0 {
                    return false;
                }
                // With what room evicting made, or with nothing left to
                // evict, the page may take from the reserve.
                keep = 0;
            }
        }
    }
    false
}

/// Give back to the pool pages of read-only mappings of files, which can
/// be read from the file again, choosing those no process has touched
/// since the last time (their entries' accessed bits, which it clears),
/// then the first of those touched. Pages stay that a process gave
/// another protection (VirtualProtect) or wrote, which reading them again
/// would undo, and pages a wave device of a process with a view plays
/// from, which the device reads without a page fault. Whether it gave any
/// back. Every process must be in the list, and the TLB is the caller's
/// to flush.
pub fn evict(bus: &mut Bus) -> bool {
    // The candidates: a page, and where the processes that have it map it.
    let mut unused = Vec::new();
    let mut used = Vec::new();
    for (s, section) in bus.win32.sections.iter().enumerate() {
        let Some(section) = section else {
            continue;
        };
        if section.file.is_none() || section.protect != PAGE_READONLY {
            continue;
        }
        let viewers: Vec<(u32, &memory::Memory)> = bus
            .win32
            .processes
            .iter()
            .filter(|p| section.viewed_by(p.id))
            .map(|p| (p.id, &p.memory))
            .collect();
        for (i, &phys) in section.pages.iter().enumerate() {
            if phys == 0 {
                continue;
            }
            let lin = section.base + i as u32 * PAGE;
            let entries: Vec<usize> = viewers
                .iter()
                .filter_map(|(_, m)| m.entry_at(bus, lin))
                .filter(|&at| bus.read_32(at) & !0xFFF == phys)
                .collect();
            let changed = entries.iter().any(|&at| {
                let entry = bus.read_32(at);
                entry & DIRTY != 0 || protection(entry) != Some(PAGE_READONLY)
            });
            let playing = viewers
                .iter()
                .any(|&(pid, _)| crate::win32::winmm::wave::plays_from(bus, pid, lin, PAGE));
            if changed || playing {
                continue;
            }
            let list = if entries.iter().any(|&at| bus.read_32(at) & ACCESSED != 0) {
                &mut used
            } else {
                &mut unused
            };
            list.push((s, i, entries));
        }
    }
    for (_, _, entries) in &used {
        for &at in entries {
            let entry = bus.read_32(at);
            bus.write_32(at, entry & !ACCESSED);
        }
    }
    let count = ((unused.len() + used.len()) / EVICTED_SHARE).max(EVICTED);
    let mut victims = unused.into_iter().chain(used).take(count).peekable();
    if victims.peek().is_none() {
        return false;
    }
    for (s, i, entries) in victims {
        for at in entries {
            bus.write_32(at, 0);
        }
        let pages = &mut bus.win32.sections[s].as_mut().expect("found").pages;
        let phys = std::mem::take(&mut pages[i]);
        memory::free_page(bus, phys);
    }
    true
}

crate::state_fields!(Section {
    object,
    base,
    size,
    protect,
    reserve,
    pages,
    file,
    views
});
