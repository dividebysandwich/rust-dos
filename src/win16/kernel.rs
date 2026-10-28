//! KERNEL: memory (the global and local heaps, selectors), modules and
//! their resources, tasks, files through DOS, and the profile and
//! string functions, which go on to KERNEL32's.

use super::builtin::{Api16, Def, register, rust, value, win32};
use super::thunk::{self, EBP, EDI, ESI, ESP};
use super::{Frame, dos, dpmi, global, ldt, local, module, resource, task};
use crate::cpu::{Cpu, Seg};
use crate::dpmi::Context;

pub static MODULE: Def = Def {
    name: "KERNEL",
    file: "KRNL386.EXE",
    apis: APIS,
    library: false,
};

/// The functions. Append only: a function's place is its id's.
static APIS: &[Api16] = &[
    // The system's own stubs.
    register("CallReturn16", 0, call_return16),
    register("Thunk16", 0, thunk16),
    register("DefaultVector", 0, dos::default_vector),
    // Exported.
    rust("FatalExit", 1, "w", fatal_exit),
    rust("ExitKernel", 2, "", fatal_exit),
    rust("GetVersion", 3, "", get_version),
    rust("LocalInit", 4, "www", local_init),
    rust("LocalAlloc", 5, "ww", local_alloc),
    rust("LocalReAlloc", 6, "www", local_realloc),
    rust("LocalFree", 7, "w", local_free),
    rust("LocalLock", 8, "w", local_lock),
    rust("LocalUnlock", 9, "w", local_unlock),
    rust("LocalSize", 10, "w", local_size),
    rust("LocalHandle", 11, "w", local_handle),
    rust("LocalFlags", 12, "w", local_flags),
    rust("LocalCompact", 13, "w", local_compact),
    rust("LocalNotify", 14, "l", zero),
    rust("GlobalAlloc", 15, "wl", global_alloc),
    rust("GlobalReAlloc", 16, "wlw", global_realloc),
    rust("GlobalFree", 17, "w", global_free),
    rust("GlobalLock", 18, "w", global_lock),
    rust("GlobalUnlock", 19, "w", global_unlock),
    rust("GlobalSize", 20, "w", global_size),
    rust("GlobalHandle", 21, "w", global_handle),
    rust("GlobalFlags", 22, "w", global_flags),
    rust("LockSegment", 23, "w", lock_segment),
    rust("UnlockSegment", 24, "w", unlock_segment),
    rust("GlobalCompact", 25, "l", global_compact),
    rust("GlobalFreeAll", 26, "w", global_free_all),
    rust("Yield", 29, "", yield16),
    rust("WaitEvent", 30, "w", wait_event),
    rust("PostEvent", 31, "w", zero),
    rust("SetPriority", 32, "ws", zero),
    rust("LockCurrentTask", 33, "w", zero),
    rust("GetCurrentTask", 36, "", get_current_task),
    rust("GetCurrentPDB", 37, "", get_current_pdb),
    rust("LoadModule", 45, "pp", load_module),
    rust("FreeModule", 46, "w", free_library),
    rust("GetModuleHandle", 47, "S", get_module_handle),
    rust("GetModuleUsage", 48, "w", get_module_usage),
    rust("GetModuleFileName", 49, "wps", get_module_file_name),
    rust("GetProcAddress", 50, "wr", get_proc_address),
    rust("MakeProcInstance", 51, "Sw", make_proc_instance),
    rust("FreeProcInstance", 52, "S", zero),
    rust("GetInstanceData", 54, "www", get_instance_data),
    register("Catch", 55, catch),
    register("Throw", 56, throw),
    win32(
        "GetProfileInt",
        57,
        "pps",
        "KERNEL32.DLL",
        "GetProfileIntA",
        b'w',
    ),
    win32(
        "GetProfileString",
        58,
        "pppps",
        "KERNEL32.DLL",
        "GetProfileStringA",
        b'w',
    ),
    win32(
        "WriteProfileString",
        59,
        "ppp",
        "KERNEL32.DLL",
        "WriteProfileStringA",
        b'b',
    ),
    rust("FindResource", 60, "wrr", find_resource),
    rust("LoadResource", 61, "ww", load_resource),
    rust("LockResource", 62, "w", global_lock),
    rust("FreeResource", 63, "w", zero),
    rust("AccessResource", 64, "ww", access_resource),
    rust("SizeofResource", 65, "ww", sizeof_resource),
    rust("AllocResource", 66, "wwl", alloc_resource),
    rust("SetResourceHandler", 67, "wrS", zero),
    rust("InitAtomTable", 68, "w", one),
    rust("FindAtom", 69, "S", find_atom),
    rust("AddAtom", 70, "S", add_atom),
    rust("DeleteAtom", 71, "w", delete_atom),
    rust("GetAtomName", 72, "wpw", get_atom_name),
    rust("GetAtomHandle", 73, "w", get_atom_handle),
    rust("OpenFile", 74, "ppw", open_file),
    rust("_lclose", 81, "w", lclose),
    rust("_lread", 82, "wpw", lread),
    rust("_lcreat", 83, "pw", lcreat),
    rust("_llseek", 84, "wlw", llseek),
    rust("_lopen", 85, "pw", lopen),
    rust("_lwrite", 86, "wpw", lwrite),
    rust("lstrcmp", 87, "pp", lstrcmp),
    rust("lstrcpy", 88, "SS", lstrcpy),
    rust("lstrcat", 89, "SS", lstrcat),
    rust("lstrlen", 90, "p", lstrlen),
    register("InitTask", 91, task::init_task),
    rust("GetTempDrive", 92, "w", get_temp_drive),
    rust("GetCodeHandle", 93, "S", get_code_handle),
    rust("DefineHandleTable", 94, "w", one),
    rust("LoadLibrary", 95, "p", load_library),
    rust("FreeLibrary", 96, "w", free_library),
    rust("GetTempFileName", 97, "wpwp", get_temp_file_name),
    rust("GetLastDiskChange", 98, "", zero),
    register("DOS3Call", 102, dos::dos3call),
    rust("GetCodeInfo", 104, "Sp", get_code_info),
    rust("GetExeVersion", 105, "", get_exe_version),
    rust("SetSwapAreaSize", 106, "w", set_swap_area_size),
    rust("SetErrorMode", 107, "w", set_error_mode),
    rust("GlobalWire", 111, "w", global_lock),
    rust("GlobalUnWire", 112, "w", global_unlock),
    value("__AHSHIFT", 113, |_| ldt::AHSHIFT),
    value("__AHINCR", 114, |_| ldt::AHINCR),
    rust("OutputDebugString", 115, "p", output_debug_string),
    rust("OldYield", 117, "", yield16),
    rust("GetTaskQueueDS", 118, "", zero),
    rust("GetTaskQueueES", 119, "", zero),
    rust("UndefDynLink", 120, "", zero),
    rust("LocalShrink", 121, "ww", local_shrink),
    rust("IsTaskLocked", 122, "", zero),
    rust("KbdRst", 123, "", zero),
    rust("EnableKernel", 124, "", zero),
    rust("DisableKernel", 125, "", zero),
    win32(
        "GetPrivateProfileInt",
        127,
        "ppsp",
        "KERNEL32.DLL",
        "GetPrivateProfileIntA",
        b'w',
    ),
    win32(
        "GetPrivateProfileString",
        128,
        "ppppsp",
        "KERNEL32.DLL",
        "GetPrivateProfileStringA",
        b'w',
    ),
    win32(
        "WritePrivateProfileString",
        129,
        "pppp",
        "KERNEL32.DLL",
        "WritePrivateProfileStringA",
        b'b',
    ),
    rust("FileCDR", 130, "S", zero),
    rust("GetDOSEnvironment", 131, "", get_dos_environment),
    rust("GetWinFlags", 132, "", get_win_flags),
    rust("GetExePtr", 133, "w", get_exe_ptr),
    win32(
        "GetWindowsDirectory",
        134,
        "pw",
        "KERNEL32.DLL",
        "GetWindowsDirectoryA",
        b'w',
    ),
    win32(
        "GetSystemDirectory",
        135,
        "pw",
        "KERNEL32.DLL",
        "GetSystemDirectoryA",
        b'w',
    ),
    rust("GetDriveType", 136, "w", get_drive_type),
    rust("FatalAppExit", 137, "wp", fatal_app_exit),
    rust("GetHeapSpaces", 138, "w", get_heap_spaces),
    rust("SetSigHandler", 140, "SSSww", zero),
    rust("GetProfileSectionNames", 142, "pw", zero),
    rust("GetNumTasks", 152, "", get_num_tasks),
    rust("GlobalNotify", 154, "S", zero),
    rust("GetTaskDS", 155, "", get_task_ds),
    rust("LimitEMSPages", 156, "l", zero),
    rust("GetCurPID", 157, "l", zero),
    rust("IsWinOldApTask", 158, "w", zero),
    rust("GlobalHandleNoRIP", 159, "w", global_handle),
    rust("LocalCountFree", 161, "", local_count_free),
    rust("LocalHeapSize", 162, "", local_heap_size),
    rust("GlobalLRUOldest", 163, "w", same),
    rust("GlobalLRUNewest", 164, "w", same),
    rust("A20Proc", 165, "w", zero),
    rust("WinExec", 166, "pw", win_exec),
    rust("GetExpWinVer", 167, "w", get_exp_win_ver),
    rust("DirectResAlloc", 168, "www", direct_res_alloc),
    rust("GetFreeSpace", 169, "w", get_free_space),
    rust("AllocCStoDSAlias", 170, "w", alloc_cs_to_ds_alias),
    rust("AllocDStoCSAlias", 171, "w", alloc_ds_to_cs_alias),
    rust("AllocAlias", 172, "w", alloc_cs_to_ds_alias),
    value("__ROMBIOS", 173, |cpu| task::real_selector(cpu, 0xF000)),
    value("__A000H", 174, |cpu| task::real_selector(cpu, 0xA000)),
    rust("AllocSelector", 175, "w", alloc_selector),
    rust("FreeSelector", 176, "w", free_selector),
    rust("PrestoChangoSelector", 177, "ww", presto_chango_selector),
    value("__WINFLAGS", 178, |_| WIN_FLAGS as u16),
    value("__D000H", 179, |cpu| task::real_selector(cpu, 0xD000)),
    rust("LongPtrAdd", 180, "ll", zero),
    value("__B000H", 181, |cpu| task::real_selector(cpu, 0xB000)),
    value("__B800H", 182, |cpu| task::real_selector(cpu, 0xB800)),
    value("__0000H", 183, |cpu| task::real_selector(cpu, 0x0000)),
    rust("GlobalDOSAlloc", 184, "l", global_dos_alloc),
    rust("GlobalDOSFree", 185, "w", global_dos_free),
    rust("GetSelectorBase", 186, "w", get_selector_base),
    rust("SetSelectorBase", 187, "wl", set_selector_base),
    rust("GetSelectorLimit", 188, "w", get_selector_limit),
    rust("SetSelectorLimit", 189, "wl", set_selector_limit),
    value("__E000H", 190, |cpu| task::real_selector(cpu, 0xE000)),
    rust("GlobalPageLock", 191, "w", one),
    rust("GlobalPageUnlock", 192, "w", zero),
    value("__0040H", 193, |cpu| task::real_selector(cpu, 0x0040)),
    value("__F000H", 194, |cpu| task::real_selector(cpu, 0xF000)),
    value("__C000H", 195, |cpu| task::real_selector(cpu, 0xC000)),
    rust("SelectorAccessRights", 196, "www", selector_access_rights),
    rust("GlobalFix", 197, "w", global_fix),
    rust("GlobalUnfix", 198, "w", zero),
    rust("SetHandleCount", 199, "w", set_handle_count),
    rust("ValidateFreeSpaces", 200, "", zero),
    rust("DebugBreak", 203, "", zero),
    rust("SwapRecording", 204, "w", zero),
    rust("CVWBreak", 205, "", zero),
    rust("AllocSelectorArray", 206, "w", alloc_selector_array),
    rust("IsDBCSLeadByte", 207, "w", zero),
    rust("IsTask", 320, "w", is_task),
    rust("IsROMModule", 323, "w", zero),
    rust("LogError", 324, "wS", zero),
    rust("LogParamError", 325, "wSS", zero),
    rust("IsROMFile", 326, "w", zero),
    rust("IsBadReadPtr", 334, "Sw", is_bad_ptr),
    rust("IsBadWritePtr", 335, "Sw", is_bad_ptr),
    rust("IsBadCodePtr", 336, "S", is_bad_code_ptr),
    rust("IsBadStringPtr", 337, "Sw", is_bad_string_ptr),
    rust("IsSharedSelector", 345, "w", zero),
    rust("IsBadHugeReadPtr", 346, "Sl", is_bad_ptr),
    rust("IsBadHugeWritePtr", 347, "Sl", is_bad_ptr),
    rust("hmemcpy", 348, "ppl", hmemcpy),
    rust("_hread", 349, "wpl", lread),
    rust("_hwrite", 350, "wpl", lwrite),
    rust("lstrcpyn", 353, "SSw", lstrcpyn),
    rust("GetAppCompatFlags", 354, "w", zero),
    rust("GetWinDebugInfo", 355, "pw", zero),
    rust("SetWinDebugInfo", 356, "p", zero),
    rust("FarSetOwner", 403, "ww", zero),
    rust("RegOpenKey", 217, "lpp", reg_fail),
    rust("RegCreateKey", 218, "lpp", reg_fail),
    rust("RegDeleteKey", 219, "lp", reg_fail),
    rust("RegCloseKey", 220, "l", zero),
    rust("RegSetValue", 221, "lplpl", reg_fail),
    rust("RegDeleteValue", 222, "lp", reg_fail),
    rust("RegEnumValue", 223, "llpppppp", reg_fail),
    rust("RegQueryValue", 224, "lppp", reg_fail),
    rust("RegQueryValueEx", 225, "lppppp", reg_fail),
    rust("RegSetValueEx", 226, "lpllpl", reg_fail),
    rust("RegFlushKey", 227, "l", zero),
    rust("RegEnumKey", 216, "llpl", reg_fail),
    rust("GetSetKernelDOSProc", 311, "S", zero),
    rust("GetModuleName", 27, "wpw", get_module_name),
    // Flat thunks (flat.rs).
    register("C16ThkSL01", 631, super::flat::c16_thk_sl01),
    rust("ThunkConnect16", 651, "SSwlSSw", super::flat::thunk_connect16),
];

