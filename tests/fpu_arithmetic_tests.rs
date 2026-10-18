use rust_dos::cpu::{Cpu, FpuFlags};
use rust_dos::f80::F80;

mod testrunners;
use testrunners::run_cpu_code;

#[test]
fn test_fpu_arithmetic_matrix() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // Setup: ST(1) = 100.0, ST(0) = 20.0
    let mut f100 = F80::new(); f100.set_f64(100.0);
    let mut f20 = F80::new(); f20.set_f64(20.0);

    let reset_stack = |c: &mut Cpu, val0: F80, val1: F80| {
        while c.fpu_top != 0 { c.fpu_pop(); } // Clear stack
        c.fpu_push(val1); // ST(1)
        c.fpu_push(val0); // ST(0)
    };

    // --- 1. ADDITION ---
    reset_stack(&mut cpu, f20, f100);
    testrunners::run_fpu_code(&mut cpu, &[0xD8, 0xC1]); // FADD ST(0), ST(1) -> ST(0)=120
    assert_eq!(cpu.fpu_get(0).get_f64(), 120.0);

    reset_stack(&mut cpu, f20, f100);
    testrunners::run_fpu_code(&mut cpu, &[0xDC, 0xC1]); // FADD ST(1), ST(0) -> ST(1)=120
    assert_eq!(cpu.fpu_get(1).get_f64(), 120.0);

    // --- 2. SUBTRACTION (The "08" Killer) ---
    reset_stack(&mut cpu, f20, f100);
    testrunners::run_fpu_code(&mut cpu, &[0xD8, 0xE1]); // FSUB ST(0), ST(1) -> 20 - 100 = -80
    assert_eq!(cpu.fpu_get(0).get_f64(), -80.0);

    reset_stack(&mut cpu, f20, f100);
    testrunners::run_fpu_code(&mut cpu, &[0xDC, 0xE9]); // FSUB ST(1), ST(0) -> 100 - 20 = 80
    assert_eq!(cpu.fpu_get(1).get_f64(), 80.0);

    // --- 3. REVERSE SUBTRACTION ---
    reset_stack(&mut cpu, f20, f100);
    testrunners::run_fpu_code(&mut cpu, &[0xD8, 0xE9]); // FSUBR ST(0), ST(1) -> 100 - 20 = 80
    assert_eq!(cpu.fpu_get(0).get_f64(), 80.0);

    reset_stack(&mut cpu, f20, f100);
    testrunners::run_fpu_code(&mut cpu, &[0xDC, 0xE1]); // FSUBR ST(1), ST(0) -> 20 - 100 = -80
    assert_eq!(cpu.fpu_get(1).get_f64(), -80.0);

    // --- 4. DIVISION ---
    reset_stack(&mut cpu, f20, f100);
    testrunners::run_fpu_code(&mut cpu, &[0xD8, 0xF1]); // FDIV ST(0), ST(1) -> 20 / 100 = 0.2
    assert_eq!(cpu.fpu_get(0).get_f64(), 0.2);

    reset_stack(&mut cpu, f20, f100);
    testrunners::run_fpu_code(&mut cpu, &[0xDC, 0xF9]); // FDIV ST(1), ST(0) -> 100 / 20 = 5.0
    assert_eq!(cpu.fpu_get(1).get_f64(), 5.0);

    // --- 5. REVERSE DIVISION ---
    reset_stack(&mut cpu, f20, f100);
    testrunners::run_fpu_code(&mut cpu, &[0xD8, 0xF9]); // FDIVR ST(0), ST(1) -> 100 / 20 = 5.0
    assert_eq!(cpu.fpu_get(0).get_f64(), 5.0);
}

