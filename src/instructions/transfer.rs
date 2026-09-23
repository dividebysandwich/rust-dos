//! Data transfer: MOV and friends, the stack, far pointer loads, port I/O,
//! and the 486 exchange instructions.

use iced_x86::{Instruction, OpKind, Register};

use super::operand::{effective_offset, loc, mem_operand, mem_operand_at, op_size, read_op};
use crate::cpu::alu::{AF, CF, PF, SF, ZF, sign_extend, size_mask};
use crate::cpu::{Access, Cpu, CpuFlags, CpuModel, CpuResult, Fault, Seg};

/// Load a segment register in real mode. MOV SS and POP SS hold off
/// interrupts for one instruction, so a program can load SP next.
fn load_seg(cpu: &mut Cpu, seg: Seg, value: u16) -> CpuResult {
    if seg == Seg::CS {
        return Err(Fault::UD);
    }
    cpu.load_seg_real(seg, value);
    if seg == Seg::SS {
        cpu.irq_shadow = true;
    }
    Ok(())
}

/// MOV between registers, memory and immediates, including segment,
/// control and debug registers.
pub fn mov(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    if instr.op0_kind() == OpKind::Register {
        let dest = instr.op0_register();
        if let Some(seg) = Seg::from_register(dest) {
            let value = read_op(cpu, instr, 1, 2)? as u16;
            return load_seg(cpu, seg, value);
        }
        if dest.is_cr() || dest.is_dr() || dest.is_tr() {
            return super::system::mov_to_system(cpu, instr);
        }
    }
    if instr.op1_kind() == OpKind::Register {
        let src = instr.op1_register();
        if src.is_cr() || src.is_dr() || src.is_tr() {
            return super::system::mov_from_system(cpu, instr);
        }
        if src.is_segment_register() {
            // MOV r/m16, Sreg. A 32-bit register destination gets the
            // selector zero-extended.
            let size = op_size(instr, 0);
            let dest = loc(cpu, instr, 0, size, Access::Write)?;
            let value = cpu.reg(src);
            dest.write(cpu, value);
            return Ok(());
        }
    }
    let size = op_size(instr, 0);
    let dest = loc(cpu, instr, 0, size, Access::Write)?;
    let value = read_op(cpu, instr, 1, size)?;
    dest.write(cpu, value);
    Ok(())
}

/// MOVZX/MOVSX: move with zero or sign extension.
pub fn movx(cpu: &mut Cpu, instr: &Instruction, signed: bool) -> CpuResult {
    let src_size = op_size(instr, 1);
    let mut value = read_op(cpu, instr, 1, src_size)?;
    if signed {
        value = sign_extend(src_size, value);
    }
    cpu.set_reg(instr.op0_register(), value);
    Ok(())
}

pub fn xchg(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = op_size(instr, 0);
    let a = loc(cpu, instr, 0, size, Access::Write)?;
    let b = loc(cpu, instr, 1, size, Access::Write)?;
    let (va, vb) = (a.read(cpu), b.read(cpu));
    a.write(cpu, vb);
    b.write(cpu, va);
    Ok(())
}

pub fn lea(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let offset = effective_offset(cpu, instr);
    cpu.set_reg(instr.op0_register(), offset);
    Ok(())
}

/// LDS/LES/LFS/LGS/LSS: load a far pointer (offset, then selector) from
/// memory into a register and a segment register.
pub fn load_far_pointer(cpu: &mut Cpu, instr: &Instruction, seg: Seg) -> CpuResult {
    if instr.op1_kind() != OpKind::Memory {
        return Err(Fault::UD);
    }
    let size = op_size(instr, 0);
    let off_ref = mem_operand(cpu, instr, size, Access::Read)?;
    let sel_ref = mem_operand_at(cpu, instr, size as u32, 2, Access::Read)?;
    let offset = cpu.mem_read(off_ref);
    let selector = cpu.mem_read(sel_ref) as u16;
    cpu.load_seg_real(seg, selector);
    cpu.set_reg(instr.op0_register(), offset);
    Ok(())
}

/// Bytes a PUSH or POP moves: 2 or 4.
#[inline(always)]
fn stack_size(instr: &Instruction) -> u8 {
    instr.stack_pointer_increment().unsigned_abs() as u8
}

