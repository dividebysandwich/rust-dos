use rust_dos::cpu::{Cpu, FpuFlags, FPU_TAG_EMPTY};
use rust_dos::f80::F80;

mod testrunners;

// Helper to push values quickly
fn push_val(cpu: &mut Cpu, val: f64) {
    let mut f = F80::new();
    f.set_f64(val);
    cpu.fpu_push(f);
}

#[test]
fn test_fcompp_stack_cleanup() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let initial_top = cpu.fpu_top;
    
    cpu.fpu_push(F80::new());
    cpu.fpu_push(F80::new());
    
    // DE D9: FCOMPP (Compare ST(0) with ST(1) and pop twice)
    testrunners::run_fpu_code(&mut cpu, &[0xDE, 0xD9]);

    assert_eq!(cpu.fpu_top, initial_top, "FCOMPP failed to restore FPU TOP");
    assert_eq!(cpu.fpu_tags[initial_top], FPU_TAG_EMPTY);
}

#[test]
fn test_fcom_reverse_opcode_dc() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // Scenario: ST(0)=8.0, ST(1)=10.0
    push_val(&mut cpu, 10.0); // ST(1)
    push_val(&mut cpu, 8.0);  // ST(0)

    // DC D1: FCOM ST(1), ST(0) -> Compare ST(1) [LHS] with ST(0) [RHS]
    // Is 10.0 < 8.0? -> NO. 
    testrunners::run_fpu_code(&mut cpu, &[0xDC, 0xD1]);

    let flags = cpu.get_fpu_flags();
    if flags.contains(FpuFlags::C0) {
        panic!("FATAL: FCOM logic is swapped! 10.0 < 8.0 returned TRUE.");
    }
}

#[test]
fn test_fcom_standard_register() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // Scenario: ST(0)=10.0, ST(1)=8.0
    push_val(&mut cpu, 8.0);  // ST(1)
    push_val(&mut cpu, 10.0); // ST(0)

    // D8 D1: FCOM ST(1) -> Compare ST(0) [LHS] with ST(1) [RHS]
    // 10.0 > 8.0? -> YES. C0=0, C3=0.
    testrunners::run_fpu_code(&mut cpu, &[0xD8, 0xD1]);

    let flags = cpu.get_fpu_flags();
    assert!(!flags.contains(FpuFlags::C0)); // Not Less Than
    assert!(!flags.contains(FpuFlags::C3)); // Not Equal
}

#[test]
fn test_fcom_memory_float32() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    push_val(&mut cpu, 5.0); // ST(0)

    // Write 10.0f32 to memory at 0x1000
    let addr = 0x1000;
    let val_f32 = 10.0f32;
    let bytes = val_f32.to_le_bytes();
    for i in 0..4 { cpu.bus.write_8(addr + i, bytes[i]); }

    // D8 16 00 10: FCOM DWORD PTR [1000]
    // Compare 5.0 vs 10.0 -> Less Than -> C0=1
    testrunners::run_fpu_code(&mut cpu, &[0xD8, 0x16, 0x00, 0x10]);

    assert!(cpu.get_fpu_flag(FpuFlags::C0), "5.0 < 10.0 should set C0");
}

#[test]
fn test_fcom_memory_float64() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    push_val(&mut cpu, 20.0); // ST(0)

    // Write 20.0f64 to memory at 0x1000
    let addr = 0x1000;
    let val_f64 = 20.0f64;
    let bytes = val_f64.to_le_bytes();
    for i in 0..8 { cpu.bus.write_8(addr + i, bytes[i]); }

    // DC 16 00 10: FCOM QWORD PTR [1000]
    // Compare 20.0 vs 20.0 -> Equal -> C3=1
    testrunners::run_fpu_code(&mut cpu, &[0xDC, 0x16, 0x00, 0x10]);

    assert!(cpu.get_fpu_flag(FpuFlags::C3), "20.0 == 20.0 should set C3");
}

#[test]
fn test_ficom_integer_memory() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    push_val(&mut cpu, 100.0); // ST(0)

    // Write 50 (Int16) to memory
    let addr = 0x1000;
    cpu.bus.write_16(addr, 50);

    // DE 16 00 10: FICOM WORD PTR [1000]
    // Compare 100.0 vs 50 -> Greater -> C0=0, C3=0
    testrunners::run_fpu_code(&mut cpu, &[0xDE, 0x16, 0x00, 0x10]);

    assert!(!cpu.get_fpu_flag(FpuFlags::C0));
    assert!(!cpu.get_fpu_flag(FpuFlags::C3));
}

#[test]
fn test_ftst_compare_zero() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // Case 1: Positive
    push_val(&mut cpu, 123.4);
    // D9 E4: FTST
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xE4]);
    assert!(!cpu.get_fpu_flag(FpuFlags::C0)); // Not < 0.0
    assert!(!cpu.get_fpu_flag(FpuFlags::C3)); // Not == 0.0

    // Case 2: Zero
    push_val(&mut cpu, 0.0);
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xE4]);
    assert!(cpu.get_fpu_flag(FpuFlags::C3)); // Equal 0.0

    // Case 3: Negative
    push_val(&mut cpu, -5.0);
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xE4]);
    assert!(cpu.get_fpu_flag(FpuFlags::C0)); // Less than 0.0
}

#[test]
fn test_fcomp_single_pop() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    push_val(&mut cpu, 1.0);
    push_val(&mut cpu, 2.0); // ST(0)

    let initial_top = cpu.fpu_top;

    // D8 D9: FCOMP ST(1)
    // Compare 2.0 vs 1.0, then Pop ONCE.
    testrunners::run_fpu_code(&mut cpu, &[0xD8, 0xD9]);

    assert_eq!(cpu.fpu_top, (initial_top + 1) % 8, "FCOMP should pop once");
    assert_eq!(cpu.fpu_get(0).get_f64(), 1.0, "Old ST(1) should now be ST(0)");
}

#[test]
fn test_nan_unordered_compare() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // Create a NaN
    let mut nan = F80::new();
    nan.set_QNaN();
    cpu.fpu_push(nan);
    
    // Compare NaN vs 0.0 (FTST)
    // D9 E4: FTST
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xE4]);

    // Expect Unordered: C3=1, C2=1, C0=1
    assert!(cpu.get_fpu_flag(FpuFlags::C0));
    assert!(cpu.get_fpu_flag(FpuFlags::C2));
    assert!(cpu.get_fpu_flag(FpuFlags::C3));
}