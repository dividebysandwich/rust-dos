//! Property sheets and wizards: PropertySheetA, CreatePropertySheetPageA
//! and DestroyPropertySheetPage.
//!
//! A property sheet is a dialog COMCTL32 makes from a template of its
//! own (OK, Cancel, Apply, Help and a tab control; a wizard has Back,
//! Next, Finish, Cancel and Help), with a procedure of its own,
//! `PropSheetDlgProc`. Its pages are dialogs too, children of the
//! sheet, made from the program's templates (resources, or in memory
//! with PSP_DLGINDIRECT) the first time they are shown, with the
//! program's dialog procedures. The sheet talks to the pages with
//! WM_NOTIFY (PSN_SETACTIVE, PSN_KILLACTIVE, PSN_APPLY, PSN_RESET,
//! PSN_WIZBACK, PSN_WIZNEXT, PSN_WIZFINISH, PSN_QUERYCANCEL), and the
//! pages and the program talk to the sheet with PSM_ messages. A modal
//! sheet runs its own message loop, its owner disabled; a modeless one
//! (PSH_MODELESS) is returned at once.
//!
//! Everything here that calls into the program (the pages' procedures,
//! the callbacks, SetWindowPos, SendMessage) is a script (script.rs),
//! and the sheet's work is continuations, which find what they need in
//! `State` by the sheet's number.
//!
//! Ported from Wine's propsheet.c:
//!
//! Copyright 1998 Francis Beaudet
//! Copyright 1999 Thuy Nguyen
//! Copyright 2004 Maxime Bellenge
//! Copyright 2004 Filip Navara
//!
//! This library is free software; you can redistribute it and/or modify
//! it under the terms of the GNU Lesser General Public License as
//! published by the Free Software Foundation; either version 2.1 of the
//! License, or (at your option) any later version.

use super::misc::{heap_alloc, heap_free, process_heap};
use super::script::{self, Arg, Ctx, Op, Script};
use super::{PROPSHEET, kernel32, state, tab};
use crate::cpu::Cpu;
use crate::win32::convert::CodePage;
use crate::win32::gdi32::Rect;
use crate::win32::user32::message::{
    WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_GETDLGCODE, WM_INITDIALOG, WM_KEYDOWN, WM_NOTIFY,
};
use crate::win32::user32::window::{
    HWND_TOP, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_CAPTION,
    WS_CHILD, WS_DISABLED, WS_EX_CONTROLPARENT, WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_THICKFRAME,
    WS_VISIBLE,
};
use crate::win32::user32::{self, dialog, read_words, window};
use std::collections::BTreeMap;

// PROPSHEETHEADERA's flags.
const PSH_PROPTITLE: u32 = 0x0001;
const PSH_PROPSHEETPAGE: u32 = 0x0008;
const PSH_WIZARDHASFINISH: u32 = 0x0010;
const PSH_WIZARD: u32 = 0x0020;
const PSH_USEPSTARTPAGE: u32 = 0x0040;
const PSH_NOAPPLYNOW: u32 = 0x0080;
const PSH_USECALLBACK: u32 = 0x0100;
const PSH_HASHELP: u32 = 0x0200;
const PSH_MODELESS: u32 = 0x0400;
const PSH_WIZARDCONTEXTHELP: u32 = 0x1000;
const PSH_WIZARD97_OLD: u32 = 0x2000;
const PSH_WIZARD_LITE: u32 = 0x0040_0000;
const PSH_WIZARD97_NEW: u32 = 0x0100_0000;
const PSH_NOCONTEXTHELP: u32 = 0x0200_0000;
const ANY_WIZARD: u32 = PSH_WIZARD | PSH_WIZARD97_OLD | PSH_WIZARD97_NEW | PSH_WIZARD_LITE;

/// PROPSHEETHEADERA's size in COMCTL32 4.70; Windows 98's 4.72 takes
/// the 52 bytes with the wizard bitmaps too, which this accepts.
const PROPSHEETHEADERA_V1_SIZE: u32 = 40;
/// PROPSHEETPAGEA's size in COMCTL32 4.70.
const PROPSHEETPAGEA_V1_SIZE: u32 = 40;

// PROPSHEETPAGEA's flags.
const PSP_DLGINDIRECT: u32 = 0x0001;
const PSP_USETITLE: u32 = 0x0008;
const PSP_HASHELP: u32 = 0x0020;
const PSP_USEREFPARENT: u32 = 0x0040;
const PSP_USECALLBACK: u32 = 0x0080;
const PSP_PREMATURE: u32 = 0x0400;

const PSCB_INITIALIZED: u32 = 1;
const PSCB_PRECREATE: u32 = 2;
const PSPCB_RELEASE: u32 = 1;
const PSPCB_CREATE: u32 = 2;

const PSN_SETACTIVE: u32 = (-200i32) as u32;
const PSN_KILLACTIVE: u32 = (-201i32) as u32;
const PSN_APPLY: u32 = (-202i32) as u32;
const PSN_RESET: u32 = (-203i32) as u32;
const PSN_HELP: u32 = (-205i32) as u32;
const PSN_WIZBACK: u32 = (-206i32) as u32;
const PSN_WIZNEXT: u32 = (-207i32) as u32;
const PSN_WIZFINISH: u32 = (-208i32) as u32;
const PSN_QUERYCANCEL: u32 = (-209i32) as u32;
const PSNRET_INVALID: u32 = 1;
const PSNRET_INVALID_NOCHANGEPAGE: u32 = 2;

const PSM_SETCURSEL: u32 = 0x0465;
const PSM_REMOVEPAGE: u32 = 0x0466;
const PSM_ADDPAGE: u32 = 0x0467;
const PSM_CHANGED: u32 = 0x0468;
const PSM_RESTARTWINDOWS: u32 = 0x0469;
const PSM_REBOOTSYSTEM: u32 = 0x046A;
const PSM_CANCELTOCLOSE: u32 = 0x046B;
const PSM_QUERYSIBLINGS: u32 = 0x046C;
const PSM_UNCHANGED: u32 = 0x046D;
const PSM_APPLY: u32 = 0x046E;
const PSM_SETTITLEA: u32 = 0x046F;
const PSM_SETWIZBUTTONS: u32 = 0x0470;
const PSM_PRESSBUTTON: u32 = 0x0471;
const PSM_SETCURSELID: u32 = 0x0472;
const PSM_SETFINISHTEXTA: u32 = 0x0473;
const PSM_GETTABCONTROL: u32 = 0x0474;
const PSM_ISDIALOGMESSAGE: u32 = 0x0475;
const PSM_GETCURRENTPAGEHWND: u32 = 0x0476;
const PSM_INSERTPAGE: u32 = 0x0477;
const PSM_SETHEADERTITLEA: u32 = 0x047D;
const PSM_SETHEADERSUBTITLEA: u32 = 0x047F;
const PSM_HWNDTOINDEX: u32 = 0x0481;
const PSM_INDEXTOHWND: u32 = 0x0482;
const PSM_PAGETOINDEX: u32 = 0x0483;
const PSM_INDEXTOPAGE: u32 = 0x0484;
const PSM_IDTOINDEX: u32 = 0x0485;
const PSM_INDEXTOID: u32 = 0x0486;
const PSM_GETRESULT: u32 = 0x0487;
const PSM_RECALCPAGESIZES: u32 = 0x0488;

const PSBTN_BACK: u32 = 0;
const PSBTN_NEXT: u32 = 1;
const PSBTN_FINISH: u32 = 2;
const PSBTN_OK: u32 = 3;
const PSBTN_APPLYNOW: u32 = 4;
const PSBTN_CANCEL: u32 = 5;
const PSBTN_HELP: u32 = 6;

const PSWIZB_BACK: u32 = 1;
const PSWIZB_NEXT: u32 = 2;
const PSWIZB_FINISH: u32 = 4;
const PSWIZB_DISABLEDFINISH: u32 = 8;

const ID_PSRESTARTWINDOWS: i32 = 2;
const ID_PSREBOOTSYSTEM: i32 = 3;

// The sheet's controls.
const IDOK: u32 = 1;
const IDCANCEL: u32 = 2;
const IDHELP: u32 = 9;
const IDC_TABCONTROL: u32 = 12320;
const IDC_APPLY_BUTTON: u32 = 12321;
const IDC_BACK_BUTTON: u32 = 12323;
const IDC_NEXT_BUTTON: u32 = 12324;
const IDC_FINISH_BUTTON: u32 = 12325;
const IDC_SUNKEN_LINE: u32 = 12326;
const IDC_SUNKEN_LINEHEADER: u32 = 12327;

const DM_SETDEFID: u32 = 0x0401;
const DWL_MSGRESULT: u32 = 0;
const DLGC_WANTMESSAGE: u32 = 0x0004;
const TCM_INSERTITEMA: u32 = 0x1307;
const TCM_DELETEITEM: u32 = 0x1308;
const TCM_SETCURSEL: u32 = 0x130C;
const TCIF_TEXT: u32 = 0x0001;
const TCN_SELCHANGE: u32 = tab::TCN_SELCHANGE;
const TCN_SELCHANGING: u32 = tab::TCN_SELCHANGING;
const VK_TAB: u32 = 0x09;
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_PRIOR: u32 = 0x21;
const VK_NEXT: u32 = 0x22;
const RT_DIALOG: u32 = 5;

const DS_SETFONT: u32 = 0x0040;
const DS_MODALFRAME: u32 = 0x0080;
const DS_CONTROL: u32 = 0x0400;
const DS_CONTEXTHELP: u32 = 0x2000;

/// Wizard pages are this far, in dialog units, from the sheet's edges.
const WIZARD_PADDING: i32 = 7;
/// The longest caption and tab text.
const MAX_TEXT: usize = 255;
/// How many pages PSN_SETACTIVE may skip to before the sheet stops.
const MAX_SKIPS: u32 = 64;

/// The caption of a sheet with PSH_PROPTITLE.
const PROPERTIES_FOR: &[u8] = b"Properties for %s";

/// The scratch space of the sheet's scripts: a PSHNOTIFY, a MSG, a
/// TCITEMA and a text.
const P_PSN: u32 = 0;
const P_MSG: u32 = 16;
const P_ITEM: u32 = 44;
const P_TEXT: u32 = 72;
const P_SIZE: u32 = 336;

// The continuations.
const T_CREATED: u16 = 1;
const T_LOOP: u16 = 2;
const T_GOT: u16 = 3;
const T_DISPATCHED: u16 = 4;
const T_DONE: u16 = 5;
const T_INIT_TABS: u16 = 6;
const T_INIT_LAYOUT: u16 = 7;
const T_INIT_END: u16 = 8;
const T_SC_BEGIN: u16 = 9;
const T_SC_PLACE: u16 = 10;
const T_SC_RESULT: u16 = 11;
const T_SHOW: u16 = 12;
const T_PAGE_CREATED: u16 = 13;
const T_FREE_PAGE: u16 = 14;
const T_BACK: u16 = 15;
const T_BACK_R: u16 = 16;
const T_BACK_K: u16 = 17;
const T_NEXT: u16 = 18;
const T_NEXT_R: u16 = 19;
const T_NEXT_K: u16 = 20;
const T_FINISH: u16 = 21;
const T_FINISH_R: u16 = 22;
const T_APPLY: u16 = 23;
const T_APPLY_K: u16 = 24;
const T_APPLY_I: u16 = 25;
const T_APPLY_TAIL: u16 = 26;
const T_CANCEL: u16 = 27;
const T_CANCEL_Q: u16 = 28;
const T_SELCHANGING: u16 = 29;
const T_PSM_SETCUR: u16 = 30;
const T_SETCURSELID: u16 = 31;
const T_QUERY: u16 = 32;
const T_ADD_TAB: u16 = 33;
const T_REMOVE: u16 = 34;
const T_ISDLG: u16 = 35;
const T_ISDLG_K: u16 = 36;
const T_RESULT_LAST: u16 = 37;
const T_HELP: u16 = 38;

