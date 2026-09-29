//! Operands: a side of a condition, a constant or a value read from
//! memory, as it is now, was the frame before (delta), or was before it
//! last changed (prior), and read as BCD or inverted.

use super::memref::{MemrefId, Memrefs, Size};
use super::parse::{Cursor, Error, Parse};
use super::typed::{Kind, Oper, Typed};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperandType {
    Address,
    Delta,
    Const,
    Fp,
    Func,
    Prior,
    Bcd,
    Inverted,
    Recall,
    /// Not set.
    None,
}

impl OperandType {
    pub fn is_memref(self) -> bool {
        !matches!(
            self,
            OperandType::Const
                | OperandType::Fp
                | OperandType::Func
                | OperandType::Recall
                | OperandType::None
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Operand {
    pub ty: OperandType,
    /// The size the definition reads, which the shared reference may
    /// differ from.
    pub size: Size,
    /// How the reference is read (a recall's: that of what it recalls).
    pub access: OperandType,
    pub memref: Option<MemrefId>,
    pub num: u32,
    pub dbl: f64,
    /// Whether it combines this condition with the chain before it.
    pub is_combining: bool,
}

impl Default for Operand {
    fn default() -> Self {
        Self::NONE
    }
}

impl Operand {
    pub const NONE: Operand = Operand {
        ty: OperandType::None,
        size: Size::Bits32,
        access: OperandType::None,
        memref: None,
        num: 0,
        dbl: 0.0,
        is_combining: false,
    };

    pub fn constant(value: u32) -> Self {
        Self {
            ty: OperandType::Const,
            size: Size::Bits32,
            access: OperandType::None,
            num: value,
            ..Self::NONE
        }
    }

    pub fn float_constant(value: f64) -> Self {
        Self {
            ty: OperandType::Fp,
            size: Size::Float,
            access: OperandType::None,
            dbl: value,
            ..Self::NONE
        }
    }

    pub fn is_set(&self) -> bool {
        self.ty != OperandType::None
    }

    pub fn is_memref(&self) -> bool {
        self.ty.is_memref()
    }

    pub fn is_float(&self, memrefs: &Memrefs) -> bool {
        match self.ty {
            OperandType::Fp => true,
            OperandType::Recall => self.size.is_float(),
            _ => self.is_float_memref(memrefs),
        }
    }

    fn is_float_memref(&self, memrefs: &Memrefs) -> bool {
        if !self.is_memref() {
            return false;
        }
        if let Some(id) = self.memref
            && let super::memref::Source::Derived(derived) = &memrefs.get(id).source
            && derived.op != Oper::IndirectRead
        {
            return memrefs.get(id).value.size.is_float();
        }
        self.size.is_float()
    }

    /// The operand's value now.
    pub fn evaluate(&self, memrefs: &Memrefs) -> Typed {
        let mut result = match self.ty {
            OperandType::Const => return Typed::unsigned(self.num),
            OperandType::Fp => return Typed::float(self.dbl as f32),
            OperandType::Func | OperandType::None => return Typed::unsigned(0),
            OperandType::Recall => {
                if !self.access.is_memref() {
                    let recalled = Operand {
                        ty: self.access,
                        ..*self
                    };
                    return recalled.evaluate(memrefs);
                }
                match self.memref {
                    None => return Typed::unsigned(0),
                    Some(id) => memref_value(memrefs, id, self.access),
                }
            }
            _ => match self.memref {
                Some(id) => memref_value(memrefs, id, self.ty),
                None => return Typed::unsigned(0),
            },
        };
        self.size.transform(&mut result);
        if result.kind == Kind::Unsigned {
            result.bits = self.transform(result.bits);
        }
        result
    }

    /// BCD and inversion.
    fn transform(&self, value: u32) -> u32 {
        match self.ty {
            OperandType::Bcd => {
                let digits = match self.size {
                    Size::Bits8 => 2,
                    Size::Bits16 | Size::Bits16Be => 4,
                    Size::Bits24 | Size::Bits24Be => 6,
                    Size::Bits32 | Size::Bits32Be | Size::Variable => 8,
                    _ => return value,
                };
                (0..digits)
                    .rev()
                    .fold(0, |acc, d| acc * 10 + ((value >> (4 * d)) & 0x0F))
            }
            OperandType::Inverted => {
                value
                    ^ match self.size {
                        Size::Low | Size::High => 0x0F,
                        Size::Bits8 => 0xFF,
                        Size::Bits16 | Size::Bits16Be => 0xFFFF,
                        Size::Bits24 | Size::Bits24Be => 0xFF_FFFF,
                        Size::Bits32 | Size::Bits32Be | Size::Variable => 0xFFFF_FFFF,
                        _ => 0x01,
                    }
            }
            _ => value,
        }
    }

    /// Make this operand the chain's accumulated value so far combined
    /// with it (AddSource and SubSource).
    pub fn add_source(&mut self, parse: &mut Parse, new_size: Size) {
        let parent = parse.addsource_parent;
        let id =
            if matches!(self.ty, OperandType::Delta | OperandType::Prior) && self.ty == parent.ty {
                // Adding delta(x) and delta(y) is the delta of x + y.
                let modifier = Operand {
                    ty: OperandType::Address,
                    ..*self
                };
                parse.addsource_parent.ty = OperandType::Address;
                let parent = parse.addsource_parent;
                parse
                    .memrefs
                    .derived(new_size, &parent, parse.addsource_oper, &modifier)
            } else {
                let id = parse
                    .memrefs
                    .derived(new_size, &parent, parse.addsource_oper, self);
                self.ty = OperandType::Address;
                self.access = OperandType::Address;
                id
            };
        self.memref = Some(id);
        // An AddSource's result is a 32-bit integer, floats or not.
        self.size = Size::Bits32;
    }
}

/// A reference's value as `ty` reads it.
fn memref_value(memrefs: &Memrefs, id: MemrefId, ty: OperandType) -> Typed {
    let value = &memrefs.get(id).value;
    let bits = match ty {
        OperandType::Delta if value.changed => value.prior,
        OperandType::Prior => value.prior,
        _ => value.value,
    };
    Typed {
        bits,
        kind: value.kind,
    }
}

/// The size letter after "0x": `0xH1234` is 8 bits.
fn memref_size(c: u8) -> Option<Size> {
    Some(match c.to_ascii_lowercase() {
        b'h' => Size::Bits8,
        b' ' => Size::Bits16,
        b'x' => Size::Bits32,
        b'm' => Size::Bit(0),
        b'n' => Size::Bit(1),
        b'o' => Size::Bit(2),
        b'p' => Size::Bit(3),
        b'q' => Size::Bit(4),
        b'r' => Size::Bit(5),
        b's' => Size::Bit(6),
        b't' => Size::Bit(7),
        b'l' => Size::Low,
        b'u' => Size::High,
        b'k' => Size::BitCount,
        b'w' => Size::Bits24,
        b'g' => Size::Bits32Be,
        b'i' => Size::Bits16Be,
        b'j' => Size::Bits24Be,
        _ => return None,
    })
}

/// A memory reference's size and address: `0xH1234`, `0x1234` (16 bits),
/// `fF1234` (a float).
pub fn parse_memref(s: &mut Cursor) -> Result<(Size, u32), Error> {
    let size = match (s.at(0), s.at(1)) {
        (b'0', b'x' | b'X') => {
            s.skip(2);
            let c = s.take();
            match memref_size(c) {
                Some(size) => size,
                None if c.is_ascii_hexdigit() => {
                    if c == b'0' && s.at(0) == b'x' {
                        // 0x0x1234: an extra 0x.
                        return Err(Error::InvalidMemoryOperand);
                    }
                    // Without a size letter: 16 bits.
                    s.back(1);
                    Size::Bits16
                }
                None => return Err(Error::InvalidMemoryOperand),
            }
        }
        (b'f' | b'F', _) => {
            s.skip(1);
            match s.take().to_ascii_lowercase() {
                b'f' => Size::Float,
                b'b' => Size::FloatBe,
                b'h' => Size::Double32,
                b'i' => Size::Double32Be,
                b'm' => Size::Mbf32,
                b'l' => Size::Mbf32Le,
                _ => return Err(Error::InvalidFpOperand),
            }
        }
        _ => return Err(Error::InvalidMemoryOperand),
    };
    let address = s.strtoul(16).ok_or(Error::InvalidMemoryOperand)?;
    Ok((size, address.min(0xFFFF_FFFF) as u32))
}

fn parse_memory(s: &mut Cursor, parse: &mut Parse) -> Result<Operand, Error> {
    let ty = match s.at(0) {
        b'd' | b'D' => OperandType::Delta,
        b'p' | b'P' => OperandType::Prior,
        b'b' | b'B' => OperandType::Bcd,
        b'~' => OperandType::Inverted,
        _ => OperandType::Address,
    };
    if ty != OperandType::Address {
        s.skip(1);
    }
    let (size, address) = parse_memref(s)?;
    let mut shared = size.shared();
    // A prior of part of a byte keeps a reference of its own if the part's
    // bits differ from the byte's: changes to the others mustn't look like
    // its changes.
    if shared != size && ty == OperandType::Prior && shared.mask() != size.mask() {
        shared = size;
    }
    let memref = if parse.indirect_parent.is_set() {
        let parent = parse.indirect_parent;
        parse.memrefs.derived(
            shared,
            &parent,
            Oper::IndirectRead,
            &Operand::constant(address),
        )
    } else {
        parse.memrefs.memory(address, shared)
    };
    Ok(Operand {
        ty,
        size,
        access: ty,
        memref: Some(memref),
        ..Operand::NONE
    })
}

fn parse_variable(s: &mut Cursor, parse: &Parse) -> Result<Operand, Error> {
    let mut name = String::new();
    while s.at(0) != b'}' {
        let c = s.at(0);
        let valid = if name.is_empty() {
            c.is_ascii_alphabetic()
        } else {
            c.is_ascii_alphanumeric()
        };
        if !valid || name.len() >= 15 {
            return Err(Error::InvalidVariableName);
        }
        name.push(c as char);
        s.skip(1);
    }
    if name.is_empty() {
        return Err(Error::InvalidVariableName);
    }
    s.skip(1);
    if name != "recall" {
        return Err(Error::UnknownVariableName);
    }
    let mut operand = if parse.remember.is_set() {
        Operand {
            is_combining: false,
            access: parse.remember.ty,
            ..parse.remember
        }
    } else {
        Operand {
            memref: None,
            size: Size::Bits32,
            access: OperandType::Address,
            ..Operand::NONE
        }
    };
    operand.ty = OperandType::Recall;
    Ok(operand)
}

/// An operand: `0xH1234`, `d0xH1234`, `5`, `h1F`, `-3`, `f1.5`,
/// `{recall}`.
pub fn parse_operand(s: &mut Cursor, parse: &mut Parse) -> Result<Operand, Error> {
    match s.at(0) {
        b'h' | b'H' => {
            if matches!(s.at(2), b'x' | b'X') {
                // H0x1234: either H1234 or 0xH1234 was meant.
                return Err(Error::InvalidConstOperand);
            }
            s.skip(1);
            let value = s.strtoul(16).ok_or(Error::InvalidConstOperand)?;
            Ok(Operand::constant(value.min(0xFFFF_FFFF) as u32))
        }
        b'f' | b'F' if s.at(1).is_ascii_alphabetic() => parse_memory(s, parse),
        b'f' | b'F' => {
            s.skip(1);
            parse_signed(s, true)
        }
        b'v' | b'V' => {
            s.skip(1);
            parse_signed(s, false)
        }
        b'+' | b'-' => parse_signed(s, false),
        b'{' => {
            s.skip(1);
            parse_variable(s, parse)
        }
        b'0' if !matches!(s.at(1), b'x' | b'X') => parse_unsigned(s),
        b'1'..=b'9' => parse_unsigned(s),
        b'@' => {
            // A function call, which never came to be.
            s.skip(1);
            if !s.at(0).is_ascii_alphabetic() {
                return Err(Error::InvalidFuncOperand);
            }
            while s.at(0).is_ascii_alphanumeric() || s.at(0) == b'_' {
                s.skip(1);
            }
            Ok(Operand {
                ty: OperandType::Func,
                size: Size::Bits32,
                access: OperandType::Address,
                ..Operand::NONE
            })
        }
        _ => parse_memory(s, parse),
    }
}

fn parse_unsigned(s: &mut Cursor) -> Result<Operand, Error> {
    let value = s.strtoul(10).ok_or(Error::InvalidConstOperand)?;
    Ok(Operand::constant(value.min(0xFFFF_FFFF) as u32))
}

/// A signed constant, or with `decimal` a float one: `-5`, `+3`, `1.25`.
fn parse_signed(s: &mut Cursor, decimal: bool) -> Result<Operand, Error> {
    let negative = s.at(0) == b'-';
    if matches!(s.at(0), b'-' | b'+') {
        s.skip(1);
    }
    let start = s.pos;
    let value = s.strtoul(10);
    if decimal && s.at(0) == b'.' {
        let value = value.unwrap_or(0);
        s.skip(1);
        if !s.at(0).is_ascii_digit() {
            return Err(Error::InvalidFpOperand);
        }
        // Parsed without the locale, keeping what fits in 32 bits.
        let (mut shift, mut fraction) = (1u64, 0u64);
        while s.at(0).is_ascii_digit() {
            if shift < 1_000_000_000 {
                fraction = fraction * 10 + (s.at(0) - b'0') as u64;
                shift *= 10;
            }
            s.skip(1);
        }
        let whole = if negative {
            -(value as i64) as f64
        } else {
            value as f64
        };
        let dbl = if fraction == 0 {
            whole
        } else if negative {
            whole - fraction as f64 / shift as f64
        } else {
            whole + fraction as f64 / shift as f64
        };
        return Ok(Operand::float_constant(dbl));
    }
    let Some(value) = value else {
        s.pos = start;
        return Err(if decimal {
            Error::InvalidFpOperand
        } else {
            Error::InvalidConstOperand
        });
    };
    let value = value.min(0x7FFF_FFFF) as i64;
    Ok(Operand::constant(if negative {
        (-value) as u32
    } else {
        value as u32
    }))
}
