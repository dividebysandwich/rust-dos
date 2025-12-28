use rust_dos::cpu::{Cpu, CpuFlags};
use iced_x86::Register;

mod testrunners;

#[test]
fn test_logic_and_or_xor() {
    let mut cpu = Cpu::new();

    // AND: 0x0F0F & 0xFF00 = 0x0F00 (ZF=0, SF=0)
    cpu.set_reg16(iced_x86::Register::AX, 0x0F0F);
    // B9 00 FF -> MOV CX, 0xFF00
    // 21 C8    -> AND AX, CX
    testrunners::run_cpu_code(&mut cpu, &[0xB9, 0x00, 0xFF, 0x21, 0xC8]);
    assert_eq!(cpu.get_reg16(iced_x86::Register::AX), 0x0F00);
    assert!(!cpu.get_cpu_flag(CpuFlags::ZF));
    assert!(!cpu.get_cpu_flag(CpuFlags::SF));

    // OR: 0x55 AA | 0xAA 55 = 0xFFFF (SF=1)
    cpu.set_reg16(iced_x86::Register::BX, 0x55AA);
    // 81 CB 55 AA -> OR BX, 0xAA55
    testrunners::run_cpu_code(&mut cpu, &[0x81, 0xCB, 0x55, 0xAA]);
    assert_eq!(cpu.get_reg16(iced_x86::Register::BX), 0xFFFF);
    assert!(cpu.get_cpu_flag(CpuFlags::SF));

    // XOR: 0x1234 ^ 0x1234 = 0 (ZF=1)
    cpu.set_reg16(iced_x86::Register::DX, 0x1234);
    // 31 D2 -> XOR DX, DX
    testrunners::run_cpu_code(&mut cpu, &[0x31, 0xD2]);
    assert_eq!(cpu.get_reg16(iced_x86::Register::DX), 0);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF));
}

#[test]
fn test_logic_not_test() {
    let mut cpu = Cpu::new();

    // NOT: ~0x00FF = 0xFF00 (Flags should not change)
    cpu.set_reg16(iced_x86::Register::AX, 0x00FF);
    cpu.set_cpu_flag(CpuFlags::ZF, true);
    // F7 D0 -> NOT AX
    testrunners::run_cpu_code(&mut cpu, &[0xF7, 0xD0]);
    assert_eq!(cpu.get_reg16(iced_x86::Register::AX), 0xFF00);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF)); // Should remain true

    // TEST: 0x80 & 0x80 = 0x80 (Flags change, result discarded)
    cpu.set_reg8(iced_x86::Register::AL, 0x80);
    // A8 80 -> TEST AL, 0x80
    testrunners::run_cpu_code(&mut cpu, &[0xA8, 0x80]);
    assert_eq!(cpu.get_reg8(iced_x86::Register::AL), 0x80); // Still 0x80
    assert!(cpu.get_cpu_flag(CpuFlags::SF));
    assert!(!cpu.get_cpu_flag(CpuFlags::ZF));
}

#[test]
fn test_shifts_shl_shr_sar() {
    let mut cpu = Cpu::new();

    // SAR Test: 0xFE (-2) >> 1 should be 0xFF (-1)
    cpu.set_reg8(iced_x86::Register::BL, 0xFE);
    
    // D0 FB is SAR BL, 1
    testrunners::run_cpu_code(&mut cpu, &[0xD0, 0xFB]);
    
    assert_eq!(cpu.get_reg8(iced_x86::Register::BL), 0xFF, "SAR 0xFE by 1 failed");
}

