//! MCI, the Media Control Interface: mciSendCommandA, mciSendStringA's
//! command strings (string.rs), and the devices they open: CD audio on
//! the emulated CD drive (cdaudio.rs) and the MIDI sequencer, which
//! plays Standard MIDI Files on the MPU-401 (sequencer.rs).
//!
//! A device is opened by a process, which gets the lowest device id
//! free. Commands with MCI_NOTIFY post MM_MCINOTIFY to their window when
//! they are done: a play when it ends, which a later command with
//! MCI_NOTIFY supersedes (MCI_NOTIFY_SUPERSEDED) and a stop, seek, new
//! play or close aborts (MCI_NOTIFY_ABORTED). A play with MCI_WAIT has
//! the thread wait until it ends. The ends of plays are found on the
//! process's multimedia thread (`poll`), at their emulated times.
//!
//! Wave audio's device (waveaudio) is wave audio's own: it is there when
//! `WAVE_DRIVER` is, and until then it is not installed.

pub mod cdaudio;
pub mod sequencer;
pub mod string;

use super::callback;
use super::*;
use crate::cpu::Cpu;
use crate::win32::thread::{self, Wait};

/// The commands.
pub const MCI_OPEN: u32 = 0x0803;
pub const MCI_CLOSE: u32 = 0x0804;
pub const MCI_PLAY: u32 = 0x0806;
pub const MCI_SEEK: u32 = 0x0807;
pub const MCI_STOP: u32 = 0x0808;
pub const MCI_PAUSE: u32 = 0x0809;
pub const MCI_INFO: u32 = 0x080A;
pub const MCI_GETDEVCAPS: u32 = 0x080B;
pub const MCI_SPIN: u32 = 0x080C;
pub const MCI_SET: u32 = 0x080D;
pub const MCI_STEP: u32 = 0x080E;
pub const MCI_RECORD: u32 = 0x080F;
pub const MCI_SYSINFO: u32 = 0x0810;
pub const MCI_BREAK: u32 = 0x0811;
pub const MCI_SAVE: u32 = 0x0813;
pub const MCI_STATUS: u32 = 0x0814;
pub const MCI_CUE: u32 = 0x0830;
pub const MCI_LOAD: u32 = 0x0850;
pub const MCI_RESUME: u32 = 0x0855;
pub const MCI_DELETE: u32 = 0x0856;

/// The flags all commands have.
pub const MCI_NOTIFY: u32 = 0x0001;
pub const MCI_WAIT: u32 = 0x0002;
pub const MCI_FROM: u32 = 0x0004;
pub const MCI_TO: u32 = 0x0008;
pub const MCI_TRACK: u32 = 0x0010;
pub const MCI_TEST: u32 = 0x0020;

/// MCI_OPEN's flags.
pub const MCI_OPEN_SHAREABLE: u32 = 0x0100;
pub const MCI_OPEN_ELEMENT: u32 = 0x0200;
pub const MCI_OPEN_ALIAS: u32 = 0x0400;
pub const MCI_OPEN_ELEMENT_ID: u32 = 0x0800;
pub const MCI_OPEN_TYPE_ID: u32 = 0x1000;
pub const MCI_OPEN_TYPE: u32 = 0x2000;

/// MCI_SEEK's.
pub const MCI_SEEK_TO_START: u32 = 0x0100;
pub const MCI_SEEK_TO_END: u32 = 0x0200;

/// MCI_STATUS's, and its items.
pub const MCI_STATUS_ITEM: u32 = 0x0100;
pub const MCI_STATUS_START: u32 = 0x0200;
pub const MCI_STATUS_LENGTH: u32 = 1;
pub const MCI_STATUS_POSITION: u32 = 2;
pub const MCI_STATUS_NUMBER_OF_TRACKS: u32 = 3;
pub const MCI_STATUS_MODE: u32 = 4;
pub const MCI_STATUS_MEDIA_PRESENT: u32 = 5;
pub const MCI_STATUS_TIME_FORMAT: u32 = 6;
pub const MCI_STATUS_READY: u32 = 7;
pub const MCI_STATUS_CURRENT_TRACK: u32 = 8;

/// MCI_INFO's.
pub const MCI_INFO_PRODUCT: u32 = 0x0100;
pub const MCI_INFO_FILE: u32 = 0x0200;
pub const MCI_INFO_MEDIA_UPC: u32 = 0x0400;
pub const MCI_INFO_MEDIA_IDENTITY: u32 = 0x0800;

