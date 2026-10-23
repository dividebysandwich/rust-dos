//! The tab control (SysTabControl32): a row of tabs, or of buttons with
//! TCS_BUTTONS, over a display area. Each tab has a label, an image of
//! the control's image list, a state and bytes of the program's own
//! (TCM_SETITEMEXTRA). With TCS_MULTILINE the tabs wrap into rows, the
//! row of the selected tab next to the display area; without it a row
//! too long for the control scrolls, with an up-down control at its
//! right end. TCM_ADJUSTRECT converts between the window and the
//! display area, which property sheets lay their pages out with.
//!
//! The parent hears of a new selection with TCN_SELCHANGING, which may
//! refuse it, then TCN_SELCHANGE. As Windows' COMCTL32, the control
//! keeps no non-client area (WM_NCCALCSIZE leaves the whole window to
//! the client), and asks its parent for WM_NOTIFYFORMAT as it is made.
//!
//! Ported from Wine's tab.c:
//!
//! Copyright 1998 Anders Carlsson
//! Copyright 1999 Alex Priem <alexp@sci.kun.nl>
//! Copyright 1999 Francis Beaudet
//! Copyright 2003 Vitaliy Margolen
//!
//! This library is free software; you can redistribute it and/or modify
//! it under the terms of the GNU Lesser General Public License as
//! published by the Free Software Foundation; either version 2.1 of the
//! License, or (at your option) any later version.

use super::gfx::{
    self, COLOR_BTNFACE, COLOR_BTNTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_SCROLLBAR,
};
use super::script::{self, Arg, Ctx, Op, Script};
use super::{DRAW, TAB, imagelist, state};
use crate::cpu::Cpu;
use crate::win32::gdi32::Rect;
use crate::win32::user32::message::{
    WM_CREATE, WM_DRAWITEM, WM_GETDLGCODE, WM_GETFONT, WM_HSCROLL, WM_KEYDOWN, WM_KILLFOCUS,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCALCSIZE, WM_NCDESTROY, WM_NCHITTEST,
    WM_NOTIFY, WM_NOTIFYFORMAT, WM_PAINT, WM_PRINTCLIENT, WM_RBUTTONDOWN, WM_SETFOCUS, WM_SETFONT,
    WM_SETREDRAW, WM_SIZE, WM_STYLECHANGED, WM_TIMER,
};
use crate::win32::user32::window::{
    CW_USEDEFAULT, SW_HIDE, SWP_NOZORDER, SWP_SHOWWINDOW, WS_CHILD, WS_CLIPSIBLINGS, WS_POPUP,
    WS_VISIBLE,
};
use crate::win32::user32::{self, defwnd, read_words, sysparams, write_words};
use std::collections::BTreeMap;

const TCM_GETIMAGELIST: u32 = 0x1302;
const TCM_SETIMAGELIST: u32 = 0x1303;
const TCM_GETITEMCOUNT: u32 = 0x1304;
const TCM_GETITEMA: u32 = 0x1305;
const TCM_SETITEMA: u32 = 0x1306;
const TCM_INSERTITEMA: u32 = 0x1307;
const TCM_DELETEITEM: u32 = 0x1308;
const TCM_DELETEALLITEMS: u32 = 0x1309;
const TCM_GETITEMRECT: u32 = 0x130A;
const TCM_GETCURSEL: u32 = 0x130B;
const TCM_SETCURSEL: u32 = 0x130C;
const TCM_HITTEST: u32 = 0x130D;
const TCM_SETITEMEXTRA: u32 = 0x130E;
const TCM_ADJUSTRECT: u32 = 0x1328;
const TCM_SETITEMSIZE: u32 = 0x1329;
const TCM_REMOVEIMAGE: u32 = 0x132A;
const TCM_SETPADDING: u32 = 0x132B;
const TCM_GETROWCOUNT: u32 = 0x132C;
const TCM_GETTOOLTIPS: u32 = 0x132D;
const TCM_SETTOOLTIPS: u32 = 0x132E;
const TCM_GETCURFOCUS: u32 = 0x132F;
const TCM_SETCURFOCUS: u32 = 0x1330;
const TCM_SETMINTABWIDTH: u32 = 0x1331;
const TCM_DESELECTALL: u32 = 0x1332;
const TCM_HIGHLIGHTITEM: u32 = 0x1333;
const TCM_SETEXTENDEDSTYLE: u32 = 0x1334;
const TCM_GETEXTENDEDSTYLE: u32 = 0x1335;
const TCM_GETITEMW: u32 = 0x133C;
const TCM_SETITEMW: u32 = 0x133D;
const TCM_INSERTITEMW: u32 = 0x133E;
const TCM_SETUNICODEFORMAT: u32 = 0x2005;
const TCM_GETUNICODEFORMAT: u32 = 0x2006;

pub const TCS_BOTTOM: u32 = 0x0002;
pub const TCS_MULTISELECT: u32 = 0x0004;
pub const TCS_FLATBUTTONS: u32 = 0x0008;
pub const TCS_FORCEICONLEFT: u32 = 0x0010;
pub const TCS_FORCELABELLEFT: u32 = 0x0020;
pub const TCS_HOTTRACK: u32 = 0x0040;
pub const TCS_VERTICAL: u32 = 0x0080;
pub const TCS_BUTTONS: u32 = 0x0100;
pub const TCS_MULTILINE: u32 = 0x0200;
pub const TCS_FIXEDWIDTH: u32 = 0x0400;
pub const TCS_RAGGEDRIGHT: u32 = 0x0800;
pub const TCS_OWNERDRAWFIXED: u32 = 0x2000;
pub const TCS_TOOLTIPS: u32 = 0x4000;
pub const TCS_FOCUSNEVER: u32 = 0x8000;

const TCS_EX_FLATSEPARATORS: u32 = 0x0001;

const TCIF_TEXT: u32 = 0x0001;
const TCIF_IMAGE: u32 = 0x0002;
const TCIF_PARAM: u32 = 0x0008;
const TCIF_STATE: u32 = 0x0010;
const TCIS_BUTTONPRESSED: u32 = 0x0001;
const TCIS_HIGHLIGHTED: u32 = 0x0002;
const TCHT_NOWHERE: u32 = 0x0001;
const TCHT_ONITEM: u32 = 0x0006;

const TCN_KEYDOWN: u32 = (-550i32) as u32;
pub const TCN_SELCHANGE: u32 = (-551i32) as u32;
pub const TCN_SELCHANGING: u32 = (-552i32) as u32;
const TCN_FOCUSCHANGE: u32 = (-554i32) as u32;
const NM_CLICK: u32 = (-2i32) as u32;
const NM_RCLICK: u32 = (-5i32) as u32;
const NM_TOOLTIPSCREATED: u32 = (-19i32) as u32;

const TTM_RELAYEVENT: u32 = 0x0407;
const UDM_SETPOS: u32 = 0x0467;
const UDM_SETRANGE32: u32 = 0x046F;
const UDS_HORZ: u32 = 0x0040;
const NF_QUERY: u32 = 3;
const GWL_STYLE: u32 = (-16i32) as u32;

const ODT_TAB: u32 = 101;
const ODA_DRAWENTIRE: u32 = 1;
const ODS_SELECTED: u32 = 0x0001;
const ODS_FOCUS: u32 = 0x0010;

const WVR_ALIGNTOP: u32 = 0x0010;
const HTTRANSPARENT: u32 = u32::MAX;
const HTCLIENT: u32 = 1;
const DLGC_WANTARROWS: u32 = 0x0001;
const DLGC_WANTCHARS: u32 = 0x0080;
const VK_LEFT: u32 = 0x25;
const VK_RIGHT: u32 = 0x27;
const MK_CONTROL: u32 = 0x0008;
const SB_THUMBPOSITION: u32 = 4;
const SM_CXHSCROLL: u32 = 21;
const SM_CYHSCROLL: u32 = 3;
const LOGPIXELSX: u32 = 88;
const SYSTEM_FONT: u32 = 13;
const TRANSPARENT: u32 = 1;
const DT_LEFT: u32 = 0;
const DT_SINGLELINE: u32 = 0x20;
const DT_CALCRECT: u32 = 0x400;
const RDW_INVALIDATE: u32 = 0x0001;
const RDW_ERASE: u32 = 0x0004;
const RDW_UPDATENOW: u32 = 0x0100;

const EDGE_RAISED: u32 = 0x0005;
const EDGE_SUNKEN: u32 = 0x000A;
const EDGE_ETCHED: u32 = 0x0006;
const BDR_RAISEDINNER: u32 = 0x0004;
const BF_LEFT: u32 = 0x0001;
const BF_TOP: u32 = 0x0002;
const BF_RIGHT: u32 = 0x0004;
const BF_BOTTOM: u32 = 0x0008;
const BF_RECT: u32 = 0x000F;
const BF_DIAGONAL: u32 = 0x0010;
const BF_SOFT: u32 = 0x1000;
const BF_DIAGONAL_ENDTOPRIGHT: u32 = BF_DIAGONAL | BF_TOP | BF_RIGHT;
const BF_DIAGONAL_ENDTOPLEFT: u32 = BF_DIAGONAL | BF_TOP | BF_LEFT;
const BF_DIAGONAL_ENDBOTTOMLEFT: u32 = BF_DIAGONAL | BF_BOTTOM | BF_LEFT;
const BF_DIAGONAL_ENDBOTTOMRIGHT: u32 = BF_DIAGONAL | BF_BOTTOM | BF_RIGHT;

const SELECTED_TAB_OFFSET: i32 = 2;
const ROUND_CORNER_SIZE: i32 = 2;
const DISPLAY_AREA_PADDINGX: i32 = 2;
const DISPLAY_AREA_PADDINGY: i32 = 2;
const CONTROL_BORDER_SIZEX: i32 = 2;
const CONTROL_BORDER_SIZEY: i32 = 2;
const BUTTON_SPACINGX: i32 = 3;
const BUTTON_SPACINGY: i32 = 3;
const FLAT_BTN_SPACINGX: i32 = 8;
const DEFAULT_MIN_TAB_WIDTH: i32 = 54;
const DEFAULT_PADDING_X: i32 = 6;
const EXTRA_ICON_PADDING: i32 = 3;

