use iced_x86::{Instruction, OpKind, MemorySize, Register};
use crate::cpu::Cpu;
use crate::f80::F80;
use crate::instructions::utils::calculate_addr;

// FLD: Load Floating Point Value
pub fn fld(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        let mut f = F80::new();
        
        match instr.memory_size() {
            MemorySize::Float32 => {
                let bits = cpu.bus.read_32(addr);
                f.set_f64(f32::from_bits(bits) as f64);
            }
            MemorySize::Float64 => {
                let bits = cpu.bus.read_64(addr);
                f.set_f64(f64::from_bits(bits));
            }
            MemorySize::Float80 => {
                // Load 10 bytes directly from memory without lossy conversion
                let mut bytes = [0u8; 10];
                for i in 0..10 {
                    bytes[i] = cpu.bus.read_8(addr + i as usize);
                }
                f.set_bytes(&bytes);
            }
            _ => {
                cpu.bus.log_string(&format!("[FPU] FLD Unsupported memory size: {:?}", instr.memory_size()));
            }
        };
        cpu.fpu_push(f);
    } else {
        // FLD ST(i) -> Push ST(i) onto stack
        let reg_offset = (instr.op0_register().number() - Register::ST0.number()) as usize;
        let val = cpu.fpu_get(reg_offset);
        cpu.fpu_push(val);
    }
}

// FILD: Load Integer (Convert to Float and Push)
pub fn fild(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let f = cpu.load_int_to_f80(addr, instr.memory_size());
    cpu.fpu_push(f);
}

fn x87_round(f_val: f64, rc: u16) -> f64 {
    match rc {
        0 => {
            // Round to nearest, ties to even
            let floor_val = f_val.floor();
            let diff = f_val - floor_val;
            if diff < 0.5 {
                floor_val
            } else if diff > 0.5 {
                floor_val + 1.0
            } else {
                // Tie: round to even
                if floor_val % 2.0 == 0.0 { floor_val } else { floor_val + 1.0 }
            }
        },
        1 => f_val.floor(), // Round Down
        2 => f_val.ceil(),  // Round Up
        3 => f_val.trunc(), // Truncate
        _ => unreachable!(),
    }
}

// FISTP: Store Integer and Pop
pub fn fistp(cpu: &mut Cpu, instr: &Instruction) {
    let val = cpu.fpu_pop();
    let addr = calculate_addr(cpu, instr);
    
    // Use the custom rounding logic
    let rc = (cpu.fpu_control >> 10) & 0x03;
    let rounded = x87_round(val.get_f64(), rc);

    match instr.memory_size() {
        MemorySize::Int16 => { cpu.bus.write_16(addr, rounded as i16 as u16); },
        MemorySize::Int32 => { cpu.bus.write_32(addr, rounded as i32 as u32); },
        MemorySize::Int64 => { cpu.bus.write_64(addr, rounded as i64 as u64); },
        _ => {}
    }
}

// FSTP: Store Float and Pop
pub fn fstp(cpu: &mut Cpu, instr: &Instruction) {
    let val: F80 = cpu.fpu_pop();
    
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        cpu.last_fstp_addr = addr;

        match instr.memory_size() {
            MemorySize::Float32 => {
                let bits = (val.get_f64() as f32).to_bits();
                cpu.bus.write_32(addr, bits);
            }
            MemorySize::Float64 => {
                let bits = val.get_f64().to_bits();
                cpu.bus.write_64(addr, bits);
            }
            MemorySize::Float80 => {
                let bytes = val.get_bytes();
                for i in 0..10 {
                    cpu.bus.write_8(addr + i as usize, bytes[i]);
                }
            }
            _ => { cpu.bus.log_string(&format!("[FPU] FSTP Unsupported memory size: {:?}", instr.memory_size())); }
        }
    } else if instr.op0_kind() == OpKind::Register {
        // FSTP ST(i)
        let idx = (instr.op0_register().number() - Register::ST0.number()) as usize;
        cpu.fpu_set(idx, val);
    }
}

