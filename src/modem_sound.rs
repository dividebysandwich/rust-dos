//! What a modem's speaker plays during a call: the dial tone, the number
//! being dialed (touch tones, or the clicks of pulse dialing), the other
//! end ringing, and the handshake once it answers. The dial tone and the
//! handshake are a real modem's (`assets/modem`), 16-bit mono at 11025 Hz,
//! played at four times that for the mixer's 44.1 kHz; the touch tones,
//! clicks and ringing are made here from the number.

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::OnceLock;

use crate::opl::RATE;

/// The samples' rate is a quarter of the mixer's.
const SAMPLE_RATE: u32 = 11_025;
const UPSAMPLE: u64 = (RATE / SAMPLE_RATE) as u64;
/// The level the synthesized tones play at, matching the recording's.
const TONE: f32 = 0.16 * 32768.0;
/// How long the dial tone plays before the modem dials, and for `W`.
const DIAL_TONE_MS: u64 = 1000;
/// After the number: a pause, then the first ring.
const AFTER_DIAL_MS: u64 = 1000;
/// The ringback cadence (North American: 440 + 480 Hz, two seconds on,
/// four off).
const RING_ON_MS: u64 = 2000;
const RING_OFF_MS: u64 = 4000;
/// Pulse dialing at ten pulses a second, 60 ms open and 40 closed, with
/// 700 ms between digits.
const PULSE_BREAK_MS: u64 = 60;
const PULSE_MAKE_MS: u64 = 40;
const PULSE_GAP_MS: u64 = 700;

/// The samples of an embedded WAVE file.
fn pcm(wav: &'static [u8]) -> &'static [u8] {
    let (format, offset, size) = crate::cdrom::image::wave_data(&mut Cursor::new(wav)).expect("a modem sound sample");
    assert!(format.channels == 1 && format.bits == 16 && format.rate == SAMPLE_RATE, "modem samples are 16-bit mono");
    let end = (offset as usize + (size as usize & !1)).min(wav.len() & !1);
    &wav[offset as usize..end]
}

fn dial_tone() -> &'static [u8] {
    static PCM: OnceLock<&'static [u8]> = OnceLock::new();
    PCM.get_or_init(|| pcm(include_bytes!("../assets/modem/dialtone.wav")))
}

fn handshake() -> &'static [u8] {
    static PCM: OnceLock<&'static [u8]> = OnceLock::new();
    PCM.get_or_init(|| pcm(include_bytes!("../assets/modem/handshake.wav")))
}

fn ms(ms: u64) -> u64 {
    ms * RATE as u64 / 1000
}

/// How long the handshake takes, in mixer frames.
pub fn handshake_frames() -> u64 {
    (handshake().len() / 2) as u64 * UPSAMPLE
}

/// A touch tone's two frequencies.
fn dtmf(key: char) -> Option<(f32, f32)> {
    const ROWS: [f32; 4] = [697.0, 770.0, 852.0, 941.0];
    const COLUMNS: [f32; 4] = [1209.0, 1336.0, 1477.0, 1633.0];
    const KEYS: [&str; 4] = ["123A", "456B", "789C", "*0#D"];
    let key = key.to_ascii_uppercase();
    KEYS.iter().enumerate().find_map(|(row, keys)| keys.find(key).map(|column| (ROWS[row], COLUMNS[column])))
}

