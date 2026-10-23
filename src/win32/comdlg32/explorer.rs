//! The Explorer-style file dialog of Windows 98: the "Look in" box, a
//! toolbar, the view of the folder's entries, the file name, the file
//! type, and the program's template or hook as a child dialog, which is
//! sent the CDN_* notifications and may send the CDM_* messages.
//! Ported from Wine 1.2.3's comdlg32 (filedlg.c; see filedlg.rs for
//! the copyright notices).
//!
//! The view is a list box of COMDLG32's (Wine's is the shell's view, a
//! list view): the folders, then the files the filter chosen matches,
//! drawn with a small folder or page in front of their names.

use super::filedlg::*;
use super::{DLL, FNERR_BUFFERTOOSMALL, set_extended_error};
use crate::cpu::Cpu;
use crate::win32::comctl32::call_builtin;
use crate::win32::comctl32::script::{Arg, Ctx, Op};
use crate::win32::gdi32::Rect;
use crate::win32::kernel32;
use crate::win32::user32;
use crate::win32::user32::message::{
    WM_COMMAND, WM_DRAWITEM, WM_INITDIALOG, WM_MEASUREITEM, WM_NOTIFY, WM_USER,
};

/// The message a shell view sends for its browser.
const WM_GETISHELLBROWSER: u32 = WM_USER + 7;

// The custom dialog's messages (CDM_*) and notifications (CDN_*).
const CDM_FIRST: u32 = WM_USER + 100;
const CDM_LAST: u32 = WM_USER + 200;
const CDM_GETSPEC: u32 = CDM_FIRST;
const CDM_GETFILEPATH: u32 = CDM_FIRST + 1;
const CDM_GETFOLDERPATH: u32 = CDM_FIRST + 2;
const CDM_GETFOLDERIDLIST: u32 = CDM_FIRST + 3;
const CDM_SETCONTROLTEXT: u32 = CDM_FIRST + 4;
const CDM_HIDECONTROL: u32 = CDM_FIRST + 5;
const CDM_SETDEFEXT: u32 = CDM_FIRST + 6;
const CDN_INITDONE: u32 = -601i32 as u32;
const CDN_SELCHANGE: u32 = -602i32 as u32;
const CDN_FOLDERCHANGE: u32 = -603i32 as u32;
const CDN_FILEOK: u32 = -606i32 as u32;
const CDN_TYPECHANGE: u32 = -607i32 as u32;
const TTN_GETDISPINFOA: u32 = -520i32 as u32;

// The toolbar: its messages, buttons and commands.
const TB_ADDBITMAP: u32 = WM_USER + 19;
const TB_ADDBUTTONSA: u32 = WM_USER + 20;
const TB_BUTTONSTRUCTSIZE: u32 = WM_USER + 30;
const TB_AUTOSIZE: u32 = WM_USER + 33;
const TB_SETMAXTEXTROWS: u32 = WM_USER + 60;
const FCIDM_TB_UPFOLDER: u32 = 0xA001;
const FCIDM_TB_NEWFOLDER: u32 = 0xA002;
const FCIDM_TB_SMALLICON: u32 = 0xA003;
const FCIDM_TB_REPORTVIEW: u32 = 0xA004;
const TBSTATE_ENABLED: u32 = 0x04;
const BTNS_SEP: u32 = 0x01;
/// COMCTL32's standard view bitmaps (HINST_COMMCTRL, IDB_VIEW_SMALL_COLOR)
/// and the images in them.
const HINST_COMMCTRL: u32 = u32::MAX;
const IDB_VIEW_SMALL_COLOR: u32 = 4;
const VIEW_LIST: u32 = 2;
const VIEW_DETAILS: u32 = 3;
const VIEW_PARENTFOLDER: u32 = 8;
const VIEW_NEWFOLDER: u32 = 11;

// Notifications of the controls.
const CBN_SELENDOK: u32 = 9;
const LBN_SELCHANGE: u32 = 1;
const LBN_DBLCLK: u32 = 2;

// Styles.
const SS_NOTIFY: u32 = 0x0100;
const CBS_DROPDOWNLIST: u32 = 0x0003;
const CBS_OWNERDRAWFIXED: u32 = 0x0010;
const CBS_HASSTRINGS: u32 = 0x0200;
const LBS_NOTIFY: u32 = 0x0001;
const LBS_SORT: u32 = 0x0002;
const LBS_OWNERDRAWFIXED: u32 = 0x0010;
const LBS_HASSTRINGS: u32 = 0x0040;
const LBS_NOINTEGRALHEIGHT: u32 = 0x0100;
const LBS_MULTICOLUMN: u32 = 0x0200;
const ES_AUTOHSCROLL: u32 = 0x0080;
const BS_DEFPUSHBUTTON: u32 = 0x0001;
const BS_AUTOCHECKBOX: u32 = 0x0003;
const TBSTYLE_TOOLTIPS: u32 = 0x0100;
const CCS_NORESIZE: u32 = 0x0004;
const CCS_NODIVIDER: u32 = 0x0040;
const WS_EX_CLIENTEDGE: u32 = 0x0200;

