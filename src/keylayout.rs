//! Keyboard layouts, as DOS's KEYB has them: what the keys of the PC
//! keyboard type in each country, by their scan codes, which follow where
//! the keys are and not what is printed on them. A German keyboard's Z key
//! sends the scan code of the US keyboard's Y, and types 'z'.
//!
//! Each layout gives the characters of the 48 keys of the main block
//! (`KEYS`) on four levels: plain, with Shift, with AltGr (the right Alt)
//! and with Shift and AltGr. '¤' marks a key that types nothing on a
//! level, and a combining accent (U+0300 and on) a dead key, whose accent
//! goes on the next letter typed: ´ then e is é. Characters are typed in
//! code page 437, so those it doesn't have (€, ø) type nothing.

use crate::video::CP437;
use std::sync::OnceLock;

/// The scan codes of the keys the layouts give characters: the row with
/// the digits and the three letter rows, with the key right of the
/// quote (2Bh, # on many keyboards) and the one left of Z (56h, < > |).
const KEYS: [u8; 48] = [
    0x29, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, //
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, //
    0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x2B, //
    0x56, 0x2C, 0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35,
];

/// No character on this level of this key.
const NONE: char = '¤';

#[derive(Debug)]
pub struct Layout {
    /// KEYB's code for it: "us", "gr", "fr" and so on.
    pub code: &'static str,
    pub name: &'static str,
    levels: [&'static str; 4],
}

/// The layouts, US first.
pub static LAYOUTS: &[Layout] = &[
    Layout {
        code: "us",
        name: "United States",
        levels: [
            "`1234567890-=qwertyuiop[]asdfghjkl;'\\\\zxcvbnm,./",
            "~!@#$%^&*()_+QWERTYUIOP{}ASDFGHJKL:\"||ZXCVBNM<>?",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
        ],
    },
    Layout {
        code: "uk",
        name: "United Kingdom",
        levels: [
            "`1234567890-=qwertyuiop[]asdfghjkl;'#\\zxcvbnm,./",
            "¬!\"£$%^&*()_+QWERTYUIOP{}ASDFGHJKL:@~|ZXCVBNM<>?",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
        ],
    },
    Layout {
        code: "gr",
        name: "German",
        levels: [
            "\u{302}1234567890ß\u{301}qwertzuiopü+asdfghjklöä#<yxcvbnm,.-",
            "°!\"§$%&/()=?\u{300}QWERTZUIOPÜ*ASDFGHJKLÖÄ'>YXCVBNM;:_",
            "¤¤²³¤¤¤{[]}\\¤@¤€¤¤¤¤¤¤¤¤~¤¤¤¤¤¤¤¤¤¤¤¤|¤¤¤¤¤¤µ¤¤¤",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
        ],
    },
    Layout {
        code: "sg",
        name: "Swiss German",
        levels: [
            "§1234567890'\u{302}qwertzuiopü\u{308}asdfghjklöä$<yxcvbnm,.-",
            "°+\"*ç%&/()=?\u{300}QWERTZUIOPè!ASDFGHJKLéà£>YXCVBNM;:_",
            "¤¦@#¤¤¬|¢¤¤\u{301}\u{303}¤¤€¤¤¤¤¤¤¤[]¤¤¤¤¤¤¤¤¤¤{}\\¤¤¤¤¤¤¤¤¤¤",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
        ],
    },
    Layout {
        code: "sf",
        name: "Swiss French",
        levels: [
            "§1234567890'\u{302}qwertzuiopè\u{308}asdfghjkléà$<yxcvbnm,.-",
            "°+\"*ç%&/()=?\u{300}QWERTZUIOPü!ASDFGHJKLöä£>YXCVBNM;:_",
            "¤¦@#¤¤¬|¢¤¤\u{301}\u{303}¤¤€¤¤¤¤¤¤¤[]¤¤¤¤¤¤¤¤¤¤{}\\¤¤¤¤¤¤¤¤¤¤",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
        ],
    },
    Layout {
        code: "fr",
        name: "French",
        levels: [
            "²&é\"'(-è_çà)=azertyuiop\u{302}$qsdfghjklmù*<wxcvbn,;:!",
            "¤1234567890°+AZERTYUIOP\u{308}£QSDFGHJKLM%µ>WXCVBN?./§",
            "¤¤\u{303}#{[|\u{300}\\^@]}¤¤€¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
        ],
    },
    Layout {
        code: "be",
        name: "Belgian",
        levels: [
            "²&é\"'(§è!çà)-azertyuiop\u{302}$qsdfghjklmùµ<wxcvbn,;:=",
            "³1234567890°_AZERTYUIOP\u{308}*QSDFGHJKLM%£>WXCVBN?./+",
            "¤|@#¤¤^¤¤{}¤¤¤¤€¤¤¤¤¤¤¤[]¤¤¤¤¤¤¤¤¤¤\u{301}\u{300}\\¤¤¤¤¤¤¤¤¤\u{303}",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
        ],
    },
    Layout {
        code: "it",
        name: "Italian",
        levels: [
            "\\1234567890'ìqwertyuiopè+asdfghjklòàù<zxcvbnm,.-",
            "|!\"£$%&/()=?^QWERTYUIOPé*ASDFGHJKLç°§>ZXCVBNM;:_",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤€¤¤¤¤¤¤¤[]¤¤¤¤¤¤¤¤¤@#¤¤¤¤¤¤¤¤¤¤¤¤",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤{}¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
        ],
    },
    Layout {
        code: "sp",
        name: "Spanish",
        levels: [
            "º1234567890'¡qwertyuiop\u{300}+asdfghjklñ\u{301}ç<zxcvbnm,.-",
            "ª!\"·$%&/()=?¿QWERTYUIOP\u{302}*ASDFGHJKLÑ\u{308}Ç>ZXCVBNM;:_",
            "\\|@#~¤¬¤¤¤¤¤¤¤¤€¤¤¤¤¤¤¤[]¤¤¤¤¤¤¤¤¤¤{}¤¤¤¤¤¤¤¤¤¤¤",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
        ],
    },
    Layout {
        code: "la",
        name: "Latin American",
        levels: [
            "|1234567890'¿qwertyuiop\u{301}+asdfghjklñ{}<zxcvbnm,.-",
            "°!\"#$%&/()=?¡QWERTYUIOP\u{308}*ASDFGHJKLÑ[]>ZXCVBNM;:_",
            "¬¤¤¤¤¤¤¤¤¤¤\\¤@¤¤¤¤¤¤¤¤¤¤~¤¤¤¤¤¤¤¤¤¤\u{302}\u{300}¤¤¤¤¤¤¤¤¤¤¤",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
        ],
    },
    Layout {
        code: "po",
        name: "Portuguese",
        levels: [
            "\\1234567890'«qwertyuiop+\u{301}asdfghjklçº\u{303}<zxcvbnm,.-",
            "|!\"#$%&/()=?»QWERTYUIOP*\u{300}ASDFGHJKLÇª\u{302}>ZXCVBNM;:_",
            "¤¤@£§¤¤{[]}¤¤¤¤€¤¤¤¤¤¤¤\u{308}¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
        ],
    },
    Layout {
        code: "dk",
        name: "Danish",
        levels: [
            "½1234567890+\u{301}qwertyuiopå\u{308}asdfghjklæø'<zxcvbnm,.-",
            "§!\"#¤%&/()=?\u{300}QWERTYUIOPÅ\u{302}ASDFGHJKLÆØ*>ZXCVBNM;:_",
            "¤¤@£$¤¤{[]}¤|¤¤€¤¤¤¤¤¤¤¤\u{303}¤¤¤¤¤¤¤¤¤¤¤¤\\¤¤¤¤¤¤µ¤¤¤",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
        ],
    },
    Layout {
        code: "no",
        name: "Norwegian",
        levels: [
            "|1234567890+\\qwertyuiopå\u{308}asdfghjkløæ'<zxcvbnm,.-",
            "§!\"#¤%&/()=?\u{300}QWERTYUIOPÅ\u{302}ASDFGHJKLØÆ*>ZXCVBNM;:_",
            "¤¤@£$¤¤{[]}¤\u{301}¤¤€¤¤¤¤¤¤¤¤\u{303}¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤µ¤¤¤",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
        ],
    },
    Layout {
        code: "sv",
        name: "Swedish",
        levels: [
            "§1234567890+\u{301}qwertyuiopå\u{308}asdfghjklöä'<zxcvbnm,.-",
            "½!\"#¤%&/()=?\u{300}QWERTYUIOPÅ\u{302}ASDFGHJKLÖÄ*>ZXCVBNM;:_",
            "¤¤@£$¤¤{[]}\\¤¤¤€¤¤¤¤¤¤¤¤\u{303}¤¤¤¤¤¤¤¤¤¤¤¤|¤¤¤¤¤¤µ¤¤¤",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
        ],
    },
    Layout {
        code: "su",
        name: "Finnish",
        levels: [
            "§1234567890+\u{301}qwertyuiopå\u{308}asdfghjklöä'<zxcvbnm,.-",
            "½!\"#¤%&/()=?\u{300}QWERTYUIOPÅ\u{302}ASDFGHJKLÖÄ*>ZXCVBNM;:_",
            "¤¤@£$¤¤{[]}\\¤¤¤€¤¤¤¤¤¤¤¤\u{303}¤¤¤¤¤¤¤¤¤¤¤¤|¤¤¤¤¤¤µ¤¤¤",
            "¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤¤",
        ],
    },

];

/// Other names the layouts go by: ISO country codes, and DOS's own.
const ALIASES: &[(&str, &str)] = &[
    ("de", "gr"),
    ("gb", "uk"),
    ("ch", "sg"),
    ("es", "sp"),
    ("pt", "po"),
    ("se", "sv"),
    ("fi", "su"),
    ("da", "dk"),
];

/// The `keyboard_layout` setting: the host keyboard's own (`auto`), or
/// one of the layouts by its KEYB code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayoutSetting {
    #[default]
    Auto,
    Named(&'static str),
}

impl LayoutSetting {
    pub fn parse(value: &str) -> Option<Self> {
        if value.trim().eq_ignore_ascii_case("auto") {
            return Some(Self::Auto);
        }
        Layout::by_code(value).map(|l| Self::Named(l.code))
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Named(code) => code,
        }
    }

