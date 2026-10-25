//! SHBrowseForFolderA: a modal dialog in which the user picks a folder.
//!
//! Windows 95 shows the folders as a tree. Here the dialog has a list
//! of the folders in the current one, ".." first to go up, and My
//! Computer's drives at the top; a double click opens a folder, and OK
//! picks the one selected in the list, or else the current one. The
//! line under the title shows the current folder, or the text the
//! program sets with BFFM_SETSTATUSTEXT. The program's callback hears
//! BFFM_INITIALIZED and can send BFFM_SETSELECTION and BFFM_ENABLEOK;
//! it doesn't hear BFFM_SELCHANGED.
//!
//! The dialog is made from a template in the task's scratch space, and
//! its procedure is SHELL32's BrowseDialogProc, which runs as a task of
//! its own for each message, as it calls USER32.

use super::{folders, pidl};
use crate::cpu::Cpu;
use crate::win32::kernel32::files;
use crate::win32::task::{self, Next, Task};
use crate::win32::user32::address_of as user32;

/// Where things are in the block the dialog shares with its procedure
/// (the SHBrowseForFolderA task's scratch space).
const B_INFO: u32 = 0;
const B_CURRENT: u32 = 8;
const B_ROOT: u32 = 272;
const B_TEXT: u32 = 536;
const B_RESULT: u32 = 800;
const B_TEMPLATE: u32 = 1064;
const BLOCK_SIZE: u32 = 1600;
const PATH_SIZE: usize = 260;

const ID_LIST: u32 = 0x3741;
const ID_TITLE: u32 = 0x3742;
const ID_PATH: u32 = 0x3743;
const IDOK: u32 = 1;
const IDCANCEL: u32 = 2;

const WM_INITDIALOG: u32 = 0x110;
const WM_COMMAND: u32 = 0x111;
const LBN_DBLCLK: u32 = 2;
const LB_ADDSTRING: u32 = 0x180;
const LB_RESETCONTENT: u32 = 0x184;
const LB_GETTEXT: u32 = 0x189;
const LB_GETCURSEL: u32 = 0x188;
const LB_ERR: u32 = u32::MAX;
const DWL_USER: u32 = 8;

const BFFM_INITIALIZED: u32 = 1;
const BFFM_SETSTATUSTEXTA: u32 = 0x464;
const BFFM_ENABLEOK: u32 = 0x465;
const BFFM_SETSELECTIONA: u32 = 0x466;
const BIF_RETURNONLYFSDIRS: u32 = 1;

/// The task's variables: which of the two it is, then their own.
const V_MODE: usize = 0;
const MODE_BROWSE: u32 = 0;
const MODE_PROC: u32 = 1;
// SHBrowseForFolderA's.
const V_INFO: usize = 1;
// The dialog procedure's.
const V_HWND: usize = 1;
const V_MSG: usize = 2;
const V_WP: usize = 3;
const V_LP: usize = 4;
const V_BLOCK: usize = 5;
const V_INDEX: usize = 6;
const V_AFTER: usize = 7;

/// SHBrowseForFolderA(info): the item id list of the folder picked, or
/// NULL if the user cancelled.
pub fn sh_browse_for_folder(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let info = args[0];
    if info == 0 || !cpu.bus.guest_probe(info, 32, true) {
        return Some(0);
    }
    task::start(cpu, task::SHELL32, vec![MODE_BROWSE, info], BLOCK_SIZE)
}

/// BrowseDialogProc(hwnd, message, wParam, lParam): the dialog's
/// procedure.
pub fn dialog_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hwnd, msg, wp, lp] = args[..] else {
        return Some(0);
    };
    let handled = matches!(
        msg,
        WM_INITDIALOG | WM_COMMAND | BFFM_SETSTATUSTEXTA | BFFM_ENABLEOK | BFFM_SETSELECTIONA
    );
    if !handled {
        return Some(0);
    }
    task::start(cpu, task::SHELL32, vec![MODE_PROC, hwnd, msg, wp, lp], 0)
}

/// The steps of both tasks.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.var(V_MODE) == MODE_BROWSE {
        browse_step(cpu, t, value)
    } else {
        proc_step(cpu, t, value)
    }
}

fn put_string(cpu: &mut Cpu, at: u32, text: &str) {
    let mut bytes = crate::dosstr::to_bytes(text);
    bytes.truncate(PATH_SIZE - 1);
    bytes.push(0);
    cpu.bus.guest_write_bytes(at, &bytes);
}

fn get_string(cpu: &mut Cpu, at: u32) -> String {
    let bytes = cpu.bus.guest_read_asciiz(at, PATH_SIZE);
    crate::dosstr::from_bytes(&bytes)
}

