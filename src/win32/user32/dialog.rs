//! Dialogs and message boxes: the windows dialog templates describe, the
//! dialog manager's window procedure (DefDlgProcA), the keyboard interface
//! IsDialogMessageA gives a dialog, the functions on a dialog's controls,
//! and MessageBoxA's boxes. Ported from Wine 1.2.3 (dialog.c, defdlg.c,
//! msgbox.c), keeping what Windows 95 does.
//!
//! A dialog keeps its state in its extra bytes, after DWL_MSGRESULT,
//! DWL_DLGPROC and DWL_USER: the control that had the focus, its font,
//! the value EndDialog gave, its base units and its flags. The state is
//! made the first time DefDlgProcA sees the window, as Wine's is, so a
//! window of a class made from #32770's is a dialog too.
//!
//! Whatever calls the program or sends messages is a step of one task
//! (TASKS' place 11). The parts of DefDlgProcA that several messages share
//! (setting the focus, finding and setting the default button) are
//! subroutines of it: a step jumps to them with the step to go on with,
//! which the task keeps on a small stack in its variables.

use super::message::{
    WM_ACTIVATE, WM_CHAR, WM_CHARTOITEM, WM_CLOSE, WM_COMMAND, WM_COMPAREITEM, WM_CTLCOLOR,
    WM_CTLCOLORDLG, WM_CTLCOLORMSGBOX, WM_CTLCOLORSTATIC, WM_ENTERIDLE, WM_ENTERMENULOOP,
    WM_ERASEBKGND, WM_GETDLGCODE, WM_GETFONT, WM_GETTEXT, WM_GETTEXTLENGTH, WM_INITDIALOG,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_NCDESTROY, WM_NCLBUTTONDOWN, WM_NEXTDLGCTL, WM_NULL,
    WM_QUERYDRAGICON, WM_SETFOCUS, WM_SETFONT, WM_SETTEXT, WM_SHOWWINDOW, WM_SYSCHAR, WM_TIMER,
    WM_USER, WM_VKEYTOITEM,
};
use super::window::{
    self, CW_USEDEFAULT, DWL_DLGPROC, DWL_MSGRESULT, SW_SHOWNORMAL, SWP_HIDEWINDOW, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_CAPTION, WS_CHILD, WS_DISABLED, WS_EX_CLIENTEDGE,
    WS_EX_CONTROLPARENT, WS_EX_DLGMODALFRAME, WS_EX_NOPARENTNOTIFY, WS_EX_TOPMOST, WS_GROUP,
    WS_MINIMIZE, WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, send,
};
use super::{
    DESKTOP, ERROR_INVALID_WINDOW_HANDLE, FALSE, State, TRUE, address_of, bytes_at, copy_out,
    current_tid, make_long, read_words, set_error, user, write_words,
};
use crate::cpu::Cpu;
use crate::win32::convert::CodePage;
use crate::win32::gdi32::{self, Font, Kind, Rect};
use crate::win32::kernel32;
use crate::win32::task::{self, Next, Task};

/// The dialog task's place in `task::TASKS`.
const TASK_DIALOG: u16 = 11;

// Dialog styles (DS_*).
const DS_ABSALIGN: u32 = 0x0001;
const DS_SETFONT: u32 = 0x0040;
const DS_MODALFRAME: u32 = 0x0080;
const DS_NOIDLEMSG: u32 = 0x0100;
const DS_NOFAILCREATE: u32 = 0x0010;
const DS_CONTROL: u32 = 0x0400;
const DS_CENTER: u32 = 0x0800;
const DS_CENTERMOUSE: u32 = 0x1000;

// Dialog messages and WM_GETDLGCODE's codes.
const DM_GETDEFID: u32 = WM_USER;
const DM_SETDEFID: u32 = WM_USER + 1;
const DC_HASDEFID: u32 = 0x534B;
const DLGC_WANTARROWS: u32 = 0x0001;
const DLGC_WANTTAB: u32 = 0x0002;
const DLGC_WANTMESSAGE: u32 = 0x0004;
const DLGC_HASSETSEL: u32 = 0x0008;
const DLGC_DEFPUSHBUTTON: u32 = 0x0010;
const DLGC_UNDEFPUSHBUTTON: u32 = 0x0020;
const DLGC_WANTCHARS: u32 = 0x0080;
const DLGC_STATIC: u32 = 0x0100;
const DLGC_BUTTON: u32 = 0x2000;

// Messages and styles of the controls.
const BM_GETCHECK: u32 = 0x00F0;
const BM_SETCHECK: u32 = 0x00F1;
const BM_SETSTYLE: u32 = 0x00F4;
const BM_CLICK: u32 = 0x00F5;
const BS_PUSHBUTTON: u32 = 0;
const BS_DEFPUSHBUTTON: u32 = 1;
const BS_GROUPBOX: u32 = 7;
const EM_SETSEL: u32 = 0x00B1;
const CB_SHOWDROPDOWN: u32 = 0x014F;
const STM_SETICON: u32 = 0x0170;
const SS_ICON: u32 = 0x0003;
const SS_NOPREFIX: u32 = 0x0080;

// Keys.
const VK_CANCEL: u32 = 0x03;
const VK_TAB: u32 = 0x09;
const VK_RETURN: u32 = 0x0D;
const VK_SHIFT: u32 = 0x10;
const VK_ESCAPE: u32 = 0x1B;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;
const VK_EXECUTE: u32 = 0x2B;

// The commands of a dialog's buttons.
const IDOK: u32 = 1;
const IDCANCEL: u32 = 2;
const IDABORT: u32 = 3;
const IDRETRY: u32 = 4;
const IDIGNORE: u32 = 5;
const IDYES: u32 = 6;
const IDNO: u32 = 7;
const IDHELP: u32 = 9;
const IDTRYAGAIN: u32 = 10;
const IDCONTINUE: u32 = 11;

// MessageBoxA's styles.
const MB_OK: u32 = 0;
const MB_OKCANCEL: u32 = 1;
const MB_ABORTRETRYIGNORE: u32 = 2;
const MB_YESNOCANCEL: u32 = 3;
const MB_YESNO: u32 = 4;
const MB_RETRYCANCEL: u32 = 5;
const MB_CANCELTRYCONTINUE: u32 = 6;
const MB_TYPEMASK: u32 = 0x000F;
const MB_ICONHAND: u32 = 0x0010;
const MB_ICONQUESTION: u32 = 0x0020;
const MB_ICONEXCLAMATION: u32 = 0x0030;
const MB_ICONASTERISK: u32 = 0x0040;
const MB_USERICON: u32 = 0x0080;
const MB_ICONMASK: u32 = 0x00F0;
const MB_DEFMASK: u32 = 0x0F00;
const MB_SYSTEMMODAL: u32 = 0x1000;
const MB_TASKMODAL: u32 = 0x2000;
const MB_HELP: u32 = 0x4000;
const MB_TOPMOST: u32 = 0x0004_0000;

/// The ids of a message box's icon and text.
const MSGBOX_IDICON: u32 = 0x0440;
const MSGBOX_IDTEXT: u32 = 0xFFFF;

/// The dialog class's atom, which a template without a class names.
const DIALOG_ATOM: u32 = 32770;
const RT_DIALOG: u32 = 5;
const PM_REMOVE: u32 = 1;
const MSGF_DIALOGBOX: u32 = 0;

// A dialog's state in its extra bytes.
const DLGWINDOWEXTRA: usize = 30;
const I_FOCUS: usize = 12;
const I_FONT: usize = 16;
const I_RESULT: usize = 20;
const I_XB: usize = 24;
const I_YB: usize = 25;
const I_FLAGS: usize = 26;
const I_MB_STYLE: usize = 28;

/// EndDialog was called.
const DF_END: u16 = 0x0001;
/// A modal dialog disabled its owner, which it enables again.
const DF_OWNERENABLED: u16 = 0x0002;
/// A message box: DefDlgProcA is its dialog procedure too.
const DF_MSGBOX: u16 = 0x0100;
/// The state was made.
const DF_INIT: u16 = 0x8000;

/// A dialog's state (Wine's DIALOGINFO).
#[derive(Clone, Copy, Debug, Default)]
struct Info {
    focus: u32,
    font: u32,
    result: u32,
    xb: u8,
    yb: u8,
    flags: u16,
    mb_style: u16,
}

fn word_at(e: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([e[at], e[at + 1], e[at + 2], e[at + 3]])
}

/// The state of the dialog `hwnd`, if it has one.
fn info(state: &State, hwnd: u32) -> Option<Info> {
    let w = state.window(hwnd)?;
    if hwnd == DESKTOP || w.extra.len() < DLGWINDOWEXTRA {
        return None;
    }
    let e = &w.extra;
    let flags = u16::from_le_bytes([e[I_FLAGS], e[I_FLAGS + 1]]);
    if flags & DF_INIT == 0 {
        return None;
    }
    Some(Info {
        focus: word_at(e, I_FOCUS),
        font: word_at(e, I_FONT),
        result: word_at(e, I_RESULT),
        xb: e[I_XB],
        yb: e[I_YB],
        flags,
        mb_style: u16::from_le_bytes([e[I_MB_STYLE], e[I_MB_STYLE + 1]]),
    })
}

/// Whether `hwnd` is a dialog, of the dialog class or of a program's own
/// class with DLGWINDOWEXTRA bytes that a dialog function created.
pub(crate) fn is_dialog(state: &State, hwnd: u32) -> bool {
    info(state, hwnd).is_some()
}

fn set_info(state: &mut State, hwnd: u32, i: &Info) {
    let Some(w) = state.window_mut(hwnd) else {
        return;
    };
    if w.extra.len() < DLGWINDOWEXTRA {
        return;
    }
    let e = &mut w.extra;
    e[I_FOCUS..I_FOCUS + 4].copy_from_slice(&i.focus.to_le_bytes());
    e[I_FONT..I_FONT + 4].copy_from_slice(&i.font.to_le_bytes());
    e[I_RESULT..I_RESULT + 4].copy_from_slice(&i.result.to_le_bytes());
    e[I_XB] = i.xb;
    e[I_YB] = i.yb;
    e[I_FLAGS..I_FLAGS + 2].copy_from_slice(&i.flags.to_le_bytes());
    e[I_MB_STYLE..I_MB_STYLE + 2].copy_from_slice(&i.mb_style.to_le_bytes());
}

/// The dialog's state, made if it has none yet (DIALOG_get_info with
/// create): None for a window without room for it.
fn make_info(state: &mut State, hwnd: u32) -> Option<Info> {
    if let Some(i) = info(state, hwnd) {
        return Some(i);
    }
    let w = state.window(hwnd)?;
    if hwnd == DESKTOP || w.extra.len() < DLGWINDOWEXTRA {
        return None;
    }
    let i = Info {
        result: IDOK,
        flags: DF_INIT,
        ..Info::default()
    };
    set_info(state, hwnd, &i);
    Some(i)
}

fn update_info(state: &mut State, hwnd: u32, f: impl FnOnce(&mut Info)) {
    if let Some(mut i) = info(state, hwnd) {
        f(&mut i);
        set_info(state, hwnd, &i);
    }
}

/// The DWL_ value at `at` of the window's extra bytes, or 0.
fn dlg_long(state: &State, hwnd: u32, at: i32) -> u32 {
    let at = at as usize;
    match state.window(hwnd) {
        Some(w) if w.extra.len() >= at + 4 => word_at(&w.extra, at),
        _ => 0,
    }
}

