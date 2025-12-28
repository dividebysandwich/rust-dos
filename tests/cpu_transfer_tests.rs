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
