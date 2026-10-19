//! The printf and scanf families' formats, as MSVCRT reads them, and the
//! digits of doubles (`_ecvt`, `_fcvt`, `%e`, `%f`, `%g`).
//!
//! Text is in 16-bit units both ways: the narrow functions' bytes, or the
//! wide functions' characters. Arguments come from the program's stack.

use super::*;

/// Where a format's arguments are: the next one's address.
pub struct Args {
    pub at: u32,
}

impl Args {
    fn u32(&mut self, cpu: &mut Cpu) -> Option<u32> {
        let value = read32(cpu, self.at)?;
        self.at = self.at.wrapping_add(4);
        Some(value)
    }

    fn u64(&mut self, cpu: &mut Cpu) -> Option<u64> {
        let lo = self.u32(cpu)? as u64;
        let hi = self.u32(cpu)? as u64;
        Some(hi << 32 | lo)
    }

    fn f64(&mut self, cpu: &mut Cpu) -> Option<f64> {
        Some(f64::from_bits(self.u64(cpu)?))
    }
}

/// How significant digits of a double go: MSVCRT has 17, then zeros.
/// It rounds those 17 digits, half up, to what a format asks for, so
/// 2.25 with one decimal is 2.3.
const MAX_DIGITS: usize = 17;

/// The most digits a number's precision asks for that MSVCRT gives: its
/// buffer's size, MAXPRECISION.
pub(super) const MAX_PRECISION: usize = 512;

/// The widest a field gets here. MSVCRT has no limit and writes a width
/// of 2^31 - 1 out space by space; this stops at 16 M characters, so a
/// width the program got wrong can't take the host's memory.
const MAX_WIDTH: usize = 0x100_0000;

/// Round the digits `digits` (of a number whose point is after `decpt` of
/// them) to their first `keep`, half up: the digits and point after. A
/// carry past the first digit adds a digit in front.
pub fn round_digits(digits: &[u8], decpt: i32, keep: usize) -> (Vec<u8>, i32) {
    let mut out: Vec<u8> = digits
        .iter()
        .copied()
        .chain(std::iter::repeat(b'0'))
        .take(keep)
        .collect();
    let mut decpt = decpt;
    if digits.get(keep).is_some_and(|&d| d >= b'5') {
        let mut i = keep;
        loop {
            if i == 0 {
                out.insert(0, b'1');
                decpt += 1;
                break;
            }
            i -= 1;
            if out[i] == b'9' {
                out[i] = b'0';
            } else {
                out[i] += 1;
                break;
            }
        }
    }
    (out, decpt)
}

/// The 17 significant digits of |v|, not 0, and where its point is.
fn all_digits(v: f64) -> (Vec<u8>, i32) {
    let text = format!("{:.*e}", MAX_DIGITS - 1, v.abs());
    let (mantissa, exp) = text.split_once('e').unwrap();
    let digits = mantissa.bytes().filter(|b| b.is_ascii_digit()).collect();
    (digits, exp.parse::<i32>().unwrap() + 1)
}

/// The digits of |v| with `frac` digits after the point, as (digits,
/// decpt): 123.45 with 2 is ("12345", 3). Leading zeros are left out, so
/// 0.06 with 2 is ("6", -1), and a value that rounds to 0 has none.
pub fn fixed_digits(v: f64, frac: usize) -> (Vec<u8>, i32) {
    if v == 0.0 {
        return (Vec::new(), 0);
    }
    let (digits, decpt) = all_digits(v);
    let keep = decpt + frac as i32;
    if keep < 0 {
        return (Vec::new(), decpt);
    }
    round_digits(&digits, decpt, keep as usize)
}

/// The first `sig` significant digits of |v|, rounded, and where the
/// point is: 123.45 with 2 is ("12", 3). Zero is all zeros, point 0.
pub fn exp_digits(v: f64, sig: usize) -> (Vec<u8>, i32) {
    if v == 0.0 {
        return (vec![b'0'; sig], 0);
    }
    let (digits, decpt) = all_digits(v);
    let (mut digits, decpt) = round_digits(&digits, decpt, sig);
    digits.truncate(sig);
    (digits, decpt)
}