/// The dialog's template: Windows 98's (NEWFILEOPENORD), its controls
/// laid out as Wine's English one lays them out.
pub fn template(save: bool) -> Vec<u32> {
    let style = DS_MODALFRAME | DS_3DLOOK | WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;
    let caption = if save { "Save As" } else { "Open" };
    let mut t = Template::new(style, (0, 0, 280, 164), caption);
    let v = WS_VISIBLE;
    let label = if save { "Save &in:" } else { "Look &in:" };
    t.item(
        v | WS_GROUP | SS_NOTIFY,
        (4, 6, 43, 8),
        STC4,
        ATOM_STATIC,
        label,
    );
    let look = CBS_DROPDOWNLIST | CBS_OWNERDRAWFIXED | CBS_HASSTRINGS | WS_VSCROLL | WS_TABSTOP;
    t.item(v | look, (49, 3, 132, 100), CMB2, ATOM_COMBOBOX, "");
    t.item(0, (181, 2, 102, 17), STC1, ATOM_STATIC, "");
    let shell = LBS_NOTIFY | LBS_SORT | LBS_NOINTEGRALHEIGHT | LBS_MULTICOLUMN | WS_HSCROLL;
    t.item(shell | WS_BORDER, (4, 20, 272, 85), LST1, ATOM_LISTBOX, "");
    let name = v | WS_GROUP | SS_NOTIFY;
    t.item(name, (5, 112, 46, 8), STC3, ATOM_STATIC, "File &name:");
    let edit = v | ES_AUTOHSCROLL | WS_BORDER | WS_TABSTOP;
    t.item(edit, (54, 110, 155, 12), EDT1, ATOM_EDIT, "");
    let types = if save {
        "Save as &type:"
    } else {
        "Files of &type:"
    };
    t.item(name, (5, 128, 42, 8), STC2, ATOM_STATIC, types);
    let combo = v | CBS_DROPDOWNLIST | WS_VSCROLL | WS_TABSTOP;
    t.item(combo, (54, 126, 155, 53), CMB1, ATOM_COMBOBOX, "");
    let check = v | BS_AUTOCHECKBOX | WS_TABSTOP;
    t.item(
        check,
        (54, 145, 100, 10),
        CHX1,
        ATOM_BUTTON,
        "Open as &read-only",
    );
    let ok = if save { "&Save" } else { "&Open" };
    let button = v | WS_TABSTOP;
    t.item(
        button | BS_DEFPUSHBUTTON,
        (222, 110, 50, 14),
        IDOK,
        ATOM_BUTTON,
        ok,
    );
    t.item(button, (222, 128, 50, 14), IDCANCEL, ATOM_BUTTON, "Cancel");
    t.item(button, (222, 145, 50, 14), PSH_HELP, ATOM_BUTTON, "&Help");
    t.words()
}

/// The template's procedure when the program gives a template but no
/// hook.
pub fn user_template_proc(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some((args[1] == WM_INITDIALOG) as u32)
}

/// The dialog procedure (Wine's FileOpenDlgProc95).
pub fn proc_(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hwnd, msg, wp, lp] = args[..] else {
        return Some(0);
    };
    match msg {
        WM_INITDIALOG => init(cpu, hwnd, lp),
        WM_MEASUREITEM => measure(cpu, lp),
        WM_DRAWITEM => draw(cpu, hwnd, lp),
        WM_COMMAND => command(cpu, hwnd, wp),
        WM_GETISHELLBROWSER => {
            let browser = dialog(cpu, hwnd).map_or(0, |d| d.browser);
            set_result(cpu, hwnd, browser);
            Some(1)
        }
        WM_NOTIFY => tooltip(cpu, lp),
        CDM_FIRST..=CDM_LAST => custom_message(cpu, hwnd, msg, wp, lp),
        _ => Some(0),
    }
}

/// SetWindowLongA(dlg, DWL_MSGRESULT, value).
fn set_result(cpu: &mut Cpu, dlg: u32, value: u32) {
    user32::window::set_window_long(cpu, &[dlg, 0, value]);
}

// The continuations.
const T_TOOLBAR: u16 = EXPLORER_FIRST;
const T_CUSTOM: u16 = EXPLORER_FIRST + 1;
const T_ARRANGED: u16 = EXPLORER_FIRST + 2;
const T_FILL: u16 = EXPLORER_FIRST + 3;
const T_VIEW: u16 = EXPLORER_FIRST + 4;
const T_VIEW_POS: u16 = EXPLORER_FIRST + 5;
const T_TEXT: u16 = EXPLORER_FIRST + 6;
const T_OPEN: u16 = EXPLORER_FIRST + 7;
const T_CONFIRM: u16 = EXPLORER_FIRST + 8;
const T_FILEOK: u16 = EXPLORER_FIRST + 9;
const T_FILEOK_STRING: u16 = EXPLORER_FIRST + 10;
const T_TYPE: u16 = EXPLORER_FIRST + 11;
const T_LOOK_IN: u16 = EXPLORER_FIRST + 12;
const T_SELECT: u16 = EXPLORER_FIRST + 13;
const T_DOUBLE_CLICK: u16 = EXPLORER_FIRST + 14;
const T_SPEC: u16 = EXPLORER_FIRST + 15;
const T_FILE_PATH: u16 = EXPLORER_FIRST + 16;
const T_RESULT: u16 = EXPLORER_FIRST + 17;
const T_NEW_FOLDER: u16 = EXPLORER_FIRST + 18;

