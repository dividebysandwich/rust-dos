//! Instructions into operations (`uop`). Each translation does what the
//! instruction's interpreter handler does, in the same order: the forms
//! here are the common ones, and anything else runs through its handler.

use iced_x86::{Code, ConditionCode, Instruction, Mnemonic, OpKind, Register};

use super::uop::*;
use crate::cpu::Seg;
use crate::cpu::alu::ShiftOp;
use crate::instructions::operand::{addr_size, mem_seg};

/// Flags bits the flag instructions change.
const CF: u32 = 0x0001;
const IF: u32 = 0x0200;
const DF: u32 = 0x0400;

/// The operations for `instr`, or None if it runs through its handler.
/// `next` is the EIP after it (which, as the interpreter has it, doesn't
/// wrap in 16-bit code), and `stack32` the stack's width (SS's B flag),
/// which blocks are translated for. With `system`, the code generator has
/// the operations of segment loads, port I/O, STI and REP string loops.
/// `real`: the block runs in real mode, where far transfers are translated.
/// With `segments`, the code generator has the loads of data segment
/// registers in protected mode too (in real mode it always has them).
pub fn translate(instr: &Instruction, next: u32, stack32: bool, system: bool, segments: bool, fpu: bool, real: bool) -> Option<Vec<Uop>> {
    use Mnemonic::*;
    let mut u = Vec::with_capacity(8);
    let ok = match instr.mnemonic() {
        Mov if (segments || real) && instr.op0_kind() == OpKind::Register && instr.op0_register().is_segment_register() => {
            mov_to_seg(instr, &mut u)
        }
        Pop if (segments || real) && instr.op0_kind() == OpKind::Register && instr.op0_register().is_segment_register() => {
            pop_seg(instr, stack32, &mut u)
        }
        Les | Lds | Lfs | Lgs if segments || real => far_pointer(instr, &mut u),
        In if system => port_in(instr, &mut u),
        Out if system => port_out(instr, &mut u),
        Sti if system => {
            u.push(Uop::CheckIopl);
            u.push(Uop::Sti);
            true
        }
        Movsb | Movsw | Movsd | Stosb | Stosw | Stosd if system => string(instr, &mut u),
        Lodsb | Lodsw | Lodsd if system => lods(instr, &mut u),
        Rcl if system => rotate_carry(instr, ShiftOp::Rcl, &mut u),
        Rcr if system => rotate_carry(instr, ShiftOp::Rcr, &mut u),
        Enter => enter(instr, stack32, &mut u),
        Leave => leave(instr, stack32, &mut u),
        Mov => mov(instr, &mut u),
        Add => alu(instr, AluOp::Add, &mut u),
        Or => alu(instr, AluOp::Or, &mut u),
        Adc => alu(instr, AluOp::Adc, &mut u),
        Sbb => alu(instr, AluOp::Sbb, &mut u),
        And => alu(instr, AluOp::And, &mut u),
        Sub => alu(instr, AluOp::Sub, &mut u),
        Xor => alu(instr, AluOp::Xor, &mut u),
        Cmp => alu(instr, AluOp::Cmp, &mut u),
        Test => alu(instr, AluOp::Test, &mut u),
        Inc => unary(instr, UnOp::Inc, &mut u),
        Dec => unary(instr, UnOp::Dec, &mut u),
        Neg => unary(instr, UnOp::Neg, &mut u),
        Not => unary(instr, UnOp::Not, &mut u),
        Lea => lea(instr, &mut u),
        Movzx => movx(instr, false, &mut u),
        Movsx => movx(instr, true, &mut u),
        Xchg => xchg(instr, &mut u),
        Cbw => extend_acc(1, false, &mut u),
        Cwde => extend_acc(2, false, &mut u),
        Cwd => extend_acc(2, true, &mut u),
        Cdq => extend_acc(4, true, &mut u),
        Clc => flag(CF, Some(false), &mut u),
        Stc => flag(CF, Some(true), &mut u),
        Cmc => flag(CF, None, &mut u),
        Cld => flag(DF, Some(false), &mut u),
        Cli => {
            // (Clearing IF makes no interrupt deliverable: the block goes on.)
            u.push(Uop::CheckIopl);
            flag(IF, Some(false), &mut u)
        }
        Std => flag(DF, Some(true), &mut u),
        Nop => instr.op_count() == 0,
        Imul => imul(instr, &mut u),
        Mul => mul_wide(instr, false, &mut u),
        Div => div_wide(instr, false, &mut u),
        Idiv => div_wide(instr, true, &mut u),
        Shld => double_shift(instr, true, &mut u),
        Shrd => double_shift(instr, false, &mut u),
        Shl | Sal => shift(instr, ShiftOp::Shl, &mut u),
        Shr => shift(instr, ShiftOp::Shr, &mut u),
        Sar => shift(instr, ShiftOp::Sar, &mut u),
        Rol => shift(instr, ShiftOp::Rol, &mut u),
        Ror => shift(instr, ShiftOp::Ror, &mut u),
        Push => push(instr, stack32, &mut u),
        Pop => pop(instr, stack32, &mut u),
        Pusha | Pushad => pusha(instr, stack32, &mut u),
        Popa | Popad => popa(instr, stack32, &mut u),
        Pushf | Pushfd => pushf(instr, stack32, &mut u),
        Bt => bit_op(instr, BitKind::Test, &mut u),
        Bts => bit_op(instr, BitKind::Set, &mut u),
        Btr => bit_op(instr, BitKind::Reset, &mut u),
        Btc => bit_op(instr, BitKind::Complement, &mut u),
        Jmp if instr.is_jmp_far() || instr.is_jmp_far_indirect() => real && far_jump(instr, next, stack32, false, &mut u),
        Call if instr.is_call_far() || instr.is_call_far_indirect() => real && far_jump(instr, next, stack32, true, &mut u),
        Retf => real && far_ret(instr, stack32, &mut u),
        Jmp => jmp(instr, &mut u),
        Jo | Jno | Jb | Jae | Je | Jne | Jbe | Ja | Js | Jns | Jp | Jnp | Jl | Jge | Jle | Jg => jcc(instr, next, &mut u),
        Seto | Setno | Setb | Setae | Sete | Setne | Setbe | Seta | Sets | Setns | Setp | Setnp | Setl | Setge
        | Setle | Setg => setcc(instr, &mut u),
        Loop | Loope | Loopne => loop_op(instr, next, &mut u),
        Jcxz | Jecxz => jcxz(instr, next, &mut u),
        Call => call(instr, next, stack32, &mut u),
        Ret => ret(instr, stack32, &mut u),
        _ => fpu && super::fpu::translate(instr, &mut u),
    };
    ok.then_some(u)
}

/// The general-purpose register `r` as an operand.
pub fn gpr(r: Register) -> Option<Gpr> {
    let n = r as u8;
    if (Register::AL as u8..=Register::BH as u8).contains(&n) {
        let i = n - Register::AL as u8;
        return Some(if i < 4 { Gpr { index: i, high: false, size: 1 } } else { Gpr { index: i - 4, high: true, size: 1 } });
    }
    if (Register::AX as u8..=Register::DI as u8).contains(&n) {
        return Some(Gpr::word(n - Register::AX as u8));
    }
    if (Register::EAX as u8..=Register::EDI as u8).contains(&n) {
        return Some(Gpr::dword(n - Register::EAX as u8));
    }
    None
}

/// The stack pointer as a stack of that width uses it.
fn sp(stack32: bool) -> Gpr {
    if stack32 { Gpr::dword(ESP) } else { Gpr::word(ESP) }
}

fn is_imm(kind: OpKind) -> bool {
    matches!(
        kind,
        OpKind::Immediate8 | OpKind::Immediate16 | OpKind::Immediate32 | OpKind::Immediate8to16 | OpKind::Immediate8to32
    )
}

