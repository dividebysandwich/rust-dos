//! The installable drivers' functions and the small ones of MMSYSTEM.
//! OpenDriver loads a driver's DLL as LoadLibrary does and talks to it
//! through its DriverProc, as Wine's winmm does: the ACM codecs of the
//! `win32_dlls` folder, which Wine's MSACM32.DLL opens, are such drivers.
//! DefDriverProc answers the messages a driver passes on to it as
//! Windows does. DriverCallback makes the callbacks a driver would.

use super::callback::{self, Callback};
use super::*;
use crate::cpu::Cpu;
use crate::win32::call::{self, Call};
use crate::win32::loader::{self, Wanted};

/// The driver messages: those OpenDriver and CloseDriver send, which
/// DefDriverProc answers.
const DRV_LOAD: u32 = 1;
const DRV_ENABLE: u32 = 2;
const DRV_OPEN: u32 = 3;
const DRV_CLOSE: u32 = 4;
const DRV_DISABLE: u32 = 5;
const DRV_FREE: u32 = 6;
const DRV_INSTALL: u32 = 9;
const DRV_REMOVE: u32 = 10;

/// What DriverProc returns to DRV_LOAD when the driver can run.
const DRV_SUCCESS: u32 = 1;

/// An installable driver a process opened (OpenDriver): its handle, its
/// DLL and the DLL's DriverProc, the id its DRV_OPEN returned and the
/// argument it got. `opening` while its DLL loads, for the thread `tid`
/// whose OpenDriver loads it.
#[derive(Clone, Debug, Default)]
pub struct Driver {
    pub pid: u32,
    pub tid: u32,
    pub handle: u32,
    pub module: u32,
    pub proc: u32,
    pub id: u32,
    pub param: u32,
    pub opening: bool,
}

crate::state_fields!(Driver {
    pid,
    tid,
    handle,
    module,
    proc,
    id,
    param,
    opening
});

/// A process ended: its drivers went with its memory.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    cpu.bus.win32.winmm.drivers.retain(|d| d.pid != pid);
}

/// The UTF-16 string at `at`, up to 260 characters; None if a page isn't
/// there.
fn wide_string(cpu: &mut Cpu, at: u32) -> Option<String> {
    let mut units = Vec::new();
    for i in 0..260 {
        let unit = cpu.bus.guest_read_16(at.wrapping_add(2 * i));
        if cpu.bus.guest_faulted() {
            return None;
        }
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    Some(String::from_utf16_lossy(&units))
}

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

/// OpenDriver(name, section, param): the installable driver `name`, an
/// alias in `section` of SYSTEM.INI ([drivers32] without one) for its
/// DLL, or else the DLL's own name, as Wine's winmm has it. The DLL is
/// loaded, its DriverProc learns that it is loaded and enabled (for the
/// first driver of the DLL) and opened, with `param`, and the driver's
/// handle returns; 0 when the DLL isn't there or the driver fails.
pub fn open_driver(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, section, param] = args else {
        unreachable!()
    };
    let Some(name) = wide_string(cpu, name) else {
        return Some(0);
    };
    let section = match section {
        0 => "drivers32".to_string(),
        at => match wide_string(cpu, at) {
            Some(section) => section,
            None => return Some(0),
        },
    };
    let alias = crate::win32::kernel32::profile_value(
        cpu,
        b"SYSTEM.INI",
        section.as_bytes(),
        name.as_bytes(),
    );
    let file = alias.map_or(name.clone(), |value| crate::dosstr::from_bytes(&value));
    // What follows the file's name is the driver's, not the DLL's.
    let file = file.split(' ').next().unwrap_or_default().to_string();
    if file.is_empty() {
        return Some(0);
    }
    let pid = crate::win32::user32::current_pid(cpu);
    let tid = crate::win32::user32::current_tid(cpu);
    let handle = new_handle(cpu);
    let drivers = &mut cpu.bus.win32.winmm.drivers;
    // One of this thread's whose DLL's entry point failed the load. Other
    // threads' may still be loading.
    drivers.retain(|d| !(d.opening && d.pid == pid && d.tid == tid));
    drivers.push(Driver {
        pid,
        tid,
        handle,
        param,
        opening: true,
        ..Driver::default()
    });
    cpu.bus
        .log_string(&format!("[W32] OpenDriver({}): {}", name, file));
    crate::win32::kernel32::module::load_driver(cpu, &file);
    None
}

