//! What USER does about the foreground palette when the window that
//! kept one goes, on a palette device (a desktop of 256 colours, or an
//! 8-bit mode DirectDraw set).
//!
//! Windows 98 SE's USER.EXE marks a top-level window as one with a
//! palette (bit 0 of WND+2Eh, `HAS_PALETTE` here) when its SelectPalette
//! (segment 13 at 1EAEh) selects a palette other than DEFAULT_PALETTE
//! into a DC of the window or of a descendant on a palette device, in
//! the foreground or not. When it first marks a window other than the
//! desktop it marks the running task too (bit 8 of the task's flags).
//! Neither mark is taken off.
//!
//! Its function at segment 11, 195Eh (`restore_default_palette`) is
//! called:
//!
//! - by DestroyWindow (11:19E4h), when the window is the active one, is
//!   marked and isn't a popup with an owner, after the next window is
//!   activated: with that window and the one destroyed;
//! - by ShowWindow when it minimizes a marked top-level window (segment
//!   13 at 31F0h): with the window SW_MINIMIZE activates (none with the
//!   other commands) and the one minimized;
//! - when a marked task ends (segment 16 at 2134h, which SignalProc32
//!   calls), after its windows went: with no windows. DestroyWindow
//!   doesn't call it for the windows of a task that is ending.
//!
//! On a palette device, when the window it is given (with none, the
//! active window) is none, the desktop, a window that isn't marked or a
//! minimized one (WS_MINIMIZE, bit 20h of WND+33h), it calls
//! RealizeDefaultPalette (GDI.365) on USER's screen DC, realizes the
//! wallpaper's palette again if there is one (the built-in Windows has
//! no wallpaper), and sends WM_PALETTECHANGED to the top-level windows,
//! its wParam the window that went, or the desktop when a task ended.
//! Otherwise the window it was given keeps its palette, which it
//! realizes when it gets WM_QUERYNEWPALETTE on its activation.

use super::window::{HAS_PALETTE, WS_MINIMIZE, WS_POPUP};
use super::{DESKTOP, State, current_tid, user};
use crate::cpu::Cpu;
use crate::win32::gdi32;

/// The W32 version whose sections have `State::palette_threads`.
pub const PALETTE_SAVED: u16 = 97;

/// SelectPalette selected a palette other than DEFAULT_PALETTE into a DC
/// of the window `hwnd` (0 for a DC of no window) on a palette device:
/// its top-level window is one with a palette, and when that isn't the
/// desktop and wasn't one before, so is the running thread.
pub fn selected(cpu: &mut Cpu, hwnd: u32) {
    let tid = current_tid(cpu);
    let state = user(cpu);
    if !state.is_window(hwnd) {
        return;
    }
    let top = state.top_level(hwnd);
    let Some(w) = state.window_mut(top) else {
        return;
    };
    if w.flags & HAS_PALETTE != 0 {
        return;
    }
    w.flags |= HAS_PALETTE;
    if top != DESKTOP && tid != 0 && !state.palette_threads.contains(&tid) {
        state.palette_threads.push(tid);
    }
}

/// Whether DestroyWindow of `hwnd`, the active window, is to call
/// `restore_default_palette` once the next window is active: it has a
/// palette and isn't a popup with an owner.
pub fn destroy_restores(state: &State, hwnd: u32) -> bool {
    state
        .window(hwnd)
        .is_some_and(|w| w.flags & HAS_PALETTE != 0 && !(w.style & WS_POPUP != 0 && w.owner != 0))
}

/// Whether the window `active`, which is to have the foreground, keeps
/// a palette there: one with a palette that isn't the desktop and isn't
/// minimized.
fn keeps_palette(state: &State, active: u32) -> bool {
    active != DESKTOP
        && state
            .window(active)
            .is_some_and(|w| w.flags & HAS_PALETTE != 0 && w.style & WS_MINIMIZE == 0)
}

/// RestoreDefaultPalette(hwndActive, hwndGone), an internal function:
/// `hwndGone`, a window with a palette, was destroyed or minimized and
/// `hwndActive` (0 for the foreground window) is active now. On a
/// palette device, unless that window keeps a palette
/// (`keeps_palette`), DEFAULT_PALETTE becomes the primary palette
/// (`gdi32::palette::realize_default`) and the top-level windows get
/// WM_PALETTECHANGED with `hwndGone` (0 for the desktop) as wParam.
pub fn restore_default_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (active, gone) = (args[0], args[1]);
    if !gdi32::palette::palette_device(cpu) {
        return Some(0);
    }
    let state = user(cpu);
    let active = match active {
        0 => state.foreground_window(),
        h => h,
    };
    if keeps_palette(state, active) {
        return Some(0);
    }
    gdi32::palette::realize_default(cpu);
    let from = if gone == 0 { DESKTOP } else { gone };
    let vars = vec![gdi32::CB_PALETTE_CHANGED, 0, from, 0];
    crate::win32::task::start(cpu, gdi32::TASK_CALLBACK, vars, 0)
}

/// The threads `tids` end, their windows gone from `state` and the
/// foreground window, if one of theirs, not yet replaced: if one of
/// them was one with a palette, `task_ended` is to judge by the
/// foreground window as it is now.
pub fn threads_ending(state: &mut State, tids: &[u32]) {
    let before = state.palette_threads.len();
    state.palette_threads.retain(|t| !tids.contains(t));
    if state.palette_threads.len() != before {
        state.palette_exit = Some(state.foreground_window());
    }
}

/// A task or process that was one with a palette ended
/// (`threads_ending`): what `restore_default_palette` does, with the
/// active window it had then, and the top-level windows get
/// WM_PALETTECHANGED in their queues, as no thread runs to send it.
/// Called once the display is the desktop's again, after DirectDraw gave
/// back the mode the process set.
pub fn task_ended(cpu: &mut Cpu) {
    let Some(active) = user(cpu).palette_exit.take() else {
        return;
    };
    if !gdi32::palette::palette_device(cpu) || keeps_palette(user(cpu), active) {
        return;
    }
    gdi32::palette::realize_default(cpu);
    use super::message::{HWND_BROADCAST, WM_PALETTECHANGED, post_message};
    post_message(cpu, &[HWND_BROADCAST, WM_PALETTECHANGED, DESKTOP, 0]);
}
