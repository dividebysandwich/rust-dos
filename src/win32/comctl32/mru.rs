//! The most-recently-used lists of the shell and its programs:
//! CreateMRUListA and the functions on them.
//!
//! A list keeps up to `nMaxItems` strings or blocks of data under a
//! registry key, as Windows keeps them: each in a value named by a
//! letter ("a", "b", ...), and the letters from the most recently used
//! to the least in the value "MRUList". Adding an item that is there
//! already moves its letter to the front; adding one more than fit
//! reuses the letter of the least recently used. The list is written
//! to the registry at each change, or when it is freed with
//! MRUF_DELAYED_SAVE.
//!
//! Items are compared with the program's function, which a script
//! calls for each item, or without one as Windows does: strings without
//! regard to case, data byte by byte.
//!
//! Ported from Wine's comctl32undoc.c:
//!
//! Copyright 1998 Eric Kohl
//! Copyright 1998 Juergen Schmied <j.schmied@metronet.de>
//! Copyright 2000 Jason Mawdsley
//!
//! This library is free software; you can redistribute it and/or modify
//! it under the terms of the GNU Lesser General Public License as
//! published by the Free Software Foundation; either version 2.1 of the
//! License, or (at your option) any later version.

use super::script::{self, Arg, Ctx, Op, Script};
use super::{MRU, state};
use crate::cpu::Cpu;
use crate::win32::registry::{self, Open, REG_BINARY, REG_SZ};
use crate::win32::user32::{self, read_words};
use std::collections::BTreeMap;

const MRUF_BINARY_LIST: u32 = 1;
const MRUF_DELAYED_SAVE: u32 = 2;
/// sizeof(CREATEMRULISTA).
const CREATE_SIZE: u32 = 24;
const ERROR_INVALID_PARAMETER: u32 = 87;
/// The most items a list may have: the letters a to z.
const MAX_ITEMS: u32 = 26;
/// Where the lists' handles start: any value a program keeps.
const HANDLE_BASE: u32 = 0x4D52_0000;

// The continuations.
const T_COMPARED: u16 = 1;
const T_NOT_FOUND: u16 = 2;

// What a search is for.
const FIND: u32 = 0;
const ADD: u32 = 1;

/// A list.
#[derive(Clone, Debug, Default)]
pub struct List {
    pub max: u32,
    pub flags: u32,
    /// The key the program gave, and the subkey of it the list is in.
    pub hkey: u32,
    pub subkey: Vec<u8>,
    pub compare: u32,
    /// The items, by letter; strings with their 0.
    pub items: Vec<Option<Vec<u8>>>,
    /// The letters' numbers, most recently used first.
    pub order: Vec<u8>,
    /// Whether it changed since it was written.
    pub changed: bool,
}

crate::state_fields!(List {
    max,
    flags,
    hkey,
    subkey,
    compare,
    items,
    order,
    changed
});

/// What the module keeps, in COMCTL32's `State`: the lists, by handle.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub lists: BTreeMap<u32, List>,
    pub next: u32,
}

crate::state_fields!(State { lists, next });

fn get(cpu: &mut Cpu, handle: u32) -> Option<List> {
    state(cpu).mru.lists.get(&handle).cloned()
}

fn put(cpu: &mut Cpu, handle: u32, list: List) {
    state(cpu).mru.lists.insert(handle, list);
}

fn binary(list: &List) -> bool {
    list.flags & MRUF_BINARY_LIST != 0
}

/// The registry key of the list: its subkey of the program's key, made
/// if it isn't there. None when the program's key is gone.
fn key_of(cpu: &mut Cpu, list: &List) -> Option<u32> {
    let pid = user32::current_pid(cpu);
    let reg = registry::get(cpu);
    let Open::Key(key) = reg.open(list.hkey, pid) else {
        return None;
    };
    let path = crate::dosstr::from_bytes(&list.subkey);
    Some(reg.create(key, &path).0)
}

