//! Values as the logic computes them: 32 bits that are an unsigned or a
//! signed integer or a float, and the arithmetic and comparisons between
//! them, with rcheevos's rules for mixing the kinds.

/// What the 32 bits of a value are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Kind {
    /// No value: a division by zero, say.
    #[default]
    None,
    Unsigned,
    Signed,
    Float,
}

/// The comparisons and the operators that combine values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Oper {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
    Ne,
    None,
    Mult,
    Div,
    And,
    Xor,
    Mod,
    Add,
    Sub,
    /// The negated parent plus the modifier (a SubSource chain's start).
    SubParent,
    /// The modifier, of the accumulator's kind, added to it.
    AddAccumulator,
    /// The modifier, of the accumulator's kind, taken from it.
    SubAccumulator,
    /// Memory read at the parent plus the modifier (AddAddress).
    IndirectRead,
}

impl Oper {
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            Oper::Eq | Oper::Lt | Oper::Le | Oper::Gt | Oper::Ge | Oper::Ne
        )
    }

    /// Whether it modifies a value rather than comparing it; "none" is
    /// "times one".
    pub fn is_modifying(self) -> bool {
        matches!(
            self,
            Oper::And
                | Oper::Xor
                | Oper::Div
                | Oper::Mult
                | Oper::Mod
                | Oper::Add
                | Oper::Sub
                | Oper::None
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Typed {
    pub bits: u32,
    pub kind: Kind,
}

impl Typed {
    pub const NONE: Typed = Typed {
        bits: 0,
        kind: Kind::None,
    };

    pub fn unsigned(value: u32) -> Self {
        Self {
            bits: value,
            kind: Kind::Unsigned,
        }
    }

    pub fn signed(value: i32) -> Self {
        Self {
            bits: value as u32,
            kind: Kind::Signed,
        }
    }

    pub fn float(value: f32) -> Self {
        Self {
            bits: value.to_bits(),
            kind: Kind::Float,
        }
    }

    pub fn u32(self) -> u32 {
        self.bits
    }

    pub fn i32(self) -> i32 {
        self.bits as i32
    }

    pub fn f32(self) -> f32 {
        f32::from_bits(self.bits)
    }

    /// The value as `kind`, as C's casts have it.
    pub fn converted(self, kind: Kind) -> Self {
        let mut value = self;
        value.convert(kind);
        value
    }

    pub fn convert(&mut self, kind: Kind) {
        if self.kind == kind {
            return;
        }
        self.bits = match kind {
            Kind::Unsigned => match self.kind {
                Kind::Signed => self.bits,
                Kind::Float => float_to_u32(self.f32() as f64),
                _ => 0,
            },
            Kind::Signed => match self.kind {
                Kind::Unsigned => self.bits,
                Kind::Float => float_to_i32(self.f32()) as u32,
                _ => 0,
            },
            Kind::Float => match self.kind {
                Kind::Unsigned => (self.bits as f32).to_bits(),
                Kind::Signed => (self.i32() as f32).to_bits(),
                _ => 0f32.to_bits(),
            },
            Kind::None => self.bits,
        };
        self.kind = kind;
    }

    pub fn negate(&mut self) {
        match self.kind {
            Kind::Unsigned => {
                self.convert(Kind::Signed);
                self.bits = self.i32().wrapping_neg() as u32;
            }
            Kind::Signed => self.bits = self.i32().wrapping_neg() as u32,
            Kind::Float => self.bits = (-self.f32()).to_bits(),
            Kind::None => {}
        }
    }

    pub fn add(&mut self, amount: Typed) {
        let mut amount = amount;
        if amount.kind != self.kind && self.kind != Kind::None {
            if amount.kind == Kind::Float {
                self.convert(Kind::Float);
            } else {
                amount.convert(self.kind);
            }
        }
        match self.kind {
            Kind::Unsigned => self.bits = self.bits.wrapping_add(amount.bits),
            Kind::Signed => self.bits = self.i32().wrapping_add(amount.i32()) as u32,
            Kind::Float => self.bits = (self.f32() + amount.f32()).to_bits(),
            Kind::None => *self = amount,
        }
    }

    pub fn multiply(&mut self, amount: Typed) {
        match self.kind {
            Kind::Unsigned => match amount.kind {
                // Unsigned multiplication wraps, which makes a negative
                // multiplier work through two's complement.
                Kind::Unsigned | Kind::Signed => self.bits = self.bits.wrapping_mul(amount.bits),
                Kind::Float => {
                    self.convert(Kind::Float);
                    self.bits = (self.f32() * amount.f32()).to_bits();
                }
                Kind::None => self.kind = Kind::None,
            },
            Kind::Signed => match amount.kind {
                Kind::Signed | Kind::Unsigned => {
                    self.bits = self.i32().wrapping_mul(amount.i32()) as u32
                }
                Kind::Float => {
                    self.convert(Kind::Float);
                    self.bits = (self.f32() * amount.f32()).to_bits();
                }
                Kind::None => self.kind = Kind::None,
            },
            Kind::Float => {
                if amount.kind == Kind::None {
                    self.kind = Kind::None;
                } else {
                    self.bits = (self.f32() * amount.converted(Kind::Float).f32()).to_bits();
                }
            }
            Kind::None => {}
        }
    }

    /// Division and remainder: integers stay integers, and anything by
    /// zero is no value.
    fn divide_with(&mut self, amount: Typed, modulus: bool) {
        let amount = match amount.kind {
            Kind::Unsigned | Kind::Signed => {
                if amount.bits == 0 {
                    self.kind = Kind::None;
                    return;
                }
                match (self.kind, amount.kind) {
                    (Kind::Unsigned, _) => {
                        self.bits = if modulus {
                            self.bits % amount.bits
                        } else {
                            self.bits / amount.bits
                        };
                        return;
                    }
                    (Kind::Signed, _) => {
                        let (a, b) = (self.i32(), amount.i32());
                        self.bits = if modulus {
                            a.wrapping_rem(b)
                        } else {
                            a.wrapping_div(b)
                        } as u32;
                        return;
                    }
                    (Kind::Float, _) => amount.converted(Kind::Float),
                    _ => {
                        self.kind = Kind::None;
                        return;
                    }
                }
            }
            Kind::Float => amount,
            Kind::None => {
                self.kind = Kind::None;
                return;
            }
        };
        if amount.f32() == 0.0 {
            self.kind = Kind::None;
            return;
        }
        self.convert(Kind::Float);
        let (a, b) = (self.f32(), amount.f32());
        self.bits = if modulus {
            (a as f64 % b as f64) as f32
        } else {
            a / b
        }
        .to_bits();
    }

    pub fn divide(&mut self, amount: Typed) {
        self.divide_with(amount, false);
    }

    pub fn modulus(&mut self, amount: Typed) {
        self.divide_with(amount, true);
    }

    /// `self oper amount`, for the modifying operators.
    pub fn combine(&mut self, amount: Typed, oper: Oper) {
        let mut amount = amount;
        match oper {
            Oper::Mult => self.multiply(amount),
            Oper::Div => self.divide(amount),
            Oper::And => {
                self.convert(Kind::Unsigned);
                self.bits &= amount.converted(Kind::Unsigned).bits;
            }
            Oper::Xor => {
                self.convert(Kind::Unsigned);
                self.bits ^= amount.converted(Kind::Unsigned).bits;
            }
            Oper::Mod => self.modulus(amount),
            Oper::Add => self.add(amount),
            Oper::Sub => {
                amount.negate();
                self.add(amount);
            }
            _ => {}
        }
    }

    /// `self oper other`: floats if either is one, else as `self`'s
    /// signedness. Not a comparison is true.
    pub fn compare(self, other: Typed, oper: Oper) -> bool {
        let (a, b) = if other.kind != self.kind {
            if other.kind == Kind::Float {
                (self.converted(Kind::Float), other)
            } else {
                (self, other.converted(self.kind))
            }
        } else {
            (self, other)
        };
        match a.kind {
            Kind::Unsigned => compare_ord(a.bits, b.bits, oper),
            Kind::Signed => compare_ord(a.i32(), b.i32(), oper),
            Kind::Float => compare_floats(a.f32(), b.f32(), oper),
            Kind::None => true,
        }
    }
}

/// C's `(unsigned)` of a float, as x86-64 does it: through a 64-bit
/// integer, so negatives wrap; what doesn't fit (and NaN) is 0.
pub fn float_to_u32(f: f64) -> u32 {
    if f.is_nan() || !(i64::MIN as f64..i64::MAX as f64).contains(&f) {
        0
    } else {
        (f as i64) as u32
    }
}

/// C's `(int)` of a float, as x86 does it: what doesn't fit (and NaN) is
/// the smallest integer.
pub fn float_to_i32(f: f32) -> i32 {
    if f.is_nan() || !(-2147483648.0..2147483648.0).contains(&f) {
        i32::MIN
    } else {
        f as i32
    }
}

pub fn compare_ord<T: PartialOrd>(a: T, b: T, oper: Oper) -> bool {
    match oper {
        Oper::Eq => a == b,
        Oper::Ne => a != b,
        Oper::Lt => a < b,
        Oper::Le => a <= b,
        Oper::Gt => a > b,
        Oper::Ge => a >= b,
        _ => true,
    }
}

/// Floats within seven significant digits of each other are equal. (As
/// C has it: a NaN is less than anything.)
fn compare_floats(f1: f32, f2: f32, oper: Oper) -> bool {
    if f1 != f2 {
        let abs = |f: f32| if f < 0.0 { -f } else { f };
        let threshold = if abs(f1) < abs(f2) { abs(f1) } else { abs(f2) } * f32::EPSILON;
        let diff = f1 - f2;
        // Not "greater than": a NaN difference isn't equal either.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        if !(abs(diff) <= threshold) {
            return if diff > threshold {
                matches!(oper, Oper::Ne | Oper::Gt | Oper::Ge)
            } else {
                matches!(oper, Oper::Ne | Oper::Lt | Oper::Le)
            };
        }
    }
    matches!(oper, Oper::Eq | Oper::Ge | Oper::Le)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_mix_as_rcheevos_does() {
        let mut v = Typed::unsigned(3);
        v.multiply(Typed::unsigned((-2i32) as u32));
        assert_eq!(v.i32(), -6);
        let mut v = Typed::unsigned(7);
        v.divide(Typed::unsigned(0));
        assert_eq!(v.kind, Kind::None);
        let mut v = Typed::unsigned(18);
        v.add(Typed::float(-17.5));
        assert_eq!((v.kind, v.f32()), (Kind::Float, 0.5));
        let mut v = Typed::unsigned(5);
        v.negate();
        assert_eq!((v.kind, v.i32()), (Kind::Signed, -5));
        assert!(Typed::signed(-1).compare(Typed::unsigned(0), Oper::Lt));
        assert!(!Typed::unsigned(u32::MAX).compare(Typed::signed(0), Oper::Lt));
        assert!(Typed::float(1.0).compare(Typed::float(1.0000001), Oper::Eq));
        assert!(Typed::float(1.0).compare(Typed::unsigned(2), Oper::Lt));
    }
}
