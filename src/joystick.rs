//! The game port at 201h and what is plugged into it (`[joystick]`): the
//! host's game controllers, or the mouse as joystick A.
//!
//! A write to 201h fires four one-shots, one per axis, each of which stays
//! high for a time that grows with its stick's position; games write the
//! port, then count reads until each bit goes low. The one-shots here count
//! reads, not time: a PC at 4.77 MHz reads the port every 2.5 µs or so, so
//! an axis trips after about 10 reads at one end and 450 at the other, and
//! games that don't calibrate (Carrier Command steers its cursor with the
//! joystick) see the stick where it is whatever the emulated CPU's speed.
//! The buttons are the high four bits, low while pressed.

use crate::mouse::{BUTTON_LEFT, BUTTON_RIGHT, MouseState};

/// What the game port has plugged in (`joysticktype`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoystickType {
    /// The controllers there are: one is both joysticks and all four
    /// buttons, two are a joystick each, and without any the mouse is
    /// joystick A.
    Auto,
    /// One controller: its left stick is joystick A, its right stick
    /// joystick B, and its A, B, X and Y buttons buttons 1 to 4.
    FourAxis,
    /// Two controllers, each a joystick with two buttons.
    TwoAxis,
    /// The mouse is joystick A, its buttons A's buttons.
    Mouse,
    /// No game port.
    None,
}

impl JoystickType {
    pub const ALL: [JoystickType; 5] =
        [JoystickType::Auto, JoystickType::FourAxis, JoystickType::TwoAxis, JoystickType::Mouse, JoystickType::None];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name().eq_ignore_ascii_case(s.trim()))
    }

    pub fn name(self) -> &'static str {
        match self {
            JoystickType::Auto => "auto",
            JoystickType::FourAxis => "4axis",
            JoystickType::TwoAxis => "2axis",
            JoystickType::Mouse => "mouse",
            JoystickType::None => "none",
        }
    }

    /// As the settings window shows it.
    pub fn describe(self) -> &'static str {
        match self {
            JoystickType::Auto => "auto (controller / mouse)",
            JoystickType::FourAxis => "one controller, 4 axes",
            JoystickType::TwoAxis => "two controllers",
            JoystickType::Mouse => "the mouse",
            JoystickType::None => "none (no game port)",
        }
    }
}

/// The most of a stick's travel the deadzone takes, in percent.
pub const MAX_DEADZONE: u8 = 90;

/// The `[joystick]` section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JoystickSettings {
    pub kind: JoystickType,
    /// How far from the middle, in percent of its travel, a stick has to
    /// move before it counts (`deadzone`).
    pub deadzone: u8,
}

impl Default for JoystickSettings {
    fn default() -> Self {
        Self { kind: JoystickType::Auto, deadzone: 10 }
    }
}

/// A deadzone as written: a number of percent, with or without the %.
pub fn parse_deadzone(value: &str) -> Result<u8, String> {
    let number = value.trim().trim_end_matches('%').trim_end();
    match number.parse::<u8>() {
        Ok(percent) if percent <= MAX_DEADZONE => Ok(percent),
        _ => Err(format!("invalid deadzone '{}' (0 to {})", value.trim(), MAX_DEADZONE)),
    }
}

/// A controller's buttons in `PadState::buttons`, as a standard gamepad
/// lays them out.
pub const PAD_A: u16 = 0x01;
pub const PAD_B: u16 = 0x02;
pub const PAD_X: u16 = 0x04;
pub const PAD_Y: u16 = 0x08;
pub const PAD_UP: u16 = 0x10;
pub const PAD_DOWN: u16 = 0x20;
pub const PAD_LEFT: u16 = 0x40;
pub const PAD_RIGHT: u16 = 0x80;

/// A host game controller as the frontend last saw it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadState {
    /// Left stick x and y, right stick x and y, from -1 (left, up) to 1.
    pub axes: [f32; 4],
    /// `PAD_*` bits of the buttons held.
    pub buttons: u16,
}