/// A page of a sheet.
#[derive(Clone, Debug, Default)]
pub struct Page {
    /// Its HPROPSHEETPAGE: COMCTL32's copy of the PROPSHEETPAGEA.
    pub hpage: u32,
    /// Its dialog, once made.
    pub hwnd: u32,
    pub dirty: bool,
    /// Its tab's text: the template's caption, or PSP_USETITLE's.
    pub text: Vec<u8>,
    pub has_help: bool,
}

crate::state_fields!(Page {
    hpage,
    hwnd,
    dirty,
    text,
    has_help
});

/// A property sheet.
#[derive(Clone, Debug, Default)]
pub struct Sheet {
    pub hwnd: u32,
    pub flags: u32,
    pub parent: u32,
    pub instance: u32,
    /// The caption: its text, or a string's number in `instance`.
    pub caption: Option<Vec<u8>>,
    pub caption_id: u32,
    pub callback: u32,
    pub pages: Vec<Page>,
    pub active: i32,
    pub modeless: bool,
    pub has_help: bool,
    pub has_apply: bool,
    pub has_finish: bool,
    pub active_valid: bool,
    pub ended: bool,
    pub result: i32,
    /// The largest page, in dialog units.
    pub width: i32,
    pub height: i32,
    /// The template the dialog is made from, until it is made.
    pub template: u32,
    /// The dialog was destroyed, while PropertySheetA still runs its
    /// loop.
    pub destroyed: bool,
}

crate::state_fields!(Sheet {
    hwnd,
    flags,
    parent,
    instance,
    caption,
    caption_id,
    callback,
    pages,
    active,
    modeless,
    has_help,
    has_apply,
    has_finish,
    active_valid,
    ended,
    result,
    width,
    height,
    template,
    destroyed
});

/// What the module keeps, in COMCTL32's `State`: the sheets by their
/// numbers, and the numbers by the sheets' windows.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub sheets: BTreeMap<u32, Sheet>,
    pub windows: BTreeMap<u32, u32>,
    pub next: u32,
}

crate::state_fields!(State {
    sheets,
    windows,
    next
});

fn get(cpu: &mut Cpu, id: u32) -> Option<Sheet> {
    state(cpu).propsheet.sheets.get(&id).cloned()
}

fn put(cpu: &mut Cpu, id: u32, s: Sheet) {
    state(cpu).propsheet.sheets.insert(id, s);
}

fn wizard(s: &Sheet) -> bool {
    s.flags & ANY_WIZARD != 0
}

// Ops.

fn call(target: u32, args: &[u32]) -> Op {
    Op::Call {
        target,
        args: args.iter().map(|&a| Arg::V(a)).collect(),
    }
}

fn user(name: &str, args: &[u32]) -> Op {
    call(user32::address_of(name), args)
}

fn send(hwnd: u32, msg: u32, wp: u32, lp: Arg) -> Op {
    Op::Call {
        target: user32::address_of("SendMessageA"),
        args: vec![Arg::V(hwnd), Arg::V(msg), Arg::V(wp), lp],
    }
}

fn then_op(id: u16, data: &[u32]) -> Op {
    Op::Then {
        id,
        data: data.to_vec(),
    }
}

/// A PSHNOTIFY with `code` and `lparam` from the sheet to `page`.
fn psn(sheet: u32, page: u32, code: u32, lparam: u32) -> Vec<Op> {
    vec![
        Op::Write {
            at: P_PSN,
            words: vec![sheet, 0, code, lparam],
        },
        send(page, WM_NOTIFY, 0, Arg::S(P_PSN)),
    ]
}

/// `text` in the scratch space's text, cut to MAX_TEXT bytes.
fn write_text(text: &[u8]) -> Op {
    let mut b = text[..text.len().min(MAX_TEXT)].to_vec();
    b.push(0);
    Op::Write {
        at: P_TEXT,
        words: script::words_of(&b),
    }
}

/// SetWindowTextA(hwnd, text).
fn set_text(hwnd: u32, text: &[u8]) -> Vec<Op> {
    vec![
        write_text(text),
        Op::Call {
            target: user32::address_of("SetWindowTextA"),
            args: vec![Arg::V(hwnd), Arg::S(P_TEXT)],
        },
    ]
}

fn item(cpu: &mut Cpu, dlg: u32, id: u32) -> u32 {
    dialog::get_dlg_item(cpu, &[dlg, id]).unwrap_or(0)
}

/// SetWindowLongA(hwnd, DWL_MSGRESULT, value), which doesn't call into
/// the program.
fn set_result(cpu: &mut Cpu, hwnd: u32, value: u32) {
    window::set_window_long(cpu, &[hwnd, DWL_MSGRESULT, value]);
}

/// A window's rectangle in its parent's client area.
fn child_rect(cpu: &mut Cpu, hwnd: u32) -> Rect {
    user32::user(cpu)
        .window(hwnd)
        .map(|w| w.rect)
        .unwrap_or_default()
}

/// The size of a window's client area.
fn client_size(cpu: &mut Cpu, hwnd: u32) -> (i32, i32) {
    let r = super::client(cpu, hwnd);
    (r.width(), r.height())
}

/// How much bigger a window is than its client area.
fn frame_size(cpu: &mut Cpu, hwnd: u32) -> (i32, i32) {
    user32::user(cpu).window(hwnd).map_or((0, 0), |w| {
        (
            w.rect.width() - w.client.width(),
            w.rect.height() - w.client.height(),
        )
    })
}

/// MapDialogRect(dlg, r).
fn map_rect(cpu: &mut Cpu, dlg: u32, r: Rect) -> Rect {
    let heap = process_heap(cpu);
    let at = heap_alloc(cpu, heap, 16);
    if at == 0 {
        return r;
    }
    r.write(cpu, at);
    dialog::map_dialog_rect(cpu, &[dlg, at]);
    let out = Rect::read(cpu, at);
    heap_free(cpu, heap, at);
    out
}

/// LoadStringA(instance, id): the string, or None.
fn load_string(cpu: &mut Cpu, instance: u32, id: u32) -> Option<Vec<u8>> {
    let heap = process_heap(cpu);
    let at = heap_alloc(cpu, heap, 256);
    if at == 0 {
        return None;
    }
    let n = user32::resource::load_string(cpu, &[instance, id, at, 256]).unwrap_or(0);
    let text = user32::bytes_at(cpu, at);
    heap_free(cpu, heap, at);
    (n != 0).then_some(text).flatten()
}

fn r16(cpu: &mut Cpu, at: u32) -> u32 {
    let b = read_bytes(cpu, at, 2);
    u16::from_le_bytes([b[0], b[1]]) as u32
}

fn read_bytes(cpu: &mut Cpu, at: u32, n: usize) -> Vec<u8> {
    let mut b = vec![0; n];
    cpu.bus.guest_read_bytes(at, &mut b);
    b
}

/// The address after the zero-ended UTF-16 string at `at`.
fn skip_wide(cpu: &mut Cpu, mut at: u32) -> u32 {
    for _ in 0..0x1_0000 {
        let c = r16(cpu, at);
        at += 2;
        if c == 0 || cpu.bus.guest_faulted() {
            break;
        }
    }
    at
}

/// The address after a template's name or number (menus, classes).
fn skip_name(cpu: &mut Cpu, at: u32) -> u32 {
    match r16(cpu, at) {
        0 => at + 2,
        0xFFFF => at + 4,
        _ => skip_wide(cpu, at),
    }
}

/// GetTemplateSize: the bytes of the dialog template at `at`, a
/// DLGTEMPLATE or a DLGTEMPLATEEX, with its items.
fn template_size(cpu: &mut Cpu, at: u32) -> u32 {
    let ex = r16(cpu, at + 2) == 0xFFFF;
    let (style, count, mut p) = if ex {
        (cpu.bus.guest_read_32(at + 12), r16(cpu, at + 16), at + 26)
    } else {
        (cpu.bus.guest_read_32(at), r16(cpu, at + 8), at + 18)
    };
    p = skip_name(cpu, p);
    p = skip_name(cpu, p);
    p = skip_wide(cpu, p);
    if style & DS_SETFONT != 0 {
        p += if ex { 6 } else { 2 };
        p = skip_wide(cpu, p);
    }
    for _ in 0..count {
        p = p.next_multiple_of(4);
        p += if ex { 24 } else { 18 };
        p = skip_name(cpu, p);
        p = skip_name(cpu, p);
        let extra = r16(cpu, p);
        p += 2 + extra;
        if cpu.bus.guest_faulted() {
            break;
        }
    }
    p - at
}

/// The template a page is made from: its address and size, or None.
fn page_template(cpu: &mut Cpu, hpage: u32) -> Option<(u32, u32)> {
    let w = read_words(cpu, hpage, 4);
    if w[1] & PSP_DLGINDIRECT != 0 {
        if w[3] == 0 {
            return None;
        }
        return Some((w[3], template_size(cpu, w[3])));
    }
    let res = kernel32(cpu, "FindResourceA", &[w[2], w[3], RT_DIALOG]).unwrap_or(0);
    if res == 0 {
        return None;
    }
    let data = kernel32(cpu, "LoadResource", &[w[2], res]).unwrap_or(0);
    let data = kernel32(cpu, "LockResource", &[data]).unwrap_or(0);
    let size = kernel32(cpu, "SizeofResource", &[w[2], res]).unwrap_or(0);
    (data != 0).then_some((data, size))
}

/// PROPSHEET_CollectPageInfo: the page `hpage` of the sheet, with its
/// tab's text, and the largest page's size in dialog units; None for a
/// page without a template.
fn collect_page(cpu: &mut Cpu, sheet: &mut Sheet, hpage: u32, resize: bool) -> Option<Page> {
    if hpage == 0 {
        return None;
    }
    let w = read_words(cpu, hpage, 6);
    let (flags, instance, title) = (w[1], w[2], w[5]);
    let (at, _) = page_template(cpu, hpage)?;
    let ex = r16(cpu, at + 2) == 0xFFFF;
    let mut p = if ex { at + 18 } else { at + 10 };
    let mut width = r16(cpu, p + 4) as i32;
    let mut height = r16(cpu, p + 6) as i32;
    p += 8;
    if sheet.flags & PSH_WIZARD != 0 {
        width += 2 * WIZARD_PADDING;
        height += 2 * WIZARD_PADDING;
    }
    if resize {
        sheet.width = sheet.width.max(width);
        sheet.height = sheet.height.max(height);
    }
    p = skip_name(cpu, p);
    p = skip_name(cpu, p);
    let mut text = Vec::new();
    for _ in 0..MAX_TEXT {
        let c = r16(cpu, p) as u16;
        p += 2;
        if c == 0 || cpu.bus.guest_faulted() {
            break;
        }
        text.push(CodePage::Ansi.byte(c).unwrap_or(b'?'));
    }
    if flags & PSP_USETITLE != 0 {
        text = if title < 0x1_0000 {
            load_string(cpu, instance, title).unwrap_or_else(|| b"(null)".to_vec())
        } else {
            user32::bytes_at(cpu, title).unwrap_or_default()
        };
    }
    let has_help = flags & PSP_HASHELP != 0;
    if has_help {
        sheet.has_help = true;
    }
    Some(Page {
        hpage,
        text,
        has_help,
        ..Page::default()
    })
}

