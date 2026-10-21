//! The Edit class: single-line and multi-line edit controls, after
//! Wine's edit.c, as Windows 95 has them, with the ANSI messages.
//!
//! The text and the rest of what the control keeps is in `Edit`. A
//! message does its work on it at once; what it then has to call
//! (notifications to the parent, SetFocus, SetScrollInfo) waits in a
//! list, `Calls`, that the controls' task goes through when the work is
//! done, so the program always sees the control whole. A change
//! invalidates the window, and WM_PAINT draws all of it.
//!
//! The list box and the combo box use `Calls` and the start of the
//! WM_PAINT task (`paint_head`) as well.

use super::super::class::is_class;
use super::super::defwnd::def_window_proc;
use super::super::message::{
    WM_CHAR, WM_CLEAR, WM_CLOSE, WM_COMMAND, WM_COPY, WM_CREATE, WM_CTLCOLOREDIT,
    WM_CTLCOLORSTATIC, WM_CUT, WM_ENABLE, WM_ERASEBKGND, WM_GETDLGCODE, WM_GETFONT, WM_GETTEXT,
    WM_GETTEXTLENGTH, WM_HSCROLL, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MOUSEMOVE, WM_NCCREATE, WM_NCDESTROY, WM_NEXTDLGCTL, WM_PAINT,
    WM_PASTE, WM_PRINTCLIENT, WM_SETFOCUS, WM_SETFONT, WM_SETTEXT, WM_SIZE, WM_STYLECHANGED,
    WM_STYLECHANGING, WM_SYSKEYDOWN, WM_UNDO, WM_USER, WM_VSCROLL,
};
use super::super::sysparams::{
    self, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, COLOR_WINDOW, COLOR_WINDOWFRAME,
    COLOR_WINDOWTEXT,
};
use super::super::window::{WS_BORDER, WS_DISABLED, WS_HSCROLL, WS_VSCROLL};
use super::super::{
    State, address_of, bytes_at, copy_out, input, make_long, paint, point_of, read_words, user,
    write_words,
};
use super::{EDIT, start};
use crate::cpu::Cpu;
use crate::win32::gdi32::{self, Rect, dc, font};
use crate::win32::task::{Next, Task};
use std::borrow::Cow;

// The styles.
pub const ES_CENTER: u32 = 0x0001;
pub const ES_RIGHT: u32 = 0x0002;
pub const ES_MULTILINE: u32 = 0x0004;
pub const ES_UPPERCASE: u32 = 0x0008;
pub const ES_LOWERCASE: u32 = 0x0010;
pub const ES_PASSWORD: u32 = 0x0020;
pub const ES_AUTOVSCROLL: u32 = 0x0040;
pub const ES_AUTOHSCROLL: u32 = 0x0080;
pub const ES_NOHIDESEL: u32 = 0x0100;
/// The edit of a combo box.
pub const ES_COMBO: u32 = 0x0200;
pub const ES_READONLY: u32 = 0x0800;
pub const ES_WANTRETURN: u32 = 0x1000;
pub const ES_NUMBER: u32 = 0x2000;

// The messages.
pub const EM_GETSEL: u32 = 0x00B0;
pub const EM_SETSEL: u32 = 0x00B1;
pub const EM_GETRECT: u32 = 0x00B2;
pub const EM_SETRECT: u32 = 0x00B3;
pub const EM_SETRECTNP: u32 = 0x00B4;
pub const EM_SCROLL: u32 = 0x00B5;
pub const EM_LINESCROLL: u32 = 0x00B6;
pub const EM_SCROLLCARET: u32 = 0x00B7;
pub const EM_GETMODIFY: u32 = 0x00B8;
pub const EM_SETMODIFY: u32 = 0x00B9;
pub const EM_GETLINECOUNT: u32 = 0x00BA;
pub const EM_LINEINDEX: u32 = 0x00BB;
pub const EM_SETHANDLE: u32 = 0x00BC;
pub const EM_GETHANDLE: u32 = 0x00BD;
pub const EM_GETTHUMB: u32 = 0x00BE;
pub const EM_LINELENGTH: u32 = 0x00C1;
pub const EM_REPLACESEL: u32 = 0x00C2;
pub const EM_GETLINE: u32 = 0x00C4;
pub const EM_LIMITTEXT: u32 = 0x00C5;
pub const EM_CANUNDO: u32 = 0x00C6;
pub const EM_UNDO: u32 = 0x00C7;
pub const EM_FMTLINES: u32 = 0x00C8;
pub const EM_LINEFROMCHAR: u32 = 0x00C9;
pub const EM_SETTABSTOPS: u32 = 0x00CB;
pub const EM_SETPASSWORDCHAR: u32 = 0x00CC;
pub const EM_EMPTYUNDOBUFFER: u32 = 0x00CD;
pub const EM_GETFIRSTVISIBLELINE: u32 = 0x00CE;
pub const EM_SETREADONLY: u32 = 0x00CF;
pub const EM_SETWORDBREAKPROC: u32 = 0x00D0;
pub const EM_GETWORDBREAKPROC: u32 = 0x00D1;
pub const EM_GETPASSWORDCHAR: u32 = 0x00D2;
pub const EM_SETMARGINS: u32 = 0x00D3;
pub const EM_GETMARGINS: u32 = 0x00D4;
pub const EM_GETLIMITTEXT: u32 = 0x00D5;
pub const EM_POSFROMCHAR: u32 = 0x00D6;
pub const EM_CHARFROMPOS: u32 = 0x00D7;

// The notifications.
pub const EN_SETFOCUS: u32 = 0x0100;
pub const EN_KILLFOCUS: u32 = 0x0200;
pub const EN_CHANGE: u32 = 0x0300;
pub const EN_UPDATE: u32 = 0x0400;
pub const EN_MAXTEXT: u32 = 0x0501;
pub const EN_HSCROLL: u32 = 0x0601;
pub const EN_VSCROLL: u32 = 0x0602;

// WM_GETDLGCODE's answers.
pub const DLGC_WANTARROWS: u32 = 0x0001;
pub const DLGC_WANTALLKEYS: u32 = 0x0004;
pub const DLGC_WANTMESSAGE: u32 = 0x0004;
pub const DLGC_HASSETSEL: u32 = 0x0008;
pub const DLGC_WANTCHARS: u32 = 0x0080;

/// DM_GETDEFID, and the high word of its answer when there is a default
/// button.
pub const DM_GETDEFID: u32 = WM_USER;
pub const DC_HASDEFID: u32 = 0x534B;

// The virtual keys.
pub const VK_BACK: u32 = 0x08;
pub const VK_TAB: u32 = 0x09;
pub const VK_RETURN: u32 = 0x0D;
pub const VK_SHIFT: u32 = 0x10;
pub const VK_CONTROL: u32 = 0x11;
pub const VK_MENU: u32 = 0x12;
pub const VK_ESCAPE: u32 = 0x1B;
pub const VK_SPACE: u32 = 0x20;
pub const VK_PRIOR: u32 = 0x21;
pub const VK_NEXT: u32 = 0x22;
pub const VK_END: u32 = 0x23;
pub const VK_HOME: u32 = 0x24;
pub const VK_LEFT: u32 = 0x25;
pub const VK_UP: u32 = 0x26;
pub const VK_RIGHT: u32 = 0x27;
pub const VK_DOWN: u32 = 0x28;
pub const VK_INSERT: u32 = 0x2D;
pub const VK_DELETE: u32 = 0x2E;
pub const VK_F4: u32 = 0x73;

/// The mouse messages' key flags.
pub const MK_SHIFT: u32 = 0x0004;
pub const MK_CONTROL: u32 = 0x0008;

// The scroll bars' requests and SCROLLINFO.
pub const SB_HORZ: u32 = 0;
pub const SB_VERT: u32 = 1;
pub const SB_LINEUP: u32 = 0;
pub const SB_LINEDOWN: u32 = 1;
pub const SB_PAGEUP: u32 = 2;
pub const SB_PAGEDOWN: u32 = 3;
pub const SB_THUMBPOSITION: u32 = 4;
pub const SB_THUMBTRACK: u32 = 5;
pub const SB_TOP: u32 = 6;
pub const SB_BOTTOM: u32 = 7;
pub const SB_ENDSCROLL: u32 = 8;
pub const SIF_RANGE: u32 = 0x0001;
pub const SIF_PAGE: u32 = 0x0002;
pub const SIF_POS: u32 = 0x0004;
pub const SIF_DISABLENOSCROLL: u32 = 0x0008;

/// CF_TEXT, and GlobalAlloc's flags for the clipboard's text and
/// EM_GETHANDLE's buffer.
const CF_TEXT: u32 = 1;
const GMEM_MOVEABLE_SHARE: u32 = 0x2002;
const LMEM_MOVEABLE_ZERO: u32 = 0x0042;

/// The text's limit until EM_LIMITTEXT.
const BUFLIMIT_INITIAL: u32 = 30000;
/// How far the text scrolls sideways, as a part of the format width.
const HSCROLL_FRACTION: i32 = 3;
/// EM_SETMARGINS's flags.
const EC_LEFTMARGIN: u32 = 0x0001;
const EC_RIGHTMARGIN: u32 = 0x0002;
const EC_USEFONTINFO: u32 = 0xFFFF;
const GWL_STYLE: u32 = (-16i32) as u32;
const SIZE_RESTORED: u32 = 0;
const SIZE_MAXIMIZED: u32 = 2;

// What the control keeps besides the style (`Edit::flags`).
const EF_MODIFIED: u32 = 0x0001;
const EF_FOCUSED: u32 = 0x0002;
const EF_UPDATE: u32 = 0x0004;
const EF_AFTER_WRAP: u32 = 0x0080;
/// EM_FMTLINES(TRUE): WM_GETTEXT gives soft line breaks.
const EF_SOFT_BREAKS: u32 = 0x0100;
/// WM_GETDLGCODE came with a message: it is in a dialog.
const EF_DIALOGMODE: u32 = 0x0400;
/// It has the capture from a click.
const EF_CAPTURE: u32 = 0x1000;
const EF_DISABLED: u32 = 0x2000;
/// WM_NCCREATE has been: the state is there.
const EF_CREATED: u32 = 0x4000;

// The controls' task kinds of the three modules: each module's `step`
// sends these to the functions here.
pub(super) const K_CALLS: u32 = 0;
pub(super) const K_PAINT: u32 = 1;
const K_RETURN: u32 = 2;

/// The scratch space of a task that writes structures for the calls it
/// makes: a PAINTSTRUCT, then room for the others.
pub(super) const SCRATCH: u32 = 128;
/// Where the structures besides the PAINTSTRUCT go in it.
pub(super) const SCRATCH_DATA: u32 = 64;

/// What an edit control keeps.
#[derive(Clone, Debug, Default)]
pub struct Edit {
    /// The style as the control has it, which NCCREATE fixes.
    pub style: u32,
    /// The window its notifications go to.
    pub parent: u32,
    pub text: Vec<u8>,
    pub limit: u32,
    /// The selection: where it started and where it ends, at the caret.
    pub sel_start: u32,
    pub sel_end: u32,
    pub flags: u32,
    pub font: u32,
    pub line_height: i32,
    pub char_width: i32,
    /// Each byte's width in the font.
    pub widths: Vec<i32>,
    /// The text's scroll: in pixels for a multi-line edit, in characters
    /// for a single-line one; and the first line shown.
    pub x_offset: i32,
    pub y_offset: i32,
    pub left_margin: i32,
    pub right_margin: i32,
    /// Where the text goes, in client coordinates.
    pub format: Rect,
    pub password: u32,
    /// The undo: what was deleted, where, and how much was inserted.
    pub undo_text: Vec<u8>,
    pub undo_pos: u32,
    pub undo_count: u32,
    /// EM_SETTABSTOPS's stops, in dialog units.
    pub tabs: Vec<i32>,
    /// EM_SETWORDBREAKPROC's procedure, which is kept but not called.
    pub word_break: u32,
    /// EM_GETHANDLE's buffer.
    pub handle: u32,
    /// The scroll bars' last range, page and position sent.
    pub bars: Vec<i32>,
}