pub fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    let dlg = c.hwnd;
    if id == T_RESULT {
        set_result(cpu, dlg, c.word(0));
        return vec![Op::Return(Arg::V(1))];
    }
    let Some(mut d) = dialog(cpu, dlg).cloned() else {
        return Vec::new();
    };
    let mut o = Ops::new();
    match id {
        T_TOOLBAR => {
            d.toolbar = c.last;
            toolbar(&mut o, c.last);
        }
        T_CUSTOM => {
            d.custom = if c.word(0) != 0 { c.last } else { 0 };
            if d.custom != 0 {
                arrange(cpu, &d, &mut o);
                o.then(T_ARRANGED, &[]);
            } else {
                shrink(cpu, &d, &mut o);
                o.then(T_FILL, &[]);
            }
        }
        T_ARRANGED => {
            let mut flags = SWP_NOACTIVATE;
            let mut rc = client_of(cpu, dlg);
            if d.flags & (OFN_ENABLETEMPLATE | OFN_ENABLETEMPLATEHANDLE) == 0 {
                // The hook's own template is empty: no size for it.
                flags |= SWP_NOSIZE;
                rc = Rect::default();
            }
            let args = [d.custom, HWND_BOTTOM, 0, 0];
            let args = [&args[..], &[rc.right as u32, rc.bottom as u32, flags]].concat();
            o.user("SetWindowPos", &args);
            o.then(T_FILL, &[]);
        }
        T_FILL => {
            let lst1 = item(cpu, dlg, LST1);
            let r = rect_in(cpu, lst1, dlg);
            o.put(B_CLASS, b"LISTBOX", 32);
            let style = WS_CHILD
                | WS_VISIBLE
                | WS_TABSTOP
                | WS_HSCROLL
                | LBS_NOTIFY
                | LBS_OWNERDRAWFIXED
                | LBS_HASSTRINGS
                | LBS_NOINTEGRALHEIGHT
                | LBS_MULTICOLUMN;
            create_window(&mut o, WS_EX_CLIENTEDGE, style, r, dlg, LST2);
            o.then(T_VIEW, &[]);
        }
        T_VIEW => {
            d.view = c.last;
            let send = user32::address_of("SendMessageA");
            o.send(dlg, WM_GETFONT, 0, 0);
            let font_args = [Arg::V(d.view), Arg::V(WM_SETFONT), Arg::Last, Arg::V(0)];
            o.call(send, &font_args);
            o.send(d.view, LB_SETCOLUMNWIDTH, 150, 0);
            let mut dir = d.dir.clone();
            if dir.is_empty() || !is_dir(cpu, &dir) {
                dir = current_dir(cpu);
            }
            browse(cpu, &mut d, &dir, &mut o);
            if d.custom != 0 {
                o.user("ShowWindow", &[d.custom, SW_SHOW]);
            }
            if d.flags & OFN_EXPLORER != 0 {
                notify(&d, &mut o, CDN_INITDONE);
                notify(&d, &mut o, CDN_FOLDERCHANGE);
            }
            o.then(T_VIEW_POS, &[]);
        }
        T_VIEW_POS => {
            // A hook that moved the hidden list box moves the view.
            let lst1 = item(cpu, dlg, LST1);
            let want = rect_in(cpu, lst1, dlg);
            let have = rect_in(cpu, d.view, dlg);
            if want != have {
                let (x, y) = (want.left as u32, want.top as u32);
                let (w, h) = (want.width() as u32, want.height() as u32);
                let flags = SWP_NOACTIVATE | SWP_NOZORDER;
                o.user("SetWindowPos", &[d.view, 0, x, y, w, h, flags]);
            }
            if d.flags & OFN_EXPLORER != 0 {
                notify(&d, &mut o, CDN_SELCHANGE);
            }
        }
        T_TEXT => {
            d.text = scratch_text(cpu, c, B_TEXT, TEXT_SIZE);
        }
        T_OPEN => open(cpu, &mut d, c.last & 3 == 1, &mut o),
        T_CONFIRM => {
            if c.last == IDYES {
                finish(cpu, &mut d, c.word(0) != 0, &mut o);
            }
        }
        T_FILEOK => {
            if c.last == 0 {
                let args = [d.custom, d.fileok, 0, d.ofn];
                o.user("SendMessageA", &args);
                o.then(T_FILEOK_STRING, &[]);
            }
        }
        T_FILEOK_STRING => {
            if c.last == 0 {
                o.user("EndDialog", &[dlg, 1]);
            }
        }
        T_TYPE => {
            if let Some((_, patterns)) = d.filters.get(c.last as usize).cloned() {
                d.index = c.last + (!d.custom_filter) as u32;
                cpu.bus.guest_write_32(d.ofn + 4 * O_INDEX as u32, d.index);
                d.filter = patterns.to_ascii_lowercase();
                let dir = d.dir.clone();
                browse(cpu, &mut d, &dir, &mut o);
                if d.flags & OFN_EXPLORER != 0 {
                    notify(&d, &mut o, CDN_TYPECHANGE);
                }
            }
        }
        T_LOOK_IN => {
            if let Some((path, _)) = d.look_in.get(c.last as usize).cloned() {
                change_folder(cpu, &mut d, &path, &mut o);
            }
        }
        T_SELECT => {
            d.selected = c.last as i32;
            if let Some((name, false)) = d.entries.get(c.last as usize).cloned() {
                o.set_item_text(dlg, EDT1, &name);
            }
            if d.flags & OFN_EXPLORER != 0 {
                notify(&d, &mut o, CDN_SELCHANGE);
            }
        }
        T_DOUBLE_CLICK => match d.entries.get(c.last as usize).cloned() {
            Some((name, true)) => {
                let path = join(&d.dir, &name);
                change_folder(cpu, &mut d, &path, &mut o);
            }
            Some((name, false)) => {
                o.set_item_text(dlg, EDT1, &name);
                o.send(dlg, WM_COMMAND, IDOK, 0);
            }
            None => {}
        },
        T_SPEC => {
            let (size, at) = (c.word(0), c.word(1));
            let edit = item(cpu, dlg, EDT1);
            if at != 0 {
                o.send(edit, WM_GETTEXT, size, at);
            }
            o.then(T_RESULT, &[c.last + 1]);
        }
        T_FILE_PATH => {
            let (size, at) = (c.word(0), c.word(1));
            let text = scratch_text(cpu, c, B_TEXT, TEXT_SIZE);
            let path = if text.is_empty() {
                d.dir.clone()
            } else {
                join(&d.dir, &text)
            };
            let bytes = crate::dosstr::to_bytes(&path);
            let total = bytes.len() as u32 + 1;
            if total <= size {
                write_string(cpu, at, &bytes, size);
            }
            o.then(T_RESULT, &[total]);
        }
        T_NEW_FOLDER => {
            new_folder(cpu, &mut d, c, &mut o);
        }
        _ => {}
    }
    if let Some(slot) = dialog(cpu, dlg) {
        *slot = d;
    }
    o.0
}

/// CreateWindowExA(ex, the class in B_CLASS, NULL, style, r, parent, id).
fn create_window(o: &mut Ops, ex: u32, style: u32, r: Rect, parent: u32, id: u32) {
    let target = user32::address_of("CreateWindowExA");
    let v = |x: i32| Arg::V(x as u32);
    o.call(
        target,
        &[
            Arg::V(ex),
            Arg::S(B_CLASS),
            Arg::V(0),
            Arg::V(style),
            v(r.left),
            v(r.top),
            v(r.width()),
            v(r.height()),
            Arg::V(parent),
            Arg::V(id),
            Arg::V(DLL.base),
            Arg::V(0),
        ],
    );
}

/// WM_NOTIFY with an OFNOTIFYA to the child dialog, if there is one.
fn notify(d: &FileDlg, o: &mut Ops, code: u32) {
    if d.custom == 0 {
        return;
    }
    o.write(B_NOTIFY, &[d.hwnd, 0, code, d.ofn, 0]);
    let target = user32::address_of("SendMessageA");
    o.call(
        target,
        &[
            Arg::V(d.custom),
            Arg::V(WM_NOTIFY),
            Arg::V(0),
            Arg::S(B_NOTIFY),
        ],
    );
}

