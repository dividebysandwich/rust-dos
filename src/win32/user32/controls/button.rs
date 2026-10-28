//! The Button class: push buttons, check boxes, radio buttons, group
//! boxes, user buttons and owner-drawn buttons, as Wine's button.c has
//! them, with Windows 95's look.
//!
//! Painting asks the parent for colors (WM_CTLCOLORBTN, or
//! WM_CTLCOLORSTATIC for check boxes, radio buttons and group boxes) and
//! so is a task, and so are clicks, which send the button BM_SETSTATE
//! and BM_SETCHECK and its parent WM_COMMAND. The painting steps here
//! (`open`, `prepare`, `close`) and the notification (`notify`) serve
//! the Static and ScrollBar classes as well.

use super::{BUTTON, kind, start};
use crate::cpu::Cpu;
use crate::win32::gdi32::{self, Rect};
use crate::win32::task::{Next, Task};
use crate::win32::user32::message::{
    WM_COMMAND, WM_CREATE, WM_CTLCOLORBTN, WM_CTLCOLORMSGBOX, WM_CTLCOLORSTATIC, WM_DRAWITEM,
    WM_ENABLE, WM_ERASEBKGND, WM_GETDLGCODE, WM_GETFONT, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS,
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT,
    WM_PRINTCLIENT, WM_SETFOCUS, WM_SETFONT, WM_SETTEXT, WM_SYSCOLORCHANGE,
};
use crate::win32::user32::paint::{
    self, BF_FLAT, BF_RECT, DFC_BUTTON, DFCS_BUTTON3STATE, DFCS_BUTTONCHECK, DFCS_BUTTONPUSH,
    DFCS_BUTTONRADIO, DFCS_CHECKED, DFCS_FLAT, DFCS_INACTIVE, DFCS_MONO, DFCS_PUSHED, DI_NORMAL,
    EDGE_ETCHED,
};
use crate::win32::user32::sysparams::{
    COLOR_BTNFACE, COLOR_BTNHIGHLIGHT, COLOR_BTNSHADOW, COLOR_BTNTEXT, COLOR_GRAYTEXT,
    COLOR_WINDOWFRAME,
};
use crate::win32::user32::text::{
    self, DT_BOTTOM, DT_CALCRECT, DT_CENTER, DT_NOCLIP, DT_RIGHT, DT_SINGLELINE, DT_VCENTER,
    DT_WORDBREAK,
};
use crate::win32::user32::window::{self, WS_CHILD, WS_DISABLED, WS_GROUP, WS_POPUP, WS_TABSTOP};
use crate::win32::user32::{DESKTOP, address_of, defwnd, message, resource, user};

/// What a button keeps: its state (`CHECKED` and the rest), its font
/// and its image (BM_SETIMAGE).
#[derive(Clone, Debug, Default)]
pub struct Button {
    pub state: u32,
    pub font: u32,
    pub image: u32,
}

// The kinds of buttons, the low four bits of the style.
const BS_PUSHBUTTON: u32 = 0x0;
const BS_DEFPUSHBUTTON: u32 = 0x1;
const BS_CHECKBOX: u32 = 0x2;
const BS_AUTOCHECKBOX: u32 = 0x3;
const BS_RADIOBUTTON: u32 = 0x4;
const BS_3STATE: u32 = 0x5;
const BS_AUTO3STATE: u32 = 0x6;
const BS_GROUPBOX: u32 = 0x7;
const BS_USERBUTTON: u32 = 0x8;
const BS_AUTORADIOBUTTON: u32 = 0x9;
#[cfg(test)]
const BS_PUSHBOX: u32 = 0xA;
const BS_OWNERDRAW: u32 = 0xB;

const BS_LEFTTEXT: u32 = 0x20;
const BS_ICON: u32 = 0x40;
const BS_BITMAP: u32 = 0x80;
const BS_LEFT: u32 = 0x100;
const BS_RIGHT: u32 = 0x200;
const BS_CENTER: u32 = 0x300;
const BS_TOP: u32 = 0x400;
const BS_BOTTOM: u32 = 0x800;
const BS_VCENTER: u32 = 0xC00;
const BS_PUSHLIKE: u32 = 0x1000;
const BS_MULTILINE: u32 = 0x2000;
const BS_NOTIFY: u32 = 0x4000;
const BS_FLAT: u32 = 0x8000;

const WS_EX_RIGHT: u32 = 0x1000;

// The state.
const CHECKED: u32 = 0x01;
const THREE_STATE: u32 = 0x02;
const HIGHLIGHTED: u32 = 0x04;
const HASFOCUS: u32 = 0x08;
const NSTATES: u32 = 0x0F;
/// The mouse button or the space bar is down on it.
const PRESSED: u32 = 0x40;

pub const BM_GETCHECK: u32 = 0x00F0;
pub const BM_SETCHECK: u32 = 0x00F1;
pub const BM_GETSTATE: u32 = 0x00F2;
pub const BM_SETSTATE: u32 = 0x00F3;
pub const BM_SETSTYLE: u32 = 0x00F4;
pub const BM_CLICK: u32 = 0x00F5;
pub const BM_GETIMAGE: u32 = 0x00F6;
pub const BM_SETIMAGE: u32 = 0x00F7;

const BN_CLICKED: u32 = 0;
const BN_PAINT: u32 = 1;
const BN_HILITE: u32 = 2;
const BN_UNHILITE: u32 = 3;
const BN_DOUBLECLICKED: u32 = 5;
const BN_SETFOCUS: u32 = 6;
const BN_KILLFOCUS: u32 = 7;

pub const DLGC_DEFPUSHBUTTON: u32 = 0x0010;
pub const DLGC_UNDEFPUSHBUTTON: u32 = 0x0020;
pub const DLGC_RADIOBUTTON: u32 = 0x0040;
pub const DLGC_STATIC: u32 = 0x0100;
pub const DLGC_BUTTON: u32 = 0x2000;

pub const ODT_BUTTON: u32 = 4;
pub const ODA_DRAWENTIRE: u32 = 1;
pub const ODA_SELECT: u32 = 2;
pub const ODA_FOCUS: u32 = 4;
pub const ODS_SELECTED: u32 = 0x01;
pub const ODS_DISABLED: u32 = 0x04;
pub const ODS_FOCUS: u32 = 0x10;

const IMAGE_BITMAP: u32 = 0;
const IMAGE_ICON: u32 = 1;

const WM_CAPTURECHANGED: u32 = 0x0215;
const VK_SPACE: u32 = 0x20;
const MK_LBUTTON: u32 = 0x01;
const HTTRANSPARENT: u32 = -1i32 as u32;
const TRANSPARENT: u32 = 1;
const OPAQUE: u32 = 2;

