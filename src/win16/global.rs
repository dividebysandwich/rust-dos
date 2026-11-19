//! The global heap: GlobalAlloc and its family, for 16-bit code. A block
//! is a range of the Win16 process's memory (from `State::heap`) with a
//! selector for each 64 KB of it. A fixed block's handle is its first
//! selector, a moveable one's that selector less one, as Windows 3.1 and
//! Wine make them; blocks never really move, so locking one only counts.
//! The arena entries are kept by the first selector's LDT index.
//!
//! Ported from Wine's krnl386 global.c (LGPL 2.1 or later, copyright
//! 1995 Alexandre Julliard).

use super::ldt::{self, ACCESS_CODE, ACCESS_DATA};
use super::{State, win16};
use crate::cpu::Cpu;
use rust_dos_ne as ne;

/// GlobalAlloc's flags.
pub const GMEM_FIXED: u16 = 0x0000;
pub const GMEM_MOVEABLE: u16 = 0x0002;
pub const GMEM_ZEROINIT: u16 = 0x0040;
pub const GMEM_MODIFY: u16 = 0x0080;
pub const GMEM_DISCARDABLE: u16 = 0x0100;
pub const GMEM_DDESHARE: u16 = 0x2000;

/// The arena's flags.
pub const GA_MOVEABLE: u8 = 0x02;
pub const GA_DGROUP: u8 = 0x04;
pub const GA_DISCARDABLE: u8 = 0x08;
pub const GA_IPCSHARE: u8 = 0x10;

/// GlobalFlags' answer: discardable, discarded, and the lock count in the
/// low byte.
const GMEM_DISCARDED: u16 = 0x4000;

/// The largest block: 16 MB less 64 KB.
const MAX_ALLOC: u32 = 0x00FF_0000;

/// A block of the global heap.
#[derive(Clone, Debug, Default)]
pub struct Arena {
    /// Its linear address, 0 when it is discarded.
    pub base: u32,
    pub size: u32,
    pub handle: u16,
    /// The task's PSP or the module that owns it.
    pub owner: u16,
    pub lock: u8,
    pub page_lock: u8,
    pub flags: u8,
    /// How many selectors it has.
    pub selectors: u16,
    /// Whether its memory is the system's (a module's code, a DOS block)
    /// rather than the heap's, so freeing it doesn't give it back.
    pub fixed_memory: bool,
}

/// The selector of a handle: a moveable block's handle is its selector
/// less one.
pub fn handle_to_sel(handle: u16) -> u16 {
    if handle == 0 { 0 } else { handle | 1 }
}

fn arena(state: &State, handle: u16) -> Option<&Arena> {
    let sel = handle_to_sel(handle);
    state.arena.get(&ldt::index(sel)).filter(|a| a.size != 0)
}

fn arena_mut(state: &mut State, handle: u16) -> Option<&mut Arena> {
    let sel = handle_to_sel(handle);
    state
        .arena
        .get_mut(&ldt::index(sel))
        .filter(|a| a.size != 0)
}

/// The block whose handle or selector is `handle`.
pub fn get(cpu: &Cpu, handle: u16) -> Option<Arena> {
    arena(&cpu.bus.win32.win16, handle).cloned()
}

/// Make a block of the `size` bytes at `base` (taken by the caller),
/// with its selectors: code (`code`) or data. Its handle.
pub fn create_block(
    cpu: &mut Cpu,
    flags: u16,
    base: u32,
    size: u32,
    owner: u16,
    code: bool,
    fixed_memory: bool,
) -> Option<u16> {
    let count = ldt::tiles(size);
    let sel = cpu.bus.win32.win16.ldt.alloc(count)?;
    let system = win16(cpu).system;
    let access = if code { ACCESS_CODE } else { ACCESS_DATA };
    if base == 0 {
        // A discarded block: a selector that isn't present.
        ldt::write(cpu, system, sel, ldt::segment(0, 0, access & !0x80));
    } else {
        ldt::set_block(cpu, system, sel, base, size, access, false);
    }
    let handle = if flags & GMEM_MOVEABLE != 0 {
        sel - 1
    } else {
        sel
    };
    let mut aflags = (flags as u8) & GA_MOVEABLE;
    if flags & GMEM_DISCARDABLE != 0 {
        aflags |= GA_DISCARDABLE;
    }
    if flags & GMEM_DDESHARE != 0 {
        aflags |= GA_IPCSHARE;
    }
    if !code {
        aflags |= GA_DGROUP;
    }
    cpu.bus.win32.win16.arena.insert(
        ldt::index(sel),
        Arena {
            base,
            size: size.max(1),
            handle,
            owner,
            lock: 0,
            page_lock: 0,
            flags: aflags,
            selectors: count,
            fixed_memory,
        },
    );
    Some(handle)
}