/// MCI_GETDEVCAPS's, and its items.
pub const MCI_GETDEVCAPS_ITEM: u32 = 0x0100;
pub const MCI_GETDEVCAPS_CAN_RECORD: u32 = 1;
pub const MCI_GETDEVCAPS_HAS_AUDIO: u32 = 2;
pub const MCI_GETDEVCAPS_HAS_VIDEO: u32 = 3;
pub const MCI_GETDEVCAPS_DEVICE_TYPE: u32 = 4;
pub const MCI_GETDEVCAPS_USES_FILES: u32 = 5;
pub const MCI_GETDEVCAPS_COMPOUND_DEVICE: u32 = 6;
pub const MCI_GETDEVCAPS_CAN_EJECT: u32 = 7;
pub const MCI_GETDEVCAPS_CAN_PLAY: u32 = 8;
pub const MCI_GETDEVCAPS_CAN_SAVE: u32 = 9;

/// MCI_SYSINFO's.
pub const MCI_SYSINFO_QUANTITY: u32 = 0x0100;
pub const MCI_SYSINFO_OPEN: u32 = 0x0200;
pub const MCI_SYSINFO_NAME: u32 = 0x0400;
pub const MCI_SYSINFO_INSTALLNAME: u32 = 0x0800;

/// MCI_SET's.
pub const MCI_SET_DOOR_OPEN: u32 = 0x0100;
pub const MCI_SET_DOOR_CLOSED: u32 = 0x0200;
pub const MCI_SET_TIME_FORMAT: u32 = 0x0400;
pub const MCI_SET_AUDIO: u32 = 0x0800;
pub const MCI_SET_VIDEO: u32 = 0x1000;
pub const MCI_SET_ON: u32 = 0x2000;
pub const MCI_SET_OFF: u32 = 0x4000;

/// The time formats.
pub const MCI_FORMAT_MILLISECONDS: u32 = 0;
pub const MCI_FORMAT_MSF: u32 = 2;
pub const MCI_FORMAT_TMSF: u32 = 10;

/// The modes.
pub const MCI_MODE_NOT_READY: u32 = 524;
pub const MCI_MODE_STOP: u32 = 525;
pub const MCI_MODE_PLAY: u32 = 526;
pub const MCI_MODE_PAUSE: u32 = 529;
pub const MCI_MODE_OPEN: u32 = 530;

/// The device types.
pub const MCI_DEVTYPE_CD_AUDIO: u32 = 516;
pub const MCI_DEVTYPE_WAVEFORM_AUDIO: u32 = 522;
pub const MCI_DEVTYPE_SEQUENCER: u32 = 523;

/// The device id of all devices.
pub const MCI_ALL_DEVICE_ID: u32 = 0xFFFF_FFFF;

/// The notifications' message, and its kinds.
const MM_MCINOTIFY: u32 = 0x03B9;
pub const MCI_NOTIFY_SUCCESSFUL: u32 = 1;
pub const MCI_NOTIFY_SUPERSEDED: u32 = 2;
pub const MCI_NOTIFY_ABORTED: u32 = 4;

