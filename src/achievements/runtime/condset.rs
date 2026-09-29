//! Condition sets: a trigger's core or one of its alternatives, or one of
//! a value's clauses. Conditions are checked in groups, whatever their
//! order in the definition: PauseIfs first, then ResetIfs, those with hit
//! targets, Measured ones, and the rest, each with the flags that chain
//! into it (AndNext, AddHits and so on) before it.

use super::condition::{CondType, Condition};
use super::memref::Memrefs;
use super::operand::{Operand, OperandType};
use super::parse::{Cursor, Error, Parse};
use super::typed::{Kind, Typed};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Class {
    Pause,
    Reset,
    HitTarget,
    Measured,
    Other,
    /// Worked out by derived references, not checked.
    Indirect,
    /// Feeds the next condition that isn't one.
    Combining,
}

fn classify(condition: &Condition) -> Class {
    match condition.ty {
        CondType::PauseIf => Class::Pause,
        CondType::ResetIf => Class::Reset,
        CondType::AddAddress | CondType::AddSource | CondType::SubSource => Class::Indirect,
        CondType::AddHits
        | CondType::AndNext
        | CondType::OrNext
        | CondType::Remember
        | CondType::ResetNextIf
        | CondType::SubHits => Class::Combining,
        CondType::Measured | CondType::MeasuredIf => Class::Measured,
        _ if condition.required_hits != 0 => Class::HitTarget,
        _ => Class::Other,
    }
}

#[derive(Clone, Debug, Default)]
pub struct Condset {
    /// In the order they are checked: the groups one after another.
    pub conditions: Vec<Condition>,
    /// The indices of `conditions` in the definition's order.
    pub order: Vec<usize>,
    pub num_pause: usize,
    pub num_reset: usize,
    pub num_hittarget: usize,
    pub num_measured: usize,
    pub num_other: usize,
    pub is_paused: bool,
}

/// What checking conditions works out.
#[derive(Clone, Copy, Debug, Default)]
pub struct EvalState {
    pub measured_value: Typed,
    pub add_hits: i32,
    pub is_true: bool,
    pub is_primed: bool,
    pub is_paused: bool,
    pub can_measure: bool,
    pub measured_from_hits: bool,
    pub and_next: bool,
    pub or_next: bool,
    pub reset_next: bool,
    pub stop_processing: bool,
    pub has_hits: bool,
    pub was_reset: bool,
    pub was_cond_reset: bool,
    pub can_short_circuit: bool,
}

