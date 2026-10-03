//! MMX (Pentium MMX): eight 64-bit registers MM0-MM7 that are the
//! significands of the FPU's physical registers R0-R7, and the packed
//! byte, word, dword and quadword integer instructions on them.
//!
//! Every MMX instruction but EMMS sets the FPU's top of stack to 0 and
//! tags all its registers valid; one that writes MMn sets that register's
//! sign and exponent to all ones, so the FPU sees a NaN there. EMMS tags
//! all registers empty again. With CR0.EM set they raise #UD, with CR0.TS
//! #NM, as a task's FPU state is the MMX state too.

use iced_x86::{CpuidFeature, Instruction, Mnemonic, OpKind, Register};

use super::operand::{effective_offset, mem_operand, mem_operand_at, mem_seg};
use crate::cpu::{Access, CR0_EM, CR0_TS, Cpu, CpuModel, CpuResult, FPU_TAG_EMPTY, FPU_TAG_VALID, Fault};

/// What a write to MMn puts in bits 64 to 79 of the FPU register.
const MMX_EXPONENT: u128 = 0xFFFF << 64;

/// An MMX instruction: the MMX forms of the mnemonics (not the SSE2 ones
/// on XMM registers, nor the SSE additions on MMX registers).
pub fn mmx(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    if cpu.model < CpuModel::PentiumMmx || instr.cpuid_features() != [CpuidFeature::MMX] {
        return Err(Fault::UD);
    }
    if cpu.cr0 & CR0_EM != 0 {
        return Err(Fault::UD);
    }
    if cpu.cr0 & CR0_TS != 0 {
        return Err(Fault::NM);
    }
    use Mnemonic::*;
    let mnemonic = instr.mnemonic();
    if mnemonic == Emms {
        cpu.fpu_tags = [FPU_TAG_EMPTY; 8];
        return Ok(());
    }
    if matches!(mnemonic, Movd | Movq) {
        return mov(cpu, instr);
    }

    let src = read(cpu, instr, 1)?;
    let dst = mm(cpu, instr.op0_register());
    let result = match mnemonic {
        Paddb => lanes::<8>(dst, src, u64::wrapping_add),
        Paddw => lanes::<16>(dst, src, u64::wrapping_add),
        Paddd => lanes::<32>(dst, src, u64::wrapping_add),
        Paddsb => signed::<8>(dst, src, |a, b| a + b),
        Paddsw => signed::<16>(dst, src, |a, b| a + b),
        Paddusb => unsigned::<8>(dst, src, |a, b| a + b),
        Paddusw => unsigned::<16>(dst, src, |a, b| a + b),
        Psubb => lanes::<8>(dst, src, u64::wrapping_sub),
        Psubw => lanes::<16>(dst, src, u64::wrapping_sub),
        Psubd => lanes::<32>(dst, src, u64::wrapping_sub),
        Psubsb => signed::<8>(dst, src, |a, b| a - b),
        Psubsw => signed::<16>(dst, src, |a, b| a - b),
        Psubusb => unsigned::<8>(dst, src, |a, b| a - b),
        Psubusw => unsigned::<16>(dst, src, |a, b| a - b),
        Pmullw => lanes::<16>(dst, src, |a, b| (sx::<16>(a) * sx::<16>(b)) as u64),
        Pmulhw => lanes::<16>(dst, src, |a, b| ((sx::<16>(a) * sx::<16>(b)) >> 16) as u64),
        // The two products of each dword's words, added; 8000h times
        // 8000h twice wraps to 80000000h.
        Pmaddwd => lanes::<32>(dst, src, |a, b| {
            let lo = sx::<16>(a) * sx::<16>(b);
            let hi = sx::<16>(a >> 16) * sx::<16>(b >> 16);
            (lo as i32).wrapping_add(hi as i32) as u32 as u64
        }),
        Pcmpeqb => lanes::<8>(dst, src, |a, b| mask(a == b)),
        Pcmpeqw => lanes::<16>(dst, src, |a, b| mask(a == b)),
        Pcmpeqd => lanes::<32>(dst, src, |a, b| mask(a == b)),
        Pcmpgtb => lanes::<8>(dst, src, |a, b| mask(sx::<8>(a) > sx::<8>(b))),
        Pcmpgtw => lanes::<16>(dst, src, |a, b| mask(sx::<16>(a) > sx::<16>(b))),
        Pcmpgtd => lanes::<32>(dst, src, |a, b| mask(sx::<32>(a) > sx::<32>(b))),
        Pand => dst & src,
        Pandn => !dst & src,
        Por => dst | src,
        Pxor => dst ^ src,
        Psllw => shift::<16>(dst, src, |a, n| a << n, false),
        Pslld => shift::<32>(dst, src, |a, n| a << n, false),
        Psllq => shift::<64>(dst, src, |a, n| a << n, false),
        Psrlw => shift::<16>(dst, src, |a, n| a >> n, false),
        Psrld => shift::<32>(dst, src, |a, n| a >> n, false),
        Psrlq => shift::<64>(dst, src, |a, n| a >> n, false),
        Psraw => shift::<16>(dst, src, |a, n| (sx::<16>(a) >> n) as u64, true),
        Psrad => shift::<32>(dst, src, |a, n| (sx::<32>(a) >> n) as u64, true),
        Packsswb => pack::<16>(dst, src, -0x80, 0x7F),
        Packssdw => pack::<32>(dst, src, -0x8000, 0x7FFF),
        Packuswb => pack::<16>(dst, src, 0, 0xFF),
        Punpcklbw => unpack::<8>(dst, src, false),
        Punpcklwd => unpack::<16>(dst, src, false),
        Punpckldq => unpack::<32>(dst, src, false),
        Punpckhbw => unpack::<8>(dst, src, true),
        Punpckhwd => unpack::<16>(dst, src, true),
        Punpckhdq => unpack::<32>(dst, src, true),
        _ => return Err(Fault::UD),
    };
    enter_mmx(cpu);
    set_mm(cpu, instr.op0_register(), result);
    Ok(())
}

