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
    report(&b.cpu);
}

/// Check that the second machine ran translated code when it should have,
/// and show the recompiler's counts.
fn report(cpu: &Cpu) {
    let stats = cpu.dynrec.stats();
    println!("{:?}", stats);
    if second_core() != CoreMode::Normal && rust_dos::dynrec::AVAILABLE {
        assert!(stats.runs > 0, "no translated code ran");
    }
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
        match lockstep(&mut a, &mut b, batches, len, |_, _| {}) {
            Ok(run) => println!(
                "{}: {} batches, {} instructions, equal (mode switches {}, exceptions {})\n  \
                 normal {:.2}s ({:.0} MIPS), {} {:.2}s ({:.0} MIPS)\n  {:?}",
                entry,
                run.batches,
                a.executed,
                a.mode_switches,
                a.exceptions,
                run.a_time.as_secs_f64(),
                a.executed as f64 / run.a_time.as_secs_f64() / 1e6,
                second_core().name(),
                run.b_time.as_secs_f64(),
                b.executed as f64 / run.b_time.as_secs_f64() / 1e6,
                b.dynrec.stats()
            ),
            Err(e) => {
                println!("{}: {}", entry, e);
                failures.push(entry.to_string());
            }
        }
    }
    assert!(failures.is_empty(), "diverged: {:?}", failures);
}

/// One program on one core, for timing and profiling: the first entry of
/// `DYNDIFF_PROGRAMS` on `DYNDIFF_CORE`, without comparisons.
#[test]
#[ignore]
fn local_program_alone() {
    let Ok(list) = std::env::var("DYNDIFF_PROGRAMS") else {
        println!("DYNDIFF_PROGRAMS is not set: nothing to run");
        return;
    };
    let batches: usize = std::env::var("DYNDIFF_BATCHES").map_or(2_000, |v| v.parse().unwrap());
    let len = std::env::var("DYNDIFF_BATCH_LEN").map_or(100_000, |v| v.parse().unwrap());
    fix_time();
    let entry = list.split(',').next().unwrap();
    let (dir, command) = entry.split_once(':').expect("DIR:COMMAND");
    let mut cpu = program_machine(dir, "a", command, second_core());
    let started = std::time::Instant::now();
    for _ in 0..batches {
        dyndiff::batch(&mut cpu, len, false);
    }
    let secs = started.elapsed().as_secs_f64();
    println!(
        "{} on {}: {} instructions in {:.2}s ({:.0} MIPS)\n  {:?}",
        entry,
        second_core().name(),
        cpu.executed,
        secs,
        cpu.executed as f64 / secs / 1e6,
        cpu.dynrec.stats()
    );
}

/// CPU-bound protected-mode programs on both cores, in lockstep, with their
/// speeds: a table-driven CRC-32, a bubble sort, and a mix of shifts and
/// rotates on registers.
#[test]
#[ignore]
fn compute_speed() {
    fix_time();
    let programs: [(&str, fn(&mut Rig)); 3] = [("crc32", crc32_program), ("sort", sort_program), ("bits", bits_program)];
    for (name, program) in programs {
        let mut a = Rig::new();
        let mut b = Rig::new();
        a.cpu.core = CoreMode::Normal;
        b.cpu.core = second_core();
        for rig in [&mut a, &mut b] {
            program(rig);
            rig.enter_pm();
        }
        let run = dyndiff::lockstep_with(&mut a.cpu, &mut b.cpu, 100_000, 100_000, true, |_, _| {}).unwrap();
        println!(
            "{}: {} instructions; normal {:.0} MIPS, {} {:.0} MIPS\n  {:?}",
            name,
            a.cpu.executed,
            a.cpu.executed as f64 / run.a_time.as_secs_f64() / 1e6,
            second_core().name(),
            b.cpu.executed as f64 / run.b_time.as_secs_f64() / 1e6,
            b.cpu.dynrec.stats()
        );
    }
}

