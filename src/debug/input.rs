//! Remote input: expanding events into key strokes and feeding them.

use std::time::{Duration, Instant};

use serde_json::json;

use super::journal::{Kind, OpState};
use super::keys::{self, PcKey};
use super::{Coords, DEFAULT_HOLD_MS, DebugHub, InputEvent, KeyAction, LowInput, MouseAction, UiInput};
use crate::config_ui::UiKey;
use crate::cpu::Cpu;
use crate::keyboard;
use crate::video;
use rust_dos::keylayout::Layout;

/// F12 in the key table: its make code.
const F12_SCAN: u8 = 0x58;
const S_SCAN: u8 = 0x1F;

fn modifier_key(name: &str) -> Result<PcKey, String> {
    match keys::lookup(name) {
        Some(k) if k.modifier != 0 => Ok(k),
        _ => Err(format!("unknown modifier '{}' (use shift, ctrl, alt)", name)),
    }
}

/// Compute the ASCII byte a key produces under the given BDA modifier bits.
fn ascii_for(key: PcKey, mods: u8) -> u8 {
    if mods & keys::MOD_ALT != 0 {
        return 0;
    }
    if mods & keys::MOD_CTRL != 0 {
        return if key.ascii.is_ascii_alphabetic() { key.ascii & 0x1F } else { key.ascii };
    }
    if mods & (keys::MOD_LSHIFT | keys::MOD_RSHIFT) != 0 { key.shifted } else { key.ascii }
}

/// The keys that type `c` in `layout` (`keyboard::keys_for_char`), as
/// queued input.
pub(crate) fn keys_for_char(c: char, layout: &Layout, out: &mut Vec<LowInput>) -> Result<(), String> {
    let mut typed = Vec::new();
    keyboard::keys_for_char(c, layout, &mut typed)?;
    out.extend(typed.into_iter().map(|t| match t {
        keyboard::TypeStep::Down { key, ascii } => LowInput::KeyDown { key, ascii },
        keyboard::TypeStep::Up { key } => LowInput::KeyUp { key },
        keyboard::TypeStep::Char(byte) => LowInput::Char(byte),
    }));
    Ok(())
}

pub(super) fn expand_input(ev: &InputEvent, layout: &Layout, out: &mut Vec<LowInput>) -> Result<(), String> {
    match ev {
        InputEvent::Key { key, scancode, ascii, action, mods, hold_ms } => {
            let mod_keys = mods.iter().map(|m| modifier_key(m)).collect::<Result<Vec<_>, _>>()?;
            let mod_bits = mod_keys.iter().fold(0u8, |a, k| a | k.modifier);
            let pc = match (key, scancode) {
                (Some(name), _) => keys::lookup(name).ok_or_else(|| {
                    format!("unknown key '{}'; valid keys: {}", name, keys::names().join(", "))
                })?,
                (None, Some(sc)) => PcKey { scan: *sc, ascii: ascii.unwrap_or(0), shifted: ascii.unwrap_or(0), modifier: 0, extended: false },
                (None, None) => return Err("key event needs 'key' or 'scancode'".into()),
            };
            let ch = match ascii {
                Some(a) if key.is_some() => *a,
                _ => ascii_for(pc, mod_bits),
            };
            if matches!(action, KeyAction::Press | KeyAction::Down) {
                for m in &mod_keys {
                    out.push(LowInput::KeyDown { key: *m, ascii: 0 });
                }
                out.push(LowInput::KeyDown { key: pc, ascii: ch });
            }
            if *action == KeyAction::Press {
                out.push(LowInput::Wait(Duration::from_millis(hold_ms.unwrap_or(DEFAULT_HOLD_MS))));
            }
            if matches!(action, KeyAction::Press | KeyAction::Up) {
                out.push(LowInput::KeyUp { key: pc });
                for m in mod_keys.iter().rev() {
                    out.push(LowInput::KeyUp { key: *m });
                }
            }
        }
        InputEvent::Type { text, delay_ms } => {
            for c in text.chars() {
                keys_for_char(c, layout, out)?;
                if let Some(d) = delay_ms.filter(|d| *d > 0) {
                    out.push(LowInput::Wait(Duration::from_millis(d)));
                }
            }
        }
        InputEvent::Mouse { action, x, y, dx, dy, button, coords, hold_ms } => {
            match (x, y) {
                (Some(x), Some(y)) => out.push(LowInput::MouseTo { x: *x, y: *y, coords: *coords }),
                (None, None) => {}
                _ => return Err("mouse event needs both x and y".into()),
            }
            if dx.is_some() || dy.is_some() {
                out.push(LowInput::MouseRel { dx: dx.unwrap_or(0), dy: dy.unwrap_or(0) });
            }
            let idx = match button.as_deref().unwrap_or("left") {
                "left" | "l" | "0" => 0,
                "right" | "r" | "1" => 1,
                "middle" | "m" | "2" => 2,
                b => return Err(format!("unknown mouse button '{}'", b)),
            };
            match action {
                MouseAction::Move => {}
                MouseAction::Down => out.push(LowInput::Button { idx, down: true }),
                MouseAction::Up => out.push(LowInput::Button { idx, down: false }),
                MouseAction::Click => {
                    out.push(LowInput::Button { idx, down: true });
                    out.push(LowInput::Wait(Duration::from_millis(hold_ms.unwrap_or(DEFAULT_HOLD_MS))));
                    out.push(LowInput::Button { idx, down: false });
                }
            }
        }
        InputEvent::Wait { ms } => out.push(LowInput::Wait(Duration::from_millis(*ms))),
    }
    Ok(())
}

