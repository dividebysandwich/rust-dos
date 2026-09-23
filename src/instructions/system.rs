//! Flag instructions, HLT, and the system instructions: descriptor table
//! registers, the machine status word, control, debug and test registers,
//! the LDT and task registers, the protected-mode segment inspection
//! instructions (LAR, LSL, VERR, VERW, ARPL), and the 486 cache and TLB
//! instructions.

use iced_x86::{Code, Instruction, Register};

use super::operand::{effective_offset, loc, mem_operand, mem_operand_at, mem_seg, op_size, read_op};
use super::transfer::require_486;
use crate::cpu::seg::{
    CALL_GATE16, CALL_GATE32, LDT, TASK_GATE, TSS16_AVAILABLE, TSS16_BUSY, TSS32_AVAILABLE, TSS32_BUSY, is_null,
    rpl, sel_error,
};
use crate::cpu::{
    Access, CR0_AM, CR0_CD, CR0_EM, CR0_ET, CR0_MP, CR0_NE, CR0_NW, CR0_PE, CR0_PG, CR0_TS, CR0_WP, Cpu,
    CpuFlags, CpuModel, CpuResult, CpuState, DescTable, Descriptor, Fault, SegCache,
};

/// Instructions only privilege level 0 may run: in protected and
/// virtual-8086 mode at any other level they raise #GP(0).
fn require_cpl0(cpu: &Cpu) -> CpuResult {
    if cpu.pe() && cpu.cpl != 0 { Err(Fault::gp(0)) } else { Ok(()) }
}

/// Instructions that only exist in protected mode: #UD in real and
/// virtual-8086 mode.
fn require_pm(cpu: &Cpu) -> CpuResult {
    if cpu.pm() { Ok(()) } else { Err(Fault::UD) }
}

/// CLI and STI need CPL <= IOPL in protected mode, and IOPL 3 in
/// virtual-8086 mode.
fn check_iopl(cpu: &Cpu) -> CpuResult {
    if cpu.pe() && cpu.cpl > cpu.iopl() { Err(Fault::gp(0)) } else { Ok(()) }
}

pub fn cli(cpu: &mut Cpu) -> CpuResult {
    check_iopl(cpu)?;
    cpu.set_cpu_flag(CpuFlags::IF, false);
    Ok(())
}

pub fn sti(cpu: &mut Cpu) -> CpuResult {
    check_iopl(cpu)?;
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
    require_cpl0(cpu)?;
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
    require_cpl0(cpu)?;
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
    require_cpl0(cpu)?;
    let src = super::operand::read_op(cpu, instr, 0, 2)?;
    let cr0 = (cpu.cr0 & !0xE) | (src & 0xF);
    set_cr0(cpu, cr0)
}

/// Write CR0. ET is hardwired on a 486, which has the NE, WP, AM, NW and
/// CD bits a 386 lacks. Paging needs protected mode.
fn set_cr0(cpu: &mut Cpu, value: u32) -> CpuResult {
    let mut writable = CR0_PE | CR0_MP | CR0_EM | CR0_TS | CR0_PG;
    if cpu.model == CpuModel::I486 {
        writable |= CR0_NE | CR0_WP | CR0_AM | CR0_NW | CR0_CD;
    } else {
        writable |= CR0_ET;
    }
    let mut value = (value & writable) | (cpu.cr0 & !writable);
    if cpu.model == CpuModel::I486 {
        value |= CR0_ET;
        if value & CR0_NW != 0 && value & CR0_CD == 0 {
            return Err(Fault::gp(0));
        }
    }
    if value & CR0_PG != 0 && value & CR0_PE == 0 {
        return Err(Fault::gp(0));
    }
    let changed = cpu.cr0 ^ value;
    cpu.cr0 = value;
    // Translations are only used with paging on, and PE can't change
    // while it is.
    if changed & (CR0_PG | CR0_WP) != 0 {
        cpu.tlb.flush();
    }
    if changed & CR0_PE != 0 {
        // Real mode runs at level 0, and so does protected mode until the
        // first far jump loads a code segment.
        cpu.cpl = 0;
        cpu.note_mode_switch(value & CR0_PE != 0);
    }
    Ok(())
}

/// MOV CRn/DRn/TRn, r32.
pub fn mov_to_system(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    require_cpl0(cpu)?;
    let dest = instr.op0_register();
    let value = cpu.reg(instr.op1_register());
    match dest {
        Register::CR0 => set_cr0(cpu, value)?,
        Register::CR2 => cpu.cr2 = value,
        Register::CR3 => {
            cpu.cr3 = value;
            cpu.tlb.flush();
        }
        r if r.is_dr() => cpu.dr[r as usize - Register::DR0 as usize] = value,
        // TR6/TR7 (TLB test registers): accepted and ignored.
        r if r.is_tr() => {}
        _ => return Err(Fault::UD),
    }
    Ok(())
}

/// MOV r32, CRn/DRn/TRn.
pub fn mov_from_system(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    require_cpl0(cpu)?;
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
    require_cpl0(cpu)?;
    cpu.cr0 &= !CR0_TS;
    Ok(())
}

/// INVD/WBINVD (486): there are no caches to flush.
pub fn cache_op(cpu: &mut Cpu) -> CpuResult {
    require_486(cpu)?;
    require_cpl0(cpu)
}

/// INVLPG m (486): drop the TLB entry of the page holding the operand.
pub fn invlpg(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    require_486(cpu)?;
    require_cpl0(cpu)?;
    if instr.op0_kind() != iced_x86::OpKind::Memory {
        return Err(Fault::UD);
    }
    let base = cpu.seg_cache(mem_seg(instr)).base;
    let lin = base.wrapping_add(effective_offset(cpu, instr));
    cpu.tlb.flush_page(lin);
    Ok(())
}

