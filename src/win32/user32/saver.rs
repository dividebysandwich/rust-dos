//! The screen saver's timer. When no key or mouse input has come for
//! WIN.INI's `[windows] ScreenSaveTimeOut` seconds, with
//! `ScreenSaveActive=1`, USER sends the active window WM_SYSCOMMAND
//! SC_SCREENSAVE, and DefWindowProc starts SYSTEM.INI's `[boot]
//! SCRNSAVE.EXE` with /s, as Windows 3.1 does. Windows 98 keeps the
//! saver in SYSTEM.INI too, but the two settings in the registry
//! (`HKEY_CURRENT_USER\Control Panel\Desktop`), which its Display
//! settings write; here nothing writes that key, and the settings come
//! from WIN.INI, where Windows 3.1 setups such as Johnny Castaway's put
//! them. USER reads them when Windows starts (`user32::init`), and
//! SystemParametersInfo's SPI_SETSCREENSAVEACTIVE and
//! SPI_SETSCREENSAVETIMEOUT change them, as games do that turn the saver
//! off while they run.
//!
//! The timer is a time on the emulated clock, `Input::saver_due`, which
//! the built-in Windows's timer event (`win32::timer`) waits for, while
//! the desktop is on the screen. Input only notes its time; when the
//! timer's time comes and input came since it was set, it is set again
//! from that input. Once the saver was asked for, the timer waits for
//! input before it starts again.

use super::{message, user};
use crate::cpu::Cpu;

/// WM_SYSCOMMAND's command that starts the screen saver.
pub const SC_SCREENSAVE: u32 = 0xF140;
const WM_SYSCOMMAND: u32 = 0x0112;
const SW_SHOWNORMAL: u16 = 1;

/// The system starts: whether the saver is on and its timeout, from
/// WIN.INI as GetProfileInt reads them.
pub fn init(cpu: &mut Cpu) {
    let mut setting =
        |key: &[u8]| crate::win32::kernel32::profile_int(cpu, b"WIN.INI", b"windows", key, 0);
    let active = setting(b"ScreenSaveActive") != 0;
    let seconds = setting(b"ScreenSaveTimeOut");
    let input = &mut user(cpu).input;
    input.saver_active = active;
    input.saver_timeout = seconds;
}

/// The desktop came: the timer from now.
pub fn desktop_started(cpu: &mut Cpu) {
    let now = cpu.bus.clock.now_micros();
    let input = &mut user(cpu).input;
    input.idle_since = now;
    input.saver_due = 0;
    arm(cpu);
}

/// The desktop left the screen: no window is there to send
/// SC_SCREENSAVE, and the timer stops until it comes back.
pub fn desktop_ended(cpu: &mut Cpu) {
    user(cpu).input.saver_due = 0;
}

/// SystemParametersInfo turned the saver on or off or changed its
/// timeout: the timer counts again from the last input.
pub fn settings_changed(cpu: &mut Cpu) {
    user(cpu).input.saver_due = 0;
    arm(cpu);
    cpu.bus.reschedule_timers();
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

/// When the timeout ends after the last input, if the saver is on.
fn end_of_timeout(input: &super::input::Input) -> Option<u64> {
    if !input.saver_active || input.saver_timeout == 0 {
        return None;
    }
    let timeout = input.saver_timeout as u64 * 1_000_000;
    Some(input.idle_since.saturating_add(timeout))
}

/// Set the timer for the timeout after the last input, if the saver is
/// on and the desktop is on the screen.
fn arm(cpu: &mut Cpu) {
    let state = user(cpu);
    if !state.desktop {
        return;
    }
    let Some(due) = end_of_timeout(&state.input) else {
        return;
    };
    state.input.saver_due = due;
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
    // Input the devices have that USER hasn't read yet counts: Windows
    // reads it as it comes, and the program may not have asked lately.
    super::input::poll(cpu);
    let input = &mut user(cpu).input;
    match end_of_timeout(input) {
        // Input came since the timer was set: count again from it.
        Some(due) if due > now => {
            input.saver_due = due;
            return;
        }
        Some(_) => input.saver_due = 0,
        // The saver was turned off: nothing to start.
        None => {
            input.saver_due = 0;
            return;
        }
    }
    let state = user(cpu);
    let active = state.foreground_window();
    if active != 0 && state.is_window(active) {
        let line = format!("[USER] Screen saver due: SC_SCREENSAVE to {:#x}", active);
        cpu.bus.log_string(&line);
        message::post_message(cpu, &[active, WM_SYSCOMMAND, SC_SCREENSAVE, 0]);
    }
}

/// DefWindowProc's SC_SCREENSAVE: SYSTEM.INI's `[boot] SCRNSAVE.EXE`
/// started with /s, if it names one.
pub fn start(cpu: &mut Cpu) {
    let path = crate::win32::kernel32::profile_value(cpu, b"SYSTEM.INI", b"boot", b"SCRNSAVE.EXE")
        .map(|path| crate::dosstr::from_bytes(&path))
        .unwrap_or_default();
    if path.is_empty() {
        return;
    }
    // Quoted, so a path with spaces stays one program name.
    let line = format!("\"{}\" /s", path);
    let startup = crate::win32::kernel32::startup_showing(SW_SHOWNORMAL);
    match crate::win32::kernel32::spawn(cpu, Some(&path), Some(&line), None, false, 0, startup) {
        Ok([process, thread, _, _]) => {
            crate::win32::object::close(cpu, process);
            crate::win32::object::close(cpu, thread);
        }
        Err(code) => {
            let line = format!("[USER] Screen saver {} can't start: error {}", path, code);
            cpu.bus.log_string(&line);
        }
    }
}
