//! Arithmetic and logic instructions: ADD..CMP, TEST, INC/DEC/NEG/NOT,
//! MUL/IMUL/DIV/IDIV, the BCD adjustments and the sign extensions.

use iced_x86::{Instruction, Register};

use super::operand::{loc, op_size, read_op};
use crate::cpu::alu::{AF, ARITH, CF, OF, PF, SF, ZF, sign_extend, size_mask};
use crate::cpu::{Access, Cpu, CpuFlags, CpuResult, Fault};

/// Two-operand ALU operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Add,
    Adc,
    Sub,
    Sbb,
    Cmp,
    And,
    Or,
    Xor,
    Test,
}

/// `dest = dest op src`, or only the flags for CMP and TEST.
pub fn binary(cpu: &mut Cpu, instr: &Instruction, op: Op) -> CpuResult {
    let size = op_size(instr, 0);
    let writes = !matches!(op, Op::Cmp | Op::Test);
    let dest = loc(cpu, instr, 0, size, if writes { Access::Write } else { Access::Read })?;
    let src = read_op(cpu, instr, 1, size)?;
    let a = dest.read(cpu);
    let cf = cpu.get_cpu_flag(CpuFlags::CF);
    let r = match op {
        Op::Add => cpu.alu_add(size, a, src, false),
        Op::Adc => cpu.alu_add(size, a, src, cf),
        Op::Sub | Op::Cmp => cpu.alu_sub(size, a, src, false),
        Op::Sbb => cpu.alu_sub(size, a, src, cf),
        Op::And | Op::Test => cpu.alu_logic(size, a & src),
        Op::Or => cpu.alu_logic(size, a | src),
        Op::Xor => cpu.alu_logic(size, a ^ src),
    };
    if writes {
        dest.write(cpu, r);
    }
    Ok(())
}

pub fn inc(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = op_size(instr, 0);
    let dest = loc(cpu, instr, 0, size, Access::Write)?;
    let r = cpu.alu_inc(size, dest.read(cpu));
    dest.write(cpu, r);
    Ok(())
}

pub fn dec(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = op_size(instr, 0);
    let dest = loc(cpu, instr, 0, size, Access::Write)?;
    let r = cpu.alu_dec(size, dest.read(cpu));
    dest.write(cpu, r);
    Ok(())
}

pub fn neg(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = op_size(instr, 0);
    let dest = loc(cpu, instr, 0, size, Access::Write)?;
    let r = cpu.alu_neg(size, dest.read(cpu));
    dest.write(cpu, r);
    Ok(())
}

pub fn not(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = op_size(instr, 0);
    let dest = loc(cpu, instr, 0, size, Access::Write)?;
    let r = !dest.read(cpu) & size_mask(size);
    dest.write(cpu, r);
    Ok(())
}

/// Set CF and OF together.
fn set_cf_of(cpu: &mut Cpu, overflow: bool) {
    cpu.set_flag_bits(CF | OF, if overflow { CF | OF } else { 0 });
}

/// MUL: AL/AX/EAX times the operand into AX, DX:AX or EDX:EAX. CF and OF
/// tell whether the upper half is in use.
pub fn mul(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = op_size(instr, 0);
    let src = read_op(cpu, instr, 0, size)? as u64;
    match size {
        1 => {
            let r = cpu.get_al() as u64 * src;
            cpu.set_ax(r as u16);
            set_cf_of(cpu, r > 0xFF);
        }
        2 => {
            let r = cpu.ax() as u64 * src;
            cpu.set_ax(r as u16);
            cpu.set_dx((r >> 16) as u16);
            set_cf_of(cpu, r > 0xFFFF);
        }
        _ => {
            let r = cpu.eax() as u64 * src;
            cpu.set_eax(r as u32);
            cpu.set_edx((r >> 32) as u32);
            set_cf_of(cpu, r > 0xFFFF_FFFF);
        }
    }
    Ok(())
}

