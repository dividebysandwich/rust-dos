//! What translated code calls and reads besides the CPU: the context the
//! execution loop enters it with, and the Rust functions it calls.

use std::any::Any;
use std::mem::offset_of;
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::block::{BlockData, Guard};
use crate::cpu::{Access, Cpu, Fault, MemRef, Seg};

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
/// An instruction raised #GP(0) (a near jump past the CS limit).
pub const EXIT_GP0: u32 = 7;
/// The block needs a larger CS limit to be fetched through the code
/// window; nothing ran.
pub const EXIT_LIMIT: u32 = 8;
/// The block left for a known EIP in its page through a link that isn't
/// set yet (the index is the link's).
pub const EXIT_UNLINKED: u32 = 9;
/// An instruction raised #DE (a division by 0, or a quotient that
/// doesn't fit).
pub const EXIT_DE: u32 = 10;
/// A watched byte of the instruction differs from what was translated:
/// the instruction didn't run (see `block::WATCH_AFTER`).
pub const EXIT_WATCHED: u32 = 11;
/// With EXIT_FAULT, EXIT_GP0, EXIT_DE, EXIT_SMC and EXIT_WATCHED: the
/// guest's arithmetic flags are in the context's `flags`, not yet in the
/// CPU.
pub const EXIT_FLAGS: u32 = 1 << 16;

/// A memory operand handle at or above this is `SLOW + slot`: the operand
/// isn't plain RAM in one page, and loads and stores go through
/// `jit_read` and `jit_write` with the operand checked in `refs[slot]`.
/// Below it, a handle is the operand's physical address in RAM.
pub const SLOW: u32 = 0xFFFF_FF00;
/// `jit_memref`'s result for a fault.
pub const MEMREF_FAULT: u64 = u64::MAX;

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
    /// `jit_memref`, `jit_read` and `jit_write`.
    pub memref: usize,
    pub read: usize,
    pub write: usize,
    /// Bytes of RAM.
    pub ram_len: u64,
    /// The TLB's entries.
    pub tlb: *const u8,
    /// The bytes of the running block after the instruction that stores,
    /// as physical addresses `lo..hi`, for `jit_write`.
    pub smc_lo: u32,
    pub smc_hi: u32,
    /// The fault an instruction raised, for EXIT_FAULT.
    pub fault: Fault,
    /// The register the code keeps the guest's arithmetic flags in where
    /// it changes them, as it left (for EXIT_FLAGS).
    pub flags: u32,
    /// A panic in Rust called from translated code, to resume in the
    /// execution loop.
    pub panic: Option<Box<dyn Any + Send>>,
    /// Memory operands checked by `jit_memref` that aren't plain RAM.
    pub refs: [MemRef; 4],
    /// PF (04h or 0) of every byte value, for hosts without a parity flag.
    pub parity: [u8; 256],
}

pub const CTX_FALLBACK: i32 = offset_of!(JitCtx, fallback) as i32;
pub const CTX_REVALIDATE: i32 = offset_of!(JitCtx, revalidate) as i32;
pub const CTX_EXIT: i32 = offset_of!(JitCtx, exit) as i32;
pub const CTX_RAM: i32 = offset_of!(JitCtx, ram) as i32;
pub const CTX_PAGE_GEN: i32 = offset_of!(JitCtx, page_gen) as i32;
pub const CTX_EXIT_DATA: i32 = offset_of!(JitCtx, exit_data) as i32;
pub const CTX_MEMREF: i32 = offset_of!(JitCtx, memref) as i32;
pub const CTX_READ: i32 = offset_of!(JitCtx, read) as i32;
pub const CTX_WRITE: i32 = offset_of!(JitCtx, write) as i32;
pub const CTX_RAM_LEN: i32 = offset_of!(JitCtx, ram_len) as i32;
pub const CTX_TLB: i32 = offset_of!(JitCtx, tlb) as i32;
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
pub const CTX_PARITY: i32 = offset_of!(JitCtx, parity) as i32;
pub const CTX_SMC_LO: i32 = offset_of!(JitCtx, smc_lo) as i32;
pub const CTX_SMC_HI: i32 = offset_of!(JitCtx, smc_hi) as i32;
pub const CTX_FLAGS: i32 = offset_of!(JitCtx, flags) as i32;
pub const DATA_GEN_SUM: i32 = offset_of!(BlockData, gen_sum) as i32;
pub const DATA_LINKS: i32 = offset_of!(BlockData, links) as i32;
pub const DATA_GUARDS: i32 = offset_of!(BlockData, guards) as i32;
pub const GUARD_SIZE: i32 = std::mem::size_of::<Guard>() as i32;
pub const GUARD_EIP: i32 = offset_of!(Guard, eip) as i32;
pub const GUARD_CS_BASE: i32 = offset_of!(Guard, cs_base) as i32;
pub const GUARD_A20: i32 = offset_of!(Guard, a20) as i32;
pub const GUARD_PAGING: i32 = offset_of!(Guard, paging) as i32;
pub const GUARD_PAGE: i32 = offset_of!(Guard, page) as i32;
pub const GUARD_PHYS: i32 = offset_of!(Guard, phys) as i32;

