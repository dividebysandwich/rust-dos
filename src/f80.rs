pub const FPU_80_BITS_MAX: u128 = (1u128 << 80) - 1;
pub const REAL_INDEFINITE: u128 = 0xffff_c000_0000_0000_0000 & FPU_80_BITS_MAX;
pub const QNAN: u128 = 0x7fff_c000_0000_0000_0000 & FPU_80_BITS_MAX;
pub const F80_PI: u128 = 0x4000c90fdaa22168c234 & FPU_80_BITS_MAX;
pub const SIGN_MASK: u128 = 1 << 79;
#[allow(dead_code)]
pub const EXP_MASK: u128 = 0x7FFF;
pub const EXP_MASK2: u128 = 0x7FFF << 64;
pub const MANTISSA_MASK: u128 = (1u128 << 64) - 1;
pub const MANTISSA_MASK_NOINT: u128 = 0x7FFF_FFFF_FFFF_FFFF;
pub const INT_BIT_MASK: u128 = 1 << 63;
pub const INT_BIT_MASK64: u64 = 1 << 63;
// Packed BCD keeps its sign in the top bit of the last byte.
pub const BCD_SIGN: u8 = 0x80;
/// The packed BCD indefinite FBSTP stores for what it can't.
pub const BCD_INDEFINITE: [u8; 10] = [0, 0, 0, 0, 0, 0, 0, 0xC0, 0xFF, 0xFF];
pub const F64_EXP_BIAS: i32 = 1023;
pub const F80_EXP_BIAS: i32 = 16383;
#[allow(dead_code)]
pub const MANTISSA_BITS: u32 = 64;
pub const SIGN_SHIFT: u32 = 79;
pub const EXP_SHIFT: u32 = 64;
pub const INT_BIT_SHIFT: u32 = 63;

// f80 emulation
// Source: https://github.com/sha0coder/mwemu
#[derive(Copy, Clone)]
pub struct F80 {
    pub st: u128,
}

impl Default for F80 {
    fn default() -> Self {
        Self::new()
    }
}

impl F80 {
    pub fn new() -> Self {
        F80 { st: 0 }
    }

    #[allow(dead_code)]
    #[allow(non_snake_case)]
    pub fn PI() -> Self {
        F80 { st: F80_PI }
    }

    #[allow(dead_code)]
    #[allow(non_snake_case)]
    pub fn QNaN() -> Self {
        F80 { st: QNAN }
    }

    #[allow(dead_code)]
    #[allow(non_snake_case)]
    pub fn set_PI(&mut self) {
        self.st = F80_PI;
    }

    #[allow(dead_code)]
    #[allow(non_snake_case)]
    pub fn set_QNaN(&mut self) {
        self.st = QNAN;
    }

    pub fn set_real_indefinite(&mut self) {
        self.st = REAL_INDEFINITE;
    }

    #[allow(dead_code)]
    pub fn is_invalid(&self) -> bool {
        let exp = self.get_exponent();
        let frac = self.get_fraction();

        exp == 0x7FFF && frac != 0
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.is_zero() && self.is_invalid()
    }

    pub fn set_bytes(&mut self, bytes: &[u8; 10]) {
        let mut val: u128 = 0;
        for (i, &b) in bytes.iter().enumerate() {
            val |= (b as u128) << (8 * i);
        }
        self.st = val & FPU_80_BITS_MAX;
    }

