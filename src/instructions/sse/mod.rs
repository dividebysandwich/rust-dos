//! SSE (Pentium III): eight 128-bit registers XMM0-XMM7, each four
//! singles, the instructions on them and MXCSR, which rounds and masks
//! their exceptions (`float.rs`); the integer instructions SSE adds on MMX
//! registers (in `mmx.rs`); and FXSAVE and FXRSTOR, which save and load
//! the FPU, MMX and SSE state together.
//!
//! An instruction on XMM registers raises #UD with CR0.EM set or with
//! CR4.OSFXSR clear, which says the system doesn't save them, and #NM with
//! CR0.TS set. A 128-bit memory operand must be aligned to 16 bytes but
//! for MOVUPS's, or #GP(0). An exception the elements of an instruction
//! raise that MXCSR doesn't mask leaves the destination as it was and
//! raises #XM, or #UD with CR4.OSXMMEXCPT clear.

pub mod float;

use iced_x86::{CpuidFeature, Instruction, Mnemonic, OpKind, Register};

use super::fpu::control as status;
use super::mmx;
use super::operand::{effective_offset, mem_seg};
use crate::cpu::{
    Access, CR0_EM, CR0_TS, CR4_OSFXSR, CR4_OSXMMEXCPT, Cpu, CpuFlags, CpuResult, FPU_TAG_EMPTY,
    FPU_TAG_VALID, Fault,
};
use crate::f80::{F80, FPU_80_BITS_MAX};
use float::Env;

/// An SSE instruction: the SSE forms of the mnemonics (not the SSE2 ones,
/// which share some of them).
pub fn sse(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    if !cpu.model.has_sse() || instr.cpuid_features() != [CpuidFeature::SSE] {
        return Err(Fault::UD);
    }
    use Mnemonic::*;
    let mnemonic = instr.mnemonic();
    match mnemonic {
        Pavgb | Pavgw | Pextrw | Pinsrw | Pmaxsw | Pmaxub | Pminsw | Pminub | Pmovmskb
        | Pmulhuw | Psadbw | Pshufw | Maskmovq | Movntq => return mmx::sse_integer(cpu, instr),
        // Stores are done in order already.
        Sfence => return Ok(()),
        _ => {}
    }
    if cpu.cr0 & CR0_EM != 0 || cpu.cr4 & CR4_OSFXSR == 0 {
        return Err(Fault::UD);
    }
    if cpu.cr0 & CR0_TS != 0 {
        return Err(Fault::NM);
    }
    match mnemonic {
        Movaps | Movups | Movntps => mov_packed(cpu, instr, mnemonic != Movups),
        Movss => movss(cpu, instr),
        Movlps | Movhps => mov_half(cpu, instr, mnemonic == Movhps),
        Movhlps => {
            let (a, b) = (
                xmm(cpu, instr.op0_register()),
                xmm(cpu, instr.op1_register()),
            );
            set_xmm(cpu, instr.op0_register(), (a & !LOW) | b >> 64);
            Ok(())
        }
        Movlhps => {
            let (a, b) = (
                xmm(cpu, instr.op0_register()),
                xmm(cpu, instr.op1_register()),
            );
            set_xmm(cpu, instr.op0_register(), (a & LOW) | b << 64);
            Ok(())
        }
        Movmskps => {
            let v = xmm(cpu, instr.op1_register());
            let mask = (0..4).fold(0, |m, i| m | (lane(v, i) >> 31) << i);
            cpu.set_reg(instr.op0_register(), mask);
            Ok(())
        }
        Andps => bitwise(cpu, instr, |a, b| a & b),
        Andnps => bitwise(cpu, instr, |a, b| !a & b),
        Orps => bitwise(cpu, instr, |a, b| a | b),
        Xorps => bitwise(cpu, instr, |a, b| a ^ b),
        Unpcklps => bitwise(cpu, instr, |a, b| {
            lanes([lane(a, 0), lane(b, 0), lane(a, 1), lane(b, 1)])
        }),
        Unpckhps => bitwise(cpu, instr, |a, b| {
            lanes([lane(a, 2), lane(b, 2), lane(a, 3), lane(b, 3)])
        }),
        Shufps => {
            let n = instr.immediate8() as u32;
            let pick = move |v: u128, i: u32| lane(v, (n >> (2 * i) & 3) as usize);
            bitwise(cpu, instr, move |a, b| {
                lanes([pick(a, 0), pick(a, 1), pick(b, 2), pick(b, 3)])
            })
        }
        Addps | Addss => arith(cpu, instr, mnemonic == Addss, Env::add),
        Subps | Subss => arith(cpu, instr, mnemonic == Subss, Env::sub),
        Mulps | Mulss => arith(cpu, instr, mnemonic == Mulss, Env::mul),
        Divps | Divss => arith(cpu, instr, mnemonic == Divss, Env::div),
        Minps | Minss => arith(cpu, instr, mnemonic == Minss, |e, a, b| {
            e.min_max(a, b, false)
        }),
        Maxps | Maxss => arith(cpu, instr, mnemonic == Maxss, |e, a, b| {
            e.min_max(a, b, true)
        }),
        Sqrtps | Sqrtss => arith(cpu, instr, mnemonic == Sqrtss, |e, _, b| e.sqrt(b)),
        Rcpps | Rcpss => arith(cpu, instr, mnemonic == Rcpss, |_, _, b| {
            float::reciprocal(b)
        }),
        Rsqrtps | Rsqrtss => arith(cpu, instr, mnemonic == Rsqrtss, |_, _, b| {
            float::reciprocal_sqrt(b)
        }),
        Cmpps | Cmpss => {
            let p = instr.immediate8();
            arith(cpu, instr, mnemonic == Cmpss, move |e, a, b| {
                if e.compare(a, b, p) { !0 } else { 0 }
            })
        }
        Comiss | Ucomiss => comiss(cpu, instr, mnemonic == Comiss),
        Cvtpi2ps | Cvtsi2ss => from_int(cpu, instr),
        Cvtps2pi | Cvttps2pi | Cvtss2si | Cvttss2si => {
            to_int(cpu, instr, matches!(mnemonic, Cvttps2pi | Cvttss2si))
        }
        Ldmxcsr => {
            let lin = span(cpu, instr, 4, Access::Read, false)?;
            let value = cpu.lin_read_32(lin as usize);
            if value & !float::MXCSR_MASK != 0 {
                return Err(Fault::gp(0));
            }
            cpu.sse.mxcsr = value;
            Ok(())
        }
        Stmxcsr => {
            let lin = span(cpu, instr, 4, Access::Write, false)?;
            cpu.lin_write_32(lin as usize, cpu.sse.mxcsr);
            Ok(())
        }
        _ => Err(Fault::UD),
    }
}

