//! QUARTZ.DLL's memory allocator (CLSID_MemoryAllocator) and its media
//! samples, and the system clock (CLSID_SystemClock): the objects a
//! program's filter gets from the graph to take the pictures of a movie
//! in, and to time them by (filter.rs). They behave as Wine's quartz
//! (memallocator.c, systemclock.c) has them, which follows Windows'.
//!
//! An allocator makes its samples when it commits, their buffers one
//! after another in pages of their own (VirtualAlloc), as the base
//! classes' CMemAllocator has them, each of the size SetProperties set
//! rounded up to the alignment. It frees them when it decommits with none
//! of them out, or when the last one out comes back. A program may read
//! a little past a buffer's end: Omikron's renderer copies its first row
//! from one row past the picture, which the slack of the last page holds
//! on Windows.
//! A sample's reference count is its object's: when the last reference
//! goes, it goes back to its allocator, which keeps it for the next
//! GetBuffer. A sample that is out holds a reference on its allocator.
//!
//! The clock's time is the clock's milliseconds since the machine
//! started, in 100-nanosecond units, as timeGetTime's are on Windows. Its
//! advises are kept with the clock and fired on the process's multimedia
//! thread (winmm/callback.rs), which `poll` is called on: an event is
//! set, a semaphore released once for each period that passed.

use super::{
    E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, Kind, S_FALSE, S_OK, VFW_E_BADALIGN,
    VFW_E_BUFFER_NOTSET, VFW_E_BUFFER_OVERFLOW, VFW_E_BUFFERS_OUTSTANDING,
    VFW_E_MEDIA_TIME_NOT_SET, VFW_E_NOT_COMMITTED, VFW_E_SAMPLE_TIME_NOT_SET, VFW_E_SIZENOTSET,
    VFW_E_TIMEOUT, VFW_S_NO_STOP_TIME, get64, put32, put64,
};
use crate::cpu::Cpu;
use crate::win32::com::Guid;
use crate::win32::object::{self, Kind as ObjectKind};
use crate::win32::thread::{self, Wait};

/// The sample's flags (AM_SAMPLE2_PROPERTIES' dwSampleFlags).
pub const AM_SAMPLE_SPLICEPOINT: u32 = 0x01;
const AM_SAMPLE_PREROLL: u32 = 0x02;
pub const AM_SAMPLE_DATADISCONTINUITY: u32 = 0x04;
const AM_SAMPLE_TYPECHANGED: u32 = 0x08;
pub const AM_SAMPLE_TIMEVALID: u32 = 0x10;
pub const AM_SAMPLE_STOPVALID: u32 = 0x100;

/// GetBuffer's flag that it mustn't wait.
const AM_GBF_NOWAIT: u32 = 0x04;

/// AM_SAMPLE2_PROPERTIES' size, and how much of it SetProperties writes
/// (up to pbBuffer).
const PROPERTIES: usize = 48;
const PROPERTIES_WRITABLE: usize = 40;

/// An allocator's state, in its `Kind::Allocator`.
#[derive(Clone, Debug, Default)]
pub struct Allocator {
    /// ALLOCATOR_PROPERTIES: how many buffers, their size, their
    /// alignment and the prefix before each.
    pub props: [u32; 4],
    pub committed: bool,
    /// Decommit was asked for while samples were out: it is done when the
    /// last of them comes back.
    pub decommit_queued: bool,
    /// Its samples, while it is committed, and the pages of their
    /// buffers.
    pub samples: Vec<u32>,
    pub pages: u32,
    /// An auto-reset event (by its object) set each time a sample comes
    /// back, which a thread that waits for a buffer waits on.
    pub freed: u32,
}

/// A media sample's state, in its `Kind::MediaSample`.
#[derive(Clone, Debug, Default)]
pub struct MediaSample {
    /// Its allocator, by its object.
    pub allocator: u32,
    /// Its buffer, in the allocator's pages, after the prefix.
    pub buffer: u32,
    pub size: u32,
    /// AM_SAMPLE2_PROPERTIES' lActual, dwSampleFlags and
    /// dwTypeSpecificFlags, its times and its stream's id.
    pub actual: u32,
    pub flags: u32,
    pub type_flags: u32,
    pub start: i64,
    pub stop: i64,
    pub stream_id: u32,
    pub media_time: Option<(i64, i64)>,
    /// The media type SetMediaType gave it: an AM_MEDIA_TYPE of the
    /// sample's own in the program's memory, or 0.
    pub media_type: u32,
    /// Whether GetBuffer gave it out and it hasn't come back.
    pub out: bool,
}