/// The DLL of the driver OpenDriver opens is loaded at `base` (0: it
/// wasn't): its DriverProc gets the messages that open it, or OpenDriver
/// returns 0 to `resume`. The driver is the one this thread's
/// OpenDriver is opening: the DLL's entry points ran on this thread.
pub(in crate::win32) fn loaded(cpu: &mut Cpu, resume: (u32, u32), base: u32) {
    let pid = crate::win32::user32::current_pid(cpu);
    let tid = crate::win32::user32::current_tid(cpu);
    let drivers = &mut cpu.bus.win32.winmm.drivers;
    let Some(i) = drivers
        .iter()
        .rposition(|d| d.opening && d.pid == pid && d.tid == tid)
    else {
        call::finish(cpu, resume, 0);
        return;
    };
    if base == 0 {
        drivers.remove(i);
        call::finish(cpu, resume, 0);
        return;
    }
    let wanted = Wanted::Name("DriverProc".to_string());
    let proc = crate::win32::with_process(cpu, |p, bus| loader::find_export(bus, p, base, &wanted));
    let drivers = &mut cpu.bus.win32.winmm.drivers;
    let Some(proc) = proc else {
        drivers.remove(i);
        crate::win32::kernel32::module::free_then(cpu, base, resume, 0);
        return;
    };
    let driver = &mut drivers[i];
    driver.module = base;
    driver.proc = proc;
    driver.opening = false;
    let (pid, handle, param) = (driver.pid, driver.handle, driver.param);
    let first = !drivers
        .iter()
        .any(|d| d.handle != handle && d.pid == pid && d.module == base);
    if first {
        send(cpu, handle, STEP_LOAD, DRV_LOAD, 0, 0, resume);
    } else {
        send(cpu, handle, STEP_OPEN, DRV_OPEN, 0, param, resume);
    }
}

/// What a driver's DriverProc was called for (`send`), which `returned`
/// goes on from: opening it (DRV_LOAD, DRV_ENABLE, DRV_OPEN),
/// SendDriverMessage, and closing it (DRV_CLOSE, then for the last
/// driver of its DLL DRV_DISABLE and DRV_FREE).
const STEP_LOAD: u32 = 0;
const STEP_ENABLE: u32 = 1;
const STEP_OPEN: u32 = 2;
const STEP_SEND: u32 = 3;
const STEP_CLOSE: u32 = 4;
const STEP_DISABLE: u32 = 5;
const STEP_FREE: u32 = 6;

/// Call the DriverProc of the driver `handle` with `message`, `p1` and
/// `p2`, for `step`; `resume` is where the function of WinMM that called
/// it returns to.
fn send(cpu: &mut Cpu, handle: u32, step: u32, message: u32, p1: u32, p2: u32, resume: (u32, u32)) {
    let Some(driver) = find(cpu, handle) else {
        call::finish(cpu, resume, 0);
        return;
    };
    let (proc, id) = (driver.proc, driver.id);
    let args = [id, handle, message, p1, p2];
    if !call::call(cpu, proc, &args, call::DRIVER, resume, vec![step, handle]) {
        crate::win32::fatal(cpu, "a thread's stack isn't there", None);
    }
}

