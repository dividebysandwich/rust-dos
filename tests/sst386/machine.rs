//! The emulator side of the harness: put a test's initial state into a
//! `Cpu`, run it to the HLT, and read the result back.
//!
//! This is the only part of the harness that uses the emulator's API, so
//! it is the place to adapt when the CPU changes (new register accessors,
//! a larger RAM, a different EFLAGS setter).

use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::rc::Rc;

use rust_dos::cpu::{Cpu, CpuModel, CpuState};
use rust_dos::instr_cache::InstrCache;

use crate::moo::{self, Regs, Test};

/// log2 of the decoded-instruction cache size. A test runs a handful of
/// instructions, so a small cache is plenty and cheap to rebuild after a
/// panic took the CPU's cache with it.
const DECODE_CACHE_LOG2: u32 = 8;

/// The EFLAGS bits a 386 has: CF, 1, PF, AF, ZF, SF, TF, IF, DF, OF, IOPL,
/// NT, RF, VM, plus the always-zero bits 3, 5 and 15. The suite's register
/// dumps come from SMM, which stores 1s in bits 18-31; those are not flags.
pub const EFLAGS_386: u32 = 0x0003_FFFF;

/// How a test's execution ended.
pub enum RunEnd {
    /// The CPU executed a HLT.
    Halted,
    /// No HLT within the step budget.
    NoHalt,
    /// The exec loop's IVT tripwire fired (CS=0, IP<100h) and asked for a
    /// shell reload.
    Tripwire,
    /// CS:IP reached an emulator service trap (FE 38/FE 39) after `steps`
    /// instructions.
    ServiceTrap { steps: usize },
    /// The emulator panicked.
    Panic(String),
}

pub struct Machine {
    pub cpu: Cpu,
    /// Emulator log lines of the current test.
    pub log: Rc<RefCell<Vec<String>>>,
    /// `bus.page_gen` before the test: pages whose generation moved were
    /// written and get cleared afterwards.
    gen_before: Vec<u32>,
}

impl Machine {
    pub fn new() -> Self {
        let mut cpu = Cpu::new(PathBuf::from("."));
        cpu.decode_cache = InstrCache::new(DECODE_CACHE_LOG2);
        // The suite was captured on a 386EX.
        cpu.model = CpuModel::I386;

        // The emulator's log lines are collected by the hook so failures
        // can show them.
        let log = Rc::new(RefCell::new(Vec::new()));
        let hook_log = log.clone();
        cpu.bus.log_hook = Some(Box::new(move |line: &str| {
            let mut log = hook_log.borrow_mut();
            if log.len() < 8 {
                log.push(line.to_string());
            }
        }));

        // The tests assume flat RAM that starts out with nothing in it: clear
        // the BIOS data area, IVT and ROM stubs the bus sets up.
        let len = cpu.bus.ram().len();
        cpu.bus.fill_ram(0..len, 0);
        cpu.bus.vga.vram_graphics.fill(0);
        cpu.bus.vga.vram_text.fill(0);

        let gen_before = cpu.bus.page_gen.to_vec();
        Self {
            cpu,
            log,
            gen_before,
        }
    }

    pub fn ram_len(&self) -> usize {
        self.cpu.bus.ram().len()
    }

