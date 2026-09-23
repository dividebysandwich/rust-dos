use rust_dos::cpu::{Cpu, CpuFlags, CpuState};
use std::fs;
use std::path::PathBuf;

/// Fresh directory with the given files under target/.
fn scratch(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let base = PathBuf::from("target/test_exec").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    for (file, bytes) in files {
        fs::write(base.join(file), bytes).unwrap();
    }
    base
}

/// A CPU at 0000:0100 with `code` there, an IRQ 0 handler at 0000:0600,
/// and interrupts enabled or not.
fn cpu_with_code(code: &[u8], interrupts: bool) -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.bus.load_bytes(0x100, code);
    cpu.bus.write_16(0x08 * 4, 0x0600);
    cpu.bus.write_16(0x08 * 4 + 2, 0x0000);
    cpu.set_ss(0x0000);
    cpu.set_sp(0x2000);
    cpu.set_cpu_flag(CpuFlags::IF, interrupts);
    cpu
}

fn raise_irq0(cpu: &mut Cpu) {
    cpu.bus.pic.raise(0);
}

#[test]
fn sti_delays_interrupts_by_one_instruction() {
    // FB -> STI ; 90 -> NOP ; 90 -> NOP
    let mut cpu = cpu_with_code(&[0xFB, 0x90, 0x90], false);
    raise_irq0(&mut cpu);

    cpu.step(); // STI
    assert_eq!(cpu.ip(), 0x101);
    cpu.step(); // The pending IRQ waits for the instruction after STI.
    assert_eq!(cpu.ip(), 0x102);
    cpu.step(); // Now it is taken.
    assert_eq!((cpu.cs(), cpu.ip()), (0x0000, 0x0600));
}

#[test]
fn mov_ss_delays_interrupts_until_sp_is_loaded() {
    // 8E D0 -> MOV SS, AX ; 89 DC -> MOV SP, BX ; 90 -> NOP
    let mut cpu = cpu_with_code(&[0x8E, 0xD0, 0x89, 0xDC, 0x90], true);
    cpu.set_ax(0x0000);
    cpu.set_bx(0x3000);

    cpu.step(); // MOV SS, AX
    raise_irq0(&mut cpu);
    cpu.step(); // MOV SP, BX runs before the IRQ.
    assert_eq!((cpu.ip(), cpu.sp()), (0x104, 0x3000));
    cpu.step();
    assert_eq!((cpu.cs(), cpu.ip()), (0x0000, 0x0600));
    // The IRQ frame is on the new stack.
    assert_eq!(cpu.sp(), 0x3000 - 6);
}

#[test]
fn com_program_returning_to_psp_offset_0_terminates() {
    // C3 -> RET: pops the 0000h that DOS leaves at the top of a COM
    // program's stack and runs the INT 20h at PSP:0000.
    let dir = scratch("ret_to_psp", &[("RET.COM", &[0xC3])]);
    let mut cpu = Cpu::new(dir);
    assert!(cpu.load_executable("RET.COM", None));

    for _ in 0..10 {
        if cpu.state == CpuState::RebootShell {
            break;
        }
        cpu.step();
    }
    assert_eq!(cpu.state, CpuState::RebootShell);
}
