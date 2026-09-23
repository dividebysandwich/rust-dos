//! SDL-independent key table for remote input. Scan codes mirror
//! `keyboard::map_sdl_to_pc` so remote keystrokes look exactly like ones
//! typed into the SDL window.

/// A PC key: set-1 scan code, unshifted/shifted ASCII, and — for modifier
/// keys — the BIOS shift-flag bit it controls at 0040:0017. Extended keys
/// (the grey cursor block, right Ctrl/Alt, keypad Enter and /) send an E0
/// prefix before their scan code.
#[derive(Clone, Copy, Debug)]
pub struct PcKey {
    pub scan: u8,
    pub ascii: u8,
    pub shifted: u8,
    pub modifier: u8,
    pub extended: bool,
}

pub const MOD_RSHIFT: u8 = 0x01;
pub const MOD_LSHIFT: u8 = 0x02;
pub const MOD_CTRL: u8 = 0x04;
pub const MOD_ALT: u8 = 0x08;

const fn k(scan: u8, ascii: u8, shifted: u8) -> PcKey {
    PcKey { scan, ascii, shifted, modifier: 0, extended: false }
}

/// An extended key.
const fn x(scan: u8, ascii: u8) -> PcKey {
    PcKey { scan, ascii, shifted: ascii, modifier: 0, extended: true }
}

const fn m(scan: u8, modifier: u8) -> PcKey {
    PcKey { scan, ascii: 0, shifted: 0, modifier, extended: false }
}

/// An extended modifier (right Ctrl, right Alt).
const fn mx(scan: u8, modifier: u8) -> PcKey {
    PcKey { scan, ascii: 0, shifted: 0, modifier, extended: true }
}

/// (name, key). Names are matched case-insensitively.
const KEYS: &[(&str, PcKey)] = &[
    ("a", k(0x1E, b'a', b'A')),
    ("b", k(0x30, b'b', b'B')),
    ("c", k(0x2E, b'c', b'C')),
    ("d", k(0x20, b'd', b'D')),
    ("e", k(0x12, b'e', b'E')),
    ("f", k(0x21, b'f', b'F')),
    ("g", k(0x22, b'g', b'G')),
    ("h", k(0x23, b'h', b'H')),
    ("i", k(0x17, b'i', b'I')),
    ("j", k(0x24, b'j', b'J')),
    ("k", k(0x25, b'k', b'K')),
    ("l", k(0x26, b'l', b'L')),
    ("m", k(0x32, b'm', b'M')),
    ("n", k(0x31, b'n', b'N')),
    ("o", k(0x18, b'o', b'O')),
    ("p", k(0x19, b'p', b'P')),
    ("q", k(0x10, b'q', b'Q')),
    ("r", k(0x13, b'r', b'R')),
    ("s", k(0x1F, b's', b'S')),
    ("t", k(0x14, b't', b'T')),
    ("u", k(0x16, b'u', b'U')),
    ("v", k(0x2F, b'v', b'V')),
    ("w", k(0x11, b'w', b'W')),
    ("x", k(0x2D, b'x', b'X')),
    ("y", k(0x15, b'y', b'Y')),
    ("z", k(0x2C, b'z', b'Z')),
    ("0", k(0x0B, b'0', b')')),
    ("1", k(0x02, b'1', b'!')),
    ("2", k(0x03, b'2', b'@')),
    ("3", k(0x04, b'3', b'#')),
    ("4", k(0x05, b'4', b'$')),
    ("5", k(0x06, b'5', b'%')),
    ("6", k(0x07, b'6', b'^')),
    ("7", k(0x08, b'7', b'&')),
    ("8", k(0x09, b'8', b'*')),
    ("9", k(0x0A, b'9', b'(')),
    ("minus", k(0x0C, b'-', b'_')),
    ("equals", k(0x0D, b'=', b'+')),
    ("leftbracket", k(0x1A, b'[', b'{')),
    ("rightbracket", k(0x1B, b']', b'}')),
    ("backslash", k(0x2B, b'\\', b'|')),
    ("semicolon", k(0x27, b';', b':')),
    ("quote", k(0x28, b'\'', b'"')),
    ("comma", k(0x33, b',', b'<')),
    ("period", k(0x34, b'.', b'>')),
    ("slash", k(0x35, b'/', b'?')),
    ("backquote", k(0x29, b'`', b'~')),
    ("space", k(0x39, b' ', b' ')),
    ("enter", k(0x1C, 0x0D, 0x0D)),
    ("return", k(0x1C, 0x0D, 0x0D)),
    ("backspace", k(0x0E, 0x08, 0x08)),
    ("tab", k(0x0F, 0x09, 0x09)),
    ("escape", k(0x01, 0x1B, 0x1B)),
    ("esc", k(0x01, 0x1B, 0x1B)),
    ("f1", k(0x3B, 0, 0)),
    ("f2", k(0x3C, 0, 0)),
    ("f3", k(0x3D, 0, 0)),
    ("f4", k(0x3E, 0, 0)),
    ("f5", k(0x3F, 0, 0)),
    ("f6", k(0x40, 0, 0)),
    ("f7", k(0x41, 0, 0)),
    ("f8", k(0x42, 0, 0)),
    ("f9", k(0x43, 0, 0)),
    ("f10", k(0x44, 0, 0)),
    ("f11", k(0x85, 0, 0)),
    ("f12", k(0x86, 0, 0)),
    ("up", x(0x48, 0)),
    ("down", x(0x50, 0)),
    ("left", x(0x4B, 0)),
    ("right", x(0x4D, 0)),
    ("home", x(0x47, 0)),
    ("end", x(0x4F, 0)),
    ("pageup", x(0x49, 0)),
    ("pagedown", x(0x51, 0)),
    ("insert", x(0x52, 0)),
    ("delete", x(0x53, 0)),
    ("kp0", k(0x52, b'0', b'0')),
    ("kp1", k(0x4F, b'1', b'1')),
    ("kp2", k(0x50, b'2', b'2')),
    ("kp3", k(0x51, b'3', b'3')),
    ("kp4", k(0x4B, b'4', b'4')),
    ("kp5", k(0x4C, b'5', b'5')),
    ("kp6", k(0x4D, b'6', b'6')),
    ("kp7", k(0x47, b'7', b'7')),
    ("kp8", k(0x48, b'8', b'8')),
    ("kp9", k(0x49, b'9', b'9')),
    ("kpperiod", k(0x53, b'.', b'.')),
    ("kpplus", k(0x4E, b'+', b'+')),
    ("kpminus", k(0x4A, b'-', b'-')),
    ("kpmultiply", k(0x37, b'*', b'*')),
    ("kpdivide", x(0x35, b'/')),
    ("kpenter", x(0x1C, 0x0D)),
    ("lshift", m(0x2A, MOD_LSHIFT)),
    ("shift", m(0x2A, MOD_LSHIFT)),
    ("rshift", m(0x36, MOD_RSHIFT)),
    ("ctrl", m(0x1D, MOD_CTRL)),
    ("lctrl", m(0x1D, MOD_CTRL)),
    ("rctrl", mx(0x1D, MOD_CTRL)),
    ("alt", m(0x38, MOD_ALT)),
    ("lalt", m(0x38, MOD_ALT)),
    ("ralt", mx(0x38, MOD_ALT)),
];