/// GetWinFlags': protected mode, 386 enhanced mode, a 486 with a
/// coprocessor.
const WIN_FLAGS: u32 = 0x0001 | 0x0008 | 0x0020 | 0x0400;

/// LoadLibrary's errors: the file wasn't found, isn't a Windows
/// library.
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_BAD_FORMAT: u32 = 11;

fn zero(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn one(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

fn same(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0])
}

pub(super) fn reg_fail(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    // ERROR_BADKEY: the 16-bit registry isn't kept.
    Some(2)
}

fn call_return16(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    thunk::call_return16(cpu);
    None
}

fn thunk16(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    thunk::thunk16_trap(cpu);
    None
}

/// The running task's handle (its TDB's selector), or 0.
fn current_handle(cpu: &Cpu) -> u16 {
    super::current_task(cpu).map_or(0, |t| cpu.bus.win32.win16.tasks[t].handle)
}

fn fatal_exit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let code = args.first().copied().unwrap_or(0) as u16;
    cpu.bus.log_string(&format!("[W16] FatalExit({:X})", code));
    task::exit(cpu, code);
    None
}

/// GetVersion: Windows 95's Windows version for 16-bit programs (3.95)
/// in the low word, and in the high word the DOS version that INT 21h
/// AH=30h gives.
fn get_version(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let dos = cpu.bus.dos_version;
    Some((dos.major as u32) << 24 | (dos.minor as u32) << 16 | 0x5F03)
}

