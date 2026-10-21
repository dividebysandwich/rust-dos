//! The installable drivers' functions and the small ones of MMSYSTEM:
//! the built-in Windows has no drivers to open, so OpenDriver finds
//! none, and DefDriverProc answers the messages a driver passes on to it
//! as Windows does. DriverCallback makes the callbacks a driver would.

use super::callback::{self, Callback};
use super::*;
use crate::cpu::Cpu;

/// The driver messages DefDriverProc answers.
const DRV_LOAD: u32 = 1;
const DRV_ENABLE: u32 = 2;
const DRV_DISABLE: u32 = 5;
const DRV_FREE: u32 = 6;
const DRV_INSTALL: u32 = 9;
const DRV_REMOVE: u32 = 10;

/// DriverCallback's flags: the callback's kind, as CALLBACK_ flags
/// without their shift.
const DCB_TYPEMASK: u32 = 0x0007;
const DCB_NOSWITCH: u32 = 0x0008;

/// mmsystemGetVersion: 3.10, Windows 95's.
pub fn mmsystem_get_version(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0x030A)
}

/// DriverCallback(callback, flags, device, message, user, p1, p2): the
/// callback of `flags` gets the message. TRUE when there was one.
pub fn driver_callback(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[cb, flags, device, msg, user, p1, p2] = args else {
        unreachable!()
    };
    let kind = flags & DCB_TYPEMASK;
    if kind == 0 || (kind != 5 && cb == 0) {
        return Some(FALSE);
    }
    let _ = DCB_NOSWITCH;
    let Ok(callback) = Callback::from_open(cpu, cb, kind << 16) else {
        return Some(FALSE);
    };
    callback::notify(cpu, &callback, device, msg, user, p1, p2);
    callback.release(cpu);
    Some(TRUE)
}

/// OpenDriver(name, section, param): no driver of that name.
pub fn open_driver(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// CloseDriver(driver, p1, p2).
pub fn close_driver(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// SendDriverMessage(driver, message, p1, p2).
pub fn send_driver_message(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// DefDriverProc(id, driver, message, p1, p2): the messages a driver
/// leaves to Windows succeed, the others return 0.
pub fn def_driver_proc(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(match args[2] {
        DRV_LOAD | DRV_FREE | DRV_ENABLE | DRV_DISABLE | DRV_INSTALL | DRV_REMOVE => 1,
        _ => 0,
    })
}

/// GetDriverModuleHandle(driver): no driver, no module.
pub fn get_driver_module_handle(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// mmTaskCreate(proc, handle, data): Windows 95 makes no such task for
/// a 32-bit program.
pub fn mm_task_create(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(TASKERR_NOTASKSUPPORT)
}

const TASKERR_NOTASKSUPPORT: u32 = 1;
