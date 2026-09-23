//! Control transfer: jumps, calls and returns (near and far, 16 and 32-bit
//! operand size), conditional jumps and loops, software interrupts, IRET,
//! BOUND, and ENTER/LEAVE.

use iced_x86::{Code, Instruction, MemorySize, OpKind, Register};

use super::logic::condition;
use super::operand::{mem_operand, mem_operand_at, op_size, read_op};
use crate::cpu::{Access, Cpu, CpuFlags, CpuResult, Fault, IntSource, Seg};

/// Operand size of a near branch: 2 or 4 bytes.
#[inline(always)]
fn branch_size(instr: &Instruction) -> u8 {
    if instr.op0_kind() == OpKind::NearBranch32 { 4 } else { 2 }
}

/// Jump to `target` in the current code segment, wrapped to the operand
/// size. A target past the CS limit raises #GP(0).
#[inline(always)]
fn jump_near(cpu: &mut Cpu, target: u32, size: u8) -> CpuResult {
    let target = if size == 2 { target & 0xFFFF } else { target };
    if target > cpu.seg_cache(Seg::CS).limit {
        return Err(Fault::gp(0));
    }
    cpu.set_eip(target);
    Ok(())
}

/// Load CS:EIP for a far transfer in real or virtual-8086 mode.
fn jump_far_real(cpu: &mut Cpu, selector: u16, offset: u32) -> CpuResult {
    if offset > cpu.seg_cache(Seg::CS).limit {
        return Err(Fault::gp(0));
    }
    if cpu.v86() {
        cpu.load_seg_v86(Seg::CS, selector);
    } else {
        cpu.load_seg_real(Seg::CS, selector);
    }
    cpu.set_eip(offset);
    Ok(())
}

/// JMP far: in protected mode to a code segment, through a call gate, or
/// to another task.
fn jump_far(cpu: &mut Cpu, selector: u16, offset: u32) -> CpuResult {
    if cpu.pm() {
        return cpu.jmp_far_pm(selector, offset);
    }
    jump_far_real(cpu, selector, offset)
}

/// CALL far with `size`-byte return address slots.
fn call_far(cpu: &mut Cpu, selector: u16, offset: u32, size: u8) -> CpuResult {
    if cpu.pm() {
        return cpu.call_far_pm(selector, offset, size);
    }
    let (cs, eip) = (cpu.cs(), cpu.eip());
    cpu.push_sized(size, cs as u32)?;
    cpu.push_sized(size, eip)?;
    jump_far_real(cpu, selector, offset)
}

/// Offset size of a far pointer in memory (m16:16 or m16:32).
fn far_pointer_size(instr: &Instruction) -> u8 {
    if instr.memory_size() == MemorySize::SegPtr32 { 4 } else { 2 }
}

/// Read a far pointer operand: (selector, offset).
fn read_far_pointer(cpu: &mut Cpu, instr: &Instruction) -> CpuResult<(u16, u32, u8)> {
    let size = far_pointer_size(instr);
    let off_ref = mem_operand(cpu, instr, size, Access::Read)?;
    let sel_ref = mem_operand_at(cpu, instr, size as u32, 2, Access::Read)?;
    Ok((cpu.mem_read(sel_ref) as u16, cpu.mem_read(off_ref), size))
}

pub fn jmp(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    match instr.op0_kind() {
        OpKind::NearBranch16 | OpKind::NearBranch32 => {
            jump_near(cpu, instr.near_branch_target() as u32, branch_size(instr))
        }
        OpKind::FarBranch16 => jump_far(cpu, instr.far_branch_selector(), instr.far_branch16() as u32),
        OpKind::FarBranch32 => jump_far(cpu, instr.far_branch_selector(), instr.far_branch32()),
        _ => {
            if matches!(instr.code(), Code::Jmp_m1616 | Code::Jmp_m1632) {
                let (selector, offset, _) = read_far_pointer(cpu, instr)?;
                jump_far(cpu, selector, offset)
            } else {
                let size = op_size(instr, 0);
                let target = read_op(cpu, instr, 0, size)?;
                jump_near(cpu, target, size)
            }
        }
    }
}

