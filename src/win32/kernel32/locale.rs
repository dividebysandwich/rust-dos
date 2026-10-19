//! The locale: English (United States), the only one a US Windows 95
//! has the data of here. Its information (GetLocaleInfo), the formats of
//! dates, times, numbers and money made with it, and the enumerations of
//! locales and formats, which call the program back.

use super::text::{ERROR_INVALID_FLAGS, put_units, read_string};
use super::{ERROR_INVALID_PARAMETER, FALSE, TRUE, set_error};
use crate::cpu::Cpu;
use crate::win32::call::{self, Call};
use crate::win32::convert::{self, SystemTime};

/// The locale, its language and their defaults' names.
const LCID_EN_US: u32 = 0x0409;
const LOCALE_SYSTEM_DEFAULT: u32 = 0x0800;
const LOCALE_USER_DEFAULT: u32 = 0x0400;
const LOCALE_NEUTRAL: u32 = 0;
const SUBLANG_NEUTRAL: u32 = 0;
const SUBLANG_DEFAULT: u32 = 1;

/// GetLocaleInfo's flags, in the LCTYPE's top bits.
const LOCALE_NOUSEROVERRIDE: u32 = 0x8000_0000;
const LOCALE_USE_CP_ACP: u32 = 0x4000_0000;
const LOCALE_RETURN_NUMBER: u32 = 0x2000_0000;
const LOCALE_RETURN_GENITIVE_NAMES: u32 = 0x1000_0000;

/// LCTYPEs the formats below read.
const LOCALE_SSHORTDATE: u32 = 0x1F;
const LOCALE_SLONGDATE: u32 = 0x20;
const LOCALE_IDATE: u32 = 0x21;
const LOCALE_ILDATE: u32 = 0x22;
const LOCALE_S1159: u32 = 0x28;
const LOCALE_S2359: u32 = 0x29;
const LOCALE_SDAYNAME1: u32 = 0x2A;
const LOCALE_SABBREVDAYNAME1: u32 = 0x31;
const LOCALE_SMONTHNAME1: u32 = 0x38;
const LOCALE_SABBREVMONTHNAME1: u32 = 0x44;
const LOCALE_STIMEFORMAT: u32 = 0x1003;

const DAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The locale's information of the kind `lctype`, as Windows 95 has it
/// for English (United States).
fn info(lctype: u32) -> Option<String> {
    let text = match lctype {
        0x01 | 0x09 => "0409",
        0x02 => "English (United States)",
        0x03 => "ENU",
        0x04 | 0x1001 => "English",
        0x05 | 0x0A => "1",
        0x06 | 0x08 | 0x1002 => "United States",
        0x07 => "USA",
        0x0B => "437",
        0x0C => ",",
        0x0D => "1",
        0x0E | 0x16 => ".",
        0x0F | 0x17 => ",",
        0x10 | 0x18 => "3;0",
        0x11 | 0x19 | 0x1A => "2",
        0x12 => "1",
        0x13 => "0123456789",
        0x14 => "$",
        0x15 => "USD",
        0x1B | 0x1C => "0",
        0x1D => "/",
        0x1E => ":",
        LOCALE_SSHORTDATE => "M/d/yy",
        LOCALE_SLONGDATE => "dddd, MMMM dd, yyyy",
        0x21..=0x27 => "0",
        LOCALE_S1159 => "AM",
        LOCALE_S2359 => "PM",
        0x2A..=0x30 => DAYS[(lctype - LOCALE_SDAYNAME1) as usize],
        0x31..=0x37 => &DAYS[(lctype - LOCALE_SABBREVDAYNAME1) as usize][..3],
        0x38..=0x43 => MONTHS[(lctype - LOCALE_SMONTHNAME1) as usize],
        0x44..=0x4F => &MONTHS[(lctype - LOCALE_SABBREVMONTHNAME1) as usize][..3],
        0x50 => "",
        0x51 => "-",
        0x52 => "3",
        0x53 | 0x55 | 0x57 => "0",
        0x54 | 0x56 => "1",
        0x59 => "en",
        0x5A => "US",
        LOCALE_STIMEFORMAT => "h:mm:ss tt",
        0x1004 => "1252",
        0x1005 => "0",
        0x1009 => "1",
        0x100B => "0",
        0x100C => "6",
        0x100D => "0",
        0x100E | 0x100F => "",
        0x1010 => "1",
        0x1011 => "10000",
        _ => return None,
    };
    Some(text.to_string())
}