/// The digits a call to a host on the internet dials: seven, the same
/// for the same host.
fn digits_for_host(host: &str) -> String {
    let mut hash: u32 = 0x811C_9DC5;
    for b in host.to_ascii_lowercase().bytes() {
        hash = (hash ^ b as u32).wrapping_mul(0x0100_0193);
    }
    // A local number doesn't start with 0 or 1.
    let mut digits = String::new();
    for i in 0..7 {
        let d = hash % 10;
        hash = (hash / 10) ^ hash.rotate_left(13);
        digits.push(char::from(b'0' + if i == 0 { 2 + d % 8 } else { d } as u8));
    }
    digits
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Sound {
    Silence,
    /// The recorded dial tone, looped.
    DialTone,
    /// Two sine tones.
    Tones(f32, f32),
    /// A pulse dial's click: the line opening or closing.
    Click,
    /// The far end ringing, over and over until it answers.
    Ringback,
    Handshake,
}

#[derive(Clone, Copy, Debug)]
struct Part {
    sound: Sound,
    /// How long it plays, in mixer frames; None until the other end
    /// answers.
    frames: Option<u64>,
    audible: bool,
}

/// The speaker: what is left to play of a call.
#[derive(Clone, Debug, Default)]
pub struct ModemSound {
    parts: VecDeque<Part>,
    /// Frames played of the first part.
    at: u64,
    gain: f32,
}

impl ModemSound {
    /// The gain of `ATL` 0 to 3.
    fn gain(volume: u8) -> f32 {
        match volume {
            0 | 1 => 0.3,
            2 => 0.6,
            _ => 1.0,
        }
    }

    fn push(&mut self, sound: Sound, frames: Option<u64>, audible: bool) {
        if frames != Some(0) {
            self.parts.push_back(Part { sound, frames, audible });
        }
    }

    /// Dial `number` (or, for a host on the internet, a number made from
    /// its name), after the dial tone, and let the other end ring until
    /// it answers. `s` are the modem's S-registers (S8 is the pause of a
    /// comma, S11 a touch tone's length), `speaker` `ATM` (0 off, 1 until
    /// the carrier, 2 always, 3 not while dialing). Returns how long the
    /// dialing and the first ring take, in mixer frames: the handshake
    /// starts no sooner.
    pub fn dial(&mut self, number: &str, host: bool, mut pulse: bool, s: &[u8], volume: u8, speaker: u8) -> u64 {
        self.stop();
        self.gain = Self::gain(volume);
        let dialing = speaker == 1 || speaker == 2;
        self.push(Sound::DialTone, Some(ms(DIAL_TONE_MS)), dialing);
        let tone = ms(s.get(11).copied().unwrap_or(95).clamp(50, 255) as u64);
        let digits = if host { digits_for_host(number) } else { number.to_string() };
        for c in digits.chars() {
            match c.to_ascii_uppercase() {
                'T' => pulse = false,
                'P' => pulse = true,
                ',' => self.push(Sound::Silence, Some(ms(s.get(8).copied().unwrap_or(2) as u64 * 1000)), false),
                'W' => self.push(Sound::DialTone, Some(ms(DIAL_TONE_MS)), dialing),
                c if pulse && c.is_ascii_digit() => {
                    let pulses = match c.to_digit(10) {
                        Some(0) => 10,
                        Some(n) => n as u64,
                        None => 0,
                    };
                    for _ in 0..pulses {
                        self.push(Sound::Click, Some(ms(PULSE_BREAK_MS)), dialing);
                        self.push(Sound::Click, Some(ms(PULSE_MAKE_MS)), dialing);
                    }
                    self.push(Sound::Silence, Some(ms(PULSE_GAP_MS)), false);
                }
                c => {
                    if let Some((low, high)) = dtmf(c) {
                        self.push(Sound::Tones(low, high), Some(tone), dialing);
                        self.push(Sound::Silence, Some(tone), false);
                    }
                }
            }
        }
        self.push(Sound::Silence, Some(ms(AFTER_DIAL_MS)), false);
        self.push(Sound::Ringback, Some(ms(RING_ON_MS)), dialing);
        let frames = self.parts.iter().filter_map(|p| p.frames).sum();
        self.push(Sound::Ringback, None, dialing);
        frames
    }

    /// The other end answered: once the dialing is done, the handshake.
    pub fn answered(&mut self, speaker: u8) {
        if self.parts.front().is_some_and(|p| p.frames.is_none()) {
            self.at = 0;
        }
        self.parts.retain(|p| p.frames.is_some());
        self.push(Sound::Handshake, Some(handshake_frames()), speaker != 0);
    }

    /// Answering a call: the handshake.
    pub fn answer(&mut self, volume: u8, speaker: u8) {
        self.stop();
        self.gain = Self::gain(volume);
        self.push(Sound::Handshake, Some(handshake_frames()), speaker != 0);
    }

    pub fn stop(&mut self) {
        self.parts.clear();
        self.at = 0;
    }

    pub fn is_silent(&self) -> bool {
        self.parts.is_empty()
    }

    fn sample(pcm: &[u8], i: usize) -> f32 {
        i16::from_le_bytes([pcm[i * 2], pcm[i * 2 + 1]]) as f32
    }

    /// The recording at frame `at`, between samples a blend of the two
    /// around it.
    fn recorded(pcm: &[u8], at: u64, looped: bool) -> f32 {
        let len = (pcm.len() / 2) as u64;
        if len == 0 {
            return 0.0;
        }
        let i = (at / UPSAMPLE) % if looped { len } else { u64::MAX };
        if i >= len {
            return 0.0;
        }
        let next = if i + 1 < len { i + 1 } else if looped { 0 } else { i };
        let t = (at % UPSAMPLE) as f32 / UPSAMPLE as f32;
        Self::sample(pcm, i as usize) * (1.0 - t) + Self::sample(pcm, next as usize) * t
    }

    fn tone(frequency: f32, at: u64) -> f32 {
        (std::f32::consts::TAU * frequency * (at % RATE as u64) as f32 / RATE as f32).sin()
    }

    fn value(part: &Part, at: u64) -> f32 {
        match part.sound {
            Sound::Silence => 0.0,
            Sound::DialTone => Self::recorded(dial_tone(), at, true),
            Sound::Tones(low, high) => (Self::tone(low, at) + Self::tone(high, at)) * TONE,
            Sound::Click => {
                // A thump that dies away in a few milliseconds.
                let decay = (-(at as f32) / (RATE as f32 * 0.002)).exp();
                let buzz = Self::tone(180.0, at) * 0.6 + Self::tone(1100.0, at) * 0.4;
                buzz * decay * TONE * 2.0
            }
            Sound::Ringback => {
                let cycle = ms(RING_ON_MS + RING_OFF_MS);
                if at % cycle < ms(RING_ON_MS) {
                    (Self::tone(440.0, at) + Self::tone(480.0, at)) * TONE * 0.7
                } else {
                    0.0
                }
            }
            Sound::Handshake => Self::recorded(handshake(), at, false),
        }
    }

    /// The next mixer frame.
    pub fn render(&mut self) -> f32 {
        let Some(part) = self.parts.front() else { return 0.0 };
        let value = if part.audible { Self::value(part, self.at) * self.gain } else { 0.0 };
        self.at += 1;
        if part.frames.is_some_and(|frames| self.at >= frames) {
            self.parts.pop_front();
            self.at = 0;
        }
        value
    }

    /// Move on `frames` without playing them.
    pub fn skip(&mut self, mut frames: u64) {
        while frames > 0 {
            let Some(part) = self.parts.front() else { return };
            let Some(length) = part.frames else {
                self.at += frames;
                return;
            };
            let step = (length - self.at).min(frames);
            self.at += step;
            frames -= step;
            if self.at >= length {
                self.parts.pop_front();
                self.at = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The strength of `frequency` in `samples` (Goertzel).
    fn power(samples: &[f32], frequency: f32) -> f32 {
        let k = 2.0 * (std::f32::consts::TAU * frequency / RATE as f32).cos();
        let (mut a, mut b) = (0.0f32, 0.0f32);
        for &x in samples {
            let c = x + k * a - b;
            b = a;
            a = c;
        }
        (a * a + b * b - k * a * b).sqrt() / samples.len() as f32
    }

    fn frames(sound: &mut ModemSound, n: u64) -> Vec<f32> {
        (0..n).map(|_| sound.render()).collect()
    }

    #[test]
    fn touch_tones_follow_the_number() {
        let mut sound = ModemSound::default();
        let s = [0, 0, 43, 13, 10, 8, 2, 50, 2, 6, 14, 95, 50, 0, 0, 0];
        let length = sound.dial("5", false, false, &s, 3, 1);
        assert_eq!(length, ms(DIAL_TONE_MS) + 2 * ms(95) + ms(AFTER_DIAL_MS) + ms(RING_ON_MS));
        // The dial tone: 350 and 440 Hz.
        let tone = frames(&mut sound, ms(DIAL_TONE_MS));
        assert!(power(&tone, 350.0) > 4.0 * power(&tone, 770.0));
        // The 5: 770 and 1336 Hz.
        let five = frames(&mut sound, ms(95));
        for (yes, no) in [(770.0, 697.0), (1336.0, 1209.0)] {
            assert!(power(&five, yes) > 10.0 * power(&five, no), "{}", yes);
        }
        assert!(frames(&mut sound, ms(95 + AFTER_DIAL_MS)).iter().all(|&x| x == 0.0));
        // It rings until the other end answers, then the handshake plays
        // and the speaker goes quiet.
        sound.skip(ms(60_000));
        assert!(!sound.is_silent());
        sound.answered(1);
        sound.skip(handshake_frames());
        assert!(sound.is_silent());
    }

    #[test]
    fn speaker_settings() {
        let s = [0u8; 16];
        let mut sound = ModemSound::default();
        sound.dial("1", false, false, &s, 2, 0);
        assert!(frames(&mut sound, ms(3000)).iter().all(|&x| x == 0.0));
        let mut sound = ModemSound::default();
        sound.dial("1", false, true, &s, 2, 3);
        sound.answered(3);
        assert!(frames(&mut sound, ms(3000)).iter().all(|&x| x == 0.0));
        sound.skip(ms(1000));
        assert!(frames(&mut sound, ms(5000)).iter().any(|&x| x != 0.0));
    }

    #[test]
    fn hosts_dial_a_number_of_their_own() {
        let digits = digits_for_host("bbs.example.com:23");
        assert_eq!(digits.len(), 7);
        assert_eq!(digits, digits_for_host("BBS.example.com:23"));
        assert_ne!(digits, digits_for_host("other.example.com:23"));
        assert!(!digits.starts_with(['0', '1']));
    }
}
