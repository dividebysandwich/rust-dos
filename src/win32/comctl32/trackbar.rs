//! The trackbar (msctls_trackbar32): a thumb that slides along a
//! channel between a range's ends, by the mouse (dragged, or a page at
//! a time by a click beside it) and by the arrow, page, home and end
//! keys, telling its parent of each move with WM_HSCROLL (WM_VSCROLL
//! with TBS_VERT). It draws tic marks at the ends and where it is told
//! (TBM_SETTIC) or every so many values (TBS_AUTOTICKS with
//! TBM_SETTICFREQ), a selected span of the channel with
//! TBS_ENABLESELRANGE, and keeps buddy windows at its ends.
//!
//! A change that the program asks to be drawn (TBM_SETPOS with TRUE,
//! the range, the selection, the thumb's length, the tic frequency, a
//! buddy) is painted at once through UpdateWindow, as Windows does. At
//! each painting the control asks its parent for its background with
//! WM_CTLCOLORSTATIC and tells it of the drawing with NM_CUSTOMDRAW.
//!
//! Ported from Wine's trackbar.c:
//!
//! Copyright 1998, 1999 Eric Kohl
//! Copyright 1998, 1999 Alex Priem
//! Copyright 2002 Dimitrie O. Paun
//!
//! This library is free software; you can redistribute it and/or modify
//! it under the terms of the GNU Lesser General Public License as
//! published by the Free Software Foundation; either version 2.1 of the
//! License, or (at your option) any later version.

use super::gfx::{self, COLOR_3DDKSHADOW, COLOR_BTNFACE, COLOR_BTNHIGHLIGHT, COLOR_HIGHLIGHT};
use super::script::{self, Arg, Ctx, Op, Script};
use super::{TRACKBAR, invalidate, misc, state};
use crate::cpu::Cpu;
use crate::win32::gdi32::{self, Rect};
use crate::win32::user32::message::{
    WM_CREATE, WM_CTLCOLORSTATIC, WM_DESTROY, WM_ERASEBKGND, WM_GETDLGCODE, WM_HSCROLL, WM_KEYDOWN,
    WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NOTIFY, WM_NOTIFYFORMAT,
    WM_PAINT, WM_SETFOCUS, WM_SIZE, WM_STYLECHANGED, WM_TIMER, WM_VSCROLL,
};
use crate::win32::user32::{self, defwnd, read_words, write_words};
use std::collections::BTreeMap;

const WM_PRINTCLIENT: u32 = 0x0318;
const WM_CAPTURECHANGED: u32 = 0x0215;
const WM_WININICHANGE: u32 = 0x001A;
const NF_QUERY: u32 = 3;
const GWL_STYLE: u32 = (-16i32) as u32;
const DLGC_WANTARROWS: u32 = 0x0001;

const TBM_GETPOS: u32 = 0x0400;
const TBM_GETRANGEMIN: u32 = 0x0401;
const TBM_GETRANGEMAX: u32 = 0x0402;
const TBM_GETTIC: u32 = 0x0403;
const TBM_SETTIC: u32 = 0x0404;
const TBM_SETPOS: u32 = 0x0405;
const TBM_SETRANGE: u32 = 0x0406;
const TBM_SETRANGEMIN: u32 = 0x0407;
const TBM_SETRANGEMAX: u32 = 0x0408;
const TBM_CLEARTICS: u32 = 0x0409;
const TBM_SETSEL: u32 = 0x040A;
const TBM_SETSELSTART: u32 = 0x040B;
const TBM_SETSELEND: u32 = 0x040C;
const TBM_GETPTICS: u32 = 0x040E;
const TBM_GETTICPOS: u32 = 0x040F;
const TBM_GETNUMTICS: u32 = 0x0410;
const TBM_GETSELSTART: u32 = 0x0411;
const TBM_GETSELEND: u32 = 0x0412;
const TBM_CLEARSEL: u32 = 0x0413;
const TBM_SETTICFREQ: u32 = 0x0414;
const TBM_SETPAGESIZE: u32 = 0x0415;
const TBM_GETPAGESIZE: u32 = 0x0416;
const TBM_SETLINESIZE: u32 = 0x0417;
const TBM_GETLINESIZE: u32 = 0x0418;
const TBM_GETTHUMBRECT: u32 = 0x0419;
const TBM_GETCHANNELRECT: u32 = 0x041A;
const TBM_SETTHUMBLENGTH: u32 = 0x041B;
const TBM_GETTHUMBLENGTH: u32 = 0x041C;
const TBM_SETTOOLTIPS: u32 = 0x041D;
const TBM_GETTOOLTIPS: u32 = 0x041E;
const TBM_SETTIPSIDE: u32 = 0x041F;
const TBM_SETBUDDY: u32 = 0x0420;
const TBM_GETBUDDY: u32 = 0x0421;
const TBM_SETUNICODEFORMAT: u32 = 0x2005;
const TBM_GETUNICODEFORMAT: u32 = 0x2006;

const TBS_AUTOTICKS: u32 = 0x0001;
const TBS_VERT: u32 = 0x0002;
const TBS_TOP: u32 = 0x0004;
const TBS_LEFT: u32 = 0x0004;
const TBS_BOTH: u32 = 0x0008;
const TBS_NOTICKS: u32 = 0x0010;
const TBS_ENABLESELRANGE: u32 = 0x0020;
const TBS_FIXEDLENGTH: u32 = 0x0040;
const TBS_NOTHUMB: u32 = 0x0080;
const TBS_TOOLTIPS: u32 = 0x0100;
const TBS_DOWNISLEFT: u32 = 0x0400;

const TB_LINEUP: u32 = 0;
const TB_LINEDOWN: u32 = 1;
const TB_PAGEUP: u32 = 2;
const TB_PAGEDOWN: u32 = 3;
const TB_THUMBPOSITION: u32 = 4;
const TB_THUMBTRACK: u32 = 5;
const TB_TOP: u32 = 6;
const TB_BOTTOM: u32 = 7;
const TB_ENDTRACK: u32 = 8;
const TBTS_TOP: i32 = 0;

const VK_PRIOR: u32 = 0x21;
const VK_NEXT: u32 = 0x22;
const VK_END: u32 = 0x23;
const VK_HOME: u32 = 0x24;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;

const NM_CUSTOMDRAW: u32 = (-12i32) as u32;
const NM_RELEASEDCAPTURE: u32 = (-16i32) as u32;
const CDDS_PREPAINT: u32 = 0x0001;
const CDDS_POSTPAINT: u32 = 0x0002;
const CDRF_SKIPDEFAULT: u32 = 0x0004;
const CDRF_NOTIFYPOSTPAINT: u32 = 0x0010;

