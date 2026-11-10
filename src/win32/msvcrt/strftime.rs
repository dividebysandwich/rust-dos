//! strftime and the time names behind it: `strftime`, `_Strftime`,
//! `wcsftime`, and `_Getdays`, `_Getmonths` and `_Gettnames`, which give
//! MSVCP60 the locale's names.
//!
//! A locale's time names (`__lc_time_data`) are 43 pointers to strings:
//! the seven short and seven long day names, the twelve short and twelve
//! long month names, "AM" and "PM", then the short date, long date and
//! time formats as Windows writes them ("M/d/yy"). The "C" locale's table
//! is one of the DLL's variables, with its strings after it; `_Strftime`
//! takes a program's own table, as MSVCP60 passes one.
//!
//! What a call writes, short buffers and odd formats and names included,
//! is what MSVCRT.DLL 6.00.8337 writes, as observed by a probe program run
//! on that DLL; strftime_vc6.txt and the tests below hold its answers.

use super::time::{DAYS, LONG_DAYS, LONG_MONTHS, MONTHS, Tm, read_tm};
use super::*;

/// The memory strftime reads and writes: the program's, or a test's.
pub(super) trait Memory {
    /// The byte at `at`, None where there is no memory.
    fn load(&mut self, at: u32) -> Option<u8>;
    fn store(&mut self, at: u32, byte: u8);

    fn load32(&mut self, at: u32) -> Option<u32> {
        let mut word = 0;
        for i in 0..4 {
            word |= (self.load(at.wrapping_add(i))? as u32) << (8 * i);
        }
        Some(word)
    }

    /// The bytes at `at` up to their 0, which isn't included.
    fn load_str(&mut self, at: u32) -> Option<Vec<u8>> {
        let mut text = Vec::new();
        while let Some(byte) = self.load(at.wrapping_add(text.len() as u32)) {
            if byte == 0 {
                return Some(text);
            }
            text.push(byte);
        }
        None
    }
}

impl Memory for Cpu {
    fn load(&mut self, at: u32) -> Option<u8> {
        let byte = self.bus.guest_read_8(at);
        (!self.bus.guest_faulted()).then_some(byte)
    }

    fn store(&mut self, at: u32, byte: u8) {
        self.bus.guest_write_8(at, byte);
    }
}

/// The places of the names in a table, counted in pointers.
const DAY_ABBR: u32 = 0;
const DAY_FULL: u32 = 7;
const MONTH_ABBR: u32 = 14;
const MONTH_FULL: u32 = 26;
const AM_PM: u32 = 38;
const SHORT_DATE: u32 = 40;
const LONG_DATE: u32 = 41;
const TIME_FORMAT: u32 = 42;
const NAME_COUNT: u32 = 43;
/// The table's size, and the room the "C" locale's variable has for the
/// table and its strings.
const TABLE_SIZE: u32 = 4 * NAME_COUNT;
pub const LC_TIME_C_SIZE: u32 = 0x200;

/// The "C" locale's names, in table order.
fn c_names() -> impl Iterator<Item = &'static str> {
    DAYS.into_iter()
        .chain(LONG_DAYS)
        .chain(MONTHS)
        .chain(LONG_MONTHS)
        .chain(["AM", "PM", "M/d/yy", "dddd, MMMM dd, yyyy", "H:mm:ss"])
}

/// The "C" locale's table as it is at address `at`: the pointers, then
/// the strings they point to.
pub fn lc_time_c(at: u32) -> Vec<u8> {
    let mut table = Vec::new();
    let mut strings = Vec::new();
    for name in c_names() {
        table.extend((at + TABLE_SIZE + strings.len() as u32).to_le_bytes());
        strings.extend_from_slice(name.as_bytes());
        strings.push(0);
    }
    table.extend(strings);
    table
}

/// The table a call uses: the one it was given, else the "C" locale's,
/// which is the only one the built-in runtime has.
fn table_or_current(table: u32) -> u32 {
    match table {
        0 => var_at(V_LC_TIME_C),
        _ => table,
    }
}

/// The string of name `index` of the table at `table`.
fn name(m: &mut impl Memory, table: u32, index: u32) -> Option<Vec<u8>> {
    let at = m.load32(table.wrapping_add(index.wrapping_mul(4)))?;
    m.load_str(at)
}