    pub fn get_bytes(&self) -> [u8; 10] {
        let mut bytes = [0u8; 10];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = ((self.st >> (8 * i)) & 0xFF) as u8;
        }
        bytes
    }

    #[allow(dead_code)]
    pub fn get_round_f64(&self, decimals: u32) -> f64 {
        let value = self.get_f64();
        let factor = 10f64.powi(decimals as i32);
        (value * factor).round() / factor
    }

    pub fn get(&self) -> u128 {
        self.st & FPU_80_BITS_MAX
    }

    pub fn set(&mut self, value: u128) {
        self.st = value & FPU_80_BITS_MAX;
    }

    #[allow(dead_code)]
    pub fn fix(&mut self) {
        self.st &= FPU_80_BITS_MAX;
    }

    pub fn is_negative(&self) -> bool {
        self.st & SIGN_MASK != 0
    }

    pub fn get_sign(&self) -> bool {
        (self.st >> SIGN_SHIFT) & 1 != 0
    }

    pub fn neg(&mut self) {
        self.st ^= SIGN_MASK;
    }

    pub fn set_sign(&mut self, sign: bool) {
        self.st = (self.st & !SIGN_MASK) | ((sign as u128) << 79);
    }

    pub fn is_zero(&self) -> bool {
        let exponent = (self.st >> 64) & 0x7FFF;
        let mantissa = self.st & ((1u128 << 64) - 1);
        exponent == 0 && mantissa == 0
    }

    pub fn get_integer_bit(&self) -> u64 {
        ((self.st >> INT_BIT_SHIFT) & 1) as u64
    }

    pub fn get_fraction(&self) -> u64 {
        (self.st & ((1u128 << 63) - 1)) as u64
    }

    pub fn get_exponent(&self) -> u16 {
        ((self.st >> EXP_SHIFT) & 0x7FFF) as u16
        //((self.st >> 64) & EXP_MASK) as u16
    }

    pub fn set_exponent(&mut self, exponent: u16) {
        self.st = (self.st & !EXP_MASK2) | ((exponent as u128) << 64);
    }

    pub fn get_mantissa(&self) -> u64 {
        (self.get_integer_bit() << 63) | self.get_fraction()
        /*
        let mantissa = (self.st & MANTISSA_MASK_NOINT) >> 11;
        let bit_integer = (self.st & INT_BIT_MASK) >> 63;
        ((bit_integer << 52) | mantissa) as u64*/
    }

    pub fn set_mantissa(&mut self, mantissa: u64) {
        let int_bit = (mantissa >> 63) as u128;
        let fraction = (mantissa & 0x7FFF_FFFF_FFFF_FFFF) as u128;
        self.st &= !((1u128 << INT_BIT_SHIFT) | ((1u128 << 63) - 1));
        self.st |= (int_bit << INT_BIT_SHIFT) | fraction;

        /*
        let bit_integer = ((mantissa >> 52) & 1) as u128;
        let fraction = (mantissa & 0x000F_FFFF_FFFF_FFFF) as u128; // solo los 52 bits bajos
        self.st &= !MANTISSA_MASK;
        self.st |= (bit_integer << 63) | (fraction << 11);*/
    }

    fn get_mantissa_with_integer_bit(&self) -> u64 {
        let mantissa = self.st & MANTISSA_MASK_NOINT;
        let int_bit = ((self.st & INT_BIT_MASK) >> 63) as u64;
        (mantissa as u64) | (int_bit << 63)
    }

    /// Add `b`, as 80-bit numbers: the smaller's significand shifted down
    /// to the larger's exponent (its low bits dropped), the significands
    /// added or subtracted, and the result brought back to 64 bits.
    /// True for an invalid operation (IE): a signalling NaN, or infinities
    /// of opposite signs, whose result is the real indefinite.
    pub fn add(&mut self, b: F80) -> bool {
        self.add_signed(b, b.get_sign())
    }

    /// Subtract `b`: add it with its sign turned.
    pub fn sub(&mut self, b: F80) -> bool {
        self.add_signed(b, !b.get_sign())
    }

    #[inline]
    fn add_signed(&mut self, b: F80, b_sign: bool) -> bool {
        if self.get_exponent() == 0x7FFF || b.get_exponent() == 0x7FFF {
            return self.add_special(b, b_sign);
        }
        let a_sign = self.get_sign();
        let (mut exp, b_exp) = (self.get_exponent(), b.get_exponent());
        let (mut a_man, mut b_man) = (self.st as u64, b.st as u64);
        let shifted = |man: u64, by: u16| if by >= 64 { 0 } else { man >> by };
        if exp > b_exp {
            b_man = shifted(b_man, exp - b_exp);
        } else if b_exp > exp {
            a_man = shifted(a_man, b_exp - exp);
            exp = b_exp;
        }
        let (sign, mut man);
        if a_sign == b_sign {
            let (sum, carry) = a_man.overflowing_add(b_man);
            if carry {
                man = sum >> 1 | INT_BIT_MASK64;
                exp = exp.wrapping_add(1);
            } else {
                man = sum;
            }
            sign = a_sign;
        } else {
            let difference = a_man.abs_diff(b_man);
            if difference == 0 {
                (sign, man, exp) = (false, 0, 0);
            } else {
                let up = difference.leading_zeros();
                man = difference << up;
                exp = exp.wrapping_sub(up as u16);
                sign = if a_man >= b_man { a_sign } else { b_sign };
            }
        }
        if exp >= 0x7FFF && a_sign == b_sign {
            // Overflow, masked: the infinity.
            (exp, man) = (0x7FFF, INT_BIT_MASK64);
        }
        // (An exponent that wrapped around below 0 reaches into the sign,
        // as it always did.)
        let st = (self.st & !SIGN_MASK) | (sign as u128) << SIGN_SHIFT;
        let st = (st & !EXP_MASK2) | (exp as u128) << EXP_SHIFT;
        self.st = (st & !MANTISSA_MASK) | man as u128;
        false
    }

    /// `add_signed` where either is a NaN or an infinity, as the FPU with
    /// its exceptions masked: a NaN goes through quieted (of two, the
    /// quiet one, else the larger significand, else the positive one),
    /// infinities of opposite signs make the real indefinite.
    #[cold]
    fn add_special(&mut self, b: F80, b_sign: bool) -> bool {
        const QUIET: u128 = 1 << 62;
        let mut b = b;
        b.set_sign(b_sign);
        let (a_nan, b_nan) = (self.is_nan(), b.is_nan());
        if a_nan || b_nan {
            let signalling = |x: &F80| x.is_nan() && x.st & QUIET == 0;
            let invalid = signalling(self) || signalling(&b);
            if a_nan && b_nan {
                let (a_quiet, b_quiet) = (!signalling(self), !signalling(&b));
                let (a_man, b_man) = (self.st as u64 & !(1 << 62), b.st as u64 & !(1 << 62));
                let take_b = if a_quiet != b_quiet {
                    b_quiet
                } else if a_man != b_man {
                    b_man > a_man
                } else {
                    !b.get_sign()
                };
                if take_b {
                    *self = b;
                }
            } else if b_nan {
                *self = b;
            }
            self.st |= QUIET | INT_BIT_MASK;
            return invalid;
        }
        if self.is_infinite() && b.is_infinite() {
            if self.get_sign() != b.get_sign() {
                self.set_real_indefinite();
                return true;
            }
        } else if b.is_infinite() {
            *self = b;
        }
        false
    }

    #[allow(dead_code)]
    pub fn is_integer(&self) -> bool {
        self.get_mantissa() == 0
    }

    #[allow(dead_code)]
    pub fn is_normal(&self) -> bool {
        let exponent = (self.st >> 64) & 0x7FFF;
        exponent != 0 && exponent != 0x7FFF
    }

    #[allow(dead_code)]
    pub fn bit_integer(&self) -> bool {
        (self.st & INT_BIT_MASK) & 1 == 1
    }

    pub fn is_denormal(&self) -> bool {
        let exponent = (self.st >> 64) & 0x7FFF;
        exponent == 0
    }

    /// Exponent 7FFF and a fraction of 0, the integer bit set (as the FPU
    /// makes it) or not (a pseudo-infinity).
    pub fn is_infinite(&self) -> bool {
        let exponent = (self.st >> 64) & 0x7FFF;
        exponent == 0x7FFF && self.st & MANTISSA_MASK_NOINT == 0
    }

    /// Exponent 7FFF and a fraction other than 0, whatever the integer bit.
    pub fn is_nan(&self) -> bool {
        let exponent = (self.st >> 64) & 0x7FFF;
        exponent == 0x7FFF && self.st & MANTISSA_MASK_NOINT != 0
    }

    /// The value as FBSTP stores it, rounded as `rc` (the control word's RC
    /// field) says: None for what doesn't fit 18 digits, an infinity or a
    /// NaN, where the FPU stores the packed BCD indefinite
    /// (`BCD_INDEFINITE`).
    pub fn to_bcd_packed(&self, rc: u16) -> Option<[u8; 10]> {
        let mut val = self
            .to_integer(rc)
            .filter(|&n| n <= 999_999_999_999_999_999)?;
        let mut bcd = [0u8; 10];

        for byte in &mut bcd[..9] {
            let lo = (val % 10) as u8;
            val /= 10;
            let hi = (val % 10) as u8;
            val /= 10;

            *byte = (hi << 4) | lo;
        }

        bcd[9] = if self.is_negative() { BCD_SIGN } else { 0 };

        Some(bcd)
    }

    pub fn from_bcd_packed(&mut self, bcd: &[u8; 10]) {
        let mut value: u128 = 0;

        // Digits above 9 give undefined results on an FPU: take them at
        // face value rather than fault.
        for i in (0..9).rev() {
            let byte = bcd[i];
            let hi = (byte >> 4) & 0x0F;
            let lo = byte & 0x0F;

            value = value * 100 + (hi as u128) * 10 + (lo as u128);
        }

        // The other bits of the sign byte are ignored.
        let is_negative = bcd[9] & BCD_SIGN != 0;

        self.st = F80::encode_from_u128(value, is_negative);
    }

    /// The magnitude of the value rounded to an integer as `rc` (the
    /// control word's RC field: nearest with ties to even, down, up, toward
    /// 0) says, from all 64 bits of the significand; None for an infinity,
    /// a NaN, or 2^64 and more.
    pub fn to_integer(&self, rc: u16) -> Option<u64> {
        let exp = self.get_exponent();
        if exp == 0x7FFF {
            return None;
        }
        // (A denormal's exponent is that of the smallest normal.)
        let e = exp.max(1) as i32 - F80_EXP_BIAS;
        if e >= 64 {
            return None;
        }
        let man = self.get_mantissa_with_integer_bit();
        // The integer part, and the fraction as 64 bits below the point,
        // with bit 0 set for any bits shifted out below them.
        let (int, frac) = if e >= 63 {
            (man, 0)
        } else if e >= 0 {
            let s = 63 - e as u32;
            (man >> s, man << (64 - s))
        } else {
            let s = (-e - 1) as u32;
            if s >= 64 {
                (0, (man != 0) as u64)
            } else {
                (0, man >> s | (man & ((1u64 << s) - 1) != 0) as u64)
            }
        };
        let half = 1u64 << 63;
        let up = match rc & 3 {
            0 => frac > half || (frac == half && int & 1 != 0),
            1 => self.is_negative() && frac != 0,
            2 => !self.is_negative() && frac != 0,
            _ => false,
        };
        int.checked_add(up as u64)
    }

    pub fn encode_from_u128(value: u128, is_negative: bool) -> u128 {
        if value == 0 {
            return if is_negative { 1u128 << 79 } else { 0 };
        }

        let msb = 127 - value.leading_zeros() as u16;
        let exponent = 16383 + msb;
        let shift = msb as i32 - 63;

        let mantissa = if shift > 0 {
            value >> shift
        } else {
            value << (-shift) as u32
        };

        let sign_bit = if is_negative { 1u128 << 79 } else { 0 };
        let exp_bits = (exponent as u128) << 64;
        let mantissa_bits = mantissa & 0xFFFFFFFFFFFFFFFF;

        sign_bit | exp_bits | mantissa_bits
    }

    pub fn set_f64(&mut self, value: f64) {
        let bits = value.to_bits();
        let sign = (bits >> 63) & 1;
        let exp = ((bits >> 52) & 0x7FF) as u16;
        let mantissa = bits & 0xFFFFFFFFFFFFF;

        let st = if exp == 0 {
            // Subnormal or zero in f64 → represent as 0.0 en FPU
            (sign as u128) << 79
        } else if exp == 0x7FF {
            // An infinity or a NaN (made quiet), with the integer bit set:
            // without it, it is a pseudo-infinity or pseudo-NaN, which a
            // 387 and later don't take (DJGPP's printf says "nan").
            let is_nan = mantissa != 0;
            let extended_exp = 0x7FFFu128;
            let extended_mantissa = (mantissa as u128) << 11;
            let nan_bit = if is_nan { 1u128 << 62 } else { 0 };
            ((sign as u128) << 79) | (extended_exp << 64) | INT_BIT_MASK | extended_mantissa | nan_bit
        } else {
            // Normal number
            let adjusted_exp = (exp as i32 - F64_EXP_BIAS) + F80_EXP_BIAS;
            let extended_exp = adjusted_exp as u128;
            let full_mantissa = ((1u64 << 52) | mantissa) as u128; // add implicit bit
            let extended_mantissa = full_mantissa << 11; // 63 bits align
            ((sign as u128) << 79) | (extended_exp << 64) | extended_mantissa
        };

        self.set(st); // masked setter
    }

    pub fn get_f64(&self) -> f64 {
        let value = self.get();
        let sign = ((value >> 79) & 1) as u64;
        let exp = ((value >> 64) & 0x7FFF) as u16;
        let mantissa = value & 0xFFFFFFFFFFFFFFFF;

        let f64_bits: u64 = if exp == 0 {
            // Zero or denormal extended → 0.0
            sign << 63
        } else if exp == 0x7FFF {
            // Inf or NaN, by the fraction (not the integer bit)
            let f64_mantissa = ((mantissa & MANTISSA_MASK_NOINT) >> 11) as u64;
            let nan_mask = if mantissa & MANTISSA_MASK_NOINT != 0 { 1 << 51 } else { 0 }; // set QNaN bit if needed
            (sign << 63) | (0x7FF << 52) | f64_mantissa | nan_mask
        } else {
            let unbiased_exp = exp as i32 - F80_EXP_BIAS;
            let f64_exp = unbiased_exp + F64_EXP_BIAS;

            if f64_exp <= 0 {
                // Subnormal in f64 → round to 0.0
                sign << 63
            } else if f64_exp >= 0x7FF {
                // Exponent too large → ∞
                (sign << 63) | (0x7FF << 52)
            } else {
                let f64_mantissa = ((mantissa >> 11) & 0xFFFFFFFFFFFFF) as u64;
                (sign << 63) | (((f64_exp as u64) << 52 | f64_mantissa) + round_up(mantissa as u64) as u64)
            }
        };

        f64::from_bits(f64_bits)
    }

    /// Sign, unbiased exponent and significand of a normal number (an
    /// exponent neither 0 nor all ones, and the integer bit set); None for
    /// anything else.
    fn normal_parts(&self) -> Option<(bool, i32, u64)> {
        let exp = self.get_exponent();
        let mantissa = self.st as u64;
        (exp != 0 && exp != 0x7FFF && mantissa & INT_BIT_MASK64 != 0)
            .then(|| (self.get_sign(), exp as i32 - F80_EXP_BIAS, mantissa))
    }

    /// The value rounded to an integer as control word rounding `rc` says
    /// (0 nearest even, 1 down, 2 up, 3 toward zero), from all 64 bits of
    /// the significand. None for anything but a normal number under 2^63
    /// (a larger one is an integer already).
    pub fn round_int(&self, rc: u16) -> Option<i128> {
        let (sign, exp, mantissa) = self.normal_parts()?;
        if exp >= 63 {
            return None;
        }
        let (int, rest, half) = if exp < -64 {
            // Under 2^-64: below a half, and not 0.
            (0, 1, 2)
        } else {
            let shift = (63 - exp) as u32;
            let m = mantissa as u128;
            (m >> shift, m & ((1u128 << shift) - 1), 1u128 << (shift - 1))
        };
        let int = int + rounds_away(rc, sign, int & 1 != 0, rest, half) as u128;
        Some(if sign { -(int as i128) } else { int as i128 })
    }

    /// The value rounded to a significand of `bits` bits (24 for a single,
    /// 53 for a double) as control word rounding `rc` says, as the double
    /// that holds it exactly. None for anything but a normal number, or
    /// where the result is outside the normal range of the format with
    /// `bits` (a single's exponents -126 to 127, a double's -1022 to 1023).
    pub fn round_to(&self, bits: u32, rc: u16) -> Option<f64> {
        let (sign, mut exp, mantissa) = self.normal_parts()?;
        let drop = 64 - bits;
        let mut kept = mantissa >> drop;
        let rest = (mantissa & ((1u64 << drop) - 1)) as u128;
        kept += rounds_away(rc, sign, kept & 1 != 0, rest, 1u128 << (drop - 1)) as u64;
        if kept >> bits != 0 {
            kept >>= 1;
            exp += 1;
        }
        let max = if bits == 24 { 127 } else { 1023 };
        if exp > max || exp < 1 - max {
            return None;
        }
        let magnitude = kept as f64 * pow2(exp - (bits as i32 - 1));
        Some(if sign { -magnitude } else { magnitude })
    }

    /// The value as a single's bits, as an x87 stores it (FST m32): its 64
    /// bits of significand rounded once, as control word rounding `rc`
    /// says. Below a single's normal range it rounds to a denormal or zero,
    /// and above it to infinity, or to the largest single where the
    /// rounding goes toward zero (chop, down for a positive value, up for
    /// a negative one). A denormal is far below a single's least denormal,
    /// and rounds to it or to zero. None for anything but a normal number,
    /// a denormal or a pseudo-denormal.
    pub fn single_bits(&self, rc: u16) -> Option<u32> {
        let (sign, mut exp, mantissa) = match self.normal_parts() {
            Some(parts) => parts,
            // (Its exponent as the least normal's: any is far enough
            // under a single's range.)
            None if self.get_exponent() == 0 && self.st as u64 != 0 => {
                (self.get_sign(), 1 - F80_EXP_BIAS, self.st as u64)
            }
            None => return None,
        };
        let sign_bit = (sign as u32) << 31;
        // The significand's bits below the single's last place: 40 in the
        // normal range, more below it, where a denormal has fewer.
        let shift = 40 + (-126 - exp).max(0) as u32;
        let (mut kept, rest, half) = if shift >= 128 {
            // Far under the least denormal: below a half, and not 0.
            (0u128, 1u128, 2u128)
        } else {
            let m = mantissa as u128;
            (m >> shift, m & ((1u128 << shift) - 1), 1u128 << (shift - 1))
        };
        kept += rounds_away(rc, sign, kept & 1 != 0, rest, half) as u128;
        if shift == 40 && kept >> 24 != 0 {
            kept >>= 1;
            exp += 1;
        }
        if exp > 127 {
            let largest = match rc {
                0 => false,
                1 => !sign,
                2 => sign,
                _ => true,
            };
            return Some(sign_bit | if largest { 0x7F7F_FFFF } else { 0x7F80_0000 });
        }
        if shift > 40 {
            // A denormal's bits are its significand, and a carry out of
            // it makes the least normal, whose bits are the same number.
            return Some(sign_bit | kept as u32);
        }
        Some(sign_bit | ((exp + 127) as u32) << 23 | (kept as u32 & 0x7F_FFFF))
    }

    /// Compare with `other` by their 80 bits, where both are normal numbers
    /// or zeros; None otherwise.
    pub fn compare(&self, other: &F80) -> Option<std::cmp::Ordering> {
        use std::cmp::Ordering;
        // A zero as (sign, 0), a normal number as (sign, exponent and
        // significand), which order as the magnitudes do.
        let key = |f: &F80| -> Option<(bool, u128)> {
            if f.is_zero() {
                Some((false, 0))
            } else {
                f.normal_parts().map(|_| (f.get_sign(), f.st & !SIGN_MASK))
            }
        };
        let ((a_sign, a), (b_sign, b)) = (key(self)?, key(other)?);
        Some(match (a_sign, b_sign) {
            (false, false) => a.cmp(&b),
            (true, true) => b.cmp(&a),
            // (A zero counts as positive here, so a negative number is
            // below it.)
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        })
    }
}

