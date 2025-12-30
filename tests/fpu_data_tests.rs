use rust_dos::cpu::{Cpu, FPU_TAG_EMPTY};
use rust_dos::f80::F80;
mod testrunners;
use testrunners::run_cpu_code;

// Helper to check FPU top value
fn assert_top_f64(cpu: &Cpu, expected: f64) {
    let val = cpu.fpu_get(0).get_f64();
    assert!((val - expected).abs() < 0.0001, "Expected {}, got {}", expected, val);
}

#[test]
fn test_fld_fstp_float32() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let addr = 0x1000;
    
    // Write 123.456 (f32) to memory
    let val_f32 = 123.456f32;
    cpu.bus.write_32(addr, val_f32.to_bits());

    // FLD DWORD PTR [1000] (D9 06 00 10)
    run_cpu_code(&mut cpu, &[0xD9, 0x06, 0x00, 0x10]);

    assert_top_f64(&cpu, 123.456);

    // FSTP DWORD PTR [1004] (D9 1E 04 10)
    run_cpu_code(&mut cpu, &[0xD9, 0x1E, 0x04, 0x10]);

    // Verify memory at [1004]
    let read_back = f32::from_bits(cpu.bus.read_32(addr + 4));
    assert_eq!(read_back, val_f32);
    
    // Stack should be empty after POP
    assert_eq!(cpu.fpu_tags[cpu.fpu_get_phys_index(0)], FPU_TAG_EMPTY);
}

#[test]
fn test_fld_fstp_float64() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let addr = 0x1000;

    // Write double precision PI
    let val_f64 = std::f64::consts::PI;
    cpu.bus.write_64(addr, val_f64.to_bits());

    // FLD QWORD PTR [1000] (DD 06 00 10)
    run_cpu_code(&mut cpu, &[0xDD, 0x06, 0x00, 0x10]);

    assert_top_f64(&cpu, val_f64);

    // FSTP QWORD PTR [1008] (DD 1E 08 10)
    run_cpu_code(&mut cpu, &[0xDD, 0x1E, 0x08, 0x10]);

    // Verify
    let read_back = f64::from_bits(cpu.bus.read_64(addr + 8));
    assert_eq!(read_back, val_f64);
}

#[test]
fn test_fld_fstp_float80() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let addr = 0x1000;

    // Manually construct an 80-bit value (e.g., 1.0)
    // 1.0 = Exp 0x3FFF, Mantissa 0x8000...
    let mut f = F80::new();
    f.set_f64(1.0);
    let bytes_out = f.get_bytes();
    for i in 0..10 { cpu.bus.write_8(addr + i, bytes_out[i]); }

    // FLD TBYTE PTR [1000] (DB 2E 00 10)
    run_cpu_code(&mut cpu, &[0xDB, 0x2E, 0x00, 0x10]);

    assert_top_f64(&cpu, 1.0);

    // Modify stack to 2.0 to ensure FSTP writes something new
    let mut f2 = F80::new(); f2.set_f64(2.0);
    cpu.fpu_set(0, f2);

    // FSTP TBYTE PTR [1010] (DB 3E 10 10)
    run_cpu_code(&mut cpu, &[0xDB, 0x3E, 0x10, 0x10]);

    // Read back from new address
    let mut f_read = F80::new();
    let mut read_bytes = [0u8; 10];
    for i in 0..10 { read_bytes[i] = cpu.bus.read_8(addr + 16 + i); }
    f_read.set_bytes(&read_bytes);
    
    assert_eq!(f_read.get_f64(), 2.0);
}

#[test]
fn test_integer_rounding_modes() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let addr = 0x1000;
    
    // Load 1.6 onto stack
    let mut f = F80::new(); f.set_f64(1.6);
    cpu.fpu_push(f);

    // Default Control Word (0x037F) -> RC=00 (Round to Nearest)
    // FISTP WORD PTR [1000] (DF 1E 00 10)
    // 1.6 should round to 2
    run_cpu_code(&mut cpu, &[0xDF, 0x1E, 0x00, 0x10]);
    assert_eq!(cpu.bus.read_16(addr), 2);

    // Setup for Truncate Mode
    cpu.fpu_push(f); // Push 1.6 again
    // Set RC=11 (Truncate) -> Bits 10,11
    cpu.fpu_control |= 0x0C00; 

    // FISTP WORD PTR [1002]
    run_cpu_code(&mut cpu, &[0xDF, 0x1E, 0x02, 0x10]);
    // 1.6 truncated is 1
    assert_eq!(cpu.bus.read_16(addr + 2), 1);
}

#[test]
fn test_constants_loading() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // D9 E8: FLD1
    run_cpu_code(&mut cpu, &[0xD9, 0xE8]);
    assert_top_f64(&cpu, 1.0);

    // D9 EB: FLDPI
    run_cpu_code(&mut cpu, &[0xD9, 0xEB]);
    assert_top_f64(&cpu, std::f64::consts::PI);
    
    // Stack should have 2 items. ST(0)=PI, ST(1)=1.0
    assert_eq!(cpu.fpu_get(1).get_f64(), 1.0);
}

#[test]
fn test_fxch_swap() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // Push 1.0, then 2.0
    let mut f1 = F80::new(); f1.set_f64(1.0);
    let mut f2 = F80::new(); f2.set_f64(2.0);
    cpu.fpu_push(f1); // ST(1)
    cpu.fpu_push(f2); // ST(0)

    assert_top_f64(&cpu, 2.0);

    // D9 C9: FXCH ST(1)
    run_cpu_code(&mut cpu, &[0xD9, 0xC9]);

    // Now ST(0) should be 1.0
    assert_top_f64(&cpu, 1.0);
    // ST(1) should be 2.0
    assert_eq!(cpu.fpu_get(1).get_f64(), 2.0);
}

#[test]
fn test_fild_integer_load() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let addr = 0x1000;

    // Write -500 (Int16) to memory
    // -500 = 0xFE0C
    cpu.bus.write_16(addr, 0xFE0C);

    // DF 06 00 10: FILD WORD PTR [1000]
    run_cpu_code(&mut cpu, &[0xDF, 0x06, 0x00, 0x10]);

    assert_top_f64(&cpu, -500.0);
}

#[test]
fn test_fbstp_bcd_store() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let addr = 0x1000;

    // Load 123.0 onto stack
    let mut f = F80::new(); f.set_f64(123.0);
    cpu.fpu_push(f);

    // DF 36 00 10: FBSTP TBYTE PTR [1000]
    run_cpu_code(&mut cpu, &[0xDF, 0x36, 0x00, 0x10]);

    // Check BCD Bytes. 123 -> 23 01 00 ...
    // Packed BCD: 2 digits per byte. Little Endian.
    // Byte 0: 0x23
    // Byte 1: 0x01
    // Byte 9: Sign (0x00 or 0x0A for +?) - F80 implementation specific
    // Usually 0x23, 0x01, 0x00...
    
    let b0 = cpu.bus.read_8(addr);
    let b1 = cpu.bus.read_8(addr + 1);
    
    assert_eq!(b0, 0x23);
    assert_eq!(b1, 0x01);
}