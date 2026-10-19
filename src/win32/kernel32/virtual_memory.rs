//! Virtual memory (VirtualAlloc and its family), the IsBad*Ptr checks,
//! KERNEL32's memory copying functions and GetSystemInfo.
//!
//! A region is reserved at a multiple of 64 KB, a whole number of pages
//! long, in the private arena (4 MB to 2 GB). Its pages are committed
//! one by one, each with a protection (a PAGE_ value) that its page table
//! entry keeps: PAGE_NOACCESS pages have their physical page in an entry
//! that isn't present, so touching them faults. Where Windows 95's answers
//! differ from NT's these are Windows 95's: VirtualQuery says
//! PAGE_NOACCESS for reserved pages, a size of 0 is ERROR_NOT_ENOUGH_MEMORY,
//! MEM_RESET and PAGE_GUARD aren't there (ERROR_INVALID_PARAMETER), and so
//! is protecting pages that aren't committed.

use super::{ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY, FALSE, TRUE, set_error};
use crate::bus::Bus;
use crate::cpu::{Cpu, CpuModel};
use crate::win32::memory::{
    GRANULARITY, MEM_MAPPED, MEM_PRIVATE, Memory, PAGE, PAGE_NOACCESS, PAGE_READWRITE, PRESENT,
    PRIVATE, SHARED, SYSTEM, USER, WRITABLE, protection,
};
use crate::win32::object::{self, Kind};
use crate::win32::{Process, running};

/// VirtualAlloc's and VirtualFree's types.
const MEM_COMMIT: u32 = 0x1000;
const MEM_RESERVE: u32 = 0x2000;
const MEM_DECOMMIT: u32 = 0x4000;
const MEM_RELEASE: u32 = 0x8000;
const MEM_FREE: u32 = 0x1_0000;
const MEM_RESET: u32 = 0x8_0000;
const MEM_TOP_DOWN: u32 = 0x10_0000;
/// The modifier of a protection Windows 95 takes (and ignores).
const PAGE_NOCACHE: u32 = 0x200;

const ERROR_INVALID_ADDRESS: u32 = 487;
const ERROR_BAD_LENGTH: u32 = 24;

/// The size of a MEMORY_BASIC_INFORMATION and a SYSTEM_INFO.
const MEMORY_BASIC_INFORMATION: u32 = 28;
const SYSTEM_INFO: u32 = 36;

/// Whether `protect` is a protection VirtualAlloc and VirtualProtect
/// take: one PAGE_ value, perhaps with PAGE_NOCACHE.
pub(super) fn valid_protection(protect: u32) -> bool {
    (protect & 0xFF).count_ones() == 1 && protect & !(0xFF | PAGE_NOCACHE) == 0
}

/// The pages from `at` to `at + len` (rounded out to whole pages), as
/// (first, end); None when that wraps past 4 GB.
fn page_range(at: u32, len: u32) -> Option<(u32, u32)> {
    let end = (at as u64 + len as u64).next_multiple_of(PAGE as u64);
    (end <= u32::MAX as u64 + 1 - PAGE as u64).then_some((at & !(PAGE - 1), end as u32))
}

/// Drop the running thread's translations of the pages from `first` to
/// `end`, whose entries changed.
fn flush(cpu: &mut Cpu, first: u32, end: u32) {
    if end - first > 64 * PAGE {
        cpu.tlb.flush();
    } else {
        for page in (first..end).step_by(PAGE as usize) {
            cpu.tlb.flush_page(page);
        }
    }
}

/// Commit the pages from `first` to `end` with the protection `protect`;
/// those committed already get it. With no memory for them, the pages it
/// committed go again and it fails.
fn commit(memory: &mut Memory, bus: &mut Bus, first: u32, end: u32, protect: u32) -> bool {
    let mut new = Vec::new();
    for page in (first..end).step_by(PAGE as usize) {
        if memory.set_protection(bus, page, protect) {
            continue;
        }
        if memory.commit_page(bus, page, protect).is_none() {
            for page in new {
                memory.unmap(bus, page);
            }
            return false;
        }
        new.push(page);
    }
    true
}

