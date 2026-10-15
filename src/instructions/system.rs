//! Flag instructions, HLT, and the system instructions: descriptor table
//! registers, the machine status word, control, debug and test registers,
//! the LDT and task registers, the protected-mode segment inspection
//! instructions (LAR, LSL, VERR, VERW, ARPL), the 486 cache and TLB
//! instructions, the Pentium's CPUID, time stamp counter and MSRs, the
//! Pentium II's SYSENTER and SYSEXIT, and the Pentium III's CPUID leaves
//! and MSRs.

use iced_x86::{Code, Instruction, Register};

use super::operand::{effective_offset, loc, mem_operand, mem_operand_at, mem_seg, op_size, read_op};
use super::transfer::{require_486, require_pentium};
use crate::cpu::seg::{
    CALL_GATE16, CALL_GATE32, LDT, TASK_GATE, TSS16_AVAILABLE, TSS16_BUSY, TSS32_AVAILABLE, TSS32_BUSY, is_null,
    rpl, sel_error,
};
use crate::cpu::{
    Access, CR0_AM, CR0_CD, CR0_EM, CR0_ET, CR0_MP, CR0_NE, CR0_NW, CR0_PE, CR0_PG, CR0_TS, CR0_WP, CR4_OSFXSR,
    CR4_OSXMMEXCPT, CR4_PSE, CR4_TSD, Cpu, CpuFlags, CpuModel, CpuResult, CpuState, DescTable, Descriptor, Fault,
    Seg, SegCache,
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

/// Write CR0. ET is hardwired from the 486 on, which has the NE, WP, AM,
/// NW and CD bits a 386 lacks. Paging needs protected mode.
fn set_cr0(cpu: &mut Cpu, value: u32) -> CpuResult {
    let mut writable = CR0_PE | CR0_MP | CR0_EM | CR0_TS | CR0_PG;
    if cpu.model >= CpuModel::I486 {
        writable |= CR0_NE | CR0_WP | CR0_AM | CR0_NW | CR0_CD;
    } else {
        writable |= CR0_ET;
    }
    let mut value = (value & writable) | (cpu.cr0 & !writable);
    if cpu.model >= CpuModel::I486 {
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

/// Write CR4 (Pentium). This Pentium has its TSD and PSE bits, and a
/// Pentium III OSFXSR and OSXMMEXCPT too; the others, among them the
/// virtual-8086 mode extensions, debugging extensions and machine checks
/// CPUID doesn't report, are reserved: #GP(0).
fn set_cr4(cpu: &mut Cpu, value: u32) -> CpuResult {
    let sse = if cpu.model.has_sse() { CR4_OSFXSR | CR4_OSXMMEXCPT } else { 0 };
    if value & !(CR4_TSD | CR4_PSE | sse) != 0 {
        return Err(Fault::gp(0));
    }
    // The translations change with the page size.
    if (cpu.cr4 ^ value) & CR4_PSE != 0 {
        cpu.tlb.flush();
    }
    cpu.cr4 = value;
    Ok(())
}

/// MOV CRn/DRn/TRn, r32. CR4 is a Pentium's.
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
        Register::CR4 if cpu.model >= CpuModel::Pentium => set_cr4(cpu, value)?,
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
        Register::CR4 if cpu.model >= CpuModel::Pentium => cpu.cr4,
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

/// The features CPUID reports in EDX, as DOSBox-X's Pentium does: an FPU,
/// 4 MB pages, the time stamp counter, MSRs and CMPXCHG8B.
const CPUID_FEATURES: u32 = 0x0001 | 0x0008 | 0x0010 | 0x0020 | 0x0100;

/// The CPUID feature bit of MMX, which a Pentium MMX and a Pentium II add.
const CPUID_MMX: u32 = 0x0080_0000;

/// The CPUID feature bit of CMOVcc (and, with an FPU, FCMOVcc and FCOMI),
/// which a P6 adds.
const CPUID_CMOV: u32 = 0x0000_8000;

/// The CPUID feature bit of SYSENTER/SYSEXIT, which a Pentium II adds.
const CPUID_SEP: u32 = 0x0000_0800;

/// The CPUID feature bits a Pentium III adds: FXSAVE/FXRSTOR (FXSR) and
/// SSE, and the processor serial number (PSN) until it is switched off.
const CPUID_FXSR: u32 = 0x0100_0000;
const CPUID_SSE: u32 = 0x0200_0000;
const CPUID_PSN: u32 = 0x0004_0000;

/// What CPUID leaf 2 gives a Pentium III (Katmai) in EAX, EBX, ECX and
/// EDX: one pass (AL 1), and the descriptors of its instruction TLBs (01h,
/// 02h), data TLBs (03h, 04h), 16 KB L1 caches (08h, 0Ch) and 512 KB L2
/// cache (43h). Windows XP waits forever at boot without them on a
/// Pentium III, as DOSBox-X notes.
const CPUID_CACHES: [u32; 4] = [0x0302_0101, 0, 0, 0x0C04_0843];

/// The low 64 bits of the processor serial number CPUID leaf 3 gives in
/// ECX and EDX ("-DOS" and "RUST"); the top 32 are the signature.
const PSN: [u32; 2] = [0x534F_442D, 0x5453_5552];

/// The extended features (CPUID 80000001h, EDX) a K6-2 adds to those of
/// leaf 1, as AMD's processor recognition note (20734) gives them:
/// SYSCALL/SYSRET (bit 11) and 3DNow! (bit 31).
const CPUID_SYSCALL: u32 = 0x0000_0800;
const CPUID_3DNOW: u32 = 0x8000_0000;

/// The highest extended leaf of a K6-2.
const K6_HIGHEST_EXTENDED: u32 = 0x8000_0005;

/// The K6-2's name, which extended leaves 80000002h to 80000004h give 16
/// bytes at a time, padded with zeros, as AMD's note has it.
const K6_NAME: &[u8] = b"AMD-K6(tm) 3D processor";

/// What extended leaf 80000005h gives a K6-2 in EBX, ECX and EDX: its TLBs
/// (128 data entries, 2-way; 64 instruction entries, direct-mapped) and its
/// 32 KB data and instruction caches (2-way, 32-byte lines, 2 lines a
/// tag), from AMD's note.
const K6_L1: [u32; 3] = [0x0280_0140, 0x2002_0220, 0x2002_0220];

/// The features CPUID leaf 1 reports in EDX for `model`, as DOSBox-X has
/// them (cpu.cpp, `CPU_CPUID`), less the 36-bit page size extension it
/// gives its Pentium II, which this one doesn't have.
pub fn cpuid_features(model: CpuModel) -> u32 {
    let mut features = CPUID_FEATURES;
    if model.has_mmx() {
        features |= CPUID_MMX;
    }
    if model.is_p6() {
        features |= CPUID_CMOV;
    }
    if model >= CpuModel::PentiumII {
        features |= CPUID_SEP;
    }
    if model.has_sse() {
        features |= CPUID_FXSR | CPUID_SSE | CPUID_PSN;
    }
    features
}

/// CPUID (Pentium): leaf 0 gives the highest leaf and "GenuineIntel",
/// leaf 1 the family, model and stepping and the features. A Pentium III
/// has leaf 2, its caches and TLBs, and leaf 3, the low 64 bits of its
/// serial number; once that is switched off (MSR 119h) leaf 3 is gone,
/// and leaf 1 doesn't report it. A K6-2 says "AuthenticAMD" and has the
/// extended leaves `amd_extended` gives. Leaves past the highest give
/// zeros.
pub fn cpuid(cpu: &mut Cpu) -> CpuResult {
    require_pentium(cpu)?;
    let text = |s: &[u8; 4]| u32::from_le_bytes(*s);
    let sse = cpu.model.has_sse();
    let psn = sse && !cpu.sse.psn_off;
    let highest = if psn { 3 } else if sse { 2 } else { 1 };
    let leaf = cpu.eax();
    let (eax, ebx, ecx, edx) = match leaf {
        0 if cpu.model.is_amd() => (highest, text(b"Auth"), text(b"cAMD"), text(b"enti")),
        0 => (highest, text(b"Genu"), text(b"ntel"), text(b"ineI")),
        0x8000_0000.. if cpu.model.is_amd() => amd_extended(cpu.model, leaf),
        _ if leaf > highest => (0, 0, 0, 0),
        1 => {
            let features = cpuid_features(cpu.model) & if psn { !0 } else { !CPUID_PSN };
            (cpu.model.signature(), 0, 0, features)
        }
        2 => (CPUID_CACHES[0], CPUID_CACHES[1], CPUID_CACHES[2], CPUID_CACHES[3]),
        _ => (0, 0, PSN[0], PSN[1]),
    };
    cpu.set_eax(eax);
    cpu.set_ebx(ebx);
    cpu.set_ecx(ecx);
    cpu.set_edx(edx);
    Ok(())
}

/// The extended CPUID leaves of a K6-2, as AMD's processor recognition
/// note (20734, table 23) gives them for model 8: 80000000h the highest,
/// 80000001h the signature with the extended family (6) and the features
/// with SYSCALL and 3DNow!, 80000002h to 80000004h the name, and
/// 80000005h the L1 caches and TLBs. Those past the highest give zeros.
fn amd_extended(model: CpuModel, leaf: u32) -> (u32, u32, u32, u32) {
    let name = |at: usize| {
        let mut bytes = [0; 4];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = K6_NAME.get(at + i).copied().unwrap_or(0);
        }
        u32::from_le_bytes(bytes)
    };
    match leaf {
        0x8000_0000 => (K6_HIGHEST_EXTENDED, 0, 0, 0),
        0x8000_0001 => (
            model.signature() + 0x100,
            0,
            0,
            cpuid_features(model) | CPUID_SYSCALL | CPUID_3DNOW,
        ),
        0x8000_0002..=0x8000_0004 => {
            let at = (leaf - 0x8000_0002) as usize * 16;
            (name(at), name(at + 4), name(at + 8), name(at + 12))
        }
        0x8000_0005 => (0, K6_L1[0], K6_L1[1], K6_L1[2]),
        _ => (0, 0, 0, 0),
    }
}

/// RDTSC (Pentium): the time stamp counter into EDX:EAX. CR4.TSD keeps it
/// to level 0.
pub fn rdtsc(cpu: &mut Cpu) -> CpuResult {
    // The time: a loop that reads it isn't the same pass again (`idle`).
    cpu.bus.observe.time_read();
    require_pentium(cpu)?;
    if cpu.cr4 & CR4_TSD != 0 {
        require_cpl0(cpu)?;
    }
    let tsc = cpu.tsc();
    cpu.set_eax(tsc as u32);
    cpu.set_edx((tsc >> 32) as u32);
    Ok(())
}

/// A hinting NOP (P6): 0F 19 to 0F 1E with a ModRM byte, and 0F 18 (the
/// Pentium III's prefetches), which a Pentium Pro or Pentium II runs as a
/// NOP, as DOSBox-X does (prefix_0f.h). Its memory operand isn't accessed.
pub fn hint_nop(cpu: &Cpu) -> CpuResult {
    if cpu.model.is_p6() { Ok(()) } else { Err(Fault::UD) }
}

/// What Glide puts in EBX for its model-specific register accesses.
const GLIDE_MSR_MARK: u32 = 0xDEAD_CAFE;

/// What a P6 reads from its microcode revision MSR (8Bh): DOSBox-X's
/// value for its Pentium Pro and Pentium II, high enough that Windows 98
/// and ME don't try to update the microcode.
const MICROCODE_REVISION: u64 = 0x003F_003F_001E_03FF;

/// What a Pentium III reads from its microcode revision MSR (8Bh) and its
/// platform ID (17h): DOSBox-X's, from a Pentium III. Windows ME updates
/// the microcode of a lower revision, and stops at a blue screen without
/// the platform ID.
const P3_MICROCODE_REVISION: u64 = 0x0000_000E_0000_0000;
const P3_PLATFORM_ID: u64 = 0x1100_0000_0000_0000;

/// The bit of the L2 cache control MSR (119h) that switches the processor
/// serial number off.
const BBL_CR_CTL_PSN_OFF: u64 = 0x0020_0000;

/// The K6-2's MSRs (data sheet 21850, table 5): test register 12, EFER,
/// STAR and WHCR.
const K6_TR12: u32 = 0x0E;
const K6_EFER: u32 = 0xC000_0080;
const K6_STAR: u32 = 0xC000_0081;
const K6_WHCR: u32 = 0xC000_0082;

/// The bits of the K6-2's MSRs that hold anything: of the machine check
/// type its 5, of test register 12 its cache inhibit bit (3), of EFER its
/// system call extension (SCE, bit 0), which enables SYSCALL and SYSRET,
/// and of WHCR its write allocate fields and the bit after them (0 to 8).
/// The others read 0; a 1 written to one of EFER's raises #GP(0).
const K6_MCTR_BITS: u64 = 0x1F;
const K6_TR12_BITS: u64 = 0x08;
const EFER_SCE: u64 = 0x01;
const K6_WHCR_BITS: u64 = 0x1FF;

/// Whether `model` has the model-specific register `msr` for a read or a
/// write: a Pentium the machine check address and type (0, 1), the time
/// stamp counter (10h) and its performance monitoring control and counters
/// (11h to 13h). A P6 has the first three, and the local APIC base (1Bh)
/// and the microcode revision (8Bh), which Windows 98 and ME use without
/// asking CPUID. A Pentium II adds those DOSBox-X gives it (cpu.cpp,
/// `CPU_RDMSR` and `CPU_WRMSR`): the power-on configuration (2Ah), the L2
/// cache control registers (119h, 11Eh), the SYSENTER registers (174h to
/// 176h), and the microcode update trigger (79h), which can only be
/// written. A Pentium III adds DOSBox-X's platform ID (17h), which can only
/// be read, the event selects of its performance counters (186h, 187h),
/// and the miscellaneous enables (140h), which Linux reads on one. A K6-2
/// has the first three, test register 12 (0Eh), and EFER, STAR and WHCR
/// (C0000080h to C0000082h), and no performance counters.
fn has_msr(model: CpuModel, msr: u32, write: bool) -> bool {
    let p6 = model.is_p6();
    let k6 = model.is_amd();
    let pentium_ii = model >= CpuModel::PentiumII;
    let pentium_iii = model.has_sse();
    match msr {
        0x00 | 0x01 | 0x10 => true,
        K6_TR12 | K6_EFER | K6_STAR | K6_WHCR => k6,
        0x11..=0x13 => !p6 && !k6,
        0x1B | 0x8B => p6,
        0x2A | 0x119 | 0x11E | 0x174..=0x176 => pentium_ii,
        0x79 => pentium_ii && write,
        0x17 => pentium_iii && !write,
        0x140 | 0x186 | 0x187 => pentium_iii,
        _ => false,
    }
}

/// RDMSR and WRMSR (Pentium): the model-specific register ECX names into
/// or from EDX:EAX, at level 0 (see `has_msr`). The machine check
/// registers read 0, as nothing checks the machine; the performance
/// counters hold what was written to them and don't count. There is no
/// local APIC, so its base reads 0 and a write to it changes nothing, as
/// in DOSBox-X; the Pentium II's cache and configuration registers read 0
/// and keep nothing, and a microcode update does nothing. A Pentium III's
/// L2 cache control (119h) keeps the bit that switches its serial number
/// off, which stays off until a reset; its event selects and
/// miscellaneous enables read 0 and keep nothing. A K6-2's MSRs hold what
/// was written to their bits (`K6Msrs`), the machine check ones too, as
/// nothing but WRMSR sets them there. Any other MSR
/// raises #GP(0), except with EBX DEADCAFEh: 3dfx's Glide marks its
/// accesses to the memory type range registers so (a Pentium II's, for
/// write-combining), for its Windows driver to catch the fault where
/// there are none, and its DOS overlay (GLIDE2X.OVL) relies on that; they
/// read 0 and write nothing.
pub fn msr(cpu: &mut Cpu, write: bool) -> CpuResult {
    require_pentium(cpu)?;
    require_cpl0(cpu)?;
    let msr = cpu.ecx();
    if !has_msr(cpu.model, msr, write) {
        if cpu.ebx() == GLIDE_MSR_MARK {
            if !write {
                cpu.set_eax(0);
                cpu.set_edx(0);
            }
            return Ok(());
        }
        return Err(Fault::gp(0));
    }
    if write {
        let value = (cpu.edx() as u64) << 32 | cpu.eax() as u64;
        if cpu.model.is_amd() {
            match msr {
                0x00 => cpu.k6.mcar = value,
                0x01 => cpu.k6.mctr = value & K6_MCTR_BITS,
                K6_TR12 => cpu.k6.tr12 = value & K6_TR12_BITS,
                K6_EFER if value & !EFER_SCE != 0 => return Err(Fault::gp(0)),
                K6_EFER => cpu.k6.efer = value,
                K6_STAR => cpu.k6.star = value,
                K6_WHCR => cpu.k6.whcr = value & K6_WHCR_BITS,
                _ => {}
            }
        }
        match msr {
            0x10 => cpu.set_tsc(value),
            0x11..=0x13 => cpu.perf_msrs[msr as usize - 0x11] = value,
            // The code selector keeps 16 bits.
            0x174 => cpu.sysenter[0] = cpu.eax() & 0xFFFF,
            0x175..=0x176 => cpu.sysenter[msr as usize - 0x174] = cpu.eax(),
            0x119 if cpu.model.has_sse() && value & BBL_CR_CTL_PSN_OFF != 0 => cpu.sse.psn_off = true,
            _ => {}
        }
    } else {
        let k6 = cpu.model.is_amd();
        let value = match msr {
            0x00 if k6 => cpu.k6.mcar,
            0x01 if k6 => cpu.k6.mctr,
            K6_TR12 => cpu.k6.tr12,
            K6_EFER => cpu.k6.efer,
            K6_STAR => cpu.k6.star,
            K6_WHCR => cpu.k6.whcr,
            0x10 => cpu.tsc(),
            0x11..=0x13 => cpu.perf_msrs[msr as usize - 0x11],
            0x8B if cpu.model.has_sse() => P3_MICROCODE_REVISION,
            0x8B => MICROCODE_REVISION,
            0x17 => P3_PLATFORM_ID,
            0x119 if cpu.sse.psn_off => BBL_CR_CTL_PSN_OFF,
            0x174..=0x176 => cpu.sysenter[msr as usize - 0x174] as u64,
            _ => 0,
        };
        cpu.set_eax(value as u32);
        cpu.set_edx((value >> 32) as u32);
    }
    Ok(())
}

/// The access rights and flags of the flat 4 GB segments SYSENTER and
/// SYSEXIT load, whatever the descriptor tables hold: a readable code
/// segment and a writable data segment, accessed, 32-bit, with 4 KB
/// granularity, at privilege level 0 or (with `0x60`) 3.
const SEP_CODE: u16 = 0xC09B;
const SEP_STACK: u16 = 0xC093;
const SEP_DPL3: u16 = 0x0060;

/// SYSENTER (Pentium II): a fast call to level 0, to the code selector
/// in MSR 174h and the stack selector after it, both flat, at the ESP and
/// EIP in MSRs 175h and 176h, with VM, IF and RF cleared. #GP(0) in real
/// mode or with a null code selector in the MSR.
pub fn sysenter(cpu: &mut Cpu) -> CpuResult {
    if cpu.model < CpuModel::PentiumII {
        return Err(Fault::UD);
    }
    let cs = cpu.sysenter[0] as u16 & 0xFFFC;
    if !cpu.pe() || cs == 0 {
        return Err(Fault::gp(0));
    }
    if crate::profile::any_active() && cpu.cpl != 0 {
        let code32 = cpu.seg_cache(Seg::CS).attr & crate::cpu::ATTR_DB != 0;
        let mode = crate::profile::Mode::of(true, cpu.v86(), code32);
        crate::profile::note_entry(mode, cpu.cs(), cpu.eip(), crate::profile::Cause::Gate, 1);
    }
    cpu.set_cpu_flag(CpuFlags::VM, false);
    cpu.set_cpu_flag(CpuFlags::IF, false);
    cpu.set_cpu_flag(CpuFlags::RF, false);
    cpu.set_seg_cache(Seg::CS, SegCache::from_descriptor(cs, 0, 0xFFFF_FFFF, SEP_CODE));
    cpu.set_seg_cache(Seg::SS, SegCache::from_descriptor(cs.wrapping_add(8), 0, 0xFFFF_FFFF, SEP_STACK));
    cpu.cpl = 0;
    cpu.set_esp(cpu.sysenter[1]);
    cpu.set_eip(cpu.sysenter[2]);
    Ok(())
}

/// SYSEXIT (Pentium II): a fast return from level 0 to level 3, to the
/// flat code and stack selectors 16 and 24 past the one in MSR 174h, at
/// EIP = EDX and ESP = ECX. #GP(0) outside protected mode, at another
/// level, or with a null code selector in the MSR.
pub fn sysexit(cpu: &mut Cpu) -> CpuResult {
    if cpu.model < CpuModel::PentiumII {
        return Err(Fault::UD);
    }
    let cs = cpu.sysenter[0] as u16 & 0xFFFC;
    if !cpu.pm() || cpu.cpl != 0 || cs == 0 {
        return Err(Fault::gp(0));
    }
    let code = SegCache::from_descriptor(cs.wrapping_add(16) | 3, 0, 0xFFFF_FFFF, SEP_CODE | SEP_DPL3);
    let stack = SegCache::from_descriptor(cs.wrapping_add(24) | 3, 0, 0xFFFF_FFFF, SEP_STACK | SEP_DPL3);
    cpu.set_seg_cache(Seg::CS, code);
    cpu.set_seg_cache(Seg::SS, stack);
    cpu.cpl = 3;
    cpu.set_esp(cpu.ecx());
    cpu.set_eip(cpu.edx());
    Ok(())
}

/// SYSCALL (K6-2): a fast call to level 0, as AMD's application note
/// (21086) has it: ECX gets the return EIP, EIP the low dword of STAR, CS
/// the selector in STAR's bits 32 to 47 with RPL 0 and SS the one after
/// it, both flat and 32-bit whatever the descriptor tables hold, and VM, IF
/// and RF are cleared. Nothing is checked: it runs in real mode and at any
/// level. #UD with EFER.SCE clear.
pub fn syscall(cpu: &mut Cpu) -> CpuResult {
    require_syscall(cpu)?;
    let cs = (cpu.k6.star >> 32) as u16 & 0xFFFC;
    if crate::profile::any_active() && cpu.cpl != 0 {
        let code32 = cpu.seg_cache(Seg::CS).attr & crate::cpu::ATTR_DB != 0;
        let mode = crate::profile::Mode::of(cpu.pe(), cpu.v86(), code32);
        crate::profile::note_entry(mode, cpu.cs(), cpu.eip(), crate::profile::Cause::Gate, 2);
    }
    cpu.set_ecx(cpu.eip());
    cpu.set_cpu_flag(CpuFlags::VM, false);
    cpu.set_cpu_flag(CpuFlags::IF, false);
    cpu.set_cpu_flag(CpuFlags::RF, false);
    cpu.set_seg_cache(Seg::CS, SegCache::from_descriptor(cs, 0, 0xFFFF_FFFF, SEP_CODE));
    cpu.set_seg_cache(Seg::SS, SegCache::from_descriptor(cs.wrapping_add(8), 0, 0xFFFF_FFFF, SEP_STACK));
    cpu.cpl = 0;
    cpu.set_eip(cpu.k6.star as u32);
    Ok(())
}

/// SYSRET (K6-2): the return from SYSCALL to level 3, as AMD's note has
/// it: EIP gets ECX, CS the selector in STAR's bits 48 to 63 with RPL 3,
/// flat and 32-bit, and SS the selector after it with RPL 3, keeping the
/// rest of the stack segment SYSCALL loaded; IF is set. The note puts CPL
/// at 3 whatever STAR's bits 48 and 49 hold, and those are CS's RPL bits.
/// #UD with EFER.SCE clear, #GP(0) at another level than 0. The note's
/// only #GP condition is the level, so SYSRET runs in real mode too (the
/// later AMD64 manual adds a #GP there; the K6 note does not).
pub fn sysret(cpu: &mut Cpu) -> CpuResult {
    require_syscall(cpu)?;
    require_cpl0(cpu)?;
    let cs = (cpu.k6.star >> 48) as u16;
    cpu.set_seg_cache(Seg::CS, SegCache::from_descriptor(cs | 3, 0, 0xFFFF_FFFF, SEP_CODE | SEP_DPL3));
    let mut stack = *cpu.seg_cache(Seg::SS);
    stack.selector = cs.wrapping_add(8) | 3;
    cpu.set_seg_cache(Seg::SS, stack);
    cpu.cpl = 3;
    cpu.set_cpu_flag(CpuFlags::IF, true);
    cpu.set_eip(cpu.ecx());
    Ok(())
}

/// #UD unless the CPU has SYSCALL and SYSRET (a K6-2) and EFER enables
/// them.
fn require_syscall(cpu: &Cpu) -> CpuResult {
    if cpu.model.has_3dnow() && cpu.k6.efer & EFER_SCE != 0 { Ok(()) } else { Err(Fault::UD) }
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
            // With the limit's bits 16-19, as the processors (and DOSBox)
            // give them: Windows 95's KRNL386 sets a huge block's first
            // selector's AVL bit by writing back what LAR read, limit and all.
            ((desc.0 >> 32) as u32) & if dest.size() == 4 { 0x00FF_FF00 } else { 0xFF00 }
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
