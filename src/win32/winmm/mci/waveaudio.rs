//! MCI's waveaudio device: WAV files played on the sound device
//! (wave.rs's `play_wav`), as Windows 95's MCIWAVE does. A device opened
//! as "new" has no samples, and records nothing: there is no wave input
//! device.
//!
//! Positions are in bytes of the samples; the time formats are
//! milliseconds, bytes and samples. A play has the samples from its
//! start to its end played as one sound, and where it is follows from
//! the emulated time since it started.

use super::*;
use crate::win32::dsound::device::{Format, WAVE_FORMAT_PCM};
use crate::win32::winmm::wave;

/// The device's state, in its `Device`.
#[derive(Clone, Debug, Default)]
pub struct Wav {
    /// The samples' format, and the samples: none for a "new" device.
    pub format: Format,
    pub data: Vec<u8>,
    /// Whether a file was opened.
    pub file: bool,
    /// Where the device is, in bytes: where a play started, while it
    /// plays.
    pub pos: u32,
    /// Where the play ends, in bytes.
    pub to: u32,
    /// When the play started, in the clock's microseconds, while it
    /// plays; and its sound.
    pub started: Option<u64>,
    pub sound: u32,
    pub paused: bool,
    /// Cue output put it in its pseudo-paused state.
    pub cued: bool,
}

/// MCI_WAVE_SET_PARMS's flags.
pub const MCI_WAVE_SET_FORMATTAG: u32 = 0x0001_0000;
pub const MCI_WAVE_SET_CHANNELS: u32 = 0x0002_0000;
pub const MCI_WAVE_SET_SAMPLESPERSEC: u32 = 0x0004_0000;
pub const MCI_WAVE_SET_BITSPERSAMPLE: u32 = 0x0020_0000;
/// All of them, with those of the bytes a second and the alignment.
pub const MCI_WAVE_SET_FORMAT: u32 = 0x003F_0000;
pub const MCI_WAVE_INPUT: u32 = 0x0040_0000;
pub const MCI_WAVE_OUTPUT: u32 = 0x0080_0000;

/// MCI_STATUS's items of waveaudio.
pub const MCI_WAVE_STATUS_FORMATTAG: u32 = 0x4001;
pub const MCI_WAVE_STATUS_CHANNELS: u32 = 0x4002;
pub const MCI_WAVE_STATUS_SAMPLESPERSEC: u32 = 0x4003;
pub const MCI_WAVE_STATUS_AVGBYTESPERSEC: u32 = 0x4004;
pub const MCI_WAVE_STATUS_BLOCKALIGN: u32 = 0x4005;
pub const MCI_WAVE_STATUS_BITSPERSAMPLE: u32 = 0x4006;
pub const MCI_WAVE_STATUS_LEVEL: u32 = 0x4007;

/// MCI_GETDEVCAPS's.
pub const MCI_WAVE_GETDEVCAPS_INPUTS: u32 = 0x4001;
pub const MCI_WAVE_GETDEVCAPS_OUTPUTS: u32 = 0x4002;

/// The time formats waveaudio has besides milliseconds.
const MCI_FORMAT_BYTES: u32 = 8;
const MCI_FORMAT_SAMPLES: u32 = 9;

const MCIERR_NONAPPLICABLE_FUNCTION: u32 = 302;
const MCIERR_WAVE_INPUTSUNSUITABLE: u32 = 328;

/// The format a new device records in: 11025 Hz, 8 bits, mono.
const NEW_FORMAT: Format = Format::pcm(11025, 8, 1);

fn wav(cpu: &mut Cpu, i: usize) -> &mut Wav {
    &mut device(cpu, i).wave
}

