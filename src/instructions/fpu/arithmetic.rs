use iced_x86::{Instruction, OpKind, MemorySize, Register};
use crate::cpu::{Cpu, FPU_C0, FPU_C1, FPU_C3};
use crate::instructions::utils::calculate_addr;

// FIADD: Add Integer
// ST(0) = ST(0) + [mem_int]
pub fn fiadd(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let val = match instr.memory_size() {
        MemorySize::Int16 => (cpu.bus.read_16(addr) as i16) as f64,
        MemorySize::Int32 => (cpu.bus.read_32(addr) as i32) as f64,
        _ => 0.0,
    };
    let st0 = cpu.fpu_get(0);
    cpu.fpu_set(0, st0 + val);
}

// FISUB: Subtract Integer
// ST(0) = ST(0) - [mem_int]
pub fn fisub(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let val = match instr.memory_size() {
        MemorySize::Int16 => (cpu.bus.read_16(addr) as i16) as f64,
        MemorySize::Int32 => (cpu.bus.read_32(addr) as i32) as f64,
        _ => 0.0,
    };
    let st0 = cpu.fpu_get(0);
    cpu.fpu_set(0, st0 - val);
}

// FISUBR: Subtract Integer Reverse
// ST(0) = [mem_int] - ST(0)
pub fn fisubr(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let val = match instr.memory_size() {
        MemorySize::Int16 => (cpu.bus.read_16(addr) as i16) as f64,
        MemorySize::Int32 => (cpu.bus.read_32(addr) as i32) as f64,
        _ => 0.0,
    };
    let st0 = cpu.fpu_get(0);
    cpu.fpu_set(0, val - st0);
}

// FIMUL: Multiply Integer
// ST(0) = ST(0) * [mem_int]
pub fn fimul(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let val = match instr.memory_size() {
        MemorySize::Int16 => (cpu.bus.read_16(addr) as i16) as f64,
        MemorySize::Int32 => (cpu.bus.read_32(addr) as i32) as f64,
        _ => 0.0,
    };
    let st0 = cpu.fpu_get(0);
    cpu.fpu_set(0, st0 * val);
}

// FIDIV: Divide Integer
// ST(0) = ST(0) / [mem_int]
pub fn fidiv(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let val = match instr.memory_size() {
        MemorySize::Int16 => (cpu.bus.read_16(addr) as i16) as f64,
        MemorySize::Int32 => (cpu.bus.read_32(addr) as i32) as f64,
        _ => 1.0,
    };
    let st0 = cpu.fpu_get(0);
    if val != 0.0 {
        cpu.fpu_set(0, st0 / val);
    } else {
        cpu.fpu_set(0, f64::INFINITY);
    }
}

// FIDIVR: Reverse Integer Divide
// ST(0) = [mem_int] / ST(0)
pub fn fidivr(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let val = match instr.memory_size() {
        MemorySize::Int16 => (cpu.bus.read_16(addr) as i16) as f64,
        MemorySize::Int32 => (cpu.bus.read_32(addr) as i32) as f64,
        _ => 1.0,
    };
    let st0 = cpu.fpu_get(0);
    if st0 != 0.0 {
        cpu.fpu_set(0, val / st0);
    } else {
        cpu.fpu_set(0, f64::INFINITY);
    }
}

