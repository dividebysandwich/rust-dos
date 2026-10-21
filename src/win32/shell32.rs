//! SHELL32.DLL: what games of Windows 95's time call the shell for. The
//! special folders have the paths of a Windows 95 installed in
//! C:\WINDOWS, by their CSIDL numbers; their item ID lists (PIDLs) are
//! the shell's own, allocated with the process's task allocator as on
//! Windows, which programs free through SHGetMalloc's allocator. There
//! is no Explorer: ShellExecute opens nothing, the folder browser and
//! file operations fail, and dropped files and icons in files there are
//! none.

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "SHELL32.DLL",
    base: 0xBFC0_0000,
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

/// The functions. Append only (see `stubs`). Their ordinals are past
/// those Windows 95 exports only by number (ILClone is 18, ILFree 155):
/// a program importing one of those by number finds nothing here.
static APIS: &[Api] = &[
    stdcall("ShellExecuteA", 1001, 6, shell_execute),
    stdcall("ShellExecuteW", 1002, 6, shell_execute),
    stdcall("Shell_NotifyIconA", 1003, 2, true_),
    stdcall("Shell_NotifyIconW", 1004, 2, true_),
    stdcall(
        "SHGetSpecialFolderLocation",
        1005,
        3,
        sh_get_special_folder_location,
    ),
    stdcall("SHGetPathFromIDListA", 1006, 2, sh_get_path_from_id_list),
    stdcall("SHGetPathFromIDList", 1007, 2, sh_get_path_from_id_list),
    stdcall("SHGetSpecialFolderPathA", 1008, 4, sh_get_special_folder_path),
    stdcall("SHGetMalloc", 1009, 1, sh_get_malloc),
    stdcall("SHBrowseForFolderA", 1010, 1, null),
    stdcall("SHBrowseForFolder", 1011, 1, null),
    stdcall("DragAcceptFiles", 1012, 2, nothing),
    stdcall("DragQueryFileA", 1013, 4, null),
    stdcall("DragQueryFile", 1014, 4, null),
    stdcall("DragFinish", 1015, 1, nothing),
    stdcall("DragQueryPoint", 1016, 2, null),
    stdcall("ExtractIconA", 1017, 3, null),
    stdcall("ExtractIcon", 1018, 3, null),
    stdcall("SHFileOperationA", 1019, 1, sh_file_operation),
    stdcall("SHFileOperation", 1020, 1, sh_file_operation),
    stdcall("SHGetFileInfoA", 1021, 5, null),
    stdcall("SHGetFileInfo", 1022, 5, null),
    stdcall("ShellAboutA", 1023, 4, true_),
    stdcall("SHAddToRecentDocs", 1024, 2, nothing),
    stdcall("SHChangeNotify", 1025, 4, nothing),
    stdcall("FindExecutableA", 1026, 3, shell_execute),
];

const TRUE: u32 = 1;
const FALSE: u32 = 0;
const S_OK: u32 = 0;
const E_FAIL: u32 = 0x8000_4005;
const E_INVALIDARG: u32 = 0x8007_0057;
const E_OUTOFMEMORY: u32 = 0x8007_000E;

fn true_(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(TRUE)
}