/// WM_INITDIALOG: the controls set up as the program asks (Wine's
/// FILEDLG95_InitControls), then the child dialog made.
fn init(cpu: &mut Cpu, dlg: u32, id: u32) -> Option<u32> {
    if !state_has(cpu, id) {
        return Some(0);
    }
    let (browser, view) = super::browser::make(cpu, dlg);
    let mut d = super::state(cpu).dialogs.get(&id).cloned()?;
    d.hwnd = dlg;
    d.browser = browser;
    d.shell_view = view;
    let mut o = Ops::new();

    // The toolbar, right of the Look in box, as high as the hidden
    // static stc1 is.
    let (cmb2, stc1) = (item(cpu, dlg, CMB2), item(cpu, dlg, STC1));
    let look = rect_in(cpu, cmb2, dlg);
    let space = rect_in(cpu, stc1, dlg);
    let r = Rect::sized(look.right, look.top - 1, space.width(), space.height());
    o.put(B_CLASS, b"ToolbarWindow32", 32);
    let style = WS_CHILD
        | WS_GROUP
        | WS_VISIBLE
        | WS_CLIPSIBLINGS
        | TBSTYLE_TOOLTIPS
        | CCS_NODIVIDER
        | CCS_NORESIZE;
    create_window(&mut o, 0, style, r, dlg, STC1);
    o.then(T_TOOLBAR, &[]);

    if !d.title.is_empty() {
        o.set_text(dlg, &d.title);
    }
    let (dir, name) = initial(cpu, &d);
    d.dir = dir;
    let file = cpu.bus.guest_read_32(d.ofn + 4 * O_FILE as u32);
    if file != 0 {
        o.set_item_text(dlg, EDT1, &name);
    }
    let edit = item(cpu, dlg, EDT1);
    o.user("SetFocus", &[edit]);
    if d.flags & OFN_READONLY != 0 {
        o.user("SendDlgItemMessageA", &[dlg, CHX1, BM_SETCHECK, 1, 0]);
    }
    let mut hide = Vec::new();
    if d.flags & OFN_HIDEREADONLY != 0 {
        hide.push(CHX1);
    }
    if d.flags & OFN_SHOWHELP == 0 {
        hide.push(PSH_HELP);
    }
    for id in hide {
        let h = item(cpu, dlg, id);
        o.user("ShowWindow", &[h, SW_HIDE]);
        o.user("EnableWindow", &[h, 0]);
    }
    file_types(cpu, &mut d, &mut o);

    // The child dialog: the program's template, or an empty one for its
    // hook.
    let ofn = d.ofn;
    let proc_ = if d.hook != 0 {
        d.hook
    } else {
        super::address_of("FileOpenDlgProcUserTemplate")
    };
    let made = d.flags & (OFN_ENABLETEMPLATE | OFN_ENABLETEMPLATEHANDLE) != 0 || d.hook != 0;
    if d.flags & OFN_ENABLETEMPLATEHANDLE != 0 {
        let template = call_builtin(cpu, "KERNEL32.DLL", "LockResource", &[d.instance])?;
        let args = [DLL.base, template, dlg, proc_, ofn];
        o.user("CreateDialogIndirectParamA", &args);
    } else if d.flags & OFN_ENABLETEMPLATE != 0 {
        let args = [d.instance, d.template, dlg, proc_, ofn];
        o.user("CreateDialogParamA", &args);
    } else if d.hook != 0 {
        let style = WS_CHILD | WS_CLIPSIBLINGS | WS_VISIBLE | DS_CONTROL | DS_3DLOOK;
        // DLGTEMPLATE: no items, no size; no menu, class or title.
        o.write(B_TEMPLATE, &[style, 0, 0, 0, 0]);
        let target = user32::address_of("CreateDialogIndirectParamA");
        o.call(
            target,
            &[
                Arg::V(DLL.base),
                Arg::S(B_TEMPLATE),
                Arg::V(dlg),
                Arg::V(proc_),
                Arg::V(ofn),
            ],
        );
    }
    o.then(T_CUSTOM, &[made as u32]);
    super::state(cpu).dialogs.insert(id, d);
    o.run(cpu, dlg, 0)
}

fn state_has(cpu: &mut Cpu, id: u32) -> bool {
    super::state(cpu).dialogs.contains_key(&id)
}

/// The folder the dialog starts in, and the file name it shows, as
/// Windows 98 chooses them: the initial directory if it is there; else
/// the folder the file name names; else the current directory if files
/// of the filters are there; else My Documents, the desktop or the
/// current directory.
fn initial(cpu: &mut Cpu, d: &FileDlg) -> (String, String) {
    let name = d.file.clone();
    if !d.initial_dir.is_empty() {
        let full = kernel32::full_path(cpu, &d.initial_dir);
        if is_dir(cpu, &full) {
            return (folder(&full), name);
        }
        if exists(cpu, &full)
            && let Some(parent) = parent_of(&full)
        {
            return (parent, name);
        }
    }
    if name.contains(['\\', '/']) {
        let full = kernel32::full_path(cpu, &name);
        let at = name_offset(&full);
        let dir = folder(&full[..at]);
        if is_dir(cpu, &dir) {
            return (dir, full[at..].to_string());
        }
    }
    let cwd = current_dir(cpu);
    for (_, patterns) in &d.filters {
        if list(cpu, &cwd, patterns).iter().any(|(_, dir)| !dir) {
            return (folder(&cwd), name);
        }
    }
    for candidate in ["C:\\My Documents", "C:\\WINDOWS\\Desktop"] {
        if is_dir(cpu, candidate) {
            return (candidate.to_string(), name);
        }
    }
    (folder(&cwd), name)
}

/// The toolbar's buttons: up one level, a new folder, list and details.
fn toolbar(o: &mut Ops, tb: u32) {
    if tb == 0 {
        return;
    }
    o.send(tb, TB_BUTTONSTRUCTSIZE, 20, 0);
    o.send(tb, TB_SETMAXTEXTROWS, 0, 0);
    o.write(B_TBA, &[HINST_COMMCTRL, IDB_VIEW_SMALL_COLOR]);
    let send = user32::address_of("SendMessageA");
    o.call(
        send,
        &[Arg::V(tb), Arg::V(TB_ADDBITMAP), Arg::V(12), Arg::S(B_TBA)],
    );
    let sep = [0, 0, TBSTATE_ENABLED | BTNS_SEP << 8, 0, 0];
    let button = |image: u32, id: u32| [image, id, TBSTATE_ENABLED, 0, 0];
    let mut words = Vec::new();
    words.extend(sep);
    words.extend(button(VIEW_PARENTFOLDER, FCIDM_TB_UPFOLDER));
    words.extend(sep);
    words.extend(button(VIEW_NEWFOLDER, FCIDM_TB_NEWFOLDER));
    words.extend(sep);
    words.extend(button(VIEW_LIST, FCIDM_TB_SMALLICON));
    words.extend(button(VIEW_DETAILS, FCIDM_TB_REPORTVIEW));
    o.write(B_TBB, &words);
    o.call(
        send,
        &[Arg::V(tb), Arg::V(TB_ADDBUTTONSA), Arg::V(7), Arg::S(B_TBB)],
    );
    o.send(tb, TB_AUTOSIZE, 0, 0);
}