const HOTTRACK_TIMER: u32 = 1;
const HOTTRACK_INTERVAL: u32 = 100;

/// The scratch space of the control's scripts: a notification (up to
/// an NMTCKEYDOWN), a class name, a MSG for the tooltip, a
/// DRAWITEMSTRUCT, a PAINTSTRUCT.
const S_NM: u32 = 0;
const S_CLASS: u32 = 24;
const S_MSG: u32 = 44;
const S_DIS: u32 = 72;
const S_PS: u32 = 120;
const S_SIZE: u32 = 184;

// The continuations.
const T_SELCHANGING: u16 = 1;
const T_CLICK: u16 = 2;
const T_UPDOWN: u16 = 3;
const T_TOOLTIP: u16 = 4;

/// A tab.
#[derive(Clone, Debug, Default)]
pub struct Item {
    pub state: u32,
    pub text: Option<Vec<u8>>,
    pub image: i32,
    /// Its place as Wine keeps it: from the left end of its row, with
    /// the row in `top` (row 0 farthest from the display area); turned
    /// for TCS_VERTICAL, the column in `left` and the place in `top`
    /// and `bottom`.
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    /// The program's bytes (TCM_SETITEMEXTRA), lParam by default.
    pub extra: Vec<u8>,
}

crate::state_fields!(Item {
    state,
    text,
    image,
    left,
    top,
    right,
    bottom,
    extra
});

/// What a tab control keeps.
#[derive(Clone, Debug, Default)]
pub struct Tab {
    pub notify: u32,
    pub items: Vec<Item>,
    pub rows: i32,
    pub tab_height: i32,
    pub tab_width: i32,
    pub min_width: i32,
    pub hpad: i32,
    pub vpad: i32,
    pub hpad_set: i32,
    pub vpad_set: i32,
    pub font: u32,
    pub himl: u32,
    pub tooltip: u32,
    pub leftmost: i32,
    pub selected: i32,
    pub hot: i32,
    pub focus: i32,
    pub redraw: bool,
    pub scrolling: bool,
    pub height_set: bool,
    pub unicode: bool,
    pub updown: u32,
    pub cb_info: i32,
    pub ex_style: u32,
    pub style: u32,
}

crate::state_fields!(Tab {
    notify,
    items,
    rows,
    tab_height,
    tab_width,
    min_width,
    hpad,
    vpad,
    hpad_set,
    vpad_set,
    font,
    himl,
    tooltip,
    leftmost,
    selected,
    hot,
    focus,
    redraw,
    scrolling,
    height_set,
    unicode,
    updown,
    cb_info,
    ex_style,
    style
});

/// What the module keeps, in COMCTL32's `State`.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub tabs: BTreeMap<u32, Tab>,
}

crate::state_fields!(State { tabs });

fn get(cpu: &mut Cpu, hwnd: u32) -> Option<Tab> {
    state(cpu).tab.tabs.get(&hwnd).cloned()
}

fn put(cpu: &mut Cpu, hwnd: u32, t: Tab) {
    state(cpu).tab.tabs.insert(hwnd, t);
}

fn read_bytes(cpu: &mut Cpu, at: u32, n: usize) -> Vec<u8> {
    let mut b = vec![0; n];
    cpu.bus.guest_read_bytes(at, &mut b);
    b
}

fn send_op(hwnd: u32, msg: u32, wp: u32, lp: Arg) -> Op {
    Op::Call {
        target: user32::address_of("SendMessageA"),
        args: vec![Arg::V(hwnd), Arg::V(msg), Arg::V(wp), lp],
    }
}

fn call_op(name: &str, args: &[u32]) -> Op {
    Op::Call {
        target: user32::address_of(name),
        args: args.iter().map(|&a| Arg::V(a)).collect(),
    }
}

/// WM_NOTIFY with `code` (TAB_SendSimpleNotify), with `more` after the
/// NMHDR, in the scratch space's notification.
fn notify_ops(cpu: &mut Cpu, hwnd: u32, to: u32, code: u32, more: &[u32]) -> Vec<Op> {
    let id = super::id_of(cpu, hwnd);
    let mut words = vec![hwnd, id, code];
    words.extend_from_slice(more);
    vec![
        Op::Write { at: S_NM, words },
        send_op(to, WM_NOTIFY, id, Arg::S(S_NM)),
    ]
}

/// TAB_InternalGetItemRect: whether the tab `i` is in sight, its
/// rectangle in the client area, and that rectangle as it is drawn
/// selected.
fn item_rect(t: &Tab, client: Rect, i: i32) -> (bool, Rect, Rect) {
    let n = t.items.len() as i32;
    let wraps = t.style & (TCS_MULTILINE | TCS_VERTICAL) != 0;
    if n <= 0 || i < 0 || i >= n || (!wraps && i < t.leftmost) {
        return (false, Rect::new(0, 0, 0, t.tab_height), Rect::default());
    }
    let it = &t.items[i as usize];
    let mut r = Rect::new(it.left, it.top, it.right, it.bottom);
    let s = t.style;
    let buttons = s & TCS_BUTTONS != 0;
    let h = t.tab_height;
    if s & TCS_BOTTOM != 0 && s & TCS_VERTICAL != 0 {
        r.right = client.right
            - SELECTED_TAB_OFFSET
            - r.left * h
            - if buttons { r.left * BUTTON_SPACINGX } else { 0 };
        r.left = r.right - h;
    } else if s & TCS_VERTICAL != 0 {
        r.left = client.left
            + SELECTED_TAB_OFFSET
            + r.left * h
            + if buttons { r.left * BUTTON_SPACINGX } else { 0 };
        r.right = r.left + h;
    } else if s & TCS_BOTTOM != 0 {
        r.bottom = client.bottom
            - r.top * h
            - if buttons {
                r.top * BUTTON_SPACINGY
            } else {
                SELECTED_TAB_OFFSET
            };
        r.top = r.bottom - h;
    } else {
        r.top = client.top
            + r.top * h
            + if buttons {
                r.top * BUTTON_SPACINGY
            } else {
                SELECTED_TAB_OFFSET
            };
        r.bottom = r.top + h;
    }
    let first = t.items.get(t.leftmost.max(0) as usize);
    if s & TCS_VERTICAL != 0 {
        let top = first.map_or(0, |f| f.top);
        r = r.offset(0, SELECTED_TAB_OFFSET - top);
    } else {
        let left = first.map_or(0, |f| f.left);
        r = r.offset(SELECTED_TAB_OFFSET - left, 0);
    }
    let mut sel = if s & TCS_VERTICAL != 0 {
        r.inflate(0, SELECTED_TAB_OFFSET)
    } else {
        r.inflate(SELECTED_TAB_OFFSET, 0)
    };
    if s & TCS_BOTTOM != 0 && s & TCS_VERTICAL != 0 {
        sel.left -= 2;
        sel.right += SELECTED_TAB_OFFSET;
    } else if s & TCS_VERTICAL != 0 {
        sel.left -= SELECTED_TAB_OFFSET;
        sel.right += 1;
    } else if s & TCS_BOTTOM != 0 {
        sel.bottom += SELECTED_TAB_OFFSET;
    } else {
        sel.top -= SELECTED_TAB_OFFSET;
        sel.bottom -= 1;
    }
    let visible = if s & TCS_VERTICAL != 0 {
        r.top < client.bottom && r.bottom > client.top
    } else {
        r.left < client.right && r.right > client.left
    };
    (visible, r, sel)
}

/// TAB_InternalHitTest: the tab at (x, y), or -1.
fn hit_test(t: &Tab, client: Rect, x: i32, y: i32) -> i32 {
    (0..t.items.len() as i32)
        .find(|&i| item_rect(t, client, i).1.contains(x, y))
        .unwrap_or(-1)
}

/// TCM_ADJUSTRECT on the tab control `hwnd`, for property sheets,
/// which lay their pages out with it: `r` unchanged for a window that
/// isn't one.
pub fn adjust(cpu: &mut Cpu, hwnd: u32, larger: bool, mut r: Rect) -> Rect {
    if let Some(t) = state(cpu).tab.tabs.get(&hwnd) {
        adjust_rect(t, larger, &mut r);
    }
    r
}

/// TCM_GETCURSEL of the tab control `hwnd`, or -1.
pub fn cur_sel(cpu: &mut Cpu, hwnd: u32) -> i32 {
    state(cpu).tab.tabs.get(&hwnd).map_or(-1, |t| t.selected)
}

/// TAB_AdjustRect on `r`: the display area of the window rectangle
/// `r`, or with `larger` the window rectangle of the display area.
fn adjust_rect(t: &Tab, larger: bool, r: &mut Rect) {
    let rows = t.rows;
    let gap = if t.style & TCS_BUTTONS != 0 {
        3 * (rows - 1)
    } else {
        0
    };
    let vertical = t.style & TCS_VERTICAL != 0;
    let bottom = t.style & TCS_BOTTOM != 0;
    let pad_x = DISPLAY_AREA_PADDINGX + CONTROL_BORDER_SIZEX;
    let pad_y = DISPLAY_AREA_PADDINGY + CONTROL_BORDER_SIZEY;
    let tabs = t.tab_height * rows;
    if larger {
        match (vertical, bottom) {
            (true, true) => r.right += tabs,
            (true, false) => r.left -= tabs + gap,
            (false, true) => r.bottom += tabs,
            (false, false) => r.top -= tabs + gap,
        }
        *r = r.inflate(pad_x, pad_y);
    } else {
        *r = r.inflate(-pad_x, -pad_y);
        match (vertical, bottom) {
            (true, true) => r.right -= tabs,
            (true, false) => r.left += tabs + gap,
            (false, true) => r.bottom -= tabs,
            (false, false) => r.top += tabs + gap,
        }
    }
}

