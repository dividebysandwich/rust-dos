//! The installable ACM drivers: the codecs SYSTEM.INI's `[drivers32]`
//! names as `msacm.<name>=<file>`, and those under the registry's
//! `HKLM\System\CurrentControlSet\Control\MediaResources\ACM\msacm.<name>`
//! with their file in `Driver`, as Windows 98's MSACM32 finds them. The
//! Windows Media Audio codec, msaud32.acm, is one.
//!
//! A driver is a DLL with a DriverProc. MSACM32 opens it through WinMM's
//! OpenDriver, which loads the DLL and sends DRV_LOAD, DRV_ENABLE and
//! DRV_OPEN with an ACMDRVOPENDESC, and talks to it through
//! SendDriverMessage (ACMDM_DRIVER_DETAILS, ACMDM_FORMAT_SUGGEST,
//! ACMDM_STREAM_OPEN and the rest), as Wine's MSACM32 does. The driver
//! runs as the program's code, so the ACM's functions that reach it are
//! tasks (task.rs), which call WinMM's functions as the program would.
//!
//! A process finds its drivers the first time it asks for one, and
//! doesn't see one installed after that. A function given no driver
//! (acmFormatSuggest, acmStreamOpen) tries each in turn: it opens the
//! driver, asks, and closes it again unless a stream keeps it. Windows
//! keeps what each driver offers in the registry (its format tags and
//! what it supports) to pass over drivers that can't help; here a
//! driver that can't refuses, which comes to the same answer.

use super::{
    ACMERR_NOTPOSSIBLE, ACMERR_UNPREPARED, HEADER_DESTINATION, HEADER_DESTINATION_LENGTH,
    HEADER_DESTINATION_USED, HEADER_SOURCE, HEADER_SOURCE_LENGTH, HEADER_SOURCE_USED,
    HEADER_STATUS, SIZE_DESTINATION, STATUS_DONE, STATUS_INQUEUE, STATUS_PREPARED, Stream, state,
};
use crate::cpu::Cpu;
use crate::savestate::Since;
use crate::win32::kernel32::virtual_memory::allows;
use crate::win32::task::{self, Next, Task};
use crate::win32::user32::current_pid;
use crate::win32::winmm::{
    MMSYSERR_INVALFLAG, MMSYSERR_INVALHANDLE, MMSYSERR_INVALPARAM, MMSYSERR_NODRIVER,
    MMSYSERR_NOERROR, MMSYSERR_NOTSUPPORTED,
};

/// A driver a process can open (HACMDRIVERID): its handle, its alias
/// (`msacm.<name>`) and its DLL.
#[derive(Clone, Debug, Default)]
pub struct DriverId {
    pub pid: u32,
    pub handle: u32,
    pub alias: String,
    pub file: String,
}

crate::state_fields!(DriverId {
    pid,
    handle,
    alias,
    file
});

/// A driver a process opened (HACMDRIVER): its handle, the driver it is
/// of (`id`) and WinMM's handle of it.
#[derive(Clone, Debug, Default)]
pub struct Driver {
    pub pid: u32,
    pub handle: u32,
    pub id: u32,
    pub hdrvr: u32,
}

crate::state_fields!(Driver {
    pid,
    handle,
    id,
    hdrvr
});

/// Where the drivers' handles start.
const FIRST_ID: u32 = 0x0AD0_0000;
const FIRST_DRIVER: u32 = 0x0AE0_0000;

/// The registry key of the drivers that Windows 98's setup and the
/// codecs' own installs register.
const MEDIA_RESOURCES: &str = r"System\CurrentControlSet\Control\MediaResources\ACM";

/// The driver messages MSACM32 sends.
const DRV_CONFIGURE: u32 = 7;
const DRV_QUERYCONFIGURE: u32 = 8;
const ACMDM_USER: u32 = 0x4000;
const ACMDM_BASE: u32 = 0x6000;
const ACMDM_DRIVER_DETAILS: u32 = ACMDM_BASE + 10;
const ACMDM_FORMAT_SUGGEST: u32 = ACMDM_BASE + 27;
const ACMDM_STREAM_OPEN: u32 = ACMDM_BASE + 76;
const ACMDM_STREAM_CLOSE: u32 = ACMDM_BASE + 77;
const ACMDM_STREAM_SIZE: u32 = ACMDM_BASE + 78;
const ACMDM_STREAM_CONVERT: u32 = ACMDM_BASE + 79;
const ACMDM_STREAM_PREPARE: u32 = ACMDM_BASE + 81;
const ACMDM_STREAM_UNPREPARE: u32 = ACMDM_BASE + 82;

/// ACMDRVOPENDESC's fccType for an audio codec: 'audc'.
const FCCTYPE_AUDIOCODEC: u32 = u32::from_le_bytes(*b"audc");

/// acmDriverEnum's flags, the top four bits.
const DRIVER_ENUM_FLAGS: u32 = 0xF000_0000;

/// ACMDRIVERDETAILS: its size in Unicode, which drivers fill, and in
/// ANSI; its fields before the strings, which both share; where
/// fdwSupport is; and the lengths of its strings, in characters.
const DETAILS_W_SIZE: u32 = 1800;
const DETAILS_A_SIZE: u32 = 920;
const DETAILS_HEAD: usize = 40;
const DETAILS_SUPPORT: u32 = 0x18;
const DETAILS_STRINGS: [usize; 5] = [32, 128, 80, 128, 512];

