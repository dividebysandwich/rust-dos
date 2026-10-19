//! Characters' types (GetStringType) and the order of strings (CompareString,
//! LCMapString's sort keys), as Windows sorts them: symbols before digits
//! before letters, letters by their base letter, then by their accents,
//! then lower case before upper case. Hyphens and apostrophes count only
//! when the strings are otherwise the same ("word sort"), unless
//! SORT_STRINGSORT makes them symbols. All locales sort alike here.

use super::text::{ERROR_INVALID_FLAGS, ansi_to_wide, put_units, read_units};
use super::{ERROR_INVALID_PARAMETER, FALSE, TRUE, set_error};
use crate::cpu::Cpu;
use crate::win32::convert::{self, CodePage};
use std::cmp::Ordering;

/// CompareString's and LCMapString's flags.
pub(super) const NORM_IGNORECASE: u32 = 0x1;
const NORM_IGNORENONSPACE: u32 = 0x2;
const NORM_IGNORESYMBOLS: u32 = 0x4;
const SORT_STRINGSORT: u32 = 0x1000;
const NORM_IGNOREKANATYPE: u32 = 0x1_0000;
const NORM_IGNOREWIDTH: u32 = 0x2_0000;
const LOCALE_USE_CP_ACP: u32 = 0x4000_0000;
const LCMAP_LOWERCASE: u32 = 0x100;
const LCMAP_UPPERCASE: u32 = 0x200;
const LCMAP_SORTKEY: u32 = 0x400;
const LCMAP_BYTEREV: u32 = 0x800;
const LCMAP_HIRAGANA: u32 = 0x10_0000;
const LCMAP_KATAKANA: u32 = 0x20_0000;
const LCMAP_HALFWIDTH: u32 = 0x40_0000;
const LCMAP_FULLWIDTH: u32 = 0x80_0000;
const LCMAP_LINGUISTIC_CASING: u32 = 0x100_0000;
const LCMAP_SIMPLIFIED_CHINESE: u32 = 0x200_0000;
const LCMAP_TRADITIONAL_CHINESE: u32 = 0x400_0000;

/// The flags CompareString takes.
const COMPARE_FLAGS: u32 = NORM_IGNORECASE
    | NORM_IGNORENONSPACE
    | NORM_IGNORESYMBOLS
    | SORT_STRINGSORT
    | NORM_IGNOREKANATYPE
    | NORM_IGNOREWIDTH
    | LOCALE_USE_CP_ACP;

/// GetStringType's kinds of information, and their bits.
const CT_CTYPE1: u32 = 1;
const CT_CTYPE2: u32 = 2;
const CT_CTYPE3: u32 = 4;
const C1_UPPER: u16 = 0x1;
const C1_LOWER: u16 = 0x2;
const C1_DIGIT: u16 = 0x4;
const C1_SPACE: u16 = 0x8;
const C1_PUNCT: u16 = 0x10;
const C1_CNTRL: u16 = 0x20;
const C1_BLANK: u16 = 0x40;
const C1_XDIGIT: u16 = 0x80;
const C1_ALPHA: u16 = 0x100;
const C2_LEFTTORIGHT: u16 = 1;
const C2_EUROPENUMBER: u16 = 3;
const C2_EUROPESEPARATOR: u16 = 4;
const C2_EUROPETERMINATOR: u16 = 5;
const C2_COMMONSEPARATOR: u16 = 7;
const C2_BLOCKSEPARATOR: u16 = 8;
const C2_SEGMENTSEPARATOR: u16 = 9;
const C2_WHITESPACE: u16 = 10;
const C2_OTHERNEUTRAL: u16 = 11;
const C3_NONSPACING: u16 = 1;
const C3_DIACRITIC: u16 = 2;
const C3_SYMBOL: u16 = 8;

