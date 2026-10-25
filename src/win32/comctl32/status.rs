//! The status bar (msctls_statusbar32): a strip along the bottom of its
//! parent, split into parts that each show a text (left, centred and
//! right, split by tabs), an icon, or what the parent draws itself
//! (SBT_OWNERDRAW), in a sunken, raised or no border. In simple mode
//! (SB_SIMPLE) it shows one part of its own instead. It keeps itself
//! at the bottom of its parent, as wide as it, and as tall as its font
//! needs, on WM_SIZE, and draws a size grip with SBARS_SIZEGRIP.
//!
//! The functions of the module: CreateStatusWindowA, DrawStatusTextA,
//! and MenuHelp, ShowHideMenuCtl and GetEffectiveClientRect, which
//! programs use with a status bar.
//!
//! Ported from Wine's status.c and the functions of its commctrl.c:
//!
//! Copyright 1996 Bruce Milner
//! Copyright 1997 Dimitrie O. Paun
//! Copyright 1998, 1999, 2000 Eric Kohl
//! Copyright 2002 Dimitrie O. Paun
//!
//! This library is free software; you can redistribute it and/or modify
//! it under the terms of the GNU Lesser General Public License as
//! published by the Free Software Foundation; either version 2.1 of the
//! License, or (at your option) any later version.

use super::gfx::{self, CLR_DEFAULT, COLOR_BTNFACE, COLOR_BTNHIGHLIGHT, COLOR_BTNSHADOW};
use super::script::{self, Arg, Ctx, Op, Script};
use super::{DRAW, STATUS, invalidate, state};
use crate::cpu::Cpu;
use crate::win32::gdi32::{self, Rect};
use crate::win32::user32::message::{
    WM_CREATE, WM_DESTROY, WM_DRAWITEM, WM_GETFONT, WM_GETTEXT, WM_GETTEXTLENGTH, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NOTIFY, WM_NOTIFYFORMAT,
    WM_PAINT, WM_SETFONT, WM_SETTEXT, WM_SIZE, WM_SYSCOLORCHANGE,
};
use crate::win32::user32::window::{SWP_HIDEWINDOW, SWP_SHOWWINDOW, WS_BORDER};
use crate::win32::user32::{self, defwnd, read_words, sysparams, write_words};
use std::collections::BTreeMap;

const WM_PRINTCLIENT: u32 = 0x0318;
const WM_LBUTTONDBLCLK: u32 = 0x0203;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_RBUTTONDBLCLK: u32 = 0x0206;
const WM_MENUSELECT: u32 = 0x011F;
const NF_QUERY: u32 = 3;
const NF_REQUERY: u32 = 4;
const NFR_ANSI: u32 = 1;
const NFR_UNICODE: u32 = 2;

const SB_SETTEXTA: u32 = 0x0401;
const SB_GETTEXTA: u32 = 0x0402;
const SB_GETTEXTLENGTHA: u32 = 0x0403;
const SB_SETPARTS: u32 = 0x0404;
const SB_SETBORDERS: u32 = 0x0405;
const SB_GETPARTS: u32 = 0x0406;
const SB_GETBORDERS: u32 = 0x0407;
const SB_SETMINHEIGHT: u32 = 0x0408;
const SB_SIMPLE: u32 = 0x0409;
const SB_GETRECT: u32 = 0x040A;
const SB_SETTEXTW: u32 = 0x040B;
const SB_GETTEXTLENGTHW: u32 = 0x040C;
const SB_GETTEXTW: u32 = 0x040D;
const SB_ISSIMPLE: u32 = 0x040E;
const SB_SETICON: u32 = 0x040F;
const SB_SETTIPTEXTA: u32 = 0x0410;
const SB_SETTIPTEXTW: u32 = 0x0411;
const SB_GETTIPTEXTA: u32 = 0x0412;
const SB_GETTIPTEXTW: u32 = 0x0413;
const SB_GETICON: u32 = 0x0414;
const SB_SETBKCOLOR: u32 = 0x2001;
const SB_SETUNICODEFORMAT: u32 = 0x2005;
const SB_GETUNICODEFORMAT: u32 = 0x2006;

const SBT_NOBORDERS: u32 = 0x0100;
const SBT_POPOUT: u32 = 0x0200;
const SBT_OWNERDRAW: u32 = 0x1000;
const SBARS_SIZEGRIP: u32 = 0x0100;
const CCS_NORESIZE: u32 = 0x0004;
/// The part SB_SETTEXT's wParam names for the simple mode's text.
const SB_SIMPLEID: u32 = 0x00FF;

const NM_CLICK: u32 = (-2i32) as u32;
const NM_DBLCLK: u32 = (-3i32) as u32;
const NM_RCLICK: u32 = (-5i32) as u32;
const NM_RDBLCLK: u32 = (-6i32) as u32;
const SBN_SIMPLEMODECHANGE: u32 = (-880i32) as u32;