/// Take `size` bytes of the heap's memory: its address.
pub fn take_memory(cpu: &mut Cpu, size: u32) -> Option<u32> {
    let pid = cpu.bus.win32.win16.pid;
    let mut heap = std::mem::take(&mut cpu.bus.win32.win16.heap);
    let at = super::with_win16(cpu, pid, |p, bus| heap.alloc(&mut p.memory, bus, size));
    cpu.bus.win32.win16.heap = heap;
    at.flatten()
}

/// Give back the memory `take_memory` gave at `base`. False when it
/// didn't.
pub fn give_memory(cpu: &mut Cpu, base: u32) -> bool {
    base != 0 && cpu.bus.win32.win16.heap.free(base)
}

fn zero(cpu: &mut Cpu, base: u32, len: u32) {
    let pid = cpu.bus.win32.win16.pid;
    super::with_win16(cpu, pid, |p, bus| {
        p.memory.write(bus, base, &vec![0; len as usize]);
    });
}

fn copy(cpu: &mut Cpu, from: u32, to: u32, len: u32) {
    let pid = cpu.bus.win32.win16.pid;
    super::with_win16(cpu, pid, |p, bus| {
        if let Some(bytes) = crate::win32::loader::peek(bus, &p.memory, from, len as usize) {
            p.memory.write(bus, to, &bytes);
        }
    });
}

/// GlobalAlloc for `owner`: a block of `size` bytes, its handle or 0.
pub fn alloc(cpu: &mut Cpu, flags: u16, size: u32, owner: u16, code: bool) -> u16 {
    if size == 0 {
        return create_block(cpu, flags, 0, 1, owner, code, false).unwrap_or(0);
    }
    if size >= MAX_ALLOC - 0x1F {
        return 0;
    }
    let size = (size + 0x1F) & !0x1F;
    let Some(base) = take_memory(cpu, size) else {
        return 0;
    };
    match create_block(cpu, flags, base, size, owner, code, false) {
        Some(handle) => {
            if flags & GMEM_ZEROINIT != 0 {
                zero(cpu, base, size);
            }
            handle
        }
        None => {
            give_memory(cpu, base);
            0
        }
    }
}

/// Free a block's selectors and arena entry, and its memory unless it is
/// the system's. Whether it was a block.
pub fn free(cpu: &mut Cpu, handle: u16) -> bool {
    let sel = handle_to_sel(handle);
    let Some(a) = arena(&cpu.bus.win32.win16, handle).cloned() else {
        return false;
    };
    // A local heap in it goes with it.
    super::local::forget(cpu, sel);
    let state = &mut cpu.bus.win32.win16;
    state.arena.remove(&ldt::index(sel));
    state.ldt.free(sel, a.selectors);
    let system = win16(cpu).system;
    for i in 0..a.selectors {
        ldt::write(cpu, system, sel + i * ldt::AHINCR, 0);
    }
    if !a.fixed_memory {
        give_memory(cpu, a.base);
    }
    true
}