fn size_mask(size: u8) -> u32 {
    match size {
        1 => 0xFF,
        2 => 0xFFFF,
        _ => 0xFFFF_FFFF,
    }
}

/// Emit the offset of the memory operand into `t`, and return its segment.
/// None for the SIB byte with no index but a scale, which means something
/// else on a 386 (see `operand::effective_offset`).
fn ea(instr: &Instruction, t: T, u: &mut Vec<Uop>) -> Option<Seg> {
    let (base, index) = (instr.memory_base(), instr.memory_index());
    let scale = instr.memory_index_scale() as u8;
    if index == Register::None && scale != 1 {
        return None;
    }
    let base = if base == Register::None { None } else { Some(gpr(base)?) };
    let index = if index == Register::None { None } else { Some(gpr(index)?) };
    let a32 = addr_size(instr) == 4;
    u.push(Uop::Ea { t, base, index, scale, disp: instr.memory_displacement32(), a32 });
    Some(mem_seg(instr))
}

/// Check the memory operand for an access of `size` bytes: `t` then
/// refers to it.
pub(super) fn mem(instr: &Instruction, t: T, size: u8, write: bool, u: &mut Vec<Uop>) -> Option<()> {
    let seg = ea(instr, t, u)?;
    u.push(Uop::MemRef { t, seg, size, write, slot: 0 });
    Some(())
}

/// Operand kinds of the two-operand forms, as `fast::form` has them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    RegReg,
    RegImm,
    RegMem,
    MemReg,
    MemImm,
}

fn form(instr: &Instruction) -> Option<(Form, u8)> {
    if instr.op_count() != 2 {
        return None;
    }
    let (k0, k1) = (instr.op0_kind(), instr.op1_kind());
    let size = match k0 {
        OpKind::Register => gpr(instr.op0_register())?.size,
        OpKind::Memory => instr.memory_size().size() as u8,
        _ => return None,
    };
    if !matches!(size, 1 | 2 | 4) {
        return None;
    }
    let form = match (k0, k1) {
        (OpKind::Register, OpKind::Register) => Form::RegReg,
        (OpKind::Register, OpKind::Memory) => Form::RegMem,
        (OpKind::Memory, OpKind::Register) => Form::MemReg,
        (OpKind::Register, k) if is_imm(k) => Form::RegImm,
        (OpKind::Memory, k) if is_imm(k) => Form::MemImm,
        _ => return None,
    };
    if k1 == OpKind::Register && gpr(instr.op1_register())?.size != size {
        return None;
    }
    Some((form, size))
}

/// The immediate second operand, cut to `size` bytes.
fn imm(instr: &Instruction, size: u8) -> u32 {
    instr.immediate(1) as u32 & size_mask(size)
}

fn mov(instr: &Instruction, u: &mut Vec<Uop>) -> bool {
    if instr.op_count() == 2
        && instr.op1_kind() == OpKind::Register
        && let Some(seg) = Seg::from_register(instr.op1_register())
    {
        return mov_from_seg(instr, seg, u);
    }
    let Some((form, size)) = form(instr) else { return false };
    let r0 = gpr(instr.op0_register());
    let r1 = gpr(instr.op1_register());
    match form {
        Form::RegReg => {
            u.push(Uop::Get { t: T0, r: r1.unwrap() });
            u.push(Uop::Set { r: r0.unwrap(), t: T0 });
        }
        Form::RegImm => {
            u.push(Uop::Const { t: T0, v: instr.immediate(1) as u32 });
            u.push(Uop::Set { r: r0.unwrap(), t: T0 });
        }
        Form::RegMem => {
            if mem(instr, T1, size, false, u).is_none() {
                return false;
            }
            u.push(Uop::Load { dst: T0, m: T1, size });
            u.push(Uop::Set { r: r0.unwrap(), t: T0 });
        }
        Form::MemReg => {
            if mem(instr, T1, size, true, u).is_none() {
                return false;
            }
            u.push(Uop::Get { t: T0, r: r1.unwrap() });
            u.push(Uop::Store { m: T1, src: T0, size });
        }
        Form::MemImm => {
            if mem(instr, T1, size, true, u).is_none() {
                return false;
            }
            u.push(Uop::Const { t: T0, v: imm(instr, size) });
            u.push(Uop::Store { m: T1, src: T0, size });
        }
    }
    true
}

fn alu(instr: &Instruction, op: AluOp, u: &mut Vec<Uop>) -> bool {
    let Some((form, size)) = form(instr) else { return false };
    let r0 = gpr(instr.op0_register());
    let r1 = gpr(instr.op1_register());
    match form {
        Form::RegReg => {
            u.push(Uop::Get { t: T0, r: r0.unwrap() });
            u.push(Uop::Get { t: T1, r: r1.unwrap() });
            u.push(Uop::Alu { op, size, a: T0, b: Src::T(T1) });
        }
        Form::RegImm => {
            u.push(Uop::Get { t: T0, r: r0.unwrap() });
            u.push(Uop::Alu { op, size, a: T0, b: Src::Imm(imm(instr, size)) });
        }
        Form::RegMem => {
            if mem(instr, T2, size, false, u).is_none() {
                return false;
            }
            u.push(Uop::Load { dst: T1, m: T2, size });
            u.push(Uop::Get { t: T0, r: r0.unwrap() });
            u.push(Uop::Alu { op, size, a: T0, b: Src::T(T1) });
        }
        Form::MemReg | Form::MemImm => {
            if mem(instr, T2, size, op.writes(), u).is_none() {
                return false;
            }
            u.push(Uop::Load { dst: T0, m: T2, size });
            let b = if form == Form::MemReg {
                u.push(Uop::Get { t: T1, r: r1.unwrap() });
                Src::T(T1)
            } else {
                Src::Imm(imm(instr, size))
            };
            u.push(Uop::Alu { op, size, a: T0, b });
            if op.writes() {
                u.push(Uop::Store { m: T2, src: T0, size });
            }
            return true;
        }
    }
    if op.writes() {
        u.push(Uop::Set { r: r0.unwrap(), t: T0 });
    }
    true
}

fn unary(instr: &Instruction, op: UnOp, u: &mut Vec<Uop>) -> bool {
    if instr.op_count() != 1 {
        return false;
    }
    match instr.op0_kind() {
        OpKind::Register => {
            let Some(r) = gpr(instr.op0_register()) else { return false };
            u.push(Uop::Get { t: T0, r });
            u.push(Uop::Unary { op, size: r.size, t: T0 });
            u.push(Uop::Set { r, t: T0 });
        }
        OpKind::Memory => {
            let size = instr.memory_size().size() as u8;
            if !matches!(size, 1 | 2 | 4) || mem(instr, T2, size, true, u).is_none() {
                return false;
            }
            u.push(Uop::Load { dst: T0, m: T2, size });
            u.push(Uop::Unary { op, size, t: T0 });
            u.push(Uop::Store { m: T2, src: T0, size });
        }
        _ => return false,
    }
    true
}

fn lea(instr: &Instruction, u: &mut Vec<Uop>) -> bool {
    if instr.op0_kind() != OpKind::Register || instr.op1_kind() != OpKind::Memory {
        return false;
    }
    let Some(r) = gpr(instr.op0_register()) else { return false };
    if r.size == 1 || ea(instr, T0, u).is_none() {
        return false;
    }
    u.push(Uop::Set { r, t: T0 });
    true
}