/// The value name of the letter `n`.
fn letter(n: u8) -> String {
    char::from(b'a' + n).to_string()
}

/// Write the list to the registry (MRU_SaveChanged).
fn save(cpu: &mut Cpu, list: &mut List) {
    let Some(key) = key_of(cpu, list) else {
        return;
    };
    let kind = if binary(list) { REG_BINARY } else { REG_SZ };
    let reg = registry::get(cpu);
    let mut order: Vec<u8> = list.order.iter().map(|&n| b'a' + n).collect();
    order.push(0);
    reg.set_value(key, "MRUList", REG_SZ, order);
    for (n, item) in list.items.iter().enumerate() {
        match item {
            Some(data) => reg.set_value(key, &letter(n as u8), kind, data.clone()),
            None => {
                reg.delete_value(key, &letter(n as u8));
            }
        }
    }
    list.changed = false;
    registry::save(cpu);
}

/// Read the list from the registry (CreateMRUListLazy_common).
fn load(cpu: &mut Cpu, list: &mut List) -> bool {
    let Some(key) = key_of(cpu, list) else {
        return false;
    };
    let reg = registry::get(cpu);
    let order = reg
        .value(key, "MRUList")
        .map(|v| v.data.clone())
        .unwrap_or_default();
    let mut seen = vec![false; list.max as usize];
    for &b in order.iter().take_while(|&&b| b != 0) {
        let n = b.wrapping_sub(b'a');
        if (n as u32) < list.max && !seen[n as usize] {
            seen[n as usize] = true;
            list.order.push(n);
        }
    }
    list.items = vec![None; list.max as usize];
    for &n in &list.order {
        let data = reg.value(key, &letter(n)).map(|v| v.data.clone());
        list.items[n as usize] = Some(data.unwrap_or_default());
    }
    true
}

/// CreateMRUListLazyA(info, ...): a list from the CREATEMRULISTA at
/// `info`, or 0 for a wrong size, no key or no subkey.
pub fn create_mru_list_lazy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let w = read_words(cpu, args[0], 6);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let [size, max, flags, hkey, subkey, compare] = [w[0], w[1], w[2], w[3], w[4], w[5]];
    if size != CREATE_SIZE || hkey == 0 || subkey == 0 {
        return Some(0);
    }
    let subkey = user32::bytes_at(cpu, subkey)?;
    let mut list = List {
        max: max.min(MAX_ITEMS),
        flags,
        hkey,
        subkey,
        compare,
        ..List::default()
    };
    if !load(cpu, &mut list) {
        return Some(0);
    }
    let st = &mut state(cpu).mru;
    st.next += 1;
    let handle = HANDLE_BASE + 0x10 * st.next;
    st.lists.insert(handle, list);
    Some(handle)
}

/// CreateMRUListA(info).
pub fn create_mru_list(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create_mru_list_lazy(cpu, &[args[0], 0, 0, 0])
}

/// FreeMRUList(list): written if it waited to be.
pub fn free_mru_list(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(mut list) = state(cpu).mru.lists.remove(&args[0])
        && list.changed
    {
        save(cpu, &mut list);
    }
    Some(0)
}

/// Whether `a` and `b` are the same item, compared as Windows does
/// without a function of the program's.
fn same(list: &List, a: &[u8], b: &[u8], len: usize) -> bool {
    if binary(list) {
        a.len() >= len && b.len() >= len && a[..len] == b[..len]
    } else {
        let strip = |s: &[u8]| s.split(|&c| c == 0).next().unwrap_or(&[]).to_vec();
        strip(a).eq_ignore_ascii_case(&strip(b))
    }
}

