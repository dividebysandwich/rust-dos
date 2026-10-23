//! The dynamic arrays: DPA, of pointers, and DSA, of structures of a
//! size. Both are structures in the program's memory, as on Windows:
//! a DPA and its array of pointers on the heap it was made with (the
//! process's, or the one DPA_CreateEx names), a DSA on the process's.
//! Sorting, searching and enumerating call the program's callbacks, so
//! they run as scripts (script.rs).
//!
//! Ported from Wine's dpa.c and dsa.c:
//!
//! Copyright 1998 Eric Kohl
//!           1998 Juergen Schmied <j.schmied@metronet.de>
//!           2000 Eric Kohl for CodeWeavers
//!
//! This library is free software; you can redistribute it and/or modify
//! it under the terms of the GNU Lesser General Public License as
//! published by the Free Software Foundation; either version 2.1 of the
//! License, or (at your option) any later version.

use super::misc::{heap_alloc, heap_free, heap_realloc, process_heap};
use super::script::{self, Arg, Ctx, Op, Script};
use super::{DPA, Module};
use crate::cpu::Cpu;

const DPAS_SORTED: u32 = 0x0001;
const DPAS_INSERTBEFORE: u32 = 0x0002;
const DPAS_INSERTAFTER: u32 = 0x0004;

/// A DPA's fields: nItemCount, ptrs, hHeap, nGrow, nMaxCount.
const D_COUNT: u32 = 0;
const D_PTRS: u32 = 4;
const D_HEAP: u32 = 8;
const D_GROW: u32 = 12;
const D_MAX: u32 = 16;
const DPA_SIZE: u32 = 20;

/// A DSA's fields: nItemCount, pData, nMaxCount, nItemSize, nGrow.
const S_COUNT: u32 = 0;
const S_DATA: u32 = 4;
const S_MAX: u32 = 8;
const S_SIZE: u32 = 12;
const S_GROW: u32 = 16;
const DSA_SIZE: u32 = 20;

fn rd(cpu: &mut Cpu, at: u32) -> u32 {
    cpu.bus.guest_read_32(at)
}

fn wr(cpu: &mut Cpu, at: u32, v: u32) {
    cpu.bus.guest_write_32(at, v);
}

/// A DPA's fields, read at once: None if its memory isn't there.
#[derive(Clone, Copy)]
struct Dpa {
    count: i32,
    ptrs: u32,
    heap: u32,
    grow: i32,
    max: i32,
}

fn read_dpa(cpu: &mut Cpu, h: u32) -> Option<Dpa> {
    let w = crate::win32::user32::read_words(cpu, h, 5);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(Dpa {
        count: w[0] as i32,
        ptrs: w[1],
        heap: w[2],
        grow: w[3] as i32,
        max: w[4] as i32,
    })
}

/// DPA_CreateEx(grow, heap).
pub fn dpa_create_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (grow, heap) = (args[0] as i32, args[1]);
    let heap = if heap != 0 { heap } else { process_heap(cpu) };
    let h = heap_alloc(cpu, heap, DPA_SIZE);
    if h != 0 {
        let grow = grow.max(8);
        let max = grow * 2;
        let ptrs = heap_alloc(cpu, heap, 4 * max as u32);
        crate::win32::user32::write_words(cpu, h, &[0, ptrs, heap, grow as u32, max as u32]);
    }
    Some(h)
}

/// DPA_Create(grow).
pub fn dpa_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    dpa_create_ex(cpu, &[args[0], 0])
}

/// DPA_Destroy(hdpa).
pub fn dpa_destroy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let h = args[0];
    if h == 0 {
        return Some(0);
    }
    let d = read_dpa(cpu, h)?;
    if d.ptrs != 0 && !heap_free(cpu, d.heap, d.ptrs) {
        return Some(0);
    }
    Some(heap_free(cpu, d.heap, h) as u32)
}

