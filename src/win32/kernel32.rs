//! KERNEL32.DLL: the process, its console and the table of functions;
//! the other parts in the modules below.

pub(super) mod atom;
pub(super) mod change;
mod console;
mod disk;
mod env;
mod file;
mod global;
mod locale;
pub(crate) mod lz;
mod mapping;
mod memory;
mod message;
pub(super) mod module;
pub(crate) mod path;
mod process;
pub use process::ProgramInLine;
mod profile;
pub(in crate::win32) use profile::profile_int;
pub(crate) use profile::profile_value;
mod sort;
mod sync;
mod text;
mod threads;
pub(crate) mod time;
mod win98;
pub use win98::is_directory;
pub(crate) mod virtual_memory;
mod vxd;

pub(super) use locale::enum_returned;
pub(crate) use path::{change_directory, current_directory, find_entries, full_path, matches};
pub(crate) use process::{spawn, startup_show, startup_showing};
pub(super) use process::{STARTUPINFO_SIZE, win_exec_step};
pub(super) use file::{
    CREATE_ALWAYS, CREATE_NEW, FILE_SHARE_READ, FILE_SHARE_WRITE, GENERIC_READ, GENERIC_WRITE,
    OPEN_ALWAYS, OPEN_EXISTING, TRUNCATE_EXISTING, new_file, open_file, open_of,
};

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;
use crate::win16::flat;

