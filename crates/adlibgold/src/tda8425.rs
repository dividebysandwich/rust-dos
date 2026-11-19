//! The Philips TDA8425 hi-fi stereo audio processor, the AdLib Gold's
//! final output stage: it picks the source (both channels of the mixer, or
//! one of them on both outputs), gives it a stereo effect, its bass and
//! treble, and the output volume. The control chip's registers 4 to 8
//! hold its settings (the AdLib Gold SDTK's register reference):
//!
//! - 4 and 5: the volume of the left and the right output, bits 0-5: 3Ch
//!   is 0 dB, each step 2 dB, from +6 dB (3Fh) down to -64 dB (1Ch), and
//!   anything below is off.
//! - 6: the bass, bits 0-3: 6 is 0 dB, each step 3 dB, from -12 dB (2 and
//!   below) up to +15 dB (Bh and above).
//! - 7: the treble, the same steps from -12 dB up to +12 dB (Ah and
//!   above).
//! - 8: the output mode: bits 0-2 the source (6 both channels, 2 the left
//!   one on both outputs, 4 the right one), bits 3-4 the effect (0 forced
//!   mono, 1 linear stereo, 2 pseudo stereo, 3 spatial stereo) and bit 5
//!   mutes.
//!
//! The datasheet gives the steps; the filters' corner frequencies and the
//! effects are DOSBox Staging's (adlib_gold.cpp), which tuned them against
//! recordings of the card: shelves at 400 Hz and 2.5 kHz with a slope of
//! 0.5, pseudo stereo as an all-pass
//! at 400 Hz on the left channel, and spatial stereo as 52% of each
//! channel's difference from the other added to it (the antiphase
//! crosstalk the datasheet gives).

use rust_dos_audio_core::dsp::Biquad;

const BASS_HZ: f32 = 400.0;
const TREBLE_HZ: f32 = 2500.0;
const SHELF_SLOPE: f32 = 0.5;
const PSEUDO_HZ: f32 = 400.0;
const PSEUDO_Q: f32 = 1.7;
const SPATIAL_CROSSTALK: f32 = 0.52;

/// The gain of a volume register's bits 0-5.
pub fn volume(value: u8) -> f32 {
    match value & 0x3F {
        v @ 0x1C.. => 10f32.powf((v as f32 - 60.0) * 2.0 / 20.0),
        _ => 0.0,
    }
}

/// The bass's gain in dB, of register 6's bits 0-3.
pub fn bass_db(value: u8) -> f32 {
    ((value & 0xF) as f32 - 6.0).clamp(-4.0, 5.0) * 3.0
}

/// The treble's gain in dB, of register 7's bits 0-3.
pub fn treble_db(value: u8) -> f32 {
    ((value & 0xF) as f32 - 6.0).clamp(-4.0, 4.0) * 3.0
}

pub struct Tda8425 {
    gain: (f32, f32),
    /// Register 8.
    mode: u8,
    bass: [Biquad; 2],
    treble: [Biquad; 2],
    pseudo: Biquad,
}

impl Tda8425 {
    /// The processor with the settings of `regs` (the control chip's
    /// registers, 4 to 8 of them).
    pub fn new(regs: &[u8]) -> Self {
        let mut tda = Self {
            gain: (1.0, 1.0),
            mode: 0xCE,
            bass: [Biquad::low_shelf(BASS_HZ, 0.0, SHELF_SLOPE); 2],
            treble: [Biquad::high_shelf(TREBLE_HZ, 0.0, SHELF_SLOPE); 2],
            pseudo: Biquad::allpass(PSEUDO_HZ, PSEUDO_Q),
        };
        for reg in 4..=8 {
            tda.write(reg, regs[reg as usize]);
        }
        tda
    }

    /// A write to control register `reg` (4 to 8).
    pub fn write(&mut self, reg: u8, value: u8) {
        match reg {
            4 => self.gain.0 = volume(value),
            5 => self.gain.1 = volume(value),
            6 => {
                let shelf = Biquad::low_shelf(BASS_HZ, bass_db(value), SHELF_SLOPE);
                self.bass.iter_mut().for_each(|f| f.retune(shelf));
            }
            7 => {
                let shelf = Biquad::high_shelf(TREBLE_HZ, treble_db(value), SHELF_SLOPE);
                self.treble.iter_mut().for_each(|f| f.retune(shelf));
            }
            8 => self.mode = value,
            _ => {}
        }
    }