#[test]
fn test_fadd_and_fsub_real() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let mut f1 = F80::new(); f1.set_f64(10.5);
    let mut f2 = F80::new(); f2.set_f64(2.5);
    
    cpu.fpu_push(f1); // ST(1) = 10.5
    cpu.fpu_push(f2); // ST(0) = 2.5

    // FADD ST(1), ST(0) -> 0xDC C1
    // Dest: ST(1)
    testrunners::run_fpu_code(&mut cpu, &[0xDC, 0xC1]);
    
    // ST(0) remains 2.5
    assert_eq!(cpu.fpu_get(0).get_f64(), 2.5);
    // ST(1) becomes 13.0
    assert_eq!(cpu.fpu_get(1).get_f64(), 13.0);
}

#[test]
fn test_faddp_behavior() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let mut f1 = F80::new(); f1.set_f64(5.0);
    let mut f2 = F80::new(); f2.set_f64(1.0);
    cpu.fpu_push(f1);
    cpu.fpu_push(f2); // ST(0)=1, ST(1)=5

    // FADDP ST(1), ST(0) -> DE C1
    testrunners::run_fpu_code(&mut cpu, &[0xDE, 0xC1]);

    // Result should be 6.0 and ST(0) should be popped
    assert_eq!(cpu.fpu_get(0).get_f64(), 6.0);
    // Check if tag of old ST(1) (now ST(0)) is valid and previous is empty
    assert_eq!(cpu.fpu_top, 7); // Pushed twice (-2), Popped once (+1) = 7 (mod 8)
}

#[test]
fn test_fiadd_integer_memory() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let mut f0 = F80::new(); f0.set_f64(100.0);
    cpu.fpu_push(f0);

    // Setup memory: [0x2000] = 50 (Int32)
    let addr = 0x2000;
    cpu.bus.write_32(addr, 50);

    // DA 06 00 20 is FIADD dword [2000] in 16-bit mode
    testrunners::run_fpu_code(&mut cpu, &[0xDA, 0x06, 0x00, 0x20]);

    assert_eq!(cpu.fpu_get(0).get_f64(), 150.0);
}

#[test]
fn test_fsqrt_and_invalid_op() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // 1. Valid Case: sqrt(16.0) = 4.0
    let mut f16 = F80::new(); 
    f16.set_f64(16.0);
    cpu.fpu_push(f16);
    
    // D9 FA: FSQRT
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xFA]); 
    
    assert_eq!(cpu.fpu_get(0).get_f64(), 4.0);
    // Ensure Invalid Operation (IE) is NOT set
    assert!(!cpu.get_fpu_flags().contains(FpuFlags::IE));

    // 2. Invalid Case: sqrt(-1.0) -> IE Exception
    let mut fneg = F80::new(); 
    fneg.set_f64(-1.0);
    cpu.fpu_push(fneg);
    
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xFA]);
    
    // Result should be Real Indefinite (NaN)
    assert!(cpu.fpu_get(0).is_nan());
    // Ensure Invalid Operation (IE) IS set
    assert!(cpu.get_fpu_flags().contains(FpuFlags::IE));
}

#[test]
fn test_fprem_partial_remainder() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // 10.0 / 3.0 -> remainder 1.0 (Quotient = 3)
    let mut f3 = F80::new(); f3.set_f64(3.0);
    let mut f10 = F80::new(); f10.set_f64(10.0);
    cpu.fpu_push(f3);   // ST(1)
    cpu.fpu_push(f10);  // ST(0)

    // D9 F8: FPREM
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xF8]); 

    assert_eq!(cpu.fpu_get(0).get_f64(), 1.0);

    // Quotient Q = 3 (Binary 011)
    // Bit 2 (4) -> C0: 0
    // Bit 1 (2) -> C3: 1
    // Bit 0 (1) -> C1: 1
    let flags = cpu.get_fpu_flags();
    
    assert!(flags.contains(FpuFlags::C3), "C3 should be set (Q bit 1 is 1)");
    assert!(flags.contains(FpuFlags::C1), "C1 should be set (Q bit 0 is 1)");
    assert!(!flags.contains(FpuFlags::C0), "C0 should be clear (Q bit 2 is 0)");
}