// The local heap: DS's.

fn local_init(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(local::init(cpu, args[0] as u16, args[1] as u16, args[2] as u16) as u32)
}

fn local_alloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ds = cpu.ds();
    Some(local::alloc(cpu, ds, args[0] as u16, args[1] as u16) as u32)
}

fn local_realloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ds = cpu.ds();
    Some(local::realloc(cpu, ds, args[0] as u16, args[1] as u16, args[2] as u16) as u32)
}

fn local_free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ds = cpu.ds();
    Some(local::free(cpu, ds, args[0] as u16) as u32)
}

/// LocalLock: a near pointer, DX the segment's selector.
fn local_lock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ds = cpu.ds();
    let offset = local::lock(cpu, ds, args[0] as u16);
    Some(if offset == 0 {
        0
    } else {
        (ds as u32) << 16 | offset as u32
    })
}

fn local_unlock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ds = cpu.ds();
    Some(local::unlock(cpu, ds, args[0] as u16) as u32)
}

fn local_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ds = cpu.ds();
    Some(local::size(cpu, ds, args[0] as u16) as u32)
}

fn local_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ds = cpu.ds();
    Some(local::handle(cpu, ds, args[0] as u16) as u32)
}

fn local_flags(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ds = cpu.ds();
    Some(local::flags(cpu, ds, args[0] as u16) as u32)
}

fn local_compact(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let ds = cpu.ds();
    Some(local::compact(cpu, ds) as u32)
}

fn local_shrink(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let sel = if args[0] == 0 {
        cpu.ds()
    } else {
        args[0] as u16
    };
    Some(local::heap_size(cpu, sel) as u32)
}

fn local_count_free(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let ds = cpu.ds();
    Some(local::count_free(cpu, ds) as u32)
}

fn local_heap_size(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let ds = cpu.ds();
    Some(local::heap_size(cpu, ds) as u32)
}

// The global heap.

fn global_alloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let owner = current_handle(cpu);
    Some(global::alloc(cpu, args[0] as u16, args[1], owner, false) as u32)
}

fn global_realloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(global::realloc(cpu, args[0] as u16, args[1], args[2] as u16) as u32)
}

fn global_free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let h = args[0] as u16;
    Some(if h == 0 || global::free(cpu, h) {
        0
    } else {
        h as u32
    })
}

/// GlobalLock (and LockResource, GlobalWire): the block's first byte.
fn global_lock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let sel = global::lock(cpu, args[0] as u16);
    Some((sel as u32) << 16)
}

fn global_unlock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(global::unlock(cpu, args[0] as u16) as u32)
}

fn global_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(global::size(cpu, args[0] as u16))
}

/// GlobalHandle: the handle in AX, the selector in DX.
fn global_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let h = global::handle_of(cpu, args[0] as u16);
    Some(if h == 0 {
        0
    } else {
        (global::handle_to_sel(h) as u32) << 16 | h as u32
    })
}

fn global_flags(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(global::flags(cpu, args[0] as u16) as u32)
}

/// LockSegment(-1) is DS's.
fn lock_segment(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let sel = if args[0] as u16 == 0xFFFF {
        cpu.ds()
    } else {
        args[0] as u16
    };
    global::lock(cpu, global::handle_of(cpu, sel));
    Some(sel as u32)
}

fn unlock_segment(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let sel = if args[0] as u16 == 0xFFFF {
        cpu.ds()
    } else {
        args[0] as u16
    };
    global::unlock(cpu, global::handle_of(cpu, sel));
    Some(sel as u32)
}

/// The global heap's free bytes, as GetFreeSpace and GlobalCompact say
/// them: what is free in its region, and what the process could commit.
pub fn free_space(cpu: &Cpu) -> u32 {
    let largest = cpu.bus.win32.win16.heap.largest_free();
    largest.max(0x0100_0000)
}

fn global_compact(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(free_space(cpu))
}

fn get_free_space(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(free_space(cpu))
}

fn global_free_all(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let owner = if args[0] == 0 {
        current_handle(cpu)
    } else {
        args[0] as u16
    };
    global::free_owned(cpu, owner);
    Some(0)
}

fn global_fix(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    global::lock(cpu, args[0] as u16);
    Some(global::handle_to_sel(args[0] as u16) as u32)
}

/// GlobalDosAlloc(bytes): a fixed global block of conventional memory,
/// its paragraph in the high word and its selector in the low one.
fn global_dos_alloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let paras = args[0].div_ceil(16).clamp(1, 0xFFFF) as u16;
    let psp = cpu.bus.win32.psp;
    let Ok(seg) = crate::mcb::alloc(&mut cpu.bus, psp, paras) else {
        return Some(0);
    };
    let owner = current_handle(cpu);
    let base = (seg as u32) << 4;
    let size = paras as u32 * 16;
    match global::create_block(cpu, global::GMEM_FIXED, base, size, owner, false, true) {
        Some(sel) => Some((seg as u32) << 16 | sel as u32),
        None => {
            let _ = crate::mcb::free(&mut cpu.bus, seg);
            Some(0)
        }
    }
}

