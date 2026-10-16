//! MMX and SSE instructions as operations: moves, logic, shuffles and the
//! packed integer forms, and the single-precision arithmetic, which runs
//! on the host's SSE with MXCSR's rounding (`Uop::VFloat`). Each starts
//! with a `SimdGuard`, which leaves the rare cases (CR0's EM or TS set, SSE
//! not enabled in CR4, an MMX register the FPU left stale) to the handler,
//! and an operand of 8 or 16 bytes that isn't plain RAM within a page goes
//! to the handler too (`BailUnlessRam`). RCPPS, RSQRTPS and their scalar
//! forms, whose approximations the host's don't match, the compares into
//! EFLAGS, the conversions to and from 32-bit registers, PINSRW and
//! MASKMOVQ go through their handlers.

use iced_x86::{CpuidFeature, Instruction, Mnemonic, OpKind, Register};

use super::translate::{ea, gpr, mem};
use super::uop::*;

/// The `simd` bits of `translate::translate`: the CPU has MMX, or SSE.
pub const MMX: u8 = 1;
pub const SSE: u8 = 2;

/// The operations of MMX or SSE instruction `instr`, or false, with `u`
/// as it was, if its handler runs it. `simd` says which the CPU has.
pub fn translate(instr: &Instruction, simd: u8, u: &mut Vec<Uop>) -> bool {
    let features = instr.cpuid_features();
    let start = u.len();
    let ok = if features == [CpuidFeature::MMX] {
        simd & MMX != 0 && mmx(instr, u).is_some()
    } else if features == [CpuidFeature::SSE] {
        simd & SSE != 0 && sse(instr, u).is_some()
    } else {
        false
    };
    if !ok {
        // (A form ruled out partway through has pushed some already.)
        u.truncate(start);
    }
    ok
}

fn mm(reg: Register) -> Option<u8> {
    (Register::MM0..=Register::MM7)
        .contains(&reg)
        .then(|| reg as u8 - Register::MM0 as u8)
}

fn xmm(reg: Register) -> Option<u8> {
    reg.is_xmm().then(|| reg as u8 - Register::XMM0 as u8)
}

/// MMn of register operand `op`.
fn mm_op(instr: &Instruction, op: u32) -> Option<u8> {
    (instr.op_kind(op) == OpKind::Register)
        .then(|| mm(instr.op_register(op)))
        .flatten()
}

fn xmm_op(instr: &Instruction, op: u32) -> Option<u8> {
    (instr.op_kind(op) == OpKind::Register)
        .then(|| xmm(instr.op_register(op)))
        .flatten()
}

/// The 32-bit register of operand `op`.
fn gpr32_op(instr: &Instruction, op: u32) -> Option<Gpr> {
    let reg = instr.op_register(op);
    (instr.op_kind(op) == OpKind::Register && reg.is_gpr32())
        .then(|| gpr(reg))
        .flatten()
}

/// The guard for an instruction reading MMn for each bit n of `mm`.
fn guard(u: &mut Vec<Uop>, xmm: bool, mm: u8) {
    u.push(Uop::SimdGuard { xmm, mm });
}

/// Check the memory operand for an access of `size` bytes, aligned to 16
/// where `aligned` (else the handler raises #GP(0)): T1 then refers to it.
fn mem_checked(
    instr: &Instruction,
    size: u8,
    write: bool,
    aligned: bool,
    u: &mut Vec<Uop>,
) -> Option<()> {
    let seg = ea(instr, T1, u)?;
    if aligned {
        u.push(Uop::BailMisaligned { t: T1, seg });
    }
    u.push(Uop::MemRef {
        t: T1,
        seg,
        size,
        write,
        slot: 0,
    });
    Some(())
}

/// x = the memory operand, `size` bytes (4, 8 or 16) zero-extended.
fn load_mem(instr: &Instruction, x: X, size: u8, aligned: bool, u: &mut Vec<Uop>) -> Option<()> {
    mem_checked(instr, size, false, aligned, u)?;
    if size == 4 {
        u.push(Uop::Load {
            dst: T0,
            m: T1,
            size: 4,
        });
        u.push(Uop::VLoad {
            x,
            src: VSrc::T(T0),
        });
    } else {
        u.push(Uop::BailUnlessRam { t: T1 });
        u.push(Uop::VLoad {
            x,
            src: VSrc::Mem { m: T1, size },
        });
    }
    Some(())
}

