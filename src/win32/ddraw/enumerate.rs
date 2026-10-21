//! The functions that call the program back for each thing they list:
//! DirectDrawEnumerate(Ex)A for the display devices (the display, and
//! the 3dfx device of an add-on card, `voodoo.rs`), EnumDisplayModes,
//! EnumSurfaces and EnumAttachedSurfaces. Each is a task (task.rs) that
//! calls the callback for an item after another, until the list ends or
//! the callback says to stop.
//!
//! DirectDrawEnumerate guards its callback as Windows does: a callback
//! that faults makes the function return DDERR_INVALIDPARAMS. The task
//! puts an exception handler frame of its own in its scratch space,
//! whose handler (a stub of the DLL's) goes on at the function's return
//! with that error.

use super::surface::{self, Desc};
use super::{DD_OK, DDERR_INVALIDPARAMS, DDERR_UNSUPPORTED, display, state, voodoo};
use crate::cpu::Cpu;
use crate::win32::task::{self, Next, Task};

/// The kinds of lists (the task's variable 0).
const DEVICES: u32 = 0;
const DEVICES_EX: u32 = 1;
const MODES: u32 = 2;
const SURFACES: u32 = 3;

/// The task's variables: the kind, the callback, its context, the size
/// of the descriptions it gets, where the list is at, and the items.
const KIND: usize = 0;
const CALLBACK: usize = 1;
const CONTEXT: usize = 2;
const DESC_SIZE: usize = 3;
const NEXT: usize = 4;
const ITEMS: usize = 5;

/// The scratch space: a description, the device's strings, the 3dfx
/// device's GUID and strings, and the exception handler frame.
const SCRATCH: u32 = 256;
const STRINGS: u32 = 128;
const CARD_GUID: u32 = 0;
const CARD_STRINGS: u32 = 16;
const FRAME: u32 = 224;

const DESCRIPTION: &[u8] = b"Primary Display Driver\0";
const NAME: &[u8] = b"display\0";

/// CONTEXT's fields the handler sets.
const CONTEXT_EAX: u32 = 0xB0;
const CONTEXT_EIP: u32 = 0xB8;
const CONTEXT_ESP: u32 = 0xC4;
const EXCEPTION_UNWINDING: u32 = 0x6;
const EXCEPTION_CONTINUE_EXECUTION: u32 = 0;
const EXCEPTION_CONTINUE_SEARCH: u32 = 1;

/// The place of the handler's stub in `APIS`.
fn handler_address() -> u32 {
    let dll = super::dll_index();
    let index = super::DLL
        .apis
        .iter()
        .position(|a| a.name == "EnumerateHandler")
        .expect("the enumeration's handler");
    crate::win32::stubs::stub_address(dll, index)
}

/// Whether `callback` is code the program may call: a pointer that
/// isn't makes the function fail, where others fault.
fn callable(cpu: &mut Cpu, callback: u32) -> bool {
    callback != 0 && crate::win32::kernel32::virtual_memory::allows(cpu, callback, 1, false)
}

/// DirectDrawEnumerateW and ExW: Windows 95 has no Unicode ones.
pub fn unsupported(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DDERR_UNSUPPORTED)
}

/// DirectDrawEnumerateA(callback, context).
pub fn direct_draw_enumerate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !callable(cpu, args[0]) {
        return Some(DDERR_INVALIDPARAMS);
    }
    task::start(
        cpu,
        task::DDRAW_ENUM,
        vec![DEVICES, args[0], args[1]],
        SCRATCH,
    )
}

/// DirectDrawEnumerateExA(callback, context, flags).
pub fn direct_draw_enumerate_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !callable(cpu, args[0]) || args[2] & !7 != 0 {
        return Some(DDERR_INVALIDPARAMS);
    }
    task::start(
        cpu,
        task::DDRAW_ENUM,
        vec![DEVICES_EX, args[0], args[1]],
        SCRATCH,
    )
}

/// EnumDisplayModes(this, flags, desc, context, callback).
pub fn enum_display_modes(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, desc_at, context, callback] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(super::directdraw::ddraw_of(cpu, this));
    if !callable(cpu, callback) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let size = if super::version(vtable) >= 4 {
        124
    } else {
        108
    };
    let filter = if desc_at != 0 {
        hr!(surface::desc_size(cpu, desc_at, super::version(vtable)));
        Some(Desc::read(cpu, desc_at, size))
    } else {
        None
    };
    let mut vars = vec![MODES, callback, context, size, 0];
    let modes = if super::directdraw::is_card(cpu, id) {
        voodoo::modes(cpu)
    } else {
        display::modes(cpu)
    };
    for (w, h, bpp) in modes {
        if let Some(f) = &filter {
            let unwanted = f.flags & surface::DDSD_WIDTH != 0 && f.width != w
                || f.flags & surface::DDSD_HEIGHT != 0 && f.height != h
                || f.flags & surface::DDSD_PIXELFORMAT != 0 && f.format.bits != bpp;
            if unwanted {
                continue;
            }
        }
        vars.push(w | h << 16);
        vars.push(bpp);
    }
    task::start(cpu, task::DDRAW_ENUM, vars, SCRATCH)
}

