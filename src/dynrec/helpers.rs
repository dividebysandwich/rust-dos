//! What translated code calls and reads besides the CPU: the context the
//! execution loop enters it with, and the Rust functions it calls.

use std::any::Any;
use std::mem::offset_of;
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::block::BlockData;
use crate::cpu::{Cpu, Fault};

/// How translated code returned to the execution loop: the low byte of
/// its return value. The rest is the index of the instruction it concerns.
pub const EXIT_FAULT: u32 = 1;
/// The instruction wrote over the block's later instructions.
pub const EXIT_SMC: u32 = 2;
pub const EXIT_PANIC: u32 = 3;
/// The block ran to its end.
pub const EXIT_NEXT: u32 = 4;
/// The block doesn't fit before the timer deadline; nothing ran.
pub const EXIT_DEADLINE: u32 = 5;
/// The block's bytes changed; nothing ran.
pub const EXIT_STALE: u32 = 6;

/// The context translated code runs in. Its first fields are read by the
/// code, which keeps a pointer to it in a register.
#[repr(C)]
pub struct JitCtx {
    /// `jit_fallback`.
    pub fallback: usize,
    /// `jit_revalidate`.
    pub revalidate: usize,
    /// Where translated code jumps to return to the execution loop.
    pub exit: usize,
    /// RAM and its code generations.
    pub ram: *const u8,
    pub page_gen: *const u32,
    /// The block the code returned from.
    pub exit_data: *mut BlockData,
    /// The fault an instruction raised, for EXIT_FAULT.
    pub fault: Fault,
    /// A handler's panic, for EXIT_PANIC, to resume in the execution loop.
    pub panic: Option<Box<dyn Any + Send>>,
}

pub const CTX_FALLBACK: i32 = offset_of!(JitCtx, fallback) as i32;
pub const CTX_REVALIDATE: i32 = offset_of!(JitCtx, revalidate) as i32;
pub const CTX_EXIT: i32 = offset_of!(JitCtx, exit) as i32;
pub const CTX_RAM: i32 = offset_of!(JitCtx, ram) as i32;
pub const CTX_PAGE_GEN: i32 = offset_of!(JitCtx, page_gen) as i32;
pub const CTX_EXIT_DATA: i32 = offset_of!(JitCtx, exit_data) as i32;
pub const DATA_GEN_SUM: i32 = offset_of!(BlockData, gen_sum) as i32;

impl JitCtx {
    pub fn new(exit: usize) -> Self {
        JitCtx {
            fallback: jit_fallback as *const () as usize,
            revalidate: jit_revalidate as *const () as usize,
            exit,
            ram: std::ptr::null(),
            page_gen: std::ptr::null(),
            exit_data: std::ptr::null_mut(),
            fault: Fault::UD,
            panic: None,
        }
    }
}

/// The calling convention between translated code and Rust: System V on
/// every x86-64 host, Windows included, so the code generator has one.
#[cfg(target_arch = "x86_64")]
macro_rules! jit_fn {
    ($(#[$m:meta])* fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty $body:block) => {
        $(#[$m])* pub extern "sysv64" fn $name($($arg: $ty),*) -> $ret $body
    };
}
#[cfg(not(target_arch = "x86_64"))]
macro_rules! jit_fn {
    ($(#[$m:meta])* fn $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty $body:block) => {
        $(#[$m])* pub extern "C" fn $name($($arg: $ty),*) -> $ret $body
    };
}

jit_fn! {
    /// Run instruction `ix` of a block through its interpreter handler, as
    /// the interpreter's `execute_at` does: EIP on the next instruction
    /// while it runs, and back on this one with ESP as it was if it faults
    /// (the execution loop sets EIP). Returns 0 to go on, or EXIT_FAULT,
    /// EXIT_SMC if it wrote over the rest of the block, or EXIT_PANIC.
    fn jit_fallback(cpu: *mut Cpu, ctx: *mut JitCtx, data: *mut BlockData, ix: u32) -> u32 {
        // SAFETY: translated code passes the CPU and context the execution
        // loop entered it with, and its own block, which outlive the call.
        let (cpu, ctx, data) = unsafe { (&mut *cpu, &mut *ctx, &mut *data) };
        let ix = ix as usize;
        let instr = &data.instrs[ix];
        let handler = data.handlers[ix];
        let start_esp = cpu.esp();
        cpu.set_eip(data.eips[ix].wrapping_add(instr.len() as u32));
        let writes = data.writes[ix];
        let before = if writes { data.gens_now(&cpu.bus.page_gen) } else { 0 };
        match catch_unwind(AssertUnwindSafe(|| handler(cpu, instr))) {
            Ok(Ok(())) => {
                if writes && data.gens_now(&cpu.bus.page_gen) != before {
                    return written(cpu, data, ix);
                }
                0
            }
            Ok(Err(fault)) => {
                cpu.set_esp(start_esp);
                ctx.fault = fault;
                EXIT_FAULT
            }
            Err(payload) => {
                ctx.panic = Some(payload);
                EXIT_PANIC
            }
        }
    }
}

/// Instruction `ix` wrote into the chunks of the block's bytes. If the
/// bytes are all as translated, the block stays valid as it is; if only
/// ones before the next instruction changed, it runs on (and is
/// translated again next time); otherwise it stops after this instruction.
#[cold]
fn written(cpu: &Cpu, data: &mut BlockData, ix: usize) -> u32 {
    let ram = cpu.bus.ram();
    if data.unchanged_from(ram, 0) {
        data.gen_sum = data.gens_now(&cpu.bus.page_gen);
        0
    } else if data.unchanged_from(ram, ix + 1) {
        0
    } else {
        EXIT_SMC
    }
}

jit_fn! {
    /// The generations of the block's chunks changed: if its bytes are
    /// still as translated (something else in the chunks was written),
    /// note the new generations and return 0 to run it, else 1.
    fn jit_revalidate(cpu: *mut Cpu, _ctx: *mut JitCtx, data: *mut BlockData) -> u32 {
        // SAFETY: as in `jit_fallback`.
        let (cpu, data) = unsafe { (&*cpu, &mut *data) };
        if data.unchanged_from(cpu.bus.ram(), 0) {
            data.gen_sum = data.gens_now(&cpu.bus.page_gen);
            0
        } else {
            1
        }
    }
}