/// The low quadword of an XMM register.
const LOW: u128 = u64::MAX as u128;

fn xmm(cpu: &Cpu, reg: Register) -> u128 {
    cpu.sse.xmm[reg as usize - Register::XMM0 as usize]
}

fn set_xmm(cpu: &mut Cpu, reg: Register, value: u128) {
    cpu.sse.xmm[reg as usize - Register::XMM0 as usize] = value;
}

/// Single `i` (0 to 3) of `v`.
fn lane(v: u128, i: usize) -> u32 {
    (v >> (32 * i)) as u32
}

fn lanes(l: [u32; 4]) -> u128 {
    l.iter()
        .enumerate()
        .fold(0, |v, (i, &x)| v | (x as u128) << (32 * i))
}

/// Check `len` bytes of the memory operand for `access`, 16-byte aligned
/// if `aligned` (or #GP(0)), and give their linear address.
fn span(
    cpu: &mut Cpu,
    instr: &Instruction,
    len: u32,
    access: Access,
    aligned: bool,
) -> CpuResult<u32> {
    let seg = mem_seg(instr);
    let off = effective_offset(cpu, instr);
    if aligned && cpu.seg_cache(seg).base.wrapping_add(off) & 15 != 0 {
        return Err(Fault::gp(0));
    }
    cpu.check_span(seg, off, len, access)
}

fn read_mem(cpu: &mut Cpu, lin: u32, len: u32) -> u128 {
    (0..len / 4).fold(0, |v, i| {
        v | (cpu.lin_read_32(lin.wrapping_add(4 * i) as usize) as u128) << (32 * i)
    })
}

