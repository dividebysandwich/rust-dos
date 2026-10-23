//! DMUSIC.DLL's objects: IDirectMusic, its ports, buffers and master
//! clock, and DLS collections with their instruments.
//!
//! DirectMusic lists two ports, the MIDI mapper and the Microsoft
//! Synthesizer, its default; both play on the MIDI synthesizer
//! (winmm/midi.rs). What a program packs in a buffer and plays on a port
//! is sent at the times of its events (`poll`).

use super::files::Inst;
use super::guids::*;
use super::{
    COLLECTION, E_INVALIDARG, E_POINTER, INSTRUMENT, Kind, MUSIC, PORT, S_FALSE, S_OK, add, alloc,
    arg64, dmus_e, give_new, read_u64, state, this_of, write_u64, write_wide,
};
use crate::cpu::Cpu;
use crate::win32::com::{self, CLASS_E_NOAGGREGATION, Guid};

pub const DMUS_E_INVALIDPATCH: u32 = dmus_e(0x114);
pub const DMUS_E_DSOUND_NOT_SET: u32 = dmus_e(0x139);
pub const DMUS_E_BUFFER_EMPTY: u32 = dmus_e(0x135);
pub const DMUS_E_BUFFER_FULL: u32 = dmus_e(0x136);
pub const DMUS_E_PORT_NOT_CAPTURE: u32 = dmus_e(0x137);
/// DMUS_S_PARAMS_CHANGED: a port was made with other parameters.
const DMUS_S_PARAMS_CHANGED: u32 = 0x0878_1102;

/// An IDirectMusic: whether it has DirectSound, its ports.
#[derive(Clone, Debug, Default)]
pub struct Music {
    pub dsound: bool,
}

impl Music {
    pub fn held(&self) -> Vec<u32> {
        Vec::new()
    }
}

/// A port: which of the two, its channel groups, whether active, and
/// the events played on it to send (reference time, bytes).
#[derive(Clone, Debug, Default)]
pub struct Port {
    pub synth: bool,
    pub groups: u32,
    pub active: bool,
    pub queue: Vec<(i64, Vec<u8>)>,
}

/// A buffer of events: its memory in the process, size, bytes used, the
/// time its events count from, its read place and its format.
#[derive(Clone, Debug, Default)]
pub struct Buf {
    pub at: u32,
    pub max: u32,
    pub used: u32,
    pub start: i64,
    pub read: u32,
    pub format: [u8; 16],
}

/// A DLS collection's instruments (patch, name).
#[derive(Clone, Debug, Default)]
pub struct Collection {
    pub instruments: Vec<(u32, String)>,
}

/// What the ports are: the MIDI mapper's and the Microsoft
/// Synthesizer's capabilities (DMUS_PORTCAPS).
struct PortInfo {
    guid: Guid,
    flags: u32,
    kind: u32,
    memory: u32,
    groups: u32,
    voices: u32,
    audio: u32,
    effects: u32,
    name: &'static str,
}

static PORTS: [PortInfo; 2] = [
    PortInfo {
        guid: GUID_MIDI_MAPPER_PORT,
        flags: 0x100,
        kind: 0,
        memory: 0,
        groups: 1,
        voices: 0,
        audio: 0,
        effects: 0,
        name: "Microsoft MIDI Mapper [Emulated]",
    },
    PortInfo {
        guid: CLSID_SYNTH,
        flags: 0x85,
        kind: 1,
        memory: 0x7FFF_FFFF,
        groups: 1000,
        voices: 1000,
        audio: 2,
        effects: 1,
        name: "Microsoft Synthesizer",
    },
];

/// DMUS_PORTCAPS's size, and DMUS_PC_OUTPUTCLASS.
const PORTCAPS: u32 = 308;
const OUTPUTCLASS: u32 = 1;
/// The latency of the ports, in 100-nanosecond units: 100 ms.
pub const LATENCY: i64 = 1_000_000;

/// The master clock's time: the emulated clock in 100-nanosecond units.
pub fn now_rt(cpu: &Cpu) -> i64 {
    (cpu.bus.clock.now_micros() as i64).saturating_mul(10)
}