/// VirtualAlloc(address, size, type, protect): reserve a region
/// (MEM_RESERVE, or MEM_COMMIT with no address), commit pages of one
/// (MEM_COMMIT), or both. The region's or the pages' address.
pub(super) fn virtual_alloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, size, kind, protect] = args else {
        unreachable!()
    };
    let error = if kind & MEM_RESET != 0
        || kind & !(MEM_COMMIT | MEM_RESERVE | MEM_TOP_DOWN) != 0
        || kind & (MEM_COMMIT | MEM_RESERVE) == 0
        || !valid_protection(protect)
    {
        Some(ERROR_INVALID_PARAMETER)
    } else if size == 0 {
        Some(ERROR_NOT_ENOUGH_MEMORY)
    } else {
        None
    };
    if let Some(error) = error {
        set_error(cpu, error);
        return Some(0);
    }
    let Some((first, end)) = page_range(at, size) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    let protect = protect & 0xFF;
    if kind & MEM_RESERVE == 0 && at != 0 {
        let region = crate::win32::process(cpu).memory.region(first).cloned();
        if region.is_some_and(|r| r.kind == MEM_MAPPED) {
            // A view of a mapping made with SEC_RESERVE.
            let committed = super::mapping::commit(cpu, first, end, protect);
            return Some(committed.unwrap_or_else(|error| {
                set_error(cpu, error);
                0
            }));
        }
    }
    let result = crate::win32::with_process(cpu, |p, bus| {
        if kind & MEM_RESERVE != 0 || at == 0 {
            let (place, len) = match at {
                0 => (None, end - first),
                _ => (
                    Some(at & !(GRANULARITY - 1)),
                    end - (at & !(GRANULARITY - 1)),
                ),
            };
            let Some(start) =
                p.memory
                    .reserve_region(place, len, PRIVATE, SHARED, protect, MEM_PRIVATE)
            else {
                return Err(if at == 0 {
                    ERROR_NOT_ENOUGH_MEMORY
                } else {
                    ERROR_INVALID_ADDRESS
                });
            };
            if kind & MEM_COMMIT != 0 && !commit(&mut p.memory, bus, start, start + len, protect) {
                p.memory.unreserve(start);
                return Err(ERROR_NOT_ENOUGH_MEMORY);
            }
            return Ok(start);
        }
        match p.memory.region(first) {
            Some(r) if end <= r.end() => {}
            _ => return Err(ERROR_INVALID_ADDRESS),
        }
        if !commit(&mut p.memory, bus, first, end, protect) {
            return Err(ERROR_NOT_ENOUGH_MEMORY);
        }
        Ok(first)
    });
    match result {
        Ok(address) => {
            if kind & MEM_RESERVE == 0 && at != 0 {
                // (Pages committed before may have a new protection.)
                flush(cpu, first, end);
            }
            Some(address)
        }
        Err(error) => {
            set_error(cpu, error);
            Some(0)
        }
    }
}

/// VirtualFree(address, size, type): decommit pages (MEM_DECOMMIT, with a
/// size of 0 to the region's end), or release the region starting at
/// `address` (MEM_RELEASE, with a size of 0) and its pages.
pub(super) fn virtual_free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, size, kind] = args else {
        unreachable!()
    };
    let result = crate::win32::with_process(cpu, |p, bus| {
        let region = p.memory.region(at).cloned().ok_or(ERROR_INVALID_ADDRESS)?;
        if region.kind != MEM_PRIVATE {
            return Err(ERROR_INVALID_PARAMETER);
        }
        let (first, end) = match kind {
            MEM_RELEASE if size != 0 => return Err(ERROR_INVALID_PARAMETER),
            MEM_RELEASE if at != region.start => return Err(ERROR_INVALID_ADDRESS),
            MEM_RELEASE => (region.start, region.end()),
            MEM_DECOMMIT if size == 0 => (at & !(PAGE - 1), region.end()),
            MEM_DECOMMIT => match page_range(at, size) {
                Some((first, end)) if end <= region.end() => (first, end),
                _ => return Err(ERROR_INVALID_PARAMETER),
            },
            _ => return Err(ERROR_INVALID_PARAMETER),
        };
        for page in (first..end).step_by(PAGE as usize) {
            p.memory.unmap(bus, page);
        }
        if kind == MEM_RELEASE {
            p.memory.unreserve(region.start);
        }
        Ok((first, end))
    });
    match result {
        Ok((first, end)) => {
            flush(cpu, first, end);
            Some(TRUE)
        }
        Err(error) => {
            set_error(cpu, error);
            Some(FALSE)
        }
    }
}

/// The place of the process a handle names, if it runs.
fn process_of(cpu: &mut Cpu, handle: u32) -> Option<usize> {
    let Some(Kind::Process { id, .. }) = object::kind_of(cpu, handle) else {
        return None;
    };
    cpu.bus.win32.processes.iter().position(|p| p.id == id)
}

/// Run `f` with the process at `index` and the bus.
fn with_process_at<T>(
    cpu: &mut Cpu,
    index: usize,
    f: impl FnOnce(&mut Process, &mut Bus) -> T,
) -> T {
    let mut process = std::mem::take(&mut cpu.bus.win32.processes[index]);
    let result = f(&mut process, &mut cpu.bus);
    cpu.bus.win32.processes[index] = process;
    result
}

