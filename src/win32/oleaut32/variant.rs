//! VariantCopy and VariantChangeType: copies of VARIANTs, and their
//! values changed to another type. A VARIANT is its VARTYPE at 0 and its
//! value at 8, or with VT_BYREF a pointer to the value there.
//!
//! The changes are those between the numbers, VT_BOOL, VT_BSTR, VT_CY,
//! VT_DATE as a number and the interfaces, as Wine's
//! oleaut32/variant.c and vartype.c make them (LGPL 2.1 or later):
//! numbers round half to even, a number too big for the type is
//! DISP_E_OVERFLOW, and a BSTR from a real number is C's "%.15G" (R8) or
//! "%.7G" (R4). Dates as text, DECIMAL, records and arrays aren't done
//! and give DISP_E_TYPEMISMATCH.
//!
//! Copyright 1998 Jean-Claude Cote
//! Copyright 2003 Jon Griffiths
//! Copyright 2005 Daniel Remenak
//! Copyright 2006 Google (Benjamin Arai)

use super::model::*;
use crate::cpu::Cpu;
use crate::win32::com::{E_INVALIDARG, S_OK};

const VT_NULL: u16 = 1;
const VT_R4: u16 = 4;
const VT_R8: u16 = 5;
const VT_CY: u16 = 6;
const VT_DATE: u16 = 7;
const VT_I8: u16 = 20;
const VT_UI8: u16 = 21;
const VT_BYREF: u16 = 0x4000;
const VT_ARRAY: u16 = 0x2000;

pub const DISP_E_TYPEMISMATCH: u32 = 0x8002_0005;
pub const DISP_E_BADVARTYPE: u32 = 0x8002_0008;
pub const DISP_E_OVERFLOW: u32 = 0x8002_000A;

/// VariantChangeType's flags: VT_BOOL as "True" and "False".
const VARIANT_ALPHABOOL: u32 = 0x02;
const VARIANT_LOCALBOOL: u32 = 0x10;

const VARIANT_TRUE: u64 = 0xFFFF;

/// A VARIANT's value, read from the program's.
#[derive(Clone, Debug, PartialEq)]
enum Val {
    Empty,
    Null,
    Int(i64),
    UInt(u64),
    Real(f64),
    Bool(bool),
    Str(Vec<u16>),
    /// Currency: the value times 10000.
    Cy(i64),
    Date(f64),
    Error(u32),
    /// VT_DISPATCH or VT_UNKNOWN, and the pointer.
    Iface(u16, u32),
}

/// The bytes of the value of the type `vt` at `at`, if it can be read.
fn bits(cpu: &mut Cpu, at: u32, size: usize) -> Option<u64> {
    if !cpu.bus.guest_probe(at, size, false) {
        return None;
    }
    let mut b = [0u8; 8];
    cpu.bus.guest_read_bytes(at, &mut b[..size]);
    Some(u64::from_le_bytes(b))
}

/// The value of the VARIANT at `at`: Err with the HRESULT for a type
/// that isn't one or isn't taken.
fn read(cpu: &mut Cpu, at: u32, depth: u32) -> Result<Val, u32> {
    if !cpu.bus.guest_probe(at, 16, false) {
        return Err(E_INVALIDARG);
    }
    let vt = cpu.bus.guest_read_16(at);
    let mut place = at + 8;
    if vt & VT_BYREF != 0 {
        place = cpu.bus.guest_read_32(at + 8);
        if place == 0 {
            return Err(DISP_E_TYPEMISMATCH);
        }
        if vt & !VT_BYREF == VT_VARIANT && depth == 0 {
            return read(cpu, place, 1);
        }
    }
    if vt & VT_ARRAY != 0 {
        return Err(DISP_E_TYPEMISMATCH);
    }
    let base = vt & !VT_BYREF;
    let size = match base {
        VT_EMPTY | VT_NULL => 0,
        VT_I1 | VT_UI1 => 1,
        VT_I2 | VT_UI2 | VT_BOOL => 2,
        VT_I4 | VT_UI4 | VT_INT | VT_UINT | VT_R4 | VT_ERROR | VT_BSTR | VT_DISPATCH
        | VT_UNKNOWN => 4,
        VT_R8 | VT_CY | VT_DATE | VT_I8 | VT_UI8 => 8,
        // DECIMAL, records and the like.
        14 | 36 => return Err(DISP_E_TYPEMISMATCH),
        _ => return Err(DISP_E_BADVARTYPE),
    };
    let b = if size > 0 {
        bits(cpu, place, size).ok_or(E_INVALIDARG)?
    } else {
        0
    };
    Ok(match base {
        VT_EMPTY if vt & VT_BYREF != 0 => return Err(DISP_E_BADVARTYPE),
        VT_EMPTY => Val::Empty,
        VT_NULL => Val::Null,
        VT_I1 => Val::Int(b as u8 as i8 as i64),
        VT_UI1 => Val::Int(b as u8 as i64),
        VT_I2 => Val::Int(b as u16 as i16 as i64),
        VT_UI2 => Val::Int(b as u16 as i64),
        VT_I4 | VT_INT => Val::Int(b as u32 as i32 as i64),
        VT_UI4 | VT_UINT => Val::Int(b as u32 as i64),
        VT_I8 => Val::Int(b as i64),
        VT_UI8 => Val::UInt(b),
        VT_R4 => Val::Real(f32::from_bits(b as u32) as f64),
        VT_R8 => Val::Real(f64::from_bits(b)),
        VT_DATE => Val::Date(f64::from_bits(b)),
        VT_CY => Val::Cy(b as i64),
        VT_BOOL => Val::Bool(b as u16 != 0),
        VT_ERROR => Val::Error(b as u32),
        VT_BSTR => Val::Str(bstr_units(cpu, b as u32)),
        _ => Val::Iface(base, b as u32),
    })
}

