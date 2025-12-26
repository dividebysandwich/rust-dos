// tests/alu_tests.rs
use rust_dos::cpu::{Cpu, FLAG_ZF, FLAG_SF, FLAG_CF, FLAG_OF, FLAG_PF};

#[test]
fn test_alu_add_8() {
    let mut cpu = Cpu::new();
    
    // Simple Add: 10 + 20 = 30
    let res = cpu.alu_add_8(10, 20);
    assert_eq!(res, 30);
    assert_eq!(cpu.get_flag(FLAG_ZF), false);
    assert_eq!(cpu.get_flag(FLAG_CF), false);

    // Zero Result: 255 + 1 = 0 (Wrap)
    let res = cpu.alu_add_8(255, 1);
    assert_eq!(res, 0);
    assert_eq!(cpu.get_flag(FLAG_ZF), true);
    assert_eq!(cpu.get_flag(FLAG_CF), true); // Carry occurred
}

#[test]
fn test_alu_add_16_overflow() {
    let mut cpu = Cpu::new();

    // Signed Overflow: 32767 (0x7FFF) + 1 = -32768 (0x8000)
    let res = cpu.alu_add_16(0x7FFF, 1);
    assert_eq!(res, 0x8000);
    assert_eq!(cpu.get_flag(FLAG_OF), true); // Signed Overflow
    assert_eq!(cpu.get_flag(FLAG_SF), true); // Negative result (Sign bit set)
}

#[test]
fn test_alu_sub_8() {
    let mut cpu = Cpu::new();

    // 10 - 5 = 5
    let res = cpu.alu_sub_8(10, 5);
    assert_eq!(res, 5);
    assert_eq!(cpu.get_flag(FLAG_ZF), false);

    // 5 - 10 = -5 (0xFB)
    let res = cpu.alu_sub_8(5, 10);
    assert_eq!(res, 0xFB);
    assert_eq!(cpu.get_flag(FLAG_CF), true); // Borrow occurred
    assert_eq!(cpu.get_flag(FLAG_SF), true); // Negative
}

#[test]
fn test_parity_flag() {
    let mut cpu = Cpu::new();

    // 1. Result: 0x03 (binary 0000 0011) -> 2 bits set -> Even -> PF=1
    let res = cpu.alu_add_8(1, 2); 
    assert_eq!(res, 3);
    assert_eq!(cpu.get_flag(FLAG_PF), true);

    // 2. Result: 0x07 (binary 0000 0111) -> 3 bits set -> Odd -> PF=0
    let res = cpu.alu_add_8(3, 4);
    assert_eq!(res, 7);
    assert_eq!(cpu.get_flag(FLAG_PF), false);

    // 3. 16-bit Check (Only low byte matters)
    // Result: 0x0103 (High byte 01, Low byte 03)
    // Low byte is 0000 0011 (2 bits set) -> Even -> PF=1
    let res = cpu.alu_add_16(0x0100, 0x0003);
    assert_eq!(res, 0x0103);
    assert_eq!(cpu.get_flag(FLAG_PF), true);
}
