//! Rebooting the built-in DOS: a CPU reset (the keyboard controller's
//! reset line, INT 19h, a jump to the reset vector) starts the session
//! over, with the resident programs gone and the startup run again.

use rust_dos::boot::{Startup, StartupItem};
use rust_dos::cpu::Cpu;
use rust_dos::exec::{self, NoHook};
use std::fs;
use std::path::{Path, PathBuf};

fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let base = PathBuf::from("target/test_reboot").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

/// A TSR keeping 100h paragraphs.
const TSR: &[u8] = &[0xB8, 0x00, 0x31, 0xBA, 0x00, 0x01, 0xCD, 0x21];
/// irtygo/dos-reboot's way: OUT 64h, FEh (then exit, if that fails).
const KBC_RESET: &[u8] = &[0xB0, 0xFE, 0xE6, 0x64, 0xCD, 0x20];
/// INT 19h.
const INT19: &[u8] = &[0xCD, 0x19, 0xCD, 0x20];
/// JMP FFFF:0000.
const JMP_RESET: &[u8] = &[0xEA, 0x00, 0x00, 0xFF, 0xFF];

fn machine(dir: &Path) -> Cpu {
    let mut cpu = Cpu::new(dir.to_path_buf());
    cpu.bus.set_cycles_per_ms(1000);
    cpu.startup = Startup {
        notes: vec![("Note: ".to_string(), "startup".to_string())],
        commands: vec![
            StartupItem::Lines(vec!["TSR".to_string()]),
            StartupItem::BatchFile("C:\\AUTOEXEC.BAT".to_string()),
        ],
    };
    cpu.start_dos();
    cpu
}

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

fn settle(cpu: &mut Cpu) {
    assert!(
        run_until(cpu, 10_000, |cpu| !cpu.batch.is_active() && cpu.pending_command.is_none() && cpu.shell_idle()),
        "the startup ends"
    );
}

fn screen(cpu: &Cpu) -> String {
    cpu.bus
        .vga
        .vram_text
        .chunks(160)
        .take(25)
        .map(|row| row.iter().step_by(2).map(|&b| if b == 0 { ' ' } else { b as char }).collect::<String>().trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn reboots_with(name: &str, program: &[u8]) {
    let dir = scratch(
        name,
        &[("TSR.COM", TSR), ("RESET.COM", program), ("AUTOEXEC.BAT", b"@echo off\r\nset BOOTED=%BOOTED%x\r\necho booted %BOOTED%\r\n")],
    );
    let mut cpu = machine(&dir);
    settle(&mut cpu);
    let resident = cpu.resident_end;
    assert!(screen(&cpu).contains("booted x"), "{}", screen(&cpu));

    cpu.queue_batch_lines(["CD \\", "MD SUB", "CD SUB", "RESET"]);
    settle(&mut cpu);
    let text = screen(&cpu);
    assert_eq!(text.matches("Rust-DOS v").count(), 1, "the banner, on a cleared screen: {}", text);
    assert!(text.contains("startup"), "{}", text);
    // AUTOEXEC.BAT ran again (the environment stays the session's).
    assert!(text.contains("booted xx"), "{}", text);
    assert!(!text.contains("booted x\n"), "the screen before is gone: {}", text);
    // The TSR loaded once more, where it was: not on top of itself.
    assert_eq!(cpu.resident_end, resident, "resident programs start over");
    assert_eq!(cpu.bus.disk.get_current_directory_of(2).unwrap_or_default(), "", "C:\\ again");
}

#[test]
fn the_keyboard_controller_reboots_dos() {
    reboots_with("kbc", KBC_RESET);
}

#[test]
fn int19_reboots_dos() {
    reboots_with("int19", INT19);
}

#[test]
fn the_reset_vector_reboots_dos() {
    reboots_with("vector", JMP_RESET);
}

#[test]
fn port_cf9_reboots_dos() {
    // MOV DX, 0CF9h ; MOV AL, 06h ; OUT DX, AL ; INT 20h
    reboots_with("cf9", &[0xBA, 0xF9, 0x0C, 0xB0, 0x06, 0xEE, 0xCD, 0x20]);
}

#[test]
fn ctrl_alt_del_reboots_dos() {
    let dir = scratch("ctrlaltdel", &[("TSR.COM", TSR), ("AUTOEXEC.BAT", b"@echo off\r\nset BOOTED=%BOOTED%x\r\necho booted %BOOTED%\r\n")]);
    let mut cpu = machine(&dir);
    settle(&mut cpu);
    use rust_dos::keyboard::key_event;
    key_event(&mut cpu.bus, 0x1D, false, true, None);
    key_event(&mut cpu.bus, 0x38, false, true, None);
    key_event(&mut cpu.bus, 0x53, true, true, None);
    assert!(run_until(&mut cpu, 1000, |cpu| screen(cpu).contains("booted xx")), "{}", screen(&cpu));
}
