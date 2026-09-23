//! Arithmetic and logic with flag results, for 8, 16 and 32-bit operands.
//!
//! Operands are passed as u32 holding the zero-extended value; `size` is
//! the operand size in bytes. Results are masked to the operand size.

use super::{Cpu, CpuFlags};

pub const CF: u32 = 0x0001;
pub const PF: u32 = 0x0004;
pub const AF: u32 = 0x0010;
pub const ZF: u32 = 0x0040;
pub const SF: u32 = 0x0080;
pub const OF: u32 = 0x0800;
/// The flags arithmetic instructions set.
pub const ARITH: u32 = CF | PF | AF | ZF | SF | OF;

/// Mask of an operand of `size` bytes.
#[inline(always)]
pub fn size_mask(size: u8) -> u32 {
    match size {
        1 => 0xFF,
        2 => 0xFFFF,
        _ => 0xFFFF_FFFF,
    }
}

/// Sign bit of an operand of `size` bytes.
#[inline(always)]
pub fn sign_bit(size: u8) -> u32 {
    1 << (size as u32 * 8 - 1)
}

/// Sign-extend an operand of `size` bytes to 32 bits.
#[inline(always)]
pub fn sign_extend(size: u8, value: u32) -> u32 {
    match size {
        1 => value as u8 as i8 as i32 as u32,
        2 => value as u16 as i16 as i32 as u32,
        _ => value,
    }
}

/// SF, ZF and PF of a result.
#[inline(always)]
fn szp(size: u8, result: u32) -> u32 {
    let mut f = 0;
    if result & size_mask(size) == 0 {
        f |= ZF;
    }
    if result & sign_bit(size) != 0 {
        f |= SF;
    }
    if (result as u8).count_ones() % 2 == 0 {
        f |= PF;
    }
    f
}

/// Shift and rotate operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShiftOp {
    Rol,
    Ror,
    Rcl,
    Rcr,
    Shl,
    Shr,
    Sar,
}

impl Cpu {
    /// Replace the flags in `mask` with `bits`.
    #[inline(always)]
    pub fn set_flag_bits(&mut self, mask: u32, bits: u32) {
        let old = self.get_cpu_flags().bits();
        self.flags = CpuFlags::from_bits_retain((old & !mask) | (bits & mask));
    }

    #[inline(always)]
    fn flag_bits(&self) -> u32 {
        self.get_cpu_flags().bits()
    }

    /// Set PF from the low byte of `result`.
    pub fn update_pf(&mut self, result: u32) {
        let pf = if (result as u8).count_ones() % 2 == 0 { PF } else { 0 };
        self.set_flag_bits(PF, pf);
    }

    /// Set SF, ZF and PF from `result`, and clear CF, OF and AF, as the
    /// logic instructions do.
    #[inline(always)]
    pub fn alu_logic(&mut self, size: u8, result: u32) -> u32 {
        let result = result & size_mask(size);
        self.set_flag_bits(ARITH, szp(size, result));
        result
    }

    /// ADD, and ADC with `carry_in`.
    #[inline(always)]
    pub fn alu_add(&mut self, size: u8, a: u32, b: u32, carry_in: bool) -> u32 {
        let mask = size_mask(size);
        let wide = a as u64 + b as u64 + carry_in as u64;
        let r = wide as u32 & mask;
        let mut f = szp(size, r);
        if wide > mask as u64 {
            f |= CF;
        }
        if (a ^ r) & (b ^ r) & sign_bit(size) != 0 {
            f |= OF;
        }
        if (a ^ b ^ r) & 0x10 != 0 {
            f |= AF;
        }
        self.set_flag_bits(ARITH, f);
        r
    }

    /// SUB and CMP, and SBB with `borrow_in`.
    #[inline(always)]
    pub fn alu_sub(&mut self, size: u8, a: u32, b: u32, borrow_in: bool) -> u32 {
        let r = a.wrapping_sub(b).wrapping_sub(borrow_in as u32) & size_mask(size);
        let mut f = szp(size, r);
        if (a as u64) < b as u64 + borrow_in as u64 {
            f |= CF;
        }
        if (a ^ b) & (a ^ r) & sign_bit(size) != 0 {
            f |= OF;
        }
        if (a ^ b ^ r) & 0x10 != 0 {
            f |= AF;
        }
        self.set_flag_bits(ARITH, f);
        r
    }

    /// INC: ADD 1 that leaves CF alone.
    #[inline(always)]
    pub fn alu_inc(&mut self, size: u8, a: u32) -> u32 {
        let cf = self.flag_bits() & CF;
        let r = self.alu_add(size, a, 1, false);
        self.set_flag_bits(CF, cf);
        r
    }

    /// DEC: SUB 1 that leaves CF alone.
    #[inline(always)]
    pub fn alu_dec(&mut self, size: u8, a: u32) -> u32 {
        let cf = self.flag_bits() & CF;
        let r = self.alu_sub(size, a, 1, false);
        self.set_flag_bits(CF, cf);
        r
    }

    /// NEG: 0 - a, with CF set unless a is 0.
    #[inline(always)]
    pub fn alu_neg(&mut self, size: u8, a: u32) -> u32 {
        self.alu_sub(size, 0, a, false)
    }