fn write_mem(cpu: &mut Cpu, lin: u32, len: u32, value: u128) {
    for i in 0..len / 4 {
        cpu.lin_write_32(lin.wrapping_add(4 * i) as usize, (value >> (32 * i)) as u32);
    }
}

/// Operand `i` of `len` bytes: an XMM register (all of it), MMn, a 32-bit
/// register, or memory (zero-extended), 16-byte aligned if `aligned`.
fn source(cpu: &mut Cpu, instr: &Instruction, i: u32, len: u32, aligned: bool) -> CpuResult<u128> {
    match instr.op_kind(i) {
        OpKind::Register => {
            let reg = instr.op_register(i);
            Ok(if reg.is_xmm() {
                xmm(cpu, reg)
            } else if mmx::is_mm(reg) {
                mmx::mm(cpu, reg) as u128
            } else {
                cpu.reg(reg) as u128
            })
        }
        OpKind::Memory => {
            let lin = span(cpu, instr, len, Access::Read, aligned)?;
            Ok(read_mem(cpu, lin, len))
        }
        _ => Err(Fault::UD),
    }
}

/// Store `value` in the memory operand, `len` bytes of it.
fn store(cpu: &mut Cpu, instr: &Instruction, len: u32, value: u128, aligned: bool) -> CpuResult {
    let lin = span(cpu, instr, len, Access::Write, aligned)?;
    write_mem(cpu, lin, len, value);
    Ok(())
}

/// MOVAPS, MOVUPS and MOVNTPS: 128 bits between XMM registers or from or
/// to memory, which MOVAPS's and MOVNTPS's must be `aligned`.
fn mov_packed(cpu: &mut Cpu, instr: &Instruction, aligned: bool) -> CpuResult {
    if instr.op0_kind() == OpKind::Memory {
        let value = xmm(cpu, instr.op1_register());
        return store(cpu, instr, 16, value, aligned);
    }
    let value = source(cpu, instr, 1, 16, aligned)?;
    set_xmm(cpu, instr.op0_register(), value);
    Ok(())
}

/// MOVSS: the low single between XMM registers (the others stay), from
/// memory (the others cleared), or to memory.
fn movss(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    match (instr.op0_kind(), instr.op1_kind()) {
        (OpKind::Memory, _) => {
            let value = xmm(cpu, instr.op1_register());
            store(cpu, instr, 4, value, false)
        }
        (_, OpKind::Memory) => {
            let value = source(cpu, instr, 1, 4, false)?;
            set_xmm(cpu, instr.op0_register(), value);
            Ok(())
        }
        _ => {
            let (a, b) = (
                xmm(cpu, instr.op0_register()),
                xmm(cpu, instr.op1_register()),
            );
            set_xmm(
                cpu,
                instr.op0_register(),
                (a & !0xFFFF_FFFF) | (b & 0xFFFF_FFFF),
            );
            Ok(())
        }
    }
}

/// MOVLPS and MOVHPS: the low or `high` quadword from or to memory; the
/// other stays.
fn mov_half(cpu: &mut Cpu, instr: &Instruction, high: bool) -> CpuResult {
    let shift = if high { 64 } else { 0 };
    if instr.op0_kind() == OpKind::Memory {
        let value = xmm(cpu, instr.op1_register()) >> shift;
        return store(cpu, instr, 8, value, false);
    }
    let value = source(cpu, instr, 1, 8, false)?;
    let reg = instr.op0_register();
    set_xmm(cpu, reg, (xmm(cpu, reg) & !(LOW << shift)) | value << shift);
    Ok(())
}

/// An instruction on the 128 bits of its destination and source that
/// raises no exception: the logical ones, UNPCKLPS, UNPCKHPS and SHUFPS.
fn bitwise(cpu: &mut Cpu, instr: &Instruction, f: impl Fn(u128, u128) -> u128) -> CpuResult {
    let b = source(cpu, instr, 1, 16, true)?;
    let reg = instr.op0_register();
    set_xmm(cpu, reg, f(xmm(cpu, reg), b));
    Ok(())
}