/// The characters of the BSTR `s`.
fn bstr_units(cpu: &mut Cpu, s: u32) -> Vec<u16> {
    if s == 0 {
        return Vec::new();
    }
    let n = super::byte_len(cpu, s) / 2;
    if !cpu.bus.guest_probe(s, 2 * n as usize, false) {
        return Vec::new();
    }
    let mut bytes = vec![0u8; 2 * n as usize];
    cpu.bus.guest_read_bytes(s, &mut bytes);
    let (units, _) = bytes.as_chunks::<2>();
    units.iter().map(|&c| u16::from_le_bytes(c)).collect()
}

/// A number, as the changes between the numeric types take it.
enum Num {
    Int(i128),
    Real(f64),
}

/// A string as a number, as VarParseNumFromStr reads one: spaces around
/// it, a sign, thousands' commas, a decimal point, an exponent, or &H
/// and &O for hexadecimal and octal.
fn parse_number(units: &[u16]) -> Option<Num> {
    let text = String::from_utf16_lossy(units);
    let text = text.trim();
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, text.strip_prefix('+').unwrap_or(text).trim_start()),
    };
    let radix = |s: &str, r: u32| i128::from_str_radix(s, r).ok().map(Num::Int);
    let lower = digits.to_ascii_lowercase();
    let value = if let Some(hex) = lower.strip_prefix("&h") {
        radix(hex, 16)?
    } else if let Some(oct) = lower.strip_prefix("&o") {
        radix(oct, 8)?
    } else {
        let plain: String = digits.chars().filter(|&c| c != ',').collect();
        let valid = !plain.is_empty()
            && plain.starts_with(|c: char| c.is_ascii_digit() || c == '.')
            && plain
                .chars()
                .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'));
        if !valid {
            return None;
        }
        match plain.parse::<i128>() {
            Ok(i) => Num::Int(i),
            Err(_) => Num::Real(plain.parse::<f64>().ok()?),
        }
    };
    Some(match (negative, value) {
        (false, v) => v,
        (true, Num::Int(i)) => Num::Int(-i),
        (true, Num::Real(f)) => Num::Real(-f),
    })
}

fn number(v: &Val) -> Result<Num, u32> {
    Ok(match v {
        Val::Empty => Num::Int(0),
        Val::Int(i) => Num::Int(*i as i128),
        Val::UInt(u) => Num::Int(*u as i128),
        Val::Bool(b) => Num::Int(if *b { -1 } else { 0 }),
        Val::Real(f) | Val::Date(f) => Num::Real(*f),
        Val::Cy(c) => Num::Real(*c as f64 / 10000.0),
        Val::Str(s) => parse_number(s).ok_or(DISP_E_TYPEMISMATCH)?,
        _ => return Err(DISP_E_TYPEMISMATCH),
    })
}

/// The integer of `n`, rounded half to even, if it is in `min..=max`.
fn integer(n: Num, min: i128, max: i128) -> Result<i128, u32> {
    let i = match n {
        Num::Int(i) => i,
        Num::Real(f) => {
            let r = f.round_ties_even();
            if !r.is_finite() || r < min as f64 - 1.0 || r > max as f64 + 1.0 {
                return Err(DISP_E_OVERFLOW);
            }
            r as i128
        }
    };
    if i < min || i > max {
        return Err(DISP_E_OVERFLOW);
    }
    Ok(i)
}