/// CoCreateInstance of DirectMusic.
pub fn create_music(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = add(
        cpu,
        Kind::Music {
            m: Music::default(),
        },
    );
    give_new(cpu, id, iid, out)
}

/// CoCreateInstance of a DLS collection, empty.
pub fn create_collection(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = add(
        cpu,
        Kind::Collection {
            c: Collection::default(),
        },
    );
    give_new(cpu, id, iid, out)
}

fn music_mut(cpu: &mut Cpu, id: u32) -> Option<&mut Music> {
    match &mut state(cpu).get_mut(id)?.kind {
        Kind::Music { m } => Some(m),
        _ => None,
    }
}

fn port_mut(cpu: &mut Cpu, id: u32) -> Option<&mut Port> {
    match &mut state(cpu).get_mut(id)?.kind {
        Kind::Port { p } => Some(p),
        _ => None,
    }
}

fn buf(cpu: &mut Cpu, this: u32) -> Option<(u32, Buf)> {
    let id = this_of(cpu, this, super::BUFFER)?;
    match &state(cpu).get(id)?.kind {
        Kind::Buffer { b } => Some((id, b.clone())),
        _ => None,
    }
}

fn set_buf(cpu: &mut Cpu, id: u32, b: Buf) {
    if let Some(o) = state(cpu).get_mut(id)
        && let Kind::Buffer { b: old } = &mut o.kind
    {
        *old = b;
    }
}

/// Write a port's capabilities to `at`, whose dwSize the caller checked.
fn write_caps(cpu: &mut Cpu, at: u32, info: &PortInfo) {
    let size = cpu.bus.guest_read_32(at);
    cpu.bus.guest_fill(at + 4, PORTCAPS as usize - 4, 0);
    cpu.bus.guest_write_32(at, size);
    cpu.bus.guest_write_32(at + 4, info.flags);
    info.guid.write(cpu, at + 8);
    let words = [
        OUTPUTCLASS,
        info.kind,
        info.memory,
        info.groups,
        info.voices,
        info.audio,
        info.effects,
    ];
    for (i, w) in words.iter().enumerate() {
        cpu.bus.guest_write_32(at + 24 + 4 * i as u32, *w);
    }
    write_wide(cpu, at + 52, info.name, 128);
}

/// Whether the caps structure at `at` can take DMUS_PORTCAPS.
fn caps_ok(cpu: &mut Cpu, at: u32) -> bool {
    at != 0
        && cpu.bus.guest_probe(at, PORTCAPS as usize, true)
        && cpu.bus.guest_read_32(at) >= PORTCAPS
}

/// IDirectMusic::EnumPort(this, index, caps).
pub fn enum_port(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if this_of(cpu, args[0], MUSIC).is_none() {
        return Some(E_INVALIDARG);
    }
    if args[2] == 0 {
        return Some(E_POINTER);
    }
    if !caps_ok(cpu, args[2]) {
        return Some(E_INVALIDARG);
    }
    let Some(info) = PORTS.get(args[1] as usize) else {
        return Some(S_FALSE);
    };
    write_caps(cpu, args[2], info);
    Some(S_OK)
}

/// IDirectMusic::CreateMusicBuffer(this, desc, buffer, outer).
pub fn create_music_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, desc, out, outer] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if this_of(cpu, this, MUSIC).is_none() || outer != 0 {
        return Some(if outer != 0 {
            CLASS_E_NOAGGREGATION
        } else {
            E_INVALIDARG
        });
    }
    if desc == 0 || out == 0 {
        return Some(E_POINTER);
    }
    // DMUS_BUFFERDESC: dwSize, dwFlags, guidBufferFormat, cbBuffer.
    if cpu.bus.guest_read_32(desc) < 28 {
        return Some(E_INVALIDARG);
    }
    let mut format = [0; 16];
    cpu.bus.guest_read_bytes(desc + 8, &mut format);
    if format == [0; 16] {
        format = KSDATAFORMAT_SUBTYPE_DIRECTMUSIC.0;
    }
    let max = cpu.bus.guest_read_32(desc + 24).clamp(32, 0x10_0000);
    let Some(at) = alloc(cpu, max) else {
        return Some(com::E_OUTOFMEMORY);
    };
    let b = Buf {
        at,
        max,
        format,
        ..Buf::default()
    };
    let id = add(cpu, Kind::Buffer { b });
    Some(give_new(cpu, id, &IID_IDIRECTMUSIC_BUFFER, out))
}

