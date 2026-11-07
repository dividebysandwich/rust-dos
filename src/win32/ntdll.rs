//! NTDLL.DLL: Windows 98's few functions of Windows NT's native layer, and
//! the ones Wine's DLLs import that the other built-in DLLs can't stand in
//! for. Programs load Wine's DLLs from the folder the `win32_dlls`
//! setting names (loader.rs).
//!
//! Windows 98 has an NTDLL.DLL in its system directory, which exports
//! RtlUnwind (forwarded to KERNEL32's) among others: Microsoft's
//! D3DRM.DLL, which games ship beside themselves, imports it from there.
//! Windows 95 has none, and a Windows 95 program that looks for it to
//! tell Windows NT apart expects that, so under `win32_version=95` there
//! is an NTDLL.DLL only with the `win32_dlls` setting (`present`).

use super::stubs::{Api, Conv, Dll, Handler};
use super::version::WindowsVersion;

pub static DLL: Dll = Dll {
    name: "NTDLL.DLL",
    base: 0xBFE4_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: &[],
};

/// Whether the system directory has NTDLL.DLL under `version`, without
/// the `win32_dlls` setting: from Windows 98 on.
pub fn present(version: WindowsVersion) -> bool {
    version.is_98()
}

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
