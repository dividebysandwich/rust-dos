//! MPR.DLL: the network provider router. Wise's installer imports
//! WNetGetConnectionA to ask whether the drive it installs to is a
//! network share; on the built-in Windows no drive is one.

use super::process;
use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "MPR.DLL",
    base: 0xBFEE_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
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
static APIS: &[Api] = &[stdcall("WNetGetConnectionA", 1, 3, wnet_get_connection_a)];

/// The local device isn't redirected to a network resource.
const ERROR_NOT_CONNECTED: u32 = 2250;
/// The name isn't a device's.
const ERROR_BAD_DEVICE: u32 = 1200;

/// WNetGetConnectionA(local, remote, length): the network name of a
/// redirected drive or port. None is redirected, so a drive ("C:") or a
/// port ("LPT1") gets ERROR_NOT_CONNECTED, with an empty string in a
/// buffer of at least one byte, and any other name ERROR_BAD_DEVICE, as
/// on Windows 98 SE. The length stays as it was.
fn wnet_get_connection_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[local, remote, length] = args else {
        unreachable!()
    };
    let name = if local == 0 {
        String::new()
    } else {
        super::user32::string_at(cpu, local)?.to_ascii_uppercase()
    };
    let b = name.as_bytes();
    let drive = b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    let port = b.len() == 4
        && (name.starts_with("LPT") || name.starts_with("COM"))
        && b[3].is_ascii_digit();
    let code = if !(drive || port) {
        ERROR_BAD_DEVICE
    } else {
        let room = if length != 0 && cpu.bus.guest_probe(length, 4, false) {
            cpu.bus.guest_read_32(length)
        } else {
            0
        };
        if remote != 0 && room > 0 && cpu.bus.guest_probe(remote, 1, true) {
            cpu.bus.guest_write_bytes(remote, &[0]);
        }
        ERROR_NOT_CONNECTED
    };
    process(cpu).last_error = code;
    Some(code)
}
