use iced_x86::{Instruction, OpKind, MemorySize, Register};
use crate::cpu::{Cpu, FpuFlags};
use crate::f80::F80;
use crate::instructions::utils::calculate_addr;

// FIADD: Add Integer
// ST(0) = ST(0) + [mem_int]
pub fn fiadd(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let val = cpu.load_int_to_f80(addr, instr.memory_size());
    let mut st0 = cpu.fpu_get(0);
    st0.add(val);
    cpu.fpu_set(0, st0);
}

// FISUB: Subtract Integer
// ST(0) = ST(0) - [mem_int]
pub fn fisub(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let val = cpu.load_int_to_f80(addr, instr.memory_size());
    let mut st0 = cpu.fpu_get(0);
    st0.sub(val);
    cpu.fpu_set(0, st0);
}

// FISUBR: Subtract Integer Reverse
// ST(0) = [mem_int] - ST(0)
pub fn fisubr(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let mut val = cpu.load_int_to_f80(addr, instr.memory_size());
    let st0 = cpu.fpu_get(0);
    val.sub(st0);
    cpu.fpu_set(0, val);
}

// FIMUL: Multiply Integer
// ST(0) = ST(0) * [mem_int]
pub fn fimul(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let val = cpu.load_int_to_f80(addr, instr.memory_size()).get_f64();
    let mut st0 = cpu.fpu_get(0);
    // Note: If F80 doesn't have mul yet, use f64 as intermediary
    let res_f = st0.get_f64() * val;
    st0.set_f64(res_f);
    cpu.fpu_set(0, st0);
}

// FIDIV: Divide Integer
// ST(0) = ST(0) / [mem_int]
pub fn fidiv(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let val = cpu.load_int_to_f80(addr, instr.memory_size()).get_f64();
    let mut st0 = cpu.fpu_get(0);
    if val != 0.0 {
        st0.set_f64(st0.get_f64() / val);
    } else {
        st0.set_f64(f64::INFINITY);
    }
    cpu.fpu_set(0, st0);
}

// FIDIVR: Reverse Integer Divide
// ST(0) = [mem_int] / ST(0)
pub fn fidivr(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let val = cpu.load_int_to_f80(addr, instr.memory_size()).get_f64();
    let mut st0 = cpu.fpu_get(0);
    let st0_f = st0.get_f64();
    if st0_f != 0.0 {
        st0.set_f64(val / st0_f);
    } else {
        st0.set_real_indefinite();
    }
    cpu.fpu_set(0, st0);
}

// FADD: Add Real
pub fn fadd(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        let mut val = F80::new();
        match instr.memory_size() {
            MemorySize::Float32 => val.set_f64(f32::from_bits(cpu.bus.read_32(addr)) as f64),
            MemorySize::Float64 => val.set_f64(f64::from_bits(cpu.bus.read_64(addr))),
            _ => {}
        }
        let mut st0 = cpu.fpu_get(0);
        st0.add(val);
        cpu.fpu_set(0, st0);
    } else {
        let dst_reg = instr.op0_register();
        let src_reg = instr.op1_register();
        let idx_src = (src_reg.number() - Register::ST0.number()) as usize;
        let idx_dst = (dst_reg.number() - Register::ST0.number()) as usize;

        let mut dest = cpu.fpu_get(idx_dst);
        let src = cpu.fpu_get(idx_src);
        dest.add(src);
        cpu.fpu_set(idx_dst, dest);
    }
}

// FADDP: Add and Pop
pub fn faddp(cpu: &mut Cpu, instr: &Instruction) {
    let dst_reg = instr.op0_register();
    let idx = if dst_reg == Register::None || dst_reg == Register::ST1 { 1 } 
              else { (dst_reg.number() - Register::ST0.number()) as usize };

    let mut sti = cpu.fpu_get(idx);
    let st0 = cpu.fpu_get(0);
    sti.add(st0);
    cpu.fpu_set(idx, sti);
    cpu.fpu_pop();
}

// FSUB: Subtract Real
// ST(0) = ST(0) - Src  OR  Dest = Dest - ST(0)
pub fn fsub(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        let mut val = F80::new();
        match instr.memory_size() {
            MemorySize::Float32 => val.set_f64(f32::from_bits(cpu.bus.read_32(addr)) as f64),
            MemorySize::Float64 => val.set_f64(f64::from_bits(cpu.bus.read_64(addr))),
            _ => {}
        }
        let mut st0 = cpu.fpu_get(0);
        st0.sub(val);
        cpu.fpu_set(0, st0);
    } else {
        let dst_idx = (instr.op0_register().number() - Register::ST0.number()) as usize;
        let src_idx = (instr.op1_register().number() - Register::ST0.number()) as usize;
        let mut dst = cpu.fpu_get(dst_idx);
        let src = cpu.fpu_get(src_idx);
        dst.sub(src);
        cpu.fpu_set(dst_idx, dst);
    }
}