/// A copy of `text` and a 0 in a block from malloc, or NULL.
fn malloc_text(cpu: &mut Cpu, text: &[u8]) -> Option<u32> {
    let Some(at) = heap_alloc(cpu, text.len() as u32 + 1) else {
        return Some(0);
    };
    put_cstr(cpu, at, text)?;
    Some(at)
}

/// ":short:long" for each of the `count` names that start at `abbr` and
/// at `full` in the current table, in one string.
fn colon_list(m: &mut impl Memory, abbr: u32, full: u32, count: u32) -> Option<Vec<u8>> {
    let table = table_or_current(0);
    let mut text = Vec::new();
    for i in 0..count {
        for index in [abbr + i, full + i] {
            text.push(b':');
            text.extend(name(m, table, index)?);
        }
    }
    Some(text)
}

/// _Getdays(): ":Sun:Sunday:Mon:Monday:...:Sat:Saturday", in a block
/// from malloc.
pub fn getdays(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let text = colon_list(cpu, DAY_ABBR, DAY_FULL, 7)?;
    malloc_text(cpu, &text)
}

/// _Getmonths(): ":Jan:January:...:Dec:December", in a block from malloc.
pub fn getmonths(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let text = colon_list(cpu, MONTH_ABBR, MONTH_FULL, 12)?;
    malloc_text(cpu, &text)
}

/// _Gettnames(): a copy of the current table in one block from malloc,
/// which the caller frees, or NULL.
pub fn gettnames(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let mut copy = table_copy(cpu, table_or_current(0))?;
    let Some(at) = heap_alloc(cpu, copy.len() as u32) else {
        return Some(0);
    };
    for pointer in copy[..TABLE_SIZE as usize].as_chunks_mut::<4>().0 {
        let offset = u32::from_le_bytes(*pointer);
        *pointer = (at + offset).to_le_bytes();
    }
    put(cpu, at, &copy)?;
    Some(at)
}

/// A copy of the table at `table` as _Gettnames lays it out, its
/// pointers counted from the copy's start. The strings follow the
/// pointers in the order the probe saw MSVCRT 6 put them: each day's
/// short then long name, each month's, then "AM", "PM" and the three
/// formats. MSVCRT 6 leaves the time format's string out, though its
/// pointer points at room for it, so a program reads whatever the heap
/// had there; this copy has it.
fn table_copy(m: &mut impl Memory, table: u32) -> Option<Vec<u8>> {
    let pairs =
        |first: u32, second: u32, count: u32| (0..count).flat_map(move |i| [first + i, second + i]);
    let order = pairs(DAY_ABBR, DAY_FULL, 7)
        .chain(pairs(MONTH_ABBR, MONTH_FULL, 12))
        .chain(AM_PM..NAME_COUNT);
    let mut copy = vec![0u8; TABLE_SIZE as usize];
    let mut strings = Vec::new();
    for index in order {
        let place = TABLE_SIZE + strings.len() as u32;
        copy[4 * index as usize..][..4].copy_from_slice(&place.to_le_bytes());
        strings.extend(name(m, table, index)?);
        strings.push(0);
    }
    copy.extend(strings);
    Some(copy)
}

/// The caller's buffer as strftime fills it. `left` counts the bytes it
/// still has, the final 0's included. Text goes in while there is room.
/// A number goes in whole or not at all, and only with a byte to spare;
/// one that doesn't fit ends the output. Both match what the probe saw
/// MSVCRT 6 write into short buffers.
struct Output {
    start: u32,
    next: u32,
    left: u32,
}

/// The character of a digit, which for a negative number's digits falls
/// below '0', as MSVCRT 6 writes them.
fn digit(value: i32) -> u8 {
    (b'0' as i32 + value) as u8
}

impl Output {
    fn new(buffer: u32, size: u32) -> Self {
        Output {
            start: buffer,
            next: buffer,
            left: size,
        }
    }

    fn byte(&mut self, m: &mut impl Memory, byte: u8) {
        if self.left > 0 {
            m.store(self.next, byte);
            self.next = self.next.wrapping_add(1);
            self.left -= 1;
        }
    }

    fn text(&mut self, m: &mut impl Memory, text: &[u8]) {
        for &byte in text {
            self.byte(m, byte);
        }
    }

