//! Passes the model needn't run one by one (`counted_passes`).
//!
//! A loop that waits for the clock runs the same pass a thousand times
//! between two of the clock's milliseconds, and the model would run each of
//! them to find where one sets other flags. Where the counts move by the
//! same step in every pass, as a count of passes does, that is known
//! without running them: every value the pass works out is the one of the
//! pass before moved on by a step, and the flags a comparison sets stay as
//! they are until a value crosses a bound that one of them looks at.
//!
//! After a pass of the model, `Drift::of` runs it again from the cells as
//! they were at its start, and follows how each value moves from one pass
//! to the next, as long as what the pass reads of the clock stays the same:
//!
//! - a value read from the cells as they were at the pass's start moves as
//!   it moved between the start and the end of the pass;
//! - MOV, INC, DEC, ADD, SUB, ADC and SBB (with the carry the recorded pass
//!   had, as the model takes it), NEG and NOT work out a value that moves
//!   by a step made of their operands' steps: modulo 2 to the operand's
//!   bits, these are sums;
//! - AND, OR, XOR, MUL and DIV may only read values that don't move, but
//!   for AND and OR of a value with itself, which is that value;
//! - a value is read whole, or as its low bytes, from where it was
//!   written: bytes of two values, or a value's upper bytes alone, don't
//!   move by a step.
//!
//! Then the cells must end the pass as the steps found make them: each
//! value read from the start moves on by its step, so the passes after
//! it start from cells that moved on by the same step again. Anything else
//! leaves `of` with nothing, and the model runs the passes one by one as
//! before.
//!
//! The flags a comparison sets that something reads are the same in each
//! pass as long as none of these crosses a bound: each operand, taken as a number from 0, stays in the half of its
//! range it is in (so it neither wraps nor changes its sign); their sum or
//! difference, taken as a number with no wrap, stays between the same two
//! multiples of half the range and off a multiple of the whole range (CF,
//! SF and ZF); and the same of the operands taken with their signs (OF).
//! The values move by steps, so they cross a bound at a pass that is found
//! by a division. PF and AF are tied to the low bits, which change in
//! every pass a value moves: where something reads them, no pass is known
//! to run the same.

use super::{CountOp, Model, Run, State, read_operand, write_value};
use crate::cpu::alu::{AF, PF};

/// How a byte of the model's state at a place in the pass goes on from
/// pass to pass.
#[derive(Clone, Copy, PartialEq)]
enum Tag {
    /// It is the same in every pass (or holds no count, and reads as what
    /// the recorded pass found).
    Fixed,
    /// It is as it was at the pass's start, in slot `.0`.
    Start(usize),
    /// It is byte `byte` of the value `word` the pass works out.
    Of { word: usize, byte: u8 },
}

/// A value the pass works out: its size in bytes, what it was in the pass
/// `of` ran and by how much it moves in each pass after it, modulo 2 to
/// its bits.
struct Word {
    size: u8,
    value: u32,
    step: u32,
}

/// What `of` found of the passes after the one it ran.
pub(super) struct Drift {
    /// How each slot of the state at the end of a pass follows.
    tags: Vec<Tag>,
    words: Vec<Word>,
    /// The passes after the one ran, while the clock reads the same, that
    /// set the flags something reads as it did.
    pub(super) bound: u64,
}