/// The errors.
pub const MCIERR_INVALID_DEVICE_ID: u32 = 257;
pub const MCIERR_UNRECOGNIZED_KEYWORD: u32 = 259;
pub const MCIERR_UNRECOGNIZED_COMMAND: u32 = 261;
pub const MCIERR_INVALID_DEVICE_NAME: u32 = 263;
pub const MCIERR_OUT_OF_MEMORY: u32 = 264;
pub const MCIERR_DEVICE_OPEN: u32 = 265;
pub const MCIERR_CANNOT_LOAD_DRIVER: u32 = 266;
pub const MCIERR_MISSING_COMMAND_STRING: u32 = 267;
pub const MCIERR_PARAM_OVERFLOW: u32 = 268;
pub const MCIERR_MISSING_STRING_ARGUMENT: u32 = 269;
pub const MCIERR_BAD_INTEGER: u32 = 270;
pub const MCIERR_MISSING_PARAMETER: u32 = 273;
pub const MCIERR_UNSUPPORTED_FUNCTION: u32 = 274;
pub const MCIERR_FILE_NOT_FOUND: u32 = 275;
pub const MCIERR_DEVICE_NOT_READY: u32 = 276;
pub const MCIERR_CANNOT_USE_ALL: u32 = 279;
pub const MCIERR_EXTENSION_NOT_FOUND: u32 = 281;
pub const MCIERR_OUTOFRANGE: u32 = 282;
pub const MCIERR_FLAGS_NOT_COMPATIBLE: u32 = 284;
pub const MCIERR_DUPLICATE_ALIAS: u32 = 289;
pub const MCIERR_BAD_CONSTANT: u32 = 290;
pub const MCIERR_BAD_TIME_FORMAT: u32 = 293;
pub const MCIERR_NO_CLOSING_QUOTE: u32 = 294;
pub const MCIERR_DUPLICATE_FLAGS: u32 = 295;
pub const MCIERR_INVALID_FILE: u32 = 296;
pub const MCIERR_NULL_PARAMETER_BLOCK: u32 = 297;
pub const MCIERR_NEW_REQUIRES_ALIAS: u32 = 299;
pub const MCIERR_NOTIFY_ON_AUTO_OPEN: u32 = 300;
pub const MCIERR_EXTRA_CHARACTERS: u32 = 305;
pub const MCIERR_NO_IDENTITY: u32 = 350;

/// How a command's dwReturn reads as a string (mciSendStringA).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ret {
    /// Nothing is returned.
    #[default]
    None,
    /// A number.
    Int,
    /// TRUE or FALSE: "true", "false".
    Bool,
    /// One of the MCI constants (a mode, a device type).
    Constant,
    /// A time format.
    TimeFormat,
    /// Bytes as "mm:ss:ff" (MSF) and "tt:mm:ss:ff" (TMSF).
    Colon3,
    Colon4,
    /// `text`.
    Text,
}

/// A command's parameters, from its MCI_*_PARMS or its command string,
/// and what it returns.
#[derive(Clone, Debug, Default)]
pub struct Parms {
    pub callback: u32,
    pub from: u32,
    pub to: u32,
    pub item: u32,
    pub track: u32,
    pub number: u32,
    pub dev_type: u32,
    pub time_format: u32,
    pub audio: u32,
    pub tempo: u32,
    pub port: u32,
    /// MCI_OPEN's device type (or its number, with MCI_OPEN_TYPE_ID),
    /// element and alias.
    pub open_type: String,
    pub open_type_id: u32,
    pub element: String,
    pub alias: String,
    /// What it returns: a number, how it reads, or text.
    pub ret: u32,
    pub ret_kind: Ret,
    pub text: String,
}

/// A driver of a kind of device: its commands on the open device `i`.
/// Wave audio's (waveaudio) is one, which `open` opens and `close`
/// closes too.
pub type Driver = fn(&mut Cpu, usize, u32, u32, &mut Parms) -> u32;

/// The waveaudio driver, which wave audio's code has: none yet, so that
/// waveaudio isn't installed.
pub static WAVE_DRIVER: Option<Driver> = None;

/// An open device.
#[derive(Clone, Debug, Default)]
pub struct Device {
    pub id: u32,
    pub pid: u32,
    /// Its type (MCI_DEVTYPE_*).
    pub kind: u32,
    /// The name commands find it by: its alias, element or type.
    pub name: String,
    pub element: String,
    /// Whether a command string opened it for itself: it closes when the
    /// command is done, or its play ends.
    pub auto: bool,
    pub time_format: u32,
    /// The window of the notification a play has pending (0 for none
    /// that posts), if it has one.
    pub pending: Option<u32>,
    /// mciSetYieldProc's procedure and data, which nothing calls: no
    /// command waits in a loop.
    pub yield_proc: u32,
    pub yield_data: u32,
    pub cd: cdaudio::Cd,
    pub seq: sequencer::Seq,
    /// For the waveaudio driver's own state.
    pub wave: u32,
}

/// MCI's state.
#[derive(Clone, Debug, Default)]
pub struct Mci {
    pub devices: Vec<Device>,
}

fn mci(cpu: &mut Cpu) -> &mut Mci {
    &mut cpu.bus.win32.winmm.mci
}

/// The device `id` of the running process.
fn find(cpu: &Cpu, id: u32) -> Option<usize> {
    let pid = crate::win32::user32::current_pid(cpu);
    cpu.bus
        .win32
        .winmm
        .mci
        .devices
        .iter()
        .position(|d| d.id == id && d.pid == pid)
}

