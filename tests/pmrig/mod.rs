//! A protected-mode test machine: a GDT with flat ring 0 and ring 3
//! segments, an IDT, a 32-bit TSS, and a real-mode stub that enters
//! protected mode the way DOS extenders do (LGDT, LIDT, MOV CR0, far JMP,
//! LTR). Test programs are assembled with iced's code assembler.

#![allow(dead_code)]

use iced_x86::code_asm::*;
use rust_dos::cpu::{Cpu, CpuState};
use rust_dos::exec::{ExecHook, StopReason, run_batch};
use std::path::PathBuf;

/// Where things are in memory (linear = physical until a test enables
/// paging).
pub const GDT: u32 = 0x0800;
pub const IDT: u32 = 0x1000;
pub const TSS: u32 = 0x2000;
/// A second TSS, for task switches.
pub const TSS2: u32 = 0x2800;
pub const RESULT: u32 = 0x5000;
pub const SETUP: u32 = 0x7C00;
pub const CODE: u32 = 0x10000;
/// Ring 3 code of tests that switch to ring 3 (see `to_ring3`).
pub const RING3: u32 = 0x18000;
pub const HANDLERS: u32 = 0x20000;
pub const DATA: u32 = 0x40000;
pub const STACK3_TOP: u32 = 0x60000;
pub const STACK0_TOP: u32 = 0x70000;

/// Selectors of the standard GDT.
pub const CODE32: u16 = 0x08;
pub const DATA32: u16 = 0x10;
pub const CODE16: u16 = 0x18;
pub const DATA16: u16 = 0x20;
pub const CODE32_R3: u16 = 0x28 | 3;
pub const DATA32_R3: u16 = 0x30 | 3;
pub const TSS_SEL: u16 = 0x38;
/// The first GDT slot left for tests.
pub const FREE: u16 = 0x40;

/// Access bytes.
pub const CODE_R0: u8 = 0x9A;
pub const DATA_R0: u8 = 0x92;
pub const CODE_R3: u8 = 0xFA;
pub const DATA_R3: u8 = 0xF2;
/// Flags nibble: 4 KB granularity and 32-bit.
pub const G32: u8 = 0xC;

/// A code or data segment descriptor. `flags` is the high nibble of byte
/// 6: G, D/B, 0, AVL.
pub fn seg_desc(base: u32, limit: u32, access: u8, flags: u8) -> u64 {
    (limit as u64 & 0xFFFF)
        | ((base as u64 & 0xFF_FFFF) << 16)
        | ((access as u64) << 40)
        | (((limit as u64 >> 16) & 0xF) << 48)
        | ((flags as u64 & 0xF) << 52)
        | (((base as u64 >> 24) & 0xFF) << 56)
}

/// A system descriptor (TSS, LDT), present, byte granular.
pub fn sys_desc(base: u32, limit: u32, typ: u8, dpl: u8) -> u64 {
    seg_desc(base, limit, 0x80 | (dpl << 5) | typ, 0)
}

/// A present gate descriptor.
pub fn gate_desc(selector: u16, offset: u32, typ: u8, dpl: u8, params: u8) -> u64 {
    (offset as u64 & 0xFFFF)
        | ((selector as u64) << 16)
        | ((params as u64 & 0x1F) << 32)
        | (((0x80 | (dpl << 5) | typ) as u64) << 40)
        | (((offset as u64) >> 16) << 48)
}

pub const INT_GATE32: u8 = 0xE;
pub const TRAP_GATE32: u8 = 0xF;
pub const CALL_GATE32: u8 = 0xC;
pub const TASK_GATE: u8 = 0x5;
pub const TSS32: u8 = 0x9;

/// Assemble 32-bit code to run at `addr`.
pub fn asm32(addr: u32, f: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) -> Vec<u8> {
    let mut a = CodeAssembler::new(32).unwrap();
    f(&mut a).unwrap();
    a.assemble(addr as u64).unwrap()
}

/// Assemble 16-bit code to run at `addr`.
pub fn asm16(addr: u32, f: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) -> Vec<u8> {
    let mut a = CodeAssembler::new(16).unwrap();
    f(&mut a).unwrap();
    a.assemble(addr as u64).unwrap()
}

pub struct Rig {
    pub cpu: Cpu,
}