    /// Reset the devices a test can reach through the CPU: no timer or IRQ
    /// may interrupt it, and the VGA window must behave like plain memory.
    fn reset_platform(&mut self) {
        let cpu = &mut self.cpu;
        cpu.state = CpuState::Running;
        cpu.irq_shadow = false;
        cpu.idle = false;

        // No timer event is due while a CPU is stepped outside a batch (and
        // HLT then leaves it halted); an OUT to the PIT reschedules it.
        cpu.bus.clock.deadline = u64::MAX;
        // Mask every IRQ at the PIC so POPF/IRET/STI setting IF can't let
        // one in.
        cpu.bus.pic = rust_dos::pic::Pic::new();
        cpu.bus.pic.master.imr = 0xFF;
        // The 386EX the suite comes from has no A20 gate.
        cpu.bus.set_a20(true);

        // A0000-AFFFF is VGA memory. In chain-4 mode with all planes
        // enabled, write mode 0, no set/reset, rotate or logical op and a
        // full bit mask, each byte there reads back what was written, like
        // RAM. (B8000-BFFFF, the text buffer, always does.)
        let vga = &mut cpu.bus.vga;
        vga.sequencer_regs[2] = 0x0F;
        vga.sequencer_regs[4] = 0x0E;
        vga.graphics_regs[0] = 0;
        vga.graphics_regs[1] = 0;
        vga.graphics_regs[3] = 0;
        vga.graphics_regs[5] = 0x40;
        vga.graphics_regs[8] = 0xFF;
        vga.dirty = false;
    }

    /// Load the test's initial registers and memory.
    pub fn load(&mut self, test: &Test) {
        self.reset_platform();
        self.log.borrow_mut().clear();
        self.gen_before.copy_from_slice(&self.cpu.bus.page_gen);

        let bus = &mut self.cpu.bus;
        for &(addr, value) in &test.initial_ram {
            // Raw RAM feeds instruction fetch; the bus write lands where
            // data reads look (the VGA planes inside the VGA window, RAM
            // everywhere else).
            bus.load_bytes(addr as usize, &[value]);
            bus.write_8(addr as usize, value);
        }

        let r = &test.initial;
        let cpu = &mut self.cpu;
        let get = |slot| r.get(slot).unwrap_or(0);
        cpu.set_eax(get(moo::EAX));
        cpu.set_ebx(get(moo::EBX));
        cpu.set_ecx(get(moo::ECX));
        cpu.set_edx(get(moo::EDX));
        cpu.set_esi(get(moo::ESI));
        cpu.set_edi(get(moo::EDI));
        cpu.set_ebp(get(moo::EBP));
        cpu.set_esp(get(moo::ESP));
        cpu.set_cs(get(moo::CS) as u16);
        cpu.set_ds(get(moo::DS) as u16);
        cpu.set_es(get(moo::ES) as u16);
        cpu.set_fs(get(moo::FS) as u16);
        cpu.set_gs(get(moo::GS) as u16);
        cpu.set_ss(get(moo::SS) as u16);
        cpu.set_eip(get(moo::EIP));
        cpu.load_eflags(get(moo::EFLAGS) & EFLAGS_386);
        if let Some(cr0) = r.get(moo::CR0) {
            cpu.cr0 = cr0;
        }
    }

    /// The register as the harness compares it, or None if the emulator
    /// doesn't model it (CR0, CR3, DR6, DR7).
    pub fn reg(&self, slot: usize) -> Option<u32> {
        let cpu = &self.cpu;
        Some(match slot {
            moo::EAX => cpu.eax(),
            moo::EBX => cpu.ebx(),
            moo::ECX => cpu.ecx(),
            moo::EDX => cpu.edx(),
            moo::ESI => cpu.esi(),
            moo::EDI => cpu.edi(),
            moo::EBP => cpu.ebp(),
            moo::ESP => cpu.esp(),
            moo::CS => cpu.cs() as u32,
            moo::DS => cpu.ds() as u32,
            moo::ES => cpu.es() as u32,
            moo::FS => cpu.fs() as u32,
            moo::GS => cpu.gs() as u32,
            moo::SS => cpu.ss() as u32,
            moo::EIP => cpu.eip(),
            moo::EFLAGS => cpu.get_cpu_flags().bits(),
            moo::CR0 => cpu.cr0,
            moo::CR3 | moo::DR6 | moo::DR7 => return None,
            _ => return None,
        })
    }

    /// A byte as the CPU's data reads see it.
    pub fn mem(&self, addr: u32) -> u8 {
        self.cpu.bus.peek_8(addr as usize)
    }

