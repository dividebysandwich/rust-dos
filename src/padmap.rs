//! A game's gamepad mapping (its profile's `[gamepad]` section, as a
//! .dosz package's DOS.YML has it): each of a modern gamepad's 24 inputs
//! pressing up to four of the PC's keys, mouse moves and buttons or
//! joystick moves and buttons (`x=leftctrl+f10 Open Menu`), and an action
//! wheel of more (`wheel_1=1 Fists`) on the input bound to `wheel`. The
//! front ends hand `PadMapper` the pad's inputs each frame.

use crate::bus::Bus;
use crate::joystick::{PAD_A, PAD_B, PAD_X, PAD_Y, PadState};
use crate::keyboard::{self, PcKey};
use std::collections::HashMap;

/// The gamepad's inputs, as the mapping names them, by their bit.
pub const INPUTS: [&str; 24] = [
    "up",
    "down",
    "left",
    "right",
    "b",
    "a",
    "x",
    "y",
    "l",
    "r",
    "l2",
    "r2",
    "l3",
    "r3",
    "select",
    "start",
    "lstick_left",
    "lstick_right",
    "lstick_up",
    "lstick_down",
    "rstick_left",
    "rstick_right",
    "rstick_up",
    "rstick_down",
];

/// The bit of the input called `name` in `PadMapper::update`'s `pressed`.
pub fn input_bit(name: &str) -> Option<u32> {
    INPUTS.iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| 1 << i)
}

/// A host gamepad as a front end reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadSnapshot {
    /// The bits of the first 16 `INPUTS` (the D-pad to Start) held.
    pub buttons: u32,
    /// Left stick x and y, right stick x and y, from -1 (left, up) to 1.
    pub axes: [f32; 4],
    /// The left and right triggers, from 0 to 1, for pads whose triggers
    /// are axes.
    pub triggers: [f32; 2],
}

impl PadSnapshot {
    /// The bits of all `INPUTS` held: the triggers and the sticks pushed
    /// past half way count.
    pub fn inputs(&self) -> u32 {
        let mut bits = self.buttons;
        let mut set = |name: &str, on: bool| {
            if on {
                bits |= input_bit(name).unwrap_or(0);
            }
        };
        set("l2", self.triggers[0] > 0.5);
        set("r2", self.triggers[1] > 0.5);
        let [lx, ly, rx, ry] = self.axes;
        set("lstick_left", lx < -0.5);
        set("lstick_right", lx > 0.5);
        set("lstick_up", ly < -0.5);
        set("lstick_down", ly > 0.5);
        set("rstick_left", rx < -0.5);
        set("rstick_right", rx > 0.5);
        set("rstick_up", ry < -0.5);
        set("rstick_down", ry > 0.5);
        bits
    }

    /// Where the wheel's pointer is: the left stick, or the D-pad.
    pub fn pointer(&self) -> (f32, f32) {
        if self.axes[0].abs() > 0.5 || self.axes[1].abs() > 0.5 {
            return (self.axes[0], self.axes[1]);
        }
        let held = |name: &str| (self.buttons & input_bit(name).unwrap_or(0) != 0) as i32 as f32;
        (held("right") - held("left"), held("down") - held("up"))
    }
}

/// The mouse's part in a mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Up,
    Down,
    Left,
    Right,
    /// A button: 0 left, 1 right, 2 middle.
    Button(usize),
    Faster,
    Slower,
}

/// The joystick's: an axis (0, 1 the first stick's; 2, 3 the second's)
/// pushed one way, or a button (0 to 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoyAction {
    Axis(usize, i8),
    Button(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Key(PcKey),
    Mouse(MouseAction),
    Joy(JoyAction),
    /// Open the action wheel.
    Wheel,
}