/// The format of DirectMusic's buffers.
const KSDATAFORMAT_SUBTYPE_DIRECTMUSIC: Guid = Guid::new(
    0x1A82_F8BC,
    0x3F8B,
    0x11D2,
    [0xB7, 0x74, 0x00, 0x60, 0x08, 0x33, 0x16, 0xC1],
);

/// Make a port of the kind `synth`: its id.
pub fn new_port(cpu: &mut Cpu, synth: bool, groups: u32) -> u32 {
    // Its events are sent by the process's multimedia thread.
    crate::win32::winmm::callback::ensure_thread(cpu);
    add(
        cpu,
        Kind::Port {
            p: Port {
                synth,
                groups,
                active: true,
                queue: Vec::new(),
            },
        },
    )
}

/// IDirectMusic::CreatePort(this, clsid, params, port, outer).
pub fn create_port(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, clsid, params, out, outer] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(id) = this_of(cpu, this, MUSIC) else {
        return Some(E_INVALIDARG);
    };
    if outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    if clsid == 0 || params == 0 || out == 0 {
        return Some(E_POINTER);
    }
    com::write_out(cpu, out, 0);
    let Some(guid) = Guid::read(cpu, clsid) else {
        return Some(E_POINTER);
    };
    let synth = if guid == com::GUID_NULL || guid == CLSID_SYNTH {
        true
    } else if guid == GUID_MIDI_MAPPER_PORT {
        false
    } else {
        return Some(E_INVALIDARG);
    };
    if synth && !music_mut(cpu, id).is_some_and(|m| m.dsound) {
        return Some(DMUS_E_DSOUND_NOT_SET);
    }
    // DMUS_PORTPARAMS: dwSize, dwValidParams, dwVoices, dwChannelGroups,
    // dwAudioChannels, dwSampleRate, dwEffectFlags, fShare.
    let size = cpu.bus.guest_read_32(params);
    if size < 32 {
        return Some(E_INVALIDARG);
    }
    let valid = cpu.bus.guest_read_32(params + 4);
    let info = &PORTS[synth as usize];
    let mut groups = 1;
    let mut changed = false;
    // DMUSP_ flags: voices 1, channel groups 2, audio channels 4,
    // sample rate 8, effects 0x20, share 0x40.
    if valid & 2 != 0 {
        let want = cpu.bus.guest_read_32(params + 12);
        groups = want.clamp(1, info.groups);
        if groups != want {
            cpu.bus.guest_write_32(params + 12, groups);
            changed = true;
        }
    }
    if valid & 1 != 0 && !synth {
        changed = true;
    }
    let port = new_port(cpu, synth, groups);
    let hr = give_new(cpu, port, &IID_IDIRECTMUSIC_PORT, out);
    if hr == S_OK && changed {
        return Some(DMUS_S_PARAMS_CHANGED);
    }
    Some(hr)
}

/// DMUS_CLOCKINFO7's size: dwSize, ctType, guidClock, wszDescription.
const CLOCKINFO: u32 = 280;

/// IDirectMusic::EnumMasterClock(this, index, info): the system clock.
pub fn enum_master_clock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if this_of(cpu, args[0], MUSIC).is_none() {
        return Some(E_INVALIDARG);
    }
    let at = args[2];
    if at == 0 || !cpu.bus.guest_probe(at, CLOCKINFO as usize, true) {
        return Some(E_POINTER);
    }
    if cpu.bus.guest_read_32(at) < CLOCKINFO {
        return Some(E_INVALIDARG);
    }
    if args[1] != 0 {
        return Some(S_FALSE);
    }
    // DMUS_CLOCK_SYSTEM.
    cpu.bus.guest_write_32(at + 4, 0);
    GUID_SYSTEM_CLOCK.write(cpu, at + 8);
    write_wide(cpu, at + 24, "System Clock", 128);
    Some(S_OK)
}

