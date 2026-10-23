//! The up-down control (msctls_updown32): two arrow buttons that step a
//! value up and down between a range's ends, shown in a buddy window's
//! text (UDS_SETBUDDYINT) and read back from it. With UDS_ALIGNLEFT or
//! UDS_ALIGNRIGHT it places itself at the buddy's side; with
//! UDS_ARROWKEYS it subclasses the buddy for the up and down keys, by
//! its window procedure, as COMCTL32 4.70 does.
//!
//! Ported from Wine's updown.c:
//!
//! Copyright 1997, 2002 Dimitrie O. Paun
//! Copyright 1999 Luc Tourangeau
//!
//! This library is free software; you can redistribute it and/or modify
//! it under the terms of the GNU Lesser General Public License as
//! published by the Free Software Foundation; either version 2.1 of the
//! License, or (at your option) any later version.

use super::script::{self, Arg, Ctx, Op, Script};
use super::{DRAW, UPDOWN, invalidate, paint_script, state};
use crate::cpu::Cpu;
use crate::win32::gdi32::Rect;
use crate::win32::user32::message::{
    WM_CREATE, WM_DESTROY, WM_ENABLE, WM_HSCROLL, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_NCDESTROY, WM_NOTIFY, WM_PAINT, WM_TIMER, WM_VSCROLL,
};
use crate::win32::user32::window::{
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOZORDER, WS_BORDER,
    WS_DISABLED,
};
use crate::win32::user32::{self, class, defwnd, read_words, write_words};

const WM_PRINTCLIENT: u32 = 0x0318;
const WM_STYLECHANGED: u32 = 0x007D;
const WM_CANCELMODE: u32 = 0x001F;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_NOTIFYFORMAT: u32 = 0x0055;
const NF_QUERY: u32 = 3;
const GWL_STYLE: u32 = (-16i32) as u32;
const GW_HWNDPREV: u32 = 3;

const UDM_SETRANGE: u32 = 0x0465;
const UDM_GETRANGE: u32 = 0x0466;
const UDM_SETPOS: u32 = 0x0467;
const UDM_GETPOS: u32 = 0x0468;
const UDM_SETBUDDY: u32 = 0x0469;
const UDM_GETBUDDY: u32 = 0x046A;
const UDM_SETACCEL: u32 = 0x046B;
const UDM_GETACCEL: u32 = 0x046C;
const UDM_SETBASE: u32 = 0x046D;
const UDM_GETBASE: u32 = 0x046E;
const UDM_SETRANGE32: u32 = 0x046F;
const UDM_GETRANGE32: u32 = 0x0470;
const UDM_SETPOS32: u32 = 0x0471;
const UDM_GETPOS32: u32 = 0x0472;
const UDM_SETUNICODEFORMAT: u32 = 0x2005;
const UDM_GETUNICODEFORMAT: u32 = 0x2006;

pub const UDS_WRAP: u32 = 0x0001;
pub const UDS_SETBUDDYINT: u32 = 0x0002;
pub const UDS_ALIGNRIGHT: u32 = 0x0004;
pub const UDS_ALIGNLEFT: u32 = 0x0008;
pub const UDS_AUTOBUDDY: u32 = 0x0010;
pub const UDS_ARROWKEYS: u32 = 0x0020;
pub const UDS_HORZ: u32 = 0x0040;
pub const UDS_NOTHOUSANDS: u32 = 0x0080;
pub const UDS_HOTTRACK: u32 = 0x0100;

const UDN_DELTAPOS: u32 = (-722i32) as u32;
const NM_RELEASEDCAPTURE: u32 = (-16i32) as u32;

const LB_SETCURSEL: u32 = 0x0186;
const LB_GETCARETINDEX: u32 = 0x019F;
const EM_SETSEL: u32 = 0x00B1;
const SB_THUMBPOSITION: u32 = 4;
const SB_ENDSCROLL: u32 = 8;
const VK_UP: u32 = 0x26;
const VK_DOWN: u32 = 0x28;

const DFC_SCROLL: u32 = 3;
const DFCS_SCROLLUP: u32 = 0x0000;
const DFCS_SCROLLDOWN: u32 = 0x0001;
const DFCS_SCROLLLEFT: u32 = 0x0002;
const DFCS_SCROLLRIGHT: u32 = 0x0003;
const DFCS_INACTIVE: u32 = 0x0100;
const DFCS_PUSHED: u32 = 0x0200;
const DFCS_HOT: u32 = 0x1000;
const EDGE_SUNKEN: u32 = 0x000A;
const BF_LEFT: u32 = 1;
const BF_TOP: u32 = 2;
const BF_RIGHT: u32 = 4;
const BF_BOTTOM: u32 = 8;

const INITIAL_DELAY: u32 = 500;
const AUTOPRESS_DELAY: u32 = 250;
const REPEAT_DELAY: u32 = 50;
const DEFAULT_WIDTH: i32 = 16;
const BUDDY_BORDER: i32 = 2;
const BUDDY_SPACER: i32 = 2;

const FLAG_INCR: u32 = 0x01;
const FLAG_DECR: u32 = 0x02;
const FLAG_MOUSEIN: u32 = 0x04;
const FLAG_PRESSED: u32 = 0x08;
const FLAG_BUDDYINT: u32 = 0x10;
const FLAG_ARROW: u32 = FLAG_INCR | FLAG_DECR;

