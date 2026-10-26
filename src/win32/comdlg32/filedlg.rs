//! The file dialogs, GetOpenFileNameA and GetSaveFileNameA: what the
//! Explorer-style dialog (explorer.rs) and Windows 3.1's (old.rs) share.
//! Ported from Wine 1.2.3's comdlg32 (filedlg.c, filedlg31.c), keeping
//! what Windows 98 does:
//!
//! Copyright 1998, 1999, 2000 Bertho A. Stultiens, Eric Kohl, Klaas van
//! Gend, Marcus Meissner and others; Copyright 2007 Google (Lei Zhang).
//! Wine is distributed under the GNU Lesser General Public License,
//! version 2.1 or later.
//!
//! The dialog is a modal dialog of a template in memory, with a dialog
//! procedure of COMDLG32's. Its procedure moves and fills its controls
//! and notifies the program's hook through SendMessageA, so it works
//! through scripts (comctl32's script.rs), whose continuations are
//! `then` here. What a dialog keeps between messages is a `FileDlg` in
//! COMDLG32's state, by an id its WM_INITDIALOG is given.
//!
//! The folders and files it lists are the emulated drives': a folder's
//! entries come from KERNEL32's directory listing, and the dialog makes
//! the folder it shows the current directory, as Windows does.

use super::{
    CDERR_FINDRESFAILURE, CDERR_INITIALIZATION, CDERR_LOADRESFAILURE, CDERR_STRUCTSIZE, DLL,
    set_extended_error, state,
};
use crate::cpu::Cpu;
use crate::win32::comctl32::script::{self, Arg, Ctx, Op, Script, words_of};
use crate::win32::comctl32::{self, call_builtin};
use crate::win32::gdi32::Rect;
use crate::win32::kernel32;
use crate::win32::user32;

/// The continuations' module in comctl32's `dispatch_then`.
pub const MODULE: comctl32::Module = comctl32::FILEDLG;

// OPENFILENAMEA's words.
pub const O_SIZE: usize = 0;
pub const O_OWNER: usize = 1;
pub const O_INSTANCE: usize = 2;
pub const O_FILTER: usize = 3;
pub const O_CUSTOM_FILTER: usize = 4;
pub const O_INDEX: usize = 6;
pub const O_FILE: usize = 7;
pub const O_MAX_FILE: usize = 8;
pub const O_FILE_TITLE: usize = 9;
pub const O_MAX_FILE_TITLE: usize = 10;
pub const O_INITIAL_DIR: usize = 11;
pub const O_TITLE: usize = 12;
pub const O_FLAGS: usize = 13;
pub const O_DEF_EXT: usize = 15;
pub const O_HOOK: usize = 17;
pub const O_TEMPLATE: usize = 18;
const OFN_WORDS: usize = 19;

// The flags (OFN_*).
pub const OFN_READONLY: u32 = 0x0000_0001;
pub const OFN_OVERWRITEPROMPT: u32 = 0x0000_0002;
pub const OFN_HIDEREADONLY: u32 = 0x0000_0004;
pub const OFN_NOCHANGEDIR: u32 = 0x0000_0008;
pub const OFN_SHOWHELP: u32 = 0x0000_0010;
pub const OFN_ENABLEHOOK: u32 = 0x0000_0020;
pub const OFN_ENABLETEMPLATE: u32 = 0x0000_0040;
pub const OFN_ENABLETEMPLATEHANDLE: u32 = 0x0000_0080;
pub const OFN_NOVALIDATE: u32 = 0x0000_0100;
pub const OFN_ALLOWMULTISELECT: u32 = 0x0000_0200;
pub const OFN_EXTENSIONDIFFERENT: u32 = 0x0000_0400;
pub const OFN_PATHMUSTEXIST: u32 = 0x0000_0800;
pub const OFN_FILEMUSTEXIST: u32 = 0x0000_1000;
pub const OFN_CREATEPROMPT: u32 = 0x0000_2000;
pub const OFN_EXPLORER: u32 = 0x0008_0000;

// The controls of the dialogs.
pub const IDOK: u32 = 1;
pub const IDCANCEL: u32 = 2;
pub const IDABORT: u32 = 3;
pub const PSH_HELP: u32 = 0x040E;
pub const CHX1: u32 = 0x0410;
pub const STC1: u32 = 0x0440;
pub const STC2: u32 = 0x0441;
pub const STC3: u32 = 0x0442;
pub const STC4: u32 = 0x0443;
pub const STC32: u32 = 0x045F;
pub const LST1: u32 = 0x0460;
pub const LST2: u32 = 0x0461;
pub const CMB1: u32 = 0x0470;
pub const CMB2: u32 = 0x0471;
pub const EDT1: u32 = 0x0480;