/// A UTF-16 string as a dialog template has it.
fn wide(out: &mut Vec<u8>, text: &str) {
    for c in text.encode_utf16() {
        out.extend(c.to_le_bytes());
    }
    out.extend([0, 0]);
}

/// The dialog's template.
fn template() -> Vec<u8> {
    const WS_POPUP: u32 = 0x8000_0000;
    const WS_CHILD: u32 = 0x4000_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const WS_CAPTION: u32 = 0x00C0_0000;
    const WS_BORDER: u32 = 0x0080_0000;
    const WS_SYSMENU: u32 = 0x0008_0000;
    const WS_VSCROLL: u32 = 0x0020_0000;
    const WS_TABSTOP: u32 = 0x0001_0000;
    const DS_MODALFRAME: u32 = 0x80;
    const DS_SETFONT: u32 = 0x40;
    const DS_CENTER: u32 = 0x800;
    const SS_NOPREFIX: u32 = 0x80;
    const SS_LEFTNOWORDWRAP: u32 = 0xC;
    const LBS_NOTIFY: u32 = 0x1;
    const LBS_HASSTRINGS: u32 = 0x40;
    const LBS_NOINTEGRALHEIGHT: u32 = 0x100;
    const BS_DEFPUSHBUTTON: u32 = 0x1;
    let child = WS_CHILD | WS_VISIBLE;
    // Class atom, id, style, x, y, width, height, text.
    let items: [(u16, u32, u32, [i16; 4], &str); 5] = [
        (0x82, ID_TITLE, child | SS_NOPREFIX, [6, 6, 200, 24], ""),
        (
            0x82,
            ID_PATH,
            child | SS_NOPREFIX | SS_LEFTNOWORDWRAP,
            [6, 32, 200, 10],
            "",
        ),
        (
            0x83,
            ID_LIST,
            child
                | WS_BORDER
                | WS_VSCROLL
                | WS_TABSTOP
                | LBS_NOTIFY
                | LBS_HASSTRINGS
                | LBS_NOINTEGRALHEIGHT,
            [6, 44, 200, 116],
            "",
        ),
        (
            0x80,
            IDOK,
            child | WS_TABSTOP | BS_DEFPUSHBUTTON,
            [100, 166, 50, 14],
            "OK",
        ),
        (
            0x80,
            IDCANCEL,
            child | WS_TABSTOP,
            [156, 166, 50, 14],
            "Cancel",
        ),
    ];
    let mut out = Vec::new();
    let style = WS_POPUP | WS_CAPTION | WS_SYSMENU | DS_MODALFRAME | DS_SETFONT | DS_CENTER;
    out.extend(style.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend((items.len() as u16).to_le_bytes());
    for v in [0i16, 0, 212, 188] {
        out.extend(v.to_le_bytes());
    }
    out.extend([0, 0, 0, 0]);
    wide(&mut out, "Browse for Folder");
    out.extend(8u16.to_le_bytes());
    wide(&mut out, "MS Sans Serif");
    for (class, id, style, rect, text) in items {
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        out.extend(style.to_le_bytes());
        out.extend(0u32.to_le_bytes());
        for v in rect {
            out.extend(v.to_le_bytes());
        }
        out.extend((id as u16).to_le_bytes());
        out.extend(0xFFFFu16.to_le_bytes());
        out.extend(class.to_le_bytes());
        wide(&mut out, text);
        out.extend(0u16.to_le_bytes());
    }
    out
}

fn browse_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let info = t.var(V_INFO);
    let block = t.scratch;
    match t.step {
        0 => {
            let root_list = cpu.bus.guest_read_32(info + 4);
            let root = match root_list {
                0 => String::new(),
                at => super::il::path_of(cpu, at).unwrap_or_default(),
            };
            cpu.bus.guest_write_32(block + B_INFO, info);
            put_string(cpu, block + B_CURRENT, &root);
            put_string(cpu, block + B_ROOT, &root);
            cpu.bus.guest_write_bytes(block + B_TEMPLATE, &template());
            let owner = cpu.bus.guest_read_32(info);
            let proc = super::address_of("BrowseDialogProc");
            let args = [0, block + B_TEMPLATE, owner, proc, block];
            Next::call(user32("DialogBoxIndirectParamA"), &args, 1)
        }
        1 => {
            if value != 1 {
                return Next::Done(0);
            }
            let path = get_string(cpu, block + B_RESULT);
            let items = if path.is_empty() {
                // My Computer.
                folders::items(cpu, 0x11)
            } else {
                pidl::from_path(cpu, &path, false)
            };
            let Some(items) = items else {
                return Next::Done(0);
            };
            let list = pidl::alloc_items(cpu, &items);
            let name_at = cpu.bus.guest_read_32(info + 8);
            if name_at != 0 && cpu.bus.guest_probe(name_at, PATH_SIZE, true) {
                put_string(cpu, name_at, &display_name(&path));
            }
            // SHELL32's folder icon, or My Computer's.
            let image = if path.is_empty() { 15 } else { 3 };
            cpu.bus.guest_write_32(info + 28, image);
            Next::Done(list)
        }
        _ => Next::Done(0),
    }
}