/// A format's flags and sizes for one conversion.
#[derive(Default)]
struct Spec {
    left: bool,
    plus: bool,
    space: bool,
    alt: bool,
    zero: bool,
    width: usize,
    precision: Option<usize>,
}

impl Spec {
    /// `body` padded to the width: on the right with '-', with zeros
    /// after `prefix` (the sign, 0x) with '0', else with spaces before.
    fn pad(&self, out: &mut Vec<u16>, prefix: &[u16], body: &[u16], zeros: bool) {
        let len = prefix.len() + body.len();
        let fill = self.width.saturating_sub(len);
        if self.left {
            out.extend_from_slice(prefix);
            out.extend_from_slice(body);
            out.extend(std::iter::repeat_n(b' ' as u16, fill));
        } else if zeros {
            out.extend_from_slice(prefix);
            out.extend(std::iter::repeat_n(b'0' as u16, fill));
            out.extend_from_slice(body);
        } else {
            out.extend(std::iter::repeat_n(b' ' as u16, fill));
            out.extend_from_slice(prefix);
            out.extend_from_slice(body);
        }
    }
}

fn units(s: &str) -> Vec<u16> {
    s.bytes().map(u16::from).collect()
}

/// Format an integer: `value` its magnitude, `negative` its sign.
fn integer(out: &mut Vec<u16>, spec: &Spec, value: u64, negative: bool, conv: u8) {
    let (base, upper) = match conv {
        b'o' => (8, false),
        b'x' => (16, false),
        b'X' | b'p' => (16, true),
        _ => (10, false),
    };
    let mut digits = match base {
        8 => format!("{:o}", value),
        16 if upper => format!("{:X}", value),
        16 => format!("{:x}", value),
        _ => format!("{}", value),
    };
    if spec.precision == Some(0) && value == 0 {
        digits.clear();
    }
    let precision = spec.precision.unwrap_or(0).min(MAX_PRECISION);
    if digits.len() < precision {
        digits = "0".repeat(precision - digits.len()) + &digits;
    }
    let mut prefix = String::new();
    match conv {
        b'd' | b'i' => {
            if negative {
                prefix.push('-');
            } else if spec.plus {
                prefix.push('+');
            } else if spec.space {
                prefix.push(' ');
            }
        }
        b'o' if spec.alt && !digits.starts_with('0') => digits.insert(0, '0'),
        b'x' if spec.alt && value != 0 => prefix.push_str("0x"),
        b'X' if spec.alt && value != 0 => prefix.push_str("0X"),
        b'p' if spec.alt => prefix.push_str("0X"),
        _ => {}
    }
    let zeros = spec.zero && !spec.left && spec.precision.is_none();
    spec.pad(out, &units(&prefix), &units(&digits), zeros);
}

/// MSVCRT's text for an infinity or NaN with `precision` digits.
pub(super) fn special(v: f64, precision: usize) -> String {
    let name = if v.is_infinite() {
        "1#INF"
    } else if v.is_sign_negative() {
        "1#IND"
    } else {
        "1#QNAN"
    };
    let mut text = format!("1.{}", &name[1..]);
    let frac = text.len() - 2;
    if precision > frac {
        text.extend(std::iter::repeat_n('0', precision - frac));
    } else if precision > 0 {
        text.truncate(2 + precision);
    } else {
        text.truncate(1);
    }
    text
}

/// The text of `v` (unsigned) for %e: d.ddde+ddd.
fn e_text(v: f64, precision: usize, alt: bool, upper: bool) -> String {
    let (digits, decpt) = exp_digits(v, precision + 1);
    let exp = if v == 0.0 { 0 } else { decpt - 1 };
    let mut text = String::new();
    text.push(digits[0] as char);
    if precision > 0 || alt {
        text.push('.');
    }
    text.extend(digits[1..].iter().map(|&d| d as char));
    text.push(if upper { 'E' } else { 'e' });
    text.push(if exp < 0 { '-' } else { '+' });
    text += &format!("{:03}", exp.abs());
    text
}