// Messages.
pub const WM_GETTEXT: u32 = 0x000D;
pub const WM_GETTEXTLENGTH: u32 = 0x000E;
pub const WM_SETFONT: u32 = 0x0030;
pub const WM_GETFONT: u32 = 0x0031;
pub const EM_SETSEL: u32 = 0x00B1;
pub const BM_GETCHECK: u32 = 0x00F0;
pub const BM_SETCHECK: u32 = 0x00F1;
pub const CB_ADDSTRING: u32 = 0x0143;
pub const CB_GETCURSEL: u32 = 0x0147;
pub const CB_RESETCONTENT: u32 = 0x014B;
pub const CB_SETCURSEL: u32 = 0x014E;
pub const LB_ADDSTRING: u32 = 0x0180;
pub const LB_RESETCONTENT: u32 = 0x0184;
pub const LB_GETCURSEL: u32 = 0x0188;
pub const LB_SETCOLUMNWIDTH: u32 = 0x0195;

// ShowWindow's and SetWindowPos's arguments.
pub const SW_HIDE: u32 = 0;
pub const SW_SHOW: u32 = 5;
pub const SWP_NOSIZE: u32 = 0x0001;
pub const SWP_NOMOVE: u32 = 0x0002;
pub const SWP_NOZORDER: u32 = 0x0004;
pub const SWP_NOACTIVATE: u32 = 0x0010;
pub const HWND_BOTTOM: u32 = 1;

// MessageBoxA's styles and answers.
pub const MB_OK: u32 = 0;
pub const MB_YESNO: u32 = 4;
pub const MB_ICONEXCLAMATION: u32 = 0x30;
pub const IDYES: u32 = 6;

// The dialogs' scripts' scratch space: a buffer for strings and the text
// read from controls, one for a second string (a message box's
// caption), an OFNOTIFY, the toolbar's structures, a class name, and a
// dialog template.
pub const B_TEXT: u32 = 0;
pub const TEXT_SIZE: u32 = 0x400;
pub const B_TEXT2: u32 = 0x400;
pub const TEXT2_SIZE: u32 = 0x100;
pub const B_NOTIFY: u32 = 0x500;
pub const B_TBA: u32 = 0x520;
pub const B_TBB: u32 = 0x530;
pub const B_CLASS: u32 = 0x5E0;
pub const B_TEMPLATE: u32 = 0x600;
pub const SCRATCH: u32 = 0x1400;

/// What a file dialog keeps from its start to its end.
#[derive(Clone, Debug, Default)]
pub struct FileDlg {
    /// The dialog, once its WM_INITDIALOG came.
    pub hwnd: u32,
    /// The program's OPENFILENAMEA.
    pub ofn: u32,
    pub save: bool,
    /// Windows 3.1's dialog.
    pub old: bool,
    pub flags: u32,
    /// The hook, if OFN_ENABLEHOOK gives one.
    pub hook: u32,
    pub instance: u32,
    pub template: u32,
    /// The child dialog of the program's template or hook, the view
    /// (a list box) and the toolbar of the Explorer-style dialog.
    pub custom: u32,
    pub view: u32,
    pub toolbar: u32,
    /// The IShellBrowser and IShellView WM_GETISHELLBROWSER gives.
    pub browser: u32,
    pub shell_view: u32,
    /// The folder shown, a full path, and the current directory when
    /// the dialog started.
    pub dir: String,
    pub saved_dir: String,
    /// The filters: the custom one first, if there is one, then the
    /// program's, each a title and its patterns.
    pub filters: Vec<(String, String)>,
    pub custom_filter: bool,
    /// The patterns of the filter chosen, and its index as
    /// OPENFILENAMEA's nFilterIndex counts it.
    pub filter: String,
    pub index: u32,
    pub file: String,
    pub initial_dir: String,
    pub title: String,
    pub def_ext: String,
    /// The folder's entries shown: a name, and whether it is a folder.
    pub entries: Vec<(String, bool)>,
    /// The view's selected entry, or -1.
    pub selected: i32,
    /// The Look in box's folders: a path and how deep it is.
    pub look_in: Vec<(String, u32)>,
    /// The drives of the old dialog's drive box.
    pub drives: Vec<u8>,
    /// The file name typed, and the path being opened, between the
    /// steps of OK.
    pub text: String,
    pub path: String,
    /// The messages the hook is sent (RegisterWindowMessageA's).
    pub fileok: u32,
    pub lbselch: u32,
}

crate::state_fields!(FileDlg {
    hwnd,
    ofn,
    save,
    old,
    flags,
    hook,
    instance,
    template,
    custom,
    view,
    toolbar,
    browser,
    shell_view,
    dir,
    saved_dir,
    filters,
    custom_filter,
    filter,
    index,
    file,
    initial_dir,
    title,
    def_ext,
    entries,
    selected,
    look_in,
    drives,
    text,
    path,
    fileok,
    lbselch,
});

/// The id of the dialog `hwnd` in COMDLG32's state.
pub fn id_of(cpu: &mut Cpu, hwnd: u32) -> Option<u32> {
    state(cpu)
        .dialogs
        .iter()
        .find(|(_, d)| d.hwnd == hwnd && hwnd != 0)
        .map(|(&id, _)| id)
}

