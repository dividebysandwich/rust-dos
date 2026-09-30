//! Conversion tables of the EMU8000: pitch, attenuation, envelope rates
//! and LFO speeds. They are built once, the first time a card is made.

use std::sync::OnceLock;

/// The envelopes' range: 1 << 21 is full scale (0 dB, or the peak of the
/// modulation envelope), 96 dB below it is silence.
pub const ENV_FULL: i32 = 1 << 21;

pub struct Tables {
    /// Initial pitch (0xE000 = the sample's own pitch, 0x1000 an octave)
    /// to the linear pitch of CPF/PTRX (0x4000 = one word a sample).
    pub pitch: Vec<u16>,
    /// IFATN's attenuation, 0.375 dB steps, to a volume target.
    pub atten: [i32; 256],
    /// Attenuation in 1/65536 of 96 dB (after >> 5) to a volume target.
    pub db_to_volume: Vec<i32>,
    /// Amplitude (0 to 65536) to attenuation in 1/65536 of 96 dB.
    pub amplitude_to_db: Vec<i32>,
    /// Attack rate to samples; 0 never attacks.
    pub attack_samples: [i32; 128],
    /// Decay and release rate to envelope units a sample, Q16.
    pub decay_step: [i32; 128],
    /// LFO frequency to the step of its 16.32 phase.
    pub lfo_speed: [u64; 256],
}

pub fn get() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(build)
}

/// The divisor a 7-bit envelope rate stands for (rate >= 1): 1 to 16,
/// then 17 to 32, and from there the step doubles every 16 rates, as the
/// tables of Creative's drivers have it.
fn rate_divisor(rate: usize) -> i32 {
    let idx = rate - 1;
    let group = (idx >> 4) & 7;
    let m = (idx & 15) as i32;
    if group == 0 { m + 1 } else { (m + 17) << (group - 1) }
}

fn build() -> Tables {
    let pitch = (0..0x10000u32)
        .map(|c| if c == 0 { 0 } else { (((c as f64 - 0xE000 as f64) / 4096.0).exp2() * 16384.0) as u64 as u16 })
        .collect();

    let mut atten = [0; 256];
    let mut out = 65535.0f64;
    for a in atten.iter_mut() {
        *a = out as i32;
        out /= 1.09018f64.sqrt();
    }
    atten[255] = 0;

    let mut db_to_volume = vec![0; 0x10001];
    let mut out = 65535.0f64;
    for v in db_to_volume.iter_mut().take(0x10000) {
        *v = out as i32;
        out /= 1.000_169_239_70;
    }
    db_to_volume[0xFFFF] = 0;
    db_to_volume[0x10000] = 0;

    let mut amplitude_to_db = vec![0; 0x10001];
    for (c, v) in amplitude_to_db.iter_mut().enumerate().take(0x10000).skip(1) {
        *v = (-680.321_428_842_64 * 20.0 * (c as f64 / 65535.0).log10()) as i32;
    }
    amplitude_to_db[0] = 65535;
    amplitude_to_db[0x10000] = 0;

    let mut attack_samples = [0; 128];
    let mut decay_step = [0; 128];
    for rate in 1..128 {
        let div = rate_divisor(rate);
        let millis = (11878.0 / div as f64) as f32;
        attack_samples[rate] = (44.1 * millis as f64) as i32;
        let db_per_s = 100.0 / (47.513 / div as f64);
        decay_step[rate] = (db_per_s / 44100.0 * (ENV_FULL as f64 / 96.0) * 65536.0 + 0.5) as i32;
    }

    let mut lfo_speed = [0; 256];
    let mut hz = 0.01f64;
    for s in lfo_speed.iter_mut() {
        *s = (hz * 65536.0 / 44100.0 * 65536.0 * 65536.0) as u64;
        hz += 0.042;
    }

    Tables { pitch, atten, db_to_volume, amplitude_to_db, attack_samples, decay_step, lfo_speed }
}

/// The LFOs' triangle at phase `pos` (65536 a period): from 0 up to
/// 32768, down to -32768 and back, as signed 16 bits.
#[inline]
pub fn lfo(pos: u32) -> i32 {
    let d = (pos as i32 + 16384) & 65535;
    if d >= 32768 { 32768 + (32768 - d) * 2 } else { d * 2 - 32768 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_original_pitch_plays_a_word_a_sample() {
        let t = get();
        assert_eq!(t.pitch[0xE000], 0x4000);
        assert_eq!(t.pitch[0xD000], 0x2000);
        assert_eq!(t.pitch[0], 0);
    }

    #[test]
    fn attenuation_ends_in_silence() {
        let t = get();
        assert_eq!(t.atten[0], 65535);
        assert_eq!(t.atten[255], 0);
        assert_eq!(t.db_to_volume[0], 65535);
        assert_eq!(t.db_to_volume[0xFFFF], 0);
        assert_eq!(t.amplitude_to_db[0x10000], 0);
    }

    #[test]
    fn envelope_rates_follow_the_drivers() {
        let t = get();
        assert_eq!(t.attack_samples[0], 0);
        // Rate 1 attacks in 11.878 s, rate 2 in half that.
        assert!((t.attack_samples[1] - 523_819).abs() < 2);
        assert!((t.attack_samples[2] - 261_909).abs() < 2);
        assert_eq!(rate_divisor(17), 17);
        assert_eq!(rate_divisor(33), 34);
        assert!(t.decay_step[127] > t.decay_step[1]);
    }

    #[test]
    fn the_lfo_is_a_triangle_from_zero() {
        assert_eq!(lfo(0), 0);
        assert_eq!(lfo(16384), 32768);
        assert_eq!(lfo(32768), 0);
        assert_eq!(lfo(49152), -32768);
    }
}