/// Whether a value rounds away from zero to the next integer (or the next
/// significand): `rest` is what is cut off, `half` what a half is in the
/// same units, `odd` whether the part kept is odd. Rounding `rc` as the
/// control word has it (0 nearest even, 1 down, 2 up, 3 toward zero).
fn rounds_away(rc: u16, negative: bool, odd: bool, rest: u128, half: u128) -> bool {
    match rc {
        0 => rest > half || (rest == half && odd),
        1 => negative && rest != 0,
        2 => !negative && rest != 0,
        _ => false,
    }
}

/// 2^k, for k from -1074 to 1023.
fn pow2(k: i32) -> f64 {
    if k >= -1022 {
        f64::from_bits(((k + 1023) as u64) << 52)
    } else {
        f64::from_bits(1u64 << (k + 1074))
    }
}

crate::state_fields!(F80 { st });

/// Whether a 64-bit significand cut to a double's 53 bits rounds up, to
/// nearest with ties to even (the x87's default rounding): the 11 bits
/// dropped are over half, or exactly half with the last bit kept odd.
/// Added to the double's bits, a carry out of the significand goes on into
/// the exponent, and past the largest double makes the infinity.
#[inline]
fn round_up(mantissa: u64) -> bool {
    let dropped = mantissa & 0x7FF;
    dropped > 0x400 || (dropped == 0x400 && mantissa & 0x800 != 0)
}