/// The locale `lcid` stands for: the user's or the system's default for
/// those, SUBLANG_DEFAULT for a language's neutral sublanguage, as
/// ConvertDefaultLocale says.
fn convert_default(lcid: u32) -> u32 {
    match lcid {
        LOCALE_SYSTEM_DEFAULT | LOCALE_USER_DEFAULT | LOCALE_NEUTRAL => LCID_EN_US,
        _ if lcid >> 10 & 0x3F == SUBLANG_NEUTRAL => lcid & !0xFC00 | SUBLANG_DEFAULT << 10,
        _ => lcid,
    }
}

/// Whether the locale `lcid` names is the one there is.
fn known(lcid: u32) -> bool {
    convert_default(lcid) == LCID_EN_US
}

/// Check the locale of a function: false, with the error set, if it is
/// another.
fn check_locale(cpu: &mut Cpu, lcid: u32) -> bool {
    let ok = known(lcid);
    if !ok {
        set_error(cpu, ERROR_INVALID_PARAMETER);
    }
    ok
}

/// Write the ANSI `text` and its zero to the buffer at `to` of `size`
/// bytes, as the locale functions do: with a size of 0 only its length
/// with the zero; when it doesn't fit, what fits and an error.
fn put_text(cpu: &mut Cpu, to: u32, size: u32, text: &[u8]) -> Option<u32> {
    let mut units: Vec<u16> = text.iter().map(|&b| b as u16).collect();
    units.push(0);
    put_units(cpu, to, size, &units, 1)
}

/// The size and buffer arguments are wrong: a negative size, or a size
/// without a buffer.
fn bad_buffer(cpu: &mut Cpu, to: u32, size: u32) -> bool {
    let bad = (size as i32) < 0 || (size != 0 && to == 0);
    if bad {
        set_error(cpu, ERROR_INVALID_PARAMETER);
    }
    bad
}

/// GetLocaleInfoA: the information `args[1]` of the locale `args[0]` into
/// `args[2]` (`args[3]` bytes, 0 for the size). Windows 95 has no
/// LOCALE_RETURN_NUMBER and no genitive month names.
pub(super) fn get_locale_info_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[lcid, lctype, to, size] = args else {
        unreachable!()
    };
    if bad_buffer(cpu, to, size) {
        return Some(0);
    }
    if lctype & (LOCALE_RETURN_NUMBER | LOCALE_RETURN_GENITIVE_NAMES) != 0 {
        set_error(cpu, ERROR_INVALID_FLAGS);
        return Some(0);
    }
    if !check_locale(cpu, lcid) {
        return Some(0);
    }
    let Some(text) = info(lctype & !(LOCALE_NOUSEROVERRIDE | LOCALE_USE_CP_ACP)) else {
        set_error(cpu, ERROR_INVALID_FLAGS);
        return Some(0);
    };
    put_text(cpu, to, size, text.as_bytes())
}

/// SetLocaleInfoA: Windows takes changes of the user's formats here; this
/// takes them (TRUE) without keeping them, and refuses the kinds that
/// can't be changed, as Windows does.
pub(super) fn set_locale_info_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[lcid, lctype, data] = args else {
        unreachable!()
    };
    if data == 0 || !known(lcid) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let lctype = lctype & !(LOCALE_NOUSEROVERRIDE | LOCALE_USE_CP_ACP);
    let settable = matches!(lctype, 0x0C..=0x20 | 0x23..=0x29 | 0x50 | 0x51 | LOCALE_STIMEFORMAT)
        && !matches!(lctype, 0x13 | 0x15 | 0x1A);
    if !settable || lctype == LOCALE_IDATE || lctype == LOCALE_ILDATE {
        set_error(cpu, ERROR_INVALID_FLAGS);
        return Some(FALSE);
    }
    read_string(cpu, data)?;
    Some(TRUE)
}

