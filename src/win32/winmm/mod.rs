//! WINMM.DLL: the multimedia functions: timers, wave audio, MIDI, MCI
//! with CD audio, multimedia files, the mixer and the joystick.
//!
//! (Phase 3 of docs/native-windows.md.)
//!
//! The devices' work that has to happen at a time (a timer's callback, a
//! MIDI stream's next event, the end of a CD track) happens on the
//! multimedia thread of the process that asked for it (callback.rs),
//! which `poll` runs: there the process's memory is the one mapped, for
//! the headers to write back, and the program's callbacks are called one
//! after another. When the next such time is, `schedule` tells the bus,
//! whose clock stops then (`due`), so that the thread runs at that time
//! to the microsecond.

pub mod callback;
pub mod driver;
pub mod joy;
pub mod mci;
pub mod midi;
pub mod mixer;
pub mod mmio;
pub mod timer;
pub mod wave;

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "WINMM.DLL",
    base: 0xBFAC_0000,
    apis: APIS,
    vars: &[],
    init: None,
    vtables: &[],
};

const fn stdcall(
    name: &'static str,
    ordinal: u16,
    args: u8,
    handler: super::stubs::Handler,
) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

/// The functions. Append only (see `stubs`).
static APIS: &[Api] = &[
    stdcall("timeGetTime", 1, 0, timer::time_get_time),
    Api {
        name: "MmThread",
        ordinal: 0,
        args: 1,
        conv: Conv::Loop,
        handler: callback::mm_thread,
    },
    // Timers, MIDI, MCI, multimedia files, the mixer, the auxiliary
    // device and the joystick.
    stdcall("timeGetSystemTime", 200, 2, timer::time_get_system_time),
    stdcall("timeGetDevCaps", 201, 2, timer::time_get_dev_caps),
    stdcall("timeBeginPeriod", 202, 1, timer::time_begin_period),
    stdcall("timeEndPeriod", 203, 1, timer::time_end_period),
    stdcall("timeSetEvent", 204, 5, timer::time_set_event),
    stdcall("timeKillEvent", 205, 1, timer::time_kill_event),
    stdcall("midiOutGetNumDevs", 206, 0, midi::midi_out_get_num_devs),
    stdcall("midiOutGetDevCapsA", 207, 3, midi::midi_out_get_dev_caps),
    stdcall("midiOutOpen", 208, 5, midi::midi_out_open),
    stdcall("midiOutClose", 209, 1, midi::midi_out_close),
    stdcall("midiOutShortMsg", 210, 2, midi::midi_out_short_msg),
    stdcall("midiOutLongMsg", 211, 3, midi::midi_out_long_msg),
    stdcall(
        "midiOutPrepareHeader",
        212,
        3,
        midi::midi_out_prepare_header,
    ),
    stdcall(
        "midiOutUnprepareHeader",
        213,
        3,
        midi::midi_out_unprepare_header,
    ),
    stdcall("midiOutReset", 214, 1, midi::midi_out_reset),
    stdcall("midiOutGetVolume", 215, 2, midi::midi_out_get_volume),
    stdcall("midiOutSetVolume", 216, 2, midi::midi_out_set_volume),
    stdcall("midiOutGetID", 217, 2, midi::midi_out_get_id),
    stdcall("midiOutGetErrorTextA", 218, 3, midi::midi_get_error_text),
    stdcall("midiOutMessage", 219, 4, midi::midi_out_message),
    stdcall("midiOutCachePatches", 220, 4, midi::not_supported),
    stdcall("midiOutCacheDrumPatches", 221, 4, midi::not_supported),
    stdcall("midiInGetNumDevs", 222, 0, midi::midi_in_get_num_devs),
    stdcall("midiInGetDevCapsA", 223, 3, midi::midi_in_bad_device),
    stdcall("midiInOpen", 224, 5, midi::midi_in_bad_device),
    stdcall("midiInClose", 225, 1, midi::midi_in_bad_handle),
    stdcall("midiInPrepareHeader", 226, 3, midi::midi_in_bad_handle),
    stdcall("midiInUnprepareHeader", 227, 3, midi::midi_in_bad_handle),
    stdcall("midiInAddBuffer", 228, 3, midi::midi_in_bad_handle),
    stdcall("midiInStart", 229, 1, midi::midi_in_bad_handle),
    stdcall("midiInStop", 230, 1, midi::midi_in_bad_handle),
    stdcall("midiInReset", 231, 1, midi::midi_in_bad_handle),
    stdcall("midiInGetID", 232, 2, midi::midi_in_bad_handle),
    stdcall("midiInMessage", 233, 4, midi::midi_in_bad_handle),
    stdcall("midiInGetErrorTextA", 234, 3, midi::midi_get_error_text),
    stdcall("midiStreamOpen", 235, 6, midi::midi_stream_open),
    stdcall("midiStreamClose", 236, 1, midi::midi_stream_close),
    stdcall("midiStreamOut", 237, 3, midi::midi_stream_out),
    stdcall("midiStreamRestart", 238, 1, midi::midi_stream_restart),
    stdcall("midiStreamPause", 239, 1, midi::midi_stream_pause),
    stdcall("midiStreamStop", 240, 1, midi::midi_stream_stop),
    stdcall("midiStreamPosition", 241, 3, midi::midi_stream_position),
    stdcall("midiStreamProperty", 242, 3, midi::midi_stream_property),
    stdcall("mciSendCommandA", 243, 4, mci::mci_send_command),
    stdcall("mciSendStringA", 244, 4, mci::mci_send_string),
    stdcall("mciGetErrorStringA", 245, 3, mci::mci_get_error_string),
    stdcall("mciGetDeviceIDA", 246, 1, mci::mci_get_device_id),
    stdcall("mciSetYieldProc", 247, 3, mci::mci_set_yield_proc),
    stdcall("mciGetYieldProc", 248, 2, mci::mci_get_yield_proc),
    stdcall("mciGetCreatorTask", 249, 1, mci::mci_get_creator_task),
    stdcall("mciExecute", 250, 1, mci::mci_execute),
    stdcall("mmioOpenA", 251, 3, mmio::mmio_open),
    stdcall("mmioClose", 252, 2, mmio::mmio_close),
    stdcall("mmioRead", 253, 3, mmio::mmio_read),
    stdcall("mmioWrite", 254, 3, mmio::mmio_write),
    stdcall("mmioSeek", 255, 3, mmio::mmio_seek),
    stdcall("mmioGetInfo", 256, 3, mmio::mmio_get_info),
    stdcall("mmioSetInfo", 257, 3, mmio::mmio_set_info),
    stdcall("mmioAdvance", 258, 3, mmio::mmio_advance),
    stdcall("mmioFlush", 259, 2, mmio::mmio_flush),
    stdcall("mmioSetBuffer", 260, 4, mmio::mmio_set_buffer),
    stdcall("mmioSendMessage", 261, 4, mmio::mmio_send_message),
    stdcall("mmioDescend", 262, 4, mmio::mmio_descend),
    stdcall("mmioAscend", 263, 3, mmio::mmio_ascend),
    stdcall("mmioCreateChunk", 264, 3, mmio::mmio_create_chunk),
    stdcall("mmioStringToFOURCCA", 265, 2, mmio::mmio_string_to_fourcc),
    stdcall("mmioInstallIOProcA", 266, 3, mmio::mmio_install_io_proc),
    stdcall("mmioRenameA", 267, 4, mmio::mmio_rename),
    stdcall("mixerGetNumDevs", 268, 0, mixer::mixer_get_num_devs),
    stdcall("mixerGetDevCapsA", 269, 3, mixer::mixer_get_dev_caps),
    stdcall("mixerOpen", 270, 5, mixer::mixer_open),
    stdcall("mixerClose", 271, 1, mixer::mixer_close),
    stdcall("mixerGetID", 272, 3, mixer::mixer_get_id),
    stdcall("mixerGetLineInfoA", 273, 3, mixer::mixer_get_line_info),
    stdcall(
        "mixerGetLineControlsA",
        274,
        3,
        mixer::mixer_get_line_controls,
    ),
    stdcall(
        "mixerGetControlDetailsA",
        275,
        3,
        mixer::mixer_get_control_details,
    ),
    stdcall(
        "mixerSetControlDetails",
        276,
        3,
        mixer::mixer_set_control_details,
    ),
    stdcall("mixerMessage", 277, 4, mixer::mixer_message),
    stdcall("mixerGetDevCapsW", 278, 3, not_supported),
    stdcall("mixerGetLineInfoW", 279, 3, not_supported),
    stdcall("mixerGetLineControlsW", 280, 3, not_supported),
    stdcall("mixerGetControlDetailsW", 281, 3, not_supported),
    stdcall("auxGetNumDevs", 282, 0, mixer::aux_get_num_devs),
    stdcall("auxGetDevCapsA", 283, 3, mixer::aux_get_dev_caps),
    stdcall("auxGetVolume", 284, 2, mixer::aux_get_volume),
    stdcall("auxSetVolume", 285, 2, mixer::aux_set_volume),
    stdcall("auxOutMessage", 286, 4, mixer::aux_out_message),
    stdcall("joyGetNumDevs", 287, 0, joy::joy_get_num_devs),
    stdcall("joyGetDevCapsA", 288, 3, joy::joy_get_dev_caps),
    stdcall("joyGetPos", 289, 2, joy::joy_get_pos),
    stdcall("joyGetPosEx", 290, 2, joy::joy_get_pos_ex),
    stdcall("joyGetThreshold", 291, 2, joy::joy_get_threshold),
    stdcall("joySetThreshold", 292, 2, joy::joy_set_threshold),
    stdcall("joySetCapture", 293, 4, joy::joy_set_capture),
    stdcall("joyReleaseCapture", 294, 1, joy::joy_release_capture),
    stdcall("joyConfigChanged", 295, 1, joy::joy_config_changed),
    stdcall("mmsystemGetVersion", 296, 0, driver::mmsystem_get_version),
    stdcall("DriverCallback", 297, 7, driver::driver_callback),
    stdcall("OpenDriver", 298, 3, driver::open_driver),
    stdcall("CloseDriver", 299, 3, driver::close_driver),
    stdcall("SendDriverMessage", 300, 4, driver::send_driver_message),
    stdcall("DefDriverProc", 301, 5, driver::def_driver_proc),
    stdcall(
        "DrvGetModuleHandle",
        302,
        1,
        driver::get_driver_module_handle,
    ),
    stdcall(
        "GetDriverModuleHandle",
        303,
        1,
        driver::get_driver_module_handle,
    ),
    stdcall("mmTaskCreate", 304, 3, driver::mm_task_create),
    stdcall(
        "mciGetDeviceIDFromElementIDA",
        305,
        2,
        mci::mci_get_device_id_from_element,
    ),
    // Windows 95's Unicode MCI functions, which do nothing.
    stdcall("mciSendCommandW", 306, 4, not_supported),
    stdcall("mciSendStringW", 307, 4, not_supported),
    stdcall("mciGetErrorStringW", 308, 3, returns_false),
    stdcall("mciGetDeviceIDW", 309, 1, returns_false),
    // Wave audio and PlaySound (wave.rs).
    stdcall("waveOutGetNumDevs", 400, 0, wave::wave_out_get_num_devs),
    stdcall("waveOutGetDevCapsA", 401, 3, wave::wave_out_get_dev_caps_a),
    stdcall("waveOutGetDevCapsW", 402, 3, wave::not_supported),
    stdcall("waveOutOpen", 403, 6, wave::wave_out_open),
    stdcall("waveOutClose", 404, 1, wave::wave_out_close),
    stdcall("waveOutPrepareHeader", 405, 3, wave::wave_out_prepare_header),
    stdcall("waveOutUnprepareHeader", 406, 3, wave::wave_out_unprepare_header),
    stdcall("waveOutWrite", 407, 3, wave::wave_out_write),
    stdcall("waveOutReset", 408, 1, wave::wave_out_reset),
    stdcall("waveOutPause", 409, 1, wave::wave_out_pause),
    stdcall("waveOutRestart", 410, 1, wave::wave_out_restart),
    stdcall("waveOutBreakLoop", 411, 1, wave::wave_out_break_loop),
    stdcall("waveOutGetPosition", 412, 3, wave::wave_out_get_position),
    stdcall("waveOutGetVolume", 413, 2, wave::wave_out_get_volume),
    stdcall("waveOutSetVolume", 414, 2, wave::wave_out_set_volume),
    stdcall("waveOutGetErrorTextA", 415, 3, wave::wave_get_error_text_a),
    stdcall("waveOutGetErrorTextW", 416, 3, wave::not_supported),
    stdcall("waveOutGetID", 417, 2, wave::wave_out_get_id),
    stdcall("waveOutMessage", 418, 4, wave::wave_out_message),
    stdcall("waveOutGetPitch", 419, 2, wave::wave_out_pitch),
    stdcall("waveOutSetPitch", 420, 2, wave::wave_out_pitch),
    stdcall("waveOutGetPlaybackRate", 421, 2, wave::wave_out_pitch),
    stdcall("waveOutSetPlaybackRate", 422, 2, wave::wave_out_pitch),
    stdcall("waveInGetNumDevs", 423, 0, wave::wave_in_get_num_devs),
    stdcall("waveInGetDevCapsA", 424, 3, wave::wave_in_bad_device),
    stdcall("waveInGetDevCapsW", 425, 3, wave::wave_in_bad_device),
    stdcall("waveInOpen", 426, 6, wave::wave_in_bad_device),
    stdcall("waveInClose", 427, 1, wave::wave_in_bad_handle),
    stdcall("waveInPrepareHeader", 428, 3, wave::wave_in_bad_handle),
    stdcall("waveInUnprepareHeader", 429, 3, wave::wave_in_bad_handle),
    stdcall("waveInAddBuffer", 430, 3, wave::wave_in_bad_handle),
    stdcall("waveInStart", 431, 1, wave::wave_in_bad_handle),
    stdcall("waveInStop", 432, 1, wave::wave_in_bad_handle),
    stdcall("waveInReset", 433, 1, wave::wave_in_bad_handle),
    stdcall("waveInGetPosition", 434, 3, wave::wave_in_bad_handle),
    stdcall("waveInGetID", 435, 2, wave::wave_in_bad_handle),
    stdcall("waveInMessage", 436, 4, wave::wave_in_bad_handle),
    stdcall("waveInGetErrorTextA", 437, 3, wave::wave_get_error_text_a),
    stdcall("waveInGetErrorTextW", 438, 3, wave::not_supported),
    stdcall("PlaySoundA", 439, 3, wave::play_sound_a),
    stdcall("PlaySound", 440, 3, wave::play_sound_a),
    stdcall("sndPlaySoundA", 441, 2, wave::snd_play_sound_a),
    stdcall("PlaySoundW", 442, 3, wave::play_sound_w),
    stdcall("sndPlaySoundW", 443, 2, wave::play_sound_w),
];