/// IMUL in its three forms: one operand (widening, like MUL), two operands
/// (`dest *= src`) and three (`dest = src * imm`).
pub fn imul(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    if instr.op_count() == 1 {
        let size = op_size(instr, 0);
        let src = sign_extend(size, read_op(cpu, instr, 0, size)?) as i32 as i64;
        match size {
            1 => {
                let r = cpu.get_al() as i8 as i64 * src;
                cpu.set_ax(r as u16);
                set_cf_of(cpu, r != r as i8 as i64);
            }
            2 => {
                let r = cpu.ax() as i16 as i64 * src;
                cpu.set_ax(r as u16);
                cpu.set_dx((r >> 16) as u16);
                set_cf_of(cpu, r != r as i16 as i64);
            }
            _ => {
                let r = cpu.eax() as i32 as i64 * src;
                cpu.set_eax(r as u32);
                cpu.set_edx((r >> 32) as u32);
                set_cf_of(cpu, r != r as i32 as i64);
            }
        }
        return Ok(());
    }

    let size = op_size(instr, 0);
    let (a, b) = if instr.op_count() == 2 {
        (cpu.reg(instr.op0_register()), read_op(cpu, instr, 1, size)?)
    } else {
        (read_op(cpu, instr, 1, size)?, read_op(cpu, instr, 2, size)?)
    };
    let a = sign_extend(size, a) as i32 as i64;
    let b = sign_extend(size, b) as i32 as i64;
    let r = a * b;
    let truncated = sign_extend(size, r as u32 & size_mask(size)) as i32 as i64;
    cpu.set_reg(instr.op0_register(), r as u32);
    set_cf_of(cpu, r != truncated);
    Ok(())
}

/// DIV: AX, DX:AX or EDX:EAX divided by the operand. A zero divisor or a
/// quotient that doesn't fit raises #DE.
pub fn div(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = op_size(instr, 0);
    let divisor = read_op(cpu, instr, 0, size)? as u64;
    if divisor == 0 {
        return Err(Fault::DE);
    }
    match size {
        1 => {
            let dividend = cpu.ax() as u64;
            let q = dividend / divisor;
            if q > 0xFF {
                return Err(Fault::DE);
            }
            cpu.set_reg(Register::AL, q as u32);
            cpu.set_reg(Register::AH, (dividend % divisor) as u32);
        }
        2 => {
            let dividend = ((cpu.dx() as u64) << 16) | cpu.ax() as u64;
            let q = dividend / divisor;
            if q > 0xFFFF {
                return Err(Fault::DE);
            }
            cpu.set_ax(q as u16);
            cpu.set_dx((dividend % divisor) as u16);
        }
        _ => {
            let dividend = ((cpu.edx() as u64) << 32) | cpu.eax() as u64;
            let q = dividend / divisor;
            if q > 0xFFFF_FFFF {
                return Err(Fault::DE);
            }
            cpu.set_eax(q as u32);
            cpu.set_edx((dividend % divisor) as u32);
        }
    }
    Ok(())
}

/// IDIV: signed DIV. The quotient is truncated toward zero; the remainder
/// has the dividend's sign.
pub fn idiv(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = op_size(instr, 0);
    let divisor = sign_extend(size, read_op(cpu, instr, 0, size)?) as i32 as i64;
    if divisor == 0 {
        return Err(Fault::DE);
    }
    let dividend: i64 = match size {
        1 => cpu.ax() as i16 as i64,
        2 => ((((cpu.dx() as u32) << 16) | cpu.ax() as u32) as i32) as i64,
        _ => (((cpu.edx() as u64) << 32) | cpu.eax() as u64) as i64,
    };
    let q = dividend.wrapping_div(divisor);
    let r = dividend.wrapping_rem(divisor);
    let bits = size as u32 * 8;
    let (min, max) = (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1);
    // i64::MIN / -1 wraps; it can only come from a 32-bit operand and is
    // out of range anyway.
    if q < min || q > max || (dividend == i64::MIN && divisor == -1) {
        return Err(Fault::DE);
    }
    match size {
        1 => {
            cpu.set_reg(Register::AL, q as u32);
            cpu.set_reg(Register::AH, r as u32);
        }
        2 => {
            cpu.set_ax(q as u16);
            cpu.set_dx(r as u16);
        }
        _ => {
            cpu.set_eax(q as u32);
            cpu.set_edx(r as u32);
        }
    }
    Ok(())
}

