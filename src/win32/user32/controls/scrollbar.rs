//! Scroll bars: a window's own (SB_HORZ, SB_VERT), which DefWindowProcA
//! draws in the frame (`draw_nc`) and tracks (`track`), and the
//! ScrollBar class (SB_CTL); and the functions that set and get their
//! state. This follows Wine's scroll.c with Windows 95's look.
//!
//! Tracking a click is a loop of GetMessageA, as on Windows: the
//! scroll bar's task gets the messages itself until the button goes
//! up, sending WM_HSCROLL or WM_VSCROLL and repeating with a system
//! timer while the button is held on an arrow or the shaft.

use super::button::{
    self, HOW_DC, K_NOTIFY, P_HDC, P_HOW, P_HWND, P_OWN, P_RESULT, PAINT_SCRATCH, paint_vars,
};
use super::{SCROLLBAR, kind, start};
use crate::cpu::Cpu;
use crate::win32::gdi32::{self, Rect};
use crate::win32::task::{Next, Task};
use crate::win32::user32::message::{
    self, WM_CREATE, WM_CTLCOLORSCROLLBAR, WM_ENABLE, WM_ERASEBKGND, WM_GETDLGCODE, WM_HSCROLL,
    WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_PAINT, WM_PRINTCLIENT, WM_SETFOCUS, WM_SYSTIMER, WM_VSCROLL,
};
use crate::win32::user32::paint::{
    self, BF_MIDDLE, BF_RECT, DFC_SCROLL, DFCS_FLAT, DFCS_INACTIVE, DFCS_PUSHED, DFCS_SCROLLDOWN,
    DFCS_SCROLLLEFT, DFCS_SCROLLRIGHT, DFCS_SCROLLSIZEGRIP, DFCS_SCROLLUP, EDGE_RAISED,
};
use crate::win32::user32::sysparams::{self, COLOR_SCROLLBAR};
use crate::win32::user32::window::{
    self, SW_HIDE, SW_SHOW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    WS_DISABLED, WS_HSCROLL, WS_TABSTOP, WS_VSCROLL,
};
use crate::win32::user32::{address_of, class, defwnd, input, read_words, user, write_words};

/// The state of a scroll bar: its range, page and position, which of
/// its arrows are disabled (ESB_*), and, while a click on it is
/// tracked, the part drawn pressed and where the thumb is dragged.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollInfo {
    pub min: i32,
    pub max: i32,
    pub pos: i32,
    pub page: i32,
    pub flags: u32,
    /// Whether the window has the state yet: Windows makes a window's
    /// the first time the bars are used, with the range 0 to 100.
    pub made: bool,
    /// The part drawn pressed (`HIT_*`).
    pub pressed: u32,
    /// Whether the thumb is dragged, to where (pixels from the top or
    /// the left) and the position that is.
    pub dragging: bool,
    pub drag: i32,
    pub track: i32,
}

pub const SB_HORZ: u32 = 0;
pub const SB_VERT: u32 = 1;
pub const SB_CTL: u32 = 2;
pub const SB_BOTH: u32 = 3;

pub const ESB_ENABLE_BOTH: u32 = 0;
pub const ESB_DISABLE_LTUP: u32 = 1;
pub const ESB_DISABLE_RTDN: u32 = 2;
pub const ESB_DISABLE_BOTH: u32 = 3;

const SIF_RANGE: u32 = 0x01;
const SIF_PAGE: u32 = 0x02;
const SIF_POS: u32 = 0x04;
const SIF_DISABLENOSCROLL: u32 = 0x08;
const SIF_TRACKPOS: u32 = 0x10;
const SIF_ALL: u32 = SIF_RANGE | SIF_PAGE | SIF_POS | SIF_TRACKPOS;

const SB_LINEUP: u32 = 0;
const SB_LINEDOWN: u32 = 1;
const SB_PAGEUP: u32 = 2;
const SB_PAGEDOWN: u32 = 3;
const SB_THUMBPOSITION: u32 = 4;
const SB_THUMBTRACK: u32 = 5;
const SB_TOP: u32 = 6;
const SB_BOTTOM: u32 = 7;
const SB_ENDSCROLL: u32 = 8;

pub const SBM_SETPOS: u32 = 0x00E0;
pub const SBM_GETPOS: u32 = 0x00E1;
pub const SBM_SETRANGE: u32 = 0x00E2;
pub const SBM_GETRANGE: u32 = 0x00E3;
pub const SBM_ENABLE_ARROWS: u32 = 0x00E4;
pub const SBM_SETRANGEREDRAW: u32 = 0x00E6;
pub const SBM_SETSCROLLINFO: u32 = 0x00E9;
pub const SBM_GETSCROLLINFO: u32 = 0x00EA;

const SBS_VERT: u32 = 0x01;
const SBS_TOPALIGN: u32 = 0x02;
const SBS_BOTTOMALIGN: u32 = 0x04;
const SBS_SIZEBOX: u32 = 0x08;
const SBS_SIZEGRIP: u32 = 0x10;

const WS_EX_LEFTSCROLLBAR: u32 = 0x4000;

const DLGC_WANTARROWS: u32 = 0x0001;
const MSGF_SCROLLBAR: u32 = 5;

const VK_PRIOR: u32 = 0x21;
const VK_NEXT: u32 = 0x22;
const VK_END: u32 = 0x23;
const VK_HOME: u32 = 0x24;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;

const SM_CXVSCROLL: u32 = 2;
const SM_CYHSCROLL: u32 = 3;

/// The parts of a scroll bar a point is on.
pub const HIT_NOWHERE: u32 = 0;
pub const HIT_TOP_ARROW: u32 = 1;
pub const HIT_TOP_RECT: u32 = 2;
pub const HIT_THUMB: u32 = 3;
pub const HIT_BOTTOM_RECT: u32 = 4;
pub const HIT_BOTTOM_ARROW: u32 = 5;

/// The smallest the shaft between the arrows is, and the thumb.
const MIN_RECT: i32 = 4;
const MIN_THUMB: i32 = 6;

/// The timer of a held click, and its delays in milliseconds: the
/// first and the ones after.
const SCROLL_TIMER: u32 = 0;
const FIRST_DELAY: u32 = 200;
const REPEAT_DELAY: u32 = 50;

const PATCOPY: u32 = 0x00F0_0021;
/// The inverse of the pattern, for the part of the shaft held down.
const PATINVERSE: u32 = 0x000F_0001;

fn metric(index: u32) -> i32 {
    sysparams::metric(index)
}

/// MulDiv: `a * b / c` rounded, or -1 when `c` is 0.
fn mul_div(a: i32, b: i32, c: i32) -> i32 {
    if c == 0 {
        return -1;
    }
    let (n, c) = (a as i64 * b as i64, c as i64);
    let q = if (n < 0) != (c < 0) {
        (n - c / 2) / c
    } else {
        (n + c / 2) / c
    };
    q as i32
}

/// Whether `hwnd` is a scroll bar control, whose state is its first
/// `scroll`.
fn is_control(cpu: &mut Cpu, hwnd: u32) -> bool {
    let proc = address_of("ScrollBarWndProc");
    class::of(user(cpu), hwnd).is_some_and(|c| c.proc == proc)
}

/// The state of the bar `bar` of `hwnd`, made with Windows' defaults
/// if `make` is set and the window has none yet.
fn info(cpu: &mut Cpu, hwnd: u32, bar: u32, make: bool) -> Option<ScrollInfo> {
    if hwnd == crate::win32::user32::DESKTOP {
        return None;
    }
    let control = bar == SB_CTL && is_control(cpu, hwnd);
    let w = user(cpu).window_mut(hwnd)?;
    match bar {
        SB_CTL if control => Some(w.scroll[0]),
        SB_HORZ | SB_VERT => {
            if !w.scroll[bar as usize].made {
                if !make {
                    return None;
                }
                let fresh = ScrollInfo {
                    max: 100,
                    made: true,
                    ..ScrollInfo::default()
                };
                w.scroll = [fresh; 2];
            }
            Some(w.scroll[bar as usize])
        }
        _ => None,
    }
}