/// What a double is after it went into an FPU register and came back
/// (`F80::set_f64`, then `get_f64`): a denormal is 0, and a NaN quiet.
#[inline]
pub fn canon_f64(value: f64) -> f64 {
    let bits = value.to_bits();
    let exp = bits >> 52 & 0x7FF;
    let canon = if exp == 0 {
        bits & 1 << 63
    } else if exp == 0x7FF && bits << 12 != 0 {
        bits | 1 << 51
    } else {
        bits
    };
    f64::from_bits(canon)
}

/// The FPU's eight registers, by their physical numbers. Each holds its
/// 80 bits, and next to them the double they come to (`F80::get_f64`),
/// which most instructions compute with. A register an instruction gave
/// a double (`set_f64`) keeps only that until something asks for its 80
/// bits (`stale`): they are the double's (`F80::set_f64`), made then.
/// The dynamic recompiler's code reads and writes the three arrays where
/// they are (`cpu::layout`).
#[derive(Clone)]
#[repr(C)]
pub struct FpuRegs {
    x80: [F80; 8],
    f64s: [f64; 8],
    stale: [u8; 8],
}

/// `FpuRegs` as bits, which compare as stored.
#[derive(Clone, PartialEq)]
pub struct FpuBits {
    x80: [u128; 8],
    f64s: [u64; 8],
    stale: [u8; 8],
}

