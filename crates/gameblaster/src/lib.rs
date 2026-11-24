//! Creative's Game Blaster (CT1302), sold first as the Creative Music
//! System (C/MS): two Philips SAA1099 sound chips (`saa1099`) on a 7.16
//! MHz clock, half the PC's 14.318 MHz crystal, and a detection chip.
//! The Sound Blaster 1.0 has the same two chips at its base, and the
//! SB 1.5 and 2.0 sockets for them, without the detection chip.
//!
//! Its ports, from its base (220h unless the jumpers move it):
//!
//! - base + 0, 1: the first chip's data and address ports;
//! - base + 2, 3: the second chip's;
//! - base + 4: reads 7Fh, the card's ID;
//! - base + 6, 7: a latch that takes a byte written there, which a read
//!   of base + 0Ah or 0Bh gives back. Creative's CMS drivers and the
//!   games that look for the card write a byte and read it back.
//!
//! Every other read gives FFh, as nothing drives the bus. On a Sound
//! Blaster those ports are its DSP's, so only the Game Blaster
//! (`CmsConfig::detection`) has them.
//!
//! The chips' outputs are added and filtered as DOSBox Staging does
//! from recordings of the real card: a first-order low-pass at 6 kHz.
//! The capacitor on the card's output takes out the steady level the
//! chips' one-sided output has.

pub mod saa1099;

use rust_dos_audio_core::dsp::{COUPLING_HZ, OnePoleHighpass};
use saa1099::Saa1099;

/// The chips' clock: the PC's 14.318 MHz crystal halved.
pub const CLOCK: u32 = 14_318_180 / 2;

/// The chips' sample rate: one sample every 256 cycles of their clock.
pub const NATIVE_RATE: f64 = CLOCK as f64 / saa1099::DIVIDER as f64;

/// The cutoff of the card's output low-pass, in hertz.
const LOWPASS_HZ: f32 = 6000.0;

/// Where the card is and what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CmsConfig {
    /// The base port: the chips at base + 0 to 3.
    pub base: u16,
    /// The Game Blaster's detection chip at base + 4 to 0Fh, which a
    /// Sound Blaster's chips lack.
    pub detection: bool,
}

/// A first-order low-pass.
#[derive(Clone, Copy, Debug, Default)]
struct Lowpass {
    y: f32,
}

impl Lowpass {
    fn coefficient() -> f32 {
        1.0 - (-2.0 * std::f32::consts::PI * LOWPASS_HZ / rust_dos_audio_core::RATE as f32).exp()
    }

    #[inline]
    fn process(&mut self, x: f32, a: f32) -> f32 {
        let y = self.y + a * (x - self.y);
        // Its tail ends in 0, not in denormal numbers.
        self.y = if (y - x).abs() < 1e-6 { x } else { y };
        self.y
    }
}

rust_dos_savestate::state_fields!(Lowpass { y });

/// The card.
#[derive(Clone, Debug)]
pub struct GameBlaster {
    config: CmsConfig,
    chips: [Saa1099; 2],
    /// The byte written to base + 6 or 7.
    latch: u8,
    /// How far the mixer's frame is between the chips' last two samples,
    /// which it takes the level between.
    phase: f64,
    /// The chips' last two samples, added, left and right.
    previous: (f32, f32),
    current: (f32, f32),
    coupling: [OnePoleHighpass; 2],
    lowpass: [Lowpass; 2],
    lowpass_a: f32,
}

impl GameBlaster {
    pub fn new(config: CmsConfig) -> Self {
        Self {
            config,
            chips: [Saa1099::new(), Saa1099::new()],
            latch: 0,
            phase: 0.0,
            previous: (0.0, 0.0),
            current: (0.0, 0.0),
            coupling: [OnePoleHighpass::new(COUPLING_HZ); 2],
            lowpass: [Lowpass::default(); 2],
            lowpass_a: Lowpass::coefficient(),
        }
    }

    pub fn config(&self) -> CmsConfig {
        self.config
    }

    pub fn chips(&self) -> &[Saa1099; 2] {
        &self.chips
    }

    /// Whether `port` is one of the card's.
    #[inline]
    pub fn claims(&self, port: u16) -> bool {
        let offset = port.wrapping_sub(self.config.base);
        offset < 4 || self.config.detection && offset < 0x10
    }

    /// A write to one of its ports.
    pub fn write(&mut self, port: u16, value: u8) {
        match port.wrapping_sub(self.config.base) {
            0 => self.chips[0].write_data(value),
            1 => self.chips[0].write_address(value),
            2 => self.chips[1].write_data(value),
            3 => self.chips[1].write_address(value),
            6 | 7 if self.config.detection => self.latch = value,
            _ => {}
        }
    }

    /// A read of one of its ports.
    pub fn read(&self, port: u16) -> u8 {
        match port.wrapping_sub(self.config.base) {
            4 if self.config.detection => 0x7F,
            0x0A | 0x0B if self.config.detection => self.latch,
            _ => 0xFF,
        }
    }

