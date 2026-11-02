//! Calls from the system into the program: exception handlers, DLL entry
//! points, the C runtime's callbacks. No Rust frame lives across such a
//! call, since the program may switch threads or be saved in a state while
//! it runs. Instead the call is a frame on the program's stack, its return
//! address the stub of `CallReturn`, and a `Call` in the thread's list that
//! says what runs when it returns (`then`, a function in `THENS`, with the
//! data it needs).
//!
//! A function of a DLL that calls into the program returns no value
//! itself (None): what it returns is set when the last of its calls is
//! done, by `finish`, which goes back to where the function was called
//! from (`resume`).

use super::{process, seh};
use crate::cpu::Cpu;

/// A call into the program the running thread has made.
#[derive(Clone, Debug, Default)]
pub struct Call {
    /// Where the work goes on when it is done: the stub's return
    /// instruction and its stack pointer, which `finish` goes back to.
    pub resume: (u32, u32),
    /// The stack pointer before the call's arguments were pushed, which it
    /// is set to again when the call returns, whatever its convention.
    pub esp: u32,
    /// Where its return address is: the stack pointer once its arguments
    /// and the return address were pushed.
    pub frame: u32,
    /// What runs when it returns: its index in `THENS`.
    pub then: u16,
    pub data: Vec<u32>,
}

/// What runs when a call returns: the value it returned in EAX, and the
/// call.
pub type Then = fn(&mut Cpu, u32, Call);

/// The functions calls return to. Append only: saved states name them by
/// their place.
pub static THENS: &[Then] = &[
    seh::handler_returned,
    seh::unwind_handler_returned,
    seh::filter_returned,
    super::thread::attached,
    super::kernel32::module::notified,
    super::kernel32::module::enumerated,
    super::kernel32::enum_returned,
    super::msvcrt::returned,
    super::task::returned,
    super::winmm::driver::returned,
    super::wnaspi32::posted,
];

/// The `THENS` places, by name.
pub const SEH_HANDLER: u16 = 0;
pub const SEH_UNWIND: u16 = 1;
pub const SEH_FILTER: u16 = 2;
pub const THREAD_ATTACHED: u16 = 3;
pub const DLL_NOTIFIED: u16 = 4;
pub const RESOURCE_ENUMERATED: u16 = 5;
pub const ENUM_STRINGS: u16 = 6;
pub const MSVCRT: u16 = 7;
pub const TASK: u16 = 8;
pub const DRIVER: u16 = 9;
pub const ASPI_POSTED: u16 = 10;

/// Call the program's function at `target` with `args` (the first pushed
/// last, as C does), on the running thread's stack below ESP. When it
/// returns, `then` runs. Returns false, having changed nothing, if the
/// stack isn't there.
pub fn call(
    cpu: &mut Cpu,
    target: u32,
    args: &[u32],
    then: u16,
    resume: (u32, u32),
    data: Vec<u32>,
) -> bool {
    let esp = cpu.esp();
    let frame = esp.wrapping_sub(4 * (args.len() as u32 + 1));
    if !cpu.bus.guest_probe(frame, 4 * (args.len() + 1), true) {
        return false;
    }
    let mut words = vec![super::kernel32::call_return_address()];
    words.extend_from_slice(args);
    for (i, &word) in words.iter().enumerate() {
        cpu.bus.guest_write_32(frame + 4 * i as u32, word);
    }
    cpu.set_esp(frame);
    cpu.set_eip(target);
    let thread = process(cpu).current_thread_mut().expect("a running thread");
    // Calls whose frames were at or above the stack pointer won't return:
    // the program left them, as a handler of its own C runtime does when
    // it jumps to its __except block.
    thread.calls.retain(|c| c.esp > esp);
    thread.calls.push(Call {
        resume,
        esp,
        frame,
        then,
        data,
    });
    true
}

/// The work of a function of a DLL is done: it returns `value` to where
/// it was called from.
pub fn finish(cpu: &mut Cpu, resume: (u32, u32), value: u32) {
    cpu.set_eip(resume.0);
    cpu.set_esp(resume.1);
    cpu.set_eax(value);
}

/// Where the running function of a DLL goes back to: its stub's return
/// instruction, where EIP is while it runs, and the stack as it is.
pub fn here(cpu: &Cpu) -> (u32, u32) {
    (cpu.eip(), cpu.esp())
}

/// A call returned to `CallReturn`: what it called for goes on.
pub fn returned(cpu: &mut Cpu) {
    let value = cpu.eax();
    let esp = cpu.esp();
    let call = process(cpu).current_thread_mut().and_then(|t| {
        // The call whose return address the stack pointer is just past;
        // calls after it in the list are ones the program left without
        // returning, whose frames were below its.
        let i = t
            .calls
            .iter()
            .rposition(|c| (c.frame.wrapping_add(4)..=c.esp).contains(&esp));
        if let Some(i) = i {
            t.calls.truncate(i + 1);
        }
        t.calls.pop()
    });
    let Some(call) = call else {
        super::fatal(cpu, "a return to the system that nothing called", None);
        return;
    };
    cpu.set_esp(call.esp);
    match THENS.get(call.then as usize) {
        Some(then) => then(cpu, value, call),
        None => super::fatal(cpu, "a return to an unknown place", None),
    }
}

/// The running thread goes on at the stack pointer `esp`, below which its
/// stack is gone: the calls whose frames were there won't return (an
/// exception unwound past them).
pub fn unwound_to(cpu: &mut Cpu, esp: u32) {
    if let Some(thread) = process(cpu).current_thread_mut() {
        thread.calls.retain(|c| c.esp > esp);
    }
}

crate::state_fields!(Call {
    resume,
    esp,
    frame,
    then,
    data
});