impl DebugHub {
    /// Deliver queued input, in deterministic mode: the front end calls
    /// this at fixed points in emulated time.
    pub fn feed_input(&mut self, cpu: &mut Cpu) {
        self.process_input(cpu);
    }

    /// Deterministic mode: run a millisecond of emulated time at a time up
    /// to `end`, a pause, the program's exit, or the time a client asked
    /// to stop at. That time is checked before `end`, so that a frame
    /// ending at it pauses there. `at_ms` runs at every whole millisecond
    /// reached; at a tick it delivers the input.
    pub fn run_deterministic(
        &mut self,
        cpu: &mut Cpu,
        mode: &mut rust_dos::deterministic::Deterministic,
        hot: bool,
        end: u64,
        mut at_ms: impl FnMut(&mut Cpu, &mut DebugHub, rust_dos::deterministic::Boundary),
    ) {
        while !self.paused && !self.time_stop(cpu) && cpu.bus.clock.icount < end {
            let (reason, reached) = mode.step(cpu, self, hot, end);
            if let Some(boundary) = reached {
                // Program starts and ends are told at the millisecond, so
                // that the journal doesn't depend on the host's frames.
                if self.shared.is_some() {
                    self.tell_programs(cpu);
                }
                at_ms(cpu, self, boundary);
            }
            if matches!(reason, crate::exec::StopReason::Paused | crate::exec::StopReason::Exit | crate::exec::StopReason::Hook) {
                break;
            }
        }
    }

    // ----- input ------------------------------------------------------------