/// x = operand 1: an MMX or XMM register, or memory of `size` bytes.
fn source(instr: &Instruction, x: X, size: u8, aligned: bool, u: &mut Vec<Uop>) -> Option<()> {
    match instr.op1_kind() {
        OpKind::Register => {
            let reg = instr.op1_register();
            let r = match (mm(reg), xmm(reg)) {
                (Some(n), _) => Vr::Mm(n),
                (_, Some(n)) => Vr::Xmm(n),
                _ => return None,
            };
            u.push(Uop::VLoad { x, src: VSrc::R(r) });
            Some(())
        }
        OpKind::Memory => load_mem(instr, x, size, aligned, u),
        _ => None,
    }
}

/// The MMX register operand 1 reads, as a `SimdGuard`'s bit.
fn mm_read(instr: &Instruction) -> u8 {
    mm_op(instr, 1).map_or(0, |n| 1 << n)
}

/// x's low `size` bytes (8 or 16) into the memory operand, after the
/// operations `value` emits: checked before them, as the handlers check
/// it before they change anything.
fn store(
    instr: &Instruction,
    size: u8,
    aligned: bool,
    u: &mut Vec<Uop>,
    value: impl FnOnce(&mut Vec<Uop>),
) -> Option<()> {
    mem_checked(instr, size, true, aligned, u)?;
    u.push(Uop::BailUnlessPlain { t: T1 });
    value(u);
    u.push(Uop::VStore { m: T1, x: X0, size });
    Some(())
}

/// The low dword of X0, which `load` puts there, into the 32-bit register
/// or memory of operand 0, after `EnterMmx` where `mmx`. Memory is checked
/// first, as the handlers check it before they change anything.
fn store_dword(
    instr: &Instruction,
    mmx: bool,
    u: &mut Vec<Uop>,
    load: impl FnOnce(&mut Vec<Uop>),
) -> Option<()> {
    let r = match instr.op0_kind() {
        OpKind::Register => Some(gpr32_op(instr, 0)?),
        OpKind::Memory => {
            mem(instr, T1, 4, true, u)?;
            None
        }
        _ => return None,
    };
    load(u);
    u.push(Uop::VToT {
        t: T0,
        x: X0,
        op: VExtract::Low,
    });
    if mmx {
        u.push(Uop::EnterMmx);
    }
    u.push(match r {
        Some(r) => Uop::Set { r, t: T0 },
        None => Uop::Store {
            m: T1,
            src: T0,
            size: 4,
        },
    });
    Some(())
}

/// MMn (operand 0) = x, as an MMX instruction leaves it.
fn set_mm(u: &mut Vec<Uop>, n: u8, x: X) {
    u.push(Uop::EnterMmx);
    u.push(Uop::VSet { dst: Vr::Mm(n), x });
}

fn mmx(instr: &Instruction, u: &mut Vec<Uop>) -> Option<()> {
    use Mnemonic::*;
    let op = match instr.mnemonic() {
        Emms => {
            guard(u, false, 0);
            u.push(Uop::Emms);
            return Some(());
        }
        Movd | Movq => return mmx_mov(instr, u),
        Paddb => VOp::Paddb,
        Paddw => VOp::Paddw,
        Paddd => VOp::Paddd,
        Paddsb => VOp::Paddsb,
        Paddsw => VOp::Paddsw,
        Paddusb => VOp::Paddusb,
        Paddusw => VOp::Paddusw,
        Psubb => VOp::Psubb,
        Psubw => VOp::Psubw,
        Psubd => VOp::Psubd,
        Psubsb => VOp::Psubsb,
        Psubsw => VOp::Psubsw,
        Psubusb => VOp::Psubusb,
        Psubusw => VOp::Psubusw,
        Pmullw => VOp::Pmullw,
        Pmulhw => VOp::Pmulhw,
        Pmaddwd => VOp::Pmaddwd,
        Pcmpeqb => VOp::Pcmpeqb,
        Pcmpeqw => VOp::Pcmpeqw,
        Pcmpeqd => VOp::Pcmpeqd,
        Pcmpgtb => VOp::Pcmpgtb,
        Pcmpgtw => VOp::Pcmpgtw,
        Pcmpgtd => VOp::Pcmpgtd,
        Pand => VOp::Pand,
        Pandn => VOp::Pandn,
        Por => VOp::Por,
        Pxor => VOp::Pxor,
        Psllw => shift(instr, VOp::Psllw, VShift::Psllw),
        Pslld => shift(instr, VOp::Pslld, VShift::Pslld),
        Psllq => shift(instr, VOp::Psllq, VShift::Psllq),
        Psrlw => shift(instr, VOp::Psrlw, VShift::Psrlw),
        Psrld => shift(instr, VOp::Psrld, VShift::Psrld),
        Psrlq => shift(instr, VOp::Psrlq, VShift::Psrlq),
        Psraw => shift(instr, VOp::Psraw, VShift::Psraw),
        Psrad => shift(instr, VOp::Psrad, VShift::Psrad),
        Packsswb => VOp::Packsswb,
        Packssdw => VOp::Packssdw,
        Packuswb => VOp::Packuswb,
        Punpcklbw => VOp::Punpcklbw,
        Punpcklwd => VOp::Punpcklwd,
        Punpckldq => VOp::Punpckldq,
        Punpckhbw => VOp::Punpckhbw,
        Punpckhwd => VOp::Punpckhwd,
        Punpckhdq => VOp::Punpckhdq,
        _ => return None,
    };
    mmx_binary(instr, op, u)
}