fn movx(instr: &Instruction, signed: bool, u: &mut Vec<Uop>) -> bool {
    let Some(dest) = gpr(instr.op0_register()) else { return false };
    let from = match instr.op1_kind() {
        OpKind::Register => {
            let Some(src) = gpr(instr.op1_register()) else { return false };
            u.push(Uop::Get { t: T0, r: src });
            src.size
        }
        OpKind::Memory => {
            let size = instr.memory_size().size() as u8;
            if !matches!(size, 1 | 2) || mem(instr, T1, size, false, u).is_none() {
                return false;
            }
            u.push(Uop::Load { dst: T0, m: T1, size });
            size
        }
        _ => return false,
    };
    if from >= dest.size {
        return false;
    }
    u.push(Uop::Extend { t: T0, from, signed });
    u.push(Uop::Set { r: dest, t: T0 });
    true
}

fn xchg(instr: &Instruction, u: &mut Vec<Uop>) -> bool {
    let reg_mem = match (instr.op0_kind(), instr.op1_kind()) {
        (OpKind::Memory, OpKind::Register) => Some(instr.op1_register()),
        (OpKind::Register, OpKind::Memory) => Some(instr.op0_register()),
        _ => None,
    };
    if let Some(reg) = reg_mem {
        // With memory: both read, then the memory and the register written.
        let Some(r) = gpr(reg) else { return false };
        if mem(instr, T2, r.size, true, u).is_none() {
            return false;
        }
        u.push(Uop::Load { dst: T0, m: T2, size: r.size });
        u.push(Uop::Get { t: T1, r });
        u.push(Uop::Store { m: T2, src: T1, size: r.size });
        u.push(Uop::Set { r, t: T0 });
        return true;
    }
    if instr.op0_kind() != OpKind::Register || instr.op1_kind() != OpKind::Register {
        return false;
    }
    let (Some(a), Some(b)) = (gpr(instr.op0_register()), gpr(instr.op1_register())) else { return false };
    if a.size != b.size {
        return false;
    }
    u.push(Uop::Get { t: T0, r: a });
    u.push(Uop::Get { t: T1, r: b });
    u.push(Uop::Set { r: a, t: T1 });
    u.push(Uop::Set { r: b, t: T0 });
    true
}

/// CBW and CWDE extend AL or AX in place (`from` 1 or 2, `high` false);
/// CWD and CDQ fill DX or EDX with the sign of AX or EAX (`from` 2 or 4).
fn extend_acc(from: u8, high: bool, u: &mut Vec<Uop>) -> bool {
    let src = Gpr { index: 0, high: false, size: from };
    u.push(Uop::Get { t: T0, r: src });
    if high {
        if from == 2 {
            u.push(Uop::Extend { t: T0, from: 2, signed: true });
        }
        u.push(Uop::SarConst { t: T0, count: 31 });
        u.push(Uop::Set { r: Gpr { index: 2, high: false, size: from }, t: T0 });
    } else {
        u.push(Uop::Extend { t: T0, from, signed: true });
        u.push(Uop::Set { r: Gpr { index: 0, high: false, size: from * 2 }, t: T0 });
    }
    true
}

fn flag(mask: u32, set: Option<bool>, u: &mut Vec<Uop>) -> bool {
    u.push(Uop::Flag { mask, set });
    true
}

/// CL, the count of the shifts by a register.
const CL: Gpr = Gpr { index: ECX, high: false, size: 1 };

/// Shifts and rotates of a register or memory by an immediate count from 1
/// to the width less 1, or by CL, which the host's instructions do as the
/// interpreter does. Counts by CL of the width or more (of a byte or word)
/// run through the handler.
fn shift(instr: &Instruction, op: ShiftOp, u: &mut Vec<Uop>) -> bool {
    let size = match instr.op0_kind() {
        OpKind::Register => match gpr(instr.op0_register()) {
            Some(r) => r.size,
            None => return false,
        },
        OpKind::Memory => instr.memory_size().size() as u8,
        _ => return false,
    };
    if !matches!(size, 1 | 2 | 4) {
        return false;
    }
    let shift = if is_imm(instr.op1_kind()) {
        let count = instr.immediate(1) as u32 & 0x1F;
        if count == 0 || count >= size as u32 * 8 {
            return false;
        }
        Uop::Shift { op, size, t: T0, count: count as u8 }
    } else if instr.op1_register() == Register::CL {
        if size < 4 {
            u.push(Uop::Get { t: T1, r: CL });
            u.push(Uop::Bail { t: T1, mask: 0x1F & !(size as u32 * 8 - 1) });
        }
        Uop::ShiftVar { op, size, t: T0, count: CL }
    } else {
        return false;
    };
    modify(instr, size, shift, u)
}

/// Operand 0 (a register or memory of `size` bytes) into T0, `op` on it,
/// and back.
fn modify(instr: &Instruction, size: u8, op: Uop, u: &mut Vec<Uop>) -> bool {
    match instr.op0_kind() {
        OpKind::Register => {
            let r = gpr(instr.op0_register()).unwrap();
            u.push(Uop::Get { t: T0, r });
            u.push(op);
            u.push(Uop::Set { r, t: T0 });
        }
        _ => {
            if mem(instr, T2, size, true, u).is_none() {
                return false;
            }
            u.push(Uop::Load { dst: T0, m: T2, size });
            u.push(op);
            u.push(Uop::Store { m: T2, src: T0, size });
        }
    }
    true
}

/// Operand `i` (a register or memory of `size` bytes) into T1, memory
/// checked for reading through T2.
fn source_t1(instr: &Instruction, i: u32, size: u8, u: &mut Vec<Uop>) -> Option<()> {
    match instr.op_kind(i) {
        OpKind::Register => {
            let r = gpr(instr.op_register(i))?;
            (r.size == size).then(|| u.push(Uop::Get { t: T1, r }))
        }
        OpKind::Memory => {
            mem(instr, T2, size, false, u)?;
            u.push(Uop::Load { dst: T1, m: T2, size });
            Some(())
        }
        _ => None,
    }
}

/// IMUL: the one-operand form widens as MUL does; the two- and
/// three-operand forms multiply into a register.
fn imul(instr: &Instruction, u: &mut Vec<Uop>) -> bool {
    if instr.op_count() == 1 {
        return mul_wide(instr, true, u);
    }
    let Some(dest) = gpr(instr.op0_register()) else { return false };
    if instr.op0_kind() != OpKind::Register || dest.size == 1 {
        return false;
    }
    let size = dest.size;
    if source_t1(instr, 1, size, u).is_none() {
        return false;
    }
    let b = if instr.op_count() == 2 {
        u.insert(0, Uop::Get { t: T0, r: dest });
        Src::T(T1)
    } else if is_imm(instr.op2_kind()) {
        u.push(Uop::Copy { dst: T0, src: T1 });
        Src::Imm(instr.immediate(2) as u32 & size_mask(size))
    } else {
        return false;
    };
    u.push(Uop::Imul { size, a: T0, b });
    u.push(Uop::Set { r: dest, t: T0 });
    true
}

/// The operand of MUL, DIV and the one-operand IMUL and IDIV into T1, and
/// its size.
fn wide_operand(instr: &Instruction, u: &mut Vec<Uop>) -> Option<u8> {
    if instr.op_count() != 1 {
        return None;
    }
    let size = match instr.op0_kind() {
        OpKind::Register => gpr(instr.op0_register())?.size,
        OpKind::Memory => instr.memory_size().size() as u8,
        _ => return None,
    };
    if !matches!(size, 1 | 2 | 4) {
        return None;
    }
    source_t1(instr, 0, size, u)?;
    Some(size)
}

/// MUL, and the one-operand IMUL.
fn mul_wide(instr: &Instruction, signed: bool, u: &mut Vec<Uop>) -> bool {
    let Some(size) = wide_operand(instr, u) else { return false };
    u.push(Uop::MulWide { signed, size, t: T1 });
    true
}

/// DIV and IDIV.
fn div_wide(instr: &Instruction, signed: bool, u: &mut Vec<Uop>) -> bool {
    let Some(size) = wide_operand(instr, u) else { return false };
    u.push(Uop::DivWide { signed, size, t: T1 });
    true
}

