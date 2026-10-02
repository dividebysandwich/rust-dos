//! EDIT's keys, from the BIOS's keystrokes (INT 16h AH=10h: scan code in
//! the high byte, character in the low) and the shift flags.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A character to type.
    Char(u8),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PgUp,
    PgDn,
    Ins,
    Del,
    CtrlLeft,
    CtrlRight,
    CtrlHome,
    CtrlEnd,
    CtrlUp,
    CtrlDown,
    CtrlIns,
    CtrlDel,
    CtrlBackspace,
    CtrlPgUp,
    CtrlPgDn,
    /// Alt+=.
    AltEquals,
    /// Ctrl and a letter, 'A' to 'Z'.
    Ctrl(u8),
    /// F1 to F12.
    F(u8),
    /// Alt and a letter, 'A' to 'Z'.
    Alt(u8),
    Other,
}

/// The letters of the keys by their scan codes, a row of the keyboard at
/// a time: Q to P, A to L, Z to M.
const ROWS: [(u8, &[u8]); 3] = [(0x10, b"QWERTYUIOP"), (0x1E, b"ASDFGHJKL"), (0x2C, b"ZXCVBNM")];

/// The letter of the key with scan code `scan`.
pub fn letter(scan: u8) -> Option<u8> {
    ROWS.iter().find_map(|&(first, letters)| letters.get(scan.checked_sub(first)? as usize).copied())
}

/// The key of keystroke `key`. Shift doesn't change it: callers read the
/// shift flags for selecting.
pub fn decode(key: u16) -> Key {
    let scan = (key >> 8) as u8;
    let mut ascii = key as u8;
    // The grey keys: E0h in place of the keypad's 00h.
    if ascii == 0xE0 && scan != 0 {
        ascii = 0;
    }
    match (ascii, scan) {
        (0x0D, _) => Key::Enter,
        (0x1B, _) => Key::Esc,
        (0x09, 0x0F) | (0x09, 0) => Key::Tab,
        (0x08, 0x0E) | (0x08, 0) => Key::Backspace,
        (0x7F, 0x0E) => Key::CtrlBackspace,
        (1..=0x1A, _) => Key::Ctrl(b'A' + ascii - 1),
        (0, _) => match scan {
            0x0F => Key::BackTab,
            0x48 => Key::Up,
            0x50 => Key::Down,
            0x4B => Key::Left,
            0x4D => Key::Right,
            0x47 => Key::Home,
            0x4F => Key::End,
            0x49 => Key::PgUp,
            0x51 => Key::PgDn,
            0x52 => Key::Ins,
            0x53 => Key::Del,
            0x73 => Key::CtrlLeft,
            0x74 => Key::CtrlRight,
            0x77 => Key::CtrlHome,
            0x75 => Key::CtrlEnd,
            0x8D => Key::CtrlUp,
            0x91 => Key::CtrlDown,
            0x92 => Key::CtrlIns,
            0x93 => Key::CtrlDel,
            0x84 => Key::CtrlPgUp,
            0x76 => Key::CtrlPgDn,
            0x83 => Key::AltEquals,
            0x3B..=0x44 => Key::F(scan - 0x3A),
            0x85 | 0x86 => Key::F(scan - 0x85 + 11),
            _ => match letter(scan) {
                Some(l) => Key::Alt(l),
                None => Key::Other,
            },
        },
        (c, _) => Key::Char(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes() {
        assert_eq!(decode(0x1E61), Key::Char(b'a'));
        assert_eq!(decode(0x48E0), Key::Up);
        assert_eq!(decode(0x4800), Key::Up);
        assert_eq!(decode(0x2100), Key::Alt(b'F'));
        assert_eq!(decode(0x2E03), Key::Ctrl(b'C'));
        assert_eq!(decode(0x0E08), Key::Backspace);
        assert_eq!(decode(0x0F09), Key::Tab);
        assert_eq!(decode(0x0F00), Key::BackTab);
        assert_eq!(decode(0x3D00), Key::F(3));
        assert_eq!(decode(0x9200), Key::CtrlIns);
        assert_eq!(decode(0x1C0D), Key::Enter);
        assert_eq!(decode(0x0E7F), Key::CtrlBackspace);
        assert_eq!(decode(0x93E0), Key::CtrlDel);
        assert_eq!(decode(0x8300), Key::AltEquals);
    }
}
