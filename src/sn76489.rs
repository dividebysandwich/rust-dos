//! The Texas Instruments SN76489 sound chip of the IBM PCjr (an SN76496)
//! and the Tandy 1000 (NCR's copy, the NCR 8496), at port C0h: three
//! square waves and a noise generator, each with a volume in 2 dB steps.
//! As MAME's sn76496.c has it (BSD-3-Clause), in DOSBox Staging's copy,
//! with DOSBox's filters for the sound of the machines' speaker circuits.

use crate::dsp::{BUTTERWORTH_Q, Biquad};

/// The chip's clock: the NTSC colour carrier, the machines' 14.318 MHz
/// crystal divided by 4.
pub const CLOCK: f64 = 14_318_180.0 / 4.0;
/// The chip counts at a sixteenth of it.
const TICK_RATE: f64 = CLOCK / 16.0;
/// The loudest a channel is, a quarter of the 16-bit range, as the four
/// sum.
const MAX_CHANNEL: f32 = 32767.0 / 4.0;

/// Which chip it is: the two differ in their noise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// The PCjr's SN76496: a 17-bit noise register tapped at bits 2 and 3.
    Sn76496,
    /// The Tandy 1000's NCR 8496: a 16-bit register tapped at bits 1 and
    /// 5 with XNOR, reset only when the noise's kind changes, and the
    /// output inverted. It ignores data bytes to the volumes and the noise
    /// control.
    Ncr8496,
}

/// The `tandy` setting: whether the chip is there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TandySound {
    /// On a Tandy or PCjr (`machine`).
    #[default]
    Auto,
    /// On any machine, for programs that play Tandy sound on a VGA.
    On,
    Off,
}

impl TandySound {
    pub const ALL: [TandySound; 3] = [TandySound::Auto, TandySound::On, TandySound::Off];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(TandySound::Auto),
            "on" | "true" | "yes" | "1" => Some(TandySound::On),
            "off" | "false" | "no" | "0" => Some(TandySound::Off),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TandySound::Auto => "auto",
            TandySound::On => "on",
            TandySound::Off => "off",
        }
    }

    /// As the settings window shows it.
    pub fn describe(self) -> &'static str {
        match self {
            TandySound::Auto => "auto (Tandy and PCjr)",
            TandySound::On => "on",
            TandySound::Off => "off",
        }
    }
}

/// The chip.
#[derive(Clone, Debug)]
pub struct Sn76489 {
    variant: Variant,
    /// The eight registers: tone periods (10 bits) and volumes of the
    /// three channels, the noise control and its volume.
    registers: [u16; 8],
    /// The register the last byte with bit 7 set chose.
    latched: usize,
    /// The channels' volumes as output levels.
    volume: [f32; 4],
    /// The periods, in ticks, and the counters toward them.
    period: [i32; 4],
    count: [i32; 4],
    /// Each channel's output bit.
    output: [bool; 4],
    /// The noise's shift register.
    rng: u32,
    /// Ticks owed to the next output sample.
    phase: f64,
    high: Biquad,
    low: Biquad,
}

impl Sn76489 {
    pub fn new(variant: Variant) -> Self {
        let rng = Self::feedback_mask(variant);
        Self {
            variant,
            registers: [0; 8],
            latched: 0,
            volume: [0.0; 4],
            period: [0; 4],
            count: [0; 4],
            output: [false, false, false, rng & 1 != 0],
            rng,
            phase: 0.0,
            // DOSBox Staging's filters for the Tandy's and PCjr's sound.
            high: Biquad::highpass(120.0, BUTTERWORTH_Q),
            low: Biquad::lowpass(4800.0, BUTTERWORTH_Q),
        }
    }

    pub fn variant(&self) -> Variant {
        self.variant
    }

    fn feedback_mask(variant: Variant) -> u32 {
        match variant {
            Variant::Sn76496 => 0x10000,
            Variant::Ncr8496 => 0x8000,
        }
    }

    fn taps(&self) -> (u32, u32) {
        match self.variant {
            Variant::Sn76496 => (0x04, 0x08),
            Variant::Ncr8496 => (0x02, 0x20),
        }
    }

    fn ncr(&self) -> bool {
        self.variant == Variant::Ncr8496
    }

    /// A 2 dB step of attenuation a count, 15 silent.
    fn level(attenuation: u16) -> f32 {
        if attenuation >= 15 { 0.0 } else { MAX_CHANNEL * 10f32.powf(-(attenuation as f32) * 2.0 / 20.0) }
    }

