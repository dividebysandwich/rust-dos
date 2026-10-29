//! Real-mode code the system runs for a program in protected mode, and
//! the way back, as Windows' VMM runs the virtual machine's real-mode
//! code for its protected-mode programs: the handler of a hardware
//! interrupt that came while a program ran (system.rs), the handler a
//! program put in the real-mode vector table for a Win16 task's INT n
//! (win16/dos.rs), and the code a Win16 task calls with DPMI's INT 31h
//! AX=0300h, 0301h and 0302h (win16/dpmi.rs).
//!
//! Each goes the same way. `call` keeps what the system goes on with
//! afterwards in a `Frame`, switches the processor to real mode with the
//! registers, stack and flags the caller gives and pushes the words under
//! the code: a return address of the trap at `RETURN` among them. When
//! the code returns there, `returned` takes the innermost frame and gives
//! the program what the code returned, as the frame's kind says.
//!
//! The frames nest. Real-mode code can call a Win16 task's procedure
//! through a real-mode callback (INT 31h AX=0303h): the real-mode code
//! waits in a `Callback` frame while the procedure runs (`suspend`), and
//! goes on when it returns (`resume`). The procedure may call real-mode
//! code again, and a hardware interrupt may come while it runs: those
//! frames go on top, and their code runs on the host's stack below the
//! waiting code's (`host_stack`).

use super::system::{enter_real, enter_user};
use super::{Context, ESP, REAL_RETURN, ROM_SEG};
use crate::cpu::{Cpu, CpuFlags, Seg, SegCache};

/// The return address real-mode code the system calls returns to.
pub const RETURN: (u16, u16) = (ROM_SEG, REAL_RETURN);

/// The arithmetic flags: what the real-mode handler of a software
/// interrupt gives back to the protected-mode program.
const STATUS_FLAGS: u32 = 0x08D5;

/// What a frame waits for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    /// The real-mode handler of a hardware interrupt: the program goes
    /// on as the interrupt found it.
    #[default]
    Irq,
    /// The real-mode handler of a Win16 task's INT n: the task goes on
    /// after the INT with the general registers and arithmetic flags the
    /// handler left.
    Int,
    /// Real-mode code a Win16 task called through DPMI: the call
    /// structure gets the registers the code left (win16/dpmi.rs).
    Call,
    /// Real-mode code that called a Win16 task's procedure through a
    /// real-mode callback, which goes on when that returns. `ctx` is the
    /// real-mode code's.
    Callback,
}

/// What the system goes on with when the code it called returns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub kind: Kind,
    /// The program's context, which goes on afterwards; the real-mode
    /// code's for a `Callback`.
    pub ctx: Context,
    /// A `Call`'s call structure, at this linear address of the Win16
    /// process; a `Callback`'s slot.
    pub at: u32,
}

impl Frame {
    pub fn new(kind: Kind, ctx: Context) -> Frame {
        Frame { kind, ctx, at: 0 }
    }
}

/// The stack real-mode code the system calls runs on when its caller
/// gives none: the DOS program's, where its DOS stub left it, below the
/// stack of any real-mode code waiting for a callback there.
pub fn host_stack(cpu: &Cpu) -> (u16, u16) {
    let win32 = &cpu.bus.win32;
    let (ss, top) = win32.rm_stack;
    let sp = win32
        .real_mode
        .iter()
        .filter(|f| f.kind == Kind::Callback && f.ctx.seg[Seg::SS as usize] == ss)
        .map(|f| f.ctx.gpr[ESP] as u16)
        .fold(top, u16::min);
    (ss, sp)
}

/// Whether real-mode code the system called runs, so that a return to
/// `RETURN` is one.
pub fn in_call(cpu: &Cpu) -> bool {
    live_context(cpu).is_some()
}

/// Whether the system waits for real-mode code: while it does, the
/// program's threads don't switch.
pub fn waiting(cpu: &Cpu) -> bool {
    !cpu.bus.win32.real_mode.is_empty()
}

/// The protected-mode context the innermost real-mode code was called
/// from, if there is real-mode code running or waiting: a callback's
/// procedure runs on its stack, below where it was.
pub fn caller(cpu: &Cpu) -> Option<Context> {
    let frames = &cpu.bus.win32.real_mode;
    frames
        .iter()
        .rev()
        .find(|f| f.kind != Kind::Callback)
        .map(|f| f.ctx)
}

/// The program's live context while real-mode code the system called
/// runs, for the debugger's view of the threads.
pub fn live_context(cpu: &Cpu) -> Option<Context> {
    cpu.bus
        .win32
        .real_mode
        .last()
        .filter(|f| f.kind != Kind::Callback)
        .map(|f| f.ctx)
}

/// Call real-mode code, keeping `frame`: at `rm`'s CS:IP, on its SS:SP,
/// with its general registers (but ESP), data segments and flags, and
/// `words` pushed on the stack, the first at the highest address. The
/// words must hold a return to `RETURN` that the code comes back to.
pub fn call(cpu: &mut Cpu, frame: Frame, rm: &Context, words: &[u16]) {
    cpu.bus.win32.real_mode.push(frame);
    enter(cpu, rm);
    for &word in words {
        cpu.push(word);
    }
}

