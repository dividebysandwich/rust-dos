//! Windows 3.1's file dialog, which Windows 98 shows a program that has
//! a hook or a template but doesn't ask for the Explorer style: the file
//! name, the files, the folders, the file type and the drive. Its hook
//! sees every message before the dialog does. Ported from Wine 1.2.3's
//! comdlg32 (filedlg31.c and filedlg.c; see filedlg.rs for the
//! copyright notices).

use super::filedlg::*;
use super::{FNERR_BUFFERTOOSMALL, set_extended_error};
use crate::cpu::Cpu;
use crate::win32::comctl32::script::{Arg, Ctx, Op};
use crate::win32::kernel32;
use crate::win32::user32;
use crate::win32::user32::message::{WM_COMMAND, WM_DRAWITEM, WM_INITDIALOG, WM_MEASUREITEM};

const CBN_SELCHANGE: u32 = 1;
const LBN_SELCHANGE: u32 = 1;
const LBN_DBLCLK: u32 = 2;
/// The code of the hook's LBSELCHSTRING message for a new selection.
const CD_LBSELCHANGE: u32 = 0;
/// Long names in the lists (OFN_LONGNAMES).
const OFN_LONGNAMES: u32 = 0x0020_0000;

// Styles.
const LBS_STANDARD: u32 = 0x0001 | 0x0002 | WS_VSCROLL | WS_BORDER;
const LBS_OWNERDRAWFIXED: u32 = 0x0010;
const LBS_HASSTRINGS: u32 = 0x0040;
const LBS_DISABLENOSCROLL: u32 = 0x1000;
const ES_AUTOHSCROLL: u32 = 0x0080;
const ES_OEMCONVERT: u32 = 0x0400;
const CBS_DROPDOWNLIST: u32 = 0x0003;
const CBS_OWNERDRAWFIXED: u32 = 0x0010;
const CBS_AUTOHSCROLL: u32 = 0x0040;
const CBS_SORT: u32 = 0x0100;
const CBS_HASSTRINGS: u32 = 0x0200;
const SS_NOPREFIX: u32 = 0x0080;
const BS_DEFPUSHBUTTON: u32 = 0x0001;
const BS_AUTOCHECKBOX: u32 = 0x0003;

/// The dialog's template (Wine's OPEN_FILE and SAVE_FILE).
pub fn template(save: bool) -> Vec<u32> {
    let style = DS_MODALFRAME | WS_POPUP | WS_CAPTION | WS_SYSMENU;
    let caption = if save { "Save As" } else { "Open" };
    let mut t = Template::new(style, (36, 24, 275, 134), caption);
    let v = WS_VISIBLE;
    let label = v | WS_GROUP;
    t.item(label, (6, 6, 76, 9), STC3, ATOM_STATIC, "File &Name:");
    let edit = v | ES_AUTOHSCROLL | ES_OEMCONVERT | WS_BORDER | WS_TABSTOP;
    t.item(edit, (6, 16, 90, 12), EDT1, ATOM_EDIT, "");
    let list = v | LBS_STANDARD | LBS_OWNERDRAWFIXED | LBS_HASSTRINGS | LBS_DISABLENOSCROLL;
    t.item(list | WS_TABSTOP, (6, 32, 90, 68), LST1, ATOM_LISTBOX, "");
    t.item(label, (110, 6, 92, 9), 0xFFFF, ATOM_STATIC, "&Directories:");
    t.item(label | SS_NOPREFIX, (110, 18, 92, 9), STC1, ATOM_STATIC, "");
    t.item(list | WS_TABSTOP, (110, 32, 92, 68), LST2, ATOM_LISTBOX, "");
    let types = if save {
        "Save File as &Type:"
    } else {
        "List Files of &Type:"
    };
    t.item(label, (6, 104, 90, 9), STC2, ATOM_STATIC, types);
    let combo = v | CBS_DROPDOWNLIST | CBS_AUTOHSCROLL | WS_BORDER | WS_VSCROLL | WS_TABSTOP;
    t.item(combo, (6, 114, 90, 36), CMB1, ATOM_COMBOBOX, "");
    t.item(label, (110, 104, 92, 9), STC4, ATOM_STATIC, "Dri&ves:");
    let drives = combo | CBS_OWNERDRAWFIXED | CBS_SORT | CBS_HASSTRINGS;
    t.item(drives, (110, 114, 92, 68), CMB2, ATOM_COMBOBOX, "");
    let button = v | WS_GROUP | WS_TABSTOP;
    t.item(
        button | BS_DEFPUSHBUTTON,
        (208, 6, 56, 14),
        IDOK,
        ATOM_BUTTON,
        "OK",
    );
    t.item(button, (208, 24, 56, 14), IDCANCEL, ATOM_BUTTON, "Cancel");
    t.item(button, (208, 46, 56, 14), PSH_HELP, ATOM_BUTTON, "&Help");
    let check = v | BS_AUTOCHECKBOX | WS_GROUP | WS_TABSTOP;
    t.item(check, (208, 68, 50, 12), CHX1, ATOM_BUTTON, "&Read Only");
    t.words()
}