/// The name Explorer shows for the folder `path`.
fn display_name(path: &str) -> String {
    if path.is_empty() {
        "My Computer".to_string()
    } else if path.len() <= 3 {
        format!("({})", &path[..2])
    } else {
        path.rsplit('\\').next().unwrap_or(path).to_string()
    }
}

/// The folder above `path`, not above `root`: My Computer ("") above a
/// drive's root.
fn parent(path: &str, root: &str) -> String {
    if path.eq_ignore_ascii_case(root) {
        return path.to_string();
    }
    if path.len() <= 3 {
        return String::new();
    }
    match path.rsplit_once('\\') {
        Some((up, _)) if up.len() <= 2 => format!("{}\\", up),
        Some((up, _)) => up.to_string(),
        None => String::new(),
    }
}

/// The folder the list's entry `entry` names, in the folder `path`.
fn open(path: &str, root: &str, entry: &str) -> String {
    if entry == ".." {
        parent(path, root)
    } else if path.is_empty() {
        entry.to_string()
    } else {
        format!("{}\\{}", path.trim_end_matches('\\'), entry)
    }
}

/// The list's entries for the folder `path`.
fn entries(cpu: &mut Cpu, path: &str, root: &str) -> Vec<String> {
    if path.is_empty() {
        return ('C'..='Z')
            .map(|d| format!("{}:\\", d))
            .filter(|d| files::find(cpu, d, "*").is_ok())
            .collect();
    }
    let mut list = Vec::new();
    if !path.eq_ignore_ascii_case(root) {
        list.push("..".to_string());
    }
    let mut dirs: Vec<String> = files::find(cpu, path, "*")
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.attributes & 0x10 != 0 && e.name != "." && e.name != "..")
        .map(|e| e.name)
        .collect();
    dirs.sort_by_key(|d| d.to_ascii_lowercase());
    list.extend(dirs);
    list
}

fn send_list(t: &Task, msg: u32, wp: u32, lp: u32, step: u16) -> Next {
    let args = [t.var(V_HWND), ID_LIST, msg, wp, lp];
    Next::call(user32("SendDlgItemMessageA"), &args, step)
}

fn go(step: u16) -> Next {
    Next::call(0, &[], step)
}

/// Fill the list, then go on at `after`.
fn fill(t: &mut Task, after: u16) -> Next {
    t.set(V_AFTER, after as u32);
    go(100)
}