/// The text of `v` (unsigned) for %f: ddd.ddd.
fn f_text(v: f64, precision: usize, alt: bool) -> String {
    let (digits, decpt) = fixed_digits(v, precision);
    let mut text = String::new();
    if decpt <= 0 {
        text.push('0');
    } else {
        text.extend(digits[..decpt as usize].iter().map(|&d| d as char));
    }
    if precision > 0 || alt {
        text.push('.');
    }
    for i in 0..precision as i32 {
        let at = decpt + i;
        text.push(if at < 0 {
            '0'
        } else {
            digits.get(at as usize).map_or('0', |&d| d as char)
        });
    }
    text
}

/// The text of `v` (unsigned) for %g: %e or %f, whichever its exponent
/// calls for, without trailing zeros unless `alt`.
pub fn g_text(v: f64, precision: usize, alt: bool, upper: bool) -> String {
    let precision = precision.max(1);
    let (_, decpt) = exp_digits(v, precision);
    let exp = if v == 0.0 { 0 } else { decpt - 1 };
    let mut text = if exp < -4 || exp >= precision as i32 {
        e_text(v, precision - 1, alt, upper)
    } else {
        f_text(v, (precision as i32 - 1 - exp) as usize, alt)
    };
    if !alt {
        let (mantissa, exponent) = match text.find(['e', 'E']) {
            Some(at) => (text[..at].to_string(), text[at..].to_string()),
            None => (text.clone(), String::new()),
        };
        let mantissa = if mantissa.contains('.') {
            mantissa
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
        } else {
            mantissa
        };
        text = mantissa + &exponent;
    }
    text
}

/// Format a double for %e, %f or %g.
fn float(out: &mut Vec<u16>, spec: &Spec, v: f64, conv: u8) {
    let precision = spec.precision.unwrap_or(6).min(MAX_PRECISION);
    let body = if !v.is_finite() {
        special(v, precision)
    } else {
        match conv {
            b'e' | b'E' => e_text(v, precision, spec.alt, conv == b'E'),
            b'f' => f_text(v, precision, spec.alt),
            _ => g_text(v, precision, spec.alt, conv == b'G'),
        }
    };
    let sign = if v.is_sign_negative() {
        "-"
    } else if spec.plus {
        "+"
    } else if spec.space {
        " "
    } else {
        ""
    };
    let zeros = spec.zero && !spec.left;
    spec.pad(out, &units(sign), &units(&body), zeros);
}

