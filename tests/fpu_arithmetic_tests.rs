use rust_dos::cpu::{Cpu, FpuFlags};
use rust_dos::f80::F80;

mod testrunners;

#[test]
fn test_fpu_arithmetic_matrix() {
    let mut cpu = Cpu::new();
    
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
    let mut cpu = Cpu::new();
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
    let mut cpu = Cpu::new();
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
    let mut cpu = Cpu::new();
    let mut f0 = F80::new(); f0.set_f64(100.0);
    cpu.fpu_push(f0);

    // Setup memory: [0x200] = 50 (Int16)
    let addr = 0x200;
    cpu.bus.write_16(addr, 50);

    // FIADD [0x200] -> DA 06 00 02 (using absolute addr for simplicity)
    // Note: iced_x86 decoder needs a valid instruction. 
    // DA 06 00 02 is FIADD [0200] in 16-bit mode
    testrunners::run_fpu_code(&mut cpu, &[0xDA, 0x06, 0x00, 0x02]);

    assert_eq!(cpu.fpu_get(0).get_f64(), 150.0);
}

#[test]
fn test_fsqrt_and_invalid_op() {
    let mut cpu = Cpu::new();
    
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
    let mut cpu = Cpu::new();
    
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
    let mut cpu = Cpu::new();
    let mut f_neg = F80::new(); f_neg.set_f64(-5.5);
    cpu.fpu_push(f_neg);

    // FABS
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xE1]);
    assert_eq!(cpu.fpu_get(0).get_f64(), 5.5);
    assert_eq!(cpu.fpu_get(0).get_sign(), false);

    // FCHS (change back to negative)
    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xE0]);
    assert_eq!(cpu.fpu_get(0).get_f64(), -5.5);
    assert_eq!(cpu.fpu_get(0).get_sign(), true);
}

#[test]
fn test_f2xm1_exponentiation() {
    let mut cpu = Cpu::new();
    
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
    let mut cpu = Cpu::new();
    
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
    let mut cpu = Cpu::new();
    let mut f12 = F80::new(); f12.set_f64(12.0); // 1.5 * 2^3
    cpu.fpu_push(f12);

    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xF4]);

    assert_eq!(cpu.fpu_get(0).get_f64(), 1.5);
    assert_eq!(cpu.fpu_get(1).get_f64(), 3.0);
}

#[test]
fn test_fscale_powers_of_two() {
    let mut cpu = Cpu::new();
    let mut f_exp = F80::new(); f_exp.set_f64(2.0);
    let mut f_val = F80::new(); f_val.set_f64(3.0);
    
    cpu.fpu_push(f_exp);
    cpu.fpu_push(f_val);

    testrunners::run_fpu_code(&mut cpu, &[0xD9, 0xFD]);

    assert_eq!(cpu.fpu_get(0).get_f64(), 12.0);
}

#[test]
fn test_fadd_diagnostic() {
    let mut cpu = Cpu::new();
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
        println!("Phys Reg {}: {:?} (Tag: {})", i, cpu.fpu_stack[i].get_f64(), cpu.fpu_tags[i]);
    }

    assert_eq!(cpu.fpu_get(1).get_f64(), 13.0);
}

#[test]
fn test_fsub_variants() {
    let mut cpu = Cpu::new();
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

