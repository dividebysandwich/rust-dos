//! COMCTL32's small functions: InitCommonControls and its Ex,
//! DllGetVersion, the memory functions the controls and the shell use
//! (Alloc, ReAlloc, Free, GetSize, on the process's heap), and the
//! string helpers Str_GetPtrA and Str_SetPtrA, as Wine's commctrl.c and
//! comctl32undoc.c have them (Copyright 1997 Dimitrie O. Paun, 1998-2000
//! Eric Kohl, LGPL 2.1 or later), with Windows 9x's answers where the
//! tests show they differ.

use super::kernel32;
use crate::cpu::Cpu;

const HEAP_ZERO_MEMORY: u32 = 0x08;
const E_INVALIDARG: u32 = 0x8007_0057;

/// The process's heap.
pub fn process_heap(cpu: &mut Cpu) -> u32 {
    kernel32(cpu, "GetProcessHeap", &[]).unwrap_or(0)
}

/// HeapAlloc(heap, HEAP_ZERO_MEMORY, size): the block, or 0.
pub fn heap_alloc(cpu: &mut Cpu, heap: u32, size: u32) -> u32 {
    kernel32(cpu, "HeapAlloc", &[heap, HEAP_ZERO_MEMORY, size]).unwrap_or(0)
}

/// HeapReAlloc(heap, HEAP_ZERO_MEMORY, at, size): the block, or 0.
pub fn heap_realloc(cpu: &mut Cpu, heap: u32, at: u32, size: u32) -> u32 {
    kernel32(cpu, "HeapReAlloc", &[heap, HEAP_ZERO_MEMORY, at, size]).unwrap_or(0)
}

pub fn heap_free(cpu: &mut Cpu, heap: u32, at: u32) -> bool {
    kernel32(cpu, "HeapFree", &[heap, 0, at]).unwrap_or(0) != 0
}

/// InitCommonControls(): the classes are there from the start.
pub fn init_common_controls(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// InitCommonControlsEx(init): TRUE for an INITCOMMONCONTROLSEX of the
/// right size.
pub fn init_common_controls_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(0);
    }
    let size = cpu.bus.guest_read_32(args[0]);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some((size == 8) as u32)
}

/// DllGetVersion(info): COMCTL32 4.70, Internet Explorer 3's.
pub fn dll_get_version(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if at == 0 {
        return Some(E_INVALIDARG);
    }
    let size = cpu.bus.guest_read_32(at);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if size != 20 && size != 32 {
        return Some(E_INVALIDARG);
    }
    // Major, minor, build, platform (DLLVER_PLATFORM_WINDOWS).
    crate::win32::user32::write_words(cpu, at + 4, &[4, 70, 1158, 1]);
    if size == 32 {
        crate::win32::user32::write_words(cpu, at + 20, &[0, 0, 0]);
    }
    Some(0)
}

/// Alloc(size): a zeroed block of the process's heap.
pub fn alloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let heap = process_heap(cpu);
    Some(heap_alloc(cpu, heap, args[0]))
}

/// ReAlloc(block, size): the block made `size` bytes, or a new one for
/// a null block.
pub fn realloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let heap = process_heap(cpu);
    if args[0] == 0 {
        return Some(heap_alloc(cpu, heap, args[1]));
    }
    Some(heap_realloc(cpu, heap, args[0], args[1]))
}

/// Free(block): TRUE, also for a null block.
pub fn free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(1);
    }
    let heap = process_heap(cpu);
    Some(heap_free(cpu, heap, args[0]) as u32)
}

/// GetSize(block).
pub fn get_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let heap = process_heap(cpu);
    let size = kernel32(cpu, "HeapSize", &[heap, 0, args[0]]).unwrap_or(0);
    Some(if size == u32::MAX { 0 } else { size })
}

/// Str_GetPtrA(src, dest, max): as Windows 9x's, the characters copied
/// without the 0, or the length of `src` when there is no `dest`; none
/// for a `max` of 0.
pub fn str_get_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[src, dest, max] = args else {
        unreachable!()
    };
    let text = if src == 0 {
        Vec::new()
    } else {
        crate::win32::user32::bytes_at(cpu, src)?
    };
    if dest == 0 {
        return Some(text.len() as u32);
    }
    if max == 0 {
        return Some(0);
    }
    Some(crate::win32::user32::copy_out(cpu, dest, max, &text))
}

/// Str_SetPtrA(dest, src): *dest made a copy of `src` in a block of
/// Alloc's, or freed and null for a null `src`.
pub fn str_set_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[dest, src] = args else { unreachable!() };
    let old = cpu.bus.guest_read_32(dest);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let heap = process_heap(cpu);
    if src == 0 {
        if old != 0 {
            heap_free(cpu, heap, old);
        }
        cpu.bus.guest_write_32(dest, 0);
        return Some(1);
    }
    let mut text = crate::win32::user32::bytes_at(cpu, src)?;
    text.push(0);
    let block = if old == 0 {
        heap_alloc(cpu, heap, text.len() as u32)
    } else {
        heap_realloc(cpu, heap, old, text.len() as u32)
    };
    if block == 0 {
        return Some(0);
    }
    cpu.bus.guest_write_bytes(block, &text);
    cpu.bus.guest_write_32(dest, block);
    Some(1)
}