impl Rig {
    /// A machine in real mode with the tables in memory.
    pub fn new() -> Self {
        let mut cpu = Cpu::new(PathBuf::from("."));
        cpu.bus.set_a20(true);
        // Nothing may interrupt the tests unless they ask for it.
        cpu.bus.io_write(0x21, 0xFF);
        cpu.bus.io_write(0xA1, 0xFF);
        let mut rig = Rig { cpu };
        rig.set_gdt(CODE32, seg_desc(0, 0xFFFFF, CODE_R0, G32));
        rig.set_gdt(DATA32, seg_desc(0, 0xFFFFF, DATA_R0, G32));
        rig.set_gdt(CODE16, seg_desc(0, 0xFFFF, CODE_R0, 0));
        rig.set_gdt(DATA16, seg_desc(0, 0xFFFF, DATA_R0, 0));
        rig.set_gdt(CODE32_R3, seg_desc(0, 0xFFFFF, CODE_R3, G32));
        rig.set_gdt(DATA32_R3, seg_desc(0, 0xFFFFF, DATA_R3, G32));
        rig.set_gdt(TSS_SEL, sys_desc(TSS, 0x67, TSS32, 0));
        // TSS: ring 0 stack; the I/O bitmap offset past the limit (no
        // bitmap: ports need CPL <= IOPL).
        rig.write32(TSS + 4, STACK0_TOP);
        rig.write32(TSS + 8, DATA32 as u32);
        rig.write16(TSS + 0x66, 0x68);
        rig
    }

    pub fn write16(&mut self, addr: u32, v: u16) {
        self.cpu.bus.write_16(addr as usize, v);
    }

    pub fn write32(&mut self, addr: u32, v: u32) {
        self.cpu.bus.write_32(addr as usize, v);
    }

    pub fn read32(&self, addr: u32) -> u32 {
        self.cpu.bus.read_32(addr as usize)
    }

    pub fn load(&mut self, addr: u32, bytes: &[u8]) {
        self.cpu.bus.load_bytes(addr as usize, bytes);
    }

    pub fn set_gdt(&mut self, selector: u16, desc: u64) {
        let at = GDT + (selector & 0xFFF8) as u32;
        self.write32(at, desc as u32);
        self.write32(at + 4, (desc >> 32) as u32);
    }

    pub fn gdt(&self, selector: u16) -> u64 {
        let at = GDT + (selector & 0xFFF8) as u32;
        self.read32(at) as u64 | (self.read32(at + 4) as u64) << 32
    }

    pub fn set_idt(&mut self, vector: u8, desc: u64) {
        let at = IDT + vector as u32 * 8;
        self.write32(at, desc as u32);
        self.write32(at + 4, (desc >> 32) as u32);
    }

    /// Install 32-bit code for interrupt `vector` behind an interrupt gate
    /// of privilege `dpl`.
    pub fn handler(&mut self, vector: u8, dpl: u8, f: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) {
        let addr = HANDLERS + vector as u32 * 0x100;
        let code = asm32(addr, f);
        self.load(addr, &code);
        self.set_idt(vector, gate_desc(CODE32, addr, INT_GATE32, dpl, 0));
    }

    /// A handler that records the vector at RESULT and the 8 dwords on its
    /// stack (error code, if any, then EIP, CS, EFLAGS, ESP, SS...) after
    /// it, then halts.
    pub fn record(&mut self, vector: u8) {
        self.handler(vector, 0, |a| record_code(a, vector));
    }

    /// The recorded vector and stack.
    pub fn recorded(&self) -> (u32, [u32; 8]) {
        let mut stack = [0; 8];
        for (i, s) in stack.iter_mut().enumerate() {
            *s = self.read32(RESULT + 4 + 4 * i as u32);
        }
        (self.read32(RESULT), stack)
    }

    /// Run the real-mode stub that switches to protected mode, loads the
    /// flat ring 0 segments, the stack and TR, and jumps to `CODE`.
    pub fn enter_pm(&mut self) {
        // GDTR and IDTR images for LGDT and LIDT.
        self.write16(0x7B00, 0x07FF);
        self.write32(0x7B02, GDT);
        self.write16(0x7B08, 0x07FF);
        self.write32(0x7B0A, IDT);
        let pm_entry = SETUP + 0x40;
        let stub = asm16(SETUP, |a| {
            a.cli()?;
            a.lgdt(ptr(0x7B00))?;
            a.lidt(ptr(0x7B08))?;
            a.mov(eax, cr0)?;
            a.or(eax, 1)?;
            a.mov(cr0, eax)?;
            // JMP 0008:pm_entry with a 32-bit offset.
            a.db(&[0x66, 0xEA])?;
            a.dd(&[pm_entry])?;
            a.dw(&[CODE32])?;
            Ok(())
        });
        assert!(stub.len() <= 0x40);
        self.load(SETUP, &stub);
        let entry = asm32(pm_entry, |a| {
            a.mov(ax, DATA32 as u32)?;
            a.mov(ds, ax)?;
            a.mov(es, ax)?;
            a.mov(fs, ax)?;
            a.mov(gs, ax)?;
            a.mov(ss, ax)?;
            a.mov(esp, STACK0_TOP)?;
            a.mov(ax, TSS_SEL as u32)?;
            a.ltr(ax)?;
            a.mov(eax, CODE)?;
            a.jmp(eax)?;
            Ok(())
        });
        self.load(pm_entry, &entry);
        self.cpu.set_cs(0);
        self.cpu.set_ip(SETUP as u16);
        for _ in 0..100 {
            if self.cpu.eip() == CODE {
                return;
            }
            self.cpu.step();
        }
        panic!("didn't reach protected mode: CS:EIP {:04X}:{:08X}", self.cpu.cs(), self.cpu.eip());
    }

