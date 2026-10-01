//! Capturing the host's mouse for the machine: Ctrl+Alt captures it and
//! lets it go, as in VMware, and with `mouse_autocapture` the mouse is
//! captured as it moves over the window while a program uses the mouse
//! driver, and let go as the program's cursor leaves the screen. Where the
//! cursor is decides that, not where the host's pointer would be: a
//! program's cursor can move slower or faster than the host's, as
//! F-117A's menu cursor does.

/// A key as Ctrl+Alt sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChordKey {
    LeftCtrl,
    RightCtrl,
    LeftAlt,
    RightAlt,
    Other,
}

impl ChordKey {
    fn bit(self) -> u8 {
        match self {
            ChordKey::LeftCtrl => LEFT_CTRL,
            ChordKey::RightCtrl => RIGHT_CTRL,
            ChordKey::LeftAlt => LEFT_ALT,
            ChordKey::RightAlt => RIGHT_ALT,
            ChordKey::Other => 0,
        }
    }
}

const LEFT_CTRL: u8 = 1;
const RIGHT_CTRL: u8 = 2;
const LEFT_ALT: u8 = 4;
const RIGHT_ALT: u8 = 8;
const CTRL: u8 = LEFT_CTRL | RIGHT_CTRL;
const ALT: u8 = LEFT_ALT | RIGHT_ALT;

/// Ctrl+Alt pressed on their own: it counts when the first of them comes
/// up, so that Ctrl+Alt+Del and the other combinations with them stay the
/// machine's. Left Ctrl with right Alt doesn't count: it is what AltGr
/// sends on Windows.
#[derive(Clone, Copy, Debug, Default)]
pub struct CtrlAlt {
    held: u8,
    armed: bool,
    spoiled: bool,
}

impl CtrlAlt {
    pub fn key_down(&mut self, key: ChordKey) {
        if key == ChordKey::Other {
            self.spoiled |= self.held != 0;
            return;
        }
        self.held |= key.bit();
        let altgr = self.held == LEFT_CTRL | RIGHT_ALT;
        if self.held & CTRL != 0 && self.held & ALT != 0 && !altgr && !self.spoiled {
            self.armed = true;
        }
    }

    /// A key comes up: whether that completes Ctrl+Alt.
    pub fn key_up(&mut self, key: ChordKey) -> bool {
        if key == ChordKey::Other {
            return false;
        }
        self.held &= !key.bit();
        let done = self.armed && !self.spoiled;
        // Once until both are up.
        self.spoiled |= done;
        if self.held == 0 {
            *self = Self::default();
        }
        done
    }

    /// The keyboard went elsewhere: what was held is forgotten.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// How far past the edge of the screen, in the driver's virtual pixels,
/// the mouse has to push the cursor to leave it.
const LEAVE_PUSH: f64 = 4.0;
/// How near the edge of the screen the cursor counts as at it, in virtual
/// pixels: programs keep their cursor short of the edge themselves, as
/// F-117A's stops at 630 of 640 by putting it back (AX=0004h).
const EDGE: i32 = 16;

/// The edges of the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// The mouse captured and let go by itself.
#[derive(Clone, Copy, Debug)]
pub struct AutoCapture {
    /// The mouse is captured the next time it moves over the window: it
    /// hasn't been let go there since it came in.
    pub armed: bool,
    /// The mouse was captured by moving over the window, and goes when
    /// the program stops using it.
    pub captured: bool,
    /// Where the mouse would have taken the cursor, past each edge it
    /// pushes against (left, right, top, bottom): from where the cursor
    /// was at the edge, on with all of the motion since.
    reach: [Option<f64>; 4],
}

impl Default for AutoCapture {
    fn default() -> Self {
        Self { armed: true, captured: false, reach: [None; 4] }
    }
}

impl AutoCapture {
    /// The mouse was let go over the window: it is captured again once it
    /// has left the window and comes back.
    pub fn released(&mut self) {
        *self = Self { armed: false, ..Self::default() };
    }

