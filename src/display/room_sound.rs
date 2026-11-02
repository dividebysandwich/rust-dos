//! The 3D scene's own sounds, made as they play: the rain on the windows,
//! thunder, a lamp's switch and a camera's shutter. They are mixed in at
//! the sound device, after the machine's sound (`SdlAudio::queue`), so
//! they are in no recording and no saved state, and come as soon as the
//! machine's sound does.
// (Without the 3D scene, nothing makes them.)
#![cfg_attr(not(feature = "vr"), allow(dead_code))]

/// The device's rate.
const RATE: f32 = 44100.0;

/// A sound that plays once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// A lamp's switch.
    Click,
    /// A polaroid camera: its shutter, then the motor pushing the picture
    /// out.
    Shutter,
    /// Thunder from a strike this many kilometres away.
    Thunder(f32),
}

/// What the room sounds like now, from the main loop each frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ambience {
    /// How loud the rain is (0 to 1, the setting's volume in it).
    pub rain: f32,
    /// Where the windows are: -1 left of the listener, 1 right.
    pub pan: f32,
    /// The listener is in a room: the rain comes through the glass.
    pub indoor: bool,
    /// How loud the thunder is (the setting's volume).
    pub thunder: f32,
}

/// A small random number source (xorshift).
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0
    }
}

/// A one-pole low-pass filter.
#[derive(Default, Clone, Copy)]
struct LowPass(f32);

impl LowPass {
    fn run(&mut self, x: f32, cutoff: f32) -> f32 {
        let a = 1.0 - (-std::f32::consts::TAU * cutoff / RATE).exp();
        self.0 += (x - self.0) * a;
        self.0
    }
}

/// A raindrop hitting the glass: a short, quickly dying ring.
#[derive(Clone, Copy)]
struct Drop {
    phase: f32,
    step: f32,
    level: f32,
    decay: f32,
    pan: f32,
}

/// Thunder: a crack, then a rumble swelling a few times.
struct Rumble {
    t: f32,
    length: f32,
    level: f32,
    /// Brown noise and how bright the crack is.
    brown: [f32; 2],
    crack: f32,
    swells: [(f32, f32); 4],
    filter: [LowPass; 2],
}

/// One of the sounds that play once, as it plays.
enum Playing {
    Click { t: f32 },
    Shutter { t: f32, phase: f32, filter: LowPass },
    Thunder(Rumble),
}

pub struct RoomSound {
    ambience: Ambience,
    /// The rain's level a moment ago, easing to the ambience's.
    rain: f32,
    noise: Noise,
    hiss: [LowPass; 2],
    body: [LowPass; 2],
    glass: [LowPass; 2],
    drops: Vec<Drop>,
    playing: Vec<Playing>,
}

impl Default for RoomSound {
    fn default() -> Self {
        RoomSound {
            ambience: Ambience::default(),
            rain: 0.0,
            noise: Noise(0x1234_5678),
            hiss: Default::default(),
            body: Default::default(),
            glass: Default::default(),
            drops: Vec::new(),
            playing: Vec::new(),
        }
    }
}

impl RoomSound {
    pub fn set(&mut self, ambience: Ambience) {
        self.ambience = ambience;
    }

    pub fn play(&mut self, effect: Effect) {
        let playing = match effect {
            Effect::Click => Playing::Click { t: 0.0 },
            Effect::Shutter => Playing::Shutter { t: 0.0, phase: 0.0, filter: LowPass::default() },
            Effect::Thunder(km) => {
                let near = (1.0 - km / 6.0).clamp(0.1, 1.0);
                let mut swell = || (self.noise.next().abs() * 3.0 + 0.3, 0.4 + self.noise.next().abs() * 0.6);
                let swells = [swell(), swell(), swell(), swell()];
                Playing::Thunder(Rumble {
                    t: 0.0,
                    length: 3.5 + km * 0.8,
                    level: self.ambience.thunder * (0.25 + 0.75 * near),
                    brown: [0.0; 2],
                    crack: near * near,
                    swells,
                    filter: Default::default(),
                })
            }
        };
        self.playing.push(playing);
    }