/// DPA_Grow(hdpa, grow): room for a multiple of `grow` pointers.
pub fn dpa_grow(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let h = args[0];
    if h == 0 {
        return Some(0);
    }
    let d = read_dpa(cpu, h)?;
    let grow = (args[1] as i32).max(8);
    let items = grow.saturating_mul((d.max - 1) / grow + 1);
    if items > d.max {
        let bytes = (items as u32).saturating_mul(4);
        let ptrs = if d.ptrs != 0 {
            heap_realloc(cpu, d.heap, d.ptrs, bytes)
        } else {
            heap_alloc(cpu, d.heap, bytes)
        };
        if ptrs == 0 {
            return Some(0);
        }
        wr(cpu, h + D_MAX, items as u32);
        wr(cpu, h + D_PTRS, ptrs);
    }
    wr(cpu, h + D_GROW, grow as u32);
    Some(1)
}

/// DPA_Clone(hdpa, new): `hdpa`'s pointers in `new`, or in a new DPA
/// on its heap.
pub fn dpa_clone(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, new) = (args[0], args[1]);
    if h == 0 {
        return Some(0);
    }
    let src = read_dpa(cpu, h)?;
    let to = if new == 0 {
        let to = heap_alloc(cpu, src.heap, DPA_SIZE);
        if to == 0 {
            return Some(0);
        }
        wr(cpu, to + D_HEAP, src.heap);
        wr(cpu, to + D_GROW, src.grow as u32);
        to
    } else {
        new
    };
    let mut d = read_dpa(cpu, to)?;
    if d.ptrs != 0 {
        heap_free(cpu, d.heap, d.ptrs);
        d.ptrs = 0;
    }
    let grow = d.grow.max(1);
    let items = grow * ((src.count - 1) / grow + 1);
    let ptrs = heap_alloc(cpu, d.heap, 4 * items.max(0) as u32);
    let mut data = vec![0u8; 4 * src.count.max(0) as usize];
    cpu.bus.guest_read_bytes(src.ptrs, &mut data);
    if ptrs != 0 {
        cpu.bus.guest_write_bytes(ptrs, &data);
    }
    crate::win32::user32::write_words(cpu, to, &[src.count as u32, ptrs]);
    wr(cpu, to + D_MAX, items as u32);
    Some(to)
}

/// DPA_GetPtr(hdpa, index).
pub fn dpa_get_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, i) = (args[0], args[1] as i32);
    if h == 0 {
        return Some(0);
    }
    let d = read_dpa(cpu, h)?;
    if d.ptrs == 0 || i < 0 || i >= d.count {
        return Some(0);
    }
    Some(rd(cpu, d.ptrs + 4 * i as u32))
}

/// DPA_GetPtrIndex(hdpa, p).
pub fn dpa_get_ptr_index(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let h = args[0];
    if h == 0 {
        return Some(u32::MAX);
    }
    let d = read_dpa(cpu, h)?;
    if d.ptrs == 0 {
        return Some(u32::MAX);
    }
    for i in 0..d.count.max(0) as u32 {
        if rd(cpu, d.ptrs + 4 * i) == args[1] {
            return Some(i);
        }
    }
    Some(u32::MAX)
}

/// DPA_SetPtr(hdpa, index, p), growing the array to hold `index`.
pub fn dpa_set_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, i, p) = (args[0], args[1] as i32, args[2]);
    if h == 0 || i < 0 {
        return Some(0);
    }
    let mut d = read_dpa(cpu, h)?;
    if d.count <= i {
        if d.max <= i {
            let grow = d.grow.max(1);
            let items = grow * (i / grow + 1);
            let bytes = 4 * items as u32;
            let ptrs = if d.ptrs != 0 {
                heap_realloc(cpu, d.heap, d.ptrs, bytes)
            } else {
                heap_alloc(cpu, d.heap, bytes)
            };
            if ptrs == 0 {
                return Some(0);
            }
            d.ptrs = ptrs;
            wr(cpu, h + D_MAX, items as u32);
            wr(cpu, h + D_PTRS, ptrs);
        }
        wr(cpu, h + D_COUNT, (i + 1) as u32);
    }
    wr(cpu, d.ptrs + 4 * i as u32, p);
    Some(1)
}

