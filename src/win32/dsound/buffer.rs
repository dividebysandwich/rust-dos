//! IDirectSoundBuffer, and the buffers' IDirectSoundNotify and
//! IKsPropertySet; and how the device mixes the buffers that play.
//!
//! A buffer's play cursor is where its anchor (`Buffer::tick` and
//! `Buffer::pos`) and its rate put it at the emulated time
//! (`device::position`), counted on past its end as it loops: the
//! notifications whose positions it passed since it was last seen
//! (`Buffer::seen`) fire when it is next seen (`settle`), and a buffer
//! that doesn't loop stops there when it played to its end.

use super::device::{self, Format, Reader};
use super::sound::DSSCL_WRITEPRIMARY;
use super::*;
use crate::timer::PIT_HZ;

/// DSBCAPS_ flags.
pub const DSBCAPS_PRIMARYBUFFER: u32 = 0x1;
pub const DSBCAPS_LOCHARDWARE: u32 = 0x4;
pub const DSBCAPS_LOCSOFTWARE: u32 = 0x8;
pub const DSBCAPS_CTRL3D: u32 = 0x10;
pub const DSBCAPS_CTRLFREQUENCY: u32 = 0x20;
pub const DSBCAPS_CTRLPAN: u32 = 0x40;
pub const DSBCAPS_CTRLVOLUME: u32 = 0x80;
pub const DSBCAPS_STICKYFOCUS: u32 = 0x4000;
pub const DSBCAPS_GLOBALFOCUS: u32 = 0x8000;
pub const DSBCAPS_MUTE3DATMAXDISTANCE: u32 = 0x2_0000;
pub const DSBCAPS_LOCDEFER: u32 = 0x4_0000;

/// GetStatus's bits.
const DSBSTATUS_PLAYING: u32 = 0x1;
const DSBSTATUS_LOOPING: u32 = 0x4;

/// Lock's flags, and Play's.
const DSBLOCK_FROMWRITECURSOR: u32 = 0x1;
const DSBLOCK_ENTIREBUFFER: u32 = 0x2;
const DSBPLAY_LOOPING: u32 = 0x1;

/// The offset of a notification at Stop.
pub const DSBPN_OFFSETSTOP: u32 = 0xFFFF_FFFF;

/// A secondary buffer's rates, and its sizes.
pub const FREQUENCY_MIN: u32 = 100;
pub const FREQUENCY_MAX: u32 = 100_000;
pub const SIZE_MIN: u32 = 4;
pub const SIZE_MAX: u32 = 0x0FFF_FFFF;

/// The primary buffer's bytes, whatever its format.
const PRIMARY_SIZE: u32 = 0x8000;

/// How far ahead of the play cursor the write cursor is: what the mixer
/// has taken already, about 10 ms.
const LEAD_DIVISOR: u32 = 100;

/// IKsPropertySet's answer for every property: there are none.
const E_PROP_ID_UNSUPPORTED: u32 = 0x8007_0490;

fn bus_format(ds: &State, b: &Buffer) -> Format {
    if b.primary {
        ds.primary_format()
    } else {
        b.format
    }
}

impl Buffer {
    /// Its frames: its bytes, by its format's.
    fn frames(&self, format: &Format) -> u32 {
        (self.size / format.block()).max(1)
    }
}

/// Memory for `size` bytes of samples, in the running process: its id in
/// `State::samples` and its address.
fn new_samples(cpu: &mut Cpu, size: u32) -> Option<(u32, u32)> {
    let addr = crate::win32::kernel32::alloc_pages(cpu, size)?;
    let pid = crate::win32::user32::current_pid(cpu);
    let ds = state(cpu);
    let id = ds.next_id();
    ds.samples.push(Samples {
        id,
        pid,
        addr,
        refs: 1,
    });
    Some((id, addr))
}

/// A new buffer of the DirectSound object `sound` with its IDirectSoundBuffer:
/// that interface's address.
fn add(cpu: &mut Cpu, mut buffer: Buffer) -> Result<u32, u32> {
    let id = state(cpu).next_id();
    let Some(iface) = new_interface(cpu, BUFFER + IFACE_BUFFER, id) else {
        release_samples(cpu, buffer.samples);
        return Err(com::E_OUTOFMEMORY);
    };
    buffer.id = id;
    buffer.pid = crate::win32::user32::current_pid(cpu);
    buffer.ifaces[IFACE_BUFFER] = iface;
    buffer.refs[IFACE_BUFFER] = 1;
    buffer.params = ds3d::Params::default();
    state(cpu).buffers.push(buffer);
    Ok(iface)
}

