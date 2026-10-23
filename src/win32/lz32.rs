//! LZ32.DLL: the LZExpand functions for 32-bit programs, which are
//! KERNEL32's own on Windows 95 (kernel32/lz.rs); this DLL exports the
//! same ones, as setup programs import them from it.

use super::kernel32::lz;
use super::stubs::{Api, Conv, Dll};

pub static DLL: Dll = Dll {
    name: "LZ32.DLL",
    base: 0xBFD2_0000,
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

/// The functions, by LZ32's ordinals. Append only (see `stubs`).
static APIS: &[Api] = &[
    stdcall("CopyLZFile", 1, 2, lz::lz_copy),
    stdcall("GetExpandedNameA", 2, 2, lz::get_expanded_name_a),
    stdcall("GetExpandedNameW", 3, 2, lz::lz_open_file_w),
    stdcall("LZClose", 4, 1, lz::lz_close),
    stdcall("LZCopy", 5, 2, lz::lz_copy),
    stdcall("LZDone", 6, 0, lz::lz_done),
    stdcall("LZInit", 7, 1, lz::lz_init),
    stdcall("LZOpenFileA", 8, 3, lz::lz_open_file_a),
    stdcall("LZOpenFileW", 9, 3, lz::lz_open_file_w),
    stdcall("LZRead", 10, 3, lz::lz_read),
    stdcall("LZSeek", 11, 3, lz::lz_seek),
    stdcall("LZStart", 12, 0, lz::lz_start),
];