/// The device of the running process that `name` names.
pub fn find_name(cpu: &Cpu, name: &str) -> Option<usize> {
    let pid = crate::win32::user32::current_pid(cpu);
    cpu.bus
        .win32
        .winmm
        .mci
        .devices
        .iter()
        .position(|d| d.pid == pid && d.name.eq_ignore_ascii_case(name))
}

pub fn device(cpu: &mut Cpu, i: usize) -> &mut Device {
    &mut cpu.bus.win32.winmm.mci.devices[i]
}

/// Whether `id` is all devices: the 32-bit id or Windows 95's 16-bit
/// one.
fn is_all(id: u32) -> bool {
    id == MCI_ALL_DEVICE_ID || id == 0xFFFF
}

/// The installed devices' types, and their names, as SYSTEM.INI's [mci]
/// lists them.
fn installed() -> Vec<(u32, &'static str)> {
    let mut list = vec![
        (MCI_DEVTYPE_CD_AUDIO, "cdaudio"),
        (MCI_DEVTYPE_SEQUENCER, "sequencer"),
    ];
    if WAVE_DRIVER.is_some() {
        list.push((MCI_DEVTYPE_WAVEFORM_AUDIO, "waveaudio"));
    }
    list
}

/// The type a device type's name, or a file's extension, names.
pub fn type_of_name(name: &str) -> Option<u32> {
    installed()
        .into_iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(name))
        .map(|(t, _)| t)
}

fn type_of_file(element: &str) -> Option<u32> {
    let ext = element.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "mid" | "rmi" => Some(MCI_DEVTYPE_SEQUENCER),
        "wav" if WAVE_DRIVER.is_some() => Some(MCI_DEVTYPE_WAVEFORM_AUDIO),
        "cda" => Some(MCI_DEVTYPE_CD_AUDIO),
        _ => None,
    }
}

/// The name of a device type.
pub fn type_name(kind: u32) -> &'static str {
    match kind {
        MCI_DEVTYPE_CD_AUDIO => "cdaudio",
        MCI_DEVTYPE_SEQUENCER => "sequencer",
        MCI_DEVTYPE_WAVEFORM_AUDIO => "waveaudio",
        _ => "other",
    }
}

/// Post MM_MCINOTIFY to `hwnd`, if there is a window, for the device
/// `id`.
fn post(cpu: &mut Cpu, hwnd: u32, status: u32, id: u32) {
    if hwnd != 0 {
        crate::win32::user32::message::post_message(cpu, &[hwnd, MM_MCINOTIFY, status, id]);
    }
}

/// A command with MCI_NOTIFY came: the notification a play has pending
/// is superseded.
fn supersede(cpu: &mut Cpu, i: usize) {
    let d = device(cpu, i);
    if let Some(hwnd) = d.pending.take() {
        let id = d.id;
        post(cpu, hwnd, MCI_NOTIFY_SUPERSEDED, id);
    }
}

/// A command ended the play: its notification is aborted.
pub fn abort(cpu: &mut Cpu, i: usize) {
    let d = device(cpu, i);
    if let Some(hwnd) = d.pending.take() {
        let id = d.id;
        post(cpu, hwnd, MCI_NOTIFY_ABORTED, id);
    }
}

/// The open device `i`'s play ended (`poll`): its notification is
/// successful, and a device a command string opened closes.
fn play_ended(cpu: &mut Cpu, i: usize) {
    let d = device(cpu, i);
    let id = d.id;
    if let Some(hwnd) = d.pending.take() {
        post(cpu, hwnd, MCI_NOTIFY_SUCCESSFUL, id);
    }
    if device(cpu, i).auto {
        close_device(cpu, i);
    }
}

/// When the open device `i`'s play ends, if it plays.
fn busy_until(cpu: &Cpu, i: usize) -> Option<u64> {
    let d = &cpu.bus.win32.winmm.mci.devices[i];
    match d.kind {
        MCI_DEVTYPE_CD_AUDIO => d.cd.end_at,
        MCI_DEVTYPE_SEQUENCER => sequencer::end_at(d),
        _ => None,
    }
}