/// GetUserDefaultLCID, GetSystemDefaultLCID and GetThreadLocale.
pub(super) fn get_default_lcid(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(LCID_EN_US)
}

/// GetUserDefaultLangID and GetSystemDefaultLangID.
pub(super) fn get_default_lang_id(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(LCID_EN_US & 0xFFFF)
}

/// SetThreadLocale: only to the locale there is.
pub(super) fn set_thread_locale(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(check_locale(cpu, args[0]) as u32)
}

/// IsValidLocale: the locale there is, installed and supported.
pub(super) fn is_valid_locale(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[lcid, flags] = args else { unreachable!() };
    Some((flags <= 2 && lcid == LCID_EN_US) as u32)
}

pub(super) fn convert_default_locale(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(convert_default(args[0]))
}

/// What the enumerations below list.
const LOCALES: u32 = 0;
const SHORT_DATES: u32 = 1;
const LONG_DATES: u32 = 2;
const TIMES: u32 = 3;

/// The strings the enumeration `kind` gives, in order: the locale's
/// default first.
fn enumerated(kind: u32) -> &'static [&'static str] {
    match kind {
        LOCALES => &["00000409"],
        SHORT_DATES => &[
            "M/d/yy",
            "M/d/yyyy",
            "MM/dd/yy",
            "MM/dd/yyyy",
            "yy/MM/dd",
            "dd-MMM-yy",
        ],
        LONG_DATES => &[
            "dddd, MMMM dd, yyyy",
            "MMMM dd, yyyy",
            "dddd, dd MMMM, yyyy",
            "dd MMMM, yyyy",
        ],
        _ => &["h:mm:ss tt", "hh:mm:ss tt", "H:mm:ss", "HH:mm:ss"],
    }
}

/// Call the enumeration's callback `data[0]` with its `data[2]`th string,
/// which goes on the stack below the stub's: false if the stack isn't
/// there.
fn call_with_string(cpu: &mut Cpu, resume: (u32, u32), data: Vec<u32>) -> bool {
    let text = enumerated(data[1])[data[2] as usize];
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    let at = resume
        .1
        .wrapping_sub(bytes.len().next_multiple_of(4) as u32 + 16);
    if !cpu.bus.guest_probe(at, bytes.len(), true) {
        cpu.bus.guest_fault = None;
        return false;
    }
    cpu.bus.guest_write_bytes(at, &bytes);
    cpu.set_esp(at);
    call::call(cpu, data[0], &[at], call::ENUM_STRINGS, resume, data)
}

/// Start the enumeration `kind` with the callback `callback`: no value
/// now (None) while the callback runs, the result when it has run.
fn enumerate(cpu: &mut Cpu, callback: u32, kind: u32) -> Option<u32> {
    let resume = call::here(cpu);
    if !call_with_string(cpu, resume, vec![callback, kind, 0]) {
        return Some(FALSE);
    }
    None
}

/// An enumeration's callback returned: on to the next string while it
/// returns TRUE and there are more, else the enumeration returns TRUE.
pub(in crate::win32) fn enum_returned(cpu: &mut Cpu, value: u32, call: Call) {
    let mut data = call.data;
    data[2] += 1;
    let more = value != 0 && (data[2] as usize) < enumerated(data[1]).len();
    if !more || !call_with_string(cpu, call.resume, data) {
        call::finish(cpu, call.resume, TRUE);
    }
}

/// EnumSystemLocalesA: the locale there is, "00000409".
pub(super) fn enum_system_locales_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    enumerate(cpu, args[0], LOCALES)
}

const DATE_SHORTDATE: u32 = 0x1;
const DATE_LONGDATE: u32 = 0x2;
const DATE_USE_ALT_CALENDAR: u32 = 0x4;
const DATE_LTRREADING: u32 = 0x10;
const DATE_RTLREADING: u32 = 0x20;

/// EnumDateFormatsA: the short or the long date formats. Windows 95 has
/// no DATE_YEARMONTH.
pub(super) fn enum_date_formats_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[callback, lcid, flags] = args else {
        unreachable!()
    };
    if callback == 0 || !check_locale(cpu, lcid) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let kind = match flags & !LOCALE_USE_CP_ACP {
        0 | DATE_SHORTDATE => SHORT_DATES,
        DATE_LONGDATE => LONG_DATES,
        _ => {
            set_error(cpu, ERROR_INVALID_FLAGS);
            return Some(FALSE);
        }
    };
    enumerate(cpu, callback, kind)
}