#[test]
fn test_fabs_fchs() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let mut f_neg = F80::new(); f_neg.set_f64(-5.5);
    cpu.fpu_push(f_neg);

    // FABS
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xE1]);
    assert_eq!(cpu.fpu_get(0).get_f64(), 5.5);
    assert!(!cpu.fpu_get(0).get_sign());

    // FCHS (change back to negative)
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xE0]);
    assert_eq!(cpu.fpu_get(0).get_f64(), -5.5);
    assert!(cpu.fpu_get(0).get_sign());
}

#[test]
fn test_f2xm1_exponentiation() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // Test 2^0.5 - 1
    // 0.5 is within the required range (-1.0 to 1.0) for F2XM1
    let mut f_half = F80::new(); f_half.set_f64(0.5);
    cpu.fpu_push(f_half);

    // D9 F0: F2XM1
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xF0]);

    let result = cpu.fpu_get(0).get_f64();
    let expected = 2.0f64.powf(0.5) - 1.0;
    
    // Check with small epsilon for float precision
    assert!((result - expected).abs() < 1e-12);
}

#[test]
fn test_fyl2x_logarithm() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // Calculate 3 * log2(8) = 3 * 3 = 9
    let mut f_y = F80::new(); f_y.set_f64(3.0);
    let mut f_x = F80::new(); f_x.set_f64(8.0);
    
    cpu.fpu_push(f_y); // ST(1)
    cpu.fpu_push(f_x); // ST(0)

    // D9 F1: FYL2X (Result in ST(1), Pops ST(0))
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xF1]);

    assert_eq!(cpu.fpu_get(0).get_f64(), 9.0);
}

#[test]
fn test_fxtract_decomposition() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let mut f12 = F80::new(); f12.set_f64(12.0); // 1.5 * 2^3
    cpu.fpu_push(f12);

    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xF4]);

    assert_eq!(cpu.fpu_get(0).get_f64(), 1.5);
    assert_eq!(cpu.fpu_get(1).get_f64(), 3.0);
}

#[test]
fn test_fscale_powers_of_two() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let mut f_exp = F80::new(); f_exp.set_f64(2.0);
    let mut f_val = F80::new(); f_val.set_f64(3.0);
    
    cpu.fpu_push(f_exp);
    cpu.fpu_push(f_val);

    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xFD]);

    assert_eq!(cpu.fpu_get(0).get_f64(), 12.0);
}

#[test]
fn test_fadd_diagnostic() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let mut f1 = F80::new(); f1.set_f64(10.5);
    let mut f2 = F80::new(); f2.set_f64(2.5);
    
    cpu.fpu_push(f1); 
    println!("After Push 1: TOP={}, Tags={:?}", cpu.fpu_top, cpu.fpu_tags);
    
    cpu.fpu_push(f2); 
    println!("After Push 2: TOP={}, Tags={:?}", cpu.fpu_top, cpu.fpu_tags);

    // FADD ST(1), ST(0)
    testrunners::run_fpu_code(&mut cpu, &[0xDC, 0xC1]);

    println!("After FADD: TOP={}, ST(0)={:?}, ST(1)={:?}", 
        cpu.fpu_top, cpu.fpu_get(0).get_f64(), cpu.fpu_get(1).get_f64());
    
    for i in 0..8 {
        println!("Phys Reg {}: {:?} (Tag: {})", i, cpu.fpu_reg(i).get_f64(), cpu.fpu_tags[i]);
    }

    assert_eq!(cpu.fpu_get(1).get_f64(), 13.0);
}

#[test]
fn test_fsub_variants() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let mut f10 = F80::new(); f10.set_f64(10.0);
    let mut f2 = F80::new(); f2.set_f64(2.0);
    cpu.fpu_push(f10); // ST(1)
    cpu.fpu_push(f2);  // ST(0)

    // Variant 1: D8 E1 -> FSUB ST(0), ST(1) 
    // ST(0) = 2.0 - 10.0 = -8.0
    testrunners::run_fpu_code(&mut cpu, &[0xD8, 0xE1]);
    assert_eq!(cpu.fpu_get(0).get_f64(), -8.0);

    // Variant 2: DC E9 -> FSUB ST(1), ST(0)
    // Resetting for test...
    cpu.fpu_set(0, f2); 
    cpu.fpu_set(1, f10);
    // ST(1) = 10.0 - 2.0 = 8.0
    testrunners::run_fpu_code(&mut cpu, &[0xDC, 0xE9]);
    assert_eq!(cpu.fpu_get(1).get_f64(), 8.0);
}