/// A clock's state, in its `Kind::Clock`.
#[derive(Clone, Debug, Default)]
pub struct Clock {
    pub advises: Vec<Advise>,
    pub next_cookie: u32,
    /// The time GetTime gave last.
    pub last: i64,
}

/// An advise: the event or semaphore (by its object, which the advise
/// holds), when it is due in the clock's time, and its period, 0 for an
/// event's.
#[derive(Clone, Debug, Default)]
pub struct Advise {
    pub cookie: u32,
    pub object: u32,
    pub due: i64,
    pub period: i64,
}

fn allocator(cpu: &mut Cpu, id: u32) -> Option<&mut Allocator> {
    match super::kind(cpu, id)? {
        Kind::Allocator { a } => Some(a),
        _ => None,
    }
}

fn sample(cpu: &mut Cpu, id: u32) -> Option<&mut MediaSample> {
    match super::kind(cpu, id)? {
        Kind::MediaSample { s } => Some(s),
        _ => None,
    }
}

fn clock(cpu: &mut Cpu, id: u32) -> Option<&mut Clock> {
    match super::kind(cpu, id)? {
        Kind::Clock { c } => Some(c),
        _ => None,
    }
}

/// The object of the kind `f` finds that `this` is an interface of.
fn of<T>(cpu: &mut Cpu, this: u32, f: fn(&mut Cpu, u32) -> Option<&mut T>) -> Result<u32, u32> {
    let id = super::object_of(cpu, this).ok_or(E_POINTER)?;
    f(cpu, id).ok_or(E_POINTER)?;
    Ok(id)
}

/// Whether the interface pointer `this` is one of an allocator of ours:
/// its object.
pub fn allocator_of(cpu: &mut Cpu, this: u32) -> Option<u32> {
    of(cpu, this, allocator).ok()
}

fn hresult(r: Result<u32, u32>) -> Option<u32> {
    Some(r.unwrap_or_else(|e| e))
}

// The allocator.

/// CoCreateInstance's CLSID_MemoryAllocator.
pub fn create_allocator(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = new_allocator(cpu);
    super::give_new(cpu, id, iid, out)
}

/// A new allocator with no properties set, with one reference.
pub fn new_allocator(cpu: &mut Cpu) -> u32 {
    super::add(
        cpu,
        Kind::Allocator {
            a: Allocator::default(),
        },
    )
}

/// Set the allocator `id`'s properties: those it has after, or the
/// HRESULT it fails with.
pub fn set_props(cpu: &mut Cpu, id: u32, request: [u32; 4]) -> Result<[u32; 4], u32> {
    let a = allocator(cpu, id).ok_or(E_POINTER)?;
    // A decommit that waits for samples to come back has decommitted
    // the allocator as far as the program sees, with buffers out.
    if a.decommit_queued {
        return Err(VFW_E_BUFFERS_OUTSTANDING);
    }
    if a.committed {
        return Err(super::VFW_E_ALREADY_COMMITTED);
    }
    if request[2] == 0 {
        return Err(VFW_E_BADALIGN);
    }
    a.props = request;
    Ok(a.props)
}

/// IMemAllocator::SetProperties(this, request, actual).
pub fn set_properties(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], allocator));
    let (request, actual) = (args[1], args[2]);
    if request == 0 || actual == 0 || !cpu.bus.guest_probe(request, 16, false) {
        return Some(E_POINTER);
    }
    let req = read_props(cpu, request);
    let result = set_props(cpu, id, req);
    let now = allocator(cpu, id).map_or([0; 4], |a| a.props);
    if !write_props(cpu, actual, now) {
        return Some(E_POINTER);
    }
    Some(result.map_or_else(|e| e, |_| S_OK))
}