pub fn call(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let eip = cpu.eip();
    match instr.op0_kind() {
        OpKind::NearBranch16 | OpKind::NearBranch32 => {
            let size = branch_size(instr);
            let target = instr.near_branch_target() as u32;
            cpu.push_sized(size, eip)?;
            jump_near(cpu, target, size)
        }
        OpKind::FarBranch16 | OpKind::FarBranch32 => {
            let (size, offset) = if instr.op0_kind() == OpKind::FarBranch16 {
                (2, instr.far_branch16() as u32)
            } else {
                (4, instr.far_branch32())
            };
            call_far(cpu, instr.far_branch_selector(), offset, size)
        }
        _ => {
            if matches!(instr.code(), Code::Call_m1616 | Code::Call_m1632) {
                let (selector, offset, size) = read_far_pointer(cpu, instr)?;
                call_far(cpu, selector, offset, size)
            } else {
                let size = op_size(instr, 0);
                let target = read_op(cpu, instr, 0, size)?;
                cpu.push_sized(size, eip)?;
                jump_near(cpu, target, size)
            }
        }
    }
}

/// Extra bytes a RET releases (RET imm16), or 0.
fn ret_release(instr: &Instruction) -> u32 {
    if instr.op_count() == 1 { instr.immediate16() as u32 } else { 0 }
}

pub fn ret_near(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = if matches!(instr.code(), Code::Retnd | Code::Retnd_imm16) { 4 } else { 2 };
    let target = cpu.stack_read(0, size)?;
    jump_near(cpu, target, size)?;
    let sp = cpu.stack_ptr().wrapping_add(size as u32 + ret_release(instr));
    cpu.set_stack_ptr(sp);
    Ok(())
}

pub fn ret_far(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = if matches!(instr.code(), Code::Retfd | Code::Retfd_imm16) { 4 } else { 2 };
    if cpu.pm() {
        return cpu.ret_far_pm(size, ret_release(instr));
    }
    let offset = cpu.stack_read(0, size)?;
    let selector = cpu.stack_read(size as u32, size)? as u16;
    let offset = if size == 2 { offset & 0xFFFF } else { offset };
    jump_far_real(cpu, selector, offset)?;
    let sp = cpu.stack_ptr().wrapping_add(2 * size as u32 + ret_release(instr));
    cpu.set_stack_ptr(sp);
    Ok(())
}

pub fn jcc(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    if condition(cpu, instr.condition_code()) {
        jump_near(cpu, instr.near_branch_target() as u32, branch_size(instr))?;
    }
    Ok(())
}

/// The counter a LOOPcc or JCXZ uses, CX or ECX, which the address size
/// selects.
fn count_register(instr: &Instruction) -> Register {
    match instr.code() {
        Code::Loop_rel8_16_ECX
        | Code::Loop_rel8_32_ECX
        | Code::Loope_rel8_16_ECX
        | Code::Loope_rel8_32_ECX
        | Code::Loopne_rel8_16_ECX
        | Code::Loopne_rel8_32_ECX
        | Code::Jecxz_rel8_16
        | Code::Jecxz_rel8_32 => Register::ECX,
        _ => Register::CX,
    }
}

/// LOOP/LOOPE/LOOPNE: decrement the counter and jump while it's not zero
/// (and ZF matches).
pub fn loop_op(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let reg = count_register(instr);
    let mask = if reg == Register::CX { 0xFFFF } else { 0xFFFF_FFFF };
    let count = cpu.reg(reg).wrapping_sub(1) & mask;
    let zf = cpu.get_cpu_flag(CpuFlags::ZF);
    let taken = count != 0
        && match instr.code() {
            Code::Loope_rel8_16_CX | Code::Loope_rel8_32_CX | Code::Loope_rel8_16_ECX | Code::Loope_rel8_32_ECX => zf,
            Code::Loopne_rel8_16_CX
            | Code::Loopne_rel8_32_CX
            | Code::Loopne_rel8_16_ECX
            | Code::Loopne_rel8_32_ECX => !zf,
            _ => true,
        };
    if taken {
        jump_near(cpu, instr.near_branch_target() as u32, branch_size(instr))?;
    }
    cpu.set_reg(reg, count);
    Ok(())
}

/// JCXZ/JECXZ: jump if the counter is zero.
pub fn jcxz(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    if cpu.reg(count_register(instr)) == 0 {
        jump_near(cpu, instr.near_branch_target() as u32, branch_size(instr))?;
    }
    Ok(())
}

/// A software interrupt: INT n, INT3, INT1 or INTO. EIP already points at
/// the next instruction. In real mode, a vector the interrupt table leaves
/// at 0000:0000 is skipped, as programs call interrupts nothing has
/// installed yet.
pub fn software_interrupt(cpu: &mut Cpu, vector: u8) -> CpuResult {
    if !cpu.pe() {
        let entry = cpu.idtr.base.wrapping_add(vector as u32 * 4);
        if cpu.read_linear_u16(entry) == 0 && cpu.read_linear_u16(entry.wrapping_add(2)) == 0 {
            cpu.note_null_interrupt(vector);
            return Ok(());
        }
    }
    cpu.deliver_interrupt(vector, IntSource::Software, None)
}

