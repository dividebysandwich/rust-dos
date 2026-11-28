//! MMSYSTEM's installable MCI drivers: the 16-bit DLLs that SYSTEM.INI's
//! [mci] section names for device types the built-in MCI (WINMM's)
//! doesn't have, which MMSYSTEM loads for a Windows 3.1 program as
//! Windows does. Outpost's FLC films play through Autodesk's
//! MCIAAP.DRV, which its setup installs as `[mci] Autodesk=mciaap.drv`
//! with `[mci extensions] flc=Autodesk` in WIN.INI.
//!
//! MCI_OPEN finds the device type as Windows does: the one the program
//! names, or the one WIN.INI's [mci extensions] gives the element's
//! extension. A type of the built-in MCI stays its own; another one that
//! [mci] names a file for opens through that file. MMSYSTEM loads it
//! as LoadLibrary does and opens it as OpenDriver does: its DriverProc
//! gets DRV_LOAD and DRV_ENABLE (for the first device of the module),
//! then DRV_OPEN with the rest of the [mci] line and an
//! MCI_OPEN_DRIVER_PARMS, then MCI_OPEN_DRIVER with the program's flags
//! and parameter block. The device's id is one of WINMM's (`mci::Mci`
//! keeps the device), and the program's later commands for it go to
//! DriverProc with the program's own parameter block, which is the
//! driver's 16-bit layout. MCI_CLOSE sends MCI_CLOSE_DRIVER, then
//! DRV_CLOSE, and for the last device of the module DRV_DISABLE and
//! DRV_FREE, and the module is freed.
//!
//! Each call into the driver is a call of 16-bit code that returns to
//! `then`, which makes the next one: `data` says which step returned.
//! The driver runs on the program's stack, below its call of
//! mciSendCommand, as it does on Windows.

use super::{module, task, thunk};
use crate::cpu::Cpu;
use crate::dpmi::Context;
use crate::win32::winmm::mci::{self as wmci, Driver16};

/// The driver messages.
const DRV_LOAD: u32 = 1;
const DRV_ENABLE: u32 = 2;
const DRV_OPEN: u32 = 3;
const DRV_CLOSE: u32 = 4;
const DRV_DISABLE: u32 = 5;
const DRV_FREE: u32 = 6;

/// The messages that open and close an MCI driver's device.
const MCI_OPEN_DRIVER: u32 = 0x0801;
const MCI_CLOSE_DRIVER: u32 = 0x0802;

/// MCI_OPEN_DRIVER_PARMS's wCustomCommandTable before the driver
/// gives one.
const MCI_NO_COMMAND_TABLE: u16 = 0xFFFF;

/// The flag of a driver's answer to MCI_STATUS or MCI_GETDEVCAPS whose
/// dwReturn's low word is a resource id: MMSYSTEM gives the program that
/// word alone.
const MCI_RESOURCE_RETURNED: u32 = 0x0002_0000;

/// The notification's message.
const MM_MCINOTIFY: u32 = 0x03B9;

/// The steps of `then`: what the driver's code that returned was called
/// for. The first runs the entry points of the libraries the driver's
/// load loaded.
const STEP_INIT: u32 = 0;
const STEP_LOAD: u32 = 1;
const STEP_ENABLE: u32 = 2;
const STEP_OPEN: u32 = 3;
const STEP_OPEN_DRIVER: u32 = 4;
const STEP_COMMAND: u32 = 5;
const STEP_CLOSE_DRIVER: u32 = 6;
const STEP_CLOSE: u32 = 7;
const STEP_DISABLE: u32 = 8;
const STEP_FREE: u32 = 9;