const TTM_ADDTOOLA: u32 = 0x0404;
const TTF_IDISHWND: u32 = 0x0001;
const TTF_TRACK: u32 = 0x0020;
const TTF_ABSOLUTE: u32 = 0x0080;
const WS_POPUP: u32 = 0x8000_0000;
const CW_USEDEFAULT: u32 = 0x8000_0000;

const EDGE_SUNKEN: u32 = 0x000A;
const BF_RECT: u32 = 0x000F;
const BF_ADJUST: u32 = 0x2000;
const WHITE_BRUSH: u32 = 0;
const WHITE_PEN: u32 = 6;
const BLACK_PEN: u32 = 7;

const TB_REFRESH_TIMER: u32 = 1;
const TB_REFRESH_DELAY: u32 = 500;
const TB_DEFAULTPAGESIZE: i32 = 20;

const FLAG_DRAG: u32 = 0x08;
const FLAG_PAGE_LEFT: u32 = 0x10;
const FLAG_PAGE_RIGHT: u32 = 0x20;
const FLAG_PAGE: u32 = FLAG_PAGE_LEFT | FLAG_PAGE_RIGHT;

const TIC_EDGE: u32 = 0x20;
const TIC_SELECTIONMARKMAX: u32 = 0x80;
const TIC_SELECTIONMARKMIN: u32 = 0x100;
const TIC_SELECTIONMARK: u32 = TIC_SELECTIONMARKMAX | TIC_SELECTIONMARKMIN;

/// The scratch space of a painting script: an NMCUSTOMDRAW, the focus
/// rectangle, the PAINTSTRUCT.
const S_NM: u32 = 0;
const S_FOCUS: u32 = 48;
const S_PS: u32 = 64;
const S_PAINT: u32 = 128;
/// The scratch space of the script that makes the tooltip: a TOOLINFO.
const S_TOOL: u32 = 0;
const S_CREATE: u32 = 64;

// The continuations.
const T_BEGIN: u16 = 1;
const T_BRUSH: u16 = 2;
const T_PREPAINT: u16 = 3;
const T_TOOLTIP: u16 = 4;

/// What a trackbar keeps.
#[derive(Clone, Debug, Default)]
pub struct Trackbar {
    pub style: u32,
    pub min: i32,
    pub max: i32,
    pub line: i32,
    pub page: i32,
    pub sel_min: i32,
    pub sel_max: i32,
    pub pos: i32,
    pub thumb_len: i32,
    pub tics: Vec<i32>,
    pub tic_freq: u32,
    pub notify: u32,
    pub tooltip: u32,
    pub buddy_la: u32,
    pub buddy_rb: u32,
    pub tip_side: i32,
    pub flags: u32,
    pub unicode: bool,
    pub focused: bool,
    /// Where the mouse last was, for the paging that repeats.
    pub mouse: (i32, i32),
    /// The block TBM_GETPTICS gave the program, and its size in tics.
    pub ptics: u32,
    pub ptics_room: u32,
}

crate::state_fields!(Trackbar {
    style,
    min,
    max,
    line,
    page,
    sel_min,
    sel_max,
    pos,
    thumb_len,
    tics,
    tic_freq,
    notify,
    tooltip,
    buddy_la,
    buddy_rb,
    tip_side,
    flags,
    unicode,
    focused,
    mouse,
    ptics,
    ptics_room
});

/// What the module keeps, in COMCTL32's `State`: the trackbars, by
/// window handle.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub bars: BTreeMap<u32, Trackbar>,
}

crate::state_fields!(State { bars });

fn get(cpu: &mut Cpu, hwnd: u32) -> Option<Trackbar> {
    state(cpu).trackbar.bars.get(&hwnd).cloned()
}

fn put(cpu: &mut Cpu, hwnd: u32, t: Trackbar) {
    state(cpu).trackbar.bars.insert(hwnd, t);
}

fn is_window(cpu: &mut Cpu, hwnd: u32) -> bool {
    hwnd != 0 && user32::user(cpu).window(hwnd).is_some()
}

/// The tics TBS_AUTOTICKS puts every `tic_freq` values between the ends
/// (TRACKBAR_RecalculateTics).
fn recalculate_tics(t: &mut Trackbar) {
    t.tics.clear();
    if t.tic_freq == 0 || t.max < t.min {
        return;
    }
    let range = t.max.wrapping_sub(t.min) as u32;
    let mut n = range / t.tic_freq;
    if n != 0 && range.is_multiple_of(t.tic_freq) {
        n -= 1;
    }
    let mut tic = t.min.wrapping_add(t.tic_freq as i32);
    for _ in 0..n.min(0x1_0000) {
        t.tics.push(tic);
        tic = tic.wrapping_add(t.tic_freq as i32);
    }
}

/// The thumb's length, for working out places: the program can set any
/// length, and this keeps it within 65536 pixels so the sums don't
/// overflow.
fn thumb_length(t: &Trackbar) -> i32 {
    t.thumb_len.clamp(-0x1_0000, 0x1_0000)
}

/// The channel's rectangle (TRACKBAR_CalcChannel).
fn channel(client: Rect, t: &Trackbar) -> Rect {
    let offset_thumb = thumb_length(t) / 4;
    let offset_edge = offset_thumb + 3;
    let sel = t.style & TBS_ENABLESELRANGE != 0;
    let cy = if sel { offset_thumb * 3 } else { 4 };
    let mut across = if sel {
        (thumb_length(t) - cy + 2) / 2
    } else {
        thumb_length(t) / 2 - 1
    };
    if t.style & TBS_BOTH != 0 {
        across += if t.style & TBS_NOTICKS != 0 { 1 } else { 9 };
    } else if t.style & TBS_TOP != 0 {
        across += if t.style & TBS_NOTICKS != 0 { 2 } else { 10 };
    }
    if t.style & TBS_VERT != 0 {
        let left = client.left + across;
        Rect::new(
            left,
            client.top + offset_edge,
            left + cy,
            client.bottom - offset_edge,
        )
    } else {
        let top = client.top + across;
        Rect::new(
            client.left + offset_edge,
            top,
            client.right - offset_edge,
            top + cy,
        )
    }
}

/// The thumb's rectangle at the position `pos` (TRACKBAR_CalcThumb).
fn thumb_at(ch: Rect, t: &Trackbar, pos: i32) -> Rect {
    let range = match t.max.wrapping_sub(t.min) {
        0 => 1,
        r => r,
    };
    let thumb_width = (thumb_length(t) / 2) | 1;
    let ticks_side = t.style & TBS_NOTICKS == 0;
    let along = |len: i32| (len as i64 * (pos as i64 - t.min as i64) / range as i64) as i32;
    if t.style & TBS_VERT != 0 {
        let height = ch.height() - thumb_width;
        let left = if t.style & (TBS_BOTH | TBS_LEFT) != 0 && ticks_side {
            10
        } else {
            2
        };
        let top = ch.top + along(height);
        Rect::new(left, top, left + thumb_length(t), top + thumb_width)
    } else {
        let width = ch.width() - thumb_width;
        let left = ch.left + along(width);
        let top = if t.style & (TBS_BOTH | TBS_TOP) != 0 && ticks_side {
            10
        } else {
            2
        };
        Rect::new(left, top, left + thumb_width, top + thumb_length(t))
    }
}