fn action(name: &str) -> Option<Action> {
    let lower = name.to_ascii_lowercase();
    let mouse = |a| Some(Action::Mouse(a));
    let joy = |a| Some(Action::Joy(a));
    match lower.as_str() {
        "wheel" => Some(Action::Wheel),
        "mouse_move_up" => mouse(MouseAction::Up),
        "mouse_move_down" => mouse(MouseAction::Down),
        "mouse_move_left" => mouse(MouseAction::Left),
        "mouse_move_right" => mouse(MouseAction::Right),
        "mouse_left_click" => mouse(MouseAction::Button(0)),
        "mouse_right_click" => mouse(MouseAction::Button(1)),
        "mouse_middle_click" => mouse(MouseAction::Button(2)),
        "mouse_speed_up" => mouse(MouseAction::Faster),
        "mouse_speed_down" => mouse(MouseAction::Slower),
        "joy_up" => joy(JoyAction::Axis(1, -1)),
        "joy_down" => joy(JoyAction::Axis(1, 1)),
        "joy_left" => joy(JoyAction::Axis(0, -1)),
        "joy_right" => joy(JoyAction::Axis(0, 1)),
        "joy_button1" => joy(JoyAction::Button(0)),
        "joy_button2" => joy(JoyAction::Button(1)),
        "joy_button3" => joy(JoyAction::Button(2)),
        "joy_button4" => joy(JoyAction::Button(3)),
        // The hat and the second stick are the joystick port's other two
        // axes.
        "joy_hat_up" | "joy_2_up" => joy(JoyAction::Axis(3, -1)),
        "joy_hat_down" | "joy_2_down" => joy(JoyAction::Axis(3, 1)),
        "joy_hat_left" | "joy_2_left" => joy(JoyAction::Axis(2, -1)),
        "joy_hat_right" | "joy_2_right" => joy(JoyAction::Axis(2, 1)),
        _ => keyboard::lookup(&lower).map(Action::Key),
    }
}

/// What one input does, and its name for the player.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Binding {
    pub actions: Vec<Action>,
    pub label: Option<String>,
}

impl Binding {
    /// `a+b+c Label`: up to four actions, then the name.
    fn parse(value: &str) -> Result<Self, String> {
        let value = value.trim();
        let (actions, label) = match value.split_once(char::is_whitespace) {
            Some((actions, label)) => (actions, Some(label.trim().to_string()).filter(|l| !l.is_empty())),
            None => (value, None),
        };
        if actions.eq_ignore_ascii_case("none") {
            return Ok(Self { actions: Vec::new(), label });
        }
        let actions = actions
            .split('+')
            .take(4)
            .map(|a| action(a).ok_or_else(|| format!("'{}' isn't a key, mouse or joystick action", a)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { actions, label })
    }
}

/// A game's mapping.
#[derive(Clone, Debug, PartialEq)]
pub struct PadMapping {
    /// By `INPUTS`.
    pub inputs: Vec<Option<Binding>>,
    /// The action wheel's, in order.
    pub wheel: Vec<Binding>,
    /// The host mouse's wheel turned up and down.
    pub mouse_wheel: [Option<Binding>; 2],
    /// Mouse movement by the pad, and by any means, in percent, and the
    /// part of it across.
    pub pad_mouse_speed: f32,
    pub mouse_speed: f32,
    pub mouse_x_factor: f32,
}

impl PadMapping {
    /// The mapping of a profile's `[gamepad]` lines, and what in them
    /// couldn't be read. None without any input mapped.
    pub fn parse(lines: &[(String, String)]) -> (Option<Self>, Vec<String>) {
        let mut mapping = Self {
            inputs: vec![None; INPUTS.len()],
            wheel: Vec::new(),
            mouse_wheel: [None, None],
            pad_mouse_speed: 100.0,
            mouse_speed: 100.0,
            mouse_x_factor: 100.0,
        };
        let mut wheel: Vec<(u32, Binding)> = Vec::new();
        let mut warnings = Vec::new();
        for (key, value) in lines {
            let key = key.to_ascii_lowercase();
            let percent = || value.trim().trim_end_matches('%').parse::<f32>().ok().filter(|v| *v > 0.0);
            let result = match key.as_str() {
                "padmousespeed" => percent().map(|v| mapping.pad_mouse_speed = v).ok_or("not a percentage".to_string()),
                "mousespeed" => percent().map(|v| mapping.mouse_speed = v).ok_or("not a percentage".to_string()),
                "mousexfactor" => percent().map(|v| mapping.mouse_x_factor = v).ok_or("not a percentage".to_string()),
                "mousewheelup" | "mousewheeldown" => Binding::parse(value).map(|b| {
                    mapping.mouse_wheel[(key == "mousewheeldown") as usize] = Some(b);
                }),
                k if k.starts_with("wheel_") => match k[6..].parse::<u32>() {
                    Ok(n) => Binding::parse(value).map(|b| wheel.push((n, b))),
                    Err(_) => Err("not a wheel item".to_string()),
                },
                k => match INPUTS.iter().position(|n| *n == k) {
                    Some(i) => Binding::parse(value).map(|b| mapping.inputs[i] = Some(b)),
                    None => Err("not a gamepad input".to_string()),
                },
            };
            if let Err(e) = result {
                warnings.push(format!("[gamepad] {}={}: {}", key, value, e));
            }
        }
        wheel.sort_by_key(|(n, _)| *n);
        mapping.wheel = wheel.into_iter().map(|(_, b)| b).filter(|b| !b.actions.is_empty()).collect();
        let any = mapping.inputs.iter().flatten().any(|b| !b.actions.is_empty());
        (any.then_some(mapping), warnings)
    }