/// A shift's operation: by a constant, or by operand 1's quadword.
fn shift(instr: &Instruction, by_reg: VOp, by_imm: VShift) -> VOp {
    if instr.op1_kind() == OpKind::Immediate8 {
        VOp::ShiftImm(by_imm, instr.immediate8())
    } else {
        by_reg
    }
}

/// MMn (operand 0) = MMn OP operand 1 (or a shift by a constant).
fn mmx_binary(instr: &Instruction, op: VOp, u: &mut Vec<Uop>) -> Option<()> {
    let d = mm_op(instr, 0)?;
    guard(u, false, 1 << d | mm_read(instr));
    if !matches!(op, VOp::ShiftImm(..)) {
        let size = if instr.op1_kind() == OpKind::Memory {
            instr.memory_size().size() as u8
        } else {
            8
        };
        source(instr, X1, size, false, u)?;
    }
    u.push(Uop::VLoad {
        x: X0,
        src: VSrc::R(Vr::Mm(d)),
    });
    u.push(Uop::VOp { op, a: X0, b: X1 });
    set_mm(u, d, X0);
    Some(())
}

/// MOVD and MOVQ (and MOVNTQ): MMn from or to a 32-bit register, memory or
/// another MMn.
fn mmx_mov(instr: &Instruction, u: &mut Vec<Uop>) -> Option<()> {
    let quad = instr.mnemonic() != Mnemonic::Movd;
    if let Some(d) = mm_op(instr, 0) {
        guard(u, false, mm_read(instr));
        match (quad, instr.op1_kind()) {
            (false, OpKind::Register) => {
                u.push(Uop::Get {
                    t: T0,
                    r: gpr32_op(instr, 1)?,
                });
                u.push(Uop::VLoad {
                    x: X0,
                    src: VSrc::T(T0),
                });
            }
            _ => source(instr, X0, if quad { 8 } else { 4 }, false, u)?,
        }
        set_mm(u, d, X0);
        return Some(());
    }
    let s = mm_op(instr, 1)?;
    guard(u, false, 1 << s);
    if quad {
        if instr.op0_kind() != OpKind::Memory {
            return None;
        }
        return store(instr, 8, false, u, |u| {
            u.push(Uop::VLoad {
                x: X0,
                src: VSrc::R(Vr::Mm(s)),
            });
            u.push(Uop::EnterMmx);
        });
    }
    store_dword(instr, true, u, |u| {
        u.push(Uop::VLoad {
            x: X0,
            src: VSrc::R(Vr::Mm(s)),
        })
    })
}

