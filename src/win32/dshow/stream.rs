//! AMSTREAM.DLL's multimedia streams (CLSID_AMMultiMediaStream): a
//! program adds a DirectDraw media stream for the movie's pictures and
//! an audio media stream for its sound, or has the sound played by the
//! default audio renderer, opens the file, and runs the stream. Each
//! IDirectDrawStreamSample::Update then draws the next picture into the
//! sample's surface, and each IAudioStreamSample::Update copies the next
//! sound into the sample's IAudioData buffer (CLSID_AMAudioData).
//!
//! The stream's clock starts when it runs: an update waits until its
//! picture's time, and a picture whose time has passed is dropped for
//! the one the clock is at, as amstream's renderer drops late samples,
//! so that the pictures keep up with the sound. A stream opened with
//! AMMSF_NOCLOCK gives each picture at once. The sound the default
//! renderer plays goes to the sound device as one sound from where the
//! stream runs from (winmm/wave.rs). The end-of-stream event is set when
//! the clock passes the end, or without a clock when the pictures run
//! out.
//!
//! A synchronous update blocks the thread until its picture's time. An
//! asynchronous one (SSUPDATE_ASYNC, or with an event) returns
//! MS_S_PENDING, and the process's multimedia thread completes it at
//! that time (`poll`), setting its event. Updates that call an APC
//! aren't supported (E_NOTIMPL), and nor are streams that write or
//! transform, the default video renderer, or shared samples.
//!
//! A save state keeps where the streams are, not the file, which is
//! read again when it is next wanted, and not the picture decoder, which
//! starts again at the key frame before the next picture.

use super::{
    E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, E_UNEXPECTED, FILE_NOT_FOUND,
    Kind, MSPID_PRIMARY_AUDIO, MSPID_PRIMARY_VIDEO, S_FALSE, S_OK, VFW_E_UNKNOWN_FILE_TYPE, put32,
    put64,
};
use crate::cpu::Cpu;
use crate::win32::com::{self, Guid};
use crate::win32::ddraw::{self, Format as PixelFormat, picture};
use crate::win32::dsound::device::{Format, WAVE_FORMAT_PCM};
use crate::win32::gdi32::surface::dib_stride;
use crate::win32::thread::{self, Wait};
use crate::win32::winmm::mci::media::{self, Media, Pictures};
use crate::win32::winmm::wave;
use std::sync::Arc;

/// Its HRESULTs: MS_SUCCESS_CODE and MS_ERROR_CODE.
pub const MS_S_PENDING: u32 = 0x0004_0001;
pub const MS_S_ENDOFSTREAM: u32 = 0x0004_0003;
pub const MS_E_SAMPLEALLOC: u32 = 0x8004_0401;
pub const MS_E_PURPOSEID: u32 = 0x8004_0402;
pub const MS_E_NOSTREAM: u32 = 0x8004_0403;
pub const MS_E_INCOMPATIBLE: u32 = 0x8004_0405;
pub const MS_E_BUSY: u32 = 0x8004_0406;
pub const MS_E_SOURCEALREADYDEFINED: u32 = 0x8004_0408;
pub const MS_E_NOTRUNNING: u32 = 0x8004_040A;

/// STREAM_TYPE and STREAM_STATE.
const STREAMTYPE_READ: u32 = 0;
const STREAMSTATE_STOP: u32 = 0;
const STREAMSTATE_RUN: u32 = 1;

/// IMultiMediaStream::GetInformation's flags.
const MMSSF_HASCLOCK: u32 = 0x1;
const MMSSF_SUPPORTSEEK: u32 = 0x2;

/// AddMediaStream's flags, and OpenFile's and Render's.
const AMMSF_ADDDEFAULTRENDERER: u32 = 0x1;
const AMMSF_CREATEPEER: u32 = 0x2;
const AMMSF_RENDERTYPEMASK: u32 = 0x3;
const AMMSF_RENDERALLSTREAMS: u32 = 0x1;
const AMMSF_NOCLOCK: u32 = 0x4;
const AMMSF_RUN: u32 = 0x8;

/// Update's flags, and CompletionStatus'.
const SSUPDATE_ASYNC: u32 = 0x1;
const COMPSTAT_WAIT: u32 = 0x2;
const COMPSTAT_ABORT: u32 = 0x4;

/// 100-nanosecond units (STREAM_TIME) a millisecond.
const UNITS_MS: u64 = 10_000;

/// A multimedia stream's state, in its `Kind::Multi`.
#[derive(Clone, Debug, Default)]
pub struct Multi {
    pub initialized: bool,
    /// The filter graph GetFilterGraph gives, which it holds, or 0.
    pub graph: u32,
    /// Its media streams, which it holds.
    pub streams: Vec<u32>,
    /// The file it opened, or none.
    pub file: String,
    /// Whether the default audio renderer plays the file's sound.
    pub renderer: bool,
    /// Whether it has a clock: not opened with AMMSF_NOCLOCK.
    pub clock: bool,
    /// Where it is, in 100-nanosecond units: where it ran from, while it
    /// runs. When it started running, in the clock's microseconds.
    pub pos: u64,
    pub started: Option<u64>,
    /// The renderer's sound, while it plays.
    pub sound: u32,
    /// The end-of-stream event (handle and object), and whether it is
    /// set.
    pub end_event: (u32, u32),
    pub ended: bool,
    /// The file, which a save state doesn't keep.
    pub media: Option<Arc<Media>>,
}

/// A DirectDraw media stream's state, in its `Kind::Video`.
#[derive(Clone, Debug, Default)]
pub struct Video {
    /// The multimedia stream it is of.
    pub multi: u32,
    /// The IDirectDraw it makes its samples' surfaces with, which it
    /// holds, or 0.
    pub ddraw: u32,
    /// SetFormat's pixel format, or the display's.
    pub format: Option<PixelFormat>,
    /// The picture the next update gives.
    pub next: u32,
    /// How many samples it has.
    pub samples: u32,
    /// The decoder, which a save state doesn't keep.
    pub pictures: Pictures,
}

/// An audio media stream's state, in its `Kind::Audio`.
#[derive(Clone, Debug, Default)]
pub struct Audio {
    pub multi: u32,
    /// The bytes of the sound updates have given.
    pub at: u32,
}

/// A sample's state: a DirectDraw stream sample's (`Kind::VideoSample`)
/// or an audio one's.
#[derive(Clone, Debug, Default)]
pub struct Sample {
    /// Its media stream, which it holds.
    pub stream: u32,
    /// A video sample's IDirectDrawSurface, which it holds, and the
    /// rectangle of it the pictures go to: left, top, right, bottom.
    pub surface: u32,
    pub rect: [i32; 4],
    /// An audio sample's IAudioData, which it holds.
    pub data: u32,
    /// The start and end of what the last update gave, in 100-nanosecond
    /// units.
    pub times: (u64, u64),
    /// An asynchronous update: when it completes, in the clock's
    /// microseconds, what it returns then, and the event it sets (an
    /// object it holds, or 0).
    pub due: Option<u64>,
    pub status: u32,
    pub event: u32,
}

/// An IAudioData's state, in its `Kind::AudioData`.
#[derive(Clone, Debug)]
pub struct AudioData {
    /// Its buffer, its size and the bytes of it that are valid; whether
    /// it allocated the buffer itself.
    pub buffer: u32,
    pub size: u32,
    pub actual: u32,
    pub own: bool,
    pub format: Format,
}