/// GlobalDosFree(selector): 0, or the selector when it isn't a block
/// GlobalDosAlloc gave.
fn global_dos_free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let sel = args[0] as u16;
    let dos =
        global::get(cpu, sel).filter(|a| a.fixed_memory && a.base < 0x10_0000 && a.base & 0xF == 0);
    let Some(a) = dos else {
        return Some(sel as u32);
    };
    let _ = crate::mcb::free(&mut cpu.bus, (a.base >> 4) as u16);
    global::free(cpu, sel);
    Some(0)
}

/// GetHeapSpaces(module): the free bytes of the local heap in the
/// module's automatic data segment (the low word) and the heap's size.
/// The system's modules have no heaps here, so they say 64 KB, all
/// free, for the programs that work out USER's and GDI's free percent.
/// A module without an automatic data segment of its own has DS's.
fn get_heap_spaces(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let module = super::module_index(cpu, args[0] as u16).map(|i| &cpu.bus.win32.win16.modules[i]);
    if module.is_some_and(|m| m.builtin.is_some()) {
        return Some(0xFFFF_FFFF);
    }
    let ds = match module.map_or(0, |m| m.dgroup) {
        0 => cpu.ds(),
        dgroup => global::handle_to_sel(dgroup),
    };
    let size = local::heap_size(cpu, ds) as u32;
    let free = local::count_free(cpu, ds) as u32;
    Some(size << 16 | free)
}

// Selectors.

fn alloc_selector(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(dpmi::alloc_selectors(cpu, 1, args[0] as u16) as u32)
}

fn alloc_selector_array(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(dpmi::alloc_selectors(cpu, args[0] as u16, 0) as u32)
}

fn free_selector(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let sel = args[0] as u16;
    Some(if dpmi::free_selector(cpu, sel) {
        0
    } else {
        sel as u32
    })
}

fn alloc_cs_to_ds_alias(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(dpmi::alias(cpu, args[0] as u16, false) as u32)
}

fn alloc_ds_to_cs_alias(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(dpmi::alias(cpu, args[0] as u16, true) as u32)
}

fn presto_chango_selector(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(dpmi::presto_chango(cpu, args[0] as u16, args[1] as u16) as u32)
}

fn get_selector_base(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(dpmi::base(cpu, args[0] as u16))
}

fn set_selector_base(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let sel = args[0] as u16;
    Some(if dpmi::set_base(cpu, sel, args[1]) {
        sel as u32
    } else {
        0
    })
}

fn get_selector_limit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(dpmi::limit(cpu, args[0] as u16))
}

fn set_selector_limit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    dpmi::set_limit(cpu, args[0] as u16, args[1]);
    Some(0)
}

/// SelectorAccessRights(sel, set, rights): the access byte and the
/// flags, or with `set` new ones.
fn selector_access_rights(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let sel = args[0] as u16;
    let Some((_, _, attr)) = super::selector_base(cpu, sel) else {
        return Some(0);
    };
    if args[1] == 0 {
        return Some(attr as u32);
    }
    let rights = args[2] as u16;
    if let Some((base, limit, _)) = super::selector_base(cpu, sel) {
        let system = super::win16(cpu).system;
        let access = rights as u8 | 0x60;
        ldt::write(
            cpu,
            system,
            sel,
            crate::dpmi::descriptor(base, limit, access, (rights >> 12) as u8 & 0x4),
        );
    }
    Some(0)
}

/// Whether `len` bytes at the 16:16 pointer `ptr` aren't all there.
fn bad_range(cpu: &Cpu, ptr: u32, len: u32) -> bool {
    let Some((_, limit, _)) = super::selector_base(cpu, (ptr >> 16) as u16) else {
        return true;
    };
    let offset = ptr & 0xFFFF;
    len != 0 && offset as u64 + len as u64 - 1 > limit as u64
}

fn is_bad_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(bad_range(cpu, args[0], args[1]) as u32)
}

fn is_bad_code_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let bad = match super::selector_base(cpu, (args[0] >> 16) as u16) {
        Some((_, limit, attr)) => attr & 0x08 == 0 || args[0] & 0xFFFF > limit,
        None => true,
    };
    Some(bad as u32)
}

fn is_bad_string_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if bad_range(cpu, args[0], 1) {
        return Some(1);
    }
    let at = super::linear(cpu, args[0]);
    let max = (args[1] as u16).max(1) as usize;
    let len = super::string_bytes(cpu, at, max).len() as u32;
    Some(bad_range(cpu, args[0], len.min(max as u32)) as u32)
}

fn hmemcpy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (to, from, len) = (args[0], args[1], args[2]);
    if let Some(bytes) = super::peek(cpu, from, len as usize) {
        super::poke(cpu, to, &bytes);
    }
    Some(0)
}

// Tasks.

fn yield16(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let ctx = Context::of(cpu);
    task::yield_now(cpu, ctx);
    None
}

/// WaitEvent: the task gives up its turn, as there are no events but
/// messages, which GetMessage waits for.
fn wait_event(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let mut ctx = Context::of(cpu);
    thunk::set_ax(&mut ctx, 0);
    task::yield_now(cpu, ctx);
    None
}

/// GetCurrentTask: the task in AX, the first task in DX.
fn get_current_task(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let first = cpu
        .bus
        .win32
        .win16
        .tasks
        .iter()
        .find(|t| t.exit_code.is_none())
        .map_or(0, |t| t.handle);
    Some((first as u32) << 16 | current_handle(cpu) as u32)
}

fn get_current_pdb(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(super::current_task(cpu).map_or(0, |t| cpu.bus.win32.win16.tasks[t].psp) as u32)
}

fn get_num_tasks(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(
        cpu.bus
            .win32
            .win16
            .tasks
            .iter()
            .filter(|t| t.exit_code.is_none())
            .count() as u32,
    )
}

fn is_task(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let h = args[0] as u16;
    Some(
        cpu.bus
            .win32
            .win16
            .tasks
            .iter()
            .any(|t| t.exit_code.is_none() && t.handle == h && h != 0) as u32,
    )
}

fn get_task_ds(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let instance = super::current_task(cpu).map_or(0, |t| cpu.bus.win32.win16.tasks[t].instance);
    Some(global::handle_to_sel(instance) as u32)
}

fn get_dos_environment(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let Some(t) = super::current_task(cpu) else {
        return Some(0);
    };
    let psp = cpu.bus.win32.win16.tasks[t].psp;
    let pdb = super::lin(cpu, psp, 0);
    Some((super::read_16(cpu, pdb + 0x2C) as u32) << 16)
}

