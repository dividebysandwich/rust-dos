//! The status word the x87's compares leave (FCOM, FCOMP, FCOMPP, FUCOM,
//! FUCOMP, FUCOMPP, FICOM, FICOMP, FTST, FCOMI, FUCOMI and their popping
//! forms), on the interpreter and the recompiler in lockstep, against the
//! Intel SDM (vol. 1 sections 8.1.3.1 and 8.5.1, and the instructions'
//! pages): C1 is cleared, also on a stack underflow; an empty register is
//! a stack fault (SF and IE) and compares unordered; a NaN is invalid (IE)
//! for FCOM, FICOM, FTST and FCOMI, a signalling one only for FUCOM and
//! FUCOMI; a format the FPU doesn't take is invalid for all of them.

use crate::dyndiff::lockstep_with;
use crate::pmrig::*;

use chrono::NaiveDate;
use iced_x86::code_asm::*;
use rust_dos::cpu::{CoreMode, CpuModel, Seg};
use rust_dos::dynrec::AVAILABLE;

/// An operand: a register's 80 bits (sign and exponent, significand), a
/// register loaded from a double, an empty register, or in memory a
/// single, a double, a word or a dword.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Ext(u16, u64),
    Dbl(f64),
    Empty,
    M32(u32),
    M64(f64),
    M16i(i16),
    M32i(i32),
}

const ONE: Op = Op::Ext(0x3FFF, 0x8000_0000_0000_0000);
const TWO: Op = Op::Ext(0x4000, 0x8000_0000_0000_0000);
const QNAN: Op = Op::Ext(0x7FFF, 0xC000_0000_0000_0001);
const SNAN: Op = Op::Ext(0x7FFF, 0x8000_0000_0000_0001);
/// 1 with the integer bit clear (an unnormal), which no FPU since the 387
/// takes.
const UNNORMAL: Op = Op::Ext(0x3FFF, 0x4000_0000_0000_0000);
const SINGLE_QNAN: u32 = 0x7FC0_0001;
const SINGLE_SNAN: u32 = 0x7F80_0001;

/// What an operand is to a compare.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Empty,
    Number(f64),
    Quiet,
    Signalling,
    Unsupported,
}

fn kind(op: Op) -> Kind {
    match op {
        Op::Empty => Kind::Empty,
        Op::Ext(se, man) if se & 0x7FFF == 0x7FFF && man << 1 != 0 => {
            if man & 1 << 62 != 0 {
                Kind::Quiet
            } else {
                Kind::Signalling
            }
        }
        Op::Ext(se, man) if se & 0x7FFF != 0 && man >> 63 == 0 => Kind::Unsupported,
        Op::Ext(se, man) => {
            let exp = (se & 0x7FFF) as i32 - 16383;
            let sign = if se & 0x8000 != 0 { -1.0 } else { 1.0 };
            Kind::Number(sign * man as f64 * 2f64.powi(exp - 63))
        }
        Op::Dbl(x) | Op::M64(x) if x.is_nan() => {
            if x.to_bits() & 1 << 51 != 0 {
                Kind::Quiet
            } else {
                Kind::Signalling
            }
        }
        Op::M32(bits) if f32::from_bits(bits).is_nan() => {
            if bits & 1 << 22 != 0 {
                Kind::Quiet
            } else {
                Kind::Signalling
            }
        }
        Op::Dbl(x) | Op::M64(x) => Kind::Number(x),
        Op::M32(bits) => Kind::Number(f32::from_bits(bits) as f64),
        Op::M16i(i) => Kind::Number(i as f64),
        Op::M32i(i) => Kind::Number(i as f64),
    }
}

/// The instructions, each comparing ST(0) with ST(1) or with a memory
/// operand (FTST with 0).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Ins {
    Fcom,
    Fcomp,
    Fcompp,
    Fucom,
    Fucomp,
    Fucompp,
    FcomMem,
    FcompMem,
    Ficom,
    Ficomp,
    Ftst,
    Fcomi,
    Fucomip,
}

