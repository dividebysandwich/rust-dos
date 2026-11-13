//! TrackMouseEvent, which USER32 has from Windows 98 on, and COMCTL32's
//! _TrackMouseEvent, which passes the call to it: a thread asks to be
//! told when the cursor leaves a window (`TME_LEAVE`, WM_MOUSELEAVE) or
//! stays still over it for a while (`TME_HOVER`, WM_MOUSEHOVER), in its
//! client area or, with `TME_NONCLIENT`, in the rest of it
//! (WM_NCMOUSELEAVE, WM_NCMOUSEHOVER).
//!
//! Each thread has one request at a time, kept in `Input::tracking`. A
//! request for the window it already tracks adds the flags to those it
//! has; one for another window replaces it. A request for a window the
//! cursor isn't on (or on the other part of it) tracks nothing, and with
//! `TME_LEAVE` posts the leave message at once. When the cursor leaves
//! the window, or the part tracked, the leave message is posted if it
//! was asked for, and the request ends, hover and all.
//!
//! The cursor hovers when it stays in a rectangle around the place it
//! was, SPI_GETMOUSEHOVERWIDTH by SPI_GETMOUSEHOVERHEIGHT pixels, for
//! the hover time: the request's, or SPI_GETMOUSEHOVERTIME's for
//! HOVER_DEFAULT. A move out of the rectangle starts it again at the
//! cursor. Once the time is over the window gets the hover message, and
//! the hover request ends, the leave one going on. The time is on the
//! emulated clock; the thread looks at it as it gets messages, and a
//! thread waiting for messages wakes for it, as for a timer.
//!
//! COMCTL32 on Windows 95 has no USER32 function to call, and makes the
//! messages with a timer of its own; the built-in COMCTL32 exports
//! _TrackMouseEvent where Windows 98's has it, and that always has
//! USER32's.

use super::input::{HTCLIENT, HTNOWHERE, hit_test};
use super::message::{self, Msg};
use super::{
    ERROR_INVALID_PARAMETER, ERROR_INVALID_WINDOW_HANDLE, FALSE, State, TRUE, current_tid,
    read_words, set_error, user, write_words,
};
use crate::cpu::Cpu;

/// TrackMouseEvent's flags.
pub const TME_HOVER: u32 = 1;
pub const TME_LEAVE: u32 = 2;
pub const TME_NONCLIENT: u32 = 0x10;
pub const TME_QUERY: u32 = 0x4000_0000;
pub const TME_CANCEL: u32 = 0x8000_0000;
/// The flags a request keeps.
const KEPT: u32 = TME_HOVER | TME_LEAVE | TME_NONCLIENT;

pub const WM_NCMOUSEHOVER: u32 = 0x2A0;
pub const WM_MOUSEHOVER: u32 = 0x2A1;
pub const WM_NCMOUSELEAVE: u32 = 0x2A2;
pub const WM_MOUSELEAVE: u32 = 0x2A3;

/// HOVER_DEFAULT: the system's hover time.
const HOVER_DEFAULT: u32 = 0xFFFF_FFFF;
/// TRACKMOUSEEVENT's size.
const TRACKMOUSEEVENT: u32 = 16;

/// A thread's request.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Tracking {
    /// The thread that asked, whose request it is.
    pub tid: u32,
    pub hwnd: u32,
    /// `TME_HOVER`, `TME_LEAVE` and `TME_NONCLIENT`.
    pub flags: u32,
    /// The hover time, in milliseconds.
    pub hover_ms: u32,
    /// The middle of the hover rectangle on the screen.
    pub origin: (i32, i32),
    /// When the hover time is over, in microseconds of the emulated
    /// clock; 0 without `TME_HOVER`.
    pub due: u64,
}

crate::state_fields!(Tracking {
    tid,
    hwnd,
    flags,
    hover_ms,
    origin,
    due
});