/// The dialog procedure (Wine's FD32_FileOpenDlgProc): the hook first,
/// then the dialog's own handling if the hook returns FALSE.
pub fn proc_(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hwnd, msg, wp, lp] = args[..] else {
        return Some(0);
    };
    if msg != WM_INITDIALOG
        && let Some(hook) = dialog(cpu, hwnd).map(|d| d.hook)
        && hook != 0
    {
        let mut o = Ops::new();
        let args = [hwnd, msg, wp, lp].map(Arg::V);
        o.call(hook, &args);
        o.then(T_HOOKED, &[msg, wp, lp]);
        return o.run(cpu, hwnd, 0);
    }
    let (o, result) = own(cpu, hwnd, msg, wp, lp)?;
    o.run(cpu, hwnd, result)
}

// The continuations.
const T_HOOKED: u16 = OLD_FIRST;
const T_FILE_SELECTED: u16 = OLD_FIRST + 1;
const T_FOLDER: u16 = OLD_FIRST + 2;
const T_TYPE: u16 = OLD_FIRST + 3;
const T_DRIVE: u16 = OLD_FIRST + 4;
const T_TEXT: u16 = OLD_FIRST + 5;
const T_OK: u16 = OLD_FIRST + 6;
const T_CONFIRM: u16 = OLD_FIRST + 7;
const T_FILEOK: u16 = OLD_FIRST + 8;

pub fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    let dlg = c.hwnd;
    if id == T_HOOKED {
        if c.last != 0 {
            return vec![Op::Return(Arg::V(c.last))];
        }
        let Some((mut o, result)) = own(cpu, dlg, c.word(0), c.word(1), c.word(2)) else {
            return vec![Op::Return(Arg::V(0))];
        };
        o.ret(result);
        return o.0;
    }
    let Some(mut d) = dialog(cpu, dlg).cloned() else {
        return Vec::new();
    };
    let mut o = Ops::new();
    let index = c.last;
    match id {
        T_FILE_SELECTED => {
            let lst1 = item(cpu, dlg, LST1);
            if let Some((text, _)) = user32::controls::listbox::item(cpu, lst1, index as i32) {
                o.set_item_text(dlg, EDT1, &crate::dosstr::from_bytes(&text));
            }
            selected(&d, &mut o, LST1, index);
        }
        T_FOLDER => {
            if let Some((dir, _)) = d.look_in.get(index as usize).cloned() {
                scan(cpu, &mut d, &dir, &mut o);
            }
            selected(&d, &mut o, LST2, index);
        }
        T_TYPE => {
            if let Some((_, patterns)) = d.filters.get(index as usize).cloned() {
                d.index = index + 1;
                cpu.bus.guest_write_32(d.ofn + 4 * O_INDEX as u32, d.index);
                d.filter = patterns.to_ascii_lowercase();
                o.set_item_text(dlg, EDT1, &d.filter);
                let dir = d.dir.clone();
                scan(cpu, &mut d, &dir, &mut o);
            }
            selected(&d, &mut o, CMB1, index);
        }
        T_DRIVE => {
            if let Some(&drive) = d.drives.get(index as usize) {
                let dir = kernel32::current_directory(cpu, drive);
                scan(cpu, &mut d, &dir, &mut o);
            }
            selected(&d, &mut o, CMB2, index);
        }
        T_TEXT => {
            d.text = scratch_text(cpu, c, B_TEXT, TEXT_SIZE);
        }
        T_OK => ok(cpu, &mut d, c.last & 3 == 1, &mut o),
        T_CONFIRM => {
            if c.last == IDYES {
                finish(cpu, &mut d, c.word(0) != 0, &mut o);
            }
        }
        T_FILEOK if c.last == 0 => {
            o.user("EndDialog", &[dlg, 1]);
        }
        _ => {}
    }
    if let Some(slot) = dialog(cpu, dlg) {
        *slot = d;
    }
    o.0
}