/// The up-down control's place: at the right end of the tabs.
fn updown_rect(t: &Tab, client: Rect) -> Rect {
    let right = client.right;
    let left = right - 2 * sysparams::metric(SM_CXHSCROLL);
    let cy = sysparams::metric(SM_CYHSCROLL);
    if t.style & TCS_BOTTOM != 0 {
        let top = client.bottom - t.tab_height;
        Rect::new(left, top, right, top + cy)
    } else {
        let bottom = client.top + t.tab_height;
        Rect::new(left, bottom - cy, right, bottom)
    }
}

/// The up-down control's highest position: how many tabs are out of
/// sight on the left when the last one shows.
fn max_range(t: &Tab, client: Rect) -> i32 {
    let n = t.items.len() as i32;
    if !t.scrolling || n == 0 {
        return 0;
    }
    let ud = updown_rect(t, client);
    let vsize = client.right - (ud.width() + 1);
    let last = t.items[n as usize - 1].right;
    let mut range = n;
    while range > 0 {
        if last - t.items[range as usize - 1].left > vsize {
            break;
        }
        range -= 1;
    }
    if range == n { range - 1 } else { range }
}

/// TAB_SetupScrolling: show, place or hide the up-down control. One
/// that doesn't exist yet is made, and T_UPDOWN finishes the work.
fn setup_scrolling(t: &Tab, hwnd: u32, client: Rect) -> Vec<Op> {
    let mut ops = Vec::new();
    if t.scrolling {
        let r = updown_rect(t, client);
        if t.updown == 0 {
            ops.push(Op::Write {
                at: S_CLASS,
                words: script::words_of(b"msctls_updown32\0"),
            });
            ops.push(Op::Call {
                target: user32::address_of("CreateWindowExA"),
                args: [
                    0,
                    0,
                    0,
                    WS_VISIBLE | WS_CHILD | UDS_HORZ,
                    r.left as u32,
                    r.top as u32,
                    r.width() as u32,
                    r.height() as u32,
                    hwnd,
                    0,
                    0,
                    0,
                ]
                .iter()
                .enumerate()
                .map(|(k, &v)| if k == 1 { Arg::S(S_CLASS) } else { Arg::V(v) })
                .collect(),
            });
            ops.push(Op::Then {
                id: T_UPDOWN,
                data: vec![],
            });
            return ops;
        }
        ops.push(call_op(
            "SetWindowPos",
            &[
                t.updown,
                0,
                r.left as u32,
                r.top as u32,
                r.width() as u32,
                r.height() as u32,
                SWP_SHOWWINDOW | SWP_NOZORDER,
            ],
        ));
    } else if t.updown != 0 {
        ops.push(call_op("ShowWindow", &[t.updown, SW_HIDE]));
    }
    if t.updown != 0 {
        let range = max_range(t, client);
        ops.push(send_op(t.updown, UDM_SETRANGE32, 0, Arg::V(range as u32)));
    }
    ops
}

/// TAB_EnsureSelectionVisible: the selected tab's row goes next to the
/// display area, and a scrolled row scrolls to it.
fn ensure_selection_visible(cpu: &mut Cpu, hwnd: u32, t: &mut Tab) -> Vec<Op> {
    let sel = t.selected;
    let original = t.leftmost;
    let vertical = t.style & TCS_VERTICAL != 0;
    if t.rows > 1 && t.style & TCS_BUTTONS == 0 && sel >= 0 && (sel as usize) < t.items.len() {
        let row = if vertical {
            t.items[sel as usize].left
        } else {
            t.items[sel as usize].top
        };
        let target = t.rows - 1;
        if row != target {
            for it in t.items.iter_mut() {
                let r = if vertical { &mut it.left } else { &mut it.top };
                if *r == row {
                    *r = target;
                } else if *r > row {
                    *r -= 1;
                }
            }
            recalc_hot_track(cpu, hwnd, t, None);
        }
    }
    if !t.scrolling || t.updown == 0 || vertical {
        return Vec::new();
    }
    if sel >= 0 && (sel as usize) < t.items.len() {
        if t.leftmost >= sel {
            t.leftmost = sel;
        } else {
            let selected = t.items[sel as usize].clone();
            let width = super::client(cpu, hwnd).right - super::client(cpu, t.updown).right;
            if selected.right - selected.left >= width {
                t.leftmost = sel;
            } else {
                let mut i = t.leftmost.max(0);
                while (i as usize) < t.items.len() {
                    if selected.right - t.items[i as usize].left < width {
                        break;
                    }
                    i += 1;
                }
                t.leftmost = i;
            }
        }
    }
    if t.leftmost != original {
        recalc_hot_track(cpu, hwnd, t, None);
    }
    vec![send_op(
        t.updown,
        UDM_SETPOS,
        0,
        Arg::V(t.leftmost as u16 as u32),
    )]
}

/// TAB_SetItemBounds: where each tab goes, the tabs' height, the rows,
/// and whether the row scrolls.
fn set_item_bounds(cpu: &mut Cpu, hwnd: u32, t: &mut Tab) -> Vec<Op> {
    let hdc = user32::paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
    let font = if t.font != 0 {
        t.font
    } else {
        gfx::stock(cpu, SYSTEM_FONT)
    };
    let old_font = gfx::select(cpu, hdc, font);
    let mut client = super::client(cpu, hwnd);
    if t.style & TCS_VERTICAL != 0 {
        std::mem::swap(&mut client.bottom, &mut client.right);
    }
    t.hpad = t.hpad_set;
    t.vpad = t.vpad_set;
    let mut left = 0;
    let mut rows = if t.items.is_empty() { 0 } else { 1 };
    if !t.height_set {
        let text_height = gfx::font_height(cpu, hdc);
        let icon_height = if t.himl != 0 {
            imagelist::icon_size(cpu, t.himl).map_or(0, |s| s.1)
        } else {
            0
        };
        let item_height = if text_height > icon_height {
            text_height + 2
        } else {
            icon_height
        };
        let factor = if t.style & TCS_BUTTONS != 0 { 2 } else { 1 };
        t.tab_height = item_height + factor * t.vpad;
    }
    let mut icon_width = 0;
    if t.himl != 0 {
        icon_width = imagelist::icon_size(cpu, t.himl).map_or(0, |s| s.0);
        icon_width += if t.style & TCS_FIXEDWIDTH != 0 {
            4
        } else {
            t.hpad
        };
    }
    let wraps = t.style & (TCS_MULTILINE | TCS_VERTICAL) != 0;
    let default_min = DEFAULT_MIN_TAB_WIDTH - (DEFAULT_PADDING_X - t.hpad) * 2;
    for i in 0..t.items.len() {
        let width = if t.style & TCS_FIXEDWIDTH != 0 {
            t.tab_width.max(icon_width)
        } else if let Some(text) = t.items[i].text.clone() {
            let (cx, _) = gfx::text_size(cpu, hdc, &text);
            let w = cx + icon_width + 2 * t.hpad;
            if t.min_width < 0 {
                w.max(default_min)
            } else {
                w.max(t.min_width)
            }
        } else if t.min_width < 0 {
            default_min
        } else {
            let mut w = t.min_width;
            if t.himl != 0 && t.min_width > 0 && t.min_width < DEFAULT_MIN_TAB_WIDTH && t.hpad > 1 {
                w += EXTRA_ICON_PADDING * (t.hpad - 1);
            }
            w
        };
        let it = &mut t.items[i];
        it.left = left;
        it.right = left + width;
        if wraps && it.right > client.right - CONTROL_BORDER_SIZEX - DISPLAY_AREA_PADDINGX {
            it.right -= it.left;
            it.left = 0;
            rows += 1;
        }
        it.bottom = 0;
        it.top = rows - 1;
        left = if t.style & TCS_BUTTONS != 0 {
            it.right
                + BUTTON_SPACINGX
                + if t.style & TCS_FLATBUTTONS != 0 {
                    FLAT_BTN_SPACINGX
                } else {
                    0
                }
        } else {
            it.right
        };
    }
    if !wraps {
        t.scrolling = left + 2 * SELECTED_TAB_OFFSET > client.right;
        if !t.scrolling {
            t.leftmost = 0;
        }
    } else {
        t.scrolling = false;
        t.leftmost = 0;
    }
    let mut ops = setup_scrolling(t, hwnd, client);
    t.rows = rows;
    if t.style & TCS_RAGGEDRIGHT == 0 && wraps && !t.items.is_empty() && t.rows > 1 {
        arrange_rows(t, client);
    }
    if t.style & TCS_VERTICAL != 0 {
        // Turn the tabs a quarter round, to run down the side.
        for it in t.items.iter_mut() {
            let o = it.clone();
            it.top = o.left - client.left;
            it.bottom = it.top + (o.right - o.left);
            it.left = o.top;
            it.right = o.bottom;
        }
    }
    ops.extend(ensure_selection_visible(cpu, hwnd, t));
    recalc_hot_track(cpu, hwnd, t, None);
    gfx::select(cpu, hdc, old_font);
    user32::paint::release_dc(cpu, &[hwnd, hdc]);
    ops
}

/// Even out the rows of a control whose tabs wrap, and stretch each
/// row to the width of the control.
fn arrange_rows(t: &mut Tab, client: Rect) {
    let n = t.items.len() as i32;
    let per_row = n / t.rows;
    let rem = n % t.rows;
    let vertical = t.style & TCS_VERTICAL != 0;
    let (mut row, mut count, mut left) = (0, 0, 0);
    for it in t.items.iter_mut() {
        it.right -= it.left;
        it.left = 0;
        // Vertical tabs put the extra ones in the last rows, the
        // others in the first.
        let limit = if vertical {
            if row >= t.rows - rem {
                per_row + 1
            } else {
                per_row
            }
        } else if row < rem {
            per_row + 1
        } else {
            per_row
        };
        if count >= limit {
            row += 1;
            left = 0;
            count = 0;
        }
        it.left += left;
        it.right += left;
        it.top = row;
        left = if t.style & TCS_BUTTONS != 0 {
            it.right
                + 1
                + if t.style & TCS_FLATBUTTONS != 0 {
                    FLAT_BTN_SPACINGX
                } else {
                    0
                }
        } else {
            it.right
        };
        count += 1;
    }
    let mut start = 0usize;
    while start < t.items.len() {
        let row = t.items[start].top;
        let mut end = start;
        while end < t.items.len() && t.items[end].top == row {
            end += 1;
        }
        let mut diff = client.right - 2 * SELECTED_TAB_OFFSET - t.items[end - 1].right;
        let count = (end - start) as i32;
        if count > 1 {
            let remainder = diff % count;
            diff /= count;
            for (k, it) in t.items[start..end].iter_mut().enumerate() {
                let k = k as i32;
                it.left += k * diff;
                it.right += (k + 1) * diff;
            }
            t.items[end - 1].right += remainder;
        } else {
            t.items[start].left = client.left;
            t.items[start].right = client.right - 4;
        }
        start = end;
    }
}

