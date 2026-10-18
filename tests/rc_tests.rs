//! RC, Rat Commander, in the shell: its screen, the command lines and
//! programs it runs and comes back from, F4's EDIT, copying on a host
//! folder, and save states taken while it waits and while what it ran
//! runs.

use rust_dos::cpu::Cpu;
use rust_dos::exec::{self, NoHook};
use rust_dos::savestate::machine;
use rust_dos::shell::ShellWait;
use std::fs;
use std::path::{Path, PathBuf};

fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let base = PathBuf::from("target/test_rc").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        let path = base.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    base
}

fn machine(dir: &Path) -> Cpu {
    let mut cpu = Cpu::new(dir.to_path_buf());
    cpu.bus.set_cycles_per_ms(1000);
    cpu.load_shell();
    cpu
}

fn run(cpu: &mut Cpu, ms: u64) {
    for _ in 0..ms {
        let end = cpu.bus.clock.icount + 1000;
        cpu.bus.start_batch(end);
        exec::run_batch(cpu, &mut NoHook, false);
    }
}

fn in_rc(cpu: &Cpu) -> bool {
    matches!(cpu.shell_wait, Some(ShellWait::Rc(_)))
}

fn editing(cpu: &Cpu) -> bool {
    matches!(cpu.shell_wait, Some(ShellWait::Edit(_)))
}

/// Wait up to `ms` for RC to wait for keys.
fn wait_rc(cpu: &mut Cpu, ms: u64) {
    for _ in 0..ms / 5 {
        run(cpu, 5);
        if in_rc(cpu) {
            return;
        }
    }
    panic!("RC waits:\n{}", screen(cpu));
}

fn rc(cpu: &mut Cpu) {
    cpu.queue_batch_lines(["RC"]);
    wait_rc(cpu, 1000);
}

/// Keystrokes (scan code << 8 | character), each given time.
fn keys(cpu: &mut Cpu, keys: &[u16]) {
    for &key in keys {
        cpu.bus.keyboard_buffer.push_back(key);
        run(cpu, 30);
    }
}

fn text(cpu: &mut Cpu, text: &str) {
    let k: Vec<u16> = text.bytes().map(|b| b as u16).collect();
    keys(cpu, &k);
}

const ENTER: u16 = 0x1C0D;
const END: u16 = 0x4FE0;
const HOME: u16 = 0x47E0;
const F4: u16 = 0x3E00;
const F5: u16 = 0x3F00;
const F10: u16 = 0x4400;
const ALT_F: u16 = 0x2100;
const CTRL_O: u16 = 0x180F;
const CTRL_Y: u16 = 0x1519;

fn rows(cpu: &Cpu) -> Vec<String> {
    cpu.bus
        .vga
        .vram_text
        .chunks(160)
        .take(25)
        .map(|row| row.iter().step_by(2).map(|&b| cp437(b)).collect::<String>())
        .collect()
}

/// The box-drawing characters RC uses, and '#' for the rest above 7Fh.
fn cp437(b: u8) -> char {
    match b {
        0 => ' ',
        0xC9 => '╔',
        0xBB => '╗',
        0xBA => '║',
        0xD1 => '╤',
        0xB3 => '│',
        0xCD => '═',
        b if b < 0x80 => b as char,
        _ => '#',
    }
}

fn screen(cpu: &Cpu) -> String {
    rows(cpu).join("\n")
}

fn attr(cpu: &Cpu, row: usize, col: usize) -> u8 {
    cpu.bus.vga.vram_text[row * 160 + col * 2 + 1]
}

#[test]
fn shows_two_panels_and_quits_back_to_the_prompt() {
    let dir = scratch("quit", &[("GAMES/X.EXE", b"MZ"), ("README.TXT", b"hi")]);
    let mut cpu = machine(&dir);
    run(&mut cpu, 50);
    rc(&mut cpu);
    let r = rows(&cpu);
    assert!(
        r[0].starts_with("╔════════════╤═══ C:\\ ════╤════════════╗╔"),
        "{}",
        screen(&cpu)
    );
    assert!(r[2].starts_with("║GAMES       │"), "{}", screen(&cpu));
    assert!(
        r[24].starts_with("1Help   2Menu   3View"),
        "{}",
        screen(&cpu)
    );
    // NC's colours: frame cyan on blue, the cursor black on cyan, the key
    // bar's digits grey on black.
    assert_eq!(attr(&cpu, 0, 0), 0x1B);
    assert_eq!(attr(&cpu, 2, 1), 0x30);
    assert_eq!((attr(&cpu, 24, 0), attr(&cpu, 24, 1)), (0x07, 0x30));
    keys(&mut cpu, &[F10, b'y' as u16]);
    run(&mut cpu, 50);
    assert!(!in_rc(&cpu));
    assert!(!screen(&cpu).contains("1Help"), "{}", screen(&cpu));
    assert!(screen(&cpu).contains("C:\\>"), "{}", screen(&cpu));
}

#[test]
fn runs_a_command_line_and_comes_back() {
    let dir = scratch("cmdline", &[]);
    let mut cpu = machine(&dir);
    run(&mut cpu, 50);
    rc(&mut cpu);
    text(&mut cpu, "echo from rc");
    keys(&mut cpu, &[ENTER]);
    wait_rc(&mut cpu, 1000);
    assert!(
        cpu.shell_history
            .entries()
            .iter()
            .any(|l| l == "echo from rc")
    );
    // Ctrl+O: the DOS screen, with what the command printed.
    keys(&mut cpu, &[CTRL_O]);
    assert!(screen(&cpu).contains("from rc"), "{}", screen(&cpu));
    assert!(!screen(&cpu).contains("╔═"), "{}", screen(&cpu));
    keys(&mut cpu, &[CTRL_O]);
    assert!(screen(&cpu).contains("╔═"), "{}", screen(&cpu));
}