/// Format the format `format` with the arguments at `args`, for a narrow
/// function (`wide` false: strings are bytes unless 'l', 'w' or %S say
/// they are wide) or a wide one. None if a page wasn't there.
pub fn printf(cpu: &mut Cpu, format: &[u16], args: &mut Args, wide: bool) -> Option<Vec<u16>> {
    let mut out: Vec<u16> = Vec::new();
    let mut i = 0;
    let at = |i: usize| {
        format
            .get(i)
            .map_or(0u8, |&c| if c < 0x80 { c as u8 } else { 0xFF })
    };
    while i < format.len() {
        if format[i] != b'%' as u16 {
            out.push(format[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i == format.len() {
            break;
        }
        if at(i) == b'%' {
            out.push(b'%' as u16);
            i += 1;
            continue;
        }
        let mut spec = Spec::default();
        loop {
            match at(i) {
                b'-' => spec.left = true,
                b'+' => spec.plus = true,
                b' ' => spec.space = true,
                b'#' => spec.alt = true,
                b'0' => spec.zero = true,
                _ => break,
            }
            i += 1;
        }
        if at(i) == b'*' {
            let width = args.u32(cpu)? as i32;
            if width < 0 {
                spec.left = true;
            }
            spec.width = width.unsigned_abs() as usize;
            i += 1;
        } else {
            while at(i).is_ascii_digit() {
                spec.width = spec
                    .width
                    .saturating_mul(10)
                    .saturating_add((at(i) - b'0') as usize);
                i += 1;
            }
        }
        spec.width = spec.width.min(MAX_WIDTH);
        if at(i) == b'.' {
            i += 1;
            let mut precision = 0;
            if at(i) == b'*' {
                let p = args.u32(cpu)? as i32;
                precision = p.max(0) as usize;
                i += 1;
                spec.precision = (p >= 0).then_some(precision);
            } else {
                while at(i).is_ascii_digit() {
                    precision = precision
                        .saturating_mul(10)
                        .saturating_add((at(i) - b'0') as usize);
                    i += 1;
                }
                spec.precision = Some(precision);
            }
        }
        // Sizes: h short, l and w long or wide, I64 64 bits, I32 32; L,
        // F and N mean nothing here.
        let (mut short, mut long, mut int64) = (false, false, false);
        loop {
            match at(i) {
                b'h' => {
                    short = true;
                    long = false;
                }
                b'l' | b'w' => {
                    long = true;
                    short = false;
                }
                b'L' | b'F' | b'N' => {}
                b'I' if at(i + 1) == b'6' && at(i + 2) == b'4' => {
                    int64 = true;
                    i += 2;
                }
                b'I' if at(i + 1) == b'3' && at(i + 2) == b'2' => i += 2,
                _ => break,
            }
            i += 1;
        }
        if i == format.len() {
            break;
        }
        let conv = at(i);
        i += 1;
        match conv {
            b'd' | b'i' | b'u' | b'o' | b'x' | b'X' => {
                let raw = if int64 {
                    args.u64(cpu)?
                } else {
                    let v = args.u32(cpu)?;
                    let v = if short { v & 0xFFFF } else { v };
                    match conv {
                        b'd' | b'i' if short => v as u16 as i16 as i64 as u64,
                        b'd' | b'i' => v as i32 as i64 as u64,
                        _ => v as u64,
                    }
                };
                let signed = matches!(conv, b'd' | b'i');
                let negative = signed && (raw as i64) < 0;
                let magnitude = if negative {
                    (raw as i64).unsigned_abs()
                } else {
                    raw
                };
                integer(&mut out, &spec, magnitude, negative, conv);
            }
            b'p' => {
                let v = args.u32(cpu)? as u64;
                let spec = Spec {
                    precision: Some(8),
                    ..spec
                };
                integer(&mut out, &spec, v, false, b'p');
            }
            b'c' | b'C' => {
                let v = args.u32(cpu)?;
                let wide_char = if conv == b'C' {
                    !short
                } else {
                    long || (wide && !short)
                };
                let c = if wide_char {
                    let c = v as u16;
                    if wide {
                        c
                    } else {
                        narrow(c).unwrap_or(b'?') as u16
                    }
                } else {
                    let b = v as u8;
                    if wide { widen(b) } else { b as u16 }
                };
                let zeros = spec.zero && !spec.left;
                spec.pad(&mut out, &[], &[c], zeros);
            }
            b's' | b'S' | b'Z' => {
                let at_str = args.u32(cpu)?;
                // %s is the function's own width, %S the other, unless 'h'
                // or 'l' says which; %Z is a counted string, wide with 'l'.
                let wide_str = match (conv, wide) {
                    (b'Z', _) => long,
                    (b'S', false) | (b's', true) => !short,
                    _ => long,
                };
                let mut text: Vec<u16> = if at_str == 0 {
                    units("(null)")
                } else if conv == b'Z' {
                    // An ANSI_STRING or UNICODE_STRING: length, room, text.
                    let len = cpu.bus.guest_read_16(at_str) as u32;
                    let buffer = read32(cpu, at_str + 4)?;
                    if long {
                        let data = bytes(cpu, buffer, len)?;
                        data.chunks(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]))
                            .collect()
                    } else {
                        bytes(cpu, buffer, len)?
                            .into_iter()
                            .map(u16::from)
                            .collect()
                    }
                } else if wide_str {
                    let limit = spec.precision.unwrap_or(usize::MAX);
                    wstr_n(cpu, at_str, limit)?
                } else {
                    let limit = spec.precision.unwrap_or(usize::MAX);
                    cstr_n(cpu, at_str, limit)?
                        .into_iter()
                        .map(u16::from)
                        .collect()
                };
                if let Some(p) = spec.precision {
                    text.truncate(p);
                }
                let wide_str = wide_str && at_str != 0;
                let text: Vec<u16> = match (wide, wide_str) {
                    (false, true) => text
                        .into_iter()
                        .map(|c| narrow(c).unwrap_or(b'?') as u16)
                        .collect(),
                    (true, false) => text.into_iter().map(|b| widen(b as u8)).collect(),
                    _ => text,
                };
                let zeros = spec.zero && !spec.left;
                spec.pad(&mut out, &[], &text, zeros);
            }
            b'e' | b'E' | b'f' | b'g' | b'G' => {
                let v = args.f64(cpu)?;
                float(&mut out, &spec, v, conv);
            }
            b'n' => {
                let at_n = args.u32(cpu)?;
                if short {
                    cpu.bus.guest_write_16(at_n, out.len() as u16);
                } else {
                    cpu.bus.guest_write_32(at_n, out.len() as u32);
                }
                if cpu.bus.guest_faulted() {
                    return None;
                }
            }
            _ => out.push(format[i - 1]),
        }
    }
    Some(out)
}

/// Where scanf reads from: a string, or a stream.
pub enum Input {
    Text { chars: Vec<u16>, pos: usize },
    Stream { file: u32 },
}

impl Input {
    /// The next character, taken; None at the end. Err if a page wasn't
    /// there.
    fn next(&mut self, cpu: &mut Cpu) -> Result<Option<u16>, ()> {
        match self {
            Input::Text { chars, pos } => {
                let c = chars.get(*pos).copied();
                if c.is_some() {
                    *pos += 1;
                }
                Ok(c)
            }
            Input::Stream { file, .. } => match super::stdio::getc(cpu, *file) {
                super::stdio::Got::Byte(b) => Ok(Some(b as u16)),
                super::stdio::Got::End | super::stdio::Got::Wait => Ok(None),
                super::stdio::Got::Fault => Err(()),
            },
        }
    }

    /// Give back `c`, the character `next` took.
    fn back(&mut self, cpu: &mut Cpu, c: Option<u16>) {
        let Some(c) = c else { return };
        match self {
            Input::Text { pos, .. } => *pos -= 1,
            Input::Stream { file, .. } => {
                super::stdio::ungetc_byte(cpu, *file, c as u8);
            }
        }
    }
}

fn is_space(c: u16) -> bool {
    c == b' ' as u16 || (9..=13).contains(&c)
}

fn digit_value(c: u16, base: u32) -> Option<u32> {
    char::from_u32(c as u32)?.to_digit(base)
}

/// Read the input as the format `format` says, storing what it converts
/// through the pointers at `args`: how many were stored, or -1 when the
/// input ended before the first conversion. `wide` for the wide
/// functions, whose %s and %c store wide characters.
#[allow(unused_assignments)] // `back!` at the end of a conversion
pub fn scanf(
    cpu: &mut Cpu,
    format: &[u16],
    input: &mut Input,
    args: &mut Args,
    wide: bool,
) -> Option<u32> {
    let mut stored = 0u32;
    let mut consumed = 0u32;
    let mut i = 0;
    let at = |i: usize| {
        format
            .get(i)
            .map_or(0u8, |&c| if c < 0x80 { c as u8 } else { 0xFF })
    };
    let mut ended = false;
    macro_rules! next {
        () => {{
            match input.next(cpu) {
                Ok(c) => {
                    if c.is_some() {
                        consumed += 1;
                    }
                    c
                }
                Err(()) => return None,
            }
        }};
    }
    macro_rules! back {
        ($c:expr) => {{
            let c = $c;
            if c.is_some() {
                consumed -= 1;
            }
            input.back(cpu, c);
        }};
    }
    while i < format.len() {
        let f = format[i];
        if is_space(f) {
            loop {
                let c = next!();
                if c.is_none() {
                    ended = true;
                }
                if !c.is_some_and(is_space) {
                    back!(c);
                    break;
                }
            }
            i += 1;
            continue;
        }
        if f != b'%' as u16 || i + 1 >= format.len() {
            let c = next!();
            if c != Some(f) {
                if c.is_none() {
                    ended = true;
                }
                back!(c);
                break;
            }
            i += 1;
            continue;
        }
        i += 1;
        let skip = at(i) == b'*';
        if skip {
            i += 1;
        }
        let mut width = 0usize;
        while at(i).is_ascii_digit() {
            width = width
                .saturating_mul(10)
                .saturating_add((at(i) - b'0') as usize);
            i += 1;
        }
        let width = if width == 0 { usize::MAX } else { width };
        let (mut short, mut long, mut int64) = (false, false, false);
        loop {
            match at(i) {
                b'h' => short = true,
                b'l' | b'w' => long = true,
                b'L' => long = true,
                b'I' if at(i + 1) == b'6' && at(i + 2) == b'4' => {
                    int64 = true;
                    i += 2;
                }
                _ => break,
            }
            i += 1;
        }
        let conv = at(i);
        i += 1;
        // All but %c, %[ and %n skip white space first.
        if !matches!(conv, b'c' | b'C' | b'[' | b'n') {
            loop {
                let c = next!();
                if !c.is_some_and(is_space) {
                    if c.is_none() {
                        ended = true;
                    }
                    back!(c);
                    break;
                }
            }
        }
        match conv {
            b'd' | b'i' | b'u' | b'o' | b'x' | b'X' | b'p' | b'P' => {
                let mut base = match conv {
                    b'o' => 8,
                    b'x' | b'X' | b'p' | b'P' => 16,
                    b'i' => 0,
                    _ => 10,
                };
                let mut taken = 0;
                let mut negative = false;
                let mut c = next!();
                if taken < width && matches!(c, Some(0x2B) | Some(0x2D)) {
                    negative = c == Some(b'-' as u16);
                    taken += 1;
                    c = next!();
                }
                let mut value: u64 = 0;
                let mut digits = 0;
                if base == 0 || base == 16 {
                    if taken < width && c == Some(b'0' as u16) {
                        taken += 1;
                        digits += 1;
                        c = next!();
                        if taken < width
                            && matches!(c, Some(0x78) | Some(0x58))
                            && conv != b'p'
                            && conv != b'P'
                        {
                            base = 16;
                            taken += 1;
                            c = next!();
                        } else if base == 0 {
                            base = 8;
                        }
                    } else if base == 0 {
                        base = 10;
                    }
                }
                while taken < width {
                    let Some(d) = c.and_then(|c| digit_value(c, base)) else {
                        break;
                    };
                    value = value.wrapping_mul(base as u64).wrapping_add(d as u64);
                    digits += 1;
                    taken += 1;
                    c = next!();
                }
                back!(c);
                if digits == 0 {
                    if c.is_none() {
                        ended = true;
                    }
                    break;
                }
                let value = if negative {
                    value.wrapping_neg()
                } else {
                    value
                };
                if !skip {
                    let to = args.u32(cpu)?;
                    if int64 {
                        write32(cpu, to, value as u32)?;
                        write32(cpu, to + 4, (value >> 32) as u32)?;
                    } else if short {
                        cpu.bus.guest_write_16(to, value as u16);
                    } else {
                        write32(cpu, to, value as u32)?;
                    }
                    stored += 1;
                }
            }
            b'e' | b'E' | b'f' | b'g' | b'G' => {
                let mut text = String::new();
                let mut c = next!();
                let mut taken = 0;
                let accept = |text: &str, c: u8| -> bool {
                    match c {
                        b'+' | b'-' => text.is_empty() || text.ends_with(['e', 'E']),
                        b'0'..=b'9' => true,
                        b'.' => !text.contains(['.', 'e', 'E']),
                        b'e' | b'E' => {
                            !text.contains(['e', 'E']) && text.bytes().any(|b| b.is_ascii_digit())
                        }
                        _ => false,
                    }
                };
                while taken < width {
                    let Some(ch) = c.filter(|&ch| ch < 0x80 && accept(&text, ch as u8)) else {
                        break;
                    };
                    text.push(ch as u8 as char);
                    taken += 1;
                    c = next!();
                }
                back!(c);
                let trimmed = text.trim_end_matches(['e', 'E', '+', '-']);
                let Ok(value) = trimmed.parse::<f64>() else {
                    if c.is_none() {
                        ended = true;
                    }
                    break;
                };
                if !skip {
                    let to = args.u32(cpu)?;
                    if long {
                        put(cpu, to, &value.to_le_bytes())?;
                    } else {
                        put(cpu, to, &(value as f32).to_le_bytes())?;
                    }
                    stored += 1;
                }
            }
            b's' | b'S' => {
                let wide_out = if conv == b'S' {
                    !wide && !short
                } else {
                    (wide && !short) || long
                };
                let mut text = Vec::new();
                let mut c = next!();
                while text.len() < width {
                    let Some(ch) = c.filter(|&ch| !is_space(ch)) else {
                        break;
                    };
                    text.push(ch);
                    c = next!();
                }
                back!(c);
                if text.is_empty() {
                    if c.is_none() {
                        ended = true;
                    }
                    break;
                }
                if !skip {
                    let to = args.u32(cpu)?;
                    store_text(cpu, to, &text, wide, wide_out, true)?;
                    stored += 1;
                }
            }
            b'c' | b'C' => {
                let wide_out = if conv == b'C' {
                    !wide && !short
                } else {
                    (wide && !short) || long
                };
                let count = if width == usize::MAX { 1 } else { width };
                let mut text = Vec::new();
                while text.len() < count {
                    match next!() {
                        Some(ch) => text.push(ch),
                        None => break,
                    }
                }
                if text.is_empty() {
                    ended = true;
                    break;
                }
                if !skip {
                    let to = args.u32(cpu)?;
                    store_text(cpu, to, &text, wide, wide_out, false)?;
                    stored += 1;
                }
            }
            b'[' => {
                let wide_out = (wide && !short) || long;
                let mut set: Vec<(u16, u16)> = Vec::new();
                let negate = at(i) == b'^';
                if negate {
                    i += 1;
                }
                if at(i) == b']' {
                    set.push((b']' as u16, b']' as u16));
                    i += 1;
                }
                while i < format.len() && at(i) != b']' {
                    let lo = format[i];
                    if at(i + 1) == b'-' && i + 2 < format.len() && at(i + 2) != b']' {
                        let hi = format[i + 2];
                        set.push((lo.min(hi), lo.max(hi)));
                        i += 3;
                    } else {
                        set.push((lo, lo));
                        i += 1;
                    }
                }
                i += 1;
                let mut text = Vec::new();
                let mut c = next!();
                while text.len() < width {
                    let Some(ch) = c else { break };
                    let inside = set.iter().any(|&(lo, hi)| (lo..=hi).contains(&ch));
                    if inside == negate {
                        break;
                    }
                    text.push(ch);
                    c = next!();
                }
                back!(c);
                if text.is_empty() {
                    if c.is_none() {
                        ended = true;
                    }
                    break;
                }
                if !skip {
                    let to = args.u32(cpu)?;
                    store_text(cpu, to, &text, wide, wide_out, true)?;
                    stored += 1;
                }
            }
            b'n' => {
                if !skip {
                    let to = args.u32(cpu)?;
                    write32(cpu, to, consumed)?;
                }
            }
            0 => break,
            other => {
                // '%' and any other character match themselves.
                let c = next!();
                if c != Some(other as u16) {
                    if c.is_none() {
                        ended = true;
                    }
                    back!(c);
                    break;
                }
            }
        }
    }
    if stored == 0 && ended {
        return Some(u32::MAX);
    }
    Some(stored)
}

/// Store the characters `text` at `to`: as bytes, or with `wide_out`
/// 16-bit characters, with a 0 after them for `terminate`.
fn store_text(
    cpu: &mut Cpu,
    to: u32,
    text: &[u16],
    wide: bool,
    wide_out: bool,
    terminate: bool,
) -> Option<()> {
    let mut out: Vec<u8> = Vec::new();
    for &c in text {
        if wide_out {
            let c = if wide { c } else { widen(c as u8) };
            out.extend(c.to_le_bytes());
        } else {
            out.push(if wide {
                narrow(c).unwrap_or(b'?')
            } else {
                c as u8
            });
        }
    }
    if terminate {
        out.extend(if wide_out { &[0u8, 0][..] } else { &[0u8][..] });
    }
    put(cpu, to, &out)
}