impl Condset {
    /// Conditions separated by `_`, up to an `S` (or the end).
    pub fn parse(s: &mut Cursor, parse: &mut Parse) -> Result<Condset, Error> {
        if matches!(s.at(0), b'S' | b's' | 0) {
            // An empty group, which the editor allows.
            return Ok(Condset::default());
        }
        // The whole group is parsed first, as rcheevos counts its
        // conditions: a condition that doesn't parse is reported before
        // what is wrong with those before it.
        {
            let mut scratch = Memrefs::default();
            let mut probe = Parse::new(&mut scratch);
            let mut cursor = *s;
            loop {
                Condition::parse(&mut cursor, &mut probe)?;
                if cursor.take() != b'_' {
                    break;
                }
            }
        }
        parse.addsource_oper = super::typed::Oper::None;
        parse.addsource_parent = Operand::NONE;
        parse.indirect_parent = Operand::NONE;
        // Each group recalls its own.
        parse.remember = Operand::NONE;

        let mut parsed = Vec::new();
        let mut measured_target = 0u32;
        loop {
            let mut condition = Condition::parse(s, parse)?;
            if condition.oper == super::typed::Oper::None {
                match condition.ty {
                    CondType::AddAddress
                    | CondType::AddSource
                    | CondType::SubSource
                    | CondType::Remember => {}
                    CondType::Measured if parse.is_value => {}
                    _ => return Err(Error::InvalidOperator),
                }
            }
            match condition.ty {
                CondType::Measured => {
                    if measured_target != 0 {
                        return Err(Error::MultipleMeasured);
                    } else if parse.is_value {
                        measured_target = u32::MAX;
                        if !condition.oper.is_modifying() {
                            // Measuring a comparison in a value counts hits.
                            condition.required_hits = measured_target;
                        }
                    } else if condition.required_hits != 0 {
                        measured_target = condition.required_hits;
                    } else if condition.operand2.ty == OperandType::Const {
                        measured_target = condition.operand2.num;
                    } else if condition.operand2.ty == OperandType::Fp {
                        measured_target = super::typed::float_to_u32(condition.operand2.dbl);
                    } else {
                        return Err(Error::InvalidMeasuredTarget);
                    }
                    if parse.measured_target != 0 && measured_target != parse.measured_target {
                        return Err(Error::MultipleMeasured);
                    }
                    parse.measured_target = measured_target;
                }
                CondType::Standard | CondType::Trigger if parse.is_value => {
                    return Err(Error::InvalidValueFlag);
                }
                _ => {}
            }
            condition.update_parse_state(parse);
            parsed.push(condition);
            if s.at(0) != b'_' {
                break;
            }
            s.skip(1);
        }

        // Flags take the group of the condition they chain into.
        let raw: Vec<Class> = parsed.iter().map(classify).collect();
        let mut classes = raw.clone();
        for i in 0..raw.len() {
            if raw[i] == Class::Combining {
                classes[i] = raw[i + 1..]
                    .iter()
                    .copied()
                    .find(|c| !matches!(c, Class::Combining | Class::Indirect))
                    .unwrap_or(Class::Other);
            }
        }
        let mut indices: Vec<usize> = (0..parsed.len()).collect();
        indices.sort_by_key(|&i| classes[i]);
        let count = |class: Class| classes.iter().filter(|&&c| c == class).count();
        let mut order = vec![0; parsed.len()];
        for (position, &i) in indices.iter().enumerate() {
            order[i] = position;
        }
        let mut slots: Vec<Option<Condition>> = parsed.into_iter().map(Some).collect();
        let conditions = indices.iter().map(|&i| slots[i].take().unwrap()).collect();
        let mut set = Condset {
            conditions,
            order,
            num_pause: count(Class::Pause),
            num_reset: count(Class::Reset),
            num_hittarget: count(Class::HitTarget),
            num_measured: count(Class::Measured),
            num_other: count(Class::Other),
            is_paused: false,
        };
        if set.num_pause > 0 && parse.remember.is_set() {
            set.update_pause_remember(parse.memrefs);
        }
        Ok(set)
    }

    /// The conditions in the definition's order.
    pub fn in_order(&self) -> impl Iterator<Item = &Condition> {
        self.order.iter().map(|&i| &self.conditions[i])
    }

    /// PauseIfs are checked first, so a `{recall}` in them can't read what
    /// a Remember after them keeps; and the rest read what the PauseIfs'
    /// last Remember kept if nothing else is kept for them.
    fn update_pause_remember(&mut self, memrefs: &mut Memrefs) {
        let mut pause_remember: Option<Operand> = None;
        for condition in &mut self.conditions[..self.num_pause] {
            if condition.ty == CondType::Remember {
                pause_remember = Some(condition.operand1);
            } else if pause_remember.is_none() {
                for operand in [&mut condition.operand1, &mut condition.operand2] {
                    if operand.ty == OperandType::Recall && operand.access.is_memref() {
                        operand.memref = None;
                    }
                }
            }
        }
        let Some(remember) = pause_remember else {
            return;
        };
        for i in 0..self.order.len() {
            let at = self.order[i];
            let condition = &mut self.conditions[at];
            if at >= self.num_pause {
                update_recall(&mut condition.operand1, &remember, memrefs);
                update_recall(&mut condition.operand2, &remember, memrefs);
            }
            if condition.ty == CondType::Remember {
                break;
            }
        }
    }

    pub fn reset(&mut self) {
        for condition in &mut self.conditions {
            condition.current_hits = 0;
        }
    }

