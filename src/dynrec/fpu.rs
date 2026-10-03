//! FPU instructions as operations: the forms programs run all the time
//! (Quake's inner loops are little else), on the registers' doubles
//! (`f80::FpuRegs`) as their handlers have them. Sums and differences,
//! which the handlers compute on the 80 bits, call the handlers' own code.
//! Every instruction starts with a `FpuGuard`, which leaves the rare cases
//! (no coprocessor, an empty register) to the handler.

use iced_x86::{Code, Instruction, OpKind, Register};

use super::translate::mem;
use super::uop::*;
use crate::instructions::fpu::arithmetic::{ADD_VALUE, SUB_VALUE, SUBR_VALUE, get_pop_dst_index};
use crate::instructions::fpu::{comparison, data};

/// ST(i)'s number, for register operand `op`.
fn st(instr: &Instruction, op: u32) -> Option<u8> {
    let reg = instr.op_register(op);
    (Register::ST0..=Register::ST7).contains(&reg).then(|| (reg.number() - Register::ST0.number()) as u8)
}

/// The numbers of the two registers of an instruction on ST(0) and ST(i).
fn st_pair(instr: &Instruction) -> Option<(u8, u8)> {
    Some((st(instr, 0)?, st(instr, 1)?))
}

/// The operations of FPU instruction `instr`, or false if its handler
/// runs it.
pub fn translate(instr: &Instruction, u: &mut Vec<Uop>) -> bool {
    use Code::*;
    let guard = |u: &mut Vec<Uop>, valid: u8| u.push(Uop::FpuGuard { valid });
    // A single in memory, into X0.
    let single = |u: &mut Vec<Uop>| -> bool {
        if mem(instr, T1, 4, false, u).is_none() {
            return false;
        }
        u.push(Uop::Load { dst: T0, m: T1, size: 4 });
        u.push(Uop::FFromT { x: X0, t: T0, kind: FKind::Single });
        true
    };
    match instr.code() {
        Fld_m32fp => {
            guard(u, 0);
            if !single(u) {
                return false;
            }
            u.push(Uop::FPush { x: X0, canon: false });
        }
        Fld_sti => {
            let Some(i) = st(instr, 0) else { return false };
            guard(u, 1 << i);
            u.push(Uop::FCopy { dst: None, src: i });
        }
        Fld1 | Fldz => {
            guard(u, 0);
            u.push(Uop::Const { t: T0, v: (instr.code() == Fld1) as u32 });
            u.push(Uop::FFromT { x: X0, t: T0, kind: FKind::Int });
            u.push(Uop::FPush { x: X0, canon: false });
        }
        Fild_m16int | Fild_m32int => {
            let size = if instr.code() == Fild_m16int { 2 } else { 4 };
            guard(u, 0);
            if mem(instr, T1, size, false, u).is_none() {
                return false;
            }
            u.push(Uop::Load { dst: T0, m: T1, size });
            if size == 2 {
                u.push(Uop::Extend { t: T0, from: 2, signed: true });
            }
            u.push(Uop::FFromT { x: X0, t: T0, kind: FKind::Int });
            u.push(Uop::FPush { x: X0, canon: false });
        }
        Fst_m32fp | Fstp_m32fp => {
            guard(u, 1);
            if mem(instr, T1, 4, true, u).is_none() {
                return false;
            }
            u.push(Uop::FGet { x: X0, i: 0 });
            u.push(Uop::FToSingle { t: T0, x: X0 });
            if instr.code() == Fstp_m32fp {
                u.push(Uop::FPop { n: 1 });
            }
            u.push(Uop::Store { m: T1, src: T0, size: 4 });
        }
        Fist_m16int | Fist_m32int | Fistp_m16int | Fistp_m32int => {
            let size = if matches!(instr.code(), Fist_m16int | Fistp_m16int) { 2 } else { 4 };
            guard(u, 1);
            if mem(instr, T1, size, true, u).is_none() {
                return false;
            }
            u.push(Uop::FGet { x: X0, i: 0 });
            u.push(Uop::FToInt { t: T0, x: X0, size });
            if matches!(instr.code(), Fistp_m16int | Fistp_m32int) {
                u.push(Uop::FPop { n: 1 });
            }
            u.push(Uop::Store { m: T1, src: T0, size });
        }
        Fst_sti | Fstp_sti | Fstpnce_sti | Fstp_sti_DFD0 | Fstp_sti_DFD8 => {
            if instr.op0_kind() != OpKind::Register {
                return false;
            }
            let Some(i) = st(instr, 0) else { return false };
            guard(u, 1);
            if i != 0 {
                u.push(Uop::FCopy { dst: Some(i), src: 0 });
            }
            if instr.code() != Fst_sti {
                u.push(Uop::FPop { n: 1 });
            }
        }
        Fxch_st0_sti | Fxch_st0_sti_DDC8 | Fxch_st0_sti_DFC8 => {
            let i = data::fxch_index(instr) as u8;
            if i == 0 {
                // (The handler does nothing, not even clear C1.)
                guard(u, 0);
            } else {
                guard(u, 1 | 1 << i);
                u.push(Uop::FXch { i });
            }
        }
        Fmul_m32fp => {
            guard(u, 1);
            if !single(u) {
                return false;
            }
            u.push(Uop::FGet { x: X1, i: 0 });
            u.push(Uop::FMul { a: X1, b: X0 });
            u.push(Uop::FSet { i: 0, x: X1, canon: true });
        }
        Fmul_st0_sti | Fmul_sti_st0 | Fmulp_sti_st0 => {
            let pop = instr.code() == Fmulp_sti_st0;
            let Some((dst, src)) = (if pop { Some((get_pop_dst_index(instr) as u8, 0)) } else { st_pair(instr) }) else {
                return false;
            };
            guard(u, 1 << dst | 1 << src);
            u.push(Uop::FGet { x: X0, i: dst });
            u.push(Uop::FGet { x: X1, i: src });
            u.push(Uop::FMul { a: X0, b: X1 });
            u.push(Uop::FSet { i: dst, x: X0, canon: true });
            if pop {
                u.push(Uop::FPop { n: 1 });
            }
        }
        Fdiv_m32fp | Fdivr_m32fp => {
            let reversed = instr.code() == Fdivr_m32fp;
            guard(u, 1);
            if !single(u) {
                return false;
            }
            u.push(Uop::FGet { x: X1, i: 0 });
            let (num, den) = if reversed { (X0, X1) } else { (X1, X0) };
            u.push(Uop::FDiv { i: 0, num, den, ze: reversed });
        }
        Fdiv_st0_sti | Fdiv_sti_st0 | Fdivr_st0_sti | Fdivr_sti_st0 | Fdivp_sti_st0 | Fdivrp_sti_st0 => {
            let pop = matches!(instr.code(), Fdivp_sti_st0 | Fdivrp_sti_st0);
            let reversed = matches!(instr.code(), Fdivr_st0_sti | Fdivr_sti_st0 | Fdivrp_sti_st0);
            let Some((dst, src)) = (if pop { Some((get_pop_dst_index(instr) as u8, 0)) } else { st_pair(instr) }) else {
                return false;
            };
            guard(u, 1 << dst | 1 << src);
            u.push(Uop::FGet { x: X0, i: dst });
            u.push(Uop::FGet { x: X1, i: src });
            let (num, den) = if reversed { (X1, X0) } else { (X0, X1) };
            u.push(Uop::FDiv { i: dst, num, den, ze: reversed });
            if pop {
                u.push(Uop::FPop { n: 1 });
            }
        }
        Fadd_m32fp | Fsub_m32fp | Fsubr_m32fp => {
            let kind = match instr.code() {
                Fadd_m32fp => ADD_VALUE,
                Fsub_m32fp => SUB_VALUE,
                _ => SUBR_VALUE,
            };
            guard(u, 0);
            if !single(u) {
                return false;
            }
            u.push(Uop::FAddValue { kind, x: X0 });
        }
        Fadd_st0_sti | Fadd_sti_st0 | Fsub_st0_sti | Fsub_sti_st0 => {
            let Some((dst, src)) = st_pair(instr) else { return false };
            guard(u, 0);
            u.push(Uop::FAddSt { dst, a: dst, b: src, sub: !matches!(instr.code(), Fadd_st0_sti | Fadd_sti_st0) });
        }
        Fsubr_st0_sti | Fsubr_sti_st0 => {
            let Some((dst, src)) = st_pair(instr) else { return false };
            guard(u, 0);
            u.push(Uop::FAddSt { dst, a: src, b: dst, sub: true });
        }
        Faddp_sti_st0 | Fsubp_sti_st0 | Fsubrp_sti_st0 => {
            // (FADDP reads its register as FSUBP's and FSUBRP's handlers do.)
            let i = get_pop_dst_index(instr) as u8;
            guard(u, 0);
            let (a, b) = if instr.code() == Fsubrp_sti_st0 { (0, i) } else { (i, 0) };
            u.push(Uop::FAddSt { dst: i, a, b, sub: instr.code() != Faddp_sti_st0 });
            u.push(Uop::FPop { n: 1 });
        }
        Fcom_m32fp | Fcomp_m32fp => {
            guard(u, 1);
            if !single(u) {
                return false;
            }
            u.push(Uop::FGet { x: X1, i: 0 });
            u.push(Uop::FCom { a: X1, b: X0 });
            if instr.code() == Fcomp_m32fp {
                u.push(Uop::FPop { n: 1 });
            }
        }
        Fcom_st0_sti | Fcom_st0_sti_DCD0 | Fcomp_st0_sti | Fcomp_st0_sti_DCD8 | Fcomp_st0_sti_DED0 | Fcompp
        | Fucom_st0_sti | Fucomp_st0_sti | Fucompp => {
            let Some((lhs, rhs, pops)) = comparison::fcom_registers(instr) else { return false };
            guard(u, 1 << lhs | 1 << rhs);
            u.push(Uop::FGet { x: X0, i: lhs as u8 });
            u.push(Uop::FGet { x: X1, i: rhs as u8 });
            u.push(Uop::FCom { a: X0, b: X1 });
            if pops > 0 {
                u.push(Uop::FPop { n: pops });
            }
        }
        Fnstsw_AX => {
            guard(u, 0);
            u.push(Uop::FStatus { t: T0 });
            u.push(Uop::Set { r: Gpr::word(0), t: T0 });
        }
        Fldcw_m2byte => {
            guard(u, 0);
            if mem(instr, T1, 2, false, u).is_none() {
                return false;
            }
            u.push(Uop::Load { dst: T0, m: T1, size: 2 });
            u.push(Uop::FSetControl { t: T0 });
        }
        Fnstcw_m2byte => {
            guard(u, 0);
            if mem(instr, T1, 2, true, u).is_none() {
                return false;
            }
            u.push(Uop::FGetControl { t: T0 });
            u.push(Uop::Store { m: T1, src: T0, size: 2 });
        }
        _ => return false,
    }
    true
}
