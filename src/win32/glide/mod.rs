//! GLIDE2X.DLL: 3dfx's Glide 2.x, the library Windows games of 1996 to
//! 1999 drew on a Voodoo with, written against the emulated Voodoo's
//! registers as 3dfx's own library was against the card.
//!
//! (Phase 4 of docs/native-windows.md.)

use super::stubs::{Api, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "GLIDE2X.DLL",
    base: 0xBFB0_0000,
    apis: APIS,
    vars: &[],
    init: None,
    vtables: &[],
};

/// The functions. Append only (see `stubs`).
static APIS: &[Api] = &[];

/// Glide's state: the system's.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// The process that has the Voodoo open (grSstWinOpen), or 0.
    pub owner: u32,
}

crate::state_fields!(State { owner });

pub fn state(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.glide
}

/// A process ended: the Voodoo it had open is closed.
pub fn process_ended(cpu: &mut Cpu, pid: u32, _tids: &[u32]) {
    if state(cpu).owner == pid {
        state(cpu).owner = 0;
    }
}
