//! The noises disk drives make, as DOSBox Staging plays them: a hard disk
//! spinning up and then humming on, a floppy's motor running while it's in
//! use, and the heads seeking on each access.
//!
//! The samples are DOSBox Staging's (`assets/disknoise`), 16-bit mono at
//! 22050 Hz, played at twice that for the mixer's 44.1 kHz. Seeks sound
//! different for sequential access (the same file as the last access, or a
//! nearby track) and random access.

use std::io::Cursor;

use crate::diskio::{DiskClass, NoiseMode};

/// The samples' rate is half the mixer's.
const SAMPLE_RATE: u32 = 22_050;
/// DOSBox's level for the samples.
const GAIN: f32 = 0.2;

/// How an access relates to the ones before, for picking its seek sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Reading or writing a file (`DiskController::file_key`).
    File { write: bool, key: u64 },
    /// Reading or writing sectors on a track.
    Track(u64),
    /// Opening, creating or seeking: as the last access was.
    Other,
}

/// The samples of an embedded WAVE file.
fn pcm(wav: &'static [u8]) -> &'static [u8] {
    let (format, offset, size) = crate::cdrom::image::wave_data(&mut Cursor::new(wav)).expect("a disk noise sample");
    assert!(format.channels == 1 && format.bits == 16 && format.rate == SAMPLE_RATE, "disk noise samples are 16-bit mono");
    let end = (offset as usize + (size as usize & !1)).min(wav.len() & !1);
    &wav[offset as usize..end]
}

macro_rules! sample {
    ($name:literal) => {
        pcm(include_bytes!(concat!("../assets/disknoise/", $name, ".wav")))
    };
}

/// The sounds of one kind of drive.
struct Sounds {
    spin_up: Option<&'static [u8]>,
    spin: &'static [u8],
    seeks: [&'static [u8]; 9],
}

impl Sounds {
    fn hard_disk() -> Self {
        Sounds {
            spin_up: Some(sample!("hdd_spinup")),
            spin: sample!("hdd_spin"),
            seeks: [
                sample!("hdd_seek1"),
                sample!("hdd_seek2"),
                sample!("hdd_seek3"),
                sample!("hdd_seek4"),
                sample!("hdd_seek5"),
                sample!("hdd_seek6"),
                sample!("hdd_seek7"),
                sample!("hdd_seek8"),
                sample!("hdd_seek9"),
            ],
        }
    }

    fn floppy() -> Self {
        Sounds {
            spin_up: None,
            spin: sample!("fdd_spin"),
            seeks: [
                sample!("fdd_seek1"),
                sample!("fdd_seek2"),
                sample!("fdd_seek3"),
                sample!("fdd_seek4"),
                sample!("fdd_seek5"),
                sample!("fdd_seek6"),
                sample!("fdd_seek7"),
                sample!("fdd_seek8"),
                sample!("fdd_seek9"),
            ],
        }
    }
}

/// A sample playing.
#[derive(Clone, Copy, Debug)]
struct Voice {
    pcm: &'static [u8],
    /// Where it is, in mixer frames: two per sample.
    at: u64,
    looped: bool,
}

impl Voice {
    fn new(pcm: &'static [u8], looped: bool) -> Self {
        Voice { pcm, at: 0, looped }
    }

    fn samples(&self) -> u64 {
        (self.pcm.len() / 2) as u64
    }

    fn sample(&self, i: u64) -> f32 {
        let i = i as usize * 2;
        i16::from_le_bytes([self.pcm[i], self.pcm[i + 1]]) as f32
    }

    /// The next mixer frame, between samples the average of the two
    /// around it; None once a sample that doesn't loop has ended.
    fn next(&mut self) -> Option<f32> {
        let len = self.samples();
        if len == 0 {
            return None;
        }
        if self.at / 2 >= len {
            if !self.looped {
                return None;
            }
            self.at = 0;
        }
        let i = self.at / 2;
        let a = self.sample(i);
        let value = if self.at & 1 == 0 {
            a
        } else {
            let b = if i + 1 < len { self.sample(i + 1) } else if self.looped { self.sample(0) } else { a };
            (a + b) / 2.0
        };
        self.at += 1;
        Some(value)
    }

    /// Move on `frames`; false if it ended meanwhile.
    fn skip(&mut self, frames: u64) -> bool {
        let len = self.samples() * 2;
        self.at += frames;
        if self.looped && len > 0 {
            self.at %= len;
        }
        self.at < len
    }
}

/// One kind of drive: what it's playing and what it's doing.
struct Device {
    class: DiskClass,
    mode: NoiseMode,
    sounds: Sounds,
    spin_up: Option<Voice>,
    spin: Option<Voice>,
    seek: Option<Voice>,
    /// Mixer frames of disk access still going on.
    busy: u64,
    /// Whether the last access went on from the one before.
    sequential: bool,
    /// The files last read and written.
    last_file: [Option<u64>; 2],
    last_track: Option<u64>,
}

impl Device {
    fn new(class: DiskClass, sounds: Sounds) -> Self {
        Device {
            class,
            mode: NoiseMode::Off,
            sounds,
            spin_up: None,
            spin: None,
            seek: None,
            busy: 0,
            sequential: false,
            last_file: [None, None],
            last_track: None,
        }
    }

    fn set_mode(&mut self, mode: NoiseMode) {
        if mode == self.mode {
            return;
        }
        let was = self.mode;
        self.mode = mode;
        match mode {
            NoiseMode::Off => {
                self.spin_up = None;
                self.spin = None;
                self.seek = None;
                self.busy = 0;
            }
            NoiseMode::SeekOnly => {
                self.spin_up = None;
                self.spin = None;
            }
            // A hard disk spins up when it's switched on, and on and on.
            NoiseMode::On if self.class == DiskClass::HardDisk && was != NoiseMode::On => {
                self.spin_up = self.sounds.spin_up.map(|pcm| Voice::new(pcm, false));
                self.spin = None;
            }
            NoiseMode::On => {}
        }
    }

    /// The disk is being accessed: a floppy's motor runs, and the heads
    /// move unless they already are.
    fn activity(&mut self, rng: &mut Rng) {
        if self.mode == NoiseMode::On && self.class == DiskClass::Floppy && self.spin.is_none() {
            self.spin = Some(Voice::new(self.sounds.spin, false));
        }
        if self.seek.is_none() {
            let index = self.seek_index(rng);
            self.seek = Some(Voice::new(self.sounds.seeks[index], false));
        }
    }

    /// Which seek sample an access makes: one of the first two for
    /// sequential access; for random access on a floppy mostly those as
    /// well, and otherwise any.
    fn seek_index(&self, rng: &mut Rng) -> usize {
        let count = self.sounds.seeks.len();
        match (self.sequential, self.class) {
            (true, _) => rng.below(2),
            (false, DiskClass::Floppy) if rng.below(10) < 8 => rng.below(2),
            (false, DiskClass::Floppy) => 2 + rng.below(count - 2),
            (false, DiskClass::HardDisk) => rng.below(count),
        }
    }

    fn io(&mut self, access: Access, busy: u64, rng: &mut Rng) {
        if self.mode == NoiseMode::Off {
            return;
        }
        match access {
            Access::File { write, key } => {
                let last = &mut self.last_file[write as usize];
                self.sequential = *last == Some(key);
                *last = Some(key);
            }
            Access::Track(track) => {
                self.sequential = self.last_track.is_some_and(|last| last.abs_diff(track) <= 1);
                self.last_track = Some(track);
            }
            Access::Other => {}
        }
        self.activity(rng);
        self.busy = self.busy.max(busy);
    }

    fn render(&mut self, rng: &mut Rng) -> f32 {
        if self.mode == NoiseMode::Off {
            return 0.0;
        }
        // While the access goes on, the motor keeps running and the heads
        // keep moving.
        if self.busy > 0 {
            self.busy -= 1;
            self.activity(rng);
        }
        let mut value = 0.0;
        if let Some(voice) = &mut self.spin_up {
            match voice.next() {
                Some(v) => value += v,
                None => {
                    self.spin_up = None;
                    self.spin = Some(Voice::new(self.sounds.spin, true));
                }
            }
        } else if let Some(voice) = &mut self.spin {
            match voice.next() {
                Some(v) => value += v,
                None => self.spin = None,
            }
        }
        if let Some(voice) = &mut self.seek {
            match voice.next() {
                Some(v) => value += v,
                None => self.seek = None,
            }
        }
        value * GAIN
    }

    fn skip(&mut self, frames: u64) {
        self.busy = self.busy.saturating_sub(frames);
        self.seek = None;
        if let Some(voice) = &mut self.spin_up
            && !voice.skip(frames)
        {
            self.spin_up = None;
            self.spin = Some(Voice::new(self.sounds.spin, true));
        }
        if let Some(voice) = &mut self.spin
            && !voice.skip(frames)
        {
            self.spin = None;
        }
    }
}

/// A small random number generator for picking seek sounds (xorshift).
struct Rng(u32);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x as usize % n
    }
}

