//! The keys RC takes, from the words INT 16h AH=10h returns: the scan
//! code in the high byte, the character in the low one.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A character typed, in code page 437.
    Char(u8),
    Enter,
    CtrlEnter,
    Esc,
    Tab,
    ShiftTab,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PgUp,
    PgDn,
    CtrlPgUp,
    CtrlPgDn,
    CtrlLeft,
    CtrlRight,
    Ins,
    Del,
    /// F1 to F12, and with Shift, Ctrl or Alt held.
    F(u8),
    ShiftF(u8),
    CtrlF(u8),
    AltF(u8),
    /// Ctrl and a letter, as the upper-case letter.
    Ctrl(u8),
    /// Alt and a letter or digit, upper case.
    Alt(u8),
    /// The keypad's grey + - and *.
    GrayPlus,
    GrayMinus,
    GrayStar,
    /// Ctrl+[ and Ctrl+].
    CtrlLeftBracket,
    CtrlRightBracket,
    Other,
}

/// Alt and a key's scan code: the letters' and digits' scan codes.
const ALT_LETTERS: [(u8, u8); 26] = [
    (0x10, b'Q'),
    (0x11, b'W'),
    (0x12, b'E'),
    (0x13, b'R'),
    (0x14, b'T'),
    (0x15, b'Y'),
    (0x16, b'U'),
    (0x17, b'I'),
    (0x18, b'O'),
    (0x19, b'P'),
    (0x1E, b'A'),
    (0x1F, b'S'),
    (0x20, b'D'),
    (0x21, b'F'),
    (0x22, b'G'),
    (0x23, b'H'),
    (0x24, b'J'),
    (0x25, b'K'),
    (0x26, b'L'),
    (0x2C, b'Z'),
    (0x2D, b'X'),
    (0x2E, b'C'),
    (0x2F, b'V'),
    (0x30, b'B'),
    (0x31, b'N'),
    (0x32, b'M'),
];

pub fn decode(word: u16) -> Key {
    let scan = (word >> 8) as u8;
    let ch = word as u8;
    // Grey keys come with E0h for the character.
    let ch = if ch == 0xE0 && scan != 0 { 0 } else { ch };
    match (scan, ch) {
        (0x1C | 0xE0, 0x0D) => return Key::Enter,
        (0x1C | 0xE0, 0x0A) => return Key::CtrlEnter,
        (0x01, 0x1B) => return Key::Esc,
        (0x0F, 0x09) => return Key::Tab,
        (0x0F, 0x00) => return Key::ShiftTab,
        (0x0E, 0x08) => return Key::Backspace,
        (0x4E, b'+') => return Key::GrayPlus,
        (0x4A, b'-') => return Key::GrayMinus,
        (0x37, b'*') => return Key::GrayStar,
        (0x1A, 0x1B) => return Key::CtrlLeftBracket,
        (0x1B, 0x1D) => return Key::CtrlRightBracket,
        _ => {}
    }
    if ch == 0 {
        return match scan {
            0x48 => Key::Up,
            0x50 => Key::Down,
            0x4B => Key::Left,
            0x4D => Key::Right,
            0x47 => Key::Home,
            0x4F => Key::End,
            0x49 => Key::PgUp,
            0x51 => Key::PgDn,
            0x84 => Key::CtrlPgUp,
            0x76 => Key::CtrlPgDn,
            0x73 => Key::CtrlLeft,
            0x74 => Key::CtrlRight,
            0x52 => Key::Ins,
            0x53 => Key::Del,
            0x3B..=0x44 => Key::F(scan - 0x3A),
            0x54..=0x5D => Key::ShiftF(scan - 0x53),
            0x5E..=0x67 => Key::CtrlF(scan - 0x5D),
            0x68..=0x71 => Key::AltF(scan - 0x67),
            0x85 | 0x86 => Key::F(scan - 0x85 + 11),
            0x87 | 0x88 => Key::ShiftF(scan - 0x87 + 11),
            0x89 | 0x8A => Key::CtrlF(scan - 0x89 + 11),
            0x8B | 0x8C => Key::AltF(scan - 0x8B + 11),
            0x78..=0x80 => Key::Alt(b'1' + scan - 0x78),
            0x81 => Key::Alt(b'0'),
            _ => ALT_LETTERS
                .iter()
                .find(|(s, _)| *s == scan)
                .map_or(Key::Other, |(_, l)| Key::Alt(*l)),
        };
    }
    // Ctrl and a letter: the letter's scan code with characters 1-26.
    // (Backspace, Tab, Enter and Esc are those of Ctrl+H, I, M and [.)
    if (1..=26).contains(&ch) && ALT_LETTERS.iter().any(|(s, _)| *s == scan) {
        return Key::Ctrl(b'A' + ch - 1);
    }
    if ch >= 0x20 && ch != 0x7F {
        return Key::Char(ch);
    }
    Key::Other
}

#[cfg(test)]
mod tests {
    use super::{Key, decode};

    #[test]
    fn decodes_nc_keys() {
        assert_eq!(decode(0x1C0D), Key::Enter);
        assert_eq!(decode(0x1C0A), Key::CtrlEnter);
        assert_eq!(decode(0x4E2B), Key::GrayPlus);
        assert_eq!(decode(0x0D2B), Key::Char(b'+'));
        assert_eq!(decode(0x372A), Key::GrayStar);
        assert_eq!(decode(0x6800), Key::AltF(1));
        assert_eq!(decode(0x6900), Key::AltF(2));
        assert_eq!(decode(0x5E00), Key::CtrlF(1));
        assert_eq!(decode(0x6000), Key::CtrlF(3));
        assert_eq!(decode(0x4400), Key::F(10));
        assert_eq!(decode(0x48E0), Key::Up);
        assert_eq!(decode(0x180F), Key::Ctrl(b'O'));
        assert_eq!(decode(0x1300), Key::Alt(b'R'));
        assert_eq!(decode(0x1E61), Key::Char(b'a'));
        assert_eq!(decode(0x2308), Key::Ctrl(b'H'));
        assert_eq!(decode(0x0E08), Key::Backspace);
    }
}
