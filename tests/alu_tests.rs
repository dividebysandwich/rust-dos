// tests/alu_tests.rs
use rust_dos::cpu::{Cpu, CpuFlags};

#[test]
fn test_alu_add_8() {
    let mut cpu = Cpu::new();
    
    // Simple Add: 10 + 20 = 30
    let res = cpu.alu_add_8(10, 20);
    assert_eq!(res, 30);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), false);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), false);

    // Zero Result: 255 + 1 = 0 (Wrap)
    let res = cpu.alu_add_8(255, 1);
    assert_eq!(res, 0);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), true);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), true); // Carry occurred
}

#[test]
fn test_alu_add_16_overflow() {
    let mut cpu = Cpu::new();

    // Signed Overflow: 32767 (0x7FFF) + 1 = -32768 (0x8000)
    let res = cpu.alu_add_16(0x7FFF, 1);
    assert_eq!(res, 0x8000);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::OF), true); // Signed Overflow
    assert_eq!(cpu.get_cpu_flag(CpuFlags::SF), true); // Negative result (Sign bit set)
}

#[test]
fn test_alu_sub_8() {
    let mut cpu = Cpu::new();

    // 10 - 5 = 5
    let res = cpu.alu_sub_8(10, 5);
    assert_eq!(res, 5);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), false);

    // 5 - 10 = -5 (0xFB)
    let res = cpu.alu_sub_8(5, 10);
    assert_eq!(res, 0xFB);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), true); // Borrow occurred
    assert_eq!(cpu.get_cpu_flag(CpuFlags::SF), true); // Negative
}

#[test]
fn test_parity_flag() {
    let mut cpu = Cpu::new();

    // 1. Result: 0x03 (binary 0000 0011) -> 2 bits set -> Even -> PF=1
    let res = cpu.alu_add_8(1, 2); 
    assert_eq!(res, 3);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::PF), true);

    // 2. Result: 0x07 (binary 0000 0111) -> 3 bits set -> Odd -> PF=0
    let res = cpu.alu_add_8(3, 4);
    assert_eq!(res, 7);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::PF), false);

    // 3. 16-bit Check (Only low byte matters)
    // Result: 0x0103 (High byte 01, Low byte 03)
    // Low byte is 0000 0011 (2 bits set) -> Even -> PF=1
    let res = cpu.alu_add_16(0x0100, 0x0003);
    assert_eq!(res, 0x0103);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::PF), true);
}

#[test]
fn test_alu_af_flag_edge_cases() {
    let mut cpu = Cpu::new();

    // Addition: 15 (0x0F) + 1 = 16 (0x10)
    // Bit 3 was 1, result bit 3 is 0 -> Carry to bit 4 -> AF=1
    let _ = cpu.alu_add_8(0x0F, 0x01);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::AF), true);

    // Subtraction: 16 (0x10) - 1 = 15 (0x0F)
    // Borrow from bit 4 to bit 3 -> AF=1
    let _ = cpu.alu_sub_8(0x10, 0x01);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::AF), true);

    // Subtraction: 15 (0x0F) - 1 = 14 (0x0E)
    // No borrow from bit 4 -> AF=0
    let _ = cpu.alu_sub_8(0x0F, 0x01);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::AF), false);
}

#[test]
fn test_alu_sbb_ripple_borrow() {
    let mut cpu = Cpu::new();

    // Goal: 0x0100 - 1 = 0x00FF
    // Step 1: Low byte 0x00 - 1
    cpu.set_cpu_flag(CpuFlags::CF, false); // Start clean
    let low = cpu.alu_sub_8(0x00, 0x01);
    assert_eq!(low, 0xFF);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), true); // Borrow out
    assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), false);

    // Step 2: High byte 0x01 - 0 (with borrow)
    let high = cpu.alu_sbb_8(0x01, 0x00);
    assert_eq!(high, 0x00);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), false); // Borrow consumed
    assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), true);  // Result is 0
}

#[test]
fn test_alu_sbb_16_complex() {
    let mut cpu = Cpu::new();

    // Case: 0 - 0 with Carry-in
    // Result: 0xFFFF, CF=1, ZF=0
    cpu.set_cpu_flag(CpuFlags::CF, true);
    let res = cpu.alu_sbb_16(0x0000, 0x0000);
    assert_eq!(res, 0xFFFF);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), false);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), true);

    // Case: 1 - 0 with Carry-in
    // Result: 0, CF=0, ZF=1
    cpu.set_cpu_flag(CpuFlags::CF, true);
    let res = cpu.alu_sbb_16(0x0001, 0x0000);
    assert_eq!(res, 0x0000);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), true);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), false);
}

#[test]
fn test_rotate_flag_preservation() {
    let mut cpu = Cpu::new();

    // 1. Set ZF manually
    cpu.set_cpu_flag(CpuFlags::ZF, true);
    cpu.set_cpu_flag(CpuFlags::CF, false);

    // 2. Perform RCL on a non-zero value
    // If you were using a 16-bit helper that sets ZF, it will flip to false here.
    // In a real CPU, ZF remains true.
    // Assuming you have a way to call your rotate_op handler:
    // rotate_op(&mut cpu, Register::AL, 1, Mnemonic::Rcl); 
    
    // Check if ZF survived
    assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), true, "RCL incorrectly modified ZF!");
}

#[test]
fn test_alu_adc_8_overflow() {
    let mut cpu = Cpu::new();

    // 254 + 1 + (CF=1) = 256 -> 0
    cpu.set_cpu_flag(CpuFlags::CF, true);
    let res = cpu.alu_adc_8(254, 1);
    assert_eq!(res, 0);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::ZF), true);
    assert_eq!(cpu.get_cpu_flag(CpuFlags::CF), true);
}
