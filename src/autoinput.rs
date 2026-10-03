//! Keys a game's profile presses as it starts (`[game]`'s `input`, as
//! .dosz packages' DOS.YML `run_input` has them): characters typed, keys
//! pressed (`(ENTER)`) or held and let go (`(leftctrl:DOWN)` ...
//! `(leftctrl:UP)`), and waits: `(WAIT:500)` for that many emulated
//! milliseconds, `(WAITMODECHANGE)` until the video mode changes, and
//! `(DELAY:15)` for the time between keys (70 ms at first). The front ends
//! run the machine fast through the waits.

use crate::cpu::Cpu;
use crate::keyboard::{self, PcKey, TypeStep};
use std::collections::VecDeque;

/// How long a pressed key is held.
const HOLD_MS: u64 = 70;
/// The time between keys, unless `(DELAY:n)` says otherwise.
const DEFAULT_DELAY_MS: u64 = 70;
/// The longest a `(WAITMODECHANGE)` waits, in emulated milliseconds: a
/// mode the game never changes to doesn't hold the keys after it for ever.
const MODE_WAIT_MS: u64 = 60_000;
/// Frames a key waits for the program to read the one before.
const STALL_FRAMES: u32 = 60;

#[derive(Clone, Copy, Debug)]
pub enum Step {
    Type(char),
    Press(PcKey),
    Down(PcKey),
    Up(PcKey),
    Wait(u64),
    WaitModeChange,
    Delay(u64),
}

/// The steps of a sequence, and what in it couldn't be read.
pub fn parse(text: &str) -> (Vec<Step>, Vec<String>) {
    let (mut steps, mut warnings) = (Vec::new(), Vec::new());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '(' {
            steps.push(Step::Type(c));
            continue;
        }
        let command: String = chars.by_ref().take_while(|&c| c != ')').collect();
        let (name, arg) = match command.split_once(':') {
            Some((name, arg)) => (name.trim(), Some(arg.trim())),
            None => (command.trim(), None),
        };
        let number = arg.and_then(|a| a.parse::<u64>().ok());
        let step = match (name.to_ascii_uppercase().as_str(), arg.map(str::to_ascii_uppercase).as_deref()) {
            ("WAIT", _) => number.map(Step::Wait),
            ("DELAY", _) => number.map(Step::Delay),
            ("WAITMODECHANGE", None) => Some(Step::WaitModeChange),
            (_, None) => keyboard::lookup(name).map(Step::Press),
            (_, Some("DOWN")) => keyboard::lookup(name).map(Step::Down),
            (_, Some("UP")) => keyboard::lookup(name).map(Step::Up),
            _ => None,
        };
        match step {
            Some(step) => steps.push(step),
            None => warnings.push(format!("input: ({}) isn't a key or a wait", command)),
        }
    }
    (steps, warnings)
}

/// What the sequence is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Pressing keys, or waiting: through `(WAIT)` and `(WAITMODECHANGE)`
    /// the machine may run fast.
    Busy { fast_forward: bool },
    Done,
}

/// The video mode and picture size, which `(WAITMODECHANGE)` waits on.
type Mode = (crate::video::VideoMode, (u32, u32));

fn mode(cpu: &Cpu) -> Mode {
    (cpu.bus.video_mode, crate::video::frame_size(&cpu.bus))
}

/// A sequence being pressed.
pub struct AutoInput {
    steps: VecDeque<Step>,
    /// The keys of the character or key being typed.
    typing: VecDeque<TypeStep>,
    /// Keys down that the sequence will let go.
    held: Vec<PcKey>,
    /// Nothing more until this emulated time (ns), and whether the machine
    /// may run fast until then.
    until: Option<(u64, bool)>,
    /// `(WAITMODECHANGE)`: the mode it started in, and when it gives up.
    mode: Option<(Mode, u64)>,
    delay_ms: u64,
    stall: u32,
}

impl AutoInput {
    pub fn new(steps: Vec<Step>) -> Self {
        Self {
            steps: steps.into(),
            typing: VecDeque::new(),
            held: Vec::new(),
            until: None,
            mode: None,
            delay_ms: DEFAULT_DELAY_MS,
            stall: 0,
        }
    }

