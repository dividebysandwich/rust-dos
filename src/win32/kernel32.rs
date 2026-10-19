//! KERNEL32.DLL: the process, its console and the table of functions;
//! the other parts in the modules below.

mod disk;
mod global;
mod mapping;
mod memory;
mod module;
mod process;
mod sync;
mod text;
mod threads;
mod time;
mod virtual_memory;

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
    // Where calls into the program return to (call.rs), the idle loop,
    // and where threads start and their functions return to.
    stdcall("CallReturn", 0, 0, call_return),
    Api {
        name: "Idle",
        ordinal: 0,
        args: 0,
        conv: Conv::Loop,
        handler: idle,
    },
    stdcall("ThreadStart", 0, 0, thread_start),
    stdcall("ThreadReturn", 0, 0, threads::thread_return),
    stdcall("CreateThread", 43, 6, threads::create_thread),
    stdcall("ExitThread", 44, 1, threads::exit_thread),
    stdcall("GetCurrentThread", 45, 0, threads::get_current_thread),
    stdcall("GetCurrentThreadId", 46, 0, threads::get_current_thread_id),
    stdcall("GetCurrentProcess", 47, 0, threads::get_current_process),
    stdcall("GetCurrentProcessId", 48, 0, threads::get_current_process_id),
    stdcall("GetExitCodeThread", 49, 2, threads::get_exit_code_thread),
    stdcall("GetExitCodeProcess", 50, 2, threads::get_exit_code_process),
    stdcall("TerminateThread", 51, 2, threads::terminate_thread),
    stdcall("TerminateProcess", 52, 2, threads::terminate_process),
    stdcall("SuspendThread", 53, 1, threads::suspend_thread),
    stdcall("ResumeThread", 54, 1, threads::resume_thread),
    stdcall("GetThreadPriority", 55, 1, threads::get_thread_priority),
    stdcall("SetThreadPriority", 56, 2, threads::set_thread_priority),
    stdcall("GetThreadContext", 57, 2, threads::get_thread_context),
    stdcall("SetThreadContext", 58, 2, threads::set_thread_context),
    stdcall("CloseHandle", 59, 1, threads::close_handle),
    stdcall("DuplicateHandle", 60, 7, threads::duplicate_handle),
    stdcall("CreateEventA", 61, 4, sync::create_event),
    stdcall("OpenEventA", 62, 3, sync::open_event),
    stdcall("SetEvent", 63, 1, sync::set_event),
    stdcall("ResetEvent", 64, 1, sync::reset_event),
    stdcall("PulseEvent", 65, 1, sync::pulse_event),
    stdcall("CreateMutexA", 66, 3, sync::create_mutex),
    stdcall("OpenMutexA", 67, 3, sync::open_mutex),
    stdcall("ReleaseMutex", 68, 1, sync::release_mutex),
    stdcall("CreateSemaphoreA", 69, 4, sync::create_semaphore),
    stdcall("OpenSemaphoreA", 70, 3, sync::open_semaphore),
    stdcall("ReleaseSemaphore", 71, 3, sync::release_semaphore),
    stdcall("WaitForSingleObject", 72, 2, sync::wait_for_single_object),
    stdcall("WaitForSingleObjectEx", 73, 3, sync::wait_for_single_object),
    stdcall(
        "WaitForMultipleObjects",
        74,
        4,
        sync::wait_for_multiple_objects,
    ),
    stdcall(
        "WaitForMultipleObjectsEx",
        75,
        5,
        sync::wait_for_multiple_objects,
    ),
    stdcall("Sleep", 76, 1, sync::sleep),
    stdcall("SleepEx", 77, 2, sync::sleep),
    stdcall(
        "InitializeCriticalSection",
        78,
        1,
        sync::initialize_critical_section,
    ),
    stdcall("DeleteCriticalSection", 79, 1, sync::delete_critical_section),
    stdcall("EnterCriticalSection", 80, 1, sync::enter_critical_section),
    stdcall("LeaveCriticalSection", 81, 1, sync::leave_critical_section),
    stdcall(
        "TryEnterCriticalSection",
        82,
        1,
        sync::try_enter_critical_section,
    ),
    stdcall("RaiseException", 83, 4, super::seh::raise_exception),
    stdcall("RtlUnwind", 84, 4, super::seh::rtl_unwind),
    stdcall(
        "UnhandledExceptionFilter",
        85,
        1,
        super::seh::unhandled_exception_filter,
    ),
    stdcall(
        "SetUnhandledExceptionFilter",
        86,
        1,
        super::seh::set_unhandled_exception_filter,
    ),
    stdcall("CreateProcessA", 87, 10, process::create_process_a),
    stdcall("OpenProcess", 88, 3, process::open_process),
    stdcall("GetStartupInfoA", 89, 1, process::get_startup_info),
    stdcall("VirtualAlloc", 200, 4, virtual_memory::virtual_alloc),
    stdcall("VirtualFree", 201, 3, virtual_memory::virtual_free),
    stdcall("VirtualProtect", 202, 4, virtual_memory::virtual_protect),
    stdcall("VirtualProtectEx", 203, 5, virtual_memory::virtual_protect_ex),
    stdcall("VirtualQuery", 204, 3, virtual_memory::virtual_query),
    stdcall("VirtualQueryEx", 205, 4, virtual_memory::virtual_query_ex),
    stdcall("VirtualLock", 206, 2, virtual_memory::virtual_lock),
    stdcall("VirtualUnlock", 207, 2, virtual_memory::virtual_lock),
    stdcall("HeapCreate", 208, 3, memory::heap_create),
    stdcall("HeapDestroy", 209, 1, memory::heap_destroy),
    stdcall("HeapValidate", 210, 3, memory::heap_validate),
    stdcall("HeapCompact", 211, 2, memory::heap_compact),
    stdcall("HeapLock", 212, 1, memory::heap_lock),
    stdcall("HeapUnlock", 213, 1, memory::heap_lock),
    stdcall("HeapWalk", 214, 2, not_implemented),
    stdcall("GetProcessHeaps", 215, 2, memory::get_process_heaps),
    stdcall("GlobalAlloc", 216, 2, global::global_alloc),
    stdcall("GlobalFree", 217, 1, global::global_free),
    stdcall("GlobalReAlloc", 218, 3, global::global_realloc),
    stdcall("GlobalSize", 219, 1, global::global_size),
    stdcall("GlobalLock", 220, 1, global::global_lock),
    stdcall("GlobalUnlock", 221, 1, global::global_unlock),
    stdcall("GlobalHandle", 222, 1, global::global_handle),
    stdcall("GlobalFlags", 223, 1, global::global_flags),
    stdcall("GlobalMemoryStatus", 224, 1, global::global_memory_status),
    stdcall("GlobalCompact", 225, 1, global::global_compact),
    stdcall("GlobalFix", 226, 1, global::global_fix),
    stdcall("GlobalUnfix", 227, 1, global::global_unfix),
    stdcall("GlobalWire", 228, 1, global::global_fix),
    stdcall("GlobalUnWire", 229, 1, global::global_unfix),
    stdcall("LocalAlloc", 230, 2, global::global_alloc),
    stdcall("LocalFree", 231, 1, global::global_free),
    stdcall("LocalReAlloc", 232, 3, global::global_realloc),
    stdcall("LocalSize", 233, 1, global::global_size),
    stdcall("LocalLock", 234, 1, global::global_lock),
    stdcall("LocalUnlock", 235, 1, global::global_unlock),
    stdcall("LocalHandle", 236, 1, global::global_handle),
    stdcall("LocalFlags", 237, 1, global::global_flags),
    stdcall("LocalCompact", 238, 1, global::global_compact),
    stdcall("LocalShrink", 239, 2, global::local_shrink),
    stdcall("IsBadReadPtr", 240, 2, virtual_memory::is_bad_read_ptr),
    stdcall("IsBadWritePtr", 241, 2, virtual_memory::is_bad_write_ptr),
    stdcall("IsBadCodePtr", 242, 1, virtual_memory::is_bad_code_ptr),
    stdcall("IsBadStringPtrA", 243, 2, virtual_memory::is_bad_string_ptr_a),
    stdcall("IsBadStringPtrW", 244, 2, virtual_memory::is_bad_string_ptr_w),
    stdcall("IsBadHugeReadPtr", 245, 2, virtual_memory::is_bad_read_ptr),
    stdcall("IsBadHugeWritePtr", 246, 2, virtual_memory::is_bad_write_ptr),
    stdcall("RtlMoveMemory", 247, 3, virtual_memory::rtl_move_memory),
    stdcall("RtlFillMemory", 248, 3, virtual_memory::rtl_fill_memory),
    stdcall("RtlZeroMemory", 249, 2, virtual_memory::rtl_zero_memory),
    stdcall("GetSystemInfo", 250, 1, virtual_memory::get_system_info),
    stdcall("CreateFileMappingA", 251, 6, mapping::create_file_mapping_a),
    stdcall("CreateFileMappingW", 252, 6, not_implemented),
    stdcall("OpenFileMappingA", 253, 3, mapping::open_file_mapping_a),
    stdcall("OpenFileMappingW", 254, 3, not_implemented),
    stdcall("MapViewOfFile", 255, 5, mapping::map_view_of_file),
    stdcall("MapViewOfFileEx", 256, 6, mapping::map_view_of_file_ex),
    stdcall("UnmapViewOfFile", 257, 1, mapping::unmap_view_of_file),
    stdcall("FlushViewOfFile", 258, 2, mapping::flush_view_of_file),
];