/// DPA_InsertPtr(hdpa, index, p): where it went, at the end for an
/// index past it.
pub fn dpa_insert_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, i, p) = (args[0], args[1] as i32, args[2]);
    if h == 0 || i < 0 {
        return Some(u32::MAX);
    }
    let d = read_dpa(cpu, h)?;
    let i = i.min(d.count);
    if dpa_set_ptr(cpu, &[h, d.count as u32, 0])? == 0 {
        return Some(u32::MAX);
    }
    let d = read_dpa(cpu, h)?;
    let n = d.count - i - 1;
    if n > 0 {
        let mut data = vec![0u8; 4 * n as usize];
        cpu.bus.guest_read_bytes(d.ptrs + 4 * i as u32, &mut data);
        cpu.bus
            .guest_write_bytes(d.ptrs + 4 * (i as u32 + 1), &data);
    }
    wr(cpu, d.ptrs + 4 * i as u32, p);
    Some(i as u32)
}

/// DPA_DeletePtr(hdpa, index): the pointer that was there.
pub fn dpa_delete_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, i) = (args[0], args[1] as i32);
    if h == 0 {
        return Some(0);
    }
    let d = read_dpa(cpu, h)?;
    if i < 0 || i >= d.count {
        return Some(0);
    }
    let p = rd(cpu, d.ptrs + 4 * i as u32);
    let n = d.count - i - 1;
    if n > 0 {
        let mut data = vec![0u8; 4 * n as usize];
        cpu.bus
            .guest_read_bytes(d.ptrs + 4 * (i as u32 + 1), &mut data);
        cpu.bus.guest_write_bytes(d.ptrs + 4 * i as u32, &data);
    }
    let count = d.count - 1;
    wr(cpu, h + D_COUNT, count as u32);
    if d.max - count >= d.grow {
        let items = (d.grow * 2).max(count);
        let ptrs = heap_realloc(cpu, d.heap, d.ptrs, 4 * items as u32);
        if ptrs == 0 {
            return Some(0);
        }
        wr(cpu, h + D_MAX, items as u32);
        wr(cpu, h + D_PTRS, ptrs);
    }
    Some(p)
}

/// DPA_DeleteAllPtrs(hdpa).
pub fn dpa_delete_all_ptrs(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let h = args[0];
    if h == 0 {
        return Some(0);
    }
    let d = read_dpa(cpu, h)?;
    if d.ptrs != 0 && !heap_free(cpu, d.heap, d.ptrs) {
        return Some(0);
    }
    let max = d.grow * 2;
    let ptrs = heap_alloc(cpu, d.heap, 4 * max as u32);
    crate::win32::user32::write_words(cpu, h, &[0, ptrs]);
    wr(cpu, h + D_MAX, max as u32);
    Some(1)
}

// The continuations of DPA's and DSA's scripts.
const SORT: u16 = 1;
const SORT_COMPARED: u16 = 2;
const SEARCH: u16 = 3;
const SEARCH_COMPARED: u16 = 4;
const ENUM: u16 = 5;
const ENUM_RETURNED: u16 = 6;

fn then(id: u16, data: &[u32]) -> Op {
    Op::Then {
        id,
        data: data.to_vec(),
    }
}

fn call(target: u32, args: &[u32]) -> Op {
    Op::Call {
        target,
        args: args.iter().map(|&a| Arg::V(a)).collect(),
    }
}

fn run(cpu: &mut Cpu, id: u16, data: &[u32], result: u32) -> Option<u32> {
    let mut s = Script::new(0);
    s.result = result;
    s.then(id, data);
    script::run(cpu, DPA as Module, s)
}

/// DPA_Sort(hdpa, compare, lParam): sorted in place by `compare`, keeping
/// the order of pointers it finds equal. A binary insertion sort, one
/// comparison at a time.
pub fn dpa_sort(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, compare, lp) = (args[0], args[1], args[2]);
    if h == 0 || compare == 0 {
        return Some(0);
    }
    let d = read_dpa(cpu, h)?;
    if d.count <= 1 || d.ptrs == 0 {
        return Some(1);
    }
    // hdpa, compare, lParam, i, lo, hi.
    run(cpu, SORT, &[h, compare, lp, 1, 0, 1], 1)
}

