//! Guest code on the two engines: a DOS program's loop of arithmetic,
//! memory accesses, calls and a port write now and then, run for a slice
//! of emulated time on the interpreter and on the dynamic recompiler.
//!
//! The profiled variants run the same with a profile started
//! (`rust_dos::profile`), which shows what profiling costs, and the plain
//! ones compared with a baseline from before a change show what the
//! profiler's hooks cost while it is off.

mod common;

use common::*;
use criterion::{Criterion, criterion_group, criterion_main};
use iced_x86::code_asm::*;
use rust_dos::cpu::{CoreMode, Cpu};
use rust_dos::exec::{self, NoHook, StopReason};
use std::hint::black_box;

/// Instructions a slice runs: a millisecond at this speed.
const SLICE: u64 = 200_000;

/// A machine running the loop on `core`, past its start-up.
fn running(name: &str, core: CoreMode) -> Cpu {
    let dir = scratch(name);
    std::fs::write(dir.join("T.COM"), loop_program()).unwrap();
    let mut cpu = machine(&dir);
    cpu.core = core;
    cpu.bus.set_cycles_per_ms(SLICE as u32);
    cpu.load_shell();
    cpu.pending_command = Some("T".to_string());
    for _ in 0..50 {
        slice(&mut cpu);
    }
    assert_eq!(cpu.dynamic_active(), core == CoreMode::Dynamic, "the core");
    cpu
}

/// One slice of the program.
fn slice(cpu: &mut Cpu) {
    let end = cpu.bus.clock.icount + SLICE;
    cpu.bus.start_batch(end);
    while cpu.bus.clock.icount < end {
        match exec::run_batch(cpu, &mut NoHook, false) {
            StopReason::BatchEnd => break,
            StopReason::Exit => panic!("the program ended"),
            _ => cpu.bus.start_batch(end),
        }
    }
    black_box(cpu.ax());
}

/// The program, as a .COM file: forever, sum and scramble a 256-word
/// table through a near call, writing port 80h after each pass.
fn loop_program() -> Vec<u8> {
    let assemble = || -> Result<Vec<u8>, IcedError> {
        let mut a = CodeAssembler::new(16)?;
        let mut pass = a.create_label();
        let mut word = a.create_label();
        let mut step = a.create_label();
        a.xor(dx, dx)?;
        a.set_label(&mut pass)?;
        a.mov(si, 0x1000)?;
        a.mov(cx, 256)?;
        a.set_label(&mut word)?;
        a.call(step)?;
        a.add(si, 2)?;
        a.loop_(word)?;
        a.mov(al, dl)?;
        a.out(0x80, al)?;
        a.jmp(pass)?;
        // One word: add it in, then scramble it.
        a.set_label(&mut step)?;
        a.mov(ax, word_ptr(si))?;
        a.add(dx, ax)?;
        a.rol(ax, 3)?;
        a.xor(ax, dx)?;
        a.mov(word_ptr(si), ax)?;
        a.ret()?;
        a.assemble(0x100)
    };
    assemble().unwrap()
}

fn engines(c: &mut Criterion) {
    for (name, core) in [("interpreter", CoreMode::Normal), ("recompiler", CoreMode::Dynamic)] {
        let mut cpu = running(&format!("cpu_{}", name), core);
        c.bench_function(&format!("cpu/{}", name), |b| b.iter(|| slice(&mut cpu)));
    }
}

fn profiled(c: &mut Criterion) {
    use rust_dos::profile::session::{Options, Session};
    for (name, core) in [("interpreter", CoreMode::Normal), ("recompiler", CoreMode::Dynamic)] {
        let mut cpu = running(&format!("cpu_profiled_{}", name), core);
        let mut session = Session::start(&mut cpu, Options::default()).unwrap();
        c.bench_function(&format!("cpu/{}/profiled", name), |b| b.iter(|| slice(&mut cpu)));
        session.stop(&mut cpu);
    }
}

criterion_group!(benches, engines, profiled);
criterion_main!(benches);
