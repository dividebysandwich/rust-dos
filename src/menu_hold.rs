//! The right mouse button held still for a few seconds opens the settings
//! window (`mouse_menu_hold`), for a TV or a remote without F12 or any
//! keyboard. The press reaches the program as usual; moving the mouse while
//! holding it, as a text selection or a game does, makes it a press like
//! any other.

use std::time::{Duration, Instant};

/// How long the button is held.
pub const HOLD: Duration = Duration::from_secs(3);
/// How far the mouse may move meanwhile: window pixels, or the captured
/// mouse's motion, summed over both axes.
const TRAVEL: i32 = 8;

#[derive(Clone, Copy, Debug, Default)]
pub struct MenuHold {
    since: Option<Instant>,
    travel: i32,
    fired: bool,
}

impl MenuHold {
    /// The right button went down.
    pub fn press(&mut self, now: Instant) {
        *self = Self { since: Some(now), travel: 0, fired: false };
    }

    /// The mouse moved by `(dx, dy)`.
    pub fn motion(&mut self, dx: i32, dy: i32) {
        if self.since.is_some() && !self.fired {
            self.travel = self.travel.saturating_add(dx.abs() + dy.abs());
            if self.travel > TRAVEL {
                self.since = None;
            }
        }
    }

    /// The right button came up: whether the hold opened the window, and
    /// the release is then nobody's.
    pub fn release(&mut self) -> bool {
        let fired = self.fired;
        *self = Self::default();
        fired
    }

    /// Whether the button has now been held long enough: once per press.
    pub fn poll(&mut self, now: Instant) -> bool {
        match self.since {
            Some(since) if !self.fired && now.duration_since(since) >= HOLD => {
                self.since = None;
                self.fired = true;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_once_after_the_hold() {
        let t = Instant::now();
        let mut h = MenuHold::default();
        h.press(t);
        assert!(!h.poll(t + HOLD - Duration::from_millis(1)));
        assert!(h.poll(t + HOLD));
        assert!(!h.poll(t + HOLD * 2));
        assert!(h.release());
        assert!(!h.poll(t + HOLD * 3));
    }

    #[test]
    fn a_short_press_is_a_click() {
        let t = Instant::now();
        let mut h = MenuHold::default();
        h.press(t);
        assert!(!h.release());
        assert!(!h.poll(t + HOLD));
    }

    #[test]
    fn moving_makes_it_a_press() {
        let t = Instant::now();
        let mut h = MenuHold::default();
        h.press(t);
        h.motion(3, -2);
        assert!(!h.poll(t + HOLD - Duration::from_millis(1)));
        h.motion(2, 2);
        assert!(!h.poll(t + HOLD));
        assert!(!h.release());
        // A jitter of a pixel or two doesn't.
        h.press(t);
        h.motion(1, 1);
        h.motion(-1, 0);
        assert!(h.poll(t + HOLD));
    }

    #[test]
    fn motion_without_a_press_does_nothing() {
        let t = Instant::now();
        let mut h = MenuHold::default();
        h.motion(100, 100);
        h.press(t);
        assert!(h.poll(t + HOLD));
    }
}
