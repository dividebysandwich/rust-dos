//! WINSPOOL.DRV: the print spooler's functions, as far as setup programs
//! ask it which ports there are (InstallShield's engine imports
//! EnumPortsA). The ports are those Windows 98 SE lists on a machine
//! without printers or a network.

use super::process;
use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "WINSPOOL.DRV",
    base: 0xBFEA_0000,
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
static APIS: &[Api] = &[stdcall("EnumPortsA", 1, 6, enum_ports_a)];

const ERROR_INVALID_LEVEL: u32 = 124;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_NOACCESS: u32 = 998;

/// PORT_INFO_2's fPortType: PORT_TYPE_WRITE, and PORT_TYPE_READ.
const PORT_TYPE_WRITE: u32 = 0x1;
const PORT_TYPE_READ_WRITE: u32 = 0x3;

/// The ports, with the description and type PORT_INFO_2 gives each, in
/// Windows 98 SE's order.
const PORTS: [(&str, &str, u32); 4] = [
    ("FILE:", "Creates a file on disk", PORT_TYPE_WRITE),
    ("LPT1:", "Printer Port", PORT_TYPE_READ_WRITE),
    ("COM1:", "Communications Port", PORT_TYPE_READ_WRITE),
    ("COM2:", "Communications Port", PORT_TYPE_READ_WRITE),
];
/// The monitor PORT_INFO_2 names for each port.
const MONITOR: &str = "Local Port";

/// EnumPortsA(server, level, buffer, size, needed, returned): the ports,
/// as PORT_INFO_1 (their names) or PORT_INFO_2, with the strings after
/// the structures. A buffer too small for them gets nothing, and the size
/// needed is in `needed`.
fn enum_ports_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[_server, level, buffer, size, needed, returned] = args else {
        unreachable!()
    };
    let entry = match level {
        1 => 4,
        2 => 20,
        _ => return fail(cpu, ERROR_INVALID_LEVEL),
    };
    // The structures, then each one's strings.
    let mut strings: Vec<&str> = Vec::new();
    for (name, description, _) in PORTS {
        strings.push(name);
        if level == 2 {
            strings.extend([MONITOR, description]);
        }
    }
    let structs = entry * PORTS.len() as u32;
    let total = structs + strings.iter().map(|s| s.len() as u32 + 1).sum::<u32>();
    if needed == 0 || !cpu.bus.guest_probe(needed, 4, true) {
        return fail(cpu, ERROR_NOACCESS);
    }
    cpu.bus.guest_write_32(needed, total);
    if returned != 0 && cpu.bus.guest_probe(returned, 4, true) {
        cpu.bus.guest_write_32(returned, 0);
    }
    if size < total || buffer == 0 {
        return fail(cpu, ERROR_INSUFFICIENT_BUFFER);
    }
    if !cpu.bus.guest_probe(buffer, total as usize, true) {
        return fail(cpu, ERROR_NOACCESS);
    }
    let mut text = buffer + structs;
    let mut place = |cpu: &mut Cpu, s: &str| {
        cpu.bus
            .guest_write_bytes(text, &[s.as_bytes(), &[0]].concat());
        text += s.len() as u32 + 1;
        text - s.len() as u32 - 1
    };
    for (i, (name, description, kind)) in PORTS.iter().enumerate() {
        let at = buffer + i as u32 * entry;
        let name = place(cpu, name);
        cpu.bus.guest_write_32(at, name);
        if level == 2 {
            let monitor = place(cpu, MONITOR);
            let description = place(cpu, description);
            cpu.bus.guest_write_32(at + 4, monitor);
            cpu.bus.guest_write_32(at + 8, description);
            cpu.bus.guest_write_32(at + 12, *kind);
            cpu.bus.guest_write_32(at + 16, 0);
        }
    }
    if returned != 0 && cpu.bus.guest_probe(returned, 4, true) {
        cpu.bus.guest_write_32(returned, PORTS.len() as u32);
    }
    Some(1)
}

fn fail(cpu: &mut Cpu, code: u32) -> Option<u32> {
    process(cpu).last_error = code;
    Some(0)
}