/// Set SF, ZF and PF from AL.
fn set_szp8(cpu: &mut Cpu, al: u8) {
    cpu.set_cpu_flag(CpuFlags::SF, al & 0x80 != 0);
    cpu.set_cpu_flag(CpuFlags::ZF, al == 0);
    cpu.update_pf(al as u32);
}

/// The flags the BCD adjustments leave undefined are set the way a 386 sets
/// them (see test386.asm, validated against a 386SX).

/// OF of adding (or subtracting) the adjustment `adj` to AL `old`.
fn bcd_overflow(old: u8, adj: u8, sub: bool) -> bool {
    let r = if sub { old.wrapping_sub(adj) } else { old.wrapping_add(adj) };
    if sub { (old ^ adj) & (old ^ r) & 0x80 != 0 } else { (old ^ r) & (adj ^ r) & 0x80 != 0 }
}

/// DAA: decimal adjust AL after addition.
pub fn daa(cpu: &mut Cpu) -> CpuResult {
    let old_al = cpu.get_al();
    let old_cf = cpu.get_cpu_flag(CpuFlags::CF);
    let mut adj = 0u8;
    let mut cf = false;
    if old_al & 0x0F > 9 || cpu.get_cpu_flag(CpuFlags::AF) {
        adj = 6;
        cf = old_cf || old_al > 0xF9;
        cpu.set_flag_bits(AF, AF);
    } else {
        cpu.set_flag_bits(AF, 0);
    }
    if old_al > 0x99 || old_cf {
        adj = adj.wrapping_add(0x60);
        cf = true;
    }
    let al = old_al.wrapping_add(adj);
    cpu.set_reg(Register::AL, al as u32);
    cpu.set_cpu_flag(CpuFlags::CF, cf);
    cpu.set_cpu_flag(CpuFlags::OF, bcd_overflow(old_al, adj, false));
    set_szp8(cpu, al);
    Ok(())
}

/// DAS: decimal adjust AL after subtraction.
pub fn das(cpu: &mut Cpu) -> CpuResult {
    let old_al = cpu.get_al();
    let old_cf = cpu.get_cpu_flag(CpuFlags::CF);
    let mut adj = 0u8;
    let mut cf = false;
    if old_al & 0x0F > 9 || cpu.get_cpu_flag(CpuFlags::AF) {
        adj = 6;
        cf = old_cf || old_al < 6;
        cpu.set_flag_bits(AF, AF);
    } else {
        cpu.set_flag_bits(AF, 0);
    }
    if old_al > 0x99 || old_cf {
        adj = adj.wrapping_add(0x60);
        cf = true;
    }
    let al = old_al.wrapping_sub(adj);
    cpu.set_reg(Register::AL, al as u32);
    cpu.set_cpu_flag(CpuFlags::CF, cf);
    cpu.set_cpu_flag(CpuFlags::OF, bcd_overflow(old_al, adj, true));
    set_szp8(cpu, al);
    Ok(())
}

