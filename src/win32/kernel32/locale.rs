//! The locales: those of the built-in Windows (`nls::LOCALES`), all of
//! them installed as on a Windows 98 with multilanguage support, and the
//! system locale (`win32_locale`) the defaults stand for. Their
//! information (GetLocaleInfo), the formats of dates, times, numbers and
//! money made with it, and the enumerations of locales and formats, which
//! call the program back. The A functions give text in the locale's ANSI
//! code page, or with LOCALE_USE_CP_ACP the system's.

use super::text::{ERROR_INVALID_FLAGS, put_units, read_string};
use super::{ERROR_INVALID_PARAMETER, FALSE, TRUE, set_error};
use crate::cpu::Cpu;
use crate::win32::call::{self, Call};
use crate::win32::convert::{self, SystemTime};
use crate::win32::nls::{self, CodePage, Locale};

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

/// The information of the kind `lctype` of the locale `l`, as Windows 98
/// has it.
fn info(l: &Locale, lctype: u32) -> Option<String> {
    let i = l.info;
    let flag = |yes: bool| if yes { "1" } else { "0" };
    let text = match lctype {
        0x01 | 0x09 => return Some(format!("{:04X}", l.lang_id())),
        0x02 => i.full_language,
        0x1001 => i.language,
        0x03 => i.abbrev_language,
        0x04 => i.native_language,
        0x05 | 0x0A => return Some(l.country.to_string()),
        0x06 | 0x1002 => i.country,
        0x07 => i.abbrev_country,
        0x08 => i.native_country,
        0x0B => return Some(l.oem.to_string()),
        0x0C => i.list,
        0x0D => i.measure,
        0x0E => i.decimal,
        0x0F => i.thousand,
        0x10 | 0x18 => i.grouping,
        0x11 | 0x19 | 0x1A => "2",
        0x12 => "1",
        0x13 => "0123456789",
        0x14 => i.currency,
        0x15 => i.intl_symbol,
        0x16 => i.money_decimal,
        0x17 => i.money_thousand,
        0x1B => i.currency_order,
        0x1C => i.negative_currency,
        0x1D => i.date_separator,
        0x1E => i.time_separator,
        LOCALE_SSHORTDATE => i.short_date,
        LOCALE_SLONGDATE => i.long_date,
        LOCALE_IDATE | LOCALE_ILDATE => i.date_order,
        0x23 => i.hours24,
        // ICENTURY, ITLZERO, IDAYLZERO and IMONLZERO: what the short date
        // and the time formats have.
        0x24 => flag(i.short_date.contains("yyyy")),
        0x25 => flag(i.time_format.starts_with("hh") || i.time_format.starts_with("HH")),
        0x26 => flag(i.short_date.contains("dd")),
        0x27 => flag(i.short_date.contains("MM")),
        LOCALE_S1159 => i.am,
        LOCALE_S2359 => i.pm,
        0x2A..=0x30 => i.days[(lctype - LOCALE_SDAYNAME1) as usize],
        0x31..=0x37 => i.abbrev_days[(lctype - LOCALE_SABBREVDAYNAME1) as usize],
        0x38..=0x43 => i.months[(lctype - LOCALE_SMONTHNAME1) as usize],
        0x44..=0x4F => i.abbrev_months[(lctype - LOCALE_SABBREVMONTHNAME1) as usize],
        0x50 => "",
        0x51 => "-",
        0x52 => "3",
        0x53 | 0x55 | 0x57 => "0",
        0x54 | 0x56 => "1",
        0x59 => i.iso_language,
        0x5A => i.iso_country,
        LOCALE_STIMEFORMAT => i.time_format,
        0x1004 => return Some(l.ansi.to_string()),
        0x1005 => "0",
        0x1009 => "1",
        0x100B => "0",
        0x100C => i.first_day,
        0x100D => "0",
        0x100E | 0x100F => "",
        0x1010 => i.negative_number,
        0x1011 => return Some(l.mac.to_string()),
        _ => return None,
    };
    Some(text.to_string())
}

/// The locale a function's `lcid` names, or None, with the error set, for
/// one there isn't.
fn check_locale(cpu: &mut Cpu, lcid: u32) -> Option<&'static Locale> {
    let l = nls::locale_of(cpu, lcid);
    if l.is_none() {
        set_error(cpu, ERROR_INVALID_PARAMETER);
    }
    l
}

/// The code page an A function of the locale `l` writes its text in: the
/// locale's, or the system's with LOCALE_USE_CP_ACP.
fn page_of(cpu: &Cpu, l: &Locale, flags: u32) -> CodePage {
    if flags & LOCALE_USE_CP_ACP != 0 {
        nls::ansi(cpu)
    } else {
        l.ansi()
    }
}