/// Tell the hook of a new selection in `control` (LBSELCHSTRING).
fn selected(d: &FileDlg, o: &mut Ops, control: u32, index: u32) {
    if d.hook != 0 {
        let lp = index & 0xFFFF | CD_LBSELCHANGE << 16;
        let args = [d.hwnd, d.lbselch, control, lp].map(Arg::V);
        o.call(d.hook, &args);
    }
}

/// The dialog's own handling of a message: the ops to run, and what it
/// returns.
fn own(cpu: &mut Cpu, dlg: u32, msg: u32, wp: u32, lp: u32) -> Option<(Ops, u32)> {
    let mut o = Ops::new();
    let result = match msg {
        WM_INITDIALOG => return init(cpu, dlg, lp),
        WM_MEASUREITEM => {
            cpu.bus.guest_write_32(lp + 16, 16);
            1
        }
        WM_DRAWITEM => draw(cpu, dlg, lp)?,
        WM_COMMAND => command(cpu, dlg, wp, &mut o),
        _ => 0,
    };
    Some((o, result))
}

/// WM_INITDIALOG (Wine's FD31_WMInitDialog): the filters, the file name,
/// the drives and the folder; then the hook's WM_INITDIALOG, with the
/// program's OPENFILENAMEA.
fn init(cpu: &mut Cpu, dlg: u32, id: u32) -> Option<(Ops, u32)> {
    let mut o = Ops::new();
    let Some(mut d) = super::state(cpu).dialogs.get(&id).cloned() else {
        return Some((o, 0));
    };
    d.hwnd = dlg;
    if !d.title.is_empty() {
        o.set_text(dlg, &d.title);
    }
    let cmb1 = item(cpu, dlg, CMB1);
    for (title, _) in &d.filters {
        o.send_text(cmb1, CB_ADDSTRING, 0, title);
    }
    if d.index == 0 && !d.custom_filter {
        d.index = 1;
    }
    o.send(cmb1, CB_SETCURSEL, d.index.wrapping_sub(1), 0);
    if let Some((_, patterns)) = d.filters.get(d.index.wrapping_sub(1) as usize) {
        d.filter = patterns.to_ascii_lowercase();
    }
    let text = if d.file.is_empty() {
        d.filter.clone()
    } else {
        d.file.clone()
    };
    o.set_item_text(dlg, EDT1, &text);
    d.drives = drives(cpu);
    let cmb2 = item(cpu, dlg, CMB2);
    for &drive in &d.drives {
        o.send_text(cmb2, CB_ADDSTRING, 0, &drive_label(drive));
    }
    let mut dir = current_dir(cpu);
    if !d.initial_dir.is_empty() {
        let full = kernel32::full_path(cpu, &d.initial_dir);
        if is_dir(cpu, &full) {
            dir = full;
        }
    }
    scan(cpu, &mut d, &dir, &mut o);
    if d.flags & OFN_SHOWHELP == 0 {
        let help = item(cpu, dlg, PSH_HELP);
        o.user("ShowWindow", &[help, SW_HIDE]);
    }
    if d.flags & OFN_HIDEREADONLY != 0 {
        let check = item(cpu, dlg, CHX1);
        o.user("ShowWindow", &[check, SW_HIDE]);
    }
    if d.flags & OFN_READONLY != 0 {
        o.user("SendDlgItemMessageA", &[dlg, CHX1, BM_SETCHECK, 1, 0]);
    }
    let hook = d.hook;
    let ofn = d.ofn;
    super::state(cpu).dialogs.insert(id, d);
    if hook != 0 {
        let args = [dlg, WM_INITDIALOG, 0, ofn].map(Arg::V);
        o.call(hook, &args);
        o.0.push(Op::Return(Arg::Last));
    }
    Some((o, 1))
}