// Calls a message makes after its work.

const OP_CALL: u32 = 1;
const OP_WRITE: u32 = 2;

/// The calls a message's handler makes when its work is done, through
/// the controls' task, in order.
#[derive(Default)]
pub(super) struct Calls {
    ops: Vec<u32>,
    scratch: bool,
}

impl Calls {
    /// Call USER32's `name` with `args`; with `mask`, the arguments whose
    /// bits are set are places in the scratch space instead.
    pub fn call_with(&mut self, name: &str, args: &[u32], mask: u32, keep: bool) {
        self.op(name, args, mask, 0, keep);
    }

    /// `call_with`, and the arguments whose bits are set in `read` are
    /// the words at those places of the scratch space when it is made.
    fn op(&mut self, name: &str, args: &[u32], mask: u32, read: u32, keep: bool) {
        self.ops.push(OP_CALL);
        self.ops.push(address_of(name));
        let head = args.len() as u32 | (keep as u32) << 8 | read << 9 | mask << 16;
        self.ops.push(head);
        self.ops.extend_from_slice(args);
        if mask | read != 0 {
            self.scratch = true;
        }
    }

    pub fn call(&mut self, name: &str, args: &[u32]) {
        self.call_with(name, args, 0, false);
    }

    pub fn send(&mut self, hwnd: u32, msg: u32, wp: u32, lp: u32) {
        self.call("SendMessageA", &[hwnd, msg, wp, lp]);
    }

    /// Send a message whose answer the handler returns.
    pub fn send_keep(&mut self, hwnd: u32, msg: u32, wp: u32, lp: u32) {
        self.call_with("SendMessageA", &[hwnd, msg, wp, lp], 0, true);
    }

    /// Send a message with a structure at `lp` of the scratch space.
    pub fn send_struct(&mut self, hwnd: u32, msg: u32, wp: u32, lp: u32, keep: bool) {
        self.call_with("SendMessageA", &[hwnd, msg, wp, lp], 8, keep);
    }

    /// Send a message whose lParam is the word at `at` of the scratch
    /// space when it is sent: what an earlier call left there.
    pub fn send_read(&mut self, hwnd: u32, msg: u32, wp: u32, at: u32) {
        self.op("SendMessageA", &[hwnd, msg, wp, at], 0, 8, false);
    }

    /// Write `words` at `offset` of the scratch space.
    pub fn write(&mut self, offset: u32, words: &[u32]) {
        self.ops.push(OP_WRITE);
        self.ops.push(offset);
        self.ops.push(words.len() as u32);
        self.ops.extend_from_slice(words);
        self.scratch = true;
    }

    /// The calls of `other` after these.
    pub fn append(&mut self, other: Calls) {
        self.ops.extend(other.ops);
        self.scratch |= other.scratch;
    }

    /// The variables of a task that makes the calls with `ops_step`, from
    /// the variable `base` on.
    pub fn into_vars(self, base: usize) -> Vec<u32> {
        let mut vars = vec![base as u32 + 3, 0, 0];
        vars.extend(self.ops);
        vars
    }
}

/// Return `result` from a handler of the module `module` once `calls`
/// are made.
pub(super) fn run(cpu: &mut Cpu, module: u32, calls: Calls, result: u32) -> Option<u32> {
    if calls.ops.is_empty() {
        return Some(result);
    }
    let mut vars = vec![4, 0, result];
    vars.extend(calls.ops);
    let scratch = if calls.scratch { SCRATCH } else { 0 };
    start(cpu, module, K_CALLS, vars, scratch)
}

/// The steps of `run`'s task.
pub(super) fn calls_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    ops_step(cpu, t, 1, value).unwrap_or_else(|| Next::Done(t.var(3)))
}

/// Make `calls` in a task of one of the modules from its next step: the
/// variable `ops_step` is then given. The task needs a scratch space if
/// they use it.
pub(super) fn ops_push(t: &mut Task, calls: Calls) -> usize {
    let base = t.vars.len();
    t.vars.extend(calls.into_vars(base));
    base
}

/// The next of the calls whose list starts at variable `base` (where it
/// is, whether the call gives the value, the value kept, then the list),
/// staying at the task's step; None when they are made.
pub(super) fn ops_step(cpu: &mut Cpu, t: &mut Task, base: usize, value: u32) -> Option<Next> {
    if t.var(base + 1) != 0 {
        t.set(base + 2, value);
        t.set(base + 1, 0);
    }
    let mut at = t.var(base) as usize;
    loop {
        match t.var(at) {
            OP_WRITE => {
                let (offset, n) = (t.var(at + 1), t.var(at + 2) as usize);
                let words: Vec<u32> = (0..n).map(|i| t.var(at + 3 + i)).collect();
                write_words(cpu, t.scratch + offset, &words);
                at += 3 + n;
            }
            OP_CALL => {
                let (target, head) = (t.var(at + 1), t.var(at + 2));
                let n = (head & 0xFF) as usize;
                let (mask, read) = (head >> 16, head >> 9 & 0x7F);
                let args: Vec<u32> = (0..n)
                    .map(|i| {
                        let a = t.var(at + 3 + i);
                        if mask >> i & 1 != 0 {
                            t.scratch + a
                        } else if read >> i & 1 != 0 {
                            cpu.bus.guest_read_32(t.scratch + a)
                        } else {
                            a
                        }
                    })
                    .collect();
                t.set(base, (at + 3 + n) as u32);
                t.set(base + 1, head >> 8 & 1);
                return Some(Next::call(target, &args, t.step));
            }
            _ => {
                t.set(base, at as u32);
                return None;
            }
        }
    }
}

// The start of a WM_PAINT task, which the three modules share. Its
// variables after the kind: the window, wParam's DC, the DC it paints
// on, whether BeginPaint gave it, the WM_CTLCOLOR* message, the brush
// and the window the message goes to; the module's own from 8.

pub(super) const P_HWND: usize = 1;
pub(super) const P_HDC: usize = 3;
pub(super) const P_BRUSH: usize = 6;
pub(super) const P_OWN: usize = 8;

/// Start a WM_PAINT task of `module` for `hwnd`, painting on `hdc` (0
/// for BeginPaint's), with the brush `msg` gets from `to`.
pub(super) fn paint_start(
    cpu: &mut Cpu,
    module: u32,
    hwnd: u32,
    hdc: u32,
    msg: u32,
    to: u32,
    own: &[u32],
) -> Option<u32> {
    let mut vars = vec![hwnd, hdc, 0, 0, msg, 0, to];
    vars.extend_from_slice(own);
    start(cpu, module, K_PAINT, vars, SCRATCH)
}

/// The steps of a WM_PAINT task before the module draws: None at step 3
/// and after, when `P_HDC` and `P_BRUSH` are set. The brush is 0 when
/// neither the window nor DefWindowProcA gave one.
pub(super) fn paint_head(t: &mut Task, value: u32) -> Option<Next> {
    let (hwnd, msg, to) = (t.var(P_HWND), t.var(5), t.var(7));
    match t.step {
        0 => {
            if t.var(2) != 0 {
                t.set(P_HDC, t.var(2));
                return Some(Next::call(0, &[], 1));
            }
            t.set(4, 1);
            Some(Next::call(address_of("BeginPaint"), &[hwnd, t.scratch], 1))
        }
        1 => {
            if t.var(4) != 0 {
                t.set(P_HDC, value);
            }
            if t.var(P_HDC) == 0 {
                return Some(Next::Done(0));
            }
            let args = [to, msg, t.var(P_HDC), hwnd];
            let target = if to != 0 {
                address_of("SendMessageA")
            } else {
                0
            };
            Some(Next::call(target, &args, 2))
        }
        2 => {
            if value != 0 || to == 0 {
                t.set(P_BRUSH, value);
                return Some(Next::call(0, &[], 3));
            }
            let args = [to, msg, t.var(P_HDC), hwnd];
            Some(Next::call(address_of("DefWindowProcA"), &args, 3))
        }
        3 => {
            if t.var(P_BRUSH) == 0 {
                t.set(P_BRUSH, value);
            }
            None
        }
        _ => None,
    }
}

/// The end of a WM_PAINT task: EndPaint if BeginPaint gave the DC, going
/// on at `step`, which is done.
pub(super) fn paint_end(t: &Task, step: u16) -> Next {
    if t.var(4) != 0 {
        return Next::call(address_of("EndPaint"), &[t.var(P_HWND), t.scratch], step);
    }
    Next::Done(0)
}

/// The brush a control paints with when the WM_CTLCOLOR* message gave
/// none: the window's color, with its text color on the DC.
pub(super) fn default_brush(cpu: &mut Cpu, hdc: u32, brush: u32, background: u32) -> u32 {
    if brush != 0 {
        return brush;
    }
    let state = user(cpu);
    let (text, bk) = (state.color(COLOR_WINDOWTEXT), state.color(background));
    dc::set_text_color(cpu, &[hdc, text]);
    dc::set_bk_color(cpu, &[hdc, bk]);
    sysparams::brush(cpu, background)
}

/// A font's widths of each byte, its height and its average width (0 for
/// the system font).
pub(super) fn measure(cpu: &mut Cpu, font: u32) -> (Vec<i32>, i32, i32) {
    let hdc = dc::create_compatible_dc(cpu, &[0]).unwrap_or(0);
    if hdc == 0 {
        return (vec![8; 256], 16, 8);
    }
    let f = if font != 0 {
        font
    } else {
        gdi32::gdi(cpu).stock(gdi32::SYSTEM_FONT)
    };
    dc::select_object(cpu, &[hdc, f]);
    let widths = (0..=255u8)
        .map(|b| font::text_extent(cpu, hdc, &[b]).map_or(0, |s| s.0))
        .collect();
    let m = font::metrics(cpu, hdc).unwrap_or_default();
    dc::delete_dc(cpu, &[hdc]);
    (widths, m.height.max(1), m.average_width.max(1))
}

/// Select `font` (0: the system font) on `hdc`.
pub(super) fn select_font(cpu: &mut Cpu, hdc: u32, font: u32) {
    let f = if font != 0 {
        font
    } else {
        gdi32::gdi(cpu).stock(gdi32::SYSTEM_FONT)
    };
    dc::select_object(cpu, &[hdc, f]);
}

/// Draw `text` at (`x`, `y`) on `hdc`, in the highlight's colors when
/// `selected`.
pub(super) fn text_out(cpu: &mut Cpu, hdc: u32, (x, y): (i32, i32), text: &[u8], selected: bool) {
    if text.is_empty() {
        return;
    }
    if !selected {
        font::draw_text(cpu, hdc, (x, y), 0, None, text, None);
        return;
    }
    let bk = dc::get_bk_color(cpu, &[hdc]).unwrap_or(0);
    let fg = dc::get_text_color(cpu, &[hdc]).unwrap_or(0);
    let mode = dc::get_bk_mode(cpu, &[hdc]).unwrap_or(dc::OPAQUE);
    let (hb, hf) = (
        user(cpu).color(COLOR_HIGHLIGHT),
        user(cpu).color(COLOR_HIGHLIGHTTEXT),
    );
    dc::set_bk_color(cpu, &[hdc, hb]);
    dc::set_text_color(cpu, &[hdc, hf]);
    dc::set_bk_mode(cpu, &[hdc, dc::OPAQUE]);
    font::draw_text(cpu, hdc, (x, y), 0, None, text, None);
    dc::set_bk_color(cpu, &[hdc, bk]);
    dc::set_text_color(cpu, &[hdc, fg]);
    dc::set_bk_mode(cpu, &[hdc, mode]);
}

/// The child of `parent` with the id `id`, or 0.
pub(super) fn child_with_id(state: &State, parent: u32, id: u32) -> u32 {
    let Some(w) = state.window(parent) else {
        return 0;
    };
    w.children
        .iter()
        .copied()
        .find(|&c| {
            state
                .window(c)
                .is_some_and(|cw| cw.id & 0xFFFF == id & 0xFFFF)
        })
        .unwrap_or(0)
}