/// The C1_ bits of the character `c`.
pub(super) fn ctype1(c: u16) -> u16 {
    let xdigit = if (c as u8 as u16 == c) && (c as u8).is_ascii_hexdigit() {
        C1_XDIGIT
    } else {
        0
    };
    let kind = match c {
        0x09 => C1_CNTRL | C1_SPACE | C1_BLANK,
        0x0A..=0x0D => C1_CNTRL | C1_SPACE,
        0x20 | 0xA0 => C1_SPACE | C1_BLANK,
        0..=0x1F | 0x7F..=0x9F => C1_CNTRL,
        0x30..=0x39 => C1_DIGIT,
        0xB2 | 0xB3 | 0xB9 => C1_DIGIT | C1_PUNCT,
        _ => match char::from_u32(c as u32) {
            Some(ch) if ch.is_alphabetic() => {
                let mut kind = C1_ALPHA;
                if ch.is_uppercase() {
                    kind |= C1_UPPER;
                }
                if ch.is_lowercase() {
                    kind |= C1_LOWER;
                }
                kind
            }
            Some(ch) if ch.is_whitespace() => C1_SPACE | C1_BLANK,
            Some(_) if convert::is_combining(c) => 0,
            Some(_) => C1_PUNCT,
            None => 0,
        },
    };
    kind | xdigit
}

/// The C2_ class (the direction of text) of the character `c`.
fn ctype2(c: u16) -> u16 {
    match c {
        0x30..=0x39 => C2_EUROPENUMBER,
        0x2B | 0x2D => C2_EUROPESEPARATOR,
        0x23..=0x25 | 0xA2..=0xA5 | 0xB0 | 0xB1 | 0xB2 | 0xB3 | 0xB9 => C2_EUROPETERMINATOR,
        0x2C | 0x2E | 0x2F | 0x3A | 0xA0 => C2_COMMONSEPARATOR,
        0x0A | 0x0D | 0x1C..=0x1E | 0x85 => C2_BLOCKSEPARATOR,
        0x09 | 0x0B | 0x1F => C2_SEGMENTSEPARATOR,
        0x0C | 0x20 => C2_WHITESPACE,
        _ => match ctype1(c) {
            t if t & C1_ALPHA != 0 => C2_LEFTTORIGHT,
            t if t & C1_PUNCT != 0 => C2_OTHERNEUTRAL,
            _ => 0,
        },
    }
}

/// The C3_ bits of the character `c`.
fn ctype3(c: u16) -> u16 {
    if convert::is_combining(c) {
        return C3_NONSPACING | C3_DIACRITIC;
    }
    let diacritic = match c {
        0x5E | 0x60 | 0xA8 | 0xAF | 0xB4 | 0xB8 | 0x02C6 | 0x02DC => C3_DIACRITIC,
        _ => 0,
    };
    let symbol = if ctype1(c) & C1_PUNCT != 0 && ctype1(c) & C1_ALPHA == 0 {
        C3_SYMBOL
    } else {
        0
    };
    diacritic | symbol
}

/// The symbols in the order they sort in, after the control characters
/// and before those not listed. Under SORT_STRINGSORT the apostrophe and
/// the hyphen come first.
const SYMBOLS: &[u16] = &[
    0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x20, 0xA0, 0x27, 0x2D, 0xAD, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26,
    0x28, 0x29, 0x2A, 0x2C, 0x2E, 0x2F, 0x3A, 0x3B, 0x3F, 0x40, 0x5B, 0x5C, 0x5D, 0x5E, 0x5F, 0x60,
    0x7B, 0x7C, 0x7D, 0x7E, 0xA1, 0xA6, 0xA8, 0xAF, 0xB4, 0xB8, 0xBF, 0x02C6, 0x02DC, 0x2018,
    0x2019, 0x201A, 0x201C, 0x201D, 0x201E, 0x2020, 0x2021, 0x2022, 0x2026, 0x2030, 0x2039, 0x203A,
    0x2013, 0x2014, 0x2B, 0xB1, 0xAB, 0xBB, 0x3C, 0x3D, 0x3E, 0xD7, 0xF7, 0xA2, 0xA3, 0xA4, 0xA5,
    0x20AC, 0xA7, 0xA9, 0xAC, 0xAE, 0xB0, 0xB6, 0xB7, 0x2122, 0xBC, 0xBD, 0xBE,
];

/// The groups of primary weights, in the order they sort in.
const SYMBOL: u32 = 0x07;
const DIGIT: u32 = 0x0C;
const LATIN: u32 = 0x0E;
const LETTER: u32 = 0x0F;
const OTHER: u32 = 0x10;

