//! KERNEL32.DLL: the process, its console and the table of functions;
//! the other parts in the modules below.

mod disk;
mod memory;
mod module;
mod text;
mod time;

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
    stdcall("GetEnvironmentVariableA", 11, 3, get_environment_variable),
    stdcall("SetEnvironmentVariableA", 12, 2, set_environment_variable),
    stdcall("TlsAlloc", 13, 0, memory::tls_alloc),
    stdcall("TlsFree", 14, 1, memory::tls_free),
    stdcall("TlsGetValue", 15, 1, memory::tls_get_value),
    stdcall("TlsSetValue", 16, 2, memory::tls_set_value),
    stdcall("GetProcessHeap", 17, 0, memory::get_process_heap),
    stdcall("HeapAlloc", 18, 3, memory::heap_alloc),
    stdcall("HeapFree", 19, 3, memory::heap_free),
    stdcall("HeapReAlloc", 20, 4, memory::heap_realloc),
    stdcall("HeapSize", 21, 3, memory::heap_size),
    stdcall("InterlockedIncrement", 22, 1, memory::interlocked_increment),
    stdcall("InterlockedDecrement", 23, 1, memory::interlocked_decrement),
    stdcall("InterlockedExchange", 24, 2, memory::interlocked_exchange),
    stdcall("GetProcAddress", 25, 2, module::get_proc_address),
    stdcall("GetVersionExA", 26, 1, module::get_version_ex),
    stdcall("GetLogicalDrives", 27, 0, disk::get_logical_drives),
    stdcall("GetDriveTypeA", 28, 1, disk::get_drive_type),
    stdcall("GetDriveTypeW", 29, 1, not_implemented),
    stdcall("GetDiskFreeSpaceA", 30, 5, disk::get_disk_free_space),
    stdcall("GetDiskFreeSpaceW", 31, 5, not_implemented),
    stdcall("GetSystemTime", 32, 1, time::get_system_time),
    stdcall("GetLocalTime", 33, 1, time::get_system_time),
    stdcall(
        "SystemTimeToFileTime",
        34,
        2,
        time::system_time_to_file_time,
    ),
    stdcall(
        "FileTimeToSystemTime",
        35,
        2,
        time::file_time_to_system_time,
    ),
    stdcall("FileTimeToLocalFileTime", 36, 2, time::copy_file_time),
    stdcall("LocalFileTimeToFileTime", 37, 2, time::copy_file_time),
    stdcall(
        "DosDateTimeToFileTime",
        38,
        3,
        time::dos_date_time_to_file_time,
    ),
    stdcall(
        "FileTimeToDosDateTime",
        39,
        3,
        time::file_time_to_dos_date_time,
    ),
    stdcall(
        "GetTimeZoneInformation",
        40,
        1,
        time::get_time_zone_information,
    ),
    stdcall("WideCharToMultiByte", 41, 8, text::wide_char_to_multi_byte),
    stdcall("MultiByteToWideChar", 42, 6, text::multi_byte_to_wide_char),
];

/// The id of `ProcessReturn`, which the loader puts on the stack.
pub const PROCESS_RETURN: usize = 0;

const TRUE: u32 = 1;
const FALSE: u32 = 0;
const INVALID_HANDLE_VALUE: u32 = 0xFFFF_FFFF;
/// Error codes (GetLastError).
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_INVALID_DRIVE: u32 = 15;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_CALL_NOT_IMPLEMENTED: u32 = 120;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_MOD_NOT_FOUND: u32 = 126;
const ERROR_PROC_NOT_FOUND: u32 = 127;
const ERROR_ENVVAR_NOT_FOUND: u32 = 203;

fn set_error(cpu: &mut Cpu, code: u32) {
    super::process(cpu).last_error = code;
}

/// A function Windows 95 has only as a stub that fails: most of the
/// Unicode (W) ones. Programs check for this error to fall back to the
/// ANSI functions.
fn not_implemented(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    set_error(cpu, ERROR_CALL_NOT_IMPLEMENTED);
    Some(FALSE)
}

/// The string at `at`, read as ANSI text into a Rust string one char per
/// byte (`dosstr`). None if a page isn't there.
fn string_at(cpu: &mut Cpu, at: u32) -> Option<String> {
    let bytes = cpu.bus.guest_read_asciiz(at, 0x1_0000);
    (!cpu.bus.guest_faulted()).then(|| crate::dosstr::from_bytes(&bytes))
}

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

/// The environment variable `name`'s place in the environment: names are
/// found whatever their case.
fn variable(process: &super::Process, name: &str) -> Option<usize> {
    process.environment.iter().position(|s| {
        s.split_once('=')
            .is_some_and(|(n, _)| n.eq_ignore_ascii_case(name))
    })
}

/// The value of the variable at `args[0]` into the buffer at `args[1]` of
/// `args[2]` bytes: its length, or when the buffer is too small, the size
/// it needs.
fn get_environment_variable(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, buffer, size] = args else {
        unreachable!()
    };
    let name = string_at(cpu, name)?;
    let process = super::process(cpu);
    let Some(i) = variable(process, &name) else {
        process.last_error = ERROR_ENVVAR_NOT_FOUND;
        return Some(0);
    };
    let value = process.environment[i]
        .split_once('=')
        .map_or("", |(_, v)| v);
    let mut bytes = crate::dosstr::to_bytes(value);
    let len = bytes.len() as u32;
    if size <= len {
        return Some(len + 1);
    }
    bytes.push(0);
    cpu.bus.guest_write_bytes(buffer, &bytes);
    Some(len)
}

/// Set the variable at `args[0]` to the string at `args[1]`, or with NULL
/// remove it.
fn set_environment_variable(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = string_at(cpu, args[0])?;
    let value = match args[1] {
        0 => None,
        at => Some(string_at(cpu, at)?),
    };
    if name.is_empty() || name.contains('=') {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let process = super::process(cpu);
    let found = variable(process, &name);
    match (found, value) {
        (Some(i), Some(value)) => process.environment[i] = format!("{}={}", name, value),
        (None, Some(value)) => process.environment.push(format!("{}={}", name, value)),
        (Some(i), None) => {
            process.environment.remove(i);
        }
        (None, None) => {}
    }
    Some(TRUE)
}