/// How the drive box names a drive.
fn drive_label(drive: u8) -> String {
    format!("{}:", (b'a' + drive) as char)
}

/// The folder list's entries for the folder `dir`, a full path, with the
/// folders `subfolders` in it, as Windows 3.1's and 98's dialog shows
/// them: open folders from the drive's root down to `dir`, each one
/// deeper, then `dir`'s folders, one deeper again. Each entry is its
/// name as the list shows it, the path it goes to and how deep it is.
fn folder_list(dir: &str, subfolders: &[String]) -> Vec<(String, String, u32)> {
    let root = &dir[..dir.len().min(3)];
    let mut list = vec![(root.to_ascii_lowercase(), folder(root), 0)];
    let mut path = folder(root);
    for name in dir[root.len()..].split('\\').filter(|n| !n.is_empty()) {
        path = join(&path, name);
        let depth = list.len() as u32;
        list.push((name.to_ascii_lowercase(), path.clone(), depth));
    }
    let depth = list.len() as u32;
    for name in subfolders {
        list.push((name.clone(), join(dir, name), depth));
    }
    list
}

/// How many of the folder list's entries for the folder `dir` are open
/// folders, `dir` and the folders it is in.
fn open_folders(dir: &str) -> usize {
    1 + dir.get(3..).unwrap_or("").split('\\').filter(|n| !n.is_empty()).count()
}

/// Show the folder `dir` (Wine's FD31_ScanDir): it becomes the current
/// directory; its files that match the filter fill the file list, and
/// the folder list shows the folders it is in and its own folders
/// (`folder_list`), in lowercase as Windows 3.1 shows them.
fn scan(cpu: &mut Cpu, d: &mut FileDlg, dir: &str, o: &mut Ops) -> bool {
    let full = folder(&kernel32::full_path(cpu, dir));
    if kernel32::change_directory(cpu, &full).is_err() {
        return false;
    }
    d.dir = full;
    let long = d.flags & OFN_LONGNAMES != 0;
    let entries = kernel32::find_entries(cpu, &d.dir, "*").unwrap_or_default();
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for e in entries {
        if e.name == "." || e.name == ".." || e.attributes & 0x0E != 0 && e.attributes & 0x10 == 0 {
            continue;
        }
        let shown = if long || e.short.is_empty() {
            e.name.clone()
        } else {
            e.short.clone()
        };
        let shown = shown.to_ascii_lowercase();
        if e.attributes & 0x10 != 0 {
            if e.attributes & 0x06 == 0 {
                dirs.push(shown);
            }
        } else if matches_filter(&e.name, &d.filter) || matches_filter(&shown, &d.filter) {
            files.push(shown);
        }
    }
    let dlg = d.hwnd;
    let lst1 = item(cpu, dlg, LST1);
    let lst2 = item(cpu, dlg, LST2);
    o.send(lst1, LB_RESETCONTENT, 0, 0);
    for f in &files {
        o.send_text(lst1, LB_ADDSTRING, 0, f);
    }
    // The folder's own folders in the order LBS_SORT would give them,
    // since they go in unsorted (below): "a~1" above "ab".
    dirs.sort_by(|a, b| {
        kernel32::compare_nocase(&crate::dosstr::to_bytes(a), &crate::dosstr::to_bytes(b))
    });
    let list = folder_list(&d.dir, &dirs);
    // Each at the end, in this order, as Windows does: the list has
    // LBS_SORT, and LB_ADDSTRING would put a folder "1997" above c:\
    // and sort the open folders in among the others.
    o.send(lst2, LB_RESETCONTENT, 0, 0);
    for (name, _, _) in &list {
        o.send_text(lst2, LB_INSERTSTRING, u32::MAX, name);
    }
    d.look_in = list.into_iter().map(|(_, path, depth)| (path, depth)).collect();
    o.set_item_text(dlg, STC1, &d.dir.to_ascii_lowercase());
    let drive = d.dir.as_bytes()[0].to_ascii_uppercase().wrapping_sub(b'A');
    if let Some(i) = d.drives.iter().position(|&x| x == drive) {
        let cmb2 = item(cpu, dlg, CMB2);
        o.send(cmb2, CB_SETCURSEL, i as u32, 0);
    }
    true
}