/// ALLOCATOR_PROPERTIES at `at`.
pub fn read_props(cpu: &mut Cpu, at: u32) -> [u32; 4] {
    std::array::from_fn(|k| cpu.bus.guest_read_32(at + 4 * k as u32))
}

fn write_props(cpu: &mut Cpu, at: u32, props: [u32; 4]) -> bool {
    if at == 0 || !cpu.bus.guest_probe(at, 16, true) {
        return false;
    }
    for (k, v) in props.into_iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * k as u32, v);
    }
    true
}

/// IMemAllocator::GetProperties(this, out).
pub fn get_properties(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], allocator));
    let props = allocator(cpu, id).map_or([0; 4], |a| a.props);
    Some(if write_props(cpu, args[1], props) {
        S_OK
    } else {
        E_POINTER
    })
}

/// Commit the allocator `id`: its samples are made.
pub fn commit_now(cpu: &mut Cpu, id: u32) -> u32 {
    let Some(a) = allocator(cpu, id) else {
        return E_POINTER;
    };
    let [count, size, align, prefix] = a.props;
    if align == 0 {
        return VFW_E_BADALIGN;
    }
    if size == 0 {
        return VFW_E_SIZENOTSET;
    }
    if count == 0 {
        return VFW_E_BUFFER_NOTSET;
    }
    if a.committed {
        a.decommit_queued = false;
        return S_OK;
    }
    // Each buffer with its prefix, rounded up to the alignment.
    let Some((step, total)) = size
        .checked_add(prefix)
        .and_then(|n| n.checked_next_multiple_of(align))
        .and_then(|step| Some((step, step.checked_mul(count)?)))
    else {
        return E_OUTOFMEMORY;
    };
    let Some(pages) = crate::win32::kernel32::alloc_pages(cpu, total) else {
        return E_OUTOFMEMORY;
    };
    let mut samples = Vec::new();
    for k in 0..count {
        let buffer = pages + k * step + prefix;
        let s = super::add(
            cpu,
            Kind::MediaSample {
                s: MediaSample {
                    allocator: id,
                    buffer,
                    size,
                    actual: size,
                    ..MediaSample::default()
                },
            },
        );
        // Made with no reference: it is the allocator's until GetBuffer.
        if let Some(o) = super::state(cpu).get_mut(s) {
            o.refs = 0;
        }
        samples.push(s);
    }
    let freed = object::objects(cpu).add(
        ObjectKind::Event {
            manual: false,
            signaled: false,
        },
        None,
    );
    if let Some(a) = allocator(cpu, id) {
        a.samples = samples;
        a.pages = pages;
        a.committed = true;
        a.decommit_queued = false;
        a.freed = freed;
    }
    S_OK
}

/// IMemAllocator::Commit(this).
pub fn commit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], allocator));
    Some(commit_now(cpu, id))
}

/// Decommit the allocator `id`: its samples go now, or when the last
/// one out comes back.
pub fn decommit_now(cpu: &mut Cpu, id: u32) {
    let Some(a) = allocator(cpu, id) else {
        return;
    };
    if !a.committed {
        return;
    }
    let samples = a.samples.clone();
    let out = samples.iter().any(|&s| {
        super::state(cpu)
            .get(s)
            .is_some_and(|o| matches!(&o.kind, Kind::MediaSample { s } if s.out))
    });
    if out {
        if let Some(a) = allocator(cpu, id) {
            a.decommit_queued = true;
        }
        // A thread waiting for a buffer gives up.
        let freed = allocator(cpu, id).map_or(0, |a| a.freed);
        super::set_event(cpu, freed);
        return;
    }
    free_buffers(cpu, id);
}

/// The allocator `id`'s samples go, and it is decommitted.
fn free_buffers(cpu: &mut Cpu, id: u32) {
    let Some(a) = allocator(cpu, id) else {
        return;
    };
    let samples = std::mem::take(&mut a.samples);
    let pages = std::mem::take(&mut a.pages);
    a.committed = false;
    a.decommit_queued = false;
    let freed = std::mem::take(&mut a.freed);
    free_samples(cpu, &samples);
    if pages != 0 {
        crate::win32::kernel32::free_pages(cpu, pages);
    }
    if freed != 0 {
        // A thread still waiting for a buffer wakes, and finds none.
        super::set_event(cpu, freed);
        object::objects(cpu).release(freed);
    }
}