pub fn int(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    // In virtual-8086 mode INT n is for the monitor to emulate unless
    // IOPL is 3.
    if cpu.v86() && cpu.iopl() < 3 {
        return Err(Fault::gp(0));
    }
    software_interrupt(cpu, instr.immediate8())
}

pub fn into(cpu: &mut Cpu) -> CpuResult {
    if cpu.get_cpu_flag(CpuFlags::OF) {
        software_interrupt(cpu, 4)?;
    }
    Ok(())
}

/// IRET/IRETD: pop (E)IP, CS and (E)FLAGS. In virtual-8086 mode only
/// with IOPL 3, and then without changing IOPL.
pub fn iret(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = if instr.code() == Code::Iretd { 4 } else { 2 };
    if cpu.pm() {
        return cpu.iret_pm(size);
    }
    if cpu.v86() && cpu.iopl() < 3 {
        return Err(Fault::gp(0));
    }
    let offset = cpu.stack_read(0, size)?;
    let selector = cpu.stack_read(size as u32, size)? as u16;
    let flags = cpu.stack_read(2 * size as u32, size)?;
    let offset = if size == 2 { offset & 0xFFFF } else { offset };
    jump_far_real(cpu, selector, offset)?;
    let sp = cpu.stack_ptr().wrapping_add(3 * size as u32);
    cpu.set_stack_ptr(sp);
    if cpu.v86() {
        cpu.load_flags_pm(flags, size, true);
    } else if size == 2 {
        cpu.load_flags16(flags as u16);
    } else {
        cpu.load_eflags(flags);
    }
    Ok(())
}

/// BOUND: #BR unless the signed register value is within the bounds pair
/// in memory.
pub fn bound(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    if instr.op1_kind() != OpKind::Memory {
        return Err(Fault::UD);
    }
    let size = op_size(instr, 0);
    let lo_ref = mem_operand(cpu, instr, size, Access::Read)?;
    let hi_ref = mem_operand_at(cpu, instr, size as u32, size, Access::Read)?;
    let sx = |v: u32| crate::cpu::alu::sign_extend(size, v) as i32;
    let value = sx(cpu.reg(instr.op0_register()));
    let (lo, hi) = (sx(cpu.mem_read(lo_ref)), sx(cpu.mem_read(hi_ref)));
    if value < lo || value > hi {
        return Err(Fault::BR);
    }
    Ok(())
}

/// ENTER alloc, level: build a stack frame, copying `level - 1` frame
/// pointers of the enclosing frames.
pub fn enter(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size: u8 = if instr.code() == Code::Enterd_imm16_imm8 { 4 } else { 2 };
    let alloc = instr.immediate16() as u32;
    let level = instr.immediate8_2nd() & 0x1F;
    let stack32 = cpu.stack32();

    let ebp = cpu.ebp();
    cpu.push_sized(size, ebp)?;
    // The frame pointer is all of ESP, whose upper half a 16-bit stack
    // leaves alone.
    let frame = cpu.esp();

    if level > 0 {
        let mut bp = if stack32 { ebp } else { ebp & 0xFFFF };
        for _ in 1..level {
            bp = bp.wrapping_sub(size as u32);
            if !stack32 {
                bp &= 0xFFFF;
            }
            let value = cpu.read_sized(Seg::SS, bp, size)?;
            cpu.push_sized(size, value)?;
        }
        cpu.push_sized(size, frame)?;
    }

    let sp = cpu.stack_ptr().wrapping_sub(alloc);
    let sp = if stack32 { sp } else { sp & 0xFFFF };
    // The 386 finishes with a write check at the final stack pointer: a
    // frame reaching into an unwritable page faults.
    cpu.mem_ref(Seg::SS, sp, size, Access::Write)?;
    if size == 4 {
        cpu.set_ebp(frame);
    } else {
        cpu.set_bp(frame as u16);
    }
    cpu.set_stack_ptr(sp);
    Ok(())
}

/// LEAVE: release the frame (stack pointer = frame pointer) and pop the
/// caller's frame pointer.
pub fn leave(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size: u8 = if instr.code() == Code::Leaved { 4 } else { 2 };
    let frame = cpu.ebp();
    cpu.set_stack_ptr(frame);
    let value = cpu.pop_sized(size)?;
    if size == 4 {
        cpu.set_ebp(value);
    } else {
        cpu.set_bp(value as u16);
    }
    Ok(())
}
