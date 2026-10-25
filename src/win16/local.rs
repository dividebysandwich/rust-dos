//! Local heaps: LocalAlloc and its family, in a data segment (DS, a
//! task's or a library's automatic data segment mostly), between the
//! offsets LocalInit gave. The blocks' sizes and handles are kept here; in
//! the segment, a moveable block's handle is the offset of an entry whose
//! first word is the block's offset, as Windows 3.1's handle tables have
//! them, since programs read them so. As there, where entries follow a
//! table's count word, a moveable handle is 2 more than a multiple of 4
//! and a fixed one (the block's offset) a multiple of 4. Blocks never move but for
//! LocalReAlloc, which may move a moveable one; a heap that is full grows
//! its segment, to 64 KB.

use super::global;
use crate::cpu::Cpu;

pub const LMEM_MOVEABLE: u16 = 0x0002;
pub const LMEM_ZEROINIT: u16 = 0x0040;
pub const LMEM_MODIFY: u16 = 0x0080;
pub const LMEM_DISCARDABLE: u16 = 0x0F00;
/// LocalFlags' discarded bit, kept in a block's flags.
const LMEM_DISCARDED: u16 = 0x4000;
/// Where a moveable block's handle is in the block of its entry.
const ENTRY: u16 = 2;

/// Blocks start at multiples of this, a header's 4 bytes before each.
const ALIGN: u16 = 4;
const HEADER: u16 = 4;
/// The bytes of the heap's information block at its start.
const INFO: u16 = 0x30;

/// A local heap.
#[derive(Clone, Debug, Default)]
pub struct LocalHeap {
    /// The segment's selector (with RPL 3), and the heap's offsets.
    pub sel: u16,
    pub start: u16,
    pub end: u16,
    /// Its blocks, in offset order: the handle tables' entries too.
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, Default)]
pub struct Block {
    /// Where its data is, and how many bytes it has.
    pub offset: u16,
    pub size: u16,
    /// Its handle: the offset for a fixed block, the handle entry's for a
    /// moveable one; 0 for a handle entry itself.
    pub handle: u16,
    pub flags: u16,
    pub lock: u8,
}

fn heap_index(cpu: &Cpu, sel: u16) -> Option<usize> {
    let sel = sel | 3;
    cpu.bus
        .win32
        .win16
        .local_heaps
        .iter()
        .position(|h| h.sel | 3 == sel)
}

/// LocalInit(segment, start, end): a heap in the segment `sel` from
/// `start` to `end`, or with `start` 0 of `end` bytes at the segment's
/// end.
pub fn init(cpu: &mut Cpu, sel: u16, start: u16, end: u16) -> bool {
    let sel = if sel == 0 { cpu.ds() } else { sel };
    let Some((base, limit, _)) = super::selector_base(cpu, sel) else {
        return false;
    };
    let (start, end) = if start == 0 {
        let size = (limit + 1).min(0xFFFF) as u16;
        let end = end.min(0xFFFE);
        let start = size.saturating_sub(1).saturating_sub(end);
        (start, start.saturating_add(end))
    } else {
        (start, end)
    };
    if end <= start || (end as u32) > limit + 1 {
        return false;
    }
    let start = start.next_multiple_of(ALIGN);
    // The information block, "LH" at its 28h, and the instance data's
    // pointer to it.
    let mut info = vec![0u8; INFO as usize];
    info[0x28..0x2A].copy_from_slice(b"LH");
    super::poke(cpu, base + start as u32, &info);
    super::write_16(cpu, base + 6, start);
    let win16 = &mut cpu.bus.win32.win16;
    win16.local_heaps.retain(|h| h.sel | 3 != sel | 3);
    win16.local_heaps.push(LocalHeap {
        sel,
        start: start + INFO,
        end,
        blocks: Vec::new(),
    });
    true
}

/// Where a block of `size` bytes fits first in the heap `h`, and its
/// place in the list.
fn fit(h: &LocalHeap, size: u16) -> Option<(u16, usize)> {
    let len = (size.max(1) as u32).next_multiple_of(ALIGN as u32) + HEADER as u32;
    let mut at = h.start as u32;
    for (i, b) in h.blocks.iter().enumerate() {
        let block_start = b.offset as u32 - HEADER as u32;
        if block_start >= at + len {
            return Some(((at + HEADER as u32) as u16, i));
        }
        at = b.offset as u32 + (b.size.max(1) as u32).next_multiple_of(ALIGN as u32);
    }
    (at + len <= h.end as u32).then(|| ((at + HEADER as u32) as u16, h.blocks.len()))
}