/// The waveaudio driver (`WAVE_DRIVER`): the command `msg` of the device
/// `i`.
pub fn command(cpu: &mut Cpu, i: usize, msg: u32, flags: u32, p: &mut Parms) -> u32 {
    match msg {
        MCI_OPEN => open(cpu, i),
        MCI_CLOSE => {
            halt(cpu, i);
            0
        }
        MCI_PLAY => play(cpu, i, flags, p),
        MCI_STOP => {
            halt(cpu, i);
            let w = wav(cpu, i);
            w.paused = false;
            w.cued = false;
            0
        }
        MCI_PAUSE => {
            if wav(cpu, i).started.is_some() {
                halt(cpu, i);
                wav(cpu, i).paused = true;
                0
            } else if wav(cpu, i).paused {
                0
            } else {
                MCIERR_NONAPPLICABLE_FUNCTION
            }
        }
        MCI_RESUME => {
            if !wav(cpu, i).paused {
                return MCIERR_NONAPPLICABLE_FUNCTION;
            }
            wav(cpu, i).paused = false;
            start(cpu, i);
            0
        }
        MCI_SEEK => seek(cpu, i, flags, p),
        MCI_CUE => {
            let w = wav(cpu, i);
            if w.started.is_some() || w.paused {
                return MCIERR_NONAPPLICABLE_FUNCTION;
            }
            if flags & MCI_WAVE_INPUT != 0 {
                return MCIERR_WAVE_INPUTSUNSUITABLE;
            }
            w.cued = true;
            0
        }
        MCI_STATUS => status(cpu, i, flags, p),
        MCI_SET => set(cpu, i, flags, p),
        MCI_GETDEVCAPS => capabilities(flags, p),
        MCI_INFO => {
            if flags & MCI_INFO_PRODUCT != 0 {
                p.text = "Built-in Windows sound".to_string();
            } else if flags & MCI_INFO_FILE != 0 {
                if !wav(cpu, i).file {
                    return MCIERR_NONAPPLICABLE_FUNCTION;
                }
                p.text = device(cpu, i).element.clone();
            } else {
                return MCIERR_MISSING_PARAMETER;
            }
            p.ret_kind = Ret::Text;
            0
        }
        // No wave input device to record from.
        MCI_RECORD => MCIERR_WAVE_INPUTSUNSUITABLE,
        MCI_SAVE | MCI_DELETE => MCIERR_NONAPPLICABLE_FUNCTION,
        _ => MCIERR_UNSUPPORTED_FUNCTION,
    }
}

/// MCI_OPEN: the file the device opens, if any.
fn open(cpu: &mut Cpu, i: usize) -> u32 {
    let element = device(cpu, i).element.clone();
    wav(cpu, i).format = NEW_FORMAT;
    if element.is_empty() {
        return 0;
    }
    use crate::win32::kernel32;
    let handle = match kernel32::open_handle(cpu, &element, 0x8000_0000, 1, 3) {
        Ok(h) => h,
        Err(_) => return MCIERR_FILE_NOT_FOUND,
    };
    let mut bytes = Vec::new();
    loop {
        match kernel32::read_file(cpu, handle, 65536) {
            Ok(chunk) if !chunk.is_empty() => bytes.extend_from_slice(&chunk),
            _ => break,
        }
    }
    crate::win32::object::close(cpu, handle);
    let Some((format, mut data)) = wave::parse_wav(&bytes) else {
        return MCIERR_INVALID_FILE;
    };
    data.truncate(data.len() / format.block() as usize * format.block() as usize);
    let w = wav(cpu, i);
    w.format = format;
    w.data = data;
    w.file = true;
    0
}

/// A position in the device's time format, in bytes.
fn to_bytes(cpu: &mut Cpu, i: usize, value: u32) -> u64 {
    let tf = device(cpu, i).time_format;
    let f = wav(cpu, i).format;
    let block = f.block() as u64;
    match tf {
        MCI_FORMAT_BYTES => value as u64 / block * block,
        MCI_FORMAT_SAMPLES => value as u64 * block,
        _ => value as u64 * f.bytes_per_second() as u64 / 1000 / block * block,
    }
}

/// A position in bytes, in the device's time format.
fn from_bytes(cpu: &mut Cpu, i: usize, bytes: u32) -> u32 {
    let tf = device(cpu, i).time_format;
    let f = wav(cpu, i).format;
    match tf {
        MCI_FORMAT_BYTES => bytes,
        MCI_FORMAT_SAMPLES => bytes / f.block(),
        _ => (bytes as u64 * 1000 / f.bytes_per_second().max(1) as u64) as u32,
    }
}

/// Where the device is, in bytes: where its play has got to.
fn position(cpu: &mut Cpu, i: usize) -> u32 {
    let now = cpu.bus.clock.now_micros();
    let w = wav(cpu, i);
    let Some(started) = w.started else {
        return w.pos;
    };
    let frames = now.saturating_sub(started) * w.format.rate as u64 / 1_000_000;
    let at = w.pos as u64 + frames * w.format.block() as u64;
    at.min(w.to as u64) as u32
}

/// When the play of `d` ends, in the clock's microseconds, if it plays.
pub fn end_at(d: &Device) -> Option<u64> {
    let w = &d.wave;
    let started = w.started?;
    let frames = (w.to - w.pos) as u64 / w.format.block() as u64;
    Some(started + (frames * 1_000_000).div_ceil(w.format.rate.max(1) as u64))
}