fn store(cpu: &mut Cpu, hwnd: u32, bar: u32, si: ScrollInfo) {
    if let Some(w) = user(cpu).window_mut(hwnd) {
        w.scroll[if bar == SB_VERT { 1 } else { 0 }] = si;
    }
}

/// Where the bar `bar` of `hwnd` is, in the coordinates it is drawn in
/// (the client area's for SB_CTL, the window's for the others), and
/// whether it is vertical (SCROLL_GetScrollBarRect's first half).
pub fn bar_rect(cpu: &mut Cpu, hwnd: u32, bar: u32) -> Option<(Rect, bool)> {
    let w = user(cpu).window(hwnd)?;
    let (style, ex) = (w.style, w.ex_style);
    let c = w.client.offset(-w.rect.left, -w.rect.top);
    match bar {
        SB_HORZ => {
            let mut r = Rect::new(c.left, c.bottom, c.right, c.bottom + metric(SM_CYHSCROLL));
            if style & WS_VSCROLL != 0 {
                r.right += 1;
            }
            Some((r, false))
        }
        SB_VERT => {
            let left = if ex & WS_EX_LEFTSCROLLBAR != 0 {
                c.left - metric(SM_CXVSCROLL)
            } else {
                c.right
            };
            let mut r = Rect::new(left, c.top, left + metric(SM_CXVSCROLL), c.bottom);
            if style & WS_HSCROLL != 0 {
                r.bottom += 1;
            }
            Some((r, true))
        }
        SB_CTL => {
            let r = Rect::new(0, 0, w.client.width(), w.client.height());
            Some((r, style & SBS_VERT != 0))
        }
        _ => None,
    }
}

/// The size of the arrows, and the thumb's size and place from the top
/// or the left of `r` (SCROLL_GetScrollBarRect's second half). A
/// thumb at 0 isn't there.
fn geometry(r: Rect, vertical: bool, si: &ScrollInfo) -> (i32, i32, i32) {
    let mut pixels = if vertical { r.height() } else { r.width() };
    let arrow_size = metric(SM_CXVSCROLL);
    if pixels <= 2 * arrow_size + MIN_RECT {
        let arrow = if pixels > MIN_RECT {
            (pixels - MIN_RECT) / 2
        } else {
            0
        };
        return (arrow, 0, 0);
    }
    pixels -= 2 * arrow_size;
    let size = if si.page != 0 {
        mul_div(pixels, si.page, si.max - si.min + 1).max(MIN_THUMB)
    } else {
        arrow_size
    };
    pixels -= size;
    if pixels < 0 || si.flags & ESB_DISABLE_BOTH == ESB_DISABLE_BOTH {
        return (arrow_size, 0, 0);
    }
    let max = si.max - (si.page - 1).max(0);
    let pos = if si.min >= max {
        arrow_size
    } else {
        arrow_size + mul_div(pixels, si.pos - si.min, max - si.min)
    };
    (arrow_size, size, pos)
}

/// The position for the thumb at `pos` pixels from the top or the left
/// of `r` (SCROLL_GetThumbVal).
fn thumb_value(si: &ScrollInfo, r: Rect, vertical: bool, pos: i32) -> i32 {
    let arrow = metric(SM_CXVSCROLL);
    let mut pixels = if vertical { r.height() } else { r.width() };
    pixels -= 2 * arrow;
    if pixels <= 0 {
        return si.min;
    }
    let size = if si.page != 0 {
        mul_div(pixels, si.page, si.max - si.min + 1).max(MIN_THUMB)
    } else {
        arrow
    };
    pixels -= size;
    if pixels <= 0 {
        return si.min;
    }
    let pos = (pos - arrow).max(0).min(pixels);
    let range = if si.page == 0 {
        si.max - si.min
    } else {
        si.max - si.min - si.page + 1
    };
    si.min + mul_div(pos, range, pixels)
}

/// Whether `p` is near enough `r` to drag the thumb on
/// (SCROLL_PtInRectEx).
fn near(r: Rect, (x, y): (i32, i32), vertical: bool) -> bool {
    let r = if vertical {
        let w = r.width();
        Rect::new(
            r.left - w * 8,
            r.top - w * 2,
            r.right + w * 8,
            r.bottom + w * 2,
        )
    } else {
        let h = r.height();
        Rect::new(
            r.left - h * 2,
            r.top - h * 8,
            r.right + h * 2,
            r.bottom + h * 8,
        )
    };
    inside(r, (x, y))
}

fn inside(r: Rect, (x, y): (i32, i32)) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// The part of the bar at `p` (SCROLL_HitTest).
fn hit_test(r: Rect, vertical: bool, si: &ScrollInfo, p: (i32, i32), dragging: bool) -> u32 {
    if (dragging && !near(r, p, vertical)) || !inside(r, p) {
        return HIT_NOWHERE;
    }
    let (arrow, size, pos) = geometry(r, vertical, si);
    let (at, start, end) = if vertical {
        (p.1, r.top, r.bottom)
    } else {
        (p.0, r.left, r.right)
    };
    if at < start + arrow {
        return HIT_TOP_ARROW;
    }
    if at >= end - arrow {
        return HIT_BOTTOM_ARROW;
    }
    if pos == 0 {
        return HIT_TOP_RECT;
    }
    let at = at - start;
    if at < pos {
        return HIT_TOP_RECT;
    }
    if at >= pos + size {
        return HIT_BOTTOM_RECT;
    }
    HIT_THUMB
}

/// Fill `r` with `brush` by the raster operation `rop`.
fn pattern(cpu: &mut Cpu, hdc: u32, r: Rect, brush: u32, rop: u32) {
    if r.is_empty() {
        return;
    }
    let old = gdi32::dc::select_object(cpu, &[hdc, brush]).unwrap_or(0);
    let args = [
        hdc,
        r.left as u32,
        r.top as u32,
        r.width() as u32,
        r.height() as u32,
        rop,
    ];
    gdi32::draw::pat_blt(cpu, &args);
    if old != 0 {
        gdi32::dc::select_object(cpu, &[hdc, old]);
    }
}