fn push_val(cpu: &mut Cpu, val: f64) {
    let mut f = F80::new();
    f.set_f64(val);
    cpu.fpu_push(f);
}

#[test]
fn test_fsubp_st2_st0() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    // Setup: ST(0)=1.0, ST(1)=10.0, ST(2)=100.0
    push_val(&mut cpu, 100.0);
    push_val(&mut cpu, 10.0);
    push_val(&mut cpu, 1.0);

    // DE EA: FSUBP ST(2), ST(0)
    // ST(2) = ST(2) - ST(0) -> 100.0 - 1.0 = 99.0
    // Pop ST(0)
    // New Stack: ST(0)=10.0, ST(1)=99.0
    testrunners::run_fpu_code(&mut cpu, &[0xDE, 0xEA]);

    assert_eq!(cpu.fpu_get(0).get_f64(), 10.0, "ST(1) should become ST(0)");
    assert_eq!(cpu.fpu_get(1).get_f64(), 99.0, "ST(2) should have been modified and become ST(1)");
}

#[test]
fn test_trigonometry() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    
    // FSIN of PI/2
    push_val(&mut cpu, std::f64::consts::FRAC_PI_2);
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xFE]); // FSIN
    let val = cpu.fpu_get(0).get_f64();
    assert!((val - 1.0).abs() < 0.0001);

    // FPTAN of PI/4 (tan = 1.0)
    // Should result in ST(0)=1.0, ST(1)=1.0 (It pushes 1.0 after calc)
    push_val(&mut cpu, std::f64::consts::FRAC_PI_4);
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xF2]); // FPTAN
    
    assert!((cpu.fpu_get(0).get_f64() - 1.0).abs() < 0.0001); // The pushed 1.0
    assert!((cpu.fpu_get(1).get_f64() - 1.0).abs() < 0.0001); // The result tan(pi/4)
}

/// FRNDINT rounds halves to even, sets PE where the result is inexact, and
/// C1 where its magnitude went up.
#[test]
fn test_frndint_rounds_to_even_and_reports_the_rounding() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    for (value, rounded, up) in [(2.5, 2.0, false), (3.5, 4.0, true), (2.6, 3.0, true), (-2.6, -3.0, true), (-2.4, -2.0, false)] {
        cpu.set_fpu_flags(FpuFlags::empty());
        push_val(&mut cpu, value);
        // D9 FC: FRNDINT
        run_cpu_code(&mut cpu, &[0xD9, 0xFC]);
        assert_eq!(cpu.fpu_get(0).get_f64(), rounded, "{value}");
        let flags = cpu.get_fpu_flags();
        assert!(flags.contains(FpuFlags::PE), "{value}");
        assert_eq!(flags.contains(FpuFlags::C1), up, "{value}");
        cpu.fpu_pop();
    }
    cpu.set_fpu_flags(FpuFlags::C1);
    push_val(&mut cpu, 3.0);
    run_cpu_code(&mut cpu, &[0xD9, 0xFC]);
    assert_eq!(cpu.fpu_get(0).get_f64(), 3.0);
    assert!(!cpu.get_fpu_flags().intersects(FpuFlags::PE | FpuFlags::C1));
}

/// From the 80 bits: 2 + 2^-62 is past its double, and rounds up under RC=up.
#[test]
fn test_frndint_from_the_80_bits_reports_the_rounding() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let mut f = F80::new();
    f.set(((16383u128 + 1) << 64) | 0x8000_0000_0000_0001);
    cpu.fpu_push(f);
    cpu.fpu_control = 0x0B7F;
    cpu.set_fpu_flags(FpuFlags::empty());
    run_cpu_code(&mut cpu, &[0xD9, 0xFC]);
    assert_eq!(cpu.fpu_get(0).get_f64(), 3.0);
    let flags = cpu.get_fpu_flags();
    assert!(flags.contains(FpuFlags::PE | FpuFlags::C1));
}