fn set_dlg_long(state: &mut State, hwnd: u32, at: i32, value: u32) {
    let at = at as usize;
    if let Some(w) = state.window_mut(hwnd)
        && w.extra.len() >= at + 4
    {
        w.extra[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
}

// The window tree, as GetWindow and GetParent walk it.

fn style(state: &State, hwnd: u32) -> u32 {
    state.window(hwnd).map_or(0, |w| w.style)
}

fn ex_style(state: &State, hwnd: u32) -> u32 {
    state.window(hwnd).map_or(0, |w| w.ex_style)
}

/// Whether the window is visible and enabled.
fn usable(state: &State, hwnd: u32) -> bool {
    style(state, hwnd) & (WS_VISIBLE | WS_DISABLED) == WS_VISIBLE
}

fn children(state: &State, hwnd: u32) -> Vec<u32> {
    state
        .window(hwnd)
        .map(|w| w.children.clone())
        .unwrap_or_default()
}

/// GetWindow(GW_CHILD).
fn first_child(state: &State, hwnd: u32) -> u32 {
    state
        .window(hwnd)
        .and_then(|w| w.children.first().copied())
        .unwrap_or(0)
}

/// GetWindow(GW_HWNDNEXT), or GW_HWNDPREV when `prev`.
fn sibling(state: &State, hwnd: u32, prev: bool) -> u32 {
    let Some(w) = state.window(hwnd) else {
        return 0;
    };
    let Some(list) = state.window(w.parent).map(|p| &p.children) else {
        return 0;
    };
    let Some(i) = list.iter().position(|&h| h == hwnd) else {
        return 0;
    };
    let j = if prev { i.checked_sub(1) } else { Some(i + 1) };
    j.and_then(|j| list.get(j).copied()).unwrap_or(0)
}

/// GetWindow(GW_HWNDLAST), or GW_HWNDFIRST when not `last`.
fn end_sibling(state: &State, hwnd: u32, last: bool) -> u32 {
    let Some(w) = state.window(hwnd) else {
        return 0;
    };
    let Some(list) = state.window(w.parent).map(|p| &p.children) else {
        return 0;
    };
    let end = if last { list.last() } else { list.first() };
    end.copied().unwrap_or(0)
}

/// GetParent: a child's parent, a popup's owner, or 0.
fn parent_of(state: &State, hwnd: u32) -> u32 {
    match state.window(hwnd) {
        Some(w) if w.style & WS_CHILD != 0 => w.parent,
        Some(w) if w.style & WS_POPUP != 0 => w.owner,
        _ => 0,
    }
}

/// GetAncestor(GA_ROOT), or 0 for what isn't a window.
fn root_of(state: &State, hwnd: u32) -> u32 {
    if hwnd == DESKTOP || !state.is_window(hwnd) {
        return 0;
    }
    let mut h = hwnd;
    while state.parent(h) != DESKTOP && state.parent(h) != 0 {
        h = state.parent(h);
    }
    h
}

fn ctrl_id(state: &State, hwnd: u32) -> u32 {
    state.window(hwnd).map_or(0, |w| w.id)
}

/// GetDlgItem: the first child, from the top of the z-order, with the
/// id.
fn dlg_item(state: &State, dlg: u32, id: u32) -> u32 {
    children(state, dlg)
        .into_iter()
        .find(|&c| ctrl_id(state, c) == id)
        .unwrap_or(0)
}

/// DIALOG_IdToHwnd: the descendant with the id, a child first. A
/// property sheet's default button can be on one of its pages.
fn id_to_hwnd(state: &State, dlg: u32, id: u32) -> u32 {
    let mut list = Vec::new();
    descendants(state, dlg, &mut list);
    list.into_iter()
        .find(|&c| ctrl_id(state, c) == id)
        .unwrap_or(0)
}

/// The window's descendants in the order EnumChildWindows gives them.
fn descendants(state: &State, hwnd: u32, out: &mut Vec<u32>) {
    for c in children(state, hwnd) {
        out.push(c);
        descendants(state, c, out);
    }
}

/// The controls DEFDLG_FindDefButton asks, in its order: it goes into
/// the visible, enabled WS_EX_CONTROLPARENT controls.
fn button_candidates(state: &State, hwnd: u32, out: &mut Vec<u32>) {
    for c in children(state, hwnd) {
        out.push(c);
        if ex_style(state, c) & WS_EX_CONTROLPARENT != 0 && usable(state, c) {
            button_candidates(state, c, out);
        }
    }
}

/// How far a walk of the tree goes before it gives up on a loop.
const MAX_WALK: usize = 0x4000;

/// GetNextDlgGroupItem.
fn next_group_item(state: &State, dlg: u32, ctrl: u32, prev: bool) -> u32 {
    let ctrl = if ctrl == dlg { 0 } else { ctrl };
    if ctrl == 0 && prev {
        return 0;
    }
    let ctrl = if ctrl != 0 {
        if !state.is_descendant(dlg, ctrl) {
            return 0;
        }
        ctrl
    } else {
        // No control: the first child, if it can take the focus.
        let first = first_child(state, dlg);
        if first == 0 || usable(state, first) {
            return first;
        }
        first
    };
    // Go forward around the group: for the previous control keep the
    // last one found, for the next stop at the first.
    let mut ret = ctrl;
    let mut hwnd = ctrl;
    let mut last_group = 0;
    let (mut looped, mut skipping) = (false, false);
    for _ in 0..MAX_WALK {
        let mut next = sibling(state, hwnd, false);
        while next == 0 {
            // Climb out until an ancestor has a next sibling, or wrap
            // around to the first control once.
            if hwnd == 0 {
                return ret;
            }
            if dlg == parent_of(state, hwnd) {
                if looped {
                    return ret;
                }
                looped = true;
                next = first_child(state, dlg);
                if next == 0 {
                    return ret;
                }
            } else {
                hwnd = parent_of(state, hwnd);
                next = sibling(state, hwnd, false);
            }
        }
        hwnd = next;
        // Go down the leading edge of control parents.
        while ex_style(state, hwnd) & WS_EX_CONTROLPARENT != 0 && usable(state, hwnd) {
            let child = first_child(state, hwnd);
            if child == 0 {
                break;
            }
            hwnd = child;
        }
        if style(state, hwnd) & WS_GROUP != 0 {
            last_group = hwnd;
            skipping = true;
        }
        if hwnd == ctrl {
            if !skipping || last_group == hwnd {
                break;
            }
            hwnd = last_group;
            skipping = false;
            looped = false;
        }
        if !skipping && usable(state, hwnd) {
            ret = hwnd;
            if !prev {
                break;
            }
        }
    }
    ret
}

/// DIALOG_GetNextTabItem: the next (or previous) control with
/// WS_TABSTOP that can take the focus, going into control parents.
fn next_tab_item(state: &State, main: u32, dlg: u32, ctrl: u32, prev: bool, depth: u32) -> u32 {
    if depth > 64 {
        return 0;
    }
    let mut first = 0;
    if ctrl == 0 {
        first = first_child(state, dlg);
        if prev {
            first = end_sibling(state, first, true);
        }
    } else if state.is_descendant(main, ctrl) {
        first = sibling(state, ctrl, prev);
        if first == 0 {
            let parent = parent_of(state, ctrl);
            first = if parent != main {
                // Not at the top of the recursion.
                sibling(state, parent, prev)
            } else {
                end_sibling(state, ctrl, prev)
            };
        }
    }
    let mut guard = 0;
    while first != 0 && guard < MAX_WALK {
        guard += 1;
        let s = style(state, first);
        let ex = ex_style(state, first);
        if ex & WS_EX_CONTROLPARENT != 0 && s & WS_VISIBLE != 0 && s & WS_DISABLED == 0 {
            let found = next_tab_item(state, main, first, 0, prev, depth + 1);
            if found != 0 {
                return found;
            }
        } else if s & WS_TABSTOP != 0 && s & WS_VISIBLE != 0 && s & WS_DISABLED == 0 {
            return first;
        }
        first = sibling(state, first, prev);
    }
    let mut ret = 0;
    if ctrl != 0 {
        let mut parent = parent_of(state, ctrl);
        while parent != 0 && parent != main {
            ret = next_tab_item(
                state,
                main,
                parent_of(state, parent),
                parent,
                prev,
                depth + 1,
            );
            if ret != 0 {
                break;
            }
            parent = parent_of(state, parent);
        }
        if ret == 0 {
            ret = next_tab_item(state, main, main, 0, prev, depth + 1);
        }
    }
    if ret != 0 { ret } else { ctrl }
}

/// GetNextDlgTabItem.
fn next_tab(state: &State, dlg: u32, ctrl: u32, prev: bool) -> u32 {
    let ctrl = if ctrl == dlg { 0 } else { ctrl };
    if ctrl == 0 && prev {
        return 0;
    }
    next_tab_item(state, dlg, dlg, ctrl, prev, 0)
}

/// DIALOG_FindMsgDestination: a DS_CONTROL dialog's messages go up to the
/// dialog it is in.
fn message_destination(state: &State, mut dlg: u32) -> u32 {
    for _ in 0..MAX_WALK {
        if style(state, dlg) & DS_CONTROL == 0 {
            break;
        }
        let parent = parent_of(state, dlg);
        if parent == 0 || parent == DESKTOP || info(state, parent).is_none() {
            break;
        }
        dlg = parent;
    }
    dlg
}

/// The controls DIALOG_IsAccelerator asks for their dialog codes, in its
/// order: from `start` through the dialog's tree and around to `start`,
/// the visible, enabled ones.
fn accelerator_order(state: &State, start: u32, dlg: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut ctl = start;
    for _ in 0..MAX_WALK {
        let mut next = 0;
        if usable(state, ctl) {
            out.push(ctl);
            next = first_child(state, ctl);
        }
        if next == 0 {
            next = sibling(state, ctl, false);
        }
        while next == 0 && ctl != 0 {
            ctl = parent_of(state, ctl);
            if ctl == dlg {
                if start == dlg {
                    next = start;
                    break;
                }
                next = first_child(state, dlg);
            } else {
                next = sibling(state, ctl, false);
            }
        }
        ctl = next;
        if ctl == 0 || ctl == start {
            break;
        }
    }
    out
}

/// Whether the text's mnemonic (the character after a single &) is the
/// character `key`.
fn has_accelerator(text: &[u8], key: u32) -> bool {
    let mut i = 0;
    while i < text.len() {
        if text[i] == b'&' {
            match text.get(i + 1) {
                Some(b'&') => {
                    i += 2;
                    continue;
                }
                Some(&c) => return c.to_ascii_uppercase() as u32 == upper(key),
                None => return false,
            }
        }
        i += 1;
    }
    false
}

fn upper(c: u32) -> u32 {
    if c < 0x80 {
        (c as u8).to_ascii_uppercase() as u32
    } else {
        c
    }
}

/// MulDiv: a * b / c, rounded half away from zero.
fn mul_div(a: i32, b: i32, c: i32) -> i32 {
    if c == 0 {
        return -1;
    }
    let p = a as i64 * b as i64;
    let q = (p.abs() * 2 + (c as i64).abs()) / (2 * (c as i64).abs());
    if (p < 0) != (c < 0) {
        -q as i32
    } else {
        q as i32
    }
}

/// A dialog's place in pixels from `v` in dialog units, `base` the base
/// unit and `div` 4 across or 8 down. Windows 98 adds half of `div` and
/// cuts toward zero, which rounds a negative place up: -1 unit across
/// with a base of 6 is -1 pixel, where MulDiv makes it -2.
fn dialog_place(v: i32, base: i32, div: i32) -> i32 {
    (v * base + div / 2) / div
}

/// GdiGetCharDimensions of the font: the average width of the letters
/// and the height, or 0s.
fn char_dimensions(cpu: &mut Cpu, font: u32) -> (i32, i32) {
    let hdc = gdi32::dc::new_dc(cpu, gdi32::dc::MEMORY);
    if hdc == 0 {
        return (0, 0);
    }
    if font != 0 {
        gdi32::dc::with(cpu, hdc, |dc| dc.attrs.font = font);
    }
    let letters = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let width = gdi32::font::text_extent(cpu, hdc, letters).map_or(0, |(w, _)| w);
    let height = gdi32::font::metrics(cpu, hdc).map_or(0, |m| m.height);
    gdi32::dc::delete_dc(cpu, &[hdc]);
    ((width / 26 + 1) / 2, height)
}

/// GetDialogBaseUnits: the system font's dimensions.
fn base_units(cpu: &mut Cpu) -> (i32, i32) {
    char_dimensions(cpu, 0)
}

fn pack(x: i32, y: i32) -> u32 {
    (x as u16 as u32) | (y as u16 as u32) << 16
}

fn unpack(v: u32) -> (i32, i32) {
    (v as u16 as i32, (v >> 16) as u16 as i32)
}

// Dialog templates.

/// A name in a template: nothing, a number or a string.
#[derive(Clone, Debug)]
enum Res {
    None,
    Id(u16),
    Text(Vec<u16>),
}

/// A dialog template's header (DLGTEMPLATE or DLGTEMPLATEEX).
struct Header {
    ex_form: bool,
    help: u32,
    style: u32,
    ex: u32,
    count: u32,
    x: i16,
    y: i16,
    cx: i16,
    cy: i16,
    menu: Res,
    class: Res,
    caption: Vec<u16>,
    point: u16,
    weight: u16,
    italic: u8,
    face: Vec<u16>,
    /// Where the first control is.
    items: u32,
}

/// A control of a template.
struct Item {
    help: u32,
    style: u32,
    ex: u32,
    x: i16,
    y: i16,
    cx: i16,
    cy: i16,
    id: u32,
    class: Res,
    title: Res,
    data: u32,
}

fn r16(cpu: &mut Cpu, at: u32) -> u16 {
    cpu.bus.guest_read_16(at)
}

fn r32(cpu: &mut Cpu, at: u32) -> u32 {
    cpu.bus.guest_read_32(at)
}

/// The UTF-16 string at `at` and where it ends.
fn wide_string(cpu: &mut Cpu, at: u32) -> (Vec<u16>, u32) {
    let mut s = Vec::new();
    let mut p = at;
    loop {
        let c = r16(cpu, p);
        p += 2;
        if c == 0 || s.len() >= 0x8000 || cpu.bus.guest_faulted() {
            break;
        }
        s.push(c);
    }
    (s, p)
}

/// A menu or class name of a header: 0, FFFFh and a number, or a string.
fn name_at(cpu: &mut Cpu, at: u32) -> (Res, u32) {
    match r16(cpu, at) {
        0 => (Res::None, at + 2),
        0xFFFF => (Res::Id(r16(cpu, at + 2)), at + 4),
        _ => {
            let (s, next) = wide_string(cpu, at);
            (Res::Text(s), next)
        }
    }
}

/// DIALOG_ParseTemplate32. None if a page isn't there.
fn header(cpu: &mut Cpu, at: u32) -> Option<Header> {
    let ex_form = r16(cpu, at) == 1 && r16(cpu, at + 2) == 0xFFFF;
    let (help, ex, style, mut p) = if ex_form {
        (
            r32(cpu, at + 4),
            r32(cpu, at + 8),
            r32(cpu, at + 12),
            at + 16,
        )
    } else {
        (0, r32(cpu, at + 4), r32(cpu, at), at + 8)
    };
    let count = r16(cpu, p) as u32;
    let x = r16(cpu, p + 2) as i16;
    let y = r16(cpu, p + 4) as i16;
    let cx = r16(cpu, p + 6) as i16;
    let cy = r16(cpu, p + 8) as i16;
    p += 10;
    let (menu, next) = name_at(cpu, p);
    let (class, next) = name_at(cpu, next);
    let (caption, mut p) = wide_string(cpu, next);
    let (mut point, mut weight, mut italic, mut face) = (0, 0, 0, Vec::new());
    if style & DS_SETFONT != 0 {
        point = r16(cpu, p);
        p += 2;
        // 7FFFh: the message box font, with nothing more.
        if point != 0x7FFF {
            if ex_form {
                weight = r16(cpu, p);
                italic = r16(cpu, p + 2) as u8;
                p += 4;
            }
            let (f, next) = wide_string(cpu, p);
            face = f;
            p = next;
        }
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(Header {
        ex_form,
        help,
        style,
        ex,
        count,
        x,
        y,
        cx,
        cy,
        menu,
        class,
        caption,
        point,
        weight,
        italic,
        face,
        items: (p + 3) & !3,
    })
}

/// The classes a template names by number, 80h to 85h (or 0 to 5).
const CLASSES: [&str; 6] = [
    "Button",
    "Edit",
    "Static",
    "ListBox",
    "ScrollBar",
    "ComboBox",
];

/// DIALOG_GetControl32: the control at `at` and where the next one is.
fn item(cpu: &mut Cpu, at: u32, ex_form: bool) -> (Item, u32) {
    let (help, ex, style, mut p) = if ex_form {
        (r32(cpu, at), r32(cpu, at + 4), r32(cpu, at + 8), at + 12)
    } else {
        (0, r32(cpu, at + 4), r32(cpu, at), at + 8)
    };
    let x = r16(cpu, p) as i16;
    let y = r16(cpu, p + 2) as i16;
    let cx = r16(cpu, p + 4) as i16;
    let cy = r16(cpu, p + 6) as i16;
    p += 8;
    let id = if ex_form {
        p += 4;
        r32(cpu, p - 4)
    } else {
        p += 2;
        r16(cpu, p - 2) as u32
    };
    let class = if r16(cpu, p) == 0xFFFF {
        let mut n = r16(cpu, p + 2);
        p += 4;
        if (0x80..=0x85).contains(&n) {
            n -= 0x80;
        }
        match CLASSES.get(n as usize) {
            Some(name) => Res::Text(name.encode_utf16().collect()),
            None => Res::None,
        }
    } else {
        let (s, next) = wide_string(cpu, p);
        p = next;
        Res::Text(s)
    };
    let title = if r16(cpu, p) == 0xFFFF {
        let n = r16(cpu, p + 2);
        p += 4;
        Res::Id(n)
    } else {
        let (s, next) = wide_string(cpu, p);
        p = next;
        Res::Text(s)
    };
    // The creation data: its size, then the data the control gets.
    let size = r16(cpu, p) as u32;
    let data = if size != 0 {
        let d = p + 2;
        p += size / 2 * 2;
        d
    } else {
        0
    };
    p += 2;
    let item = Item {
        help,
        style,
        ex,
        x,
        y,
        cx,
        cy,
        id,
        class,
        title,
        data,
    };
    (item, (p + 3) & !3)
}

/// UTF-16 to the ANSI code page.
fn ansi(units: &[u16]) -> Vec<u8> {
    units
        .iter()
        .map(|&c| CodePage::Ansi.byte(c).unwrap_or(b'?'))
        .collect()
}

fn wide(bytes: &[u8]) -> Vec<u16> {
    bytes.iter().map(|&b| CodePage::Ansi.char(b)).collect()
}

/// Write `bytes` and a 0 to the `size` bytes at `at`: `at`.
fn put(cpu: &mut Cpu, at: u32, size: u32, bytes: &[u8]) -> u32 {
    copy_out(cpu, at, size, bytes);
    at
}

/// A class or window name for CreateWindowExA, in the scratch space at
/// `at` if it is a string. A control's title that is a number goes as
/// "#123", which LoadIconA and LoadBitmapA take as the number.
fn put_name(cpu: &mut Cpu, at: u32, size: u32, name: &Res, number_as_text: bool) -> u32 {
    match name {
        Res::None => 0,
        Res::Id(n) if number_as_text => put(cpu, at, size, format!("#{}", n).as_bytes()),
        Res::Id(n) => *n as u32,
        Res::Text(s) => put(cpu, at, size, &ansi(s)),
    }
}

/// The LOGFONTA of a template's font.
fn template_font(h: &Header) -> Vec<u8> {
    if h.point == 0x7FFF {
        return super::sysparams::sans_serif(false);
    }
    // Points to pixels at 96 dots an inch, negative for the em height.
    let pixels = mul_div(h.point as i16 as i32, 96, 72);
    let mut log = gdi32::font::logfont(
        -pixels,
        0,
        h.weight as i32,
        gdi32::font::DEFAULT_CHARSET,
        0,
        "",
    );
    log[20] = h.italic;
    // PROOF_QUALITY.
    log[26] = 2;
    let face = ansi(&h.face);
    let n = face.len().min(31);
    log[28..28 + n].copy_from_slice(&face[..n]);
    log
}

// The task.

/// What the task does, its first variable.
const K_CREATE: u32 = 0;
const K_DEFDLG: u32 = 1;
const K_END: u32 = 2;
const K_ISDLG: u32 = 3;
const K_SETINT: u32 = 4;
const K_GETINT: u32 = 5;
const K_SEND: u32 = 6;
const K_RADIO: u32 = 7;

/// The dialog the task works on.
const V_HWND: usize = 1;

// Dialog creation's variables.
const C_INST: usize = 2;
const C_TEMPLATE: usize = 3;
const C_OWNER: usize = 4;
const C_PROC: usize = 5;
const C_PARAM: usize = 6;
const C_MODE: usize = 7;
const C_FAIL: usize = 8;
const C_ROOT: usize = 9;
const C_FLAGS: usize = 10;
const C_FONT: usize = 11;
const C_UNITS: usize = 12;
const C_SCALE: usize = 13;
const C_MENU: usize = 14;
const C_STYLE: usize = 15;
const C_EX: usize = 16;
const C_POS: usize = 17;
const C_SIZE: usize = 18;
const C_ITEM: usize = 19;
const C_PTR: usize = 20;
const C_CTRL: usize = 21;
const C_DEF: usize = 22;
const C_ICON: usize = 23;
const C_MB: usize = 24;
const C_MB_DEF: usize = 25;
const C_PAGES: usize = 26;
const C_FIRST: usize = 27;
const C_RESULT: usize = 28;
const C_I: usize = 29;

/// Dialog creation's modes.
const M_MODAL: u32 = 1;
const M_MSGBOX: u32 = 2;
const M_TASKMODAL: u32 = 4;

// DefDlgProcA's variables: the message, the dialog procedure's result
// and a value a branch keeps.
const D_MSG: usize = 2;
const D_WP: usize = 3;
const D_LP: usize = 4;
const D_RESULT: usize = 5;
const D_TMP: usize = 6;

// IsDialogMessageA's variables.
const I_MSGPTR: usize = 2;
const I_MHWND: usize = 3;
const I_MSG: usize = 4;
const I_WP: usize = 5;
const I_CODE: usize = 6;
const I_NEXT: usize = 7;
const I_I: usize = 8;

// The subroutines' variables.
const S_CTRL: usize = 30;
const S_NEW: usize = 31;
const S_OLD: usize = 32;
const S_CODE: usize = 33;
const S_I: usize = 34;
const S_FOUND: usize = 35;
const S_ID: usize = 36;

/// The subroutines' return steps, and a list of windows.
const SP: usize = 37;
const LIST_LEN: usize = 38;
const STACK: usize = 40;
const STACK_DEPTH: usize = 16;
const LIST: usize = STACK + STACK_DEPTH;

// The subroutines.
const SUB_SET_FOCUS: u16 = 600;
const SUB_RESTORE_FOCUS: u16 = 610;
const SUB_FIND_DEF: u16 = 620;
const SUB_SET_DEF_BUTTON: u16 = 630;
const SUB_SET_DEF_ID: u16 = 640;

/// Creation's scratch space: a MSG, a class name, a title.
const MSG_AT: u32 = 0;
const CLASS_AT: u32 = 0x20;
const CLASS_SIZE: u32 = 0x100;
const TITLE_AT: u32 = CLASS_AT + CLASS_SIZE;
const SCRATCH: u32 = 0x1000;
const TITLE_SIZE: u32 = SCRATCH - TITLE_AT;

/// Go on at `step` with 0.
fn goto(step: u16) -> Next {
    Next::call(0, &[], step)
}

/// Run the subroutine at `sub`, then go on at `back`.
fn gosub(t: &mut Task, sub: u16, back: u16) -> Next {
    let sp = t.var(SP) as usize;
    if sp >= STACK_DEPTH {
        return Next::Done(0);
    }
    t.set(STACK + sp, back as u32);
    t.set(SP, sp as u32 + 1);
    goto(sub)
}

/// The end of a subroutine: back where it was run from.
fn ret(t: &mut Task) -> Next {
    let sp = t.var(SP) as usize;
    if sp == 0 {
        return Next::Done(0);
    }
    t.set(SP, sp as u32 - 1);
    goto(t.var(STACK + sp - 1) as u16)
}

fn set_list(t: &mut Task, list: &[u32]) {
    t.set(LIST_LEN, list.len() as u32);
    for (i, &h) in list.iter().enumerate() {
        t.set(LIST + i, h);
    }
}

fn list_at(t: &Task, i: u32) -> Option<u32> {
    (i < t.var(LIST_LEN)).then(|| t.var(LIST + i as usize))
}

/// The variables of a task of `kind`, with room for the list.
fn vars_with_list(mut vars: Vec<u32>, list: &[u32]) -> Vec<u32> {
    vars.resize(LIST, 0);
    vars[LIST_LEN] = list.len() as u32;
    vars.extend_from_slice(list);
    vars
}

fn call_api(name: &str, args: &[u32], step: u16) -> Next {
    Next::call(address_of(name), args, step)
}

/// The dialog task's steps.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.step {
        0 => match t.var(0) {
            K_CREATE => goto(80),
            K_DEFDLG => goto(300),
            K_END => goto(400),
            K_ISDLG => goto(500),
            K_SETINT => goto(700),
            K_GETINT => goto(710),
            K_SEND => goto(720),
            K_RADIO => goto(730),
            _ => Next::Done(0),
        },
        80..=199 => create_step(cpu, t, value),
        200..=299 => modal_step(cpu, t, value),
        300..=399 => def_step(cpu, t, value),
        400..=499 => end_step(cpu, t),
        500..=599 => message_step(cpu, t, value),
        600..=699 => sub_step(cpu, t, value),
        _ => item_step(cpu, t, value),
    }
}

// Making a dialog.

/// What a dialog is made from and how.
#[derive(Default)]
struct Create {
    inst: u32,
    template: u32,
    owner: u32,
    proc: u32,
    param: u32,
    mode: u32,
    fail: u32,
    font: u32,
    icon: u32,
    mb_style: u32,
    mb_default: u32,
    pages: u32,
    /// The windows a task-modal message box disabled.
    disabled: Vec<u32>,
}

/// DIALOG_CreateIndirect, and DIALOG_DoDialogBox after it for a modal
/// dialog.
fn create(cpu: &mut Cpu, c: Create) -> Option<u32> {
    header(cpu, c.template)?;
    let mut vars = vec![0; C_I + 1];
    vars[0] = K_CREATE;
    vars[C_INST] = c.inst;
    vars[C_TEMPLATE] = c.template;
    vars[C_OWNER] = c.owner;
    vars[C_PROC] = c.proc;
    vars[C_PARAM] = c.param;
    vars[C_MODE] = c.mode;
    vars[C_FAIL] = c.fail;
    vars[C_FONT] = c.font;
    vars[C_ICON] = c.icon;
    vars[C_MB] = c.mb_style;
    vars[C_MB_DEF] = c.mb_default;
    vars[C_PAGES] = c.pages;
    let vars = vars_with_list(vars, &c.disabled);
    task::start(cpu, TASK_DIALOG, vars, SCRATCH)
}

/// The task ends with `value`: a task-modal message box enables the
/// windows it disabled and lets its template go first.
fn finish(t: &mut Task, value: u32) -> Next {
    t.set(C_RESULT, value);
    t.set(C_I, 0);
    goto(250)
}

fn create_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let msgbox = t.var(C_MODE) & M_MSGBOX != 0;
    let modal = t.var(C_MODE) & M_MODAL != 0;
    match t.step {
        // A task-modal message box without an owner disables the
        // thread's windows first.
        80 | 81 => {
            if t.step == 81 {
                t.set(C_I, t.var(C_I) + 1);
            }
            match list_at(t, t.var(C_I)) {
                Some(h) => call_api("EnableWindow", &[h, FALSE], 81),
                None => goto(90),
            }
        }
        // The menu.
        90 => {
            let Some(h) = header(cpu, t.var(C_TEMPLATE)) else {
                return finish(t, t.var(C_FAIL));
            };
            let name = match &h.menu {
                Res::None => return goto(91),
                Res::Id(n) => *n as u32,
                Res::Text(s) => put(cpu, t.scratch + CLASS_AT, CLASS_SIZE, &ansi(s)),
            };
            call_api("LoadMenuA", &[t.var(C_INST), name], 91)
        }
        // The font, the units, where the dialog goes; a modal dialog
        // disables its owner.
        91 => {
            t.set(C_MENU, value);
            let Some(h) = header(cpu, t.var(C_TEMPLATE)) else {
                return finish(t, t.var(C_FAIL));
            };
            let (mut xb, mut yb) = base_units(cpu);
            if !msgbox && h.style & DS_SETFONT != 0 {
                let log = template_font(&h);
                let font = gdi32::add(cpu, Kind::Font { font: Font { log } });
                t.set(C_FONT, font);
                if font != 0 {
                    let (cx, cy) = char_dimensions(cpu, font);
                    if cx != 0 {
                        (xb, yb) = (cx, cy);
                    }
                }
            }
            // A message box's template is laid out in pixels.
            let (sx, sy) = if msgbox { (4, 8) } else { (xb, yb) };
            t.set(C_UNITS, pack(xb, yb));
            t.set(C_SCALE, pack(sx, sy));
            let mut style = h.style;
            let mut ex = h.ex;
            // A DS_CONTROL dialog, child or popup, loses its caption, its
            // system menu and with them WS_BORDER and WS_DLGFRAME. Any
            // other child dialog keeps them all and Windows 98 draws its
            // caption.
            if style & DS_CONTROL != 0 {
                style &= !(WS_CAPTION | WS_SYSMENU);
            }
            if style & DS_MODALFRAME != 0 {
                ex |= WS_EX_DLGMODALFRAME;
            }
            if style & DS_CONTROL != 0 {
                ex |= WS_EX_CONTROLPARENT;
            }
            let client = Rect::new(
                0,
                0,
                mul_div(h.cx as i32, sx, 4),
                mul_div(h.cy as i32, sy, 8),
            );
            let r = window::adjust(client, style, ex, t.var(C_MENU) != 0);
            let (cx, cy) = (r.width(), r.height());
            // Windows 98 puts the window's corner where the template
            // says, not its client area: the frame and the edges make the
            // window bigger to the right and down.
            let (mut x, mut y) = (0, 0);
            let owner = t.var(C_OWNER);
            let (sw, sh) = super::sysparams::screen_size(cpu);
            let state = user(cpu);
            if h.x == i16::MIN {
                x = CW_USEDEFAULT as i32;
                y = CW_USEDEFAULT as i32;
            } else {
                if style & DS_CENTER != 0 {
                    x = (sw - cx) / 2;
                    y = (sh - cy) / 2;
                } else if style & DS_CENTERMOUSE != 0 {
                    (x, y) = state.input.cursor;
                } else {
                    x += dialog_place(h.x as i32, sx, 4);
                    y += dialog_place(h.y as i32, sy, 8);
                    if style & (WS_CHILD | DS_ABSALIGN) == 0 && state.is_window(owner) {
                        let (ox, oy) = state.client_origin(owner);
                        x += ox;
                        y += oy;
                    }
                }
                if style & WS_CHILD == 0 {
                    // Keep it on the screen: Windows 98 lets the window
                    // reach the right edge, but keeps 4 pixels free below
                    // it, whatever its frame.
                    x = x.min(sw - cx);
                    y = y.min(sh - 4 - cy);
                }
                // A child dialog left of or above its parent's client
                // area starts at its edge, as on Windows 98; one past the
                // right or the bottom stays there.
                x = x.max(0);
                y = y.max(0);
            }
            t.set(C_STYLE, style);
            t.set(C_EX, ex);
            t.set(C_POS, pack(x, y));
            t.set(C_SIZE, pack(cx, cy));
            if x == CW_USEDEFAULT as i32 {
                t.set(C_POS, CW_USEDEFAULT);
            }
            if modal {
                let root = root_of(state, owner);
                t.set(C_ROOT, root);
                if root != 0 && style_of_root_enabled(state, root) {
                    t.set(C_FLAGS, DF_OWNERENABLED as u32);
                    return call_api("EnableWindow", &[root, FALSE], 92);
                }
            }
            goto(92)
        }
        // The dialog's window.
        92 => {
            let Some(h) = header(cpu, t.var(C_TEMPLATE)) else {
                return goto(93);
            };
            let s = t.scratch;
            let class = match &h.class {
                Res::None => DIALOG_ATOM,
                name => put_name(cpu, s + CLASS_AT, CLASS_SIZE, name, false),
            };
            let caption = put(cpu, s + TITLE_AT, TITLE_SIZE, &ansi(&h.caption));
            let (x, y) = if t.var(C_POS) == CW_USEDEFAULT {
                (CW_USEDEFAULT, CW_USEDEFAULT)
            } else {
                let (x, y) = unpack(t.var(C_POS));
                (x as u32, y as u32)
            };
            let (cx, cy) = unpack(t.var(C_SIZE));
            let args = [
                t.var(C_EX),
                class,
                caption,
                t.var(C_STYLE) & !WS_VISIBLE,
                x,
                y,
                cx as u32,
                cy as u32,
                t.var(C_OWNER),
                t.var(C_MENU),
                t.var(C_INST),
                0,
            ];
            call_api("CreateWindowExA", &args, 93)
        }
        93 => {
            let hwnd = value;
            if hwnd == 0 {
                if t.var(C_FONT) != 0 {
                    gdi32::delete_object(cpu, &[t.var(C_FONT)]);
                }
                return match t.var(C_MENU) {
                    0 => goto(94),
                    menu => call_api("DestroyMenu", &[menu], 94),
                };
            }
            t.set(V_HWND, hwnd);
            let help = header(cpu, t.var(C_TEMPLATE)).map_or(0, |h| h.help);
            let (xb, yb) = unpack(t.var(C_UNITS));
            let (font, proc) = (t.var(C_FONT), t.var(C_PROC));
            let mut flags = t.var(C_FLAGS) as u16;
            if msgbox {
                flags |= DF_MSGBOX;
            }
            let mb_style = t.var(C_MB) as u16;
            let state = user(cpu);
            if let Some(mut i) = make_info(state, hwnd) {
                i.focus = 0;
                i.font = font;
                i.xb = xb as u8;
                i.yb = yb as u8;
                i.flags = DF_INIT | flags;
                i.mb_style = mb_style;
                set_info(state, hwnd, &i);
            }
            if let Some(w) = state.window_mut(hwnd)
                && help != 0
            {
                w.help_id = help;
            }
            set_dlg_long(state, hwnd, DWL_DLGPROC, proc);
            let h = header(cpu, t.var(C_TEMPLATE));
            t.set(C_ITEM, 0);
            t.set(C_PTR, h.map_or(0, |h| h.items));
            if proc != 0 && font != 0 {
                return send(hwnd, WM_SETFONT, font, 0, 100);
            }
            goto(100)
        }
        // It wasn't made: the owner is enabled again.
        94 => {
            if t.var(C_FLAGS) & DF_OWNERENABLED as u32 != 0 {
                return call_api("EnableWindow", &[t.var(C_ROOT), TRUE], 95);
            }
            goto(95)
        }
        95 => finish(t, t.var(C_FAIL)),
        // The controls, one at a time.
        100 => {
            let Some(h) = header(cpu, t.var(C_TEMPLATE)) else {
                return goto(110);
            };
            if t.var(C_ITEM) >= h.count {
                return goto(110);
            }
            let (it, next) = item(cpu, t.var(C_PTR), h.ex_form);
            t.set(C_PTR, next);
            t.set(C_ITEM, t.var(C_ITEM) + 1);
            t.set(C_CTRL, it.help);
            let (mut style, mut ex) = (it.style, it.ex);
            if style & window::WS_BORDER != 0 {
                style &= !window::WS_BORDER;
                ex |= WS_EX_CLIENTEDGE;
            }
            let (sx, sy) = unpack(t.var(C_SCALE));
            let s = t.scratch;
            let class = put_name(cpu, s + CLASS_AT, CLASS_SIZE, &it.class, false);
            let title = put_name(cpu, s + TITLE_AT, TITLE_SIZE, &it.title, true);
            let args = [
                ex | WS_EX_NOPARENTNOTIFY,
                class,
                title,
                style | WS_CHILD,
                mul_div(it.x as i32, sx, 4) as u32,
                mul_div(it.y as i32, sy, 8) as u32,
                mul_div(it.cx as i32, sx, 4) as u32,
                mul_div(it.cy as i32, sy, 8) as u32,
                t.var(V_HWND),
                it.id,
                t.var(C_INST),
                it.data,
            ];
            call_api("CreateWindowExA", &args, 101)
        }
        101 => {
            let ctrl = value;
            if ctrl == 0 {
                if t.var(C_STYLE) & DS_NOFAILCREATE != 0 {
                    return goto(100);
                }
                return goto(120);
            }
            // The help id was kept while the control was made.
            let help = t.var(C_CTRL);
            if let Some(w) = user(cpu).window_mut(ctrl)
                && help != 0
            {
                w.help_id = help;
            }
            t.set(C_CTRL, ctrl);
            match t.var(C_FONT) {
                0 => goto(102),
                font => send(ctrl, WM_SETFONT, font, 0, 102),
            }
        }
        102 => send(t.var(C_CTRL), WM_GETDLGCODE, 0, 0, 103),
        103 => {
            if value & DLGC_DEFPUSHBUTTON == 0 {
                return goto(100);
            }
            // A later default button takes over from an earlier one.
            let (hwnd, ctrl, old) = (t.var(V_HWND), t.var(C_CTRL), t.var(C_DEF));
            t.set(C_DEF, ctrl);
            let state = user(cpu);
            let id = ctrl_id(state, ctrl);
            update_info(state, hwnd, |i| i.result = id);
            if old != 0 {
                return send(old, BM_SETSTYLE, BS_PUSHBUTTON, FALSE, 100);
            }
            goto(100)
        }
        // A message box's icon.
        110 => {
            let icon = t.var(C_ICON);
            let item = dlg_item(user(cpu), t.var(V_HWND), MSGBOX_IDICON);
            if msgbox && icon != 0 && item != 0 {
                return send(item, STM_SETICON, icon, 0, 111);
            }
            goto(111)
        }
        // WM_INITDIALOG, with the control that would get the focus.
        111 => {
            let hwnd = t.var(V_HWND);
            if t.var(C_PROC) == 0 && !msgbox {
                return goto(114);
            }
            let focus = next_tab(user(cpu), hwnd, 0, false);
            send(hwnd, WM_INITDIALOG, focus, t.var(C_PARAM), 112)
        }
        112 => {
            let hwnd = t.var(V_HWND);
            let style = t.var(C_STYLE);
            let state = user(cpu);
            if msgbox {
                // The default button has the focus.
                let def = dlg_item(state, hwnd, t.var(C_MB_DEF));
                if t.var(C_MB_DEF) != 0 && def != 0 {
                    return call_api("SetFocus", &[def], 114);
                }
                return goto(114);
            }
            if value != 0 && (style & DS_CONTROL == 0 || style & WS_VISIBLE != 0) {
                // TRUE asks for the first control with WS_TABSTOP, which
                // WM_INITDIALOG may have changed.
                let focus = next_tab(state, hwnd, 0, false);
                update_info(state, hwnd, |i| i.focus = focus);
                if focus != 0 {
                    return call_api("SetFocus", &[focus], 114);
                }
            }
            goto(114)
        }
        114 => {
            let hwnd = t.var(V_HWND);
            let shown = style(user(cpu), hwnd) & WS_VISIBLE != 0;
            if t.var(C_STYLE) & WS_VISIBLE != 0 && user(cpu).is_window(hwnd) && !shown {
                return call_api("ShowWindow", &[hwnd, SW_SHOWNORMAL], 115);
            }
            goto(115)
        }
        115 => {
            if modal {
                return goto(200);
            }
            finish(t, t.var(V_HWND))
        }
        // A control wasn't made: the dialog goes.
        120 => {
            if modal && t.var(C_FLAGS) & DF_OWNERENABLED as u32 != 0 {
                return call_api("EnableWindow", &[t.var(C_ROOT), TRUE], 121);
            }
            goto(121)
        }
        121 => {
            let hwnd = t.var(V_HWND);
            if user(cpu).is_window(hwnd) {
                return call_api("DestroyWindow", &[hwnd], 122);
            }
            goto(122)
        }
        _ => finish(t, t.var(C_FAIL)),
    }
}

