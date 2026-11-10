//! strftime and the names it takes, as Visual C++ 6's MSVCRT.DLL has them:
//! `strftime`, `_Strftime`, `wcsftime`, and `_Getdays`, `_Getmonths` and
//! `_Gettnames`, which give MSVCP60 the locale's names.
//!
//! A locale's names are a `__lc_time_data`: 43 pointers to strings, the
//! days' short and long names, the months' short and long names, "AM" and
//! "PM", then the short date, long date and time formats in Windows'
//! pictures ("M/d/yy"). The "C" locale's is in the DLL's data section,
//! its strings after it. `_Strftime` takes one of a program's own, as
//! MSVCP60 passes them. strftime reads the names and writes its text in
//! the program's memory as MSVCRT does, a byte at a time, so what a short
//! buffer or a format that runs past a name's end gets is what it gets
//! on Windows; strftime_vc6.txt has what MSVCRT 6 gave for made-up
//! formats and names.

use super::time::{DAYS, LONG_DAYS, LONG_MONTHS, MONTHS, Tm, read_tm};
use super::*;

/// The memory strftime reads and writes: the program's, or a test's.
pub(super) trait Guest {
    /// The byte at `at`, None where there is no memory.
    fn read8(&mut self, at: u32) -> Option<u8>;
    fn write8(&mut self, at: u32, byte: u8);
}

impl Guest for Cpu {
    fn read8(&mut self, at: u32) -> Option<u8> {
        let byte = self.bus.guest_read_8(at);
        (!self.bus.guest_faulted()).then_some(byte)
    }

    fn write8(&mut self, at: u32, byte: u8) {
        self.bus.guest_write_8(at, byte);
    }
}

/// The 32-bit word at `at`.
fn word(m: &mut impl Guest, at: u32) -> Option<u32> {
    let mut bytes = [0; 4];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = m.read8(at.wrapping_add(i as u32))?;
    }
    Some(u32::from_le_bytes(bytes))
}

/// The string at `at`, without its 0.
fn string(m: &mut impl Guest, at: u32) -> Option<Vec<u8>> {
    let mut text = Vec::new();
    loop {
        match m.read8(at.wrapping_add(text.len() as u32))? {
            0 => return Some(text),
            byte => text.push(byte),
        }
    }
}

/// `__lc_time_data`'s fields, by their place.
const LC_WDAY_ABBR: u32 = 0;
const LC_WDAY: u32 = 7;
const LC_MONTH_ABBR: u32 = 14;
const LC_MONTH: u32 = 26;
const LC_AMPM: u32 = 38;
const LC_SDATE: u32 = 40;
const LC_LDATE: u32 = 41;
const LC_TIME: u32 = 42;
const LC_FIELDS: u32 = 43;
/// The struct's size, and the room for the "C" locale's with its
/// strings.
const LC_TIME_SIZE: u32 = 4 * LC_FIELDS;
pub const LC_TIME_C_SIZE: u32 = 0x200;

/// The "C" locale's names and formats, in the order of the fields.
fn c_locale_names() -> Vec<&'static str> {
    let mut names = Vec::new();
    names.extend(DAYS);
    names.extend(LONG_DAYS);
    names.extend(MONTHS);
    names.extend(LONG_MONTHS);
    names.extend(["AM", "PM", "M/d/yy", "dddd, MMMM dd, yyyy", "H:mm:ss"]);
    names
}

/// The "C" locale's `__lc_time_data` for the address `at`: the pointers,
/// then the strings they point to.
pub fn lc_time_c(at: u32) -> Vec<u8> {
    let names = c_locale_names();
    let mut strings = Vec::new();
    let mut data = Vec::new();
    for name in names {
        let pointer = at + LC_TIME_SIZE + strings.len() as u32;
        data.extend(pointer.to_le_bytes());
        strings.extend(name.as_bytes());
        strings.push(0);
    }
    data.extend(strings);
    data
}

/// The `__lc_time_data` a function was given, the "C" locale's for NULL.
fn lc_time(lc: u32) -> u32 {
    if lc == 0 { var_at(V_LC_TIME_C) } else { lc }
}

/// The string of field `field` of the `__lc_time_data` at `lc`.
fn lc_name(m: &mut impl Guest, lc: u32, field: u32) -> Option<Vec<u8>> {
    let at = word(m, lc.wrapping_add(4 * field))?;
    string(m, at)
}