/// TrackMouseEvent(info) and COMCTL32's _TrackMouseEvent(info).
pub fn track_mouse_event(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let info = args[0];
    let [size, flags, hwnd, hover] = read_words(cpu, info, 4)[..] else {
        unreachable!()
    };
    if size != TRACKMOUSEEVENT {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let tid = current_tid(cpu);
    let mine = position(user(cpu), tid);
    if flags & TME_QUERY != 0 {
        // The request that would track what is tracked now.
        let words = match mine.map(|i| &user(cpu).input.tracking[i]) {
            Some(t) => [t.flags, t.hwnd, t.hover_ms],
            None => [0; 3],
        };
        write_words(cpu, info + 4, &words);
        return Some(TRUE);
    }
    if !user(cpu).is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    }
    if flags & TME_CANCEL != 0 {
        if let Some(i) = mine {
            let tracking = &mut user(cpu).input.tracking;
            if tracking[i].hwnd == hwnd {
                tracking[i].flags &= !(flags & (TME_HOVER | TME_LEAVE));
                if tracking[i].flags & TME_HOVER == 0 {
                    tracking[i].due = 0;
                }
                if tracking[i].flags & (TME_HOVER | TME_LEAVE) == 0 {
                    tracking.remove(i);
                }
            }
        }
        return Some(TRUE);
    }
    let (_, _, default_ms) = hover_metrics(cpu);
    let hover_ms = if hover == HOVER_DEFAULT || hover == 0 {
        default_ms
    } else {
        hover
    };
    let nonclient = flags & TME_NONCLIENT != 0;
    // The request it had for another window, or for the other part of
    // this one, ends here, and the cursor may have left that already.
    if let Some(i) = mine {
        let t = &user(cpu).input.tracking[i];
        if t.hwnd != hwnd || (t.flags & TME_NONCLIENT != 0) != nonclient {
            let old = user(cpu).input.tracking.remove(i);
            if old.flags & TME_LEAVE != 0 && !on_part(user(cpu), &old) {
                post_leave(cpu, &old);
            }
        }
    }
    let mut t = match position(user(cpu), tid) {
        Some(i) => user(cpu).input.tracking.remove(i),
        None => Tracking {
            tid,
            hwnd,
            ..Tracking::default()
        },
    };
    t.flags |= flags & KEPT;
    if !on_part(user(cpu), &t) {
        if flags & TME_LEAVE != 0 {
            post_leave(cpu, &t);
        }
        return Some(TRUE);
    }
    if flags & TME_HOVER != 0 {
        t.hover_ms = hover_ms;
        t.origin = user(cpu).input.cursor;
        t.due = cpu.bus.clock.now_micros() + hover_ms as u64 * 1000;
    }
    if t.flags & (TME_HOVER | TME_LEAVE) != 0 {
        user(cpu).input.tracking.push(t);
        // A thread waiting for messages wakes in time for the hover.
        crate::win32::thread::wake(cpu);
    }
    Some(TRUE)
}

/// The place of the request of the thread `tid`.
fn position(state: &State, tid: u32) -> Option<usize> {
    state.input.tracking.iter().position(|t| t.tid == tid)
}

/// The hover rectangle's width and height and the hover time, as
/// SystemParametersInfo has them.
pub fn hover_metrics(cpu: &mut Cpu) -> (u32, u32, u32) {
    use super::sysparams::param;
    (
        param(cpu, SPI_SETMOUSEHOVERWIDTH, 4),
        param(cpu, SPI_SETMOUSEHOVERHEIGHT, 4),
        param(cpu, SPI_SETMOUSEHOVERTIME, 400),
    )
}

/// SystemParametersInfo's actions for the hover: each GET action is the
/// SET one less one.
pub const SPI_SETMOUSEHOVERWIDTH: u32 = 0x63;
pub const SPI_SETMOUSEHOVERHEIGHT: u32 = 0x65;
pub const SPI_SETMOUSEHOVERTIME: u32 = 0x67;

/// What part of its window the cursor is on: HTCLIENT, another hit test
/// value, or HTNOWHERE when another window is under it.
fn part_under_cursor(state: &State, hwnd: u32) -> u32 {
    let (x, y) = state.input.cursor;
    if super::window::window_at(state, x, y) != hwnd {
        return HTNOWHERE;
    }
    hit_test(state, hwnd, x, y)
}

/// Whether the cursor is on the part of the window `t` tracks.
fn on_part(state: &State, t: &Tracking) -> bool {
    if !state.is_window(t.hwnd) {
        return false;
    }
    match part_under_cursor(state, t.hwnd) {
        HTNOWHERE => false,
        HTCLIENT => t.flags & TME_NONCLIENT == 0,
        _ => t.flags & TME_NONCLIENT != 0,
    }
}