    /// Press the next key, or keep waiting. Call once a frame while the
    /// machine runs.
    pub fn step(&mut self, cpu: &mut Cpu) -> Status {
        let now = cpu.bus.clock.now_ns();
        if let Some((until, fast)) = self.until {
            if now < until {
                return Status::Busy { fast_forward: fast };
            }
            self.until = None;
        }
        if let Some((from, give_up)) = self.mode {
            if mode(cpu) == from && now < give_up {
                return Status::Busy { fast_forward: true };
            }
            self.mode = None;
        }
        loop {
            if let Some(typed) = self.typing.front().copied() {
                // The program reads a key before the next goes in.
                let bios = keyboard::bios_keystrokes(&cpu.bus);
                let full = !bios && keyboard::queued_keystrokes(&mut cpu.bus) >= keyboard::BIOS_BUFFER_KEYS;
                let busy = cpu.bus.kbc.pending() > 0 || (full && !matches!(typed, TypeStep::Up { .. }));
                if busy && self.stall < STALL_FRAMES {
                    self.stall += 1;
                    return Status::Busy { fast_forward: false };
                }
                self.stall = 0;
                self.typing.pop_front();
                match typed {
                    TypeStep::Down { key, ascii } => {
                        keyboard::apply_key(&mut cpu.bus, key, ascii, true);
                        self.held.push(key);
                        self.wait(now, HOLD_MS, false);
                    }
                    TypeStep::Up { key } => {
                        self.release(cpu, key);
                        if self.typing.is_empty() {
                            self.wait(now, self.delay_ms, false);
                        }
                    }
                    TypeStep::Char(byte) => {
                        keyboard::queue_keystroke(&mut cpu.bus, byte as u16);
                        self.wait(now, self.delay_ms, false);
                    }
                }
                return Status::Busy { fast_forward: false };
            }
            let Some(step) = self.steps.pop_front() else { return Status::Done };
            match step {
                Step::Type(c) => {
                    let mut keys = Vec::new();
                    // A character that can't be typed is left out.
                    let _ = keyboard::keys_for_char(c, cpu.bus.typing_layout(), &mut keys);
                    self.typing.extend(keys);
                }
                Step::Press(key) => self.typing.extend([TypeStep::Down { key, ascii: key.ascii }, TypeStep::Up { key }]),
                // A key held or let go on its own is no wait.
                Step::Down(key) => {
                    keyboard::apply_key(&mut cpu.bus, key, key.ascii, true);
                    self.held.push(key);
                    return Status::Busy { fast_forward: false };
                }
                Step::Up(key) => {
                    self.release(cpu, key);
                    return Status::Busy { fast_forward: false };
                }
                Step::Wait(ms) => {
                    self.wait(now, ms, true);
                    return Status::Busy { fast_forward: true };
                }
                Step::WaitModeChange => {
                    self.mode = Some((mode(cpu), now + MODE_WAIT_MS * 1_000_000));
                    return Status::Busy { fast_forward: true };
                }
                Step::Delay(ms) => self.delay_ms = ms,
            }
        }
    }

    fn wait(&mut self, now: u64, ms: u64, fast: bool) {
        self.until = Some((now + ms * 1_000_000, fast));
    }

    fn release(&mut self, cpu: &mut Cpu, key: PcKey) {
        if let Some(i) = self.held.iter().position(|k| k.scan == key.scan && k.extended == key.extended) {
            self.held.remove(i);
        }
        keyboard::apply_key(&mut cpu.bus, key, 0, false);
    }

    /// Stop, letting go of the keys it holds: the player took over, or the
    /// game ended.
    pub fn stop(&mut self, cpu: &mut Cpu) {
        for key in std::mem::take(&mut self.held) {
            keyboard::apply_key(&mut cpu.bus, key, 0, false);
        }
        self.steps.clear();
        self.typing.clear();
        self.until = None;
        self.mode = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequences_are_read() {
        let (steps, warnings) = parse("(WAIT:200)(ENTER)(WAITMODECHANGE)(DELAY:15)i5(leftctrl:DOWN)c(LEFTCTRL:UP)(nokey)");
        assert_eq!(warnings.len(), 1, "{:?}", warnings);
        let shown: Vec<String> = steps
            .iter()
            .map(|s| match s {
                Step::Type(c) => format!("type {}", c),
                Step::Press(k) => format!("press {:02X}", k.scan),
                Step::Down(k) => format!("down {:02X}", k.scan),
                Step::Up(k) => format!("up {:02X}", k.scan),
                Step::Wait(ms) => format!("wait {}", ms),
                Step::WaitModeChange => "mode".to_string(),
                Step::Delay(ms) => format!("delay {}", ms),
            })
            .collect();
        assert_eq!(
            shown,
            ["wait 200", "press 1C", "mode", "delay 15", "type i", "type 5", "down 1D", "type c", "up 1D"]
        );
    }

    #[test]
    fn keys_go_in_at_emulated_times() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.load_shell();
        let (steps, _) = parse("(WAIT:100)a");
        let mut input = AutoInput::new(steps);
        assert_eq!(input.step(&mut cpu), Status::Busy { fast_forward: true }, "waiting");
        let start = cpu.bus.clock.now_ns();
        // Not yet: the time is emulated, not the host's.
        assert_eq!(input.step(&mut cpu), Status::Busy { fast_forward: true });
        let mut steps = 0;
        while input.step(&mut cpu) != Status::Done {
            let end = cpu.bus.clock.icount + 1000;
            cpu.bus.start_batch(end);
            crate::exec::run_batch(&mut cpu, &mut crate::exec::NoHook, false);
            steps += 1;
            assert!(steps < 10_000, "it finishes");
        }
        assert!(cpu.bus.clock.now_ns() >= start + 100_000_000);
        assert!(input.held.is_empty(), "the key is let go");
    }
}