fn free_samples(cpu: &mut Cpu, samples: &[u32]) {
    for &s in samples {
        if let Some(Kind::MediaSample { s }) = super::state(cpu).get(s).map(|o| o.kind.clone()) {
            free_media_type(cpu, s.media_type);
        }
        super::forget(cpu, s);
    }
}

/// IMemAllocator::Decommit(this).
pub fn decommit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], allocator));
    decommit_now(cpu, id);
    Some(S_OK)
}

/// The allocator `id` goes: its samples with it.
pub fn free_allocator(cpu: &mut Cpu, id: u32) {
    free_buffers(cpu, id);
}

/// A free sample of the allocator `id`, given out with one reference:
/// None when all are out, or the HRESULT GetBuffer fails with.
pub fn take_buffer(cpu: &mut Cpu, id: u32) -> Result<Option<u32>, u32> {
    let a = allocator(cpu, id).ok_or(E_POINTER)?;
    if !a.committed || a.decommit_queued {
        return Err(VFW_E_NOT_COMMITTED);
    }
    let samples = a.samples.clone();
    let free = samples
        .into_iter()
        .find(|&s| sample(cpu, s).is_some_and(|s| !s.out));
    let Some(s) = free else {
        return Ok(None);
    };
    if let Some(m) = sample(cpu, s) {
        m.out = true;
        m.actual = m.size;
    }
    if let Some(o) = super::state(cpu).get_mut(s) {
        o.refs = 1;
    }
    super::hold(cpu, id);
    Ok(Some(s))
}

/// The event the allocator `id` sets when a sample comes back.
pub fn freed_event(cpu: &mut Cpu, id: u32) -> u32 {
    allocator(cpu, id).map_or(0, |a| a.freed)
}

/// IMemAllocator::GetBuffer(this, out, start, end, flags): a free sample;
/// with none, the thread waits for one unless AM_GBF_NOWAIT says not to.
pub fn get_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out, _, _, flags] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let id = hr!(of(cpu, this, allocator));
    hr!(put32(cpu, out, 0));
    match take_buffer(cpu, id) {
        Err(e) => Some(e),
        Ok(Some(s)) => {
            let at = super::block(cpu, s, super::MEDIA_SAMPLE).unwrap_or(0);
            cpu.bus.guest_write_32(out, at);
            Some(S_OK)
        }
        Ok(None) if flags & AM_GBF_NOWAIT != 0 => Some(VFW_E_TIMEOUT),
        Ok(None) => {
            // Wait for a sample to come back, then try again.
            let freed = freed_event(cpu, id);
            cpu.set_eip(cpu.eip().wrapping_sub(4));
            thread::block(
                cpu,
                Wait {
                    objects: vec![freed],
                    ..Wait::default()
                },
            );
            None
        }
    }
}

/// The last reference on the sample `id` went: it goes back to its
/// allocator, which gives back the reference it held.
pub fn returned(cpu: &mut Cpu, id: u32) {
    let Some(s) = sample(cpu, id) else {
        return;
    };
    if !s.out {
        return;
    }
    s.out = false;
    s.flags = 0;
    s.media_time = None;
    let (a, media_type) = (s.allocator, std::mem::take(&mut s.media_type));
    free_media_type(cpu, media_type);
    let freed = freed_event(cpu, a);
    super::set_event(cpu, freed);
    let queued = allocator(cpu, a).is_some_and(|a| a.decommit_queued);
    if queued {
        let samples = allocator(cpu, a).map_or(Vec::new(), |a| a.samples.clone());
        if !samples
            .iter()
            .any(|&s| sample(cpu, s).is_some_and(|s| s.out))
        {
            free_buffers(cpu, a);
        }
    }
    super::drop_ref(cpu, a);
}

// The samples.

fn sample_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    of(cpu, this, sample)
}

/// Run `f` on the sample `this` is: its HRESULT.
fn with_sample(cpu: &mut Cpu, this: u32, f: impl FnOnce(&mut MediaSample) -> u32) -> Option<u32> {
    let id = hr!(sample_of(cpu, this));
    Some(sample(cpu, id).map_or(E_POINTER, f))
}