    /// The next stereo frame at the mixer's rate, on a 16-bit scale.
    pub fn render(&mut self) -> (f32, f32) {
        self.phase += NATIVE_RATE / rust_dos_audio_core::RATE as f64;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.previous = self.current;
            let (l0, r0) = self.chips[0].step();
            let (l1, r1) = self.chips[1].step();
            // Each chip's six channels make 16-bit full scale.
            self.current = ((l0 + l1) as f32 / 6.0, (r0 + r1) as f32 / 6.0);
        }
        let t = self.phase as f32;
        let l = self.previous.0 + (self.current.0 - self.previous.0) * t;
        let r = self.previous.1 + (self.current.1 - self.previous.1) * t;
        let a = self.lowpass_a;
        let l = self.lowpass[0].process(self.coupling[0].process(l), a);
        let r = self.lowpass[1].process(self.coupling[1].process(r), a);
        (l, r)
    }

    /// Whether the card adds nothing to the mix and nothing in it moves:
    /// both chips' sound is off, which stops their generators, and the
    /// filters have settled on the silence.
    pub fn is_quiet(&self) -> bool {
        self.chips.iter().all(|c| !c.enabled())
            && self.previous == (0.0, 0.0)
            && self.current == (0.0, 0.0)
            && self.coupling.iter().all(|f| f.rests_on(0.0))
            && self.lowpass.iter().all(|f| f.y == 0.0)
    }
}

// The configuration comes with the machine.
rust_dos_savestate::state_fields!(GameBlaster {
    chips,
    latch,
    phase,
    previous,
    current,
    coupling,
    lowpass
} skip { config, lowpass_a });

#[cfg(test)]
mod tests {
    use super::*;
    use rust_dos_savestate::State;

    fn card(detection: bool) -> GameBlaster {
        GameBlaster::new(CmsConfig {
            base: 0x220,
            detection,
        })
    }

    fn set(card: &mut GameBlaster, chip: u16, reg: u8, value: u8) {
        card.write(0x221 + 2 * chip, reg);
        card.write(0x220 + 2 * chip, value);
    }

    #[test]
    fn detection_latch() {
        let mut gb = card(true);
        assert!(gb.claims(0x220) && gb.claims(0x22F) && !gb.claims(0x230) && !gb.claims(0x21F));
        assert_eq!(gb.read(0x224), 0x7F);
        gb.write(0x226, 0xA5);
        assert_eq!(gb.read(0x22A), 0xA5);
        assert_eq!(gb.read(0x22B), 0xA5);
        gb.write(0x227, 0x5A);
        assert_eq!(gb.read(0x22A), 0x5A);
        assert_eq!(gb.read(0x220), 0xFF);
        assert_eq!(gb.read(0x225), 0xFF);
        // A Sound Blaster's chips leave its DSP's ports alone.
        let sb = card(false);
        assert!(sb.claims(0x223) && !sb.claims(0x224) && !sb.claims(0x226) && !sb.claims(0x22A));
    }

    /// A tone on each chip, the first on the left and the second on the
    /// right, is heard on its side; the output swings around 0 once the
    /// capacitor has taken the steady level out.
    #[test]
    fn both_chips_play() {
        let mut gb = card(true);
        assert!(gb.is_quiet());
        // 440 Hz or so: octave 4, frequency 511 - 7159090 / (2 * 440 * 16).
        for (chip, amplitude) in [(0, 0x0F), (1, 0xF0)] {
            set(&mut gb, chip, 0x00, amplitude);
            set(&mut gb, chip, 0x08, 3);
            set(&mut gb, chip, 0x10, 0x04);
            set(&mut gb, chip, 0x14, 0x01);
            set(&mut gb, chip, 0x1C, 0x01);
        }
        let hz = gb.chips()[0].channels()[0].hz(CLOCK as f64);
        assert!((hz - 440.0).abs() < 2.0, "{}", hz);
        let frames: Vec<(f32, f32)> = (0..44_100).map(|_| gb.render()).collect();
        let late = &frames[22_050..];
        let peak =
            |f: fn(&(f32, f32)) -> f32| late.iter().map(f).fold(0.0f32, |m, s| m.max(s.abs()));
        let (pl, pr) = (peak(|f| f.0), peak(|f| f.1));
        assert!(pl > 1500.0 && pr > 1500.0, "{} {}", pl, pr);
        let mean = late.iter().map(|f| f.0).sum::<f32>() / late.len() as f32;
        assert!(mean.abs() < 100.0, "{}", mean);
        // About 440 rising zero crossings a second.
        let crossings = late
            .windows(2)
            .filter(|w| w[0].0 < 0.0 && w[1].0 >= 0.0)
            .count();
        assert!((215..225).contains(&crossings), "{}", crossings);
        assert!(!gb.is_quiet());
        for chip in 0..2 {
            set(&mut gb, chip, 0x1C, 0x00);
        }
        for _ in 0..44_100 * 4 {
            gb.render();
        }
        assert!(gb.is_quiet());
    }

    #[test]
    fn state_round_trip() {
        let mut gb = card(true);
        set(&mut gb, 1, 0x00, 0x77);
        set(&mut gb, 1, 0x14, 0x01);
        set(&mut gb, 1, 0x15, 0x01);
        set(&mut gb, 1, 0x1C, 0x01);
        gb.write(0x226, 0x42);
        for _ in 0..1000 {
            gb.render();
        }
        let mut w = rust_dos_savestate::Writer::new();
        gb.save(&mut w);
        let mut copy = card(true);
        copy.load(&mut rust_dos_savestate::Reader::new(&w.buf))
            .unwrap();
        assert_eq!(copy.read(0x22A), 0x42);
        let a: Vec<(f32, f32)> = (0..500).map(|_| gb.render()).collect();
        let b: Vec<(f32, f32)> = (0..500).map(|_| copy.render()).collect();
        assert_eq!(a, b);
    }
}
