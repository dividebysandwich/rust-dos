use rust_dos::cpu::{Cpu, CpuFlags};
use iced_x86::Register;

mod testrunners;
use testrunners::run_cpu_code;

#[test]
fn test_register_mapping_8_vs_16() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // 1. Write 16-bit, Read 8-bit
    cpu.set_reg16(Register::AX, 0xABCD);
    assert_eq!(cpu.get_reg8(Register::AH), 0xAB);
    assert_eq!(cpu.get_reg8(Register::AL), 0xCD);

    // 2. Write 8-bit, check 16-bit
    cpu.set_reg8(Register::AL, 0xEF);
    assert_eq!(cpu.get_reg16(Register::AX), 0xABEF); // AH should be unchanged

    cpu.set_reg8(Register::AH, 0x12);
    assert_eq!(cpu.get_reg16(Register::AX), 0x12EF);
}

#[test]
fn test_physical_address_wrapping_20bit() {
    let cpu = Cpu::new(std::path::PathBuf::from("."));

    // Segment: FFFF, Offset: 0010
    // Linear: FFFF0 + 0010 = 100000
    // 20-bit Wrapped: 00000
    let phys = cpu.get_physical_addr(0xFFFF, 0x0010);
    assert_eq!(phys, 0x00000, "Address failed to wrap at 1MB boundary (20-bit)");

    // Standard calc
    let phys_norm = cpu.get_physical_addr(0x1000, 0x0005);
    assert_eq!(phys_norm, 0x10005);
}

#[test]
fn test_alu_add_flags_16bit() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // 1. Simple Add
    cpu.alu_add_16(10, 20);
    assert!(!cpu.get_cpu_flag(CpuFlags::ZF));
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
    assert!(!cpu.get_cpu_flag(CpuFlags::SF));

    // 2. Zero Flag
    cpu.alu_add_16(0, 0);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF));

    // 3. Carry Flag (Unsigned Overflow)
    // 0xFFFF + 1 = 0x0000 (Carry)
    cpu.alu_add_16(0xFFFF, 1);
    assert!(cpu.get_cpu_flag(CpuFlags::CF));
    assert!(cpu.get_cpu_flag(CpuFlags::ZF));

    // 4. Overflow Flag (Signed Overflow)
    // Max Positive (0x7FFF) + 1 = 0x8000 (Negative)
    cpu.alu_add_16(0x7FFF, 1);
    assert!(cpu.get_cpu_flag(CpuFlags::OF), "Signed Overflow not detected");
    assert!(cpu.get_cpu_flag(CpuFlags::SF), "Sign Flag should be set (result 0x8000 is negative)");
    assert!(!cpu.get_cpu_flag(CpuFlags::CF), "Unsigned Carry should NOT be set");

    // 5. Auxiliary Flag (Bit 3 -> 4 carry)
    // 0x0008 + 0x0008 = 0x0010
    cpu.alu_add_16(0x0008, 0x0008);
    assert!(cpu.get_cpu_flag(CpuFlags::AF), "Auxiliary Flag failed");
}

#[test]
fn test_alu_sub_flags_8bit() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // 1. Simple Sub
    let res = cpu.alu_sub_8(10, 3);
    assert_eq!(res, 7);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));

    // 2. Borrow (Carry Flag)
    // 3 - 5 = 254 (0xFE)
    let res = cpu.alu_sub_8(3, 5);
    assert_eq!(res, 0xFE);
    assert!(cpu.get_cpu_flag(CpuFlags::CF), "Borrow (CF) not set for 3 - 5");
    assert!(cpu.get_cpu_flag(CpuFlags::SF), "Sign Flag not set for negative result");

    // 3. Signed Overflow
    // -128 (0x80) - 1 = 127 (0x7F) -> Underflow in signed space
    cpu.alu_sub_8(0x80, 1);
    assert!(cpu.get_cpu_flag(CpuFlags::OF), "Signed Overflow not detected for -128 - 1");
}

#[test]
fn test_alu_adc_carry_in() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // Set CF = 1
    cpu.set_cpu_flag(CpuFlags::CF, true);

    // 10 + 10 + 1(CF) = 21
    let res = cpu.alu_adc_16(10, 10);
    assert_eq!(res, 21);
    
    // Check if CF was updated by the result (21 fits, so CF=0)
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
}

#[test]
fn test_parity_flag_logic() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // Parity is calculated on the LOW byte only, even for 16-bit ops.
    // Even number of bits set = Parity Flag TRUE.

    // 1. Result 0 (0 bits set -> Even) -> PF=1
    cpu.update_pf(0);
    assert!(cpu.get_cpu_flag(CpuFlags::PF));

    // 2. Result 3 (Binary 11 -> 2 bits set -> Even) -> PF=1
    cpu.update_pf(3);
    assert!(cpu.get_cpu_flag(CpuFlags::PF));

    // 3. Result 7 (Binary 111 -> 3 bits set -> Odd) -> PF=0
    cpu.update_pf(7);
    assert!(!cpu.get_cpu_flag(CpuFlags::PF));

    // 4. 16-bit edge case:
    // High byte has bits, Low byte is 0.
    // 0x0100 -> Low byte 0x00 -> Even Parity -> PF=1
    cpu.update_pf(0x0100);
    assert!(cpu.get_cpu_flag(CpuFlags::PF));
}

#[test]
fn test_fpu_stack_rotation() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    use rust_dos::f80::F80;

    // FPU stack is a ring buffer of 8 elements.
    // Pushing decrements TOP. Popping increments TOP.
    
    // Initial TOP = 0.
    
    // Push 1.0 (TOP becomes 7)
    let mut val1 = F80::new();
    val1.set_f64(100.0);
    cpu.fpu_push(val1);
    assert_eq!(cpu.fpu_top, 7);
    
    // Push 2.0 (TOP becomes 6)
    cpu.fpu_push(val1);
    assert_eq!(cpu.fpu_top, 6);

    // Pop (TOP becomes 7)
    cpu.fpu_pop();
    assert_eq!(cpu.fpu_top, 7);
    
    // Pop (TOP becomes 0)
    cpu.fpu_pop();
    assert_eq!(cpu.fpu_top, 0);
}

#[test]
fn test_segment_override_prefix() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // SCENARIO: 
    // Default Access: MOV AX, [BX]      -> Reads from DS:[BX]
    // Override Access: MOV AX, ES:[BX]  -> Reads from ES:[BX]
    
    // 1. Setup different segments
    cpu.set_reg16(Register::DS, 0x1000); // Data Segment
    cpu.set_reg16(Register::ES, 0x2000); // Extra Segment
    cpu.set_reg16(Register::BX, 0x0010); // Offset

    // 2. Write distinct values
    // DS:[BX] -> 0x10010 = 0xDDDD (Data)
    cpu.bus.write_16(0x10010, 0xDDDD);
    
    // ES:[BX] -> 0x20010 = 0xEEEE (Extra)
    cpu.bus.write_16(0x20010, 0xEEEE);

    // 3. Execute Instruction WITH Prefix
    // 26 8B 07 -> MOV AX, ES:[BX]
    // 26 = ES Segment Override Prefix
    run_cpu_code(&mut cpu, &[0x26, 0x8B, 0x07]);

    // 4. Verification
    // If bug exists: It reads 0xDDDD (Default DS)
    // If fixed: It reads 0xEEEE (Override ES)
    assert_eq!(cpu.get_reg16(Register::AX), 0xEEEE, 
        "Segment Override Prefix (ES:) was IGNORED! CPU read from default segment instead.");
}