fn set_flag(s: &mut MediaSample, flag: u32, on: bool) -> u32 {
    if on {
        s.flags |= flag;
    } else {
        s.flags &= !flag;
    }
    S_OK
}

fn flag(s: &MediaSample, flag: u32) -> u32 {
    if s.flags & flag != 0 { S_OK } else { S_FALSE }
}

/// IMediaSample::GetPointer(this, out).
pub fn get_pointer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(sample_of(cpu, args[0]));
    let buffer = sample(cpu, id).map_or(0, |s| s.buffer);
    Some(put32(cpu, args[1], buffer).map_or_else(|e| e, |_| S_OK))
}

/// IMediaSample::GetSize(this): the buffer's size, 0 for what isn't a
/// sample.
pub fn get_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let size = sample_of(cpu, args[0])
        .ok()
        .and_then(|id| sample(cpu, id))
        .map_or(0, |s| s.size);
    Some(size)
}

/// IMediaSample::GetTime(this, start, end).
pub fn get_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(sample_of(cpu, args[0]));
    let Some(s) = sample(cpu, id).cloned() else {
        return Some(E_POINTER);
    };
    if s.flags & AM_SAMPLE_TIMEVALID == 0 {
        return Some(VFW_E_SAMPLE_TIME_NOT_SET);
    }
    let (end, result) = if s.flags & AM_SAMPLE_STOPVALID == 0 {
        (s.start + 1, VFW_S_NO_STOP_TIME)
    } else {
        (s.stop, S_OK)
    };
    hr!(put64(cpu, args[1], s.start as u64));
    hr!(put64(cpu, args[2], end as u64));
    Some(result)
}

/// IMediaSample::SetTime(this, start, end): either may be NULL.
pub fn set_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(sample_of(cpu, args[0]));
    let start = if args[1] != 0 {
        Some(hr!(get64(cpu, args[1])) as i64)
    } else {
        None
    };
    let end = if args[2] != 0 {
        Some(hr!(get64(cpu, args[2])) as i64)
    } else {
        None
    };
    let Some(s) = sample(cpu, id) else {
        return Some(E_POINTER);
    };
    s.flags &= !(AM_SAMPLE_TIMEVALID | AM_SAMPLE_STOPVALID);
    if let Some(start) = start {
        s.start = start;
        s.flags |= AM_SAMPLE_TIMEVALID;
        if let Some(end) = end {
            s.stop = end;
            s.flags |= AM_SAMPLE_STOPVALID;
        }
    }
    Some(S_OK)
}

pub fn is_sync_point(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    with_sample(cpu, args[0], |s| flag(s, AM_SAMPLE_SPLICEPOINT))
}

pub fn set_sync_point(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let on = args[1] != 0;
    with_sample(cpu, args[0], |s| set_flag(s, AM_SAMPLE_SPLICEPOINT, on))
}

pub fn is_preroll(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    with_sample(cpu, args[0], |s| flag(s, AM_SAMPLE_PREROLL))
}

pub fn set_preroll(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let on = args[1] != 0;
    with_sample(cpu, args[0], |s| set_flag(s, AM_SAMPLE_PREROLL, on))
}

pub fn is_discontinuity(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    with_sample(cpu, args[0], |s| flag(s, AM_SAMPLE_DATADISCONTINUITY))
}

pub fn set_discontinuity(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let on = args[1] != 0;
    with_sample(cpu, args[0], |s| {
        set_flag(s, AM_SAMPLE_DATADISCONTINUITY, on)
    })
}

/// IMediaSample::GetActualDataLength(this): the bytes of data, 0 for
/// what isn't a sample.
pub fn get_actual(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let actual = sample_of(cpu, args[0])
        .ok()
        .and_then(|id| sample(cpu, id))
        .map_or(0, |s| s.actual);
    Some(actual)
}

/// IMediaSample::SetActualDataLength(this, length).
pub fn set_actual(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let len = args[1];
    with_sample(cpu, args[0], |s| {
        if len as i32 > s.size as i32 || (len as i32) < 0 {
            return VFW_E_BUFFER_OVERFLOW;
        }
        s.actual = len;
        S_OK
    })
}

