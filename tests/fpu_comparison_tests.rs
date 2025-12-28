use rust_dos::cpu::{Cpu, FpuFlags};
use rust_dos::f80::F80;

mod testrunners;

#[test]
fn test_fcompp_stack_cleanup() {
    let mut cpu = Cpu::new();
    let initial_top = cpu.fpu_top;
    
    cpu.fpu_push(F80::new());
    cpu.fpu_push(F80::new());
    
    // DE D9: FCOMPP (Compare ST(0) with ST(1) and pop twice)
    testrunners::run_fpu_code(&mut cpu, &[0xDE, 0xD9]);

    assert_eq!(cpu.fpu_top, initial_top, "FCOMPP failed to restore FPU TOP");
    assert_eq!(cpu.fpu_tags[initial_top], rust_dos::cpu::FPU_TAG_EMPTY);
}

#[test]
fn test_fcom_reverse_opcode_dc() {
    let mut cpu = Cpu::new();
    
    // Scenario: ST(0)=8.0, ST(1)=10.0
    // We want to check if 10.0 < 8.0 (Which is FALSE)
    let mut f10 = F80::new(); f10.set_f64(10.0);
    let mut f8 = F80::new(); f8.set_f64(8.0);
    
    cpu.fpu_push(f10); // ST(1)
    cpu.fpu_push(f8);  // ST(0)

    // Opcode DC D1: FCOM ST(1), ST(0)
    // Semantics: Compare ST(1) [LHS] with ST(0) [RHS]
    // Is 10.0 < 8.0? -> NO. C0 should be 0.
    testrunners::run_fpu_code(&mut cpu, &[0xDC, 0xD1]);

    // YOUR CURRENT CODE FAIL:
    // It ignores DC, grabs ST(0)=8.0 as LHS, ST(1)=10.0 as RHS.
    // Checks: 8.0 < 10.0? -> YES. Sets C0=1.
    
    let flags = cpu.get_fpu_flags();
    if flags.contains(FpuFlags::C0) {
        panic!("FATAL: FCOM logic is swapped! 10.0 < 8.0 returned TRUE (C0 set). This causes the '08' bug.");
    }
}