impl Ins {
    /// Whether only a signalling NaN is invalid (FUCOM and FUCOMI).
    fn unordered(self) -> bool {
        matches!(self, Ins::Fucom | Ins::Fucomp | Ins::Fucompp | Ins::Fucomip)
    }

    fn pops(self) -> u16 {
        match self {
            Ins::Fcomp | Ins::Fucomp | Ins::FcompMem | Ins::Ficomp | Ins::Fucomip => 1,
            Ins::Fcompp | Ins::Fucompp => 2,
            _ => 0,
        }
    }

    /// Whether it sets EFLAGS rather than C0, C2 and C3.
    fn eflags(self) -> bool {
        matches!(self, Ins::Fcomi | Ins::Fucomip)
    }
}

const C0: u16 = 0x0100;
const C1: u16 = 0x0200;
const C2: u16 = 0x0400;
const C3: u16 = 0x4000;
const IE: u16 = 0x0001;
const SF: u16 = 0x0040;
const UNORDERED: u16 = C3 | C2 | C0;

/// The status word's C0 to C3, IE and SF after `ins` of `first` and
/// `other` (ST(1) or the memory operand), started with C0 to C3 set, as
/// the SDM has it with IE masked: an empty register is a stack underflow
/// (SF, IE, C1 0, unordered), an invalid operand sets IE (unordered), and
/// C1 is cleared.
fn expected(ins: Ins, first: Op, other: Op) -> u16 {
    let kinds = if ins == Ins::Ftst {
        [kind(first), Kind::Number(0.0)]
    } else {
        [kind(first), kind(other)]
    };
    let invalid = |k: &Kind| match k {
        Kind::Unsupported | Kind::Signalling => true,
        Kind::Quiet => !ins.unordered(),
        _ => false,
    };
    let (flags, order) = if kinds.contains(&Kind::Empty) {
        (SF | IE, UNORDERED)
    } else if kinds.iter().any(invalid) {
        (IE, UNORDERED)
    } else {
        match kinds {
            [Kind::Number(a), Kind::Number(b)] if a < b => (0, C0),
            [Kind::Number(a), Kind::Number(b)] if a > b => (0, 0),
            [Kind::Number(_), Kind::Number(_)] => (0, C3),
            _ => (0, UNORDERED),
        }
    };
    // FCOMI and FUCOMI leave C0, C2 and C3 as they were.
    flags | if ins.eflags() { UNORDERED } else { order }
}

/// ZF, PF and CF that FCOMI and FUCOMI leave: the order as C3, C2 and C0
/// are for FCOM.
fn expected_eflags(ins: Ins, first: Op, other: Op) -> u32 {
    let codes = expected(
        match ins {
            Ins::Fcomi => Ins::Fcom,
            _ => Ins::Fucom,
        },
        first,
        other,
    );
    let (zf, pf, cf) = (codes & C3 != 0, codes & C2 != 0, codes & C0 != 0);
    (zf as u32) << 6 | (pf as u32) << 2 | cf as u32
}

