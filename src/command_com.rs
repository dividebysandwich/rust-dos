//! COMMAND.COM as programs start it: Z:\COMMAND.COM, the COMSPEC, which
//! games run to shell out to DOS ("COMMAND /C command", or a prompt until
//! EXIT) and installers to run their batch files.
//!
//! It is a small program (`stub_code`) that asks the emulator what to do
//! next (SERVICE_COMMAND) and does it: EXEC a program, wait for a key,
//! read a line with INT 21h AH=0Ah, or exit with an exit code. The
//! emulator runs the command lines with the top-level shell's built-in
//! commands and batch engine, on the secondary shell's own batch files,
//! ECHO, ERRORLEVEL and what it waits for, swapped in for the line.
//!
//! Its memory: the PSP, the code from 0100h, the path of the program to
//! EXEC at 0200h, its command tail at 0280h, EXEC's parameter block at
//! 0300h with the two FCBs at 0310h and 0320h, the line buffer for AH=0Ah
//! at 0340h, and the stack up to 0500h. The block is cut to that.

use crate::asm16::Asm;
use crate::batch::Batch;
use crate::cpu::{Cpu, CpuFlags};
use crate::shell::ShellWait;
use crate::video::print_string;

const PATH: u16 = 0x0200;
const TAIL: u16 = 0x0280;
const PARAMS: u16 = 0x0300;
const FCB1: u16 = 0x0310;
const FCB2: u16 = 0x0320;
const LINE: u16 = 0x0340;
const STACK: u16 = 0x0500;
/// Paragraphs the program keeps.
const PARAGRAPHS: u16 = STACK / 16;

/// What the code does next, in AL after SERVICE_COMMAND.
const EXIT: u8 = 0;
const EXEC: u8 = 1;
const KEY: u8 = 2;
const READ_LINE: u8 = 3;
const AGAIN: u8 = 4;

/// The program: loop asking the emulator, and do what it says.
pub fn stub_code() -> Vec<u8> {
    let mut a = Asm::new(0x0100);
    a.op(&[0xBC, STACK as u8, (STACK >> 8) as u8]); // MOV SP, STACK
    a.label("LOOP");
    a.op(&[0xFE, 0x39, crate::bios::SERVICE_COMMAND]);
    a.op(&[0x3C, EXEC]); // CMP AL, EXEC
    a.jump(0x74, "EXEC"); // JE
    a.op(&[0x3C, KEY]);
    a.jump(0x74, "KEY");
    a.op(&[0x3C, READ_LINE]);
    a.jump(0x74, "LINE");
    a.op(&[0x3C, EXIT]);
    a.jump(0x75, "LOOP"); // JNE: AGAIN
    a.op(&[0x88, 0xD8, 0xB4, 0x4C, 0xCD, 0x21]); // MOV AL, BL; MOV AH, 4Ch; INT 21h

    a.label("EXEC");
    a.op(&[0x0E, 0x07]); // PUSH CS; POP ES
    a.op(&[0xBB, PARAMS as u8, (PARAMS >> 8) as u8]); // MOV BX, PARAMS
    a.op(&[0xBA, PATH as u8, (PATH >> 8) as u8]); // MOV DX, PATH
    a.op(&[0xB8, 0x00, 0x4B, 0xCD, 0x21]); // MOV AX, 4B00h; INT 21h
    a.jump(0xEB, "LOOP");

    // A key, or AX=0 after a tick without one.
    a.label("KEY");
    a.op(&[0xB4, 0x11, 0xCD, 0x16]); // MOV AH, 11h; INT 16h
    a.jump(0x74, "NO_KEY"); // JZ
    a.op(&[0xB4, 0x10, 0xCD, 0x16]); // MOV AH, 10h; INT 16h
    a.jump(0xEB, "LOOP");
    a.label("NO_KEY");
    a.op(&[0xF4, 0x31, 0xC0]); // HLT; XOR AX, AX
    a.jump(0xEB, "LOOP");

    a.label("LINE");
    a.op(&[0xBA, LINE as u8, (LINE >> 8) as u8]); // MOV DX, LINE
    a.op(&[0xB4, 0x0A, 0xCD, 0x21]); // MOV AH, 0Ah; INT 21h
    a.jump(0xEB, "LOOP");
    a.finish()
}

