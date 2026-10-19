//! Thread local storage, heaps and the interlocked functions.

use super::{
    ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY, FALSE, TRUE, set_error,
};
use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::win32::heap::Heap;
use crate::win32::loader::{TLS_ARRAY, TLS_SLOTS};
use crate::win32::memory::Memory;
use crate::win32::{process, with_process};

const TLS_OUT_OF_INDEXES: u32 = 0xFFFF_FFFF;

/// HeapCreate's, HeapAlloc's and HeapReAlloc's flags.
const HEAP_NO_SERIALIZE: u32 = 0x01;
const HEAP_GENERATE_EXCEPTIONS: u32 = 0x04;
pub(super) const HEAP_ZERO_MEMORY: u32 = 0x08;
pub(super) const HEAP_REALLOC_IN_PLACE_ONLY: u32 = 0x10;
/// The exception HEAP_GENERATE_EXCEPTIONS raises.
const STATUS_NO_MEMORY: u32 = 0xC000_0017;

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

/// Run `f` with the running process's heap `handle` and its memory. None
/// if there is no such heap.
fn with_heap<T>(
    cpu: &mut Cpu,
    handle: u32,
    f: impl FnOnce(&mut Heap, &mut Memory, &mut Bus) -> T,
) -> Option<T> {
    with_process(cpu, |p, bus| {
        let heap = if p.heap.base == handle {
            &mut p.heap
        } else {
            p.heaps.iter_mut().find(|h| h.base == handle)?
        };
        Some(f(heap, &mut p.memory, bus))
    })
}

/// A block of `size` bytes in `heap`, zeroed with HEAP_ZERO_MEMORY; or the
/// error code.
pub(super) fn alloc(cpu: &mut Cpu, heap: u32, flags: u32, size: u32) -> Result<u32, u32> {
    match with_heap(cpu, heap, |h, m, bus| h.alloc(m, bus, size)) {
        None => Err(ERROR_INVALID_HANDLE),
        Some(None) => Err(ERROR_NOT_ENOUGH_MEMORY),
        Some(Some(at)) => {
            if flags & HEAP_ZERO_MEMORY != 0 {
                cpu.bus.guest_fill(at, size as usize, 0);
            }
            Ok(at)
        }
    }
}

/// Give the block at `at` of `heap` back.
pub(super) fn free(cpu: &mut Cpu, heap: u32, at: u32) -> Result<(), u32> {
    match with_heap(cpu, heap, |h, _, _| at == 0 || h.free(at)) {
        None => Err(ERROR_INVALID_HANDLE),
        Some(false) => Err(ERROR_INVALID_PARAMETER),
        Some(true) => Ok(()),
    }
}

/// The size of the block at `at` of `heap`.
pub(super) fn size(cpu: &mut Cpu, heap: u32, at: u32) -> Result<u32, u32> {
    match with_heap(cpu, heap, |h, _, _| h.size(at)) {
        None => Err(ERROR_INVALID_HANDLE),
        Some(None) => Err(ERROR_INVALID_PARAMETER),
        Some(Some(size)) => Ok(size),
    }
}

/// The block at `at` of `heap` made `size` bytes long: where it is if
/// there is room after it, else moved, unless HEAP_REALLOC_IN_PLACE_ONLY
/// says not to. With HEAP_ZERO_MEMORY the bytes it gains are zeroed.
pub(super) fn realloc(
    cpu: &mut Cpu,
    heap: u32,
    flags: u32,
    at: u32,
    size: u32,
) -> Result<u32, u32> {
    let old = self::size(cpu, heap, at)?;
    let in_place = with_heap(cpu, heap, |h, m, bus| h.resize_in_place(m, bus, at, size));
    let to = if in_place == Some(true) {
        at
    } else if flags & HEAP_REALLOC_IN_PLACE_ONLY != 0 {
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    } else {
        let to = alloc(cpu, heap, 0, size)?;
        let mut data = vec![0; old.min(size) as usize];
        cpu.bus.guest_read_bytes(at, &mut data);
        cpu.bus.guest_write_bytes(to, &data);
        let _ = free(cpu, heap, at);
        to
    };
    if flags & HEAP_ZERO_MEMORY != 0 && size > old {
        cpu.bus.guest_fill(to + old, (size - old) as usize, 0);
    }
    Ok(to)
}