const HTBOTTOMRIGHT: u32 = 17;
const SM_CYBORDER: u32 = 6;
const SM_CYSIZE: u32 = 31;
const SIZE_RESTORED: u32 = 0;
const SIZE_MAXIMIZED: u32 = 2;
const CW_USEDEFAULT: u32 = 0x8000_0000;
const MF_POPUP: u32 = 0x0010;
const MF_BYCOMMAND: u32 = 0;
const MF_CHECKED: u32 = 0x0008;
const MF_UNCHECKED: u32 = 0;

const BDR_RAISEDOUTER: u32 = 0x0001;
const BDR_SUNKENOUTER: u32 = 0x0002;
const BF_RECT: u32 = 0x000F;
const BF_ADJUST: u32 = 0x2000;
const DT_LEFT: u32 = 0x0000;
const DT_CENTER: u32 = 0x0001;
const DT_RIGHT: u32 = 0x0002;
const DT_VCENTER: u32 = 0x0004;
const DT_SINGLELINE: u32 = 0x0020;
const DT_NOPREFIX: u32 = 0x0800;
const TRANSPARENT: u32 = 1;
const DI_NORMAL: u32 = 3;

const HORZ_BORDER: i32 = 0;
const VERT_BORDER: i32 = 2;
const HORZ_GAP: i32 = 2;

/// The scratch space of a painting script: a DRAWITEMSTRUCT, then the
/// PAINTSTRUCT.
const S_DIS: u32 = 0;
const S_PS: u32 = 48;
const S_PAINT: u32 = 112;

// The continuations.
const T_MOUSE: u16 = 1;
const T_FORMAT: u16 = 2;

/// A part of the bar.
#[derive(Clone, Debug, Default)]
pub struct Part {
    /// Its right edge, or -1 for the rest of the bar.
    pub x: i32,
    /// The SBT_ flags its text was set with.
    pub style: u32,
    /// Its text, with what isn't printable made spaces.
    pub text: Vec<u8>,
    /// Whether it has a text (SB_SETTEXT with NULL takes it away).
    pub has_text: bool,
    /// What SBT_OWNERDRAW gave as its text: the parent's data.
    pub data: u32,
    pub icon: u32,
}

crate::state_fields!(Part {
    x,
    style,
    text,
    has_text,
    data,
    icon
});

/// What a status bar keeps.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub notify: u32,
    pub parts: Vec<Part>,
    /// The simple mode's part.
    pub simple_part: Part,
    pub simple: bool,
    pub height: i32,
    pub min_height: i32,
    pub font: u32,
    pub bk_color: u32,
    pub unicode: bool,
    pub h_border: i32,
    pub v_border: i32,
    pub gap: i32,
}

crate::state_fields!(Status {
    notify,
    parts,
    simple_part,
    simple,
    height,
    min_height,
    font,
    bk_color,
    unicode,
    h_border,
    v_border,
    gap
});

/// What the module keeps, in COMCTL32's `State`: the status bars, by
/// window handle.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub bars: BTreeMap<u32, Status>,
}

crate::state_fields!(State { bars });

fn get(cpu: &mut Cpu, hwnd: u32) -> Option<Status> {
    state(cpu).status.bars.get(&hwnd).cloned()
}

fn put(cpu: &mut Cpu, hwnd: u32, s: Status) {
    state(cpu).status.bars.insert(hwnd, s);
}

fn is_window(cpu: &mut Cpu, hwnd: u32) -> bool {
    hwnd != 0 && user32::user(cpu).window(hwnd).is_some()
}

/// The font the bar draws with: its own, or the default one.
fn font_of(s: &Status, cpu: &mut Cpu) -> u32 {
    if s.font != 0 {
        s.font
    } else {
        gfx::default_font(cpu)
    }
}

/// The height and internal leading of `font` on the screen.
fn font_metrics(cpu: &mut Cpu, font: u32) -> (i32, i32) {
    let hdc = user32::paint::get_dc(cpu, &[0]).unwrap_or(0);
    if hdc == 0 {
        return (13, 2);
    }
    let old = gfx::select(cpu, hdc, font);
    let m = gdi32::font::font_of(cpu, hdc).map_or((13, 2), |(_, f)| (f.height, f.internal_leading));
    if old != 0 {
        gfx::select(cpu, hdc, old);
    }
    user32::paint::release_dc(cpu, &[0, hdc]);
    m
}

/// The bar's height for its font (STATUSBAR_ComputeHeight).
fn compute_height(cpu: &mut Cpu, s: &Status) -> i32 {
    let font = font_of(s, cpu);
    let (height, leading) = font_metrics(cpu, font);
    let margin = if leading != 0 { leading } else { 2 };
    let border = sysparams::metric(SM_CYBORDER);
    (height + margin + 2 * border)
        .max(s.min_height)
        .saturating_add(s.v_border)
}

/// SM_CYSIZE made even, the least the bar's height may be.
fn min_size() -> i32 {
    sysparams::metric(SM_CYSIZE) & !1
}