/// Where the multimedia thread starts (callback.rs).
pub fn mm_thread_address() -> u32 {
    let index = APIS
        .iter()
        .position(|a| a.name == "MmThread")
        .expect("MmThread");
    super::stubs::stub_address(dll_index(), index)
}

fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("WINMM is a built-in DLL")
}

/// The errors of the multimedia functions.
pub const MMSYSERR_NOERROR: u32 = 0;
pub const MMSYSERR_BADDEVICEID: u32 = 2;
pub const MMSYSERR_INVALHANDLE: u32 = 5;
pub const MMSYSERR_NODRIVER: u32 = 6;
pub const MMSYSERR_NOMEM: u32 = 7;
pub const MMSYSERR_NOTSUPPORTED: u32 = 8;
pub const MMSYSERR_BADERRNUM: u32 = 9;
pub const MMSYSERR_INVALFLAG: u32 = 10;
pub const MMSYSERR_INVALPARAM: u32 = 11;
pub const MIDIERR_UNPREPARED: u32 = 64;
pub const MIDIERR_STILLPLAYING: u32 = 65;

pub const TRUE: u32 = 1;
pub const FALSE: u32 = 0;

/// The manufacturer and product ids of the devices' caps.
pub const MM_MICROSOFT: u16 = 1;
pub const MM_MPU401_MIDIOUT: u16 = 10;