/// Draw the scroll bar `si` in `r` of `hdc`: its arrows, its shaft in
/// `brush` and its thumb, where it is dragged to while it is
/// (SCROLL_DrawArrows and SCROLL_DrawInterior_9x).
fn draw_bar(cpu: &mut Cpu, hdc: u32, r: Rect, vertical: bool, si: &ScrollInfo, brush: u32) {
    if r.is_empty() {
        return;
    }
    let (arrow, size, mut pos) = geometry(r, vertical, si);
    if arrow != 0 {
        let (top, bottom) = if vertical {
            (
                Rect::new(r.left, r.top, r.right, r.top + arrow),
                Rect::new(r.left, r.bottom - arrow, r.right, r.bottom),
            )
        } else {
            (
                Rect::new(r.left, r.top, r.left + arrow, r.bottom),
                Rect::new(r.right - arrow, r.top, r.right, r.bottom),
            )
        };
        let pushed = |hit| {
            if si.pressed == hit {
                DFCS_PUSHED | DFCS_FLAT
            } else {
                0
            }
        };
        let (up, down) = if vertical {
            (DFCS_SCROLLUP, DFCS_SCROLLDOWN)
        } else {
            (DFCS_SCROLLLEFT, DFCS_SCROLLRIGHT)
        };
        let inactive = |flag| {
            if si.flags & flag != 0 {
                DFCS_INACTIVE
            } else {
                0
            }
        };
        let state = up | pushed(HIT_TOP_ARROW) | inactive(ESB_DISABLE_LTUP);
        paint::frame_control(cpu, hdc, top, DFC_SCROLL, state);
        let state = down | pushed(HIT_BOTTOM_ARROW) | inactive(ESB_DISABLE_RTDN);
        paint::frame_control(cpu, hdc, bottom, DFC_SCROLL, state);
    }
    let shaft = if vertical {
        Rect::new(r.left, r.top + arrow, r.right, r.bottom - arrow)
    } else {
        Rect::new(r.left + arrow, r.top, r.right - arrow, r.bottom)
    };
    if pos == 0 {
        pattern(cpu, hdc, shaft, brush, PATCOPY);
        return;
    }
    if si.dragging {
        let length = if vertical { r.height() } else { r.width() };
        pos = si.drag.min(length - arrow - size).max(arrow);
    }
    let rop = |hit| {
        if si.pressed == hit {
            PATINVERSE
        } else {
            PATCOPY
        }
    };
    let (before, thumb, after) = if vertical {
        let t = r.top + pos;
        (
            Rect::new(shaft.left, shaft.top, shaft.right, t),
            Rect::new(shaft.left, t, shaft.right, t + size),
            Rect::new(shaft.left, t + size, shaft.right, shaft.bottom),
        )
    } else {
        let l = r.left + pos;
        (
            Rect::new(shaft.left, shaft.top, l, shaft.bottom),
            Rect::new(l, shaft.top, l + size, shaft.bottom),
            Rect::new(l + size, shaft.top, shaft.right, shaft.bottom),
        )
    };
    pattern(cpu, hdc, before, brush, rop(HIT_TOP_RECT));
    pattern(cpu, hdc, after, brush, rop(HIT_BOTTOM_RECT));
    paint::edge(cpu, hdc, thumb, EDGE_RAISED, BF_RECT | BF_MIDDLE);
}

/// Draw the window's scroll bar `bar` (SB_HORZ or SB_VERT) in `rect` of
/// `hdc`, a DC of the whole window: for DefWindowProcA's WM_NCPAINT.
/// The bar's state is made if the window has none yet.
pub fn draw_nc(cpu: &mut Cpu, hwnd: u32, hdc: u32, bar: u32, rect: Rect) {
    let Some(si) = info(cpu, hwnd, bar, true) else {
        return;
    };
    let style = user(cpu).window(hwnd).map_or(0, |w| w.style);
    let shown = match bar {
        SB_HORZ => style & WS_HSCROLL != 0,
        SB_VERT => style & WS_VSCROLL != 0,
        _ => false,
    };
    if !shown || rect.is_empty() {
        return;
    }
    let brush = button::default_color(cpu, hdc, WM_CTLCOLORSCROLLBAR);
    draw_bar(cpu, hdc, rect, bar == SB_VERT, &si, brush);
}

/// Draw a window's own scroll bar again at once.
fn redraw_nc(cpu: &mut Cpu, hwnd: u32, bar: u32) {
    if !user(cpu).is_visible(hwnd) {
        return;
    }
    let Some((rect, _)) = bar_rect(cpu, hwnd, bar) else {
        return;
    };
    let Some(hdc) = paint::get_window_dc(cpu, &[hwnd]).filter(|&h| h != 0) else {
        return;
    };
    draw_nc(cpu, hwnd, hdc, bar, rect);
    paint::release_dc(cpu, &[hwnd, hdc]);
}

/// Draw the bar `bar` again (SCROLL_RefreshScrollBar) and return
/// `result`: a control asks its parent for the brush, in a task.
fn refresh(cpu: &mut Cpu, hwnd: u32, bar: u32, result: u32) -> Option<u32> {
    if bar != SB_CTL {
        redraw_nc(cpu, hwnd, bar);
        return Some(result);
    }
    if !user(cpu).is_visible(hwnd) {
        return Some(result);
    }
    let mut vars = paint_vars(hwnd, 0, false, 0);
    vars[P_RESULT - 1] = result;
    start(cpu, SCROLLBAR, K_PAINT, vars, PAINT_SCRATCH)
}

// Setting the state.

/// What is to be done after the state changed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum After {
    Nothing,
    /// Hide the bar, by its window style.
    Hide,
    /// Show the bar; draw it if that doesn't change the frame and the
    /// field says so.
    Show(bool),
    Redraw,
}

/// SCROLL_SetScrollInfo's change of the state: the position after it,
/// and what is to be done.
#[allow(clippy::too_many_arguments)]
fn apply(
    si: &mut ScrollInfo,
    bar: u32,
    mask: u32,
    min: i32,
    max: i32,
    page: u32,
    pos: i32,
    redraw: bool,
) -> After {
    let mut refresh = false;
    let mut arrows = false;
    let mut hide = false;
    let mut show = false;
    if mask & SIF_PAGE != 0 && si.page != page as i32 {
        si.page = page as i32;
        refresh = true;
    }
    if mask & SIF_POS != 0 && si.pos != pos {
        si.pos = pos;
        refresh = true;
    }
    if mask & SIF_RANGE != 0 {
        if min > max || (max.wrapping_sub(min) as u32) >= 0x8000_0000 {
            refresh = true;
            si.min = 0;
            si.max = 0;
        } else if si.min != min || si.max != max {
            refresh = true;
            si.min = min;
            si.max = max;
        }
    }
    if si.page < 0 {
        si.page = 0;
    } else if si.page > si.max - si.min + 1 {
        si.page = si.max - si.min + 1;
    }
    let top = si.max - (si.page - 1).max(0);
    if si.pos < si.min {
        si.pos = si.min;
    } else if si.pos > top {
        si.pos = top;
    }
    if mask & SIF_ALL != 0 && mask & (SIF_RANGE | SIF_PAGE | SIF_DISABLENOSCROLL) != 0 {
        let mut flags = si.flags;
        if si.min >= top {
            if mask & SIF_DISABLENOSCROLL != 0 {
                flags = ESB_DISABLE_BOTH;
            } else if bar != SB_CTL && refresh {
                hide = true;
            }
        } else if mask != SIF_PAGE {
            flags = ESB_ENABLE_BOTH;
            if bar != SB_CTL && refresh {
                show = true;
            }
        }
        if si.flags != flags {
            si.flags = flags;
            arrows = true;
        }
    }
    if hide {
        After::Hide
    } else if show {
        After::Show(redraw || arrows)
    } else if redraw || arrows {
        After::Redraw
    } else {
        After::Nothing
    }
}

/// Set the window style of the bar `bar` to `show` it
/// (SCROLL_ShowScrollBar for SB_HORZ, SB_VERT and SB_BOTH): whether
/// that changed it, so that the frame needs SWP_FRAMECHANGED.
fn set_shown(cpu: &mut Cpu, hwnd: u32, bar: u32, show: bool) -> bool {
    let bits = match bar {
        SB_HORZ => WS_HSCROLL,
        SB_VERT => WS_VSCROLL,
        SB_BOTH => WS_HSCROLL | WS_VSCROLL,
        _ => return false,
    };
    let Some(w) = user(cpu).window_mut(hwnd) else {
        return false;
    };
    let old = w.style;
    if show {
        w.style |= bits;
    } else {
        w.style &= !bits;
    }
    old != w.style
}

/// SWP_FRAMECHANGED for a window whose scroll bars came or went, in a
/// task that then returns `result`.
fn frame_changed(cpu: &mut Cpu, hwnd: u32, result: u32) -> Option<u32> {
    start(cpu, SCROLLBAR, K_FRAME, vec![hwnd, result], 0)
}