/// The file type box filled with the filters, the one nFilterIndex
/// names chosen (Wine's FILEDLG95_FILETYPE_Init).
fn file_types(cpu: &mut Cpu, d: &mut FileDlg, o: &mut Ops) {
    let cmb1 = item(cpu, d.hwnd, CMB1);
    for (title, _) in &d.filters {
        o.send_text(cmb1, CB_ADDSTRING, 0, title);
    }
    let custom = d.custom_filter as u32;
    let n = d.filters.len() as u32;
    if n != 0 {
        if d.index > n - custom {
            d.index = 1 - custom;
        }
        if d.index == 0 && custom == 0 {
            d.index = 1;
        }
        let cb = d.index - (1 - custom);
        o.send(cmb1, CB_SETCURSEL, cb, 0);
        if let Some((_, patterns)) = d.filters.get(cb as usize) {
            d.filter = patterns.to_ascii_lowercase();
        }
    } else {
        d.index = 0;
    }
    cpu.bus.guest_write_32(d.ofn + 4 * O_INDEX as u32, d.index);
}

/// Without a child dialog, a dialog without the read-only box and the
/// help button is shorter by the help button's place.
fn shrink(cpu: &mut Cpu, d: &FileDlg, o: &mut Ops) {
    let keep = OFN_SHOWHELP | OFN_ENABLETEMPLATE | OFN_ENABLETEMPLATEHANDLE;
    if d.flags & OFN_HIDEREADONLY == 0 || d.flags & keep != 0 {
        return;
    }
    let dlg = d.hwnd;
    let u = user32::user(cpu);
    let r = u.screen_rect(dlg);
    let help = item(cpu, dlg, PSH_HELP);
    let cancel = item(cpu, dlg, IDCANCEL);
    let u = user32::user(cpu);
    let less = u.screen_rect(help).bottom - u.screen_rect(cancel).bottom;
    let (w, h) = (r.width() as u32, (r.height() - less) as u32);
    let flags = SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER;
    o.user("SetWindowPos", &[dlg, 0, 0, 0, w, h, flags]);
}

/// The child dialog's controls and the dialog's laid out around each
/// other (Wine's ArrangeCtrlPositions): the child's controls below the
/// dialog's, or around them where its stc32 control says the dialog's
/// go; the dialog made big enough for both.
fn arrange(cpu: &mut Cpu, d: &FileDlg, o: &mut Ops) {
    let (dlg, child) = (d.hwnd, d.custom);
    let hide_help = d.flags & (OFN_HIDEREADONLY | OFN_SHOWHELP) == OFN_HIDEREADONLY;
    let screen = |cpu: &mut Cpu, h: u32| user32::user(cpu).screen_rect(h);
    let help_fixup = if hide_help {
        let help = item(cpu, dlg, PSH_HELP);
        let cancel = item(cpu, dlg, IDCANCEL);
        screen(cpu, help).bottom - screen(cpu, cancel).bottom
    } else {
        0
    };
    let mut parent = client_of(cpu, dlg);
    parent.bottom -= help_fixup;
    let stc32 = item(cpu, child, STC32);
    let r32 = if stc32 != 0 {
        rect_in(cpu, stc32, child)
    } else {
        Rect::default()
    };
    if stc32 != 0 {
        let size = [parent.right as u32, parent.bottom as u32];
        let args = [stc32, 0, 0, 0, size[0], size[1], SWP_NOMOVE | SWP_NOZORDER];
        o.user("SetWindowPos", &args);
    }
    let mv = |o: &mut Ops, h: u32, x: i32, y: i32| {
        let args = [h, 0, x as u32, y as u32, 0, 0, SWP_NOSIZE | SWP_NOZORDER];
        o.user("SetWindowPos", &args);
    };
    for h in children(cpu, child) {
        if h == stc32 {
            continue;
        }
        let mut r = rect_in(cpu, h, child);
        if stc32 != 0 && r.left > r32.right {
            r.left += parent.right - r32.right;
        }
        if r.top >= r32.bottom {
            r.top += parent.bottom - (r32.bottom - r32.top);
        }
        mv(o, h, r.left, r.top);
    }
    for h in children(cpu, dlg) {
        if h == child {
            continue;
        }
        let r = rect_in(cpu, h, dlg);
        mv(o, h, r.left + r32.left, r.top + r32.top);
    }
    let parent = client_of(cpu, dlg);
    let rc = client_of(cpu, child);
    let (chgx, chgy) = if stc32 != 0 {
        let x = if parent.right > r32.width() {
            rc.right - r32.width()
        } else {
            rc.right - parent.right
        };
        let y = if parent.bottom > r32.height() {
            rc.bottom - r32.height() - help_fixup
        } else {
            rc.bottom - parent.bottom
        };
        (x, y)
    } else {
        (0, rc.bottom - help_fixup)
    };
    let w = screen(cpu, dlg);
    let size = [(w.width() + chgx) as u32, (w.height() + chgy) as u32];
    let args = [dlg, 0, 0, 0, size[0], size[1], SWP_NOMOVE | SWP_NOZORDER];
    o.user("SetWindowPos", &args);
}

/// Show the folder `dir`: it becomes the current directory, its
/// entries fill the view, and the Look in box shows it.
fn browse(cpu: &mut Cpu, d: &mut FileDlg, dir: &str, o: &mut Ops) {
    d.dir = folder(dir);
    let _ = kernel32::change_directory(cpu, &d.dir);
    d.entries = list(cpu, &d.dir, &d.filter);
    d.selected = -1;
    if d.view != 0 {
        o.send(d.view, LB_RESETCONTENT, 0, 0);
        for (name, _) in &d.entries {
            o.send_text(d.view, LB_ADDSTRING, 0, name);
        }
    }
    // The drives, and the current one's folders down to the one shown.
    d.look_in.clear();
    let mut current = 0;
    let drive = d.dir.as_bytes()[0].to_ascii_uppercase();
    for letter in drives(cpu) {
        let root = format!("{}:\\", (b'A' + letter) as char);
        d.look_in.push((root.clone(), 0));
        if b'A' + letter != drive {
            continue;
        }
        current = d.look_in.len() - 1;
        let mut path = root;
        for (depth, part) in d.dir[3..].split('\\').filter(|p| !p.is_empty()).enumerate() {
            path = join(&path, part);
            d.look_in.push((path.clone(), depth as u32 + 1));
            current = d.look_in.len() - 1;
        }
    }
    let cmb2 = item(cpu, d.hwnd, CMB2);
    o.send(cmb2, CB_RESETCONTENT, 0, 0);
    for (path, depth) in &d.look_in {
        o.send_text(cmb2, CB_ADDSTRING, 0, &look_in_label(path, *depth));
    }
    o.send(cmb2, CB_SETCURSEL, current as u32, 0);
}

