//! WinMM's wave audio: waveOut, which plays the blocks of samples the
//! program writes one after another on the sound device (dsound's
//! device.rs mixes them), waveIn as on a machine without an input
//! device, and PlaySound.
//!
//! An open device (`Out`) keeps the blocks written to it as segments of
//! one stream of frames (`Segment`), a block between WHDR_BEGINLOOP and
//! WHDR_ENDLOOP once for each time it plays, and where the stream is at
//! as a function of the emulated time: from the frame it was at at its
//! anchor (`Out::tick`, `Out::anchor`) on at its rate, up to the end of
//! what was written, or still while it is paused. A block is done when
//! the stream passed its last segment: `tick` (from thread::wake when
//! time passes) marks its header WHDR_DONE and tells the program with
//! WOM_DONE, by the callback waveOutOpen was given. The samples stay in
//! the program's memory, where the device reads them as it plays.
//!
//! PlaySound's sounds (`Sound`) are the data of WAV files, kept here as
//! they play. `play_wav` plays such data for the rest of WinMM (MCI's
//! waveaudio device).

use super::callback::{self, Callback};
use crate::cpu::Cpu;
use crate::timer::PIT_HZ;
use crate::win32::dsound::device::{self, Format, Reader, WAVE_FORMAT_PCM};

/// MMSYSERR_ and WAVERR_ results.
pub use super::{
    MMSYSERR_BADDEVICEID, MMSYSERR_BADERRNUM, MMSYSERR_INVALHANDLE, MMSYSERR_INVALPARAM,
    MMSYSERR_NOERROR, MMSYSERR_NOTSUPPORTED,
};
pub const WAVERR_BADFORMAT: u32 = 32;
pub const WAVERR_STILLPLAYING: u32 = 33;
pub const WAVERR_UNPREPARED: u32 = 34;

/// WAVEHDR's flags.
const WHDR_DONE: u32 = 0x1;
const WHDR_PREPARED: u32 = 0x2;
const WHDR_BEGINLOOP: u32 = 0x4;
const WHDR_ENDLOOP: u32 = 0x8;
const WHDR_INQUEUE: u32 = 0x10;
/// WAVEHDR's bytes, and where its fields are.
const WAVEHDR_SIZE: u32 = 32;
const HDR_DATA: u32 = 0;
const HDR_LENGTH: u32 = 4;
const HDR_FLAGS: u32 = 16;
const HDR_LOOPS: u32 = 20;

/// waveOutOpen's flags, and the device ID of the wave mapper.
const WAVE_FORMAT_QUERY: u32 = 0x1;
const WAVE_MAPPER: u32 = 0xFFFF_FFFF;

/// The messages a device sends its callback.
const WOM_OPEN: u32 = 0x3BB;
const WOM_CLOSE: u32 = 0x3BC;
const WOM_DONE: u32 = 0x3BD;

/// MMTIME's kinds.
const TIME_MS: u32 = 0x1;
const TIME_SAMPLES: u32 = 0x2;
const TIME_BYTES: u32 = 0x4;

/// The first handle of an open device; the next ones follow.
const HANDLE_BASE: u32 = 0x5A00;

/// The sound device's name, as waveOutGetDevCaps gives it, and the
/// mapper's.
const DEVICE_NAME: &str = "Built-in Windows sound";
const MAPPER_NAME: &str = "Microsoft Sound Mapper";

/// A part of an open device's stream: the frames of the block whose
/// header is at `header`, from the stream's frame `start`. `last` when
/// it is the block's last time to play (the others are earlier times of
/// a loop), `pass` which time it is.
#[derive(Clone, Debug, Default)]
pub struct Segment {
    pub header: u32,
    pub data: u32,
    pub frames: u64,
    pub start: u64,
    pub last: bool,
    pub pass: u32,
    /// The loop it is in, if it is (by its first block's header), or 0.
    pub group: u32,
}

/// An open wave output device.
#[derive(Clone, Debug, Default)]
pub struct Out {
    pub handle: u32,
    pub pid: u32,
    pub format: Format,
    pub callback: Callback,
    pub instance: u32,
    pub segments: Vec<Segment>,
    /// The frames written, as the stream counts them.
    pub end: u64,
    /// Where the stream was at the PIT tick `tick` (frames, 32.32), and
    /// whether it is paused there.
    pub tick: u64,
    pub anchor: u64,
    pub paused: bool,
    /// The segments of the loop being written: from the one at this place
    /// in `segments`, with the BEGINLOOP header's dwLoops.
    pub loop_start: Option<usize>,
    pub loops: u32,
}

impl Out {
    /// Where the stream is at the time `num / den` seconds (frames,
    /// 32.32).
    fn position(&self, num: u64, den: u64) -> u64 {
        if self.paused {
            return self.anchor;
        }
        device::position(self.tick, self.anchor, self.format.rate, num, den).min(self.end << 32)
    }

    fn now(&self, now: u64) -> u64 {
        self.position(now, PIT_HZ)
    }

