//! Hooks: SetWindowsHookExA's procedures, which USER calls as it sends
//! messages, makes windows and gets messages (WH_CBT, WH_CALLWNDPROC,
//! WH_GETMESSAGE and the rest). A thread's chain is its own hooks, the
//! newest first, then the process's hooks for all threads; each hook
//! calls the next with CallNextHookEx.
//!
//! A hook isn't called again for the thread while it runs, as on
//! Windows: what its procedure does doesn't call it once more.

use super::{
    ERROR_INVALID_HOOK_HANDLE, ERROR_INVALID_PARAMETER, FALSE, TRUE, address_of, current_pid,
    current_tid, set_error, user,
};
use crate::cpu::Cpu;
use crate::win32::task::{self, Next, Task};

pub const WH_MSGFILTER: i32 = -1;
pub const WH_JOURNALRECORD: i32 = 0;
pub const WH_KEYBOARD: i32 = 2;
pub const WH_GETMESSAGE: i32 = 3;
pub const WH_CALLWNDPROC: i32 = 4;
pub const WH_CBT: i32 = 5;
pub const WH_SYSMSGFILTER: i32 = 6;
pub const WH_MOUSE: i32 = 7;
pub const WH_SHELL: i32 = 10;
pub const WH_FOREGROUNDIDLE: i32 = 11;
pub const WH_CALLWNDPROCRET: i32 = 12;
// Windows 95 has no low-level hooks (WH_KEYBOARD_LL 13, WH_MOUSE_LL
// 14): they came with Windows NT 4.0 SP3.
const WH_MAX: i32 = 12;

const ERROR_INVALID_HOOK_FILTER: u32 = 1426;
const ERROR_INVALID_FILTER_PROC: u32 = 1427;
const ERROR_HOOK_NEEDS_HMOD: u32 = 1428;

/// Where hook handles start, and how far apart they are.
const HOOK_BASE: u32 = 0x2000_0000;

#[derive(Clone, Debug, Default)]
pub struct Hook {
    pub handle: u32,
    pub kind: i32,
    pub proc: u32,
    pub instance: u32,
    /// The thread it hooks, or 0 for all of its process's.
    pub tid: u32,
    pub pid: u32,
}

/// The hooks of `kind` that run for the thread `tid` of process `pid`,
/// in the order they are called.
fn chain(cpu: &Cpu, kind: i32) -> Vec<u32> {
    let (tid, pid) = (current_tid(cpu), current_pid(cpu));
    let hooks = &cpu.bus.win32.user.hooks;
    let own = hooks.iter().filter(|h| h.kind == kind && h.tid == tid);
    let global = hooks
        .iter()
        .filter(|h| h.kind == kind && h.tid == 0 && h.pid == pid);
    own.chain(global).map(|h| h.handle).collect()
}

/// Whether a hook of `kind` (WH_*, as a u32) would run for the running
/// thread now.
pub fn has(cpu: &Cpu, kind: u32) -> bool {
    let kind = kind as i32;
    let tid = current_tid(cpu);
    if cpu.bus.win32.user.in_hooks.contains(&(tid, kind)) {
        return false;
    }
    !chain(cpu, kind).is_empty()
}

/// Call the hooks of `kind` with `code`, `wp` and `lp`, going on at
/// `step` with what the first returns.
pub fn call(kind: u32, code: u32, wp: u32, lp: u32, step: u16) -> Next {
    Next::call(address_of("CallHooks"), &[kind, code, wp, lp], step)
}

const TASK_HOOK: u16 = 8;

/// The variables of a hook's call: its handle, kind, code and arguments.
const H_HANDLE: usize = 0;
const H_KIND: usize = 1;
const H_CODE: usize = 2;
const H_WP: usize = 3;
const H_LP: usize = 4;

/// CallHooks(kind, code, wParam, lParam): USER's own function, which calls
/// the first hook of the chain; 0 when there is none.
pub fn call_hooks(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let kind = args[0] as i32;
    let tid = current_tid(cpu);
    if cpu.bus.win32.user.in_hooks.contains(&(tid, kind)) {
        return Some(0);
    }
    let Some(&first) = chain(cpu, kind).first() else {
        return Some(0);
    };
    task::start(
        cpu,
        TASK_HOOK,
        vec![first, args[0], args[1], args[2], args[3], 1],
        0,
    )
}