    /// auto, then every layout, for the settings window.
    pub fn all() -> Vec<Self> {
        std::iter::once(Self::Auto).chain(LAYOUTS.iter().map(|l| Self::Named(l.code))).collect()
    }

    pub fn describe(self) -> String {
        match self {
            Self::Auto => "auto (the keyboard's)".to_string(),
            Self::Named(code) => Layout::by_code(code).map_or(code.to_string(), |l| format!("{} ({})", l.name, l.code)),
        }
    }

    /// The layout, with `detected` the host keyboard's for auto.
    pub fn layout(self, detected: &'static Layout) -> &'static Layout {
        match self {
            Self::Auto => detected,
            Self::Named(code) => Layout::by_code(code).unwrap_or(detected),
        }
    }
}

/// What a key types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Typed {
    /// A character of code page 437.
    Char(u8),
    /// A dead key: the accent (as a combining character) for the next
    /// letter.
    Dead(char),
    None,
}

/// The state of the keys that change what the others type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub caps: bool,
    pub num: bool,
    pub altgr: bool,
}

/// The code page 437 byte of a character, if it has one.
pub fn cp437(c: char) -> Option<u8> {
    static REVERSE: OnceLock<std::collections::HashMap<char, u8>> = OnceLock::new();
    let reverse = REVERSE.get_or_init(|| {
        let mut map: std::collections::HashMap<char, u8> = CP437.iter().enumerate().map(|(i, &c)| (c, i as u8)).collect();
        // ASCII is itself, whatever glyphs the control characters have.
        for b in 0x20..0x7F {
            map.insert(b as u8 as char, b as u8);
        }
        map
    });
    reverse.get(&c).copied()
}