/// The channel, the thumb and the selection, for the control's client
/// area now.
struct Layout {
    client: Rect,
    channel: Rect,
    thumb: Rect,
    selection: Rect,
}

fn layout(cpu: &mut Cpu, hwnd: u32, t: &Trackbar) -> Layout {
    let client = super::client(cpu, hwnd);
    let ch = channel(client, t);
    let thumb = thumb_at(ch, t, t.pos);
    let range = t.max.wrapping_sub(t.min);
    let selection = if range <= 0 {
        Rect::default()
    } else if t.style & TBS_VERT != 0 {
        let off = thumb.height() / 2;
        let height = ch.height() - off * 2;
        let at = |v: i32| ch.top + off + (height as i64 * v as i64 / range as i64) as i32;
        Rect::new(ch.left + 3, at(t.sel_min), ch.right - 3, at(t.sel_max))
    } else {
        let off = thumb.width() / 2;
        let width = ch.width() - off * 2;
        let at = |v: i32| ch.left + off + (width as i64 * v as i64 / range as i64) as i32;
        Rect::new(at(t.sel_min), ch.top + 3, at(t.sel_max), ch.bottom - 3)
    };
    Layout {
        client,
        channel: ch,
        thumb,
        selection,
    }
}

/// The thumb's length for the client area (TRACKBAR_InitializeThumb).
fn initialize_thumb(cpu: &mut Cpu, hwnd: u32, t: &mut Trackbar) {
    let metric = if t.style & TBS_ENABLESELRANGE != 0 {
        23
    } else {
        21
    };
    let r = super::client(cpu, hwnd);
    let width = if t.style & TBS_VERT != 0 {
        r.width()
    } else {
        r.height()
    };
    t.thumb_len = if width >= metric {
        metric
    } else if width > 9 {
        width - 6
    } else {
        4
    };
}

/// The value at the place `place` along the channel
/// (TRACKBAR_ConvertPlaceToPosition).
fn place_to_position(l: &Layout, t: &Trackbar, place: i32) -> i32 {
    let range = (t.max as f64) - (t.min as f64);
    let (offset, width, start) = if t.style & TBS_VERT != 0 {
        let o = (l.thumb.height() / 2) as f64;
        (o, l.channel.height() as f64 - o * 2.0 - 1.0, l.channel.top)
    } else {
        let o = (l.thumb.width() / 2) as f64;
        (o, l.channel.width() as f64 - o * 2.0 - 1.0, l.channel.left)
    };
    let mut pos = if width != 0.0 {
        range * (place as f64 - start as f64 - offset) / width
    } else {
        0.0
    };
    pos += t.min as f64;
    pos = pos.min(t.max as f64).max(t.min as f64);
    (pos + 0.5).floor() as i32
}

/// Which way a click at (`x`, `y`) pages: by how many values, or 0
/// when it is off the channel (TRACKBAR_GetAutoPageDirection).
fn page_direction(l: &Layout, t: &Trackbar, x: i32, y: i32) -> i32 {
    let r = if t.style & TBS_VERT != 0 {
        Rect::new(l.thumb.left, l.channel.top, l.thumb.right, l.channel.bottom)
    } else {
        Rect::new(l.channel.left, l.thumb.top, l.channel.right, l.thumb.bottom)
    };
    if !r.contains(x, y) {
        return 0;
    }
    let place = if t.style & TBS_VERT != 0 { y } else { x };
    place_to_position(l, t, place).wrapping_sub(t.pos)
}

/// Add telling the parent of a move, with WM_HSCROLL or WM_VSCROLL.
fn notify_scroll(hwnd: u32, t: &Trackbar, s: &mut Script, code: u32) {
    if t.notify == 0 {
        return;
    }
    let msg = if t.style & TBS_VERT != 0 {
        WM_VSCROLL
    } else {
        WM_HSCROLL
    };
    s.send(t.notify, msg, Arg::V(code), Arg::V(hwnd));
}

fn page_down(hwnd: u32, t: &mut Trackbar, s: &mut Script) {
    if t.pos == t.max {
        return;
    }
    t.pos = t.pos.saturating_add(t.page).min(t.max);
    notify_scroll(hwnd, t, s, TB_PAGEDOWN);
}

fn page_up(hwnd: u32, t: &mut Trackbar, s: &mut Script) {
    if t.pos == t.min {
        return;
    }
    t.pos = t.pos.saturating_sub(t.page).max(t.min);
    notify_scroll(hwnd, t, s, TB_PAGEUP);
}

fn line_up(hwnd: u32, t: &mut Trackbar, s: &mut Script) {
    if t.pos == t.min {
        return;
    }
    t.pos = t.pos.saturating_sub(t.line).max(t.min);
    notify_scroll(hwnd, t, s, TB_LINEUP);
}

fn line_down(hwnd: u32, t: &mut Trackbar, s: &mut Script) {
    if t.pos == t.max {
        return;
    }
    t.pos = t.pos.saturating_add(t.line).min(t.max);
    notify_scroll(hwnd, t, s, TB_LINEDOWN);
}

/// Page towards the mouse at (`x`, `y`) while the button is held
/// (TRACKBAR_AutoPage): whether it moved.
fn auto_page(cpu: &mut Cpu, hwnd: u32, t: &mut Trackbar, x: i32, y: i32, s: &mut Script) -> bool {
    let l = layout(cpu, hwnd, t);
    let dir = page_direction(&l, t, x, y);
    if dir > 0 && t.flags & FLAG_PAGE_RIGHT != 0 {
        page_down(hwnd, t, s);
    } else if dir < 0 && t.flags & FLAG_PAGE_LEFT != 0 {
        page_up(hwnd, t, s);
    } else {
        return false;
    }
    invalidate(cpu, hwnd, false);
    true
}