const BUDDY_UNKNOWN: u32 = 0;
const BUDDY_LISTBOX: u32 = 1;
const BUDDY_EDIT: u32 = 2;

const TIMER_AUTOREPEAT: u32 = 1;
const TIMER_ACCEL: u32 = 2;
const TIMER_AUTOPRESS: u32 = 3;

/// The scratch space of the control's scripts: the buddy's text as
/// read, the text written, an NMUPDOWN.
const S_TEXT: u32 = 0;
const S_NEW: u32 = 32;
const S_NM: u32 = 64;
const S_SIZE: u32 = 96;
const TEXT_MAX: u32 = 20;

// The continuations.
const T_GOT_TEXT: u16 = 1;
const T_GOT_INDEX: u16 = 2;
const T_RET_POS: u16 = 3;
const T_RET_POS32: u16 = 4;
const T_SET_TEXT: u16 = 5;
const T_ACTION: u16 = 6;
const T_DELTA: u16 = 7;
const T_CREATED: u16 = 8;

/// What an up-down control keeps.
#[derive(Clone, Debug, Default)]
pub struct UpDown {
    pub notify: u32,
    pub style: u32,
    /// UDACCEL's seconds and increments.
    pub accel: Vec<(u32, u32)>,
    pub accel_index: i32,
    pub base: u32,
    pub cur: i32,
    pub min: i32,
    pub max: i32,
    pub buddy: u32,
    pub buddy_type: u32,
    pub flags: u32,
    pub unicode: bool,
    /// Whether the buddy's text was read as a number, the last time.
    pub got: bool,
}

crate::state_fields!(UpDown {
    notify,
    style,
    accel,
    accel_index,
    base,
    cur,
    min,
    max,
    buddy,
    buddy_type,
    flags,
    unicode,
    got
});

/// A buddy the control subclassed: its control and its own procedure.
#[derive(Clone, Debug, Default)]
pub struct Buddy {
    pub updown: u32,
    pub proc: u32,
}

crate::state_fields!(Buddy { updown, proc });

fn get(cpu: &mut Cpu, hwnd: u32) -> Option<UpDown> {
    state(cpu).updown.get(&hwnd).cloned()
}

fn put(cpu: &mut Cpu, hwnd: u32, u: UpDown) {
    state(cpu).updown.insert(hwnd, u);
}

fn is_window(cpu: &mut Cpu, hwnd: u32) -> bool {
    hwnd != 0 && user32::user(cpu).window(hwnd).is_some()
}

fn in_bounds(u: &UpDown, v: i32) -> bool {
    if u.max > u.min {
        u.min <= v && v <= u.max
    } else {
        u.max <= v && v <= u.min
    }
}

fn offset(u: &mut UpDown, delta: i32) -> bool {
    let mut delta = delta;
    if !in_bounds(u, u.cur.wrapping_add(delta)) {
        if u.style & UDS_WRAP == 0 {
            return false;
        }
        let sign = if delta < 0 { -1 } else { 1 };
        let order = if u.max < u.min { -1 } else { 1 };
        delta += sign * order * (u.min - u.max) + if delta < 0 { 1 } else { -1 };
    }
    u.cur = u.cur.wrapping_add(delta);
    true
}

fn has_buddy_border(u: &UpDown) -> bool {
    u.style & (UDS_ALIGNLEFT | UDS_ALIGNRIGHT) != 0 && u.buddy_type == BUDDY_EDIT
}

fn arrow_rect(cpu: &mut Cpu, hwnd: u32, u: &UpDown, arrow: u32) -> Rect {
    let mut r = super::client(cpu, hwnd);
    if has_buddy_border(u) {
        if u.style & UDS_ALIGNLEFT != 0 {
            r.left += BUDDY_BORDER;
        } else {
            r.right -= BUDDY_BORDER;
        }
        r = r.inflate(0, -BUDDY_BORDER);
    }
    if is_window(cpu, u.buddy) {
        if u.style & UDS_ALIGNLEFT != 0 {
            r.right -= BUDDY_SPACER;
        } else if u.style & UDS_ALIGNRIGHT != 0 {
            r.left += BUDDY_SPACER;
        }
    }
    if u.style & UDS_HORZ != 0 {
        let len = r.right - r.left + 1;
        if arrow & FLAG_INCR != 0 {
            r.left += len / 2;
        }
        if arrow & FLAG_DECR != 0 {
            r.right = r.left + len / 2 - 1;
        }
    } else {
        let len = r.bottom - r.top + 1;
        if arrow & FLAG_INCR != 0 {
            r.bottom = r.top + len / 2 - 1;
        }
        if arrow & FLAG_DECR != 0 {
            r.top += len / 2;
        }
    }
    r
}

fn arrow_at(cpu: &mut Cpu, hwnd: u32, u: &UpDown, x: i32, y: i32) -> u32 {
    for arrow in [FLAG_INCR, FLAG_DECR] {
        let r = arrow_rect(cpu, hwnd, u, arrow);
        if x >= r.left && x < r.right && y >= r.top && y < r.bottom {
            return arrow;
        }
    }
    0
}