/// MOVD and MOVQ: MMn from or to a register, memory or another MMn. MOVD
/// zero-extends a dword into MMn and stores its low dword.
fn mov(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let value = read(cpu, instr, 1)?;
    match instr.op0_kind() {
        OpKind::Register if is_mm(instr.op0_register()) => {
            enter_mmx(cpu);
            set_mm(cpu, instr.op0_register(), value);
        }
        OpKind::Register => {
            enter_mmx(cpu);
            cpu.set_reg(instr.op0_register(), value as u32);
        }
        OpKind::Memory => {
            let size = instr.memory_size().size() as u32;
            cpu.check_span(mem_seg(instr), effective_offset(cpu, instr), size, Access::Write)?;
            let lo = mem_operand(cpu, instr, 4, Access::Write)?;
            let hi = if size == 8 { Some(mem_operand_at(cpu, instr, 4, 4, Access::Write)?) } else { None };
            enter_mmx(cpu);
            cpu.mem_write(lo, value as u32);
            if let Some(hi) = hi {
                cpu.mem_write(hi, (value >> 32) as u32);
            }
        }
        _ => return Err(Fault::UD),
    }
    Ok(())
}

/// Operand `i`: MMn, a 32-bit register, a dword or quadword in memory
/// (checked whole before it is read), or a shift's imm8.
fn read(cpu: &mut Cpu, instr: &Instruction, i: u32) -> CpuResult<u64> {
    Ok(match instr.op_kind(i) {
        OpKind::Register if is_mm(instr.op_register(i)) => mm(cpu, instr.op_register(i)),
        OpKind::Register => cpu.reg(instr.op_register(i)) as u64,
        OpKind::Memory => {
            let size = instr.memory_size().size() as u32;
            cpu.check_span(mem_seg(instr), effective_offset(cpu, instr), size, Access::Read)?;
            let lo = mem_operand(cpu, instr, 4, Access::Read)?;
            let lo = cpu.mem_read(lo) as u64;
            if size == 8 {
                let hi = mem_operand_at(cpu, instr, 4, 4, Access::Read)?;
                lo | (cpu.mem_read(hi) as u64) << 32
            } else {
                lo
            }
        }
        OpKind::Immediate8 => instr.immediate8() as u64,
        _ => return Err(Fault::UD),
    })
}