fn is_accent(c: char) -> bool {
    ('\u{300}'..='\u{36F}').contains(&c)
}

/// An accent standing on its own, as a dead key types it before a space
/// or a letter it doesn't go on.
pub fn spacing_accent(accent: char) -> u8 {
    match accent {
        '\u{300}' => b'`',
        '\u{302}' => b'^',
        '\u{303}' => b'~',
        '\u{308}' => b'"',
        _ => b'\'',
    }
}

/// A letter with an accent on it, in code page 437, if it has one.
pub fn compose(accent: char, base: u8) -> Option<u8> {
    let base = CP437[base as usize];
    let composed = match (accent, base) {
        ('\u{301}', 'a') => 'á',
        ('\u{301}', 'e') => 'é',
        ('\u{301}', 'i') => 'í',
        ('\u{301}', 'o') => 'ó',
        ('\u{301}', 'u') => 'ú',
        ('\u{301}', 'E') => 'É',
        ('\u{300}', 'a') => 'à',
        ('\u{300}', 'e') => 'è',
        ('\u{300}', 'i') => 'ì',
        ('\u{300}', 'o') => 'ò',
        ('\u{300}', 'u') => 'ù',
        ('\u{302}', 'a') => 'â',
        ('\u{302}', 'e') => 'ê',
        ('\u{302}', 'i') => 'î',
        ('\u{302}', 'o') => 'ô',
        ('\u{302}', 'u') => 'û',
        ('\u{308}', 'a') => 'ä',
        ('\u{308}', 'e') => 'ë',
        ('\u{308}', 'i') => 'ï',
        ('\u{308}', 'o') => 'ö',
        ('\u{308}', 'u') => 'ü',
        ('\u{308}', 'y') => 'ÿ',
        ('\u{308}', 'A') => 'Ä',
        ('\u{308}', 'O') => 'Ö',
        ('\u{308}', 'U') => 'Ü',
        ('\u{303}', 'n') => 'ñ',
        ('\u{303}', 'N') => 'Ñ',
        _ => return None,
    };
    cp437(composed)
}