    /// Anchor it at `now`, where it is.
    fn anchor_at(&mut self, now: u64) {
        self.anchor = self.now(now);
        self.tick = now;
    }
}

/// A sound PlaySound (or MCI) plays: its samples, from the PIT tick
/// `tick`, once or in a loop.
#[derive(Clone, Debug, Default)]
pub struct Sound {
    pub id: u32,
    pub pid: u32,
    pub format: Format,
    pub data: Vec<u8>,
    pub tick: u64,
    pub looping: bool,
    /// Whether PlaySound started it (one at a time), or MCI.
    pub play_sound: bool,
}

impl Sound {
    fn frames(&self) -> u64 {
        self.data.len() as u64 / self.format.block() as u64
    }

    /// When it ends, in PIT ticks: None if it loops.
    fn end(&self) -> Option<u64> {
        if self.looping {
            return None;
        }
        device::reaches(self.tick, 0, self.format.rate, self.frames() << 32)
    }
}

/// WinMM's wave audio: the open devices, the sounds playing, and the
/// device's volume (both channels, as waveOutSetVolume has it; None for
/// full).
#[derive(Clone, Debug, Default)]
pub struct Wave {
    pub outs: Vec<Out>,
    pub sounds: Vec<Sound>,
    pub volume: Option<u32>,
    pub last_handle: u32,
    pub last_sound: u32,
    /// When `tick` is next due, in PIT ticks.
    pub due: Option<u64>,
}

fn wave(cpu: &mut Cpu) -> &mut Wave {
    &mut cpu.bus.win32.winmm.wave
}

/// Whether anything plays: an open device with frames to go that isn't
/// paused, or a sound.
pub fn playing(w: &Wave) -> bool {
    !w.sounds.is_empty() || w.outs.iter().any(|o| !o.paused && !o.segments.is_empty())
}

/// A process ended: its devices close and its sounds stop.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let w = wave(cpu);
    let gone: Vec<Callback> = w
        .outs
        .iter()
        .filter(|o| o.pid == pid)
        .map(|o| o.callback.clone())
        .collect();
    w.outs.retain(|o| o.pid != pid);
    w.sounds.retain(|s| s.pid != pid);
    for c in gone {
        c.release(cpu);
    }
}

/// The open device `handle`, by its place.
fn out_of(cpu: &mut Cpu, handle: u32) -> Option<usize> {
    wave(cpu).outs.iter().position(|o| o.handle == handle)
}

/// Write the double word `value` at `lin` in the memory of the process
/// `pid`, whichever runs.
fn write_32(cpu: &mut Cpu, pid: u32, lin: u32, value: u32) {
    let bytes = value.to_le_bytes();
    for (n, byte) in bytes.into_iter().enumerate() {
        let at = lin.wrapping_add(n as u32);
        let phys = cpu
            .bus
            .win32
            .processes
            .iter()
            .find(|p| p.id == pid)
            .and_then(|p| p.memory.translate(&cpu.bus, at));
        if let Some(phys) = phys {
            cpu.bus.write_8(phys as usize, byte);
        }
    }
}

/// The double word at `lin` in the memory of the process `pid`.
fn read_32(cpu: &Cpu, pid: u32, lin: u32) -> u32 {
    let mut reader = Reader::new(&cpu.bus, pid);
    u32::from_le_bytes([0, 1, 2, 3].map(|n| reader.byte(lin.wrapping_add(n))))
}

/// Time passed (`super::tick`): the blocks played are done, and the
/// sounds played to their ends stop.
pub fn tick(cpu: &mut Cpu) {
    if !playing(wave(cpu)) && wave(cpu).due.is_none() {
        return;
    }
    device::catch_up(cpu);
    let now = cpu.bus.clock.now_ticks();
    let handles: Vec<u32> = wave(cpu).outs.iter().map(|o| o.handle).collect();
    for h in handles {
        finish_played(cpu, h, now);
    }
    wave(cpu)
        .sounds
        .retain(|s| s.end().is_none_or(|end| end > now));
    reschedule(cpu);
}

/// Work out when `tick` is next due, and have the clock stop then.
fn reschedule(cpu: &mut Cpu) {
    let now = cpu.bus.clock.now_ticks();
    let w = wave(cpu);
    let outs = w.outs.iter().filter(|o| !o.paused).filter_map(|o| {
        let s = o.segments.first()?;
        device::reaches(o.tick, o.anchor, o.format.rate, (s.start + s.frames) << 32)
    });
    let sounds = w.sounds.iter().filter_map(Sound::end);
    w.due = outs.chain(sounds).filter(|&t| t > now).min();
    cpu.bus.reschedule_timers();
}

/// When the wave audio is next to need `tick`, in PIT ticks.
pub fn next_event(w: &Wave, now: u64) -> Option<u64> {
    w.due.filter(|&t| t > now)
}

