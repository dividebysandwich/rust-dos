//! What a headset's controllers do: the hand that last pulled its trigger
//! points a laser, which is the mouse where it meets the screen; the sticks
//! and the other buttons are a gamepad; the menu button opens the settings
//! window, or held for a second centres the view. And what they look like:
//! a stick for each hand, the laser and its dot.

use super::pick;
use super::render::Extra;
use super::scene::Screen;
use glam::{Mat4, Vec2, Vec3};
use rust_dos::padmap::PadSnapshot;
use rust_dos::vr::VrControllers;
use std::time::{Duration, Instant};

/// A controller, where it points and what is held on it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Hand {
    /// Its aim in the scene: the unit's -Z is the way it points.
    pub aim: Mat4,
    /// The trigger, the grip, the face buttons (A or X, B or Y) and the
    /// menu button.
    pub select: bool,
    pub squeeze: bool,
    pub primary: bool,
    pub secondary: bool,
    pub menu: bool,
    /// The thumbstick or trackpad, -1 to 1, up positive.
    pub stick: Vec2,
}

/// Where the head and the hands are at a frame's time, in the scene.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tracking {
    pub head: Option<Mat4>,
    /// Left and right.
    pub hands: [Option<Hand>; 2],
}

/// What the controllers do to the machine at a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VrInput {
    /// Where the laser meets the screen (0 to 1 across and down its
    /// texture), if it does.
    pub pointer: Option<Vec2>,
    /// The mouse's left and right buttons.
    pub buttons: [bool; 2],
    /// The controllers as a gamepad, while they are one and tracked.
    pub pad: Option<PadSnapshot>,
    /// The menu button's short presses so far: the settings window opens
    /// or closes at each.
    pub menu_presses: u32,
}

/// How long the menu button is held to centre the view.
const LONG_PRESS: Duration = Duration::from_secs(1);

/// The laser's length where it meets nothing, in metres.
const REACH: f32 = 3.0;

use rust_dos::vr::{PAD_A, PAD_B, PAD_LEFT_SHOULDER, PAD_RIGHT_SHOULDER, PAD_X, PAD_Y};

/// What the controllers did before, to tell presses from holds.
pub struct Controllers {
    /// The pointing hand: 0 left, 1 right.
    active: usize,
    selected: [bool; 2],
    /// Since when the menu button is held, and whether that centred the
    /// view already.
    menu_since: Option<Instant>,
    centred: bool,
    menu_presses: u32,
}

impl Default for Controllers {
    fn default() -> Self {
        Controllers { active: 1, selected: [false; 2], menu_since: None, centred: false, menu_presses: 0 }
    }
}

impl Controllers {
    /// The input of `tracking` with the controllers doing what `mode`
    /// says, the boxes that show them, and whether to centre the view.
    pub fn update(
        &mut self,
        mode: VrControllers,
        tracking: &Tracking,
        screen: &Screen,
        now: Instant,
    ) -> (VrInput, Vec<Extra>, bool) {
        let hands = tracking.hands;
        // The hand pulling its trigger becomes the pointing one.
        for (i, hand) in hands.iter().enumerate() {
            let select = hand.is_some_and(|h| h.select);
            if select && !self.selected[i] {
                self.active = i;
            }
            self.selected[i] = select;
        }
        let mut input = VrInput { menu_presses: self.menu_presses, ..VrInput::default() };
        let mut extras = Vec::new();
        for hand in hands.iter().flatten() {
            // The controller: a stick a little behind where it points from.
            let body = hand.aim * Mat4::from_translation(Vec3::new(0.0, 0.0, 0.06)) * Mat4::from_scale(Vec3::new(0.03, 0.03, 0.12));
            extras.push(Extra { model: body, color: [0.06, 0.06, 0.07, 1.0] });
        }
        let pointing = hands[self.active].filter(|_| mode.pointer());
        if let Some(hand) = pointing {
            let origin = hand.aim.transform_point3(Vec3::ZERO);
            let dir = hand.aim.transform_vector3(Vec3::NEG_Z).normalize_or(Vec3::NEG_Z);
            let hit = pick::pick_hit(screen, origin, dir);
            input.pointer = hit.map(|(_, uv)| uv);
            input.buttons = [hand.select, hand.squeeze];
            let length = hit.map_or(REACH, |(t, _)| t);
            let glow = if hand.select { 1.0 } else { 0.55 };
            let beam = hand.aim
                * Mat4::from_translation(Vec3::new(0.0, 0.0, -length / 2.0))
                * Mat4::from_scale(Vec3::new(0.002, 0.002, length));
            extras.push(Extra { model: beam, color: [0.3 * glow, 0.75 * glow, 1.6 * glow, if hit.is_some() { 0.7 } else { 0.3 }] });
            if let Some((t, _)) = hit {
                let at = origin + dir * (t - 0.003);
                extras.push(Extra {
                    model: Mat4::from_translation(at) * Mat4::from_scale(Vec3::splat(0.012)),
                    color: [0.6, 1.2, 2.0, 1.0],
                });
            }
        }
        if mode.gamepad() && hands.iter().any(Option::is_some) {
            let skip = pointing.map(|_| self.active);
            input.pad = Some(gamepad(&hands, skip));
        }
        // The menu button: a press opens or closes the settings window; a
        // second's hold centres the view instead.
        let menu = hands.iter().flatten().any(|h| h.menu);
        let mut recenter = false;
        match (menu, self.menu_since) {
            (true, None) => {
                self.menu_since = Some(now);
                self.centred = false;
            }
            (true, Some(since)) if !self.centred && now.duration_since(since) >= LONG_PRESS => {
                self.centred = true;
                recenter = true;
            }
            (false, Some(_)) => {
                if !self.centred {
                    self.menu_presses += 1;
                }
                self.menu_since = None;
            }
            _ => {}
        }
        input.menu_presses = self.menu_presses;
        (input, extras, recenter)
    }
}

