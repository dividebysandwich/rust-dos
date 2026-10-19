//! KERNEL32.DLL: the process, its console and the table of functions;
//! the other parts in the modules below.

mod disk;
mod env;
mod locale;
mod memory;
mod message;
mod module;
mod process;
mod profile;
mod sort;
mod sync;
mod text;
mod threads;
mod time;

pub(super) use locale::enum_returned;

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
    stdcall("lstrlenA", 400, 1, text::lstrlen_a),
    stdcall("lstrcpyA", 401, 2, text::lstrcpy_a),
    stdcall("lstrcpynA", 402, 3, text::lstrcpyn_a),
    stdcall("lstrcatA", 403, 2, text::lstrcat_a),
    stdcall("lstrcmpA", 404, 2, text::lstrcmp_a),
    stdcall("lstrcmpiA", 405, 2, text::lstrcmpi_a),
    stdcall("lstrlenW", 406, 1, text::lstrlen_w),
    stdcall("lstrcpyW", 407, 2, not_implemented),
    stdcall("lstrcpynW", 408, 3, not_implemented),
    stdcall("lstrcatW", 409, 2, not_implemented),
    stdcall("lstrcmpW", 410, 2, not_implemented),
    stdcall("lstrcmpiW", 411, 2, not_implemented),
    stdcall("GetACP", 412, 0, text::get_acp),
    stdcall("GetOEMCP", 413, 0, text::get_oem_cp),
    stdcall("GetCPInfo", 414, 2, text::get_cp_info),
    stdcall("IsValidCodePage", 415, 1, text::is_valid_code_page),
    stdcall("IsDBCSLeadByte", 416, 1, text::is_dbcs_lead_byte),
    stdcall("IsDBCSLeadByteEx", 417, 2, text::is_dbcs_lead_byte_ex),
    stdcall("GetStringTypeA", 418, 5, sort::get_string_type_a),
    stdcall("GetStringTypeExA", 419, 5, sort::get_string_type_a),
    stdcall("GetStringTypeW", 420, 4, not_implemented),
    stdcall("GetStringTypeExW", 421, 5, not_implemented),
    stdcall("CompareStringA", 422, 6, sort::compare_string_a),
    stdcall("CompareStringW", 423, 6, not_implemented),
    stdcall("LCMapStringA", 424, 6, sort::lc_map_string_a),
    stdcall("LCMapStringW", 425, 6, not_implemented),
    stdcall("GetLocaleInfoA", 426, 4, locale::get_locale_info_a),
    stdcall("GetLocaleInfoW", 427, 4, not_implemented),
    stdcall("SetLocaleInfoA", 428, 3, locale::set_locale_info_a),
    stdcall("GetUserDefaultLCID", 429, 0, locale::get_default_lcid),
    stdcall("GetSystemDefaultLCID", 430, 0, locale::get_default_lcid),
    stdcall("GetUserDefaultLangID", 431, 0, locale::get_default_lang_id),
    stdcall("GetSystemDefaultLangID", 432, 0, locale::get_default_lang_id),
    stdcall("GetThreadLocale", 433, 0, locale::get_default_lcid),
    stdcall("SetThreadLocale", 434, 1, locale::set_thread_locale),
    stdcall("IsValidLocale", 435, 2, locale::is_valid_locale),
    stdcall("ConvertDefaultLocale", 436, 1, locale::convert_default_locale),
    stdcall("EnumSystemLocalesA", 437, 2, locale::enum_system_locales_a),
    stdcall("EnumDateFormatsA", 438, 3, locale::enum_date_formats_a),
    stdcall("EnumTimeFormatsA", 439, 3, locale::enum_time_formats_a),
    stdcall("GetDateFormatA", 440, 6, locale::get_date_format_a),
    stdcall("GetDateFormatW", 441, 6, not_implemented),
    stdcall("GetTimeFormatA", 442, 6, locale::get_time_format_a),
    stdcall("GetTimeFormatW", 443, 6, not_implemented),
    stdcall("GetNumberFormatA", 444, 6, locale::get_number_format_a),
    stdcall("GetNumberFormatW", 445, 6, not_implemented),
    stdcall("GetCurrencyFormatA", 446, 6, locale::get_currency_format_a),
    stdcall("GetCurrencyFormatW", 447, 6, not_implemented),
    stdcall("FormatMessageA", 448, 7, message::format_message_a),
    stdcall("FormatMessageW", 449, 7, not_implemented),
    stdcall("GetEnvironmentStrings", 450, 0, env::get_environment_strings),
    stdcall("GetEnvironmentStringsA", 451, 0, env::get_environment_strings),
    stdcall("GetEnvironmentStringsW", 452, 0, not_implemented),
    stdcall("FreeEnvironmentStringsA", 453, 1, env::free_environment_strings_a),
    stdcall("FreeEnvironmentStringsW", 454, 1, not_implemented),
    stdcall("ExpandEnvironmentStringsA", 455, 3, env::expand_environment_strings_a),
    stdcall("ExpandEnvironmentStringsW", 456, 3, not_implemented),
    stdcall("GetEnvironmentVariableW", 457, 3, not_implemented),
    stdcall("SetEnvironmentVariableW", 458, 2, not_implemented),
    stdcall("GetComputerNameA", 459, 2, env::get_computer_name_a),
    stdcall("GetComputerNameW", 460, 2, not_implemented),
    stdcall("SetComputerNameA", 461, 1, env::set_computer_name_a),
    stdcall("GetPrivateProfileStringA", 462, 6, profile::get_private_profile_string_a),
    stdcall("GetPrivateProfileStringW", 463, 6, not_implemented),
    stdcall("WritePrivateProfileStringA", 464, 4, profile::write_private_profile_string_a),
    stdcall("WritePrivateProfileStringW", 465, 4, not_implemented),
    stdcall("GetPrivateProfileIntA", 466, 4, profile::get_private_profile_int_a),
    stdcall("GetProfileIntA", 467, 3, profile::get_profile_int_a),
    stdcall("GetProfileStringA", 468, 5, profile::get_profile_string_a),
    stdcall("WriteProfileStringA", 469, 3, profile::write_profile_string_a),
    stdcall("GetPrivateProfileSectionA", 470, 4, profile::get_private_profile_section_a),
    stdcall("GetProfileSectionA", 471, 3, profile::get_profile_section_a),
    stdcall("GetPrivateProfileSectionNamesA", 472, 3, profile::get_private_profile_section_names_a),
    stdcall("GetPrivateProfileSectionNamesW", 473, 3, not_implemented),
    stdcall("WritePrivateProfileSectionA", 474, 3, profile::write_private_profile_section_a),
    stdcall("WriteProfileSectionA", 475, 2, profile::write_profile_section_a),
    stdcall("OutputDebugStringA", 476, 1, env::output_debug_string_a),
    stdcall("OutputDebugStringW", 477, 1, not_implemented),
    stdcall("Beep", 478, 2, env::beep),
    stdcall("GetSystemTimeAsFileTime", 479, 1, time::get_system_time_as_file_time),
    stdcall("QueryPerformanceCounter", 480, 1, env::query_performance_counter),
    stdcall("QueryPerformanceFrequency", 481, 1, env::query_performance_frequency),
    stdcall("CompareFileTime", 482, 2, time::compare_file_time),
    stdcall("SetSystemTime", 483, 1, time::set_system_time),
    stdcall("SetLocalTime", 484, 1, time::set_system_time),
    stdcall("MulDiv", 485, 3, env::mul_div),
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
/// it needs. No name is found for NULL.
fn get_environment_variable(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[name, buffer, size] = args else {
        unreachable!()
    };
    if name == 0 {
        set_error(cpu, ERROR_ENVVAR_NOT_FOUND);
        return Some(0);
    }
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
/// or, as on Windows 95, an empty string remove it.
fn set_environment_variable(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = string_at(cpu, args[0])?;
    let value = match args[1] {
        0 => None,
        at => Some(string_at(cpu, at)?).filter(|v| !v.is_empty()),
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