    /// The two bytes at CS:IP.
    fn at_ip(&self) -> (u8, u8) {
        let cpu = &self.cpu;
        let phys = cpu.get_physical_addr(cpu.cs(), cpu.ip());
        let ram = cpu.bus.ram();
        let byte = |a: usize| ram.get(a).copied().unwrap_or(0);
        (byte(phys), byte(phys + 1))
    }

    /// Step until the CPU halts, at most `budget` instructions.
    pub fn run(&mut self, budget: usize) -> RunEnd {
        for steps in 0..budget {
            let (op, next) = self.at_ip();
            if op == 0xFE && (next == 0x38 || next == 0x39) {
                // A BOP runs an emulator service (DOS, BIOS, shell) on
                // whatever the registers hold. Never execute one.
                return RunEnd::ServiceTrap { steps };
            }
            let at_hlt = op == 0xF4;
            let cpu = &mut self.cpu;
            if let Err(payload) = panic::catch_unwind(AssertUnwindSafe(|| cpu.step())) {
                // The decode cache was lent to the unwound exec loop.
                self.cpu.decode_cache = InstrCache::new(DECODE_CACHE_LOG2);
                return RunEnd::Panic(panic_message(payload));
            }
            match self.cpu.state {
                CpuState::Halted => return RunEnd::Halted,
                CpuState::RebootShell => return RunEnd::Tripwire,
                // A HLT that found a timer event scheduled resumes at once.
                CpuState::Running if at_hlt => return RunEnd::Halted,
                CpuState::Running => {}
            }
        }
        RunEnd::NoHalt
    }

    /// Clear everything the test wrote: the 4 KiB pages whose generation
    /// moved (the test's own bytes and any stray writes) and, if touched,
    /// the VGA memory.
    pub fn clean(&mut self) {
        let len = self.cpu.bus.ram().len();
        let pages = self.gen_before.len();
        for index in 0..pages {
            if self.cpu.bus.page_gen[index] == self.gen_before[index] {
                continue;
            }
            // page_gen is indexed by page number modulo its length.
            let mut start = index << 12;
            while start < len {
                self.cpu.bus.fill_ram(start..(start + 4096).min(len), 0);
                start += pages << 12;
            }
        }
        if self.cpu.bus.vga.dirty {
            self.cpu.bus.vga.vram_graphics.fill(0);
            self.cpu.bus.vga.vram_text.fill(0);
        }
    }
}

thread_local! {
    /// Where a panic happened, recorded by the harness's panic hook.
    pub static PANIC_LOCATION: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    let msg = if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    };
    match PANIC_LOCATION.with(|l| l.borrow_mut().take()) {
        Some(at) => format!("{msg} (at {at})"),
        None => msg,
    }
}

/// Registers the harness compares, in report order.
pub const COMPARED: [usize; 16] = [
    moo::EAX,
    moo::EBX,
    moo::ECX,
    moo::EDX,
    moo::ESI,
    moo::EDI,
    moo::EBP,
    moo::ESP,
    moo::CS,
    moo::DS,
    moo::ES,
    moo::FS,
    moo::GS,
    moo::SS,
    moo::EIP,
    moo::EFLAGS,
];

/// Bits of a register the comparison covers.
pub fn width_mask(slot: usize) -> u32 {
    match slot {
        moo::CS | moo::DS | moo::ES | moo::FS | moo::GS | moo::SS => 0xFFFF,
        moo::EFLAGS => EFLAGS_386,
        _ => u32::MAX,
    }
}

/// Mask of the defined bits of `slot`: the file-wide and per-test masks
/// and, for EFLAGS, the opcode's `f_umask` from 80386.csv.
pub fn defined_mask(slot: usize, file_mask: &Regs, test_mask: &Regs, csv_flags: u32) -> u32 {
    let mut m = width_mask(slot)
        & file_mask.get(slot).unwrap_or(u32::MAX)
        & test_mask.get(slot).unwrap_or(u32::MAX);
    if slot == moo::EFLAGS {
        m &= csv_flags;
    }
    m
}
