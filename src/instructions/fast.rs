//! Handlers for the most common instruction forms, chosen once when an
//! instruction is decoded (see `handler`).
//!
//! The generic handlers work out at every execution what kind of operand
//! each one is (register, memory, immediate), its size, the address size
//! and, for ALU instructions, the operation. For the forms that make up
//! most of what programs execute (MOV, the ALU operations, INC/DEC, shifts
//! by an immediate or CL, PUSH/POP of a register, LEA, and near jumps,
//! calls and returns) these are fixed per instruction, so the handlers here
//! are instantiated for them at compile time. They use the same memory,
//! flag and stack primitives as the generic handlers and fault the same
//! way; anything unusual is left to `execute_instruction`.

use iced_x86::{ConditionCode, Instruction, Mnemonic, OpKind, Register};

use super::Handler;
use super::operand::{addr_size, mem_seg};
use crate::cpu::alu::{ShiftOp, size_mask};
use crate::cpu::{Access, Cpu, CpuFlags, CpuResult, Fault, Seg};

// ALU operations, as const generic parameters.
const ADD: u8 = 0;
const OR: u8 = 1;
const ADC: u8 = 2;
const SBB: u8 = 3;
const AND: u8 = 4;
const SUB: u8 = 5;
const XOR: u8 = 6;
const CMP: u8 = 7;
const TEST: u8 = 8;

// Shift and rotate operations, as const generic parameters.
const SHL: u8 = 0;
const SHR: u8 = 1;
const SAR: u8 = 2;
const ROL: u8 = 3;
const ROR: u8 = 4;
const RCL: u8 = 5;
const RCR: u8 = 6;

/// The handler instantiated for operand size `$size` (1, 2 or 4), with the
/// const parameters before it.
macro_rules! sized {
    ($size:expr, $f:ident $(, $c:expr)*) => {
        match $size {
            1 => $f::<$($c,)* 1> as Handler,
            2 => $f::<$($c,)* 2> as Handler,
            _ => $f::<$($c,)* 4> as Handler,
        }
    };
}

/// As `sized!`, for a handler with a memory operand that also takes the
/// address size.
macro_rules! sized_mem {
    ($size:expr, $a32:expr, $f:ident $(, $c:expr)*) => {
        match ($size, $a32) {
            (1, false) => $f::<$($c,)* 1, false> as Handler,
            (2, false) => $f::<$($c,)* 2, false> as Handler,
            (_, false) => $f::<$($c,)* 4, false> as Handler,
            (1, true) => $f::<$($c,)* 1, true> as Handler,
            (2, true) => $f::<$($c,)* 2, true> as Handler,
            (_, true) => $f::<$($c,)* 4, true> as Handler,
        }
    };
}

/// The handler for the common form `instr` has, if it has one.
pub fn select(instr: &Instruction) -> Option<Handler> {
    use Mnemonic::*;
    match instr.mnemonic() {
        Mov => mov(instr),
        Add => alu::<ADD>(instr),
        Or => alu::<OR>(instr),
        Adc => alu::<ADC>(instr),
        Sbb => alu::<SBB>(instr),
        And => alu::<AND>(instr),
        Sub => alu::<SUB>(instr),
        Xor => alu::<XOR>(instr),
        Cmp => alu::<CMP>(instr),
        Test => alu::<TEST>(instr),
        Inc => inc_dec(instr, true),
        Dec => inc_dec(instr, false),
        Shl | Sal => shift::<SHL>(instr),
        Shr => shift::<SHR>(instr),
        Sar => shift::<SAR>(instr),
        Rol => shift::<ROL>(instr),
        Ror => shift::<ROR>(instr),
        Rcl => shift::<RCL>(instr),
        Rcr => shift::<RCR>(instr),
        Push => push_pop(instr, true),
        Pop => push_pop(instr, false),
        Lea => lea(instr),
        Jmp => branch(instr).map(|s| sized!(s, jmp_rel)),
        Call => branch(instr).map(|s| sized!(s, call_rel)),
        Ret if instr.op_count() == 0 => Some(if instr.code() == iced_x86::Code::Retnd { ret::<4> } else { ret::<2> }),
        Jo | Jno | Jb | Jae | Je | Jne | Jbe | Ja | Js | Jns | Jp | Jnp | Jl | Jge | Jle | Jg => jcc(instr),
        _ => None,
    }
}