/// The dialog `hwnd`'s state.
pub fn dialog(cpu: &mut Cpu, hwnd: u32) -> Option<&mut FileDlg> {
    let id = id_of(cpu, hwnd)?;
    state(cpu).dialogs.get_mut(&id)
}

/// GetOpenFileNameA(ofn).
pub fn get_open_file_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_file_name(cpu, args[0], false)
}

/// GetSaveFileNameA(ofn).
pub fn get_save_file_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_file_name(cpu, args[0], true)
}

/// COMMDLG's GetOpenFileName(ofn), with the 16-bit structure made a
/// 32-bit one: Windows 95's 16-bit COMMDLG has no Explorer style, so a
/// Windows 3.1 program always gets Windows 3.1's dialog.
pub fn get_open_file_name16(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_file_name_as(cpu, args[0], false, true)
}

/// COMMDLG's GetSaveFileName(ofn), as `get_open_file_name16`.
pub fn get_save_file_name16(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_file_name_as(cpu, args[0], true, true)
}

/// The string at `at`, or an empty one for NULL.
fn string(cpu: &mut Cpu, at: u32) -> Option<String> {
    if at == 0 {
        return Some(String::new());
    }
    user32::string_at(cpu, at)
}

/// The pairs of strings at `at` ("title\0patterns\0...\0\0"), at most
/// `max` of them.
fn pairs(cpu: &mut Cpu, mut at: u32, max: usize) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    if at == 0 {
        return Some(out);
    }
    while out.len() < max {
        let title = user32::bytes_at(cpu, at)?;
        if title.is_empty() {
            break;
        }
        at += title.len() as u32 + 1;
        let patterns = user32::bytes_at(cpu, at)?;
        at += patterns.len() as u32 + 1;
        let empty = patterns.is_empty();
        out.push((
            crate::dosstr::from_bytes(&title),
            crate::dosstr::from_bytes(&patterns),
        ));
        // A filter without patterns ends the list, as Wine reads it.
        if empty {
            break;
        }
    }
    Some(out)
}

/// The current directory, a full path.
pub fn current_dir(cpu: &mut Cpu) -> String {
    let drive = cpu.bus.disk.get_current_drive();
    kernel32::current_directory(cpu, drive)
}

/// Whether `full` (a full path) is a folder.
pub fn is_dir(cpu: &mut Cpu, full: &str) -> bool {
    let full = kernel32::full_path(cpu, &kernel32::dos_path(full));
    let b = full.as_bytes();
    if b.len() <= 3 {
        let drive = b[0].to_ascii_uppercase().wrapping_sub(b'A');
        return cpu.bus.disk.is_mounted(drive);
    }
    cpu.bus.disk.is_directory(full.trim_end_matches('\\'))
}

/// Whether `full` (a full path) is a file or a folder.
pub fn exists(cpu: &mut Cpu, full: &str) -> bool {
    if is_dir(cpu, full) {
        return true;
    }
    let full = kernel32::full_path(cpu, &kernel32::dos_path(full));
    cpu.bus.disk.get_file_attribute(&full).is_ok()
}

/// `name` in the folder `dir`.
pub fn join(dir: &str, name: &str) -> String {
    format!("{}\\{}", dir.trim_end_matches('\\'), name)
}

/// The folder `dir` is in, or None for a root.
pub fn parent_of(dir: &str) -> Option<String> {
    let trimmed = dir.trim_end_matches('\\');
    if trimmed.len() <= 2 {
        return None;
    }
    let cut = trimmed.rfind('\\')?;
    Some(if cut <= 2 {
        trimmed[..3].to_string()
    } else {
        trimmed[..cut].to_string()
    })
}

/// A full path as the dialog shows a folder: a root with its backslash
/// ("C:\"), others without one.
pub fn folder(full: &str) -> String {
    let t = full.trim_end_matches('\\');
    if t.len() <= 2 {
        format!("{}\\", t)
    } else {
        t.to_string()
    }
}

/// Whether `name` matches one of `patterns` ("*.txt;*.doc"); no patterns
/// match every name.
pub fn matches_filter(name: &str, patterns: &str) -> bool {
    let mut any = false;
    for p in patterns.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        any = true;
        if kernel32::matches(name, p) {
            return true;
        }
    }
    !any
}

/// The entries of the folder `dir` the dialog lists: its folders, then
/// its files that match `patterns`, each in name order; hidden and
/// system ones left out, as Explorer leaves them.
pub fn list(cpu: &mut Cpu, dir: &str, patterns: &str) -> Vec<(String, bool)> {
    let entries = kernel32::find_entries(cpu, &folder(dir), "*").unwrap_or_default();
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for e in entries {
        if e.name == "." || e.name == ".." || e.attributes & 0x06 != 0 {
            continue;
        }
        if e.attributes & 0x10 != 0 {
            dirs.push(e.name);
        } else if e.attributes & 0x08 == 0 && matches_filter(&e.name, patterns) {
            files.push(e.name);
        }
    }
    let key = |s: &String| s.to_ascii_lowercase();
    dirs.sort_by_key(key);
    files.sort_by_key(key);
    let mut out: Vec<(String, bool)> = dirs.into_iter().map(|d| (d, true)).collect();
    out.extend(files.into_iter().map(|f| (f, false)));
    out
}

