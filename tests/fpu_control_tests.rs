use rust_dos::cpu::{Cpu, FpuFlags, FPU_TAG_EMPTY, FPU_TAG_VALID};

mod testrunners;

#[test]
fn test_fninit_defaults() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // Dirty the state first
    cpu.fpu_top = 3;
    cpu.fpu_control = 0xFFFF;
    cpu.set_fpu_flags(FpuFlags::C0 | FpuFlags::C3 | FpuFlags::PE);
    cpu.fpu_tags[0] = FPU_TAG_VALID;
    cpu.fpu_tags[7] = FPU_TAG_VALID;

    // DB E3: FNINIT
    testrunners::run_cpu_code(&mut cpu, &[0xDB, 0xE3]);

    // Verify Defaults
    assert_eq!(cpu.fpu_top, 0, "FNINIT should reset TOP to 0");
    assert_eq!(cpu.fpu_control, 0x037F, "FNINIT should reset CW to 0x037F");
    assert_eq!(cpu.get_fpu_flags().bits(), 0, "FNINIT should clear Status Word");
    
    // Verify all tags are empty
    for i in 0..8 {
        assert_eq!(cpu.fpu_tags[i], FPU_TAG_EMPTY, "Tag {} should be EMPTY", i);
    }
}

#[test]
fn test_fnclex_clears_exceptions() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // Set Condition Codes (should stay) and Exceptions (should clear)
    cpu.set_fpu_flags(FpuFlags::C0 | FpuFlags::C3 | FpuFlags::PE | FpuFlags::ZE | FpuFlags::IE);
    
    // DB E2: FNCLEX
    testrunners::run_cpu_code(&mut cpu, &[0xDB, 0xE2]);

    let flags = cpu.get_fpu_flags();
    
    // Exceptions should be gone
    assert!(!flags.contains(FpuFlags::PE));
    assert!(!flags.contains(FpuFlags::ZE));
    assert!(!flags.contains(FpuFlags::IE));
    
    // Condition codes should persist
    assert!(flags.contains(FpuFlags::C0), "C0 should be preserved");
    assert!(flags.contains(FpuFlags::C3), "C3 should be preserved");
}

#[test]
fn test_fnstsw_ax_status_word_bridge() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // Scenario: TOP=5, C3=1 (Eq), C0=1 (Carry/Less)
    // This simulates a comparison result that needs to move to CPU flags
    cpu.fpu_top = 5;
    cpu.set_fpu_flags(FpuFlags::C3 | FpuFlags::C0);

    // DF E0: FNSTSW AX
    testrunners::run_cpu_code(&mut cpu, &[0xDF, 0xE0]);

    let sw = cpu.ax;

    // 1. Check Condition Code Bits
    // C0 is Bit 8 (0x0100)
    // C3 is Bit 14 (0x4000)
    assert_eq!(sw & 0x0100, 0x0100, "Bit 8 (C0) not set in AX");
    assert_eq!(sw & 0x4000, 0x4000, "Bit 14 (C3) not set in AX");

    // 2. Check TOP Pointer Packing (Bits 11-13)
    // TOP=5 (101 binary) -> should be at bits 11-13
    // (5 << 11) = 0x2800
    let top_in_sw = (sw >> 11) & 0x07;
    assert_eq!(top_in_sw, 5, "TOP pointer not correctly packed into Status Word bits 11-13");
    
    // 3. Verify Busy Bit (Bit 15) is 0 (since we aren't executing)
    assert_eq!(sw & 0x8000, 0);
}

#[test]
fn test_fnstsw_memory() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.fpu_top = 2;
    cpu.set_fpu_flags(FpuFlags::C2); // Bit 10

    // DD 3E 00 02: FNSTSW [0200]
    // Write 0x0000 to memory first to ensure we actually wrote to it
    cpu.bus.write_16(0x200, 0x0000);

    testrunners::run_cpu_code(&mut cpu, &[0xDD, 0x3E, 0x00, 0x02]);

    let val = cpu.bus.read_16(0x200);
    
    // Expected: TOP=2 in bits 11-13 (0x1000) | C2 in bit 10 (0x0400)
    // Total: 0x1400
    assert_eq!(val & 0x1400, 0x1400);
}