fn get_win_flags(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(WIN_FLAGS)
}

fn set_error_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(t) = super::current_task(cpu) else {
        return Some(0);
    };
    let task = &mut cpu.bus.win32.win16.tasks[t];
    let old = task.error_mode;
    task.error_mode = args[0] as u16;
    Some(old as u32)
}

fn set_handle_count(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0].min(255))
}

fn set_swap_area_size(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0] & 0xFFFF | 0x1000 << 16)
}

fn output_debug_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = super::string(cpu, args[0]);
    cpu.bus
        .log_string(&format!("[W16] Debug: {}", text.trim_end()));
    Some(0)
}

fn fatal_app_exit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = super::string(cpu, args[1]);
    cpu.bus.log_string(&format!("[W16] FatalAppExit: {}", text));
    task::exit(cpu, 0xFF);
    None
}

/// Catch(buffer), a register function: the caller's place, where Throw
/// goes back to, in the buffer's nine words; 0 returned.
fn catch(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let mut ctx = Context::of(cpu);
    let (ip, cs, buffer) = far_call_frame(cpu, &ctx);
    let sp = (ctx.gpr[ESP] as u16).wrapping_add(8);
    let at = super::linear(cpu, buffer);
    let words = [
        ip,
        cs,
        sp,
        ctx.gpr[EBP] as u16,
        ctx.gpr[ESI] as u16,
        ctx.gpr[EDI] as u16,
        ctx.seg[Seg::DS as usize],
        0,
        ctx.seg[Seg::SS as usize],
    ];
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    super::poke(cpu, at, &bytes);
    ctx.eip = ip as u32;
    ctx.seg[Seg::CS as usize] = cs;
    thunk::set_sp(&mut ctx, sp);
    thunk::set_ax(&mut ctx, 0);
    crate::win32::system::load_context(cpu, &ctx);
    None
}

/// The far return address of a register function's caller, and the long
/// argument above it.
fn far_call_frame(cpu: &Cpu, ctx: &Context) -> (u16, u16, u32) {
    let base = super::selector_base(cpu, ctx.seg[Seg::SS as usize]).map_or(0, |(b, _, _)| b);
    let sp = ctx.gpr[ESP] as u16;
    let at = |i: u16| base.wrapping_add(sp.wrapping_add(i) as u32);
    (
        super::read_16(cpu, at(0)),
        super::read_16(cpu, at(2)),
        super::read_32(cpu, at(4)),
    )
}

/// Throw(buffer, value): back to the Catch that filled the buffer, which
/// returns `value`. The switches to 32-bit code made since are dropped.
fn throw(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let mut ctx = Context::of(cpu);
    let base = super::selector_base(cpu, ctx.seg[Seg::SS as usize]).map_or(0, |(b, _, _)| b);
    let sp = ctx.gpr[ESP] as u16;
    let value = super::read_16(cpu, base.wrapping_add(sp.wrapping_add(4) as u32));
    let buffer = super::read_32(cpu, base.wrapping_add(sp.wrapping_add(6) as u32));
    let at = super::linear(cpu, buffer);
    let words: Vec<u16> = (0..9).map(|i| super::read_16(cpu, at + 2 * i)).collect();
    let target = words[2];
    if let Some(t) = super::current_task(cpu) {
        let frames = &mut cpu.bus.win32.win16.tasks[t].frames;
        let inner = frames.iter().position(|f| match f {
            Frame::To32 { ctx, .. } => (ctx.gpr[ESP] as u16) < target,
            _ => false,
        });
        if let Some(i) = inner {
            frames.truncate(i);
        }
    }
    ctx.eip = words[0] as u32;
    ctx.seg[Seg::CS as usize] = words[1];
    thunk::set_sp(&mut ctx, target);
    ctx.gpr[EBP] = words[3] as u32;
    ctx.gpr[ESI] = words[4] as u32;
    ctx.gpr[EDI] = words[5] as u32;
    ctx.seg[Seg::DS as usize] = words[6];
    ctx.seg[Seg::SS as usize] = words[8];
    thunk::set_ax(&mut ctx, value);
    crate::win32::system::load_context(cpu, &ctx);
    None
}

// Modules.

/// The directory of the running task's program, for the libraries it
/// loads.
fn task_dir(cpu: &Cpu) -> String {
    let Some(t) = super::current_task(cpu) else {
        return String::new();
    };
    let handle = cpu.bus.win32.win16.tasks[t].module;
    super::module_index(cpu, handle)
        .map(|i| crate::win32::loader::directory_of(&cpu.bus.win32.win16.modules[i].path))
        .unwrap_or_default()
}

/// The handle 16-bit code gets for the module at `index`: a library's
/// instance, or the module's handle.
fn instance_of(cpu: &Cpu, index: usize) -> u16 {
    let m = &cpu.bus.win32.win16.modules[index];
    if m.dgroup != 0 { m.dgroup } else { m.handle }
}

/// LoadLibrary: the library and those it imports from loaded, their
/// entry points run; its instance, or an error below 32.
fn load_library(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = super::string(cpu, args[0]);
    let dir = task_dir(cpu);
    let mut inits = Vec::new();
    let index = match module::load(cpu, &name, &dir, &mut inits) {
        Ok(index) => index,
        Err(message) => {
            cpu.bus
                .log_string(&format!("[W16] LoadLibrary({}): {}", name, message));
            return Some(if message.contains("found") {
                ERROR_FILE_NOT_FOUND
            } else {
                ERROR_BAD_FORMAT
            });
        }
    };
    let instance = instance_of(cpu, index) as u32;
    let mut data = vec![instance];
    data.extend(
        inits
            .iter()
            .map(|&i| cpu.bus.win32.win16.modules[i].handle as u32),
    );
    let mut resume = Context::of(cpu);
    then_libs(cpu, 0, &data, &mut resume)
}

/// The libraries `data[1..]` are to have their entry points run, then
/// the context goes on with `data[0]` in AX. None when an entry point
/// runs first (and this then again after it).
pub fn then_libs(cpu: &mut Cpu, _: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    if data.len() <= 1 {
        return Some(data.first().copied().unwrap_or(0));
    }
    let mut rest = vec![data[0]];
    rest.extend_from_slice(&data[2..]);
    task::call_lib_init(cpu, data[1] as u16, *ctx, thunk::THEN_LIBS, rest);
    None
}

fn free_library(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    module::release(cpu, args[0] as u16);
    Some(1)
}

/// GetModuleHandle(name): a module name's module, or with a selector of
/// 0 the module of the instance it is.
fn get_module_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] >> 16 == 0 {
        let index = super::module_index(cpu, args[0] as u16);
        return Some(index.map_or(0, |i| cpu.bus.win32.win16.modules[i].handle) as u32);
    }
    let name = super::string(cpu, super::linear(cpu, args[0]));
    let base = name
        .rsplit(['\\', '/', ':'])
        .next()
        .unwrap_or(&name)
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    Some(
        cpu.bus
            .win32
            .win16
            .modules
            .iter()
            .find(|m| m.name == base)
            .map_or(0, |m| m.handle) as u32,
    )
}

