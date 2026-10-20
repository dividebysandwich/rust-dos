//! The ComboBox class, after Wine's combo.c, as Windows 95 has it, with
//! the ANSI messages.
//!
//! A combo box is a text area (an Edit child, or text it draws itself
//! for CBS_DROPDOWNLIST) and a ComboLBox list: a child below the text for
//! CBS_SIMPLE, a popup on the desktop that drops down for the others.
//! Most CB_* messages go on to the list or the edit.
//!
//! As in the edit control, a message does its work on the state at once
//! and leaves what it then calls in `Calls`. The combo box reads its
//! list's state when it needs an answer from it, and sends it messages
//! to change it. Where Windows changes the state after a call returns,
//! the combo box sends itself `CBM_CLEAR` or `CBM_CLICK` after it.

use super::super::defwnd::def_window_proc;
use super::super::message::{
    WM_CHAR, WM_CLEAR, WM_COMMAND, WM_COMPAREITEM, WM_COPY, WM_CREATE, WM_CTLCOLOREDIT,
    WM_CTLCOLORSTATIC, WM_CUT, WM_DELETEITEM, WM_DRAWITEM, WM_ENABLE, WM_ERASEBKGND, WM_GETDLGCODE,
    WM_GETFONT, WM_GETTEXT, WM_GETTEXTLENGTH, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MEASUREITEM, WM_MOUSEMOVE, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_PASTE,
    WM_PRINTCLIENT, WM_SETFOCUS, WM_SETFONT, WM_SETREDRAW, WM_SETTEXT, WM_SIZE, WM_SYSKEYDOWN,
    WM_WINDOWPOSCHANGED, WM_WINDOWPOSCHANGING,
};
use super::super::paint::{
    self, BF_RECT, DFC_SCROLL, DFCS_INACTIVE, DFCS_PUSHED, DFCS_SCROLLCOMBOBOX, EDGE_SUNKEN,
};
use super::super::sysparams::{
    self, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW,
};
use super::super::window::{
    HWND_TOP, SW_HIDE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOREDRAW, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, WS_BORDER, WS_CHILD, WS_CLIPSIBLINGS, WS_DISABLED, WS_EX_CLIENTEDGE,
    WS_EX_NOPARENTNOTIFY, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_HSCROLL, WS_VISIBLE, WS_VSCROLL,
};
use super::super::{address_of, bytes_at, make_long, read_words, user, write_words};
use super::edit::{
    self, Calls, DLGC_WANTARROWS, DLGC_WANTCHARS, DLGC_WANTMESSAGE, EM_GETSEL, EM_LIMITTEXT,
    EM_SETSEL, EN_CHANGE, EN_KILLFOCUS, EN_SETFOCUS, EN_UPDATE, ES_AUTOHSCROLL, ES_COMBO,
    ES_LOWERCASE, ES_NOHIDESEL, ES_UPPERCASE, K_CALLS, K_PAINT, P_BRUSH, P_HDC, P_HWND, P_OWN,
    SCRATCH, SCRATCH_DATA, VK_DOWN, VK_ESCAPE, VK_F4, VK_RETURN, VK_UP, calls_step, client_of,
    clip, default_brush, measure, ops_push, ops_step, paint_end, paint_head, paint_start, run,
    select_font,
};
use super::listbox::{
    self, LB_ADDSTRING, LB_CARETOFF, LB_CARETON, LB_DELETESTRING, LB_DIR, LB_FINDSTRING,
    LB_FINDSTRINGEXACT, LB_GETCOUNT, LB_GETCURSEL, LB_GETHORIZONTALEXTENT, LB_GETITEMDATA,
    LB_GETITEMHEIGHT, LB_GETLOCALE, LB_GETTEXT, LB_GETTEXTLEN, LB_GETTOPINDEX, LB_INITSTORAGE,
    LB_INSERTSTRING, LB_RESETCONTENT, LB_SELECTSTRING, LB_SETCARETINDEX, LB_SETCURSEL,
    LB_SETHORIZONTALEXTENT, LB_SETITEMDATA, LB_SETITEMHEIGHT, LB_SETLOCALE, LB_SETTOPINDEX,
    LBN_DBLCLK, LBN_ERRSPACE, LBN_SELCANCEL, LBN_SELCHANGE, LBS_COMBOBOX, LBS_DISABLENOSCROLL,
    LBS_HASSTRINGS, LBS_NOINTEGRALHEIGHT, LBS_NOTIFY, LBS_SORT,
};
use super::{COMBOBOX, start};
use crate::cpu::Cpu;
use crate::win32::gdi32::{Rect, dc, font};
use crate::win32::task::{Next, Task};

// The styles.
pub const CBS_SIMPLE: u32 = 0x0001;
pub const CBS_DROPDOWN: u32 = 0x0002;
pub const CBS_DROPDOWNLIST: u32 = 0x0003;
pub const CBS_OWNERDRAWFIXED: u32 = 0x0010;
pub const CBS_OWNERDRAWVARIABLE: u32 = 0x0020;
pub const CBS_AUTOHSCROLL: u32 = 0x0040;
pub const CBS_OEMCONVERT: u32 = 0x0080;
pub const CBS_SORT: u32 = 0x0100;
pub const CBS_HASSTRINGS: u32 = 0x0200;
pub const CBS_NOINTEGRALHEIGHT: u32 = 0x0400;
pub const CBS_DISABLENOSCROLL: u32 = 0x0800;
pub const CBS_UPPERCASE: u32 = 0x2000;
pub const CBS_LOWERCASE: u32 = 0x4000;

// The messages.
pub const CB_GETEDITSEL: u32 = 0x0140;
pub const CB_LIMITTEXT: u32 = 0x0141;
pub const CB_SETEDITSEL: u32 = 0x0142;
pub const CB_ADDSTRING: u32 = 0x0143;
pub const CB_DELETESTRING: u32 = 0x0144;
pub const CB_DIR: u32 = 0x0145;
pub const CB_GETCOUNT: u32 = 0x0146;
pub const CB_GETCURSEL: u32 = 0x0147;
pub const CB_GETLBTEXT: u32 = 0x0148;
pub const CB_GETLBTEXTLEN: u32 = 0x0149;
pub const CB_INSERTSTRING: u32 = 0x014A;
pub const CB_RESETCONTENT: u32 = 0x014B;
pub const CB_FINDSTRING: u32 = 0x014C;
pub const CB_SELECTSTRING: u32 = 0x014D;
pub const CB_SETCURSEL: u32 = 0x014E;
pub const CB_SHOWDROPDOWN: u32 = 0x014F;
pub const CB_GETITEMDATA: u32 = 0x0150;
pub const CB_SETITEMDATA: u32 = 0x0151;
pub const CB_GETDROPPEDCONTROLRECT: u32 = 0x0152;
pub const CB_SETITEMHEIGHT: u32 = 0x0153;
pub const CB_GETITEMHEIGHT: u32 = 0x0154;
pub const CB_SETEXTENDEDUI: u32 = 0x0155;
pub const CB_GETEXTENDEDUI: u32 = 0x0156;
pub const CB_GETDROPPEDSTATE: u32 = 0x0157;
pub const CB_FINDSTRINGEXACT: u32 = 0x0158;
pub const CB_SETLOCALE: u32 = 0x0159;
pub const CB_GETLOCALE: u32 = 0x015A;
pub const CB_GETTOPINDEX: u32 = 0x015B;
pub const CB_SETTOPINDEX: u32 = 0x015C;
pub const CB_GETHORIZONTALEXTENT: u32 = 0x015D;
pub const CB_SETHORIZONTALEXTENT: u32 = 0x015E;
pub const CB_GETDROPPEDWIDTH: u32 = 0x015F;
pub const CB_SETDROPPEDWIDTH: u32 = 0x0160;
pub const CB_INITSTORAGE: u32 = 0x0161;
pub const CB_GETCOMBOBOXINFO: u32 = 0x0164;

