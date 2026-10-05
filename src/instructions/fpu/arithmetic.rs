use crate::cpu::{Cpu, FpuFlags};
use crate::f80::{F80, canon_f64};
use crate::instructions::utils::calculate_addr;
use iced_x86::{Instruction, MemorySize, OpKind, Register};

// Get the destination index for Pop instructions (e.g., FADDP ST(i), ST(0))
pub fn get_pop_dst_index(instr: &Instruction) -> usize {
    let reg = instr.op0_register();
    if reg == Register::None || reg == Register::ST1 {
        1
    } else {
        (reg.number() - Register::ST0.number()) as usize
    }
}

/// ST(dst) = ST(a) + ST(b), or - with `sub`, as the registers' 80 bits add
/// (`F80::add`), or with `fpu_fast` as their doubles do (`sum`). The
/// dynamic recompiler's code calls it too.
pub fn addsub_st(cpu: &mut Cpu, dst: usize, a: usize, b: usize, sub: bool) {
    if cpu.fpu_fast {
        let sum = sum(cpu.fpu_get_f64(a), cpu.fpu_get_f64(b), sub);
        cpu.fpu_set_f64(dst, sum);
        return;
    }
    let mut x = cpu.fpu_get(a);
    let y = cpu.fpu_get(b);
    if sub {
        x.sub(y);
    } else {
        x.add(y);
    }
    cpu.fpu_set(dst, x);
}

/// What `addsub_value` does with ST(0) and the value.
pub const ADD_VALUE: u32 = 0;
pub const SUB_VALUE: u32 = 1;
pub const SUBR_VALUE: u32 = 2;

/// ST(0) += a memory operand's value, or -=, or the value - ST(0).
pub fn addsub_value(cpu: &mut Cpu, kind: u32, value: f64) {
    if cpu.fpu_fast {
        // (The value as a register would have it, as FMUL's.)
        let (value, st0) = (canon_f64(value), cpu.fpu_get_f64(0));
        let sum = match kind {
            ADD_VALUE => sum(st0, value, false),
            SUB_VALUE => sum(st0, value, true),
            _ => sum(value, st0, true),
        };
        cpu.fpu_set_f64(0, sum);
        return;
    }
    let mut val = F80::new();
    val.set_f64(value);
    let mut st0 = cpu.fpu_get(0);
    match kind {
        ADD_VALUE => st0.add(val),
        SUB_VALUE => st0.sub(val),
        _ => {
            val.sub(st0);
            st0 = val;
        }
    }
    cpu.fpu_set(0, st0);
}

/// A real memory operand as a double.
fn real_operand(cpu: &mut Cpu, instr: &Instruction) -> Option<f64> {
    let addr = calculate_addr(cpu, instr);
    match instr.memory_size() {
        MemorySize::Float32 => Some(f32::from_bits(cpu.lin_read_32(addr)) as f64),
        MemorySize::Float64 => Some(f64::from_bits(cpu.lin_read_64(addr))),
        _ => None,
    }
}

/// ST(i) = dividend / divisor as doubles, or what a division by 0 leaves
/// (`divided_by_zero`, with ZE for the reversed divisions).
fn divide(cpu: &mut Cpu, i: usize, dividend: f64, divisor: f64, reversed: bool) {
    if divisor != 0.0 {
        cpu.fpu_set_f64(i, dividend / divisor);
    } else {
        divided_by_zero(cpu, i, reversed);
    }
}

/// The quotient of a division by 0: ST(i) is the real indefinite, and
/// the reversed divisions set ZE as well.
pub fn divided_by_zero(cpu: &mut Cpu, i: usize, ze: bool) {
    let mut v = F80::new();
    v.set_real_indefinite();
    cpu.fpu_set(i, v);
    if ze {
        cpu.set_fpu_flag(FpuFlags::ZE, true);
    }
}

// FIADD: Add Integer
// ST(0) = ST(0) + [mem_int]
pub fn fiadd(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    if cpu.fpu_fast {
        let val = cpu.load_int_to_f80(addr, instr.memory_size()).get_f64();
        addsub_value(cpu, ADD_VALUE, val);
        return;
    }
    let val = cpu.load_int_to_f80(addr, instr.memory_size());
    let mut st0 = cpu.fpu_get(0);
    st0.add(val);
    cpu.fpu_set(0, st0);
}

// FISUB: Subtract Integer
// ST(0) = ST(0) - [mem_int]
pub fn fisub(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    if cpu.fpu_fast {
        let val = cpu.load_int_to_f80(addr, instr.memory_size()).get_f64();
        addsub_value(cpu, SUB_VALUE, val);
        return;
    }
    let val = cpu.load_int_to_f80(addr, instr.memory_size());
    let mut st0 = cpu.fpu_get(0);
    st0.sub(val);
    cpu.fpu_set(0, st0);
}

