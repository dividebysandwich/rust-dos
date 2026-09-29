//! The frontend's keys as PC keys. libretro's key codes (`retro_key`) name
//! the keys of a US keyboard by where they are, as the PC's scan codes do,
//! whatever the host's layout types with them; the machine's own layout
//! (`keyboard_layout`) makes the characters.

use crate::ffi::key;
use rust_dos::config_ui::UiKey;

/// The PC keyboard's set-1 scan code of the key `keycode` names, and
/// whether it sends E0 first.
pub fn pc_scan(keycode: u32) -> Option<(u8, bool)> {
    const LETTERS: [u8; 26] = [
        0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10, 0x13,
        0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
    ];
    // 0-9 across the top row, and on the keypad.
    const DIGITS: [u8; 10] = [0x0B, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A];
    const KEYPAD: [u8; 10] = [0x52, 0x4F, 0x50, 0x51, 0x4B, 0x4C, 0x4D, 0x47, 0x48, 0x49];
    let plain = |scan: u8| Some((scan, false));
    let extended = |scan: u8| Some((scan, true));
    match keycode {
        key::A..=key::Z => plain(LETTERS[(keycode - key::A) as usize]),
        key::N0..=key::N9 => plain(DIGITS[(keycode - key::N0) as usize]),
        key::KP0..=key::KP9 => plain(KEYPAD[(keycode - key::KP0) as usize]),
        key::F1..=key::F12 => {
            let n = keycode - key::F1;
            // F11 and F12 came later, apart from the others.
            plain(if n < 10 { 0x3B + n as u8 } else { 0x57 + (n - 10) as u8 })
        }
        key::RETURN => plain(0x1C),
        key::ESCAPE => plain(0x01),
        key::BACKSPACE => plain(0x0E),
        key::TAB => plain(0x0F),
        key::SPACE => plain(0x39),
        key::MINUS => plain(0x0C),
        key::EQUALS => plain(0x0D),
        key::LEFTBRACKET => plain(0x1A),
        key::RIGHTBRACKET => plain(0x1B),
        key::BACKSLASH => plain(0x2B),
        key::SEMICOLON => plain(0x27),
        key::QUOTE => plain(0x28),
        key::BACKQUOTE => plain(0x29),
        key::COMMA => plain(0x33),
        key::PERIOD => plain(0x34),
        key::SLASH => plain(0x35),
        key::OEM_102 => plain(0x56),
        key::CAPSLOCK => plain(0x3A),
        key::SCROLLOCK => plain(0x46),
        key::NUMLOCK => plain(0x45),
        key::INSERT => extended(0x52),
        key::HOME => extended(0x47),
        key::PAGEUP => extended(0x49),
        key::DELETE => extended(0x53),
        key::END => extended(0x4F),
        key::PAGEDOWN => extended(0x51),
        key::RIGHT => extended(0x4D),
        key::LEFT => extended(0x4B),
        key::DOWN => extended(0x50),
        key::UP => extended(0x48),
        key::KP_DIVIDE => extended(0x35),
        key::KP_MULTIPLY => plain(0x37),
        key::KP_MINUS => plain(0x4A),
        key::KP_PLUS => plain(0x4E),
        key::KP_ENTER => extended(0x1C),
        key::KP_PERIOD => plain(0x53),
        key::LCTRL => plain(0x1D),
        key::LSHIFT => plain(0x2A),
        key::LALT => plain(0x38),
        key::RCTRL => extended(0x1D),
        key::RSHIFT => plain(0x36),
        key::RALT => extended(0x38),
        key::LSUPER => extended(0x5B),
        key::RSUPER => extended(0x5C),
        key::MENU => extended(0x5D),
        _ => None,
    }
}

/// The settings window's key for a key press, if it takes it. `character`
/// is what the key typed, if anything.
pub fn ui_key(keycode: u32, character: Option<char>, ctrl: bool, shift: bool) -> Option<UiKey> {
    Some(match keycode {
        key::UP => UiKey::Up,
        key::DOWN => UiKey::Down,
        key::LEFT => UiKey::Left,
        key::RIGHT => UiKey::Right,
        key::PAGEUP => UiKey::PageUp,
        key::PAGEDOWN => UiKey::PageDown,
        key::HOME => UiKey::Home,
        key::END => UiKey::End,
        key::RETURN | key::KP_ENTER => UiKey::Enter,
        key::ESCAPE => UiKey::Esc,
        key::TAB if shift => UiKey::BackTab,
        key::TAB => UiKey::Tab,
        key::BACKSPACE => UiKey::Backspace,
        key::DELETE => UiKey::Delete,
        key::INSERT => UiKey::Insert,
        key::F1 => UiKey::Help,
        key::F2 => UiKey::Save,
        _ if ctrl && keycode == u32::from(b's') => UiKey::Save,
        _ if ctrl => return None,
        _ => UiKey::Char(character.filter(|c| !c.is_control())?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_where_a_pc_has_them() {
        assert_eq!(pc_scan(u32::from(b'a')), Some((0x1E, false)));
        assert_eq!(pc_scan(u32::from(b'z')), Some((0x2C, false)));
        assert_eq!(pc_scan(u32::from(b'0')), Some((0x0B, false)));
        assert_eq!(pc_scan(u32::from(b'1')), Some((0x02, false)));
        assert_eq!(pc_scan(key::F1), Some((0x3B, false)));
        assert_eq!(pc_scan(key::F10), Some((0x44, false)));
        assert_eq!(pc_scan(key::F11), Some((0x57, false)));
        assert_eq!(pc_scan(key::F12), Some((0x58, false)));
        assert_eq!(pc_scan(key::KP0), Some((0x52, false)));
        assert_eq!(pc_scan(key::KP9), Some((0x49, false)));
        assert_eq!(pc_scan(key::UP), Some((0x48, true)));
        assert_eq!(pc_scan(key::RALT), Some((0x38, true)));
        assert_eq!(pc_scan(key::PAUSE), None);
    }

    #[test]
    fn the_settings_window_takes_its_keys() {
        assert_eq!(ui_key(key::TAB, None, false, true), Some(UiKey::BackTab));
        assert_eq!(ui_key(u32::from(b's'), Some('s'), true, false), Some(UiKey::Save));
        assert_eq!(ui_key(u32::from(b'x'), Some('X'), false, true), Some(UiKey::Char('X')));
        assert_eq!(ui_key(u32::from(b'x'), Some('x'), true, false), None);
        assert_eq!(ui_key(key::LSHIFT, None, false, true), None);
    }
}
