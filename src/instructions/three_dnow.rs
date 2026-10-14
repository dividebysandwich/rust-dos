//! 3DNow! (K6-2): two single-precision numbers in each MMX register and
//! the arithmetic on them, and the instructions that came with it: the
//! rounded byte averages and word products (PAVGUSB, PMULHRW), the faster
//! EMMS (FEMMS) and the prefetches (PREFETCH, PREFETCHW), as AMD's 3DNow!
//! Technology Manual (21928) has them. They are MMX instructions in every
//! other way (`mmx`).
//!
//! The arithmetic is IEEE 754's single precision with less: it rounds to
//! nearest (ties to even), raises no exceptions and sets no flags. An
//! exponent of 0 is a zero whatever the significand; a result below 2^-126
//! is a zero, and one of 2^128 or more the largest normal, with the sign.
//! The manual leaves undefined what an exponent of FFh gives; here it is no
//! infinity or NaN but a number like the others, its significand times
//! 2^128. A sum or product is worked out exactly and rounded once, and the
//! rounded result is what is tested for being too small or too large.
//!
//! AMD doesn't publish the tables the K6-2's reciprocal and reciprocal
//! square root approximations (PFRCP, PFRSQRT) come from, which are good to
//! 14 and 15 bits; these give the exact result rounded to a single, as
//! `sse::float` does for the Pentium III's. The steps that refine them
//! (PFRCPIT1, PFRSQIT1, PFRCPIT2) give the Newton-Raphson terms the manual
//! describes, rounded once.

use iced_x86::{CpuidFeature, Instruction, Mnemonic};

use super::mmx::{enter_mmx, lanes, mm, read, set_mm, sx};
use crate::cpu::{CR0_EM, CR0_TS, Cpu, CpuResult, FPU_TAG_EMPTY, Fault};

const SIGN: u32 = 0x8000_0000;
const EXPONENT: u32 = 0x7F80_0000;
/// The largest normal.
const MAX: u32 = 0x7F7F_FFFF;

/// A 3DNow! instruction on MMX registers, or FEMMS.
pub fn three_dnow(cpu: &mut Cpu, instr: &Instruction) -> CpuResult {
    if !cpu.model.has_3dnow() || instr.cpuid_features() != [CpuidFeature::D3NOW] {
        return Err(Fault::UD);
    }
    if cpu.cr0 & CR0_EM != 0 {
        return Err(Fault::UD);
    }
    if cpu.cr0 & CR0_TS != 0 {
        return Err(Fault::NM);
    }
    use Mnemonic::*;
    let mnemonic = instr.mnemonic();
    if mnemonic == Femms {
        // As EMMS; the registers are left as they are, which the manual
        // leaves undefined.
        cpu.fpu_tags = [FPU_TAG_EMPTY; 8];
        return Ok(());
    }
    let src = read(cpu, instr, 1)?;
    let dst = mm(cpu, instr.op0_register());
    let result = match mnemonic {
        Pavgusb => lanes::<8>(dst, src, |a, b| (a + b + 1) >> 1),
        Pmulhrw => lanes::<16>(dst, src, |a, b| ((sx::<16>(a) * sx::<16>(b) + 0x8000) >> 16) as u64),
        Pfadd => pairs(dst, src, add),
        Pfsub => pairs(dst, src, sub),
        Pfsubr => pairs(dst, src, |a, b| sub(b, a)),
        // The sum of the destination's two numbers in the low half, of the
        // source's in the high one.
        Pfacc => join(add(dst as u32, (dst >> 32) as u32), add(src as u32, (src >> 32) as u32)),
        Pfmul | Pfrcpit2 => pairs(dst, src, mul),
        Pfmax => pairs(dst, src, |a, b| plus_zero(if value(a) > value(b) { a } else { b })),
        Pfmin => pairs(dst, src, |a, b| plus_zero(if value(a) < value(b) { a } else { b })),
        Pfcmpeq => pairs(dst, src, |a, b| mask(value(a) == value(b))),
        Pfcmpge => pairs(dst, src, |a, b| mask(value(a) >= value(b))),
        Pfcmpgt => pairs(dst, src, |a, b| mask(value(a) > value(b))),
        Pf2id => pairs(dst, src, |_, b| to_int(b)),
        Pi2fd => pairs(dst, src, |_, b| from_int(b as i32)),
        // Of the source's low number, in both halves.
        Pfrcp => {
            let r = reciprocal(src as u32);
            join(r, r)
        }
        Pfrsqrt => {
            let r = reciprocal_sqrt(src as u32);
            join(r, r)
        }
        Pfrcpit1 => pairs(dst, src, reciprocal_step),
        Pfrsqit1 => pairs(dst, src, reciprocal_sqrt_step),
        _ => return Err(Fault::UD),
    };
    enter_mmx(cpu);
    set_mm(cpu, instr.op0_register(), result);
    Ok(())
}

