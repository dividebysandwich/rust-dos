//! Memory references: the addresses the logic reads, each once a frame
//! however many conditions read it, with the value it had the frame
//! before (delta) and the last value that differed (prior). Chains of
//! AddSource, SubSource, AddAddress and Remember become derived
//! references, worked out from others after them each frame.

use super::operand::{Operand, OperandType};
use super::typed::{Kind, Oper, Typed};

/// How a value is read from memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Bits8,
    Bits16,
    Bits24,
    Bits32,
    Low,
    High,
    Bit(u8),
    BitCount,
    Bits16Be,
    Bits24Be,
    Bits32Be,
    Float,
    Mbf32,
    Mbf32Le,
    FloatBe,
    Double32,
    Double32Be,
    /// A value of the logic's own (a rich presence macro's).
    Variable,
}

impl Size {
    /// How many bytes it reads.
    pub fn bytes(self) -> usize {
        match self {
            Size::Bits8 | Size::Low | Size::High | Size::Bit(_) | Size::BitCount => 1,
            Size::Bits16 | Size::Bits16Be => 2,
            Size::Bits24 | Size::Bits24Be => 3,
            Size::Variable => 0,
            _ => 4,
        }
    }

    /// The bits of what it reads that matter.
    pub fn mask(self) -> u32 {
        match self {
            Size::Bits8 | Size::BitCount => 0xFF,
            Size::Bits16 | Size::Bits16Be => 0xFFFF,
            Size::Bits24 | Size::Bits24Be => 0xFF_FFFF,
            Size::Low => 0x0F,
            Size::High => 0xF0,
            Size::Bit(n) => 1 << n,
            _ => 0xFFFF_FFFF,
        }
    }

    /// The size of the reference that serves it: a byte for the parts of
    /// one, 32 bits for 24 and the floats, and the little-endian read of
    /// the big-endian ones.
    pub fn shared(self) -> Size {
        match self {
            Size::Bits8 | Size::Low | Size::High | Size::Bit(_) | Size::BitCount => Size::Bits8,
            Size::Bits16 | Size::Bits16Be => Size::Bits16,
            Size::Variable => Size::Bits32,
            _ => Size::Bits32,
        }
    }

    pub fn is_float(self) -> bool {
        matches!(
            self,
            Size::Float
                | Size::FloatBe
                | Size::Double32
                | Size::Double32Be
                | Size::Mbf32
                | Size::Mbf32Le
        )
    }

    /// The value of `self` in `value`'s bits, which were read at the
    /// shared size.
    pub fn transform(self, value: &mut Typed) {
        let v = value.bits;
        value.bits = match self {
            Size::Bits8 => v & 0xFF,
            Size::Bits16 => v & 0xFFFF,
            Size::Bits24 => v & 0xFF_FFFF,
            Size::Bits32 | Size::Variable => v,
            Size::Bit(n) => (v >> n) & 1,
            Size::Low => v & 0x0F,
            Size::High => (v >> 4) & 0x0F,
            Size::BitCount => (v & 0xFF).count_ones(),
            Size::Bits16Be => ((v & 0xFF00) >> 8) | ((v & 0xFF) << 8),
            Size::Bits24Be => ((v & 0xFF_0000) >> 16) | (v & 0xFF00) | ((v & 0xFF) << 16),
            Size::Bits32Be => v.swap_bytes(),
            Size::Float => return *value = float(v),
            Size::FloatBe => return *value = float_be(v),
            Size::Double32 => return *value = double32(v),
            Size::Double32Be => return *value = double32_be(v),
            Size::Mbf32 => return *value = mbf32(v),
            Size::Mbf32Le => return *value = mbf32_le(v),
        };
    }
}

/// A float from its parts, as rcheevos builds it: a 23-bit mantissa
/// without its leading 1 and an unbiased exponent.
fn build_float(mantissa_bits: u32, exponent: i32, negative: bool) -> Typed {
    let implied = (1u32 << 23) as f64;
    let mut dbl = (mantissa_bits | 1 << 23) as f64 / implied;
    if exponent > 127 {
        dbl = if mantissa_bits == 0 {
            f64::INFINITY
        } else {
            f64::NAN
        };
    } else if exponent > 0 {
        dbl *= 2f64.powi(exponent);
    } else if exponent < 0 {
        if exponent == -127 {
            // All exponent bits zero: denormal, without the leading 1.
            dbl = mantissa_bits as f64 / implied;
            dbl /= 2f64.powi(126);
        } else {
            dbl /= 2f64.powi(-exponent);
        }
    }
    Typed::float(if negative { -dbl } else { dbl } as f32)
}

fn float(v: u32) -> Typed {
    build_float(
        v & 0x7F_FFFF,
        ((v >> 23) & 0xFF) as i32 - 127,
        v & 0x8000_0000 != 0,
    )
}