/// What the Look in box shows for a folder: a drive's root, or the
/// folder's name.
fn look_in_label(path: &str, depth: u32) -> String {
    if depth == 0 {
        format!("({})", &path[..2])
    } else {
        path.rsplit('\\').next().unwrap_or(path).to_string()
    }
}

/// Show another folder, and tell the hook (CDN_FOLDERCHANGE).
fn change_folder(cpu: &mut Cpu, d: &mut FileDlg, dir: &str, o: &mut Ops) {
    if !is_dir(cpu, dir) {
        return;
    }
    browse(cpu, d, dir, o);
    if d.flags & OFN_EXPLORER != 0 {
        notify(d, o, CDN_FOLDERCHANGE);
    }
}

/// WM_COMMAND (Wine's FILEDLG95_OnWMCommand).
fn command(cpu: &mut Cpu, dlg: u32, wp: u32) -> Option<u32> {
    let (id, code) = (wp & 0xFFFF, wp >> 16);
    let Some(d) = dialog(cpu, dlg).cloned() else {
        return Some(0);
    };
    let mut o = Ops::new();
    match id {
        IDOK => {
            o.get_item_text(dlg, EDT1);
            o.then(T_TEXT, &[]);
            o.user("SendDlgItemMessageA", &[dlg, CHX1, BM_GETCHECK, 0, 0]);
            o.then(T_OPEN, &[]);
        }
        IDCANCEL => {
            o.user("EndDialog", &[dlg, 0]);
        }
        CMB1 if code == CBN_SELENDOK => {
            let cmb1 = item(cpu, dlg, CMB1);
            o.send(cmb1, CB_GETCURSEL, 0, 0);
            o.then(T_TYPE, &[]);
        }
        CMB2 if code == CBN_SELENDOK => {
            let cmb2 = item(cpu, dlg, CMB2);
            o.send(cmb2, CB_GETCURSEL, 0, 0);
            o.then(T_LOOK_IN, &[]);
        }
        LST2 if code == LBN_SELCHANGE => {
            o.send(d.view, LB_GETCURSEL, 0, 0);
            o.then(T_SELECT, &[]);
        }
        LST2 if code == LBN_DBLCLK => {
            o.send(d.view, LB_GETCURSEL, 0, 0);
            o.then(T_DOUBLE_CLICK, &[]);
        }
        FCIDM_TB_UPFOLDER => {
            if let Some(up) = parent_of(&d.dir) {
                let mut d = d;
                change_folder(cpu, &mut d, &up, &mut o);
                if let Some(slot) = dialog(cpu, dlg) {
                    *slot = d;
                }
            }
        }
        FCIDM_TB_NEWFOLDER => {
            o.then(T_NEW_FOLDER, &[]);
        }
        _ => {}
    }
    o.run(cpu, dlg, 0)
}

/// Make a folder "New Folder" (or "New Folder (2)" and on) in the folder
/// shown, and show it.
fn new_folder(cpu: &mut Cpu, d: &mut FileDlg, c: &Ctx, o: &mut Ops) {
    for n in 1..100 {
        let name = if n == 1 {
            "New Folder".to_string()
        } else {
            format!("New Folder ({})", n)
        };
        let path = join(&d.dir, &name);
        if exists(cpu, &path) {
            continue;
        }
        write_string(cpu, c.scratch + B_TEXT, path.as_bytes(), TEXT_SIZE);
        let at = c.scratch + B_TEXT;
        call_builtin(cpu, "KERNEL32.DLL", "CreateDirectoryA", &[at, 0]);
        break;
    }
    let dir = d.dir.clone();
    browse(cpu, d, &dir, o);
}

/// The names in `text` between double quotes.
fn quoted(text: &str) -> Vec<String> {
    text.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .collect()
}

/// The full path `text` names, from the folder `dir`.
fn combine(cpu: &mut Cpu, dir: &str, text: &str) -> String {
    let t = text.replace('/', "\\");
    let b = t.as_bytes();
    if t.starts_with("\\\\") {
        return t;
    }
    if b.len() >= 2 && b[1] == b':' {
        return kernel32::full_path(cpu, &t);
    }
    if t.starts_with('\\') {
        return kernel32::full_path(cpu, &format!("{}{}", &dir[..2], t));
    }
    kernel32::full_path(cpu, &join(dir, &t))
}