/// Diacritic weights: none, then the accents.
const PLAIN: u8 = 2;
const fn accent(mark: u16) -> u8 {
    match mark {
        convert::ACUTE => 3,
        convert::GRAVE => 4,
        convert::CIRCUMFLEX => 5,
        convert::DIAERESIS => 6,
        convert::TILDE => 7,
        convert::RING => 8,
        convert::CEDILLA => 9,
        convert::CARON => 10,
        _ => 11,
    }
}
const SUPERSCRIPT: u8 = 12;
const STROKE: u8 = 13;

/// Case weights.
const LOWER: u8 = 2;
const UPPER: u8 = 3;

/// One sorting element of a string: its primary weight (group and value),
/// its accent and its case.
#[derive(Clone, Copy)]
struct Element {
    primary: u32,
    diacritic: u8,
    case: u8,
}

/// What a character sorts as.
enum Sorts {
    Ignored,
    /// A hyphen or apostrophe, which counts only after all else.
    Special(u8),
    Elements(Vec<Element>),
}

fn element(group: u32, value: u32, diacritic: u8, upper: bool) -> Element {
    Element {
        primary: group << 24 | value,
        diacritic,
        case: if upper { UPPER } else { LOWER },
    }
}

/// The sorting of the character `c`.
fn sorts(c: u16, string_sort: bool) -> Sorts {
    let latin = |base: u8, diacritic: u8| {
        let upper = base.is_ascii_uppercase();
        element(
            LATIN,
            (base.to_ascii_lowercase() - b'a') as u32,
            diacritic,
            upper,
        )
    };
    let letters =
        |s: &[u8], diacritic: u8| Sorts::Elements(s.iter().map(|&b| latin(b, diacritic)).collect());
    let symbol = |c: u16| {
        let value = match SYMBOLS.iter().position(|&s| s == c) {
            Some(i) => 0x100 + i as u32,
            None => 0x1000 + c as u32,
        };
        Sorts::Elements(vec![element(SYMBOL, value, PLAIN, false)])
    };
    match c {
        0 => Sorts::Ignored,
        0x27 | 0x2D | 0xAD if !string_sort => Sorts::Special(c as u8),
        0x30..=0x39 => Sorts::Elements(vec![element(DIGIT, (c - 0x30) as u32, PLAIN, false)]),
        0xB9 => Sorts::Elements(vec![element(DIGIT, 1, SUPERSCRIPT, false)]),
        0xB2 | 0xB3 => Sorts::Elements(vec![element(DIGIT, (c - 0xB0) as u32, SUPERSCRIPT, false)]),
        0x41..=0x5A | 0x61..=0x7A => letters(&[c as u8], PLAIN),
        0xAA => letters(b"a", SUPERSCRIPT),
        0xBA => letters(b"o", SUPERSCRIPT),
        0xC6 => letters(b"AE", PLAIN),
        0xE6 => letters(b"ae", PLAIN),
        0x0152 => letters(b"OE", PLAIN),
        0x0153 => letters(b"oe", PLAIN),
        0xDF => letters(b"ss", PLAIN),
        0xDE => letters(b"TH", PLAIN),
        0xFE => letters(b"th", PLAIN),
        0xD0 => letters(b"D", STROKE),
        0xF0 => letters(b"d", STROKE),
        0xD8 => letters(b"O", STROKE),
        0xF8 => letters(b"o", STROKE),
        0x0192 => letters(b"f", STROKE),
        _ => {
            if let Some((base, mark)) = convert::decompose(c) {
                return letters(&[base as u8], accent(mark));
            }
            if convert::is_combining(c) {
                return Sorts::Ignored;
            }
            let t = ctype1(c);
            if t & C1_ALPHA != 0 {
                let ch = char::from_u32(c as u32).unwrap_or('\0');
                let lower = ch.to_lowercase().next().unwrap_or(ch) as u32;
                Sorts::Elements(vec![element(LETTER, lower, PLAIN, t & C1_UPPER != 0)])
            } else if t & (C1_PUNCT | C1_SPACE | C1_CNTRL) != 0 {
                symbol(c)
            } else {
                Sorts::Elements(vec![element(OTHER, c as u32, PLAIN, false)])
            }
        }
    }
}

