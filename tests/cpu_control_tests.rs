use rust_dos::cpu::{Cpu, CpuFlags};
mod testrunners;
use testrunners::run_cpu_code;

#[test]
fn test_unconditional_jmp_near_and_far() {
    let mut cpu = Cpu::new();

    // 1. JMP Short (Relative)
    // EB 05 -> JMP +5 (Target: 0x107)
    // 90 90 90 90 90 (Padding)
    // B8 01 00 -> MOV AX, 1
    let code_short = [0xEB, 0x05, 0x90, 0x90, 0x90, 0x90, 0x90, 0xB8, 0x01, 0x00];
    run_cpu_code(&mut cpu, &code_short);
    assert_eq!(cpu.ax, 1, "Short JMP failed to reach target");

    // 2. JMP Far Direct (ptr16:16)
    // EA 00 10 00 20 -> JMP 2000:1000
    let mut cpu_far = Cpu::new();
    let code_far = [0xEA, 0x00, 0x10, 0x00, 0x20];
    run_cpu_code(&mut cpu_far, &code_far);
    assert_eq!(cpu_far.ip, 0x1000);
    assert_eq!(cpu_far.cs, 0x2000);
}

#[test]
fn test_call_and_ret() {
    let mut cpu = Cpu::new();
    cpu.ip = 0x100;
    cpu.sp = 0xFFFE;
    cpu.ss = 0x0000; // Ensure stack segment is zeroed for test simplicity

    // 0x100: CALL 0x105 (E8 02 00) -> Pushes 0x103
    // 0x103: HLT (F4)              -> STOP HERE
    // 0x104: NOP (90)              -> Padding
    // 0x105: RET (C3)              -> Returns to 0x103
    let code = [
        0xE8, 0x02, 0x00, 
        0xF4, 
        0x90, 
        0xC3
    ];

    run_cpu_code(&mut cpu, &code);
    
    assert_eq!(cpu.sp, 0xFFFE, "Stack pointer should return to initial state");
    assert_eq!(cpu.ip, 0x104, "Final IP should be at the end of the return site");
}

#[test]
fn test_conditional_jumps_logic() {
    let mut cpu = Cpu::new();
    cpu.ip = 0x100;
    
    // 31 C0    (2 bytes) -> XOR AX, AX (ZF=1)
    // 74 03    (2 bytes) -> JZ +3 (Target 0x107)
    // B8 FF FF (3 bytes) -> MOV AX, 0xFFFF (At 0x104, should be skipped)
    // B0 02    (2 bytes) -> Target (At 0x107): MOV AL, 2
    let code = [
        0x31, 0xC0, 
        0x74, 0x03, 
        0xB8, 0xFF, 0xFF, 
        0xB0, 0x02
    ];
    run_cpu_code(&mut cpu, &code);

    assert_eq!(cpu.get_al(), 2);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF));
}

#[test]
fn test_loop_instructions() {
    let mut cpu = Cpu::new();
    cpu.ip = 0x100;
    
    // B9 05 00 (3 bytes) -> MOV CX, 5
    // B0 00    (2 bytes) -> MOV AL, 0 (Loop Start at 0x103)
    // FE C0    (2 bytes) -> INC AL    (Target at 0x105)
    // E2 FD    (2 bytes) -> LOOP -3   (Back to 0x105)
    // Target is INC AL. 
    // Instruction starts at 0x107. Next IP is 0x109. 
    // 0x109 - 3 = 0x106 (Middle of INC AL!) -> Fix: E2 FB (Back to 0x105)
    
    let code = [
        0xB9, 0x05, 0x00, 
        0xB0, 0x00, 
        0xFE, 0xC0, 
        0xE2, 0xFC  // Changed FD to FC to hit 0x105 accurately
    ];
    run_cpu_code(&mut cpu, &code);

    assert_eq!(cpu.get_al(), 5);
    assert_eq!(cpu.cx, 0);
}

#[test]
fn test_jcxz_behavior() {
    let mut cpu = Cpu::new();
    
    // B9 00 00 -> MOV CX, 0
    // E3 02    -> JCXZ +2
    // B0 FF    -> MOV AL, 0xFF (Skipped)
    // B0 01    -> MOV AL, 1
    let code = [0xB9, 0x00, 0x00, 0xE3, 0x02, 0xB0, 0xFF, 0xB0, 0x01];
    run_cpu_code(&mut cpu, &code);

    assert_eq!(cpu.get_al(), 1);
}


#[test]
fn test_loop_instruction() {
    let mut cpu = Cpu::new();
    
    // Scenario: LOOP decrements CX and jumps if CX != 0.
    // Loop 5 times.
    cpu.set_reg16(iced_x86::Register::CX, 5);
    
    // E2 FE -> LOOP -2 (Jump back to self)
    // The test runner will execute this instruction repeatedly until 
    // CX becomes 0 and the loop condition fails (IP moves to next instruction).
    
    cpu.ip = 0x100;
    testrunners::run_cpu_code(&mut cpu, &[0xE2, 0xFE]); 
    
    // The loop runs to completion in one go.
    assert_eq!(cpu.get_reg16(iced_x86::Register::CX), 0);
    // IP should have advanced past the 2-byte instruction
    assert_eq!(cpu.ip, 0x102); 
}

#[test]
fn test_jump_signed_overflow_logic() {
    let mut cpu = Cpu::new();

    // Setup a specific scenario:
    // We want to simulate a Signed Overflow where the result LOOKS positive (SF=0)
    // but is actually negative (due to OF=1).
    //
    // Example: -128 (0x80) - 1 (0x01) = +127 (0x7F).
    // SF=0 (Positive result bit), OF=1 (Overflow), ZF=0.
    //
    // A correct "Jump Less" (JL) checks (SF != OF).
    // Here: (0 != 1) is TRUE. It SHOULD jump.
    //
    // If your emulator incorrectly checks only SF, it will NOT jump.
    
    cpu.set_cpu_flag(CpuFlags::SF, false);
    cpu.set_cpu_flag(CpuFlags::OF, true);
    
    // 7C 00 -> JL +0 (Jump Less). 
    // If taken, IP moves +2 (inst size) + 0 = +2.
    // If NOT taken, IP moves +2. 
    // Wait, jump offset 0 is useless for testing. Let's jump +5.
    // 7C 05 -> JL +5
    
    cpu.ip = 0x100;
    run_cpu_code(&mut cpu, &[0x7C, 0x05]);
    
    // If Jump Taken: IP = 0x100 + 2 (size) + 5 = 0x107
    // If Jump Not Taken: IP = 0x100 + 2 = 0x102
    assert_eq!(cpu.ip, 0x107, "JL failed to respect Overflow Flag (SF=0, OF=1)");
}