/// Windows 95's KERNEL32 is at BFF70000h, which programs and their copy
/// protection check to tell Windows 9x from NT.
pub static DLL: Dll = Dll {
    name: "KERNEL32.DLL",
    base: 0xBFF7_0000,
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
static APIS: &[Api] = &[
    // Where the program's entry point returns to: it ends with the
    // value it returned.
    stdcall("ProcessReturn", 0, 0, process_return),
    stdcall("ExitProcess", 248, 1, exit_process),
    stdcall("GetStdHandle", 443, 1, get_std_handle),
    stdcall("WriteFile", 822, 5, file::write_file),
    stdcall("WriteConsoleA", 813, 5, file::write_file),
    stdcall("GetCommandLineA", 329, 0, get_command_line),
    stdcall("GetVersion", 476, 0, get_version),
    stdcall("GetModuleHandleA", 397, 1, get_module_handle),
    stdcall("GetLastError", 385, 0, get_last_error),
    stdcall("SetLastError", 724, 1, set_last_error),
    stdcall("GetTickCount", 470, 0, get_tick_count),
    stdcall("GetEnvironmentVariableA", 365, 3, get_environment_variable),
    stdcall("SetEnvironmentVariableA", 711, 2, set_environment_variable),
    stdcall("TlsAlloc", 770, 0, memory::tls_alloc),
    stdcall("TlsFree", 772, 1, memory::tls_free),
    stdcall("TlsGetValue", 774, 1, memory::tls_get_value),
    stdcall("TlsSetValue", 775, 2, memory::tls_set_value),
    stdcall("GetProcessHeap", 422, 0, memory::get_process_heap),
    stdcall("HeapAlloc", 509, 3, memory::heap_alloc),
    stdcall("HeapFree", 513, 3, memory::heap_free),
    stdcall("HeapReAlloc", 515, 4, memory::heap_realloc),
    stdcall("HeapSize", 517, 3, memory::heap_size),
    stdcall("InterlockedIncrement", 528, 1, memory::interlocked_increment),
    stdcall("InterlockedDecrement", 525, 1, memory::interlocked_decrement),
    stdcall("InterlockedExchange", 526, 2, memory::interlocked_exchange),
    stdcall("GetProcAddress", 419, 2, module::get_proc_address),
    stdcall("GetVersionExA", 477, 1, module::get_version_ex),
    stdcall("GetLogicalDrives", 391, 0, disk::get_logical_drives),
    stdcall("GetDriveTypeA", 360, 1, disk::get_drive_type),
    stdcall("GetDriveTypeW", 361, 1, not_implemented),
    stdcall("GetDiskFreeSpaceA", 356, 5, disk::get_disk_free_space),
    stdcall("GetDiskFreeSpaceW", 359, 5, not_implemented),
    stdcall("GetSystemTime", 454, 1, time::get_system_time),
    stdcall("GetLocalTime", 386, 1, time::get_system_time),
    stdcall(
        "SystemTimeToFileTime",
        763,
        2,
        time::system_time_to_file_time,
    ),
    stdcall(
        "FileTimeToSystemTime",
        274,
        2,
        time::file_time_to_system_time,
    ),
    stdcall("FileTimeToLocalFileTime", 273, 2, time::copy_file_time),
    stdcall("LocalFileTimeToFileTime", 563, 2, time::copy_file_time),
    stdcall(
        "DosDateTimeToFileTime",
        221,
        3,
        time::dos_date_time_to_file_time,
    ),
    stdcall(
        "FileTimeToDosDateTime",
        272,
        3,
        time::file_time_to_dos_date_time,
    ),
    stdcall(
        "GetTimeZoneInformation",
        473,
        1,
        time::get_time_zone_information,
    ),
    stdcall("WideCharToMultiByte", 811, 8, text::wide_char_to_multi_byte),
    stdcall("MultiByteToWideChar", 594, 6, text::multi_byte_to_wide_char),
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
    stdcall("CreateThread", 205, 6, threads::create_thread),
    stdcall("ExitThread", 249, 1, threads::exit_thread),
    stdcall("GetCurrentThread", 348, 0, threads::get_current_thread),
    stdcall("GetCurrentThreadId", 349, 0, threads::get_current_thread_id),
    stdcall("GetCurrentProcess", 346, 0, threads::get_current_process),
    stdcall("GetCurrentProcessId", 347, 0, threads::get_current_process_id),
    stdcall("GetExitCodeThread", 369, 2, threads::get_exit_code_thread),
    stdcall("GetExitCodeProcess", 368, 2, threads::get_exit_code_process),
    stdcall("TerminateThread", 766, 2, threads::terminate_thread),
    stdcall("TerminateProcess", 765, 2, threads::terminate_process),
    stdcall("SuspendThread", 760, 1, threads::suspend_thread),
    stdcall("ResumeThread", 654, 1, threads::resume_thread),
    stdcall("GetThreadPriority", 466, 1, threads::get_thread_priority),
    stdcall("SetThreadPriority", 747, 2, threads::set_thread_priority),
    stdcall("GetThreadContext", 464, 2, threads::get_thread_context),
    stdcall("SetThreadContext", 743, 2, threads::set_thread_context),
    stdcall("CloseHandle", 160, 1, threads::close_handle),
    stdcall("DuplicateHandle", 222, 7, threads::duplicate_handle),
    stdcall("CreateEventA", 182, 4, sync::create_event),
    stdcall("OpenEventA", 596, 3, sync::open_event),
    stdcall("SetEvent", 714, 1, sync::set_event),
    stdcall("ResetEvent", 651, 1, sync::reset_event),
    stdcall("PulseEvent", 559, 1, sync::pulse_event),
    stdcall("CreateMutexA", 193, 3, sync::create_mutex),
    stdcall("OpenMutexA", 601, 3, sync::open_mutex),
    stdcall("ReleaseMutex", 645, 1, sync::release_mutex),
    stdcall("CreateSemaphoreA", 201, 4, sync::create_semaphore),
    stdcall("OpenSemaphoreA", 605, 3, sync::open_semaphore),
    stdcall("ReleaseSemaphore", 646, 3, sync::release_semaphore),
    stdcall("WaitForSingleObject", 807, 2, sync::wait_for_single_object),
    stdcall("WaitForSingleObjectEx", 808, 3, sync::wait_for_single_object),
    stdcall(
        "WaitForMultipleObjects",
        805,
        4,
        sync::wait_for_multiple_objects,
    ),
    stdcall(
        "WaitForMultipleObjectsEx",
        806,
        5,
        sync::wait_for_multiple_objects,
    ),
    stdcall("Sleep", 758, 1, sync::sleep),
    stdcall("SleepEx", 759, 2, sync::sleep),
    stdcall(
        "InitializeCriticalSection",
        522,
        1,
        sync::initialize_critical_section,
    ),
    stdcall("DeleteCriticalSection", 214, 1, sync::delete_critical_section),
    stdcall("EnterCriticalSection", 225, 1, sync::enter_critical_section),
    stdcall("LeaveCriticalSection", 552, 1, sync::leave_critical_section),
    stdcall(
        "TryEnterCriticalSection",
        779,
        1,
        sync::try_enter_critical_section,
    ),
    stdcall("RaiseException", 627, 4, super::seh::raise_exception),
    stdcall("RtlUnwind", 657, 4, super::seh::rtl_unwind),
    stdcall(
        "UnhandledExceptionFilter",
        784,
        1,
        super::seh::unhandled_exception_filter,
    ),
    stdcall(
        "SetUnhandledExceptionFilter",
        750,
        1,
        super::seh::set_unhandled_exception_filter,
    ),
    stdcall("CreateProcessA", 198, 10, process::create_process_a),
    stdcall("OpenProcess", 603, 3, process::open_process),
    stdcall("GetStartupInfoA", 441, 1, process::get_startup_info),
    stdcall("CreateRemoteThread", 200, 7, not_implemented),
    stdcall("GetThreadTimes", 469, 5, not_implemented),
    stdcall("GetProcessTimes", 426, 5, not_implemented),
    stdcall(
        "GetProcessAffinityMask",
        420,
        3,
        process::get_process_affinity_mask,
    ),
    stdcall(
        "SetThreadAffinityMask",
        742,
        2,
        process::set_thread_affinity_mask,
    ),
    stdcall("GetPriorityClass", 408, 1, process::get_priority_class),
    stdcall("SetPriorityClass", 731, 2, process::set_priority_class),
    stdcall("ReadProcessMemory", 641, 5, process::read_process_memory),
    stdcall("WriteProcessMemory", 831, 5, process::write_process_memory),
    stdcall(
        "FlushInstructionCache",
        297,
        3,
        process::flush_instruction_cache,
    ),
    stdcall("SetErrorMode", 713, 1, process::set_error_mode),
    stdcall("GetProcessVersion", 427, 1, process::get_process_version),
    stdcall("FatalAppExitA", 269, 2, process::fatal_app_exit),
    stdcall("FatalExit", 271, 1, process::fatal_exit),
    stdcall("CreateEventW", 183, 4, not_implemented),
    stdcall("CreateMutexW", 194, 3, not_implemented),
    stdcall("CreateSemaphoreW", 202, 4, not_implemented),
    stdcall("OpenEventW", 597, 3, not_implemented),
    stdcall("OpenMutexW", 602, 3, not_implemented),
    stdcall("OpenSemaphoreW", 606, 3, not_implemented),

    stdcall("VirtualAlloc", 793, 4, virtual_memory::virtual_alloc),
    stdcall("VirtualFree", 795, 3, virtual_memory::virtual_free),
    stdcall("VirtualProtect", 798, 4, virtual_memory::virtual_protect),
    stdcall("VirtualProtectEx", 799, 5, virtual_memory::virtual_protect_ex),
    stdcall("VirtualQuery", 800, 3, virtual_memory::virtual_query),
    stdcall("VirtualQueryEx", 801, 4, virtual_memory::virtual_query_ex),
    stdcall("VirtualLock", 797, 2, virtual_memory::virtual_lock),
    stdcall("VirtualUnlock", 802, 2, virtual_memory::virtual_lock),
    stdcall("HeapCreate", 511, 3, memory::heap_create),
    stdcall("HeapDestroy", 512, 1, memory::heap_destroy),
    stdcall("HeapValidate", 519, 3, memory::heap_validate),
    stdcall("HeapCompact", 510, 2, memory::heap_compact),
    stdcall("HeapLock", 514, 1, memory::heap_lock),
    stdcall("HeapUnlock", 518, 1, memory::heap_lock),
    stdcall("HeapWalk", 520, 2, not_implemented),
    stdcall("GetProcessHeaps", 423, 2, memory::get_process_heaps),
    stdcall("GlobalAlloc", 486, 2, global::global_alloc),
    stdcall("GlobalFree", 493, 1, global::global_free),
    stdcall("GlobalReAlloc", 499, 3, global::global_realloc),
    stdcall("GlobalSize", 500, 1, global::global_size),
    stdcall("GlobalLock", 497, 1, global::global_lock),
    stdcall("GlobalUnlock", 503, 1, global::global_unlock),
    stdcall("GlobalHandle", 496, 1, global::global_handle),
    stdcall("GlobalFlags", 492, 1, global::global_flags),
    stdcall("GlobalMemoryStatus", 498, 1, global::global_memory_status),
    stdcall("GlobalCompact", 487, 1, global::global_compact),
    stdcall("GlobalFix", 491, 1, global::global_fix),
    stdcall("GlobalUnfix", 502, 1, global::global_unfix),
    stdcall("GlobalWire", 504, 1, global::global_fix),
    stdcall("GlobalUnWire", 501, 1, global::global_unfix),
    stdcall("LocalAlloc", 560, 2, global::global_alloc),
    stdcall("LocalFree", 565, 1, global::global_free),
    stdcall("LocalReAlloc", 568, 3, global::global_realloc),
    stdcall("LocalSize", 570, 1, global::global_size),
    stdcall("LocalLock", 567, 1, global::global_lock),
    stdcall("LocalUnlock", 571, 1, global::global_unlock),
    stdcall("LocalHandle", 566, 1, global::global_handle),
    stdcall("LocalFlags", 564, 1, global::global_flags),
    stdcall("LocalCompact", 562, 1, global::global_compact),
    stdcall("LocalShrink", 569, 2, global::local_shrink),
    stdcall("IsBadReadPtr", 533, 2, virtual_memory::is_bad_read_ptr),
    stdcall("IsBadWritePtr", 536, 2, virtual_memory::is_bad_write_ptr),
    stdcall("IsBadCodePtr", 530, 1, virtual_memory::is_bad_code_ptr),
    stdcall("IsBadStringPtrA", 534, 2, virtual_memory::is_bad_string_ptr_a),
    stdcall("IsBadStringPtrW", 535, 2, virtual_memory::is_bad_string_ptr_w),
    stdcall("IsBadHugeReadPtr", 531, 2, virtual_memory::is_bad_read_ptr),
    stdcall("IsBadHugeWritePtr", 532, 2, virtual_memory::is_bad_write_ptr),
    stdcall("RtlMoveMemory", 656, 3, virtual_memory::rtl_move_memory),
    stdcall("RtlFillMemory", 655, 3, virtual_memory::rtl_fill_memory),
    stdcall("RtlZeroMemory", 658, 2, virtual_memory::rtl_zero_memory),
    stdcall("GetSystemInfo", 452, 1, virtual_memory::get_system_info),
    stdcall("CreateFileMappingA", 186, 6, mapping::create_file_mapping_a),
    stdcall("CreateFileMappingW", 187, 6, not_implemented),
    stdcall("OpenFileMappingA", 599, 3, mapping::open_file_mapping_a),
    stdcall("OpenFileMappingW", 600, 3, not_implemented),
    stdcall("MapViewOfFile", 585, 5, mapping::map_view_of_file),
    stdcall("MapViewOfFileEx", 586, 6, mapping::map_view_of_file_ex),
    stdcall("UnmapViewOfFile", 788, 1, mapping::unmap_view_of_file),
    stdcall("FlushViewOfFile", 298, 2, mapping::flush_view_of_file),

    // Files, directories, paths and the console.
    stdcall("CreateFileA", 185, 7, file::create_file),
    stdcall("CreateFileW", 188, 7, not_implemented_handle),
    stdcall("ReadFile", 638, 5, file::read_file),
    stdcall("SetFilePointer", 719, 4, file::set_file_pointer),
    stdcall("GetFileSize", 375, 2, file::get_file_size),
    stdcall("GetFileType", 377, 1, file::get_file_type),
    stdcall("FlushFileBuffers", 296, 1, file::flush_file_buffers),
    stdcall("SetEndOfFile", 710, 1, file::set_end_of_file),
    stdcall("GetFileInformationByHandle", 374, 2, file::get_file_information_by_handle),
    stdcall("GetFileTime", 376, 4, file::get_file_time),
    stdcall("SetFileTime", 720, 4, file::set_file_time),
    stdcall("LockFile", 572, 5, file::lock_file),
    stdcall("UnlockFile", 786, 5, file::unlock_file),
    stdcall("LockFileEx", 573, 6, file::lock_file_ex),
    stdcall("UnlockFileEx", 787, 5, file::lock_file_ex),
    stdcall("GetOverlappedResult", 407, 4, file::get_overlapped_result),
    stdcall("ReadFileEx", 639, 5, file::file_ex),
    stdcall("WriteFileEx", 823, 5, file::file_ex),
    stdcall("_lopen", 844, 2, file::lopen),
    stdcall("_lcreat", 842, 2, file::lcreat),
    stdcall("_lclose", 841, 1, file::lclose),
    stdcall("_lread", 845, 3, file::lread),
    stdcall("_hread", 839, 3, file::lread),
    stdcall("_lwrite", 846, 3, file::lwrite),
    stdcall("_hwrite", 840, 3, file::lwrite),
    stdcall("_llseek", 843, 3, file::llseek),
    stdcall("OpenFile", 598, 3, file::open_file_16),
    stdcall("SetHandleCount", 722, 1, file::set_handle_count),
    stdcall("DeleteFileA", 216, 1, path::delete_file),
    stdcall("DeleteFileW", 217, 1, not_implemented),
    stdcall("MoveFileA", 589, 2, path::move_file),
    stdcall("MoveFileW", 592, 2, not_implemented),
    stdcall("CopyFileA", 173, 3, path::copy_file),
    stdcall("CopyFileW", 176, 3, not_implemented),
    stdcall("GetFileAttributesA", 370, 1, path::get_file_attributes),
    stdcall("GetFileAttributesW", 373, 1, not_implemented_handle),
    stdcall("SetFileAttributesA", 717, 2, path::set_file_attributes),
    stdcall("SetFileAttributesW", 718, 2, not_implemented),
    stdcall("FindFirstFileA", 284, 2, path::find_first_file),
    stdcall("FindFirstFileW", 287, 2, not_implemented_handle),
    stdcall("FindNextFileA", 289, 2, path::find_next_file),
    stdcall("FindNextFileW", 290, 2, not_implemented),
    stdcall("FindClose", 280, 1, path::find_close),
    stdcall("CreateDirectoryA", 178, 2, path::create_directory),
    stdcall("CreateDirectoryW", 181, 2, not_implemented),
    stdcall("RemoveDirectoryA", 647, 1, path::remove_directory),
    stdcall("RemoveDirectoryW", 648, 1, not_implemented),
    stdcall("GetCurrentDirectoryA", 344, 2, path::get_current_directory),
    stdcall("GetCurrentDirectoryW", 345, 2, not_implemented),
    stdcall("SetCurrentDirectoryA", 705, 1, path::set_current_directory),
    stdcall("SetCurrentDirectoryW", 706, 1, not_implemented),
    stdcall("GetFullPathNameA", 378, 4, path::get_full_path_name),
    stdcall("GetFullPathNameW", 379, 4, not_implemented),
    stdcall("GetShortPathNameA", 439, 3, path::get_short_path_name),
    stdcall("GetShortPathNameW", 440, 3, not_implemented),
    stdcall("GetTempPathA", 462, 2, path::get_temp_path),
    stdcall("GetTempPathW", 463, 2, not_implemented),
    stdcall("GetTempFileNameA", 460, 4, path::get_temp_file_name),
    stdcall("GetTempFileNameW", 461, 4, not_implemented),
    stdcall("SearchPathA", 681, 6, path::search_path),
    stdcall("SearchPathW", 682, 6, not_implemented),
    stdcall("GetWindowsDirectoryA", 481, 2, path::get_windows_directory),
    stdcall("GetWindowsDirectoryW", 482, 2, not_implemented),
    stdcall("GetSystemDirectoryA", 450, 2, path::get_system_directory),
    stdcall("GetSystemDirectoryW", 451, 2, not_implemented),
    stdcall("ReadConsoleA", 628, 5, console::read_console),
    stdcall("GetConsoleMode", 337, 2, console::get_console_mode),
    stdcall("SetConsoleMode", 697, 2, console::set_console_mode),
    stdcall("SetConsoleCtrlHandler", 694, 2, console::set_console_ctrl_handler),
    stdcall("GetConsoleScreenBufferInfo", 339, 2, console::get_console_screen_buffer_info),
    stdcall("SetConsoleCursorPosition", 696, 2, console::set_console_cursor_position),
    stdcall("FillConsoleOutputCharacterA", 276, 5, console::fill_console_output_character),
    stdcall("FillConsoleOutputAttribute", 275, 5, console::fill_console_output_attribute),
    stdcall("WriteConsoleOutputA", 816, 5, console::write_console_output),
    stdcall("SetConsoleTextAttribute", 700, 2, console::set_console_text_attribute),
    stdcall("AllocConsole", 123, 0, console::alloc_console),
    stdcall("FreeConsole", 303, 0, console::free_console),
    stdcall("GetConsoleCP", 335, 0, console::get_console_cp),
    stdcall("GetConsoleOutputCP", 338, 0, console::get_console_cp),
    stdcall("SetStdHandle", 736, 2, console::set_std_handle),
    stdcall("GetNumberOfConsoleInputEvents", 404, 2, console::get_number_of_console_input_events),
    stdcall("PeekConsoleInputA", 612, 4, console::peek_console_input),
    stdcall("ReadConsoleInputA", 629, 4, console::read_console_input),
    stdcall("FlushConsoleInputBuffer", 295, 1, console::flush_console_input_buffer),
    stdcall("SetConsoleTitleA", 701, 1, console::set_console_title),

    // Where the main thread starts when DLLs that came with the program
    // have entry points, which run before the program's.
    stdcall("ProcessStart", 0, 0, module::process_start),
    stdcall("LoadLibraryA", 553, 1, module::load_library),
    stdcall("LoadLibraryExA", 554, 3, module::load_library_ex),
    stdcall("FreeLibrary", 307, 1, module::free_library),
    stdcall(
        "FreeLibraryAndExitThread",
        308,
        2,
        module::free_library_and_exit_thread,
    ),
    stdcall("GetModuleFileNameA", 395, 3, module::get_module_file_name),
    stdcall("GetModuleFileNameW", 396, 3, not_implemented),
    stdcall(
        "DisableThreadLibraryCalls",
        219,
        1,
        module::disable_thread_library_calls,
    ),
    stdcall("FindResourceA", 291, 3, module::find_resource),
    stdcall("FindResourceExA", 292, 4, module::find_resource_ex),
    stdcall("LoadResource", 558, 2, module::load_resource),
    stdcall("LockResource", 574, 1, module::lock_resource),
    stdcall("SizeofResource", 757, 2, module::sizeof_resource),
    stdcall("FreeResource", 309, 1, module::free_resource),
    stdcall("EnumResourceTypesA", 238, 3, module::enum_resource_types),
    stdcall("EnumResourceNamesA", 236, 4, module::enum_resource_names),
    stdcall("LoadLibraryW", 556, 1, not_implemented),
    stdcall("LoadLibraryExW", 555, 3, not_implemented),
    stdcall("GetModuleHandleW", 398, 1, not_implemented),

    stdcall("lstrlenA", 864, 1, text::lstrlen_a),
    stdcall("lstrcpyA", 858, 2, text::lstrcpy_a),
    stdcall("lstrcpynA", 861, 3, text::lstrcpyn_a),
    stdcall("lstrcatA", 849, 2, text::lstrcat_a),
    stdcall("lstrcmpA", 852, 2, text::lstrcmp_a),
    stdcall("lstrcmpiA", 855, 2, text::lstrcmpi_a),
    stdcall("lstrlenW", 865, 1, text::lstrlen_w),
    stdcall("lstrcpyW", 859, 2, not_implemented),
    stdcall("lstrcpynW", 862, 3, not_implemented),
    stdcall("lstrcatW", 850, 2, not_implemented),
    stdcall("lstrcmpW", 853, 2, not_implemented),
    stdcall("lstrcmpiW", 856, 2, not_implemented),
    stdcall("GetACP", 312, 0, text::get_acp),
    stdcall("GetOEMCP", 406, 0, text::get_oem_cp),
    stdcall("GetCPInfo", 318, 2, text::get_cp_info),
    stdcall("IsValidCodePage", 544, 1, text::is_valid_code_page),
    stdcall("IsDBCSLeadByte", 537, 1, text::is_dbcs_lead_byte),
    stdcall("IsDBCSLeadByteEx", 538, 2, text::is_dbcs_lead_byte_ex),
    stdcall("GetStringTypeA", 444, 5, sort::get_string_type_a),
    stdcall("GetStringTypeExA", 445, 5, sort::get_string_type_a),
    stdcall("GetStringTypeW", 447, 4, not_implemented),
    stdcall("GetStringTypeExW", 446, 5, not_implemented),
    stdcall("CompareStringA", 166, 6, sort::compare_string_a),
    stdcall("CompareStringW", 167, 6, not_implemented),
    stdcall("LCMapStringA", 550, 6, sort::lc_map_string_a),
    stdcall("LCMapStringW", 551, 6, not_implemented),
    stdcall("GetLocaleInfoA", 387, 4, locale::get_locale_info_a),
    stdcall("GetLocaleInfoW", 388, 4, not_implemented),
    stdcall("SetLocaleInfoA", 726, 3, locale::set_locale_info_a),
    stdcall("GetUserDefaultLCID", 474, 0, locale::get_default_lcid),
    stdcall("GetSystemDefaultLCID", 448, 0, locale::get_default_lcid),
    stdcall("GetUserDefaultLangID", 475, 0, locale::get_default_lang_id),
    stdcall("GetSystemDefaultLangID", 449, 0, locale::get_default_lang_id),
    stdcall("GetThreadLocale", 465, 0, locale::get_default_lcid),
    stdcall("SetThreadLocale", 746, 1, locale::set_thread_locale),
    stdcall("IsValidLocale", 545, 2, locale::is_valid_locale),
    stdcall("ConvertDefaultLocale", 170, 1, locale::convert_default_locale),
    stdcall("EnumSystemLocalesA", 242, 2, locale::enum_system_locales_a),
    stdcall("EnumDateFormatsA", 230, 3, locale::enum_date_formats_a),
    stdcall("EnumTimeFormatsA", 244, 3, locale::enum_time_formats_a),
    stdcall("GetDateFormatA", 350, 6, locale::get_date_format_a),
    stdcall("GetDateFormatW", 351, 6, not_implemented),
    stdcall("GetTimeFormatA", 471, 6, locale::get_time_format_a),
    stdcall("GetTimeFormatW", 472, 6, not_implemented),
    stdcall("GetNumberFormatA", 402, 6, locale::get_number_format_a),
    stdcall("GetNumberFormatW", 403, 6, not_implemented),
    stdcall("GetCurrencyFormatA", 342, 6, locale::get_currency_format_a),
    stdcall("GetCurrencyFormatW", 343, 6, not_implemented),
    stdcall("FormatMessageA", 301, 7, message::format_message_a),
    stdcall("FormatMessageW", 302, 7, not_implemented),
    stdcall("GetEnvironmentStrings", 362, 0, env::get_environment_strings),
    stdcall("GetEnvironmentStringsA", 363, 0, env::get_environment_strings),
    stdcall("GetEnvironmentStringsW", 364, 0, not_implemented),
    stdcall("FreeEnvironmentStringsA", 304, 1, env::free_environment_strings_a),
    stdcall("FreeEnvironmentStringsW", 305, 1, not_implemented),
    stdcall("ExpandEnvironmentStringsA", 250, 3, env::expand_environment_strings_a),
    stdcall("ExpandEnvironmentStringsW", 251, 3, not_implemented),
    stdcall("GetEnvironmentVariableW", 366, 3, not_implemented),
    stdcall("SetEnvironmentVariableW", 712, 2, not_implemented),
    stdcall("GetComputerNameA", 333, 2, env::get_computer_name_a),
    stdcall("GetComputerNameW", 334, 2, not_implemented),
    stdcall("SetComputerNameA", 690, 1, env::set_computer_name_a),
    stdcall("GetPrivateProfileStringA", 415, 6, profile::get_private_profile_string_a),
    stdcall("GetPrivateProfileStringW", 416, 6, not_implemented),
    stdcall("WritePrivateProfileStringA", 827, 4, profile::write_private_profile_string_a),
    stdcall("WritePrivateProfileStringW", 828, 4, not_implemented),
    stdcall("GetPrivateProfileIntA", 409, 4, profile::get_private_profile_int_a),
    stdcall("GetProfileIntA", 430, 3, profile::get_profile_int_a),
    stdcall("GetProfileStringA", 434, 5, profile::get_profile_string_a),
    stdcall("WriteProfileStringA", 834, 3, profile::write_profile_string_a),
    stdcall("GetPrivateProfileSectionA", 411, 4, profile::get_private_profile_section_a),
    stdcall("GetProfileSectionA", 432, 3, profile::get_profile_section_a),
    stdcall("GetPrivateProfileSectionNamesA", 412, 3, profile::get_private_profile_section_names_a),
    stdcall("GetPrivateProfileSectionNamesW", 413, 3, not_implemented),
    stdcall("WritePrivateProfileSectionA", 825, 3, profile::write_private_profile_section_a),
    stdcall("WriteProfileSectionA", 832, 2, profile::write_profile_section_a),
    stdcall("OutputDebugStringA", 610, 1, env::output_debug_string_a),
    stdcall("OutputDebugStringW", 611, 1, not_implemented),
    stdcall("Beep", 130, 2, env::beep),
    stdcall("GetSystemTimeAsFileTime", 456, 1, time::get_system_time_as_file_time),
    stdcall("QueryPerformanceCounter", 624, 1, env::query_performance_counter),
    stdcall("QueryPerformanceFrequency", 625, 1, env::query_performance_frequency),
    stdcall("CompareFileTime", 165, 2, time::compare_file_time),
    stdcall("SetSystemTime", 738, 1, time::set_system_time),
    stdcall("SetLocalTime", 725, 1, time::set_system_time),
    stdcall("MulDiv", 593, 3, env::mul_div),
    stdcall("GetCommandLineW", 330, 0, get_command_line_w),
    stdcall("GetProcessWorkingSetSize", 428, 3, not_implemented),
    stdcall("GetVersionExW", 478, 1, not_implemented),
    stdcall("GlobalAddAtomW", 485, 1, not_implemented),
    stdcall("CreateProcessW", 199, 10, not_implemented),
    stdcall("WinExec", 812, 2, process::win_exec),
    stdcall("GetDiskFreeSpaceExA", 357, 4, win98::get_disk_free_space_ex),
    stdcall("IsDebuggerPresent", 539, 0, win98::is_debugger_present),
    stdcall("InterlockedCompareExchange", 524, 3, win98::interlocked_compare_exchange),
    stdcall("InterlockedExchangeAdd", 527, 2, win98::interlocked_exchange_add),
    stdcall("GetFileAttributesExA", 371, 3, win98::get_file_attributes_ex),
    stdcall("GetLongPathNameA", 392, 3, win98::get_long_path_name),
    stdcall("GetPrivateProfileSectionW", 414, 4, profile::get_private_profile_section_w),
    stdcall("LZStart", 866, 0, lz::lz_start),
    stdcall("LZDone", 867, 0, lz::lz_done),
    stdcall("LZInit", 868, 1, lz::lz_init),
    stdcall("LZOpenFileA", 869, 3, lz::lz_open_file_a),
    stdcall("LZOpenFileW", 870, 3, lz::lz_open_file_w),
    stdcall("LZRead", 871, 3, lz::lz_read),
    stdcall("LZSeek", 872, 3, lz::lz_seek),
    stdcall("LZCopy", 873, 2, lz::lz_copy),
    stdcall("CopyLZFile", 874, 2, lz::lz_copy),
    stdcall("LZClose", 875, 1, lz::lz_close),
    stdcall("GetExpandedNameA", 876, 2, lz::get_expanded_name_a),
    stdcall("GetExpandedNameW", 877, 2, lz::lz_open_file_w),
    stdcall(
        "GetLogicalDriveStringsA",
        389,
        2,
        disk::get_logical_drive_strings,
    ),
    stdcall("GetLogicalDriveStringsW", 390, 2, not_implemented),
    // Win16 (src/win16/thunk.rs): where 32-bit code a 16-bit function
    // went on to returns, and the trap of the 32-bit slots that call
    // 16-bit procedures.
    stdcall("Win16Return", 878, 0, crate::win16::thunk::win16_return),
    stdcall("Win16Thunk", 879, 0, crate::win16::thunk::thunk32_trap),
    // Added after the Win16 slots, so the stubs above keep their
    // addresses.
    stdcall("GetVolumeInformationA", 479, 8, disk::get_volume_information),
    stdcall("GetVolumeInformationW", 480, 8, not_implemented),
    stdcall("GlobalAddAtomA", 484, 1, atom::global_add_atom),
    stdcall("GlobalFindAtomA", 489, 1, atom::global_find_atom),
    stdcall("GlobalDeleteAtom", 488, 1, atom::global_delete_atom),
    stdcall("GlobalGetAtomNameA", 494, 3, atom::global_get_atom_name),
    // Flat thunks (src/win16/flat.rs): where a Win32 process's task in
    // the Win16 process waits for its calls, and the functions the thunk
    // compiler's code calls.
    Api {
        name: "Win16Serve",
        ordinal: 0,
        args: 0,
        conv: Conv::Loop,
        handler: flat::serve,
    },
    stdcall("ThunkConnect32", 769, 6, flat::thunk_connect32),
    stdcall("QT_Thunk", 561, 0, flat::qt_thunk),
    stdcall("FT_Prolog", 267, 0, flat::ft_prolog),
    stdcall("MapLS", 582, 1, flat::map_ls_api),
    stdcall("UnMapLS", 782, 1, flat::unmap_ls_api),
    stdcall("MapSL", 583, 1, flat::map_sl_api),
    stdcall("MapSLFix", 584, 1, flat::map_sl_api),
    stdcall("UnMapSLFixArray", 783, 2, flat::unmap_sl_fix_array),
    stdcall("SMapLS", 659, 0, flat::smap_ls),
    stdcall("SUnMapLS", 669, 0, flat::sunmap_ls),
    stdcall("SMapLS_IP_EBP_8", 668, 0, flat::smap_ls_ebp_8),
    stdcall("SUnMapLS_IP_EBP_8", 678, 0, flat::sunmap_ls_ebp_8),
    stdcall("SMapLS_IP_EBP_12", 660, 0, flat::smap_ls_ebp_12),
    stdcall("SUnMapLS_IP_EBP_12", 670, 0, flat::sunmap_ls_ebp_12),
    stdcall("SMapLS_IP_EBP_16", 661, 0, flat::smap_ls_ebp_16),
    stdcall("SUnMapLS_IP_EBP_16", 671, 0, flat::sunmap_ls_ebp_16),
    stdcall("SMapLS_IP_EBP_20", 662, 0, flat::smap_ls_ebp_20),
    stdcall("SUnMapLS_IP_EBP_20", 672, 0, flat::sunmap_ls_ebp_20),
    stdcall("SMapLS_IP_EBP_24", 663, 0, flat::smap_ls_ebp_24),
    stdcall("SUnMapLS_IP_EBP_24", 673, 0, flat::sunmap_ls_ebp_24),
    stdcall("SMapLS_IP_EBP_28", 664, 0, flat::smap_ls_ebp_28),
    stdcall("SUnMapLS_IP_EBP_28", 674, 0, flat::sunmap_ls_ebp_28),
    stdcall("SMapLS_IP_EBP_32", 665, 0, flat::smap_ls_ebp_32),
    stdcall("SUnMapLS_IP_EBP_32", 675, 0, flat::sunmap_ls_ebp_32),
    stdcall("SMapLS_IP_EBP_36", 666, 0, flat::smap_ls_ebp_36),
    stdcall("SUnMapLS_IP_EBP_36", 676, 0, flat::sunmap_ls_ebp_36),
    stdcall("SMapLS_IP_EBP_40", 667, 0, flat::smap_ls_ebp_40),
    stdcall("SUnMapLS_IP_EBP_40", 677, 0, flat::sunmap_ls_ebp_40),
    // Windows 98's functions it exports by ordinal only, under those
    // ordinals: VxDCall0 to VxDCall8 (vxd.rs), and the Win16 loader's
    // LoadLibrary16, FreeLibrary16 and GetProcAddress16.
    stdcall("VxDCall0", 1, 1, vxd::vxd_call),
    stdcall("VxDCall1", 2, 2, vxd::vxd_call),
    stdcall("VxDCall2", 3, 3, vxd::vxd_call),
    stdcall("VxDCall3", 4, 4, vxd::vxd_call),
    stdcall("VxDCall4", 5, 5, vxd::vxd_call),
    stdcall("VxDCall5", 6, 6, vxd::vxd_call),
    stdcall("VxDCall6", 7, 7, vxd::vxd_call),
    stdcall("VxDCall7", 8, 8, vxd::vxd_call),
    stdcall("VxDCall8", 9, 9, vxd::vxd_call),
    stdcall("LoadLibrary16", 35, 1, flat::load_library16),
    stdcall("FreeLibrary16", 36, 1, flat::free_library16),
    stdcall("GetProcAddress16", 37, 2, flat::get_proc_address16),
    stdcall("SetProcessShutdownParameters", 734, 2, not_implemented),
    // A debugger's loop. No process here debugs another, so these fail,
    // but SafeDisc 2 resolves both through its own export walk and stops
    // with "Fatal error, code 12" where either is missing.
    stdcall("WaitForDebugEvent", 804, 2, not_implemented),
    stdcall("ContinueDebugEvent", 169, 3, not_implemented),
    // The handlers of the frames the exception dispatcher and RtlUnwind
    // put on the chain around each handler they call (seh.rs).
    Api {
        name: "NestedExceptionHandler",
        ordinal: 0,
        args: 4,
        conv: Conv::Cdecl,
        handler: super::seh::nested_exception_handler,
    },
    Api {
        name: "NestedUnwindHandler",
        ordinal: 0,
        args: 4,
        conv: Conv::Cdecl,
        handler: super::seh::nested_unwind_handler,
    },
    // The flat thunks' FT_Thunk and FT_ExitN (src/win16/flat.rs), and the
    // handler of the exception handler frame FT_Prolog makes.
    stdcall("FT_Thunk", 268, 0, flat::ft_thunk),
    stdcall("FT_Exit0", 252, 0, flat::ft_exit0),
    stdcall("FT_Exit4", 260, 0, flat::ft_exit4),
    stdcall("FT_Exit8", 266, 0, flat::ft_exit8),
    stdcall("FT_Exit12", 253, 0, flat::ft_exit12),
    stdcall("FT_Exit16", 254, 0, flat::ft_exit16),
    stdcall("FT_Exit20", 255, 0, flat::ft_exit20),
    stdcall("FT_Exit24", 256, 0, flat::ft_exit24),
    stdcall("FT_Exit28", 257, 0, flat::ft_exit28),
    stdcall("FT_Exit32", 258, 0, flat::ft_exit32),
    stdcall("FT_Exit36", 259, 0, flat::ft_exit36),
    stdcall("FT_Exit40", 261, 0, flat::ft_exit40),
    stdcall("FT_Exit44", 262, 0, flat::ft_exit44),
    stdcall("FT_Exit48", 263, 0, flat::ft_exit48),
    stdcall("FT_Exit52", 264, 0, flat::ft_exit52),
    stdcall("FT_Exit56", 265, 0, flat::ft_exit56),
    Api {
        name: "FlatThunkHandler",
        ordinal: 0,
        args: 4,
        conv: Conv::Cdecl,
        handler: flat::ft_handler,
    },
    // Windows 98 SE's is one of its stubs that fail with
    // ERROR_CALL_NOT_IMPLEMENTED and return 0, leaving the structure as
    // it was. SafeDisc 1's loader (Omikron's RUNTIME.EXE) resolves it
    // with the process functions it uses and exits with -1 where
    // GetProcAddress finds it missing.
    stdcall("GetStartupInfoW", 442, 1, not_implemented),
    // Another of Windows 98 SE's stubs: it fails with
    // ERROR_CALL_NOT_IMPLEMENTED and returns FALSE, and the SYSTEMTIME
    // it was to fill stays as it was. Myth II 1.8.3 imports it and
    // calls it for the date of each saved game it lists.
    stdcall("SystemTimeToTzSpecificLocalTime", 764, 3, not_implemented),
    stdcall("FindFirstChangeNotificationA", 282, 3, change::find_first),
    stdcall("FindNextChangeNotification", 288, 1, change::find_next),
    stdcall("FindCloseChangeNotification", 281, 1, change::find_close),
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

/// Let go of the file mappings no handle or view holds any more. Whether
/// that changed anything.
pub fn sweep_mappings(cpu: &mut Cpu) -> bool {
    mapping::sweep(cpu)
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
    crate::interrupts::int21::read_open_file(cpu, open.sft, n as usize).map_err(|e| e as u32)
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

/// The full path of the file `name` where SearchPathA looks for it: in
/// the directory it names if it starts with a drive, a backslash, ".\"
/// or "..\", or else in each of those SearchPathA looks in.
pub fn search_file(cpu: &mut Cpu, name: &str) -> Option<String> {
    path::search(cpu, None, name)
}

/// `search_file` for `name` with `extension` added if its file name has
/// none, as SearchPathA(NULL, name, extension, ...) looks for it.
pub fn search_file_with_extension(cpu: &mut Cpu, name: &str, extension: &str) -> Option<String> {
    let mut name = name.to_string();
    path::add_extension(&mut name, extension);
    path::search(cpu, None, &name)
}

/// The program the command line `line` names first, as CreateProcessA
/// finds it with no application name (process.rs).
pub fn program_in_line<'a>(cpu: &mut Cpu, line: &'a str) -> Option<ProgramInLine<'a>> {
    process::program_in_line(cpu, line)
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

/// Where 32-bit code that 16-bit code called returns to (Win16Return).
pub fn win16_return_address() -> u32 {
    address_of("Win16Return")
}

/// The trap id of the 32-bit slots that call 16-bit procedures
/// (Win16Thunk).
pub fn win16_thunk_id() -> u16 {
    let index = APIS
        .iter()
        .position(|a| a.name == "Win16Thunk")
        .expect("KERNEL32 has Win16Thunk");
    let dll = super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("KERNEL32 is a built-in DLL");
    super::stubs::id(dll, index)
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
        let startup = super::process::startup_showing(show as u16);
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

/// The data of the resource `name` of the named type `kind` ("WAVE"), as
/// `resource_data` gives it.
pub fn named_resource_data(
    cpu: &mut Cpu,
    module: u32,
    kind: &str,
    name: u32,
) -> Option<Result<(u32, u32), u32>> {
    module::named_resource_data(cpu, module, kind, name)
}

/// IsBadStringPtrA(at, max): whether the string at `at`, up to its 0 or
/// `max` bytes, can't be read. Unlike a service's own read, a page that
/// isn't there doesn't fault.
pub fn bad_string_ptr(cpu: &mut Cpu, at: u32, max: u32) -> bool {
    virtual_memory::is_bad_string_ptr_a(cpu, &[at, max]) != Some(0)
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

/// ANSI text in Unicode, by the system's ANSI code page.
pub fn ansi_to_wide(cpu: &Cpu, bytes: &[u8]) -> Vec<u16> {
    text::ansi_to_wide(cpu, bytes)
}

/// Unicode text in the system's ANSI code page, '?' for what it lacks.
pub fn wide_to_ansi(cpu: &Cpu, units: &[u16]) -> Vec<u8> {
    text::wide_to_ansi(cpu, units)
}

/// lstrcmpiA's comparison of two strings of the code page `page`,
/// without their zeros.
pub fn compare_nocase(
    page: crate::win32::nls::CodePage,
    a: &[u8],
    b: &[u8],
) -> std::cmp::Ordering {
    let (a, b) = (page.decode(a), page.decode(b));
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

/// The version in the low word (minor in the high byte), and the high
/// bits set for Windows 9x (version.rs).
fn get_version(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(super::version::info(super::version::current(cpu)).get_version)
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
    Some(crate::idle::tick_count(cpu))
}

/// The environment variable `name`'s place in the environment: names are
/// found whatever their case.
fn variable(process: &super::Process, name: &str) -> Option<usize> {
    process
        .environment
        .iter()
        .position(|s| super::split_variable(s).is_some_and(|(n, _)| n.eq_ignore_ascii_case(name)))
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
    let Some(value) = super::variable_value(&process.environment, &name) else {
        process.last_error = ERROR_ENVVAR_NOT_FOUND;
        return Some(0);
    };
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
/// or, as on Windows 95, an empty string remove it. A name can start with
/// "=" but not have one further on: MSVCRT's _chdir keeps each drive's
/// current directory as "=C:", and fails when it can't.
fn set_environment_variable(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = string_at(cpu, args[0])?;
    let value = match args[1] {
        0 => None,
        at => Some(string_at(cpu, at)?).filter(|v| !v.is_empty()),
    };
    // A name is one that split_variable can't split.
    if name.is_empty() || super::split_variable(&name).is_some() {
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