fn real(n: Num) -> f64 {
    match n {
        Num::Int(i) => i as f64,
        Num::Real(f) => f,
    }
}

/// C's "%.<precision>G" of `f`.
fn format_g(f: f64, precision: usize) -> String {
    if f == 0.0 {
        return "0".into();
    }
    if !f.is_finite() {
        return if f.is_nan() {
            "NAN".into()
        } else if f > 0.0 {
            "INF".into()
        } else {
            "-INF".into()
        };
    }
    let sci = format!("{:.*e}", precision - 1, f);
    let (mantissa, exponent) = sci.split_once('e').expect("an exponent");
    let exponent: i32 = exponent.parse().expect("a number");
    let trim = |s: String| -> String {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    };
    if exponent < -4 || exponent >= precision as i32 {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!(
            "{}E{}{:02}",
            trim(mantissa.to_string()),
            sign,
            exponent.abs()
        )
    } else {
        let decimals = (precision as i32 - 1 - exponent).max(0) as usize;
        trim(format!("{:.*}", decimals, f))
    }
}

fn currency_text(c: i64) -> String {
    let sign = if c < 0 { "-" } else { "" };
    let a = c.unsigned_abs();
    let whole = a / 10000;
    let fraction = a % 10000;
    if fraction == 0 {
        format!("{sign}{whole}")
    } else {
        let f = format!("{fraction:04}");
        format!("{sign}{whole}.{}", f.trim_end_matches('0'))
    }
}

/// What the destination gets.
enum Out {
    Bits(u16, u64),
    Str(Vec<u16>),
    Iface(u16, u32),
}

/// The value `v` changed to the type `vt`.
fn change(v: &Val, vt: u16, flags: u32) -> Result<Out, u32> {
    if matches!(v, Val::Null) && vt != VT_NULL && vt != VT_EMPTY {
        return Err(DISP_E_TYPEMISMATCH);
    }
    let int = |min: i128, max: i128| -> Result<u64, u32> {
        if let (Val::Str(s), VT_BOOL) = (v, vt) {
            let t = String::from_utf16_lossy(s);
            let t = t.trim();
            if t.eq_ignore_ascii_case("true") {
                return Ok(VARIANT_TRUE);
            }
            if t.eq_ignore_ascii_case("false") {
                return Ok(0);
            }
        }
        Ok(integer(number(v)?, min, max)? as u64)
    };
    let bits = |b: u64| Ok(Out::Bits(vt, b));
    match vt {
        VT_EMPTY if matches!(v, Val::Null) => Err(DISP_E_TYPEMISMATCH),
        VT_EMPTY => bits(0),
        VT_NULL if matches!(v, Val::Empty | Val::Null) => bits(0),
        VT_NULL => Err(DISP_E_TYPEMISMATCH),
        VT_I1 => bits(int(-128, 127)? & 0xFF),
        VT_UI1 => bits(int(0, 255)?),
        VT_I2 => bits(int(-0x8000, 0x7FFF)? & 0xFFFF),
        VT_UI2 => bits(int(0, 0xFFFF)?),
        VT_I4 | VT_INT => bits(int(i32::MIN as i128, i32::MAX as i128)? & 0xFFFF_FFFF),
        VT_UI4 | VT_UINT => bits(int(0, u32::MAX as i128)?),
        VT_I8 => bits(int(i64::MIN as i128, i64::MAX as i128)?),
        VT_UI8 => bits(int(0, u64::MAX as i128)?),
        VT_BOOL => {
            let b = match v {
                Val::Str(_) => int(i128::MIN, i128::MAX)? != 0,
                _ => match number(v)? {
                    Num::Int(i) => i != 0,
                    Num::Real(f) => f != 0.0,
                },
            };
            bits(if b { VARIANT_TRUE } else { 0 })
        }
        VT_R4 => {
            let f = real(number(v)?);
            if f.is_finite() && f.abs() > f32::MAX as f64 {
                return Err(DISP_E_OVERFLOW);
            }
            bits((f as f32).to_bits() as u64)
        }
        VT_R8 | VT_DATE => bits(real(number(v)?).to_bits()),
        VT_CY => {
            let c = match number(v)? {
                Num::Int(i) => i.checked_mul(10000).ok_or(DISP_E_OVERFLOW)?,
                Num::Real(f) => integer(Num::Real(f * 10000.0), i128::MIN, i128::MAX)?,
            };
            if c < i64::MIN as i128 || c > i64::MAX as i128 {
                return Err(DISP_E_OVERFLOW);
            }
            bits(c as i64 as u64)
        }
        VT_ERROR => match v {
            Val::Error(e) => bits(*e as u64),
            _ => Err(DISP_E_TYPEMISMATCH),
        },
        VT_BSTR => {
            let text = match v {
                Val::Empty => String::new(),
                Val::Str(s) => return Ok(Out::Str(s.clone())),
                Val::Int(i) => i.to_string(),
                Val::UInt(u) => u.to_string(),
                Val::Bool(b) if flags & (VARIANT_ALPHABOOL | VARIANT_LOCALBOOL) != 0 => {
                    (if *b { "True" } else { "False" }).to_string()
                }
                Val::Bool(b) => (if *b { "-1" } else { "0" }).to_string(),
                Val::Real(f) => format_g(*f, 15),
                Val::Cy(c) => currency_text(*c),
                _ => return Err(DISP_E_TYPEMISMATCH),
            };
            Ok(Out::Str(text.encode_utf16().collect()))
        }
        VT_DISPATCH => match v {
            Val::Iface(VT_DISPATCH, p) => Ok(Out::Iface(vt, *p)),
            _ => Err(DISP_E_TYPEMISMATCH),
        },
        // An IDispatch is an IUnknown.
        VT_UNKNOWN => match v {
            Val::Iface(_, p) => Ok(Out::Iface(vt, *p)),
            _ => Err(DISP_E_TYPEMISMATCH),
        },
        14 | 36 => Err(DISP_E_TYPEMISMATCH),
        _ => Err(DISP_E_BADVARTYPE),
    }
}

