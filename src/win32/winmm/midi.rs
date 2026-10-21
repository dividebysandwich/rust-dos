//! MIDI: the midiOut functions on the emulated MPU-401, whose bytes go to
//! the synthesizer it plays (Bus::mpu), the MIDI mapper in front of it,
//! MIDI streams (midiStream*), and the midiIn functions of a machine
//! that has no MIDI input.
//!
//! The device is the synthesizer behind the MPU-401, as a sound card's
//! own MIDI synthesizer was: it has a volume, which scales what the
//! synthesizer plays (`gain`). Messages go out at once; a long message
//! (System Exclusive) is sent whole before midiOutLongMsg returns, as
//! Windows 95's MPU-401 driver did, and its header is done then.
//!
//! A stream's buffers are read when midiStreamOut queues them, and their
//! events play at the stream's emulated times on the process's
//! multimedia thread (`poll`), which also writes the headers back and
//! makes the callbacks. As on Windows 9x, a header's dwOffset is the
//! offset of the event played last.

use super::callback::{self, Callback};
use super::*;
use crate::cpu::Cpu;

/// MIDI_MAPPER, as a device id.
pub const MIDI_MAPPER: u32 = 0xFFFF_FFFF;

/// The callback messages.
const MOM_OPEN: u32 = 0x3C7;
const MOM_CLOSE: u32 = 0x3C8;
const MOM_DONE: u32 = 0x3C9;
const MOM_POSITIONCB: u32 = 0x3CA;

/// MIDIHDR's flags, and the offsets of its fields.
const MHDR_DONE: u32 = 1;
const MHDR_PREPARED: u32 = 2;
const MHDR_INQUEUE: u32 = 4;
const MHDR_ISSTRM: u32 = 8;
const HDR_DATA: u32 = 0;
const HDR_LENGTH: u32 = 4;
const HDR_RECORDED: u32 = 8;
const HDR_FLAGS: u32 = 16;
const HDR_OFFSET: u32 = 28;
/// The size of a MIDIHDR without dwOffset, the least the functions take.
const HDR_OLD_SIZE: u32 = 28;
/// The largest buffer a stream plays.
const STREAM_BUFFER_MAX: u32 = 65_536;

/// MIDIOUTCAPS's technologies and support flags.
const MOD_SYNTH: u16 = 2;
const MOD_MAPPER: u16 = 5;
const MIDICAPS_VOLUME: u32 = 1;
const MIDICAPS_LRVOLUME: u32 = 2;
const MIDICAPS_STREAM: u32 = 8;

/// midiStreamProperty's flags.
const MIDIPROP_SET: u32 = 0x8000_0000;
const MIDIPROP_GET: u32 = 0x4000_0000;
const MIDIPROP_TIMEDIV: u32 = 1;
const MIDIPROP_TEMPO: u32 = 2;

/// A stream event's flags and types (the high byte of dwEvent).
const MEVT_F_LONG: u32 = 0x8000_0000;
const MEVT_F_CALLBACK: u32 = 0x4000_0000;
const MEVT_SHORTMSG: u8 = 0x00;
const MEVT_TEMPO: u8 = 0x01;
const MEVT_LONGMSG: u8 = 0x80;

/// MMTIME's types.
const TIME_MS: u32 = 0x0001;
const TIME_MIDI: u32 = 0x0010;
const TIME_TICKS: u32 = 0x0020;

/// The driver messages midiOutMessage passes on.
const MODM_DATA: u32 = 7;
const MODM_LONGDATA: u32 = 8;
const MODM_RESET: u32 = 9;
const MODM_GETVOLUME: u32 = 10;
const MODM_SETVOLUME: u32 = 11;
const MODM_PREPARE: u32 = 13;
const MODM_UNPREPARE: u32 = 14;

/// A stream's default tempo (microseconds a quarter note) and time
/// division (ticks a quarter note).
const DEFAULT_TEMPO: u32 = 500_000;
const DEFAULT_TIMEDIV: u32 = 24;

/// An event of a stream's buffer: where it is in the buffer, its delta
/// time, its dwEvent, and the bytes of a long one.
#[derive(Clone, Debug, Default)]
pub struct Event {
    pub offset: u32,
    pub delta: u32,
    pub event: u32,
    pub data: Vec<u8>,
}