// FADD: Add Real
pub fn fadd(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        // FADD [mem]
        let addr = calculate_addr(cpu, instr);
        let val = match instr.memory_size() {
            MemorySize::Float32 => f32::from_bits(cpu.bus.read_32(addr)) as f64,
            MemorySize::Float64 => f64::from_bits(cpu.bus.read_64(addr)),
            _ => 0.0,
        };
        let st0 = cpu.fpu_get(0);
        cpu.fpu_set(0, st0 + val);
    } else {
        // Register form
        let dst_reg = instr.op0_register();
        let src_reg = instr.op1_register();

        if dst_reg == Register::ST0 {
            // FADD ST(0), ST(i)
            let idx = src_reg.number() - Register::ST0.number();
            let st0 = cpu.fpu_get(0);
            let sti = cpu.fpu_get(idx as usize);
            cpu.fpu_set(0, st0 + sti);
        } else if src_reg == Register::ST0 {
            // FADD ST(i), ST(0)
            let idx = dst_reg.number() - Register::ST0.number();
            let sti = cpu.fpu_get(idx as usize);
            let st0 = cpu.fpu_get(0);
            cpu.fpu_set(idx as usize, sti + st0);
        }
    }
}

// FADDP: Add and Pop
pub fn faddp(cpu: &mut Cpu, instr: &Instruction) {
    let dst_reg = instr.op0_register();
    // Default to ST(1) if implicit
    let idx = if dst_reg == Register::None || dst_reg == Register::ST1 {
        1
    } else {
        (dst_reg.number() - Register::ST0.number()) as usize
    };

    let st0 = cpu.fpu_get(0);
    let sti = cpu.fpu_get(idx);
    cpu.fpu_set(idx, sti + st0);
    cpu.fpu_pop();
}

// FSUB: Subtract Real
// ST(0) = ST(0) - Src  OR  Dest = Dest - ST(0)
pub fn fsub(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        // FSUB [mem] -> ST(0) = ST(0) - [mem]
        let addr = calculate_addr(cpu, instr);
        let val = match instr.memory_size() {
            MemorySize::Float32 => f32::from_bits(cpu.bus.read_32(addr)) as f64,
            MemorySize::Float64 => f64::from_bits(cpu.bus.read_64(addr)),
            _ => 0.0,
        };
        let st0 = cpu.fpu_get(0);
        cpu.fpu_set(0, st0 - val);
    } else {
        let dst_reg = instr.op0_register();
        let src_reg = instr.op1_register();

        if dst_reg == Register::ST0 {
            // FSUB ST(0), ST(i) -> ST(0) = ST(0) - ST(i)
            let idx = src_reg.number() - Register::ST0.number();
            let st0 = cpu.fpu_get(0);
            let sti = cpu.fpu_get(idx as usize);
            cpu.fpu_set(0, st0 - sti);
        } else if src_reg == Register::ST0 {
            // FSUB ST(i), ST(0) -> ST(i) = ST(i) - ST(0)
            let idx = dst_reg.number() - Register::ST0.number();
            let sti = cpu.fpu_get(idx as usize);
            let st0 = cpu.fpu_get(0);
            cpu.fpu_set(idx as usize, sti - st0);
        }
    }
}

// FSUBP: Subtract and Pop
// ST(1) = ST(1) - ST(0); Pop ST(0)
pub fn fsubp(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0); // Source
    let st1 = cpu.fpu_get(1); // Destination
    
    cpu.fpu_set(1, st1 - st0);
    cpu.fpu_pop();
}

// FSUBR: Reverse Subtract
// ST(0) = Src - ST(0)  OR  Dest = ST(0) - Dest
pub fn fsubr(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        // FSUBR [mem] -> ST(0) = [mem] - ST(0)
        let addr = calculate_addr(cpu, instr);
        let val = match instr.memory_size() {
            MemorySize::Float32 => f32::from_bits(cpu.bus.read_32(addr)) as f64,
            MemorySize::Float64 => f64::from_bits(cpu.bus.read_64(addr)),
            _ => 0.0,
        };
        let st0 = cpu.fpu_get(0);
        cpu.fpu_set(0, val - st0);
    } else {
        let dst_reg = instr.op0_register();
        let src_reg = instr.op1_register();

        if dst_reg == Register::ST0 {
            // FSUBR ST(0), ST(i) -> ST(0) = ST(i) - ST(0)
            let idx = src_reg.number() - Register::ST0.number();
            let st0 = cpu.fpu_get(0);
            let sti = cpu.fpu_get(idx as usize);
            cpu.fpu_set(0, sti - st0);
        } else if src_reg == Register::ST0 {
            // FSUBR ST(i), ST(0) -> ST(i) = ST(0) - ST(i)
            let idx = dst_reg.number() - Register::ST0.number();
            let sti = cpu.fpu_get(idx as usize);
            let st0 = cpu.fpu_get(0);
            cpu.fpu_set(idx as usize, st0 - sti);
        }
    }
}

