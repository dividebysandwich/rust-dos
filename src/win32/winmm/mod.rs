//! WINMM.DLL: the multimedia functions: timers, wave audio, MIDI, MCI
//! with CD audio, multimedia files and the joystick.
//!
//! (Phase 3 of docs/native-windows.md.)

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
static APIS: &[Api] = &[stdcall("timeGetTime", 1, 0, time_get_time)];

/// WinMM's state: the system's.
#[derive(Clone, Debug, Default)]
pub struct State {}

/// A process ended: what it had goes.
pub fn process_ended(_cpu: &mut Cpu, _pid: u32, _tids: &[u32]) {}

/// timeGetTime: the milliseconds since the machine started, by the
/// emulated clock.
fn time_get_time(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some((cpu.bus.clock.now_micros() / 1000) as u32)
}

crate::state_fields!(State {});
