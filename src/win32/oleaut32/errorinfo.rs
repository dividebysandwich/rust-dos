//! Error objects: CreateErrorInfo's, which the program fills through
//! ICreateErrorInfo and its caller reads through IErrorInfo, and the
//! error object of each thread that SetErrorInfo sets and GetErrorInfo
//! takes. Controls set one when a method fails, as OLE Automation
//! expects of them.
//!
//! What the functions do follows Wine's oleaut32/errorinfo.c (LGPL 2.1
//! or later):
//!
//! Copyright 2000 Patrik Stridvall, Juergen Schmied

use super::{dll_index, insert, vtable};
use crate::cpu::Cpu;
use crate::win32::com::{self, E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, Guid, S_FALSE, S_OK};

const IID_IERRORINFO: Guid = Guid::new(
    0x1CF2_B120,
    0x547D,
    0x101B,
    [0x8E, 0x65, 0x08, 0x00, 0x2B, 0x2B, 0xD1, 0x19],
);
const IID_ICREATEERRORINFO: Guid = Guid::new(
    0x22F0_3340,
    0x547D,
    0x101B,
    [0x8E, 0x65, 0x08, 0x00, 0x2B, 0x2B, 0xD1, 0x19],
);

/// An error object, with its two interfaces.
#[derive(Clone, Debug, Default)]
pub struct ErrorObject {
    pub pid: u32,
    pub refs: u32,
    pub guid: [u8; 16],
    pub source: Option<String>,
    pub description: Option<String>,
    pub help_file: Option<String>,
    pub help_context: u32,
    pub create: u32,
    pub error: u32,
}

crate::state_fields!(ErrorObject {
    pid,
    refs,
    guid,
    source,
    description,
    help_file,
    help_context,
    create,
    error
});

fn errors(cpu: &mut Cpu) -> &mut Vec<Option<ErrorObject>> {
    &mut cpu.bus.win32.oleaut32.errors
}

/// The error object of the interface `this`, if it is one of these.
fn object(cpu: &mut Cpu, this: u32) -> Option<usize> {
    let (v, id) = com::interface_of(cpu, dll_index(), this)?;
    if v != vtable("ICreateErrorInfo") && v != vtable("IErrorInfo") {
        return None;
    }
    errors(cpu).get(id as usize)?.as_ref()?;
    Some(id as usize)
}

fn get(cpu: &mut Cpu, id: usize) -> &mut ErrorObject {
    errors(cpu)[id].as_mut().expect("an error object")
}

/// A process ended: its objects went with its heap, and its threads'
/// error objects with them.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    for slot in errors(cpu).iter_mut() {
        if slot.as_ref().is_some_and(|o| o.pid == pid) {
            *slot = None;
        }
    }
    cpu.bus
        .win32
        .oleaut32
        .thread_errors
        .retain(|&(p, _, _)| p != pid);
}

/// CreateErrorInfo(out): a new object's ICreateErrorInfo.
pub fn create_error_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let out = args[0];
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let object = ErrorObject {
        pid: crate::win32::user32::current_pid(cpu),
        refs: 1,
        ..ErrorObject::default()
    };
    let id = insert(errors(cpu), object);
    let create = com::new_interface(cpu, dll_index(), vtable("ICreateErrorInfo"), id as u32, 0);
    let error = com::new_interface(cpu, dll_index(), vtable("IErrorInfo"), id as u32, 0);
    let (Some(create), Some(error)) = (create, error) else {
        for at in [create, error].into_iter().flatten() {
            com::free_interface(cpu, at);
        }
        errors(cpu)[id] = None;
        com::write_out(cpu, out, 0);
        return Some(E_OUTOFMEMORY);
    };
    let o = get(cpu, id);
    o.create = create;
    o.error = error;
    com::write_out(cpu, out, create);
    Some(S_OK)
}

fn add_ref(cpu: &mut Cpu, id: usize) -> u32 {
    let o = get(cpu, id);
    o.refs = o.refs.saturating_add(1);
    o.refs
}

fn release(cpu: &mut Cpu, id: usize) -> u32 {
    let o = get(cpu, id);
    o.refs = o.refs.saturating_sub(1);
    if o.refs > 0 {
        return o.refs;
    }
    let o = errors(cpu)[id].take().expect("an error object");
    com::free_interface(cpu, o.create);
    com::free_interface(cpu, o.error);
    0
}