/// SHLD and SHRD by an immediate count below the operand's width, or by
/// CL (a 386 rotates the source in for 16-bit operands shifted by more:
/// by CL, those run through the handler).
fn double_shift(instr: &Instruction, left: bool, u: &mut Vec<Uop>) -> bool {
    if instr.op_count() != 3 || instr.op1_kind() != OpKind::Register {
        return false;
    }
    let Some(src) = gpr(instr.op1_register()) else { return false };
    let size = src.size;
    if size == 1 {
        return false;
    }
    let shift = if is_imm(instr.op2_kind()) {
        let count = instr.immediate(2) as u32 & 0x1F;
        if count == 0 || count >= size as u32 * 8 {
            return false;
        }
        Uop::DoubleShift { left, size, dst: T0, src: T1, count: count as u8 }
    } else if instr.op2_register() == Register::CL {
        if size == 2 {
            u.push(Uop::Get { t: T1, r: CL });
            u.push(Uop::Bail { t: T1, mask: 0x10 });
        }
        Uop::DoubleShiftVar { left, size, dst: T0, src: T1, count: CL }
    } else {
        return false;
    };
    match instr.op0_kind() {
        OpKind::Register => {
            let Some(dest) = gpr(instr.op0_register()) else { return false };
            if dest.size != size {
                return false;
            }
            u.push(Uop::Get { t: T0, r: dest });
            u.push(Uop::Get { t: T1, r: src });
            u.push(shift);
            u.push(Uop::Set { r: dest, t: T0 });
        }
        OpKind::Memory => {
            if mem(instr, T2, size, true, u).is_none() {
                return false;
            }
            u.push(Uop::Load { dst: T0, m: T2, size });
            u.push(Uop::Get { t: T1, r: src });
            u.push(shift);
            u.push(Uop::Store { m: T2, src: T0, size });
        }
        _ => return false,
    }
    true
}

/// Push T0 as `size` bytes: check the slot, write it, then move the stack
/// pointer, as `Cpu::push_sized` does.
fn push_t0(size: u8, stack32: bool, u: &mut Vec<Uop>) {
    let sp = sp(stack32);
    u.push(Uop::Get { t: T1, r: sp });
    u.push(Uop::AddConst { t: T1, v: (size as u32).wrapping_neg(), size: sp.size });
    u.push(Uop::Copy { dst: T2, src: T1 });
    u.push(Uop::MemRef { t: T2, seg: Seg::SS, size, write: true, slot: 0 });
    u.push(Uop::Store { m: T2, src: T0, size });
}

fn push(instr: &Instruction, stack32: bool, u: &mut Vec<Uop>) -> bool {
    if instr.op0_kind() == OpKind::Register && instr.op0_register().is_segment_register() {
        return push_seg(instr, stack32, u);
    }
    let size = match instr.code() {
        Code::Push_r16 | Code::Push_imm16 | Code::Pushw_imm8 | Code::Push_rm16 => 2,
        Code::Push_r32 | Code::Pushd_imm32 | Code::Pushd_imm8 | Code::Push_rm32 => 4,
        _ => return false,
    };
    if instr.op0_kind() == OpKind::Memory {
        // The operand first, its address with the stack pointer as it was.
        if mem(instr, T2, size, false, u).is_none() {
            return false;
        }
        u.push(Uop::Load { dst: T0, m: T2, size });
    } else if instr.op0_kind() == OpKind::Register {
        let Some(r) = gpr(instr.op0_register()) else { return false };
        u.push(Uop::Get { t: T0, r });
    } else {
        u.push(Uop::Const { t: T0, v: instr.immediate(0) as u32 & size_mask(size) });
    }
    push_t0(size, stack32, u);
    u.push(Uop::Set { r: sp(stack32), t: T1 });
    true
}

/// Read `size` bytes from the top of the stack into T0, and leave the
/// stack pointer past them in T1 (not set yet).
fn pop_t0(size: u8, stack32: bool, u: &mut Vec<Uop>) {
    let sp = sp(stack32);
    u.push(Uop::Get { t: T1, r: sp });
    u.push(Uop::Copy { dst: T2, src: T1 });
    u.push(Uop::MemRef { t: T2, seg: Seg::SS, size, write: false, slot: 0 });
    u.push(Uop::Load { dst: T0, m: T2, size });
    u.push(Uop::AddConst { t: T1, v: size as u32, size: sp.size });
}

fn pop(instr: &Instruction, stack32: bool, u: &mut Vec<Uop>) -> bool {
    let size = match instr.code() {
        Code::Pop_r16 | Code::Pop_rm16 => 2,
        Code::Pop_r32 | Code::Pop_rm32 => 4,
        _ => return false,
    };
    if instr.op0_kind() == OpKind::Memory {
        // Into memory addressed without ESP (with it, the address is that
        // after the pop, which the handler works out): the stack's top,
        // then the operand's checks, the store and the stack pointer.
        let esp = |r: Register| matches!(r, Register::ESP | Register::SP);
        if esp(instr.memory_base()) || esp(instr.memory_index()) {
            return false;
        }
        pop_t0(size, stack32, u);
        if mem(instr, T2, size, true, u).is_none() {
            return false;
        }
        u.push(Uop::Store { m: T2, src: T0, size });
        u.push(Uop::Set { r: sp(stack32), t: T1 });
        return true;
    }
    let Some(r) = gpr(instr.op0_register()) else { return false };
    pop_t0(size, stack32, u);
    // The stack pointer first: POP ESP loads the popped value.
    u.push(Uop::Set { r: sp(stack32), t: T1 });
    u.push(Uop::Set { r, t: T0 });
    true
}

/// MOV r/m16, Sreg: the selector into a word of memory, or a register,
/// zero-extended into a 32-bit one.
fn mov_from_seg(instr: &Instruction, seg: Seg, u: &mut Vec<Uop>) -> bool {
    match instr.op0_kind() {
        OpKind::Register => {
            let Some(r) = gpr(instr.op0_register()).filter(|r| r.size > 1) else { return false };
            u.push(Uop::GetSeg { t: T0, seg });
            u.push(Uop::Set { r, t: T0 });
        }
        OpKind::Memory => {
            if mem(instr, T1, 2, true, u).is_none() {
                return false;
            }
            u.push(Uop::GetSeg { t: T0, seg });
            u.push(Uop::Store { m: T1, src: T0, size: 2 });
        }
        _ => return false,
    }
    true
}

/// The segment register a MOV or POP loads, but CS and SS: CS can't be
/// loaded so, and SS ends the block.
fn loaded_seg(instr: &Instruction) -> Option<Seg> {
    Seg::from_register(instr.op0_register()).filter(|&s| s != Seg::CS && s != Seg::SS)
}

/// MOV Sreg, r/m16: the selector from a register's low word or memory.
fn mov_to_seg(instr: &Instruction, u: &mut Vec<Uop>) -> bool {
    let Some(seg) = loaded_seg(instr) else { return false };
    match instr.op1_kind() {
        OpKind::Register => {
            let Some(r) = gpr(instr.op1_register()).filter(|r| r.size > 1) else { return false };
            u.push(Uop::Get { t: T0, r: Gpr::word(r.index) });
        }
        OpKind::Memory => {
            if mem(instr, T1, 2, false, u).is_none() {
                return false;
            }
            u.push(Uop::Load { dst: T0, m: T1, size: 2 });
        }
        _ => return false,
    }
    u.push(Uop::LoadSeg { seg, t: T0 });
    true
}