// FSUBRP: Reverse Subtract and Pop
// ST(1) = ST(0) - ST(1); Pop ST(0)
pub fn fsubrp(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    let st1 = cpu.fpu_get(1);
    
    cpu.fpu_set(1, st0 - st1);
    cpu.fpu_pop();
}

// FMUL: Multiply Real
pub fn fmul(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        let val = match instr.memory_size() {
            MemorySize::Float32 => f32::from_bits(cpu.bus.read_32(addr)) as f64,
            MemorySize::Float64 => f64::from_bits(cpu.bus.read_64(addr)),
            _ => 0.0,
        };
        let st0 = cpu.fpu_get(0);
        cpu.fpu_set(0, st0 * val);
    } else {
        let dst_reg = instr.op0_register();
        let src_reg = instr.op1_register();

        if dst_reg == Register::ST0 {
            let idx = src_reg.number() - Register::ST0.number();
            let st0 = cpu.fpu_get(0);
            let sti = cpu.fpu_get(idx as usize);
            cpu.fpu_set(0, st0 * sti);
        } else if src_reg == Register::ST0 {
            let idx = dst_reg.number() - Register::ST0.number();
            let sti = cpu.fpu_get(idx as usize);
            let st0 = cpu.fpu_get(0);
            cpu.fpu_set(idx as usize, sti * st0);
        }
    }
}

// FMULP: Multiply and Pop
pub fn fmulp(cpu: &mut Cpu, instr: &Instruction) {
    let dst_reg = instr.op0_register();
    let idx = if dst_reg == Register::None || dst_reg == Register::ST1 {
        1
    } else {
        (dst_reg.number() - Register::ST0.number()) as usize
    };

    let st0 = cpu.fpu_get(0);
    let sti = cpu.fpu_get(idx);
    cpu.fpu_set(idx, sti * st0);
    cpu.fpu_pop();
}

// FDIV: Floating Point Divide
pub fn fdiv(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        let divisor = match instr.memory_size() {
            MemorySize::Float32 => f32::from_bits(cpu.bus.read_32(addr)) as f64,
            MemorySize::Float64 => f64::from_bits(cpu.bus.read_64(addr)),
            _ => 1.0,
        };
        let st0 = cpu.fpu_get(0);
        if divisor != 0.0 {
            cpu.fpu_set(0, st0 / divisor);
        } else {
            cpu.fpu_set(0, f64::INFINITY);
        }
    } else {
        let dst_reg = instr.op0_register();
        let src_reg = instr.op1_register();

        if dst_reg == Register::ST0 {
            // FDIV ST(0), ST(i)
            let idx = src_reg.number() - Register::ST0.number();
            let sti = cpu.fpu_get(idx as usize);
            let st0 = cpu.fpu_get(0);
            if sti != 0.0 { cpu.fpu_set(0, st0 / sti); } else { cpu.fpu_set(0, f64::INFINITY); }
        } else {
            // FDIV ST(i), ST(0)
            let idx = dst_reg.number() - Register::ST0.number();
            let sti = cpu.fpu_get(idx as usize);
            let st0 = cpu.fpu_get(0);
            if st0 != 0.0 { cpu.fpu_set(idx as usize, sti / st0); } else { cpu.fpu_set(idx as usize, f64::INFINITY); }
        }
    }
}

