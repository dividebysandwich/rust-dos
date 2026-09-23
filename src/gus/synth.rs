//! A General MIDI synthesizer playing Ultrasound patches, for the MPU-401:
//! what a Gravis MIDI driver (ULTRAMID) does with the card, done on the
//! host. Envelopes run on the GF1's volume scale, as the patches were made
//! for it.

use std::sync::Arc;

use super::patch::{self, Patch, PatchBank, PatchSample};
use super::tables;

/// Output rate: the mixer's.
const RATE: f64 = crate::opl::RATE as f64;
const MAX_VOICES: usize = 64;
/// Frames a stolen or silenced voice takes to fade out.
const FADE: u32 = 64;
/// Frames between updates of vibrato and tremolo.
const CONTROL: u32 = 32;
/// Overall level, leaving headroom for many voices.
const MASTER: f32 = 0.4;
/// The GF1 frame rate envelopes step at: patches hold raw volume ramp
/// rates, and Gravis's own player (PLAYMIDI) runs the card with 20 voices.
const ENVELOPE_CLOCK: f32 = (1e6 / (1.619_695_497 * 20.0)) as f32;
const DRUMS: usize = 9;

#[derive(Clone, Copy)]
struct Channel {
    program: u8,
    volume: u8,
    expression: u8,
    /// Pan from controller 10; the patch's own until a program sets one.
    pan: Option<u8>,
    sustain: bool,
    modulation: u8,
    /// Pitch bend, -8192 to 8191, and its range in semitones.
    bend: i16,
    bend_range: f32,
    /// Registered parameter selected by controllers 101/100.
    rpn: (u8, u8),
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            program: 0,
            volume: 100,
            expression: 127,
            pan: None,
            sustain: false,
            modulation: 0,
            bend: 0,
            bend_range: 2.0,
            rpn: (127, 127),
        }
    }
}

