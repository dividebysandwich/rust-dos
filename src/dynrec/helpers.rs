//! What translated code calls and reads besides the CPU: the context the
//! execution loop enters it with, and the Rust functions it calls.

use std::any::Any;
use std::mem::offset_of;
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::block::{BlockData, Guard, Return};
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
/// The instruction ran and changed what the execution loop checks
/// between instructions (see `block::ends_block`): the block stops after
/// it.
pub const EXIT_AFTER: u32 = 12;
/// The instruction starts in the page's last 15 bytes, and with paging the
/// TLB doesn't hold the next page, which the interpreter's fetch looks up
/// (and may walk the page tables for): the instruction didn't run (see
/// `BlockData::in_tail`).
pub const EXIT_NEXT_PAGE: u32 = 13;
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
    /// `Bus::code_blocks`.
    pub code_blocks: *const u8,
    /// Set where a store hit the running block's later bytes (x86-64).
    pub smc: u8,
    /// The stack's width the running blocks were translated for.
    pub stack32: bool,
    /// The `ENV_FLAT` and `ENV_PLAIN` bits now: the running blocks were translated for
    /// them, and a segment load in a block changes them (see
    /// `block::loaded_segment`).
    pub flat: u32,
    /// The block the code returned from.
    pub exit_data: *mut BlockData,
    /// `jit_memref`, `jit_read` and `jit_write`.
    pub memref: usize,
    pub read: usize,
    pub write: usize,
    /// `jit_load_seg` and `jit_port`.
    pub load_seg: usize,
    pub port: usize,
    /// Where `jit_port` asks for the block to stop after the instruction:
    /// EXIT_AFTER, or EXIT_SMC where a device wrote its later bytes.
    pub after: u8,
    /// Bytes of RAM.
    pub ram_len: u64,
    /// The TLB's entries.
    pub tlb: *const u8,
    /// The engine's places returns went to (`Return`).
    pub returns: *const Return,
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
    /// `jit_fpu_addsub_st`, `jit_fpu_addsub_value`, `jit_fpu_to_int` and
    /// `jit_fpu_div_zero` (x86-64).
    pub fpu: [usize; 4],
    /// `jit_dev_read` and `jit_dev_write` (x86-64).
    pub dev: [usize; 2],
}

pub const CTX_FALLBACK: i32 = offset_of!(JitCtx, fallback) as i32;
pub const CTX_REVALIDATE: i32 = offset_of!(JitCtx, revalidate) as i32;
pub const CTX_EXIT: i32 = offset_of!(JitCtx, exit) as i32;
pub const CTX_RAM: i32 = offset_of!(JitCtx, ram) as i32;
pub const CTX_PAGE_GEN: i32 = offset_of!(JitCtx, page_gen) as i32;
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub const CTX_CODE_BLOCKS: i32 = offset_of!(JitCtx, code_blocks) as i32;
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub const CTX_SMC: i32 = offset_of!(JitCtx, smc) as i32;
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub const CTX_FLAT: i32 = offset_of!(JitCtx, flat) as i32;
pub const CTX_EXIT_DATA: i32 = offset_of!(JitCtx, exit_data) as i32;
pub const CTX_MEMREF: i32 = offset_of!(JitCtx, memref) as i32;
pub const CTX_READ: i32 = offset_of!(JitCtx, read) as i32;
pub const CTX_WRITE: i32 = offset_of!(JitCtx, write) as i32;
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub const CTX_LOAD_SEG: i32 = offset_of!(JitCtx, load_seg) as i32;
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub const CTX_PORT: i32 = offset_of!(JitCtx, port) as i32;
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub const CTX_AFTER: i32 = offset_of!(JitCtx, after) as i32;
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
pub const CTX_RAM_LEN: i32 = offset_of!(JitCtx, ram_len) as i32;
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
pub const CTX_TLB: i32 = offset_of!(JitCtx, tlb) as i32;
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
pub const CTX_PARITY: i32 = offset_of!(JitCtx, parity) as i32;
pub const CTX_SMC_LO: i32 = offset_of!(JitCtx, smc_lo) as i32;
pub const CTX_SMC_HI: i32 = offset_of!(JitCtx, smc_hi) as i32;
pub const CTX_FLAGS: i32 = offset_of!(JitCtx, flags) as i32;
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub const CTX_FPU: i32 = offset_of!(JitCtx, fpu) as i32;
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub const CTX_DEV: i32 = offset_of!(JitCtx, dev) as i32;
/// On x86-64, a memory operand handle with this bit set is the physical
/// address (its low dword) of an operand within one page that isn't plain
/// RAM (video memory, a frame buffer, a card's registers): loads and
/// stores go through `jit_dev_read` and `jit_dev_write`.
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
pub const DEV_BIT: u8 = 32;
pub const DATA_GEN_SUM: i32 = offset_of!(BlockData, gen_sum) as i32;
pub const DATA_LINKS: i32 = offset_of!(BlockData, links) as i32;
pub const DATA_GUARDS: i32 = offset_of!(BlockData, guards) as i32;
pub const GUARD_SIZE: i32 = std::mem::size_of::<Guard>() as i32;
pub const GUARD_EIP: i32 = offset_of!(Guard, eip) as i32;
pub const GUARD_CS_BASE: i32 = offset_of!(Guard, cs_base) as i32;
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
pub const GUARD_A20: i32 = offset_of!(Guard, a20) as i32;
#[cfg_attr(not(target_arch = "aarch64"), allow(dead_code))]
pub const GUARD_PAGING: i32 = offset_of!(Guard, paging) as i32;
pub const GUARD_PAGE: i32 = offset_of!(Guard, page) as i32;
pub const GUARD_PHYS: i32 = offset_of!(Guard, phys) as i32;
pub const CTX_RETURNS: i32 = offset_of!(JitCtx, returns) as i32;
pub const RETURN_GUARD: i32 = offset_of!(Return, guard) as i32;
pub const RETURN_MODE: i32 = offset_of!(Return, mode) as i32;
pub const RETURN_CODE: i32 = offset_of!(Return, code) as i32;
/// (The code indexes the table with a scale of 5, then 8.)
const _: () = assert!(std::mem::size_of::<Return>() == 40);