// The sheet's own template.

/// A control of the sheet's template: style, place, id, class (a
/// predefined class's atom, or a name) and text.
struct Control {
    style: u32,
    rect: (i16, i16, i16, i16),
    id: u16,
    class: Result<u16, &'static str>,
    text: &'static str,
}

const BUTTON: Result<u16, &str> = Ok(0x0080);
const STATIC: Result<u16, &str> = Ok(0x0082);
const TAB: Result<u16, &str> = Err("SysTabControl32");
const BS_DEFPUSHBUTTON: u32 = 0x0001;
const SS_SUNKEN: u32 = 0x1000;
const WS_GROUP: u32 = 0x0002_0000;
const WS_CLIPSIBLINGS: u32 = 0x0400_0000;
const TCS_MULTILINE: u32 = tab::TCS_MULTILINE;
const CONTROL: u32 = WS_CHILD | WS_VISIBLE;
const PUSH: u32 = CONTROL | WS_TABSTOP;
const LTEXT: u32 = CONTROL | WS_GROUP;

/// Wine's IDD_PROPSHEET.
fn sheet_controls() -> Vec<Control> {
    let b = |style, x, id, text| Control {
        style,
        rect: (x, 122, 50, 14),
        id,
        class: BUTTON,
        text,
    };
    vec![
        b(PUSH | BS_DEFPUSHBUTTON | WS_GROUP, 4, IDOK as u16, "OK"),
        b(PUSH, 58, IDCANCEL as u16, "Cancel"),
        b(PUSH | WS_DISABLED, 112, IDC_APPLY_BUTTON as u16, "&Apply"),
        b(PUSH | WS_GROUP, 166, IDHELP as u16, "Help"),
        Control {
            style: CONTROL | WS_CLIPSIBLINGS | WS_GROUP | WS_TABSTOP | TCS_MULTILINE,
            rect: (4, 4, 212, 114),
            id: IDC_TABCONTROL as u16,
            class: TAB,
            text: "Tab",
        },
    ]
}

/// Wine's IDD_WIZARD.
fn wizard_controls() -> Vec<Control> {
    let b = |style, x, id, text| Control {
        style,
        rect: (x, 138, 50, 14),
        id,
        class: BUTTON,
        text,
    };
    vec![
        b(PUSH, 71, IDC_BACK_BUTTON as u16, "< &Back"),
        b(
            PUSH | BS_DEFPUSHBUTTON,
            121,
            IDC_NEXT_BUTTON as u16,
            "&Next >",
        ),
        b(
            PUSH | BS_DEFPUSHBUTTON,
            121,
            IDC_FINISH_BUTTON as u16,
            "Finish",
        ),
        b(PUSH, 178, IDCANCEL as u16, "Cancel"),
        b(PUSH | WS_GROUP, 235, IDHELP as u16, "Help"),
        Control {
            style: LTEXT | SS_SUNKEN,
            rect: (7, 129, 278, 1),
            id: IDC_SUNKEN_LINE as u16,
            class: STATIC,
            text: "",
        },
        Control {
            style: CONTROL | WS_CLIPSIBLINGS | WS_DISABLED,
            rect: (7, 7, 258, 5),
            id: IDC_TABCONTROL as u16,
            class: TAB,
            text: "Tab",
        },
        Control {
            style: LTEXT | SS_SUNKEN,
            rect: (0, 35, 290, 1),
            id: IDC_SUNKEN_LINEHEADER as u16,
            class: STATIC,
            text: "",
        },
    ]
}

fn push_wide(b: &mut Vec<u8>, s: &str) {
    for c in s.encode_utf16().chain([0]) {
        b.extend_from_slice(&c.to_le_bytes());
    }
}

fn push16(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&(v as u16).to_le_bytes());
}

fn push32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}

/// The sheet's DLGTEMPLATE, or the wizard's, as PROPSHEET_CreateDialog
/// adjusts it for PSH_NOCONTEXTHELP and PSH_WIZARDCONTEXTHELP.
fn sheet_template(flags: u32) -> Vec<u8> {
    let wizard = flags & ANY_WIZARD != 0;
    let (mut style, size, caption, controls) = if wizard {
        (0, (290, 159), "Wizard", wizard_controls())
    } else {
        let caption = std::str::from_utf8(PROPERTIES_FOR).unwrap_or("");
        (DS_CONTEXTHELP, (220, 140), caption, sheet_controls())
    };
    style |= DS_MODALFRAME | WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VISIBLE | DS_SETFONT;
    if flags & PSH_NOCONTEXTHELP != 0 {
        style &= !DS_CONTEXTHELP;
    }
    if wizard && flags & PSH_WIZARDCONTEXTHELP != 0 {
        style |= DS_CONTEXTHELP;
    }
    let mut b = Vec::new();
    push32(&mut b, style);
    push32(&mut b, 0);
    push16(&mut b, controls.len() as u32);
    for v in [0, 0, size.0, size.1] {
        push16(&mut b, v);
    }
    push16(&mut b, 0);
    push16(&mut b, 0);
    push_wide(&mut b, caption);
    push16(&mut b, 8);
    push_wide(&mut b, "MS Shell Dlg");
    for c in &controls {
        while b.len() % 4 != 0 {
            b.push(0);
        }
        push32(&mut b, c.style);
        push32(&mut b, 0);
        let (x, y, cx, cy) = c.rect;
        for v in [x, y, cx, cy] {
            push16(&mut b, v as u32);
        }
        push16(&mut b, c.id as u32);
        match c.class {
            Ok(atom) => {
                push16(&mut b, 0xFFFF);
                push16(&mut b, atom as u32);
            }
            Err(name) => push_wide(&mut b, name),
        }
        push_wide(&mut b, c.text);
        push16(&mut b, 0);
    }
    b
}

/// A block of the process's heap holding `bytes`, or 0.
fn heap_copy(cpu: &mut Cpu, bytes: &[u8]) -> u32 {
    let heap = process_heap(cpu);
    let at = heap_alloc(cpu, heap, bytes.len() as u32);
    if at != 0 {
        cpu.bus.guest_write_bytes(at, bytes);
    }
    at
}

fn heap_release(cpu: &mut Cpu, at: u32) {
    if at != 0 {
        let heap = process_heap(cpu);
        heap_free(cpu, heap, at);
    }
}

// The exports.

/// CreatePropertySheetPageA(page): COMCTL32's copy of the
/// PROPSHEETPAGEA, all its dwSize bytes as Windows copies them (MFC
/// keeps its own data after the structure), with the template's and
/// the title's names copied too.
pub fn create_property_sheet_page(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if at == 0 {
        return Some(0);
    }
    let size = cpu.bus.guest_read_32(at);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let len = size.clamp(PROPSHEETPAGEA_V1_SIZE, 0x1_0000) as usize;
    let mut bytes = read_bytes(cpu, at, len);
    let word = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let (flags, template, title) = (word(&bytes, 4), word(&bytes, 12), word(&bytes, 20));
    let template_name = (flags & PSP_DLGINDIRECT == 0 && template >= 0x1_0000)
        .then(|| user32::bytes_at(cpu, template).unwrap_or_default());
    let title_text = (flags & PSP_USETITLE != 0 && title >= 0x1_0000)
        .then(|| user32::bytes_at(cpu, title).unwrap_or_default());
    let refs = word(&bytes, 36);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let mut block = len as u32;
    let mut tail = Vec::new();
    let mut names = Vec::new();
    for (field, text) in [(12, template_name), (20, title_text)] {
        if let Some(mut t) = text {
            t.push(0);
            names.push((field, block + tail.len() as u32));
            tail.extend(t);
        }
    }
    block += tail.len() as u32;
    let heap = process_heap(cpu);
    let hpage = heap_alloc(cpu, heap, block);
    if hpage == 0 {
        return Some(0);
    }
    for (field, offset) in names {
        bytes[field..field + 4].copy_from_slice(&(hpage + offset).to_le_bytes());
    }
    bytes.extend(tail);
    cpu.bus.guest_write_bytes(hpage, &bytes);
    if flags & PSP_USEREFPARENT != 0 && refs != 0 {
        let n = cpu.bus.guest_read_32(refs);
        cpu.bus.guest_write_32(refs, n.wrapping_add(1));
    }
    Some(hpage)
}