/// `text` in the code page `page`.
fn encode(page: CodePage, text: &str) -> Vec<u8> {
    page.encode(&text.encode_utf16().collect::<Vec<_>>())
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
    let Some(l) = check_locale(cpu, lcid) else {
        return Some(0);
    };
    let Some(text) = info(l, lctype & !(LOCALE_NOUSEROVERRIDE | LOCALE_USE_CP_ACP)) else {
        set_error(cpu, ERROR_INVALID_FLAGS);
        return Some(0);
    };
    let text = encode(page_of(cpu, l, lctype), &text);
    put_text(cpu, to, size, &text)
}

/// SetLocaleInfoA: Windows takes changes of the user's formats here; this
/// takes them (TRUE) without keeping them, and refuses the kinds that
/// can't be changed, as Windows does.
pub(super) fn set_locale_info_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[lcid, lctype, data] = args else {
        unreachable!()
    };
    if data == 0 || nls::locale_of(cpu, lcid).is_none() {
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

/// GetUserDefaultLCID, GetSystemDefaultLCID and GetThreadLocale: the
/// system locale, which is the user's too.
pub(super) fn get_default_lcid(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(nls::current(cpu).lcid)
}

/// GetUserDefaultLangID and GetSystemDefaultLangID.
pub(super) fn get_default_lang_id(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(nls::current(cpu).lang_id())
}

/// SetThreadLocale: TRUE for a locale there is. The thread goes on in the
/// system locale, which GetThreadLocale gives.
pub(super) fn set_thread_locale(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(check_locale(cpu, args[0]).is_some() as u32)
}

/// IsValidLocale: the locales there are, all installed and supported.
pub(super) fn is_valid_locale(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[lcid, flags] = args else { unreachable!() };
    Some((flags <= 2 && nls::by_lcid(lcid).is_some()) as u32)
}

pub(super) fn convert_default_locale(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(nls::convert_default(cpu, args[0]))
}

/// What the enumerations below list.
const LOCALES: u32 = 0;
const SHORT_DATES: u32 = 1;
const LONG_DATES: u32 = 2;
const TIMES: u32 = 3;

/// The strings the enumeration `kind` of the locale `lcid` gives, in
/// order: the locale's default first. The US locale has the formats
/// Windows 95 lists for it, the others their own.
fn enumerated(kind: u32, lcid: u32) -> Vec<String> {
    let l = nls::by_lcid(lcid).unwrap_or(&nls::LOCALES[0]);
    let us = |formats: &[&str]| formats.iter().map(|f| f.to_string()).collect();
    match kind {
        LOCALES => nls::LOCALES
            .iter()
            .map(|l| format!("{:08X}", l.lcid))
            .collect(),
        _ if l.lcid != 0x0409 => vec![
            match kind {
                SHORT_DATES => l.info.short_date,
                LONG_DATES => l.info.long_date,
                _ => l.info.time_format,
            }
            .to_string(),
        ],
        SHORT_DATES => us(&[
            "M/d/yy",
            "M/d/yyyy",
            "MM/dd/yy",
            "MM/dd/yyyy",
            "yy/MM/dd",
            "dd-MMM-yy",
        ]),
        LONG_DATES => us(&[
            "dddd, MMMM dd, yyyy",
            "MMMM dd, yyyy",
            "dddd, dd MMMM, yyyy",
            "dd MMMM, yyyy",
        ]),
        _ => us(&["h:mm:ss tt", "hh:mm:ss tt", "H:mm:ss", "HH:mm:ss"]),
    }
}

/// Call the enumeration's callback `data[0]` with its `data[2]`th string
/// of the locale `data[3]`, in the code page `data[4]`, which goes on the
/// stack below the stub's: false if the stack isn't there.
fn call_with_string(cpu: &mut Cpu, resume: (u32, u32), data: Vec<u32>) -> bool {
    let text = &enumerated(data[1], data[3])[data[2] as usize];
    let page = CodePage::by_number(data[4]).unwrap_or(CodePage::WESTERN);
    let mut bytes = encode(page, text);
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

/// Start the enumeration `kind` of the locale `l` with the callback
/// `callback`, its strings in the code page `page`: no value now (None)
/// while the callback runs, the result when it has run.
fn enumerate(cpu: &mut Cpu, callback: u32, kind: u32, l: &Locale, page: CodePage) -> Option<u32> {
    let resume = call::here(cpu);
    let data = vec![callback, kind, 0, l.lcid, page.number()];
    if !call_with_string(cpu, resume, data) {
        return Some(FALSE);
    }
    None
}

/// An enumeration's callback returned: on to the next string while it
/// returns TRUE and there are more, else the enumeration returns TRUE.
pub(in crate::win32) fn enum_returned(cpu: &mut Cpu, value: u32, call: Call) {
    let mut data = call.data;
    data[2] += 1;
    let more = value != 0 && (data[2] as usize) < enumerated(data[1], data[3]).len();
    if !more || !call_with_string(cpu, call.resume, data) {
        call::finish(cpu, call.resume, TRUE);
    }
}

/// EnumSystemLocalesA: the locales there are, "00000409" first.
pub(super) fn enum_system_locales_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let us = &nls::LOCALES[0];
    enumerate(cpu, args[0], LOCALES, us, us.ansi())
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
    let l = if callback == 0 { None } else { check_locale(cpu, lcid) };
    let Some(l) = l else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    };
    let kind = match flags & !LOCALE_USE_CP_ACP {
        0 | DATE_SHORTDATE => SHORT_DATES,
        DATE_LONGDATE => LONG_DATES,
        _ => {
            set_error(cpu, ERROR_INVALID_FLAGS);
            return Some(FALSE);
        }
    };
    let page = page_of(cpu, l, flags);
    enumerate(cpu, callback, kind, l, page)
}