/// The noises of the floppy drives and the hard disks.
pub struct DiskNoise {
    floppy: Device,
    hard_disk: Device,
    rng: Rng,
}

impl Default for DiskNoise {
    fn default() -> Self {
        Self::new()
    }
}

impl DiskNoise {
    pub fn new() -> Self {
        DiskNoise {
            floppy: Device::new(DiskClass::Floppy, Sounds::floppy()),
            hard_disk: Device::new(DiskClass::HardDisk, Sounds::hard_disk()),
            rng: Rng(0x2545_F491),
        }
    }

    fn device(&mut self, class: DiskClass) -> &mut Device {
        match class {
            DiskClass::Floppy => &mut self.floppy,
            DiskClass::HardDisk => &mut self.hard_disk,
        }
    }

    pub fn set_modes(&mut self, floppy: NoiseMode, hard_disk: NoiseMode) {
        self.floppy.set_mode(floppy);
        self.hard_disk.set_mode(hard_disk);
    }

    /// Whether a kind of drive makes noises.
    pub fn enabled(&self, class: DiskClass) -> bool {
        match class {
            DiskClass::Floppy => self.floppy.mode != NoiseMode::Off,
            DiskClass::HardDisk => self.hard_disk.mode != NoiseMode::Off,
        }
    }

