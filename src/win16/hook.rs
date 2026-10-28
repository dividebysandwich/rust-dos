//! Hooks of 16-bit programs: SetWindowsHook and SetWindowsHookEx set a
//! hook of USER32's (src/win32/user32/hook.rs) whose procedure is a
//! thunk for the 16-bit one. The thunk gives it its code and wParam as
//! words and its lParam as Windows 3.1 has it: the 16-bit copy of the
//! structure the 32-bit one points to (a CBT_CREATEWND16 and its
//! CREATESTRUCT16, a MSG16, a CWPSTRUCT16 and the others), on the 16-bit
//! stack below the call. What the procedure changes in the copy goes
//! back to the structure when it returns, or when it passes the call on
//! with CallNextHookEx or DefHookProc, which give the next hook the
//! 32-bit structure again.

use super::message::{self, Stack16, sx};
use super::thunk::{self, ESP, KIND_HOOK};
use super::{Frame, Thunk, read_16, read_32, write_16, write_32};
use crate::cpu::Cpu;
use crate::dpmi::Context;
use crate::win32::user32::hook::{
    Hook, WH_CALLWNDPROC, WH_CBT, WH_GETMESSAGE, WH_JOURNALRECORD, WH_MOUSE, WH_MSGFILTER,
    WH_SHELL, WH_SYSMSGFILTER,
};
use crate::win32::user32::window::{HCBT_ACTIVATE, HCBT_CREATEWND, HCBT_MOVESIZE};

const WH_JOURNALPLAYBACK: i32 = 1;
const HCBT_CLICKSKIPPED: u32 = 6;

/// The kinds of hook Windows 3.1 has: WH_SHELL is its last.
const KINDS: std::ops::RangeInclusive<i32> = WH_MSGFILTER..=WH_SHELL;

/// Windows 3.1's CW_USEDEFAULT, and the 32-bit one.
const CW_USEDEFAULT16: u32 = 0x8000;
const CW_USEDEFAULT: u32 = 0x8000_0000;

/// A hook thunk's `extra`: the hook's kind in the low byte, the data
/// segment its procedure gets in the high word.
fn extra(kind: i32, ds: u16) -> u32 {
    kind as u8 as u32 | (ds as u32) << 16
}

/// The data segment a hook of the instance `instance` (0 for the running
/// task's) gets: its automatic data segment's selector.
fn hook_ds(cpu: &Cpu, instance: u16) -> u16 {
    let instance = if instance != 0 {
        instance
    } else {
        super::current_task(cpu).map_or(0, |t| cpu.bus.win32.win16.tasks[t].instance)
    };
    message::instance_ds(cpu, instance)
}

/// The 16-bit hook a USER32 hook stands for: its procedure's 16:16
/// address and its data segment, if it is one.
fn hook16(cpu: &Cpu, hook: &Hook) -> Option<(u32, u16)> {
    let t = thunk::thunk32_of(cpu, hook.proc).filter(|t| t.kind == KIND_HOOK)?;
    Some((t.target, (t.extra >> 16) as u16))
}

/// Take away the 16-bit hooks `gone` picks.
fn remove(cpu: &mut Cpu, gone: impl Fn(&Hook, u32, u16) -> bool) {
    let hooks = std::mem::take(&mut cpu.bus.win32.user.hooks);
    let kept = hooks
        .into_iter()
        .filter(|h| hook16(cpu, h).is_none_or(|(proc, ds)| !gone(h, proc, ds)))
        .collect();
    cpu.bus.win32.user.hooks = kept;
}

/// A task ended: the hooks of its thread go, and those whose data
/// segment was its instance's when that goes with it (`ds`, else 0), as
/// Windows 3.1 takes a task's hooks away.
pub fn task_ended(cpu: &mut Cpu, tid: u32, ds: u16) {
    remove(cpu, |h, _, hook_ds| h.tid == tid || ds != 0 && hook_ds == ds);
}

/// A module was unloaded: the hooks whose procedures were in its code
/// segments, the selectors `code`, go.
pub fn module_unloaded(cpu: &mut Cpu, code: &[u16]) {
    remove(cpu, |_, proc, _| code.contains(&((proc >> 16) as u16 | 3)));
}

/// The 32-bit slot for the 16-bit hook procedure `proc` of `kind`.
fn proc32(cpu: &mut Cpu, proc: u32, kind: i32, instance: u16) -> u32 {
    let ds = hook_ds(cpu, instance);
    thunk::proc32(cpu, proc, KIND_HOOK, extra(kind, ds))
}