impl Layout {
    /// The layout called `code` (KEYB's code or an ISO country code), in any
    /// case.
    pub fn by_code(code: &str) -> Option<&'static Layout> {
        let code = code.trim().to_ascii_lowercase();
        let code = ALIASES.iter().find(|(alias, _)| *alias == code).map_or(code.as_str(), |(_, real)| real);
        LAYOUTS.iter().find(|l| l.code == code)
    }

    /// The US layout, which the BIOS has without KEYB.
    pub fn us() -> &'static Layout {
        &LAYOUTS[0]
    }

    /// The character on a level of the key at `index` in `KEYS`.
    fn char_at(&self, index: usize, level: usize) -> char {
        self.levels[level].chars().nth(index).unwrap_or(NONE)
    }

    /// What the key with the set-1 scan code `scan` (`extended` for the E0
    /// keys) types in this layout with the state `mods`, and whether it
    /// typed it on the AltGr level (not as an Alt combination).
    pub fn translate(&self, scan: u8, extended: bool, mods: Mods) -> (Typed, bool) {
        let fixed = |c: u8| (Typed::Char(c), false);
        match (scan, extended) {
            (0x01, false) => return fixed(0x1B),
            (0x0E, false) => return fixed(0x08),
            (0x0F, false) => return fixed(0x09),
            (0x1C, _) => return fixed(0x0D),
            (0x39, false) => return fixed(b' '),
            (0x35, true) => return fixed(b'/'),
            (0x37, false) => return fixed(b'*'),
            (0x4A, false) => return fixed(b'-'),
            (0x4E, false) => return fixed(b'+'),
            // The keypad: digits with Num Lock (or Shift without it), else
            // the cursor keys it doubles as, which type nothing.
            (0x47..=0x53, false) => {
                let digit = b"789-456+1230."[(scan - 0x47) as usize];
                return if mods.num != mods.shift { fixed(digit) } else { (Typed::None, false) };
            }
            (_, true) => return (Typed::None, false),
            _ => {}
        }
        let Some(index) = KEYS.iter().position(|&k| k == scan) else {
            return (Typed::None, false);
        };
        let shift_level = |shift: bool| usize::from(shift);
        if mods.altgr {
            let c = self.char_at(index, 2 + shift_level(mods.shift));
            if c != NONE {
                return (typed(c), true);
            }
        }
        // Caps Lock goes with the letters, those with accents too.
        let letter = self.char_at(index, 0).is_alphabetic();
        let c = self.char_at(index, shift_level(mods.shift != (mods.caps && letter)));
        (typed(c), false)
    }

    /// How to type a code page 437 character in this layout: the scan code
    /// of its key, and whether with Shift and with AltGr.
    pub fn reverse(&self, byte: u8) -> Option<(u8, bool, bool)> {
        let c = CP437[byte as usize];
        (0..4).find_map(|level| {
            let index = self.levels[level].chars().position(|k| k == c || (byte < 0x80 && k == byte as char))?;
            Some((KEYS[index], level & 1 != 0, level >= 2))
        })
    }
}