/// The primary buffer of the DirectSound object `sound`: the one it has,
/// with a reference more, or a new one.
pub fn primary(cpu: &mut Cpu, sound: u32, flags: u32) -> Result<u32, u32> {
    let ds = state(cpu);
    let s = ds.sound(sound).ok_or(DSERR_INVALIDPARAM)?;
    if let Some(i) = ds.buffer(ds.sounds[s].primary) {
        let b = &mut ds.buffers[i];
        b.refs[IFACE_BUFFER] += 1;
        return Ok(b.ifaces[IFACE_BUFFER]);
    }
    let (samples, addr) = new_samples(cpu, PRIMARY_SIZE).ok_or(com::E_OUTOFMEMORY)?;
    let format = state(cpu).primary_format();
    let iface = add(
        cpu,
        Buffer {
            sound,
            primary: true,
            flags: flags & !DSBCAPS_LOCDEFER | DSBCAPS_LOCHARDWARE,
            size: PRIMARY_SIZE,
            samples,
            addr,
            frequency: format.rate,
            ..Buffer::default()
        },
    )?;
    let ds = state(cpu);
    let id = ds.last_id;
    if let Some(s) = ds.sound(sound) {
        ds.sounds[s].primary = id;
    }
    Ok(iface)
}

/// A new secondary buffer of `bytes` (rounded up to whole frames) of
/// `format`.
pub fn secondary(
    cpu: &mut Cpu,
    sound: u32,
    flags: u32,
    format: Format,
    bytes: u32,
) -> Result<u32, u32> {
    let size = bytes.next_multiple_of(format.block());
    let (samples, addr) = new_samples(cpu, size).ok_or(com::E_OUTOFMEMORY)?;
    add(
        cpu,
        Buffer {
            sound,
            flags: flags & !(DSBCAPS_LOCDEFER | DSBCAPS_LOCHARDWARE) | DSBCAPS_LOCSOFTWARE,
            format,
            size,
            samples,
            addr,
            frequency: format.rate,
            ..Buffer::default()
        },
    )
}

/// A buffer with the samples of the buffer `original` (an interface of
/// it), for DuplicateSoundBuffer.
pub fn duplicate(cpu: &mut Cpu, sound: u32, original: u32) -> Result<u32, u32> {
    let (i, _) = buffer_of(cpu, original).ok_or(DSERR_INVALIDPARAM)?;
    let ds = state(cpu);
    let b = &ds.buffers[i];
    if b.primary {
        return Err(DSERR_INVALIDCALL);
    }
    let copy = Buffer {
        sound,
        flags: b.flags,
        format: b.format,
        size: b.size,
        samples: b.samples,
        addr: b.addr,
        frequency: b.frequency,
        volume: b.volume,
        pan: b.pan,
        ..Buffer::default()
    };
    let samples = b.samples;
    if let Some(s) = ds.samples.iter_mut().find(|s| s.id == samples) {
        s.refs += 1;
    }
    add(cpu, copy)
}

/// One buffer fewer uses the samples `id`: they go when none does.
fn release_samples(cpu: &mut Cpu, id: u32) {
    let ds = state(cpu);
    let Some(i) = ds.samples.iter().position(|s| s.id == id) else {
        return;
    };
    ds.samples[i].refs -= 1;
    if ds.samples[i].refs == 0 {
        let addr = ds.samples.remove(i).addr;
        crate::win32::kernel32::free_pages(cpu, addr);
    }
}

/// The buffer `id` goes: it stops, its interfaces' blocks and its
/// samples are given back, and its events.
pub fn destroy(cpu: &mut Cpu, id: u32) {
    device::catch_up(cpu);
    let ds = state(cpu);
    let Some(i) = ds.buffer(id) else {
        return;
    };
    let b = ds.buffers.remove(i);
    for s in ds.sounds.iter_mut().filter(|s| s.primary == id) {
        s.primary = 0;
    }
    for iface in b.ifaces.into_iter().filter(|&a| a != 0) {
        com::free_interface(cpu, iface);
    }
    for (_, event) in &b.notes {
        crate::win32::object::objects(cpu).release(*event);
    }
    release_samples(cpu, b.samples);
}

/// The buffer `this` is an interface of: its index and which interface.
fn buffer_of(cpu: &mut Cpu, this: u32) -> Option<(usize, usize)> {
    let (vtable, id) = interface(cpu, this)?;
    if !(BUFFER..BUFFER + IFACES).contains(&vtable) {
        return None;
    }
    Some((state(cpu).buffer(id)?, vtable - BUFFER))
}

/// The buffer `this` is an interface of, by its index.
pub fn index_of(cpu: &mut Cpu, this: u32) -> Option<usize> {
    buffer_of(cpu, this).map(|(i, _)| i)
}

/// The interface `kind` of the buffer at `i`, with a reference more:
/// made the first time it is asked for.
fn give(cpu: &mut Cpu, i: usize, kind: usize) -> Option<u32> {
    let b = &state(cpu).buffers[i];
    let iface = match b.ifaces[kind] {
        0 => {
            let id = b.id;
            let iface = new_interface(cpu, BUFFER + kind, id)?;
            state(cpu).buffers[i].ifaces[kind] = iface;
            iface
        }
        iface => iface,
    };
    state(cpu).buffers[i].refs[kind] += 1;
    Some(iface)
}

