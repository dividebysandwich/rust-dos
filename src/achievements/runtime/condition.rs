//! Conditions: `flag:operand oper operand (hits)`, such as
//! `R:0xH1234=5` (reset when the byte at 1234h is 5) or
//! `0xH00A2>d0xH00A2.10.` (the byte went up, ten times).

use super::memref::Size;
use super::operand::{Operand, OperandType, parse_operand};
use super::parse::{Cursor, Error, Parse};
use super::typed::{Oper, Typed};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CondType {
    Standard,
    PauseIf,
    ResetIf,
    MeasuredIf,
    Trigger,
    Measured,
    AddSource,
    SubSource,
    AddAddress,
    Remember,
    AddHits,
    SubHits,
    ResetNextIf,
    AndNext,
    OrNext,
}

impl CondType {
    /// Whether it feeds the condition after it rather than standing on
    /// its own.
    pub fn is_combining(self) -> bool {
        !matches!(
            self,
            CondType::Standard
                | CondType::PauseIf
                | CondType::ResetIf
                | CondType::MeasuredIf
                | CondType::Trigger
                | CondType::Measured
        )
    }
}

#[derive(Clone, Debug)]
pub struct Condition {
    pub ty: CondType,
    pub oper: Oper,
    pub operand1: Operand,
    pub operand2: Operand,
    pub required_hits: u32,
    pub current_hits: u32,
    /// Bit 0: true at the last check. Bit 1: this ResetIf reset the
    /// trigger.
    pub is_true: u8,
}

fn parse_operator(s: &mut Cursor) -> Result<Oper, Error> {
    let oper = match s.at(0) {
        b'=' => {
            s.skip(if s.at(1) == b'=' { 2 } else { 1 });
            return Ok(Oper::Eq);
        }
        b'!' if s.at(1) == b'=' => {
            s.skip(2);
            return Ok(Oper::Ne);
        }
        b'<' | b'>' if s.at(1) == b'=' => {
            let oper = if s.at(0) == b'<' { Oper::Le } else { Oper::Ge };
            s.skip(2);
            return Ok(oper);
        }
        b'<' => Oper::Lt,
        b'>' => Oper::Gt,
        b'*' => Oper::Mult,
        b'/' => Oper::Div,
        b'&' => Oper::And,
        b'^' => Oper::Xor,
        b'%' => Oper::Mod,
        b'+' => Oper::Add,
        b'-' => Oper::Sub,
        // The end of the condition: it may have no operator.
        0 | b'_' | b'S' | b')' | b'$' => return Ok(Oper::None),
        _ => return Err(Error::InvalidOperator),
    };
    s.skip(1);
    Ok(oper)
}

/// A hit count: `(10)` or `.10.`.
fn parse_hits(s: &mut Cursor, close: u8) -> Result<u32, Error> {
    s.skip(1);
    let hits = s.strtoul(10).ok_or(Error::InvalidRequiredHits)?;
    if s.at(0) != close {
        return Err(Error::InvalidRequiredHits);
    }
    s.skip(1);
    Ok(hits as u32)
}