impl Default for FpuRegs {
    fn default() -> Self {
        FpuRegs { x80: [F80::new(); 8], f64s: [0.0; 8], stale: [0; 8] }
    }
}

impl FpuRegs {
    pub const X80_OFFSET: usize = std::mem::offset_of!(FpuRegs, x80);
    pub const F64_OFFSET: usize = std::mem::offset_of!(FpuRegs, f64s);
    pub const STALE_OFFSET: usize = std::mem::offset_of!(FpuRegs, stale);

    /// The registers' bits, as stored (for comparing states).
    pub fn bits(&self) -> FpuBits {
        FpuBits { x80: self.x80.map(|f| f.st), f64s: self.f64s.map(f64::to_bits), stale: self.stale }
    }

    /// Register `i`'s 80 bits.
    #[inline]
    pub fn get(&self, i: usize) -> F80 {
        if self.stale[i] != 0 {
            let mut f = F80::new();
            f.set_f64(self.f64s[i]);
            f
        } else {
            self.x80[i]
        }
    }

    #[inline]
    pub fn set(&mut self, i: usize, value: F80) {
        self.x80[i] = value;
        self.f64s[i] = value.get_f64();
        self.stale[i] = 0;
    }

    /// Register `i` as a double.
    #[inline]
    pub fn get_f64(&self, i: usize) -> f64 {
        self.f64s[i]
    }

