//! Time: `time`, `clock`, the `struct tm` conversions, `strftime` and the
//! date and time strings. There are no time zones, as in KERNEL32's
//! time.rs: local time is the machine's clock, and UTC is the same.

use super::*;

/// The time_t of the DOS date and time `date` and `time`.
pub fn dos_to_time_t(date: u16, time: u16) -> u32 {
    let year = 1980 + (date >> 9) as i64;
    let month = ((date >> 5) & 15).clamp(1, 12) as i64;
    let day = (date & 31).max(1) as i64;
    let secs = (time >> 11) as i64 * 3600 + ((time >> 5) & 63) as i64 * 60 + (time & 31) as i64 * 2;
    (days_from_civil(year, month, day) * 86400 + secs) as u32
}

/// Days since 1970-01-01 of a date in the proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// The date of a day since 1970-01-01: year, month (1 to 12), day.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A `struct tm`: seconds, minutes, hours, day of the month, month (0 to
/// 11), years since 1900, day of the week and of the year, DST flag.
type Tm = [i32; 9];

/// The `struct tm` of `t` seconds since 1970.
fn tm_of(t: i64) -> Tm {
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let yday = days - days_from_civil(y, 1, 1);
    [
        (secs % 60) as i32,
        (secs / 60 % 60) as i32,
        (secs / 3600) as i32,
        d as i32,
        m as i32 - 1,
        (y - 1900) as i32,
        (days + 4).rem_euclid(7) as i32,
        yday as i32,
        0,
    ]
}

/// The seconds since 1970 a `struct tm` says, its fields out of range or
/// not, and the fields made right.
fn normalize(tm: &Tm) -> (i64, Tm) {
    let mut year = tm[5] as i64 + 1900;
    let mut month = tm[4] as i64;
    year += month.div_euclid(12);
    month = month.rem_euclid(12);
    let days = days_from_civil(year, month + 1, 1) + tm[3] as i64 - 1;
    let t = days * 86400 + tm[2] as i64 * 3600 + tm[1] as i64 * 60 + tm[0] as i64;
    (t, tm_of(t))
}

fn read_tm(cpu: &mut Cpu, at: u32) -> Option<Tm> {
    let mut tm = [0; 9];
    for (i, field) in tm.iter_mut().enumerate() {
        *field = read32(cpu, at + 4 * i as u32)? as i32;
    }
    Some(tm)
}

fn write_tm(cpu: &mut Cpu, at: u32, tm: &Tm) -> Option<()> {
    let data: Vec<u8> = tm.iter().flat_map(|f| f.to_le_bytes()).collect();
    put(cpu, at, &data)
}

/// Seconds since 1970 by the machine's clock.
fn now(cpu: &Cpu) -> i64 {
    let at = cpu.bus.cmos.now();
    at.and_utc().timestamp()
}

/// time(t): the time, also in `*t` when t isn't NULL.
pub fn time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let t = now(cpu) as u32;
    if args[0] != 0 {
        write32(cpu, args[0], t)?;
    }
    Some(t)
}

/// clock(): milliseconds since the process started.
pub fn clock(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let start = state(cpu, S_START);
    Some(((cpu.bus.clock.now_micros() / 1000) as u32).wrapping_sub(start))
}

/// The thread's `struct tm` for the time at `*at`: None, NULL returned,
/// for one before 1970 (a negative time_t).
fn convert(cpu: &mut Cpu, at: u32) -> Option<u32> {
    let t = read32(cpu, at)? as i32;
    if t < 0 {
        return Some(0);
    }
    let tm = tm_of(t as i64);
    let block = thread_field(cpu, T_TM)?;
    write_tm(cpu, block, &tm)?;
    Some(block)
}

/// localtime(t) and gmtime(t): the same, with no time zones.
pub fn localtime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    convert(cpu, args[0])
}

/// mktime(tm): the time `*tm` says, the struct made right; -1 for one
/// before 1970 or past 2038.
pub fn mktime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let tm = read_tm(cpu, args[0])?;
    let (t, fixed) = normalize(&tm);
    if !(0..=i32::MAX as i64).contains(&t) {
        return Some(u32::MAX);
    }
    write_tm(cpu, args[0], &fixed)?;
    Some(t as u32)
}