/// CallNextHookEx(hook, code, wParam, lParam): the next hook of the
/// chain, or 0 at its end.
pub fn call_next_hook_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let handle = args[0];
    let state = user(cpu);
    let kind = match state.hooks.iter().find(|h| h.handle == handle) {
        Some(h) => h.kind,
        None => {
            // A hook that has gone: what follows it can't be known.
            return Some(0);
        }
    };
    let list = chain(cpu, kind);
    let next = list
        .iter()
        .position(|&h| h == handle)
        .and_then(|i| list.get(i + 1))
        .copied();
    let Some(next) = next else {
        return Some(0);
    };
    task::start(
        cpu,
        TASK_HOOK,
        vec![next, kind as u32, args[1], args[2], args[3], 0],
        0,
    )
}

/// A hook's call: the thread is in hooks of its kind while the first
/// runs.
pub fn call_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let tid = current_tid(cpu);
    let kind = t.var(H_KIND) as i32;
    let first = t.var(5) != 0;
    match t.step {
        0 => {
            let Some(proc) = user(cpu)
                .hooks
                .iter()
                .find(|h| h.handle == t.var(H_HANDLE))
                .map(|h| h.proc)
            else {
                return Next::Done(0);
            };
            if first {
                user(cpu).in_hooks.push((tid, kind));
            }
            Next::call(proc, &[t.var(H_CODE), t.var(H_WP), t.var(H_LP)], 1)
        }
        _ => {
            if first {
                let in_hooks = &mut user(cpu).in_hooks;
                if let Some(i) = in_hooks.iter().rposition(|&e| e == (tid, kind)) {
                    in_hooks.remove(i);
                }
            }
            Next::Done(value)
        }
    }
}

/// SetWindowsHookExA(kind, proc, instance, tid): the hook's handle.
pub fn set_windows_hook_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[kind, proc, instance, tid] = args else {
        unreachable!()
    };
    let kind = kind as i32;
    if !(WH_MSGFILTER..=WH_MAX).contains(&kind) {
        set_error(cpu, ERROR_INVALID_HOOK_FILTER);
        return Some(0);
    }
    if proc == 0 {
        set_error(cpu, ERROR_INVALID_FILTER_PROC);
        return Some(0);
    }
    if tid == 0 && instance == 0 {
        set_error(cpu, ERROR_HOOK_NEEDS_HMOD);
        return Some(0);
    }
    if tid != 0 && crate::win32::thread::find(&cpu.bus.win32, tid).is_none() {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    Some(add(cpu, kind, proc, instance, tid))
}

fn add(cpu: &mut Cpu, kind: i32, proc: u32, instance: u32, tid: u32) -> u32 {
    let pid = current_pid(cpu);
    let state = user(cpu);
    state.last_hook += 1;
    let handle = HOOK_BASE + state.last_hook;
    state.hooks.insert(
        0,
        Hook {
            handle,
            kind,
            proc,
            instance,
            tid,
            pid,
        },
    );
    handle
}

/// SetWindowsHookA(kind, proc): a hook of the running thread.
pub fn set_windows_hook(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let kind = args[0] as i32;
    if !(WH_MSGFILTER..=WH_MAX).contains(&kind) {
        set_error(cpu, ERROR_INVALID_HOOK_FILTER);
        return Some(0);
    }
    let tid = current_tid(cpu);
    Some(add(cpu, kind, args[1], 0, tid))
}

/// UnhookWindowsHookEx(hook).
pub fn unhook_windows_hook_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let state = user(cpu);
    match state.hooks.iter().position(|h| h.handle == args[0]) {
        Some(i) => {
            state.hooks.remove(i);
            Some(TRUE)
        }
        None => {
            set_error(cpu, ERROR_INVALID_HOOK_HANDLE);
            Some(FALSE)
        }
    }
}

/// UnhookWindowsHook(kind, proc).
pub fn unhook_windows_hook(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (kind, proc) = (args[0] as i32, args[1]);
    let tid = current_tid(cpu);
    let state = user(cpu);
    match state
        .hooks
        .iter()
        .position(|h| h.kind == kind && h.proc == proc && h.tid == tid)
    {
        Some(i) => {
            state.hooks.remove(i);
            Some(TRUE)
        }
        None => {
            set_error(cpu, ERROR_INVALID_HOOK_HANDLE);
            Some(FALSE)
        }
    }
}

/// CallMsgFilterA(msg, code): the WH_MSGFILTER hooks' answer, whether
/// they handled the message.
pub fn call_msg_filter(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !has(cpu, WH_MSGFILTER as u32) {
        return Some(FALSE);
    }
    call_hooks(cpu, &[WH_MSGFILTER as u32, args[1], 0, args[0]])
}

crate::state_fields!(Hook {
    handle,
    kind,
    proc,
    instance,
    tid,
    pid
});