// FSUBP: Subtract and Pop
// ST(1) = ST(1) - ST(0); Pop ST(0)
pub fn fsubp(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    let mut st1 = cpu.fpu_get(1);
    st1.sub(st0);
    cpu.fpu_set(1, st1);
    cpu.fpu_pop();
}

// FSUBR: Reverse Subtract
// ST(0) = Src - ST(0)  OR  Dest = ST(0) - Dest
pub fn fsubr(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        let mut val = F80::new();
        match instr.memory_size() {
            MemorySize::Float32 => val.set_f64(f32::from_bits(cpu.bus.read_32(addr)) as f64),
            MemorySize::Float64 => val.set_f64(f64::from_bits(cpu.bus.read_64(addr))),
            _ => {}
        }
        let st0 = cpu.fpu_get(0);
        val.sub(st0);
        cpu.fpu_set(0, val);
    } else {
        let dst_idx = (instr.op0_register().number() - Register::ST0.number()) as usize;
        let src_idx = (instr.op1_register().number() - Register::ST0.number()) as usize;
        let dst = cpu.fpu_get(dst_idx);
        let mut src = cpu.fpu_get(src_idx);
        src.sub(dst);
        cpu.fpu_set(dst_idx, src);
    }
}

// FSUBRP: Reverse Subtract and Pop
// ST(1) = ST(0) - ST(1); Pop ST(0)
pub fn fsubrp(cpu: &mut Cpu) {
    let mut st0 = cpu.fpu_get(0);
    let st1 = cpu.fpu_get(1);
    st0.sub(st1);
    cpu.fpu_set(1, st0);
    cpu.fpu_pop();
}

// FMUL: Multiply Real
pub fn fmul(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        let mut val = F80::new();
        match instr.memory_size() {
            MemorySize::Float32 => val.set_f64(f32::from_bits(cpu.bus.read_32(addr)) as f64),
            MemorySize::Float64 => val.set_f64(f64::from_bits(cpu.bus.read_64(addr))),
            _ => {}
        }
        let mut st0 = cpu.fpu_get(0);
        st0.set_f64(st0.get_f64() * val.get_f64());
        cpu.fpu_set(0, st0);
    } else {
        let dst_idx = (instr.op0_register().number() - Register::ST0.number()) as usize;
        let src_idx = (instr.op1_register().number() - Register::ST0.number()) as usize;
        let mut dst = cpu.fpu_get(dst_idx);
        let src = cpu.fpu_get(src_idx);
        dst.set_f64(dst.get_f64() * src.get_f64());
        cpu.fpu_set(dst_idx, dst);
    }
}

// FMULP: Multiply and Pop
pub fn fmulp(cpu: &mut Cpu, instr: &Instruction) {
    let dst_reg = instr.op0_register();
    let idx = if dst_reg == Register::None || dst_reg == Register::ST1 { 1 } 
              else { (dst_reg.number() - Register::ST0.number()) as usize };
    let mut sti = cpu.fpu_get(idx);
    let st0 = cpu.fpu_get(0);
    sti.set_f64(sti.get_f64() * st0.get_f64());
    cpu.fpu_set(idx, sti);
    cpu.fpu_pop();
}

// FDIV: Floating Point Divide
pub fn fdiv(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        let mut divisor = F80::new();
        match instr.memory_size() {
            MemorySize::Float32 => divisor.set_f64(f32::from_bits(cpu.bus.read_32(addr)) as f64),
            MemorySize::Float64 => divisor.set_f64(f64::from_bits(cpu.bus.read_64(addr))),
            _ => {}
        }
        let mut st0 = cpu.fpu_get(0);
        let div_f = divisor.get_f64();
        if div_f != 0.0 { st0.set_f64(st0.get_f64() / div_f); } else { st0.set_real_indefinite(); }
        cpu.fpu_set(0, st0);
    } else {
        let dst_idx = (instr.op0_register().number() - Register::ST0.number()) as usize;
        let src_idx = (instr.op1_register().number() - Register::ST0.number()) as usize;
        let mut dst = cpu.fpu_get(dst_idx);
        let src = cpu.fpu_get(src_idx);
        let src_f = src.get_f64();
        if src_f != 0.0 { dst.set_f64(dst.get_f64() / src_f); } else { dst.set_real_indefinite(); }
        cpu.fpu_set(dst_idx, dst);
    }
}