/// Mark the header at `header` done and tell the program.
fn done(cpu: &mut Cpu, i: usize, header: u32) {
    let (pid, handle, instance, cb) = {
        let o = &wave(cpu).outs[i];
        (o.pid, o.handle, o.instance, o.callback.clone())
    };
    let flags = read_32(cpu, pid, header + HDR_FLAGS);
    write_32(
        cpu,
        pid,
        header + HDR_FLAGS,
        flags & !WHDR_INQUEUE | WHDR_DONE,
    );
    callback::notify(cpu, &cb, handle, WOM_DONE, instance, header, 0);
}

/// The blocks of the device `handle` that played by `now` are done.
fn finish_played(cpu: &mut Cpu, handle: u32, now: u64) {
    let Some(i) = out_of(cpu, handle) else {
        return;
    };
    let pos = wave(cpu).outs[i].now(now) >> 32;
    loop {
        let o = &mut wave(cpu).outs[i];
        let Some(s) = o.segments.first() else {
            break;
        };
        if s.start + s.frames > pos {
            break;
        }
        let s = o.segments.remove(0);
        if let Some(start) = o.loop_start.as_mut() {
            *start = start.saturating_sub(1);
        }
        if s.last {
            done(cpu, i, s.header);
        }
    }
}

/// The device's samples for the host's frames from `first`, added to
/// `out`: the open devices' streams and the sounds, at the device's
/// volume.
pub fn render(bus: &crate::bus::Bus, first: u64, out: &mut [(f32, f32)]) {
    let w = &bus.win32.winmm.wave;
    let volume = w.volume.unwrap_or(0xFFFF_FFFF);
    let (gl, gr) = (
        (volume & 0xFFFF) as f32 / 65535.0,
        (volume >> 16) as f32 / 65535.0,
    );
    for o in w
        .outs
        .iter()
        .filter(|o| !o.segments.is_empty() && !o.paused)
    {
        let mut reader = Reader::new(bus, o.pid);
        let mut at = 0;
        for (n, frame) in out.iter_mut().enumerate() {
            let pos = o.position(first + n as u64, device::RATE);
            let whole = pos >> 32;
            while at < o.segments.len() && o.segments[at].start + o.segments[at].frames <= whole {
                at += 1;
            }
            let Some(s) = o.segments.get(at).filter(|s| s.start <= whole) else {
                continue;
            };
            let local = pos - (s.start << 32);
            let (l, r) = device::interpolate(
                &mut reader,
                s.data,
                &o.format,
                s.frames as u32,
                false,
                local,
            );
            frame.0 += l * gl;
            frame.1 += r * gr;
        }
    }
    for s in &w.sounds {
        let frames = s.frames();
        if frames == 0 {
            continue;
        }
        for (n, frame) in out.iter_mut().enumerate() {
            let pos = device::position(s.tick, 0, s.format.rate, first + n as u64, device::RATE);
            let mut index = pos >> 32;
            if s.looping {
                index %= frames;
            } else if index >= frames {
                break;
            }
            let (l, r) = sample(&s.data, &s.format, index as usize);
            frame.0 += l * gl;
            frame.1 += r * gr;
        }
    }
}

/// The frame `index` of the samples `data` in `format`, as `Reader::frame`
/// reads them from memory.
fn sample(data: &[u8], format: &Format, index: usize) -> (f32, f32) {
    let start = index * format.block() as usize;
    let get = |i: usize| -> f32 {
        if format.bits == 8 {
            (data.get(start + i).copied().unwrap_or(128) as f32 - 128.0) * 256.0
        } else {
            let lo = data.get(start + 2 * i).copied().unwrap_or(0);
            let hi = data.get(start + 2 * i + 1).copied().unwrap_or(0);
            i16::from_le_bytes([lo, hi]) as f32
        }
    };
    let left = get(0);
    if format.channels == 2 {
        (left, get(1))
    } else {
        (left, left)
    }
}