    /// `value` in `width` digits, with zeros in front. Its last `width`
    /// digits, where it has more.
    fn padded(&mut self, m: &mut impl Memory, value: i32, width: u32) {
        if width >= self.left {
            self.left = 0;
            return;
        }
        for place in (0..width).rev() {
            let digit_value = (value / 10i32.pow(place)) % 10;
            self.byte(m, digit(digit_value));
        }
    }

    /// `value` without leading zeros, for the '#' flag; of a negative
    /// value, only the last digit, as MSVCRT 6 writes it. Where its digits
    /// don't all fit with a byte to spare, its last ones go in, as many as
    /// do. Where only the byte for the final 0 is left, MSVCRT 6 swaps the
    /// byte before the place with the one at it, which the probe saw in
    /// short buffers ("x%#d" in two bytes gives "*", the 'x' moved to
    /// where the 0 then goes). This does too, but only within the buffer,
    /// where MSVCRT 6 also changes the byte before a buffer of one.
    fn unpadded(&mut self, m: &mut impl Memory, value: i32) {
        if self.left == 0 {
            return;
        }
        let mut digits = Vec::new();
        let mut rest = value;
        loop {
            digits.push(digit(rest % 10));
            rest /= 10;
            if rest <= 0 {
                break;
            }
        }
        let room = self.left.saturating_sub(1) as usize;
        if room == 0 {
            if self.next != self.start {
                let before = self.next.wrapping_sub(1);
                if let (Some(a), Some(b)) = (m.load(before), m.load(self.next)) {
                    m.store(before, b);
                    m.store(self.next, a);
                }
            }
            return;
        }
        digits.truncate(room);
        for &byte in digits.iter().rev() {
            self.byte(m, byte);
        }
    }

    /// The final 0 and the length, or 0 when the text didn't fit.
    fn finish(mut self, m: &mut impl Memory) -> u32 {
        if self.left == 0 {
            return 0;
        }
        let length = self.next.wrapping_sub(self.start);
        self.byte(m, 0);
        length
    }
}

/// What a format is expanded with: the time, the names table, and the
/// '#' flag of the conversion being expanded.
struct Expansion<'a> {
    tm: &'a Tm,
    table: u32,
    alt: bool,
}