/// The cases for each instruction: (ST(0), ST(1) or the memory operand).
fn cases(ins: Ins) -> Vec<(Op, Op)> {
    let numbers = [
        (ONE, TWO),
        (TWO, ONE),
        (TWO, TWO),
        (Op::Dbl(1.0), Op::Dbl(2.0)),
        (Op::Dbl(2.0), Op::Dbl(2.0)),
    ];
    let register = || {
        let mut v = numbers.to_vec();
        v.extend([
            (QNAN, ONE),
            (ONE, QNAN),
            (Op::Dbl(f64::NAN), Op::Dbl(1.0)),
            (SNAN, ONE),
            (ONE, SNAN),
            (UNNORMAL, ONE),
            (ONE, UNNORMAL),
            (Op::Empty, ONE),
            (ONE, Op::Empty),
            (Op::Empty, Op::Empty),
        ]);
        v
    };
    match ins {
        Ins::FcomMem | Ins::FcompMem => vec![
            (ONE, Op::M32(2f32.to_bits())),
            (TWO, Op::M64(1.0)),
            (TWO, Op::M64(2.0)),
            (Op::Dbl(2.0), Op::M32(1f32.to_bits())),
            (ONE, Op::M32(SINGLE_QNAN)),
            (ONE, Op::M32(SINGLE_SNAN)),
            (ONE, Op::M64(f64::from_bits(0x7FF8_0000_0000_0001))),
            (ONE, Op::M64(f64::from_bits(0x7FF0_0000_0000_0001))),
            (QNAN, Op::M64(1.0)),
            (Op::Dbl(f64::NAN), Op::M32(1f32.to_bits())),
            (SNAN, Op::M32(1f32.to_bits())),
            (UNNORMAL, Op::M64(1.0)),
            (Op::Empty, Op::M64(1.0)),
        ],
        Ins::Ficom | Ins::Ficomp => vec![
            (ONE, Op::M16i(2)),
            (TWO, Op::M32i(1)),
            (TWO, Op::M16i(2)),
            (Op::Dbl(-3.0), Op::M32i(-3)),
            (QNAN, Op::M16i(1)),
            (Op::Dbl(f64::NAN), Op::M32i(1)),
            (SNAN, Op::M32i(1)),
            (UNNORMAL, Op::M16i(1)),
            (Op::Empty, Op::M16i(1)),
        ],
        Ins::Ftst => vec![
            (ONE, Op::Empty),
            (Op::Dbl(-1.0), Op::Empty),
            (Op::Dbl(0.0), Op::Empty),
            (QNAN, Op::Empty),
            (SNAN, Op::Empty),
            (UNNORMAL, Op::Empty),
            (Op::Empty, Op::Empty),
        ],
        _ => register(),
    }
}

const INS: [Ins; 13] = [
    Ins::Fcom,
    Ins::Fcomp,
    Ins::Fcompp,
    Ins::Fucom,
    Ins::Fucomp,
    Ins::Fucompp,
    Ins::FcomMem,
    Ins::FcompMem,
    Ins::Ficom,
    Ins::Ficomp,
    Ins::Ftst,
    Ins::Fcomi,
    Ins::Fucomip,
];

/// Control words: IE masked (as after FNINIT) and unmasked.
const CONTROLS: [u16; 2] = [0x037F, 0x037E];

const OPERANDS: u32 = DATA;
const ENV: u32 = DATA + 0x7000;
const MEM: u32 = DATA + 0x7100;
const CONTROL: u32 = DATA + 0x7200;
const OUTPUT: u32 = DATA + 0x8000;

/// Push a register operand from `at` (none for an empty one).
fn push(a: &mut CodeAssembler, op: Op, at: u32) -> Result<(), IcedError> {
    match op {
        Op::Ext(..) => a.fld(tword_ptr(at)),
        Op::Dbl(_) => a.fld(qword_ptr(at)),
        _ => Ok(()),
    }
}

fn write_operand(rig: &mut Rig, op: Op, at: u32) {
    match op {
        Op::Ext(se, man) => {
            rig.write32(at, man as u32);
            rig.write32(at + 4, (man >> 32) as u32);
            rig.write16(at + 8, se);
        }
        Op::Dbl(x) | Op::M64(x) => {
            rig.write32(at, x.to_bits() as u32);
            rig.write32(at + 4, (x.to_bits() >> 32) as u32);
        }
        Op::M32(bits) => rig.write32(at, bits),
        Op::M16i(i) => rig.write16(at, i as u16),
        Op::M32i(i) => rig.write32(at, i as u32),
        Op::Empty => {}
    }
}

/// Every case of every instruction under both control words, in order.
fn all_cases() -> Vec<(u16, Ins, Op, Op)> {
    let mut all = vec![];
    for control in CONTROLS {
        for ins in INS {
            for (first, other) in cases(ins) {
                all.push((control, ins, first, other));
            }
        }
    }
    all
}