/// ACMDRVSTREAMINSTANCE and its fields: what MSACM32 keeps of a stream
/// for its driver, in the system's heap, with the stream's formats and
/// filter after it.
const INSTANCE_SIZE: u32 = 40;
const INSTANCE_SOURCE: u32 = 4;
const INSTANCE_DESTINATION: u32 = 8;
const INSTANCE_FILTER: u32 = 12;
const INSTANCE_CALLBACK: u32 = 16;
const INSTANCE_INSTANCE: u32 = 20;
const INSTANCE_OPEN: u32 = 24;
const INSTANCE_STREAM: u32 = 36;
/// WAVEFILTER's size.
const FILTER_SIZE: u32 = 32;

/// ACMDRVSTREAMHEADER's fields past ACMSTREAMHEADER's own, which the
/// ACM and the driver keep.
const HEADER_CONVERT: u32 = 0x2C;
const HEADER_PREPARED: u32 = 0x3C;
const HEADER_PREPARED_SOURCE: u32 = 0x44;
const HEADER_PREPARED_SOURCE_LENGTH: u32 = 0x48;
const HEADER_PREPARED_DESTINATION: u32 = 0x4C;
const HEADER_PREPARED_DESTINATION_LENGTH: u32 = 0x50;

/// The task's scratch space: the message's structure (ACMDRVFORMATSUGGEST
/// or ACMDRVSTREAMSIZE), then for the functions that open a driver its
/// ACMDRVOPENDESC, the strings it points to, the DLL's name for
/// OpenDriver and ACMDRIVERDETAILSW.
const S_MESSAGE: u32 = 0;
const S_OPEN_DESC: u32 = 32;
const S_SECTION: u32 = 72;
const S_ALIAS: u32 = 96;
const S_FILE: u32 = 352;
const S_DETAILS: u32 = 872;
const SCRATCH_OPEN: u32 = S_DETAILS + DETAILS_W_SIZE;
const SCRATCH_MESSAGE: u32 = S_OPEN_DESC;
/// The longest alias and file name the scratch space holds, in
/// characters, with their zero.
const ALIAS_CHARS: usize = 128;
const FILE_CHARS: usize = 260;

/// The task's variables: which function it is, the driver it tries or
/// opened (a HACMDRIVERID), WinMM's handle of it, a result kept across a
/// call, and the function's own arguments.
const V_OP: usize = 0;
const V_ID: usize = 1;
const V_HDRVR: usize = 2;
const V_RESULT: usize = 3;
const V_A: usize = 4;
const V_B: usize = 5;
const V_C: usize = 6;
const V_D: usize = 7;

/// The functions, by V_OP.
const OP_DRIVER_OPEN: u32 = 0;
const OP_DRIVER_CLOSE: u32 = 1;
const OP_DRIVER_MESSAGE: u32 = 2;
const OP_DRIVER_DETAILS: u32 = 3;
const OP_DRIVER_ENUM: u32 = 4;
const OP_SUGGEST: u32 = 5;
const OP_STREAM_OPEN: u32 = 6;
const OP_STREAM_CLOSE: u32 = 7;
const OP_STREAM: u32 = 8;

/// The process's drivers, found the first time it asks: those of
/// SYSTEM.INI's `[drivers32]`, then those of the registry that SYSTEM.INI
/// doesn't name. A name needs the `msacm.` prefix to be an ACM driver.
fn register(cpu: &mut Cpu) {
    let pid = current_pid(cpu);
    if state(cpu).registered.contains(&pid) {
        return;
    }
    state(cpu).registered.push(pid);
    let ini = crate::win32::kernel32::profile_entries(cpu, b"SYSTEM.INI", b"drivers32");
    let mut found: Vec<(String, String)> = ini
        .iter()
        .map(|(key, value)| {
            (
                crate::dosstr::from_bytes(key),
                crate::dosstr::from_bytes(value),
            )
        })
        .collect();
    let hklm = crate::win32::registry::HKEY_LOCAL_MACHINE;
    found.extend(crate::win32::registry::subkey_strings(
        cpu,
        hklm,
        MEDIA_RESOURCES,
        "Driver",
    ));
    let mut ids: Vec<DriverId> = Vec::new();
    for (alias, file) in found {
        let acm = alias
            .get(..6)
            .is_some_and(|p| p.eq_ignore_ascii_case("msacm."));
        // What follows the file's name is the driver's, not the DLL's.
        let file = file.split(' ').next().unwrap_or_default().to_string();
        let known = ids.iter().any(|d| d.alias.eq_ignore_ascii_case(&alias));
        if !acm || file.is_empty() || known {
            continue;
        }
        cpu.bus
            .log_string(&format!("[W32] MSACM32: driver {} is {}", alias, file));
        let handle = new_handle(cpu, FIRST_ID);
        ids.push(DriverId {
            pid,
            handle,
            alias,
            file,
        });
    }
    state(cpu).ids.extend(ids);
}

/// A new handle of an ACM object, counting up from `first`.
pub(super) fn new_handle(cpu: &mut Cpu, first: u32) -> u32 {
    let s = state(cpu);
    let handle = first + s.next;
    s.next += 1;
    handle
}

/// The process ended: its drivers went with it.
pub(super) fn process_ended(cpu: &mut Cpu, pid: u32) {
    let s = state(cpu);
    s.registered.retain(|&p| p != pid);
    s.ids.retain(|d| d.pid != pid);
    s.drivers.retain(|d| d.pid != pid);
}