/// TAB_RecalcHotTrack: which tab is under the mouse, with
/// TCS_HOTTRACK. The tabs to draw again: the one the mouse left and
/// the one it entered, or -1.
fn recalc_hot_track(cpu: &mut Cpu, hwnd: u32, t: &mut Tab, pos: Option<(i32, i32)>) -> (i32, i32) {
    let mut item = -1;
    if t.style & TCS_HOTTRACK != 0 {
        let (x, y) = pos.unwrap_or_else(|| {
            let st = user32::user(cpu);
            let (cx, cy) = st.input.cursor;
            let (ox, oy) = st.client_origin(hwnd);
            (cx - ox, cy - oy)
        });
        let client = super::client(cpu, hwnd);
        item = hit_test(t, client, x, y);
    }
    let (mut leave, mut enter) = (-1, -1);
    if item != t.hot {
        if t.hot >= 0 {
            leave = t.hot;
            if item < 0 {
                user32::message::kill_timer(cpu, &[hwnd, HOTTRACK_TIMER]);
            }
        } else {
            user32::message::set_timer(cpu, &[hwnd, HOTTRACK_TIMER, HOTTRACK_INTERVAL, 0]);
        }
        t.hot = item;
        if item >= 0 {
            enter = item;
        }
    }
    (leave, enter)
}

/// TAB_InvalidateTabArea. The whole control is drawn again, which
/// covers the tabs Wine works out.
fn invalidate_tabs(cpu: &mut Cpu, hwnd: u32, t: &Tab) {
    if t.rows < 1 {
        return;
    }
    super::invalidate(cpu, hwnd, true);
}

/// Draw the tab `i` again at once, through the window's DC, as the
/// mouse comes or goes.
fn hot_track_refresh(cpu: &mut Cpu, hwnd: u32, t: &Tab, i: i32) {
    if i < 0 || t.style & TCS_OWNERDRAWFIXED != 0 {
        return;
    }
    let hdc = user32::paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
    if hdc == 0 {
        return;
    }
    let mut ops = Vec::new();
    draw_item_interior(cpu, hwnd, t, hdc, i, &mut ops);
    user32::paint::release_dc(cpu, &[hwnd, hdc]);
}

// Drawing.

fn edge(cpu: &mut Cpu, hdc: u32, r: Rect, kind: u32, flags: u32) {
    gfx::edge(cpu, hdc, r, kind, flags);
}

/// TAB_EraseTabInterior.
fn erase_interior(cpu: &mut Cpu, t: &Tab, hdc: u32, i: i32, r: Rect) {
    let mut r = r;
    if t.style & TCS_BUTTONS != 0 {
        if i == t.selected {
            if t.style & TCS_OWNERDRAWFIXED == 0 {
                // The pressed button's dither of face and highlight.
                gfx::fill_sys(cpu, hdc, r, COLOR_SCROLLBAR);
            } else {
                gfx::fill_sys(cpu, hdc, r, COLOR_BTNFACE);
            }
        } else if t.style & TCS_FLATBUTTONS != 0 {
            r = r.inflate(2, 2);
            gfx::fill_sys(cpu, hdc, r, COLOR_BTNFACE);
            if i == t.hot || i == t.focus {
                edge(cpu, hdc, r, BDR_RAISEDINNER, BF_RECT);
            }
        } else {
            gfx::fill_sys(cpu, hdc, r, COLOR_BTNFACE);
        }
    } else {
        r = r.inflate(-2, -2);
        gfx::fill_sys(cpu, hdc, r, COLOR_BTNFACE);
    }
    if t.items[i as usize].state & TCIS_HIGHLIGHTED != 0 {
        gfx::fill_sys(cpu, hdc, r, COLOR_HIGHLIGHT);
    }
}

/// TAB_DrawItemInterior: the label and image of the tab `i`. An
/// owner-drawn control's parent is asked with WM_DRAWITEM, which `ops`
/// gets.
fn draw_item_interior(cpu: &mut Cpu, hwnd: u32, t: &Tab, hdc: u32, i: i32, ops: &mut Vec<Op>) {
    let client = super::client(cpu, hwnd);
    let (visible, item_r, sel_r) = item_rect(t, client, i);
    if !visible {
        return;
    }
    let s = t.style;
    let selected = i == t.selected;
    let mut r = if selected { sel_r } else { item_r };
    if s & TCS_BUTTONS != 0 {
        if selected {
            r.left += 4;
            r.top += 4;
            r.right -= 4;
            if s & TCS_VERTICAL != 0 {
                if s & TCS_BOTTOM == 0 {
                    r.right += 1;
                }
                r.bottom -= 4;
            } else if s & TCS_BOTTOM != 0 {
                r.top -= 2;
                r.bottom -= 4;
            } else {
                r.bottom -= 1;
            }
        } else {
            r = r.inflate(-2, -2);
        }
    } else if s & TCS_VERTICAL != 0 && s & TCS_BOTTOM != 0 {
        if !selected {
            r.left += 2;
            r.top += 2;
            r.bottom -= 2;
        }
    } else if s & TCS_VERTICAL != 0 {
        if selected {
            r.right += 1;
        } else {
            r.top += 2;
            r.right -= 2;
            r.bottom -= 2;
        }
    } else if s & TCS_BOTTOM != 0 {
        if selected {
            r.top -= 2;
        } else {
            r = r.inflate(-2, -2);
            r.bottom += 2;
        }
    } else if selected {
        r.bottom += 3;
    } else {
        r.bottom -= 2;
        r = r.inflate(-2, 0);
    }
    erase_interior(cpu, t, hdc, i, r);
    let focused = user32::input::get_focus(cpu, &[]).unwrap_or(0) == hwnd;
    if s & TCS_FOCUSNEVER == 0 && focused && i == t.focus {
        let mut f = r;
        if s & TCS_BUTTONS == 0 {
            f = f.inflate(-3, -3);
        }
        if s & TCS_BOTTOM != 0 && s & TCS_VERTICAL == 0 {
            f.top -= 3;
        }
        if !(s & TCS_BUTTONS != 0 && !selected) {
            gfx::focus_rect(cpu, hdc, f);
        }
    }
    let font = if t.font != 0 {
        t.font
    } else {
        gfx::stock(cpu, SYSTEM_FONT)
    };
    let old_font = gfx::select(cpu, hdc, font);
    let old_mode = gfx::set_bk_mode(cpu, hdc, TRANSPARENT);
    let item = t.items[i as usize].clone();
    let color = if s & TCS_HOTTRACK != 0 && i == t.hot && s & TCS_FLATBUTTONS == 0 {
        gfx::sys_color(cpu, COLOR_HIGHLIGHT)
    } else if item.state & TCIS_HIGHLIGHTED != 0 {
        gfx::sys_color(cpu, COLOR_HIGHLIGHTTEXT)
    } else {
        gfx::sys_color(cpu, COLOR_BTNTEXT)
    };
    let old_color = gfx::set_text_color(cpu, hdc, color);
    if s & TCS_OWNERDRAWFIXED != 0 {
        r.top += 2;
        r.right -= 1;
        if selected {
            r.right -= 1;
            r.left += 1;
        }
        let id = super::id_of(cpu, hwnd);
        let mut bits = 0;
        if selected {
            bits |= ODS_SELECTED;
        }
        if t.focus == i {
            bits |= ODS_FOCUS;
        }
        let mut data = [0u8; 4];
        let n = item.extra.len().min(4);
        data[..n].copy_from_slice(&item.extra[..n]);
        ops.push(Op::Write {
            at: S_DIS,
            words: vec![
                ODT_TAB,
                id,
                i as u32,
                ODA_DRAWENTIRE,
                bits,
                hwnd,
                hdc,
                r.left as u32,
                r.top as u32,
                r.right as u32,
                r.bottom as u32,
                u32::from_le_bytes(data),
            ],
        });
        ops.push(send_op(t.notify, WM_DRAWITEM, id, Arg::S(S_DIS)));
    } else {
        let mut text_r = Rect::default();
        if let Some(text) = &item.text {
            gfx::draw_text(cpu, hdc, text, &mut text_r, DT_LEFT | DT_CALCRECT);
        }
        let (text_w, text_h) = (text_r.width(), text_r.height());
        if t.himl != 0 && item.image != -1 {
            let (cx, cy) = imagelist::icon_size(cpu, t.himl).unwrap_or((0, 0));
            let (mut off_h, mut off_v) = if s & TCS_VERTICAL != 0 {
                (
                    (r.height() - (cy + t.hpad + text_w)) / 2,
                    (r.width() - cx) / 2,
                )
            } else {
                (
                    (r.width() - (cx + t.hpad + text_w)) / 2,
                    (r.height() - cy) / 2,
                )
            };
            if selected {
                off_v -= t.vpad / 2;
            } else {
                off_v += t.vpad / 2;
            }
            if s & TCS_FIXEDWIDTH != 0 && s & (TCS_FORCELABELLEFT | TCS_FORCEICONLEFT) != 0 {
                off_h = t.hpad;
            }
            off_h = off_h.max(2);
            off_v = off_v.max(0);
            let (x, y);
            if s & TCS_VERTICAL != 0 && s & TCS_BOTTOM != 0 {
                y = r.top + off_h;
                x = r.right - cx - off_v;
                r.top += cy + t.hpad;
            } else if s & TCS_VERTICAL != 0 {
                y = r.bottom - cy - off_h;
                x = r.left + off_v;
                r.bottom -= cy + t.hpad;
            } else {
                x = r.left + off_h;
                y = r.top + off_v;
                r.left += cx + t.hpad;
            }
            imagelist::draw(cpu, t.himl, item.image, hdc, x, y, 0);
        }
        let off_h = if s & TCS_FIXEDWIDTH != 0 && s & TCS_FORCELABELLEFT != 0 {
            t.hpad
        } else if s & TCS_VERTICAL != 0 {
            (r.height() - text_w) / 2
        } else {
            (r.width() - text_w) / 2
        };
        let mut off_v;
        if s & TCS_VERTICAL != 0 {
            if s & TCS_BOTTOM != 0 {
                r.top += off_h;
            } else {
                r.bottom -= off_h;
            }
            off_v = (r.width() - text_h) / 2;
        } else {
            r.left += off_h;
            off_v = (r.height() - text_h) / 2;
        }
        if selected {
            off_v -= t.vpad / 2;
        } else {
            off_v += t.vpad / 2;
        }
        off_v = off_v.max(0);
        if s & TCS_VERTICAL != 0 {
            r.left += off_v;
        } else {
            r.top += off_v;
        }
        // Vertical tabs have their labels drawn across: GDI32 here has
        // no turned fonts.
        if let Some(text) = &item.text {
            gfx::draw_text(cpu, hdc, text, &mut r, DT_LEFT | DT_SINGLELINE);
        }
    }
    gfx::set_text_color(cpu, hdc, old_color);
    gfx::select(cpu, hdc, old_font);
    gfx::set_bk_mode(cpu, hdc, old_mode);
}

