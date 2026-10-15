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

/// FLD TBYTE of (sign << 15 | exponent, significand), then `store`, an
/// instruction that stores ST(0) at [1010]; whether it set IE.
fn store_extended(se: u16, man: u64, store: &[u8]) -> (Cpu, bool) {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    cpu.bus.write_64(0x1000, man);
    cpu.bus.write_16(0x1008, se);
    // DB 2E 00 10: FLD TBYTE PTR [1000]
    run_cpu_code(&mut cpu, &[0xDB, 0x2E, 0x00, 0x10]);
    run_cpu_code(&mut cpu, store);
    let ie = cpu.get_fpu_flag(rust_dos::cpu::FpuFlags::IE);
    (cpu, ie)
}

// DF 1E 10 10: FISTP WORD PTR [1010]
const FISTP16: [u8; 4] = [0xDF, 0x1E, 0x10, 0x10];
// DB 1E 10 10: FISTP DWORD PTR [1010]
const FISTP32: [u8; 4] = [0xDB, 0x1E, 0x10, 0x10];
// DF 3E 10 10: FISTP QWORD PTR [1010]
const FISTP64: [u8; 4] = [0xDF, 0x3E, 0x10, 0x10];
// DF 36 10 10: FBSTP TBYTE PTR [1010]
const FBSTP: [u8; 4] = [0xDF, 0x36, 0x10, 0x10];

/// Integers that don't fit (and infinities and NaNs) store the integer
/// indefinite, the smallest negative integer, with IE set, as an x87 with
/// its exceptions masked does (the expected values are an x87's).
#[test]
fn test_fistp_out_of_range_stores_the_indefinite() {
    // (FLD's sign and exponent, its significand, the store, how many bits
    // it stores, and what it stores.)
    let cases: [(u16, u64, &[u8; 4], u32, u64); 8] = [
        (0x400E, 0x9C40_0000_0000_0000, &FISTP16, 16, 0x8000), // 40000
        (0xC00E, 0x9C40_0000_0000_0000, &FISTP16, 16, 0x8000), // -40000
        (0x401E, 0xB2D0_5E00_0000_0000, &FISTP32, 32, 0x8000_0000), // 3e9
        (0x7FFF, 0xC000_0000_0000_0000, &FISTP32, 32, 0x8000_0000), // a NaN
        (0x7FFF, 0x8000_0000_0000_0000, &FISTP16, 16, 0x8000), // infinity
        (0x403F, 0x8000_0000_0000_0000, &FISTP64, 64, 0x8000_0000_0000_0000), // 2^64
        (0x3FFF + 200, 0x8000_0000_0000_0000, &FISTP64, 64, 0x8000_0000_0000_0000), // 2^200
        (0x7FFF, 0xC000_0000_0000_0000, &FISTP64, 64, 0x8000_0000_0000_0000), // a NaN
    ];
    for (se, man, store, bits, want) in cases {
        let (cpu, ie) = store_extended(se, man, store);
        assert_eq!(cpu.bus.read_64(0x1010) & u64::MAX >> (64 - bits), want, "{:04X}:{:016X}", se, man);
        assert!(ie, "IE for {:04X}:{:016X}", se, man);
    }
    // The ones that fit set no IE.
    let (cpu, ie) = store_extended(0xC00E, 0x9C40_0000_0000_0000, &FISTP32);
    assert_eq!(cpu.bus.read_32(0x1010), 0xFFFF_63C0);
    assert!(!ie);
}

/// FISTP QWORD stores the register's 64-bit integer whole: FILD QWORD,
/// FISTP QWORD copies any integer (a double has only 53 bits of it).
#[test]
fn test_fild_fistp_qword_keeps_all_64_bits() {
    for value in [0x1234_5678_90AB_CDEFu64, 0x8000_0000_0000_0000, 0x7FFF_FFFF_FFFF_FFFF, 0xFFFF_FFFF_FFFF_FFFF] {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.write_64(0x1000, value);
        // DF 2E 00 10: FILD QWORD PTR [1000]
        run_cpu_code(&mut cpu, &[0xDF, 0x2E, 0x00, 0x10]);
        run_cpu_code(&mut cpu, &FISTP64);
        assert_eq!(cpu.bus.read_64(0x1010), value);
    }
}

/// FBSTP rounds as the control word says (to nearest here), and stores the
/// packed BCD indefinite for what has more than 18 digits, an infinity or a
/// NaN, with IE set (the expected bytes are an x87's).
#[test]
fn test_fbstp_rounds_and_stores_the_indefinite() {
    let indefinite = [0, 0, 0, 0, 0, 0, 0, 0xC0, 0xFF, 0xFF];
    let cases: [(u16, u64, [u8; 10], bool); 8] = [
        (0x4000, 0xA000_0000_0000_0000, [2, 0, 0, 0, 0, 0, 0, 0, 0, 0], false), // 2.5
        (0x4000, 0xE000_0000_0000_0000, [4, 0, 0, 0, 0, 0, 0, 0, 0, 0], false), // 3.5
        (0xBFFE, 0xC000_0000_0000_0000, [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80], false), // -0.75
        (0x403A, 0xDE0B_6B3A_7640_0000, indefinite, true), // 10^18
        (0x403F, 0x8000_0000_0000_0000, indefinite, true), // 2^64
        (0x3FFF + 200, 0x8000_0000_0000_0000, indefinite, true), // 2^200
        (0x7FFF, 0x8000_0000_0000_0000, indefinite, true), // infinity
        (0x7FFF, 0xC000_0000_0000_0000, indefinite, true), // a NaN
    ];
    for (se, man, want, want_ie) in cases {
        let (cpu, ie) = store_extended(se, man, &FBSTP);
        let got: Vec<u8> = (0..10).map(|i| cpu.bus.read_8(0x1010 + i)).collect();
        assert_eq!(got, want, "{:04X}:{:016X}", se, man);
        assert_eq!(ie, want_ie, "IE for {:04X}:{:016X}", se, man);
    }
}