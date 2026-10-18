use crate::cpu::{Cpu, FpuFlags};
use crate::f80::{BCD_INDEFINITE, F80};
use crate::instructions::utils::calculate_addr;
use iced_x86::{Instruction, MemorySize, OpKind, Register};

// FLD: Load Floating Point Value
pub fn fld(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        let mut f = F80::new();

        match instr.memory_size() {
            MemorySize::Float32 => {
                let bits = cpu.lin_read_32(addr);
                cpu.fpu_push_f64(f32::from_bits(bits) as f64);
                return;
            }
            MemorySize::Float64 => {
                let bits = cpu.lin_read_64(addr);
                cpu.fpu_push_f64(f64::from_bits(bits));
                return;
            }
            MemorySize::Float80 => {
                // Load 10 bytes directly from memory without lossy conversion
                let mut bytes = [0u8; 10];
                for (i, byte) in bytes.iter_mut().enumerate() {
                    *byte = cpu.lin_read_8(addr + i);
                }
                f.set_bytes(&bytes);
            }
            _ => {
                cpu.bus.log_string(&format!(
                    "[FPU] FLD Unsupported memory size: {:?}",
                    instr.memory_size()
                ));
            }
        };
        cpu.fpu_push(f);
    } else {
        // FLD ST(i) -> Push ST(i) onto stack
        let reg_offset = instr.op0_register().number() - Register::ST0.number();
        let val = cpu.fpu_get(reg_offset);
        cpu.fpu_push(val);
    }
}

// FILD: Load Integer (Convert to Float and Push)
pub fn fild(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    // (A word or a dword as a double has the 80 bits `load_int_to_f80`
    // gives it.)
    match instr.memory_size() {
        MemorySize::Int16 => {
            let v = cpu.lin_read_16(addr) as i16;
            cpu.fpu_push_f64(v as f64);
        }
        MemorySize::Int32 => {
            let v = cpu.lin_read_32(addr) as i32;
            cpu.fpu_push_f64(v as f64);
        }
        size => {
            let f = cpu.load_int_to_f80(addr, size);
            cpu.fpu_push(f);
        }
    }
}

// FBLD: Load BCD Integer (Convert to Float and Push)
pub fn fbld(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    let mut bcd = [0u8; 10];
    for (i, byte) in bcd.iter_mut().enumerate() {
        *byte = cpu.lin_read_8(addr + i);
    }
    let mut f = F80::new();
    f.from_bcd_packed(&bcd);
    cpu.fpu_push(f);
}

/// A double as the word or dword (`size` 2 or 4) FIST and FISTP store,
/// rounded as the control word says. The dynamic recompiler's code calls
/// it for what its own conversion doesn't cover.
pub fn to_int(cpu: &mut Cpu, value: f64, size: u8) -> u32 {
    let n = int_of_double(value, cpu.fpu_control);
    int_or_indefinite(cpu, n, size) as u32
}

/// A double rounded to an integer as the control word says; None for a NaN.
/// (Past i128's range it saturates, which no integer store holds either.)
fn int_of_double(value: f64, control: u16) -> Option<i128> {
    let rounded = x87_round(value, (control >> 10) & 0x03);
    (!rounded.is_nan()).then_some(rounded as i128)
}

/// What FIST and FISTP store of `size` bytes (2, 4 or 8) for integer `n`,
/// None for a NaN: one that doesn't fit, or a NaN, is an invalid operation,
/// and with it masked the FPU stores the integer indefinite, the smallest
/// integer of the size.
fn int_or_indefinite(cpu: &mut Cpu, n: Option<i128>, size: u8) -> u64 {
    let bits = 8 * size as u32;
    let min = -(1i128 << (bits - 1));
    match n {
        Some(n) if n >= min && n < -min => n as u64 & (u64::MAX >> (64 - bits)),
        _ => {
            cpu.set_fpu_flag(FpuFlags::IE, true);
            min as u64 & (u64::MAX >> (64 - bits))
        }
    }
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
                if floor_val % 2.0 == 0.0 {
                    floor_val
                } else {
                    floor_val + 1.0
                }
            }
        }
        1 => f_val.floor(), // Round Down
        2 => f_val.ceil(),  // Round Up
        3 => f_val.trunc(), // Truncate
        _ => unreachable!(),
    }
}

/// The integer a register whose 80 bits hold more than its double
/// (`FpuRegs::beyond_double`) rounds to, from all 80 bits as the control
/// word says, for `int_or_indefinite`. None where the double's rounding
/// stands (`F80::round_int`). From 2^63 on (where the double could round
/// a value under -2^63 to it) the value is past any integer store but
/// -2^63's.
fn int_of_80_bits(value: F80, control: u16) -> Option<i128> {
    if let Some(n) = value.round_int((control >> 10) & 0x03) {
        return Some(n);
    }
    let exp = value.get_exponent() as i32 - crate::f80::F80_EXP_BIAS;
    if !value.is_normal() || value.get_integer_bit() == 0 || exp < 63 {
        return None;
    }
    let exact_min = value.get_sign() && exp == 63 && value.get_mantissa() == 1 << 63;
    Some(if exact_min { i64::MIN as i128 } else if value.get_sign() { i128::MIN } else { i128::MAX })
}