/// The drives there are, 0 for A:.
pub fn drives(cpu: &mut Cpu) -> Vec<u8> {
    (0..26u8).filter(|&d| cpu.bus.disk.is_mounted(d)).collect()
}

/// A file name's extension, after its dot, or "".
pub fn extension(name: &str) -> &str {
    let base = name.rsplit(['\\', '/', ':']).next().unwrap_or(name);
    match base.rfind('.') {
        Some(dot) => &base[dot + 1..],
        None => "",
    }
}

/// Where the name of the file starts in `path`, after its folder.
pub fn name_offset(path: &str) -> usize {
    path.rfind(['\\', ':']).map_or(0, |i| i + 1)
}

/// The ops of a script made by a dialog: calls, continuations and
/// strings in the scratch space at fixed places.
#[derive(Default)]
pub struct Ops(pub Vec<Op>);

impl Ops {
    pub fn new() -> Ops {
        Ops(Vec::new())
    }

    pub fn call(&mut self, target: u32, args: &[Arg]) -> &mut Self {
        self.0.push(Op::Call {
            target,
            args: args.to_vec(),
        });
        self
    }

    /// USER32's function `name` with plain values.
    pub fn user(&mut self, name: &str, args: &[u32]) -> &mut Self {
        let args: Vec<Arg> = args.iter().map(|&a| Arg::V(a)).collect();
        self.call(user32::address_of(name), &args)
    }

    /// SendMessageA with plain values.
    pub fn send(&mut self, hwnd: u32, msg: u32, wp: u32, lp: u32) -> &mut Self {
        self.user("SendMessageA", &[hwnd, msg, wp, lp])
    }

    /// Write `bytes` and a 0 at the offset `at` of the scratch space, in
    /// at most `size` bytes.
    pub fn put(&mut self, at: u32, bytes: &[u8], size: u32) -> &mut Self {
        let mut b = bytes.to_vec();
        b.truncate(size as usize - 1);
        b.push(0);
        self.write(at, &words_of(&b))
    }

    pub fn write(&mut self, at: u32, words: &[u32]) -> &mut Self {
        self.0.push(Op::Write {
            at,
            words: words.to_vec(),
        });
        self
    }

    /// SendMessageA(hwnd, msg, wp, the string `text`).
    pub fn send_text(&mut self, hwnd: u32, msg: u32, wp: u32, text: &str) -> &mut Self {
        self.put(B_TEXT, &crate::dosstr::to_bytes(text), TEXT_SIZE);
        let target = user32::address_of("SendMessageA");
        self.call(
            target,
            &[Arg::V(hwnd), Arg::V(msg), Arg::V(wp), Arg::S(B_TEXT)],
        )
    }

    /// SetWindowTextA(hwnd, text).
    pub fn set_text(&mut self, hwnd: u32, text: &str) -> &mut Self {
        self.put(B_TEXT, &crate::dosstr::to_bytes(text), TEXT_SIZE);
        let target = user32::address_of("SetWindowTextA");
        self.call(target, &[Arg::V(hwnd), Arg::S(B_TEXT)])
    }

    /// SetDlgItemTextA(dlg, id, text).
    pub fn set_item_text(&mut self, dlg: u32, id: u32, text: &str) -> &mut Self {
        self.put(B_TEXT, &crate::dosstr::to_bytes(text), TEXT_SIZE);
        let target = user32::address_of("SetDlgItemTextA");
        self.call(target, &[Arg::V(dlg), Arg::V(id), Arg::S(B_TEXT)])
    }

    /// GetDlgItemTextA(dlg, id) into the text buffer.
    pub fn get_item_text(&mut self, dlg: u32, id: u32) -> &mut Self {
        let target = user32::address_of("GetDlgItemTextA");
        self.call(
            target,
            &[Arg::V(dlg), Arg::V(id), Arg::S(B_TEXT), Arg::V(TEXT_SIZE)],
        )
    }

    /// MessageBoxA(owner, text, caption, style).
    pub fn message_box(&mut self, owner: u32, text: &str, caption: &str, style: u32) -> &mut Self {
        self.put(B_TEXT, &crate::dosstr::to_bytes(text), TEXT_SIZE);
        self.put(B_TEXT2, &crate::dosstr::to_bytes(caption), TEXT2_SIZE);
        let target = user32::address_of("MessageBoxA");
        self.call(
            target,
            &[
                Arg::V(owner),
                Arg::S(B_TEXT),
                Arg::S(B_TEXT2),
                Arg::V(style),
            ],
        )
    }

    pub fn then(&mut self, id: u16, data: &[u32]) -> &mut Self {
        self.0.push(Op::Then {
            id,
            data: data.to_vec(),
        });
        self
    }