fn null(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

fn nothing(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// ShellExecute and FindExecutable: nothing to open with, and the
/// result says the file wasn't found (SE_ERR_FNF).
fn shell_execute(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(2)
}

/// SHFileOperation: not done, and the result says so (a nonzero error,
/// ERROR_ACCESS_DENIED).
fn sh_file_operation(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(5)
}

/// The path of the special folder `csidl`, as Windows 95 has it.
fn folder_path(csidl: u32) -> Option<&'static str> {
    Some(match csidl & 0xFF {
        0x00 | 0x10 | 0x19 => "C:\\WINDOWS\\Desktop",
        0x02 | 0x17 => "C:\\WINDOWS\\Start Menu\\Programs",
        0x05 => "C:\\My Documents",
        0x06 => "C:\\WINDOWS\\Favorites",
        0x07 | 0x18 => "C:\\WINDOWS\\Start Menu\\Programs\\StartUp",
        0x08 => "C:\\WINDOWS\\Recent",
        0x09 => "C:\\WINDOWS\\SendTo",
        0x0B | 0x16 => "C:\\WINDOWS\\Start Menu",
        0x13 => "C:\\WINDOWS\\NetHood",
        0x14 => "C:\\WINDOWS\\Fonts",
        0x15 => "C:\\WINDOWS\\ShellNew",
        0x1A => "C:\\WINDOWS\\Application Data",
        0x1B => "C:\\WINDOWS\\PrintHood",
        0x20 => "C:\\WINDOWS\\Temporary Internet Files",
        0x21 => "C:\\WINDOWS\\Cookies",
        0x22 => "C:\\WINDOWS\\History",
        0x24 => "C:\\WINDOWS",
        0x25 => "C:\\WINDOWS\\SYSTEM",
        0x26 => "C:\\Program Files",
        0x2B => "C:\\Program Files\\Common Files",
        _ => return None,
    })
}

/// The shell's ID list of a special folder: one item, its size (8),
/// "RD" and the CSIDL, then the list's end (a size of 0).
const PIDL_SIZE: u32 = 10;
const PIDL_MARK: u16 = 0x4452;

/// SHGetSpecialFolderLocation(hwnd, csidl, out): the folder's ID list,
/// for SHGetPathFromIDList, from the task allocator.
fn sh_get_special_folder_location(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [_, csidl, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    if !cpu.bus.guest_probe(out, 4, true) {
        return None;
    }
    if folder_path(csidl).is_none() {
        cpu.bus.guest_write_32(out, 0);
        return Some(E_FAIL);
    }
    let pidl = super::ole32::task_alloc(cpu, PIDL_SIZE);
    if pidl == 0 {
        cpu.bus.guest_write_32(out, 0);
        return Some(E_OUTOFMEMORY);
    }
    let mut bytes = [0u8; PIDL_SIZE as usize];
    bytes[0..2].copy_from_slice(&8u16.to_le_bytes());
    bytes[2..4].copy_from_slice(&PIDL_MARK.to_le_bytes());
    bytes[4..8].copy_from_slice(&(csidl & 0xFF).to_le_bytes());
    cpu.bus.guest_write_bytes(pidl, &bytes);
    cpu.bus.guest_write_32(out, pidl);
    Some(S_OK)
}

/// Write `path` and its 0 at `at`, in MAX_PATH bytes.
fn write_path(cpu: &mut Cpu, at: u32, path: &str) -> Option<()> {
    let mut bytes = path.as_bytes().to_vec();
    bytes.truncate(259);
    bytes.push(0);
    if !cpu.bus.guest_probe(at, bytes.len(), true) {
        return None;
    }
    cpu.bus.guest_write_bytes(at, &bytes);
    Some(())
}

/// SHGetPathFromIDList(pidl, path): a special folder's path; FALSE for
/// other lists, which name no file system folder the shell knows.
fn sh_get_path_from_id_list(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [pidl, out] = args[..] else {
        return Some(FALSE);
    };
    if pidl == 0 || out == 0 {
        return Some(FALSE);
    }
    let mut bytes = [0u8; PIDL_SIZE as usize];
    cpu.bus.guest_read_bytes(pidl, &mut bytes);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let size = u16::from_le_bytes([bytes[0], bytes[1]]);
    let mark = u16::from_le_bytes([bytes[2], bytes[3]]);
    let csidl = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let path = if size == 8 && mark == PIDL_MARK {
        folder_path(csidl)
    } else {
        None
    };
    let Some(path) = path else {
        cpu.bus.guest_write_8(out, 0);
        return Some(FALSE);
    };
    write_path(cpu, out, path)?;
    Some(TRUE)
}

/// SHGetSpecialFolderPathA(hwnd, path, csidl, create).
fn sh_get_special_folder_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [_, out, csidl, _] = args[..] else {
        return Some(FALSE);
    };
    let Some(path) = folder_path(csidl) else {
        return Some(FALSE);
    };
    if out == 0 {
        return Some(FALSE);
    }
    write_path(cpu, out, path)?;
    Some(TRUE)
}

/// SHGetMalloc(out): the process's task allocator, as CoGetMalloc's.
/// Its methods are OLE32's, mapped if the program doesn't import it.
fn sh_get_malloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if super::loader::map_builtin_named(cpu, "OLE32.DLL").is_none() {
        return Some(E_OUTOFMEMORY);
    }
    super::ole32::co_get_malloc(cpu, &[1, args[0]])
}