/// Whether the running process has drivers to try, finding them if it
/// hasn't yet.
pub(super) fn any(cpu: &mut Cpu) -> bool {
    register(cpu);
    let pid = current_pid(cpu);
    state(cpu).ids.iter().any(|d| d.pid == pid)
}

/// The running process's driver `handle` (a HACMDRIVERID).
fn id(cpu: &mut Cpu, handle: u32) -> Option<DriverId> {
    register(cpu);
    let pid = current_pid(cpu);
    state(cpu)
        .ids
        .iter()
        .find(|d| d.pid == pid && d.handle == handle)
        .cloned()
}

/// The running process's driver after `after` in their order, the
/// first for 0.
fn next_id(cpu: &mut Cpu, after: u32) -> Option<DriverId> {
    register(cpu);
    let pid = current_pid(cpu);
    state(cpu)
        .ids
        .iter()
        .find(|d| d.pid == pid && d.handle > after)
        .cloned()
}

/// The running process's open driver `handle` (a HACMDRIVER).
pub(super) fn driver(cpu: &mut Cpu, handle: u32) -> Option<Driver> {
    let pid = current_pid(cpu);
    state(cpu)
        .drivers
        .iter()
        .find(|d| d.pid == pid && d.handle == handle)
        .cloned()
}

/// Keep the driver `id` that WinMM opened as `hdrvr`: its HACMDRIVER.
fn add_driver(cpu: &mut Cpu, id: u32, hdrvr: u32) -> u32 {
    let pid = current_pid(cpu);
    let handle = new_handle(cpu, FIRST_DRIVER);
    state(cpu).drivers.push(Driver {
        pid,
        handle,
        id,
        hdrvr,
    });
    handle
}

fn remove_driver(cpu: &mut Cpu, handle: u32) {
    state(cpu).drivers.retain(|d| d.handle != handle);
}

/// The address of WinMM's function `name`, WinMM mapped in the process
/// if it isn't yet, as MSACM32 imports it. 0 if it can't be: the call
/// then returns 0.
fn winmm(cpu: &mut Cpu, name: &str) -> u32 {
    if crate::win32::loader::map_builtin_named(cpu, "WINMM.DLL").is_none() {
        return 0;
    }
    crate::win32::winmm::address_of(cpu, name)
}

/// Write `text` at `at` in Unicode, cut to `chars` characters with its
/// zero.
fn put_wide(cpu: &mut Cpu, at: u32, text: &str, chars: usize) {
    let mut units = crate::win32::kernel32::ansi_to_wide(cpu, text.as_bytes());
    units.truncate(chars - 1);
    units.push(0);
    let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
    cpu.bus.guest_write_bytes(at, &bytes);
}

/// Open the driver `d` through OpenDriver, with its ACMDRVOPENDESC in
/// the scratch space; `step` goes on with WinMM's handle, 0 if it
/// failed. The DLL is named by its file: OpenDriver would find it by
/// the alias only in SYSTEM.INI, and a driver of the registry isn't
/// there.
fn open(cpu: &mut Cpu, t: &mut Task, d: &DriverId, step: u16) -> Next {
    t.set(V_ID, d.handle);
    let s = t.scratch;
    put_wide(cpu, s + S_SECTION, "Drivers32", 12);
    put_wide(cpu, s + S_ALIAS, &d.alias, ALIAS_CHARS);
    put_wide(cpu, s + S_FILE, &d.file, FILE_CHARS);
    let version = super::acm_get_version(cpu, &[]).unwrap_or(0);
    let desc = [
        36,
        FCCTYPE_AUDIOCODEC,
        0,
        version,
        0,
        0,
        s + S_SECTION,
        s + S_ALIAS,
        0,
    ];
    for (i, word) in desc.into_iter().enumerate() {
        cpu.bus.guest_write_32(s + S_OPEN_DESC + 4 * i as u32, word);
    }
    let target = winmm(cpu, "OpenDriver");
    Next::call(target, &[s + S_FILE, 0, s + S_OPEN_DESC], step)
}

/// What a failed open returns: the error the driver put in its
/// ACMDRVOPENDESC, or MMSYSERR_NODRIVER.
fn open_error(cpu: &mut Cpu, t: &Task) -> u32 {
    match cpu.bus.guest_read_32(t.scratch + S_OPEN_DESC + 20) {
        0 => MMSYSERR_NODRIVER,
        e => e,
    }
}

/// Send the driver WinMM opened as `hdrvr` `message`; `step` goes on
/// with what its DriverProc returns.
fn send(cpu: &mut Cpu, hdrvr: u32, message: u32, p1: u32, p2: u32, step: u16) -> Next {
    let target = winmm(cpu, "SendDriverMessage");
    Next::call(target, &[hdrvr, message, p1, p2], step)
}

/// Close the driver WinMM opened as `hdrvr`; `step` goes on.
fn close(cpu: &mut Cpu, hdrvr: u32, step: u16) -> Next {
    let target = winmm(cpu, "CloseDriver");
    Next::call(target, &[hdrvr, 0, 0], step)
}

/// The steps of MSACM32's functions that reach a driver.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.var(V_OP) {
        OP_DRIVER_OPEN => driver_open_step(cpu, t, value),
        OP_DRIVER_CLOSE => driver_close_step(cpu, t),
        OP_DRIVER_MESSAGE => driver_message_step(cpu, t, value),
        OP_DRIVER_DETAILS => details_step(cpu, t, value),
        OP_DRIVER_ENUM => enum_step(cpu, t, value),
        OP_SUGGEST => suggest_step(cpu, t, value),
        OP_STREAM_OPEN => stream_open_step(cpu, t, value),
        OP_STREAM_CLOSE => stream_close_step(cpu, t, value),
        OP_STREAM => stream_step(cpu, t, value),
        _ => Next::Done(MMSYSERR_INVALHANDLE),
    }
}