    /// Whether register `i`'s 80 bits hold more than its double does: it
    /// isn't stale and has significand bits below a double's 53. The double
    /// is rounded to nearest, so FIST, FST to a single (or to a double under
    /// another rounding), FRNDINT and the compares then work from the 80
    /// bits; the recompiler's code leaves those cases to the handlers
    /// (`Uop::FBailPrecise`, and `Uop::FToSingle` for the single).
    #[inline]
    pub fn beyond_double(&self, i: usize) -> bool {
        self.stale[i] == 0 && self.x80[i].st & 0x7FF != 0
    }

    /// Give register `i` a double: what `set` of an `F80` set to it
    /// (`F80::set_f64`) leaves.
    #[inline]
    pub fn set_f64(&mut self, i: usize, value: f64) {
        self.f64s[i] = canon_f64(value);
        self.stale[i] = 1;
    }
}

impl crate::savestate::State for FpuRegs {
    /// As the eight registers' 80 bits.
    fn save(&self, w: &mut crate::savestate::Writer) {
        let x80: [F80; 8] = std::array::from_fn(|i| self.get(i));
        crate::savestate::State::save(&x80, w);
    }

    fn load(&mut self, r: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        let mut x80 = [F80::new(); 8];
        crate::savestate::State::load(&mut x80, r)?;
        for (i, value) in x80.into_iter().enumerate() {
            self.set(i, value);
        }
        Ok(())
    }
}