impl Condition {
    pub fn parse(s: &mut Cursor, parse: &mut Parse) -> Result<Condition, Error> {
        let mut can_modify = false;
        let ty = if s.at(0) != 0 && s.at(1) == b':' {
            let ty = match s.at(0).to_ascii_lowercase() {
                b'p' => CondType::PauseIf,
                b'r' => CondType::ResetIf,
                b'a' => CondType::AddSource,
                b'b' => CondType::SubSource,
                b'c' => CondType::AddHits,
                b'd' => CondType::SubHits,
                b'n' => CondType::AndNext,
                b'o' => CondType::OrNext,
                b'm' => CondType::Measured,
                b'q' => CondType::MeasuredIf,
                b'i' => CondType::AddAddress,
                b't' => CondType::Trigger,
                b'k' => CondType::Remember,
                b'z' => CondType::ResetNextIf,
                b'g' => {
                    parse.measured_as_percent = true;
                    CondType::Measured
                }
                _ => return Err(Error::InvalidConditionType),
            };
            can_modify = matches!(
                ty,
                CondType::AddSource
                    | CondType::SubSource
                    | CondType::AddAddress
                    | CondType::Remember
            );
            s.skip(2);
            ty
        } else {
            CondType::Standard
        };
        let operand1 = parse_operand(s, parse)?;
        let mut oper = parse_operator(s)?;
        let mut condition = Condition {
            ty,
            oper,
            operand1,
            operand2: Operand::NONE,
            required_hits: 0,
            current_hits: 0,
            is_true: 0,
        };
        if oper == Oper::None {
            // Only modifiers, and Measured in a value, go without a
            // right side.
            if !can_modify && ty != CondType::Measured {
                return Err(Error::InvalidOperator);
            }
            condition.operand2 = Operand::constant(1);
            return Ok(condition);
        }
        if can_modify && !oper.is_modifying() {
            match ty {
                // An old definition that was a comparison before its type
                // changed.
                CondType::AddSource | CondType::SubSource | CondType::AddAddress => {
                    oper = Oper::None
                }
                _ => return Err(Error::InvalidOperator),
            }
        }
        condition.oper = oper;
        condition.operand2 = parse_operand(s, parse)?;
        if oper == Oper::None {
            condition.operand2 = Operand::constant(0);
        }
        let close = match s.at(0) {
            b'(' => Some(b')'),
            b'.' => Some(b'.'),
            _ => None,
        };
        if let Some(close) = close {
            let hits = parse_hits(s, close)?;
            if oper == Oper::None {
                condition.required_hits = 0;
            } else {
                condition.required_hits = hits;
                parse.has_required_hits = true;
            }
        }
        Ok(condition)
    }

    /// This condition as one operand: its left side, or its left side
    /// combined with its right as a derived reference.
    pub fn to_operand(&self, parse: &mut Parse) -> Operand {
        if self.oper == Oper::None {
            return self.operand1;
        }
        let float = self.operand1.is_float(parse.memrefs) || self.operand2.is_float(parse.memrefs);
        let size = if float { Size::Float } else { Size::Bits32 };
        let id = parse
            .memrefs
            .derived(size, &self.operand1, self.oper, &self.operand2);
        Operand {
            ty: OperandType::Address,
            access: OperandType::Address,
            memref: Some(id),
            size,
            ..self.operand1
        }
    }