// FDIVP: Divide and Pop
pub fn fdivp(cpu: &mut Cpu, instr: &Instruction) {
    let dst_reg = instr.op0_register();
    let idx = if dst_reg == Register::None || dst_reg == Register::ST1 {
        1
    } else {
        (dst_reg.number() - Register::ST0.number()) as usize
    };

    let st0 = cpu.fpu_get(0);
    let sti = cpu.fpu_get(idx);
    
    if st0 != 0.0 {
        cpu.fpu_set(idx, sti / st0);
    } else {
        cpu.fpu_set(idx, f64::INFINITY);
    }
    cpu.fpu_pop();
}

// FDIVR: Reverse Divide
pub fn fdivr(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        let val = match instr.memory_size() {
            MemorySize::Float32 => f32::from_bits(cpu.bus.read_32(addr)) as f64,
            MemorySize::Float64 => f64::from_bits(cpu.bus.read_64(addr)),
            _ => 1.0,
        };
        let st0 = cpu.fpu_get(0);
        if st0 != 0.0 {
            cpu.fpu_set(0, val / st0);
        } else {
            cpu.fpu_set(0, f64::INFINITY);
        }
    } else {
        let dst_reg = instr.op0_register();
        let src_reg = instr.op1_register();

        if dst_reg == Register::ST0 {
            // FDIVR ST(0), ST(i) -> ST(0) = ST(i) / ST(0)
            let idx = src_reg.number() - Register::ST0.number();
            let st0 = cpu.fpu_get(0);
            let sti = cpu.fpu_get(idx as usize);
            if st0 != 0.0 { cpu.fpu_set(0, sti / st0); } else { cpu.fpu_set(0, f64::INFINITY); }
        } else {
            // FDIVR ST(i), ST(0) -> ST(i) = ST(0) / ST(i)
            let idx = dst_reg.number() - Register::ST0.number();
            let sti = cpu.fpu_get(idx as usize);
            let st0 = cpu.fpu_get(0);
            if sti != 0.0 { cpu.fpu_set(idx as usize, st0 / sti); } else { cpu.fpu_set(idx as usize, f64::INFINITY); }
        }
    }
}

// FDIVRP: Reverse Divide and Pop
// ST(1) = ST(0) / ST(1); Pop ST(0)
pub fn fdivrp(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    let st1 = cpu.fpu_get(1);
    
    if st1 != 0.0 {
        cpu.fpu_set(1, st0 / st1);
    } else {
        cpu.fpu_set(1, f64::INFINITY);
    }
    cpu.fpu_pop();
}

// --- ADVANCED ARITHMETIC ---

// FPREM: Partial Remainder (Rounding toward Zero)
// ST(0) = ST(0) % ST(1)
pub fn fprem(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    let st1 = cpu.fpu_get(1);

    if st1 != 0.0 {
        let quotient = (st0 / st1).trunc() as i64;
        let remainder = st0 % st1;
        
        cpu.fpu_set(0, remainder);

        // Set C0, C1, C3 (Quotient bits)
        let q0 = (quotient & 1) != 0;
        let q1 = (quotient & 2) != 0;
        let q2 = (quotient & 4) != 0;

        cpu.fpu_status &= !0x4700; // Clear C0-C3
        if q2 { cpu.fpu_status |= FPU_C0; }
        if q0 { cpu.fpu_status |= FPU_C1; }
        if q1 { cpu.fpu_status |= FPU_C3; }
        // C2 cleared -> Complete
    }
}

// FPREM1: IEEE Partial Remainder (Rounding to Nearest)
// Difference from FPREM: Uses Round-to-Nearest for the quotient logic
pub fn fprem1(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    let st1 = cpu.fpu_get(1);

    if st1 != 0.0 {
        let quotient_f = st0 / st1;
        let quotient = quotient_f.round() as i64;
        let remainder = st0 - (quotient as f64 * st1);
        
        cpu.fpu_set(0, remainder);

        let q0 = (quotient & 1) != 0;
        let q1 = (quotient & 2) != 0;
        let q2 = (quotient & 4) != 0;

        cpu.fpu_status &= !0x4700;
        if q2 { cpu.fpu_status |= FPU_C0; }
        if q0 { cpu.fpu_status |= FPU_C1; }
        if q1 { cpu.fpu_status |= FPU_C3; }
    }
}