/// `F80::add` and `F80::sub` as they were first written, a bit at a time,
/// for the tests to compare with.
#[cfg(test)]
impl F80 {
    fn normalize_add(&self, mantissa: u64, exponent: u16) -> (u64, u16) {
        let mut man = mantissa;
        let mut exp = exponent;
        if man == 0 {
            return (0, 0);
        }
        while (man & INT_BIT_MASK64) == 0 {
            man <<= 1;
            exp = exp.wrapping_sub(1);
        }
        (man, exp)
    }

    fn add_reference(&mut self, b: F80) {
        let a_sign = self.get_sign();
        let b_sign = b.get_sign();

        let mut a_exp = self.get_exponent();
        let b_exp = b.get_exponent();

        let mut a_man = self.get_mantissa() as u128;
        let mut b_man = b.get_mantissa() as u128;

        // Clamp the shift amount. 
        // Shifting a u128 by >= 128 causes a Rust panic.
        if a_exp > b_exp {
            let shift = (a_exp - b_exp) as u32;
            if shift >= 128 {
                b_man = 0; // Underflow: b is negligible compared to a
            } else {
                b_man >>= shift;
            }
        } else if b_exp > a_exp {
            let shift = (b_exp - a_exp) as u32;
            if shift >= 128 {
                a_man = 0; // Underflow: a is negligible compared to b
            } else {
                a_man >>= shift;
            }
            a_exp = b_exp;
        }

        let mut result_sign;
        let mut result_exp = a_exp;
        let mut result_man: u128;

        if a_sign == b_sign {
            result_man = a_man + b_man;
            if (result_man & (1u128 << 64)) != 0 {
                result_man >>= 1;
                result_exp = result_exp.wrapping_add(1);
            }
            result_sign = a_sign;
        } else {
            if a_man >= b_man {
                result_man = a_man - b_man;
                result_sign = a_sign;
            } else {
                result_man = b_man - a_man;
                result_sign = b_sign;
            }
            let (norm_man, norm_exp) = self.normalize_add(result_man as u64, result_exp);
            result_man = norm_man as u128;
            result_exp = norm_exp;

            if result_man == 0 {
                result_sign = false;
                result_exp = 0;
            }
        }

        self.set_sign(result_sign);
        self.set_exponent(result_exp);
        self.set_mantissa(result_man as u64);
    }

    fn sub_reference(&mut self, b: F80) {
        let a_sign = self.get_sign();
        let b_sign = b.get_sign();

        let mut a_exp = self.get_exponent();
        let b_exp = b.get_exponent();

        let mut a_man = self.get_mantissa() as u128;
        let mut b_man = b.get_mantissa() as u128;

        // Clamp the shift amount.
        if a_exp > b_exp {
            let shift = (a_exp - b_exp) as u32;
            if shift >= 128 {
                b_man = 0;
            } else {
                b_man >>= shift;
            }
        } else if b_exp > a_exp {
            let shift = (b_exp - a_exp) as u32;
            if shift >= 128 {
                a_man = 0;
            } else {
                a_man >>= shift;
            }
            a_exp = b_exp;
        }

        let mut result_sign;
        let mut result_exp = a_exp;
        let mut result_man: u128;

        if a_sign != b_sign {
            result_man = a_man + b_man;
            if (result_man & (1u128 << 64)) != 0 {
                result_man >>= 1;
                result_exp = result_exp.wrapping_add(1);
            }
            result_sign = a_sign;
        } else {
            if a_man >= b_man {
                result_man = a_man - b_man;
                result_sign = a_sign;
            } else {
                result_man = b_man - a_man;
                result_sign = !a_sign;
            }
            let (norm_man, norm_exp) = self.normalize_sub(result_man as u64, result_exp);
            result_man = norm_man as u128;
            result_exp = norm_exp;

            if result_man == 0 {
                result_sign = false; // +0
                result_exp = 0;
            }
        }

        self.set_sign(result_sign);
        self.set_exponent(result_exp);
        self.set_mantissa(result_man as u64);
    }

    fn normalize_sub(&self, mut mant: u64, mut exp: u16) -> (u64, u16) {
        while mant != 0 && (mant & (1 << 63)) == 0 {
            mant <<= 1;
            exp = exp.wrapping_sub(1);
        }
        (mant, exp)
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    /// Doubles of every kind: zeros, denormals, normals, infinities, NaNs.
    fn samples() -> Vec<f64> {
        let mut bits: Vec<u64> = vec![0, 1, 0xF_FFFF_FFFF_FFFF, 0x10_0000_0000_0000, 0x3FF0_0000_0000_0000];
        bits.extend([0x7FEF_FFFF_FFFF_FFFF, 0x7FF0_0000_0000_0000, 0x7FF0_0000_0000_0001, 0x7FF8_0000_0000_0000]);
        bits.extend([0x7FF4_0000_1234_5678, 0x4009_21FB_5444_2D18, 0x0008_0000_0000_0000]);
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..10_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            bits.push(x);
        }
        bits.iter().flat_map(|&b| [f64::from_bits(b), f64::from_bits(b | 1 << 63)]).collect()
    }