/// The places in `then`'s data: the step, the device's id, the error
/// the command returns once the device is closed, the command's
/// message, flags and parameter block (16:16), whether it closes all
/// the program's devices, and for an open of "type!element" the length
/// of "type!" (`Found::skip`). The libraries whose entry points are
/// still to run follow, for STEP_INIT.
const D_STEP: usize = 0;
const D_ID: usize = 1;
const D_ERROR: usize = 2;
const D_MSG: usize = 3;
const D_FLAGS: usize = 4;
const D_PARMS: usize = 5;
const D_ALL: usize = 6;
const D_SKIP: usize = 7;
const D_LEN: usize = 8;

/// The bytes of the program's stack below its call that DRV_OPEN's
/// MCI_OPEN_DRIVER_PARMS (at 0) and the parameters' string (at
/// `PARAMS_AT`) take.
const SCRATCH: u16 = 0x100;
const PARAMS_AT: u16 = 0x10;

/// The driver an MCI_OPEN opens: its file, the rest of its [mci] line,
/// the name the device gets, and for an element "type!element" the
/// length of "type!", which the driver doesn't see (`move_element`).
struct Found {
    file: String,
    params: String,
    name: String,
    skip: u32,
}

/// mciSendCommand(id, message, flags, parms), when it is for a device of
/// an installable driver: what the function returns (None when it calls
/// the driver first), or None when it isn't, and WINMM does the
/// command.
pub fn send_command(cpu: &mut Cpu, args: &[u32]) -> Option<Option<u32>> {
    let (id, msg, flags, segptr) = (args[0], args[1], args[2], args[3]);
    if msg == wmci::MCI_OPEN {
        let found = found(cpu, flags, segptr)?;
        return Some(open(cpu, flags, segptr, found));
    }
    let pid = crate::win32::user32::current_pid(cpu);
    let drivers = wmci::drivers16(cpu);
    if (id == 0xFFFF || id == wmci::MCI_ALL_DEVICE_ID) && msg == wmci::MCI_CLOSE {
        // The drivers' devices close first, then WINMM's.
        let first = drivers.iter().find(|d| d.pid == pid && d.open)?.id;
        let mut ctx = Context::of(cpu);
        let data = vec![STEP_CLOSE_DRIVER, first, 0, msg, flags, segptr, 1, 0];
        return Some(close_driver(cpu, first, &data, &mut ctx));
    }
    // A 16-bit device id is a word.
    let id = id & 0xFFFF;
    drivers
        .iter()
        .find(|d| d.id == id && d.pid == pid && d.open)?;
    let mut ctx = Context::of(cpu);
    let data = vec![STEP_COMMAND, id, 0, msg, flags, segptr, 0, 0];
    Some(match msg {
        wmci::MCI_CLOSE => close_driver(cpu, id, &data, &mut ctx),
        // MMSYSTEM keeps the break key itself, as WINMM does for its own
        // devices (`mci::device_command`).
        wmci::MCI_BREAK => Some(0),
        // MMSYSTEM answers it from the devices it has, as WINMM does.
        wmci::MCI_SYSINFO => return None,
        _ => call(cpu, id, msg, flags, segptr, &data, &ctx),
    })
}