/// Grow the segment of the heap `i` so it has `more` bytes more, to 64 KB.
fn grow(cpu: &mut Cpu, i: usize, more: u16) -> bool {
    let h = &cpu.bus.win32.win16.local_heaps[i];
    let sel = h.sel;
    let end = h.end as u32;
    let new_end = (end + more as u32 + 0x400).min(0xFFF0);
    if new_end <= end {
        return false;
    }
    let handle = global::handle_of(cpu, sel);
    let Some(a) = global::get(cpu, handle) else {
        return false;
    };
    if a.size < new_end {
        let h2 = global::realloc(cpu, handle, new_end, 0);
        if h2 == 0 {
            return false;
        }
        // The segment registers that have it see where it is now.
        let ctx = crate::dpmi::Context::of(cpu);
        crate::win32::system::load_context(cpu, &ctx);
    }
    cpu.bus.win32.win16.local_heaps[i].end = new_end as u16;
    true
}

/// Take a block of `size` bytes in the heap `i`: its place in the list.
fn take(cpu: &mut Cpu, i: usize, size: u16, handle: u16, flags: u16) -> Option<usize> {
    let found = match fit(&cpu.bus.win32.win16.local_heaps[i], size) {
        Some(f) => f,
        None => {
            if !grow(cpu, i, size.saturating_add(HEADER)) {
                return None;
            }
            fit(&cpu.bus.win32.win16.local_heaps[i], size)?
        }
    };
    let (offset, place) = found;
    let h = &mut cpu.bus.win32.win16.local_heaps[i];
    h.blocks.insert(
        place,
        Block {
            offset,
            size,
            handle: if handle == 0 { offset } else { handle },
            flags,
            lock: 0,
        },
    );
    Some(place)
}

fn seg_base(cpu: &Cpu, i: usize) -> u32 {
    let sel = cpu.bus.win32.win16.local_heaps[i].sel;
    super::selector_base(cpu, sel).map_or(0, |(b, _, _)| b)
}

/// LocalAlloc(flags, size) in the heap of the segment `sel`: the block's
/// handle, or 0.
pub fn alloc(cpu: &mut Cpu, sel: u16, flags: u16, size: u16) -> u16 {
    let Some(i) = heap_index(cpu, sel) else {
        return 0;
    };
    let offset = if flags & LMEM_MOVEABLE != 0 {
        // The handle's entry, then the block.
        let Some(e) = take(cpu, i, ENTRY + 4, 0, 0xFFFF) else {
            return 0;
        };
        let entry = cpu.bus.win32.win16.local_heaps[i].blocks[e].offset + ENTRY;
        cpu.bus.win32.win16.local_heaps[i].blocks[e].handle = 0;
        let Some(b) = take(cpu, i, size, entry, flags) else {
            free_entry(cpu, i, entry);
            return 0;
        };
        let data = cpu.bus.win32.win16.local_heaps[i].blocks[b].offset;
        let base = seg_base(cpu, i);
        super::poke(
            cpu,
            base + entry as u32,
            &[data as u8, (data >> 8) as u8, flags as u8, 0],
        );
        (entry, data)
    } else {
        let Some(b) = take(cpu, i, size, 0, flags) else {
            return 0;
        };
        let data = cpu.bus.win32.win16.local_heaps[i].blocks[b].offset;
        (data, data)
    };
    if flags & LMEM_ZEROINIT != 0 {
        let base = seg_base(cpu, i);
        super::poke(cpu, base + offset.1 as u32, &vec![0; size as usize]);
    }
    offset.0
}

fn free_entry(cpu: &mut Cpu, i: usize, entry: u16) {
    cpu.bus.win32.win16.local_heaps[i]
        .blocks
        .retain(|b| !(b.handle == 0 && b.offset + ENTRY == entry));
}

/// The place in its heap's list of the block whose handle is `handle`.
fn find(cpu: &Cpu, i: usize, handle: u16) -> Option<usize> {
    cpu.bus.win32.win16.local_heaps[i]
        .blocks
        .iter()
        .position(|b| b.handle == handle && b.handle != 0)
}