impl Drift {
    /// The pass of `model` from `start` to `end`, reading the clock with
    /// `clock`, which set the flags it read as the recorded pass did: how
    /// the passes after it go on, if they move by steps.
    pub(super) fn of(
        model: &Model,
        start: &State,
        end: &State,
        clock: impl Fn(u64) -> u32,
    ) -> Option<Drift> {
        if start
            .iter()
            .zip(end)
            .any(|(a, b)| a.is_some() != b.is_some())
        {
            return None;
        }
        let mut state = start.clone();
        let mut tags: Vec<Tag> = (0..state.len())
            .map(|s| {
                if state[s].is_some() {
                    Tag::Start(s)
                } else {
                    Tag::Fixed
                }
            })
            .collect();
        let mut words = Vec::new();
        // The values read from the cells as they were at the start: their
        // slot, size and step.
        let mut from_start: Vec<(usize, u8, u32)> = Vec::new();
        let mut bound = u64::MAX;
        let mut index = 0;
        for run in &model.runs {
            match *run {
                Run::Kill(s) => {
                    state[s] = None;
                    tags[s] = Tag::Fixed;
                }
                Run::Clock { slot, offset } => {
                    write_value(&mut state, Some(slot), Some(clock(offset)), 4);
                    tags[slot..slot + 4].fill(Tag::Fixed);
                }
                Run::Op {
                    op,
                    size,
                    dst,
                    src,
                    hi,
                    carry,
                } => {
                    let mask = crate::cpu::alu::size_mask(size);
                    let [d, s, h] = [dst, src, hi].map(|o| read_operand(&state, o, size));
                    let (reads_dst, reads_src) = match op {
                        CountOp::Mov => (false, true),
                        CountOp::Inc | CountOp::Dec | CountOp::Neg | CountOp::Not => (true, false),
                        _ => (true, true),
                    };
                    let mut step = |o: (Option<usize>, u32), reads: bool| {
                        if !reads {
                            return Some(0);
                        }
                        let (step, whole) = step_of(&state, &tags, &words, o.0, size)?;
                        if !whole {
                            return Some(step);
                        }
                        // As it was at the start: it moves as it moved in
                        // the pass ran.
                        let moved = read_operand(end, o, size)
                            .wrapping_sub(read_operand(start, o, size))
                            & mask;
                        from_start.push((o.0?, size, moved));
                        Some(moved)
                    };
                    let sd = step(dst, reads_dst)?;
                    let ss = step(src, reads_src)?;
                    let sh = step(hi, op == CountOp::Div)?;
                    let (after, upper, _) = op.run(size, d, s, h, carry)?;
                    // The same value twice, as TEST EAX,EAX reads it.
                    let twice = dst.0.is_some() && dst == src && sd == ss;
                    let moved = sd | ss | sh != 0;
                    let after_step = match op {
                        CountOp::Mov => ss,
                        CountOp::Inc | CountOp::Dec => sd,
                        CountOp::Add | CountOp::Adc => sd.wrapping_add(ss),
                        CountOp::Sub | CountOp::Sbb | CountOp::Cmp => sd.wrapping_sub(ss),
                        CountOp::Neg | CountOp::Not => sd.wrapping_neg(),
                        CountOp::And | CountOp::Or | CountOp::Test if twice => sd,
                        _ if !moved => 0,
                        _ => return None,
                    } & mask;
                    // The flags read after it. Those it leaves the head
                    // come from the last pass alone, which the model runs.
                    let used = model.checks[index].0 & op.sets();
                    if used != 0 && moved {
                        let passes = if used & (PF | AF) != 0 {
                            0
                        } else {
                            match op {
                                CountOp::Inc => arith(size, (d, sd), (1, 0), 0, false),
                                CountOp::Dec => arith(size, (d, sd), (1, 0), 0, true),
                                CountOp::Add => arith(size, (d, sd), (s, ss), 0, false),
                                CountOp::Adc => arith(size, (d, sd), (s, ss), carry as i64, false),
                                CountOp::Sub | CountOp::Cmp => {
                                    arith(size, (d, sd), (s, ss), 0, true)
                                }
                                CountOp::Sbb => arith(size, (d, sd), (s, ss), carry as i64, true),
                                CountOp::Neg => arith(size, (0, 0), (d, sd), 0, true),
                                CountOp::And | CountOp::Or | CountOp::Test if twice => {
                                    logic(size, d, sd)
                                }
                                _ => 0,
                            }
                        };
                        bound = bound.min(passes);
                    }
                    if let (Some(slot), Some(value)) = (dst.0, after) {
                        write_value(&mut state, Some(slot), Some(value), size);
                        let tag = new_word(&mut words, size, value, after_step);
                        for i in 0..size {
                            tags[slot + i as usize] = tag(i);
                        }
                    }
                    if let (Some(slot), Some(value)) = (hi.0, upper) {
                        // MUL and DIV, whose operands don't move.
                        write_value(&mut state, Some(slot), Some(value), size);
                        tags[slot..slot + size as usize].fill(Tag::Fixed);
                    }
                    index += 1;
                }
            }
        }
        // The values read from the start must end as they began, moved on
        // by their step: then the next pass starts from them moved on by
        // the step again.
        for &(slot, size, moved) in &from_start {
            let (step, whole) = step_of(&state, &tags, &words, Some(slot), size)?;
            if !whole && step != moved {
                return None;
            }
        }
        Some(Drift { tags, words, bound })
    }

    /// The state at the end of the `j`th pass after the one `of` ran, from
    /// `end`, the state at its end.
    pub(super) fn after(&self, end: &State, j: u64) -> State {
        let mut state = end.clone();
        for (slot, tag) in self.tags.iter().enumerate() {
            if let (Tag::Of { word, byte }, Some(_)) = (*tag, state[slot]) {
                let w = &self.words[word];
                let value = w.value.wrapping_add(w.step.wrapping_mul(j as u32));
                state[slot] = Some((value >> (8 * byte)) as u8);
            }
        }
        state
    }
}

/// The tags of a value of `size` bytes worked out to `value`, moving by
/// `step` in each pass: none where it doesn't move.
fn new_word(words: &mut Vec<Word>, size: u8, value: u32, step: u32) -> impl Fn(u8) -> Tag {
    let word = (step != 0).then(|| {
        words.push(Word { size, value, step });
        words.len() - 1
    });
    move |byte| word.map_or(Tag::Fixed, |word| Tag::Of { word, byte })
}