fn get_module_usage(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let index = super::module_index(cpu, args[0] as u16);
    Some(index.map_or(0, |i| cpu.bus.win32.win16.modules[i].refs) as u32)
}

/// Copy `text` and its NUL to `at`, at most `size` bytes: the bytes
/// copied but the NUL.
fn copy_string(cpu: &mut Cpu, at: u32, size: u32, text: &[u8]) -> u32 {
    if size == 0 || at == 0 {
        return 0;
    }
    let n = text.len().min(size as usize - 1);
    let mut bytes = text[..n].to_vec();
    bytes.push(0);
    super::poke(cpu, at, &bytes);
    n as u32
}

fn get_module_file_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let handle = if args[0] == 0 {
        super::current_task(cpu).map_or(0, |t| cpu.bus.win32.win16.tasks[t].module)
    } else {
        args[0] as u16
    };
    let Some(index) = super::module_index(cpu, handle) else {
        return Some(0);
    };
    let path = crate::dosstr::to_bytes(&cpu.bus.win32.win16.modules[index].path);
    Some(copy_string(
        cpu,
        args[1],
        args[2] as i32 as u32 & 0xFFFF,
        &path,
    ))
}

fn get_module_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = super::module_index(cpu, args[0] as u16) else {
        return Some(0);
    };
    let name = cpu.bus.win32.win16.modules[index].name.clone().into_bytes();
    copy_string(cpu, args[1], args[2], &name);
    Some(1)
}

/// GetProcAddress(module, name or ordinal): the entry point's 16:16
/// address, or 0.
fn get_proc_address(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let module = if args[0] == 0 {
        super::current_task(cpu).map_or(0, |t| cpu.bus.win32.win16.tasks[t].module)
    } else {
        args[0] as u16
    };
    let ordinal = if args[1] < 0x1_0000 {
        Some(args[1] as u16)
    } else {
        let name = super::string(cpu, args[1]);
        module::ordinal_of(cpu, module, &name)
    };
    Some(ordinal.map_or(0, |o| module::entry_point(cpu, module, o)))
}

/// GetExePtr(handle): the module of an instance, a task or a module.
fn get_exe_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let h = args[0] as u16;
    if let Some(t) = cpu.bus.win32.win16.tasks.iter().find(|t| t.handle == h) {
        return Some(t.module as u32);
    }
    let index = super::module_index(cpu, h);
    Some(index.map_or(0, |i| cpu.bus.win32.win16.modules[i].handle) as u32)
}

fn get_exp_win_ver(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let index = super::module_index(cpu, args[0] as u16);
    Some(index.map_or(0x030A, |i| cpu.bus.win32.win16.modules[i].expected_version) as u32)
}

fn get_exe_version(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let module = super::current_task(cpu).map_or(0, |t| cpu.bus.win32.win16.tasks[t].module);
    get_exp_win_ver(cpu, &[module as u32])
}

/// GetCodeHandle(function): the handle of the segment it is in.
fn get_code_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(global::handle_of(cpu, (args[0] >> 16) as u16) as u32)
}

/// GetCodeInfo(function, info): the segment's entry of the segment
/// table, in sectors as the file has them.
fn get_code_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let sel = (args[0] >> 16) as u16;
    let found = cpu.bus.win32.win16.modules.iter().find_map(|m| {
        let n = m.segments.iter().position(|s| s.selector == sel)?;
        Some((m.segments[n].clone(), m.segments[n].size))
    });
    let Some((segment, size)) = found else {
        return Some(0);
    };
    let mut info = [0u8; 12];
    info[2..4].copy_from_slice(&(size as u16).to_le_bytes());
    info[4..6].copy_from_slice(&segment.flags.to_le_bytes());
    info[6..8].copy_from_slice(&(size as u16).to_le_bytes());
    info[8..10].copy_from_slice(&segment.handle.to_le_bytes());
    super::poke(cpu, args[1], &info);
    Some(1)
}

/// GetInstanceData(instance, offset, count): the bytes of another
/// instance's data segment, to the same offset of DS.
fn get_instance_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (instance, offset, count) = (args[0] as u16, args[1] as u16, args[2] as u16);
    let from = super::lin(cpu, global::handle_to_sel(instance), offset);
    let ds = cpu.ds();
    let to = super::lin(cpu, ds, offset);
    if from == 0 || to == 0 {
        return Some(0);
    }
    if let Some(bytes) = super::peek(cpu, from, count as usize) {
        super::poke(cpu, to, &bytes);
    }
    Some(count as u32)
}

/// MakeProcInstance(function, instance): a thunk that loads AX with the
/// instance's data segment, for an exported function whose prologue
/// takes DS from AX, and jumps to it.
fn make_proc_instance(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (function, instance) = (args[0], args[1] as u16);
    if function == 0 {
        return Some(0);
    }
    // A library's functions load their own data segment.
    let library = super::module_index(cpu, (function >> 16) as u16)
        .is_some_and(|i| cpu.bus.win32.win16.modules[i].flags & rust_dos_ne::LIBMODULE != 0);
    if instance == 0 || library {
        return Some(function);
    }
    Some(thunk::instance_thunk(
        cpu,
        function,
        global::handle_to_sel(instance),
    ))
}

/// WinExec(command line, show): the program as a new task; its instance,
/// or an error below 32.
fn win_exec(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let line = super::string(cpu, args[0]);
    let line = line.trim();
    let (path, arguments) = match line.find([' ', '\t']) {
        Some(n) => (&line[..n], line[n..].trim_start()),
        None => (line, ""),
    };
    Some(match task::exec(cpu, path, arguments, args[1] as u16) {
        Ok(instance) => instance as u32,
        Err(code) => code as u32,
    })
}

/// LoadModule(name, parameters): WinExec's, with the command tail and
/// the show command from the parameter block.
fn load_module(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let path = super::string(cpu, args[0]);
    let block = args[1];
    if block == 0 {
        // A library.
        return load_library(cpu, &[args[0]]);
    }
    let tail = super::linear(cpu, super::read_32(cpu, block + 2));
    let len = super::peek(cpu, tail, 1).map_or(0, |b| b[0]) as usize;
    let arguments = crate::dosstr::from_bytes(&super::peek(cpu, tail + 1, len).unwrap_or_default());
    let show_at = super::linear(cpu, super::read_32(cpu, block + 6));
    let show = if show_at != 0 {
        super::read_16(cpu, show_at + 2)
    } else {
        1
    };
    Some(match task::exec(cpu, &path, arguments.trim(), show) {
        Ok(instance) => instance as u32,
        Err(code) => code as u32,
    })
}

// Resources.