/// A command for the device `i` (not MCI_OPEN): the driver's work, and
/// the notification. What it returns is in `p`; an error, or 0.
pub fn device_command(cpu: &mut Cpu, i: usize, msg: u32, flags: u32, p: &mut Parms) -> u32 {
    if flags & MCI_NOTIFY != 0 {
        supersede(cpu, i);
    }
    if matches!(msg, MCI_PLAY | MCI_STOP | MCI_SEEK | MCI_CLOSE) && flags & MCI_TEST == 0 {
        abort(cpu, i);
    }
    let id = device(cpu, i).id;
    if msg == MCI_CLOSE {
        if flags & MCI_NOTIFY != 0 {
            post(cpu, p.callback, MCI_NOTIFY_SUCCESSFUL, id);
        }
        close_device(cpu, i);
        return 0;
    }
    let kind = device(cpu, i).kind;
    let error = match kind {
        MCI_DEVTYPE_CD_AUDIO => cdaudio::command(cpu, i, msg, flags, p),
        MCI_DEVTYPE_SEQUENCER => sequencer::command(cpu, i, msg, flags, p),
        _ => match WAVE_DRIVER {
            Some(driver) => driver(cpu, i, msg, flags, p),
            None => MCIERR_UNSUPPORTED_FUNCTION,
        },
    };
    if error != 0 {
        return error;
    }
    let playing = msg == MCI_PLAY && busy_until(cpu, i).is_some();
    if flags & MCI_NOTIFY != 0 {
        if playing {
            device(cpu, i).pending = Some(p.callback);
        } else {
            post(cpu, p.callback, MCI_NOTIFY_SUCCESSFUL, id);
        }
    }
    if playing {
        callback::ensure_thread(cpu);
        super::schedule(cpu);
    }
    0
}

/// After a command: a play with MCI_WAIT has the thread wait until it
/// ends. True when the thread waits (the handler returns None).
pub fn wait_for_play(cpu: &mut Cpu, id: u32, msg: u32, flags: u32) -> bool {
    if msg != MCI_PLAY || flags & MCI_WAIT == 0 {
        return false;
    }
    let pid = crate::win32::user32::current_pid(cpu);
    let Some(i) = cpu
        .bus
        .win32
        .winmm
        .mci
        .devices
        .iter()
        .position(|d| d.id == id && d.pid == pid)
    else {
        return false;
    };
    let Some(until) = busy_until(cpu, i) else {
        return false;
    };
    cpu.set_eax(0);
    thread::block(
        cpu,
        Wait {
            deadline: Some(until),
            ..Wait::default()
        },
    );
    true
}

/// Close the device `i`: what it plays stops.
pub fn close_device(cpu: &mut Cpu, i: usize) {
    abort(cpu, i);
    match device(cpu, i).kind {
        MCI_DEVTYPE_CD_AUDIO => cdaudio::close(cpu, i),
        MCI_DEVTYPE_SEQUENCER => sequencer::close(cpu, i),
        _ => {
            if let Some(driver) = WAVE_DRIVER {
                driver(cpu, i, MCI_CLOSE, 0, &mut Parms::default());
            }
        }
    }
    mci(cpu).devices.remove(i);
    super::schedule(cpu);
}

/// MCI_OPEN: open a device of the running process. `auto` for one a
/// command string opens for itself. Its id is in `p.ret`.
pub fn open(cpu: &mut Cpu, flags: u32, p: &mut Parms, auto: bool) -> u32 {
    if flags & MCI_OPEN_ELEMENT_ID != 0 {
        return MCIERR_UNSUPPORTED_FUNCTION;
    }
    let element = if flags & MCI_OPEN_ELEMENT != 0 {
        p.element.clone()
    } else {
        String::new()
    };
    // "type!element" names both.
    let (type_name_given, element) = match element.split_once('!') {
        Some((t, e)) if flags & MCI_OPEN_TYPE == 0 => (Some(t.to_string()), e.to_string()),
        _ => (None, element),
    };
    let kind = if flags & MCI_OPEN_TYPE != 0 {
        if flags & MCI_OPEN_TYPE_ID != 0 {
            let id = p.open_type_id & 0xFFFF;
            match installed().into_iter().find(|&(t, _)| t == id) {
                Some((t, _)) => t,
                None => return MCIERR_INVALID_DEVICE_NAME,
            }
        } else {
            match type_of_name(&p.open_type) {
                Some(t) => t,
                None => return MCIERR_INVALID_DEVICE_NAME,
            }
        }
    } else if let Some(t) = type_name_given {
        match type_of_name(&t) {
            Some(t) => t,
            None => return MCIERR_INVALID_DEVICE_NAME,
        }
    } else if !element.is_empty() {
        // A device's name as the element, or a file with an extension
        // that names its device.
        match type_of_name(&element).or_else(|| type_of_file(&element)) {
            Some(t) => {
                if type_of_name(&element).is_some() {
                    return open_kind(cpu, flags, p, t, String::new(), auto);
                }
                t
            }
            None if element.contains('.') => return MCIERR_EXTENSION_NOT_FOUND,
            None => return MCIERR_INVALID_DEVICE_NAME,
        }
    } else {
        return MCIERR_MISSING_PARAMETER;
    };
    open_kind(cpu, flags, p, kind, element, auto)
}

