//! WOW32.DLL: the generic thunk functions, how a Win32 program calls
//! 16-bit code and reaches its memory without a thunk compiler's DLLs.
//! Windows 98 has the DLL for programs written for NT's WOW; its
//! functions there go on to KERNEL32's K32WOW ones. Copy protections
//! such as SafeDisc load it to call into KRNL386.
//!
//! The 16-bit calls run as QT_Thunk's do (src/win16/flat.rs): on the
//! process's task in the Win16 process, while the calling thread waits.
//! The functions it has no use for in the built-in Windows yet
//! (WOWGlobalAlloc16 and the others, WOWHandle16 and WOWHandle32) aren't
//! exported, so a program that needs one is told which.

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;
use crate::win16::flat;

pub static DLL: Dll = Dll {
    name: "WOW32.DLL",
    base: 0xBF6E_0000,
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

/// The functions, by Windows 98 SE's ordinals. Append only (see
/// `stubs`).
static APIS: &[Api] = &[
    stdcall("WOWCallback16", 2, 2, flat::wow_callback16),
    stdcall("WOWCallback16Ex", 3, 5, flat::wow_callback16_ex),
    stdcall("WOWDirectedYield16", 4, 1, yield16),
    stdcall("WOWGetVDMPointer", 5, 3, get_vdm_pointer),
    stdcall("WOWGetVDMPointerFix", 6, 3, get_vdm_pointer),
    stdcall("WOWGetVDMPointerUnfix", 7, 1, nothing),
    stdcall("WOWYield16", 17, 0, yield16),
];

/// WOWGetVDMPointer(address, bytes, protected mode): the flat address of
/// a 16:16 one, as MapSL makes it. A real-mode segment:offset has none
/// the program can reach (the first megabyte isn't mapped for it): 0.
fn get_vdm_pointer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[2] == 0 {
        cpu.bus.log_string(&format!(
            "[W32] WOWGetVDMPointer of the real-mode address {:04X}:{:04X}",
            args[0] >> 16,
            args[0] & 0xFFFF
        ));
        return Some(0);
    }
    Some(flat::map_sl(cpu, args[0]))
}

/// WOWYield16, WOWDirectedYield16: let other 16-bit tasks run. The
/// process's task runs only when it is called, so there is nothing to
/// give up.
fn yield16(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn nothing(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}
