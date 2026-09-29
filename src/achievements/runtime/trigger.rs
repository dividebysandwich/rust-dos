//! Triggers: an achievement's logic, a core group of conditions and
//! alternatives (`S`) of which one must hold too.

use super::condset::{Condset, EvalState};
use super::memref::Memrefs;
use super::parse::{Cursor, Error, Parse};
use super::typed::{Kind, Oper, Typed};

/// A measured value not known yet.
pub const MEASURED_UNKNOWN: u32 = 0xFFFF_FFFF;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerState {
    Inactive,
    /// It can't trigger until it has been false for a frame.
    Waiting,
    Active,
    Paused,
    /// Its hits were reset (reported, not kept).
    Reset,
    Triggered,
    /// All but its Trigger conditions hold.
    Primed,
    Disabled,
}

impl TriggerState {
    pub fn is_active(self) -> bool {
        !matches!(
            self,
            TriggerState::Disabled | TriggerState::Inactive | TriggerState::Triggered
        )
    }
}

#[derive(Clone, Debug)]
pub struct Trigger {
    pub requirement: Option<Condset>,
    pub alternatives: Vec<Condset>,
    pub measured_value: u32,
    pub measured_target: u32,
    pub state: TriggerState,
    pub has_hits: bool,
    pub measured_as_percent: bool,
}

impl Trigger {
    pub fn parse(s: &mut Cursor, parse: &mut Parse) -> Result<Trigger, Error> {
        parse.measured_target = 0;
        parse.has_required_hits = false;
        parse.measured_as_percent = false;
        let requirement = if matches!(s.at(0), b's' | b'S') {
            None
        } else {
            Some(Condset::parse(s, parse)?)
        };
        let mut alternatives = Vec::new();
        while matches!(s.at(0), b's' | b'S') {
            s.skip(1);
            alternatives.push(Condset::parse(s, parse)?);
        }
        Ok(Trigger {
            requirement,
            alternatives,
            measured_value: if parse.measured_target != 0 {
                MEASURED_UNKNOWN
            } else {
                0
            },
            measured_target: parse.measured_target,
            state: TriggerState::Waiting,
            has_hits: false,
            measured_as_percent: parse.measured_as_percent,
        })
    }

    /// Whether it has no conditions at all.
    pub fn is_empty(&self) -> bool {
        self.requirement.is_none() && self.alternatives.is_empty()
    }

    fn reset_hits(&mut self) {
        if let Some(requirement) = &mut self.requirement {
            requirement.reset();
        }
        for alternative in &mut self.alternatives {
            alternative.reset();
        }
    }

    /// Back to waiting for its conditions to be false, without hits.
    pub fn reset(&mut self) {
        self.reset_hits();
        self.state = TriggerState::Waiting;
        if self.measured_target != 0 {
            self.measured_value = MEASURED_UNKNOWN;
        }
        self.has_hits = false;
    }

    /// Check it for this frame (the references read already). Returns its
    /// new state, or Reset when hits were reset.
    pub fn evaluate(&mut self, memrefs: &Memrefs) -> TriggerState {
        match self.state {
            TriggerState::Triggered | TriggerState::Disabled | TriggerState::Inactive => {
                return TriggerState::Inactive;
            }
            _ => {}
        }
        let mut eval = EvalState::default();
        let mut measured = Typed::NONE;
        let mut measured_from_hits = false;
        let (mut ret, mut is_paused, mut is_primed) = (true, false, true);
        if let Some(requirement) = &mut self.requirement {
            ret = requirement.test(&mut eval, memrefs);
            is_paused = eval.is_paused;
            is_primed = eval.is_primed;
            if eval.measured_value.kind != Kind::None {
                measured = eval.measured_value;
                measured_from_hits = eval.measured_from_hits;
            }
        }
        if !self.alternatives.is_empty() {
            let (mut sub, mut sub_paused, mut sub_primed) = (false, true, false);
            for alternative in &mut self.alternatives {
                sub |= alternative.test(&mut eval, memrefs);
                sub_paused &= eval.is_paused;
                sub_primed |= eval.is_primed;
                if eval.measured_value.kind != Kind::None
                    && (measured.kind == Kind::None
                        || eval.measured_value.compare(measured, Oper::Gt))
                {
                    measured = eval.measured_value;
                    measured_from_hits = eval.measured_from_hits;
                }
            }
            ret &= sub;
            is_primed &= sub_primed;
            // Paused if the core is, or every alternative.
            is_paused |= sub_paused;
        }
        if !is_paused && measured.kind != Kind::None {
            self.measured_value = measured.converted(Kind::Unsigned).bits;
        }
        if eval.was_reset {
            if measured_from_hits {
                self.measured_value = 0;
            } else if is_paused && self.measured_value != 0 {
                // A measured hit count in a paused group isn't flagged as
                // one: look for it.
                let from_hits = |set: &Condset, value: u32| {
                    set.conditions.iter().any(|c| {
                        c.ty == super::condition::CondType::Measured
                            && c.required_hits != 0
                            && c.current_hits == value
                    })
                };
                let value = self.measured_value;
                let in_requirement = self
                    .requirement
                    .as_ref()
                    .is_some_and(|r| r.is_paused && from_hits(r, value));
                if in_requirement
                    || self
                        .alternatives
                        .iter()
                        .any(|a| a.is_paused && from_hits(a, value))
                {
                    self.measured_value = 0;
                }
            }
            self.reset_hits();
            if self.has_hits {
                self.has_hits = false;
                if self.state == TriggerState::Primed {
                    self.state = TriggerState::Active;
                }
                return TriggerState::Reset;
            }
            eval.has_hits = false;
            is_primed = false;
        } else if ret {
            if self.state == TriggerState::Waiting {
                // True from the start: it waits to be false first.
                self.reset();
                self.has_hits = false;
                return TriggerState::Waiting;
            }
            self.state = TriggerState::Triggered;
            return TriggerState::Triggered;
        }
        self.has_hits = eval.has_hits;
        self.state = if is_paused {
            TriggerState::Paused
        } else if is_primed {
            TriggerState::Primed
        } else {
            TriggerState::Active
        };
        if eval.was_cond_reset {
            return TriggerState::Reset;
        }
        self.state
    }

    /// Check it as if active, as leaderboards and rich presence do;
    /// whether it holds.
    pub fn test(&mut self, memrefs: &Memrefs) -> bool {
        self.state = TriggerState::Active;
        self.evaluate(memrefs) == TriggerState::Triggered
    }
}