    /// Shift or rotate `value` by `count`, which the caller has already
    /// masked to 5 bits. A count of 0 changes neither the value nor flags.
    pub fn alu_shift(&mut self, op: ShiftOp, size: u8, value: u32, count: u32) -> u32 {
        if count == 0 {
            return value;
        }
        let bits = size as u32 * 8;
        let mask = size_mask(size);
        let msb = sign_bit(size);
        let value = value & mask;
        let cf_in = self.flag_bits() & CF;

        // Shifting a byte or word by a multiple of its width past the
        // width itself (16 or 24 for a byte) leaves CF with the last bit
        // shifted out, as a shift by the width does; other counts past the
        // width shift CF out to 0. That's what a 386 does.
        let logical_count = if count > bits && count % bits == 0 { bits } else { count };
        let (r, cf, of, szp_flags) = match op {
            ShiftOp::Shl => {
                let wide = (value as u64) << logical_count;
                let r = wide as u32 & mask;
                let cf = ((wide >> bits) & 1) as u32;
                let of = ((r & msb != 0) as u32) ^ cf;
                (r, cf, of, true)
            }
            ShiftOp::Shr => {
                let r = ((value as u64) >> logical_count) as u32;
                let cf = ((value as u64 >> (logical_count - 1)) & 1) as u32;
                // The top two bits of the result differ: for a shift by 1,
                // the original sign bit.
                let of = ((r & msb != 0) as u32) ^ ((r & (msb >> 1) != 0) as u32);
                (r, cf, of, true)
            }
            ShiftOp::Sar => {
                let sv = sign_extend(size, value) as i32 as i64;
                let r = (sv >> count) as u32 & mask;
                let cf = ((sv >> (count - 1)) & 1) as u32;
                (r, cf, 0, true)
            }
            ShiftOp::Rol => {
                let n = count % bits;
                let r = if n == 0 { value } else { ((value << n) | (value >> (bits - n))) & mask };
                let cf = r & 1;
                let of = ((r & msb != 0) as u32) ^ cf;
                (r, cf, of, false)
            }
            ShiftOp::Ror => {
                let n = count % bits;
                let r = if n == 0 { value } else { ((value >> n) | (value << (bits - n))) & mask };
                let cf = (r & msb != 0) as u32;
                let of = cf ^ ((r & (msb >> 1) != 0) as u32);
                (r, cf, of, false)
            }
            ShiftOp::Rcl | ShiftOp::Rcr => {
                // Rotate through a (bits + 1)-bit value with CF on top.
                let n = count % (bits + 1);
                let width = bits + 1;
                let all = (1u64 << width) - 1;
                let wide = ((cf_in as u64) << bits) | value as u64;
                let rotated = if n == 0 {
                    wide
                } else if op == ShiftOp::Rcl {
                    ((wide << n) | (wide >> (width - n))) & all
                } else {
                    ((wide >> n) | (wide << (width - n))) & all
                };
                let r = rotated as u32 & mask;
                let cf = ((rotated >> bits) & 1) as u32;
                let of = if op == ShiftOp::Rcl {
                    ((r & msb != 0) as u32) ^ cf
                } else {
                    ((r & msb != 0) as u32) ^ ((r & (msb >> 1) != 0) as u32)
                };
                (r, cf, of, false)
            }
        };

        let mut f = if cf != 0 { CF } else { 0 };
        if of != 0 {
            f |= OF;
        }
        if szp_flags {
            // Shifts set SF, ZF and PF from the result; AF is undefined.
            self.set_flag_bits(CF | OF | SF | ZF | PF, f | szp(size, r));
        } else {
            // Rotates only change CF and OF.
            self.set_flag_bits(CF | OF, f);
        }
        r
    }

    /// SHLD/SHRD: shift `dest` by `count` (already masked to 5 bits),
    /// filling the vacated bits from `src`.
    pub fn alu_double_shift(&mut self, left: bool, size: u8, dest: u32, src: u32, count: u32) -> u32 {
        if count == 0 {
            return dest;
        }
        let bits = size as u32 * 8;
        let mask = size_mask(size);
        let msb = sign_bit(size);
        let (dest, src) = (dest & mask, src & mask);

        // A 386 shifts dest:src:src (SHLD) or src:src:dest (SHRD), so a
        // 16-bit operand shifted by 16 to 31 gets the source rotated in.
        let (r, cf, of) = if left {
            let wide = ((dest as u128) << (2 * bits)) | ((src as u128) << bits) | src as u128;
            let shifted = wide << count;
            let r = (shifted >> (2 * bits)) as u32 & mask;
            let cf = ((shifted >> (3 * bits)) & 1) as u32;
            (r, cf, ((r & msb != 0) as u32) ^ cf)
        } else {
            let wide = ((src as u128) << (2 * bits)) | ((src as u128) << bits) | dest as u128;
            let shifted = wide >> count;
            let r = shifted as u32 & mask;
            let cf = ((wide >> (count - 1)) & 1) as u32;
            (r, cf, ((r & msb != 0) as u32) ^ ((r & (msb >> 1) != 0) as u32))
        };
        let mut f = szp(size, r);
        if cf != 0 {
            f |= CF;
        }
        if of != 0 {
            f |= OF;
        }
        self.set_flag_bits(CF | OF | SF | ZF | PF, f);
        r
    }
}