/// The current locale's names of `count` days or months, from the fields
/// `abbr` and `full` on: for each the short name and the long one, each
/// after a colon, in a block from malloc. NULL when there is no memory
/// for it.
fn names_block(cpu: &mut Cpu, abbr: u32, full: u32, count: u32) -> Option<u32> {
    let lc = lc_time(0);
    let mut text = Vec::new();
    for i in 0..count {
        text.push(b':');
        text.extend(lc_name(cpu, lc, abbr + i)?);
        text.push(b':');
        text.extend(lc_name(cpu, lc, full + i)?);
    }
    let Some(at) = heap_alloc(cpu, text.len() as u32 + 1) else {
        return Some(0);
    };
    put_cstr(cpu, at, &text)?;
    Some(at)
}

/// _Getdays(): ":Sun:Sunday:Mon:Monday:...:Sat:Saturday", malloc'ed.
pub fn getdays(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    names_block(cpu, LC_WDAY_ABBR, LC_WDAY, 7)
}

/// _Getmonths(): ":Jan:January:...:Dec:December", malloc'ed.
pub fn getmonths(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    names_block(cpu, LC_MONTH_ABBR, LC_MONTH, 12)
}

/// _Gettnames(): a copy of the current locale's `__lc_time_data` in one
/// block from malloc, its strings after it: each day's short and long
/// name, then each month's, then the rest in the struct's order. NULL
/// when there is no memory for it. MSVCRT 6 leaves out the time format:
/// the block has room for it, and its pointer points there, but the
/// string isn't copied, so it holds what the heap had there.
pub fn gettnames(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let lc = lc_time(0);
    let mut order = Vec::new();
    for i in 0..7 {
        order.extend([LC_WDAY_ABBR + i, LC_WDAY + i]);
    }
    for i in 0..12 {
        order.extend([LC_MONTH_ABBR + i, LC_MONTH + i]);
    }
    order.extend(LC_AMPM..LC_FIELDS);
    let mut strings = Vec::new();
    for &field in &order {
        strings.push((field, lc_name(cpu, lc, field)?));
    }
    let size = LC_TIME_SIZE + strings.iter().map(|(_, s)| s.len() as u32 + 1).sum::<u32>();
    let Some(at) = heap_alloc(cpu, size) else {
        return Some(0);
    };
    let mut next = at + LC_TIME_SIZE;
    for (field, text) in strings {
        write32(cpu, at + 4 * field, next)?;
        if field != LC_TIME {
            put_cstr(cpu, next, &text)?;
        }
        next += text.len() as u32 + 1;
    }
    Some(at)
}

/// Where strftime writes: the next place in the buffer, and the room left
/// there. Writes go straight to the program's memory, as MSVCRT's do, so
/// what a short buffer gets is what it gets on Windows.
struct Out {
    at: u32,
    left: u32,
}

impl Out {
    fn put(&mut self, m: &mut impl Guest, byte: u8) {
        m.write8(self.at, byte);
        self.at = self.at.wrapping_add(1);
        self.left = self.left.wrapping_sub(1);
    }

    /// `_store_str`: `text`, as much of it as there is room for.
    fn text(&mut self, m: &mut impl Guest, text: &[u8]) {
        for &byte in text {
            if self.left == 0 {
                break;
            }
            self.put(m, byte);
        }
    }

    /// `_store_num`: `num` in `digits` digits, or with `no_zeros` in as
    /// many as it takes. Without room for the digits and a 0 after them,
    /// nothing is written and there is no more room.
    fn number(&mut self, m: &mut impl Guest, num: i32, digits: u32, no_zeros: bool) {
        if no_zeros {
            return self.bare_number(m, num);
        }
        if digits >= self.left {
            self.left = 0;
            return;
        }
        let mut n = num;
        for i in (0..digits).rev() {
            m.write8(self.at.wrapping_add(i), (0x30 + n % 10) as u8);
            n /= 10;
        }
        self.at = self.at.wrapping_add(digits);
        self.left -= digits;
    }

    /// `_store_number`: the digits of `num`, last first while there is
    /// room for one and a 0, then turned around. With no room for even
    /// one, the turn swaps the byte before with the one at the place, as
    /// MSVCRT's does.
    fn bare_number(&mut self, m: &mut impl Guest, num: i32) {
        let start = self.at;
        let mut n = num;
        while self.left > 1 {
            self.put(m, (0x30 + n % 10) as u8);
            n /= 10;
            if n <= 0 {
                break;
            }
        }
        let (mut low, mut high) = (start, self.at.wrapping_sub(1));
        loop {
            let a = m.read8(low).unwrap_or(0);
            let b = m.read8(high).unwrap_or(0);
            m.write8(high, a);
            m.write8(low, b);
            low = low.wrapping_add(1);
            high = high.wrapping_sub(1);
            if low >= high {
                break;
            }
        }
    }
}