/// QueryInterface on a buffer's interfaces.
pub fn query(cpu: &mut Cpu, id: u32, iid: &Guid, out: u32) -> u32 {
    let ds = state(cpu);
    let Some(i) = ds.buffer(id) else {
        return DSERR_INVALIDPARAM;
    };
    let b = &ds.buffers[i];
    let three_d = b.flags & DSBCAPS_CTRL3D != 0;
    let kind = if *iid == com::IID_IUNKNOWN || *iid == IID_IDIRECTSOUNDBUFFER {
        IFACE_BUFFER
    } else if *iid == IID_IDIRECTSOUND3DBUFFER && three_d && !b.primary {
        IFACE_3D
    } else if *iid == IID_IDIRECTSOUND3DLISTENER && three_d && b.primary {
        IFACE_LISTENER
    } else if *iid == IID_IDIRECTSOUNDNOTIFY && !b.primary {
        IFACE_NOTIFY
    } else if *iid == IID_IKSPROPERTYSET {
        IFACE_PROPERTIES
    } else {
        return com::E_NOINTERFACE;
    };
    let Some(iface) = give(cpu, i, kind) else {
        return com::E_OUTOFMEMORY;
    };
    com::write_out(cpu, out, iface);
    DS_OK
}

/// Release on one of a buffer's interfaces: the buffer goes once none of
/// them has a reference left.
pub fn release(cpu: &mut Cpu, id: u32, kind: usize) -> u32 {
    let ds = state(cpu);
    let Some(i) = ds.buffer(id) else {
        return 0;
    };
    let b = &mut ds.buffers[i];
    b.refs[kind] = b.refs[kind].saturating_sub(1);
    let left = b.refs[kind];
    if b.refs.iter().all(|&r| r == 0) {
        destroy(cpu, id);
    }
    left
}

/// IDirectSoundBuffer::GetCaps(this, caps).
pub fn get_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = index_of(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    let caps = args[1];
    if caps == 0 || !cpu.bus.guest_probe(caps, 20, true) || cpu.bus.guest_read_32(caps) != 20 {
        return Some(DSERR_INVALIDPARAM);
    }
    let b = &state(cpu).buffers[i];
    let (flags, size) = (b.flags, b.size);
    for (n, value) in [20, flags, size, 0, 0].into_iter().enumerate() {
        cpu.bus.guest_write_32(caps + 4 * n as u32, value);
    }
    Some(DS_OK)
}

/// The play and write cursors of the buffer at `i` now, in bytes.
fn cursors(cpu: &mut Cpu, i: usize) -> (u32, u32) {
    let now = cpu.bus.clock.now_ticks();
    let ds = state(cpu);
    let b = &ds.buffers[i];
    let format = bus_format(ds, b);
    let frames = b.frames(&format) as u64;
    let pos = if b.playing {
        device::position(b.tick, b.pos, b.frequency, now, PIT_HZ)
    } else {
        b.pos
    };
    let play = ((pos >> 32) % frames) as u32 * format.block();
    if !b.playing {
        return (play, play);
    }
    let lead = (b.frequency / LEAD_DIVISOR + 1) * format.block();
    (play, (play + lead) % b.size)
}

/// IDirectSoundBuffer::GetCurrentPosition(this, play, write).
pub fn get_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, play, write] = args[..] else {
        return Some(DSERR_INVALIDPARAM);
    };
    let Some(i) = index_of(cpu, this) else {
        return Some(DSERR_INVALIDPARAM);
    };
    if play == 0 && write == 0 {
        return Some(DSERR_INVALIDPARAM);
    }
    let id = state(cpu).buffers[i].id;
    settle_now(cpu, id);
    let Some(i) = state(cpu).buffer(id) else {
        return Some(DSERR_INVALIDPARAM);
    };
    let (p, w) = cursors(cpu, i);
    if play != 0 && !com::write_out(cpu, play, p) || write != 0 && !com::write_out(cpu, write, w) {
        return Some(DSERR_INVALIDPARAM);
    }
    Some(DS_OK)
}

/// IDirectSoundBuffer::GetFormat(this, format, size, written).
pub fn get_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at, size, written] = args[..] else {
        return Some(DSERR_INVALIDPARAM);
    };
    let Some(i) = index_of(cpu, this) else {
        return Some(DSERR_INVALIDPARAM);
    };
    if at == 0 && written == 0 {
        return Some(DSERR_INVALIDPARAM);
    }
    let ds = state(cpu);
    let bytes = bus_format(ds, &ds.buffers[i]).to_bytes();
    if at != 0 {
        let n = (size as usize).min(bytes.len());
        if n < 16 || !cpu.bus.guest_probe(at, n, true) {
            return Some(DSERR_INVALIDPARAM);
        }
        cpu.bus.guest_write_bytes(at, &bytes[..n]);
    }
    if written != 0 && !com::write_out(cpu, written, device::WAVEFORMATEX_SIZE) {
        return Some(DSERR_INVALIDPARAM);
    }
    Some(DS_OK)
}