/// Run every case on the interpreter and the recompiler in lockstep, and
/// return the two rigs: each case's status word at OUTPUT + 16 * k, its
/// EFLAGS at + 4.
fn run_cases() -> (Rig, Rig) {
    rust_dos::hosttime::fix(
        NaiveDate::from_ymd_opt(1995, 4, 11)
            .unwrap()
            .and_hms_opt(12, 34, 56),
    );
    let all = all_cases();
    let setup = |rig: &mut Rig| {
        rig.cpu.model = CpuModel::PentiumPro;
        rig.write16(CONTROL, CONTROLS[0]);
        rig.write16(CONTROL + 2, CONTROLS[1]);
        for (k, &(_, ins, first, other)) in all.iter().enumerate() {
            let at = OPERANDS + 32 * k as u32;
            write_operand(rig, first, at);
            if !matches!(ins, Ins::FcomMem | Ins::FcompMem | Ins::Ficom | Ins::Ficomp) {
                write_operand(rig, other, at + 16);
            }
        }
        rig.load(
            CODE,
            &asm32(CODE, |a| {
                for (k, &(control, ins, first, other)) in all.iter().enumerate() {
                    let at = OPERANDS + 32 * k as u32;
                    let out = OUTPUT + 16 * k as u32;
                    a.fninit()?;
                    a.fldcw(word_ptr(
                        CONTROL + if control == CONTROLS[0] { 0 } else { 2 },
                    ))?;
                    // ST(1) first, then ST(0); an empty ST(0) is a register
                    // pushed and freed.
                    if !matches!(
                        ins,
                        Ins::FcomMem | Ins::FcompMem | Ins::Ficom | Ins::Ficomp | Ins::Ftst
                    ) {
                        push(a, other, at + 16)?;
                    }
                    if first == Op::Empty {
                        a.fld1()?;
                        a.ffree(st0)?;
                    } else {
                        push(a, first, at)?;
                    }
                    // The memory operand.
                    match other {
                        Op::M32(_) | Op::M64(_) | Op::M16i(_) | Op::M32i(_) => {
                            // (Written by the program, so each case has its own
                            // in the one place.)
                            let mut bytes = [0u8; 8];
                            match other {
                                Op::M32(b) => bytes[..4].copy_from_slice(&b.to_le_bytes()),
                                Op::M64(x) => bytes.copy_from_slice(&x.to_bits().to_le_bytes()),
                                Op::M16i(i) => bytes[..2].copy_from_slice(&i.to_le_bytes()),
                                Op::M32i(i) => bytes[..4].copy_from_slice(&i.to_le_bytes()),
                                _ => {}
                            }
                            a.mov(
                                dword_ptr(MEM),
                                u32::from_le_bytes(bytes[..4].try_into().unwrap()),
                            )?;
                            a.mov(
                                dword_ptr(MEM + 4),
                                u32::from_le_bytes(bytes[4..].try_into().unwrap()),
                            )?;
                        }
                        _ => {}
                    }
                    // Clear the exception flags and set C0 to C3, keeping TOP:
                    // the compare must clear C1 and set what it sets.
                    a.fnstenv(dword_ptr(ENV))?;
                    a.mov(ax, word_ptr(ENV + 4))?;
                    a.and(ax, 0x3800)?;
                    a.or(ax, (C3 | C2 | C1 | C0) as i32)?;
                    a.mov(word_ptr(ENV + 4), ax)?;
                    a.fldenv(dword_ptr(ENV))?;
                    // EFLAGS with ZF, PF and CF clear.
                    a.xor(eax, eax)?;
                    a.add(eax, 1)?;
                    match (ins, other) {
                        (Ins::Fcom, _) => a.fcom_2(st0, st1)?,
                        (Ins::Fcomp, _) => a.fcomp_2(st0, st1)?,
                        (Ins::Fcompp, _) => a.fcompp()?,
                        (Ins::Fucom, _) => a.fucom(st0, st1)?,
                        (Ins::Fucomp, _) => a.fucomp(st0, st1)?,
                        (Ins::Fucompp, _) => a.fucompp()?,
                        (Ins::FcomMem, Op::M32(_)) => a.fcom(dword_ptr(MEM))?,
                        (Ins::FcomMem, _) => a.fcom(qword_ptr(MEM))?,
                        (Ins::FcompMem, Op::M32(_)) => a.fcomp(dword_ptr(MEM))?,
                        (Ins::FcompMem, _) => a.fcomp(qword_ptr(MEM))?,
                        (Ins::Ficom, Op::M16i(_)) => a.ficom(word_ptr(MEM))?,
                        (Ins::Ficom, _) => a.ficom(dword_ptr(MEM))?,
                        (Ins::Ficomp, Op::M16i(_)) => a.ficomp(word_ptr(MEM))?,
                        (Ins::Ficomp, _) => a.ficomp(dword_ptr(MEM))?,
                        (Ins::Ftst, _) => a.ftst()?,
                        (Ins::Fcomi, _) => a.fcomi(st0, st1)?,
                        (Ins::Fucomip, _) => a.fucomip(st0, st1)?,
                    }
                    a.pushfd()?;
                    a.pop(dword_ptr(out + 4))?;
                    a.fnstsw(word_ptr(out))?;
                }
                a.hlt()
            }),
        );
    };
    let mut a = Rig::new();
    let mut b = Rig::new();
    a.cpu.core = CoreMode::Normal;
    b.cpu.core = CoreMode::Dynamic;
    for rig in [&mut a, &mut b] {
        setup(rig);
        rig.enter_pm();
    }
    lockstep_with(&mut a.cpu, &mut b.cpu, 4000, 997, true, |_, _| {}).unwrap();
    let eip = |rig: &Rig| rig.cpu.seg_cache(Seg::CS).base.wrapping_add(rig.cpu.eip());
    assert_eq!(
        a.cpu.bus.read_8(eip(&a) as usize),
        0xF4,
        "the interpreter didn't reach the HLT"
    );
    if AVAILABLE {
        assert!(b.cpu.dynrec.stats().runs > 0, "no translated code ran");
    }
    (a, b)
}