/// waveOutGetNumDevs(): the one device.
pub fn wave_out_get_num_devs(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

/// Whether `id` names the device (0, the mapper, or an open device's
/// handle).
fn is_device(cpu: &mut Cpu, id: u32) -> bool {
    id == 0 || id == WAVE_MAPPER || out_of(cpu, id).is_some()
}

/// WAVEOUTCAPSA's bytes.
const WAVEOUTCAPS_SIZE: usize = 52;
/// The formats the device plays: 11, 22 and 44 kHz, mono and stereo, 8 and
/// 16 bits.
const WAVE_FORMATS: u32 = 0xFFF;
/// WAVECAPS_VOLUME, WAVECAPS_LRVOLUME and WAVECAPS_SAMPLEACCURATE.
const WAVE_SUPPORT: u32 = 0x4 | 0x8 | 0x20;

/// waveOutGetDevCapsA(id, caps, size): as much as `size` holds.
pub fn wave_out_get_dev_caps_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [id, caps, size] = args[..] else {
        return Some(MMSYSERR_INVALPARAM);
    };
    if !is_device(cpu, id) {
        return Some(MMSYSERR_BADDEVICEID);
    }
    if caps == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    let mut bytes = [0u8; WAVEOUTCAPS_SIZE];
    let mapper = id == WAVE_MAPPER;
    bytes[0..2].copy_from_slice(&1u16.to_le_bytes()); // MM_MICROSOFT
    let product: u16 = if mapper { 2 } else { 0x68 };
    bytes[2..4].copy_from_slice(&product.to_le_bytes());
    bytes[4..8].copy_from_slice(&0x0400u32.to_le_bytes());
    let name = if mapper { MAPPER_NAME } else { DEVICE_NAME };
    bytes[8..8 + name.len()].copy_from_slice(name.as_bytes());
    bytes[40..44].copy_from_slice(&WAVE_FORMATS.to_le_bytes());
    bytes[44..46].copy_from_slice(&2u16.to_le_bytes());
    bytes[48..52].copy_from_slice(&WAVE_SUPPORT.to_le_bytes());
    let n = (size as usize).min(WAVEOUTCAPS_SIZE);
    if !cpu.bus.guest_probe(caps, n, true) {
        return Some(MMSYSERR_INVALPARAM);
    }
    cpu.bus.guest_write_bytes(caps, &bytes[..n]);
    Some(MMSYSERR_NOERROR)
}

/// waveOutGetDevCapsW and the other wide functions: Windows 95 has only
/// stubs of them.
pub fn not_supported(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_NOTSUPPORTED)
}

/// Whether the device plays `format`, given as its 16 bytes and
/// nBlockAlign.
fn check_format(format: &Format, align: u16) -> Result<(), u32> {
    if format.tag != WAVE_FORMAT_PCM
        || !format.playable()
        || align as u32 != format.block()
        || !(100..=100_000).contains(&format.rate)
    {
        return Err(WAVERR_BADFORMAT);
    }
    Ok(())
}

/// waveOutOpen(out, id, format, callback, instance, flags).
pub fn wave_out_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [out, id, at, cb, instance, flags] = args[..] else {
        return Some(MMSYSERR_INVALPARAM);
    };
    if id != 0 && id != WAVE_MAPPER {
        return Some(MMSYSERR_BADDEVICEID);
    }
    let Some((format, align)) = Format::read(cpu, at) else {
        return Some(MMSYSERR_INVALPARAM);
    };
    if let Err(e) = check_format(&format, align) {
        return Some(e);
    }
    if flags & WAVE_FORMAT_QUERY != 0 {
        return Some(MMSYSERR_NOERROR);
    }
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(MMSYSERR_INVALPARAM);
    }
    let callback = match Callback::from_open(cpu, cb, flags) {
        Ok(c) => c,
        Err(e) => return Some(e),
    };
    let now = cpu.bus.clock.now_ticks();
    let pid = crate::win32::user32::current_pid(cpu);
    let w = wave(cpu);
    w.last_handle += 1;
    let handle = HANDLE_BASE + w.last_handle;
    w.outs.push(Out {
        handle,
        pid,
        format,
        callback: callback.clone(),
        instance,
        tick: now,
        ..Out::default()
    });
    cpu.bus.guest_write_32(out, handle);
    callback::notify(cpu, &callback, handle, WOM_OPEN, instance, 0, 0);
    Some(MMSYSERR_NOERROR)
}

/// waveOutClose(handle): only once its blocks are done.
pub fn wave_out_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = out_of(cpu, args[0]) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    tick(cpu);
    if !wave(cpu).outs[i].segments.is_empty() {
        return Some(WAVERR_STILLPLAYING);
    }
    let o = wave(cpu).outs.remove(i);
    callback::notify(cpu, &o.callback, o.handle, WOM_CLOSE, o.instance, 0, 0);
    o.callback.release(cpu);
    Some(MMSYSERR_NOERROR)
}

/// The header at `header`, of `size` bytes, can be read and written: its
/// flags, or the MMSYSERR to fail with.
fn header_flags(cpu: &mut Cpu, header: u32, size: u32) -> Result<u32, u32> {
    if header == 0 || size < WAVEHDR_SIZE || !cpu.bus.guest_probe(header, 32, true) {
        return Err(MMSYSERR_INVALPARAM);
    }
    Ok(cpu.bus.guest_read_32(header + HDR_FLAGS))
}

/// waveOutPrepareHeader(handle, header, size).
pub fn wave_out_prepare_header(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if out_of(cpu, args[0]).is_none() {
        return Some(MMSYSERR_INVALHANDLE);
    }
    let flags = match header_flags(cpu, args[1], args[2]) {
        Ok(f) => f,
        Err(e) => return Some(e),
    };
    if flags & WHDR_INQUEUE != 0 {
        return Some(WAVERR_STILLPLAYING);
    }
    cpu.bus
        .guest_write_32(args[1] + HDR_FLAGS, flags | WHDR_PREPARED);
    Some(MMSYSERR_NOERROR)
}