/// Size in bytes of a general-purpose register, or None for any other
/// register.
fn gpr_size(reg: Register) -> Option<u8> {
    if reg.is_gpr8() {
        Some(1)
    } else if reg.is_gpr16() {
        Some(2)
    } else if reg.is_gpr32() {
        Some(4)
    } else {
        None
    }
}

fn is_imm(kind: OpKind) -> bool {
    matches!(
        kind,
        OpKind::Immediate8
            | OpKind::Immediate16
            | OpKind::Immediate32
            | OpKind::Immediate8to16
            | OpKind::Immediate8to32
    )
}

/// The address size of the memory operand, as `sized_mem!` takes it: true
/// for 32-bit addressing. None for a SIB byte that scales no index, whose
/// meaning depends on the CPU model (see `operand::effective_offset`).
fn mem_a32(instr: &Instruction) -> Option<bool> {
    if instr.memory_index() == Register::None && instr.memory_index_scale() != 1 {
        return None;
    }
    Some(addr_size(instr) == 4)
}

/// Operand kinds of the two-operand forms.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    RegReg,
    RegImm,
    RegMem,
    MemReg,
    MemImm,
}

/// The form of a two-operand instruction whose first operand is a
/// general-purpose register or memory of 1, 2 or 4 bytes, and that
/// operand's size.
fn form(instr: &Instruction) -> Option<(Form, u8)> {
    if instr.op_count() != 2 {
        return None;
    }
    let (k0, k1) = (instr.op0_kind(), instr.op1_kind());
    let size = match k0 {
        OpKind::Register => gpr_size(instr.op0_register())?,
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
    // Both registers the same size and general-purpose (no segment,
    // control or debug registers).
    if k1 == OpKind::Register && gpr_size(instr.op1_register()) != Some(size) {
        return None;
    }
    Some((form, size))
}

fn mov(instr: &Instruction) -> Option<Handler> {
    let (form, size) = form(instr)?;
    Some(match form {
        Form::RegReg => mov_rr,
        Form::RegImm => mov_ri,
        Form::RegMem => sized_mem!(size, mem_a32(instr)?, mov_rm),
        Form::MemReg => sized_mem!(size, mem_a32(instr)?, mov_mr),
        Form::MemImm => sized_mem!(size, mem_a32(instr)?, mov_mi),
    })
}

fn alu<const OP: u8>(instr: &Instruction) -> Option<Handler> {
    let (form, size) = form(instr)?;
    Some(match form {
        Form::RegReg => sized!(size, alu_rr, OP),
        Form::RegImm => sized!(size, alu_ri, OP),
        Form::RegMem => sized_mem!(size, mem_a32(instr)?, alu_rm, OP),
        Form::MemReg => sized_mem!(size, mem_a32(instr)?, alu_mr, OP),
        Form::MemImm => sized_mem!(size, mem_a32(instr)?, alu_mi, OP),
    })
}

fn inc_dec(instr: &Instruction, inc: bool) -> Option<Handler> {
    if instr.op0_kind() != OpKind::Register {
        return None;
    }
    let size = gpr_size(instr.op0_register())?;
    Some(if inc { sized!(size, inc_r) } else { sized!(size, dec_r) })
}

fn shift<const OP: u8>(instr: &Instruction) -> Option<Handler> {
    if instr.op0_kind() != OpKind::Register {
        return None;
    }
    let size = gpr_size(instr.op0_register())?;
    match instr.op1_kind() {
        k if is_imm(k) => Some(sized!(size, shift_ri, OP)),
        OpKind::Register if instr.op1_register() == Register::CL => Some(sized!(size, shift_rcl, OP)),
        _ => None,
    }
}

fn push_pop(instr: &Instruction, push: bool) -> Option<Handler> {
    use iced_x86::Code;
    Some(match instr.code() {
        Code::Push_r16 if push => push_r::<2>,
        Code::Push_r32 if push => push_r::<4>,
        Code::Pop_r16 if !push => pop_r::<2>,
        Code::Pop_r32 if !push => pop_r::<4>,
        _ => return None,
    })
}

fn lea(instr: &Instruction) -> Option<Handler> {
    if instr.op0_kind() != OpKind::Register || instr.op1_kind() != OpKind::Memory {
        return None;
    }
    gpr_size(instr.op0_register())?;
    Some(if mem_a32(instr)? { lea_r::<true> } else { lea_r::<false> })
}

/// The operand size (2 or 4) of a near branch to a relative target.
fn branch(instr: &Instruction) -> Option<u8> {
    match instr.op0_kind() {
        OpKind::NearBranch16 => Some(2),
        OpKind::NearBranch32 => Some(4),
        _ => None,
    }
}

fn jcc(instr: &Instruction) -> Option<Handler> {
    use ConditionCode as C;
    let size = branch(instr)?;
    macro_rules! cc {
        ($cc:expr) => {
            if size == 2 { jcc_rel::<{ $cc as u8 }, 2> as Handler } else { jcc_rel::<{ $cc as u8 }, 4> as Handler }
        };
    }
    Some(match instr.condition_code() {
        C::o => cc!(C::o),
        C::no => cc!(C::no),
        C::b => cc!(C::b),
        C::ae => cc!(C::ae),
        C::e => cc!(C::e),
        C::ne => cc!(C::ne),
        C::be => cc!(C::be),
        C::a => cc!(C::a),
        C::s => cc!(C::s),
        C::ns => cc!(C::ns),
        C::p => cc!(C::p),
        C::np => cc!(C::np),
        C::l => cc!(C::l),
        C::ge => cc!(C::ge),
        C::le => cc!(C::le),
        C::g => cc!(C::g),
        C::None => return None,
    })
}

// --- Operands ---

/// Offset of the memory operand. `mem_a32` checked that it needs no
/// model-specific handling. A missing base or index register reads as 0.
#[inline(always)]
fn ea<const A32: bool>(cpu: &Cpu, instr: &Instruction) -> u32 {
    let ea = instr
        .memory_displacement32()
        .wrapping_add(cpu.reg(instr.memory_base()))
        .wrapping_add(cpu.reg(instr.memory_index()).wrapping_mul(instr.memory_index_scale()));
    if A32 { ea } else { ea & 0xFFFF }
}

/// Check the memory operand for an access of `S` bytes.
#[inline(always)]
fn mem<const S: u8, const A32: bool>(cpu: &mut Cpu, instr: &Instruction, access: Access) -> CpuResult<crate::cpu::MemRef> {
    let off = ea::<A32>(cpu, instr);
    cpu.mem_ref(mem_seg(instr), off, S, access)
}

/// The immediate operand (the second), sign-extended where the encoding
/// says so and cut to `S` bytes.
#[inline(always)]
fn imm<const S: u8>(instr: &Instruction) -> u32 {
    instr.immediate(1) as u32 & size_mask(S)
}

// --- MOV ---

fn mov_rr(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let value = cpu.reg(instr.op1_register());
    cpu.set_reg(instr.op0_register(), value);
    Ok(())
}

fn mov_ri(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    cpu.set_reg(instr.op0_register(), instr.immediate(1) as u32);
    Ok(())
}

fn mov_rm<const S: u8, const A32: bool>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let m = mem::<S, A32>(cpu, instr, Access::Read)?;
    let value = cpu.mem_read(m);
    cpu.set_reg(instr.op0_register(), value);
    Ok(())
}

