//! SHELL32.DLL: the shell of Windows 95 (4.00) as setup programs and
//! games use it. The special folders are on drive C: where Windows 95
//! keeps them (folders.rs), named by item ID lists (pidl.rs, il.rs).
//! There is no Explorer: dropped files and icons in files there are
//! none.

pub(crate) mod browse;
mod exec;
mod fileop;
mod folders;
mod il;
mod info;
mod link;
mod pidl;
mod progman;

use super::com;
use super::stubs::{Api, Conv, Dll, Vtable};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "SHELL32.DLL",
    base: 0xBFC0_0000,
    apis: APIS,
    vars: &[],
    init: None,
    vtables: VTABLES,
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

/// The functions. Append only (see `stubs`). Those Windows 95 exports
/// by number alone (ILClone is 18, ILFree 155) have their numbers; the
/// others have numbers past Windows 95's, as programs import them by
/// name.
static APIS: &[Api] = &[
    stdcall("ShellExecuteA", 1001, 6, exec::shell_execute),
    stdcall("ShellExecuteW", 1002, 6, not_found),
    stdcall("Shell_NotifyIconA", 1003, 2, true_),
    stdcall("Shell_NotifyIconW", 1004, 2, true_),
    stdcall(
        "SHGetSpecialFolderLocation",
        1005,
        3,
        il::sh_get_special_folder_location,
    ),
    stdcall(
        "SHGetPathFromIDListA",
        1006,
        2,
        il::sh_get_path_from_id_list,
    ),
    stdcall("SHGetPathFromIDList", 1007, 2, il::sh_get_path_from_id_list),
    stdcall(
        "SHGetSpecialFolderPathA",
        1008,
        4,
        il::sh_get_special_folder_path,
    ),
    stdcall("SHGetMalloc", 1009, 1, sh_get_malloc),
    stdcall("SHBrowseForFolderA", 1010, 1, browse::sh_browse_for_folder),
    stdcall("SHBrowseForFolder", 1011, 1, browse::sh_browse_for_folder),
    stdcall("DragAcceptFiles", 1012, 2, nothing),
    stdcall("DragQueryFileA", 1013, 4, null),
    stdcall("DragQueryFile", 1014, 4, null),
    stdcall("DragFinish", 1015, 1, nothing),
    stdcall("DragQueryPoint", 1016, 2, null),
    stdcall("ExtractIconA", 1017, 3, null),
    stdcall("ExtractIcon", 1018, 3, null),
    stdcall("SHFileOperationA", 1019, 1, fileop::sh_file_operation),
    stdcall("SHFileOperation", 1020, 1, fileop::sh_file_operation),
    stdcall("SHGetFileInfoA", 1021, 5, info::sh_get_file_info),
    stdcall("SHGetFileInfo", 1022, 5, info::sh_get_file_info),
    stdcall("ShellAboutA", 1023, 4, true_),
    stdcall("SHAddToRecentDocs", 1024, 2, nothing),
    stdcall("SHChangeNotify", 1025, 4, nothing),
    stdcall("FindExecutableA", 1026, 3, exec::find_executable),
    stdcall("ILFindLastID", 16, 1, il::il_find_last_id),
    stdcall("ILRemoveLastID", 17, 1, il::il_remove_last_id),
    stdcall("ILClone", 18, 1, il::il_clone),
    stdcall("ILCloneFirst", 19, 1, il::il_clone_first),
    stdcall("ILIsEqual", 21, 2, il::il_is_equal),
    stdcall("ILIsParent", 23, 3, il::il_is_parent),
    stdcall("ILFindChild", 24, 2, il::il_find_child),
    stdcall("ILCombine", 25, 2, il::il_combine),
    stdcall("SHILCreateFromPath", 28, 3, il::sh_il_create_from_path),
    stdcall("StrRetToStrN", 96, 4, il::str_ret_to_str_n),
    stdcall("ILGetSize", 152, 1, il::il_get_size),
    stdcall("ILGetNext", 153, 1, il::il_get_next),
    stdcall("ILAppendID", 154, 3, il::il_append_id),
    stdcall("ILFree", 155, 1, il::sh_free),
    stdcall("ILCreateFromPath", 157, 1, il::il_create_from_path),
    stdcall(
        "SHSimpleIDListFromPath",
        162,
        1,
        il::sh_simple_id_list_from_path,
    ),
    stdcall(
        "SHGetSpecialFolderPath",
        175,
        4,
        il::sh_get_special_folder_path,
    ),
    stdcall("SHFree", 195, 1, il::sh_free),
    stdcall("SHAlloc", 196, 1, il::sh_alloc),
    stdcall("DllGetClassObject", 128, 3, dll_get_class_object),
    stdcall("IShellLinkA_QueryInterface", 0, 3, link::query_interface),
    stdcall("IShellLinkA_AddRef", 0, 1, link::add_ref),
    stdcall("IShellLinkA_Release", 0, 1, link::release),
    stdcall("IShellLinkA_GetPath", 0, 5, link::get_path),
    stdcall("IShellLinkA_GetIDList", 0, 2, link::get_id_list),
    stdcall("IShellLinkA_SetIDList", 0, 2, link::set_id_list),
    stdcall("IShellLinkA_GetDescription", 0, 3, link::get_description),
    stdcall("IShellLinkA_SetDescription", 0, 2, link::set_description),
    stdcall(
        "IShellLinkA_GetWorkingDirectory",
        0,
        3,
        link::get_working_directory,
    ),
    stdcall(
        "IShellLinkA_SetWorkingDirectory",
        0,
        2,
        link::set_working_directory,
    ),
    stdcall("IShellLinkA_GetArguments", 0, 3, link::get_arguments),
    stdcall("IShellLinkA_SetArguments", 0, 2, link::set_arguments),
    stdcall("IShellLinkA_GetHotkey", 0, 2, link::get_hotkey),
    stdcall("IShellLinkA_SetHotkey", 0, 2, link::set_hotkey),
    stdcall("IShellLinkA_GetShowCmd", 0, 2, link::get_show_cmd),
    stdcall("IShellLinkA_SetShowCmd", 0, 2, link::set_show_cmd),
    stdcall("IShellLinkA_GetIconLocation", 0, 4, link::get_icon_location),
    stdcall("IShellLinkA_SetIconLocation", 0, 3, link::set_icon_location),
    stdcall("IShellLinkA_SetRelativePath", 0, 3, link::set_relative_path),
    stdcall("IShellLinkA_Resolve", 0, 3, link::resolve),
    stdcall("IShellLinkA_SetPath", 0, 2, link::set_path),
    stdcall("IPersistFile_GetClassID", 0, 2, link::get_class_id),
    stdcall("IPersistFile_IsDirty", 0, 1, link::is_dirty),
    stdcall("IPersistFile_Load", 0, 3, link::load),
    stdcall("IPersistFile_Save", 0, 3, link::save),
    stdcall("IPersistFile_SaveCompleted", 0, 2, link::save_completed),
    stdcall("IPersistFile_GetCurFile", 0, 2, link::get_cur_file),
    stdcall("SHCreateDirectory", 165, 2, fileop::sh_create_directory),
    stdcall("Shell_MergeMenus", 67, 6, info::shell_merge_menus),
    stdcall("ShellExecuteExA", 1027, 1, exec::shell_execute_ex),
    stdcall("ShellExecuteEx", 1028, 1, exec::shell_execute_ex),
    stdcall("SHGetNewLinkInfoA", 1029, 5, info::sh_get_new_link_info),
    stdcall("SHGetNewLinkInfo", 179, 5, info::sh_get_new_link_info),
    stdcall("BrowseDialogProc", 0, 4, browse::dialog_proc),
    stdcall("CommandLineToArgvW", 1030, 2, exec::command_line_to_argv),
];