/// The time strftime formats and the names it takes, with '#' given for
/// the conversion running (`__alternate_form`).
struct Expand {
    tm: Tm,
    lc: u32,
    alternate: bool,
}

impl Expand {
    /// The string of field `field`, `index` places on.
    fn name(&self, m: &mut impl Guest, out: &mut Out, field: u32, index: i32) {
        let field = (field as i32).wrapping_add(index) as u32;
        if let Some(text) = lc_name(m, self.lc, field) {
            out.text(m, &text);
        }
    }

    /// `_expandtime`: the conversion `spec`. Numbers have no leading zeros
    /// with '#'.
    fn conversion(&mut self, m: &mut impl Guest, out: &mut Out, spec: u8) {
        let tm = self.tm;
        let (sec, min, hour, mday, mon, year, wday, yday, isdst) = (
            tm[0], tm[1], tm[2], tm[3], tm[4], tm[5], tm[6], tm[7], tm[8],
        );
        let bare = self.alternate;
        match spec {
            b'a' => self.name(m, out, LC_WDAY_ABBR, wday),
            b'A' => self.name(m, out, LC_WDAY, wday),
            b'b' => self.name(m, out, LC_MONTH_ABBR, mon),
            b'B' => self.name(m, out, LC_MONTH, mon),
            b'c' => {
                let date = if self.alternate { LC_LDATE } else { LC_SDATE };
                self.alternate = false;
                self.picture(m, out, date);
                if out.left == 0 {
                    return;
                }
                out.put(m, b' ');
                self.picture(m, out, LC_TIME);
            }
            b'd' => out.number(m, mday, 2, bare),
            b'H' => out.number(m, hour, 2, bare),
            b'I' => {
                let h = match hour % 12 {
                    0 => 12,
                    h => h,
                };
                out.number(m, h, 2, bare);
            }
            b'j' => out.number(m, yday.wrapping_add(1), 3, bare),
            b'm' => out.number(m, mon.wrapping_add(1), 2, bare),
            b'M' => out.number(m, min, 2, bare),
            b'p' => self.name(m, out, LC_AMPM, (hour > 11) as i32),
            b'S' => out.number(m, sec, 2, bare),
            b'U' | b'W' => {
                // The week of the year, the first starting on the first
                // Sunday (U) or Monday (W).
                let first = match (spec, wday) {
                    (b'U', d) => d,
                    (_, 0) => 6,
                    (_, d) => d.wrapping_sub(1),
                };
                let week = if yday < first {
                    0
                } else {
                    yday / 7 + (yday % 7 >= first) as i32
                };
                out.number(m, week, 2, bare);
            }
            b'w' => out.number(m, wday, 1, bare),
            b'x' => {
                let date = if self.alternate { LC_LDATE } else { LC_SDATE };
                self.alternate = false;
                self.picture(m, out, date);
            }
            b'X' => {
                self.alternate = false;
                self.picture(m, out, LC_TIME);
            }
            b'y' => out.number(m, year % 100, 2, bare),
            b'Y' => {
                let full = (year / 100).wrapping_add(19).wrapping_mul(100);
                out.number(m, full.wrapping_add(year % 100), 4, bare);
            }
            b'z' | b'Z' => {
                let name = word(m, var_at(V_TZNAME) + 4 * (isdst != 0) as u32)
                    .and_then(|at| string(m, at));
                out.text(m, &name.unwrap_or_default());
            }
            b'%' => out.put(m, b'%'),
            _ => {}
        }
    }

