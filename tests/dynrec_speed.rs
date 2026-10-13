//! How fast translated code with memory operands runs: `blocks` distinct
//! blocks of loads, stores and read-modify-writes, each ending in a jump to
//! the next, run `passes` times on the recompiler. With few blocks the code
//! stays in the host's caches; with more it doesn't; with the most it
//! doesn't fit the code memory (`CODE_SIZE`) where each memory operand
//! takes a lot of host code, and is flushed and translated again on every
//! pass. Prints the first pass's time, which is mostly translation, and
//! each pass's after it:
//!
//! `cargo test --release --test dynrec_speed -- --ignored --nocapture`

mod pmrig;

use iced_x86::code_asm::*;
use pmrig::*;
use rust_dos::cpu::{CoreMode, Cpu};
use rust_dos::exec::{ExecHook, StopReason, run_batch};
use std::time::Instant;

/// Stops the execution loop before a HLT.
struct StopAtHlt;

impl ExecHook for StopAtHlt {
    fn before_exec(&mut self, _cpu: &Cpu, phys_ip: usize, ram: &[u8]) -> bool {
        ram.get(phys_ip) == Some(&0xF4)
    }

    fn per_instruction(&self) -> bool {
        false
    }
}

/// Where the blocks are.
const BODY: u32 = 0x20_0000;

/// `blocks` blocks of six memory operands and a jump, `passes` times over.
fn program(rig: &mut Rig, blocks: u32, passes: u32) {
    rig.load(CODE, &asm32(CODE, |a| {
        a.mov(esi, DATA)?;
        a.mov(edi, DATA + 0x1000)?;
        a.mov(ecx, passes)?;
        a.jmp(BODY as u64)
    }));
    let code = asm32(BODY, |a| {
        let mut top = a.create_label();
        let mut end = a.create_label();
        a.set_label(&mut top)?;
        for k in 0..blocks {
            let mut next = a.create_label();
            let o = ((k * 12) & 0x3F0) as i32;
            a.mov(eax, dword_ptr(esi + o))?;
            a.add(ebx, dword_ptr(esi + o + 4))?;
            a.mov(dword_ptr(edi + o), eax)?;
            a.add(dword_ptr(edi + o + 8), ebx)?;
            a.mov(edx, dword_ptr(esi + o + 8))?;
            a.xor(dword_ptr(edi + o + 4), edx)?;
            a.jmp(next)?;
            a.set_label(&mut next)?;
        }
        a.dec(ecx)?;
        a.jnz(top)?;
        a.jmp(end)?;
        a.set_label(&mut end)?;
        a.hlt()
    });
    rig.load(BODY, &code);
}

/// Seconds for `passes` passes, and the recompiler's stats.
fn time(blocks: u32, passes: u32) -> (f64, rust_dos::dynrec::DynStats) {
    let mut rig = Rig::new();
    rig.cpu.core = CoreMode::Dynamic;
    program(&mut rig, blocks, passes);
    rig.enter_pm();
    let start = Instant::now();
    loop {
        let end = rig.cpu.bus.clock.icount + 1_000_000;
        rig.cpu.bus.start_batch(end);
        if run_batch(&mut rig.cpu, &mut StopAtHlt, true) == StopReason::Paused {
            break;
        }
    }
    (start.elapsed().as_secs_f64(), rig.cpu.dynrec.stats())
}

/// The first pass (translating), then each pass after it.
fn run(blocks: u32, passes: u32) {
    let best = |p| (0..3).map(|_| time(blocks, p)).min_by(|x, y| x.0.total_cmp(&y.0)).unwrap();
    let (one, first) = best(1);
    let (all, s) = best(passes);
    println!(
        "blocks {:6}: first pass {:7.1} ms ({:5} B/instr), then {:8.1} ns a block, {:6} translated in {} passes, {} flushes",
        blocks,
        one * 1e3,
        first.code_bytes / first.instructions.max(1),
        (all - one) * 1e9 / (blocks as f64 * (passes - 1) as f64),
        s.blocks,
        passes,
        s.flushes,
    );
}

#[test]
#[ignore]
fn memory_operand_speed() {
    for (blocks, passes) in [(200, 20_000), (4_000, 2_000), (20_000, 400), (34_000, 100), (44_000, 40)] {
        run(blocks, passes);
    }
}