/// The size of the check box and the radio button.
const CHECK_SIZE: i32 = 13;

// Painting, for the controls of all the modules: the variables of a
// painting task after its kind.

/// The control.
pub const P_HWND: usize = 1;
/// The DC.
pub const P_HDC: usize = 2;
/// How the DC was had: one of `HOW_GIVEN`, `HOW_PAINT`, `HOW_DC`.
pub const P_HOW: usize = 3;
/// The DC's clip region before, or 0.
pub const P_CLIP: usize = 4;
/// The font that was selected before the control's, or 0.
pub const P_FONT: usize = 5;
/// What the task returns.
pub const P_RESULT: usize = 6;
/// The ODA_ action.
pub const P_ACTION: usize = 7;
/// The DC's background mode before, or 0.
pub const P_MODE: usize = 8;
/// The step that goes on after the painting, or 0 for none.
pub const P_THEN: usize = 9;
/// The first variable of the module's own.
pub const P_OWN: usize = 10;

/// The DC is wParam's.
pub const HOW_GIVEN: u32 = 0;
/// BeginPaint gives it.
pub const HOW_PAINT: u32 = 1;
/// GetDC gives it.
pub const HOW_DC: u32 = 2;

/// The scratch space of painting: the PAINTSTRUCT, then the
/// DRAWITEMSTRUCT.
pub const PAINT_SCRATCH: u32 = 112;
pub const DRAWITEM: u32 = 64;

/// The kind of task that sends the parent WM_COMMAND, in every module.
pub const K_NOTIFY: u32 = 0xFF;

/// The window a control notifies and asks for colors: its parent, a
/// popup's owner, or else itself (GetParent).
pub fn parent_of(cpu: &mut Cpu, hwnd: u32) -> u32 {
    let Some(w) = user(cpu).window(hwnd) else {
        return hwnd;
    };
    let p = if w.style & WS_CHILD != 0 {
        w.parent
    } else if w.style & WS_POPUP != 0 {
        w.owner
    } else {
        0
    };
    if p == 0 || p == DESKTOP { hwnd } else { p }
}

/// The client area of `hwnd`, in client coordinates.
pub fn client_rect(cpu: &mut Cpu, hwnd: u32) -> Rect {
    let c = user(cpu).window(hwnd).map(|w| w.client).unwrap_or_default();
    Rect::new(0, 0, c.width(), c.height())
}

/// What DefWindowProcA of `parent` does with WM_CTLCOLOR* (`msg`): set
/// the DC's colors for the kind of control and give its brush. A control
/// uses it when its parent gives no brush back.
pub fn default_color(cpu: &mut Cpu, parent: u32, hdc: u32, msg: u32) -> u32 {
    defwnd::control_color(cpu, parent, hdc, msg - WM_CTLCOLORMSGBOX)
}

/// Clip `hdc` to the client area of `hwnd`, as a control does while it
/// paints: the DC's clip region before, a new region for `unclip`, or 0
/// for none.
pub fn clip(cpu: &mut Cpu, hwnd: u32, hdc: u32) -> u32 {
    let rgn = gdi32::rgn::create_rect_rgn(cpu, &[0, 0, 1, 1]).unwrap_or(0);
    let saved = if rgn != 0 && gdi32::dc::get_clip_rgn(cpu, &[hdc, rgn]) == Some(1) {
        rgn
    } else {
        if rgn != 0 {
            gdi32::delete_object(cpu, &[rgn]);
        }
        0
    };
    let r = client_rect(cpu, hwnd);
    let args = [
        hdc,
        r.left as u32,
        r.top as u32,
        r.right as u32,
        r.bottom as u32,
    ];
    gdi32::dc::intersect_clip_rect(cpu, &args);
    saved
}

/// Give the DC back the clip region `clip` saved.
pub fn unclip(cpu: &mut Cpu, hdc: u32, saved: u32) {
    gdi32::dc::select_clip_rgn(cpu, &[hdc, saved]);
    if saved != 0 {
        gdi32::delete_object(cpu, &[saved]);
    }
}

/// DrawFocusRect on the logical rectangle `r`.
pub fn focus_rect(cpu: &mut Cpu, hdc: u32, r: Rect) {
    let Some(dc) = gdi32::gdi(cpu).dc(hdc) else {
        return;
    };
    let d = dc.rect_to_device(&r).ordered();
    gdi32::draw::focus_device_rect(cpu, hdc, d);
}

/// The variables of a painting task for `hwnd`, on the DC `hdc` if not
/// 0, else on one that BeginPaint (`begin`) or GetDC gives.
pub fn paint_vars(hwnd: u32, hdc: u32, begin: bool, action: u32) -> Vec<u32> {
    let how = if hdc != 0 {
        HOW_GIVEN
    } else if begin {
        HOW_PAINT
    } else {
        HOW_DC
    };
    let mut vars = vec![0; P_OWN - 1];
    vars[P_HWND - 1] = hwnd;
    vars[P_HDC - 1] = hdc;
    vars[P_HOW - 1] = how;
    vars[P_ACTION - 1] = action;
    vars
}

/// The first step of painting: have the DC as `P_HOW` says, going on
/// at `next`.
pub fn open(cpu: &mut Cpu, t: &mut Task, next: u16) -> Next {
    let hwnd = t.var(P_HWND);
    match t.var(P_HOW) {
        HOW_PAINT => Next::call(address_of("BeginPaint"), &[hwnd, t.scratch], next),
        HOW_DC => {
            let hdc = paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
            t.set(P_HDC, hdc);
            Next::call(0, &[], next)
        }
        _ => Next::call(0, &[], next),
    }
}

/// At the step `open` went on to, with the value it went on with: clip
/// the DC to the control and select the font `font` (0 for the DC's
/// own). The DC, or 0 if there is none.
pub fn prepare(cpu: &mut Cpu, t: &mut Task, value: u32, font: u32) -> u32 {
    if t.var(P_HOW) == HOW_PAINT {
        t.set(P_HDC, value);
    }
    let hdc = t.var(P_HDC);
    if hdc == 0 {
        return 0;
    }
    let hwnd = t.var(P_HWND);
    let saved = clip(cpu, hwnd, hdc);
    t.set(P_CLIP, saved);
    if font != 0 {
        let old = gdi32::dc::select_object(cpu, &[hdc, font]).unwrap_or(0);
        t.set(P_FONT, old);
    }
    hdc
}

