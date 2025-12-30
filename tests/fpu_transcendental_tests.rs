use rust_dos::cpu::{Cpu, FpuFlags};
use rust_dos::f80::F80;
use std::f64::consts::{PI, FRAC_PI_2, FRAC_PI_4};

mod testrunners;
use testrunners::run_cpu_code;

// Helper to push values
fn push_val(cpu: &mut Cpu, val: f64) {
    let mut f = F80::new();
    f.set_f64(val);
    cpu.fpu_push(f);
}

// Helper to assert float equality with epsilon
fn assert_f64_eq(val: f64, expected: f64, message: &str) {
    let diff = (val - expected).abs();
    assert!(diff < 0.00001, "{}: Expected {}, got {}", message, expected, val);
}

#[test]
fn test_fsin_sine_wave() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // 1. Test sin(PI/2) = 1.0
    push_val(&mut cpu, FRAC_PI_2);
    // D9 FE: FSIN
    run_cpu_code(&mut cpu, &[0xD9, 0xFE]);
    
    assert_f64_eq(cpu.fpu_get(0).get_f64(), 1.0, "sin(PI/2) should be 1.0");
    assert!(!cpu.get_fpu_flag(FpuFlags::C2)); // C2 Cleared on success

    // 2. Test sin(PI) ≈ 0.0
    push_val(&mut cpu, PI);
    run_cpu_code(&mut cpu, &[0xD9, 0xFE]);
    
    assert_f64_eq(cpu.fpu_get(0).get_f64(), 0.0, "sin(PI) should be ~0.0");
}

#[test]
fn test_fcos_cosine_wave() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // 1. Test cos(0) = 1.0
    push_val(&mut cpu, 0.0);
    // D9 FF: FCOS
    run_cpu_code(&mut cpu, &[0xD9, 0xFF]);

    assert_f64_eq(cpu.fpu_get(0).get_f64(), 1.0, "cos(0) should be 1.0");

    // 2. Test cos(PI) = -1.0
    push_val(&mut cpu, PI);
    run_cpu_code(&mut cpu, &[0xD9, 0xFF]);
    
    assert_f64_eq(cpu.fpu_get(0).get_f64(), -1.0, "cos(PI) should be -1.0");
}

#[test]
fn test_fsincos_simultaneous() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // Compute sin and cos of PI/4 (both should be ~0.7071)
    // Stack: Top -> PI/4
    push_val(&mut cpu, FRAC_PI_4);

    // D9 FB: FSINCOS
    // Expected behavior: 
    // 1. Calculate Sin and Cos.
    // 2. ST(0) becomes Cos.
    // 3. ST(1) becomes Sin. (Stack grows by 1)
    run_cpu_code(&mut cpu, &[0xD9, 0xFB]);

    // Check Cosine at ST(0)
    assert_f64_eq(cpu.fpu_get(0).get_f64(), 0.7071067, "ST(0) should be Cosine");
    
    // Check Sine at ST(1)
    assert_f64_eq(cpu.fpu_get(1).get_f64(), 0.7071067, "ST(1) should be Sine");
    
    // Check C2 cleared
    assert!(!cpu.get_fpu_flag(FpuFlags::C2));
}

#[test]
fn test_fptan_partial_tangent() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // tan(PI/4) = 1.0
    push_val(&mut cpu, FRAC_PI_4);

    // D9 F2: FPTAN
    // Expected behavior:
    // 1. Replace ST(0) with tan(ST(0)) -> 1.0
    // 2. Push 1.0 onto stack.
    // Final: ST(0)=1.0 (The constant), ST(1)=1.0 (The result)
    run_cpu_code(&mut cpu, &[0xD9, 0xF2]);

    assert_f64_eq(cpu.fpu_get(0).get_f64(), 1.0, "ST(0) should be the pushed 1.0 constant");
    assert_f64_eq(cpu.fpu_get(1).get_f64(), 1.0, "ST(1) should be tan(PI/4)");
}

#[test]
fn test_fpatan_arctangent_coordinate() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // FPATAN calculates atan2(Y, X) = atan2(ST(1), ST(0))
    // Let's compute angle of (1, 1) which is PI/4 (45 degrees)
    
    push_val(&mut cpu, 1.0); // Y (will become ST(1))
    push_val(&mut cpu, 1.0); // X (will become ST(0))

    // D9 F3: FPATAN
    // Result stored in ST(1), ST(0) popped.
    run_cpu_code(&mut cpu, &[0xD9, 0xF3]);

    // Stack should shrink by 1
    // New ST(0) holds the result
    assert_f64_eq(cpu.fpu_get(0).get_f64(), FRAC_PI_4, "atan2(1,1) should be PI/4");
    
    // Previous ST(0) should be gone (Tag empty)
    // Note: cpu.fpu_pop() moves the top pointer, but we access physical slots to check tags usually.
    // For this test, just ensuring the value is right is sufficient.
}

#[test]
fn test_fpatan_negative_coordinates() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));

    // atan2(-1.0, -1.0) should be -3*PI/4 (-135 degrees)
    push_val(&mut cpu, -1.0); // Y
    push_val(&mut cpu, -1.0); // X

    run_cpu_code(&mut cpu, &[0xD9, 0xF3]);

    let expected = -3.0 * FRAC_PI_4;
    assert_f64_eq(cpu.fpu_get(0).get_f64(), expected, "atan2(-1,-1) should be -3PI/4");
}