/// The rectangles of the simple part and the parts, from the client
/// area (STATUSBAR_SetPartBounds).
fn bounds(cpu: &mut Cpu, hwnd: u32, s: &Status) -> (Rect, Vec<Rect>) {
    let mut r = super::client(cpu, hwnd);
    r.left = r.left.saturating_add(s.h_border);
    r.top = r.top.saturating_add(s.v_border);
    let mut parts: Vec<Rect> = Vec::with_capacity(s.parts.len());
    for (i, p) in s.parts.iter().enumerate() {
        let left = if i == 0 {
            0
        } else {
            parts.last().map_or(0, |b| b.right).saturating_add(s.gap)
        };
        let right = if p.x == -1 { r.right } else { p.x };
        parts.push(Rect::new(left, r.top, right, r.bottom));
    }
    (r, parts)
}

/// `text` as status bars keep it: what isn't printable a space.
fn printable(text: &[u8]) -> Vec<u8> {
    text.iter()
        .map(|&b| {
            if (b < 0x20 && b != b'\t') || b == 0x7F {
                b' '
            } else {
                b
            }
        })
        .collect()
}

/// The edge an SBT_ style draws a part in.
fn border_of(style: u32) -> u32 {
    if style & SBT_POPOUT != 0 {
        BDR_RAISEDOUTER
    } else if style & SBT_NOBORDERS != 0 {
        0
    } else {
        BDR_SUNKENOUTER
    }
}

/// `r` inside the edge `border` drawn around it.
fn draw_border(cpu: &mut Cpu, hdc: u32, r: Rect, border: u32) -> Rect {
    if border != 0 {
        gfx::edge(cpu, hdc, r, border, BF_RECT | BF_ADJUST)
    } else {
        r
    }
}

/// DrawStatusText's drawing of `text` in `r` with the SBT_ flags
/// `style`: the border, then the text's tab-separated pieces at the
/// left, the centre and the right.
fn draw_status(cpu: &mut Cpu, hdc: u32, r: Rect, text: Option<&[u8]>, style: u32) {
    let mut r = draw_border(cpu, hdc, r, border_of(style));
    let Some(text) = text else {
        return;
    };
    let old = gfx::set_bk_mode(cpu, hdc, TRANSPARENT);
    r.left += 3;
    let flags = DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX;
    let mut align = DT_LEFT;
    let mut start = 0;
    let mut pieces = Vec::new();
    for (i, &b) in text.iter().enumerate() {
        if b != b'\t' {
            continue;
        }
        if i > start {
            pieces.push((start, i, align));
        }
        start = i + 1;
        if align == DT_RIGHT {
            start = text.len();
            break;
        }
        align = if align == DT_LEFT {
            DT_CENTER
        } else {
            DT_RIGHT
        };
    }
    if start < text.len() {
        pieces.push((start, text.len(), align));
    }
    for (a, b, align) in pieces {
        let mut piece = r;
        gfx::draw_text(cpu, hdc, &text[a..b], &mut piece, align | flags);
    }
    gfx::set_bk_mode(cpu, hdc, old);
}

/// The size grip in the bottom right corner of `r`.
fn draw_size_grip(cpu: &mut Cpu, hdc: u32, r: Rect) {
    let pen = |cpu: &mut Cpu, index: u32| {
        let color = gfx::sys_color(cpu, index);
        gdi32::create_pen(cpu, &[0, 1, color]).unwrap_or(0)
    };
    let (mut x, mut y) = (r.right - 1, r.bottom - 1);
    let face = pen(cpu, COLOR_BTNFACE);
    let old = gfx::select(cpu, hdc, face);
    gfx::line(cpu, hdc, (x - 12, y), (x, y));
    gfx::line(cpu, hdc, (x, y), (x, y - 13));
    x -= 1;
    y -= 1;
    let shadow = pen(cpu, COLOR_BTNSHADOW);
    gfx::select(cpu, hdc, shadow);
    for i in (1..11).step_by(4) {
        gfx::line(cpu, hdc, (x - i, y), (x + 1, y - i - 1));
        gfx::line(cpu, hdc, (x - i - 1, y), (x + 1, y - i - 2));
    }
    let light = pen(cpu, COLOR_BTNHIGHLIGHT);
    gfx::select(cpu, hdc, light);
    for i in (3..13).step_by(4) {
        gfx::line(cpu, hdc, (x - i, y), (x + 1, y - i - 1));
    }
    gfx::select(cpu, hdc, old);
    for p in [face, shadow, light] {
        gdi32::delete_object(cpu, &[p]);
    }
}

