//! Instruction execution: one handler per instruction family, dispatched on
//! iced's mnemonic. Handlers return `Err(Fault)` to raise an exception;
//! anything not implemented for the emulated CPU raises #UD.

use iced_x86::{Instruction, Mnemonic};

use crate::cpu::alu::ShiftOp;
use crate::cpu::{CR0_EM, CR0_TS, Cpu, CpuFlags, CpuResult, Fault, Seg};

pub mod arith;
pub mod control;
pub mod fpu;
pub mod logic;
pub mod operand;
pub mod string;
pub mod system;
pub mod transfer;
pub mod utils;

use arith::Op;
use logic::BitOp;
use string::StrOp;

/// An FPU instruction. Kept out of `execute_instruction`, whose every
/// call pays for the registers the inlined code of its arms needs.
#[inline(never)]
fn fpu_instruction(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    // No coprocessor (EM), or it belongs to another task (TS).
    if cpu.cr0 & (CR0_EM | CR0_TS) != 0 {
        return Err(Fault::NM);
    }
    check_fpu_operand(cpu, instr)?;
    fpu::handle(cpu, instr);
    Ok(())
}

/// Check an FPU instruction's memory operand, all of it, before the FPU
/// changes any state: the FPU then accesses it without faulting.
fn check_fpu_operand(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    if !(0..instr.op_count()).any(|i| instr.op_kind(i) == iced_x86::OpKind::Memory) {
        return Ok(());
    }
    use Mnemonic::*;
    let access = match instr.mnemonic() {
        Fst | Fstp | Fist | Fistp | Fisttp | Fbstp | Fstcw | Fnstcw | Fstsw | Fnstsw | Fsave | Fnsave
        | Fstenv | Fnstenv => crate::cpu::Access::Write,
        _ => crate::cpu::Access::Read,
    };
    let len = instr.memory_size().size().max(1) as u32;
    let off = operand::effective_offset(cpu, instr);
    cpu.check_span(operand::mem_seg(instr), off, len, access)?;
    Ok(())
}