/// acmDriverOpen(out, driver, flags): the driver `driver` opened, its
/// handle at `out`.
pub(super) fn driver_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[out, handle, flags] = args else {
        unreachable!()
    };
    if !super::clear(cpu, out) {
        return Some(MMSYSERR_INVALPARAM);
    }
    if flags != 0 {
        return Some(MMSYSERR_INVALFLAG);
    }
    if id(cpu, handle).is_none() {
        return Some(MMSYSERR_INVALHANDLE);
    }
    let vars = vec![OP_DRIVER_OPEN, handle, 0, 0, out];
    task::start(cpu, task::MSACM32, vars, SCRATCH_OPEN)
}

fn driver_open_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.step {
        0 => match id(cpu, t.var(V_ID)) {
            Some(d) => open(cpu, t, &d, 1),
            None => Next::Done(MMSYSERR_INVALHANDLE),
        },
        _ if value == 0 => Next::Done(open_error(cpu, t)),
        _ => {
            let handle = add_driver(cpu, t.var(V_ID), value);
            cpu.bus.guest_write_32(t.var(V_A), handle);
            Next::Done(MMSYSERR_NOERROR)
        }
    }
}

/// acmDriverClose(driver, flags).
pub(super) fn driver_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, flags] = args else {
        unreachable!()
    };
    if flags != 0 {
        return Some(MMSYSERR_INVALFLAG);
    }
    let Some(d) = driver(cpu, handle) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    // Windows refuses to close a driver a stream still uses.
    if state(cpu).streams.values().any(|s| *s.driver == handle) {
        return Some(super::ACMERR_BUSY);
    }
    remove_driver(cpu, handle);
    let vars = vec![OP_DRIVER_CLOSE, d.id, d.hdrvr];
    task::start(cpu, task::MSACM32, vars, 0)
}

fn driver_close_step(cpu: &mut Cpu, t: &mut Task) -> Next {
    match t.step {
        0 => close(cpu, t.var(V_HDRVR), 1),
        _ => Next::Done(MMSYSERR_NOERROR),
    }
}

/// acmDriverMessage(driver, message, first, second): the driver's own
/// messages (from ACMDM_USER to the ACM's) and the two of its
/// configuration go to it; the ACM's own messages may not be sent this
/// way (issue 443 has what Windows 98's code checks).
pub(super) fn driver_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, message, p1, p2] = args else {
        unreachable!()
    };
    let allowed = (ACMDM_USER..ACMDM_BASE).contains(&message)
        || matches!(message, DRV_QUERYCONFIGURE | DRV_CONFIGURE);
    if !allowed {
        return Some(MMSYSERR_INVALPARAM);
    }
    let Some(d) = driver(cpu, handle) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    let vars = vec![OP_DRIVER_MESSAGE, d.id, d.hdrvr, 0, message, p1, p2];
    task::start(cpu, task::MSACM32, vars, 0)
}

fn driver_message_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.step {
        0 => {
            let (message, p1, p2) = (t.var(V_A), t.var(V_B), t.var(V_C));
            send(cpu, t.var(V_HDRVR), message, p1, p2, 1)
        }
        _ => Next::Done(value),
    }
}

/// acmDriverID(object, out, flags): the driver an open driver or a
/// stream of a driver is of, or a driver's own handle.
pub(super) fn driver_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[object, out, flags] = args else {
        unreachable!()
    };
    if flags != 0 {
        return Some(MMSYSERR_INVALFLAG);
    }
    let of_stream = state(cpu)
        .streams
        .get(&object)
        .map(|s| *s.driver)
        .filter(|&d| d != 0);
    let found = match of_stream {
        Some(d) => driver(cpu, d).map(|d| d.id),
        None => match driver(cpu, object) {
            Some(d) => Some(d.id),
            None => id(cpu, object).map(|d| d.handle),
        },
    };
    let Some(found) = found else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    if !allows(cpu, out, 4, true) {
        return Some(MMSYSERR_INVALPARAM);
    }
    cpu.bus.guest_write_32(out, found);
    Some(MMSYSERR_NOERROR)
}

/// acmDriverDetailsA and W(driver, details, flags): what the driver
/// says of itself, as much as the program's structure holds.
pub(super) fn driver_details(cpu: &mut Cpu, args: &[u32], wide: bool) -> Option<u32> {
    let &[handle, details, flags] = args else {
        unreachable!()
    };
    if flags != 0 {
        return Some(MMSYSERR_INVALFLAG);
    }
    if id(cpu, handle).is_none() {
        return Some(MMSYSERR_INVALHANDLE);
    }
    if !allows(cpu, details, 4, true) {
        return Some(MMSYSERR_INVALPARAM);
    }
    let size = cpu.bus.guest_read_32(details);
    if size < 4 || !allows(cpu, details, size, true) {
        return Some(MMSYSERR_INVALPARAM);
    }
    let vars = vec![OP_DRIVER_DETAILS, handle, 0, 0, details, wide as u32];
    task::start(cpu, task::MSACM32, vars, SCRATCH_OPEN)
}