/// Put the exceptions `env` found into MXCSR, and raise #XM (#UD with
/// CR4.OSXMMEXCPT clear) for one it doesn't mask.
fn commit(cpu: &mut Cpu, env: &Env) -> CpuResult {
    if env.finish(&mut cpu.sse.mxcsr) {
        Ok(())
    } else if cpu.cr4 & CR4_OSXMMEXCPT != 0 {
        Err(Fault::XM)
    } else {
        Err(Fault::UD)
    }
}

/// `f` of each single of the destination and the source, or of their low
/// ones only (`scalar`, the destination's others staying).
fn arith(
    cpu: &mut Cpu,
    instr: &Instruction,
    scalar: bool,
    f: impl Fn(&mut Env, u32, u32) -> u32,
) -> CpuResult {
    let b = if scalar {
        source(cpu, instr, 1, 4, false)?
    } else {
        source(cpu, instr, 1, 16, true)?
    };
    let reg = instr.op0_register();
    let a = xmm(cpu, reg);
    let mut env = Env::new(cpu.sse.mxcsr);
    let mut result = [lane(a, 0), lane(a, 1), lane(a, 2), lane(a, 3)];
    for (i, r) in result
        .iter_mut()
        .enumerate()
        .take(if scalar { 1 } else { 4 })
    {
        *r = f(&mut env, lane(a, i), lane(b, i));
    }
    commit(cpu, &env)?;
    set_xmm(cpu, reg, lanes(result));
    Ok(())
}

/// COMISS and UCOMISS: ZF, PF and CF as the low singles compare, OF, SF
/// and AF cleared.
fn comiss(cpu: &mut Cpu, instr: &Instruction, signaling: bool) -> CpuResult {
    let b = source(cpu, instr, 1, 4, false)? as u32;
    let a = xmm(cpu, instr.op0_register()) as u32;
    let mut env = Env::new(cpu.sse.mxcsr);
    let (zf, pf, cf) = env.compare_flags(a, b, signaling);
    commit(cpu, &env)?;
    cpu.set_cpu_flag(CpuFlags::ZF, zf);
    cpu.set_cpu_flag(CpuFlags::PF, pf);
    cpu.set_cpu_flag(CpuFlags::CF, cf);
    cpu.set_cpu_flag(CpuFlags::OF | CpuFlags::SF | CpuFlags::AF, false);
    Ok(())
}

/// CVTPI2PS (two doublewords of MMn or memory into the low two singles)
/// and CVTSI2SS (one of a register or memory into the low single). The
/// destination's other singles stay. From MMn, it is an MMX instruction
/// to the FPU.
fn from_int(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let count = if instr.mnemonic() == Mnemonic::Cvtpi2ps {
        2
    } else {
        1
    };
    let b = source(cpu, instr, 1, 4 * count as u32, false)?;
    let reg = instr.op0_register();
    let a = xmm(cpu, reg);
    let mut env = Env::new(cpu.sse.mxcsr);
    let mut result = [lane(a, 0), lane(a, 1), lane(a, 2), lane(a, 3)];
    for (i, r) in result.iter_mut().enumerate().take(count) {
        *r = env.from_int(lane(b, i) as i32);
    }
    // A fault leaves the FPU as it was.
    commit(cpu, &env)?;
    if instr.op1_kind() == OpKind::Register && mmx::is_mm(instr.op1_register()) {
        mmx::enter_mmx(cpu);
    }
    set_xmm(cpu, reg, lanes(result));
    Ok(())
}

/// CVTPS2PI and CVTTPS2PI (the low two singles into MMn, an MMX
/// instruction to the FPU) and CVTSS2SI and CVTTSS2SI (the low one into a
/// 32-bit register), rounded as MXCSR says or truncated.
fn to_int(cpu: &mut Cpu, instr: &Instruction, truncate: bool) -> CpuResult {
    let dst = instr.op0_register();
    let to_mm = mmx::is_mm(dst);
    let count = if to_mm { 2 } else { 1 };
    let b = source(cpu, instr, 1, 4 * count as u32, false)?;
    let mut env = Env::new(cpu.sse.mxcsr);
    let mut result = [0u32; 2];
    for (i, r) in result.iter_mut().enumerate().take(count) {
        *r = env.to_int(lane(b, i), truncate);
    }
    // A fault leaves the FPU as it was.
    commit(cpu, &env)?;
    if to_mm {
        mmx::enter_mmx(cpu);
        mmx::set_mm(cpu, dst, result[0] as u64 | (result[1] as u64) << 32);
    } else {
        cpu.set_reg(dst, result[0]);
    }
    Ok(())
}