/// CRC-32 of 64 KB at DATA, 64 times over, with the table at DATA + 64K.
fn crc32_program(rig: &mut Rig) {
    let table = DATA + 0x10000;
    let code = asm32(CODE, |a| {
        // The table.
        a.xor(ecx, ecx)?;
        let mut entry = a.create_label();
        a.set_label(&mut entry)?;
        a.mov(eax, ecx)?;
        a.mov(edx, 8u32)?;
        let mut bit = a.create_label();
        let mut no_xor = a.create_label();
        a.set_label(&mut bit)?;
        a.shr(eax, 1)?;
        a.jnc(no_xor)?;
        a.xor(eax, 0xEDB8_8320u32)?;
        a.set_label(&mut no_xor)?;
        a.dec(edx)?;
        a.jnz(bit)?;
        a.mov(dword_ptr(ecx * 4 + table), eax)?;
        a.inc(ecx)?;
        a.cmp(ecx, 256)?;
        a.jb(entry)?;
        // The data: a pattern.
        a.xor(ecx, ecx)?;
        let mut fill = a.create_label();
        a.set_label(&mut fill)?;
        a.mov(eax, ecx)?;
        a.imul_3(eax, eax, 1_103_515_245)?;
        a.mov(byte_ptr(ecx + DATA), al)?;
        a.inc(ecx)?;
        a.cmp(ecx, 0x10000)?;
        a.jb(fill)?;
        // 64 passes.
        a.mov(ebp, 64u32)?;
        let mut pass = a.create_label();
        a.set_label(&mut pass)?;
        a.mov(eax, 0xFFFF_FFFFu32)?;
        a.mov(esi, DATA)?;
        a.mov(ecx, 0x10000u32)?;
        let mut byte = a.create_label();
        a.set_label(&mut byte)?;
        a.movzx(edx, byte_ptr(esi))?;
        a.xor(dl, al)?;
        a.shr(eax, 8)?;
        a.xor(eax, dword_ptr(edx * 4 + table))?;
        a.inc(esi)?;
        a.dec(ecx)?;
        a.jnz(byte)?;
        a.not(eax)?;
        a.mov(dword_ptr(RESULT), eax)?;
        a.dec(ebp)?;
        a.jnz(pass)?;
        a.hlt()
    });
    rig.load(CODE, &code);
}

/// Bubble sort of 1500 dwords at DATA.
fn sort_program(rig: &mut Rig) {
    let code = asm32(CODE, |a| {
        a.xor(ecx, ecx)?;
        let mut fill = a.create_label();
        a.set_label(&mut fill)?;
        a.mov(eax, ecx)?;
        a.imul_3(eax, eax, 0x9E37_79B1u32 as i32)?;
        a.mov(dword_ptr(ecx * 4 + DATA), eax)?;
        a.inc(ecx)?;
        a.cmp(ecx, 1500)?;
        a.jb(fill)?;
        a.mov(ebx, 1499u32)?;
        let mut outer = a.create_label();
        a.set_label(&mut outer)?;
        a.xor(esi, esi)?;
        let mut inner = a.create_label();
        let mut no_swap = a.create_label();
        a.set_label(&mut inner)?;
        a.mov(eax, dword_ptr(esi * 4 + DATA))?;
        a.mov(edx, dword_ptr(esi * 4 + DATA + 4))?;
        a.cmp(eax, edx)?;
        a.jbe(no_swap)?;
        a.mov(dword_ptr(esi * 4 + DATA), edx)?;
        a.mov(dword_ptr(esi * 4 + DATA + 4), eax)?;
        a.set_label(&mut no_swap)?;
        a.inc(esi)?;
        a.cmp(esi, ebx)?;
        a.jb(inner)?;
        a.dec(ebx)?;
        a.jnz(outer)?;
        a.hlt()
    });
    rig.load(CODE, &code);
}

/// Shifts, rotates and arithmetic on registers, 3 million rounds.
fn bits_program(rig: &mut Rig) {
    let code = asm32(CODE, |a| {
        a.mov(eax, 0x1234_5678u32)?;
        a.mov(ebx, 0x9ABC_DEF0u32)?;
        a.mov(ecx, 3_000_000u32)?;
        let mut top = a.create_label();
        a.set_label(&mut top)?;
        a.rol(eax, 5)?;
        a.add(eax, ebx)?;
        a.mov(edx, eax)?;
        a.shr(edx, 3)?;
        a.xor(ebx, edx)?;
        a.lea(esi, ptr(eax + ebx * 2 + 7))?;
        a.sub(ebx, esi)?;
        a.adc(eax, 0)?;
        a.dec(ecx)?;
        a.jnz(top)?;
        a.mov(dword_ptr(RESULT), eax)?;
        a.hlt()
    });
    rig.load(CODE, &code);
}
