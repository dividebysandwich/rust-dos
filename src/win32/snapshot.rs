//! What a debugger sees of the built-in Windows: its processes, their
//! modules, threads and handles, copied into plain structs that don't
//! change as the machine runs on and don't depend on how the built-in
//! Windows keeps them.

use super::object::{Kind, Objects};
use super::{Context, ESP, Process, Win32, stubs};
use crate::cpu::{Cpu, Seg};

/// The built-in Windows as it is.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Whether PE programs run (the `win32` setting).
    pub enabled: bool,
    /// Whether a Win32 program runs.
    pub active: bool,
    /// The program DOS loaded that hasn't started yet, if any.
    pub pending: Option<String>,
    /// The processes, the one DOS started first.
    pub processes: Vec<ProcessInfo>,
}

#[derive(Clone, Debug, Default)]
pub struct ProcessInfo {
    pub id: u32,
    pub name: String,
    /// Its path as DOS has it.
    pub path: String,
    /// What GetCommandLineA returns, if its page is there.
    pub command_line: Option<String>,
    pub image_base: u32,
    /// Its exit code once it ended; a process is gone from the list when
    /// it ends, so this is None while it is listed.
    pub exit_code: Option<u32>,
    /// Whether it is the process whose thread (or idle loop) runs.
    pub running: bool,
    /// Whether DOS started it: its exit code is the DOS process's.
    pub root: bool,
    /// The handles of its standard input, output and error.
    pub std: [u32; 3],
    pub modules: Vec<ModuleInfo>,
    pub threads: Vec<ThreadInfo>,
    pub handles: Vec<HandleInfo>,
}

#[derive(Clone, Debug, Default)]
pub struct ModuleInfo {
    pub name: String,
    pub base: u32,
    pub size: u32,
    /// Its entry point (DllMain), or 0 when it has none to call.
    pub entry: u32,
    /// Whether it is one of the built-in DLLs; otherwise it was loaded
    /// from a file (the program, or a DLL of its own).
    pub builtin: bool,
}

/// What a thread is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThreadState {
    /// It is in the processor.
    Running,
    /// It can run, when its turn comes.
    #[default]
    Ready,
    /// It waits for objects, a critical section or time to pass.
    Waiting,
    /// Its suspend count isn't 0 (it may wait as well).
    Suspended,
    Ended,
}

impl ThreadState {
    pub fn name(self) -> &'static str {
        match self {
            ThreadState::Running => "running",
            ThreadState::Ready => "ready",
            ThreadState::Waiting => "waiting",
            ThreadState::Suspended => "suspended",
            ThreadState::Ended => "ended",
        }
    }
}

/// Where a thread's registers in `ThreadInfo` come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Registers {
    /// The processor's: the thread runs.
    Cpu,
    /// The context a hardware interrupt left: the thread runs, and the
    /// interrupt's real-mode handler is in the processor.
    Interrupted,
    /// The context saved in its entry while another thread runs.
    #[default]
    Saved,
}