impl JitCtx {
    pub fn new(exit: usize) -> Self {
        JitCtx {
            fallback: jit_fallback as *const () as usize,
            revalidate: jit_revalidate as *const () as usize,
            exit,
            ram: std::ptr::null(),
            page_gen: std::ptr::null(),
            exit_data: std::ptr::null_mut(),
            memref: jit_memref as *const () as usize,
            read: jit_read as *const () as usize,
            write: jit_write as *const () as usize,
            ram_len: 0,
            tlb: std::ptr::null(),
            smc_lo: 0,
            smc_hi: 0,
            fault: Fault::UD,
            flags: 0,
            panic: None,
            refs: [MemRef { lin: 0, phys: 0, phys2: 0, size: 1 }; 4],
            parity: std::array::from_fn(|b| if (b as u8).count_ones().is_multiple_of(2) { 0x04 } else { 0 }),
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

/// Keep a panic in Rust code that translated code called for the execution
/// loop to resume (unwinding can't cross translated code).
fn guard<R>(ctx: &mut JitCtx, fallback: R, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(payload) => {
            ctx.panic.get_or_insert(payload);
            fallback
        }
    }
}

/// How a MemRef's operand is described to `jit_memref`: the segment, the
/// size, whether it is written, and the slot.
pub fn memref_desc(seg: Seg, size: u8, write: bool, slot: u8) -> u32 {
    seg as u32 | (size as u32) << 4 | (write as u32) << 7 | (slot as u32) << 8
}

jit_fn! {
    /// Check a memory operand at seg:off as `Cpu::mem_ref` does. Returns
    /// its physical address if it is plain RAM within a page, which the
    /// code then accesses itself, else `SLOW + slot` (the checked operand
    /// kept in the slot), or MEMREF_FAULT with the fault in the context.
    fn jit_memref(cpu: *mut Cpu, ctx: *mut JitCtx, off: u32, desc: u32) -> u64 {
        // SAFETY: as in `jit_fallback`.
        let (cpu, ctx) = unsafe { (&mut *cpu, &mut *ctx) };
        let seg = Seg::ALL[(desc & 7) as usize];
        let size = (desc >> 4 & 7) as u8;
        let access = if desc & 0x80 != 0 { Access::Write } else { Access::Read };
        let slot = (desc >> 8 & 3) as usize;
        match guard(ctx, Err(Fault::UD), || cpu.mem_ref(seg, off, size, access)) {
            Ok(r) => {
                if r.phys & 0xFFF <= 0x1000 - size as u32 && cpu.bus.is_plain_ram(r.phys as usize, size as usize) {
                    r.phys as u64
                } else {
                    ctx.refs[slot] = r;
                    (SLOW + slot as u32) as u64
                }
            }
            Err(fault) => {
                ctx.fault = fault;
                MEMREF_FAULT
            }
        }
    }
}

jit_fn! {
    /// Read the operand checked into `slot`.
    fn jit_read(cpu: *mut Cpu, ctx: *mut JitCtx, slot: u32) -> u32 {
        // SAFETY: as in `jit_fallback`.
        let (cpu, ctx) = unsafe { (&mut *cpu, &mut *ctx) };
        let r = ctx.refs[slot as usize & 3];
        guard(ctx, 0, || cpu.mem_read(r))
    }
}

jit_fn! {
    /// Write the operand checked into `slot`. Returns 1 if the write hit
    /// the running block's later bytes (`smc_lo..smc_hi`), else 0.
    fn jit_write(cpu: *mut Cpu, ctx: *mut JitCtx, slot: u32, value: u32) -> u32 {
        // SAFETY: as in `jit_fallback`.
        let (cpu, ctx) = unsafe { (&mut *cpu, &mut *ctx) };
        let r = ctx.refs[slot as usize & 3];
        guard(ctx, (), || cpu.mem_write(r, value));
        let in_first = 0x1000 - (r.phys & 0xFFF);
        let hit = (0..r.size as u32).any(|i| {
            let p = if i < in_first { r.phys + i } else { r.phys2 + i - in_first };
            (ctx.smc_lo..ctx.smc_hi).contains(&p)
        });
        hit as u32
    }
}