/// A tic mark at the value `tic` (TRACKBAR_DrawOneTic).
fn draw_one_tic(cpu: &mut Cpu, hdc: u32, l: &Layout, t: &Trackbar, tic: i32, flags: u32) {
    let vert = flags & TBS_VERT != 0;
    let tics = if vert {
        let off = l.thumb.height() / 2;
        Rect::new(
            l.thumb.left - 2,
            l.channel.top + off + 1,
            l.thumb.right + 2,
            l.channel.bottom - off,
        )
    } else {
        let off = l.thumb.width() / 2;
        Rect::new(
            l.channel.left + off + 1,
            l.thumb.top - 2,
            l.channel.right - off,
            l.thumb.bottom + 2,
        )
    };
    let (mut x, mut y, side) = if flags & (TBS_TOP | TBS_LEFT) != 0 {
        (tics.left, tics.top, -1)
    } else {
        (tics.right, tics.bottom, 1)
    };
    let range = match t.max.wrapping_sub(t.min) {
        r if r <= 0 => 1,
        r => r,
    };
    let mut indent = 0;
    let mut len = 3;
    if flags & TIC_SELECTIONMARK != 0 {
        indent = if flags & TIC_SELECTIONMARKMIN != 0 {
            -1
        } else {
            1
        };
    } else if flags & TIC_EDGE != 0 {
        len += 1;
    }
    let at = |size: i32| (size as i64 * (tic as i64 - t.min as i64) / range as i64) as i32;
    if vert {
        y = tics.top + at(tics.height());
    } else {
        x = tics.left + at(tics.width());
    }
    let (ox, oy) = (x, y);
    let (x1, y1) = if vert {
        (x + len * side, y)
    } else {
        (x, y + len * side)
    };
    gfx::line(cpu, hdc, (x, y), (x1, y1));
    if flags & TIC_SELECTIONMARK != 0 {
        let (x2, y2) = if vert {
            (x1 - side, y1)
        } else {
            (x1, y1 - side)
        };
        let (x3, y3) = if vert {
            (x2, y2 + 2 * indent)
        } else {
            (x2 + 2 * indent, y2)
        };
        gfx::line(cpu, hdc, (x2, y2), (x3, y3));
        gfx::line(cpu, hdc, (x3, y3), (ox, oy));
    }
}

/// A tic mark on the sides the style puts them (TRACKBAR_DrawTic).
fn draw_tic(cpu: &mut Cpu, hdc: u32, l: &Layout, t: &Trackbar, tic: i32, flags: u32) {
    if flags & (TBS_LEFT | TBS_TOP) != 0 || flags & TBS_BOTH != 0 {
        draw_one_tic(cpu, hdc, l, t, tic, flags | TBS_LEFT);
    }
    if flags & (TBS_LEFT | TBS_TOP) == 0 || flags & TBS_BOTH != 0 {
        draw_one_tic(cpu, hdc, l, t, tic, flags & !TBS_LEFT);
    }
}

fn has_selection(t: &Trackbar) -> bool {
    t.sel_min != t.sel_max
}

/// The thumb's outline (TRACKBAR_DrawThumb's points), and how many of
/// its points the black part takes.
fn thumb_points(t: &Trackbar, r: Rect) -> (Vec<(i32, i32)>, usize) {
    if t.style & TBS_BOTH != 0 {
        let p = vec![
            (r.right, r.top),
            (r.right, r.bottom),
            (r.left, r.bottom),
            (r.left, r.top),
            (r.right, r.top),
        ];
        return (p, 3);
    }
    if t.style & TBS_VERT != 0 {
        let depth = r.height() / 2;
        let mid = r.height() / 2 + r.top + 1;
        if t.style & TBS_LEFT != 0 {
            let p = vec![
                (r.right, r.top),
                (r.right, r.bottom),
                (r.left + depth, r.bottom),
                (r.left, mid),
                (r.left + depth, r.top),
                (r.right, r.top),
            ];
            (p, 4)
        } else {
            let p = vec![
                (r.right, mid),
                (r.right - depth, r.bottom),
                (r.left, r.bottom),
                (r.left, r.top),
                (r.right - depth, r.top),
                (r.right, mid),
            ];
            (p, 3)
        }
    } else {
        let depth = r.width() / 2;
        let mid = r.width() / 2 + r.left + 1;
        if t.style & TBS_TOP != 0 {
            let p = vec![
                (mid, r.top),
                (r.right, r.top + depth),
                (r.right, r.bottom),
                (r.left, r.bottom),
                (r.left, r.top + depth),
                (mid, r.top),
            ];
            (p, 4)
        } else {
            let p = vec![
                (r.right, r.top),
                (r.right, r.bottom - depth),
                (mid, r.bottom),
                (r.left, r.bottom - depth),
                (r.left, r.top),
                (r.right, r.top),
            ];
            (p, 3)
        }
    }
}

/// Draw the thumb: filled, its top and left sides in white and the
/// rest in black.
fn draw_thumb(cpu: &mut Cpu, hdc: u32, l: &Layout, t: &Trackbar) {
    let fill = if t.flags & FLAG_DRAG != 0 {
        COLOR_BTNHIGHLIGHT
    } else {
        COLOR_BTNFACE
    };
    let brush = gfx::sys_brush(cpu, fill);
    let (points, black) = thumb_points(t, l.thumb);
    for (y, x0, x1) in gdi32::draw::polygon_spans(std::slice::from_ref(&points), true) {
        gfx::fill(cpu, hdc, Rect::new(x0, y, x1, y + 1), brush);
    }
    let polyline = |cpu: &mut Cpu, pts: &[(i32, i32)]| {
        for w in pts.windows(2) {
            gfx::line(cpu, hdc, w[0], w[1]);
        }
    };
    let black_pen = gfx::stock(cpu, BLACK_PEN);
    let old = gfx::select(cpu, hdc, black_pen);
    polyline(cpu, &points[..black]);
    let white_pen = gfx::stock(cpu, WHITE_PEN);
    gfx::select(cpu, hdc, white_pen);
    polyline(cpu, &points[black - 1..]);
    gfx::select(cpu, hdc, old);
}