#[test]
fn fadd_fsub_of_infinities_and_nans() {
    // (ST(1), ST(0), FADD or FSUB ST(0),ST(1), the result's 80 bits, IE)
    let inf = 0x7FFF_8000_0000_0000_0000u128;
    let ninf = 0xFFFF_8000_0000_0000_0000u128;
    let indefinite = 0xFFFF_C000_0000_0000_0000u128;
    let qnan = 0x7FFF_C000_0000_0000_1234u128;
    let snan = 0x7FFF_8000_0000_0000_1234u128;
    let one = 0x3FFF_8000_0000_0000_0000u128;
    let max = 0x7FFE_FFFF_FFFF_FFFF_FFFFu128;
    let cases = [
        (inf, inf, [0xD8, 0xC1], inf, false),
        (ninf, ninf, [0xD8, 0xC1], ninf, false),
        (inf, ninf, [0xD8, 0xC1], indefinite, true),
        (inf, inf, [0xD8, 0xE1], indefinite, true),
        (one, inf, [0xD8, 0xC1], inf, false),
        (inf, one, [0xD8, 0xE1], ninf, false),
        (qnan, one, [0xD8, 0xC1], qnan, false),
        (one, qnan, [0xD8, 0xC1], qnan, false),
        (snan, one, [0xD8, 0xC1], qnan, true),
        (qnan, qnan, [0xD8, 0xC1], qnan, false),
        (max, max, [0xD8, 0xC1], inf, false),
    ];
    for (st1, st0, code, result, ie) in cases {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        for v in [st1, st0] {
            let mut f = F80::new();
            f.set(v);
            cpu.fpu_push(f);
        }
        testrunners::run_fpu_code(&mut cpu, &code);
        let what = format!("{st0:020X} {code:02X?} {st1:020X}");
        assert_eq!(cpu.fpu_get(0).get(), result, "{what}");
        assert_eq!(cpu.get_fpu_flag(FpuFlags::IE), ie, "{what}: IE");
    }
}

#[test]
fn fprem_quotient_bits_are_the_magnitudes_and_fprem1_ties_to_even() {
    // (ST(0), ST(1), FPREM (D9 F8) or FPREM1 (D9 F5), remainder, quotient)
    let cases = [
        (-5.0, 4.0, [0xD9, 0xF8], -1.0, 1),
        (5.0, -4.0, [0xD9, 0xF8], 1.0, 1),
        (-14.0, 4.0, [0xD9, 0xF8], -2.0, 3),
        (5.0, 2.0, [0xD9, 0xF5], 1.0, 2),
        (7.0, 2.0, [0xD9, 0xF5], -1.0, 4),
        (-7.0, 2.0, [0xD9, 0xF5], 1.0, 4),
        (-5.0, 4.0, [0xD9, 0xF5], -1.0, 1),
        (23.0, 3.0, [0xD9, 0xF5], -1.0, 0),
    ];
    for (st0, st1, code, rem, q) in cases {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        for v in [st1, st0] {
            let mut f = F80::new();
            f.set_f64(v);
            cpu.fpu_push(f);
        }
        testrunners::run_fpu_code(&mut cpu, &code);
        let what = format!("{st0} {code:02X?} {st1}");
        assert_eq!(cpu.fpu_get(0).get_f64(), rem, "{what}");
        let bits = [FpuFlags::C0, FpuFlags::C3, FpuFlags::C1].map(|f| cpu.get_fpu_flag(f) as u32);
        assert_eq!(bits[0] << 2 | bits[1] << 1 | bits[2], q, "{what}: quotient bits");
        assert!(!cpu.get_fpu_flag(FpuFlags::C2), "{what}: complete");
    }
}