/// Paint the control (UPDOWN_Draw).
pub fn draw(cpu: &mut Cpu, hwnd: u32, hdc: u32) {
    let Some(u) = get(cpu, hwnd) else {
        return;
    };
    let pressed = u.flags & FLAG_PRESSED != 0;
    let up_pressed = pressed && u.flags & FLAG_INCR != 0;
    let up_hot = u.flags & FLAG_INCR != 0 && u.flags & FLAG_MOUSEIN != 0;
    let down_pressed = pressed && u.flags & FLAG_DECR != 0;
    let down_hot = u.flags & FLAG_DECR != 0 && u.flags & FLAG_MOUSEIN != 0;
    if has_buddy_border(&u) {
        let r = super::client(cpu, hwnd);
        let side = if u.style & UDS_ALIGNLEFT != 0 {
            BF_LEFT
        } else {
            BF_RIGHT
        };
        super::gfx::edge(cpu, hdc, r, EDGE_SUNKEN, BF_BOTTOM | BF_TOP | side);
    }
    let horz = u.style & UDS_HORZ != 0;
    let inactive = if u.style & WS_DISABLED != 0 {
        DFCS_INACTIVE
    } else {
        0
    };
    let hot_track = u.style & UDS_HOTTRACK != 0;
    for (arrow, kind, p, hot) in [
        (
            FLAG_INCR,
            if horz { DFCS_SCROLLRIGHT } else { DFCS_SCROLLUP },
            up_pressed,
            up_hot,
        ),
        (
            FLAG_DECR,
            if horz { DFCS_SCROLLLEFT } else { DFCS_SCROLLDOWN },
            down_pressed,
            down_hot,
        ),
    ] {
        let r = arrow_rect(cpu, hwnd, &u, arrow);
        let flags = kind
            | if hot_track && hot { DFCS_HOT } else { 0 }
            | if p { DFCS_PUSHED } else { 0 }
            | inactive;
        user32::paint::frame_control(cpu, hdc, r, DFC_SCROLL, flags);
    }
}

/// The buddy's text as GetWindowTextA put it in the scratch space.
fn read_text(cpu: &mut Cpu, at: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for i in 0..TEXT_MAX {
        let b = cpu.bus.guest_read_8(at + i);
        if b == 0 {
            break;
        }
        out.push(b);
    }
    out
}

