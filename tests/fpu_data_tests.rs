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
fn test_undocumented_fstp_and_ffreep() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    for value in [1.0, 2.0, 3.0] {
        let mut f = F80::new();
        f.set_f64(value);
        cpu.fpu_push(f);
    }

    // D9 D9: FSTP ST(1) in its undocumented form (TFX pops with D9 D8)
    run_cpu_code(&mut cpu, &[0xD9, 0xD9]);
    assert_top_f64(&cpu, 3.0);
    assert_eq!(cpu.fpu_get(1).get_f64(), 1.0);

    // DF C0: FFREEP ST(0), free and pop
    run_cpu_code(&mut cpu, &[0xDF, 0xC0]);
    assert_top_f64(&cpu, 1.0);
    assert_eq!(cpu.fpu_tags[cpu.fpu_get_phys_index(7)], FPU_TAG_EMPTY);
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
    // Byte 9: Sign in bit 7, 0x00 for +
    
    let b0 = cpu.bus.read_8(addr);
    let b1 = cpu.bus.read_8(addr + 1);
    let b9 = cpu.bus.read_8(addr + 9);
    
    assert_eq!(b0, 0x23);
    assert_eq!(b1, 0x01);
    assert_eq!(b9, 0x00);
}

#[test]
fn test_fbld_bcd_load() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let addr = 0x1000;

    // -1234567890 as packed BCD: sign in bit 7 of byte 9
    let bcd = [0x90, 0x78, 0x56, 0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x80];
    for (i, b) in bcd.iter().enumerate() {
        cpu.bus.write_8(addr + i, *b);
    }

    // DF 26 00 10: FBLD TBYTE PTR [1000]
    run_cpu_code(&mut cpu, &[0xDF, 0x26, 0x00, 0x10]);

    assert_top_f64(&cpu, -1234567890.0);
}

#[test]
fn test_fbld_fbstp_round_trip() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let addr = 0x1000;

    // -999999999999999999: 18 digits, more than an f64 holds exactly
    let bcd = [0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x99, 0x80];
    for (i, b) in bcd.iter().enumerate() {
        cpu.bus.write_8(addr + i, *b);
    }

    // DF 26 00 10: FBLD TBYTE PTR [1000]
    run_cpu_code(&mut cpu, &[0xDF, 0x26, 0x00, 0x10]);
    // DF 36 10 10: FBSTP TBYTE PTR [1010]
    run_cpu_code(&mut cpu, &[0xDF, 0x36, 0x10, 0x10]);

    for (i, b) in bcd.iter().enumerate() {
        assert_eq!(cpu.bus.read_8(addr + 0x10 + i), *b, "byte {}", i);
    }
    assert_eq!(cpu.fpu_tags[cpu.fpu_get_phys_index(0)], FPU_TAG_EMPTY);
}

/// 80-bit values stored as doubles round to nearest, ties to even, as an
/// x87 with its default control word stores them (the expected doubles are
/// what FLD TBYTE, FSTP QWORD gave on an x87).
#[test]
fn test_fstp_float64_rounds_to_nearest() {
    let cases: [(u16, u64, u64); 7] = [
        (0x3FFF, 0x8000_0000_0000_0001, 0x3FF0_0000_0000_0000), // 1 + 2^-63
        (0x3FFF, 0x8000_0000_0000_0400, 0x3FF0_0000_0000_0000), // a tie, even below
        (0x3FFF, 0x8000_0000_0000_0C00, 0x3FF0_0000_0000_0002), // a tie, odd below
        (0x3FFF, 0x8000_0000_0000_0401, 0x3FF0_0000_0000_0001), // just over a tie
        (0x3FFF, 0xFFFF_FFFF_FFFF_FFFF, 0x4000_0000_0000_0000), // up into 2
        (0x43FE, 0xFFFF_FFFF_FFFF_FFFF, 0x7FF0_0000_0000_0000), // up past the largest
        (0xC005, 0xA5A5_A5A5_A5A5_A5A5, 0xC054_B4B4_B4B4_B4B5), // -41.29...
    ];
    for (se, man, want) in cases {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.write_64(0x1000, man);
        cpu.bus.write_16(0x1008, se);
        // DB 2E 00 10: FLD TBYTE PTR [1000]
        run_cpu_code(&mut cpu, &[0xDB, 0x2E, 0x00, 0x10]);
        // DD 1E 10 10: FSTP QWORD PTR [1010]
        run_cpu_code(&mut cpu, &[0xDD, 0x1E, 0x10, 0x10]);
        assert_eq!(cpu.bus.read_64(0x1010), want, "{:04X}:{:016X}", se, man);
    }
}