    /// A captured mouse moves by (`dx`, `dy`) virtual pixels with the
    /// program's cursor at (`x`, `y`) on a virtual screen `screen` big:
    /// the edge it pushes the cursor out through, if it does. Whether the
    /// driver's range or the program stops the cursor at the edge, the
    /// mouse goes on: it leaves once it would have taken the cursor past
    /// the screen. A cursor kept away from the edges stays.
    pub fn motion(&mut self, (x, y): (i32, i32), screen: (i32, i32), (dx, dy): (f64, f64)) -> Option<Side> {
        let axes = [(dx, x, screen.0, Side::Left, Side::Right), (dy, y, screen.1, Side::Top, Side::Bottom)];
        let mut out = None;
        for (d, at, size, low, high) in axes {
            if d == 0.0 {
                continue;
            }
            let (toward, away, at_edge) = if d < 0.0 { (low, high, at < EDGE) } else { (high, low, at >= size - EDGE) };
            self.reach[away as usize] = None;
            let reach = &mut self.reach[toward as usize];
            *reach = at_edge.then(|| reach.unwrap_or(at as f64) + d);
            let past = match toward {
                Side::Left | Side::Top => reach.is_some_and(|r| r <= -LEAVE_PUSH),
                Side::Right | Side::Bottom => reach.is_some_and(|r| r >= (size - 1) as f64 + LEAVE_PUSH),
            };
            if past {
                out = out.or(Some(toward));
            }
        }
        if out.is_some() {
            self.reach = [None; 4];
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::ChordKey::*;
    use super::*;

    fn chord(keys: &[(ChordKey, bool)]) -> Vec<bool> {
        let mut c = CtrlAlt::default();
        keys.iter()
            .filter_map(|&(key, down)| if down { c.key_down(key); None } else { Some(c.key_up(key)) })
            .collect()
    }

    #[test]
    fn ctrl_alt_on_their_own() {
        assert_eq!(chord(&[(LeftCtrl, true), (LeftAlt, true), (LeftAlt, false), (LeftCtrl, false)]), [true, false]);
        assert_eq!(chord(&[(LeftAlt, true), (RightCtrl, true), (LeftAlt, false), (RightCtrl, false)]), [true, false]);
        assert_eq!(chord(&[(RightCtrl, true), (RightAlt, true), (RightCtrl, false), (RightAlt, false)]), [true, false]);
    }

    #[test]
    fn other_keys_keep_it_the_machines() {
        // Ctrl+Alt+Del.
        assert_eq!(
            chord(&[(LeftCtrl, true), (LeftAlt, true), (Other, true), (Other, false), (LeftAlt, false), (LeftCtrl, false)]),
            [false, false, false]
        );
        // Ctrl alone, Alt alone, and a key held first.
        assert_eq!(chord(&[(LeftCtrl, true), (LeftCtrl, false), (LeftAlt, true), (LeftAlt, false)]), [false, false]);
        assert_eq!(chord(&[(Other, true), (LeftCtrl, true), (LeftAlt, true), (LeftAlt, false)]), [true]);
        // AltGr on Windows.
        assert_eq!(chord(&[(LeftCtrl, true), (RightAlt, true), (RightAlt, false), (LeftCtrl, false)]), [false, false]);
    }

    #[test]
    fn again_after_both_are_up() {
        let mut c = CtrlAlt::default();
        for _ in 0..2 {
            c.key_down(LeftCtrl);
            c.key_down(LeftAlt);
            assert!(c.key_up(LeftCtrl));
            // Ctrl again while Alt is held: not twice.
            c.key_down(LeftCtrl);
            assert!(!c.key_up(LeftCtrl));
            assert!(!c.key_up(LeftAlt));
        }
    }

    #[test]
    fn the_cursor_leaves_past_the_screens_edge() {
        let mut auto = AutoCapture::default();
        let screen = (640, 200);
        // Moving inside, and up to the edge: still in.
        assert_eq!(auto.motion((320, 100), screen, (100.0, 0.0)), None);
        assert_eq!(auto.motion((630, 100), screen, (8.0, 0.0)), None);
        // The driver's range stops it at 638: pushed on past the edge.
        assert_eq!(auto.motion((638, 100), screen, (2.0, 0.0)), None);
        assert_eq!(auto.motion((638, 100), screen, (3.0, 1.0)), Some(Side::Right));
        // Back in between starts over.
        assert_eq!(auto.motion((638, 100), screen, (3.0, 0.0)), None);
        assert_eq!(auto.motion((636, 100), screen, (-2.0, 0.0)), None);
        assert_eq!(auto.motion((634, 100), screen, (3.0, 0.0)), None);
        // At once, through the others.
        assert_eq!(auto.motion((100, 2), screen, (0.0, -10.0)), Some(Side::Top));
        assert_eq!(auto.motion((0, 100), screen, (-5.0, 0.0)), Some(Side::Left));
        assert_eq!(auto.motion((100, 199), screen, (0.0, 5.0)), Some(Side::Bottom));
    }

    #[test]
    fn a_cursor_the_program_holds_short_of_the_edge_leaves_too() {
        // F-117A's menu puts its cursor back at 630 of 640.
        let mut auto = AutoCapture::default();
        let screen = (640, 200);
        let steps = (0..10).take_while(|_| auto.motion((630, 100), screen, (2.0, 0.0)).is_none()).count();
        // From 630 to 643: 13 pixels, in 2s.
        assert_eq!(steps, 6);
    }

    #[test]
    fn a_cursor_away_from_the_edges_stays() {
        let mut auto = AutoCapture::default();
        // Held at 400 by its range, or by the program.
        for _ in 0..100 {
            assert_eq!(auto.motion((400, 100), (640, 200), (50.0, 0.0)), None);
        }
    }
}