/// The places in `APIS` of the system's own stubs.
const CALL_RETURN: usize = 43;
const IDLE: usize = 44;
const THREAD_START: usize = 45;
const THREAD_RETURN: usize = 46;

/// Where calls into the program return to (call.rs).
pub fn call_return_address() -> u32 {
    super::stubs::stub_address(0, CALL_RETURN)
}

/// The idle loop's stub (thread.rs).
pub fn idle_address() -> u32 {
    super::stubs::stub_address(0, IDLE)
}

/// Where a new thread starts, and where its function returns to.
pub fn thread_start_address() -> u32 {
    super::stubs::stub_address(0, THREAD_START)
}

pub fn thread_return_address() -> u32 {
    super::stubs::stub_address(0, THREAD_RETURN)
}

fn call_return(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    super::call::returned(cpu);
    None
}

fn idle(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    super::thread::idle_trap(cpu);
    None
}

fn thread_start(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    super::thread::start(cpu);
    None
}

/// The id of `ProcessReturn`, which the loader puts on the stack.
pub const PROCESS_RETURN: usize = 0;

const TRUE: u32 = 1;
const FALSE: u32 = 0;
const INVALID_HANDLE_VALUE: u32 = 0xFFFF_FFFF;
/// Error codes (GetLastError).
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_ACCESS_DENIED: u32 = 5;
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

