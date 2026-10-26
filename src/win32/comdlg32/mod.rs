//! COMDLG32.DLL: the common dialogs. The file dialogs are real:
//! GetOpenFileNameA and GetSaveFileNameA show Windows 98's Explorer-style
//! dialog, or Windows 3.1's when a program with a hook or a template
//! doesn't ask for the Explorer style, and always to a Windows 3.1
//! program, through COMMDLG (filedlg.rs, explorer.rs, old.rs).
//! ChooseColorA, ChooseFontA, PrintDlgA and PageSetupDlgA are cancelled,
//! as when the user closes them, with no extended error; FindTextA and
//! ReplaceTextA make no dialog. The Unicode functions fail with
//! ERROR_CALL_NOT_IMPLEMENTED, as Windows 98's stubs do.

mod browser;
mod explorer;
pub mod filedlg;
mod old;

use super::stubs::{Api, Conv, Dll, Vtable};
use crate::cpu::Cpu;
use crate::win32::user32;
use std::collections::BTreeMap;

pub static DLL: Dll = Dll {
    name: "COMDLG32.DLL",
    base: 0xBFC8_0000,
    apis: APIS,
    vars: &[],
    init: None,
    vtables: VTABLES,
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

/// COMDLG32's functions, with the ordinals Windows 98's has them at; the
/// dialog procedures and the methods of the file dialog's shell objects
/// (ordinal 0) aren't exported. Append only (see `stubs`).
static APIS: &[Api] = &[
    stdcall("ChooseColorA", 101, 1, cancelled),
    stdcall("ChooseFontA", 103, 1, cancelled),
    stdcall("GetOpenFileNameA", 110, 1, filedlg::get_open_file_name),
    stdcall("GetSaveFileNameA", 112, 1, filedlg::get_save_file_name),
    stdcall("PrintDlgA", 117, 1, cancelled),
    stdcall("PageSetupDlgA", 115, 1, cancelled),
    stdcall("CommDlgExtendedError", 105, 0, comm_dlg_extended_error),
    stdcall("GetFileTitleA", 108, 3, filedlg::get_file_title),
    stdcall("ChooseColorW", 102, 1, not_implemented),
    stdcall("ChooseFontW", 104, 1, not_implemented),
    stdcall("FindTextA", 106, 1, no_window),
    stdcall("FindTextW", 107, 1, not_implemented),
    stdcall("GetFileTitleW", 109, 3, not_implemented),
    stdcall("GetOpenFileNameW", 111, 1, not_implemented),
    stdcall("GetSaveFileNameW", 113, 1, not_implemented),
    stdcall("PageSetupDlgW", 116, 1, not_implemented),
    stdcall("PrintDlgW", 118, 1, not_implemented),
    stdcall("ReplaceTextA", 119, 1, no_window),
    stdcall("ReplaceTextW", 120, 1, not_implemented),
    stdcall("FileOpenDlgProc", 0, 4, explorer::proc_),
    stdcall(
        "FileOpenDlgProcUserTemplate",
        0,
        4,
        explorer::user_template_proc,
    ),
    stdcall("FileOpenDlgProc31", 0, 4, old::proc_),
    stdcall(
        "ShellBrowser_QueryInterface",
        0,
        3,
        browser::query_interface,
    ),
    stdcall("ShellBrowser_AddRef", 0, 1, browser::add_ref),
    stdcall("ShellBrowser_Release", 0, 1, browser::add_ref),
    stdcall("ShellBrowser_GetWindow", 0, 2, browser::get_window),
    stdcall("ShellBrowser_Arg0", 0, 1, browser::not_impl),
    stdcall("ShellBrowser_Arg1", 0, 2, browser::not_impl),
    stdcall("ShellBrowser_Arg2", 0, 3, browser::not_impl),
    stdcall("ShellBrowser_Arg3", 0, 4, browser::not_impl),
    stdcall("ShellBrowser_Arg5", 0, 6, browser::not_impl),
    stdcall(
        "ShellBrowser_GetControlWindow",
        0,
        3,
        browser::get_control_window,
    ),
    stdcall(
        "ShellBrowser_QueryActiveShellView",
        0,
        2,
        browser::query_active_shell_view,
    ),
    stdcall("ShellBrowser_Ok1", 0, 2, browser::ok),
    stdcall(
        "ShellView_QueryInterface",
        0,
        3,
        browser::view_query_interface,
    ),
    stdcall("GetOpenFileName16", 0, 1, filedlg::get_open_file_name16),
    stdcall("GetSaveFileName16", 0, 1, filedlg::get_save_file_name16),
];

/// The tables of methods of the file dialog's shell objects: the
/// IShellBrowser its WM_GETISHELLBROWSER gives, and its IShellView.
static VTABLES: &[Vtable] = &[
    Vtable {
        name: "IShellBrowser",
        methods: &[
            "ShellBrowser_QueryInterface",
            "ShellBrowser_AddRef",
            "ShellBrowser_Release",
            "ShellBrowser_GetWindow",
            "ShellBrowser_Arg1",
            "ShellBrowser_Arg2",
            "ShellBrowser_Arg3",
            "ShellBrowser_Arg1",
            "ShellBrowser_Arg1",
            "ShellBrowser_Arg1",
            "ShellBrowser_Arg2",
            "ShellBrowser_Arg2",
            "ShellBrowser_Arg2",
            "ShellBrowser_GetControlWindow",
            "ShellBrowser_Arg5",
            "ShellBrowser_QueryActiveShellView",
            "ShellBrowser_Ok1",
            "ShellBrowser_Arg3",
        ],
    },
    Vtable {
        name: "IShellView",
        methods: &[
            "ShellView_QueryInterface",
            "ShellBrowser_AddRef",
            "ShellBrowser_Release",
            "ShellBrowser_GetWindow",
            "ShellBrowser_Arg1",
            "ShellBrowser_Arg1",
            "ShellBrowser_Arg1",
            "ShellBrowser_Arg1",
            "ShellBrowser_Arg0",
            "ShellBrowser_Arg5",
            "ShellBrowser_Arg0",
            "ShellBrowser_Arg1",
            "ShellBrowser_Arg3",
            "ShellBrowser_Arg0",
            "ShellBrowser_Arg2",
            "ShellBrowser_Arg3",
        ],
    },
];

/// COMDLG32's place in `stubs::DLLS`.
fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("COMDLG32 is a built-in DLL")
}

