//! WINMM.DLL: the multimedia functions: timers, wave audio, MIDI, MCI
//! with CD audio, multimedia files and the joystick.
//!
//! (Phase 3 of docs/native-windows.md.)

pub mod callback;
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
    stdcall("timeGetTime", 1, 0, time_get_time),
    Api {
        name: "MmThread",
        ordinal: 0,
        args: 1,
        conv: Conv::Loop,
        handler: callback::mm_thread,
    },
    // Wave audio and PlaySound (wave.rs).
    stdcall("waveOutGetNumDevs", 300, 0, wave::wave_out_get_num_devs),
    stdcall("waveOutGetDevCapsA", 301, 3, wave::wave_out_get_dev_caps_a),
    stdcall("waveOutGetDevCapsW", 302, 3, wave::not_supported),
    stdcall("waveOutOpen", 303, 6, wave::wave_out_open),
    stdcall("waveOutClose", 304, 1, wave::wave_out_close),
    stdcall("waveOutPrepareHeader", 305, 3, wave::wave_out_prepare_header),
    stdcall("waveOutUnprepareHeader", 306, 3, wave::wave_out_unprepare_header),
    stdcall("waveOutWrite", 307, 3, wave::wave_out_write),
    stdcall("waveOutReset", 308, 1, wave::wave_out_reset),
    stdcall("waveOutPause", 309, 1, wave::wave_out_pause),
    stdcall("waveOutRestart", 310, 1, wave::wave_out_restart),
    stdcall("waveOutBreakLoop", 311, 1, wave::wave_out_break_loop),
    stdcall("waveOutGetPosition", 312, 3, wave::wave_out_get_position),
    stdcall("waveOutGetVolume", 313, 2, wave::wave_out_get_volume),
    stdcall("waveOutSetVolume", 314, 2, wave::wave_out_set_volume),
    stdcall("waveOutGetErrorTextA", 315, 3, wave::wave_get_error_text_a),
    stdcall("waveOutGetErrorTextW", 316, 3, wave::not_supported),
    stdcall("waveOutGetID", 317, 2, wave::wave_out_get_id),
    stdcall("waveOutMessage", 318, 4, wave::wave_out_message),
    stdcall("waveOutGetPitch", 319, 2, wave::wave_out_pitch),
    stdcall("waveOutSetPitch", 320, 2, wave::wave_out_pitch),
    stdcall("waveOutGetPlaybackRate", 321, 2, wave::wave_out_pitch),
    stdcall("waveOutSetPlaybackRate", 322, 2, wave::wave_out_pitch),
    stdcall("waveInGetNumDevs", 323, 0, wave::wave_in_get_num_devs),
    stdcall("waveInGetDevCapsA", 324, 3, wave::wave_in_bad_device),
    stdcall("waveInGetDevCapsW", 325, 3, wave::wave_in_bad_device),
    stdcall("waveInOpen", 326, 6, wave::wave_in_bad_device),
    stdcall("waveInClose", 327, 1, wave::wave_in_bad_handle),
    stdcall("waveInPrepareHeader", 328, 3, wave::wave_in_bad_handle),
    stdcall("waveInUnprepareHeader", 329, 3, wave::wave_in_bad_handle),
    stdcall("waveInAddBuffer", 330, 3, wave::wave_in_bad_handle),
    stdcall("waveInStart", 331, 1, wave::wave_in_bad_handle),
    stdcall("waveInStop", 332, 1, wave::wave_in_bad_handle),
    stdcall("waveInReset", 333, 1, wave::wave_in_bad_handle),
    stdcall("waveInGetPosition", 334, 3, wave::wave_in_bad_handle),
    stdcall("waveInGetID", 335, 2, wave::wave_in_bad_handle),
    stdcall("waveInMessage", 336, 4, wave::wave_in_bad_handle),
    stdcall("waveInGetErrorTextA", 337, 3, wave::wave_get_error_text_a),
    stdcall("waveInGetErrorTextW", 338, 3, wave::not_supported),
    stdcall("PlaySoundA", 339, 3, wave::play_sound_a),
    stdcall("PlaySound", 340, 3, wave::play_sound_a),
    stdcall("sndPlaySoundA", 341, 2, wave::snd_play_sound_a),
    stdcall("PlaySoundW", 342, 3, wave::play_sound_w),
    stdcall("sndPlaySoundW", 343, 2, wave::play_sound_w),
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
}

/// A process ended: what it had goes.
pub fn process_ended(cpu: &mut Cpu, pid: u32, _tids: &[u32]) {
    callback::process_ended(cpu, pid);
    wave::process_ended(cpu, pid);
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
pub fn poll(_cpu: &mut Cpu, _pid: u32, _now: u64) -> Option<u64> {
    None
}

/// timeGetTime: the milliseconds since the machine started, by the
/// emulated clock.
fn time_get_time(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some((cpu.bus.clock.now_micros() / 1000) as u32)
}

crate::state_fields!(State { threads, queued, wave } skip { ticking });