/// OK (Wine's FILEDLG95_OnOpen): a folder typed or chosen is shown, a
/// pattern becomes the filter, a file is checked as the flags ask and
/// becomes the result.
fn open(cpu: &mut Cpu, d: &mut FileDlg, read_only: bool, o: &mut Ops) {
    let dlg = d.hwnd;
    let mut text = d.text.trim_end().to_string();
    if text.is_empty() {
        let chosen = usize::try_from(d.selected).ok();
        if let Some((name, true)) = chosen.and_then(|i| d.entries.get(i)).cloned() {
            let path = join(&d.dir, &name);
            change_folder(cpu, d, &path, o);
        }
        return;
    }
    if d.flags & OFN_ALLOWMULTISELECT != 0 && text.contains('"') {
        let names = quoted(&text);
        if names.len() > 1 {
            open_several(cpu, d, &names, read_only, o);
            return;
        }
        if let Some(name) = names.into_iter().next() {
            text = name;
        }
    }
    let caption = if d.title.is_empty() {
        if d.save { "Save As" } else { "Open" }.to_string()
    } else {
        d.title.clone()
    };
    let mut path = combine(cpu, &d.dir, &text);
    let validate = d.flags & OFN_NOVALIDATE == 0;
    if validate
        && path
            .get(3..)
            .is_some_and(|p| p.contains(['/', ':', '<', '>', '|']))
    {
        let msg = "A filename cannot contain any of the following characters:\n/ : < > |";
        o.message_box(dlg, msg, &caption, MB_OK | MB_ICONEXCLAMATION);
        return;
    }
    let at = name_offset(&path);
    let base = path[at..].to_string();
    let edit = item(cpu, dlg, EDT1);
    if validate && base.contains(['*', '?']) {
        // A pattern: the filter, in the folder it names.
        let dir = folder(&path[..at]);
        if !is_dir(cpu, &dir) {
            return;
        }
        d.filter = base.to_ascii_lowercase();
        let found = d
            .filters
            .iter()
            .position(|(_, p)| p.eq_ignore_ascii_case(&base));
        if let Some(i) = found {
            let cmb1 = item(cpu, dlg, CMB1);
            o.send(cmb1, CB_SETCURSEL, i as u32, 0);
        }
        if dir.eq_ignore_ascii_case(&d.dir) {
            browse(cpu, d, &dir, o);
        } else {
            change_folder(cpu, d, &dir, o);
        }
        o.send(edit, EM_SETSEL, 0, u32::MAX);
        return;
    }
    if !base.is_empty() && is_dir(cpu, &path) || path.len() <= 3 {
        change_folder(cpu, d, &path, o);
        o.send(edit, EM_SETSEL, 0, u32::MAX);
        return;
    }
    if validate {
        let folder_there = is_dir(cpu, &folder(&path[..at]));
        if !folder_there {
            if d.flags & OFN_PATHMUSTEXIST != 0 {
                let msg = format!(
                    "{}\nPath does not exist.\nCheck the path and try again.",
                    path
                );
                o.message_box(dlg, &msg, &caption, MB_OK | MB_ICONEXCLAMATION);
                return;
            }
        } else if d.flags & OFN_FILEMUSTEXIST != 0 && !d.save && !exists(cpu, &path) {
            let msg = format!(
                "{}\nFile not found.\nPlease verify the correct file name was given.",
                base
            );
            o.message_box(dlg, &msg, &caption, MB_OK | MB_ICONEXCLAMATION);
            return;
        }
    }
    // A name without an extension takes the filter's, or the default.
    if extension(&base).is_empty() {
        let first = d.filter.split(';').next().unwrap_or("").trim().to_string();
        let mut ext = extension(&first).to_string();
        if ext.is_empty() {
            ext = d.def_ext.clone();
        }
        if !ext.is_empty() && !ext.contains(['*', '?']) {
            let with = format!("{}.{}", path, ext);
            if d.save || exists(cpu, &with) {
                path = with;
            }
        }
    }
    if !d.def_ext.is_empty() {
        let same = extension(&path).eq_ignore_ascii_case(&d.def_ext);
        set_ofn_flags(cpu, d, OFN_EXTENSIONDIFFERENT, !same);
    }
    d.path = path.clone();
    let there = exists(cpu, &path);
    if d.save && d.flags & OFN_OVERWRITEPROMPT != 0 && there {
        let msg = format!("{} already exists.\nDo you want to replace it?", path);
        o.message_box(dlg, &msg, &caption, MB_YESNO | MB_ICONEXCLAMATION);
        o.then(T_CONFIRM, &[read_only as u32]);
        return;
    }
    if !d.save && d.flags & OFN_CREATEPROMPT != 0 && !there {
        let msg = format!(
            "{}\nThis file does not exist.\nDo you want to create it?",
            path
        );
        o.message_box(dlg, &msg, &caption, MB_YESNO | MB_ICONEXCLAMATION);
        o.then(T_CONFIRM, &[read_only as u32]);
        return;
    }
    finish(cpu, d, read_only, o);
}

/// The file chosen becomes the result, and the dialog ends when the
/// hook lets it (CDN_FILEOK, then FILEOKSTRING).
fn finish(cpu: &mut Cpu, d: &mut FileDlg, read_only: bool, o: &mut Ops) {
    set_ofn_flags(cpu, d, OFN_READONLY, read_only);
    let path = d.path.clone();
    if !write_result(cpu, d, &path) {
        set_extended_error(cpu, FNERR_BUFFERTOOSMALL);
        o.user("EndDialog", &[d.hwnd, 0]);
        return;
    }
    file_ok(d, o);
}

/// Ask the hook whether the dialog may end, and end it if so.
fn file_ok(d: &FileDlg, o: &mut Ops) {
    if d.hook == 0 || d.custom == 0 {
        o.user("EndDialog", &[d.hwnd, 1]);
        return;
    }
    if d.flags & OFN_EXPLORER != 0 {
        notify(d, o, CDN_FILEOK);
        o.then(T_FILEOK, &[]);
    } else {
        let args = [d.custom, d.fileok, 0, d.ofn];
        o.user("SendMessageA", &args);
        o.then(T_FILEOK_STRING, &[]);
    }
}

/// Several files chosen: lpstrFile gets the folder, then each name,
/// each after a 0, and a 0 after the last.
fn open_several(cpu: &mut Cpu, d: &mut FileDlg, names: &[String], read_only: bool, o: &mut Ops) {
    if d.flags & OFN_FILEMUSTEXIST != 0 && !d.save {
        for name in names {
            let path = combine(cpu, &d.dir, name);
            if !exists(cpu, &path) {
                let msg = format!(
                    "{}\nFile not found.\nPlease verify the correct file name was given.",
                    name
                );
                let caption = if d.save { "Save As" } else { "Open" };
                o.message_box(d.hwnd, &msg, caption, MB_OK | MB_ICONEXCLAMATION);
                return;
            }
        }
    }
    set_ofn_flags(cpu, d, OFN_READONLY, read_only);
    let dir = folder(&d.dir);
    let mut out = crate::dosstr::to_bytes(&dir);
    out.push(0);
    let offset = out.len();
    for name in names {
        out.extend(crate::dosstr::to_bytes(name));
        out.push(0);
    }
    out.push(0);
    let file = cpu.bus.guest_read_32(d.ofn + 4 * O_FILE as u32);
    let max = cpu.bus.guest_read_32(d.ofn + 4 * O_MAX_FILE as u32);
    if out.len() > max as usize {
        if file != 0 && cpu.bus.guest_probe(file, 2, true) {
            cpu.bus.guest_write_16(file, out.len() as u16);
        }
        set_extended_error(cpu, FNERR_BUFFERTOOSMALL);
        o.user("EndDialog", &[d.hwnd, 0]);
        return;
    }
    if file != 0 && cpu.bus.guest_probe(file, out.len(), true) {
        cpu.bus.guest_write_bytes(file, &out);
    }
    cpu.bus.guest_write_16(d.ofn + 56, offset as u16);
    cpu.bus.guest_write_16(d.ofn + 58, 0);
    file_ok(d, o);
}