impl PadState {
    /// A stick's position (0 left, 1 right) past the deadzone, `deadzone`
    /// being a fraction of its travel: the rest of the travel is spread
    /// over the whole range again. The left stick at rest follows the
    /// D-pad.
    fn stick(&self, which: usize, deadzone: f32) -> (f32, f32) {
        let (x, y) = apply_deadzone(self.axes[which * 2], self.axes[which * 2 + 1], deadzone);
        if which == 0 && x == 0.0 && y == 0.0 {
            let dir = |minus: u16, plus: u16| {
                (self.buttons & plus != 0) as i32 as f32 - (self.buttons & minus != 0) as i32 as f32
            };
            return (dir(PAD_LEFT, PAD_RIGHT), dir(PAD_UP, PAD_DOWN));
        }
        (x, y)
    }
}

/// (`x`, `y`) with a round deadzone of `deadzone` (a fraction of the
/// travel) in the middle. Each axis still reaches its ends, so a stick in
/// a corner reads as a joystick's square corner.
fn apply_deadzone(x: f32, y: f32, deadzone: f32) -> (f32, f32) {
    let r = (x * x + y * y).sqrt();
    if r <= deadzone || r == 0.0 {
        return (0.0, 0.0);
    }
    let scale = (r - deadzone) / (1.0 - deadzone) / r;
    ((x * scale).clamp(-1.0, 1.0), (y * scale).clamp(-1.0, 1.0))
}

/// Reads until an axis trips, at one end of its travel and past it.
const TRIP_MIN: u32 = 10;
const TRIP_RANGE: u32 = 440;

/// The reads until an axis at `pos` (-1 to 1) trips.
fn trip(pos: f32) -> u32 {
    TRIP_MIN + ((pos.clamp(-1.0, 1.0) + 1.0) / 2.0 * TRIP_RANGE as f32).round() as u32
}

/// What the game port answers from, `JoystickType::Auto` resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Mouse,
    FourAxis,
    TwoAxis,
    None,
}

/// The game port and what is plugged into it.
#[derive(Clone, Debug, Default)]
pub struct GamePort {
    settings: JoystickSettings,
    /// The first two of the host's controllers.
    pads: [Option<PadState>; 2],
    /// Reads since the one-shots last fired.
    read_count: u32,
}

impl GamePort {
    pub fn settings(&self) -> JoystickSettings {
        self.settings
    }

    pub fn set_settings(&mut self, settings: JoystickSettings) {
        self.settings = settings;
    }

    /// The host's controller in `slot` (0 or 1) as it is now, or None
    /// where there is none.
    pub fn set_pad(&mut self, slot: usize, pad: Option<PadState>) {
        if let Some(entry) = self.pads.get_mut(slot) {
            *entry = pad;
        }
    }

    /// Whether there is a game port at all.
    pub fn present(&self) -> bool {
        self.settings.kind != JoystickType::None
    }

    fn layout(&self) -> Layout {
        match self.settings.kind {
            JoystickType::Auto => match self.pads.iter().flatten().count() {
                0 => Layout::Mouse,
                1 => Layout::FourAxis,
                _ => Layout::TwoAxis,
            },
            JoystickType::FourAxis => Layout::FourAxis,
            JoystickType::TwoAxis => Layout::TwoAxis,
            JoystickType::Mouse => Layout::Mouse,
            JoystickType::None => Layout::None,
        }
    }