/// SetWindowsHookEx(kind, proc, instance, task): USER32's hook for the
/// task's thread, or for all of them with no task.
pub fn set_windows_hook_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let kind = args[0] as i16 as i32;
    let instance = args[2] as u16;
    let task = args[3] as u16;
    if !KINDS.contains(&kind) {
        return Some(0);
    }
    let tid = if task == 0 {
        0
    } else {
        let win16 = &cpu.bus.win32.win16;
        match win16.tasks.iter().find(|t| t.handle == task) {
            Some(t) => t.tid,
            None => return Some(0),
        }
    };
    let proc = proc32(cpu, args[1], kind, instance);
    let hook = crate::win32::user32::address_of("SetWindowsHookExA");
    let args = [kind as u32, proc, instance as u32, tid];
    thunk::call32(cpu, hook, &args, thunk::POST_PLAIN, Vec::new());
    None
}

/// SetWindowsHook(kind, proc): a hook of the running task, whose handle
/// it gives for DefHookProc, where Windows 3.1 gives the hook before it.
pub fn set_windows_hook(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let kind = args[0] as i16 as i32;
    if !KINDS.contains(&kind) {
        return Some(0);
    }
    let proc = proc32(cpu, args[1], kind, 0);
    let hook = crate::win32::user32::address_of("SetWindowsHookA");
    thunk::call32(cpu, hook, &[kind as u32, proc], thunk::POST_PLAIN, Vec::new());
    None
}

/// UnhookWindowsHook(kind, proc): the latest hook of `kind` with the
/// procedure `proc`, whichever task set it, since Windows 3.1's tasks
/// share their hooks.
pub fn unhook_windows_hook(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (kind, proc) = (args[0] as i16 as i32, args[1]);
    let hooks = &cpu.bus.win32.user.hooks;
    let found = hooks
        .iter()
        .rposition(|h| h.kind == kind && hook16(cpu, h).is_some_and(|(p, _)| p == proc));
    match found {
        Some(i) => {
            cpu.bus.win32.user.hooks.remove(i);
            Some(1)
        }
        None => Some(0),
    }
}

/// CallNextHookEx(hook, code, wParam, lParam).
pub fn call_next_hook_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    call_next(cpu, args[0], sx(args[1]), args[2], args[3])
}

/// DefHookProc(code, wParam, lParam, &hook): the hook SetWindowsHook
/// gave passes the call on.
pub fn def_hook_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hook = read_32(cpu, args[3]);
    call_next(cpu, hook, sx(args[0]), args[1], args[2])
}

/// Pass a hook's call on to the next hook of `hook`'s chain. When `lp`
/// is the 16-bit copy a hook was called with, the next hook gets the
/// 32-bit structure, with what the hook changed, and what the next one
/// changes is copied back.
fn call_next(cpu: &mut Cpu, hook: u32, code: u32, wp: u32, lp: u32) -> Option<u32> {
    let next = crate::win32::user32::address_of("CallNextHookEx");
    let Some(call) = call_of(cpu, lp) else {
        thunk::call32(cpu, next, &[hook, code, wp, lp], thunk::POST_PLAIN, Vec::new());
        return None;
    };
    to32(cpu, &call);
    let args = [hook, code, call.wp, call.lp];
    thunk::call32(cpu, next, &args, thunk::POST_HOOK, call.data());
    None
}

/// CallMsgFilter(msg, code): the WH_MSGFILTER hooks' answer, whether
/// they took the message. What the hooks change in it goes back to the
/// caller's MSG16, which on Windows 3.1 is the one they get.
pub fn call_msg_filter(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let m = thunk::scratch32(cpu, 28);
    let top = message::msg32(cpu, args[0], m);
    let filter = crate::win32::user32::address_of("CallMsgFilterA");
    let ctx = Context::of(cpu);
    let mut data = vec![args[0], m];
    data.extend((0..7).map(|i| read_32(cpu, m + 4 * i)));
    let post = thunk::POST_MSG_FILTER;
    thunk::enter32_at(cpu, ctx, top, filter, &[m, args[1]], post, data);
    None
}

