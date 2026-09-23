//! The GF1's volume and pan scales, shared by the card and the General MIDI
//! synthesizer that plays Ultrasound patches.

use std::sync::OnceLock;

/// Fraction bits of a volume position: volume ramps move in 1/512 steps
/// of the 12-bit volume.
pub const RAMP_FRAC: u32 = 9;

/// The highest 12-bit volume.
pub const VOLUME_MAX: u32 = 4095;

/// Gain of each 12-bit volume. The GF1's volume is logarithmic, about
/// 0.0235 dB a step below full scale; 0 is silence.
pub fn volumes() -> &'static [f32; 4096] {
    static TABLE: OnceLock<Box<[f32; 4096]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = Box::new([0.0f32; 4096]);
        let mut gain = 1.0f64;
        for v in (1..4096).rev() {
            table[v] = gain as f32;
            gain /= 1.002_709_201;
        }
        table
    })
}

/// Left and right gains of the 16 pan positions: 0 is left, 7 about the
/// centre, 15 right, at constant power.
pub fn pans() -> &'static [(f32, f32); 16] {
    static TABLE: OnceLock<[(f32, f32); 16]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [(0.0, 0.0); 16];
        for (i, pan) in table.iter_mut().enumerate() {
            let norm = (i as f64 - 7.0) / if i < 7 { 7.0 } else { 8.0 };
            let angle = (norm + 1.0) * std::f64::consts::FRAC_PI_4;
            *pan = (angle.cos() as f32, angle.sin() as f32);
        }
        table
    })
}

/// Volume change per frame of a ramp rate register (bits 0-5 the step,
/// bits 6-7 update every 1, 8, 64 or 512 frames), in 1/512 steps.
pub fn ramp_increment(rate: u8) -> u32 {
    ((rate as u32 & 63) << RAMP_FRAC) >> (3 * (rate >> 6))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_scale() {
        let v = volumes();
        assert_eq!(v[0], 0.0);
        assert_eq!(v[4095], 1.0);
        // 256 steps are about 6 dB.
        assert!((v[4095 - 256] - 0.5).abs() < 0.01);
    }

    #[test]
    fn pan_positions() {
        let p = pans();
        assert!((p[0].0 - 1.0).abs() < 1e-6 && p[0].1.abs() < 1e-6);
        assert!(p[15].0.abs() < 1e-6 && (p[15].1 - 1.0).abs() < 1e-6);
        assert!((p[7].0 - p[7].1).abs() < 1e-6);
    }

    #[test]
    fn ramp_rates() {
        assert_eq!(ramp_increment(0x3F), 63 << RAMP_FRAC);
        assert_eq!(ramp_increment(0x41), 64);
        assert_eq!(ramp_increment(0xC1), 1);
    }
}
