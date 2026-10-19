//! GlobalAlloc, LocalAlloc and their families, which are the same
//! functions in Win32: blocks of the process heap. A fixed block's handle
//! is its address. A moveable block's is the address of a pointer to it,
//! in a small block of the heap of its own (so that it is never a block's
//! address: it is 4 past a multiple of 8), and the block moves when it is
//! reallocated while it isn't locked. Where Windows 95's answers differ
//! from NT's, these are Windows 95's: an unlock that leaves a block
//! unlocked, or of a fixed block, leaves the last error as it was, and a
//! handle that isn't one is ERROR_INVALID_PARAMETER.

use super::memory::{self, HEAP_REALLOC_IN_PLACE_ONLY, HEAP_ZERO_MEMORY};
use super::{ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, TRUE, set_error};
use crate::cpu::Cpu;
use crate::win32::heap::Moveable;
use crate::win32::process;

/// GlobalAlloc's flags (LocalAlloc's LMEM_ ones have the same values;
/// its LMEM_DISCARDABLE, F00h, has GMEM_DISCARDABLE's bit in it).
const GMEM_MOVEABLE: u32 = 0x0002;
const GMEM_ZEROINIT: u32 = 0x0040;
const GMEM_MODIFY: u32 = 0x0080;
const GMEM_DISCARDABLE: u32 = 0x0100;
/// GlobalFlags' answers besides the lock count.
const GMEM_DISCARDED: u32 = 0x4000;
const GMEM_INVALID_HANDLE: u32 = 0x8000;
/// A moveable block's lock count stops here, as Windows' byte does.
const MAX_LOCKS: u32 = 0xFF;

/// Whether `handle` has the form of a moveable block's handle.
fn is_moveable(handle: u32) -> bool {
    handle & 7 == 4
}

fn heap(cpu: &mut Cpu) -> u32 {
    process(cpu).heap.base
}

/// The index of the moveable block `handle`, if there is one.
fn moveable(cpu: &mut Cpu, handle: u32) -> Option<usize> {
    process(cpu)
        .moveables
        .iter()
        .position(|m| m.handle == handle)
}

/// Point the handle of the moveable block `i` at its block.
fn update(cpu: &mut Cpu, i: usize) {
    let m = &process(cpu).moveables[i];
    let (handle, block) = (m.handle, m.block);
    cpu.bus.guest_write_32(handle, block);
}

/// GlobalAlloc(flags, size) and LocalAlloc: a fixed block, or with
/// GMEM_MOVEABLE the handle of a moveable one (with no block yet when
/// `size` is 0). GMEM_ZEROINIT zeroes it.
pub(super) fn global_alloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[flags, size] = args else { unreachable!() };
    let heap = heap(cpu);
    let zero = if flags & GMEM_ZEROINIT != 0 {
        HEAP_ZERO_MEMORY
    } else {
        0
    };
    if flags & GMEM_MOVEABLE == 0 {
        return Some(memory::alloc(cpu, heap, zero, size).unwrap_or_else(|error| {
            set_error(cpu, error);
            0
        }));
    }
    let entry = match memory::alloc(cpu, heap, HEAP_ZERO_MEMORY, 8) {
        Ok(entry) => entry,
        Err(error) => {
            set_error(cpu, error);
            return Some(0);
        }
    };
    let block = match size {
        0 => 0,
        _ => match memory::alloc(cpu, heap, zero, size) {
            Ok(block) => block,
            Err(error) => {
                let _ = memory::free(cpu, heap, entry);
                set_error(cpu, error);
                return Some(0);
            }
        },
    };
    let handle = entry + 4;
    let moveables = &mut process(cpu).moveables;
    moveables.push(Moveable {
        handle,
        block,
        flags: flags & GMEM_DISCARDABLE,
        locks: 0,
    });
    let i = moveables.len() - 1;
    update(cpu, i);
    Some(handle)
}