/// CallMsgFilterA returned: the MSG16 gets the message again if a hook
/// changed it.
pub fn post_msg_filter(cpu: &mut Cpu, value: u32, data: &[u32], _: &mut Context) -> Option<u32> {
    if let [m16, m, was @ ..] = data
        && was
            .iter()
            .enumerate()
            .any(|(i, &v)| read_32(cpu, m + 4 * i as u32) != v)
    {
        message::msg16(cpu, *m, *m16);
    }
    Some(value)
}

/// A hook's call as its thunk made it: the 32-bit arguments, and the
/// 16-bit copy of what lParam points to (its 16:16 and linear address,
/// 0 when lParam is passed as it is).
struct Call {
    kind: i32,
    code: u32,
    wp: u32,
    lp: u32,
    lp16: u32,
    at16: u32,
}

impl Call {
    fn data(&self) -> Vec<u32> {
        vec![
            self.kind as u32,
            self.code,
            self.wp,
            self.lp,
            self.lp16,
            self.at16,
        ]
    }

    fn of(data: &[u32]) -> Option<Call> {
        let &[kind, code, wp, lp, lp16, at16] = data else {
            return None;
        };
        Some(Call {
            kind: kind as i32,
            code,
            wp,
            lp,
            lp16,
            at16,
        })
    }
}

/// The call of a 16-bit hook running on the task whose lParam is the
/// copy at `lp16`, the latest if there are several.
fn call_of(cpu: &Cpu, lp16: u32) -> Option<Call> {
    let t = super::current_task(cpu)?;
    cpu.bus.win32.win16.tasks[t]
        .frames
        .iter()
        .rev()
        .find_map(|f| match f {
            Frame::To16 { then, data, .. } if *then == thunk::THEN_HOOK => {
                Call::of(data).filter(|c| c.at16 != 0 && c.lp16 == lp16)
            }
            _ => None,
        })
}

/// USER32 called a hook's 32-bit slot (`thunk::thunk32_trap`):
/// (code, wParam, lParam), stdcall's. The 16-bit procedure runs with
/// them as Windows 3.1 gives them.
pub fn call_hook16(cpu: &mut Cpu, thunk: &Thunk) {
    let esp = cpu.esp();
    let ret = cpu.bus.guest_read_32(esp);
    let code = cpu.bus.guest_read_32(esp + 4);
    let wp = cpu.bus.guest_read_32(esp + 8);
    let lp = cpu.bus.guest_read_32(esp + 12);
    let mut resume = Context::of(cpu);
    resume.eip = ret;
    resume.gpr[ESP] = esp + 16;
    let at = thunk::stack16_for(cpu, &resume);
    let mut stack = Stack16::new(cpu, at);
    let kind = thunk.extra as u8 as i8 as i32;
    let (lp16, at16) = to16(cpu, kind, code, lp, &mut stack);
    let call = Call {
        kind,
        code,
        wp,
        lp,
        lp16,
        at16,
    };
    let ds = (thunk.extra >> 16) as u16;
    let args = [code as u16, wp as u16, (lp16 >> 16) as u16, lp16 as u16];
    let at = (stack.ss, stack.sp);
    let data = call.data();
    thunk::call16_at(cpu, thunk.target, &args, ds, resume, thunk::THEN_HOOK, data, at);
}

/// A 16-bit hook returned DX:AX: what it changed goes back to the 32-bit
/// structure. USER32 gets AX, the BOOL hooks of Windows 3.1 answer with
/// (they may leave anything in DX), or the whole of DX:AX for a journal
/// playback hook, which gives a time to wait.
pub fn then(cpu: &mut Cpu, value: u32, data: &[u32], _: &mut Context) -> Option<u32> {
    let Some(call) = Call::of(data) else {
        return Some(value);
    };
    to32(cpu, &call);
    if call.kind == WH_JOURNALPLAYBACK {
        Some(value)
    } else {
        Some(value & 0xFFFF)
    }
}

/// CallNextHookEx of a 16-bit hook returned: what the next hooks changed
/// in the 32-bit structure goes to the 16-bit copy.
pub fn post(cpu: &mut Cpu, value: u32, data: &[u32], _: &mut Context) -> Option<u32> {
    if let Some(call) = Call::of(data) {
        fill16(cpu, &call);
    }
    Some(value)
}

fn r16(cpu: &Cpu, at: u32) -> u32 {
    read_16(cpu, at) as u32
}

fn w16(cpu: &mut Cpu, at: u32, value: u32) {
    write_16(cpu, at, value as u16);
}