/// The installable driver the MCI_OPEN with `flags` and the block at
/// `segptr` opens, if it opens one: a device type that the built-in MCI
/// doesn't have and SYSTEM.INI's [mci] names a file for.
fn found(cpu: &mut Cpu, flags: u32, segptr: u32) -> Option<Found> {
    let at = super::linear(cpu, segptr);
    if at == 0 || flags & (wmci::MCI_OPEN_ELEMENT_ID | wmci::MCI_OPEN_TYPE_ID) != 0 {
        return None;
    }
    let string_at = |cpu: &mut Cpu, off: u32| {
        let p = super::read_32(cpu, at + off);
        super::string(cpu, super::linear(cpu, p))
    };
    let mut element = if flags & wmci::MCI_OPEN_ELEMENT != 0 {
        string_at(cpu, 12)
    } else {
        String::new()
    };
    let mut skip = 0;
    let kind = if flags & wmci::MCI_OPEN_TYPE != 0 {
        string_at(cpu, 8)
    } else if let Some((kind, rest)) = element.split_once('!') {
        // "type!element" names both. The device is named by the element
        // alone, as WINMM names its own (`mci::open`), and the driver
        // opens the element alone. A character is a byte of the
        // program's string.
        skip = kind.chars().count() as u32 + 1;
        let kind = kind.to_string();
        element = rest.to_string();
        kind
    } else {
        // The type WIN.INI's [mci extensions] gives the extension.
        let (_, ext) = element.rsplit_once('.')?;
        if ext.contains(['\\', '/']) {
            return None;
        }
        let value = crate::win32::kernel32::profile_value(
            cpu,
            b"WIN.INI",
            b"mci extensions",
            ext.as_bytes(),
        )?;
        crate::dosstr::from_bytes(&value).trim().to_string()
    };
    if kind.is_empty() || wmci::type_of_name(&kind).is_some() {
        return None;
    }
    let line = crate::win32::kernel32::profile_value(cpu, b"SYSTEM.INI", b"mci", kind.as_bytes())?;
    let line = crate::dosstr::from_bytes(&line);
    let line = line.trim();
    let (file, params) = line.split_once([' ', '\t']).unwrap_or((line, ""));
    if file.is_empty() {
        return None;
    }
    let name = if flags & wmci::MCI_OPEN_ALIAS != 0 {
        string_at(cpu, 16)
    } else if !element.is_empty() {
        element
    } else {
        kind
    };
    Some(Found {
        file: file.to_string(),
        params: params.trim().to_string(),
        name,
        skip,
    })
}

/// MCI_OPEN of a device of the driver `found`: its module is loaded, and
/// the entry points of the libraries loaded with it run first.
fn open(cpu: &mut Cpu, flags: u32, segptr: u32, found: Found) -> Option<u32> {
    if wmci::find_name(cpu, &found.name).is_some()
        || wmci::find_driver16_name(cpu, &found.name).is_some()
    {
        return Some(if flags & wmci::MCI_OPEN_ALIAS != 0 {
            wmci::MCIERR_DUPLICATE_ALIAS
        } else {
            wmci::MCIERR_DEVICE_OPEN
        });
    }
    let dir = module::task_dir(cpu);
    let mut inits = Vec::new();
    let index = match module::load(cpu, &found.file, &dir, &mut inits) {
        Ok(index) => index,
        Err(e) => {
            cpu.bus.log_string(&format!(
                "[W16] MCI driver {} for {}: {}",
                found.file, found.name, e.message
            ));
            return Some(wmci::MCIERR_CANNOT_LOAD_DRIVER);
        }
    };
    let module = cpu.bus.win32.win16.modules[index].handle as u32;
    let id = wmci::free_id(cpu);
    cpu.bus.log_string(&format!(
        "[W16] MCI driver {} for {}: device {}",
        found.file, found.name, id
    ));
    let pid = crate::win32::user32::current_pid(cpu);
    wmci::drivers16(cpu).push(Driver16 {
        id,
        pid,
        name: found.name,
        // A handle of its own for each device, as OpenDriver gives.
        handle: 0x4000 | id,
        module,
        params: found.params,
        ..Driver16::default()
    });
    let mut data = vec![
        STEP_INIT,
        id,
        0,
        wmci::MCI_OPEN,
        flags,
        segptr,
        0,
        found.skip,
    ];
    data.extend(
        inits
            .iter()
            .map(|&i| cpu.bus.win32.win16.modules[i].handle as u32),
    );
    let mut ctx = Context::of(cpu);
    then(cpu, 0, &data, &mut ctx)
}

/// The running process's device `id` of a driver.
fn device(cpu: &mut Cpu, id: u32) -> Option<&mut Driver16> {
    let pid = crate::win32::user32::current_pid(cpu);
    wmci::drivers16(cpu)
        .iter_mut()
        .find(|d| d.id == id && d.pid == pid)
}

