//! Capturing the host's mouse for the machine: Ctrl+Alt captures it and
//! lets it go, as in VMware.

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
}
