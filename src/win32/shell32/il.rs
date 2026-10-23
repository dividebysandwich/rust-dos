//! The functions on item ID lists (pidl.rs): ILClone, ILCombine and the
//! rest, which Windows 95 exports by number alone, and the functions
//! that turn special folders and paths into lists and lists into paths.
//! On Windows 95, those that take a string take an ANSI one.

use super::folders;
use super::pidl::{self, Item};
use crate::cpu::Cpu;
use crate::win32::com::{E_INVALIDARG, E_OUTOFMEMORY, S_OK};
use crate::win32::ole32;
use crate::win32::user32::string_at;

const TRUE: u32 = 1;
const FALSE: u32 = 0;
const E_FAIL: u32 = 0x8000_4005;
/// HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND).
const E_FILE_NOT_FOUND: u32 = 0x8007_0002;
const MAX_PATH: usize = 260;

/// The items of the list at `at`, or None for NULL or a list that isn't
/// readable.
fn items(cpu: &mut Cpu, at: u32) -> Option<Vec<Item>> {
    if at == 0 {
        return None;
    }
    pidl::read(cpu, at)
}

/// The address of the item `n` of the list at `at`, which `items` read.
fn item_at(cpu: &mut Cpu, at: u32, n: usize) -> u32 {
    let mut p = at;
    for _ in 0..n {
        p += cpu.bus.guest_read_16(p) as u32;
    }
    p
}

/// SHAlloc(size): the task allocator's Alloc.
pub fn sh_alloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(ole32::task_alloc(cpu, args[0]))
}

/// SHFree(block) and ILFree(list): the task allocator's Free.
pub fn sh_free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] != 0 {
        ole32::task_free(cpu, args[0]);
    }
    Some(0)
}

/// ILGetSize(list): its bytes, the end's two counted; 0 for NULL.
pub fn il_get_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(list) = items(cpu, args[0]) else {
        return Some(0);
    };
    Some(pidl::encode(&list).len() as u32)
}

/// ILGetNext(item): the item after it, or NULL at the list's end.
pub fn il_get_next(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if at == 0 || !cpu.bus.guest_probe(at, 2, false) {
        return Some(0);
    }
    let cb = cpu.bus.guest_read_16(at) as u32;
    if cb == 0 {
        return Some(0);
    }
    let next = at + cb;
    if !cpu.bus.guest_probe(next, 2, false) || cpu.bus.guest_read_16(next) == 0 {
        return Some(0);
    }
    Some(next)
}

/// ILFindLastID(list): its last item, or the list itself when empty.
pub fn il_find_last_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(list) = items(cpu, args[0]) else {
        return Some(0);
    };
    Some(item_at(cpu, args[0], list.len().saturating_sub(1)))
}

/// ILRemoveLastID(list): the last item cut off in place.
pub fn il_remove_last_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(list) = items(cpu, args[0]) else {
        return Some(FALSE);
    };
    if list.is_empty() {
        return Some(FALSE);
    }
    let last = item_at(cpu, args[0], list.len() - 1);
    cpu.bus.guest_write_16(last, 0);
    Some(TRUE)
}

/// ILClone(list): a copy.
pub fn il_clone(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(list) = items(cpu, args[0]) else {
        return Some(0);
    };
    Some(pidl::alloc_items(cpu, &list))
}

/// ILCloneFirst(list): a list of its first item.
pub fn il_clone_first(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(list) = items(cpu, args[0]) else {
        return Some(0);
    };
    Some(pidl::alloc_items(cpu, &list[..list.len().min(1)]))
}

/// ILCombine(first, second): a new list of the two's items.
pub fn il_combine(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = items(cpu, args[0]);
    let b = items(cpu, args[1]);
    let list = match (a, b) {
        (None, None) => return Some(0),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (Some(mut a), Some(b)) => {
            a.extend(b);
            a
        }
    };
    Some(pidl::alloc_items(cpu, &list))
}

