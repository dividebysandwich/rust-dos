//! KERNEL32.DLL: the process, its console and the table of functions;
//! the other parts in the modules below.

mod console;
mod disk;
mod env;
mod file;
mod global;
mod locale;
pub(super) mod lz;
mod mapping;
mod memory;
mod message;
pub(super) mod module;
mod path;
mod process;
mod profile;
mod sort;
mod sync;
mod text;
mod threads;
mod time;
pub(crate) mod virtual_memory;

pub(super) use locale::enum_returned;
pub(super) use path::{change_directory, current_directory, full_path};
pub(super) use process::{STARTUPINFO_SIZE, spawn};
pub(super) use file::{
    CREATE_ALWAYS, CREATE_NEW, FILE_SHARE_READ, FILE_SHARE_WRITE, GENERIC_READ, GENERIC_WRITE,
    OPEN_ALWAYS, OPEN_EXISTING, TRUNCATE_EXISTING, new_file, open_file, open_of,
};

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

/// Windows 95's KERNEL32 is at BFF70000h, which programs and their copy
/// protection check to tell Windows 9x from NT.
pub static DLL: Dll = Dll {
    name: "KERNEL32.DLL",
    base: 0xBFF7_0000,
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
    // Where the program's entry point returns to: it ends with the
    // value it returned.
    stdcall("ProcessReturn", 0, 0, process_return),
    stdcall("ExitProcess", 1, 1, exit_process),
    stdcall("GetStdHandle", 2, 1, get_std_handle),
    stdcall("WriteFile", 3, 5, file::write_file),
    stdcall("WriteConsoleA", 4, 5, file::write_file),
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
    stdcall("CreateRemoteThread", 90, 7, not_implemented),
    stdcall("GetThreadTimes", 91, 5, not_implemented),
    stdcall("GetProcessTimes", 92, 5, not_implemented),
    stdcall(
        "GetProcessAffinityMask",
        93,
        3,
        process::get_process_affinity_mask,
    ),
    stdcall(
        "SetThreadAffinityMask",
        94,
        2,
        process::set_thread_affinity_mask,
    ),
    stdcall("GetPriorityClass", 95, 1, process::get_priority_class),
    stdcall("SetPriorityClass", 96, 2, process::set_priority_class),
    stdcall("ReadProcessMemory", 97, 5, process::read_process_memory),
    stdcall("WriteProcessMemory", 98, 5, process::write_process_memory),
    stdcall(
        "FlushInstructionCache",
        99,
        3,
        process::flush_instruction_cache,
    ),
    stdcall("SetErrorMode", 500, 1, process::set_error_mode),
    stdcall("GetProcessVersion", 501, 1, process::get_process_version),
    stdcall("FatalAppExitA", 502, 2, process::fatal_app_exit),
    stdcall("FatalExit", 503, 1, process::fatal_exit),
    stdcall("CreateEventW", 504, 4, not_implemented),
    stdcall("CreateMutexW", 505, 3, not_implemented),
    stdcall("CreateSemaphoreW", 506, 4, not_implemented),
    stdcall("OpenEventW", 507, 3, not_implemented),
    stdcall("OpenMutexW", 508, 3, not_implemented),
    stdcall("OpenSemaphoreW", 509, 3, not_implemented),

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

    // Files, directories, paths and the console.
    stdcall("CreateFileA", 100, 7, file::create_file),
    stdcall("CreateFileW", 101, 7, not_implemented_handle),
    stdcall("ReadFile", 102, 5, file::read_file),
    stdcall("SetFilePointer", 103, 4, file::set_file_pointer),
    stdcall("GetFileSize", 104, 2, file::get_file_size),
    stdcall("GetFileType", 105, 1, file::get_file_type),
    stdcall("FlushFileBuffers", 106, 1, file::flush_file_buffers),
    stdcall("SetEndOfFile", 107, 1, file::set_end_of_file),
    stdcall("GetFileInformationByHandle", 108, 2, file::get_file_information_by_handle),
    stdcall("GetFileTime", 109, 4, file::get_file_time),
    stdcall("SetFileTime", 110, 4, file::set_file_time),
    stdcall("LockFile", 111, 5, file::lock_file),
    stdcall("UnlockFile", 112, 5, file::unlock_file),
    stdcall("LockFileEx", 113, 6, file::lock_file_ex),
    stdcall("UnlockFileEx", 114, 5, file::lock_file_ex),
    stdcall("GetOverlappedResult", 115, 4, file::get_overlapped_result),
    stdcall("ReadFileEx", 116, 5, file::file_ex),
    stdcall("WriteFileEx", 117, 5, file::file_ex),
    stdcall("_lopen", 118, 2, file::lopen),
    stdcall("_lcreat", 119, 2, file::lcreat),
    stdcall("_lclose", 120, 1, file::lclose),
    stdcall("_lread", 121, 3, file::lread),
    stdcall("_hread", 122, 3, file::lread),
    stdcall("_lwrite", 123, 3, file::lwrite),
    stdcall("_hwrite", 124, 3, file::lwrite),
    stdcall("_llseek", 125, 3, file::llseek),
    stdcall("OpenFile", 126, 3, file::open_file_16),
    stdcall("SetHandleCount", 127, 1, file::set_handle_count),
    stdcall("DeleteFileA", 128, 1, path::delete_file),
    stdcall("DeleteFileW", 129, 1, not_implemented),
    stdcall("MoveFileA", 130, 2, path::move_file),
    stdcall("MoveFileW", 131, 2, not_implemented),
    stdcall("CopyFileA", 132, 3, path::copy_file),
    stdcall("CopyFileW", 133, 3, not_implemented),
    stdcall("GetFileAttributesA", 134, 1, path::get_file_attributes),
    stdcall("GetFileAttributesW", 135, 1, not_implemented_handle),
    stdcall("SetFileAttributesA", 136, 2, path::set_file_attributes),
    stdcall("SetFileAttributesW", 137, 2, not_implemented),
    stdcall("FindFirstFileA", 138, 2, path::find_first_file),
    stdcall("FindFirstFileW", 139, 2, not_implemented_handle),
    stdcall("FindNextFileA", 140, 2, path::find_next_file),
    stdcall("FindNextFileW", 141, 2, not_implemented),
    stdcall("FindClose", 142, 1, path::find_close),
    stdcall("CreateDirectoryA", 143, 2, path::create_directory),
    stdcall("CreateDirectoryW", 144, 2, not_implemented),
    stdcall("RemoveDirectoryA", 145, 1, path::remove_directory),
    stdcall("RemoveDirectoryW", 146, 1, not_implemented),
    stdcall("GetCurrentDirectoryA", 147, 2, path::get_current_directory),
    stdcall("GetCurrentDirectoryW", 148, 2, not_implemented),
    stdcall("SetCurrentDirectoryA", 149, 1, path::set_current_directory),
    stdcall("SetCurrentDirectoryW", 150, 1, not_implemented),
    stdcall("GetFullPathNameA", 151, 4, path::get_full_path_name),
    stdcall("GetFullPathNameW", 152, 4, not_implemented),
    stdcall("GetShortPathNameA", 153, 3, path::get_short_path_name),
    stdcall("GetShortPathNameW", 154, 3, not_implemented),
    stdcall("GetTempPathA", 155, 2, path::get_temp_path),
    stdcall("GetTempPathW", 156, 2, not_implemented),
    stdcall("GetTempFileNameA", 157, 4, path::get_temp_file_name),
    stdcall("GetTempFileNameW", 158, 4, not_implemented),
    stdcall("SearchPathA", 159, 6, path::search_path),
    stdcall("SearchPathW", 160, 6, not_implemented),
    stdcall("GetWindowsDirectoryA", 161, 2, path::get_windows_directory),
    stdcall("GetWindowsDirectoryW", 162, 2, not_implemented),
    stdcall("GetSystemDirectoryA", 163, 2, path::get_system_directory),
    stdcall("GetSystemDirectoryW", 164, 2, not_implemented),
    stdcall("ReadConsoleA", 165, 5, console::read_console),
    stdcall("GetConsoleMode", 166, 2, console::get_console_mode),
    stdcall("SetConsoleMode", 167, 2, console::set_console_mode),
    stdcall("SetConsoleCtrlHandler", 168, 2, console::set_console_ctrl_handler),
    stdcall("GetConsoleScreenBufferInfo", 169, 2, console::get_console_screen_buffer_info),
    stdcall("SetConsoleCursorPosition", 170, 2, console::set_console_cursor_position),
    stdcall("FillConsoleOutputCharacterA", 171, 5, console::fill_console_output_character),
    stdcall("FillConsoleOutputAttribute", 172, 5, console::fill_console_output_attribute),
    stdcall("WriteConsoleOutputA", 173, 5, console::write_console_output),
    stdcall("SetConsoleTextAttribute", 174, 2, console::set_console_text_attribute),
    stdcall("AllocConsole", 175, 0, console::alloc_console),
    stdcall("FreeConsole", 176, 0, console::free_console),
    stdcall("GetConsoleCP", 177, 0, console::get_console_cp),
    stdcall("GetConsoleOutputCP", 178, 0, console::get_console_cp),
    stdcall("SetStdHandle", 179, 2, console::set_std_handle),
    stdcall("GetNumberOfConsoleInputEvents", 180, 2, console::get_number_of_console_input_events),
    stdcall("PeekConsoleInputA", 181, 4, console::peek_console_input),
    stdcall("ReadConsoleInputA", 182, 4, console::read_console_input),
    stdcall("FlushConsoleInputBuffer", 183, 1, console::flush_console_input_buffer),
    stdcall("SetConsoleTitleA", 184, 1, console::set_console_title),

    // Where the main thread starts when DLLs that came with the program
    // have entry points, which run before the program's.
    stdcall("ProcessStart", 0, 0, module::process_start),
    stdcall("LoadLibraryA", 300, 1, module::load_library),
    stdcall("LoadLibraryExA", 301, 3, module::load_library_ex),
    stdcall("FreeLibrary", 302, 1, module::free_library),
    stdcall(
        "FreeLibraryAndExitThread",
        303,
        2,
        module::free_library_and_exit_thread,
    ),
    stdcall("GetModuleFileNameA", 304, 3, module::get_module_file_name),
    stdcall("GetModuleFileNameW", 305, 3, not_implemented),
    stdcall(
        "DisableThreadLibraryCalls",
        306,
        1,
        module::disable_thread_library_calls,
    ),
    stdcall("FindResourceA", 307, 3, module::find_resource),
    stdcall("FindResourceExA", 308, 4, module::find_resource_ex),
    stdcall("LoadResource", 309, 2, module::load_resource),
    stdcall("LockResource", 310, 1, module::lock_resource),
    stdcall("SizeofResource", 311, 2, module::sizeof_resource),
    stdcall("FreeResource", 312, 1, module::free_resource),
    stdcall("EnumResourceTypesA", 313, 3, module::enum_resource_types),
    stdcall("EnumResourceNamesA", 314, 4, module::enum_resource_names),
    stdcall("LoadLibraryW", 315, 1, not_implemented),
    stdcall("LoadLibraryExW", 316, 3, not_implemented),
    stdcall("GetModuleHandleW", 317, 1, not_implemented),

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
    stdcall("GetCommandLineW", 486, 0, get_command_line_w),
    stdcall("GetProcessWorkingSetSize", 487, 3, not_implemented),
    stdcall("GetVersionExW", 488, 1, not_implemented),
    stdcall("GlobalAddAtomW", 489, 1, not_implemented),
    stdcall("CreateProcessW", 490, 10, not_implemented),
    stdcall("WinExec", 510, 2, process::win_exec),
    stdcall("LZStart", 901, 0, lz::lz_start),
    stdcall("LZDone", 902, 0, lz::lz_done),
    stdcall("LZInit", 903, 1, lz::lz_init),
    stdcall("LZOpenFileA", 904, 3, lz::lz_open_file_a),
    stdcall("LZOpenFileW", 905, 3, lz::lz_open_file_w),
    stdcall("LZRead", 906, 3, lz::lz_read),
    stdcall("LZSeek", 907, 3, lz::lz_seek),
    stdcall("LZCopy", 908, 2, lz::lz_copy),
    stdcall("CopyLZFile", 909, 2, lz::lz_copy),
    stdcall("LZClose", 910, 1, lz::lz_close),
    stdcall("GetExpandedNameA", 911, 2, lz::get_expanded_name_a),
    stdcall("GetExpandedNameW", 912, 2, lz::lz_open_file_w),
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

/// Whether the running process's environment has the variable `name`.
pub fn has_environment_variable(cpu: &mut Cpu, name: &str) -> bool {
    variable(super::process(cpu), name).is_some()
}

/// Let go of the file mappings no handle or view holds any more.
pub fn sweep_mappings(cpu: &mut Cpu) {
    mapping::sweep(cpu);
}

/// Committed pages for a built-in DLL's own use, readable and writable
/// (a DIB section's bits): their address, or None when there is no room.
pub fn alloc_pages(cpu: &mut Cpu, size: u32) -> Option<u32> {
    let at = virtual_memory::virtual_alloc(cpu, &[0, size, 0x3000, 4])?;
    (at != 0).then_some(at)
}

/// Free the pages `alloc_pages` gave.
pub fn free_pages(cpu: &mut Cpu, at: u32) {
    virtual_memory::virtual_free(cpu, &[at, 0, 0x8000]);
}

/// A thread of the running process for a built-in DLL's own work, as
/// CreateThread(NULL, 0, start, 0, 0, NULL) makes it, its handle closed:
/// its id, or None when there is no room for it.
pub fn create_system_thread(cpu: &mut Cpu, start: u32) -> Option<u32> {
    let handle = threads::create_thread(cpu, &[0, 0, start, 0, 0, 0])?;
    if handle == 0 {
        return None;
    }
    let id = threads::thread_of(cpu, handle).map(|(p, t)| cpu.bus.win32.processes[p].threads[t].id);
    super::object::close(cpu, handle);
    id
}

/// A file for a built-in DLL (WINMM's multimedia files), opened as
/// CreateFileA opens it with `access`, `share` and `disposition`: its
/// handle, or the error.
pub fn open_handle(
    cpu: &mut Cpu,
    name: &str,
    access: u32,
    share: u32,
    disposition: u32,
) -> Result<u32, u32> {
    let (sft, _) = file::open_file(cpu, name, access, share, disposition)?;
    Ok(file::new_file(cpu, sft, access, share))
}

/// Up to `n` bytes of the file `handle` from its pointer, or the error.
pub fn read_file(cpu: &mut Cpu, handle: u32, n: u32) -> Result<Vec<u8>, u32> {
    let open = file::open_of(cpu, handle).ok_or(ERROR_INVALID_HANDLE)?;
    if open.access & file::GENERIC_READ == 0 {
        return Err(ERROR_ACCESS_DENIED);
    }
    cpu.bus
        .disk
        .read_file(open.sft, n as usize)
        .map_err(|e| e as u32)
}

/// Write `data` to the file `handle` at its pointer: the count written,
/// or the error.
pub fn write_file(cpu: &mut Cpu, handle: u32, data: &[u8]) -> Result<u32, u32> {
    let open = file::open_of(cpu, handle).ok_or(ERROR_INVALID_HANDLE)?;
    if open.access & file::GENERIC_WRITE == 0 {
        return Err(ERROR_ACCESS_DENIED);
    }
    let (dos, sft) = (open.dos, open.sft);
    super::dos(cpu, |cpu| crate::interrupts::int21::write_open_file(cpu, dos, sft, data))
        .map(|n| n as u32)
        .map_err(|e| e as u32)
}

/// Move the pointer of the file `handle` by `offset` from the start (0),
/// the pointer (1) or the end (2): where it is then, or the error.
pub fn seek_file(cpu: &mut Cpu, handle: u32, offset: i64, method: u32) -> Result<u32, u32> {
    let open = file::open_of(cpu, handle).ok_or(ERROR_INVALID_HANDLE)?;
    let disk = &mut cpu.bus.disk;
    let base = match method {
        0 => 0,
        1 => disk.position(open.sft).unwrap_or(0) as i64,
        2 => disk.sft_entry(open.sft).map_or(0, |e| e.size) as i64,
        _ => return Err(ERROR_INVALID_PARAMETER),
    };
    let to = base
        .checked_add(offset)
        .filter(|&to| to >= 0)
        .ok_or(ERROR_INVALID_PARAMETER)?;
    disk.seek_file(open.sft, to, 0)
        .map(|at| at as u32)
        .map_err(|e| e as u32)
}

/// A Win32 path as DOS takes it (path.rs).
pub fn dos_path(name: &str) -> String {
    path::dos_path(name)
}

/// The full path of the file `name` where OpenFile looks for it: in the
/// directory it names, or else in those SearchPathA looks in.
pub fn search_file(cpu: &mut Cpu, name: &str) -> Option<String> {
    path::search(cpu, None, name)
}

/// A file read whole: its bytes, its DOS time and date, and its
/// attributes.
pub type FileData = (Vec<u8>, (u16, u16), u16);

/// The file at the Win32 path `name`, read whole, or the error.
pub fn read_path(cpu: &mut Cpu, name: &str) -> Result<FileData, u32> {
    let full = path::dos_path(&path::full_path(cpu, name));
    path::read_whole(cpu, &full)
}

/// Whether a handle CreateFileA opened has the file at the Win32 path
/// `name` open.
pub fn path_in_use(cpu: &mut Cpu, name: &str) -> bool {
    let full = path::dos_path(&path::full_path(cpu, name));
    path::open_sharing(cpu, &full).is_some()
}

/// The DOS path of the running program, as GetModuleFileNameA(NULL)
/// gives it.
pub fn program_path(cpu: &mut Cpu) -> Option<String> {
    let process = super::process(cpu);
    let base = process.image_base;
    process
        .modules
        .iter()
        .find(|m| m.base == base)
        .map(|m| m.path.clone())
}

/// The address of KERNEL32's function `name`, for a task to call.
pub fn address_of(name: &str) -> u32 {
    let index = APIS
        .iter()
        .position(|a| a.name == name)
        .unwrap_or_else(|| panic!("KERNEL32 has no {}", name));
    let dll = super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("KERNEL32 is a built-in DLL");
    super::stubs::stub_address(dll, index)
}

/// Path operations for other DLLs (SHELL32's file operations), on Win32
/// paths, each as the KERNEL32 function it names does it, with that
/// function's error.
pub mod files {
    use super::path;
    use crate::cpu::Cpu;
    pub use crate::win32::object::FindEntry;

    /// The full path of `name` as DOS takes it.
    pub fn full_path(cpu: &Cpu, name: &str) -> String {
        path::dos_path(&path::full_path(cpu, name))
    }

    /// CreateDirectoryA.
    pub fn create_directory(cpu: &mut Cpu, name: &str) -> Result<(), u32> {
        let full = full_path(cpu, name);
        path::make_directory(cpu, &full)
    }

    /// RemoveDirectoryA.
    pub fn remove_directory(cpu: &mut Cpu, name: &str) -> Result<(), u32> {
        let full = full_path(cpu, name);
        path::unmake_directory(cpu, &full)
    }

    /// DeleteFileA.
    pub fn delete_file(cpu: &mut Cpu, name: &str) -> Result<(), u32> {
        let full = full_path(cpu, name);
        path::delete(cpu, &full)
    }

    /// CopyFileA.
    pub fn copy_file(cpu: &mut Cpu, from: &str, to: &str, fail: bool) -> Result<(), u32> {
        let (from, to) = (full_path(cpu, from), full_path(cpu, to));
        path::copy_path(cpu, &from, &to, fail)
    }

    /// MoveFileA.
    pub fn move_file(cpu: &mut Cpu, from: &str, to: &str) -> Result<(), u32> {
        let (from, to) = (full_path(cpu, from), full_path(cpu, to));
        path::move_path(cpu, &from, &to)
    }

    /// GetFileAttributesA.
    pub fn attributes(cpu: &mut Cpu, name: &str) -> Result<u32, u32> {
        let full = full_path(cpu, name);
        path::attributes(cpu, &full)
    }

    /// SetFileAttributesA with DOS's attributes.
    pub fn set_attributes(cpu: &mut Cpu, name: &str, attr: u32) -> Result<(), u32> {
        let full = full_path(cpu, name);
        path::set_attributes(cpu, &full, attr)
    }

    /// The entries of the directory `dir` whose names match `pattern`,
    /// as FindFirstFileA finds them, "." and ".." among them.
    pub fn find(cpu: &Cpu, dir: &str, pattern: &str) -> Result<Vec<FindEntry>, u32> {
        let full = full_path(cpu, dir);
        path::find_entries(cpu, &full, pattern)
    }

    /// Write `data` as the whole file `name`, made anew.
    pub fn write_whole(cpu: &mut Cpu, name: &str, data: &[u8]) -> Result<(), u32> {
        let full = full_path(cpu, name);
        cpu.bus
            .disk
            .write_whole_file(&full, data, None)
            .map_err(|e| e as u32)
    }

    /// CreateProcessA for the command line `line`, its first window
    /// shown as `show` says: a handle of the new process, or the error.
    pub fn run(cpu: &mut Cpu, line: &str, show: u32) -> Result<u32, u32> {
        const STARTF_USESHOWWINDOW: u32 = 1;
        let mut startup = vec![0u8; super::process::STARTUPINFO_SIZE as usize];
        startup[44..48].copy_from_slice(&STARTF_USESHOWWINDOW.to_le_bytes());
        startup[48..50].copy_from_slice(&(show as u16).to_le_bytes());
        let [process, thread, _, _] =
            super::process::spawn(cpu, None, Some(line), None, false, 0, startup)?;
        crate::win32::object::close(cpu, thread);
        Ok(process)
    }

    /// CloseHandle.
    pub fn close(cpu: &mut Cpu, handle: u32) {
        crate::win32::object::close(cpu, handle);
    }

    /// SearchPathA without an extension: the full path of the file
    /// `name` in `dirs` (separated by semicolons), or where SearchPathA
    /// looks without them.
    pub fn search(cpu: &mut Cpu, dirs: Option<&str>, name: &str) -> Option<String> {
        path::search(cpu, dirs, name)
    }

    /// ExpandEnvironmentStringsA.
    pub fn expand(cpu: &mut Cpu, text: &str) -> String {
        let (out, _) = super::env::expand(cpu, &crate::dosstr::to_bytes(text));
        crate::dosstr::from_bytes(&out)
    }

    /// GetCurrentDirectoryA.
    pub fn current_directory(cpu: &Cpu) -> String {
        path::current_directory(cpu, cpu.bus.disk.get_current_drive())
    }

    /// SetCurrentDirectoryA.
    pub fn change_directory(cpu: &mut Cpu, name: &str) -> Result<(), u32> {
        path::change_directory(cpu, name)
    }
}

/// A writable view of the file mapping `handle` for a DIB section whose
/// bits are in it: the view's address and the mapping's size, or None
/// with the error set.
pub fn map_dib_view(cpu: &mut Cpu, handle: u32) -> Option<(u32, u32)> {
    mapping::dib_view(cpu, handle)
}

/// Let go of the view `map_dib_view` gave.
pub fn unmap_dib_view(cpu: &mut Cpu, at: u32) {
    mapping::unmap_view_of_file(cpu, &[at]);
}

/// The data of the resource `name` of type `kind` in the module `module`
/// (0: the program), as FindResourceA takes them: its address and size,
/// or the error. None if a name's page isn't there.
pub fn resource_data(
    cpu: &mut Cpu,
    module: u32,
    kind: u32,
    name: u32,
) -> Option<Result<(u32, u32), u32>> {
    module::resource_data(cpu, module, kind, name)
}

/// GlobalAlloc(flags, size) for a built-in DLL (the clipboard's data):
/// the handle, or 0.
pub fn global_alloc(cpu: &mut Cpu, flags: u32, size: u32) -> u32 {
    global::global_alloc(cpu, &[flags, size]).unwrap_or(0)
}

/// GlobalSize(handle): the size of its block, or 0.
pub fn global_size(cpu: &mut Cpu, handle: u32) -> u32 {
    global::global_size(cpu, &[handle]).unwrap_or(0)
}

/// GlobalReAlloc(handle, size, flags) for a built-in DLL: the handle,
/// or 0.
pub fn global_realloc(cpu: &mut Cpu, handle: u32, size: u32, flags: u32) -> u32 {
    global::global_realloc(cpu, &[handle, size, flags]).unwrap_or(0)
}

/// GlobalFree(handle) for a built-in DLL.
pub fn global_free(cpu: &mut Cpu, handle: u32) {
    global::global_free(cpu, &[handle]);
}

/// ANSI text in Unicode, by the ANSI code page.
pub fn ansi_to_wide(bytes: &[u8]) -> Vec<u16> {
    text::ansi_to_wide(bytes)
}

/// lstrcmpiA's comparison of two ANSI strings, without their zeros.
pub fn compare_nocase(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let (a, b) = (text::ansi_to_wide(a), text::ansi_to_wide(b));
    sort::compare(sort::NORM_IGNORECASE, &a, &b)
}

/// The address of the memory of the global handle `handle`, as
/// GlobalLock gives it (without locking it), or 0.
pub fn global_address(cpu: &mut Cpu, handle: u32) -> u32 {
    let at = global::global_lock(cpu, &[handle]).unwrap_or(0);
    if at != 0 {
        global::global_unlock(cpu, &[handle]);
    }
    at
}

/// The file named by the string at `name`, read into pages
/// `alloc_pages` gives: their address and the file's size, which the
/// caller frees with `free_pages`. Some(None) when it can't be read, with
/// the error set; None if a page of the name isn't there.
pub fn read_whole_file(cpu: &mut Cpu, name: u32) -> Option<Option<(u32, u32)>> {
    const GENERIC_READ: u32 = 0x8000_0000;
    const OPEN_EXISTING: u32 = 3;
    let handle = file::create_file(cpu, &[name, GENERIC_READ, 1, 0, OPEN_EXISTING, 0, 0])?;
    if handle == 0xFFFF_FFFF {
        return Some(None);
    }
    let size = file::get_file_size(cpu, &[handle, 0]).unwrap_or(0xFFFF_FFFF);
    let pages = if size == 0xFFFF_FFFF {
        None
    } else {
        alloc_pages(cpu, size + 4)
    };
    let mut result = None;
    if let Some(at) = pages {
        let read = file::read_file(cpu, &[handle, at, size, at + size, 0]).unwrap_or(0);
        if read != 0 && cpu.bus.guest_read_32(at + size) == size {
            result = Some((at, size));
        } else {
            free_pages(cpu, at);
        }
    }
    threads::close_handle(cpu, &[handle]);
    Some(result)
}

/// Where the main thread starts when DLLs' entry points run first.
pub fn process_start_address() -> u32 {
    let index = APIS.iter().position(|a| a.name == "ProcessStart");
    super::stubs::stub_address(0, index.expect("ProcessStart"))
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
const ERROR_ALREADY_EXISTS: u32 = 183;
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

/// `not_implemented` for a function that fails with -1:
/// INVALID_HANDLE_VALUE, INVALID_FILE_ATTRIBUTES.
fn not_implemented_handle(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    set_error(cpu, ERROR_CALL_NOT_IMPLEMENTED);
    Some(INVALID_HANDLE_VALUE)
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
    module::exit_thread(cpu, code);
    None
}

/// ExitProcess: the DLLs learn that the process ends, then it does.
fn exit_process(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    module::exit_process(cpu, args[0]);
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

fn get_command_line(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(super::process(cpu).command_line)
}

/// GetCommandLineW, which Windows 95 has: the command line in Unicode,
/// in the page after the ANSI one.
fn get_command_line_w(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(super::process(cpu).command_line + super::memory::PAGE)
}

/// Windows 95: version 4.0, with bit 31 saying it isn't NT.
fn get_version(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0xC000_0004)
}

/// The module named `name` (with ".DLL" when it has no extension), or
/// with NULL the program's. Windows 95 knows modules by their file
/// names: a directory in `name` doesn't matter.
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
