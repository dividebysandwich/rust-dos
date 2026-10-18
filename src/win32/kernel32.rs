//! KERNEL32.DLL: what a console program needs to start, write to its
//! console and end.

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

/// Windows 95's KERNEL32 is at BFF70000h, which programs and their copy
/// protection check to tell Windows 9x from NT.
pub static DLL: Dll = Dll {
    name: "KERNEL32.DLL",
    base: 0xBFF7_0000,
    apis: APIS,
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
    // Where the program's entry point returns to: it ends with the
    // value it returned.
    stdcall("ProcessReturn", 0, 0, process_return),
    stdcall("ExitProcess", 1, 1, exit_process),
    stdcall("GetStdHandle", 2, 1, get_std_handle),
    stdcall("WriteFile", 3, 5, write_file),
    stdcall("WriteConsoleA", 4, 5, write_file),
    stdcall("GetCommandLineA", 5, 0, get_command_line),
    stdcall("GetVersion", 6, 0, get_version),
    stdcall("GetModuleHandleA", 7, 1, get_module_handle),
    stdcall("GetLastError", 8, 0, get_last_error),
    stdcall("SetLastError", 9, 1, set_last_error),
    stdcall("GetTickCount", 10, 0, get_tick_count),
];

/// The id of `ProcessReturn`, which the loader puts on the stack.
pub const PROCESS_RETURN: usize = 0;

const TRUE: u32 = 1;
const FALSE: u32 = 0;
const INVALID_HANDLE_VALUE: u32 = 0xFFFF_FFFF;
/// Error codes (GetLastError).
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_MOD_NOT_FOUND: u32 = 126;

fn process_return(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let code = cpu.eax();
    super::exit(cpu, code);
    None
}

fn exit_process(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    super::exit(cpu, args[0]);
    None
}

/// STD_INPUT_HANDLE, STD_OUTPUT_HANDLE and STD_ERROR_HANDLE (-10, -11
/// and -12): handles of DOS's handles 0, 1 and 2.
fn get_std_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let process = super::process(cpu);
    let dos = match args[0] as i32 {
        -10 => 0,
        -11 => 1,
        -12 => 2,
        _ => {
            process.last_error = ERROR_INVALID_HANDLE;
            return Some(INVALID_HANDLE_VALUE);
        }
    };
    Some(process.dos_handle(dos))
}

/// WriteFile and WriteConsoleA: `n` bytes at `buffer` to `handle`, the
/// count written to `*written` (when it isn't NULL).
fn write_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, buffer, n, written, _] = args else {
        unreachable!()
    };
    let Some(dos) = super::process(cpu).dos_of(handle) else {
        super::process(cpu).last_error = ERROR_INVALID_HANDLE;
        return Some(FALSE);
    };
    let mut data = vec![0u8; n as usize];
    cpu.bus.guest_read_bytes(buffer, &mut data);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let result = super::dos(cpu, |cpu| {
        crate::interrupts::int21::write_handle(cpu, dos, &data)
    });
    let count = *result.as_ref().unwrap_or(&0) as u32;
    if written != 0 {
        cpu.bus.guest_write_32(written, count);
    }
    match result {
        Ok(_) => Some(TRUE),
        Err(code) => {
            super::process(cpu).last_error = code as u32;
            Some(FALSE)
        }
    }
}

fn get_command_line(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(super::process(cpu).command_line)
}

/// Windows 95: version 4.0, with bit 31 saying it isn't NT.
fn get_version(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0xC000_0004)
}

/// The module named `name` (with ".DLL" when it has no extension), or
/// with NULL the program's.
fn get_module_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(super::process(cpu).image_base);
    }
    let name = crate::dosstr::from_bytes(&cpu.bus.guest_read_asciiz(args[0], 260));
    if cpu.bus.guest_faulted() {
        return None;
    }
    let process = super::process(cpu);
    match process.module(&name) {
        Some(base) => Some(base),
        None => {
            process.last_error = ERROR_MOD_NOT_FOUND;
            Some(0)
        }
    }
}

fn get_last_error(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(super::process(cpu).last_error)
}

fn set_last_error(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    super::process(cpu).last_error = args[0];
    Some(0)
}

/// Milliseconds since the machine started, by its clock.
fn get_tick_count(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some((cpu.bus.clock.now_micros() / 1000) as u32)
}