/// A buffer midiStreamOut queued: its header, and its events, the next
/// to play at `at`.
#[derive(Clone, Debug, Default)]
pub struct Buffer {
    pub header: u32,
    pub events: Vec<Event>,
    pub at: u32,
}

/// What makes a MIDI out handle a stream's.
#[derive(Clone, Debug, Default)]
pub struct Stream {
    pub playing: bool,
    pub tempo: u32,
    pub timediv: u32,
    pub queue: Vec<Buffer>,
    /// The microseconds played before `started` (while paused, all of
    /// them), and when it last started, by the clock.
    pub elapsed: u64,
    pub started: u64,
    /// The tick of the event played last.
    pub tick: u64,
    /// Where the tempo last changed: the tick and the microseconds
    /// played, from which times are counted at the tempo.
    pub seg_tick: u64,
    pub seg_us: u64,
}

impl Stream {
    fn new() -> Stream {
        Stream {
            tempo: DEFAULT_TEMPO,
            timediv: DEFAULT_TIMEDIV,
            ..Stream::default()
        }
    }

    /// The microseconds a tick takes, as a fraction.
    fn rate(&self) -> (u64, u64) {
        let div = self.timediv & 0xFFFF;
        if div & 0x8000 != 0 {
            // SMPTE: frames a second and ticks a frame.
            let fps = (-((div >> 8) as u8 as i8) as i32).max(1) as u64;
            let per_frame = (div & 0xFF).max(1) as u64;
            (1_000_000, fps * per_frame)
        } else {
            (self.tempo.max(1) as u64, div.max(1) as u64)
        }
    }

    /// The microseconds played at `now`.
    fn played(&self, now: u64) -> u64 {
        if self.playing {
            self.elapsed + now.saturating_sub(self.started)
        } else {
            self.elapsed
        }
    }

    /// The microseconds played at which the tick `tick` comes.
    fn us_at(&self, tick: u64) -> u64 {
        let (num, den) = self.rate();
        self.seg_us + crate::timer::mul_div(tick.saturating_sub(self.seg_tick), num, den)
    }

    /// The tick at `played` microseconds.
    fn tick_at(&self, played: u64) -> u64 {
        let (num, den) = self.rate();
        self.seg_tick + crate::timer::mul_div(played.saturating_sub(self.seg_us), den, num)
    }

    /// When the next event plays, by the clock, if it does.
    fn next_due(&self) -> Option<u64> {
        if !self.playing {
            return None;
        }
        let buffer = self.queue.first()?;
        let due = match buffer.events.get(buffer.at as usize) {
            Some(e) => self.us_at(self.tick + e.delta as u64),
            // A buffer whose events have all played is done at once.
            None => self.played(self.started),
        };
        Some(self.started + due.saturating_sub(self.elapsed))
    }
}

/// An open MIDI out device or stream.
#[derive(Clone, Debug, Default)]
pub struct MidiOut {
    pub handle: u32,
    /// The device it was opened as: 0, or MIDI_MAPPER.
    pub device: u32,
    pub pid: u32,
    pub callback: Callback,
    pub instance: u32,
    pub stream: Option<Stream>,
}

/// MIDI's state.
#[derive(Clone, Debug, Default)]
pub struct Midi {
    pub outs: Vec<MidiOut>,
    /// The volume of the device and that the mapper reports, as
    /// midiOutSetVolume takes it (left in the low word), or None for
    /// full.
    pub volume: Option<u32>,
    pub mapper_volume: Option<u32>,
    /// The status byte the last message had, for running status.
    pub status: u8,
}

fn midi(cpu: &mut Cpu) -> &mut Midi {
    &mut cpu.bus.win32.winmm.midi
}

/// The open MIDI out device of `handle`, by its place.
fn find(cpu: &Cpu, handle: u32) -> Option<usize> {
    cpu.bus
        .win32
        .winmm
        .midi
        .outs
        .iter()
        .position(|o| o.handle == handle && handle != 0)
}

/// Whether `handle` is an open MIDI out device's.
pub fn is_handle(cpu: &Cpu, handle: u32) -> bool {
    find(cpu, handle).is_some()
}

