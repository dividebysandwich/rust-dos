//! What parsing the logic's definitions shares: a cursor over the text,
//! the errors, and the state carried from one condition to the next.

use super::memref::Memrefs;
use super::operand::Operand;
use super::typed::Oper;

/// Why a definition can't be parsed, as rcheevos names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidFuncOperand,
    InvalidMemoryOperand,
    InvalidConstOperand,
    InvalidFpOperand,
    InvalidConditionType,
    InvalidOperator,
    InvalidRequiredHits,
    DuplicatedStart,
    DuplicatedCancel,
    DuplicatedSubmit,
    DuplicatedValue,
    DuplicatedProgress,
    MissingStart,
    MissingCancel,
    MissingSubmit,
    MissingValue,
    InvalidLboardField,
    MissingDisplayString,
    InvalidValueFlag,
    MissingValueMeasured,
    MultipleMeasured,
    InvalidMeasuredTarget,
    InvalidVariableName,
    UnknownVariableName,
    InvalidValue,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// A position in a definition's text; past its end reads as NUL, as C's
/// strings do.
#[derive(Clone, Copy, Debug)]
pub struct Cursor<'a> {
    pub text: &'a [u8],
    pub pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text: text.as_bytes(),
            pos: 0,
        }
    }

    pub fn at(&self, offset: usize) -> u8 {
        self.text.get(self.pos + offset).copied().unwrap_or(0)
    }

    /// The character here, and past it.
    pub fn take(&mut self) -> u8 {
        let c = self.at(0);
        self.pos += 1;
        c
    }

    pub fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    pub fn back(&mut self, n: usize) {
        self.pos -= n;
    }

    pub fn at_end(&self) -> bool {
        self.at(0) == 0
    }

    pub fn rest(&self) -> &'a [u8] {
        self.text.get(self.pos..).unwrap_or(&[])
    }

    /// C's `strtoul`: blanks, a sign, (with base 16) an optional 0x, then
    /// digits, saturating at the most a u64 holds. None without digits,
    /// and the cursor stays.
    pub fn strtoul(&mut self, base: u32) -> Option<u64> {
        let mut pos = self.pos;
        let at = |p: usize| self.text.get(p).copied().unwrap_or(0);
        while matches!(at(pos), b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C) {
            pos += 1;
        }
        let negative = at(pos) == b'-';
        if matches!(at(pos), b'-' | b'+') {
            pos += 1;
        }
        if base == 16
            && at(pos) == b'0'
            && matches!(at(pos + 1), b'x' | b'X')
            && (at(pos + 2) as char).is_ascii_hexdigit()
        {
            pos += 2;
        }
        let start = pos;
        let mut value: u64 = 0;
        let mut overflow = false;
        while let Some(d) = (at(pos) as char).to_digit(base) {
            match value
                .checked_mul(base as u64)
                .and_then(|v| v.checked_add(d as u64))
            {
                Some(v) => value = v,
                None => overflow = true,
            }
            pos += 1;
        }
        if pos == start {
            return None;
        }
        self.pos = pos;
        Some(if overflow {
            u64::MAX
        } else if negative {
            value.wrapping_neg()
        } else {
            value
        })
    }
}

/// What parsing carries from one condition to the next.
pub struct Parse<'m> {
    pub memrefs: &'m mut Memrefs,
    /// The rich presence's helper values, for its macros.
    pub variables: Option<&'m mut Vec<super::value::Variable>>,
    pub measured_target: u32,
    pub lines_read: usize,
    /// The AddSource/SubSource chain so far, and how the next condition
    /// joins it.
    pub addsource_parent: Operand,
    pub addsource_oper: Oper,
    /// The AddAddress pointer the next memory read is relative to.
    pub indirect_parent: Operand,
    /// What `{recall}` reads.
    pub remember: Operand,
    /// Parsing a value, not a trigger.
    pub is_value: bool,
    pub has_required_hits: bool,
    pub measured_as_percent: bool,
}

impl<'m> Parse<'m> {
    pub fn new(memrefs: &'m mut Memrefs) -> Self {
        Self {
            memrefs,
            variables: None,
            measured_target: 0,
            lines_read: 0,
            addsource_parent: Operand::NONE,
            addsource_oper: Oper::None,
            indirect_parent: Operand::NONE,
            remember: Operand::NONE,
            is_value: false,
            has_required_hits: false,
            measured_as_percent: false,
        }
    }
}