impl Default for AudioData {
    /// amstream's default format: 16-bit mono PCM at 11025 Hz.
    fn default() -> AudioData {
        AudioData {
            buffer: 0,
            size: 0,
            actual: 0,
            own: false,
            format: Format::pcm(11025, 16, 1),
        }
    }
}

// Making and finding the objects.

/// CoCreateInstance's CLSID_AMMultiMediaStream.
pub fn create(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = super::add(
        cpu,
        Kind::Multi {
            m: Multi {
                clock: true,
                ..Multi::default()
            },
        },
    );
    let event = super::new_event(cpu);
    if let Some(m) = multi(cpu, id) {
        m.end_event = event;
    }
    super::give_new(cpu, id, iid, out)
}

/// CoCreateInstance's CLSID_AMAudioData.
pub fn create_audio_data(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = super::add(
        cpu,
        Kind::AudioData {
            d: AudioData::default(),
        },
    );
    super::give_new(cpu, id, iid, out)
}

/// The multimedia stream `id` goes: its sound stops, its event goes.
pub fn close(cpu: &mut Cpu, id: u32) {
    stop_sound(cpu, id);
    let event = multi(cpu, id).map_or((0, 0), |m| std::mem::take(&mut m.end_event));
    super::free_event(cpu, event);
}

/// An object went: what it held is let go.
pub fn let_go(cpu: &mut Cpu, kind: &Kind) {
    match kind {
        Kind::Multi { m } => {
            if m.graph != 0 {
                super::drop_ref(cpu, m.graph);
            }
            for &s in &m.streams {
                super::drop_ref(cpu, s);
            }
        }
        Kind::Video { v } => {
            if v.ddraw != 0 {
                ddraw::release(cpu, v.ddraw);
            }
        }
        Kind::VideoSample { s } | Kind::AudioSample { s } => {
            if s.surface != 0 {
                ddraw::release(cpu, s.surface);
            }
            if s.data != 0 {
                super::drop_ref(cpu, s.data);
            }
            if s.event != 0 {
                crate::win32::object::objects(cpu).release(s.event);
            }
            if let Some(Kind::Video { v }) = super::kind(cpu, s.stream) {
                v.samples = v.samples.saturating_sub(1);
            }
            super::drop_ref(cpu, s.stream);
        }
        Kind::AudioData { d } => {
            if d.own && d.buffer != 0 {
                crate::win32::dmusic::free(cpu, d.buffer);
            }
        }
        Kind::Graph { .. } | Kind::Audio { .. } => {}
    }
}

fn multi(cpu: &mut Cpu, id: u32) -> Option<&mut Multi> {
    match super::kind(cpu, id)? {
        Kind::Multi { m } => Some(m),
        _ => None,
    }
}

fn video(cpu: &mut Cpu, id: u32) -> Option<&mut Video> {
    match super::kind(cpu, id)? {
        Kind::Video { v } => Some(v),
        _ => None,
    }
}

fn audio(cpu: &mut Cpu, id: u32) -> Option<&mut Audio> {
    match super::kind(cpu, id)? {
        Kind::Audio { a } => Some(a),
        _ => None,
    }
}

fn sample(cpu: &mut Cpu, id: u32) -> Option<&mut Sample> {
    match super::kind(cpu, id)? {
        Kind::VideoSample { s } | Kind::AudioSample { s } => Some(s),
        _ => None,
    }
}

fn audio_data(cpu: &mut Cpu, id: u32) -> Option<&mut AudioData> {
    match super::kind(cpu, id)? {
        Kind::AudioData { d } => Some(d),
        _ => None,
    }
}

/// The object `this` is, if `f` finds it of its kind.
fn of<T>(cpu: &mut Cpu, this: u32, f: fn(&mut Cpu, u32) -> Option<&mut T>) -> Result<u32, u32> {
    let id = super::object_of(cpu, this).ok_or(E_POINTER)?;
    f(cpu, id).ok_or(E_POINTER)?;
    Ok(id)
}

/// The multimedia stream a media stream is of.
fn multi_of_stream(cpu: &mut Cpu, stream: u32) -> u32 {
    match super::kind(cpu, stream) {
        Some(Kind::Video { v }) => v.multi,
        Some(Kind::Audio { a }) => a.multi,
        _ => 0,
    }
}

/// The purpose of a media stream.
fn purpose(cpu: &mut Cpu, stream: u32) -> Option<Guid> {
    match super::kind(cpu, stream)? {
        Kind::Video { .. } => Some(MSPID_PRIMARY_VIDEO),
        Kind::Audio { .. } => Some(MSPID_PRIMARY_AUDIO),
        _ => None,
    }
}

/// The file the multimedia stream `id` opened, read again after a
/// restore.
fn media(cpu: &mut Cpu, id: u32) -> Option<Arc<Media>> {
    let m = multi(cpu, id)?;
    if let Some(media) = &m.media {
        return Some(media.clone());
    }
    let file = m.file.clone();
    if file.is_empty() {
        return None;
    }
    let bytes = media::read_file(cpu, &file)?;
    let media = Arc::new(Media::parse(&bytes, true)?);
    multi(cpu, id)?.media = Some(media.clone());
    Some(media)
}

/// The stream time of the multimedia stream `m` at `now`, in
/// 100-nanosecond units.
fn time_at(m: &Multi, now: u64) -> u64 {
    match m.started {
        Some(started) => m.pos + now.saturating_sub(started) * 10,
        None => m.pos,
    }
}

/// When the stream time `t` comes, in the clock's microseconds, for a
/// running stream.
fn when(m: &Multi, t: u64) -> Option<u64> {
    let started = m.started?;
    Some(started + t.saturating_sub(m.pos) / 10)
}

/// When the running stream `m` of the file `length` (milliseconds) long
/// reaches its end.
fn end_of(m: &Multi, length: u32) -> Option<u64> {
    if !m.clock || m.ended {
        return None;
    }
    when(m, length as u64 * UNITS_MS)
}

/// The end of the multimedia stream `id`: its event is set.
fn reach_end(cpu: &mut Cpu, id: u32) {
    let Some(m) = multi(cpu, id) else {
        return;
    };
    if std::mem::replace(&mut m.ended, true) {
        return;
    }
    let event = m.end_event.1;
    super::set_event(cpu, event);
}

/// The process `pid`'s streams at `now`: asynchronous updates that are
/// due complete, and a stream that reached its end sets its event.
pub fn poll(cpu: &mut Cpu, pid: u32, now: u64) {
    let due: Vec<u32> = super::state(cpu)
        .objects
        .iter()
        .filter(|o| o.pid == pid)
        .filter(|o| match &o.kind {
            Kind::VideoSample { s } | Kind::AudioSample { s } => s.due.is_some_and(|d| d <= now),
            _ => false,
        })
        .map(|o| o.id)
        .collect();
    for id in due {
        complete(cpu, id);
    }
    // A running stream's file, not read again since a restore, is read
    // now, so that its end can be seen. One that is gone has nothing
    // more to give: the stream ends.
    let unread: Vec<u32> = super::state(cpu)
        .objects
        .iter()
        .filter(|o| o.pid == pid)
        .filter(|o| matches!(&o.kind, Kind::Multi { m } if needs_read(m)))
        .map(|o| o.id)
        .collect();
    for id in unread {
        if media(cpu, id).is_none() {
            reach_end(cpu, id);
        }
    }
    let ended: Vec<u32> = super::state(cpu)
        .objects
        .iter()
        .filter(|o| o.pid == pid)
        .filter_map(|o| match &o.kind {
            Kind::Multi { m } => {
                let length = m.media.as_ref()?.length_ms();
                end_of(m, length).filter(|&e| e <= now).map(|_| o.id)
            }
            _ => None,
        })
        .collect();
    for id in ended {
        reach_end(cpu, id);
    }
}