/// Set the state of the bar `bar` from the fields of a SCROLLINFO and
/// draw or show it as that calls for: the position, which the function
/// returns, unless `result` says otherwise.
#[allow(clippy::too_many_arguments)]
fn set(
    cpu: &mut Cpu,
    hwnd: u32,
    bar: u32,
    mask: u32,
    range: (i32, i32),
    page: u32,
    pos: i32,
    redraw: bool,
    result: Option<u32>,
) -> Option<u32> {
    let Some(mut si) = info(cpu, hwnd, bar, true) else {
        return Some(result.unwrap_or(0));
    };
    let after = apply(&mut si, bar, mask, range.0, range.1, page, pos, redraw);
    store(cpu, hwnd, bar, si);
    let result = result.unwrap_or(si.pos as u32);
    match after {
        After::Hide => {
            if set_shown(cpu, hwnd, bar, false) {
                return frame_changed(cpu, hwnd, result);
            }
            Some(result)
        }
        After::Show(draw) => {
            if set_shown(cpu, hwnd, bar, true) {
                return frame_changed(cpu, hwnd, result);
            }
            if draw {
                return refresh(cpu, hwnd, bar, result);
            }
            Some(result)
        }
        After::Redraw => refresh(cpu, hwnd, bar, result),
        After::Nothing => Some(result),
    }
}

/// A scroll bar's minimum and maximum.
type Range = (i32, i32);

/// A SCROLLINFO at `at`: its size, mask, range, page and position, if
/// it is valid (SCROLL_ScrollInfoValid).
fn read_info(cpu: &mut Cpu, at: u32) -> Option<(u32, u32, Range, u32, i32)> {
    if at == 0 {
        return None;
    }
    let w = read_words(cpu, at, 6);
    let (size, mask) = (w[0], w[1]);
    if mask & !(SIF_ALL | SIF_DISABLENOSCROLL) != 0 || (size != 28 && size != 24) {
        return None;
    }
    Some((size, mask, (w[2] as i32, w[3] as i32), w[4], w[5] as i32))
}

fn set_from(cpu: &mut Cpu, hwnd: u32, bar: u32, at: u32, redraw: bool) -> Option<u32> {
    let Some((_, mask, range, page, pos)) = read_info(cpu, at) else {
        return Some(0);
    };
    set(cpu, hwnd, bar, mask, range, page, pos, redraw, None)
}

/// Fill the SCROLLINFO at `at` (SCROLL_GetScrollInfo): whether it asked
/// for anything.
fn get_into(cpu: &mut Cpu, hwnd: u32, bar: u32, at: u32) -> bool {
    let Some((size, mask, ..)) = read_info(cpu, at) else {
        return false;
    };
    let Some(si) = info(cpu, hwnd, bar, false) else {
        return false;
    };
    if mask & SIF_RANGE != 0 {
        write_words(cpu, at + 8, &[si.min as u32, si.max as u32]);
    }
    if mask & SIF_PAGE != 0 {
        cpu.bus.guest_write_32(at + 16, si.page as u32);
    }
    if mask & SIF_POS != 0 {
        cpu.bus.guest_write_32(at + 20, si.pos as u32);
    }
    if mask & SIF_TRACKPOS != 0 && size == 28 {
        let track = if si.dragging { si.track } else { si.pos };
        cpu.bus.guest_write_32(at + 24, track as u32);
    }
    mask & SIF_ALL != 0
}

/// SetScrollInfo(hwnd, bar, info, redraw): the position. A control gets
/// SBM_SETSCROLLINFO.
pub fn set_scroll_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, bar, at, redraw) = (args[0], args[1], args[2], args[3]);
    if bar == SB_CTL {
        return message::send_message(cpu, &[hwnd, SBM_SETSCROLLINFO, redraw, at]);
    }
    set_from(cpu, hwnd, bar, at, redraw != 0)
}

/// GetScrollInfo(hwnd, bar, info). A control gets SBM_GETSCROLLINFO,
/// and the function succeeds whatever it does.
pub fn get_scroll_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, bar, at) = (args[0], args[1], args[2]);
    if bar == SB_CTL {
        let vars = vec![hwnd, SBM_GETSCROLLINFO, 0, at, 1];
        return start(cpu, SCROLLBAR, K_SEND, vars, 0);
    }
    Some(get_into(cpu, hwnd, bar, at) as u32)
}

/// SetScrollPos(hwnd, bar, pos, redraw): the position before. A
/// control gets SBM_SETPOS.
pub fn set_scroll_pos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, bar, pos, redraw) = (args[0], args[1], args[2], args[3]);
    if bar == SB_CTL {
        if info(cpu, hwnd, bar, false).is_none() {
            return Some(0);
        }
        return message::send_message(cpu, &[hwnd, SBM_SETPOS, pos, redraw]);
    }
    let Some(si) = info(cpu, hwnd, bar, false) else {
        return Some(0);
    };
    let old = si.pos as u32;
    let range = (0, 0);
    set(
        cpu,
        hwnd,
        bar,
        SIF_POS,
        range,
        0,
        pos as i32,
        redraw != 0,
        Some(old),
    )
}

/// GetScrollPos(hwnd, bar). A control gets SBM_GETPOS.
pub fn get_scroll_pos(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, bar) = (args[0], args[1]);
    if bar == SB_CTL {
        return message::send_message(cpu, &[hwnd, SBM_GETPOS, 0, 0]);
    }
    Some(info(cpu, hwnd, bar, false).map_or(0, |si| si.pos as u32))
}

/// SetScrollRange(hwnd, bar, min, max, redraw): TRUE. A control gets
/// SBM_SETRANGE, or SBM_SETRANGEREDRAW to redraw.
pub fn set_scroll_range(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, bar, min, max, redraw) = (args[0], args[1], args[2], args[3], args[4]);
    if bar == SB_CTL {
        let msg = if redraw != 0 {
            SBM_SETRANGEREDRAW
        } else {
            SBM_SETRANGE
        };
        let vars = vec![hwnd, msg, min, max, 1];
        return start(cpu, SCROLLBAR, K_SEND, vars, 0);
    }
    let range = (min as i32, max as i32);
    set(cpu, hwnd, bar, SIF_RANGE, range, 0, 0, redraw != 0, Some(1))
}

/// GetScrollRange(hwnd, bar, min, max): TRUE. A control gets
/// SBM_GETRANGE.
pub fn get_scroll_range(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, bar, min, max) = (args[0], args[1], args[2], args[3]);
    if bar == SB_CTL {
        let vars = vec![hwnd, SBM_GETRANGE, min, max, 1];
        return start(cpu, SCROLLBAR, K_SEND, vars, 0);
    }
    write_range(cpu, hwnd, bar, min, max);
    Some(1)
}

fn write_range(cpu: &mut Cpu, hwnd: u32, bar: u32, min: u32, max: u32) {
    let si = info(cpu, hwnd, bar, false);
    let (lo, hi) = si.map_or((0, 0), |si| (si.min, si.max));
    if min != 0 {
        cpu.bus.guest_write_32(min, lo as u32);
    }
    if max != 0 {
        cpu.bus.guest_write_32(max, hi as u32);
    }
}

/// ShowScrollBar(hwnd, bar, show): TRUE for a window.
pub fn show_scroll_bar(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, bar, show) = (args[0], args[1], args[2] != 0);
    if hwnd == 0 {
        return Some(0);
    }
    match bar {
        SB_CTL => {
            let cmd = if show { SW_SHOW } else { SW_HIDE };
            start(cpu, SCROLLBAR, K_SHOW, vec![hwnd, cmd], 0)
        }
        SB_HORZ | SB_VERT | SB_BOTH => {
            if set_shown(cpu, hwnd, bar, show) {
                return frame_changed(cpu, hwnd, 1);
            }
            Some(1)
        }
        _ => Some(1),
    }
}