/// IMediaSample::GetMediaType(this, out): a copy of the sample's own
/// type, or NULL and S_FALSE when the type hasn't changed.
pub fn get_media_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(sample_of(cpu, args[0]));
    let own = sample(cpu, id).map_or(0, |s| s.media_type);
    if own == 0 {
        if args[1] != 0 {
            hr!(put32(cpu, args[1], 0));
        }
        return Some(S_FALSE);
    }
    let Some(mt) = super::filter::read_media_type(cpu, own) else {
        return Some(E_POINTER);
    };
    let copy = super::filter::new_media_type(cpu, &mt);
    if copy == 0 {
        return Some(E_OUTOFMEMORY);
    }
    Some(put32(cpu, args[1], copy).map_or_else(|e| e, |_| S_OK))
}

/// IMediaSample::SetMediaType(this, type): a copy of it, or with NULL
/// none.
pub fn set_media_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(sample_of(cpu, args[0]));
    let copy = if args[1] != 0 {
        let Some(mt) = super::filter::read_media_type(cpu, args[1]) else {
            return Some(E_POINTER);
        };
        let copy = super::filter::new_media_type(cpu, &mt);
        if copy == 0 {
            return Some(E_OUTOFMEMORY);
        }
        copy
    } else {
        0
    };
    let Some(s) = sample(cpu, id) else {
        return Some(E_POINTER);
    };
    let old = std::mem::replace(&mut s.media_type, copy);
    set_flag(s, AM_SAMPLE_TYPECHANGED, copy != 0);
    free_media_type(cpu, old);
    Some(S_OK)
}

/// Free an AM_MEDIA_TYPE of the program's task heap, as a sample keeps
/// or an enumerator gives: its format block, then itself
/// (DeleteMediaType).
pub fn free_media_type(cpu: &mut Cpu, at: u32) {
    if at == 0 {
        return;
    }
    let format = cpu.bus.guest_read_32(at + 68);
    crate::win32::ole32::task_free(cpu, format);
    crate::win32::ole32::task_free(cpu, at);
}

/// IMediaSample::GetMediaTime(this, start, end).
pub fn get_media_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(sample_of(cpu, args[0]));
    let Some((start, end)) = sample(cpu, id).and_then(|s| s.media_time) else {
        return Some(VFW_E_MEDIA_TIME_NOT_SET);
    };
    hr!(put64(cpu, args[1], start as u64));
    hr!(put64(cpu, args[2], end as u64));
    Some(S_OK)
}

/// IMediaSample::SetMediaTime(this, start, end): with no start, none;
/// a start needs an end.
pub fn set_media_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(sample_of(cpu, args[0]));
    let times = if args[1] != 0 {
        if args[2] == 0 {
            return Some(E_POINTER);
        }
        let start = hr!(get64(cpu, args[1])) as i64;
        let end = hr!(get64(cpu, args[2])) as i64;
        Some((start, end))
    } else {
        None
    };
    if let Some(s) = sample(cpu, id) {
        s.media_time = times;
    }
    Some(S_OK)
}