/// The controllers as a gamepad: the left stick and the right, the right
/// hand's buttons A and B, the left's X and Y, the grips the shoulder
/// buttons and the triggers the triggers; but the pointing hand's (`skip`)
/// trigger and grip, which are the mouse's.
fn gamepad(hands: &[Option<Hand>; 2], skip: Option<usize>) -> PadSnapshot {
    let mut pad = PadSnapshot::default();
    for (i, hand) in hands.iter().enumerate() {
        let Some(hand) = hand else { continue };
        // Up is negative on a gamepad.
        pad.axes[i * 2] = hand.stick.x.clamp(-1.0, 1.0);
        pad.axes[i * 2 + 1] = (-hand.stick.y).clamp(-1.0, 1.0);
        let (primary, secondary, shoulder) = if i == 0 { (PAD_X, PAD_Y, PAD_LEFT_SHOULDER) } else { (PAD_A, PAD_B, PAD_RIGHT_SHOULDER) };
        for (held, bit) in [(hand.primary, primary), (hand.secondary, secondary)] {
            if held {
                pad.buttons |= bit;
            }
        }
        if skip != Some(i) {
            if hand.squeeze {
                pad.buttons |= shoulder;
            }
            pad.triggers[i] = if hand.select { 1.0 } else { 0.0 };
        }
    }
    pad
}

#[cfg(test)]
mod tests {
    use super::super::scene::Scene;
    use super::*;

    /// A hand at the eyes' height, pointing straight ahead (-Z).
    fn ahead(x: f32) -> Hand {
        Hand { aim: Mat4::from_translation(Vec3::new(x, 1.4, 0.0)), ..Hand::default() }
    }

    #[test]
    fn the_hand_that_pulls_its_trigger_points() {
        let room = Scene::test_room();
        let mut c = Controllers::default();
        let now = Instant::now();
        let mut tracking = Tracking { head: None, hands: [Some(ahead(0.0)), Some(ahead(5.0))] };
        // The right hand points first, past the screen: no pointer.
        let (input, extras, _) = c.update(VrControllers::Both, &tracking, &room.screen, now);
        assert_eq!(input.pointer, None);
        assert_eq!(extras.len(), 3, "two controllers and a beam");
        // The left pulls its trigger: it points, at the screen's middle,
        // and clicks; its trigger isn't the gamepad's.
        tracking.hands[0].as_mut().unwrap().select = true;
        let (input, extras, _) = c.update(VrControllers::Both, &tracking, &room.screen, now);
        let uv = input.pointer.unwrap();
        assert!(uv.abs_diff_eq(Vec2::new(0.5, 0.5), 1e-3), "{:?}", uv);
        assert_eq!(input.buttons, [true, false]);
        assert_eq!(extras.len(), 4, "and the dot where it meets the screen");
        assert_eq!(input.pad.unwrap().triggers, [0.0, 0.0]);
        // Laser only: no gamepad. Gamepad only: no laser, the trigger fires.
        let (input, _, _) = c.update(VrControllers::Pointer, &tracking, &room.screen, now);
        assert!(input.pad.is_none() && input.pointer.is_some());
        let (input, extras, _) = c.update(VrControllers::Gamepad, &tracking, &room.screen, now);
        assert!(input.pointer.is_none() && extras.len() == 2);
        assert_eq!(input.pad.unwrap().triggers, [1.0, 0.0]);
    }

    #[test]
    fn the_gamepad_has_both_hands() {
        let mut left = ahead(0.0);
        left.stick = Vec2::new(-1.0, 1.0);
        left.secondary = true;
        let mut right = ahead(0.0);
        right.primary = true;
        right.squeeze = true;
        let pad = gamepad(&[Some(left), Some(right)], None);
        assert_eq!(pad.axes, [-1.0, -1.0, 0.0, 0.0], "pushed up and left");
        assert_eq!(pad.buttons, PAD_Y | PAD_A | PAD_RIGHT_SHOULDER);
        let stick = rust_dos::vr::joystick(&pad);
        use rust_dos::joystick::{PAD_A as A, PAD_B as B, PAD_Y as Y};
        assert_eq!(stick.buttons, A | B | Y);
    }

    #[test]
    fn the_menu_button_opens_the_window_or_centres_the_view() {
        let room = Scene::test_room();
        let mut c = Controllers::default();
        let start = Instant::now();
        let mut hand = ahead(0.0);
        let at = |c: &mut Controllers, hand: Hand, ms: u64| {
            let tracking = Tracking { head: None, hands: [None, Some(hand)] };
            c.update(VrControllers::Both, &tracking, &room.screen, start + Duration::from_millis(ms))
        };
        hand.menu = true;
        assert_eq!(at(&mut c, hand, 0).0.menu_presses, 0);
        hand.menu = false;
        assert_eq!(at(&mut c, hand, 200).0.menu_presses, 1);
        hand.menu = true;
        assert!(!at(&mut c, hand, 300).2);
        assert!(at(&mut c, hand, 1400).2, "held a second");
        assert!(!at(&mut c, hand, 1500).2, "once");
        hand.menu = false;
        assert_eq!(at(&mut c, hand, 1600).0.menu_presses, 1, "a hold isn't a press");
    }
}