/// When the process `pid`'s streams next have something due.
pub fn next_due(s: &super::State, pid: Option<u32>) -> Option<u64> {
    s.objects
        .iter()
        .filter(|o| pid.is_none_or(|p| o.pid == p))
        .filter_map(|o| match &o.kind {
            Kind::VideoSample { s } | Kind::AudioSample { s } => s.due,
            Kind::Multi { m } if needs_read(m) => m.started,
            Kind::Multi { m } => end_of(m, m.media.as_ref()?.length_ms()),
            _ => None,
        })
        .min()
}

/// Whether the multimedia stream `m` runs towards its end on its clock
/// without its file, as after a restore: `poll` reads it at once.
fn needs_read(m: &Multi) -> bool {
    m.media.is_none() && !m.file.is_empty() && m.started.is_some() && m.clock && !m.ended
}

/// The asynchronous update of the sample `id` is done: its event is set.
fn complete(cpu: &mut Cpu, id: u32) {
    let Some(s) = sample(cpu, id) else {
        return;
    };
    s.due = None;
    let event = std::mem::take(&mut s.event);
    if event != 0 {
        super::set_event(cpu, event);
        crate::win32::object::objects(cpu).release(event);
    }
}

/// Whether the sample `id` has an asynchronous update still to come. One
/// whose time has passed completes here, before the multimedia thread
/// gets to it: a thread that waited for it with CompletionStatus can
/// wake first, and its next Update mustn't find the sample busy.
fn pending(cpu: &mut Cpu, id: u32) -> bool {
    let Some(due) = sample(cpu, id).and_then(|s| s.due) else {
        return false;
    };
    if due > cpu.bus.clock.now_micros() {
        return true;
    }
    complete(cpu, id);
    false
}

/// The multimedia thread's times changed.
fn reschedule(cpu: &mut Cpu) {
    crate::win32::winmm::schedule(cpu);
}

// IAMMultiMediaStream.

/// IMultiMediaStream::GetInformation(this, flags, type): either may be
/// NULL.
pub fn get_information(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], multi));
    let clock = multi(cpu, id).is_some_and(|m| m.clock);
    let flags = MMSSF_SUPPORTSEEK | if clock { MMSSF_HASCLOCK } else { 0 };
    if args[1] != 0 {
        hr!(put32(cpu, args[1], flags));
    }
    if args[2] != 0 {
        hr!(put32(cpu, args[2], STREAMTYPE_READ));
    }
    Some(S_OK)
}

/// IMultiMediaStream::GetMediaStream(this, purpose, out).
pub fn get_media_stream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], multi));
    if args[2] == 0 {
        return Some(E_POINTER);
    }
    com::write_out(cpu, args[2], 0);
    let Some(want) = Guid::read(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    let streams = multi(cpu, id)
        .map(|m| m.streams.clone())
        .unwrap_or_default();
    for s in streams {
        if purpose(cpu, s) == Some(want) {
            let at = super::pointer(cpu, s);
            return Some(put32(cpu, args[2], at).map_or_else(|e| e, |_| S_OK));
        }
    }
    Some(MS_E_NOSTREAM)
}

/// IMultiMediaStream::EnumMediaStreams(this, index, out): S_FALSE past
/// the last.
pub fn enum_media_streams(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], multi));
    if args[2] == 0 {
        return Some(E_POINTER);
    }
    let stream = multi(cpu, id).and_then(|m| m.streams.get(args[1] as usize).copied());
    let Some(s) = stream else {
        com::write_out(cpu, args[2], 0);
        return Some(S_FALSE);
    };
    let at = super::pointer(cpu, s);
    Some(put32(cpu, args[2], at).map_or_else(|e| e, |_| S_OK))
}

/// IMultiMediaStream::GetState(this, out).
pub fn get_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], multi));
    let running = multi(cpu, id).is_some_and(|m| m.started.is_some());
    let state = if running {
        STREAMSTATE_RUN
    } else {
        STREAMSTATE_STOP
    };
    Some(put32(cpu, args[1], state).map_or_else(|e| e, |_| S_OK))
}

/// IMultiMediaStream::SetState(this, state): run from where it is, or
/// stop there.
pub fn set_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], multi));
    match args[1] {
        STREAMSTATE_RUN => hr!(run(cpu, id)),
        STREAMSTATE_STOP => halt(cpu, id),
        _ => return Some(E_INVALIDARG),
    }
    Some(S_OK)
}

/// The multimedia stream `id` runs from where it is: its clock starts,
/// and the renderer's sound plays from there.
fn run(cpu: &mut Cpu, id: u32) -> Result<(), u32> {
    if multi(cpu, id).is_none_or(|m| m.started.is_some()) {
        return Ok(());
    }
    crate::win32::winmm::callback::ensure_thread(cpu).ok_or(E_OUTOFMEMORY)?;
    let now = cpu.bus.clock.now_micros();
    if let Some(m) = multi(cpu, id) {
        m.started = Some(now);
    }
    start_sound(cpu, id);
    reschedule(cpu);
    Ok(())
}

/// The multimedia stream `id` stops where it is.
fn halt(cpu: &mut Cpu, id: u32) {
    let now = cpu.bus.clock.now_micros();
    if let Some(m) = multi(cpu, id) {
        m.pos = time_at(m, now);
        m.started = None;
    }
    stop_sound(cpu, id);
    reschedule(cpu);
}

/// The renderer's sound plays from where the running stream `id` is.
fn start_sound(cpu: &mut Cpu, id: u32) {
    let Some(m) = multi(cpu, id) else {
        return;
    };
    if !m.renderer || m.started.is_none() {
        return;
    }
    let pos = m.pos;
    let Some(media) = media(cpu, id) else {
        return;
    };
    let Some((f, data)) = &media.audio else {
        return;
    };
    let at = audio_offset(f, pos).min(data.len());
    if at >= data.len() {
        return;
    }
    let sound = wave::play_wav(cpu, *f, data[at..].to_vec(), false);
    if let Some(m) = multi(cpu, id) {
        m.sound = sound;
    }
}

fn stop_sound(cpu: &mut Cpu, id: u32) {
    let sound = multi(cpu, id).map_or(0, |m| std::mem::take(&mut m.sound));
    if sound != 0 {
        wave::stop_sound(cpu, sound);
    }
}

/// The byte of PCM sound in the format `f` at the time `t` (100 ns), at
/// the start of its frame.
fn audio_offset(f: &Format, t: u64) -> usize {
    let block = f.block() as u64;
    (t * f.bytes_per_second() as u64 / 10_000_000 / block * block) as usize
}