/// Whether the owner's root window is enabled (IsWindowEnabled).
fn style_of_root_enabled(state: &State, root: u32) -> bool {
    state.is_window(root) && style(state, root) & WS_DISABLED == 0
}

/// DIALOG_DoDialogBox: the modal loop, and the task's end.
fn modal_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let hwnd = t.var(V_HWND);
    let msg = t.scratch + MSG_AT;
    match t.step {
        200 => {
            let Some(i) = info(user(cpu), hwnd) else {
                return finish(t, u32::MAX);
            };
            t.set(C_FIRST, 1);
            // Was EndDialog called in WM_INITDIALOG?
            if i.flags & DF_END != 0 {
                return goto(220);
            }
            goto(201)
        }
        201 => call_api("PeekMessageA", &[msg, 0, 0, 0, PM_REMOVE], 202),
        202 => {
            if value != 0 {
                return goto(206);
            }
            // The dialog shows the first time the queue is empty.
            if t.var(C_FIRST) != 0 {
                t.set(C_FIRST, 0);
                return call_api("ShowWindow", &[hwnd, SW_SHOWNORMAL], 203);
            }
            goto(203)
        }
        203 => {
            let root = t.var(C_ROOT);
            if style(user(cpu), hwnd) & DS_NOIDLEMSG == 0 && root != 0 {
                return send(root, WM_ENTERIDLE, MSGF_DIALOGBOX, hwnd, 204);
            }
            goto(204)
        }
        204 => call_api("GetMessageA", &[msg, 0, 0, 0], 205),
        205 => {
            if value == 0 {
                return goto(220);
            }
            goto(206)
        }
        206 => {
            let state = user(cpu);
            if !state.is_window(hwnd) {
                return finish(t, 0);
            }
            match info(state, hwnd) {
                Some(i) if i.flags & DF_END == 0 => call_api("IsDialogMessageA", &[hwnd, msg], 207),
                _ => goto(209),
            }
        }
        207 => {
            if value != 0 {
                return goto(209);
            }
            call_api("TranslateMessage", &[msg], 208)
        }
        208 => call_api("DispatchMessageA", &[msg], 209),
        209 => {
            let message = cpu.bus.guest_read_32(msg + 4);
            let state = user(cpu);
            if let Some(i) = info(state, hwnd)
                && i.flags & DF_END != 0
            {
                return goto(220);
            }
            if t.var(C_FIRST) != 0 && message == WM_TIMER && state.is_window(hwnd) {
                t.set(C_FIRST, 0);
                return call_api("ShowWindow", &[hwnd, SW_SHOWNORMAL], 201);
            }
            goto(201)
        }
        220 => {
            let i = info(user(cpu), hwnd).unwrap_or_default();
            t.set(C_RESULT, i.result);
            if i.flags & DF_OWNERENABLED != 0 && t.var(C_ROOT) != 0 {
                return call_api("EnableWindow", &[t.var(C_ROOT), TRUE], 221);
            }
            goto(221)
        }
        221 => call_api("DestroyWindow", &[hwnd], 222),
        222 => finish(t, t.var(C_RESULT)),
        // The end: what a task-modal message box disabled is enabled.
        250 | 251 => {
            if t.step == 251 {
                t.set(C_I, t.var(C_I) + 1);
            }
            if t.var(C_MODE) & M_TASKMODAL != 0
                && let Some(h) = list_at(t, t.var(C_I))
            {
                return call_api("EnableWindow", &[h, TRUE], 251);
            }
            if t.var(C_PAGES) != 0 {
                kernel32::free_pages(cpu, t.var(C_PAGES));
            }
            Next::Done(t.var(C_RESULT))
        }
        _ => Next::Done(0),
    }
}