/// Paint the bar on `hdc` (STATUSBAR_Refresh): the steps that send
/// owner-drawn parts' WM_DRAWITEM to the parent, with their
/// DRAWITEMSTRUCT at `S_DIS` of the script's scratch space.
pub fn draw(cpu: &mut Cpu, hwnd: u32, hdc: u32) -> Vec<Op> {
    let mut ops = Vec::new();
    let Some(s) = get(cpu, hwnd) else {
        return ops;
    };
    if user32::window::is_window_visible(cpu, &[hwnd]) != Some(1) {
        return ops;
    }
    let client = super::client(cpu, hwnd);
    let bk = if s.bk_color != CLR_DEFAULT {
        gdi32::create_solid_brush(cpu, &[s.bk_color]).unwrap_or(0)
    } else {
        gfx::sys_brush(cpu, COLOR_BTNFACE)
    };
    gfx::fill(cpu, hdc, client, bk);
    let font = font_of(&s, cpu);
    let old_font = gfx::select(cpu, hdc, font);
    let (simple, rects) = bounds(cpu, hwnd, &s);
    let list: Vec<(usize, &Part, Rect)> = if s.simple {
        vec![(0, &s.simple_part, simple)]
    } else {
        s.parts
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p, rects[i]))
            .collect()
    };
    let id = super::id_of(cpu, hwnd);
    for (i, part, r) in list {
        if r.right < r.left {
            continue;
        }
        gfx::fill(cpu, hdc, r, bk);
        let mut r = draw_border(cpu, hdc, r, border_of(part.style));
        if part.style & SBT_OWNERDRAW != 0 {
            if is_window(cpu, s.notify) {
                let dis = vec![
                    0,
                    id,
                    i as u32,
                    0,
                    0,
                    hwnd,
                    hdc,
                    r.left as u32,
                    r.top as u32,
                    r.right as u32,
                    r.bottom as u32,
                    part.data,
                ];
                ops.push(Op::Write {
                    at: S_DIS,
                    words: dis,
                });
                ops.push(Op::Call {
                    target: user32::address_of("SendMessageA"),
                    args: vec![
                        Arg::V(s.notify),
                        Arg::V(WM_DRAWITEM),
                        Arg::V(id),
                        Arg::S(S_DIS),
                    ],
                });
            }
            continue;
        }
        if part.icon != 0 {
            let cy = r.height();
            r.left += 2;
            user32::paint::draw_icon_at(cpu, hdc, (r.left, r.top), part.icon, (cy, cy), DI_NORMAL);
            r.left += cy;
        }
        let text = part.has_text.then_some(part.text.as_slice());
        draw_status(cpu, hdc, r, text, SBT_NOBORDERS);
    }
    gfx::select(cpu, hdc, old_font);
    if s.bk_color != CLR_DEFAULT {
        gdi32::delete_object(cpu, &[bk]);
    }
    if super::style(cpu, hwnd) & SBARS_SIZEGRIP != 0 {
        draw_size_grip(cpu, hdc, client);
    }
    ops
}

/// A script that paints the bar through BeginPaint and EndPaint.
fn paint_script(hwnd: u32) -> Script {
    let mut s = Script::new(hwnd);
    s.alloc(S_PAINT);
    let begin = user32::address_of("BeginPaint");
    let end = user32::address_of("EndPaint");
    s.call(begin, &[Arg::V(hwnd), Arg::S(S_PS)])
        .then(DRAW, &[])
        .call(end, &[Arg::V(hwnd), Arg::S(S_PS)])
        .ret(0);
    s
}

