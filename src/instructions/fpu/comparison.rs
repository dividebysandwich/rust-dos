use iced_x86::{Code, Instruction, Mnemonic, OpKind, MemorySize, Register};

use std::cmp::Ordering;

use crate::cpu::{Cpu, FpuFlags, CpuFlags, FPU_TAG_EMPTY};
use crate::f80::F80;
use crate::instructions::utils::calculate_addr;

// Performs the FPU comparison and sets Status Word flags
// Used by FCOM, FCOMP, FCOMPP
fn fpu_compare_values(cpu: &mut Cpu, lhs: f64, rhs: f64) {
    // Clear C0, C2, C3
    cpu.set_fpu_flag(FpuFlags::C0 | FpuFlags::C2 | FpuFlags::C3, false);

    if lhs.is_nan() || rhs.is_nan() {
        // Unordered: C3=1, C2=1, C0=1
        cpu.set_fpu_flag(FpuFlags::C0 | FpuFlags::C2 | FpuFlags::C3, true);
    } else if lhs == rhs {
        // Equal: C3=1
        cpu.set_fpu_flag(FpuFlags::C3, true);
    } else if lhs < rhs {
        // Less Than: C0=1
        cpu.set_fpu_flag(FpuFlags::C0, true);
    }
    // Greater Than: All flags 0
}

/// Compare two values by their 80 bits (`F80::compare`), given where
/// either holds more than its double (`precise`): the doubles are rounded
/// to nearest, so a value just below a double could compare equal to it.
/// False where the doubles' comparison stands.
fn fpu_compare_precise(cpu: &mut Cpu, precise: Option<(F80, F80)>) -> bool {
    let Some(order) = precise.and_then(|(lhs, rhs)| lhs.compare(&rhs)) else {
        return false;
    };
    cpu.set_fpu_flag(FpuFlags::C0 | FpuFlags::C2 | FpuFlags::C3, false);
    match order {
        Ordering::Equal => cpu.set_fpu_flag(FpuFlags::C3, true),
        Ordering::Less => cpu.set_fpu_flag(FpuFlags::C0, true),
        Ordering::Greater => {}
    }
    true
}

/// A double from memory as 80 bits, where a register holds it as it is (0
/// or a normal number); None otherwise.
fn f80_of(value: f64) -> Option<F80> {
    (value == 0.0 || value.is_normal()).then(|| {
        let mut f = F80::new();
        f.set_f64(value);
        f
    })
}

/// The registers FCOM, FUCOM and their popping forms compare, ST(lhs)
/// with ST(rhs), and how many they pop; None for a memory operand.
pub fn fcom_registers(instr: &Instruction) -> Option<(usize, usize, u8)> {
    let pops = match instr.mnemonic() {
        Mnemonic::Fcomp | Mnemonic::Fucomp => 1,
        Mnemonic::Fcompp | Mnemonic::Fucompp => 2,
        _ => 0,
    };
    if pops == 2 {
        // FCOMPP is always ST(0) vs ST(1)
        return Some((0, 1, pops));
    }
    match instr.op0_kind() {
        OpKind::Memory => None,
        OpKind::Register => {
            // The DC D0+i and DC D8+i encodings compare the other way
            // round (ST(i) with ST(0)).
            let dc_form = matches!(instr.code(), Code::Fcom_st0_sti_DCD0 | Code::Fcomp_st0_sti_DCD8);

            // Identify the operand register index (i)
            // iced_x86 might say "FCOM ST0, ST1" or "FCOM ST1, ST0"
            // We need the register that is NOT ST0 to find 'i'.
            let reg_op0 = instr.op0_register();
            let reg_op1 = instr.op1_register();

            let idx = if reg_op0 != Register::ST0 && reg_op0 != Register::None {
                reg_op0.number() - Register::ST0.number()
            } else if reg_op1 != Register::ST0 && reg_op1 != Register::None {
                reg_op1.number() - Register::ST0.number()
            } else {
                1 // Default to ST(1) if parsing fails or implicit
            } as usize;
            if idx > 7 {
                return None;
            }
            Some(if dc_form { (idx, 0, pops) } else { (0, idx, pops) })
        }
        _ => Some((0, 1, pops)),
    }
}

pub fn fcom_variants(cpu: &mut Cpu, instr: &Instruction) {
    // The doubles, and the 80 bits where either register holds more.
    let (lhs, rhs, precise) = match fcom_registers(instr) {
        Some((lhs, rhs, _)) => {
            let precise = (cpu.fpu_beyond_double(lhs) || cpu.fpu_beyond_double(rhs))
                .then(|| (cpu.fpu_get(lhs), cpu.fpu_get(rhs)));
            (cpu.fpu_get_f64(lhs), cpu.fpu_get_f64(rhs), precise)
        }
        None => {
            // Memory Comparison is ALWAYS ST(0) vs Memory
            let val_0 = cpu.fpu_get_f64(0);
            let addr = calculate_addr(cpu, instr);
            let val_op = match instr.memory_size() {
                MemorySize::Float32 => f32::from_bits(cpu.lin_read_32(addr)) as f64,
                MemorySize::Float64 => f64::from_bits(cpu.lin_read_64(addr)),
                _ => f64::NAN,
            };
            let precise = cpu.fpu_beyond_double(0).then(|| f80_of(val_op).map(|op| (cpu.fpu_get(0), op))).flatten();
            (val_0, val_op, precise)
        }
    };

    if !fpu_compare_precise(cpu, precise) {
        fpu_compare_values(cpu, lhs, rhs);
    }

    match instr.mnemonic() {
        Mnemonic::Fcomp | Mnemonic::Fucomp => cpu.fpu_drop(),
        Mnemonic::Fcompp | Mnemonic::Fucompp => {
            cpu.fpu_drop();
            cpu.fpu_drop();
        }
        _ => {}
    }
}

