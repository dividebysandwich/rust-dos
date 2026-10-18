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

/// When rounding an FPU register's double (itself the 80 bits rounded to
/// nearest) could give another result than rounding the 80 bits: rounding
/// to nearest is monotonic and keeps every double, so a second rounding of
/// the double comes out as one of the 80 bits unless the double is a
/// boundary of the second rounding (`Uop::FBailPrecise`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precise {
    /// To a double, where the control word rounds other than to nearest.
    Directed,
    /// To an integer, where the double in x is one or halfway between two
    /// (or is 2^52 or more, or not finite).
    Int(X),
    /// To a single, where the double in x has 25 significant bits or fewer
    /// (each single, and each value halfway between two), or the control
    /// word rounds other than to nearest (a single from the double rounds
    /// to nearest, as `data::single_bits` has it).
    Single(X),
    /// Compared, where the doubles in the two are equal.
    Equal(X, X),
}

pub const X0: X = X(0);
pub const X1: X = X(1);

/// An MMX or SSE register: MMn (the significand of physical FPU register
/// n) or XMMn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Vr {
    Mm(u8),
    Xmm(u8),
}

/// What `Uop::VLoad` puts in a host register, zero-extended to 128 bits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VSrc {
    /// The register's 64 or 128 bits.
    R(Vr),
    /// A dword.
    T(T),
    /// The `size` bytes (8 or 16) at handle m, which is RAM's
    /// (`BailUnlessRam`).
    Mem { m: T, size: u8 },
}

/// Operations on two host registers' bits that leave MXCSR alone
/// (`Uop::VOp`): a = a OP b, as the SSE2 instruction of the name does it
/// on 128 bits, but where said. An MMX instruction's operands are in the
/// low quadwords, and only its result's low quadword counts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VOp {
    Andps,
    Andnps,
    Orps,
    Xorps,
    Unpcklps,
    Unpckhps,
    Shufps(u8),
    /// MOVSS between registers: b's low single into a.
    Movss,
    /// b's low quadword into a's (MOVLPS from memory).
    Movsd,
    Movlhps,
    Movhlps,
    Paddb,
    Paddw,
    Paddd,
    Paddsb,
    Paddsw,
    Paddusb,
    Paddusw,
    Psubb,
    Psubw,
    Psubd,
    Psubsb,
    Psubsw,
    Psubusb,
    Psubusw,
    Pmullw,
    Pmulhw,
    Pmulhuw,
    Pmaddwd,
    Pcmpeqb,
    Pcmpeqw,
    Pcmpeqd,
    Pcmpgtb,
    Pcmpgtw,
    Pcmpgtd,
    Pand,
    Pandn,
    Por,
    Pxor,
    Pavgb,
    Pavgw,
    Pminub,
    Pmaxub,
    Pminsw,
    Pmaxsw,
    Psadbw,
    /// Shifts by b's low quadword.
    Psllw,
    Pslld,
    Psllq,
    Psrlw,
    Psrld,
    Psrlq,
    Psraw,
    Psrad,
    /// Shifts of a by a constant (b unused).
    ShiftImm(VShift, u8),
    /// The MMX packs: a's low quadword's lanes, then b's, each saturated
    /// into a lane half as wide.
    Packsswb,
    Packssdw,
    Packuswb,
    /// The MMX unpacks: a's and b's low (or `high`) dwords' lanes,
    /// interleaved.
    Punpcklbw,
    Punpcklwd,
    Punpckldq,
    Punpckhbw,
    Punpckhwd,
    Punpckhdq,
    /// PSHUFW: a's low four words, from b's as the constant names them.
    Pshufw(u8),
}

/// The shift of `VOp::ShiftImm`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VShift {
    Psllw,
    Pslld,
    Psllq,
    Psrlw,
    Psrld,
    Psrlq,
    Psraw,
    Psrad,
}

/// Floating-point operations on two host registers (`Uop::VFloat`): a = a
/// OP b, as the SSE instruction does it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VFloatOp {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
    Sqrt,
    /// CMPPS and CMPSS with predicate 0 to 7.
    Cmp(u8),
    /// CVTPS2PI and CVTTPS2PI (`truncate`): b's low two singles into
    /// dwords in a's low quadword (`scalar` is false).
    ToInt { truncate: bool },
    /// CVTPI2PS: the two dwords in b's low quadword into a's low two
    /// singles (`scalar` is false).
    FromInt,
}

