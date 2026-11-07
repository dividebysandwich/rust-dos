//! NTDLL.DLL: the few functions of Windows NT's native layer that Windows
//! 9x's NTDLL.DLL has and programs import, and the ones Wine's DLLs import
//! that the other built-in DLLs can't stand in for. Programs load Wine's
//! DLLs from the folder the `win32_dlls` setting names (loader.rs).
//!
//! Windows 95 and 98 both have an NTDLL.DLL in the system directory:
//! Windows 95 OSR2.5's is 4.00.950 with 40 exports, Windows 98's and 98
//! SE's 4.10.1998 with 48. Its RtlUnwind jumps to KERNEL32's. Microsoft's
//! D3DRM.DLL, which games ship beside themselves, and DirectX 6.1a's
//! ADVPACK.DLL import RtlUnwind from it. The ordinals are Windows 98's
//! under every `win32_version` (ordinals.rs).

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

const fn stdcall(name: &'static str, ordinal: u16, args: u8, handler: Handler) -> Api {
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
    cdecl("_vsnprintf", 53, 4, super::msvcrt::vsnprintf),
    stdcall("RtlUnwind", 46, 4, super::seh::rtl_unwind),
];