fn open_kind(
    cpu: &mut Cpu,
    flags: u32,
    p: &mut Parms,
    kind: u32,
    element: String,
    auto: bool,
) -> u32 {
    if kind == MCI_DEVTYPE_WAVEFORM_AUDIO && WAVE_DRIVER.is_none() {
        return MCIERR_INVALID_DEVICE_NAME;
    }
    let name = if flags & MCI_OPEN_ALIAS != 0 {
        p.alias.clone()
    } else if !element.is_empty() {
        element.clone()
    } else {
        type_name(kind).to_string()
    };
    if find_name(cpu, &name).is_some() {
        return if flags & MCI_OPEN_ALIAS != 0 {
            MCIERR_DUPLICATE_ALIAS
        } else {
            MCIERR_DEVICE_OPEN
        };
    }
    let pid = crate::win32::user32::current_pid(cpu);
    let mut id = 1;
    while cpu.bus.win32.winmm.mci.devices.iter().any(|d| d.id == id) {
        id += 1;
    }
    let time_format = match kind {
        MCI_DEVTYPE_CD_AUDIO => MCI_FORMAT_MSF,
        _ => MCI_FORMAT_MILLISECONDS,
    };
    mci(cpu).devices.push(Device {
        id,
        pid,
        kind,
        name,
        element,
        auto,
        time_format,
        ..Device::default()
    });
    let i = mci(cpu).devices.len() - 1;
    let error = match kind {
        MCI_DEVTYPE_CD_AUDIO => cdaudio::open(cpu, i),
        MCI_DEVTYPE_SEQUENCER => sequencer::open(cpu, i),
        _ => match WAVE_DRIVER {
            Some(driver) => driver(cpu, i, MCI_OPEN, flags, p),
            None => MCIERR_INVALID_DEVICE_NAME,
        },
    };
    if error != 0 {
        mci(cpu).devices.remove(i);
        return error;
    }
    if callback::ensure_thread(cpu).is_none() {
        mci(cpu).devices.remove(i);
        return MCIERR_OUT_OF_MEMORY;
    }
    p.ret = id;
    p.ret_kind = Ret::Int;
    if flags & MCI_NOTIFY != 0 {
        post(cpu, p.callback, MCI_NOTIFY_SUCCESSFUL, id);
    }
    0
}

/// MCI_SYSINFO, of all devices (`kind` None) or those of a type: how
/// many there are, installed or open, or the name of one.
pub fn sysinfo(cpu: &mut Cpu, kind: Option<u32>, flags: u32, p: &mut Parms) -> u32 {
    let pid = crate::win32::user32::current_pid(cpu);
    let open: Vec<String> = cpu
        .bus
        .win32
        .winmm
        .mci
        .devices
        .iter()
        .filter(|d| d.pid == pid && kind.is_none_or(|k| d.kind == k))
        .map(|d| d.name.clone())
        .collect();
    let types: Vec<String> = installed()
        .into_iter()
        .filter(|&(t, _)| kind.is_none_or(|k| k == t))
        .map(|(_, n)| n.to_string())
        .collect();
    let list = if flags & MCI_SYSINFO_OPEN != 0 {
        open
    } else {
        types
    };
    if flags & MCI_SYSINFO_QUANTITY != 0 {
        p.ret = list.len() as u32;
        p.ret_kind = Ret::Int;
    } else if flags & (MCI_SYSINFO_NAME | MCI_SYSINFO_INSTALLNAME) != 0 {
        let n = p.number as usize;
        let Some(name) = n.checked_sub(1).and_then(|k| list.get(k)) else {
            return MCIERR_OUTOFRANGE;
        };
        p.text = name.clone();
        p.ret_kind = Ret::Text;
    } else {
        return MCIERR_MISSING_PARAMETER;
    }
    if flags & MCI_NOTIFY != 0 {
        post(cpu, p.callback, MCI_NOTIFY_SUCCESSFUL, 0);
    }
    0
}