    pub(super) fn process_input(&mut self, cpu: &mut Cpu) {
        let now = Instant::now();
        let now_ns = cpu.bus.clock.now_ns();
        if let Some(t) = self.input_wait_until {
            if now < t {
                return;
            }
            self.input_wait_until = None;
        }
        // Emulated time stands still while the settings window pauses the
        // machine: a wait in emulated time would never end.
        if self.divert {
            self.input_wait_ns = None;
        }
        if let Some(t) = self.input_wait_ns {
            if now_ns < t {
                return;
            }
            self.input_wait_ns = None;
        }
        while let Some(ev) = self.input.front() {
            match ev {
                LowInput::KeyDown { .. } | LowInput::KeyUp { .. } => {
                    // Let the program catch up with the keyboard controller's
                    // queue before adding more, but don't stall forever on
                    // programs that never read the port. The settings window
                    // takes keys at once.
                    if !self.divert && cpu.bus.kbc.pending() > 0 && self.key_stall_frames < 2 {
                        self.key_stall_frames += 1;
                        return;
                    }
                    self.key_stall_frames = 0;
                    match self.input.pop_front().unwrap() {
                        LowInput::KeyDown { key, ascii } => self.key_down(cpu, key, ascii),
                        LowInput::KeyUp { key } => self.key_up(cpu, key),
                        _ => unreachable!(),
                    }
                    // One scan code per frame.
                    return;
                }
                LowInput::Wait(d) if self.deterministic.is_some() && !self.divert => {
                    self.input_wait_ns = Some(now_ns + d.as_nanos() as u64);
                    self.input.pop_front();
                    return;
                }
                LowInput::Wait(d) => {
                    self.input_wait_until = Some(now + *d);
                    self.input.pop_front();
                    return;
                }
                _ => {}
            }
            match self.input.pop_front().unwrap() {
                // Typed as Alt and the keypad type it: a keystroke with no
                // scan code.
                LowInput::Char(byte) if !self.divert => {
                    keyboard::queue_keystroke(&mut cpu.bus, byte as u16);
                }
                LowInput::Char(_) => {}
                LowInput::MouseTo { x, y, coords: Coords::Screen } if self.divert => self.ui_pointer = (x, y),
                LowInput::Button { idx: 0, down: true } if self.divert => {
                    self.ui_input.push(UiInput::Click(self.ui_pointer.0, self.ui_pointer.1));
                }
                LowInput::MouseTo { .. } | LowInput::MouseRel { .. } | LowInput::Button { .. } if self.divert => {}
                LowInput::MouseTo { x, y, coords } => {
                    let (vx, vy) = match coords {
                        Coords::Virtual => (x, y),
                        Coords::Screen => screen_to_virtual_mouse(cpu, x, y),
                    };
                    cpu.bus.mouse.set_position(vx, vy);
                }
                // As a captured mouse moves: all of the motion counts, at
                // the edges of the cursor's window too.
                LowInput::MouseRel { dx, dy } => cpu.bus.mouse.move_by(dx as f64, dy as f64),
                LowInput::Button { idx, down } => {
                    if down {
                        cpu.bus.mouse.button_down(idx);
                    } else {
                        cpu.bus.mouse.button_up(idx);
                    }
                }
                LowInput::End { req } => {
                    self.append_then(cpu, Kind::InputDelivered, Some(req), json!({}), |j, seq| {
                        j.end_op(req, OpState::Done, Some(seq));
                    });
                }
                _ => unreachable!(),
            }
        }
    }

    /// A remote key press: to the settings window while it is open, else
    /// to the machine. Ctrl+F12 toggles the window, as on the keyboard.
    pub(super) fn key_down(&mut self, cpu: &mut Cpu, key: PcKey, ascii: u8) {
        self.remote_mods |= key.modifier;
        let shift = self.remote_mods & (keys::MOD_LSHIFT | keys::MOD_RSHIFT) != 0;
        let ctrl = self.remote_mods & (keys::MOD_CTRL | keys::MOD_ALT) == keys::MOD_CTRL;
        if key.scan == F12_SCAN && ctrl {
            self.hotkey = true;
        } else if key.scan == S_SCAN && ctrl && shift && !self.divert {
            self.sound_viz_key = true;
        } else if self.divert {
            self.ui_input.extend(ui_key(key, ascii, self.remote_mods).map(UiInput::Key));
        } else {
            keyboard::apply_key(&mut cpu.bus, key, ascii, true);
            if !self.remote_held.iter().any(|k| same_key(k, &key)) {
                self.remote_held.push(key);
            }
        }
    }