impl Expansion<'_> {
    fn second(&self) -> i32 {
        self.tm[0]
    }
    fn minute(&self) -> i32 {
        self.tm[1]
    }
    fn hour(&self) -> i32 {
        self.tm[2]
    }
    fn mday(&self) -> i32 {
        self.tm[3]
    }
    fn month(&self) -> i32 {
        self.tm[4]
    }
    fn year(&self) -> i32 {
        self.tm[5]
    }
    fn wday(&self) -> i32 {
        self.tm[6]
    }
    fn yday(&self) -> i32 {
        self.tm[7]
    }

    /// The hour on a 12-hour clock, 12 for 0.
    fn hour12(&self) -> i32 {
        match self.hour() % 12 {
            0 => 12,
            h => h,
        }
    }

    /// The week of the year, the weeks starting on Sunday, or on Monday
    /// with `monday`; the days before the first such day are in week 0,
    /// a negative day of the year among them. For days of the week and
    /// of the year out of their ranges this gives what the probe saw
    /// MSVCRT 6 give.
    fn week(&self, monday: bool) -> i32 {
        // The days the week had gone on for, by the start of the year's.
        let into_week = match (monday, self.wday()) {
            (false, wday) => wday,
            (true, 0) => 6,
            (true, wday) => wday.wrapping_sub(1),
        };
        if self.yday() < into_week {
            return 0;
        }
        self.yday() / 7 + (self.yday() % 7 >= into_week) as i32
    }

    /// The year with its century, which `tm_year` counts from 1900.
    fn full_year(&self) -> i32 {
        self.year().wrapping_add(1900)
    }

    /// Name `base + index` of the table. As in MSVCRT 6 the index isn't
    /// checked: a day of the week or a month out of its range takes the
    /// pointer that many places on, a name of the next field ("Sunday"
    /// for `%a` on day 7) or a word before or after the table.
    fn put_name(&self, m: &mut impl Memory, out: &mut Output, base: u32, index: i32) {
        let index = base.wrapping_add(index as u32);
        if let Some(text) = name(m, self.table, index) {
            out.text(m, &text);
        }
    }

    fn put_am_pm(&self, m: &mut impl Memory, out: &mut Output) {
        let index = (self.hour() >= 12) as i32;
        self.put_name(m, out, AM_PM, index);
    }

    /// A number: zero-padded to `width` digits, without the zeros under
    /// '#'.
    fn number(&self, m: &mut impl Memory, out: &mut Output, value: i32, width: u32) {
        if self.alt {
            out.unpadded(m, value);
        } else {
            out.padded(m, value, width);
        }
    }

    /// The format at `format`, its conversions expanded.
    fn format(&mut self, m: &mut impl Memory, out: &mut Output, format: &[u8]) {
        let mut rest = format;
        while let Some((&first, after)) = rest.split_first() {
            rest = after;
            if first != b'%' {
                out.byte(m, first);
                continue;
            }
            self.alt = rest.first() == Some(&b'#');
            if self.alt {
                rest = &rest[1..];
            }
            let Some((&spec, after)) = rest.split_first() else {
                break;
            };
            rest = after;
            self.conversion(m, out, spec);
        }
    }

    fn conversion(&mut self, m: &mut impl Memory, out: &mut Output, spec: u8) {
        match spec {
            b'a' => self.put_name(m, out, DAY_ABBR, self.wday()),
            b'A' => self.put_name(m, out, DAY_FULL, self.wday()),
            b'b' => self.put_name(m, out, MONTH_ABBR, self.month()),
            b'B' => self.put_name(m, out, MONTH_FULL, self.month()),
            b'c' => {
                self.picture(m, out, self.date_format());
                out.byte(m, b' ');
                self.picture(m, out, TIME_FORMAT);
            }
            b'd' => self.number(m, out, self.mday(), 2),
            b'H' => self.number(m, out, self.hour(), 2),
            b'I' => self.number(m, out, self.hour12(), 2),
            b'j' => self.number(m, out, self.yday().wrapping_add(1), 3),
            b'm' => self.number(m, out, self.month().wrapping_add(1), 2),
            b'M' => self.number(m, out, self.minute(), 2),
            b'p' => self.put_am_pm(m, out),
            b'S' => self.number(m, out, self.second(), 2),
            b'U' => self.number(m, out, self.week(false), 2),
            b'w' => self.number(m, out, self.wday(), 1),
            b'W' => self.number(m, out, self.week(true), 2),
            b'x' => self.picture(m, out, self.date_format()),
            b'X' => self.picture(m, out, TIME_FORMAT),
            b'y' => self.number(m, out, self.year() % 100, 2),
            b'Y' => self.number(m, out, self.full_year(), 4),
            b'z' | b'Z' => self.time_zone(m, out),
            b'%' => out.byte(m, b'%'),
            _ => {}
        }
    }

    /// The long date format under '#', else the short one.
    fn date_format(&self) -> u32 {
        if self.alt { LONG_DATE } else { SHORT_DATE }
    }

    /// The name of the time zone, `_tzname`'s standard or daylight one.
    /// Any nonzero `tm_isdst` takes the daylight one, -1 ("not known")
    /// included, as in MSVCRT 6 and Wine.
    fn time_zone(&self, m: &mut impl Memory, out: &mut Output) {
        let which = (self.tm[8] != 0) as u32;
        if let Some(at) = m.load32(var_at(V_TZNAME) + 4 * which)
            && let Some(text) = m.load_str(at)
        {
            out.text(m, &text);
        }
    }

    /// The table's format `index` in Windows' date and time pictures:
    /// runs of d, M, y, h, H, m, s and t stand for the parts of the time,
    /// 'a' or a closing "am/pm" or "a/p" for AM or PM, and text between
    /// single quotes is written as it is. The rules, longer runs and
    /// unknown letters included, are those the probe saw MSVCRT 6 follow.
    fn picture(&self, m: &mut impl Memory, out: &mut Output, index: u32) {
        let Some(at) = m.load32(self.table.wrapping_add(4 * index)) else {
            return;
        };
        let Some(picture) = m.load_str(at) else {
            return;
        };
        // The numbers of a picture keep their zeros under '#' too.
        let plain = Expansion {
            tm: self.tm,
            table: self.table,
            alt: false,
        };
        let mut quoted = false;
        let mut i = 0;
        while i < picture.len() {
            let c = picture[i];
            if c == b'\'' {
                quoted = !quoted;
                i += 1;
                continue;
            }
            if quoted {
                out.byte(m, c);
                i += 1;
                continue;
            }
            let run = picture[i..].iter().take_while(|&&b| b == c).count();
            i += run;
            match c {
                b'd' => plain.name_or_number(m, out, c, run, DAY_ABBR, DAY_FULL),
                b'M' => plain.name_or_number(m, out, c, run, MONTH_ABBR, MONTH_FULL),
                b'y' => {
                    // "yy" and "yyyy"; the y's beyond the longest of them
                    // that fits the run are written as they are.
                    let width = if run >= 4 { 4 } else { 2 * (run / 2) };
                    for _ in width..run {
                        out.byte(m, b'y');
                    }
                    match width {
                        4 => plain.number(m, out, self.full_year(), 4),
                        2 => plain.number(m, out, self.year() % 100, 2),
                        _ => {}
                    }
                }
                b'h' | b'H' | b'm' | b's' => {
                    for _ in 2..run {
                        out.byte(m, c);
                    }
                    let value = match c {
                        b'h' => self.hour12(),
                        b'H' => self.hour(),
                        b'm' => self.minute(),
                        _ => self.second(),
                    };
                    plain.number(m, out, value, 2);
                }
                b't' => {
                    // The first `run` bytes of the designator, whatever
                    // its length.
                    let index = AM_PM + (self.hour() >= 12) as u32;
                    if let Some(at) = m.load32(self.table.wrapping_add(4 * index)) {
                        for k in 0..run as u32 {
                            match m.load(at.wrapping_add(k)) {
                                Some(byte) => out.byte(m, byte),
                                None => break,
                            }
                        }
                    }
                }
                b'a' | b'A' => {
                    let tail = &picture[i - run..];
                    let closing =
                        tail.eq_ignore_ascii_case(b"am/pm") || tail.eq_ignore_ascii_case(b"a/p");
                    self.put_am_pm(m, out);
                    if closing {
                        return;
                    }
                }
                _ => {
                    for _ in 0..run {
                        out.byte(m, c);
                    }
                }
            }
        }
    }

    /// A run of `run` d's or M's: one or two for the number in two digits,
    /// three for the short name, four for the long one. Beyond four the
    /// extra letters are written as they are, then the long name.
    fn name_or_number(
        &self,
        m: &mut impl Memory,
        out: &mut Output,
        letter: u8,
        run: usize,
        abbr: u32,
        full: u32,
    ) {
        let (value, index) = if letter == b'd' {
            (self.mday(), self.wday())
        } else {
            (self.month().wrapping_add(1), self.month())
        };
        for _ in 4..run {
            out.byte(m, letter);
        }
        match run {
            1 | 2 => self.number(m, out, value, 2),
            3 => self.put_name(m, out, abbr, index),
            _ => self.put_name(m, out, full, index),
        }
    }
}