/// difftime(t1, t0): t1 - t0, as a double.
pub fn difftime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    ret_f64(cpu, args[0] as i32 as f64 - args[1] as i32 as f64)
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const LONG_DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const LONG_MONTHS: [&str; 12] = [
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

/// "Wed Jan 02 02:03:55 1980\n", as asctime writes it.
fn asctime_text(tm: &Tm) -> String {
    format!(
        "{} {} {:02} {:02}:{:02}:{:02} {}\n",
        DAYS[tm[6].rem_euclid(7) as usize],
        MONTHS[tm[4].rem_euclid(12) as usize],
        tm[3],
        tm[2],
        tm[1],
        tm[0],
        tm[5] + 1900
    )
}

/// asctime(tm): the thread's buffer.
pub fn asctime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let tm = read_tm(cpu, args[0])?;
    let at = thread_field(cpu, T_ASCTIME)?;
    put_cstr(cpu, at, asctime_text(&tm).as_bytes())?;
    Some(at)
}

/// ctime(t): asctime(localtime(t)); NULL for a time before 1970.
pub fn ctime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let tm = convert(cpu, args[0])?;
    if tm == 0 {
        return Some(0);
    }
    asctime(cpu, &[tm])
}

/// _wasctime(tm) and _wctime(t): the same, as wide strings.
pub fn wasctime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let tm = read_tm(cpu, args[0])?;
    let at = thread_field(cpu, T_WASCTIME)?;
    let text: Vec<u16> = asctime_text(&tm).bytes().map(u16::from).collect();
    put_wstr(cpu, at, &text)?;
    Some(at)
}

pub fn wctime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let tm = convert(cpu, args[0])?;
    if tm == 0 {
        return Some(0);
    }
    wasctime(cpu, &[tm])
}

/// `strftime`'s text for the format `format` and the time `tm`.
fn strftime_text(format: &[u8], tm: &Tm) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    let wday = tm[6].rem_euclid(7) as usize;
    let mon = tm[4].rem_euclid(12) as usize;
    let year = tm[5] as i64 + 1900;
    while i < format.len() {
        if format[i] != b'%' || i + 1 == format.len() {
            out.push(format[i]);
            i += 1;
            continue;
        }
        i += 1;
        // '#' takes the zeros off numbers and makes %c and %x long.
        let alt = format[i] == b'#';
        if alt {
            i += 1;
            if i == format.len() {
                break;
            }
        }
        let num = |n: i64, width: usize| {
            if alt {
                n.to_string()
            } else {
                format!("{:0width$}", n, width = width)
            }
        };
        let hour12 = if tm[2] % 12 == 0 { 12 } else { tm[2] % 12 } as i64;
        let week = |first: i32| (tm[7] + 7 - ((tm[6] - first).rem_euclid(7))) / 7;
        let text = match format[i] {
            b'a' => DAYS[wday].to_string(),
            b'A' => LONG_DAYS[wday].to_string(),
            b'b' => MONTHS[mon].to_string(),
            b'B' => LONG_MONTHS[mon].to_string(),
            b'c' if alt => format!(
                "{}, {} {}, {} {:02}:{:02}:{:02}",
                LONG_DAYS[wday], LONG_MONTHS[mon], tm[3], year, tm[2], tm[1], tm[0]
            ),
            b'c' => format!(
                "{:02}/{:02}/{:02} {:02}:{:02}:{:02}",
                mon + 1,
                tm[3],
                year % 100,
                tm[2],
                tm[1],
                tm[0]
            ),
            b'd' => num(tm[3] as i64, 2),
            b'H' => num(tm[2] as i64, 2),
            b'I' => num(hour12, 2),
            b'j' => num(tm[7] as i64 + 1, 3),
            b'm' => num(mon as i64 + 1, 2),
            b'M' => num(tm[1] as i64, 2),
            b'p' => if tm[2] < 12 { "AM" } else { "PM" }.to_string(),
            b'S' => num(tm[0] as i64, 2),
            b'U' => num(week(0) as i64, 2),
            b'w' => wday.to_string(),
            b'W' => num(week(1) as i64, 2),
            b'x' if alt => format!(
                "{}, {} {}, {}",
                LONG_DAYS[wday], LONG_MONTHS[mon], tm[3], year
            ),
            b'x' => format!("{:02}/{:02}/{:02}", mon + 1, tm[3], year % 100),
            b'X' => format!("{:02}:{:02}:{:02}", tm[2], tm[1], tm[0]),
            b'y' => num(year % 100, 2),
            b'Y' => num(year, 1),
            b'z' | b'Z' => "GMT".to_string(),
            b'%' => "%".to_string(),
            _ => String::new(),
        };
        out.extend_from_slice(text.as_bytes());
        i += 1;
    }
    out
}