/// The size of the 16-bit copy of what a hook's lParam points to, if it
/// is a pointer, as its kind and code say.
fn size16(kind: i32, code: u32) -> Option<u32> {
    match kind {
        WH_CBT => match code {
            HCBT_MOVESIZE => Some(8),
            HCBT_CREATEWND => Some(6),
            HCBT_ACTIVATE => Some(4),
            HCBT_CLICKSKIPPED => Some(12),
            _ => None,
        },
        WH_MSGFILTER | WH_SYSMSGFILTER | WH_GETMESSAGE => Some(18),
        WH_CALLWNDPROC => Some(10),
        WH_MOUSE => Some(12),
        WH_JOURNALRECORD | WH_JOURNALPLAYBACK => Some(10),
        _ => None,
    }
}

/// The lParam the 16-bit hook gets for the 32-bit `lp`: a copy of the
/// structure on `stack`, its 16:16 and linear addresses; or `lp` as it
/// is and 0.
fn to16(cpu: &mut Cpu, kind: i32, code: u32, lp: u32, stack: &mut Stack16) -> (u32, u32) {
    let Some(size) = size16(kind, code).filter(|_| lp != 0) else {
        return (lp, 0);
    };
    if kind == WH_CALLWNDPROC {
        // CWPSTRUCT (lParam, wParam, message, hwnd) and CWPSTRUCT16
        // (lParam, wParam, message, hwnd), the message translated as a
        // window procedure gets it.
        let hwnd = read_32(cpu, lp + 12);
        let msg = read_32(cpu, lp + 8);
        let wp = read_32(cpu, lp + 4);
        let lp32 = read_32(cpu, lp);
        let mut fixes = Vec::new();
        let (msg, wp, lp32) = message::to16(cpu, hwnd, msg, wp, lp32, stack, &mut fixes);
        let (seg, at) = stack.alloc(size);
        write_32(cpu, at, lp32);
        w16(cpu, at + 4, wp);
        w16(cpu, at + 6, msg);
        w16(cpu, at + 8, hwnd);
        return (seg, at);
    }
    let cs = if kind == WH_CBT && code == HCBT_CREATEWND {
        message::create_struct16(cpu, read_32(cpu, lp), stack)
    } else {
        0
    };
    let (seg, at) = stack.alloc(size);
    if kind == WH_CBT && code == HCBT_CREATEWND {
        write_32(cpu, at, cs);
    }
    let call = Call {
        kind,
        code,
        wp: 0,
        lp,
        lp16: seg,
        at16: at,
    };
    fill16(cpu, &call);
    (seg, at)
}

/// What a CBT hook changed in the CREATESTRUCT16 at `cs16` (its place,
/// size and styles) back to the CREATESTRUCT at `cs`. A field the hook
/// left keeps its 32-bit value, which the word may not hold.
fn create_struct32(cpu: &mut Cpu, cs16: u32, cs: u32) {
    if cs16 == 0 || cs == 0 {
        return;
    }
    // cy, cx, y, x.
    for i in 0..4 {
        let (at16, at) = (cs16 + 10 + 2 * i, cs + 16 + 4 * i);
        let (v16, v) = (r16(cpu, at16), read_32(cpu, at));
        if v16 != v & 0xFFFF {
            let v16 = if v16 == CW_USEDEFAULT16 { CW_USEDEFAULT } else { sx(v16) };
            write_32(cpu, at, v16);
        }
    }
    let style = read_32(cpu, cs16 + 18);
    write_32(cpu, cs + 32, style);
    // The copy has no WS_EX_WINDOWEDGE, which Windows 3.1 doesn't have.
    let edge = super::user::WS_EX_WINDOWEDGE;
    let (ex16, ex) = (read_32(cpu, cs16 + 30), read_32(cpu, cs + 44));
    if ex16 != ex & !edge {
        write_32(cpu, cs + 44, ex16 | ex & edge);
    }
}

/// The place, size and styles of the CREATESTRUCT at `cs` to the
/// CREATESTRUCT16 at `cs16`, as a hook passing the call on gets them back
/// from the next ones.
fn create_struct16(cpu: &mut Cpu, cs: u32, cs16: u32) {
    if cs16 == 0 || cs == 0 {
        return;
    }
    for i in 0..4 {
        let v = read_32(cpu, cs + 16 + 4 * i);
        w16(cpu, cs16 + 10 + 2 * i, v);
    }
    let style = read_32(cpu, cs + 32);
    write_32(cpu, cs16 + 18, style);
    let ex = read_32(cpu, cs + 44) & !super::user::WS_EX_WINDOWEDGE;
    write_32(cpu, cs16 + 30, ex);
}

