//! TOOLHELP's notifications: the callbacks NotifyRegister registers,
//! which Windows calls as tasks and modules come and go. A callback is
//! `BOOL CALLBACK f(WORD wID, DWORD dwData)`, Pascal's, and is called in
//! the context of the task the event is about, whichever task registered
//! it.
//!
//! A task's end is told as Windows 95 tells it (a probe program run on
//! it): NFY_EXITTASK with the exit code, then NFY_DELMODULE for each of
//! the program's own DLLs that goes with it and last for the program's
//! module, all in the ending task's context, where GetCurrentTask and
//! TaskFindHandle give that task. Sierra's setup waits for the copy of
//! itself it started that way: on NFY_EXITTASK it compares the current
//! task's instance with the one WinExec gave. The built-in libraries
//! (COMMDLG, SHELL, VER and the others) stand for system DLLs Windows 95
//! keeps loaded, so their going isn't told. The other events (segments
//! loaded and freed, a task's or a DLL's start, FreeLibrary's
//! NFY_DELMODULE, the task switches and the debug strings) aren't told.

use super::thunk;
use crate::cpu::Cpu;
use crate::dpmi::Context;

/// NotifyRegister's events, the `wID` a callback is given.
const NFY_EXITTASK: u16 = 5;
const NFY_DELMODULE: u16 = 6;

/// The W32 section's version that first keeps the callbacks
/// (`State::notifications`).
pub const NOTIFY_SAVED: u16 = 113;

/// A callback NotifyRegister registered: the task it is for, its 16:16
/// address, and the flags (NF_TASKSWITCH, NF_RIP) that ask for events
/// beyond the usual ones.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notification {
    pub task: u16,
    pub callback: u32,
    pub flags: u16,
}

crate::state_fields!(Notification {
    task,
    callback,
    flags
});

/// The task `handle` stands for: the running one for 0.
fn task_of(cpu: &Cpu, handle: u16) -> u16 {
    if handle != 0 {
        return handle;
    }
    super::current_task(cpu).map_or(0, |t| cpu.bus.win32.win16.tasks[t].handle)
}

/// NotifyRegister(task, callback, flags): the callback for the task, the
/// running one for 0. A task has one callback: Windows 95 answers FALSE
/// to a second NotifyRegister for it, and keeps the first. FALSE for a
/// task that isn't running too.
pub fn notify_register(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let task = task_of(cpu, args[0] as u16);
    if super::live_task(&cpu.bus.win32.win16, task).is_none() {
        return Some(0);
    }
    let notification = Notification {
        task,
        callback: args[1],
        flags: args[2] as u16,
    };
    let list = &mut cpu.bus.win32.win16.notifications;
    if list.iter().any(|n| n.task == task) {
        return Some(0);
    }
    list.push(notification);
    Some(1)
}

/// NotifyUnregister(task): the task's callback (the running task's for
/// 0) goes. FALSE when it has none.
pub fn notify_unregister(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let task = task_of(cpu, args[0] as u16);
    let list = &mut cpu.bus.win32.win16.notifications;
    let before = list.len();
    list.retain(|n| n.task != task);
    Some((list.len() < before) as u32)
}

/// A task ended: its callback goes.
pub fn task_ended(cpu: &mut Cpu, task: u16) {
    cpu.bus.win32.win16.notifications.retain(|n| n.task != task);
}

/// The running task, in the context `ctx`, is to end with exit code
/// `code`: call the callbacks with NFY_EXITTASK and then NFY_DELMODULE
/// for each module of the program's that goes with it
/// (`module::going`), its DLLs before its own, on its stack.
/// The task ends when the last returns (`then_exit`). False, with nothing
/// called, when there is nothing to call, when the task is already
/// telling its end (a callback that faulted, or ended the task, ends it
/// without the rest), or when its stack is no stack.
pub fn exit(cpu: &mut Cpu, code: u16, ctx: &Context) -> bool {
    let Some(t) = super::current_task(cpu) else {
        return false;
    };
    let win16 = &cpu.bus.win32.win16;
    if win16.notifications.is_empty() {
        return false;
    }
    let task = &win16.tasks[t];
    let telling = task
        .frames
        .iter()
        .any(|f| matches!(f, super::Frame::To16 { then, .. } if *then == thunk::THEN_NOTIFY_EXIT));
    if telling {
        return false;
    }
    let (ss, _) = thunk::stack16_for(cpu, ctx);
    if super::selector_base(cpu, ss).is_none() {
        return false;
    }
    let module = cpu.bus.win32.win16.tasks[t].module;
    let mut going: Vec<u16> = super::module::going(cpu, module)
        .into_iter()
        .filter(|&h| {
            super::module_index(cpu, h)
                .is_some_and(|i| cpu.bus.win32.win16.modules[i].builtin.is_none())
        })
        .collect();
    // The program's own module last, after its DLLs.
    if let Some(i) = going.iter().position(|&h| h == module) {
        let own = going.remove(i);
        going.push(own);
    }
    let mut events = vec![(NFY_EXITTASK, (code & 0xFF) as u32)];
    events.extend(going.into_iter().map(|h| (NFY_DELMODULE, h as u32)));
    // The code to end with, then each call's callback, wID and dwData.
    let mut data = vec![code as u32];
    for (id, value) in events {
        for n in cpu.bus.win32.win16.notifications.iter() {
            data.extend([n.callback, id as u32, value]);
        }
    }
    call_next(cpu, *ctx, data)
}

/// Make the first call `data` has after the exit code (`exit`) whose
/// callback is still registered: one that an earlier callback
/// unregistered is left out. False when there is none.
fn call_next(cpu: &mut Cpu, resume: Context, mut data: Vec<u32>) -> bool {
    let (callback, id, value) = loop {
        if data.len() < 4 {
            return false;
        }
        let call: Vec<u32> = data.drain(1..4).collect();
        let list = &cpu.bus.win32.win16.notifications;
        if list.iter().any(|n| n.callback == call[0]) {
            break (call[0], call[1] as u16, call[2]);
        }
    };
    cpu.bus.log_string(&format!(
        "[W16] TOOLHELP notification {} ({:X}) to {:04X}:{:04X}",
        id,
        value,
        callback >> 16,
        callback & 0xFFFF
    ));
    let words = [id, (value >> 16) as u16, value as u16];
    thunk::call16(
        cpu,
        callback,
        &words,
        0,
        resume,
        thunk::THEN_NOTIFY_EXIT,
        data,
    );
    true
}

/// A callback `exit` called returned: the next call, or the task's end.
pub(super) fn then_exit(cpu: &mut Cpu, _: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    let code = data.first().copied().unwrap_or(0) as u16;
    if !call_next(cpu, *ctx, data.to_vec()) {
        super::task::end(cpu, code);
    }
    None
}