/// WinMM's state: the system's.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// The processes' multimedia threads, and the calls queued for them
    /// (callback.rs).
    pub threads: Vec<callback::MmThread>,
    pub queued: Vec<callback::Queued>,
    /// Wave audio's devices and sounds (wave.rs).
    pub wave: wave::Wave,
    /// Whether `tick` runs, so that what it does doesn't run it again.
    ticking: bool,
    /// When the devices' next work is due, by the clock's microseconds
    /// (`schedule`).
    pub due: Option<u64>,
    /// The last handle a device or file was given (`new_handle`).
    pub last_handle: u32,
    pub timers: timer::Timers,
    pub midi: midi::Midi,
    pub mci: mci::Mci,
    pub mmio: mmio::Mmio,
    pub mixer: mixer::Mixer,
    pub joy: joy::Joy,
}

/// A process ended: what it had goes.
pub fn process_ended(cpu: &mut Cpu, pid: u32, _tids: &[u32]) {
    timer::process_ended(cpu, pid);
    midi::process_ended(cpu, pid);
    mci::process_ended(cpu, pid);
    mmio::process_ended(cpu, pid);
    mixer::process_ended(cpu, pid);
    joy::process_ended(cpu, pid);
    callback::process_ended(cpu, pid);
    wave::process_ended(cpu, pid);
    schedule(cpu);
}