/// WM_COMMAND (Wine's FD31_WMCommand): what it returns.
fn command(cpu: &mut Cpu, dlg: u32, wp: u32, o: &mut Ops) -> u32 {
    let (id, code) = (wp & 0xFFFF, wp >> 16);
    let control = item(cpu, dlg, id);
    match id {
        LST1 if code == LBN_DBLCLK => {
            o.send(dlg, WM_COMMAND, IDOK, 0);
            1
        }
        LST1 if code == LBN_SELCHANGE => {
            o.send(control, LB_GETCURSEL, 0, 0);
            o.then(T_FILE_SELECTED, &[]);
            1
        }
        LST2 if code == LBN_DBLCLK => {
            o.send(control, LB_GETCURSEL, 0, 0);
            o.then(T_FOLDER, &[]);
            1
        }
        CMB1 if code == CBN_SELCHANGE => {
            o.send(control, CB_GETCURSEL, 0, 0);
            o.then(T_TYPE, &[]);
            1
        }
        CMB2 if code == CBN_SELCHANGE => {
            o.send(control, CB_GETCURSEL, 0, 0);
            o.then(T_DRIVE, &[]);
            1
        }
        IDOK => {
            o.get_item_text(dlg, EDT1);
            o.then(T_TEXT, &[]);
            o.user("SendDlgItemMessageA", &[dlg, CHX1, BM_GETCHECK, 0, 0]);
            o.then(T_OK, &[]);
            1
        }
        IDCANCEL => {
            o.user("EndDialog", &[dlg, 0]);
            1
        }
        IDABORT => {
            o.user("EndDialog", &[dlg, 1]);
            1
        }
        _ => 0,
    }
}

/// OK (Wine's FD31_Validate): a pattern becomes the filter, a folder is
/// shown, a file becomes the result if the flags let it.
fn ok(cpu: &mut Cpu, d: &mut FileDlg, read_only: bool, o: &mut Ops) {
    let dlg = d.hwnd;
    let text = d.text.trim().to_string();
    if text.is_empty() {
        return;
    }
    let path = kernel32::full_path(cpu, &join(&d.dir, &text).replace('/', "\\"));
    let path = if text.as_bytes().get(1) == Some(&b':') || text.starts_with('\\') {
        kernel32::full_path(cpu, &text)
    } else {
        path
    };
    let at = name_offset(&path);
    let base = path[at..].to_string();
    let caption = if d.title.is_empty() {
        if d.save { "Save As" } else { "Open" }.to_string()
    } else {
        d.title.clone()
    };
    if base.contains(['*', '?']) {
        d.filter = base.to_ascii_lowercase();
        o.set_item_text(dlg, EDT1, &d.filter);
        let dir = folder(&path[..at]);
        scan(cpu, d, &dir, o);
        return;
    }
    if is_dir(cpu, &path) {
        o.set_item_text(dlg, EDT1, &d.filter);
        scan(cpu, d, &path, o);
        return;
    }
    let mut path = path;
    if d.flags & OFN_NOVALIDATE == 0 {
        if !is_dir(cpu, &folder(&path[..at])) {
            if d.flags & OFN_PATHMUSTEXIST != 0 {
                let msg = format!(
                    "{}\nPath does not exist.\nCheck the path and try again.",
                    path
                );
                o.message_box(dlg, &msg, &caption, MB_OK | MB_ICONEXCLAMATION);
                return;
            }
        } else if extension(&base).is_empty() && !d.def_ext.is_empty() {
            path = format!("{}.{}", path, d.def_ext);
        }
        if d.flags & OFN_FILEMUSTEXIST != 0 && !d.save && !exists(cpu, &path) {
            let msg = format!(
                "{}\nFile not found.\nPlease verify the correct file name was given.",
                &path[at..]
            );
            o.message_box(dlg, &msg, &caption, MB_OK | MB_ICONEXCLAMATION);
            return;
        }
    }
    d.path = path.clone();
    let there = exists(cpu, &path);
    let ask = if d.save && d.flags & OFN_OVERWRITEPROMPT != 0 && there {
        Some(format!(
            "{} already exists.\nDo you want to replace it?",
            path
        ))
    } else if !d.save && d.flags & OFN_CREATEPROMPT != 0 && !there {
        Some(format!(
            "{}\nThis file does not exist.\nDo you want to create it?",
            path
        ))
    } else {
        None
    };
    if let Some(msg) = ask {
        o.message_box(dlg, &msg, &caption, MB_YESNO | MB_ICONEXCLAMATION);
        o.then(T_CONFIRM, &[read_only as u32]);
        return;
    }
    finish(cpu, d, read_only, o);
}