fn find_resource(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = resource::res_id(cpu, args[1]);
    let kind = resource::res_id(cpu, args[2]);
    let found = resource::find(cpu, args[0] as u16, &name, &kind);
    if found == 0 {
        cpu.bus.log_string(&format!(
            "[W16] FindResource: no resource {:?} of type {:?} in {:04X}",
            name, kind, args[0]
        ));
    }
    Some(found as u32)
}

fn load_resource(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(resource::load(cpu, args[0] as u16, args[1] as u16) as u32)
}

fn sizeof_resource(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(resource::size(cpu, args[0] as u16, args[1] as u16))
}

/// AccessResource(module, hrsrc): the module's file opened for reading,
/// at the resource's data, or -1.
fn access_resource(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (module, hrsrc) = (args[0] as u16, args[1] as u16);
    let (Some(offset), Some(index)) = (
        resource::offset(cpu, module, hrsrc),
        super::module_index(cpu, module),
    ) else {
        return Some(HFILE_ERROR);
    };
    let path = crate::dosstr::to_bytes(&cpu.bus.win32.win16.modules[index].path);
    let Ok((file, _)) = dos::call21(cpu, [0x3D00, 0, 0, 0], Some(&path)) else {
        return Some(HFILE_ERROR);
    };
    llseek(cpu, &[file as u32, offset, 0]);
    Some(file as u32)
}

fn alloc_resource(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let size = args[2].max(resource::size(cpu, args[0] as u16, args[1] as u16));
    let owner = super::module_index(cpu, args[0] as u16)
        .map_or(0, |i| cpu.bus.win32.win16.modules[i].handle);
    Some(global::alloc(cpu, global::GMEM_MOVEABLE, size, owner, false) as u32)
}

fn direct_res_alloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let owner = super::module_index(cpu, args[0] as u16)
        .map_or(0, |i| cpu.bus.win32.win16.modules[i].handle);
    Some(global::alloc(cpu, global::GMEM_MOVEABLE, args[2] * 16, owner, false) as u32)
}

// The atom tables: the tasks' local ones and the global one are one
// table here, whose atoms are C000h on, an integer atom its number.

/// The atom of `name` (a 16:16 pointer, or an integer atom when its
/// selector is 0), added to the table with `add` or used once more; 0
/// when it isn't there or can't be.
fn atom(cpu: &mut Cpu, name: u32, add: bool) -> u16 {
    // MAKEINTATOM: an integer atom, below C000h.
    if name >> 16 == 0 {
        return if name < 0xC000 { name as u16 } else { 0 };
    }
    let text = super::string(cpu, super::linear(cpu, name));
    atom_of(cpu, text, add)
}

/// The atom of the string `text` ("#n" for an integer atom), added to
/// the table with `add` or used once more; 0 when it isn't there or
/// can't be.
pub fn atom_of(cpu: &mut Cpu, text: String, add: bool) -> u16 {
    if let Some(n) = text.strip_prefix('#').and_then(|n| n.parse::<u16>().ok()) {
        return if n < 0xC000 { n } else { 0 };
    }
    if text.is_empty() || text.len() > 255 {
        return 0;
    }
    let atoms = &mut cpu.bus.win32.win16.atoms;
    if let Some(i) = atoms
        .iter()
        .position(|(a, n)| *n > 0 && a.eq_ignore_ascii_case(&text))
    {
        if add {
            atoms[i].1 = atoms[i].1.saturating_add(1);
        }
        return 0xC000 + i as u16;
    }
    if !add {
        return 0;
    }
    let i = match atoms.iter().position(|(_, n)| *n == 0) {
        Some(i) => {
            atoms[i] = (text, 1);
            i
        }
        None if atoms.len() < 0x3FFF => {
            atoms.push((text, 1));
            atoms.len() - 1
        }
        None => return 0,
    };
    0xC000 + i as u16
}

pub fn find_atom(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(atom(cpu, args[0], false) as u32)
}

pub fn add_atom(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(atom(cpu, args[0], true) as u32)
}

/// DeleteAtom: a string atom's count of uses less one, the atom gone at
/// 0. 0 when it went (or is an integer atom), the atom when it didn't.
pub fn delete_atom(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = args[0] as u16;
    if a < 0xC000 {
        return Some(0);
    }
    match cpu.bus.win32.win16.atoms.get_mut((a - 0xC000) as usize) {
        Some((name, n)) if *n > 0 => {
            *n -= 1;
            if *n == 0 {
                name.clear();
            }
            Some(0)
        }
        _ => Some(a as u32),
    }
}

/// The string of the atom `a` ("#n" for an integer atom), if it is one.
pub fn atom_name(cpu: &Cpu, a: u16) -> Option<String> {
    if a == 0 {
        None
    } else if a < 0xC000 {
        Some(format!("#{}", a))
    } else {
        match cpu.bus.win32.win16.atoms.get((a - 0xC000) as usize) {
            Some((n, uses)) if *uses > 0 => Some(n.clone()),
            _ => None,
        }
    }
}

pub fn get_atom_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(name) = atom_name(cpu, args[0] as u16) else {
        return Some(0);
    };
    let name = crate::dosstr::to_bytes(&name);
    Some(copy_string(cpu, args[1], args[2] & 0xFFFF, &name))
}

fn get_atom_handle(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0])
}

// Files: DOS's handles, as INT 21h's.

const HFILE_ERROR: u32 = 0xFFFF;

fn name_of(cpu: &Cpu, at: u32) -> Vec<u8> {
    super::string_bytes(cpu, at, 0x100)
}

fn lopen(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = name_of(cpu, args[0]);
    let mode = args[1] as u16 & 0xFF;
    Some(
        match dos::call21(cpu, [0x3D00 | mode, 0, 0, 0], Some(&name)) {
            Ok((ax, _)) => ax as u32,
            Err(_) => HFILE_ERROR,
        },
    )
}

fn lcreat(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = name_of(cpu, args[0]);
    Some(
        match dos::call21(cpu, [0x3C00, 0, args[1] as u16, 0], Some(&name)) {
            Ok((ax, _)) => ax as u32,
            Err(_) => HFILE_ERROR,
        },
    )
}

pub(super) fn lclose(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(
        match dos::call21(cpu, [0x3E00, args[0] as u16, 0, 0], None) {
            Ok(_) => 0,
            Err(_) => HFILE_ERROR,
        },
    )
}

/// _lread and _hread: the bytes read, or -1.
pub(super) fn lread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let len = if args.len() > 2 { args[2] } else { 0 };
    Some(dos::transfer(cpu, args[0] as u16, args[1], len, false).unwrap_or(0xFFFF_FFFF))
}

fn lwrite(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(dos::transfer(cpu, args[0] as u16, args[1], args[2], true).unwrap_or(0xFFFF_FFFF))
}

pub(super) fn llseek(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let offset = args[1];
    let regs = [
        0x4200 | args[2] as u16 & 0xFF,
        args[0] as u16,
        (offset >> 16) as u16,
        offset as u16,
    ];
    Some(match dos::call21(cpu, regs, None) {
        Ok((ax, dx)) => (dx as u32) << 16 | ax as u32,
        Err(_) => 0xFFFF_FFFF,
    })
}