fn sse(instr: &Instruction, u: &mut Vec<Uop>) -> Option<()> {
    use Mnemonic::*;
    let float = |op: VFloatOp| (op, false);
    let (op, scalar) = match instr.mnemonic() {
        // The integer instructions on MMX registers.
        Pavgb => return mmx_binary(instr, VOp::Pavgb, u),
        Pavgw => return mmx_binary(instr, VOp::Pavgw, u),
        Pmaxsw => return mmx_binary(instr, VOp::Pmaxsw, u),
        Pmaxub => return mmx_binary(instr, VOp::Pmaxub, u),
        Pminsw => return mmx_binary(instr, VOp::Pminsw, u),
        Pminub => return mmx_binary(instr, VOp::Pminub, u),
        Pmulhuw => return mmx_binary(instr, VOp::Pmulhuw, u),
        Psadbw => return mmx_binary(instr, VOp::Psadbw, u),
        Pshufw => {
            // Of operand 1 alone.
            let d = mm_op(instr, 0)?;
            guard(u, false, mm_read(instr));
            source(instr, X1, 8, false, u)?;
            u.push(Uop::VOp {
                op: VOp::Pshufw(instr.immediate8()),
                a: X0,
                b: X1,
            });
            set_mm(u, d, X0);
            return Some(());
        }
        Pextrw | Pmovmskb => {
            let (r, s) = (gpr32_op(instr, 0)?, mm_op(instr, 1)?);
            guard(u, false, 1 << s);
            let op = if instr.mnemonic() == Pextrw {
                VExtract::Word(instr.immediate8() & 3)
            } else {
                VExtract::Pmovmskb
            };
            u.push(Uop::VLoad {
                x: X0,
                src: VSrc::R(Vr::Mm(s)),
            });
            u.push(Uop::VToT { t: T0, x: X0, op });
            u.push(Uop::EnterMmx);
            u.push(Uop::Set { r, t: T0 });
            return Some(());
        }
        Movntq => return mmx_mov(instr, u),
        Movaps | Movups | Movntps => return mov_packed(instr, instr.mnemonic() != Movups, u),
        Movss => return movss(instr, u),
        Movlps | Movhps => return mov_half(instr, instr.mnemonic() == Movhps, u),
        Movhlps => return bitwise(instr, VOp::Movhlps, u),
        Movlhps => return bitwise(instr, VOp::Movlhps, u),
        Movmskps => {
            let (r, s) = (gpr32_op(instr, 0)?, xmm_op(instr, 1)?);
            guard(u, true, 0);
            u.push(Uop::VLoad {
                x: X0,
                src: VSrc::R(Vr::Xmm(s)),
            });
            u.push(Uop::VToT {
                t: T0,
                x: X0,
                op: VExtract::Movmskps,
            });
            u.push(Uop::Set { r, t: T0 });
            return Some(());
        }
        Andps => return bitwise(instr, VOp::Andps, u),
        Andnps => return bitwise(instr, VOp::Andnps, u),
        Orps => return bitwise(instr, VOp::Orps, u),
        Xorps => return bitwise(instr, VOp::Xorps, u),
        Unpcklps => return bitwise(instr, VOp::Unpcklps, u),
        Unpckhps => return bitwise(instr, VOp::Unpckhps, u),
        Shufps => return bitwise(instr, VOp::Shufps(instr.immediate8()), u),
        Addps => float(VFloatOp::Add),
        Subps => float(VFloatOp::Sub),
        Mulps => float(VFloatOp::Mul),
        Divps => float(VFloatOp::Div),
        Minps => float(VFloatOp::Min),
        Maxps => float(VFloatOp::Max),
        Sqrtps => float(VFloatOp::Sqrt),
        Cmpps if instr.immediate8() < 8 => float(VFloatOp::Cmp(instr.immediate8())),
        Addss => (VFloatOp::Add, true),
        Subss => (VFloatOp::Sub, true),
        Mulss => (VFloatOp::Mul, true),
        Divss => (VFloatOp::Div, true),
        Minss => (VFloatOp::Min, true),
        Maxss => (VFloatOp::Max, true),
        Sqrtss => (VFloatOp::Sqrt, true),
        Cmpss if instr.immediate8() < 8 => (VFloatOp::Cmp(instr.immediate8()), true),
        Cvtps2pi | Cvttps2pi => {
            let d = mm_op(instr, 0)?;
            if instr.op1_kind() == OpKind::Register {
                xmm_op(instr, 1)?;
            }
            guard(u, true, 0);
            source(instr, X1, 8, false, u)?;
            let truncate = instr.mnemonic() == Cvttps2pi;
            u.push(Uop::VFloat {
                op: VFloatOp::ToInt { truncate },
                scalar: false,
                a: X0,
                b: X1,
            });
            set_mm(u, d, X0);
            return Some(());
        }
        Cvtpi2ps => {
            let d = xmm_op(instr, 0)?;
            let from_mm = mm_op(instr, 1).is_some();
            guard(u, true, mm_read(instr));
            source(instr, X1, 8, false, u)?;
            u.push(Uop::VLoad {
                x: X0,
                src: VSrc::R(Vr::Xmm(d)),
            });
            u.push(Uop::VFloat {
                op: VFloatOp::FromInt,
                scalar: false,
                a: X0,
                b: X1,
            });
            if from_mm {
                u.push(Uop::EnterMmx);
            }
            u.push(Uop::VSet {
                dst: Vr::Xmm(d),
                x: X0,
            });
            return Some(());
        }
        _ => return None,
    };
    let d = xmm_op(instr, 0)?;
    if instr.op1_kind() == OpKind::Register {
        xmm_op(instr, 1)?;
    }
    guard(u, true, 0);
    source(instr, X1, if scalar { 4 } else { 16 }, !scalar, u)?;
    u.push(Uop::VLoad {
        x: X0,
        src: VSrc::R(Vr::Xmm(d)),
    });
    u.push(Uop::VFloat {
        op,
        scalar,
        a: X0,
        b: X1,
    });
    u.push(Uop::VSet {
        dst: Vr::Xmm(d),
        x: X0,
    });
    Some(())
}

