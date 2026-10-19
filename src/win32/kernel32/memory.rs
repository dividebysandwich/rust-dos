//! Thread local storage, the process heap and the interlocked functions.

use super::{
    ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY, FALSE, TRUE, set_error,
};
use crate::cpu::Cpu;
use crate::win32::loader::{TLS_ARRAY, TLS_SLOTS};
use crate::win32::{process, with_process};

const TLS_OUT_OF_INDEXES: u32 = 0xFFFF_FFFF;

/// HeapAlloc's and HeapReAlloc's flags.
const HEAP_ZERO_MEMORY: u32 = 0x08;
const HEAP_REALLOC_IN_PLACE_ONLY: u32 = 0x10;

/// The address of the thread's TLS slot `index`.
fn slot(cpu: &mut Cpu, index: u32) -> u32 {
    process(cpu).tib + TLS_ARRAY + 4 * index
}

/// The lowest free slot, its value set to 0.
pub(super) fn tls_alloc(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let used = process(cpu).tls_used;
    let Some(index) = (0..TLS_SLOTS).find(|&i| used & 1 << i == 0) else {
        return Some(TLS_OUT_OF_INDEXES);
    };
    super::threads::clear_tls_slot(cpu, index);
    process(cpu).tls_used |= 1 << index;
    Some(index)
}

pub(super) fn tls_free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let index = args[0];
    let process = process(cpu);
    if index >= TLS_SLOTS || process.tls_used & 1 << index == 0 {
        process.last_error = ERROR_INVALID_PARAMETER;
        return Some(FALSE);
    }
    process.tls_used &= !(1 << index);
    super::threads::clear_tls_slot(cpu, index);
    Some(TRUE)
}

/// The value in slot `args[0]`, the last error cleared, as Windows does
/// so that a value of 0 can be told from a failure.
pub(super) fn tls_get_value(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] >= TLS_SLOTS {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let at = slot(cpu, args[0]);
    let value = cpu.bus.guest_read_32(at);
    set_error(cpu, 0);
    Some(value)
}

pub(super) fn tls_set_value(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] >= TLS_SLOTS {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let at = slot(cpu, args[0]);
    cpu.bus.guest_write_32(at, args[1]);
    Some(TRUE)
}

pub(super) fn get_process_heap(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(process(cpu).heap.base)
}

/// Whether `heap` is the process heap, the only heap there is so far. If
/// it isn't, the last error says so.
fn known_heap(cpu: &mut Cpu, heap: u32) -> bool {
    let known = process(cpu).heap.base == heap;
    if !known {
        set_error(cpu, ERROR_INVALID_HANDLE);
    }
    known
}

/// A block of `size` bytes in `heap`, zeroed with HEAP_ZERO_MEMORY.
fn alloc(cpu: &mut Cpu, flags: u32, size: u32) -> Option<u32> {
    let at = with_process(cpu, |p, bus| p.heap.alloc(&mut p.memory, bus, size));
    match at {
        Some(at) => {
            if flags & HEAP_ZERO_MEMORY != 0 {
                cpu.bus.guest_fill(at, size as usize, 0);
            }
            Some(at)
        }
        None => {
            set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
            None
        }
    }
}

pub(super) fn heap_alloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[heap, flags, size] = args else {
        unreachable!()
    };
    if !known_heap(cpu, heap) {
        return Some(0);
    }
    Some(alloc(cpu, flags, size).unwrap_or(0))
}

pub(super) fn heap_free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[heap, _, at] = args else { unreachable!() };
    if !known_heap(cpu, heap) {
        return Some(FALSE);
    }
    if at == 0 || process(cpu).heap.free(at) {
        return Some(TRUE);
    }
    set_error(cpu, ERROR_INVALID_PARAMETER);
    Some(FALSE)
}

/// The block at `args[2]` made `args[3]` bytes long: where it is if there
/// is room after it, else moved, unless HEAP_REALLOC_IN_PLACE_ONLY says not
/// to. With HEAP_ZERO_MEMORY the bytes it gains are zeroed.
pub(super) fn heap_realloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[heap, flags, at, size] = args else {
        unreachable!()
    };
    if !known_heap(cpu, heap) {
        return Some(0);
    }
    let Some(old) = process(cpu).heap.size(at) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    let in_place = with_process(cpu, |p, bus| {
        p.heap.resize_in_place(&mut p.memory, bus, at, size)
    });
    let to = if in_place {
        at
    } else if flags & HEAP_REALLOC_IN_PLACE_ONLY != 0 {
        set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
        return Some(0);
    } else {
        let Some(to) = alloc(cpu, 0, size) else {
            return Some(0);
        };
        let mut data = vec![0; old.min(size) as usize];
        cpu.bus.guest_read_bytes(at, &mut data);
        cpu.bus.guest_write_bytes(to, &data);
        process(cpu).heap.free(at);
        to
    };
    if flags & HEAP_ZERO_MEMORY != 0 && size > old {
        cpu.bus.guest_fill(to + old, (size - old) as usize, 0);
    }
    Some(to)
}

pub(super) fn heap_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[heap, _, at] = args else { unreachable!() };
    if !known_heap(cpu, heap) {
        return Some(u32::MAX);
    }
    match process(cpu).heap.size(at) {
        Some(size) => Some(size),
        None => {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            Some(u32::MAX)
        }
    }
}

/// Add `delta` to the variable at `at`: its new value. (Windows 95's
/// returns only its sign, which this agrees with.)
fn interlocked_add(cpu: &mut Cpu, at: u32, delta: i32) -> Option<u32> {
    let value = cpu.bus.guest_read_32(at).wrapping_add(delta as u32);
    cpu.bus.guest_write_32(at, value);
    Some(value)
}

pub(super) fn interlocked_increment(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    interlocked_add(cpu, args[0], 1)
}

pub(super) fn interlocked_decrement(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    interlocked_add(cpu, args[0], -1)
}

/// Store `args[1]` in the variable at `args[0]`: its old value.
pub(super) fn interlocked_exchange(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let old = cpu.bus.guest_read_32(args[0]);
    cpu.bus.guest_write_32(args[0], args[1]);
    Some(old)
}