/// The client area of `hwnd` at the origin.
pub(super) fn client_of(state: &State, hwnd: u32) -> Rect {
    state.window(hwnd).map_or(Rect::default(), |w| {
        Rect::new(0, 0, w.client.width(), w.client.height())
    })
}

/// Whether the key `vk` is down.
pub(super) fn key_down(cpu: &mut Cpu, vk: u32) -> bool {
    input::get_key_state(cpu, &[vk]).unwrap_or(0) & 0x8000 != 0
}

/// GlobalAlloc's handle of a block holding `text` and a 0 after it.
pub(super) fn global_text(cpu: &mut Cpu, flags: u32, text: &[u8]) -> u32 {
    let handle = crate::win32::kernel32::global_alloc(cpu, flags, text.len() as u32 + 1);
    let at = crate::win32::kernel32::global_address(cpu, handle);
    if at == 0 {
        return 0;
    }
    let mut bytes = text.to_vec();
    bytes.push(0);
    cpu.bus.guest_write_bytes(at, &bytes);
    handle
}

// The lines of the text.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// The text's end.
    Zero,
    /// A soft break, "\r\r\n".
    Soft,
    /// A hard break, "\r\n".
    Hard,
    /// A "\n" alone.
    Rich,
    /// Word wrap.
    Wrap,
}

#[derive(Clone, Copy, Debug)]
struct Line {
    /// Where it starts, its length without and with its break, and its
    /// width.
    index: usize,
    net: usize,
    length: usize,
    width: i32,
    end: End,
}

/// The default word break procedure's WB_LEFT and WB_RIGHT on the
/// `count` characters of `s`.
fn word_break(s: &[u8], index: usize, count: usize, right: bool) -> usize {
    let at = |i: usize| s.get(i).copied().unwrap_or(0);
    if count == 0 {
        return 0;
    }
    let mut i = index.saturating_sub(1);
    if !right {
        if at(i) == b' ' {
            while i > 0 && at(i) == b' ' {
                i -= 1;
            }
            if i > 0 {
                while i > 0 && at(i) != b' ' {
                    i -= 1;
                }
                if at(i) == b' ' {
                    i += 1;
                }
            }
        } else {
            while i > 0 && at(i) != b' ' {
                i -= 1;
            }
            if at(i) == b' ' {
                i += 1;
            }
        }
        return i;
    }
    if at(i) == b' ' {
        while i < count && at(i) == b' ' {
            i += 1;
        }
    } else {
        while at(i) != 0 && at(i) != b' ' && i < count {
            i += 1;
        }
        while at(i) == b' ' && i < count {
            i += 1;
        }
    }
    i
}

impl Edit {
    fn ml(&self) -> bool {
        self.style & ES_MULTILINE != 0
    }

    fn len(&self) -> usize {
        self.text.len()
    }

    fn width_of(&self, b: u8) -> i32 {
        self.widths
            .get(b as usize)
            .copied()
            .unwrap_or(self.char_width)
    }

    fn line_height(&self) -> i32 {
        self.line_height.max(1)
    }

    /// The text as it shows: a single-line edit's with a password
    /// character is all that character.
    fn shown(&self) -> Cow<'_, [u8]> {
        if !self.ml() && self.style & ES_PASSWORD != 0 && self.password != 0 {
            Cow::Owned(vec![self.password as u8; self.text.len()])
        } else {
            Cow::Borrowed(&self.text)
        }
    }

    /// The distance from `x` to the next tab stop after it, `x` being
    /// from the line's start.
    fn tab(&self, x: i32) -> i32 {
        let unit = |du: i32| (du * self.char_width / 4).max(1);
        let stops: Vec<i32> = self.tabs.iter().map(|&t| unit(t)).collect();
        let step = match stops.len() {
            0 => (8 * self.char_width).max(1),
            1 => stops[0],
            _ => {
                if let Some(&s) = stops.iter().find(|&&s| s > x) {
                    return s - x;
                }
                (8 * self.char_width).max(1)
            }
        };
        (x / step + 1) * step - x
    }

    /// The width of the text from `from` to `to` of the line starting at
    /// `start`, with a multi-line edit's tabs.
    fn extent(&self, start: usize, from: usize, to: usize) -> i32 {
        let shown = self.shown();
        let to = to.min(shown.len());
        if !self.ml() {
            return shown[from.min(to)..to]
                .iter()
                .map(|&b| self.width_of(b))
                .sum();
        }
        let mut x = 0;
        let mut at_from = 0;
        for (i, &b) in shown[start..to].iter().enumerate() {
            if start + i == from {
                at_from = x;
            }
            x += if b == b'\t' {
                self.tab(x)
            } else {
                self.width_of(b)
            };
        }
        if from >= to {
            at_from = x;
        }
        x - at_from
    }

    fn format_width(&self) -> i32 {
        self.format.right - self.format.left
    }

    /// The lines, as EDIT_BuildLineDefs_ML makes them.
    fn lines(&self) -> Vec<Line> {
        let len = self.len();
        if !self.ml() {
            let width = self.extent(0, 0, len);
            return vec![Line {
                index: 0,
                net: len,
                length: len,
                width,
                end: End::Zero,
            }];
        }
        let t = &self.text;
        let fw = self.format_width();
        let wrap = self.style & ES_AUTOHSCROLL == 0;
        let mut out = Vec::new();
        let mut pos = 0;
        loop {
            let mut cp = pos;
            while cp < len && t[cp] != b'\n' && !(t[cp] == b'\r' && t.get(cp + 1) == Some(&b'\n')) {
                cp += 1;
            }
            let (mut end, mut net) = if cp >= len {
                (End::Zero, len - pos)
            } else if cp > pos && t[cp - 1] == b'\r' {
                (End::Soft, cp - pos - 1)
            } else if t[cp] == b'\n' {
                (End::Rich, cp - pos)
            } else {
                (End::Hard, cp - pos)
            };
            let mut width = self.extent(pos, pos, pos + net);
            if wrap && width > fw {
                let line = &t[pos..pos + net];
                let mut next = 0;
                let mut prev;
                loop {
                    prev = next;
                    next = word_break(line, prev + 1, net, true);
                    width = self.extent(pos, pos, pos + next);
                    if width > fw || next <= prev || next >= net {
                        break;
                    }
                }
                if width <= fw {
                    prev = next;
                }
                if prev == 0 {
                    next = 0;
                    loop {
                        prev = next;
                        next += 1;
                        width = self.extent(pos, pos, pos + next);
                        if width > fw || next >= net {
                            break;
                        }
                    }
                    if prev == 0 {
                        prev = 1;
                    }
                }
                net = prev;
                end = End::Wrap;
                width = self.extent(pos, pos, pos + net);
            }
            let length = match end {
                End::Soft => net + 3,
                End::Rich => net + 1,
                End::Hard => net + 2,
                End::Wrap | End::Zero => net,
            };
            out.push(Line {
                index: pos,
                net,
                length,
                width,
                end,
            });
            pos += length;
            if end == End::Zero || length == 0 && end != End::Wrap {
                break;
            }
        }
        out
    }

    fn text_width(&self, lines: &[Line]) -> i32 {
        lines.iter().map(|l| l.width).max().unwrap_or(0)
    }

    /// How many lines the format rectangle shows.
    fn vlc(&self) -> i32 {
        ((self.format.bottom - self.format.top) / self.line_height()).max(1)
    }

    fn line_from_char(&self, lines: &[Line], index: i32) -> i32 {
        if !self.ml() {
            return 0;
        }
        if index > self.len() as i32 {
            return lines.len() as i32 - 1;
        }
        let mut index = if index == -1 {
            self.sel_start.min(self.sel_end) as i32
        } else {
            index
        };
        let mut line = 0;
        index -= lines[0].length as i32;
        while index >= 0 && line + 1 < lines.len() {
            line += 1;
            index -= lines[line].length as i32;
        }
        line as i32
    }

    fn line_index(&self, lines: &[Line], line: i32) -> i32 {
        if !self.ml() {
            return 0;
        }
        if line >= lines.len() as i32 {
            return -1;
        }
        let line = if line == -1 {
            self.line_from_char(lines, self.sel_end as i32)
        } else {
            line
        };
        lines[line.max(0) as usize].index as i32
    }

    fn line_length(&self, lines: &[Line], index: i32) -> i32 {
        if !self.ml() {
            return self.len() as i32;
        }
        if index == -1 {
            let l = self.line_from_char(lines, self.sel_start as i32);
            let mut count = self.sel_start as i32 - self.line_index(lines, l);
            let l = self.line_from_char(lines, self.sel_end as i32);
            let li = self.line_index(lines, l);
            count += li + self.line_length(lines, li) - self.sel_end as i32;
            return count;
        }
        let l = self.line_from_char(lines, index);
        lines[l as usize].net as i32
    }

    /// Where the character `index` shows, in client coordinates.
    fn pos_from_char(&self, lines: &[Line], index: i32, after_wrap: bool) -> (i32, i32) {
        let len = self.len() as i32;
        let index = index.clamp(0, len);
        let (mut x, mut y);
        if self.ml() {
            let mut l = self.line_from_char(lines, index);
            y = (l - self.y_offset) * self.line_height();
            let mut li = self.line_index(lines, l);
            if after_wrap && li == index && l > 0 && lines[l as usize - 1].end == End::Wrap {
                l -= 1;
                y -= self.line_height();
                li = self.line_index(lines, l);
            }
            let line = lines[l as usize];
            let w = self.format_width();
            let li = li as usize;
            let index = index as usize;
            x = if self.style & ES_RIGHT != 0 {
                w - (self.extent(li, index, li + line.net) - self.x_offset)
            } else if self.style & ES_CENTER != 0 {
                self.extent(li, li, index) - self.x_offset + (w - line.width) / 2
            } else {
                self.extent(li, li, index) - self.x_offset
            };
        } else {
            let xo = self.x_offset.clamp(0, len);
            if index < xo {
                x = -self.extent(0, index as usize, xo as usize);
            } else {
                x = self.extent(0, xo as usize, index as usize);
                if xo == 0 && self.style & (ES_RIGHT | ES_CENTER) != 0 {
                    let w = self.format_width();
                    let tw = lines[0].width;
                    if w > tw {
                        x += if self.style & ES_RIGHT != 0 {
                            w - tw
                        } else {
                            (w - tw) / 2
                        };
                    }
                }
            }
            y = 0;
        }
        x += self.format.left;
        y += self.format.top;
        (x as i16 as i32, y as i16 as i32)
    }

    /// The character nearest the point (`x`, `y`), which may be outside
    /// the format rectangle, and whether it is at the end of a wrapped
    /// line.
    fn char_from_pos(&self, lines: &[Line], x: i32, y: i32) -> (i32, bool) {
        if self.ml() {
            let mut line = (y - self.format.top) / self.line_height() + self.y_offset;
            if y < self.format.top && (y - self.format.top) % self.line_height() != 0 {
                line -= 1;
            }
            let l = line.clamp(0, lines.len() as i32 - 1) as usize;
            let def = lines[l];
            let mut x = x + self.x_offset - self.format.left;
            if self.style & ES_RIGHT != 0 {
                x -= self.format_width() - def.width;
            } else if self.style & ES_CENTER != 0 {
                x -= (self.format_width() - def.width) / 2;
            }
            if x >= def.width {
                return ((def.index + def.net) as i32, def.end == End::Wrap);
            }
            if x <= 0 {
                return (def.index as i32, false);
            }
            let index = self.nearest(def.index, def.index, def.index + def.net, x, false);
            let after = index == def.index + def.net && def.end == End::Wrap;
            return (index as i32, after);
        }
        let mut x = x - self.format.left;
        if x == 0 {
            return (self.x_offset, false);
        }
        let len = self.len();
        let xo = self.x_offset.clamp(0, len as i32) as usize;
        if xo == 0 {
            let indent = self.format_width() - lines[0].width;
            if self.style & ES_RIGHT != 0 {
                x -= indent;
            } else if self.style & ES_CENTER != 0 {
                x -= indent / 2;
            }
        }
        if x < 0 {
            // Left of the scroll: the character whose distance to it is
            // nearest -x.
            let mut best = xo;
            let mut best_d = i32::MAX;
            for i in (0..=xo).rev() {
                let d = (self.extent(0, i, xo) + x).abs();
                if d <= best_d {
                    best_d = d;
                    best = i;
                }
            }
            return (best as i32, false);
        }
        (self.nearest(0, xo, len, x, true) as i32, false)
    }

    /// The place from `from` to `to` (of the line starting at `start`)
    /// whose distance from `from` is nearest `x`.
    fn nearest(&self, start: usize, from: usize, to: usize, x: i32, single: bool) -> usize {
        let (mut low, mut x_low) = (from, 0);
        let (mut high, mut x_high) = (from, 0);
        for i in from..=to {
            let w = self.extent(start, from, i);
            if w > x {
                high = i;
                x_high = w;
                break;
            }
            low = i;
            x_low = w;
            high = i;
            x_high = w;
        }
        if high == low {
            return low;
        }
        let pick_high = if single {
            (x_high - x).abs() <= (x_low - x).abs() + 1
        } else {
            (x_high - x).abs() < (x_low - x).abs()
        };
        if pick_high { high } else { low }
    }

    /// The text with soft breaks where lines wrap, for EM_FMTLINES.
    fn text_with_breaks(&self) -> Vec<u8> {
        if self.flags & EF_SOFT_BREAKS == 0 || !self.ml() {
            return self.text.clone();
        }
        let mut out = Vec::with_capacity(self.text.len());
        for l in self.lines() {
            out.extend_from_slice(&self.text[l.index..l.index + l.length]);
            if l.end == End::Wrap {
                out.extend_from_slice(b"\r\r\n");
            }
        }
        out
    }

    fn can_undo(&self) -> bool {
        self.undo_count != 0 || !self.undo_text.is_empty()
    }

    fn empty_undo(&mut self) {
        self.undo_count = 0;
        self.undo_text.clear();
    }
}