#[test]
fn test_rotates_rol_ror_rcl_rcr() {
    let mut cpu = Cpu::new();

    // ROL Test: 0x8001 rotated left 1 = 0x0003 (CF=1)
    cpu.set_reg16(iced_x86::Register::AX, 0x8001);
    testrunners::run_cpu_code(&mut cpu, &[0xD1, 0xC0]); 
    assert_eq!(cpu.get_reg16(iced_x86::Register::AX), 0x0003);

    // RCR Test: 0x01 rotated right through Carry (CF=1) = 0x80 (CF=1)
    cpu.set_reg8(iced_x86::Register::CL, 0x01);
    cpu.set_cpu_flag(CpuFlags::CF, true);
    
    // D0 D9 is RCR CL, 1
    testrunners::run_cpu_code(&mut cpu, &[0xD0, 0xD9]);
    
    assert_eq!(cpu.get_reg8(iced_x86::Register::CL), 0x80, "RCR 0x01 with CF=1 failed");
}

#[test]
fn test_logic_memory_operands() {
    let mut cpu = Cpu::new();
    let addr = 0x1000;
    cpu.bus.write_16(addr, 0xAAAA);
    
    // XOR [0x1000], 0xAAAA -> Sets memory to 0
    // 81 36 00 10 AA AA -> XOR word ptr [0x1000], 0xAAAA
    testrunners::run_cpu_code(&mut cpu, &[0x81, 0x36, 0x00, 0x10, 0xAA, 0xAA]);
    assert_eq!(cpu.bus.read_16(addr), 0x0000);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF));
}

#[test]
fn test_rotates_rcr_only() {
    let mut cpu = Cpu::new();
    // Clear all registers
    cpu.set_reg16(iced_x86::Register::AX, 0);
    cpu.set_reg16(iced_x86::Register::BX, 0);
    cpu.set_reg16(iced_x86::Register::CX, 0);
    cpu.set_reg16(iced_x86::Register::DX, 0);
    
    cpu.set_reg8(iced_x86::Register::CL, 0x01);
    cpu.set_cpu_flag(CpuFlags::CF, true);
    
    // D0 D9 is RCR CL, 1
    testrunners::run_cpu_code(&mut cpu, &[0xD0, 0xD9]); 
    
    let cl_val = cpu.get_reg8(iced_x86::Register::CL);
    let al_val = cpu.get_reg8(iced_x86::Register::AL);
    
    println!("[DEBUG] AL: {}, CL: {}", al_val, cl_val);
    assert_eq!(cl_val, 0x80);
}


#[test]
fn test_test_instr_must_clear_carry_overflow() {
    let mut cpu = Cpu::new();

    // Scenario: Graphics code often checks bits with TEST, then branches.
    // TEST must force CF and OF to zero.

    // 1. Set flags to Dirty state (TRUE)
    cpu.set_cpu_flag(CpuFlags::CF, true);
    cpu.set_cpu_flag(CpuFlags::OF, true);
    cpu.set_reg8(Register::AL, 0xFF);

    // 2. TEST AL, 0x80 (A8 80)
    // Should set SF=1, ZF=0. MUST CLEAR CF and OF.
    testrunners::run_cpu_code(&mut cpu, &[0xA8, 0x80]);

    assert!(cpu.get_cpu_flag(CpuFlags::SF)); // Result is negative
    assert!(!cpu.get_cpu_flag(CpuFlags::CF), "TEST instruction failed to clear Carry Flag");
    assert!(!cpu.get_cpu_flag(CpuFlags::OF), "TEST instruction failed to clear Overflow Flag");
}

#[test]
fn test_logic_imm8_sign_extension() {
    let mut cpu = Cpu::new();

    // SCENARIO: Align AX to even number using AND with -2 (0xFE).
    // Opcode: 83 E0 FE -> AND AX, imm8
    //
    // Correct (Sign Extended): 
    //   Imm8 0xFE (-2) -> 0xFFFE.
    //   0x1235 & 0xFFFE = 0x1234. (Upper byte preserved).
    //
    // Buggy (Zero Extended):
    //   Imm8 0xFE (254) -> 0x00FE.
    //   0x1235 & 0x00FE = 0x0034. (Upper byte DESTROYED).

    cpu.set_reg16(iced_x86::Register::AX, 0x1235);

    // 83 E0 FE -> AND AX, -2
    testrunners::run_cpu_code(&mut cpu, &[0x83, 0xE0, 0xFE]);

    assert_eq!(cpu.get_reg16(iced_x86::Register::AX), 0x1234, 
        "AND AX, imm8 failed sign extension! Upper byte was destroyed.");
}