// The notifications.
pub const CBN_ERRSPACE: u32 = 0xFFFF;
pub const CBN_SELCHANGE: u32 = 1;
pub const CBN_DBLCLK: u32 = 2;
pub const CBN_SETFOCUS: u32 = 3;
pub const CBN_KILLFOCUS: u32 = 4;
pub const CBN_EDITCHANGE: u32 = 5;
pub const CBN_EDITUPDATE: u32 = 6;
pub const CBN_DROPDOWN: u32 = 7;
pub const CBN_CLOSEUP: u32 = 8;
pub const CBN_SELENDOK: u32 = 9;
pub const CBN_SELENDCANCEL: u32 = 10;

const CB_OKAY: i32 = 0;
const CB_ERR: i32 = -1;

/// The combo box's own messages, to itself after a call: clear the
/// state's bits in wParam; the mouse button went down (after SetFocus).
const CBM_CLEAR: u32 = 0x03FE;
const CBM_CLICK: u32 = 0x03FF;

// The state's bits.
pub const CBF_DROPPED: u32 = 0x0001;
pub const CBF_BUTTONDOWN: u32 = 0x0002;
pub const CBF_NOROLLUP: u32 = 0x0004;
pub const CBF_MEASUREITEM: u32 = 0x0008;
pub const CBF_FOCUSED: u32 = 0x0010;
pub const CBF_CAPTURE: u32 = 0x0020;
pub const CBF_EDIT: u32 = 0x0040;
pub const CBF_NORESIZE: u32 = 0x0080;
pub const CBF_NOTIFY: u32 = 0x0100;
pub const CBF_NOREDRAW: u32 = 0x0200;
pub const CBF_SELCHANGE: u32 = 0x0400;
pub const CBF_NOEDITNOTIFY: u32 = 0x1000;
pub const CBF_NOLBSELECT: u32 = 0x2000;
pub const CBF_BEENFOCUSED: u32 = 0x4000;
pub const CBF_EUI: u32 = 0x8000;
/// WM_NCCREATE has been: the state is there.
const CBF_CREATED: u32 = 0x1_0000;

const ID_CB_LISTBOX: u32 = 1000;
const ID_CB_EDIT: u32 = 1001;

const X_BORDER: i32 = 2;
const Y_BORDER: i32 = 2;
const EDIT_PADDING: i32 = 1;

const ODT_COMBOBOX: u32 = 3;
const ODA_DRAWENTIRE: u32 = 1;
const ODS_SELECTED: u32 = 0x0001;
const ODS_DISABLED: u32 = 0x0004;
const ODS_FOCUS: u32 = 0x0010;
const ODS_COMBOBOXEDIT: u32 = 0x1000;

const ES_OEMCONVERT: u32 = 0x0400;
const SM_CXVSCROLL: u32 = 2;
const SM_CYSCREEN: u32 = 1;
const KEYDATA_ALT: u32 = 0x2000;
const ETO_OPAQUE: u32 = 2;
const ETO_CLIPPED: u32 = 4;
const RDW_INVALIDATE: u32 = 0x0001;
const RDW_ERASE: u32 = 0x0004;
const RDW_NOCHILDREN: u32 = 0x0040;
const RDW_UPDATENOW: u32 = 0x0100;

// The task kinds besides the shared ones.
const K_CREATE: u32 = 2;

/// What a combo box keeps.
#[derive(Clone, Debug, Default)]
pub struct Combo {
    /// The style as the combo box has it.
    pub style: u32,
    /// The window its notifications go to.
    pub owner: u32,
    pub edit: u32,
    pub list: u32,
    /// The CBF_* bits.
    pub state: u32,
    pub font: u32,
    /// The text area, the button and the list's place: in client
    /// coordinates, the list's on the screen when it drops down.
    pub text_rect: Rect,
    pub button_rect: Rect,
    pub dropped_rect: Rect,
    /// The list's selection when it dropped down.
    pub dropped_index: i32,
    pub fixed_height: i32,
    pub dropped_width: i32,
    pub edit_height: i32,
}

impl Combo {
    fn kind(&self) -> u32 {
        self.style & CBS_DROPDOWNLIST
    }

    fn owner_drawn(&self) -> bool {
        self.style & (CBS_OWNERDRAWFIXED | CBS_OWNERDRAWVARIABLE) != 0
    }
}

// What the list and the edit ask of their combo box.

/// Whether `hwnd` is a combo box that has its state.
pub(super) fn is_combo(cpu: &mut Cpu, hwnd: u32) -> bool {
    hwnd != 0
        && user(cpu)
            .window(hwnd)
            .is_some_and(|w| w.control.combo.state & CBF_CREATED != 0)
}

/// The state of the combo box `hwnd`.
pub(super) fn state(cpu: &mut Cpu, hwnd: u32) -> Combo {
    user(cpu)
        .window(hwnd)
        .map(|w| w.control.combo.clone())
        .unwrap_or_default()
}

fn with(cpu: &mut Cpu, hwnd: u32, f: impl FnOnce(&mut Combo)) {
    if let Some(w) = user(cpu).window_mut(hwnd) {
        f(&mut w.control.combo);
    }
}

/// Set or clear the state's bits `bits`.
pub(super) fn set_state(cpu: &mut Cpu, hwnd: u32, bits: u32, on: bool) {
    with(cpu, hwnd, |c| {
        if on {
            c.state |= bits;
        } else {
            c.state &= !bits;
        }
    });
}

pub(super) fn set_dropped_index(cpu: &mut Cpu, hwnd: u32, index: i32) {
    with(cpu, hwnd, |c| c.dropped_index = index);
}

pub(super) fn set_list(cpu: &mut Cpu, hwnd: u32, list: u32) {
    with(cpu, hwnd, |c| c.list = list);
}

/// The combo box's list, whether it has the extended UI, and whether
/// the list is dropped down.
pub fn state_of(cpu: &mut Cpu, hwnd: u32) -> (u32, bool, bool) {
    let c = state(cpu, hwnd);
    (c.list, c.state & CBF_EUI != 0, c.state & CBF_DROPPED != 0)
}

pub fn set_extended_ui(cpu: &mut Cpu, hwnd: u32, on: bool) {
    set_state(cpu, hwnd, CBF_EUI, on);
}

/// COMBO_FlipListbox: close the list if it is dropped down, or drop it
/// down; the calls go after `out`. Whether it dropped down.
pub(super) fn flip(cpu: &mut Cpu, hwnd: u32, ok: bool, button: bool, out: &mut Calls) -> bool {
    let Some(mut c) = Ctx::load(cpu, hwnd) else {
        return false;
    };
    let dropped = c.c.state & CBF_DROPPED == 0;
    c.flip(cpu, ok, button);
    out.append(c.finish(cpu));
    dropped
}

// A message's work: the state taken out of the window, put back when it
// is done.

struct Ctx {
    hwnd: u32,
    id: u32,
    c: Combo,
    out: Calls,
}

impl Ctx {
    fn load(cpu: &mut Cpu, hwnd: u32) -> Option<Ctx> {
        let w = user(cpu).window(hwnd)?;
        Some(Ctx {
            hwnd,
            id: w.id & 0xFFFF,
            c: w.control.combo.clone(),
            out: Calls::default(),
        })
    }

    /// Put the state back while the work goes on, for code that reads it
    /// from the window.
    fn put(&mut self, cpu: &mut Cpu) {
        if let Some(w) = user(cpu).window_mut(self.hwnd) {
            w.control.combo = self.c.clone();
        }
    }

    fn finish(mut self, cpu: &mut Cpu) -> Calls {
        self.put(cpu);
        self.out
    }

    fn store(self, cpu: &mut Cpu, result: i32) -> Option<u32> {
        let out = self.finish(cpu);
        run(cpu, COMBOBOX, out, result as u32)
    }

    fn notify(&mut self, code: u32) {
        let wp = self.id | code << 16;
        self.out.send(self.c.owner, WM_COMMAND, wp, self.hwnd);
    }

    fn enabled(&self, cpu: &mut Cpu) -> bool {
        user(cpu)
            .window(self.hwnd)
            .is_some_and(|w| w.style & WS_DISABLED == 0)
    }