/// PREFETCH and PREFETCHW (K6-2), and the reserved prefetch types 2 to 7
/// (0F 0D /2 to /7), which run as PREFETCH: they load a cache line, so
/// they do nothing here, and never fault, as the manual has it.
pub fn prefetch(cpu: &Cpu) -> CpuResult {
    if cpu.model.has_3dnow() { Ok(()) } else { Err(Fault::UD) }
}

/// `f` of the low numbers of `a` and `b` in the low half, of the high ones
/// in the high half.
fn pairs(a: u64, b: u64, f: impl Fn(u32, u32) -> u32) -> u64 {
    join(f(a as u32, b as u32), f((a >> 32) as u32, (b >> 32) as u32))
}

fn join(lo: u32, hi: u32) -> u64 {
    lo as u64 | (hi as u64) << 32
}

fn mask(c: bool) -> u32 {
    if c { u32::MAX } else { 0 }
}

/// Whether `a` counts as a zero: its exponent is 0.
fn is_zero(a: u32) -> bool {
    a & EXPONENT == 0
}

/// `a`, or +0 for a zero, as PFMAX and PFMIN give one.
fn plus_zero(a: u32) -> u32 {
    if is_zero(a) { 0 } else { a }
}

/// The magnitude of `a`, which isn't a zero: its significand with the
/// leading 1, and the power of two that scales it.
fn magnitude(a: u32) -> (u128, i32) {
    ((a & 0x7F_FFFF | 0x80_0000) as u128, (a >> 23 & 0xFF) as i32 - 150)
}

/// The value of `a`, exactly.
fn value(a: u32) -> f64 {
    if is_zero(a) {
        return 0.0;
    }
    let (sig, exp) = magnitude(a);
    let v = sig as f64 * 2f64.powi(exp);
    if a & SIGN != 0 { -v } else { v }
}

/// `sign` and the magnitude `sig` (not 0) times 2^`exp`, rounded to a
/// single's 24 bits, to nearest or toward zero: a zero if that is below
/// 2^-126, the largest normal if it is 2^128 or more.
fn pack(sign: u32, sig: u128, exp: i32, toward_zero: bool) -> u32 {
    let shift = 128 - sig.leading_zeros() as i32 - 24;
    let (mut q, mut exp) = if shift > 0 {
        let q = sig >> shift;
        let rest = sig & ((1 << shift) - 1);
        let half = 1 << (shift - 1);
        let up = !toward_zero && (rest > half || rest == half && q & 1 != 0);
        (q + up as u128, exp + shift)
    } else {
        (sig << -shift, exp + shift)
    };
    if q >> 24 != 0 {
        q >>= 1;
        exp += 1;
    }
    match exp + 150 {
        255.. => sign | MAX,
        ..=0 => sign,
        biased => sign | (biased as u32) << 23 | (q as u32 & 0x7F_FFFF),
    }
}

/// A sign (0 or `SIGN`) and a magnitude, `sig` (not 0) times 2^`exp`.
type Term = (u32, u128, i32);

/// `x` + `y`, exactly, as a term: the sign is that of the larger magnitude,
/// `x`'s if they are the same, and the magnitude 0 where they cancel.
fn sum(x: Term, y: Term) -> Term {
    // Both to 64 bits, so their order is that of their exponents and then
    // their significands.
    let wide = |(s, sig, exp): Term| {
        let k = sig.leading_zeros() as i32 - 64;
        (s, sig << k, exp - k)
    };
    let (x, y) = (wide(x), wide(y));
    let (big, small) = if (x.2, x.1) >= (y.2, y.1) { (x, y) } else { (y, x) };
    // A smaller term 60 bits or more below the larger can only round the
    // way any amount that small does, so it stays 60 bits below.
    let shift = (big.2 - small.2).min(60);
    let a = big.1 << shift;
    let sig = if big.0 == small.0 { a + small.1 } else { a - small.1 };
    (big.0, sig, big.2 - shift)
}

/// PFADD: `a` + `b`. A zero and a number give the number (whatever its
/// exponent); two zeros a zero negative only if both are.
fn add(a: u32, b: u32) -> u32 {
    match (is_zero(a), is_zero(b)) {
        (true, true) => a & b & SIGN,
        (true, false) => b,
        (false, true) => a,
        (false, false) => {
            let ((sa, ea), (sb, eb)) = (magnitude(a), magnitude(b));
            let (sign, sig, exp) = sum((a & SIGN, sa, ea), (b & SIGN, sb, eb));
            if sig == 0 { sign } else { pack(sign, sig, exp, false) }
        }
    }
}

