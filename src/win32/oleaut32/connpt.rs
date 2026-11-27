//! The connection points of OLE's standard font and picture objects
//! (font.rs, picture.rs): a program advises one of its sinks of an
//! object's changes through IConnectionPoint::Advise, and each property
//! put calls the sinks. A font has two points, for IPropertyNotifySink
//! and IFontEventsDisp; a picture one, for IPropertyNotifySink.
//!
//! A point's interface is a block of its own, its id the object's and its
//! extra word the object's kind and the point's place. Its references are
//! its object's, which keeps the point with it.
//!
//! What the methods do follows Wine's oleaut32/connpt.c (LGPL 2.1 or
//! later):
//!
//! Copyright 2000 Huw D M Davies for CodeWeavers

use super::{dll_index, font, picture, vtable};
use crate::cpu::Cpu;
use crate::win32::com::{self, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_POINTER, Guid, S_OK};
use crate::win32::task::{self, Next, Task};

/// The kinds of objects that have points, in a point's extra word.
pub const FONT: u32 = 0;
pub const PICTURE: u32 = 1;

pub const IID_ICONNECTIONPOINTCONTAINER: Guid = Guid::new(
    0xB196_B284,
    0xBAB4,
    0x101A,
    [0xB6, 0x9C, 0x00, 0xAA, 0x00, 0x34, 0x1D, 0x07],
);
const IID_ICONNECTIONPOINT: Guid = Guid::new(
    0xB196_B286,
    0xBAB4,
    0x101A,
    [0xB6, 0x9C, 0x00, 0xAA, 0x00, 0x34, 0x1D, 0x07],
);
pub const IID_IPROPERTYNOTIFYSINK: Guid = Guid::new(
    0x9BFB_BC02,
    0xEFF1,
    0x101A,
    [0x84, 0xED, 0x00, 0xAA, 0x00, 0x34, 0x1D, 0x07],
);
pub const IID_IFONTEVENTSDISP: Guid = Guid::new(
    0x4EF6_100A,
    0xAF88,
    0x11D0,
    [0x98, 0x46, 0x00, 0xC0, 0x4F, 0xC2, 0x99, 0x93],
);

/// HRESULTs.
pub const CONNECT_E_NOCONNECTION: u32 = 0x8004_0200;
const CONNECT_E_CANNOTCONNECT: u32 = 0x8004_0202;
const OLE_E_NOCONNECTION: u32 = 0x8004_0004;

/// IUnknown's QueryInterface and Release, IPropertyNotifySink's
/// OnChanged, IDispatch's Invoke.
const QUERY_INTERFACE: u32 = 0;
const RELEASE: u32 = 2;
const ON_CHANGED: u32 = 3;
const INVOKE: u32 = 6;

/// IFontEventsDisp's FontChanged, and IDispatch::Invoke's call of a
/// method.
const DISPID_FONT_CHANGED: u32 = 9;
const DISPATCH_METHOD: u32 = 1;
const VT_BSTR: u16 = 8;

/// The sink interface of the point `point` of an object of `kind`.
fn sink_iid(kind: u32, point: u32) -> Guid {
    if kind == FONT && point == font::FONT_EVENTS {
        IID_IFONTEVENTSDISP
    } else {
        IID_IPROPERTYNOTIFYSINK
    }
}

/// A new point of the object `id` of `kind`: its interface, or None when
/// the heap is full.
pub fn new_point(cpu: &mut Cpu, kind: u32, id: usize, point: u32) -> Option<u32> {
    let at = com::new_interface(cpu, dll_index(), vtable("IConnectionPoint"), id as u32, 4)?;
    cpu.bus.guest_write_32(at + com::HEADER, kind << 8 | point);
    Some(at)
}

/// The point `this` is: its object's kind and id, and its place.
fn point_of(cpu: &mut Cpu, this: u32) -> Option<(u32, usize, u32)> {
    let (v, id) = com::interface_of(cpu, dll_index(), this)?;
    if v != vtable("IConnectionPoint") {
        return None;
    }
    let word = cpu.bus.guest_read_32(this + com::HEADER);
    let (kind, point) = (word >> 8, word & 0xFF);
    sinks(cpu, kind, id as usize, point)?;
    Some((kind, id as usize, point))
}