/// The time (100 ns) of the byte `at` of PCM sound in the format `f`.
fn audio_time(f: &Format, at: u64) -> u64 {
    at * 10_000_000 / f.bytes_per_second().max(1) as u64
}

/// IMultiMediaStream::GetTime(this, out): the stream time.
pub fn get_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], multi));
    let now = cpu.bus.clock.now_micros();
    let t = multi(cpu, id).map_or(0, |m| time_at(m, now));
    Some(put64(cpu, args[1], t).map_or_else(|e| e, |_| S_OK))
}

/// IMultiMediaStream::GetDuration(this, out).
pub fn get_duration(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], multi));
    let Some(m) = media(cpu, id) else {
        return Some(MS_E_NOSTREAM);
    };
    let t = m.length_ms() as u64 * UNITS_MS;
    Some(put64(cpu, args[1], t).map_or_else(|e| e, |_| S_OK))
}

/// IMultiMediaStream::Seek(this, time (64 bits)): the streams go on from
/// there.
pub fn seek(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], multi));
    let t = (args[2] as u64) << 32 | args[1] as u64;
    let t = (t as i64).max(0) as u64;
    let Some(media) = media(cpu, id) else {
        return Some(MS_E_NOSTREAM);
    };
    let running = multi(cpu, id).is_some_and(|m| m.started.is_some());
    halt(cpu, id);
    let (event, streams) = {
        let Some(m) = multi(cpu, id) else {
            return Some(E_POINTER);
        };
        m.pos = t;
        m.ended = false;
        (m.end_event.1, m.streams.clone())
    };
    super::reset_event(cpu, event);
    let frame = media.frame_at(t / UNITS_MS);
    for s in streams {
        if let Some(v) = video(cpu, s) {
            v.next = frame;
        }
        if let Some(a) = audio(cpu, s)
            && let Some((f, _)) = &media.audio
        {
            a.at = audio_offset(f, t) as u32;
        }
    }
    if running {
        hr!(run(cpu, id));
    }
    Some(S_OK)
}

/// IMultiMediaStream::GetEndOfStreamEventHandle(this, out).
pub fn get_end_of_stream_event(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], multi));
    let handle = multi(cpu, id).map_or(0, |m| m.end_event.0);
    Some(put32(cpu, args[1], handle).map_or_else(|e| e, |_| S_OK))
}

/// IAMMultiMediaStream::Initialize(this, type, flags, graph): a stream
/// that reads, in the graph given or one of its own.
pub fn initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, _, graph] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let id = hr!(of(cpu, this, multi));
    if kind != STREAMTYPE_READ {
        return Some(E_NOTIMPL);
    }
    if graph != 0 {
        let g = super::object_of(cpu, graph);
        let Some(g) = g.filter(|&g| matches!(super::kind(cpu, g), Some(Kind::Graph { .. }))) else {
            return Some(E_NOINTERFACE);
        };
        super::hold(cpu, g);
        let old = multi(cpu, id).map_or(0, |m| std::mem::replace(&mut m.graph, g));
        if old != 0 {
            super::drop_ref(cpu, old);
        }
    }
    if let Some(m) = multi(cpu, id) {
        m.initialized = true;
    }
    Some(S_OK)
}

/// IAMMultiMediaStream::GetFilterGraph(this, out): the graph it was
/// given, or one of its own, made the first time.
pub fn get_filter_graph(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], multi));
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    let mut g = multi(cpu, id).map_or(0, |m| m.graph);
    if g == 0 {
        // The stream keeps the reference the new graph has.
        g = super::graph::new(cpu);
        if let Some(m) = multi(cpu, id) {
            m.graph = g;
        }
    }
    let Some(at) = super::block(cpu, g, super::GRAPH_BUILDER) else {
        return Some(E_OUTOFMEMORY);
    };
    super::hold(cpu, g);
    Some(put32(cpu, args[1], at).map_or_else(|e| e, |_| S_OK))
}

/// IAMMultiMediaStream::AddMediaStream(this, object, purpose, flags,
/// out): a DirectDraw media stream for MSPID_PrimaryVideo, with the
/// IDirectDraw `object` or none, or an audio media stream for
/// MSPID_PrimaryAudio; or with AMMSF_ADDDEFAULTRENDERER, the default
/// audio renderer, which has no stream to give.
pub fn add_media_stream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, object, purpose_at, flags, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let id = hr!(of(cpu, this, multi));
    if out != 0 {
        com::write_out(cpu, out, 0);
    }
    let Some(want) = Guid::read(cpu, purpose_at) else {
        return Some(E_POINTER);
    };
    if flags & AMMSF_CREATEPEER != 0 {
        return Some(E_NOTIMPL);
    }
    let streams = multi(cpu, id)
        .map(|m| m.streams.clone())
        .unwrap_or_default();
    let taken = streams.iter().any(|&s| purpose(cpu, s) == Some(want));
    if flags & AMMSF_ADDDEFAULTRENDERER != 0 {
        if want != MSPID_PRIMARY_AUDIO {
            return Some(E_NOTIMPL);
        }
        let Some(m) = multi(cpu, id) else {
            return Some(E_POINTER);
        };
        if taken || m.renderer {
            return Some(MS_E_PURPOSEID);
        }
        m.renderer = true;
        start_sound(cpu, id);
        return Some(S_OK);
    }
    if taken || multi(cpu, id).is_some_and(|m| m.renderer && want == MSPID_PRIMARY_AUDIO) {
        return Some(MS_E_PURPOSEID);
    }
    let kind = if want == MSPID_PRIMARY_VIDEO {
        let ddraw = if object == 0 {
            0
        } else if picture::is_ddraw(cpu, object) {
            hr!(picture::ddraw_interface(cpu, object))
        } else {
            return Some(E_NOINTERFACE);
        };
        Kind::Video {
            v: Video {
                multi: id,
                ddraw,
                ..Video::default()
            },
        }
    } else if want == MSPID_PRIMARY_AUDIO {
        Kind::Audio {
            a: Audio { multi: id, at: 0 },
        }
    } else {
        return Some(MS_E_PURPOSEID);
    };
    let stream = super::add(cpu, kind);
    if let Some(m) = multi(cpu, id) {
        m.streams.push(stream);
    }
    if out != 0 {
        let at = super::pointer(cpu, stream);
        com::write_out(cpu, out, at);
    }
    Some(S_OK)
}

/// IAMMultiMediaStream::OpenFile(this, file, flags): the file's streams
/// go to the media streams; with AMMSF_RENDERALLSTREAMS its sound
/// without one to the default renderer; with AMMSF_RUN it runs.
pub fn open_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, file, flags] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let id = hr!(of(cpu, this, multi));
    if file == 0 {
        return Some(E_POINTER);
    }
    if multi(cpu, id).is_some_and(|m| !m.file.is_empty()) {
        return Some(MS_E_SOURCEALREADYDEFINED);
    }
    let path = crate::win32::dmusic::read_wide(cpu, file, 260);
    let Some(bytes) = media::read_file(cpu, &path) else {
        return Some(FILE_NOT_FOUND);
    };
    let Some(media) = Media::parse(&bytes, true) else {
        return Some(VFW_E_UNKNOWN_FILE_TYPE);
    };
    let media = Arc::new(media);
    let Some(m) = multi(cpu, id) else {
        return Some(E_POINTER);
    };
    m.initialized = true;
    m.file = path;
    m.media = Some(media);
    m.clock = flags & AMMSF_NOCLOCK == 0;
    hr!(render_streams(cpu, id, flags));
    if flags & AMMSF_RUN != 0 {
        hr!(run(cpu, id));
    }
    Some(S_OK)
}