/// mciSendCommandA(id, message, flags, parms).
pub fn mci_send_command(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, msg, flags, at] = args else {
        unreachable!()
    };
    // The size of each command's parameters, and whether it can go
    // without them.
    let size: u32 = match msg {
        MCI_OPEN | MCI_SYSINFO => 20,
        MCI_STATUS => 16,
        MCI_PLAY | MCI_GETDEVCAPS | MCI_INFO => 12,
        MCI_SET => 20,
        MCI_SEEK => 8,
        _ => 4,
    };
    if at == 0
        && !matches!(
            msg,
            MCI_CLOSE | MCI_STOP | MCI_PAUSE | MCI_RESUME | MCI_BREAK
        )
    {
        return Some(MCIERR_NULL_PARAMETER_BLOCK);
    }
    if at != 0 && !cpu.bus.guest_probe(at, size as usize, true) {
        return None;
    }
    let r = |cpu: &mut Cpu, off: u32| {
        if at == 0 {
            0
        } else {
            cpu.bus.guest_read_32(at + off)
        }
    };
    let mut p = Parms {
        callback: r(cpu, 0),
        ..Parms::default()
    };
    match msg {
        MCI_OPEN => {
            let (ty, el, al) = (r(cpu, 8), r(cpu, 12), r(cpu, 16));
            if flags & MCI_OPEN_TYPE != 0 {
                if flags & MCI_OPEN_TYPE_ID != 0 {
                    p.open_type_id = ty;
                } else {
                    p.open_type = read_string(cpu, ty, 128)?;
                }
            }
            if flags & MCI_OPEN_ELEMENT != 0 && flags & MCI_OPEN_ELEMENT_ID == 0 {
                p.element = read_string(cpu, el, 260)?;
            }
            if flags & MCI_OPEN_ALIAS != 0 {
                p.alias = read_string(cpu, al, 128)?;
            }
            let error = open(cpu, flags, &mut p, false);
            if error == 0 {
                cpu.bus.guest_write_32(at + 4, p.ret);
            }
            return Some(error);
        }
        MCI_PLAY => {
            p.from = r(cpu, 4);
            p.to = r(cpu, 8);
        }
        MCI_SEEK => p.to = r(cpu, 4),
        MCI_STATUS => {
            p.item = r(cpu, 8);
            p.track = r(cpu, 12);
        }
        MCI_GETDEVCAPS => p.item = r(cpu, 8),
        MCI_SYSINFO => {
            p.number = r(cpu, 12);
            p.dev_type = r(cpu, 16);
        }
        MCI_SET => {
            p.time_format = r(cpu, 4);
            p.audio = r(cpu, 8);
            p.tempo = r(cpu, 12);
            p.port = r(cpu, 16);
        }
        _ => {}
    }
    let error = if msg == MCI_SYSINFO {
        let kind = if is_all(id) {
            None
        } else if let Some(i) = find(cpu, id) {
            Some(device(cpu, i).kind)
        } else {
            Some(p.dev_type)
        };
        sysinfo(cpu, kind, flags, &mut p)
    } else if is_all(id) {
        if msg != MCI_CLOSE {
            return Some(MCIERR_CANNOT_USE_ALL);
        }
        close_all(cpu, flags, &mut p);
        0
    } else {
        let Some(i) = find(cpu, id) else {
            return Some(MCIERR_INVALID_DEVICE_ID);
        };
        device_command(cpu, i, msg, flags, &mut p)
    };
    if error != 0 {
        return Some(error);
    }
    // What the command returns.
    match msg {
        MCI_STATUS | MCI_GETDEVCAPS => cpu.bus.guest_write_32(at + 4, p.ret),
        MCI_INFO | MCI_SYSINFO => {
            let (to, room) = (r(cpu, 4), r(cpu, 8));
            if p.ret_kind == Ret::Int {
                // A quantity is a DWORD.
                if room < 4 {
                    return Some(MCIERR_PARAM_OVERFLOW);
                }
                if !cpu.bus.guest_probe(to, 4, true) {
                    return None;
                }
                cpu.bus.guest_write_32(to, p.ret);
            } else {
                if p.text.len() as u32 >= room {
                    return Some(MCIERR_PARAM_OVERFLOW);
                }
                write_text(cpu, to, room, &p.text)?;
            }
        }
        _ => {}
    }
    if wait_for_play(cpu, id, msg, flags) {
        return None;
    }
    Some(0)
}

