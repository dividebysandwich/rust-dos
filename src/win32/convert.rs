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

/// Code pages a program names: the ANSI one (CP_ACP, or 1252 by its
/// number) and the OEM one (CP_OEMCP, or 437), as on a US Windows 95.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CodePage {
    Ansi,
    Oem,
}

impl CodePage {
    pub fn of(number: u32) -> Option<CodePage> {
        match number {
            0 | 1252 => Some(CodePage::Ansi),
            1 | 437 => Some(CodePage::Oem),
            _ => None,
        }
    }

    /// The character of `byte`.
    pub fn char(self, byte: u8) -> u16 {
        match self {
            CodePage::Ansi => match byte {
                0x80..=0x9F => CP1252_80[byte as usize - 0x80],
                _ => byte as u16,
            },
            CodePage::Oem => crate::video::CP437[byte as usize] as u16,
        }
    }

    /// The byte of the character `c`, if the code page has it.
    pub fn byte(self, c: u16) -> Option<u8> {
        match self {
            CodePage::Ansi => match c {
                0..=0x7F | 0xA0..=0xFF => Some(c as u8),
                _ => CP1252_80
                    .iter()
                    .position(|&x| x == c)
                    .map(|i| 0x80 + i as u8),
            },
            CodePage::Oem => char::from_u32(c as u32).and_then(crate::keylayout::cp437),
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
