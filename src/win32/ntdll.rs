//! NTDLL.DLL: the few functions of Windows NT's native layer that Wine's
//! DLLs import and the other built-in DLLs can't stand in for. Programs
//! load these DLLs from the folder the `win32_dlls` setting names
//! (loader.rs); without the setting there is no NTDLL.DLL, as a Windows
//! 95 program that looks for it to tell Windows NT apart expects.

use super::stubs::{Api, Conv, Dll, Handler};

pub static DLL: Dll = Dll {
    name: "NTDLL.DLL",
    base: 0xBFE4_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: &[],
};

const fn cdecl(name: &'static str, ordinal: u16, args: u8, handler: Handler) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Cdecl,
        handler,
    }
}

/// The functions. Append only (see `stubs`).
static APIS: &[Api] = &[cdecl("_vsnprintf", 1, 4, super::msvcrt::vsnprintf)];