    pub fn ret(&mut self, value: u32) -> &mut Self {
        self.0.push(Op::Return(Arg::V(value)));
        self
    }

    /// Run the ops as the script of the window `hwnd`: what the dialog
    /// procedure returns, `result` if no op returns.
    pub fn run(self, cpu: &mut Cpu, hwnd: u32, result: u32) -> Option<u32> {
        let s = Script {
            hwnd,
            ops: self.0,
            scratch: SCRATCH,
            result,
        };
        script::run(cpu, MODULE, s)
    }
}

/// A dialog template being made (DLGTEMPLATE and its items), in
/// dialog units, with a font.
pub struct Template(pub Vec<u8>);

impl Template {
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn wide(&mut self, s: &str) {
        for c in s.encode_utf16() {
            self.u16(c);
        }
        self.u16(0);
    }

    fn align(&mut self) {
        while !self.0.len().is_multiple_of(4) {
            self.0.push(0);
        }
    }

    pub fn new(style: u32, rect: (i16, i16, i16, i16), caption: &str) -> Template {
        let mut t = Template(Vec::new());
        t.u32(style | DS_SETFONT);
        t.u32(0);
        t.u16(0);
        for v in [rect.0, rect.1, rect.2, rect.3] {
            t.u16(v as u16);
        }
        t.u16(0);
        t.u16(0);
        t.wide(caption);
        t.u16(8);
        t.wide("MS Sans Serif");
        t
    }

    /// An item of the class `class` (an atom of a system class).
    pub fn item(
        &mut self,
        style: u32,
        rect: (i16, i16, i16, i16),
        id: u32,
        class: u16,
        title: &str,
    ) {
        self.align();
        self.u32(style | WS_CHILD);
        self.u32(0);
        for v in [rect.0, rect.1, rect.2, rect.3] {
            self.u16(v as u16);
        }
        self.u16(id as u16);
        self.u16(0xFFFF);
        self.u16(class);
        self.wide(title);
        self.u16(0);
        let n = u16::from_le_bytes([self.0[8], self.0[9]]) + 1;
        self.0[8..10].copy_from_slice(&n.to_le_bytes());
    }

    pub fn words(mut self) -> Vec<u32> {
        self.align();
        words_of(&self.0)
    }
}

pub const DS_SETFONT: u32 = 0x0040;
pub const DS_MODALFRAME: u32 = 0x0080;
pub const DS_3DLOOK: u32 = 0x0004;
pub const DS_CONTROL: u32 = 0x0400;
pub const WS_POPUP: u32 = 0x8000_0000;
pub const WS_CHILD: u32 = 0x4000_0000;
pub const WS_VISIBLE: u32 = 0x1000_0000;
pub const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
pub const WS_CLIPCHILDREN: u32 = 0x0200_0000;
pub const WS_CAPTION: u32 = 0x00C0_0000;
pub const WS_BORDER: u32 = 0x0080_0000;
pub const WS_VSCROLL: u32 = 0x0020_0000;
pub const WS_HSCROLL: u32 = 0x0010_0000;
pub const WS_SYSMENU: u32 = 0x0008_0000;
pub const WS_GROUP: u32 = 0x0002_0000;
pub const WS_TABSTOP: u32 = 0x0001_0000;
pub const ATOM_BUTTON: u16 = 0x80;
pub const ATOM_EDIT: u16 = 0x81;
pub const ATOM_STATIC: u16 = 0x82;
pub const ATOM_LISTBOX: u16 = 0x83;
pub const ATOM_COMBOBOX: u16 = 0x85;

/// Start a file dialog: the dialog runs as a modal dialog, and its
/// result is the function's.
fn get_file_name(cpu: &mut Cpu, ofn: u32, save: bool) -> Option<u32> {
    get_file_name_as(cpu, ofn, save, false)
}