/// A corner of a tab: its square cleared to the face color, then the
/// slanted edge across it, drawn in `rr` as `flags` say.
fn corner(cpu: &mut Cpu, hdc: u32, square: Rect, rr: Rect, kind: u32, flags: u32) {
    gfx::fill_sys(cpu, hdc, square, COLOR_BTNFACE);
    edge(cpu, hdc, rr, kind, flags);
}

/// TAB_DrawItem: the tab `i`, its edges and its inside.
fn draw_item(cpu: &mut Cpu, hwnd: u32, t: &Tab, hdc: u32, i: i32, ops: &mut Vec<Op>) {
    let client = super::client(cpu, hwnd);
    let (visible, item_r, sel_r) = item_rect(t, client, i);
    if !visible {
        return;
    }
    let s = t.style;
    let selected = i == t.selected;
    const SOFT: u32 = BF_SOFT;
    if s & TCS_BUTTONS != 0 {
        let r = item_r;
        if s & TCS_FLATBUTTONS != 0 && t.ex_style & TCS_EX_FLATSEPARATORS != 0 {
            let mut r1 = r;
            r1.right += FLAT_BTN_SPACINGX - 2;
            edge(cpu, hdc, r1, EDGE_ETCHED, BF_RIGHT);
        }
        if selected {
            edge(cpu, hdc, r, EDGE_SUNKEN, SOFT | BF_RECT);
        } else {
            let st = t.items[i as usize].state;
            if st & TCIS_BUTTONPRESSED != 0 || i == t.focus {
                edge(cpu, hdc, r, EDGE_SUNKEN, SOFT | BF_RECT);
            } else if s & TCS_FLATBUTTONS == 0 {
                edge(cpu, hdc, r, EDGE_RAISED, SOFT | BF_RECT);
            }
        }
    } else {
        let mut r = if selected { sel_r } else { item_r };
        let mut fill = r;
        let last_row = t.items[i as usize].top == t.rows - 1;
        let rc = ROUND_CORNER_SIZE;
        if s & TCS_VERTICAL != 0 {
            if selected {
                if sel_r.top == 0 {
                    fill.top += CONTROL_BORDER_SIZEY;
                }
                if sel_r.bottom == client.bottom {
                    fill.bottom -= CONTROL_BORDER_SIZEY;
                }
            }
            if s & TCS_BOTTOM != 0 {
                if selected {
                    r.left += 1;
                }
                gfx::fill_sys(cpu, hdc, fill, COLOR_BTNFACE);
                edge(
                    cpu,
                    hdc,
                    r,
                    EDGE_RAISED,
                    SOFT | BF_RIGHT | BF_TOP | BF_BOTTOM,
                );
                let sq = Rect::new(r.right - rc - 1, r.top, r.right, r.top + rc);
                let mut d = sq;
                d.right -= 1;
                corner(cpu, hdc, sq, d, EDGE_RAISED, SOFT | BF_DIAGONAL_ENDTOPLEFT);
                let sq = Rect::new(r.right - rc - 1, r.bottom - rc, r.right, r.bottom);
                let mut d = sq;
                d.right -= 1;
                corner(
                    cpu,
                    hdc,
                    sq,
                    d,
                    EDGE_RAISED,
                    SOFT | BF_DIAGONAL_ENDBOTTOMLEFT,
                );
                if selected && sel_r.top == 0 {
                    let r1 = Rect::new(r.left - 1, r.top, r.left, r.bottom);
                    edge(cpu, hdc, r1, EDGE_RAISED, SOFT | BF_TOP);
                }
            } else {
                gfx::fill_sys(cpu, hdc, fill, COLOR_BTNFACE);
                edge(
                    cpu,
                    hdc,
                    r,
                    EDGE_RAISED,
                    SOFT | BF_LEFT | BF_TOP | BF_BOTTOM,
                );
                let sq = Rect::new(r.left, r.top, r.left + rc + 1, r.top + rc);
                let mut d = sq;
                d.left += 1;
                corner(cpu, hdc, sq, d, EDGE_RAISED, SOFT | BF_DIAGONAL_ENDTOPRIGHT);
                let sq = Rect::new(r.left, r.bottom - rc, r.left + rc + 1, r.bottom);
                let mut d = sq;
                d.left += 1;
                corner(cpu, hdc, sq, d, EDGE_SUNKEN, BF_DIAGONAL_ENDTOPLEFT);
            }
        } else {
            if selected {
                if sel_r.left == 0 {
                    fill.left += CONTROL_BORDER_SIZEX;
                }
                if sel_r.right == client.right {
                    fill.right -= CONTROL_BORDER_SIZEX;
                }
            }
            if s & TCS_BOTTOM != 0 {
                if last_row {
                    fill.top -= 2;
                    r.top -= 1;
                }
                gfx::fill_sys(cpu, hdc, fill, COLOR_BTNFACE);
                edge(
                    cpu,
                    hdc,
                    r,
                    EDGE_RAISED,
                    SOFT | BF_LEFT | BF_BOTTOM | BF_RIGHT,
                );
                let sq = Rect::new(r.right - rc, r.bottom - rc - 1, r.right, r.bottom);
                let mut d = sq;
                d.bottom -= 1;
                corner(
                    cpu,
                    hdc,
                    sq,
                    d,
                    EDGE_RAISED,
                    SOFT | BF_DIAGONAL_ENDBOTTOMLEFT,
                );
                let sq = Rect::new(r.left, r.bottom - rc - 1, r.left + rc, r.bottom);
                let mut d = sq;
                d.bottom -= 1;
                corner(cpu, hdc, sq, d, EDGE_RAISED, SOFT | BF_DIAGONAL_ENDTOPLEFT);
                if selected {
                    r.top += 2;
                    r.left += 1;
                    if sel_r.left == 0 {
                        let r1 = Rect::new(r.left, r.top - 1, r.right, r.top);
                        edge(cpu, hdc, r1, EDGE_RAISED, SOFT | BF_LEFT);
                    }
                }
            } else {
                if last_row {
                    fill.bottom += 3;
                    r.bottom += 2;
                }
                gfx::fill_sys(cpu, hdc, fill, COLOR_BTNFACE);
                edge(cpu, hdc, r, EDGE_RAISED, SOFT | BF_LEFT | BF_TOP | BF_RIGHT);
                let sq = Rect::new(r.right - rc, r.top, r.right, r.top + rc + 1);
                let mut d = sq;
                d.top += 1;
                corner(
                    cpu,
                    hdc,
                    sq,
                    d,
                    EDGE_RAISED,
                    SOFT | BF_DIAGONAL_ENDBOTTOMRIGHT,
                );
                let sq = Rect::new(r.left, r.top, r.left + rc, r.top + rc + 1);
                let mut d = sq;
                d.top += 1;
                corner(cpu, hdc, sq, d, EDGE_RAISED, SOFT | BF_DIAGONAL_ENDTOPRIGHT);
            }
        }
    }
    draw_item_interior(cpu, hwnd, t, hdc, i, ops);
}

/// TAB_DrawBorder: the raised edge round the display area.
fn draw_border(cpu: &mut Cpu, hwnd: u32, t: &Tab, hdc: u32) {
    let mut r = super::client(cpu, hwnd);
    if !t.items.is_empty() {
        let tabs = t.tab_height * t.rows + CONTROL_BORDER_SIZEX;
        match (t.style & TCS_VERTICAL != 0, t.style & TCS_BOTTOM != 0) {
            (false, true) => r.bottom -= tabs,
            (true, true) => r.right -= tabs,
            (true, false) => r.left += tabs,
            (false, false) => r.top += tabs,
        }
    }
    edge(cpu, hdc, r, EDGE_RAISED, BF_SOFT | BF_RECT);
}

/// Paint the control on `hdc` (TAB_Refresh): the WM_DRAWITEM messages
/// an owner-drawn control's parent is sent.
pub fn draw(cpu: &mut Cpu, hwnd: u32, hdc: u32) -> Vec<Op> {
    let mut ops = Vec::new();
    let Some(t) = get(cpu, hwnd) else {
        return ops;
    };
    if !t.redraw {
        return ops;
    }
    let font = if t.font != 0 {
        t.font
    } else {
        gfx::stock(cpu, SYSTEM_FONT)
    };
    let old_font = gfx::select(cpu, hdc, font);
    let n = t.items.len() as i32;
    if t.style & TCS_BUTTONS != 0 {
        for i in 0..n {
            draw_item(cpu, hwnd, &t, hdc, i, &mut ops);
        }
    } else {
        for i in 0..n {
            if i != t.selected {
                draw_item(cpu, hwnd, &t, hdc, i, &mut ops);
            }
        }
        draw_border(cpu, hwnd, &t, hdc);
        draw_item(cpu, hwnd, &t, hdc, t.selected, &mut ops);
    }
    gfx::select(cpu, hdc, old_font);
    ops
}

