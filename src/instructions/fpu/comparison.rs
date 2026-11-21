use iced_x86::{Instruction, Mnemonic, OpKind, MemorySize, Register};

use std::cmp::Ordering;

use crate::cpu::{Cpu, FpuFlags, CpuFlags, FPU_TAG_EMPTY};
use crate::f80::F80;
use crate::instructions::utils::calculate_addr;

// The compares report what the x87 does with the invalid exception
// masked, whatever the control word says, as every x87 instruction here
// does: with it unmasked, the FPU would leave the condition codes and the
// stack as they were and raise the error at the next waiting instruction
// (#914).

/// Whether a register's value makes a compare invalid (IE): a format the
/// FPU doesn't take (`F80::is_unsupported`), or a NaN, of either kind for
/// FCOM, FICOM, FTST and FCOMI, a signalling one for FUCOM and FUCOMI
/// (`unordered`).
fn invalid_operand(x: F80, unordered: bool) -> bool {
    x.is_unsupported() || if unordered { x.is_signalling() } else { x.is_nan() }
}

/// How `lhs` and `rhs` order, None where either is a NaN. Where either
/// register holds more than its double, `precise` has their 80 bits,
/// which order them (`F80::compare`) where both are normal numbers or
/// zeros: the doubles are rounded to nearest, so a value just below a
/// double could compare equal to it.
fn order(lhs: f64, rhs: f64, precise: Option<(F80, F80)>) -> Option<Ordering> {
    precise.and_then(|(lhs, rhs)| lhs.compare(&rhs)).or_else(|| lhs.partial_cmp(&rhs))
}

/// Leave the status word as FCOM, FUCOM, FICOM and FTST do: C3, C2 and C0
/// as `order` says (all three set for unordered, None), and C1 clear. An
/// empty register (`underflow`, a stack fault) sets SF and IE, and an
/// invalid operand (`invalid_operand`) IE; either makes it unordered.
fn set_condition_codes(cpu: &mut Cpu, order: Option<Ordering>, underflow: bool, invalid: bool) {
    cpu.set_fpu_flag(FpuFlags::C0 | FpuFlags::C1 | FpuFlags::C2 | FpuFlags::C3, false);
    set_invalid(cpu, underflow, invalid);
    let order = if underflow || invalid { None } else { order };
    let codes = match order {
        None => FpuFlags::C0 | FpuFlags::C2 | FpuFlags::C3,
        Some(Ordering::Equal) => FpuFlags::C3,
        Some(Ordering::Less) => FpuFlags::C0,
        Some(Ordering::Greater) => FpuFlags::empty(),
    };
    cpu.set_fpu_flag(codes, true);
}

/// SF and IE for a stack underflow, IE alone for an invalid operand.
fn set_invalid(cpu: &mut Cpu, underflow: bool, invalid: bool) {
    if underflow {
        cpu.set_fpu_flag(FpuFlags::SF | FpuFlags::IE, true);
    } else if invalid {
        cpu.set_fpu_flag(FpuFlags::IE, true);
    }
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
            // The undocumented DC D0+i, DC D8+i and DE D0+i encodings are
            // aliases of FCOM/FCOMP ST(i): ST(0) with ST(i), as on real
            // CPUs.
            // ST(i) is the last operand: iced lists ST(0) first. It can be
            // ST(0) itself, as in FUCOMP ST(0), GCC's test for NaN.
            let idx = match instr.op_register(instr.op_count().saturating_sub(1)) {
                Register::None => 1,
                reg => reg.number() - Register::ST0.number(),
            };
            if idx > 7 {
                return None;
            }
            Some((0, idx, pops))
        }
        _ => Some((0, 1, pops)),
    }
}

