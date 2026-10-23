//! The progress bar (msctls_progress32): a bar of blocks, or a smooth
//! one with PBS_SMOOTH, across or up (PBS_VERTICAL). A new position
//! paints at once, through UpdateWindow, and the background is erased
//! only when the bar gets shorter.
//!
//! Ported from Wine's progress.c:
//!
//! Copyright 1997, 2002 Dimitrie O. Paun
//! Copyright 1998, 1999 Eric Kohl
//!
//! This library is free software; you can redistribute it and/or modify
//! it under the terms of the GNU Lesser General Public License as
//! published by the Free Software Foundation; either version 2.1 of the
//! License, or (at your option) any later version.

use super::gfx::{self, CLR_DEFAULT, COLOR_BTNFACE, COLOR_HIGHLIGHT};
use super::script::{Ctx, Op, Script};
use super::{DRAW, PROGRESS, invalidate, paint_script, state};
use crate::cpu::Cpu;
use crate::win32::gdi32::Rect;
use crate::win32::user32::message::{
    WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_GETFONT, WM_PAINT, WM_SETFONT, WM_TIMER,
};
use crate::win32::user32::window::{
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_CLIENTEDGE,
    WS_EX_STATICEDGE, WS_EX_WINDOWEDGE,
};
use crate::win32::user32::{self, defwnd};

const WM_PRINTCLIENT: u32 = 0x0318;

const PBM_SETRANGE: u32 = 0x0401;
const PBM_SETPOS: u32 = 0x0402;
const PBM_DELTAPOS: u32 = 0x0403;
const PBM_SETSTEP: u32 = 0x0404;
const PBM_STEPIT: u32 = 0x0405;
const PBM_SETRANGE32: u32 = 0x0406;
const PBM_GETRANGE: u32 = 0x0407;
const PBM_GETPOS: u32 = 0x0408;
const PBM_SETBARCOLOR: u32 = 0x0409;
const PBM_SETMARQUEE: u32 = 0x040A;
const PBM_GETBKCOLOR: u32 = 0x040E;
const PBM_GETBARCOLOR: u32 = 0x040F;
const PBM_SETBKCOLOR: u32 = 0x2001;

const PBS_SMOOTH: u32 = 0x01;
const PBS_VERTICAL: u32 = 0x04;
const PBS_MARQUEE: u32 = 0x08;

const LED_GAP: i32 = 2;
const MARQUEE_LEDS: i32 = 5;
const ID_MARQUEE_TIMER: u32 = 1;

/// What a progress bar keeps.
#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub pos: i32,
    pub min: i32,
    pub max: i32,
    pub step: i32,
    pub marquee_pos: i32,
    pub marquee: bool,
    pub bar_color: u32,
    pub bk_color: u32,
    pub font: u32,
}

crate::state_fields!(Progress {
    pos,
    min,
    max,
    step,
    marquee_pos,
    marquee,
    bar_color,
    bk_color,
    font
});

fn bar_size(style: u32, r: Rect) -> i32 {
    if style & PBS_VERTICAL != 0 {
        r.height()
    } else {
        r.width()
    }
}

fn led_size(style: u32, r: Rect) -> i32 {
    if style & PBS_VERTICAL != 0 {
        gfx::mul_div(r.width(), 2, 3)
    } else {
        gfx::mul_div(r.height(), 2, 3)
    }
}