/// strtol's reading of the whole of `text` in `base`.
fn parse(text: &[u8], base: u32) -> Option<i32> {
    let s = std::str::from_utf8(text).ok()?.trim_start();
    let (neg, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let s = if base == 16 {
        s.strip_prefix("0x")
            .or_else(|| s.strip_prefix("0X"))
            .unwrap_or(s)
    } else {
        s
    };
    if s.is_empty() {
        return Some(0);
    }
    let v = i64::from_str_radix(s, base).ok()?;
    let v = if neg { -v } else { v };
    Some(v as i32)
}

/// What UDS_SETBUDDYINT shows for the value: decimal with thousands'
/// commas, or hex.
fn format(u: &UpDown) -> Vec<u8> {
    if u.base == 16 {
        return format!("0x{:04X}", u.cur).into_bytes();
    }
    let digits = format!("{}", u.cur);
    if u.style & UDS_NOTHOUSANDS != 0 || digits.len() <= 3 {
        return digits.into_bytes();
    }
    let b = digits.as_bytes();
    let mut start = b.len() % 3;
    if start == 0 {
        start = 3;
    }
    let mut out = b[..start].to_vec();
    for (i, &c) in b[start..].iter().enumerate() {
        if i % 3 == 0 {
            out.push(b',');
        }
        out.push(c);
    }
    out
}

/// Add reading the buddy's value to `s` (UPDOWN_GetBuddyInt): the
/// continuations set `cur` and `got`.
fn get_buddy_int(cpu: &mut Cpu, s: &mut Script, hwnd: u32, u: &mut UpDown) {
    u.got = false;
    if u.flags & FLAG_BUDDYINT == 0 || !is_window(cpu, u.buddy) {
        return;
    }
    if u.buddy_type == BUDDY_LISTBOX {
        s.send(u.buddy, LB_GETCARETINDEX, Arg::V(0), Arg::V(0))
            .then(T_GOT_INDEX, &[hwnd]);
    } else {
        let f = user32::address_of("GetWindowTextA");
        s.call(f, &[Arg::V(u.buddy), Arg::S(S_TEXT), Arg::V(TEXT_MAX)])
            .then(T_GOT_TEXT, &[hwnd]);
    }
}

/// The steps of UPDOWN_SetBuddyInt: show the value in the buddy if its
/// text isn't that already.
fn set_buddy_int_ops(cpu: &mut Cpu, u: &UpDown) -> Vec<Op> {
    if u.flags & FLAG_BUDDYINT == 0 || !is_window(cpu, u.buddy) {
        return Vec::new();
    }
    if u.buddy_type == BUDDY_LISTBOX {
        return vec![Op::Call {
            target: user32::address_of("SendMessageA"),
            args: vec![
                Arg::V(u.buddy),
                Arg::V(LB_SETCURSEL),
                Arg::V(u.cur as u32),
                Arg::V(0),
            ],
        }];
    }
    let text = format(u);
    let mut bytes = text.clone();
    bytes.push(0);
    let f = user32::address_of("GetWindowTextA");
    vec![
        Op::Write {
            at: S_TEXT,
            words: vec![0; 8],
        },
        Op::Call {
            target: f,
            args: vec![Arg::V(u.buddy), Arg::S(S_TEXT), Arg::V(TEXT_MAX)],
        },
        Op::Write {
            at: S_NEW,
            words: script::words_of(&bytes),
        },
        Op::Then {
            id: T_SET_TEXT,
            data: vec![u.buddy],
        },
    ]
}

fn set_buddy_int(cpu: &mut Cpu, s: &mut Script, u: &UpDown) {
    let ops = set_buddy_int_ops(cpu, u);
    s.ops.extend(ops);
}

/// The steps of UPDOWN_DoAction: ask the parent with UDN_DELTAPOS,
/// step, then tell it with WM_VSCROLL or WM_HSCROLL.
fn action_ops(cpu: &mut Cpu, hwnd: u32, u: &UpDown, delta: i32, action: u32) -> Vec<Op> {
    let mut delta = delta;
    delta *= if action & FLAG_INCR != 0 { 1 } else { -1 };
    delta *= if u.max < u.min { -1 } else { 1 };
    if action & FLAG_INCR != 0 && action & FLAG_DECR != 0 {
        delta = 0;
    }
    let id = super::id_of(cpu, hwnd);
    let send = user32::address_of("SendMessageA");
    vec![
        Op::Write {
            at: S_NM,
            words: vec![hwnd, id, UDN_DELTAPOS, u.cur as u32, delta as u32],
        },
        Op::Call {
            target: send,
            args: vec![Arg::V(u.notify), Arg::V(WM_NOTIFY), Arg::V(id), Arg::S(S_NM)],
        },
        Op::Then {
            id: T_DELTA,
            data: vec![hwnd],
        },
    ]
}

/// The steps after UDN_DELTAPOS.
fn after_delta(cpu: &mut Cpu, hwnd: u32, c: &Ctx) -> Vec<Op> {
    let Some(mut u) = get(cpu, hwnd) else {
        return Vec::new();
    };
    let mut ops = Vec::new();
    if c.last == 0 {
        let delta = cpu.bus.guest_read_32(c.scratch + S_NM + 16) as i32;
        if offset(&mut u, delta) {
            ops = set_buddy_int_ops(cpu, &u);
        }
        put(cpu, hwnd, u.clone());
    }
    let msg = if u.style & UDS_HORZ != 0 {
        WM_HSCROLL
    } else {
        WM_VSCROLL
    };
    let wp = SB_THUMBPOSITION | (u.cur as u32) << 16;
    ops.push(Op::Call {
        target: user32::address_of("SendMessageA"),
        args: vec![Arg::V(u.notify), Arg::V(msg), Arg::V(wp), Arg::V(hwnd)],
    });
    ops
}

/// The continuations.
pub fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    match id {
        DRAW => {
            if c.last != 0 {
                draw(cpu, c.hwnd, c.last);
            }
            Vec::new()
        }
        T_GOT_TEXT => {
            let hwnd = c.word(0);
            let Some(mut u) = get(cpu, hwnd) else {
                return Vec::new();
            };
            let text: Vec<u8> = read_text(cpu, c.scratch + S_TEXT)
                .into_iter()
                .filter(|&b| b != b',')
                .collect();
            if let Some(v) = parse(&text, u.base)
                && in_bounds(&u, v)
            {
                u.cur = v;
                u.got = true;
            }
            put(cpu, hwnd, u);
            Vec::new()
        }
        T_GOT_INDEX => {
            let hwnd = c.word(0);
            if let Some(mut u) = get(cpu, hwnd)
                && (c.last as i32) >= 0
            {
                u.cur = c.last as i32;
                u.got = true;
                put(cpu, hwnd, u);
            }
            Vec::new()
        }
        T_RET_POS => {
            let u = get(cpu, c.word(0)).unwrap_or_default();
            let fail = !u.got as u32;
            vec![Op::Return(Arg::V((u.cur as u32 & 0xFFFF) | fail << 16))]
        }
        T_RET_POS32 => {
            let u = get(cpu, c.word(0)).unwrap_or_default();
            let at = c.word(1);
            if at != 0 {
                cpu.bus.guest_write_32(at, !u.got as u32);
            }
            vec![Op::Return(Arg::V(u.cur as u32))]
        }
        T_SET_TEXT => {
            let old = read_text(cpu, c.scratch + S_TEXT);
            let new = read_text(cpu, c.scratch + S_NEW);
            if old.eq_ignore_ascii_case(&new) {
                return Vec::new();
            }
            vec![Op::Call {
                target: user32::address_of("SetWindowTextA"),
                args: vec![Arg::V(c.word(0)), Arg::S(S_NEW)],
            }]
        }
        T_ACTION => {
            // After reading the buddy: step by `delta` towards `action`.
            let (hwnd, delta, action) = (c.word(0), c.word(1) as i32, c.word(2));
            let Some(u) = get(cpu, hwnd) else {
                return Vec::new();
            };
            action_ops(cpu, hwnd, &u, delta, action)
        }
        T_DELTA => after_delta(cpu, c.word(0), c),
        T_CREATED => {
            let hwnd = c.last;
            if hwnd == 0 {
                return vec![Op::Return(Arg::V(0))];
            }
            let send = user32::address_of("SendMessageA");
            let msg = |m: u32, wp: u32, lp: u32| Op::Call {
                target: send,
                args: vec![Arg::V(hwnd), Arg::V(m), Arg::V(wp), Arg::V(lp)],
            };
            vec![
                msg(UDM_SETBUDDY, c.word(0), 0),
                msg(UDM_SETRANGE, 0, c.word(1)),
                msg(UDM_SETPOS, 0, c.word(2)),
                Op::Return(Arg::V(hwnd)),
            ]
        }
        _ => Vec::new(),
    }
}