/// GlobalReAlloc: the block's handle afterwards, or 0.
pub fn realloc(cpu: &mut Cpu, handle: u16, size: u32, flags: u16) -> u16 {
    if handle == 0 {
        return 0;
    }
    let Some(a) = get(cpu, handle) else {
        return 0;
    };
    let sel = handle_to_sel(handle);
    let system = win16(cpu).system;
    let desc = ldt::read(cpu, system, sel);
    let code = desc >> 43 & 1 != 0;
    let big = ldt::is_big(desc);
    let access = if code { ACCESS_CODE } else { ACCESS_DATA };

    // Discard the block.
    if size == 0 && flags & GMEM_MOVEABLE != 0 && flags & GMEM_MODIFY == 0 {
        if a.flags & GA_MOVEABLE == 0
            || a.flags & GA_DISCARDABLE == 0
            || a.lock > 0
            || a.page_lock > 0
        {
            return 0;
        }
        if !a.fixed_memory {
            give_memory(cpu, a.base);
        }
        // Every tile of a huge block, so none still reaches the memory.
        for i in 0..a.selectors {
            let tile = sel + i * ldt::AHINCR;
            ldt::write(cpu, system, tile, ldt::segment(0, 0, access & !0x80));
        }
        if let Some(a) = arena_mut(&mut cpu.bus.win32.win16, handle) {
            a.base = 0;
        }
        return handle;
    }
    if size > MAX_ALLOC - 0x20 {
        return 0;
    }
    let size = if size == 0 {
        0x20
    } else {
        (size + 0x1F) & !0x1F
    };

    if flags & GMEM_MODIFY != 0 {
        let a = arena_mut(&mut cpu.bus.win32.win16, handle).expect("the block");
        a.flags = (a.flags & GA_DGROUP) | (flags as u8 & GA_MOVEABLE);
        if flags & GMEM_DISCARDABLE != 0 {
            a.flags |= GA_DISCARDABLE;
        }
        return handle;
    }
    if a.base != 0 && size == a.size {
        return handle;
    }

    // The memory: in place if the heap can, else a new block and a copy.
    let in_place = a.base != 0
        && !a.fixed_memory
        && super::with_win16_heap(cpu, |heap, memory, bus| {
            heap.resize_in_place(memory, bus, a.base, size)
        });
    let base = if in_place {
        a.base
    } else {
        let Some(base) = take_memory(cpu, size) else {
            return 0;
        };
        if a.base != 0 {
            copy(cpu, a.base, base, a.size.min(size));
        }
        base
    };
    // The old memory goes back to the heap once the selectors are
    // there, so a failure below leaves the block as it was.
    let moved = !in_place && a.base != 0 && !a.fixed_memory;
    if size > a.size && flags & GMEM_ZEROINIT != 0 {
        zero(cpu, base + a.size.min(size), size - a.size.min(size));
    }

    // The selectors: more of them in a row for a block that grew past a
    // 64 KB boundary, in place when the entries after it are free.
    let old_count = a.selectors;
    let count = ldt::tiles(size);
    let state = &mut cpu.bus.win32.win16;
    let mut new_sel = sel;
    if count > old_count {
        if state
            .ldt
            .free_after(sel + (old_count - 1) * ldt::AHINCR, count - old_count)
        {
            state
                .ldt
                .take_after(sel + (old_count - 1) * ldt::AHINCR, count - old_count);
        } else {
            let Some(s) = state.ldt.alloc(count) else {
                if !in_place {
                    give_memory(cpu, base);
                }
                return 0;
            };
            state.ldt.free(sel, old_count);
            new_sel = s;
        }
    } else if count < old_count {
        state.ldt.free(sel + count * ldt::AHINCR, old_count - count);
        for i in count..old_count {
            ldt::write_desc(cpu, system, sel + i * ldt::AHINCR, 0);
        }
    }
    if new_sel != sel {
        for i in 0..old_count {
            ldt::write(cpu, system, sel + i * ldt::AHINCR, 0);
        }
    }
    if moved {
        give_memory(cpu, a.base);
    }
    ldt::set_block(cpu, system, new_sel, base, size, access, big);
    let state = &mut cpu.bus.win32.win16;
    let mut entry = state.arena.remove(&ldt::index(sel)).expect("the block");
    entry.base = base;
    entry.size = size;
    entry.selectors = count;
    // Its memory is the heap's now, grown in place or moved.
    entry.fixed_memory = false;
    entry.handle = if entry.flags & GA_MOVEABLE != 0 {
        new_sel - 1
    } else {
        new_sel
    };
    let handle = entry.handle;
    state.arena.insert(ldt::index(new_sel), entry);
    handle
}