/// The continuations.
pub fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    match id {
        DRAW => {
            if c.last == 0 {
                return Vec::new();
            }
            draw(cpu, c.hwnd, c.last)
        }
        T_MOUSE => {
            // The parent let the default happen by answering 0.
            if c.last != 0 {
                return vec![Op::Return(Arg::V(0))];
            }
            let args = [c.hwnd, c.word(0), c.word(1), c.word(2)];
            vec![
                Op::Call {
                    target: user32::address_of("DefWindowProcA"),
                    args: args.iter().map(|&a| Arg::V(a)).collect(),
                },
                Op::Return(Arg::Last),
            ]
        }
        T_FORMAT => {
            if let Some(mut s) = get(cpu, c.hwnd) {
                s.unicode = c.last == NFR_UNICODE;
                let result = if s.unicode { NFR_UNICODE } else { NFR_ANSI };
                put(cpu, c.hwnd, s);
                c.result = result;
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// The status bar's window procedure.
pub fn proc_(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    if msg == WM_CREATE {
        return create(cpu, hwnd, lp);
    }
    let Some(mut s) = get(cpu, hwnd) else {
        return defwnd::def_window_proc(cpu, args);
    };
    let n = (wp & 0xFF) as usize;
    let result = match msg {
        SB_GETBORDERS => {
            if lp != 0 {
                write_words(
                    cpu,
                    lp,
                    &[s.h_border as u32, s.v_border as u32, s.gap as u32],
                );
                if cpu.bus.guest_faulted() {
                    return None;
                }
            }
            1
        }
        SB_SETBORDERS => {
            let w = read_words(cpu, lp, 3);
            if cpu.bus.guest_faulted() {
                return None;
            }
            s.h_border = w[0] as i32;
            s.v_border = w[1] as i32;
            s.gap = w[2] as i32;
            put(cpu, hwnd, s);
            invalidate(cpu, hwnd, false);
            1
        }
        SB_GETICON => {
            // The simple part is -1 here.
            if wp == u32::MAX {
                s.simple_part.icon
            } else {
                s.parts.get(wp as usize).map_or(0, |p| p.icon)
            }
        }
        SB_SETICON => {
            let part = if wp == u32::MAX {
                Some(&mut s.simple_part)
            } else {
                s.parts.get_mut(wp as usize)
            };
            let Some(part) = part else {
                return Some(0);
            };
            if part.icon != lp {
                part.icon = lp;
                put(cpu, hwnd, s);
                invalidate(cpu, hwnd, false);
            }
            1
        }
        SB_GETPARTS => {
            if lp != 0 {
                let words: Vec<u32> = s
                    .parts
                    .iter()
                    .take(wp as usize)
                    .map(|p| p.x as u32)
                    .collect();
                write_words(cpu, lp, &words);
                if cpu.bus.guest_faulted() {
                    return None;
                }
            }
            s.parts.len() as u32
        }
        SB_SETPARTS => {
            if wp == 0 || wp > 256 {
                return Some(0);
            }
            let w = read_words(cpu, lp, wp as usize);
            if cpu.bus.guest_faulted() {
                return None;
            }
            s.parts.resize_with(wp as usize, || Part {
                x: -1,
                ..Part::default()
            });
            for (p, &x) in s.parts.iter_mut().zip(&w) {
                p.x = x as i32;
            }
            put(cpu, hwnd, s);
            invalidate(cpu, hwnd, false);
            1
        }
        SB_GETRECT => {
            if n >= s.parts.len() {
                return Some(0);
            }
            let (simple, parts) = bounds(cpu, hwnd, &s);
            let r = if s.simple { simple } else { parts[n] };
            r.write(cpu, lp);
            if cpu.bus.guest_faulted() {
                return None;
            }
            1
        }
        SB_GETTEXTA | SB_GETTEXTLENGTHA | SB_GETTEXTLENGTHW => {
            if n >= s.parts.len() {
                return Some(0);
            }
            let part = if s.simple {
                &s.simple_part
            } else {
                &s.parts[n]
            };
            if part.style & SBT_OWNERDRAW != 0 {
                if msg == SB_GETTEXTA {
                    part.data
                } else {
                    part.style << 16
                }
            } else {
                let len = part.text.len() as u32;
                if msg == SB_GETTEXTA && part.has_text && lp != 0 {
                    let mut bytes = part.text.clone();
                    bytes.push(0);
                    cpu.bus.guest_write_bytes(lp, &bytes);
                    if cpu.bus.guest_faulted() {
                        return None;
                    }
                }
                (len & 0xFFFF) | part.style << 16
            }
        }
        // COMCTL32 4.70 has no Unicode texts.
        SB_GETTEXTW | SB_SETTEXTW | SB_SETTIPTEXTW | SB_GETTIPTEXTW => 0,
        // Tips need SBT_TOOLTIPS, which the bar doesn't make; without
        // them the bar leaves the buffer as it was, as Windows 95's
        // does.
        SB_SETTIPTEXTA | SB_GETTIPTEXTA => 0,
        SB_GETUNICODEFORMAT => s.unicode as u32,
        SB_SETUNICODEFORMAT => {
            let old = s.unicode;
            s.unicode = wp != 0;
            put(cpu, hwnd, s);
            old as u32
        }
        SB_ISSIMPLE => s.simple as u32,
        SB_SETBKCOLOR => {
            let old = std::mem::replace(&mut s.bk_color, lp);
            put(cpu, hwnd, s);
            invalidate(cpu, hwnd, false);
            old
        }
        SB_SETMINHEIGHT => {
            s.min_height = (wp as i32).max(min_size());
            s.height = compute_height(cpu, &s);
            // As Windows, it doesn't resize the bar until WM_SIZE.
            put(cpu, hwnd, s);
            1
        }
        SB_SETTEXTA => return set_text(cpu, hwnd, s, wp, lp),
        SB_SIMPLE => {
            let simple = wp != 0;
            if s.simple == simple {
                return Some(1);
            }
            s.simple = simple;
            let notify = s.notify;
            put(cpu, hwnd, s);
            invalidate(cpu, hwnd, false);
            let mut sc = Script::new(hwnd);
            if is_window(cpu, notify) {
                let id = super::id_of(cpu, hwnd);
                let at = sc.data(&[hwnd, id, SBN_SIMPLEMODECHANGE]);
                sc.send(notify, WM_NOTIFY, Arg::V(0), Arg::S(at));
            }
            sc.ret(1);
            return script::run(cpu, STATUS, sc);
        }
        WM_DESTROY => {
            state(cpu).status.bars.remove(&hwnd);
            0
        }
        WM_GETFONT => font_of(&s, cpu),
        WM_GETTEXT => {
            let part = &s.parts[0];
            if !part.has_text {
                return Some(0);
            }
            let len = part.text.len() as u32;
            if wp == 0 {
                return Some(len);
            }
            let copied = user32::copy_out(cpu, lp, wp, &part.text);
            if cpu.bus.guest_faulted() {
                return None;
            }
            copied
        }
        WM_GETTEXTLENGTH => {
            let part = &s.parts[0];
            let len = if part.style & SBT_OWNERDRAW == 0 {
                part.text.len() as u32
            } else {
                0
            };
            len | part.style << 16
        }
        WM_SETTEXT => {
            let text = if lp != 0 {
                user32::bytes_at(cpu, lp)?
            } else {
                Vec::new()
            };
            let part = &mut s.parts[0];
            part.has_text = !text.is_empty();
            part.text = text;
            put(cpu, hwnd, s);
            invalidate(cpu, hwnd, false);
            1
        }
        WM_LBUTTONDBLCLK => return mouse_notify(cpu, hwnd, &s, NM_DBLCLK, args),
        WM_LBUTTONUP => return mouse_notify(cpu, hwnd, &s, NM_CLICK, args),
        WM_RBUTTONDBLCLK => return mouse_notify(cpu, hwnd, &s, NM_RDBLCLK, args),
        WM_RBUTTONUP => return mouse_notify(cpu, hwnd, &s, NM_RCLICK, args),
        // Tooltips would have it relayed: the bar makes none.
        WM_MOUSEMOVE => 0,
        WM_NCHITTEST => {
            if super::style(cpu, hwnd) & SBARS_SIZEGRIP != 0 {
                let (x, y) = user32::point_of(lp);
                let (ox, oy) = user32::user(cpu).client_origin(hwnd);
                let mut r = super::client(cpu, hwnd);
                r.left = r.right - 13;
                r.top += 2;
                if r.contains(x - ox, y - oy) {
                    return Some(HTBOTTOMRIGHT);
                }
            }
            return defwnd::def_window_proc(cpu, args);
        }
        WM_NCLBUTTONUP | WM_NCLBUTTONDOWN => {
            if s.notify != 0 {
                user32::message::post_message(cpu, &[s.notify, msg, wp, lp]);
            }
            0
        }
        WM_NOTIFYFORMAT => {
            if lp == NF_REQUERY && is_window(cpu, wp) {
                let mut sc = Script::new(hwnd);
                sc.send(wp, WM_NOTIFYFORMAT, Arg::V(hwnd), Arg::V(NF_QUERY))
                    .then(T_FORMAT, &[]);
                return script::run(cpu, STATUS, sc);
            }
            if s.unicode { NFR_UNICODE } else { NFR_ANSI }
        }
        WM_PAINT | WM_PRINTCLIENT => {
            if wp != 0 {
                let ops = draw(cpu, hwnd, wp);
                let mut sc = Script::new(hwnd);
                sc.alloc(S_PAINT);
                sc.ops.extend(ops);
                sc.ret(0);
                return script::run(cpu, STATUS, sc);
            }
            return script::run(cpu, STATUS, paint_script(hwnd));
        }
        WM_SETFONT => {
            s.font = wp;
            s.height = compute_height(cpu, &s);
            put(cpu, hwnd, s);
            if lp & 0xFFFF != 0 {
                invalidate(cpu, hwnd, false);
            }
            let mut sc = Script::new(hwnd);
            sc.send(hwnd, WM_SIZE, Arg::V(0), Arg::V(0)).ret(0);
            return script::run(cpu, STATUS, sc);
        }
        WM_SIZE => {
            let flags = wp & 0xFFFF;
            if (flags != SIZE_RESTORED && flags != SIZE_MAXIMIZED)
                || super::style(cpu, hwnd) & CCS_NORESIZE != 0
                || !is_window(cpu, s.notify)
            {
                return defwnd::def_window_proc(cpu, args);
            }
            let parent = super::client(cpu, s.notify);
            let y = parent.bottom - s.height;
            let mut sc = Script::new(hwnd);
            sc.user(
                "MoveWindow",
                &[
                    hwnd,
                    parent.left as u32,
                    y as u32,
                    parent.width() as u32,
                    s.height as u32,
                    1,
                ],
            )
            .ret(0);
            return script::run(cpu, STATUS, sc);
        }
        WM_SYSCOLORCHANGE => 0,
        _ => return defwnd::def_window_proc(cpu, args),
    };
    Some(result)
}

/// SB_SETTEXTA (STATUSBAR_SetTextT): the part's text and style, then a
/// repaint at once.
fn set_text(cpu: &mut Cpu, hwnd: u32, mut s: Status, wp: u32, lp: u32) -> Option<u32> {
    let n = wp & 0xFF;
    let style = wp & 0xFF00;
    let text = if style & SBT_OWNERDRAW == 0 && lp != 0 {
        Some(printable(&user32::bytes_at(cpu, lp)?))
    } else {
        None
    };
    let part = if n == SB_SIMPLEID {
        &mut s.simple_part
    } else {
        match s.parts.get_mut(n as usize) {
            Some(p) => p,
            None => return Some(0),
        }
    };
    let changed = part.style != style;
    part.style = style;
    if style & SBT_OWNERDRAW != 0 {
        part.data = lp;
        part.text.clear();
        part.has_text = false;
    } else {
        match text {
            Some(t) => {
                if !changed && part.has_text && part.text == t {
                    return Some(1);
                }
                part.text = t;
                part.has_text = true;
            }
            None => {
                if !changed && !part.has_text {
                    return Some(1);
                }
                part.text.clear();
                part.has_text = false;
            }
        }
    }
    put(cpu, hwnd, s);
    invalidate(cpu, hwnd, false);
    let mut sc = Script::new(hwnd);
    sc.user("UpdateWindow", &[hwnd]).ret(1);
    script::run(cpu, STATUS, sc)
}

/// Tell the parent of a click with an NMMOUSE
/// (STATUSBAR_SendMouseNotify): when it answers 0, the default happens
/// too.
fn mouse_notify(cpu: &mut Cpu, hwnd: u32, s: &Status, code: u32, args: &[u32]) -> Option<u32> {
    if !is_window(cpu, s.notify) {
        return defwnd::def_window_proc(cpu, args);
    }
    let (msg, wp, lp) = (args[1], args[2], args[3]);
    let (x, y) = user32::point_of(lp);
    let spec = if s.simple {
        255
    } else {
        let (_, parts) = bounds(cpu, hwnd, s);
        parts
            .iter()
            .position(|r| x >= r.left && x <= r.right)
            .map_or(-2i32 as u32, |i| i as u32)
    };
    let mut sc = Script::new(hwnd);
    let more = [spec, 0, x as u32, y as u32, 0x30000];
    super::notify(cpu, &mut sc, hwnd, s.notify, code, &more);
    sc.then(T_MOUSE, &[msg, wp, lp]);
    script::run(cpu, STATUS, sc)
}

/// WM_CREATE: one part across the bar with the window's text, the
/// default borders, WS_BORDER taken away, and the parent asked for
/// its notification format.
fn create(cpu: &mut Cpu, hwnd: u32, cs: u32) -> Option<u32> {
    let w = read_words(cpu, cs, 12);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (parent, name) = (w[3], w[9]);
    let text = if name != 0 {
        user32::bytes_at(cpu, name)?
    } else {
        Vec::new()
    };
    let mut s = Status {
        notify: parent,
        parts: vec![Part {
            x: -1,
            has_text: !text.is_empty(),
            text,
            ..Part::default()
        }],
        bk_color: CLR_DEFAULT,
        h_border: HORZ_BORDER,
        v_border: VERT_BORDER,
        gap: HORZ_GAP,
        min_height: min_size(),
        ..Status::default()
    };
    if let Some(win) = user32::user(cpu).window_mut(hwnd) {
        win.style &= !WS_BORDER;
    }
    s.height = compute_height(cpu, &s);
    put(cpu, hwnd, s);
    let mut sc = Script::new(hwnd);
    if is_window(cpu, parent) {
        sc.send(parent, WM_NOTIFYFORMAT, Arg::V(hwnd), Arg::V(NF_QUERY))
            .then(T_FORMAT, &[]);
    }
    sc.ret(0);
    script::run(cpu, STATUS, sc)
}

/// MenuHelp(msg, wParam, lParam, menu, instance, status, ids): show
/// the help text of the menu item WM_MENUSELECT selected, from the
/// string table, in the status bar's simple mode, and leave the simple
/// mode when the menu closes.
pub fn menu_help(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[msg, wp, lp, _menu, instance, status, ids] = args else {
        unreachable!()
    };
    if !is_window(cpu, status) || msg != WM_MENUSELECT {
        return Some(0);
    }
    let mut s = Script::new(0);
    if wp >> 16 == 0xFFFF && lp == 0 {
        s.send(status, SB_SIMPLE, Arg::V(0), Arg::V(0));
    } else {
        let id = if (wp >> 16) & MF_POPUP != 0 {
            let id = cpu.bus.guest_read_32(ids.wrapping_add(4));
            if cpu.bus.guest_faulted() {
                return None;
            }
            id
        } else {
            wp & 0xFFFF
        };
        if id != 0 {
            let buf = s.data(&[0; 64]);
            let load = user32::address_of("LoadStringA");
            s.call(
                load,
                &[Arg::V(instance), Arg::V(id), Arg::S(buf), Arg::V(256)],
            );
            let wp = Arg::V(SB_SIMPLEID | SBT_NOBORDERS);
            s.send(status, SB_SETTEXTA, wp, Arg::S(buf));
            s.send(status, SB_SIMPLE, Arg::V(1), Arg::V(0));
        }
    }
    s.ret(0);
    script::run(cpu, STATUS, s)
}

/// ShowHideMenuCtl(hwnd, id, info): check or uncheck the menu item `id`
/// and show or hide the control the pairs of `info` give for it.
pub fn show_hide_menu_ctl(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, id, info] = args else {
        unreachable!()
    };
    if info == 0 {
        return Some(0);
    }
    let head = read_words(cpu, info, 2);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if head[0] == 0 || head[1] == 0 {
        return Some(0);
    }
    // The pairs of menu item and control ids, to the one for `id`.
    let mut at = info + 8;
    let mut found = None;
    for _ in 0..1024 {
        let w = read_words(cpu, at, 2);
        if cpu.bus.guest_faulted() {
            return None;
        }
        if w[0] == id {
            found = Some((w[0], w[1]));
            break;
        }
        if w[0] == 0 && w[1] == 0 {
            break;
        }
        at += 8;
    }
    let Some((item, control)) = found else {
        return Some(0);
    };
    let state = user32::menu::get_menu_state(cpu, &[head[1], id, MF_BYCOMMAND]).unwrap_or(0);
    let (check, show) = if state & MF_CHECKED != 0 {
        (MF_UNCHECKED, SWP_HIDEWINDOW)
    } else {
        (MF_CHECKED, SWP_SHOWWINDOW)
    };
    user32::menu::check_menu_item(cpu, &[head[0], item, MF_BYCOMMAND | check]);
    let ctl = user32::dialog::get_dlg_item(cpu, &[hwnd, control]).unwrap_or(0);
    let mut s = Script::new(0);
    s.user("SetWindowPos", &[ctl, 0, 0, 0, 0, 0, show]).ret(1);
    script::run(cpu, STATUS, s)
}

/// GetEffectiveClientRect(hwnd, rect, info): the client area less the
/// visible controls `info` names.
pub fn get_effective_client_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, out, info] = args else {
        unreachable!()
    };
    let mut r = super::client(cpu, hwnd);
    let mut at = info;
    for _ in 0..1024 {
        at += 8;
        let id = cpu.bus.guest_read_32(at);
        if cpu.bus.guest_faulted() {
            return None;
        }
        if id == 0 {
            break;
        }
        at += 4;
        let control = cpu.bus.guest_read_32(at);
        if cpu.bus.guest_faulted() {
            return None;
        }
        let ctl = user32::dialog::get_dlg_item(cpu, &[hwnd, control]).unwrap_or(0);
        let visible = user32::user(cpu)
            .window(ctl)
            .is_some_and(|w| w.style & user32::window::WS_VISIBLE != 0);
        if visible {
            let st = user32::user(cpu);
            let screen = st.screen_rect(ctl);
            let (ox, oy) = st.client_origin(hwnd);
            r = subtract(r, screen.offset(-ox, -oy));
        }
        at += 4;
        let next = cpu.bus.guest_read_32(at);
        if cpu.bus.guest_faulted() {
            return None;
        }
        if next == 0 {
            break;
        }
    }
    r.write(cpu, out);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(0)
}