/// The sinks of a point, by their cookies less one: the interface Advise
/// got from each sink, 0 for a free cookie. None when the object is gone.
fn sinks(cpu: &mut Cpu, kind: u32, id: usize, point: u32) -> Option<&mut Vec<u32>> {
    match kind {
        FONT => font::sinks(cpu, id, point),
        PICTURE => picture::sinks(cpu, id, point),
        _ => None,
    }
}

fn add_ref(cpu: &mut Cpu, kind: u32, id: usize) -> u32 {
    match kind {
        FONT => font::add_ref(cpu, id),
        _ => picture::add_ref(cpu, id),
    }
}

/// The calls that release each sink of `sinks`, for an object that goes.
pub fn release_calls(cpu: &mut Cpu, sinks: impl IntoIterator<Item = u32>) -> Vec<(u32, u32)> {
    sinks
        .into_iter()
        .filter(|&s| s != 0)
        .filter_map(|s| crate::win32::ole32::method(cpu, s, RELEASE).map(|m| (m, s)))
        .collect()
}

pub fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, iid, out] = args else {
        unreachable!()
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let (Some((kind, id, _)), Some(iid)) = (point_of(cpu, this), Guid::read(cpu, iid)) else {
        return Some(E_INVALIDARG);
    };
    if iid != IID_ICONNECTIONPOINT && iid != com::IID_IUNKNOWN {
        com::write_out(cpu, out, 0);
        return Some(E_NOINTERFACE);
    }
    add_ref(cpu, kind, id);
    com::write_out(cpu, out, this);
    Some(S_OK)
}

pub fn method_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(point_of(cpu, args[0]).map_or(0, |(kind, id, _)| add_ref(cpu, kind, id)))
}

pub fn method_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match point_of(cpu, args[0]) {
        Some((FONT, id, _)) => font::release_ref(cpu, id),
        Some((_, id, _)) => picture::release_ref(cpu, id),
        None => Some(0),
    }
}

pub fn get_connection_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((kind, _, point)) = point_of(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    Some(if sink_iid(kind, point).write(cpu, args[1]) {
        S_OK
    } else {
        E_POINTER
    })
}

pub fn get_connection_point_container(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((kind, id, _)) = point_of(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    Some(match kind {
        FONT => font::query(cpu, id, &IID_ICONNECTIONPOINTCONTAINER, args[1]),
        _ => picture::query(cpu, id, &IID_ICONNECTIONPOINTCONTAINER, args[1]),
    })
}

/// Advise(this, sink, cookie): the sink's interface of the point's IID
/// is asked for and kept, its cookie the place it takes, from 1.
pub fn advise(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, sink, cookie] = args else {
        unreachable!()
    };
    let Some((kind, id, point)) = point_of(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    com::write_out(cpu, cookie, 0);
    let vars = vec![super::ADVISE, kind, id as u32, point, sink, cookie];
    task::start(cpu, task::OLEAUT32_OBJECTS, vars, A_SCRATCH)
}

/// ADVISE's scratch space: the sink interface's IID, and the interface
/// QueryInterface gives.
const A_IID: u32 = 0;
const A_OUT: u32 = 16;
const A_SCRATCH: u32 = 20;

/// ADVISE: variables the object's kind, its id, the point, the sink and
/// where the cookie goes.
pub fn advise_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let s = t.scratch;
    let (kind, id, point, sink, cookie) =
        (t.var(1), t.var(2) as usize, t.var(3), t.var(4), t.var(5));
    match t.step {
        0 => {
            sink_iid(kind, point).write(cpu, s + A_IID);
            let Some(method) = crate::win32::ole32::method(cpu, sink, QUERY_INTERFACE) else {
                return Next::Done(CONNECT_E_CANNOTCONNECT);
            };
            Next::call(method, &[sink, s + A_IID, s + A_OUT], 1)
        }
        _ => {
            let got = cpu.bus.guest_read_32(s + A_OUT);
            if value >> 31 != 0 || got == 0 {
                return Next::Done(CONNECT_E_CANNOTCONNECT);
            }
            let Some(sinks) = sinks(cpu, kind, id, point) else {
                return Next::Done(CONNECT_E_CANNOTCONNECT);
            };
            let i = match sinks.iter().position(|&x| x == 0) {
                Some(i) => {
                    sinks[i] = got;
                    i
                }
                None => {
                    sinks.push(got);
                    sinks.len() - 1
                }
            };
            com::write_out(cpu, cookie, i as u32 + 1);
            Next::Done(S_OK)
        }
    }
}