/// The bytes of a weight: each at least 2, so that a key has no zero
/// and its parts' separator, 1, sorts below them.
fn weight_bytes(key: &mut Vec<u8>, value: u32) {
    key.push(2 + (value / 253 / 253 % 253) as u8);
    key.push(2 + (value / 253 % 253) as u8);
    key.push(2 + (value % 253) as u8);
}

/// The sort key of `text` (UTF-16): the primary weights, the accents', the
/// cases' and the hyphens' and apostrophes', each part ending with a 1,
/// so that comparing keys byte by byte compares the strings. It has no
/// terminating zero.
fn sort_key(flags: u32, text: &[u16]) -> Vec<u8> {
    let string_sort = flags & SORT_STRINGSORT != 0;
    let mut elements = Vec::new();
    let mut specials = Vec::new();
    for (i, &c) in text.iter().enumerate() {
        match sorts(c, string_sort) {
            Sorts::Ignored => {}
            Sorts::Special(_) if flags & NORM_IGNORESYMBOLS != 0 => {}
            Sorts::Special(w) => specials.push((i as u32, w)),
            Sorts::Elements(es) => elements.extend(
                es.into_iter()
                    .filter(|e| flags & NORM_IGNORESYMBOLS == 0 || e.primary >> 24 != SYMBOL),
            ),
        }
    }
    let mut key = Vec::new();
    for e in &elements {
        key.push((e.primary >> 24) as u8);
        weight_bytes(&mut key, e.primary & 0xFF_FFFF);
    }
    key.push(1);
    let level = |key: &mut Vec<u8>, weights: Vec<u8>| {
        let end = weights
            .iter()
            .rposition(|&w| w != PLAIN)
            .map_or(0, |i| i + 1);
        key.extend(&weights[..end]);
        key.push(1);
    };
    let diacritics = match flags & NORM_IGNORENONSPACE {
        0 => elements.iter().map(|e| e.diacritic).collect(),
        _ => Vec::new(),
    };
    level(&mut key, diacritics);
    let cases = match flags & NORM_IGNORECASE {
        0 => elements.iter().map(|e| e.case).collect(),
        _ => Vec::new(),
    };
    level(&mut key, cases);
    for (at, w) in specials {
        weight_bytes(&mut key, at);
        key.push(w);
    }
    key.push(1);
    key
}

/// How `a` compares with `b` (UTF-16) under the CompareString `flags`.
pub(super) fn compare(flags: u32, a: &[u16], b: &[u16]) -> Ordering {
    sort_key(flags, a).cmp(&sort_key(flags, b))
}

/// The ANSI string at `at`, `len` bytes or with -1 up to its zero (which
/// it leaves out), as UTF-16.
fn read_ansi(cpu: &mut Cpu, at: u32, len: u32) -> Option<Vec<u16>> {
    let mut bytes = read_units(cpu, at, len, 1)?;
    if (len as i32) < 0 {
        bytes.pop();
    }
    Some(
        bytes
            .iter()
            .map(|&b| CodePage::Ansi.char(b as u8))
            .collect(),
    )
}

/// CompareStringA: the string `args[2]` (`args[3]` bytes, or -1) against
/// `args[4]` (`args[5]`): 1 when it sorts first, 2 when they are equal, 3
/// when it sorts last, 0 on an error.
pub(super) fn compare_string_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[_, flags, a, a_len, b, b_len] = args else {
        unreachable!()
    };
    if flags & !COMPARE_FLAGS != 0 {
        set_error(cpu, ERROR_INVALID_FLAGS);
        return Some(0);
    }
    if a == 0 || b == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let a = read_ansi(cpu, a, a_len)?;
    let b = read_ansi(cpu, b, b_len)?;
    Some((compare(flags, &a, &b) as i32 + 2) as u32)
}

/// Upper or lower case of the ANSI character `b`, where the code page has
/// the other case of it.
fn change_case(b: u8, upper: bool) -> u8 {
    let c = char::from_u32(CodePage::Ansi.char(b) as u32).unwrap_or('\0');
    let mut changed: Vec<char> = if upper {
        c.to_uppercase().collect()
    } else {
        c.to_lowercase().collect()
    };
    match (changed.pop(), changed.is_empty()) {
        (Some(one), true) if (one as u32) < 0x1_0000 => {
            CodePage::Ansi.byte(one as u16).unwrap_or(b)
        }
        _ => b,
    }
}