// A message's work: the state taken out of the window, put back when it
// is done.

struct Ctx {
    hwnd: u32,
    id: u32,
    client: Rect,
    e: Edit,
    out: Calls,
    /// Whether the window needs painting again.
    redraw: bool,
}

impl Ctx {
    fn load(cpu: &mut Cpu, hwnd: u32) -> Option<Ctx> {
        let state = user(cpu);
        let w = state.window(hwnd)?;
        Some(Ctx {
            hwnd,
            id: w.id & 0xFFFF,
            client: client_of(state, hwnd),
            e: w.control.edit.clone(),
            out: Calls::default(),
            redraw: false,
        })
    }

    /// Return `result` once the state is back (`finish`) and the calls
    /// are made.
    fn store(self, cpu: &mut Cpu, result: u32) -> Option<u32> {
        let out = self.finish(cpu);
        run(cpu, EDIT, out, result)
    }

    /// Put the state back, invalidate the window if it changed, and move
    /// the caret: the calls to make.
    fn finish(mut self, cpu: &mut Cpu) -> Calls {
        let lines = self.e.lines();
        self.scroll_info(&lines);
        let hwnd = self.hwnd;
        let caret = (self.e.flags & EF_FOCUSED != 0).then(|| {
            let after = self.e.flags & EF_AFTER_WRAP != 0;
            self.e.pos_from_char(&lines, self.e.sel_end as i32, after)
        });
        let text = self.e.text.clone();
        if let Some(w) = user(cpu).window_mut(hwnd) {
            w.control.edit = self.e;
            w.text = text;
        }
        if self.redraw {
            paint::invalidate_rect(cpu, &[hwnd, 0, 1]);
        }
        if let Some((x, y)) = caret
            && user(cpu).caret.hwnd == hwnd
        {
            input::set_caret_pos(cpu, &[x as u32, y as u32]);
        }
        self.out
    }

    fn notify(&mut self, code: u32) {
        let lp = self.hwnd;
        let wp = self.id | code << 16;
        self.out.send(self.e.parent, WM_COMMAND, wp, lp);
    }

    /// SetScrollInfo for the scroll bars whose values changed.
    fn scroll_info(&mut self, lines: &[Line]) {
        let e = &self.e;
        let mut bars = e.bars.clone();
        bars.resize(6, -1);
        let mask = SIF_PAGE | SIF_POS | SIF_RANGE | SIF_DISABLENOSCROLL;
        if e.style & WS_VSCROLL != 0 {
            let v = [
                lines.len() as i32 - 1,
                (e.format.bottom - e.format.top) / e.line_height(),
                e.y_offset,
            ];
            if bars[0..3] != v {
                bars[0..3].copy_from_slice(&v);
                let si = [28, mask, 0, v[0] as u32, v[1] as u32, v[2] as u32, 0];
                self.out.write(SCRATCH_DATA, &si);
                let args = [self.hwnd, SB_VERT, SCRATCH_DATA, 1];
                self.out.call_with("SetScrollInfo", &args, 4, false);
            }
        }
        if e.style & WS_HSCROLL != 0 {
            let v = [e.text_width(lines) - 1, e.format_width(), e.x_offset];
            if bars[3..6] != v {
                bars[3..6].copy_from_slice(&v);
                let si = [28, mask, 0, v[0] as u32, v[1] as u32, v[2] as u32, 0];
                self.out.write(SCRATCH_DATA + 32, &si);
                let args = [self.hwnd, SB_HORZ, SCRATCH_DATA + 32, 1];
                self.out.call_with("SetScrollInfo", &args, 4, false);
            }
        }
        self.e.bars = bars;
    }

    fn in_combo(&self, cpu: &mut Cpu) -> bool {
        self.e.style & ES_COMBO != 0 && is_class(user(cpu), self.e.parent, "ComboBox")
    }

    // The format rectangle.

    fn adjust_format_rect(&mut self) {
        let e = &mut self.e;
        e.format.right = e.format.right.max(e.format.left + e.char_width);
        if e.ml() {
            let vlc = e.vlc();
            e.format.bottom = e.format.top + vlc * e.line_height();
            let lines = e.lines();
            let max_x = (e.text_width(&lines) - e.format_width()).max(0);
            e.x_offset = e.x_offset.min(max_x);
            let max_y = (lines.len() as i32 - vlc).max(0);
            e.y_offset = e.y_offset.min(max_y);
        } else {
            e.format.bottom = e.format.top + e.line_height();
        }
        e.format.bottom = e.format.bottom.min(self.client.bottom);
    }

    fn set_rect_np(&mut self, cpu: &mut Cpu, rc: Rect) {
        let ex = user(cpu).window(self.hwnd).map_or(0, |w| w.ex_style);
        let e = &mut self.e;
        e.format = rc;
        let lh = e.line_height();
        if ex & super::super::window::WS_EX_CLIENTEDGE != 0 {
            e.format.left += 1;
            e.format.right -= 1;
            if e.format.bottom - e.format.top >= lh + 2 {
                e.format.top += 1;
                e.format.bottom -= 1;
            }
        } else if e.style & WS_BORDER != 0 {
            let (bw, bh) = (sysparams::metric(5) + 1, sysparams::metric(6) + 1);
            e.format.left += bw;
            e.format.right -= bw;
            if e.format.bottom - e.format.top >= lh + 2 * bh {
                e.format.top += bh;
                e.format.bottom -= bh;
            }
        }
        e.format.left += e.left_margin;
        e.format.right -= e.right_margin;
        self.adjust_format_rect();
    }

    fn set_margins(&mut self, action: u32, left: u32, right: u32) {
        let e = &mut self.e;
        // The fonts here are raster fonts, whose default margins are 0.
        if action & EC_LEFTMARGIN != 0 {
            e.format.left -= e.left_margin;
            e.left_margin = if left != EC_USEFONTINFO {
                left as i32
            } else {
                0
            };
            e.format.left += e.left_margin;
        }
        if action & EC_RIGHTMARGIN != 0 {
            e.format.right += e.right_margin;
            e.right_margin = if right != EC_USEFONTINFO {
                right as i32
            } else {
                0
            };
            e.format.right -= e.right_margin;
        }
        if action & (EC_LEFTMARGIN | EC_RIGHTMARGIN) != 0 {
            self.adjust_format_rect();
        }
    }

    fn set_font(&mut self, cpu: &mut Cpu, font: u32, redraw: bool) {
        let (widths, height, average) = measure(cpu, font);
        self.e.font = font;
        self.e.widths = widths;
        self.e.line_height = height;
        self.e.char_width = average;
        let client = self.client;
        self.set_rect_np(cpu, client);
        self.set_margins(
            EC_LEFTMARGIN | EC_RIGHTMARGIN,
            EC_USEFONTINFO,
            EC_USEFONTINFO,
        );
        if redraw {
            self.redraw = true;
        }
        if self.e.flags & EF_FOCUSED != 0 {
            input::destroy_caret(cpu, &[]);
            input::create_caret(cpu, &[self.hwnd, 0, 1, self.e.line_height as u32]);
            input::show_caret(cpu, &[self.hwnd]);
        }
    }

    // The selection and scrolling.

    fn set_sel(&mut self, start: u32, end: u32, after_wrap: bool) {
        let e = &mut self.e;
        let len = e.len() as u32;
        let (start, end) = if start == u32::MAX {
            (e.sel_end, e.sel_end)
        } else {
            (start.min(len), end.min(len))
        };
        if (start, end) != (e.sel_start, e.sel_end) {
            self.redraw = true;
        }
        e.sel_start = start;
        e.sel_end = end;
        if after_wrap {
            e.flags |= EF_AFTER_WRAP;
        } else {
            e.flags &= !EF_AFTER_WRAP;
        }
    }

    fn line_scroll_internal(&mut self, mut dx: i32, dy: i32) -> bool {
        let lines = self.e.lines();
        let e = &mut self.e;
        let lh = e.line_height();
        let per_page = (e.format.bottom - e.format.top) / lh;
        let (x_px, dy) = if e.ml() {
            (e.x_offset, dy)
        } else {
            (e.pos_from_char(&lines, e.x_offset, false).0, 0)
        };
        let tw = e.text_width(&lines);
        if -dx > x_px {
            dx = -x_px;
        }
        if dx > tw - x_px {
            dx = tw - x_px;
        }
        let count = lines.len() as i32;
        let mut ny = (e.y_offset + dy).max(0);
        if ny >= count - per_page {
            ny = (count - per_page).max(0);
        }
        let dy = (e.y_offset - ny) * lh;
        if dx != 0 || dy != 0 {
            e.y_offset = ny;
            if e.ml() {
                e.x_offset += dx;
            } else {
                e.x_offset += dx / e.char_width.max(1);
            }
            self.redraw = true;
        }
        if dx != 0 {
            self.notify(EN_HSCROLL);
        }
        if dy != 0 {
            self.notify(EN_VSCROLL);
        }
        true
    }