/// The program's entry point returned: its main thread ends with the
/// value it returned, and with it the process, unless other threads run.
fn process_return(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let code = cpu.eax();
    super::thread::exit_current(cpu, code);
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
    match args[0] as i32 {
        n @ -12..=-10 => Some(process.std[(-10 - n) as usize]),
        _ => {
            process.last_error = ERROR_INVALID_HANDLE;
            Some(INVALID_HANDLE_VALUE)
        }
    }
}

/// How much of a WriteFile's buffer goes to DOS at a time: DOS counts
/// bytes in 16 bits, and the program's length is its own.
const WRITE_CHUNK: u32 = 0x8000;

/// WriteFile and WriteConsoleA: `n` bytes at `buffer` to `handle`, the
/// count written to `*written` (when it isn't NULL).
fn write_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, buffer, n, written, _] = args else {
        unreachable!()
    };
    let Some(super::object::Kind::Dos { handle: dos }) = super::object::kind_of(cpu, handle) else {
        super::process(cpu).last_error = ERROR_INVALID_HANDLE;
        return Some(FALSE);
    };
    let mut count = 0u32;
    let mut error = None;
    loop {
        let len = (n - count).min(WRITE_CHUNK);
        let mut data = vec![0u8; len as usize];
        cpu.bus
            .guest_read_bytes(buffer.wrapping_add(count), &mut data);
        if cpu.bus.guest_faulted() {
            return None;
        }
        let result = super::dos(cpu, |cpu| {
            crate::interrupts::int21::write_handle(cpu, dos, &data)
        });
        match result {
            Ok(done) => {
                count += done as u32;
                // A short write is a full disk: the rest isn't written.
                if (done as u32) < len || count == n {
                    break;
                }
            }
            Err(code) => {
                error = Some(code);
                break;
            }
        }
    }
    if written != 0 {
        cpu.bus.guest_write_32(written, count);
    }
    match error {
        None => Some(TRUE),
        Some(code) => {
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
