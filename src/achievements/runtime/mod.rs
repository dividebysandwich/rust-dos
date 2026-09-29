//! The achievement logic, as RetroAchievements defines it and rcheevos
//! runs it: triggers of conditions on memory, checked once a frame;
//! leaderboards that start, run and submit a value; and the rich
//! presence line. A port of rcheevos's runtime, so definitions behave as
//! they do in the emulators the sets are made with.

pub mod condition;
pub mod condset;
pub mod format;
pub mod lboard;
pub mod memref;
pub mod operand;
pub mod parse;
pub mod richpresence;
pub mod trigger;
pub mod typed;
pub mod value;

pub use format::Format;
pub use memref::Peek;
pub use parse::Error;

use lboard::{Lboard, LboardState};
use memref::Memrefs;
use parse::{Cursor, Parse};
use richpresence::RichPresence;
use trigger::{MEASURED_UNKNOWN, Trigger, TriggerState};

/// An achievement's trigger, or a leaderboard, by its id, parsed from its
/// definition; one that can't be parsed stays out.
pub fn parse_trigger(definition: &str, memrefs: &mut Memrefs) -> Result<Trigger, Error> {
    let mut parse = Parse::new(memrefs);
    Trigger::parse(&mut Cursor::new(definition), &mut parse)
}

pub fn parse_lboard(definition: &str, memrefs: &mut Memrefs) -> Result<Lboard, Error> {
    let mut parse = Parse::new(memrefs);
    Lboard::parse(definition, &mut parse)
}

/// What happened in a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Triggered(u32),
    /// All but its Trigger conditions hold: a challenge is on.
    Primed(u32),
    Unprimed(u32),
    /// Its measured value moved: `value` of `target`.
    Progress {
        id: u32,
        value: u32,
        target: u32,
        percent: bool,
    },
    LeaderboardStarted(u32),
    LeaderboardUpdated {
        id: u32,
        value: i32,
    },
    LeaderboardCanceled(u32),
    LeaderboardSubmitted {
        id: u32,
        value: i32,
    },
}

struct Achievement {
    id: u32,
    trigger: Trigger,
    /// Unlocked ones aren't checked.
    active: bool,
}

struct Leaderboard {
    id: u32,
    lboard: Lboard,
    value: i32,
}

/// A game's logic.
#[derive(Default)]
pub struct Runtime {
    memrefs: Memrefs,
    achievements: Vec<Achievement>,
    leaderboards: Vec<Leaderboard>,
    richpresence: Option<RichPresence>,
}