/// The device a device id or handle names: 0 or MIDI_MAPPER, or None.
fn device_of(cpu: &Cpu, id: u32) -> Option<u32> {
    if let Some(i) = find(cpu, id) {
        return Some(cpu.bus.win32.winmm.midi.outs[i].device);
    }
    (id == 0 || id == MIDI_MAPPER).then_some(id)
}

/// The volume scale (left, right) of what the synthesizer plays: the
/// device's volume and the mixer's MIDI line.
pub fn gain(state: &super::State) -> (f32, f32) {
    let (l, r) = split(state.midi.volume.unwrap_or(0xFFFF_FFFF));
    let (ml, mr) = state.mixer.midi_gain();
    (l * ml, r * mr)
}

/// A volume (left in the low word, right in the high) as scales.
pub fn split(volume: u32) -> (f32, f32) {
    let l = (volume & 0xFFFF) as f32 / 65535.0;
    let r = (volume >> 16) as f32 / 65535.0;
    (l, r)
}

/// Send `bytes` to the MPU-401's synthesizer.
pub fn send(cpu: &mut Cpu, bytes: &[u8]) {
    cpu.bus.audio_catch_up();
    for &b in bytes {
        cpu.bus.mpu.write_data(b);
    }
}

/// How many data bytes a message with `status` has.
fn data_bytes(status: u8) -> usize {
    match status & 0xF0 {
        0xC0 | 0xD0 => 1,
        0xF0 => match status {
            0xF1 | 0xF3 => 1,
            0xF2 => 2,
            _ => 0,
        },
        _ => 2,
    }
}

/// Send the short message `msg` (status in the low byte), with running
/// status when it has none.
pub fn short_message(cpu: &mut Cpu, msg: u32) {
    let [b0, b1, b2, _] = msg.to_le_bytes();
    let (status, data) = if b0 & 0x80 != 0 {
        (b0, [b1, b2])
    } else {
        (midi(cpu).status, [b0, b1])
    };
    if status & 0x80 == 0 {
        return;
    }
    if status < 0xF0 {
        midi(cpu).status = status;
    }
    let mut bytes = vec![status];
    bytes.extend_from_slice(&data[..data_bytes(status)]);
    send(cpu, &bytes);
}

/// End every note: all notes and the sustain off on each channel.
pub fn notes_off(cpu: &mut Cpu) {
    let mut bytes = Vec::new();
    for ch in 0..16u8 {
        bytes.extend_from_slice(&[0xB0 | ch, 64, 0, 0xB0 | ch, 123, 0]);
    }
    send(cpu, &bytes);
    midi(cpu).status = 0;
}

/// Tell the callback of the open device `i` about `msg`.
fn notify(cpu: &mut Cpu, i: usize, msg: u32, p1: u32) {
    let o = &cpu.bus.win32.winmm.midi.outs[i];
    let (cb, handle, instance) = (o.callback.clone(), o.handle, o.instance);
    callback::notify(cpu, &cb, handle, msg, instance, p1, 0);
}

/// midiOutGetNumDevs: the MPU-401's synthesizer.
pub fn midi_out_get_num_devs(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

/// midiOutGetDevCapsA(id, caps, size): MIDIOUTCAPSA, as much of it as
/// `size` has room for.
pub fn midi_out_get_dev_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, at, size] = args else {
        unreachable!()
    };
    let Some(device) = device_of(cpu, id) else {
        return Some(MMSYSERR_BADDEVICEID);
    };
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    let mut caps = Vec::with_capacity(52);
    let (pid, name, tech, voices, support) = if device == MIDI_MAPPER {
        (
            1u16,
            "MIDI Mapper",
            MOD_MAPPER,
            0u16,
            MIDICAPS_VOLUME | MIDICAPS_LRVOLUME,
        )
    } else {
        (
            MM_MPU401_MIDIOUT,
            "MPU-401 Synthesizer",
            MOD_SYNTH,
            32,
            MIDICAPS_VOLUME | MIDICAPS_LRVOLUME | MIDICAPS_STREAM,
        )
    };
    caps.extend_from_slice(&MM_MICROSOFT.to_le_bytes());
    caps.extend_from_slice(&pid.to_le_bytes());
    caps.extend_from_slice(&0x0400u32.to_le_bytes());
    caps.extend_from_slice(&name_field(name, 32));
    caps.extend_from_slice(&tech.to_le_bytes());
    caps.extend_from_slice(&voices.to_le_bytes());
    caps.extend_from_slice(&voices.to_le_bytes());
    caps.extend_from_slice(&0xFFFFu16.to_le_bytes());
    caps.extend_from_slice(&support.to_le_bytes());
    write_struct(cpu, at, size, &caps)
}