pub(super) fn enum_time_formats_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[callback, lcid, flags] = args else {
        unreachable!()
    };
    let l = if callback == 0 { None } else { check_locale(cpu, lcid) };
    let Some(l) = l else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    };
    if flags & !LOCALE_USE_CP_ACP != 0 {
        set_error(cpu, ERROR_INVALID_FLAGS);
        return Some(FALSE);
    }
    let page = page_of(cpu, l, flags);
    enumerate(cpu, callback, TIMES, l, page)
}

const TIME_NOMINUTESORSECONDS: u32 = 0x1;
const TIME_NOSECONDS: u32 = 0x2;
const TIME_NOTIMEMARKER: u32 = 0x4;
const TIME_FORCE24HOURFORMAT: u32 = 0x8;

/// `value` with at least `digits` digits.
fn number(value: u32, digits: usize) -> Vec<u16> {
    wide(&format!("{:0digits$}", value))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// A date or a time (`date`) in the picture `format`, as GetDateFormat
/// and GetTimeFormat make it. Text in quotes is copied; with the
/// TIME_NO... flags a left-out part takes the text before it with it, up
/// to the part before, and the text after it up to the next part, but
/// for spaces. The names are the locale `l`'s.
fn format_date_time(
    l: &Locale,
    format: &[u16],
    st: &SystemTime,
    flags: u32,
    date: bool,
) -> Vec<u16> {
    let names = l.info;
    let &[year, month, weekday, day, hour, minute, second, _] = st;
    let mut out = Vec::new();
    let mut last = 0;
    let mut skipping = false;
    let mut i = 0;
    while i < format.len() {
        let c = format[i];
        if c == QUOTE {
            i += 1;
            while i < format.len() {
                if format[i] == QUOTE {
                    i += 1;
                    if format.get(i) != Some(&QUOTE) {
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
        if c >= 0x80 || !pictures.contains(&(c as u8)) {
            if !skipping || c == b' ' as u16 {
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
        let marker = wide(if hour < 12 { names.am } else { names.pm });
        let day_index = (weekday as usize + 6) % 7;
        let month_index = month as usize - 1;
        let text = match c as u8 {
            b'd' if count >= 4 => wide(names.days[day_index]),
            b'd' if count == 3 => wide(names.abbrev_days[day_index]),
            b'd' => number(day as u32, count),
            b'M' if count >= 4 => wide(names.months[month_index]),
            b'M' if count == 3 => wide(names.abbrev_months[month_index]),
            b'M' => number(month as u32, count),
            b'y' if count >= 4 => number(year as u32, 4),
            b'y' => number(year as u32 % 100, count.min(2)),
            b'g' if count == 2 => wide("AD"),
            b'g' => Vec::new(),
            b'h' if flags & TIME_FORCE24HOURFORMAT == 0 => number(hour12 as u32, count.min(2)),
            b'h' | b'H' => number(hour as u32, count.min(2)),
            b'm' if flags & TIME_NOMINUTESORSECONDS == 0 => number(minute as u32, count.min(2)),
            b's' if flags & (TIME_NOSECONDS | TIME_NOMINUTESORSECONDS) == 0 => {
                number(second as u32, count.min(2))
            }
            b't' if flags & TIME_NOTIMEMARKER == 0 => match count {
                1 => marker.into_iter().take(1).collect(),
                _ => marker,
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

const QUOTE: u16 = b'\'' as u16;

/// The SYSTEMTIME at `at`, or the system's time for NULL. Reading it
/// doesn't make GetDateFormat or GetTimeFormat a service an idle proof
/// can follow: they may set the last error after it (`idle::nested`).
fn read_time(cpu: &mut Cpu, at: u32) -> Option<SystemTime> {
    if at == 0 {
        let now = crate::idle::nested(cpu, super::time::now);
        return Some(convert::system_time_of(now));
    }
    let st = std::array::from_fn(|i| cpu.bus.guest_read_16(at.wrapping_add(2 * i as u32)));
    (!cpu.bus.guest_faulted()).then_some(st)
}

/// The picture `at` points to in the code page `page`, as Unicode, or
/// None for NULL.
fn read_format(cpu: &mut Cpu, at: u32, page: CodePage) -> Option<Option<Vec<u16>>> {
    match at {
        0 => Some(None),
        _ => Some(Some(page.decode(&read_string(cpu, at)?))),
    }
}

/// GetDateFormatA: the date `args[2]` (NULL for today) of the locale
/// `args[0]` in the picture `args[3]`, or the locale's short or long date
/// (DATE_SHORTDATE, DATE_LONGDATE), into `args[4]` (`args[5]` bytes).
pub(super) fn get_date_format_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[lcid, flags, time, format, to, size] = args else {
        unreachable!()
    };
    if bad_buffer(cpu, to, size) {
        return Some(0);
    }
    let Some(l) = check_locale(cpu, lcid) else {
        return Some(0);
    };
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
    let page = page_of(cpu, l, flags);
    let st = read_time(cpu, time)?;
    let format = read_format(cpu, format, page)?;
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
        wide(&info(l, kind).unwrap_or_default())
    });
    let text = format_date_time(l, &format, &st, flags, true);
    put_text(cpu, to, size, &page.encode(&text))
}

/// GetTimeFormatA: the time `args[2]` (NULL for now) in the picture
/// `args[3]` or the locale's, without minutes, seconds or the AM/PM
/// marker if the TIME_ flags say so, into `args[4]` (`args[5]` bytes).
pub(super) fn get_time_format_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[lcid, flags, time, format, to, size] = args else {
        unreachable!()
    };
    if bad_buffer(cpu, to, size) {
        return Some(0);
    }
    let Some(l) = check_locale(cpu, lcid) else {
        return Some(0);
    };
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
    let page = page_of(cpu, l, flags);
    let st = read_time(cpu, time)?;
    let format = read_format(cpu, format, page)?;
    if st[4] > 23 || st[5] > 59 || st[6] > 59 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let format = format.unwrap_or_else(|| wide(l.info.time_format));
    let text = format_date_time(l, &format, &st, flags, false);
    put_text(cpu, to, size, &page.encode(&text))
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

/// A locale's grouping as group sizes: "3;0" is groups of 3, the last
/// size repeating before a 0.
fn grouping_of(text: &str) -> Vec<usize> {
    let mut sizes: Vec<usize> = text.split(';').filter_map(|s| s.parse().ok()).collect();
    if sizes.last() == Some(&0) {
        sizes.pop();
    }
    sizes
}

/// The locale `l`'s format of numbers, or of money (`money`), its
/// separators in the code page `page`.
fn locale_number_format(l: &Locale, page: CodePage, money: bool) -> NumberFormat {
    let i = l.info;
    let (decimal, thousand, negative) = if money {
        (i.money_decimal, i.money_thousand, i.negative_currency)
    } else {
        (i.decimal, i.thousand, i.negative_number)
    };
    NumberFormat {
        digits: 2,
        leading_zero: true,
        grouping: grouping_of(i.grouping),
        decimal: encode(page, decimal),
        thousand: encode(page, thousand),
        negative_order: negative.parse().unwrap_or(0),
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
    let l = if value == 0 || bad_buffer(cpu, to, size) {
        None
    } else {
        check_locale(cpu, lcid)
    };
    let Some(l) = l else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(Err(()));
    };
    let (format, positive, symbol) = if format == 0 {
        if flags & !(LOCALE_NOUSEROVERRIDE | LOCALE_USE_CP_ACP) != 0 {
            set_error(cpu, ERROR_INVALID_FLAGS);
            return Some(Err(()));
        }
        let page = page_of(cpu, l, flags);
        let positive = l.info.currency_order.parse().unwrap_or(0);
        let symbol = encode(page, l.info.currency);
        (locale_number_format(l, page, money), positive, symbol)
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