/// What the program was told to do last, whose result the next
/// SERVICE_COMMAND takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asked {
    Nothing,
    Exec,
    Key,
    Line,
}

/// A COMMAND.COM that runs, by the PSP of its program and where that is
/// in physical memory (`Cpu::psp_address`): the DOS machines of Windows'
/// 386 enhanced mode each run one at the same segment.
#[derive(Clone, Debug)]
pub struct SecondaryShell {
    psp: u16,
    at: u32,
    batch: Batch,
    errorlevel: u8,
    wait: Option<ShellWait>,
    /// /C: it exits once its command (and the batch files it starts) have
    /// run.
    once: bool,
    /// The /C or /K command, until it runs.
    command: Option<String>,
    asked: Asked,
}

/// What the command line running in a secondary shell asked for, which the
/// shell's program does.
#[derive(Clone, Debug, Default)]
pub struct Dispatch {
    /// EXIT.
    pub exit: bool,
    /// A program to EXEC and its parameters.
    pub exec: Option<(String, String)>,
}

impl SecondaryShell {
    /// A shell started with the command tail `tail`: /C command runs the
    /// command and exits, /K command runs it and stays; other switches
    /// (/P, /E:size) and a path before them are taken and ignored.
    fn new(psp: u16, tail: &str) -> Self {
        let mut shell = Self {
            psp,
            at: psp as u32 * 16,
            batch: Batch::default(),
            errorlevel: 0,
            wait: None,
            once: false,
            command: None,
            asked: Asked::Nothing,
        };
        let mut rest = tail.trim_start();
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('/') {
                match after.chars().next().map(|c| c.to_ascii_uppercase()) {
                    Some('C') | Some('K') => {
                        shell.once = after.starts_with(['C', 'c']);
                        shell.command = Some(after[1..].trim().to_string());
                        break;
                    }
                    _ => {}
                }
            }
            rest = rest.split_once([' ', '\t']).map_or("", |(_, r)| r.trim_start());
        }
        shell
    }

    /// Put its state in the machine's place for a command line, or back.
    fn swap(&mut self, cpu: &mut Cpu) {
        std::mem::swap(&mut self.batch, &mut cpu.batch);
        std::mem::swap(&mut self.errorlevel, &mut cpu.errorlevel);
        std::mem::swap(&mut self.wait, &mut cpu.shell_wait);
    }
}

/// SERVICE_COMMAND: take the result of what the program did last and tell
/// it what to do next, in AL (and the exit code in BL).
pub fn service(cpu: &mut Cpu) {
    // The running process is the machine's own under Windows; the screen
    // the command line writes is there before it runs, as it runs again
    // from the start after a fault.
    crate::dos_data::load_process_state(cpu);
    if !crate::video::console_ready(cpu) {
        return;
    }
    let psp = cpu.current_psp;
    let at = cpu.psp_address(psp);
    let found = cpu.secondary_shells.iter().rposition(|s| s.psp == psp && s.at == at);
    let mut shell = match found {
        Some(index) => cpu.secondary_shells.remove(index),
        None => {
            let base = psp as u32 * 16;
            let len = cpu.bus.guest_read_8(base + 0x80) as usize;
            let mut tail = vec![0u8; len.min(126)];
            cpu.bus.guest_read_bytes(base + 0x81, &mut tail);
            if cpu.bus.guest_faulted() {
                return;
            }
            let shell = SecondaryShell { at, ..SecondaryShell::new(psp, &crate::dosstr::from_bytes(&tail)) };
            // The program keeps the memory it needs; the rest is for the
            // programs it runs.
            let _ = crate::mcb::resize(&mut cpu.bus, psp, PARAGRAPHS);
            if shell.command.is_none() {
                print_string(cpu, &format!("\r\nRust-DOS {}. Type EXIT to go back.\r\n", env!("CARGO_PKG_VERSION")));
            }
            shell
        }
    };
    shell.swap(cpu);
    let next = step(cpu, &mut shell);
    shell.swap(cpu);
    match next {
        Ok(action) => {
            cpu.set_reg8(iced_x86::Register::AL, action);
            cpu.secondary_shells.push(shell);
        }
        Err(code) => {
            cpu.set_reg8(iced_x86::Register::AL, EXIT);
            cpu.set_reg8(iced_x86::Register::BL, code);
        }
    }
    crate::dos_data::store_process_state(cpu);
}