// DefDlgProcA.

/// The messages DefDlgProcA handles when the dialog procedure didn't.
fn handled(msg: u32) -> bool {
    matches!(
        msg,
        WM_ERASEBKGND
            | WM_SHOWWINDOW
            | WM_ACTIVATE
            | WM_SETFOCUS
            | DM_SETDEFID
            | DM_GETDEFID
            | WM_NEXTDLGCTL
            | WM_GETFONT
            | WM_CLOSE
            | WM_NCDESTROY
            | WM_ENTERMENULOOP
            | WM_LBUTTONDOWN
            | WM_NCLBUTTONDOWN
    )
}

/// The messages whose value is the dialog procedure's own, not
/// DWL_MSGRESULT.
fn returns_result(msg: u32) -> bool {
    (WM_CTLCOLORMSGBOX..=WM_CTLCOLORSTATIC).contains(&msg)
        || matches!(
            msg,
            WM_CTLCOLOR
                | WM_COMPAREITEM
                | WM_VKEYTOITEM
                | WM_CHARTOITEM
                | WM_QUERYDRAGICON
                | WM_INITDIALOG
        )
}

/// DEFDLG_SaveFocus.
fn save_focus(state: &mut State, hwnd: u32) {
    let focus = state.input.focus;
    if focus == 0 || !state.is_descendant(hwnd, focus) {
        return;
    }
    update_info(state, hwnd, |i| i.focus = focus);
}