/// XMMn (operand 0) = XMMn OP operand 1, 16 bytes aligned: the logical
/// instructions, UNPCKLPS, UNPCKHPS and SHUFPS, and (between registers
/// only) MOVHLPS and MOVLHPS.
fn bitwise(instr: &Instruction, op: VOp, u: &mut Vec<Uop>) -> Option<()> {
    let d = xmm_op(instr, 0)?;
    if instr.op1_kind() == OpKind::Register {
        xmm_op(instr, 1)?;
    }
    guard(u, true, 0);
    source(instr, X1, 16, true, u)?;
    u.push(Uop::VLoad {
        x: X0,
        src: VSrc::R(Vr::Xmm(d)),
    });
    u.push(Uop::VOp { op, a: X0, b: X1 });
    u.push(Uop::VSet {
        dst: Vr::Xmm(d),
        x: X0,
    });
    Some(())
}

/// MOVAPS, MOVUPS and MOVNTPS: 128 bits between XMM registers, or from or
/// to memory (`aligned` but for MOVUPS).
fn mov_packed(instr: &Instruction, aligned: bool, u: &mut Vec<Uop>) -> Option<()> {
    if let Some(d) = xmm_op(instr, 0) {
        if instr.op1_kind() == OpKind::Register {
            xmm_op(instr, 1)?;
        }
        guard(u, true, 0);
        source(instr, X0, 16, aligned, u)?;
        u.push(Uop::VSet {
            dst: Vr::Xmm(d),
            x: X0,
        });
        return Some(());
    }
    let s = xmm_op(instr, 1)?;
    guard(u, true, 0);
    store(instr, 16, aligned, u, |u| {
        u.push(Uop::VLoad {
            x: X0,
            src: VSrc::R(Vr::Xmm(s)),
        })
    })
}

/// MOVSS: the low single between XMM registers (the others stay), from
/// memory (the others cleared), or to memory.
fn movss(instr: &Instruction, u: &mut Vec<Uop>) -> Option<()> {
    match (xmm_op(instr, 0), xmm_op(instr, 1)) {
        (Some(d), Some(s)) => {
            guard(u, true, 0);
            u.push(Uop::VLoad {
                x: X0,
                src: VSrc::R(Vr::Xmm(d)),
            });
            u.push(Uop::VLoad {
                x: X1,
                src: VSrc::R(Vr::Xmm(s)),
            });
            u.push(Uop::VOp {
                op: VOp::Movss,
                a: X0,
                b: X1,
            });
            u.push(Uop::VSet {
                dst: Vr::Xmm(d),
                x: X0,
            });
        }
        (Some(d), None) => {
            guard(u, true, 0);
            load_mem(instr, X0, 4, false, u)?;
            u.push(Uop::VSet {
                dst: Vr::Xmm(d),
                x: X0,
            });
        }
        (None, Some(s)) => {
            guard(u, true, 0);
            store_dword(instr, false, u, |u| {
                u.push(Uop::VLoad {
                    x: X0,
                    src: VSrc::R(Vr::Xmm(s)),
                })
            })?;
        }
        (None, None) => return None,
    }
    Some(())
}

/// MOVLPS and MOVHPS: the low or `high` quadword from or to memory; the
/// other stays.
fn mov_half(instr: &Instruction, high: bool, u: &mut Vec<Uop>) -> Option<()> {
    if let Some(d) = xmm_op(instr, 0) {
        guard(u, true, 0);
        load_mem(instr, X1, 8, false, u)?;
        u.push(Uop::VLoad {
            x: X0,
            src: VSrc::R(Vr::Xmm(d)),
        });
        u.push(Uop::VOp {
            op: if high { VOp::Movlhps } else { VOp::Movsd },
            a: X0,
            b: X1,
        });
        u.push(Uop::VSet {
            dst: Vr::Xmm(d),
            x: X0,
        });
        return Some(());
    }
    let s = xmm_op(instr, 1)?;
    guard(u, true, 0);
    store(instr, 8, false, u, |u| {
        u.push(Uop::VLoad {
            x: X0,
            src: VSrc::R(Vr::Xmm(s)),
        });
        if high {
            u.push(Uop::VOp {
                op: VOp::Movhlps,
                a: X0,
                b: X0,
            });
        }
    })
}