/// GlobalLock: the block's selector (with offset 0), or 0 for a block
/// that is discarded or isn't one.
pub fn lock(cpu: &mut Cpu, handle: u16) -> u16 {
    match arena_mut(&mut cpu.bus.win32.win16, handle) {
        Some(a) if a.base != 0 => {
            a.lock = a.lock.saturating_add(1);
            handle_to_sel(handle)
        }
        _ => 0,
    }
}

/// GlobalUnlock: whether the block is still locked.
pub fn unlock(cpu: &mut Cpu, handle: u16) -> bool {
    match arena_mut(&mut cpu.bus.win32.win16, handle) {
        Some(a) => {
            a.lock = a.lock.saturating_sub(1);
            a.lock > 0
        }
        None => false,
    }
}

/// GlobalSize: the block's size, 0 for a handle that isn't one.
pub fn size(cpu: &Cpu, handle: u16) -> u32 {
    arena(&cpu.bus.win32.win16, handle).map_or(0, |a| if a.base == 0 { 0 } else { a.size })
}

/// GlobalFlags.
pub fn flags(cpu: &Cpu, handle: u16) -> u16 {
    let Some(a) = arena(&cpu.bus.win32.win16, handle) else {
        return 0;
    };
    let mut flags = a.lock as u16;
    if a.flags & GA_DISCARDABLE != 0 {
        flags |= GMEM_DISCARDABLE;
    }
    if a.base == 0 {
        flags |= GMEM_DISCARDED;
    }
    flags
}

/// GlobalHandle: the handle of the block whose selector is `sel`, or 0.
pub fn handle_of(cpu: &Cpu, sel: u16) -> u16 {
    arena(&cpu.bus.win32.win16, sel).map_or(0, |a| a.handle)
}

/// Free the blocks `owner` owns, as a task's or module's end does.
pub fn free_owned(cpu: &mut Cpu, owner: u16) {
    let handles: Vec<u16> = cpu
        .bus
        .win32
        .win16
        .arena
        .values()
        .filter(|a| a.owner == owner && a.size != 0)
        .map(|a| a.handle)
        .collect();
    for h in handles {
        free(cpu, h);
    }
}

// TOOLHELP's walk of the heap.

/// GLOBALENTRY's wType: what a block holds.
pub const GT_UNKNOWN: u16 = 0;
pub const GT_DGROUP: u16 = 1;
pub const GT_DATA: u16 = 2;
pub const GT_CODE: u16 = 3;
pub const GT_TASK: u16 = 4;
pub const GT_RESOURCE: u16 = 5;
pub const GT_MODULE: u16 = 6;
pub const GT_FREE: u16 = 7;

/// A block or a free piece of the heap as TOOLHELP describes it in a
/// GLOBALENTRY.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HeapItem {
    /// Its linear address and size: 0 and 0 for a discarded block.
    pub address: u32,
    pub size: u32,
    /// Its handle, 0 for a free piece.
    pub handle: u16,
    pub lock: u16,
    pub page_lock: u16,
    /// Its owner: the task for the blocks a task allocated, else the
    /// module that owns it.
    pub owner: u16,
    /// Whether a task owns it (GF_PDB_OWNER: Windows 3.1's KERNEL has the
    /// task's PDB as the owner, and TOOLHELP gives the task).
    pub pdb_owner: bool,
    /// The arena's flags (GA_), the low byte of wFlags.
    pub flags: u8,
    /// Whether the block has a local heap.
    pub heap_present: bool,
    /// Its wType and wData: the segment's number for a module's segment,
    /// the resource's type (its GD_ value) for a resource.
    pub kind: u16,
    pub data: u16,
    /// Whether it is on the LRU list: present and discardable.
    pub lru: bool,
}

/// What TOOLHELP tells of the block whose handle or selector is
/// `handle`, or None when it isn't one.
pub fn heap_item(cpu: &Cpu, handle: u16) -> Option<HeapItem> {
    arena(&cpu.bus.win32.win16, handle).map(|a| describe(cpu, a))
}