/// Write the 32-bit structure of `call` to its 16-bit copy (all of it
/// but CBT_CREATEWND16's CREATESTRUCT16, which is made once).
fn fill16(cpu: &mut Cpu, call: &Call) {
    let (lp, at) = (call.lp, call.at16);
    if at == 0 {
        return;
    }
    match (call.kind, call.code) {
        (WH_CBT, HCBT_MOVESIZE) => {
            for i in 0..4 {
                let v = read_32(cpu, lp + 4 * i);
                w16(cpu, at + 2 * i, v);
            }
        }
        // CBT_CREATEWND: lpcs, hwndInsertAfter.
        (WH_CBT, HCBT_CREATEWND) => {
            let after = read_32(cpu, lp + 4);
            w16(cpu, at + 4, after);
            let cs16 = super::linear(cpu, read_32(cpu, at));
            create_struct16(cpu, read_32(cpu, lp), cs16);
        }
        // CBTACTIVATESTRUCT: fMouse, hWndActive.
        (WH_CBT, HCBT_ACTIVATE) => {
            let mouse = read_32(cpu, lp);
            w16(cpu, at, mouse);
            let active = read_32(cpu, lp + 4);
            w16(cpu, at + 2, active);
        }
        (WH_CBT, HCBT_CLICKSKIPPED) | (WH_MOUSE, _) => {
            // MOUSEHOOKSTRUCT: pt, hwnd, wHitTestCode, dwExtraInfo.
            for i in 0..4 {
                let v = read_32(cpu, lp + 4 * i);
                w16(cpu, at + 2 * i, v);
            }
            let extra = read_32(cpu, lp + 16);
            write_32(cpu, at + 8, extra);
        }
        (WH_MSGFILTER | WH_SYSMSGFILTER | WH_GETMESSAGE, _) => {
            message::msg16(cpu, lp, at);
        }
        (WH_JOURNALRECORD | WH_JOURNALPLAYBACK, _) => {
            // EVENTMSG: message, paramL, paramH, time, hwnd.
            for i in 0..3 {
                let v = read_32(cpu, lp + 4 * i);
                w16(cpu, at + 2 * i, v);
            }
            let time = read_32(cpu, lp + 12);
            write_32(cpu, at + 6, time);
        }
        _ => {}
    }
}

/// Write what a 16-bit hook may change in its copy back to the 32-bit
/// structure of `call`: the window being made, and the window to go
/// after; the message a filter or GetMessage hook gets; the event a
/// journal gives back.
fn to32(cpu: &mut Cpu, call: &Call) {
    let (lp, at) = (call.lp, call.at16);
    if at == 0 {
        return;
    }
    match (call.kind, call.code) {
        (WH_CBT, HCBT_CREATEWND) => {
            // HWND_TOPMOST and HWND_NOTOPMOST are -1 and -2.
            let after = r16(cpu, at + 4);
            let after = if after >= 0xFFFE { sx(after) } else { after };
            write_32(cpu, lp + 4, after);
            let cs16 = super::linear(cpu, read_32(cpu, at));
            create_struct32(cpu, cs16, read_32(cpu, lp));
        }
        (WH_MSGFILTER | WH_SYSMSGFILTER | WH_GETMESSAGE, _) => {
            // The message again from the copy if the hook changed it, so
            // that one whose parameters a MSG16 packs differently keeps
            // its own when it didn't.
            let m16 = thunk::scratch32(cpu, 18);
            message::msg16(cpu, lp, m16);
            if super::peek(cpu, m16, 18) != super::peek(cpu, at, 18) {
                let m = thunk::scratch32(cpu, 28);
                message::msg32(cpu, at, m);
                for i in 0..7 {
                    let v = read_32(cpu, m + 4 * i);
                    write_32(cpu, lp + 4 * i, v);
                }
            }
        }
        (WH_JOURNALPLAYBACK, _) => {
            for i in 0..3 {
                let v = r16(cpu, at + 2 * i);
                write_32(cpu, lp + 4 * i, v);
            }
            let time = read_32(cpu, at + 6);
            write_32(cpu, lp + 12, time);
        }
        _ => {}
    }
}
