use rust_dos::cpu::{Cpu, SHELL_SEGMENT};
use rust_dos::exec::{self, ExecHook};
use rust_dos::shell;
use std::fs;
use std::path::PathBuf;

/// Fresh directory with the given files under target/.
fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let base = PathBuf::from("target/test_ivt").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

fn ivt(cpu: &Cpu, vector: usize) -> (u16, u16) {
    (cpu.bus.read_16(vector * 4 + 2), cpu.bus.read_16(vector * 4))
}

/// INT 3, INT 1 and INT 4, then AX = 1234h and HLT.
const INTS: &[u8] = &[0xCC, 0xCD, 0x01, 0xCD, 0x04, 0xB8, 0x34, 0x12, 0xF4];

/// Step `cpu` until it reaches a HLT in the program at `psp`.
fn run_to_hlt(cpu: &mut Cpu, psp: u16) {
    for _ in 0..1000 {
        let at = cpu.get_physical_addr(cpu.cs(), cpu.ip());
        if cpu.cs() == psp && cpu.bus.read_8(at) == 0xF4 {
            return;
        }
        cpu.step();
    }
    panic!("no HLT reached; at {:04X}:{:04X}", cpu.cs(), cpu.ip());
}

#[test]
fn int_1_3_and_4_point_at_an_iret_in_segment_0070_as_in_ms_dos() {
    // MS-DOS's IO.SYS points INT 1, 3 and 4 at an IRET in its own
    // segment, 0070h. Maelstrom takes the word at 0000:000E, INT 3's
    // segment, as the flags of a button it has no pointer to, and skips
    // calling it only when bit 4 is set, as it is in 0070h and not in the
    // BIOS's F000h.
    let base = scratch("dos_iret", &[("INTS.COM", INTS)]);
    let mut cpu = Cpu::new(base);
    cpu.load_shell();
    for vector in [0x01, 0x03, 0x04] {
        let (segment, offset) = ivt(&cpu, vector);
        assert_eq!(segment, SHELL_SEGMENT, "INT {vector:02X}h");
        assert_eq!(
            cpu.bus.read_8(cpu.get_physical_addr(segment, offset)),
            0xCF,
            "INT {vector:02X}h"
        );
    }
    assert_ne!(cpu.bus.read_16(0x0E) & 0x10, 0);

    // A program's INT 3, INT 1 and INT 4 come straight back.
    assert!(cpu.load_executable("INTS.COM", None));
    let psp = cpu.current_psp;
    run_to_hlt(&mut cpu, psp);
    assert_eq!(cpu.ax(), 0x1234);
}

/// Where the shell was each time it took a command line to run.
struct ShellAt(Vec<(u16, u16)>);

impl ExecHook for ShellAt {
    fn before_exec(&mut self, _cpu: &Cpu, _phys_ip: usize, _ram: &[u8]) -> bool {
        false
    }

    fn before_shell(&mut self, cpu: &Cpu) -> bool {
        self.0.push((cpu.cs(), cpu.ip()));
        false
    }
}

#[test]
fn a_handler_at_the_iret_is_not_the_shell() {
    // A resident program's handler can reach the IRET while the shell
    // waits at its prompt: an INT 3 or INTO in a timer hook, or code
    // traced with TF. It runs in the shell's segment, but the shell must
    // not take a batch line there: the line would run inside the handler,
    // which would never return or send its EOI.
    let base = scratch("not_the_shell", &[]);
    let mut cpu = Cpu::new(base);
    cpu.bus.set_cycles_per_ms(1000);
    cpu.load_shell();
    let at_shell = (cpu.cs(), cpu.ip());
    // The handler, INT 3 and IRET, entered as an interrupt would be
    // before the shell's first instruction.
    let handler = 0x9000;
    cpu.bus.write_8(handler * 16, 0xCC);
    cpu.bus.write_8(handler * 16 + 1, 0xCF);
    cpu.push(cpu.flags16());
    cpu.push(at_shell.0);
    cpu.push(at_shell.1);
    cpu.set_cs(handler as u16);
    cpu.set_ip(0);
    cpu.queue_batch_lines(["REM"]);

    cpu.step();
    assert_eq!((cpu.cs(), cpu.ip()), (SHELL_SEGMENT, shell::labels().int_iret));
    assert!(!cpu.in_shell_code());
    assert!(!cpu.at_shell_prompt());
    assert!(!cpu.shell_idle());

    // The line waits until the handler has returned to the shell.
    let mut hook = ShellAt(Vec::new());
    cpu.bus.start_batch(cpu.bus.clock.icount + 100);
    exec::run_batch(&mut cpu, &mut hook, true);
    assert_eq!(hook.0.first(), Some(&at_shell));
}

#[test]
fn the_iret_is_there_before_the_shell_loads() {
    // The BIOS puts it there as it sets the vectors up, for a program
    // that runs on a machine whose shell hasn't loaded yet.
    let base = scratch("before_shell", &[]);
    let cpu = Cpu::new(base);
    let (segment, offset) = ivt(&cpu, 0x03);
    assert_eq!(segment, SHELL_SEGMENT);
    assert_eq!(cpu.bus.read_8(cpu.get_physical_addr(segment, offset)), 0xCF);
}

#[test]
fn a_booted_system_gets_the_bioss_iret_for_int_1_3_and_4() {
    // A DOS booted from a disk sets these itself, as IO.SYS does; until
    // then they point at the BIOS's dummy handler, as on an AT.
    let ivt = rust_dos::bios::boot_ivt();
    for vector in [0x01, 0x03, 0x04] {
        assert_eq!(
            ivt[vector],
            0xF000_0000 | rust_dos::bios::IRET_HANDLER as u32
        );
    }
}