/// LCMapStringA: the string `args[2]` (`args[3]` bytes, or -1) mapped as
/// `args[1]` says (to upper or lower case, without symbols, or to its sort
/// key) into `args[4]` (`args[5]` bytes, 0 for the size).
pub(super) fn lc_map_string_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[_, flags, from, len, to, size] = args else {
        unreachable!()
    };
    if from == 0 || len == 0 || (size as i32) < 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let exclusive = [
        LCMAP_LOWERCASE | LCMAP_UPPERCASE,
        LCMAP_HIRAGANA | LCMAP_KATAKANA,
        LCMAP_HALFWIDTH | LCMAP_FULLWIDTH,
        LCMAP_TRADITIONAL_CHINESE | LCMAP_SIMPLIFIED_CHINESE,
    ];
    let known = COMPARE_FLAGS
        | LCMAP_LOWERCASE
        | LCMAP_UPPERCASE
        | LCMAP_SORTKEY
        | LCMAP_BYTEREV
        | LCMAP_HIRAGANA
        | LCMAP_KATAKANA
        | LCMAP_HALFWIDTH
        | LCMAP_FULLWIDTH
        | LCMAP_LINGUISTIC_CASING
        | LCMAP_SIMPLIFIED_CHINESE
        | LCMAP_TRADITIONAL_CHINESE;
    if flags & !known != 0 || exclusive.iter().any(|&both| both & !flags == 0) {
        set_error(cpu, ERROR_INVALID_FLAGS);
        return Some(0);
    }
    let to = if size == 0 { 0 } else { to };
    if flags & LCMAP_SORTKEY != 0 {
        if from == to {
            set_error(cpu, ERROR_INVALID_FLAGS);
            return Some(0);
        }
        let text = read_ansi(cpu, from, len)?;
        let mut key = sort_key(flags, &text);
        key.push(0);
        let key: Vec<u16> = key.into_iter().map(u16::from).collect();
        return put_units(cpu, to, size, &key, 1);
    }
    if flags & SORT_STRINGSORT != 0 {
        set_error(cpu, ERROR_INVALID_FLAGS);
        return Some(0);
    }
    let bytes = read_units(cpu, from, len, 1)?;
    let mapped: Vec<u16> = bytes
        .into_iter()
        .filter(|&b| {
            flags & NORM_IGNORESYMBOLS == 0
                || ctype1(CodePage::Ansi.char(b as u8)) & (C1_PUNCT | C1_SPACE) == 0
        })
        .map(|b| match flags & (LCMAP_UPPERCASE | LCMAP_LOWERCASE) {
            LCMAP_UPPERCASE => change_case(b as u8, true) as u16,
            LCMAP_LOWERCASE => change_case(b as u8, false) as u16,
            _ => b,
        })
        .collect();
    put_units(cpu, to, size, &mapped, 1)
}

/// GetStringTypeA and GetStringTypeExA: the type `args[1]` (CT_CTYPE1, 2
/// or 3) of each character of the string `args[2]` (`args[3]` bytes, or
/// -1 with its zero), into the WORDs at `args[4]`.
pub(super) fn get_string_type_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[_, kind, from, len, to] = args else {
        unreachable!()
    };
    let classify: fn(u16) -> u16 = match kind {
        CT_CTYPE1 => ctype1,
        CT_CTYPE2 => ctype2,
        CT_CTYPE3 => ctype3,
        _ => {
            set_error(cpu, ERROR_INVALID_FLAGS);
            return Some(FALSE);
        }
    };
    if from == 0 || to == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let text = ansi_to_wide(
        &read_units(cpu, from, len, 1)?
            .into_iter()
            .map(|b| b as u8)
            .collect::<Vec<u8>>(),
    );
    for (i, &c) in text.iter().enumerate() {
        cpu.bus
            .guest_write_16(to.wrapping_add(2 * i as u32), classify(c));
    }
    Some(TRUE)
}