/// Expand `format` for `tm` with the names table at `table` (0 for the
/// current one) into the `size` bytes at `buffer`: the length written,
/// or 0 when the text and its 0 didn't fit.
fn expand(m: &mut impl Memory, buffer: u32, size: u32, format: &[u8], tm: &Tm, table: u32) -> u32 {
    let mut out = Output::new(buffer, size);
    let mut expansion = Expansion {
        tm,
        table: table_or_current(table),
        alt: false,
    };
    expansion.format(m, &mut out, format);
    out.finish(m)
}

/// strftime(buffer, size, format, tm).
pub fn strftime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[buffer, size, format, at] = args else {
        unreachable!()
    };
    let format = cstr(cpu, format)?;
    let tm = read_tm(cpu, at)?;
    Some(expand(cpu, buffer, size, &format, &tm, 0))
}

/// _Strftime(buffer, size, format, tm, names): strftime with a names
/// table of the caller's, or the current one for NULL.
pub fn big_strftime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[buffer, size, format, at, table] = args else {
        unreachable!()
    };
    let format = cstr(cpu, format)?;
    let tm = read_tm(cpu, at)?;
    Some(expand(cpu, buffer, size, &format, &tm, table))
}

/// errno for a character with no byte in the code page.
const EILSEQ: u32 = 42;