/// The value of the first acceleration, or 1.
fn first_inc(u: &UpDown) -> i32 {
    u.accel.first().map_or(1, |a| a.1 as i32)
}

/// UPDOWN_KeyPressed: step for an arrow key.
fn key_pressed(cpu: &mut Cpu, hwnd: u32, key: u32, s: &mut Script) -> bool {
    let Some(mut u) = get(cpu, hwnd) else {
        return false;
    };
    let arrow = match key {
        VK_UP => FLAG_INCR,
        VK_DOWN => FLAG_DECR,
        _ => return false,
    };
    get_buddy_int(cpu, s, hwnd, &mut u);
    u.flags &= !FLAG_ARROW;
    u.flags |= FLAG_PRESSED | arrow;
    let inc = first_inc(&u);
    put(cpu, hwnd, u);
    invalidate(cpu, hwnd, false);
    user32::message::set_timer(cpu, &[hwnd, TIMER_AUTOPRESS, AUTOPRESS_DELAY, 0]);
    s.then(T_ACTION, &[hwnd, inc as u32, arrow]);
    true
}

fn is_enabled(cpu: &mut Cpu, hwnd: u32, u: &UpDown) -> bool {
    let enabled = |cpu: &mut Cpu, h: u32| {
        user32::user(cpu)
            .window(h)
            .is_some_and(|w| w.style & WS_DISABLED == 0)
    };
    if !enabled(cpu, hwnd) {
        return false;
    }
    u.buddy == 0 || enabled(cpu, u.buddy)
}

/// UPDOWN_CancelMode.
fn cancel_mode(cpu: &mut Cpu, hwnd: u32, s: &mut Script) -> bool {
    let Some(mut u) = get(cpu, hwnd) else {
        return false;
    };
    if u.flags & FLAG_PRESSED == 0 {
        return false;
    }
    for t in [TIMER_AUTOREPEAT, TIMER_ACCEL, TIMER_AUTOPRESS] {
        user32::message::kill_timer(cpu, &[hwnd, t]);
    }
    if user32::input::get_capture(cpu, &[]) == Some(hwnd) {
        super::notify(cpu, s, hwnd, u.notify, NM_RELEASEDCAPTURE, &[]);
        s.user("ReleaseCapture", &[]);
    }
    u.flags &= !FLAG_PRESSED;
    put(cpu, hwnd, u);
    invalidate(cpu, hwnd, false);
    true
}

/// Make `bud` the buddy (UPDOWN_SetBuddy), adding the moves to `s`:
/// the buddy before.
fn set_buddy(cpu: &mut Cpu, hwnd: u32, u: &mut UpDown, bud: u32, s: &mut Script) -> u32 {
    let old = u.buddy;
    if old != 0 {
        unsubclass(cpu, old);
    }
    let bud = if is_window(cpu, bud) { bud } else { 0 };
    u.buddy = bud;
    let flags = SWP_NOACTIVATE | SWP_NOZORDER;
    if bud == 0 {
        let Some(r) = user32::user(cpu).window(hwnd).map(|w| w.rect) else {
            return old;
        };
        s.user(
            "SetWindowPos",
            &[
                hwnd,
                0,
                r.left as u32,
                r.top as u32,
                DEFAULT_WIDTH as u32,
                r.height() as u32,
                flags | SWP_FRAMECHANGED,
            ],
        );
        return old;
    }
    u.buddy_type = {
        let st = user32::user(cpu);
        let name = class::of(st, bud).map(|c| c.name.to_ascii_lowercase());
        match name.as_deref() {
            Some("edit") => BUDDY_EDIT,
            Some("listbox") => BUDDY_LISTBOX,
            _ => BUDDY_UNKNOWN,
        }
    };
    if u.style & UDS_ARROWKEYS != 0 {
        subclass(cpu, bud, hwnd);
    }
    let Some(mut r) = user32::user(cpu).window(bud).map(|w| w.rect) else {
        return old;
    };
    let mut x;
    if u.style & UDS_ALIGNLEFT != 0 {
        x = r.left;
        r.left += DEFAULT_WIDTH;
    } else if u.style & UDS_ALIGNRIGHT != 0 {
        r.right -= DEFAULT_WIDTH;
        x = r.right;
    } else {
        return old;
    }
    s.user(
        "SetWindowPos",
        &[
            bud,
            0,
            r.left as u32,
            r.top as u32,
            r.width() as u32,
            r.height() as u32,
            flags,
        ],
    );
    let mut width = DEFAULT_WIDTH;
    if has_buddy_border(u) {
        if u.style & UDS_ALIGNLEFT != 0 {
            width += BUDDY_BORDER;
        } else {
            x -= BUDDY_BORDER;
        }
    }
    s.user(
        "SetWindowPos",
        &[
            hwnd,
            0,
            x as u32,
            r.top as u32,
            width as u32,
            r.height() as u32,
            flags | SWP_FRAMECHANGED,
        ],
    );
    old
}

