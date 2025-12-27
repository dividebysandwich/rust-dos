use crate::cpu::{Cpu, FpuFlags};

// FSIN: Sine
pub fn fsin(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    cpu.fpu_set(0, st0.sin());
    // Clear C2 to indicate success (no out-of-bounds)
    cpu.set_fpu_flag(FpuFlags::C2, false);
}

// FCOS: Cosine
pub fn fcos(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    cpu.fpu_set(0, st0.cos());
    cpu.set_fpu_flag(FpuFlags::C2, false);
}

// FSINCOS: Sine and Cosine
pub fn fsincos(cpu: &mut Cpu) {
    let theta = cpu.fpu_get(0);
    let sin_val = theta.sin();
    let cos_val = theta.cos();

    // Store Sin in current ST(0)
    cpu.fpu_set(0, sin_val);
    
    // Push Cos to become new ST(0)
    cpu.fpu_push(cos_val);
    
    cpu.set_fpu_flag(FpuFlags::C2, false);
}

// FPTAN: Partial Tangent
pub fn fptan(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    cpu.fpu_set(0, st0.tan());
    cpu.fpu_push(1.0); // Partial tangent requirement!
    cpu.set_fpu_flag(FpuFlags::C2, false);
}

// FPATAN: Partial Arctangent
pub fn fpatan(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0); // X
    let st1 = cpu.fpu_get(1); // Y
    
    let res = st1.atan2(st0);
    
    cpu.fpu_set(1, res);
    cpu.fpu_pop();
}