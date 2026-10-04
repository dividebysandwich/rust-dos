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
            // As a 486 leaves them (instructions/arith.rs `division_flags`).
            Uop::DivWide { .. } => ARITH,
            Uop::Flag { mask, .. } => mask & ARITH,
            _ => 0,
        }
    }

    /// The arithmetic flags that must be right before the operation: those
    /// it reads, and all of them where it may leave the block.
    pub fn flags_used(&self) -> u32 {
        match *self {
            Uop::Alu { op: AluOp::Adc | AluOp::Sbb, .. } => CF,
            Uop::Shift { op: ShiftOp::Rcl | ShiftOp::Rcr, .. } => CF,
            Uop::Flag { mask, set: None } => mask & ARITH,
            Uop::SetCond { cc, .. } => cond_flags(cc),
            // A count of 0 leaves the flags as they were.
            Uop::ShiftVar { op, .. } => shift_flags(op),
            Uop::DoubleShiftVar { .. } => CF | OF | SZP,
            Uop::Bail { .. } | Uop::RepStart { .. } | Uop::Forward | Uop::FpuGuard { .. } => ARITH,
            Uop::MemRef { .. }
            | Uop::LoadSeg { .. }
            | Uop::In { .. }
            | Uop::Out { .. }
            | Uop::Sti
            | Uop::CheckLimit { .. }
            | Uop::CheckIopl
            | Uop::DivWide { .. }
            | Uop::Exit { .. }
            | Uop::ExitIf { .. } => ARITH,
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

/// Which flags translated code computes, and where it records an
/// operation's operands instead (see `Plan::record`).
pub struct Plan {
    /// The flags live after each operation (`items[ix][k]`).
    pub live: Vec<Vec<u32>>,
    /// Operations that compute only the flags something in the block
    /// reads, and record their operands (`JitCtx::lazy`) for the ways out
    /// that see all the flags: a fault, and a store into the block's later
    /// bytes. The flags come from the record there (`jit_lazy_flags`).
    pub record: Vec<Vec<bool>>,
    /// Per instruction, whether the flags as it starts are a recorded
    /// operation's (for its faults), and as it ends (for a store into the
    /// block's later bytes).
    pub lazy_start: Vec<bool>,
    pub lazy_end: Vec<bool>,
}

/// What `jit_lazy_flags` works the flags out from: the operation.
pub const LAZY_ADD: u32 = 0;
pub const LAZY_SUB: u32 = 1;
pub const LAZY_AND: u32 = 2;
pub const LAZY_OR: u32 = 3;
pub const LAZY_XOR: u32 = 4;
pub const LAZY_NEG: u32 = 5;

impl Uop {
    /// The operation `jit_lazy_flags` can work the flags of out from its
    /// operands, if it is one.
    pub fn lazy_kind(&self) -> Option<u32> {
        match *self {
            Uop::Alu { op, .. } => match op {
                AluOp::Add => Some(LAZY_ADD),
                AluOp::Sub | AluOp::Cmp => Some(LAZY_SUB),
                AluOp::And | AluOp::Test => Some(LAZY_AND),
                AluOp::Or => Some(LAZY_OR),
                AluOp::Xor => Some(LAZY_XOR),
                AluOp::Adc | AluOp::Sbb => None,
            },
            Uop::Unary { op: UnOp::Neg, .. } => Some(LAZY_NEG),
            _ => None,
        }
    }

    /// Whether the operation sets all the arithmetic flags, whatever its
    /// operands.
    fn sets_all(&self) -> bool {
        matches!(self, Uop::Alu { .. } | Uop::Unary { op: UnOp::Neg, .. })
    }
}

/// The flags plan of a block: `live`, but where an operation's flags are
/// live only for a fault or a store into the block's later bytes (before
/// an operation that sets them all again), recorded instead of computed.
pub fn plan(items: &[Option<Vec<Uop>>]) -> Plan {
    let mut out = live(items);
    // The flags something reads after each operation, not counting the
    // ways out that see them all.
    let mut real: Vec<Vec<u32>> = items.iter().map(|i| vec![0; i.as_ref().map_or(0, |u| u.len())]).collect();
    let mut live_now = ARITH;
    for (ix, item) in items.iter().enumerate().rev() {
        let Some(uops) = item else {
            live_now = ARITH;
            continue;
        };
        for (k, uop) in uops.iter().enumerate().rev() {
            real[ix][k] = live_now;
            let used = match uop {
                Uop::MemRef { .. } | Uop::CheckLimit { .. } => 0,
                _ => uop.flags_used(),
            };
            live_now = (live_now & !uop.flags_set()) | used;
        }
    }
    // The operations, in order, and whether the next one that sets flags
    // sets them all (a handler's call counts as one that doesn't).
    let positions: Vec<(usize, usize)> = items
        .iter()
        .enumerate()
        .flat_map(|(ix, item)| match item {
            Some(uops) => (0..uops.len()).map(|k| (ix, k)).collect::<Vec<_>>(),
            None => vec![(ix, usize::MAX)],
        })
        .collect();
    let uop = |(ix, k): (usize, usize)| items[ix].as_ref().filter(|_| k != usize::MAX).map(|u| &u[k]);
    let mut record: Vec<Vec<bool>> = items.iter().map(|i| vec![false; i.as_ref().map_or(0, |u| u.len())]).collect();
    for (n, &(ix, k)) in positions.iter().enumerate() {
        let Some(u) = uop((ix, k)) else { continue };
        if u.lazy_kind().is_none() || out[ix][k] & ARITH == real[ix][k] & ARITH {
            continue;
        }
        let next = positions[n + 1..].iter().find_map(|&p| match uop(p) {
            None => Some(false),
            Some(u) if u.flags_set() != 0 => Some(u.sets_all()),
            Some(_) => None,
        });
        if next == Some(true) {
            record[ix][k] = true;
            out[ix][k] = real[ix][k];
        }
    }
    // Where the flags are a recorded operation's.
    let (mut lazy_start, mut lazy_end) = (vec![false; items.len()], vec![false; items.len()]);
    let mut pending = false;
    for (ix, item) in items.iter().enumerate() {
        lazy_start[ix] = pending;
        match item {
            None => pending = false,
            Some(uops) => {
                for (k, u) in uops.iter().enumerate() {
                    if u.flags_set() != 0 {
                        pending = record[ix][k];
                    }
                }
            }
        }
        lazy_end[ix] = pending;
    }
    Plan { live: out, record, lazy_start, lazy_end }
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

    #[test]
    fn flags_live_only_for_a_fault_are_recorded() {
        let memref = Uop::MemRef { t: T2, seg: Seg::DS, size: 4, write: false, slot: 0 };
        let store = Uop::Store { m: T2, src: T0, size: 4 };
        let inc = Uop::Unary { op: UnOp::Inc, size: 4, t: T0 };
        let items = vec![
            Some(vec![alu(AluOp::Add)]),
            Some(vec![memref]),
            Some(vec![alu(AluOp::Sub), store]),
            Some(vec![alu(AluOp::Xor)]),
            Some(vec![alu(AluOp::Cmp)]),
            Some(vec![memref]),
            Some(vec![inc]),
        ];
        let p = plan(&items);
        // The ADD's flags matter only for the MemRef's fault, until the
        // SUB sets them all again: recorded, none computed.
        assert!(p.record[0][0]);
        assert_eq!(p.live[0], vec![0]);
        assert!(p.lazy_start[1] && p.lazy_end[1]);
        // The SUB's for the store into the block, until the XOR; its
        // instruction starts with the ADD's.
        assert!(p.record[2][0] && p.lazy_start[2] && p.lazy_end[2]);
        // The XOR's flags die at the CMP, which no one reads: nothing to
        // record.
        assert!(!p.record[3][0]);
        // The CMP's go on past the INC, which leaves CF: computed.
        assert!(!p.record[4][0]);
        assert_eq!(p.live[4][0] & CF, CF);
        assert!(!p.lazy_start[5]);
    }
}