/// Subclass the buddy for the arrow keys.
fn subclass(cpu: &mut Cpu, bud: u32, updown: u32) {
    let ours = super::address_of("UpDownBuddyProc");
    let Some(w) = user32::user(cpu).window_mut(bud) else {
        return;
    };
    if w.proc == ours {
        if let Some(b) = state(cpu).buddies.get_mut(&bud) {
            b.updown = updown;
        }
        return;
    }
    let proc = std::mem::replace(&mut w.proc, ours);
    state(cpu).buddies.insert(bud, Buddy { updown, proc });
}

/// Give the buddy its own procedure back.
fn unsubclass(cpu: &mut Cpu, bud: u32) {
    let Some(b) = state(cpu).buddies.remove(&bud) else {
        return;
    };
    let ours = super::address_of("UpDownBuddyProc");
    if let Some(w) = user32::user(cpu).window_mut(bud)
        && w.proc == ours
    {
        w.proc = b.proc;
    }
}

/// The procedure the buddy is subclassed with: the up and down keys
/// step the control, and the rest goes to the buddy's own procedure.
pub fn buddy_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    let Some(b) = state(cpu).buddies.get(&hwnd).cloned() else {
        return defwnd::def_window_proc(cpu, args);
    };
    let mut s = Script::new(b.updown);
    s.alloc(S_SIZE);
    let mut swallow = false;
    if msg == WM_KEYDOWN {
        key_pressed(cpu, b.updown, wp, &mut s);
        swallow = wp == VK_UP || wp == VK_DOWN;
    }
    if msg == WM_NCDESTROY {
        state(cpu).buddies.remove(&hwnd);
        if let Some(w) = user32::user(cpu).window_mut(hwnd) {
            w.proc = b.proc;
        }
    }
    if swallow {
        s.ret(0);
    } else {
        let f = user32::address_of("CallWindowProcA");
        s.call(
            f,
            &[
                Arg::V(b.proc),
                Arg::V(hwnd),
                Arg::V(msg),
                Arg::V(wp),
                Arg::V(lp),
            ],
        )
        .ret_last();
    }
    script::run(cpu, UPDOWN, s)
}