fn is_mm(reg: Register) -> bool {
    (Register::MM0..=Register::MM7).contains(&reg)
}

/// MMn: the significand of physical FPU register n.
fn mm(cpu: &Cpu, reg: Register) -> u64 {
    cpu.fpu_reg(reg as usize - Register::MM0 as usize).st as u64
}

fn set_mm(cpu: &mut Cpu, reg: Register, value: u64) {
    cpu.fpu_set_reg(reg as usize - Register::MM0 as usize, crate::f80::F80 { st: MMX_EXPONENT | value as u128 });
}

/// What every MMX instruction but EMMS does to the FPU: top of stack 0,
/// all registers valid.
fn enter_mmx(cpu: &mut Cpu) {
    cpu.fpu_top = 0;
    cpu.fpu_tags = [FPU_TAG_VALID; 8];
}

/// All ones where `c` holds, for a lane the caller masks.
fn mask(c: bool) -> u64 {
    if c { u64::MAX } else { 0 }
}

/// The low `W` bits of `v`, sign-extended.
fn sx<const W: u32>(v: u64) -> i64 {
    ((v << (64 - W)) as i64) >> (64 - W)
}

/// `f` of each `W`-bit lane of `a` and `b` (zero-extended), cut to `W` bits.
fn lanes<const W: u32>(a: u64, b: u64, f: impl Fn(u64, u64) -> u64) -> u64 {
    let m = u64::MAX >> (64 - W);
    (0..64 / W).fold(0, |r, i| {
        let s = i * W;
        r | (f(a >> s & m, b >> s & m) & m) << s
    })
}

/// `f` of each lane's signed values, saturated to a signed lane.
fn signed<const W: u32>(a: u64, b: u64, f: impl Fn(i64, i64) -> i64) -> u64 {
    let (lo, hi) = (-(1i64 << (W - 1)), (1i64 << (W - 1)) - 1);
    lanes::<W>(a, b, |x, y| f(sx::<W>(x), sx::<W>(y)).clamp(lo, hi) as u64)
}

/// `f` of each lane's unsigned values, saturated to an unsigned lane.
fn unsigned<const W: u32>(a: u64, b: u64, f: impl Fn(i64, i64) -> i64) -> u64 {
    let hi = (1i64 << W) - 1;
    lanes::<W>(a, b, |x, y| f(x as i64, y as i64).clamp(0, hi) as u64)
}

/// Each lane of `a` shifted by the count `n` (all 64 bits of it): past the
/// lane's width, a logical shift gives 0 and an arithmetic one the sign.
fn shift<const W: u32>(a: u64, n: u64, f: impl Fn(u64, u32) -> u64, arithmetic: bool) -> u64 {
    if n >= W as u64 && !arithmetic {
        return 0;
    }
    let n = n.min(W as u64 - 1) as u32;
    lanes::<W>(a, 0, |x, _| f(x, n))
}

/// PACKSS and PACKUS: the `W`-bit signed lanes of `a`, then of `b`, each
/// saturated to [`lo`, `hi`] in a lane half as wide.
fn pack<const W: u32>(a: u64, b: u64, lo: i64, hi: i64) -> u64 {
    let half = W / 2;
    let n = 64 / W;
    (0..2 * n).fold(0, |r, i| {
        let v = if i < n { a >> (i * W) } else { b >> ((i - n) * W) };
        let v = sx::<W>(v).clamp(lo, hi) as u64 & (u64::MAX >> (64 - half));
        r | v << (i * half)
    })
}

/// PUNPCK: the low (or high) `W`-bit lanes of `a` and `b`, interleaved,
/// starting with `a`'s.
fn unpack<const W: u32>(a: u64, b: u64, high: bool) -> u64 {
    let m = u64::MAX >> (64 - W);
    let n = 32 / W;
    let base = if high { n } else { 0 };
    (0..n).fold(0, |r, i| {
        let (x, y) = (a >> ((base + i) * W) & m, b >> ((base + i) * W) & m);
        r | x << (2 * i * W) | y << ((2 * i + 1) * W)
    })
}