/// waveOutUnprepareHeader(handle, header, size).
pub fn wave_out_unprepare_header(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if out_of(cpu, args[0]).is_none() {
        return Some(MMSYSERR_INVALHANDLE);
    }
    let flags = match header_flags(cpu, args[1], args[2]) {
        Ok(f) => f,
        Err(e) => return Some(e),
    };
    if flags & WHDR_INQUEUE != 0 {
        return Some(WAVERR_STILLPLAYING);
    }
    cpu.bus
        .guest_write_32(args[1] + HDR_FLAGS, flags & !WHDR_PREPARED);
    Some(MMSYSERR_NOERROR)
}

/// waveOutWrite(handle, header, size): the block plays after those
/// written before it; from now, if they are done.
pub fn wave_out_write(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [handle, header, size] = args[..] else {
        return Some(MMSYSERR_INVALPARAM);
    };
    let Some(i) = out_of(cpu, handle) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    let flags = match header_flags(cpu, header, size) {
        Ok(f) => f,
        Err(e) => return Some(e),
    };
    if flags & WHDR_PREPARED == 0 {
        return Some(WAVERR_UNPREPARED);
    }
    if flags & WHDR_INQUEUE != 0 {
        return Some(WAVERR_STILLPLAYING);
    }
    let data = cpu.bus.guest_read_32(header + HDR_DATA);
    let length = cpu.bus.guest_read_32(header + HDR_LENGTH);
    let loops = cpu.bus.guest_read_32(header + HDR_LOOPS);
    cpu.bus
        .guest_write_32(header + HDR_FLAGS, flags & !WHDR_DONE | WHDR_INQUEUE);
    device::catch_up(cpu);
    let now = cpu.bus.clock.now_ticks();
    let o = &mut wave(cpu).outs[i];
    o.anchor_at(now);
    let frames = (length / o.format.block()) as u64;
    if flags & WHDR_BEGINLOOP != 0 {
        o.loop_start = Some(o.segments.len());
        o.loops = loops;
    }
    let group = o.loop_start.map_or(0, |s| {
        o.segments
            .get(s)
            .map_or(header, |first: &Segment| first.header)
    });
    o.segments.push(Segment {
        header,
        data,
        frames,
        start: o.end,
        last: true,
        pass: 0,
        group,
    });
    o.end += frames;
    if flags & WHDR_ENDLOOP != 0
        && let Some(start) = o.loop_start.take()
    {
        // The loop's blocks again, for each time more it plays.
        let body: Vec<Segment> = o.segments[start..].to_vec();
        for pass in 1..o.loops.max(1) {
            for s in o.segments.iter_mut().skip(start) {
                if body.iter().any(|b| b.header == s.header) {
                    s.last = false;
                }
            }
            for b in &body {
                o.segments.push(Segment {
                    start: o.end,
                    last: true,
                    pass,
                    ..b.clone()
                });
                o.end += b.frames;
            }
        }
    }
    reschedule(cpu);
    Some(MMSYSERR_NOERROR)
}

/// waveOutReset(handle): playing stops, every block is done, and the
/// position is 0 again.
pub fn wave_out_reset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = out_of(cpu, args[0]) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    tick(cpu);
    let now = cpu.bus.clock.now_ticks();
    let o = &mut wave(cpu).outs[i];
    let segments = std::mem::take(&mut o.segments);
    o.end = 0;
    o.anchor = 0;
    o.tick = now;
    o.loop_start = None;
    let mut seen = Vec::new();
    for s in segments {
        if !seen.contains(&s.header) {
            seen.push(s.header);
            done(cpu, i, s.header);
        }
    }
    reschedule(cpu);
    Some(MMSYSERR_NOERROR)
}

/// waveOutPause(handle).
pub fn wave_out_pause(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = out_of(cpu, args[0]) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    device::catch_up(cpu);
    let now = cpu.bus.clock.now_ticks();
    let o = &mut wave(cpu).outs[i];
    if !o.paused {
        o.anchor_at(now);
        o.paused = true;
    }
    reschedule(cpu);
    Some(MMSYSERR_NOERROR)
}

/// waveOutRestart(handle).
pub fn wave_out_restart(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = out_of(cpu, args[0]) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    device::catch_up(cpu);
    let now = cpu.bus.clock.now_ticks();
    let o = &mut wave(cpu).outs[i];
    if o.paused {
        o.tick = now;
        o.paused = false;
    }
    reschedule(cpu);
    Some(MMSYSERR_NOERROR)
}