    /// A byte written to the chip: with bit 7, the register (bits 4-6) and
    /// its low 4 bits; without, a tone period's high 6 bits.
    pub fn write(&mut self, data: u8) {
        let r;
        if data & 0x80 != 0 {
            r = ((data >> 4) & 7) as usize;
            self.latched = r;
            if self.ncr() && r == 6 && (data as u16 & 0x04) != (self.registers[6] & 0x04) {
                self.rng = Self::feedback_mask(self.variant);
            }
            self.registers[r] = (self.registers[r] & 0x3F0) | (data & 0x0F) as u16;
        } else {
            r = self.latched;
            if self.ncr() && (r & 1 != 0 || r == 6) {
                return;
            }
        }
        let channel = r >> 1;
        match r {
            0 | 2 | 4 => {
                if data & 0x80 == 0 {
                    self.registers[r] = (self.registers[r] & 0x0F) | ((data & 0x3F) as u16) << 4;
                }
                // A period of 0 counts as 1024.
                let period = self.registers[r];
                self.period[channel] = if period != 0 { period as i32 } else { 0x400 };
                if r == 4 && self.registers[6] & 0x03 == 0x03 {
                    self.period[3] = self.period[2] << 1;
                }
            }
            1 | 3 | 5 | 7 => {
                self.volume[channel] = Self::level((data & 0x0F) as u16);
                if data & 0x80 == 0 {
                    self.registers[r] = (self.registers[r] & 0x3F0) | (data & 0x0F) as u16;
                }
            }
            _ => {
                if data & 0x80 == 0 {
                    self.registers[r] = (self.registers[r] & 0x3F0) | (data & 0x0F) as u16;
                }
                // The noise shifts at the clock / 512, 1024 or 2048, or
                // with the third tone.
                let n = self.registers[6];
                self.period[3] = if n & 3 == 3 { self.period[2] << 1 } else { 1 << (5 + (n & 3)) };
                if !self.ncr() {
                    self.rng = Self::feedback_mask(self.variant);
                }
            }
        }
    }

    /// One tick of the chip's counters.
    fn tick(&mut self) {
        for i in 0..3 {
            self.count[i] -= 1;
            if self.count[i] <= 0 {
                self.output[i] = !self.output[i];
                self.count[i] = self.period[i];
            }
        }
        self.count[3] -= 1;
        if self.count[3] <= 0 {
            // White noise feeds back both taps; periodic noise only the
            // first (the second held at its resting value).
            let (tap1, tap2) = self.taps();
            let resting = if self.ncr() { tap2 } else { 0 };
            let noise = self.registers[6] & 4 != 0;
            if ((self.rng & tap1) != 0) != ((self.rng & tap2 != resting) && noise) {
                self.rng >>= 1;
                self.rng |= Self::feedback_mask(self.variant);
            } else {
                self.rng >>= 1;
            }
            self.output[3] = self.rng & 1 != 0;
            self.count[3] = self.period[3];
        }
    }

    fn level_now(&self) -> f32 {
        let sum: f32 = (0..4).filter(|&i| self.output[i]).map(|i| self.volume[i]).sum();
        if self.ncr() { -sum } else { sum }
    }

    /// The next sample at the mixer's rate, on a 16-bit scale: the chip's
    /// output averaged over the ticks since the last one, through the
    /// filters.
    pub fn render(&mut self) -> f32 {
        self.phase += TICK_RATE / crate::opl::RATE as f64;
        let mut sum = 0.0;
        let mut ticks = 0;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.tick();
            sum += self.level_now();
            ticks += 1;
        }
        let sample = if ticks > 0 { sum / ticks as f32 } else { self.level_now() };
        self.low.process(self.high.process(sample))
    }

    /// Whether any channel can be heard.
    pub fn audible(&self) -> bool {
        self.volume.iter().any(|&v| v > 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_at_reset() {
        let mut chip = Sn76489::new(Variant::Ncr8496);
        assert!(!chip.audible());
        assert!((0..1000).all(|_| chip.render().abs() < 1.0));
    }

    #[test]
    fn attenuation_is_2_db_a_step() {
        assert_eq!(Sn76489::level(15), 0.0);
        let ratio = Sn76489::level(1) / Sn76489::level(0);
        assert!((ratio - 0.794).abs() < 0.001, "{}", ratio);
    }

    #[test]
    fn data_bytes_set_the_high_bits_of_a_period() {
        let mut chip = Sn76489::new(Variant::Sn76496);
        chip.write(0x80 | 0x0E);
        chip.write(0x0F);
        assert_eq!(chip.period[0], 0xFE);
        // The NCR 8496 ignores data bytes to a volume.
        let mut chip = Sn76489::new(Variant::Ncr8496);
        chip.write(0x90 | 0x02);
        chip.write(0x0F);
        assert_eq!(chip.registers[1], 0x02);
    }
}