pub(super) fn enum_time_formats_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[callback, lcid, flags] = args else {
        unreachable!()
    };
    if callback == 0 || !check_locale(cpu, lcid) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    if flags & !LOCALE_USE_CP_ACP != 0 {
        set_error(cpu, ERROR_INVALID_FLAGS);
        return Some(FALSE);
    }
    enumerate(cpu, callback, TIMES)
}

const TIME_NOMINUTESORSECONDS: u32 = 0x1;
const TIME_NOSECONDS: u32 = 0x2;
const TIME_NOTIMEMARKER: u32 = 0x4;
const TIME_FORCE24HOURFORMAT: u32 = 0x8;

/// `value` with at least `digits` digits.
fn number(value: u32, digits: usize) -> Vec<u8> {
    format!("{:0digits$}", value).into_bytes()
}

/// A date or a time (`date`) in the picture `format`, as GetDateFormat
/// and GetTimeFormat make it. Text in quotes is copied; with the
/// TIME_NO... flags a left-out part takes the text before it with it, up
/// to the part before, and the text after it up to the next part, but
/// for spaces.
fn format_date_time(format: &[u8], st: &SystemTime, flags: u32, date: bool) -> Vec<u8> {
    let &[year, month, weekday, day, hour, minute, second, _] = st;
    let mut out = Vec::new();
    let mut last = 0;
    let mut skipping = false;
    let mut i = 0;
    while i < format.len() {
        let c = format[i];
        if c == b'\'' {
            i += 1;
            while i < format.len() {
                if format[i] == b'\'' {
                    i += 1;
                    if format.get(i) != Some(&b'\'') {
                        break;
                    }
                }
                if !skipping {
                    out.push(format[i]);
                }
                i += 1;
            }
            continue;
        }
        let pictures: &[u8] = if date { b"dMyg" } else { b"hHmst" };
        if !pictures.contains(&c) {
            if !skipping || c == b' ' {
                out.push(c);
            }
            i += 1;
            continue;
        }
        let mut count = 0;
        while format.get(i) == Some(&c) {
            count += 1;
            i += 1;
        }
        skipping = false;
        let hour12 = if hour == 0 { 12 } else { (hour - 1) % 12 + 1 };
        let marker = if hour < 12 { "AM" } else { "PM" };
        let text = match c {
            b'd' if count >= 4 => DAYS[(weekday as usize + 6) % 7].as_bytes().to_vec(),
            b'd' if count == 3 => DAYS[(weekday as usize + 6) % 7].as_bytes()[..3].to_vec(),
            b'd' => number(day as u32, count),
            b'M' if count >= 4 => MONTHS[month as usize - 1].as_bytes().to_vec(),
            b'M' if count == 3 => MONTHS[month as usize - 1].as_bytes()[..3].to_vec(),
            b'M' => number(month as u32, count),
            b'y' if count >= 4 => number(year as u32, 4),
            b'y' => number(year as u32 % 100, count.min(2)),
            b'g' if count == 2 => b"AD".to_vec(),
            b'g' => Vec::new(),
            b'h' if flags & TIME_FORCE24HOURFORMAT == 0 => number(hour12 as u32, count.min(2)),
            b'h' | b'H' => number(hour as u32, count.min(2)),
            b'm' if flags & TIME_NOMINUTESORSECONDS == 0 => number(minute as u32, count.min(2)),
            b's' if flags & (TIME_NOSECONDS | TIME_NOMINUTESORSECONDS) == 0 => {
                number(second as u32, count.min(2))
            }
            b't' if flags & TIME_NOTIMEMARKER == 0 => match count {
                1 => marker.as_bytes()[..1].to_vec(),
                _ => marker.as_bytes().to_vec(),
            },
            _ => {
                out.truncate(last);
                skipping = true;
                Vec::new()
            }
        };
        out.extend(text);
        last = out.len();
    }
    out
}