/// Undo `prepare` and give the DC back. The painting is done at `done`
/// (after EndPaint), or at once.
pub fn close(cpu: &mut Cpu, t: &mut Task, done: u16) -> Next {
    let (hwnd, hdc) = (t.var(P_HWND), t.var(P_HDC));
    if hdc != 0 {
        if t.var(P_MODE) != 0 {
            gdi32::dc::set_bk_mode(cpu, &[hdc, t.var(P_MODE)]);
        }
        if t.var(P_FONT) != 0 {
            gdi32::dc::select_object(cpu, &[hdc, t.var(P_FONT)]);
        }
        unclip(cpu, hdc, t.var(P_CLIP));
    }
    match t.var(P_HOW) {
        HOW_PAINT => Next::call(address_of("EndPaint"), &[hwnd, t.scratch], done),
        HOW_DC => {
            if hdc != 0 {
                paint::release_dc(cpu, &[hwnd, hdc]);
            }
            Next::call(0, &[], done)
        }
        _ => Next::call(0, &[], done),
    }
}

/// The last step of painting: go on at `P_THEN`, or return `P_RESULT`.
pub fn finish(t: &mut Task) -> Next {
    match t.var(P_THEN) {
        0 => Next::Done(t.var(P_RESULT)),
        then => Next::call(0, &[], then as u16),
    }
}

/// Send the parent of `hwnd` WM_COMMAND with the notification `code`,
/// as the control's module `module` does; the window procedure then
/// returns `result`.
pub fn notify(cpu: &mut Cpu, module: u32, hwnd: u32, code: u32, result: u32) -> Option<u32> {
    let parent = parent_of(cpu, hwnd);
    let id = user(cpu).window(hwnd).map_or(0, |w| w.id);
    let vars = vec![parent, id & 0xFFFF | code << 16, hwnd, result];
    start(cpu, module, K_NOTIFY, vars, 0)
}

/// The steps of `notify`.
pub fn notify_step(t: &mut Task) -> Next {
    match t.step {
        0 => window::send(t.var(1), WM_COMMAND, t.var(2), t.var(3), 1),
        _ => Next::Done(t.var(4)),
    }
}

// The button's own tasks.

/// WM_PAINT, WM_PRINTCLIENT and painting at once (with GetDC).
const K_PAINT: u32 = 0;
/// An owner-drawn button's WM_ERASEBKGND.
const K_ERASE: u32 = 1;
const K_DOWN: u32 = 2;
const K_SPACE: u32 = 3;
const K_UP: u32 = 4;
const K_CAPTURE: u32 = 5;
const K_SETTEXT: u32 = 6;
const K_SETFOCUS: u32 = 7;
const K_KILLFOCUS: u32 = 8;
const K_CLICK: u32 = 9;
const K_SETCHECK: u32 = 10;

/// The steps painting starts at, in every kind of the button's task.
const STEP_PAINT: u16 = 20;

/// The variables of the button's own after the painting ones.
const V_A: usize = P_OWN;
const V_B: usize = P_OWN + 1;
/// Where the radio buttons to uncheck start.
const V_LIST: usize = P_OWN + 2;

fn button(cpu: &mut Cpu, hwnd: u32) -> Button {
    user(cpu)
        .window(hwnd)
        .map(|w| w.control.button.clone())
        .unwrap_or_default()
}

fn set_state(cpu: &mut Cpu, hwnd: u32, state: u32) {
    if let Some(w) = user(cpu).window_mut(hwnd) {
        w.control.button.state = state;
    }
}

fn style_of(cpu: &mut Cpu, hwnd: u32) -> u32 {
    user(cpu).window(hwnd).map_or(0, |w| w.style)
}

/// Whether the kind of button `kind` paints.
fn paints(kind: u32) -> bool {
    kind <= BS_AUTORADIOBUTTON || kind == BS_OWNERDRAW
}

/// Paint the button at once with `action`, if it is visible, and
/// return `result`.
fn repaint(cpu: &mut Cpu, hwnd: u32, action: u32, result: u32) -> Option<u32> {
    let kind = style_of(cpu, hwnd) & 0x0F;
    if !paints(kind) || !user(cpu).is_visible(hwnd) {
        return Some(result);
    }
    let mut vars = paint_vars(hwnd, 0, false, action);
    vars[P_RESULT - 1] = result;
    start(cpu, BUTTON, K_PAINT, vars, PAINT_SCRATCH)
}

/// In a task: paint the button at once with `action`, if it is
/// visible, and go on at `then` (0: return `result`).
fn repaint_then(cpu: &mut Cpu, t: &mut Task, action: u32, result: u32, then: u16) -> Next {
    let hwnd = t.var(P_HWND);
    t.set(P_RESULT, result);
    t.set(P_THEN, then as u32);
    let kind = style_of(cpu, hwnd) & 0x0F;
    if !paints(kind) || !user(cpu).is_visible(hwnd) {
        return finish(t);
    }
    t.set(P_HDC, 0);
    t.set(P_HOW, HOW_DC);
    t.set(P_ACTION, action);
    Next::call(0, &[], STEP_PAINT)
}

/// Start the task `kind` for `hwnd` with its own variables `own`.
fn begin(cpu: &mut Cpu, hwnd: u32, kind: u32, own: &[u32]) -> Option<u32> {
    let mut vars = paint_vars(hwnd, 0, false, ODA_DRAWENTIRE);
    vars.extend_from_slice(own);
    start(cpu, BUTTON, kind, vars, PAINT_SCRATCH)
}