fn mov_mr<const S: u8, const A32: bool>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let m = mem::<S, A32>(cpu, instr, Access::Write)?;
    let value = cpu.reg(instr.op1_register());
    cpu.mem_write(m, value);
    Ok(())
}

fn mov_mi<const S: u8, const A32: bool>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let m = mem::<S, A32>(cpu, instr, Access::Write)?;
    cpu.mem_write(m, imm::<S>(instr));
    Ok(())
}

// --- ALU ---

/// `a OP b` on `S`-byte operands, setting the flags.
#[inline(always)]
fn alu_op<const OP: u8, const S: u8>(cpu: &mut Cpu, a: u32, b: u32) -> u32 {
    match OP {
        ADD => cpu.alu_add(S, a, b, false),
        ADC => {
            let cf = cpu.get_cpu_flag(CpuFlags::CF);
            cpu.alu_add(S, a, b, cf)
        }
        SUB | CMP => cpu.alu_sub(S, a, b, false),
        SBB => {
            let cf = cpu.get_cpu_flag(CpuFlags::CF);
            cpu.alu_sub(S, a, b, cf)
        }
        AND | TEST => cpu.alu_logic(S, a & b),
        OR => cpu.alu_logic(S, a | b),
        _ => cpu.alu_logic(S, a ^ b),
    }
}

