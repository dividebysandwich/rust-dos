//! Values: what a leaderboard submits and a rich presence macro shows.
//! A value is the Measured of its clauses (the largest, with several
//! joined by `$`), or in the old format a sum of memory reads with
//! multipliers, `0xH1234*10_0xH1235`.

use super::condition::{CondType, Condition};
use super::condset::{Condset, EvalState};
use super::memref::{MemrefId, MemrefValue, Memrefs, Size};
use super::parse::{Cursor, Error, Parse};
use super::typed::{Kind, Typed};

#[derive(Clone, Debug)]
pub struct Value {
    pub conditions: Vec<Condset>,
    /// The last value, kept while the clauses are paused.
    pub value: MemrefValue,
}

/// A rich presence macro's value that isn't a plain memory read, worked
/// out every frame; `memref` is where operands read it.
#[derive(Clone, Debug)]
pub struct Variable {
    pub name: String,
    pub value: Value,
    pub memref: MemrefId,
}

impl Value {
    pub fn parse(s: &mut Cursor, parse: &mut Parse) -> Result<Value, Error> {
        let was_value = parse.is_value;
        parse.is_value = true;
        let result = if s.at(1) == b':' {
            parse_conditions(s, parse)
        } else {
            parse_legacy(s, parse)
        };
        parse.is_value = was_value;
        let conditions = result?;
        let mut value = Value {
            conditions,
            value: MemrefValue::new(Size::Bits32, Kind::Unsigned),
        };
        if let Some(measured) = value
            .conditions
            .first()
            .and_then(|c| c.in_order().find(|c| c.ty == CondType::Measured))
            && measured.operand1.is_float(parse.memrefs)
        {
            value.value.size = Size::Float;
            value.value.kind = Kind::Float;
        }
        Ok(value)
    }

    /// The value now, and whether any clause measured one.
    pub fn evaluate_typed(&mut self, memrefs: &Memrefs) -> Option<Typed> {
        let mut result: Option<Typed> = None;
        for condset in &mut self.conditions {
            let mut eval = EvalState::default();
            condset.test(&mut eval, memrefs);
            if condset.is_paused {
                continue;
            }
            if eval.was_reset {
                // In a value, a ResetIf resets its own clause alone.
                condset.reset();
            }
            if eval.measured_value.kind != Kind::None {
                match result {
                    None => result = Some(eval.measured_value),
                    Some(best) if eval.measured_value.compare(best, super::typed::Oper::Gt) => {
                        result = Some(eval.measured_value)
                    }
                    _ => {}
                }
            }
        }
        result
    }

    /// The value now as a signed integer; the last one while paused.
    pub fn evaluate(&mut self, memrefs: &Memrefs) -> i32 {
        let result = match self.evaluate_typed(memrefs) {
            Some(mut result) => {
                result.convert(Kind::Unsigned);
                self.value.update(result.bits);
                result
            }
            None => Typed::unsigned(self.value.value),
        };
        result.converted(Kind::Signed).i32()
    }

    pub fn reset(&mut self) {
        for condset in &mut self.conditions {
            condset.reset();
        }
        self.value.value = 0;
        self.value.prior = 0;
        self.value.changed = false;
    }

    /// Whether it counts hits rather than measuring a value.
    pub fn from_hits(&self) -> bool {
        for condset in &self.conditions {
            if let Some(measured) = condset.in_order().find(|c| c.ty == CondType::Measured) {
                return measured.required_hits != 0;
            }
        }
        false
    }
}

/// Clauses with flags (`M:0xH1234`), joined by `$` for the largest.
fn parse_conditions(s: &mut Cursor, parse: &mut Parse) -> Result<Vec<Condset>, Error> {
    let mut clauses = Vec::new();
    loop {
        parse.measured_target = 0;
        clauses.push(Condset::parse(s, parse)?);
        if matches!(s.at(0), b'S' | b's') {
            // Values have no alternatives.
            return Err(Error::InvalidValueFlag);
        } else if parse.measured_target == 0 {
            return Err(Error::MissingValueMeasured);
        } else if s.at(0) == b'$' {
            s.skip(1);
            continue;
        }
        return Ok(clauses);
    }
}

/// The old format, `0xH1234*10_0xH1235_v5$0x2000`: each clause made an
/// AddSource (SubSource for a negative multiplier), the last a Measured.
fn parse_legacy(s: &mut Cursor, parse: &mut Parse) -> Result<Vec<Condset>, Error> {
    let mut clauses = Vec::new();
    loop {
        let mut conditions: Vec<Condition> = Vec::new();
        loop {
            let mut kind = b'A';
            let mut text: Vec<u8> = Vec::new();
            loop {
                if text.len() >= 62 {
                    return Err(Error::InvalidValue);
                }
                match s.at(0) {
                    b'_' => break,
                    b'$' | 0 | b':' | b')' => {
                        if kind == b'A' {
                            kind = b'M';
                        }
                        break;
                    }
                    b'*' => {
                        text.push(b'*');
                        let mut ahead = s.pos + 1;
                        let at = |p: usize| s.text.get(p).copied().unwrap_or(0);
                        if at(ahead) == b'-' {
                            kind = b'B';
                            s.skip(1);
                            ahead += 1;
                        } else if at(ahead) == b'+' {
                            ahead += 1;
                        }
                        while at(ahead).is_ascii_digit() {
                            ahead += 1;
                        }
                        if at(ahead) == b'.' {
                            if text.len() >= 62 {
                                return Err(Error::InvalidValue);
                            }
                            text.push(b'f');
                        }
                        s.skip(1);
                    }
                    c => {
                        text.push(c);
                        s.skip(1);
                    }
                }
            }
            let mut definition = vec![kind, b':'];
            definition.extend_from_slice(&text);
            let definition = String::from_utf8_lossy(&definition).into_owned();
            let mut clause = Cursor::new(&definition);
            let mut condition = Condition::parse(&mut clause, parse)?;
            if !clause.at_end() {
                return Err(Error::InvalidValue);
            }
            if condition.ty == CondType::Measured && !condition.oper.is_modifying() {
                // A comparison here is ignored: the old format only adds.
                condition.oper = super::typed::Oper::None;
            }
            condition.update_parse_state(parse);
            conditions.push(condition);
            if s.at(0) != b'_' {
                break;
            }
            s.skip(1);
        }
        if conditions
            .last()
            .is_some_and(|c| c.ty != CondType::Measured)
        {
            let mut zero = Cursor::new("M:0");
            let mut condition = Condition::parse(&mut zero, parse)?;
            condition.update_parse_state(parse);
            conditions.push(condition);
        }
        let n = conditions.len();
        clauses.push(Condset {
            conditions,
            order: (0..n).collect(),
            num_measured: n,
            ..Condset::default()
        });
        if s.at(0) != b'$' {
            return Ok(clauses);
        }
        s.skip(1);
    }
}