    /// Check the group; true if it holds.
    pub fn test(&mut self, eval: &mut EvalState, memrefs: &Memrefs) -> bool {
        eval.measured_value = Typed::NONE;
        eval.add_hits = 0;
        eval.is_true = true;
        eval.is_primed = true;
        eval.is_paused = false;
        eval.can_measure = true;
        eval.measured_from_hits = false;
        eval.and_next = true;
        eval.or_next = false;
        eval.reset_next = false;
        eval.stop_processing = false;

        let mut at = 0;
        if self.num_pause > 0 {
            test_range(&mut self.conditions[..self.num_pause], eval, memrefs, true);
            self.is_paused = eval.is_paused;
            if self.is_paused {
                return false;
            }
            at += self.num_pause;
        }
        if self.num_reset > 0 {
            let short = eval.can_short_circuit;
            test_range(
                &mut self.conditions[at..at + self.num_reset],
                eval,
                memrefs,
                short,
            );
            at += self.num_reset;
        }
        if self.num_hittarget > 0 {
            // Every frame, unless their hits are about to go.
            if !eval.was_reset {
                test_range(
                    &mut self.conditions[at..at + self.num_hittarget],
                    eval,
                    memrefs,
                    false,
                );
            }
            at += self.num_hittarget;
        }
        if self.num_measured > 0 {
            let range = at..at + self.num_measured;
            // Their hits go before they are checked, so the measured value
            // is right.
            if eval.was_reset {
                for condition in &mut self.conditions[range.clone()] {
                    condition.current_hits = 0;
                }
            }
            test_range(&mut self.conditions[range], eval, memrefs, false);
            at += self.num_measured;
            if eval.measured_value.kind != Kind::None
                && (!eval.can_measure || (eval.measured_from_hits && eval.was_reset))
            {
                eval.measured_value = Typed::unsigned(0);
            }
        }
        if self.num_other > 0 {
            let short = eval.can_short_circuit;
            if eval.is_true || (!short && !eval.was_reset) {
                test_range(
                    &mut self.conditions[at..at + self.num_other],
                    eval,
                    memrefs,
                    short,
                );
            }
        }
        eval.is_true
    }
}

/// Point an unresolved `{recall}` at `remember`, in derived references
/// too.
fn update_recall(operand: &mut Operand, remember: &Operand, memrefs: &mut Memrefs) {
    if operand.ty == OperandType::Recall {
        if operand.access.is_memref() && operand.memref.is_none() {
            *operand = Operand {
                access: remember.ty,
                ty: OperandType::Recall,
                ..*remember
            };
        }
    } else if operand.is_memref()
        && let Some(id) = operand.memref
        && let super::memref::Source::Derived(derived) = &memrefs.items[id].source
    {
        let (mut parent, mut modifier) = (derived.parent, derived.modifier);
        update_recall(&mut parent, remember, memrefs);
        update_recall(&mut modifier, remember, memrefs);
        if let super::memref::Source::Derived(derived) = &mut memrefs.items[id].source {
            derived.parent = parent;
            derived.modifier = modifier;
        }
    }
}

/// The condition's own hit logic, without AddHits.
fn evaluate_no_add_hits(
    condition: &mut Condition,
    eval: &mut EvalState,
    memrefs: &Memrefs,
) -> bool {
    let mut valid = condition.test(memrefs);
    condition.is_true = valid as u8;
    if eval.reset_next {
        // A ResetNextIf before: no hits, and not true.
        eval.was_cond_reset |= condition.current_hits != 0;
        condition.current_hits = 0;
        valid = false;
    } else {
        valid &= eval.and_next;
        valid |= eval.or_next;
        if valid {
            eval.has_hits = true;
            if condition.required_hits == 0 {
                condition.current_hits = condition.current_hits.wrapping_add(1);
            } else if condition.current_hits < condition.required_hits {
                condition.current_hits += 1;
                valid = condition.current_hits == condition.required_hits;
            }
        } else if condition.current_hits > 0 {
            eval.has_hits = true;
            valid = condition.current_hits == condition.required_hits;
        }
    }
    eval.and_next = true;
    eval.or_next = false;
    valid
}