pub fn execute_instruction(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    use Mnemonic::*;
    match instr.mnemonic() {
        // --- Data transfer ---
        Mov => transfer::mov(cpu, instr),
        Movzx => transfer::movx(cpu, instr, false),
        Movsx => transfer::movx(cpu, instr, true),
        Xchg => transfer::xchg(cpu, instr),
        Lea => transfer::lea(cpu, instr),
        Lds => transfer::load_far_pointer(cpu, instr, Seg::DS),
        Les => transfer::load_far_pointer(cpu, instr, Seg::ES),
        Lfs => transfer::load_far_pointer(cpu, instr, Seg::FS),
        Lgs => transfer::load_far_pointer(cpu, instr, Seg::GS),
        Lss => transfer::load_far_pointer(cpu, instr, Seg::SS),
        Push => transfer::push(cpu, instr),
        Pop => transfer::pop(cpu, instr),
        Pusha | Pushad => transfer::pusha(cpu, instr),
        Popa | Popad => transfer::popa(cpu, instr),
        Pushf | Pushfd => transfer::pushf(cpu, instr),
        Popf | Popfd => transfer::popf(cpu, instr),
        In => transfer::port_in(cpu, instr),
        Out => transfer::port_out(cpu, instr),
        Xlatb => transfer::xlat(cpu, instr),
        Lahf => transfer::lahf(cpu),
        Sahf => transfer::sahf(cpu),
        Salc => transfer::salc(cpu),
        Bswap => transfer::bswap(cpu, instr),
        Xadd => transfer::xadd(cpu, instr),
        Cmpxchg => transfer::cmpxchg(cpu, instr),
        Cbw => arith::cbw(cpu),
        Cwde => arith::cwde(cpu),
        Cwd => arith::cwd(cpu),
        Cdq => arith::cdq(cpu),

        // --- Arithmetic and logic ---
        Add => arith::binary(cpu, instr, Op::Add),
        Adc => arith::binary(cpu, instr, Op::Adc),
        Sub => arith::binary(cpu, instr, Op::Sub),
        Sbb => arith::binary(cpu, instr, Op::Sbb),
        Cmp => arith::binary(cpu, instr, Op::Cmp),
        And => arith::binary(cpu, instr, Op::And),
        Or => arith::binary(cpu, instr, Op::Or),
        Xor => arith::binary(cpu, instr, Op::Xor),
        Test => arith::binary(cpu, instr, Op::Test),
        Inc => arith::inc(cpu, instr),
        Dec => arith::dec(cpu, instr),
        Neg => arith::neg(cpu, instr),
        Not => arith::not(cpu, instr),
        Mul => arith::mul(cpu, instr),
        Imul => arith::imul(cpu, instr),
        Div => arith::div(cpu, instr),
        Idiv => arith::idiv(cpu, instr),
        Daa => arith::daa(cpu),
        Das => arith::das(cpu),
        Aaa => arith::aaa(cpu),
        Aas => arith::aas(cpu),
        Aam => arith::aam(cpu, instr),
        Aad => arith::aad(cpu, instr),

        // --- Shifts, rotates and bits ---
        Rol => logic::shift(cpu, instr, ShiftOp::Rol),
        Ror => logic::shift(cpu, instr, ShiftOp::Ror),
        Rcl => logic::shift(cpu, instr, ShiftOp::Rcl),
        Rcr => logic::shift(cpu, instr, ShiftOp::Rcr),
        Shl | Sal => logic::shift(cpu, instr, ShiftOp::Shl),
        Shr => logic::shift(cpu, instr, ShiftOp::Shr),
        Sar => logic::shift(cpu, instr, ShiftOp::Sar),
        Shld => logic::double_shift(cpu, instr, true),
        Shrd => logic::double_shift(cpu, instr, false),
        Bt => logic::bit_test(cpu, instr, BitOp::Test),
        Bts => logic::bit_test(cpu, instr, BitOp::Set),
        Btr => logic::bit_test(cpu, instr, BitOp::Reset),
        Btc => logic::bit_test(cpu, instr, BitOp::Complement),
        Bsf => logic::bit_scan(cpu, instr, true),
        Bsr => logic::bit_scan(cpu, instr, false),
        Seto | Setno | Setb | Setae | Sete | Setne | Setbe | Seta | Sets | Setns | Setp | Setnp
        | Setl | Setge | Setle | Setg => logic::setcc(cpu, instr),

        // --- Control transfer ---
        Jmp => control::jmp(cpu, instr),
        Call => control::call(cpu, instr),
        Ret => control::ret_near(cpu, instr),
        Retf => control::ret_far(cpu, instr),
        Jo | Jno | Jb | Jae | Je | Jne | Jbe | Ja | Js | Jns | Jp | Jnp | Jl | Jge | Jle | Jg => {
            control::jcc(cpu, instr)
        }
        Jcxz | Jecxz => control::jcxz(cpu, instr),
        Loop | Loope | Loopne => control::loop_op(cpu, instr),
        Int => control::int(cpu, instr),
        Int3 => control::software_interrupt(cpu, 3),
        Int1 => control::software_interrupt(cpu, 1),
        Into => control::into(cpu),
        Iret | Iretd => control::iret(cpu, instr),
        Bound => control::bound(cpu, instr),
        Enter => control::enter(cpu, instr),
        Leave => control::leave(cpu, instr),

        // --- Strings ---
        Movsb => string::string(cpu, instr, StrOp::Movs, 1),
        Movsw => string::string(cpu, instr, StrOp::Movs, 2),
        Movsd => string::string(cpu, instr, StrOp::Movs, 4),
        Cmpsb => string::string(cpu, instr, StrOp::Cmps, 1),
        Cmpsw => string::string(cpu, instr, StrOp::Cmps, 2),
        Cmpsd => string::string(cpu, instr, StrOp::Cmps, 4),
        Scasb => string::string(cpu, instr, StrOp::Scas, 1),
        Scasw => string::string(cpu, instr, StrOp::Scas, 2),
        Scasd => string::string(cpu, instr, StrOp::Scas, 4),
        Lodsb => string::string(cpu, instr, StrOp::Lods, 1),
        Lodsw => string::string(cpu, instr, StrOp::Lods, 2),
        Lodsd => string::string(cpu, instr, StrOp::Lods, 4),
        Stosb => string::string(cpu, instr, StrOp::Stos, 1),
        Stosw => string::string(cpu, instr, StrOp::Stos, 2),
        Stosd => string::string(cpu, instr, StrOp::Stos, 4),
        Insb => string::string(cpu, instr, StrOp::Ins, 1),
        Insw => string::string(cpu, instr, StrOp::Ins, 2),
        Insd => string::string(cpu, instr, StrOp::Ins, 4),
        Outsb => string::string(cpu, instr, StrOp::Outs, 1),
        Outsw => string::string(cpu, instr, StrOp::Outs, 2),
        Outsd => string::string(cpu, instr, StrOp::Outs, 4),

        // --- Flags and system ---
        Clc => system::set_flag(cpu, CpuFlags::CF, false),
        Stc => system::set_flag(cpu, CpuFlags::CF, true),
        Cmc => system::cmc(cpu),
        Cld => system::set_flag(cpu, CpuFlags::DF, false),
        Std => system::set_flag(cpu, CpuFlags::DF, true),
        Cli => system::cli(cpu),
        Sti => system::sti(cpu),
        Hlt => system::hlt(cpu),
        Nop | Pause => Ok(()),
        Wait => system::wait(cpu),
        Lgdt => system::load_table(cpu, instr, false),
        Lidt => system::load_table(cpu, instr, true),
        Sgdt => system::store_table(cpu, instr, false),
        Sidt => system::store_table(cpu, instr, true),
        Smsw => system::smsw(cpu, instr),
        Lmsw => system::lmsw(cpu, instr),
        Clts => system::clts(cpu),
        Invd | Wbinvd => system::cache_op(cpu),
        Invlpg => system::invlpg(cpu, instr),
        Lldt => system::lldt(cpu, instr),
        Ltr => system::ltr(cpu, instr),
        Sldt => system::store_selector(cpu, instr, false),
        Str => system::store_selector(cpu, instr, true),
        Lar => system::load_access(cpu, instr, false),
        Lsl => system::load_access(cpu, instr, true),
        Verr => system::verify(cpu, instr, false),
        Verw => system::verify(cpu, instr, true),
        Arpl => system::arpl(cpu, instr),

        // --- FPU ---
        Fadd | Faddp | Fiadd | Fsub | Fsubp | Fsubr | Fsubrp | Fisub | Fisubr | Fmul | Fmulp
        | Fimul | Fdiv | Fdivp | Fdivr | Fdivrp | Fidiv | Fidivr | Fsqrt | Fscale | Fprem
        | Fprem1 | Frndint | Fxtract | Fabs | Fchs | F2xm1 | Fyl2x | Fyl2xp1 | Fsin | Fcos
        | Fsincos | Fptan | Fpatan | Fld | Fst | Fstp | Fild | Fist | Fistp | Fisttp | Fbld
        | Fbstp | Fxch | Fld1 | Fldz | Fldpi | Fldl2e | Fldl2t | Fldlg2 | Fldln2 | Fcom | Fcomp
        | Fcompp | Ficom | Ficomp | Ftst | Fxam | Fcomi | Fcomip | Fucomi | Fucomip | Finit
        | Fninit | Fldcw | Fstcw | Fnstcw | Fstsw | Fnstsw | Fclex | Fnclex | Fsave | Fnsave
        | Frstor | Fstenv | Fnstenv | Fldenv | Fnop | Ffree | Fincstp | Fdecstp | Fucom | Fucomp
        | Fucompp | Fneni | Fndisi | Fnsetpm | Feni | Fdisi | Fsetpm => fpu_instruction(cpu, instr),

        _ => Err(Fault::UD),
    }
}