#[test]
fn fyl2x_of_zero_and_negative_values() {
    // (y, x, the result's 80 bits, ZE, IE)
    let indefinite = 0xFFFF_C000_0000_0000_0000u128;
    let ninf = 0xFFFF_8000_0000_0000_0000u128;
    let inf = 0x7FFF_8000_0000_0000_0000u128;
    let cases = [
        (2.0, 0.0, ninf, true, false),
        (-2.0, -0.0, inf, true, false),
        (0.0, 0.0, indefinite, false, true),
        (2.0, -1.0, indefinite, false, true),
        (f64::INFINITY, 1.0, indefinite, false, true),
    ];
    for (y, x, result, ze, ie) in cases {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        for v in [y, x] {
            let mut f = F80::new();
            f.set_f64(v);
            cpu.fpu_push(f);
        }
        testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xF1]);
        assert_eq!(cpu.fpu_get(0).get(), result, "{y} * log2({x})");
        assert_eq!(cpu.get_fpu_flag(FpuFlags::ZE), ze, "{y} * log2({x}): ZE");
        assert_eq!(cpu.get_fpu_flag(FpuFlags::IE), ie, "{y} * log2({x}): IE");
    }

    // FYL2XP1 (D9 F9) keeps a tiny x: 1 * log2(1 + 2^-60).
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    for v in [1.0, 2f64.powi(-60)] {
        let mut f = F80::new();
        f.set_f64(v);
        cpu.fpu_push(f);
    }
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xF9]);
    let r = cpu.fpu_get(0).get_f64();
    assert!((r / (2f64.powi(-60) * std::f64::consts::LOG2_E) - 1.0).abs() < 1e-12, "{r}");
}

#[test]
fn fscale_fxtract_fchs_and_trigonometry_edges() {
    let fresh = |values: &[u128]| {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        for &v in values {
            let mut f = F80::new();
            f.set(v);
            cpu.fpu_push(f);
        }
        cpu
    };
    let bits = |v: f64| {
        let mut f = F80::new();
        f.set_f64(v);
        f.get()
    };

    // FSCALE (D9 FD) keeps all 64 bits and goes past a double's range.
    let x = 0x3FFF_8000_0000_0000_0001u128;
    let mut cpu = fresh(&[bits(-2000.0), x]);
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xFD]);
    assert_eq!(cpu.fpu_get(0).get(), x - (2000 << 64));

    // FXTRACT (D9 F4) of -0: ST(1) -infinity, ST(0) -0, ZE.
    let mut cpu = fresh(&[bits(-0.0)]);
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xF4]);
    assert_eq!(cpu.fpu_get(0).get(), bits(-0.0));
    assert_eq!(cpu.fpu_get(1).get(), bits(f64::NEG_INFINITY));
    assert!(cpu.get_fpu_flag(FpuFlags::ZE));

    // FCHS (D9 E0) clears C1.
    let mut cpu = fresh(&[bits(1.0)]);
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xE0]);
    assert!(!cpu.get_fpu_flag(FpuFlags::C1));

    // FSIN (D9 FE) and FPTAN (D9 F2) of 2^63: C2, nothing changed.
    for code in [[0xD9, 0xFE], [0xD9, 0xF2]] {
        let big = 0x403E_8000_0000_0000_0000u128;
        let mut cpu = fresh(&[big]);
        let top = cpu.fpu_top;
        testrunners::run_fpu_code(&mut cpu, &code);
        assert!(cpu.get_fpu_flag(FpuFlags::C2), "{code:02X?}");
        assert_eq!(cpu.fpu_get(0).get(), big, "{code:02X?}");
        assert_eq!(cpu.fpu_top, top, "{code:02X?}");
    }

    // FSIN of an infinity: the real indefinite, IE.
    let mut cpu = fresh(&[bits(f64::INFINITY)]);
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xFE]);
    assert_eq!(cpu.fpu_get(0).get(), 0xFFFF_C000_0000_0000_0000);
    assert!(cpu.get_fpu_flag(FpuFlags::IE));
}
