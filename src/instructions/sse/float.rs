//! Single-precision arithmetic as the Pentium III's SSE unit does it:
//! IEEE 754 results rounded as MXCSR says, flush to zero, and the six
//! exception flags, from which `Env::finish` tells whether an unmasked one
//! must raise an exception.
//!
//! Each operation works out its result as a double and the sign of what
//! the double leaves out, which pins the exact result down as well as
//! single precision needs: the double's error comes from an error-free
//! transformation (a sum's from TwoSum, a quotient's and a square root's
//! from a fused multiply-add of the residual), and a product of two
//! singles is exact. Rounding that to a single follows the rounding mode,
//! detects tininess after rounding, as an x86 does, and flushes or
//! denormalizes a tiny result.

/// The MXCSR exception flags (bits 0 to 5), and their masks six bits up:
/// invalid operation, denormal operand, divide by zero, overflow,
/// underflow and precision.
pub const IE: u32 = 0x01;
pub const DE: u32 = 0x02;
pub const ZE: u32 = 0x04;
pub const OE: u32 = 0x08;
pub const UE: u32 = 0x10;
pub const PE: u32 = 0x20;
/// The exceptions found before an operation computes anything, which keep
/// it from computing if one is unmasked.
const PRE: u32 = IE | DE | ZE;
/// The masks' shift from the flags.
const MASK_SHIFT: u32 = 7;
/// MXCSR's rounding control (bits 13 and 14) and flush to zero (bit 15).
const RC_SHIFT: u32 = 13;
const FZ: u32 = 0x8000;

/// MXCSR after a reset: every exception masked, round to nearest.
pub const MXCSR_DEFAULT: u32 = 0x1F80;
/// The MXCSR bits a Pentium III has: all of the low 16 but 6, which later
/// processors made denormals-are-zero. LDMXCSR and FXRSTOR raise #GP(0)
/// for any other set, and FXSAVE stores 0 in its mask field, which says
/// this one.
pub const MXCSR_MASK: u32 = 0xFFBF;

/// The default NaN an invalid operation gives: the negative QNaN with the
/// smallest payload, the real indefinite.
pub const INDEFINITE: u32 = 0xFFC0_0000;
/// The integer indefinite a conversion out of range gives.
pub const INT_INDEFINITE: u32 = 0x8000_0000;

const SIGN: u32 = 0x8000_0000;
const QUIET: u32 = 0x0040_0000;
const INF: u32 = 0x7F80_0000;
const MAX: u32 = 0x7F7F_FFFF;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Round {
    Nearest,
    Down,
    Up,
    Zero,
}

pub fn is_nan(a: u32) -> bool {
    a & !SIGN > INF
}

fn is_snan(a: u32) -> bool {
    is_nan(a) && a & QUIET == 0
}

fn is_inf(a: u32) -> bool {
    a & !SIGN == INF
}

fn is_zero(a: u32) -> bool {
    a & !SIGN == 0
}

fn is_denormal(a: u32) -> bool {
    a & INF == 0 && !is_zero(a)
}

fn value(a: u32) -> f64 {
    f32::from_bits(a) as f64
}

/// -1, 0 or 1 as `x` is negative, zero or positive.
fn sign_of(x: f64) -> i64 {
    if x > 0.0 {
        1
    } else if x < 0.0 {
        -1
    } else {
        0
    }
}

/// Whether rounding the double `x` to a single depends on the sign of what
/// `x` leaves out of an exact result. A single keeps 24 of the double's 53
/// bits (fewer for a denormal), and that sign only decides the rounding
/// and the precision flag when the 29 bits below a single's last are all
/// clear or are exactly half of it. Any other pattern rounds and flags the
/// same whatever that sign, so `div` and `sqrt` skip the fused
/// multiply-add, a C runtime call on hosts without FMA, for it.
fn needs_residual(x: f64) -> bool {
    let low = x.to_bits() & ((1 << 29) - 1);
    low == 0 || low == 1 << 28
}