/// The open device `i` at the time `now`: true when its play ended.
pub fn poll(cpu: &mut Cpu, i: usize, now: u64) -> bool {
    match end_at(device(cpu, i)) {
        Some(end) if end <= now => {
            let w = wav(cpu, i);
            w.pos = w.to;
            w.started = None;
            let sound = std::mem::take(&mut w.sound);
            if sound != 0 {
                wave::stop_sound(cpu, sound);
            }
            true
        }
        _ => false,
    }
}

/// The play stops where it is.
fn halt(cpu: &mut Cpu, i: usize) {
    if wav(cpu, i).started.is_none() {
        return;
    }
    let at = position(cpu, i);
    let w = wav(cpu, i);
    w.pos = at;
    w.started = None;
    let sound = std::mem::take(&mut w.sound);
    if sound != 0 {
        wave::stop_sound(cpu, sound);
    }
}

/// The play goes from `pos` to `to`, from now.
fn start(cpu: &mut Cpu, i: usize) {
    let w = wav(cpu, i);
    if w.pos >= w.to {
        return;
    }
    let (format, samples) = (w.format, w.data[w.pos as usize..w.to as usize].to_vec());
    let sound = wave::play_wav(cpu, format, samples, false);
    let now = cpu.bus.clock.now_micros();
    let w = wav(cpu, i);
    w.sound = sound;
    w.started = Some(now);
}

/// MCI_PLAY: from where the device is, or `from`, to its end, or `to`.
fn play(cpu: &mut Cpu, i: usize, flags: u32, p: &Parms) -> u32 {
    let length = wav(cpu, i).data.len() as u64;
    let from = if flags & MCI_FROM != 0 {
        to_bytes(cpu, i, p.from)
    } else {
        position(cpu, i) as u64
    };
    let to = if flags & MCI_TO != 0 {
        to_bytes(cpu, i, p.to)
    } else {
        length
    };
    if from > length || to > length || to < from {
        return MCIERR_OUTOFRANGE;
    }
    halt(cpu, i);
    let w = wav(cpu, i);
    w.pos = from as u32;
    w.to = to as u32;
    w.paused = false;
    w.cued = false;
    start(cpu, i);
    0
}

/// MCI_SEEK: to the start, the end or a position; it stops the play.
fn seek(cpu: &mut Cpu, i: usize, flags: u32, p: &Parms) -> u32 {
    let length = wav(cpu, i).data.len() as u64;
    let ways = [MCI_SEEK_TO_START, MCI_SEEK_TO_END, MCI_TO]
        .iter()
        .filter(|&&f| flags & f != 0)
        .count();
    if ways > 1 {
        return MCIERR_FLAGS_NOT_COMPATIBLE;
    }
    let to = if flags & MCI_SEEK_TO_START != 0 {
        0
    } else if flags & MCI_SEEK_TO_END != 0 {
        length
    } else if flags & MCI_TO != 0 {
        let to = to_bytes(cpu, i, p.to);
        if to > length {
            return MCIERR_OUTOFRANGE;
        }
        to
    } else {
        return MCIERR_MISSING_PARAMETER;
    };
    halt(cpu, i);
    let w = wav(cpu, i);
    w.pos = to as u32;
    w.paused = false;
    w.cued = false;
    0
}

fn status(cpu: &mut Cpu, i: usize, flags: u32, p: &mut Parms) -> u32 {
    if flags & MCI_STATUS_ITEM == 0 {
        return MCIERR_MISSING_PARAMETER;
    }
    let f = wav(cpu, i).format;
    let (ret, kind) = match p.item {
        MCI_STATUS_LENGTH => {
            let length = wav(cpu, i).data.len() as u32;
            (from_bytes(cpu, i, length), Ret::Int)
        }
        MCI_STATUS_POSITION => {
            let at = if flags & MCI_STATUS_START != 0 {
                0
            } else {
                position(cpu, i)
            };
            (from_bytes(cpu, i, at), Ret::Int)
        }
        MCI_STATUS_NUMBER_OF_TRACKS | MCI_STATUS_CURRENT_TRACK => (1, Ret::Int),
        MCI_STATUS_MODE => {
            let w = wav(cpu, i);
            let mode = if w.started.is_some() {
                MCI_MODE_PLAY
            } else if w.paused || w.cued {
                MCI_MODE_PAUSE
            } else {
                MCI_MODE_STOP
            };
            (mode, Ret::Constant)
        }
        MCI_STATUS_MEDIA_PRESENT | MCI_STATUS_READY => (TRUE, Ret::Bool),
        MCI_STATUS_TIME_FORMAT => (device(cpu, i).time_format, Ret::TimeFormat),
        MCI_WAVE_STATUS_FORMATTAG => (f.tag as u32, Ret::Int),
        MCI_WAVE_STATUS_CHANNELS => (f.channels as u32, Ret::Int),
        MCI_WAVE_STATUS_SAMPLESPERSEC => (f.rate, Ret::Int),
        MCI_WAVE_STATUS_AVGBYTESPERSEC => (f.bytes_per_second(), Ret::Int),
        MCI_WAVE_STATUS_BLOCKALIGN => (f.block(), Ret::Int),
        MCI_WAVE_STATUS_BITSPERSAMPLE => (f.bits as u32, Ret::Int),
        // Nothing records, so there is no input level.
        MCI_WAVE_STATUS_LEVEL => (0, Ret::Int),
        MCI_WAVE_INPUT | MCI_WAVE_OUTPUT => (0, Ret::Int),
        _ => return MCIERR_UNSUPPORTED_FUNCTION,
    };
    p.ret = ret;
    p.ret_kind = kind;
    0
}

