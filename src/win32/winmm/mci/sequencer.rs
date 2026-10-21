//! MCI's MIDI sequencer (sequencer): a Standard MIDI File, read whole
//! when the device opens, played on the MPU-401's synthesizer at the
//! emulated times of its events, by the process's multimedia thread.
//!
//! The file's tracks are merged into one list of events with their
//! times in microseconds, by the file's tempo changes. Positions are in
//! milliseconds. A play from the middle sends the controllers, programs
//! and pitch bends before that point first, so that the instruments are
//! the ones the song has there; a stop or pause ends the notes.

use super::*;
use crate::win32::winmm::midi;

/// MCI_SEQ_STATUS's items, and what they return.
const MCI_SEQ_STATUS_TEMPO: u32 = 0x4002;
const MCI_SEQ_STATUS_PORT: u32 = 0x4003;
const MCI_SEQ_STATUS_SLAVE: u32 = 0x4007;
const MCI_SEQ_STATUS_MASTER: u32 = 0x4008;
const MCI_SEQ_STATUS_OFFSET: u32 = 0x4009;
const MCI_SEQ_STATUS_DIVTYPE: u32 = 0x400A;
const MCI_SEQ_DIV_PPQN: u32 = 1216;
const MCI_SEQ_DIV_SMPTE_24: u32 = 1217;
const MCI_SEQ_FILE: u32 = 1220;
const MCI_SEQ_NONE: u32 = 65533;
const MCI_SEQ_MAPPER: u32 = 65535;

/// MCI_SET's flags of the sequencer.
const MCI_SEQ_SET_TEMPO: u32 = 0x0001_0000;
const MCI_SEQ_SET_PORT: u32 = 0x0002_0000;
const MCI_SEQ_SET_SLAVE: u32 = 0x0004_0000;
const MCI_SEQ_SET_MASTER: u32 = 0x0008_0000;
const MCI_SEQ_SET_OFFSET: u32 = 0x0100_0000;

/// The default tempo, in microseconds a quarter note.
const TEMPO: u32 = 500_000;

/// An event of the song: its time from the start, in microseconds, and
/// the bytes it sends.
#[derive(Clone, Debug, Default)]
pub struct Event {
    pub at: u64,
    pub data: Vec<u8>,
}

/// The sequencer's state.
#[derive(Clone, Debug, Default)]
pub struct Seq {
    pub events: Vec<Event>,
    /// The song's length, in microseconds.
    pub length: u64,
    /// The tempo the song starts at, and its division (as the file has
    /// it).
    pub tempo: u32,
    pub division: u16,
    /// Where it is when it doesn't play, in microseconds of the song.
    pub pos: u64,
    /// While it plays: the clock's time of the song's start, where it
    /// is to end, and the next event.
    pub started: Option<u64>,
    pub until: u64,
    pub next: usize,
    /// Whether it sends to the synthesizer (port none doesn't).
    pub port: u32,
    pub paused: bool,
}

fn seq(cpu: &mut Cpu, i: usize) -> &mut Seq {
    &mut device(cpu, i).seq
}