// FDIVP: Divide and Pop
pub fn fdivp(cpu: &mut Cpu, instr: &Instruction) {
    let dst_reg = instr.op0_register();
    let idx = if dst_reg == Register::None || dst_reg == Register::ST1 { 1 } 
              else { (dst_reg.number() - Register::ST0.number()) as usize };
    let mut sti = cpu.fpu_get(idx);
    let st0 = cpu.fpu_get(0).get_f64();
    if st0 != 0.0 { sti.set_f64(sti.get_f64() / st0); } else { sti.set_real_indefinite(); }
    cpu.fpu_set(idx, sti);
    cpu.fpu_pop();
}

// FDIVR: Reverse Divide
pub fn fdivr(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        // FDIVR [mem] -> ST(0) = [mem] / ST(0)
        let addr = calculate_addr(cpu, instr);
        let mut mem_val = F80::new();
        match instr.memory_size() {
            MemorySize::Float32 => mem_val.set_f64(f32::from_bits(cpu.bus.read_32(addr)) as f64),
            MemorySize::Float64 => mem_val.set_f64(f64::from_bits(cpu.bus.read_64(addr))),
            _ => mem_val.set_f64(1.0),
        }
        
        let mut st0 = cpu.fpu_get(0);
        let st0_f = st0.get_f64();
        
        if st0_f != 0.0 {
            st0.set_f64(mem_val.get_f64() / st0_f);
        } else {
            st0.set_real_indefinite(); // Handle division by zero
            cpu.set_fpu_flag(FpuFlags::ZE, true);
        }
        cpu.fpu_set(0, st0);
    } else {
        let dst_idx = (instr.op0_register().number() - Register::ST0.number()) as usize;
        let src_idx = (instr.op1_register().number() - Register::ST0.number()) as usize;
        
        let mut dst = cpu.fpu_get(dst_idx);
        let src = cpu.fpu_get(src_idx);
        
        // FDIVR ST(0), ST(i) -> ST(0) = ST(i) / ST(0)
        // FDIVR ST(i), ST(0) -> ST(i) = ST(0) / ST(i)
        // In both cases, we divide the "Source" by the "Destination"
        let dst_f = dst.get_f64();
        if dst_f != 0.0 {
            dst.set_f64(src.get_f64() / dst_f);
        } else {
            dst.set_real_indefinite();
            cpu.set_fpu_flag(FpuFlags::ZE, true);
        }
        cpu.fpu_set(dst_idx, dst);
    }
}

// FDIVRP: Reverse Divide and Pop
// ST(1) = ST(0) / ST(1); Pop ST(0)
pub fn fdivrp(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    let mut st1 = cpu.fpu_get(1);
    
    let st1_f = st1.get_f64();
    if st1_f != 0.0 {
        st1.set_f64(st0.get_f64() / st1_f);
    } else {
        st1.set_real_indefinite();
        cpu.set_fpu_flag(FpuFlags::ZE, true);
    }
    
    cpu.fpu_set(1, st1);
    cpu.fpu_pop();
}

// --- ADVANCED ARITHMETIC ---


pub fn fprem_internal(cpu: &mut Cpu, ieee: bool) {
    let st0 = cpu.fpu_get(0).get_f64();
    let st1 = cpu.fpu_get(1).get_f64();

    if st1 == 0.0 {
        cpu.set_fpu_flag(FpuFlags::IE, true);
        let mut nan = F80::new();
        nan.set_f64(f64::NAN);
        cpu.fpu_set(0, nan);
        return;
    }

    let quotient_f = st0 / st1;
    let q_int = if ieee { quotient_f.round() as i64 } else { quotient_f.trunc() as i64 };
    let remainder = st0 - (q_int as f64 * st1);
    
    let mut res = F80::new();
    res.set_f64(remainder);
    cpu.fpu_set(0, res);

    let q = q_int.abs();
    cpu.set_fpu_flag(FpuFlags::C0 | FpuFlags::C1 | FpuFlags::C2 | FpuFlags::C3, false);
    if (q & 4) != 0 { cpu.set_fpu_flag(FpuFlags::C0, true); }
    if (q & 1) != 0 { cpu.set_fpu_flag(FpuFlags::C1, true); }
    if (q & 2) != 0 { cpu.set_fpu_flag(FpuFlags::C3, true); }
}

// FPREM: Partial Remainder (Rounding toward Zero)
// ST(0) = ST(0) % ST(1)
pub fn fprem(cpu: &mut Cpu) {
    fprem_internal(cpu, false);
}

// FPREM1: IEEE Partial Remainder (Rounding to Nearest)
// Difference from FPREM: Uses Round-to-Nearest for the quotient logic
pub fn fprem1(cpu: &mut Cpu) {
    fprem_internal(cpu, true);
}