fn float_be(v: u32) -> Typed {
    let mantissa = ((v & 0xFF00_0000) >> 24) | ((v & 0xFF_0000) >> 8) | ((v & 0x7F00) << 8);
    let exponent = (((v & 0x7F) << 1) | ((v & 0x8000) >> 15)) as i32 - 127;
    build_float(mantissa, exponent, v & 0x80 != 0)
}

fn double32(v: u32) -> Typed {
    build_float(
        (v & 0xF_FFFF) << 3,
        ((v >> 20) & 0x7FF) as i32 - 1023,
        v & 0x8000_0000 != 0,
    )
}

fn double32_be(v: u32) -> Typed {
    let mantissa = (((v & 0xFF00_0000) >> 24) | ((v & 0xFF_0000) >> 8) | ((v & 0xF00) << 8)) << 3;
    let exponent = (((v & 0x7F) << 4) | ((v & 0xF000) >> 12)) as i32 - 1023;
    build_float(mantissa, exponent, v & 0x80 != 0)
}

/// Microsoft Binary Format, stored big-endian.
fn mbf32(v: u32) -> Typed {
    let mantissa = ((v & 0xFF00_0000) >> 24) | ((v & 0xFF_0000) >> 8) | ((v & 0x7F00) << 8);
    let exponent = (v & 0xFF) as i32 - 129;
    let negative = v & 0x8000 != 0;
    if mantissa == 0 && exponent == -129 {
        return Typed::float(if negative { -0.0 } else { 0.0 });
    }
    build_float(mantissa, exponent, negative)
}

/// Microsoft Binary Format, little-endian.
fn mbf32_le(v: u32) -> Typed {
    let mantissa = v & 0x7F_FFFF;
    let exponent = (v >> 24) as i32 - 129;
    let negative = v & 0x80_0000 != 0;
    if mantissa == 0 && exponent == -129 {
        return Typed::float(if negative { -0.0 } else { 0.0 });
    }
    build_float(mantissa, exponent, negative)
}

/// A reference's value now, the frame before and the last that differed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MemrefValue {
    pub value: u32,
    pub prior: u32,
    pub size: Size,
    /// Whether the value changed this frame.
    pub changed: bool,
    pub kind: Kind,
}

impl MemrefValue {
    pub fn new(size: Size, kind: Kind) -> Self {
        Self {
            value: 0,
            prior: 0,
            size,
            changed: false,
            kind,
        }
    }

    pub fn update(&mut self, value: u32) {
        if self.value == value {
            self.changed = false;
        } else {
            self.prior = self.value;
            self.value = value;
            self.changed = true;
        }
    }
}

/// Where a reference's value comes from.
#[derive(Clone, Debug)]
pub enum Source {
    Memory(u32),
    Derived(Derived),
    /// A rich presence macro's value, worked out with the rich presence.
    Variable,
}

/// A value worked out from others: `parent op modifier`.
#[derive(Clone, Debug)]
pub struct Derived {
    pub parent: Operand,
    pub modifier: Operand,
    pub op: Oper,
    pub depth: u16,
}

#[derive(Clone, Debug)]
pub struct Memref {
    pub value: MemrefValue,
    pub source: Source,
}

/// An index into `Memrefs`.
pub type MemrefId = usize;

/// Reads `bytes` bytes of the logic's memory at an address, little-endian;
/// what isn't there reads as zero.
pub trait Peek {
    fn peek(&self, address: u32, bytes: usize) -> u32;
}

impl<F: Fn(u32) -> u8> Peek for F {
    fn peek(&self, address: u32, bytes: usize) -> u32 {
        (0..bytes).fold(0, |v, i| {
            v | (self(address.wrapping_add(i as u32)) as u32) << (8 * i)
        })
    }
}

/// The value of `size` at `address`.
pub fn read_memory(peek: &dyn Peek, address: u32, size: Size) -> u32 {
    let bytes = size.bytes();
    if bytes == 0 {
        return 0;
    }
    peek.peek(address, bytes) & size.mask()
}

/// All the references of a game's logic.
#[derive(Clone, Debug, Default)]
pub struct Memrefs {
    pub items: Vec<Memref>,
}

impl Memrefs {
    pub fn get(&self, id: MemrefId) -> &Memref {
        &self.items[id]
    }

    /// The reference to `size` at `address`, shared with any that reads
    /// the same.
    pub fn memory(&mut self, address: u32, size: Size) -> MemrefId {
        if let Some(id) = self.items.iter().position(|m| {
            matches!(m.source, Source::Memory(a) if a == address) && m.value.size == size
        }) {
            return id;
        }
        self.items.push(Memref {
            value: MemrefValue::new(size, Kind::Unsigned),
            source: Source::Memory(address),
        });
        self.items.len() - 1
    }

