//! The SDL window's keys as PC keys: the scan codes of where they are on
//! the keyboard, and the layout of the host's keyboard, which the machine
//! types in with `keyboard_layout=auto`.

use rust_dos::keylayout::{self, Layout};
use sdl2::keyboard::{Keycode, Scancode};

/// The PC keyboard's set-1 scan code of the key at `scancode`, and whether
/// it sends E0 first. SDL's scan codes are the positions of the keys, as
/// the PC's are, whatever they type.
pub fn pc_scan(scancode: Scancode) -> Option<(u8, bool)> {
    use Scancode::*;
    let plain = |scan: u8| Some((scan, false));
    let extended = |scan: u8| Some((scan, true));
    match scancode {
        A => plain(0x1E),
        B => plain(0x30),
        C => plain(0x2E),
        D => plain(0x20),
        E => plain(0x12),
        F => plain(0x21),
        G => plain(0x22),
        H => plain(0x23),
        I => plain(0x17),
        J => plain(0x24),
        K => plain(0x25),
        L => plain(0x26),
        M => plain(0x32),
        N => plain(0x31),
        O => plain(0x18),
        P => plain(0x19),
        Q => plain(0x10),
        R => plain(0x13),
        S => plain(0x1F),
        T => plain(0x14),
        U => plain(0x16),
        V => plain(0x2F),
        W => plain(0x11),
        X => plain(0x2D),
        Y => plain(0x15),
        Z => plain(0x2C),
        Num1 => plain(0x02),
        Num2 => plain(0x03),
        Num3 => plain(0x04),
        Num4 => plain(0x05),
        Num5 => plain(0x06),
        Num6 => plain(0x07),
        Num7 => plain(0x08),
        Num8 => plain(0x09),
        Num9 => plain(0x0A),
        Num0 => plain(0x0B),
        Return => plain(0x1C),
        Escape => plain(0x01),
        Backspace => plain(0x0E),
        Tab => plain(0x0F),
        Space => plain(0x39),
        Minus => plain(0x0C),
        Equals => plain(0x0D),
        LeftBracket => plain(0x1A),
        RightBracket => plain(0x1B),
        Backslash | NonUsHash => plain(0x2B),
        Semicolon => plain(0x27),
        Apostrophe => plain(0x28),
        Grave => plain(0x29),
        Comma => plain(0x33),
        Period => plain(0x34),
        Slash => plain(0x35),
        NonUsBackslash => plain(0x56),
        CapsLock => plain(0x3A),
        F1 => plain(0x3B),
        F2 => plain(0x3C),
        F3 => plain(0x3D),
        F4 => plain(0x3E),
        F5 => plain(0x3F),
        F6 => plain(0x40),
        F7 => plain(0x41),
        F8 => plain(0x42),
        F9 => plain(0x43),
        F10 => plain(0x44),
        F11 => plain(0x57),
        F12 => plain(0x58),
        ScrollLock => plain(0x46),
        NumLockClear => plain(0x45),
        Insert => extended(0x52),
        Home => extended(0x47),
        PageUp => extended(0x49),
        Delete => extended(0x53),
        End => extended(0x4F),
        PageDown => extended(0x51),
        Right => extended(0x4D),
        Left => extended(0x4B),
        Down => extended(0x50),
        Up => extended(0x48),
        KpDivide => extended(0x35),
        KpMultiply => plain(0x37),
        KpMinus => plain(0x4A),
        KpPlus => plain(0x4E),
        KpEnter => extended(0x1C),
        Kp1 => plain(0x4F),
        Kp2 => plain(0x50),
        Kp3 => plain(0x51),
        Kp4 => plain(0x4B),
        Kp5 => plain(0x4C),
        Kp6 => plain(0x4D),
        Kp7 => plain(0x47),
        Kp8 => plain(0x48),
        Kp9 => plain(0x49),
        Kp0 => plain(0x52),
        KpPeriod => plain(0x53),
        LCtrl => plain(0x1D),
        LShift => plain(0x2A),
        LAlt => plain(0x38),
        RCtrl => extended(0x1D),
        RShift => plain(0x36),
        RAlt => extended(0x38),
        LGui => extended(0x5B),
        RGui => extended(0x5C),
        Application => extended(0x5D),
        _ => None,
    }
}

/// The layout of the host's keyboard: from the characters the host says
/// its character keys type, and its locale where layouts are alike.
pub fn detect_layout() -> &'static Layout {
    // The keys whose characters tell layouts apart, by where they are.
    use Scancode::*;
    let keys = [
        Q, W, E, R, T, Y, U, I, O, P, A, S, D, F, G, H, J, K, L, Z, X, C, V, B, N, M, Num1, Num2, Num3, Num4, Num5,
        Num6, Num7, Num8, Num9, Num0, Minus, Equals, LeftBracket, RightBracket, Backslash, Semicolon, Apostrophe,
        Grave, Comma, Period, Slash, NonUsBackslash,
    ];
    let host: Vec<(u8, char)> = keys
        .into_iter()
        .filter_map(|scancode| {
            let (scan, _) = pc_scan(scancode)?;
            let keycode = Keycode::from_scancode(scancode)?;
            let c = char::from_u32(keycode.into_i32() as u32).filter(|c| !c.is_control())?;
            Some((scan, c))
        })
        .collect();
    let locale = sdl2::locale::get_preferred_locales()
        .next()
        .and_then(|l| keylayout::locale_layout(&l.lang, l.country.as_deref()));
    keylayout::detect(&host, locale)
}