/// A variable-length number of the file at `at`.
fn var_len(data: &[u8], at: &mut usize) -> Option<u32> {
    let mut value = 0u32;
    for _ in 0..4 {
        let b = *data.get(*at)?;
        *at += 1;
        value = value << 7 | (b & 0x7F) as u32;
        if b & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Read a Standard MIDI File: its events, with their ticks, the tempo
/// changes with theirs, and the division. None when it isn't one.
/// What `parse` reads: the events (tick, track, bytes), the tempo
/// changes (tick, microseconds a quarter note) and the division.
type Song = (Vec<(u64, usize, Vec<u8>)>, Vec<(u64, u32)>, u16);

fn parse(data: &[u8]) -> Option<Song> {
    // A RIFF MIDI file (.RMI) holds one in its "data" chunk.
    let data = if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"RMID") {
        let mut at = 12;
        loop {
            let id = data.get(at..at + 4)?;
            let len = u32::from_le_bytes(data.get(at + 4..at + 8)?.try_into().ok()?) as usize;
            if id == b"data" {
                break data.get(at + 8..at + 8 + len)?;
            }
            at += 8 + len.div_ceil(2) * 2;
        }
    } else {
        data
    };
    if !data.starts_with(b"MThd") {
        return None;
    }
    let be32 = |at: usize| Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?));
    let be16 = |at: usize| Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?));
    let header = be32(4)? as usize;
    let tracks = be16(10)?;
    let division = be16(12)?;
    let mut events = Vec::new();
    let mut tempos = Vec::new();
    let mut at = 8 + header;
    for track in 0..tracks as usize {
        if data.get(at..at + 4) != Some(b"MTrk") {
            break;
        }
        let len = be32(at + 4)? as usize;
        let end = (at + 8 + len).min(data.len());
        let mut p = at + 8;
        let mut tick = 0u64;
        let mut status = 0u8;
        while p < end {
            tick += var_len(&data[..end], &mut p)? as u64;
            let mut b = *data.get(p)?;
            if b & 0x80 != 0 {
                p += 1;
            } else {
                b = status;
            }
            match b {
                0xFF => {
                    let kind = *data.get(p)?;
                    p += 1;
                    let n = var_len(&data[..end], &mut p)? as usize;
                    let body = data.get(p..p + n)?;
                    p += n;
                    match kind {
                        0x51 if n == 3 => {
                            let t = (body[0] as u32) << 16 | (body[1] as u32) << 8 | body[2] as u32;
                            tempos.push((tick, t));
                        }
                        0x2F => break,
                        _ => {}
                    }
                }
                0xF0 | 0xF7 => {
                    let n = var_len(&data[..end], &mut p)? as usize;
                    let body = data.get(p..p + n)?;
                    p += n;
                    let mut bytes = Vec::with_capacity(n + 1);
                    if b == 0xF0 {
                        bytes.push(0xF0);
                    }
                    bytes.extend_from_slice(body);
                    events.push((tick, track, bytes));
                }
                0x80..=0xEF => {
                    status = b;
                    let n = if matches!(b & 0xF0, 0xC0 | 0xD0) {
                        1
                    } else {
                        2
                    };
                    let body = data.get(p..p + n)?;
                    p += n;
                    let mut bytes = vec![b];
                    bytes.extend_from_slice(body);
                    events.push((tick, track, bytes));
                }
                _ => return None,
            }
        }
        at += 8 + len;
    }
    // In time order, the tracks' order kept for events at one time.
    events.sort_by_key(|&(tick, track, _)| (tick, track));
    tempos.sort_by_key(|&(tick, _)| tick);
    Some((events, tempos, division))
}

/// The song's events with their times, its length, and the tempo it
/// starts at.
fn timed(data: &[u8]) -> Option<(Vec<Event>, u64, u32, u16)> {
    let (events, tempos, division) = parse(data)?;
    // Microseconds a tick: by the tempo for PPQN, fixed for SMPTE.
    let smpte = division & 0x8000 != 0;
    let per_tick = |tempo: u32| -> f64 {
        if smpte {
            let fps = -((division >> 8) as i8) as f64;
            let ticks = (division & 0xFF) as f64;
            1_000_000.0 / (fps * ticks).max(1.0)
        } else {
            tempo as f64 / (division.max(1)) as f64
        }
    };
    let start_tempo = tempos.first().filter(|t| t.0 == 0).map_or(TEMPO, |t| t.1);
    let mut out = Vec::with_capacity(events.len());
    let (mut last_tick, mut time, mut tempo) = (0u64, 0f64, TEMPO);
    let mut changes = tempos.iter().peekable();
    let mut advance = |to: u64, time: &mut f64, tempo: &mut u32, last: &mut u64| {
        while let Some(&&(tick, t)) = changes.peek() {
            if tick > to {
                break;
            }
            *time += (tick - *last) as f64 * per_tick(*tempo);
            *last = tick;
            *tempo = t;
            changes.next();
        }
        *time += (to - *last) as f64 * per_tick(*tempo);
        *last = to;
    };
    for (tick, _, data) in events {
        advance(tick, &mut time, &mut tempo, &mut last_tick);
        out.push(Event {
            at: time as u64,
            data,
        });
    }
    let length = out.last().map_or(0, |e| e.at);
    Some((out, length, start_tempo, division))
}