/// IAMMultiMediaStream::Render(this, flags).
pub fn render(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], multi));
    if multi(cpu, id).is_some_and(|m| m.file.is_empty()) {
        return Some(MS_E_NOSTREAM);
    }
    hr!(render_streams(cpu, id, args[1]));
    Some(S_OK)
}

/// With AMMSF_RENDERALLSTREAMS, the sound of the file of the stream `id`
/// that no audio media stream takes goes to the default renderer.
fn render_streams(cpu: &mut Cpu, id: u32, flags: u32) -> Result<(), u32> {
    if flags & AMMSF_RENDERTYPEMASK != AMMSF_RENDERALLSTREAMS {
        return Ok(());
    }
    let has_sound = media(cpu, id).is_some_and(|m| m.audio.is_some());
    let streams = multi(cpu, id)
        .map(|m| m.streams.clone())
        .unwrap_or_default();
    let taken = streams
        .iter()
        .any(|&s| purpose(cpu, s) == Some(MSPID_PRIMARY_AUDIO));
    if has_sound
        && !taken
        && let Some(m) = multi(cpu, id)
    {
        m.renderer = true;
    }
    Ok(())
}

// IMediaStream.

/// The media stream `this` is, of either kind.
fn stream_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    let id = super::object_of(cpu, this).ok_or(E_POINTER)?;
    match super::kind(cpu, id) {
        Some(Kind::Video { .. } | Kind::Audio { .. }) => Ok(id),
        _ => Err(E_POINTER),
    }
}

/// IMediaStream::GetMultiMediaStream(this, out).
pub fn get_multi_media_stream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(stream_of(cpu, args[0]));
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    let m = multi_of_stream(cpu, id);
    if multi(cpu, m).is_none() {
        com::write_out(cpu, args[1], 0);
        return Some(E_UNEXPECTED);
    }
    let at = super::pointer(cpu, m);
    Some(put32(cpu, args[1], at).map_or_else(|e| e, |_| S_OK))
}

/// IMediaStream::GetInformation(this, purpose, type): either may be
/// NULL.
pub fn stream_get_information(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(stream_of(cpu, args[0]));
    if args[1] != 0 {
        let p = purpose(cpu, id).unwrap_or_default();
        if !p.write(cpu, args[1]) {
            return Some(E_POINTER);
        }
    }
    if args[2] != 0 {
        hr!(put32(cpu, args[2], STREAMTYPE_READ));
    }
    Some(S_OK)
}

/// IMediaStream::SetSameFormat(this, other, flags): a stream of the same
/// kind has the same format, the file's.
pub fn set_same_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(stream_of(cpu, args[0]));
    let other = hr!(stream_of(cpu, args[1]));
    Some(if purpose(cpu, id) == purpose(cpu, other) {
        S_OK
    } else {
        MS_E_INCOMPATIBLE
    })
}

/// IMediaStream::AllocateSample(this, flags, out): a DirectDraw stream's
/// sample with a surface of its own. An audio stream's samples need the
/// program's IAudioData.
pub fn allocate_sample(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(stream_of(cpu, args[0]));
    if video(cpu, id).is_none() {
        return Some(E_NOTIMPL);
    }
    Some(new_video_sample(cpu, id, 0, 0, args[2]))
}

/// IMediaStream::SendEndOfStream(this, flags): for streams that write.
pub fn send_end_of_stream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(stream_of(cpu, args[0]));
    Some(MS_E_INCOMPATIBLE)
}

// IDirectDrawMediaStream.

/// The picture size of the video stream `id`'s file, if it has
/// pictures.
fn picture_size(cpu: &mut Cpu, id: u32) -> Option<(u32, u32)> {
    let m = multi_of_stream(cpu, id);
    let media = media(cpu, m)?;
    (media.frames > 0).then_some((media.width, media.height))
}

/// The pixel format the video stream `id` gives its pictures in.
fn pixel_format(cpu: &mut Cpu, id: u32) -> PixelFormat {
    match video(cpu, id).and_then(|v| v.format) {
        Some(f) => f,
        None => picture::display_format(cpu),
    }
}

/// IDirectDrawMediaStream::GetFormat(this, current, palette, desired,
/// flags): the pictures' size, in the display's format or SetFormat's.
/// Any of them may be NULL.
pub fn dd_get_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, current, palette, desired, flags] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let id = hr!(of(cpu, this, video));
    let Some((w, h)) = picture_size(cpu, id) else {
        return Some(MS_E_NOSTREAM);
    };
    let format = pixel_format(cpu, id);
    for at in [current, desired] {
        if at != 0 {
            hr!(picture::write_desc(cpu, at, w, h, &format));
        }
    }
    if palette != 0 {
        hr!(put32(cpu, palette, 0));
    }
    if flags != 0 {
        hr!(put32(cpu, flags, 0));
    }
    Some(S_OK)
}

/// IDirectDrawMediaStream::SetFormat(this, desc, palette): the pixel
/// format of the samples it makes. The size, if the description has
/// one, must be the pictures'.
pub fn dd_set_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], video));
    let (w, h, format) = hr!(picture::read_desc(cpu, args[1]));
    if let Some((pw, ph)) = picture_size(cpu, id)
        && ((w != 0 && w != pw) || (h != 0 && h != ph))
    {
        return Some(MS_E_INCOMPATIBLE);
    }
    if let Some(f) = format {
        if !matches!(f.bits, 8 | 16 | 24 | 32) || f.flags & 0x4 != 0 {
            return Some(MS_E_INCOMPATIBLE);
        }
        if video(cpu, id).is_some_and(|v| v.samples > 0) {
            return Some(MS_E_SAMPLEALLOC);
        }
        if let Some(v) = video(cpu, id) {
            v.format = Some(f);
        }
    }
    Some(S_OK)
}

/// The IDirectDraw the video stream `id` makes surfaces with: the one it
/// was given, or one of its own, made the first time.
fn ddraw_of(cpu: &mut Cpu, id: u32) -> Result<u32, u32> {
    let dd = video(cpu, id).map_or(0, |v| v.ddraw);
    if dd != 0 {
        return Ok(dd);
    }
    let dd = picture::create_ddraw(cpu).ok_or(E_OUTOFMEMORY)?;
    if let Some(v) = video(cpu, id) {
        v.ddraw = dd;
    }
    Ok(dd)
}

/// IDirectDrawMediaStream::GetDirectDraw(this, out).
pub fn dd_get_direct_draw(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], video));
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    let dd = hr!(ddraw_of(cpu, id));
    ddraw::add_ref(cpu, dd);
    Some(put32(cpu, args[1], dd).map_or_else(|e| e, |_| S_OK))
}