/// midiOutOpen(handle, id, callback, instance, flags).
pub fn midi_out_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[out, id, cb, instance, flags] = args else {
        unreachable!()
    };
    open(cpu, out, id, cb, instance, flags, false)
}

/// Open the device `id` for midiOutOpen or midiStreamOpen (`stream`).
fn open(
    cpu: &mut Cpu,
    out: u32,
    id: u32,
    cb: u32,
    instance: u32,
    flags: u32,
    stream: bool,
) -> Option<u32> {
    if id != 0 && id != MIDI_MAPPER {
        return Some(MMSYSERR_BADDEVICEID);
    }
    if out == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(out, 4, true) {
        return None;
    }
    // Windows 98 has a function callback be a function.
    if flags & callback::CALLBACK_TYPEMASK == callback::CALLBACK_FUNCTION && cb == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    let callback = match Callback::from_open(cpu, cb, flags) {
        Ok(c) => c,
        Err(e) => return Some(e),
    };
    if callback::ensure_thread(cpu).is_none() {
        callback.release(cpu);
        return Some(MMSYSERR_NOMEM);
    }
    let handle = new_handle(cpu);
    let pid = crate::win32::user32::current_pid(cpu);
    midi(cpu).outs.push(MidiOut {
        handle,
        device: id,
        pid,
        callback,
        instance,
        stream: stream.then(Stream::new),
    });
    cpu.bus.guest_write_32(out, handle);
    let i = midi(cpu).outs.len() - 1;
    notify(cpu, i, MOM_OPEN, 0);
    Some(MMSYSERR_NOERROR)
}

/// midiOutClose(handle). A stream's queued buffers have to be done.
pub fn midi_out_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = find(cpu, args[0]) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    if midi(cpu).outs[i]
        .stream
        .as_ref()
        .is_some_and(|s| !s.queue.is_empty())
    {
        return Some(MIDIERR_STILLPLAYING);
    }
    Some(close(cpu, i))
}

fn close(cpu: &mut Cpu, i: usize) -> u32 {
    notify(cpu, i, MOM_CLOSE, 0);
    let o = midi(cpu).outs.remove(i);
    o.callback.release(cpu);
    super::schedule(cpu);
    MMSYSERR_NOERROR
}

/// midiOutShortMsg(handle, msg).
pub fn midi_out_short_msg(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if find(cpu, args[0]).is_none() {
        return Some(MMSYSERR_INVALHANDLE);
    }
    short_message(cpu, args[1]);
    Some(MMSYSERR_NOERROR)
}

/// Read the header `hdr` (`size` bytes of it as the program says): its
/// flags, or the error.
/// None when the page isn't there.
fn header_flags(cpu: &mut Cpu, hdr: u32, size: u32) -> Option<Result<u32, u32>> {
    if hdr == 0 || size < HDR_OLD_SIZE {
        return Some(Err(MMSYSERR_INVALPARAM));
    }
    if !cpu.bus.guest_probe(hdr, HDR_OLD_SIZE as usize, true) {
        return None;
    }
    Some(Ok(cpu.bus.guest_read_32(hdr + HDR_FLAGS)))
}

/// midiOutLongMsg(handle, header, size): the buffer's bytes go out, and
/// the header is done.
pub fn midi_out_long_msg(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, hdr, size] = args else {
        unreachable!()
    };
    let Some(i) = find(cpu, handle) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    let flags = match header_flags(cpu, hdr, size)? {
        Ok(f) => f,
        Err(e) => return Some(e),
    };
    if flags & MHDR_PREPARED == 0 {
        return Some(MIDIERR_UNPREPARED);
    }
    if flags & MHDR_INQUEUE != 0 {
        return Some(MIDIERR_STILLPLAYING);
    }
    let data = cpu.bus.guest_read_32(hdr + HDR_DATA);
    let len = cpu.bus.guest_read_32(hdr + HDR_LENGTH);
    if !cpu.bus.guest_probe(data, len as usize, false) {
        return None;
    }
    let mut bytes = vec![0; len as usize];
    cpu.bus.guest_read_bytes(data, &mut bytes);
    send(cpu, &bytes);
    midi(cpu).status = 0;
    cpu.bus
        .guest_write_32(hdr + HDR_FLAGS, (flags & !MHDR_INQUEUE) | MHDR_DONE);
    notify(cpu, i, MOM_DONE, hdr);
    Some(MMSYSERR_NOERROR)
}