fn details_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let s = t.scratch;
    match t.step {
        0 => match id(cpu, t.var(V_ID)) {
            Some(d) => open(cpu, t, &d, 1),
            None => Next::Done(MMSYSERR_INVALHANDLE),
        },
        1 if value == 0 => match open_error(cpu, t) {
            MMSYSERR_NODRIVER => Next::Done(MMSYSERR_NOTSUPPORTED),
            e => Next::Done(e),
        },
        1 => {
            t.set(V_HDRVR, value);
            cpu.bus.guest_write_32(s + S_DETAILS, DETAILS_W_SIZE);
            send(cpu, value, ACMDM_DRIVER_DETAILS, s + S_DETAILS, 0, 2)
        }
        2 => {
            t.set(V_RESULT, value);
            close(cpu, t.var(V_HDRVR), 3)
        }
        _ => {
            let result = t.var(V_RESULT);
            if result == MMSYSERR_NOERROR {
                copy_details(cpu, s + S_DETAILS, t.var(V_A), t.var(V_B) != 0);
            }
            Next::Done(result)
        }
    }
}

/// Copy the ACMDRIVERDETAILSW at `from` to the program's structure at
/// `to`, in ANSI unless `wide`: as much of it as the program's cbStruct
/// says, which then says how much that was.
fn copy_details(cpu: &mut Cpu, from: u32, to: u32, wide: bool) {
    let mut w = vec![0; DETAILS_W_SIZE as usize];
    cpu.bus.guest_read_bytes(from, &mut w);
    let bytes = if wide {
        w
    } else {
        let mut a = w[..DETAILS_HEAD].to_vec();
        let mut at = DETAILS_HEAD;
        for chars in DETAILS_STRINGS {
            let units: Vec<u16> = w[at..at + 2 * chars]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_le_bytes(c))
                .take_while(|&u| u != 0)
                .collect();
            let mut text = crate::win32::kernel32::wide_to_ansi(cpu, &units);
            text.resize(chars, 0);
            text[chars - 1] = 0;
            a.extend(text);
            at += 2 * chars;
        }
        a
    };
    debug_assert_eq!(
        bytes.len(),
        if wide { DETAILS_W_SIZE } else { DETAILS_A_SIZE } as usize
    );
    let size = cpu.bus.guest_read_32(to).min(bytes.len() as u32);
    cpu.bus.guest_write_bytes(to, &bytes[..size as usize]);
    cpu.bus.guest_write_32(to, size);
}

/// acmDriverEnum(callback, instance, flags): the callback gets each
/// driver that opens, with what it supports, until it returns FALSE.
pub(super) fn driver_enum(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[callback, instance, flags] = args else {
        unreachable!()
    };
    if !allows(cpu, callback, 1, false) {
        return Some(MMSYSERR_INVALPARAM);
    }
    if flags & !DRIVER_ENUM_FLAGS != 0 {
        return Some(MMSYSERR_INVALFLAG);
    }
    if !any(cpu) {
        return Some(MMSYSERR_NOERROR);
    }
    let vars = vec![OP_DRIVER_ENUM, 0, 0, 0, callback, instance];
    task::start(cpu, task::MSACM32, vars, SCRATCH_OPEN)
}

fn enum_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let s = t.scratch;
    match t.step {
        1 if value != 0 => {
            t.set(V_HDRVR, value);
            cpu.bus
                .guest_fill(s + S_DETAILS, DETAILS_W_SIZE as usize, 0);
            cpu.bus.guest_write_32(s + S_DETAILS, DETAILS_W_SIZE);
            send(cpu, value, ACMDM_DRIVER_DETAILS, s + S_DETAILS, 0, 2)
        }
        2 => {
            t.set(V_RESULT, value);
            close(cpu, t.var(V_HDRVR), 3)
        }
        3 if t.var(V_RESULT) == MMSYSERR_NOERROR => {
            let support = cpu.bus.guest_read_32(s + S_DETAILS + DETAILS_SUPPORT);
            let args = [t.var(V_ID), t.var(V_B), support];
            Next::call(t.var(V_A), &args, 4)
        }
        4 if value == 0 => Next::Done(MMSYSERR_NOERROR),
        // The first driver, or the next: one that doesn't open or say
        // what it is isn't listed.
        _ => match next_id(cpu, t.var(V_ID)) {
            Some(d) => open(cpu, t, &d, 1),
            None => Next::Done(MMSYSERR_NOERROR),
        },
    }
}

/// acmFormatSuggest(driver, source, destination, size, flags) of a
/// format the built-in codecs don't take: the driver `driver` suggests,
/// or with none each driver in turn until one does. The arguments are
/// checked.
pub(super) fn format_suggest(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, source, destination, size, flags] = args else {
        unreachable!()
    };
    let hdrvr = match handle {
        0 => 0,
        h => match driver(cpu, h) {
            Some(d) => d.hdrvr,
            None => return Some(MMSYSERR_INVALHANDLE),
        },
    };
    if handle == 0 && !any(cpu) {
        return Some(ACMERR_NOTPOSSIBLE);
    }
    let Some(source_size) = format_size(cpu, source) else {
        return Some(MMSYSERR_INVALPARAM);
    };
    // ACMDRVFORMATSUGGEST.
    let vars = vec![
        OP_SUGGEST,
        0,
        hdrvr,
        0,
        flags,
        source,
        source_size,
        destination,
        size,
    ];
    task::start(cpu, task::MSACM32, vars, SCRATCH_OPEN)
}