/// The buffer at `i` has the control `flag`: Ok, or DSERR_CONTROLUNAVAIL.
fn control(cpu: &mut Cpu, i: usize, flag: u32) -> Result<(), u32> {
    if state(cpu).buffers[i].flags & flag == 0 {
        return Err(DSERR_CONTROLUNAVAIL);
    }
    Ok(())
}

/// A Get method's buffer, for its control `flag`, and its out pointer
/// checked; or the HRESULT to fail with.
fn getter(cpu: &mut Cpu, args: &[u32], flag: u32) -> Result<usize, u32> {
    let i = index_of(cpu, args[0]).ok_or(DSERR_INVALIDPARAM)?;
    if args[1] == 0 || !cpu.bus.guest_probe(args[1], 4, true) {
        return Err(DSERR_INVALIDPARAM);
    }
    control(cpu, i, flag)?;
    Ok(i)
}

fn answer(cpu: &mut Cpu, out: u32, result: Result<u32, u32>) -> Option<u32> {
    Some(match result {
        Ok(value) => {
            com::write_out(cpu, out, value);
            DS_OK
        }
        Err(e) => e,
    })
}

/// IDirectSoundBuffer::GetVolume(this, out): the primary buffer's is the
/// device's.
pub fn get_volume(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let result = getter(cpu, args, DSBCAPS_CTRLVOLUME).map(|i| {
        let ds = state(cpu);
        let b = &ds.buffers[i];
        (if b.primary { ds.volume } else { b.volume }) as u32
    });
    answer(cpu, args[1], result)
}

/// IDirectSoundBuffer::GetPan(this, out).
pub fn get_pan(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let result = getter(cpu, args, DSBCAPS_CTRLPAN).map(|i| {
        let ds = state(cpu);
        let b = &ds.buffers[i];
        (if b.primary { ds.pan } else { b.pan }) as u32
    });
    answer(cpu, args[1], result)
}

/// IDirectSoundBuffer::GetFrequency(this, out): the primary buffer has
/// none of its own.
pub fn get_frequency(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let result = getter(cpu, args, DSBCAPS_CTRLFREQUENCY).and_then(|i| {
        let b = &state(cpu).buffers[i];
        if b.primary {
            Err(DSERR_CONTROLUNAVAIL)
        } else {
            Ok(b.frequency)
        }
    });
    answer(cpu, args[1], result)
}

/// IDirectSoundBuffer::GetStatus(this, out).
pub fn get_status(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = index_of(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    if args[1] == 0 || !cpu.bus.guest_probe(args[1], 4, true) {
        return Some(DSERR_INVALIDPARAM);
    }
    let id = state(cpu).buffers[i].id;
    settle_now(cpu, id);
    let status = state(cpu).buffer(id).map_or(0, |i| {
        let b = &state(cpu).buffers[i];
        match (b.playing, b.looping) {
            (false, _) => 0,
            (true, false) => DSBSTATUS_PLAYING,
            (true, true) => DSBSTATUS_PLAYING | DSBSTATUS_LOOPING,
        }
    });
    com::write_out(cpu, args[1], status);
    Some(DS_OK)
}

/// IDirectSoundBuffer::Initialize(this, sound, desc): CreateSoundBuffer's
/// buffers are.
pub fn initialize(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DSERR_ALREADYINITIALIZED)
}

/// IDirectSoundBuffer::Restore(this): the buffers' memory isn't lost.
pub fn restore(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DS_OK)
}

/// The cooperative level of the DirectSound object of the buffer at `i`.
fn level(cpu: &mut Cpu, i: usize) -> u32 {
    let ds = state(cpu);
    let sound = ds.buffers[i].sound;
    ds.sound(sound).map_or(0, |s| ds.sounds[s].level)
}