/// The real-mode context the handler of `vector` in the real-mode vector
/// table starts in: on the host's stack, with the general registers
/// `gpr`, DS and ES `seg` and the flags `flags`.
pub fn handler_context(cpu: &Cpu, vector: u8, gpr: [u32; 8], seg: u16, flags: u32) -> Context {
    let handler = cpu.bus.read_32(vector as usize * 4);
    let (ss, sp) = host_stack(cpu);
    let mut rm = Context {
        gpr,
        eip: handler & 0xFFFF,
        eflags: flags,
        seg: [seg, (handler >> 16) as u16, ss, seg, 0, 0],
    };
    rm.gpr[ESP] = sp as u32;
    rm
}

/// A Win16 task's software interrupt `vector` that goes down to real
/// mode, as Windows reflects one its protected-mode code doesn't handle
/// to the virtual machine's vector table: the handler there runs with
/// the general registers of `ctx`, its arithmetic flags and direction
/// flag, and DS and ES `seg`, on the host's stack, interrupts off. The
/// task goes on after its INT with the general registers and the
/// arithmetic flags the handler left, its own stack and segment
/// registers.
pub fn reflect_int(cpu: &mut Cpu, vector: u8, ctx: Context, seg: u16) {
    let flags = ctx.eflags & (STATUS_FLAGS | CpuFlags::DF.bits()) | 0x0002;
    let rm = handler_context(cpu, vector, ctx.gpr, seg, flags);
    // The frame the INT pushed: the handler's IRET comes back to the
    // trap, with interrupts on, as the task had them.
    let (ret_seg, ret_off) = RETURN;
    let words = [(flags | CpuFlags::IF.bits()) as u16, ret_seg, ret_off];
    call(cpu, Frame::new(Kind::Int, ctx), &rm, &words);
}

/// Switch the processor to real mode with all of `rm`'s registers.
fn enter(cpu: &mut Cpu, rm: &Context) {
    let seg = |s: Seg| rm.seg[s as usize];
    enter_real(
        cpu,
        (seg(Seg::CS), rm.eip as u16),
        (seg(Seg::SS), rm.gpr[ESP] as u16),
        rm.eflags,
    );
    let mut snapshot = cpu.snapshot();
    let esp = snapshot.gpr[ESP];
    snapshot.gpr = rm.gpr;
    snapshot.gpr[ESP] = esp;
    cpu.restore(&snapshot);
    for s in [Seg::ES, Seg::DS, Seg::FS, Seg::GS] {
        cpu.set_seg_cache(s, SegCache::real(seg(s)));
    }
}

/// The trap at `RETURN`: the innermost real-mode code the system called
/// returned, and the program goes on as its frame says.
pub fn returned(cpu: &mut Cpu) {
    let frame = cpu
        .bus
        .win32
        .real_mode
        .pop()
        .expect("real-mode code called");
    let rm = Context::of(cpu);
    let mut ctx = frame.ctx;
    match frame.kind {
        Kind::Irq if cpu.bus.win32.real_mode.is_empty() => {
            super::thread::after_interrupt(cpu, ctx);
        }
        // In a callback's procedure: that goes on, without switching
        // threads.
        Kind::Irq => enter_user(cpu, &ctx),
        Kind::Int => {
            let esp = ctx.gpr[ESP];
            ctx.gpr = rm.gpr;
            ctx.gpr[ESP] = esp;
            ctx.eflags = ctx.eflags & !STATUS_FLAGS | rm.eflags & STATUS_FLAGS;
            enter_user(cpu, &ctx);
        }
        Kind::Call => crate::win16::dpmi::call_returned(cpu, frame, &rm),
        Kind::Callback => unreachable!("a callback's code returned to the system"),
    }
}

/// Real-mode code with the context `rm` called a protected-mode
/// procedure through the callback `slot`: it waits while that runs.
pub fn suspend(cpu: &mut Cpu, rm: Context, slot: usize) {
    let frame = Frame {
        kind: Kind::Callback,
        ctx: rm,
        at: slot as u32,
    };
    cpu.bus.win32.real_mode.push(frame);
}

/// A callback's procedure returned: the real-mode code that called it
/// goes on with the context `rm`. False if no real-mode code waits for
/// one, as when the procedure returned twice.
pub fn resume(cpu: &mut Cpu, rm: &Context) -> bool {
    let frames = &mut cpu.bus.win32.real_mode;
    if frames.last().is_none_or(|f| f.kind != Kind::Callback) {
        return false;
    }
    frames.pop();
    enter(cpu, rm);
    true
}

/// A Win16 task ended, maybe in a callback's procedure: the real-mode
/// code that waits for it, and what that code was called for, can't go
/// on, and their frames go. Only a procedure can end a task while frames
/// are kept: the processor is in real mode for the others.
pub fn abandon(cpu: &mut Cpu) {
    let frames = std::mem::take(&mut cpu.bus.win32.real_mode);
    if !frames.is_empty() {
        cpu.bus.log_string(&format!(
            "[W32] A task ended in a real-mode callback: {} real-mode calls dropped",
            frames.len()
        ));
    }
}

crate::state_enum!(Kind {
    Kind::Irq,
    Kind::Int,
    Kind::Call,
    Kind::Callback
});
crate::state_fields!(Frame { kind, ctx, at });