/// Write the ACMDRVFORMATSUGGEST of the task's arguments in its scratch
/// space.
fn put_suggest(cpu: &mut Cpu, t: &Task) {
    let at = t.scratch + S_MESSAGE;
    let words = [
        24,
        t.var(V_A),
        t.var(V_B),
        t.var(V_C),
        t.var(V_D),
        t.var(V_D + 1),
    ];
    for (i, word) in words.into_iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, word);
    }
}

fn suggest_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let message = t.scratch + S_MESSAGE;
    // A driver the program opened.
    if t.var(V_ID) == 0 && t.var(V_HDRVR) != 0 {
        return match t.step {
            0 => {
                put_suggest(cpu, t);
                send(cpu, t.var(V_HDRVR), ACMDM_FORMAT_SUGGEST, message, 0, 1)
            }
            _ => Next::Done(value),
        };
    }
    match t.step {
        1 if value != 0 => {
            t.set(V_HDRVR, value);
            put_suggest(cpu, t);
            send(cpu, value, ACMDM_FORMAT_SUGGEST, message, 0, 2)
        }
        2 => {
            t.set(V_RESULT, value);
            close(cpu, t.var(V_HDRVR), 3)
        }
        3 if t.var(V_RESULT) == MMSYSERR_NOERROR => Next::Done(MMSYSERR_NOERROR),
        _ => match next_id(cpu, t.var(V_ID)) {
            Some(d) => open(cpu, t, &d, 1),
            None => Next::Done(ACMERR_NOTPOSSIBLE),
        },
    }
}

/// The size of the wave format at `at` as the ACM copies it: a
/// WAVEFORMATEX and its extra bytes, PCM's with none. None if the
/// program may not read it.
fn format_size(cpu: &mut Cpu, at: u32) -> Option<u32> {
    if !allows(cpu, at, 16, false) {
        return None;
    }
    if cpu.bus.guest_read_16(at) == super::WAVE_FORMAT_PCM {
        return Some(18);
    }
    if !allows(cpu, at, 18, false) {
        return None;
    }
    let size = 18 + cpu.bus.guest_read_16(at + 16) as u32;
    allows(cpu, at, size, false).then_some(size)
}

/// Copy the wave format at `from` to `to` as the ACM keeps it: PCM's
/// with no extra bytes, as WAVEFORMATEX.
fn copy_format(cpu: &mut Cpu, from: u32, to: u32, size: u32) {
    let mut bytes = vec![0; size as usize];
    let pcm = cpu.bus.guest_read_16(from) == super::WAVE_FORMAT_PCM;
    let len = if pcm { 16 } else { size as usize };
    cpu.bus.guest_read_bytes(from, &mut bytes[..len]);
    cpu.bus.guest_write_bytes(to, &bytes);
}

/// acmStreamOpen(out, driver, source, destination, filter, callback,
/// instance, flags) of formats the built-in codecs don't take: a stream
/// of the driver `driver`, or with none of the first driver that takes
/// it. The arguments are checked, `out` cleared. Its ACMDRVSTREAMINSTANCE
/// goes in the system's heap with copies of the formats and the filter,
/// as the driver keeps a pointer to it.
pub(super) fn stream_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[
        out,
        handle,
        source,
        destination,
        filter,
        callback,
        user,
        flags,
    ] = args
    else {
        unreachable!()
    };
    let hdrvr = match handle {
        0 => 0,
        h => match driver(cpu, h) {
            Some(d) => d.hdrvr,
            None => return Some(MMSYSERR_INVALHANDLE),
        },
    };
    if handle == 0 && !any(cpu) {
        return Some(ACMERR_NOTPOSSIBLE);
    }
    let (Some(source_size), Some(destination_size)) =
        (format_size(cpu, source), format_size(cpu, destination))
    else {
        return Some(MMSYSERR_INVALPARAM);
    };
    if filter != 0 && !allows(cpu, filter, FILTER_SIZE, false) {
        return Some(MMSYSERR_INVALPARAM);
    }
    let filter_size = if filter != 0 { FILTER_SIZE } else { 0 };
    let total = INSTANCE_SIZE + source_size + destination_size + filter_size;
    let Some(at) = crate::win32::with_process(cpu, |p, bus| p.system_alloc(bus, total)) else {
        return Some(super::MMSYSERR_NOMEM);
    };
    cpu.bus.guest_fill(at, total as usize, 0);
    let source_copy = at + INSTANCE_SIZE;
    let destination_copy = source_copy + source_size;
    copy_format(cpu, source, source_copy, source_size);
    copy_format(cpu, destination, destination_copy, destination_size);
    let filter_copy = if filter != 0 {
        let to = destination_copy + destination_size;
        let mut bytes = vec![0; FILTER_SIZE as usize];
        cpu.bus.guest_read_bytes(filter, &mut bytes);
        cpu.bus.guest_write_bytes(to, &bytes);
        to
    } else {
        0
    };
    let query = flags & super::STREAM_OPEN_QUERY != 0;
    let stream = if query {
        0
    } else {
        new_handle(cpu, super::FIRST_STREAM)
    };
    let words = [
        (0, INSTANCE_SIZE),
        (INSTANCE_SOURCE, source_copy),
        (INSTANCE_DESTINATION, destination_copy),
        (INSTANCE_FILTER, filter_copy),
        (INSTANCE_CALLBACK, callback),
        (INSTANCE_INSTANCE, user),
        (INSTANCE_OPEN, flags),
        (INSTANCE_STREAM, stream),
    ];
    for (offset, word) in words {
        cpu.bus.guest_write_32(at + offset, word);
    }
    let vars = vec![OP_STREAM_OPEN, 0, hdrvr, 0, out, handle, at, stream];
    task::start(cpu, task::MSACM32, vars, SCRATCH_OPEN)
}

