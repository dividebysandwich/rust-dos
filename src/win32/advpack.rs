//! ADVPACK.DLL: the advanced INF installer of Internet Explorer's time,
//! as far as setup programs ask it what kind of Windows they run on.
//! Windows 9x has no administrators: IsNTAdmin is FALSE.

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "ADVPACK.DLL",
    base: 0xBFE4_0000,
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
    stdcall("IsNTAdmin", 1, 2, is_nt_admin),
    stdcall("NeedReboot", 2, 1, false_),
    stdcall("NeedRebootInit", 3, 0, false_),
];

/// IsNTAdmin(reserved, reserved): FALSE, as on Windows 9x.
fn is_nt_admin(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn false_(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}
