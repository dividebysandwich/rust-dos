//! The operations translated instructions are made of, which each code
//! generator turns into host code. They mean what the interpreter's
//! handlers do, flags included (see `cpu::alu`), and an instruction checks
//! everything that can fault before it changes anything, as the handlers
//! do: a MemRef or CheckLimit never follows a Set, Store or flag change.

use iced_x86::ConditionCode;

use crate::cpu::Seg;
use crate::cpu::alu::ShiftOp;

/// A host register holding a 32-bit value while an instruction runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct T(pub u8);

pub const T0: T = T(0);
pub const T1: T = T(1);
pub const T2: T = T(2);

/// A general-purpose register: its slot (EAX..EDI), whether it is the
/// high byte of the slot's low word (AH..BH), and its size.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Gpr {
    pub index: u8,
    pub high: bool,
    pub size: u8,
}

impl Gpr {
    pub const fn dword(index: u8) -> Gpr {
        Gpr { index, high: false, size: 4 }
    }

    pub const fn word(index: u8) -> Gpr {
        Gpr { index, high: false, size: 2 }
    }
}

pub const ECX: u8 = 1;
pub const ESP: u8 = 4;

/// A second operand: a register value or a constant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Src {
    T(T),
    Imm(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AluOp {
    Add,
    Or,
    Adc,
    Sbb,
    And,
    Sub,
    Xor,
    Cmp,
    Test,
}

impl AluOp {
    /// CMP and TEST only set the flags.
    pub fn writes(self) -> bool {
        !matches!(self, AluOp::Cmp | AluOp::Test)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Inc,
    Dec,
    Neg,
    Not,
}

/// When a conditional exit is taken.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cond {
    /// A condition code on the flags.
    Flags(ConditionCode),
    /// The register is 0 (JCXZ), not 0 (LOOP), or not 0 with ZF set
    /// (LOOPE) or clear (LOOPNE).
    Zero(T),
    NonZero(T),
    NonZeroZf(T, bool),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Uop {
    /// t = the register, zero-extended.
    Get { t: T, r: Gpr },
    /// The register = the low bytes of t (the rest of its slot stays).
    Set { r: Gpr, t: T },
    Const { t: T, v: u32 },
    Copy { dst: T, src: T },
    /// t += v, wrapped to `size` bytes (2 or 4), without flags.
    AddConst { t: T, v: u32, size: u8 },
    /// t >>= count, arithmetically, without flags.
    SarConst { t: T, count: u8 },
    /// Zero- or sign-extend t's low `from` bytes to 32 bits.
    Extend { t: T, from: u8, signed: bool },
    /// t = the offset of a memory operand: disp + base + index * scale,
    /// wrapped to 16 bits unless `a32`.
    Ea { t: T, base: Option<Gpr>, index: Option<Gpr>, scale: u8, disp: u32, a32: bool },
    /// Check an access of `size` bytes at seg:t, as `Cpu::mem_ref` does
    /// (it may fault); t then stands for the operand in Load and Store.
    /// `slot` (0..4) tells operands of one instruction apart.
    MemRef { t: T, seg: Seg, size: u8, write: bool, slot: u8 },
    Load { dst: T, m: T, size: u8 },
    Store { m: T, src: T, size: u8 },
    /// a = a OP b, and the flags as the interpreter's ALU sets them.
    Alu { op: AluOp, size: u8, a: T, b: Src },
    Unary { op: UnOp, size: u8, t: T },
    /// Shift or rotate by a count of 1 to size * 8 - 1.
    Shift { op: ShiftOp, size: u8, t: T, count: u8 },
    /// SHLD (`left`) or SHRD of `dst` by a count of 1 to size * 8 - 1,
    /// filling in from `src`.
    DoubleShift { left: bool, size: u8, dst: T, src: T, count: u8 },
    /// a = a * b, signed, cut to `size` (2 or 4) bytes: the two- and
    /// three-operand IMUL. Only CF and OF change.
    Imul { size: u8, a: T, b: Src },
    /// MUL or IMUL (`signed`) of AL, AX or EAX by t, into AX, DX:AX or
    /// EDX:EAX. Only CF and OF change.
    MulWide { signed: bool, size: u8, t: T },
    /// DIV or IDIV (`signed`) of AX, DX:AX or EDX:EAX by t: the quotient
    /// into AL, AX or EAX and the remainder into AH, DX or EDX, or #DE
    /// before anything changes if t is 0 or the quotient doesn't fit. The
    /// flags stay, as the interpreter leaves them.
    DivWide { signed: bool, size: u8, t: T },
    /// Set (Some(true)), clear or complement (None) the flags in `mask`.
    Flag { mask: u32, set: Option<bool> },
    /// #GP(0) if the value is past the CS limit (a near jump's target).
    CheckLimit { src: Src },
    /// Leave the block with EIP = the value.
    Exit { eip: Src },
    /// Leave the block at `taken` if the condition holds (#GP(0) if that
    /// is past the CS limit), else at `next`; either way first set `commit`
    /// (LOOP's counter).
    ExitIf { cond: Cond, taken: u32, next: u32, commit: Option<(Gpr, T)> },
}