/// The SYSTEMTIME at `at`, or the machine's time for NULL.
fn read_time(cpu: &mut Cpu, at: u32) -> Option<SystemTime> {
    if at == 0 {
        return Some(convert::system_time_of(cpu.bus.cmos.now()));
    }
    let st = std::array::from_fn(|i| cpu.bus.guest_read_16(at.wrapping_add(2 * i as u32)));
    (!cpu.bus.guest_faulted()).then_some(st)
}

/// The picture `at` points to, or None for NULL.
fn read_format(cpu: &mut Cpu, at: u32) -> Option<Option<Vec<u8>>> {
    match at {
        0 => Some(None),
        _ => Some(Some(read_string(cpu, at)?)),
    }
}

/// GetDateFormatA: the date `args[2]` (NULL for today) of the locale
/// `args[0]` in the picture `args[3]`, or the locale's short or long date
/// (DATE_SHORTDATE, DATE_LONGDATE), into `args[4]` (`args[5]` bytes).
pub(super) fn get_date_format_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[lcid, flags, time, format, to, size] = args else {
        unreachable!()
    };
    if bad_buffer(cpu, to, size) || !check_locale(cpu, lcid) {
        return Some(0);
    }
    let picture_flags =
        DATE_USE_ALT_CALENDAR | DATE_LTRREADING | DATE_RTLREADING | LOCALE_USE_CP_ACP;
    let allowed = match format {
        0 => picture_flags | DATE_SHORTDATE | DATE_LONGDATE | LOCALE_NOUSEROVERRIDE,
        _ => picture_flags,
    };
    let both = |pair: u32| flags & pair == pair;
    if flags & !allowed != 0
        || both(DATE_SHORTDATE | DATE_LONGDATE)
        || both(DATE_LTRREADING | DATE_RTLREADING)
    {
        set_error(cpu, ERROR_INVALID_FLAGS);
        return Some(0);
    }
    let st = read_time(cpu, time)?;
    let format = read_format(cpu, format)?;
    let date = [st[0], st[1], 0, st[3], 0, 0, 0, 0];
    let Some(st) = (st[1] <= 12 && st[3] <= 31)
        .then(|| convert::system_time_to_ticks(&date))
        .flatten()
        .and_then(convert::ticks_to_system_time)
    else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    let format = format.unwrap_or_else(|| {
        let kind = match flags & DATE_LONGDATE {
            0 => LOCALE_SSHORTDATE,
            _ => LOCALE_SLONGDATE,
        };
        info(kind).unwrap_or_default().into_bytes()
    });
    let text = format_date_time(&format, &st, flags, true);
    put_text(cpu, to, size, &text)
}

/// GetTimeFormatA: the time `args[2]` (NULL for now) in the picture
/// `args[3]` or the locale's, without minutes, seconds or the AM/PM
/// marker if the TIME_ flags say so, into `args[4]` (`args[5]` bytes).
pub(super) fn get_time_format_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[lcid, flags, time, format, to, size] = args else {
        unreachable!()
    };
    if bad_buffer(cpu, to, size) || !check_locale(cpu, lcid) {
        return Some(0);
    }
    let mut allowed = TIME_NOMINUTESORSECONDS
        | TIME_NOSECONDS
        | TIME_NOTIMEMARKER
        | TIME_FORCE24HOURFORMAT
        | LOCALE_USE_CP_ACP;
    if format == 0 {
        allowed |= LOCALE_NOUSEROVERRIDE;
    }
    if flags & !allowed != 0 {
        set_error(cpu, ERROR_INVALID_FLAGS);
        return Some(0);
    }
    let st = read_time(cpu, time)?;
    let format = read_format(cpu, format)?;
    if st[4] > 23 || st[5] > 59 || st[6] > 59 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let format =
        format.unwrap_or_else(|| info(LOCALE_STIMEFORMAT).unwrap_or_default().into_bytes());
    let text = format_date_time(&format, &st, flags, false);
    put_text(cpu, to, size, &text)
}

/// How a number is written: NUMBERFMT, and CURRENCYFMT's part of it.
struct NumberFormat {
    digits: u32,
    leading_zero: bool,
    /// The sizes of the groups of digits from the decimal point, the last
    /// repeating; none for no groups.
    grouping: Vec<usize>,
    decimal: Vec<u8>,
    thousand: Vec<u8>,
    negative_order: u32,
}