fn in_client(cpu: &mut Cpu, hwnd: u32, lp: u32) -> bool {
    let (x, y) = (lp as u16 as i16 as i32, (lp >> 16) as u16 as i16 as i32);
    let r = client_rect(cpu, hwnd);
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// The Button class's window procedure.
pub fn button_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    let Some(w) = user(cpu).window(hwnd) else {
        return Some(0);
    };
    let style = w.style;
    let kind = style & 0x0F;
    let b = w.control.button.clone();
    match msg {
        WM_GETDLGCODE => Some(match kind {
            BS_USERBUTTON | BS_PUSHBUTTON => DLGC_BUTTON | DLGC_UNDEFPUSHBUTTON,
            BS_DEFPUSHBUTTON => DLGC_BUTTON | DLGC_DEFPUSHBUTTON,
            BS_RADIOBUTTON | BS_AUTORADIOBUTTON => DLGC_BUTTON | DLGC_RADIOBUTTON,
            BS_GROUPBOX => DLGC_STATIC,
            _ => DLGC_BUTTON,
        }),
        WM_ENABLE => repaint(cpu, hwnd, ODA_DRAWENTIRE, 0),
        WM_CREATE => {
            set_state(cpu, hwnd, 0);
            Some(0)
        }
        WM_ERASEBKGND => {
            if kind == BS_OWNERDRAW {
                let mut vars = paint_vars(hwnd, wp, false, 0);
                vars[P_RESULT - 1] = 1;
                return start(cpu, BUTTON, K_ERASE, vars, 0);
            }
            Some(1)
        }
        WM_PAINT | WM_PRINTCLIENT => {
            if !paints(kind) {
                return defwnd::def_window_proc(cpu, args);
            }
            let vars = paint_vars(hwnd, wp, true, ODA_DRAWENTIRE);
            start(cpu, BUTTON, K_PAINT, vars, PAINT_SCRATCH)
        }
        WM_KEYDOWN => {
            if wp == VK_SPACE {
                return begin(cpu, hwnd, K_SPACE, &[]);
            }
            Some(0)
        }
        WM_LBUTTONDBLCLK
            if style & BS_NOTIFY != 0
                || matches!(kind, BS_RADIOBUTTON | BS_USERBUTTON | BS_OWNERDRAW) =>
        {
            notify(cpu, BUTTON, hwnd, BN_DOUBLECLICKED, 0)
        }
        WM_LBUTTONDBLCLK | WM_LBUTTONDOWN => begin(cpu, hwnd, K_DOWN, &[]),
        WM_KEYUP if wp != VK_SPACE => Some(0),
        WM_KEYUP | WM_LBUTTONUP => {
            let key = (msg == WM_KEYUP) as u32;
            begin(cpu, hwnd, K_UP, &[lp, key])
        }
        WM_CAPTURECHANGED => {
            if b.state & PRESSED != 0 {
                let state = b.state & NSTATES;
                set_state(cpu, hwnd, state);
                if state & HIGHLIGHTED != 0 {
                    return begin(cpu, hwnd, K_CAPTURE, &[]);
                }
            }
            Some(0)
        }
        WM_MOUSEMOVE => {
            if wp & MK_LBUTTON != 0 && user(cpu).input.capture == hwnd {
                let inside = in_client(cpu, hwnd, lp) as u32;
                return message::send_message(cpu, &[hwnd, BM_SETSTATE, inside, 0]);
            }
            Some(0)
        }
        WM_SETTEXT => begin(cpu, hwnd, K_SETTEXT, &[wp, lp]),
        WM_SETFONT => {
            if let Some(w) = user(cpu).window_mut(hwnd) {
                w.control.button.font = wp;
            }
            if lp != 0 {
                paint::invalidate_rect(cpu, &[hwnd, 0, 1]);
            }
            Some(0)
        }
        WM_GETFONT => Some(b.font),
        WM_SETFOCUS => {
            set_state(cpu, hwnd, b.state | HASFOCUS);
            begin(cpu, hwnd, K_SETFOCUS, &[])
        }
        WM_KILLFOCUS => {
            set_state(cpu, hwnd, b.state & !HASFOCUS);
            begin(cpu, hwnd, K_KILLFOCUS, &[b.state])
        }
        WM_SYSCOLORCHANGE => {
            paint::invalidate_rect(cpu, &[hwnd, 0, 0]);
            Some(0)
        }
        BM_SETSTYLE => {
            if let Some(w) = user(cpu).window_mut(hwnd) {
                w.style = (w.style & !0x0F) | (wp & 0x0F);
            }
            if lp != 0 {
                paint::invalidate_rect(cpu, &[hwnd, 0, 1]);
            }
            Some(0)
        }
        BM_CLICK => begin(cpu, hwnd, K_CLICK, &[]),
        BM_SETIMAGE => {
            let fits = match style & (BS_BITMAP | BS_ICON) {
                BS_BITMAP => wp == IMAGE_BITMAP,
                BS_ICON => wp == IMAGE_ICON,
                _ => false,
            };
            if !fits {
                return Some(0);
            }
            if let Some(w) = user(cpu).window_mut(hwnd) {
                w.control.button.image = lp;
            }
            paint::invalidate_rect(cpu, &[hwnd, 0, 0]);
            Some(b.image)
        }
        BM_GETIMAGE => Some(b.image),
        BM_GETCHECK => Some(b.state & 3),
        BM_SETCHECK => begin(cpu, hwnd, K_SETCHECK, &[wp]),
        BM_GETSTATE => Some(b.state),
        BM_SETSTATE => {
            let state = if wp != 0 {
                b.state | HIGHLIGHTED
            } else {
                b.state & !HIGHLIGHTED
            };
            set_state(cpu, hwnd, state);
            repaint(cpu, hwnd, ODA_SELECT, 0)
        }
        WM_NCHITTEST if kind == BS_GROUPBOX => Some(HTTRANSPARENT),
        _ => defwnd::def_window_proc(cpu, args),
    }
}

/// The steps of the button's tasks.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let k = kind(t);
    if k == K_NOTIFY {
        return notify_step(t);
    }
    if t.step >= STEP_PAINT {
        return paint_step(cpu, t, value);
    }
    let hwnd = t.var(P_HWND);
    match k {
        K_ERASE => erase_step(cpu, t, value),
        K_PAINT => Next::call(0, &[], STEP_PAINT),
        K_DOWN => match t.step {
            0 => Next::call(address_of("SetCapture"), &[hwnd], 1),
            1 => Next::call(address_of("SetFocus"), &[hwnd], 2),
            2 => {
                let state = button(cpu, hwnd).state;
                set_state(cpu, hwnd, state | PRESSED);
                window::send(hwnd, BM_SETSTATE, 1, 0, 3)
            }
            _ => Next::Done(0),
        },
        K_SPACE => match t.step {
            0 => window::send(hwnd, BM_SETSTATE, 1, 0, 1),
            1 => {
                let state = button(cpu, hwnd).state;
                set_state(cpu, hwnd, state | PRESSED);
                Next::call(address_of("SetCapture"), &[hwnd], 2)
            }
            _ => Next::Done(0),
        },
        K_UP => up_step(cpu, t),
        K_CAPTURE => match t.step {
            0 => window::send(hwnd, BM_SETSTATE, 0, 0, 1),
            _ => Next::Done(0),
        },
        K_SETTEXT => settext_step(cpu, t, value),
        K_SETFOCUS => match t.step {
            0 => repaint_then(cpu, t, ODA_FOCUS, 0, 1),
            1 if style_of(cpu, hwnd) & BS_NOTIFY != 0 => notify_from(cpu, t, BN_SETFOCUS, 2),
            _ => Next::Done(0),
        },
        K_KILLFOCUS => match t.step {
            0 => repaint_then(cpu, t, ODA_FOCUS, 0, 1),
            1 => {
                if t.var(V_A) & PRESSED != 0 && user(cpu).input.capture == hwnd {
                    return Next::call(address_of("ReleaseCapture"), &[], 2);
                }
                Next::call(0, &[], 2)
            }
            2 if style_of(cpu, hwnd) & BS_NOTIFY != 0 => notify_from(cpu, t, BN_KILLFOCUS, 3),
            _ => {
                paint::invalidate_rect(cpu, &[hwnd, 0, 0]);
                Next::Done(0)
            }
        },
        K_CLICK => match t.step {
            0 => window::send(hwnd, WM_LBUTTONDOWN, 0, 0, 1),
            1 => window::send(hwnd, WM_LBUTTONUP, 0, 0, 2),
            _ => Next::Done(0),
        },
        K_SETCHECK => setcheck_step(cpu, t),
        _ => Next::Done(0),
    }
}

