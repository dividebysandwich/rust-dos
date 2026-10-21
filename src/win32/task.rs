//! Functions of USER32 and GDI32 that call into the program as they go:
//! CreateWindowExA sends a window procedure a message after another,
//! DispatchMessageA calls one, hooks and enumeration functions call the
//! program's callbacks. As with every call into the program (call.rs), no
//! Rust frame lives across such a call. A task is a function written as
//! steps instead: each step does its part and says what comes next, a
//! call into the program and the step that goes on when it returns, or
//! the value the function returns. The task's state, its variables, is
//! the call's data while the program runs.
//!
//! A task that needs structures in the program's memory to pass to it
//! (CREATESTRUCT, WINDOWPOS) gets scratch space on the thread's stack,
//! below the function's arguments, which it gives back when it is done.
//! A message a task sends goes through SendMessageA itself, called as the
//! program would call it, so a task that sends messages to a window whose
//! procedure is another task needs nothing more.

use super::call::{self, Call};
use crate::cpu::Cpu;

/// What comes after a step.
pub enum Next {
    /// The function returns this value.
    Done(u32),
    /// Call the program's function at `target` with `args` (the first
    /// pushed last, as C does); `step` goes on with what it returns.
    Call {
        target: u32,
        args: Vec<u32>,
        step: u16,
    },
    /// The function has done what it does to the processor itself: it
    /// ended the thread, or made it wait to run again.
    Stop,
}

impl Next {
    pub fn call(target: u32, args: &[u32], step: u16) -> Next {
        Next::Call {
            target,
            args: args.to_vec(),
            step,
        }
    }
}

/// A task's state: which function it is (its place in `TASKS`), the step
/// it is at, its scratch space, if it has any, and its variables; and
/// where the function returns to, which the call keeps.
#[derive(Clone, Debug, Default)]
pub struct Task {
    pub kind: u16,
    pub step: u16,
    pub scratch: u32,
    pub vars: Vec<u32>,
    pub resume: (u32, u32),
}

impl Task {
    /// The variable at `i`, or 0 where there is none yet.
    pub fn var(&self, i: usize) -> u32 {
        self.vars.get(i).copied().unwrap_or(0)
    }

    pub fn set(&mut self, i: usize, value: u32) {
        if self.vars.len() <= i {
            self.vars.resize(i + 1, 0);
        }
        self.vars[i] = value;
    }

    fn encode(&self) -> Vec<u32> {
        let mut data = vec![self.kind as u32 | (self.step as u32) << 16, self.scratch];
        data.extend_from_slice(&self.vars);
        data
    }

    fn decode(data: &[u32], resume: (u32, u32)) -> Task {
        let head = data.first().copied().unwrap_or(0);
        Task {
            kind: head as u16,
            step: (head >> 16) as u16,
            scratch: data.get(1).copied().unwrap_or(0),
            vars: data.get(2..).map(<[u32]>::to_vec).unwrap_or_default(),
            resume,
        }
    }

    /// Make the function run again from its start when the thread goes
    /// on: its stub's trap, 4 bytes before the return instruction it
    /// returns to, with the stack it was called with. For a function that
    /// makes the thread wait.
    pub fn retry(&self, cpu: &mut Cpu) {
        cpu.set_eip(self.resume.0.wrapping_sub(4));
        cpu.set_esp(self.resume.1);
    }
}

/// A step: the task, at the step it is at, and the value the program's
/// function it called last returned (0 for the first step).
pub type Step = fn(&mut Cpu, &mut Task, u32) -> Next;

/// The tasks' functions. Append only: saved states name them by their
/// place.
pub static TASKS: &[Step] = &[
    super::user32::message::send_step,
    super::user32::message::dispatch_step,
    super::user32::message::get_step,
    super::user32::window::create_step,
    super::user32::window::destroy_step,
    super::user32::window::show_step,
    super::user32::window::set_pos_step,
    super::user32::defwnd::step,
    super::user32::hook::call_step,
    super::user32::window::enum_step,
    super::user32::paint::step,
    super::user32::dialog::step,
    super::user32::message::broadcast_step,
    super::user32::controls::step,
    super::gdi32::callback_step,
    super::user32::window::misc_step,
    super::user32::window::defer_step,
    super::user32::text::step,
    super::user32::resource::step,
    super::user32::input::step,
    super::user32::menu::step,
    super::winmm::callback::step,
    super::dinput::step,
    super::winmm::mmio::step,
];

/// The `TASKS` places of the tasks other DLLs than USER32 start.
pub const MM_CALLBACK: u16 = 21;
pub const DI_ENUM: u16 = 22;
pub const MMIO: u16 = 23;

/// Start the task `kind` with the variables `vars` and, if `scratch` is
/// more than 0, that many bytes of scratch space: for a function's
/// handler to return. It returns at once what the function returns, or
/// None when the task calls into the program, which it returns from
/// later.
pub fn start(cpu: &mut Cpu, kind: u16, vars: Vec<u32>, scratch: u32) -> Option<u32> {
    let resume = call::here(cpu);
    let mut task = Task {
        kind,
        step: 0,
        scratch: 0,
        vars,
        resume,
    };
    if scratch > 0 {
        let at = cpu.esp().wrapping_sub(scratch) & !3;
        if !cpu.bus.guest_probe(at, scratch as usize, true) {
            return None;
        }
        cpu.bus.guest_fill(at, scratch as usize, 0);
        cpu.set_esp(at);
        task.scratch = at;
    }
    match run(cpu, task, resume, 0) {
        Some(value) => {
            cpu.set_esp(resume.1);
            Some(value)
        }
        None => None,
    }
}

/// Run the task's steps from the one it is at, until it calls into the
/// program (None) or is done (its value). `resume` is where the function
/// returns to.
fn run(cpu: &mut Cpu, mut task: Task, resume: (u32, u32), mut value: u32) -> Option<u32> {
    loop {
        let Some(step) = TASKS.get(task.kind as usize) else {
            super::fatal(cpu, "a step of an unknown task", None);
            return None;
        };
        match step(cpu, &mut task, value) {
            Next::Done(v) => return Some(v),
            Next::Stop => return None,
            Next::Call { target, args, step } => {
                task.step = step;
                if target == 0 {
                    // Nothing to call: the step goes on with 0.
                    value = 0;
                    continue;
                }
                if !call::call(cpu, target, &args, call::TASK, resume, task.encode()) {
                    super::fatal(cpu, "a thread's stack isn't there", None);
                }
                return None;
            }
        }
    }
}

/// A call a task made returned `value`: it goes on.
pub fn returned(cpu: &mut Cpu, value: u32, call: Call) {
    let task = Task::decode(&call.data, call.resume);
    if let Some(v) = run(cpu, task, call.resume, value) {
        call::finish(cpu, call.resume, v);
    }
}