/// LocalFree: 0, or the handle when it isn't one.
pub fn free(cpu: &mut Cpu, sel: u16, handle: u16) -> u16 {
    let Some(i) = heap_index(cpu, sel) else {
        return handle;
    };
    let Some(b) = find(cpu, i, handle) else {
        return handle;
    };
    let block = cpu.bus.win32.win16.local_heaps[i].blocks.remove(b);
    if block.handle != block.offset {
        free_entry(cpu, i, block.handle);
    }
    0
}

/// LocalLock: the block's offset (and a moveable one's lock counted), or
/// 0.
pub fn lock(cpu: &mut Cpu, sel: u16, handle: u16) -> u16 {
    let Some(i) = heap_index(cpu, sel) else {
        return 0;
    };
    let Some(b) = find(cpu, i, handle) else {
        return 0;
    };
    let block = &mut cpu.bus.win32.win16.local_heaps[i].blocks[b];
    if block.flags & LMEM_DISCARDED != 0 {
        return 0;
    }
    if block.handle != block.offset {
        block.lock = block.lock.saturating_add(1);
    }
    block.offset
}

/// LocalUnlock: whether it is still locked.
pub fn unlock(cpu: &mut Cpu, sel: u16, handle: u16) -> bool {
    let Some(i) = heap_index(cpu, sel) else {
        return false;
    };
    let Some(b) = find(cpu, i, handle) else {
        return false;
    };
    let block = &mut cpu.bus.win32.win16.local_heaps[i].blocks[b];
    block.lock = block.lock.saturating_sub(1);
    block.lock > 0
}

/// LocalSize.
pub fn size(cpu: &Cpu, sel: u16, handle: u16) -> u16 {
    let Some(i) = heap_index(cpu, sel) else {
        return 0;
    };
    find(cpu, i, handle).map_or(0, |b| cpu.bus.win32.win16.local_heaps[i].blocks[b].size)
}

/// LocalFlags: the lock count, discardable and discarded.
pub fn flags(cpu: &Cpu, sel: u16, handle: u16) -> u16 {
    let Some(i) = heap_index(cpu, sel) else {
        return 0;
    };
    let Some(b) = find(cpu, i, handle) else {
        return 0;
    };
    let block = &cpu.bus.win32.win16.local_heaps[i].blocks[b];
    block.lock as u16 | block.flags & (LMEM_DISCARDABLE | LMEM_DISCARDED)
}

/// LocalHandle: the handle of the block at `offset`.
pub fn handle(cpu: &Cpu, sel: u16, offset: u16) -> u16 {
    let Some(i) = heap_index(cpu, sel) else {
        return 0;
    };
    cpu.bus.win32.win16.local_heaps[i]
        .blocks
        .iter()
        .find(|b| b.offset == offset && b.handle != 0)
        .map_or(0, |b| b.handle)
}