    /// Clear the state's bits `bits` after the calls before.
    fn clear_after(&mut self, bits: u32) {
        if bits != 0 {
            self.out.send(self.hwnd, CBM_CLEAR, bits, 0);
        }
    }

    /// Invalidate `r` of the combo box, or all of it.
    fn invalidate(&mut self, r: Option<Rect>) {
        match r {
            Some(r) => {
                self.out.write(SCRATCH_DATA, &rect_words(r));
                let args = [self.hwnd, SCRATCH_DATA, 1];
                self.out.call_with("InvalidateRect", &args, 2, false);
            }
            None => self.out.call("InvalidateRect", &[self.hwnd, 0, 1]),
        }
    }

    fn repaint_button(&mut self) {
        let r = self.c.button_rect;
        self.invalidate(Some(r));
        self.out.call("UpdateWindow", &[self.hwnd]);
    }

    // The layout.

    /// CBGetTextAreaHeight, without asking the owner.
    fn text_area_height(&self, cpu: &mut Cpu) -> i32 {
        if self.c.edit_height != 0 {
            return self.c.edit_height;
        }
        measure(cpu, self.c.font).1 + 4
    }

    /// CBCalcPlacement: the text area, the button, and the list's width
    /// (and place, for CBS_SIMPLE).
    fn calc_placement(&mut self, cpu: &mut Cpu) {
        let client = client_of(user(cpu), self.hwnd);
        let mut edit = client.inflate(-X_BORDER, -Y_BORDER);
        edit.bottom = edit.top + self.text_area_height(cpu);
        let mut button = edit;
        let kind = self.c.kind();
        if kind == CBS_SIMPLE {
            button = Rect::new(button.left, button.top, 0, 0);
        } else {
            button.left = button.right - sysparams::metric(SM_CXVSCROLL);
            edit.right = button.left;
        }
        if kind != CBS_DROPDOWNLIST {
            edit = edit.inflate(-EDIT_PADDING, -EDIT_PADDING);
        }
        let mut lb = self.c.dropped_rect;
        if kind == CBS_SIMPLE {
            lb = client;
            lb.top = edit.bottom + Y_BORDER;
        } else if self.c.dropped_width < button.right + X_BORDER {
            lb.right = lb.left + button.right + X_BORDER;
        } else {
            lb.right = lb.left + self.c.dropped_width;
        }
        edit.right = edit.right.max(edit.left);
        self.c.text_rect = edit;
        self.c.button_rect = button;
        self.c.dropped_rect = lb;
    }

    /// CBForceDummyResize: SetWindowPos to the height of the text area,
    /// which WM_WINDOWPOSCHANGING keeps, to lay the combo box out.
    fn dummy_resize(&mut self, cpu: &mut Cpu) {
        let h = self.text_area_height(cpu) + 2 * Y_BORDER;
        let w = user(cpu).window(self.hwnd).map_or(0, |w| w.rect.width());
        let flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
        self.out.call(
            "SetWindowPos",
            &[self.hwnd, 0, 0, 0, w as u32, h as u32, flags],
        );
    }

    /// CBResetPos: the edit and the list where the placement says.
    fn reset_pos(&mut self, redraw: bool) {
        let drop = self.c.kind() != CBS_SIMPLE;
        let flags = SWP_NOZORDER | SWP_NOACTIVATE | if drop { SWP_NOREDRAW } else { 0 };
        if self.c.state & CBF_EDIT != 0 {
            let args = rect_pos(self.c.edit, self.c.text_rect, flags);
            self.out.call("SetWindowPos", &args);
        }
        let args = rect_pos(self.c.list, self.c.dropped_rect, flags);
        self.out.call("SetWindowPos", &args);
        if drop {
            if self.c.state & CBF_DROPPED != 0 {
                self.c.state &= !CBF_DROPPED;
                self.out.call("ShowWindow", &[self.c.list, SW_HIDE]);
            }
            if redraw && self.c.state & CBF_NOREDRAW == 0 {
                let flags = RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW;
                self.out.call("RedrawWindow", &[self.hwnd, 0, 0, flags]);
            }
        }
    }

    fn size(&mut self, cpu: &mut Cpu, redraw: bool) {
        self.calc_placement(cpu);
        self.reset_pos(redraw);
    }

    /// Lay out again after the text area's height changed.
    fn relayout(&mut self, cpu: &mut Cpu) {
        if self.c.kind() == CBS_SIMPLE {
            self.size(cpu, true);
        } else {
            self.dummy_resize(cpu);
        }
    }

    // The list and the edit.

    /// The text of the list's item `index`, empty for none.
    fn item_text(&self, cpu: &mut Cpu, index: i32) -> Vec<u8> {
        listbox::item(cpu, self.c.list, index)
            .map(|i| i.0)
            .unwrap_or_default()
    }

    /// CBUpdateLBox: the list's item the edit's text starts, selected if
    /// `select`, and none selected if not. The item, or -1.
    fn update_lbox(&mut self, cpu: &mut Cpu, select: bool) -> i32 {
        let text = user(cpu)
            .window(self.c.edit)
            .map(|w| w.text.clone())
            .unwrap_or_default();
        let list = self.c.list;
        let index = if text.is_empty() {
            -1
        } else {
            listbox::find_prefix(cpu, list, &text)
        };
        let at = index.max(0) as u32;
        self.out.send(
            list,
            LB_SETCURSEL,
            if select { index as u32 } else { u32::MAX },
            0,
        );
        self.out.send(list, LB_SETCARETINDEX, at, 0);
        self.out.send(list, LB_SETTOPINDEX, at, 0);
        index
    }

    /// The edit's text set without the combo box passing on its
    /// notifications.
    fn set_edit_text(&mut self, cpu: &mut Cpu, text: &[u8]) {
        let bits = CBF_NOEDITNOTIFY | CBF_NOLBSELECT;
        let had = self.c.state & bits;
        self.c.state |= bits;
        self.put(cpu);
        let calls = edit::set_window_text(cpu, self.c.edit, text);
        self.out.append(calls);
        // What was set before stays set.
        self.clear_after(bits & !had);
    }

    /// CBUpdateEdit: the list's item `index` into the edit.
    fn update_edit(&mut self, cpu: &mut Cpu, index: i32) {
        let text = if index >= 0 {
            self.item_text(cpu, index)
        } else {
            vec![]
        };
        if self.c.style & CBS_HASSTRINGS != 0 {
            self.set_edit_text(cpu, &text);
        }
        if self.c.state & CBF_FOCUSED != 0 {
            self.out.send(self.c.edit, EM_SETSEL, 0, u32::MAX);
        }
    }

    // Dropping down and closing the list.

    /// CBDropDown: show the list below (or above) the combo box. With
    /// `self_capture`, the combo box has just asked for the capture.
    fn drop_down(&mut self, cpu: &mut Cpu, self_capture: bool) {
        self.notify(CBN_DROPDOWN);
        self.c.state |= CBF_DROPPED;
        let list = self.c.list;
        if self.c.kind() == CBS_DROPDOWN {
            self.c.dropped_index = self.update_lbox(cpu, true);
            if self.c.state & CBF_CAPTURE == 0 && self.c.dropped_index >= 0 {
                let index = self.c.dropped_index;
                self.update_edit(cpu, index);
            }
        } else {
            self.c.dropped_index = listbox::cur_sel(cpu, list);
            self.out
                .send(list, LB_SETTOPINDEX, self.c.dropped_index.max(0) as u32, 0);
            self.out.send(list, LB_CARETON, 0, 0);
        }
        let state = user(cpu);
        let mut rect = state.screen_rect(self.hwnd);
        let list_height = state.screen_rect(list).height();
        let capture = state.input.capture;
        let d = self.c.dropped_rect;
        let mut height = (d.bottom - d.top).max(list_height);
        let n = listbox::count(cpu, list);
        if n > 0 {
            let ih = listbox::item_height(cpu, list);
            let h = ih * n;
            if h < height - Y_BORDER {
                height = h + Y_BORDER;
            }
            if height < h {
                if n < 5 {
                    height = (n + 1) * ih;
                } else if height < 6 * ih {
                    height = 6 * ih;
                }
            }
        }
        if rect.bottom + height >= sysparams::metric(SM_CYSCREEN) {
            rect.bottom = rect.top - height;
        }
        let args = [
            list,
            HWND_TOP,
            rect.left as u32,
            rect.bottom as u32,
            d.width() as u32,
            height as u32,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        ];
        self.out.call("SetWindowPos", &args);
        if self.c.state & CBF_NOREDRAW == 0 {
            let flags = RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW | RDW_NOCHILDREN;
            self.out.call("RedrawWindow", &[self.hwnd, 0, 0, flags]);
        }
        self.out.call("EnableWindow", &[list, 1]);
        if !self_capture && capture != self.hwnd {
            self.out.call("SetCapture", &[list]);
        }
    }