/// ILAppendID(list, item, at the end): a new list with the item added
/// at the end or the start; the old list is freed.
pub fn il_append_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [list, item, end] = args[..] else {
        return Some(0);
    };
    let mut all = if list == 0 {
        Vec::new()
    } else {
        match items(cpu, list) {
            Some(all) => all,
            None => return Some(0),
        }
    };
    let Some(mut added) = items(cpu, item) else {
        return Some(0);
    };
    added.truncate(1);
    if end != 0 {
        all.extend(added);
    } else {
        added.extend(all);
        all = added;
    }
    let new = pidl::alloc_items(cpu, &all);
    if new != 0 && list != 0 {
        ole32::task_free(cpu, list);
    }
    Some(new)
}

/// Whether the list `a` is the start of `b`, and how many of `b`'s
/// items that is.
fn prefix(a: &[Item], b: &[Item]) -> bool {
    a.len() <= b.len() && a.iter().zip(b).all(|(x, y)| pidl::same_item(x, y))
}

/// ILIsEqual(a, b).
pub fn il_is_equal(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (Some(a), Some(b)) = (items(cpu, args[0]), items(cpu, args[1])) else {
        return Some(FALSE);
    };
    Some((a.len() == b.len() && prefix(&a, &b)) as u32)
}

/// ILIsParent(parent, child, immediate): whether `child` is in
/// `parent`, right in it for `immediate`.
pub fn il_is_parent(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (Some(a), Some(b)) = (items(cpu, args[0]), items(cpu, args[1])) else {
        return Some(FALSE);
    };
    let below = b.len() > a.len() && prefix(&a, &b);
    Some((below && (args[2] == 0 || b.len() == a.len() + 1)) as u32)
}

/// ILFindChild(parent, child): where in `child` the part under
/// `parent` starts, or NULL if it isn't under it.
pub fn il_find_child(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (Some(a), Some(b)) = (items(cpu, args[0]), items(cpu, args[1])) else {
        return Some(0);
    };
    if !prefix(&a, &b) {
        return Some(0);
    }
    Some(item_at(cpu, args[1], a.len()))
}

/// The ANSI path at `at`, or None.
fn path_arg(cpu: &mut Cpu, at: u32) -> Option<String> {
    if at == 0 {
        return None;
    }
    string_at(cpu, at).filter(|p| !p.is_empty())
}

/// ILCreateFromPath(path): the list of a file or folder that is there.
pub fn il_create_from_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(path) = path_arg(cpu, args[0]) else {
        return Some(0);
    };
    match pidl::from_path(cpu, &path, false) {
        Some(list) => Some(pidl::alloc_items(cpu, &list)),
        None => Some(0),
    }
}

/// SHSimpleIDListFromPath(path): its list, made from the names where
/// nothing is on disk.
pub fn sh_simple_id_list_from_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(path) = path_arg(cpu, args[0]) else {
        return Some(0);
    };
    match pidl::from_path(cpu, &path, true) {
        Some(list) => Some(pidl::alloc_items(cpu, &list)),
        None => Some(0),
    }
}

/// SHILCreateFromPath(path, out, attributes): the list of a file or
/// folder that is there; the attributes asked for, SFGAO_FOLDER of
/// them, when `attributes` isn't NULL.
pub fn sh_il_create_from_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [path, out, attributes] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(E_INVALIDARG);
    }
    cpu.bus.guest_write_32(out, 0);
    let Some(path) = path_arg(cpu, path) else {
        return Some(E_INVALIDARG);
    };
    let Some(list) = pidl::from_path(cpu, &path, false) else {
        return Some(E_FILE_NOT_FOUND);
    };
    let at = pidl::alloc_items(cpu, &list);
    if at == 0 {
        return Some(E_OUTOFMEMORY);
    }
    cpu.bus.guest_write_32(out, at);
    if attributes != 0 && cpu.bus.guest_probe(attributes, 4, true) {
        const SFGAO_FOLDER: u32 = 0x2000_0000;
        let asked = cpu.bus.guest_read_32(attributes);
        let folder = list.last().is_some_and(|i| i[0] == pidl::PT_FOLDER);
        let has = if folder { SFGAO_FOLDER } else { 0 };
        cpu.bus.guest_write_32(attributes, asked & has);
    }
    Some(S_OK)
}

