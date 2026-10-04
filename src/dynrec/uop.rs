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

/// A host register holding a double while an FPU instruction runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct X(pub u8);

pub const X0: X = X(0);
pub const X1: X = X(1);

/// What `Uop::FFromT` makes a double of, and `Uop::FToT` of a double.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FKind {
    /// A single's bits.
    Single,
    /// A signed dword.
    Int,
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

/// What BT, BTS, BTR and BTC do with the bit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BitKind {
    Test,
    Set,
    Reset,
    Complement,
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
    /// t = the `size` bytes of RAM at physical address `phys`, zero- or
    /// sign-extended: an immediate read from the instruction's own bytes,
    /// which the program changes (see `translate::live_immediate`).
    LoadCode { t: T, phys: u32, size: u8, signed: bool },
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
    /// Shift or rotate by register `count` (CL) & 31: if that is 0, nothing
    /// changes and the instruction ends here, without the operations after
    /// this one. For bytes and words, a `Bail` has made it less than the
    /// width.
    ShiftVar { op: ShiftOp, size: u8, t: T, count: Gpr },
    /// SHLD or SHRD by register `count` & 31, as `ShiftVar` and
    /// `DoubleShift`.
    DoubleShiftVar { left: bool, size: u8, dst: T, src: T, count: Gpr },
    /// If t & `mask` isn't 0, the instruction's handler runs it instead of
    /// the operations after this one (a case they don't cover). Nothing
    /// before it may change anything, or fault.
    Bail { t: T, mask: u32 },
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
    /// t = 1 if condition `cc` holds on the flags, else 0 (SETcc).
    SetCond { t: T, cc: ConditionCode },
    /// BT, BTS, BTR or BTC of bit `bit` (modulo size * 8) of t, a `size`
    /// byte value: CF the bit and OF as `logic::bit_test` sets them (the
    /// others stay), and t the value with the bit set, cleared or
    /// complemented.
    BitOp { op: BitKind, size: u8, t: T, bit: Src },
    /// #GP(0) if the value is past the CS limit (a near jump's target).
    CheckLimit { src: Src },
    /// #GP(0) in protected mode if CPL is above IOPL (CLI).
    CheckIopl,
    /// t = the segment register's selector.
    GetSeg { t: T, seg: Seg },
    /// In real mode, before a far transfer: the instruction's handler runs
    /// it instead unless loading CS changes only its selector and base, as
    /// `Cpu::load_seg_real` does once CS is a plain data-like segment
    /// (that isn't flat, which a load could make it, changing the block's
    /// environment).
    CsReal,
    /// Load CS with the selector in t as `Cpu::load_seg_real` does, after
    /// `CsReal`, noting the block and the CS base it ran under for the
    /// engine (`JitCtx::far_block`).
    LoadCsReal { t: T },
    /// Keep t in the context's `slot` (0 or 1), and get it back: a value
    /// an instruction needs past operations that use all temporaries.
    Spill { t: T, slot: u8 },
    Unspill { t: T, slot: u8 },
    /// Leave the block with EIP = the value.
    Exit { eip: Src },
    /// Load segment register `seg` (not CS or SS) with the selector in t,
    /// as `Cpu::load_segment` does (it may fault).
    LoadSeg { seg: Seg, t: T },
    /// IN of `size` bytes from the port into t, or OUT of t, with the
    /// instruction count up to date: the I/O permission may fault, and the
    /// block stops after the instruction where the port access changed what
    /// the execution loop checks (see `helpers::jit_port`).
    In { size: u8, port: Src, t: T },
    Out { size: u8, port: Src, t: T },
    /// STI after its `CheckIopl`: IF set, the interrupt shadow where it was
    /// clear, and the block stopped after it where an interrupt waits.
    Sti,
    /// Leave the block at `taken` if the condition holds (#GP(0) if that
    /// is past the CS limit), else at `next`; either way first set `commit`
    /// (LOOP's counter).
    ExitIf { cond: Cond, taken: u32, next: u32, commit: Option<(Gpr, T)> },
    /// The start of a REP MOVS or STOS: t = the `count` register. Where DF
    /// or TF is set (iterations going down, or a trap after each) or the
    /// count is above `max`, the instruction's handler runs it instead, all
    /// at once; with a count of 0 the instruction ends here. The operations
    /// up to `RepEnd` are an iteration, which leaves the count left in t.
    RepStart { t: T, count: Gpr, max: u32 },
    /// Back to the iteration after `RepStart` while t isn't 0.
    RepEnd { t: T },
    /// MOVS or STOS without REP: where DF is set, the instruction's handler
    /// runs it instead of the operations after this one.
    Forward,
    /// The start of an FPU instruction: its handler runs it instead of the
    /// operations after this one where CR0's EM or TS is set (#NM), or one
    /// of the registers ST(i) with bit i set in `valid` is empty (it then
    /// reads as the real indefinite). The operations below read and write
    /// the registers as doubles (`f80::FpuRegs`), as the handlers do.
    FpuGuard { valid: u8 },
    /// x = ST(i).
    FGet { x: X, i: u8 },
    /// ST(i) = x; with `canon`, x may be a denormal or a signalling NaN
    /// (`f80::canon_f64` makes it what a register holds).
    FSet { i: u8, x: X, canon: bool },
    /// Push x, as `FSet` has it.
    FPush { x: X, canon: bool },
    /// Pop `n` (1 or 2) registers.
    FPop { n: u8 },
    /// ST(dst) = ST(src), or with no `dst` push ST(src): all a register
    /// holds.
    FCopy { dst: Option<u8>, src: u8 },
    /// Exchange ST(0) and ST(i), and clear C1.
    FXch { i: u8 },
    /// x = the single or the signed dword in t.
    FFromT { x: X, t: T, kind: FKind },
    /// t = x as a single's bits.
    FToSingle { t: T, x: X },
    /// t = x as a word or dword (`size` 2 or 4), rounded as the control
    /// word says (`fpu::data::to_int`).
    FToInt { t: T, x: X, size: u8 },
    /// a *= b.
    FMul { a: X, b: X },
    /// ST(i) = num / den, or with a divisor of 0 the real indefinite, and
    /// ZE set with `ze` (`fpu::arithmetic::divided_by_zero`).
    FDiv { i: u8, num: X, den: X, ze: bool },
    /// ST(dst) = ST(a) + ST(b), or - with `sub`, as their 80 bits add
    /// (`fpu::arithmetic::addsub_st`).
    FAddSt { dst: u8, a: u8, b: u8, sub: bool },
    /// ST(0) and x: `fpu::arithmetic::addsub_value` of `kind`.
    FAddValue { kind: u32, x: X },
    /// Compare a with b into C0, C2 and C3.
    FCom { a: X, b: X },
    /// t = the status word.
    FStatus { t: T },
    /// t = the control word, and the control word = t's low word.
    FGetControl { t: T },
    FSetControl { t: T },
}