/// Paint the control on `hdc` with `brush` behind it
/// (TRACKBAR_Refresh, without the custom draw of its parts).
fn draw(cpu: &mut Cpu, hwnd: u32, hdc: u32, brush: u32, focus_at: u32) {
    let Some(t) = get(cpu, hwnd) else {
        return;
    };
    let l = layout(cpu, hwnd, &t);
    let brush = if brush != 0 {
        brush
    } else {
        gfx::sys_brush(cpu, COLOR_BTNFACE)
    };
    gfx::fill(cpu, hdc, l.client, brush);
    // The channel.
    let inner = gfx::edge(cpu, hdc, l.channel, EDGE_SUNKEN, BF_RECT | BF_ADJUST);
    if t.style & TBS_ENABLESELRANGE != 0 {
        let white = gfx::stock(cpu, WHITE_BRUSH);
        gfx::fill(cpu, hdc, inner, white);
        if has_selection(&t) {
            let b = gfx::sys_brush(cpu, COLOR_HIGHLIGHT);
            gfx::fill(cpu, hdc, l.selection, b);
        }
    }
    // The tics.
    if t.style & TBS_NOTICKS == 0 {
        let color = gfx::sys_color(cpu, COLOR_3DDKSHADOW);
        let pen = gdi32::create_pen(cpu, &[0, 1, color]).unwrap_or(0);
        let old = if pen != 0 {
            gfx::select(cpu, hdc, pen)
        } else {
            0
        };
        let flags = t.style & 0x0F;
        for &tic in &t.tics {
            draw_tic(cpu, hdc, &l, &t, tic, flags);
        }
        draw_tic(cpu, hdc, &l, &t, t.min, flags | TIC_EDGE);
        draw_tic(cpu, hdc, &l, &t, t.max, flags | TIC_EDGE);
        if t.style & TBS_ENABLESELRANGE != 0 && has_selection(&t) {
            draw_tic(cpu, hdc, &l, &t, t.sel_min, flags | TIC_SELECTIONMARKMIN);
            draw_tic(cpu, hdc, &l, &t, t.sel_max, flags | TIC_SELECTIONMARKMAX);
        }
        if pen != 0 {
            gfx::select(cpu, hdc, old);
            gdi32::delete_object(cpu, &[pen]);
        }
    }
    if t.style & TBS_NOTHUMB == 0 {
        draw_thumb(cpu, hdc, &l, &t);
    }
    if t.focused {
        l.client.write(cpu, focus_at);
        user32::paint::draw_focus_rect(cpu, &[hdc, focus_at]);
    }
}

/// The NMCUSTOMDRAW of a stage of the painting.
fn custom_draw(cpu: &mut Cpu, hwnd: u32, hdc: u32, stage: u32) -> Vec<u32> {
    let id = super::id_of(cpu, hwnd);
    let r = super::client(cpu, hwnd);
    vec![
        hwnd,
        id,
        NM_CUSTOMDRAW,
        stage,
        hdc,
        r.left as u32,
        r.top as u32,
        r.right as u32,
        r.bottom as u32,
        0,
        0,
        0,
    ]
}

/// The steps that send the parent `msg` with `wp` and `lp`.
fn send_op(to: u32, msg: u32, wp: Arg, lp: Arg) -> Op {
    Op::Call {
        target: user32::address_of("SendMessageA"),
        args: vec![Arg::V(to), Arg::V(msg), wp, lp],
    }
}

/// The continuations: the stages of painting, and the tooltip made.
pub fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    let hwnd = c.hwnd;
    let notify = get(cpu, hwnd).map_or(0, |t| t.notify);
    let notify = if is_window(cpu, notify) { notify } else { 0 };
    match id {
        T_BEGIN => {
            let hdc = if c.word(0) != 0 { c.word(0) } else { c.last };
            if hdc == 0 {
                return Vec::new();
            }
            if notify == 0 {
                draw(cpu, hwnd, hdc, 0, c.scratch + S_FOCUS);
                return Vec::new();
            }
            vec![
                send_op(notify, WM_CTLCOLORSTATIC, Arg::V(hdc), Arg::V(hwnd)),
                Op::Then {
                    id: T_BRUSH,
                    data: vec![hdc],
                },
            ]
        }
        T_BRUSH => {
            let (hdc, brush) = (c.word(0), c.last);
            let ctl = super::id_of(cpu, hwnd);
            vec![
                Op::Write {
                    at: S_NM,
                    words: custom_draw(cpu, hwnd, hdc, CDDS_PREPAINT),
                },
                send_op(notify, WM_NOTIFY, Arg::V(ctl), Arg::S(S_NM)),
                Op::Then {
                    id: T_PREPAINT,
                    data: vec![hdc, brush],
                },
            ]
        }
        T_PREPAINT => {
            let (hdc, brush, answer) = (c.word(0), c.word(1), c.last);
            if answer & CDRF_SKIPDEFAULT != 0 {
                return Vec::new();
            }
            draw(cpu, hwnd, hdc, brush, c.scratch + S_FOCUS);
            if answer & CDRF_NOTIFYPOSTPAINT == 0 || notify == 0 {
                return Vec::new();
            }
            let ctl = super::id_of(cpu, hwnd);
            vec![
                Op::Write {
                    at: S_NM,
                    words: custom_draw(cpu, hwnd, hdc, CDDS_POSTPAINT),
                },
                send_op(notify, WM_NOTIFY, Arg::V(ctl), Arg::S(S_NM)),
            ]
        }
        T_TOOLTIP => {
            let tip = c.last;
            let Some(mut t) = get(cpu, hwnd) else {
                return Vec::new();
            };
            t.tooltip = tip;
            put(cpu, hwnd, t);
            if tip == 0 {
                return Vec::new();
            }
            let flags = TTF_IDISHWND | TTF_TRACK | TTF_ABSOLUTE;
            vec![
                Op::Write {
                    at: S_TOOL,
                    words: vec![44, flags, hwnd, hwnd, 0, 0, 0, 0, 0, 0, 0],
                },
                send_op(tip, TTM_ADDTOOLA, Arg::V(0), Arg::S(S_TOOL)),
            ]
        }
        _ => Vec::new(),
    }
}

/// A script that paints the control: through BeginPaint and EndPaint,
/// or on `hdc`.
fn paint_script(hwnd: u32, hdc: u32) -> Script {
    let mut s = Script::new(hwnd);
    s.alloc(S_PAINT);
    if hdc != 0 {
        s.then(T_BEGIN, &[hdc]).ret(0);
        return s;
    }
    let begin = user32::address_of("BeginPaint");
    let end = user32::address_of("EndPaint");
    s.call(begin, &[Arg::V(hwnd), Arg::S(S_PS)])
        .then(T_BEGIN, &[0])
        .call(end, &[Arg::V(hwnd), Arg::S(S_PS)])
        .ret(0);
    s
}

/// Repaint the whole control at once and return `result`.
fn redraw(cpu: &mut Cpu, hwnd: u32, result: u32) -> Option<u32> {
    invalidate(cpu, hwnd, false);
    let mut s = Script::new(hwnd);
    s.user("UpdateWindow", &[hwnd]).ret(result);
    script::run(cpu, TRACKBAR, s)
}