/// An instruction's view of MXCSR, and the exceptions its elements raised.
pub struct Env {
    round: Round,
    ftz: bool,
    /// The exceptions that are masked.
    masked: u32,
    pub flags: u32,
}

impl Env {
    pub fn new(mxcsr: u32) -> Self {
        let round = match (mxcsr >> RC_SHIFT) & 3 {
            0 => Round::Nearest,
            1 => Round::Down,
            2 => Round::Up,
            _ => Round::Zero,
        };
        Env { round, ftz: mxcsr & FZ != 0, masked: (mxcsr >> MASK_SHIFT) & 0x3F, flags: 0 }
    }

    /// Put the exceptions the elements raised into `mxcsr`, and say
    /// whether one that is unmasked must raise #XM (`false`), in which
    /// case the destination is left as it was. If one found before
    /// computing is unmasked, nothing was computed, and only those count.
    pub fn finish(&self, mxcsr: &mut u32) -> bool {
        let unmasked = self.flags & !self.masked;
        if unmasked & PRE != 0 {
            *mxcsr |= self.flags & PRE;
            return false;
        }
        *mxcsr |= self.flags;
        unmasked == 0
    }

    /// The NaN an operation on `a` and `b` gives if either is one: the
    /// first NaN, quieted. A signaling NaN is an invalid operation.
    fn nan2(&mut self, a: u32, b: u32) -> Option<u32> {
        if !is_nan(a) && !is_nan(b) {
            return None;
        }
        if is_snan(a) || is_snan(b) {
            self.flags |= IE;
        }
        Some(if is_nan(a) { a | QUIET } else { b | QUIET })
    }

    fn invalid(&mut self) -> u32 {
        self.flags |= IE;
        INDEFINITE
    }

    fn denormals(&mut self, a: u32, b: u32) {
        if is_denormal(a) || is_denormal(b) {
            self.flags |= DE;
        }
    }

    pub fn add(&mut self, a: u32, b: u32) -> u32 {
        if let Some(nan) = self.nan2(a, b) {
            return nan;
        }
        self.sum(a, b)
    }

    pub fn sub(&mut self, a: u32, b: u32) -> u32 {
        if let Some(nan) = self.nan2(a, b) {
            return nan;
        }
        self.sum(a, b ^ SIGN)
    }

    /// `a` + `b`, neither a NaN.
    fn sum(&mut self, a: u32, b: u32) -> u32 {
        if is_inf(a) && is_inf(b) && (a ^ b) & SIGN != 0 {
            return self.invalid();
        }
        self.denormals(a, b);
        if is_inf(a) {
            return a;
        }
        if is_inf(b) {
            return b;
        }
        let (x, y) = (value(a), value(b));
        let s = x + y;
        if s == 0.0 {
            // An exact zero: of two zeros of the same sign that sign,
            // otherwise +0, or -0 rounding down.
            return if is_zero(a) && is_zero(b) && (a ^ b) & SIGN == 0 {
                a
            } else if self.round == Round::Down {
                SIGN
            } else {
                0
            };
        }
        // TwoSum: what the double s leaves out of x + y, exactly.
        let bb = s - x;
        let err = (x - (s - bb)) + (y - bb);
        self.round(s, sign_of(err))
    }

    pub fn mul(&mut self, a: u32, b: u32) -> u32 {
        if let Some(nan) = self.nan2(a, b) {
            return nan;
        }
        let sign = (a ^ b) & SIGN;
        if (is_inf(a) && is_zero(b)) || (is_zero(a) && is_inf(b)) {
            return self.invalid();
        }
        self.denormals(a, b);
        if is_inf(a) || is_inf(b) {
            return sign | INF;
        }
        if is_zero(a) || is_zero(b) {
            return sign;
        }
        // 24 by 24 bits: exact in a double.
        self.round(value(a) * value(b), 0)
    }