/// Open the device: its file is read.
pub fn open(cpu: &mut Cpu, i: usize) -> u32 {
    let element = device(cpu, i).element.clone();
    seq(cpu, i).port = MCI_SEQ_MAPPER;
    seq(cpu, i).tempo = TEMPO;
    if element.is_empty() {
        return 0;
    }
    use crate::win32::kernel32;
    let handle = match kernel32::open_handle(cpu, &element, 0x8000_0000, 1, 3) {
        Ok(h) => h,
        Err(_) => return MCIERR_FILE_NOT_FOUND,
    };
    let mut data = Vec::new();
    loop {
        match kernel32::read_file(cpu, handle, 65536) {
            Ok(chunk) if !chunk.is_empty() => data.extend_from_slice(&chunk),
            _ => break,
        }
    }
    crate::win32::object::close(cpu, handle);
    let Some((events, length, tempo, division)) = timed(&data) else {
        return MCIERR_INVALID_FILE;
    };
    let s = seq(cpu, i);
    s.events = events;
    s.length = length;
    s.tempo = tempo;
    s.division = division;
    0
}

/// Close the device: its notes end.
pub fn close(cpu: &mut Cpu, i: usize) {
    halt(cpu, i);
}

/// When its play ends, if it plays.
pub fn end_at(d: &Device) -> Option<u64> {
    d.seq.started.map(|s| s + d.seq.until)
}

/// When it next has an event to send, or its play ends.
pub fn next_due(d: &Device) -> Option<u64> {
    let s = &d.seq;
    let start = s.started?;
    let event = s.events.get(s.next).filter(|e| e.at < s.until);
    Some(start + event.map_or(s.until, |e| e.at))
}

/// Where it is, in microseconds of the song.
fn position(cpu: &Cpu, i: usize) -> u64 {
    let s = &cpu.bus.win32.winmm.mci.devices[i].seq;
    match s.started {
        Some(start) => {
            let now = cpu.bus.clock.now_micros();
            now.saturating_sub(start).min(s.until)
        }
        None => s.pos,
    }
}

/// Stop where it is: the notes end.
fn halt(cpu: &mut Cpu, i: usize) {
    let at = position(cpu, i);
    let s = seq(cpu, i);
    let was = s.started.take().is_some();
    s.pos = at;
    if was && s.port != MCI_SEQ_NONE {
        midi::notes_off(cpu);
    }
}

/// Send an event's bytes, unless the port is none.
fn send(cpu: &mut Cpu, i: usize, k: usize) {
    if seq(cpu, i).port == MCI_SEQ_NONE {
        return;
    }
    let data = std::mem::take(&mut seq(cpu, i).events[k].data);
    midi::send(cpu, &data);
    seq(cpu, i).events[k].data = data;
}

/// Start playing from `from` to `until` (microseconds of the song).
fn start(cpu: &mut Cpu, i: usize, from: u64, until: u64) {
    let now = cpu.bus.clock.now_micros();
    let first = seq(cpu, i).events.partition_point(|e| e.at < from);
    // What sets the instruments up before `from` is sent first.
    for k in 0..first {
        let status = seq(cpu, i).events[k].data.first().copied().unwrap_or(0);
        if !matches!(status & 0xF0, 0x80 | 0x90 | 0xA0) {
            send(cpu, i, k);
        }
    }
    let s = seq(cpu, i);
    s.started = Some(now - from.min(now));
    s.until = until;
    s.next = first;
    s.paused = false;
}

