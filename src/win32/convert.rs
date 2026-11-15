//! Conversions the KERNEL32 functions share: times (SYSTEMTIME, FILETIME
//! and DOS's packed dates and times), and accented letters as a letter
//! and a combining mark. The code pages are in nls.rs.

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


/// The combining marks the precomposed letters below are made of.
pub const GRAVE: u16 = 0x0300;
pub const ACUTE: u16 = 0x0301;
pub const CIRCUMFLEX: u16 = 0x0302;
pub const TILDE: u16 = 0x0303;
pub const DIAERESIS: u16 = 0x0308;
pub const RING: u16 = 0x030A;
pub const CARON: u16 = 0x030C;
pub const CEDILLA: u16 = 0x0327;
pub const MACRON: u16 = 0x0304;
pub const BREVE: u16 = 0x0306;
pub const DOT_ABOVE: u16 = 0x0307;
pub const DOUBLE_ACUTE: u16 = 0x030B;
pub const OGONEK: u16 = 0x0328;

/// The accented letters of the code pages (nls.rs) as a letter and a
/// combining mark, as Unicode decomposes them, for MB_COMPOSITE,
/// WC_COMPOSITECHECK and sorting.
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
    (0x0100, 0x41, MACRON),
    (0x0101, 0x61, MACRON),
    (0x0102, 0x41, BREVE),
    (0x0103, 0x61, BREVE),
    (0x0104, 0x41, OGONEK),
    (0x0105, 0x61, OGONEK),
    (0x0106, 0x43, ACUTE),
    (0x0107, 0x63, ACUTE),
    (0x010C, 0x43, CARON),
    (0x010D, 0x63, CARON),
    (0x010E, 0x44, CARON),
    (0x010F, 0x64, CARON),
    (0x0112, 0x45, MACRON),
    (0x0113, 0x65, MACRON),
    (0x0116, 0x45, DOT_ABOVE),
    (0x0117, 0x65, DOT_ABOVE),
    (0x0118, 0x45, OGONEK),
    (0x0119, 0x65, OGONEK),
    (0x011A, 0x45, CARON),
    (0x011B, 0x65, CARON),
    (0x011E, 0x47, BREVE),
    (0x011F, 0x67, BREVE),
    (0x0122, 0x47, CEDILLA),
    (0x0123, 0x67, CEDILLA),
    (0x012A, 0x49, MACRON),
    (0x012B, 0x69, MACRON),
    (0x012E, 0x49, OGONEK),
    (0x012F, 0x69, OGONEK),
    (0x0130, 0x49, DOT_ABOVE),
    (0x0136, 0x4B, CEDILLA),
    (0x0137, 0x6B, CEDILLA),
    (0x0139, 0x4C, ACUTE),
    (0x013A, 0x6C, ACUTE),
    (0x013B, 0x4C, CEDILLA),
    (0x013C, 0x6C, CEDILLA),
    (0x013D, 0x4C, CARON),
    (0x013E, 0x6C, CARON),
    (0x0143, 0x4E, ACUTE),
    (0x0144, 0x6E, ACUTE),
    (0x0145, 0x4E, CEDILLA),
    (0x0146, 0x6E, CEDILLA),
    (0x0147, 0x4E, CARON),
    (0x0148, 0x6E, CARON),
    (0x014C, 0x4F, MACRON),
    (0x014D, 0x6F, MACRON),
    (0x0150, 0x4F, DOUBLE_ACUTE),
    (0x0151, 0x6F, DOUBLE_ACUTE),
    (0x0154, 0x52, ACUTE),
    (0x0155, 0x72, ACUTE),
    (0x0158, 0x52, CARON),
    (0x0159, 0x72, CARON),
    (0x015A, 0x53, ACUTE),
    (0x015B, 0x73, ACUTE),
    (0x015E, 0x53, CEDILLA),
    (0x015F, 0x73, CEDILLA),
    (0x0162, 0x54, CEDILLA),
    (0x0163, 0x74, CEDILLA),
    (0x0164, 0x54, CARON),
    (0x0165, 0x74, CARON),
    (0x016A, 0x55, MACRON),
    (0x016B, 0x75, MACRON),
    (0x016E, 0x55, RING),
    (0x016F, 0x75, RING),
    (0x0170, 0x55, DOUBLE_ACUTE),
    (0x0171, 0x75, DOUBLE_ACUTE),
    (0x0172, 0x55, OGONEK),
    (0x0173, 0x75, OGONEK),
    (0x0179, 0x5A, ACUTE),
    (0x017A, 0x7A, ACUTE),
    (0x017B, 0x5A, DOT_ABOVE),
    (0x017C, 0x7A, DOT_ABOVE),
    (0x0386, 0x0391, ACUTE),
    (0x0388, 0x0395, ACUTE),
    (0x0389, 0x0397, ACUTE),
    (0x038A, 0x0399, ACUTE),
    (0x038C, 0x039F, ACUTE),
    (0x038E, 0x03A5, ACUTE),
    (0x038F, 0x03A9, ACUTE),
    (0x03AA, 0x0399, DIAERESIS),
    (0x03AB, 0x03A5, DIAERESIS),
    (0x03AC, 0x03B1, ACUTE),
    (0x03AD, 0x03B5, ACUTE),
    (0x03AE, 0x03B7, ACUTE),
    (0x03AF, 0x03B9, ACUTE),
    (0x03CA, 0x03B9, DIAERESIS),
    (0x03CB, 0x03C5, DIAERESIS),
    (0x03CC, 0x03BF, ACUTE),
    (0x03CD, 0x03C5, ACUTE),
    (0x03CE, 0x03C9, ACUTE),
    (0x0401, 0x0415, DIAERESIS),
    (0x0403, 0x0413, ACUTE),
    (0x0407, 0x0406, DIAERESIS),
    (0x040C, 0x041A, ACUTE),
    (0x040E, 0x0423, BREVE),
    (0x0419, 0x0418, BREVE),
    (0x0439, 0x0438, BREVE),
    (0x0451, 0x0435, DIAERESIS),
    (0x0453, 0x0433, ACUTE),
    (0x0457, 0x0456, DIAERESIS),
    (0x045C, 0x043A, ACUTE),
    (0x045E, 0x0443, BREVE),
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