/// Post the window of `t` its leave message.
fn post_leave(cpu: &mut Cpu, t: &Tracking) {
    let message = if t.flags & TME_NONCLIENT != 0 {
        WM_NCMOUSELEAVE
    } else {
        WM_MOUSELEAVE
    };
    post(cpu, t.hwnd, message, 0, 0);
}

fn post(cpu: &mut Cpu, hwnd: u32, message: u32, wparam: u32, lparam: u32) {
    let Some(tid) = user(cpu).window(hwnd).map(|w| w.tid) else {
        return;
    };
    let time = message::now_ms(cpu);
    let (x, y) = user(cpu).input.cursor;
    let msg = Msg {
        hwnd,
        message,
        wparam,
        lparam,
        time,
        x,
        y,
    };
    message::post(cpu, tid, msg);
}

/// The cursor moved, or the window under it changed: a request whose
/// window the cursor has left ends, with the leave message if it asked
/// for one, and a hover rectangle the cursor has left starts again
/// around it.
pub fn mouse_moved(cpu: &mut Cpu) {
    if user(cpu).input.tracking.is_empty() {
        return;
    }
    let now = cpu.bus.clock.now_micros();
    let (width, height, _) = hover_metrics(cpu);
    let tracking = std::mem::take(&mut user(cpu).input.tracking);
    let mut kept = Vec::with_capacity(tracking.len());
    for mut t in tracking {
        if !on_part(user(cpu), &t) {
            if t.flags & TME_LEAVE != 0 && user(cpu).is_window(t.hwnd) {
                post_leave(cpu, &t);
            }
            continue;
        }
        let (x, y) = user(cpu).input.cursor;
        let outside = (x - t.origin.0).unsigned_abs() > width / 2
            || (y - t.origin.1).unsigned_abs() > height / 2;
        if t.flags & TME_HOVER != 0 && outside {
            t.origin = (x, y);
            t.due = now + t.hover_ms as u64 * 1000;
        }
        kept.push(t);
    }
    // Messages posted on the way don't add requests, so the list is the
    // one taken.
    user(cpu).input.tracking = kept;
}

/// When the hover time of the thread `tid` is over, if it tracks the
/// hover.
pub fn hover_due(state: &State, tid: u32) -> Option<u64> {
    state
        .input
        .tracking
        .iter()
        .find(|t| t.tid == tid && t.flags & TME_HOVER != 0)
        .map(|t| t.due)
}

/// The thread `tid` looks for messages: if its hover time is over, the
/// window gets the hover message, and the hover request ends.
pub fn hover_tick(cpu: &mut Cpu, tid: u32) {
    let now = cpu.bus.clock.now_micros();
    if hover_due(user(cpu), tid).is_none_or(|due| due > now) {
        return;
    }
    let Some(i) = position(user(cpu), tid) else {
        return;
    };
    let state = user(cpu);
    let t = state.input.tracking[i].clone();
    if !on_part(state, &t) {
        // The cursor left without a move USER saw, as when a window
        // came over it: the request ends as for a move.
        mouse_moved(cpu);
        return;
    }
    let (x, y) = state.input.cursor;
    if t.flags & TME_NONCLIENT != 0 {
        let hit = part_under_cursor(state, t.hwnd);
        post(cpu, t.hwnd, WM_NCMOUSEHOVER, hit, super::make_long(x, y));
    } else {
        let keys = super::input::mk_flags(&state.input);
        let (cx, cy) = state.client_origin(t.hwnd);
        post(cpu, t.hwnd, WM_MOUSEHOVER, keys, super::make_long(x - cx, y - cy));
    }
    if let Some(i) = position(user(cpu), tid) {
        let tracking = &mut user(cpu).input.tracking;
        tracking[i].flags &= !TME_HOVER;
        tracking[i].due = 0;
        if tracking[i].flags & TME_LEAVE == 0 {
            tracking.remove(i);
        }
    }
}

/// The window `hwnd` is destroyed: its requests end without a message.
pub fn window_destroyed(state: &mut State, hwnd: u32) {
    state.input.tracking.retain(|t| t.hwnd != hwnd);
}