// FISUBR: Subtract Integer Reverse
// ST(0) = [mem_int] - ST(0)
pub fn fisubr(cpu: &mut Cpu, instr: &Instruction) {
    let addr = calculate_addr(cpu, instr);
    if cpu.fpu_fast {
        let val = cpu.load_int_to_f80(addr, instr.memory_size()).get_f64();
        addsub_value(cpu, SUBR_VALUE, val);
        return;
    }
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
    // As a double, as FMUL's (which an F80 set to it comes to).
    let st0 = cpu.fpu_get(0).get_f64();
    cpu.fpu_set_f64(0, product(st0, val));
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
        let val = real_operand(cpu, instr).unwrap_or(0.0);
        addsub_value(cpu, ADD_VALUE, val);
    } else {
        let dst_reg = instr.op0_register();
        let src_reg = instr.op1_register();
        let idx_src = (src_reg.number() - Register::ST0.number()) as usize;
        let idx_dst = (dst_reg.number() - Register::ST0.number()) as usize;
        addsub_st(cpu, idx_dst, idx_dst, idx_src, false);
    }
}

// FADDP: Add and Pop
pub fn faddp(cpu: &mut Cpu, instr: &Instruction) {
    let dst_reg = instr.op0_register();
    let idx = if dst_reg == Register::None || dst_reg == Register::ST1 {
        1
    } else {
        (dst_reg.number() - Register::ST0.number()) as usize
    };

    addsub_st(cpu, idx, idx, 0, false);
    cpu.fpu_drop();
}

// FSUB: Subtract Real
// ST(0) = ST(0) - Src  OR  Dest = Dest - ST(0)
pub fn fsub(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let val = real_operand(cpu, instr).unwrap_or(0.0);
        addsub_value(cpu, SUB_VALUE, val);
    } else {
        let dst_idx = (instr.op0_register().number() - Register::ST0.number()) as usize;
        let src_idx = (instr.op1_register().number() - Register::ST0.number()) as usize;
        addsub_st(cpu, dst_idx, dst_idx, src_idx, true);
    }
}

// FSUBP: Subtract and Pop
// ST(i) = ST(i) - ST(0); Pop ST(0)
pub fn fsubp(cpu: &mut Cpu, instr: &Instruction) {
    let idx = get_pop_dst_index(instr);

    addsub_st(cpu, idx, idx, 0, true);
    cpu.fpu_drop();
}

// FSUBR: Reverse Subtract
// ST(0) = Src - ST(0)  OR  Dest = ST(0) - Dest
pub fn fsubr(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let val = real_operand(cpu, instr).unwrap_or(0.0);
        addsub_value(cpu, SUBR_VALUE, val);
    } else {
        let dst_idx = (instr.op0_register().number() - Register::ST0.number()) as usize;
        let src_idx = (instr.op1_register().number() - Register::ST0.number()) as usize;
        addsub_st(cpu, dst_idx, src_idx, dst_idx, true);
    }
}

// FSUBRP: Reverse Subtract and Pop
// ST(i) = ST(0) - ST(i); Pop ST(0)
pub fn fsubrp(cpu: &mut Cpu, instr: &Instruction) {
    let idx = get_pop_dst_index(instr);

    addsub_st(cpu, idx, 0, idx, true);
    cpu.fpu_drop();
}

/// a * b, with a NaN of a's where both are NaNs, as SSE2's MULSD and
/// ARM64's FMUL give it with a as the first operand, which the dynamic
/// recompiler's code has it. (The compiler takes `*` as commutative and
/// may swap them, which would make the NaN b's.)
#[inline]
pub fn product(a: f64, b: f64) -> f64 {
    if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else {
        a * b
    }
}

/// a + b, or a - b with `sub`, in fast mode (`fpu_fast`): rounded to
/// nearest, whatever the control word says, as the multiplications and
/// divisions are. Where both are NaNs, a's, as SSE2's ADDSD and SUBSD and
/// ARM64's FADD and FSUB give it with a as the first operand (as
/// `product`).
#[inline]
pub fn sum(a: f64, b: f64, sub: bool) -> f64 {
    if a.is_nan() {
        a
    } else if b.is_nan() {
        b
    } else if sub {
        a - b
    } else {
        a + b
    }
}

// FMUL: Multiply Real
pub fn fmul(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let val = canon_f64(real_operand(cpu, instr).unwrap_or(0.0));
        let product = product(cpu.fpu_get_f64(0), val);
        cpu.fpu_set_f64(0, product);
    } else {
        let dst_idx = (instr.op0_register().number() - Register::ST0.number()) as usize;
        let src_idx = (instr.op1_register().number() - Register::ST0.number()) as usize;
        let product = product(cpu.fpu_get_f64(dst_idx), cpu.fpu_get_f64(src_idx));
        cpu.fpu_set_f64(dst_idx, product);
    }
}

// FMULP: Multiply and Pop
pub fn fmulp(cpu: &mut Cpu, instr: &Instruction) {
    let idx = get_pop_dst_index(instr);
    let product = product(cpu.fpu_get_f64(idx), cpu.fpu_get_f64(0));
    cpu.fpu_set_f64(idx, product);
    cpu.fpu_drop();
}