/// midiOutPrepareHeader(handle, header, size). A stream's buffers are
/// 64 KB at most.
pub fn midi_out_prepare_header(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, hdr, size] = args else {
        unreachable!()
    };
    let Some(i) = find(cpu, handle) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    let flags = match header_flags(cpu, hdr, size)? {
        Ok(f) => f,
        Err(e) => return Some(e),
    };
    let len = cpu.bus.guest_read_32(hdr + HDR_LENGTH);
    if midi(cpu).outs[i].stream.is_some() && len > STREAM_BUFFER_MAX {
        return Some(MMSYSERR_INVALPARAM);
    }
    cpu.bus
        .guest_write_32(hdr + HDR_FLAGS, flags | MHDR_PREPARED);
    Some(MMSYSERR_NOERROR)
}

/// midiOutUnprepareHeader(handle, header, size).
pub fn midi_out_unprepare_header(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, hdr, size] = args else {
        unreachable!()
    };
    if find(cpu, handle).is_none() {
        return Some(MMSYSERR_INVALHANDLE);
    }
    let flags = match header_flags(cpu, hdr, size)? {
        Ok(f) => f,
        Err(e) => return Some(e),
    };
    if flags & MHDR_INQUEUE != 0 {
        return Some(MIDIERR_STILLPLAYING);
    }
    cpu.bus
        .guest_write_32(hdr + HDR_FLAGS, flags & !MHDR_PREPARED);
    Some(MMSYSERR_NOERROR)
}

/// midiOutReset(handle): the notes end, and a stream's buffers are done.
pub fn midi_out_reset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = find(cpu, args[0]) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    if midi(cpu).outs[i].stream.is_some() {
        stop(cpu, i);
    }
    notes_off(cpu);
    Some(MMSYSERR_NOERROR)
}

/// midiOutGetVolume(id or handle, volume).
pub fn midi_out_get_volume(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, at] = args else { unreachable!() };
    let Some(device) = device_of(cpu, id) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(at, 4, true) {
        return None;
    }
    let m = midi(cpu);
    let volume = if device == MIDI_MAPPER {
        m.mapper_volume
    } else {
        m.volume
    };
    cpu.bus.guest_write_32(at, volume.unwrap_or(0xFFFF_FFFF));
    Some(MMSYSERR_NOERROR)
}

/// midiOutSetVolume(id or handle, volume): the mapper's goes to the
/// device behind it too.
pub fn midi_out_set_volume(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, volume] = args else { unreachable!() };
    let Some(device) = device_of(cpu, id) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    let m = midi(cpu);
    if device == MIDI_MAPPER {
        m.mapper_volume = Some(volume);
    }
    m.volume = Some(volume);
    Some(MMSYSERR_NOERROR)
}

/// midiOutGetID(handle, id): the device it was opened as.
pub fn midi_out_get_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, at] = args else { unreachable!() };
    let Some(i) = find(cpu, handle) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(at, 4, true) {
        return None;
    }
    let device = midi(cpu).outs[i].device;
    cpu.bus.guest_write_32(at, device);
    Some(MMSYSERR_NOERROR)
}

/// midiOutGetErrorTextA and midiInGetErrorTextA (error, text, size).
pub fn midi_get_error_text(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[error, at, size] = args else {
        unreachable!()
    };
    error_text_to(cpu, error, at, size)
}

/// midiOutMessage(handle, message, p1, p2): the driver's messages that
/// have functions of their own.
pub fn midi_out_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, msg, p1, p2] = args else {
        unreachable!()
    };
    match msg {
        MODM_DATA => midi_out_short_msg(cpu, &[handle, p1]),
        MODM_LONGDATA => midi_out_long_msg(cpu, &[handle, p1, p2]),
        MODM_RESET => midi_out_reset(cpu, &[handle]),
        MODM_GETVOLUME => midi_out_get_volume(cpu, &[handle, p1]),
        MODM_SETVOLUME => midi_out_set_volume(cpu, &[handle, p1]),
        MODM_PREPARE => midi_out_prepare_header(cpu, &[handle, p1, p2]),
        MODM_UNPREPARE => midi_out_unprepare_header(cpu, &[handle, p1, p2]),
        _ if find(cpu, handle).is_none() => Some(MMSYSERR_INVALHANDLE),
        _ => Some(MMSYSERR_NOTSUPPORTED),
    }
}