/// In a task: send the parent WM_COMMAND with `code`, going on at
/// `next`.
fn notify_from(cpu: &mut Cpu, t: &Task, code: u32, next: u16) -> Next {
    let hwnd = t.var(P_HWND);
    let parent = parent_of(cpu, hwnd);
    let id = user(cpu).window(hwnd).map_or(0, |w| w.id);
    window::send(parent, WM_COMMAND, id & 0xFFFF | code << 16, hwnd, next)
}

/// WM_LBUTTONUP, or WM_KEYUP of the space bar: the click.
fn up_step(cpu: &mut Cpu, t: &mut Task) -> Next {
    let hwnd = t.var(P_HWND);
    match t.step {
        0 => {
            let state = button(cpu, hwnd).state;
            if state & PRESSED == 0 {
                return Next::Done(0);
            }
            let state = state & NSTATES;
            set_state(cpu, hwnd, state);
            if state & HIGHLIGHTED == 0 {
                return Next::call(address_of("ReleaseCapture"), &[], 9);
            }
            window::send(hwnd, BM_SETSTATE, 0, 0, 1)
        }
        1 => Next::call(address_of("ReleaseCapture"), &[], 2),
        2 => {
            if t.var(V_B) == 0 && !in_client(cpu, hwnd, t.var(V_A)) {
                return Next::Done(0);
            }
            let state = button(cpu, hwnd).state;
            match style_of(cpu, hwnd) & 0x0F {
                BS_AUTOCHECKBOX => {
                    let check = (state & CHECKED == 0) as u32;
                    window::send(hwnd, BM_SETCHECK, check, 0, 3)
                }
                BS_AUTORADIOBUTTON => window::send(hwnd, BM_SETCHECK, 1, 0, 3),
                BS_AUTO3STATE => {
                    let check = if state & THREE_STATE != 0 {
                        0
                    } else {
                        (state & 3) + 1
                    };
                    window::send(hwnd, BM_SETCHECK, check, 0, 3)
                }
                _ => Next::call(0, &[], 3),
            }
        }
        3 => notify_from(cpu, t, BN_CLICKED, 9),
        _ => Next::Done(0),
    }
}

/// WM_SETTEXT: the old label is cleared with the WM_CTLCOLORSTATIC
/// brush, DefWindowProcA sets the text, and the button is painted (a
/// group box invalidated).
fn settext_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let hwnd = t.var(P_HWND);
    match t.step {
        0 => {
            let hdc = paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
            t.set(P_HDC, hdc);
            if hdc == 0 {
                return Next::call(0, &[], 2);
            }
            let parent = parent_of(cpu, hwnd);
            window::send(parent, WM_CTLCOLORSTATIC, hdc, hwnd, 1)
        }
        1 => {
            let hdc = t.var(P_HDC);
            let brush = if value != 0 {
                value
            } else {
                let parent = parent_of(cpu, hwnd);
                default_color(cpu, parent, hdc, WM_CTLCOLORSTATIC)
            };
            let client = client_rect(cpu, hwnd);
            if let Some((_, r)) = label_rect(cpu, hwnd, hdc, client) {
                let r = Rect::new(
                    r.left,
                    r.top,
                    r.right.min(client.right),
                    r.bottom.min(client.bottom),
                );
                paint::fill(cpu, hdc, r, brush);
            }
            paint::release_dc(cpu, &[hwnd, hdc]);
            t.set(P_HDC, 0);
            Next::call(0, &[], 2)
        }
        2 => {
            let args = [hwnd, WM_SETTEXT, t.var(V_A), t.var(V_B)];
            Next::call(address_of("DefWindowProcA"), &args, 3)
        }
        _ => {
            if style_of(cpu, hwnd) & 0x0F == BS_GROUPBOX {
                paint::invalidate_rect(cpu, &[hwnd, 0, 1]);
                return Next::Done(1);
            }
            repaint_then(cpu, t, ODA_DRAWENTIRE, 1, 0)
        }
    }
}

/// BM_SETCHECK: a radio button's WS_TABSTOP follows the check, and an
/// auto radio button checked unchecks the others of its group.
fn setcheck_step(cpu: &mut Cpu, t: &mut Task) -> Next {
    let hwnd = t.var(P_HWND);
    let style = style_of(cpu, hwnd);
    let kind = style & 0x0F;
    match t.step {
        0 => {
            let max = match kind {
                BS_CHECKBOX | BS_AUTOCHECKBOX | BS_RADIOBUTTON | BS_AUTORADIOBUTTON => CHECKED,
                BS_3STATE | BS_AUTO3STATE => THREE_STATE,
                _ => 0,
            };
            let check = t.var(V_A).min(max);
            t.set(V_A, check);
            if matches!(kind, BS_RADIOBUTTON | BS_AUTORADIOBUTTON)
                && let Some(w) = user(cpu).window_mut(hwnd)
            {
                if check != 0 {
                    w.style |= WS_TABSTOP;
                } else {
                    w.style &= !WS_TABSTOP;
                }
            }
            let state = button(cpu, hwnd).state;
            if state & 3 != check {
                set_state(cpu, hwnd, (state & !3) | check);
                return repaint_then(cpu, t, ODA_SELECT, 0, 1);
            }
            Next::call(0, &[], 1)
        }
        1 => {
            if kind != BS_AUTORADIOBUTTON || t.var(V_A) != CHECKED || style & WS_CHILD == 0 {
                return Next::Done(0);
            }
            let others = group_radios(cpu, hwnd);
            t.set(V_B, 0);
            for (i, h) in others.iter().enumerate() {
                t.set(V_LIST + i, *h);
            }
            t.vars.truncate(V_LIST + others.len());
            Next::call(0, &[], 2)
        }
        _ => {
            let i = t.var(V_B) as usize;
            if V_LIST + i >= t.vars.len() {
                return Next::Done(0);
            }
            t.set(V_B, i as u32 + 1);
            window::send(t.var(V_LIST + i), BM_SETCHECK, 0, 0, 2)
        }
    }
}

