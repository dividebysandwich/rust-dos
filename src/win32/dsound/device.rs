//! The sound device the built-in Windows plays DirectSound's buffers and
//! WinMM's wave audio on: a mixer that reads the samples where the
//! programs keep them, in their memory, at the emulated rate, into the
//! host's sound (`render`, which `Bus::audio_catch_up` calls for the
//! Windows channel of the host's mixer).
//!
//! Where a sound is at is a function of the emulated time alone: each
//! sound that plays keeps the time and position it was last set at (its
//! anchor) and its rate, so the play cursors programs read, the
//! notifications that fire and the samples heard agree, and runs are the
//! same each time. Whatever changes how a sound goes on (Play, Stop, a
//! new position, rate or volume, waveOutWrite) first has the host's mixer
//! catch up (`catch_up`), so what was heard until then was heard with the
//! sound as it was.
//!
//! The samples are read through the page tables of the process that owns
//! them (its cr3), whichever process runs, and never written.

use super::super::Win32;
use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::timer::PIT_HZ;

/// The host's sample rate, which `render` makes frames at.
pub const RATE: u64 = crate::opl::RATE as u64;

/// A PCM format: WAVEFORMATEX's fields, without what follows from them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Format {
    pub tag: u16,
    pub channels: u16,
    pub rate: u32,
    pub bits: u16,
}

pub const WAVE_FORMAT_PCM: u16 = 1;

/// The bytes of a WAVEFORMATEX.
pub const WAVEFORMATEX_SIZE: u32 = 18;

impl Format {
    pub const fn pcm(rate: u32, bits: u16, channels: u16) -> Format {
        Format {
            tag: WAVE_FORMAT_PCM,
            channels,
            rate,
            bits,
        }
    }

    /// Bytes of a frame (nBlockAlign).
    pub fn block(&self) -> u32 {
        (self.channels as u32 * self.bits as u32 / 8).max(1)
    }

    /// Bytes a second (nAvgBytesPerSec).
    pub fn bytes_per_second(&self) -> u32 {
        self.rate * self.block()
    }

    /// Whether the device plays it: PCM, 8 or 16 bits, mono or stereo.
    pub fn playable(&self) -> bool {
        self.tag == WAVE_FORMAT_PCM
            && matches!(self.bits, 8 | 16)
            && matches!(self.channels, 1 | 2)
    }

    /// The WAVEFORMATEX this format is, as 18 bytes (cbSize 0).
    pub fn to_bytes(self) -> [u8; 18] {
        let mut b = [0; 18];
        b[0..2].copy_from_slice(&self.tag.to_le_bytes());
        b[2..4].copy_from_slice(&self.channels.to_le_bytes());
        b[4..8].copy_from_slice(&self.rate.to_le_bytes());
        b[8..12].copy_from_slice(&self.bytes_per_second().to_le_bytes());
        b[12..14].copy_from_slice(&(self.block() as u16).to_le_bytes());
        b[14..16].copy_from_slice(&self.bits.to_le_bytes());
        b
    }

    /// The format of the 16 bytes of a PCMWAVEFORMAT (WAVEFORMATEX without
    /// cbSize), and its nBlockAlign.
    pub fn from_bytes(b: &[u8; 16]) -> (Format, u16) {
        let format = Format {
            tag: u16::from_le_bytes([b[0], b[1]]),
            channels: u16::from_le_bytes([b[2], b[3]]),
            rate: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            bits: u16::from_le_bytes([b[14], b[15]]),
        };
        (format, u16::from_le_bytes([b[12], b[13]]))
    }

    /// The format of the WAVEFORMAT the program passed at `at`: its first
    /// 16 bytes, as a PCMWAVEFORMAT has, and its nBlockAlign. None if
    /// they can't be read.
    pub fn read(cpu: &mut Cpu, at: u32) -> Option<(Format, u16)> {
        if at == 0 || !cpu.bus.guest_probe(at, 16, false) {
            return None;
        }
        let mut b = [0; 16];
        cpu.bus.guest_read_bytes(at, &mut b);
        Some(Format::from_bytes(&b))
    }
}

/// Where a sound that plays is, in its frames (32.32 fixed point), at the
/// time `num / den` seconds: from `pos` at the PIT tick `tick`, on at
/// `rate` frames a second. Before `tick` it is at `pos`.
pub fn position(tick: u64, pos: u64, rate: u32, num: u64, den: u64) -> u64 {
    let now = num as u128 * PIT_HZ as u128;
    let then = tick as u128 * den as u128;
    if now <= then {
        return pos;
    }
    let frames = ((now - then) * rate as u128) << 32;
    pos.saturating_add((frames / (den as u128 * PIT_HZ as u128)) as u64)
}

/// The first PIT tick at which a sound at `pos` at `tick`, on at `rate`
/// frames a second, reaches the frame `target` (32.32): `tick` if it is
/// there already, None if it doesn't move.
pub fn reaches(tick: u64, pos: u64, rate: u32, target: u64) -> Option<u64> {
    if target <= pos {
        return Some(tick);
    }
    if rate == 0 {
        return None;
    }
    let left = (target - pos) as u128 * PIT_HZ as u128;
    let per = (rate as u128) << 32;
    Some(tick + left.div_ceil(per) as u64)
}

