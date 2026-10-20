//! WINMM.DLL: the multimedia functions: timers, wave audio, MIDI, MCI
//! with CD audio, multimedia files and the joystick.
//!
//! (Phase 3 of docs/native-windows.md.)

pub mod callback;

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
    /// Whether `tick` runs, so that what it does doesn't run it again.
    ticking: bool,
}

/// A process ended: what it had goes.
pub fn process_ended(cpu: &mut Cpu, pid: u32, _tids: &[u32]) {
    callback::process_ended(cpu, pid);
}

/// Time passed (thread::wake): the devices' work that is due is done,
/// whichever process runs.
pub fn tick(cpu: &mut Cpu) {
    if cpu.bus.win32.winmm.ticking {
        return;
    }
    cpu.bus.win32.winmm.ticking = true;
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

crate::state_fields!(State { threads, queued } skip { ticking });
