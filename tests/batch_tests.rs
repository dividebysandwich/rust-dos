//! Batch files on a running machine: their lines run at the prompt one
//! after the other, with their parameters and the environment's
//! variables put in, programs they start run before the next line, and
//! chaining, CALL, labels and Ctrl+C behave as in DOS.

use rust_dos::cpu::Cpu;
use rust_dos::exec::{self, NoHook, StopReason};
use std::fs;
use std::path::{Path, PathBuf};

fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let base = PathBuf::from("target/test_batch").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

/// A .COM program that runs for a little while and exits with `code`.
fn exits_with(code: u8) -> Vec<u8> {
    #[rustfmt::skip]
    let program = vec![
        0xB9, 0x00, 0x40,       // MOV CX, 4000h
        0xE2, 0xFE,             // LOOP $
        0xB8, code, 0x4C,       // MOV AX, 4Cxxh
        0xCD, 0x21,             // INT 21h
    ];
    program
}

fn machine(dir: &Path) -> Cpu {
    let mut cpu = Cpu::new(dir.to_path_buf());
    cpu.bus.set_cycles_per_ms(1000);
    cpu.load_shell();
    cpu
}

/// Run the machine for up to `ms` ms of emulated time, until `stop`.
fn run_until(cpu: &mut Cpu, ms: u64, stop: impl Fn(&Cpu) -> bool) -> bool {
    for _ in 0..ms {
        if stop(cpu) {
            return true;
        }
        let end = cpu.bus.clock.icount + 1000;
        cpu.bus.start_batch(end);
        exec::run_batch(cpu, &mut NoHook, false);
    }
    stop(cpu)
}

/// Run until the batch files have ended and the prompt is back.
fn run_batch_files(cpu: &mut Cpu) {
    assert!(
        run_until(cpu, 10_000, |cpu| !cpu.batch.is_active() && cpu.pending_command.is_none() && cpu.shell_idle()),
        "the batch files end"
    );
}

/// The text screen's rows, without their trailing blanks.
fn rows(cpu: &Cpu) -> Vec<String> {
    cpu.bus
        .vga
        .vram_text
        .chunks(160)
        .take(25)
        .map(|row| row.iter().step_by(2).map(|&b| if b == 0 { ' ' } else { b as char }).collect::<String>().trim_end().to_string())
        .collect()
}

fn screen(cpu: &Cpu) -> String {
    rows(cpu).join("\n").trim_end().to_string()
}

#[test]
fn parameters_and_variables_are_put_in() {
    let dir = scratch("params", &[("GO.BAT", b"@echo off\r\necho %0 [%1] [%2] [%3]\r\necho %GAME%!\r\n")]);
    let mut cpu = machine(&dir);
    cpu.queue_batch_lines(["@SET GAME=keen", "@GO one two"]);
    run_batch_files(&mut cpu);
    let screen = screen(&cpu);
    assert!(screen.contains("GO [one] [two] []\nkeen!"), "{}", screen);
    assert!(cpu.batch.echo, "ECHO is on again after the batch file");
}

#[test]
fn lines_are_echoed_at_the_prompt_unless_echo_is_off() {
    let dir = scratch("echo", &[("X.BAT", b"echo one\r\n@echo two\r\necho off\r\necho three\r\n")]);
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("X".into());
    run_batch_files(&mut cpu);
    let screen = screen(&cpu);
    assert!(screen.contains("C:\\>echo one\none\ntwo\nC:\\>echo off\nthree"), "{}", screen);
}

#[test]
fn a_program_runs_before_the_next_line() {
    let dir = scratch("program", &[("RUN.BAT", b"@echo off\r\nEXIT3\r\necho after\r\n"), ("EXIT3.COM", &exits_with(3))]);
    let mut cpu = machine(&dir);
    cpu.queue_batch_lines(["RUN"]);
    assert!(run_until(&mut cpu, 1000, |cpu| !cpu.shell_idle()), "the program starts");
    assert!(cpu.batch.is_active(), "the rest of the batch file waits for it");
    run_batch_files(&mut cpu);
    assert!(screen(&cpu).contains("after"), "{}", screen(&cpu));
}

#[test]
fn autoexec_bat_runs_after_autoexec_lines_that_chain_to_a_batch_file() {
    let dir = scratch(
        "chain",
        &[("X.BAT", b"@echo in x\r\n"), ("AUTOEXEC.BAT", b"@echo in autoexec\r\n")],
    );
    let mut cpu = machine(&dir);
    cpu.queue_batch_lines(["@X", "@echo never"]);
    assert!(cpu.queue_batch_file("C:\\AUTOEXEC.BAT"));
    run_batch_files(&mut cpu);
    let screen = screen(&cpu);
    assert!(screen.contains("in x\nin autoexec"), "{}", screen);
    assert!(!screen.contains("never"), "chaining never comes back: {}", screen);
}

#[test]
fn code_page_437_characters_are_echoed_as_they_are() {
    let dir = scratch("cp437", &[("MENU.BAT", b"@echo \xC9\xCD\xBB \x84\r\n")]);
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("MENU".into());
    run_batch_files(&mut cpu);
    let text: Vec<u8> = cpu.bus.vga.vram_text.iter().step_by(2).copied().collect();
    assert!(text.windows(5).any(|w| w == b"\xC9\xCD\xBB \x84"));
}

#[test]
fn a_batch_file_that_never_ends_gives_the_machine_back_and_ctrl_c_ends_it() {
    // It starts itself again and again, and never runs a program.
    let dir = scratch("forever", &[("LOOP.BAT", b"@LOOP\r\n")]);
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("LOOP".into());
    let end = cpu.bus.clock.icount + 100_000;
    cpu.bus.start_batch(end);
    assert_eq!(exec::run_batch(&mut cpu, &mut NoHook, false), StopReason::BatchEnd);
    assert!(cpu.batch.is_active());

    cpu.bus.keyboard_buffer.push_back(0x2E03); // Ctrl+C
    run_batch_files(&mut cpu);
    assert!(screen(&cpu).contains("^C"));
}

#[test]
fn a_typed_batch_file_gets_its_arguments() {
    let dir = scratch("typed", &[("GO.BAT", b"@echo [%1]\r\n")]);
    let mut cpu = machine(&dir);
    cpu.pending_command = Some("go.bat hello".into());
    run_batch_files(&mut cpu);
    assert!(screen(&cpu).contains("[hello]"), "{}", screen(&cpu));
}