/// wcsftime(buffer, size, format, tm): strftime for wide strings, `size`
/// counted in characters. As the probe saw MSVCRT 6 do it: the format is
/// made ANSI, a character with no byte in the code page giving 0 and
/// errno EILSEQ; the text is what strftime gives in twice `size` bytes,
/// and 0 when that is 0, with the buffer untouched; of that text, the
/// first `size` characters are made wide, with a 0 after them where
/// there is room, and their count is returned. So a text of `size` to
/// `2 * size - 1` characters is cut to `size` with no 0.
pub fn wcsftime(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[buffer, size, format, at] = args else {
        unreachable!()
    };
    let format = wstr(cpu, format)?;
    let tm = read_tm(cpu, at)?;
    let Some(wide) = wide_text(cpu, size, &format, &tm) else {
        set_errno(cpu, EILSEQ);
        return Some(0);
    };
    let count = wide
        .iter()
        .take_while(|&&c| c != 0)
        .count()
        .min(size as usize);
    let raw: Vec<u8> = wide.iter().flat_map(|c| c.to_le_bytes()).collect();
    put(cpu, buffer, &raw)?;
    Some(count as u32)
}

/// What wcsftime writes for the wide `format` in a buffer of `size`
/// characters: the text's characters up to its 0, and the 0 where there
/// is room for it; nothing when strftime gives 0. None for a format with
/// a character the code page has no byte for.
fn wide_text(m: &mut impl Memory, size: u32, format: &[u16], tm: &Tm) -> Option<Vec<u16>> {
    let format: Vec<u8> = format.iter().map(|&c| narrow(c)).collect::<Option<_>>()?;
    let Some(text) = text_of(m, size.saturating_mul(2), &format, tm) else {
        return Some(Vec::new());
    };
    let mut wide: Vec<u16> = text
        .into_iter()
        .take_while(|&b| b != 0)
        .take(size as usize)
        .map(widen)
        .collect();
    if wide.len() < size as usize {
        wide.push(0);
    }
    Some(wide)
}

/// Memory of the host's for strftime to write into, over the memory it
/// reads the names from.
struct Scratch<'a, M: Memory> {
    names: &'a mut M,
    base: u32,
    bytes: Vec<u8>,
}

impl<M: Memory> Memory for Scratch<'_, M> {
    fn load(&mut self, at: u32) -> Option<u8> {
        match at.checked_sub(self.base) {
            Some(i) if (i as usize) < self.bytes.len() => Some(self.bytes[i as usize]),
            _ => self.names.load(at),
        }
    }

    fn store(&mut self, at: u32, byte: u8) {
        if let Some(i) = at.checked_sub(self.base)
            && let Some(slot) = self.bytes.get_mut(i as usize)
        {
            *slot = byte;
        }
    }
}