    /// Forget what is playing. Sounds only end in `mix_into`, so without a
    /// device to pull them they would pile up for as long as the run lasts.
    pub fn clear(&mut self) {
        self.playing.clear();
        self.drops.clear();
    }

    /// Whether there is anything to hear.
    pub fn active(&self) -> bool {
        self.ambience.rain > 0.0 || self.rain > 1e-4 || !self.playing.is_empty() || !self.drops.is_empty()
    }

    /// Add the room's sound to `samples`, stereo frames.
    pub fn mix_into(&mut self, samples: &mut [i16]) {
        let dt = 1.0 / RATE;
        // The rain through glass is duller and quieter than out in it.
        let (cutoff, glass) = if self.ambience.indoor { (2200.0, 0.55) } else { (7000.0, 1.0) };
        let pan = self.ambience.pan.clamp(-1.0, 1.0);
        // More of the rain from the windows' side, but some from both.
        let side = [0.6 - 0.4 * pan, 0.6 + 0.4 * pan];
        for frame in samples.as_chunks_mut::<2>().0 {
            self.rain += (self.ambience.rain - self.rain) * 0.0002;
            let mut out = [0.0f32; 2];
            if self.rain > 1e-4 {
                // The hiss: noise without its lowest and highest, each
                // side its own.
                for (c, o) in out.iter_mut().enumerate() {
                    let n = self.noise.next();
                    let high = n - self.body[c].run(n, 400.0);
                    let hiss = self.hiss[c].run(high, cutoff);
                    *o += hiss * self.rain * 0.3 * glass * side[c];
                }
                // The drops: more the harder it rains.
                if self.noise.next().abs() < self.rain * 180.0 * dt && self.drops.len() < 64 {
                    let pitch = 1800.0 + self.noise.next().abs() * 3500.0;
                    self.drops.push(Drop {
                        phase: 0.0,
                        step: pitch / RATE,
                        level: self.noise.next().abs() * 0.035 * self.rain * glass,
                        decay: (-dt / (0.004 + self.noise.next().abs() * 0.01)).exp(),
                        pan: self.noise.next() * 0.6 + pan * 0.4,
                    });
                }
            }
            for drop in &mut self.drops {
                let s = (drop.phase * std::f32::consts::TAU).sin() * drop.level;
                drop.phase = (drop.phase + drop.step).fract();
                drop.level *= drop.decay;
                out[0] += s * (1.0 - drop.pan) * 0.5;
                out[1] += s * (1.0 + drop.pan) * 0.5;
            }
            self.drops.retain(|d| d.level > 1e-4);
            for (o, glass) in out.iter_mut().zip(&mut self.glass) {
                *o = glass.run(*o, cutoff * 1.5);
            }
            for playing in &mut self.playing {
                let s = match playing {
                    Playing::Click { t } => {
                        *t += dt;
                        // Two clicks: the switch going over, and settling.
                        let click = |at: f32, n: f32| if *t > at && *t < at + 0.004 { n * (1.0 - (*t - at) / 0.004) } else { 0.0 };
                        let n = self.noise.next();
                        [click(0.0, n) * 0.35 + click(0.028, n) * 0.2; 2]
                    }
                    Playing::Shutter { t, phase, filter } => {
                        *t += dt;
                        let n = self.noise.next();
                        // The shutter's snap, then the motor's whirr
                        // (and the picture coming out), dying down.
                        let snap = if *t < 0.012 { n * 0.5 * (1.0 - *t / 0.012) } else { 0.0 };
                        let motor = if (0.08..1.3).contains(t) {
                            let pitch = 110.0 + 25.0 * (*t * 9.0).sin();
                            *phase = (*phase + pitch / RATE).fract();
                            let buzz = (*phase * 2.0 - 1.0) * 0.5 + filter.run(n, 1800.0) * 0.6;
                            let envelope = ((*t - 0.08) / 0.05).min(1.0) * ((1.3 - *t) / 0.2).min(1.0);
                            buzz * envelope * 0.12
                        } else {
                            0.0
                        };
                        [snap + motor; 2]
                    }
                    Playing::Thunder(r) => {
                        r.t += dt;
                        let t = r.t;
                        let mut s = [0.0; 2];
                        for (c, v) in s.iter_mut().enumerate() {
                            let n = self.noise.next();
                            r.brown[c] = (r.brown[c] + n * 0.02) * 0.998;
                            // The crack: bright, short.
                            let crack = if t < 0.25 { n * r.crack * (1.0 - t / 0.25).powi(2) * 0.25 } else { 0.0 };
                            // The rumble, swelling a few times.
                            let mut env = (t / 0.15).min(1.0) * (1.0 - t / r.length).max(0.0).powf(1.5);
                            for &(at, much) in &r.swells {
                                env += much * (-((t - at) * 1.6).powi(2)).exp() * (1.0 - t / r.length).max(0.0);
                            }
                            let rumble = r.filter[c].run(r.brown[c] * 0.5, 260.0 + r.crack * 400.0);
                            *v = (rumble * env + crack) * r.level;
                        }
                        s
                    }
                };
                out[0] += s[0];
                out[1] += s[1];
            }
            self.playing.retain(|p| match p {
                Playing::Click { t } => *t < 0.05,
                Playing::Shutter { t, .. } => *t < 1.3,
                Playing::Thunder(r) => r.t < r.length,
            });
            for (sample, o) in frame.iter_mut().zip(out) {
                *sample = (*sample as f32 + o * 32767.0).clamp(-32768.0, 32767.0) as i16;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loudness(samples: &[i16]) -> f32 {
        (samples.iter().map(|&s| (s as f32).powi(2)).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn rain_sounds_from_the_windows_side_and_stops() {
        let mut room = RoomSound::default();
        let mut silent = vec![0i16; 4410 * 2];
        room.mix_into(&mut silent);
        assert!(silent.iter().all(|&s| s == 0) && !room.active());
        room.set(Ambience { rain: 1.0, pan: 1.0, indoor: true, thunder: 1.0 });
        let mut samples = vec![0i16; 44100 * 2];
        room.mix_into(&mut samples);
        let left: Vec<i16> = samples[44100..].iter().step_by(2).copied().collect();
        let right: Vec<i16> = samples[44101..].iter().step_by(2).copied().collect();
        assert!(loudness(&right) > 100.0, "{}", loudness(&right));
        assert!(loudness(&right) > loudness(&left) * 1.5, "{} {}", loudness(&right), loudness(&left));
        room.set(Ambience::default());
        let mut fading = vec![0i16; 44100 * 2 * 4];
        room.mix_into(&mut fading);
        assert!(loudness(&fading[fading.len() - 2000..]) < 1.0);
    }

    #[test]
    fn effects_play_once_and_end() {
        let mut room = RoomSound::default();
        room.set(Ambience { thunder: 1.0, ..Ambience::default() });
        for effect in [Effect::Click, Effect::Shutter, Effect::Thunder(1.0)] {
            room.play(effect);
            let mut samples = vec![0i16; 44100 * 2 * 8];
            room.mix_into(&mut samples);
            assert!(loudness(&samples[..20000]) > 10.0, "{:?}", effect);
            assert!(!room.active(), "{:?} is over", effect);
        }
    }

    #[test]
    fn clearing_leaves_nothing_waiting_for_a_device() {
        let mut room = RoomSound::default();
        room.play(Effect::Thunder(1.0));
        assert!(room.active());
        room.clear();
        assert!(!room.active());
    }
}