/// Keep the stream the task opened, of the driver `handle`, which it
/// closes with the stream when `owns`.
fn add_stream(cpu: &mut Cpu, t: &Task, handle: u32, owns: bool) {
    let pid = current_pid(cpu);
    let stream = Stream {
        pid,
        driver: Since(handle),
        instance: Since(t.var(V_C)),
        owns: Since(owns),
        ..Stream::default()
    };
    state(cpu).streams.insert(t.var(V_D), stream);
    cpu.bus.guest_write_32(t.var(V_A), t.var(V_D));
}

/// The stream's ACMDRVSTREAMINSTANCE goes back to the heap.
fn free_instance(cpu: &mut Cpu, at: u32) {
    crate::win32::with_process(cpu, |p, bus| p.system_free(bus, at));
}

fn stream_open_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let instance = t.var(V_C);
    let query = t.var(V_D) == 0;
    // A driver the program opened.
    if t.var(V_B) != 0 {
        return match t.step {
            0 => send(cpu, t.var(V_HDRVR), ACMDM_STREAM_OPEN, instance, 0, 1),
            1 if value != MMSYSERR_NOERROR => {
                free_instance(cpu, instance);
                Next::Done(value)
            }
            1 if query => send(cpu, t.var(V_HDRVR), ACMDM_STREAM_CLOSE, instance, 0, 2),
            1 => {
                add_stream(cpu, t, t.var(V_B), false);
                Next::Done(MMSYSERR_NOERROR)
            }
            _ => {
                free_instance(cpu, instance);
                Next::Done(MMSYSERR_NOERROR)
            }
        };
    }
    match t.step {
        // Opened: the driver's stream.
        1 if value != 0 => {
            t.set(V_HDRVR, value);
            send(cpu, value, ACMDM_STREAM_OPEN, instance, 0, 2)
        }
        2 if value == MMSYSERR_NOERROR && query => {
            send(cpu, t.var(V_HDRVR), ACMDM_STREAM_CLOSE, instance, 0, 3)
        }
        2 if value == MMSYSERR_NOERROR => {
            let handle = add_driver(cpu, t.var(V_ID), t.var(V_HDRVR));
            add_stream(cpu, t, handle, true);
            Next::Done(MMSYSERR_NOERROR)
        }
        // It refused: the next one.
        2 => close(cpu, t.var(V_HDRVR), 4),
        // A query it took: it is closed again.
        3 => close(cpu, t.var(V_HDRVR), 5),
        5 => {
            free_instance(cpu, instance);
            Next::Done(MMSYSERR_NOERROR)
        }
        _ => match next_id(cpu, t.var(V_ID)) {
            Some(d) => open(cpu, t, &d, 1),
            None => {
                free_instance(cpu, instance);
                Next::Done(ACMERR_NOTPOSSIBLE)
            }
        },
    }
}

/// acmStreamClose(stream, flags) of a driver's stream: the driver closes
/// it, and then the driver the stream opened is closed.
pub(super) fn stream_close(cpu: &mut Cpu, handle: u32, s: &Stream) -> Option<u32> {
    let Some(d) = driver(cpu, *s.driver) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    let vars = vec![OP_STREAM_CLOSE, d.handle, d.hdrvr, 0, handle, *s.instance];
    task::start(cpu, task::MSACM32, vars, 0)
}

fn stream_close_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let (handle, instance) = (t.var(V_A), t.var(V_B));
    match t.step {
        0 => send(cpu, t.var(V_HDRVR), ACMDM_STREAM_CLOSE, instance, 0, 1),
        1 if value != MMSYSERR_NOERROR => Next::Done(value),
        1 => {
            let owns = state(cpu).streams.remove(&handle).is_some_and(|s| *s.owns);
            free_instance(cpu, instance);
            if owns {
                remove_driver(cpu, t.var(V_ID));
                close(cpu, t.var(V_HDRVR), 2)
            } else {
                Next::Done(MMSYSERR_NOERROR)
            }
        }
        _ => Next::Done(MMSYSERR_NOERROR),
    }
}

/// What a driver's stream is asked, by the message the task sends.
/// acmStreamSize(stream, size, out, flags): the driver sizes the other
/// side of `size` bytes.
pub(super) fn stream_size(
    cpu: &mut Cpu,
    s: &Stream,
    size: u32,
    out: u32,
    flags: u32,
) -> Option<u32> {
    let Some(d) = driver(cpu, *s.driver) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    let message = ACMDM_STREAM_SIZE;
    let vars = vec![
        OP_STREAM,
        0,
        d.hdrvr,
        0,
        message,
        *s.instance,
        out,
        flags,
        size,
    ];
    task::start(cpu, task::MSACM32, vars, SCRATCH_MESSAGE)
}