/// POP Sreg, as `transfer::pop`: only the selector's word read, the
/// segment loaded, then the stack pointer moved by the operand's size.
fn pop_seg(instr: &Instruction, stack32: bool, u: &mut Vec<Uop>) -> bool {
    let Some(seg) = loaded_seg(instr) else { return false };
    let size = match instr.stack_pointer_increment() {
        2 => 2,
        4 => 4,
        _ => return false,
    };
    let sp = sp(stack32);
    u.push(Uop::Get { t: T1, r: sp });
    u.push(Uop::Copy { dst: T2, src: T1 });
    u.push(Uop::MemRef { t: T2, seg: Seg::SS, size: 2, write: false, slot: 0 });
    u.push(Uop::Load { dst: T0, m: T2, size: 2 });
    u.push(Uop::LoadSeg { seg, t: T0 });
    u.push(Uop::AddConst { t: T1, v: size, size: sp.size });
    u.push(Uop::Set { r: sp, t: T1 });
    true
}

/// The port of IN or OUT operand `i`: DX into T1, or the immediate.
fn port_src(instr: &Instruction, i: u32, u: &mut Vec<Uop>) -> Src {
    if instr.op_kind(i) == OpKind::Register {
        u.push(Uop::Get { t: T1, r: Gpr::word(2) });
        Src::T(T1)
    } else {
        Src::Imm(instr.immediate8() as u32)
    }
}

/// IN AL/AX/EAX, from an immediate port or DX.
fn port_in(instr: &Instruction, u: &mut Vec<Uop>) -> bool {
    let Some(dest) = gpr(instr.op0_register()) else { return false };
    let port = port_src(instr, 1, u);
    u.push(Uop::In { size: dest.size, port, t: T0 });
    u.push(Uop::Set { r: dest, t: T0 });
    true
}

/// OUT to an immediate port or DX from AL/AX/EAX.
fn port_out(instr: &Instruction, u: &mut Vec<Uop>) -> bool {
    let Some(src) = gpr(instr.op1_register()) else { return false };
    let port = port_src(instr, 0, u);
    u.push(Uop::Get { t: T0, r: src });
    u.push(Uop::Out { size: src.size, port, t: T0 });
    true
}

/// PUSH Sreg: with a 32-bit operand size, a 386 or 486 writes the selector
/// into the low word of the dword slot, as `transfer::push` does.
fn push_seg(instr: &Instruction, stack32: bool, u: &mut Vec<Uop>) -> bool {
    let Some(seg) = Seg::from_register(instr.op0_register()) else { return false };
    let sp = sp(stack32);
    u.push(Uop::GetSeg { t: T0, seg });
    if instr.stack_pointer_increment() == -2 {
        push_t0(2, stack32, u);
    } else {
        u.push(Uop::Get { t: T1, r: sp });
        u.push(Uop::AddConst { t: T1, v: 4u32.wrapping_neg(), size: sp.size });
        u.push(Uop::Copy { dst: T2, src: T1 });
        u.push(Uop::MemRef { t: T2, seg: Seg::SS, size: 2, write: true, slot: 0 });
        u.push(Uop::Store { m: T2, src: T0, size: 2 });
    }
    u.push(Uop::Set { r: sp, t: T1 });
    true
}

/// The general-purpose registers in the order PUSHA pushes them (its
/// stack pointer is the one before it).
const PUSHA_ORDER: [u8; 8] = [0, 1, 2, 3, ESP, 5, 6, 7];

/// PUSHA and PUSHAD, as `transfer::pusha`: each slot checked and written
/// in turn from the lowest, EDI's, so a fault leaves those below it
/// written, then the stack pointer moved.
fn pusha(instr: &Instruction, stack32: bool, u: &mut Vec<Uop>) -> bool {
    let size = if instr.code() == Code::Pushad { 4 } else { 2 };
    let sp = sp(stack32);
    let total = 8 * size as u32;
    for (i, &index) in PUSHA_ORDER.iter().enumerate().rev() {
        let below = (i as u32 + 1) * size as u32;
        u.push(Uop::Get { t: T1, r: sp });
        u.push(Uop::AddConst { t: T1, v: below.wrapping_neg(), size: sp.size });
        u.push(Uop::Copy { dst: T2, src: T1 });
        u.push(Uop::MemRef { t: T2, seg: Seg::SS, size, write: true, slot: 0 });
        let r = if size == 4 { Gpr::dword(index) } else { Gpr::word(index) };
        u.push(Uop::Get { t: T0, r });
        u.push(Uop::Store { m: T2, src: T0, size });
    }
    u.push(Uop::Get { t: T1, r: sp });
    u.push(Uop::AddConst { t: T1, v: total.wrapping_neg(), size: sp.size });
    u.push(Uop::Set { r: sp, t: T1 });
    true
}

/// POPA and POPAD, as `transfer::popa`: the registers loaded one by one
/// from EDI's slot up, skipping the stack pointer's (which is still read),
/// then the stack pointer moved. POPAD on a 16-bit stack loads the upper
/// half of ESP on a 386 only: its handler runs it.
fn popa(instr: &Instruction, stack32: bool, u: &mut Vec<Uop>) -> bool {
    let size = if instr.code() == Code::Popad { 4 } else { 2 };
    if size == 4 && !stack32 {
        return false;
    }
    let sp = sp(stack32);
    for (i, &index) in PUSHA_ORDER.iter().rev().enumerate() {
        u.push(Uop::Get { t: T1, r: sp });
        u.push(Uop::AddConst { t: T1, v: i as u32 * size as u32, size: sp.size });
        u.push(Uop::Copy { dst: T2, src: T1 });
        u.push(Uop::MemRef { t: T2, seg: Seg::SS, size, write: false, slot: 0 });
        u.push(Uop::Load { dst: T0, m: T2, size });
        if index != ESP {
            let r = if size == 4 { Gpr::dword(index) } else { Gpr::word(index) };
            u.push(Uop::Set { r, t: T0 });
        }
    }
    u.push(Uop::Get { t: T1, r: sp });
    u.push(Uop::AddConst { t: T1, v: 8 * size as u32, size: sp.size });
    u.push(Uop::Set { r: sp, t: T1 });
    true
}

/// The target of a relative near branch.
fn near_target(instr: &Instruction) -> Option<u32> {
    match instr.op0_kind() {
        OpKind::NearBranch16 => Some(instr.near_branch16() as u32),
        OpKind::NearBranch32 => Some(instr.near_branch32()),
        _ => None,
    }
}

fn jmp(instr: &Instruction, u: &mut Vec<Uop>) -> bool {
    let Some(target) = near_target(instr) else { return jmp_indirect(instr, u) };
    u.push(Uop::CheckLimit { src: Src::Imm(target) });
    u.push(Uop::Exit { eip: Src::Imm(target) });
    true
}

/// JMP to a near target in a register or memory, which leaves the block
/// through the links a return's does (a jump table's targets).
fn jmp_indirect(instr: &Instruction, u: &mut Vec<Uop>) -> bool {
    let size = match instr.code() {
        Code::Jmp_rm16 => 2,
        Code::Jmp_rm32 => 4,
        _ => return false,
    };
    if !near_rm(instr, size, u) {
        return false;
    }
    u.push(Uop::CheckLimit { src: Src::T(T0) });
    u.push(Uop::Exit { eip: Src::T(T0) });
    true
}

/// T0 = the near target in the register or memory of a CALL or JMP, zero-
/// extended from `size` bytes.
fn near_rm(instr: &Instruction, size: u8, u: &mut Vec<Uop>) -> bool {
    match instr.op0_kind() {
        OpKind::Register => {
            let Some(r) = gpr(instr.op0_register()) else { return false };
            u.push(Uop::Get { t: T0, r });
        }
        OpKind::Memory => {
            if mem(instr, T2, size, false, u).is_none() {
                return false;
            }
            u.push(Uop::Load { dst: T0, m: T2, size });
        }
        _ => return false,
    }
    true
}