/// A DriverProc returned `value`: the next step.
pub(in crate::win32) fn returned(cpu: &mut Cpu, value: u32, call: Call) {
    let &[step, handle] = call.data.as_slice() else {
        return;
    };
    let resume = call.resume;
    let Some(driver) = find(cpu, handle) else {
        call::finish(cpu, resume, 0);
        return;
    };
    let (param, module) = (driver.param, driver.module);
    match step {
        STEP_LOAD if value != DRV_SUCCESS => fail(cpu, handle, module, resume),
        STEP_LOAD => send(cpu, handle, STEP_ENABLE, DRV_ENABLE, 0, 0, resume),
        STEP_ENABLE => send(cpu, handle, STEP_OPEN, DRV_OPEN, 0, param, resume),
        STEP_OPEN if value == 0 => fail(cpu, handle, module, resume),
        STEP_OPEN => {
            if let Some(driver) = find(cpu, handle) {
                driver.id = value;
            }
            call::finish(cpu, resume, handle);
        }
        STEP_CLOSE => {
            let pid = crate::win32::user32::current_pid(cpu);
            let last = !cpu
                .bus
                .win32
                .winmm
                .drivers
                .iter()
                .any(|d| d.handle != handle && d.pid == pid && d.module == module);
            if last {
                send(cpu, handle, STEP_DISABLE, DRV_DISABLE, 0, 0, resume);
            } else {
                remove(cpu, handle, module, resume, TRUE);
            }
        }
        STEP_DISABLE => send(cpu, handle, STEP_FREE, DRV_FREE, 0, 0, resume),
        STEP_FREE => remove(cpu, handle, module, resume, TRUE),
        _ => call::finish(cpu, resume, value),
    }
}

/// The driver `handle` couldn't be opened: it goes, and OpenDriver
/// returns 0.
fn fail(cpu: &mut Cpu, handle: u32, module: u32, resume: (u32, u32)) {
    remove(cpu, handle, module, resume, 0);
}

/// The driver `handle` goes, and its DLL loses the reference OpenDriver
/// took: `value` returns to `resume`, once a DLL that goes has learned
/// so.
fn remove(cpu: &mut Cpu, handle: u32, module: u32, resume: (u32, u32), value: u32) {
    cpu.bus.win32.winmm.drivers.retain(|d| d.handle != handle);
    crate::win32::kernel32::module::free_then(cpu, module, resume, value);
}

/// The running process's driver `handle`.
fn find(cpu: &mut Cpu, handle: u32) -> Option<&mut Driver> {
    let pid = crate::win32::user32::current_pid(cpu);
    cpu.bus
        .win32
        .winmm
        .drivers
        .iter_mut()
        .find(|d| d.pid == pid && d.handle == handle && !d.opening)
}

/// CloseDriver(driver, p1, p2): DRV_CLOSE, and for the last driver of its
/// DLL DRV_DISABLE and DRV_FREE; the DLL loses a reference. TRUE, or FALSE
/// for a handle that isn't a driver.
pub fn close_driver(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, p1, p2] = args else {
        unreachable!()
    };
    if find(cpu, handle).is_none() {
        return Some(FALSE);
    }
    send(cpu, handle, STEP_CLOSE, DRV_CLOSE, p1, p2, call::here(cpu));
    None
}

/// SendDriverMessage(driver, message, p1, p2): what the driver's
/// DriverProc returns, or 0 for a handle that isn't a driver.
pub fn send_driver_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, message, p1, p2] = args else {
        unreachable!()
    };
    if find(cpu, handle).is_none() {
        return Some(0);
    }
    send(cpu, handle, STEP_SEND, message, p1, p2, call::here(cpu));
    None
}

/// DefDriverProc(id, driver, message, p1, p2): the messages a driver
/// leaves to Windows succeed, the others return 0.
pub fn def_driver_proc(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(match args[2] {
        DRV_LOAD | DRV_FREE | DRV_ENABLE | DRV_DISABLE | DRV_INSTALL | DRV_REMOVE => 1,
        _ => 0,
    })
}

/// GetDriverModuleHandle(driver): the driver's DLL, or 0.
pub fn get_driver_module_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(find(cpu, args[0]).map_or(0, |d| d.module))
}

/// mmTaskCreate(proc, handle, data): Windows 95 makes no such task for
/// a 32-bit program.
pub fn mm_task_create(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(TASKERR_NOTASKSUPPORT)
}

const TASKERR_NOTASKSUPPORT: u32 = 1;