    /// Whether it moves the joystick: the joystick port is then its.
    pub fn uses_joystick(&self) -> bool {
        self.bindings().any(|b| b.actions.iter().any(|a| matches!(a, Action::Joy(_))))
    }

    fn bindings(&self) -> impl Iterator<Item = &Binding> {
        self.inputs.iter().flatten().chain(&self.wheel)
    }

    /// The inputs' names for the player: (input, what it does).
    pub fn labels(&self) -> Vec<(String, String)> {
        let mut labels: Vec<(String, String)> = INPUTS
            .iter()
            .zip(&self.inputs)
            .filter_map(|(name, b)| {
                let b = b.as_ref()?;
                let text = match (&b.label, b.actions.first()) {
                    (Some(label), _) => label.clone(),
                    (None, Some(Action::Wheel)) => "Action wheel".to_string(),
                    (None, _) => String::new(),
                };
                Some((name.replace('_', " ").to_uppercase(), text))
            })
            .collect();
        for (i, b) in self.wheel.iter().enumerate() {
            labels.push((format!("WHEEL {}", i + 1), b.label.clone().unwrap_or_default()));
        }
        labels
    }
}

/// How far the mouse moves a frame by the pad, in the machine's mouse
/// pixels, at 100%.
const PAD_MOUSE_STEP: f64 = 3.0;
/// Frames a wheel item's actions are held.
const WHEEL_PRESS_FRAMES: u32 = 4;

/// The action wheel as shown: its items' names, and the one the stick
/// points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WheelView {
    pub items: Vec<String>,
    pub selected: Option<usize>,
}

/// A mapping at work: what its inputs hold down.
pub struct PadMapper {
    mapping: PadMapping,
    /// The inputs held last frame.
    held: u32,
    /// Keys and mouse buttons down, with how many inputs hold each.
    keys: HashMap<(u8, bool), (PcKey, u32)>,
    buttons: [u32; 3],
    /// The wheel is open: the item pointed at.
    wheel: Option<Option<usize>>,
    /// A wheel item pressed, and the frames left until it is let go.
    pressing: Option<(Binding, u32)>,
}

impl PadMapper {
    pub fn new(mapping: PadMapping) -> Self {
        Self { mapping, held: 0, keys: HashMap::new(), buttons: [0; 3], wheel: None, pressing: None }
    }

    pub fn mapping(&self) -> &PadMapping {
        &self.mapping
    }

    fn press(&mut self, bus: &mut Bus, binding: &Binding, down: bool) {
        for action in &binding.actions {
            match *action {
                Action::Key(key) => {
                    let entry = self.keys.entry((key.scan, key.extended)).or_insert((key, 0));
                    let was = entry.1;
                    entry.1 = if down { was + 1 } else { was.saturating_sub(1) };
                    if (was == 0) != (entry.1 == 0) {
                        keyboard::key_event(bus, key.scan, key.extended, down, None);
                    }
                }
                Action::Mouse(MouseAction::Button(b)) => {
                    let was = self.buttons[b];
                    self.buttons[b] = if down { was + 1 } else { was.saturating_sub(1) };
                    if was == 0 && down {
                        bus.mouse.button_down(b);
                    } else if was == 1 && !down {
                        bus.mouse.button_up(b);
                    }
                }
                _ => {}
            }
        }
    }

