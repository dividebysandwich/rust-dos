//! EnumDevices and EnumObjects, which call the program's callback for
//! each device or object: a task (task.rs) whose steps fill a
//! DIDEVICEINSTANCEA or DIDEVICEOBJECTINSTANCEA in its scratch space and
//! call the callback with it, until the callback returns DIENUM_STOP or
//! there are no more.

use super::device::dev;
use super::objects::{self, Obj};
use super::{DI_OK, DIERR_INVALIDPARAM, DIERR_NOTINITIALIZED, initialized, result, state};
use crate::cpu::Cpu;
use crate::win32::task::{self, Next, Task};

/// The task's place in `task::TASKS`.
const TASK: u16 = task::DI_ENUM;

/// What the task enumerates, its first variable.
const K_DEVICES: u32 = 0;
const K_OBJECTS: u32 = 1;

/// The task's variables: the kind, the callback and its reference, the
/// structure's size, the place reached; then the device (EnumObjects)
/// and the flags, or the list of devices (EnumDevices).
const V_CALLBACK: usize = 1;
const V_REF: usize = 2;
const V_SIZE: usize = 3;
const V_AT: usize = 4;
const V_DEVICE: usize = 5;
const V_FLAGS: usize = 6;
const V_LIST: usize = 5;

const DIENUM_STOP: u32 = 0;

/// EnumDevices's flags DirectX 5 to 7 know.
const DIEDFL_ATTACHEDONLY: u32 = 0x1;
const DIEDFL_FORCEFEEDBACK: u32 = 0x100;
const DIEDFL_INCLUDEALIASES: u32 = 0x1_0000;
const DIEDFL_INCLUDEPHANTOMS: u32 = 0x2_0000;

/// IDirectInput::EnumDevices(type, callback, ref, flags): the devices of
/// DIDEVTYPE_ `type`, or all for 0, that are there. None has force
/// feedback.
pub fn enum_devices(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, ty, callback, reference, flags) = (args[0], args[1], args[2], args[3], args[4]);
    let r = (|| {
        let (_, version) = initialized(cpu, this)?;
        let known = DIEDFL_ATTACHEDONLY
            | DIEDFL_FORCEFEEDBACK
            | DIEDFL_INCLUDEALIASES
            | DIEDFL_INCLUDEPHANTOMS;
        if callback == 0 || ty > 4 || flags & !known != 0 {
            return Err(DIERR_INVALIDPARAM);
        }
        let mut vars = vec![K_DEVICES, callback, reference, 0, 0];
        vars[V_SIZE] = if version >= 0x500 {
            objects::INSTANCE_SIZE
        } else {
            objects::INSTANCE_SIZE_DX3
        };
        if flags & DIEDFL_FORCEFEEDBACK == 0 {
            vars.extend(
                objects::present(cpu, version)
                    .into_iter()
                    .filter(|&k| ty == 0 || objects::device_type(k) & 0xFF == ty),
            );
        }
        let size = vars[V_SIZE];
        Ok((vars, size))
    })();
    match r {
        Ok((vars, size)) => task::start(cpu, TASK, vars, size),
        Err(e) => result(Err(e)),
    }
}

/// IDirectInputDevice::EnumObjects(callback, ref, flags): the device's
/// objects of the DIDFT_ types in `flags`, or all.
pub fn enum_objects(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, callback, reference, flags) = (args[0], args[1], args[2], args[3]);
    let r = (|| {
        let id = super::object_of(cpu, this, super::DEVICE).ok_or(crate::win32::com::E_POINTER)?;
        if dev(cpu, id).kind == 0 {
            return Err(DIERR_NOTINITIALIZED);
        }
        if callback == 0 {
            return Err(DIERR_INVALIDPARAM);
        }
        let version = state(cpu).object(id).map_or(0, |o| o.version);
        let size = if version >= 0x500 {
            objects::OBJECT_SIZE
        } else {
            objects::OBJECT_SIZE_DX3
        };
        Ok((
            vec![K_OBJECTS, callback, reference, size, 0, id, flags],
            size,
        ))
    })();
    match r {
        Ok((vars, size)) => task::start(cpu, TASK, vars, size),
        Err(e) => result(Err(e)),
    }
}

/// Whether EnumObjects's `flags` take `obj`: its type among them, and
/// none of the attributes they ask for, which no object here has.
fn wanted(flags: u32, obj: &Obj) -> bool {
    let types = flags & 0xFF;
    if types != 0 && obj.ty & types == 0 {
        return false;
    }
    flags & 0x1F00_0000 == 0
}

/// A step of the task: the callback said whether to go on (`value`).
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step == 1 && value == DIENUM_STOP {
        return Next::Done(DI_OK);
    }
    let (callback, reference, size, at) =
        (t.var(V_CALLBACK), t.var(V_REF), t.var(V_SIZE), t.var(V_AT));
    let bytes = if t.var(0) == K_DEVICES {
        let Some(&kind) = t.vars.get(V_LIST + at as usize) else {
            return Next::Done(DI_OK);
        };
        t.set(V_AT, at + 1);
        objects::instance(kind, objects::shape(cpu, kind), size)
    } else {
        let (id, flags) = (t.var(V_DEVICE), t.var(V_FLAGS));
        let Some(d) = state(cpu).object(id).and_then(|o| o.device.as_ref()) else {
            return Next::Done(DI_OK);
        };
        let objs = d.objects();
        let Some(k) = (at as usize..objs.len()).find(|&k| wanted(flags, &objs[k])) else {
            return Next::Done(DI_OK);
        };
        t.set(V_AT, k as u32 + 1);
        let ofs = d
            .format
            .as_ref()
            .and_then(|f| f.offsets.get(k).copied())
            .filter(|&o| o != objects::NONE)
            .unwrap_or(objs[k].ofs);
        objects::object_instance(d.kind, &objs[k], ofs, size)
    };
    cpu.bus.guest_write_bytes(t.scratch, &bytes);
    Next::call(callback, &[t.scratch, reference], 1)
}