/// `get_file_name`, with Windows 3.1's dialog whatever the flags say
/// when `win16`.
fn get_file_name_as(cpu: &mut Cpu, ofn: u32, save: bool, win16: bool) -> Option<u32> {
    if ofn == 0 {
        set_extended_error(cpu, CDERR_INITIALIZATION);
        return Some(0);
    }
    let mut w = [0u32; OFN_WORDS];
    for (i, v) in w.iter_mut().enumerate() {
        *v = cpu.bus.guest_read_32(ofn + 4 * i as u32);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    // COMDLG32 imports COMCTL32, whose toolbar the dialog makes: map it
    // in a program that doesn't import it.
    crate::win32::loader::map_builtin_named(cpu, "COMCTL32.DLL")?;
    // The size of Windows 98's structure, or of Windows 2000's.
    if w[O_SIZE] != 76 && w[O_SIZE] != 88 {
        set_extended_error(cpu, CDERR_STRUCTSIZE);
        return Some(0);
    }
    let mut flags = w[O_FLAGS];
    if !save && flags & OFN_FILEMUSTEXIST != 0 {
        flags |= OFN_PATHMUSTEXIST;
    }
    let filters = pairs(cpu, w[O_FILTER], 256)?;
    let custom = pairs(cpu, w[O_CUSTOM_FILTER], 1)?;
    let file = if w[O_FILE] != 0 && w[O_MAX_FILE] != 0 {
        let bytes = user32::bytes_at(cpu, w[O_FILE])?;
        let max = w[O_MAX_FILE] as usize;
        crate::dosstr::from_bytes(&bytes[..bytes.len().min(max)])
    } else {
        String::new()
    };
    let initial_dir = string(cpu, w[O_INITIAL_DIR])?;
    let title = string(cpu, w[O_TITLE])?;
    let def_ext = string(cpu, w[O_DEF_EXT])?;
    let hook = if flags & OFN_ENABLEHOOK != 0 {
        w[O_HOOK]
    } else {
        0
    };
    let customized = OFN_ALLOWMULTISELECT | OFN_ENABLEHOOK | OFN_ENABLETEMPLATE;
    let old = win16 || flags & customized != 0 && flags & OFN_EXPLORER == 0;

    // A template that isn't there is an error before any dialog.
    let mut template = 0;
    if flags & OFN_ENABLETEMPLATEHANDLE != 0 {
        template = call_builtin(cpu, "KERNEL32.DLL", "LockResource", &[w[O_INSTANCE]])?;
        if template == 0 {
            set_extended_error(cpu, CDERR_LOADRESFAILURE);
            return Some(0);
        }
    } else if flags & OFN_ENABLETEMPLATE != 0 {
        let args = [w[O_INSTANCE], w[O_TEMPLATE], 5];
        let found = call_builtin(cpu, "KERNEL32.DLL", "FindResourceA", &args)?;
        if found == 0 {
            set_extended_error(cpu, CDERR_FINDRESFAILURE);
            return Some(0);
        }
    }
    cpu.bus.guest_write_32(ofn + 4 * O_FLAGS as u32, flags);
    set_extended_error(cpu, 0);

    let atom = |cpu: &mut Cpu, name: &str| user32::class::atom(user32::user(cpu), name) as u32;
    let fileok = atom(cpu, "commdlg_FileNameOK");
    let lbselch = atom(cpu, "commdlg_LBSelChangedNotify");
    let saved_dir = current_dir(cpu);
    let mut all = custom.clone();
    all.extend(filters);
    let d = FileDlg {
        ofn,
        save,
        old,
        flags,
        hook,
        instance: w[O_INSTANCE],
        template: w[O_TEMPLATE],
        saved_dir,
        filters: all,
        custom_filter: !custom.is_empty(),
        index: w[O_INDEX],
        file,
        initial_dir,
        title,
        def_ext,
        selected: -1,
        fileok,
        lbselch,
        ..FileDlg::default()
    };
    let s = state(cpu);
    s.next = s.next.wrapping_add(1).max(1);
    let id = s.next;
    s.dialogs.insert(id, d);

    let owner = w[O_OWNER];
    let mut o = Ops::new();
    if old && flags & OFN_ENABLETEMPLATEHANDLE != 0 {
        let proc_ = super::address_of("FileOpenDlgProc31");
        let args = [DLL.base, template, owner, proc_, id];
        o.user("DialogBoxIndirectParamA", &args);
    } else if old && flags & OFN_ENABLETEMPLATE != 0 {
        let proc_ = super::address_of("FileOpenDlgProc31");
        let args = [w[O_INSTANCE], w[O_TEMPLATE], owner, proc_, id];
        o.user("DialogBoxParamA", &args);
    } else {
        let (words, proc_) = if old {
            (super::old::template(save), "FileOpenDlgProc31")
        } else {
            (super::explorer::template(save), "FileOpenDlgProc")
        };
        o.write(B_TEMPLATE, &words);
        let target = user32::address_of("DialogBoxIndirectParamA");
        let proc_ = super::address_of(proc_);
        o.call(
            target,
            &[
                Arg::V(DLL.base),
                Arg::S(B_TEMPLATE),
                Arg::V(owner),
                Arg::V(proc_),
                Arg::V(id),
            ],
        );
    }
    o.then(T_END, &[id]);
    o.run(cpu, 0, 0)
}

// The continuations: this file's, the Explorer-style dialog's and the
// old dialog's.
const T_END: u16 = 1;
pub const EXPLORER_FIRST: u16 = 100;
pub const OLD_FIRST: u16 = 200;

/// The continuations of the file dialogs' scripts.
pub fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    if id >= OLD_FIRST {
        return super::old::then(cpu, id, c);
    }
    if id >= EXPLORER_FIRST {
        return super::explorer::then(cpu, id, c);
    }
    match id {
        T_END => end(cpu, c),
        _ => Vec::new(),
    }
}