/// VirtualProtect's work on the process at `index`: give the committed
/// pages from `at` to `at + size` the protection `protect`, the old one
/// (the first page's) written to `old`.
fn protect(cpu: &mut Cpu, index: usize, at: u32, size: u32, protect: u32, old: u32) -> Option<u32> {
    let range = page_range(at, size.max(1));
    let found = with_process_at(cpu, index, |p, bus| {
        let (first, end) = range?;
        let region = p.memory.region(first)?;
        if end > region.end() {
            return None;
        }
        let entries: Vec<u32> = (first..end)
            .step_by(PAGE as usize)
            .map(|page| p.memory.entry(bus, page).unwrap_or(0))
            .collect();
        if entries.contains(&0) {
            return None;
        }
        protection(entries[0])
    });
    if !valid_protection(protect) || old == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let Some(previous) = found else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    };
    cpu.bus.guest_write_32(old, previous);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (first, end) = range.expect("checked");
    with_process_at(cpu, index, |p, bus| {
        for page in (first..end).step_by(PAGE as usize) {
            p.memory.set_protection(bus, page, protect & 0xFF);
        }
    });
    if index == cpu.bus.win32.running {
        flush(cpu, first, end);
    }
    Some(TRUE)
}

/// VirtualProtect(address, size, protect, old).
pub(super) fn virtual_protect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, size, new, old] = args else {
        unreachable!()
    };
    let index = cpu.bus.win32.running;
    protect(cpu, index, at, size, new, old)
}

/// VirtualProtectEx(process, address, size, protect, old).
pub(super) fn virtual_protect_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, at, size, new, old] = args else {
        unreachable!()
    };
    let Some(index) = process_of(cpu, handle) else {
        set_error(cpu, super::ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    protect(cpu, index, at, size, new, old)
}

/// What VirtualQuery says of `at` in `memory`: the MEMORY_BASIC_INFORMATION
/// of the run of pages from its page with the same state and protection.
fn query(memory: &Memory, bus: &Bus, at: u32) -> [u32; 7] {
    let page = at & !(PAGE - 1);
    let Some(region) = memory.region(page) else {
        // Free: up to the next region, or the end of the arena.
        let arena_end = [PRIVATE, SHARED, SYSTEM]
            .into_iter()
            .find(|&end| end > page)
            .unwrap_or(SYSTEM);
        let end = memory
            .regions()
            .iter()
            .map(|r| r.start)
            .find(|&start| start > page)
            .map_or(arena_end, |start| start.min(arena_end));
        return [page, 0, 0, end - page, MEM_FREE, PAGE_NOACCESS, 0];
    };
    let state = |page: u32| protection(memory.entry(bus, page).unwrap_or(0));
    let first = state(page);
    let mut end = page + PAGE;
    while end < region.end() && state(end) == first {
        end += PAGE;
    }
    // Windows 95's views of mappings are memory of the shared arena
    // reserved with PAGE_NOACCESS.
    let (allocation, kind) = match (region.kind, region.protect) {
        (MEM_MAPPED, _) => (PAGE_NOACCESS, MEM_PRIVATE),
        (kind, 0) => (PAGE_READWRITE, kind),
        (kind, protect) => (protect, kind),
    };
    let (state, protect) = match first {
        Some(protect) => (MEM_COMMIT, protect),
        None => (MEM_RESERVE, PAGE_NOACCESS),
    };
    [
        page,
        region.start,
        allocation,
        end - page,
        state,
        protect,
        kind,
    ]
}

/// VirtualQuery's work on the process at `index`.
fn query_into(cpu: &mut Cpu, index: usize, at: u32, buffer: u32, len: u32) -> Option<u32> {
    if len < MEMORY_BASIC_INFORMATION {
        set_error(cpu, ERROR_BAD_LENGTH);
        return Some(0);
    }
    if at >= SYSTEM {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let info = with_process_at(cpu, index, |p, bus| query(&p.memory, bus, at));
    for (i, value) in info.into_iter().enumerate() {
        cpu.bus
            .guest_write_32(buffer.wrapping_add(4 * i as u32), value);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(MEMORY_BASIC_INFORMATION)
}

/// VirtualQuery(address, info, length): a MEMORY_BASIC_INFORMATION of the
/// pages from `address` on that are alike; its size.
pub(super) fn virtual_query(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, buffer, len] = args else {
        unreachable!()
    };
    let index = cpu.bus.win32.running;
    query_into(cpu, index, at, buffer, len)
}

/// VirtualQueryEx(process, address, info, length).
pub(super) fn virtual_query_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, at, buffer, len] = args else {
        unreachable!()
    };
    let Some(index) = process_of(cpu, handle) else {
        set_error(cpu, super::ERROR_INVALID_HANDLE);
        return Some(0);
    };
    query_into(cpu, index, at, buffer, len)
}

