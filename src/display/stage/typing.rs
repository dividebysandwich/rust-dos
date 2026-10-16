//! Typing on the keyboard on the desk (`keycaps`): a controller's tip
//! pushed down into a key presses it until it comes up out of it again,
//! and a key held down repeats as a PC's does. To press another, the tip
//! comes up over the keys first, so that it doesn't press the keys it
//! slides over. Clicked (with the laser, the trigger at a touch, or the
//! mouse), a key goes down for a moment; a shift key clicked (Shift, Ctrl,
//! Alt) stays down until the next key clicked has come up.

use super::keycaps::Key;
use glam::Vec3;
pub use rust_dos::vr::KeyEvent;
use std::time::{Duration, Instant};

/// How long a key is held before it repeats, and how often it does then.
const DELAY: Duration = Duration::from_millis(500);
const RATE: Duration = Duration::from_millis(50);
/// How long a click holds a key down.
const TAP: Duration = Duration::from_millis(90);
/// How far below its top the tip is when a key goes down; how far above
/// it, it has come up; and below how deep it no longer pushes the key,
/// in metres.
const PRESS: f32 = 0.002;
const LIFT: f32 = 0.003;
const DEEP: f32 = 0.06;
/// How far past a key's edges a tip still holds it down.
const SLACK: f32 = 0.004;
/// How far over the keys' tops the tip comes, to press another.
const ARM: f32 = 0.006;

/// A hand's tip: the key it holds down, and whether it is ready to press
/// one (it has been over the keys since).
#[derive(Clone, Copy, Debug, Default)]
struct Tip {
    key: Option<usize>,
    armed: bool,
}

/// A key: the tips holding it down (a bit each), until when a click
/// does, whether it is a shift key kept down by a click, whether the
/// machine has it down, and when it repeats next.
#[derive(Clone, Copy, Debug, Default)]
struct State {
    tips: u8,
    until: Option<Instant>,
    latched: bool,
    down: bool,
    repeat: Option<Instant>,
}

#[derive(Default)]
pub struct Typing {
    /// What each key sends, and whether it is a shift key.
    scans: Vec<Option<(u8, bool)>>,
    shifts: Vec<bool>,
    states: Vec<State>,
    tips: [Tip; 2],
    events: Vec<KeyEvent>,
}

impl Typing {
    pub fn new(keys: &[Key]) -> Self {
        Typing {
            scans: keys.iter().map(|k| k.scan).collect(),
            shifts: keys.iter().map(|k| k.shift).collect(),
            states: vec![State::default(); keys.len()],
            tips: [Tip::default(); 2],
            events: Vec::new(),
        }
    }

    /// The keys of another keyboard, `keys`: those down on this one come
    /// up first.
    pub fn rebuild(&mut self, keys: &[Key], now: Instant) {
        self.release_all(now);
        let events = std::mem::take(&mut self.events);
        *self = Typing {
            events,
            ..Typing::new(keys)
        };
    }

    /// The hands' tips where `tips` has them, in the keyboard's frame:
    /// the keys pressed and let go. Which of them pressed a key now.
    pub fn strike(&mut self, keys: &[Key], tips: [Option<Vec3>; 2], now: Instant) -> [bool; 2] {
        let top = keys.iter().map(|k| k.top).fold(f32::MIN, f32::max);
        let mut pressed = [false; 2];
        for (i, at) in tips.into_iter().enumerate() {
            let tip = &mut self.tips[i];
            let Some(p) = at else {
                if let Some(k) = tip.key.take() {
                    self.states[k].tips &= !(1 << i);
                }
                tip.armed = false;
                continue;
            };
            if let Some(k) = tip.key {
                let key = &keys[k];
                if !(key.over(p, SLACK) && p.y < key.top + LIFT && p.y > key.top - DEEP) {
                    self.states[k].tips &= !(1 << i);
                    tip.key = None;
                }
            }
            if p.y > top + ARM {
                tip.armed = true;
            }
            if tip.key.is_none() && tip.armed {
                let under = keys
                    .iter()
                    .position(|k| k.over(p, 0.0) && p.y < k.top - PRESS && p.y > k.top - DEEP);
                if let Some(k) = under {
                    tip.key = Some(k);
                    tip.armed = false;
                    self.states[k].tips |= 1 << i;
                    pressed[i] = true;
                }
            }
        }
        self.settle(now);
        pressed
    }

    /// Key `k` clicked: down for a moment, or a shift key down (or up
    /// again) until the next key clicked.
    pub fn tap(&mut self, k: usize, now: Instant) {
        if k >= self.states.len() {
            return;
        }
        if self.shifts[k] {
            self.states[k].latched = !self.states[k].latched;
        } else {
            self.states[k].until = Some(now + TAP);
            // The shift keys come up after it.
            for state in self.states.iter_mut().filter(|s| s.latched) {
                state.latched = false;
                state.until = Some(now + TAP + TAP / 2);
            }
        }
        self.settle(now);
    }

    /// Whether hand `i`'s tip holds a key down.
    pub fn on_key(&self, i: usize) -> bool {
        self.tips[i].key.is_some()
    }

    /// Whether key `k` is down.
    pub fn down(&self, k: usize) -> bool {
        self.states.get(k).is_some_and(|s| s.down)
    }

    /// Every key up: the keyboard is gone.
    pub fn release_all(&mut self, now: Instant) {
        for state in &mut self.states {
            *state = State {
                down: state.down,
                ..State::default()
            };
        }
        for tip in &mut self.tips {
            *tip = Tip::default();
        }
        self.settle(now);
    }

    /// What happened to the keys since, for the machine.
    pub fn take_events(&mut self) -> Vec<KeyEvent> {
        std::mem::take(&mut self.events)
    }

