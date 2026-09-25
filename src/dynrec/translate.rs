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
const DF: u32 = 0x0400;

/// The operations for `instr`, or None if it runs through its handler.
/// `next` is the EIP after it (which, as the interpreter has it, doesn't
/// wrap in 16-bit code), and `stack32` the stack's width (SS's B flag),
/// which blocks are translated for.
pub fn translate(instr: &Instruction, next: u32, stack32: bool) -> Option<Vec<Uop>> {
    use Mnemonic::*;
    let mut u = Vec::with_capacity(8);
    let ok = match instr.mnemonic() {
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
        Jmp => jmp(instr, &mut u),
        Jo | Jno | Jb | Jae | Je | Jne | Jbe | Ja | Js | Jns | Jp | Jnp | Jl | Jge | Jle | Jg => jcc(instr, next, &mut u),
        Seto | Setno | Setb | Setae | Sete | Setne | Setbe | Seta | Sets | Setns | Setp | Setnp | Setl | Setge
        | Setle | Setg => setcc(instr, &mut u),
        Loop | Loope | Loopne => loop_op(instr, next, &mut u),
        Jcxz | Jecxz => jcxz(instr, next, &mut u),
        Call => call(instr, next, stack32, &mut u),
        Ret => ret(instr, stack32, &mut u),
        _ => false,
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
fn mem(instr: &Instruction, t: T, size: u8, write: bool, u: &mut Vec<Uop>) -> Option<()> {
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

/// Shifts and rotates of a register by an immediate count from 1 to the
/// width less 1, which the host's instructions do as the interpreter does.
fn shift(instr: &Instruction, op: ShiftOp, u: &mut Vec<Uop>) -> bool {
    if instr.op0_kind() != OpKind::Register || !is_imm(instr.op1_kind()) {
        return false;
    }
    let Some(r) = gpr(instr.op0_register()) else { return false };
    let count = instr.immediate(1) as u32 & 0x1F;
    if count == 0 || count >= r.size as u32 * 8 {
        return false;
    }
    u.push(Uop::Get { t: T0, r });
    u.push(Uop::Shift { op, size: r.size, t: T0, count: count as u8 });
    u.push(Uop::Set { r, t: T0 });
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

/// SHLD and SHRD by an immediate count below the operand's width (a 386
/// rotates the source in for 16-bit operands shifted by more).
fn double_shift(instr: &Instruction, left: bool, u: &mut Vec<Uop>) -> bool {
    if instr.op_count() != 3 || instr.op1_kind() != OpKind::Register || !is_imm(instr.op2_kind()) {
        return false;
    }
    let Some(src) = gpr(instr.op1_register()) else { return false };
    let size = src.size;
    let count = instr.immediate(2) as u32 & 0x1F;
    if size == 1 || count == 0 || count >= size as u32 * 8 {
        return false;
    }
    let count = count as u8;
    match instr.op0_kind() {
        OpKind::Register => {
            let Some(dest) = gpr(instr.op0_register()) else { return false };
            if dest.size != size {
                return false;
            }
            u.push(Uop::Get { t: T0, r: dest });
            u.push(Uop::Get { t: T1, r: src });
            u.push(Uop::DoubleShift { left, size, dst: T0, src: T1, count });
            u.push(Uop::Set { r: dest, t: T0 });
        }
        OpKind::Memory => {
            if mem(instr, T2, size, true, u).is_none() {
                return false;
            }
            u.push(Uop::Load { dst: T0, m: T2, size });
            u.push(Uop::Get { t: T1, r: src });
            u.push(Uop::DoubleShift { left, size, dst: T0, src: T1, count });
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
    let size = match instr.code() {
        Code::Push_r16 | Code::Push_imm16 | Code::Pushw_imm8 => 2,
        Code::Push_r32 | Code::Pushd_imm32 | Code::Pushd_imm8 => 4,
        _ => return false,
    };
    if instr.op0_kind() == OpKind::Register {
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
        Code::Pop_r16 => 2,
        Code::Pop_r32 => 4,
        _ => return false,
    };
    let Some(r) = gpr(instr.op0_register()) else { return false };
    pop_t0(size, stack32, u);
    // The stack pointer first: POP ESP loads the popped value.
    u.push(Uop::Set { r: sp(stack32), t: T1 });
    u.push(Uop::Set { r, t: T0 });
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
    let Some(target) = near_target(instr) else { return false };
    u.push(Uop::CheckLimit { src: Src::Imm(target) });
    u.push(Uop::Exit { eip: Src::Imm(target) });
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
    let Some(target) = near_target(instr) else { return false };
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