/// midiOutCachePatches, midiOutCacheDrumPatches, midiConnect and
/// midiDisconnect: what the synthesizer doesn't do.
pub fn not_supported(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_NOTSUPPORTED)
}

/// midiInGetNumDevs: there is no MIDI input.
pub fn midi_in_get_num_devs(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// midiInGetDevCapsA and midiInOpen: no device.
pub fn midi_in_bad_device(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_BADDEVICEID)
}

/// The midiIn functions that take a handle: there is none.
pub fn midi_in_bad_handle(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_INVALHANDLE)
}

/// The open stream of `handle`, by its place.
fn find_stream(cpu: &Cpu, handle: u32) -> Option<usize> {
    find(cpu, handle).filter(|&i| cpu.bus.win32.winmm.midi.outs[i].stream.is_some())
}

fn stream(cpu: &mut Cpu, i: usize) -> &mut Stream {
    midi(cpu).outs[i].stream.as_mut().expect("a stream")
}

/// midiStreamOpen(handle, device ids, count, callback, instance, flags):
/// a stream on one device, paused.
pub fn midi_stream_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[out, ids, count, cb, instance, flags] = args else {
        unreachable!()
    };
    if count != 1 || ids == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(ids, 4, false) {
        return None;
    }
    let id = cpu.bus.guest_read_32(ids);
    open(cpu, out, id, cb, instance, flags, true)
}

/// midiStreamClose(handle): its buffers are done, then it closes.
pub fn midi_stream_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = find_stream(cpu, args[0]) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    stop(cpu, i);
    notes_off(cpu);
    Some(close(cpu, i))
}

/// The events of the buffer of `len` bytes at `data`, or None if they
/// don't fit it.
fn read_events(cpu: &mut Cpu, data: u32, len: u32) -> Option<Vec<Event>> {
    let mut bytes = vec![0; len as usize];
    cpu.bus.guest_read_bytes(data, &mut bytes);
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
    let mut events = Vec::new();
    let mut at = 0usize;
    while at + 12 <= bytes.len() {
        let (delta, event) = (word(at), word(at + 8));
        let mut e = Event {
            offset: at as u32,
            delta,
            event,
            data: Vec::new(),
        };
        at += 12;
        if event & MEVT_F_LONG != 0 {
            let n = (event & 0xFF_FFFF) as usize;
            if at + n > bytes.len() {
                return None;
            }
            e.data = bytes[at..at + n].to_vec();
            at += n.next_multiple_of(4);
        }
        events.push(e);
    }
    Some(events)
}

/// midiStreamOut(handle, header, size): the buffer's events are queued,
/// to play when the stream does.
pub fn midi_stream_out(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, hdr, size] = args else {
        unreachable!()
    };
    let Some(i) = find_stream(cpu, handle) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    let flags = match header_flags(cpu, hdr, size)? {
        Ok(f) => f,
        Err(e) => return Some(e),
    };
    if flags & MHDR_PREPARED == 0 {
        return Some(MIDIERR_UNPREPARED);
    }
    if flags & MHDR_INQUEUE != 0 {
        return Some(MIDIERR_STILLPLAYING);
    }
    let data = cpu.bus.guest_read_32(hdr + HDR_DATA);
    let len = cpu.bus.guest_read_32(hdr + HDR_LENGTH);
    let recorded = cpu.bus.guest_read_32(hdr + HDR_RECORDED);
    if recorded > len || !recorded.is_multiple_of(4) {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(data, recorded as usize, false) {
        return None;
    }
    let Some(events) = read_events(cpu, data, recorded) else {
        return Some(MMSYSERR_INVALPARAM);
    };
    cpu.bus
        .guest_write_32(hdr + HDR_FLAGS, (flags & !MHDR_DONE) | MHDR_INQUEUE);
    stream(cpu, i).queue.push(Buffer {
        header: hdr,
        events,
        at: 0,
    });
    started(cpu, i);
    Some(MMSYSERR_NOERROR)
}