/// A new IReferenceClock to `out`: S_OK, or E_POINTER.
fn give_clock(cpu: &mut Cpu, out: u32) -> u32 {
    if out == 0 {
        return E_POINTER;
    }
    let id = add(cpu, Kind::Clock {});
    give_new(cpu, id, &IID_IREFERENCE_CLOCK, out)
}

/// IDirectMusic::GetMasterClock(this, guid, clock).
pub fn get_master_clock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if this_of(cpu, args[0], MUSIC).is_none() {
        return Some(E_INVALIDARG);
    }
    if args[1] != 0 {
        GUID_SYSTEM_CLOCK.write(cpu, args[1]);
    }
    if args[2] == 0 {
        return Some(S_OK);
    }
    Some(give_clock(cpu, args[2]))
}

/// IDirectMusic::SetMasterClock(this, guid): only the system clock.
pub fn set_master_clock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if this_of(cpu, args[0], MUSIC).is_none() {
        return Some(E_INVALIDARG);
    }
    Some(match Guid::read(cpu, args[1]) {
        Some(g) if g == GUID_SYSTEM_CLOCK => S_OK,
        Some(_) => E_INVALIDARG,
        None => E_POINTER,
    })
}

/// IDirectMusic::GetDefaultPort(this, guid): the synthesizer.
pub fn get_default_port(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if this_of(cpu, args[0], MUSIC).is_none() {
        return Some(E_INVALIDARG);
    }
    Some(if CLSID_SYNTH.write(cpu, args[1]) {
        S_OK
    } else {
        E_POINTER
    })
}

/// IDirectMusic::SetDirectSound(this, dsound, window): DirectMusic has
/// it, or makes its own for NULL. The program's DirectSound isn't held.
pub fn set_direct_sound(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_of(cpu, args[0], MUSIC) else {
        return Some(E_INVALIDARG);
    };
    if let Some(m) = music_mut(cpu, id) {
        m.dsound = true;
    }
    Some(S_OK)
}

/// IReferenceClock::GetTime(this, time).
pub fn clock_get_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[1] == 0 || !cpu.bus.guest_probe(args[1], 8, true) {
        return Some(E_POINTER);
    }
    let now = now_rt(cpu);
    write_u64(cpu, args[1], now as u64);
    Some(S_OK)
}

/// The port `this` is.
fn port_id(cpu: &mut Cpu, this: u32) -> Option<u32> {
    this_of(cpu, this, PORT)
}

/// The events of the buffer `b`: (time, channel group, bytes).
fn events(cpu: &mut Cpu, b: &Buf) -> Vec<(i64, u32, Vec<u8>)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + HEADER <= b.used {
        let cb = cpu.bus.guest_read_32(b.at + at);
        let group = cpu.bus.guest_read_32(b.at + at + 4);
        let delta = read_u64(cpu, b.at + at + 8) as i64;
        if cb > b.used - at - HEADER {
            break;
        }
        let mut bytes = vec![0; cb as usize];
        cpu.bus.guest_read_bytes(b.at + at + HEADER, &mut bytes);
        out.push((b.start.saturating_add(delta), group, bytes));
        at += event_size(cb);
    }
    out
}

/// DMUS_EVENTHEADER's size, and an event's with its bytes, which start
/// the next on a multiple of 8.
const HEADER: u32 = 20;
fn event_size(cb: u32) -> u32 {
    (HEADER + cb).next_multiple_of(8)
}

/// IDirectMusicPort::PlayBuffer(this, buffer): its events are sent at
/// their times.
pub fn play_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(port) = port_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some((_, b)) = buf(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    let list = events(cpu, &b);
    if let Some(p) = port_mut(cpu, port) {
        for (rt, _, bytes) in list {
            let i = p.queue.partition_point(|e| e.0 <= rt);
            p.queue.insert(i, (rt, bytes));
        }
    }
    crate::win32::winmm::schedule(cpu);
    Some(S_OK)
}