/// strftime(buffer, size, format, tm): the length written, or 0 when it
/// doesn't fit.
pub fn strftime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[buffer, size, format, at] = args else {
        unreachable!()
    };
    let format = cstr(cpu, format)?;
    let tm = read_tm(cpu, at)?;
    let text = strftime_text(&format, &tm);
    if text.len() as u32 >= size {
        if size > 0 {
            put(cpu, buffer, &[0])?;
        }
        return Some(0);
    }
    put_cstr(cpu, buffer, &text)?;
    Some(text.len() as u32)
}

/// wcsftime(buffer, size, format, tm), with wide strings.
pub fn wcsftime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[buffer, size, format, at] = args else {
        unreachable!()
    };
    let format: Vec<u8> = wstr(cpu, format)?
        .into_iter()
        .map(|c| narrow(c).unwrap_or(b'?'))
        .collect();
    let tm = read_tm(cpu, at)?;
    let text: Vec<u16> = strftime_text(&format, &tm).into_iter().map(widen).collect();
    if text.len() as u32 >= size {
        return Some(0);
    }
    put_wstr(cpu, buffer, &text)?;
    Some(text.len() as u32)
}

/// _ftime(timeb): the time, its milliseconds, and no time zone.
pub fn ftime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let t = now(cpu) as u32;
    let millis = (cpu.bus.cmos.now().and_utc().timestamp_subsec_millis()) as u16;
    let mut out = [0u8; 12];
    out[0..4].copy_from_slice(&t.to_le_bytes());
    out[4..6].copy_from_slice(&millis.to_le_bytes());
    put(cpu, args[0], &out)?;
    Some(0)
}

/// The date and time now as `struct tm`.
fn now_tm(cpu: &Cpu) -> Tm {
    tm_of(now(cpu))
}

/// _strdate(buffer): "MM/DD/YY".
pub fn strdate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let tm = now_tm(cpu);
    let text = format!("{:02}/{:02}/{:02}", tm[4] + 1, tm[3], tm[5] % 100);
    put_cstr(cpu, args[0], text.as_bytes())?;
    Some(args[0])
}

/// _strtime(buffer): "HH:MM:SS".
pub fn strtime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let tm = now_tm(cpu);
    let text = format!("{:02}:{:02}:{:02}", tm[2], tm[1], tm[0]);
    put_cstr(cpu, args[0], text.as_bytes())?;
    Some(args[0])
}

pub fn wstrdate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let tm = now_tm(cpu);
    let text = format!("{:02}/{:02}/{:02}", tm[4] + 1, tm[3], tm[5] % 100);
    put_wstr(
        cpu,
        args[0],
        &text.bytes().map(u16::from).collect::<Vec<_>>(),
    )?;
    Some(args[0])
}

pub fn wstrtime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let tm = now_tm(cpu);
    let text = format!("{:02}:{:02}:{:02}", tm[2], tm[1], tm[0]);
    put_wstr(
        cpu,
        args[0],
        &text.bytes().map(u16::from).collect::<Vec<_>>(),
    )?;
    Some(args[0])
}

/// _tzset(): there are no time zones to read.
pub fn tzset(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}