/// The hits with those AddHits and SubHits brought.
fn total_hits(condition: &Condition, eval: &mut EvalState) -> u32 {
    let mut total = condition.current_hits;
    if condition.required_hits != 0 {
        let signed = (condition.current_hits as i32).wrapping_add(eval.add_hits);
        total = signed.max(0) as u32;
    }
    eval.add_hits = 0;
    total
}

fn evaluate(condition: &mut Condition, eval: &mut EvalState, memrefs: &Memrefs) -> bool {
    let mut valid = evaluate_no_add_hits(condition, eval, memrefs);
    if eval.add_hits != 0 && condition.required_hits != 0 {
        valid = total_hits(condition, eval) >= condition.required_hits;
    }
    eval.reset_next = false;
    valid
}

fn test_range(
    conditions: &mut [Condition],
    eval: &mut EvalState,
    memrefs: &Memrefs,
    can_short_circuit: bool,
) {
    for condition in conditions {
        match condition.ty {
            CondType::Standard => {
                let valid = evaluate(condition, eval, memrefs);
                eval.is_true &= valid;
                eval.is_primed &= valid;
                if !valid && eval.can_short_circuit {
                    eval.stop_processing = true;
                }
            }
            CondType::PauseIf => {
                if evaluate(condition, eval, memrefs) {
                    eval.is_paused = true;
                    eval.is_true = false;
                    eval.is_primed = false;
                    eval.stop_processing = true;
                } else if condition.required_hits == 0 {
                    // Not true, and no hit target: it didn't match.
                    condition.current_hits = 0;
                }
            }
            CondType::ResetIf => {
                if evaluate(condition, eval, memrefs) {
                    condition.is_true |= 0x02;
                    eval.is_true = false;
                    eval.is_primed = false;
                    eval.was_reset = true;
                    eval.stop_processing = true;
                }
            }
            CondType::Trigger => {
                let valid = evaluate(condition, eval, memrefs);
                eval.is_true &= valid;
            }
            CondType::Measured => {
                if condition.required_hits == 0 {
                    let valid = evaluate(condition, eval, memrefs);
                    eval.is_true &= valid;
                    eval.is_primed &= valid;
                    if !valid && eval.can_short_circuit {
                        eval.stop_processing = true;
                    }
                    // Without a hit target, it measures its left side.
                    eval.measured_value = condition.operand1.evaluate(memrefs);
                    eval.measured_from_hits = false;
                } else {
                    evaluate_no_add_hits(condition, eval, memrefs);
                    let total = total_hits(condition, eval);
                    let valid = total >= condition.required_hits;
                    eval.is_true &= valid;
                    eval.is_primed &= valid;
                    eval.measured_value = Typed::unsigned(total);
                    eval.measured_from_hits = true;
                    eval.reset_next = false;
                }
            }
            CondType::MeasuredIf => {
                let valid = evaluate(condition, eval, memrefs);
                eval.is_true &= valid;
                eval.is_primed &= valid;
                eval.can_measure &= valid;
            }
            CondType::AddSource
            | CondType::SubSource
            | CondType::AddAddress
            | CondType::Remember => {}
            CondType::AddHits => {
                evaluate_no_add_hits(condition, eval, memrefs);
                eval.add_hits = eval.add_hits.wrapping_add(condition.current_hits as i32);
                eval.reset_next = false;
            }
            CondType::SubHits => {
                evaluate_no_add_hits(condition, eval, memrefs);
                eval.add_hits = eval.add_hits.wrapping_sub(condition.current_hits as i32);
                eval.reset_next = false;
            }
            CondType::ResetNextIf => {
                eval.reset_next = evaluate_no_add_hits(condition, eval, memrefs)
            }
            CondType::AndNext => eval.and_next = evaluate_no_add_hits(condition, eval, memrefs),
            CondType::OrNext => eval.or_next = evaluate_no_add_hits(condition, eval, memrefs),
        }
        if eval.stop_processing && can_short_circuit {
            break;
        }
    }
}