// FISTP: Store Integer and Pop
pub fn fistp(cpu: &mut Cpu, instr: &Instruction) {
    // (The register's value whatever its tag.)
    let top = cpu.fpu_top;
    let val = cpu.fpu_reg_f64(top);
    let precise = cpu.fpu_reg_beyond_double(top).then(|| cpu.fpu_reg(top));
    cpu.fpu_drop();
    let addr = calculate_addr(cpu, instr);
    let size = match instr.memory_size() {
        MemorySize::Int16 => 2,
        MemorySize::Int32 => 4,
        MemorySize::Int64 => 8,
        _ => return,
    };
    let control = cpu.fpu_control;
    let n = precise.and_then(|f| int_of_80_bits(f, control)).or_else(|| int_of_double(val, control));
    let stored = int_or_indefinite(cpu, n, size);
    match size {
        2 => cpu.lin_write_16(addr, stored as u16),
        4 => cpu.lin_write_32(addr, stored as u32),
        _ => cpu.lin_write_64(addr, stored),
    }
}

/// The single FST and FSTP store of a register with double `double`, and
/// with `precise` its 80 bits where they hold more than the double
/// (`FpuRegs::beyond_double`): those rounded once, to 24 bits, as the
/// control word says. (Rounding the double, itself rounded to nearest,
/// could round twice.)
fn single_bits(precise: Option<F80>, double: f64, control: u16) -> u32 {
    match precise.and_then(|f| f.round_to(24, (control >> 10) & 0x03)) {
        Some(value) => (value as f32).to_bits(),
        None => (double as f32).to_bits(),
    }
}

/// The double FST and FSTP store, as `single_bits` has it. The register's
/// double is its 80 bits rounded to nearest: under another rounding they
/// are rounded again, as the control word says.
fn double_bits(precise: Option<F80>, double: f64, control: u16) -> u64 {
    let rc = (control >> 10) & 0x03;
    match precise.filter(|_| rc != 0).and_then(|f| f.round_to(53, rc)) {
        Some(value) => value.to_bits(),
        None => double.to_bits(),
    }
}

// FSTP: Store Float and Pop
pub fn fstp(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        // (The register's value whatever its tag.)
        let double = cpu.fpu_reg_f64(cpu.fpu_top);
        let precise = cpu.fpu_reg_beyond_double(cpu.fpu_top).then(|| cpu.fpu_reg(cpu.fpu_top));
        if instr.memory_size() == MemorySize::Float32 {
            cpu.fpu_drop();
            cpu.lin_write_32(addr, single_bits(precise, double, cpu.fpu_control));
            return;
        }
        if instr.memory_size() == MemorySize::Float64 {
            cpu.fpu_drop();
            cpu.lin_write_64(addr, double_bits(precise, double, cpu.fpu_control));
            return;
        }
        let val: F80 = cpu.fpu_pop();

        match instr.memory_size() {
            MemorySize::Float32 => {
                let bits = (val.get_f64() as f32).to_bits();
                cpu.lin_write_32(addr, bits);
            }
            MemorySize::Float64 => {
                let bits = val.get_f64().to_bits();
                cpu.lin_write_64(addr, bits);
            }
            MemorySize::Float80 => {
                let bytes = val.get_bytes();
                for (i, &byte) in bytes.iter().enumerate() {
                    cpu.lin_write_8(addr + i, byte);
                }
            }
            _ => {
                cpu.bus.log_string(&format!(
                    "[FPU] FSTP Unsupported memory size: {:?}",
                    instr.memory_size()
                ));
            }
        }
    } else if instr.op0_kind() == OpKind::Register {
        // FSTP ST(i)
        // Store ST(0) to ST(i), THEN pop.
        let val = cpu.fpu_get(0);
        let idx = instr.op0_register().number() - Register::ST0.number();
        cpu.fpu_set_valid(idx, val);
        cpu.fpu_pop();
    }
}

// FBSTP: Store BCD Integer and Pop
pub fn fbstp(cpu: &mut Cpu, instr: &Instruction) {
    let val: F80 = cpu.fpu_pop();
    let addr = calculate_addr(cpu, instr);

    let bcd_bytes = val
        .to_bcd_packed((cpu.fpu_control >> 10) & 0x03)
        .unwrap_or_else(|| {
            cpu.set_fpu_flag(FpuFlags::IE, true);
            BCD_INDEFINITE
        });

    // Write the 10-byte BCD block to Memory
    for (i, &byte) in bcd_bytes.iter().enumerate() {
        cpu.lin_write_8(addr + i, byte);
    }
}