/// The stream `i` may have work now: its process's multimedia thread
/// looks.
fn started(cpu: &mut Cpu, i: usize) {
    let pid = midi(cpu).outs[i].pid;
    callback::wake_thread(cpu, pid);
    super::schedule(cpu);
}

/// midiStreamRestart(handle): it plays.
pub fn midi_stream_restart(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = find_stream(cpu, args[0]) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    let now = cpu.bus.clock.now_micros();
    let s = stream(cpu, i);
    if !s.playing {
        s.playing = true;
        s.started = now;
    }
    started(cpu, i);
    Some(MMSYSERR_NOERROR)
}

/// midiStreamPause(handle): it stays where it is, and its notes end.
pub fn midi_stream_pause(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = find_stream(cpu, args[0]) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    let now = cpu.bus.clock.now_micros();
    let s = stream(cpu, i);
    if s.playing {
        s.elapsed = s.played(now);
        s.playing = false;
        notes_off(cpu);
    }
    super::schedule(cpu);
    Some(MMSYSERR_NOERROR)
}

/// midiStreamStop(handle): its buffers are done, and it goes back to its
/// start, paused.
pub fn midi_stream_stop(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = find_stream(cpu, args[0]) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    stop(cpu, i);
    notes_off(cpu);
    Some(MMSYSERR_NOERROR)
}

/// Return the stream `i`'s buffers, done, and set it back to its start.
fn stop(cpu: &mut Cpu, i: usize) {
    let queue = std::mem::take(&mut stream(cpu, i).queue);
    for buffer in queue {
        buffer_done(cpu, i, buffer.header);
    }
    let s = stream(cpu, i);
    let (tempo, timediv) = (s.tempo, s.timediv);
    *s = Stream {
        tempo,
        timediv,
        ..Stream::default()
    };
    super::schedule(cpu);
}

/// The buffer of `hdr` is done.
fn buffer_done(cpu: &mut Cpu, i: usize, hdr: u32) {
    if cpu.bus.guest_probe(hdr, HDR_OLD_SIZE as usize, true) {
        let flags = cpu.bus.guest_read_32(hdr + HDR_FLAGS);
        cpu.bus.guest_write_32(
            hdr + HDR_FLAGS,
            (flags & !MHDR_INQUEUE) | MHDR_DONE | MHDR_ISSTRM,
        );
    }
    notify(cpu, i, MOM_DONE, hdr);
}

/// midiStreamPosition(handle, mmtime, size): where the stream is, in
/// milliseconds, ticks or a song position; other types give
/// milliseconds.
pub fn midi_stream_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, at, size] = args else {
        unreachable!()
    };
    let Some(i) = find_stream(cpu, handle) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    if at == 0 || size < 12 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(at, 12, true) {
        return None;
    }
    let now = cpu.bus.clock.now_micros();
    let s = stream(cpu, i);
    let played = s.played(now);
    let tick = s.tick_at(played).max(s.tick);
    let div = (s.timediv & 0x7FFF).max(1) as u64;
    let smpte = s.timediv & 0x8000 != 0;
    let kind = cpu.bus.guest_read_32(at);
    let (kind, value) = match kind {
        TIME_TICKS => (TIME_TICKS, tick as u32),
        // Sixteenth notes since the start.
        TIME_MIDI if !smpte => (TIME_MIDI, (tick * 4 / div) as u32),
        _ => (TIME_MS, (played / 1000) as u32),
    };
    cpu.bus.guest_write_32(at, kind);
    cpu.bus.guest_write_32(at + 4, value);
    Some(MMSYSERR_NOERROR)
}