/// IDirectMusicPort::Read(this, buffer): the ports play only.
pub fn port_read(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if port_id(cpu, args[0]).is_none() {
        return Some(E_INVALIDARG);
    }
    Some(DMUS_E_PORT_NOT_CAPTURE)
}

/// A downloaded instrument to `out`.
pub fn give_downloaded(cpu: &mut Cpu, out: u32) -> u32 {
    if out == 0 {
        return E_POINTER;
    }
    let id = add(cpu, Kind::Downloaded {});
    give_new(cpu, id, &IID_IDIRECTMUSIC_DOWNLOADED_INSTRUMENT, out)
}

/// IDirectMusicPort::DownloadInstrument(this, instrument, downloaded,
/// ranges, count): the synthesizer plays the patch with its own
/// instrument.
pub fn download_instrument(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if port_id(cpu, args[0]).is_none() {
        return Some(E_INVALIDARG);
    }
    if this_of(cpu, args[1], INSTRUMENT).is_none() || args[2] == 0 {
        return Some(E_POINTER);
    }
    Some(give_downloaded(cpu, args[2]))
}

/// IDirectMusicPort::GetLatencyClock(this, clock).
pub fn get_latency_clock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if port_id(cpu, args[0]).is_none() {
        return Some(E_INVALIDARG);
    }
    Some(give_clock(cpu, args[1]))
}

/// IDirectMusicPort::GetRunningStats(this, stats): none kept.
pub fn get_running_stats(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if port_id(cpu, args[0]).is_none() {
        return Some(E_INVALIDARG);
    }
    let at = args[1];
    // DMUS_SYNTHSTATS: dwSize and seven fields.
    if at == 0 || !cpu.bus.guest_probe(at, 32, true) {
        return Some(E_POINTER);
    }
    if cpu.bus.guest_read_32(at) < 32 {
        return Some(E_INVALIDARG);
    }
    cpu.bus.guest_fill(at + 4, 28, 0);
    Some(S_OK)
}

/// IDirectMusicPort::GetCaps(this, caps).
pub fn port_get_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = port_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    if !caps_ok(cpu, args[1]) {
        return Some(E_INVALIDARG);
    }
    let synth = port_mut(cpu, id).is_some_and(|p| p.synth);
    write_caps(cpu, args[1], &PORTS[synth as usize]);
    Some(S_OK)
}

/// IDirectMusicPort::SetNumChannelGroups(this, groups).
pub fn set_channel_groups(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = port_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let p = port_mut(cpu, id)?;
    let max = PORTS[p.synth as usize].groups;
    if args[1] == 0 || args[1] > max {
        return Some(E_INVALIDARG);
    }
    p.groups = args[1];
    Some(S_OK)
}

/// IDirectMusicPort::GetNumChannelGroups(this, groups).
pub fn get_channel_groups(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = port_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let groups = port_mut(cpu, id)?.groups;
    Some(if com::write_out(cpu, args[1], groups) {
        S_OK
    } else {
        E_POINTER
    })
}

/// IDirectMusicPort::Activate(this, active): S_FALSE when it already is
/// or isn't.
pub fn port_activate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = port_id(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let p = port_mut(cpu, id)?;
    let want = args[1] != 0;
    if p.active == want {
        return Some(S_FALSE);
    }
    p.active = want;
    Some(S_OK)
}

/// IDirectMusicPort::GetChannelPriority(this, group, channel, priority):
/// the standard priority (DAUD_STANDARD_VOICE_PRIORITY) and the
/// channel's place, as the synthesizer starts with.
pub fn get_channel_priority(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if port_id(cpu, args[0]).is_none() {
        return Some(E_INVALIDARG);
    }
    if args[2] > 15 {
        return Some(E_INVALIDARG);
    }
    let priority = if args[2] == 9 {
        0x4000_000F
    } else {
        0x4000_000E - args[2].min(9)
    };
    Some(if com::write_out(cpu, args[3], priority) {
        S_OK
    } else {
        E_POINTER
    })
}