impl Runtime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_achievement(&mut self, id: u32, definition: &str) -> Result<(), Error> {
        let trigger = parse_trigger(definition, &mut self.memrefs)?;
        self.achievements.push(Achievement {
            id,
            trigger,
            active: true,
        });
        Ok(())
    }

    pub fn add_leaderboard(&mut self, id: u32, definition: &str) -> Result<(), Error> {
        let lboard = parse_lboard(definition, &mut self.memrefs)?;
        self.leaderboards.push(Leaderboard {
            id,
            lboard,
            value: 0,
        });
        Ok(())
    }

    pub fn set_richpresence(&mut self, script: &str) -> Result<(), Error> {
        self.richpresence = Some(RichPresence::parse(script, &mut self.memrefs)?);
        Ok(())
    }

    /// Stop checking achievement `id` (it's unlocked).
    pub fn deactivate(&mut self, id: u32) {
        for achievement in self.achievements.iter_mut().filter(|a| a.id == id) {
            achievement.active = false;
        }
    }

    pub fn is_active(&self, id: u32) -> bool {
        self.achievements.iter().any(|a| a.id == id && a.active)
    }

    /// Achievement `id`'s measured progress, if it has any: value and
    /// target.
    pub fn measured(&self, id: u32) -> Option<(u32, u32)> {
        let a = self.achievements.iter().find(|a| a.id == id)?;
        let t = &a.trigger;
        (t.measured_target != 0 && a.active).then(|| {
            (
                if t.measured_value == MEASURED_UNKNOWN {
                    0
                } else {
                    t.measured_value.min(t.measured_target)
                },
                t.measured_target,
            )
        })
    }

    /// Whether achievement `id` is primed (a challenge on).
    pub fn is_primed(&self, id: u32) -> bool {
        self.achievements
            .iter()
            .any(|a| a.id == id && a.active && a.trigger.state == TriggerState::Primed)
    }

    /// Everything back to waiting: after a state is loaded, or the game
    /// starts over.
    pub fn reset(&mut self) {
        for achievement in &mut self.achievements {
            achievement.trigger.reset();
        }
        for leaderboard in &mut self.leaderboards {
            leaderboard.lboard.reset();
        }
        if let Some(rp) = &mut self.richpresence {
            rp.reset();
        }
    }

    /// Read memory and check everything for a frame. Leaderboards are
    /// checked with `leaderboards` (in hardcore mode).
    pub fn do_frame(&mut self, peek: &dyn Peek, leaderboards: bool) -> Vec<Event> {
        let mut events = Vec::new();
        self.memrefs.update(peek);
        for achievement in self.achievements.iter_mut().filter(|a| a.active) {
            let trigger = &mut achievement.trigger;
            let old_measured = trigger.measured_value;
            let old_state = trigger.state;
            let mut new_state = trigger.evaluate(&self.memrefs);
            if new_state == TriggerState::Reset {
                new_state = trigger.state;
            }
            if trigger.measured_value != old_measured
                && old_measured != MEASURED_UNKNOWN
                && trigger.measured_value <= trigger.measured_target
                && trigger.measured_target != 0
                && new_state.is_active()
                && new_state != TriggerState::Waiting
            {
                let percent = trigger.measured_as_percent;
                let moved = !percent
                    || (old_measured as u64 * 100 / trigger.measured_target as u64)
                        != (trigger.measured_value as u64 * 100 / trigger.measured_target as u64);
                if moved {
                    events.push(Event::Progress {
                        id: achievement.id,
                        value: trigger.measured_value,
                        target: trigger.measured_target,
                        percent,
                    });
                }
            }
            if new_state == old_state {
                continue;
            }
            if old_state == TriggerState::Primed {
                events.push(Event::Unprimed(achievement.id));
            }
            match new_state {
                TriggerState::Triggered => {
                    achievement.active = false;
                    events.push(Event::Triggered(achievement.id));
                }
                TriggerState::Primed => events.push(Event::Primed(achievement.id)),
                _ => {}
            }
        }
        if leaderboards {
            for leaderboard in &mut self.leaderboards {
                let old_state = leaderboard.lboard.state;
                let (state, value) = leaderboard.lboard.evaluate(&self.memrefs);
                let id = leaderboard.id;
                match state {
                    LboardState::Started if old_state != LboardState::Started => {
                        leaderboard.value = value;
                        events.push(Event::LeaderboardStarted(id));
                    }
                    LboardState::Started if value != leaderboard.value => {
                        leaderboard.value = value;
                        events.push(Event::LeaderboardUpdated { id, value });
                    }
                    LboardState::Canceled if old_state != LboardState::Canceled => {
                        events.push(Event::LeaderboardCanceled(id))
                    }
                    LboardState::Triggered if old_state != LboardState::Triggered => {
                        leaderboard.value = value;
                        events.push(Event::LeaderboardSubmitted { id, value });
                    }
                    _ => {}
                }
            }
        }
        if let Some(rp) = &mut self.richpresence {
            rp.update(&mut self.memrefs);
        }
        events
    }

    /// The rich presence line now.
    pub fn richpresence(&mut self) -> Option<String> {
        let rp = self.richpresence.as_mut()?;
        Some(rp.display(&self.memrefs))
    }

    /// Leaderboard `id`'s value now.
    pub fn leaderboard_value(&self, id: u32) -> Option<i32> {
        self.leaderboards
            .iter()
            .find(|l| l.id == id)
            .map(|l| l.value)
    }
}

#[cfg(test)]
mod tests;
