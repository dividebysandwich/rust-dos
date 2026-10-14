//! The host's keyboard, mouse and game controllers as the machine and the
//! settings window take them.

use std::collections::HashMap;

use sdl2::keyboard::{Keycode, Mod, Scancode};
use sdl2::mouse::MouseButton;

use crate::config_ui::UiKey;
use crate::cpu::Cpu;
use crate::{joystick, keyboard, sdl_keys};
use rust_dos::games::ActiveGame;

/// Start the keys a game's profile presses once its program started, and
/// press the next. Returns what they are doing, None without any.
pub(crate) fn step_autoinput(
    cpu: &mut Cpu,
    game: &mut Option<ActiveGame>,
    autoinput: &mut Option<rust_dos::autoinput::AutoInput>,
) -> Option<rust_dos::autoinput::Status> {
    if autoinput.is_none()
        && let Some(started) = game.as_mut().and_then(|g| g.take_input(cpu))
    {
        *autoinput = Some(started);
    }
    let status = autoinput.as_mut().map(|input| input.step(cpu));
    if status == Some(rust_dos::autoinput::Status::Done) {
        *autoinput = None;
    }
    status
}

/// The settings window's key for a key press, if it takes it. Characters
/// come from text input, which follows the keyboard layout.
pub(crate) fn ui_key(keycode: Keycode, keymod: Mod) -> Option<UiKey> {
    let shift = keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD);
    let ctrl = keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) && !keymod.intersects(Mod::LALTMOD | Mod::RALTMOD);
    Some(match keycode {
        Keycode::Up => UiKey::Up,
        Keycode::Down => UiKey::Down,
        Keycode::Left => UiKey::Left,
        Keycode::Right => UiKey::Right,
        Keycode::PageUp => UiKey::PageUp,
        Keycode::PageDown => UiKey::PageDown,
        Keycode::Home => UiKey::Home,
        Keycode::End => UiKey::End,
        Keycode::Return | Keycode::KpEnter => UiKey::Enter,
        Keycode::Escape => UiKey::Esc,
        Keycode::Tab if shift => UiKey::BackTab,
        Keycode::Tab => UiKey::Tab,
        Keycode::Backspace => UiKey::Backspace,
        Keycode::Delete => UiKey::Delete,
        Keycode::Insert => UiKey::Insert,
        Keycode::F1 => UiKey::Help,
        Keycode::F2 => UiKey::Save,
        Keycode::F10 if !ctrl => UiKey::F10,
        Keycode::S if ctrl => UiKey::Save,
        _ => return None,
    })
}

/// Let go of everything the machine holds as the settings window takes the
/// keyboard and mouse: release the keys and buttons, so no game is left
/// with Ctrl or a fire button held down.
pub(crate) fn release_input(cpu: &mut Cpu, held: &mut HashMap<Scancode, (u8, bool)>) {
    held.clear();
    // Shift, Ctrl and Alt too; the lock states stay.
    keyboard::release_all(&mut cpu.bus);
    for button in 0..3 {
        if cpu.bus.mouse.buttons & (1 << button) != 0 {
            cpu.bus.mouse.button_up(button);
        }
    }
}

/// Caps Lock and Num Lock as the host's keyboard has them.
pub(crate) fn sync_locks(cpu: &mut Cpu, mods: Mod) {
    // A booted system's BIOS keeps its own locks, from the keys.
    if cpu.bus.boot.is_some() {
        return;
    }
    let mut flags = cpu.bus.read_8(0x0417) & !0x60;
    if mods.contains(Mod::CAPSMOD) {
        flags |= 0x40;
    }
    if mods.contains(Mod::NUMMOD) {
        flags |= 0x20;
    }
    cpu.bus.write_8(0x0417, flags);
}

/// The keyboard layout the machine types in: the setting's, or the host
/// keyboard's for auto.
pub(crate) fn apply_keyboard_layout(cpu: &mut Cpu, setting: rust_dos::keylayout::LayoutSetting) {
    let layout = setting.layout(sdl_keys::detect_layout());
    if !std::ptr::eq(cpu.bus.kbd.layout, layout) {
        cpu.bus.log_string(&format!("[INPUT] Keyboard layout: {} ({})", layout.name, layout.code));
        cpu.bus.kbd.layout = layout;
    }
}

/// A game controller's sticks and buttons, as the game port takes them.
/// All of a controller's inputs, for a game's gamepad mapping (padmap.rs).
pub(crate) fn pad_snapshot(pad: &sdl2::controller::GameController) -> rust_dos::padmap::PadSnapshot {
    use sdl2::controller::{Axis, Button};
    let axis = |axis| pad.axis(axis) as f32 / 32767.0;
    // In the order of padmap::INPUTS.
    let order = [
        Button::DPadUp,
        Button::DPadDown,
        Button::DPadLeft,
        Button::DPadRight,
        Button::A,
        Button::B,
        Button::X,
        Button::Y,
        Button::LeftShoulder,
        Button::RightShoulder,
    ];
    let mut buttons = order.iter().enumerate().filter(|(_, b)| pad.button(**b)).fold(0u32, |bits, (i, _)| bits | 1 << i);
    // L2 and R2 are axes here; L3, R3, Select and Start after them.
    for (i, button) in [(12, Button::LeftStick), (13, Button::RightStick), (14, Button::Back), (15, Button::Start)] {
        if pad.button(button) {
            buttons |= 1 << i;
        }
    }
    rust_dos::padmap::PadSnapshot {
        buttons,
        axes: [axis(Axis::LeftX), axis(Axis::LeftY), axis(Axis::RightX), axis(Axis::RightY)],
        triggers: [axis(Axis::TriggerLeft), axis(Axis::TriggerRight)],
    }
}

pub(crate) fn pad_state(pad: &sdl2::controller::GameController) -> joystick::PadState {
    use sdl2::controller::{Axis, Button};
    let axis = |axis| pad.axis(axis) as f32 / 32767.0;
    let buttons = [
        (Button::A, joystick::PAD_A),
        (Button::B, joystick::PAD_B),
        (Button::X, joystick::PAD_X),
        (Button::Y, joystick::PAD_Y),
        (Button::DPadUp, joystick::PAD_UP),
        (Button::DPadDown, joystick::PAD_DOWN),
        (Button::DPadLeft, joystick::PAD_LEFT),
        (Button::DPadRight, joystick::PAD_RIGHT),
    ]
    .into_iter()
    .filter(|&(button, _)| pad.button(button))
    .fold(0, |bits, (_, bit)| bits | bit);
    joystick::PadState {
        axes: [axis(Axis::LeftX), axis(Axis::LeftY), axis(Axis::RightX), axis(Axis::RightY)],
        buttons,
    }
}

pub(crate) fn sdl_button_to_index(button: MouseButton) -> Option<usize> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Right => Some(1),
        MouseButton::Middle => Some(2),
        _ => None,
    }
}

/// What the mouse captured says, with how to let it go.
pub(crate) const CAPTURED: &str = "Mouse captured (Ctrl+Alt releases)";

/// The host's key as Ctrl+Alt sees it.
pub(crate) fn chord_key(scancode: Option<Scancode>) -> rust_dos::mouse_capture::ChordKey {
    use rust_dos::mouse_capture::ChordKey;
    match scancode {
        Some(Scancode::LCtrl) => ChordKey::LeftCtrl,
        Some(Scancode::RCtrl) => ChordKey::RightCtrl,
        Some(Scancode::LAlt) => ChordKey::LeftAlt,
        Some(Scancode::RAlt) => ChordKey::RightAlt,
        _ => ChordKey::Other,
    }
}