impl Registers {
    pub fn name(self) -> &'static str {
        match self {
            Registers::Cpu => "cpu",
            Registers::Interrupted => "interrupted",
            Registers::Saved => "saved",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ThreadInfo {
    pub id: u32,
    pub state: ThreadState,
    pub registers: Registers,
    pub cs: u16,
    pub eip: u32,
    pub esp: u32,
    /// Its TIB's linear address, and the selector FS has for it.
    pub tib: u32,
    pub selector: u16,
    /// Its stack: the lowest address of its reserve, and its top.
    pub stack: (u32, u32),
    pub last_error: u32,
    pub priority: i32,
    /// How many times it is suspended.
    pub suspended: u32,
    pub exit_code: Option<u32>,
    pub wait: Option<WaitInfo>,
}

#[derive(Clone, Debug, Default)]
pub struct WaitInfo {
    /// The objects it waits for, with its process's handle of each where
    /// it has one.
    pub objects: Vec<(Option<u32>, ObjectInfo)>,
    /// Whether all of them must be signaled at once, or one does.
    pub all: bool,
    /// How long until it times out, in milliseconds, rounded up; None
    /// for a wait without a timeout.
    pub timeout_ms: Option<u64>,
    /// The critical section it waits to enter, or 0.
    pub section: u32,
}

#[derive(Clone, Debug, Default)]
pub struct HandleInfo {
    pub handle: u32,
    pub object: ObjectInfo,
}

/// A kernel object.
#[derive(Clone, Debug, Default)]
pub struct ObjectInfo {
    /// Its number in the system's table, the same in every process.
    pub id: u32,
    /// What it is: "event", "mutex", "thread" and so on.
    pub kind: &'static str,
    /// Its state, as the fields of its kind.
    pub state: Vec<(&'static str, Field)>,
    pub name: Option<String>,
    /// The handles of it, and the system's holds.
    pub refs: u32,
}

/// A field of an object's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Field {
    Bool(bool),
    Number(u32),
    /// A thread or process id.
    Id(u32),
    /// An exit code, which there isn't until the thread or process ended.
    Code(Option<u32>),
    /// The state of a kind this list doesn't know, as Rust prints it.
    Text(String),
}

/// The built-in Windows's processes as they are, the running thread's
/// registers taken from the processor.
pub fn snapshot(cpu: &Cpu) -> Snapshot {
    let win32 = &cpu.bus.win32;
    let live = match &win32.interrupted {
        Some(ctx) => (Registers::Interrupted, *ctx),
        None => (Registers::Cpu, Context::of(cpu)),
    };
    let now = cpu.bus.clock.now_micros();
    Snapshot {
        enabled: win32.enabled,
        active: win32.active(),
        pending: win32.pending.clone(),
        processes: win32
            .processes
            .iter()
            .enumerate()
            .map(|(i, p)| process_info(cpu, win32, p, i == win32.running, live, now))
            .collect(),
    }
}

fn process_info(
    cpu: &Cpu,
    win32: &Win32,
    process: &Process,
    running: bool,
    live: (Registers, Context),
    now: u64,
) -> ProcessInfo {
    let objects = &win32.objects;
    let exit_code = match objects.get(process.object).map(|o| &o.kind) {
        Some(Kind::Process { exit_code, .. }) => *exit_code,
        _ => None,
    };
    let modules = process
        .modules
        .iter()
        .map(|m| ModuleInfo {
            name: m.name.clone(),
            base: m.base,
            size: m.size,
            entry: m.entry,
            builtin: stubs::DLLS
                .iter()
                .any(|d| d.base == m.base && d.name.eq_ignore_ascii_case(&m.name)),
        })
        .collect();
    let handle_of = |id: u32| {
        process
            .handles
            .iter()
            .find(|&(_, o)| o == id)
            .map(|(h, _)| h)
    };
    let threads = process
        .threads
        .iter()
        .enumerate()
        .map(|(t, thread)| {
            let current = running && process.current == Some(t);
            let state = if thread.exit_code.is_some() {
                ThreadState::Ended
            } else if current {
                ThreadState::Running
            } else if thread.suspended > 0 {
                ThreadState::Suspended
            } else if thread.wait.is_some() {
                ThreadState::Waiting
            } else {
                ThreadState::Ready
            };
            let (registers, ctx) = if current {
                live
            } else {
                (Registers::Saved, thread.ctx)
            };
            let wait = thread.wait.as_ref().map(|w| WaitInfo {
                objects: w
                    .objects
                    .iter()
                    .map(|&id| (handle_of(id), object_info(objects, id)))
                    .collect(),
                all: w.all,
                timeout_ms: w.deadline.map(|d| d.saturating_sub(now).div_ceil(1000)),
                section: w.section,
            });
            ThreadInfo {
                id: thread.id,
                state,
                registers,
                cs: ctx.seg[Seg::CS as usize],
                eip: ctx.eip,
                esp: ctx.gpr[ESP],
                tib: thread.tib,
                selector: thread.selector,
                stack: thread.stack,
                // The running thread's last error is the process's.
                last_error: if current {
                    process.last_error
                } else {
                    thread.last_error
                },
                priority: thread.priority,
                suspended: thread.suspended,
                exit_code: thread.exit_code,
                wait,
            }
        })
        .collect();
    let handles = process
        .handles
        .iter()
        .map(|(handle, id)| HandleInfo {
            handle,
            object: object_info(objects, id),
        })
        .collect();
    ProcessInfo {
        id: process.id,
        name: process.name.clone(),
        path: process.path.clone(),
        command_line: command_line(cpu, process),
        image_base: process.image_base,
        exit_code,
        running,
        root: process.id == win32.root,
        std: process.std,
        modules,
        threads,
        handles,
    }
}

/// The process's command line, read through its page tables without a
/// fault: None where its page isn't there.
fn command_line(cpu: &Cpu, process: &Process) -> Option<String> {
    if process.command_line == 0 {
        return None;
    }
    let mut bytes = Vec::new();
    // A command line is at most 32 KB.
    for i in 0..0x8000 {
        let lin = process.command_line.wrapping_add(i);
        let phys = process.memory.translate(&cpu.bus, lin)?;
        match cpu.bus.read_8(phys as usize) {
            0 => break,
            b => bytes.push(b),
        }
    }
    Some(crate::dosstr::from_bytes(&bytes))
}

/// The object `id`, or an empty entry if it is gone.
fn object_info(objects: &Objects, id: u32) -> ObjectInfo {
    let Some(object) = objects.get(id) else {
        return ObjectInfo {
            id,
            kind: "gone",
            ..ObjectInfo::default()
        };
    };
    let (kind, state) = describe(&object.kind);
    ObjectInfo {
        id,
        kind,
        state,
        name: object.name.clone(),
        refs: object.refs,
    }
}

/// An object's kind's name and its state.
#[allow(unreachable_patterns)]
fn describe(kind: &Kind) -> (&'static str, Vec<(&'static str, Field)>) {
    use Field::*;
    match kind {
        Kind::Dos { handle } => ("dos", vec![("handle", Number(*handle as u32))]),
        Kind::Event { manual, signaled } => (
            "event",
            vec![("manual", Bool(*manual)), ("signaled", Bool(*signaled))],
        ),
        Kind::Mutex {
            owner,
            count,
            abandoned,
        } => (
            "mutex",
            vec![
                ("owner", Id(*owner)),
                ("count", Number(*count)),
                ("abandoned", Bool(*abandoned)),
            ],
        ),
        Kind::Semaphore { count, max } => (
            "semaphore",
            vec![("count", Number(*count)), ("max", Number(*max))],
        ),
        Kind::Thread { id, exit_code } => (
            "thread",
            vec![("id", Id(*id)), ("exit_code", Code(*exit_code))],
        ),
        Kind::Process { id, exit_code } => (
            "process",
            vec![("id", Id(*id)), ("exit_code", Code(*exit_code))],
        ),
        // Kinds added after this list: their state as Rust prints it.
        other => ("other", vec![("debug", Text(format!("{:?}", other)))]),
    }
}
