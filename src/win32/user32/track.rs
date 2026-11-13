//! TrackMouseEvent, which USER32 has from Windows 98 on, and COMCTL32's
//! _TrackMouseEvent, which passes the call to it: a program asks to be
//! told when the cursor leaves a window (`TME_LEAVE`, WM_MOUSELEAVE) or
//! stays still over it for a while (`TME_HOVER`, WM_MOUSEHOVER), in its
//! client area or, with `TME_NONCLIENT`, in the rest of it
//! (WM_NCMOUSELEAVE, WM_NCMOUSEHOVER).
//!
//! There is one request for the desktop, kept in `Input::tracking`, and
//! it is for the window under the cursor and the part of it the cursor
//! is on, as Windows keeps it (ReactOS's IntTrackMouseEvent has it with
//! the desktop's spwndTrack, Wine's TrackMouseEvent with one
//! tracking_info). A request for that window and part adds its flags to
//! those the request has. A request for any other window, or for the
//! other part, posts the leave message at once if it asks for one and
//! leaves the request as it was. When the cursor leaves the window or
//! the part, the leave message is posted if it was asked for, and the
//! request ends, hover and all. The request ends without a message when
//! its window is destroyed.
//!
//! The cursor hovers when it stays in a rectangle around the place it
//! was, SPI_GETMOUSEHOVERWIDTH by SPI_GETMOUSEHOVERHEIGHT pixels, for
//! the hover time: the request's, or SPI_GETMOUSEHOVERTIME's for
//! HOVER_DEFAULT. A move out of the rectangle starts it again at the
//! cursor. Once the time is over the window gets the hover message, and
//! the hover request ends, the leave one going on. The time is on the
//! emulated clock, and is the window's thread's: the thread looks at it
//! as it gets messages, and a thread waiting for messages wakes for it,
//! as for a timer.
//!
//! COMCTL32 has _TrackMouseEvent from version 4.70 on (Windows NT 4.0,
//! Windows 95 OSR2, Internet Explorer 3.01), which is the version the
//! built-in one gives, so it is there under every version of Windows.
//! On a Windows 95 without USER32's function it makes the messages with
//! a timer of its own; the built-in one has this file's under each.

use super::input::{HTCLIENT, HTNOWHERE, hit_test};
use super::message::{self, Msg};
use super::{
    ERROR_INVALID_FLAGS, ERROR_INVALID_PARAMETER, ERROR_INVALID_WINDOW_HANDLE, FALSE, State, TRUE,
    current_tid, read_words, set_error, user, write_words,
};
use crate::cpu::Cpu;

/// TrackMouseEvent's flags.
pub const TME_HOVER: u32 = 1;
pub const TME_LEAVE: u32 = 2;
pub const TME_NONCLIENT: u32 = 0x10;
pub const TME_QUERY: u32 = 0x4000_0000;
pub const TME_CANCEL: u32 = 0x8000_0000;
/// The flags TrackMouseEvent knows; any other fails the call.
const KNOWN: u32 = TME_HOVER | TME_LEAVE | TME_NONCLIENT | TME_QUERY | TME_CANCEL;

pub const WM_NCMOUSEHOVER: u32 = 0x2A0;
pub const WM_MOUSEHOVER: u32 = 0x2A1;
pub const WM_NCMOUSELEAVE: u32 = 0x2A2;
pub const WM_MOUSELEAVE: u32 = 0x2A3;

/// HOVER_DEFAULT: the system's hover time.
const HOVER_DEFAULT: u32 = 0xFFFF_FFFF;
/// TRACKMOUSEEVENT's size.
const TRACKMOUSEEVENT: u32 = 16;

/// The desktop's request.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Tracking {
    /// The window under the cursor when the request was made.
    pub hwnd: u32,
    /// `TME_HOVER` and `TME_LEAVE`, with `TME_NONCLIENT` for the part
    /// outside the client area.
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
    hwnd,
    flags,
    hover_ms,
    origin,
    due
});