pub fn push(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = stack_size(instr);
    let value = read_op(cpu, instr, 0, size)?;
    cpu.push_sized(size, value)
}

/// POP to a register, a segment register or memory. A memory destination
/// addressed through ESP is addressed with the incremented ESP.
pub fn pop(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = stack_size(instr);
    if instr.op0_kind() == OpKind::Register {
        if let Some(seg) = Seg::from_register(instr.op0_register()) {
            // A segment register pop reads only the selector word, also
            // with a 32-bit operand size, then releases the whole slot.
            let selector = cpu.stack_read(0, 2)? as u16;
            load_seg(cpu, seg, selector)?;
            let sp = cpu.stack_ptr().wrapping_add(size as u32);
            cpu.set_stack_ptr(sp);
            return Ok(());
        }
    }
    let value = cpu.pop_sized(size)?;
    match instr.op0_kind() {
        OpKind::Register => cpu.set_reg(instr.op0_register(), value),
        _ => {
            let dest = mem_operand(cpu, instr, size, Access::Write)?;
            cpu.mem_write(dest, value);
        }
    }
    Ok(())
}

/// PUSHA/PUSHAD: push the eight general-purpose registers, with the stack
/// pointer as it was before the first push. The 386 stores them from the
/// lowest address up, so when a slot faults, the ones below it are written.
pub fn pusha(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = stack_size(instr) / 8;
    let mask = size_mask(size);
    let sp = cpu.esp() & mask;
    let values = [cpu.eax(), cpu.ecx(), cpu.edx(), cpu.ebx(), sp, cpu.ebp(), cpu.esi(), cpu.edi()];
    let total = 8 * size as u32;
    for (i, v) in values.iter().enumerate().rev() {
        let depth = total - (i as u32 + 1) * size as u32;
        cpu.stack_write_below(total, depth, size, v & mask)?;
    }
    let sp = cpu.stack_ptr().wrapping_sub(total);
    cpu.set_stack_ptr(sp);
    Ok(())
}

/// POPA/POPAD: pop the general-purpose registers pushed by PUSHA, skipping
/// the stack pointer. They load one by one: a fault leaves the registers
/// popped before it loaded.
pub fn popa(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = stack_size(instr) / 8;
    let regs: [Register; 8] = if size == 2 {
        [Register::DI, Register::SI, Register::BP, Register::None, Register::BX, Register::DX, Register::CX, Register::AX]
    } else {
        [Register::EDI, Register::ESI, Register::EBP, Register::None, Register::EBX, Register::EDX, Register::ECX, Register::EAX]
    };
    let mut esp_image = 0;
    for (i, reg) in regs.into_iter().enumerate() {
        let v = cpu.stack_read(i as u32 * size as u32, size)?;
        if reg == Register::None {
            esp_image = v;
        } else {
            cpu.set_reg(reg, v);
        }
    }
    let sp = cpu.stack_ptr().wrapping_add(8 * size as u32);
    cpu.set_stack_ptr(sp);
    if size == 4 && !cpu.stack32() && cpu.model == CpuModel::I386 {
        // POPAD on a 16-bit stack: a 386 loads the upper half of ESP from
        // the ESP image it otherwise skips.
        let esp = (esp_image & 0xFFFF_0000) | (cpu.esp() & 0xFFFF);
        cpu.set_esp(esp);
    }
    Ok(())
}

pub fn pushf(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    if stack_size(instr) == 2 {
        let flags = cpu.flags16();
        cpu.push_sized(2, flags as u32)
    } else {
        let eflags = cpu.eflags_image();
        cpu.push_sized(4, eflags)
    }
}

pub fn popf(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = stack_size(instr);
    let value = cpu.pop_sized(size)?;
    if size == 2 {
        cpu.load_flags16(value as u16);
    } else {
        cpu.load_eflags(value);
    }
    Ok(())
}

/// Port of IN/OUT: an immediate byte or DX.
fn port(cpu: &Cpu, instr: &Instruction, i: u32) -> u16 {
    if instr.op_kind(i) == OpKind::Register {
        cpu.dx()
    } else {
        instr.immediate8() as u16
    }
}

