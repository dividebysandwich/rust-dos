//! Conversions the KERNEL32 functions share: times (SYSTEMTIME, FILETIME
//! and DOS's packed dates and times) and text in the ANSI and OEM code
//! pages.

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

/// FILETIME ticks: 100 ns each, from 1 January 1601.
const TICKS_PER_MS: u64 = 10_000;
const MS_PER_DAY: u64 = 86_400_000;

/// A SYSTEMTIME: year, month, day of the week, day, hour, minute, second,
/// millisecond.
pub type SystemTime = [u16; 8];

fn epoch() -> NaiveDate {
    NaiveDate::from_ymd_opt(1601, 1, 1).expect("1601-01-01")
}

/// The FILETIME of `st`, or None if it isn't a valid time (the day of the
/// week isn't looked at).
pub fn system_time_to_ticks(st: &SystemTime) -> Option<u64> {
    let &[year, month, _, day, hour, minute, second, ms] = st;
    if hour > 23 || minute > 59 || second > 59 || ms > 999 {
        return None;
    }
    let date = NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32)?;
    let days = u64::try_from((date - epoch()).num_days()).ok()?;
    let ms = days * MS_PER_DAY
        + ((hour as u64 * 60 + minute as u64) * 60 + second as u64) * 1000
        + ms as u64;
    Some(ms * TICKS_PER_MS)
}

/// The SYSTEMTIME of the FILETIME `ticks`.
pub fn ticks_to_system_time(ticks: u64) -> Option<SystemTime> {
    let ms = ticks / TICKS_PER_MS;
    let date = epoch().checked_add_days(chrono::Days::new(ms / MS_PER_DAY))?;
    let in_day = ms % MS_PER_DAY;
    let seconds = in_day / 1000;
    Some([
        date.year() as u16,
        date.month() as u16,
        date.weekday().num_days_from_sunday() as u16,
        date.day() as u16,
        (seconds / 3600) as u16,
        (seconds / 60 % 60) as u16,
        (seconds % 60) as u16,
        (in_day % 1000) as u16,
    ])
}

/// The SYSTEMTIME of a time of the machine's clock.
pub fn system_time_of(at: NaiveDateTime) -> SystemTime {
    [
        at.year() as u16,
        at.month() as u16,
        at.weekday().num_days_from_sunday() as u16,
        at.day() as u16,
        at.hour() as u16,
        at.minute() as u16,
        at.second() as u16,
        (at.nanosecond() / 1_000_000).min(999) as u16,
    ]
}

/// The FILETIME of a DOS date and time, as INT 21h AX=5700h gives them.
/// Windows 95 takes any day of a month up to 31 and moves on into the
/// next month, but no month above 12.
pub fn dos_to_ticks(date: u16, time: u16) -> Option<u64> {
    let year = 1980 + (date >> 9) as i32;
    let month = (date >> 5 & 0xF) as u32;
    let day = (date & 0x1F) as u64;
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let days = u64::try_from((first - epoch()).num_days()).ok()? + day.saturating_sub(1);
    let seconds =
        (time >> 11) as u64 * 3600 + (time >> 5 & 0x3F) as u64 * 60 + (time & 0x1F) as u64 * 2;
    Some((days * MS_PER_DAY + seconds * 1000) * TICKS_PER_MS)
}

/// The DOS date and time of the FILETIME `ticks`, the seconds rounded
/// down to even ones, or None outside 1980 to 2107.
pub fn ticks_to_dos(ticks: u64) -> Option<(u16, u16)> {
    let st = ticks_to_system_time(ticks)?;
    if !(1980..=2107).contains(&st[0]) {
        return None;
    }
    let date = (st[0] - 1980) << 9 | st[1] << 5 | st[3];
    let time = st[4] << 11 | st[5] << 5 | (st[6] / 2);
    Some((date, time))
}


/// The code pages a US Windows 95 has: the ANSI one, 1252 (CP_ACP), and
/// the OEM ones, 437 (CP_OEMCP) and 850.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodePage {
    Ansi,
    Oem,
    Multilingual,
}

/// CP_ACP and CP_OEMCP, which name the system's code pages.
pub const CP_ACP: u32 = 0;
pub const CP_OEMCP: u32 = 1;
pub const ANSI_CODE_PAGE: u32 = 1252;
pub const OEM_CODE_PAGE: u32 = 437;