// FST: Store Real (No POP)
pub fn fst(cpu: &mut Cpu, instr: &Instruction) {
    let st0: F80 = cpu.fpu_get(0);
    let precise = cpu.fpu_beyond_double(0).then_some(st0);

    if instr.op0_kind() == OpKind::Memory {
        let addr = calculate_addr(cpu, instr);
        match instr.memory_size() {
            MemorySize::Float32 => {
                let bits = single_bits(precise, st0.get_f64(), cpu.fpu_control);
                cpu.lin_write_32(addr, bits);
            }
            MemorySize::Float64 => {
                let bits = double_bits(precise, st0.get_f64(), cpu.fpu_control);
                cpu.lin_write_64(addr, bits);
            }
            MemorySize::Float80 => {
                let bytes = st0.get_bytes();
                for (i, &byte) in bytes.iter().enumerate() {
                    cpu.lin_write_8(addr + i, byte);
                }
            }
            _ => {
                cpu.bus.log_string(&format!(
                    "[FPU] FST Unsupported memory size: {:?}",
                    instr.memory_size()
                ));
            }
        }
    } else if instr.op0_kind() == OpKind::Register {
        // FST ST(i)
        let idx = instr.op0_register().number() - Register::ST0.number();
        cpu.fpu_set_valid(idx, st0);
    }
}

/// The register FXCH exchanges ST(0) with.
pub fn fxch_index(instr: &Instruction) -> usize {
    // The last operand, as iced lists ST(0) first; ST(1) if implicit.
    // FXCH ST(0) exchanges ST(0) with itself.
    match instr.op_register(instr.op_count().saturating_sub(1)) {
        reg if (Register::ST0..=Register::ST7).contains(&reg) => reg.number() - Register::ST0.number(),
        _ => 1,
    }
}

// FXCH: Exchange Register Contents
pub fn fxch(cpu: &mut Cpu, instr: &Instruction) {
    let idx = fxch_index(instr);

    // Optimization: Swapping ST(0) with ST(0) is a NOP
    if idx == 0 {
        return;
    }

    // An empty register reads as the real indefinite (a masked stack
    // underflow, IE), and both come out valid.
    if cpu.fpu_is_empty(0) || cpu.fpu_is_empty(idx) {
        let (st0, sti) = (cpu.fpu_get(0), cpu.fpu_get(idx));
        cpu.fpu_set_valid(0, sti);
        cpu.fpu_set_valid(idx, st0);
        cpu.set_fpu_flag(FpuFlags::IE | FpuFlags::SF, true);
    } else if cpu.fpu_fast {
        // Only the doubles: the 80 bits become theirs.
        let st0 = cpu.fpu_get_f64(0);
        let sti = cpu.fpu_get_f64(idx);
        cpu.fpu_set_f64(0, sti);
        cpu.fpu_set_f64(idx, st0);
    } else {
        let st0 = cpu.fpu_get(0);
        let sti = cpu.fpu_get(idx);
        cpu.fpu_set(0, sti);
        cpu.fpu_set(idx, st0);
    }

    // C1 is usually cleared by FXCH on modern processors
    cpu.set_fpu_flag(crate::cpu::FpuFlags::C1, false);
}

/// FCMOVcc ST(0), ST(i) (P6): ST(i) into ST(0) if the condition on CF, ZF
/// and PF holds (B, E, BE, U and their negations).
pub fn fcmov(cpu: &mut Cpu, instr: &Instruction) {
    use crate::cpu::CpuFlags;
    use iced_x86::Mnemonic::*;
    let f = |flag| cpu.get_cpu_flag(flag);
    let holds = match instr.mnemonic() {
        Fcmovb => f(CpuFlags::CF),
        Fcmove => f(CpuFlags::ZF),
        Fcmovbe => f(CpuFlags::CF) || f(CpuFlags::ZF),
        Fcmovu => f(CpuFlags::PF),
        Fcmovnb => !f(CpuFlags::CF),
        Fcmovne => !f(CpuFlags::ZF),
        Fcmovnbe => !f(CpuFlags::CF) && !f(CpuFlags::ZF),
        _ => !f(CpuFlags::PF),
    };
    cpu.set_fpu_flag(crate::cpu::FpuFlags::C1, false);
    if !holds {
        return;
    }
    let idx = (instr.op1_register().number() - Register::ST0.number()) & 7;
    if cpu.fpu_fast {
        let sti = cpu.fpu_get_f64(idx);
        cpu.fpu_set_f64(0, sti);
    } else {
        let sti = cpu.fpu_get(idx);
        cpu.fpu_set(0, sti);
    }
    cpu.fpu_tags[cpu.fpu_top] = crate::cpu::FPU_TAG_VALID;
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
    let precise = cpu.fpu_beyond_double(0);
    let addr = calculate_addr(cpu, instr);
    let f_val = val.get_f64();
    let control = cpu.fpu_control;
    let n = precise.then(|| int_of_80_bits(val, control)).flatten().or_else(|| int_of_double(f_val, control));

    match instr.memory_size() {
        MemorySize::Int16 => {
            let int = int_or_indefinite(cpu, n, 2);
            cpu.lin_write_16(addr, int as u16);
        }
        MemorySize::Int32 => {
            let int = int_or_indefinite(cpu, n, 4);
            cpu.lin_write_32(addr, int as u32);
        }
        _ => {
            cpu.bus.log_string(&format!(
                "[FPU] FIST Unsupported memory size: {:?}",
                instr.memory_size()
            ));
        }
    }
}