/// What `Uop::VToT` takes from a host register.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VExtract {
    /// The low dword.
    Low,
    /// The sign bits of the four singles (MOVMSKPS).
    Movmskps,
    /// The top bits of the low quadword's bytes (PMOVMSKB).
    Pmovmskb,
    /// Word n (0 to 3), zero-extended (PEXTRW).
    Word(u8),
}

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

/// The flags POPF and POPFD load at CPL 0 (`Uop::LoadFlags`): CF, PF, AF,
/// ZF, SF, TF, IF, DF, OF, IOPL and NT, as `Cpu::load_flags_pm` has them.
pub const POPF_RING0: u32 = 0x7FD5;
/// The flags a translated POPFD leaves as they were: AC and ID.
pub const POPF_KEPT: u32 = 0x24_0000;

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
    /// Reverse the bytes of dword t, without flags (BSWAP).
    Bswap { t: T },
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
    /// The instruction's handler runs it instead of the operations after
    /// this one unless handle t (a `MemRef`'s) is RAM's: an operand of
    /// plain RAM within a page. Nothing before it may change anything.
    BailUnlessRam { t: T },
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
    /// The arithmetic flags in `mask` = those bits of t, the others as
    /// they were (SAHF).
    FlagsFrom { t: T, mask: u32 },
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
    /// #GP(0) in virtual-8086 mode if IOPL is below 3 (PUSHF, POPF).
    CheckV86Iopl,
    /// t = the flags as PUSHF pushes them: their low word (`size` 2), or
    /// all but VM and RF.
    GetFlags { t: T, size: u8 },
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
    /// RETF in protected mode to CS:EIP `sel:off`, read from the stack
    /// (their accesses checked), through `helpers::jit_far_ret`, which
    /// loads CS and EIP; the handler's after all where that can't (the
    /// block stops after it). The stack pointer is the code's to set, then
    /// `FarExit` leaves the block.
    FarRetPm { sel: T, off: T },
    /// A far CALL in protected mode to `sel:off`: its checks before the
    /// pushes (`helpers::jit_far_call`), the handler's after all where it
    /// isn't a direct call (the block stops after it); then CS and EIP
    /// loaded after the pushes.
    FarCallCheck { sel: T, off: T },
    /// `next`: the return address, for the profiler's note of a thunk.
    FarCallLoad { next: u32 },
    /// Leave the block after `FarRetPm` or `FarCallLoad`: through the links a return takes,
    /// or back to the execution loop where `jit_far_ret` said so.
    FarExit,
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
    /// POPF or POPFD's flags from t, a `size` byte value, as
    /// `Cpu::load_flags_pm` loads them at CPL 0: the arithmetic flags, TF,
    /// DF, NT, IF and IOPL. The instruction's handler runs it instead of the
    /// operations after this one where CPL isn't 0 (or the CPU is in
    /// virtual-8086 mode), or a POPFD would change AC or ID (as CPUID's
    /// detection does). Nothing before it may change anything.
    LoadFlags { t: T, size: u8 },
    /// After `LoadFlags` and the stack pointer: the block stops after the
    /// instruction where TF is set (its trap comes from the execution loop)
    /// or IF is set with an interrupt waiting, as `jit_fallback` stops it.
    FlagsLoaded,
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
    /// The handler runs the instruction instead of the operations after
    /// this one where one of the registers ST(i) with bit i set in `regs`
    /// holds more than its double (`FpuRegs::beyond_double`), which the
    /// handler then rounds from the 80 bits, and `when` says that rounding
    /// the double instead could come out different. Nothing before it may
    /// change anything. The block stops after it, as after an `FpuGuard`'s.
    FBailPrecise { regs: u8, when: Precise },
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
    /// Exchange ST(0) and ST(i), and clear C1; with `fast`, only their
    /// doubles, which leaves both stale (fast mode's FXCH).
    FXch { i: u8, fast: bool },
    /// x = the single or the signed dword in t.
    FFromT { x: X, t: T, kind: FKind },
    /// x = the double at handle m, which is RAM's (`BailUnlessRam`); with
    /// `canon` as `FSet` makes one (the handlers' `canon_f64` of an operand).
    FLoad64 { x: X, m: T, canon: bool },
    /// x = the signed qword at handle m, which is RAM's (`BailUnlessRam`),
    /// where the double holds it exactly (below 2^53 either way); else the
    /// instruction's handler runs it instead of the operations after this
    /// one, as `Bail`.
    FLoadInt64 { x: X, m: T },
    /// t = the low dword of x's bits, or the high one with `high`.
    FToHalf { t: T, x: X, high: bool },
    /// t = x as a single's bits.
    FToSingle { t: T, x: X },
    /// t = dword `part` (0 or 1) of ST(0)'s 80 bits, or (2) its sign and
    /// exponent, as `FpuRegs::get` makes them.
    FToX80 { t: T, part: u8 },
    /// t = x as a word or dword (`size` 2 or 4), rounded as the control
    /// word says (`fpu::data::to_int`).
    FToInt { t: T, x: X, size: u8 },
    /// lo and hi = the low and high dwords of x as a qword, where the
    /// control word rounds to nearest or chops and the value fits; else
    /// the instruction's handler runs it instead of the operations after
    /// this one, as `Bail` (nothing before it may change anything).
    FToInt64 { lo: T, hi: T, x: X },
    /// a *= b.
    FMul { a: X, b: X },
    /// a += b, or -= with `sub` (fast mode's `arithmetic::sum`).
    FAdd { a: X, b: X, sub: bool },
    /// ST(i) = num / den, a divisor of 0 then finished by
    /// `fpu::arithmetic::divided_by_zero`.
    FDiv { i: u8, num: X, den: X },
    /// ST(dst) = ST(a) + ST(b), or - with `sub`, as their 80 bits add
    /// (`fpu::arithmetic::addsub_st`).
    FAddSt { dst: u8, a: u8, b: u8, sub: bool },
    /// ST(0) and x: `fpu::arithmetic::addsub_value` of `kind`.
    FAddValue { kind: u32, x: X },
    /// FCHS: `fpu::arithmetic::fchs`.
    FChs,
    /// Push the 80 bits at handle m, which is RAM's (`BailUnlessRam`), as
    /// FLD's handler does.
    FLoad80 { m: T },
    /// Compare a with b into C0, C2 and C3.
    FCom { a: X, b: X },
    /// t = the status word.
    FStatus { t: T },
    /// t = the control word, and the control word = t's low word.
    FGetControl { t: T },
    FSetControl { t: T },
    /// The start of an MMX or SSE instruction: its handler runs it instead
    /// of the operations after this one where CR0's EM or TS is set, or
    /// with `xmm` CR4's OSFXSR is clear, or one of MMn with bit n set in
    /// `mm` is stale (`f80::FpuRegs`: its 80 bits are made from its double,
    /// which the handler does). The block stops after it, as after an
    /// `FpuGuard`'s.
    SimdGuard { xmm: bool, mm: u8 },
    /// Before the `MemRef` of a 16-byte operand that must be aligned: the
    /// handler runs the instruction instead where segment `seg`'s base plus
    /// offset t isn't a multiple of 16 (#GP(0)). Nothing before it may
    /// change anything.
    BailMisaligned { t: T, seg: Seg },
    /// The handler runs the instruction instead unless handle t is one
    /// `VStore` takes: RAM of a block without code (not one in
    /// `Bus::code_blocks`), or an operand within one page that isn't
    /// plain RAM (`DEV_BIT`), such as the linear frame buffer. Nothing
    /// before it may change anything.
    BailUnlessStorable { t: T },
    /// x = the source, zero-extended to 128 bits.
    VLoad { x: X, src: VSrc },
    /// a = a OP b.
    VOp { op: VOp, a: X, b: X },
    /// a = a OP b, on all four singles or the low one (`scalar`), rounded
    /// and flushed as MXCSR says, with the exceptions raised added to its
    /// flags. Where one of them isn't masked, the handler runs the
    /// instruction instead of the operations after this one (it raises
    /// #XM), as `Bail`: nothing before it may change anything. So does
    /// ADD, SUB, MUL or DIV with underflow unmasked.
    VFloat { op: VFloatOp, scalar: bool, a: X, b: X },
    /// t = what `op` takes from x.
    VToT { t: T, x: X, op: VExtract },
    /// The register = x: all of it for XMMn; for MMn, the low quadword, as
    /// `mmx::set_mm` sets it.
    VSet { dst: Vr, x: X },
    /// Store x's low `size` bytes (8 or 16) at handle m, which
    /// `BailUnlessStorable` let through: into RAM as they are, or with
    /// `DEV_BIT` a dword at a time from the lowest, as the handlers write
    /// them (`Cpu::lin_write_32` each), through `jit_dev_write`.
    VStore { m: T, x: X, size: u8 },
    /// What every MMX instruction but EMMS does to the FPU: the stack top
    /// 0, all registers valid (`mmx::enter_mmx`).
    EnterMmx,
    /// EMMS: all FPU registers empty.
    Emms,
}