/// LODS without REP, going up (DF clear): the element into AL, AX or EAX,
/// then SI or ESI moved past it.
fn lods(instr: &Instruction, u: &mut Vec<Uop>) -> bool {
    let size = match instr.code() {
        Code::Lodsb_AL_m8 => 1,
        Code::Lodsw_AX_m16 => 2,
        Code::Lodsd_EAX_m32 => 4,
        _ => return false,
    };
    if instr.has_rep_prefix() || instr.has_repne_prefix() {
        return false;
    }
    let a32 = (0..instr.op_count()).any(|i| instr.op_kind(i) == OpKind::MemorySegESI);
    let (si, width) = if a32 { (Gpr::dword(6), 4) } else { (Gpr::word(6), 2) };
    u.push(Uop::Forward);
    u.push(Uop::Get { t: T2, r: si });
    u.push(Uop::MemRef { t: T2, seg: mem_seg(instr), size, write: false, slot: 0 });
    u.push(Uop::Load { dst: T0, m: T2, size });
    u.push(Uop::Set { r: Gpr { index: 0, high: false, size }, t: T0 });
    u.push(Uop::Get { t: T2, r: si });
    u.push(Uop::AddConst { t: T2, v: size as u32, size: width });
    u.push(Uop::Set { r: si, t: T2 });
    true
}

/// RCL and RCR of a register or memory by 1, through CF.
fn rotate_carry(instr: &Instruction, op: ShiftOp, u: &mut Vec<Uop>) -> bool {
    if !is_imm(instr.op1_kind()) || instr.immediate(1) & 0x1F != 1 {
        return false;
    }
    let size = match instr.op0_kind() {
        OpKind::Register => match gpr(instr.op0_register()) {
            Some(r) => r.size,
            None => return false,
        },
        OpKind::Memory => instr.memory_size().size() as u8,
        _ => return false,
    };
    if !matches!(size, 1 | 2 | 4) {
        return false;
    }
    modify(instr, size, Uop::Shift { op, size, t: T0, count: 1 }, u)
}

/// ENTER with nesting level 0: push the frame pointer, which then becomes
/// the stack pointer, and move that down by the frame's size, which must
/// be writable at its bottom (see `control::enter`). A 32-bit frame
/// pointer on a 16-bit stack (the upper half of ESP with it) runs through
/// the handler.
fn enter(instr: &Instruction, stack32: bool, u: &mut Vec<Uop>) -> bool {
    let size = match instr.code() {
        Code::Enterw_imm16_imm8 => 2,
        Code::Enterd_imm16_imm8 => 4,
        _ => return false,
    };
    if instr.immediate8_2nd() & 0x1F != 0 || (size == 4 && !stack32) {
        return false;
    }
    let sp = sp(stack32);
    u.push(Uop::Get { t: T0, r: Gpr::dword(5) });
    push_t0(size, stack32, u);
    u.push(Uop::Copy { dst: T0, src: T1 });
    u.push(Uop::AddConst { t: T1, v: (instr.immediate16() as u32).wrapping_neg(), size: sp.size });
    u.push(Uop::Copy { dst: T2, src: T1 });
    u.push(Uop::MemRef { t: T2, seg: Seg::SS, size, write: true, slot: 1 });
    u.push(Uop::Set { r: Gpr { index: 5, high: false, size }, t: T0 });
    u.push(Uop::Set { r: sp, t: T1 });
    true
}

/// LEAVE: the frame pointer's top of the stack popped into it, the stack
/// pointer then past it.
fn leave(instr: &Instruction, stack32: bool, u: &mut Vec<Uop>) -> bool {
    let size = match instr.code() {
        Code::Leavew => 2,
        Code::Leaved => 4,
        _ => return false,
    };
    let sp = sp(stack32);
    u.push(Uop::Get { t: T1, r: Gpr { index: 5, high: false, size: sp.size } });
    u.push(Uop::Copy { dst: T2, src: T1 });
    u.push(Uop::MemRef { t: T2, seg: Seg::SS, size, write: false, slot: 0 });
    u.push(Uop::Load { dst: T0, m: T2, size });
    u.push(Uop::AddConst { t: T1, v: size as u32, size: sp.size });
    u.push(Uop::Set { r: sp, t: T1 });
    u.push(Uop::Set { r: Gpr { index: 5, high: false, size }, t: T0 });
    true
}

/// REP counts up to which a translated loop does the iterations: the
/// handler does more at once, and counts their time (see
/// `instructions::string`).
const REP_INLINE: u32 = crate::instructions::string::REP_ONE_INSTRUCTION;

/// MOVS or STOS, with or without REP, going up (DF clear), an iteration as
/// the handler does it: the source's and destination's checks, the
/// element moved, then the indexes and the count.
fn string(instr: &Instruction, u: &mut Vec<Uop>) -> bool {
    let (size, stos) = match instr.code() {
        Code::Movsb_m8_m8 => (1, false),
        Code::Movsw_m16_m16 => (2, false),
        Code::Movsd_m32_m32 => (4, false),
        Code::Stosb_m8_AL => (1, true),
        Code::Stosw_m16_AX => (2, true),
        Code::Stosd_m32_EAX => (4, true),
        _ => return false,
    };
    let a32 = (0..instr.op_count()).any(|i| matches!(instr.op_kind(i), OpKind::MemorySegESI | OpKind::MemoryESEDI));
    let (reg, width) = if a32 { (Gpr::dword as fn(u8) -> Gpr, 4) } else { (Gpr::word as fn(u8) -> Gpr, 2) };
    const ESI: u8 = 6;
    const EDI: u8 = 7;
    let rep = instr.has_rep_prefix() || instr.has_repne_prefix();
    if rep {
        u.push(Uop::RepStart { t: T0, count: reg(ECX), max: REP_INLINE });
    } else {
        u.push(Uop::Forward);
    }
    u.push(Uop::Get { t: T1, r: reg(EDI) });
    if stos {
        u.push(Uop::MemRef { t: T1, seg: Seg::ES, size, write: true, slot: 0 });
        u.push(Uop::Get { t: T0, r: Gpr { index: 0, high: false, size } });
    } else {
        u.push(Uop::Get { t: T2, r: reg(ESI) });
        u.push(Uop::MemRef { t: T2, seg: mem_seg(instr), size, write: false, slot: 0 });
        u.push(Uop::MemRef { t: T1, seg: Seg::ES, size, write: true, slot: 1 });
        u.push(Uop::Load { dst: T0, m: T2, size });
    }
    u.push(Uop::Store { m: T1, src: T0, size });
    let step = |i: u8, u: &mut Vec<Uop>| {
        u.push(Uop::Get { t: T2, r: reg(i) });
        u.push(Uop::AddConst { t: T2, v: size as u32, size: width });
        u.push(Uop::Set { r: reg(i), t: T2 });
    };
    if !stos {
        step(ESI, u);
    }
    step(EDI, u);
    if rep {
        u.push(Uop::Get { t: T0, r: reg(ECX) });
        u.push(Uop::AddConst { t: T0, v: u32::MAX, size: width });
        u.push(Uop::Set { r: reg(ECX), t: T0 });
        u.push(Uop::RepEnd { t: T0 });
    }
    true
}

fn jcc(instr: &Instruction, next: u32, u: &mut Vec<Uop>) -> bool {
    let Some(target) = near_target(instr) else { return false };
    if instr.condition_code() == ConditionCode::None {
        return false;
    }
    u.push(Uop::ExitIf { cond: Cond::Flags(instr.condition_code()), taken: target, next, commit: None });
    true
}

/// SETcc of a byte register or memory.
fn setcc(instr: &Instruction, u: &mut Vec<Uop>) -> bool {
    let cc = instr.condition_code();
    match instr.op0_kind() {
        OpKind::Register => {
            let Some(r) = gpr(instr.op0_register()) else { return false };
            u.push(Uop::SetCond { t: T0, cc });
            u.push(Uop::Set { r, t: T0 });
        }
        OpKind::Memory => {
            if mem(instr, T2, 1, true, u).is_none() {
                return false;
            }
            u.push(Uop::SetCond { t: T0, cc });
            u.push(Uop::Store { m: T2, src: T0, size: 1 });
        }
        _ => return false,
    }
    true
}