pub fn ficom_variants(cpu: &mut Cpu, instr: &Instruction) {
    let st0 = cpu.fpu_get(0).get_f64();
    let addr = calculate_addr(cpu, instr);
    let val = match instr.memory_size() {
        MemorySize::Int16 => (cpu.lin_read_16(addr) as i16) as f64,
        MemorySize::Int32 => (cpu.lin_read_32(addr) as i32) as f64,
        _ => 0.0,
    };
    let precise = cpu.fpu_beyond_double(0).then(|| f80_of(val).map(|op| (cpu.fpu_get(0), op))).flatten();
    if !fpu_compare_precise(cpu, precise) {
        fpu_compare_values(cpu, st0, val);
    }
    if instr.mnemonic() == Mnemonic::Ficomp {
        cpu.fpu_pop();
    }
}

// FXAM: Examine ST(0)
pub fn fxam(cpu: &mut Cpu) {
    // Clear C0, C1, C2, C3
    cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C2 | FpuFlags::C1 | FpuFlags::C0, false);

    let tag = cpu.fpu_tags[cpu.fpu_top as usize];
    let st0 = cpu.fpu_reg(cpu.fpu_top); // Access raw stack to avoid fpu_get logic

    // Set C1 to the Sign Bit
    if st0.get_sign() {
        cpu.set_fpu_flag(FpuFlags::C1, true);
    }

    // Categorize based on Tag and Bits
    // Empty:    C3=1, C2=0, C0=1
    // Zero:     C3=1, C2=0, C0=0
    // Normal:   C3=0, C2=1, C0=0
    // Infinity: C3=0, C2=1, C0=1
    // NaN:      C3=0, C2=0, C0=1
    if tag == FPU_TAG_EMPTY {
        cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C0, true);
    } else if st0.is_nan() {
        cpu.set_fpu_flag(FpuFlags::C0, true);
    } else if st0.is_zero() {
        cpu.set_fpu_flag(FpuFlags::C3, true);
    } else if st0.is_infinite() {
        cpu.set_fpu_flag(FpuFlags::C2 | FpuFlags::C0, true);
    } else if st0.is_denormal() {
        // Denormal: C3=1, C2=1, C0=0
        cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C2, true);
    } else {
        // Normal Finite
        cpu.set_fpu_flag(FpuFlags::C2, true);
    }
}

// FTST: Test ST(0) against 0.0
pub fn ftst(cpu: &mut Cpu) {
    let st0 = cpu.fpu_get(0).get_f64();
    // Compare ST(0) vs 0.0
    fpu_compare_values(cpu, st0, 0.0);
}

// FCOMI/FUCOMI... (Pentium Pro+)
// These set CPU EFLAGS (ZF, PF, CF) directly, not the FPU status word condition codes.
pub fn fcomi_variants(cpu: &mut Cpu, instr: &Instruction) {
    let idx = (instr.op1_register().number() - iced_x86::Register::ST0.number()) as usize;
    let st0 = cpu.fpu_get(0);
    let sti = cpu.fpu_get(idx);
    
    // ZF=1 if equal, CF=1 if less, all three if unordered (a NaN). The
    // comparison is of all 80 bits, and +0 equals -0.
    let (zf, pf, cf) = if st0.is_nan() || sti.is_nan() {
        (true, true, true)
    } else if st0.get() == sti.get() || (st0.is_zero() && sti.is_zero()) {
        (true, false, false)
    } else {
        const MAGNITUDE: u128 = (1u128 << 79) - 1;
        let (a, b) = (st0.get() & MAGNITUDE, sti.get() & MAGNITUDE);
        let less = match (st0.get_sign(), sti.get_sign()) {
            (false, false) => a < b,
            (true, true) => a > b,
            (negative, _) => negative,
        };
        (false, false, less)
    };

    cpu.set_cpu_flag(CpuFlags::ZF, zf);
    cpu.set_cpu_flag(CpuFlags::PF, pf);
    cpu.set_cpu_flag(CpuFlags::CF, cf);
    // OF, SF and AF are cleared.
    cpu.set_cpu_flag(CpuFlags::OF | CpuFlags::SF | CpuFlags::AF, false);

    // Pop if P-variant (FCOMIP / FUCOMIP)
    let m = instr.mnemonic();
    if m == iced_x86::Mnemonic::Fcomip || m == iced_x86::Mnemonic::Fucomip {
        cpu.fpu_pop();
    }
}