/// IDirectDrawMediaStream::SetDirectDraw(this, ddraw): before it has
/// samples.
pub fn dd_set_direct_draw(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], video));
    if video(cpu, id).is_some_and(|v| v.samples > 0) {
        return Some(MS_E_SAMPLEALLOC);
    }
    let new = if args[1] == 0 {
        0
    } else {
        hr!(picture::ddraw_interface(cpu, args[1]))
    };
    let old = video(cpu, id).map_or(0, |v| std::mem::replace(&mut v.ddraw, new));
    if old != 0 {
        ddraw::release(cpu, old);
    }
    Some(S_OK)
}

/// IDirectDrawMediaStream::CreateSample(this, surface, rect, flags,
/// out): a sample that draws into the program's surface, at `rect` or
/// its top left corner, or into one it makes the pictures' size.
pub fn dd_create_sample(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, surface, rect, _, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let id = hr!(of(cpu, this, video));
    Some(new_video_sample(cpu, id, surface, rect, out))
}

fn new_video_sample(cpu: &mut Cpu, id: u32, surface: u32, rect_at: u32, out: u32) -> u32 {
    if out == 0 {
        return E_POINTER;
    }
    com::write_out(cpu, out, 0);
    let surface = if surface == 0 {
        let Some(size) = picture_size(cpu, id) else {
            return MS_E_NOSTREAM;
        };
        let dd = match ddraw_of(cpu, id) {
            Ok(dd) => dd,
            Err(e) => return e,
        };
        let format = video(cpu, id).and_then(|v| v.format);
        match picture::offscreen_surface(cpu, dd, size, format) {
            Ok(s) => s,
            Err(e) => return e,
        }
    } else if picture::is_surface(cpu, surface) {
        ddraw::add_ref(cpu, surface);
        surface
    } else {
        return E_INVALIDARG;
    };
    let (sw, sh) = picture::surface_size(cpu, surface).unwrap_or((0, 0));
    let rect = if rect_at != 0 && cpu.bus.guest_probe(rect_at, 16, false) {
        [0, 4, 8, 12].map(|k| cpu.bus.guest_read_32(rect_at + k) as i32)
    } else {
        [0, 0, sw as i32, sh as i32]
    };
    super::hold(cpu, id);
    if let Some(v) = video(cpu, id) {
        v.samples += 1;
    }
    let s = super::add(
        cpu,
        Kind::VideoSample {
            s: Sample {
                stream: id,
                surface,
                rect,
                ..Sample::default()
            },
        },
    );
    let at = super::pointer(cpu, s);
    super::drop_ref(cpu, s);
    if at == 0 {
        return E_OUTOFMEMORY;
    }
    com::write_out(cpu, out, at);
    S_OK
}

/// IDirectDrawMediaStream::GetTimePerFrame(this, out).
pub fn dd_get_time_per_frame(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], video));
    let m = multi_of_stream(cpu, id);
    let Some(media) = media(cpu, m).filter(|m| m.frames > 0) else {
        return Some(MS_E_NOSTREAM);
    };
    let (num, den) = media.rate;
    let t = den as u64 * 10_000_000 / num.max(1) as u64;
    Some(put64(cpu, args[1], t).map_or_else(|e| e, |_| S_OK))
}

// IAudioMediaStream.

/// The file's sound format of the audio stream `id`.
fn sound_format(cpu: &mut Cpu, id: u32) -> Option<Format> {
    let m = multi_of_stream(cpu, id);
    media(cpu, m)?.audio.as_ref().map(|(f, _)| *f)
}

/// Write a WAVEFORMATEX of `f` at `at`.
fn write_wave_format(cpu: &mut Cpu, at: u32, f: &Format) -> Result<(), u32> {
    if at == 0 || !cpu.bus.guest_probe(at, 18, true) {
        return Err(E_POINTER);
    }
    let mut b = Vec::with_capacity(18);
    b.extend_from_slice(&f.tag.to_le_bytes());
    b.extend_from_slice(&f.channels.to_le_bytes());
    b.extend_from_slice(&f.rate.to_le_bytes());
    b.extend_from_slice(&f.bytes_per_second().to_le_bytes());
    b.extend_from_slice(&(f.block() as u16).to_le_bytes());
    b.extend_from_slice(&f.bits.to_le_bytes());
    b.extend_from_slice(&0u16.to_le_bytes());
    cpu.bus.guest_write_bytes(at, &b);
    Ok(())
}

/// The PCM format of the WAVEFORMATEX at `at`.
fn read_wave_format(cpu: &mut Cpu, at: u32) -> Result<Format, u32> {
    if at == 0 || !cpu.bus.guest_probe(at, 16, false) {
        return Err(E_POINTER);
    }
    let tag = cpu.bus.guest_read_16(at);
    let channels = cpu.bus.guest_read_16(at + 2);
    let rate = cpu.bus.guest_read_32(at + 4);
    let bits = cpu.bus.guest_read_16(at + 14);
    if tag != WAVE_FORMAT_PCM || !matches!(channels, 1 | 2) || !matches!(bits, 8 | 16) {
        return Err(E_INVALIDARG);
    }
    if rate == 0 {
        return Err(E_INVALIDARG);
    }
    Ok(Format::pcm(rate, bits, channels))
}

/// IAudioMediaStream::GetFormat(this, out): the file's.
pub fn audio_get_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], audio));
    let Some(f) = sound_format(cpu, id) else {
        return Some(MS_E_NOSTREAM);
    };
    Some(write_wave_format(cpu, args[1], &f).map_or_else(|e| e, |_| S_OK))
}

/// IAudioMediaStream::SetFormat(this, format): the file's only, once it
/// is open.
pub fn audio_set_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], audio));
    let f = hr!(read_wave_format(cpu, args[1]));
    Some(match sound_format(cpu, id) {
        Some(file) if file != f => MS_E_INCOMPATIBLE,
        _ => S_OK,
    })
}

/// IAudioMediaStream::CreateSample(this, data, flags, out): a sample
/// that copies the sound into the IAudioData `data`.
pub fn audio_create_sample(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, data, _, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let id = hr!(of(cpu, this, audio));
    if out == 0 {
        return Some(E_POINTER);
    }
    com::write_out(cpu, out, 0);
    let Ok(data) = of(cpu, data, audio_data) else {
        return Some(E_INVALIDARG);
    };
    super::hold(cpu, id);
    super::hold(cpu, data);
    let s = super::add(
        cpu,
        Kind::AudioSample {
            s: Sample {
                stream: id,
                data,
                ..Sample::default()
            },
        },
    );
    let at = super::pointer(cpu, s);
    super::drop_ref(cpu, s);
    if at == 0 {
        return Some(E_OUTOFMEMORY);
    }
    com::write_out(cpu, out, at);
    Some(S_OK)
}

// IStreamSample.

fn sample_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    of(cpu, this, sample)
}

/// IStreamSample::GetMediaStream(this, out).
pub fn sample_get_media_stream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(sample_of(cpu, args[0]));
    let stream = sample(cpu, id).map_or(0, |s| s.stream);
    let at = super::pointer(cpu, stream);
    Some(put32(cpu, args[1], at).map_or_else(|e| e, |_| S_OK))
}

