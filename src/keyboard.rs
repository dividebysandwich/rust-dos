use sdl2::keyboard::Keycode;
use sdl2::keyboard::Mod;

use crate::bus::Bus;

/// Keystrokes the BIOS keyboard buffer at 40:1E holds.
pub const BIOS_BUFFER_KEYS: usize = 15;

/// Send a key's make or break code to the keyboard controller, with the E0
/// prefix of the extended keys.
fn send_scan(bus: &mut Bus, scan: u8, extended: bool) {
    if extended {
        bus.kbc.push_scancodes(&[0xE0, scan]);
    } else {
        bus.kbc.push_scancodes(&[scan]);
    }
    bus.sync_keyboard_irq();
}

/// Deliver a key press: queue `(scan << 8) | ascii` for BIOS INT 16h readers,
/// and send the make code to the keyboard controller for programs that read
/// port 60h or install their own INT 09h ISR.
pub fn deliver_key_down(bus: &mut Bus, code: u16, extended: bool) {
    // The BIOS buffer holds 15 keystrokes; when it's full (a program that
    // reads the keyboard itself never empties it) new ones are dropped.
    if bus.keyboard_buffer.len() < BIOS_BUFFER_KEYS {
        bus.keyboard_buffer.push_back(code);
    }
    send_scan(bus, (code >> 8) as u8, extended);
}

/// Deliver a key release: the break code (scan code | 80h). Games that track
/// held keys (arrow-key movement, etc.) need these to know when the key
/// stops being pressed.
pub fn deliver_key_up(bus: &mut Bus, scan: u8, extended: bool) {
    if scan != 0 {
        send_scan(bus, scan | 0x80, extended);
    }
}

/// Deliver a make code without touching the INT 16h buffer (modifier keys
/// like Shift produce make/break codes but no buffered keystroke).
pub fn deliver_scan_only(bus: &mut Bus, scan: u8, extended: bool) {
    send_scan(bus, scan, extended);
}

/// Extended keys, which send an E0 prefix: the grey cursor block, keypad
/// Enter and /, right Ctrl and right Alt.
pub fn is_extended(keycode: Keycode) -> bool {
    matches!(
        keycode,
        Keycode::Up
            | Keycode::Down
            | Keycode::Left
            | Keycode::Right
            | Keycode::Home
            | Keycode::End
            | Keycode::PageUp
            | Keycode::PageDown
            | Keycode::Insert
            | Keycode::Delete
            | Keycode::KpEnter
            | Keycode::KpDivide
            | Keycode::RCtrl
            | Keycode::RAlt
    )
}

/// Scan code of a modifier key, which has no INT 16h keystroke.
pub fn modifier_scan(keycode: Keycode) -> Option<u8> {
    match keycode {
        Keycode::LShift => Some(0x2A),
        Keycode::RShift => Some(0x36),
        Keycode::LCtrl | Keycode::RCtrl => Some(0x1D),
        Keycode::LAlt | Keycode::RAlt => Some(0x38),
        Keycode::CapsLock => Some(0x3A),
        _ => None,
    }
}

