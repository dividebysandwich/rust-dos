use rust_dos::cpu::{Cpu, CpuFlags, CpuState};
use iced_x86::Register;
mod testrunners;
use testrunners::run_cpu_code;

#[test]
fn test_flags_operations() {
    let mut cpu = Cpu::new();

    // STC: Set Carry
    // F9
    run_cpu_code(&mut cpu, &[0xF9]); 
    assert!(cpu.get_cpu_flag(CpuFlags::CF));

    // CLC: Clear Carry
    // F8
    run_cpu_code(&mut cpu, &[0xF8]); 
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));

    // CMC: Complement (Toggle) Carry
    // F9 (Set), F5 (Toggle -> Clear)
    run_cpu_code(&mut cpu, &[0xF9, 0xF5]); 
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));

    // STD / CLD: Direction Flag
    // FD (Set), FC (Clear)
    run_cpu_code(&mut cpu, &[0xFD]); 
    assert!(cpu.get_cpu_flag(CpuFlags::DF));
    run_cpu_code(&mut cpu, &[0xFC]); 
    assert!(!cpu.get_cpu_flag(CpuFlags::DF));
}

#[test]
fn test_hlt_state() {
    let mut cpu = Cpu::new();
    // F4: HLT
    run_cpu_code(&mut cpu, &[0xF4]);
    assert!(matches!(cpu.state, CpuState::Halted));
}

#[test]
fn test_int_and_iret() {
    let mut cpu = Cpu::new();
    cpu.ip = 0x100;
    cpu.sp = 0xFFFE;
    cpu.cs = 0x0000;

    // We construct a scenario where the Interrupt Handler is INLINE
    // with our test code buffer so the runner can verify execution.
    // Layout:
    // 0x100: INT 0x20   (CD 20) -> Jumps to Handler
    // 0x102: HLT        (F4)    -> Return point (Runner stops here)
    // 0x103: IRET       (CF)    -> Handler Code
    
    // 1. Setup IVT for INT 0x20 (Offset 0x80)
    // Vector points to 0000:0103
    let ivt_addr = 0x20 * 4;
    cpu.bus.write_16(ivt_addr, 0x0103); // Offset
    cpu.bus.write_16(ivt_addr + 2, 0x0000); // Segment

    let code = [
        0xCD, 0x20, // 0x100: INT 0x20
        0xF4,       // 0x102: HLT (Expected Return Address)
        0xCF        // 0x103: IRET (Handler)
    ];
    
    run_cpu_code(&mut cpu, &code);

    // Logic Check:
    // 1. INT executes. Pushes Flags, CS, IP(0x102). IP becomes 0x103.
    // 2. IRET executes. Pops IP(0x102), CS, Flags. IP becomes 0x102.
    // 3. HLT executes. Runner stops.

    assert_eq!(cpu.sp, 0xFFFE, "Stack should be balanced after INT+IRET");
    assert_eq!(cpu.ip, 0x103, "Runner IP should point after HLT (0x102 + 1)");
}

#[test]
fn test_into_overflow() {
    let mut cpu = Cpu::new();
    cpu.sp = 0xFFFE;
    cpu.ss = 0x0000;
    
    // 1. Setup INT 4 Vector (Address 0x10)
    // We must point it somewhere valid so the emulator performs the PUSH
    cpu.bus.write_16(0x10, 0x0200); // IP
    cpu.bus.write_16(0x12, 0x0000); // CS

    // Case 1: Overflow (OF=1)
    cpu.set_cpu_flag(CpuFlags::OF, true);
    
    // CE: INTO
    run_cpu_code(&mut cpu, &[0xCE]);
    
    // Should have pushed Flags(2) + CS(2) + IP(2) = 6 bytes
    // SP: FFFE - 6 = FFF8
    assert_eq!(cpu.sp, 0xFFF8, "INTO should push interrupt stack frame if OF=1");
}

#[test]
fn test_enter_leave_stack_frames() {
    let mut cpu = Cpu::new();
    cpu.sp = 0xFFFE;
    cpu.bp = 0xAAAA;
    cpu.ss = 0x0000; // Important for get_physical_addr

    // ENTER 10, 0  (C8 0A 00 00)
    let code_enter_0 = [0xC8, 0x0A, 0x00, 0x00];
    run_cpu_code(&mut cpu, &code_enter_0);

    // 1. Push BP (FFFC) = AAAA
    // 2. FP = FFFC
    // 3. SP = FFFC - 10 = FFF2
    assert_eq!(cpu.bus.read_16(0xFFFC), 0xAAAA, "Old BP not saved correctly");
    assert_eq!(cpu.bp, 0xFFFC, "BP not updated to Frame Pointer");
    assert_eq!(cpu.sp, 0xFFF2, "SP not allocated for locals");

    // LEAVE (C9)
    run_cpu_code(&mut cpu, &[0xC9]);
    
    assert_eq!(cpu.bp, 0xAAAA);
    assert_eq!(cpu.sp, 0xFFFE);
}

#[test]
fn test_enter_nested_level() {
    let mut cpu = Cpu::new();
    cpu.sp = 0xFFFE;
    cpu.bp = 0x8000;
    cpu.ss = 0x0000;

    // ENTER 4, 1 (C8 04 00 01)
    run_cpu_code(&mut cpu, &[0xC8, 0x04, 0x00, 0x01]);

    // 1. Push BP (FFFC)=8000
    // 2. FP = FFFC
    // 3. Level 1: Push FP (FFFC) at FFFA
    // 4. BP = FFFC
    // 5. SP = FFFA - 4 = FFF6
    
    assert_eq!(cpu.bus.read_16(0xFFFC), 0x8000, "Should push old BP");
    assert_eq!(cpu.bus.read_16(0xFFFA), 0xFFFC, "Should push Frame Pointer for Display");
    assert_eq!(cpu.bp, 0xFFFC, "BP should point to the new frame base");
    assert_eq!(cpu.sp, 0xFFF6, "Final SP incorrect");
}