/// GlobalFree(handle) and LocalFree: NULL, or the handle when it is none.
pub(super) fn global_free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let handle = args[0];
    if handle == 0 {
        return Some(0);
    }
    let heap = heap(cpu);
    if is_moveable(handle) {
        let Some(i) = moveable(cpu, handle) else {
            set_error(cpu, ERROR_INVALID_HANDLE);
            return Some(handle);
        };
        let m = process(cpu).moveables.remove(i);
        if m.block != 0 {
            let _ = memory::free(cpu, heap, m.block);
        }
        let _ = memory::free(cpu, heap, handle - 4);
        return Some(0);
    }
    match memory::free(cpu, heap, handle) {
        Ok(()) => Some(0),
        Err(_) => {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            Some(handle)
        }
    }
}

/// GlobalReAlloc(handle, size, flags) and LocalReAlloc. With GMEM_MODIFY
/// only a moveable block's GMEM_DISCARDABLE changes (a fixed block can't
/// be made moveable on Windows 95). Otherwise the block gets `size`
/// bytes, moving if GMEM_MOVEABLE allows it or it is a moveable block
/// that isn't locked; a moveable block that isn't locked is discarded
/// with a size of 0. GMEM_ZEROINIT zeroes what it gains.
pub(super) fn global_realloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, size, flags] = args else {
        unreachable!()
    };
    let heap = heap(cpu);
    let zero = if flags & GMEM_ZEROINIT != 0 {
        HEAP_ZERO_MEMORY
    } else {
        0
    };
    if handle == 0 {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    }
    if !is_moveable(handle) {
        if flags & GMEM_MODIFY != 0 {
            if flags & GMEM_MOVEABLE != 0 {
                set_error(cpu, ERROR_INVALID_PARAMETER);
                return Some(0);
            }
            return Some(handle);
        }
        let place = if flags & GMEM_MOVEABLE != 0 {
            0
        } else {
            HEAP_REALLOC_IN_PLACE_ONLY
        };
        return Some(
            memory::realloc(cpu, heap, zero | place, handle, size).unwrap_or_else(|error| {
                set_error(cpu, error);
                0
            }),
        );
    }
    let Some(i) = moveable(cpu, handle) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    let m = process(cpu).moveables[i].clone();
    if flags & GMEM_MODIFY != 0 {
        let modified = &mut process(cpu).moveables[i];
        modified.flags = flags & GMEM_DISCARDABLE;
        return Some(handle);
    }
    let block = if size == 0 {
        if m.locks > 0 {
            return Some(0);
        }
        if m.block != 0 {
            let _ = memory::free(cpu, heap, m.block);
        }
        0
    } else if m.block == 0 {
        match memory::alloc(cpu, heap, zero, size) {
            Ok(block) => block,
            Err(error) => {
                set_error(cpu, error);
                return Some(0);
            }
        }
    } else {
        let place = if m.locks > 0 && flags & GMEM_MOVEABLE == 0 {
            HEAP_REALLOC_IN_PLACE_ONLY
        } else {
            0
        };
        match memory::realloc(cpu, heap, zero | place, m.block, size) {
            Ok(block) => block,
            Err(error) => {
                set_error(cpu, error);
                return Some(0);
            }
        }
    };
    process(cpu).moveables[i].block = block;
    update(cpu, i);
    Some(handle)
}

/// GlobalSize(handle) and LocalSize: its block's size, 0 if it has none.
pub(super) fn global_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let handle = args[0];
    let heap = heap(cpu);
    let block = if is_moveable(handle) {
        let Some(i) = moveable(cpu, handle) else {
            set_error(cpu, ERROR_INVALID_HANDLE);
            return Some(0);
        };
        match process(cpu).moveables[i].block {
            0 => return Some(0),
            block => block,
        }
    } else {
        handle
    };
    Some(memory::size(cpu, heap, block).unwrap_or_else(|_| {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        0
    }))
}