/// Where FXSAVE's image has MXCSR, the FPU's registers and the XMM
/// registers.
const FX_MXCSR: u32 = 24;
const FX_ST: u32 = 32;
const FX_XMM: u32 = 160;

/// FXSAVE and FXRSTOR (Pentium III): save the FPU, MMX and SSE state in,
/// or `restore` it from, 512 bytes of memory, 16-byte aligned (or #GP(0)).
/// They raise #NM with CR0.EM or CR0.TS set, whatever CR4 says.
///
/// The image holds the control and status words, the tag word abridged to
/// one bit per physical register (set unless empty), the instruction and
/// operand pointers and the opcode (which aren't tracked, so 0), MXCSR,
/// a mask field of 0 (the Pentium III's MXCSR_MASK), ST(0) to ST(7) in 16
/// bytes each, and XMM0 to XMM7; FXSAVE leaves the rest of the 512 bytes
/// alone. FXRSTOR raises #GP(0) for an MXCSR with reserved bits set,
/// before it loads anything.
pub fn fxsave(cpu: &mut Cpu, instr: &Instruction, restore: bool) -> CpuResult {
    if !cpu.model.has_sse() || instr.cpuid_features() != [CpuidFeature::FXSR] {
        return Err(Fault::UD);
    }
    if cpu.cr0 & (CR0_EM | CR0_TS) != 0 {
        return Err(Fault::NM);
    }
    let lin = span(
        cpu,
        instr,
        512,
        if restore { Access::Read } else { Access::Write },
        true,
    )?;
    // ST(i) and XMMi take 16 bytes each.
    let slot = |base: u32, i: usize| lin.wrapping_add(base + 16 * i as u32);
    if restore {
        let mxcsr = cpu.lin_read_32(lin.wrapping_add(FX_MXCSR) as usize);
        if mxcsr & !float::MXCSR_MASK != 0 {
            return Err(Fault::gp(0));
        }
        cpu.fpu_control = cpu.lin_read_16(lin as usize);
        let sw = cpu.lin_read_16(lin as usize + 2);
        status::set_status_word(cpu, sw);
        let tags = cpu.lin_read_8(lin as usize + 4);
        for i in 0..8 {
            cpu.fpu_tags[i] = if tags >> i & 1 != 0 {
                FPU_TAG_VALID
            } else {
                FPU_TAG_EMPTY
            };
        }
        for i in 0..8 {
            let st = read_mem(cpu, slot(FX_ST, i), 16) & FPU_80_BITS_MAX;
            let phys = cpu.fpu_get_phys_index(i);
            cpu.fpu_set_reg(phys, F80 { st });
        }
        cpu.sse.mxcsr = mxcsr;
        for i in 0..8 {
            cpu.sse.xmm[i] = read_mem(cpu, slot(FX_XMM, i), 16);
        }
    } else {
        let sw = status::status_word(cpu);
        let tags = (0..8).fold(0u32, |t, i| {
            t | ((cpu.fpu_tags[i] != FPU_TAG_EMPTY) as u32) << i
        });
        let header = [
            cpu.fpu_control as u32 | (sw as u32) << 16,
            tags,
            0,
            0,
            0,
            0,
            cpu.sse.mxcsr,
            0,
        ];
        for (i, &dword) in header.iter().enumerate() {
            cpu.lin_write_32(lin as usize + 4 * i, dword);
        }
        for i in 0..8 {
            // The 80 bits of the register, then 6 bytes of 0.
            let st = cpu.fpu_reg(cpu.fpu_get_phys_index(i)).st & FPU_80_BITS_MAX;
            write_mem(cpu, slot(FX_ST, i), 16, st);
        }
        for i in 0..8 {
            let value = cpu.sse.xmm[i];
            write_mem(cpu, slot(FX_XMM, i), 16, value);
        }
    }
    Ok(())
}