/// How the `size` bytes from `slot` move from pass to pass, if they move
/// by a step: the step, and whether they are as they were at the pass's
/// start (whose step `of` takes from the pass it ran).
fn step_of(
    state: &State,
    tags: &[Tag],
    words: &[Word],
    slot: Option<usize>,
    size: u8,
) -> Option<(u32, bool)> {
    let Some(slot) = slot else {
        return Some((0, false));
    };
    let tag = |i: usize| match state[slot + i] {
        None => Tag::Fixed,
        Some(_) => tags[slot + i],
    };
    let n = size as usize;
    if (0..n).all(|i| tag(i) == Tag::Fixed) {
        return Some((0, false));
    }
    if (0..n).all(|i| tag(i) == Tag::Start(slot + i)) {
        return Some((0, true));
    }
    match tag(0) {
        Tag::Of { word, byte: 0 }
            if words[word].size >= size
                && (1..n).all(|i| {
                    tag(i)
                        == Tag::Of {
                            word,
                            byte: i as u8,
                        }
                }) =>
        {
            Some((words[word].step & crate::cpu::alu::size_mask(size), false))
        }
        _ => None,
    }
}

/// A step modulo 2 to `bits`, as a signed number.
fn signed(step: u32, bits: u32) -> i64 {
    let shift = 64 - bits;
    ((step as i64) << shift) >> shift
}

/// The passes after the one ran (whose values are 0 passes on) for which
/// `x`, moving by `step`, stays between the same two multiples of `half`,
/// and off a multiple of `whole` where `whole` is given.
fn stays(x: i64, step: i64, half: i64, whole: Option<i64>) -> u64 {
    if step == 0 {
        return u64::MAX;
    }
    let mut low = x.div_euclid(half) * half;
    let high = low + half - 1;
    if whole.is_some_and(|whole| low.rem_euclid(whole) == 0) {
        low += 1;
    }
    if x < low {
        // On a multiple of `whole`, which the next step leaves.
        return 0;
    }
    let room = if step > 0 { high - x } else { x - low };
    if room < 0 {
        0
    } else {
        (room / step.abs()) as u64
    }
}

/// The passes for which an addition (or with `sub` a subtraction) of
/// `size` bytes of `a` and `b`, each a value and its step, with `carry`,
/// sets CF, ZF, SF and OF as it did.
fn arith(size: u8, a: (u32, u32), b: (u32, u32), carry: i64, sub: bool) -> u64 {
    let bits = 8 * size as u32;
    let half = 1i64 << (bits - 1);
    let (sa, sb) = (signed(a.1, bits), signed(b.1, bits));
    let (va, vb) = (a.0 as i64, b.0 as i64);
    let (wa, wb) = (signed(a.0, bits), signed(b.0, bits));
    let (t, u, step) = if sub {
        (va - vb - carry, wa - wb - carry, sa - sb)
    } else {
        (va + vb + carry, wa + wb + carry, sa + sb)
    };
    [
        stays(va, sa, half, None),
        stays(vb, sb, half, None),
        stays(t, step, half, Some(2 * half)),
        stays(u, step, half, None),
    ]
    .into_iter()
    .min()
    .unwrap_or(0)
}

/// The passes for which a value of `size` bytes, `x` moving by `step`,
/// gives AND, OR and TEST of it with itself the ZF and SF they had.
fn logic(size: u8, x: u32, step: u32) -> u64 {
    let bits = 8 * size as u32;
    let half = 1i64 << (bits - 1);
    stays(x as i64, signed(step, bits), half, Some(2 * half))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comparison_holds_until_the_count_crosses_a_bound() {
        // CMP of a dword count at 5 that goes up by one with 1: the flags
        // stay until the count wraps past the sign bit.
        assert_eq!(arith(4, (5, 1), (1, 0), 0, true), 0x7FFF_FFFF - 5);
        // Going down from 5 towards 1, ZF comes at the 4th pass on.
        assert_eq!(arith(4, (5, u32::MAX), (1, 0), 0, true), 3);
        // A count equal to what it is compared with sets ZF only there.
        assert_eq!(arith(4, (1, 1), (1, 0), 0, true), 0);
        // A byte count going up from 0x70 changes SF at 0x80.
        assert_eq!(arith(1, (0x70, 1), (0, 0), 0, true), 0x0F);
        // TEST of a count with itself: ZF and SF hold from 1 up.
        assert_eq!(logic(2, 1, 1), 0x7FFE);
        assert_eq!(logic(2, 0, 1), 0);
    }

    #[test]
    fn a_sum_crossing_the_range_carries() {
        // 0xF0 + a count from 0x08 going up by 1: the sum reaches 0x100,
        // which sets CF and ZF, at the 8th pass on.
        assert_eq!(arith(1, (0xF0, 0), (0x08, 1), 0, false), 7);
    }
}