/// A number as GetNumberFormat takes it: an optional '-', digits and one
/// optional '.' with digits after it. Its sign, and its whole and
/// fractional digits.
fn parse_number(text: &[u8]) -> Option<(bool, Vec<u8>, Vec<u8>)> {
    let (negative, text) = match text.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, text),
    };
    let (whole, fraction) = match text.iter().position(|&b| b == b'.') {
        Some(at) => (&text[..at], &text[at + 1..]),
        None => (text, &text[..0]),
    };
    let digits = |s: &[u8]| s.iter().all(u8::is_ascii_digit);
    if !digits(whole) || !digits(fraction) || whole.len() + fraction.len() == 0 {
        return None;
    }
    Some((negative, whole.to_vec(), fraction.to_vec()))
}

/// The number's digits as `format` writes them, without its sign: rounded
/// to the format's digits after the point, its whole part in groups.
fn format_digits(whole: &[u8], fraction: &[u8], format: &NumberFormat) -> Vec<u8> {
    let digits = format.digits as usize;
    let mut whole: Vec<u8> = whole.iter().copied().skip_while(|&b| b == b'0').collect();
    let mut fraction = fraction.to_vec();
    let round_up = fraction.get(digits).is_some_and(|&b| b >= b'5');
    fraction.resize(digits, b'0');
    if round_up {
        let mut carry = true;
        for d in fraction.iter_mut().rev().chain(whole.iter_mut().rev()) {
            if *d == b'9' {
                *d = b'0';
            } else {
                *d += 1;
                carry = false;
                break;
            }
        }
        if carry {
            whole.insert(0, b'1');
        }
    }
    let mut out = Vec::new();
    if whole.is_empty() {
        if format.leading_zero || digits == 0 {
            out.push(b'0');
        }
    } else {
        let mut groups = Vec::new();
        let mut rest = &whole[..];
        let mut sizes = format.grouping.iter();
        let mut size = sizes.next().copied().unwrap_or(0);
        while size > 0 && rest.len() > size {
            groups.push(&rest[rest.len() - size..]);
            rest = &rest[..rest.len() - size];
            size = sizes.next().copied().unwrap_or(size);
        }
        groups.push(rest);
        for (i, group) in groups.iter().rev().enumerate() {
            if i > 0 {
                out.extend(&format.thousand);
            }
            out.extend(*group);
        }
    }
    if digits > 0 {
        out.extend(&format.decimal);
        out.extend(&fraction);
    }
    out
}

/// A locale's grouping ("3;0") or NUMBERFMT's (3, or 32 for "3;2;0"), as
/// group sizes.
fn grouping(value: u32) -> Vec<usize> {
    match value {
        0 => Vec::new(),
        32 => vec![3, 2],
        n => vec![n as usize],
    }
}

/// The locale's format of numbers, or of money (`money`).
fn locale_number_format(money: bool) -> NumberFormat {
    NumberFormat {
        digits: 2,
        leading_zero: true,
        grouping: grouping(3),
        decimal: b".".to_vec(),
        thousand: b",".to_vec(),
        negative_order: if money { 0 } else { 1 },
    }
}

/// The NUMBERFMTA or CURRENCYFMTA at `at`: the number's format, and for
/// money the positive order and the symbol. None if a page isn't there;
/// Err(()) if it has no separators or orders out of range.
#[allow(clippy::type_complexity)]
fn read_number_format(
    cpu: &mut Cpu,
    at: u32,
    money: bool,
) -> Option<Result<(NumberFormat, u32, Vec<u8>), ()>> {
    let words: Vec<u32> = (0..if money { 8 } else { 6 })
        .map(|i| cpu.bus.guest_read_32(at.wrapping_add(4 * i)))
        .collect();
    if cpu.bus.guest_faulted() {
        return None;
    }
    let max_negative = if money { 15 } else { 4 };
    if words[3] == 0 || words[4] == 0 || words[5] > max_negative {
        return Some(Err(()));
    }
    if money && (words[6] > 3 || words[7] == 0) {
        return Some(Err(()));
    }
    let format = NumberFormat {
        digits: words[0],
        leading_zero: words[1] != 0,
        grouping: grouping(words[2]),
        decimal: read_string(cpu, words[3])?,
        thousand: read_string(cpu, words[4])?,
        negative_order: words[5],
    };
    let symbol = if money {
        read_string(cpu, words[7])?
    } else {
        Vec::new()
    };
    Some(Ok((format, words.get(6).copied().unwrap_or(0), symbol)))
}