/// The heap as GlobalFirst and GlobalNext walk it: its blocks and the
/// free pieces between them, in address order. Discarded blocks have no
/// memory, so they aren't in it.
pub fn heap_items(cpu: &Cpu) -> Vec<HeapItem> {
    let state = &cpu.bus.win32.win16;
    let mut items: Vec<HeapItem> = state
        .arena
        .values()
        .filter(|a| a.size != 0 && a.base != 0)
        .map(|a| describe(cpu, a))
        .collect();
    let mut after = None;
    while let Some((at, len, free)) = state.heap.piece_after(after) {
        after = Some(at);
        if free {
            items.push(HeapItem {
                address: at,
                size: len,
                kind: GT_FREE,
                ..HeapItem::default()
            });
        }
    }
    items.sort_by_key(|i| i.address);
    items
}

fn describe(cpu: &Cpu, a: &Arena) -> HeapItem {
    let state = &cpu.bus.win32.win16;
    let sel = handle_to_sel(a.handle);
    let task = state.tasks.iter().find(|t| t.handle == a.owner);
    let (kind, data) = kind_of(cpu, a);
    HeapItem {
        address: a.base,
        size: if a.base == 0 { 0 } else { a.size },
        handle: a.handle,
        lock: a.lock as u16,
        page_lock: a.page_lock as u16,
        owner: a.owner,
        pdb_owner: task.is_some(),
        flags: a.flags,
        heap_present: state.local_heaps.iter().any(|h| h.sel | 3 == sel | 3),
        kind,
        data,
        lru: a.base != 0 && a.flags & GA_DISCARDABLE != 0,
    }
}

/// A block's wType and wData, from what has it: a task's database, a
/// module's database, one of a module's segments or an instance's copy
/// of its automatic data segment, or a resource LoadResource loaded.
/// The blocks programs allocate are GT_UNKNOWN, as Windows 3.1 has them.
fn kind_of(cpu: &Cpu, a: &Arena) -> (u16, u16) {
    let state = &cpu.bus.win32.win16;
    if state.tasks.iter().any(|t| t.handle == a.handle) {
        return (GT_TASK, 0);
    }
    if state.modules.iter().any(|m| m.handle == a.handle) {
        return (GT_MODULE, 0);
    }
    let Some(m) = state.modules.iter().find(|m| m.handle == a.owner) else {
        return (GT_UNKNOWN, 0);
    };
    let sel = handle_to_sel(a.handle);
    let dgroup = m
        .segments
        .iter()
        .position(|s| s.handle == m.dgroup && s.handle != 0);
    let count = m.segments.len().min(u8::MAX as usize) as u8;
    let number = (1..=count).find(|&n| super::module::segment_selector(cpu, m, n) == Some(sel));
    if let Some(n) = number {
        let kind = if dgroup == Some(n as usize - 1) {
            GT_DGROUP
        } else if m.segments[n as usize - 1].flags & ne::SEG_DATA != 0 {
            GT_DATA
        } else {
            GT_CODE
        };
        return (kind, n as u16);
    }
    if let Some(i) = dgroup
        && state.tasks.iter().any(|t| t.instance == a.handle)
    {
        return (GT_DGROUP, i as u16 + 1);
    }
    if let Some(&(hrsrc, _)) = m.loaded.iter().find(|&&(_, h)| h == a.handle) {
        return (GT_RESOURCE, resource_type(m, hrsrc));
    }
    (GT_UNKNOWN, 0)
}

/// The GD_ value of the resource `hrsrc` of `m`: its type's number for
/// the types Windows numbers (1 to 15), else GD_USERDEFINED, 0.
fn resource_type(m: &super::Module16, hrsrc: u16) -> u16 {
    let Ok(parsed) = ne::Module::parse(&m.file) else {
        return 0;
    };
    match super::resource::type_of(&parsed, hrsrc).map(|t| &t.type_id) {
        Some(ne::ResId::Int(n)) if *n <= 15 => *n,
        _ => 0,
    }
}

crate::state_fields!(Arena {
    base,
    size,
    handle,
    owner,
    lock,
    page_lock,
    flags,
    selectors,
    fixed_memory
});
