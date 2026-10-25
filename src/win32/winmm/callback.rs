//! How WinMM tells the program about its devices (DriverCallback): a
//! message to a window or a thread, an event it sets, or a function of
//! the program's it calls, as the `CALLBACK_` flags of waveOutOpen,
//! midiOutOpen and the others say. timeSetEvent's timers call functions
//! too.
//!
//! Functions are called on the process's multimedia thread, as on
//! Windows 95, where they ran at interrupt time outside the program's
//! threads: a thread of the process that WinMM makes the first time a
//! device or timer may call one (`ensure_thread`). Its code is a stub of
//! `mm_thread`, a loop: the calls queued for the process (`notify`) are
//! made one at a time, then due work is looked for (`super::poll`), and
//! then the thread waits for the next call queued or the time `poll`
//! asked to be woken at.
//!
//! `notify` may be called when time passes (`super::tick`), whichever
//! process runs then: it only posts, sets or queues.

use super::State;
use crate::cpu::Cpu;
use crate::win32::object::{self, Kind};
use crate::win32::task::{self, Next, Task};
use crate::win32::thread::{self, Wait};

/// The kinds of callback, in an open function's flags.
pub const CALLBACK_TYPEMASK: u32 = 0x0007_0000;
pub const CALLBACK_NULL: u32 = 0x0000_0000;
pub const CALLBACK_WINDOW: u32 = 0x0001_0000;
pub const CALLBACK_THREAD: u32 = 0x0002_0000;
pub const CALLBACK_FUNCTION: u32 = 0x0003_0000;
pub const CALLBACK_EVENT: u32 = 0x0005_0000;

/// THREAD_PRIORITY_TIME_CRITICAL.
const TIME_CRITICAL: i32 = 15;

/// MMSYSERR_INVALFLAG and MMSYSERR_INVALPARAM.
const MMSYSERR_INVALFLAG: u32 = 10;
const MMSYSERR_INVALPARAM: u32 = 11;

/// Where a device's or timer's notices go.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Callback {
    #[default]
    None,
    Window {
        hwnd: u32,
    },
    /// A thread, by its id.
    Thread {
        tid: u32,
    },
    /// A function, in the process with this id.
    Function {
        pid: u32,
        address: u32,
    },
    /// An event, by its object, which the callback holds.
    Event {
        id: u32,
    },
}

impl Callback {
    /// The callback an open function's `callback` and `flags` name, for
    /// the running process: an event's handle is taken to its object,
    /// which the callback holds until `release`. Or the MMSYSERR to fail
    /// with.
    pub fn from_open(cpu: &mut Cpu, callback: u32, flags: u32) -> Result<Callback, u32> {
        Ok(match flags & CALLBACK_TYPEMASK {
            CALLBACK_NULL => Callback::None,
            CALLBACK_WINDOW => {
                if callback != 0 && cpu.bus.win32.user.window(callback).is_none() {
                    return Err(MMSYSERR_INVALPARAM);
                }
                Callback::Window { hwnd: callback }
            }
            CALLBACK_THREAD => Callback::Thread { tid: callback },
            CALLBACK_FUNCTION => {
                ensure_thread(cpu).ok_or(MMSYSERR_INVALPARAM)?;
                Callback::Function {
                    pid: crate::win32::user32::current_pid(cpu),
                    address: callback,
                }
            }
            CALLBACK_EVENT => {
                let id = object::resolve(cpu, callback)
                    .filter(|&id| {
                        matches!(
                            object::objects(cpu).get(id).map(|o| &o.kind),
                            Some(Kind::Event { .. })
                        )
                    })
                    .ok_or(MMSYSERR_INVALPARAM)?;
                object::objects(cpu).hold(id);
                Callback::Event { id }
            }
            _ => return Err(MMSYSERR_INVALFLAG),
        })
    }

    /// Give back what the callback holds: an event's object.
    pub fn release(&self, cpu: &mut Cpu) {
        if let Callback::Event { id } = *self {
            object::objects(cpu).release(id);
        }
    }
}

/// Tell `callback` about `msg` of the device or timer `handle`: a
/// window's message has the handle in wParam and `p1` in lParam, as
/// MM_WOM_DONE and the others do; a thread's message the same; a
/// function is called as `f(handle, msg, instance, p1, p2)`; an event is
/// set.
pub fn notify(
    cpu: &mut Cpu,
    callback: &Callback,
    handle: u32,
    msg: u32,
    instance: u32,
    p1: u32,
    p2: u32,
) {
    match *callback {
        Callback::None => {}
        Callback::Window { hwnd } => {
            if hwnd != 0 {
                crate::win32::user32::message::post_message(cpu, &[hwnd, msg, handle, p1]);
            }
        }
        Callback::Thread { tid } => {
            crate::win32::user32::message::post_thread_message(cpu, &[tid, msg, handle, p1]);
        }
        Callback::Function { pid, address } => {
            queue_call(cpu, pid, address, [handle, msg, instance, p1, p2]);
        }
        Callback::Event { id } => set_event(cpu, id),
    }
}

/// Set the event `id`, by its object.
pub fn set_event(cpu: &mut Cpu, id: u32) {
    if let Some(o) = object::objects(cpu).get_mut(id)
        && let Kind::Event { signaled, .. } = &mut o.kind
    {
        *signaled = true;
    }
}