/// The up-down control's window procedure.
pub fn proc_(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    if msg == WM_CREATE {
        return create(cpu, hwnd, lp);
    }
    let Some(mut u) = get(cpu, hwnd) else {
        return defwnd::def_window_proc(cpu, args);
    };
    let mut s = Script::new(hwnd);
    s.alloc(S_SIZE);
    let result = match msg {
        WM_DESTROY => {
            if u.buddy != 0 {
                unsubclass(cpu, u.buddy);
            }
            state(cpu).updown.remove(&hwnd);
            0
        }
        WM_ENABLE => {
            if wp != 0 {
                u.style &= !WS_DISABLED;
                put(cpu, hwnd, u);
            } else {
                u.style |= WS_DISABLED;
                put(cpu, hwnd, u);
                cancel_mode(cpu, hwnd, &mut s);
            }
            invalidate(cpu, hwnd, false);
            0
        }
        WM_STYLECHANGED => {
            if wp == GWL_STYLE {
                let new = cpu.bus.guest_read_32(lp + 4);
                if cpu.bus.guest_faulted() {
                    return None;
                }
                u.style = new;
                put(cpu, hwnd, u);
                invalidate(cpu, hwnd, false);
            }
            0
        }
        WM_TIMER => {
            timer(cpu, hwnd, u, wp, &mut s);
            0
        }
        WM_CANCELMODE => cancel_mode(cpu, hwnd, &mut s) as u32,
        WM_LBUTTONUP => {
            if user32::input::get_capture(cpu, &[]) == Some(hwnd) {
                if u.flags & FLAG_MOUSEIN != 0 && u.flags & FLAG_ARROW != 0 {
                    let m = if u.style & UDS_HORZ != 0 {
                        WM_HSCROLL
                    } else {
                        WM_VSCROLL
                    };
                    let wp = SB_ENDSCROLL | (u.cur as u32) << 16;
                    s.send(u.notify, m, Arg::V(wp), Arg::V(hwnd));
                    if u.buddy_type == BUDDY_EDIT {
                        s.send(u.buddy, EM_SETSEL, Arg::V(0), Arg::V(0xFFFF_0000));
                    }
                }
                cancel_mode(cpu, hwnd, &mut s);
            }
            0
        }
        WM_LBUTTONDOWN | WM_MOUSEMOVE => {
            if is_enabled(cpu, hwnd, &u) {
                let (x, y) = (lp as i16 as i32, (lp >> 16) as i16 as i32);
                mouse(cpu, hwnd, u, msg, x, y, &mut s);
            }
            0
        }
        WM_MOUSEWHEEL => {
            let delta = (wp >> 16) as i16 as i32 / 120;
            if wp & 0x0C == 0 && delta != 0 {
                get_buddy_int(cpu, &mut s, hwnd, &mut u);
                put(cpu, hwnd, u);
                let arrow = if delta > 0 { FLAG_INCR } else { FLAG_DECR };
                s.then(T_ACTION, &[hwnd, delta.unsigned_abs(), arrow]);
            }
            1
        }
        WM_KEYDOWN => {
            if u.style & UDS_ARROWKEYS != 0 && is_enabled(cpu, hwnd, &u) {
                if !key_pressed(cpu, hwnd, wp, &mut s) {
                    return Some(1);
                }
                s.ret(0);
                return script::run(cpu, UPDOWN, s);
            }
            0
        }
        WM_PAINT | WM_PRINTCLIENT => {
            if wp != 0 {
                draw(cpu, hwnd, wp);
                return Some(0);
            }
            return script::run(cpu, UPDOWN, paint_script(hwnd));
        }
        UDM_GETACCEL => {
            if wp == 0 && lp == 0 {
                return Some(u.accel.len() as u32);
            }
            if wp != 0 && lp != 0 {
                let n = u.accel.len().min(wp as usize);
                let words: Vec<u32> = u.accel[..n].iter().flat_map(|&(a, b)| [a, b]).collect();
                write_words(cpu, lp, &words);
                if cpu.bus.guest_faulted() {
                    return None;
                }
                return Some(n as u32);
            }
            0
        }
        UDM_SETACCEL => {
            if wp == 0 {
                u.accel.clear();
                put(cpu, hwnd, u);
                return Some(1);
            }
            let n = (wp as usize).min(1024);
            let w = read_words(cpu, lp, 2 * n);
            if cpu.bus.guest_faulted() {
                return None;
            }
            u.accel = w.chunks(2).map(|c| (c[0], c[1])).collect();
            put(cpu, hwnd, u);
            1
        }
        UDM_GETBASE => u.base,
        UDM_SETBASE => {
            if wp != 10 && wp != 16 {
                return Some(0);
            }
            let old = u.base;
            u.base = wp;
            put(cpu, hwnd, u.clone());
            if old != wp {
                set_buddy_int(cpu, &mut s, &u);
            }
            old
        }
        UDM_GETBUDDY => u.buddy,
        UDM_SETBUDDY => {
            // Windows sets the style again on the way, telling the
            // window twice.
            let style = super::style(cpu, hwnd);
            s.user("SetWindowLongA", &[hwnd, GWL_STYLE, style]);
            s.user("SetWindowLongA", &[hwnd, GWL_STYLE, style]);
            let old = set_buddy(cpu, hwnd, &mut u, wp, &mut s);
            put(cpu, hwnd, u);
            old
        }
        UDM_GETPOS => {
            get_buddy_int(cpu, &mut s, hwnd, &mut u);
            put(cpu, hwnd, u);
            s.then(T_RET_POS, &[hwnd]);
            return script::run(cpu, UPDOWN, s);
        }
        UDM_SETPOS => {
            let mut v = lp as i16 as i32;
            if !in_bounds(&u, v) {
                v = v.max(u.min).min(u.max);
            }
            let old = u.cur;
            u.cur = v;
            put(cpu, hwnd, u.clone());
            set_buddy_int(cpu, &mut s, &u);
            old as u32
        }
        UDM_GETRANGE => (u.max as u32 & 0xFFFF) | (u.min as u32) << 16,
        UDM_SETRANGE => {
            u.max = lp as i16 as i32;
            u.min = (lp >> 16) as i16 as i32;
            put(cpu, hwnd, u);
            0
        }
        UDM_GETRANGE32 => {
            if wp != 0 {
                write_words(cpu, wp, &[u.min as u32]);
            }
            if lp != 0 {
                write_words(cpu, lp, &[u.max as u32]);
            }
            if cpu.bus.guest_faulted() {
                return None;
            }
            0
        }
        UDM_SETRANGE32 => {
            u.max = lp as i32;
            u.min = wp as i32;
            put(cpu, hwnd, u);
            0
        }
        UDM_GETPOS32 => {
            get_buddy_int(cpu, &mut s, hwnd, &mut u);
            put(cpu, hwnd, u);
            s.then(T_RET_POS32, &[hwnd, lp]);
            return script::run(cpu, UPDOWN, s);
        }
        UDM_SETPOS32 => {
            let mut v = lp as i32;
            if !in_bounds(&u, v) {
                v = v.max(u.min).min(u.max);
            }
            let old = u.cur;
            u.cur = v;
            put(cpu, hwnd, u.clone());
            set_buddy_int(cpu, &mut s, &u);
            old as u32
        }
        UDM_GETUNICODEFORMAT => u.unicode as u32,
        UDM_SETUNICODEFORMAT => {
            let old = u.unicode;
            u.unicode = wp != 0;
            put(cpu, hwnd, u);
            old as u32
        }
        _ => return defwnd::def_window_proc(cpu, args),
    };
    s.result = result;
    s.ret(result);
    script::run(cpu, UPDOWN, s)
}