#[test]
fn compares_set_c1_ie_and_sf_as_the_sdm_says_on_both_cores() {
    let (a, b) = run_cases();
    let mut wrong = vec![];
    for (k, &(control, ins, first, other)) in all_cases().iter().enumerate() {
        let out = OUTPUT + 16 * k as u32;
        for (core, rig) in [("interpreter", &a), ("recompiler", &b)] {
            let sw = rig.read32(out) as u16;
            let eflags = rig.read32(out + 4);
            // Loaded: one register for FTST and the memory forms, two for
            // the others; TOP after them and the pops.
            let loaded = if matches!(
                ins,
                Ins::FcomMem | Ins::FcompMem | Ins::Ficom | Ins::Ficomp | Ins::Ftst
            ) {
                1
            } else {
                2 - (other == Op::Empty) as u16
            };
            let top = (8 - loaded + ins.pops()) & 7;
            if control == CONTROLS[0] {
                // IE masked: all of it.
                let want = expected(ins, first, other) | top << 11;
                if sw & (0x3800 | UNORDERED | C1 | IE | SF) != want {
                    wrong.push(format!(
                        "{core}: {ins:?} {first:?} {other:?}: status {sw:04X}, want {want:04X}"
                    ));
                }
                if ins.eflags() && eflags & 0x45 != expected_eflags(ins, first, other) {
                    wrong.push(format!(
                        "{core}: {ins:?} {first:?} {other:?}: EFLAGS {eflags:08X}"
                    ));
                }
            } else {
                // IE unmasked: the invalid exception is flagged as when
                // masked. (The rest of what the FPU does then, leaving the
                // condition codes and the stack and raising #MF or IRQ 13,
                // the emulator doesn't do for any instruction: #914.)
                let want = expected(ins, first, other) & (IE | SF);
                if sw & (IE | SF) != want {
                    wrong.push(format!(
                        "{core}: unmasked {ins:?} {first:?} {other:?}: status {sw:04X}"
                    ));
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}