    /// `_store_winword`: the date or time format of field `field`, a
    /// Windows picture. A run of a letter is a conversion: d and dd the
    /// day, ddd and dddd its names; M and MM the month, MMM and MMMM its
    /// names; yy and yyyy the year; h, hh, H and HH the hour; m and mm the
    /// minutes; s and ss the seconds; as many bytes of AM or PM as there
    /// are t's, past its end too; and a or A AM or PM, "am/pm" and "a/p"
    /// at the end of the picture with it. Quotes take what they enclose
    /// as it is; another run is its first byte as it is, then the rest.
    /// Its numbers keep their zeros: strftime gives no '#' here.
    fn picture(&mut self, m: &mut impl Guest, out: &mut Out, field: u32) {
        let Some(format) = lc_name(m, self.lc, field) else {
            return;
        };
        let mut i = 0;
        while i < format.len() && out.left != 0 {
            let c = format[i];
            let run = format[i..].iter().take_while(|&&b| b == c).count();
            let next = i + run;
            let spec = match (c, run) {
                (b'd', 1 | 2) => Some(b'd'),
                (b'd', 3) => Some(b'a'),
                (b'd', 4) => Some(b'A'),
                (b'M', 1 | 2) => Some(b'm'),
                (b'M', 3) => Some(b'b'),
                (b'M', 4) => Some(b'B'),
                (b'y', 2) => Some(b'y'),
                (b'y', 4) => Some(b'Y'),
                (b'h', 1 | 2) => Some(b'I'),
                (b'H', 1 | 2) => Some(b'H'),
                (b'm', 1 | 2) => Some(b'M'),
                (b's', 1 | 2) => Some(b'S'),
                _ => None,
            };
            if let Some(spec) = spec {
                self.conversion(m, out, spec);
                i = next;
                continue;
            }
            match c {
                b't' => {
                    let field = LC_AMPM + (self.tm[2] > 11) as u32;
                    if let Some(at) = word(m, self.lc.wrapping_add(4 * field)) {
                        for k in 0..run as u32 {
                            if out.left == 0 {
                                break;
                            }
                            let byte = m.read8(at.wrapping_add(k)).unwrap_or(0);
                            out.put(m, byte);
                        }
                    }
                    i = next;
                }
                b'a' | b'A' => {
                    let rest = &format[i..];
                    i = if rest.eq_ignore_ascii_case(b"am/pm") {
                        i + 5
                    } else if rest.eq_ignore_ascii_case(b"a/p") {
                        i + 3
                    } else {
                        next
                    };
                    self.conversion(m, out, b'p');
                }
                b'\'' if run % 2 == 0 => i = next,
                b'\'' => {
                    i = next;
                    while i < format.len() && out.left != 0 {
                        if format[i] == b'\'' {
                            i += 1;
                            break;
                        }
                        out.put(m, format[i]);
                        i += 1;
                    }
                }
                _ => {
                    out.put(m, c);
                    i += 1;
                }
            }
        }
    }
}

/// strftime's work: the format `format` and the time `tm` with the names
/// of the `__lc_time_data` at `lc` (the "C" locale's for 0), into
/// `size` bytes at `buffer`. The length written, and a 0 after it; or 0,
/// with as much as there was room for and no 0, when it doesn't fit.
fn format_time(m: &mut impl Guest, buffer: u32, size: u32, format: &[u8], tm: &Tm, lc: u32) -> u32 {
    let mut out = Out {
        at: buffer,
        left: size,
    };
    let mut e = Expand {
        tm: *tm,
        lc: lc_time(lc),
        alternate: false,
    };
    let mut i = 0;
    while out.left > 0 && i < format.len() {
        if format[i] != b'%' {
            out.put(m, format[i]);
            i += 1;
            continue;
        }
        i += 1;
        e.alternate = format.get(i) == Some(&b'#');
        if e.alternate {
            i += 1;
        }
        let spec = format.get(i).copied().unwrap_or(0);
        e.conversion(m, &mut out, spec);
        i += 1;
    }
    if out.left == 0 {
        return 0;
    }
    m.write8(out.at, 0);
    size - out.left
}

/// strftime(buffer, size, format, tm): `_Strftime` with the current
/// locale's names.
pub fn strftime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[buffer, size, format, at] = args else {
        unreachable!()
    };
    big_strftime(cpu, &[buffer, size, format, at, 0])
}

/// _Strftime(buffer, size, format, tm, names): strftime with the names of
/// the `__lc_time_data` `names`, the current locale's for NULL.
pub fn big_strftime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[buffer, size, format, at, lc] = args else {
        unreachable!()
    };
    let format = cstr(cpu, format)?;
    let tm = read_tm(cpu, at)?;
    let written = format_time(cpu, buffer, size, &format, &tm, lc);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(written)
}