/// The other auto radio buttons of the group of `hwnd` among its
/// siblings, as GetNextDlgGroupItem goes through them: from the last
/// WS_GROUP at or before it to the next, the visible and enabled ones.
fn group_radios(cpu: &mut Cpu, hwnd: u32) -> Vec<u32> {
    let state = user(cpu);
    let Some(parent) = state.window(hwnd).map(|w| w.parent) else {
        return Vec::new();
    };
    let Some(siblings) = state.window(parent).map(|p| p.children.clone()) else {
        return Vec::new();
    };
    let Some(at) = siblings.iter().position(|&h| h == hwnd) else {
        return Vec::new();
    };
    let style = |h: u32| state.window(h).map_or(0, |w| w.style);
    let first = (0..=at)
        .rev()
        .find(|&i| style(siblings[i]) & WS_GROUP != 0)
        .unwrap_or(0);
    let end = (first + 1..siblings.len())
        .find(|&i| style(siblings[i]) & WS_GROUP != 0)
        .unwrap_or(siblings.len());
    siblings[first..end]
        .iter()
        .copied()
        .filter(|&h| {
            let s = style(h);
            h != hwnd
                && s & 0x0F == BS_AUTORADIOBUTTON
                && s & window::WS_VISIBLE != 0
                && s & WS_DISABLED == 0
        })
        .collect()
}

/// An owner-drawn button's WM_ERASEBKGND: the client area in the
/// WM_CTLCOLORBTN brush.
fn erase_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let (hwnd, hdc) = (t.var(P_HWND), t.var(P_HDC));
    match t.step {
        0 => {
            let parent = parent_of(cpu, hwnd);
            window::send(parent, WM_CTLCOLORBTN, hdc, hwnd, 1)
        }
        _ => {
            let brush = if value != 0 {
                value
            } else {
                let parent = parent_of(cpu, hwnd);
                default_color(cpu, parent, hdc, WM_CTLCOLORBTN)
            };
            let r = client_rect(cpu, hwnd);
            paint::fill(cpu, hdc, r, brush);
            Next::Done(1)
        }
    }
}

/// The painting steps, `STEP_PAINT` on.
fn paint_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let hwnd = t.var(P_HWND);
    let Some(w) = user(cpu).window(hwnd) else {
        return Next::Done(t.var(P_RESULT));
    };
    let style = w.style;
    let b = w.control.button.clone();
    let kind = style & 0x0F;
    let push = kind <= BS_DEFPUSHBUTTON || style & BS_PUSHLIKE != 0;
    match t.step - STEP_PAINT {
        0 => open(cpu, t, STEP_PAINT + 1),
        1 => {
            let hdc = prepare(cpu, t, value, b.font);
            if hdc == 0 {
                return close(cpu, t, STEP_PAINT + 4);
            }
            if t.var(P_HOW) != HOW_DC {
                let old = gdi32::dc::set_bk_mode(cpu, &[hdc, OPAQUE]).unwrap_or(0);
                t.set(P_MODE, old);
            }
            let msg = match kind {
                BS_GROUPBOX => WM_CTLCOLORSTATIC,
                _ if push || kind == BS_USERBUTTON || kind == BS_OWNERDRAW => WM_CTLCOLORBTN,
                _ => WM_CTLCOLORSTATIC,
            };
            let parent = parent_of(cpu, hwnd);
            window::send(parent, msg, hdc, hwnd, STEP_PAINT + 2)
        }
        2 => {
            let hdc = t.var(P_HDC);
            let action = t.var(P_ACTION);
            let brush = if value != 0 {
                value
            } else {
                let msg = if push {
                    WM_CTLCOLORBTN
                } else {
                    WM_CTLCOLORSTATIC
                };
                let parent = parent_of(cpu, hwnd);
                default_color(cpu, parent, hdc, msg)
            };
            match kind {
                BS_OWNERDRAW => {
                    let dis = t.scratch + DRAWITEM;
                    write_draw_item(cpu, hwnd, hdc, dis, action, b.state, style);
                    let parent = parent_of(cpu, hwnd);
                    let id = user(cpu).window(hwnd).map_or(0, |w| w.id);
                    return window::send(parent, WM_DRAWITEM, id, dis, STEP_PAINT + 3);
                }
                BS_USERBUTTON => {
                    let rc = client_rect(cpu, hwnd);
                    paint::fill(cpu, hdc, rc, brush);
                    if action == ODA_FOCUS || b.state & HASFOCUS != 0 {
                        focus_rect(cpu, hdc, rc);
                    }
                    let code = match action {
                        ODA_FOCUS if b.state & HASFOCUS != 0 => BN_SETFOCUS,
                        ODA_FOCUS => BN_KILLFOCUS,
                        ODA_SELECT if b.state & HIGHLIGHTED != 0 => BN_HILITE,
                        ODA_SELECT => BN_UNHILITE,
                        _ => BN_PAINT,
                    };
                    return notify_from(cpu, t, code, STEP_PAINT + 3);
                }
                BS_GROUPBOX => paint_group(cpu, hwnd, hdc, brush, style),
                _ if push => paint_push(cpu, hwnd, hdc, action, style, b.state),
                _ => paint_check(cpu, hwnd, hdc, action, brush, style, b.state),
            }
            close(cpu, t, STEP_PAINT + 4)
        }
        3 => close(cpu, t, STEP_PAINT + 4),
        _ => finish(t),
    }
}

/// Fill the DRAWITEMSTRUCT at `at` for an owner-drawn button.
fn write_draw_item(
    cpu: &mut Cpu,
    hwnd: u32,
    hdc: u32,
    at: u32,
    action: u32,
    state: u32,
    style: u32,
) {
    let id = user(cpu).window(hwnd).map_or(0, |w| w.id);
    let item_state = if state & HASFOCUS != 0 { ODS_FOCUS } else { 0 }
        | if state & HIGHLIGHTED != 0 {
            ODS_SELECTED
        } else {
            0
        }
        | if style & WS_DISABLED != 0 {
            ODS_DISABLED
        } else {
            0
        };
    let r = client_rect(cpu, hwnd);
    let words = [
        ODT_BUTTON,
        id,
        0,
        action,
        item_state,
        hwnd,
        hdc,
        r.left as u32,
        r.top as u32,
        r.right as u32,
        r.bottom as u32,
        0,
    ];
    crate::win32::user32::write_words(cpu, at, &words);
}