/// The vtables, by their places in this list (link.rs's constants).
static VTABLES: &[Vtable] = &[
    Vtable {
        name: "IShellLinkA",
        methods: &[
            "IShellLinkA_QueryInterface",
            "IShellLinkA_AddRef",
            "IShellLinkA_Release",
            "IShellLinkA_GetPath",
            "IShellLinkA_GetIDList",
            "IShellLinkA_SetIDList",
            "IShellLinkA_GetDescription",
            "IShellLinkA_SetDescription",
            "IShellLinkA_GetWorkingDirectory",
            "IShellLinkA_SetWorkingDirectory",
            "IShellLinkA_GetArguments",
            "IShellLinkA_SetArguments",
            "IShellLinkA_GetHotkey",
            "IShellLinkA_SetHotkey",
            "IShellLinkA_GetShowCmd",
            "IShellLinkA_SetShowCmd",
            "IShellLinkA_GetIconLocation",
            "IShellLinkA_SetIconLocation",
            "IShellLinkA_SetRelativePath",
            "IShellLinkA_Resolve",
            "IShellLinkA_SetPath",
        ],
    },
    Vtable {
        name: "IPersistFile",
        methods: &[
            "IShellLinkA_QueryInterface",
            "IShellLinkA_AddRef",
            "IShellLinkA_Release",
            "IPersistFile_GetClassID",
            "IPersistFile_IsDirty",
            "IPersistFile_Load",
            "IPersistFile_Save",
            "IPersistFile_SaveCompleted",
            "IPersistFile_GetCurFile",
        ],
    },
];