/// Add moving the buddies to the control's ends to `s`
/// (TRACKBAR_AlignBuddies).
fn align_buddies(cpu: &mut Cpu, hwnd: u32, t: &Trackbar, s: &mut Script) {
    let parent = super::parent(cpu, hwnd);
    let st = user32::user(cpu);
    let origin = if parent != 0 && st.is_window(parent) {
        st.client_origin(parent)
    } else {
        (0, 0)
    };
    let own = st.screen_rect(hwnd).offset(-origin.0, -origin.1);
    let la = st.is_window(t.buddy_la).then(|| st.screen_rect(t.buddy_la));
    let rb = st.is_window(t.buddy_rb).then(|| st.screen_rect(t.buddy_rb));
    let ch = channel(super::client(cpu, hwnd), t);
    let vert = t.style & TBS_VERT != 0;
    let flags = 0x0004 | 0x0001; // SWP_NOZORDER | SWP_NOSIZE
    if let Some(b) = la {
        let (x, y) = if vert {
            (
                (ch.right + ch.left) / 2 - b.width() / 2 + own.left,
                own.top - b.height(),
            )
        } else {
            (
                own.left - b.width(),
                (ch.bottom + ch.top) / 2 - b.height() / 2 + own.top,
            )
        };
        s.user(
            "SetWindowPos",
            &[t.buddy_la, 0, x as u32, y as u32, 0, 0, flags],
        );
    }
    if let Some(b) = rb {
        let (x, y) = if vert {
            (
                (ch.right + ch.left) / 2 - b.width() / 2 + own.left,
                own.bottom,
            )
        } else {
            (
                own.right,
                (ch.bottom + ch.top) / 2 - b.height() / 2 + own.top,
            )
        };
        s.user(
            "SetWindowPos",
            &[t.buddy_rb, 0, x as u32, y as u32, 0, 0, flags],
        );
    }
}

/// The trackbar's window procedure.
pub fn proc_(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    if msg == WM_CREATE {
        return create(cpu, hwnd, lp);
    }
    let Some(mut t) = get(cpu, hwnd) else {
        return defwnd::def_window_proc(cpu, args);
    };
    let result = match msg {
        TBM_CLEARSEL => {
            t.sel_min = 0;
            t.sel_max = 0;
            put(cpu, hwnd, t);
            if wp != 0 {
                return redraw(cpu, hwnd, 0);
            }
            0
        }
        TBM_CLEARTICS => {
            t.tics.clear();
            put(cpu, hwnd, t);
            if wp != 0 {
                return redraw(cpu, hwnd, 0);
            }
            0
        }
        TBM_GETBUDDY => {
            if wp != 0 {
                t.buddy_la
            } else {
                t.buddy_rb
            }
        }
        TBM_GETCHANNELRECT => {
            if lp != 0 {
                layout(cpu, hwnd, &t).channel.write(cpu, lp);
                if cpu.bus.guest_faulted() {
                    return None;
                }
            }
            0
        }
        TBM_GETLINESIZE => t.line as u32,
        TBM_GETNUMTICS => {
            if t.style & TBS_NOTICKS != 0 {
                0
            } else {
                t.tics.len() as u32 + 2
            }
        }
        TBM_GETPAGESIZE => t.page as u32,
        TBM_GETPOS => t.pos as u32,
        TBM_GETPTICS => return get_ptics(cpu, hwnd, t),
        TBM_GETRANGEMAX => t.max as u32,
        TBM_GETRANGEMIN => t.min as u32,
        TBM_GETSELEND => t.sel_max as u32,
        TBM_GETSELSTART => t.sel_min as u32,
        TBM_GETTHUMBLENGTH => t.thumb_len as u32,
        TBM_GETTHUMBRECT => {
            if lp == 0 {
                return Some(0);
            }
            layout(cpu, hwnd, &t).thumb.write(cpu, lp);
            if cpu.bus.guest_faulted() {
                return None;
            }
            1
        }
        TBM_GETTIC => {
            let i = wp as i32;
            if i < 0 || i as usize >= t.tics.len() {
                return Some(u32::MAX);
            }
            // Sorted as Windows sorts them, as unsigned numbers.
            t.tics.sort_by_key(|&v| v as u32);
            let v = t.tics[i as usize];
            put(cpu, hwnd, t);
            v as u32
        }
        TBM_GETTICPOS => {
            let i = wp as i32;
            if i < 0 || i as usize >= t.tics.len() {
                return Some(u32::MAX);
            }
            t.tics.sort_by_key(|&v| v as u32);
            let tic = t.tics[i as usize];
            put(cpu, hwnd, t.clone());
            let l = layout(cpu, hwnd, &t);
            let range = match t.max.wrapping_sub(t.min) {
                r if r <= 0 => 1,
                r => r,
            };
            let off = l.thumb.width() / 2;
            let width = l.channel.width() - off * 2;
            let along = (width as i64 * (tic as i64 - t.min as i64) / range as i64) as i32;
            (l.channel.left + off + along) as u32
        }
        TBM_GETTOOLTIPS => t.tooltip,
        TBM_GETUNICODEFORMAT => t.unicode as u32,
        TBM_SETBUDDY => {
            let old = if wp != 0 {
                std::mem::replace(&mut t.buddy_la, lp)
            } else {
                std::mem::replace(&mut t.buddy_rb, lp)
            };
            put(cpu, hwnd, t.clone());
            let mut s = Script::new(hwnd);
            align_buddies(cpu, hwnd, &t, &mut s);
            invalidate(cpu, hwnd, false);
            s.user("UpdateWindow", &[hwnd]).ret(old);
            return script::run(cpu, TRACKBAR, s);
        }
        TBM_SETLINESIZE => {
            let old = std::mem::replace(&mut t.line, lp as i32);
            put(cpu, hwnd, t);
            old as u32
        }
        TBM_SETPAGESIZE => {
            let new = if lp as i32 == -1 {
                TB_DEFAULTPAGESIZE
            } else {
                lp as i32
            };
            let old = std::mem::replace(&mut t.page, new);
            put(cpu, hwnd, t);
            old as u32
        }
        TBM_SETPOS => {
            let old = t.pos;
            t.pos = (lp as i32).max(t.min).min(t.max);
            put(cpu, hwnd, t);
            if wp != 0 && old != lp as i32 {
                return redraw(cpu, hwnd, 0);
            }
            0
        }
        TBM_SETRANGE | TBM_SETRANGEMIN | TBM_SETRANGEMAX => {
            match msg {
                TBM_SETRANGE => {
                    t.min = lp as u16 as i16 as i32;
                    t.max = (lp >> 16) as u16 as i16 as i32;
                }
                TBM_SETRANGEMIN => t.min = lp as i32,
                _ => t.max = lp as i32,
            }
            if msg != TBM_SETRANGEMAX && t.pos < t.min {
                t.pos = t.min;
            }
            if msg != TBM_SETRANGEMIN && t.pos > t.max {
                t.pos = t.max;
            }
            t.page = (t.max.wrapping_sub(t.min) / 5).max(1);
            if t.max.wrapping_sub(t.min) / 5 == 0 {
                t.page = 1;
            }
            put(cpu, hwnd, t);
            if wp != 0 {
                return redraw(cpu, hwnd, 0);
            }
            0
        }
        TBM_SETSEL | TBM_SETSELSTART | TBM_SETSELEND => {
            if t.style & TBS_ENABLESELRANGE == 0 {
                if msg != TBM_SETSELEND {
                    t.sel_min = 0;
                }
                if msg != TBM_SETSELSTART {
                    t.sel_max = 0;
                }
                put(cpu, hwnd, t);
                return Some(0);
            }
            match msg {
                TBM_SETSEL => {
                    t.sel_min = (lp as u16 as i16 as i32).max(t.min);
                    t.sel_max = ((lp >> 16) as u16 as i16 as i32).min(t.max);
                }
                TBM_SETSELSTART => t.sel_min = (lp as i32).max(t.min),
                _ => t.sel_max = (lp as i32).min(t.max),
            }
            put(cpu, hwnd, t);
            if wp != 0 {
                return redraw(cpu, hwnd, 0);
            }
            0
        }
        TBM_SETTHUMBLENGTH => {
            if t.style & TBS_FIXEDLENGTH == 0 {
                return Some(0);
            }
            t.thumb_len = wp as i32;
            put(cpu, hwnd, t);
            return redraw(cpu, hwnd, 0);
        }
        TBM_SETTIC => {
            let v = lp as i32;
            if v < t.min || v > t.max {
                return Some(0);
            }
            t.tics.push(v);
            put(cpu, hwnd, t);
            invalidate(cpu, hwnd, false);
            1
        }
        TBM_SETTICFREQ => {
            if t.style & TBS_AUTOTICKS == 0 {
                return Some(0);
            }
            t.tic_freq = wp & 0xFFFF;
            recalculate_tics(&mut t);
            put(cpu, hwnd, t);
            return redraw(cpu, hwnd, 0);
        }
        TBM_SETTIPSIDE => {
            let old = std::mem::replace(&mut t.tip_side, wp as i32);
            put(cpu, hwnd, t);
            old as u32
        }
        TBM_SETTOOLTIPS => {
            t.tooltip = wp;
            put(cpu, hwnd, t);
            0
        }
        TBM_SETUNICODEFORMAT => {
            let old = std::mem::replace(&mut t.unicode, wp != 0);
            put(cpu, hwnd, t);
            old as u32
        }
        WM_CAPTURECHANGED => {
            let mut s = Script::new(hwnd);
            notify_scroll(hwnd, &t, &mut s, TB_ENDTRACK);
            s.ret(0);
            return script::run(cpu, TRACKBAR, s);
        }
        WM_DESTROY => {
            state(cpu).trackbar.bars.remove(&hwnd);
            if t.ptics != 0 {
                let heap = misc::process_heap(cpu);
                misc::heap_free(cpu, heap, t.ptics);
            }
            if is_window(cpu, t.tooltip) {
                let mut s = Script::new(hwnd);
                s.user("DestroyWindow", &[t.tooltip]).ret(0);
                return script::run(cpu, TRACKBAR, s);
            }
            0
        }
        WM_ERASEBKGND => 0,
        WM_GETDLGCODE => DLGC_WANTARROWS,
        WM_KEYDOWN => return key_down(cpu, hwnd, t, wp),
        WM_KEYUP => {
            let mut s = Script::new(hwnd);
            if matches!(
                wp,
                VK_LEFT | VK_UP | VK_RIGHT | VK_DOWN | VK_NEXT | VK_PRIOR | VK_HOME | VK_END
            ) {
                notify_scroll(hwnd, &t, &mut s, TB_ENDTRACK);
            }
            s.ret(1);
            return script::run(cpu, TRACKBAR, s);
        }
        WM_KILLFOCUS | WM_SETFOCUS => {
            t.focused = msg == WM_SETFOCUS;
            put(cpu, hwnd, t);
            invalidate(cpu, hwnd, false);
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = user32::point_of(lp);
            return button_down(cpu, hwnd, t, x, y);
        }
        WM_LBUTTONUP => return button_up(cpu, hwnd, t),
        WM_MOUSEMOVE => {
            let (x, y) = user32::point_of(lp);
            return mouse_move(cpu, hwnd, t, x, y);
        }
        WM_PAINT | WM_PRINTCLIENT => return script::run(cpu, TRACKBAR, paint_script(hwnd, wp)),
        WM_SIZE => {
            initialize_thumb(cpu, hwnd, &mut t);
            put(cpu, hwnd, t.clone());
            let mut s = Script::new(hwnd);
            align_buddies(cpu, hwnd, &t, &mut s);
            s.ret(0);
            return script::run(cpu, TRACKBAR, s);
        }
        WM_STYLECHANGED => {
            if wp == GWL_STYLE {
                let new = cpu.bus.guest_read_32(lp.wrapping_add(4));
                if cpu.bus.guest_faulted() {
                    return None;
                }
                t.style = new;
                put(cpu, hwnd, t);
            }
            0
        }
        WM_TIMER => {
            if t.flags & FLAG_PAGE == 0 {
                return Some(0);
            }
            let (x, y) = t.mouse;
            let mut s = Script::new(hwnd);
            auto_page(cpu, hwnd, &mut t, x, y, &mut s);
            put(cpu, hwnd, t);
            s.ret(0);
            return script::run(cpu, TRACKBAR, s);
        }
        WM_WININICHANGE => {
            initialize_thumb(cpu, hwnd, &mut t);
            put(cpu, hwnd, t);
            0
        }
        _ => return defwnd::def_window_proc(cpu, args),
    };
    Some(result)
}