/// waveOutBreakLoop(handle): the loop playing ends after the time it is
/// at.
pub fn wave_out_break_loop(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = out_of(cpu, args[0]) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    tick(cpu);
    let now = cpu.bus.clock.now_ticks();
    let o = &mut wave(cpu).outs[i];
    let Some(current) = o.segments.first().cloned() else {
        return Some(MMSYSERR_NOERROR);
    };
    if current.group == 0 {
        return Some(MMSYSERR_NOERROR);
    }
    o.anchor_at(now);
    o.segments
        .retain(|s| s.group != current.group || s.pass <= current.pass);
    // The blocks' last times are now those of this pass, and what follows
    // the loop moves up.
    let mut start = o.segments.first().map_or(0, |s| s.start);
    let mut last: Vec<u32> = Vec::new();
    for s in o.segments.iter_mut().rev() {
        s.last = !last.contains(&s.header);
        last.push(s.header);
    }
    for s in o.segments.iter_mut() {
        s.start = start;
        start += s.frames;
    }
    o.end = start;
    reschedule(cpu);
    Some(MMSYSERR_NOERROR)
}

/// waveOutGetPosition(handle, mmtime, size): in bytes, samples or
/// milliseconds; other kinds are given in bytes.
pub fn wave_out_get_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [handle, at, size] = args[..] else {
        return Some(MMSYSERR_INVALPARAM);
    };
    let Some(i) = out_of(cpu, handle) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    if at == 0 || size < 8 || !cpu.bus.guest_probe(at, 8, true) {
        return Some(MMSYSERR_INVALPARAM);
    }
    tick(cpu);
    let now = cpu.bus.clock.now_ticks();
    let o = &wave(cpu).outs[i];
    let frames = o.now(now) >> 32;
    let format = o.format;
    let (kind, value) = match cpu.bus.guest_read_32(at) {
        TIME_SAMPLES => (TIME_SAMPLES, frames),
        TIME_MS => (TIME_MS, frames * 1000 / format.rate as u64),
        _ => (TIME_BYTES, frames * format.block() as u64),
    };
    cpu.bus.guest_write_32(at, kind);
    cpu.bus.guest_write_32(at + 4, value as u32);
    Some(MMSYSERR_NOERROR)
}

/// waveOutGetVolume(id or handle, out).
pub fn wave_out_get_volume(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !is_device(cpu, args[0]) {
        return Some(MMSYSERR_INVALHANDLE);
    }
    if args[1] == 0 || !cpu.bus.guest_probe(args[1], 4, true) {
        return Some(MMSYSERR_INVALPARAM);
    }
    let volume = wave(cpu).volume.unwrap_or(0xFFFF_FFFF);
    cpu.bus.guest_write_32(args[1], volume);
    Some(MMSYSERR_NOERROR)
}

/// waveOutSetVolume(id or handle, volume): the device's, left in the low
/// word.
pub fn wave_out_set_volume(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !is_device(cpu, args[0]) {
        return Some(MMSYSERR_INVALHANDLE);
    }
    device::catch_up(cpu);
    wave(cpu).volume = Some(args[1]);
    Some(MMSYSERR_NOERROR)
}

/// waveOutGetID(handle, out).
pub fn wave_out_get_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if out_of(cpu, args[0]).is_none() {
        return Some(MMSYSERR_INVALHANDLE);
    }
    if args[1] == 0 || !cpu.bus.guest_probe(args[1], 4, true) {
        return Some(MMSYSERR_INVALPARAM);
    }
    cpu.bus.guest_write_32(args[1], 0);
    Some(MMSYSERR_NOERROR)
}

/// waveOutMessage(id or handle, message, p1, p2): the driver has no
/// messages of its own for programs.
pub fn wave_out_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !is_device(cpu, args[0]) {
        return Some(MMSYSERR_INVALHANDLE);
    }
    Some(MMSYSERR_NOTSUPPORTED)
}

/// waveOutGetPitch, waveOutSetPitch, waveOutGetPlaybackRate and
/// waveOutSetPlaybackRate: the device has neither control.
pub fn wave_out_pitch(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if out_of(cpu, args[0]).is_none() {
        return Some(MMSYSERR_INVALHANDLE);
    }
    Some(MMSYSERR_NOTSUPPORTED)
}

/// The text of an MMSYSERR or WAVERR, as Windows words it.
fn error_text(error: u32) -> Option<&'static str> {
    Some(match error {
        0 => "The specified command was carried out.",
        1 => "Undefined external error.",
        2 => "A device ID has been used that is out of range for your system.",
        3 => "The driver was not enabled.",
        4 => "The specified device is already in use. Wait until it is free, and then try again.",
        5 => "The specified device handle is invalid.",
        6 => "There is no driver installed on your system.",
        7 => {
            "There is not enough memory available for this task. Quit one or more \
             applications to increase available memory, and then try again."
        }
        8 => {
            "This function is not supported. Use the Capabilities function to determine \
             which functions and messages the driver supports."
        }
        9 => "An error number was specified that is not defined in the system.",
        10 => "An invalid flag was passed to a system function.",
        11 => "An invalid parameter was passed to a system function.",
        12 => {
            "The specified device is in use by another program. Wait until it is free, and \
             then try again."
        }
        13 => "The specified alias was not found.",
        14 => "The registry database is corrupt.",
        15 => "The specified registry key was not found.",
        16 => "The registry could not be opened or could not be read.",
        17 => "The registry could not be written to.",
        18 => "The specified registry key could not be deleted.",
        19 => "The specified registry key value could not be found.",
        20 => "The driver did not generate a valid OPEN callback.",
        32 => {
            "The specified format is not supported or cannot be translated. Use the \
             Capabilities function to determine the supported formats."
        }
        33 => {
            "Cannot perform this operation while media data is still playing. Reset the \
             device, or wait until the data is finished playing."
        }
        34 => {
            "The wave header was not prepared. Use the Prepare function to prepare the \
             header, and then try again."
        }
        35 => {
            "Cannot open the device without using the WAVE_ALLOWSYNC flag. Use the flag, \
             and then try again."
        }
        _ => return None,
    })
}

