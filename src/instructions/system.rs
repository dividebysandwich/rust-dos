//! Flag instructions, HLT, and the system instructions that also work in
//! real mode: LGDT/LIDT/SGDT/SIDT, SMSW/LMSW, MOV to and from control,
//! debug and test registers, CLTS, and the 486 cache instructions.

use iced_x86::{Code, Instruction, Register};

use super::operand::{loc, mem_operand, mem_operand_at};
use super::transfer::require_486;
use crate::cpu::{Access, CR0_ET, CR0_MP, CR0_PE, CR0_TS, Cpu, CpuFlags, CpuModel, CpuResult, CpuState, DescTable, Fault};

pub fn cli(cpu: &mut Cpu) -> CpuResult {
    cpu.set_cpu_flag(CpuFlags::IF, false);
    Ok(())
}

pub fn sti(cpu: &mut Cpu) -> CpuResult {
    // Interrupts become deliverable only after the next instruction.
    if !cpu.get_cpu_flag(CpuFlags::IF) {
        cpu.irq_shadow = true;
    }
    cpu.set_cpu_flag(CpuFlags::IF, true);
    Ok(())
}

pub fn set_flag(cpu: &mut Cpu, flag: CpuFlags, value: bool) -> CpuResult {
    cpu.set_cpu_flag(flag, value);
    Ok(())
}

pub fn cmc(cpu: &mut Cpu) -> CpuResult {
    let cf = cpu.get_cpu_flag(CpuFlags::CF);
    cpu.set_cpu_flag(CpuFlags::CF, !cf);
    Ok(())
}

/// HLT: stop until an interrupt.
pub fn hlt(cpu: &mut Cpu) -> CpuResult {
    cpu.state = CpuState::Halted;
    Ok(())
}

/// WAIT/FWAIT: #NM when the FPU belongs to another task (CR0.MP and TS).
pub fn wait(cpu: &mut Cpu) -> CpuResult {
    if cpu.cr0 & (CR0_MP | CR0_TS) == (CR0_MP | CR0_TS) {
        return Err(Fault::NM);
    }
    Ok(())
}

/// LGDT/LIDT: load a descriptor table register from a 6-byte operand. With
/// a 16-bit operand size only 24 bits of the base are used.
pub fn load_table(cpu: &mut Cpu, instr: &Instruction, idt: bool) -> CpuResult {
    let limit_ref = mem_operand(cpu, instr, 2, Access::Read)?;
    let base_ref = mem_operand_at(cpu, instr, 2, 4, Access::Read)?;
    let limit = cpu.mem_read(limit_ref) as u16;
    let mut base = cpu.mem_read(base_ref);
    if matches!(instr.code(), Code::Lgdt_m1632_16 | Code::Lidt_m1632_16) {
        base &= 0x00FF_FFFF;
    }
    let table = DescTable { base, limit };
    if idt {
        cpu.idtr = table;
    } else {
        cpu.gdtr = table;
    }
    Ok(())
}

/// SGDT/SIDT: store a descriptor table register. With a 16-bit operand
/// size the base's top byte is stored as 0.
pub fn store_table(cpu: &mut Cpu, instr: &Instruction, idt: bool) -> CpuResult {
    let limit_ref = mem_operand(cpu, instr, 2, Access::Write)?;
    let base_ref = mem_operand_at(cpu, instr, 2, 4, Access::Write)?;
    let table = if idt { cpu.idtr } else { cpu.gdtr };
    let mut base = table.base;
    if matches!(instr.code(), Code::Sgdt_m1632_16 | Code::Sidt_m1632_16) {
        base &= 0x00FF_FFFF;
    }
    cpu.mem_write(limit_ref, table.limit as u32);
    cpu.mem_write(base_ref, base);
    Ok(())
}

/// SMSW: the low word of CR0 (the machine status word).
pub fn smsw(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let size = if instr.op0_kind() == iced_x86::OpKind::Register { instr.op0_register().size() as u8 } else { 2 };
    let dest = loc(cpu, instr, 0, size, Access::Write)?;
    let value = if size == 4 { cpu.cr0 } else { cpu.cr0 & 0xFFFF };
    dest.write(cpu, value);
    Ok(())
}

/// LMSW: load PE, MP, EM and TS. It can set PE but not clear it.
pub fn lmsw(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let src = super::operand::read_op(cpu, instr, 0, 2)?;
    let cr0 = (cpu.cr0 & !0xE) | (src & 0xF);
    set_cr0(cpu, cr0);
    Ok(())
}

/// Write CR0. ET is hardwired on a 486.
fn set_cr0(cpu: &mut Cpu, value: u32) {
    let mut value = value;
    if cpu.model == CpuModel::I486 {
        value |= CR0_ET;
    }
    if value & CR0_PE != 0 && cpu.cr0 & CR0_PE == 0 {
        cpu.bus.log_string(&format!(
            "[CPU] Protected mode enabled at {:04X}:{:08X} (not supported yet)",
            cpu.cs(),
            cpu.eip()
        ));
    }
    cpu.cr0 = value;
}

/// MOV CRn/DRn/TRn, r32.
pub fn mov_to_system(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let dest = instr.op0_register();
    let value = cpu.reg(instr.op1_register());
    match dest {
        Register::CR0 => set_cr0(cpu, value),
        Register::CR2 => cpu.cr2 = value,
        Register::CR3 => cpu.cr3 = value,
        r if r.is_dr() => cpu.dr[r as usize - Register::DR0 as usize] = value,
        // TR6/TR7 (TLB test registers): accepted and ignored.
        r if r.is_tr() => {}
        _ => return Err(Fault::UD),
    }
    Ok(())
}

/// MOV r32, CRn/DRn/TRn.
pub fn mov_from_system(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let value = match instr.op1_register() {
        Register::CR0 => cpu.cr0,
        Register::CR2 => cpu.cr2,
        Register::CR3 => cpu.cr3,
        r if r.is_dr() => cpu.dr[r as usize - Register::DR0 as usize],
        r if r.is_tr() => 0,
        _ => return Err(Fault::UD),
    };
    cpu.set_reg(instr.op0_register(), value);
    Ok(())
}

/// CLTS: clear CR0.TS.
pub fn clts(cpu: &mut Cpu) -> CpuResult {
    cpu.cr0 &= !CR0_TS;
    Ok(())
}

/// INVD/WBINVD/INVLPG (486): there are no caches to flush.
pub fn cache_op(cpu: &mut Cpu) -> CpuResult {
    require_486(cpu)
}
