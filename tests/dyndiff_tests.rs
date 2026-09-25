//! Lockstep runs of the interpreter against the dynamic recompiler (see
//! tests/dyndiff): both must leave identical machines after every batch.
//!
//! `local_programs_in_lockstep` runs DOS programs from the git-ignored
//! `programs/` directory, opt-in:
//!
//! ```sh
//! DYNDIFF_PROGRAMS="D1SW:DCNTSHR,STUNTS:STUNTS" DYNDIFF_BATCHES=3000 \
//!   cargo test --release --test dyndiff_tests -- --ignored --nocapture
//! ```
//!
//! Each entry is a directory under `programs/` and the command that starts
//! the program there. `DYNDIFF_CORE` picks the second machine's core
//! (default `dynamic`; `normal` checks that the comparison itself is
//! deterministic).

mod dyndiff;
mod pmrig;

use chrono::NaiveDate;
use dyndiff::lockstep;
use iced_x86::code_asm::*;
use pmrig::*;
use rust_dos::cpu::{CoreMode, Cpu};
use std::fs;
use std::path::{Path, PathBuf};

/// The time both machines see.
fn fix_time() {
    let at = NaiveDate::from_ymd_opt(1995, 4, 11).unwrap().and_hms_opt(12, 34, 56).unwrap();
    rust_dos::hosttime::fix(Some(at));
}

/// The core of the second machine.
fn second_core() -> CoreMode {
    std::env::var("DYNDIFF_CORE").ok().map_or(CoreMode::Dynamic, |v| CoreMode::parse(&v).unwrap())
}

/// A protected-mode program with a timer interrupt: IRQ 0 counts in EDI
/// while the main loop sums, stores and calls, so interrupts land all over
/// the loop.
fn timer_program(rig: &mut Rig) {
    rig.handler(0x08, 0, |a| {
        a.push(eax)?;
        a.inc(edi)?;
        a.mov(al, 0x20)?;
        a.out(0x20, al)?;
        a.pop(eax)?;
        a.iretd()
    });
    let sub = asm32(0x10800, |a| {
        a.add(dword_ptr(DATA + 4), eax)?;
        a.rol(dword_ptr(DATA + 8), 3)?;
        a.ret()
    });
    rig.load(0x10800, &sub);
    let code = asm32(CODE, |a| {
        // IRQ 0 at vector 8, unmasked, the PIT at a short period.
        a.mov(al, 0xFE)?;
        a.out(0x21, al)?;
        a.mov(al, 0x34)?;
        a.out(0x43, al)?;
        a.mov(al, 0x00)?;
        a.out(0x40, al)?;
        a.mov(al, 0x01)?;
        a.out(0x40, al)?;
        a.xor(edi, edi)?;
        a.mov(ecx, 200_000u32)?;
        a.sti()?;
        let mut top = a.create_label();
        a.set_label(&mut top)?;
        a.mov(eax, ecx)?;
        a.imul_3(eax, eax, 7)?;
        a.xor(dword_ptr(DATA), eax)?;
        a.call(0x10800u64)?;
        a.dec(ecx)?;
        a.jnz(top)?;
        a.cli()?;
        a.hlt()
    });
    rig.load(CODE, &code);
}

#[test]
fn a_protected_mode_program_with_timer_interrupts_runs_in_lockstep() {
    fix_time();
    let mut a = Rig::new();
    let mut b = Rig::new();
    b.cpu.core = second_core();
    for rig in [&mut a, &mut b] {
        timer_program(rig);
        rig.enter_pm();
    }
    lockstep(&mut a.cpu, &mut b.cpu, 400, 5_000, |_, _| {}).unwrap();
    assert!(a.cpu.edi() > 10, "IRQ 0 came {} times", a.cpu.edi());
    assert_eq!(a.cpu.ecx(), 0, "the loop ran to the end");
}

/// A copy of `programs/<dir>` for one machine, without the swap files a
/// DOS extender left there.
fn program_copy(dir: &str, machine: &str) -> PathBuf {
    let src = Path::new("programs").join(dir);
    let dest = Path::new("target/dyndiff").join(dir).join(machine);
    let _ = fs::remove_dir_all(&dest);
    copy_dir(&src, &dest);
    dest
}

fn copy_dir(src: &Path, dest: &Path) {
    fs::create_dir_all(dest).unwrap();
    for entry in fs::read_dir(src).unwrap_or_else(|e| panic!("{}: {}", src.display(), e)) {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap();
        if path.is_dir() {
            copy_dir(&path, &dest.join(name));
        } else if !name.to_string_lossy().to_ascii_uppercase().ends_with(".SWP") {
            fs::copy(&path, dest.join(name)).unwrap();
        }
    }
}

fn program_machine(dir: &str, machine: &str, command: &str, core: CoreMode) -> Cpu {
    let mut cpu = Cpu::new(program_copy(dir, machine));
    cpu.core = core;
    cpu.load_shell();
    cpu.pending_command = Some(command.to_string());
    cpu
}

#[test]
#[ignore]
fn local_programs_in_lockstep() {
    let Ok(list) = std::env::var("DYNDIFF_PROGRAMS") else {
        println!("DYNDIFF_PROGRAMS is not set: nothing to run");
        return;
    };
    let batches = std::env::var("DYNDIFF_BATCHES").map_or(2_000, |v| v.parse().unwrap());
    let len = std::env::var("DYNDIFF_BATCH_LEN").map_or(100_000, |v| v.parse().unwrap());
    fix_time();
    let mut failures = Vec::new();
    for entry in list.split(',') {
        let (dir, command) = entry.split_once(':').expect("DIR:COMMAND");
        let mut a = program_machine(dir, "a", command, CoreMode::Normal);
        let mut b = program_machine(dir, "b", command, second_core());
        let started = std::time::Instant::now();
        match lockstep(&mut a, &mut b, batches, len, |_, _| {}) {
            Ok(n) => println!(
                "{}: {} batches, {} instructions, equal ({:.1}s; mode switches {}, exceptions {})",
                entry,
                n,
                a.executed,
                started.elapsed().as_secs_f64(),
                a.mode_switches,
                a.exceptions
            ),
            Err(e) => {
                println!("{}: {}", entry, e);
                failures.push(entry.to_string());
            }
        }
    }
    assert!(failures.is_empty(), "diverged: {:?}", failures);
}