impl Channel {
    fn bend_factor(&self) -> f64 {
        2f64.powf(self.bend as f64 / 8192.0 * self.bend_range as f64 / 12.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    On,
    /// Released while the sustain pedal is down.
    Sustained,
    Off,
}

struct Voice {
    channel: u8,
    note: u8,
    patch: Arc<Patch>,
    sample: usize,
    state: State,
    pos: f64,
    forward: bool,
    /// Samples a frame before pitch bend and vibrato, and with them.
    base_inc: f64,
    inc: f64,
    /// Envelope on the GF1 volume scale (0 to 4095): the stage coming
    /// next, the level, where it is heading and how fast.
    stage: usize,
    env: f32,
    target: f32,
    step: f32,
    velocity: f32,
    /// Left and right gains of the patch's pan.
    pan: (f32, f32),
    age: u64,
    fade: Option<u32>,
    control: u32,
    lfo_time: f32,
    vibrato: f64,
    tremolo: f32,
}

impl Voice {
    fn sample(&self) -> &PatchSample {
        &self.patch.samples[self.sample]
    }

    fn enveloped(&self) -> bool {
        self.sample().modes & patch::MODE_ENVELOPE != 0
    }

    fn looping(&self) -> bool {
        self.sample().modes & (patch::MODE_LOOP | patch::MODE_BIDIRECTIONAL) != 0
            && (self.enveloped() || self.state != State::Off)
    }

    /// Move to the next envelope stage that goes somewhere. Returns false
    /// when the envelope has run out.
    fn next_stage(&mut self) -> bool {
        loop {
            if self.stage > 5 {
                return false;
            }
            if self.state != State::Off && self.stage > 2 {
                // Hold the sustain level until the note is released.
                self.step = 0.0;
                self.target = self.env;
                return true;
            }
            let s = self.sample();
            let target = (s.env_offset[self.stage] as f32) * 16.0;
            let step = tables::ramp_increment(s.env_rate[self.stage]) as f32 / (1 << tables::RAMP_FRAC) as f32
                * (ENVELOPE_CLOCK / RATE as f32);
            self.stage += 1;
            if target != self.env {
                self.target = target;
                self.step = if target > self.env { step } else { -step };
                return true;
            }
        }
    }

    fn release(&mut self) {
        self.state = State::Off;
        if self.enveloped() {
            self.stage = 3;
            if !self.next_stage() {
                self.fade = Some(FADE);
            }
        }
    }

    /// Render one frame. Returns None when the voice has finished.
    #[inline]
    fn render(&mut self, gain: f32) -> Option<(f32, f32)> {
        let (value, start, end, modes) = {
            let s = &self.patch.samples[self.sample];
            let i = self.pos as usize;
            let a = *s.data.get(i)? as f32;
            let b = s.data.get(i + 1).map_or(a, |&b| b as f32);
            (a + (b - a) * (self.pos - i as f64) as f32, s.loop_start, s.loop_end, s.modes)
        };

        let level = if self.enveloped() { tables::volumes()[self.env.clamp(0.0, 4095.0) as usize] } else { 1.0 };
        let fade = self.fade.map_or(1.0, |f| f as f32 / FADE as f32);
        let g = value * level * gain * self.velocity * self.tremolo * fade;
        let out = (g * self.pan.0, g * self.pan.1);

        // Envelope.
        if self.enveloped() && self.step != 0.0 {
            self.env += self.step;
            if (self.step > 0.0 && self.env >= self.target) || (self.step < 0.0 && self.env <= self.target) {
                self.env = self.target;
                if !self.next_stage() {
                    return None;
                }
            }
        }
        if let Some(f) = &mut self.fade {
            if *f == 0 {
                return None;
            }
            *f -= 1;
        }

        // Position.
        let looping = self.looping();
        if self.forward {
            self.pos += self.inc;
            if looping && self.pos >= end {
                if modes & patch::MODE_BIDIRECTIONAL != 0 {
                    self.pos = (end - (self.pos - end)).max(start);
                    self.forward = false;
                } else {
                    let len = (end - start).max(1e-6);
                    self.pos = start + (self.pos - end) % len;
                }
            }
        } else {
            self.pos -= self.inc;
            if self.pos <= start {
                self.pos = (start + (start - self.pos)).min(end);
                self.forward = true;
            }
        }
        Some(out)
    }
}

/// Frequency of a (fractional) MIDI note in mHz.
fn note_freq(note: f64) -> f64 {
    440_000.0 * 2f64.powf((note - 69.0) / 12.0)
}

pub struct GusSynth {
    bank: PatchBank,
    channels: [Channel; 16],
    voices: Vec<Voice>,
    age: u64,
}

impl GusSynth {
    pub fn new(bank: PatchBank) -> Self {
        Self { bank, channels: [Channel::default(); 16], voices: Vec::new(), age: 0 }
    }

    /// Patches the bank names that could not be loaded.
    pub fn missing_patches(&self) -> &[String] {
        &self.bank.missing
    }

    pub fn active_voices(&self) -> usize {
        self.voices.len()
    }

    /// Silence everything and return the controllers to their defaults.
    pub fn reset(&mut self) {
        self.voices.clear();
        self.channels = [Channel::default(); 16];
    }

    /// A channel message: status (with the channel) and its data bytes.
    pub fn message(&mut self, status: u8, d1: u8, d2: u8) {
        let ch = (status & 0x0F) as usize;
        match status & 0xF0 {
            0x80 => self.note_off(ch, d1),
            0x90 if d2 == 0 => self.note_off(ch, d1),
            0x90 => self.note_on(ch, d1, d2),
            0xB0 => self.controller(ch, d1, d2),
            0xC0 => {
                self.channels[ch].program = d1 & 0x7F;
                if ch != DRUMS {
                    // Load it now rather than at its first note.
                    self.bank.melodic(d1 & 0x7F);
                }
            }
            0xE0 => {
                self.channels[ch].bend = ((d2 as i16) << 7 | d1 as i16) - 8192;
                self.update_pitch(ch);
            }
            _ => {}
        }
    }

    /// A System Exclusive message (without F0h and F7h): General MIDI, GS
    /// and XG resets.
    pub fn sysex(&mut self, data: &[u8]) {
        let gm_on = data.len() >= 4 && data[0] == 0x7E && data[2] == 0x09 && data[3] == 0x01;
        let gs_reset = data.len() >= 8 && data[0] == 0x41 && data[2] == 0x42 && data[3] == 0x12 && data[4..7] == [0x40, 0x00, 0x7F];
        let xg_reset = data.len() >= 6 && data[0] == 0x43 && data[2] == 0x4C && data[3..6] == [0x00, 0x00, 0x7E];
        if gm_on || gs_reset || xg_reset {
            for v in &mut self.voices {
                v.fade.get_or_insert(FADE);
            }
            self.channels = [Channel::default(); 16];
        }
    }

    fn note_on(&mut self, ch: usize, note: u8, velocity: u8) {
        let patch = if ch == DRUMS { self.bank.drum(note) } else { self.bank.melodic(self.channels[ch].program) };
        let Some(patch) = patch else { return };
        // The same key again: the old note ends.
        for v in self.voices.iter_mut().filter(|v| v.channel as usize == ch && v.note == note && v.state != State::Off) {
            v.release();
        }

        let freq = note_freq(note as f64);
        let sample = patch.sample_for(freq as u32);
        let index = patch.samples.iter().position(|s| std::ptr::eq(s, sample)).unwrap_or(0);
        let sf = sample.scale_freq as f64;
        let played = sf + (note as f64 - sf) * sample.scale_factor as f64 / 1024.0;
        let base_inc = note_freq(played) / sample.root_freq as f64 * sample.rate as f64 / RATE;
        let pan = match self.channels[ch].pan {
            Some(p) => {
                let angle = p as f32 / 127.0 * std::f32::consts::FRAC_PI_2;
                (angle.cos(), angle.sin())
            }
            None => tables::pans()[sample.balance as usize],
        };
        let v = (velocity as f32 / 127.0).powi(2);

        self.make_room();
        self.age += 1;
        let mut voice = Voice {
            channel: ch as u8,
            note,
            patch: patch.clone(),
            sample: index,
            state: State::On,
            pos: 0.0,
            forward: true,
            base_inc,
            inc: base_inc,
            stage: 0,
            env: 0.0,
            target: 0.0,
            step: 0.0,
            velocity: v,
            pan,
            age: self.age,
            fade: None,
            control: 0,
            lfo_time: 0.0,
            vibrato: 1.0,
            tremolo: 1.0,
        };
        if voice.enveloped() {
            voice.next_stage();
        }
        voice.inc = base_inc * self.channels[ch].bend_factor();
        self.voices.push(voice);
    }

    /// Free a voice slot if all are taken: fade out the quietest released
    /// voice, or else the oldest.
    fn make_room(&mut self) {
        let playing = self.voices.iter().filter(|v| v.fade.is_none()).count();
        if playing < MAX_VOICES {
            return;
        }
        let victim = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.fade.is_none())
            .min_by(|(_, a), (_, b)| {
                let key = |v: &Voice| (v.state == State::On, if v.enveloped() { v.env } else { 4095.0 }, v.age);
                key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i);
        if let Some(i) = victim {
            self.voices[i].fade = Some(FADE);
        }
        // Fading voices don't count, but don't let them pile up.
        if self.voices.len() > MAX_VOICES + 16
            && let Some(i) = self.voices.iter().position(|v| v.fade.is_some())
        {
            self.voices.swap_remove(i);
        }
    }

    fn note_off(&mut self, ch: usize, note: u8) {
        let sustain = self.channels[ch].sustain;
        for v in self.voices.iter_mut().filter(|v| v.channel as usize == ch && v.note == note && v.state == State::On) {
            if sustain {
                v.state = State::Sustained;
            } else {
                v.release();
            }
        }
    }

    fn controller(&mut self, ch: usize, cc: u8, value: u8) {
        let c = &mut self.channels[ch];
        match cc {
            1 => c.modulation = value,
            6 => {
                if c.rpn == (0, 0) {
                    c.bend_range = value as f32 + c.bend_range.fract();
                }
            }
            38 => {
                if c.rpn == (0, 0) {
                    c.bend_range = c.bend_range.trunc() + value as f32 / 100.0;
                }
            }
            7 => c.volume = value,
            10 => c.pan = Some(value),
            11 => c.expression = value,
            64 => {
                c.sustain = value >= 64;
                if !c.sustain {
                    for v in self.voices.iter_mut().filter(|v| v.channel as usize == ch && v.state == State::Sustained) {
                        v.release();
                    }
                }
            }
            98 | 99 => c.rpn = (127, 127),
            100 => c.rpn.1 = value,
            101 => c.rpn.0 = value,
            120 => {
                for v in self.voices.iter_mut().filter(|v| v.channel as usize == ch) {
                    v.fade.get_or_insert(FADE);
                }
            }
            121 => {
                c.modulation = 0;
                c.expression = 127;
                c.sustain = false;
                c.bend = 0;
                c.rpn = (127, 127);
                self.update_pitch(ch);
                for v in self.voices.iter_mut().filter(|v| v.channel as usize == ch && v.state == State::Sustained) {
                    v.release();
                }
            }
            123..=127 => {
                for v in self.voices.iter_mut().filter(|v| v.channel as usize == ch && v.state == State::On) {
                    if c.sustain {
                        v.state = State::Sustained;
                    } else {
                        v.release();
                    }
                }
            }
            _ => {}
        }
    }

    fn update_pitch(&mut self, ch: usize) {
        let bend = self.channels[ch].bend_factor();
        for v in self.voices.iter_mut().filter(|v| v.channel as usize == ch) {
            v.inc = v.base_inc * bend * v.vibrato;
        }
    }

    /// Recompute a voice's vibrato and tremolo, every `CONTROL` frames.
    fn update_lfos(v: &mut Voice, c: &Channel) {
        v.lfo_time += CONTROL as f32 / RATE as f32;
        let t = v.lfo_time;
        let s = v.sample();
        let (vib_sweep, vib_rate, vib_depth) = (s.vibrato[0], s.vibrato[1], s.vibrato[2]);
        let (trem_sweep, trem_rate, trem_depth) = (s.tremolo[0], s.tremolo[1], s.tremolo[2]);
        // Sweeps fade the LFO in over sweep/38 seconds; rates are
        // rate/38 Hz (TiMidity's reading of the patch format).
        let fade_in = |sweep: u8| if sweep == 0 { 1.0 } else { (t * 38.0 / sweep as f32).min(1.0) };
        let mut semitones = 0.0f32;
        let mut hz = 5.5f32;
        if vib_depth > 0 && vib_rate > 0 {
            hz = vib_rate as f32 / 38.0;
            semitones = vib_depth as f32 / 64.0 * fade_in(vib_sweep);
        }
        semitones += c.modulation as f32 / 127.0 * 0.5;
        v.vibrato = if semitones > 0.0 {
            let lfo = (t * hz * std::f32::consts::TAU).sin();
            2f64.powf((semitones * lfo) as f64 / 12.0)
        } else {
            1.0
        };
        v.tremolo = if trem_depth > 0 && trem_rate > 0 {
            let lfo = (t * trem_rate as f32 / 38.0 * std::f32::consts::TAU).sin();
            1.0 - (lfo + 1.0) * trem_depth as f32 / 1024.0 * fade_in(trem_sweep)
        } else {
            1.0
        };
        v.inc = v.base_inc * c.bend_factor() * v.vibrato;
    }

    /// One stereo frame at the mixer's rate, in 16-bit sample units.
    pub fn render(&mut self) -> (f32, f32) {
        let mut l = 0.0;
        let mut r = 0.0;
        let mut i = 0;
        while i < self.voices.len() {
            let v = &mut self.voices[i];
            let c = &self.channels[v.channel as usize];
            if v.control == 0 {
                Self::update_lfos(v, c);
                v.control = CONTROL;
            }
            v.control -= 1;
            let gain = (c.volume as f32 / 127.0).powi(2) * (c.expression as f32 / 127.0).powi(2) * MASTER;
            match v.render(gain) {
                Some((a, b)) => {
                    l += a;
                    r += b;
                    i += 1;
                }
                None => {
                    self.voices.swap_remove(i);
                }
            }
        }
        (l, r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gus::patch::tests::{build, sine};
    use crate::gus::patch::{MODE_ENVELOPE, MODE_LOOP};

    /// A synthesizer whose every program and drum key is `file`.
    fn synth_with(file: &[u8]) -> (GusSynth, tempdir::Dir) {
        let dir = tempdir::Dir::new();
        std::fs::write(dir.path().join("TONE.PAT"), file).unwrap();
        let mut ini = String::from("[Melodic Bank 0]\n");
        for p in 0..128 {
            ini.push_str(&format!("{}=tone\n", p));
        }
        ini.push_str("[Drum Bank 0]\n35=tone\n");
        (GusSynth::new(PatchBank::from_ini(&ini, dir.path())), dir)
    }

    /// A looped 441 Hz sine (100 samples at 44.1 kHz) with a sustaining
    /// envelope, rooted at 441 Hz.
    fn tone() -> Vec<u8> {
        build(&[(sine(100, 100), 0, 100, MODE_LOOP | MODE_ENVELOPE, 441_000, 0, 100_000_000)])
    }

    fn render(s: &mut GusSynth, frames: usize) -> Vec<f32> {
        (0..frames).map(|_| s.render().0).collect()
    }

    fn crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    mod tempdir {
        pub struct Dir(std::path::PathBuf);
        impl Dir {
            pub fn new() -> Self {
                static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let p = std::env::temp_dir().join(format!("rust-dos-gus-{}-{}", std::process::id(), n));
                std::fs::create_dir_all(&p).unwrap();
                Self(p)
            }
            pub fn path(&self) -> &std::path::Path {
                &self.0
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn a_note_sounds_at_its_pitch() {
        let (mut s, _dir) = synth_with(&tone());
        // A above middle C is 440 Hz: about 440 crossings in a second.
        s.message(0x90, 69, 127);
        let out = render(&mut s, 44100);
        let n = crossings(&out[4410..]) as f64 / 0.9;
        assert!((n - 440.0).abs() < 3.0, "{} Hz", n);
        // An octave up.
        s.message(0x80, 69, 0);
        render(&mut s, 44100);
        s.message(0x90, 81, 127);
        let out = render(&mut s, 44100);
        let n = crossings(&out[4410..]) as f64 / 0.9;
        assert!((n - 880.0).abs() < 5.0, "{} Hz", n);
    }

    #[test]
    fn release_fades_to_silence_and_frees_the_voice() {
        let (mut s, _dir) = synth_with(&tone());
        s.message(0x90, 60, 100);
        assert!(peak(&render(&mut s, 4410)) > 1000.0);
        s.message(0x80, 60, 0);
        render(&mut s, 44100);
        assert_eq!(s.active_voices(), 0);
    }

    #[test]
    fn sustain_pedal_holds_notes() {
        let (mut s, _dir) = synth_with(&tone());
        s.message(0xB0, 64, 127);
        s.message(0x90, 60, 100);
        s.message(0x80, 60, 0);
        render(&mut s, 44100);
        assert_eq!(s.active_voices(), 1);
        s.message(0xB0, 64, 0);
        render(&mut s, 44100);
        assert_eq!(s.active_voices(), 0);
    }

    #[test]
    fn pitch_bend_follows_the_range() {
        let (mut s, _dir) = synth_with(&tone());
        // Range 12 semitones through RPN 0, then bend fully up.
        for (cc, v) in [(101, 0), (100, 0), (6, 12), (38, 0)] {
            s.message(0xB0, cc, v);
        }
        s.message(0xE0, 0x7F, 0x7F);
        s.message(0x90, 69, 127);
        let out = render(&mut s, 44100);
        let n = crossings(&out[4410..]) as f64 / 0.9;
        assert!((n - 880.0).abs() < 6.0, "{} Hz", n);
    }

    #[test]
    fn all_notes_off_and_gm_reset() {
        let (mut s, _dir) = synth_with(&tone());
        s.message(0x90, 60, 100);
        s.message(0x90, 64, 100);
        s.message(0xB0, 123, 0);
        render(&mut s, 44100);
        assert_eq!(s.active_voices(), 0);
        s.message(0xB0, 7, 20);
        s.sysex(&[0x7E, 0x7F, 0x09, 0x01]);
        assert_eq!(s.channels[0].volume, 100);
    }

    #[test]
    fn fixed_pitch_drums_play_at_their_root() {
        let mut file = tone();
        // Scale factor 0: every key plays the pitch of the scale note (A,
        // 440 Hz, near the sample's root).
        let at = HEADER_TO_SAMPLE + 56;
        file[at..at + 2].copy_from_slice(&69i16.to_le_bytes());
        file[at + 2..at + 4].copy_from_slice(&0u16.to_le_bytes());
        let (mut s, _dir) = synth_with(&file);
        s.message(0x99, 35, 127);
        let out = render(&mut s, 44100);
        let n = crossings(&out[4410..]) as f64 / 0.9;
        assert!((n - 440.0).abs() < 3.0, "{} Hz", n);
    }

    const HEADER_TO_SAMPLE: usize = 129 + 63 + 47;

    #[test]
    fn voices_are_stolen_when_all_are_busy() {
        let (mut s, _dir) = synth_with(&tone());
        for n in 0..100u8 {
            s.message(0x90 | (n % 8), 20 + n % 80, 100);
            s.render();
        }
        render(&mut s, 200);
        assert!(s.active_voices() <= MAX_VOICES);
    }
}
