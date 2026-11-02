//! What the sound devices of rust-dos share: the rate the mixer renders
//! at, the clock of the PC's timer that cards count time in, and the
//! filters and effects of `dsp`.

pub mod dsp;

/// Output sample rate of the mixer, which the sound devices render at.
pub const RATE: u32 = 44_100;

/// The 8253/8254 timer's input clock, in Hz: the PC's time base.
pub const PIT_HZ: u64 = 1_193_182;

/// `a * b / c`, as with 128-bit numbers (the quotient cut to 64 bits), but
/// in 64 where the product fits: a 128-bit division is a call of a
/// library function on 64-bit hosts, and the clock works these out at
/// every port access, the cards for every frame they render. The clock
/// (`timer.rs`) and the cards share this one, so a frame a card counts
/// from a tick is the frame the mixer counts from it.
#[inline]
pub fn mul_div(a: u64, b: u64, c: u64) -> u64 {
    match a.checked_mul(b) {
        Some(p) => p / c,
        None => (a as u128 * b as u128 / c as u128) as u64,
    }
}

/// `a * b / c` rounded up, as `mul_div`, saturating at `u64::MAX`.
#[inline]
pub fn mul_div_ceil(a: u64, b: u64, c: u64) -> u64 {
    match a.checked_mul(b) {
        Some(p) => p.div_ceil(c),
        None => (a as u128 * b as u128).div_ceil(c as u128).min(u64::MAX as u128) as u64,
    }
}