/// DPA_Search(hdpa, find, start, compare, lParam, options).
pub fn dpa_search(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[h, find, start, compare, lp, options] = args else {
        unreachable!()
    };
    if h == 0 || compare == 0 || find == 0 {
        return Some(u32::MAX);
    }
    let d = read_dpa(cpu, h)?;
    if options & DPAS_SORTED != 0 {
        // A binary search over the whole array: hdpa, find, compare,
        // lParam, options, l, r.
        let data = [h, find, compare, lp, options, 0, (d.count - 1) as u32];
        return run(cpu, SEARCH, &data, u32::MAX);
    }
    let start = if start as i32 == -1 { 0 } else { start };
    // A linear one from `start`, with options 0.
    let data = [h, find, compare, lp, 0, start, 0];
    run(cpu, SEARCH, &data, u32::MAX)
}

/// DPA_EnumCallback(hdpa, proc, lParam).
pub fn dpa_enum_callback(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(0);
    }
    // kind (0: DPA, 1: DSA), destroy after, h, proc, lParam, i.
    run(cpu, ENUM, &[0, 0, args[0], args[1], args[2], 0], 0)
}

/// DPA_DestroyCallback(hdpa, proc, lParam).
pub fn dpa_destroy_callback(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(0);
    }
    run(cpu, ENUM, &[0, 1, args[0], args[1], args[2], 0], 0)
}

/// DSA_EnumCallback(hdsa, proc, lParam).
pub fn dsa_enum_callback(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(0);
    }
    run(cpu, ENUM, &[1, 0, args[0], args[1], args[2], 0], 0)
}

/// DSA_DestroyCallback(hdsa, proc, lParam).
pub fn dsa_destroy_callback(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(0);
    }
    run(cpu, ENUM, &[1, 1, args[0], args[1], args[2], 0], 0)
}

/// The continuations.
pub fn then_step(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    match id {
        SORT | SORT_COMPARED => sort_step(cpu, id, c),
        SEARCH | SEARCH_COMPARED => search_step(cpu, id, c),
        _ => enum_step(cpu, id, c),
    }
}