/// The device's commands.
pub fn command(cpu: &mut Cpu, i: usize, msg: u32, flags: u32, p: &mut Parms) -> u32 {
    let length = seq(cpu, i).length;
    let ms = |v: u32| v as u64 * 1000;
    match msg {
        MCI_PLAY => {
            let from = if flags & MCI_FROM != 0 {
                ms(p.from)
            } else {
                position(cpu, i)
            };
            let to = if flags & MCI_TO != 0 {
                ms(p.to)
            } else {
                length
            };
            // (A position a millisecond past the end is the end.)
            if from > length + 999 || to > length + 999 || to < from {
                return MCIERR_OUTOFRANGE;
            }
            halt(cpu, i);
            let (from, to) = (from.min(length), to.min(length));
            seq(cpu, i).pos = from;
            if from >= to {
                return 0;
            }
            start(cpu, i, from, to);
            0
        }
        MCI_SEEK => {
            let to = if flags & MCI_SEEK_TO_START != 0 {
                0
            } else if flags & MCI_SEEK_TO_END != 0 {
                length
            } else if flags & MCI_TO != 0 {
                if ms(p.to) > length + 999 {
                    return MCIERR_OUTOFRANGE;
                }
                ms(p.to).min(length)
            } else {
                return MCIERR_MISSING_PARAMETER;
            };
            halt(cpu, i);
            seq(cpu, i).pos = to;
            0
        }
        MCI_STOP => {
            halt(cpu, i);
            seq(cpu, i).paused = false;
            0
        }
        MCI_PAUSE => {
            if seq(cpu, i).started.is_some() {
                let until = seq(cpu, i).until;
                halt(cpu, i);
                let s = seq(cpu, i);
                s.paused = true;
                s.until = until;
            }
            0
        }
        MCI_RESUME => {
            if seq(cpu, i).paused {
                let (from, until) = (seq(cpu, i).pos, seq(cpu, i).until);
                start(cpu, i, from, until);
                super::super::schedule(cpu);
            }
            0
        }
        MCI_STATUS => status(cpu, i, flags, p),
        MCI_SET => {
            if flags & MCI_SET_TIME_FORMAT != 0 {
                if p.time_format != MCI_FORMAT_MILLISECONDS {
                    return MCIERR_BAD_TIME_FORMAT;
                }
                device(cpu, i).time_format = p.time_format;
            }
            if flags & MCI_SEQ_SET_PORT != 0 {
                match p.port {
                    0 | MCI_SEQ_MAPPER => seq(cpu, i).port = MCI_SEQ_MAPPER,
                    MCI_SEQ_NONE => {
                        halt_notes(cpu, i);
                        seq(cpu, i).port = MCI_SEQ_NONE;
                    }
                    _ => return MCIERR_OUTOFRANGE,
                }
            }
            if flags & MCI_SEQ_SET_TEMPO != 0 && p.tempo == 0 {
                return MCIERR_OUTOFRANGE;
            }
            if flags & (MCI_SET_AUDIO | MCI_SET_VIDEO | MCI_SET_DOOR_OPEN | MCI_SET_DOOR_CLOSED)
                != 0
                || flags & (MCI_SEQ_SET_SLAVE | MCI_SEQ_SET_MASTER | MCI_SEQ_SET_OFFSET) != 0
            {
                return MCIERR_UNSUPPORTED_FUNCTION;
            }
            0
        }
        MCI_GETDEVCAPS => {
            if flags & MCI_GETDEVCAPS_ITEM == 0 {
                return MCIERR_MISSING_PARAMETER;
            }
            let (ret, kind) = match p.item {
                MCI_GETDEVCAPS_DEVICE_TYPE => (MCI_DEVTYPE_SEQUENCER, Ret::Constant),
                MCI_GETDEVCAPS_HAS_AUDIO
                | MCI_GETDEVCAPS_CAN_PLAY
                | MCI_GETDEVCAPS_USES_FILES
                | MCI_GETDEVCAPS_COMPOUND_DEVICE => (TRUE, Ret::Bool),
                MCI_GETDEVCAPS_CAN_RECORD
                | MCI_GETDEVCAPS_HAS_VIDEO
                | MCI_GETDEVCAPS_CAN_EJECT
                | MCI_GETDEVCAPS_CAN_SAVE => (FALSE, Ret::Bool),
                _ => return MCIERR_UNSUPPORTED_FUNCTION,
            };
            p.ret = ret;
            p.ret_kind = kind;
            0
        }
        MCI_INFO => {
            p.ret_kind = Ret::Text;
            if flags & MCI_INFO_PRODUCT != 0 {
                p.text = "MIDI Sequencer".to_string();
            } else if flags & MCI_INFO_FILE != 0 {
                p.text = device(cpu, i).element.clone();
            } else if flags & (MCI_INFO_MEDIA_UPC | MCI_INFO_MEDIA_IDENTITY) != 0 {
                return MCIERR_UNSUPPORTED_FUNCTION;
            } else {
                return MCIERR_MISSING_PARAMETER;
            }
            0
        }
        MCI_BREAK => 0,
        _ => MCIERR_UNSUPPORTED_FUNCTION,
    }
}