/// midiStreamProperty(handle, data, property): get or set the tempo or
/// the time division.
pub fn midi_stream_property(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, at, property] = args else {
        unreachable!()
    };
    let Some(i) = find_stream(cpu, handle) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    let set = property & MIDIPROP_SET != 0;
    if set == (property & MIDIPROP_GET != 0) {
        return Some(MMSYSERR_INVALPARAM);
    }
    let which = property & !(MIDIPROP_SET | MIDIPROP_GET);
    if which != MIDIPROP_TEMPO && which != MIDIPROP_TIMEDIV {
        return Some(MMSYSERR_INVALPARAM);
    }
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(at, 8, true) {
        return None;
    }
    if cpu.bus.guest_read_32(at) < 8 {
        return Some(MMSYSERR_INVALPARAM);
    }
    let now = cpu.bus.clock.now_micros();
    if set {
        let value = cpu.bus.guest_read_32(at + 4);
        let s = stream(cpu, i);
        let played = s.played(now);
        s.seg_tick = s.tick_at(played).max(s.tick);
        s.seg_us = played;
        if which == MIDIPROP_TEMPO {
            s.tempo = value;
        } else {
            s.timediv = value;
        }
        super::schedule(cpu);
    } else {
        let s = stream(cpu, i);
        let value = if which == MIDIPROP_TEMPO {
            s.tempo
        } else {
            s.timediv
        };
        cpu.bus.guest_write_32(at + 4, value);
    }
    Some(MMSYSERR_NOERROR)
}

/// Play the events of the streams of the process `pid` that are due at
/// `now` (on its multimedia thread).
pub fn poll(cpu: &mut Cpu, pid: u32, now: u64) {
    let mut i = 0;
    while i < midi(cpu).outs.len() {
        if midi(cpu).outs[i].pid == pid && midi(cpu).outs[i].stream.is_some() {
            play_due(cpu, i, now);
        }
        i += 1;
    }
}

/// Play what the stream `i` has due at `now`.
fn play_due(cpu: &mut Cpu, i: usize, now: u64) {
    while let Some(due) = stream(cpu, i).next_due().filter(|&d| d <= now) {
        let s = stream(cpu, i);
        let buffer = &mut s.queue[0];
        let Some(e) = buffer.events.get(buffer.at as usize).cloned() else {
            let header = s.queue.remove(0).header;
            buffer_done(cpu, i, header);
            continue;
        };
        buffer.at += 1;
        let header = buffer.header;
        s.tick += e.delta as u64;
        let tick = s.tick;
        // The event's time, by what was played.
        let played = s.elapsed + due.saturating_sub(s.started);
        if cpu.bus.guest_probe(header + HDR_OFFSET, 4, true) {
            cpu.bus.guest_write_32(header + HDR_OFFSET, e.offset);
        }
        let kind = (e.event >> 24) as u8 & !0x40;
        match kind {
            MEVT_SHORTMSG => short_message(cpu, e.event & 0xFF_FFFF),
            MEVT_TEMPO => {
                let s = stream(cpu, i);
                s.seg_tick = tick;
                s.seg_us = played;
                s.tempo = e.event & 0xFF_FFFF;
            }
            MEVT_LONGMSG => {
                send(cpu, &e.data);
                midi(cpu).status = 0;
            }
            _ => {}
        }
        if e.event & MEVT_F_CALLBACK != 0 {
            notify(cpu, i, MOM_POSITIONCB, header);
        }
    }
}

/// When a stream of the process `pid` (or any process's, for None) next
/// has an event due.
pub fn next_due(cpu: &Cpu, pid: Option<u32>) -> Option<u64> {
    cpu.bus
        .win32
        .winmm
        .midi
        .outs
        .iter()
        .filter(|o| pid.is_none_or(|p| p == o.pid))
        .filter_map(|o| o.stream.as_ref()?.next_due())
        .min()
}

/// A process ended: its devices close, their notes end.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let had = midi(cpu).outs.iter().any(|o| o.pid == pid);
    let gone: Vec<MidiOut> = {
        let m = midi(cpu);
        let (gone, kept) = m.outs.drain(..).partition(|o| o.pid == pid);
        m.outs = kept;
        gone
    };
    for o in gone {
        o.callback.release(cpu);
    }
    if had {
        notes_off(cpu);
    }
}

crate::state_fields!(Event {
    offset,
    delta,
    event,
    data
});
crate::state_fields!(Buffer { header, events, at });
crate::state_fields!(Stream {
    playing,
    tempo,
    timediv,
    queue,
    elapsed,
    started,
    tick,
    seg_tick,
    seg_us
});
crate::state_fields!(MidiOut {
    handle,
    device,
    pid,
    callback,
    instance,
    stream
});
crate::state_fields!(Midi {
    outs,
    volume,
    mapper_volume,
    status
});