#[test]
fn test_control_word_store_load() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // Set a weird control word manually: Round Down (0x0400), Single Precision (0x0000)
    let new_cw: u16 = 0x0400 | 0x0300 | 0x003F; // + Mask all exceptions
    cpu.fpu_control = new_cw;

    // D9 3E 00 02: FNSTCW [0200]
    testrunners::run_cpu_code(&mut cpu, &[0xD9, 0x3E, 0x00, 0x02]);
    assert_eq!(cpu.bus.read_16(0x200), new_cw);

    // Now modify memory and load it back
    let load_cw: u16 = 0x0C00; // Round to Zero
    cpu.bus.write_16(0x200, load_cw);

    // D9 2E 00 02: FLDCW [0200]
    testrunners::run_cpu_code(&mut cpu, &[0xD9, 0x2E, 0x00, 0x02]);

    assert_eq!(cpu.fpu_control, load_cw);
}

#[test]
fn test_ffree_tag_management() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // Initialize standard stack
    cpu.fpu_top = 0;
    // ST(0) is at phys index 0
    // ST(1) is at phys index 1 (if we view it as a ring buffer growing up/down)
    // Let's use internal logic to set tags
    
    // Mark ST(0) and ST(1) as Valid
    cpu.fpu_tags[0] = FPU_TAG_VALID;
    cpu.fpu_tags[1] = FPU_TAG_VALID;

    // DD C0: FFREE ST(0)
    // iced_x86 might map DD C0 to FFREE ST(0)
    testrunners::run_cpu_code(&mut cpu, &[0xDD, 0xC0]);
    
    // ST(0) (Phys 0) should be empty
    assert_eq!(cpu.fpu_tags[0], FPU_TAG_EMPTY);
    
    // ST(1) (Phys 1) should still be valid
    assert_eq!(cpu.fpu_tags[1], FPU_TAG_VALID);
}
#[test]
fn test_stack_pointer_manipulation() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.fpu_top = 0;
    
    // D9 F7: FINCSTP (Increment TOP)
    // 0 -> 1 (Does not change tags or values, just the pointer)
    testrunners::run_cpu_code(&mut cpu, &[0xD9, 0xF7]);
    assert_eq!(cpu.fpu_top, 1);

    // D9 F6: FDECSTP (Decrement TOP)
    // 1 -> 0
    testrunners::run_cpu_code(&mut cpu, &[0xD9, 0xF6]);
    assert_eq!(cpu.fpu_top, 0);

    // Wrap around check: 0 -> 7
    testrunners::run_cpu_code(&mut cpu, &[0xD9, 0xF6]);
    assert_eq!(cpu.fpu_top, 7);
}

#[test]
fn test_fsave_frstor_full_cycle() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // 1. Setup a unique "Dirty" State
    cpu.fpu_control = 0x1234; // Non-default Control Word
    cpu.set_fpu_flags(FpuFlags::C2 | FpuFlags::ZE); // Non-default Status
    cpu.fpu_top = 3;
    
    // Set a value at Physical Register 3.
    // Since TOP=3, this would be ST(0). 
    // We modify the physical array directly to ensure raw state preservation.
    let mut val = rust_dos::f80::F80::new();
    val.set_f64(123.456);
    cpu.fpu_stack[3] = val; // CORRECTION: Use fpu_stack
    cpu.fpu_tags[3] = FPU_TAG_VALID;

    // 2. Execute FSAVE [1000] (9B DD 36 00 10)
    // FSAVE writes state to memory and then runs FNINIT
    testrunners::run_cpu_code(&mut cpu, &[0x9B, 0xDD, 0x36, 0x00, 0x10]);

    // Verify FPU was reset by FSAVE (FNINIT behavior)
    assert_eq!(cpu.fpu_top, 0);
    assert_eq!(cpu.fpu_control, 0x037F);
    assert_eq!(cpu.fpu_tags[3], FPU_TAG_EMPTY); 
    
    // 3. Scramble the state to prove FRSTOR actually overwrites it
    cpu.fpu_control = 0xFFFF;
    cpu.fpu_stack[3].set_f64(0.0); // CORRECTION: Use fpu_stack

    // 4. Execute FRSTOR [1000] (DD 26 00 10)
    testrunners::run_cpu_code(&mut cpu, &[0xDD, 0x26, 0x00, 0x10]);

    // 5. Verify Restoration
    assert_eq!(cpu.fpu_control, 0x1234, "Control Word not restored");
    assert_eq!(cpu.fpu_top, 3, "TOP ptr not restored");
    assert!(cpu.get_fpu_flags().contains(FpuFlags::C2), "Status Flags not restored");
    
    // Verify the value came back
    let restored_val = cpu.fpu_stack[3].get_f64(); // CORRECTION: Use fpu_stack
    assert!((restored_val - 123.456).abs() < 0.001, "Register value lost during Save/Restore");
    assert_eq!(cpu.fpu_tags[3], FPU_TAG_VALID, "Tag Word not restored correctly");
}