/// The host's mixer catches up to now, before a sound changes.
pub fn catch_up(cpu: &mut Cpu) {
    cpu.bus.audio_catch_up();
}

/// Reads a process's memory through its page tables, a page at a time.
pub struct Reader<'a> {
    bus: &'a Bus,
    memory: Option<&'a super::super::memory::Memory>,
    page: u32,
    phys: Option<u32>,
}

impl<'a> Reader<'a> {
    pub fn new(bus: &'a Bus, pid: u32) -> Reader<'a> {
        let memory = bus
            .win32
            .processes
            .iter()
            .find(|p| p.id == pid)
            .map(|p| &p.memory);
        Reader {
            bus,
            memory,
            page: u32::MAX,
            phys: None,
        }
    }

    /// The byte at `lin`, or 0 where no page is.
    pub fn byte(&mut self, lin: u32) -> u8 {
        let page = lin & !0xFFF;
        if page != self.page {
            self.page = page;
            self.phys = self.memory.and_then(|m| m.translate(self.bus, page));
        }
        self.phys
            .map_or(0, |p| self.bus.read_8((p | (lin & 0xFFF)) as usize))
    }

    /// The frame `index` of the samples at `at` in `format`, on the scale
    /// of 16-bit samples, left and right.
    pub fn frame(&mut self, at: u32, format: &Format, index: u32) -> (f32, f32) {
        let start = at.wrapping_add(index.wrapping_mul(format.block()));
        let sample = |r: &mut Self, i: u32| -> f32 {
            if format.bits == 8 {
                (r.byte(start.wrapping_add(i)) as f32 - 128.0) * 256.0
            } else {
                let lo = r.byte(start.wrapping_add(2 * i));
                let hi = r.byte(start.wrapping_add(2 * i + 1));
                i16::from_le_bytes([lo, hi]) as f32
            }
        };
        let left = sample(self, 0);
        if format.channels == 2 {
            (left, sample(self, 1))
        } else {
            (left, left)
        }
    }
}

/// Samples of `frames` frames in `format`, from `start` (32.32): the
/// frame there, between it and the next by how far it is between them.
/// `wrap` frames make a loop (0 for none: past them is silence).
pub fn interpolate(
    reader: &mut Reader,
    at: u32,
    format: &Format,
    frames: u32,
    wrap: bool,
    pos: u64,
) -> (f32, f32) {
    if frames == 0 {
        return (0.0, 0.0);
    }
    let whole = pos >> 32;
    let (i, next) = if wrap {
        let i = (whole % frames as u64) as u32;
        (i, (i + 1) % frames)
    } else {
        if whole >= frames as u64 {
            return (0.0, 0.0);
        }
        let i = whole as u32;
        (i, (i + 1).min(frames - 1))
    };
    let frac = (pos & 0xFFFF_FFFF) as f32 / 4_294_967_296.0;
    let (l0, r0) = reader.frame(at, format, i);
    if frac == 0.0 || next == i {
        return (l0, r0);
    }
    let (l1, r1) = reader.frame(at, format, next);
    (l0 + (l1 - l0) * frac, r0 + (r1 - r0) * frac)
}

/// Gains for a volume in hundredths of a decibel (0 the loudest) and a
/// pan (-10000 left, 10000 right), left and right.
pub fn gains(volume: i32, pan: i32) -> (f32, f32) {
    let gain = |db100: i32| -> f32 {
        if db100 <= -10000 {
            0.0
        } else {
            10f32.powf(db100 as f32 / 2000.0)
        }
    };
    let g = gain(volume);
    let left = if pan > 0 { gain(-pan) } else { 1.0 };
    let right = if pan < 0 { gain(pan) } else { 1.0 };
    (g * left, g * right)
}

/// Whether anything plays on the device, for the host's mixer to mix it.
pub fn playing(win32: &Win32) -> bool {
    super::playing(&win32.dsound) || super::super::winmm::wave::playing(&win32.winmm.wave)
}

/// The device's sound for the host's frames `first` to `first + frames`
/// (frames at `RATE` since the machine started), on the scale of 16-bit
/// samples, at the mixer's wave volume: None if nothing plays then.
pub fn render(bus: &Bus, first: u64, frames: usize) -> Option<Vec<(f32, f32)>> {
    if !playing(&bus.win32) {
        return None;
    }
    let mut out = super::render(bus, first, frames).unwrap_or_else(|| vec![(0.0, 0.0); frames]);
    super::super::winmm::wave::render(bus, first, &mut out);
    // The mixer's wave line, which DirectSound plays through too.
    let (gl, gr) = bus.win32.winmm.mixer.gain(super::super::winmm::mixer::WAVE);
    if (gl, gr) != (1.0, 1.0) {
        for (l, r) in &mut out {
            *l *= gl;
            *r *= gr;
        }
    }
    Some(out)
}