fn timer(cpu: &mut Cpu, hwnd: u32, mut u: UpDown, id: u32, s: &mut Script) {
    use user32::message::{kill_timer, set_timer};
    if id == TIMER_AUTOPRESS {
        kill_timer(cpu, &[hwnd, TIMER_AUTOPRESS]);
        u.flags &= !(FLAG_PRESSED | FLAG_ARROW);
        invalidate(cpu, hwnd, false);
    }
    if id == TIMER_AUTOREPEAT {
        kill_timer(cpu, &[hwnd, TIMER_AUTOREPEAT]);
        let delay = if u.accel.is_empty() {
            u.accel_index = -1;
            REPEAT_DELAY
        } else {
            u.accel_index = 0;
            u.accel[0].0 * 1000 + 1
        };
        set_timer(cpu, &[hwnd, TIMER_ACCEL, delay, 0]);
    }
    if u.flags & FLAG_MOUSEIN != 0 {
        let inc = if u.accel_index < 0 {
            1
        } else {
            u.accel
                .get(u.accel_index as usize)
                .map_or(1, |a| a.1 as i32)
        };
        let ops = action_ops(cpu, hwnd, &u, inc, u.flags & FLAG_ARROW);
        s.ops.extend(ops);
        if u.accel_index >= 0 && (u.accel_index as usize) + 1 < u.accel.len() {
            kill_timer(cpu, &[hwnd, TIMER_ACCEL]);
            u.accel_index += 1;
            let delay = u.accel[u.accel_index as usize].0 * 1000 + 1;
            set_timer(cpu, &[hwnd, TIMER_ACCEL, delay, 0]);
        }
    }
    put(cpu, hwnd, u);
}

/// UPDOWN_HandleMouseEvent for a press and a move.
fn mouse(cpu: &mut Cpu, hwnd: u32, mut u: UpDown, msg: u32, x: i32, y: i32, s: &mut Script) {
    let arrow = arrow_at(cpu, hwnd, &u, x, y);
    if msg == WM_LBUTTONDOWN {
        if u.buddy_type == BUDDY_EDIT {
            s.user("SetFocus", &[u.buddy]);
        }
        u.flags &= !(FLAG_MOUSEIN | FLAG_ARROW);
        if arrow != 0 {
            u.flags |= FLAG_MOUSEIN | arrow;
        } else if u.accel_index != -1 {
            u.accel_index = 0;
        }
        if u.flags & FLAG_ARROW != 0 {
            get_buddy_int(cpu, s, hwnd, &mut u);
            u.flags |= FLAG_PRESSED;
            invalidate(cpu, hwnd, false);
            let inc = first_inc(&u);
            s.then(T_ACTION, &[hwnd, inc as u32, u.flags & FLAG_ARROW]);
            s.user("SetCapture", &[hwnd]);
            user32::message::set_timer(cpu, &[hwnd, TIMER_AUTOREPEAT, INITIAL_DELAY, 0]);
        }
    } else {
        let before = u.flags;
        u.flags &= !(FLAG_MOUSEIN | FLAG_ARROW);
        if arrow != 0 {
            u.flags |= FLAG_MOUSEIN | arrow;
        } else if u.accel_index != -1 {
            u.accel_index = 0;
        }
        if before != u.flags {
            invalidate(cpu, hwnd, false);
        }
    }
    put(cpu, hwnd, u);
}

/// WM_CREATE: the control's state, its width, and its buddy if it picks
/// one itself (UDS_AUTOBUDDY).
fn create(cpu: &mut Cpu, hwnd: u32, cs: u32) -> Option<u32> {
    let w = read_words(cpu, cs, 12);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let (cy, parent, style) = (w[4], w[3], w[8]);
    let mut u = UpDown {
        notify: parent,
        style,
        accel_index: -1,
        min: 100,
        max: 0,
        base: 10,
        flags: if style & UDS_SETBUDDYINT != 0 {
            FLAG_BUDDYINT
        } else {
            0
        },
        ..UpDown::default()
    };
    if let Some(win) = user32::user(cpu).window_mut(hwnd) {
        win.style &= !WS_BORDER;
    }
    let mut s = Script::new(hwnd);
    s.alloc(S_SIZE);
    s.send(parent, WM_NOTIFYFORMAT, Arg::V(hwnd), Arg::V(NF_QUERY));
    if style & UDS_HORZ == 0 {
        let flags = SWP_NOOWNERZORDER | SWP_NOZORDER | SWP_NOMOVE;
        s.user(
            "SetWindowPos",
            &[hwnd, 0, 0, 0, DEFAULT_WIDTH as u32, cy, flags],
        );
    }
    if style & UDS_AUTOBUDDY != 0 {
        let prev = user32::window::get_window(cpu, &[hwnd, GW_HWNDPREV]).unwrap_or(0);
        set_buddy(cpu, hwnd, &mut u, prev, &mut s);
    }
    put(cpu, hwnd, u);
    s.ret(0);
    script::run(cpu, UPDOWN, s)
}

/// CreateUpDownControl(style, x, y, cx, cy, parent, id, instance,
/// buddy, max, min, pos).
pub fn create_up_down_control(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut s = Script::new(0);
    let name = s.string(b"msctls_updown32");
    let f = user32::address_of("CreateWindowExA");
    let v = |i: usize| Arg::V(args[i]);
    s.call(
        f,
        &[
            Arg::V(0),
            Arg::S(name),
            Arg::V(0),
            v(0),
            v(1),
            v(2),
            v(3),
            v(4),
            v(5),
            v(6),
            v(7),
            Arg::V(0),
        ],
    );
    let range = (args[9] & 0xFFFF) | (args[10] & 0xFFFF) << 16;
    s.then(T_CREATED, &[args[8], range, args[11] & 0xFFFF]);
    script::run(cpu, UPDOWN, s)
}