/// The sample `id`'s AM_SAMPLE2_PROPERTIES.
fn properties(s: &MediaSample) -> [u8; PROPERTIES] {
    let mut p = [0u8; PROPERTIES];
    let words: [(usize, u32); 5] = [
        (0, PROPERTIES as u32),
        (4, s.type_flags),
        (8, s.flags),
        (12, s.actual),
        (32, s.stream_id),
    ];
    for (at, v) in words {
        p[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    p[16..24].copy_from_slice(&s.start.to_le_bytes());
    p[24..32].copy_from_slice(&s.stop.to_le_bytes());
    p[36..40].copy_from_slice(&s.media_type.to_le_bytes());
    p[40..44].copy_from_slice(&s.buffer.to_le_bytes());
    p[44..48].copy_from_slice(&s.size.to_le_bytes());
    p
}

/// IMediaSample2::GetProperties(this, size, out).
pub fn get_sample_properties(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(sample_of(cpu, args[0]));
    let n = (args[1] as usize).min(PROPERTIES);
    if n > 0 && (args[2] == 0 || !cpu.bus.guest_probe(args[2], n, true)) {
        return Some(E_POINTER);
    }
    let Some(s) = sample(cpu, id) else {
        return Some(E_POINTER);
    };
    let p = properties(s);
    cpu.bus.guest_write_bytes(args[2], &p[..n]);
    Some(S_OK)
}

/// IMediaSample2::SetProperties(this, size, properties): what is
/// writable of them, up to the buffer, which is the sample's own. The
/// media type stays the sample's.
pub fn set_sample_properties(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(sample_of(cpu, args[0]));
    let n = (args[1] as usize).min(PROPERTIES_WRITABLE);
    if n > 0 && (args[2] == 0 || !cpu.bus.guest_probe(args[2], n, false)) {
        return Some(E_POINTER);
    }
    let Some(s) = sample(cpu, id) else {
        return Some(E_POINTER);
    };
    let mut p = properties(s);
    let mut given = vec![0u8; n];
    cpu.bus.guest_read_bytes(args[2], &mut given);
    p[..n].copy_from_slice(&given);
    let word = |at: usize| u32::from_le_bytes(p[at..at + 4].try_into().unwrap());
    let long = |at: usize| i64::from_le_bytes(p[at..at + 8].try_into().unwrap());
    let Some(s) = sample(cpu, id) else {
        return Some(E_POINTER);
    };
    s.type_flags = word(4);
    s.flags = word(8);
    s.actual = word(12).min(s.size);
    s.start = long(16);
    s.stop = long(24);
    s.stream_id = word(32);
    Some(S_OK)
}

// The clock.

/// CoCreateInstance's CLSID_SystemClock.
pub fn create_clock(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = new_clock(cpu);
    super::give_new(cpu, id, iid, out)
}

/// A new clock with no advises, with one reference.
pub fn new_clock(cpu: &mut Cpu) -> u32 {
    super::add(
        cpu,
        Kind::Clock {
            c: Clock::default(),
        },
    )
}

/// The clock's time: the clock's milliseconds, in 100-nanosecond units.
pub fn now(cpu: &Cpu) -> i64 {
    (cpu.bus.clock.now_micros() / 1000) as i64 * 10_000
}

/// When the clock's time reaches `t`, in the clock's microseconds.
fn micros_at(t: i64) -> u64 {
    (t.max(0) as u64).div_ceil(10_000) * 1000
}

/// IReferenceClock::GetTime(this, out): S_FALSE when the time is the
/// one it gave last.
pub fn clock_get_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], clock));
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    let t = now(cpu);
    let Some(c) = clock(cpu, id) else {
        return Some(E_POINTER);
    };
    let same = std::mem::replace(&mut c.last, t) == t;
    hr!(put64(cpu, args[1], t as u64));
    Some(if same { S_FALSE } else { S_OK })
}

/// A double-word time the program passed as two arguments.
fn time_arg(lo: u32, hi: u32) -> i64 {
    ((hi as u64) << 32 | lo as u64) as i64
}

/// IReferenceClock::AdviseTime(this, base, offset, event, cookie): the
/// event is set at base + offset.
pub fn clock_advise_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let due = time_arg(args[1], args[2]).wrapping_add(time_arg(args[3], args[4]));
    if due <= 0 {
        return Some(E_INVALIDARG);
    }
    hresult(advise(cpu, args[0], args[5], args[6], due, 0, false))
}

/// IReferenceClock::AdvisePeriodic(this, start, period, semaphore,
/// cookie): the semaphore is released at start and each period after.
pub fn clock_advise_periodic(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (start, period) = (time_arg(args[1], args[2]), time_arg(args[3], args[4]));
    if start <= 0 || period <= 0 {
        return Some(E_INVALIDARG);
    }
    hresult(advise(cpu, args[0], args[5], args[6], start, period, true))
}