/// IN AL/AX/EAX. Wider reads take consecutive byte ports.
pub fn port_in(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let dest = instr.op0_register();
    let port = port(cpu, instr, 1);
    let mut value = 0;
    for i in 0..dest.size() as u16 {
        value |= (cpu.bus.io_read(port.wrapping_add(i)) as u32) << (8 * i);
    }
    cpu.set_reg(dest, value);
    Ok(())
}

/// OUT AL/AX/EAX. Wider writes go to consecutive byte ports, low byte
/// first: EGA/VGA code programs index and data registers with one
/// OUT DX, AX.
pub fn port_out(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let src = instr.op1_register();
    let port = port(cpu, instr, 0);
    let value = cpu.reg(src);
    for i in 0..src.size() as u16 {
        cpu.bus.io_write(port.wrapping_add(i), (value >> (8 * i)) as u8);
    }
    Ok(())
}

/// XLAT: AL = [seg:(E)BX + AL]. iced describes the operand as a memory
/// reference with base (E)BX and index AL.
pub fn xlat(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let m = mem_operand(cpu, instr, 1, Access::Read)?;
    let value = cpu.mem_read(m);
    cpu.set_reg(Register::AL, value);
    Ok(())
}

/// LAHF: SF, ZF, AF, PF, CF (and the always-set bit 1) into AH.
pub fn lahf(cpu: &mut Cpu) -> CpuResult {
    let flags = cpu.get_cpu_flags().bits();
    cpu.set_reg(Register::AH, (flags & (SF | ZF | AF | PF | CF)) | 0x02);
    Ok(())
}

/// SAHF: AH into SF, ZF, AF, PF and CF.
pub fn sahf(cpu: &mut Cpu) -> CpuResult {
    let ah = cpu.get_ah() as u32;
    cpu.set_flag_bits(SF | ZF | AF | PF | CF, ah);
    Ok(())
}

/// SALC (undocumented D6): AL = CF ? FFh : 00h.
pub fn salc(cpu: &mut Cpu) -> CpuResult {
    let value = if cpu.get_cpu_flag(CpuFlags::CF) { 0xFF } else { 0 };
    cpu.set_reg(Register::AL, value);
    Ok(())
}

/// Raise #UD for 486 instructions on a 386.
pub fn require_486(cpu: &Cpu) -> CpuResult {
    if cpu.model == CpuModel::I386 { Err(Fault::UD) } else { Ok(()) }
}

/// BSWAP r32 (486).
pub fn bswap(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    require_486(cpu)?;
    let reg = instr.op0_register();
    let value = cpu.reg(reg);
    // With a 16-bit operand the result is undefined; a 486 clears it.
    let r = if reg.size() == 2 { 0 } else { value.swap_bytes() };
    cpu.set_reg(reg, r);
    Ok(())
}

/// XADD r/m, reg (486): exchange, then store the sum in the destination.
pub fn xadd(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    require_486(cpu)?;
    let size = op_size(instr, 0);
    let dest = loc(cpu, instr, 0, size, Access::Write)?;
    let src_reg = instr.op1_register();
    let (d, s) = (dest.read(cpu), cpu.reg(src_reg));
    let sum = cpu.alu_add(size, d, s, false);
    cpu.set_reg(src_reg, d);
    dest.write(cpu, sum);
    Ok(())
}

/// CMPXCHG r/m, reg (486): if the accumulator equals the destination, store
/// the source there (ZF=1); otherwise load the destination into the
/// accumulator (ZF=0). The destination is always written.
pub fn cmpxchg(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    require_486(cpu)?;
    let size = op_size(instr, 0);
    let dest = loc(cpu, instr, 0, size, Access::Write)?;
    let acc = match size {
        1 => Register::AL,
        2 => Register::AX,
        _ => Register::EAX,
    };
    let d = dest.read(cpu);
    let a = cpu.reg(acc);
    cpu.alu_sub(size, a, d, false);
    if a == d {
        let src = cpu.reg(instr.op1_register());
        dest.write(cpu, src);
    } else {
        dest.write(cpu, d);
        cpu.set_reg(acc, d);
    }
    Ok(())
}