/// The layout the country (or language) of a locale uses, as KEYB's code.
pub fn locale_layout(lang: &str, country: Option<&str>) -> Option<&'static str> {
    let lang = lang.to_ascii_lowercase();
    let country = country.map(str::to_ascii_uppercase);
    Some(match (lang.as_str(), country.as_deref()) {
        ("de", Some("CH")) => "sg",
        ("fr", Some("CH")) => "sf",
        ("fr" | "nl", Some("BE")) => "be",
        ("en", Some("GB" | "IE")) => "uk",
        ("es", Some(c)) if c != "ES" => "la",
        ("de", _) => "gr",
        ("fr", _) => "fr",
        ("it", _) => "it",
        ("es", _) => "sp",
        ("pt", _) => "po",
        ("da", _) => "dk",
        ("nb" | "nn" | "no", _) => "no",
        ("sv", _) => "sv",
        ("fi", _) => "su",
        ("en", _) => "us",
        _ => return None,
    })
}

/// The layout of the host's keyboard, from the characters `host` its keys
/// type (scan code, character) and its locale: the one that has the most
/// of them in the same places, the locale's where several do.
pub fn detect(host: &[(u8, char)], locale: Option<&'static str>) -> &'static Layout {
    let spacing = |c: char| match c {
        '\u{301}' => '´',
        '\u{300}' => '`',
        '\u{302}' => '^',
        '\u{308}' => '¨',
        '\u{303}' => '~',
        c => c,
    };
    let score = |layout: &Layout| {
        host.iter()
            .filter(|&&(scan, c)| {
                KEYS.iter().position(|&k| k == scan).is_some_and(|i| {
                    let own = spacing(layout.char_at(i, 0));
                    own.to_lowercase().eq(c.to_lowercase())
                })
            })
            .count()
    };
    let best = LAYOUTS.iter().map(score).max().unwrap_or(0);
    let tied = |layout: &&Layout| score(layout) == best;
    locale
        .and_then(|code| LAYOUTS.iter().filter(tied).find(|l| l.code == code))
        .or_else(|| LAYOUTS.iter().find(tied))
        .unwrap_or(Layout::us())
}

fn typed(c: char) -> Typed {
    if c == NONE {
        Typed::None
    } else if is_accent(c) {
        Typed::Dead(c)
    } else {
        cp437(c).map_or(Typed::None, Typed::Char)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: Mods = Mods { shift: false, caps: false, num: false, altgr: false };
    const SHIFT: Mods = Mods { shift: true, caps: false, num: false, altgr: false };
    const ALTGR: Mods = Mods { shift: false, caps: false, num: false, altgr: true };
    const CAPS: Mods = Mods { shift: false, caps: true, num: false, altgr: false };

    fn char_of(layout: &str, scan: u8, mods: Mods) -> Typed {
        Layout::by_code(layout).unwrap().translate(scan, false, mods).0
    }

    #[test]
    fn every_layout_has_48_keys_on_each_level() {
        for layout in LAYOUTS {
            for level in layout.levels {
                assert_eq!(level.chars().count(), 48, "{}", layout.code);
            }
        }
    }

    #[test]
    fn keys_type_their_layout_s_characters() {
        assert_eq!(char_of("gr", 0x15, PLAIN), Typed::Char(b'z'));
        assert_eq!(char_of("de", 0x2C, SHIFT), Typed::Char(b'Y'));
        assert_eq!(char_of("gr", 0x0C, PLAIN), Typed::Char(0xE1), "ß");
        assert_eq!(char_of("gr", 0x10, ALTGR), Typed::Char(b'@'));
        assert_eq!(char_of("gr", 0x08, SHIFT), Typed::Char(b'/'));
        assert_eq!(char_of("gr", 0x0C, ALTGR), Typed::Char(b'\\'));
        assert_eq!(char_of("gr", 0x56, ALTGR), Typed::Char(b'|'));
        assert_eq!(char_of("gr", 0x28, CAPS), Typed::Char(0x8E), "Caps Lock with ä is Ä");
        assert_eq!(char_of("gr", 0x0D, PLAIN), Typed::Dead('\u{301}'));
        assert_eq!(char_of("fr", 0x10, PLAIN), Typed::Char(b'a'));
        assert_eq!(char_of("fr", 0x02, SHIFT), Typed::Char(b'1'));
        assert_eq!(char_of("fr", 0x03, PLAIN), Typed::Char(0x82), "é");
        assert_eq!(char_of("uk", 0x03, SHIFT), Typed::Char(b'"'));
        assert_eq!(char_of("uk", 0x2B, PLAIN), Typed::Char(b'#'));
        assert_eq!(char_of("uk", 0x56, PLAIN), Typed::Char(b'\\'));
        assert_eq!(char_of("us", 0x28, SHIFT), Typed::Char(b'"'));
        // What code page 437 doesn't have types nothing.
        assert_eq!(char_of("gr", 0x12, ALTGR), Typed::None, "€");
    }

    #[test]
    fn the_us_layout_types_what_the_key_table_does() {
        let us = Layout::us();
        for name in crate::keyboard::names() {
            let key = crate::keyboard::lookup(name).unwrap();
            if key.modifier != 0 || key.extended || name.starts_with("kp") || key.ascii < 0x20 {
                continue;
            }
            assert_eq!(us.translate(key.scan, false, PLAIN).0, Typed::Char(key.ascii), "{}", name);
            assert_eq!(us.translate(key.scan, false, SHIFT).0, Typed::Char(key.shifted), "{}", name);
        }
    }

    #[test]
    fn accents_go_on_letters() {
        assert_eq!(compose('\u{301}', b'e'), Some(0x82));
        assert_eq!(compose('\u{308}', b'U'), Some(0x9A));
        assert_eq!(compose('\u{301}', b'x'), None);
        assert_eq!(spacing_accent('\u{302}'), b'^');
    }

    #[test]
    fn the_host_s_layout_is_found_from_its_keys() {
        // A German keyboard: Z and Y swapped, ö ä ü.
        let host: Vec<(u8, char)> = [(0x15, 'z'), (0x2C, 'y'), (0x27, 'ö'), (0x28, 'ä'), (0x1A, 'ü'), (0x10, 'q')].to_vec();
        assert_eq!(detect(&host, None).code, "gr");
        // A Swiss one has them too; the locale tells.
        let swiss: Vec<(u8, char)> = [(0x15, 'z'), (0x2C, 'y'), (0x27, 'ö'), (0x28, 'ä')].to_vec();
        assert_eq!(detect(&swiss, locale_layout("de", Some("CH"))).code, "sg");
        assert_eq!(detect(&[(0x10, 'a'), (0x1E, 'q'), (0x11, 'z')], None).code, "fr");
        assert_eq!(detect(&[], None).code, "us");
        assert_eq!(detect(&[(0x10, 'q'), (0x2B, '#')], locale_layout("en", Some("GB"))).code, "uk");
    }

    #[test]
    fn characters_are_found_on_their_keys() {
        let gr = Layout::by_code("gr").unwrap();
        assert_eq!(gr.reverse(b'y'), Some((0x2C, false, false)));
        assert_eq!(gr.reverse(b':'), Some((0x34, true, false)));
        assert_eq!(gr.reverse(b'\\'), Some((0x0C, false, true)));
        assert_eq!(Layout::us().reverse(b'?'), Some((0x35, true, false)));
    }
}
