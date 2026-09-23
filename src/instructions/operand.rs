//! Operand access for instruction handlers: effective addresses with 16 or
//! 32-bit addressing, and register, memory and immediate operands of 8, 16
//! or 32 bits.

use iced_x86::{Instruction, OpKind, Register};

use crate::cpu::alu::size_mask;
use crate::cpu::{Access, Cpu, CpuModel, CpuResult, Fault, MemRef, Seg};

/// Address size of the memory operand, in bytes: 2 or 4.
#[inline(always)]
pub fn addr_size(instr: &Instruction) -> u8 {
    let base = instr.memory_base();
    if base != Register::None {
        return base.size() as u8;
    }
    let index = instr.memory_index();
    if index != Register::None {
        return index.size() as u8;
    }
    // A direct address: disp16 or disp32.
    instr.memory_displ_size() as u8
}

/// Offset (effective address) of the memory operand, wrapped to the address
/// size.
#[inline(always)]
pub fn effective_offset(cpu: &Cpu, instr: &Instruction) -> u32 {
    let mut ea = instr.memory_displacement32();
    let base = instr.memory_base();
    if base != Register::None {
        ea = ea.wrapping_add(cpu.reg(base));
    }
    let index = instr.memory_index();
    if index != Register::None {
        ea = ea.wrapping_add(cpu.reg(index).wrapping_mul(instr.memory_index_scale()));
    } else if base != Register::None && instr.memory_index_scale() != 1 && cpu.model == CpuModel::I386 {
        // A SIB byte with no index (100b) but a scale other than 1: the 386
        // scales the base register. Intel leaves this encoding undefined.
        ea = ea
            .wrapping_sub(cpu.reg(base))
            .wrapping_add(cpu.reg(base).wrapping_mul(instr.memory_index_scale()));
    }
    if addr_size(instr) == 2 { ea & 0xFFFF } else { ea }
}

/// Segment register of the memory operand: the override prefix, or the
/// default (SS for BP/EBP/ESP-based addresses, DS otherwise).
#[inline(always)]
pub fn mem_seg(instr: &Instruction) -> Seg {
    Seg::from_register(instr.memory_segment()).unwrap_or(Seg::DS)
}

/// Check the memory operand for an access of `size` bytes.
#[inline(always)]
pub fn mem_operand(cpu: &mut Cpu, instr: &Instruction, size: u8, access: Access) -> CpuResult<MemRef> {
    let off = effective_offset(cpu, instr);
    cpu.mem_ref(mem_seg(instr), off, size, access)
}

/// Check `size` bytes of memory at `delta` bytes past the memory operand,
/// for instructions whose operand is several fields (far pointers, BOUND,
/// LGDT). The offset wraps at the address size.
pub fn mem_operand_at(
    cpu: &mut Cpu,
    instr: &Instruction,
    delta: u32,
    size: u8,
    access: Access,
) -> CpuResult<MemRef> {
    let mut off = effective_offset(cpu, instr).wrapping_add(delta);
    if addr_size(instr) == 2 {
        off &= 0xFFFF;
    }
    cpu.mem_ref(mem_seg(instr), off, size, access)
}

/// Size in bytes of operand `i`, which must be a register or memory.
#[inline(always)]
pub fn op_size(instr: &Instruction, i: u32) -> u8 {
    match instr.op_kind(i) {
        OpKind::Register => instr.op_register(i).size() as u8,
        _ => instr.memory_size().size() as u8,
    }
}

/// A register or checked memory location that can be read and written
/// without faulting.
#[derive(Clone, Copy, Debug)]
pub enum Loc {
    Reg(Register),
    Mem(MemRef),
}

impl Loc {
    #[inline(always)]
    pub fn read(self, cpu: &Cpu) -> u32 {
        match self {
            Loc::Reg(r) => cpu.reg(r),
            Loc::Mem(m) => cpu.mem_read(m),
        }
    }

    #[inline(always)]
    pub fn write(self, cpu: &mut Cpu, value: u32) {
        match self {
            Loc::Reg(r) => cpu.set_reg(r, value),
            Loc::Mem(m) => cpu.mem_write(m, value),
        }
    }
}

/// Operand `i` (register or memory of `size` bytes) as a location, checked
/// for `access`. A read-modify-write destination is checked for writing.
#[inline(always)]
pub fn loc(cpu: &mut Cpu, instr: &Instruction, i: u32, size: u8, access: Access) -> CpuResult<Loc> {
    match instr.op_kind(i) {
        OpKind::Register => Ok(Loc::Reg(instr.op_register(i))),
        OpKind::Memory => Ok(Loc::Mem(mem_operand(cpu, instr, size, access)?)),
        _ => Err(Fault::UD),
    }
}

/// Value of source operand `i` of `size` bytes: a register, memory, or an
/// immediate (sign-extended to the operand size where the encoding says so).
#[inline(always)]
pub fn read_op(cpu: &mut Cpu, instr: &Instruction, i: u32, size: u8) -> CpuResult<u32> {
    match instr.op_kind(i) {
        OpKind::Register => Ok(cpu.reg(instr.op_register(i))),
        OpKind::Memory => {
            let m = mem_operand(cpu, instr, size, Access::Read)?;
            Ok(cpu.mem_read(m))
        }
        OpKind::Immediate8
        | OpKind::Immediate8_2nd
        | OpKind::Immediate16
        | OpKind::Immediate32
        | OpKind::Immediate8to16
        | OpKind::Immediate8to32 => Ok(instr.immediate(i) as u32 & size_mask(size)),
        _ => Err(Fault::UD),
    }
}
