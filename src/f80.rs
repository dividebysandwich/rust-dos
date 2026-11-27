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

// f80 emulation, from libmwemu 0.23's src/fpu/f80.rs by sha0coder
// (https://github.com/sha0coder/mwemu), which crates.io publishes under
// the MIT licence. Its notice (Copyright (c) sha0coder) and the licence
// text are in THIRD-PARTY-NOTICES, section 1.
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

    /// Add `b` as a 387 does under control word `control`: the exact sum
    /// rounded once to the precision field's significand (24, 53 or 64
    /// bits; the reserved setting as 64) as the rounding field says, with
    /// the 15-bit exponent whatever the precision. A sum that cancels to
    /// 0 is +0, or -0 when rounding down. Past the largest number it is
    /// the infinity, or the largest number where the rounding goes toward
    /// zero; below the smallest normal one it is a denormal. An operand
    /// in a format the 387 doesn't take (`is_unsupported`) makes the real
    /// indefinite. True for an invalid operation (IE): that, a signalling
    /// NaN, or infinities of opposite signs.
    pub fn add(&mut self, b: F80, control: u16) -> bool {
        self.add_signed(b, b.get_sign(), control)
    }

    /// Subtract `b`: add it with its sign turned.
    pub fn sub(&mut self, b: F80, control: u16) -> bool {
        self.add_signed(b, !b.get_sign(), control)
    }

    #[inline]
    fn add_signed(&mut self, b: F80, b_sign: bool, control: u16) -> bool {
        if self.is_unsupported() || b.is_unsupported() {
            self.set_real_indefinite();
            return true;
        }
        if self.get_exponent() == 0x7FFF || b.get_exponent() == 0x7FFF {
            return self.add_special(b, b_sign);
        }
        let rc = control >> 10 & 3;
        // Sign, exponent (a denormal's counts as 1, as the least normal
        // one's) and significand at the top of 128 bits.
        let parts = |f: &F80, sign: bool| {
            (
                sign,
                f.get_exponent().max(1) as i32,
                (f.st as u64 as u128) << 64,
            )
        };
        let (mut big, mut small) = (parts(self, self.get_sign()), parts(&b, b_sign));
        if (small.1, small.2) > (big.1, big.2) {
            std::mem::swap(&mut big, &mut small);
        }
        let (sign, mut exp) = (big.0, big.1);
        let low = sticky_shift(small.2, (big.1 - small.1) as u32);
        let mut man = if big.0 == small.0 {
            let (sum, carry) = big.2.overflowing_add(low);
            if carry {
                exp += 1;
                sum >> 1 | sum & 1 | 1 << 127
            } else {
                sum
            }
        } else {
            // Larger first, so never below 0.
            big.2 - low
        };
        if man == 0 {
            // Two zeros of one sign keep it; any other 0 is an exact
            // cancellation.
            let negative = if big.0 == small.0 { sign } else { rc == 1 };
            self.st = (negative as u128) << SIGN_SHIFT;
            return false;
        }
        // To the integer bit. (A difference whose operands were more than
        // one place apart loses one place at most, so the sticky bit stays
        // far below the rounding.)
        let up = man.leading_zeros();
        man <<= up;
        exp -= up as i32;
        if exp < 1 {
            // A denormal: its bits above the least normal's last place.
            man = sticky_shift(man, (1 - exp) as u32);
            exp = 1;
        }
        let bits = match control >> 8 & 3 {
            0 => 24,
            2 => 53,
            _ => 64,
        };
        let mut kept = man >> (128 - bits);
        kept += rounds_away(rc, sign, kept & 1 != 0, man << bits, 1 << 127) as u128;
        if kept >> bits != 0 {
            kept >>= 1;
            exp += 1;
        }
        let mut significand = (kept << (64 - bits)) as u64;
        if exp >= 0x7FFF {
            // Overflow, masked: the infinity, or the largest number where
            // the rounding goes toward zero.
            let largest = match rc {
                0 => false,
                1 => !sign,
                2 => sign,
                _ => true,
            };
            (exp, significand) = if largest {
                (0x7FFE, !0 << (64 - bits))
            } else {
                (0x7FFF, INT_BIT_MASK64)
            };
        } else if significand >> 63 == 0 {
            // A denormal (or 0): exponent 0.
            exp = 0;
        }
        self.st = (sign as u128) << SIGN_SHIFT | (exp as u128) << EXP_SHIFT | significand as u128;
        false
    }

    /// `add_signed` where either is a NaN or an infinity, as the FPU with
    /// its exceptions masked: a NaN goes through quieted (of two, the
    /// quiet one, else the larger significand, else the positive one),
    /// infinities of opposite signs make the real indefinite.
    #[cold]
    fn add_special(&mut self, b: F80, b_sign: bool) -> bool {
        let mut b = b;
        b.set_sign(b_sign);
        if let Some((nan, invalid)) = self.nan_result(b) {
            *self = nan;
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

    /// The NaN an operation on `self` and `b` gives when either is a NaN,
    /// as the FPU with its exceptions masked, and whether it is invalid
    /// (IE): a NaN goes through quieted (of two, the quiet one, else the
    /// larger significand, else the positive one), and a signalling one
    /// is invalid. None when neither is a NaN.
    pub fn nan_result(&self, b: F80) -> Option<(F80, bool)> {
        const QUIET: u128 = 1 << 62;
        let (a_nan, b_nan) = (self.is_nan(), b.is_nan());
        if !a_nan && !b_nan {
            return None;
        }
        let signalling = |x: &F80| x.is_signalling();
        let invalid = signalling(self) || signalling(&b);
        let mut nan = *self;
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
                nan = b;
            }
        } else if b_nan {
            nan = b;
        }
        nan.st |= QUIET | INT_BIT_MASK;
        Some((nan, invalid))
    }

    /// A value the FPU since the 387 doesn't take as an operand: a nonzero
    /// exponent with the integer bit clear (an unnormal, a pseudo-NaN or
    /// a pseudo-infinity). Arithmetic on one is invalid.
    pub fn is_unsupported(&self) -> bool {
        self.get_exponent() != 0 && self.st & INT_BIT_MASK == 0
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

    /// A NaN with the quiet bit (bit 62 of the significand) clear.
    pub fn is_signalling(&self) -> bool {
        self.is_nan() && self.st & 1 << 62 == 0
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

        let st = if exp == 0 && mantissa == 0 {
            (sign as u128) << 79
        } else if exp == 0 {
            // A denormal double is a normal number in 80 bits, exactly:
            // mantissa * 2^-1074, with its top bit moved up to the
            // integer bit (12 places or more).
            let up = mantissa.leading_zeros();
            let exponent = (F80_EXP_BIAS - 1074 + 63 - up as i32) as u128;
            ((sign as u128) << 79) | (exponent << 64) | (mantissa << up) as u128
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
            // A zero, or a denormal far under the least denormal double:
            // 0 to nearest.
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
                // Below a double's normals: a denormal double, the
                // significand in units of 2^-1074 rounded to nearest, even
                // on a tie. Rounding up to 2^52 units makes the least
                // normal double, whose bits are that number.
                let shift = (12 - f64_exp) as u32;
                let m = mantissa as u64 as u128;
                let (kept, rest, half) = if shift >= 66 {
                    (0, 1, 2)
                } else {
                    (m >> shift, m & ((1u128 << shift) - 1), 1u128 << (shift - 1))
                };
                let kept = kept + rounds_away(0, false, kept & 1 != 0, rest, half) as u128;
                (sign << 63) | kept as u64
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

/// `value` shifted right by `by`, with a 1 in its lowest bit where any
/// bit that falls out was 1 (a sticky bit, for rounding).
#[inline]
fn sticky_shift(value: u128, by: u32) -> u128 {
    if by == 0 {
        value
    } else if by >= 128 {
        (value != 0) as u128
    } else {
        value >> by | (value << (128 - by) != 0) as u128
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
/// (`F80::set_f64`, then `get_f64`): a NaN quiet, and every other double
/// as it was, a denormal too (the 80 bits hold it exactly).
#[inline]
pub fn canon_f64(value: f64) -> f64 {
    let bits = value.to_bits();
    let canon = if bits >> 52 & 0x7FF == 0x7FF && bits << 12 != 0 {
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
    /// (`Uop::FBailPrecise`, `Uop::FToSingle` for the single and
    /// `Uop::FToInt64` for a qword).
    #[inline]
    pub fn beyond_double(&self, i: usize) -> bool {
        self.stale[i] == 0 && self.x80[i].st & 0x7FF != 0
    }

    /// Register `i`'s double where its 80 bits are that double and no
    /// more: a register that is stale, or one whose significand is the
    /// double's (its 53 bits, the integer bit set) where the double is a
    /// normal number, and no bits where it is 0. That leaves out a number
    /// below a double's normals, which the double has rounded to a
    /// denormal, to 0 or up to the least normal double, with fewer bits
    /// than 53 or other ones, and the formats a 387 doesn't take (an
    /// unnormal, a pseudo-zero or a pseudo-infinity), whose double
    /// `F80::get_f64` makes as if the integer bit were set.
    #[inline]
    pub fn only_double(&self, i: usize) -> Option<f64> {
        let value = self.f64s[i];
        if self.stale[i] == 0 {
            let (bits, significand) = (value.to_bits(), self.x80[i].st as u64);
            let whole = if bits << 1 >> 53 == 0 {
                value == 0.0 && significand == 0
            } else {
                significand == (bits << 11 | 1 << 63)
            };
            if !whole {
                return None;
            }
        }
        Some(value)
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


/// The sum of two 80-bit numbers as `F80::add` should give it, worked out
/// another way for the tests to compare with: the exact sum as a 256-bit
/// integer (an operand more than 130 places below the other as one unit
/// at the bottom, which rounds the same), rounded a bit at a time.
#[cfg(test)]
impl F80 {
    fn sum_reference(a: F80, b: F80, b_sign: bool, control: u16) -> u128 {
        type U256 = [u64; 4];
        fn bit(x: &U256, i: i32) -> bool {
            (0..256).contains(&i) && x[(i / 64) as usize] >> (i % 64) & 1 != 0
        }
        fn shl(x: U256, by: i32) -> U256 {
            let mut out = [0; 4];
            for i in (0..256).filter(|&i| bit(&x, i - by)) {
                out[(i / 64) as usize] |= 1 << (i % 64);
            }
            out
        }
        fn add(x: U256, y: U256) -> U256 {
            let (mut out, mut carry) = ([0; 4], false);
            for i in 0..4 {
                let (s, c1) = x[i].overflowing_add(y[i]);
                let (s, c2) = s.overflowing_add(carry as u64);
                (out[i], carry) = (s, c1 || c2);
            }
            out
        }
        let rc = control >> 10 & 3;
        let bits = match control >> 8 & 3 {
            0 => 24,
            2 => 53,
            _ => 64,
        };
        let parts = |f: F80, sign: bool| (sign, f.get_exponent().max(1) as i32, f.st as u64);
        let (mut x, mut y) = (parts(a, a.get_sign()), parts(b, b_sign));
        if (y.1, y.2) > (x.1, x.2) {
            std::mem::swap(&mut x, &mut y);
        }
        // In units of 2^(x's exponent - bias - 63 - 130).
        let big = shl([x.2, 0, 0, 0], 130);
        let d = x.1 - y.1;
        let small = match y.2 {
            0 => [0; 4],
            _ if d > 130 => [1, 0, 0, 0],
            m => shl([m, 0, 0, 0], 130 - d),
        };
        let total = if x.0 == y.0 {
            add(big, small)
        } else {
            add(big, add(small.map(|w| !w), [1, 0, 0, 0]))
        };
        let sign = x.0;
        // The biased exponent of the bit at place i.
        let exponent = |i: i32| x.1 + i - 130 - 63;
        let Some(top) = (0..256).rev().find(|&i| bit(&total, i)) else {
            let negative = if x.0 == y.0 { sign } else { rc == 1 };
            return (negative as u128) << 79;
        };
        // The last place kept: `bits` from the top, or from the place of
        // the least normal number's integer bit below it (a denormal).
        let last = top + (1 - exponent(top)).max(0) - (bits - 1);
        let mut kept = (last..=top)
            .rev()
            .fold(0u128, |k, i| k << 1 | bit(&total, i) as u128);
        let half = bit(&total, last - 1);
        let rest = (0..last - 1).any(|i| bit(&total, i));
        let up = match rc {
            0 => half && (rest || kept & 1 != 0),
            1 => sign && (half || rest),
            2 => !sign && (half || rest),
            _ => false,
        };
        kept += up as u128;
        if kept == 0 {
            return (sign as u128) << 79;
        }
        let length = 128 - kept.leading_zeros() as i32;
        let exp = exponent(last + length - 1);
        if exp >= 0x7FFF {
            let largest = match rc {
                0 => false,
                1 => !sign,
                2 => sign,
                _ => true,
            };
            let st = if largest {
                0x7FFE << 64 | (!0u64 << (64 - bits)) as u128
            } else {
                0x7FFF << 64 | 1 << 63
            };
            return (sign as u128) << 79 | st;
        }
        let (exp, significand) = if exp >= 1 {
            (
                exp,
                if length > 64 {
                    kept >> (length - 64)
                } else {
                    kept << (64 - length)
                } as u64,
            )
        } else {
            (0, (kept << (64 - bits)) as u64)
        };
        (sign as u128) << 79 | (exp as u128) << 64 | significand as u128
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
    fn sums_and_differences_are_the_exact_ones_rounded_once() {
        // 80 bits of every kind: random ones, doubles in a register,
        // zeros, denormals, exponents at both ends, equal and near-equal
        // pairs; at every precision and rounding, the reserved precision
        // too.
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
                1 => 0x7FFE - hi % 3,
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
                _ => lo | (exp != 0) as u64 * (1 << 63),
            };
            values.push(F80 { st: (hi as u128 & 1) << 79 | (exp as u128) << 64 | man as u128 });
        }
        // Formats the 387 takes, NaNs and infinities left out.
        values.retain(|f| !f.is_unsupported() && f.get_exponent() != 0x7FFF);
        let mut checked = 0;
        for (i, &a) in values.iter().enumerate() {
            for (j, &b) in values.iter().enumerate().skip(i % 41).step_by(41) {
                let pairs = [
                    (a, b),
                    (b, a),
                    (a, a),
                    (
                        a,
                        F80 {
                            st: a.st ^ SIGN_MASK,
                        },
                    ),
                    (a, F80 { st: a.st ^ 1 }),
                ];
                for (k, (a, b)) in pairs.into_iter().enumerate() {
                    for n in 0..4 {
                        let control = 0x7F | (((i + j + k + n * 4) % 16) as u16) << 8;
                        let (mut sum, mut difference) = (a, a);
                        assert!(!sum.add(b, control));
                        let expected = F80::sum_reference(a, b, b.get_sign(), control);
                        assert_eq!(
                            sum.st, expected,
                            "{:020X} + {:020X} at {control:04X}",
                            a.st, b.st
                        );
                        assert!(!difference.sub(b, control));
                        let expected = F80::sum_reference(a, b, !b.get_sign(), control);
                        assert_eq!(
                            difference.st, expected,
                            "{:020X} - {:020X} at {control:04X}",
                            a.st, b.st
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 100_000, "{checked}");
    }

    #[test]
    fn sums_round_as_the_control_word_says() {
        let f = |v: f64| {
            let mut x = F80::new();
            x.set_f64(v);
            x
        };
        let sum = |a: F80, b: F80, control: u16| {
            let mut s = a;
            s.add(b, control);
            s
        };
        // 1 + 2^-64 + 2^-70: above a half of the 64th bit, and a tie.
        let one = f(1.0);
        let tiny = F80 {
            st: (0x3FFF - 64) << 64 | 1 << 63 | 1 << 57,
        };
        let tie = F80 {
            st: (0x3FFF - 64) << 64 | 1 << 63,
        };
        assert_eq!(sum(one, tiny, 0x037F).st, one.st | 1);
        assert_eq!(sum(one, tie, 0x037F).st, one.st);
        assert_eq!(sum(one, tie, 0x0B7F).st, one.st | 1);
        assert_eq!(
            sum(
                f(-1.0),
                F80 {
                    st: tie.st | SIGN_MASK
                },
                0x077F
            )
            .st,
            f(-1.0).st | 1
        );
        assert_eq!(
            sum(
                f(-1.0),
                F80 {
                    st: tie.st | SIGN_MASK
                },
                0x0F7F
            )
            .st,
            f(-1.0).st
        );
        // 1 - 2^-80 chopped is the 64-bit number below 1.
        let below = F80 {
            st: SIGN_MASK | (0x3FFF - 80) << 64 | 1 << 63,
        };
        assert_eq!(
            sum(one, below, 0x0F7F).st,
            (0x3FFE << 64) | u64::MAX as u128
        );
        assert_eq!(sum(one, below, 0x037F).st, one.st);
        // At 24 bits, 1 + 0.1 keeps 24 bits: rounded to nearest, up;
        // chopped, the single below.
        let single = 1.1f32;
        assert_eq!(sum(one, f(0.1), 0x007F).get_f64(), single as f64);
        assert_eq!(
            sum(one, f(0.1), 0x0C7F).get_f64(),
            f32::from_bits(single.to_bits() - 1) as f64
        );
        // A cancellation is -0 only rounding down.
        assert_eq!(sum(one, f(-1.0), 0x037F).st, 0);
        assert_eq!(sum(one, f(-1.0), 0x077F).st, SIGN_MASK);
        assert_eq!(sum(f(-0.0), f(-0.0), 0x037F).st, SIGN_MASK);
        // Past the largest number: the infinity, or the largest number.
        let max = F80 {
            st: 0x7FFE << 64 | u64::MAX as u128,
        };
        assert_eq!(sum(max, max, 0x037F).st, 0x7FFF << 64 | 1 << 63);
        assert_eq!(sum(max, max, 0x0F7F).st, max.st);
        assert_eq!(
            sum(max, max, 0x0C7F).st,
            0x7FFE << 64 | 0xFFFF_FF00_0000_0000
        );
        // A format the 387 doesn't take makes the real indefinite.
        let mut unnormal = F80 {
            st: 0x3FFF << 64 | 1 << 62,
        };
        assert!(unnormal.add(one, 0x037F));
        assert_eq!(unnormal.st, REAL_INDEFINITE);
    }

    #[test]
    fn values_below_a_doubles_normals_round_to_denormals() {
        // (Exponent, significand) of 80 bits, and the double's bits.
        let cases: [(u16, u64, u64); 7] = [
            // 2^-1074 and 2^-1023, exactly.
            (16383 - 1074, 1 << 63, 1),
            (16383 - 1023, 1 << 63, 1 << 51),
            // 1.5 and 2.5 times 2^-1074: ties, to the even one.
            (16383 - 1074, 3 << 62, 2),
            (16383 - 1073, 5 << 61, 2),
            // Half of 2^-1074 is a tie with 0; a little more is 2^-1074.
            (16383 - 1075, 1 << 63, 0),
            (16383 - 1075, 1 << 63 | 1, 1),
            // Just under 2^-1022 rounds up to the least normal double.
            (16383 - 1023, !0, 1 << 52),
        ];
        for (exp, significand, bits) in cases {
            let f = F80 { st: (exp as u128) << 64 | significand as u128 };
            assert_eq!(f.get_f64().to_bits(), bits, "{:04X} {:016X}", exp, significand);
            let negative = F80 { st: f.st | 1 << 79 };
            assert_eq!(negative.get_f64().to_bits(), bits | 1 << 63);
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

    #[test]
    fn only_a_register_that_is_its_double_is_only_double() {
        let mut regs = FpuRegs::default();
        // (Exponent, significand) of 80 bits, and whether they are the
        // double they round to.
        let cases: [(u16, u64, bool); 7] = [
            (0, 0, true),
            (16383, 1 << 63, true),
            (16383 - 1022, 1 << 63, true),
            // One bit more than a double's 53.
            (16383, 1 << 63 | 1 << 10, false),
            // 2^-1023 is the denormal double 2^-1023, exactly.
            (16383 - 1023, 1 << 63, false),
            // 2^-1022 - 2^-1075 rounds up to the least normal double,
            // 2^-1022, with the low 11 bits clear.
            (16383 - 1023, !0 << 11, false),
            // An unnormal.
            (16383, 1 << 62, false),
        ];
        for (exp, significand, only) in cases {
            regs.set(0, F80 { st: (exp as u128) << 64 | significand as u128 });
            let double = regs.only_double(0);
            assert_eq!(double.is_some(), only, "{:04X} {:016X}", exp, significand);
            if let Some(v) = double {
                let mut f = F80::new();
                f.set_f64(v);
                assert_eq!(f.st, regs.get(0).st);
            }
        }
    }
}