/// wcsftime(buffer, size, format, tm), with wide strings: the format made
/// ANSI, formatted on a block of the heap, the text made wide.
pub fn wcsftime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[buffer, size, format, at] = args else {
        unreachable!()
    };
    let format: Vec<u8> = wstr(cpu, format)?
        .into_iter()
        .map(|c| narrow(c).unwrap_or(b'?'))
        .collect();
    let tm = read_tm(cpu, at)?;
    if size == 0 {
        return Some(0);
    }
    let scratch = heap_alloc(cpu, size)?;
    let written = format_time(cpu, scratch, size, &format, &tm, 0);
    let text = bytes(cpu, scratch, written);
    heap_free(cpu, scratch);
    if written == 0 {
        return Some(0);
    }
    let text: Vec<u16> = text?.into_iter().map(widen).collect();
    put_wstr(cpu, buffer, &text)?;
    Some(text.len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Memory for the tests: every byte not written is 0.
    #[derive(Default)]
    struct Bytes(HashMap<u32, u8>);

    impl Guest for Bytes {
        fn read8(&mut self, at: u32) -> Option<u8> {
            Some(self.0.get(&at).copied().unwrap_or(0))
        }

        fn write8(&mut self, at: u32, byte: u8) {
            self.0.insert(at, byte);
        }
    }

    impl Bytes {
        fn put(&mut self, at: u32, data: &[u8]) {
            for (i, &byte) in data.iter().enumerate() {
                self.write8(at + i as u32, byte);
            }
        }
    }

    /// The probe's times: seconds, minutes, hours, day, month, year, day
    /// of the week and of the year, DST.
    const TIMES: [Tm; 6] = [
        [5, 3, 0, 2, 0, 80, 3, 1, 0],
        [59, 59, 23, 31, 11, 99, 5, 364, 0],
        [0, 30, 12, 9, 8, 100, 6, 252, 1],
        [9, 7, 11, 29, 1, 104, 0, 59, 0],
        [-3, 75, 25, -4, 6, -1, 2, 400, 0],
        [1, 1, 1, 1, 6, 205, 1, 6, 0],
    ];

    /// The fixture's escapes undone.
    fn unescape(text: &str) -> Vec<u8> {
        let bytes = text.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'x') {
                out.push(u8::from_str_radix(&text[i + 2..i + 4], 16).unwrap());
                i += 4;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        out
    }

    /// A made-up locale's names, laid out as the probe laid them out:
    /// the strings one after another, so that a format that reads past
    /// one's end reads the next.
    fn made_up(m: &mut Bytes, at: u32, formats: [&[u8]; 3]) -> u32 {
        let table = at + 0x1000;
        let mut names: Vec<Vec<u8>> = Vec::new();
        names.extend((0..7).map(|i| format!("D{i}a").into_bytes()));
        names.extend((0..7).map(|i| format!("Day{i}").into_bytes()));
        names.extend((0..12).map(|i| format!("M{i}a").into_bytes()));
        names.extend((0..12).map(|i| format!("Month{i}").into_bytes()));
        names.extend([b"am!".to_vec(), b"pm!".to_vec()]);
        names.extend(formats.iter().map(|f| f.to_vec()));
        names.push(b"TAIL".to_vec());
        let mut next = at;
        for (i, name) in names.iter().enumerate() {
            if i < LC_FIELDS as usize {
                m.put(table + 4 * i as u32, &next.to_le_bytes());
            }
            m.put(next, name);
            m.write8(next + name.len() as u32, 0);
            next += name.len() as u32 + 1;
        }
        table
    }

    /// strftime and _Strftime give what MSVCRT 6 gave the probe, byte for
    /// byte, short buffers and the bytes around them included.
    #[test]
    fn as_visual_cpp_6() {
        const BUFFER: u32 = 0x30000;
        let mut failed = Vec::new();
        let fixture = include_str!("strftime_vc6.txt");
        for line in fixture.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = line.split('\t').collect();
            let mut m = Bytes::default();
            let c = var_at(V_LC_TIME_C);
            m.put(c, &lc_time_c(c));
            // The time zone's names are empty, as on the probe's machine.
            m.put(var_at(V_TZNAME), &[0, 0x90, 0, 0, 0, 0x90, 0, 0]);
            let lc = match f[2] {
                "-" => 0,
                _ => made_up(
                    &mut m,
                    0x20000,
                    [&unescape(f[2]), &unescape(f[3]), &unescape(f[4])],
                ),
            };
            m.put(BUFFER, &[b'*'; 200]);
            let tm = &TIMES[f[0].parse::<usize>().unwrap()];
            let size = f[1].parse().unwrap();
            let written = format_time(&mut m, BUFFER, size, &unescape(f[5]), tm, lc);
            let expected = unescape(f[7]);
            let got: Vec<u8> = (0..expected.len() as u32)
                .map(|i| m.read8(BUFFER + i).unwrap())
                .collect();
            if written != f[6].parse::<u32>().unwrap() || got != expected {
                failed.push(format!(
                    "{line}: {written} {:?}",
                    String::from_utf8_lossy(&got)
                ));
            }
        }
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    }

    /// The "C" locale's names fit their variable.
    #[test]
    fn c_locale_fits() {
        assert!(lc_time_c(0).len() as u32 <= LC_TIME_C_SIZE);
    }
}
