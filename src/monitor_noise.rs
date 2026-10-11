//! The monitor's sounds for `monitor_effects`, made up as they play: the
//! relay that clicks when the monitor syncs to another frequency, the
//! degaussing coil's thump and dying hum at power on (or Ctrl+Shift+D), the
//! high voltage's crackle as the tube charges, and the thunk of switching
//! off. They mix in as the `monitor` channel.

use crate::opl::RATE;

/// A sound the monitor makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    Relay,
    Degauss,
    Crackle,
    PowerOff,
}

impl Sound {
    /// How long it plays, in frames.
    fn frames(self) -> u32 {
        let ms = match self {
            Sound::Relay => 40,
            Sound::Degauss => 1400,
            Sound::Crackle => 900,
            Sound::PowerOff => 300,
        };
        RATE * ms / 1000
    }
}

/// A sound playing: how far in, and its filters' state.
#[derive(Clone, Copy, Debug)]
struct Voice {
    sound: Sound,
    /// Frames played, and frames before it starts.
    at: u32,
    delay: u32,
    low: f32,
    band: (f32, f32),
}

#[derive(Clone, Debug)]
pub struct MonitorNoise {
    /// Whether it makes sounds at all (`monitor_effects`).
    pub enabled: bool,
    voices: Vec<Voice>,
    rng: u32,
}

impl Default for MonitorNoise {
    fn default() -> Self {
        Self { enabled: false, voices: Vec::new(), rng: 0x1234_5678 }
    }
}

impl MonitorNoise {
    /// Play `sound` after `delay_ms`.
    pub fn play(&mut self, sound: Sound, delay_ms: u32) {
        if self.enabled && self.voices.len() < 8 {
            self.voices.push(Voice { sound, at: 0, delay: RATE * delay_ms / 1000, low: 0.0, band: (0.0, 0.0) });
        }
    }

    pub fn is_silent(&self) -> bool {
        self.voices.is_empty()
    }

    fn noise(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng as i32 as f32 / i32::MAX as f32
    }

    /// The next mixer frame, mono, on the 16-bit scale.
    pub fn render(&mut self) -> f32 {
        if self.voices.is_empty() {
            return 0.0;
        }
        let mut out = 0.0;
        for i in 0..self.voices.len() {
            let n = self.noise();
            let spark = self.noise();
            let v = &mut self.voices[i];
            if v.delay > 0 {
                v.delay -= 1;
                continue;
            }
            let t = v.at as f32 / RATE as f32;
            v.at += 1;
            out += match v.sound {
                // Two clicks 12 ms apart, the contact and its bounce: a
                // burst of noise ringing at 2.5 kHz.
                Sound::Relay => {
                    let click = |t: f32| if t >= 0.0 { (-t / 0.0008).exp() } else { 0.0 };
                    let env = click(t) + 0.6 * click(t - 0.012);
                    let x = n * env;
                    // A resonant band-pass (state variable filter).
                    let f = 2.0 * (std::f32::consts::PI * 2500.0 / RATE as f32).sin();
                    let (low, band) = v.band;
                    let high = x - low - 0.3 * band;
                    let band = band + f * high;
                    v.band = (low + f * band, band);
                    band * 5000.0
                }
                // A thump, then the coil's 60 Hz buzz with its harmonics
                // dying away.
                Sound::Degauss => {
                    let attack = (t / 0.006).min(1.0);
                    let hum_env = attack * (-t / 0.38).exp();
                    let w = 2.0 * std::f32::consts::PI * 60.0 * t;
                    let hum = (w.sin() + 0.5 * (2.0 * w).sin() + 0.3 * (3.0 * w).sin() + 0.15 * (5.0 * w).sin()) * 1.6;
                    let buzz = hum.tanh();
                    v.low += (n - v.low) * 0.03;
                    let thump = v.low * 6.0 * (-t / 0.05).exp();
                    (buzz * 0.55 * hum_env + thump) * 9000.0
                }
                // Sparse sparks as the high voltage builds up.
                Sound::Crackle => {
                    let env = (1.0 - t / 0.9).max(0.0);
                    let hit = if spark.abs() > 0.9965 { spark.signum() } else { 0.0 };
                    v.low = v.low * 0.6 + hit;
                    v.low * env * 7000.0
                }
                // The supply letting go: a low thunk.
                Sound::PowerOff => {
                    v.low += (n - v.low) * 0.02;
                    let env = (t / 0.003).min(1.0) * (-t / 0.07).exp();
                    v.low * env * 40_000.0
                }
            };
        }
        self.voices.retain(|v| v.at < v.sound.frames());
        out
    }

    /// The next `frames` frames, for playing after the machine stopped.
    pub fn take(&mut self, frames: usize) -> Vec<f32> {
        (0..frames).map(|_| self.render()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_play_their_length_and_end() {
        let mut noise = MonitorNoise::default();
        noise.play(Sound::Relay, 0);
        assert!(noise.is_silent(), "nothing while it is off");
        noise.enabled = true;
        for sound in [Sound::Relay, Sound::Degauss, Sound::Crackle, Sound::PowerOff] {
            noise.play(sound, 10);
            let frames = noise.take(sound.frames() as usize + RATE as usize / 100 + 1);
            let loudest = frames.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(loudest > 300.0 && loudest < 32768.0, "{:?}: {}", sound, loudest);
            assert!(noise.is_silent(), "{:?} ended", sound);
        }
    }
}