// Selection and focus.

/// TAB_SetCurSel: the tab selected before.
fn set_cur_sel(cpu: &mut Cpu, hwnd: u32, t: &mut Tab, item: i32, ops: &mut Vec<Op>) -> i32 {
    let prev = t.selected;
    if item < 0 {
        t.selected = -1;
    } else if item >= t.items.len() as i32 {
        return -1;
    } else if prev != item {
        if let Some(p) = usize::try_from(prev).ok().and_then(|p| t.items.get_mut(p)) {
            p.state &= !TCIS_BUTTONPRESSED;
        }
        t.items[item as usize].state |= TCIS_BUTTONPRESSED;
        t.selected = item;
        t.focus = item;
        ops.extend(ensure_selection_visible(cpu, hwnd, t));
        invalidate_tabs(cpu, hwnd, t);
    }
    prev
}

/// TAB_SetCurFocus: the focus moves to `item`, and the selection with
/// it but for buttons; TCN_SELCHANGING asks the parent first.
fn set_cur_focus(cpu: &mut Cpu, hwnd: u32, t: &mut Tab, item: i32) -> Vec<Op> {
    let mut ops = Vec::new();
    if item < 0 {
        // Windows doesn't tell the parent the selection went: the
        // tests' message sequences have nothing (Wine sends
        // TCN_SELCHANGE).
        t.focus = -1;
        if t.selected != -1 {
            t.selected = -1;
            invalidate_tabs(cpu, hwnd, t);
        }
    } else if item < t.items.len() as i32 {
        if t.style & TCS_BUTTONS != 0 {
            if t.focus != item {
                t.focus = item;
                super::invalidate(cpu, hwnd, false);
                ops.extend(notify_ops(cpu, hwnd, t.notify, TCN_FOCUSCHANGE, &[]));
            }
        } else {
            let old = t.focus;
            if t.selected != item || old == -1 {
                t.focus = item;
                if old != -1 {
                    ops.extend(notify_ops(cpu, hwnd, t.notify, TCN_SELCHANGING, &[]));
                    ops.push(Op::Then {
                        id: T_SELCHANGING,
                        data: vec![item as u32, old as u32],
                    });
                }
            }
        }
    }
    ops
}

/// Select the tab `item`, as TCN_SELCHANGING allowed.
fn select(cpu: &mut Cpu, hwnd: u32, t: &mut Tab, item: i32) -> Vec<Op> {
    if let Some(p) = usize::try_from(t.selected)
        .ok()
        .and_then(|p| t.items.get_mut(p))
    {
        p.state &= !TCIS_BUTTONPRESSED;
    }
    t.items[item as usize].state |= TCIS_BUTTONPRESSED;
    t.selected = item;
    let ops = ensure_selection_visible(cpu, hwnd, t);
    invalidate_tabs(cpu, hwnd, t);
    ops
}

/// After TCN_SELCHANGING from the keyboard or TCM_SETCURFOCUS: a
/// refusal leaves the selection and the focus as they were (Wine
/// selects the tab all the same); otherwise the tab is selected and
/// TCN_SELCHANGE tells the parent.
fn after_selchanging(cpu: &mut Cpu, hwnd: u32, c: &Ctx) -> Vec<Op> {
    let Some(mut t) = get(cpu, hwnd) else {
        return Vec::new();
    };
    let (item, old) = (c.word(0) as i32, c.word(1) as i32);
    if c.last != 0 {
        t.focus = old;
        put(cpu, hwnd, t);
        return Vec::new();
    }
    if item < 0 || item as usize >= t.items.len() {
        return Vec::new();
    }
    let mut ops = select(cpu, hwnd, &mut t, item);
    let notify = t.notify;
    put(cpu, hwnd, t);
    ops.extend(notify_ops(cpu, hwnd, notify, TCN_SELCHANGE, &[]));
    ops
}

/// After TCN_SELCHANGING from a click: the tab is selected, or with
/// buttons that TCS_MULTISELECT left pressed all are let go, then
/// TCN_SELCHANGE.
fn after_click(cpu: &mut Cpu, hwnd: u32, c: &Ctx) -> Vec<Op> {
    let Some(mut t) = get(cpu, hwnd) else {
        return Vec::new();
    };
    if c.last != 0 {
        return Vec::new();
    }
    let (item, pressed) = (c.word(0) as i32, c.word(1) != 0);
    let mut ops = Vec::new();
    if pressed {
        deselect_all(cpu, hwnd, &mut t, false);
    } else {
        set_cur_sel(cpu, hwnd, &mut t, item, &mut ops);
    }
    let notify = t.notify;
    put(cpu, hwnd, t);
    ops.extend(notify_ops(cpu, hwnd, notify, TCN_SELCHANGE, &[]));
    ops
}

/// TAB_DeselectAll: buttons let go.
fn deselect_all(cpu: &mut Cpu, hwnd: u32, t: &mut Tab, exclude_selected: bool) {
    if t.style & TCS_BUTTONS == 0 {
        return;
    }
    let mut paint = false;
    let sel = t.selected;
    for (i, it) in t.items.iter_mut().enumerate() {
        if it.state & TCIS_BUTTONPRESSED != 0 && sel != i as i32 {
            it.state &= !TCIS_BUTTONPRESSED;
            paint = true;
        }
    }
    if !exclude_selected && sel != -1 {
        if let Some(it) = t.items.get_mut(sel as usize) {
            it.state &= !TCIS_BUTTONPRESSED;
        }
        t.selected = -1;
        paint = true;
    }
    if paint {
        invalidate_tabs(cpu, hwnd, t);
    }
}

/// The continuations of the control's scripts.
pub fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    match id {
        DRAW if c.last != 0 => draw(cpu, c.hwnd, c.last),
        T_SELCHANGING => after_selchanging(cpu, c.hwnd, c),
        T_CLICK => after_click(cpu, c.hwnd, c),
        T_UPDOWN => {
            let Some(mut t) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            t.updown = c.last;
            let mut ops = Vec::new();
            if t.updown != 0 {
                let client = super::client(cpu, c.hwnd);
                let range = max_range(&t, client);
                ops.push(send_op(t.updown, UDM_SETRANGE32, 0, Arg::V(range as u32)));
                ops.extend(ensure_selection_visible(cpu, c.hwnd, &mut t));
            }
            put(cpu, c.hwnd, t);
            ops
        }
        T_TOOLTIP => {
            let Some(mut t) = get(cpu, c.hwnd) else {
                return Vec::new();
            };
            t.tooltip = c.last;
            let notify = t.notify;
            put(cpu, c.hwnd, t);
            if c.last == 0 {
                return Vec::new();
            }
            notify_ops(cpu, c.hwnd, notify, NM_TOOLTIPSCREATED, &[c.last])
        }
        _ => Vec::new(),
    }
}

/// TTM_RELAYEVENT of a mouse message to the tooltip.
fn relay_ops(cpu: &mut Cpu, hwnd: u32, tip: u32, msg: u32, wp: u32, lp: u32) -> Vec<Op> {
    let (x, y) = user32::user(cpu).input.cursor;
    vec![
        Op::Write {
            at: S_MSG,
            words: vec![hwnd, msg, wp, lp, 0, x as u32, y as u32],
        },
        send_op(tip, TTM_RELAYEVENT, 0, Arg::S(S_MSG)),
    ]
}

/// The words of a TCITEMA: mask, dwState, dwStateMask, pszText,
/// cchTextMax, iImage; lParam (or the program's bytes) follows.
const TCITEM_WORDS: usize = 6;
const TCITEM_PARAM: u32 = 24;

/// A TCITEMA's text: None for none, or LPSTR_TEXTCALLBACKA.
fn item_text(cpu: &mut Cpu, w: &[u32]) -> Option<Vec<u8>> {
    if w[0] & TCIF_TEXT != 0 && w[3] != 0 && w[3] != u32::MAX {
        Some(user32::bytes_at(cpu, w[3]).unwrap_or_default())
    } else {
        None
    }
}