fn proc_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let (hwnd, msg, wp, lp) = (t.var(V_HWND), t.var(V_MSG), t.var(V_WP), t.var(V_LP));
    let block = t.var(V_BLOCK);
    match t.step {
        0 => {
            if msg == WM_INITDIALOG {
                t.set(V_BLOCK, lp);
                let args = [hwnd, DWL_USER, lp];
                return Next::call(user32("SetWindowLongA"), &args, 10);
            }
            Next::call(user32("GetWindowLongA"), &[hwnd, DWL_USER], 1)
        }
        1 => {
            t.set(V_BLOCK, value);
            if value == 0 {
                return Next::Done(0);
            }
            match msg {
                WM_COMMAND => match (wp & 0xFFFF, wp >> 16) {
                    (IDOK, _) => send_list(t, LB_GETCURSEL, 0, 0, 30),
                    (IDCANCEL, _) => Next::call(user32("EndDialog"), &[hwnd, 0], 99),
                    (ID_LIST, LBN_DBLCLK) => send_list(t, LB_GETCURSEL, 0, 0, 40),
                    _ => Next::Done(0),
                },
                BFFM_SETSELECTIONA => {
                    let path = match (wp, lp) {
                        (_, 0) => None,
                        (0, at) => super::il::path_of(cpu, at),
                        (_, at) => crate::win32::user32::string_at(cpu, at),
                    };
                    let Some(path) = path else {
                        return Next::Done(0);
                    };
                    let full = files::full_path(cpu, &path);
                    if !files::attributes(cpu, &full).is_ok_and(|a| a & 0x10 != 0) {
                        return Next::Done(0);
                    }
                    let full = if full.len() > 3 {
                        full.trim_end_matches('\\').to_string()
                    } else {
                        full
                    };
                    put_string(cpu, value + B_CURRENT, &full);
                    fill(t, 98)
                }
                BFFM_ENABLEOK => Next::call(user32("GetDlgItem"), &[hwnd, IDOK], 60),
                BFFM_SETSTATUSTEXTA => {
                    Next::call(user32("SetDlgItemTextA"), &[hwnd, ID_PATH, lp], 98)
                }
                _ => Next::Done(0),
            }
        }
        // WM_INITDIALOG: the title, the list, then the callback.
        10 => {
            let info = cpu.bus.guest_read_32(block + B_INFO);
            let title = cpu.bus.guest_read_32(info + 12);
            if title == 0 {
                return go(11);
            }
            Next::call(user32("SetDlgItemTextA"), &[hwnd, ID_TITLE, title], 11)
        }
        11 => fill(t, 12),
        12 => {
            let info = cpu.bus.guest_read_32(block + B_INFO);
            let callback = cpu.bus.guest_read_32(info + 20);
            if callback == 0 {
                return Next::Done(1);
            }
            let param = cpu.bus.guest_read_32(info + 24);
            Next::call(callback, &[hwnd, BFFM_INITIALIZED, 0, param], 13)
        }
        13 => Next::Done(1),
        // OK: the selected entry's folder, or the current one.
        30 | 40 => {
            let next = t.step + 1;
            if value == LB_ERR {
                put_string(cpu, block + B_TEXT, "");
                return go(next);
            }
            send_list(t, LB_GETTEXT, value, block + B_TEXT, next)
        }
        31 => {
            let entry = get_string(cpu, block + B_TEXT);
            let current = get_string(cpu, block + B_CURRENT);
            let root = get_string(cpu, block + B_ROOT);
            let picked = if entry.is_empty() || entry == ".." {
                current
            } else {
                open(&current, &root, &entry)
            };
            let info = cpu.bus.guest_read_32(block + B_INFO);
            let flags = cpu.bus.guest_read_32(info + 16);
            if picked.is_empty() && flags & BIF_RETURNONLYFSDIRS != 0 {
                return Next::Done(1);
            }
            put_string(cpu, block + B_RESULT, &picked);
            Next::call(user32("EndDialog"), &[hwnd, 1], 99)
        }
        // A double click: into the folder.
        41 => {
            let entry = get_string(cpu, block + B_TEXT);
            if entry.is_empty() {
                return Next::Done(1);
            }
            let current = get_string(cpu, block + B_CURRENT);
            let root = get_string(cpu, block + B_ROOT);
            let next = open(&current, &root, &entry);
            put_string(cpu, block + B_CURRENT, &next);
            fill(t, 99)
        }
        60 => Next::call(user32("EnableWindow"), &[value, (lp != 0) as u32], 98),
        98 => Next::Done(0),
        99 => Next::Done(1),
        // The list: the current folder's line, then its entries.
        100 => {
            let current = get_string(cpu, block + B_CURRENT);
            let shown = if current.is_empty() {
                "My Computer".to_string()
            } else {
                current
            };
            put_string(cpu, block + B_TEXT, &shown);
            let args = [hwnd, ID_PATH, block + B_TEXT];
            Next::call(user32("SetDlgItemTextA"), &args, 101)
        }
        101 => {
            t.set(V_INDEX, 0);
            send_list(t, LB_RESETCONTENT, 0, 0, 102)
        }
        102 => {
            let current = get_string(cpu, block + B_CURRENT);
            let root = get_string(cpu, block + B_ROOT);
            let list = entries(cpu, &current, &root);
            let i = t.var(V_INDEX) as usize;
            let Some(entry) = list.get(i) else {
                return go(t.var(V_AFTER) as u16);
            };
            put_string(cpu, block + B_TEXT, entry);
            t.set(V_INDEX, i as u32 + 1);
            send_list(t, LB_ADDSTRING, 0, block + B_TEXT, 102)
        }
        _ => Next::Done(0),
    }
}

#[cfg(test)]
mod tests {
    use super::{open, parent};

    #[test]
    fn folders_open_and_go_up() {
        assert_eq!(open("", "", "C:\\"), "C:\\");
        assert_eq!(open("C:\\", "", "GAMES"), "C:\\GAMES");
        assert_eq!(open("C:\\GAMES", "", "X"), "C:\\GAMES\\X");
        assert_eq!(parent("C:\\GAMES\\X", ""), "C:\\GAMES");
        assert_eq!(parent("C:\\GAMES", ""), "C:\\");
        assert_eq!(parent("C:\\", ""), "");
        assert_eq!(parent("C:\\GAMES", "C:\\GAMES"), "C:\\GAMES");
    }
}