    pub fn div(&mut self, a: u32, b: u32) -> u32 {
        if let Some(nan) = self.nan2(a, b) {
            return nan;
        }
        let sign = (a ^ b) & SIGN;
        if (is_zero(a) && is_zero(b)) || (is_inf(a) && is_inf(b)) {
            return self.invalid();
        }
        self.denormals(a, b);
        if is_inf(a) {
            return sign | INF;
        }
        if is_inf(b) || is_zero(a) {
            return sign;
        }
        if is_zero(b) {
            self.flags |= ZE;
            return sign | INF;
        }
        let (x, y) = (value(a), value(b));
        let q = x / y;
        if !needs_residual(q) {
            return self.round(q, 0);
        }
        // The exact quotient is q + r / y.
        let r = (-q).mul_add(y, x);
        self.round(q, sign_of(r) * sign_of(y))
    }

    pub fn sqrt(&mut self, a: u32) -> u32 {
        if is_nan(a) {
            if is_snan(a) {
                self.flags |= IE;
            }
            return a | QUIET;
        }
        if is_zero(a) {
            return a;
        }
        if a & SIGN != 0 {
            return self.invalid();
        }
        self.denormals(a, 0);
        if is_inf(a) {
            return a;
        }
        let x = value(a);
        let r = x.sqrt();
        if !needs_residual(r) {
            return self.round(r, 0);
        }
        self.round(r, sign_of((-r).mul_add(r, x)))
    }

    /// MINPS and MAXPS: the smaller or larger of `a` and `b`, or `b` as it
    /// is if they are equal (+0 and -0 too) or either is a NaN, which is
    /// an invalid operation, quiet or not.
    pub fn min_max(&mut self, a: u32, b: u32, max: bool) -> u32 {
        if is_nan(a) || is_nan(b) {
            self.flags |= IE;
            return b;
        }
        self.denormals(a, b);
        let (x, y) = (value(a), value(b));
        if (max && x > y) || (!max && x < y) { a } else { b }
    }

    /// CMPPS's predicate `p` (its low three bits) of `a` and `b`. The
    /// ordered less-than forms (LT, LE, NLT, NLE) treat any NaN as an
    /// invalid operation, the others only a signaling one.
    pub fn compare(&mut self, a: u32, b: u32, p: u8) -> bool {
        let unordered = is_nan(a) || is_nan(b);
        if unordered {
            if is_snan(a) || is_snan(b) || matches!(p & 3, 1 | 2) {
                self.flags |= IE;
            }
        } else {
            self.denormals(a, b);
        }
        let (x, y) = (value(a), value(b));
        match p & 7 {
            0 => !unordered && x == y,
            1 => !unordered && x < y,
            2 => !unordered && x <= y,
            3 => unordered,
            4 => unordered || x != y,
            5 => unordered || x >= y,
            6 => unordered || x > y,
            _ => !unordered,
        }
    }

    /// COMISS and UCOMISS: ZF, PF and CF as `a` and `b` compare (all
    /// three for unordered). COMISS treats any NaN as an invalid
    /// operation, UCOMISS only a signaling one.
    pub fn compare_flags(&mut self, a: u32, b: u32, signaling: bool) -> (bool, bool, bool) {
        if is_nan(a) || is_nan(b) {
            if signaling || is_snan(a) || is_snan(b) {
                self.flags |= IE;
            }
            return (true, true, true);
        }
        self.denormals(a, b);
        let (x, y) = (value(a), value(b));
        (x == y, false, x < y)
    }

    /// A signed doubleword as a single (CVTSI2SS, CVTPI2PS).
    pub fn from_int(&mut self, i: i32) -> u32 {
        if i == 0 { 0 } else { self.round(i as f64, 0) }
    }

    /// A single as a signed doubleword (CVTSS2SI, CVTPS2PI), rounded as
    /// MXCSR says or `truncate`d (CVTTSS2SI, CVTTPS2PI). A NaN or a value
    /// out of range is an invalid operation and gives the integer
    /// indefinite.
    pub fn to_int(&mut self, a: u32, truncate: bool) -> u32 {
        if is_nan(a) {
            self.flags |= IE;
            return INT_INDEFINITE;
        }
        let x = value(a);
        let r = match if truncate { Round::Zero } else { self.round } {
            Round::Nearest => x.round_ties_even(),
            Round::Down => x.floor(),
            Round::Up => x.ceil(),
            Round::Zero => x.trunc(),
        };
        if !(-2_147_483_648.0..=2_147_483_647.0).contains(&r) {
            self.flags |= IE;
            return INT_INDEFINITE;
        }
        if r != x {
            self.flags |= PE;
        }
        r as i32 as u32
    }