/// What a message box's button ends it with, if it ends it: Escape gives
/// IDCANCEL, which a box with only OK takes as OK.
fn message_box_end(state: &State, hwnd: u32, mb_style: u32, wp: u32) -> Option<u32> {
    let id = wp & 0xFFFF;
    match id {
        IDCANCEL if dlg_item(state, hwnd, IDCANCEL) == 0 => {
            (mb_style & MB_TYPEMASK == MB_OK).then_some(IDOK)
        }
        IDOK | IDCANCEL | IDABORT | IDRETRY | IDIGNORE | IDYES | IDNO | IDTRYAGAIN | IDCONTINUE => {
            Some(wp)
        }
        _ => None,
    }
}

fn def_window(t: &Task, step: u16) -> Next {
    let args = [t.var(V_HWND), t.var(D_MSG), t.var(D_WP), t.var(D_LP)];
    call_api("DefWindowProcA", &args, step)
}

fn def_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let (hwnd, msg, wp, lp) = (t.var(V_HWND), t.var(D_MSG), t.var(D_WP), t.var(D_LP));
    match t.step {
        // The dialog procedure, or a message box's own.
        300 => {
            let state = user(cpu);
            let proc = dlg_long(state, hwnd, DWL_DLGPROC);
            if proc != 0 {
                return Next::call(proc, &[hwnd, msg, wp, lp], 301);
            }
            if let Some(i) = info(state, hwnd)
                && i.flags & DF_MSGBOX != 0
                && msg == WM_COMMAND
                && let Some(code) = message_box_end(state, hwnd, i.mb_style as u32, wp)
            {
                return call_api("EndDialog", &[hwnd, code], 302);
            }
            goto(301)
        }
        302 => goto(301),
        301 => {
            t.set(D_RESULT, value);
            let state = user(cpu);
            if value != 0 || !state.is_window(hwnd) {
                return goto(398);
            }
            let i = info(state, hwnd).unwrap_or_default();
            match msg {
                WM_ERASEBKGND => send(hwnd, WM_CTLCOLORDLG, wp, hwnd, 310),
                WM_NCDESTROY => {
                    if i.font != 0 {
                        gdi32::delete_object(cpu, &[i.font]);
                    }
                    let state = user(cpu);
                    if info(state, hwnd).is_some() {
                        set_info(state, hwnd, &Info::default());
                    }
                    def_window(t, 399)
                }
                WM_SHOWWINDOW => {
                    if wp == 0 {
                        save_focus(state, hwnd);
                    }
                    def_window(t, 399)
                }
                WM_ACTIVATE => {
                    if wp != 0 {
                        return gosub(t, SUB_RESTORE_FOCUS, 397);
                    }
                    save_focus(state, hwnd);
                    Next::Done(0)
                }
                WM_SETFOCUS => gosub(t, SUB_RESTORE_FOCUS, 397),
                DM_SETDEFID => {
                    if i.flags & DF_END == 0 {
                        t.set(S_ID, wp);
                        return gosub(t, SUB_SET_DEF_ID, 396);
                    }
                    Next::Done(TRUE)
                }
                DM_GETDEFID => {
                    if i.flags & DF_END != 0 {
                        return Next::Done(0);
                    }
                    if i.result != 0 {
                        return Next::Done(make_long(i.result as i32, DC_HASDEFID as i32));
                    }
                    gosub(t, SUB_FIND_DEF, 395)
                }
                WM_NEXTDLGCTL => {
                    let dest = if lp == 0 {
                        next_tab(state, hwnd, state.input.focus, wp != 0)
                    } else {
                        wp
                    };
                    t.set(D_TMP, dest);
                    if dest != 0 {
                        t.set(S_CTRL, dest);
                        return gosub(t, SUB_SET_FOCUS, 330);
                    }
                    goto(330)
                }
                WM_ENTERMENULOOP | WM_LBUTTONDOWN | WM_NCLBUTTONDOWN => {
                    // A combo box hides its list.
                    let focus = state.input.focus;
                    if focus != 0 {
                        t.set(D_TMP, focus);
                        return send(focus, CB_SHOWDROPDOWN, FALSE, 0, 341);
                    }
                    def_window(t, 399)
                }
                WM_GETFONT => Next::Done(i.font),
                WM_CLOSE => {
                    let cancel = dlg_item(state, hwnd, IDCANCEL);
                    super::message::post_message(cpu, &[hwnd, WM_COMMAND, IDCANCEL, cancel]);
                    Next::Done(0)
                }
                WM_INITDIALOG | WM_VKEYTOITEM | WM_COMPAREITEM | WM_CHARTOITEM => goto(398),
                _ => def_window(t, 399),
            }
        }
        // WM_ERASEBKGND: the dialog's brush, or DefWindowProcA's.
        310 => {
            if value != 0 {
                t.set(D_TMP, value);
                return goto(312);
            }
            call_api("DefWindowProcA", &[hwnd, WM_CTLCOLORDLG, wp, hwnd], 311)
        }
        311 => {
            t.set(D_TMP, value);
            goto(312)
        }
        312 => {
            let brush = t.var(D_TMP);
            if let Some(w) = user(cpu).window(hwnd)
                && brush != 0
            {
                let r = Rect::new(0, 0, w.client.width(), w.client.height());
                super::paint::fill(cpu, wp, r, brush);
            }
            Next::Done(TRUE)
        }
        // WM_NEXTDLGCTL: the default button follows.
        330 => {
            t.set(S_NEW, t.var(D_TMP));
            gosub(t, SUB_SET_DEF_BUTTON, 397)
        }
        341 => {
            if value == 0 {
                let parent = parent_of(user(cpu), t.var(D_TMP));
                if parent != 0 {
                    return send(parent, CB_SHOWDROPDOWN, FALSE, 0, 342);
                }
            }
            goto(342)
        }
        342 => def_window(t, 399),
        395 => {
            let found = t.var(S_FOUND);
            if found != 0 {
                let id = ctrl_id(user(cpu), found);
                return Next::Done(make_long(id as i32, DC_HASDEFID as i32));
            }
            Next::Done(0)
        }
        396 => Next::Done(TRUE),
        397 => Next::Done(0),
        // The dialog procedure's result, or DWL_MSGRESULT.
        398 => {
            if returns_result(msg) {
                return Next::Done(t.var(D_RESULT));
            }
            Next::Done(dlg_long(user(cpu), hwnd, DWL_MSGRESULT))
        }
        _ => Next::Done(value),
    }
}