/// IDirectMusicPort::GetFormat(this, format, size, buffer size): the
/// synthesizer's 22,050 Hz 16-bit stereo.
pub fn port_get_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, format, size, buffer] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if port_id(cpu, this).is_none() {
        return Some(E_INVALIDARG);
    }
    if size == 0 {
        return Some(E_POINTER);
    }
    // WAVEFORMATEX.
    const WFX: u32 = 18;
    if format != 0 {
        if cpu.bus.guest_read_32(size) < WFX || !cpu.bus.guest_probe(format, WFX as usize, true) {
            return Some(E_INVALIDARG);
        }
        let mut w = Vec::with_capacity(18);
        w.extend(1u16.to_le_bytes());
        w.extend(2u16.to_le_bytes());
        w.extend(22050u32.to_le_bytes());
        w.extend((22050u32 * 4).to_le_bytes());
        w.extend(4u16.to_le_bytes());
        w.extend(16u16.to_le_bytes());
        w.extend(0u16.to_le_bytes());
        cpu.bus.guest_write_bytes(format, &w);
    }
    com::write_out(cpu, size, WFX);
    if buffer != 0 {
        com::write_out(cpu, buffer, 22050 * 4 / 4);
    }
    Some(S_OK)
}

/// IDirectMusicBuffer::Flush(this).
pub fn buffer_flush(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((id, mut b)) = buf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    b.used = 0;
    b.read = 0;
    set_buf(cpu, id, b);
    Some(S_OK)
}

/// IDirectMusicBuffer::TotalTime(this, time): from the first event to
/// the last.
pub fn buffer_total_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((_, b)) = buf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    let list = events(cpu, &b);
    let total = list.iter().map(|e| e.0 - b.start).max().unwrap_or(0);
    write_u64(cpu, args[1], total as u64);
    Some(S_OK)
}

/// Put an event of `bytes` in the buffer `this` at `rt`.
fn pack(cpu: &mut Cpu, this: u32, rt: i64, group: u32, bytes: &[u8]) -> u32 {
    let Some((id, mut b)) = buf(cpu, this) else {
        return E_INVALIDARG;
    };
    let size = event_size(bytes.len() as u32);
    if b.used.saturating_add(size) > b.max {
        return DMUS_E_BUFFER_FULL;
    }
    if b.used == 0 {
        b.start = rt;
    }
    let at = b.at + b.used;
    cpu.bus.guest_fill(at, size as usize, 0);
    cpu.bus.guest_write_32(at, bytes.len() as u32);
    cpu.bus.guest_write_32(at + 4, group);
    write_u64(cpu, at + 8, rt.wrapping_sub(b.start) as u64);
    // DMUS_EVENT_STRUCTURED for a short message.
    cpu.bus
        .guest_write_32(at + 16, (bytes.first().is_some_and(|&s| s < 0xF0)) as u32);
    cpu.bus.guest_write_bytes(at + HEADER, bytes);
    b.used += size;
    set_buf(cpu, id, b);
    S_OK
}

/// IDirectMusicBuffer::PackStructured(this, time (2 words), group,
/// message).
pub fn pack_structured(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let msg = args[4].to_le_bytes();
    let n = match msg[0] & 0xF0 {
        0xC0 | 0xD0 => 2,
        0xF0 => 1,
        _ => 3,
    };
    Some(pack(
        cpu,
        args[0],
        arg64(args[1], args[2]),
        args[3],
        &msg[..n],
    ))
}

/// IDirectMusicBuffer::PackUnstructured(this, time (2 words), group,
/// length, bytes).
pub fn pack_unstructured(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[5] == 0 {
        return Some(E_POINTER);
    }
    let len = args[4].min(0x10_0000);
    if !cpu.bus.guest_probe(args[5], len as usize, false) {
        return Some(E_POINTER);
    }
    let mut bytes = vec![0; len as usize];
    cpu.bus.guest_read_bytes(args[5], &mut bytes);
    Some(pack(cpu, args[0], arg64(args[1], args[2]), args[3], &bytes))
}

