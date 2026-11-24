use rust_dos::cpu::{Cpu, SHELL_SEGMENT};
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
