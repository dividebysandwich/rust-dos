use iced_x86::{Instruction, OpKind, MemorySize, Register};
use crate::cpu::Cpu;
use crate::instructions::utils::calculate_addr;

// Convert x87 80-bit float to Rust f64
fn convert_f80_to_f64(mantissa: u64, sign_exp: u16) -> f64 {
    let sign = (sign_exp >> 15) & 1;
    let exp80 = sign_exp & 0x7FFF;

    // Handle Zero (Exp=0, Mantissa=0)
    if exp80 == 0 && mantissa == 0 {
        return if sign == 1 { -0.0 } else { 0.0 };
    }

    // Handle Infinity / NaN (Exp=Max)
    if exp80 == 0x7FFF {
        // If mantissa (excluding integer bit) is 0, it's Infinity
        if (mantissa & 0x7FFF_FFFF_FFFF_FFFF) == 0 {
            return if sign == 1 { f64::NEG_INFINITY } else { f64::INFINITY };
        } else {
            return f64::NAN;
        }
    }

    // Normal Numbers
    // Re-bias the exponent: 
    // 80-bit bias is 16383. 64-bit bias is 1023.
    let exp64 = (exp80 as i32) - 16383 + 1023;

    // Check for overflow/underflow
    if exp64 >= 2047 {
        return if sign == 1 { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    if exp64 <= 0 {
        // Subnormal or underflow to zero
        return 0.0; 
    }

    // Adjust Mantissa
    // 80-bit has an explicit integer bit at bit 63 (always 1 for normal numbers).
    // 64-bit has an implicit integer bit (assumed 1).
    // So we discard bit 63, and keep the next 52 bits (62 down to 11).
    let mantissa64 = (mantissa >> 11) & 0x000F_FFFF_FFFF_FFFF;

    // Avengers Assemble f64
    let bits = ((sign as u64) << 63) | ((exp64 as u64) << 52) | mantissa64;
    f64::from_bits(bits)
}

// FLD: Load Floating Point Value
pub fn fld(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        let val = match instr.memory_size() {
            MemorySize::Float32 => {
                let bits = cpu.bus.read_32(addr);
                f32::from_bits(bits) as f64
            }
            MemorySize::Float64 => {
                let bits = cpu.bus.read_64(addr);
                f64::from_bits(bits)
            }
            MemorySize::Float80 => {
                // 80-bit Layout: 
                // Byte 0-7: Mantissa (64 bits)
                // Byte 8-9: Sign (1 bit) + Exponent (15 bits)
                let mantissa = cpu.bus.read_64(addr);
                let sign_exp = cpu.bus.read_16(addr + 8);
                convert_f80_to_f64(mantissa, sign_exp)
            }
            _ => {
                cpu.bus.log_string(&format!("[FPU] FLD Unsupported memory size: {:?}", instr.memory_size()));
                0.0
            }
        };
        cpu.fpu_push(val);
    } else {
        // FLD ST(i) -> Push ST(i) onto stack
        // Register ST0 is 0, ST1 is 1, etc.
        let reg_offset = instr.op0_register().number() - Register::ST0.number();
        let val = cpu.fpu_get(reg_offset as usize);
        cpu.fpu_push(val);
    }
}

// FILD: Load Integer (Convert to Float and Push)
pub fn fild(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let val = match instr.memory_size() {
        MemorySize::Int16 => (cpu.bus.read_16(addr) as i16) as f64,
        MemorySize::Int32 => (cpu.bus.read_32(addr) as i32) as f64,
        MemorySize::Int64 => (cpu.bus.read_64(addr) as i64) as f64,
        _ => 0.0,
    };
    cpu.fpu_push(val);
}

// FISTP: Store Integer and Pop
// Respects FPU Control Word Rounding Bits (10-11)
pub fn fistp(cpu: &mut Cpu, instr: &Instruction) {
    let val = cpu.fpu_pop();
    let addr = calculate_addr(cpu, instr);
    
    // Extract Rounding Control (RC) - Bits 10 and 11
    let rc = (cpu.fpu_control >> 10) & 0x03;
    
    let i_val = match rc {
        0 => val.round(), // 00: Round to Nearest
        1 => val.floor(), // 01: Round Down (Toward -Infinity)
        2 => val.ceil(),  // 10: Round Up (Toward +Infinity)
        3 => val.trunc(), // 11: Round Toward Zero (Truncate)
        _ => val,         // Unreachable
    };

    match instr.memory_size() {
        MemorySize::Int16 => {
            // TODO: Check if just casting is okay here.
            cpu.bus.write_16(addr, (i_val as i16) as u16);
        }
        MemorySize::Int32 => {
            cpu.bus.write_32(addr, (i_val as i32) as u32);
        }
        MemorySize::Int64 => {
            // Write 64-bit int
            let v = i_val as i64 as u64;
            cpu.bus.write_32(addr, (v & 0xFFFFFFFF) as u32);
            cpu.bus.write_32(addr + 4, (v >> 32) as u32);
        }
        _ => {}
    }
}

// FSTP: Store Float and Pop
pub fn fstp(cpu: &mut Cpu, instr: &Instruction) {
    let val = cpu.fpu_pop();
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        match instr.memory_size() {
            MemorySize::Float32 => {
                let bits = (val as f32).to_bits();
                cpu.bus.write_32(addr, bits);
            }
            MemorySize::Float64 => {
                let bits = val.to_bits();
                cpu.bus.write_32(addr, (bits & 0xFFFFFFFF) as u32);
                cpu.bus.write_32(addr + 4, (bits >> 32) as u32);
            }
            _ => {}
        }
    }
}