    /// The keys down or up as they are held now, those held long enough
    /// repeating, and the clicks over let go.
    pub fn settle(&mut self, now: Instant) {
        for (k, state) in self.states.iter_mut().enumerate() {
            if state.until.is_some_and(|until| now >= until) {
                state.until = None;
            }
            let held = state.tips != 0;
            let down = held || state.until.is_some() || state.latched;
            let send = |events: &mut Vec<KeyEvent>, down: bool| {
                if let Some((scan, extended)) = self.scans[k] {
                    events.push(KeyEvent {
                        scan,
                        extended,
                        down,
                    });
                }
            };
            if down != state.down {
                state.down = down;
                send(&mut self.events, down);
                state.repeat = (held && !self.shifts[k]).then_some(now + DELAY);
            } else if held && state.repeat.is_some_and(|at| now >= at) {
                send(&mut self.events, true);
                state.repeat = Some(now + RATE);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::stage::keycaps;

    fn find(keys: &[Key], legend: &str) -> usize {
        keys.iter().position(|k| k.legend == legend).unwrap()
    }

    fn over(key: &Key, height: f32) -> Option<Vec3> {
        Some(Vec3::new(key.center.x, key.top + height, key.center.y))
    }

    #[test]
    fn a_tip_pushed_into_a_key_presses_it_until_it_comes_up() {
        let keys = keycaps::model().keys;
        let a = find(&keys, "A");
        let mut typing = Typing::new(&keys);
        let now = Instant::now();
        // Coming down from above: over the top, nothing; in, down.
        assert_eq!(
            typing.strike(&keys, [None, over(&keys[a], 0.02)], now),
            [false, false]
        );
        assert_eq!(
            typing.strike(&keys, [None, over(&keys[a], -0.001)], now),
            [false, false]
        );
        assert_eq!(
            typing.strike(&keys, [None, over(&keys[a], -0.004)], now),
            [false, true]
        );
        assert_eq!(
            typing.take_events(),
            [KeyEvent {
                scan: 0x1E,
                extended: false,
                down: true
            }]
        );
        assert!(typing.down(a) && typing.on_key(1));
        // Held, it repeats after a while.
        typing.strike(&keys, [None, over(&keys[a], -0.004)], now + DELAY);
        assert_eq!(typing.take_events().len(), 1);
        // Up out of it, it comes up.
        typing.strike(&keys, [None, over(&keys[a], 0.004)], now + DELAY);
        assert_eq!(
            typing.take_events(),
            [KeyEvent {
                scan: 0x1E,
                extended: false,
                down: false
            }]
        );
        // Slid over to S without coming up over the keys, S stays up;
        // lifted and pushed in again, S goes down.
        let s = find(&keys, "S");
        typing.strike(&keys, [None, over(&keys[a], 0.02)], now);
        typing.strike(&keys, [None, over(&keys[a], -0.004)], now);
        typing.strike(&keys, [None, over(&keys[s], -0.004)], now);
        assert!(!typing.down(s) && !typing.down(a));
        typing.strike(&keys, [None, over(&keys[s], 0.02)], now);
        typing.strike(&keys, [None, over(&keys[s], -0.004)], now);
        assert!(typing.down(s));
        // Its hand gone, it comes up.
        typing.take_events();
        typing.strike(&keys, [None, None], now);
        assert_eq!(
            typing.take_events(),
            [KeyEvent {
                scan: 0x1F,
                extended: false,
                down: false
            }]
        );
    }

    #[test]
    fn a_shift_key_clicked_stays_down_for_the_next_key() {
        let keys = keycaps::model().keys;
        let (shift, q) = (find(&keys, "Shift"), find(&keys, "Q"));
        let mut typing = Typing::new(&keys);
        let now = Instant::now();
        typing.tap(shift, now);
        typing.tap(q, now);
        let down = |scan| KeyEvent {
            scan,
            extended: false,
            down: true,
        };
        let up = |scan| KeyEvent {
            scan,
            extended: false,
            down: false,
        };
        assert_eq!(typing.take_events(), [down(0x2A), down(0x10)]);
        typing.settle(now + TAP);
        assert_eq!(typing.take_events(), [up(0x10)]);
        typing.settle(now + TAP * 2);
        assert_eq!(typing.take_events(), [up(0x2A)]);
        // Clicked twice, it comes up again.
        typing.tap(shift, now);
        typing.tap(shift, now);
        assert_eq!(typing.take_events(), [down(0x2A), up(0x2A)]);
    }

    #[test]
    fn the_keys_send_their_scan_codes() {
        let keys = keycaps::model().keys;
        assert_eq!(keys.len(), 101);
        let scan = |legend: &str| keys[find(&keys, legend)].scan;
        assert_eq!(scan("Esc"), Some((0x01, false)));
        assert_eq!(scan("F12"), Some((0x58, false)));
        assert_eq!(scan("M"), Some((0x32, false)));
        assert_eq!(scan("\u{18}"), Some((0x48, true)));
        assert_eq!(scan("Num\nLock"), Some((0x45, false)));
        // No two keys send the same.
        let mut sent: Vec<(u8, bool)> = keys.iter().filter_map(|k| k.scan).collect();
        sent.sort();
        let all = sent.len();
        sent.dedup();
        assert_eq!(sent.len(), all);
        // None of them overlap.
        for (i, a) in keys.iter().enumerate() {
            for b in &keys[i + 1..] {
                let d = (a.center - b.center).abs() - a.half - b.half;
                assert!(d.x > 0.0 || d.y > 0.0, "{:?} {:?}", a.legend, b.legend);
            }
        }
    }
}