pub fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, iid, out] = args else {
        unreachable!()
    };
    let (Some(id), Some(iid)) = (object(cpu, this), Guid::read(cpu, iid)) else {
        return Some(E_INVALIDARG);
    };
    let o = get(cpu, id);
    let iface = match iid {
        _ if iid == com::IID_IUNKNOWN || iid == IID_IERRORINFO => o.error,
        _ if iid == IID_ICREATEERRORINFO => o.create,
        _ => {
            com::write_out(cpu, out, 0);
            return Some(E_NOINTERFACE);
        }
    };
    add_ref(cpu, id);
    com::write_out(cpu, out, iface);
    Some(S_OK)
}

pub fn method_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(object(cpu, args[0]).map_or(0, |id| add_ref(cpu, id)))
}

pub fn method_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(object(cpu, args[0]).map_or(0, |id| release(cpu, id)))
}

pub fn set_guid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (Some(id), Some(guid)) = (object(cpu, args[0]), Guid::read(cpu, args[1])) else {
        return Some(E_INVALIDARG);
    };
    get(cpu, id).guid = guid.0;
    Some(S_OK)
}

/// Set one of the strings, a copy of the program's.
fn set_string(
    cpu: &mut Cpu,
    args: &[u32],
    field: fn(&mut ErrorObject) -> &mut Option<String>,
) -> Option<u32> {
    let Some(id) = object(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let s = super::wide_string(cpu, args[1]);
    *field(get(cpu, id)) = s;
    Some(S_OK)
}

pub fn set_source(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_string(cpu, args, |o| &mut o.source)
}

pub fn set_description(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_string(cpu, args, |o| &mut o.description)
}

pub fn set_help_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_string(cpu, args, |o| &mut o.help_file)
}

pub fn set_help_context(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    get(cpu, id).help_context = args[1];
    Some(S_OK)
}

pub fn get_guid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let guid = Guid(get(cpu, id).guid);
    Some(if guid.write(cpu, args[1]) {
        S_OK
    } else {
        E_INVALIDARG
    })
}

/// Give one of the strings as a new BSTR.
fn get_string(
    cpu: &mut Cpu,
    args: &[u32],
    field: fn(&ErrorObject) -> &Option<String>,
) -> Option<u32> {
    let Some(id) = object(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_INVALIDARG);
    }
    let s = field(get(cpu, id)).clone();
    let b = s.map_or(0, |s| super::bstr(cpu, &s));
    com::write_out(cpu, args[1], b);
    Some(S_OK)
}

pub fn get_source(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_string(cpu, args, |o| &o.source)
}

pub fn get_description(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_string(cpu, args, |o| &o.description)
}

pub fn get_help_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_string(cpu, args, |o| &o.help_file)
}

pub fn get_help_context(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let context = get(cpu, id).help_context;
    Some(if com::write_out(cpu, args[1], context) {
        S_OK
    } else {
        E_INVALIDARG
    })
}

/// A reference added to or released from the interface `iface`: at once
/// for one of these objects, else a call of its method `index` to make.
fn count(cpu: &mut Cpu, iface: u32, index: u32, calls: &mut Vec<(u32, u32)>) {
    match object(cpu, iface) {
        Some(id) if index == 1 => {
            add_ref(cpu, id);
        }
        Some(id) => {
            release(cpu, id);
        }
        None => {
            if let Some(method) = crate::win32::ole32::method(cpu, iface, index) {
                calls.push((method, iface));
            }
        }
    }
}

/// SetErrorInfo(reserved, error): the thread's error object, which holds
/// a reference to it; NULL clears it.
pub fn set_error_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let error = args[1];
    let pid = crate::win32::user32::current_pid(cpu);
    let tid = crate::win32::user32::current_tid(cpu);
    let slots = &mut cpu.bus.win32.oleaut32.thread_errors;
    let old = slots
        .iter()
        .position(|&(_, t, _)| t == tid)
        .map(|i| slots.remove(i).2);
    if error != 0 {
        slots.push((pid, tid, error));
    }
    let mut calls = Vec::new();
    if error != 0 {
        count(cpu, error, 1, &mut calls);
    }
    if let Some(old) = old {
        count(cpu, old, 2, &mut calls);
    }
    crate::win32::ole32::call_all(cpu, S_OK, &calls)
}

/// GetErrorInfo(reserved, out): the thread's error object, which it no
/// longer has; S_FALSE when there is none.
pub fn get_error_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let out = args[1];
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let tid = crate::win32::user32::current_tid(cpu);
    let slots = &mut cpu.bus.win32.oleaut32.thread_errors;
    let error = slots
        .iter()
        .position(|&(_, t, _)| t == tid)
        .map(|i| slots.remove(i).2);
    com::write_out(cpu, out, error.unwrap_or(0));
    Some(if error.is_some() { S_OK } else { S_FALSE })
}
