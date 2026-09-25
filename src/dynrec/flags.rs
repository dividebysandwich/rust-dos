//! Which arithmetic flags translated code must compute. A flag an operation
//! sets is dead if a later one in the block sets it again before anything
//! reads it: a condition, a carry in, or anything outside the block, which
//! sees all of them wherever the block may leave (its exits, a fault, a
//! handler's call, a store into the block's later bytes). The code
//! generators skip computing dead flags.

use super::uop::*;
use crate::cpu::alu::{ARITH, CF, OF, PF, SF, ShiftOp, ZF};
use iced_x86::ConditionCode;

const SZP: u32 = SF | ZF | PF;

/// The flags condition `cc` reads.
pub fn cond_flags(cc: ConditionCode) -> u32 {
    use ConditionCode as C;
    match cc {
        C::o | C::no => OF,
        C::b | C::ae => CF,
        C::e | C::ne => ZF,
        C::be | C::a => CF | ZF,
        C::s | C::ns => SF,
        C::p | C::np => PF,
        C::l | C::ge => SF | OF,
        C::le | C::g => ZF | SF | OF,
        C::None => 0,
    }
}

/// The flags a shift or rotate by a count other than 0 sets: a 386 sets
/// AF for SHL and SHR, and leaves it for SAR.
pub fn shift_flags(op: ShiftOp) -> u32 {
    match op {
        ShiftOp::Shl | ShiftOp::Shr => ARITH,
        ShiftOp::Sar => CF | OF | SZP,
        _ => CF | OF,
    }
}

impl Uop {
    /// The arithmetic flags the operation sets, whatever they were.
    pub fn flags_set(&self) -> u32 {
        match *self {
            Uop::Alu { .. } => ARITH,
            Uop::Unary { op: UnOp::Inc | UnOp::Dec, .. } => ARITH & !CF,
            Uop::Unary { op: UnOp::Neg, .. } => ARITH,
            Uop::Shift { op, .. } | Uop::ShiftVar { op, .. } => shift_flags(op),
            Uop::DoubleShift { .. } | Uop::DoubleShiftVar { .. } => CF | OF | SZP,
            Uop::Imul { .. } | Uop::MulWide { .. } => CF | OF,
            Uop::Flag { mask, .. } => mask & ARITH,
            _ => 0,
        }
    }

    /// The arithmetic flags that must be right before the operation: those
    /// it reads, and all of them where it may leave the block.
    pub fn flags_used(&self) -> u32 {
        match *self {
            Uop::Alu { op: AluOp::Adc | AluOp::Sbb, .. } => CF,
            Uop::Flag { mask, set: None } => mask & ARITH,
            Uop::SetCond { cc, .. } => cond_flags(cc),
            // A count of 0 leaves the flags as they were.
            Uop::ShiftVar { op, .. } => shift_flags(op),
            Uop::DoubleShiftVar { .. } => CF | OF | SZP,
            Uop::Bail { .. } => ARITH,
            Uop::MemRef { .. } | Uop::CheckLimit { .. } | Uop::DivWide { .. } | Uop::Exit { .. } | Uop::ExitIf { .. } => {
                ARITH
            }
            _ => 0,
        }
    }
}

/// The arithmetic flags live after each operation of each instruction
/// (`items[ix]`, None for one its handler runs).
pub fn live(items: &[Option<Vec<Uop>>]) -> Vec<Vec<u32>> {
    let mut out: Vec<Vec<u32>> = items.iter().map(|i| vec![0; i.as_ref().map_or(0, |u| u.len())]).collect();
    // The block's end is an exit.
    let mut live = ARITH;
    for (ix, item) in items.iter().enumerate().rev() {
        let Some(uops) = item else {
            // A handler may read any flag.
            live = ARITH;
            continue;
        };
        if uops.iter().any(|u| matches!(u, Uop::Store { .. })) {
            // The block may stop after a store into its later bytes.
            live = ARITH;
        }
        for (k, uop) in uops.iter().enumerate().rev() {
            out[ix][k] = live;
            live = (live & !uop.flags_set()) | uop.flags_used();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::Seg;

    fn alu(op: AluOp) -> Uop {
        Uop::Alu { op, size: 4, a: T0, b: Src::T(T1) }
    }

    #[test]
    fn flags_set_again_before_they_are_read_are_dead() {
        let shr = Uop::Shift { op: ShiftOp::Shr, size: 4, t: T0, count: 26 };
        let shld = Uop::DoubleShift { left: true, size: 2, dst: T0, src: T1, count: 6 };
        let items = vec![Some(vec![shr]), Some(vec![shld]), Some(vec![alu(AluOp::Add)])];
        // SHLD leaves AF, which the ADD sets; the block's end reads all.
        assert_eq!(live(&items), vec![vec![0], vec![0], vec![ARITH]]);
    }

    #[test]
    fn a_partial_setter_keeps_the_flags_it_leaves_alive() {
        let inc = Uop::Unary { op: UnOp::Inc, size: 4, t: T0 };
        let adc = alu(AluOp::Adc);
        // ADC reads the SUB's CF past the INC.
        let items = vec![Some(vec![alu(AluOp::Sub)]), Some(vec![inc]), Some(vec![adc])];
        assert_eq!(live(&items), vec![vec![CF], vec![CF], vec![ARITH]]);
    }

    #[test]
    fn where_the_block_may_leave_all_flags_are_live() {
        let memref = Uop::MemRef { t: T2, seg: Seg::DS, size: 4, write: false, slot: 0 };
        let store = Uop::Store { m: T2, src: T0, size: 4 };
        let items = vec![
            Some(vec![alu(AluOp::Add)]),
            Some(vec![memref]),
            Some(vec![alu(AluOp::Add)]),
            Some(vec![store]),
            Some(vec![alu(AluOp::Add)]),
            None,
            Some(vec![alu(AluOp::Add)]),
        ];
        let live = live(&items);
        assert_eq!(live[0], vec![ARITH], "a fault");
        assert_eq!(live[2], vec![ARITH], "a store into the block");
        assert_eq!(live[4], vec![ARITH], "a handler");
        assert_eq!(live[6], vec![ARITH], "the end");
    }
}