/// The dialog ended with `c.last`: forget it and return.
fn end(cpu: &mut Cpu, c: &mut Ctx) -> Vec<Op> {
    let Some(d) = state(cpu).dialogs.remove(&c.word(0)) else {
        return vec![Op::Return(Arg::V(0))];
    };
    super::browser::free(cpu, &d);
    if d.flags & OFN_NOCHANGEDIR != 0 {
        let _ = kernel32::change_directory(cpu, &d.saved_dir);
    }
    if c.last == u32::MAX {
        set_extended_error(cpu, CDERR_LOADRESFAILURE);
    }
    vec![Op::Return(Arg::V((c.last == 1) as u32))]
}

/// Write `s` and its 0 at `at`, in at most `size` bytes.
pub fn write_string(cpu: &mut Cpu, at: u32, s: &[u8], size: u32) {
    if at == 0 || size == 0 {
        return;
    }
    let mut b = s.to_vec();
    b.truncate(size as usize - 1);
    b.push(0);
    if cpu.bus.guest_probe(at, b.len(), true) {
        cpu.bus.guest_write_bytes(at, &b);
    }
}

/// Fill the program's OPENFILENAMEA with the file `path` chosen: its
/// lpstrFile, nFileOffset, nFileExtension and lpstrFileTitle. False if
/// lpstrFile is too small, whose first word then says how big it must
/// be.
pub fn write_result(cpu: &mut Cpu, d: &FileDlg, path: &str) -> bool {
    let ofn = d.ofn;
    let file = cpu.bus.guest_read_32(ofn + 4 * O_FILE as u32);
    let max = cpu.bus.guest_read_32(ofn + 4 * O_MAX_FILE as u32);
    let multi = (d.flags & OFN_ALLOWMULTISELECT != 0) as usize;
    let bytes = crate::dosstr::to_bytes(path);
    if bytes.len() + multi >= max as usize {
        if file != 0 && cpu.bus.guest_probe(file, 2, true) {
            let need = bytes.len() + 1 + multi;
            cpu.bus.guest_write_16(file, need as u16);
        }
        return false;
    }
    let mut out = bytes.clone();
    out.push(0);
    if multi != 0 {
        out.push(0);
    }
    if file != 0 && cpu.bus.guest_probe(file, out.len(), true) {
        cpu.bus.guest_write_bytes(file, &out);
    }
    let offset = name_offset(path);
    let ext = extension(path);
    let ext_offset = if ext.is_empty() {
        0
    } else {
        path.len() - ext.len()
    };
    cpu.bus.guest_write_16(ofn + 56, offset as u16);
    cpu.bus.guest_write_16(ofn + 58, ext_offset as u16);
    let title = cpu.bus.guest_read_32(ofn + 4 * O_FILE_TITLE as u32);
    let max_title = cpu.bus.guest_read_32(ofn + 4 * O_MAX_FILE_TITLE as u32);
    write_string(cpu, title, &bytes[offset..], max_title);
    true
}

/// Set or clear the flags `bits` of the program's OPENFILENAMEA.
pub fn set_ofn_flags(cpu: &mut Cpu, d: &mut FileDlg, bits: u32, on: bool) {
    if on {
        d.flags |= bits;
    } else {
        d.flags &= !bits;
    }
    cpu.bus.guest_write_32(d.ofn + 4 * O_FLAGS as u32, d.flags);
}

/// GetFileTitleA(file, title, size): the name of the file `file` names,
/// 0; or the size the buffer must have; or -1 for no file name.
pub fn get_file_title(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [file, title, size] = args[..] else {
        return Some(u32::MAX);
    };
    if file == 0 || title == 0 {
        return Some(0xFFFF_FFFF);
    }
    let name = user32::bytes_at(cpu, file)?;
    let size = size & 0xFFFF;
    if name.is_empty() || name.iter().any(|&b| matches!(b, b'*' | b'[' | b']')) {
        return Some(u32::MAX);
    }
    if matches!(name.last(), Some(b'/' | b'\\' | b':')) {
        return Some(u32::MAX);
    }
    let start = name
        .iter()
        .rposition(|&b| matches!(b, b'/' | b'\\' | b':'))
        .map_or(0, |i| i + 1);
    let base = &name[start..];
    if (size as usize) < base.len() + 1 {
        return Some(base.len() as u32 + 1);
    }
    let mut out = base.to_vec();
    out.push(0);
    if !cpu.bus.guest_probe(title, out.len(), true) {
        return None;
    }
    cpu.bus.guest_write_bytes(title, &out);
    Some(0)
}

/// The rectangle of the window `hwnd` in the client coordinates of
/// `to` (GetWindowRect and MapWindowPoints).
pub fn rect_in(cpu: &mut Cpu, hwnd: u32, to: u32) -> Rect {
    let u = user32::user(cpu);
    let r = u.screen_rect(hwnd);
    let (x, y) = u.client_origin(to);
    r.offset(-x, -y)
}

/// The client area of `hwnd`, from (0, 0).
pub fn client_of(cpu: &mut Cpu, hwnd: u32) -> Rect {
    comctl32::client(cpu, hwnd)
}

