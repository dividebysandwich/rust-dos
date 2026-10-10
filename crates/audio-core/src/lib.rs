//! What the sound devices of rust-dos share: the rate the mixer renders
//! at, the clock of the PC's timer that cards count time in, and the
//! filters and effects of `dsp`.

pub mod dsp;

/// Output sample rate of the mixer, which the sound devices render at.
pub const RATE: u32 = 44_100;

/// The 8253/8254 timer's input clock, in Hz: the PC's time base.
pub const PIT_HZ: u64 = 1_193_182;