// FRNDINT: Round to Integer
pub fn frndint(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    let rc = (cpu.fpu_control >> 10) & 0x03;
    let result = match rc {
        0 => st0.round(),
        1 => st0.floor(),
        2 => st0.ceil(),
        3 => st0.trunc(),
        _ => st0,
    };
    cpu.fpu_set(0, result);
}

// FABS: Absolute Value
pub fn fabs(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    cpu.fpu_set(0, st0.abs());
    cpu.fpu_status &= !FPU_C1; // Clear C1 (Sign)
}

// FCHS: Change Sign
pub fn fchs(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    cpu.fpu_set(0, -st0);
    
    // Update C1 (Sign)
    if (-st0).is_sign_negative() {
        cpu.fpu_status |= FPU_C1;
    } else {
        cpu.fpu_status &= !FPU_C1;
    }
}

// FSQRT: Square Root
pub fn fsqrt(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    if st0 >= 0.0 {
        cpu.fpu_set(0, st0.sqrt());
    } else {
        // Invalid op for negative (should assume positive or handle error)
        // For HLE, just don't crash, or use abs? 
        // Real hardware sets Invalid Operation Exception.
        cpu.fpu_set(0, f64::NAN); 
    }
}

// FSCALE: Scale by 2^trunc(ST(1))
// ST(0) = ST(0) * 2^(trunc(ST(1)))
pub fn fscale(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    let st1 = cpu.fpu_get(1);
    
    // Truncate ST(1) towards zero to get the integer power
    let power = st1.trunc();
    
    // Calculate 2^power
    let scale = 2.0_f64.powf(power);
    
    cpu.fpu_set(0, st0 * scale);
}

// FXTRACT: Extract Exponent and Significand
// Separates ST(0) into exponent and significand.
// ST(0) becomes Exponent (unbiased), Push Significand.
pub fn fxtract(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    
    // Deconstruct float
    // Note: Rust doesn't have frexp in std easily, doing manually
    if st0 == 0.0 {
        // Zero handling: Exp = -inf, Sig = 0.0
        cpu.fpu_set(0, f64::NEG_INFINITY);
        cpu.fpu_push(0.0);
    } else {
        let exp = st0.abs().log2().floor();
        let sig = st0 / 2.0_f64.powf(exp);
        
        cpu.fpu_set(0, exp);
        cpu.fpu_push(sig);
    }
}

// F2XM1: 2^x - 1
pub fn f2xm1(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0);
    // x87 limits st0 to -1.0 to +1.0, but we can just calc it.
    cpu.fpu_set(0, 2.0f64.powf(st0) - 1.0);
}

// FYL2X: y * log2(x)
// ST(1) = ST(1) * log2(ST(0)); Pop ST(0)
pub fn fyl2x(cpu: &mut Cpu) {
    let x = cpu.fpu_get(0);
    let y = cpu.fpu_get(1);
    
    if x > 0.0 {
        let res = y * x.log2();
        cpu.fpu_set(1, res);
    } else {
        // Log(negative) is invalid
        cpu.fpu_set(1, f64::NEG_INFINITY); // or NaN
    }
    cpu.fpu_pop();
}

// FYL2XP1: y * log2(x + 1)
pub fn fyl2xp1(cpu: &mut Cpu) {
    let x = cpu.fpu_get(0);
    let y = cpu.fpu_get(1);
    
    let res = y * (x + 1.0).log2();
    cpu.fpu_set(1, res);
    cpu.fpu_pop();
}