/// The text strftime gives for `format` in a buffer of `size` bytes, or
/// None when it gives 0.
fn text_of(m: &mut impl Memory, size: u32, format: &[u8], tm: &Tm) -> Option<Vec<u8>> {
    // Addresses no names table or string of the program's is at.
    const BASE: u32 = 0xFFFF_0000;
    let size = size.min(0xFFFF);
    let mut scratch = Scratch {
        names: m,
        base: BASE,
        bytes: vec![0; size as usize],
    };
    let length = expand(&mut scratch, BASE, size, format, tm, 0);
    scratch.bytes.truncate(length as usize);
    (length != 0).then_some(scratch.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Memory for the tests: every byte not written is 0.
    #[derive(Default)]
    struct Bytes(HashMap<u32, u8>);

    impl Memory for Bytes {
        fn load(&mut self, at: u32) -> Option<u8> {
            Some(self.0.get(&at).copied().unwrap_or(0))
        }

        fn store(&mut self, at: u32, byte: u8) {
            self.0.insert(at, byte);
        }
    }

    impl Bytes {
        fn put(&mut self, at: u32, data: &[u8]) {
            for (i, &byte) in data.iter().enumerate() {
                self.store(at + i as u32, byte);
            }
        }

        fn get(&mut self, at: u32, len: usize) -> Vec<u8> {
            (0..len as u32)
                .map(|i| self.load(at + i).unwrap())
                .collect()
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

    /// The fixtures' escapes undone: \xNN is the byte NN.
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

    /// A memory with the "C" locale's table and empty time zone names, as
    /// on the probe's machine.
    fn memory() -> Bytes {
        let mut m = Bytes::default();
        let c = var_at(V_LC_TIME_C);
        m.put(c, &lc_time_c(c));
        m.put(var_at(V_TZNAME), &[0, 0x90, 0, 0, 0, 0x90, 0, 0]);
        m
    }

    /// A made-up names table at `at`, as the probe made it: the strings
    /// one after another, so a picture that reads past one's end reads
    /// the next, then "TAIL". The table's address.
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
        for (i, text) in names.iter().enumerate() {
            if i < NAME_COUNT as usize {
                m.put(table + 4 * i as u32, &next.to_le_bytes());
            }
            m.put(next, text);
            m.store(next + text.len() as u32, 0);
            next += text.len() as u32 + 1;
        }
        table
    }

    const BUFFER: u32 = 0x30000;

    /// A fixture's time: an index into TIMES, or the nine fields.
    fn time_of(column: &str) -> Tm {
        if let Ok(index) = column.parse::<usize>() {
            return TIMES[index];
        }
        let fields: Vec<i32> = column.split(',').map(|f| f.parse().unwrap()).collect();
        fields.try_into().unwrap()
    }

    /// The failures of the strftime fixture `fixture` (see
    /// strftime_vc6.txt), as lines.
    fn check_fixture(fixture: &str) -> Vec<String> {
        let mut failed = Vec::new();
        for line in fixture.lines().filter(|l| !l.starts_with('#')) {
            let f: Vec<&str> = line.split('\t').collect();
            let mut m = memory();
            let table = match f[2] {
                "-" => 0,
                _ => made_up(
                    &mut m,
                    0x20000,
                    [&unescape(f[2]), &unescape(f[3]), &unescape(f[4])],
                ),
            };
            m.put(BUFFER, &[b'*'; 200]);
            let tm = time_of(f[0]);
            let size = f[1].parse().unwrap();
            let written = expand(&mut m, BUFFER, size, &unescape(f[5]), &tm, table);
            let expected = unescape(f[7]);
            let got = m.get(BUFFER, expected.len());
            if written != f[6].parse::<u32>().unwrap() || got != expected {
                failed.push(format!(
                    "{line}: {written} {:?}",
                    String::from_utf8_lossy(&got)
                ));
            }
        }
        failed
    }

    /// strftime and _Strftime give what MSVCRT 6 gave the probe, byte for
    /// byte, short buffers and the bytes after the text included.
    #[test]
    fn as_visual_cpp_6() {
        let failed = check_fixture(include_str!("strftime_vc6.txt"));
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    }

    /// The same for the second probe's cases: '#' numbers in short
    /// buffers, numbers out of range, formats ending in '%', pictures
    /// under '#', and more AM/PM and quote forms.
    #[test]
    fn as_visual_cpp_6_more() {
        let failed = check_fixture(include_str!("strftime_vc6_more.txt"));
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    }

    /// Where the built-in runtime gives something else than MSVCRT 6 did
    /// for the probe, by choice.
    #[test]
    fn own_choices() {
        let mut m = memory();
        // A '%' at the format's end ends it; MSVCRT 6 reads on past the
        // format's 0.
        m.put(BUFFER, &[b'*'; 8]);
        assert_eq!(expand(&mut m, BUFFER, 8, b"%%%", &TIMES[0], 0), 1);
        assert_eq!(m.get(BUFFER, 2), b"%\0");
        // "%#d" in one byte leaves the byte before the buffer alone,
        // where MSVCRT 6 swaps it with the buffer's first.
        m.put(BUFFER - 1, b"B*");
        assert_eq!(expand(&mut m, BUFFER, 1, b"%#d", &TIMES[1], 0), 0);
        assert_eq!(m.get(BUFFER - 1, 2), b"B\0");
    }

    /// Fields out of their ranges: a negative day of the year is in week
    /// 0, as MSVCRT 6 gave the probe for day -9 of a Saturday; a day of
    /// the week or a month past its names takes the next field's, as
    /// MSVCRT 6 checks no range; a negative `tm_isdst` takes the daylight
    /// name; and numbers at the ends of `int` wrap instead of overflowing.
    #[test]
    fn fields_out_of_range() {
        let mut m = memory();
        let tm = [0, 0, -13, 1, 0, -21, 6, -9, 0];
        m.put(BUFFER, &[b'*'; 8]);
        assert_eq!(expand(&mut m, BUFFER, 8, b"%U%W", &tm, 0), 4);
        assert_eq!(m.get(BUFFER, 5), b"0000\0");
        let tm = [0, 0, 0, 1, 12, 0, 7, 0, 0];
        m.put(BUFFER, &[b'*'; 16]);
        assert_eq!(expand(&mut m, BUFFER, 16, b"%a %b", &tm, 0), 14);
        assert_eq!(m.get(BUFFER, 15), b"Sunday January\0");
        let names = var_at(V_TZNAME);
        m.put(0x50000, b"STD\0DST\0");
        m.put(names, &[0x00, 0x00, 0x05, 0x00, 0x04, 0x00, 0x05, 0x00]);
        let tm = [0, 0, 0, 1, 0, 0, 0, 0, -1];
        assert_eq!(expand(&mut m, BUFFER, 16, b"%Z", &tm, 0), 3);
        assert_eq!(m.get(BUFFER, 4), b"DST\0");
        let tm = [0, 0, 0, 1, i32::MAX, i32::MAX, 0, i32::MAX, 0];
        assert_eq!(expand(&mut m, BUFFER, 16, b"%j%m%Y", &tm, 0), 9);
    }

    /// The fixture's \uNNNN escapes undone.
    fn unescape_wide(text: &str) -> Vec<u16> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(c) = rest.chars().next() {
            if let Some(hex) = rest.strip_prefix(r"\u") {
                out.push(u16::from_str_radix(&hex[..4], 16).unwrap());
                rest = &hex[4..];
            } else {
                out.push(c as u16);
                rest = &rest[c.len_utf8()..];
            }
        }
        out
    }

    /// wcsftime gives what MSVCRT 6 gave the probe: the value, errno
    /// (EILSEQ or untouched) and the buffer.
    #[test]
    fn wcsftime_as_visual_cpp_6() {
        let mut failed = Vec::new();
        for line in include_str!("wcsftime_vc6.txt").lines() {
            if line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            let size: u32 = f[1].parse().unwrap();
            let mut buffer = vec![0x2A2A; 80];
            let (value, errno) =
                match wide_text(&mut memory(), size, &unescape_wide(f[0]), &TIMES[1]) {
                    Some(wide) => {
                        buffer[..wide.len()].copy_from_slice(&wide);
                        (wide.iter().take_while(|&&c| c != 0).count() as u32, 0)
                    }
                    None => (0, EILSEQ),
                };
            let expected = unescape_wide(f[4]);
            if value != f[2].parse::<u32>().unwrap()
                || errno != f[3].parse::<u32>().unwrap()
                || buffer[..expected.len()] != expected[..]
            {
                failed.push(format!("{line}: {value} {errno} {:?}", &buffer[..20]));
            }
        }
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    }

    /// _Getdays' text, and _Gettnames' copy: where each string is from
    /// the copy's start and what it says, as the probe saw them, but for
    /// the time format, which this copy has.
    #[test]
    fn names_as_visual_cpp_6() {
        let mut m = memory();
        let days = colon_list(&mut m, DAY_ABBR, DAY_FULL, 7).unwrap();
        assert_eq!(
            days,
            b":Sun:Sunday:Mon:Monday:Tue:Tuesday:Wed:Wednesday:Thu:Thursday\
              :Fri:Friday:Sat:Saturday"
        );
        let months = colon_list(&mut m, MONTH_ABBR, MONTH_FULL, 12).unwrap();
        assert_eq!(months.len() + 1, 135);
        let copy = table_copy(&mut m, var_at(V_LC_TIME_C)).unwrap();
        let offsets = [
            172, 183, 194, 206, 220, 233, 244, 176, 187, 198, 210, 224, 237, 248, 257, 269, 282,
            292, 302, 310, 319, 328, 339, 353, 365, 378, 261, 273, 286, 296, 306, 314, 323, 332,
            343, 357, 369, 382, 391, 394, 397, 404, 424,
        ];
        let names: Vec<&str> = c_names().collect();
        for (index, &offset) in offsets.iter().enumerate() {
            let pointer = u32::from_le_bytes(copy[4 * index..][..4].try_into().unwrap());
            assert_eq!(pointer, offset, "name {index}");
            let text = &copy[offset as usize..][..names[index].len() + 1];
            assert_eq!(&text[..names[index].len()], names[index].as_bytes());
            assert_eq!(text[names[index].len()], 0);
        }
        assert_eq!(copy.len(), 432);
    }

    /// The "C" locale's table fits its variable.
    #[test]
    fn c_locale_fits() {
        assert!(lc_time_c(0).len() as u32 <= LC_TIME_C_SIZE);
    }
}