// FRNDINT: Round to Integer
pub fn frndint(cpu: &mut Cpu) {
    let mut st0 = cpu.fpu_get(0);
    let val = st0.get_f64();
    let rc = (cpu.fpu_control >> 10) & 0x03;
    let result = match rc {
        0 => val.round(), // Nearest
        1 => val.floor(), // Down
        2 => val.ceil(),  // Up
        3 => val.trunc(), // Toward Zero
        _ => val,
    };
    st0.set_f64(result);
    cpu.fpu_set(0, st0);
    cpu.set_fpu_flag(FpuFlags::C2, false);
}

// FABS: Absolute Value
pub fn fabs(cpu: &mut Cpu) {
    let mut st0 = cpu.fpu_get(0);
    st0.set_sign(false);
    cpu.fpu_set(0, st0);
    cpu.set_fpu_flag(FpuFlags::C1, false);
}

// FCHS: Change Sign
pub fn fchs(cpu: &mut Cpu) {
    let mut st0 = cpu.fpu_get(0);
    st0.neg();
    cpu.fpu_set(0, st0);
    cpu.set_fpu_flag(FpuFlags::C1, st0.get_sign());
}

// FSCALE: Scale by 2^trunc(ST(1))
// ST(0) = ST(0) * 2^(trunc(ST(1)))
pub fn fscale(cpu: &mut Cpu) {
    let mut st0 = cpu.fpu_get(0);
    let st1 = cpu.fpu_get(1).get_f64().trunc();
    let res = st0.get_f64() * 2.0_f64.powf(st1);
    st0.set_f64(res);
    cpu.fpu_set(0, st0);
}

// FSQRT: Square Root
// ST(0) = sqrt(ST(0))
pub fn fsqrt(cpu: &mut Cpu) {
    let mut st0 = cpu.fpu_get(0);
    let val = st0.get_f64();

    // x87 handles -0.0 by returning -0.0
    if st0.is_zero() {
        // Result is already zero, just preserve the sign (usually +0.0 or -0.0)
        cpu.fpu_set(0, st0);
        cpu.set_fpu_flag(FpuFlags::C1, false); 
        return;
    }

    if !st0.get_sign() {
        // Case: Positive number
        st0.set_f64(val.sqrt());
        cpu.fpu_set(0, st0);
        cpu.set_fpu_flag(FpuFlags::C1, false); // No rounding-up occurred (simplified)
    } else {
        // Case: Negative number (Invalid Operation)
        cpu.set_fpu_flag(FpuFlags::IE, true);  // Set Invalid Operation bit
        
        // Return "Real Indefinite" (The special NaN for FPU errors)
        st0.set_real_indefinite();
        cpu.fpu_set(0, st0);
    }
}

// FXTRACT: Extract Exponent and Significand
// Separates ST(0) into exponent and significand.
// ST(0) becomes Exponent (unbiased), Push Significand.
pub fn fxtract(cpu: &mut Cpu) {
    let val = cpu.fpu_get(0);
    let f_val = val.get_f64();
    
    // Handle Zero case
    if f_val == 0.0 {
        // ST(0) = -Infinity
        let mut neg_inf = F80::new(); 
        neg_inf.set_f64(f64::NEG_INFINITY);
        cpu.fpu_set(0, neg_inf);
        
        // Push 0.0
        let mut zero = F80::new(); 
        zero.set_f64(0.0);
        cpu.fpu_push(zero);
        return;
    }

    let exp = (val.get_exponent() as i32) - 16383;
    let mut sig = val; 
    sig.set_exponent(16383); // Normalize significand to 1.xx

    // ST(0) becomes Exponent
    let mut f_exp = F80::new(); 
    f_exp.set_f64(exp as f64);
    cpu.fpu_set(0, f_exp);

    // Push Significand (New ST(0))
    cpu.fpu_push(sig);
}

// F2XM1: 2^x - 1
pub fn f2xm1(cpu: &mut Cpu) {
    let mut st0 = cpu.fpu_get(0);
    st0.set_f64(2.0f64.powf(st0.get_f64()) - 1.0);
    cpu.fpu_set(0, st0);
}

// FYL2X: y * log2(x)
// ST(1) = ST(1) * log2(ST(0)); Pop ST(0)
pub fn fyl2x(cpu: &mut Cpu) {
    let x = cpu.fpu_get(0).get_f64();
    let mut y = cpu.fpu_get(1);
    if x > 0.0 { y.set_f64(y.get_f64() * x.log2()); } else { y.set_QNaN(); }
    cpu.fpu_set(1, y);
    cpu.fpu_pop();
}

// FYL2XP1: y * log2(x + 1)
pub fn fyl2xp1(cpu: &mut Cpu) {
    let x = cpu.fpu_get(0).get_f64();
    let mut y = cpu.fpu_get(1);
    y.set_f64(y.get_f64() * (x + 1.0).log2());
    cpu.fpu_set(1, y);
    cpu.fpu_pop();
}