    fn scroll_caret(&mut self) {
        let lines = self.e.lines();
        let e = &self.e;
        if e.ml() {
            let l = e.line_from_char(&lines, e.sel_end as i32);
            let x = e
                .pos_from_char(&lines, e.sel_end as i32, e.flags & EF_AFTER_WRAP != 0)
                .0;
            let vlc = e.vlc();
            let cw = e.char_width.max(1);
            let mut dy = 0;
            let mut dx = 0;
            if l >= e.y_offset + vlc {
                dy = l - vlc + 1 - e.y_offset;
            }
            if l < e.y_offset {
                dy = l - e.y_offset;
            }
            let ww = e.format_width();
            if x < e.format.left {
                dx = x - e.format.left - ww / HSCROLL_FRACTION / cw * cw;
            }
            if x > e.format.right {
                dx = x - e.format.left - (HSCROLL_FRACTION - 1) * ww / HSCROLL_FRACTION / cw * cw;
            }
            let count = lines.len() as i32;
            let short = e.y_offset != 0 && count - e.y_offset < vlc;
            if dy != 0 || dx != 0 || short {
                let tw = e.text_width(&lines);
                if e.x_offset + dx + ww > tw {
                    dx = tw - ww - e.x_offset;
                }
                if dx != 0 || dy != 0 || short {
                    self.line_scroll_internal(dx, dy);
                }
            }
        } else {
            let mut e = self.e.clone();
            let mut x = e.pos_from_char(&lines, e.sel_end as i32, false).0;
            let fw = e.format_width();
            let len = e.len() as i32;
            if x < e.format.left {
                let goal = e.format.left + fw / HSCROLL_FRACTION;
                loop {
                    e.x_offset -= 1;
                    x = e.pos_from_char(&lines, e.sel_end as i32, false).0;
                    if x >= goal || e.x_offset <= 0 {
                        break;
                    }
                }
                e.x_offset = e.x_offset.max(0);
                self.redraw = true;
            } else if x > e.format.right {
                let goal = e.format.right - fw / HSCROLL_FRACTION;
                loop {
                    e.x_offset += 1;
                    x = e.pos_from_char(&lines, e.sel_end as i32, false).0;
                    let last = e.pos_from_char(&lines, len, false).0;
                    if x <= goal || last <= e.format.right || e.x_offset >= len {
                        break;
                    }
                }
                self.redraw = true;
            }
            self.e = e;
        }
    }

    // Changing the text.

    fn replace_sel(&mut self, can_undo: bool, replace: &[u8], send_update: bool, honor: bool) {
        let e = &mut self.e;
        let (mut s, mut e_) = (e.sel_start as usize, e.sel_end as usize);
        let mut insert = replace.to_vec();
        if s == e_ && insert.is_empty() {
            return;
        }
        if e_ < s {
            std::mem::swap(&mut s, &mut e_);
        }
        let tl = e.len();
        let size = tl - (e_ - s) + insert.len();
        if honor && size as u64 > e.limit as u64 {
            self.notify(EN_MAXTEXT);
            let e = &mut self.e;
            let keep = (e.limit as usize).saturating_sub(tl - (e_ - s));
            insert.truncate(keep);
        }
        let e = &mut self.e;
        if e.style & ES_UPPERCASE != 0 {
            insert.make_ascii_uppercase();
        } else if e.style & ES_LOWERCASE != 0 {
            insert.make_ascii_lowercase();
        }
        let deleted: Vec<u8> = e.text.splice(s..e_, insert.iter().copied()).collect();
        let mut strl = insert.len();
        let mut maxed = false;
        if e.ml() {
            if honor && e.style & ES_AUTOVSCROLL == 0 && e.lines().len() as i32 > e.vlc() {
                // Too many lines: none of it.
                e.text.splice(s..s + strl, deleted.iter().copied());
                strl = 0;
                e_ = s;
                maxed = true;
            }
        } else if honor && e.style & ES_AUTOHSCROLL == 0 {
            let fw = e.format_width();
            if e.lines()[0].width > fw {
                while e.lines()[0].width > fw && strl > 0 {
                    e.text.remove(s + strl - 1);
                    strl -= 1;
                }
                maxed = true;
            }
        }
        if maxed {
            self.notify(EN_MAXTEXT);
        }
        let e = &mut self.e;
        if e_ != s {
            if can_undo {
                let deleted = &deleted[..e_ - s];
                if e.undo_count == 0 && !e.undo_text.is_empty() && s as u32 == e.undo_pos {
                    e.undo_text.extend_from_slice(deleted);
                } else if e.undo_count == 0 && !e.undo_text.is_empty() && e_ as u32 == e.undo_pos {
                    let mut t = deleted.to_vec();
                    t.extend_from_slice(&e.undo_text);
                    e.undo_text = t;
                    e.undo_pos = s as u32;
                } else {
                    e.undo_text = deleted.to_vec();
                    e.undo_pos = s as u32;
                }
                e.undo_count = 0;
            } else {
                e.empty_undo();
            }
        }
        if strl > 0 {
            if can_undo {
                if s as u32 == e.undo_pos
                    || e.undo_count != 0 && s as u32 == e.undo_pos + e.undo_count
                {
                    e.undo_count += strl as u32;
                } else {
                    e.undo_pos = s as u32;
                    e.undo_count = strl as u32;
                    e.undo_text.clear();
                }
            } else {
                e.empty_undo();
            }
        }
        let new_end = s + strl;
        if e.style & (ES_RIGHT | ES_CENTER) != 0 {
            let delta = strl as i32 - (e.sel_end as i32 - e.sel_start as i32).abs();
            if delta < 0 && e.x_offset != 0 {
                if -delta > e.x_offset {
                    e.x_offset = 0;
                } else {
                    e.x_offset += delta;
                }
            }
        }
        self.set_sel(new_end as u32, new_end as u32, false);
        self.e.flags |= EF_MODIFIED;
        self.redraw = true;
        if send_update {
            self.notify(EN_UPDATE);
        }
        self.scroll_caret();
        if send_update {
            self.e.flags &= !EF_UPDATE;
            self.notify(EN_CHANGE);
        }
    }

    fn set_text(&mut self, cpu: &mut Cpu, text: &[u8]) {
        self.set_sel(0, u32::MAX, false);
        self.replace_sel(false, text, false, false);
        self.e.x_offset = 0;
        self.e.flags &= !EF_MODIFIED;
        self.set_sel(0, 0, false);
        if !(self.e.ml() || self.in_combo(cpu)) {
            self.notify(EN_UPDATE);
            self.notify(EN_CHANGE);
        }
        self.scroll_caret();
    }

    fn undo(&mut self) -> u32 {
        let e = &self.e;
        if e.style & ES_READONLY != 0 {
            return !e.ml() as u32;
        }
        let text = e.undo_text.clone();
        let (pos, count) = (e.undo_pos, e.undo_count);
        self.set_sel(pos, pos + count, false);
        self.e.empty_undo();
        self.replace_sel(true, &text, true, true);
        let (pos, count) = (self.e.undo_pos, self.e.undo_count);
        self.set_sel(pos, pos + count, false);
        self.notify(EN_CHANGE);
        self.scroll_caret();
        1
    }

    fn selected(&self) -> Vec<u8> {
        let (s, e) = order(self.e.sel_start, self.e.sel_end);
        self.e.text[s as usize..e as usize].to_vec()
    }

    fn copy(&mut self, cpu: &mut Cpu) {
        let text = self.selected();
        if text.is_empty() {
            return;
        }
        let handle = global_text(cpu, GMEM_MOVEABLE_SHARE, &text);
        input::open_clipboard(cpu, &[self.hwnd]);
        input::empty_clipboard(cpu, &[]);
        input::set_clipboard_data(cpu, &[CF_TEXT, handle]);
        input::close_clipboard(cpu, &[]);
    }

    fn clear(&mut self) {
        if self.e.style & ES_READONLY != 0 {
            return;
        }
        self.replace_sel(true, &[], true, true);
    }

    fn paste(&mut self, cpu: &mut Cpu) {
        if self.e.style & ES_READONLY != 0 {
            return;
        }
        input::open_clipboard(cpu, &[self.hwnd]);
        let handle = input::get_clipboard_data(cpu, &[CF_TEXT]).unwrap_or(0);
        let at = crate::win32::kernel32::global_address(cpu, handle);
        if handle != 0 && at != 0 {
            let text = bytes_at(cpu, at).unwrap_or_default();
            self.replace_sel(true, &text, true, true);
        } else if self.e.style & ES_PASSWORD != 0 {
            self.replace_sel(true, &[], true, true);
        }
        input::close_clipboard(cpu, &[]);
    }

    // Moving the caret.

    fn sel_to(&mut self, extend: bool, e: i32, after_wrap: bool) {
        let s = if extend { self.e.sel_start } else { e as u32 };
        self.set_sel(s, e as u32, after_wrap);
        self.scroll_caret();
    }

    fn move_backward(&mut self, extend: bool) {
        let t = &self.e.text;
        let mut e = self.e.sel_end as usize;
        if e > 0 {
            e -= 1;
            if self.e.ml() && e > 0 && t[e - 1] == b'\r' && t.get(e) == Some(&b'\n') {
                e -= 1;
                if e > 0 && t[e - 1] == b'\r' {
                    e -= 1;
                }
            }
        }
        self.sel_to(extend, e as i32, false);
    }

    fn move_forward(&mut self, extend: bool) {
        let t = &self.e.text;
        let mut e = self.e.sel_end as usize;
        if e < t.len() {
            e += 1;
            if self.e.ml() && t[e - 1] == b'\r' {
                if t.get(e) == Some(&b'\n') {
                    e += 1;
                } else if t.get(e) == Some(&b'\r') && t.get(e + 1) == Some(&b'\n') {
                    e += 2;
                }
            }
        }
        self.sel_to(extend, e as i32, false);
    }

    /// Move to the place `dy` pixels below the caret.
    fn move_vertically(&mut self, extend: bool, dy: i32) {
        let lines = self.e.lines();
        let after = self.e.flags & EF_AFTER_WRAP != 0;
        let (x, y) = self.e.pos_from_char(&lines, self.e.sel_end as i32, after);
        let (e, after) = self.e.char_from_pos(&lines, x, y + dy);
        self.sel_to(extend, e, after);
    }

    fn move_home(&mut self, extend: bool, ctrl: bool) {
        let e = if !ctrl && self.e.ml() {
            let lines = self.e.lines();
            let after = self.e.flags & EF_AFTER_WRAP != 0;
            let y = self.e.pos_from_char(&lines, self.e.sel_end as i32, after).1;
            self.e.char_from_pos(&lines, -self.e.x_offset, y).0
        } else {
            0
        };
        self.sel_to(extend, e, false);
    }

    fn move_end(&mut self, extend: bool, ctrl: bool) {
        let (e, after) = if !ctrl && self.e.ml() {
            let lines = self.e.lines();
            let after = self.e.flags & EF_AFTER_WRAP != 0;
            let y = self.e.pos_from_char(&lines, self.e.sel_end as i32, after).1;
            self.e.char_from_pos(&lines, 0x3FFF_FFFF, y)
        } else {
            (self.e.len() as i32, false)
        };
        self.sel_to(extend, e, after);
    }

    fn move_word(&mut self, extend: bool, right: bool) {
        let lines = self.e.lines();
        let e = &self.e;
        let mut end = e.sel_end as i32;
        let l = e.line_from_char(&lines, end);
        let ll = e.line_length(&lines, end);
        let li = e.line_index(&lines, l);
        let text = &e.text[li as usize..];
        if !right {
            if end - li == 0 {
                if l > 0 {
                    let li = e.line_index(&lines, l - 1);
                    end = li + e.line_length(&lines, li);
                }
            } else {
                end = li + word_break(text, (end - li) as usize, ll as usize, false) as i32;
            }
        } else if end - li == ll {
            if e.ml() && l != lines.len() as i32 - 1 {
                end = e.line_index(&lines, l + 1);
            }
        } else {
            end = li + word_break(text, (end - li + 1) as usize, ll as usize, true) as i32;
        }
        self.sel_to(extend, end, false);
    }

    fn select_word(&mut self) {
        let lines = self.e.lines();
        let e = &self.e;
        let end = e.sel_end as i32;
        let l = e.line_from_char(&lines, end);
        let li = e.line_index(&lines, l);
        let ll = e.line_length(&lines, end) as usize;
        let text = &e.text[li as usize..];
        let s = li + word_break(text, (end - li) as usize, ll, false) as i32;
        let en = li + word_break(text, (end - li) as usize, ll, true) as i32;
        self.set_sel(s as u32, en as u32, false);
        self.scroll_caret();
    }

    /// The point moved into the format rectangle.
    fn confine(&self, x: i32, y: i32) -> (i32, i32) {
        let f = &self.e.format;
        (
            x.max(f.left).min(f.right - 1),
            y.max(f.top).min(f.bottom - 1),
        )
    }

