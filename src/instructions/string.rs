//! String instructions and their REP/REPE/REPNE prefixes, with 16 or 32-bit
//! addressing (SI/DI/CX or ESI/EDI/ECX).
//!
//! Each iteration commits its index and count updates only after its
//! memory accesses succeeded, so a fault leaves the registers at the
//! faulting iteration and re-executing the instruction resumes there.

use iced_x86::{Instruction, OpKind, Register};

use super::operand::mem_seg;
use crate::cpu::{Access, Cpu, CpuFlags, CpuResult, Seg};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrOp {
    Movs,
    Cmps,
    Scas,
    Lods,
    Stos,
    Ins,
    Outs,
}

/// True when the instruction uses 32-bit addressing (ESI/EDI/ECX).
fn addr32(instr: &Instruction) -> bool {
    (0..instr.op_count()).any(|i| {
        matches!(
            instr.op_kind(i),
            OpKind::MemorySegESI | OpKind::MemorySegEDI | OpKind::MemoryESEDI
        )
    })
}

/// The accumulator of an operand size: AL, AX or EAX.
fn accumulator(size: u8) -> Register {
    match size {
        1 => Register::AL,
        2 => Register::AX,
        _ => Register::EAX,
    }
}

/// Addressing of one string instruction.
struct Addr {
    mask: u32,
    delta: u32,
    src_seg: Seg,
}

impl Addr {
    fn get(&self, cpu: &Cpu, reg: Register) -> u32 {
        cpu.reg(reg) & self.mask
    }

    /// Step SI/ESI or DI/EDI by the operand size in the direction DF says.
    fn step(&self, cpu: &mut Cpu, reg: Register) {
        let full = cpu.reg(reg);
        let next = full.wrapping_add(self.delta);
        cpu.set_reg(reg, (full & !self.mask) | (next & self.mask));
    }
}

/// One iteration: the memory and port accesses, then the index updates.
fn iteration(cpu: &mut Cpu, op: StrOp, size: u8, a: &Addr) -> CpuResult {
    let (si, di) = (Register::ESI, Register::EDI);
    match op {
        StrOp::Movs => {
            let src = cpu.mem_ref(a.src_seg, a.get(cpu, si), size, Access::Read)?;
            let dst = cpu.mem_ref(Seg::ES, a.get(cpu, di), size, Access::Write)?;
            let value = cpu.mem_read(src);
            cpu.mem_write(dst, value);
            a.step(cpu, si);
            a.step(cpu, di);
        }
        StrOp::Stos => {
            let dst = cpu.mem_ref(Seg::ES, a.get(cpu, di), size, Access::Write)?;
            let value = cpu.reg(accumulator(size));
            cpu.mem_write(dst, value);
            a.step(cpu, di);
        }
        StrOp::Lods => {
            let src = cpu.mem_ref(a.src_seg, a.get(cpu, si), size, Access::Read)?;
            let value = cpu.mem_read(src);
            cpu.set_reg(accumulator(size), value);
            a.step(cpu, si);
        }
        StrOp::Cmps => {
            let src = cpu.mem_ref(a.src_seg, a.get(cpu, si), size, Access::Read)?;
            let dst = cpu.mem_ref(Seg::ES, a.get(cpu, di), size, Access::Read)?;
            let (x, y) = (cpu.mem_read(src), cpu.mem_read(dst));
            cpu.alu_sub(size, x, y, false);
            a.step(cpu, si);
            a.step(cpu, di);
        }
        StrOp::Scas => {
            let dst = cpu.mem_ref(Seg::ES, a.get(cpu, di), size, Access::Read)?;
            let (x, y) = (cpu.reg(accumulator(size)), cpu.mem_read(dst));
            cpu.alu_sub(size, x, y, false);
            a.step(cpu, di);
        }
        StrOp::Ins => {
            let dst = cpu.mem_ref(Seg::ES, a.get(cpu, di), size, Access::Write)?;
            let port = cpu.dx();
            let mut value = 0;
            for i in 0..size as u16 {
                value |= (cpu.bus.io_read(port.wrapping_add(i)) as u32) << (8 * i);
            }
            cpu.mem_write(dst, value);
            a.step(cpu, di);
        }
        StrOp::Outs => {
            let src = cpu.mem_ref(a.src_seg, a.get(cpu, si), size, Access::Read)?;
            let value = cpu.mem_read(src);
            let port = cpu.dx();
            for i in 0..size as u16 {
                cpu.bus.io_write(port.wrapping_add(i), (value >> (8 * i)) as u8);
            }
            a.step(cpu, si);
        }
    }
    Ok(())
}

pub fn string(cpu: &mut Cpu, instr: &Instruction, op: StrOp, size: u8) -> CpuResult {
    let a32 = addr32(instr);
    let addr = Addr {
        mask: if a32 { 0xFFFF_FFFF } else { 0xFFFF },
        delta: if cpu.get_cpu_flag(CpuFlags::DF) { (size as u32).wrapping_neg() } else { size as u32 },
        // The source segment can be overridden; the destination is ES.
        src_seg: mem_seg(instr),
    };

    let repe = instr.has_repe_prefix();
    let repne = instr.has_repne_prefix();
    if !repe && !repne {
        return iteration(cpu, op, size, &addr);
    }

    let counter = if a32 { Register::ECX } else { Register::CX };
    let compares = matches!(op, StrOp::Cmps | StrOp::Scas);
    loop {
        let count = cpu.reg(counter);
        if count == 0 {
            return Ok(());
        }
        iteration(cpu, op, size, &addr)?;
        cpu.set_reg(counter, count - 1);
        if compares {
            let zf = cpu.get_cpu_flag(CpuFlags::ZF);
            if (repe && !zf) || (repne && zf) {
                return Ok(());
            }
        }
    }
}
