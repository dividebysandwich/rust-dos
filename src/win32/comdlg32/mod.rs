//! COMDLG32.DLL: the common dialogs. Each is cancelled, as when the
//! user closes it, with no extended error.

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "COMDLG32.DLL",
    base: 0xBFC8_0000,
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

/// COMDLG32's functions, with the ordinals Windows 98's has them at.
/// Append only (see `stubs`).
static APIS: &[Api] = &[
    stdcall("ChooseColorA", 101, 1, nothing),
    stdcall("ChooseFontA", 103, 1, nothing),
    stdcall("GetOpenFileNameA", 110, 1, nothing),
    stdcall("GetSaveFileNameA", 112, 1, nothing),
    stdcall("PrintDlgA", 117, 1, nothing),
    stdcall("PageSetupDlgA", 115, 1, nothing),
    stdcall("CommDlgExtendedError", 105, 0, nothing),
    stdcall("GetFileTitleA", 108, 3, minus_one),
];

/// A dialog cancelled: FALSE.
fn nothing(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn minus_one(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(u32::MAX)
}