/// The CDM_* messages a hook sends the dialog: what they return goes in
/// DWL_MSGRESULT, and the procedure returns TRUE.
fn custom_message(cpu: &mut Cpu, dlg: u32, msg: u32, wp: u32, lp: u32) -> Option<u32> {
    let Some(mut d) = dialog(cpu, dlg).cloned() else {
        return Some(0);
    };
    let mut o = Ops::new();
    let explorer = d.flags & OFN_EXPLORER != 0;
    let edit = item(cpu, dlg, EDT1);
    match msg {
        CDM_GETSPEC => {
            o.send(edit, WM_GETTEXTLENGTH, 0, 0);
            o.then(T_SPEC, &[wp, lp]);
        }
        CDM_GETFILEPATH => {
            if !explorer {
                set_result(cpu, dlg, u32::MAX);
                return Some(1);
            }
            o.get_item_text(dlg, EDT1);
            o.then(T_FILE_PATH, &[wp, lp]);
        }
        CDM_GETFOLDERPATH => {
            let bytes = crate::dosstr::to_bytes(&d.dir);
            if lp != 0 {
                write_string(cpu, lp, &bytes, wp);
            }
            set_result(cpu, dlg, bytes.len() as u32 + 1);
            return Some(1);
        }
        CDM_GETFOLDERIDLIST => {
            let list = id_list(&d.dir);
            if list.len() as u32 <= wp && lp != 0 && cpu.bus.guest_probe(lp, list.len(), true) {
                cpu.bus.guest_write_bytes(lp, &list);
            }
            set_result(cpu, dlg, list.len() as u32);
            return Some(1);
        }
        CDM_SETCONTROLTEXT => {
            if lp != 0 {
                o.user("SetDlgItemTextA", &[dlg, wp, lp]);
            }
            o.then(T_RESULT, &[1]);
        }
        CDM_HIDECONTROL => {
            if !explorer {
                set_result(cpu, dlg, 0);
                return Some(1);
            }
            let control = item(cpu, dlg, wp);
            if control != 0 {
                o.user("ShowWindow", &[control, SW_HIDE]);
            }
            o.then(T_RESULT, &[1]);
        }
        CDM_SETDEFEXT => {
            d.def_ext = if lp == 0 {
                String::new()
            } else {
                user32::string_at(cpu, lp)?
            };
            if let Some(slot) = dialog(cpu, dlg) {
                *slot = d;
            }
            set_result(cpu, dlg, 1);
            return Some(1);
        }
        _ => return Some(0),
    }
    o.run(cpu, dlg, 1)
}

/// An item ID list for the folder `dir`, for CDM_GETFOLDERIDLIST: an
/// item for the drive and one for each folder, each its size, a kind
/// (2Fh a drive, 31h a folder) and its name; then a size of 0.
fn id_list(dir: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut add = |kind: u8, name: &[u8]| {
        let size = 2 + 1 + name.len() + 1;
        out.extend_from_slice(&(size as u16).to_le_bytes());
        out.push(kind);
        out.extend_from_slice(name);
        out.push(0);
    };
    let bytes = crate::dosstr::to_bytes(dir);
    if bytes.len() >= 3 {
        add(0x2F, &bytes[..3]);
        for part in bytes[3..].split(|&b| b == b'\\').filter(|p| !p.is_empty()) {
            add(0x31, part);
        }
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// WM_MEASUREITEM: the view's and the Look in box's items are as high
/// as their small icons.
fn measure(cpu: &mut Cpu, lp: u32) -> Option<u32> {
    let id = cpu.bus.guest_read_32(lp + 4);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if id != LST2 && id != CMB2 {
        return Some(0);
    }
    cpu.bus.guest_write_32(lp + 16, 16);
    Some(1)
}

/// WM_DRAWITEM: an entry of the view or of the Look in box, with a
/// folder, a drive or a page in front of its name.
fn draw(cpu: &mut Cpu, dlg: u32, lp: u32) -> Option<u32> {
    let mut w = [0u32; 12];
    for (i, v) in w.iter_mut().enumerate() {
        *v = cpu.bus.guest_read_32(lp + 4 * i as u32);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (ctl, item_id, item_state, hdc) = (w[1], w[2], w[4], w[6]);
    let Some(d) = dialog(cpu, dlg).cloned() else {
        return Some(0);
    };
    let entry = match ctl {
        LST2 => d
            .entries
            .get(item_id as usize)
            .map(|(name, dir)| (name.clone(), if *dir { 1 } else { 0 }, 0)),
        CMB2 => d.look_in.get(item_id as usize).map(|(path, depth)| {
            let indent = if item_state & ODS_COMBOBOXEDIT != 0 {
                0
            } else {
                *depth
            };
            let kind = if *depth == 0 { 2 } else { 1 };
            (look_in_label(path, *depth), kind, indent)
        }),
        _ => return Some(0),
    };
    let (text, kind, indent) = match &entry {
        Some((text, kind, indent)) => (Some(text.as_str()), *kind, *indent),
        None => (None, 0, 0),
    };
    paint_item(cpu, hdc, lp, text, kind, indent, item_state);
    Some(1)
}

/// WM_NOTIFY: the toolbar's tooltips ask for their text.
fn tooltip(cpu: &mut Cpu, lp: u32) -> Option<u32> {
    let code = cpu.bus.guest_read_32(lp + 8);
    let id = cpu.bus.guest_read_32(lp + 4);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if code != TTN_GETDISPINFOA {
        return Some(0);
    }
    let text: &[u8] = match id {
        FCIDM_TB_UPFOLDER => b"Up One Level",
        FCIDM_TB_NEWFOLDER => b"Create New Folder",
        FCIDM_TB_SMALLICON => b"List",
        FCIDM_TB_REPORTVIEW => b"Details",
        _ => b"",
    };
    write_string(cpu, lp + 16, text, 80);
    cpu.bus.guest_write_32(lp + 12, lp + 16);
    Some(0)
}