/// LLDT r/m16: load the LDT register from a GDT descriptor; a null
/// selector leaves no LDT.
pub fn lldt(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    require_pm(cpu)?;
    require_cpl0(cpu)?;
    let selector = read_op(cpu, instr, 0, 2)? as u16;
    if is_null(selector) {
        cpu.ldtr = SegCache::null(selector);
        return Ok(());
    }
    let err = sel_error(selector);
    if selector & 4 != 0 {
        return Err(Fault::gp(err));
    }
    let desc = cpu.fetch_descriptor(selector, 0)?;
    if desc.is_segment() || desc.typ() != LDT {
        return Err(Fault::gp(err));
    }
    if !desc.present() {
        return Err(Fault::np(err));
    }
    cpu.ldtr = desc.cache(selector);
    Ok(())
}

/// LTR r/m16: load the task register from an available TSS descriptor,
/// which becomes busy.
pub fn ltr(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    require_pm(cpu)?;
    require_cpl0(cpu)?;
    let selector = read_op(cpu, instr, 0, 2)? as u16;
    if is_null(selector) {
        return Err(Fault::gp(0));
    }
    let err = sel_error(selector);
    if selector & 4 != 0 {
        return Err(Fault::gp(err));
    }
    let mut desc = cpu.fetch_descriptor(selector, 0)?;
    if desc.is_segment() || !matches!(desc.typ(), TSS16_AVAILABLE | TSS32_AVAILABLE) {
        return Err(Fault::gp(err));
    }
    if !desc.present() {
        return Err(Fault::np(err));
    }
    let addr = cpu.gdtr.base.wrapping_add((selector & 0xFFF8) as u32 + 5);
    desc.0 |= 2 << 40;
    cpu.sys_write_u8(addr, (desc.0 >> 40) as u8)?;
    cpu.tr = desc.cache(selector);
    Ok(())
}

/// SLDT and STR: store the LDT or task register's selector.
pub fn store_selector(cpu: &mut Cpu, instr: &Instruction, task: bool) -> CpuResult {
    require_pm(cpu)?;
    let size = op_size(instr, 0);
    let dest = loc(cpu, instr, 0, size, Access::Write)?;
    let selector = if task { cpu.tr.selector } else { cpu.ldtr.selector };
    dest.write(cpu, selector as u32);
    Ok(())
}

/// The descriptor a LAR, LSL, VERR or VERW selector names, if the
/// selector is inside its table and the descriptor is visible at the CPL
/// and the selector's RPL (conforming code always is).
fn visible_descriptor(cpu: &mut Cpu, selector: u16) -> CpuResult<Option<Descriptor>> {
    if is_null(selector) {
        return Ok(None);
    }
    let desc = match cpu.fetch_descriptor(selector, 0) {
        Ok(d) => d,
        Err(f) if f.vector == 13 => return Ok(None),
        Err(f) => return Err(f),
    };
    if !desc.conforming() && (desc.dpl() < cpu.cpl || desc.dpl() < rpl(selector)) {
        return Ok(None);
    }
    Ok(Some(desc))
}

/// LAR and LSL: the access rights or the limit of a descriptor into the
/// destination, with ZF telling whether the selector was valid.
pub fn load_access(cpu: &mut Cpu, instr: &Instruction, limit: bool) -> CpuResult {
    require_pm(cpu)?;
    let selector = read_op(cpu, instr, 1, 2)? as u16;
    let found = visible_descriptor(cpu, selector)?.filter(|d| {
        d.is_segment()
            || match d.typ() {
                TSS16_AVAILABLE | LDT | TSS16_BUSY | TSS32_AVAILABLE | TSS32_BUSY => true,
                CALL_GATE16 | TASK_GATE | CALL_GATE32 => !limit,
                _ => false,
            }
    });
    cpu.set_cpu_flag(CpuFlags::ZF, found.is_some());
    if let Some(desc) = found {
        let dest = instr.op0_register();
        let value = if limit {
            desc.limit()
        } else {
            ((desc.0 >> 32) as u32) & if dest.size() == 4 { 0x00F0_FF00 } else { 0xFF00 }
        };
        cpu.set_reg(dest, value);
    }
    Ok(())
}

/// VERR and VERW: whether the segment could be read (or written) at the
/// CPL, in ZF.
pub fn verify(cpu: &mut Cpu, instr: &Instruction, write: bool) -> CpuResult {
    require_pm(cpu)?;
    let selector = read_op(cpu, instr, 0, 2)? as u16;
    let ok = visible_descriptor(cpu, selector)?
        .is_some_and(|d| if write { d.writable_data() } else { d.is_segment() && d.readable() });
    cpu.set_cpu_flag(CpuFlags::ZF, ok);
    Ok(())
}

/// ARPL r/m16, r16: raise the destination selector's RPL to the source's.
/// Memory is only written (and checked for writing) when it changes.
pub fn arpl(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    require_pm(cpu)?;
    let d = read_op(cpu, instr, 0, 2)?;
    let s = cpu.reg(instr.op1_register());
    if d & 3 < s & 3 {
        let dest = loc(cpu, instr, 0, 2, Access::Write)?;
        dest.write(cpu, (d & !3) | (s & 3));
        cpu.set_cpu_flag(CpuFlags::ZF, true);
    } else {
        cpu.set_cpu_flag(CpuFlags::ZF, false);
    }
    Ok(())
}