    /// Carry the chains on: AddSource and SubSource add to the
    /// accumulator, AddAddress sets the pointer, Remember what `{recall}`
    /// reads, and the next condition that isn't one of them takes the
    /// chain as its left side.
    pub fn update_parse_state(&mut self, parse: &mut Parse) {
        match self.ty {
            CondType::AddAddress => {
                parse.indirect_parent = if self.oper != Oper::None {
                    self.to_operand(parse)
                } else {
                    self.operand1
                };
            }
            CondType::AddSource => {
                if !parse.addsource_parent.is_set() {
                    parse.addsource_parent = self.to_operand(parse);
                } else {
                    let size = if parse.addsource_parent.is_float(parse.memrefs) {
                        Size::Float
                    } else {
                        Size::Bits32
                    };
                    let mut operand = self.to_operand(parse);
                    operand.add_source(parse, size);
                    parse.addsource_parent = operand;
                }
                parse.addsource_oper = Oper::AddAccumulator;
                parse.indirect_parent = Operand::NONE;
            }
            CondType::SubSource => {
                if !parse.addsource_parent.is_set() {
                    parse.addsource_parent = self.to_operand(parse);
                    parse.addsource_oper = Oper::SubParent;
                } else {
                    let size = if parse.addsource_parent.is_float(parse.memrefs) {
                        Size::Float
                    } else {
                        Size::Bits32
                    };
                    if parse.addsource_oper == Oper::AddAccumulator
                        && !parse.addsource_parent.is_memref()
                    {
                        // A constant before: make it a reference by adding
                        // zero.
                        let parent = parse.addsource_parent;
                        let id = parse.memrefs.derived(
                            parent.size,
                            &parent,
                            Oper::AddAccumulator,
                            &Operand::constant(0),
                        );
                        parse.addsource_parent.memref = Some(id);
                        parse.addsource_parent.ty = OperandType::Address;
                    } else if parse.addsource_oper == Oper::SubParent {
                        // Two SubSources: start from zero minus the first.
                        let parent = parse.addsource_parent;
                        let zero = if parent.is_float(parse.memrefs) {
                            Operand::float_constant(0.0)
                        } else {
                            Operand::constant(0)
                        };
                        let id = parse.memrefs.derived(size, &parent, Oper::SubParent, &zero);
                        parse.addsource_parent.memref = Some(id);
                        parse.addsource_parent.size = zero.size;
                        match parent.ty {
                            OperandType::Const => {
                                parse.addsource_parent.num = parse.memrefs.derived_now(id)
                            }
                            OperandType::Fp => {
                                parse.addsource_parent.dbl = Typed {
                                    bits: parse.memrefs.derived_now(id),
                                    kind: super::typed::Kind::Float,
                                }
                                .f32()
                                    as f64
                            }
                            _ => {
                                parse.addsource_parent.ty = OperandType::Address;
                                parse.addsource_parent.access = OperandType::Address;
                            }
                        }
                    }
                    parse.addsource_oper = Oper::SubAccumulator;
                    let mut operand = self.to_operand(parse);
                    operand.add_source(parse, size);
                    parse.addsource_parent = operand;
                    parse.addsource_oper = Oper::AddAccumulator;
                }
                parse.indirect_parent = Operand::NONE;
            }
            CondType::Remember => {
                if self.operand1.ty == OperandType::Recall
                    && self.oper == Oper::None
                    && !parse.addsource_parent.is_set()
                    && !parse.indirect_parent.is_set()
                {
                    // Remembering {recall} as it is does nothing.
                    return;
                }
                self.operand1 = self.to_operand(parse);
                if parse.addsource_parent.is_set() {
                    let size = self.operand1.size;
                    self.operand1.add_source(parse, size);
                    self.operand1.is_combining = true;
                }
                parse.remember = self.operand1;
                parse.addsource_parent = Operand::NONE;
                parse.indirect_parent = Operand::NONE;
            }
            _ => {
                if self.ty == CondType::Measured
                    && parse.is_value
                    && matches!(
                        self.oper,
                        Oper::And
                            | Oper::Xor
                            | Oper::Div
                            | Oper::Mult
                            | Oper::Mod
                            | Oper::Add
                            | Oper::Sub
                    )
                {
                    // A value's Measured may modify its left side.
                    self.operand1 = self.to_operand(parse);
                }
                if parse.addsource_parent.is_set() {
                    if parse.addsource_oper == Oper::AddAccumulator {
                        parse.addsource_oper = Oper::Add;
                    }
                    let size = self.operand1.size;
                    self.operand1.add_source(parse, size);
                    self.operand1.is_combining = true;
                }
                parse.addsource_parent = Operand::NONE;
                parse.indirect_parent = Operand::NONE;
            }
        }
    }

    /// Whether the condition holds now. Operators that don't compare
    /// always do.
    pub fn test(&self, memrefs: &super::memref::Memrefs) -> bool {
        if !self.oper.is_comparison() {
            return true;
        }
        // rcheevos's shortcut for a value against its own delta: equal
        // when it didn't change, even with the sides read at different
        // sizes (`d0xH1234=0xT1234`), which sets are tested with.
        let plain = |o: &Operand| {
            matches!(o.ty, OperandType::Address | OperandType::Delta)
                && o.memref.is_some_and(|id| {
                    matches!(memrefs.get(id).source, super::memref::Source::Memory(_))
                })
                && !o.is_float(memrefs)
        };
        if plain(&self.operand1)
            && plain(&self.operand2)
            && self.operand1.ty != self.operand2.ty
            && self.operand1.memref == self.operand2.memref
            && let Some(id) = self.operand1.memref
            && !memrefs.get(id).value.changed
        {
            return matches!(self.oper, Oper::Eq | Oper::Ge | Oper::Le);
        }
        let value1 = self.operand1.evaluate(memrefs);
        let value2 = self.operand2.evaluate(memrefs);
        value1.compare(value2, self.oper)
    }
}