/// SHGetSpecialFolderLocation(hwnd, csidl, out): the folder's list.
pub fn sh_get_special_folder_location(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [_, csidl, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(E_INVALIDARG);
    }
    cpu.bus.guest_write_32(out, 0);
    if folders::folder(csidl).is_none() {
        return Some(E_INVALIDARG);
    }
    let Some(list) = folders::items(cpu, csidl) else {
        return Some(E_FAIL);
    };
    let at = pidl::alloc_items(cpu, &list);
    if at == 0 {
        return Some(E_OUTOFMEMORY);
    }
    cpu.bus.guest_write_32(out, at);
    Some(S_OK)
}

/// Write `path` and its 0 at `at`, in MAX_PATH bytes.
fn write_path(cpu: &mut Cpu, at: u32, path: &str) -> bool {
    let mut bytes = crate::dosstr::to_bytes(path);
    bytes.truncate(MAX_PATH - 1);
    bytes.push(0);
    if !cpu.bus.guest_probe(at, bytes.len(), true) {
        return false;
    }
    cpu.bus.guest_write_bytes(at, &bytes);
    true
}

/// The path the list at `at` names, if it names one on disk.
pub fn path_of(cpu: &mut Cpu, at: u32) -> Option<String> {
    let list = items(cpu, at)?;
    let desktop = folders::desktop(cpu);
    pidl::path_of(&list, &desktop)
}

/// SHGetPathFromIDList(list, path): the path of a file or folder's
/// list; FALSE and an empty string for others.
pub fn sh_get_path_from_id_list(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [list, out] = args[..] else {
        return Some(FALSE);
    };
    if out == 0 {
        return Some(FALSE);
    }
    match path_of(cpu, list) {
        Some(path) => Some(write_path(cpu, out, &path) as u32),
        None => {
            if cpu.bus.guest_probe(out, 1, true) {
                cpu.bus.guest_write_8(out, 0);
            }
            Some(FALSE)
        }
    }
}

/// SHGetSpecialFolderPathA(hwnd, path, csidl, create): the path of a
/// special folder on disk. The folder is made whether or not `create`
/// asks, as Explorer has made it on Windows.
pub fn sh_get_special_folder_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [_, out, csidl, _] = args[..] else {
        return Some(FALSE);
    };
    let Some(path) = folders::path(cpu, csidl) else {
        return Some(FALSE);
    };
    if out == 0 {
        return Some(FALSE);
    }
    Some(write_path(cpu, out, &path) as u32)
}

/// StrRetToStrN(buffer, size, strret, list): a STRRET's string, ANSI
/// on Windows 95, copied to `buffer`; a STRRET_WSTR's string is freed.
pub fn str_ret_to_str_n(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [dest, size, strret, list] = args[..] else {
        return Some(FALSE);
    };
    if dest == 0 || strret == 0 || !cpu.bus.guest_probe(strret, 8, false) {
        return Some(FALSE);
    }
    let kind = cpu.bus.guest_read_32(strret);
    let text: Vec<u8> = match kind {
        // STRRET_WSTR.
        0 => {
            let at = cpu.bus.guest_read_32(strret + 4);
            let mut units = Vec::new();
            while units.len() < 0x8000 && cpu.bus.guest_probe(at + 2 * units.len() as u32, 2, false)
            {
                let c = cpu.bus.guest_read_16(at + 2 * units.len() as u32);
                if c == 0 {
                    break;
                }
                units.push(c);
            }
            if at != 0 {
                ole32::task_free(cpu, at);
            }
            units
                .iter()
                .map(|&c| if c < 0x100 { c as u8 } else { b'?' })
                .collect()
        }
        // STRRET_OFFSET, from the list's start.
        1 if list != 0 => {
            let at = list + cpu.bus.guest_read_32(strret + 4);
            crate::win32::user32::bytes_at(cpu, at).unwrap_or_default()
        }
        // STRRET_CSTR.
        2 => crate::win32::user32::bytes_at(cpu, strret + 4).unwrap_or_default(),
        _ => Vec::new(),
    };
    if size == 0 {
        return Some(TRUE);
    }
    let n = text.len().min(size as usize - 1);
    if !cpu.bus.guest_probe(dest, n + 1, true) {
        return Some(FALSE);
    }
    cpu.bus.guest_write_bytes(dest, &text[..n]);
    cpu.bus.guest_write_8(dest + n as u32, 0);
    Some(TRUE)
}