/// LocalReAlloc(handle, size, flags): the block's handle afterwards, or
/// 0. A fixed block grows where it is only; a moveable one moves if it
/// must, its handle's entry following it.
pub fn realloc(cpu: &mut Cpu, sel: u16, handle: u16, size: u16, flags: u16) -> u16 {
    let Some(i) = heap_index(cpu, sel) else {
        return 0;
    };
    let Some(b) = find(cpu, i, handle) else {
        return 0;
    };
    if flags & LMEM_MODIFY != 0 {
        let block = &mut cpu.bus.win32.win16.local_heaps[i].blocks[b];
        block.flags = block.flags & !LMEM_DISCARDABLE | flags & LMEM_DISCARDABLE;
        return handle;
    }
    let block = cpu.bus.win32.win16.local_heaps[i].blocks[b].clone();
    let moveable = block.handle != block.offset;
    // LocalDiscard (size 0, LMEM_MOVEABLE): an unlocked moveable block is
    // discarded, its handle kept; a locked one isn't.
    if size == 0 && moveable && flags & LMEM_MOVEABLE != 0 {
        if block.lock > 0 {
            return 0;
        }
        let block = &mut cpu.bus.win32.win16.local_heaps[i].blocks[b];
        block.size = 0;
        block.flags |= LMEM_DISCARDED;
        let base = seg_base(cpu, i);
        super::write_16(cpu, base + handle as u32, 0);
        return handle;
    }
    cpu.bus.win32.win16.local_heaps[i].blocks[b].flags &= !LMEM_DISCARDED;
    let h = &cpu.bus.win32.win16.local_heaps[i];
    let limit = h
        .blocks
        .get(b + 1)
        .map_or(h.end as u32, |n| (n.offset - HEADER) as u32);
    let len = (size.max(1) as u32).next_multiple_of(ALIGN as u32);
    if block.offset as u32 + len <= limit {
        let base = seg_base(cpu, i);
        if size > block.size && flags & LMEM_ZEROINIT != 0 {
            let at = base + block.offset as u32 + block.size as u32;
            super::poke(cpu, at, &vec![0; (size - block.size) as usize]);
        }
        cpu.bus.win32.win16.local_heaps[i].blocks[b].size = size;
        if moveable {
            super::write_16(cpu, base + handle as u32, block.offset);
        }
        return handle;
    }
    if !moveable && flags & LMEM_MOVEABLE == 0 {
        return 0;
    }
    // Move it.
    let base = seg_base(cpu, i);
    let old = super::peek(cpu, base + block.offset as u32, block.size as usize).unwrap_or_default();
    cpu.bus.win32.win16.local_heaps[i].blocks.remove(b);
    let new_handle = if moveable { block.handle } else { 0 };
    let kept = block.flags & !LMEM_DISCARDED;
    let Some(n) = take(cpu, i, size, new_handle, kept) else {
        // Back where it was.
        let h = &mut cpu.bus.win32.win16.local_heaps[i];
        let place = h.blocks.partition_point(|x| x.offset < block.offset);
        h.blocks.insert(place, block);
        return 0;
    };
    let base = seg_base(cpu, i);
    let data = cpu.bus.win32.win16.local_heaps[i].blocks[n].offset;
    super::poke(cpu, base + data as u32, &old);
    if size > block.size && flags & LMEM_ZEROINIT != 0 {
        super::poke(
            cpu,
            base + data as u32 + block.size as u32,
            &vec![0; (size - block.size) as usize],
        );
    }
    if moveable {
        super::write_16(cpu, base + block.handle as u32, data);
        block.handle
    } else {
        data
    }
}

/// LocalCompact: the largest free block's size.
pub fn compact(cpu: &Cpu, sel: u16) -> u16 {
    let Some(i) = heap_index(cpu, sel) else {
        return 0;
    };
    let h = &cpu.bus.win32.win16.local_heaps[i];
    let mut largest = 0u32;
    let mut at = h.start as u32;
    for b in &h.blocks {
        let start = (b.offset - HEADER) as u32;
        largest = largest.max(start.saturating_sub(at));
        at = b.offset as u32 + (b.size.max(1) as u32).next_multiple_of(ALIGN as u32);
    }
    largest = largest.max((h.end as u32).saturating_sub(at));
    largest.saturating_sub(HEADER as u32).min(0xFFFF) as u16
}

/// LocalHeapSize: the heap's bytes.
pub fn heap_size(cpu: &Cpu, sel: u16) -> u16 {
    heap_index(cpu, sel).map_or(0, |i| {
        let h = &cpu.bus.win32.win16.local_heaps[i];
        h.end - h.start
    })
}

/// The free bytes of the heap (LocalCountFree).
pub fn count_free(cpu: &Cpu, sel: u16) -> u16 {
    heap_index(cpu, sel).map_or(0, |i| {
        let h = &cpu.bus.win32.win16.local_heaps[i];
        let used: u32 = h
            .blocks
            .iter()
            .map(|b| (b.size.max(1) as u32).next_multiple_of(ALIGN as u32) + HEADER as u32)
            .sum();
        ((h.end - h.start) as u32).saturating_sub(used) as u16
    })
}

/// The heaps of the segment `sel` go, as its block is freed.
pub fn forget(cpu: &mut Cpu, sel: u16) {
    cpu.bus
        .win32
        .win16
        .local_heaps
        .retain(|h| h.sel | 3 != sel | 3);
}

crate::state_fields!(LocalHeap {
    sel,
    start,
    end,
    blocks
});
crate::state_fields!(Block {
    offset,
    size,
    handle,
    flags,
    lock
});