/// Put the item `data` at the front of the list, at the letter `found`
/// when it is there already (AddMRUData): its letter's number.
fn add(cpu: &mut Cpu, handle: u32, found: Option<u8>, data: Vec<u8>) -> u32 {
    let Some(mut list) = get(cpu, handle) else {
        return u32::MAX;
    };
    let n = match found {
        Some(n) => n,
        None => {
            let free = list.items.iter().position(Option::is_none);
            let n = match free {
                Some(n) => n as u8,
                None => match list.order.last() {
                    Some(&n) => n,
                    None => return u32::MAX,
                },
            };
            list.items[n as usize] = Some(data);
            n
        }
    };
    list.order.retain(|&m| m != n);
    list.order.insert(0, n);
    list.changed = true;
    if list.flags & MRUF_DELAYED_SAVE == 0 {
        save(cpu, &mut list);
    }
    put(cpu, handle, list);
    n as u32
}

/// What the program's item is, read again: a string with its 0, or
/// `len` bytes of data.
fn item_of(cpu: &mut Cpu, list: &List, at: u32, len: u32) -> Option<Vec<u8>> {
    if binary(list) {
        // No program keeps more than 16 MB in the registry for a list.
        let mut b = vec![0; len.min(0x100_0000) as usize];
        cpu.bus.guest_read_bytes(at, &mut b);
        (!cpu.bus.guest_faulted()).then_some(b)
    } else {
        let mut b = user32::bytes_at(cpu, at)?;
        b.push(0);
        Some(b)
    }
}

/// Look for the item at `at` (`len` bytes for data) in the list, then
/// add it (`what` ADD) or return where it is (FIND), with its letter
/// written at `reg_num`.
fn search(cpu: &mut Cpu, handle: u32, at: u32, len: u32, what: u32, reg_num: u32) -> Option<u32> {
    let Some(list) = get(cpu, handle) else {
        return Some(u32::MAX);
    };
    let item = item_of(cpu, &list, at, len)?;
    let done = |cpu: &mut Cpu, found: Option<u8>| -> Option<u32> {
        if what == ADD {
            return Some(add(cpu, handle, found, item.clone()));
        }
        let Some(n) = found else {
            return Some(u32::MAX);
        };
        if reg_num != 0 {
            cpu.bus.guest_write_32(reg_num, (b'a' + n) as u32);
            if cpu.bus.guest_faulted() {
                return None;
            }
        }
        Some(n as u32)
    };
    let present: Vec<(u8, Vec<u8>)> = list
        .items
        .iter()
        .enumerate()
        .filter_map(|(n, d)| d.as_ref().map(|d| (n as u8, d.clone())))
        .collect();
    if list.compare == 0 {
        let found = present
            .iter()
            .find(|(_, d)| same(&list, &item, d, len as usize))
            .map(|&(n, _)| n);
        return done(cpu, found);
    }
    // The program's function compares: each item is put in the scratch
    // space for it, and the first it calls the same ends the search.
    let mut s = Script::new(0);
    let mut calls = Vec::new();
    for (n, data) in &present {
        let off = s.data(&script::words_of(data));
        calls.push((*n, off));
    }
    for (n, off) in calls {
        let mut args = vec![Arg::V(at), Arg::S(off)];
        if binary(&list) {
            args.push(Arg::V(len));
        }
        s.call(list.compare, &args);
        s.then(T_COMPARED, &[handle, what, n as u32, at, len, reg_num]);
    }
    s.then(T_NOT_FOUND, &[handle, what, 0, at, len, reg_num]);
    script::run(cpu, MRU, s)
}

/// The continuations: after the program's function compared an item,
/// and after it found none the same.
pub fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    let (handle, what, n, at, len, reg_num) = (
        c.word(0),
        c.word(1),
        c.word(2) as u8,
        c.word(3),
        c.word(4),
        c.word(5),
    );
    let found = match id {
        T_COMPARED if c.last == 0 => Some(n),
        T_COMPARED => return Vec::new(),
        T_NOT_FOUND => None,
        _ => return Vec::new(),
    };
    let result = if what == ADD {
        let Some(list) = get(cpu, handle) else {
            return vec![Op::Return(Arg::V(u32::MAX))];
        };
        match item_of(cpu, &list, at, len) {
            Some(item) => add(cpu, handle, found, item),
            None => u32::MAX,
        }
    } else {
        match found {
            Some(n) => {
                if reg_num != 0 {
                    cpu.bus.guest_write_32(reg_num, (b'a' + n) as u32);
                }
                n as u32
            }
            None => u32::MAX,
        }
    };
    vec![Op::Return(Arg::V(result))]
}