/// TBM_GETPTICS: the tics, in a block of the process's heap the control
/// keeps for it; NULL when there are none.
fn get_ptics(cpu: &mut Cpu, hwnd: u32, mut t: Trackbar) -> Option<u32> {
    if t.tics.is_empty() {
        return Some(0);
    }
    let n = t.tics.len() as u32;
    if t.ptics == 0 || t.ptics_room < n {
        let heap = misc::process_heap(cpu);
        if t.ptics != 0 {
            misc::heap_free(cpu, heap, t.ptics);
        }
        t.ptics = misc::heap_alloc(cpu, heap, 4 * n);
        t.ptics_room = if t.ptics != 0 { n } else { 0 };
    }
    if t.ptics != 0 {
        let words: Vec<u32> = t.tics.iter().map(|&v| v as u32).collect();
        write_words(cpu, t.ptics, &words);
    }
    let at = t.ptics;
    put(cpu, hwnd, t);
    Some(at)
}

/// WM_KEYDOWN (TRACKBAR_KeyDown).
fn key_down(cpu: &mut Cpu, hwnd: u32, mut t: Trackbar, key: u32) -> Option<u32> {
    let down_is_left = t.style & TBS_DOWNISLEFT != 0;
    let vert = t.style & TBS_VERT != 0;
    let before = t.pos;
    let mut s = Script::new(hwnd);
    match key {
        VK_UP | VK_DOWN => {
            if (key == VK_UP) == (!vert && down_is_left) {
                line_down(hwnd, &mut t, &mut s);
            } else {
                line_up(hwnd, &mut t, &mut s);
            }
        }
        VK_LEFT | VK_RIGHT => {
            if (key == VK_LEFT) == (vert && down_is_left) {
                line_down(hwnd, &mut t, &mut s);
            } else {
                line_up(hwnd, &mut t, &mut s);
            }
        }
        VK_NEXT | VK_PRIOR => {
            if (key == VK_NEXT) == (!vert && down_is_left) {
                page_up(hwnd, &mut t, &mut s);
            } else {
                page_down(hwnd, &mut t, &mut s);
            }
        }
        VK_HOME => {
            if t.pos == t.min {
                return Some(0);
            }
            t.pos = t.min;
            notify_scroll(hwnd, &t, &mut s, TB_TOP);
        }
        VK_END => {
            if t.pos == t.max {
                return Some(0);
            }
            t.pos = t.max;
            notify_scroll(hwnd, &t, &mut s, TB_BOTTOM);
        }
        _ => {}
    }
    let moved = before != t.pos;
    put(cpu, hwnd, t);
    if moved {
        invalidate(cpu, hwnd, false);
    }
    s.ret(1);
    script::run(cpu, TRACKBAR, s)
}