/// PFSUB: `a` - `b`, which cancels to a zero with `a`'s sign.
fn sub(a: u32, b: u32) -> u32 {
    add(a, b ^ SIGN)
}

/// PFMUL and PFRCPIT2: `a` * `b`, a zero with either a zero.
fn mul(a: u32, b: u32) -> u32 {
    let sign = (a ^ b) & SIGN;
    if is_zero(a) || is_zero(b) {
        return sign;
    }
    let ((sa, ea), (sb, eb)) = (magnitude(a), magnitude(b));
    pack(sign, sa * sb, ea + eb, false)
}

/// PFRCP: 1/`a`, with `a`'s sign; a zero gives the largest normal.
fn reciprocal(a: u32) -> u32 {
    let sign = a & SIGN;
    if is_zero(a) {
        return sign | MAX;
    }
    approximate(1.0 / value(a & !SIGN), sign)
}

/// PFRSQRT: 1/sqrt(|`a`|), with `a`'s sign; a zero gives the largest
/// normal.
fn reciprocal_sqrt(a: u32) -> u32 {
    let sign = a & SIGN;
    if is_zero(a) {
        return sign | MAX;
    }
    approximate(1.0 / value(a & !SIGN).sqrt(), sign)
}

fn approximate(x: f64, sign: u32) -> u32 {
    if x < f32::MIN_POSITIVE as f64 { sign } else { sign | (x as f32).to_bits() }
}

/// PFRCPIT1 of `b` and PFRCP's `x` of it: the Newton-Raphson step's
/// 2 - b*x, positive. With a zero, a zero with the signs' exclusive or.
fn reciprocal_step(b: u32, x: u32) -> u32 {
    newton(b, x, (0, 2, 0), 0)
}

/// PFRSQIT1 of `b` and the square `x` of PFRSQRT's result for it: the
/// Newton-Raphson step's (3 - |b|*x)/2, positive. With a zero, a zero with
/// the signs' exclusive or.
fn reciprocal_sqrt_step(b: u32, x: u32) -> u32 {
    newton(b, x, (0, 3, -1), 1)
}

/// `c` - |`b`*`x`|/2^`halve`, positive, rounded once; a zero with the signs'
/// exclusive or if `b` or `x` is a zero.
fn newton(b: u32, x: u32, c: Term, halve: i32) -> u32 {
    if is_zero(b) || is_zero(x) {
        return (b ^ x) & SIGN;
    }
    let ((sb, eb), (sx, ex)) = (magnitude(b), magnitude(x));
    let (_, sig, exp) = sum(c, (SIGN, sb * sx, eb + ex - halve));
    if sig == 0 { 0 } else { pack(0, sig, exp, false) }
}

/// PF2ID: `a` truncated to a signed dword, 7FFFFFFFh from 2^31 and
/// 80000000h from -2^31.
fn to_int(a: u32) -> u32 {
    let biased = (a >> 23 & 0xFF) as i32;
    if is_zero(a) || biased < 127 {
        return 0;
    }
    if biased >= 158 {
        return if a & SIGN != 0 { 0x8000_0000 } else { 0x7FFF_FFFF };
    }
    let sig = a & 0x7F_FFFF | 0x80_0000;
    let int = if biased <= 150 { sig >> (150 - biased) } else { sig << (biased - 150) };
    if a & SIGN != 0 { int.wrapping_neg() } else { int }
}