/// The DrawText flags of the label for the style (BUTTON_BStoDT).
fn text_flags(style: u32, ex: u32) -> u32 {
    let mut style = style;
    let mut flags = DT_NOCLIP;
    if style & BS_PUSHLIKE != 0 {
        style &= !0x0F;
    }
    if style & BS_MULTILINE == 0 {
        flags |= DT_SINGLELINE;
    } else {
        flags |= DT_WORDBREAK;
    }
    match style & BS_CENTER {
        BS_LEFT => {}
        BS_RIGHT => flags |= DT_RIGHT,
        BS_CENTER => flags |= DT_CENTER,
        _ => {
            if style & 0x0F <= BS_DEFPUSHBUTTON {
                flags |= DT_CENTER;
            }
        }
    }
    if ex & WS_EX_RIGHT != 0 {
        flags = DT_RIGHT | (flags & !DT_CENTER);
    }
    if style & 0x0F != BS_GROUPBOX {
        match style & BS_VCENTER {
            BS_TOP => {}
            BS_BOTTOM => flags |= DT_BOTTOM,
            _ => flags |= DT_VCENTER,
        }
    } else {
        flags |= DT_SINGLELINE;
    }
    flags
}

/// The label's rectangle in `rc` and its DrawText flags
/// (BUTTON_CalcLabelRect), or None when there is no label.
fn label_rect(cpu: &mut Cpu, hwnd: u32, hdc: u32, rc: Rect) -> Option<(u32, Rect)> {
    let w = user(cpu).window(hwnd)?;
    let (style, ex) = (w.style, w.ex_style);
    let label = w.text.clone();
    let image = w.control.button.image;
    let flags = text_flags(style, ex);
    let mut r = rc;
    match style & (BS_ICON | BS_BITMAP) {
        0 => {
            if label.is_empty() {
                return None;
            }
            text::draw_text_bytes(cpu, hdc, &label, &mut r, flags | DT_CALCRECT);
        }
        BS_ICON => {
            let icon = resource::icon(cpu, image)?;
            r.right = r.left + icon.width;
            r.bottom = r.top + icon.height;
        }
        BS_BITMAP => {
            let (w, h) = gdi32::gdi(cpu).bitmap(image).map(|b| (b.width, b.height))?;
            r.right = r.left + w;
            r.bottom = r.top + h;
        }
        _ => return None,
    }
    match flags & (DT_CENTER | DT_RIGHT) {
        DT_CENTER => {
            let n = r.width();
            r.left = rc.left + (rc.width() - n) / 2;
            r.right = r.left + n;
        }
        DT_RIGHT => {
            let n = r.width();
            r.right = rc.right - 1;
            r.left = r.right - n;
        }
        _ => {
            r.left += 1;
            r.right += 1;
        }
    }
    match flags & (DT_VCENTER | DT_BOTTOM) {
        DT_VCENTER => {
            let n = r.height();
            r.top = rc.top + (rc.height() - n) / 2;
            r.bottom = r.top + n;
        }
        DT_BOTTOM => {
            let n = r.height();
            r.bottom = rc.bottom - 1;
            r.top = r.bottom - n;
        }
        _ => {
            r.top += 1;
            r.bottom += 1;
        }
    }
    Some((flags, r))
}

/// Draw the label in `r` (BUTTON_DrawLabel): a disabled button's text
/// embossed, as DrawState's DSS_DISABLED does.
fn draw_label(cpu: &mut Cpu, hwnd: u32, hdc: u32, flags: u32, r: Rect) {
    let Some(w) = user(cpu).window(hwnd) else {
        return;
    };
    let (style, label, b) = (w.style, w.text.clone(), w.control.button.clone());
    let enabled = style & WS_DISABLED == 0;
    match style & (BS_ICON | BS_BITMAP) {
        0 => {
            let gray = style & BS_PUSHLIKE != 0 && b.state & THREE_STATE != 0;
            if enabled {
                let old = if gray {
                    let c = user(cpu).color(COLOR_GRAYTEXT);
                    gdi32::dc::set_text_color(cpu, &[hdc, c])
                } else {
                    None
                };
                let mut r = r;
                text::draw_text_bytes(cpu, hdc, &label, &mut r, flags);
                if let Some(old) = old {
                    gdi32::dc::set_text_color(cpu, &[hdc, old]);
                }
                return;
            }
            let mode = gdi32::dc::set_bk_mode(cpu, &[hdc, TRANSPARENT]).unwrap_or(0);
            let light = user(cpu).color(COLOR_BTNHIGHLIGHT);
            let old = gdi32::dc::set_text_color(cpu, &[hdc, light]).unwrap_or(0);
            let mut moved = r.offset(1, 1);
            text::draw_text_bytes(cpu, hdc, &label, &mut moved, flags);
            let dark = user(cpu).color(COLOR_BTNSHADOW);
            gdi32::dc::set_text_color(cpu, &[hdc, dark]);
            let mut r = r;
            text::draw_text_bytes(cpu, hdc, &label, &mut r, flags);
            gdi32::dc::set_text_color(cpu, &[hdc, old]);
            if mode != 0 {
                gdi32::dc::set_bk_mode(cpu, &[hdc, mode]);
            }
        }
        BS_ICON => {
            if let Some(icon) = resource::icon(cpu, b.image) {
                let size = (icon.width, icon.height);
                paint::draw_icon_at(cpu, hdc, (r.left, r.top), b.image, size, DI_NORMAL);
            }
        }
        BS_BITMAP => {
            text::draw_bitmap(cpu, hdc, b.image, (r.left, r.top), (0, 0));
        }
        _ => {}
    }
}

/// A push button, or a check box or radio button with BS_PUSHLIKE
/// (PB_Paint).
fn paint_push(cpu: &mut Cpu, hwnd: u32, hdc: u32, action: u32, style: u32, state: u32) {
    let mut rc = client_rect(cpu, hwnd);
    let pushed = state & HIGHLIGHTED != 0;
    let default = style & 0x0F == BS_DEFPUSHBUTTON;
    let mode = gdi32::dc::set_bk_mode(cpu, &[hdc, TRANSPARENT]).unwrap_or(0);
    if default {
        if action != ODA_FOCUS {
            // Rectangle with the window frame's pen and the face's
            // brush.
            paint::fill_color(cpu, hdc, rc, COLOR_WINDOWFRAME);
            let inner = Rect::new(rc.left + 1, rc.top + 1, rc.right - 1, rc.bottom - 1);
            paint::fill_color(cpu, hdc, inner, COLOR_BTNFACE);
        }
        rc = Rect::new(rc.left + 1, rc.top + 1, rc.right - 1, rc.bottom - 1);
    }
    let mut focus = true;
    if action != ODA_FOCUS {
        let mut flags = DFCS_BUTTONPUSH;
        if style & BS_FLAT != 0 {
            flags |= DFCS_MONO;
        } else if pushed {
            flags |= if default { DFCS_FLAT } else { DFCS_PUSHED };
        }
        if state & (CHECKED | THREE_STATE) != 0 {
            flags |= DFCS_CHECKED;
        }
        paint::frame_control(cpu, hdc, rc, DFC_BUTTON, flags);
        match label_rect(cpu, hwnd, hdc, rc) {
            Some((flags, r)) => {
                let r = if pushed { r.offset(1, 1) } else { r };
                let text = user(cpu).color(COLOR_BTNTEXT);
                let old = gdi32::dc::set_text_color(cpu, &[hdc, text]).unwrap_or(0);
                draw_label(cpu, hwnd, hdc, flags, r);
                gdi32::dc::set_text_color(cpu, &[hdc, old]);
            }
            None => focus = false,
        }
    }
    if focus && (action == ODA_FOCUS || state & HASFOCUS != 0) {
        let r = Rect::new(rc.left + 2, rc.top + 2, rc.right - 2, rc.bottom - 2);
        focus_rect(cpu, hdc, r);
    }
    if mode != 0 {
        gdi32::dc::set_bk_mode(cpu, &[hdc, mode]);
    }
}