/// The tab control's window procedure.
pub fn proc_(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        return Some(0);
    };
    if msg == WM_CREATE {
        return create(cpu, hwnd, lp);
    }
    let Some(mut t) = get(cpu, hwnd) else {
        return defwnd::def_window_proc(cpu, args);
    };
    let mut s = Script::new(hwnd);
    s.alloc(S_SIZE);
    let result: u32 = match msg {
        TCM_GETIMAGELIST => t.himl,
        TCM_SETIMAGELIST => {
            let old = std::mem::replace(&mut t.himl, lp);
            let ops = set_item_bounds(cpu, hwnd, &mut t);
            s.ops.extend(ops);
            put(cpu, hwnd, t);
            super::invalidate(cpu, hwnd, true);
            old
        }
        TCM_GETITEMCOUNT => t.items.len() as u32,
        TCM_GETITEMA => {
            if lp == 0 {
                return Some(0);
            }
            let w = read_words(cpu, lp, TCITEM_WORDS);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let mask = w[0];
            let i = wp as i32;
            let cb = t.cb_info.max(0) as usize;
            if i < 0 || i >= t.items.len() as i32 {
                // Windows clears what the mask asks for.
                if mask & TCIF_IMAGE != 0 {
                    write_words(cpu, lp + 20, &[0]);
                }
                if mask & TCIF_PARAM != 0 {
                    cpu.bus.guest_write_bytes(lp + TCITEM_PARAM, &vec![0; cb]);
                }
                if mask & TCIF_STATE != 0 {
                    write_words(cpu, lp + 4, &[0]);
                }
                return Some(0);
            }
            let it = t.items[i as usize].clone();
            if mask & TCIF_IMAGE != 0 {
                write_words(cpu, lp + 20, &[it.image as u32]);
            }
            if mask & TCIF_PARAM != 0 {
                cpu.bus.guest_write_bytes(lp + TCITEM_PARAM, &it.extra);
            }
            if mask & TCIF_STATE != 0 {
                write_words(cpu, lp + 4, &[it.state & w[2]]);
            }
            if mask & TCIF_TEXT != 0 && w[3] != 0 && w[4] != 0 {
                user32::copy_out(cpu, w[3], w[4], &it.text.unwrap_or_default());
            }
            if cpu.bus.guest_faulted() {
                return None;
            }
            1
        }
        TCM_SETITEMA => {
            let i = wp as i32;
            if i < 0 || i >= t.items.len() as i32 || lp == 0 {
                return Some(0);
            }
            let w = read_words(cpu, lp, TCITEM_WORDS);
            let extra = read_bytes(cpu, lp + TCITEM_PARAM, t.cb_info.max(0) as usize);
            let text = item_text(cpu, &w);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let it = &mut t.items[i as usize];
            if w[0] & TCIF_IMAGE != 0 {
                it.image = w[5] as i32;
            }
            if w[0] & TCIF_PARAM != 0 {
                it.extra = extra;
            }
            if w[0] & TCIF_STATE != 0 {
                it.state = (it.state & !w[2]) | (w[1] & w[2]);
            }
            if w[0] & TCIF_TEXT != 0 {
                it.text = text;
            }
            let ops = set_item_bounds(cpu, hwnd, &mut t);
            s.ops.extend(ops);
            invalidate_tabs(cpu, hwnd, &t);
            put(cpu, hwnd, t);
            1
        }
        TCM_INSERTITEMA => {
            if lp == 0 {
                return Some(u32::MAX);
            }
            let w = read_words(cpu, lp, TCITEM_WORDS);
            let extra = read_bytes(cpu, lp + TCITEM_PARAM, t.cb_info.max(0) as usize);
            let text = item_text(cpu, &w);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let mut i = wp as i32;
            if i < 0 {
                return Some(u32::MAX);
            }
            let n = t.items.len() as i32;
            i = i.min(n);
            if n == 0 {
                t.selected = 0;
            } else if i <= t.selected {
                t.selected += 1;
            }
            let item = Item {
                text,
                image: if w[0] & TCIF_IMAGE != 0 {
                    w[5] as i32
                } else {
                    -1
                },
                extra: if w[0] & TCIF_PARAM != 0 {
                    extra
                } else {
                    vec![0; t.cb_info.max(0) as usize]
                },
                ..Item::default()
            };
            t.items.insert(i as usize, item);
            let ops = set_item_bounds(cpu, hwnd, &mut t);
            s.ops.extend(ops);
            if t.items.len() > 1 {
                invalidate_tabs(cpu, hwnd, &t);
            } else {
                super::invalidate(cpu, hwnd, true);
            }
            if t.focus == -1 {
                let ops = set_cur_focus(cpu, hwnd, &mut t, i);
                s.ops.extend(ops);
            }
            put(cpu, hwnd, t);
            i as u32
        }
        TCM_DELETEITEM => {
            let i = wp as i32;
            if i < 0 || i >= t.items.len() as i32 {
                return Some(0);
            }
            invalidate_tabs(cpu, hwnd, &t);
            t.items.remove(i as usize);
            if t.items.is_empty() && t.hot >= 0 {
                user32::message::kill_timer(cpu, &[hwnd, HOTTRACK_TIMER]);
                t.hot = -1;
            }
            if i == t.selected {
                t.selected = -1;
            } else if i < t.selected {
                t.selected -= 1;
            }
            if t.items.is_empty() {
                t.selected = -1;
            }
            let ops = set_item_bounds(cpu, hwnd, &mut t);
            s.ops.extend(ops);
            put(cpu, hwnd, t);
            1
        }
        TCM_DELETEALLITEMS => {
            invalidate_tabs(cpu, hwnd, &t);
            t.items.clear();
            if t.hot >= 0 {
                user32::message::kill_timer(cpu, &[hwnd, HOTTRACK_TIMER]);
                t.hot = -1;
            }
            t.selected = -1;
            let ops = set_item_bounds(cpu, hwnd, &mut t);
            s.ops.extend(ops);
            put(cpu, hwnd, t);
            1
        }
        TCM_GETITEMRECT => {
            let client = super::client(cpu, hwnd);
            let (visible, r, _) = item_rect(&t, client, wp as i32);
            if lp != 0 {
                r.write(cpu, lp);
                if cpu.bus.guest_faulted() {
                    return None;
                }
            }
            visible as u32
        }
        TCM_GETCURSEL => t.selected as u32,
        TCM_HITTEST => {
            if lp == 0 {
                return Some(u32::MAX);
            }
            let w = read_words(cpu, lp, 2);
            if cpu.bus.guest_faulted() {
                return None;
            }
            let client = super::client(cpu, hwnd);
            let i = hit_test(&t, client, w[0] as i32, w[1] as i32);
            let flags = if i < 0 { TCHT_NOWHERE } else { TCHT_ONITEM };
            write_words(cpu, lp + 8, &[flags]);
            i as u32
        }
        TCM_SETCURSEL => {
            let mut ops = Vec::new();
            let prev = set_cur_sel(cpu, hwnd, &mut t, wp as i32, &mut ops);
            s.ops.extend(ops);
            put(cpu, hwnd, t);
            prev as u32
        }
        TCM_SETITEMEXTRA => {
            let n = wp as i32;
            if n < 0 || !t.items.is_empty() {
                return Some(0);
            }
            t.cb_info = n;
            put(cpu, hwnd, t);
            1
        }
        TCM_ADJUSTRECT => {
            if lp == 0 {
                return Some(u32::MAX);
            }
            let mut r = Rect::read(cpu, lp);
            if cpu.bus.guest_faulted() {
                return None;
            }
            adjust_rect(&t, wp != 0, &mut r);
            r.write(cpu, lp);
            0
        }
        TCM_SETITEMSIZE => {
            let (cx, cy) = (lp as u16 as i32, (lp >> 16) as u16 as i32);
            let old = user32::make_long(t.tab_width, t.tab_height);
            let mut paint = false;
            if t.style & TCS_FIXEDWIDTH != 0 && t.tab_width != cx {
                t.tab_width = cx;
                paint = true;
            }
            if t.tab_height != cy {
                t.height_set = cy != 0;
                if t.height_set {
                    t.tab_height = cy;
                }
                paint = true;
            }
            if paint {
                let ops = set_item_bounds(cpu, hwnd, &mut t);
                s.ops.extend(ops);
                let flags = RDW_ERASE | RDW_INVALIDATE | RDW_UPDATENOW;
                s.user("RedrawWindow", &[hwnd, 0, 0, flags]);
            }
            put(cpu, hwnd, t);
            old
        }
        TCM_REMOVEIMAGE => {
            let image = wp as i32;
            let removed = imagelist::il_remove(cpu, &[t.himl, wp]).unwrap_or(0) != 0;
            if removed {
                for it in t.items.iter_mut() {
                    if it.image == image {
                        it.image = -1;
                    } else if it.image > image {
                        it.image -= 1;
                    }
                }
                super::invalidate(cpu, hwnd, true);
                put(cpu, hwnd, t);
            }
            0
        }
        TCM_SETPADDING => {
            t.hpad_set = lp as u16 as i32;
            t.vpad_set = (lp >> 16) as u16 as i32;
            put(cpu, hwnd, t);
            0
        }
        TCM_GETROWCOUNT => t.rows as u32,
        TCM_GETUNICODEFORMAT => t.unicode as u32,
        TCM_SETUNICODEFORMAT => {
            let old = std::mem::replace(&mut t.unicode, wp != 0);
            put(cpu, hwnd, t);
            old as u32
        }
        TCM_HIGHLIGHTITEM => {
            let i = wp as i32;
            if i < 0 || i >= t.items.len() as i32 {
                return Some(0);
            }
            let it = &mut t.items[i as usize];
            let old = it.state;
            if lp as u16 != 0 {
                it.state |= TCIS_HIGHLIGHTED;
            } else {
                it.state &= !TCIS_HIGHLIGHTED;
            }
            if old != it.state {
                super::invalidate(cpu, hwnd, true);
            }
            put(cpu, hwnd, t);
            1
        }
        TCM_GETTOOLTIPS => t.tooltip,
        TCM_SETTOOLTIPS => {
            t.tooltip = wp;
            put(cpu, hwnd, t);
            0
        }
        TCM_GETCURFOCUS => t.focus as u32,
        TCM_SETCURFOCUS => {
            let ops = set_cur_focus(cpu, hwnd, &mut t, wp as i32);
            s.ops.extend(ops);
            put(cpu, hwnd, t);
            0
        }
        TCM_SETMINTABWIDTH => {
            let old = if t.min_width < 0 {
                DEFAULT_MIN_TAB_WIDTH
            } else {
                t.min_width
            };
            t.min_width = lp as i32;
            let ops = set_item_bounds(cpu, hwnd, &mut t);
            s.ops.extend(ops);
            put(cpu, hwnd, t);
            old as u32
        }
        TCM_DESELECTALL => {
            deselect_all(cpu, hwnd, &mut t, wp != 0);
            put(cpu, hwnd, t);
            0
        }
        TCM_GETEXTENDEDSTYLE => t.ex_style,
        TCM_SETEXTENDEDSTYLE => {
            let old = t.ex_style;
            let mask = if wp == 0 { u32::MAX } else { wp };
            t.ex_style = (t.ex_style & !mask) | (lp & mask);
            if (old ^ t.ex_style) & TCS_EX_FLATSEPARATORS != 0 {
                invalidate_tabs(cpu, hwnd, &t);
            }
            put(cpu, hwnd, t);
            old
        }
        // Windows 95's COMCTL32 has no Unicode tabs here.
        TCM_INSERTITEMW => u32::MAX,
        TCM_GETITEMW | TCM_SETITEMW => 0,
        WM_GETFONT => t.font,
        WM_SETFONT => {
            t.font = wp;
            let ops = set_item_bounds(cpu, hwnd, &mut t);
            s.ops.extend(ops);
            invalidate_tabs(cpu, hwnd, &t);
            put(cpu, hwnd, t);
            0
        }
        WM_NCDESTROY => {
            state(cpu).tab.tabs.remove(&hwnd);
            if t.hot >= 0 {
                user32::message::kill_timer(cpu, &[hwnd, HOTTRACK_TIMER]);
            }
            if t.tooltip != 0 && user32::user(cpu).is_window(t.tooltip) {
                s.user("DestroyWindow", &[t.tooltip]);
            }
            0
        }
        WM_GETDLGCODE => DLGC_WANTARROWS | DLGC_WANTCHARS,
        WM_LBUTTONDOWN => {
            if t.tooltip != 0 {
                s.ops.extend(relay_ops(cpu, hwnd, t.tooltip, msg, wp, lp));
            }
            if t.style & TCS_FOCUSNEVER == 0 {
                s.user("SetFocus", &[hwnd]);
            }
            let (x, y) = user32::point_of(lp);
            let client = super::client(cpu, hwnd);
            let item = hit_test(&t, client, x, y);
            if item != -1 && t.selected != item {
                let buttons = t.style & TCS_BUTTONS != 0;
                if buttons && t.style & TCS_MULTISELECT != 0 && wp & MK_CONTROL != 0 {
                    t.items[item as usize].state ^= TCIS_BUTTONPRESSED;
                    super::invalidate(cpu, hwnd, false);
                    put(cpu, hwnd, t);
                } else {
                    let sel = t.selected;
                    let pressed = buttons
                        && t.items
                            .iter()
                            .enumerate()
                            .any(|(i, it)| it.state & TCIS_BUTTONPRESSED != 0 && sel != i as i32);
                    let ops = notify_ops(cpu, hwnd, t.notify, TCN_SELCHANGING, &[]);
                    s.ops.extend(ops);
                    s.then(T_CLICK, &[item as u32, pressed as u32]);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            let ops = notify_ops(cpu, hwnd, t.notify, NM_CLICK, &[]);
            s.ops.extend(ops);
            0
        }
        WM_RBUTTONDOWN => {
            let ops = notify_ops(cpu, hwnd, t.notify, NM_RCLICK, &[]);
            s.ops.extend(ops);
            0
        }
        WM_NOTIFY => {
            s.send(t.notify, WM_NOTIFY, Arg::V(wp), Arg::V(lp))
                .ret_last();
            return script::run(cpu, TAB, s);
        }
        WM_MOUSEMOVE => {
            if t.tooltip != 0 {
                s.ops.extend(relay_ops(cpu, hwnd, t.tooltip, msg, wp, lp));
            }
            let pos = user32::point_of(lp);
            let (leave, enter) = recalc_hot_track(cpu, hwnd, &mut t, Some(pos));
            hot_track_refresh(cpu, hwnd, &t, leave);
            hot_track_refresh(cpu, hwnd, &t, enter);
            put(cpu, hwnd, t);
            0
        }
        WM_TIMER if wp == HOTTRACK_TIMER => {
            // The mouse may have left the control without a
            // WM_MOUSEMOVE to say so.
            if t.hot >= 0 {
                let st = user32::user(cpu);
                let (cx, cy) = st.input.cursor;
                if user32::window::window_at(st, cx, cy) != hwnd {
                    let redraw = t.hot;
                    t.hot = -1;
                    hot_track_refresh(cpu, hwnd, &t, redraw);
                    user32::message::kill_timer(cpu, &[hwnd, HOTTRACK_TIMER]);
                }
            }
            put(cpu, hwnd, t);
            0
        }
        WM_PRINTCLIENT | WM_PAINT => {
            if wp != 0 {
                let ops = draw(cpu, hwnd, wp);
                s.ops.extend(ops);
                s.ret(0);
                return script::run(cpu, TAB, s);
            }
            let begin = user32::address_of("BeginPaint");
            let end = user32::address_of("EndPaint");
            s.call(begin, &[Arg::V(hwnd), Arg::S(S_PS)])
                .then(DRAW, &[])
                .call(end, &[Arg::V(hwnd), Arg::S(S_PS)])
                .ret(0);
            return script::run(cpu, TAB, s);
        }
        WM_SIZE => {
            let ops = set_item_bounds(cpu, hwnd, &mut t);
            s.ops.extend(ops);
            put(cpu, hwnd, t);
            super::invalidate(cpu, hwnd, true);
            0
        }
        WM_SETREDRAW => {
            t.redraw = wp != 0;
            put(cpu, hwnd, t);
            0
        }
        WM_HSCROLL => {
            let (code, pos) = (wp & 0xFFFF, (wp >> 16) as u16 as i32);
            if code == SB_THUMBPOSITION && pos != t.leftmost {
                if pos < t.leftmost {
                    t.leftmost -= 1;
                } else {
                    t.leftmost += 1;
                }
                recalc_hot_track(cpu, hwnd, &mut t, None);
                invalidate_tabs(cpu, hwnd, &t);
                let pos = Arg::V(t.leftmost as u16 as u32);
                s.send(t.updown, UDM_SETPOS, Arg::V(0), pos);
                put(cpu, hwnd, t);
            }
            0
        }
        WM_STYLECHANGED => {
            if wp == GWL_STYLE {
                let new = cpu.bus.guest_read_32(lp + 4);
                if cpu.bus.guest_faulted() {
                    return None;
                }
                t.style = new;
                let ops = set_item_bounds(cpu, hwnd, &mut t);
                s.ops.extend(ops);
                put(cpu, hwnd, t);
                super::invalidate(cpu, hwnd, true);
            }
            0
        }
        WM_KILLFOCUS | WM_SETFOCUS => {
            // A button control's focus goes back to the selected one.
            if msg == WM_KILLFOCUS && t.style & TCS_BUTTONS != 0 && t.focus != t.selected {
                t.focus = t.selected;
                put(cpu, hwnd, t);
            }
            super::invalidate(cpu, hwnd, true);
            return defwnd::def_window_proc(cpu, args);
        }
        WM_KEYDOWN => {
            // NMTCKEYDOWN is packed: the key's WORD, then the flags.
            let more = [wp & 0xFFFF | (lp & 0xFFFF) << 16, lp >> 16];
            let ops = notify_ops(cpu, hwnd, t.notify, TCN_KEYDOWN, &more);
            s.ops.extend(ops);
            let new = match wp {
                VK_LEFT => t.focus - 1,
                VK_RIGHT => t.focus + 1,
                _ => -1,
            };
            if new >= 0 && t.focus != new {
                let ops = set_cur_focus(cpu, hwnd, &mut t, new);
                s.ops.extend(ops);
            }
            put(cpu, hwnd, t);
            0
        }
        WM_NCHITTEST => {
            let (x, y) = user32::point_of(lp);
            let (ox, oy) = user32::user(cpu).client_origin(hwnd);
            let client = super::client(cpu, hwnd);
            if hit_test(&t, client, x - ox, y - oy) == -1 {
                HTTRANSPARENT
            } else {
                HTCLIENT
            }
        }
        WM_NCCALCSIZE => {
            if wp != 0 {
                WVR_ALIGNTOP
            } else {
                0
            }
        }
        _ => return defwnd::def_window_proc(cpu, args),
    };
    s.result = result;
    s.ret(result);
    script::run(cpu, TAB, s)
}

/// WM_CREATE: the control's state, WS_CLIPSIBLINGS (which a tab
/// control always has), WM_NOTIFYFORMAT to the parent, the tooltip of
/// TCS_TOOLTIPS, and the tabs' first height, from the system font.
fn create(cpu: &mut Cpu, hwnd: u32, cs: u32) -> Option<u32> {
    let w = read_words(cpu, cs, 12);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let parent = w[3];
    let style = super::style(cpu, hwnd) | WS_CLIPSIBLINGS;
    if let Some(win) = user32::user(cpu).window_mut(hwnd) {
        win.style = style;
    }
    let mut t = Tab {
        notify: parent,
        hpad: 6,
        vpad: 3,
        hpad_set: 6,
        vpad_set: 3,
        selected: -1,
        hot: -1,
        focus: -1,
        redraw: true,
        cb_info: 4,
        style,
        ex_style: if style & TCS_FLATBUTTONS != 0 {
            TCS_EX_FLATSEPARATORS
        } else {
            0
        },
        min_width: -1,
        ..Tab::default()
    };
    let hdc = user32::paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
    let system = gfx::stock(cpu, SYSTEM_FONT);
    let old = gfx::select(cpu, hdc, system);
    let factor = if style & TCS_BUTTONS != 0 { 2 } else { 1 };
    t.tab_height = gfx::font_height(cpu, hdc) + SELECTED_TAB_OFFSET + factor * t.vpad;
    if style & TCS_FIXEDWIDTH != 0 {
        let caps = crate::win32::gdi32::dc::get_device_caps(cpu, &[hdc, LOGPIXELSX]);
        t.tab_width = caps.unwrap_or(96) as i32;
    }
    gfx::select(cpu, hdc, old);
    user32::paint::release_dc(cpu, &[hwnd, hdc]);
    put(cpu, hwnd, t);
    let mut s = Script::new(hwnd);
    s.alloc(S_SIZE);
    if style & TCS_TOOLTIPS != 0 {
        s.ops.push(Op::Write {
            at: S_CLASS,
            words: script::words_of(b"tooltips_class32\0"),
        });
        let cw = CW_USEDEFAULT;
        let args: Vec<Arg> = [0, 0, 0, WS_POPUP, cw, cw, cw, cw, hwnd, 0, 0, 0]
            .iter()
            .enumerate()
            .map(|(k, &v)| if k == 1 { Arg::S(S_CLASS) } else { Arg::V(v) })
            .collect();
        s.call(user32::address_of("CreateWindowExA"), &args)
            .then(T_TOOLTIP, &[]);
    }
    s.send(parent, WM_NOTIFYFORMAT, Arg::V(hwnd), Arg::V(NF_QUERY));
    s.ret(0);
    script::run(cpu, TAB, s)
}
