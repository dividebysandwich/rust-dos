use rust_dos::cpu::{Cpu, FpuFlags, CpuFlags};
use rust_dos::f80::F80;

mod testrunners;

#[test]
fn test_fpu_to_cpu_flags_bridge() {
    let mut cpu = Cpu::new();
    
    // Compare 10.0 and 10.0 (Should set ZF)
    let mut f10 = F80::new(); f10.set_f64(10.0);
    cpu.fpu_push(f10);
    cpu.fpu_push(f10);
    
    // 1. FCOM ST(1) -> D8 D1
    testrunners::run_fpu_code(&mut cpu, &[0xD8, 0xD1]);
    assert!(cpu.get_fpu_flag(FpuFlags::C3), "FPU C3 should be set for equality");

    // 2. FSTSW AX -> DF E0
    testrunners::run_fpu_code(&mut cpu, &[0xDF, 0xE0]);
    let ah = (cpu.ax >> 8) as u8;
    assert!((ah & 0x40) != 0, "AH bit 6 (from C3) should be set. AH was {:02X}", ah);

    // 3. SAHF -> 9E
    testrunners::run_cpu_code(&mut cpu, &[0x9E]);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF), "CPU ZF should be set after SAHF");
}

#[test]
fn test_cwd_idiv_chain() {
    let mut cpu = Cpu::new();

    // SCENARIO: -100 / 2 = -50
    // AX = -100 (0xFF9C)
    // We execute CWD. This MUST set DX to 0xFFFF (Sign extension).
    // If DX stays 0, the dividend becomes 0x0000FF9C (65436), and result is huge/positive.
    
    cpu.ax = 0xFF9C; // -100
    cpu.dx = 0x0000; // Reset DX to ensure CWD actually changes it
    
    // 99 -> CWD
    testrunners::run_cpu_code(&mut cpu, &[0x99]);
    
    assert_eq!(cpu.dx, 0xFFFF, "CWD failed to sign extend AX into DX");

    // B9 02 00 -> MOV CX, 2
    // F7 F9    -> IDIV CX
    testrunners::run_cpu_code(&mut cpu, &[0xB9, 0x02, 0x00, 0xF7, 0xF9]);

    assert_eq!(cpu.ax as i16, -50, "IDIV failed (likely due to bad CWD setup)");
}