/// A VT_R4's text is "%.7G" of the single value.
fn change_from(cpu: &mut Cpu, src: u32, vt: u16, flags: u32) -> Result<Out, u32> {
    let v = read(cpu, src, 0)?;
    if vt == VT_BSTR
        && let Val::Real(f) = v
        && cpu.bus.guest_read_16(src) & !VT_BYREF == VT_R4
    {
        return Ok(Out::Str(format_g(f, 7).encode_utf16().collect()));
    }
    change(&v, vt, flags)
}

/// Put `out` in the VARIANT at `dest`, after clearing what it held: the
/// calls to make, AddRef of the new interface and Release of the old.
fn store(cpu: &mut Cpu, dest: u32, out: Out) -> Result<Vec<(u32, u32)>, u32> {
    let mut calls = Vec::new();
    let (vt, value) = match out {
        Out::Bits(vt, b) => (vt, b),
        Out::Str(units) => {
            let s = super::alloc(cpu, 0, 2 * units.len() as u32);
            if s == 0 {
                return Err(crate::win32::com::E_OUTOFMEMORY);
            }
            for (i, u) in units.iter().enumerate() {
                cpu.bus.guest_write_16(s + 2 * i as u32, *u);
            }
            (VT_BSTR, s as u64)
        }
        Out::Iface(vt, p) => {
            if let Some(m) = crate::win32::ole32::method(cpu, p, 1) {
                calls.push((m, p));
            }
            (vt, p as u64)
        }
    };
    let old_vt = cpu.bus.guest_read_16(dest);
    let old = cpu.bus.guest_read_32(dest + 8);
    if old_vt & (VT_BYREF | VT_ARRAY) == 0 {
        match old_vt {
            VT_BSTR => super::free(cpu, old),
            VT_DISPATCH | VT_UNKNOWN => {
                if let Some(m) = crate::win32::ole32::method(cpu, old, 2) {
                    calls.push((m, old));
                }
            }
            _ => {}
        }
    }
    cpu.bus.guest_fill(dest, 16, 0);
    cpu.bus.guest_write_16(dest, vt);
    cpu.bus.guest_write_bytes(dest + 8, &value.to_le_bytes());
    Ok(calls)
}

/// VariantChangeType(dest, src, flags, vt).
pub fn variant_change_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[dest, src, flags, vt] = args else {
        unreachable!()
    };
    change_type(cpu, dest, src, flags & 0xFFFF, vt as u16)
}

/// VariantChangeTypeEx(dest, src, lcid, flags, vt): the locale's own
/// formats aren't taken; those of English (United States) are.
pub fn variant_change_type_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[dest, src, _lcid, flags, vt] = args else {
        unreachable!()
    };
    change_type(cpu, dest, src, flags & 0xFFFF, vt as u16)
}

fn change_type(cpu: &mut Cpu, dest: u32, src: u32, flags: u32, vt: u16) -> Option<u32> {
    if !cpu.bus.guest_probe(dest, 16, true) || !cpu.bus.guest_probe(src, 16, false) {
        return Some(E_INVALIDARG);
    }
    let result = change_from(cpu, src, vt, flags).and_then(|out| store(cpu, dest, out));
    match result {
        Ok(calls) => crate::win32::ole32::call_all(cpu, S_OK, &calls),
        Err(e) => Some(e),
    }
}