/// The notes end, as the port changes.
fn halt_notes(cpu: &mut Cpu, i: usize) {
    if seq(cpu, i).started.is_some() && seq(cpu, i).port != MCI_SEQ_NONE {
        midi::notes_off(cpu);
    }
}

/// MCI_STATUS.
fn status(cpu: &mut Cpu, i: usize, flags: u32, p: &mut Parms) -> u32 {
    if flags & MCI_STATUS_ITEM == 0 {
        return MCIERR_MISSING_PARAMETER;
    }
    let s = seq(cpu, i);
    let (length, started, paused, tempo, port, division) =
        (s.length, s.started, s.paused, s.tempo, s.port, s.division);
    let ms = |us: u64| (us / 1000) as u32;
    let (ret, kind) = match p.item {
        MCI_STATUS_LENGTH => (ms(length), Ret::Int),
        MCI_STATUS_POSITION => {
            let at = if flags & MCI_STATUS_START != 0 {
                0
            } else {
                position(cpu, i)
            };
            (ms(at), Ret::Int)
        }
        MCI_STATUS_NUMBER_OF_TRACKS | MCI_STATUS_CURRENT_TRACK => (1, Ret::Int),
        MCI_STATUS_MODE => {
            let mode = if started.is_some() {
                MCI_MODE_PLAY
            } else if paused {
                MCI_MODE_PAUSE
            } else {
                MCI_MODE_STOP
            };
            (mode, Ret::Constant)
        }
        MCI_STATUS_MEDIA_PRESENT | MCI_STATUS_READY => (TRUE, Ret::Bool),
        MCI_STATUS_TIME_FORMAT => (device(cpu, i).time_format, Ret::TimeFormat),
        // Beats a minute.
        MCI_SEQ_STATUS_TEMPO => (60_000_000 / tempo.max(1), Ret::Int),
        MCI_SEQ_STATUS_PORT => (port, Ret::Int),
        MCI_SEQ_STATUS_DIVTYPE => {
            let div = if division & 0x8000 != 0 {
                MCI_SEQ_DIV_SMPTE_24
            } else {
                MCI_SEQ_DIV_PPQN
            };
            (div, Ret::Constant)
        }
        MCI_SEQ_STATUS_SLAVE | MCI_SEQ_STATUS_MASTER => (MCI_SEQ_FILE, Ret::Constant),
        MCI_SEQ_STATUS_OFFSET => (0, Ret::Int),
        _ => return MCIERR_UNSUPPORTED_FUNCTION,
    };
    p.ret = ret;
    p.ret_kind = kind;
    0
}

/// The device looks at the time `now`: the events due go out. Whether
/// its play ended.
pub fn poll(cpu: &mut Cpu, i: usize, now: u64) -> bool {
    let Some(start) = seq(cpu, i).started else {
        return false;
    };
    let until = seq(cpu, i).until;
    let at = now.saturating_sub(start);
    loop {
        let s = seq(cpu, i);
        let k = s.next;
        match s.events.get(k) {
            Some(e) if e.at <= at && e.at < until => {
                s.next += 1;
                send(cpu, i, k);
            }
            _ => break,
        }
    }
    if at < until {
        return false;
    }
    halt(cpu, i);
    seq(cpu, i).pos = until;
    true
}

crate::state_fields!(Event { at, data });
crate::state_fields!(Seq {
    events,
    length,
    tempo,
    division,
    pos,
    started,
    until,
    next,
    port,
    paused
});
