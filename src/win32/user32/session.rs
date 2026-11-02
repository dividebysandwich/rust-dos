//! The end of the session: ExitWindowsEx. Logging off, a shutdown and a
//! restart all end the built-in Windows' session, as nothing starts after
//! it: every process ends and the DOS prompt comes back.
//!
//! The top-level windows are asked with WM_QUERYENDSESSION first, and any
//! of them can refuse. Then each gets WM_ENDSESSION, with what was decided,
//! which is where a program saves its work. The caller's windows come
//! last, after the other processes have gone, so that a caller that ends
//! itself on WM_ENDSESSION, as many do, doesn't leave them running.
//!
//! Windows returns TRUE from ExitWindowsEx once the shutdown has started,
//! and ends the caller when its turn comes. That can come at any time
//! after its WM_ENDSESSION, so a program can't count on the code after the
//! call. Here it comes at once, and a call that isn't refused doesn't come
//! back.

use super::message::{WM_ENDSESSION, WM_QUERYENDSESSION, thread_alive};
use super::{TRUE, address_of, current_pid, current_tid, user};
use crate::cpu::Cpu;
use crate::win32::task::{self, Next, Task};

const EWX_SHUTDOWN: u32 = 1;
const EWX_REBOOT: u32 = 2;
const EWX_FORCE: u32 = 4;
const EWX_POWEROFF: u32 = 8;

/// lParam of both messages when the user logs off.
const ENDSESSION_LOGOFF: u32 = 0x8000_0000;

/// SendMessageTimeoutA's flags, none.
const SMTO_NORMAL: u32 = 0;

/// How long a window has to answer, in milliseconds: Windows's default
/// HungAppTimeout. A window that takes longer doesn't stop the session
/// from ending, as the user would end its program there.
const ANSWER_MS: u32 = 5000;

/// The steps of the task.
const QUERY: u16 = 0;
const ANSWER: u16 = 1;
const NOTIFY: u16 = 2;

/// The variables of the task: lParam of the messages, the next window's
/// place, wParam of WM_ENDSESSION, the place of the caller's first
/// window, the calling thread, then the windows.
const LPARAM: usize = 0;
const NEXT: usize = 1;
const ENDING: usize = 2;
const OWN: usize = 3;
const CALLER: usize = 4;
const WINDOWS: usize = 5;

/// ExitWindowsEx(flags, reserved). With EWX_FORCE no window is asked or
/// told. FALSE when a window refused.
pub fn exit_windows_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let flags = args[0];
    let logoff = flags & (EWX_SHUTDOWN | EWX_REBOOT | EWX_POWEROFF) == 0;
    let lparam = if logoff { ENDSESSION_LOGOFF } else { 0 };
    let (pid, tid) = (current_pid(cpu), current_tid(cpu));
    let state = user(cpu);
    let (own, others): (Vec<u32>, Vec<u32>) = if flags & EWX_FORCE != 0 {
        Default::default()
    } else {
        state
            .top_levels()
            .into_iter()
            .partition(|&h| state.window(h).is_some_and(|w| w.pid == pid))
    };
    let mut vars = vec![lparam, 0, TRUE, others.len() as u32, tid];
    vars.extend(others);
    vars.extend(own);
    // The answer comes back in the scratch space.
    task::start(cpu, task::END_SESSION, vars, 4)
}

/// The call that sends `msg` to `hwnd`, going on at `step`. Its value is 0
/// when no answer came.
fn send(t: &Task, hwnd: u32, msg: u32, wparam: u32, step: u16) -> Next {
    let args = [
        hwnd,
        msg,
        wparam,
        t.var(LPARAM),
        SMTO_NORMAL,
        ANSWER_MS,
        t.scratch,
    ];
    Next::call(address_of("SendMessageTimeoutA"), &args, step)
}

/// The steps of ExitWindowsEx's task.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    loop {
        let at = t.var(NEXT) as usize;
        match t.step {
            QUERY => {
                let Some(&hwnd) = t.vars.get(WINDOWS + at) else {
                    t.set(NEXT, 0);
                    t.step = NOTIFY;
                    continue;
                };
                t.set(NEXT, at as u32 + 1);
                if user(cpu).is_window(hwnd) {
                    return send(t, hwnd, WM_QUERYENDSESSION, 0, ANSWER);
                }
            }
            ANSWER => {
                let hwnd = t.vars[WINDOWS + at - 1];
                // A window that went while it was asked answered nothing.
                let refused = value != 0
                    && cpu.bus.guest_read_32(t.scratch) == 0
                    && user(cpu).is_window(hwnd);
                t.step = QUERY;
                if refused {
                    // The windows asked before it learn that the session
                    // goes on.
                    t.set(ENDING, 0);
                    t.vars.truncate(WINDOWS + at - 1);
                    t.set(NEXT, 0);
                    t.step = NOTIFY;
                }
            }
            _ => {
                let ending = t.var(ENDING) != 0;
                if ending && at == t.var(OWN) as usize {
                    crate::win32::end_others(cpu);
                    // A 16-bit client task goes with the process it served.
                    if !thread_alive(&cpu.bus.win32, t.var(CALLER)) {
                        crate::win32::exit(cpu, 0);
                        return Next::Stop;
                    }
                }
                let Some(&hwnd) = t.vars.get(WINDOWS + at) else {
                    if ending {
                        crate::win32::exit(cpu, 0);
                        return Next::Stop;
                    }
                    return Next::Done(0);
                };
                t.set(NEXT, at as u32 + 1);
                if user(cpu).is_window(hwnd) {
                    return send(t, hwnd, WM_ENDSESSION, t.var(ENDING), NOTIFY);
                }
            }
        }
    }
}