// FBSTP: Store BCD Integer and Pop
pub fn fbstp(cpu: &mut Cpu, instr: &Instruction) {
    let val: F80 = cpu.fpu_pop();
    let addr = calculate_addr(cpu, instr);

    let bcd_bytes = val.to_bcd_packed();

    // Write the 10-byte BCD block to Memory
    for i in 0..10 {
        cpu.bus.write_8(addr + i as usize, bcd_bytes[i]);
    }
}

// FST: Store Real (No POP)
pub fn fst(cpu: &mut Cpu, instr: &Instruction) {
    let st0: F80 = cpu.fpu_get(0);
    
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        match instr.memory_size() {
            MemorySize::Float32 => {
                let bits = (st0.get_f64() as f32).to_bits();
                cpu.bus.write_32(addr, bits);
            }
            MemorySize::Float64 => {
                let bits = st0.get_f64().to_bits();
                cpu.bus.write_64(addr, bits);
            }
            MemorySize::Float80 => {
                let bytes = st0.get_bytes();
                for i in 0..10 {
                    cpu.bus.write_8(addr + i as usize, bytes[i]);
                }
            }
            _ => { cpu.bus.log_string(&format!("[FPU] FST Unsupported memory size: {:?}", instr.memory_size())); }
        }
    } else if instr.op0_kind() == OpKind::Register {
        // FST ST(i)
        let idx = (instr.op0_register().number() - Register::ST0.number()) as usize;
        cpu.fpu_set(idx, st0);
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
    let mut f = F80::new();
    // 1.0 in F80: Sign=0, Exp=0x3FFF (16383 bias), Mantissa=0x8000000000000000
    f.set_exponent(16383);
    f.set_mantissa(1u64 << 63); 
    cpu.fpu_push(f);
}

pub fn fldz(cpu: &mut Cpu) {
    cpu.fpu_push(F80::new()); // F80::new() initializes to 0.0
}

pub fn fldpi(cpu: &mut Cpu) {
    cpu.fpu_push(F80::PI());
}

pub fn fldl2e(cpu: &mut Cpu) {
    let mut f = F80::new();
    // log2(e) bit pattern: 0x3FFF B8AA 3B29 5C17 F0BB
    f.set(0x3FFFB8AA3B295C17F0BB);
    cpu.fpu_push(f);
}

pub fn fldl2t(cpu: &mut Cpu) {
    let mut f = F80::new();
    // log2(10) bit pattern: 0x4000 D49A 784B CD1B 8AFE
    f.set(0x4000D49A784BCD1B8AFE);
    cpu.fpu_push(f);
}

pub fn fldlg2(cpu: &mut Cpu) {
    let mut f = F80::new();
    // log10(2) bit pattern: 0x3FFD 9A20 9A84 FBCF 7980
    f.set(0x3FFD9A209A84FBCF7980);
    cpu.fpu_push(f);
}

pub fn fldln2(cpu: &mut Cpu) {
    let mut f = F80::new();
    // ln(2) bit pattern: 0x3FFE B172 17F7 D1CF 79AC
    f.set(0x3FFEB17217F7D1CF79AC);
    cpu.fpu_push(f);
}


// FIST: Store Integer (No Pop)
pub fn fist(cpu: &mut Cpu, instr: &Instruction) {
    let val = cpu.fpu_get(0);
    let addr = calculate_addr(cpu, instr);
    
    // Extract Rounding Control (RC) - Bits 10 and 11
    let rc = (cpu.fpu_control >> 10) & 0x03;
    
    // Use the x87-compliant rounding helper
    let f_val = val.get_f64();
    let i_val = x87_round(f_val, rc);
    
    match instr.memory_size() {
        MemorySize::Int16 => {
            cpu.bus.write_16(addr, i_val as i16 as u16);
        },
        MemorySize::Int32 => {
            cpu.bus.write_32(addr, i_val as i32 as u32);
        },
        _ => {
            cpu.bus.log_string(&format!("[FPU] FIST Unsupported memory size: {:?}", instr.memory_size()));
        }
    }
}