/// SubtractRect: `a` less `b` when `b` takes a whole side of it.
fn subtract(a: Rect, b: Rect) -> Rect {
    let i = a.intersect(&b);
    if i.is_empty() {
        return a;
    }
    let mut r = a;
    if i.left == a.left && i.right == a.right {
        if i.top == a.top {
            r.top = i.bottom;
        } else if i.bottom == a.bottom {
            r.bottom = i.top;
        }
    } else if i.top == a.top && i.bottom == a.bottom {
        if i.left == a.left {
            r.left = i.right;
        } else if i.right == a.right {
            r.right = i.left;
        }
    }
    r
}

/// DrawStatusTextA(hdc, rect, text, style).
pub fn draw_status_text(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hdc, rect, text, style] = args else {
        unreachable!()
    };
    let r = Rect::read(cpu, rect);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let text = if text != 0 {
        Some(user32::bytes_at(cpu, text)?)
    } else {
        None
    };
    draw_status(cpu, hdc, r, text.as_deref(), style);
    Some(0)
}

/// CreateStatusWindowA(style, text, parent, id).
pub fn create_status_window(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[style, text, parent, id] = args else {
        unreachable!()
    };
    let mut s = Script::new(0);
    let class = s.string(b"msctls_statusbar32");
    let f = user32::address_of("CreateWindowExA");
    let d = Arg::V(CW_USEDEFAULT);
    s.call(
        f,
        &[
            Arg::V(0),
            Arg::S(class),
            Arg::V(text),
            Arg::V(style),
            d,
            d,
            d,
            d,
            Arg::V(parent),
            Arg::V(id),
            Arg::V(0),
            Arg::V(0),
        ],
    )
    .ret_last();
    script::run(cpu, STATUS, s)
}