/// A function Windows 95 has no Unicode version of: it fails.
pub fn fail(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// What destroying the page `hpage` takes: PSPCB_RELEASE to its
/// callback, then the block freed.
fn destroy_page(cpu: &mut Cpu, hpage: u32) -> Vec<Op> {
    let w = read_words(cpu, hpage, 10);
    let mut ops = Vec::new();
    if w[1] & PSP_USECALLBACK != 0 && w[8] != 0 {
        ops.push(call(w[8], &[0, PSPCB_RELEASE, hpage]));
    }
    ops.push(then_op(T_FREE_PAGE, &[hpage]));
    ops
}

/// DestroyPropertySheetPage(hpage).
pub fn destroy_property_sheet_page(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hpage = args[0];
    if hpage == 0 {
        return Some(0);
    }
    let mut s = Script::new(0);
    s.alloc(P_SIZE);
    s.ops = destroy_page(cpu, hpage);
    s.result = 1;
    script::run(cpu, PROPSHEET, s)
}

/// PropertySheetA(header): a modeless sheet's window, or -1; a modal
/// sheet's result when it has ended (IDOK, ID_PSRESTARTWINDOWS,
/// ID_PSREBOOTSYSTEM, 0 for Cancel), or -1.
pub fn property_sheet(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if at == 0 {
        return Some(u32::MAX);
    }
    let w = read_words(cpu, at, 10);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (size, flags, parent, instance) = (w[0], w[1], w[2], w[3]);
    let (caption, n, start, pages, callback) = (w[5], w[6], w[7], w[8], w[9]);
    if size < PROPSHEETHEADERA_V1_SIZE {
        return Some(u32::MAX);
    }
    // The pages, read before anything changes.
    let n = n.min(0x1000);
    let mut structs = Vec::new();
    let mut handles = Vec::new();
    if flags & PSH_PROPSHEETPAGE != 0 {
        let mut p = pages;
        for _ in 0..n {
            let len = cpu.bus.guest_read_32(p).max(PROPSHEETPAGEA_V1_SIZE);
            structs.push(p);
            p = p.wrapping_add(len);
        }
    } else {
        handles = read_words(cpu, pages, n as usize);
    }
    let caption_text = (flags & ANY_WIZARD == 0 && caption >= 0x1_0000)
        .then(|| user32::bytes_at(cpu, caption).unwrap_or_default());
    if cpu.bus.guest_faulted() {
        return None;
    }
    for p in structs {
        let h = create_property_sheet_page(cpu, &[p]).unwrap_or(0);
        handles.push(h);
    }
    let mut sheet = Sheet {
        flags,
        parent,
        instance,
        caption: caption_text,
        caption_id: if flags & ANY_WIZARD == 0 && caption < 0x1_0000 {
            caption
        } else {
            0
        },
        callback: if flags & PSH_USECALLBACK != 0 {
            callback
        } else {
            0
        },
        modeless: flags & PSH_MODELESS != 0,
        has_help: flags & PSH_HASHELP != 0,
        has_apply: flags & PSH_NOAPPLYNOW == 0,
        has_finish: flags & PSH_WIZARDHASFINISH != 0,
        ..Sheet::default()
    };
    let mut dropped = Vec::new();
    for h in handles {
        match collect_page(cpu, &mut sheet, h, true) {
            Some(p) => sheet.pages.push(p),
            None => {
                if flags & PSH_PROPSHEETPAGE != 0 && h != 0 {
                    dropped.push(h);
                }
            }
        }
    }
    for h in dropped {
        heap_release(cpu, h);
    }
    // PSH_USEPSTARTPAGE names the page by its title; Wine and this
    // start at the first.
    let start = if flags & PSH_USEPSTARTPAGE != 0 {
        0
    } else {
        start as i32
    };
    sheet.active = if start < 0 || start as usize >= sheet.pages.len() {
        0
    } else {
        start
    };
    let template = heap_copy(cpu, &sheet_template(flags));
    sheet.template = template;
    let ps = &mut state(cpu).propsheet;
    ps.next += 1;
    let id = ps.next;
    let (modeless, sheet_callback) = (sheet.modeless, sheet.callback);
    put(cpu, id, sheet);
    let mut s = Script::new(0);
    s.alloc(P_SIZE);
    s.result = u32::MAX;
    if sheet_callback != 0 {
        s.ops
            .push(call(sheet_callback, &[0, PSCB_PRECREATE, template]));
    }
    if !modeless && parent != 0 {
        s.ops.push(user("EnableWindow", &[parent, 0]));
    }
    let proc = super::address_of("PropSheetDlgProc");
    let create = user32::address_of("CreateDialogIndirectParamA");
    s.ops
        .push(call(create, &[instance, template, parent, proc, id]));
    s.ops.push(then_op(T_CREATED, &[id]));
    script::run(cpu, PROPSHEET, s)
}

/// A function Windows 95 has no Unicode version of: -1.
pub fn fail_minus_one(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(u32::MAX)
}

// The modal loop (do_loop).

/// After CreateDialogIndirectParamA: a modeless sheet returns its
/// window; a modal one runs its loop.
fn created(cpu: &mut Cpu, c: &mut Ctx, id: u32) -> Vec<Op> {
    let Some(mut sheet) = get(cpu, id) else {
        return Vec::new();
    };
    heap_release(cpu, sheet.template);
    sheet.template = 0;
    let hwnd = c.last;
    if sheet.modeless {
        c.result = if hwnd != 0 { hwnd } else { u32::MAX };
        if hwnd == 0 {
            state(cpu).propsheet.sheets.remove(&id);
        } else {
            put(cpu, id, sheet);
        }
        return Vec::new();
    }
    if hwnd == 0 {
        c.result = u32::MAX;
        state(cpu).propsheet.sheets.remove(&id);
        if sheet.parent != 0 {
            return vec![user("EnableWindow", &[sheet.parent, 1])];
        }
        return Vec::new();
    }
    put(cpu, id, sheet);
    vec![then_op(T_LOOP, &[id])]
}

/// The end of the loop: the dialog destroyed and the owner enabled.
fn end_loop(cpu: &mut Cpu, c: &mut Ctx, id: u32, result: u32) -> Vec<Op> {
    c.result = result;
    let Some(sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let mut ops = Vec::new();
    if !sheet.destroyed && user32::user(cpu).is_window(sheet.hwnd) {
        ops.push(user("DestroyWindow", &[sheet.hwnd]));
    }
    if sheet.parent != 0 {
        ops.push(user("EnableWindow", &[sheet.parent, 1]));
    }
    ops.push(then_op(T_DONE, &[id]));
    ops
}

fn run_loop(cpu: &mut Cpu, c: &mut Ctx, id: u32) -> Vec<Op> {
    let Some(sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let alive = !sheet.destroyed && user32::user(cpu).is_window(sheet.hwnd);
    if !alive || sheet.ended {
        return end_loop(cpu, c, id, sheet.result as u32);
    }
    vec![
        Op::Call {
            target: user32::address_of("GetMessageA"),
            args: vec![Arg::S(P_MSG), Arg::V(0), Arg::V(0), Arg::V(0)],
        },
        then_op(T_GOT, &[id]),
    ]
}

fn got_message(cpu: &mut Cpu, c: &mut Ctx, id: u32) -> Vec<Op> {
    match c.last {
        // WM_QUIT goes back in the queue for the program's loop. Windows
        // returns 0 then (Wine -1).
        0 => {
            let code = cpu.bus.guest_read_32(c.scratch + P_MSG + 8);
            user32::message::post_quit_message(cpu, &[code]);
            end_loop(cpu, c, id, 0)
        }
        u32::MAX => end_loop(cpu, c, id, u32::MAX),
        _ => {
            let hwnd = get(cpu, id).map_or(0, |s| s.hwnd);
            vec![
                Op::Call {
                    target: user32::address_of("IsDialogMessageA"),
                    args: vec![Arg::V(hwnd), Arg::S(P_MSG)],
                },
                then_op(T_DISPATCHED, &[id]),
            ]
        }
    }
}

fn dispatched(c: &Ctx, id: u32) -> Vec<Op> {
    let mut ops = Vec::new();
    if c.last == 0 {
        for name in ["TranslateMessage", "DispatchMessageA"] {
            ops.push(Op::Call {
                target: user32::address_of(name),
                args: vec![Arg::S(P_MSG)],
            });
        }
    }
    ops.push(then_op(T_LOOP, &[id]));
    ops
}

// The sheet's work.

/// The index of the page `hpage`, or -1.
fn page_index(sheet: &Sheet, hpage: u32) -> i32 {
    sheet
        .pages
        .iter()
        .position(|p| p.hpage == hpage)
        .map_or(-1, |i| i as i32)
}

/// PROPSHEET_FindPageByResId: the page made from the template
/// `res`, or the number of pages.
fn find_by_res_id(cpu: &mut Cpu, sheet: &Sheet, res: u32) -> i32 {
    for (i, p) in sheet.pages.iter().enumerate() {
        if cpu.bus.guest_read_32(p.hpage + 12) == res {
            return i as i32;
        }
    }
    sheet.pages.len() as i32
}

fn page_hwnd(sheet: &Sheet, i: i32) -> u32 {
    usize::try_from(i)
        .ok()
        .and_then(|i| sheet.pages.get(i))
        .map_or(0, |p| p.hwnd)
}

/// PSN_KILLACTIVE to the active page, then the continuation `id` with
/// `data` and a word saying whether the page was asked: `refused` then
/// tells whether it kept the sheet on it (PROPSHEET_CanSetCurSel).
fn kill_active(sheet: &Sheet, id: u16, data: &[u32]) -> Vec<Op> {
    let mut data = data.to_vec();
    let mut ops = Vec::new();
    if sheet.active >= 0 {
        ops = psn(
            sheet.hwnd,
            page_hwnd(sheet, sheet.active),
            PSN_KILLACTIVE,
            0,
        );
        data.push(1);
    } else {
        data.push(0);
    }
    ops.push(then_op(id, &data));
    ops
}

/// Whether the page kept the sheet on it, after `kill_active`, whose
/// word is the continuation's `i`th.
fn refused(c: &Ctx, i: usize) -> bool {
    c.word(i) != 0 && c.last != 0
}

/// PROPSHEET_SetTitle: the sheet's caption, "Properties for" it with
/// PSH_PROPTITLE.
fn title_ops(hwnd: u32, style: u32, text: &[u8]) -> Vec<Op> {
    let mut caption = text.to_vec();
    if style & PSH_PROPTITLE != 0 {
        let at = PROPERTIES_FOR
            .windows(2)
            .position(|w| w == b"%s")
            .unwrap_or(0);
        caption = PROPERTIES_FOR[..at].to_vec();
        caption.extend_from_slice(text);
        caption.extend_from_slice(&PROPERTIES_FOR[at + 2..]);
    }
    set_text(hwnd, &caption)
}

/// PROPSHEET_SetCurSel(index, skipdir, hpage).
fn set_cur_sel(id: u32, index: i32, skipdir: i32, hpage: u32) -> Vec<Op> {
    vec![then_op(
        T_SC_BEGIN,
        &[id, index as u32, skipdir as u32, hpage],
    )]
}

/// PROPSHEET_ShowPage(index).
fn show_page(id: u32, index: i32) -> Vec<Op> {
    vec![then_op(T_SHOW, &[id, index as u32])]
}

fn sc_begin(cpu: &mut Cpu, id: u32, c: &Ctx) -> Vec<Op> {
    let Some(mut sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let (mut index, skipdir, hpage) = (c.word(1) as i32, c.word(2) as i32, c.word(3));
    if hpage != 0 {
        index = page_index(&sheet, hpage);
    }
    if index < 0 || index as usize >= sheet.pages.len() {
        return Vec::new();
    }
    let mut ops = Vec::new();
    if sheet.active != -1 {
        ops.push(user(
            "ShowWindow",
            &[page_hwnd(&sheet, sheet.active), SW_HIDE],
        ));
    }
    sheet.active = -1;
    put(cpu, id, sheet.clone());
    ops.extend(sc_step(cpu, &sheet, id, index, skipdir, 0));
    ops
}

/// A turn of PROPSHEET_SetCurSel's loop: the tab selected, the page
/// made if it isn't yet, then T_SC_PLACE.
fn sc_step(cpu: &mut Cpu, sheet: &Sheet, id: u32, index: i32, skipdir: i32, n: u32) -> Vec<Op> {
    let mut ops = Vec::new();
    let tab = item(cpu, sheet.hwnd, IDC_TABCONTROL);
    if tab != 0 {
        ops.push(send(tab, TCM_SETCURSEL, index as u32, Arg::V(0)));
    }
    if page_hwnd(sheet, index) == 0 {
        ops.extend(create_page(cpu, sheet, id, index));
    }
    ops.push(then_op(T_SC_PLACE, &[id, index as u32, skipdir as u32, n]));
    ops
}

/// PROPSHEET_GetPageRect: where the page goes in the sheet.
fn page_rect(cpu: &mut Cpu, sheet: &Sheet) -> Rect {
    if sheet.flags & PSH_WIZARD != 0 {
        let r = Rect::new(
            WIZARD_PADDING,
            WIZARD_PADDING,
            sheet.width - WIZARD_PADDING,
            sheet.height - WIZARD_PADDING,
        );
        return map_rect(cpu, sheet.hwnd, r);
    }
    if wizard(sheet) {
        return map_rect(cpu, sheet.hwnd, Rect::new(0, 0, sheet.width, sheet.height));
    }
    let tab = item(cpu, sheet.hwnd, IDC_TABCONTROL);
    let client = super::client(cpu, tab);
    let r = tab::adjust(cpu, tab, false, client);
    let st = user32::user(cpu);
    let (tx, ty) = st.client_origin(tab);
    let (dx, dy) = st.client_origin(sheet.hwnd);
    r.offset(tx - dx, ty - dy)
}

fn sc_place(cpu: &mut Cpu, id: u32, c: &Ctx) -> Vec<Op> {
    let Some(sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let index = c.word(1) as i32;
    let page = page_hwnd(&sheet, index);
    let r = page_rect(cpu, &sheet);
    let mut ops = vec![user(
        "SetWindowPos",
        &[
            page,
            HWND_TOP,
            r.left as u32,
            r.top as u32,
            r.width() as u32,
            r.height() as u32,
            0,
        ],
    )];
    ops.extend(psn(sheet.hwnd, page, PSN_SETACTIVE, 0));
    ops.push(then_op(T_SC_RESULT, &c.data));
    ops
}

/// After PSN_SETACTIVE: 0 keeps the page, -1 skips it the way the
/// sheet goes, and a template's number goes to that page.
fn sc_result(cpu: &mut Cpu, id: u32, c: &Ctx) -> Vec<Op> {
    let Some(sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let (mut index, skipdir, n) = (c.word(1) as i32, c.word(2) as i32, c.word(3));
    let count = sheet.pages.len() as i32;
    let result = c.last;
    let mut again = false;
    if result != 0 && n < MAX_SKIPS {
        if result == u32::MAX {
            index += skipdir;
            if index < 0 {
                index = 0;
            } else if index >= count {
                index = count - 1;
            } else {
                again = true;
            }
        } else {
            let found = find_by_res_id(cpu, &sheet, result);
            if found < count {
                index = found;
                again = true;
            }
        }
    }
    if again {
        return sc_step(cpu, &sheet, id, index, skipdir, n + 1);
    }
    let mut ops = show_page(id, index);
    let help = item(cpu, sheet.hwnd, IDHELP);
    let has_help = sheet.pages.get(index as usize).is_some_and(|p| p.has_help);
    ops.push(user("EnableWindow", &[help, has_help as u32]));
    ops
}

/// PROPSHEET_ShowPage: the page shown, the one before hidden; a
/// wizard takes the page's caption and puts the focus on its first
/// control.
fn show(cpu: &mut Cpu, id: u32, c: &Ctx) -> Vec<Op> {
    let Some(mut sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let index = c.word(1) as i32;
    if index == sheet.active || index < 0 || index as usize >= sheet.pages.len() {
        return Vec::new();
    }
    let page = page_hwnd(&sheet, index);
    if page == 0 {
        // Made first, then shown.
        let mut ops = create_page(cpu, &sheet, id, index);
        if ops.is_empty() {
            return ops;
        }
        ops.push(then_op(T_SHOW, &c.data));
        return ops;
    }
    let mut ops = Vec::new();
    if wizard(&sheet) {
        let text = sheet.pages[index as usize].text.clone();
        ops.extend(title_ops(sheet.hwnd, sheet.flags, &text));
        let control = dialog::get_next_dlg_tab_item(cpu, &[page, 0, 0]).unwrap_or(0);
        if control != 0 {
            ops.push(user("SetFocus", &[control]));
        }
    }
    if sheet.active != -1 {
        ops.push(user(
            "ShowWindow",
            &[page_hwnd(&sheet, sheet.active), SW_HIDE],
        ));
    }
    ops.push(user("ShowWindow", &[page, SW_SHOW]));
    let tab = item(cpu, sheet.hwnd, IDC_TABCONTROL);
    ops.push(send(tab, TCM_SETCURSEL, index as u32, Arg::V(0)));
    sheet.active = index;
    sheet.active_valid = true;
    put(cpu, id, sheet);
    ops
}

/// PROPSHEET_CreatePage: the page's dialog, from a copy of its template
/// made a child: PSPCB_CREATE to its callback, then
/// CreateDialogIndirectParamA with the page's procedure.
fn create_page(cpu: &mut Cpu, sheet: &Sheet, id: u32, index: i32) -> Vec<Op> {
    let Some(page) = sheet.pages.get(index as usize) else {
        return Vec::new();
    };
    let hpage = page.hpage;
    let w = read_words(cpu, hpage, 10);
    let Some((at, size)) = page_template(cpu, hpage) else {
        return Vec::new();
    };
    let mut t = read_bytes(cpu, at, size as usize);
    if t.len() < 18 {
        return Vec::new();
    }
    let ex = t[2] == 0xFF && t[3] == 0xFF;
    let (style_at, ex_at) = if ex { (12, 8) } else { (0, 4) };
    let get32 = |t: &[u8], i: usize| u32::from_le_bytes([t[i], t[i + 1], t[i + 2], t[i + 3]]);
    let mut style = get32(&t, style_at);
    style |= WS_CHILD | WS_TABSTOP | DS_CONTROL;
    style &= !(DS_MODALFRAME
        | WS_CAPTION
        | WS_SYSMENU
        | WS_POPUP
        | WS_DISABLED
        | WS_VISIBLE
        | WS_THICKFRAME);
    let ex_style = get32(&t, ex_at) | WS_EX_CONTROLPARENT;
    t[style_at..style_at + 4].copy_from_slice(&style.to_le_bytes());
    t[ex_at..ex_at + 4].copy_from_slice(&ex_style.to_le_bytes());
    let copy = heap_copy(cpu, &t);
    if copy == 0 {
        return Vec::new();
    }
    let mut ops = Vec::new();
    if w[1] & PSP_USECALLBACK != 0 && w[8] != 0 {
        ops.push(call(w[8], &[0, PSPCB_CREATE, hpage]));
    }
    let create = user32::address_of("CreateDialogIndirectParamA");
    ops.push(call(create, &[w[2], copy, sheet.hwnd, w[6], hpage]));
    ops.push(then_op(T_PAGE_CREATED, &[id, hpage, copy]));
    ops
}

fn page_created(cpu: &mut Cpu, id: u32, c: &Ctx) -> Vec<Op> {
    heap_release(cpu, c.word(2));
    if let Some(mut sheet) = get(cpu, id) {
        if let Some(p) = sheet.pages.iter_mut().find(|p| p.hpage == c.word(1)) {
            p.hwnd = c.last;
        }
        put(cpu, id, sheet);
    }
    Vec::new()
}

/// PROPSHEET_SetWizButtons.
fn wiz_buttons(cpu: &mut Cpu, sheet: &Sheet, flags: u32) -> Vec<Op> {
    let hwnd = sheet.hwnd;
    let back = item(cpu, hwnd, IDC_BACK_BUTTON);
    let next = item(cpu, hwnd, IDC_NEXT_BUTTON);
    let finish = item(cpu, hwnd, IDC_FINISH_BUTTON);
    let mut ops = vec![
        user("EnableWindow", &[back, 0]),
        user("EnableWindow", &[next, 0]),
        user("EnableWindow", &[finish, 0]),
    ];
    let disabled_finish = flags & PSWIZB_DISABLEDFINISH != 0;
    let def = if (flags & PSWIZB_FINISH != 0 || sheet.has_finish) && !disabled_finish {
        IDC_FINISH_BUTTON
    } else if flags & PSWIZB_NEXT != 0 {
        IDC_NEXT_BUTTON
    } else if flags & PSWIZB_BACK != 0 {
        IDC_BACK_BUTTON
    } else {
        IDCANCEL
    };
    ops.push(send(hwnd, DM_SETDEFID, def, Arg::V(0)));
    if flags & PSWIZB_BACK != 0 {
        ops.push(user("EnableWindow", &[back, 1]));
    }
    if flags & PSWIZB_NEXT != 0 {
        ops.push(user("EnableWindow", &[next, 1]));
    }
    if !sheet.has_finish {
        if flags & (PSWIZB_FINISH | PSWIZB_DISABLEDFINISH) != 0 {
            ops.push(user("ShowWindow", &[next, SW_HIDE]));
            ops.push(user("ShowWindow", &[finish, SW_SHOW]));
            if !disabled_finish {
                ops.push(user("EnableWindow", &[finish, 1]));
            }
        } else {
            ops.push(user("ShowWindow", &[finish, SW_HIDE]));
            ops.push(user("ShowWindow", &[next, SW_SHOW]));
        }
    } else if !disabled_finish {
        ops.push(user("EnableWindow", &[finish, 1]));
    }
    ops
}

fn move_to(hwnd: u32, x: i32, y: i32) -> Op {
    user(
        "SetWindowPos",
        &[
            hwnd,
            0,
            x as u32,
            y as u32,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        ],
    )
}

fn resize(hwnd: u32, cx: i32, cy: i32) -> Op {
    user(
        "SetWindowPos",
        &[
            hwnd,
            0,
            0,
            0,
            cx as u32,
            cy as u32,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        ],
    )
}

/// PROPSHEET_AdjustSize and PROPSHEET_AdjustButtons: the tab control
/// and the sheet made to fit the largest page, the buttons along the
/// bottom, right-aligned.
fn adjust_sheet(cpu: &mut Cpu, sheet: &mut Sheet) -> Vec<Op> {
    let hwnd = sheet.hwnd;
    let tab = item(cpu, hwnd, IDC_TABCONTROL);
    let ok = item(cpu, hwnd, IDOK);
    let pad = child_rect(cpu, tab);
    let (px, py) = (pad.left, pad.top);
    let (bw, bh) = client_size(cpu, ok);
    let mut rc = map_rect(cpu, hwnd, Rect::new(0, 0, sheet.width, sheet.height));
    let units = map_rect(cpu, hwnd, Rect::new(4, 8, 4, 8));
    let tab_client = super::client(cpu, tab);
    // PROPSHEET_SizeMismatch.
    if rc.width() == tab_client.width() && rc.height() == tab_client.height() {
        return Vec::new();
    }
    let tab_rect = tab::adjust(cpu, tab, false, tab_client);
    if rc.height() < tab_rect.height() {
        rc.bottom = rc.top + tab_rect.height();
        sheet.height = super::gfx::mul_div(rc.height(), 8, units.top);
    }
    if rc.width() < tab_rect.width() {
        rc.right = rc.left + tab_rect.width();
        sheet.width = super::gfx::mul_div(rc.width(), 4, units.left);
    }
    let rc = tab::adjust(cpu, tab, true, rc);
    let (tw, th) = (rc.width(), rc.height());
    let mut ops = vec![resize(tab, tw, th)];
    // The sheet's client area, then its window.
    let cl = Rect::new(0, 0, tw + 2 * px, th + bh + 3 * py);
    let (fx, fy) = frame_size(cpu, hwnd);
    ops.push(resize(hwnd, cl.right + fx, cl.bottom + fy));
    // The buttons.
    let n = 2 + sheet.has_apply as i32 + sheet.has_help as i32;
    let y = cl.bottom - (py + bh);
    let mut x = cl.right - (px + bw) * n;
    ops.push(move_to(ok, x, y));
    ops.push(send(hwnd, DM_SETDEFID, IDOK, Arg::V(0)));
    x += px + bw;
    ops.push(move_to(item(cpu, hwnd, IDCANCEL), x, y));
    let apply = item(cpu, hwnd, IDC_APPLY_BUTTON);
    if sheet.has_apply {
        x += px + bw;
    } else {
        ops.push(user("ShowWindow", &[apply, SW_HIDE]));
    }
    ops.push(move_to(apply, x, y));
    ops.push(user("EnableWindow", &[apply, 0]));
    let help = item(cpu, hwnd, IDHELP);
    x += px + bw;
    ops.push(move_to(help, x, y));
    if !sheet.has_help {
        ops.push(user("ShowWindow", &[help, SW_HIDE]));
    }
    ops
}

/// PROPSHEET_AdjustSizeWizard and PROPSHEET_AdjustButtonsWizard.
fn adjust_wizard(cpu: &mut Cpu, sheet: &Sheet) -> Vec<Op> {
    let hwnd = sheet.hwnd;
    let line = item(cpu, hwnd, IDC_SUNKEN_LINE);
    let header = item(cpu, hwnd, IDC_SUNKEN_LINEHEADER);
    let mut rc = map_rect(cpu, hwnd, Rect::new(0, 0, sheet.width, sheet.height));
    let line_rect = child_rect(cpu, line);
    let (_, client_h) = client_size(cpu, hwnd);
    rc.bottom += client_h - line_rect.top - 1;
    let (fx, fy) = frame_size(cpu, hwnd);
    let mut ops = vec![resize(hwnd, rc.width() + fx, rc.height() + fy)];
    let cl = Rect::new(0, 0, rc.width(), rc.height());
    // PROPSHEET_GetPaddingInfoWizard: from the line to the buttons.
    let measure = if sheet.has_help {
        IDHELP
    } else {
        IDC_NEXT_BUTTON
    };
    let measure = item(cpu, hwnd, measure);
    let py = child_rect(cpu, measure).top - line_rect.bottom;
    let px = py;
    let n = 3 + sheet.has_help as i32 + sheet.has_finish as i32;
    let cancel = item(cpu, hwnd, IDCANCEL);
    let (bw, bh) = client_size(cpu, cancel);
    let (_, line_h) = client_size(cpu, line);
    let y = cl.bottom - (py + bh);
    let mut x = cl.right - (px + bw) * (n - 1) - bw;
    ops.push(move_to(item(cpu, hwnd, IDC_BACK_BUTTON), x, y));
    x += bw;
    ops.push(move_to(item(cpu, hwnd, IDC_NEXT_BUTTON), x, y));
    let finish = item(cpu, hwnd, IDC_FINISH_BUTTON);
    if sheet.has_finish {
        x += px + bw;
    }
    ops.push(move_to(finish, x, y));
    if !sheet.has_finish {
        ops.push(user("ShowWindow", &[finish, SW_HIDE]));
    }
    x += px + bw;
    ops.push(move_to(cancel, x, y));
    let help = item(cpu, hwnd, IDHELP);
    if sheet.has_help {
        x += px + bw;
        ops.push(move_to(help, x, y));
    } else {
        ops.push(user("ShowWindow", &[help, SW_HIDE]));
    }
    let y = cl.bottom - (py * 2 + bh + line_h);
    let width = cl.right - px * 2;
    ops.push(user(
        "SetWindowPos",
        &[
            line,
            0,
            px as u32,
            y as u32,
            width as u32,
            2,
            SWP_NOZORDER | SWP_NOACTIVATE,
        ],
    ));
    ops.push(resize(header, cl.right, 2));
    ops.push(user("ShowWindow", &[header, SW_HIDE]));
    ops
}

/// WM_INITDIALOG: the sheet takes its window, then the tabs go in
/// (T_INIT_TABS), the sheet is laid out, titled and shown to the
/// callback (T_INIT_LAYOUT), and the first page is selected.
fn init(cpu: &mut Cpu, hwnd: u32, id: u32) -> Option<u32> {
    let Some(mut sheet) = get(cpu, id) else {
        return Some(0);
    };
    sheet.hwnd = hwnd;
    state(cpu).propsheet.windows.insert(hwnd, id);
    let mut s = Script::new(hwnd);
    s.alloc(P_SIZE);
    if wizard(&sheet) {
        s.ops = wiz_buttons(cpu, &sheet, PSWIZB_BACK | PSWIZB_NEXT);
        let next = item(cpu, hwnd, IDC_NEXT_BUTTON);
        s.ops.push(user("SetFocus", &[next]));
    }
    s.ops.push(then_op(T_INIT_TABS, &[id]));
    s.ops.push(then_op(T_INIT_LAYOUT, &[id]));
    // Wizards set their focus themselves.
    s.result = (!wizard(&sheet)) as u32;
    put(cpu, id, sheet);
    script::run(cpu, PROPSHEET, s)
}

/// PROPSHEET_CreateTabControl: a tab for each page.
fn init_tabs(cpu: &mut Cpu, id: u32, c: &Ctx) -> Vec<Op> {
    let Some(sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let tab = item(cpu, sheet.hwnd, IDC_TABCONTROL);
    let mut ops = Vec::new();
    for (i, p) in sheet.pages.iter().enumerate() {
        ops.extend(insert_tab(c, tab, i as u32, &p.text));
    }
    ops
}

/// TCM_INSERTITEMA of a tab with `text`, from a continuation, which
/// knows where the scratch space is.
fn insert_tab(c: &Ctx, tab: u32, at: u32, text: &[u8]) -> Vec<Op> {
    vec![
        write_text(text),
        Op::Write {
            at: P_ITEM,
            words: vec![TCIF_TEXT, 0, 0, c.scratch + P_TEXT, MAX_TEXT as u32, 0, 0],
        },
        send(tab, TCM_INSERTITEMA, at, Arg::S(P_ITEM)),
    ]
}

fn init_layout(cpu: &mut Cpu, id: u32) -> Vec<Op> {
    let Some(mut sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let hwnd = sheet.hwnd;
    let mut ops = Vec::new();
    if wizard(&sheet) {
        let tab = item(cpu, hwnd, IDC_TABCONTROL);
        ops.push(user("ShowWindow", &[tab, SW_HIDE]));
        ops.extend(adjust_wizard(cpu, &sheet));
    } else {
        ops.extend(adjust_sheet(cpu, &mut sheet));
    }
    let caption = match &sheet.caption {
        Some(text) => Some(text.clone()),
        None if sheet.caption_id != 0 && sheet.instance != 0 => {
            load_string(cpu, sheet.instance, sheet.caption_id)
        }
        None => None,
    };
    if let Some(text) = caption {
        ops.extend(title_ops(hwnd, sheet.flags, &text));
    }
    if sheet.callback != 0 {
        ops.push(call(sheet.callback, &[hwnd, PSCB_INITIALIZED, 0]));
    }
    let index = sheet.active;
    let hpage = sheet.pages.get(index as usize).map_or(0, |p| p.hpage);
    sheet.active = -1;
    ops.extend(set_cur_sel(id, index, 1, hpage));
    ops.push(then_op(T_INIT_END, &[id]));
    put(cpu, id, sheet);
    ops
}

fn init_end(cpu: &mut Cpu, id: u32) -> Vec<Op> {
    let Some(sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let tab = item(cpu, sheet.hwnd, IDC_TABCONTROL);
    let mut ops = vec![send(tab, TCM_SETCURSEL, sheet.active as u32, Arg::V(0))];
    ops.extend(unchanged(cpu, id, 0));
    ops
}

/// PROPSHEET_Changed: the page marked dirty, Apply enabled.
fn changed(cpu: &mut Cpu, id: u32, page: u32) -> Vec<Op> {
    let Some(mut sheet) = get(cpu, id) else {
        return Vec::new();
    };
    for p in sheet.pages.iter_mut().filter(|p| p.hwnd == page) {
        p.dirty = true;
    }
    let mut ops = Vec::new();
    if sheet.has_apply {
        let apply = item(cpu, sheet.hwnd, IDC_APPLY_BUTTON);
        ops.push(user("EnableWindow", &[apply, 1]));
    }
    put(cpu, id, sheet);
    ops
}

/// PROPSHEET_UnChanged: the page marked clean, Apply disabled when no
/// page is dirty.
fn unchanged(cpu: &mut Cpu, id: u32, page: u32) -> Vec<Op> {
    let Some(mut sheet) = get(cpu, id) else {
        return Vec::new();
    };
    for p in sheet.pages.iter_mut().filter(|p| p.hwnd == page) {
        p.dirty = false;
    }
    let mut ops = Vec::new();
    if !sheet.pages.iter().any(|p| p.dirty) {
        let apply = item(cpu, sheet.hwnd, IDC_APPLY_BUTTON);
        ops.push(user("EnableWindow", &[apply, 0]));
    }
    put(cpu, id, sheet);
    ops
}

// The buttons.

/// PROPSHEET_Back and PROPSHEET_Next: PSN_WIZBACK or PSN_WIZNEXT to the
/// page, which may name the page to go to or refuse (-1), then
/// PSN_KILLACTIVE, then the page.
fn wiz_step(cpu: &mut Cpu, id: u32, back: bool) -> Vec<Op> {
    let Some(sheet) = get(cpu, id) else {
        return Vec::new();
    };
    if sheet.active < 0 {
        return Vec::new();
    }
    let code = if back { PSN_WIZBACK } else { PSN_WIZNEXT };
    let mut ops = psn(sheet.hwnd, page_hwnd(&sheet, sheet.active), code, 0);
    ops.push(then_op(if back { T_BACK_R } else { T_NEXT_R }, &[id]));
    ops
}

fn wiz_step_result(cpu: &mut Cpu, id: u32, c: &Ctx, back: bool) -> Vec<Op> {
    let Some(sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let index = match c.last {
        u32::MAX => return Vec::new(),
        0 if back => sheet.active - 1,
        0 => sheet.active + 1,
        res => find_by_res_id(cpu, &sheet, res),
    };
    if index < 0 || index as usize >= sheet.pages.len() {
        return Vec::new();
    }
    let next = if back { T_BACK_K } else { T_NEXT_K };
    kill_active(&sheet, next, &[id, index as u32])
}

fn wiz_step_go(cpu: &mut Cpu, id: u32, c: &Ctx, back: bool) -> Vec<Op> {
    let Some(sheet) = get(cpu, id) else {
        return Vec::new();
    };
    if refused(c, 2) {
        return Vec::new();
    }
    let button = if back {
        IDC_BACK_BUTTON
    } else {
        IDC_NEXT_BUTTON
    };
    let control = item(cpu, sheet.hwnd, button);
    let mut ops = vec![
        user("SetFocus", &[control]),
        send(sheet.hwnd, DM_SETDEFID, button, Arg::V(0)),
    ];
    let skipdir = if back { -1 } else { 1 };
    ops.extend(set_cur_sel(id, c.word(1) as i32, skipdir, 0));
    ops
}

/// PROPSHEET_Finish: PSN_WIZFINISH, which the page may refuse.
fn finish(cpu: &mut Cpu, id: u32) -> Vec<Op> {
    let Some(sheet) = get(cpu, id) else {
        return Vec::new();
    };
    if sheet.active < 0 {
        return Vec::new();
    }
    let page = page_hwnd(&sheet, sheet.active);
    let mut ops = psn(sheet.hwnd, page, PSN_WIZFINISH, 0);
    ops.push(then_op(T_FINISH_R, &[id]));
    ops
}

/// The sheet is done with: a modeless one's PSM_GETCURRENTPAGEHWND
/// gives 0 from now, a modal one's loop ends.
fn close(sheet: &mut Sheet) {
    if sheet.modeless {
        sheet.active_valid = false;
    } else {
        sheet.ended = true;
    }
}

fn finish_result(cpu: &mut Cpu, id: u32, c: &Ctx) -> Vec<Op> {
    let Some(mut sheet) = get(cpu, id) else {
        return Vec::new();
    };
    if c.last == 0 {
        if sheet.result == 0 {
            sheet.result = IDOK as i32;
        }
        close(&mut sheet);
        put(cpu, id, sheet);
    }
    Vec::new()
}

/// PROPSHEET_Apply(lparam), then `tail`: what PSM_APPLY (0), OK or
/// Apply does with the outcome.
fn apply(id: u32, lparam: u32, tail: u32) -> Vec<Op> {
    vec![then_op(T_APPLY, &[id, lparam, tail])]
}

fn apply_begin(cpu: &mut Cpu, id: u32, c: &Ctx) -> Vec<Op> {
    let Some(sheet) = get(cpu, id) else {
        return Vec::new();
    };
    if sheet.active < 0 {
        return vec![then_op(T_APPLY_TAIL, &[id, c.word(2), 0])];
    }
    let page = page_hwnd(&sheet, sheet.active);
    let mut ops = psn(sheet.hwnd, page, PSN_KILLACTIVE, 0);
    ops.push(then_op(T_APPLY_K, &c.data));
    ops
}

/// PSN_APPLY to the pages from `from` on that have their dialogs.
fn apply_from(cpu: &mut Cpu, id: u32, from: usize, lparam: u32, tail: u32) -> Vec<Op> {
    let Some(sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let next = (from..sheet.pages.len()).find(|&i| sheet.pages[i].hwnd != 0);
    let Some(i) = next else {
        // All took it.
        let mut ops = Vec::new();
        let mut sheet = sheet;
        if lparam != 0 {
            sheet.active_valid = false;
        } else if sheet.active >= 0 {
            let page = page_hwnd(&sheet, sheet.active);
            ops.extend(psn(sheet.hwnd, page, PSN_SETACTIVE, 0));
        }
        put(cpu, id, sheet);
        ops.push(then_op(T_APPLY_TAIL, &[id, tail, 1]));
        return ops;
    };
    let mut ops = psn(sheet.hwnd, sheet.pages[i].hwnd, PSN_APPLY, lparam);
    ops.push(then_op(T_APPLY_I, &[id, i as u32, lparam, tail]));
    ops
}

fn apply_page_result(cpu: &mut Cpu, id: u32, c: &Ctx) -> Vec<Op> {
    let (i, lparam, tail) = (c.word(1), c.word(2), c.word(3));
    match c.last {
        PSNRET_INVALID => {
            let mut ops = show_page(id, i as i32);
            ops.push(then_op(T_APPLY_TAIL, &[id, tail, 0]));
            ops
        }
        PSNRET_INVALID_NOCHANGEPAGE => vec![then_op(T_APPLY_TAIL, &[id, tail, 0])],
        _ => apply_from(cpu, id, i as usize + 1, lparam, tail),
    }
}

fn apply_tail(cpu: &mut Cpu, id: u32, c: &Ctx) -> Vec<Op> {
    let Some(mut sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let (tail, ok) = (c.word(1), c.word(2) != 0);
    match tail {
        0 => set_result(cpu, sheet.hwnd, ok as u32),
        IDOK if ok => {
            if sheet.result == 0 {
                sheet.result = IDOK as i32;
            }
            close(&mut sheet);
            put(cpu, id, sheet);
        }
        IDC_APPLY_BUTTON if ok => {
            let apply = item(cpu, sheet.hwnd, IDC_APPLY_BUTTON);
            return vec![user("EnableWindow", &[apply, 0])];
        }
        _ => {}
    }
    Vec::new()
}

/// PROPSHEET_Cancel(lparam): PSN_QUERYCANCEL, which the page may
/// refuse, then PSN_RESET to all pages.
fn cancel(cpu: &mut Cpu, id: u32, lparam: u32) -> Vec<Op> {
    let Some(sheet) = get(cpu, id) else {
        return Vec::new();
    };
    if sheet.active < 0 {
        return Vec::new();
    }
    let page = page_hwnd(&sheet, sheet.active);
    let mut ops = psn(sheet.hwnd, page, PSN_QUERYCANCEL, 0);
    ops.push(then_op(T_CANCEL_Q, &[id, lparam]));
    ops
}

fn cancel_result(cpu: &mut Cpu, id: u32, c: &Ctx) -> Vec<Op> {
    let Some(mut sheet) = get(cpu, id) else {
        return Vec::new();
    };
    if c.last != 0 {
        return Vec::new();
    }
    let mut ops = Vec::new();
    for p in sheet.pages.iter().filter(|p| p.hwnd != 0) {
        ops.extend(psn(sheet.hwnd, p.hwnd, PSN_RESET, c.word(1)));
    }
    close(&mut sheet);
    put(cpu, id, sheet);
    ops
}

/// PROPSHEET_Help: PSN_HELP to the page.
fn help(cpu: &mut Cpu, id: u32) -> Vec<Op> {
    let Some(sheet) = get(cpu, id) else {
        return Vec::new();
    };
    if sheet.active < 0 {
        return Vec::new();
    }
    psn(sheet.hwnd, page_hwnd(&sheet, sheet.active), PSN_HELP, 0)
}

/// PROPSHEET_DoCommand: what a button does, or None for another
/// command.
fn do_command(id: u32, command: u32) -> Option<Vec<Op>> {
    Some(match command {
        IDOK => apply(id, 1, IDOK),
        IDC_APPLY_BUTTON => apply(id, 0, IDC_APPLY_BUTTON),
        IDC_BACK_BUTTON => vec![then_op(T_BACK, &[id])],
        IDC_NEXT_BUTTON => vec![then_op(T_NEXT, &[id])],
        IDC_FINISH_BUTTON => vec![then_op(T_FINISH, &[id])],
        IDCANCEL => vec![then_op(T_CANCEL, &[id, 0])],
        IDHELP => vec![then_op(T_HELP, &[id])],
        _ => return None,
    })
}

/// PROPSHEET_RemovePage, after the page that replaces the active one
/// is selected: the page's dialog destroyed, its tab removed.
fn remove(cpu: &mut Cpu, id: u32, c: &Ctx) -> Vec<Op> {
    let Some(mut sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let (index, active_after) = (c.word(1) as usize, c.word(2) as i32);
    if index >= sheet.pages.len() {
        return Vec::new();
    }
    if active_after != -1 {
        sheet.active = active_after;
    }
    let page = sheet.pages.remove(index);
    let mut ops = Vec::new();
    if page.hwnd != 0 {
        ops.push(user("DestroyWindow", &[page.hwnd]));
    }
    ops.extend(destroy_page(cpu, page.hpage));
    let tab = item(cpu, sheet.hwnd, IDC_TABCONTROL);
    ops.push(send(tab, TCM_DELETEITEM, index as u32, Arg::V(0)));
    put(cpu, id, sheet);
    ops
}

fn remove_page(cpu: &mut Cpu, id: u32, index: i32, hpage: u32) -> Vec<Op> {
    let Some(mut sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let index = if hpage != 0 {
        page_index(&sheet, hpage)
    } else {
        index
    };
    let n = sheet.pages.len() as i32;
    if index < 0 || index >= n {
        return Vec::new();
    }
    let mut ops = Vec::new();
    let mut active_after = -1;
    if index == sheet.active {
        if n > 1 {
            if index > 0 {
                ops.extend(set_cur_sel(id, index - 1, -1, 0));
            } else {
                ops.extend(set_cur_sel(id, index + 1, 1, 0));
                active_after = index;
            }
        } else {
            sheet.active = -1;
            if !sheet.modeless {
                sheet.ended = true;
                put(cpu, id, sheet);
                return Vec::new();
            }
        }
    } else if index < sheet.active {
        sheet.active -= 1;
    }
    put(cpu, id, sheet);
    ops.push(then_op(T_REMOVE, &[id, index as u32, active_after as u32]));
    ops
}

/// PSM_ADDPAGE: the page at the end, made at once with PSP_PREMATURE,
/// selected when it is the only one.
fn add_page(cpu: &mut Cpu, id: u32, hwnd: u32, hpage: u32) -> Vec<Op> {
    let Some(mut sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let Some(page) = collect_page(cpu, &mut sheet, hpage, false) else {
        set_result(cpu, hwnd, 0);
        return Vec::new();
    };
    sheet.pages.push(page);
    let index = sheet.pages.len() as i32 - 1;
    let mut ops = Vec::new();
    if cpu.bus.guest_read_32(hpage + 4) & PSP_PREMATURE != 0 {
        ops.extend(create_page(cpu, &sheet, id, index));
    }
    ops.push(then_op(T_ADD_TAB, &[id, index as u32]));
    if sheet.pages.len() == 1 {
        ops.extend(set_cur_sel(id, 0, 1, 0));
    }
    put(cpu, id, sheet);
    set_result(cpu, hwnd, 1);
    ops
}

/// The sheet's window is going: its pages too (PROPSHEET_CleanUp).
fn clean_up(cpu: &mut Cpu, hwnd: u32, id: u32) -> Vec<Op> {
    state(cpu).propsheet.windows.remove(&hwnd);
    let Some(mut sheet) = get(cpu, id) else {
        return Vec::new();
    };
    let mut ops = Vec::new();
    for p in std::mem::take(&mut sheet.pages) {
        if p.hwnd != 0 {
            ops.push(user("DestroyWindow", &[p.hwnd]));
        }
        ops.extend(destroy_page(cpu, p.hpage));
    }
    sheet.destroyed = true;
    if sheet.modeless {
        state(cpu).propsheet.sheets.remove(&id);
    } else {
        put(cpu, id, sheet);
    }
    ops
}

/// The continuations of the module's scripts.
pub fn then(cpu: &mut Cpu, t: u16, c: &mut Ctx) -> Vec<Op> {
    let id = c.word(0);
    match t {
        T_CREATED => created(cpu, c, id),
        T_LOOP => run_loop(cpu, c, id),
        T_GOT => got_message(cpu, c, id),
        T_DISPATCHED => dispatched(c, id),
        T_DONE => {
            state(cpu).propsheet.sheets.remove(&id);
            Vec::new()
        }
        T_FREE_PAGE => {
            heap_release(cpu, id);
            Vec::new()
        }
        T_INIT_TABS => init_tabs(cpu, id, c),
        T_INIT_LAYOUT => init_layout(cpu, id),
        T_INIT_END => init_end(cpu, id),
        T_SC_BEGIN => sc_begin(cpu, id, c),
        T_SC_PLACE => sc_place(cpu, id, c),
        T_SC_RESULT => sc_result(cpu, id, c),
        T_SHOW => show(cpu, id, c),
        T_PAGE_CREATED => page_created(cpu, id, c),
        T_BACK => wiz_step(cpu, id, true),
        T_NEXT => wiz_step(cpu, id, false),
        T_BACK_R => wiz_step_result(cpu, id, c, true),
        T_NEXT_R => wiz_step_result(cpu, id, c, false),
        T_BACK_K => wiz_step_go(cpu, id, c, true),
        T_NEXT_K => wiz_step_go(cpu, id, c, false),
        T_FINISH => finish(cpu, id),
        T_FINISH_R => finish_result(cpu, id, c),
        T_APPLY => apply_begin(cpu, id, c),
        T_APPLY_K => {
            if c.last != 0 {
                return vec![then_op(T_APPLY_TAIL, &[id, c.word(2), 0])];
            }
            apply_from(cpu, id, 0, c.word(1), c.word(2))
        }
        T_APPLY_I => apply_page_result(cpu, id, c),
        T_APPLY_TAIL => apply_tail(cpu, id, c),
        T_CANCEL => cancel(cpu, id, c.word(1)),
        T_CANCEL_Q => cancel_result(cpu, id, c),
        T_HELP => help(cpu, id),
        T_SELCHANGING => {
            set_result(cpu, c.hwnd, refused(c, 1) as u32);
            Vec::new()
        }
        T_PSM_SETCUR => {
            let Some(sheet) = get(cpu, id) else {
                return Vec::new();
            };
            if refused(c, 3) {
                set_result(cpu, c.hwnd, 0);
                return Vec::new();
            }
            let (index, hpage) = (c.word(1) as i32, c.word(2));
            let index = if hpage != 0 {
                page_index(&sheet, hpage)
            } else {
                index
            };
            let valid = index >= 0 && (index as usize) < sheet.pages.len();
            set_result(cpu, c.hwnd, valid as u32);
            set_cur_sel(id, index, 1, 0)
        }
        T_SETCURSELID => {
            if refused(c, 2) {
                return Vec::new();
            }
            set_cur_sel(id, c.word(1) as i32, 1, 0)
        }
        T_QUERY => {
            let Some(sheet) = get(cpu, id) else {
                return Vec::new();
            };
            let (i, wp, lp) = (c.word(1) as usize, c.word(2), c.word(3));
            if i > 0 && c.last != 0 {
                set_result(cpu, c.hwnd, c.last);
                return Vec::new();
            }
            let next = (i..sheet.pages.len()).find(|&k| sheet.pages[k].hwnd != 0);
            let Some(k) = next else {
                set_result(cpu, c.hwnd, 0);
                return Vec::new();
            };
            vec![
                send(sheet.pages[k].hwnd, PSM_QUERYSIBLINGS, wp, Arg::V(lp)),
                then_op(T_QUERY, &[id, k as u32 + 1, wp, lp]),
            ]
        }
        T_ADD_TAB => {
            let Some(sheet) = get(cpu, id) else {
                return Vec::new();
            };
            let index = c.word(1);
            let text = sheet
                .pages
                .get(index as usize)
                .map(|p| p.text.clone())
                .unwrap_or_default();
            let tab = item(cpu, sheet.hwnd, IDC_TABCONTROL);
            insert_tab(c, tab, index + 1, &text)
        }
        T_REMOVE => remove(cpu, id, c),
        T_ISDLG => {
            let Some(sheet) = get(cpu, id) else {
                return Vec::new();
            };
            let (msg, key, shift) = (c.word(1), c.word(2), c.word(3) != 0);
            let delta: i32 = if c.last & DLGC_WANTMESSAGE != 0 {
                0
            } else {
                match key {
                    VK_TAB if shift => -1,
                    VK_TAB | VK_NEXT => 1,
                    VK_PRIOR => -1,
                    _ => 0,
                }
            };
            if delta == 0 {
                return is_dialog_message(sheet.hwnd, id, msg);
            }
            set_result(cpu, c.hwnd, 1);
            kill_active(&sheet, T_ISDLG_K, &[id, delta as u32])
        }
        T_ISDLG_K => {
            let Some(sheet) = get(cpu, id) else {
                return Vec::new();
            };
            if refused(c, 2) {
                return Vec::new();
            }
            let n = sheet.pages.len() as i32;
            let mut page = sheet.active + c.word(1) as i32;
            if page < 0 {
                page = n - 1;
            } else if page >= n {
                page = 0;
            }
            set_cur_sel(id, page, 1, 0)
        }
        T_RESULT_LAST => {
            set_result(cpu, c.hwnd, c.last);
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn is_dialog_message(hwnd: u32, id: u32, msg: u32) -> Vec<Op> {
    vec![
        user("IsDialogMessageA", &[hwnd, msg]),
        then_op(T_RESULT_LAST, &[id]),
    ]
}

/// The sheet's dialog procedure (PROPSHEET_DialogProc).
pub fn dlg_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        return Some(0);
    };
    if msg == WM_INITDIALOG {
        return init(cpu, hwnd, lp);
    }
    let Some(&id) = state(cpu).propsheet.windows.get(&hwnd) else {
        return Some(0);
    };
    let Some(sheet) = get(cpu, id) else {
        return Some(0);
    };
    let mut s = Script::new(hwnd);
    s.alloc(P_SIZE);
    s.result = 1;
    match msg {
        WM_DESTROY => s.ops = clean_up(cpu, hwnd, id),
        WM_CLOSE => {
            s.ops = vec![then_op(T_CANCEL, &[id, 1])];
            // DefDlgProc goes on to post IDCANCEL.
            s.result = 0;
        }
        WM_COMMAND => match do_command(id, wp & 0xFFFF) {
            Some(ops) => s.ops = ops,
            None => {
                if sheet.active_valid && sheet.active != -1 {
                    let page = page_hwnd(&sheet, sheet.active);
                    s.ops.push(send(page, WM_COMMAND, wp, Arg::V(lp)));
                }
            }
        },
        WM_NOTIFY => {
            let code = cpu.bus.guest_read_32(lp + 8);
            let from = cpu.bus.guest_read_32(lp);
            if cpu.bus.guest_faulted() {
                return None;
            }
            s.result = 0;
            if code == TCN_SELCHANGE {
                let index = tab::cur_sel(cpu, from);
                s.ops = set_cur_sel(id, index, 1, 0);
            } else if code == TCN_SELCHANGING {
                s.ops = kill_active(&sheet, T_SELCHANGING, &[id]);
                s.result = 1;
            }
        }
        PSM_GETCURRENTPAGEHWND => {
            let page = if sheet.active_valid && sheet.active != -1 {
                page_hwnd(&sheet, sheet.active)
            } else {
                0
            };
            set_result(cpu, hwnd, page);
        }
        PSM_CHANGED => s.ops = changed(cpu, id, wp),
        PSM_UNCHANGED => s.ops = unchanged(cpu, id, wp),
        PSM_GETTABCONTROL => {
            let tab = item(cpu, hwnd, IDC_TABCONTROL);
            set_result(cpu, hwnd, tab);
        }
        PSM_SETCURSEL => s.ops = kill_active(&sheet, T_PSM_SETCUR, &[id, wp, lp]),
        PSM_CANCELTOCLOSE => {
            let cancel = item(cpu, hwnd, IDCANCEL);
            let ok = item(cpu, hwnd, IDOK);
            s.ops.push(user("EnableWindow", &[cancel, 0]));
            s.ops.extend(set_text(ok, b"Close"));
            s.result = 0;
        }
        PSM_RESTARTWINDOWS | PSM_REBOOTSYSTEM => {
            let mut sheet = sheet;
            if msg == PSM_REBOOTSYSTEM {
                sheet.result = ID_PSREBOOTSYSTEM;
            } else if sheet.result != ID_PSREBOOTSYSTEM {
                sheet.result = ID_PSRESTARTWINDOWS;
            }
            put(cpu, id, sheet);
        }
        PSM_SETTITLEA => {
            let text = if lp >= 0x1_0000 {
                let t = user32::bytes_at(cpu, lp);
                if cpu.bus.guest_faulted() {
                    return None;
                }
                t
            } else {
                load_string(cpu, sheet.instance, lp)
            };
            if let Some(text) = text {
                s.ops = title_ops(hwnd, wp, &text);
            }
        }
        PSM_APPLY => s.ops = apply(id, 0, 0),
        PSM_QUERYSIBLINGS => s.ops = vec![then_op(T_QUERY, &[id, 0, wp, lp])],
        PSM_ADDPAGE => s.ops = add_page(cpu, id, hwnd, lp),
        PSM_REMOVEPAGE => s.ops = remove_page(cpu, id, wp as i32, lp),
        PSM_ISDIALOGMESSAGE => {
            let m = read_words(cpu, lp, 3);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let (target, message, key) = (m[0], m[1], m[2]);
            let ours = target == hwnd || user32::user(cpu).is_descendant(hwnd, target);
            if !ours {
                set_result(cpu, hwnd, 0);
            } else {
                let control = user32::input::get_key_state(cpu, &[VK_CONTROL]).unwrap_or(0);
                if message == WM_KEYDOWN && control & 0x8000 != 0 {
                    let shift = user32::input::get_key_state(cpu, &[VK_SHIFT]).unwrap_or(0);
                    let shift = (shift & 0x8000 != 0) as u32;
                    s.ops = vec![
                        send(target, WM_GETDLGCODE, 0, Arg::V(lp)),
                        then_op(T_ISDLG, &[id, lp, key, shift]),
                    ];
                } else {
                    s.ops = is_dialog_message(hwnd, id, lp);
                }
            }
        }
        PSM_PRESSBUTTON => {
            let command = match wp {
                PSBTN_BACK => IDC_BACK_BUTTON,
                PSBTN_NEXT => IDC_NEXT_BUTTON,
                PSBTN_FINISH => IDC_FINISH_BUTTON,
                PSBTN_OK => IDOK,
                PSBTN_APPLYNOW => IDC_APPLY_BUTTON,
                PSBTN_CANCEL => IDCANCEL,
                PSBTN_HELP => IDHELP,
                _ => 0,
            };
            s.ops = do_command(id, command).unwrap_or_default();
        }
        PSM_SETFINISHTEXTA => {
            let text = user32::bytes_at(cpu, lp);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let finish = item(cpu, hwnd, IDC_FINISH_BUTTON);
            s.ops = set_text(finish, &text.unwrap_or_default());
            s.ops.push(user("ShowWindow", &[finish, SW_SHOW]));
            s.ops.push(user("EnableWindow", &[finish, 1]));
            s.ops
                .push(send(hwnd, DM_SETDEFID, IDC_FINISH_BUTTON, Arg::V(0)));
            let back = item(cpu, hwnd, IDC_BACK_BUTTON);
            s.ops.push(user("ShowWindow", &[back, SW_HIDE]));
            if !sheet.has_finish {
                let next = item(cpu, hwnd, IDC_NEXT_BUTTON);
                s.ops.push(user("ShowWindow", &[next, SW_HIDE]));
            }
        }
        PSM_SETWIZBUTTONS => s.ops = wiz_buttons(cpu, &sheet, lp),
        PSM_SETCURSELID => {
            let index = find_by_res_id(cpu, &sheet, lp);
            if (index as usize) < sheet.pages.len() {
                s.ops = kill_active(&sheet, T_SETCURSELID, &[id, index as u32]);
            }
        }
        // The messages from here on are Internet Explorer 4's (COMCTL32
        // 4.71, Windows 98's), which Windows 95's sheets don't answer.
        PSM_HWNDTOINDEX => {
            let index = sheet
                .pages
                .iter()
                .position(|p| p.hwnd == wp)
                .map_or(-1, |i| i as i32);
            set_result(cpu, hwnd, index as u32);
        }
        PSM_INDEXTOHWND => set_result(cpu, hwnd, page_hwnd(&sheet, wp as i32)),
        PSM_PAGETOINDEX => set_result(cpu, hwnd, page_index(&sheet, wp) as u32),
        PSM_INDEXTOPAGE => {
            let page = sheet.pages.get(wp as usize).map_or(0, |p| p.hpage);
            set_result(cpu, hwnd, page);
        }
        PSM_IDTOINDEX => {
            let index = find_by_res_id(cpu, &sheet, lp);
            let index = if (index as usize) < sheet.pages.len() {
                index
            } else {
                -1
            };
            set_result(cpu, hwnd, index as u32);
        }
        PSM_INDEXTOID => {
            let id_of = match sheet.pages.get(wp as usize) {
                Some(p) => {
                    let w = read_words(cpu, p.hpage, 4);
                    if w[1] & PSP_DLGINDIRECT != 0 || w[3] >= 0x1_0000 {
                        0
                    } else {
                        w[3]
                    }
                }
                None => 0,
            };
            set_result(cpu, hwnd, id_of);
        }
        PSM_GETRESULT => set_result(cpu, hwnd, sheet.result as u32),
        PSM_INSERTPAGE | PSM_RECALCPAGESIZES => set_result(cpu, hwnd, 0),
        PSM_SETHEADERTITLEA | PSM_SETHEADERSUBTITLEA => {}
        _ => return Some(0),
    }
    script::run(cpu, PROPSHEET, s)
}