/// Whether another device of the running process is of the module
/// `module`.
fn module_shared(cpu: &mut Cpu, id: u32, module: u32) -> bool {
    let pid = crate::win32::user32::current_pid(cpu);
    wmci::drivers16(cpu)
        .iter()
        .any(|d| d.id != id && d.pid == pid && d.module == module)
}

/// Call the DriverProc of the device `id` with `message`, `p1` and `p2`,
/// with the driver's id that DRV_OPEN returned (0 before it), on the
/// program's stack below `ctx`'s, or at `stack`. `then` goes on with
/// `data` when it returns.
#[allow(clippy::too_many_arguments)]
fn send(
    cpu: &mut Cpu,
    id: u32,
    message: u32,
    p1: u32,
    p2: u32,
    data: Vec<u32>,
    ctx: &Context,
    stack: Option<(u16, u16)>,
) -> Option<u32> {
    let Some(d) = device(cpu, id) else {
        return finish(wmci::MCIERR_INVALID_DEVICE_ID);
    };
    let (proc, ds, driver_id, handle) = (d.proc, d.ds, d.driver_id, d.handle);
    // DriverProc(dwDriverID, hDriver, wMessage, lParam1, lParam2), Pascal's
    // order: a long's high word first.
    let args = [
        (driver_id >> 16) as u16,
        driver_id as u16,
        handle as u16,
        message as u16,
        (p1 >> 16) as u16,
        p1 as u16,
        (p2 >> 16) as u16,
        p2 as u16,
    ];
    let stack = stack.unwrap_or_else(|| thunk::stack16_for(cpu, ctx));
    thunk::call16_at(
        cpu,
        proc,
        &args,
        ds as u16,
        *ctx,
        thunk::THEN_MCI_DRIVER,
        data,
        stack,
    );
    None
}

/// A command of the program's for the device `id`: its message, flags
/// and parameter block go to the driver as they are.
fn call(
    cpu: &mut Cpu,
    id: u32,
    msg: u32,
    flags: u32,
    segptr: u32,
    data: &[u32],
    ctx: &Context,
) -> Option<u32> {
    let mut data = data.to_vec();
    data[D_STEP] = STEP_COMMAND;
    send(cpu, id, msg, flags, segptr, data, ctx, None)
}

/// MCI_CLOSE of the device `id`: the driver gets MCI_CLOSE_DRIVER.
fn close_driver(cpu: &mut Cpu, id: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    let mut data = data.to_vec();
    data[D_STEP] = STEP_CLOSE_DRIVER;
    data[D_ID] = id;
    let (flags, segptr) = (data[D_FLAGS], data[D_PARMS]);
    if let Some(d) = device(cpu, id) {
        // No command reaches it while it closes.
        d.open = false;
    }
    send(cpu, id, MCI_CLOSE_DRIVER, flags, segptr, data, ctx, None)
}

/// The function returns `value`, a word, to the program in DX:AX.
fn finish(value: u32) -> Option<u32> {
    Some(value & 0xFFFF)
}

/// `data` for the step `step`.
fn at_step(data: &[u32], step: u32) -> Vec<u32> {
    let mut data = data.to_vec();
    data[D_STEP] = step;
    data
}

/// The program's stack below its call: where DRV_OPEN's structures are
/// (SS, its first byte's offset) and its address.
fn scratch(cpu: &mut Cpu, ctx: &Context) -> (u16, u16, u32) {
    let (ss, sp) = thunk::stack16_for(cpu, ctx);
    let at = sp.wrapping_sub(SCRATCH) & !1;
    (ss, at, super::lin(cpu, ss, at))
}