    /// What EDIT_CheckCombo does for a key in a combo box's edit: the key
    /// goes to the list box. Whether it is in one.
    fn check_combo(&mut self, cpu: &mut Cpu, msg: u32, key: u32) -> bool {
        if !self.in_combo(cpu) {
            return false;
        }
        let combo = self.e.parent;
        let (list, extended, dropped) = super::combobox::state_of(cpu, combo);
        let mut eui = 0;
        let mut is_dropped = true;
        let mut key = key;
        if key == VK_UP || key == VK_DOWN {
            if extended {
                eui = 1;
            }
            if msg == WM_KEYDOWN || eui != 0 {
                is_dropped = dropped;
            }
        }
        match msg {
            WM_KEYDOWN => {
                if !is_dropped && eui != 0 && (key == VK_UP || key == VK_DOWN) {
                    super::combobox::set_extended_ui(cpu, combo, false);
                    key = VK_F4;
                    eui = 2;
                }
                self.out.send(list, WM_KEYDOWN, key, 0);
            }
            _ => {
                if eui != 0 {
                    let show = !is_dropped as u32;
                    self.out
                        .send(combo, super::combobox::CB_SHOWDROPDOWN, show, 0);
                } else {
                    self.out.send(list, WM_KEYDOWN, VK_F4, 0);
                }
            }
        }
        if eui == 2 {
            // The extended UI comes back after the key.
            self.out
                .send(combo, super::combobox::CB_SETEXTENDEDUI, 1, 0);
        }
        true
    }
}

fn order(a: u32, b: u32) -> (u32, u32) {
    if b < a { (b, a) } else { (a, b) }
}

// The window procedure.

/// EditWndProc(hwnd, msg, wParam, lParam).
pub fn edit_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, msg, wp, lp) = (args[0], args[1], args[2], args[3]);
    let created = user(cpu)
        .window(hwnd)
        .is_some_and(|w| w.control.edit.flags & EF_CREATED != 0);
    if msg == WM_NCCREATE {
        return Some(nc_create(cpu, hwnd, lp));
    }
    if !created {
        return def_window_proc(cpu, args);
    }
    let mut c = Ctx::load(cpu, hwnd)?;
    let result = match msg {
        EM_GETSEL => {
            let (s, e) = order(c.e.sel_start, c.e.sel_end);
            if wp != 0 {
                write_words(cpu, wp, &[s]);
            }
            if lp != 0 {
                write_words(cpu, lp, &[e]);
            }
            return Some(make_long(s as i32, e as i32));
        }
        EM_SETSEL => {
            c.set_sel(wp, lp, false);
            c.scroll_caret();
            1
        }
        EM_GETRECT => {
            if lp != 0 {
                c.e.format.write(cpu, lp);
            }
            return Some(0);
        }
        EM_SETRECT | EM_SETRECTNP => {
            if c.e.ml() && lp != 0 {
                let r = Rect::read(cpu, lp);
                c.set_rect_np(cpu, r);
                if msg == EM_SETRECT {
                    c.redraw = true;
                }
            }
            0
        }
        EM_SCROLL => scroll(&mut c, wp),
        EM_LINESCROLL => {
            if !c.e.ml() {
                0
            } else {
                let dx = wp as i32 * c.e.char_width;
                c.line_scroll_internal(dx, lp as i32) as u32
            }
        }
        EM_SCROLLCARET => {
            c.scroll_caret();
            1
        }
        EM_GETMODIFY => return Some((c.e.flags & EF_MODIFIED != 0) as u32),
        EM_SETMODIFY => {
            if wp != 0 {
                c.e.flags |= EF_MODIFIED;
            } else {
                c.e.flags &= !(EF_MODIFIED | EF_UPDATE);
            }
            0
        }
        EM_GETLINECOUNT => {
            return Some(if c.e.ml() {
                c.e.lines().len() as u32
            } else {
                1
            });
        }
        EM_LINEINDEX => return Some(c.e.line_index(&c.e.lines(), wp as i32) as u32),
        EM_LINELENGTH => return Some(c.e.line_length(&c.e.lines(), wp as i32) as u32),
        EM_LINEFROMCHAR => return Some(c.e.line_from_char(&c.e.lines(), wp as i32) as u32),
        EM_SETHANDLE => {
            if c.e.ml() && wp != 0 {
                let at = crate::win32::kernel32::global_address(cpu, wp);
                let text = if at != 0 {
                    bytes_at(cpu, at).unwrap_or_default()
                } else {
                    vec![]
                };
                c.e.text = text;
                c.e.handle = wp;
                c.e.x_offset = 0;
                c.e.y_offset = 0;
                c.e.sel_start = 0;
                c.e.sel_end = 0;
                c.e.empty_undo();
                c.e.flags &= !(EF_MODIFIED | EF_UPDATE);
                c.redraw = true;
                c.scroll_caret();
            }
            0
        }
        EM_GETHANDLE => {
            if !c.e.ml() {
                0
            } else {
                get_handle(cpu, &mut c)
            }
        }
        EM_GETTHUMB => {
            let lines = c.e.lines();
            let count = lines.len() as i32;
            let v = if count > 1 {
                c.e.y_offset * 100 / (count - 1)
            } else {
                0
            };
            let tw = c.e.text_width(&lines);
            let fw = c.e.format_width();
            let h = if tw > fw {
                c.e.x_offset * 100 / (tw - fw)
            } else {
                0
            };
            return Some(make_long(v, h));
        }
        EM_REPLACESEL => {
            let text = if lp != 0 {
                bytes_at(cpu, lp).unwrap_or_default()
            } else {
                vec![]
            };
            c.replace_sel(wp != 0, &text, true, true);
            1
        }
        EM_GETLINE => return Some(get_line(cpu, &c, wp as i32, lp)),
        EM_LIMITTEXT => {
            let mut limit = if wp == 0 { u32::MAX } else { wp };
            if !c.e.ml() {
                limit = limit.min(0x7FFF_FFFE);
            }
            c.e.limit = limit;
            0
        }
        EM_CANUNDO => return Some(c.e.can_undo() as u32),
        EM_UNDO | WM_UNDO => c.undo(),
        EM_FMTLINES => {
            c.e.flags &= !EF_SOFT_BREAKS;
            if wp != 0 {
                c.e.flags |= EF_SOFT_BREAKS;
            }
            wp
        }
        EM_SETTABSTOPS => {
            if !c.e.ml() {
                0
            } else {
                let count = wp as usize;
                c.e.tabs = read_words(cpu, lp, count)
                    .iter()
                    .map(|&t| t as i32)
                    .collect();
                1
            }
        }
        EM_SETPASSWORDCHAR => {
            if !c.e.ml() && c.e.password != wp & 0xFF {
                c.e.password = wp & 0xFF;
                if let Some(w) = user(cpu).window_mut(hwnd) {
                    if wp & 0xFF != 0 {
                        w.style |= ES_PASSWORD;
                    } else {
                        w.style &= !ES_PASSWORD;
                    }
                }
                if wp & 0xFF != 0 {
                    c.e.style |= ES_PASSWORD;
                } else {
                    c.e.style &= !ES_PASSWORD;
                }
                c.redraw = true;
            }
            0
        }
        EM_EMPTYUNDOBUFFER => {
            c.e.empty_undo();
            0
        }
        EM_GETFIRSTVISIBLELINE => {
            return Some(if c.e.ml() { c.e.y_offset } else { c.e.x_offset } as u32);
        }
        EM_SETREADONLY => {
            let old = c.e.style;
            if let Some(w) = user(cpu).window_mut(hwnd) {
                if wp != 0 {
                    w.style |= ES_READONLY;
                } else {
                    w.style &= !ES_READONLY;
                }
            }
            if wp != 0 {
                c.e.style |= ES_READONLY;
            } else {
                c.e.style &= !ES_READONLY;
            }
            if old != c.e.style {
                c.redraw = true;
            }
            1
        }
        EM_SETWORDBREAKPROC => {
            c.e.word_break = lp;
            0
        }
        EM_GETWORDBREAKPROC => return Some(c.e.word_break),
        EM_GETPASSWORDCHAR => return Some(c.e.password),
        EM_SETMARGINS => {
            c.set_margins(wp, lp & 0xFFFF, lp >> 16);
            c.redraw = true;
            0
        }
        EM_GETMARGINS => return Some(make_long(c.e.left_margin, c.e.right_margin)),
        EM_GETLIMITTEXT => return Some(c.e.limit),
        EM_POSFROMCHAR => {
            if wp as i32 >= c.e.len() as i32 || (wp as i32) < 0 {
                return Some(u32::MAX);
            }
            let (x, y) = c.e.pos_from_char(&c.e.lines(), wp as i32, false);
            return Some(make_long(x, y));
        }
        EM_CHARFROMPOS => {
            let (x, y) = point_of(lp);
            if !c.client.contains(x, y) {
                return Some(u32::MAX);
            }
            let lines = c.e.lines();
            let index = c.e.char_from_pos(&lines, x, y).0;
            let line = c.e.line_from_char(&lines, index);
            return Some(make_long(index, line));
        }
        WM_NCDESTROY => {
            if let Some(w) = user(cpu).window_mut(hwnd) {
                w.control.edit = Edit::default();
            }
            return Some(0);
        }
        WM_GETDLGCODE => {
            let mut code = DLGC_HASSETSEL | DLGC_WANTCHARS | DLGC_WANTARROWS;
            if c.e.ml() {
                code |= DLGC_WANTALLKEYS;
            }
            if lp != 0 {
                c.e.flags |= EF_DIALOGMODE;
                let m = read_words(cpu, lp, 3);
                if m[1] == WM_KEYDOWN
                    && (m[2] == VK_RETURN || m[2] == VK_ESCAPE)
                    && c.in_combo(cpu)
                    && super::combobox::state_of(cpu, c.e.parent).2
                {
                    code |= DLGC_WANTMESSAGE;
                }
            }
            code
        }
        WM_CHAR => {
            let ch = wp & 0xFF;
            if c.in_combo(cpu) && (ch == VK_RETURN || ch == VK_ESCAPE) {
                if super::combobox::state_of(cpu, c.e.parent).2 {
                    c.out.send(c.e.parent, WM_KEYDOWN, ch, 0);
                }
                0
            } else {
                char_(cpu, &mut c, ch as u8);
                1
            }
        }
        WM_CLEAR => {
            c.clear();
            0
        }
        WM_COMMAND => {
            if wp >> 16 == 0 && lp == 0 {
                match wp & 0xFFFF {
                    EM_UNDO => c.out.send(hwnd, WM_UNDO, 0, 0),
                    WM_CUT | WM_COPY | WM_PASTE | WM_CLEAR => c.out.send(hwnd, wp & 0xFFFF, 0, 0),
                    EM_SETSEL => {
                        c.set_sel(0, u32::MAX, false);
                        c.scroll_caret();
                    }
                    _ => {}
                }
            }
            0
        }
        WM_COPY => {
            c.copy(cpu);
            0
        }
        WM_CUT => {
            c.copy(cpu);
            c.clear();
            0
        }
        WM_PASTE => {
            c.paste(cpu);
            0
        }
        WM_CREATE => create(cpu, &mut c, lp),
        WM_ENABLE => {
            if wp != 0 {
                c.e.flags &= !EF_DISABLED;
            } else {
                c.e.flags |= EF_DISABLED;
            }
            c.redraw = true;
            0
        }
        WM_ERASEBKGND => return Some(1),
        WM_GETFONT => return Some(c.e.font),
        WM_GETTEXT => {
            if wp == 0 {
                return Some(0);
            }
            let text = c.e.text_with_breaks();
            return Some(copy_out(cpu, lp, wp, &text));
        }
        WM_GETTEXTLENGTH => return Some(c.e.text_with_breaks().len() as u32),
        WM_HSCROLL => h_scroll(&mut c, wp & 0xFFFF, (wp >> 16) as i16 as i32),
        WM_VSCROLL => v_scroll(&mut c, wp & 0xFFFF, (wp >> 16) as i16 as i32),
        WM_KEYDOWN => {
            if key_down(cpu, VK_MENU) {
                return Some(0);
            }
            let wants = !c.e.ml() || c.e.style & ES_WANTRETURN == 0;
            let dialog = c.e.flags & EF_DIALOGMODE != 0;
            if wp == VK_RETURN && wants && dialog && !key_down(cpu, VK_CONTROL) {
                // DM_GETDEFID, and the default button gets the key.
                return start(cpu, EDIT, K_RETURN, vec![hwnd, c.e.parent], 0);
            }
            keydown(cpu, &mut c, wp);
            1
        }
        WM_KILLFOCUS => {
            c.e.flags &= !EF_FOCUSED;
            input::destroy_caret(cpu, &[]);
            if c.e.style & ES_NOHIDESEL == 0 {
                c.redraw = true;
            }
            c.notify(EN_KILLFOCUS);
            0
        }
        WM_LBUTTONDBLCLK => {
            c.e.flags |= EF_CAPTURE;
            c.out.call("SetCapture", &[hwnd]);
            c.select_word();
            0
        }
        WM_LBUTTONDOWN => {
            c.e.flags |= EF_CAPTURE;
            c.out.call("SetCapture", &[hwnd]);
            let (x, y) = point_of(lp);
            let (x, y) = c.confine(x, y);
            let lines = c.e.lines();
            let (e, after) = c.e.char_from_pos(&lines, x, y);
            let s = if wp & MK_SHIFT != 0 {
                c.e.sel_start
            } else {
                e as u32
            };
            c.set_sel(s, e as u32, after);
            c.scroll_caret();
            if c.e.flags & EF_FOCUSED == 0 {
                c.out.call("SetFocus", &[hwnd]);
            }
            0
        }
        WM_LBUTTONUP => {
            if c.e.flags & EF_CAPTURE != 0 && user(cpu).input.capture == hwnd {
                c.out.call("ReleaseCapture", &[]);
            }
            c.e.flags &= !EF_CAPTURE;
            0
        }
        WM_MBUTTONDOWN => {
            c.out.send(hwnd, WM_PASTE, 0, 0);
            0
        }
        WM_MOUSEMOVE => {
            if c.e.flags & EF_CAPTURE != 0 && user(cpu).input.capture == hwnd {
                let (x, y) = point_of(lp);
                let (x, y) = c.confine(x, y);
                let lines = c.e.lines();
                let (e, after) = c.e.char_from_pos(&lines, x, y);
                c.set_sel(c.e.sel_start, e as u32, after);
            }
            0
        }
        WM_PAINT | WM_PRINTCLIENT => {
            let parent = user(cpu).window(hwnd).map_or(0, |w| w.parent);
            let to = if parent == super::super::DESKTOP {
                0
            } else {
                parent
            };
            let static_ = c.e.flags & EF_DISABLED != 0 || c.e.style & ES_READONLY != 0;
            let ctl = if static_ {
                WM_CTLCOLORSTATIC
            } else {
                WM_CTLCOLOREDIT
            };
            let hdc = if msg == WM_PAINT { wp } else { wp.max(1) };
            return paint_start(cpu, EDIT, hwnd, hdc, ctl, to, &[]);
        }
        WM_SETFOCUS => {
            c.e.flags |= EF_FOCUSED;
            if c.e.style & ES_NOHIDESEL == 0 {
                c.redraw = true;
            }
            input::create_caret(cpu, &[hwnd, 0, 1, c.e.line_height as u32]);
            let lines = c.e.lines();
            let after = c.e.flags & EF_AFTER_WRAP != 0;
            let (x, y) = c.e.pos_from_char(&lines, c.e.sel_end as i32, after);
            input::set_caret_pos(cpu, &[x as u32, y as u32]);
            input::show_caret(cpu, &[hwnd]);
            c.notify(EN_SETFOCUS);
            0
        }
        WM_SETFONT => {
            c.set_font(cpu, wp, lp & 0xFFFF != 0);
            0
        }
        WM_SETTEXT => {
            let text = if lp != 0 {
                bytes_at(cpu, lp).unwrap_or_default()
            } else {
                vec![]
            };
            c.set_text(cpu, &text);
            1
        }
        WM_SIZE => {
            if wp == SIZE_MAXIMIZED || wp == SIZE_RESTORED {
                let r = Rect::new(0, 0, (lp & 0xFFFF) as i32, (lp >> 16) as i32);
                c.client = r;
                c.set_rect_np(cpu, r);
                c.redraw = true;
            }
            0
        }
        WM_STYLECHANGED => {
            if wp == GWL_STYLE && lp != 0 {
                let new = cpu.bus.guest_read_32(lp + 4);
                let mut mask = ES_UPPERCASE | ES_LOWERCASE | ES_NUMBER;
                if c.e.ml() {
                    mask |= ES_WANTRETURN;
                }
                let mut new = new & mask;
                if new & ES_NUMBER != 0 {
                } else if new & ES_LOWERCASE != 0 {
                    new &= !ES_UPPERCASE;
                }
                c.e.style = (c.e.style & !mask) | new;
            }
            0
        }
        WM_STYLECHANGING => 0,
        WM_SYSKEYDOWN => {
            if wp == VK_BACK && lp & 0x2000_0000 != 0 {
                if c.e.can_undo() {
                    c.undo();
                }
                0
            } else if (wp == VK_UP || wp == VK_DOWN) && c.check_combo(cpu, WM_SYSKEYDOWN, wp) {
                0
            } else {
                return def_window_proc(cpu, args);
            }
        }
        _ => return def_window_proc(cpu, args),
    };
    c.store(cpu, result)
}

