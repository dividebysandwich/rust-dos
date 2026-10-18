use crate::cpu::{Cpu, FpuFlags};
use crate::f80::F80;

/// Whether ST(0) is in range for FSIN, FCOS, FSINCOS and FPTAN (|x| <
/// 2^63, C2 clear); out of range, C2 is set and nothing else changes.
fn in_range(cpu: &mut Cpu) -> bool {
    let st0 = cpu.fpu_get(0);
    let out = !st0.is_nan() && !st0.is_infinite() && st0.get_exponent() >= 0x3FFF + 63;
    cpu.set_fpu_flag(FpuFlags::C2, out);
    !out
}

/// A result as 80 bits: a NaN from a number (an infinity's sine) is the
/// real indefinite with IE, whatever NaN the host makes.
fn result(cpu: &mut Cpu, arg: f64, value: f64) -> F80 {
    let mut f = F80::new();
    if value.is_nan() && !arg.is_nan() {
        f.set_real_indefinite();
        cpu.set_fpu_flag(FpuFlags::IE, true);
    } else {
        f.set_f64(value);
    }
    f
}

// FSIN: Sine
pub fn fsin(cpu: &mut Cpu) {
    if !in_range(cpu) {
        return;
    }
    let val_f = cpu.fpu_get(0).get_f64();
    let st0 = result(cpu, val_f, val_f.sin());
    cpu.fpu_set(0, st0);
}

// FCOS: Cosine
pub fn fcos(cpu: &mut Cpu) {
    if !in_range(cpu) {
        return;
    }
    let val_f = cpu.fpu_get(0).get_f64();
    let st0 = result(cpu, val_f, val_f.cos());
    cpu.fpu_set(0, st0);
}

// FSINCOS: Sine and Cosine
pub fn fsincos(cpu: &mut Cpu) {
    if !in_range(cpu) {
        return;
    }
    let theta = cpu.fpu_get(0).get_f64();
    let sin_f80 = result(cpu, theta, theta.sin());
    let cos_f80 = result(cpu, theta, theta.cos());

    // Replace ST(0) with Sine
    cpu.fpu_set(0, sin_f80);
    
    // Push Cosine to become the new ST(0)
    cpu.fpu_push(cos_f80);
}

// FPTAN: Partial Tangent
pub fn fptan(cpu: &mut Cpu) {
    if !in_range(cpu) {
        return;
    }
    let val_f = cpu.fpu_get(0).get_f64();
    let st0 = result(cpu, val_f, val_f.tan());
    cpu.fpu_set(0, st0);
    
    // FPTAN pushes 1.0 onto the stack after the result for compatibility with 8087
    let mut one = F80::new();
    one.set_f64(1.0);
    cpu.fpu_push(one);
}

// FPATAN: Partial Arctangent
pub fn fpatan(cpu: &mut Cpu) {
    let x = cpu.fpu_get(0).get_f64(); // ST(0)
    let y = cpu.fpu_get(1).get_f64(); // ST(1)
    
    let mut res = F80::new();
    // Result is atan(ST(1)/ST(0))
    res.set_f64(y.atan2(x));
    
    // Store in ST(1) and pop ST(0)
    cpu.fpu_set(1, res);
    cpu.fpu_pop();
}