/// DRV_OPEN, with the rest of the [mci] line and an
/// MCI_OPEN_DRIVER_PARMS for the device on the program's stack.
fn send_open(cpu: &mut Cpu, id: u32, data: &[u32], ctx: &Context) -> Option<u32> {
    let params = device(cpu, id).map_or(String::new(), |d| std::mem::take(&mut d.params));
    let (ss, at, linear) = scratch(cpu, ctx);
    let mut bytes = crate::dosstr::to_bytes(&params);
    bytes.truncate((SCRATCH - PARAMS_AT - 1) as usize);
    bytes.push(0);
    super::poke(cpu, linear + PARAMS_AT as u32, &bytes);
    let params16 = (ss as u32) << 16 | at.wrapping_add(PARAMS_AT) as u32;
    // MCI_OPEN_DRIVER_PARMS: wDeviceID, lpstrParams, wCustomCommandTable
    // and wType, which the driver sets.
    super::write_16(cpu, linear, id as u16);
    super::write_32(cpu, linear + 2, params16);
    super::write_16(cpu, linear + 6, MCI_NO_COMMAND_TABLE);
    super::write_16(cpu, linear + 8, 0);
    let parms16 = (ss as u32) << 16 | at as u32;
    let mut data = data.to_vec();
    data[D_STEP] = STEP_OPEN;
    send(
        cpu,
        id,
        DRV_OPEN,
        params16,
        parms16,
        data,
        ctx,
        Some((ss, at)),
    )
}

/// A step's call of the driver's code returned `value` (DX:AX), or the
/// libraries' entry points ran (STEP_INIT): the next call, or what the
/// MMSYSTEM function returns, a word: the context going on gets it in
/// AX, and DX is cleared, as the function returns a DWORD.
pub(super) fn then(cpu: &mut Cpu, value: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    let result = step(cpu, value, data, ctx);
    if result.is_some() {
        ctx.gpr[thunk::EDX] &= 0xFFFF_0000;
    }
    result
}

/// `then`'s steps.
fn step(cpu: &mut Cpu, value: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    if data.len() < D_LEN {
        return finish(wmci::MCIERR_INTERNAL);
    }
    let id = data[D_ID];
    let mut next = data[..D_LEN].to_vec();
    match data[D_STEP] {
        STEP_INIT => {
            if let Some(&library) = data.get(D_LEN) {
                next.extend_from_slice(&data[D_LEN + 1..]);
                task::call_lib_init(cpu, library as u16, *ctx, thunk::THEN_MCI_DRIVER, next);
                return None;
            }
            loaded(cpu, id, &next, ctx)
        }
        STEP_LOAD if value == 0 => {
            // DRV_LOAD failed: the module goes, told nothing more.
            next[D_ERROR] = wmci::MCIERR_CANNOT_LOAD_DRIVER;
            removed(cpu, id, &next, ctx)
        }
        STEP_LOAD => send(
            cpu,
            id,
            DRV_ENABLE,
            0,
            0,
            at_step(&next, STEP_ENABLE),
            ctx,
            None,
        ),
        STEP_ENABLE => send_open(cpu, id, &next, ctx),
        STEP_OPEN if value == 0 => {
            next[D_ERROR] = wmci::MCIERR_CANNOT_LOAD_DRIVER;
            unload(cpu, id, &next, ctx)
        }
        STEP_OPEN => {
            let (_, _, linear) = scratch(cpu, ctx);
            let kind = super::read_16(cpu, linear + 8) as u32;
            if let Some(d) = device(cpu, id) {
                d.driver_id = value;
                d.kind = kind;
            }
            let (flags, segptr) = (data[D_FLAGS], data[D_PARMS]);
            write_id(cpu, id, segptr);
            move_element(cpu, segptr, data[D_SKIP], true);
            send(
                cpu,
                id,
                MCI_OPEN_DRIVER,
                flags,
                segptr,
                at_step(&next, STEP_OPEN_DRIVER),
                ctx,
                None,
            )
        }
        STEP_OPEN_DRIVER => {
            move_element(cpu, data[D_PARMS], data[D_SKIP], false);
            if value & 0xFFFF != 0 {
                // The driver refused the element: the device closes, and
                // the open returns the driver's error.
                next[D_ERROR] = value & 0xFFFF;
                return send(
                    cpu,
                    id,
                    DRV_CLOSE,
                    0,
                    0,
                    at_step(&next, STEP_CLOSE),
                    ctx,
                    None,
                );
            }
            if let Some(d) = device(cpu, id) {
                d.open = true;
            }
            write_id(cpu, id, data[D_PARMS]);
            finish(0)
        }
        STEP_COMMAND => {
            let msg = data[D_MSG];
            let at = super::linear(cpu, data[D_PARMS]);
            if value & 0xFFFF == 0
                && value & MCI_RESOURCE_RETURNED != 0
                && matches!(msg, wmci::MCI_STATUS | wmci::MCI_GETDEVCAPS)
                && at != 0
            {
                let ret = super::read_32(cpu, at + 4);
                super::write_32(cpu, at + 4, ret & 0xFFFF);
            }
            finish(value)
        }
        STEP_CLOSE_DRIVER => {
            next[D_ERROR] = value & 0xFFFF;
            send(
                cpu,
                id,
                DRV_CLOSE,
                0,
                0,
                at_step(&next, STEP_CLOSE),
                ctx,
                None,
            )
        }
        STEP_CLOSE => unload(cpu, id, &next, ctx),
        STEP_DISABLE => send(
            cpu,
            id,
            DRV_FREE,
            0,
            0,
            at_step(&next, STEP_FREE),
            ctx,
            None,
        ),
        STEP_FREE => removed(cpu, id, &next, ctx),
        _ => finish(wmci::MCIERR_INTERNAL),
    }
}

