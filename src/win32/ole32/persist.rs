//! The helpers that save and load objects and their classes through
//! streams and storages: WriteClassStm, ReadClassStm, WriteClassStg,
//! ReadClassStg, OleSaveToStream and OleLoadFromStream. The streams and
//! storages may be the program's, so each is a task that calls their
//! methods (calls.rs).
//!
//! What they do follows Wine's ole32/storage32.c and ole2.c (LGPL 2.1 or
//! later):
//!
//! Copyright 1999 Francis Beaudet
//! Copyright 1999 Sylvain St-Germain
//! Copyright 1999 Thuy Nguyen
//! Copyright 2005 Mike McCormack

use super::calls;
use super::storage::STG_E_READFAULT;
use crate::cpu::Cpu;
use crate::win32::com::{E_INVALIDARG, Guid};
use crate::win32::task::{Next, Task};

/// Which helper the task is: its variable 1.
const WRITE_CLASS_STM: u32 = 0;
const READ_CLASS_STM: u32 = 1;
const WRITE_CLASS_STG: u32 = 2;
const READ_CLASS_STG: u32 = 3;
const SAVE_TO_STREAM: u32 = 4;
const LOAD_FROM_STREAM: u32 = 5;

/// The variables after it: the helper's arguments.
const A: usize = 2;
const B: usize = 3;
const C: usize = 4;

/// The scratch space: a CLSID (or a STATSTG), a count, an interface,
/// IID_IPersistStream.
const S_CLSID: u32 = 0;
const S_COUNT: u32 = 72;
const S_IFACE: u32 = 76;
const S_IID: u32 = 80;
const SCRATCH: u32 = 96;

const IID_IPERSISTSTREAM: Guid = Guid::new(0x109, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const CLSCTX_SERVER: u32 = 0x15;
const STATFLAG_NONAME: u32 = 1;

/// IStream's Read and Write, IStorage's SetClass and Stat, IPersist's
/// GetClassID, IPersistStream's Load and Save.
const READ: u32 = 3;
const WRITE: u32 = 4;
const SET_CLASS: u32 = 15;
const STAT: u32 = 17;
const GET_CLASS_ID: u32 = 3;
const LOAD: u32 = 5;
const SAVE: u32 = 6;
const QUERY_INTERFACE: u32 = 0;
const RELEASE: u32 = 2;

fn start(cpu: &mut Cpu, helper: u32, args: &[u32]) -> Option<u32> {
    let mut vars = vec![calls::PERSIST, helper];
    vars.extend_from_slice(args);
    calls::start(cpu, vars, SCRATCH)
}

/// WriteClassStm(stream, clsid): the CLSID written to the stream.
pub fn write_class_stm(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 || args[1] == 0 {
        return Some(E_INVALIDARG);
    }
    start(cpu, WRITE_CLASS_STM, args)
}

/// ReadClassStm(stream, clsid): a CLSID read from the stream;
/// STG_E_READFAULT when it ends first.
pub fn read_class_stm(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 || args[1] == 0 {
        return Some(E_INVALIDARG);
    }
    start(cpu, READ_CLASS_STM, args)
}

/// WriteClassStg(storage, clsid): IStorage::SetClass.
pub fn write_class_stg(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(E_INVALIDARG);
    }
    start(cpu, WRITE_CLASS_STG, args)
}

/// ReadClassStg(storage, clsid): the class IStorage::Stat gives.
pub fn read_class_stg(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 || args[1] == 0 {
        return Some(E_INVALIDARG);
    }
    start(cpu, READ_CLASS_STG, args)
}

/// OleSaveToStream(object, stream): the object's class, then the object
/// as its IPersistStream::Save writes it.
pub fn ole_save_to_stream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 || args[1] == 0 {
        return Some(E_INVALIDARG);
    }
    start(cpu, SAVE_TO_STREAM, args)
}

/// OleLoadFromStream(stream, iid, out): an object of the class the
/// stream names, made with CoCreateInstance and loaded with its
/// IPersistStream::Load.
pub fn ole_load_from_stream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 || args[2] == 0 {
        return Some(E_INVALIDARG);
    }
    start(cpu, LOAD_FROM_STREAM, args)
}

fn failed(hr: u32) -> bool {
    hr & 0x8000_0000 != 0
}