/// AAA: ASCII adjust after addition.
pub fn aaa(cpu: &mut Cpu) -> CpuResult {
    let old_al = cpu.get_al();
    let sf = (0x7A..=0xF9).contains(&old_al);
    let (adjust, of, zf_forced_clear) = if old_al & 0x0F > 9 {
        (true, old_al & 0xF0 == 0x70, false)
    } else if cpu.get_cpu_flag(CpuFlags::AF) {
        (true, false, true)
    } else {
        (false, false, false)
    };
    if adjust {
        cpu.set_ax(cpu.ax().wrapping_add(0x106));
    }
    let al = cpu.get_al();
    let mut f = if adjust { AF | CF } else { 0 };
    if sf {
        f |= SF;
    }
    if of {
        f |= OF;
    }
    if al == 0 && !zf_forced_clear {
        f |= ZF;
    }
    if al.count_ones() % 2 == 0 {
        f |= PF;
    }
    cpu.set_flag_bits(ARITH, f);
    cpu.set_reg(Register::AL, (al & 0x0F) as u32);
    Ok(())
}

/// AAS: ASCII adjust after subtraction.
pub fn aas(cpu: &mut Cpu) -> CpuResult {
    let old_al = cpu.get_al();
    let (adjust, sf, of) = if old_al & 0x0F > 9 {
        (true, old_al > 0x85, false)
    } else if cpu.get_cpu_flag(CpuFlags::AF) {
        (true, !(0x06..=0x85).contains(&old_al), (0x80..=0x85).contains(&old_al))
    } else {
        (false, old_al >= 0x80, false)
    };
    if adjust {
        cpu.set_ax(cpu.ax().wrapping_sub(6));
        cpu.set_reg(Register::AH, cpu.get_ah().wrapping_sub(1) as u32);
    }
    let al = cpu.get_al();
    let mut f = if adjust { AF | CF } else { 0 };
    if sf {
        f |= SF;
    }
    if of {
        f |= OF;
    }
    if al == 0 {
        f |= ZF;
    }
    if al.count_ones() % 2 == 0 {
        f |= PF;
    }
    cpu.set_flag_bits(ARITH, f);
    cpu.set_reg(Register::AL, (al & 0x0F) as u32);
    Ok(())
}

/// AAM: split AL into two digits of the immediate's base (10 unless
/// encoded otherwise). A base of 0 raises #DE. CF, OF and AF are cleared.
pub fn aam(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let base = instr.immediate8();
    if base == 0 {
        // A 386 changes the flags before it raises #DE.
        cpu.set_flag_bits(ARITH, PF);
        return Err(Fault::DE);
    }
    let al = cpu.get_al();
    cpu.set_reg(Register::AH, (al / base) as u32);
    cpu.set_reg(Register::AL, (al % base) as u32);
    cpu.set_flag_bits(CF | OF | AF, 0);
    set_szp8(cpu, al % base);
    Ok(())
}

/// AAD: combine AH and AL digits of the immediate's base into AL. The
/// flags are those of adding the high digit's value to AL.
pub fn aad(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let base = instr.immediate8();
    let high = cpu.get_ah().wrapping_mul(base);
    let al = cpu.alu_add(1, cpu.get_al() as u32, high as u32, false) as u8;
    cpu.set_ax(al as u16);
    Ok(())
}

/// CBW / CWDE: sign-extend AL into AX, or AX into EAX.
pub fn cbw(cpu: &mut Cpu) -> CpuResult {
    cpu.set_ax(cpu.get_al() as i8 as i16 as u16);
    Ok(())
}

pub fn cwde(cpu: &mut Cpu) -> CpuResult {
    cpu.set_eax(cpu.ax() as i16 as i32 as u32);
    Ok(())
}

/// CWD / CDQ: sign-extend AX into DX:AX, or EAX into EDX:EAX.
pub fn cwd(cpu: &mut Cpu) -> CpuResult {
    cpu.set_dx(if cpu.ax() & 0x8000 != 0 { 0xFFFF } else { 0 });
    Ok(())
}

pub fn cdq(cpu: &mut Cpu) -> CpuResult {
    cpu.set_edx(if cpu.eax() & 0x8000_0000 != 0 { 0xFFFF_FFFF } else { 0 });
    Ok(())
}