/// EnableScrollBar(hwnd, bar, flags): whether it changed anything.
pub fn enable_scroll_bar(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, mut bar, flags) = (args[0], args[1], args[2] & ESB_DISABLE_BOTH);
    let mut fine = true;
    if bar == SB_BOTH {
        let Some(mut si) = info(cpu, hwnd, SB_VERT, true) else {
            return Some(0);
        };
        fine = si.flags == flags;
        if !fine {
            si.flags = flags;
            store(cpu, hwnd, SB_VERT, si);
            redraw_nc(cpu, hwnd, SB_VERT);
        }
        bar = SB_HORZ;
    }
    let Some(mut si) = info(cpu, hwnd, bar, true) else {
        return Some(0);
    };
    if fine && si.flags == flags {
        return Some(0);
    }
    si.flags = flags;
    store(cpu, hwnd, bar, si);
    if bar == SB_CTL {
        let enable = match flags {
            ESB_DISABLE_BOTH => 0,
            ESB_ENABLE_BOTH => 1,
            _ => 2,
        };
        let mut vars = paint_vars(hwnd, 0, false, 0);
        vars[P_RESULT - 1] = 1;
        vars.push(enable);
        return start(cpu, SCROLLBAR, K_ENABLE, vars, PAINT_SCRATCH);
    }
    refresh(cpu, hwnd, bar, 1)
}

// The task.

/// Painting a control (WM_PAINT, WM_PRINTCLIENT, or at once).
const K_PAINT: u32 = 0;
/// SetWindowPos with SWP_FRAMECHANGED: variables window, result.
const K_FRAME: u32 = 1;
/// ShowWindow: variables window, command.
const K_SHOW: u32 = 2;
/// Send a message: variables window, message, wParam, lParam, and what
/// to return (`u32::MAX` for what the message returns).
const K_SEND: u32 = 3;
/// EnableScrollBar of a control: EnableWindow, then painting.
const K_ENABLE: u32 = 4;
/// Tracking a click: variables window, bar, and the point.
const K_TRACK: u32 = 5;
/// A control's WM_CREATE: MoveWindow to its alignment.
const K_CREATE: u32 = 6;

const STEP_PAINT: u16 = 40;

/// The steps of the scroll bars' tasks.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let k = kind(t);
    if k == K_NOTIFY {
        return button::notify_step(t);
    }
    match k {
        K_PAINT => paint_step(cpu, t, value),
        K_FRAME => match t.step {
            0 => {
                let flags =
                    SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED;
                let args = [t.var(1), 0, 0, 0, 0, 0, flags];
                Next::call(address_of("SetWindowPos"), &args, 1)
            }
            _ => Next::Done(t.var(2)),
        },
        K_SHOW => match t.step {
            0 => Next::call(address_of("ShowWindow"), &[t.var(1), t.var(2)], 1),
            _ => Next::Done(1),
        },
        K_SEND => match t.step {
            0 => window::send(t.var(1), t.var(2), t.var(3), t.var(4), 1),
            _ => Next::Done(if t.var(5) == u32::MAX {
                value
            } else {
                t.var(5)
            }),
        },
        K_ENABLE => {
            if t.step >= STEP_PAINT {
                return paint_step(cpu, t, value);
            }
            match t.step {
                0 if t.var(P_OWN) < 2 => {
                    let args = [t.var(P_HWND), t.var(P_OWN)];
                    Next::call(address_of("EnableWindow"), &args, 1)
                }
                _ => {
                    if !user(cpu).is_visible(t.var(P_HWND)) {
                        return Next::Done(1);
                    }
                    t.set(P_HDC, 0);
                    t.set(P_HOW, HOW_DC);
                    Next::call(0, &[], STEP_PAINT)
                }
            }
        }
        K_TRACK => track_step(cpu, t, value),
        K_CREATE => match t.step {
            0 => {
                let args = [t.var(1), t.var(2), t.var(3), t.var(4), t.var(5), 0];
                Next::call(address_of("MoveWindow"), &args, 1)
            }
            _ => Next::Done(0),
        },
        _ => Next::Done(0),
    }
}

/// A control's painting: its brush from WM_CTLCOLORSCROLLBAR, or a
/// size box.
fn paint_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let hwnd = t.var(P_HWND);
    let style = user(cpu).window(hwnd).map_or(0, |w| w.style);
    match t.step.saturating_sub(STEP_PAINT) {
        0 => button::open(cpu, t, STEP_PAINT + 1),
        1 => {
            let hdc = button::prepare(cpu, t, value, 0);
            if hdc == 0 {
                return button::close(cpu, t, STEP_PAINT + 3);
            }
            if style & (SBS_SIZEGRIP | SBS_SIZEBOX) != 0 {
                let r = button::client_rect(cpu, hwnd);
                paint::fill_color(cpu, hdc, r, COLOR_SCROLLBAR);
                if style & SBS_SIZEGRIP != 0 {
                    let grip = Rect::new(
                        r.left.max(r.right - metric(SM_CXVSCROLL) - 1),
                        r.top.max(r.bottom - metric(SM_CYHSCROLL) - 1),
                        r.right,
                        r.bottom,
                    );
                    paint::frame_control(cpu, hdc, grip, DFC_SCROLL, DFCS_SCROLLSIZEGRIP);
                }
                return button::close(cpu, t, STEP_PAINT + 3);
            }
            let parent = button::parent_of(cpu, hwnd);
            window::send(parent, WM_CTLCOLORSCROLLBAR, hdc, hwnd, STEP_PAINT + 2)
        }
        2 => {
            let hdc = t.var(P_HDC);
            let brush = if value != 0 {
                value
            } else {
                button::default_color(cpu, hdc, WM_CTLCOLORSCROLLBAR)
            };
            if let (Some(si), Some((r, vertical))) =
                (info(cpu, hwnd, SB_CTL, true), bar_rect(cpu, hwnd, SB_CTL))
            {
                draw_bar(cpu, hdc, r, vertical, &si, brush);
            }
            button::close(cpu, t, STEP_PAINT + 3)
        }
        _ => button::finish(t),
    }
}