/// The address of COMDLG32's function `name`: a dialog procedure.
pub fn address_of(name: &str) -> u32 {
    let index = APIS
        .iter()
        .position(|a| a.name == name)
        .unwrap_or_else(|| panic!("COMDLG32 has no {}", name));
    super::stubs::stub_address(dll_index(), index)
}

/// What the common dialogs keep: the extended error of the last one,
/// and the file dialogs open, by an id of their own.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub error: u32,
    pub next: u32,
    pub dialogs: BTreeMap<u32, filedlg::FileDlg>,
}

crate::state_fields!(State {
    error,
    next,
    dialogs
});

/// COMDLG32's state.
pub fn state(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.comdlg32
}

// The extended errors (CDERR_*, FNERR_*).
pub const CDERR_STRUCTSIZE: u32 = 0x0001;
pub const CDERR_INITIALIZATION: u32 = 0x0002;
pub const CDERR_FINDRESFAILURE: u32 = 0x0006;
pub const CDERR_LOADRESFAILURE: u32 = 0x0007;
pub const FNERR_BUFFERTOOSMALL: u32 = 0x3003;

pub fn set_extended_error(cpu: &mut Cpu, code: u32) {
    state(cpu).error = code;
}

/// CommDlgExtendedError(): why the last common dialog failed, or 0.
fn comm_dlg_extended_error(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(state(cpu).error)
}

/// ChooseColorA, ChooseFontA, PrintDlgA, PageSetupDlgA(info): the dialog
/// cancelled, FALSE with no extended error; without its structure,
/// CDERR_INITIALIZATION.
fn cancelled(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let error = if args[0] == 0 {
        CDERR_INITIALIZATION
    } else {
        0
    };
    set_extended_error(cpu, error);
    Some(0)
}

/// FindTextA, ReplaceTextA(info): no dialog, NULL.
fn no_window(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    set_extended_error(cpu, CDERR_INITIALIZATION);
    Some(0)
}

/// The Unicode functions, which Windows 98 has as stubs.
fn not_implemented(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    user32::not_implemented(cpu, args)
}