/// With the shell's state in the machine's place: what the program does
/// next, or Err(exit code).
fn step(cpu: &mut Cpu, shell: &mut SecondaryShell) -> Result<u8, u8> {
    let base = shell.psp as u32 * 16;
    match std::mem::replace(&mut shell.asked, Asked::Nothing) {
        Asked::Nothing => {}
        Asked::Exec => {
            // The files the program's command line redirected its input
            // and output to are closed.
            crate::dos_files::restore_console(&mut cpu.bus, shell.psp);
            crate::dos_files::flush(&mut cpu.bus);
            if cpu.get_cpu_flag(CpuFlags::CF) {
                print_string(cpu, "Bad command or file name.\r\n");
            } else {
                cpu.errorlevel = cpu.last_child_exit as u8;
                cpu.last_child_exit = 0;
            }
        }
        Asked::Key => {
            // AX 0: no key came before the tick.
            let done = match cpu.ax() {
                0 if matches!(cpu.shell_wait, Some(ShellWait::Edit(_))) => !crate::edit::tick(cpu),
                0 => crate::shell::timed_out_key(cpu).is_some_and(|key| crate::shell::take_key(cpu, key as u16)),
                key => crate::shell::take_key(cpu, key),
            };
            if !done {
                shell.asked = Asked::Key;
                return Ok(KEY);
            }
        }
        Asked::Line => {
            let len = cpu.bus.guest_read_8(base + LINE as u32 + 1) as usize;
            let mut line = vec![0u8; len];
            cpu.bus.guest_read_bytes(base + LINE as u32 + 2, &mut line);
            let line = crate::dosstr::from_bytes(&line);
            print_string(cpu, "\r\n");
            match cpu.shell_wait.take() {
                Some(ShellWait::Line(purpose)) => crate::time_commands::line_entered(cpu, purpose, &line),
                _ => {
                    run(cpu, shell, &line, false);
                    return Ok(next_action(cpu, shell));
                }
            }
        }
    }
    if let Some(wait) = &cpu.shell_wait {
        return Ok(wait_action(shell, wait));
    }
    // The next command line: the /C or /K command, a batch line, or one
    // typed at the prompt.
    if let Some(command) = shell.command.take() {
        run(cpu, shell, &command, false);
        return Ok(next_action(cpu, shell));
    }
    if cpu.batch.is_active() {
        if let Some(line) = cpu.batch.next_line(&cpu.environment) {
            if line.echo {
                crate::shell::show_prompt(cpu);
                print_string(cpu, &format!("{}\r\n", line.text));
            }
            cpu.bus.clock.stall(crate::exec::BATCH_LINE_NS);
            run(cpu, shell, &line.text, true);
        }
        return Ok(next_action(cpu, shell));
    }
    cpu.batch.settle();
    if shell.once {
        return Err(cpu.errorlevel);
    }
    if cpu.batch.echo {
        crate::shell::show_prompt(cpu);
    }
    cpu.bus.guest_write_8(base + LINE as u32, 128);
    shell.asked = Asked::Line;
    Ok(READ_LINE)
}