fn sort_step(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    let (h, compare, lp) = (c.word(0), c.word(1), c.word(2));
    let (mut i, mut lo, mut hi) = (c.word(3) as i32, c.word(4) as i32, c.word(5) as i32);
    let Some(d) = read_dpa(cpu, h) else {
        return Vec::new();
    };
    if id == SORT_COMPARED {
        let mid = (lo + hi) / 2;
        if (c.last as i32) < 0 {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    loop {
        if i >= d.count {
            return Vec::new();
        }
        if lo < hi {
            let mid = (lo + hi) / 2;
            let x = rd(cpu, d.ptrs + 4 * i as u32);
            let y = rd(cpu, d.ptrs + 4 * mid as u32);
            let data = [h, compare, lp, i as u32, lo as u32, hi as u32];
            return vec![call(compare, &[x, y, lp]), then(SORT_COMPARED, &data)];
        }
        // Move the pointer at i to lo.
        if lo < i {
            let x = rd(cpu, d.ptrs + 4 * i as u32);
            let mut data = vec![0u8; 4 * (i - lo) as usize];
            cpu.bus.guest_read_bytes(d.ptrs + 4 * lo as u32, &mut data);
            cpu.bus
                .guest_write_bytes(d.ptrs + 4 * (lo as u32 + 1), &data);
            wr(cpu, d.ptrs + 4 * lo as u32, x);
        }
        i += 1;
        lo = 0;
        hi = i;
    }
}

fn search_step(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    let (h, find, compare, lp, options) = (c.word(0), c.word(1), c.word(2), c.word(3), c.word(4));
    let (mut l, mut r) = (c.word(5) as i32, c.word(6) as i32);
    let Some(d) = read_dpa(cpu, h) else {
        return Vec::new();
    };
    let sorted = options & DPAS_SORTED != 0;
    if id == SEARCH_COMPARED {
        let n = c.last as i32;
        if sorted {
            let x = (l + r) / 2;
            if n == 0 {
                c.result = x as u32;
                return Vec::new();
            } else if n < 0 {
                r = x - 1;
            } else {
                l = x + 1;
            }
        } else {
            if n == 0 {
                c.result = l as u32;
                return Vec::new();
            }
            l += 1;
        }
    }
    let data = |l: i32, r: i32| [h, find, compare, lp, options, l as u32, r as u32];
    if sorted {
        if r >= l {
            let x = (l + r) / 2;
            let p = rd(cpu, d.ptrs + 4 * x as u32);
            return vec![
                call(compare, &[find, p, lp]),
                then(SEARCH_COMPARED, &data(l, r)),
            ];
        }
        if options & (DPAS_INSERTBEFORE | DPAS_INSERTAFTER) != 0 {
            c.result = l as u32;
        }
        return Vec::new();
    }
    if l < d.count {
        let p = rd(cpu, d.ptrs + 4 * l as u32);
        return vec![
            call(compare, &[find, p, lp]),
            then(SEARCH_COMPARED, &data(l, r)),
        ];
    }
    Vec::new()
}

fn enum_step(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    let (dsa, destroy, h, proc, lp) = (c.word(0), c.word(1), c.word(2), c.word(3), c.word(4));
    let mut i = c.word(5);
    let finish = |cpu: &mut Cpu| {
        if destroy != 0 {
            if dsa != 0 {
                dsa_destroy(cpu, &[h]);
            } else {
                dpa_destroy(cpu, &[h]);
            }
        }
        Vec::new()
    };
    if id == ENUM_RETURNED {
        if c.last == 0 {
            return finish(cpu);
        }
        i += 1;
    }
    let count = rd(cpu, h) as i32;
    if cpu.bus.guest_faulted() || i as i32 >= count {
        return finish(cpu);
    }
    let item = if dsa != 0 {
        dsa_get_item_ptr(cpu, &[h, i]).unwrap_or(0)
    } else {
        let ptrs = rd(cpu, h + D_PTRS);
        rd(cpu, ptrs + 4 * i)
    };
    vec![
        call(proc, &[item, lp]),
        then(ENUM_RETURNED, &[dsa, destroy, h, proc, lp, i]),
    ]
}

// The DSA.

/// DSA_Create(size, grow).
pub fn dsa_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let heap = process_heap(cpu);
    let h = heap_alloc(cpu, heap, DSA_SIZE);
    if h != 0 {
        let grow = (args[1] as i32).max(1) as u32;
        crate::win32::user32::write_words(cpu, h, &[0, 0, 0, args[0], grow]);
    }
    Some(h)
}

/// DSA_Destroy(hdsa).
pub fn dsa_destroy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let h = args[0];
    if h == 0 {
        return Some(0);
    }
    let data = rd(cpu, h + S_DATA);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let heap = process_heap(cpu);
    if data != 0 && !heap_free(cpu, heap, data) {
        return Some(0);
    }
    Some(heap_free(cpu, heap, h) as u32)
}

/// A DSA's fields: count, data, max, item size, grow.
fn read_dsa(cpu: &mut Cpu, h: u32) -> Option<[i32; 5]> {
    let w = crate::win32::user32::read_words(cpu, h, 5);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some([
        w[0] as i32,
        w[1] as i32,
        w[2] as i32,
        w[3] as i32,
        w[4] as i32,
    ])
}

/// DSA_GetItem(hdsa, index, dest).
pub fn dsa_get_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, i, dest) = (args[0], args[1] as i32, args[2]);
    if h == 0 {
        return Some(0);
    }
    let [count, data, _, size, _] = read_dsa(cpu, h)?;
    if i < 0 || i >= count {
        return Some(0);
    }
    let mut item = vec![0u8; size.max(0) as usize];
    cpu.bus
        .guest_read_bytes(data as u32 + (size * i) as u32, &mut item);
    cpu.bus.guest_write_bytes(dest, &item);
    Some(1)
}

/// DSA_GetItemPtr(hdsa, index).
pub fn dsa_get_item_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, i) = (args[0], args[1] as i32);
    if h == 0 {
        return Some(0);
    }
    let [count, data, _, size, _] = read_dsa(cpu, h)?;
    if i < 0 || i >= count {
        return Some(0);
    }
    Some(data as u32 + (size * i) as u32)
}