/// CMP and TEST only set the flags.
const fn writes(op: u8) -> bool {
    op != CMP && op != TEST
}

fn alu_rr<const OP: u8, const S: u8>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let dest = instr.op0_register();
    let (a, b) = (cpu.reg(dest), cpu.reg(instr.op1_register()));
    let r = alu_op::<OP, S>(cpu, a, b);
    if writes(OP) {
        cpu.set_reg(dest, r);
    }
    Ok(())
}

fn alu_ri<const OP: u8, const S: u8>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let dest = instr.op0_register();
    let a = cpu.reg(dest);
    let r = alu_op::<OP, S>(cpu, a, imm::<S>(instr));
    if writes(OP) {
        cpu.set_reg(dest, r);
    }
    Ok(())
}

fn alu_rm<const OP: u8, const S: u8, const A32: bool>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let m = mem::<S, A32>(cpu, instr, Access::Read)?;
    let dest = instr.op0_register();
    let (a, b) = (cpu.reg(dest), cpu.mem_read(m));
    let r = alu_op::<OP, S>(cpu, a, b);
    if writes(OP) {
        cpu.set_reg(dest, r);
    }
    Ok(())
}

fn alu_mr<const OP: u8, const S: u8, const A32: bool>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let access = if writes(OP) { Access::Write } else { Access::Read };
    let m = mem::<S, A32>(cpu, instr, access)?;
    let (a, b) = (cpu.mem_read(m), cpu.reg(instr.op1_register()));
    let r = alu_op::<OP, S>(cpu, a, b);
    if writes(OP) {
        cpu.mem_write(m, r);
    }
    Ok(())
}

fn alu_mi<const OP: u8, const S: u8, const A32: bool>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let access = if writes(OP) { Access::Write } else { Access::Read };
    let m = mem::<S, A32>(cpu, instr, access)?;
    let a = cpu.mem_read(m);
    let r = alu_op::<OP, S>(cpu, a, imm::<S>(instr));
    if writes(OP) {
        cpu.mem_write(m, r);
    }
    Ok(())
}

fn inc_r<const S: u8>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let reg = instr.op0_register();
    let r = cpu.alu_inc(S, cpu.reg(reg));
    cpu.set_reg(reg, r);
    Ok(())
}

fn dec_r<const S: u8>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let reg = instr.op0_register();
    let r = cpu.alu_dec(S, cpu.reg(reg));
    cpu.set_reg(reg, r);
    Ok(())
}

// --- Shifts and rotates ---

#[inline(always)]
fn shift_op(op: u8) -> ShiftOp {
    match op {
        SHL => ShiftOp::Shl,
        SHR => ShiftOp::Shr,
        SAR => ShiftOp::Sar,
        ROL => ShiftOp::Rol,
        ROR => ShiftOp::Ror,
        RCL => ShiftOp::Rcl,
        _ => ShiftOp::Rcr,
    }
}

/// Shift or rotate register operand 0 by `count`.
#[inline(always)]
fn shift_reg<const OP: u8, const S: u8>(cpu: &mut Cpu, instr: &Instruction, count: u32) {
    let count = count & 0x1F;
    if count == 0 {
        return;
    }
    let reg = instr.op0_register();
    let r = cpu.alu_shift(shift_op(OP), S, cpu.reg(reg), count);
    cpu.set_reg(reg, r);
}