/// WM_LBUTTONDOWN (TRACKBAR_LButtonDown): drag the thumb, or page
/// towards the click and on while the button is held.
fn button_down(cpu: &mut Cpu, hwnd: u32, mut t: Trackbar, x: i32, y: i32) -> Option<u32> {
    let mut s = Script::new(hwnd);
    s.user("SetFocus", &[hwnd]);
    t.mouse = (x, y);
    let l = layout(cpu, hwnd, &t);
    if l.thumb.contains(x, y) {
        t.flags |= FLAG_DRAG;
        s.user("SetCapture", &[hwnd]);
        invalidate(cpu, hwnd, false);
    } else {
        let dir = page_direction(&l, &t, x, y);
        if dir != 0 {
            t.flags |= if dir < 0 {
                FLAG_PAGE_LEFT
            } else {
                FLAG_PAGE_RIGHT
            };
            auto_page(cpu, hwnd, &mut t, x, y, &mut s);
            s.user("SetCapture", &[hwnd]);
            user32::message::set_timer(cpu, &[hwnd, TB_REFRESH_TIMER, TB_REFRESH_DELAY, 0]);
        }
    }
    put(cpu, hwnd, t);
    s.ret(0);
    script::run(cpu, TRACKBAR, s)
}

/// WM_LBUTTONUP (TRACKBAR_LButtonUp): the end of a drag or of paging.
fn button_up(cpu: &mut Cpu, hwnd: u32, mut t: Trackbar) -> Option<u32> {
    let mut s = Script::new(hwnd);
    let release = |cpu: &mut Cpu, t: &Trackbar, s: &mut Script| {
        s.user("ReleaseCapture", &[]);
        if is_window(cpu, t.notify) {
            super::notify(cpu, s, hwnd, t.notify, NM_RELEASEDCAPTURE, &[]);
        }
    };
    if t.flags & FLAG_DRAG != 0 {
        let pos = (t.pos as u32) << 16;
        notify_scroll(hwnd, &t, &mut s, TB_THUMBPOSITION | pos);
        notify_scroll(hwnd, &t, &mut s, TB_ENDTRACK);
        t.flags &= !FLAG_DRAG;
        release(cpu, &t, &mut s);
        invalidate(cpu, hwnd, false);
    }
    if t.flags & FLAG_PAGE != 0 {
        user32::message::kill_timer(cpu, &[hwnd, TB_REFRESH_TIMER]);
        t.flags &= !FLAG_PAGE;
        notify_scroll(hwnd, &t, &mut s, TB_ENDTRACK);
        release(cpu, &t, &mut s);
    }
    put(cpu, hwnd, t);
    s.ret(0);
    script::run(cpu, TRACKBAR, s)
}

/// WM_MOUSEMOVE (TRACKBAR_MouseMove): the thumb follows a drag.
fn mouse_move(cpu: &mut Cpu, hwnd: u32, mut t: Trackbar, x: i32, y: i32) -> Option<u32> {
    t.mouse = (x, y);
    let mut s = Script::new(hwnd);
    if t.flags & FLAG_PAGE != 0 {
        auto_page(cpu, hwnd, &mut t, x, y, &mut s);
    } else if t.flags & FLAG_DRAG != 0 {
        let l = layout(cpu, hwnd, &t);
        let place = if t.style & TBS_VERT != 0 { y } else { x };
        let pos = place_to_position(&l, &t, place);
        if pos != t.pos {
            t.pos = pos;
            let code = TB_THUMBTRACK | (pos as u32) << 16;
            notify_scroll(hwnd, &t, &mut s, code);
            invalidate(cpu, hwnd, false);
            s.user("UpdateWindow", &[hwnd]);
        }
    }
    put(cpu, hwnd, t);
    s.ret(1);
    script::run(cpu, TRACKBAR, s)
}

/// WM_CREATE (TRACKBAR_Create): the range 0 to 100, the thumb's length
/// for the size, the parent asked for its notification format, and a
/// tooltip with TBS_TOOLTIPS.
fn create(cpu: &mut Cpu, hwnd: u32, cs: u32) -> Option<u32> {
    let w = read_words(cpu, cs, 12);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (parent, style) = (w[3], w[8]);
    let mut t = Trackbar {
        style,
        max: 100,
        line: 1,
        page: TB_DEFAULTPAGESIZE,
        tip_side: TBTS_TOP,
        tic_freq: 1,
        notify: parent,
        ..Trackbar::default()
    };
    initialize_thumb(cpu, hwnd, &mut t);
    put(cpu, hwnd, t);
    let mut s = Script::new(hwnd);
    s.alloc(S_CREATE);
    if is_window(cpu, parent) {
        s.send(parent, WM_NOTIFYFORMAT, Arg::V(hwnd), Arg::V(NF_QUERY));
    }
    if style & TBS_TOOLTIPS != 0 {
        let class = s.string(b"tooltips_class32");
        let d = Arg::V(CW_USEDEFAULT);
        s.call(
            user32::address_of("CreateWindowExA"),
            &[
                Arg::V(0),
                Arg::S(class),
                Arg::V(0),
                Arg::V(WS_POPUP),
                d,
                d,
                d,
                d,
                Arg::V(hwnd),
                Arg::V(0),
                Arg::V(0),
                Arg::V(0),
            ],
        )
        .then(T_TOOLTIP, &[]);
    }
    s.ret(0);
    script::run(cpu, TRACKBAR, s)
}
