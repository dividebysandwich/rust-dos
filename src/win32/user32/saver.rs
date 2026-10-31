//! The screen saver's timer. When no key or mouse input has come for
//! WIN.INI's `[windows] ScreenSaveTimeOut` seconds, with
//! `ScreenSaveActive=1`, USER sends the active window WM_SYSCOMMAND
//! SC_SCREENSAVE, and DefWindowProc starts SYSTEM.INI's `[boot]
//! SCRNSAVE.EXE` with /s. Windows 3.1 does this, and Windows 95 and 98
//! do it with the same entries. USER reads them when Windows starts:
//! here, when the desktop comes.
//!
//! The timer is a time on the emulated clock, `Input::saver_due`, which
//! the built-in Windows's timer event (`win32::timer`) waits for. Input
//! only notes its time; when the timer's time comes and input came
//! since it was set, it is set again from that input. Once the saver
//! was asked for, the timer waits for input before it starts again.

use super::{message, user};
use crate::cpu::Cpu;

/// WM_SYSCOMMAND's command that starts the screen saver.
pub const SC_SCREENSAVE: u32 = 0xF140;
const WM_SYSCOMMAND: u32 = 0x0112;
const SW_SHOWNORMAL: u16 = 1;

/// The number at the start of a profile value, as GetProfileInt reads
/// it: 0 if there is none.
fn number(value: &[u8]) -> u64 {
    value
        .iter()
        .skip_while(|b| b.is_ascii_whitespace())
        .take_while(|b| b.is_ascii_digit())
        .fold(0u64, |n, &d| n.saturating_mul(10).saturating_add((d - b'0') as u64))
}

/// The desktop came: the timeout from WIN.INI, and the timer from now.
pub fn desktop_started(cpu: &mut Cpu) {
    let mut setting = |key: &[u8]| {
        crate::win32::kernel32::profile_value(cpu, b"WIN.INI", b"windows", key)
            .map_or(0, |v| number(&v))
    };
    let active = setting(b"ScreenSaveActive") != 0;
    let seconds = setting(b"ScreenSaveTimeOut");
    let now = cpu.bus.clock.now_micros();
    let input = &mut user(cpu).input;
    input.saver_timeout = if active { seconds.saturating_mul(1_000_000) } else { 0 };
    input.idle_since = now;
    input.saver_due = 0;
    arm(cpu);
}

/// Key or mouse input came.
pub fn input(cpu: &mut Cpu) {
    let now = cpu.bus.clock.now_micros();
    let input = &mut user(cpu).input;
    input.idle_since = now;
    if input.saver_due == 0 {
        arm(cpu);
    }
}

/// Set the timer for the timeout after the last input.
fn arm(cpu: &mut Cpu) {
    let input = &mut user(cpu).input;
    if input.saver_timeout == 0 {
        return;
    }
    input.saver_due = input.idle_since.saturating_add(input.saver_timeout);
    cpu.bus.reschedule_timers();
}

/// Whether the timer's time has come at `now`.
pub fn due(win32: &crate::win32::Win32, now: u64) -> bool {
    let due = win32.user.input.saver_due;
    due != 0 && due <= now
}

/// The timer's time came (`win32::timer`): the active window gets
/// SC_SCREENSAVE, unless input came since the timer was set, which sets
/// it again. With no active window, nothing starts the saver until
/// input sets the timer again.
pub fn tick(cpu: &mut Cpu) {
    let now = cpu.bus.clock.now_micros();
    if !due(&cpu.bus.win32, now) {
        return;
    }
    let state = user(cpu);
    let input = &mut state.input;
    let due = input.idle_since.saturating_add(input.saver_timeout);
    if due > now {
        input.saver_due = due;
        return;
    }
    input.saver_due = 0;
    let active = state.activation(state.input.foreground).active;
    if active != 0 && state.is_window(active) {
        cpu.bus.log_string(&format!("[USER] Screen saver due: SC_SCREENSAVE to {:#x}", active));
        message::post_message(cpu, &[active, WM_SYSCOMMAND, SC_SCREENSAVE, 0]);
        crate::win32::thread::wake(cpu);
    }
}

/// DefWindowProc's SC_SCREENSAVE: SYSTEM.INI's `[boot] SCRNSAVE.EXE`
/// started with /s, if it names one.
pub fn start(cpu: &mut Cpu) {
    let Some(path) = crate::win32::kernel32::profile_value(cpu, b"SYSTEM.INI", b"boot", b"SCRNSAVE.EXE")
    else {
        return;
    };
    let path = crate::dosstr::from_bytes(&path).trim().to_string();
    if path.is_empty() {
        return;
    }
    let line = format!("{} /s", path);
    let startup = crate::win32::kernel32::startup_showing(SW_SHOWNORMAL);
    match crate::win32::kernel32::spawn(cpu, Some(&path), Some(&line), None, false, 0, startup) {
        Ok([process, thread, _, _]) => {
            crate::win32::object::close(cpu, process);
            crate::win32::object::close(cpu, thread);
        }
        Err(code) => {
            cpu.bus
                .log_string(&format!("[USER] Screen saver {} can't start: error {}", path, code));
        }
    }
}