/// ReAlloc of the process's heap: `at`, or a new block when it is 0.
fn realloc(cpu: &mut Cpu, at: u32, size: u32) -> u32 {
    let heap = process_heap(cpu);
    if at == 0 {
        heap_alloc(cpu, heap, size)
    } else {
        heap_realloc(cpu, heap, at, size)
    }
}

/// DSA_SetItem(hdsa, index, src).
pub fn dsa_set_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, i, src) = (args[0], args[1] as i32, args[2]);
    if h == 0 || i < 0 {
        return Some(0);
    }
    let [count, mut data, max, size, grow] = read_dsa(cpu, h)?;
    if count <= i {
        if max > i {
            wr(cpu, h + S_COUNT, (i + 1) as u32);
        } else {
            let items = grow * (i / grow.max(1) + 1);
            let to = realloc(cpu, data as u32, (size * items) as u32);
            if to == 0 {
                return Some(0);
            }
            data = to as i32;
            crate::win32::user32::write_words(cpu, h, &[(i + 1) as u32, to, items as u32]);
        }
    }
    let mut item = vec![0u8; size.max(0) as usize];
    cpu.bus.guest_read_bytes(src, &mut item);
    cpu.bus
        .guest_write_bytes(data as u32 + (size * i) as u32, &item);
    Some(1)
}

/// DSA_InsertItem(hdsa, index, src): where it went.
pub fn dsa_insert_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, i, src) = (args[0], args[1] as i32, args[2]);
    if h == 0 || i < 0 {
        return Some(u32::MAX);
    }
    let [count, mut data, max, size, grow] = read_dsa(cpu, h)?;
    let i = i.min(count);
    if count >= max {
        let items = max + grow;
        let to = realloc(cpu, data as u32, (size * items) as u32);
        if to == 0 {
            return Some(u32::MAX);
        }
        data = to as i32;
        wr(cpu, h + S_DATA, to);
        wr(cpu, h + S_MAX, items as u32);
    }
    let base = data as u32;
    if i < count {
        let mut moved = vec![0u8; ((count - i) * size) as usize];
        cpu.bus
            .guest_read_bytes(base + (size * i) as u32, &mut moved);
        cpu.bus
            .guest_write_bytes(base + (size * (i + 1)) as u32, &moved);
    }
    wr(cpu, h + S_COUNT, (count + 1) as u32);
    let mut item = vec![0u8; size.max(0) as usize];
    cpu.bus.guest_read_bytes(src, &mut item);
    cpu.bus.guest_write_bytes(base + (size * i) as u32, &item);
    Some(i as u32)
}

/// DSA_DeleteItem(hdsa, index).
pub fn dsa_delete_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (h, i) = (args[0], args[1] as i32);
    if h == 0 {
        return Some(u32::MAX);
    }
    let [count, data, max, size, grow] = read_dsa(cpu, h)?;
    if i < 0 || i >= count {
        return Some(u32::MAX);
    }
    let base = data as u32;
    if i < count - 1 {
        let mut moved = vec![0u8; ((count - i - 1) * size) as usize];
        cpu.bus
            .guest_read_bytes(base + (size * (i + 1)) as u32, &mut moved);
        cpu.bus.guest_write_bytes(base + (size * i) as u32, &moved);
    }
    let count = count - 1;
    wr(cpu, h + S_COUNT, count as u32);
    if max - count >= grow {
        let to = realloc(cpu, base, (size * count) as u32);
        if to == 0 {
            return Some(u32::MAX);
        }
        wr(cpu, h + S_MAX, count as u32);
        wr(cpu, h + S_DATA, to);
    }
    Some(i as u32)
}

/// DSA_DeleteAllItems(hdsa).
pub fn dsa_delete_all_items(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let h = args[0];
    if h == 0 {
        return Some(0);
    }
    let data = rd(cpu, h + S_DATA);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let heap = process_heap(cpu);
    if data != 0 && !heap_free(cpu, heap, data) {
        return Some(0);
    }
    crate::win32::user32::write_words(cpu, h, &[0, 0, 0]);
    Some(1)
}

#[allow(dead_code)]
const _UNUSED: (u32, u32) = (S_SIZE, S_GROW);