/// Time passed (thread::wake): the devices' work that is due is done,
/// whichever process runs.
pub fn tick(cpu: &mut Cpu) {
    if cpu.bus.win32.winmm.ticking {
        return;
    }
    cpu.bus.win32.winmm.ticking = true;
    wave::tick(cpu);
    cpu.bus.win32.winmm.ticking = false;
}

/// The multimedia thread of the process `pid` looks for due work at
/// `now` (callback.rs): it queues the calls due, and returns when it is
/// to look again, if it is to before a call is queued.
pub fn poll(cpu: &mut Cpu, pid: u32, now: u64) -> Option<u64> {
    timer::poll(cpu, pid, now);
    midi::poll(cpu, pid, now);
    mci::poll(cpu, pid, now);
    joy::poll(cpu, pid, now);
    schedule(cpu);
    next_due(cpu, pid)
}

/// When the process `pid` next has work due.
fn next_due(cpu: &Cpu, pid: u32) -> Option<u64> {
    [
        timer::next_due(cpu, Some(pid)),
        midi::next_due(cpu, Some(pid)),
        mci::next_due(cpu, Some(pid)),
        joy::next_due(cpu, Some(pid)),
    ]
    .into_iter()
    .flatten()
    .min()
}

/// The devices' times changed: the multimedia threads with work due now
/// are woken, and the bus is told when the next work after now is due.
pub fn schedule(cpu: &mut Cpu) {
    let now = cpu.bus.clock.now_micros();
    let pids: Vec<u32> = cpu.bus.win32.winmm.threads.iter().map(|t| t.pid).collect();
    let mut due: Option<u64> = None;
    for pid in pids {
        match next_due(cpu, pid) {
            Some(t) if t <= now => callback::wake_thread(cpu, pid),
            Some(t) => due = Some(due.map_or(t, |d| d.min(t))),
            None => {}
        }
    }
    cpu.bus.win32.winmm.due = due;
    cpu.bus.reschedule_timers();
}