#[test]
fn enter_runs_a_program_and_rc_comes_back_after_it() {
    // MOV AH,9; MOV DX,109h; INT 21h; RET, and the text.
    let com = b"\xb4\x09\xba\x09\x01\xcd\x21\xc3\x90Hello from COM$";
    let dir = scratch("program", &[("HELLO.COM", com)]);
    let mut cpu = machine(&dir);
    run(&mut cpu, 50);
    rc(&mut cpu);
    keys(&mut cpu, &[HOME, ENTER]);
    wait_rc(&mut cpu, 2000);
    keys(&mut cpu, &[CTRL_O]);
    assert!(screen(&cpu).contains("C:\\>hello.com"), "{}", screen(&cpu));
    assert!(screen(&cpu).contains("Hello from COM"), "{}", screen(&cpu));
}

#[test]
fn a_batch_file_runs_to_its_end_before_rc_comes_back() {
    let dir = scratch(
        "batch",
        &[("GO.BAT", b"@echo off\r\necho one\r\necho two\r\n")],
    );
    let mut cpu = machine(&dir);
    run(&mut cpu, 50);
    rc(&mut cpu);
    keys(&mut cpu, &[HOME, ENTER]);
    wait_rc(&mut cpu, 2000);
    keys(&mut cpu, &[CTRL_O]);
    let s = screen(&cpu);
    assert!(s.contains("one") && s.contains("two"), "{}", s);
}

#[test]
fn f4_edits_the_file_and_rc_comes_back() {
    let dir = scratch("edit", &[("A.TXT", b"text")]);
    let mut cpu = machine(&dir);
    run(&mut cpu, 50);
    rc(&mut cpu);
    keys(&mut cpu, &[HOME, F4]);
    run(&mut cpu, 100);
    assert!(editing(&cpu), "{}", screen(&cpu));
    assert!(screen(&cpu).contains("A.TXT"), "{}", screen(&cpu));
    text(&mut cpu, "more ");
    keys(&mut cpu, &[ALT_F, b's' as u16, ALT_F, b'x' as u16]);
    wait_rc(&mut cpu, 1000);
    assert_eq!(fs::read(dir.join("A.TXT")).unwrap(), b"more text");
}

#[test]
fn f5_copies_into_a_host_folder() {
    let dir = scratch("copy", &[("A.TXT", b"copied"), ("SUB/KEEP", b"")]);
    let mut cpu = machine(&dir);
    run(&mut cpu, 50);
    rc(&mut cpu);
    // SUB, then A.TXT.
    keys(&mut cpu, &[END, F5, CTRL_Y]);
    text(&mut cpu, "sub");
    keys(&mut cpu, &[ENTER]);
    run(&mut cpu, 100);
    assert_eq!(fs::read(dir.join("SUB/A.TXT")).unwrap(), b"copied");
    assert!(in_rc(&cpu));
}

#[test]
fn rc_steps_aside_for_batch_lines_and_comes_back() {
    let dir = scratch("aside", &[]);
    let mut cpu = machine(&dir);
    run(&mut cpu, 50);
    rc(&mut cpu);
    // A game launched from the settings window, say.
    cpu.queue_batch_lines(["echo launched"]);
    run(&mut cpu, 100);
    wait_rc(&mut cpu, 1000);
    keys(&mut cpu, &[CTRL_O]);
    assert!(screen(&cpu).contains("launched"), "{}", screen(&cpu));
}

#[test]
fn lines_after_rc_in_a_batch_file_wait_for_it() {
    let dir = scratch("after", &[]);
    let mut cpu = machine(&dir);
    run(&mut cpu, 50);
    cpu.queue_batch_lines(["RC", "echo after rc"]);
    wait_rc(&mut cpu, 1000);
    run(&mut cpu, 100);
    assert!(in_rc(&cpu));
    keys(&mut cpu, &[F10, b'y' as u16]);
    run(&mut cpu, 100);
    assert!(screen(&cpu).contains("after rc"), "{}", screen(&cpu));
}

#[test]
fn a_save_state_keeps_rc_waiting_and_to_come_back() {
    let dir = scratch("state", &[("A.TXT", b"x")]);
    let mut cpu = machine(&dir);
    run(&mut cpu, 50);
    rc(&mut cpu);
    let state = machine::save(&cpu);
    keys(&mut cpu, &[F10, b'y' as u16]);
    assert!(!in_rc(&cpu));
    machine::load(&mut cpu, &state).unwrap();
    run(&mut cpu, 100);
    assert!(in_rc(&cpu));
    // While EDIT, which RC ran, runs: RC comes back after it.
    keys(&mut cpu, &[HOME, F4]);
    run(&mut cpu, 100);
    assert!(editing(&cpu));
    let state = machine::save(&cpu);
    let mut other = machine(&dir);
    machine::load(&mut other, &state).unwrap();
    run(&mut other, 100);
    keys(&mut other, &[ALT_F, b'x' as u16]);
    wait_rc(&mut other, 1000);
    assert!(screen(&other).contains("1Help"), "{}", screen(&other));
}