/// Paint the bar on `hdc` (PROGRESS_Draw).
pub fn draw(cpu: &mut Cpu, hwnd: u32, hdc: u32) {
    let Some(p) = state(cpu).progress.get(&hwnd).cloned() else {
        return;
    };
    let style = super::style(cpu, hwnd);
    let bar = if p.bar_color == CLR_DEFAULT {
        gfx::sys_brush(cpu, COLOR_HIGHLIGHT)
    } else {
        crate::win32::gdi32::create_solid_brush(cpu, &[p.bar_color]).unwrap_or(0)
    };
    let bk = if p.bk_color == CLR_DEFAULT {
        gfx::sys_brush(cpu, COLOR_BTNFACE)
    } else {
        crate::win32::gdi32::create_solid_brush(cpu, &[p.bk_color]).unwrap_or(0)
    };
    let mut r = super::client(cpu, hwnd);
    gfx::frame(cpu, hdc, r, bk);
    r = r.inflate(-1, -1);
    let smooth = style & PBS_SMOOTH != 0;
    let vertical = style & PBS_VERTICAL != 0;
    let size = bar_size(style, r);
    let led = if smooth { 0 } else { led_size(style, r) };
    // A run of the bar, from `start` to `end` along it.
    let part = |cpu: &mut Cpu, start: i32, end: i32, filled: bool| {
        let span = |a: i32, b: i32| {
            if vertical {
                Rect::new(r.left, r.bottom - b, r.right, r.bottom - a)
            } else {
                Rect::new(r.left + a, r.top, r.left + b, r.bottom)
            }
        };
        if !filled || smooth {
            gfx::fill(cpu, hdc, span(start, end), if filled { bar } else { bk });
            return;
        }
        let mut a = start;
        while a < end {
            let b = (a + led).min(end);
            gfx::fill(cpu, hdc, span(a, b), bar);
            let g = (b + LED_GAP).min(end);
            gfx::fill(cpu, hdc, span(b, g), bk);
            a = g;
        }
    };
    if style & PBS_MARQUEE != 0 {
        let w = if smooth { 1 } else { led + LED_GAP };
        let leds = (size + w - 1) / w;
        let end = p.marquee_pos + MARQUEE_LEDS;
        if end > leds {
            let gap_start = ((end - leds) * w).max(0);
            let gap_end = (p.marquee_pos * w).min(size);
            part(cpu, 0, gap_start, true);
            part(cpu, gap_start, gap_end, false);
            part(cpu, gap_end, size, true);
        } else {
            let start = p.marquee_pos * w;
            let stop = (end * w).min(size);
            part(cpu, 0, start, false);
            part(cpu, start, stop, true);
            part(cpu, stop, size, false);
        }
    } else {
        let mut end = if p.max == p.min {
            0
        } else {
            gfx::mul_div(p.pos - p.min, size, p.max - p.min)
        };
        if !smooth {
            let w = led + LED_GAP;
            end = (((end + w - 1) / w) * w).min(size);
        }
        part(cpu, 0, end, true);
        part(cpu, end, size, false);
    }
    if p.bar_color != CLR_DEFAULT {
        crate::win32::gdi32::delete_object(cpu, &[bar]);
    }
    if p.bk_color != CLR_DEFAULT {
        crate::win32::gdi32::delete_object(cpu, &[bk]);
    }
}

/// The continuations: painting.
pub fn then(cpu: &mut Cpu, id: u16, c: &mut Ctx) -> Vec<Op> {
    if id == DRAW && c.last != 0 {
        draw(cpu, c.hwnd, c.last);
    }
    Vec::new()
}

/// Repaint after the position went from `old` to `new`, erasing when
/// the bar gets shorter, and return `result`.
fn moved(cpu: &mut Cpu, hwnd: u32, old: i32, new: i32, result: u32) -> Option<u32> {
    invalidate(cpu, hwnd, old > new);
    let mut s = Script::new(hwnd);
    s.user("UpdateWindow", &[hwnd]).ret(result);
    super::script::run(cpu, PROGRESS, s)
}

fn coerce(p: &mut Progress) {
    if p.pos < p.min {
        p.pos = p.min;
    }
    if p.pos > p.max {
        p.pos = p.max;
    }
}