/// IDirectSoundBuffer::Lock(this, offset, bytes, ptr1, len1, ptr2, len2,
/// flags): where the program writes the bytes, in two parts when they
/// wrap past the buffer's end.
pub fn lock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, offset, bytes, ptr1, len1, ptr2, len2, flags] = args[..] else {
        return Some(DSERR_INVALIDPARAM);
    };
    let Some(i) = index_of(cpu, this) else {
        return Some(DSERR_INVALIDPARAM);
    };
    if ptr1 == 0 || len1 == 0 {
        return Some(DSERR_INVALIDPARAM);
    }
    if state(cpu).buffers[i].primary && level(cpu, i) != DSSCL_WRITEPRIMARY {
        return Some(DSERR_PRIOLEVELNEEDED);
    }
    let (size, addr) = {
        let b = &state(cpu).buffers[i];
        (b.size, b.addr)
    };
    let offset = if flags & DSBLOCK_FROMWRITECURSOR != 0 {
        cursors(cpu, i).1
    } else {
        offset
    };
    let bytes = if flags & DSBLOCK_ENTIREBUFFER != 0 {
        size
    } else {
        bytes
    };
    if offset >= size || bytes > size || bytes == 0 {
        return Some(DSERR_INVALIDPARAM);
    }
    device::catch_up(cpu);
    let first = bytes.min(size - offset);
    let second = if ptr2 != 0 { bytes - first } else { 0 };
    let written = com::write_out(cpu, ptr1, addr + offset)
        && com::write_out(cpu, len1, first)
        && (ptr2 == 0 || com::write_out(cpu, ptr2, if second > 0 { addr } else { 0 }))
        && (len2 == 0 || com::write_out(cpu, len2, second));
    Some(if written { DS_OK } else { DSERR_INVALIDPARAM })
}

/// IDirectSoundBuffer::Unlock(this, ptr1, len1, ptr2, len2): the parts
/// must be the buffer's.
pub fn unlock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, ptr1, len1, ptr2, len2] = args[..] else {
        return Some(DSERR_INVALIDPARAM);
    };
    let Some(i) = index_of(cpu, this) else {
        return Some(DSERR_INVALIDPARAM);
    };
    let b = &state(cpu).buffers[i];
    let (start, end) = (b.addr as u64, b.addr as u64 + b.size as u64);
    let inside = |at: u32, len: u32| at as u64 >= start && at as u64 + len as u64 <= end;
    if ptr1 == 0 || !inside(ptr1, len1) || ptr2 != 0 && !inside(ptr2, len2) {
        return Some(DSERR_INVALIDPARAM);
    }
    device::catch_up(cpu);
    Some(DS_OK)
}