/// The subroutines of DefDlgProcA.
fn sub_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let hwnd = t.var(V_HWND);
    match t.step {
        // DEFDLG_SetFocus(S_CTRL): an edit control's text is selected.
        600 => send(t.var(S_CTRL), WM_GETDLGCODE, 0, 0, 601),
        601 => {
            if value & DLGC_HASSETSEL != 0 {
                return send(t.var(S_CTRL), EM_SETSEL, 0, u32::MAX, 602);
            }
            goto(602)
        }
        602 => call_api("SetFocus", &[t.var(S_CTRL)], 603),
        603 => ret(t),
        // DEFDLG_RestoreFocus: the saved control, or the first with
        // WS_TABSTOP.
        610 => {
            let state = user(cpu);
            if style(state, hwnd) & WS_MINIMIZE != 0 {
                return ret(t);
            }
            let Some(i) = info(state, hwnd) else {
                return ret(t);
            };
            if i.flags & DF_END != 0 {
                return ret(t);
            }
            let mut focus = i.focus;
            if !state.is_window(focus) || focus == hwnd {
                focus = next_tab(state, hwnd, 0, false);
                update_info(state, hwnd, |i| i.focus = focus);
                if !state.is_window(focus) {
                    return ret(t);
                }
            }
            t.set(S_CTRL, focus);
            gosub(t, SUB_SET_FOCUS, 611)
        }
        611 => ret(t),
        // DEFDLG_FindDefButton: S_FOUND.
        620 => {
            let mut list = Vec::new();
            button_candidates(user(cpu), hwnd, &mut list);
            set_list(t, &list);
            t.set(S_I, 0);
            goto(621)
        }
        621 => match list_at(t, t.var(S_I)) {
            Some(c) => send(c, WM_GETDLGCODE, 0, 0, 622),
            None => {
                t.set(S_FOUND, 0);
                ret(t)
            }
        },
        622 => {
            if value & DLGC_DEFPUSHBUTTON != 0 {
                t.set(S_FOUND, t.var(LIST + t.var(S_I) as usize));
                return ret(t);
            }
            t.set(S_I, t.var(S_I) + 1);
            goto(621)
        }
        // DEFDLG_SetDefButton(S_NEW).
        630 => {
            let state = user(cpu);
            let result = info(state, hwnd).unwrap_or_default().result;
            t.set(S_OLD, dlg_item(state, hwnd, result));
            t.set(S_CODE, 0);
            match t.var(S_NEW) {
                0 => goto(633),
                new => send(new, WM_GETDLGCODE, 0, 0, 631),
            }
        }
        631 => {
            t.set(S_CODE, value);
            if value & (DLGC_UNDEFPUSHBUTTON | DLGC_DEFPUSHBUTTON) != 0 {
                return goto(633);
            }
            // Not a push button: the default button keeps its look.
            let old = t.var(S_OLD);
            t.set(S_NEW, old);
            t.set(S_CODE, 0);
            if old != 0 {
                return send(old, WM_GETDLGCODE, 0, 0, 632);
            }
            goto(633)
        }
        632 => {
            t.set(S_CODE, value);
            goto(633)
        }
        // Make sure the old default is a default push button.
        633 => match t.var(S_OLD) {
            0 => goto(635),
            old => send(old, WM_GETDLGCODE, 0, 0, 634),
        },
        634 => {
            if value & DLGC_DEFPUSHBUTTON != 0 {
                return goto(636);
            }
            goto(635)
        }
        635 => gosub(t, SUB_FIND_DEF, 637),
        637 => {
            t.set(S_OLD, t.var(S_FOUND));
            goto(636)
        }
        636 => {
            let (old, new) = (t.var(S_OLD), t.var(S_NEW));
            if old != 0 && old != new {
                return send(old, BM_SETSTYLE, BS_PUSHBUTTON, TRUE, 638);
            }
            goto(638)
        }
        638 => {
            let new = t.var(S_NEW);
            if new != 0 && t.var(S_CODE) & DLGC_UNDEFPUSHBUTTON != 0 {
                return send(new, BM_SETSTYLE, BS_DEFPUSHBUTTON, TRUE, 639);
            }
            goto(639)
        }
        639 => ret(t),
        // DEFDLG_SetDefId(S_ID).
        640 => {
            let id = t.var(S_ID);
            let state = user(cpu);
            let old_id = info(state, hwnd).unwrap_or_default().result;
            let new = dlg_item(state, hwnd, id);
            update_info(state, hwnd, |i| i.result = id);
            t.set(S_NEW, new);
            t.set(S_OLD, dlg_item(state, hwnd, old_id));
            t.set(S_CODE, 0);
            match new {
                0 => goto(633),
                new => send(new, WM_GETDLGCODE, 0, 0, 641),
            }
        }
        641 => {
            t.set(S_CODE, value);
            // Not a push button.
            if value & (DLGC_UNDEFPUSHBUTTON | DLGC_BUTTON) == 0 {
                return ret(t);
            }
            goto(633)
        }
        _ => ret(t),
    }
}

// EndDialog.

fn end_step(cpu: &mut Cpu, t: &mut Task) -> Next {
    let hwnd = t.var(V_HWND);
    match t.step {
        400 => {
            let (was_enabled, owner) = (t.var(2), t.var(3));
            let root = root_of(user(cpu), owner);
            if was_enabled != 0 && root != 0 {
                return call_api("EnableWindow", &[root, TRUE], 401);
            }
            goto(401)
        }
        // Windows gives the dialog itself the focus.
        401 => {
            let state = user(cpu);
            if state.is_descendant(hwnd, state.input.focus) {
                return call_api("SetFocus", &[hwnd], 402);
            }
            goto(402)
        }
        402 => {
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_HIDEWINDOW;
            call_api("SetWindowPos", &[hwnd, 0, 0, 0, 0, 0, flags], 403)
        }
        403 => {
            if user(cpu).input.active == hwnd {
                return call_api("ActivateOtherWindow", &[hwnd], 404);
            }
            goto(404)
        }
        _ => {
            // Wake the dialog's loop.
            super::message::post_message(cpu, &[hwnd, WM_NULL, 0, 0]);
            Next::Done(TRUE)
        }
    }
}

// IsDialogMessageA.

fn message_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let dlg = t.var(V_HWND);
    let (at, mhwnd, wp) = (t.var(I_MSGPTR), t.var(I_MHWND), t.var(I_WP));
    match t.step {
        500 => call_api("CallMsgFilterA", &[at, MSGF_DIALOGBOX], 501),
        501 => {
            if value != 0 {
                return Next::Done(TRUE);
            }
            let state = user(cpu);
            if dlg == DESKTOP || (dlg != mhwnd && !state.is_descendant(dlg, mhwnd)) {
                return Next::Done(FALSE);
            }
            let dlg = message_destination(state, dlg);
            t.set(V_HWND, dlg);
            match t.var(I_MSG) {
                WM_KEYDOWN => send(mhwnd, WM_GETDLGCODE, wp, at, 510),
                WM_CHAR => send(mhwnd, WM_GETDLGCODE, wp, at, 530),
                WM_SYSCHAR => goto(540),
                _ => goto(590),
            }
        }
        // WM_KEYDOWN.
        510 => {
            let code = value;
            if code & DLGC_WANTMESSAGE != 0 {
                return goto(590);
            }
            match wp {
                VK_TAB => {
                    if code & DLGC_WANTTAB != 0 {
                        return goto(590);
                    }
                    let shift = super::input::get_key_state(cpu, &[VK_SHIFT]).unwrap_or(0) & 0x8000;
                    let state = user(cpu);
                    if info(state, dlg).is_some() {
                        return send(dlg, WM_NEXTDLGCTL, shift, 0, 599);
                    }
                    // Not a dialog: the focus moves here.
                    let focus = state.input.focus;
                    let from = if focus == dlg { 0 } else { focus };
                    let next = next_tab(state, dlg, from, shift != 0);
                    if next == 0 {
                        return Next::Done(FALSE);
                    }
                    t.set(I_NEXT, next);
                    send(next, WM_GETDLGCODE, wp, at, 512)
                }
                VK_RIGHT | VK_DOWN | VK_LEFT | VK_UP => {
                    if code & DLGC_WANTARROWS != 0 {
                        return goto(590);
                    }
                    let prev = wp == VK_LEFT || wp == VK_UP;
                    let state = user(cpu);
                    let next = next_group_item(state, dlg, state.input.focus, prev);
                    send(dlg, WM_NEXTDLGCTL, next, 1, 599)
                }
                VK_CANCEL | VK_ESCAPE => {
                    let cancel = dlg_item(user(cpu), dlg, IDCANCEL);
                    send(dlg, WM_COMMAND, IDCANCEL, cancel, 599)
                }
                VK_EXECUTE | VK_RETURN => {
                    if user(cpu).input.focus == mhwnd {
                        return send(mhwnd, WM_GETDLGCODE, 0, 0, 525);
                    }
                    goto(526)
                }
                _ => goto(590),
            }
        }
        // Tab outside a dialog: an edit control's text is selected, the
        // focus moves, the default button follows.
        512 => {
            if value & DLGC_HASSETSEL != 0 {
                return send(t.var(I_NEXT), WM_GETTEXTLENGTH, 0, 0, 513);
            }
            goto(514)
        }
        513 => send(t.var(I_NEXT), EM_SETSEL, 0, value, 514),
        514 => call_api("SetFocus", &[t.var(I_NEXT)], 515),
        515 => send(t.var(I_NEXT), WM_GETDLGCODE, 0, 0, 516),
        // DIALOG_FixChildrenOnChangeFocus: no control is the default
        // push button.
        516 => {
            t.set(I_CODE, value);
            let mut list = Vec::new();
            descendants(user(cpu), dlg, &mut list);
            set_list(t, &list);
            t.set(I_I, 0);
            goto(517)
        }
        517 => match list_at(t, t.var(I_I)) {
            Some(c) => send(c, WM_GETDLGCODE, 0, 0, 518),
            None => goto(519),
        },
        518 => {
            let c = t.var(LIST + t.var(I_I) as usize);
            t.set(I_I, t.var(I_I) + 1);
            if value & DLGC_DEFPUSHBUTTON != 0 {
                return send(c, BM_SETSTYLE, BS_PUSHBUTTON, TRUE, 517);
            }
            goto(517)
        }
        519 => {
            let code = t.var(I_CODE);
            if code & DLGC_DEFPUSHBUTTON != 0 {
                return send(dlg, DM_GETDEFID, 0, 0, 520);
            }
            if code & DLGC_BUTTON != 0 && code & DLGC_UNDEFPUSHBUTTON != 0 {
                return send(t.var(I_NEXT), BM_SETSTYLE, BS_DEFPUSHBUTTON, TRUE, 599);
            }
            Next::Done(TRUE)
        }
        520 => {
            if value >> 16 == DC_HASDEFID {
                let def = dlg_item(user(cpu), dlg, value & 0xFFFF);
                if def != 0 {
                    t.set(I_NEXT, def);
                    return send(def, WM_GETDLGCODE, 0, 0, 521);
                }
            }
            Next::Done(TRUE)
        }
        521 => {
            if value & DLGC_BUTTON != 0 && value & DLGC_UNDEFPUSHBUTTON != 0 {
                return send(t.var(I_NEXT), BM_SETSTYLE, BS_DEFPUSHBUTTON, TRUE, 599);
            }
            Next::Done(TRUE)
        }
        // Return: the focused default button, the dialog's default, or
        // IDOK.
        525 => {
            if value & DLGC_DEFPUSHBUTTON != 0 {
                let id = ctrl_id(user(cpu), mhwnd) & 0xFFFF;
                return send(dlg, WM_COMMAND, id, mhwnd, 599);
            }
            goto(526)
        }
        526 => send(dlg, DM_GETDEFID, 0, 0, 527),
        527 => {
            let state = user(cpu);
            if value >> 16 == DC_HASDEFID {
                let id = value & 0xFFFF;
                let def = id_to_hwnd(state, dlg, id);
                let enabled = if def != 0 {
                    style(state, def) & WS_DISABLED == 0
                } else {
                    id == IDOK
                };
                if enabled {
                    return send(dlg, WM_COMMAND, id, def, 599);
                }
                return Next::Done(TRUE);
            }
            let ok = dlg_item(state, dlg, IDOK);
            send(dlg, WM_COMMAND, IDOK, ok, 599)
        }
        // WM_CHAR.
        530 => {
            if value & (DLGC_WANTCHARS | DLGC_WANTMESSAGE) != 0 {
                return goto(590);
            }
            if wp == b'\t' as u32 && value & DLGC_WANTTAB != 0 {
                return goto(590);
            }
            goto(540)
        }
        // DIALOG_IsAccelerator: a control whose mnemonic is the
        // character.
        540 => {
            let list = accelerator_order(user(cpu), mhwnd, dlg);
            set_list(t, &list);
            t.set(I_I, 0);
            goto(541)
        }
        541 => match list_at(t, t.var(I_I)) {
            Some(c) => send(c, WM_GETDLGCODE, 0, 0, 542),
            None => goto(590),
        },
        542 => {
            let c = t.var(LIST + t.var(I_I) as usize);
            t.set(I_I, t.var(I_I) + 1);
            let state = user(cpu);
            let (text, s) = match state.window(c) {
                Some(w) => (w.text.clone(), w.style),
                None => return goto(541),
            };
            if value & (DLGC_BUTTON | DLGC_STATIC) == 0 || !has_accelerator(&text, wp) {
                return goto(541);
            }
            if value & DLGC_STATIC != 0 || s & 0x0F == BS_GROUPBOX {
                // The focus goes to the control, then on to the next.
                return send(dlg, WM_NEXTDLGCTL, c, 1, 543);
            }
            send(c, BM_CLICK, 0, 0, 599)
        }
        543 => send(dlg, WM_NEXTDLGCTL, 0, 0, 599),
        590 => call_api("TranslateMessage", &[at], 591),
        591 => call_api("DispatchMessageA", &[at], 599),
        _ => Next::Done(TRUE),
    }
}