impl CodePage {
    /// The code page `number` names, CP_ACP and CP_OEMCP among them.
    pub fn of(number: u32) -> Option<CodePage> {
        match number {
            CP_ACP | ANSI_CODE_PAGE => Some(CodePage::Ansi),
            CP_OEMCP | OEM_CODE_PAGE => Some(CodePage::Oem),
            850 => Some(CodePage::Multilingual),
            _ => None,
        }
    }

    /// The character of `byte`. In the OEM code pages the control
    /// characters stand for themselves, as Windows converts them unless
    /// asked for their glyphs (`glyph`).
    pub fn char(self, byte: u8) -> u16 {
        match (self, byte) {
            (CodePage::Ansi, 0x80..=0x9F) => CP1252_80[byte as usize - 0x80],
            (CodePage::Ansi, _) | (_, 0..=0x7F) => byte as u16,
            (CodePage::Oem, _) => crate::video::CP437[byte as usize] as u16,
            (CodePage::Multilingual, _) => CP850_80[byte as usize - 0x80],
        }
    }

    /// The character of `byte` as the screen shows it in an OEM code page:
    /// the control characters' glyphs (MB_USEGLYPHCHARS).
    pub fn glyph(self, byte: u8) -> u16 {
        match (self, byte) {
            (CodePage::Ansi, _) | (_, 0x20..=0x7E) | (_, 0x80..=0xFF) => self.char(byte),
            _ => crate::video::CP437[byte as usize] as u16,
        }
    }

    /// The byte of the character `c`, if the code page has it. The OEM
    /// code pages take the control characters' glyphs too.
    pub fn byte(self, c: u16) -> Option<u8> {
        if c < 0x80 {
            return Some(c as u8);
        }
        let high = |table: &[u16]| table.iter().position(|&x| x == c).map(|i| 0x80 + i as u8);
        let glyphs = || {
            (0..0x20u8)
                .chain([0x7F])
                .find(|&b| crate::video::CP437[b as usize] as u16 == c)
        };
        match self {
            CodePage::Ansi => match c {
                0xA0..=0xFF => Some(c as u8),
                _ => high(&CP1252_80),
            },
            CodePage::Oem => (0x80..=0xFFu8)
                .find(|&b| crate::video::CP437[b as usize] as u16 == c)
                .or_else(glyphs),
            CodePage::Multilingual => high(&CP850_80).or_else(glyphs),
        }
    }
}

/// Bytes 80h to 9Fh of code page 1252, where it differs from Latin-1.
/// Those it leaves out stand for themselves, as on Windows.
const CP1252_80: [u16; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
];

/// Bytes 80h to FFh of code page 850 (Multilingual Latin I).
const CP850_80: [u16; 128] = [
    0x00C7, 0x00FC, 0x00E9, 0x00E2, 0x00E4, 0x00E0, 0x00E5, 0x00E7, 0x00EA, 0x00EB, 0x00E8, 0x00EF,
    0x00EE, 0x00EC, 0x00C4, 0x00C5, 0x00C9, 0x00E6, 0x00C6, 0x00F4, 0x00F6, 0x00F2, 0x00FB, 0x00F9,
    0x00FF, 0x00D6, 0x00DC, 0x00F8, 0x00A3, 0x00D8, 0x00D7, 0x0192, 0x00E1, 0x00ED, 0x00F3, 0x00FA,
    0x00F1, 0x00D1, 0x00AA, 0x00BA, 0x00BF, 0x00AE, 0x00AC, 0x00BD, 0x00BC, 0x00A1, 0x00AB, 0x00BB,
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x00C1, 0x00C2, 0x00C0, 0x00A9, 0x2563, 0x2551, 0x2557,
    0x255D, 0x00A2, 0x00A5, 0x2510, 0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x00E3, 0x00C3,
    0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x00A4, 0x00F0, 0x00D0, 0x00CA, 0x00CB,
    0x00C8, 0x0131, 0x00CD, 0x00CE, 0x00CF, 0x2518, 0x250C, 0x2588, 0x2584, 0x00A6, 0x00CC, 0x2580,
    0x00D3, 0x00DF, 0x00D4, 0x00D2, 0x00F5, 0x00D5, 0x00B5, 0x00FE, 0x00DE, 0x00DA, 0x00DB, 0x00D9,
    0x00FD, 0x00DD, 0x00AF, 0x00B4, 0x00AD, 0x00B1, 0x2017, 0x00BE, 0x00B6, 0x00A7, 0x00F7, 0x00B8,
    0x00B0, 0x00A8, 0x00B7, 0x00B9, 0x00B3, 0x00B2, 0x25A0, 0x00A0,
];