/// VirtualLock and VirtualUnlock: pages are never swapped out, so there
/// is nothing to do.
pub(super) fn virtual_lock(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(TRUE)
}

/// Whether the program may read (or with `write` write) the page at
/// `page`, by its page table entry: no page fault is recorded.
fn page_allows(cpu: &Cpu, page: u32, write: bool) -> bool {
    let Some(process) = running(cpu) else {
        return false;
    };
    let entry = process.memory.entry(&cpu.bus, page).unwrap_or(0);
    let needed = PRESENT | USER | if write { WRITABLE } else { 0 };
    entry & needed == needed
}

/// Whether the program may reach the `len` bytes at `at`.
pub(super) fn allows(cpu: &Cpu, at: u32, len: u32, write: bool) -> bool {
    if len == 0 {
        return true;
    }
    let Some(last) = at.checked_add(len - 1) else {
        return false;
    };
    (at & !(PAGE - 1)..=last)
        .step_by(PAGE as usize)
        .all(|page| page_allows(cpu, page, write))
}

/// IsBadReadPtr(address, size) and IsBadHugeReadPtr: whether a byte of
/// it can't be read.
pub(super) fn is_bad_read_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(!allows(cpu, args[0], args[1], false) as u32)
}

/// IsBadWritePtr(address, size) and IsBadHugeWritePtr.
pub(super) fn is_bad_write_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(!allows(cpu, args[0], args[1], true) as u32)
}

/// IsBadCodePtr(address): whether its byte can't be read.
pub(super) fn is_bad_code_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(!allows(cpu, args[0], 1, false) as u32)
}

/// Whether a string of `unit`-byte characters at `at`, up to its 0 or
/// `max` characters, can't be read.
fn bad_string(cpu: &mut Cpu, at: u32, max: u32, unit: u32) -> u32 {
    let Some(process) = running(cpu) else {
        return TRUE;
    };
    for i in 0..max {
        let Some(char_at) = at.checked_add(i * unit) else {
            return TRUE;
        };
        let mut value = 0;
        for byte in 0..unit {
            let Some(lin) = char_at.checked_add(byte) else {
                return TRUE;
            };
            if !page_allows(cpu, lin & !(PAGE - 1), false) {
                return TRUE;
            }
            let phys = process.memory.translate(&cpu.bus, lin).expect("present");
            value |= (cpu.bus.read_8(phys as usize) as u32) << (8 * byte);
        }
        if value == 0 {
            break;
        }
    }
    FALSE
}

/// IsBadStringPtrA(address, max).
pub(super) fn is_bad_string_ptr_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(bad_string(cpu, args[0], args[1], 1))
}

/// IsBadStringPtrW(address, max), which Windows 95 has.
pub(super) fn is_bad_string_ptr_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(bad_string(cpu, args[0], args[1], 2))
}

/// RtlMoveMemory(to, from, length): a copy that may overlap.
pub(super) fn rtl_move_memory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[to, from, len] = args else {
        unreachable!()
    };
    let mut data = vec![0; len as usize];
    cpu.bus.guest_read_bytes(from, &mut data);
    if cpu.bus.guest_faulted() {
        return None;
    }
    cpu.bus.guest_write_bytes(to, &data);
    Some(0)
}

/// RtlFillMemory(to, length, byte).
pub(super) fn rtl_fill_memory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[to, len, fill] = args else {
        unreachable!()
    };
    cpu.bus.guest_fill(to, len as usize, fill as u8);
    Some(0)
}

/// RtlZeroMemory(to, length).
pub(super) fn rtl_zero_memory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[to, len] = args else { unreachable!() };
    cpu.bus.guest_fill(to, len as usize, 0);
    Some(0)
}

/// GetSystemInfo(info): one Intel processor of the type the machine has
/// (386, 486 or, for a Pentium and later, 586: Windows 95 leaves the
/// level and revision 0), 4 KB pages, 64 KB granularity and the private
/// arena's addresses.
pub(super) fn get_system_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    let processor = match cpu.model {
        CpuModel::I386 => 386,
        CpuModel::I486 => 486,
        _ => 586,
    };
    let fields = [
        0, // PROCESSOR_ARCHITECTURE_INTEL
        PAGE,
        PRIVATE,
        SHARED - 1,
        1,
        1,
        processor,
        GRANULARITY,
        0, // wProcessorLevel and wProcessorRevision
    ];
    debug_assert_eq!(fields.len() as u32 * 4, SYSTEM_INFO);
    for (i, value) in fields.into_iter().enumerate() {
        cpu.bus.guest_write_32(at.wrapping_add(4 * i as u32), value);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(0)
}