/// IStreamSample::GetSampleTimes(this, start, end, current): what the
/// last update gave, and the stream time. Any may be NULL.
pub fn get_sample_times(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(sample_of(cpu, args[0]));
    let (stream, (start, end)) = sample(cpu, id).map_or((0, (0, 0)), |s| (s.stream, s.times));
    let m = multi_of_stream(cpu, stream);
    let now = cpu.bus.clock.now_micros();
    let current = multi(cpu, m).map_or(0, |m| time_at(m, now));
    for (at, t) in [(args[1], start), (args[2], end), (args[3], current)] {
        if at != 0 {
            hr!(put64(cpu, at, t));
        }
    }
    Some(S_OK)
}

/// IStreamSample::SetSampleTimes(this, start, end): for streams that
/// write.
pub fn set_sample_times(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(sample_of(cpu, args[0]));
    Some(MS_E_INCOMPATIBLE)
}

/// IStreamSample::Update(this, flags, event, APC, APC data): the next
/// picture or sound into the sample. Synchronous, the thread waits until
/// its time; asynchronous (SSUPDATE_ASYNC, or an event), MS_S_PENDING
/// until then, and the event is set.
pub fn update(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, flags, event, apc, _] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let id = hr!(sample_of(cpu, this));
    if apc != 0 {
        return Some(E_NOTIMPL);
    }
    if pending(cpu, id) {
        return Some(MS_E_BUSY);
    }
    let event_id = if event == 0 {
        0
    } else {
        match crate::win32::object::resolve(cpu, event) {
            Some(e) => e,
            None => return Some(E_INVALIDARG),
        }
    };
    if event_id != 0 {
        super::reset_event(cpu, event_id);
    }
    let is_video = matches!(super::kind(cpu, id), Some(Kind::VideoSample { .. }));
    let (status, due) = if is_video {
        hr!(video_update(cpu, id))
    } else {
        hr!(audio_update(cpu, id))
    };
    let now = cpu.bus.clock.now_micros();
    let due = due.filter(|&d| d > now);
    let asynchronous = flags & SSUPDATE_ASYNC != 0 || event != 0;
    if let Some(s) = sample(cpu, id) {
        s.status = status;
    }
    match (due, asynchronous) {
        (None, _) => {
            if event_id != 0 {
                super::set_event(cpu, event_id);
            }
            Some(status)
        }
        (Some(due), true) => {
            if event_id != 0 {
                crate::win32::object::objects(cpu).hold(event_id);
            }
            if let Some(s) = sample(cpu, id) {
                s.due = Some(due);
                s.event = event_id;
            }
            reschedule(cpu);
            Some(MS_S_PENDING)
        }
        (Some(due), false) => {
            cpu.set_eax(status);
            thread::block(
                cpu,
                Wait {
                    deadline: Some(due),
                    timeout_result: status,
                    ..Wait::default()
                },
            );
            None
        }
    }
}

/// IStreamSample::CompletionStatus(this, flags, timeout): the result of
/// the last update; MS_S_PENDING while one is pending, unless
/// COMPSTAT_WAIT waits for it, or COMPSTAT_ABORT ends it.
pub fn completion_status(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, flags, timeout] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let id = hr!(sample_of(cpu, this));
    if !pending(cpu, id) {
        return Some(sample(cpu, id).map_or(E_POINTER, |s| s.status));
    }
    let Some((Some(due), status)) = sample(cpu, id).map(|s| (s.due, s.status)) else {
        return Some(E_POINTER);
    };
    if flags & COMPSTAT_ABORT != 0 {
        complete(cpu, id);
        return Some(status);
    }
    if flags & COMPSTAT_WAIT == 0 || timeout == 0 {
        return Some(MS_S_PENDING);
    }
    let now = cpu.bus.clock.now_micros();
    let limit = (timeout != 0xFFFF_FFFF).then(|| now + timeout as u64 * 1000);
    if limit.is_some_and(|l| l < due) {
        cpu.set_eax(MS_S_PENDING);
        thread::block(
            cpu,
            Wait {
                deadline: limit,
                timeout_result: MS_S_PENDING,
                ..Wait::default()
            },
        );
        return None;
    }
    // Done when it is due: the multimedia thread sets its event then.
    cpu.set_eax(status);
    thread::block(
        cpu,
        Wait {
            deadline: Some(due),
            timeout_result: status,
            ..Wait::default()
        },
    );
    None
}

/// A DirectDraw stream sample's update: the picture the clock is at, or
/// the next one, into its surface. What it returns, and when it is
/// done.
fn video_update(cpu: &mut Cpu, id: u32) -> Result<(u32, Option<u64>), u32> {
    let s = sample(cpu, id).cloned().ok_or(E_POINTER)?;
    let m = multi_of_stream(cpu, s.stream);
    let state = multi(cpu, m).cloned().ok_or(MS_E_NOTRUNNING)?;
    if state.started.is_none() {
        return Err(MS_E_NOTRUNNING);
    }
    let Some(media) = media(cpu, m).filter(|m| m.frames > 0) else {
        reach_end_unclocked(cpu, m);
        return Ok((MS_S_ENDOFSTREAM, None));
    };
    let mut n = video(cpu, s.stream).map_or(0, |v| v.next);
    if n >= media.frames {
        reach_end_unclocked(cpu, m);
        return Ok((MS_S_ENDOFSTREAM, None));
    }
    let now = cpu.bus.clock.now_micros();
    if state.clock {
        // A picture whose time has passed is dropped for the one the
        // clock is at.
        let at = media.frame_at(time_at(&state, now) / UNITS_MS);
        n = n.max(at);
    }
    let (w, h) = (media.width, media.height);
    let stride = dib_stride(w as i32, 24);
    let mut pictures =
        video(cpu, s.stream).map_or_else(Pictures::default, |v| std::mem::take(&mut v.pictures));
    let rows = pictures.rows(&media, n, stride);
    if let Some(v) = video(cpu, s.stream) {
        v.pictures = pictures;
        v.next = n + 1;
    }
    if let Some(rows) = rows {
        let at = (s.rect[0].max(0) as u32, s.rect[1].max(0) as u32);
        let rw = (s.rect[2] - s.rect[0]).max(0) as u32;
        let rh = (s.rect[3] - s.rect[1]).max(0) as u32;
        picture::put_picture(cpu, s.surface, at, &rows, (w.min(rw), h.min(rh)), stride)?;
    }
    let start = media.frame_ms(n) as u64 * UNITS_MS;
    let end = media.frame_ms(n + 1) as u64 * UNITS_MS;
    if let Some(s) = sample(cpu, id) {
        s.times = (start, end);
    }
    let due = if state.clock {
        when(&state, start)
    } else {
        None
    };
    Ok((S_OK, due))
}

/// Without a clock, a stream ends when its pictures run out.
fn reach_end_unclocked(cpu: &mut Cpu, m: u32) {
    if multi(cpu, m).is_some_and(|m| !m.clock) {
        reach_end(cpu, m);
    }
}

