use iced_x86::{Instruction, OpKind, MemorySize, Register};
use crate::cpu::Cpu;
use crate::instructions::utils::calculate_addr;

pub fn convert_f80_to_f64(mantissa: u64, sign_exp: u16) -> f64 {
    let sign = (sign_exp >> 15) & 1;
    let exp80 = sign_exp & 0x7FFF;
    
    // Zero
    if exp80 == 0 && mantissa == 0 {
        return if sign == 1 { -0.0 } else { 0.0 };
    }

    // Infinity / NaN
    if exp80 == 0x7FFF {
        if (mantissa & 0x7FFFFFFFFFFFFFFF) == 0 {
            return if sign == 1 { f64::NEG_INFINITY } else { f64::INFINITY };
        }
        return f64::NAN;
    }

    // Normal Numbers
    let exp64_signed = (exp80 as i32) - 16383 + 1023;

    if exp64_signed <= 0 {
        // Subnormal or Zero
        return if sign == 1 { -0.0 } else { 0.0 };
    }
    if exp64_signed >= 0x7FF {
        return if sign == 1 { f64::NEG_INFINITY } else { f64::INFINITY };
    }

    // Mantissa
    // Discard Integer Bit (63), take next 52 bits (62..11)
    let mantissa64 = (mantissa >> 11) & 0xFFFFFFFFFFFFF;
    
    let bits = ((sign as u64) << 63) | ((exp64_signed as u64) << 52) | mantissa64;
    f64::from_bits(bits)
}

pub fn convert_f64_to_f80(val: f64) -> (u64, u16) {
    let bits = val.to_bits();
    let sign = (bits >> 63) as u16;
    let exp64 = ((bits >> 52) & 0x7FF) as i16;
    let mantissa64 = bits & 0xFFFFFFFFFFFFF;

    // Zero / Denormal (Treat denormal as zero for simplicity)
    if exp64 == 0 {
        return (0, sign << 15);
    }

    // Infinity / NaN
    if exp64 == 0x7FF {
        // x87 Infinity/NaN has Integer Bit=1 (0x8000...)
        let mantissa80 = (1u64 << 63) | (mantissa64 << 11);
        return (mantissa80, (sign << 15) | 0x7FFF);
    }

    // Normal Number
    // Rebias: 1023 -> 16383
    let exp80 = (exp64 - 1023 + 16383) as u16;
    
    // Mantissa:
    // f64 (52 bits) -> f80 (64 bits).
    // Shift left 11 positions.
    // Set Explicit Integer Bit 63 (which is implicit in f64).
    let mantissa80 = (1u64 << 63) | (mantissa64 << 11);

    (mantissa80, (sign << 15) | exp80)
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
        _ => { cpu.bus.log_string(&format!("[FPU] FISTP Unsupported memory size: {:?}", instr.memory_size())); }
    }
}

// FSTP: Store Float and Pop
pub fn fstp(cpu: &mut Cpu, instr: &Instruction) {
    let val = cpu.fpu_pop();
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        
        // REMOVEME - QB Float debugging
        cpu.last_fstp_addr = addr;

        match instr.memory_size() {
            MemorySize::Float32 => {
                let bits = (val as f32).to_bits();
                cpu.bus.write_32(addr, bits);
            }
            MemorySize::Float64 => {
                let bits = val.to_bits();
                cpu.bus.write_64(addr, bits);
                // cpu.bus.write_32(addr, (bits & 0xFFFFFFFF) as u32);
                // cpu.bus.write_32(addr + 4, (bits >> 32) as u32);
            }
            MemorySize::Float80 => {
                //let val = cpu.fpu_pop();
                let (m, se) = convert_f64_to_f80(val);
                println!("[FPU] FSTP80: Val={} -> Mantissa={:X}, SignExp={:X}", val, m, se);
                cpu.bus.write_64(addr, m);
                cpu.bus.write_16(addr + 8, se);
            }
            _ => { cpu.bus.log_string(&format!("[FPU] FSTP Unsupported memory size: {:?}", instr.memory_size())); }
        }
    }
}

// FBSTP: Store BCD Integer and Pop
pub fn fbstp(cpu: &mut Cpu, instr: &Instruction) {
    let val = cpu.fpu_pop();
    let addr = calculate_addr(cpu, instr);

    // Handle Sign
    let is_neg = val.is_sign_negative();
    let abs_val = val.abs();

    // Round to u64
    let int_val = (abs_val.round()) as u64;

    // Check limits (18 decimal digits approx 10^18)
    if int_val >= std::u64::MAX / 10 {
        // Overflow / Indefinite
        // Write "Indefinite" BCD pattern? Or just saturate.
        // For now, let's saturate or just panic log.
        cpu.bus.log_string("[FPU] FBSTP Overflow");
        return; 
    }

    // Create 10-byte BCD array
    // Bytes 0-8: 18 digits (2 digits per byte)
    // Byte 9: Sign bit (0x80 if negative, 0x00 if positive)
    let mut bcd_bytes = [0u8; 10];
    
    let mut temp = int_val;
    for i in 0..9 {
        let digit_low = temp % 10;
        temp /= 10;
        let digit_high = temp % 10;
        temp /= 10;
        
        bcd_bytes[i] = (digit_high as u8) << 4 | (digit_low as u8);
    }

    if is_neg {
        bcd_bytes[9] = 0x80;
    }

    // Write to Memory
    for i in 0..10 {
        cpu.bus.write_8(addr + i, bcd_bytes[i]);
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
            MemorySize::Float80 => {
                let (m, se) = convert_f64_to_f80(st0);
                cpu.bus.write_64(addr, m);
                cpu.bus.write_16(addr + 8, se);
            }
            _ => { cpu.bus.log_string(&format!("[FPU] FST Unsupported memory size: {:?}", instr.memory_size())); }
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
        _ => { cpu.bus.log_string(&format!("[FPU] FIST Unsupported memory size: {:?}", instr.memory_size())); }
    }
}

// CONSTANTS
pub fn fldz(cpu: &mut Cpu)   { cpu.fpu_push(0.0); }
pub fn fldpi(cpu: &mut Cpu)  { cpu.fpu_push(std::f64::consts::PI); }
pub fn fldl2e(cpu: &mut Cpu) { cpu.fpu_push(std::f64::consts::LOG2_E); }
pub fn fldl2t(cpu: &mut Cpu) { cpu.fpu_push(std::f64::consts::LOG2_10); }
pub fn fldlg2(cpu: &mut Cpu) { cpu.fpu_push(std::f64::consts::LOG10_2); }
pub fn fldln2(cpu: &mut Cpu) { cpu.fpu_push(std::f64::consts::LN_2); }