/// The child of the dialog `dlg` with the id `id`, or 0.
pub fn item(cpu: &mut Cpu, dlg: u32, id: u32) -> u32 {
    if dlg == 0 {
        return 0;
    }
    call_builtin(cpu, "USER32.DLL", "GetDlgItem", &[dlg, id]).unwrap_or(0)
}

/// The children of `hwnd`, first to last (GetWindow's GW_CHILD and
/// GW_HWNDNEXT).
pub fn children(cpu: &mut Cpu, hwnd: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut c = user32::window::get_window(cpu, &[hwnd, 5]).unwrap_or(0);
    while c != 0 && out.len() < 0x1000 {
        out.push(c);
        c = user32::window::get_window(cpu, &[c, 2]).unwrap_or(0);
    }
    out
}

/// The text of the scratch space's buffer `at`, as a script's call left
/// it there.
pub fn scratch_text(cpu: &mut Cpu, c: &Ctx, at: u32, size: u32) -> String {
    let bytes = cpu.bus.guest_read_asciiz(c.scratch + at, size as usize);
    crate::dosstr::from_bytes(&bytes)
}

/// Paint an owner-drawn entry of a list or a combo box, as the
/// DRAWITEMSTRUCT at `lp` asks: its background, a small picture of its
/// kind (0 a file, 1 a folder, 2 a drive, 3 an open folder) `indent`
/// steps in, its text, and the focus.
pub fn paint_item(
    cpu: &mut Cpu,
    hdc: u32,
    lp: u32,
    text: Option<&str>,
    kind: u32,
    indent: u32,
    item_state: u32,
) {
    use crate::win32::comctl32::gfx;
    let rc = Rect::read(cpu, lp + 28);
    let (bg, fg) = if item_state & ODS_SELECTED != 0 {
        (COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT)
    } else {
        (COLOR_WINDOW, COLOR_WINDOWTEXT)
    };
    gfx::fill_sys(cpu, hdc, rc, bg);
    let Some(text) = text else {
        return;
    };
    let x = rc.left + 2 + 10 * indent as i32;
    let y = rc.top + (rc.height() - 12) / 2;
    let black = gfx::stock(cpu, 4);
    match kind {
        // A folder: a yellow box with a tab.
        1 => {
            gfx::fill_rgb(cpu, hdc, Rect::sized(x, y + 2, 14, 10), 0x0000_FFFF);
            gfx::frame(cpu, hdc, Rect::sized(x, y + 2, 14, 10), black);
            gfx::frame(cpu, hdc, Rect::sized(x, y, 6, 3), black);
        }
        // An open folder: the same box with its front lowered.
        3 => {
            gfx::fill_rgb(cpu, hdc, Rect::sized(x, y + 2, 14, 10), 0x0000_FFFF);
            gfx::frame(cpu, hdc, Rect::sized(x, y + 2, 14, 10), black);
            gfx::frame(cpu, hdc, Rect::sized(x, y, 6, 3), black);
            gfx::fill_rgb(cpu, hdc, Rect::sized(x + 2, y + 6, 14, 6), 0x0080_FFFF);
            gfx::frame(cpu, hdc, Rect::sized(x + 2, y + 6, 14, 6), black);
        }
        // A drive: a gray box with a green light.
        2 => {
            gfx::fill_rgb(cpu, hdc, Rect::sized(x, y + 3, 14, 7), 0x00C0_C0C0);
            gfx::frame(cpu, hdc, Rect::sized(x, y + 3, 14, 7), black);
            gfx::fill_rgb(cpu, hdc, Rect::sized(x + 10, y + 6, 2, 2), 0x0000_FF00);
        }
        // A file: a white page.
        _ => {
            gfx::fill_rgb(cpu, hdc, Rect::sized(x + 2, y, 10, 12), 0x00FF_FFFF);
            gfx::frame(cpu, hdc, Rect::sized(x + 2, y, 10, 12), black);
        }
    }
    gfx::set_bk_mode(cpu, hdc, 1);
    let color = gfx::sys_color(cpu, fg);
    gfx::set_text_color(cpu, hdc, color);
    let mut r = Rect::new(x + 18, rc.top, rc.right, rc.bottom);
    // DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX
    gfx::draw_text(cpu, hdc, &crate::dosstr::to_bytes(text), &mut r, 0x0824);
    if item_state & ODS_FOCUS != 0 {
        call_builtin(cpu, "USER32.DLL", "DrawFocusRect", &[hdc, lp + 28]);
    }
}

// DRAWITEMSTRUCT's states, and the colors.
pub const ODS_SELECTED: u32 = 0x0001;
pub const ODS_FOCUS: u32 = 0x0010;
pub const ODS_COMBOBOXEDIT: u32 = 0x1000;
const COLOR_WINDOW: u32 = 5;
const COLOR_WINDOWTEXT: u32 = 8;
const COLOR_HIGHLIGHT: u32 = 13;
const COLOR_HIGHLIGHTTEXT: u32 = 14;