/// IDirectMusicBuffer::ResetReadPtr(this).
pub fn reset_read_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((id, mut b)) = buf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    b.read = 0;
    set_buf(cpu, id, b);
    Some(S_OK)
}

/// IDirectMusicBuffer::GetNextEvent(this, time, group, length, data):
/// S_FALSE after the last.
pub fn get_next_event(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((id, mut b)) = buf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if b.read + HEADER > b.used {
        return Some(S_FALSE);
    }
    let at = b.at + b.read;
    let cb = cpu.bus.guest_read_32(at);
    let group = cpu.bus.guest_read_32(at + 4);
    let delta = read_u64(cpu, at + 8) as i64;
    if args[1] != 0 {
        write_u64(cpu, args[1], b.start.wrapping_add(delta) as u64);
    }
    com::write_out(cpu, args[2], group);
    com::write_out(cpu, args[3], cb);
    com::write_out(cpu, args[4], at + HEADER);
    b.read += event_size(cb);
    set_buf(cpu, id, b);
    Some(S_OK)
}

/// IDirectMusicBuffer::GetRawBufferPtr(this, data).
pub fn get_raw_buffer_ptr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((_, b)) = buf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    Some(if com::write_out(cpu, args[1], b.at) {
        S_OK
    } else {
        E_POINTER
    })
}

/// IDirectMusicBuffer::GetStartTime(this, time): DMUS_E_BUFFER_EMPTY
/// for an empty buffer.
pub fn buffer_get_start_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((_, b)) = buf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    if b.used == 0 {
        return Some(DMUS_E_BUFFER_EMPTY);
    }
    write_u64(cpu, args[1], b.start as u64);
    Some(S_OK)
}

/// IDirectMusicBuffer::GetUsedBytes(this, bytes).
pub fn get_used_bytes(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((_, b)) = buf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    Some(if com::write_out(cpu, args[1], b.used) {
        S_OK
    } else {
        E_POINTER
    })
}

/// IDirectMusicBuffer::GetMaxBytes(this, bytes).
pub fn get_max_bytes(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((_, b)) = buf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    Some(if com::write_out(cpu, args[1], b.max) {
        S_OK
    } else {
        E_POINTER
    })
}

/// IDirectMusicBuffer::GetBufferFormat(this, guid).
pub fn get_buffer_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((_, b)) = buf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    Some(if Guid(b.format).write(cpu, args[1]) {
        S_OK
    } else {
        E_POINTER
    })
}

/// IDirectMusicBuffer::SetStartTime(this, time (2 words)).
pub fn buffer_set_start_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((id, mut b)) = buf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    b.start = arg64(args[1], args[2]);
    set_buf(cpu, id, b);
    Some(S_OK)
}

/// IDirectMusicBuffer::SetUsedBytes(this, bytes).
pub fn set_used_bytes(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((id, mut b)) = buf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] > b.max {
        return Some(DMUS_E_BUFFER_FULL);
    }
    b.used = args[1];
    b.read = b.read.min(b.used);
    set_buf(cpu, id, b);
    Some(S_OK)
}

/// The collection `this` is: its instruments.
fn instruments(cpu: &mut Cpu, this: u32) -> Option<Vec<(u32, String)>> {
    let id = this_of(cpu, this, COLLECTION)?;
    match &state(cpu).get(id)?.kind {
        Kind::Collection { c } => Some(c.instruments.clone()),
        _ => None,
    }
}