// FDIV: Floating Point Divide
pub fn fdiv(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        let divisor = canon_f64(real_operand(cpu, instr).unwrap_or(0.0));
        divide(cpu, 0, cpu.fpu_get_f64(0), divisor, false);
    } else {
        let dst_idx = (instr.op0_register().number() - Register::ST0.number()) as usize;
        let src_idx = (instr.op1_register().number() - Register::ST0.number()) as usize;
        divide(cpu, dst_idx, cpu.fpu_get_f64(dst_idx), cpu.fpu_get_f64(src_idx), false);
    }
}

// FDIVP: Divide and Pop
pub fn fdivp(cpu: &mut Cpu, instr: &Instruction) {
    let idx = get_pop_dst_index(instr);
    divide(cpu, idx, cpu.fpu_get_f64(idx), cpu.fpu_get_f64(0), false);
    cpu.fpu_drop();
}

// FDIVR: Reverse Divide
pub fn fdivr(cpu: &mut Cpu, instr: &Instruction) {
    if instr.op0_kind() == OpKind::Memory {
        // FDIVR [mem] -> ST(0) = [mem] / ST(0)
        let val = canon_f64(real_operand(cpu, instr).unwrap_or(1.0));
        divide(cpu, 0, val, cpu.fpu_get_f64(0), true);
    } else {
        let dst_idx = (instr.op0_register().number() - Register::ST0.number()) as usize;
        let src_idx = (instr.op1_register().number() - Register::ST0.number()) as usize;
        // FDIVR ST(0), ST(i) -> ST(0) = ST(i) / ST(0)
        // FDIVR ST(i), ST(0) -> ST(i) = ST(0) / ST(i)
        divide(cpu, dst_idx, cpu.fpu_get_f64(src_idx), cpu.fpu_get_f64(dst_idx), true);
    }
}

// FDIVRP: Reverse Divide and Pop
// ST(i) = ST(0) / ST(i); Pop ST(0)
pub fn fdivrp(cpu: &mut Cpu, instr: &Instruction) {
    let idx = get_pop_dst_index(instr);
    divide(cpu, idx, cpu.fpu_get_f64(0), cpu.fpu_get_f64(idx), true);
    cpu.fpu_drop();
}

// --- ADVANCED ARITHMETIC ---

pub fn fprem_internal(cpu: &mut Cpu, ieee: bool) {
    let st0_obj = cpu.fpu_get(0);
    let st1_obj = cpu.fpu_get(1);

    let st0 = st0_obj.get_f64();
    let st1 = st1_obj.get_f64();

    if st1 == 0.0 || st0.is_infinite() {
        cpu.set_fpu_flag(FpuFlags::IE, true);
        let mut nan = F80::new();
        nan.set_real_indefinite();
        cpu.fpu_set(0, nan);
        return;
    }

    // FPREM spec allows partial reduction if exponent difference is large.
    // However, to avoid complexity and precision issues with large quotients in manual reduction,
    // we perform a "complete" reduction immediately using the underlying f64 remainder.
    // This is compliant as long as we clear C2 (indicating completion).

    let (remainder, q_bits) = if ieee {
        // FPREM1: Round to nearest
        let quotient = st0 / st1;
        let q_rnd = quotient.round();
        let rem = st0 - q_rnd * st1;
        (rem, q_rnd as i64)
    } else {
        // FPREM: Round toward zero (Truncate)
        // Use f64 standard remainder (%) which is truncating
        let rem = st0 % st1;

        // Calculate quotient bits (Q2, Q1, Q0) without full quotient overflow
        // Q = trunc(ST0 / ST1). We need Q % 8.
        // Q % 8 = trunc( (ST0 % (8 * ST1)) / ST1 )
        let st0_abs = st0.abs();
        let st1_abs = st1.abs();
        let st0_mod8 = st0_abs % (8.0 * st1_abs);
        let q_mod8 = (st0_mod8 / st1_abs).trunc() as i64;

        let sign_diff = st0.is_sign_negative() ^ st1.is_sign_negative();
        let q_final = if sign_diff { -q_mod8 } else { q_mod8 };

        (rem, q_final)
    };

    let mut res = F80::new();
    res.set_f64(remainder);
    cpu.fpu_set(0, res);

    cpu.set_fpu_flag(
        FpuFlags::C0 | FpuFlags::C1 | FpuFlags::C2 | FpuFlags::C3,
        false,
    );

    // Set C0, C3, C1 from Q2, Q1, Q0
    if (q_bits & 4) != 0 {
        cpu.set_fpu_flag(FpuFlags::C0, true);
    }
    if (q_bits & 1) != 0 {
        cpu.set_fpu_flag(FpuFlags::C1, true);
    }
    if (q_bits & 2) != 0 {
        cpu.set_fpu_flag(FpuFlags::C3, true);
    }
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
        cpu.set_fpu_flag(FpuFlags::IE, true); // Set Invalid Operation bit

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
    if x > 0.0 {
        y.set_f64(y.get_f64() * x.log2());
    } else {
        y.set_QNaN();
    }
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
