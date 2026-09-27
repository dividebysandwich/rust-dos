//! String instructions and their REP/REPE/REPNE prefixes, with 16 or 32-bit
//! addressing (SI/DI/CX or ESI/EDI/ECX).
//!
//! Each iteration commits its index and count updates only after its
//! memory accesses succeeded, so a fault leaves the registers at the
//! faulting iteration and re-executing the instruction resumes there.

use iced_x86::{Instruction, OpKind, Register};

use super::operand::mem_seg;
use crate::cpu::{Access, Cpu, CpuFlags, CpuResult, MemRef, Seg};

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

/// How many of `max` elements of `size` bytes an access at `seg:off` on
/// (down with `backward`) reaches without a fault through the segment or
/// the addressing, and without leaving the page the first is in. 0 where
/// the first can't go without the checks of an iteration.
fn span(cpu: &Cpu, seg: Seg, off: u32, size: u32, max: u32, backward: bool, mask: u32, write: bool) -> u32 {
    use crate::cpu::layout::{RIGHT_READ, RIGHT_WRITE};
    let c = cpu.seg_cache(seg);
    let need = if write { RIGHT_WRITE } else { RIGHT_READ };
    let last = off.wrapping_add(size - 1);
    if c.rights & need == 0 || last < off || last > mask || off < c.lo || last > c.hi {
        return 0;
    }
    let in_page = c.base.wrapping_add(off) & 0xFFF;
    if in_page > 0x1000 - size {
        return 0;
    }
    let shift = size.trailing_zeros();
    let (by_limit, by_page) = if backward {
        (((off - c.lo) >> shift) + 1, (in_page >> shift) + 1)
    } else {
        (((mask.min(c.hi) - last) >> shift) + 1, (0x1000 - in_page) >> shift)
    };
    max.min(by_limit).min(by_page)
}

/// Element `k` of a run in one page from the checked first one, `delta`
/// bytes apart.
fn element(first: MemRef, delta: u32, k: u32) -> MemRef {
    let step = delta.wrapping_mul(k);
    MemRef { lin: first.lin.wrapping_add(step), phys: first.phys.wrapping_add(step), ..first }
}

/// What `bulk` did.
enum Bulk {
    /// These iterations.
    Done(u32),
    /// None: the next goes on its own.
    One,
}

/// The iterations of a REP MOVS or STOS from here on that stay in the pages
/// and segment limits their first elements are in, done at once: they are
/// the iterations one at a time would do, with the first one's checks and
/// page walks (the others' go through the TLB as they did, changing
/// nothing). In plain RAM, the elements are moved together; elsewhere (the
/// video memory) one by one through the bus, as each iteration would.
fn bulk(cpu: &mut Cpu, op: StrOp, size: u8, a: &Addr, count: u32) -> CpuResult<Bulk> {
    let backward = a.delta != size as u32;
    let bytes = size as u32;
    let di = a.get(cpu, Register::EDI);
    let mut n = span(cpu, Seg::ES, di, bytes, count, backward, a.mask, true);
    let si = a.get(cpu, Register::ESI);
    if op == StrOp::Movs {
        n = span(cpu, a.src_seg, si, bytes, n, backward, a.mask, false);
    }
    if n < 2 {
        return Ok(Bulk::One);
    }
    let len = (n * bytes) as usize;
    // The range from the first element's (up, or down).
    let range = |phys: u32| if backward { phys as usize + bytes as usize - len } else { phys as usize };
    match op {
        StrOp::Movs => {
            let src = cpu.mem_ref(a.src_seg, si, size, Access::Read)?;
            let dst = cpu.mem_ref(Seg::ES, di, size, Access::Write)?;
            let src_ram = cpu.bus.is_plain_ram(range(src.phys), len);
            if src_ram && cpu.bus.is_plain_ram(range(dst.phys), len) {
                cpu.bus.move_elements(src.phys as usize, dst.phys as usize, n as usize, size as usize, backward);
            } else if !(src_ram && cpu.bus.move_to_vga(range(src.phys), range(dst.phys), len)) {
                for k in 0..n {
                    let value = cpu.mem_read(element(src, a.delta, k));
                    cpu.mem_write(element(dst, a.delta, k), value);
                }
            }
        }
        _ => {
            let dst = cpu.mem_ref(Seg::ES, di, size, Access::Write)?;
            let value = cpu.reg(accumulator(size));
            if cpu.bus.is_plain_ram(range(dst.phys), len) {
                cpu.bus.fill_elements(range(dst.phys), n as usize, size as usize, value);
            } else if !cpu.bus.fill_vga(range(dst.phys), n as usize, size as usize, value) {
                for k in 0..n {
                    cpu.mem_write(element(dst, a.delta, k), value);
                }
            }
        }
    }
    let moved = a.delta.wrapping_mul(n);
    for reg in if op == StrOp::Movs { &[Register::ESI, Register::EDI][..] } else { &[Register::EDI][..] } {
        let full = cpu.reg(*reg);
        cpu.set_reg(*reg, (full & !a.mask) | (full.wrapping_add(moved) & a.mask));
    }
    Ok(Bulk::Done(n))
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
            let port = cpu.dx();
            cpu.check_io(port, size)?;
            let dst = cpu.mem_ref(Seg::ES, a.get(cpu, di), size, Access::Write)?;
            let value = match size {
                1 => cpu.bus.io_read(port) as u32,
                _ => cpu.bus.io_read_wide(port, size),
            };
            cpu.mem_write(dst, value);
            a.step(cpu, di);
        }
        StrOp::Outs => {
            let port = cpu.dx();
            cpu.check_io(port, size)?;
            let src = cpu.mem_ref(a.src_seg, a.get(cpu, si), size, Access::Read)?;
            let value = cpu.mem_read(src);
            match size {
                1 => cpu.bus.io_write(port, value as u8),
                _ => cpu.bus.io_write_wide(port, value, size),
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
    // Traced (TF), the single-step trap follows every iteration, and comes
    // back to the instruction while iterations remain.
    let traced = cpu.get_cpu_flag(CpuFlags::TF);
    let bulky = !traced && cpu.string_bulk && matches!(op, StrOp::Movs | StrOp::Stos);
    loop {
        let count = cpu.reg(counter);
        if count == 0 {
            return Ok(());
        }
        if bulky {
            match bulk(cpu, op, size, &addr, count)? {
                Bulk::Done(n) => {
                    cpu.set_reg(counter, count - n);
                    continue;
                }
                Bulk::One => {}
            }
        }
        iteration(cpu, op, size, &addr)?;
        cpu.set_reg(counter, count - 1);
        if compares {
            let zf = cpu.get_cpu_flag(CpuFlags::ZF);
            if (repe && !zf) || (repne && zf) {
                return Ok(());
            }
        }
        if traced && count != 1 {
            cpu.set_eip(cpu.eip().wrapping_sub(instr.len() as u32));
            return Ok(());
        }
    }
}