/// IDirectSoundBuffer::Play(this, reserved, priority, flags).
pub fn play(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = index_of(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    let looping = args[3] & DSBPLAY_LOOPING != 0;
    // The primary buffer plays only in a loop.
    if state(cpu).buffers[i].primary && !looping {
        return Some(DSERR_INVALIDPARAM);
    }
    device::catch_up(cpu);
    let id = state(cpu).buffers[i].id;
    settle_now(cpu, id);
    let now = cpu.bus.clock.now_ticks();
    let ds = state(cpu);
    let Some(i) = ds.buffer(id) else {
        return Some(DSERR_INVALIDPARAM);
    };
    let format = bus_format(ds, &ds.buffers[i]);
    let b = &mut ds.buffers[i];
    let frames = b.frames(&format) as u64;
    if !b.playing {
        b.pos = ((b.pos >> 32) % frames) << 32;
        b.seen = b.pos;
        b.tick = now;
        b.playing = true;
    } else if b.looping {
        // Anchored where it is in this time round the loop, so a buffer
        // that stops looping plays on to its end from there.
        let pos = device::position(b.tick, b.pos, b.frequency, now, PIT_HZ);
        b.pos = pos % (frames << 32);
        b.seen = b.pos;
        b.tick = now;
    }
    b.looping = looping;
    super::reschedule(cpu);
    Some(DS_OK)
}

/// IDirectSoundBuffer::Stop(this): it stays where it got to, and its
/// DSBPN_OFFSETSTOP notifications fire.
pub fn stop(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = index_of(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    device::catch_up(cpu);
    let id = state(cpu).buffers[i].id;
    settle_now(cpu, id);
    let now = cpu.bus.clock.now_ticks();
    let ds = state(cpu);
    let Some(i) = ds.buffer(id) else {
        return Some(DSERR_INVALIDPARAM);
    };
    let format = bus_format(ds, &ds.buffers[i]);
    let b = &mut ds.buffers[i];
    if b.playing {
        let pos = device::position(b.tick, b.pos, b.frequency, now, PIT_HZ);
        b.pos = ((pos >> 32) % b.frames(&format) as u64) << 32;
        b.seen = b.pos;
        b.tick = now;
        b.playing = false;
        stopped(cpu, i);
    }
    super::reschedule(cpu);
    Some(DS_OK)
}

/// The buffer at `i` stopped: its DSBPN_OFFSETSTOP events are set.
fn stopped(cpu: &mut Cpu, i: usize) {
    let events: Vec<u32> = state(cpu).buffers[i]
        .notes
        .iter()
        .filter(|&&(offset, _)| offset == DSBPN_OFFSETSTOP)
        .map(|&(_, e)| e)
        .collect();
    for e in events {
        crate::win32::winmm::callback::set_event(cpu, e);
    }
}

/// IDirectSoundBuffer::SetCurrentPosition(this, offset): at the start of
/// the frame it is in.
pub fn set_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = index_of(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    let ds = state(cpu);
    let b = &ds.buffers[i];
    if b.primary {
        return Some(DSERR_INVALIDCALL);
    }
    if args[1] >= b.size {
        return Some(DSERR_INVALIDPARAM);
    }
    let frame = args[1] / b.format.block();
    device::catch_up(cpu);
    let id = state(cpu).buffers[i].id;
    settle_now(cpu, id);
    let now = cpu.bus.clock.now_ticks();
    if let Some(i) = state(cpu).buffer(id) {
        let b = &mut state(cpu).buffers[i];
        b.pos = (frame as u64) << 32;
        b.seen = b.pos;
        b.tick = now;
    }
    super::reschedule(cpu);
    Some(DS_OK)
}

/// IDirectSoundBuffer::SetFormat(this, format): the primary buffer's,
/// which is the device's, at DSSCL_PRIORITY or above.
pub fn set_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = index_of(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    if !state(cpu).buffers[i].primary {
        return Some(DSERR_INVALIDCALL);
    }
    let Some((format, _)) = Format::read(cpu, args[1]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    if level(cpu, i) < sound::DSSCL_PRIORITY {
        return Some(DSERR_PRIOLEVELNEEDED);
    }
    if let Err(e) = sound::check_format(&format) {
        return Some(e);
    }
    device::catch_up(cpu);
    let now = cpu.bus.clock.now_ticks();
    let ds = state(cpu);
    ds.format = format;
    for b in ds.buffers.iter_mut().filter(|b| b.primary) {
        b.frequency = format.rate;
        b.pos = 0;
        b.seen = 0;
        b.tick = now;
    }
    Some(DS_OK)
}

/// A Set method's buffer, for its control `flag`, with `value` in
/// `range`; or the HRESULT to fail with.
fn setter(
    cpu: &mut Cpu,
    this: u32,
    flag: u32,
    value: i32,
    range: std::ops::RangeInclusive<i32>,
) -> Result<usize, u32> {
    let i = index_of(cpu, this).ok_or(DSERR_INVALIDPARAM)?;
    control(cpu, i, flag)?;
    if !range.contains(&value) {
        return Err(DSERR_INVALIDPARAM);
    }
    device::catch_up(cpu);
    Ok(i)
}

/// IDirectSoundBuffer::SetVolume(this, volume): in hundredths of a
/// decibel, -10000 to 0. The primary buffer's is the device's.
pub fn set_volume(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let value = args[1] as i32;
    match setter(cpu, args[0], DSBCAPS_CTRLVOLUME, value, -10000..=0) {
        Ok(i) => {
            let ds = state(cpu);
            if ds.buffers[i].primary {
                ds.volume = value;
            } else {
                ds.buffers[i].volume = value;
            }
            Some(DS_OK)
        }
        Err(e) => Some(e),
    }
}

/// IDirectSoundBuffer::SetPan(this, pan): -10000 (left) to 10000.
pub fn set_pan(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let value = args[1] as i32;
    match setter(cpu, args[0], DSBCAPS_CTRLPAN, value, -10000..=10000) {
        Ok(i) => {
            let ds = state(cpu);
            if ds.buffers[i].primary {
                ds.pan = value;
            } else {
                ds.buffers[i].pan = value;
            }
            Some(DS_OK)
        }
        Err(e) => Some(e),
    }
}

/// IDirectSoundBuffer::SetFrequency(this, rate): 0 for its format's.
pub fn set_frequency(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = index_of(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    if state(cpu).buffers[i].primary {
        return Some(DSERR_CONTROLUNAVAIL);
    }
    let rate = match args[1] {
        0 => state(cpu).buffers[i].format.rate,
        r => r,
    };
    let range = FREQUENCY_MIN as i32..=FREQUENCY_MAX as i32;
    if let Err(e) = setter(cpu, args[0], DSBCAPS_CTRLFREQUENCY, rate as i32, range) {
        return Some(e);
    }
    let id = state(cpu).buffers[i].id;
    settle_now(cpu, id);
    let now = cpu.bus.clock.now_ticks();
    if let Some(i) = state(cpu).buffer(id) {
        let b = &mut state(cpu).buffers[i];
        if b.playing {
            b.pos = device::position(b.tick, b.pos, b.frequency, now, PIT_HZ);
            b.tick = now;
        }
        b.frequency = rate;
    }
    super::reschedule(cpu);
    Some(DS_OK)
}

/// IDirectSoundNotify::SetNotificationPositions(this, count, positions):
/// DSBPOSITIONNOTIFY's offset and event, for a buffer that doesn't play.
/// They take the place of those it had.
pub fn set_notifications(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, count, at] = args[..] else {
        return Some(DSERR_INVALIDPARAM);
    };
    let Some(i) = index_of(cpu, this) else {
        return Some(DSERR_INVALIDPARAM);
    };
    if count > 0 && (at == 0 || !cpu.bus.guest_probe(at, 8 * count as usize, false)) {
        return Some(DSERR_INVALIDPARAM);
    }
    if state(cpu).buffers[i].playing {
        return Some(DSERR_INVALIDCALL);
    }
    let size = state(cpu).buffers[i].size;
    let mut notes = Vec::new();
    for n in 0..count {
        let offset = cpu.bus.guest_read_32(at + 8 * n);
        let event = cpu.bus.guest_read_32(at + 8 * n + 4);
        if offset >= size && offset != DSBPN_OFFSETSTOP {
            return Some(DSERR_INVALIDPARAM);
        }
        let id = crate::win32::object::resolve(cpu, event).filter(|&id| {
            matches!(
                crate::win32::object::objects(cpu).get(id).map(|o| &o.kind),
                Some(crate::win32::object::Kind::Event { .. })
            )
        });
        let Some(id) = id else {
            return Some(DSERR_INVALIDPARAM);
        };
        notes.push((offset, id));
    }
    let objects = crate::win32::object::objects(cpu);
    for &(_, id) in &notes {
        objects.hold(id);
    }
    let old = std::mem::replace(&mut state(cpu).buffers[i].notes, notes);
    for (_, id) in old {
        crate::win32::object::objects(cpu).release(id);
    }
    Some(DS_OK)
}

/// IKsPropertySet::Get(this, set, id, instance, length, data, size,
/// returned): the device has no properties.
pub fn property_get(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(E_PROP_ID_UNSUPPORTED)
}

/// IKsPropertySet::Set(this, set, id, instance, length, data, size).
pub fn property_set(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(E_PROP_ID_UNSUPPORTED)
}

/// IKsPropertySet::QuerySupport(this, set, id, support).
pub fn property_query(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(E_PROP_ID_UNSUPPORTED)
}

/// The first frame at or after `from` (whole frames) that is the frame
/// `frame` of a loop of `frames`.
fn next_frame(from: u64, frame: u64, frames: u64) -> u64 {
    from + (frame + frames - from % frames) % frames
}

/// The buffer `id`, seen now by a program's call: its notifications that
/// came due fire, and the threads waiting for them may go on.
fn settle_now(cpu: &mut Cpu, id: u32) {
    if settle(cpu, id) {
        crate::win32::thread::wake(cpu);
    }
}

/// The buffer `id` is seen now: the events of the notifications whose
/// positions it passed are set, and if it played to its end without a
/// loop it stops there, at its start. Whether it set events.
pub fn settle(cpu: &mut Cpu, id: u32) -> bool {
    let now = cpu.bus.clock.now_ticks();
    let ds = state(cpu);
    let Some(i) = ds.buffer(id) else {
        return false;
    };
    let format = bus_format(ds, &ds.buffers[i]);
    let b = &mut ds.buffers[i];
    if !b.playing {
        return false;
    }
    let frames = b.frames(&format) as u64;
    let end = frames << 32;
    let pos = device::position(b.tick, b.pos, b.frequency, now, PIT_HZ);
    let ended = !b.looping && pos >= end;
    let until = if ended { end } else { pos };
    let from = (b.seen >> 32) + 1;
    let mut events = Vec::new();
    for &(offset, event) in &b.notes {
        if offset == DSBPN_OFFSETSTOP {
            continue;
        }
        let at = next_frame(from, (offset / format.block()) as u64, frames);
        if at << 32 <= until && (b.looping || at < frames) {
            events.push(event);
        }
    }
    b.seen = until;
    if ended {
        b.playing = false;
        b.pos = 0;
        b.seen = 0;
        b.tick = now;
    } else if b.seen >= end && b.pos >= end {
        // Loops played: counted off, so the counts stay small.
        let whole = (b.seen.min(b.pos) >> 32) / frames * frames;
        b.seen -= whole << 32;
        b.pos -= whole << 32;
    }
    let fired = !events.is_empty() || ended;
    for e in events {
        crate::win32::winmm::callback::set_event(cpu, e);
    }
    if ended {
        stopped(cpu, i);
    }
    fired
}

/// When the buffer next needs to be seen: its next notification's
/// position, or its end.
pub fn next_event(b: &Buffer) -> Option<u64> {
    if !b.playing || b.frequency == 0 {
        return None;
    }
    // A primary buffer's format isn't its own, but its notifications
    // aren't either (it has no IDirectSoundNotify).
    let block = b.format.block();
    let frames = (b.size / block).max(1) as u64;
    let from = (b.seen >> 32) + 1;
    let notes = b
        .notes
        .iter()
        .filter(|&&(offset, _)| offset != DSBPN_OFFSETSTOP)
        .map(|&(offset, _)| next_frame(from, (offset / block) as u64, frames))
        .filter(|&at| b.looping || at < frames)
        .filter_map(|at| device::reaches(b.tick, b.pos, b.frequency, at << 32));
    let end = (!b.looping)
        .then(|| device::reaches(b.tick, b.pos, b.frequency, frames << 32))
        .flatten();
    notes.chain(end).min()
}

/// Whether a buffer of the DirectSound object `sound` with `flags` is
/// heard: Windows 95 mutes a program's buffers while another program's
/// window is in the foreground, unless they have global focus, or sticky
/// focus and the other program doesn't use DirectSound.
fn audible(bus: &crate::bus::Bus, sound: &Sound, flags: u32) -> bool {
    if flags & DSBCAPS_GLOBALFOCUS != 0 || sound.hwnd == 0 {
        return true;
    }
    let user = &bus.win32.user;
    let foreground = user.foreground_window();
    let Some(front) = user.window(foreground).map(|w| w.pid) else {
        return true;
    };
    if front == sound.pid {
        return true;
    }
    flags & DSBCAPS_STICKYFOCUS != 0 && !sound::has_sound(&bus.win32.dsound, front)
}

/// A buffer as the mixer plays it.
struct Voice {
    pid: u32,
    addr: u32,
    format: Format,
    frames: u32,
    looping: bool,
    tick: u64,
    pos: u64,
    rate: u32,
    gains: (f32, f32),
}

/// The buffers that play and are heard, as voices.
fn voices(bus: &crate::bus::Bus) -> Vec<Voice> {
    let ds = &bus.win32.dsound;
    let mut voices = Vec::new();
    for sound in &ds.sounds {
        let write_primary = sound.level == DSSCL_WRITEPRIMARY;
        for b in ds
            .buffers
            .iter()
            .filter(|b| b.sound == sound.id && b.playing)
        {
            if b.primary != write_primary || !audible(bus, sound, b.flags) {
                continue;
            }
            let format = bus_format(ds, b);
            let (vl, vr) = if b.primary {
                (1.0, 1.0)
            } else {
                device::gains(b.volume, b.pan)
            };
            let (sl, sr) = if b.flags & DSBCAPS_CTRL3D != 0 && !b.primary {
                ds3d::gains(&b.params, &sound.listener, b.flags)
            } else {
                (1.0, 1.0)
            };
            voices.push(Voice {
                pid: b.pid,
                addr: b.addr,
                format,
                frames: b.frames(&format),
                looping: b.looping,
                tick: b.tick,
                pos: b.pos,
                rate: b.frequency,
                gains: (vl * sl, vr * sr),
            });
        }
    }
    voices
}

/// The buffers that play, mixed at the primary buffer's format (its rate,
/// its bits' steps, mono or stereo) for the host's frames `first` to
/// `first + frames`, through the primary buffer's volume and pan.
pub fn render(bus: &crate::bus::Bus, first: u64, frames: usize) -> Option<Vec<(f32, f32)>> {
    let ds = &bus.win32.dsound;
    let voices = voices(bus);
    if voices.is_empty() || frames == 0 {
        return None;
    }
    let primary = ds.primary_format();
    let rate = primary.rate as u64;
    let k0 = first * rate / device::RATE;
    let k1 = (first + frames as u64 - 1) * rate / device::RATE + 1;
    let mut mix = vec![(0.0f32, 0.0f32); (k1 - k0 + 1) as usize];
    for v in &voices {
        let mut reader = Reader::new(bus, v.pid);
        for (n, out) in mix.iter_mut().enumerate() {
            let pos = device::position(v.tick, v.pos, v.rate, k0 + n as u64, rate);
            if !v.looping && pos >= (v.frames as u64) << 32 {
                break;
            }
            let (l, r) =
                device::interpolate(&mut reader, v.addr, &v.format, v.frames, v.looping, pos);
            out.0 += l * v.gains.0;
            out.1 += r * v.gains.1;
        }
    }
    for frame in mix.iter_mut() {
        let mut l = frame.0.clamp(-32768.0, 32767.0);
        let mut r = frame.1.clamp(-32768.0, 32767.0);
        if primary.channels == 1 {
            l = (l + r) / 2.0;
            r = l;
        }
        if primary.bits == 8 {
            l = (l / 256.0).floor() * 256.0;
            r = (r / 256.0).floor() * 256.0;
        }
        *frame = (l, r);
    }
    let (gl, gr) = device::gains(ds.volume, ds.pan);
    let out = (0..frames as u64)
        .map(|n| {
            let x = (first + n) * rate;
            let k = (x / device::RATE - k0) as usize;
            let frac = (x % device::RATE) as f32 / device::RATE as f32;
            let (a, b) = (mix[k], mix[(k + 1).min(mix.len() - 1)]);
            (
                (a.0 + (b.0 - a.0) * frac) * gl,
                (a.1 + (b.1 - a.1) * frac) * gr,
            )
        })
        .collect();
    Some(out)
}