/// VariantCopy(dest, src): a BSTR copied, an interface with a reference
/// added, a VT_BYREF pointer as it is.
pub fn variant_copy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[dest, src] = args else { unreachable!() };
    if !cpu.bus.guest_probe(dest, 16, true) || !cpu.bus.guest_probe(src, 16, false) {
        return Some(E_INVALIDARG);
    }
    if dest == src {
        return Some(S_OK);
    }
    let vt = cpu.bus.guest_read_16(src);
    let out = if vt & VT_BYREF != 0 {
        Out::Bits(vt, cpu.bus.guest_read_32(src + 8) as u64)
    } else if vt & VT_ARRAY != 0 {
        // SAFEARRAYs aren't taken yet.
        return Some(DISP_E_BADVARTYPE);
    } else {
        match read(cpu, src, 0) {
            Ok(Val::Str(units)) => {
                // A NULL BSTR stays NULL.
                if cpu.bus.guest_read_32(src + 8) == 0 {
                    Out::Bits(VT_BSTR, 0)
                } else {
                    Out::Str(units)
                }
            }
            Ok(Val::Iface(vt, p)) => Out::Iface(vt, p),
            Ok(_) => Out::Bits(vt, bits(cpu, src + 8, 8).unwrap_or(0)),
            Err(e) => return Some(e),
        }
    };
    match store(cpu, dest, out) {
        Ok(calls) => crate::win32::ole32::call_all(cpu, S_OK, &calls),
        Err(e) => Some(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(v: &Val, vt: u16) -> String {
        match change(v, VT_BSTR, 0).ok() {
            Some(Out::Str(s)) => String::from_utf16_lossy(&s),
            _ => format!("no text of {vt}"),
        }
    }

    #[test]
    fn reals_print_as_percent_g() {
        assert_eq!(format_g(1.5, 15), "1.5");
        assert_eq!(format_g(100.0, 15), "100");
        assert_eq!(format_g(0.1, 15), "0.1");
        assert_eq!(format_g(1e20, 15), "1E+20");
        assert_eq!(format_g(1.25e-7, 15), "1.25E-07");
        assert_eq!(format_g(0.0001, 15), "0.0001");
        assert_eq!(format_g(-2.0 / 3.0, 7), "-0.6666667");
        assert_eq!(text(&Val::Real(3.0), VT_R8), "3");
    }

    #[test]
    fn numbers_round_half_to_even() {
        let int = |v: Val| match change(&v, VT_I4, 0) {
            Ok(Out::Bits(_, b)) => Ok(b as u32 as i32),
            Ok(_) => Err(0),
            Err(e) => Err(e),
        };
        assert_eq!(int(Val::Real(2.5)), Ok(2));
        assert_eq!(int(Val::Real(3.5)), Ok(4));
        assert_eq!(int(Val::Real(-2.5)), Ok(-2));
        assert_eq!(int(Val::Str("  1,234 ".encode_utf16().collect())), Ok(1234));
        assert_eq!(int(Val::Str("&H10".encode_utf16().collect())), Ok(16));
        assert_eq!(
            int(Val::Str("x".encode_utf16().collect())),
            Err(DISP_E_TYPEMISMATCH)
        );
        assert_eq!(int(Val::Real(3e9)), Err(DISP_E_OVERFLOW));
        assert_eq!(int(Val::Null), Err(DISP_E_TYPEMISMATCH));
        assert_eq!(int(Val::Bool(true)), Ok(-1));
    }

    #[test]
    fn bools_print_as_numbers_unless_asked() {
        assert_eq!(text(&Val::Bool(true), VT_BOOL), "-1");
        match change(&Val::Bool(false), VT_BSTR, VARIANT_ALPHABOOL) {
            Ok(Out::Str(s)) => assert_eq!(String::from_utf16_lossy(&s), "False"),
            _ => panic!("no text"),
        }
        match change(&Val::Str("True".encode_utf16().collect()), VT_BOOL, 0) {
            Ok(Out::Bits(VT_BOOL, b)) => assert_eq!(b, VARIANT_TRUE),
            _ => panic!("no bool"),
        }
    }

    #[test]
    fn currency_keeps_four_decimals() {
        assert_eq!(currency_text(15000), "1.5");
        assert_eq!(currency_text(-12345), "-1.2345");
        assert_eq!(currency_text(20000), "2");
    }
}