/// WM_NCCREATE: the state, from the CREATESTRUCT at `cs`.
/// WM_SETTEXT's work on the edit `hwnd`, for a combo box that sets the
/// text of its edit: the calls it makes (the notifications).
pub(super) fn set_window_text(cpu: &mut Cpu, hwnd: u32, text: &[u8]) -> Calls {
    let created = user(cpu)
        .window(hwnd)
        .is_some_and(|w| w.control.edit.flags & EF_CREATED != 0);
    if !created {
        return Calls::default();
    }
    let Some(mut c) = Ctx::load(cpu, hwnd) else {
        return Calls::default();
    };
    c.set_text(cpu, text);
    c.finish(cpu)
}

fn nc_create(cpu: &mut Cpu, hwnd: u32, cs: u32) -> u32 {
    let parent = cpu.bus.guest_read_32(cs + 12);
    let mut style = cpu.bus.guest_read_32(cs + 32);
    let ex = cpu.bus.guest_read_32(cs + 44);
    let mut e = Edit {
        parent,
        flags: EF_CREATED,
        limit: BUFLIMIT_INITIAL,
        ..Edit::default()
    };
    if style & WS_DISABLED != 0 {
        e.flags |= EF_DISABLED;
    }
    const WS_EX_RIGHT: u32 = 0x1000;
    if ex & WS_EX_RIGHT != 0 {
        style |= ES_RIGHT;
    }
    if style & ES_NUMBER == 0 && style & ES_LOWERCASE != 0 {
        style &= !ES_UPPERCASE;
    }
    if style & ES_MULTILINE != 0 {
        if style & WS_VSCROLL != 0 {
            style |= ES_AUTOVSCROLL;
        }
        if style & WS_HSCROLL != 0 {
            style |= ES_AUTOHSCROLL;
        }
        style &= !ES_PASSWORD;
        if style & (ES_CENTER | ES_RIGHT) != 0 {
            if style & ES_RIGHT != 0 {
                style &= !ES_CENTER;
            }
            style &= !(WS_HSCROLL | ES_AUTOHSCROLL);
        }
    } else {
        if style & ES_RIGHT != 0 && style & ES_CENTER != 0 {
            style &= !ES_CENTER;
        }
        style &= !(WS_HSCROLL | WS_VSCROLL);
        if style & ES_PASSWORD != 0 {
            e.password = b'*' as u32;
        }
    }
    let client_edge = ex & super::super::window::WS_EX_CLIENTEDGE != 0;
    if client_edge {
        style &= !WS_BORDER;
    }
    e.style = style;
    if let Some(w) = user(cpu).window_mut(hwnd) {
        // A WS_BORDER edit without WS_EX_CLIENTEDGE draws its own border,
        // so the window has none.
        if !client_edge && style & WS_BORDER != 0 {
            w.style &= !WS_BORDER;
        }
        w.control.edit = e;
    }
    1
}

/// WM_CREATE: the system font, the format rectangle, and the window's
/// name as the text.
fn create(cpu: &mut Cpu, c: &mut Ctx, cs: u32) -> u32 {
    c.set_font(cpu, 0, false);
    c.e.empty_undo();
    let client = c.client;
    c.set_rect_np(cpu, client);
    let name = if cs != 0 {
        cpu.bus.guest_read_32(cs + 36)
    } else {
        0
    };
    if name >= 0x1_0000 {
        let text = bytes_at(cpu, name).unwrap_or_default();
        if !text.is_empty() {
            c.replace_sel(false, &text, false, false);
            c.e.sel_start = 0;
            c.e.sel_end = 0;
            c.scroll_caret();
        }
    }
    1
}

/// EM_GETHANDLE: a buffer of the local heap with the text.
fn get_handle(cpu: &mut Cpu, c: &mut Ctx) -> u32 {
    let text = c.e.text.clone();
    let at = crate::win32::kernel32::global_address(cpu, c.e.handle);
    let size = crate::win32::kernel32::global_size(cpu, c.e.handle);
    if c.e.handle != 0 && at != 0 && size as usize > text.len() {
        let mut bytes = text;
        bytes.push(0);
        cpu.bus.guest_write_bytes(at, &bytes);
        return c.e.handle;
    }
    c.e.handle = global_text(cpu, LMEM_MOVEABLE_ZERO, &text);
    c.e.handle
}

/// EM_GETLINE(line, buffer): the buffer's first word is its size.
fn get_line(cpu: &mut Cpu, c: &Ctx, line: i32, dst: u32) -> u32 {
    let lines = c.e.lines();
    let line = if c.e.ml() {
        if line >= lines.len() as i32 || line < 0 {
            return 0;
        }
        line
    } else {
        0
    };
    let i = c.e.line_index(&lines, line).max(0) as usize;
    let len = c.e.line_length(&lines, i as i32).max(0) as usize;
    let size = (cpu.bus.guest_read_32(dst) & 0xFFFF) as usize;
    let n = len.min(size);
    let mut bytes = c.e.text[i..i + n].to_vec();
    if n < size {
        bytes.push(0);
    }
    cpu.bus.guest_write_bytes(dst, &bytes);
    n as u32
}

/// EM_SCROLL(action).
fn scroll(c: &mut Ctx, action: u32) -> u32 {
    if !c.e.ml() {
        return 0;
    }
    let lines = c.e.lines();
    let count = lines.len() as i32;
    let page = (c.e.format.bottom - c.e.format.top) / c.e.line_height();
    let y = c.e.y_offset;
    let mut dy = match action {
        SB_LINEUP if y != 0 => -1,
        SB_LINEDOWN if y < count - 1 => 1,
        SB_PAGEUP if y != 0 => -page,
        SB_PAGEDOWN if y < count - 1 => page,
        SB_LINEUP | SB_LINEDOWN | SB_PAGEUP | SB_PAGEDOWN => 0,
        _ => return 0,
    };
    if dy != 0 {
        let vlc = c.e.vlc();
        if y + dy > count - vlc {
            dy = (count - vlc).max(0) - y;
        }
        if dy != 0 {
            c.line_scroll_internal(0, dy);
            return make_long(dy, 1);
        }
    }
    0
}

/// WM_HSCROLL(action, position).
fn h_scroll(c: &mut Ctx, action: u32, pos: i32) -> u32 {
    let lines = c.e.lines();
    let fw = c.e.format_width();
    let tw = c.e.text_width(&lines);
    let cw = c.e.char_width.max(1);
    let x = c.e.x_offset;
    if !c.e.ml() || c.e.style & ES_AUTOHSCROLL == 0 && c.e.style & WS_HSCROLL == 0 {
        return 0;
    }
    let dx = match action {
        SB_LINEUP if x != 0 => -cw,
        SB_LINEDOWN if x < tw => cw,
        SB_PAGEUP if x != 0 => -fw / HSCROLL_FRACTION / cw * cw,
        SB_PAGEDOWN if x < tw => fw / HSCROLL_FRACTION / cw * cw,
        SB_TOP => -x,
        SB_BOTTOM => tw - fw - x,
        SB_THUMBTRACK | SB_THUMBPOSITION => pos - x,
        _ => 0,
    };
    if dx != 0 {
        c.line_scroll_internal(dx, 0);
    }
    0
}