/// A call of the method `index` of the interface `this`, going on at
/// `step`; E_INVALIDARG when `this` has no vtable.
fn method(cpu: &mut Cpu, this: u32, index: u32, args: &[u32], step: u16) -> Next {
    match super::method(cpu, this, index) {
        Some(m) => {
            let mut all = vec![this];
            all.extend_from_slice(args);
            Next::call(m, &all, step)
        }
        None => Next::Done(E_INVALIDARG),
    }
}

pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let s = t.scratch;
    let (a, b, c) = (t.var(A), t.var(B), t.var(C));
    match (t.var(1), t.step) {
        (WRITE_CLASS_STM, 0) => {
            let mut clsid = [0; 16];
            cpu.bus.guest_read_bytes(b, &mut clsid);
            cpu.bus.guest_write_bytes(s + S_CLSID, &clsid);
            method(cpu, a, WRITE, &[s + S_CLSID, 16, 0], 1)
        }
        (READ_CLASS_STM, 0) => {
            cpu.bus.guest_write_32(s + S_COUNT, 0);
            method(cpu, a, READ, &[s + S_CLSID, 16, s + S_COUNT], 1)
        }
        (READ_CLASS_STM, _) => {
            if failed(value) {
                return Next::Done(value);
            }
            if cpu.bus.guest_read_32(s + S_COUNT) != 16 {
                return Next::Done(STG_E_READFAULT);
            }
            copy_clsid(cpu, s + S_CLSID, b);
            Next::Done(0)
        }
        (WRITE_CLASS_STG, 0) => method(cpu, a, SET_CLASS, &[b], 1),
        (READ_CLASS_STG, 0) => method(cpu, a, STAT, &[s, STATFLAG_NONAME], 1),
        (READ_CLASS_STG, _) => {
            if !failed(value) {
                copy_clsid(cpu, s + 48, b);
            }
            Next::Done(value)
        }
        (SAVE_TO_STREAM, 0) => method(cpu, a, GET_CLASS_ID, &[s + S_CLSID], 1),
        (SAVE_TO_STREAM, 1) => {
            if failed(value) {
                return Next::Done(value);
            }
            method(cpu, b, WRITE, &[s + S_CLSID, 16, 0], 2)
        }
        (SAVE_TO_STREAM, 2) => {
            if failed(value) {
                return Next::Done(value);
            }
            method(cpu, a, SAVE, &[b, 1], 3)
        }
        (LOAD_FROM_STREAM, 0) => {
            cpu.bus.guest_write_32(c, 0);
            cpu.bus.guest_write_32(s + S_COUNT, 0);
            method(cpu, a, READ, &[s + S_CLSID, 16, s + S_COUNT], 1)
        }
        (LOAD_FROM_STREAM, 1) => {
            if failed(value) {
                return Next::Done(value);
            }
            if cpu.bus.guest_read_32(s + S_COUNT) != 16 {
                return Next::Done(STG_E_READFAULT);
            }
            let create = super::address_of("CoCreateInstance");
            Next::call(create, &[s + S_CLSID, 0, CLSCTX_SERVER, b, c], 2)
        }
        (LOAD_FROM_STREAM, 2) => {
            if failed(value) {
                return Next::Done(value);
            }
            IID_IPERSISTSTREAM.write(cpu, s + S_IID);
            cpu.bus.guest_write_32(s + S_IFACE, 0);
            let object = cpu.bus.guest_read_32(c);
            method(cpu, object, QUERY_INTERFACE, &[s + S_IID, s + S_IFACE], 3)
        }
        (LOAD_FROM_STREAM, 3) => {
            if failed(value) {
                return Next::Done(value);
            }
            let persist = cpu.bus.guest_read_32(s + S_IFACE);
            method(cpu, persist, LOAD, &[a], 4)
        }
        (LOAD_FROM_STREAM, 4) => {
            // Load's result is the answer, after the release.
            t.set(A, value);
            let persist = cpu.bus.guest_read_32(s + S_IFACE);
            method(cpu, persist, RELEASE, &[], 5)
        }
        (LOAD_FROM_STREAM, _) => Next::Done(t.var(A)),
        _ => Next::Done(value),
    }
}

fn copy_clsid(cpu: &mut Cpu, from: u32, to: u32) {
    if to == 0 || !cpu.bus.guest_probe(to, 16, true) {
        return;
    }
    let mut clsid = [0; 16];
    cpu.bus.guest_read_bytes(from, &mut clsid);
    cpu.bus.guest_write_bytes(to, &clsid);
}