    /// A drive of `class` is accessed, for `busy` mixer frames from now.
    pub fn io(&mut self, class: DiskClass, access: Access, busy: u64) {
        let device = match class {
            DiskClass::Floppy => &mut self.floppy,
            DiskClass::HardDisk => &mut self.hard_disk,
        };
        device.io(access, busy, &mut self.rng);
    }

    /// The next mixer frame, mono, on the 16-bit scale.
    pub fn render(&mut self) -> f32 {
        let rng = &mut self.rng;
        self.floppy.render(rng) + self.hard_disk.render(rng)
    }

    /// Move on `frames` without playing them, after a long pause.
    pub fn skip(&mut self, frames: u64) {
        self.floppy.skip(frames);
        self.hard_disk.skip(frames);
    }

    /// Whether nothing is playing.
    pub fn is_silent(&mut self) -> bool {
        [DiskClass::Floppy, DiskClass::HardDisk].into_iter().all(|class| {
            let device = self.device(class);
            device.spin_up.is_none() && device.spin.is_none() && device.seek.is_none()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(noise: &mut DiskNoise, n: usize) -> Vec<f32> {
        (0..n).map(|_| noise.render()).collect()
    }

    #[test]
    fn samples_play_at_twice_their_rate() {
        let pcm: &'static [u8] = &[0x00, 0x00, 0x64, 0x00, 0xC8, 0x00];
        let mut voice = Voice::new(pcm, false);
        let played: Vec<f32> = std::iter::from_fn(|| voice.next()).collect();
        assert_eq!(played, [0.0, 50.0, 100.0, 150.0, 200.0, 200.0]);
        let mut looped = Voice::new(pcm, true);
        let played: Vec<f32> = (0..8).filter_map(|_| looped.next()).collect();
        assert_eq!(played, [0.0, 50.0, 100.0, 150.0, 200.0, 100.0, 0.0, 50.0]);
    }

    #[test]
    fn silent_when_off() {
        let mut noise = DiskNoise::new();
        noise.io(DiskClass::Floppy, Access::Other, 1000);
        assert!(frames(&mut noise, 2000).iter().all(|&v| v == 0.0));
        assert!(noise.is_silent());
    }

    #[test]
    fn floppy_access_seeks_and_spins() {
        let mut noise = DiskNoise::new();
        noise.set_modes(NoiseMode::On, NoiseMode::Off);
        assert!(noise.is_silent());
        noise.io(DiskClass::Floppy, Access::File { write: false, key: 1 }, 0);
        assert!(frames(&mut noise, 4410).iter().any(|&v| v != 0.0));
        // The motor runs out a few seconds after the last access.
        let _ = frames(&mut noise, 44_100 * 3);
        assert!(noise.is_silent());

        // Seek only: no motor, the heads fall silent after the seek.
        noise.set_modes(NoiseMode::SeekOnly, NoiseMode::Off);
        noise.io(DiskClass::Floppy, Access::Other, 0);
        let _ = frames(&mut noise, 44_100);
        assert!(noise.is_silent());
    }

    #[test]
    fn long_access_keeps_the_heads_moving() {
        let mut noise = DiskNoise::new();
        noise.set_modes(NoiseMode::SeekOnly, NoiseMode::Off);
        noise.io(DiskClass::Floppy, Access::Other, 44_100 * 2);
        let played = frames(&mut noise, 44_100 * 2);
        // No stretch of half a second without sound.
        assert!(played.chunks(22_050).all(|c| c.iter().any(|&v| v != 0.0)));
    }

    #[test]
    fn hard_disks_spin_up_and_on() {
        let mut noise = DiskNoise::new();
        noise.set_modes(NoiseMode::Off, NoiseMode::On);
        assert!(frames(&mut noise, 4410).iter().any(|&v| v != 0.0));
        noise.skip(44_100 * 60);
        assert!(!noise.is_silent(), "the spin loops");
        noise.set_modes(NoiseMode::Off, NoiseMode::Off);
        assert!(noise.is_silent());
    }

    #[test]
    fn sequential_access_picks_the_first_seeks() {
        let mut device = Device::new(DiskClass::HardDisk, Sounds::hard_disk());
        device.set_mode(NoiseMode::SeekOnly);
        let mut rng = Rng(1);
        device.io(Access::Track(10), 0, &mut rng);
        assert!(!device.sequential);
        device.io(Access::Track(11), 0, &mut rng);
        assert!(device.sequential);
        device.io(Access::File { write: true, key: 5 }, 0, &mut rng);
        device.io(Access::File { write: true, key: 5 }, 0, &mut rng);
        assert!(device.sequential);
        for _ in 0..100 {
            assert!(device.seek_index(&mut rng) < 2);
        }
        device.io(Access::File { write: false, key: 6 }, 0, &mut rng);
        assert!(!device.sequential);
    }
}