/// GlobalLock(handle) and LocalLock: the block's address, a moveable
/// block's lock count raised; NULL for one discarded.
pub(super) fn global_lock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let handle = args[0];
    if !is_moveable(handle) {
        return Some(handle);
    }
    let Some(i) = moveable(cpu, handle) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(0);
    };
    let m = &mut process(cpu).moveables[i];
    if m.block == 0 {
        return Some(0);
    }
    m.locks = (m.locks + 1).min(MAX_LOCKS);
    Some(m.block)
}

/// GlobalUnlock(handle) and LocalUnlock: whether a moveable block is still
/// locked after its lock count went down.
pub(super) fn global_unlock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let handle = args[0];
    if !is_moveable(handle) {
        return Some(FALSE);
    }
    let Some(i) = moveable(cpu, handle) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    let m = &mut process(cpu).moveables[i];
    m.locks = m.locks.saturating_sub(1);
    Some((m.locks > 0) as u32)
}

/// GlobalHandle(address) and LocalHandle: the handle of the block at the
/// address, or a handle given as it is.
pub(super) fn global_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    let process = process(cpu);
    if let Some(m) = process
        .moveables
        .iter()
        .find(|m| m.handle == at || (m.block == at && at != 0))
    {
        return Some(m.handle);
    }
    if process.heap.has(at) {
        return Some(at);
    }
    set_error(cpu, ERROR_INVALID_HANDLE);
    Some(0)
}

/// GlobalFlags(handle) and LocalFlags: a moveable block's lock count,
/// GMEM_DISCARDABLE and GMEM_DISCARDED; 0 for a fixed block.
pub(super) fn global_flags(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let handle = args[0];
    if !is_moveable(handle) {
        return Some(0);
    }
    let Some(i) = moveable(cpu, handle) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(GMEM_INVALID_HANDLE);
    };
    let m = &process(cpu).moveables[i];
    let discarded = if m.block == 0 { GMEM_DISCARDED } else { 0 };
    Some(m.locks | m.flags | discarded)
}

/// GlobalFix and GlobalWire lock a block, GlobalUnfix and GlobalUnWire
/// unlock it, as in Win32 they only do that.
pub(super) fn global_fix(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    global_lock(cpu, args)
}

pub(super) fn global_unfix(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    global_unlock(cpu, args)
}

/// GlobalCompact(minimum) and LocalCompact: the largest block there is
/// room for, which nothing has to move for.
pub(super) fn global_compact(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let available = cpu.bus.win32.pool.as_ref().map_or(0, |p| p.available());
    Some(available)
}

/// LocalShrink(handle, size): Win32's local heap doesn't shrink; 0.
pub(super) fn local_shrink(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// GlobalMemoryStatus(status): the machine's memory, what the processes
/// have left of it, and the private arena's address space.
pub(super) fn global_memory_status(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    const MEMORYSTATUS: u32 = 32;
    let at = args[0];
    let total = cpu.bus.ram().len() as u32;
    let available = cpu.bus.win32.pool.as_ref().map_or(0, |p| p.available());
    let load = 100 - (available as u64 * 100 / total.max(1) as u64).min(100) as u32;
    let (virtual_total, virtual_free) = crate::win32::with_process(cpu, |p, _| {
        let total = crate::win32::memory::SHARED - crate::win32::memory::PRIVATE;
        let used: u32 = p
            .memory
            .regions()
            .iter()
            .filter(|r| r.start < crate::win32::memory::SHARED)
            .map(|r| r.len)
            .sum();
        (total, total.saturating_sub(used))
    });
    let fields = [
        MEMORYSTATUS,
        load,
        total,
        available,
        // The swap file: as much again, which Windows 95 has by default.
        total * 2,
        available + total,
        virtual_total,
        virtual_free,
    ];
    for (i, value) in fields.into_iter().enumerate() {
        cpu.bus.guest_write_32(at.wrapping_add(4 * i as u32), value);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(TRUE)
}
