//! DDRAW.DLL: DirectDraw: the display's surfaces, palettes and clippers, in full
//! screen on the VBE modes and in a window on the desktop.
//!
//! (Phase 3 of docs/native-windows.md.)

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "DDRAW.DLL",
    base: 0xBFA0_0000,
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
    stdcall("DllCanUnloadNow", 1, 0, dll_can_unload_now),
    stdcall("DllGetClassObject", 2, 3, dll_get_class_object),
];

/// DirectDraw's state: the system's.
#[derive(Clone, Debug, Default)]
pub struct State {}

/// A process ended: what it had goes.
pub fn process_ended(_cpu: &mut Cpu, _pid: u32, _tids: &[u32]) {}

fn dll_can_unload_now(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(super::com::S_FALSE)
}

fn dll_get_class_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(super::ole32::dll_get_class_object(cpu, DLL.name, args))
}

crate::state_fields!(State {});