#[test]
fn test_rep_cx_zero_does_nothing() {
    let mut cpu = Cpu::new();

    // SCENARIO: REP STOSW with CX = 0.
    // Should NOT write to memory. Should NOT decrement DI.
    // If implemented as a do-while loop, it will write once.

    cpu.set_reg16(iced_x86::Register::CX, 0);
    cpu.set_reg16(iced_x86::Register::DI, 0x1000);
    cpu.set_reg16(iced_x86::Register::AX, 0xDEAD);
    cpu.set_reg16(iced_x86::Register::ES, 0x0000);

    // Write "Safe" value to memory
    cpu.bus.write_16(0x1000, 0x0000);

    // F3 AB -> REP STOSW
    testrunners::run_cpu_code(&mut cpu, &[0xF3, 0xAB]);

    assert_eq!(cpu.bus.read_16(0x1000), 0x0000, "REP STOSW executed even though CX was 0!");
    assert_eq!(cpu.get_reg16(iced_x86::Register::DI), 0x1000, "DI should not change if CX is 0");
}

#[test]
fn test_aad_logic() {
    let mut cpu = Cpu::new();

    // SCENARIO: AAD converts unpacked BCD in AH:AL into binary in AL.
    // AL = AL + (AH * base). AH = 0.
    // Standard base is 10 (0x0A).
    
    // AH = 0x02, AL = 0x05.
    // Result should be 2 * 10 + 5 = 25 (0x19).
    cpu.set_reg16(iced_x86::Register::AX, 0x0205);

    // D5 0A -> AAD 10
    testrunners::run_cpu_code(&mut cpu, &[0xD5, 0x0A]);

    assert_eq!(cpu.get_reg16(iced_x86::Register::AX), 0x0019, "AAD failed to convert BCD!");
    assert!(!cpu.get_cpu_flag(CpuFlags::ZF));
    assert!(!cpu.get_cpu_flag(CpuFlags::SF));
    
    // Test Flags (Zero result)
    // AH=0, AL=0 -> Result 0
    cpu.set_reg16(iced_x86::Register::AX, 0x0000);
    testrunners::run_cpu_code(&mut cpu, &[0xD5, 0x0A]);
    assert_eq!(cpu.get_reg16(iced_x86::Register::AX), 0x0000);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF), "AAD failed to set ZF");
}

#[test]
fn test_xlat_segment_override() {
    let mut cpu = Cpu::new();

    // SCENARIO: XLAT with Segment Override (ES:).
    // Instruction: 26 D7 -> XLAT ES:[BX]
    // Default (DS:[BX]): 0xDD
    // Override (ES:[BX]): 0xEE
    
    cpu.set_reg16(iced_x86::Register::BX, 0x0100);
    cpu.set_reg16(iced_x86::Register::DS, 0x1000);
    cpu.set_reg16(iced_x86::Register::ES, 0x2000);
    cpu.set_reg8(iced_x86::Register::AL, 0x02); // Index 2

    // Setup Memory
    // DS:[BX+AL] -> 0x10000 + 0x100 + 2 = 0x10102
    cpu.bus.write_8(0x10102, 0xDD);
    // ES:[BX+AL] -> 0x20000 + 0x100 + 2 = 0x20102
    cpu.bus.write_8(0x20102, 0xEE);

    // 26 D7 -> XLAT ES:[BX]
    testrunners::run_cpu_code(&mut cpu, &[0x26, 0xD7]);

    assert_eq!(cpu.get_reg8(iced_x86::Register::AL), 0xEE, 
        "XLAT ignored Segment Override (read from DS instead of ES)");
}