/// TrackMouseEvent(info) and COMCTL32's _TrackMouseEvent(info). A wrong
/// size fails with ERROR_INVALID_PARAMETER and an unknown flag with
/// ERROR_INVALID_FLAGS, as NT's and ReactOS's NtUserTrackMouseEvent do.
pub fn track_mouse_event(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let info = args[0];
    let [size, flags, hwnd, hover] = read_words(cpu, info, 4)[..] else {
        unreachable!()
    };
    if size != TRACKMOUSEEVENT {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    if flags & !KNOWN != 0 {
        set_error(cpu, ERROR_INVALID_FLAGS);
        return Some(FALSE);
    }
    if flags & TME_QUERY != 0 {
        let words = query(cpu);
        write_words(cpu, info, &words);
        return Some(TRUE);
    }
    if !user(cpu).is_window(hwnd) {
        set_error(cpu, ERROR_INVALID_WINDOW_HANDLE);
        return Some(FALSE);
    }
    let asked = Tracking {
        hwnd,
        flags: flags & TME_NONCLIENT,
        ..Tracking::default()
    };
    if !on_part(user(cpu), &asked) {
        // Not the window under the cursor, or not the part it is on: the
        // request the desktop has goes on as it was.
        if flags & (TME_LEAVE | TME_CANCEL) == TME_LEAVE {
            post_leave(cpu, &asked);
        }
        return Some(TRUE);
    }
    // A request for another window or part is one the cursor left
    // without a move USER saw, as when this window came over the other.
    if user(cpu)
        .input
        .tracking
        .as_ref()
        .is_some_and(|t| t.hwnd != hwnd || (t.flags ^ flags) & TME_NONCLIENT != 0)
    {
        end(cpu);
    }
    let mut t = user(cpu).input.tracking.take().unwrap_or(asked);
    if flags & TME_CANCEL != 0 {
        t.flags &= !(flags & (TME_HOVER | TME_LEAVE));
        if t.flags & TME_HOVER == 0 {
            t.due = 0;
        }
    } else {
        t.flags |= flags & (TME_HOVER | TME_LEAVE);
        if flags & TME_HOVER != 0 {
            let (_, _, default_ms) = hover_metrics(cpu);
            t.hover_ms = if hover == HOVER_DEFAULT || hover == 0 {
                default_ms
            } else {
                hover
            };
            t.origin = user(cpu).input.cursor;
            t.due = cpu.bus.clock.now_micros() + t.hover_ms as u64 * 1000;
        }
    }
    if t.flags & (TME_HOVER | TME_LEAVE) != 0 {
        let tid = current_tid(cpu);
        let other_thread = user(cpu).window(hwnd).is_some_and(|w| w.tid != tid);
        user(cpu).input.tracking = Some(t);
        // The hover time is the window's thread's. When that is another
        // thread, it may be waiting for messages until a time that
        // doesn't know of the hover: it looks again.
        if other_thread {
            crate::win32::thread::wake(cpu);
        }
    }
    Some(TRUE)
}

/// TME_QUERY: the TRACKMOUSEEVENT that would make the desktop's request,
/// for a thread whose window it is; otherwise only the size, as ReactOS's
/// IntQueryTrackMouseEvent has it. The hover time is there only with
/// `TME_HOVER`.
fn query(cpu: &mut Cpu) -> [u32; 4] {
    let tid = current_tid(cpu);
    let state = user(cpu);
    let Some(t) = state.input.tracking.as_ref() else {
        return [TRACKMOUSEEVENT, 0, 0, 0];
    };
    if state.window(t.hwnd).is_none_or(|w| w.tid != tid) {
        return [TRACKMOUSEEVENT, 0, 0, 0];
    }
    let hover = if t.flags & TME_HOVER != 0 {
        t.hover_ms
    } else {
        0
    };
    [TRACKMOUSEEVENT, t.flags, t.hwnd, hover]
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

/// Whether the screen point (`x`, `y`) is in the hover rectangle around
/// `origin`. The rectangle runs from half its size before the middle to
/// half after it, the right and bottom edges outside it, as ReactOS's
/// rcMouseHover is with RECTL_bPointInRect: `width` pixels across.
fn in_hover_rect(origin: (i32, i32), (x, y): (i32, i32), width: u32, height: u32) -> bool {
    let (w, h) = ((width / 2) as i32, (height / 2) as i32);
    (origin.0 - w..origin.0 + w).contains(&x) && (origin.1 - h..origin.1 + h).contains(&y)
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
    let msg = Msg {
        hwnd,
        message,
        wparam,
        lparam,
        ..Msg::default()
    };
    let msg = message::stamp(cpu, msg);
    message::post(cpu, tid, msg);
}

/// The request ends, as the cursor left its window or part: the leave
/// message goes out if it was asked for.
fn end(cpu: &mut Cpu) {
    // The request is gone before the message goes out: posting wakes the
    // threads, which reads the mouse, and a move found there comes back
    // to `mouse_moved`.
    let Some(t) = user(cpu).input.tracking.take() else {
        return;
    };
    if t.flags & TME_LEAVE != 0 {
        post_leave(cpu, &t);
    }
}

/// The cursor moved, or the window under it changed: a request whose
/// window or part the cursor has left ends, with the leave message if it
/// asked for one, and a hover rectangle the cursor has left starts again
/// around it.
pub fn mouse_moved(cpu: &mut Cpu) {
    let now = cpu.bus.clock.now_micros();
    let (width, height, _) = hover_metrics(cpu);
    let state = user(cpu);
    let cursor = state.input.cursor;
    let Some(t) = state.input.tracking.as_ref() else {
        return;
    };
    if !on_part(state, t) {
        end(cpu);
        return;
    }
    let t = state.input.tracking.as_mut().unwrap();
    if t.flags & TME_HOVER != 0 && !in_hover_rect(t.origin, cursor, width, height) {
        t.origin = cursor;
        t.due = now + t.hover_ms as u64 * 1000;
    }
}

/// When the hover time of the thread `tid` is over, if the desktop's
/// request tracks the hover of one of its windows.
pub fn hover_due(state: &State, tid: u32) -> Option<u64> {
    let t = state.input.tracking.as_ref()?;
    let mine = state.window(t.hwnd).is_some_and(|w| w.tid == tid);
    (mine && t.flags & TME_HOVER != 0).then_some(t.due)
}

/// The thread `tid` looks for messages: if the hover time of its window
/// is over, the window gets the hover message, and the hover request
/// ends.
pub fn hover_tick(cpu: &mut Cpu, tid: u32) {
    let now = cpu.bus.clock.now_micros();
    if hover_due(user(cpu), tid).is_none_or(|due| due > now) {
        return;
    }
    let state = user(cpu);
    let Some(t) = state.input.tracking.clone() else {
        return;
    };
    if !on_part(state, &t) {
        // The cursor left without a move USER saw: the request ends as
        // for a move.
        end(cpu);
        return;
    }
    // The hover request ends before the message goes out (see `end`).
    let tracking = &mut state.input.tracking;
    if t.flags & TME_LEAVE == 0 {
        *tracking = None;
    } else if let Some(left) = tracking.as_mut() {
        left.flags &= !TME_HOVER;
        left.due = 0;
    }
    let (x, y) = state.input.cursor;
    if t.flags & TME_NONCLIENT != 0 {
        let hit = part_under_cursor(state, t.hwnd);
        post(cpu, t.hwnd, WM_NCMOUSEHOVER, hit, super::make_long(x, y));
    } else {
        let keys = super::input::mk_flags(&state.input);
        let (cx, cy) = state.client_origin(t.hwnd);
        let place = super::make_long(x - cx, y - cy);
        post(cpu, t.hwnd, WM_MOUSEHOVER, keys, place);
    }
}

/// The window `hwnd` is destroyed: a request for it ends without a
/// message.
pub fn window_destroyed(state: &mut State, hwnd: u32) {
    if state
        .input
        .tracking
        .as_ref()
        .is_some_and(|t| t.hwnd == hwnd)
    {
        state.input.tracking = None;
    }
}

#[cfg(test)]
mod tests {
    use super::in_hover_rect;

    /// The rectangle is `width` pixels across: two left of the middle
    /// and one right of it for 4, the pixel two right outside.
    #[test]
    fn the_hover_rectangle_is_its_size_across() {
        let o = (100, 100);
        assert!(in_hover_rect(o, (98, 98), 4, 4));
        assert!(in_hover_rect(o, (101, 101), 4, 4));
        assert!(!in_hover_rect(o, (102, 100), 4, 4));
        assert!(!in_hover_rect(o, (100, 102), 4, 4));
        assert!(!in_hover_rect(o, (97, 100), 4, 4));
    }
}