    /// The reference to `parent op modifier`, shared with any that works
    /// out the same.
    pub fn derived(
        &mut self,
        size: Size,
        parent: &Operand,
        op: Oper,
        modifier: &Operand,
    ) -> MemrefId {
        let same = |m: &Memref| match &m.source {
            Source::Derived(d) => {
                m.value.size == size
                    && d.op == op
                    && self.operands_equal(&d.parent, parent)
                    && self.operands_equal(&d.modifier, modifier)
            }
            _ => false,
        };
        if let Some(id) = self.items.iter().position(same) {
            return id;
        }
        let depth = match parent.memref.map(|id| &self.items[id].source) {
            Some(Source::Derived(d)) if parent.is_memref() => d.depth + 1,
            _ => 0,
        };
        let kind = if size.is_float() {
            Kind::Float
        } else {
            Kind::Unsigned
        };
        self.items.push(Memref {
            value: MemrefValue::new(size, kind),
            source: Source::Derived(Derived {
                parent: *parent,
                modifier: *modifier,
                op,
                depth,
            }),
        });
        self.items.len() - 1
    }

    /// A rich presence macro's value.
    pub fn variable(&mut self) -> MemrefId {
        self.items.push(Memref {
            value: MemrefValue::new(Size::Variable, Kind::None),
            source: Source::Variable,
        });
        self.items.len() - 1
    }

    /// Whether two operands read the same, as rcheevos decides it for
    /// sharing references.
    pub fn operands_equal(&self, left: &Operand, right: &Operand) -> bool {
        if left.ty != right.ty {
            return false;
        }
        match left.ty {
            OperandType::Const => return left.num == right.num,
            OperandType::Fp => return left.dbl == right.dbl,
            OperandType::Recall => {
                if left.access != right.access {
                    return false;
                }
                match left.access {
                    OperandType::Const => return left.num == right.num,
                    OperandType::Fp => return left.dbl == right.dbl,
                    _ if left.memref.is_none() || right.memref.is_none() => return false,
                    _ => {}
                }
            }
            OperandType::Func | OperandType::None => return true,
            _ => {}
        }
        if left.size != right.size {
            return false;
        }
        let (Some(l), Some(r)) = (left.memref, right.memref) else {
            return false;
        };
        if l == r {
            return true;
        }
        let (l, r) = (&self.items[l], &self.items[r]);
        match (&l.source, &r.source) {
            (Source::Derived(a), Source::Derived(b)) => {
                a.op == b.op
                    && a.depth == b.depth
                    && self.operands_equal(&a.modifier, &b.modifier)
                    && self.operands_equal(&a.parent, &b.parent)
            }
            (Source::Memory(a), Source::Memory(b)) => a == b && l.value.size == r.value.size,
            _ => false,
        }
    }

    /// The value of the derived reference `id` from the others as they are.
    fn derived_value(&self, derived: &Derived, own: &MemrefValue, peek: &dyn Peek) -> u32 {
        let mut value = derived.parent.evaluate(self);
        let mut modifier = derived.modifier.evaluate(self);
        match derived.op {
            Oper::IndirectRead => {
                value.add(modifier);
                value.convert(Kind::Unsigned);
                return read_memory(peek, value.bits, own.size);
            }
            Oper::SubParent => {
                value.negate();
                value.add(modifier);
            }
            Oper::SubAccumulator | Oper::AddAccumulator => {
                if derived.op == Oper::SubAccumulator {
                    modifier.negate();
                }
                // The modifier takes the accumulator's kind: 18 - 17.5 is
                // 1 for an integer accumulator.
                modifier.convert(value.kind);
                value.add(modifier);
            }
            op => value.combine(modifier, op),
        }
        value.convert(own.kind);
        value.bits
    }

    /// Read memory for the frame: the addresses first, then the derived
    /// values in the order they were made, which is after what they use.
    pub fn update(&mut self, peek: &dyn Peek) {
        for memref in &mut self.items {
            if let Source::Memory(address) = memref.source
                && memref.value.kind != Kind::None
            {
                memref
                    .value
                    .update(read_memory(peek, address, memref.value.size));
            }
        }
        for i in 0..self.items.len() {
            if let Source::Derived(derived) = &self.items[i].source {
                let value = self.derived_value(derived, &self.items[i].value, peek);
                self.items[i].value.update(value);
            }
        }
    }

    /// The value of the derived reference `id` now, without memory: for
    /// constants folded while parsing.
    pub fn derived_now(&self, id: MemrefId) -> u32 {
        match &self.items[id].source {
            Source::Derived(derived) => {
                self.derived_value(derived, &self.items[id].value, &|_: u32| 0u8)
            }
            _ => self.items[id].value.value,
        }
    }
}