pub fn lookup(name: &str) -> Option<PcKey> {
    let lower = name.to_ascii_lowercase();
    KEYS.iter().find(|(n, _)| *n == lower).map(|(_, key)| *key)
}

/// Map a character to (key, needs_shift) for typing text.
pub fn char_to_key(c: char) -> Option<(PcKey, bool)> {
    match c {
        '\n' | '\r' => return lookup("enter").map(|key| (key, false)),
        '\t' => return lookup("tab").map(|key| (key, false)),
        '\x08' => return lookup("backspace").map(|key| (key, false)),
        '\x1b' => return lookup("escape").map(|key| (key, false)),
        _ => {}
    }
    if !c.is_ascii() || c.is_ascii_control() {
        return None;
    }
    let b = c as u8;
    // Only the printable main-block keys; the keypad duplicates would
    // otherwise shadow digits and operators.
    KEYS.iter()
        .filter(|(n, _)| !n.starts_with("kp"))
        .find_map(|(_, key)| {
            if key.ascii == b {
                Some((*key, false))
            } else if key.shifted == b && key.shifted != key.ascii {
                Some((*key, true))
            } else {
                None
            }
        })
}

pub fn names() -> Vec<&'static str> {
    KEYS.iter().map(|(n, _)| *n).collect()
}

/// Code page 437 → Unicode, for rendering text-mode screen dumps.
pub const CP437: [char; 256] = [
    ' ', '☺', '☻', '♥', '♦', '♣', '♠', '•', '◘', '○', '◙', '♂', '♀', '♪', '♫', '☼',
    '►', '◄', '↕', '‼', '¶', '§', '▬', '↨', '↑', '↓', '→', '←', '∟', '↔', '▲', '▼',
    ' ', '!', '"', '#', '$', '%', '&', '\'', '(', ')', '*', '+', ',', '-', '.', '/',
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', ':', ';', '<', '=', '>', '?',
    '@', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L', 'M', 'N', 'O',
    'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z', '[', '\\', ']', '^', '_',
    '`', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o',
    'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z', '{', '|', '}', '~', '⌂',
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å',
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ',
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»',
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐',
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧',
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀',
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩',
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', ' ',
];
