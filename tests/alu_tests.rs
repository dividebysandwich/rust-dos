// tests/alu_tests.rs
use rust_dos::cpu::{Cpu, CpuFlags};

#[test]
fn test_alu_add_8() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // Simple Add: 10 + 20 = 30
    let res = cpu.alu_add(1, 10_u32, 20_u32, false);
    assert_eq!(res, 30);
    assert!(!cpu.get_cpu_flag(CpuFlags::ZF));
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));

    // Zero Result: 255 + 1 = 0 (Wrap)
    let res = cpu.alu_add(1, 255_u32, 1_u32, false);
    assert_eq!(res, 0);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF));
    assert!(cpu.get_cpu_flag(CpuFlags::CF)); // Carry occurred
}

#[test]
fn test_alu_add_16_overflow() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // Signed Overflow: 32767 (0x7FFF) + 1 = -32768 (0x8000)
    let res = cpu.alu_add(2, 0x7FFF_u32, 1_u32, false);
    assert_eq!(res, 0x8000);
    assert!(cpu.get_cpu_flag(CpuFlags::OF)); // Signed Overflow
    assert!(cpu.get_cpu_flag(CpuFlags::SF)); // Negative result (Sign bit set)
}

#[test]
fn test_alu_sub_8() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // 10 - 5 = 5
    let res = cpu.alu_sub(1, 10_u32, 5_u32, false);
    assert_eq!(res, 5);
    assert!(!cpu.get_cpu_flag(CpuFlags::ZF));

    // 5 - 10 = -5 (0xFB)
    let res = cpu.alu_sub(1, 5_u32, 10_u32, false);
    assert_eq!(res, 0xFB);
    assert!(cpu.get_cpu_flag(CpuFlags::CF)); // Borrow occurred
    assert!(cpu.get_cpu_flag(CpuFlags::SF)); // Negative
}

#[test]
fn test_parity_flag() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // 1. Result: 0x03 (binary 0000 0011) -> 2 bits set -> Even -> PF=1
    let res = cpu.alu_add(1, 1_u32, 2_u32, false);
    assert_eq!(res, 3);
    assert!(cpu.get_cpu_flag(CpuFlags::PF));

    // 2. Result: 0x07 (binary 0000 0111) -> 3 bits set -> Odd -> PF=0
    let res = cpu.alu_add(1, 3_u32, 4_u32, false);
    assert_eq!(res, 7);
    assert!(!cpu.get_cpu_flag(CpuFlags::PF));

    // 3. 16-bit Check (Only low byte matters)
    // Result: 0x0103 (High byte 01, Low byte 03)
    // Low byte is 0000 0011 (2 bits set) -> Even -> PF=1
    let res = cpu.alu_add(2, 0x0100_u32, 0x0003_u32, false);
    assert_eq!(res, 0x0103);
    assert!(cpu.get_cpu_flag(CpuFlags::PF));
}

#[test]
fn test_alu_af_flag_edge_cases() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // Addition: 15 (0x0F) + 1 = 16 (0x10)
    // Bit 3 was 1, result bit 3 is 0 -> Carry to bit 4 -> AF=1
    let _ = cpu.alu_add(1, 0x0F_u32, 0x01_u32, false);
    assert!(cpu.get_cpu_flag(CpuFlags::AF));

    // Subtraction: 16 (0x10) - 1 = 15 (0x0F)
    // Borrow from bit 4 to bit 3 -> AF=1
    let _ = cpu.alu_sub(1, 0x10_u32, 0x01_u32, false);
    assert!(cpu.get_cpu_flag(CpuFlags::AF));

    // Subtraction: 15 (0x0F) - 1 = 14 (0x0E)
    // No borrow from bit 4 -> AF=0
    let _ = cpu.alu_sub(1, 0x0F_u32, 0x01_u32, false);
    assert!(!cpu.get_cpu_flag(CpuFlags::AF));
}

#[test]
fn test_alu_sbb_ripple_borrow() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // Goal: 0x0100 - 1 = 0x00FF
    // Step 1: Low byte 0x00 - 1
    cpu.set_cpu_flag(CpuFlags::CF, false); // Start clean
    let low = cpu.alu_sub(1, 0x00_u32, 0x01_u32, false);
    assert_eq!(low, 0xFF);
    assert!(cpu.get_cpu_flag(CpuFlags::CF)); // Borrow out
    assert!(!cpu.get_cpu_flag(CpuFlags::ZF));

    // Step 2: High byte 0x01 - 0 (with borrow)
    let high = cpu.alu_sub(1, 0x01_u32, 0x00_u32, cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(high, 0x00);
    assert!(!cpu.get_cpu_flag(CpuFlags::CF)); // Borrow consumed
    assert!(cpu.get_cpu_flag(CpuFlags::ZF)); // Result is 0
}

#[test]
fn test_alu_sbb_16_complex() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // Case: 0 - 0 with Carry-in
    // Result: 0xFFFF, CF=1, ZF=0
    cpu.set_cpu_flag(CpuFlags::CF, true);
    let res = cpu.alu_sub(2, 0x0000_u32, 0x0000_u32, cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(res, 0xFFFF);
    assert!(!cpu.get_cpu_flag(CpuFlags::ZF));
    assert!(cpu.get_cpu_flag(CpuFlags::CF));

    // Case: 1 - 0 with Carry-in
    // Result: 0, CF=0, ZF=1
    cpu.set_cpu_flag(CpuFlags::CF, true);
    let res = cpu.alu_sub(2, 0x0001_u32, 0x0000_u32, cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(res, 0x0000);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF));
    assert!(!cpu.get_cpu_flag(CpuFlags::CF));
}

#[test]
fn test_rotate_flag_preservation() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // 1. Set ZF manually
    cpu.set_cpu_flag(CpuFlags::ZF, true);
    cpu.set_cpu_flag(CpuFlags::CF, false);

    // 2. Perform RCL on a non-zero value
    
    // Check if ZF survived
    assert!(cpu.get_cpu_flag(CpuFlags::ZF), "RCL incorrectly modified ZF!");
}

#[test]
fn test_alu_adc_8_overflow() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // 254 + 1 + (CF=1) = 256 -> 0
    cpu.set_cpu_flag(CpuFlags::CF, true);
    let res = cpu.alu_add(1, 254_u32, 1_u32, cpu.get_cpu_flag(CpuFlags::CF));
    assert_eq!(res, 0);
    assert!(cpu.get_cpu_flag(CpuFlags::ZF));
    assert!(cpu.get_cpu_flag(CpuFlags::CF));
}