    /// The pad as it is now: `pressed` the bits of `INPUTS` held, `stick`
    /// the left stick (for the wheel). Returns the wheel while it is open.
    pub fn update(&mut self, bus: &mut Bus, pressed: u32, stick: (f32, f32)) -> Option<WheelView> {
        // A wheel item pressed a moment ago goes up.
        if let Some((binding, frames)) = &mut self.pressing {
            *frames -= 1;
            if *frames == 0 {
                let binding = binding.clone();
                self.press(bus, &binding, false);
                self.pressing = None;
            }
        }
        let changed = pressed ^ self.held;
        let wheel_bit = self.wheel_bits();
        let mut held = self.held;
        for i in 0..INPUTS.len() {
            let bit = 1 << i;
            if changed & bit == 0 {
                continue;
            }
            let down = pressed & bit != 0;
            // The wheel's input opens it, and letting go picks.
            if wheel_bit & bit != 0 {
                if down {
                    self.wheel = Some(None);
                    held |= bit;
                } else {
                    held &= !bit;
                    if let Some(binding) = self.wheel.take().flatten().and_then(|i| self.mapping.wheel.get(i)).cloned() {
                        self.press(bus, &binding, true);
                        self.pressing = Some((binding, WHEEL_PRESS_FRAMES));
                    }
                }
                continue;
            }
            // While the wheel is open, its stick points instead.
            if down && self.wheel.is_some() {
                continue;
            }
            if let Some(binding) = self.mapping.inputs[i].clone() {
                // Let go only what was pressed.
                if down || held & bit != 0 {
                    self.press(bus, &binding, down);
                }
            }
            if down {
                held |= bit;
            } else {
                held &= !bit;
            }
        }
        self.held = held;

        // The mouse, moved while its inputs are held.
        let mut faster = 1.0;
        let (mut dx, mut dy) = (0.0, 0.0);
        for (i, binding) in self.mapping.inputs.iter().enumerate() {
            let Some(binding) = binding else { continue };
            if self.held & (1 << i) == 0 {
                continue;
            }
            for action in &binding.actions {
                match action {
                    Action::Mouse(MouseAction::Up) => dy -= 1.0,
                    Action::Mouse(MouseAction::Down) => dy += 1.0,
                    Action::Mouse(MouseAction::Left) => dx -= 1.0,
                    Action::Mouse(MouseAction::Right) => dx += 1.0,
                    Action::Mouse(MouseAction::Faster) => faster *= 2.0,
                    Action::Mouse(MouseAction::Slower) => faster *= 0.5,
                    _ => {}
                }
            }
        }
        if dx != 0.0 || dy != 0.0 {
            let speed = PAD_MOUSE_STEP * faster * (self.mapping.pad_mouse_speed * self.mapping.mouse_speed) as f64 / 10_000.0;
            bus.mouse.move_by(dx * speed * self.mapping.mouse_x_factor as f64 / 100.0, dy * speed);
        }

        self.wheel.as_mut().map(|selected| {
            let (x, y) = stick;
            let count = self.mapping.wheel.len();
            if (x * x + y * y).sqrt() > 0.5 && count > 0 {
                // From the top, clockwise.
                let angle = x.atan2(-y).rem_euclid(std::f32::consts::TAU);
                let step = std::f32::consts::TAU / count as f32;
                *selected = Some(((angle + step / 2.0) / step) as usize % count);
            }
            WheelView {
                items: self.mapping.wheel.iter().map(|b| b.label.clone().unwrap_or_default()).collect(),
                selected: *selected,
            }
        })
    }

    /// The bits of the inputs bound to the wheel.
    fn wheel_bits(&self) -> u32 {
        if self.mapping.wheel.is_empty() {
            return 0;
        }
        self.mapping
            .inputs
            .iter()
            .enumerate()
            .filter(|(_, b)| b.as_ref().is_some_and(|b| b.actions.contains(&Action::Wheel)))
            .fold(0, |bits, (i, _)| bits | 1 << i)
    }

    /// The joystick as the mapping moves it, if it does.
    pub fn joystick(&self) -> Option<PadState> {
        if !self.mapping.uses_joystick() {
            return None;
        }
        let mut state = PadState::default();
        let bindings = self.mapping.inputs.iter().enumerate().filter(|(i, _)| self.held & (1 << i) != 0);
        let pressing = self.pressing.iter().map(|(b, _)| b);
        for binding in bindings.filter_map(|(_, b)| b.as_ref()).chain(pressing) {
            for action in &binding.actions {
                match *action {
                    Action::Joy(JoyAction::Axis(axis, dir)) => state.axes[axis] = (state.axes[axis] + dir as f32).clamp(-1.0, 1.0),
                    Action::Joy(JoyAction::Button(b)) => state.buttons |= [PAD_A, PAD_B, PAD_X, PAD_Y][b],
                    _ => {}
                }
            }
        }
        Some(state)
    }