pub fn fcom_variants(cpu: &mut Cpu, instr: &Instruction) {
    let unordered =
        matches!(instr.mnemonic(), Mnemonic::Fucom | Mnemonic::Fucomp | Mnemonic::Fucompp);
    // The doubles, and the 80 bits where either register holds more.
    let (order, underflow, invalid) = match fcom_registers(instr) {
        Some((lhs, rhs, _)) => {
            let (a, b) = (cpu.fpu_get(lhs), cpu.fpu_get(rhs));
            let beyond = cpu.fpu_beyond_double(lhs) || cpu.fpu_beyond_double(rhs);
            let precise = beyond.then_some((a, b));
            (
                order(cpu.fpu_get_f64(lhs), cpu.fpu_get_f64(rhs), precise),
                cpu.fpu_is_empty(lhs) || cpu.fpu_is_empty(rhs),
                invalid_operand(a, unordered) || invalid_operand(b, unordered),
            )
        }
        None => {
            // Memory Comparison is ALWAYS ST(0) vs Memory
            let st0 = cpu.fpu_get(0);
            let addr = calculate_addr(cpu, instr);
            let val_op = match instr.memory_size() {
                MemorySize::Float32 => f32::from_bits(cpu.lin_read_32(addr)) as f64,
                MemorySize::Float64 => f64::from_bits(cpu.lin_read_64(addr)),
                _ => f64::NAN,
            };
            let precise =
                cpu.fpu_beyond_double(0).then(|| f80_of(val_op).map(|op| (st0, op))).flatten();
            (
                order(cpu.fpu_get_f64(0), val_op, precise),
                cpu.fpu_is_empty(0),
                invalid_operand(st0, false) || val_op.is_nan(),
            )
        }
    };
    set_condition_codes(cpu, order, underflow, invalid);

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
    let st0 = cpu.fpu_get(0);
    let addr = calculate_addr(cpu, instr);
    let val = match instr.memory_size() {
        MemorySize::Int16 => (cpu.lin_read_16(addr) as i16) as f64,
        MemorySize::Int32 => (cpu.lin_read_32(addr) as i32) as f64,
        _ => 0.0,
    };
    let precise = cpu.fpu_beyond_double(0).then(|| f80_of(val).map(|op| (st0, op))).flatten();
    let order = order(st0.get_f64(), val, precise);
    set_condition_codes(cpu, order, cpu.fpu_is_empty(0), invalid_operand(st0, false));
    if instr.mnemonic() == Mnemonic::Ficomp {
        cpu.fpu_pop();
    }
}

// FXAM: Examine ST(0)
pub fn fxam(cpu: &mut Cpu) {
    // Clear C0, C1, C2, C3
    cpu.set_fpu_flag(FpuFlags::C3 | FpuFlags::C2 | FpuFlags::C1 | FpuFlags::C0, false);

    let tag = cpu.fpu_tags[cpu.fpu_top];
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
    let st0 = cpu.fpu_get(0);
    let order = order(st0.get_f64(), 0.0, None);
    set_condition_codes(cpu, order, cpu.fpu_is_empty(0), invalid_operand(st0, false));
}

/// FCOMI, FCOMIP, FUCOMI and FUCOMIP (Pentium Pro and later): the result
/// goes to EFLAGS' ZF, PF and CF rather than to the condition codes. In
/// the status word they clear C1 and set IE and SF as FCOM and FUCOM do.
pub fn fcomi_variants(cpu: &mut Cpu, instr: &Instruction) {
    let idx = instr.op1_register().number() - iced_x86::Register::ST0.number();
    let st0 = cpu.fpu_get(0);
    let sti = cpu.fpu_get(idx);
    let m = instr.mnemonic();
    let unordered = matches!(m, Mnemonic::Fucomi | Mnemonic::Fucomip);
    let underflow = cpu.fpu_is_empty(0) || cpu.fpu_is_empty(idx);
    let invalid = invalid_operand(st0, unordered) || invalid_operand(sti, unordered);
    cpu.set_fpu_flag(FpuFlags::C1, false);
    set_invalid(cpu, underflow, invalid);

    // ZF=1 if equal, CF=1 if less, all three if unordered (a NaN, an
    // empty register or an invalid operand). The comparison is of all 80
    // bits, and +0 equals -0.
    let (zf, pf, cf) = if underflow || invalid || st0.is_nan() || sti.is_nan() {
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
    if m == Mnemonic::Fcomip || m == Mnemonic::Fucomip {
        cpu.fpu_pop();
    }
}