/// MCI_SET: the time format, and a new device's format.
fn set(cpu: &mut Cpu, i: usize, flags: u32, p: &Parms) -> u32 {
    if flags & MCI_SET_TIME_FORMAT != 0
        && !matches!(
            p.time_format,
            MCI_FORMAT_MILLISECONDS | MCI_FORMAT_BYTES | MCI_FORMAT_SAMPLES
        )
    {
        return MCIERR_BAD_TIME_FORMAT;
    }
    if flags & MCI_WAVE_SET_FORMAT != 0 {
        if !wav(cpu, i).data.is_empty() {
            return MCIERR_NONAPPLICABLE_FUNCTION;
        }
        let mut f = wav(cpu, i).format;
        if flags & MCI_WAVE_SET_FORMATTAG != 0 {
            if p.format_tag != WAVE_FORMAT_PCM as u32 {
                return MCIERR_OUTOFRANGE;
            }
            f.tag = WAVE_FORMAT_PCM;
        }
        if flags & MCI_WAVE_SET_CHANNELS != 0 {
            if !matches!(p.channels, 1 | 2) {
                return MCIERR_OUTOFRANGE;
            }
            f.channels = p.channels as u16;
        }
        if flags & MCI_WAVE_SET_SAMPLESPERSEC != 0 {
            if !(100..=100_000).contains(&p.samples_per_sec) {
                return MCIERR_OUTOFRANGE;
            }
            f.rate = p.samples_per_sec;
        }
        if flags & MCI_WAVE_SET_BITSPERSAMPLE != 0 {
            if !matches!(p.bits_per_sample, 8 | 16) {
                return MCIERR_OUTOFRANGE;
            }
            f.bits = p.bits_per_sample as u16;
        }
        // The block alignment and bytes a second follow from the rest.
        wav(cpu, i).format = f;
    }
    if flags & MCI_SET_TIME_FORMAT != 0 {
        device(cpu, i).time_format = p.time_format;
    }
    0
}

/// MCI_GETDEVCAPS.
fn capabilities(flags: u32, p: &mut Parms) -> u32 {
    if flags & MCI_GETDEVCAPS_ITEM == 0 {
        return MCIERR_MISSING_PARAMETER;
    }
    let (ret, kind) = match p.item {
        MCI_GETDEVCAPS_DEVICE_TYPE => (MCI_DEVTYPE_WAVEFORM_AUDIO, Ret::Constant),
        MCI_GETDEVCAPS_HAS_AUDIO
        | MCI_GETDEVCAPS_USES_FILES
        | MCI_GETDEVCAPS_COMPOUND_DEVICE
        | MCI_GETDEVCAPS_CAN_PLAY
        | MCI_GETDEVCAPS_CAN_SAVE => (TRUE, Ret::Bool),
        MCI_GETDEVCAPS_CAN_RECORD | MCI_GETDEVCAPS_HAS_VIDEO | MCI_GETDEVCAPS_CAN_EJECT => {
            (FALSE, Ret::Bool)
        }
        MCI_WAVE_GETDEVCAPS_INPUTS => (0, Ret::Int),
        MCI_WAVE_GETDEVCAPS_OUTPUTS => (1, Ret::Int),
        _ => return MCIERR_UNSUPPORTED_FUNCTION,
    };
    p.ret = ret;
    p.ret_kind = kind;
    0
}

crate::state_fields!(Wav {
    format,
    data,
    file,
    pos,
    to,
    started,
    sound,
    paused,
    cued
});