#[test]
fn test_imul_3_op_sign_extension() {
    let mut cpu = Cpu::new();

    // SCENARIO: IMUL AX, BX, -5
    // Opcode: 6B C3 FB
    // BX = 2.
    // Calculation: 2 * -5 = -10 (0xFFF6).
    //
    // If sign extension fails: 2 * 251 = 502 (0x01F6).
    
    cpu.set_reg16(iced_x86::Register::BX, 2);
    
    testrunners::run_cpu_code(&mut cpu, &[0x6B, 0xC3, 0xFB]);
    
    assert_eq!(cpu.get_reg16(iced_x86::Register::AX), 0xFFF6, 
        "IMUL 3-operand failed sign extension on immediate!");
}

#[test]
fn test_rcl_9bit_rotation() {
    let mut cpu = Cpu::new();

    // SCENARIO: Rotate Through Carry (RCL) behaves like a 9-bit rotate for 8-bit registers.
    // The ring is: [CF] <- [7...0] <- [CF]
    
    // Setup:
    // AL = 0xFF (1111 1111)
    // CF = 0
    // Rotate Left by 1.
    // New CF should be the old MSB (1).
    // New AL should be (0xFF << 1) | Old_CF = 0xFE (1111 1110).
    
    cpu.set_reg8(iced_x86::Register::AL, 0xFF);
    cpu.set_cpu_flag(CpuFlags::CF, false);

    // D0 D0 -> RCL AL, 1
    testrunners::run_cpu_code(&mut cpu, &[0xD0, 0xD0]);

    assert_eq!(cpu.get_reg8(iced_x86::Register::AL), 0xFE);
    assert!(cpu.get_cpu_flag(CpuFlags::CF), "RCL failed to rotate MSB into CF");

    // NOW, the tricky part: Rotate by 9.
    // A 9-bit rotate on a 9-bit ring (8 bits + CF) should be a No-Op (Identity).
    
    cpu.set_reg8(iced_x86::Register::AL, 0x55); // 0101 0101
    cpu.set_cpu_flag(CpuFlags::CF, true);       // 1
    
    // C0 D0 09 -> RCL AL, 9
    testrunners::run_cpu_code(&mut cpu, &[0xC0, 0xD0, 0x09]);

    assert_eq!(cpu.get_reg8(iced_x86::Register::AL), 0x55, "RCL 9-bit identity failed");
    assert!(cpu.get_cpu_flag(CpuFlags::CF), "RCL 9-bit identity failed to preserve CF");
}

#[test]
fn test_shl_overshift_behavior() {
    let mut cpu = Cpu::new();

    // SCENARIO: Shift Left by 16 or more.
    // On 8086, the CPU does NOT mask the count. It literally shifts 16 times.
    // Result should be 0.

    cpu.set_reg16(iced_x86::Register::AX, 0xFFFF);

    // C1 E0 10 -> SHL AX, 16
    testrunners::run_cpu_code(&mut cpu, &[0xC1, 0xE0, 0x10]);

    assert_eq!(cpu.get_reg16(iced_x86::Register::AX), 0x0000, 
        "SHL AX, 16 failed! (Expected 0, likely got wrapped result)");
}

#[test]
fn test_pushf_popf_preserves_direction() {
    let mut cpu = Cpu::new();

    cpu.set_cpu_flag(CpuFlags::DF, true); // Set Direction Flag (Down)
    cpu.set_cpu_flag(CpuFlags::CF, true); // Set Carry Flag

    // 9C -> PUSHF
    testrunners::run_cpu_code(&mut cpu, &[0x9C]);
    
    // Corrupt Flags
    cpu.set_cpu_flag(CpuFlags::DF, false);
    cpu.set_cpu_flag(CpuFlags::CF, false);

    // 9D -> POPF
    testrunners::run_cpu_code(&mut cpu, &[0x9D]);

    assert!(cpu.get_cpu_flag(CpuFlags::DF), "POPF failed to restore Direction Flag!");
    assert!(cpu.get_cpu_flag(CpuFlags::CF), "POPF failed to restore Carry Flag!");
}