    /// `x` (nonzero and finite, and normal as a double) rounded to a
    /// single, where the exact value is `x` plus something with the sign
    /// `dir`.
    fn round(&mut self, x: f64, dir: i64) -> u32 {
        let sign = x < 0.0;
        let sign_bit = if sign { SIGN } else { 0 };
        let bits = x.abs().to_bits();
        let exp = (bits >> 52) as i32;
        debug_assert!(exp != 0 && exp != 0x7FF);
        // The exact magnitude, but for a sticky bit: m * 2^e.
        let dir = if sign { -dir } else { dir };
        let m = ((((bits & ((1 << 52) - 1)) | (1 << 52)) << 2) as i64 + dir) as u64;
        let e = exp - 1075 - 2;
        // Its exponent, as of a number in [1, 2).
        let top = 63 - m.leading_zeros() as i32 + e;
        // Rounded to 24 bits whatever its exponent.
        let (k, inexact) = self.shift_round(m, (top - 23 - e) as u32, sign);
        let rounded_top = if k == 1 << 24 { top + 1 } else { top };
        if rounded_top > 127 {
            return self.overflow(sign_bit);
        }
        if rounded_top < -126 {
            return self.underflow(m, e, sign);
        }
        if inexact {
            self.flags |= PE;
        }
        // A carry into bit 24 carries into the exponent.
        sign_bit | ((((top + 127) as u32) << 23) + (k as u32 - (1 << 23)))
    }

    /// `m` shifted right by `s` bits, rounded as MXCSR says for a number
    /// of the sign `sign`, and whether that lost anything.
    fn shift_round(&self, m: u64, s: u32, sign: bool) -> (u64, bool) {
        let s = s.min(100);
        let m = m as u128;
        let kept = (m >> s) as u64;
        let rest = m & ((1u128 << s) - 1);
        let half = 1u128 << (s - 1);
        let up = match self.round {
            Round::Nearest => rest > half || (rest == half && kept & 1 == 1),
            Round::Zero => false,
            Round::Up => !sign && rest != 0,
            Round::Down => sign && rest != 0,
        };
        (kept + up as u64, rest != 0)
    }

    /// A result too large for a single: masked, infinity or the largest
    /// finite single, as the rounding goes.
    fn overflow(&mut self, sign: u32) -> u32 {
        if self.masked & OE == 0 {
            self.flags |= OE;
            return 0;
        }
        self.flags |= OE | PE;
        let to_inf = match self.round {
            Round::Nearest => true,
            Round::Zero => false,
            Round::Up => sign == 0,
            Round::Down => sign != 0,
        };
        sign | if to_inf { INF } else { MAX }
    }

    /// A result smaller than the smallest normal single even after
    /// rounding, `m` * 2^`e`. Masked, it is rounded to a denormal (or with
    /// flush to zero, it is a zero), and underflows if that loses
    /// anything; unmasked, it underflows whether or not.
    fn underflow(&mut self, m: u64, e: i32, sign: bool) -> u32 {
        let sign_bit = if sign { SIGN } else { 0 };
        if self.masked & UE == 0 {
            self.flags |= UE;
            return 0;
        }
        if self.ftz {
            self.flags |= UE | PE;
            return sign_bit;
        }
        // The denormals' quantum is 2^-149.
        let (k, inexact) = self.shift_round(m, (-149 - e) as u32, sign);
        if inexact {
            self.flags |= UE | PE;
        }
        // At most 2^23, the smallest normal, which the bits encode as such.
        sign_bit | k as u32
    }
}