/// A check box or a radio button (CB_Paint).
fn paint_check(
    cpu: &mut Cpu,
    hwnd: u32,
    hdc: u32,
    action: u32,
    brush: u32,
    style: u32,
    state: u32,
) {
    let client = client_rect(cpu, hwnd);
    let (mut rbox, mut rtext) = (client, client);
    if style & BS_LEFTTEXT != 0 {
        rtext.right -= CHECK_SIZE + 4;
        rbox.left = rbox.right - CHECK_SIZE;
    } else {
        rtext.left += CHECK_SIZE + 4;
        rbox.right = CHECK_SIZE;
    }
    if action == ODA_SELECT {
        paint::fill(cpu, hdc, rbox, brush);
    }
    if action == ODA_DRAWENTIRE {
        paint::fill(cpu, hdc, client, brush);
    }
    let area = rtext;
    let label = label_rect(cpu, hwnd, hdc, rtext);
    if let Some((_, r)) = label {
        rtext = r;
        rbox.top = r.top;
        rbox.bottom = r.bottom;
    }
    if action == ODA_DRAWENTIRE || action == ODA_SELECT {
        let kind = style & 0x0F;
        let mut flags = if kind == BS_RADIOBUTTON || kind == BS_AUTORADIOBUTTON {
            DFCS_BUTTONRADIO
        } else if state & THREE_STATE != 0 {
            DFCS_BUTTON3STATE
        } else {
            DFCS_BUTTONCHECK
        };
        if state & (CHECKED | THREE_STATE) != 0 {
            flags |= DFCS_CHECKED;
        }
        if state & HIGHLIGHTED != 0 {
            flags |= DFCS_PUSHED;
        }
        if style & WS_DISABLED != 0 {
            flags |= DFCS_INACTIVE;
        }
        let delta = rbox.height() - CHECK_SIZE;
        if style & BS_VCENTER == BS_TOP {
            if delta > 0 {
                rbox.bottom = rbox.top + CHECK_SIZE;
            } else {
                rbox.top -= -delta / 2 + 1;
                rbox.bottom = rbox.top + CHECK_SIZE;
            }
        } else if style & BS_VCENTER == BS_BOTTOM {
            if delta > 0 {
                rbox.top = rbox.bottom - CHECK_SIZE;
            } else {
                rbox.bottom += -delta / 2 + 1;
                rbox.top = rbox.bottom - CHECK_SIZE;
            }
        } else if delta > 0 {
            rbox.bottom -= delta / 2 + 1;
            rbox.top = rbox.bottom - CHECK_SIZE;
        } else if delta < 0 {
            rbox.top -= -delta / 2 + 1;
            rbox.bottom = rbox.top + CHECK_SIZE;
        }
        paint::frame_control(cpu, hdc, rbox, DFC_BUTTON, flags);
    }
    let Some((flags, _)) = label else {
        return;
    };
    if action == ODA_DRAWENTIRE {
        draw_label(cpu, hwnd, hdc, flags, rtext);
    }
    if action == ODA_FOCUS || state & HASFOCUS != 0 {
        let r = Rect::new(rtext.left - 1, rtext.top, rtext.right + 1, rtext.bottom);
        focus_rect(cpu, hdc, r.intersect(&area));
    }
}

/// A group box (GB_Paint): an etched frame, its top through the middle
/// of the label.
fn paint_group(cpu: &mut Cpu, hwnd: u32, hdc: u32, brush: u32, style: u32) {
    let rc = client_rect(cpu, hwnd);
    let height = gdi32::font::metrics(cpu, hdc).map_or(16, |m| m.height);
    let mut frame = rc;
    frame.top += height / 2 - 1;
    let flat = if style & BS_FLAT != 0 { BF_FLAT } else { 0 };
    paint::edge(cpu, hdc, frame, EDGE_ETCHED, BF_RECT | flat);
    let inner = Rect::new(rc.left + 7, rc.top - 1, rc.right - 7, rc.bottom + 1);
    let Some((flags, r)) = label_rect(cpu, hwnd, hdc, inner) else {
        return;
    };
    let back = Rect::new(r.left - 1, r.top, r.right + 1, r.bottom + 1);
    paint::fill(cpu, hdc, back, brush);
    draw_label(cpu, hwnd, hdc, flags, r);
}

crate::state_fields!(Button { state, font, image });

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_buttons_center_their_text() {
        let f = text_flags(BS_PUSHBUTTON, 0);
        assert_eq!(f & (DT_CENTER | DT_RIGHT), DT_CENTER);
        assert_ne!(f & DT_VCENTER, 0);
        assert_ne!(f & DT_SINGLELINE, 0);
    }

    #[test]
    fn check_boxes_put_their_text_left() {
        let f = text_flags(BS_AUTOCHECKBOX, 0);
        assert_eq!(f & (DT_CENTER | DT_RIGHT), 0);
        let f = text_flags(BS_AUTOCHECKBOX | BS_RIGHT | BS_MULTILINE, 0);
        assert_eq!(f & (DT_CENTER | DT_RIGHT), DT_RIGHT);
        assert_ne!(f & DT_WORDBREAK, 0);
    }

    #[test]
    fn group_boxes_have_one_line_at_the_top() {
        let f = text_flags(BS_GROUPBOX | BS_VCENTER, 0);
        assert_eq!(f & (DT_VCENTER | DT_BOTTOM), 0);
        assert_ne!(f & DT_SINGLELINE, 0);
    }

    #[test]
    fn push_box_does_not_paint() {
        assert!(!paints(BS_PUSHBOX));
        assert!(paints(BS_OWNERDRAW));
        assert!(paints(BS_AUTORADIOBUTTON));
    }
}