    /// CBRollUp: hide the list, the selection `ok` or not.
    fn roll_up(&mut self, cpu: &mut Cpu, ok: bool, button: bool) {
        self.notify(if ok { CBN_SELENDOK } else { CBN_SELENDCANCEL });
        if self.c.kind() == CBS_SIMPLE || self.c.state & CBF_DROPPED == 0 {
            return;
        }
        self.c.state &= !CBF_DROPPED;
        let list = self.c.list;
        self.out.call("ShowWindow", &[list, SW_HIDE]);
        if user(cpu).input.capture == list {
            self.out.call("ReleaseCapture", &[]);
        }
        let mut button = button;
        let rect = if self.c.kind() == CBS_DROPDOWN {
            self.c.button_rect
        } else {
            let r = if button {
                self.c.button_rect.union(&self.c.text_rect)
            } else {
                self.c.text_rect
            };
            button = true;
            r
        };
        if button && self.c.state & CBF_NOREDRAW == 0 {
            self.out.write(SCRATCH_DATA, &rect_words(rect));
            let flags = RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW | RDW_NOCHILDREN;
            let args = [self.hwnd, SCRATCH_DATA, 0, flags];
            self.out.call_with("RedrawWindow", &args, 2, false);
        }
        self.notify(CBN_CLOSEUP);
    }

    fn flip(&mut self, cpu: &mut Cpu, ok: bool, button: bool) {
        if self.c.state & CBF_DROPPED != 0 {
            self.roll_up(cpu, ok, button);
        } else {
            self.drop_down(cpu, false);
        }
    }

    // The focus.

    fn set_focus(&mut self) {
        if self.c.state & CBF_FOCUSED != 0 {
            return;
        }
        if self.c.kind() == CBS_DROPDOWNLIST {
            self.out.send(self.c.list, LB_CARETON, 0, 0);
        }
        if self.c.state & CBF_EDIT == 0 {
            let r = self.c.text_rect;
            self.invalidate(Some(r));
        }
        self.notify(CBN_SETFOCUS);
        self.c.state |= CBF_FOCUSED;
    }

    fn kill_focus(&mut self, cpu: &mut Cpu) {
        if self.c.state & CBF_FOCUSED == 0 {
            return;
        }
        self.roll_up(cpu, false, true);
        if self.c.kind() == CBS_DROPDOWNLIST {
            self.out.send(self.c.list, LB_CARETOFF, 0, 0);
        }
        self.c.state &= !CBF_FOCUSED;
        if self.c.state & CBF_EDIT == 0 {
            let r = self.c.text_rect;
            self.invalidate(Some(r));
        }
        self.notify(CBN_KILLFOCUS);
    }

    // Notifications from the edit and the list.

