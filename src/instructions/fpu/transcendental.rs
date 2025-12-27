use crate::cpu::{Cpu, FpuFlags};
use crate::f80::F80;

// FSIN: Sine
pub fn fsin(cpu: &mut Cpu) {
    let mut st0 = cpu.fpu_get(0);
    let val_f = st0.get_f64();
    
    // Perform calculation and re-encode to F80
    st0.set_f64(val_f.sin());
    cpu.fpu_set(0, st0);
    
    // C2=0 indicates the operand was within range (-2^63 to 2^63)
    cpu.set_fpu_flag(FpuFlags::C2, false);
}

// FCOS: Cosine
pub fn fcos(cpu: &mut Cpu) {
    let mut st0 = cpu.fpu_get(0);
    let val_f = st0.get_f64();
    
    st0.set_f64(val_f.cos());
    cpu.fpu_set(0, st0);
    
    cpu.set_fpu_flag(FpuFlags::C2, false);
}

// FSINCOS: Sine and Cosine
pub fn fsincos(cpu: &mut Cpu) {
    let theta = cpu.fpu_get(0).get_f64();
    
    let mut sin_f80 = F80::new();
    let mut cos_f80 = F80::new();
    
    sin_f80.set_f64(theta.sin());
    cos_f80.set_f64(theta.cos());

    // Replace ST(0) with Sine
    cpu.fpu_set(0, sin_f80);
    
    // Push Cosine to become the new ST(0)
    cpu.fpu_push(cos_f80);
    
    cpu.set_fpu_flag(FpuFlags::C2, false);
}

// FPTAN: Partial Tangent
pub fn fptan(cpu: &mut Cpu) {
    let mut st0 = cpu.fpu_get(0);
    let val_f = st0.get_f64();
    
    st0.set_f64(val_f.tan());
    cpu.fpu_set(0, st0);
    
    // FPTAN pushes 1.0 onto the stack after the result for compatibility with 8087
    let mut one = F80::new();
    one.set_f64(1.0);
    cpu.fpu_push(one);
    
    cpu.set_fpu_flag(FpuFlags::C2, false);
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