/// The ScrollBar class's window procedure.
pub fn scrollbar_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    let Some(w) = user(cpu).window(hwnd) else {
        return Some(0);
    };
    let style = w.style;
    match msg {
        WM_CREATE => create(cpu, hwnd, lp),
        WM_ENABLE => {
            if let Some(mut si) = info(cpu, hwnd, SB_CTL, false) {
                si.flags = if wp != 0 {
                    ESB_ENABLE_BOTH
                } else {
                    ESB_DISABLE_BOTH
                };
                store(cpu, hwnd, SB_CTL, si);
                return refresh(cpu, hwnd, SB_CTL, 0);
            }
            Some(0)
        }
        WM_LBUTTONDBLCLK | WM_LBUTTONDOWN => {
            let x = lp as u16 as i16 as i32;
            let y = (lp >> 16) as u16 as i16 as i32;
            let vars = vec![hwnd, SB_CTL, x as u32, y as u32];
            start(cpu, SCROLLBAR, K_TRACK, vars, TRACK_SCRATCH)
        }
        // The tracking loop takes these while a click is tracked.
        WM_LBUTTONUP | WM_MOUSEMOVE | WM_SYSTIMER => Some(0),
        WM_KEYDOWN => {
            let code = match wp {
                VK_PRIOR => SB_PAGEUP,
                VK_NEXT => SB_PAGEDOWN,
                VK_HOME => SB_TOP,
                VK_END => SB_BOTTOM,
                VK_UP | VK_LEFT => SB_LINEUP,
                VK_DOWN | VK_RIGHT => SB_LINEDOWN,
                _ => return Some(0),
            };
            let scroll = if style & SBS_VERT != 0 {
                WM_VSCROLL
            } else {
                WM_HSCROLL
            };
            let parent = button::parent_of(cpu, hwnd);
            let vars = vec![parent, scroll, code, hwnd, 0];
            start(cpu, SCROLLBAR, K_SEND, vars, 0)
        }
        WM_KEYUP => {
            input::show_caret(cpu, &[hwnd]);
            Some(0)
        }
        WM_SETFOCUS => {
            focus_caret(cpu, hwnd, true);
            Some(0)
        }
        WM_KILLFOCUS => {
            focus_caret(cpu, hwnd, false);
            Some(0)
        }
        WM_ERASEBKGND => Some(1),
        WM_GETDLGCODE => Some(DLGC_WANTARROWS),
        WM_PAINT | WM_PRINTCLIENT => {
            let vars = paint_vars(hwnd, wp, true, 0);
            start(cpu, SCROLLBAR, K_PAINT, vars, PAINT_SCRATCH)
        }
        SBM_SETPOS => {
            let Some(si) = info(cpu, hwnd, SB_CTL, false) else {
                return Some(0);
            };
            let old = si.pos as u32;
            set(
                cpu,
                hwnd,
                SB_CTL,
                SIF_POS,
                (0, 0),
                0,
                wp as i32,
                lp != 0,
                Some(old),
            )
        }
        SBM_GETPOS => Some(info(cpu, hwnd, SB_CTL, false).map_or(0, |si| si.pos as u32)),
        SBM_SETRANGE | SBM_SETRANGEREDRAW => {
            // The range as it is given, unchecked (SCROLL_SetScrollRange).
            if let Some(mut si) = info(cpu, hwnd, SB_CTL, false) {
                si.min = wp as i32;
                si.max = lp as i32;
                store(cpu, hwnd, SB_CTL, si);
                if msg == SBM_SETRANGEREDRAW {
                    return refresh(cpu, hwnd, SB_CTL, 0);
                }
            }
            Some(0)
        }
        SBM_GETRANGE => {
            write_range(cpu, hwnd, SB_CTL, wp, lp);
            Some(1)
        }
        SBM_ENABLE_ARROWS => enable_scroll_bar(cpu, &[hwnd, SB_CTL, wp]),
        SBM_SETSCROLLINFO => set_from(cpu, hwnd, SB_CTL, lp, wp != 0),
        SBM_GETSCROLLINFO => Some(get_into(cpu, hwnd, SB_CTL, lp) as u32),
        _ => defwnd::def_window_proc(cpu, args),
    }
}

/// WM_CREATE (SCROLL_CreateScrollBar): a disabled control starts with
/// its arrows disabled; a size box or an aligned bar moves to its
/// place and takes the system's size.
fn create(cpu: &mut Cpu, hwnd: u32, cs: u32) -> Option<u32> {
    let w = read_words(cpu, cs, 9);
    let (cy, cx, y, x, style) = (w[4] as i32, w[5] as i32, w[6] as i32, w[7] as i32, w[8]);
    if style & WS_DISABLED != 0
        && let Some(mut si) = info(cpu, hwnd, SB_CTL, false)
    {
        si.flags = ESB_DISABLE_BOTH;
        store(cpu, hwnd, SB_CTL, si);
    }
    let (sx, sy) = (metric(SM_CXVSCROLL) + 1, metric(SM_CYHSCROLL) + 1);
    let place = if style & (SBS_SIZEGRIP | SBS_SIZEBOX) != 0 {
        if style & SBS_TOPALIGN != 0 {
            Some((x, y, sx, sy))
        } else if style & SBS_BOTTOMALIGN != 0 {
            Some((x + cx - sx, y + cy - sy, sx, sy))
        } else {
            None
        }
    } else if style & SBS_VERT != 0 {
        if style & SBS_TOPALIGN != 0 {
            Some((x, y, sx, cy))
        } else if style & SBS_BOTTOMALIGN != 0 {
            Some((x + cx - sx, y, sx, cy))
        } else {
            None
        }
    } else if style & SBS_TOPALIGN != 0 {
        Some((x, y, cx, sy))
    } else if style & SBS_BOTTOMALIGN != 0 {
        Some((x, y + cy - sy, cx, sy))
    } else {
        None
    };
    match place {
        Some((x, y, cx, cy)) => {
            let vars = vec![hwnd, x as u32, y as u32, cx as u32, cy as u32];
            start(cpu, SCROLLBAR, K_CREATE, vars, 0)
        }
        None => Some(0),
    }
}

/// A focused control shows a blinking caret over its thumb.
fn focus_caret(cpu: &mut Cpu, hwnd: u32, focus: bool) {
    let (Some(si), Some((r, vertical))) =
        (info(cpu, hwnd, SB_CTL, false), bar_rect(cpu, hwnd, SB_CTL))
    else {
        return;
    };
    let (_, size, pos) = geometry(r, vertical, &si);
    if focus {
        let (w, h, x, y) = if vertical {
            (r.width() - 2, size - 2, r.left + 1, pos + 1)
        } else {
            (size - 2, r.height() - 2, pos + 1, r.top + 1)
        };
        input::create_caret(cpu, &[hwnd, 1, w as u32, h as u32]);
        input::set_caret_pos(cpu, &[x as u32, y as u32]);
        input::show_caret(cpu, &[hwnd]);
        return;
    }
    let thumb = if vertical {
        Rect::new(r.left, pos + 1, r.right, pos + 1 + size)
    } else {
        Rect::new(pos + 1, r.top, pos + 1 + size, r.bottom)
    };
    input::hide_caret(cpu, &[hwnd]);
    let at = cpu.esp().wrapping_sub(64) & !3;
    thumb.write(cpu, at);
    paint::invalidate_rect(cpu, &[hwnd, at, 0]);
    input::destroy_caret(cpu, &[]);
}

// Tracking.

/// The scratch space of tracking: the MSG GetMessageA fills.
const TRACK_SCRATCH: u32 = 32;

// The variables of tracking.
const T_HWND: usize = 1;
const T_BAR: usize = 2;
/// The point of the event, in the coordinates the bar is drawn in.
const T_X: usize = 3;
const T_Y: usize = 4;
/// The offset of the client area in the window, for a window's bar.
const T_DX: usize = 5;
const T_DY: usize = 6;
/// The part the click went down on (SCROLL_trackHitTest).
const T_HIT: usize = 7;
/// The pixel of the click, of the mouse last, and the thumb's at the
/// click.
const T_CLICK: usize = 8;
const T_MOUSE: usize = 9;
const T_THUMB: usize = 10;
/// The event: WM_LBUTTONDOWN, WM_MOUSEMOVE, WM_LBUTTONUP or
/// WM_SYSTIMER.
const T_MSG: usize = 11;
/// The DC a control's bar is drawn on during the event.
const T_HDC: usize = 12;
/// What is left to do after the event's drawing: the next to do, the
/// count, then the operations, two variables each.
const T_NEXT: usize = 13;
const T_OPS: usize = 14;
const T_OP0: usize = 15;

/// The operations.
const OP_SETFOCUS: u32 = 1;
const OP_SETCAPTURE: u32 = 2;
const OP_RELEASE: u32 = 3;
/// Send WM_HSCROLL or WM_VSCROLL with the wParam.
const OP_SCROLL: u32 = 4;

// The steps of tracking.
const S_EVENT: u16 = 1;
const S_DRAW: u16 = 2;
const S_OPS: u16 = 3;
const S_GET: u16 = 4;
const S_GOT: u16 = 5;
const S_FILTERED: u16 = 6;
const S_TRANSLATED: u16 = 7;
const S_DISPATCHED: u16 = 8;

/// DefWindowProcA's WM_SYSCOMMAND with SC_HSCROLL or SC_VSCROLL: args
/// are the window, the bar (0 horizontal, 1 vertical) and the point
/// (screen coordinates) as lParam has it.
pub fn track(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hwnd, bar, lp) = (args[0], args[1], args[2]);
    let x = lp as u16 as i16 as i32;
    let y = (lp >> 16) as u16 as i16 as i32;
    let vars = vec![hwnd, bar, x as u32, y as u32];
    start(cpu, SCROLLBAR, K_TRACK, vars, TRACK_SCRATCH)
}