/// Unadvise(this, cookie): the sink of the cookie is released.
pub fn unadvise(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, cookie] = args else {
        unreachable!()
    };
    let Some((kind, id, point)) = point_of(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let sinks = sinks(cpu, kind, id, point).expect("a point's sinks");
    if cookie == 0 || cookie as usize > sinks.len() {
        return Some(E_INVALIDARG);
    }
    let sink = std::mem::take(&mut sinks[cookie as usize - 1]);
    if sink == 0 {
        return Some(CONNECT_E_NOCONNECTION);
    }
    let calls = release_calls(cpu, [sink]);
    crate::win32::ole32::call_all(cpu, S_OK, &calls)
}

/// EnumConnections(this, out): OLE_E_NOCONNECTION with no sinks; an
/// enumerator of them isn't written yet, and E_NOTIMPL says so.
pub fn enum_connections(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, out] = args else { unreachable!() };
    let Some((kind, id, point)) = point_of(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    com::write_out(cpu, out, 0);
    let any = sinks(cpu, kind, id, point).is_some_and(|s| s.iter().any(|&x| x != 0));
    Some(if any { E_NOTIMPL } else { OLE_E_NOCONNECTION })
}

/// A property `dispid` of an object changed: its IPropertyNotifySinks
/// hear of it through OnChanged, then its IFontEventsDisps through
/// FontChanged with the property's name `name`. S_OK when they have.
pub fn notify(
    cpu: &mut Cpu,
    dispid: u32,
    notify: &[u32],
    events: &[u32],
    name: &str,
) -> Option<u32> {
    let notify: Vec<u32> = notify.iter().copied().filter(|&s| s != 0).collect();
    let events: Vec<u32> = events.iter().copied().filter(|&s| s != 0).collect();
    if notify.is_empty() && events.is_empty() {
        return Some(S_OK);
    }
    let name = if events.is_empty() {
        0
    } else {
        super::new_bstr(cpu, name)
    };
    let mut vars = vec![super::NOTIFY, dispid, name, notify.len() as u32];
    vars.extend(notify);
    vars.extend(events);
    task::start(cpu, task::OLEAUT32_OBJECTS, vars, N_SCRATCH)
}

/// NOTIFY's scratch space: IID_NULL, the FontChanged argument's VARIANT
/// and its DISPPARAMS.
const N_IID: u32 = 0;
const N_ARG: u32 = 16;
const N_PARAMS: u32 = 32;
const N_SCRATCH: u32 = 48;

/// NOTIFY: variables the DISPID, the name's BSTR (0 without
/// IFontEventsDisps), the count of IPropertyNotifySinks, and the sinks,
/// those first.
pub fn notify_step(cpu: &mut Cpu, t: &mut Task) -> Next {
    const FIRST: usize = 4;
    let i = FIRST + t.step as usize;
    let (dispid, name, notify) = (t.var(1), t.var(2), t.var(3) as usize);
    if i >= t.vars.len() {
        if name != 0 {
            super::free_bstr(cpu, name);
        }
        return Next::Done(S_OK);
    }
    let sink = t.vars[i];
    if i < FIRST + notify {
        let method = crate::win32::ole32::method(cpu, sink, ON_CHANGED).unwrap_or(0);
        return Next::call(method, &[sink, dispid], t.step + 1);
    }
    let s = t.scratch;
    cpu.bus.guest_fill(s, N_SCRATCH as usize, 0);
    cpu.bus.guest_write_16(s + N_ARG, VT_BSTR);
    cpu.bus.guest_write_32(s + N_ARG + 8, name);
    cpu.bus.guest_write_32(s + N_PARAMS, s + N_ARG);
    cpu.bus.guest_write_32(s + N_PARAMS + 8, 1);
    let method = crate::win32::ole32::method(cpu, sink, INVOKE).unwrap_or(0);
    Next::call(
        method,
        &[
            sink,
            DISPID_FONT_CHANGED,
            s + N_IID,
            0,
            DISPATCH_METHOD,
            s + N_PARAMS,
            0,
            0,
            0,
        ],
        t.step + 1,
    )
}