/// acmStreamPrepareHeader, acmStreamUnprepareHeader and acmStreamConvert
/// (stream, header, flags) of a driver's stream, the header checked:
/// the ACM's part of the header is set, the driver gets it and the
/// status follows what the driver says. A driver that doesn't prepare
/// or unprepare headers (MMSYSERR_NOTSUPPORTED) leaves it to the ACM.
pub(super) fn stream_header(
    cpu: &mut Cpu,
    s: &Stream,
    message: u32,
    header: u32,
    flags: u32,
) -> Option<u32> {
    let Some(d) = driver(cpu, *s.driver) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    let status = cpu.bus.guest_read_32(header + HEADER_STATUS);
    let read = |cpu: &mut Cpu, field| cpu.bus.guest_read_32(header + field);
    let prepared_as_now = read(cpu, HEADER_PREPARED_SOURCE) == read(cpu, HEADER_SOURCE)
        && read(cpu, HEADER_PREPARED_SOURCE_LENGTH) >= read(cpu, HEADER_SOURCE_LENGTH)
        && read(cpu, HEADER_PREPARED_DESTINATION) == read(cpu, HEADER_DESTINATION)
        && read(cpu, HEADER_PREPARED_DESTINATION_LENGTH) >= read(cpu, HEADER_DESTINATION_LENGTH);
    match message {
        ACMDM_STREAM_PREPARE => {
            if status & STATUS_PREPARED != 0 {
                return Some(MMSYSERR_NOERROR);
            }
            // fdwConvert to dwPrepared and the prepared buffers.
            cpu.bus.guest_fill(header + HEADER_CONVERT, 0x54 - 0x2C, 0);
        }
        _ => {
            if status & STATUS_PREPARED == 0 {
                return Some(ACMERR_UNPREPARED);
            }
            if !prepared_as_now {
                return Some(MMSYSERR_INVALPARAM);
            }
            cpu.bus.guest_write_32(header + HEADER_CONVERT, flags);
        }
    }
    if message == ACMDM_STREAM_CONVERT {
        cpu.bus.guest_write_32(header + HEADER_SOURCE_USED, 0);
        cpu.bus.guest_write_32(header + HEADER_DESTINATION_USED, 0);
    }
    let vars = vec![OP_STREAM, 0, d.hdrvr, 0, message, *s.instance, header];
    task::start(cpu, task::MSACM32, vars, 0)
}

fn stream_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let (message, instance, at) = (t.var(V_A), t.var(V_B), t.var(V_C));
    if t.step == 0 {
        if message == ACMDM_STREAM_SIZE {
            // ACMDRVSTREAMSIZE: the size given on its side.
            let flags = t.var(V_D);
            let size = t.var(V_D + 1);
            let destination = flags & 0xF == SIZE_DESTINATION;
            let (source, dest) = if destination { (0, size) } else { (size, 0) };
            let words = [16, flags, source, dest];
            for (i, word) in words.into_iter().enumerate() {
                cpu.bus.guest_write_32(t.scratch + 4 * i as u32, word);
            }
            return send(cpu, t.var(V_HDRVR), message, instance, t.scratch, 1);
        }
        return send(cpu, t.var(V_HDRVR), message, instance, at, 1);
    }
    if message == ACMDM_STREAM_SIZE {
        if value == MMSYSERR_NOERROR {
            let destination = t.var(V_D) & 0xF == SIZE_DESTINATION;
            let field = if destination { 8 } else { 12 };
            let bytes = cpu.bus.guest_read_32(t.scratch + field);
            cpu.bus.guest_write_32(at, bytes);
        }
        return Next::Done(value);
    }
    let status = cpu.bus.guest_read_32(at + HEADER_STATUS);
    let set_status = |cpu: &mut Cpu, status| cpu.bus.guest_write_32(at + HEADER_STATUS, status);
    let done = |value| value == MMSYSERR_NOERROR || value == MMSYSERR_NOTSUPPORTED;
    match message {
        ACMDM_STREAM_PREPARE if done(value) => {
            let status = (status & !STATUS_INQUEUE) | STATUS_PREPARED;
            set_status(cpu, status);
            let fields = [
                (HEADER_PREPARED, status),
                (
                    HEADER_PREPARED_SOURCE,
                    cpu.bus.guest_read_32(at + HEADER_SOURCE),
                ),
                (
                    HEADER_PREPARED_SOURCE_LENGTH,
                    cpu.bus.guest_read_32(at + HEADER_SOURCE_LENGTH),
                ),
                (
                    HEADER_PREPARED_DESTINATION,
                    cpu.bus.guest_read_32(at + HEADER_DESTINATION),
                ),
                (
                    HEADER_PREPARED_DESTINATION_LENGTH,
                    cpu.bus.guest_read_32(at + HEADER_DESTINATION_LENGTH),
                ),
            ];
            for (field, word) in fields {
                cpu.bus.guest_write_32(at + field, word);
            }
            Next::Done(MMSYSERR_NOERROR)
        }
        ACMDM_STREAM_PREPARE => {
            cpu.bus.guest_fill(at + HEADER_PREPARED, 0x54 - 0x3C, 0);
            Next::Done(value)
        }
        ACMDM_STREAM_UNPREPARE if done(value) => {
            set_status(cpu, status & !(STATUS_INQUEUE | STATUS_PREPARED));
            Next::Done(MMSYSERR_NOERROR)
        }
        ACMDM_STREAM_CONVERT if value == MMSYSERR_NOERROR => {
            set_status(cpu, (status | STATUS_DONE) & !STATUS_INQUEUE);
            Next::Done(value)
        }
        _ => Next::Done(value),
    }
}

/// The stream messages acmStreamPrepareHeader, acmStreamUnprepareHeader
/// and acmStreamConvert send.
pub(super) const PREPARE: u32 = ACMDM_STREAM_PREPARE;
pub(super) const UNPREPARE: u32 = ACMDM_STREAM_UNPREPARE;
pub(super) const CONVERT: u32 = ACMDM_STREAM_CONVERT;