const DDENUMSURFACES_ALL: u32 = 0x1;
const DDENUMSURFACES_MATCH: u32 = 0x2;
const DDENUMSURFACES_NOMATCH: u32 = 0x4;
const DDENUMSURFACES_CANBECREATED: u32 = 0x8;
const DDENUMSURFACES_DOESEXIST: u32 = 0x10;

/// EnumSurfaces(this, flags, desc, context, callback): the surfaces
/// that exist. Those that could be made aren't listed.
pub fn enum_surfaces(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, flags, desc_at, context, callback] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, dd) = hr!(super::directdraw::ddraw_of(cpu, this));
    if !callable(cpu, callback) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let which = flags & (DDENUMSURFACES_ALL | DDENUMSURFACES_MATCH | DDENUMSURFACES_NOMATCH);
    let how = flags & (DDENUMSURFACES_CANBECREATED | DDENUMSURFACES_DOESEXIST);
    if which.count_ones() != 1 || how.count_ones() != 1 {
        return Some(DDERR_INVALIDPARAMS);
    }
    if how == DDENUMSURFACES_CANBECREATED {
        return Some(DD_OK);
    }
    let version = super::version(vtable);
    let size = if version >= 4 { 124 } else { 108 };
    let filter = if which != DDENUMSURFACES_ALL {
        hr!(surface::desc_size(cpu, desc_at, version));
        Some(Desc::read(cpu, desc_at, size))
    } else {
        None
    };
    let mut vars = vec![SURFACES, callback, context, size, 0];
    for id in state(cpu).ids(|o| o.owner == dd && o.surface.is_some()) {
        let Some(s) = state(cpu).surface(id).cloned() else {
            continue;
        };
        if let Some(f) = &filter {
            let matches = matches(f, &s);
            if matches != (which == DDENUMSURFACES_MATCH) {
                continue;
            }
        }
        let iface = super::iface(cpu, id, surface::vtable_of(s.version));
        if iface != 0 {
            vars.push(iface);
        }
    }
    task::start(cpu, task::DDRAW_ENUM, vars, SCRATCH)
}

/// Whether the surface `s` is what the description `f` asks for.
fn matches(f: &Desc, s: &surface::Surface) -> bool {
    !(f.flags & surface::DDSD_WIDTH != 0 && f.width != s.width
        || f.flags & surface::DDSD_HEIGHT != 0 && f.height != s.height
        || f.flags & surface::DDSD_CAPS != 0 && s.caps[0] & f.caps[0] != f.caps[0]
        || f.flags & surface::DDSD_PIXELFORMAT != 0 && f.format.bits != s.format.bits)
}

/// EnumAttachedSurfaces(this, context, callback).
pub fn enum_attached_surfaces(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, context, callback] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(surface::surface_of(cpu, this));
    if !callable(cpu, callback) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let size = if super::version(vtable) >= 4 {
        124
    } else {
        108
    };
    let mut vars = vec![SURFACES, callback, context, size, 0];
    let attached = state(cpu)
        .surface(id)
        .map(|s| s.attached.clone())
        .unwrap_or_default();
    for (other, _) in attached {
        let iface = super::iface(cpu, other, vtable);
        if iface != 0 {
            vars.push(iface);
        }
    }
    task::start(cpu, task::DDRAW_ENUM, vars, SCRATCH)
}

/// The task's step.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.var(KIND) {
        DEVICES | DEVICES_EX => devices(cpu, t, value),
        _ => list(cpu, t, value),
    }
}