/// What GetNumberFormatA and GetCurrencyFormatA share: the checks and the
/// number, with its format, the positive order and the symbol.
#[allow(clippy::type_complexity)]
fn number_arguments(
    cpu: &mut Cpu,
    args: &[u32],
    money: bool,
) -> Option<Result<(bool, Vec<u8>, NumberFormat, u32, Vec<u8>), ()>> {
    let &[lcid, flags, value, format, to, size] = args else {
        unreachable!()
    };
    if value == 0 || bad_buffer(cpu, to, size) || !check_locale(cpu, lcid) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(Err(()));
    }
    let (format, positive, symbol) = if format == 0 {
        if flags & !(LOCALE_NOUSEROVERRIDE | LOCALE_USE_CP_ACP) != 0 {
            set_error(cpu, ERROR_INVALID_FLAGS);
            return Some(Err(()));
        }
        (locale_number_format(money), 0, b"$".to_vec())
    } else {
        if flags != 0 {
            set_error(cpu, ERROR_INVALID_FLAGS);
            return Some(Err(()));
        }
        match read_number_format(cpu, format, money)? {
            Ok(read) => read,
            Err(()) => {
                set_error(cpu, ERROR_INVALID_PARAMETER);
                return Some(Err(()));
            }
        }
    };
    let text = read_string(cpu, value)?;
    let Some((negative, whole, fraction)) = parse_number(&text) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(Err(()));
    };
    let digits = format_digits(&whole, &fraction, &format);
    Some(Ok((negative, digits, format, positive, symbol)))
}

/// GetNumberFormatA: the number in the string `args[2]` in the format
/// `args[3]` (NUMBERFMTA) or the locale's, into `args[4]` (`args[5]`
/// bytes).
pub(super) fn get_number_format_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok((negative, n, format, _, _)) = number_arguments(cpu, args, false)? else {
        return Some(0);
    };
    let text = if !negative {
        n
    } else {
        let parts: [&[u8]; 3] = match format.negative_order {
            0 => [b"(", &n, b")"],
            1 => [b"-", &n, b""],
            2 => [b"- ", &n, b""],
            3 => [b"", &n, b"-"],
            _ => [b"", &n, b" -"],
        };
        parts.concat()
    };
    put_text(cpu, args[4], args[5], &text)
}

/// GetCurrencyFormatA: as GetNumberFormatA, with the currency's symbol
/// placed as CURRENCYFMTA's orders, or the locale's, say.
pub(super) fn get_currency_format_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok((negative, n, format, positive, s)) = number_arguments(cpu, args, true)? else {
        return Some(0);
    };
    let (n, s) = (&n[..], &s[..]);
    let parts: Vec<&[u8]> = if !negative {
        match positive {
            0 => vec![s, n],
            1 => vec![n, s],
            2 => vec![s, b" ", n],
            _ => vec![n, b" ", s],
        }
    } else {
        match format.negative_order {
            0 => vec![b"(", s, n, b")"],
            1 => vec![b"-", s, n],
            2 => vec![s, b"-", n],
            3 => vec![s, n, b"-"],
            4 => vec![b"(", n, s, b")"],
            5 => vec![b"-", n, s],
            6 => vec![n, b"-", s],
            7 => vec![n, s, b"-"],
            8 => vec![b"-", n, b" ", s],
            9 => vec![b"-", s, b" ", n],
            10 => vec![n, b" ", s, b"-"],
            11 => vec![s, b" ", n, b"-"],
            12 => vec![s, b" -", n],
            13 => vec![n, b"- ", s],
            14 => vec![b"(", s, b" ", n, b")"],
            _ => vec![b"(", n, b" ", s, b")"],
        }
    };
    put_text(cpu, args[4], args[5], &parts.concat())
}
