//! COMCTL32.DLL and COMDLG32.DLL: the common controls' registration,
//! which USER32's window classes have no need of, and the common
//! dialogs, which the built-in Windows hasn't: each is cancelled, as
//! when the user closes it, with no extended error.

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "COMCTL32.DLL",
    base: 0xBFC4_0000,
    apis: APIS,
    vars: &[],
    init: None,
    vtables: &[],
};

pub static COMDLG32: Dll = Dll {
    name: "COMDLG32.DLL",
    base: 0xBFC8_0000,
    apis: COMDLG32_APIS,
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

/// COMCTL32's functions, with Windows 95's ordinals. Append only (see
/// `stubs`).
static APIS: &[Api] = &[
    stdcall("InitCommonControls", 17, 0, nothing),
    stdcall("InitCommonControlsEx", 81, 1, true_),
];

/// COMDLG32's functions. Append only.
static COMDLG32_APIS: &[Api] = &[
    stdcall("ChooseColorA", 1, 1, nothing),
    stdcall("ChooseFontA", 2, 1, nothing),
    stdcall("GetOpenFileNameA", 3, 1, nothing),
    stdcall("GetSaveFileNameA", 4, 1, nothing),
    stdcall("PrintDlgA", 5, 1, nothing),
    stdcall("PageSetupDlgA", 6, 1, nothing),
    stdcall("CommDlgExtendedError", 7, 0, nothing),
    stdcall("GetFileTitleA", 8, 3, minus_one),
];

/// A dialog cancelled: FALSE; and InitCommonControls.
fn nothing(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn true_(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

fn minus_one(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(u32::MAX)
}