/// The counter of LOOPcc and JCXZ, CX or ECX as the address size says.
fn counter(instr: &Instruction) -> Gpr {
    match instr.code() {
        Code::Loop_rel8_16_ECX
        | Code::Loop_rel8_32_ECX
        | Code::Loope_rel8_16_ECX
        | Code::Loope_rel8_32_ECX
        | Code::Loopne_rel8_16_ECX
        | Code::Loopne_rel8_32_ECX
        | Code::Jecxz_rel8_16
        | Code::Jecxz_rel8_32 => Gpr::dword(ECX),
        _ => Gpr::word(ECX),
    }
}

fn loop_op(instr: &Instruction, next: u32, u: &mut Vec<Uop>) -> bool {
    let Some(target) = near_target(instr) else { return false };
    let cx = counter(instr);
    u.push(Uop::Get { t: T0, r: cx });
    u.push(Uop::AddConst { t: T0, v: u32::MAX, size: cx.size });
    let cond = match instr.mnemonic() {
        Mnemonic::Loope => Cond::NonZeroZf(T0, true),
        Mnemonic::Loopne => Cond::NonZeroZf(T0, false),
        _ => Cond::NonZero(T0),
    };
    u.push(Uop::ExitIf { cond, taken: target, next, commit: Some((cx, T0)) });
    true
}

fn jcxz(instr: &Instruction, next: u32, u: &mut Vec<Uop>) -> bool {
    let Some(target) = near_target(instr) else { return false };
    u.push(Uop::Get { t: T0, r: counter(instr) });
    u.push(Uop::ExitIf { cond: Cond::Zero(T0), taken: target, next, commit: None });
    true
}

fn call(instr: &Instruction, next: u32, stack32: bool, u: &mut Vec<Uop>) -> bool {
    let Some(target) = near_target(instr) else { return call_indirect(instr, next, stack32, u) };
    let size = if instr.op0_kind() == OpKind::NearBranch32 { 4 } else { 2 };
    // Push the return address, then jump; a target past the limit leaves
    // the slot written but the stack pointer as it was.
    u.push(Uop::Const { t: T0, v: next });
    push_t0(size, stack32, u);
    u.push(Uop::CheckLimit { src: Src::Imm(target) });
    u.push(Uop::Set { r: sp(stack32), t: T1 });
    u.push(Uop::Exit { eip: Src::Imm(target) });
    true
}

/// CALL of a near target in a register or memory: the target first, as
/// the handler reads it before it pushes, then the return address, and the
/// stack pointer as `push_t0` leaves it, made again (three temporaries).
fn call_indirect(instr: &Instruction, next: u32, stack32: bool, u: &mut Vec<Uop>) -> bool {
    let size = match instr.code() {
        Code::Call_rm16 => 2,
        Code::Call_rm32 => 4,
        _ => return false,
    };
    if !near_rm(instr, size, u) {
        return false;
    }
    let sp = sp(stack32);
    let slot = |u: &mut Vec<Uop>| {
        u.push(Uop::Get { t: T1, r: sp });
        u.push(Uop::AddConst { t: T1, v: (size as u32).wrapping_neg(), size: sp.size });
    };
    slot(u);
    u.push(Uop::Copy { dst: T2, src: T1 });
    u.push(Uop::MemRef { t: T2, seg: Seg::SS, size, write: true, slot: 0 });
    u.push(Uop::Const { t: T1, v: next });
    u.push(Uop::Store { m: T2, src: T1, size });
    // Past the limit, the slot is written but the stack pointer as it was.
    u.push(Uop::CheckLimit { src: Src::T(T0) });
    slot(u);
    u.push(Uop::Set { r: sp, t: T1 });
    u.push(Uop::Exit { eip: Src::T(T0) });
    true
}

fn ret(instr: &Instruction, stack32: bool, u: &mut Vec<Uop>) -> bool {
    let (size, release) = match instr.code() {
        Code::Retnw => (2, 0),
        Code::Retnd => (4, 0),
        Code::Retnw_imm16 => (2, instr.immediate16() as u32),
        Code::Retnd_imm16 => (4, instr.immediate16() as u32),
        _ => return false,
    };
    let sp = sp(stack32);
    pop_t0(size, stack32, u);
    u.push(Uop::CheckLimit { src: Src::T(T0) });
    if release != 0 {
        u.push(Uop::AddConst { t: T1, v: release, size: sp.size });
    }
    u.push(Uop::Set { r: sp, t: T1 });
    u.push(Uop::Exit { eip: Src::T(T0) });
    true
}

/// PUSHF or PUSHFD, as `transfer::pushf`.
fn pushf(instr: &Instruction, stack32: bool, u: &mut Vec<Uop>) -> bool {
    let size = match instr.stack_pointer_increment() {
        -2 => 2,
        -4 => 4,
        _ => return false,
    };
    u.push(Uop::CheckV86Iopl);
    u.push(Uop::GetFlags { t: T0, size });
    push_t0(size, stack32, u);
    u.push(Uop::Set { r: sp(stack32), t: T1 });
    true
}

/// BT, BTS, BTR or BTC of a register, or of memory with an immediate bit
/// offset, as `logic::bit_test`: the offset first, then the operand's
/// access checked (for writing but for BT), read, and written back. (With
/// a register offset, a memory operand is found from the offset too: its
/// handler runs it.)
fn bit_op(instr: &Instruction, op: BitKind, u: &mut Vec<Uop>) -> bool {
    let size = match instr.op0_kind() {
        OpKind::Register => match gpr(instr.op0_register()) {
            Some(r) if r.size > 1 => r.size,
            _ => return false,
        },
        OpKind::Memory if instr.op1_kind() == OpKind::Immediate8 => instr.memory_size().size() as u8,
        _ => return false,
    };
    if size != 2 && size != 4 {
        return false;
    }
    let bit = match instr.op1_kind() {
        OpKind::Immediate8 => Src::Imm(instr.immediate8() as u32 & (size as u32 * 8 - 1)),
        OpKind::Register => {
            let Some(b) = gpr(instr.op1_register()) else { return false };
            u.push(Uop::Get { t: T1, r: b });
            Src::T(T1)
        }
        _ => return false,
    };
    if instr.op0_kind() == OpKind::Register {
        let r = gpr(instr.op0_register()).unwrap();
        u.push(Uop::Get { t: T0, r });
        u.push(Uop::BitOp { op, size, t: T0, bit });
        if op != BitKind::Test {
            u.push(Uop::Set { r, t: T0 });
        }
    } else {
        if mem(instr, T2, size, op != BitKind::Test, u).is_none() {
            return false;
        }
        u.push(Uop::Load { dst: T0, m: T2, size });
        u.push(Uop::BitOp { op, size, t: T0, bit });
        if op != BitKind::Test {
            u.push(Uop::Store { m: T2, src: T0, size });
        }
    }
    true
}

/// LES, LDS, LFS or LGS, as `transfer::load_far_pointer`: the offset's and
/// selector's accesses checked, both read, the segment loaded, then the
/// register set.
fn far_pointer(instr: &Instruction, u: &mut Vec<Uop>) -> bool {
    let seg = match instr.mnemonic() {
        Mnemonic::Les => Seg::ES,
        Mnemonic::Lds => Seg::DS,
        Mnemonic::Lfs => Seg::FS,
        Mnemonic::Lgs => Seg::GS,
        _ => return false,
    };
    let Some(r) = gpr(instr.op0_register()).filter(|r| r.size > 1) else { return false };
    if instr.op1_kind() != OpKind::Memory {
        return false;
    }
    let Some(mseg) = ea(instr, T2, u) else { return false };
    let Some(Uop::Ea { base, index, scale, disp, a32, .. }) = u.last().copied() else { return false };
    u.push(Uop::MemRef { t: T2, seg: mseg, size: r.size, write: false, slot: 0 });
    u.push(Uop::Ea { t: T1, base, index, scale, disp: disp.wrapping_add(r.size as u32), a32 });
    u.push(Uop::MemRef { t: T1, seg: mseg, size: 2, write: false, slot: 1 });
    u.push(Uop::Load { dst: T0, m: T2, size: r.size });
    u.push(Uop::Load { dst: T1, m: T1, size: 2 });
    u.push(Uop::LoadSeg { seg, t: T1 });
    u.push(Uop::Set { r, t: T0 });
    true
}