/// WM_VSCROLL(action, position).
fn v_scroll(c: &mut Ctx, action: u32, pos: i32) -> u32 {
    if !c.e.ml() {
        return 0;
    }
    match action {
        SB_LINEUP | SB_LINEDOWN | SB_PAGEUP | SB_PAGEDOWN => {
            scroll(c, action);
        }
        SB_TOP => {
            let dy = -c.e.y_offset;
            c.line_scroll_internal(0, dy);
        }
        SB_BOTTOM => {
            let count = c.e.lines().len() as i32;
            let dy = count - 1 - c.e.y_offset;
            c.line_scroll_internal(0, dy);
        }
        SB_THUMBTRACK | SB_THUMBPOSITION => {
            let dy = pos - c.e.y_offset;
            c.line_scroll_internal(0, dy);
        }
        _ => {}
    }
    0
}

/// WM_CHAR.
fn char_(cpu: &mut Cpu, c: &mut Ctx, ch: u8) {
    let control = key_down(cpu, VK_CONTROL);
    let style = c.e.style;
    let ml = c.e.ml();
    let readonly = style & ES_READONLY != 0;
    let dialog = c.e.flags & EF_DIALOGMODE != 0;
    match ch {
        b'\r' | b'\n' => {
            if ch == b'\r' && ml && style & ES_WANTRETURN == 0 && dialog {
                return;
            }
            if ml {
                if readonly {
                    c.move_home(false, false);
                    let lh = c.e.line_height();
                    c.move_vertically(false, lh);
                } else {
                    c.replace_sel(true, b"\r\n", true, true);
                }
            }
        }
        b'\t' => {
            if ml && !readonly && !dialog {
                c.replace_sel(true, b"\t", true, true);
            }
        }
        0x08 => {
            if !readonly && !control {
                if c.e.sel_start != c.e.sel_end {
                    c.clear();
                } else {
                    c.set_sel(u32::MAX, 0, false);
                    c.move_backward(true);
                    c.clear();
                }
            }
        }
        0x03 => {
            if style & ES_PASSWORD == 0 {
                c.out.send(c.hwnd, WM_COPY, 0, 0);
            }
        }
        0x16 => {
            if !readonly {
                c.out.send(c.hwnd, WM_PASTE, 0, 0);
            }
        }
        0x18 => {
            if !readonly && style & ES_PASSWORD == 0 {
                c.out.send(c.hwnd, WM_CUT, 0, 0);
            }
        }
        0x1A => {
            if !readonly {
                c.out.send(c.hwnd, WM_UNDO, 0, 0);
            }
        }
        _ => {
            if style & ES_NUMBER != 0 && !ch.is_ascii_digit() {
                return;
            }
            if !readonly && ch >= b' ' && ch != 127 {
                c.replace_sel(true, &[ch], true, true);
            }
        }
    }
}

/// WM_KEYDOWN, but for VK_RETURN in a dialog.
fn keydown(cpu: &mut Cpu, c: &mut Ctx, key: u32) {
    let shift = key_down(cpu, VK_SHIFT);
    let control = key_down(cpu, VK_CONTROL);
    let ml = c.e.ml();
    let readonly = c.e.style & ES_READONLY != 0;
    let dialog = c.e.flags & EF_DIALOGMODE != 0;
    let lh = c.e.line_height();
    let page = c.e.format.bottom - c.e.format.top;
    match key {
        VK_F4 | VK_UP | VK_LEFT => {
            if key != VK_LEFT && (c.check_combo(cpu, WM_KEYDOWN, key) || key == VK_F4) {
                return;
            }
            if ml && key == VK_UP {
                c.move_vertically(shift, -lh);
            } else if control {
                c.move_word(shift, false);
            } else {
                c.move_backward(shift);
            }
        }
        VK_DOWN | VK_RIGHT => {
            if key == VK_DOWN && c.check_combo(cpu, WM_KEYDOWN, key) {
                return;
            }
            if ml && key == VK_DOWN {
                c.move_vertically(shift, lh);
            } else if control {
                c.move_word(shift, true);
            } else {
                c.move_forward(shift);
            }
        }
        VK_HOME => c.move_home(shift, control),
        VK_END => c.move_end(shift, control),
        VK_PRIOR | VK_NEXT => {
            if ml {
                c.move_vertically(shift, if key == VK_PRIOR { -page } else { page });
            } else {
                c.check_combo(cpu, WM_KEYDOWN, key);
            }
        }
        VK_DELETE => {
            if !readonly && !(shift && control) {
                if c.e.sel_start != c.e.sel_end {
                    if shift {
                        c.copy(cpu);
                    }
                    c.clear();
                } else {
                    c.set_sel(u32::MAX, 0, false);
                    if shift {
                        c.move_backward(true);
                    } else if control {
                        c.move_end(true, false);
                    } else {
                        c.move_forward(true);
                    }
                    c.clear();
                }
            }
        }
        VK_INSERT => {
            if shift {
                if !readonly {
                    c.paste(cpu);
                }
            } else if control {
                c.copy(cpu);
            }
        }
        VK_ESCAPE => {
            if ml && dialog {
                super::super::message::post_message(cpu, &[c.e.parent, WM_CLOSE, 0, 0]);
            }
        }
        VK_TAB if ml && dialog => c.out.send(c.e.parent, WM_NEXTDLGCTL, shift as u32, 0),
        _ => {}
    }
}

// The tasks.

/// The Edit module's steps of the controls' task.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match super::kind(t) {
        K_CALLS => calls_step(cpu, t, value),
        K_PAINT => paint_step(cpu, t, value),
        _ => return_step(cpu, t, value),
    }
}

/// VK_RETURN in a dialog: the default button gets it.
fn return_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let parent = t.var(2);
    match t.step {
        0 => super::super::window::send(parent, DM_GETDEFID, 0, 0, 1),
        1 => {
            if value >> 16 != DC_HASDEFID {
                return Next::Done(1);
            }
            let child = child_with_id(user(cpu), parent, value & 0xFFFF);
            if child == 0 {
                return Next::Done(1);
            }
            t.set(3, child);
            super::super::window::send(parent, WM_NEXTDLGCTL, child, 1, 2)
        }
        _ => {
            let child = t.var(3);
            super::super::message::post_message(cpu, &[child, WM_KEYDOWN, VK_RETURN, 0]);
            Next::Done(1)
        }
    }
}

fn paint_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if let Some(next) = paint_head(t, value) {
        return next;
    }
    if t.step == 3 {
        draw(cpu, t.var(P_HWND), t.var(P_HDC), t.var(P_BRUSH));
        return paint_end(t, 4);
    }
    Next::Done(0)
}

/// Draw the edit `hwnd` on `hdc` with `brush`, as EDIT_WM_Paint does.
fn draw(cpu: &mut Cpu, hwnd: u32, hdc: u32, brush: u32) {
    let state = user(cpu);
    let Some(w) = state.window(hwnd) else {
        return;
    };
    let e = w.control.edit.clone();
    let client = client_of(state, hwnd);
    let brush = default_brush(cpu, hdc, brush, COLOR_WINDOW);
    let enabled = e.flags & EF_DISABLED == 0;
    let rev = enabled && (e.flags & EF_FOCUSED != 0 || e.style & ES_NOHIDESEL != 0);
    dc::save_dc(cpu, &[hdc]);
    clip(cpu, hdc, client);
    let mut inner = client;
    if e.style & WS_BORDER != 0 {
        let mut rc = client;
        if e.ml() {
            if e.style & WS_HSCROLL != 0 {
                rc.bottom += 1;
            }
            if e.style & WS_VSCROLL != 0 {
                rc.right += 1;
            }
        }
        let frame = sysparams::brush(cpu, COLOR_WINDOWFRAME);
        paint::fill(
            cpu,
            hdc,
            Rect::new(rc.left, rc.top, rc.right, rc.top + 1),
            frame,
        );
        paint::fill(
            cpu,
            hdc,
            Rect::new(rc.left, rc.top, rc.left + 1, rc.bottom),
            frame,
        );
        paint::fill(
            cpu,
            hdc,
            Rect::new(rc.left, rc.bottom - 1, rc.right, rc.bottom),
            frame,
        );
        paint::fill(
            cpu,
            hdc,
            Rect::new(rc.right - 1, rc.top, rc.right, rc.bottom),
            frame,
        );
        inner = Rect::new(
            rc.left + 1,
            rc.top + 1,
            (rc.right - 1).max(rc.left + 1),
            (rc.bottom - 1).max(rc.top + 1),
        );
        clip(cpu, hdc, inner);
    }
    paint::fill(cpu, hdc, inner, brush);
    clip(cpu, hdc, e.format);
    select_font(cpu, hdc, e.font);
    if !enabled {
        let gray = user(cpu).color(COLOR_GRAYTEXT);
        dc::set_text_color(cpu, &[hdc, gray]);
    }
    let lines = e.lines();
    let (first, last) = if e.ml() {
        let vlc = e.vlc();
        (e.y_offset, (e.y_offset + vlc).min(lines.len() as i32 - 1))
    } else {
        (0, 0)
    };
    let shown = e.shown().into_owned();
    let (s, en) = order(e.sel_start, e.sel_end);
    for l in first.max(0)..=last {
        let line = lines[l as usize];
        let (x, y) = e.pos_from_char(&lines, line.index as i32, false);
        let li = line.index;
        let end = li + line.net;
        let s = (s as usize).clamp(li, end);
        let en = (en as usize).clamp(li, end);
        let segments = if rev && s != en {
            vec![(li, s, false), (s, en, true), (en, end, false)]
        } else {
            vec![(li, end, false)]
        };
        let mut x = x;
        for (from, to, selected) in segments {
            x += draw_run(cpu, hdc, &e, &shown, (x, y), li, from, to, selected);
        }
    }
    dc::restore_dc(cpu, &[hdc, u32::MAX]);
}

/// Draw `from` to `to` of the line starting at `start` at (`x`, `y`),
/// a multi-line edit's tabs as gaps: its width.
#[allow(clippy::too_many_arguments)]
fn draw_run(
    cpu: &mut Cpu,
    hdc: u32,
    e: &Edit,
    shown: &[u8],
    (x, y): (i32, i32),
    start: usize,
    from: usize,
    to: usize,
    selected: bool,
) -> i32 {
    if from >= to {
        return 0;
    }
    let mut cx = x;
    let mut run = from;
    for i in from..=to {
        if i == to || e.ml() && shown[i] == b'\t' {
            text_out(cpu, hdc, (cx, y), &shown[run..i], selected);
            cx += e.extent(start, run, i);
            if i < to {
                let w = e.extent(start, i, i + 1);
                if selected {
                    let r = Rect::new(cx, y, cx + w, y + e.line_height());
                    let b = sysparams::brush(cpu, COLOR_HIGHLIGHT);
                    paint::fill(cpu, hdc, r, b);
                }
                cx += w;
            }
            run = i + 1;
        }
    }
    cx - x
}

/// Clip `hdc` to `r` as well.
pub(super) fn clip(cpu: &mut Cpu, hdc: u32, r: Rect) {
    let args = [
        hdc,
        r.left as u32,
        r.top as u32,
        r.right as u32,
        r.bottom as u32,
    ];
    dc::intersect_clip_rect(cpu, &args);
}

crate::state_fields!(Edit {
    style,
    parent,
    text,
    limit,
    sel_start,
    sel_end,
    flags,
    font,
    line_height,
    char_width,
    widths,
    x_offset,
    y_offset,
    left_margin,
    right_margin,
    format,
    password,
    undo_text,
    undo_pos,
    undo_count,
    tabs,
    word_break,
    handle,
    bars
});