/// The classes CoCreateInstance makes of SHELL32's (com.rs).
pub static CLASSES: &[com::Class] = &[com::Class {
    clsid: link::CLSID_SHELL_LINK,
    dll: "SHELL32.DLL",
    name: "ShellLink",
    create: link::create,
}];

/// SHELL32's state: the system's.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// The shell links, by their ids.
    links: Vec<Option<link::Link>>,
    /// The folder of Program Manager's group that its commands work in.
    group: Option<String>,
}

crate::state_fields!(State { links, group });

/// Program Manager's execute string `text` (progman.rs): the titles of
/// the windows to open, or None if it failed.
pub fn progman_execute(cpu: &mut Cpu, text: &str) -> Option<Vec<String>> {
    progman::execute(cpu, text)
}

/// Program Manager's answer to a request for `item` (progman.rs).
pub fn progman_request(cpu: &mut Cpu, item: &str) -> Option<String> {
    progman::request(cpu, item)
}

/// Make the directory `path` and those it is in, where they aren't
/// (folders.rs).
pub fn make_directories(cpu: &mut Cpu, path: &str) {
    folders::make_directories(cpu, path);
}

/// The process `pid` ended.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    link::process_ended(cpu, pid);
}

/// The address of SHELL32's function `name`.
fn address_of(name: &str) -> u32 {
    let index = APIS
        .iter()
        .position(|a| a.name == name)
        .unwrap_or_else(|| panic!("SHELL32 has no {}", name));
    super::stubs::stub_address(dll_index(), index)
}

fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("SHELL32 is a built-in DLL")
}

/// DllGetClassObject(clsid, iid, out): SHELL32's classes' factories.
fn dll_get_class_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if super::loader::map_builtin_named(cpu, "OLE32.DLL").is_none() {
        return Some(E_OUTOFMEMORY);
    }
    Some(super::ole32::dll_get_class_object(cpu, DLL.name, args))
}

const TRUE: u32 = 1;
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

/// ShellExecuteW: Windows 95 has no Unicode shell, and the result says
/// the file wasn't found (SE_ERR_FNF).
fn not_found(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(2)
}

/// SHGetMalloc(out): the process's task allocator, as CoGetMalloc's.
/// Its methods are OLE32's, mapped if the program doesn't import it.
fn sh_get_malloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if super::loader::map_builtin_named(cpu, "OLE32.DLL").is_none() {
        return Some(E_OUTOFMEMORY);
    }
    super::ole32::co_get_malloc(cpu, &[1, args[0]])
}