/// 0.1 + 0.2 adds in 80 bits exactly and rounds up when stored as a
/// double: 0.30000000000000004 on an x87.
#[test]
fn test_sum_stored_as_double_rounds_to_nearest() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.bus.write_64(0x1000, 0.1f64.to_bits());
    cpu.bus.write_64(0x1008, 0.2f64.to_bits());
    // DD 06 00 10: FLD QWORD PTR [1000]
    run_cpu_code(&mut cpu, &[0xDD, 0x06, 0x00, 0x10]);
    // DC 06 08 10: FADD QWORD PTR [1008]
    run_cpu_code(&mut cpu, &[0xDC, 0x06, 0x08, 0x10]);
    // DD 1E 10 10: FSTP QWORD PTR [1010]
    run_cpu_code(&mut cpu, &[0xDD, 0x1E, 0x10, 0x10]);
    assert_eq!(cpu.bus.read_64(0x1010), 0x3FD3_3333_3333_3334);
}

/// FISTP QWORD of 3 - 2^-60 under chop stores 2, as an x87 does: from the
/// 80 bits, not from their double (3.0).
#[test]
fn test_fistp_int64_chops_the_80_bits() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.bus.write_64(0x1000, 0xBFFF_FFFF_FFFF_FFF0);
    cpu.bus.write_16(0x1008, 0x4000);
    cpu.bus.write_16(0x1020, 0x0F7F);
    // D9 2E 20 10: FLDCW [1020]
    run_cpu_code(&mut cpu, &[0xD9, 0x2E, 0x20, 0x10]);
    // DB 2E 00 10: FLD TBYTE PTR [1000]
    run_cpu_code(&mut cpu, &[0xDB, 0x2E, 0x00, 0x10]);
    // DF 3E 10 10: FISTP QWORD PTR [1010]
    run_cpu_code(&mut cpu, &[0xDF, 0x3E, 0x10, 0x10]);
    assert_eq!(cpu.bus.read_64(0x1010), 2);
}
/// FISTP of a value that doesn't fit, or of a NaN, is an invalid operation:
/// with it masked the FPU stores the integer indefinite and sets IE.
#[test]
fn test_fistp_stores_the_integer_indefinite() {
    use rust_dos::cpu::FpuFlags;
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let store = |cpu: &mut Cpu, value: f64, code: &[u8]| {
        cpu.set_fpu_flags(FpuFlags::empty());
        let mut f = F80::new();
        f.set_f64(value);
        cpu.fpu_push(f);
        run_cpu_code(cpu, code);
        cpu.get_fpu_flags().contains(FpuFlags::IE)
    };
    // DF 1E 00 10: FISTP WORD PTR [1000]
    assert!(store(&mut cpu, 40000.0, &[0xDF, 0x1E, 0x00, 0x10]));
    assert_eq!(cpu.bus.read_16(0x1000), 0x8000);
    assert!(!store(&mut cpu, -32768.0, &[0xDF, 0x1E, 0x00, 0x10]));
    assert_eq!(cpu.bus.read_16(0x1000), 0x8000);
    assert!(!store(&mut cpu, -1.0, &[0xDF, 0x1E, 0x00, 0x10]));
    assert_eq!(cpu.bus.read_16(0x1000), 0xFFFF);
    // DB 1E 04 10: FISTP DWORD PTR [1004]
    assert!(store(&mut cpu, 3e9, &[0xDB, 0x1E, 0x04, 0x10]));
    assert_eq!(cpu.bus.read_32(0x1004), 0x8000_0000);
    // DF 3E 08 10: FISTP QWORD PTR [1008]
    assert!(store(&mut cpu, f64::NAN, &[0xDF, 0x3E, 0x08, 0x10]));
    assert_eq!(cpu.bus.read_64(0x1008), 0x8000_0000_0000_0000);
    assert!(store(&mut cpu, f64::INFINITY, &[0xDF, 0x3E, 0x08, 0x10]));
    assert!(!store(&mut cpu, -9.223372036854775808e18, &[0xDF, 0x3E, 0x08, 0x10]));
    assert_eq!(cpu.bus.read_64(0x1008), 0x8000_0000_0000_0000);
}

/// -(2^63 + 1) doesn't fit a qword, though its double (-2^63) does.
#[test]
fn test_fistp_int64_past_its_range_from_the_80_bits() {
    use rust_dos::cpu::FpuFlags;
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.bus.write_64(0x1000, 0x8000_0000_0000_0001);
    cpu.bus.write_16(0x1008, 0xC000 | (16383 + 63));
    cpu.set_fpu_flags(FpuFlags::empty());
    // DB 2E 00 10: FLD TBYTE PTR [1000]
    run_cpu_code(&mut cpu, &[0xDB, 0x2E, 0x00, 0x10]);
    // DF 3E 10 10: FISTP QWORD PTR [1010]
    run_cpu_code(&mut cpu, &[0xDF, 0x3E, 0x10, 0x10]);
    assert_eq!(cpu.bus.read_64(0x1010), 0x8000_0000_0000_0000);
    assert!(cpu.get_fpu_flags().contains(FpuFlags::IE));
}