    /// The reads until each axis trips (A x, A y, B x, B y), None for a
    /// stick that isn't there, and the buttons held: bit n for button n+1
    /// (A's two, then B's).
    fn state(&self, mouse: &MouseState) -> ([Option<u32>; 4], u8) {
        let deadzone = self.settings.deadzone.min(MAX_DEADZONE) as f32 / 100.0;
        // In Auto, the controllers there are, in order.
        let mut pads = self.pads.iter().flatten().copied();
        let (first, second) = match self.settings.kind {
            JoystickType::Auto => (pads.next(), pads.next()),
            _ => (self.pads[0], self.pads[1]),
        };
        let pad_buttons = |pad: Option<PadState>, a: u8, b: u8| {
            let buttons = pad.map_or(0, |p| p.buttons);
            (if buttons & PAD_A != 0 { a } else { 0 }) | (if buttons & PAD_B != 0 { b } else { 0 })
        };
        let stick = |pad: Option<PadState>, which: usize| {
            let (x, y) = pad.map_or((0.0, 0.0), |p| p.stick(which, deadzone));
            (Some(trip(x)), Some(trip(y)))
        };
        match self.layout() {
            // The mouse on a 640x200 screen, as Carrier Command reads it.
            Layout::Mouse => {
                let mx = mouse.x.clamp(0, 639) as u32;
                let my = mouse.y.clamp(0, 199) as u32;
                let trips = [Some(TRIP_MIN + TRIP_RANGE * mx / 639), Some(TRIP_MIN + TRIP_RANGE * my / 199), None, None];
                let mut buttons = 0;
                if mouse.buttons & BUTTON_LEFT != 0 {
                    buttons |= 1;
                }
                if mouse.buttons & BUTTON_RIGHT != 0 {
                    buttons |= 2;
                }
                (trips, buttons)
            }
            Layout::FourAxis => {
                let (ax, ay) = stick(first, 0);
                let (bx, by) = stick(first, 1);
                let held = first.map_or(0, |p| p.buttons);
                let buttons = [PAD_A, PAD_B, PAD_X, PAD_Y]
                    .iter()
                    .enumerate()
                    .fold(0, |bits, (i, &b)| if held & b != 0 { bits | 1 << i } else { bits });
                ([ax, ay, bx, by], buttons)
            }
            Layout::TwoAxis => {
                let (ax, ay) = stick(first, 0);
                let (bx, by) = stick(second, 0);
                ([ax, ay, bx, by], pad_buttons(first, 1, 2) | pad_buttons(second, 4, 8))
            }
            Layout::None => ([None; 4], 0),
        }
    }

    /// A write to 201h: the one-shots fire.
    pub fn arm(&mut self) {
        self.read_count = 0;
    }

    /// A read of 201h: an axis bit is high until its one-shot trips, and
    /// one that isn't there has tripped; a button bit is low while pressed.
    pub fn read(&mut self, mouse: &MouseState) -> u8 {
        if !self.present() {
            return 0xFF;
        }
        let count = self.read_count;
        self.read_count = count.saturating_add(1);
        let (trips, buttons) = self.state(mouse);
        let mut value = 0xF0 & !(buttons << 4);
        for (bit, trip) in trips.iter().enumerate() {
            if trip.is_some_and(|t| count < t) {
                value |= 1 << bit;
            }
        }
        value
    }

    /// INT 15h AH=84h DX=0: the buttons as the port has them, in bits 4-7.
    pub fn bios_switches(&self, mouse: &MouseState) -> u8 {
        let (_, buttons) = self.state(mouse);
        0xF0 & !(buttons << 4)
    }

    /// INT 15h AH=84h DX=1: the axes (A x, A y, B x, B y) as counts, 0
    /// for a stick that isn't there.
    pub fn bios_axes(&self, mouse: &MouseState) -> [u16; 4] {
        let (trips, _) = self.state(mouse);
        trips.map(|t| t.unwrap_or(0) as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_parse_back() {
        for kind in JoystickType::ALL {
            assert_eq!(JoystickType::parse(kind.name()), Some(kind));
        }
        assert_eq!(JoystickType::parse(" 4AXIS "), Some(JoystickType::FourAxis));
        assert_eq!(JoystickType::parse("fcs"), None);
    }

    #[test]
    fn deadzones() {
        assert_eq!(parse_deadzone("15"), Ok(15));
        assert_eq!(parse_deadzone(" 0 % "), Ok(0));
        assert!(parse_deadzone("91").is_err());
        assert!(parse_deadzone("-1").is_err());
    }

    #[test]
    fn the_deadzone_is_round_and_rescales() {
        assert_eq!(apply_deadzone(0.05, 0.05, 0.1), (0.0, 0.0));
        let (x, y) = apply_deadzone(1.0, 0.0, 0.1);
        assert!((x - 1.0).abs() < 1e-6 && y == 0.0);
        // Halfway out of the deadzone is halfway along.
        let (x, _) = apply_deadzone(0.55, 0.0, 0.1);
        assert!((x - 0.5).abs() < 1e-6, "{}", x);
        // Diagonals keep their direction, and reach the corners.
        let (x, y) = apply_deadzone(0.5, -0.5, 0.2);
        assert!((x + y).abs() < 1e-6 && x > 0.0);
        assert_eq!(apply_deadzone(1.0, 1.0, 0.1), (1.0, 1.0));
    }

    #[test]
    fn trips_span_the_range() {
        assert_eq!((trip(-1.0), trip(0.0), trip(1.0)), (10, 230, 450));
        assert_eq!(trip(7.0), 450);
    }
}