impl JitCtx {
    pub fn new(exit: usize) -> Self {
        JitCtx {
            fallback: jit_fallback as *const () as usize,
            revalidate: jit_revalidate as *const () as usize,
            exit,
            ram: std::ptr::null(),
            page_gen: std::ptr::null(),
            code_blocks: std::ptr::null(),
            smc: 0,
            flat: 0,
            stack32: false,
            exit_data: std::ptr::null_mut(),
            memref: jit_memref as *const () as usize,
            read: jit_read as *const () as usize,
            write: jit_write as *const () as usize,
            load_seg: jit_load_seg as *const () as usize,
            port: jit_port as *const () as usize,
            after: 0,
            ram_len: 0,
            tlb: std::ptr::null(),
            returns: std::ptr::null(),
            smc_lo: 0,
            smc_hi: 0,
            fault: Fault::UD,
            flags: 0,
            panic: None,
            refs: [MemRef { lin: 0, phys: 0, phys2: 0, size: 1 }; 4],
            parity: std::array::from_fn(|b| if (b as u8).count_ones().is_multiple_of(2) { 0x04 } else { 0 }),
            #[cfg(target_arch = "x86_64")]
            fpu: [
                jit_fpu_addsub_st as *const () as usize,
                jit_fpu_addsub_value as *const () as usize,
                jit_fpu_to_int as *const () as usize,
                jit_fpu_div_zero as *const () as usize,
            ],
            #[cfg(not(target_arch = "x86_64"))]
            fpu: [0; 4],
            #[cfg(target_arch = "x86_64")]
            dev: [jit_dev_read as *const () as usize, jit_dev_write as *const () as usize],
            #[cfg(not(target_arch = "x86_64"))]
            dev: [0; 2],
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
    /// EXIT_SMC if it wrote over the rest of the block, EXIT_AFTER if it
    /// changed what the execution loop checks, or EXIT_PANIC.
    fn jit_fallback(cpu: *mut Cpu, ctx: *mut JitCtx, data: *mut BlockData, ix: u32) -> u32 {
        // SAFETY: translated code passes the CPU and context the execution
        // loop entered it with, and its own block, which outlive the call.
        let (cpu, ctx, data) = unsafe { (&mut *cpu, &mut *ctx, &mut *data) };
        let ix = ix as usize;
        let instr = &data.instrs[ix];
        let handler = data.handlers[ix];
        let start_esp = cpu.esp();
        cpu.set_eip(data.eips[ix].wrapping_add(instr.len() as u32));
        let port = super::block::port_io(instr);
        // Port I/O and long string instructions take time of their own.
        let timed = port || super::block::repeated_string(instr);
        let sti = instr.mnemonic() == iced_x86::Mnemonic::Sti;
        let popf = matches!(instr.mnemonic(), iced_x86::Mnemonic::Popf | iced_x86::Mnemonic::Popfd);
        let seg_load = super::block::loaded_segment(instr);
        // A device may write RAM (by DMA) as well.
        let writes = data.writes[ix] || port;
        let before = if writes { data.gens_now(&cpu.bus.page_gen) } else { 0 };
        let time = (cpu.bus.clock.deadline, cpu.bus.a20_mask());
        match catch_unwind(AssertUnwindSafe(|| handler(cpu, instr))) {
            Ok(Ok(())) => {
                if writes && data.gens_now(&cpu.bus.page_gen) != before {
                    let code = written(cpu, data, ix);
                    if code != 0 {
                        return code;
                    }
                }
                if timed && loop_would_act(cpu, time, (data.count() - ix) as u64) {
                    return EXIT_AFTER;
                }
                if let Some(seg) = seg_load {
                    // The rest of the block checks the segment's accesses as
                    // it does a segment's that isn't flat, and follows its
                    // links only where the segments are flat as they were.
                    ctx.flat = ctx.flat & !((super::ENV_FLAT | super::ENV_PLAIN) << seg as u32) | super::flat_bit(cpu, seg);
                }
                if popf
                    && (cpu.get_cpu_flag(crate::cpu::CpuFlags::TF)
                        || cpu.bus.irq_ready && cpu.get_cpu_flag(crate::cpu::CpuFlags::IF))
                {
                    // A single-step trap to come, or an interrupt POPF let
                    // through: the execution loop takes them.
                    return EXIT_AFTER;
                }
                if seg_load == Some(Seg::SS) {
                    // The rest of the block, and the blocks linked to it, were
                    // translated for the stack's width as it was.
                    if cpu.stack32() != ctx.stack32 {
                        return EXIT_AFTER;
                    }
                    // Interrupts wait for the instruction after a stack
                    // switch, which runs in the block (as after STI).
                    if ix + 1 < data.count() {
                        cpu.irq_shadow = false;
                    }
                }
                if sti {
                    // Interrupts are recognized after the next instruction:
                    // with one waiting the execution loop runs that. Else no
                    // interrupt can become deliverable in the rest of the
                    // block, where the next instruction runs, but after port
                    // I/O, which checks for one.
                    if cpu.bus.irq_ready {
                        return EXIT_AFTER;
                    }
                    if ix + 1 < data.count() {
                        cpu.irq_shadow = false;
                    }
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

jit_fn! {
    /// Load segment register `seg` (a `Seg`) with `selector` for a
    /// translated MOV or POP, noting which segments are flat after it (see
    /// `JitCtx::flat`). Returns 0, or EXIT_FAULT with the fault in the
    /// context (or EXIT_PANIC).
    fn jit_load_seg(cpu: *mut Cpu, ctx: *mut JitCtx, seg: u32, selector: u32) -> u32 {
        // SAFETY: as in `jit_fallback`.
        let (cpu, ctx) = unsafe { (&mut *cpu, &mut *ctx) };
        let seg = Seg::ALL[seg as usize];
        match catch_unwind(AssertUnwindSafe(|| cpu.load_segment(seg, selector as u16))) {
            Ok(Ok(())) => {
                ctx.flat = ctx.flat & !((super::ENV_FLAT | super::ENV_PLAIN) << seg as u32) | super::flat_bit(cpu, seg);
                0
            }
            Ok(Err(fault)) => {
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

jit_fn! {
    /// A translated IN or OUT of instruction `desc >> 8 & 0xFF` of the block
    /// (bit 0 of `desc` set for OUT, bits 1-3 the size), as `port_in` and
    /// `port_out` do it and `jit_fallback` runs them: EIP on the next
    /// instruction while it runs, and the block asked to stop after it
    /// (`JitCtx::after`) where a device wrote its later bytes or the port
    /// access changed what the execution loop checks. Returns the value
    /// read, with EXIT_FAULT or EXIT_PANIC in bits 32 and up where it
    /// didn't run.
    fn jit_port(cpu: *mut Cpu, ctx: *mut JitCtx, data: *mut BlockData, desc: u32, port: u32, value: u32) -> u64 {
        // SAFETY: as in `jit_fallback`.
        let (cpu, ctx, data) = unsafe { (&mut *cpu, &mut *ctx, &mut *data) };
        let (out, size, ix) = (desc & 1 != 0, (desc >> 1 & 7) as u8, (desc >> 8 & 0xFF) as usize);
        let port = port as u16;
        cpu.set_eip(data.eips[ix].wrapping_add(data.instrs[ix].len() as u32));
        // A device may write RAM (by DMA).
        let before = data.gens_now(&cpu.bus.page_gen);
        let time = (cpu.bus.clock.deadline, cpu.bus.a20_mask());
        let access = || -> Result<u32, Fault> {
            cpu.check_io(port, size)?;
            Ok(match (out, size) {
                (true, 1) => {
                    cpu.bus.io_write(port, value as u8);
                    0
                }
                (true, _) => {
                    cpu.bus.io_write_wide(port, value, size);
                    0
                }
                (false, 1) => cpu.bus.io_read(port) as u32,
                (false, _) => cpu.bus.io_read_wide(port, size),
            })
        };
        match catch_unwind(AssertUnwindSafe(access)) {
            Ok(Ok(read)) => {
                if data.gens_now(&cpu.bus.page_gen) != before && written(cpu, data, ix) != 0 {
                    ctx.after = EXIT_SMC as u8;
                } else if loop_would_act(cpu, time, (data.count() - ix) as u64) {
                    ctx.after = EXIT_AFTER as u8;
                }
                read as u64
            }
            Ok(Err(fault)) => {
                ctx.fault = fault;
                (EXIT_FAULT as u64) << 32
            }
            Err(payload) => {
                ctx.panic = Some(payload);
                (EXIT_PANIC as u64) << 32
            }
        }
    }
}

/// Whether port I/O (or a long string instruction) changed what the
/// execution loop checks before the next instruction: an interrupt to
/// deliver, the next timer event (the block checked it would end before the
/// old one, but a port access or a string's elements take time: the `left`
/// instructions of the block from this one on must still fit), the A20 gate (the code window), a reset, or a CPU no longer
/// running. `time` is the timer deadline and A20 mask before it.
fn loop_would_act(cpu: &Cpu, time: (u64, u32), left: u64) -> bool {
    (cpu.bus.irq_ready && cpu.get_cpu_flag(crate::cpu::CpuFlags::IF))
        || (cpu.bus.clock.deadline, cpu.bus.a20_mask()) != time
        || cpu.bus.clock.icount + left > cpu.bus.clock.deadline
        || cpu.bus.reset_requested
        || cpu.state != crate::cpu::CpuState::Running
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
    /// code then accesses itself; on x86-64, that address with `DEV_BIT`
    /// if it is something else within a page; else `SLOW + slot` (the
    /// checked operand kept in the slot), or MEMREF_FAULT with the fault
    /// in the context.
    fn jit_memref(cpu: *mut Cpu, ctx: *mut JitCtx, off: u32, desc: u32) -> u64 {
        // SAFETY: as in `jit_fallback`.
        let (cpu, ctx) = unsafe { (&mut *cpu, &mut *ctx) };
        let seg = Seg::ALL[(desc & 7) as usize];
        let size = (desc >> 4 & 7) as u8;
        let access = if desc & 0x80 != 0 { Access::Write } else { Access::Read };
        let slot = (desc >> 8 & 3) as usize;
        // (The operand goes straight into its slot: handed back through
        // the guard, its two dwords would be stored apart and loaded as
        // one, which the host can't forward.)
        let refs = &mut ctx.refs[slot];
        let fault = match catch_unwind(AssertUnwindSafe(|| match cpu.mem_ref(seg, off, size, access) {
            Ok(r) => {
                *refs = r;
                None
            }
            Err(fault) => Some(fault),
        })) {
            Ok(fault) => fault,
            Err(payload) => {
                ctx.panic.get_or_insert(payload);
                Some(Fault::UD)
            }
        };
        if let Some(fault) = fault {
            ctx.fault = fault;
            return MEMREF_FAULT;
        }
        let phys = ctx.refs[slot].phys;
        if phys & 0xFFF > 0x1000 - size as u32 {
            (SLOW + slot as u32) as u64
        } else if cpu.bus.is_plain_ram(phys as usize, size as usize) {
            phys as u64
        } else if cfg!(target_arch = "x86_64") {
            1 << DEV_BIT | phys as u64
        } else {
            (SLOW + slot as u32) as u64
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

/// Run a piece of an FPU instruction's handler for translated code: a
/// panic in it resumes in the execution loop, as `jit_fallback`'s does.
#[cfg(target_arch = "x86_64")]
fn fpu_helper<R: Default>(cpu: *mut Cpu, ctx: *mut JitCtx, f: impl FnOnce(&mut Cpu) -> R) -> R {
    // SAFETY: as in `jit_fallback`.
    let (cpu, ctx) = unsafe { (&mut *cpu, &mut *ctx) };
    match catch_unwind(AssertUnwindSafe(|| f(cpu))) {
        Ok(value) => value,
        Err(payload) => {
            ctx.panic = Some(payload);
            R::default()
        }
    }
}

#[cfg(target_arch = "x86_64")]
jit_fn! {
    /// `Uop::FAddSt`: `desc` is dst, a << 4, b << 8 and sub << 12.
    fn jit_fpu_addsub_st(cpu: *mut Cpu, ctx: *mut JitCtx, desc: u32) -> u32 {
        let (dst, a, b) = ((desc & 7) as usize, (desc >> 4 & 7) as usize, (desc >> 8 & 7) as usize);
        fpu_helper(cpu, ctx, |cpu| crate::instructions::fpu::arithmetic::addsub_st(cpu, dst, a, b, desc >> 12 & 1 != 0));
        0
    }
}

#[cfg(target_arch = "x86_64")]
jit_fn! {
    /// `Uop::FAddValue`.
    fn jit_fpu_addsub_value(cpu: *mut Cpu, ctx: *mut JitCtx, kind: u32, value: f64) -> u32 {
        fpu_helper(cpu, ctx, |cpu| crate::instructions::fpu::arithmetic::addsub_value(cpu, kind, value));
        0
    }
}

#[cfg(target_arch = "x86_64")]
jit_fn! {
    /// `Uop::FToInt`, for what the code's own conversion doesn't cover:
    /// rounding up or down, and values that don't fit.
    fn jit_fpu_to_int(cpu: *mut Cpu, ctx: *mut JitCtx, size: u32, value: f64) -> u32 {
        fpu_helper(cpu, ctx, |cpu| crate::instructions::fpu::data::to_int(value, cpu.fpu_control, size as u8))
    }
}

#[cfg(target_arch = "x86_64")]
jit_fn! {
    /// `Uop::FDiv` by 0: `desc` is the register, and ze << 8.
    fn jit_fpu_div_zero(cpu: *mut Cpu, ctx: *mut JitCtx, desc: u32) -> u32 {
        fpu_helper(cpu, ctx, |cpu| {
            crate::instructions::fpu::arithmetic::divided_by_zero(cpu, (desc & 7) as usize, desc >> 8 & 1 != 0)
        });
        0
    }
}

#[cfg(target_arch = "x86_64")]
jit_fn! {
    /// Read `size` bytes at physical address `phys`, an operand within one
    /// page that isn't plain RAM (`DEV_BIT`), as `Cpu::mem_read` reads it.
    fn jit_dev_read(cpu: *mut Cpu, ctx: *mut JitCtx, phys: u32, size: u32) -> u32 {
        // SAFETY: as in `jit_fallback`.
        let (cpu, ctx) = unsafe { (&mut *cpu, &mut *ctx) };
        let p = phys as usize;
        guard(ctx, 0, || match size {
            1 => cpu.bus.read_8(p) as u32,
            2 => cpu.bus.read_16(p) as u32,
            _ => cpu.bus.read_32(p),
        })
    }
}

#[cfg(target_arch = "x86_64")]
jit_fn! {
    /// Write such an operand, as `Cpu::mem_write` writes it. Returns 1 if
    /// the write hit the running block's later bytes (`smc_lo..smc_hi`),
    /// else 0.
    fn jit_dev_write(cpu: *mut Cpu, ctx: *mut JitCtx, phys: u32, value: u32, size: u32) -> u32 {
        // SAFETY: as in `jit_fallback`.
        let (cpu, ctx) = unsafe { (&mut *cpu, &mut *ctx) };
        // Pixels into the VGA's planes, which mode X programs write one or
        // two at a time. (They can't be in the block's bytes, which are in
        // RAM.)
        if cpu.bus.write_planes_plainly(phys as usize, value, size as usize) {
            return 0;
        }
        dev_write(cpu, ctx, phys, value, size)
    }
}

/// `jit_dev_write` but for pixels into the VGA's planes.
#[cfg(target_arch = "x86_64")]
#[inline(never)]
fn dev_write(cpu: &mut Cpu, ctx: &mut JitCtx, phys: u32, value: u32, size: u32) -> u32 {
    let p = phys as usize;
    guard(ctx, (), || match size {
        1 => _ = cpu.bus.write_8(p, value as u8),
        2 => _ = cpu.bus.write_16(p, value as u16),
        _ => _ = cpu.bus.write_32(p, value),
    });
    (phys < ctx.smc_hi && phys.wrapping_add(size) > ctx.smc_lo) as u32
}
