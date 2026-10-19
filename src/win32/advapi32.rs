//! ADVAPI32.DLL: of what Windows 95 has, the security functions, which it
//! has only as stubs that fail.

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "ADVAPI32.DLL",
    base: 0xBFEC_0000,
    apis: APIS,
    vars: &[],
    init: None,
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
    stdcall("InitializeSecurityDescriptor", 1, 2, not_implemented),
    stdcall("SetSecurityDescriptorDacl", 2, 4, not_implemented),
    stdcall("InitializeAcl", 3, 3, not_implemented),
];

const FALSE: u32 = 0;
const ERROR_CALL_NOT_IMPLEMENTED: u32 = 120;

/// A function Windows 95 has only as a stub that fails.
fn not_implemented(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    super::process(cpu).last_error = ERROR_CALL_NOT_IMPLEMENTED;
    Some(FALSE)
}