/// The libraries the driver's module needs are ready: its DriverProc
/// gets DRV_LOAD, or for a module another device has open DRV_OPEN.
fn loaded(cpu: &mut Cpu, id: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    let Some(module) = device(cpu, id).map(|d| d.module) else {
        return finish(wmci::MCIERR_CANNOT_LOAD_DRIVER);
    };
    let handle = module as u16;
    let proc = module::ordinal_of(cpu, handle, "DriverProc")
        .map_or(0, |o| module::entry_point(cpu, handle, o));
    let Some(index) = super::module_index(cpu, handle).filter(|_| proc != 0) else {
        let mut data = data.to_vec();
        data[D_ERROR] = wmci::MCIERR_CANNOT_LOAD_DRIVER;
        return removed(cpu, id, &data, ctx);
    };
    let ds = module::instance_of(cpu, index) as u32;
    if let Some(d) = device(cpu, id) {
        d.proc = proc;
        d.ds = ds;
    }
    if module_shared(cpu, id, module) {
        return send_open(cpu, id, data, ctx);
    }
    let mut data = data.to_vec();
    data[D_STEP] = STEP_LOAD;
    send(cpu, id, DRV_LOAD, 0, 0, data, ctx, None)
}

/// The device `id` closed (DRV_CLOSE, or an open that failed after
/// DRV_ENABLE): for the last device of its module, DRV_DISABLE and
/// DRV_FREE first.
fn unload(cpu: &mut Cpu, id: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    let module = device(cpu, id).map_or(0, |d| d.module);
    if module_shared(cpu, id, module) {
        return removed(cpu, id, data, ctx);
    }
    let mut data = data.to_vec();
    data[D_STEP] = STEP_DISABLE;
    send(cpu, id, DRV_DISABLE, 0, 0, data, ctx, None)
}