/// Close all the running process's devices, each with its notification.
pub fn close_all(cpu: &mut Cpu, flags: u32, p: &mut Parms) {
    while let Some(i) = {
        let pid = crate::win32::user32::current_pid(cpu);
        cpu.bus
            .win32
            .winmm
            .mci
            .devices
            .iter()
            .position(|d| d.pid == pid)
    } {
        device_command(cpu, i, MCI_CLOSE, flags, p);
    }
}

/// mciGetDeviceIDA(name): the id of the device of that name, or of all.
pub fn mci_get_device_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = read_string(cpu, args[0], 260)?;
    if name.eq_ignore_ascii_case("all") {
        return Some(0xFFFF);
    }
    Some(find_name(cpu, &name).map_or(0, |i| device(cpu, i).id))
}

/// mciGetDeviceIDFromElementIDA(element, type): no device opens by an
/// element id.
pub fn mci_get_device_id_from_element(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// mciSetYieldProc(id, proc, data).
pub fn mci_set_yield_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, proc, data] = args else {
        unreachable!()
    };
    let Some(i) = find(cpu, id) else {
        return Some(FALSE);
    };
    let d = device(cpu, i);
    d.yield_proc = proc;
    d.yield_data = data;
    Some(TRUE)
}

/// mciGetYieldProc(id, data).
pub fn mci_get_yield_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, at] = args else { unreachable!() };
    let Some(i) = find(cpu, id) else {
        return Some(0);
    };
    let (proc, data) = {
        let d = device(cpu, i);
        (d.yield_proc, d.yield_data)
    };
    if at != 0 {
        if !cpu.bus.guest_probe(at, 4, true) {
            return None;
        }
        cpu.bus.guest_write_32(at, data);
    }
    Some(proc)
}

/// mciGetCreatorTask(id): the process that opened it.
pub fn mci_get_creator_task(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(find(cpu, args[0]).map_or(0, |i| device(cpu, i).pid))
}

/// The open devices of the process `pid` look at the time `now`: plays
/// that ended end.
pub fn poll(cpu: &mut Cpu, pid: u32, now: u64) {
    let mut i = 0;
    while i < mci(cpu).devices.len() {
        let d = &mci(cpu).devices[i];
        if d.pid != pid {
            i += 1;
            continue;
        }
        let ended = match d.kind {
            MCI_DEVTYPE_CD_AUDIO => cdaudio::poll(cpu, i, now),
            MCI_DEVTYPE_SEQUENCER => sequencer::poll(cpu, i, now),
            _ => false,
        };
        let before = mci(cpu).devices.len();
        if ended {
            play_ended(cpu, i);
        }
        if mci(cpu).devices.len() == before {
            i += 1;
        }
    }
}

/// When the next of the devices' work is due.
pub fn next_due(cpu: &Cpu, pid: Option<u32>) -> Option<u64> {
    cpu.bus
        .win32
        .winmm
        .mci
        .devices
        .iter()
        .filter(|d| pid.is_none_or(|p| d.pid == p))
        .filter_map(|d| match d.kind {
            MCI_DEVTYPE_CD_AUDIO => d.cd.end_at,
            MCI_DEVTYPE_SEQUENCER => sequencer::next_due(d),
            _ => None,
        })
        .min()
}

/// A process ended: its devices close.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    while let Some(i) = mci(cpu).devices.iter().position(|d| d.pid == pid) {
        device(cpu, i).pending = None;
        close_device(cpu, i);
    }
}

pub use string::{mci_execute, mci_get_error_string, mci_send_string};

crate::state_fields!(Device {
    id,
    pid,
    kind,
    name,
    element,
    auto,
    time_format,
    pending,
    yield_proc,
    yield_data,
    cd,
    seq,
    wave
});
crate::state_fields!(Mci { devices });
