use iced_x86::{Instruction, Mnemonic};
use crate::cpu::Cpu;

pub mod arithmetic;
pub mod comparison;
pub mod control;
pub mod data;
pub mod transcendental;

pub fn handle(cpu: &mut Cpu, instr: &Instruction) {
    match instr.mnemonic() {

        // Source: https://linasm.sourceforge.net/docs/instructions/fpu.php

        // CONTROL & STATE
        // ---------------

        Mnemonic::Fninit | Mnemonic::Finit => control::fninit(cpu),
        Mnemonic::Fnclex | Mnemonic::Fclex => control::fnclex(cpu),
        
        Mnemonic::Fldcw => control::fldcw(cpu, instr),
        Mnemonic::Fstcw | Mnemonic::Fnstcw => control::fnstcw(cpu, instr),
        
        Mnemonic::Fstsw | Mnemonic::Fnstsw => control::fnstsw(cpu, instr),
        
        // No-ops or Wait in HLE
        Mnemonic::Fnop => {}, 
        Mnemonic::Ffree => control::ffree(cpu, instr),

        // DATA TRANSFER
        // -------------

        // Load Float
        Mnemonic::Fld => data::fld(cpu, instr),
        
        // Load/Store Integer
        Mnemonic::Fild => data::fild(cpu, instr),
        Mnemonic::Fist => data::fist(cpu, instr),
        Mnemonic::Fistp => data::fistp(cpu, instr),
        
        // Store Float
        Mnemonic::Fst => data::fst(cpu, instr),
        Mnemonic::Fstp => data::fstp(cpu, instr),
        
        // Exchange
        Mnemonic::Fxch => data::fxch(cpu, instr),
        
        // Constants
        Mnemonic::Fld1 => data::fld1(cpu),
        Mnemonic::Fldz => data::fldz(cpu),
        Mnemonic::Fldpi => data::fldpi(cpu),
        Mnemonic::Fldl2e => data::fldl2e(cpu),
        Mnemonic::Fldl2t => data::fldl2t(cpu),
        Mnemonic::Fldlg2 => data::fldlg2(cpu),
        Mnemonic::Fldln2 => data::fldln2(cpu),
        
        // Conditional Move (Pentium Pro+)
        // Mnemonic::Fcmovb | Mnemonic::Fcmove | ... => data::fcmov(cpu, instr),
        // Don't know if we'll ever get there...

        // ARITHMETIC
        // ----------

        // Unary
        Mnemonic::Fchs => arithmetic::fchs(cpu),
        Mnemonic::Fabs => arithmetic::fabs(cpu),
        Mnemonic::Fsqrt => arithmetic::fsqrt(cpu),
        Mnemonic::Frndint => arithmetic::frndint(cpu),
        Mnemonic::Fscale => arithmetic::fscale(cpu),
        Mnemonic::Fxtract => arithmetic::fxtract(cpu),

        // Addition
        Mnemonic::Fadd => arithmetic::fadd(cpu, instr),
        Mnemonic::Faddp => arithmetic::faddp(cpu, instr),
        Mnemonic::Fiadd => arithmetic::fiadd(cpu, instr),

        // Subtraction
        Mnemonic::Fsub => arithmetic::fsub(cpu, instr),
        Mnemonic::Fsubp => arithmetic::fsubp(cpu),
        Mnemonic::Fsubr => arithmetic::fsubr(cpu, instr),
        Mnemonic::Fsubrp => arithmetic::fsubrp(cpu),
        Mnemonic::Fisub => arithmetic::fisub(cpu, instr),
        Mnemonic::Fisubr => arithmetic::fisubr(cpu, instr),

        // Multiplication
        Mnemonic::Fmul => arithmetic::fmul(cpu, instr),
        Mnemonic::Fmulp => arithmetic::fmulp(cpu, instr),
        Mnemonic::Fimul => arithmetic::fimul(cpu, instr),

        // Division
        Mnemonic::Fdiv => arithmetic::fdiv(cpu, instr),
        Mnemonic::Fdivp => arithmetic::fdivp(cpu, instr),
        Mnemonic::Fdivr => arithmetic::fdivr(cpu, instr),
        Mnemonic::Fdivrp => arithmetic::fdivrp(cpu),
        Mnemonic::Fidiv => arithmetic::fidiv(cpu, instr),
        Mnemonic::Fidivr => arithmetic::fidivr(cpu, instr),

        // Remainders
        Mnemonic::Fprem => arithmetic::fprem(cpu),
        Mnemonic::Fprem1 => arithmetic::fprem1(cpu),
        
        // Advanced Math
        Mnemonic::F2xm1 => arithmetic::f2xm1(cpu),
        Mnemonic::Fyl2x => arithmetic::fyl2x(cpu),
        Mnemonic::Fyl2xp1 => arithmetic::fyl2xp1(cpu),

        // COMPARISON
        // ----------

        // Float Compare
        Mnemonic::Fcom | Mnemonic::Fcomp | Mnemonic::Fcompp => comparison::fcom_variants(cpu, instr),
        
        // Integer Compare
        Mnemonic::Ficom | Mnemonic::Ficomp => comparison::ficom_variants(cpu, instr),
        
        // Test against 0.0
        Mnemonic::Ftst => comparison::ftst(cpu),
        
        // Examine
        Mnemonic::Fxam => comparison::fxam(cpu),

        // Modern Compare (Pentium Pro+) sets EFLAGS directly
        Mnemonic::Fcomi | Mnemonic::Fcomip | 
        Mnemonic::Fucomi | Mnemonic::Fucomip => comparison::fcomi_variants(cpu, instr),

        // TRANSCENDENTAL
        // --------------

        Mnemonic::Fsin => transcendental::fsin(cpu),
        Mnemonic::Fcos => transcendental::fcos(cpu),
        Mnemonic::Fsincos => transcendental::fsincos(cpu),
        Mnemonic::Fptan => transcendental::fptan(cpu),
        Mnemonic::Fpatan => transcendental::fpatan(cpu),

        _ => {
            cpu.bus.log_string(&format!("[FPU] Unhandled instruction: {:?}", instr.mnemonic()));
        }
    }
}