fn shift_ri<const OP: u8, const S: u8>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    shift_reg::<OP, S>(cpu, instr, instr.immediate(1) as u32);
    Ok(())
}

fn shift_rcl<const OP: u8, const S: u8>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let count = cpu.ecx() & 0xFF;
    shift_reg::<OP, S>(cpu, instr, count);
    Ok(())
}

// --- Stack ---

fn push_r<const S: u8>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let value = cpu.reg(instr.op0_register());
    cpu.push_sized(S, value)
}

fn pop_r<const S: u8>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let value = cpu.pop_sized(S)?;
    cpu.set_reg(instr.op0_register(), value);
    Ok(())
}

fn lea_r<const A32: bool>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let offset = ea::<A32>(cpu, instr);
    cpu.set_reg(instr.op0_register(), offset);
    Ok(())
}

// --- Near branches ---

/// The target of a relative near branch with operand size `S`.
#[inline(always)]
fn target<const S: u8>(instr: &Instruction) -> u32 {
    if S == 2 { instr.near_branch16() as u32 } else { instr.near_branch32() }
}

/// Jump to `target` in the current code segment; past the CS limit it
/// raises #GP(0).
#[inline(always)]
fn jump<const S: u8>(cpu: &mut Cpu, target: u32) -> CpuResult {
    let target = if S == 2 { target & 0xFFFF } else { target };
    if target > cpu.seg_cache(Seg::CS).limit {
        return Err(Fault::gp(0));
    }
    cpu.set_eip(target);
    Ok(())
}

/// Whether condition code `CC` (an iced `ConditionCode`) holds.
#[inline(always)]
fn condition<const CC: u8>(cpu: &Cpu) -> bool {
    use ConditionCode as C;
    let f = |flag| cpu.get_cpu_flag(flag);
    match CC {
        c if c == C::o as u8 => f(CpuFlags::OF),
        c if c == C::no as u8 => !f(CpuFlags::OF),
        c if c == C::b as u8 => f(CpuFlags::CF),
        c if c == C::ae as u8 => !f(CpuFlags::CF),
        c if c == C::e as u8 => f(CpuFlags::ZF),
        c if c == C::ne as u8 => !f(CpuFlags::ZF),
        c if c == C::be as u8 => f(CpuFlags::CF) || f(CpuFlags::ZF),
        c if c == C::a as u8 => !f(CpuFlags::CF) && !f(CpuFlags::ZF),
        c if c == C::s as u8 => f(CpuFlags::SF),
        c if c == C::ns as u8 => !f(CpuFlags::SF),
        c if c == C::p as u8 => f(CpuFlags::PF),
        c if c == C::np as u8 => !f(CpuFlags::PF),
        c if c == C::l as u8 => f(CpuFlags::SF) != f(CpuFlags::OF),
        c if c == C::ge as u8 => f(CpuFlags::SF) == f(CpuFlags::OF),
        c if c == C::le as u8 => f(CpuFlags::ZF) || f(CpuFlags::SF) != f(CpuFlags::OF),
        _ => !f(CpuFlags::ZF) && f(CpuFlags::SF) == f(CpuFlags::OF),
    }
}

fn jcc_rel<const CC: u8, const S: u8>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    if condition::<CC>(cpu) {
        return jump::<S>(cpu, target::<S>(instr));
    }
    Ok(())
}

fn jmp_rel<const S: u8>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    jump::<S>(cpu, target::<S>(instr))
}

fn call_rel<const S: u8>(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    let eip = cpu.eip();
    cpu.push_sized(S, eip)?;
    jump::<S>(cpu, target::<S>(instr))
}

fn ret<const S: u8>(cpu: &mut Cpu, _instr: &Instruction) -> CpuResult {
    let target = cpu.stack_read(0, S)?;
    jump::<S>(cpu, target)?;
    let sp = cpu.stack_ptr().wrapping_add(S as u32);
    cpu.set_stack_ptr(sp);
    Ok(())
}