/// A new handle for a device or a file: small numbers, as Windows 95's
/// 16-bit multimedia handles were, none 0.
pub fn new_handle(cpu: &mut Cpu) -> u32 {
    let s = &mut cpu.bus.win32.winmm;
    s.last_handle = if (0x1000..0xFFFC).contains(&s.last_handle) {
        s.last_handle + 4
    } else {
        0x1000
    };
    s.last_handle
}

/// A name in a structure's field of `len` bytes, cut short to leave
/// room for its 0.
pub fn name_field(name: &str, len: usize) -> Vec<u8> {
    let mut field = vec![0; len];
    let bytes = name.as_bytes();
    let n = bytes.len().min(len.saturating_sub(1));
    field[..n].copy_from_slice(&bytes[..n]);
    field
}

/// Write `bytes`, a structure, to `at`, as much of it as the program's
/// `size` has room for: MMSYSERR_NOERROR, or None when the page isn't
/// there.
pub fn write_struct(cpu: &mut Cpu, at: u32, size: u32, bytes: &[u8]) -> Option<u32> {
    let n = (size as usize).min(bytes.len());
    if !cpu.bus.guest_probe(at, n, true) {
        return None;
    }
    cpu.bus.guest_write_bytes(at, &bytes[..n]);
    Some(MMSYSERR_NOERROR)
}