/// The file chosen becomes the result; the hook's FILEOKSTRING may keep
/// the dialog open.
fn finish(cpu: &mut Cpu, d: &mut FileDlg, read_only: bool, o: &mut Ops) {
    set_ofn_flags(cpu, d, OFN_READONLY, read_only);
    if !d.def_ext.is_empty() {
        let same = extension(&d.path).eq_ignore_ascii_case(&d.def_ext);
        set_ofn_flags(cpu, d, OFN_EXTENSIONDIFFERENT, !same);
    }
    let path = d.path.clone();
    if !write_result(cpu, d, &path) {
        set_extended_error(cpu, FNERR_BUFFERTOOSMALL);
        o.user("EndDialog", &[d.hwnd, 0]);
        return;
    }
    if d.hook != 0 {
        let args = [d.hwnd, d.fileok, 0, d.ofn].map(Arg::V);
        o.call(d.hook, &args);
        o.then(T_FILEOK, &[]);
    } else {
        o.user("EndDialog", &[d.hwnd, 1]);
    }
}

/// WM_DRAWITEM (Wine's FD31_WMDrawItem): a file, a folder or a drive.
fn draw(cpu: &mut Cpu, dlg: u32, lp: u32) -> Option<u32> {
    let mut w = [0u32; 12];
    for (i, v) in w.iter_mut().enumerate() {
        *v = cpu.bus.guest_read_32(lp + 4 * i as u32);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (ctl, item_id, item_state, hwnd_item, hdc) = (w[1], w[2], w[4], w[5], w[6]);
    let Some(d) = dialog(cpu, dlg).cloned() else {
        return Some(0);
    };
    let (text, kind, indent) = match ctl {
        LST1 | LST2 => {
            let text = user32::controls::listbox::item(cpu, hwnd_item, item_id as i32)
                .map(|(t, _)| crate::dosstr::from_bytes(&t));
            if ctl == LST1 {
                (text, 0, 0)
            } else {
                // An open folder for the folder shown and those it is in.
                let open = (item_id as usize) < open_folders(&d.dir);
                let indent = d.look_in.get(item_id as usize).map_or(0, |&(_, n)| n);
                (text, if open { 3 } else { 1 }, indent)
            }
        }
        CMB2 => {
            let text = d.drives.get(item_id as usize).map(|&x| drive_label(x));
            (text, 2, 0)
        }
        _ => return Some(0),
    };
    paint_item(cpu, hdc, lp, text.as_deref(), kind, indent, item_state);
    Some(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(dir: &str, subfolders: &[&str]) -> Vec<(String, String, u32)> {
        let subfolders: Vec<String> = subfolders.iter().map(|s| s.to_string()).collect();
        folder_list(dir, &subfolders)
    }

    fn entry(name: &str, path: &str, depth: u32) -> (String, String, u32) {
        (name.to_string(), path.to_string(), depth)
    }

    #[test]
    fn a_root_lists_itself_and_its_folders() {
        let list = entries("C:\\", &["saves", "windows"]);
        assert_eq!(
            list,
            [
                entry("c:\\", "C:\\", 0),
                entry("saves", "C:\\saves", 1),
                entry("windows", "C:\\windows", 1),
            ]
        );
        assert_eq!(open_folders("C:\\"), 1);
    }

    #[test]
    fn a_root_without_folders_still_lists_itself() {
        assert_eq!(entries("D:\\", &[]), [entry("d:\\", "D:\\", 0)]);
    }

    #[test]
    fn a_folder_lists_the_folders_it_is_in_then_its_own() {
        let list = entries("C:\\GAMES\\SHADOW", &["saves"]);
        assert_eq!(
            list,
            [
                entry("c:\\", "C:\\", 0),
                entry("games", "C:\\GAMES", 1),
                entry("shadow", "C:\\GAMES\\SHADOW", 2),
                entry("saves", "C:\\GAMES\\SHADOW\\saves", 3),
            ]
        );
        assert_eq!(open_folders("C:\\GAMES\\SHADOW"), 3);
    }
}