fn advise(
    cpu: &mut Cpu,
    this: u32,
    handle: u32,
    cookie_at: u32,
    due: i64,
    period: i64,
    semaphore: bool,
) -> Result<u32, u32> {
    let id = of(cpu, this, clock)?;
    if handle == 0 {
        return Err(E_INVALIDARG);
    }
    if cookie_at == 0 {
        return Err(E_POINTER);
    }
    let object = object::resolve(cpu, handle).ok_or(E_INVALIDARG)?;
    let fits = match object::objects(cpu).get(object).map(|o| &o.kind) {
        Some(ObjectKind::Event { .. }) => !semaphore,
        Some(ObjectKind::Semaphore { .. }) => semaphore,
        _ => false,
    };
    if !fits {
        return Err(E_INVALIDARG);
    }
    // The advises are fired on the multimedia thread.
    crate::win32::winmm::callback::ensure_thread(cpu).ok_or(E_OUTOFMEMORY)?;
    let c = clock(cpu, id).ok_or(E_POINTER)?;
    c.next_cookie = c.next_cookie.wrapping_add(1).max(1);
    let cookie = c.next_cookie;
    c.advises.push(Advise {
        cookie,
        object,
        due,
        period,
    });
    object::objects(cpu).hold(object);
    put32(cpu, cookie_at, cookie)?;
    let at = cpu.bus.clock.now_micros();
    fire(cpu, id, at);
    crate::win32::winmm::schedule(cpu);
    Ok(S_OK)
}

/// IReferenceClock::Unadvise(this, cookie): S_FALSE for one it hasn't.
pub fn clock_unadvise(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(of(cpu, args[0], clock));
    let Some(c) = clock(cpu, id) else {
        return Some(E_POINTER);
    };
    let Some(k) = c.advises.iter().position(|a| a.cookie == args[1]) else {
        return Some(S_FALSE);
    };
    let gone = c.advises.remove(k);
    object::objects(cpu).release(gone.object);
    crate::win32::winmm::schedule(cpu);
    Some(S_OK)
}

/// The clock `id`'s advises that are due at `at` (the clock's
/// microseconds) fire.
fn fire(cpu: &mut Cpu, id: u32, at: u64) {
    let t = (at / 1000) as i64 * 10_000;
    let Some(c) = clock(cpu, id) else {
        return;
    };
    let mut events = Vec::new();
    let mut releases = Vec::new();
    c.advises.retain_mut(|a| {
        if a.due > t {
            return true;
        }
        if a.period == 0 {
            events.push(a.object);
            return false;
        }
        let periods = (t - a.due) / a.period + 1;
        a.due += periods * a.period;
        releases.push((a.object, periods as u32));
        true
    });
    for e in events {
        super::set_event(cpu, e);
        object::objects(cpu).release(e);
    }
    for (s, n) in releases {
        if let Some(o) = object::objects(cpu).get_mut(s)
            && let ObjectKind::Semaphore { count, max } = &mut o.kind
        {
            *count = count.saturating_add(n).min(*max);
        }
        thread::wake(cpu);
    }
}

/// The process `pid`'s clocks at `now`: their due advises fire.
pub fn poll(cpu: &mut Cpu, pid: u32, now: u64) {
    let clocks: Vec<u32> = super::state(cpu)
        .objects
        .iter()
        .filter(|o| o.pid == pid && matches!(o.kind, Kind::Clock { .. }))
        .map(|o| o.id)
        .collect();
    for id in clocks {
        fire(cpu, id, now);
    }
}

/// When the process `pid`'s clocks next have an advise due.
pub fn next_due(s: &super::State, pid: Option<u32>) -> Option<u64> {
    s.objects
        .iter()
        .filter(|o| pid.is_none_or(|p| o.pid == p))
        .filter_map(|o| match &o.kind {
            Kind::Clock { c } => c.advises.iter().map(|a| micros_at(a.due)).min(),
            _ => None,
        })
        .min()
}

/// The clock `id` goes: its advises let go of their objects.
pub fn close_clock(cpu: &mut Cpu, id: u32) {
    let Some(c) = clock(cpu, id) else {
        return;
    };
    for a in std::mem::take(&mut c.advises) {
        object::objects(cpu).release(a.object);
    }
}

crate::state_fields!(Allocator {
    props,
    committed,
    decommit_queued,
    samples,
    pages,
    freed
});
crate::state_fields!(MediaSample {
    allocator,
    buffer,
    size,
    actual,
    flags,
    type_flags,
    start,
    stop,
    stream_id,
    media_time,
    media_type,
    out
});
crate::state_fields!(Clock {
    advises,
    next_cookie,
    last
});
crate::state_fields!(Advise {
    cookie,
    object,
    due,
    period
});