/// The string at `at`, at most `max` bytes of it, or None when its page
/// isn't there.
pub fn read_string(cpu: &mut Cpu, at: u32, max: usize) -> Option<String> {
    let bytes = cpu.bus.guest_read_asciiz(at, max);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(bytes.iter().map(|&b| b as char).collect())
}

/// Write `text` and its 0 to the buffer `at` of `size` bytes, cut short
/// to fit. None when the page isn't there.
pub fn write_text(cpu: &mut Cpu, at: u32, size: u32, text: &str) -> Option<()> {
    if size == 0 {
        return Some(());
    }
    let mut bytes: Vec<u8> = text.chars().map(|c| c as u32 as u8).collect();
    bytes.truncate(size as usize - 1);
    bytes.push(0);
    if !cpu.bus.guest_probe(at, bytes.len(), true) {
        return None;
    }
    cpu.bus.guest_write_bytes(at, &bytes);
    Some(())
}

/// The text of a multimedia error, as Windows 95 has it.
pub fn error_text(error: u32) -> Option<&'static str> {
    Some(match error {
        0 => "The specified command was carried out.",
        1 => "Undefined external error.",
        2 => "A device ID has been used that is out of range for your system.",
        3 => "The driver was not enabled.",
        4 => {
            "The specified device is already in use. Wait until it is free, and then try \
             again."
        }
        5 => "The specified device handle is invalid.",
        6 => "There is no driver installed on your system.",
        7 => {
            "There is not enough memory available for this task. Quit one or more \
             applications to increase available memory, and then try again."
        }
        8 => {
            "This function is not supported. Use the Capabilities function to determine \
             which functions and messages the driver supports."
        }
        9 => "An error number was specified that is not defined in the system.",
        10 => "An invalid flag was passed to a system function.",
        11 => "An invalid parameter was passed to a system function.",
        12 => {
            "The specified format is not supported or cannot be translated. Use the \
             Capabilities function to determine the supported formats."
        }
        13 => {
            "The specified operation cannot be performed while the data is being played. \
             Reset the device, or wait until the data is finished playing."
        }
        14 => {
            "The wave header was not prepared. Use the Prepare function to prepare the \
             header, and then try again."
        }
        15 => "The specified registry key was not found.",
        16 => "The registry could not be read.",
        17 => "The registry could not be written.",
        18 => "The registry key could not be deleted.",
        19 => "The specified registry value was not found.",
        20 => "The driver did not generate a valid OPEN callback.",
        64 => {
            "The MIDI header was not prepared. Use the Prepare function to prepare the \
             header, and then try again."
        }
        65 => {
            "Cannot perform this operation while media data is still playing. Reset the \
             device, or wait until the data is finished playing."
        }
        66 => {
            "The MIDI map was not found. There may be a problem with the driver, or the \
             MIDIMAP.CFG file may be corrupt or missing."
        }
        67 => {
            "The port is transmitting data to the device. Wait until the data has been \
             transmitted, and then try again."
        }
        68 => {
            "The current MIDI Mapper setup refers to a MIDI device that is not installed \
             on the system."
        }
        69 => {
            "The current MIDI setup is damaged. Copy the original MIDIMAP.CFG file to the \
             Windows SYSTEM directory, and then try again."
        }
        _ => return None,
    })
}

/// The error functions (error, text, size): the text of `error` in the
/// buffer.
pub fn error_text_to(cpu: &mut Cpu, error: u32, at: u32, size: u32) -> Option<u32> {
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if size == 0 {
        return Some(MMSYSERR_NOERROR);
    }
    let Some(text) = error_text(error) else {
        return Some(MMSYSERR_BADERRNUM);
    };
    write_text(cpu, at, size, text)?;
    Some(MMSYSERR_NOERROR)
}

/// A function the built-in devices don't do.
fn not_supported(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_NOTSUPPORTED)
}

fn returns_false(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(FALSE)
}

crate::state_fields!(State {
    threads,
    queued,
    due,
    last_handle,
    timers,
    midi,
    mci,
    mmio,
    mixer,
    joy,
    wave
} skip { ticking });
