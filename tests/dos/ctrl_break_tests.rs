//! Ctrl+C in DOS calls: DOS looks for it in the character I/O functions
//! always, and in the others while BREAK is on, and runs the program's
//! Ctrl+C handler (INT 23h), as MS-DOS does.

use rust_dos::cpu::{Cpu, CpuState};
use std::fs;
use std::path::PathBuf;

/// Where the program counts its calls and its handler's runs.
const CALLS: u16 = 0x0122;
const BREAKS: u16 = 0x0123;

/// A program that sets its own INT 23h handler (`own_handler`), sets
/// BREAK to `break_on` (AX=3301h), and then calls INT 21h function `ah`
/// in a loop, counting the calls at `CALLS`. Its handler counts its runs
/// at `BREAKS` and returns with an IRET.
#[rustfmt::skip]
fn program(own_handler: bool, break_on: bool, ah: u8) -> Vec<u8> {
    let mut code = if own_handler {
        vec![
            0xB8, 0x23, 0x25, // MOV AX, 2523h
            0xBA, 0x1C, 0x01, // MOV DX, handler
            0xCD, 0x21, // INT 21h
        ]
    } else {
        vec![0x90; 8]
    };
    code.extend([
        0xB8, 0x01, 0x33, // MOV AX, 3301h
        0xB2, break_on as u8, // MOV DL, on
        0xCD, 0x21, // INT 21h
        0xB4, ah, // loop: MOV AH, ah
        0xB2, 0xFF, // MOV DL, FFh (input, for AH=06h)
        0xCD, 0x21, // INT 21h
        0x2E, 0xFE, 0x06, 0x22, 0x01, // INC BYTE CS:[CALLS]
        0xEB, 0xF3, // JMP loop
        0x2E, 0xFE, 0x06, 0x23, 0x01, // handler: INC BYTE CS:[BREAKS]
        0xCF, // IRET
        0x00, 0x00, // CALLS, BREAKS
    ]);
    assert_eq!(code.len(), 0x24);
    code
}

/// Run `program` until it has called its function a few times, type
/// Ctrl+C, and run on.
fn run(name: &str, code: &[u8]) -> Box<Cpu> {
    let base = PathBuf::from("target/test_ctrl_break").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    fs::write(base.join("BRK.COM"), code).unwrap();
    let mut cpu = Cpu::new(base);
    cpu.load_shell();
    assert!(cpu.load_executable("BRK.COM", None));
    let psp = cpu.current_psp;
    let calls = |cpu: &Cpu| cpu.bus.read_8(cpu.get_physical_addr(psp, CALLS));
    while calls(&cpu) < 3 {
        cpu.step();
    }
    cpu.bus.keyboard_buffer.push_back(0x2E03); // Ctrl+C
    for _ in 0..2000 {
        if cpu.state != CpuState::Running {
            break;
        }
        cpu.step();
    }
    cpu
}

fn count(cpu: &Cpu, at: u16) -> u8 {
    cpu.bus.read_8(cpu.get_physical_addr(cpu.current_psp, at))
}

fn ctrl_c_waits(cpu: &mut Cpu) -> bool {
    rust_dos::keyboard::peek_keystroke(&mut cpu.bus).is_some()
}

/// The text screen's first rows, as text.
fn screen(cpu: &Cpu) -> String {
    (0..80 * 5)
        .map(|i| cpu.bus.read_8(0xB8000 + 2 * i) as char)
        .collect()
}

#[test]
fn break_on_runs_the_handler_in_a_file_call_and_the_call_again() {
    let mut cpu = run("on", &program(true, true, 0x19));
    assert_eq!(count(&cpu, BREAKS), 1);
    assert!(!ctrl_c_waits(&mut cpu));
    assert!(screen(&cpu).contains("^C"));
    // The handler's IRET takes the program back into the call it made,
    // and the loop goes on.
    assert!(count(&cpu, CALLS) > 10);
    assert_eq!(cpu.state, CpuState::Running);
}

#[test]
fn break_off_leaves_ctrl_c_for_the_console_in_a_file_call() {
    let mut cpu = run("off", &program(true, false, 0x19));
    assert_eq!(count(&cpu, BREAKS), 0);
    assert!(ctrl_c_waits(&mut cpu));
}

#[test]
fn console_status_checks_for_ctrl_c_with_break_off() {
    let mut cpu = run("status", &program(true, false, 0x0B));
    assert_eq!(count(&cpu, BREAKS), 1);
    assert!(!ctrl_c_waits(&mut cpu));
}

#[test]
fn direct_console_io_reads_ctrl_c_as_a_character() {
    let mut cpu = run("direct", &program(true, true, 0x06));
    assert_eq!(count(&cpu, BREAKS), 0);
    assert!(!ctrl_c_waits(&mut cpu));
}

/// COMMAND.COM's handler, which a program that sets none has, ends the
/// program, as Ctrl+C (termination type 1).
#[test]
fn the_default_handler_ends_the_program() {
    let cpu = run("default", &program(false, true, 0x19));
    assert_eq!(cpu.state, CpuState::RebootShell);
    assert_eq!(cpu.last_child_exit, 0x0100);
}