    /// Put `code` at CODE (assembled for that address), enter protected
    /// mode and run until the CPU halts.
    pub fn run(&mut self, f: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) {
        let code = asm32(CODE, f);
        self.load(CODE, &code);
        self.enter_pm();
        self.run_to_halt();
    }

    /// Put ring 3 code at RING3 (see `to_ring3`).
    pub fn ring3(&mut self, f: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) {
        let code = asm32(RING3, f);
        self.load(RING3, &code);
    }

    /// Step until HLT or a reset that ends the program (at most a million
    /// instructions).
    pub fn run_to_halt(&mut self) {
        for _ in 0..1_000_000 {
            if self.cpu.state != CpuState::Running {
                return;
            }
            self.cpu.step();
        }
        panic!("no HLT: CS:EIP {:04X}:{:08X}", self.cpu.cs(), self.cpu.eip());
    }
}

/// Stops the execution loop before a HLT instruction.
struct StopAtHlt;

impl ExecHook for StopAtHlt {
    fn before_exec(&mut self, _cpu: &Cpu, phys_ip: usize, ram: &[u8]) -> bool {
        ram.get(phys_ip) == Some(&0xF4)
    }

    /// The dynamic recompiler runs blocks between the calls: it leaves
    /// every HLT to the interpreter, so the hook sees them all.
    fn per_instruction(&self) -> bool {
        false
    }
}

impl Rig {
    /// Like `run`, but the program runs the way the emulator runs programs:
    /// in batches through `run_batch`, whose instruction fetch differs from
    /// `Cpu::step`'s (it keeps a code window, see exec.rs).
    pub fn run_batched(&mut self, f: impl FnOnce(&mut CodeAssembler) -> Result<(), IcedError>) {
        let code = asm32(CODE, f);
        self.load(CODE, &code);
        self.enter_pm();
        self.run_batched_to_halt();
    }

    /// Run through `run_batch` until the next instruction is a HLT (HLT
    /// itself waits for the next timer event there and goes on), at most a
    /// million instructions.
    pub fn run_batched_to_halt(&mut self) {
        let start = self.cpu.executed;
        while self.cpu.executed - start < 1_000_000 {
            let end = self.cpu.bus.clock.icount + 10_000;
            self.cpu.bus.start_batch(end);
            if run_batch(&mut self.cpu, &mut StopAtHlt, true) == StopReason::Paused {
                return;
            }
        }
        panic!("no HLT: CS:EIP {:04X}:{:08X}", self.cpu.cs(), self.cpu.eip());
    }
}

/// Code that continues at ring 3 at RING3, on the ring 3 stack, by
/// building an IRET frame.
pub fn to_ring3(a: &mut CodeAssembler) -> Result<(), IcedError> {
    a.push(DATA32_R3 as u32)?;
    a.push(STACK3_TOP)?;
    a.pushfd()?;
    a.push(CODE32_R3 as u32)?;
    a.push(RING3)?;
    a.iretd()
}

/// Code of `Rig::record`.
pub fn record_code(a: &mut CodeAssembler, vector: u8) -> Result<(), IcedError> {
    a.push(eax)?;
    a.mov(ax, DATA32 as u32)?;
    a.mov(ds, ax)?;
    a.mov(es, ax)?;
    a.pop(eax)?;
    a.mov(dword_ptr(RESULT), vector as u32)?;
    a.mov(esi, esp)?;
    a.mov(edi, RESULT + 4)?;
    a.mov(ecx, 8u32)?;
    a.cld()?;
    a.rep().movsd()?;
    a.hlt()?;
    Ok(())
}