// FST: Store Real (No POP)
pub fn fst(cpu: &mut Cpu, instr: &Instruction) {
    let st0 = cpu.fpu_get(0);
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        match instr.memory_size() {
            MemorySize::Float32 => {
                let bits = (st0 as f32).to_bits();
                cpu.bus.write_32(addr, bits);
            }
            MemorySize::Float64 => {
                let bits = st0.to_bits();
                cpu.bus.write_64(addr, bits);
            }
            _ => {}
        }
    }
}

// FXCH: Exchange Register Contents
pub fn fxch(cpu: &mut Cpu, instr: &Instruction) {
    let dst_reg = instr.op0_register();
    let idx = if dst_reg == Register::None || dst_reg == Register::ST1 {
        1
    } else {
        (dst_reg.number() - Register::ST0.number()) as usize
    };

    let st0 = cpu.fpu_get(0);
    let sti = cpu.fpu_get(idx);
    
    cpu.fpu_set(0, sti);
    cpu.fpu_set(idx, st0);
}

// FLD1: Push +1.0
pub fn fld1(cpu: &mut Cpu) {
    cpu.fpu_push(1.0);
}

// FIST: Store Integer (No Pop)
pub fn fist(cpu: &mut Cpu, instr: &Instruction) {
    let val = cpu.fpu_get(0);
    let addr = crate::instructions::utils::calculate_addr(cpu, instr);
    let rc = (cpu.fpu_control >> 10) & 0x03;
    let i_val = match rc {
        0 => val.round(), 1 => val.floor(), 2 => val.ceil(), 3 => val.trunc(), _ => val,
    };
    
    match instr.memory_size() {
        iced_x86::MemorySize::Int16 => { cpu.bus.write_16(addr, (i_val as i16) as u16); },
        iced_x86::MemorySize::Int32 => { cpu.bus.write_32(addr, (i_val as i32) as u32); },
        _ => {}
    }
}

// CONSTANTS
pub fn fldz(cpu: &mut Cpu)   { cpu.fpu_push(0.0); }
pub fn fldpi(cpu: &mut Cpu)  { cpu.fpu_push(std::f64::consts::PI); }
pub fn fldl2e(cpu: &mut Cpu) { cpu.fpu_push(std::f64::consts::LOG2_E); }
pub fn fldl2t(cpu: &mut Cpu) { cpu.fpu_push(std::f64::consts::LOG2_10); }
pub fn fldlg2(cpu: &mut Cpu) { cpu.fpu_push(std::f64::consts::LOG10_2); }
pub fn fldln2(cpu: &mut Cpu) { cpu.fpu_push(std::f64::consts::LN_2); }