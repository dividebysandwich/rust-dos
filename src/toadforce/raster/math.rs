//! Logarithm and exponential by the four exactly rounded operations
//! only, so the pictures are the same on every host: the standard
//! library's `log2` and `exp` come from the host's C library, and
//! differ in the last bit from one to the next.

/// The base 2 logarithm of a positive, finite `x`, to about 1e-5.
/// Negative infinity for 0 and below, and for a number that is none.
pub fn log2(x: f32) -> f32 {
    if x.is_nan() || x <= 0.0 {
        return f32::NEG_INFINITY;
    }
    if x == f32::INFINITY {
        return f32::INFINITY;
    }
    // Bring a subnormal into the normal range.
    let (x, shift) = if x < f32::MIN_POSITIVE {
        (x * 16_777_216.0, -24)
    } else {
        (x, 0)
    };
    let bits = x.to_bits();
    let exponent = (bits >> 23) as i32 - 127 + shift;
    // The mantissa, 1 to 2.
    let m = f32::from_bits(bits & 0x007F_FFFF | 0x3F80_0000) as f64;
    // ln m = 2 atanh(s) with s = (m - 1) / (m + 1), at most 1/3.
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    let series = s * (1.0 + s2 * (1.0 / 3.0 + s2 * (1.0 / 5.0 + s2 * (1.0 / 7.0 + s2 / 9.0))));
    (exponent as f64 + series * (2.0 / std::f64::consts::LN_2)) as f32
}

/// e to the power of minus `x`, for `x` of 0 and above. 1 for a number
/// below 0 or none.
pub fn exp_neg(x: f32) -> f32 {
    if x.is_nan() || x <= 0.0 {
        return 1.0;
    }
    let x = x as f64;
    let halvings = (x / std::f64::consts::LN_2).floor();
    if halvings > 200.0 {
        return 0.0;
    }
    // What is left is below ln 2: the series to its 12th term is exact
    // to more than a float holds.
    let r = x - halvings * std::f64::consts::LN_2;
    let mut term = 1.0;
    let mut sum = 1.0;
    for n in 1..=12 {
        term *= -r / n as f64;
        sum += term;
    }
    let scale = f64::from_bits((1023 - halvings as u64) << 52);
    (sum * scale) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log2_is_exact_at_powers_of_two_and_close_between() {
        for e in -30..30 {
            assert_eq!(log2(2.0f32.powi(e)), e as f32);
        }
        for x in [0.001f32, 0.3, 0.75, 1.5, 2.83, 3.0, 100.0, 12345.678, 1e-40] {
            assert!((log2(x) - x.log2()).abs() < 2e-5, "{x}");
        }
        assert_eq!(log2(0.0), f32::NEG_INFINITY);
        assert_eq!(log2(-1.0), f32::NEG_INFINITY);
        assert_eq!(log2(f32::NAN), f32::NEG_INFINITY);
        assert_eq!(log2(f32::INFINITY), f32::INFINITY);
    }

    #[test]
    fn exp_neg_follows_the_exponential() {
        assert_eq!(exp_neg(0.0), 1.0);
        assert_eq!(exp_neg(-3.0), 1.0);
        assert_eq!(exp_neg(f32::NAN), 1.0);
        assert_eq!(exp_neg(1000.0), 0.0);
        assert_eq!(exp_neg(f32::INFINITY), 0.0);
        for x in [0.001f32, 0.1, 0.5, 0.7, 1.0, 2.5, 10.0, 50.0] {
            let want = (-(x as f64)).exp() as f32;
            assert!((exp_neg(x) - want).abs() <= want * 1e-6, "{x}");
        }
    }
}