/// The progress bar's window procedure.
pub fn proc_(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hwnd, msg, wp, lp] = args else {
        unreachable!()
    };
    if msg == WM_CREATE {
        return create(cpu, hwnd);
    }
    let Some(mut p) = state(cpu).progress.get(&hwnd).cloned() else {
        return defwnd::def_window_proc(cpu, args);
    };
    let put = |cpu: &mut Cpu, p: Progress| {
        state(cpu).progress.insert(hwnd, p);
    };
    match msg {
        WM_DESTROY => {
            state(cpu).progress.remove(&hwnd);
            Some(0)
        }
        WM_ERASEBKGND => Some(1),
        WM_GETFONT => Some(p.font),
        WM_SETFONT => {
            let old = std::mem::replace(&mut p.font, wp);
            put(cpu, p);
            Some(old)
        }
        WM_PAINT | WM_PRINTCLIENT => {
            if wp != 0 {
                draw(cpu, hwnd, wp);
                return Some(0);
            }
            super::script::run(cpu, PROGRESS, paint_script(hwnd))
        }
        WM_TIMER => {
            if wp != ID_MARQUEE_TIMER {
                return Some(0);
            }
            let style = super::style(cpu, hwnd);
            let r = super::client(cpu, hwnd).inflate(-1, -1);
            let w = if style & PBS_SMOOTH != 0 {
                1
            } else {
                led_size(style, r) + LED_GAP
            };
            let leds = (bar_size(style, r) + w - 1) / w.max(1);
            p.marquee_pos += 1;
            if p.marquee_pos >= leds {
                p.marquee_pos = 0;
            }
            put(cpu, p);
            invalidate(cpu, hwnd, false);
            let mut s = Script::new(hwnd);
            s.user("UpdateWindow", &[hwnd]).ret(0);
            super::script::run(cpu, PROGRESS, s)
        }
        PBM_DELTAPOS => {
            let old = p.pos;
            if wp == 0 {
                return Some(old as u32);
            }
            p.pos = p.pos.wrapping_add(wp as i32);
            coerce(&mut p);
            let new = p.pos;
            put(cpu, p);
            moved(cpu, hwnd, old, new, old as u32)
        }
        PBM_SETPOS => {
            let old = p.pos;
            if old as u32 == wp {
                return Some(old as u32);
            }
            p.pos = wp as i32;
            coerce(&mut p);
            let new = p.pos;
            put(cpu, p);
            moved(cpu, hwnd, old, new, old as u32)
        }
        PBM_SETRANGE => Some(set_range(
            cpu,
            hwnd,
            p,
            lp as u16 as i32,
            (lp >> 16) as u16 as i32,
        )),
        PBM_SETRANGE32 => Some(set_range(cpu, hwnd, p, wp as i32, lp as i32)),
        PBM_SETSTEP => {
            let old = std::mem::replace(&mut p.step, wp as i32);
            put(cpu, p);
            Some(old as u32)
        }
        PBM_STEPIT => {
            let old = p.pos;
            p.pos = p.pos.wrapping_add(p.step);
            if p.pos > p.max {
                p.pos = p.min;
            }
            let new = p.pos;
            put(cpu, p);
            if old == new {
                return Some(old as u32);
            }
            moved(cpu, hwnd, old, new, old as u32)
        }
        PBM_GETRANGE => {
            if lp != 0 {
                user32::write_words(cpu, lp, &[p.min as u32, p.max as u32]);
            }
            Some(if wp != 0 { p.min } else { p.max } as u32)
        }
        PBM_GETPOS => Some(p.pos as u32),
        PBM_SETBARCOLOR => {
            p.bar_color = lp;
            put(cpu, p);
            invalidate(cpu, hwnd, true);
            Some(0)
        }
        PBM_GETBARCOLOR => Some(p.bar_color),
        PBM_SETBKCOLOR => {
            p.bk_color = lp;
            put(cpu, p);
            invalidate(cpu, hwnd, true);
            Some(0)
        }
        PBM_GETBKCOLOR => Some(p.bk_color),
        PBM_SETMARQUEE => {
            p.marquee = wp != 0;
            let on = p.marquee;
            put(cpu, p);
            if on {
                user32::message::set_timer(cpu, &[hwnd, ID_MARQUEE_TIMER, lp, 0]);
            } else {
                user32::message::kill_timer(cpu, &[hwnd, ID_MARQUEE_TIMER]);
            }
            Some(on as u32)
        }
        _ => defwnd::def_window_proc(cpu, args),
    }
}

/// PBM_SETRANGE and PBM_SETRANGE32: the range before, as two words.
fn set_range(cpu: &mut Cpu, hwnd: u32, mut p: Progress, low: i32, high: i32) -> u32 {
    let old = (p.min as u16 as u32) | (p.max as u16 as u32) << 16;
    if p.min == low && p.max == high {
        return old;
    }
    p.min = low;
    p.max = high;
    coerce(&mut p);
    state(cpu).progress.insert(hwnd, p);
    invalidate(cpu, hwnd, true);
    old
}

/// WM_CREATE: a static edge for the frame, and the bar from 0 to 100.
fn create(cpu: &mut Cpu, hwnd: u32) -> Option<u32> {
    if let Some(w) = user32::user(cpu).window_mut(hwnd) {
        w.ex_style &= !(WS_EX_CLIENTEDGE | WS_EX_WINDOWEDGE);
        w.ex_style |= WS_EX_STATICEDGE;
    }
    state(cpu).progress.insert(
        hwnd,
        Progress {
            max: 100,
            step: 10,
            bar_color: CLR_DEFAULT,
            bk_color: CLR_DEFAULT,
            ..Progress::default()
        },
    );
    let flags = SWP_FRAMECHANGED | SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
    let mut s = Script::new(hwnd);
    s.user("SetWindowPos", &[hwnd, 0, 0, 0, 0, 0, flags]).ret(0);
    super::script::run(cpu, PROGRESS, s)
}