/// OpenFile's styles.
const OF_PARSE: u16 = 0x0100;
const OF_DELETE: u16 = 0x0200;
const OF_CREATE: u16 = 0x1000;
const OF_EXIST: u16 = 0x4000;
const OF_REOPEN: u16 = 0x8000;

/// OpenFile(name, OFSTRUCT, style): the file opened (created, deleted,
/// looked for) as `style` says, found in the current directory and
/// Windows' when the name has none; the OFSTRUCT gets its full name.
pub(super) fn open_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (ofs, style) = (args[1], args[2] as u16);
    let name = if style & OF_REOPEN != 0 {
        name_of(cpu, ofs + 8)
    } else {
        name_of(cpu, args[0])
    };
    let has_dir = name.iter().any(|&b| b == b'\\' || b == b'/' || b == b':');
    let mut candidates = vec![name.clone()];
    if !has_dir && style & OF_CREATE == 0 {
        for dir in [
            crate::win32::loader::WINDOWS_DIR,
            crate::win32::loader::SYSTEM_DIR,
        ] {
            let mut path = crate::dosstr::to_bytes(dir);
            path.push(b'\\');
            path.extend_from_slice(&name);
            candidates.push(path);
        }
        let dir = task_dir(cpu);
        if !dir.is_empty() {
            let mut path = crate::dosstr::to_bytes(&dir);
            path.push(b'\\');
            path.extend_from_slice(&name);
            candidates.insert(1, path);
        }
    }
    let write_ofs = |cpu: &mut Cpu, path: &[u8], error: u16| {
        let mut bytes = vec![0u8; 136];
        bytes[0] = 136;
        bytes[1] = 1;
        bytes[2..4].copy_from_slice(&error.to_le_bytes());
        let n = path.len().min(127);
        bytes[8..8 + n].copy_from_slice(&path[..n]);
        super::poke(cpu, ofs, &bytes);
    };
    if style & OF_PARSE != 0 {
        let full = dos::full_name(cpu, &name).unwrap_or(name);
        write_ofs(cpu, &full, 0);
        return Some(0);
    }
    if style & OF_CREATE != 0 {
        let full = dos::full_name(cpu, &name).unwrap_or_else(|| name.clone());
        return Some(match dos::call21(cpu, [0x3C00, 0, 0, 0], Some(&name)) {
            Ok((h, _)) => {
                write_ofs(cpu, &full, 0);
                h as u32
            }
            Err(e) => {
                write_ofs(cpu, &full, e);
                HFILE_ERROR
            }
        });
    }
    let mut error = 2;
    for path in candidates {
        let full = dos::full_name(cpu, &path).unwrap_or_else(|| path.clone());
        if style & OF_DELETE != 0 {
            match dos::call21(cpu, [0x4100, 0, 0, 0], Some(&path)) {
                Ok(_) => {
                    write_ofs(cpu, &full, 0);
                    return Some(1);
                }
                Err(e) => error = e,
            }
            continue;
        }
        match dos::call21(cpu, [0x3D00 | style & 0x7F, 0, 0, 0], Some(&path)) {
            Ok((h, _)) => {
                write_ofs(cpu, &full, 0);
                if style & OF_EXIST != 0 {
                    // The handle it had, closed already, as Windows 3.1
                    // returns it.
                    let _ = dos::call21(cpu, [0x3E00, h, 0, 0], None);
                    return Some(h as u32);
                }
                return Some(h as u32);
            }
            Err(e) => error = e,
        }
    }
    write_ofs(cpu, &name, error);
    Some(HFILE_ERROR)
}

fn get_temp_drive(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(b'C' as u32 | (b':' as u32) << 8)
}

/// GetTempFileName(drive, prefix, unique, buffer): a name in the root of
/// the drive, made unique (and the file made) when `unique` is 0.
fn get_temp_file_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let drive = args[0] as u8;
    let letter = if drive & 0x80 != 0 && (drive & 0x7F).is_ascii_alphabetic() {
        (drive & 0x7F).to_ascii_uppercase()
    } else {
        b'C'
    };
    let prefix: Vec<u8> = name_of(cpu, args[1]).into_iter().take(3).collect();
    let prefix = crate::dosstr::from_bytes(&prefix);
    let mut unique = args[2] as u16;
    let make = unique == 0;
    if make {
        unique = (cpu.bus.clock.now_micros() as u16) | 1;
    }
    for _ in 0..0x100 {
        let path = format!("{}:\\~{}{:04X}.TMP", letter as char, prefix, unique);
        let bytes = crate::dosstr::to_bytes(&path);
        if !make {
            copy_string(cpu, args[3], 144, &bytes);
            return Some(unique as u32);
        }
        if let Ok((h, _)) = dos::call21(cpu, [0x5B00, 0, 0, 0], Some(&bytes)) {
            let _ = dos::call21(cpu, [0x3E00, h, 0, 0], None);
            copy_string(cpu, args[3], 144, &bytes);
            return Some(unique as u32);
        }
        unique = unique.wrapping_add(1).max(1);
    }
    Some(0)
}

/// GetDriveType(drive): removable for A: and B:, fixed for the others
/// DOS has, 0 for those it hasn't.
fn get_drive_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let drive = args[0] as u16;
    let exists =
        matches!(dos::call21(cpu, [0x3600, 0, 0, drive + 1], None), Ok((ax, _)) if ax != 0xFFFF);
    Some(match (exists, drive) {
        (false, _) => 0,
        (true, 0 | 1) => 2,
        _ => 3,
    })
}

// Strings.

fn lstrlen(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(super::string_bytes(cpu, args[0], 0x1_0000).len() as u32)
}

pub fn lstrcmp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = super::string(cpu, args[0]);
    let b = super::string(cpu, args[1]);
    Some(match a.cmp(&b) {
        std::cmp::Ordering::Less => 0xFFFF,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    })
}

fn lstrcpy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let to = super::linear(cpu, args[0]);
    let from = super::linear(cpu, args[1]);
    let mut text = super::string_bytes(cpu, from, 0x1_0000);
    text.push(0);
    super::poke(cpu, to, &text);
    Some(args[0])
}

fn lstrcpyn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let to = super::linear(cpu, args[0]);
    let from = super::linear(cpu, args[1]);
    let n = args[2] as u16 as usize;
    if n == 0 {
        return Some(args[0]);
    }
    let mut text = super::string_bytes(cpu, from, n - 1);
    text.truncate(n - 1);
    text.push(0);
    super::poke(cpu, to, &text);
    Some(args[0])
}

fn lstrcat(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let to = super::linear(cpu, args[0]);
    let from = super::linear(cpu, args[1]);
    let len = super::string_bytes(cpu, to, 0x1_0000).len() as u32;
    let mut text = super::string_bytes(cpu, from, 0x1_0000);
    text.push(0);
    super::poke(cpu, to + len, &text);
    Some(args[0])
}