fn push_op(t: &mut Task, op: u32, arg: u32) {
    let n = t.var(T_OPS) as usize;
    t.set(T_OP0 + 2 * n, op);
    t.set(T_OP0 + 2 * n + 1, arg);
    t.set(T_OPS, n as u32 + 1);
}

fn track_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let (hwnd, bar) = (t.var(T_HWND), t.var(T_BAR));
    match t.step {
        0 => {
            // SCROLL_TrackScrollBar: a window's bar takes the point in
            // window coordinates.
            if bar != SB_CTL {
                let state = user(cpu);
                let Some(w) = state.window(hwnd) else {
                    return Next::Done(0);
                };
                let (dx, dy) = (w.client.left - w.rect.left, w.client.top - w.rect.top);
                let (ox, oy) = state.client_origin(hwnd);
                t.set(T_DX, dx as u32);
                t.set(T_DY, dy as u32);
                let x = t.var(T_X) as i32 - ox + dx;
                let y = t.var(T_Y) as i32 - oy + dy;
                t.set(T_X, x as u32);
                t.set(T_Y, y as u32);
            }
            t.set(T_HIT, HIT_NOWHERE);
            t.set(T_MSG, WM_LBUTTONDOWN);
            Next::call(0, &[], S_EVENT)
        }
        S_EVENT => {
            // A control asks its parent for the brush to draw with.
            t.set(T_OPS, 0);
            t.set(T_NEXT, 0);
            t.set(T_HDC, 0);
            if bar == SB_CTL && user(cpu).is_window(hwnd) {
                let hdc = paint::get_dc(cpu, &[hwnd]).unwrap_or(0);
                if hdc != 0 {
                    t.set(T_HDC, hdc);
                    let parent = button::parent_of(cpu, hwnd);
                    return window::send(parent, WM_CTLCOLORSCROLLBAR, hdc, hwnd, S_DRAW);
                }
            }
            Next::call(0, &[], S_DRAW)
        }
        S_DRAW => {
            event(cpu, t, value);
            Next::call(0, &[], S_OPS)
        }
        S_OPS => {
            let at = t.var(T_NEXT) as usize;
            if at >= t.var(T_OPS) as usize {
                return Next::call(0, &[], S_GET);
            }
            t.set(T_NEXT, at as u32 + 1);
            let (op, arg) = (t.var(T_OP0 + 2 * at), t.var(T_OP0 + 2 * at + 1));
            match op {
                OP_SETFOCUS => Next::call(address_of("SetFocus"), &[hwnd], S_OPS),
                OP_SETCAPTURE => Next::call(address_of("SetCapture"), &[hwnd], S_OPS),
                OP_RELEASE => Next::call(address_of("ReleaseCapture"), &[], S_OPS),
                _ => {
                    let (owner, ctl) = if bar == SB_CTL {
                        (button::parent_of(cpu, hwnd), hwnd)
                    } else {
                        (hwnd, 0)
                    };
                    let msg = if is_vertical(cpu, hwnd, bar) {
                        WM_VSCROLL
                    } else {
                        WM_HSCROLL
                    };
                    window::send(owner, msg, arg, ctl, S_OPS)
                }
            }
        }
        S_GET => {
            let state = user(cpu);
            if !state.is_window(hwnd) {
                return Next::call(address_of("ReleaseCapture"), &[], 99);
            }
            if t.var(T_MSG) == WM_LBUTTONUP || state.input.capture != hwnd {
                return Next::Done(0);
            }
            Next::call(address_of("GetMessageA"), &[t.scratch, 0, 0, 0], S_GOT)
        }
        S_GOT => {
            if value == 0 {
                return Next::Done(0);
            }
            Next::call(
                address_of("CallMsgFilterA"),
                &[t.scratch, MSGF_SCROLLBAR],
                S_FILTERED,
            )
        }
        S_FILTERED => {
            if value != 0 {
                return Next::call(0, &[], S_GET);
            }
            let m = read_words(cpu, t.scratch, 4);
            let (msg, wp, lp) = (m[1], m[2], m[3]);
            if msg == WM_LBUTTONUP
                || msg == WM_MOUSEMOVE
                || (msg == WM_SYSTIMER && wp == SCROLL_TIMER)
            {
                let x = lp as u16 as i16 as i32 + t.var(T_DX) as i32;
                let y = (lp >> 16) as u16 as i16 as i32 + t.var(T_DY) as i32;
                t.set(T_X, x as u32);
                t.set(T_Y, y as u32);
                t.set(T_MSG, msg);
                return Next::call(0, &[], S_EVENT);
            }
            Next::call(address_of("TranslateMessage"), &[t.scratch], S_TRANSLATED)
        }
        S_TRANSLATED => Next::call(address_of("DispatchMessageA"), &[t.scratch], S_DISPATCHED),
        S_DISPATCHED => Next::call(0, &[], S_GET),
        _ => Next::Done(0),
    }
}

fn is_vertical(cpu: &mut Cpu, hwnd: u32, bar: u32) -> bool {
    bar_rect(cpu, hwnd, bar).is_some_and(|(_, v)| v)
}