/// waveOutGetErrorTextA(error, text, size) and waveInGetErrorTextA.
pub fn wave_get_error_text_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [error, at, size] = args[..] else {
        return Some(MMSYSERR_INVALPARAM);
    };
    let Some(text) = error_text(error) else {
        return Some(MMSYSERR_BADERRNUM);
    };
    if at == 0 || size == 0 {
        return Some(MMSYSERR_NOERROR);
    }
    let n = text.len().min(size as usize - 1);
    if !cpu.bus.guest_probe(at, n + 1, true) {
        return Some(MMSYSERR_INVALPARAM);
    }
    let mut bytes = text.as_bytes()[..n].to_vec();
    bytes.push(0);
    cpu.bus.guest_write_bytes(at, &bytes);
    Some(MMSYSERR_NOERROR)
}

/// waveInGetNumDevs(): there is no input device.
pub fn wave_in_get_num_devs(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// waveInGetDevCaps and waveInOpen: no device has the ID.
pub fn wave_in_bad_device(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_BADDEVICEID)
}

/// The other waveIn functions: no handle is a device's.
pub fn wave_in_bad_handle(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(MMSYSERR_INVALHANDLE)
}

/// PlaySound's flags.
const SND_ASYNC: u32 = 0x1;
const SND_NODEFAULT: u32 = 0x2;
const SND_MEMORY: u32 = 0x4;
const SND_LOOP: u32 = 0x8;
const SND_NOSTOP: u32 = 0x10;
const SND_PURGE: u32 = 0x40;
const SND_ALIAS: u32 = 0x1_0000;
const SND_FILENAME: u32 = 0x2_0000;
const SND_RESOURCE: u32 = 0x4_0004;

/// The system's sounds by their names, which Windows without a sound
/// scheme has no files for: they play nothing.
const SYSTEM_SOUNDS: &[&str] = &[
    ".Default",
    "SystemAsterisk",
    "SystemDefault",
    "SystemExclamation",
    "SystemExit",
    "SystemHand",
    "SystemQuestion",
    "SystemStart",
    "MailBeep",
    "Maximize",
    "Minimize",
    "MenuCommand",
    "MenuPopup",
    "Open",
    "Close",
    "RestoreDown",
    "RestoreUp",
    "AppGPFault",
];

/// The format and samples of a WAV file's data: its RIFF chunks "fmt "
/// and "data". None if it isn't one the device plays.
pub fn parse_wav(bytes: &[u8]) -> Option<(Format, Vec<u8>)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut at = 12;
    let mut format = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let body = bytes.get(at + 8..(at + 8).saturating_add(size).min(bytes.len()))?;
        if id == b"fmt " && body.len() >= 16 {
            let (f, align) = Format::from_bytes(body[..16].try_into().ok()?);
            check_format(&f, align).ok()?;
            format = Some(f);
        } else if id == b"data" {
            return Some((format?, body.to_vec()));
        }
        at += 8 + size + (size & 1);
    }
    None
}

/// Play the samples `data` in `format` on the device, from now, for the
/// running process: for MCI's waveaudio device. The sound's id, for
/// `stop_sound` and `sound_playing`; it ends by itself unless it loops.
#[allow(dead_code)] // MCI's waveaudio device is to use these.
pub fn play_wav(cpu: &mut Cpu, format: Format, data: Vec<u8>, looping: bool) -> u32 {
    start_sound(cpu, format, data, looping, false)
}

/// Stop the sound `id` that `play_wav` started.
#[allow(dead_code)]
pub fn stop_sound(cpu: &mut Cpu, id: u32) {
    device::catch_up(cpu);
    wave(cpu).sounds.retain(|s| s.id != id);
    reschedule(cpu);
}

/// Whether the sound `id` plays still.
#[allow(dead_code)]
pub fn sound_playing(cpu: &mut Cpu, id: u32) -> bool {
    tick(cpu);
    wave(cpu).sounds.iter().any(|s| s.id == id)
}