/// Pulse the event `id`, by its object: the threads waiting for it that
/// it can release go, and it is reset.
pub fn pulse_event(cpu: &mut Cpu, id: u32) {
    set_event(cpu, id);
    thread::wake(cpu);
    if let Some(o) = object::objects(cpu).get_mut(id)
        && let Kind::Event { signaled, .. } = &mut o.kind
    {
        *signaled = false;
    }
}

/// A process's multimedia thread: its id, and the auto-reset event that
/// wakes it, by its object.
#[derive(Clone, Debug, Default)]
pub struct MmThread {
    pub pid: u32,
    pub tid: u32,
    pub wake: u32,
}

/// A call of the program's function `address` with `args`, queued for
/// the multimedia thread of the process `pid`.
#[derive(Clone, Debug, Default)]
pub struct Queued {
    pub pid: u32,
    pub address: u32,
    pub args: Vec<u32>,
}

fn state(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.winmm
}

/// Queue the call of `address` with `args` for the multimedia thread of
/// the process `pid`, and wake the thread.
pub fn queue_call(cpu: &mut Cpu, pid: u32, address: u32, args: [u32; 5]) {
    let Some(wake) = state(cpu)
        .threads
        .iter()
        .find(|t| t.pid == pid)
        .map(|t| t.wake)
    else {
        return;
    };
    state(cpu).queued.push(Queued {
        pid,
        address,
        args: args.to_vec(),
    });
    set_event(cpu, wake);
}

/// Wake the multimedia thread of the process `pid`, if it has one, so it
/// looks for due work (`super::poll`).
pub fn wake_thread(cpu: &mut Cpu, pid: u32) {
    if let Some(wake) = state(cpu)
        .threads
        .iter()
        .find(|t| t.pid == pid)
        .map(|t| t.wake)
    {
        set_event(cpu, wake);
    }
}

/// Make the running process's multimedia thread if it has none. None if
/// it can't be made.
pub fn ensure_thread(cpu: &mut Cpu) -> Option<()> {
    let pid = crate::win32::user32::current_pid(cpu);
    if state(cpu).threads.iter().any(|t| t.pid == pid) {
        return Some(());
    }
    let wake = object::objects(cpu).add(
        Kind::Event {
            manual: false,
            signaled: false,
        },
        None,
    );
    // The thread's code is in WINMM's image, which a program that only
    // uses DirectMusic hasn't mapped.
    crate::win32::loader::map_builtin_named(cpu, "WINMM.DLL")?;
    let start = super::mm_thread_address();
    let Some(tid) = crate::win32::kernel32::create_system_thread(cpu, start) else {
        object::objects(cpu).release(wake);
        return None;
    };
    // It runs at THREAD_PRIORITY_TIME_CRITICAL, as Windows 95's timer
    // callbacks did at interrupt time, ahead of the program's threads.
    if let Some((p, t)) = thread::find(&cpu.bus.win32, tid) {
        cpu.bus.win32.processes[p].threads[t].priority = TIME_CRITICAL;
    }
    state(cpu).threads.push(MmThread { pid, tid, wake });
    Some(())
}

/// A process ended: its multimedia thread and calls go.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let winmm = state(cpu);
    winmm.queued.retain(|q| q.pid != pid);
    let gone: Vec<u32> = winmm
        .threads
        .iter()
        .filter(|t| t.pid == pid)
        .map(|t| t.wake)
        .collect();
    winmm.threads.retain(|t| t.pid != pid);
    for wake in gone {
        object::objects(cpu).release(wake);
    }
}

/// The multimedia thread's loop (a stub that jumps back to its trap):
/// the next call queued for the process, else the due work `poll` finds,
/// else a wait until there is some.
pub fn mm_thread(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let pid = crate::win32::user32::current_pid(cpu);
    let Some(wake) = state(cpu)
        .threads
        .iter()
        .find(|t| t.pid == pid)
        .map(|t| t.wake)
    else {
        // Not a multimedia thread: a program that called the stub.
        thread::exit_current(cpu, 0);
        return None;
    };
    let winmm = state(cpu);
    if let Some(i) = winmm.queued.iter().position(|q| q.pid == pid) {
        let q = winmm.queued.remove(i);
        let mut vars = vec![q.address];
        vars.extend_from_slice(&q.args);
        return task::start(cpu, task::MM_CALLBACK, vars, 0);
    }
    let now = cpu.bus.clock.now_micros();
    let deadline = super::poll(cpu, pid, now);
    if state(cpu).queued.iter().any(|q| q.pid == pid) {
        // Calls the work queued: around the loop again.
        return Some(0);
    }
    let wait = Wait {
        objects: vec![wake],
        deadline,
        ..Wait::default()
    };
    if thread::wait_now(cpu, &wait).is_some() {
        // Woken meanwhile: around the loop again.
        return Some(0);
    }
    thread::block(cpu, wait);
    None
}

/// The task of a call the multimedia thread makes: its variables are the
/// function and its arguments.
pub fn step(_: &mut Cpu, t: &mut Task, _: u32) -> Next {
    match t.step {
        0 => Next::call(t.var(0), &t.vars[1..], 1),
        _ => Next::Done(0),
    }
}

crate::state_fields!(MmThread { pid, tid, wake });
crate::state_fields!(Queued { pid, address, args });
crate::win32::state_variants!(Callback {
    None {},
    Window { hwnd },
    Thread { tid },
    Function { pid, address },
    Event { id },
});