/// A far JMP or CALL (`call`) in real mode, as `control::jmp` and `call`
/// with `jump_far_real`: the target from the instruction or memory (its
/// offset's and selector's accesses checked first, as `read_far_pointer`
/// does), for a CALL the pushes of CS and the return address, then the
/// target's offset checked against the CS limit, the stack pointer set,
/// CS loaded, and the block left for the target through the links a
/// return takes (their guards hold the CS base).
fn far_jump(instr: &Instruction, next: u32, stack32: bool, call: bool, u: &mut Vec<Uop>) -> bool {
    let size = match instr.code() {
        Code::Jmp_ptr1616 | Code::Call_ptr1616 | Code::Jmp_m1616 | Code::Call_m1616 => 2,
        Code::Jmp_ptr1632 | Code::Call_ptr1632 | Code::Jmp_m1632 | Code::Call_m1632 => 4,
        _ => return false,
    };
    u.push(Uop::CsReal);
    // The target: offset in slot 0 and selector in slot 1 of the context.
    if instr.op0_kind() == OpKind::Memory {
        let Some(seg) = ea(instr, T2, u) else { return false };
        let Some(Uop::Ea { base, index, scale, disp, a32, .. }) = u.last().copied() else { return false };
        u.push(Uop::MemRef { t: T2, seg, size, write: false, slot: 0 });
        u.push(Uop::Ea { t: T1, base, index, scale, disp: disp.wrapping_add(size as u32), a32 });
        u.push(Uop::MemRef { t: T1, seg, size: 2, write: false, slot: 1 });
        u.push(Uop::Load { dst: T0, m: T2, size });
        u.push(Uop::Load { dst: T1, m: T1, size: 2 });
    } else {
        let offset = if size == 2 { instr.far_branch16() as u32 } else { instr.far_branch32() };
        u.push(Uop::Const { t: T0, v: offset });
        u.push(Uop::Const { t: T1, v: instr.far_branch_selector() as u32 });
    }
    u.push(Uop::Spill { t: T0, slot: 0 });
    u.push(Uop::Spill { t: T1, slot: 1 });
    let sp = sp(stack32);
    if call {
        u.push(Uop::GetSeg { t: T0, seg: Seg::CS });
        push_t0(size, stack32, u);
        u.push(Uop::Const { t: T0, v: next });
        u.push(Uop::AddConst { t: T1, v: (size as u32).wrapping_neg(), size: sp.size });
        u.push(Uop::Copy { dst: T2, src: T1 });
        u.push(Uop::MemRef { t: T2, seg: Seg::SS, size, write: true, slot: 2 });
        u.push(Uop::Store { m: T2, src: T0, size });
    }
    u.push(Uop::Unspill { t: T0, slot: 0 });
    u.push(Uop::CheckLimit { src: Src::T(T0) });
    if call {
        u.push(Uop::Set { r: sp, t: T1 });
    }
    u.push(Uop::Unspill { t: T1, slot: 1 });
    u.push(Uop::LoadCsReal { t: T1 });
    u.push(Uop::Exit { eip: Src::T(T0) });
    true
}

/// RETF in real mode, as `control::ret_far`: the offset and selector read
/// (the selector's whole slot), the offset checked against the CS limit,
/// CS loaded, the stack pointer moved past them and any bytes released,
/// and the block left through the links a return takes.
fn far_ret(instr: &Instruction, stack32: bool, u: &mut Vec<Uop>) -> bool {
    let (size, release) = match instr.code() {
        Code::Retfw => (2, 0),
        Code::Retfd => (4, 0),
        Code::Retfw_imm16 => (2, instr.immediate16() as u32),
        Code::Retfd_imm16 => (4, instr.immediate16() as u32),
        _ => return false,
    };
    let sp = sp(stack32);
    u.push(Uop::CsReal);
    pop_t0(size, stack32, u);
    u.push(Uop::Copy { dst: T2, src: T1 });
    u.push(Uop::MemRef { t: T2, seg: Seg::SS, size, write: false, slot: 1 });
    u.push(Uop::Load { dst: T2, m: T2, size });
    u.push(Uop::CheckLimit { src: Src::T(T0) });
    u.push(Uop::AddConst { t: T1, v: size as u32 + release, size: sp.size });
    u.push(Uop::Set { r: sp, t: T1 });
    u.push(Uop::LoadCsReal { t: T2 });
    u.push(Uop::Exit { eip: Src::T(T0) });
    true
}

/// Make the operations of `instr`, whose bytes at physical address `phys`
/// the program changes at offsets `watched`, read its immediate from those
/// bytes rather than use the constant it was decoded with, if only the
/// immediate changes: Doom-engine games poke each column's and span's step
/// into the `ADD r32, imm32` of their drawing loops. The translated code
/// then needn't leave the block whenever the bytes differ from what was
/// translated. False, with nothing changed, for forms other than MOV and
/// the ALU operations with an immediate, and where the watched bytes take
/// in more than the immediate.
pub fn live_immediate(instr: &Instruction, phys: u32, watched: &[usize], u: &mut Vec<Uop>) -> bool {
    let last = match instr.op_count() {
        2 => 1,
        _ => return false,
    };
    let (size, signed) = match instr.op_kind(last) {
        OpKind::Immediate8 => (1, false),
        OpKind::Immediate16 => (2, false),
        OpKind::Immediate32 => (4, false),
        // (Sign-extended to 32 bits, as the other operand's size is.)
        OpKind::Immediate8to32 => (1, true),
        _ => return false,
    };
    // The immediate is the last bytes of these forms.
    let start = instr.len() - size as usize;
    if watched.iter().any(|&w| w < start) {
        return false;
    }
    let mut used = 0u8;
    let mut site = None;
    for (k, uop) in u.iter().enumerate() {
        let (a, b) = match *uop {
            Uop::Get { t, .. } | Uop::Set { t, .. } | Uop::Ea { t, .. } | Uop::MemRef { t, .. } => (t, None),
            Uop::Load { dst, m, .. } => (dst, Some(m)),
            Uop::Store { m, src, .. } => (m, Some(src)),
            Uop::Alu { a, b: Src::T(b), .. } => (a, Some(b)),
            Uop::Const { t, .. } | Uop::Alu { a: t, b: Src::Imm(_), .. } => {
                if site.replace(k).is_some() {
                    return false;
                }
                (t, None)
            }
            _ => return false,
        };
        used |= 1 << a.0 | b.map_or(0, |b| 1 << b.0);
    }
    let Some(site) = site else { return false };
    let load = |t| Uop::LoadCode { t, phys: phys + start as u32, size, signed };
    match u[site] {
        Uop::Const { t, .. } => u[site] = load(t),
        Uop::Alu { op, size: alu_size, a, .. } => {
            let Some(free) = [T0, T1, T2].into_iter().find(|t| used & 1 << t.0 == 0) else { return false };
            u[site] = Uop::Alu { op, size: alu_size, a, b: Src::T(free) };
            // The load neither faults nor changes anything, so it may go first.
            u.insert(0, load(free));
        }
        _ => unreachable!(),
    }
    true
}