    #[test]
    fn canon_is_a_double_through_a_register() {
        for v in samples() {
            let mut f = F80::new();
            f.set_f64(v);
            assert_eq!(canon_f64(v).to_bits(), f.get_f64().to_bits(), "{:016X}", v.to_bits());
            // And a register given it holds the same 80 bits either way.
            let mut g = F80::new();
            g.set_f64(canon_f64(v));
            assert_eq!(f.st, g.st, "{:016X}", v.to_bits());
            assert_eq!(canon_f64(canon_f64(v)).to_bits(), canon_f64(v).to_bits());
        }
    }

    #[test]
    fn a_single_as_a_double_is_canonical() {
        let mut x = 0x2545_F491u32;
        let mut bits = vec![0u32, 1, 0x7F_FFFF, 0x80_0000, 0x7F80_0000, 0x7F80_0001, 0x7FC0_0000, 0x3F80_0000];
        for _ in 0..10_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            bits.push(x);
        }
        for b in bits.into_iter().flat_map(|b| [b, b | 1 << 31]) {
            let v = f32::from_bits(b) as f64;
            assert_eq!(canon_f64(v).to_bits(), v.to_bits(), "{:08X}", b);
        }
    }

    #[test]
    fn an_integer_as_a_double_has_the_integers_80_bits() {
        let mut values: Vec<i32> = vec![0, 1, -1, 2, -2, 255, 32767, -32768, i32::MAX, i32::MIN, 0x1234_5678];
        let mut x = 0x2545_F491u32;
        for _ in 0..10_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            values.push(x as i32);
        }
        for v in values {
            let mut f = F80::new();
            f.set_f64(v as f64);
            assert_eq!(f.st, F80::encode_from_u128(v.unsigned_abs() as u128, v < 0), "{}", v);
            assert_eq!(canon_f64(v as f64).to_bits(), (v as f64).to_bits());
        }
    }

    #[test]
    fn sums_and_differences_are_what_they_always_were() {
        // 80 bits of every kind: random ones, doubles in a register,
        // zeros, denormals, exponents at both ends, equal and near-equal
        // pairs.
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let mut values: Vec<F80> = Vec::new();
        for v in samples().into_iter().take(400) {
            let mut f = F80::new();
            f.set_f64(v);
            values.push(f);
        }
        for i in 0..600 {
            let (hi, lo) = (next(), next());
            let exp = match i % 6 {
                0 => 0,
                1 => 0x7FFF,
                2 => 0x7FFE,
                3 => 1 + hi % 70,
                4 => 0x3FFF + hi % 70,
                _ => hi >> 3 & 0x7FFF,
            };
            let man = match i % 5 {
                0 => lo | 1 << 63,
                1 => lo >> (hi % 64),
                2 => 0,
                3 => 1 << 63,
                _ => lo,
            };
            values.push(F80 { st: (hi as u128 & 1) << 79 | (exp as u128) << 64 | man as u128 });
        }
        for (i, &a) in values.iter().enumerate() {
            for &b in values.iter().skip(i % 7).step_by(7) {
                for (a, b) in [(a, b), (b, a), (a, a), (a, F80 { st: a.st ^ SIGN_MASK }), (a, F80 { st: a.st ^ 1 })] {
                    // The reference knows no NaNs, infinities or overflow.
                    if a.get_exponent() >= 0x7FFE || b.get_exponent() >= 0x7FFE {
                        continue;
                    }
                    let (mut sum, mut reference) = (a, a);
                    sum.add(b);
                    reference.add_reference(b);
                    assert_eq!(sum.st, reference.st, "{:020X} + {:020X}", a.st, b.st);
                    let (mut difference, mut reference) = (a, a);
                    difference.sub(b);
                    reference.sub_reference(b);
                    assert_eq!(difference.st, reference.st, "{:020X} - {:020X}", a.st, b.st);
                }
            }
        }
    }

    #[test]
    fn registers_keep_a_double_as_its_80_bits() {
        let mut regs = FpuRegs::default();
        for (i, v) in samples().into_iter().enumerate() {
            let i = i & 7;
            regs.set_f64(i, v);
            let mut f = F80::new();
            f.set_f64(v);
            assert_eq!(regs.get(i).st, f.st);
            assert_eq!(regs.get_f64(i).to_bits(), f.get_f64().to_bits());
            let got = regs.get(i);
            regs.set(i, got);
            assert_eq!(regs.get(i).st, f.st);
            assert_eq!(regs.get_f64(i).to_bits(), f.get_f64().to_bits());
        }
    }
}