// The functions on controls that send messages.

fn item_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.step {
        // SetDlgItemInt.
        700 => {
            let (item, number, signed) = (t.var(1), t.var(2), t.var(3));
            let text = if signed != 0 {
                (number as i32).to_string()
            } else {
                number.to_string()
            };
            let at = put(cpu, t.scratch, 32, text.as_bytes());
            send(item, WM_SETTEXT, 0, at, 701)
        }
        // GetDlgItemInt.
        710 => send(t.var(1), WM_GETTEXT, 30, t.scratch, 711),
        711 => {
            if value == 0 {
                return Next::Done(0);
            }
            let text = bytes_at(cpu, t.scratch).unwrap_or_default();
            let Some(n) = parse_int(&text, t.var(3) != 0) else {
                return Next::Done(0);
            };
            if t.var(2) != 0 {
                cpu.bus.guest_write_32(t.var(2), TRUE);
            }
            Next::Done(n)
        }
        // A message, then a value of the function's own.
        720 => send(t.var(1), t.var(2), t.var(3), t.var(4), 721),
        721 => Next::Done(t.var(5)),
        // CheckRadioButton.
        730 | 731 => {
            if t.step == 731 {
                t.set(C_I, t.var(C_I) + 1);
            }
            let Some(c) = list_at(t, t.var(C_I)) else {
                return Next::Done(TRUE);
            };
            let check = (ctrl_id(user(cpu), c) == t.var(2)) as u32;
            send(c, BM_SETCHECK, check, 0, 731)
        }
        _ => Next::Done(value),
    }
}

/// strtol or strtoul of the text, as GetDlgItemInt takes it: None if
/// there is no number or it doesn't fit.
fn parse_int(text: &[u8], signed: bool) -> Option<u32> {
    let mut i = 0;
    while i < text.len() && matches!(text[i], b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r') {
        i += 1;
    }
    let mut negative = false;
    if i < text.len() && (text[i] == b'+' || text[i] == b'-') {
        negative = text[i] == b'-';
        i += 1;
    }
    let start = i;
    let mut n: u64 = 0;
    let mut over = false;
    while i < text.len() && text[i].is_ascii_digit() {
        n = n * 10 + (text[i] - b'0') as u64;
        if n > u32::MAX as u64 + 1 {
            over = true;
            n = u32::MAX as u64 + 1;
        }
        i += 1;
    }
    if i == start {
        return None;
    }
    if signed {
        let v = if negative { -(n as i64) } else { n as i64 };
        if over || v > i32::MAX as i64 || v < i32::MIN as i64 {
            return None;
        }
        Some(v as i32 as u32)
    } else {
        if over || n > u32::MAX as u64 {
            return None;
        }
        let v = n as u32;
        Some(if negative { v.wrapping_neg() } else { v })
    }
}

// The functions.

/// CreateDialogParamA(instance, name, owner, proc, param).
pub fn create_dialog_param(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[inst, name, owner, proc, param] = args else {
        unreachable!()
    };
    let template = match kernel32::resource_data(cpu, inst, RT_DIALOG, name)? {
        Ok((at, _)) => at,
        Err(code) => {
            set_error(cpu, code);
            return Some(0);
        }
    };
    create(
        cpu,
        Create {
            inst,
            template,
            owner,
            proc,
            param,
            ..Create::default()
        },
    )
}

/// CreateDialogIndirectParamA(instance, template, owner, proc, param).
pub fn create_dialog_indirect_param(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[inst, template, owner, proc, param] = args else {
        unreachable!()
    };
    if template == 0 {
        return Some(0);
    }
    create(
        cpu,
        Create {
            inst,
            template,
            owner,
            proc,
            param,
            ..Create::default()
        },
    )
}

/// DialogBoxParamA(instance, name, owner, proc, param): -1 when the
/// template isn't there, 0 when the dialog can't be made.
pub fn dialog_box_param(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[inst, name, owner, proc, param] = args else {
        unreachable!()
    };
    let template = match kernel32::resource_data(cpu, inst, RT_DIALOG, name)? {
        Ok((at, _)) => at,
        Err(code) => {
            set_error(cpu, code);
            return Some(u32::MAX);
        }
    };
    create(
        cpu,
        Create {
            inst,
            template,
            owner,
            proc,
            param,
            mode: M_MODAL,
            ..Create::default()
        },
    )
}

/// DialogBoxIndirectParamA(instance, template, owner, proc, param): -1
/// when the dialog can't be made.
pub fn dialog_box_indirect_param(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[inst, template, owner, proc, param] = args else {
        unreachable!()
    };
    if template == 0 {
        return Some(u32::MAX);
    }
    create(
        cpu,
        Create {
            inst,
            template,
            owner,
            proc,
            param,
            mode: M_MODAL,
            fail: u32::MAX,
            ..Create::default()
        },
    )
}

/// EndDialog(hwnd, result): the dialog hides and its loop ends.
pub fn end_dialog(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, result) = (args[0], args[1]);
    let state = user(cpu);
    if !state.is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    }
    let Some(mut i) = info(state, hwnd) else {
        return Some(FALSE);
    };
    i.result = result;
    i.flags |= DF_END;
    set_info(state, hwnd, &i);
    let was_enabled = (i.flags & DF_OWNERENABLED != 0) as u32;
    let owner = state.window(hwnd).map_or(0, |w| w.owner);
    task::start(cpu, TASK_DIALOG, vec![K_END, hwnd, was_enabled, owner], 0)
}

/// DefDlgProcA(hwnd, msg, wParam, lParam).
pub fn def_dlg_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, msg) = (args[0], args[1]);
    let state = user(cpu);
    if hwnd == DESKTOP || !state.is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(0);
    }
    let Some(i) = make_info(state, hwnd) else {
        // No room for a dialog's state.
        return super::defwnd::def_window_proc(cpu, args);
    };
    set_dlg_long(state, hwnd, DWL_MSGRESULT, 0);
    let proc = dlg_long(state, hwnd, DWL_DLGPROC);
    let message_box = i.flags & DF_MSGBOX != 0 && msg == WM_COMMAND;
    if proc == 0 && !message_box && !handled(msg) && !returns_result(msg) {
        return super::defwnd::def_window_proc(cpu, args);
    }
    let vars = vec![K_DEFDLG, hwnd, msg, args[2], args[3]];
    task::start(cpu, TASK_DIALOG, vars, 0)
}

/// IsDialogMessageA(hwnd, msg).
pub fn is_dialog_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (dlg, at) = (args[0], args[1]);
    let w = read_words(cpu, at, 3);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let vars = vec![K_ISDLG, dlg, at, w[0], w[1], w[2]];
    task::start(cpu, TASK_DIALOG, vars, 0)
}

/// GetDlgItem(hwnd, id).
pub fn get_dlg_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(dlg_item(user(cpu), args[0], args[1]))
}

/// GetDlgCtrlID(hwnd): GetWindowLongA's GWL_ID.
pub fn get_dlg_ctrl_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    window::get_window_long(cpu, &[args[0], window::GWL_ID as u32])
}

/// SetDlgItemTextA(hwnd, id, text): WM_SETTEXT.
pub fn set_dlg_item_text(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match dlg_item(user(cpu), args[0], args[1]) {
        0 => Some(0),
        item => super::message::send_message(cpu, &[item, WM_SETTEXT, 0, args[2]]),
    }
}

/// GetDlgItemTextA(hwnd, id, buffer, size): WM_GETTEXT.
pub fn get_dlg_item_text(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (dlg, id, buffer, size) = (args[0], args[1], args[2], args[3]);
    if buffer != 0 && (size as i32) > 0 {
        cpu.bus.guest_write_8(buffer, 0);
    }
    match dlg_item(user(cpu), dlg, id) {
        0 => Some(0),
        item => super::message::send_message(cpu, &[item, WM_GETTEXT, size, buffer]),
    }
}

/// SetDlgItemInt(hwnd, id, value, signed): TRUE.
pub fn set_dlg_item_int(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (dlg, id, number, signed) = (args[0], args[1], args[2], args[3]);
    match dlg_item(user(cpu), dlg, id) {
        0 => Some(TRUE),
        item => task::start(cpu, TASK_DIALOG, vec![K_SETINT, item, number, signed], 32),
    }
}

/// GetDlgItemInt(hwnd, id, translated, signed).
pub fn get_dlg_item_int(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (dlg, id, translated, signed) = (args[0], args[1], args[2], args[3]);
    if translated != 0 {
        cpu.bus.guest_write_32(translated, FALSE);
    }
    match dlg_item(user(cpu), dlg, id) {
        0 => Some(0),
        item => task::start(
            cpu,
            TASK_DIALOG,
            vec![K_GETINT, item, translated, signed],
            32,
        ),
    }
}

/// SendDlgItemMessageA(hwnd, id, msg, wParam, lParam).
pub fn send_dlg_item_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match dlg_item(user(cpu), args[0], args[1]) {
        0 => Some(0),
        item => super::message::send_message(cpu, &[item, args[2], args[3], args[4]]),
    }
}

/// CheckDlgButton(hwnd, id, check): BM_SETCHECK, and TRUE.
pub fn check_dlg_button(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match dlg_item(user(cpu), args[0], args[1]) {
        0 => Some(TRUE),
        item => {
            let vars = vec![K_SEND, item, BM_SETCHECK, args[2], 0, TRUE];
            task::start(cpu, TASK_DIALOG, vars, 0)
        }
    }
}

/// IsDlgButtonChecked(hwnd, id): BM_GETCHECK.
pub fn is_dlg_button_checked(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match dlg_item(user(cpu), args[0], args[1]) {
        0 => Some(0),
        item => super::message::send_message(cpu, &[item, BM_GETCHECK, 0, 0]),
    }
}

/// CheckRadioButton(hwnd, first, last, check): the dialog's descendants
/// with ids from `first` to `last` are checked or not.
pub fn check_radio_button(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (dlg, first, last, check) = (args[0], args[1], args[2], args[3]);
    let state = user(cpu);
    if !state.is_window(dlg) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    }
    let mut all = Vec::new();
    descendants(state, dlg, &mut all);
    if all.is_empty() {
        return Some(FALSE);
    }
    let list: Vec<u32> = all
        .into_iter()
        .filter(|&c| (first..=last).contains(&ctrl_id(state, c)))
        .collect();
    let vars = vars_with_list(vec![K_RADIO, dlg, check], &list);
    task::start(cpu, TASK_DIALOG, vars, 0)
}