    /// A remote key release. Only keys the machine saw go down come up.
    fn key_up(&mut self, cpu: &mut Cpu, key: PcKey) {
        self.remote_mods &= !key.modifier;
        if let Some(i) = self.remote_held.iter().position(|k| same_key(k, &key)) {
            self.remote_held.remove(i);
            keyboard::apply_key(&mut cpu.bus, key, 0, false);
        }
    }

    /// Release the keys the remote client holds on the machine, as the
    /// settings window opens and takes the keyboard.
    pub fn release_keys(&mut self, cpu: &mut Cpu) {
        for key in std::mem::take(&mut self.remote_held) {
            keyboard::apply_key(&mut cpu.bus, key, 0, false);
        }
    }

    /// Remote input for the settings window since the last call.
    pub fn take_ui_input(&mut self) -> Vec<UiInput> {
        std::mem::take(&mut self.ui_input)
    }

    /// Whether a remote Ctrl+F12 came in since the last call.
    pub fn take_hotkey(&mut self) -> bool {
        std::mem::take(&mut self.hotkey)
    }

    /// Whether a remote Ctrl+Shift+S came in since the last call.
    pub fn take_sound_viz_key(&mut self) -> bool {
        std::mem::take(&mut self.sound_viz_key)
    }

    /// Whether reboot_shell closed the program since the last call. The
    /// front end then stops its pad mapper, autoinput and game, as the
    /// settings window's close does.
    pub fn take_closed_program(&mut self) -> bool {
        std::mem::take(&mut self.closed_program)
    }
}

fn same_key(a: &PcKey, b: &PcKey) -> bool {
    (a.scan, a.extended) == (b.scan, b.extended)
}

/// What a remote key does in the settings window: `ascii` is the character
/// it types under the modifiers `mods`.
fn ui_key(key: PcKey, ascii: u8, mods: u8) -> Option<UiKey> {
    use UiKey::*;
    let shift = mods & (keys::MOD_LSHIFT | keys::MOD_RSHIFT) != 0;
    let ctrl = mods & keys::MOD_CTRL != 0;
    Some(match key.scan {
        _ if key.modifier != 0 => return None,
        0x48 => Up,
        0x50 => Down,
        0x4B => Left,
        0x4D => Right,
        0x49 => PageUp,
        0x51 => PageDown,
        0x47 => Home,
        0x4F => End,
        0x1C => Enter,
        0x01 => Esc,
        0x0F if shift => BackTab,
        0x0F => Tab,
        0x0E => Backspace,
        0x53 => Delete,
        0x52 => Insert,
        0x3B if !ctrl => Help,
        0x3C => Save,
        0x44 => F10,
        0x1F if ctrl => Save,
        _ if (0x20..0x7F).contains(&ascii) => Char(ascii as char),
        _ => return None,
    })
}

/// Map screenshot pixels (the picture's own size) into the mouse driver's
/// virtual coordinate system. Unlike captured SDL motion, an explicit screen
/// position must also span large absolute ranges (e.g. Allegro's 8x pixels).
fn screen_to_virtual_mouse(cpu: &Cpu, x: i32, y: i32) -> (i32, i32) {
    let (w, h) = video::frame_size(&cpu.bus);
    let (w, h) = (w as i32, h as i32);
    let px = x.clamp(0, w - 1);
    let py = y.clamp(0, h - 1);
    let (virt_w, virt_h) = cpu.bus.mouse.virtual_screen(&cpu.bus);
    let axis = |pixel: i32, size: i32, extent: i32, max: i32| {
        if max >= extent {
            // virtual_screen intentionally ignores large ranges for relative
            // mouse users. Absolute positions need their inclusive endpoints.
            (pixel as i64 * max as i64 / (size - 1).max(1) as i64) as i32
        } else {
            (pixel as i64 * extent as i64 / size as i64) as i32
        }
    };
    (axis(px, w, virt_w, cpu.bus.mouse.max_x), axis(py, h, virt_h, cpu.bus.mouse.max_y))
}