/// One event of a tracked click (SCROLL_HandleScrollEvent): the bar's
/// state and drawing change at once, and what calls into the program
/// is left in the operations. `brush` is a control's brush, or 0.
fn event(cpu: &mut Cpu, t: &mut Task, brush: u32) {
    let (hwnd, bar, msg) = (t.var(T_HWND), t.var(T_BAR), t.var(T_MSG));
    let p = (t.var(T_X) as i32, t.var(T_Y) as i32);
    let hdc = t.var(T_HDC);
    let release = |cpu: &mut Cpu| {
        if hdc != 0 {
            paint::release_dc(cpu, &[hwnd, hdc]);
        }
    };
    let Some(mut si) = info(cpu, hwnd, bar, false) else {
        release(cpu);
        return;
    };
    if t.var(T_HIT) == HIT_NOWHERE && msg != WM_LBUTTONDOWN {
        release(cpu);
        return;
    }
    let style = user(cpu).window(hwnd).map_or(0, |w| w.style);
    if bar == SB_CTL && style & (SBS_SIZEGRIP | SBS_SIZEBOX) != 0 {
        match msg {
            WM_LBUTTONDOWN => {
                push_op(t, OP_SETCAPTURE, 0);
                t.set(T_HIT, HIT_THUMB);
            }
            WM_LBUTTONUP => {
                push_op(t, OP_RELEASE, 0);
                t.set(T_HIT, HIT_NOWHERE);
            }
            _ => {}
        }
        release(cpu);
        return;
    }
    let Some((r, vertical)) = bar_rect(cpu, hwnd, bar) else {
        release(cpu);
        return;
    };
    let (_, _, thumb) = geometry(r, vertical, &si);
    let along = |(x, y): (i32, i32)| if vertical { y - r.top } else { x - r.left };
    let hit = match msg {
        WM_LBUTTONDOWN => {
            let hit = hit_test(r, vertical, &si, p, false);
            t.set(T_HIT, hit);
            t.set(T_CLICK, along(p) as u32);
            t.set(T_MOUSE, along(p) as u32);
            t.set(T_THUMB, thumb as u32);
            if bar == SB_CTL && style & WS_TABSTOP != 0 {
                push_op(t, OP_SETFOCUS, 0);
            }
            push_op(t, OP_SETCAPTURE, 0);
            hit
        }
        WM_MOUSEMOVE => hit_test(r, vertical, &si, p, true),
        WM_LBUTTONUP => {
            push_op(t, OP_RELEASE, 0);
            HIT_NOWHERE
        }
        _ => hit_test(r, vertical, &si, p, false),
    };
    let tracked = t.var(T_HIT);
    let (click, thumb_at) = (t.var(T_CLICK) as i32, t.var(T_THUMB) as i32);
    let codes = [
        (HIT_TOP_ARROW, SB_LINEUP),
        (HIT_TOP_RECT, SB_PAGEUP),
        (HIT_BOTTOM_RECT, SB_PAGEDOWN),
        (HIT_BOTTOM_ARROW, SB_LINEDOWN),
    ];
    if let Some(&(_, code)) = codes.iter().find(|(h, _)| *h == tracked) {
        if hit == tracked {
            si.pressed = tracked;
            if msg == WM_LBUTTONDOWN || msg == WM_SYSTIMER {
                push_op(t, OP_SCROLL, code);
            }
            let delay = if msg == WM_LBUTTONDOWN {
                FIRST_DELAY
            } else {
                REPEAT_DELAY
            };
            message::set_system_timer(cpu, &[hwnd, SCROLL_TIMER, delay, 0]);
        } else {
            si.pressed = HIT_NOWHERE;
            message::kill_system_timer(cpu, &[hwnd, SCROLL_TIMER]);
        }
    } else if tracked == HIT_THUMB {
        match msg {
            WM_LBUTTONDOWN => {
                si.dragging = true;
                si.drag = thumb_at + t.var(T_MOUSE) as i32 - click;
                si.track = thumb_value(&si, r, vertical, si.drag);
            }
            WM_LBUTTONUP => si.dragging = false,
            WM_MOUSEMOVE => {
                let pos = if !near(r, p, vertical) {
                    click
                } else {
                    let x = p.0.clamp(r.left, r.right);
                    let y = p.1.clamp(r.top, r.bottom);
                    along((x, y))
                };
                if pos != t.var(T_MOUSE) as i32 || !si.dragging {
                    t.set(T_MOUSE, pos as u32);
                    si.dragging = true;
                    si.drag = thumb_at + pos - click;
                    si.track = thumb_value(&si, r, vertical, si.drag);
                    push_op(
                        t,
                        OP_SCROLL,
                        SB_THUMBTRACK | (si.track as u32 & 0xFFFF) << 16,
                    );
                }
            }
            _ => {}
        }
    }
    let mouse = t.var(T_MOUSE) as i32;
    if msg == WM_LBUTTONDOWN && hit == HIT_THUMB {
        let val = thumb_value(&si, r, vertical, thumb_at + mouse - click);
        push_op(t, OP_SCROLL, SB_THUMBTRACK | (val as u32 & 0xFFFF) << 16);
    }
    if msg == WM_LBUTTONUP {
        if tracked == HIT_THUMB {
            let val = thumb_value(&si, r, vertical, thumb_at + mouse - click);
            push_op(t, OP_SCROLL, SB_THUMBPOSITION | (val as u32 & 0xFFFF) << 16);
        }
        push_op(t, OP_SCROLL, SB_ENDSCROLL);
        t.set(T_HIT, HIT_NOWHERE);
        si.pressed = HIT_NOWHERE;
        si.dragging = false;
        message::kill_system_timer(cpu, &[hwnd, SCROLL_TIMER]);
    }
    store(cpu, hwnd, bar, si);
    // Draw the bar as it is now.
    if bar == SB_CTL {
        if hdc != 0 {
            let brush = if brush != 0 {
                brush
            } else {
                button::default_color(cpu, hdc, WM_CTLCOLORSCROLLBAR)
            };
            draw_bar(cpu, hdc, r, vertical, &si, brush);
        }
    } else {
        redraw_nc(cpu, hwnd, bar);
    }
    release(cpu);
}

/// A window of the ScrollBar class is made: its bar's state is there
/// from the start, with the range 0 to 0.
pub fn init(cpu: &mut Cpu, hwnd: u32) {
    if !is_control(cpu, hwnd) {
        return;
    }
    if let Some(w) = user(cpu).window_mut(hwnd) {
        w.scroll[0] = ScrollInfo {
            made: true,
            ..ScrollInfo::default()
        };
    }
}

crate::state_fields!(ScrollInfo {
    min,
    max,
    pos,
    page,
    flags,
    made,
    pressed,
    dragging,
    drag,
    track
});

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(min: i32, max: i32, page: i32, pos: i32) -> ScrollInfo {
        ScrollInfo {
            min,
            max,
            page,
            pos,
            made: true,
            ..ScrollInfo::default()
        }
    }

    #[test]
    fn mul_div_rounds() {
        assert_eq!(mul_div(10, 1, 3), 3);
        assert_eq!(mul_div(10, 2, 3), 7);
        assert_eq!(mul_div(-10, 2, 3), -7);
        assert_eq!(mul_div(1, 1, 0), -1);
    }

    #[test]
    fn a_page_bigger_than_the_range_is_cut() {
        let mut si = bar(0, 100, 0, 0);
        apply(&mut si, SB_CTL, SIF_PAGE, 0, 0, 500, 0, false);
        assert_eq!(si.page, 101);
    }

    #[test]
    fn the_position_stays_in_the_range_less_the_page() {
        let mut si = bar(0, 100, 0, 0);
        apply(&mut si, SB_CTL, SIF_PAGE | SIF_POS, 0, 0, 10, 200, false);
        assert_eq!(si.pos, 91);
        apply(&mut si, SB_CTL, SIF_POS, 0, 0, 0, -5, false);
        assert_eq!(si.pos, 0);
    }

    #[test]
    fn a_bad_range_becomes_empty() {
        let mut si = bar(0, 100, 0, 50);
        apply(&mut si, SB_CTL, SIF_RANGE, 10, 5, 0, 0, false);
        assert_eq!((si.min, si.max, si.pos), (0, 0, 0));
    }

    #[test]
    fn an_empty_range_hides_a_window_bar() {
        let mut si = bar(0, 100, 0, 0);
        let after = apply(&mut si, SB_VERT, SIF_RANGE, 0, 0, 0, 0, false);
        assert_eq!(after, After::Hide);
        let after = apply(&mut si, SB_VERT, SIF_RANGE, 0, 50, 0, 0, false);
        assert_eq!(after, After::Show(false));
        let mut si = bar(0, 100, 0, 0);
        let after = apply(
            &mut si,
            SB_VERT,
            SIF_RANGE | SIF_DISABLENOSCROLL,
            0,
            0,
            0,
            0,
            false,
        );
        assert_eq!(after, After::Redraw);
        assert_eq!(si.flags, ESB_DISABLE_BOTH);
    }

    #[test]
    fn the_thumb_goes_back_to_its_value() {
        let r = Rect::new(0, 0, 16, 200);
        let si = bar(0, 100, 0, 40);
        let (arrow, size, pos) = geometry(r, true, &si);
        assert_eq!(arrow, metric(SM_CXVSCROLL));
        assert!(size > 0 && pos > arrow);
        assert_eq!(thumb_value(&si, r, true, pos), 40);
        assert_eq!(hit_test(r, true, &si, (5, pos + 1), false), HIT_THUMB);
        assert_eq!(hit_test(r, true, &si, (5, 1), false), HIT_TOP_ARROW);
        assert_eq!(hit_test(r, true, &si, (5, 199), false), HIT_BOTTOM_ARROW);
        assert_eq!(
            hit_test(r, true, &si, (5, 190 - arrow), false),
            HIT_BOTTOM_RECT
        );
    }

    #[test]
    fn a_short_bar_has_no_thumb() {
        let r = Rect::new(0, 0, 16, 30);
        let (arrow, size, pos) = geometry(r, true, &bar(0, 100, 0, 0));
        assert_eq!((arrow, size, pos), (13, 0, 0));
    }
}