/// GetNextDlgTabItem(hwnd, control, previous).
pub fn get_next_dlg_tab_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(next_tab(user(cpu), args[0], args[1], args[2] != 0))
}

/// GetNextDlgGroupItem(hwnd, control, previous).
pub fn get_next_dlg_group_item(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(next_group_item(user(cpu), args[0], args[1], args[2] != 0))
}

/// MapDialogRect(hwnd, rect): dialog units to pixels.
pub fn map_dialog_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, at) = (args[0], args[1]);
    let state = user(cpu);
    if !state.is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    }
    let Some(i) = info(state, hwnd) else {
        return Some(FALSE);
    };
    let (xb, yb) = (i.xb as i32, i.yb as i32);
    let r = read_words(cpu, at, 4);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let mapped = [
        mul_div(r[0] as i32, xb, 4) as u32,
        mul_div(r[1] as i32, yb, 8) as u32,
        mul_div(r[2] as i32, xb, 4) as u32,
        mul_div(r[3] as i32, yb, 8) as u32,
    ];
    write_words(cpu, at, &mapped);
    Some(TRUE)
}

/// GetDialogBaseUnits: the system font's average width and height.
pub fn get_dialog_base_units(cpu: &mut Cpu, _args: &[u32]) -> Option<u32> {
    let (x, y) = base_units(cpu);
    Some(make_long(x, y))
}

// Message boxes.

/// MessageBoxA(owner, text, caption, style), and MessageBoxExA with its
/// language.
pub fn message_box(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = box_text(cpu, 0, args[1])?;
    let caption = box_caption(cpu, 0, args[2])?;
    message_box_with(cpu, args[0], 0, text, caption, args[3], 0)
}

/// MessageBoxW and MessageBoxExW: their strings in the ANSI code page.
pub fn message_box_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = match args[1] {
        0 => Vec::new(),
        at => ansi(&wide_string(cpu, at).0),
    };
    let caption = match args[2] {
        0 => b"Error".to_vec(),
        at => ansi(&wide_string(cpu, at).0),
    };
    if cpu.bus.guest_faulted() {
        return None;
    }
    message_box_with(cpu, args[0], 0, text, caption, args[3], 0)
}

/// MessageBoxIndirectA(params).
pub fn message_box_indirect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let p = read_words(cpu, args[0], 7);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (owner, inst, style, icon) = (p[1], p[2], p[5], p[6]);
    let text = box_text(cpu, inst, p[3])?;
    let caption = box_caption(cpu, inst, p[4])?;
    message_box_with(cpu, owner, inst, text, caption, style, icon)
}

/// A string of the module's string table, in the ANSI code page.
fn load_string(cpu: &mut Cpu, inst: u32, id: u32) -> Option<Vec<u8>> {
    let units = super::resource::string_units(cpu, inst, id)?.unwrap_or_default();
    Some(ansi(&units))
}

/// A message box's text: a string, or a string table's number.
fn box_text(cpu: &mut Cpu, inst: u32, at: u32) -> Option<Vec<u8>> {
    match at {
        0 => Some(Vec::new()),
        id if id < 0x1_0000 => load_string(cpu, inst, id),
        at => bytes_at(cpu, at),
    }
}

/// A message box's caption: "Error" when there is none.
fn box_caption(cpu: &mut Cpu, inst: u32, at: u32) -> Option<Vec<u8>> {
    let caption = match at {
        0 => Vec::new(),
        id if id < 0x1_0000 => load_string(cpu, inst, id)?,
        at => bytes_at(cpu, at)?,
    };
    if caption.is_empty() && at < 0x1_0000 {
        return Some(b"Error".to_vec());
    }
    Some(caption)
}

/// The buttons of the message box style, in the order of their ids, which
/// is the order of the tab stops.
fn box_buttons(style: u32) -> Vec<u32> {
    let mut ids = match style & MB_TYPEMASK {
        MB_OKCANCEL => vec![IDOK, IDCANCEL],
        MB_ABORTRETRYIGNORE => vec![IDABORT, IDRETRY, IDIGNORE],
        MB_YESNOCANCEL => vec![IDCANCEL, IDYES, IDNO],
        MB_YESNO => vec![IDYES, IDNO],
        MB_RETRYCANCEL => vec![IDCANCEL, IDRETRY],
        MB_CANCELTRYCONTINUE => vec![IDCANCEL, IDTRYAGAIN, IDCONTINUE],
        _ => vec![IDOK],
    };
    if style & MB_HELP != 0 {
        ids.push(IDHELP);
    }
    ids
}

fn button_label(id: u32) -> &'static str {
    match id {
        IDOK => "OK",
        IDCANCEL => "Cancel",
        IDABORT => "&Abort",
        IDRETRY => "&Retry",
        IDIGNORE => "&Ignore",
        IDYES => "&Yes",
        IDNO => "&No",
        IDTRYAGAIN => "&Try Again",
        IDCONTINUE => "&Continue",
        _ => "Help",
    }
}

/// The order of the buttons from left to right, by id.
const BUTTON_ORDER: [u32; 10] = [6, 7, 1, 3, 4, 2, 5, 10, 11, 9];

/// A message box's dialog style.
const BOX_STYLE: u32 =
    DS_MODALFRAME | DS_NOIDLEMSG | DS_CENTER | WS_POPUP | WS_VISIBLE | WS_CAPTION | WS_SYSMENU;

/// Template bytes.
struct Bytes(Vec<u8>);

impl Bytes {
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn string(&mut self, units: &[u16]) {
        for &c in units {
            self.u16(c);
        }
        self.u16(0);
    }

    fn align(&mut self) {
        while !self.0.len().is_multiple_of(4) {
            self.0.push(0);
        }
    }

    /// A control of a DLGTEMPLATE, in pixels: its class is one of the
    /// numbered ones.
    fn control(&mut self, style: u32, r: Rect, id: u32, class: u16, title: &[u16]) {
        self.align();
        self.u32(style);
        self.u32(0);
        for v in [r.left, r.top, r.width(), r.height()] {
            self.u16(v as i16 as u16);
        }
        self.u16(id as u16);
        self.u16(0xFFFF);
        self.u16(class);
        self.string(title);
        self.u16(0);
    }
}

/// MSGBOX_OnInit's layout, as a template in pixels: the icon, the text
/// and the buttons, the window sized to them. Also the default button's
/// id.
fn box_template(
    cpu: &mut Cpu,
    font: u32,
    text: &[u8],
    caption: &[u8],
    style: u32,
    icon: bool,
) -> (Vec<u8>, u32) {
    let (xb, _) = base_units(cpu);
    let hdc = gdi32::dc::new_dc(cpu, gdi32::dc::MEMORY);
    gdi32::dc::with(cpu, hdc, |dc| dc.attrs.font = font);
    let measure = |cpu: &mut Cpu, s: &[u8], mut r: Rect, flags: u32| {
        let flags = flags | super::text::DT_EXPANDTABS | super::text::DT_CALCRECT;
        super::text::draw_text_bytes(cpu, hdc, s, &mut r, flags);
        r
    };
    // The buttons are as big as the biggest label needs.
    let buttons = box_buttons(style);
    let (mut bw, mut bh) = (1, 1);
    for &id in &buttons {
        let r = measure(cpu, button_label(id).as_bytes(), Rect::default(), 0);
        bw = bw.max(r.width());
        bh = bh.max(r.height());
    }
    bw = bw.max(bh * 2);
    bh *= 2;
    bw *= 2;
    let bspace = bw / 3;
    let n = buttons.len() as i32;
    let m = super::sysparams::metric;
    let ileft = mul_div(8, xb, 4);
    let (iwidth, iheight) = if icon { (m(11), m(12)) } else { (0, 0) };
    let wrap = Rect::new(0, 0, mul_div(176, xb, 4), 0);
    let tr = measure(cpu, text, wrap, super::text::DT_WORDBREAK);
    gdi32::dc::delete_dc(cpu, &[hdc]);
    let mut tleft = ileft;
    if iwidth != 0 {
        tleft += ileft + iwidth;
    }
    let twidth = ((bw + bspace) * n + bspace - tleft).max(tr.right);
    let theight = tr.bottom;
    let tiheight = 16 + iheight.max(theight);
    let (cw, ch) = (tleft + twidth + ileft, 8 + tiheight + bh);
    let ex = if style & (MB_SYSTEMMODAL | MB_TOPMOST) != 0 {
        WS_EX_TOPMOST
    } else {
        0
    };
    let wwidth = window::adjust(
        Rect::new(0, 0, cw, ch),
        BOX_STYLE,
        WS_EX_DLGMODALFRAME,
        false,
    )
    .width();

    let mut b = Bytes(Vec::new());
    b.u32(BOX_STYLE);
    b.u32(ex);
    b.u16(n as u16 + 1 + icon as u16);
    for v in [0, 0, cw, ch] {
        b.u16(v as i16 as u16);
    }
    b.u16(0);
    b.u16(0);
    b.string(&wide(caption));
    if icon {
        let r = Rect::sized(ileft, (tiheight - iheight) / 2, iwidth, iheight);
        b.control(WS_CHILD | WS_VISIBLE | SS_ICON, r, MSGBOX_IDICON, 0x82, &[]);
    }
    let r = Rect::sized(tleft, (tiheight - theight) / 2, twidth, theight);
    let text_style = WS_CHILD | WS_VISIBLE | WS_GROUP | SS_NOPREFIX;
    b.control(text_style, r, MSGBOX_IDTEXT, 0x82, &wide(text));
    // Left to right in BUTTON_ORDER; the default is the one MB_DEFMASK
    // counts to.
    let default = ((style & MB_DEFMASK) >> 8) as usize;
    let ordered: Vec<u32> = BUTTON_ORDER
        .iter()
        .copied()
        .filter(|id| buttons.contains(id))
        .collect();
    let default_id = ordered.get(default).copied().unwrap_or(0);
    let mut bpos = (wwidth - (bw + bspace) * n + bspace) / 2;
    let mut places = Vec::new();
    for &id in &ordered {
        places.push((id, bpos));
        bpos += bw + bspace;
    }
    for &id in &buttons {
        let x = places.iter().find(|p| p.0 == id).map_or(0, |p| p.1);
        let kind = if id == default_id {
            BS_DEFPUSHBUTTON
        } else {
            BS_PUSHBUTTON
        };
        let style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | kind;
        let label: Vec<u16> = button_label(id).encode_utf16().collect();
        b.control(style, Rect::sized(x, tiheight, bw, bh), id, 0x80, &label);
    }
    b.align();
    (b.0, default_id)
}

/// MessageBoxIndirectA's work: the box's template is made, and the box
/// runs as a modal dialog.
fn message_box_with(
    cpu: &mut Cpu,
    owner: u32,
    inst: u32,
    text: Vec<u8>,
    caption: Vec<u8>,
    style: u32,
    icon_name: u32,
) -> Option<u32> {
    super::init(cpu);
    let system_icon = match style & MB_ICONMASK {
        MB_ICONHAND => 32513,
        MB_ICONQUESTION => 32514,
        MB_ICONEXCLAMATION => 32515,
        MB_ICONASTERISK => 32516,
        _ => 0,
    };
    let icon = if system_icon != 0 {
        super::resource::load_icon(cpu, &[0, system_icon])?
    } else if style & MB_ICONMASK == MB_USERICON {
        super::resource::load_icon(cpu, &[inst, icon_name])?
    } else {
        0
    };
    let log = super::sysparams::sans_serif(false);
    let font = gdi32::add(cpu, Kind::Font { font: Font { log } });
    let (template, default_id) = box_template(cpu, font, &text, &caption, style, icon != 0);
    let Some(pages) = kernel32::alloc_pages(cpu, template.len() as u32) else {
        gdi32::delete_object(cpu, &[font]);
        return Some(0);
    };
    cpu.bus.guest_write_bytes(pages, &template);
    // A task-modal box without an owner disables the thread's windows.
    let mut disabled = Vec::new();
    if style & MB_TASKMODAL != 0 && owner == 0 {
        let tid = current_tid(cpu);
        let state = user(cpu);
        disabled = state
            .top_levels()
            .into_iter()
            .filter(|&h| {
                state
                    .window(h)
                    .is_some_and(|w| w.tid == tid && w.style & WS_DISABLED == 0)
            })
            .collect();
    }
    let mut mode = M_MODAL | M_MSGBOX;
    if !disabled.is_empty() {
        mode |= M_TASKMODAL;
    }
    create(
        cpu,
        Create {
            inst,
            template: pages,
            owner,
            mode,
            font,
            icon,
            mb_style: style,
            mb_default: default_id,
            pages,
            disabled,
            ..Create::default()
        },
    )
}
