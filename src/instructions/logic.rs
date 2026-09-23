//! Shifts and rotates, double shifts, bit tests and scans, and SETcc.

use iced_x86::{ConditionCode, Instruction, OpKind};

use super::operand::{Loc, addr_size, effective_offset, loc, mem_seg, op_size, read_op};
use crate::cpu::alu::{CF, ShiftOp, ZF, sign_extend};
use crate::cpu::{Access, Cpu, CpuFlags, CpuResult};

/// ROL/ROR/RCL/RCR/SHL/SHR/SAR r/m by 1, CL or an immediate.
pub fn shift(cpu: &mut Cpu, instr: &Instruction, op: ShiftOp) -> CpuResult {
    let size = op_size(instr, 0);
    let dest = loc(cpu, instr, 0, size, Access::Write)?;
    let count = read_op(cpu, instr, 1, 1)? & 0x1F;
    let r = cpu.alu_shift(op, size, dest.read(cpu), count);
    if count != 0 {
        dest.write(cpu, r);
    }
    Ok(())
}

/// SHLD/SHRD r/m, reg, CL or imm8.
pub fn double_shift(cpu: &mut Cpu, instr: &Instruction, left: bool) -> CpuResult {
    let size = op_size(instr, 0);
    let dest = loc(cpu, instr, 0, size, Access::Write)?;
    let src = cpu.reg(instr.op1_register());
    let count = read_op(cpu, instr, 2, 1)? & 0x1F;
    let r = cpu.alu_double_shift(left, size, dest.read(cpu), src, count);
    if count != 0 {
        dest.write(cpu, r);
    }
    Ok(())
}

/// Bit test operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitOp {
    Test,
    Set,
    Reset,
    Complement,
}

/// BT/BTS/BTR/BTC: copy the selected bit to CF, then leave, set, clear or
/// flip it. With a register bit offset and a memory operand, the offset is
/// signed and can select a bit outside the addressed word.
pub fn bit_test(cpu: &mut Cpu, instr: &Instruction, op: BitOp) -> CpuResult {
    let size = op_size(instr, 0);
    let bits = size as u32 * 8;
    let access = if op == BitOp::Test { Access::Read } else { Access::Write };
    let offset = read_op(cpu, instr, 1, size)?;

    let (dest, bit) = if instr.op0_kind() == OpKind::Memory && instr.op1_kind() == OpKind::Register {
        let offset = sign_extend(size, offset) as i32;
        let words = offset >> bits.trailing_zeros();
        let mut addr = effective_offset(cpu, instr).wrapping_add((words * size as i32) as u32);
        if addr_size(instr) == 2 {
            addr &= 0xFFFF;
        }
        let m = cpu.mem_ref(mem_seg(instr), addr, size, access)?;
        (Loc::Mem(m), offset as u32 & (bits - 1))
    } else {
        (loc(cpu, instr, 0, size, access)?, offset & (bits - 1))
    };

    let value = dest.read(cpu);
    let mask = 1u32 << bit;
    cpu.set_flag_bits(CF, if value & mask != 0 { CF } else { 0 });
    let r = match op {
        BitOp::Test => return Ok(()),
        BitOp::Set => value | mask,
        BitOp::Reset => value & !mask,
        BitOp::Complement => value ^ mask,
    };
    dest.write(cpu, r);
    Ok(())
}

/// BSF/BSR: index of the lowest or highest set bit. A zero source sets ZF
/// and leaves the destination alone.
pub fn bit_scan(cpu: &mut Cpu, instr: &Instruction, forward: bool) -> CpuResult {
    let size = op_size(instr, 0);
    let src = read_op(cpu, instr, 1, size)?;
    if src == 0 {
        cpu.set_flag_bits(ZF, ZF);
        return Ok(());
    }
    let index = if forward { src.trailing_zeros() } else { 31 - src.leading_zeros() };
    cpu.set_flag_bits(ZF, 0);
    cpu.set_reg(instr.op0_register(), index);
    Ok(())
}

/// Whether condition `cc` of a Jcc, SETcc or LOOPcc holds.
#[inline(always)]
pub fn condition(cpu: &Cpu, cc: ConditionCode) -> bool {
    let f = |flag| cpu.get_cpu_flag(flag);
    match cc {
        ConditionCode::o => f(CpuFlags::OF),
        ConditionCode::no => !f(CpuFlags::OF),
        ConditionCode::b => f(CpuFlags::CF),
        ConditionCode::ae => !f(CpuFlags::CF),
        ConditionCode::e => f(CpuFlags::ZF),
        ConditionCode::ne => !f(CpuFlags::ZF),
        ConditionCode::be => f(CpuFlags::CF) || f(CpuFlags::ZF),
        ConditionCode::a => !f(CpuFlags::CF) && !f(CpuFlags::ZF),
        ConditionCode::s => f(CpuFlags::SF),
        ConditionCode::ns => !f(CpuFlags::SF),
        ConditionCode::p => f(CpuFlags::PF),
        ConditionCode::np => !f(CpuFlags::PF),
        ConditionCode::l => f(CpuFlags::SF) != f(CpuFlags::OF),
        ConditionCode::ge => f(CpuFlags::SF) == f(CpuFlags::OF),
        ConditionCode::le => f(CpuFlags::ZF) || f(CpuFlags::SF) != f(CpuFlags::OF),
        ConditionCode::g => !f(CpuFlags::ZF) && f(CpuFlags::SF) == f(CpuFlags::OF),
        ConditionCode::None => true,
    }
}

/// SETcc r/m8.
pub fn setcc(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let dest = loc(cpu, instr, 0, 1, Access::Write)?;
    let value = condition(cpu, instr.condition_code()) as u32;
    dest.write(cpu, value);
    Ok(())
}
