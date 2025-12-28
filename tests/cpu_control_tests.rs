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