/// An audio stream sample's update: as much of the sound from where the
/// stream got to as its buffer holds, at once.
fn audio_update(cpu: &mut Cpu, id: u32) -> Result<(u32, Option<u64>), u32> {
    let s = sample(cpu, id).cloned().ok_or(E_POINTER)?;
    let m = multi_of_stream(cpu, s.stream);
    if multi(cpu, m).is_none_or(|m| m.started.is_none()) {
        return Err(MS_E_NOTRUNNING);
    }
    let d = audio_data(cpu, s.data).cloned().ok_or(E_POINTER)?;
    let media = media(cpu, m);
    let Some((f, pcm)) = media.as_ref().and_then(|m| m.audio.as_ref()) else {
        if let Some(d) = audio_data(cpu, s.data) {
            d.actual = 0;
        }
        return Ok((MS_S_ENDOFSTREAM, None));
    };
    let from = audio(cpu, s.stream).map_or(0, |a| a.at) as usize;
    let block = f.block() as usize;
    let n = (d.size as usize / block * block).min(pcm.len().saturating_sub(from));
    if n == 0 || d.buffer == 0 {
        if let Some(d) = audio_data(cpu, s.data) {
            d.actual = 0;
        }
        return Ok((MS_S_ENDOFSTREAM, None));
    }
    if !cpu.bus.guest_probe(d.buffer, n, true) {
        return Err(E_POINTER);
    }
    cpu.bus.guest_write_bytes(d.buffer, &pcm[from..from + n]);
    if let Some(d) = audio_data(cpu, s.data) {
        d.actual = n as u32;
    }
    if let Some(a) = audio(cpu, s.stream) {
        a.at = (from + n) as u32;
    }
    let times = (audio_time(f, from as u64), audio_time(f, (from + n) as u64));
    if let Some(s) = sample(cpu, id) {
        s.times = times;
    }
    Ok((S_OK, None))
}

// IDirectDrawStreamSample.

/// IDirectDrawStreamSample::GetSurface(this, surface, rect): either may
/// be NULL.
pub fn get_surface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(sample_of(cpu, args[0]));
    let Some(s) = sample(cpu, id).cloned() else {
        return Some(E_POINTER);
    };
    if args[1] != 0 {
        ddraw::add_ref(cpu, s.surface);
        hr!(put32(cpu, args[1], s.surface));
    }
    if args[2] != 0 {
        if !cpu.bus.guest_probe(args[2], 16, true) {
            return Some(E_POINTER);
        }
        for (k, v) in s.rect.iter().enumerate() {
            cpu.bus.guest_write_32(args[2] + 4 * k as u32, *v as u32);
        }
    }
    Some(S_OK)
}

/// IDirectDrawStreamSample::SetRect(this, rect): inside the surface.
pub fn set_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(sample_of(cpu, args[0]));
    if args[1] == 0 || !cpu.bus.guest_probe(args[1], 16, false) {
        return Some(E_POINTER);
    }
    let r = [0, 4, 8, 12].map(|k| cpu.bus.guest_read_32(args[1] + k) as i32);
    let surface = sample(cpu, id).map_or(0, |s| s.surface);
    let (w, h) = picture::surface_size(cpu, surface).unwrap_or((0, 0));
    if r[0] < 0 || r[1] < 0 || r[2] <= r[0] || r[3] <= r[1] || r[2] > w as i32 || r[3] > h as i32 {
        return Some(E_INVALIDARG);
    }
    if let Some(s) = sample(cpu, id) {
        s.rect = r;
    }
    Some(S_OK)
}

// IAudioStreamSample.

/// IAudioStreamSample::GetAudioData(this, out).
pub fn get_audio_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(sample_of(cpu, args[0]));
    let data = sample(cpu, id).map_or(0, |s| s.data);
    if data == 0 {
        return Some(E_NOINTERFACE);
    }
    let at = super::pointer(cpu, data);
    Some(put32(cpu, args[1], at).map_or_else(|e| e, |_| S_OK))
}

// IAudioData.

/// IMemoryData::SetBuffer(this, size, data, flags): the program's
/// buffer, or with NULL one of its own of that size.
pub fn data_set_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, size, data, _] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let id = hr!(of(cpu, this, audio_data));
    if size == 0 {
        return Some(E_INVALIDARG);
    }
    let (buffer, own) = if data == 0 {
        match crate::win32::dmusic::alloc(cpu, size) {
            Some(at) => (at, true),
            None => return Some(E_OUTOFMEMORY),
        }
    } else {
        (data, false)
    };
    let Some(d) = audio_data(cpu, id) else {
        return Some(E_POINTER);
    };
    let old = (d.own && d.buffer != 0).then_some(d.buffer);
    d.buffer = buffer;
    d.size = size;
    d.actual = 0;
    d.own = own;
    if let Some(old) = old {
        crate::win32::dmusic::free(cpu, old);
    }
    Some(S_OK)
}

/// IMemoryData::GetInfo(this, length, data, actual): any may be NULL.
pub fn data_get_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], audio_data));
    let Some(d) = audio_data(cpu, id).cloned() else {
        return Some(E_POINTER);
    };
    for (at, v) in [(args[1], d.size), (args[2], d.buffer), (args[3], d.actual)] {
        if at != 0 {
            hr!(put32(cpu, at, v));
        }
    }
    Some(S_OK)
}

/// IMemoryData::SetActual(this, bytes): no more than the buffer holds.
pub fn data_set_actual(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], audio_data));
    let Some(d) = audio_data(cpu, id) else {
        return Some(E_POINTER);
    };
    if args[1] > d.size {
        return Some(E_INVALIDARG);
    }
    d.actual = args[1];
    Some(S_OK)
}

/// IAudioData::GetFormat(this, out).
pub fn data_get_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], audio_data));
    let f = audio_data(cpu, id).map_or(Format::default(), |d| d.format);
    Some(write_wave_format(cpu, args[1], &f).map_or_else(|e| e, |_| S_OK))
}

/// IAudioData::SetFormat(this, format): PCM.
pub fn data_set_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], audio_data));
    let f = hr!(read_wave_format(cpu, args[1]));
    if let Some(d) = audio_data(cpu, id) {
        d.format = f;
    }
    Some(S_OK)
}

crate::state_fields!(Multi {
    initialized,
    graph,
    streams,
    file,
    renderer,
    clock,
    pos,
    started,
    sound,
    end_event,
    ended
} skip {
    media
});
crate::state_fields!(Video {
    multi,
    ddraw,
    format,
    next,
    samples
} skip {
    pictures
});
crate::state_fields!(Audio { multi, at });
crate::state_fields!(Sample {
    stream,
    surface,
    rect,
    data,
    times,
    due,
    status,
    event
});
crate::state_fields!(AudioData {
    buffer,
    size,
    actual,
    own,
    format
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stream_time_runs_from_where_it_started() {
        let m = Multi {
            pos: 5_000_000,
            started: Some(1_000_000),
            clock: true,
            ..Multi::default()
        };
        assert_eq!(time_at(&m, 1_250_000), 7_500_000);
        assert_eq!(when(&m, 6_000_000), Some(1_100_000));
        // Ten seconds long: half a second from the start, 9.5 s later.
        assert_eq!(end_of(&m, 10_000), Some(10_500_000));
        let stopped = Multi { started: None, ..m };
        assert_eq!(time_at(&stopped, 9_000_000), 5_000_000);
        assert_eq!(end_of(&stopped, 10_000), None);
    }

    #[test]
    fn sound_offsets_are_whole_frames() {
        let f = Format::pcm(22050, 16, 2);
        // A second is 88200 bytes; 1.00001 s is still in frame 22050.
        assert_eq!(audio_offset(&f, 10_000_100), 88200);
        assert_eq!(audio_offset(&f, 5_000_000), 44100);
        assert_eq!(audio_time(&f, 44100), 5_000_000);
    }
}