/// RCPSS and RCPPS: an approximation of 1/`a`, which Intel specifies only
/// to within 1.5 * 2^-12 of it; this is the reciprocal rounded to a single.
/// Denormals count as zeros, a result below the normals is a zero, and
/// nothing raises an exception.
pub fn reciprocal(a: u32) -> u32 {
    if is_nan(a) {
        return a | QUIET;
    }
    let sign = a & SIGN;
    if is_zero(a) || is_denormal(a) {
        return sign | INF;
    }
    if is_inf(a) {
        return sign;
    }
    approximate(1.0 / value(a & !SIGN), sign)
}

/// RSQRTSS and RSQRTPS: an approximation of 1/sqrt(`a`), as `reciprocal`;
/// a negative number gives the default NaN.
pub fn reciprocal_sqrt(a: u32) -> u32 {
    if is_nan(a) {
        return a | QUIET;
    }
    let sign = a & SIGN;
    if is_zero(a) || is_denormal(a) {
        return sign | INF;
    }
    if sign != 0 {
        return INDEFINITE;
    }
    if is_inf(a) {
        return 0;
    }
    approximate(1.0 / value(a).sqrt(), 0)
}

fn approximate(x: f64, sign: u32) -> u32 {
    if x < f32::MIN_POSITIVE as f64 { sign } else { sign | (x as f32).to_bits() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(rc: u32) -> Env {
        Env::new(MXCSR_DEFAULT | rc << RC_SHIFT)
    }

    fn f(x: f32) -> u32 {
        x.to_bits()
    }

    #[test]
    fn nearest_matches_the_host() {
        let values = [1.0f32, 3.0, 0.1, -7.25, 1e30, 3e-39, f32::MIN_POSITIVE, 16777215.0, -0.0, 2.5e-45];
        for &x in &values {
            for &y in &values {
                let mut e = env(0);
                assert_eq!(e.add(f(x), f(y)), f(x + y), "{x} + {y}");
                assert_eq!(e.sub(f(x), f(y)), f(x - y), "{x} - {y}");
                assert_eq!(e.mul(f(x), f(y)), f(x * y), "{x} * {y}");
                if y != 0.0 {
                    assert_eq!(e.div(f(x), f(y)), f(x / y), "{x} / {y}");
                }
            }
            if x >= 0.0 {
                assert_eq!(env(0).sqrt(f(x)), f(x.sqrt()), "sqrt {x}");
            }
        }
    }

    #[test]
    fn directed_rounding() {
        // 1 + 2^-30 is between 1 and the next single.
        let tiny = f(2f32.powi(-30));
        let next = f(1.0) + 1;
        assert_eq!(env(1).add(f(1.0), tiny), f(1.0));
        assert_eq!(env(2).add(f(1.0), tiny), next);
        assert_eq!(env(3).add(f(1.0), tiny), f(1.0));
        // 1 - 2^-30: just below 1.
        assert_eq!(env(1).sub(f(1.0), tiny), f(1.0) - 1);
        assert_eq!(env(2).sub(f(1.0), tiny), f(1.0));
        assert_eq!(env(3).sub(f(1.0), tiny), f(1.0) - 1);
        // 1/3's nearest single is above it: toward zero is the one below,
        // toward +inf the nearest, and -1/3 toward -inf the nearest.
        assert_eq!(env(3).div(f(1.0), f(3.0)), f(1.0 / 3.0) - 1);
        assert_eq!(env(2).div(f(1.0), f(3.0)), f(1.0 / 3.0));
        assert_eq!(env(1).div(f(-1.0), f(3.0)), f(-1.0 / 3.0));
        // sqrt(2) is above its nearest single.
        assert_eq!(env(2).sqrt(f(2.0)), f(2f32.sqrt()) + 1);
        assert_eq!(env(1).sqrt(f(2.0)), f(2f32.sqrt()));
        // x - x is -0 rounding down.
        assert_eq!(env(1).sub(f(5.0), f(5.0)), SIGN);
        assert_eq!(env(0).sub(f(5.0), f(5.0)), 0);
    }

    #[test]
    fn flags() {
        let mut e = env(0);
        e.div(f(1.0), f(3.0));
        assert_eq!(e.flags, PE);
        let mut e = env(0);
        e.div(f(1.0), 0);
        assert_eq!(e.flags, ZE);
        let mut e = env(0);
        assert_eq!(e.sub(INF, INF), INDEFINITE);
        assert_eq!(e.flags, IE);
        let mut e = env(0);
        assert_eq!(e.mul(f(1e30), f(1e30)), INF);
        assert_eq!(e.flags, OE | PE);
        let mut e = env(3);
        assert_eq!(e.mul(f(1e30), f(1e30)), MAX);
        // A denormal result that is exact doesn't underflow; one that isn't
        // does. (The operand 2^-140 is a denormal, given by its bits: powi
        // gives 0 for it where it divides 1 by 2^140, which is infinite.)
        let mut e = env(0);
        assert_eq!(e.mul(1 << 9, f(2f32.powi(-5))), 1 << 4);
        assert_eq!(e.flags, DE);
        let mut e = env(0);
        e.mul(f(1.5 * 2f32.powi(-120)), f(2f32.powi(-29)));
        assert_eq!(e.flags, UE | PE);
        // A denormal operand.
        let mut e = env(0);
        e.add(1, f(1.0));
        assert_eq!(e.flags, DE | PE);
        // A quiet NaN passes through quietly; a signaling one is quieted.
        let mut e = env(0);
        assert_eq!(e.add(f(1.0), 0x7FC0_0001), 0x7FC0_0001);
        assert_eq!(e.flags, 0);
        assert_eq!(e.add(0x7F80_0001, 0xFFC0_0002), 0x7FC0_0001);
        assert_eq!(e.flags, IE);
    }

    #[test]
    fn tininess_is_after_rounding() {
        // Just below the smallest normal, rounding up to it: not tiny.
        let x = f32::MIN_POSITIVE as f64 * (1.0 - 2f64.powi(-26));
        let mut e = env(0);
        assert_eq!(e.round(x, 0), f(f32::MIN_POSITIVE));
        assert_eq!(e.flags, PE);
        // Flush to zero.
        let mut e = Env::new(MXCSR_DEFAULT | FZ);
        assert_eq!(e.mul(f(-2f32.powi(-120)), f(2f32.powi(-25))), SIGN);
        assert_eq!(e.flags, UE | PE);
    }

    #[test]
    fn unmasked_exceptions() {
        // Overflow unmasked: no result, OE set.
        let mut e = Env::new(MXCSR_DEFAULT & !(OE << MASK_SHIFT));
        e.mul(f(1e30), f(1e30));
        let mut mxcsr = 0;
        assert!(!e.finish(&mut mxcsr));
        assert_eq!(mxcsr, OE);
        // Invalid unmasked in one element: only the elements' exceptions
        // found before computing count.
        let mut e = Env::new(MXCSR_DEFAULT & !(IE << MASK_SHIFT));
        e.div(f(1.0), f(3.0));
        e.sub(INF, INF);
        let mut mxcsr = 0;
        assert!(!e.finish(&mut mxcsr));
        assert_eq!(mxcsr, IE);
    }

    #[test]
    fn conversions() {
        let mut e = env(0);
        assert_eq!(e.to_int(f(2.5), false), 2);
        assert_eq!(e.to_int(f(3.5), false), 4);
        assert_eq!(e.to_int(f(-2.7), true), -2i32 as u32);
        assert_eq!(e.flags, PE);
        assert_eq!(env(1).to_int(f(-2.5), false), -3i32 as u32);
        assert_eq!(env(2).to_int(f(2.1), false), 3);
        let mut e = env(0);
        assert_eq!(e.to_int(f(3e9), false), INT_INDEFINITE);
        assert_eq!(e.to_int(INDEFINITE, false), INT_INDEFINITE);
        assert_eq!(e.flags, IE);
        assert_eq!(env(0).to_int(f(-2147483648.0), false), 0x8000_0000);
        let mut e = env(0);
        assert_eq!(e.from_int(16_777_217), f(16_777_216.0));
        assert_eq!(e.flags, PE);
        assert_eq!(env(2).from_int(16_777_217), f(16_777_218.0));
        assert_eq!(env(0).from_int(i32::MIN), f(-2147483648.0));
    }

    #[test]
    fn compares_and_min_max() {
        let mut e = env(0);
        assert!(e.compare(f(1.0), f(2.0), 1));
        assert!(e.compare(f(-0.0), f(0.0), 0));
        assert!(e.compare(0x7FC0_0000, f(1.0), 4));
        assert_eq!(e.flags, 0);
        assert!(!e.compare(0x7FC0_0000, f(1.0), 1));
        assert_eq!(e.flags, IE);
        let mut e = env(0);
        assert_eq!(e.min_max(f(-0.0), f(0.0), false), f(0.0));
        assert_eq!(e.min_max(f(0.0), f(-0.0), true), f(-0.0));
        assert_eq!(e.min_max(0x7FC0_0000, f(1.0), true), f(1.0));
        assert_eq!(e.min_max(f(1.0), 0x7F80_0001, true), 0x7F80_0001);
        assert_eq!(e.flags, IE);
        assert_eq!(env(0).compare_flags(f(1.0), f(1.0), true), (true, false, false));
        assert_eq!(env(0).compare_flags(f(1.0), f(2.0), true), (false, false, true));
    }

    /// Skipping the residual where `needs_residual` says it can't matter
    /// gives the results and flags that always computing it gives, in
    /// every rounding mode and with flush to zero.
    #[test]
    fn residual_shortcut() {
        let full_div = |e: &mut Env, a: u32, b: u32| {
            let (x, y) = (value(a), value(b));
            let q = x / y;
            e.round(q, sign_of((-q).mul_add(y, x)) * sign_of(y))
        };
        let full_sqrt = |e: &mut Env, a: u32| {
            let r = value(a).sqrt();
            e.round(r, sign_of((-r).mul_add(r, value(a))))
        };
        let mut seed = 0x1234_5678_9ABC_DEF0u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 32) as u32
        };
        for i in 0..200_000 {
            // Every fourth pair is a product and one of its factors, so
            // the quotient is exact or nearly so.
            let b = next() & 0x7FFF_FFFF | (next() & SIGN);
            let a = if i % 4 == 0 {
                f(f32::from_bits(b) * f32::from_bits(next() & 0x7FFF_FFFF))
            } else {
                next()
            };
            let nonzero_finite = |x: u32| !is_nan(x) && !is_inf(x) && !is_zero(x);
            let (a_ok, b_ok) = (nonzero_finite(a), nonzero_finite(b));
            for mxcsr in [0x1F80u32, 0x3F80, 0x5F80, 0x7F80, 0x9F80, 0xDF80] {
                if a_ok && b_ok {
                    let (mut fast, mut full) = (Env::new(mxcsr), Env::new(mxcsr));
                    full.denormals(a, b);
                    let got = fast.div(a, b);
                    let want = full_div(&mut full, a, b);
                    assert_eq!((got, fast.flags), (want, full.flags), "{a:08x} / {b:08x}, {mxcsr:04x}");
                }
                if a_ok && a & SIGN == 0 {
                    let (mut fast, mut full) = (Env::new(mxcsr), Env::new(mxcsr));
                    full.denormals(a, 0);
                    let got = fast.sqrt(a);
                    let want = full_sqrt(&mut full, a);
                    assert_eq!((got, fast.flags), (want, full.flags), "sqrt {a:08x}, {mxcsr:04x}");
                }
            }
        }
    }

    #[test]
    fn approximations() {
        assert_eq!(reciprocal(f(4.0)), f(0.25));
        assert_eq!(reciprocal(1), INF);
        assert_eq!(reciprocal(f(-0.0)), SIGN | INF);
        assert_eq!(reciprocal(f(1e38)), 0);
        assert_eq!(reciprocal_sqrt(f(4.0)), f(0.5));
        assert_eq!(reciprocal_sqrt(f(-4.0)), INDEFINITE);
        assert_eq!(reciprocal_sqrt(INF), 0);
    }
}