/// Run a command line in the shell (a batch line's with `from_batch`).
fn run(cpu: &mut Cpu, shell: &mut SecondaryShell, line: &str, from_batch: bool) {
    cpu.secondary = Some(Dispatch::default());
    cpu.batch.dispatching = from_batch;
    crate::exec::run_command_line(cpu, line);
    cpu.batch.dispatching = false;
    if from_batch {
        cpu.batch.settle();
    }
    let dispatch = cpu.secondary.take().unwrap_or_default();
    if dispatch.exit {
        shell.once = true;
        cpu.batch.clear();
    }
    if let Some((path, args)) = dispatch.exec {
        set_up_exec(cpu, shell.psp, &path, &args);
        shell.asked = Asked::Exec;
    }
}

/// What the program does after a command line ran: EXEC what it asked
/// for, wait for what it waits for, or ask again.
fn next_action(cpu: &Cpu, shell: &mut SecondaryShell) -> u8 {
    if shell.asked == Asked::Exec {
        return EXEC;
    }
    match &cpu.shell_wait {
        Some(wait) => wait_action(shell, wait),
        None => AGAIN,
    }
}

fn wait_action(shell: &mut SecondaryShell, wait: &ShellWait) -> u8 {
    if matches!(wait, ShellWait::Line(_)) {
        shell.asked = Asked::Line;
        READ_LINE
    } else {
        shell.asked = Asked::Key;
        KEY
    }
}

/// Write what EXEC needs into the program's memory: the path, the command
/// tail, the FCBs of the first two parameters and the parameter block.
fn set_up_exec(cpu: &mut Cpu, psp: u16, path: &str, args: &str) {
    let base = psp as u32 * 16;
    let bus = &mut cpu.bus;
    let mut name = crate::dosstr::to_bytes(path);
    name.truncate(127);
    name.push(0);
    bus.guest_write_bytes(base + PATH as u32, &name);
    let args = crate::dosstr::to_bytes(args.trim());
    let mut tail = Vec::new();
    if !args.is_empty() {
        tail.push(b' ');
        tail.extend(args.iter().take(125));
    }
    tail.insert(0, tail.len() as u8);
    tail.push(0x0D);
    bus.guest_write_bytes(base + TAIL as u32, &tail);
    crate::interrupts::fcb::set_fcbs(bus, base + FCB1 as u32, base + FCB2 as u32, &args);
    let params = base + PARAMS as u32;
    bus.guest_write_16(params, 0);
    for (i, offset) in [TAIL, FCB1, FCB2].into_iter().enumerate() {
        bus.guest_write_16(params + 2 + 4 * i as u32, offset);
        bus.guest_write_16(params + 4 + 4 * i as u32, psp);
    }
}

crate::state_enum!(Asked { Asked::Nothing, Asked::Exec, Asked::Key, Asked::Line });
crate::state_fields!(SecondaryShell { psp, at, batch, errorlevel, wait, once, command, asked });

impl Default for SecondaryShell {
    fn default() -> Self {
        SecondaryShell::new(0, "")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_say_what_it_runs() {
        let shell = SecondaryShell::new(1, " /C DIR /W");
        assert_eq!((shell.once, shell.command.as_deref()), (true, Some("DIR /W")));
        let shell = SecondaryShell::new(1, "C:\\ /E:1024 /k game");
        assert_eq!((shell.once, shell.command.as_deref()), (false, Some("game")));
        let shell = SecondaryShell::new(1, "/cgame.bat");
        assert_eq!((shell.once, shell.command.as_deref()), (true, Some("game.bat")));
        let shell = SecondaryShell::new(1, "");
        assert_eq!((shell.once, shell.command), (false, None));
    }

    #[test]
    fn the_program_fits_below_its_buffers() {
        assert!(0x100 + stub_code().len() <= PATH as usize);
    }
}