/// The combining marks the precomposed letters below are made of.
pub const GRAVE: u16 = 0x0300;
pub const ACUTE: u16 = 0x0301;
pub const CIRCUMFLEX: u16 = 0x0302;
pub const TILDE: u16 = 0x0303;
pub const DIAERESIS: u16 = 0x0308;
pub const RING: u16 = 0x030A;
pub const CARON: u16 = 0x030C;
pub const CEDILLA: u16 = 0x0327;

/// The accented letters of the code pages above as a letter and a
/// combining mark, for MB_COMPOSITE, WC_COMPOSITECHECK and sorting.
const DECOMPOSED: &[(u16, u16, u16)] = &[
    (0xC0, 0x41, GRAVE),
    (0xC1, 0x41, ACUTE),
    (0xC2, 0x41, CIRCUMFLEX),
    (0xC3, 0x41, TILDE),
    (0xC4, 0x41, DIAERESIS),
    (0xC5, 0x41, RING),
    (0xC7, 0x43, CEDILLA),
    (0xC8, 0x45, GRAVE),
    (0xC9, 0x45, ACUTE),
    (0xCA, 0x45, CIRCUMFLEX),
    (0xCB, 0x45, DIAERESIS),
    (0xCC, 0x49, GRAVE),
    (0xCD, 0x49, ACUTE),
    (0xCE, 0x49, CIRCUMFLEX),
    (0xCF, 0x49, DIAERESIS),
    (0xD1, 0x4E, TILDE),
    (0xD2, 0x4F, GRAVE),
    (0xD3, 0x4F, ACUTE),
    (0xD4, 0x4F, CIRCUMFLEX),
    (0xD5, 0x4F, TILDE),
    (0xD6, 0x4F, DIAERESIS),
    (0xD9, 0x55, GRAVE),
    (0xDA, 0x55, ACUTE),
    (0xDB, 0x55, CIRCUMFLEX),
    (0xDC, 0x55, DIAERESIS),
    (0xDD, 0x59, ACUTE),
    (0xE0, 0x61, GRAVE),
    (0xE1, 0x61, ACUTE),
    (0xE2, 0x61, CIRCUMFLEX),
    (0xE3, 0x61, TILDE),
    (0xE4, 0x61, DIAERESIS),
    (0xE5, 0x61, RING),
    (0xE7, 0x63, CEDILLA),
    (0xE8, 0x65, GRAVE),
    (0xE9, 0x65, ACUTE),
    (0xEA, 0x65, CIRCUMFLEX),
    (0xEB, 0x65, DIAERESIS),
    (0xEC, 0x69, GRAVE),
    (0xED, 0x69, ACUTE),
    (0xEE, 0x69, CIRCUMFLEX),
    (0xEF, 0x69, DIAERESIS),
    (0xF1, 0x6E, TILDE),
    (0xF2, 0x6F, GRAVE),
    (0xF3, 0x6F, ACUTE),
    (0xF4, 0x6F, CIRCUMFLEX),
    (0xF5, 0x6F, TILDE),
    (0xF6, 0x6F, DIAERESIS),
    (0xF9, 0x75, GRAVE),
    (0xFA, 0x75, ACUTE),
    (0xFB, 0x75, CIRCUMFLEX),
    (0xFC, 0x75, DIAERESIS),
    (0xFD, 0x79, ACUTE),
    (0xFF, 0x79, DIAERESIS),
    (0x0160, 0x53, CARON),
    (0x0161, 0x73, CARON),
    (0x0178, 0x59, DIAERESIS),
    (0x017D, 0x5A, CARON),
    (0x017E, 0x7A, CARON),
];

/// The letter and combining mark the precomposed character `c` is made
/// of, if it is one.
pub fn decompose(c: u16) -> Option<(u16, u16)> {
    DECOMPOSED
        .iter()
        .find(|&&(p, _, _)| p == c)
        .map(|&(_, base, mark)| (base, mark))
}

/// The precomposed character of the letter `base` with the combining mark
/// `mark`, if there is one.
pub fn compose(base: u16, mark: u16) -> Option<u16> {
    DECOMPOSED
        .iter()
        .find(|&&(_, b, m)| b == base && m == mark)
        .map(|&(p, _, _)| p)
}

/// Whether `c` is a combining (non-spacing) mark.
pub fn is_combining(c: u16) -> bool {
    (0x0300..=0x036F).contains(&c)
}