/// PI2FD: the signed dword `i`, truncated to a single.
fn from_int(i: i32) -> u32 {
    if i == 0 {
        return 0;
    }
    let sign = if i < 0 { SIGN } else { 0 };
    pack(sign, i.unsigned_abs() as u128, 0, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(x: f32) -> u32 {
        x.to_bits()
    }

    /// Normals whose sums and products stay normal round as the host's
    /// IEEE arithmetic does, to nearest.
    #[test]
    fn normal_results_match_the_host() {
        let values = [1.0f32, 3.0, 0.1, -7.25, 1e30, 1e-30, 16777215.0, -1.5e-19, 3.4e38, 1.0000001, -0.99999994];
        for &x in &values {
            for &y in &values {
                for (got, want, op) in [(add(f(x), f(y)), x + y, "+"), (sub(f(x), f(y)), x - y, "-"), (mul(f(x), f(y)), x * y, "*")] {
                    if want.is_normal() {
                        assert_eq!(got, f(want), "{x} {op} {y}");
                    }
                }
            }
        }
    }

    #[test]
    fn zeros_follow_the_manual() {
        let (p, n) = (0u32, SIGN);
        // Two zeros: negative only if both are (PFADD), source 1 negative
        // and source 2 positive (PFSUB).
        assert_eq!([add(p, p), add(p, n), add(n, p), add(n, n)], [p, p, p, n]);
        assert_eq!([sub(p, p), sub(p, n), sub(n, p), sub(n, n)], [p, p, n, p]);
        // A cancellation has the first source's sign.
        assert_eq!(add(f(1.0), f(-1.0)), p);
        assert_eq!(add(f(-1.0), f(1.0)), n);
        assert_eq!(sub(f(-2.0), f(-2.0)), n);
        // An exponent of 0 is a zero whatever the significand.
        assert_eq!(add(0x0000_0001, f(2.0)), f(2.0));
        assert_eq!(mul(0x8000_0001, f(2.0)), n);
        assert_eq!(mul(f(-3.0), 0), n);
        // PFMAX and PFMIN give +0 for a zero.
        let max = |a, b| plus_zero(if value(a) > value(b) { a } else { b });
        assert_eq!(max(n, f(-1.0)), p);
        assert_eq!(max(n, n), p);
        assert_eq!(max(f(-1.0), f(-2.0)), f(-1.0));
    }

    #[test]
    fn out_of_range_results_saturate_or_vanish() {
        assert_eq!(mul(f(3e38), f(2.0)), MAX);
        assert_eq!(mul(f(-3e38), f(3e38)), SIGN | MAX);
        assert_eq!(add(MAX, MAX), MAX);
        // Below 2^-126 a zero, with the sign of the larger magnitude.
        assert_eq!(mul(f(1e-20), f(-1e-20)), SIGN);
        assert_eq!(sub(f(f32::MIN_POSITIVE * 1.5), f(f32::MIN_POSITIVE)), 0);
        assert_eq!(sub(f(f32::MIN_POSITIVE), f(f32::MIN_POSITIVE * 1.5)), SIGN);
        // An exponent of FFh is a number past the largest normal.
        assert_eq!(add(0x7F80_0000, f(1.0)), MAX);
        assert_eq!(mul(0x7FC0_0000, f(0.25)), f(1.5 * 2f32.powi(126)));
    }

    #[test]
    fn conversions_truncate_and_saturate() {
        assert_eq!(to_int(f(-2.75)), -2i32 as u32);
        assert_eq!(to_int(f(0.99)), 0);
        assert_eq!(to_int(f(3e9)), 0x7FFF_FFFF);
        assert_eq!(to_int(f(-3e9)), 0x8000_0000);
        assert_eq!(to_int(f(-2147483648.0)), 0x8000_0000);
        assert_eq!(to_int(f(16777216.0)), 16777216);
        // 2^24 + 1 and 2^31 - 1 lose their low bits.
        assert_eq!(from_int(16777217), f(16777216.0));
        assert_eq!(from_int(i32::MAX), f(2147483520.0));
        assert_eq!(from_int(-16777219), f(-16777218.0));
        assert_eq!(from_int(i32::MIN), f(-2147483648.0));
    }

    /// The manual's division and square root sequences come out exact for
    /// numbers whose reciprocals and square roots are.
    #[test]
    fn newton_raphson_refines_the_approximations() {
        for b in [3.0f32, 0.1, -7.0, 1e20, 12345.678] {
            let x0 = reciprocal(f(b));
            let x1 = reciprocal_step(f(b), x0);
            let x2 = mul(x1, x0);
            let q = mul(f(1.0), x2);
            assert!((f32::from_bits(q) - 1.0 / b).abs() <= (1.0 / b).abs() * 2e-7, "1/{b}");
        }
        for b in [4.0f32, 2.0, 0.01, 1e-10, -9.0] {
            let x0 = reciprocal_sqrt(f(b));
            let x1 = mul(x0, x0);
            let x2 = reciprocal_sqrt_step(f(b), x1);
            let a = mul(x2, x0);
            let want = 1.0 / b.abs().sqrt() * b.signum();
            assert!((f32::from_bits(a) - want).abs() <= want.abs() * 2e-7, "1/sqrt({b})");
        }
        assert_eq!(reciprocal(0), MAX);
        assert_eq!(reciprocal_sqrt(SIGN), SIGN | MAX);
        assert_eq!(reciprocal(f(4.0)), f(0.25));
        assert_eq!(reciprocal_sqrt(f(-16.0)), f(-0.25));
        assert_eq!(reciprocal_step(0, SIGN), SIGN);
    }
}