/// The device `id` goes, and its module loses the use its open took.
/// Then the next of the program's devices closes, for a close of all
/// of them, or the function returns the error the data has.
fn removed(cpu: &mut Cpu, id: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    let pid = crate::win32::user32::current_pid(cpu);
    let drivers = wmci::drivers16(cpu);
    let module = drivers
        .iter()
        .find(|d| d.id == id && d.pid == pid)
        .map_or(0, |d| d.module);
    drivers.retain(|d| !(d.id == id && d.pid == pid));
    if module != 0 {
        module::release(cpu, module as u16);
    }
    if data[D_ALL] == 0 {
        return finish(data[D_ERROR]);
    }
    let drivers = wmci::drivers16(cpu);
    if let Some(next) = drivers
        .iter()
        .find(|d| d.pid == pid && d.open)
        .map(|d| d.id)
    {
        return close_driver(cpu, next, data, ctx);
    }
    // WINMM closes its own devices.
    let (flags, segptr) = (data[D_FLAGS], data[D_PARMS]);
    let at = super::linear(cpu, segptr);
    let block = thunk::scratch32(cpu, 16);
    let parms = if at == 0 {
        0
    } else {
        // MCI_GENERIC_PARMS: dwCallback's window is its low word.
        let window = super::read_16(cpu, at) as u32;
        cpu.bus.guest_write_32(block, window);
        block
    };
    let target = super::builtin::win32_address(cpu, "WINMM.DLL", "mciSendCommandA")
        .expect("WINMM has mciSendCommandA");
    let args = [wmci::MCI_ALL_DEVICE_ID, wmci::MCI_CLOSE, flags, parms];
    thunk::enter32_at(
        cpu,
        *ctx,
        block,
        target,
        &args,
        thunk::POST_PLAIN,
        Vec::new(),
    );
    None
}

/// MCI_OPEN gives the program the device's id in its block's wDeviceID.
fn write_id(cpu: &mut Cpu, id: u32, segptr: u32) {
    let at = super::linear(cpu, segptr);
    if at != 0 {
        super::write_16(cpu, at + 4, id as u16);
    }
}

/// For an open of "type!element", the program's lpstrElementName at
/// `segptr` moved `by` characters on (`forward`) past "type!" while the
/// driver's MCI_OPEN_DRIVER runs, and back afterwards: Windows' MMSYSTEM
/// gives the driver the element alone, and the program its own block
/// back as it was.
fn move_element(cpu: &mut Cpu, segptr: u32, by: u32, forward: bool) {
    let at = super::linear(cpu, segptr);
    if by == 0 || at == 0 {
        return;
    }
    let p = super::read_32(cpu, at + 12);
    let offset = if forward {
        (p as u16).wrapping_add(by as u16)
    } else {
        (p as u16).wrapping_sub(by as u16)
    };
    super::write_32(cpu, at + 12, p & 0xFFFF_0000 | offset as u32);
}

/// GetDriverModuleHandle(driver), USER's: for the handle a device's
/// DriverProc is given, its driver's instance, which LoadLibrary
/// returned for it, as OpenDriver's handles give theirs; 0 for another
/// handle.
pub fn get_driver_module_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let pid = crate::win32::user32::current_pid(cpu);
    let handle = args[0] & 0xFFFF;
    let module = wmci::drivers16(cpu)
        .iter()
        .find(|d| d.handle == handle && d.pid == pid)
        .map_or(0, |d| d.module as u16);
    Some(super::module_index(cpu, module).map_or(0, |i| module::instance_of(cpu, i) as u32))
}

/// mciSetDriverData(id, data): the driver's data for its device. FALSE
/// for an id that isn't a driver's device.
pub fn mci_set_driver_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(device(cpu, args[0]).map_or(0, |d| {
        d.data = args[1];
        1
    }))
}

/// mciGetDriverData(id): what mciSetDriverData gave, or 0.
pub fn mci_get_driver_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(device(cpu, args[0]).map_or(0, |d| d.data))
}

/// mciDriverNotify(window, id, status): MM_MCINOTIFY posted to the
/// window, TRUE when there is one.
pub fn mci_driver_notify(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, id, status) = (args[0], args[1], args[2]);
    if hwnd == 0 {
        return Some(0);
    }
    crate::win32::user32::message::post_message(cpu, &[hwnd, MM_MCINOTIFY, status, id]);
    Some(1)
}

/// mciDriverYield(id): the device's command goes on; no break key was
/// pressed.
pub fn mci_driver_yield(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}