/// What a heap function that failed with `error` returns, 0: the last
/// error set, or with HEAP_GENERATE_EXCEPTIONS (in `flags` or the heap's
/// own) for want of memory, STATUS_NO_MEMORY raised. `args` is how many
/// arguments the function takes.
fn failed(cpu: &mut Cpu, heap: u32, flags: u32, error: u32, args: u32) -> Option<u32> {
    let own = with_heap(cpu, heap, |h, _, _| h.flags).unwrap_or(0);
    if error == ERROR_NOT_ENOUGH_MEMORY && (flags | own) & HEAP_GENERATE_EXCEPTIONS != 0 {
        return super::super::seh::raise_on_return(cpu, args, STATUS_NO_MEMORY);
    }
    set_error(cpu, error);
    Some(0)
}

pub(super) fn heap_alloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[heap, flags, size] = args else {
        unreachable!()
    };
    match alloc(cpu, heap, flags, size) {
        Ok(at) => Some(at),
        Err(error) => failed(cpu, heap, flags, error, 3),
    }
}

pub(super) fn heap_free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[heap, _, at] = args else { unreachable!() };
    match free(cpu, heap, at) {
        Ok(()) => Some(TRUE),
        Err(error) => {
            set_error(cpu, error);
            Some(FALSE)
        }
    }
}

pub(super) fn heap_realloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[heap, flags, at, size] = args else {
        unreachable!()
    };
    match realloc(cpu, heap, flags, at, size) {
        Ok(to) => Some(to),
        Err(error) => failed(cpu, heap, flags, error, 4),
    }
}

pub(super) fn heap_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[heap, _, at] = args else { unreachable!() };
    match size(cpu, heap, at) {
        Ok(size) => Some(size),
        Err(error) => {
            set_error(cpu, error);
            Some(u32::MAX)
        }
    }
}

/// HeapCreate(flags, initial, maximum): a heap of `maximum` bytes, or with
/// 0 one that grows, `initial` bytes of it committed. Its handle is its
/// address, as on Windows 95.
pub(super) fn heap_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[flags, initial, max] = args else {
        unreachable!()
    };
    let created = with_process(cpu, |p, bus| {
        let mut heap = Heap::create(&mut p.memory, bus, initial, max)?;
        heap.flags = flags & (HEAP_NO_SERIALIZE | HEAP_GENERATE_EXCEPTIONS);
        let base = heap.base;
        p.heaps.push(heap);
        Some(base)
    });
    match created {
        Some(base) => Some(base),
        None => {
            set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
            Some(0)
        }
    }
}

/// HeapDestroy(heap): its memory goes. The process heap stays.
pub(super) fn heap_destroy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let handle = args[0];
    let destroyed = with_process(cpu, |p, bus| {
        let i = p.heaps.iter().position(|h| h.base == handle)?;
        let mut heap = p.heaps.remove(i);
        heap.destroy(&mut p.memory, bus);
        Some(())
    });
    if destroyed.is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    cpu.tlb.flush();
    Some(TRUE)
}

/// HeapValidate(heap, flags, block): whether the block is one of the
/// heap's, or with NULL whether the heap is there. Blocks' sizes are kept
/// outside the heap's memory, so there is nothing in it to spoil.
pub(super) fn heap_validate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[heap, _, at] = args else { unreachable!() };
    match with_heap(cpu, heap, |h, _, _| at == 0 || h.has(at)) {
        Some(valid) => Some(valid as u32),
        None => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            Some(FALSE)
        }
    }
}

/// HeapCompact(heap, flags): the size of the largest block it could give
/// out without growing.
pub(super) fn heap_compact(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match with_heap(cpu, args[0], |h, _, _| h.largest_free()) {
        Some(size) => Some(size),
        None => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            Some(0)
        }
    }
}

/// HeapLock and HeapUnlock: one thread runs at a time between traps, so
/// there is nothing to hold; they succeed for any heap there is.
pub(super) fn heap_lock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match with_heap(cpu, args[0], |_, _, _| ()) {
        Some(()) => Some(TRUE),
        None => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            Some(FALSE)
        }
    }
}

/// GetProcessHeaps(count, heaps): the process heap and those HeapCreate
/// made, as many as fit in the array; how many there are.
pub(super) fn get_process_heaps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[count, array] = args else {
        unreachable!()
    };
    let process = process(cpu);
    let heaps: Vec<u32> = std::iter::once(process.heap.base)
        .chain(process.heaps.iter().map(|h| h.base))
        .collect();
    for (i, &heap) in heaps.iter().take(count as usize).enumerate() {
        cpu.bus
            .guest_write_32(array.wrapping_add(4 * i as u32), heap);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(heaps.len() as u32)
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