/// AddMRUStringA(list, string): the letter's number it is kept at; -1
/// without a list, 0 with ERROR_INVALID_PARAMETER for a bad string.
pub fn add_mru_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, text] = args else {
        unreachable!()
    };
    if !state(cpu).mru.lists.contains_key(&handle) {
        return Some(u32::MAX);
    }
    if text == 0 || !cpu.bus.guest_probe(text, 1, false) {
        user32::set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    search(cpu, handle, text, 0, ADD, 0)
}

/// AddMRUData(list, data, size).
pub fn add_mru_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, data, len] = args else {
        unreachable!()
    };
    if !state(cpu).mru.lists.contains_key(&handle) || len > 0x1_0000 {
        return Some(u32::MAX);
    }
    search(cpu, handle, data, len, ADD, 0)
}

/// FindMRUStringA(list, string, regnum): the number of the item's
/// letter, which goes at `regnum` too, or -1.
pub fn find_mru_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, text, reg_num] = args else {
        unreachable!()
    };
    if text == 0 {
        return Some(u32::MAX);
    }
    search(cpu, handle, text, 0, FIND, reg_num)
}

/// FindMRUData(list, data, size, regnum).
pub fn find_mru_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, data, len, reg_num] = args else {
        unreachable!()
    };
    if len > 0x1_0000 {
        return Some(u32::MAX);
    }
    search(cpu, handle, data, len, FIND, reg_num)
}

/// EnumMRUListA(list, pos, buffer, size): the item `pos` places from
/// the front, copied to the buffer as far as it fits. For a string, its
/// length, and it is cut short with a 0; for data, the bytes copied.
/// With no buffer or a negative place, how many items there are; past
/// the end, -1.
pub fn enum_mru_list(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, pos, buffer, size] = args else {
        unreachable!()
    };
    let Some(list) = get(cpu, handle) else {
        return Some(u32::MAX);
    };
    if (pos as i32) < 0 || buffer == 0 {
        return Some(list.order.len() as u32);
    }
    let Some(&n) = list.order.get(pos as usize) else {
        return Some(u32::MAX);
    };
    let data = list.items[n as usize].clone().unwrap_or_default();
    let count = data.len().min(size as usize);
    if binary(&list) {
        cpu.bus.guest_write_bytes(buffer, &data[..count]);
        if cpu.bus.guest_faulted() {
            return None;
        }
        return Some(count as u32);
    }
    let text: Vec<u8> = data.split(|&c| c == 0).next().unwrap_or(&[]).to_vec();
    if count > 0 {
        let mut out = text.clone();
        out.push(0);
        out.truncate(count);
        out[count - 1] = 0;
        cpu.bus.guest_write_bytes(buffer, &out);
        if cpu.bus.guest_faulted() {
            return None;
        }
    }
    Some(text.len() as u32)
}

/// DelMRUString(list, pos): take out the item `pos` places from the
/// front; FALSE when there is none there.
pub fn del_mru_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, pos] = args else { unreachable!() };
    let Some(mut list) = get(cpu, handle) else {
        return Some(0);
    };
    if pos as usize >= list.order.len() {
        return Some(0);
    }
    let n = list.order.remove(pos as usize);
    list.items[n as usize] = None;
    list.changed = true;
    if list.flags & MRUF_DELAYED_SAVE == 0 {
        save(cpu, &mut list);
    }
    put(cpu, handle, list);
    Some(1)
}