    fn command(&mut self, cpu: &mut Cpu, wp: u32, from: u32) {
        let code = wp >> 16;
        if self.c.state & CBF_EDIT != 0 && from == self.c.edit {
            match code {
                EN_SETFOCUS => self.set_focus(),
                EN_KILLFOCUS => self.kill_focus(cpu),
                EN_CHANGE => {
                    if self.c.state & CBF_NOLBSELECT != 0 {
                        self.c.state &= !CBF_NOLBSELECT;
                    } else {
                        let dropped = self.c.state & CBF_DROPPED != 0;
                        self.update_lbox(cpu, dropped);
                    }
                    if self.c.state & CBF_NOEDITNOTIFY == 0 {
                        self.notify(CBN_EDITCHANGE);
                    }
                }
                EN_UPDATE => {
                    if self.c.state & CBF_NOEDITNOTIFY == 0 {
                        self.notify(CBN_EDITUPDATE);
                    }
                }
                // EN_ERRSPACE.
                0x0500 => self.notify(CBN_ERRSPACE),
                _ => {}
            }
        } else if from == self.c.list {
            match code {
                LBN_ERRSPACE => self.notify(CBN_ERRSPACE),
                LBN_DBLCLK => self.notify(CBN_DBLCLK),
                LBN_SELCHANGE | LBN_SELCANCEL => {
                    if self.c.state & CBF_NOROLLUP == 0 {
                        self.roll_up(cpu, code == LBN_SELCHANGE, true);
                    } else {
                        self.c.state &= !CBF_NOROLLUP;
                    }
                    self.notify(CBN_SELCHANGE);
                    if code == LBN_SELCHANGE {
                        if self.c.state & CBF_EDIT != 0 {
                            let index = listbox::cur_sel(cpu, self.c.list);
                            self.c.state |= CBF_NOLBSELECT;
                            self.update_edit(cpu, index);
                            self.out.send(self.c.edit, EM_SETSEL, 0, u32::MAX);
                        } else {
                            let r = self.c.text_rect;
                            self.invalidate(Some(r));
                            self.out.call("UpdateWindow", &[self.hwnd]);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // The mouse.

    fn lbutton_down(&mut self, cpu: &mut Cpu, lp: u32) {
        let (x, y) = (lp as i16 as i32, (lp >> 16) as i16 as i32);
        let button = self.c.button_rect.contains(x, y);
        let kind = self.c.kind();
        if kind != CBS_DROPDOWNLIST && !(button && kind == CBS_DROPDOWN) {
            return;
        }
        self.c.state |= CBF_BUTTONDOWN;
        if self.c.state & CBF_DROPPED != 0 {
            // A click closes the list.
            self.c.state &= !CBF_BUTTONDOWN;
            self.roll_up(cpu, true, false);
            if self.c.state & CBF_CAPTURE != 0 {
                self.c.state &= !CBF_CAPTURE;
                self.out.call("ReleaseCapture", &[]);
            }
        } else {
            self.c.state |= CBF_CAPTURE;
            self.out.call("SetCapture", &[self.hwnd]);
            self.drop_down(cpu, true);
        }
        if button {
            self.repaint_button();
        }
    }

    fn lbutton_up(&mut self, cpu: &mut Cpu) {
        if self.c.state & CBF_CAPTURE != 0 {
            self.c.state &= !CBF_CAPTURE;
            if self.c.kind() == CBS_DROPDOWN {
                let index = self.update_lbox(cpu, true);
                if index >= 0 {
                    let had = self.c.state & CBF_NOLBSELECT;
                    self.c.state |= CBF_NOLBSELECT;
                    self.update_edit(cpu, index);
                    self.clear_after(CBF_NOLBSELECT & !had);
                }
            }
            self.out.call("ReleaseCapture", &[]);
            self.out.call("SetCapture", &[self.c.list]);
        }
        if self.c.state & CBF_BUTTONDOWN != 0 {
            self.c.state &= !CBF_BUTTONDOWN;
            self.repaint_button();
        }
    }

    /// The mouse moves with the capture: the button pops up when it
    /// leaves it, and the list tracks the mouse when it goes into it.
    fn mouse_move(&mut self, cpu: &mut Cpu, wp: u32, lp: u32) {
        let (x, y) = (lp as i16 as i32, (lp >> 16) as i16 as i32);
        if self.c.state & CBF_BUTTONDOWN != 0 && !self.c.button_rect.contains(x, y) {
            self.c.state &= !CBF_BUTTONDOWN;
            self.repaint_button();
        }
        let state = user(cpu);
        let list = self.c.list;
        let (cx, cy) = state.client_origin(self.hwnd);
        let (lx, ly) = state.client_origin(list);
        let (px, py) = (x + cx - lx, y + cy - ly);
        if client_of(state, list).contains(px, py) {
            self.c.state &= !CBF_CAPTURE;
            self.out.call("ReleaseCapture", &[]);
            if self.c.kind() == CBS_DROPDOWN {
                self.update_lbox(cpu, true);
            }
            self.out.send(list, WM_LBUTTONDOWN, wp, make_long(px, py));
        }
    }
}

/// A rectangle's words, as RECT has them.
fn rect_words(r: Rect) -> [u32; 4] {
    [r.left as u32, r.top as u32, r.right as u32, r.bottom as u32]
}

/// SetWindowPos's arguments that move `hwnd` to `r`.
fn rect_pos(hwnd: u32, r: Rect, flags: u32) -> [u32; 7] {
    let (w, h) = (r.width() as u32, r.height() as u32);
    [hwnd, 0, r.left as u32, r.top as u32, w, h, flags]
}

/// Send the list `msg`, and answer what it does.
fn forward(c: &mut Ctx, msg: u32, wp: u32, lp: u32) -> i32 {
    c.out.send_keep(c.c.list, msg, wp, lp);
    0
}

// The window procedure.

pub fn combobox_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, msg, wp, lp) = (args[0], args[1], args[2], args[3]);
    if msg == WM_NCCREATE {
        return Some(nc_create(cpu, hwnd, lp));
    }
    if !is_combo(cpu, hwnd) {
        return def_window_proc(cpu, args);
    }
    let mut c = Ctx::load(cpu, hwnd)?;
    let (list, edit_wnd) = (c.c.list, c.c.edit);
    let has_edit = c.c.state & CBF_EDIT != 0;
    let result: i32 = match msg {
        WM_NCDESTROY => {
            if c.c.kind() != CBS_SIMPLE && list != 0 {
                c.out.call("DestroyWindow", &[list]);
            }
            c.c.state &= !CBF_CREATED;
            c.out.call_with("DefWindowProcA", args, 0, true);
            0
        }
        WM_CREATE => {
            c.finish(cpu);
            return start(cpu, COMBOBOX, K_CREATE, vec![hwnd, lp, 0], SCRATCH);
        }
        WM_PAINT | WM_PRINTCLIENT => {
            let ctl = if c.enabled(cpu) {
                WM_CTLCOLOREDIT
            } else {
                WM_CTLCOLORSTATIC
            };
            let owner = c.c.owner;
            return paint_start(cpu, COMBOBOX, hwnd, wp, ctl, owner, &[0]);
        }
        WM_ERASEBKGND => 1,
        WM_GETDLGCODE => {
            let mut code = DLGC_WANTARROWS | DLGC_WANTCHARS;
            if lp != 0 {
                let m = read_words(cpu, lp, 3);
                let vk = m[2];
                if m[1] == WM_KEYDOWN
                    && (vk == VK_RETURN || vk == VK_ESCAPE)
                    && c.c.state & CBF_DROPPED != 0
                {
                    code |= DLGC_WANTMESSAGE;
                }
            }
            code as i32
        }
        WM_WINDOWPOSCHANGING => {
            // A drop-down combo box is as high as its text area; more
            // height makes the list longer.
            let flags = cpu.bus.guest_read_32(lp + 24);
            if c.c.kind() != CBS_SIMPLE && flags & SWP_NOSIZE == 0 {
                let h = c.text_area_height(cpu) + 2 * Y_BORDER;
                let cy = cpu.bus.guest_read_32(lp + 20) as i32;
                if cy > h {
                    c.c.dropped_rect.bottom = c.c.dropped_rect.top + cy - h;
                }
                write_words(cpu, lp + 20, &[h as u32]);
            }
            0
        }
        WM_WINDOWPOSCHANGED | WM_SIZE => {
            if list != 0 && c.c.state & CBF_NORESIZE == 0 {
                c.size(cpu, msg == WM_SIZE);
            }
            1
        }
        WM_SETFONT => {
            c.c.font = wp;
            if has_edit {
                c.out.send(edit_wnd, WM_SETFONT, wp, lp);
            }
            c.out.send(list, WM_SETFONT, wp, lp);
            c.relayout(cpu);
            1
        }
        WM_GETFONT => c.c.font as i32,
        WM_SETFOCUS => {
            if has_edit {
                c.out.call("SetFocus", &[edit_wnd]);
                // The first time, all the text is selected.
                if c.c.state & CBF_BEENFOCUSED == 0 {
                    c.out.send(edit_wnd, EM_SETSEL, 0, u32::MAX);
                    c.c.state |= CBF_BEENFOCUSED;
                }
            } else {
                c.set_focus();
            }
            1
        }
        WM_KILLFOCUS => {
            if wp == 0 || wp != edit_wnd && wp != list {
                c.kill_focus(cpu);
            }
            1
        }
        WM_COMMAND => {
            c.command(cpu, wp, lp);
            0
        }
        WM_GETTEXT => {
            if has_edit {
                forward_to(&mut c, edit_wnd, msg, wp, lp)
            } else if wp == 0 || lp == 0 {
                0
            } else {
                // The list's selected item, cut to fit.
                let index = listbox::cur_sel(cpu, list);
                match listbox::item(cpu, list, index) {
                    Some((text, _)) => {
                        let n = text.len().min(wp as usize - 1);
                        let mut bytes = text[..n].to_vec();
                        bytes.push(0);
                        cpu.bus.guest_write_bytes(lp, &bytes);
                        if text.len() >= wp as usize {
                            wp as i32
                        } else {
                            n as i32
                        }
                    }
                    None => {
                        cpu.bus.guest_write_bytes(lp, &[0]);
                        0
                    }
                }
            }
        }
        WM_SETTEXT | WM_GETTEXTLENGTH | WM_CLEAR => {
            if msg == WM_GETTEXTLENGTH && !has_edit {
                let index = listbox::cur_sel(cpu, list);
                listbox::item(cpu, list, index).map_or(0, |i| i.0.len() as i32)
            } else if has_edit {
                let had = c.c.state & CBF_NOEDITNOTIFY;
                c.c.state |= CBF_NOEDITNOTIFY;
                forward_to(&mut c, edit_wnd, msg, wp, lp);
                c.clear_after(CBF_NOEDITNOTIFY & !had);
                0
            } else {
                CB_ERR
            }
        }
        WM_CUT | WM_PASTE | WM_COPY => {
            if has_edit {
                forward_to(&mut c, edit_wnd, msg, wp, lp)
            } else {
                CB_ERR
            }
        }
        WM_DRAWITEM | WM_DELETEITEM | WM_COMPAREITEM | WM_MEASUREITEM => {
            // The owner sees the combo box's item, not the list's.
            write_words(cpu, lp, &[ODT_COMBOBOX, c.id]);
            let at = match msg {
                WM_DELETEITEM => Some(12),
                WM_DRAWITEM => Some(20),
                WM_COMPAREITEM => Some(8),
                _ => None,
            };
            if let Some(at) = at {
                write_words(cpu, lp + at, &[hwnd]);
            }
            let (owner, id) = (c.c.owner, c.id);
            forward_to(&mut c, owner, msg, id, lp)
        }
        WM_ENABLE => {
            if has_edit {
                c.out.call("EnableWindow", &[edit_wnd, wp]);
            }
            c.out.call("EnableWindow", &[list, wp]);
            c.invalidate(None);
            1
        }
        WM_SETREDRAW => {
            if wp != 0 {
                c.c.state &= !CBF_NOREDRAW;
            } else {
                c.c.state |= CBF_NOREDRAW;
            }
            if has_edit {
                c.out.send(edit_wnd, msg, wp, lp);
            }
            c.out.send(list, msg, wp, lp);
            0
        }
        WM_SYSKEYDOWN => {
            if (lp >> 16) & KEYDATA_ALT != 0 && (wp == VK_UP || wp == VK_DOWN) {
                c.flip(cpu, false, false);
            }
            0
        }
        WM_KEYDOWN | WM_CHAR => {
            let dropped = c.c.state & CBF_DROPPED != 0;
            if msg == WM_KEYDOWN && (wp == VK_RETURN || wp == VK_ESCAPE) && dropped {
                c.roll_up(cpu, wp == VK_RETURN, false);
                1
            } else if msg == WM_KEYDOWN && wp == VK_F4 && c.c.state & CBF_EUI == 0 {
                c.flip(cpu, false, false);
                1
            } else {
                let to = if has_edit { edit_wnd } else { list };
                forward_to(&mut c, to, msg, wp, lp)
            }
        }
        WM_LBUTTONDOWN => {
            if c.c.state & CBF_FOCUSED == 0 {
                // The rest once the focus is here.
                c.out.call("SetFocus", &[hwnd]);
                c.out.send(hwnd, CBM_CLICK, wp, lp);
            } else {
                c.lbutton_down(cpu, lp);
            }
            1
        }
        CBM_CLICK => {
            if c.c.state & CBF_FOCUSED != 0 {
                c.lbutton_down(cpu, lp);
            }
            0
        }
        CBM_CLEAR => {
            c.c.state &= !wp;
            0
        }
        WM_LBUTTONUP => {
            c.lbutton_up(cpu);
            1
        }
        WM_MOUSEMOVE => {
            if c.c.state & CBF_CAPTURE != 0 {
                c.mouse_move(cpu, wp, lp);
            }
            1
        }
        CB_ADDSTRING | CB_INSERTSTRING => {
            let (lb, wp) = if msg == CB_ADDSTRING {
                (LB_ADDSTRING, 0)
            } else {
                (LB_INSERTSTRING, wp)
            };
            let case = c.c.style & (CBS_LOWERCASE | CBS_UPPERCASE);
            if case != 0 && lp != 0 && c.c.style & CBS_HASSTRINGS != 0 {
                add_in_case(cpu, &mut c, lb, wp, lp, case)
            } else {
                forward(&mut c, lb, wp, lp)
            }
        }
        CB_DELETESTRING => forward(&mut c, LB_DELETESTRING, wp, 0),
        CB_SELECTSTRING => {
            let index = listbox::select_string_result(cpu, list, wp as i32, lp);
            c.out.send(list, LB_SELECTSTRING, wp, lp);
            if index >= 0 {
                if has_edit {
                    c.update_edit(cpu, index);
                } else {
                    let r = c.c.text_rect;
                    c.invalidate(Some(r));
                }
            }
            index
        }
        CB_FINDSTRING => forward(&mut c, LB_FINDSTRING, wp, lp),
        CB_FINDSTRINGEXACT => forward(&mut c, LB_FINDSTRINGEXACT, wp, lp),
        CB_SETITEMHEIGHT => {
            if wp as i32 == -1 {
                if (lp as i32) < 32768 {
                    c.c.edit_height = lp as i32 + 2;
                    c.relayout(cpu);
                    lp as i32
                } else {
                    CB_ERR
                }
            } else if c.c.owner_drawn() {
                forward(&mut c, LB_SETITEMHEIGHT, wp, lp)
            } else {
                CB_ERR
            }
        }
        CB_GETITEMHEIGHT => {
            if wp as i32 >= 0 {
                forward(&mut c, LB_GETITEMHEIGHT, wp, 0)
            } else {
                c.text_area_height(cpu)
            }
        }
        CB_RESETCONTENT => {
            c.out.send(list, LB_RESETCONTENT, 0, 0);
            if has_edit && c.c.style & CBS_HASSTRINGS != 0 {
                c.set_edit_text(cpu, &[]);
            } else {
                c.invalidate(None);
            }
            1
        }
        CB_INITSTORAGE => forward(&mut c, LB_INITSTORAGE, wp, lp),
        CB_GETHORIZONTALEXTENT => forward(&mut c, LB_GETHORIZONTALEXTENT, 0, 0),
        CB_SETHORIZONTALEXTENT => forward(&mut c, LB_SETHORIZONTALEXTENT, wp, 0),
        CB_GETTOPINDEX => forward(&mut c, LB_GETTOPINDEX, 0, 0),
        CB_GETLOCALE => forward(&mut c, LB_GETLOCALE, 0, 0),
        CB_SETLOCALE => forward(&mut c, LB_SETLOCALE, wp, 0),
        CB_GETDROPPEDWIDTH => {
            if c.c.dropped_width != 0 {
                c.c.dropped_width
            } else {
                c.c.dropped_rect.width()
            }
        }
        CB_SETDROPPEDWIDTH => {
            if c.c.kind() != CBS_SIMPLE && (wp as i32) < 32768 {
                c.c.dropped_width = wp as i32;
            }
            CB_ERR
        }
        CB_GETDROPPEDCONTROLRECT => {
            // The combo box's top left and the list's size.
            if lp != 0 {
                let r = user(cpu).screen_rect(hwnd);
                let d = c.c.dropped_rect;
                Rect::sized(r.left, r.top, d.width(), d.height()).write(cpu, lp);
            }
            CB_OKAY
        }
        CB_GETDROPPEDSTATE => (c.c.state & CBF_DROPPED != 0) as i32,
        CB_DIR => forward(&mut c, LB_DIR, wp, lp),
        CB_SHOWDROPDOWN => {
            if c.c.kind() != CBS_SIMPLE {
                let dropped = c.c.state & CBF_DROPPED != 0;
                if wp != 0 && !dropped {
                    c.drop_down(cpu, false);
                } else if wp == 0 && dropped {
                    c.roll_up(cpu, false, true);
                }
            }
            1
        }
        CB_GETCOUNT => forward(&mut c, LB_GETCOUNT, 0, 0),
        CB_GETCURSEL => forward(&mut c, LB_GETCURSEL, 0, 0),
        CB_SETCURSEL => {
            let r = listbox::set_cur_sel_result(cpu, list, wp as i32);
            c.out.send(list, LB_SETCURSEL, wp, 0);
            if r >= 0 {
                c.out.send(list, LB_SETTOPINDEX, wp, 0);
            }
            // The list sends no LBN_SELCHANGE for it.
            if has_edit {
                c.update_edit(cpu, wp as i32);
            } else {
                let r = c.c.text_rect;
                c.invalidate(Some(r));
            }
            c.c.state &= !CBF_SELCHANGE;
            r
        }
        CB_GETLBTEXT => forward(&mut c, LB_GETTEXT, wp, lp),
        CB_GETLBTEXTLEN => forward(&mut c, LB_GETTEXTLEN, wp, 0),
        CB_GETITEMDATA => forward(&mut c, LB_GETITEMDATA, wp, 0),
        CB_SETITEMDATA => forward(&mut c, LB_SETITEMDATA, wp, lp),
        CB_GETEDITSEL => {
            if has_edit {
                forward_to(&mut c, edit_wnd, EM_GETSEL, wp, lp)
            } else {
                CB_ERR
            }
        }
        CB_SETEDITSEL => {
            if has_edit {
                let (s, e) = (lp as i16 as i32 as u32, (lp >> 16) as i16 as i32 as u32);
                forward_to(&mut c, edit_wnd, EM_SETSEL, s, e)
            } else {
                CB_ERR
            }
        }
        CB_SETEXTENDEDUI => {
            if c.c.kind() == CBS_SIMPLE {
                CB_ERR
            } else {
                if wp != 0 {
                    c.c.state |= CBF_EUI;
                } else {
                    c.c.state &= !CBF_EUI;
                }
                CB_OKAY
            }
        }
        CB_GETEXTENDEDUI => (c.c.state & CBF_EUI != 0) as i32,
        CB_GETCOMBOBOXINFO => {
            if lp == 0 || cpu.bus.guest_read_32(lp) < 52 {
                0
            } else {
                // STATE_SYSTEM_PRESSED and STATE_SYSTEM_INVISIBLE.
                let mut button_state = 0;
                if c.c.state & CBF_BUTTONDOWN != 0 {
                    button_state |= 0x0008;
                }
                if c.c.button_rect.is_empty() {
                    button_state |= 0x8000;
                }
                let mut words = rect_words(c.c.text_rect).to_vec();
                words.extend(rect_words(c.c.button_rect));
                words.extend([button_state, hwnd, edit_wnd, list]);
                write_words(cpu, lp + 4, &words);
                1
            }
        }
        CB_LIMITTEXT if has_edit => forward_to(&mut c, edit_wnd, EM_LIMITTEXT, wp, lp),
        _ => {
            c.finish(cpu);
            return def_window_proc(cpu, args);
        }
    };
    c.store(cpu, result)
}

/// Send `to` the message, and answer what it does.
fn forward_to(c: &mut Ctx, to: u32, msg: u32, wp: u32, lp: u32) -> i32 {
    c.out.send_keep(to, msg, wp, lp);
    0
}

/// CB_ADDSTRING or CB_INSERTSTRING with CBS_LOWERCASE or CBS_UPPERCASE:
/// the list gets the string in the case, a copy in the scratch space
/// when it fits there and the program's own string when not.
fn add_in_case(cpu: &mut Cpu, c: &mut Ctx, lb: u32, wp: u32, lp: u32, case: u32) -> i32 {
    let mut text = bytes_at(cpu, lp).unwrap_or_default();
    if case & CBS_LOWERCASE != 0 {
        text.make_ascii_lowercase();
    } else {
        text.make_ascii_uppercase();
    }
    text.push(0);
    if text.len() > (SCRATCH - SCRATCH_DATA) as usize {
        cpu.bus.guest_write_bytes(lp, &text);
        return forward(c, lb, wp, lp);
    }
    text.resize(text.len().next_multiple_of(4), 0);
    let words: Vec<u32> = text
        .chunks(4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    c.out.write(SCRATCH_DATA, &words);
    c.out.send_struct(c.c.list, lb, wp, SCRATCH_DATA, true);
    0
}

/// WM_NCCREATE: the state, and the window without a border, scroll bars
/// (some programs give them) or a client edge.
fn nc_create(cpu: &mut Cpu, hwnd: u32, cs: u32) -> u32 {
    let style = cpu.bus.guest_read_32(cs + 32) & !(WS_BORDER | WS_HSCROLL | WS_VSCROLL);
    let Some(w) = user(cpu).window_mut(hwnd) else {
        return 0;
    };
    w.style = style;
    w.ex_style &= !WS_EX_CLIENTEDGE;
    let mut c = Combo {
        style,
        state: CBF_CREATED,
        ..Combo::default()
    };
    if style & (CBS_OWNERDRAWFIXED | CBS_OWNERDRAWVARIABLE) == 0 {
        c.style |= CBS_HASSTRINGS;
    }
    if w.ex_style & WS_EX_NOPARENTNOTIFY == 0 {
        c.state |= CBF_NOTIFY;
    }
    w.control.combo = c;
    1
}

pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match super::kind(t) {
        K_CALLS => calls_step(cpu, t, value),
        K_PAINT => paint_step(cpu, t, value),
        _ => create_step(cpu, t, value),
    }
}

/// Write the MEASUREITEMSTRUCT of an owner-drawn combo box's item `item`
/// at `at`.
fn measure_item(cpu: &mut Cpu, at: u32, id: u32, item: i32, width: i32, height: i32) {
    let words = [
        ODT_COMBOBOX,
        id,
        item as u32,
        width as u32,
        height as u32,
        0,
    ];
    write_words(cpu, at, &words);
}

/// WM_CREATE's task, COMBO_Create: an owner-drawn combo box asks its
/// owner the height of its text area (and of a fixed list's items), then
/// it makes its list and its edit. Its variables: the combo box, the
/// CREATESTRUCT, and whether the second WM_MEASUREITEM went (later where
/// the calls of the last step are).
fn create_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let hwnd = t.var(1);
    let Some(mut c) = Ctx::load(cpu, hwnd) else {
        return Next::Done(u32::MAX);
    };
    let mis = t.scratch + SCRATCH_DATA;
    let send = address_of("SendMessageA");
    let width = client_of(user(cpu), hwnd).right;
    match t.step {
        0 => {
            if c.c.kind() == 0 {
                c.c.style |= CBS_SIMPLE;
            }
            if c.c.kind() != CBS_DROPDOWNLIST {
                c.c.state |= CBF_EDIT;
            }
            c.c.owner = cpu.bus.guest_read_32(t.var(2) + 12);
            if !c.c.owner_drawn() {
                c.finish(cpu);
                return Next::call(0, &[], 2);
            }
            let height = c.text_area_height(cpu);
            measure_item(cpu, mis, c.id, -1, width, height - 6);
            let args = [c.c.owner, WM_MEASUREITEM, c.id, mis];
            c.finish(cpu);
            Next::call(send, &args, 1)
        }
        1 => {
            let original = c.text_area_height(cpu);
            c.c.edit_height = 6 + cpu.bus.guest_read_32(mis + 16) as i32;
            if c.c.style & CBS_OWNERDRAWFIXED == 0 {
                c.finish(cpu);
                return Next::call(0, &[], 2);
            }
            measure_item(cpu, mis, c.id, 0, width, original);
            t.set(3, 1);
            let args = [c.c.owner, WM_MEASUREITEM, c.id, mis];
            c.finish(cpu);
            Next::call(send, &args, 2)
        }
        2 => {
            if t.var(3) != 0 {
                c.c.fixed_height = cpu.bus.guest_read_32(mis + 16) as i32;
            }
            create_list(cpu, t, c)
        }
        3 => {
            if value == 0 {
                return Next::Done(u32::MAX);
            }
            c.c.list = value;
            if c.c.state & CBF_EDIT == 0 {
                c.finish(cpu);
                return Next::call(0, &[], 4);
            }
            create_edit(cpu, t, c)
        }
        4 => {
            if c.c.state & CBF_EDIT != 0 {
                if value == 0 {
                    return Next::Done(u32::MAX);
                }
                c.c.edit = value;
            }
            let list = c.c.list;
            let simple = c.c.kind() == CBS_SIMPLE;
            c.finish(cpu);
            if simple {
                return Next::Done(0);
            }
            // The list goes on the desktop, and the combo box gets the
            // height of its text area.
            Next::call(address_of("SetParent"), &[list, 0], 5)
        }
        5 => {
            c.dummy_resize(cpu);
            let out = c.finish(cpu);
            let base = ops_push(t, out);
            t.set(3, base as u32);
            Next::call(0, &[], 6)
        }
        _ => {
            let base = t.var(3) as usize;
            ops_step(cpu, t, base, value).unwrap_or(Next::Done(0))
        }
    }
}

/// COMBO_Create's list: placed, then made, with the combo box as its
/// creation data.
fn create_list(cpu: &mut Cpu, t: &mut Task, mut c: Ctx) -> Next {
    let hwnd = c.hwnd;
    let style = cpu.bus.guest_read_32(t.var(2) + 32);
    c.c.state &= !CBF_MEASUREITEM;
    c.c.dropped_rect = client_of(user(cpu), hwnd);
    c.calc_placement(cpu);
    let kind = c.c.kind();
    if kind != CBS_SIMPLE {
        let (x, y) = user(cpu).client_origin(hwnd);
        let d = &mut c.c.dropped_rect;
        d.top = c.c.text_rect.bottom + Y_BORDER;
        d.bottom = d.bottom.max(d.top);
        d.right = d.right.max(d.left);
        *d = d.offset(x, y);
    }
    let mut lb_style = LBS_NOTIFY | LBS_COMBOBOX | WS_BORDER | WS_CLIPSIBLINGS | WS_CHILD;
    lb_style |= style & (WS_VSCROLL | CBS_OWNERDRAWFIXED | CBS_OWNERDRAWVARIABLE);
    for (cb, lb) in [
        (CBS_SORT, LBS_SORT),
        (CBS_HASSTRINGS, LBS_HASSTRINGS),
        (CBS_NOINTEGRALHEIGHT, LBS_NOINTEGRALHEIGHT),
        (CBS_DISABLENOSCROLL, LBS_DISABLENOSCROLL),
    ] {
        if c.c.style & cb != 0 {
            lb_style |= lb;
        }
    }
    let ex = if kind == CBS_SIMPLE {
        // A child with a client edge instead of the border.
        lb_style = lb_style & !WS_BORDER | WS_VISIBLE;
        WS_EX_CLIENTEDGE
    } else {
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW
    };
    let class = t.scratch + SCRATCH_DATA;
    cpu.bus.guest_write_bytes(class, b"ComboLBox\0");
    let d = c.c.dropped_rect;
    let instance = user(cpu).window(hwnd).map_or(0, |w| w.instance);
    let args = [
        ex,
        class,
        0,
        lb_style,
        d.left as u32,
        d.top as u32,
        d.width() as u32,
        d.height() as u32,
        hwnd,
        ID_CB_LISTBOX,
        instance,
        hwnd,
    ];
    c.finish(cpu);
    Next::call(address_of("CreateWindowExA"), &args, 3)
}

/// COMBO_Create's edit, in the text area.
fn create_edit(cpu: &mut Cpu, t: &mut Task, c: Ctx) -> Next {
    let hwnd = c.hwnd;
    let mut style = WS_CHILD | WS_VISIBLE | ES_NOHIDESEL | ES_COMBO;
    let cs = c.c.style;
    if cs & CBS_OEMCONVERT != 0 {
        style |= ES_OEMCONVERT;
    }
    if cs & CBS_AUTOHSCROLL != 0 {
        style |= ES_AUTOHSCROLL;
    }
    if cs & CBS_LOWERCASE != 0 {
        style |= ES_LOWERCASE;
    } else if cs & CBS_UPPERCASE != 0 {
        style |= ES_UPPERCASE;
    }
    if !c.enabled(cpu) {
        style |= WS_DISABLED;
    }
    let class = t.scratch + SCRATCH_DATA;
    cpu.bus.guest_write_bytes(class, b"Edit\0");
    let r = c.c.text_rect;
    let instance = user(cpu).window(hwnd).map_or(0, |w| w.instance);
    let args = [
        0,
        class,
        0,
        style,
        r.left as u32,
        r.top as u32,
        r.width() as u32,
        r.height() as u32,
        hwnd,
        ID_CB_EDIT,
        instance,
        0,
    ];
    c.finish(cpu);
    Next::call(address_of("CreateWindowExA"), &args, 4)
}

// Painting.

/// The WM_PAINT task: the border, the button and the text area, then the
/// owner's item in calls whose list its own variable gives.
fn paint_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if let Some(next) = paint_head(t, value) {
        return next;
    }
    match t.step {
        3 => {
            let (hwnd, hdc, brush) = (t.var(P_HWND), t.var(P_HDC), t.var(P_BRUSH));
            let calls = draw(cpu, hwnd, hdc, brush, t.scratch + SCRATCH_DATA);
            let base = ops_push(t, calls);
            t.set(P_OWN, base as u32);
            Next::call(0, &[], 4)
        }
        4 => {
            let base = t.var(P_OWN) as usize;
            if let Some(next) = ops_step(cpu, t, base, value) {
                return next;
            }
            paint_end(t, 5)
        }
        _ => Next::Done(0),
    }
}

/// Draw the combo box `hwnd` on `hdc` with `brush`, as COMBO_Paint does;
/// `at` is a place for a rectangle in the task's scratch space. The calls
/// that have the owner draw its item.
fn draw(cpu: &mut Cpu, hwnd: u32, hdc: u32, brush: u32, at: u32) -> Calls {
    let mut out = Calls::default();
    let state = user(cpu);
    let Some(w) = state.window(hwnd) else {
        return out;
    };
    let (id, enabled) = (w.id & 0xFFFF, w.style & WS_DISABLED == 0);
    let c = w.control.combo.clone();
    let client = client_of(state, hwnd);
    if c.state & CBF_NOREDRAW != 0 {
        return out;
    }
    let brush = default_brush(cpu, hdc, brush, COLOR_WINDOW);
    if !enabled {
        let gray = user(cpu).color(COLOR_GRAYTEXT);
        dc::set_text_color(cpu, &[hdc, gray]);
    }
    if c.state & CBF_EDIT == 0 {
        paint::fill(cpu, hdc, c.text_rect, brush);
    }
    let border = if c.kind() != CBS_SIMPLE {
        client
    } else {
        c.text_rect
            .inflate(EDIT_PADDING + X_BORDER, EDIT_PADDING + Y_BORDER)
    };
    paint::edge(cpu, hdc, border, EDGE_SUNKEN, BF_RECT);
    if !c.button_rect.is_empty() {
        let mut kind = DFCS_SCROLLCOMBOBOX;
        if c.state & CBF_BUTTONDOWN != 0 {
            kind |= DFCS_PUSHED;
        }
        if !enabled {
            kind |= DFCS_INACTIVE;
        }
        c.button_rect.write(cpu, at);
        paint::draw_frame_control(cpu, &[hdc, at, DFC_SCROLL, kind]);
    }
    if c.kind() != CBS_DROPDOWNLIST {
        // The edit's padding.
        c.text_rect
            .inflate(EDIT_PADDING, EDIT_PADDING)
            .write(cpu, at);
        let window = sysparams::brush(cpu, COLOR_WINDOW);
        paint::frame_rect(cpu, &[hdc, at, window]);
    }
    if c.state & CBF_EDIT == 0 {
        draw_text(cpu, &c, (hwnd, id, enabled), hdc, at, &mut out);
    }
    out
}

/// CBPaintText: a CBS_DROPDOWNLIST's text, the list's selected item, of
/// the combo box (`hwnd`, its id, whether it is enabled).
fn draw_text(
    cpu: &mut Cpu,
    c: &Combo,
    (hwnd, id, enabled): (u32, u32, bool),
    hdc: u32,
    at: u32,
    out: &mut Calls,
) {
    let index = listbox::cur_sel(cpu, c.list);
    let item = listbox::item(cpu, c.list, index);
    if item.is_none() && !c.owner_drawn() {
        return;
    }
    let (text, data) = item.unwrap_or_default();
    select_font(cpu, hdc, c.font);
    let r = c.text_rect.inflate(-1, -1);
    let focused = c.state & CBF_FOCUSED != 0 && c.state & CBF_DROPPED == 0;
    if c.owner_drawn() {
        // The owner highlights the item.
        let mut state = ODS_COMBOBOXEDIT;
        if focused {
            state |= ODS_SELECTED | ODS_FOCUS;
        }
        if !enabled {
            state |= ODS_DISABLED;
        }
        clip(cpu, hdc, r);
        let mut dis = vec![
            ODT_COMBOBOX,
            id,
            index as u32,
            ODA_DRAWENTIRE,
            state,
            hwnd,
            hdc,
        ];
        dis.extend(rect_words(r));
        dis.push(data);
        out.write(SCRATCH_DATA, &dis);
        out.send_struct(c.owner, WM_DRAWITEM, id, SCRATCH_DATA, false);
        return;
    }
    if focused {
        let b = sysparams::brush(cpu, COLOR_HIGHLIGHT);
        paint::fill(cpu, hdc, r, b);
        let (hb, hf) = (
            user(cpu).color(COLOR_HIGHLIGHT),
            user(cpu).color(COLOR_HIGHLIGHTTEXT),
        );
        dc::set_bk_color(cpu, &[hdc, hb]);
        dc::set_text_color(cpu, &[hdc, hf]);
    }
    let options = ETO_OPAQUE | ETO_CLIPPED;
    font::draw_text(
        cpu,
        hdc,
        (r.left + 1, r.top + 1),
        options,
        Some(r),
        &text,
        None,
    );
    if focused {
        r.write(cpu, at);
        paint::draw_focus_rect(cpu, &[hdc, at]);
    }
}

crate::state_fields!(Combo {
    style,
    owner,
    edit,
    list,
    state,
    font,
    text_rect,
    button_rect,
    dropped_rect,
    dropped_index,
    fixed_height,
    dropped_width,
    edit_height
});