/// Returns a tuple of (Scancode, ASCII) for a given SDL Keycode.
/// Scancode is the high byte, ASCII is the low byte.
pub fn map_sdl_to_pc(keycode: Keycode, keymod: Mod) -> Option<u16> {
    let shift = keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD);
    let _ctrl = keymod.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD);
    let _alt = keymod.intersects(Mod::LALTMOD | Mod::RALTMOD);

    // Construct u16 from (Scan, Ascii)
    let k = |scan: u8, ascii: u8| Some(((scan as u16) << 8) | (ascii as u16));

    match keycode {
        // Alphanumeric (Respects Shift)
        Keycode::A => if shift { k(0x1E, b'A') } else { k(0x1E, b'a') },
        Keycode::B => if shift { k(0x30, b'B') } else { k(0x30, b'b') },
        Keycode::C => if shift { k(0x2E, b'C') } else { k(0x2E, b'c') },
        Keycode::D => if shift { k(0x20, b'D') } else { k(0x20, b'd') },
        Keycode::E => if shift { k(0x12, b'E') } else { k(0x12, b'e') },
        Keycode::F => if shift { k(0x21, b'F') } else { k(0x21, b'f') },
        Keycode::G => if shift { k(0x22, b'G') } else { k(0x22, b'g') },
        Keycode::H => if shift { k(0x23, b'H') } else { k(0x23, b'h') },
        Keycode::I => if shift { k(0x17, b'I') } else { k(0x17, b'i') },
        Keycode::J => if shift { k(0x24, b'J') } else { k(0x24, b'j') },
        Keycode::K => if shift { k(0x25, b'K') } else { k(0x25, b'k') },
        Keycode::L => if shift { k(0x26, b'L') } else { k(0x26, b'l') },
        Keycode::M => if shift { k(0x32, b'M') } else { k(0x32, b'm') },
        Keycode::N => if shift { k(0x31, b'N') } else { k(0x31, b'n') },
        Keycode::O => if shift { k(0x18, b'O') } else { k(0x18, b'o') },
        Keycode::P => if shift { k(0x19, b'P') } else { k(0x19, b'p') },
        Keycode::Q => if shift { k(0x10, b'Q') } else { k(0x10, b'q') },
        Keycode::R => if shift { k(0x13, b'R') } else { k(0x13, b'r') },
        Keycode::S => if shift { k(0x1F, b'S') } else { k(0x1F, b's') },
        Keycode::T => if shift { k(0x14, b'T') } else { k(0x14, b't') },
        Keycode::U => if shift { k(0x16, b'U') } else { k(0x16, b'u') },
        Keycode::V => if shift { k(0x2F, b'V') } else { k(0x2F, b'v') },
        Keycode::W => if shift { k(0x11, b'W') } else { k(0x11, b'w') },
        Keycode::X => if shift { k(0x2D, b'X') } else { k(0x2D, b'x') },
        Keycode::Y => if shift { k(0x15, b'Y') } else { k(0x15, b'y') },
        Keycode::Z => if shift { k(0x2C, b'Z') } else { k(0x2C, b'z') },

        // Numbers (Top Row)
        Keycode::Num0 => if shift { k(0x0B, b')') } else { k(0x0B, b'0') },
        Keycode::Num1 => if shift { k(0x02, b'!') } else { k(0x02, b'1') },
        Keycode::Num2 => if shift { k(0x03, b'@') } else { k(0x03, b'2') },
        Keycode::Num3 => if shift { k(0x04, b'#') } else { k(0x04, b'3') },
        Keycode::Num4 => if shift { k(0x05, b'$') } else { k(0x05, b'4') },
        Keycode::Num5 => if shift { k(0x06, b'%') } else { k(0x06, b'5') },
        Keycode::Num6 => if shift { k(0x07, b'^') } else { k(0x07, b'6') },
        Keycode::Num7 => if shift { k(0x08, b'&') } else { k(0x08, b'7') },
        Keycode::Num8 => if shift { k(0x09, b'*') } else { k(0x09, b'8') },
        Keycode::Num9 => if shift { k(0x0A, b'(') } else { k(0x0A, b'9') },

        // Special Characters
        Keycode::Space => k(0x39, b' '),
        Keycode::Return => k(0x1C, 0x0D),
        Keycode::Backspace => k(0x0E, 0x08),
        Keycode::Tab => k(0x0F, 0x09),
        Keycode::Escape => k(0x01, 0x1B),
        Keycode::Minus => if shift { k(0x0C, b'_') } else { k(0x0C, b'-') },
        Keycode::Equals => if shift { k(0x0D, b'+') } else { k(0x0D, b'=') },
        Keycode::LeftBracket => if shift { k(0x1A, b'{') } else { k(0x1A, b'[') },
        Keycode::RightBracket => if shift { k(0x1B, b'}') } else { k(0x1B, b']') },
        Keycode::Backslash => if shift { k(0x2B, b'|') } else { k(0x2B, b'\\') },
        Keycode::Semicolon => if shift { k(0x27, b':') } else { k(0x27, b';') },
        Keycode::Quote => if shift { k(0x28, b'"') } else { k(0x28, b'\'') },
        Keycode::Comma => if shift { k(0x33, b'<') } else { k(0x33, b',') },
        Keycode::Period => if shift { k(0x34, b'>') } else { k(0x34, b'.') },
        Keycode::Slash => if shift { k(0x35, b'?') } else { k(0x35, b'/') },
        Keycode::Backquote => if shift { k(0x29, b'~') } else { k(0x29, b'`') },

        // Function Keys (F1-F10: Standard | F11-F12: Extended)
        Keycode::F1 => k(0x3B, 0),
        Keycode::F2 => k(0x3C, 0),
        Keycode::F3 => k(0x3D, 0),
        Keycode::F4 => k(0x3E, 0),
        Keycode::F5 => k(0x3F, 0),
        Keycode::F6 => k(0x40, 0),
        Keycode::F7 => k(0x41, 0),
        Keycode::F8 => k(0x42, 0),
        Keycode::F9 => k(0x43, 0),
        Keycode::F10 => k(0x44, 0),
        Keycode::F11 => k(0x85, 0),
        Keycode::F12 => k(0x86, 0),

        // Navigation / Editing (Extended Keys usually have 0x00 or 0xE0 prefix)
        // DOS usually returns 0x00 as the ASCII code for these extended keys.
        Keycode::Up => k(0x48, 0),
        Keycode::Down => k(0x50, 0),
        Keycode::Left => k(0x4B, 0),
        Keycode::Right => k(0x4D, 0),
        Keycode::Home => k(0x47, 0),
        Keycode::End => k(0x4F, 0),
        Keycode::PageUp => k(0x49, 0),
        Keycode::PageDown => k(0x51, 0),
        Keycode::Insert => k(0x52, 0),
        Keycode::Delete => k(0x53, 0), // Note: Sometimes 0xE0 prefix in modern BIOS

        // Keypad (Assuming NumLock Off for navigation, On for numbers)
        // Simplified: Always treat as Numbers for now
        Keycode::Kp0 => k(0x52, b'0'),
        Keycode::Kp1 => k(0x4F, b'1'),
        Keycode::Kp2 => k(0x50, b'2'),
        Keycode::Kp3 => k(0x51, b'3'),
        Keycode::Kp4 => k(0x4B, b'4'),
        Keycode::Kp5 => k(0x4C, b'5'),
        Keycode::Kp6 => k(0x4D, b'6'),
        Keycode::Kp7 => k(0x47, b'7'),
        Keycode::Kp8 => k(0x48, b'8'),
        Keycode::Kp9 => k(0x49, b'9'),
        Keycode::KpPeriod => k(0x53, b'.'),
        Keycode::KpPlus => k(0x4E, b'+'),
        Keycode::KpMinus => k(0x4A, b'-'),
        Keycode::KpMultiply => k(0x37, b'*'),
        Keycode::KpDivide => k(0x35, b'/'),
        Keycode::KpEnter => k(0x1C, 0x0D), // Treated same as main Enter

        _ => None,
    }
}