fn start_sound(cpu: &mut Cpu, format: Format, data: Vec<u8>, looping: bool, ps: bool) -> u32 {
    device::catch_up(cpu);
    let now = cpu.bus.clock.now_ticks();
    let pid = crate::win32::user32::current_pid(cpu);
    let w = wave(cpu);
    w.last_sound += 1;
    let id = w.last_sound;
    w.sounds.push(Sound {
        id,
        pid,
        format,
        data,
        tick: now,
        looping,
        play_sound: ps,
    });
    reschedule(cpu);
    id
}

/// Stop PlaySound's sound, if one plays.
fn stop_play_sound(cpu: &mut Cpu) {
    device::catch_up(cpu);
    wave(cpu).sounds.retain(|s| !s.play_sound);
    reschedule(cpu);
}

/// The data of a WAV file in memory at `at`: its RIFF chunk.
fn wav_in_memory(cpu: &mut Cpu, at: u32) -> Option<Vec<u8>> {
    if at == 0 || !cpu.bus.guest_probe(at, 8, false) {
        return None;
    }
    let size = cpu.bus.guest_read_32(at + 4).checked_add(8)?;
    if size > 0x0400_0000 || !cpu.bus.guest_probe(at, size as usize, false) {
        return None;
    }
    let mut bytes = vec![0; size as usize];
    cpu.bus.guest_read_bytes(at, &mut bytes);
    Some(bytes)
}

/// The data of the WAV file named at `name`.
fn wav_file(cpu: &mut Cpu, name: u32) -> Option<Vec<u8>> {
    let (at, size) = crate::win32::kernel32::read_whole_file(cpu, name)??;
    let mut bytes = vec![0; size as usize];
    cpu.bus.guest_read_bytes(at, &mut bytes);
    crate::win32::kernel32::free_pages(cpu, at);
    Some(bytes)
}

/// PlaySoundA(name, module, flags): a WAV file, one in memory, or a
/// system sound by its name; from now, in a loop, or until it is done
/// before PlaySound returns. A sound PlaySound plays stops the one it
/// played before; a NULL name or SND_PURGE only stops it.
pub fn play_sound_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [name, _module, flags] = args[..] else {
        return Some(0);
    };
    play(cpu, name, flags)
}

/// sndPlaySoundA(name, flags): PlaySound with a name that is a system
/// sound's or a file's.
pub fn snd_play_sound_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    play(cpu, args[0], args[1] & 0xFF)
}

fn play(cpu: &mut Cpu, name: u32, flags: u32) -> Option<u32> {
    const TRUE: u32 = 1;
    if name == 0 || flags & SND_PURGE != 0 {
        stop_play_sound(cpu);
        return Some(TRUE);
    }
    if flags & SND_NOSTOP != 0 && wave(cpu).sounds.iter().any(|s| s.play_sound) {
        return Some(0);
    }
    let data = if flags & SND_MEMORY != 0 && flags & SND_RESOURCE != SND_RESOURCE {
        wav_in_memory(cpu, name)
    } else if flags & SND_RESOURCE == SND_RESOURCE {
        // Sounds in a module's resources aren't looked for.
        None
    } else {
        let Some(text) = crate::win32::user32::string_at(cpu, name) else {
            return Some(0);
        };
        let system = SYSTEM_SOUNDS.iter().any(|s| s.eq_ignore_ascii_case(&text));
        if system && flags & SND_FILENAME == 0 {
            stop_play_sound(cpu);
            return Some(TRUE);
        }
        if flags & SND_ALIAS != 0 {
            None
        } else {
            wav_file(cpu, name)
        }
    };
    let Some((format, samples)) = data.as_deref().and_then(parse_wav) else {
        // No sound of that name: the default sound would play, and
        // without a sound scheme there is none.
        let _ = flags & SND_NODEFAULT;
        return Some(0);
    };
    stop_play_sound(cpu);
    let looping = flags & SND_LOOP != 0;
    let frames = samples.len() as u64 / format.block() as u64;
    start_sound(cpu, format, samples, looping, true);
    if flags & SND_ASYNC != 0 || looping {
        return Some(TRUE);
    }
    // Synchronous: the thread waits until the sound is done.
    let micros = frames * 1_000_000 / format.rate as u64;
    let deadline = cpu.bus.clock.now_micros() + micros;
    cpu.set_eax(TRUE);
    crate::win32::thread::block(
        cpu,
        crate::win32::thread::Wait {
            deadline: Some(deadline),
            timeout_result: TRUE,
            ..Default::default()
        },
    );
    None
}

crate::state_fields!(Segment {
    header,
    data,
    frames,
    start,
    last,
    pass,
    group
});
crate::state_fields!(Out {
    handle,
    pid,
    format,
    callback,
    instance,
    segments,
    end,
    tick,
    anchor,
    paused,
    loop_start,
    loops
});
crate::state_fields!(Sound {
    id,
    pid,
    format,
    data,
    tick,
    looping,
    play_sound
});
crate::state_fields!(Wave {
    outs,
    sounds,
    volume,
    last_handle,
    last_sound,
    due
});

/// PlaySoundW and sndPlaySoundW: Windows 95 has them as stubs that fail.
pub fn play_sound_w(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}