    /// The host mouse's wheel turned (`up`): what it is bound to, pressed
    /// for a moment.
    pub fn mouse_wheel(&mut self, bus: &mut Bus, up: bool) -> bool {
        let Some(binding) = self.mapping.mouse_wheel[(!up) as usize].clone() else { return false };
        if self.pressing.is_none() {
            self.press(bus, &binding, true);
            self.pressing = Some((binding, WHEEL_PRESS_FRAMES));
        }
        true
    }

    /// Let go of everything it holds.
    pub fn release(&mut self, bus: &mut Bus) {
        for ((scan, extended), (_, count)) in self.keys.drain() {
            if count > 0 {
                keyboard::key_event(bus, scan, extended, false, None);
            }
        }
        for (b, count) in self.buttons.iter_mut().enumerate() {
            if *count > 0 {
                bus.mouse.button_up(b);
            }
            *count = 0;
        }
        self.held = 0;
        self.wheel = None;
        self.pressing = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn mappings_are_read() {
        let (mapping, warnings) = PadMapping::parse(&lines(&[
            ("up", "up Move Up"),
            ("x", "space Jump"),
            ("start", "leftctrl+f10 Open Menu"),
            ("l", "wheel"),
            ("b", "joy_button1"),
            ("wheel_2", "2 Pistol"),
            ("wheel_1", "1 Fists"),
            ("padmousespeed", "150"),
            ("r", "nosuchkey"),
        ]));
        let mapping = mapping.unwrap();
        assert_eq!(warnings.len(), 1, "{:?}", warnings);
        let start = mapping.inputs[15].as_ref().unwrap();
        assert_eq!(start.actions.len(), 2);
        assert_eq!(start.label.as_deref(), Some("Open Menu"));
        let wheel: Vec<_> = mapping.wheel.iter().map(|b| b.label.clone().unwrap()).collect();
        assert_eq!(wheel, ["Fists", "Pistol"]);
        assert_eq!(mapping.pad_mouse_speed, 150.0);
        assert!(mapping.uses_joystick());
        assert!(mapping.labels().contains(&("START".to_string(), "Open Menu".to_string())));
    }

    #[test]
    fn inputs_press_and_let_go() {
        let mut bus = Bus::new(std::path::PathBuf::from("."));
        let (mapping, _) = PadMapping::parse(&lines(&[("a", "space"), ("b", "space"), ("x", "joy_button2+joy_left")]));
        let mut mapper = PadMapper::new(mapping.unwrap());
        let a = input_bit("a").unwrap();
        let b = input_bit("b").unwrap();
        let x = input_bit("x").unwrap();
        mapper.update(&mut bus, a, (0.0, 0.0));
        mapper.update(&mut bus, a | b, (0.0, 0.0));
        mapper.update(&mut bus, b, (0.0, 0.0));
        assert_eq!(mapper.keys[&(0x39, false)].1, 1, "space is held by B still");
        mapper.update(&mut bus, x, (0.0, 0.0));
        assert_eq!(mapper.keys[&(0x39, false)].1, 0);
        let joy = mapper.joystick().unwrap();
        assert_eq!((joy.buttons, joy.axes[0]), (PAD_B, -1.0));
    }

    #[test]
    fn the_wheel_picks_by_the_stick() {
        let mut bus = Bus::new(std::path::PathBuf::from("."));
        let (mapping, _) =
            PadMapping::parse(&lines(&[("l", "wheel"), ("wheel_1", "1 Up"), ("wheel_2", "2 Right"), ("wheel_3", "3 Down"), ("wheel_4", "4 Left")]));
        let mut mapper = PadMapper::new(mapping.unwrap());
        let l = input_bit("l").unwrap();
        assert_eq!(mapper.update(&mut bus, l, (0.0, 0.0)).unwrap().selected, None);
        assert_eq!(mapper.update(&mut bus, l, (1.0, 0.0)).unwrap().selected, Some(1));
        assert_eq!(mapper.update(&mut bus, l, (0.0, 1.0)).unwrap().selected, Some(2));
        assert!(mapper.update(&mut bus, 0, (0.0, 0.0)).is_none(), "closed");
        assert_eq!(mapper.keys[&(0x04, false)].1, 1, "3 is pressed");
        for _ in 0..WHEEL_PRESS_FRAMES {
            mapper.update(&mut bus, 0, (0.0, 0.0));
        }
        assert_eq!(mapper.keys[&(0x04, false)].1, 0, "and let go");
    }
}