/// IDirectMusicCollection::GetInstrument(this, patch, instrument).
pub fn get_instrument(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(list) = instruments(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[2] == 0 {
        return Some(E_POINTER);
    }
    com::write_out(cpu, args[2], 0);
    let Some((patch, name)) = list.into_iter().find(|i| i.0 == args[1]) else {
        return Some(DMUS_E_INVALIDPATCH);
    };
    let id = add(cpu, Kind::Instrument { patch, name });
    Some(give_new(cpu, id, &IID_IDIRECTMUSIC_INSTRUMENT, args[2]))
}

/// IDirectMusicCollection::EnumInstrument(this, index, patch, name,
/// length): S_FALSE past the last.
pub fn enum_instrument(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(list) = instruments(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some((patch, name)) = list.get(args[1] as usize) else {
        return Some(S_FALSE);
    };
    if !com::write_out(cpu, args[2], *patch) {
        return Some(E_POINTER);
    }
    if args[3] != 0 && args[4] != 0 {
        write_wide(cpu, args[3], name, args[4] as usize);
    }
    Some(S_OK)
}

/// IDirectMusicInstrument::GetPatch(this, patch).
pub fn get_patch(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_of(cpu, args[0], INSTRUMENT) else {
        return Some(E_INVALIDARG);
    };
    let Kind::Instrument { patch, .. } = state(cpu).get(id)?.kind else {
        return Some(E_INVALIDARG);
    };
    Some(if com::write_out(cpu, args[1], patch) {
        S_OK
    } else {
        E_POINTER
    })
}

/// IDirectMusicInstrument::SetPatch(this, patch).
pub fn set_patch(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_of(cpu, args[0], INSTRUMENT) else {
        return Some(E_INVALIDARG);
    };
    if let Some(o) = state(cpu).get_mut(id)
        && let Kind::Instrument { patch, .. } = &mut o.kind
    {
        *patch = args[1];
    }
    Some(S_OK)
}

/// A new DirectMusic object with DirectSound set, for a performance
/// that makes its own: its id.
pub fn new_music(cpu: &mut Cpu) -> u32 {
    add(
        cpu,
        Kind::Music {
            m: Music { dsound: true },
        },
    )
}

/// Set DirectSound on the DirectMusic object `id`.
pub fn music_has_dsound(cpu: &mut Cpu, id: u32) {
    if let Some(m) = music_mut(cpu, id) {
        m.dsound = true;
    }
}

/// Send what the ports of the process `pid` have due at `rt`.
pub fn poll_ports(cpu: &mut Cpu, pid: u32, rt: i64) {
    let ids: Vec<u32> = state(cpu)
        .objects
        .iter()
        .filter(|o| o.pid == pid && matches!(o.kind, Kind::Port { .. }))
        .map(|o| o.id)
        .collect();
    for id in ids {
        let due: Vec<Vec<u8>> = match port_mut(cpu, id) {
            Some(p) => {
                let n = p.queue.partition_point(|e| e.0 <= rt);
                p.queue.drain(..n).map(|e| e.1).collect()
            }
            None => continue,
        };
        for bytes in due {
            crate::win32::winmm::midi::send(cpu, &bytes);
        }
    }
}

/// When a port of the process `pid` (any, for None) next has an event
/// due, in reference time.
pub fn ports_due(s: &super::State, pid: Option<u32>) -> Option<i64> {
    s.objects
        .iter()
        .filter(|o| pid.is_none_or(|p| p == o.pid))
        .filter_map(|o| match &o.kind {
            Kind::Port { p } => p.queue.first().map(|e| e.0),
            _ => None,
        })
        .min()
}

/// A collection of the instruments `list`: its id.
pub fn new_collection(cpu: &mut Cpu, list: Vec<(u32, String)>) -> u32 {
    add(
        cpu,
        Kind::Collection {
            c: Collection { instruments: list },
        },
    )
}

/// The MIDI messages that set up a band's instruments on the channels
/// `channel_of` maps their performance channels to.
pub fn band_messages(inst: &[Inst], channel_of: impl Fn(u32) -> Option<u8>) -> Vec<Vec<u8>> {
    inst.iter()
        .filter_map(|i| Some(i.messages(channel_of(i.pchannel)?)))
        .flatten()
        .collect()
}

crate::state_fields!(Music { dsound });
crate::state_fields!(Port {
    synth,
    groups,
    active,
    queue
});
crate::state_fields!(Buf {
    at,
    max,
    used,
    start,
    read,
    format
});
crate::state_fields!(Collection { instruments });
