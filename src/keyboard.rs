use crate::bus::Bus;
use crate::keylayout::{Layout, Mods, Typed, compose, spacing_accent};

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

/// Deliver a key press: queue its keystroke for BIOS INT 16h readers, as
/// the BIOS translates `(scan << 8) | ascii` under the Shift, Ctrl and Alt
/// state at 40:17h, and send the make code to the keyboard controller for
/// programs that read port 60h or install their own INT 09h ISR.
pub fn deliver_key_down(bus: &mut Bus, code: u16, extended: bool) {
    let scan = (code >> 8) as u8;
    // The BIOS buffer holds 15 keystrokes; when it's full (a program that
    // reads the keyboard itself never empties it) new ones are dropped.
    if bus.keyboard_buffer.len() < BIOS_BUFFER_KEYS {
        let keystroke = bios_keystroke(scan, code as u8, bus.read_8(0x0417));
        bus.keyboard_buffer.push_back(keystroke);
    }
    send_scan(bus, scan, extended);
}

/// The keystroke the BIOS keyboard handler stores for the make code `scan`
/// of a key whose character is `ascii`, with the Shift (bits 0-1), Ctrl
/// (bit 2) and Alt (bit 3) state `flags`: Alt and Ctrl combinations and
/// shifted function keys have codes of their own. Ctrl with a letter is
/// the letter's control character, wherever the layout has the letter.
pub fn bios_keystroke(scan: u8, ascii: u8, flags: u8) -> u16 {
    let key = |scan: u8, ascii: u8| (scan as u16) << 8 | ascii as u16;
    let letter = ascii.is_ascii_alphabetic();
    if flags & 0x08 != 0 {
        return match scan {
            0x02..=0x0D => key(scan + 0x76, 0),
            0x3B..=0x44 => key(scan + 0x2D, 0),
            0x57 | 0x58 => key(scan + 0x34, 0),
            0x0F => key(0xA5, 0),
            0x39 => key(scan, ascii),
            0x47 => key(0x97, 0),
            0x48 => key(0x98, 0),
            0x49 => key(0x99, 0),
            0x4B => key(0x9B, 0),
            0x4D => key(0x9D, 0),
            0x4F => key(0x9F, 0),
            0x50 => key(0xA0, 0),
            0x51 => key(0xA1, 0),
            0x52 => key(0xA2, 0),
            0x53 => key(0xA3, 0),
            _ => key(scan, 0),
        };
    }
    if flags & 0x04 != 0 {
        return match scan {
            _ if letter => key(scan, ascii & 0x1F),
            0x03 => key(scan, 0),
            0x07 => key(scan, 0x1E),
            0x0C => key(scan, 0x1F),
            0x1A => key(scan, 0x1B),
            0x1B => key(scan, 0x1D),
            0x2B => key(scan, 0x1C),
            0x1C => key(scan, 0x0A),
            0x0E => key(scan, 0x7F),
            0x0F => key(0x94, 0),
            0x3B..=0x44 => key(scan + 0x23, 0),
            0x57 | 0x58 => key(scan + 0x32, 0),
            0x47 => key(0x77, 0),
            0x48 => key(0x8D, 0),
            0x49 => key(0x84, 0),
            0x4B => key(0x73, 0),
            0x4D => key(0x74, 0),
            0x4F => key(0x75, 0),
            0x50 => key(0x91, 0),
            0x51 => key(0x76, 0),
            0x52 => key(0x92, 0),
            0x53 => key(0x93, 0),
            _ => key(scan, ascii),
        };
    }
    if flags & 0x03 != 0 {
        return match scan {
            0x3B..=0x44 => key(scan + 0x19, 0),
            0x57 | 0x58 => key(scan + 0x30, 0),
            0x0F => key(scan, 0),
            _ => key(scan, ascii),
        };
    }
    match scan {
        // F11 and F12 came with the enhanced keyboard, and their
        // keystrokes don't have their scan codes.
        0x57 | 0x58 => key(scan + 0x2E, 0),
        _ => key(scan, ascii),
    }
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

/// A key of `KEYS` going down (`down`) or up, typing `ascii` (0: what the
/// layout makes of it), as `key_event` has it.
pub fn apply_key(bus: &mut Bus, key: PcKey, ascii: u8, down: bool) {
    key_event(bus, key.scan, key.extended, down, (ascii != 0).then_some(ascii));
}

/// The keyboard as the BIOS and KEYB see it: the layout the keys type in,
/// the keys held, and the accent a dead key left for the next letter.
#[derive(Clone, Debug)]
pub struct KeyboardState {
    pub layout: &'static Layout,
    /// One bit for each key held, by `key_id`.
    held: [u64; 8],
    dead: Option<char>,
}

impl Default for KeyboardState {
    fn default() -> Self {
        Self { layout: Layout::us(), held: [0; 8], dead: None }
    }
}

fn key_id(scan: u8, extended: bool) -> usize {
    (scan & 0x7F) as usize | (extended as usize) << 8
}

impl KeyboardState {
    fn is_held(&self, id: usize) -> bool {
        self.held[id / 64] & 1 << (id % 64) != 0
    }

    fn set_held(&mut self, id: usize, down: bool) {
        if down {
            self.held[id / 64] |= 1 << (id % 64);
        } else {
            self.held[id / 64] &= !(1 << (id % 64));
        }
    }
}

/// BDA 40:17h: the shift keys and locks.
const FLAGS: usize = 0x0417;
/// BDA 40:18h: the left Ctrl and Alt and the lock keys held.
const FLAGS2: usize = 0x0418;
/// BDA 40:96h: the right Ctrl and Alt held, and an enhanced keyboard.
const FLAGS3: usize = 0x0496;
pub const ENHANCED_KEYBOARD: u8 = 0x10;

fn set_bit(byte: &mut u8, bit: u8, on: bool) {
    if on {
        *byte |= bit;
    } else {
        *byte &= !bit;
    }
}

/// A key of the PC keyboard went down (again, when held) or up: its set-1
/// scan code `scan`, with `extended` for the keys that send E0 first (the
/// grey keys, right Ctrl and Alt, keypad Enter and /). The BIOS's state
/// follows: Shift, Ctrl and Alt at 40:17h, the left and right ones apart
/// at 40:18h and 40:96h, and the locks. A key that types something queues
/// its keystroke for INT 16h: `host_char` when the host says what it typed
/// (code page 437), else what the layout makes of it, with a dead key's
/// accent on it. AltGr (the right Alt) types a layout's third level as a
/// plain character; with nothing there it is Alt. Every key sends its make
/// or break code to the keyboard controller, for programs that read the
/// keyboard themselves.
pub fn key_event(bus: &mut Bus, scan: u8, extended: bool, down: bool, host_char: Option<u8>) {
    let id = key_id(scan, extended);
    let repeat = down && bus.kbd.is_held(id);
    bus.kbd.set_held(id, down);
    let (mut flags, mut flags2, mut flags3) = (bus.read_8(FLAGS), bus.read_8(FLAGS2), bus.read_8(FLAGS3));
    let modifier = match (scan, extended) {
        (0x2A, false) => {
            set_bit(&mut flags, 0x02, down);
            true
        }
        (0x36, false) => {
            set_bit(&mut flags, 0x01, down);
            true
        }
        (0x1D, _) => {
            if extended { set_bit(&mut flags3, 0x04, down) } else { set_bit(&mut flags2, 0x01, down) }
            set_bit(&mut flags, 0x04, flags2 & 0x01 != 0 || flags3 & 0x04 != 0);
            true
        }
        (0x38, _) => {
            if extended { set_bit(&mut flags3, 0x08, down) } else { set_bit(&mut flags2, 0x02, down) }
            set_bit(&mut flags, 0x08, flags2 & 0x02 != 0 || flags3 & 0x08 != 0);
            true
        }
        (0x3A | 0x45 | 0x46, false) => {
            let bit = match scan {
                0x3A => 0x40,
                0x45 => 0x20,
                _ => 0x10,
            };
            set_bit(&mut flags2, bit, down);
            if down && !repeat {
                flags ^= bit;
            }
            true
        }
        _ => false,
    };
    bus.write_8(FLAGS, flags);
    bus.write_8(FLAGS2, flags2);
    bus.write_8(FLAGS3, flags3);
    if !down {
        send_scan(bus, scan | 0x80, extended);
        return;
    }
    if modifier {
        send_scan(bus, scan, extended);
        return;
    }
    // Insert switches the BIOS's insert mode.
    if scan == 0x52 && (extended || (flags & 0x20 == 0) == (flags & 0x03 == 0)) && !repeat {
        bus.write_8(FLAGS, flags ^ 0x80);
    }

    let mods = Mods { shift: flags & 0x03 != 0, caps: flags & 0x40 != 0, num: flags & 0x20 != 0, altgr: flags3 & 0x08 != 0 };
    let (typed, on_altgr) = match host_char {
        // A character typed with AltGr held is one the host's layout has
        // there.
        Some(c) => (Typed::Char(c), mods.altgr && c >= 0x20),
        None => bus.kbd.layout.translate(scan, extended, mods),
    };
    // A character typed with AltGr is no Alt (or Ctrl, which some hosts
    // send with it) combination.
    let flags = if on_altgr { flags & !0x0C } else { flags };
    let combination = flags & 0x0C != 0;
    let mut keystrokes: Vec<u16> = Vec::new();
    match typed {
        Typed::Dead(accent) if !combination => {
            // A second dead key types the first accent and waits with its own.
            if let Some(previous) = bus.kbd.dead.replace(accent) {
                keystrokes.push(bios_keystroke(0, spacing_accent(previous), 0));
            }
        }
        Typed::Dead(_) => keystrokes.push(bios_keystroke(scan, 0, flags)),
        Typed::Char(c) => {
            match bus.kbd.dead.take() {
                Some(accent) if !combination => match compose(accent, c) {
                    Some(composed) => keystrokes.push(bios_keystroke(scan, composed, flags)),
                    None if c == b' ' => keystrokes.push(bios_keystroke(scan, spacing_accent(accent), flags)),
                    None => {
                        keystrokes.push(bios_keystroke(0, spacing_accent(accent), 0));
                        keystrokes.push(bios_keystroke(scan, c, flags));
                    }
                },
                _ => keystrokes.push(bios_keystroke(scan, c, flags)),
            }
        }
        Typed::None => keystrokes.push(bios_keystroke(scan, 0, flags)),
    }
    for keystroke in keystrokes {
        // The BIOS buffer holds 15 keystrokes; when it's full (a program
        // that reads the keyboard itself never empties it) new ones are
        // dropped.
        if bus.keyboard_buffer.len() < BIOS_BUFFER_KEYS {
            bus.keyboard_buffer.push_back(keystroke);
        }
    }
    send_scan(bus, scan, extended);
}

/// Let go of every key the machine holds, as the host takes the keyboard
/// away: their break codes, and Shift, Ctrl and Alt up. The locks stay.
pub fn release_all(bus: &mut Bus) {
    for id in 0..512 {
        if bus.kbd.is_held(id) {
            key_event(bus, (id & 0x7F) as u8, id >= 256, false, None);
        }
    }
    bus.kbd.dead = None;
}

// The keys by name, for input that doesn't come from the SDL window (the
// debug server's, the browser's). Scan codes mirror the window's
// (`sdl_keys.rs` in the rust-dos program).

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
    ("f11", k(0x57, 0, 0)),
    ("f12", k(0x58, 0, 0)),
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

/// The layout is saved by its code; the keys held and the dead key's accent
/// as they are.
impl crate::savestate::State for KeyboardState {
    fn save(&self, w: &mut crate::savestate::Writer) {
        self.layout.code.to_string().save(w);
        self.held.save(w);
        self.dead.save(w);
    }
    fn load(&mut self, r: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        let mut code = String::new();
        code.load(r)?;
        self.layout = Layout::by_code(&code).unwrap_or(self.layout);
        self.held.load(r)?;
        self.dead.load(r)
    }
}