    /// Whether the filters hold silence.
    pub fn at_rest(&self) -> bool {
        self.bass.iter().chain(&self.treble).all(Biquad::at_rest) && self.pseudo.at_rest()
    }

    /// One frame of the mixer's output through the processor.
    pub fn process(&mut self, (l, r): (f32, f32)) -> (f32, f32) {
        if self.mode & 0x20 != 0 {
            return (0.0, 0.0);
        }
        // The source. Values the SDTK leaves out (Dune writes 0) play
        // both channels, as DOSBox Staging found.
        let (l, r) = match self.mode & 6 {
            2 => (l, l),
            4 => (r, r),
            _ => (l, r),
        };
        let mut out = [l, r];
        for (n, s) in out.iter_mut().enumerate() {
            *s = self.treble[n].process(self.bass[n].process(*s));
        }
        let [l, r] = out;
        let (l, r) = match self.mode >> 3 & 3 {
            0 => {
                let m = (l + r) / 2.0;
                (m, m)
            }
            2 => (self.pseudo.process(l), r),
            3 => (
                l + (l - r) * SPATIAL_CROSSTALK,
                r + (r - l) * SPATIAL_CROSSTALK,
            ),
            _ => (l, r),
        };
        (l * self.gain.0, r * self.gain.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_steps_follow_the_sdtk() {
        assert_eq!(volume(0xFC), 1.0);
        assert!((volume(0x3F) - 10f32.powf(0.3)).abs() < 1e-5);
        assert!((volume(0x1C) - 10f32.powf(-3.2)).abs() < 1e-6);
        assert_eq!(volume(0x1B), 0.0);
        assert_eq!(volume(0xC0), 0.0);
        assert_eq!(bass_db(0xF6), 0.0);
        assert_eq!(bass_db(0xFF), 15.0);
        assert_eq!(bass_db(0xF0), -12.0);
        assert_eq!(treble_db(0xFF), 12.0);
        assert_eq!(treble_db(0xF9), 9.0);
    }

    fn regs(r8: u8) -> [u8; 9] {
        [0, 0, 0, 0, 0xFC, 0xFC, 0xF6, 0xF6, r8]
    }

    /// Flat settings and linear stereo pass the signal as it is.
    #[test]
    fn flat_linear_stereo_passes_the_signal() {
        let mut tda = Tda8425::new(&regs(0xCE));
        for n in 0..1000 {
            let x = (n as f32 * 0.1).sin() * 1000.0;
            let (l, r) = tda.process((x, -x / 2.0));
            assert!(
                (l - x).abs() < 0.01 && (r + x / 2.0).abs() < 0.01,
                "{n}: {l} {r}"
            );
        }
    }

    #[test]
    fn source_mute_mono_and_spatial() {
        let mut tda = Tda8425::new(&regs(0xCA));
        assert_eq!(tda.process((100.0, 50.0)), (100.0, 100.0));
        tda.write(8, 0xCC);
        assert_eq!(tda.process((100.0, 50.0)), (50.0, 50.0));
        tda.write(8, 0xC6);
        assert_eq!(tda.process((100.0, 50.0)), (75.0, 75.0));
        tda.write(8, 0xDE);
        let (l, r) = tda.process((100.0, 50.0));
        assert!(
            (l - 126.0).abs() < 1e-3 && (r - 24.0).abs() < 1e-3,
            "{l} {r}"
        );
        tda.write(8, 0xEE);
        assert_eq!(tda.process((100.0, 50.0)), (0.0, 0.0));
    }

    /// A bass boost raises a low tone and leaves a high one.
    #[test]
    fn bass_boost_raises_low_tones() {
        let level = |reg6: u8, hz: f32| {
            let mut r = regs(0xCE);
            r[6] = reg6;
            let mut tda = Tda8425::new(&r);
            let mut peak = 0.0f32;
            for n in 0..44100 {
                let x = (n as f32 * hz * std::f32::consts::TAU / 44100.0).sin();
                let (l, _) = tda.process((x, x));
                if n > 22050 {
                    peak = peak.max(l.abs());
                }
            }
            peak
        };
        assert!(level(0xFF, 50.0) > 4.0, "{}", level(0xFF, 50.0));
        assert!((level(0xFF, 10000.0) - 1.0).abs() < 0.1);
        assert!(level(0xF0, 50.0) < 0.35);
    }
}