/// The devices to the callback: the display, whose GUID is NULL, then
/// the 3dfx device if there is one and the callback didn't stop.
fn devices(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let tib = crate::win32::process(cpu).tib;
    let frame = t.scratch + FRAME;
    let ex = t.var(KIND) == DEVICES_EX;
    if t.step == 1
        && value != 0
        && let Some(device) = voodoo::device(cpu)
    {
        let guid = t.scratch + CARD_GUID;
        cpu.bus.guest_write_bytes(guid, &device.guid.0);
        let description = t.scratch + CARD_STRINGS;
        cpu.bus.guest_write_bytes(description, device.description);
        let name = description + device.description.len() as u32;
        cpu.bus.guest_write_bytes(name, device.name);
        let mut args = vec![guid, description, name, t.var(CONTEXT)];
        if ex {
            args.push(0);
        }
        return Next::call(t.var(CALLBACK), &args, 2);
    }
    if t.step == 0 {
        let strings = t.scratch + STRINGS;
        cpu.bus.guest_write_bytes(strings, DESCRIPTION);
        let name = strings + DESCRIPTION.len() as u32;
        cpu.bus.guest_write_bytes(name, NAME);
        // The handler frame, first in the thread's list while the
        // callback runs.
        let previous = cpu.bus.guest_read_32(tib);
        let words = [previous, handler_address(), t.resume.0, t.resume.1];
        for (i, w) in words.iter().enumerate() {
            cpu.bus.guest_write_32(frame + 4 * i as u32, *w);
        }
        cpu.bus.guest_write_32(tib, frame);
        let mut args = vec![0, strings, name, t.var(CONTEXT)];
        if ex {
            args.push(0);
        }
        return Next::call(t.var(CALLBACK), &args, 1);
    }
    let previous = cpu.bus.guest_read_32(frame);
    cpu.bus.guest_write_32(tib, previous);
    Next::Done(DD_OK)
}

/// The next item of a list to the callback.
fn list(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step == 1 && value == 0 {
        // DDENUMRET_CANCEL.
        return Next::Done(DD_OK);
    }
    let next = t.var(NEXT) as usize;
    let size = t.var(DESC_SIZE);
    let desc = t.scratch;
    let args = match t.var(KIND) {
        MODES => {
            let at = ITEMS + 2 * next;
            if at + 1 >= t.vars.len() {
                return Next::Done(DD_OK);
            }
            let (size_word, bpp) = (t.var(at), t.var(at + 1));
            let (w, h) = (size_word & 0xFFFF, size_word >> 16);
            let pitch = (w * bpp).div_ceil(8).next_multiple_of(4);
            let d = Desc {
                flags: surface::DDSD_WIDTH
                    | surface::DDSD_HEIGHT
                    | surface::DDSD_PITCH
                    | surface::DDSD_PIXELFORMAT
                    | surface::DDSD_REFRESHRATE,
                width: w,
                height: h,
                pitch,
                format: surface::rgb_format(bpp),
                ..Desc::default()
            };
            d.write(cpu, desc, size);
            vec![desc, t.var(CONTEXT)]
        }
        _ => {
            let at = ITEMS + next;
            if at >= t.vars.len() {
                return Next::Done(DD_OK);
            }
            let iface = t.var(at);
            let Some((_, id)) = super::this(cpu, iface) else {
                t.set(NEXT, next as u32 + 1);
                return Next::call(0, &[], 2);
            };
            let Some(s) = state(cpu).surface(id).cloned() else {
                t.set(NEXT, next as u32 + 1);
                return Next::call(0, &[], 2);
            };
            s.desc(size).write(cpu, desc, size);
            super::add_ref(cpu, iface);
            vec![iface, desc, t.var(CONTEXT)]
        }
    };
    t.set(NEXT, next as u32 + 1);
    Next::call(t.var(CALLBACK), &args, 1)
}

/// The exception handler of DirectDrawEnumerate's callback:
/// (record, frame, context, dispatcher). It takes its frame off the
/// thread's list and goes on at the function's return, which returns
/// DDERR_INVALIDPARAMS.
pub fn handler(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [record, frame, context, _] = args[..] else {
        return Some(EXCEPTION_CONTINUE_SEARCH);
    };
    if !cpu.bus.guest_probe(record, 8, false) || !cpu.bus.guest_probe(frame, 16, false) {
        return Some(EXCEPTION_CONTINUE_SEARCH);
    }
    if cpu.bus.guest_read_32(record + 4) & EXCEPTION_UNWINDING != 0 {
        return Some(EXCEPTION_CONTINUE_SEARCH);
    }
    if !cpu.bus.guest_probe(context, 0xCC, true) {
        return Some(EXCEPTION_CONTINUE_SEARCH);
    }
    let previous = cpu.bus.guest_read_32(frame);
    let (eip, esp) = (
        cpu.bus.guest_read_32(frame + 8),
        cpu.bus.guest_read_32(frame + 12),
    );
    let tib = crate::win32::process(cpu).tib;
    cpu.bus.guest_write_32(tib, previous);
    cpu.bus.guest_write_32(context + CONTEXT_EIP, eip);
    cpu.bus.guest_write_32(context + CONTEXT_ESP, esp);
    cpu.bus
        .guest_write_32(context + CONTEXT_EAX, DDERR_INVALIDPARAMS);
    Some(EXCEPTION_CONTINUE_EXECUTION)
}
