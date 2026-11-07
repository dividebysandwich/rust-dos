//! The program's own filters in a filter graph (graph.rs): AddFilter
//! joins one to the graph, and RenderFile connects the file's streams to
//! their input pins, as DirectShow's graph builder renders each output
//! pin of the source with a filter already in the graph before it adds
//! one of its own. The MPEG video decoder's output pin gives the movie's
//! pictures, and the sound's output pin gives its PCM, each in media
//! samples through IMemInputPin::Receive, as a program's video and sound
//! renderers take them on Windows. The graph moves the program's filters
//! through Pause, Run and Stop with itself, gives them its reference
//! clock (sample.rs), and passes on the events they tell it through
//! IMediaEventSink.
//!
//! How it is done follows Wine's quartz and strmbase (filtergraph.c,
//! strmbase's pin.c, quartz_transform.c), which follow Windows'. An
//! output pin tries the input pin's own media types first, then its own:
//! the video decoder's in its order, YV12, YUY2, UYVY, RGB24, RGB32,
//! RGB565 and RGB555, and the sound's one PCM type with the file's
//! WAVEFORMATEX. A type is tried with ReceiveConnection; the pin that
//! takes it gives its IMemInputPin and its allocator (GetAllocator, or
//! the graph's own memory allocator), whose buffers are sized for a
//! picture, or for a piece of the sound, with GetAllocatorRequirements,
//! and NotifyAllocator ends it. The pictures connect first, then the
//! sound. A stream no filter of the program's takes plays through the
//! graph's MPEGVideo device: the pictures in its window, which it opens
//! without when they are connected, and the sound on the sound device,
//! which it leaves silent when the sound is connected.
//!
//! Every call into the program goes through a task (task.rs): the
//! connection's steps (`connect_step`), lists of calls the graph's
//! methods make (`list_step`), and each call of a stream's streaming
//! thread (`stream_thread`), a thread of the process's own that delivers
//! the stream's samples one after another while the graph is paused or
//! runs, as each output pin of Windows' splitters has one. It waits for
//! a free sample of the allocator, fills it with the next picture or the
//! next piece of the sound, timed from the segment's start, and hands it
//! over; the renderer holds it until its time on the clock comes. After
//! the last one it calls EndOfStream, and the renderer then tells the
//! graph EC_COMPLETE.
//!
//! Wine's quartz is under the LGPL 2.1 or later; what is taken from it
//! here is its behaviour, with credit to its authors.

use super::graph::{graph, graph_of};
use super::sample;
use super::{
    E_FAIL, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, Kind, S_FALSE, S_OK, VFW_E_ALREADY_CONNECTED,
    VFW_E_NOT_CONNECTED, VFW_E_NOT_FOUND, VFW_E_NOT_STOPPED, VFW_S_DUPLICATE_NAME, put32,
};
use crate::cpu::Cpu;
use crate::win32::com::Guid;
use crate::win32::gdi32::surface::dib_stride;
use crate::win32::ole32::stream::method;
use crate::win32::task::{self, Next, Task};
use crate::win32::thread::{self, Wait};
use crate::win32::winmm::mci::media::{self, Media, Pictures};
use std::sync::Arc;

/// IMediaControl's states, which IMediaFilter's are too.
pub const STATE_STOPPED: u32 = 0;
pub const STATE_PAUSED: u32 = 1;
pub const STATE_RUNNING: u32 = 2;

/// PIN_DIRECTION's.
const PINDIR_INPUT: u32 = 0;
const PINDIR_OUTPUT: u32 = 1;

/// The methods' places in their vtables.
const RELEASE: u32 = 2;
const ADD_REF: u32 = 1;
const QUERY_INTERFACE: u32 = 0;
const FILTER_STOP: u32 = 4;
const FILTER_PAUSE: u32 = 5;
const FILTER_RUN: u32 = 6;
const FILTER_SET_SYNC_SOURCE: u32 = 8;
const FILTER_ENUM_PINS: u32 = 10;
const FILTER_JOIN_FILTER_GRAPH: u32 = 13;
const ENUM_NEXT: u32 = 3;
const PIN_RECEIVE_CONNECTION: u32 = 4;
const PIN_DISCONNECT: u32 = 5;
const PIN_CONNECTED_TO: u32 = 6;
const PIN_QUERY_DIRECTION: u32 = 9;
const PIN_ENUM_MEDIA_TYPES: u32 = 12;
const PIN_END_OF_STREAM: u32 = 14;
const PIN_NEW_SEGMENT: u32 = 17;
const INPUT_GET_ALLOCATOR: u32 = 3;
const INPUT_NOTIFY_ALLOCATOR: u32 = 4;
const INPUT_GET_ALLOCATOR_REQUIREMENTS: u32 = 5;
const INPUT_RECEIVE: u32 = 6;

/// The decoder's name and its output pin's, as Windows' MPEG video
/// decoder has them.
const DECODER_NAME: &str = "MPEG Video Decoder";
const PIN_NAME: &str = "Output";

/// MEDIATYPE_Video and FORMAT_VideoInfo.
const MEDIATYPE_VIDEO: Guid = Guid::new(
    0x7364_6976,
    0,
    0x10,
    [0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71],
);
const FORMAT_VIDEO_INFO: Guid = Guid::new(
    0x0558_9F80,
    0xC356,
    0x11CE,
    [0xBF, 0x01, 0x00, 0xAA, 0x00, 0x55, 0x59, 0x5A],
);

/// MEDIATYPE_Audio, MEDIASUBTYPE_PCM and FORMAT_WaveFormatEx.
const MEDIATYPE_AUDIO: Guid = fourcc_subtype(0x7364_7561);
const MEDIASUBTYPE_PCM: Guid = fourcc_subtype(1);
const FORMAT_WAVE_FORMAT_EX: Guid = Guid::new(
    0x0558_9F81,
    0xC356,
    0x11CE,
    [0xBF, 0x01, 0x00, 0xAA, 0x00, 0x55, 0x59, 0x5A],
);

/// The filter whose output pin gives the sound, and the pin's name, as
/// Windows has them: the AVI splitter's second stream for an AVI file
/// (its PCM, or the ACM wrapper's from its ADPCM, under the splitter's
/// name here), the MPEG audio decoder's output for an MPEG one.
const AVI_SPLITTER: (&str, &str) = ("AVI Splitter", "Stream 01");
const MPEG_AUDIO: (&str, &str) = ("MPEG Audio Decoder", "XForm Out");

/// A sample of the sound holds a picture's time of it, as an AVI file
/// interleaves a chunk of sound a picture and the splitter gives each
/// chunk as a sample; this long in a file without pictures.
const AUDIO_SAMPLE_MS: u32 = 100;

/// The subtype {fourcc-0000-0010-8000-00AA00389B71} of a FOURCC.
const fn fourcc_subtype(fourcc: u32) -> Guid {
    Guid::new(fourcc, 0, 0x10, [0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71])
}

/// The RGB subtypes, {E436EBxx-524F-11CE-9F53-0020AF0BA770}.
const fn rgb_subtype(d1: u32) -> Guid {
    Guid::new(
        d1,
        0x524F,
        0x11CE,
        [0x9F, 0x53, 0x00, 0x20, 0xAF, 0x0B, 0xA7, 0x70],
    )
}

const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;

/// AM_MEDIA_TYPE's size, VIDEOINFOHEADER's, and the most of a format
/// block of the program's that the connection copies.
const MEDIA_TYPE: usize = 72;
const VIDEO_INFO: usize = 88;
const MAX_FORMAT: usize = 1200;

/// A kind of picture the decoder gives.
struct Format {
    subtype: Guid,
    compression: u32,
    bits: u16,
    /// RGB565's masks, after the header.
    masks: Option<[u32; 3]>,
}

/// The decoder's output types, in the order it offers them (Wine's
/// quartz_transform.c).
static FORMATS: [Format; 7] = [
    Format {
        subtype: fourcc_subtype(0x3231_5659),
        compression: 0x3231_5659,
        bits: 12,
        masks: None,
    },
    Format {
        subtype: fourcc_subtype(0x3259_5559),
        compression: 0x3259_5559,
        bits: 16,
        masks: None,
    },
    Format {
        subtype: fourcc_subtype(0x5956_5955),
        compression: 0x5956_5955,
        bits: 16,
        masks: None,
    },
    Format {
        subtype: rgb_subtype(0xE436_EB7D),
        compression: BI_RGB,
        bits: 24,
        masks: None,
    },
    Format {
        subtype: rgb_subtype(0xE436_EB7E),
        compression: BI_RGB,
        bits: 32,
        masks: None,
    },
    Format {
        subtype: rgb_subtype(0xE436_EB7B),
        compression: BI_BITFIELDS,
        bits: 16,
        masks: Some([0xF800, 0x07E0, 0x001F]),
    },
    Format {
        subtype: rgb_subtype(0xE436_EB7C),
        compression: BI_RGB,
        bits: 16,
        masks: None,
    },
];

const YV12: usize = 0;
const YUY2: usize = 1;
const UYVY: usize = 2;
const RGB24: usize = 3;
const RGB32: usize = 4;
const RGB565: usize = 5;

/// A program's filter in a graph: its IBaseFilter, which the graph
/// holds a reference on, and its name in the graph.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub iface: u32,
    pub name: String,
}

/// A stream of the file that a link carries: the pictures or the sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Video,
    Audio,
}

const STREAMS: [Stream; 2] = [Stream::Video, Stream::Audio];

impl Stream {
    /// Its bit in a mask of streams.
    fn bit(self) -> u32 {
        match self {
            Stream::Video => 1,
            Stream::Audio => 2,
        }
    }
}

/// An output pin's connection to a program's input pin, and its stream.
#[derive(Clone, Debug, Default)]
pub struct Link {
    /// Whether it carries the sound rather than the pictures.
    pub audio: bool,
    /// The program's filter (its IBaseFilter), its input pin and the
    /// pin's IMemInputPin, which the link holds references on.
    pub filter: u32,
    pub pin: u32,
    pub input: u32,
    /// The allocator (by its object), the filter that gives the stream
    /// and its output pin, which the link holds references on.
    pub allocator: u32,
    pub decoder: u32,
    pub out_pin: u32,
    /// The connection's media type: AM_MEDIA_TYPE without its format
    /// block's pointer, and the block. The pictures' place in `FORMATS`,
    /// and whether their RGB rows go from the bottom up.
    pub head: Vec<u8>,
    pub format: Vec<u8>,
    pub kind: u32,
    pub bottom_up: bool,
    /// The file.
    pub file: String,
    /// The streaming thread and the auto-reset event that wakes it, by
    /// its object; 0 until the graph first pauses.
    pub tid: u32,
    pub wake: u32,
    /// The next picture, or the next byte of the sound, and the segment
    /// the stream plays: where it starts and stops, in milliseconds.
    pub next: u32,
    pub from_ms: u32,
    pub to_ms: u32,
    /// Whether it streams (the graph is paused or runs), whether it sent
    /// EndOfStream, and whether the pin refused a sample, after which it
    /// sends no more until the graph stops.
    pub active: bool,
    pub ended: bool,
    pub refused: bool,
    /// Whether NewSegment is still to be sent, and the next sample is a
    /// discontinuity.
    pub segment: bool,
    pub discontinuity: bool,
    /// Counts the stream's restarts, so that a call that returns after
    /// one doesn't count for the stream that follows.
    pub generation: u32,
    /// The file, read again after a restore, and its decoder.
    pub media: Option<Arc<Media>>,
    pub pictures: Pictures,
}

impl Link {
    pub fn stream(&self) -> Stream {
        if self.audio {
            Stream::Audio
        } else {
            Stream::Video
        }
    }
}

/// The filter that gives a stream (the MPEG video decoder, or the sound's
/// splitter or decoder), which is only seen through its pin: its graph.
#[derive(Clone, Debug, Default)]
pub struct Decoder {
    pub graph: u32,
}

/// That filter's output pin: its graph.
#[derive(Clone, Debug, Default)]
pub struct Pin {
    pub graph: u32,
}

/// The graph of the decoder or pin `this`.
fn owner_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    let id = super::object_of(cpu, this).ok_or(E_POINTER)?;
    match super::kind(cpu, id) {
        Some(Kind::Decoder { d }) => Ok(d.graph),
        Some(Kind::Pin { p }) => Ok(p.graph),
        _ => Err(E_POINTER),
    }
}

/// The graph `id`'s link of the stream `s`.
fn link(cpu: &mut Cpu, id: u32, s: Stream) -> Option<&mut Link> {
    let g = graph(cpu, id)?;
    match s {
        Stream::Video => g.link.as_mut(),
        Stream::Audio => g.sound.as_mut(),
    }
}

/// The link, made or being made, whose decoder or output pin is the
/// object `object` of the graph `id`.
fn link_of(cpu: &mut Cpu, id: u32, object: u32) -> Option<&mut Link> {
    let g = graph(cpu, id)?;
    [&mut g.link, &mut g.sound, &mut g.connecting]
        .into_iter()
        .filter_map(|l| l.as_mut())
        .find(|l| l.out_pin == object || l.decoder == object)
}

/// Whether the graph `id` has filters of the program's in it.
pub fn has_filters(cpu: &mut Cpu, id: u32) -> bool {
    graph(cpu, id)
        .is_some_and(|g| !g.filters.is_empty() || g.link.is_some() || g.sound.is_some())
}

/// Whether the graph `id`'s pictures go to a program's filter.
pub fn linked(cpu: &mut Cpu, id: u32) -> bool {
    graph(cpu, id).is_some_and(|g| g.link.is_some())
}

/// Whether the graph `id`'s sound goes to a program's filter.
pub fn sound_linked(cpu: &mut Cpu, id: u32) -> bool {
    graph(cpu, id).is_some_and(|g| g.sound.is_some())
}

/// How many of the graph `id`'s streams go to a program's filter.
pub fn links(cpu: &mut Cpu, id: u32) -> u32 {
    linked(cpu, id) as u32 + sound_linked(cpu, id) as u32
}

/// The streams of the graph `id` whose link has the program's filter
/// `filter`, as a mask.
fn links_to(cpu: &mut Cpu, id: u32, filter: u32) -> u32 {
    STREAMS
        .into_iter()
        .filter(|&s| link(cpu, id, s).is_some_and(|l| l.filter == filter))
        .fold(0, |mask, s| mask | s.bit())
}

// The calls into the program.

/// Which of the tasks of `TASKS`' DSHOW entry runs: its first variable.
const JOB_CONNECT: u32 = 1;
const JOB_LIST: u32 = 2;
const JOB_STREAM: u32 = 3;

/// The DSHOW task's steps (task.rs).
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    match t.var(0) {
        JOB_CONNECT => connect_step(cpu, t, value),
        JOB_LIST => list_step(cpu, t, value),
        JOB_STREAM => stream_step(cpu, t, value),
        _ => Next::Done(E_FAIL),
    }
}

/// What a list of calls does when it is done (`list_step`).
const THEN_DONE: u32 = 0;
const THEN_STATE: u32 = 1;
const THEN_ADDED: u32 = 2;
const THEN_CLOSED: u32 = 3;
const THEN_UNLINKED: u32 = 4;

/// Both streams, as a mask (`Stream::bit`).
const ALL_STREAMS: u32 = 3;

/// A call's flags in a list: the list stops when it fails, with its
/// HRESULT; the list's result is what it returns.
const STOP_ON_FAIL: u32 = 0x100;
const TAKE_RESULT: u32 = 0x200;

/// The variables a list has before its calls: the job, where the next
/// call is, the result, what is done after, and its three arguments.
const LIST_HEAD: usize = 7;

/// A list of calls of the program's methods, made one after another.
struct List {
    vars: Vec<u32>,
}

impl List {
    fn new(result: u32, then: u32, arg: u32, arg2: u32, arg3: u32) -> List {
        List {
            vars: vec![JOB_LIST, LIST_HEAD as u32, result, then, arg, arg2, arg3],
        }
    }

    /// The method `index` of the interface `this`, with `args` after
    /// `this`. An interface that can't be read is skipped.
    fn call(&mut self, cpu: &mut Cpu, this: u32, index: u32, args: &[u32], flags: u32) {
        let Some(target) = method(cpu, this, index) else {
            return;
        };
        self.vars.push(flags | (args.len() as u32 + 1));
        self.vars.push(target);
        self.vars.push(this);
        self.vars.extend_from_slice(args);
    }

    fn release(&mut self, cpu: &mut Cpu, this: u32) {
        if this != 0 {
            self.call(cpu, this, RELEASE, &[], 0);
        }
    }

    /// Make the calls, from the running function: what it returns, or
    /// None while the calls are made.
    fn start(self, cpu: &mut Cpu, scratch: u32) -> Option<u32> {
        task::start(cpu, task::DSHOW, self.vars, scratch)
    }
}

fn list_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step == 1 {
        let at = t.var(1) as usize;
        let head = t.var(at);
        t.set(1, (at + 2 + (head & 0xFF) as usize) as u32);
        if head & TAKE_RESULT != 0 {
            t.set(2, value);
        }
        if head & STOP_ON_FAIL != 0 && (value as i32) < 0 {
            t.set(2, value);
            return then(cpu, t, true);
        }
    }
    let at = t.var(1) as usize;
    if at >= t.vars.len() {
        return then(cpu, t, false);
    }
    let n = (t.var(at) & 0xFF) as usize;
    let target = t.var(at + 1);
    let end = (at + 2 + n).min(t.vars.len());
    let args = t.vars[at + 2..end].to_vec();
    Next::call(target, &args, 1)
}

/// A list of calls is done (or stopped at a call that failed).
fn then(cpu: &mut Cpu, t: &mut Task, failed: bool) -> Next {
    let (result, arg, arg2) = (t.var(2), t.var(4), t.var(5));
    match t.var(3) {
        THEN_STATE => {
            moved(cpu, arg, arg2);
            t.retry(cpu);
            Next::Stop
        }
        THEN_ADDED => {
            let block = t.var(6);
            let name = crate::win32::dmusic::read_wide(cpu, block, 128);
            crate::win32::ole32::task_free(cpu, block);
            if !failed && let Some(g) = graph(cpu, arg) {
                g.filters.push(Filter { iface: arg2, name });
            }
            Next::Done(result)
        }
        THEN_CLOSED => {
            unlink(cpu, arg, ALL_STREAMS);
            Next::Done(super::drop_ref(cpu, arg))
        }
        THEN_UNLINKED => {
            unlink(cpu, arg, arg2);
            Next::Done(result)
        }
        _ => Next::Done(result),
    }
}

// IFilterGraph's methods for the program's filters.

/// The name the graph gives a filter: the one asked for, or one made
/// from a number when it is NULL or taken (Wine's filtergraph.c), and
/// whether it was taken.
fn name_for(cpu: &mut Cpu, id: u32, asked: Option<String>) -> (String, bool) {
    let Some(g) = graph(cpu, id) else {
        return (String::new(), false);
    };
    let taken = |g: &super::graph::Graph, n: &str| g.filters.iter().any(|f| f.name == n);
    if let Some(name) = &asked
        && !taken(g, name)
    {
        return (name.clone(), false);
    }
    loop {
        let n = g.name_index;
        g.name_index = g.name_index.wrapping_add(1);
        let name = match &asked {
            Some(a) => format!("{a} {n:04}"),
            None => format!("{n:04}"),
        };
        if !taken(g, &name) {
            return (name, asked.is_some());
        }
    }
}

/// IFilterGraph::AddFilter(this, filter, name): the filter joins the
/// graph (JoinFilterGraph), which holds a reference on it, and gets the
/// graph's clock if it has one. The name the filter is given is in a
/// block of the task heap while it joins, which filters copy.
pub fn add_filter(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, filter, name] = args[..] else {
        return Some(super::E_INVALIDARG);
    };
    let id = hr!(graph_of(cpu, this));
    if filter == 0 {
        return Some(E_POINTER);
    }
    if graph(cpu, id).is_some_and(|g| g.filters.iter().any(|f| f.iface == filter)) {
        // Windows' graph takes a filter in once.
        return Some(super::E_INVALIDARG);
    }
    let asked = (name != 0).then(|| crate::win32::dmusic::read_wide(cpu, name, 127));
    let (name, renamed) = name_for(cpu, id, asked);
    let Some(me) = super::block(cpu, id, super::GRAPH_BUILDER) else {
        return Some(E_OUTOFMEMORY);
    };
    let clock = graph(cpu, id)
        .filter(|g| g.clock_given)
        .map_or(0, |g| g.clock);
    let clock = if clock != 0 {
        super::block(cpu, clock, super::CLOCK).unwrap_or(0)
    } else {
        0
    };
    let block = crate::win32::ole32::task_alloc(cpu, 256);
    if block == 0 {
        return Some(E_OUTOFMEMORY);
    }
    crate::win32::dmusic::write_wide(cpu, block, &name, 128);
    let result = if renamed { VFW_S_DUPLICATE_NAME } else { S_OK };
    let mut list = List::new(result, THEN_ADDED, id, filter, block);
    list.call(
        cpu,
        filter,
        FILTER_JOIN_FILTER_GRAPH,
        &[me, block],
        STOP_ON_FAIL,
    );
    list.call(cpu, filter, ADD_REF, &[], 0);
    if clock != 0 {
        list.call(cpu, filter, FILTER_SET_SYNC_SOURCE, &[clock], 0);
    }
    list.start(cpu, 0)
}

/// IFilterGraph::RemoveFilter(this, filter): the filter leaves the
/// graph, its connections with the streams undone. A filter of a graph
/// that isn't stopped is stopped first (paused first when it runs), as
/// Wine's filtergraph.c does, so that its pins can disconnect. E_FAIL
/// for one that isn't in it.
pub fn remove_filter(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(graph_of(cpu, args[0]));
    let filter = args[1];
    let Some(g) = graph(cpu, id) else {
        return Some(E_POINTER);
    };
    let Some(k) = g.filters.iter().position(|f| f.iface == filter) else {
        return Some(E_FAIL);
    };
    g.filters.remove(k);
    let state = g.state;
    let linked = links_to(cpu, id, filter);
    let then = if linked != 0 { THEN_UNLINKED } else { THEN_DONE };
    let mut list = List::new(S_OK, then, id, linked, 0);
    if state == STATE_RUNNING {
        list.call(cpu, filter, FILTER_PAUSE, &[], 0);
    }
    if state != STATE_STOPPED {
        list.call(cpu, filter, FILTER_STOP, &[], 0);
    }
    disconnect_calls(cpu, &mut list, id, linked);
    list.call(cpu, filter, FILTER_JOIN_FILTER_GRAPH, &[0, 0], 0);
    list.release(cpu, filter);
    list.start(cpu, 0)
}

/// IFilterGraph::FindFilterByName(this, name, out).
pub fn find_filter_by_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, name, out] = args[..] else {
        return Some(super::E_INVALIDARG);
    };
    let id = hr!(graph_of(cpu, this));
    hr!(put32(cpu, out, 0));
    if name == 0 {
        return Some(E_POINTER);
    }
    let name = crate::win32::dmusic::read_wide(cpu, name, 128);
    let found = graph(cpu, id)
        .and_then(|g| g.filters.iter().find(|f| f.name == name))
        .map(|f| f.iface);
    let Some(filter) = found else {
        return Some(VFW_E_NOT_FOUND);
    };
    cpu.bus.guest_write_32(out, filter);
    let mut list = List::new(S_OK, THEN_DONE, id, 0, 0);
    list.call(cpu, filter, ADD_REF, &[], 0);
    list.start(cpu, 0)
}

/// The calls that undo the graph `id`'s links of the streams `mask`:
/// their streams stop, the program's pins disconnect, and the links let
/// go of their references on the pins. The links themselves go after
/// the calls (`unlink`): each pin lets go of the output pin as it
/// disconnects.
fn disconnect_calls(cpu: &mut Cpu, list: &mut List, id: u32, mask: u32) {
    for s in STREAMS {
        if mask & s.bit() == 0 {
            continue;
        }
        halt(cpu, id, s);
        let Some(l) = link(cpu, id, s) else {
            continue;
        };
        let (pin, input) = (std::mem::take(&mut l.pin), std::mem::take(&mut l.input));
        list.call(cpu, pin, PIN_DISCONNECT, &[], 0);
        list.release(cpu, input);
        list.release(cpu, pin);
    }
}

/// The graph `id`'s stream `s` stops: its output pin decommits the
/// allocator, and the streaming thread waits.
fn halt(cpu: &mut Cpu, id: u32, s: Stream) {
    let Some(l) = link(cpu, id, s) else {
        return;
    };
    l.active = false;
    l.generation = l.generation.wrapping_add(1);
    let (a, wake) = (l.allocator, l.wake);
    sample::decommit_now(cpu, a);
    super::set_event(cpu, wake);
}

/// The link `l` is gone: its streaming thread ends (it finds no link
/// when it wakes), and the allocator, the decoder and its pin are let
/// go.
fn drop_link(cpu: &mut Cpu, l: Link) {
    sample::decommit_now(cpu, l.allocator);
    for id in [l.allocator, l.out_pin, l.decoder] {
        if id != 0 {
            super::drop_ref(cpu, id);
        }
    }
    if l.wake != 0 {
        super::set_event(cpu, l.wake);
        crate::win32::object::objects(cpu).release(l.wake);
    }
}

/// The graph `id` lets go of its links of the streams `mask`, after the
/// calls that undo them.
fn unlink(cpu: &mut Cpu, id: u32, mask: u32) {
    for s in STREAMS {
        if mask & s.bit() == 0 {
            continue;
        }
        let taken = graph(cpu, id).and_then(|g| match s {
            Stream::Video => g.link.take(),
            Stream::Audio => g.sound.take(),
        });
        if let Some(l) = taken {
            drop_link(cpu, l);
        }
    }
}

/// The graph's last reference goes, with filters of the program's in
/// it: they stop, the link is undone, and they leave the graph, each
/// let go of, before the graph goes (Wine's filtergraph.c). What
/// Release returns.
pub fn close(cpu: &mut Cpu, id: u32) -> Option<u32> {
    let Some(g) = graph(cpu, id) else {
        return Some(0);
    };
    let filters = std::mem::take(&mut g.filters);
    let running = g.state != STATE_STOPPED;
    let mut list = List::new(0, THEN_CLOSED, id, 0, 0);
    if running {
        for f in &filters {
            list.call(cpu, f.iface, FILTER_STOP, &[], 0);
        }
    }
    disconnect_calls(cpu, &mut list, id, ALL_STREAMS);
    for f in &filters {
        list.call(cpu, f.iface, FILTER_JOIN_FILTER_GRAPH, &[0, 0], 0);
        list.release(cpu, f.iface);
    }
    list.start(cpu, 0)
}

/// The graph `id` is gone without the calls (its process ended, or its
/// count reached 0 on the system's side): its links go.
pub fn forget(cpu: &mut Cpu, id: u32) {
    unlink(cpu, id, ALL_STREAMS);
    let clock = graph(cpu, id).map_or(0, |g| std::mem::take(&mut g.clock));
    if clock != 0 {
        super::drop_ref(cpu, clock);
    }
}

// The graph's state changes.

/// Move the program's filters in the graph `id` to `state`, as the
/// graph does before it changes its own: from the renderers up, a Run
/// from Stopped going through Pause first, the clock given to every
/// filter the first time the graph leaves Stopped. Each Run counts the
/// renderers' EC_COMPLETE again (Wine's `graph_start`), since a renderer
/// at its stream's end tells it again when it runs. None while the calls
/// are made, after which the method runs again with `filters_state`
/// set; Some(()) when there is nothing to call.
pub fn change_state(cpu: &mut Cpu, id: u32, state: u32) -> Option<()> {
    let g = graph(cpu, id)?;
    if g.filters_state == Some(state) || !has_filters(cpu, id) {
        return Some(());
    }
    let g = graph(cpu, id)?;
    let from = g.state;
    let filters: Vec<u32> = g.filters.iter().map(|f| f.iface).collect();
    let mut list = List::new(S_OK, THEN_STATE, id, state, 0);
    if from == STATE_STOPPED && state != STATE_STOPPED {
        give_clock(cpu, id, &mut list, &filters);
    }
    if state == STATE_RUNNING
        && let Some(g) = graph(cpu, id)
    {
        g.completes = 0;
    }
    let now = sample::now(cpu);
    let g = graph(cpu, id)?;
    match state {
        STATE_RUNNING => {
            if from == STATE_STOPPED {
                for &f in &filters {
                    list.call(cpu, f, FILTER_PAUSE, &[], 0);
                }
            }
            let g = graph(cpu, id)?;
            g.start = now - g.elapsed;
            let start = g.start as u64;
            for &f in &filters {
                list.call(cpu, f, FILTER_RUN, &[start as u32, (start >> 32) as u32], 0);
            }
        }
        STATE_PAUSED => {
            if from == STATE_RUNNING {
                g.elapsed = now - g.start;
            }
            for &f in &filters {
                list.call(cpu, f, FILTER_PAUSE, &[], 0);
            }
        }
        _ => {
            g.elapsed = 0;
            for &f in &filters {
                list.call(cpu, f, FILTER_STOP, &[], 0);
            }
        }
    }
    if list.vars.len() == LIST_HEAD {
        moved(cpu, id, state);
        return Some(());
    }
    // The method runs again when the calls are done.
    if let Some(v) = list.start(cpu, 0) {
        cpu.set_eax(v);
    }
    None
}

/// The graph's clock goes to its filters (SetSyncSource) the first time
/// it leaves Stopped: the system clock, made then.
fn give_clock(cpu: &mut Cpu, id: u32, list: &mut List, filters: &[u32]) {
    let Some(g) = graph(cpu, id) else {
        return;
    };
    if g.clock_given {
        return;
    }
    g.clock_given = true;
    if g.clock == 0 {
        let clock = sample::new_clock(cpu);
        if let Some(g) = graph(cpu, id) {
            g.clock = clock;
        }
    }
    let clock = graph(cpu, id).map_or(0, |g| g.clock);
    let Some(at) = super::block(cpu, clock, super::CLOCK) else {
        return;
    };
    for &f in filters {
        list.call(cpu, f, FILTER_SET_SYNC_SOURCE, &[at], 0);
    }
}

/// The program's filters in the graph `id` moved to `state`: the
/// output pins do too, after them. Each commits its allocator and its
/// stream starts where the graph's device is when it leaves Stopped,
/// and they stop when it stops.
fn moved(cpu: &mut Cpu, id: u32, state: u32) {
    let Some(g) = graph(cpu, id) else {
        return;
    };
    g.filters_state = Some(state);
    let from = g.state;
    if (from == STATE_STOPPED) == (state == STATE_STOPPED) {
        return;
    }
    let (pos, to) = match crate::win32::winmm::mci::graph_device(cpu, id) {
        Some(i) => (
            crate::win32::winmm::mci::video::position_ms(cpu, i),
            crate::win32::winmm::mci::device(cpu, i).video.to,
        ),
        None => (0, 0),
    };
    for s in STREAMS {
        if link(cpu, id, s).is_none() {
            continue;
        }
        if state == STATE_STOPPED {
            halt(cpu, id, s);
            continue;
        }
        let a = link(cpu, id, s).map_or(0, |l| l.allocator);
        if sample::commit_now(cpu, a) != S_OK {
            continue;
        }
        let media = stream_media(cpu, id, s);
        let Some(l) = link(cpu, id, s) else {
            continue;
        };
        l.next = media.as_ref().map_or(0, |m| first_sample(m, s, pos));
        l.from_ms = pos;
        l.to_ms = to;
        l.active = true;
        l.ended = false;
        l.refused = false;
        l.segment = true;
        l.discontinuity = true;
        l.generation = l.generation.wrapping_add(1);
        if l.tid == 0 {
            start_thread(cpu, id, s);
        }
        let wake = link(cpu, id, s).map_or(0, |l| l.wake);
        super::set_event(cpu, wake);
    }
}

/// The graph `id`'s play moved to `ms` (IMediaSeeking): the streams go
/// on from there, as a new segment.
pub fn seeked(cpu: &mut Cpu, id: u32, ms: u32, to: u32) {
    for s in STREAMS {
        let media = stream_media(cpu, id, s);
        let Some(l) = link(cpu, id, s) else {
            continue;
        };
        l.next = media.as_ref().map_or(0, |m| first_sample(m, s, ms));
        l.from_ms = ms;
        l.to_ms = to;
        l.ended = false;
        l.refused = false;
        l.segment = true;
        l.discontinuity = true;
        l.generation = l.generation.wrapping_add(1);
        let wake = l.wake;
        super::set_event(cpu, wake);
    }
}

/// Where the stream `s` of a segment from `ms` starts: the picture shown
/// there, or the byte of the sound.
fn first_sample(m: &Media, s: Stream, ms: u32) -> u32 {
    match s {
        Stream::Video if ms == 0 => 0,
        Stream::Video => m.frame_at(ms as u64),
        Stream::Audio => sound_byte(m, ms),
    }
}

/// The byte of the sound of `m` played at `ms`, at a block's start.
fn sound_byte(m: &Media, ms: u32) -> u32 {
    let Some((f, data)) = &m.audio else {
        return 0;
    };
    let block = f.block() as u64;
    let at = ms as u64 * f.bytes_per_second() as u64 / 1000 / block * block;
    at.min(data.len() as u64) as u32
}

/// The bytes of the sound a sample holds (`AUDIO_SAMPLE_MS`), whole
/// blocks, at least one.
fn sound_sample_bytes(m: &Media) -> u32 {
    let Some((f, _)) = &m.audio else {
        return 0;
    };
    let (num, den) = if m.frames > 0 {
        (m.rate.0.max(1) as u64, m.rate.1 as u64)
    } else {
        (1000, AUDIO_SAMPLE_MS as u64)
    };
    let block = f.block() as u64;
    let bytes = f.bytes_per_second() as u64 * den / num / block * block;
    bytes.max(block) as u32
}

/// Start the stream `s`'s streaming thread, with the event that wakes
/// it.
fn start_thread(cpu: &mut Cpu, id: u32, s: Stream) {
    let Some(dll) = crate::win32::stubs::find_dll(super::QUARTZ.name) else {
        return;
    };
    let Some(index) = super::QUARTZ
        .apis
        .iter()
        .position(|a| a.name == "StreamThread")
    else {
        return;
    };
    let start = crate::win32::stubs::stub_address(dll, index);
    let wake = crate::win32::object::objects(cpu).add(
        crate::win32::object::Kind::Event {
            manual: false,
            signaled: false,
        },
        None,
    );
    let tid = crate::win32::kernel32::create_system_thread(cpu, start).unwrap_or(0);
    match link(cpu, id, s) {
        Some(l) if tid != 0 => {
            l.tid = tid;
            l.wake = wake;
        }
        _ => crate::win32::object::objects(cpu).release(wake),
    }
}

/// The file the graph `id`'s link of the stream `s` streams, read again
/// after a restore.
fn stream_media(cpu: &mut Cpu, id: u32, s: Stream) -> Option<Arc<Media>> {
    media_of(cpu, id, Some(s))
}

/// The file of the connection RenderFile is making in the graph `id`.
fn connecting_media(cpu: &mut Cpu, id: u32) -> Option<Arc<Media>> {
    media_of(cpu, id, None)
}

/// The graph `id`'s link of the stream `s`, or for None the one being
/// made while RenderFile connects.
fn slot(cpu: &mut Cpu, id: u32, s: Option<Stream>) -> Option<&mut Link> {
    match s {
        Some(s) => link(cpu, id, s),
        None => graph(cpu, id)?.connecting.as_mut(),
    }
}

/// The file of a link (`slot`), which a save state doesn't keep: read
/// again when it isn't there, unless another link of the graph's has it.
fn media_of(cpu: &mut Cpu, id: u32, s: Option<Stream>) -> Option<Arc<Media>> {
    let l = slot(cpu, id, s)?;
    if let Some(m) = &l.media {
        return Some(m.clone());
    }
    let file = l.file.clone();
    let g = graph(cpu, id)?;
    let shared = [&g.link, &g.sound, &g.connecting]
        .into_iter()
        .flatten()
        .find(|l| l.file == file && l.media.is_some())
        .and_then(|l| l.media.clone());
    let m = match shared {
        Some(m) => m,
        None => Arc::new(read_media(cpu, &file)?),
    };
    slot(cpu, id, s)?.media = Some(m.clone());
    Some(m)
}

fn read_media(cpu: &mut Cpu, path: &str) -> Option<Media> {
    let bytes = media::read_file(cpu, path)?;
    Media::parse(&bytes, true)
}

// The streaming threads.

/// A streaming thread's loop (a stub that jumps back to its trap): what
/// its stream has to do next, a call of the program's input pin, or a
/// wait until there is something.
pub fn stream_thread(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let tid = crate::win32::user32::current_tid(cpu);
    let found = super::state(cpu)
        .objects
        .iter()
        .find_map(|o| match &o.kind {
            Kind::Graph { g } => [&g.link, &g.sound]
                .into_iter()
                .flatten()
                .find(|l| l.tid == tid)
                .map(|l| (o.id, l.stream())),
            _ => None,
        });
    let Some((id, stream)) = found else {
        // Its link is gone, or a program called the stub.
        thread::exit_current(cpu, 0);
        return None;
    };
    let l = link(cpu, id, stream)?.clone_shallow();
    if !l.active || l.ended || l.refused {
        return wait(cpu, vec![l.wake]);
    }
    if l.segment {
        if let Some(l) = link(cpu, id, stream) {
            l.segment = false;
        }
        let start = l.from_ms as u64 * 10_000;
        let stop = l.to_ms as u64 * 10_000;
        let rate = 1.0f64.to_bits();
        let args = [
            start as u32,
            (start >> 32) as u32,
            stop as u32,
            (stop >> 32) as u32,
            rate as u32,
            (rate >> 32) as u32,
        ];
        return stream_call(cpu, (id, &l), (0, 0), l.pin, PIN_NEW_SEGMENT, &args);
    }
    let Some(m) = stream_media(cpu, id, stream) else {
        return end_stream(cpu, id, &l);
    };
    if l.next >= last_sample(&m, &l) {
        return end_stream(cpu, id, &l);
    }
    let s = match sample::take_buffer(cpu, l.allocator) {
        Ok(Some(s)) => s,
        Ok(None) => {
            let freed = sample::freed_event(cpu, l.allocator);
            return wait(cpu, vec![l.wake, freed]);
        }
        // Decommitted: the graph stopped.
        Err(_) => return wait(cpu, vec![l.wake]),
    };
    let advance = match stream {
        Stream::Video => {
            fill(cpu, id, &l, &m, s);
            1
        }
        Stream::Audio => fill_sound(cpu, &l, &m, s),
    };
    if advance == 0 {
        // A buffer too small for a block of the sound.
        super::drop_ref(cpu, s);
        return end_stream(cpu, id, &l);
    }
    let Some(at) = super::block(cpu, s, super::MEDIA_SAMPLE) else {
        super::drop_ref(cpu, s);
        return wait(cpu, vec![l.wake]);
    };
    stream_call(cpu, (id, &l), (s, advance), l.input, INPUT_RECEIVE, &[at])
}

/// Where the link `l`'s segment of `m` ends: the first picture shown at
/// its stop or after, or the byte of the sound there.
fn last_sample(m: &Media, l: &Link) -> u32 {
    let whole = l.to_ms >= m.length_ms();
    match l.stream() {
        Stream::Video if whole => m.frames,
        Stream::Video => (0..m.frames)
            .find(|&n| m.frame_ms(n) >= l.to_ms)
            .unwrap_or(m.frames),
        Stream::Audio => {
            let len = m.audio.as_ref().map_or(0, |(_, d)| d.len() as u32);
            if whole { len } else { sound_byte(m, l.to_ms) }
        }
    }
}

impl Link {
    /// The link's state without its file, its media type and its
    /// decoder, to read while the link changes.
    fn clone_shallow(&self) -> Link {
        // The fields left are numbers and flags, which copy.
        Link {
            head: Vec::new(),
            format: Vec::new(),
            file: String::new(),
            media: None,
            pictures: Pictures::default(),
            ..*self
        }
    }
}

/// The streaming thread waits for one of `objects`, then loops.
fn wait(cpu: &mut Cpu, objects: Vec<u32>) -> Option<u32> {
    let wait = Wait {
        objects: objects.into_iter().filter(|&o| o != 0).collect(),
        ..Wait::default()
    };
    if thread::wait_now(cpu, &wait).is_some() {
        return Some(0);
    }
    thread::block(cpu, wait);
    None
}

/// The stream's last sample went: EndOfStream.
fn end_stream(cpu: &mut Cpu, id: u32, l: &Link) -> Option<u32> {
    if let Some(l) = link(cpu, id, l.stream()) {
        l.ended = true;
    }
    stream_call(cpu, (id, l), (0, 0), l.pin, PIN_END_OF_STREAM, &[])
}

/// The streaming thread of the graph `id`'s link `l` calls the method
/// `index` of `this` with `args`, for the sample `s` (0 for none), which
/// moves the stream on by `advance` when the pin takes it.
fn stream_call(
    cpu: &mut Cpu,
    (id, l): (u32, &Link),
    (s, advance): (u32, u32),
    this: u32,
    index: u32,
    args: &[u32],
) -> Option<u32> {
    let stream = l.stream();
    let Some(target) = method(cpu, this, index) else {
        if s != 0 {
            super::drop_ref(cpu, s);
        }
        if let Some(l) = link(cpu, id, stream) {
            l.refused = true;
        }
        return Some(0);
    };
    let mut vars = vec![
        JOB_STREAM,
        id,
        l.audio as u32,
        l.generation,
        s,
        advance,
        target,
        this,
    ];
    vars.extend_from_slice(args);
    task::start(cpu, task::DSHOW, vars, 0)
}

fn stream_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step == 0 {
        return Next::call(t.var(6), &t.vars[7..], 1);
    }
    let (id, generation, s, advance) = (t.var(1), t.var(3), t.var(4), t.var(5));
    let stream = if t.var(2) != 0 {
        Stream::Audio
    } else {
        Stream::Video
    };
    if s == 0 {
        return Next::Done(0);
    }
    // The sample comes back to the allocator when the pin has let go of
    // it too.
    super::drop_ref(cpu, s);
    if let Some(l) = link(cpu, id, stream)
        && l.generation == generation
    {
        if value == S_OK {
            l.next += advance;
            l.discontinuity = false;
        } else {
            // S_FALSE or an error: the pin wants no more (strmbase's
            // source pins stop their thread).
            l.refused = true;
        }
    }
    Next::Done(0)
}

/// The flags of a sample of the link `l`: its times are valid, and it
/// starts a picture or a piece of the sound that stands alone.
fn sample_flags(l: &Link) -> u32 {
    let mut flags =
        sample::AM_SAMPLE_TIMEVALID | sample::AM_SAMPLE_STOPVALID | sample::AM_SAMPLE_SPLICEPOINT;
    if l.discontinuity {
        flags |= sample::AM_SAMPLE_DATADISCONTINUITY;
    }
    flags
}

/// Fill the sample `s` with the link's next picture, timed from the
/// segment's start.
fn fill(cpu: &mut Cpu, id: u32, l: &Link, m: &Media, s: u32) {
    let (w, h) = (m.width, m.height);
    let stride = dib_stride(w as i32, 24);
    let mut pictures = link(cpu, id, Stream::Video)
        .map_or_else(Pictures::default, |l| std::mem::take(&mut l.pictures));
    let rows = pictures.rows(m, l.next, stride);
    if let Some(lk) = link(cpu, id, Stream::Video) {
        lk.pictures = pictures;
    }
    let data = rows.map(|rows| convert(l.kind as usize, &rows, w, h, stride, l.bottom_up));
    let from = l.from_ms as i64;
    let start = (m.frame_ms(l.next) as i64 - from).max(0) * 10_000;
    let stop = (m.frame_ms(l.next + 1) as i64 - from).max(0) * 10_000;
    let Some(Kind::MediaSample { s: ms }) = super::kind(cpu, s) else {
        return;
    };
    ms.flags = sample_flags(l);
    ms.start = start;
    ms.stop = stop;
    ms.media_time = Some((l.next as i64, l.next as i64 + 1));
    let (buffer, size) = (ms.buffer, ms.size);
    let len = image_size(l.kind as usize, w, h).min(size);
    ms.actual = len;
    if let Some(data) = data {
        let n = data.len().min(len as usize);
        cpu.bus.guest_write_bytes(buffer, &data[..n]);
    }
}

/// Fill the sample `s` with the link's next piece of the sound, as much
/// as a sample holds (`sound_sample_bytes`) in whole blocks, timed from
/// the segment's start, with media times in the sound's sample frames
/// as the AVI splitter gives them: the bytes it holds.
fn fill_sound(cpu: &mut Cpu, l: &Link, m: &Media, s: u32) -> u32 {
    let Some((f, data)) = &m.audio else {
        return 0;
    };
    let (bps, block) = (f.bytes_per_second() as i64, f.block());
    let last = last_sample(m, l).min(data.len() as u32);
    let Some(Kind::MediaSample { s: ms }) = super::kind(cpu, s) else {
        return 0;
    };
    let room = ms.size / block * block;
    let len = sound_sample_bytes(m)
        .min(room)
        .min(last.saturating_sub(l.next));
    let time = |byte: u32| byte as i64 * 10_000_000 / bps.max(1);
    let from = l.from_ms as i64 * 10_000;
    ms.flags = sample_flags(l);
    ms.start = (time(l.next) - from).max(0);
    ms.stop = (time(l.next + len) - from).max(0);
    let frames = |byte: u32| (byte / block) as i64;
    ms.media_time = Some((frames(l.next), frames(l.next + len)));
    ms.actual = len;
    let buffer = ms.buffer;
    let at = l.next as usize;
    cpu.bus
        .guest_write_bytes(buffer, &data[at..at + len as usize]);
    len
}

// The pictures' formats.

/// The bytes of a `w` by `h` picture of the format `k`.
fn image_size(k: usize, w: u32, h: u32) -> u32 {
    match k {
        YV12 => w * h + 2 * (w.div_ceil(2) * h.div_ceil(2)),
        YUY2 | UYVY => w.div_ceil(2) * 4 * h,
        _ => dib_stride(w as i32, FORMATS[k].bits as u32) as u32 * h,
    }
}

/// BT.601's Y, U and V of a pixel.
fn yuv(b: u8, g: u8, r: u8) -> (u8, u8, u8) {
    let (r, g, b) = (r as i32, g as i32, b as i32);
    let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
    (
        y.clamp(0, 255) as u8,
        u.clamp(0, 255) as u8,
        v.clamp(0, 255) as u8,
    )
}

/// A picture of 24-bit rows from the top (`stride` bytes each) in the
/// format `k`: RGB from the bottom up when `bottom_up` says so, YUV from
/// the top.
fn convert(k: usize, rows: &[u8], w: u32, h: u32, stride: usize, bottom_up: bool) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let px = |x: usize, y: usize| -> (u8, u8, u8) {
        let at = y * stride + x.min(w.saturating_sub(1)) * 3;
        match rows.get(at..at + 3) {
            Some(p) => (p[0], p[1], p[2]),
            None => (0, 0, 0),
        }
    };
    let mut out = vec![0u8; image_size(k, w as u32, h as u32) as usize];
    match k {
        YV12 => {
            let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
            for y in 0..h {
                for x in 0..w {
                    let (b, g, r) = px(x, y);
                    out[y * w + x] = yuv(b, g, r).0;
                }
            }
            let (v_at, u_at) = (w * h, w * h + cw * ch);
            for y in 0..ch {
                for x in 0..cw {
                    let (b, g, r) = px(2 * x, (2 * y).min(h - 1));
                    let (_, u, v) = yuv(b, g, r);
                    out[v_at + y * cw + x] = v;
                    out[u_at + y * cw + x] = u;
                }
            }
        }
        YUY2 | UYVY => {
            let line = w.div_ceil(2) * 4;
            for y in 0..h {
                for x in (0..w).step_by(2) {
                    let (b0, g0, r0) = px(x, y);
                    let (b1, g1, r1) = px(x + 1, y);
                    let (y0, u, v) = yuv(b0, g0, r0);
                    let y1 = yuv(b1, g1, r1).0;
                    let at = y * line + x * 2;
                    let quad = if k == YUY2 {
                        [y0, u, y1, v]
                    } else {
                        [u, y0, v, y1]
                    };
                    out[at..at + 4].copy_from_slice(&quad);
                }
            }
        }
        _ => {
            let bits = FORMATS[k].bits as u32;
            let line = dib_stride(w as i32, bits);
            for y in 0..h {
                let row = if bottom_up { h - 1 - y } else { y };
                for x in 0..w {
                    let (b, g, r) = px(x, y);
                    let at = row * line;
                    match k {
                        RGB24 => out[at + 3 * x..at + 3 * x + 3].copy_from_slice(&[b, g, r]),
                        RGB32 => out[at + 4 * x..at + 4 * x + 4].copy_from_slice(&[b, g, r, 0]),
                        _ => {
                            let p = if k == RGB565 {
                                (r as u16 >> 3) << 11 | (g as u16 >> 2) << 5 | b as u16 >> 3
                            } else {
                                (r as u16 >> 3) << 10 | (g as u16 >> 3) << 5 | b as u16 >> 3
                            };
                            out[at + 2 * x..at + 2 * x + 2].copy_from_slice(&p.to_le_bytes());
                        }
                    }
                }
            }
        }
    }
    out
}

/// The decoder's media type of the format `k` for `m`: AM_MEDIA_TYPE
/// with no format pointer, and its VIDEOINFOHEADER (with RGB565's
/// masks after it).
fn own_type(m: &Media, k: usize) -> (Vec<u8>, Vec<u8>) {
    let f = &FORMATS[k];
    let (w, h) = (m.width, m.height);
    let size = image_size(k, w, h);
    let mut format = vec![0u8; VIDEO_INFO + if f.masks.is_some() { 12 } else { 0 }];
    let put = |b: &mut [u8], at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    // rcSource and rcTarget: the whole picture.
    for base in [0, 16] {
        put(&mut format, base + 8, w);
        put(&mut format, base + 12, h);
    }
    let (num, den) = m.rate;
    let per_frame = den as u64 * 10_000_000 / num.max(1) as u64;
    format[40..48].copy_from_slice(&per_frame.to_le_bytes());
    put(&mut format, 48, 40);
    put(&mut format, 52, w);
    put(&mut format, 56, h);
    format[60..62].copy_from_slice(&1u16.to_le_bytes());
    format[62..64].copy_from_slice(&f.bits.to_le_bytes());
    put(&mut format, 64, f.compression);
    put(&mut format, 68, size);
    put(&mut format, 72, 2000);
    put(&mut format, 76, 2000);
    if let Some(masks) = f.masks {
        for (j, mask) in masks.into_iter().enumerate() {
            put(&mut format, VIDEO_INFO + 4 * j, mask);
        }
    }
    let mut head = vec![0u8; MEDIA_TYPE];
    head[0..16].copy_from_slice(&MEDIATYPE_VIDEO.0);
    head[16..32].copy_from_slice(&f.subtype.0);
    put(&mut head, 32, 1);
    put(&mut head, 40, size);
    head[44..60].copy_from_slice(&FORMAT_VIDEO_INFO.0);
    put(&mut head, 64, format.len() as u32);
    (head, format)
}

/// An AM_MEDIA_TYPE the program passed: its 72 bytes with the format
/// pointer, and its format block.
pub struct MediaType {
    pub head: Vec<u8>,
    pub format: Vec<u8>,
}

fn word(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4)
        .map_or(0, |w| u32::from_le_bytes(w.try_into().unwrap()))
}

/// The AM_MEDIA_TYPE at `at`, if it and its format block can be read.
pub fn read_media_type(cpu: &mut Cpu, at: u32) -> Option<MediaType> {
    if at == 0 || !cpu.bus.guest_probe(at, MEDIA_TYPE, false) {
        return None;
    }
    let mut head = vec![0u8; MEDIA_TYPE];
    cpu.bus.guest_read_bytes(at, &mut head);
    let (n, p) = (word(&head, 64) as usize, word(&head, 68));
    if n > MAX_FORMAT || (n > 0 && (p == 0 || !cpu.bus.guest_probe(p, n, false))) {
        return None;
    }
    let mut format = vec![0u8; n];
    if n > 0 {
        cpu.bus.guest_read_bytes(p, &mut format);
    }
    Some(MediaType { head, format })
}

/// Write the media type `mt` at `at`, its format block in a new block
/// of the task heap (CoTaskMemAlloc), as CopyMediaType does: false when
/// the heap is full.
fn write_media_type(cpu: &mut Cpu, at: u32, head: &[u8], format: &[u8]) -> bool {
    let mut head = head.to_vec();
    let block = if format.is_empty() {
        0
    } else {
        let b = crate::win32::ole32::task_alloc(cpu, format.len() as u32);
        if b == 0 {
            return false;
        }
        cpu.bus.guest_write_bytes(b, format);
        b
    };
    head[64..68].copy_from_slice(&(format.len() as u32).to_le_bytes());
    head[68..72].copy_from_slice(&block.to_le_bytes());
    // The type's IUnknown isn't passed on.
    head[60..64].fill(0);
    cpu.bus.guest_write_bytes(at, &head);
    true
}

/// A new AM_MEDIA_TYPE of the task heap with a copy of `mt`
/// (CreateMediaType): 0 when the heap is full.
pub fn new_media_type(cpu: &mut Cpu, mt: &MediaType) -> u32 {
    let at = crate::win32::ole32::task_alloc(cpu, MEDIA_TYPE as u32);
    if at == 0 {
        return 0;
    }
    if !write_media_type(cpu, at, &mt.head, &mt.format) {
        crate::win32::ole32::task_free(cpu, at);
        return 0;
    }
    at
}

/// The place in `FORMATS` of a type the decoder can give for `m`, and
/// whether its rows go from the bottom up: a video type of
/// FORMAT_VideoInfo of one of its subtypes, the picture's size and the
/// subtype's depth.
fn accepts(m: &Media, mt: &MediaType) -> Option<(usize, bool)> {
    let (head, f) = (&mt.head, &mt.format);
    if head[0..16] != MEDIATYPE_VIDEO.0 || head[44..60] != FORMAT_VIDEO_INFO.0 {
        return None;
    }
    if f.len() < VIDEO_INFO {
        return None;
    }
    let k = FORMATS.iter().position(|x| head[16..32] == x.subtype.0)?;
    let width = word(f, 52);
    let height = word(f, 56) as i32;
    let bits = u16::from_le_bytes([f[62], f[63]]);
    let compression = word(f, 64);
    let fmt = &FORMATS[k];
    if width != m.width || height.unsigned_abs() != m.height || bits != fmt.bits {
        return None;
    }
    if compression != fmt.compression && !(k == RGB565 && compression == BI_RGB) {
        return None;
    }
    let rgb = k >= RGB24;
    Some((k, rgb && height > 0))
}

/// The sound's media type for `m`: AM_MEDIA_TYPE with no format
/// pointer, and its WAVEFORMATEX, as the AVI splitter gives a PCM stream
/// (fixed-size samples of a block each, cbSize 0).
fn own_sound_type(m: &Media) -> (Vec<u8>, Vec<u8>) {
    let f = m.audio.as_ref().map(|(f, _)| *f).unwrap_or_default();
    let format = f.to_bytes().to_vec();
    let put = |b: &mut [u8], at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let mut head = vec![0u8; MEDIA_TYPE];
    head[0..16].copy_from_slice(&MEDIATYPE_AUDIO.0);
    head[16..32].copy_from_slice(&MEDIASUBTYPE_PCM.0);
    put(&mut head, 32, 1);
    put(&mut head, 40, f.block());
    head[44..60].copy_from_slice(&FORMAT_WAVE_FORMAT_EX.0);
    put(&mut head, 64, format.len() as u32);
    (head, format)
}

/// Whether the sound's output pin can give the type `mt` for `m`: PCM
/// of FORMAT_WaveFormatEx whose WAVEFORMATEX is the sound's, but for
/// what follows its first 16 bytes.
fn accepts_sound(m: &Media, mt: &MediaType) -> bool {
    let (head, f) = (&mt.head, &mt.format);
    let (_, own) = own_sound_type(m);
    head[0..16] == MEDIATYPE_AUDIO.0
        && head[16..32] == MEDIASUBTYPE_PCM.0
        && head[44..60] == FORMAT_WAVE_FORMAT_EX.0
        && f.len() >= 16
        && f[..16] == own[..16]
}

// The connection (RenderFile).

/// The scratch space of the connection's calls: the pointer a call
/// gives back, a count, a pin's direction, IID_IMemInputPin, the
/// allocator's properties, and the media type tried, with its format
/// block.
const C_OUT: u32 = 0;
const C_FETCHED: u32 = 4;
const C_DIR: u32 = 8;
const C_IID: u32 = 12;
const C_PROPS: u32 = 28;
const C_TYPE: u32 = 44;
const C_FORMAT: u32 = C_TYPE + MEDIA_TYPE as u32;
const C_SCRATCH: u32 = C_FORMAT + MAX_FORMAT as u32;

/// The connection's variables.
const V_GRAPH: usize = 1;
const V_FILTER: usize = 2;
const V_ENUM: usize = 3;
const V_PIN: usize = 4;
const V_TYPES: usize = 5;
const V_OWN: usize = 6;
const V_INPUT: usize = 7;
const V_ALLOC: usize = 8;
const V_PEER: usize = 9;
const V_THEN: usize = 10;
const V_RELEASE: usize = 11;
const V_FILE: usize = 12;
const V_KIND: usize = 13;
const V_BOTTOM_UP: usize = 14;
/// The stream connecting: 1 for the sound.
const V_AUDIO: usize = 15;

/// The connection's steps.
const S_START: u16 = 0;
const S_FILTER: u16 = 1;
const S_PINS: u16 = 2;
const S_NEXT_PIN: u16 = 3;
const S_PIN: u16 = 4;
const S_NEXT_FILTER: u16 = 5;
const S_DIRECTION: u16 = 6;
const S_CONNECTED: u16 = 7;
const S_SKIP_PIN: u16 = 8;
const S_TYPES: u16 = 9;
const S_NEXT_TYPE: u16 = 10;
const S_TYPE: u16 = 11;
const S_OWN_START: u16 = 12;
const S_OWN: u16 = 13;
const S_ATTEMPT: u16 = 14;
const S_RECEIVED: u16 = 15;
const S_NEXT_CANDIDATE: u16 = 16;
const S_INPUT: u16 = 17;
const S_ALLOCATOR: u16 = 18;
const S_REQUIRED: u16 = 19;
const S_DECIDE: u16 = 20;
const S_NOTIFIED: u16 = 21;
const S_FAILED: u16 = 22;
const S_DISCONNECT: u16 = 23;
const S_DONE: u16 = 24;
const S_RELEASE: u16 = 25;
const S_FINISH: u16 = 26;

/// Connect the streams of the file named by the wide string at `file`,
/// its pictures and then its sound, each to an input pin of the
/// program's filters in the graph `id` that takes it, before RenderFile
/// opens its device: None while the calls are made, after which
/// RenderFile runs again with `connect_tried` set.
pub fn connect(cpu: &mut Cpu, id: u32, file: u32) -> Option<u32> {
    let mut vars = vec![0u32; V_AUDIO + 1];
    vars[0] = JOB_CONNECT;
    vars[V_GRAPH] = id;
    vars[V_FILE] = file;
    task::start(cpu, task::DSHOW, vars, C_SCRATCH)
}

/// Go on at `step` at once.
fn goto(step: u16) -> Next {
    Next::call(0, &[], step)
}

/// Release `this`, then go on at `then`.
fn release_then(t: &mut Task, this: u32, then: u16) -> Next {
    if this == 0 {
        return goto(then);
    }
    t.set(V_RELEASE, this);
    t.set(V_THEN, then as u32);
    goto(S_RELEASE)
}

/// Call the method `index` of `this` with `args` after it.
fn call_method(cpu: &mut Cpu, this: u32, index: u32, args: &[u32], then: u16) -> Next {
    let Some(target) = method(cpu, this, index) else {
        // Not an interface: the call fails.
        return Next::call(0, &[], then);
    };
    let mut all = vec![this];
    all.extend_from_slice(args);
    Next::call(target, &all, then)
}

fn out(cpu: &mut Cpu, t: &Task) -> u32 {
    cpu.bus.guest_read_32(t.scratch + C_OUT)
}

fn clear_out(cpu: &mut Cpu, t: &Task) {
    cpu.bus.guest_write_32(t.scratch + C_OUT, 0);
}

fn failed(value: u32) -> bool {
    (value as i32) < 0
}

fn connect_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let id = t.var(V_GRAPH);
    let s = t.scratch;
    // A call that couldn't be made (not an interface) went on with 0,
    // which reads as S_OK: the steps that check results treat a missing
    // method by what it leaves in the scratch space.
    match t.step {
        S_START => {
            let path = crate::win32::dmusic::read_wide(cpu, t.var(V_FILE), 260);
            let Some(m) = read_media(cpu, &path) else {
                return goto(S_FINISH);
            };
            super::IID_IMEM_INPUT_PIN.write(cpu, s + C_IID);
            let m = Arc::new(m);
            let stream = if m.frames > 0 {
                Stream::Video
            } else {
                Stream::Audio
            };
            begin(cpu, t, path, m, stream)
        }
        S_FILTER => {
            let k = t.var(V_FILTER) as usize;
            let Some(f) = graph(cpu, id)
                .and_then(|g| g.filters.get(k))
                .map(|f| f.iface)
            else {
                return goto(S_FINISH);
            };
            clear_out(cpu, t);
            call_method(cpu, f, FILTER_ENUM_PINS, &[s + C_OUT], S_PINS)
        }
        S_PINS => {
            let e = out(cpu, t);
            if failed(value) || e == 0 {
                return goto(S_NEXT_FILTER);
            }
            t.set(V_ENUM, e);
            goto(S_NEXT_PIN)
        }
        S_NEXT_PIN => {
            clear_out(cpu, t);
            let e = t.var(V_ENUM);
            call_method(cpu, e, ENUM_NEXT, &[1, s + C_OUT, s + C_FETCHED], S_PIN)
        }
        S_PIN => {
            let pin = out(cpu, t);
            if value != S_OK || pin == 0 {
                let e = t.var(V_ENUM);
                return release_then(t, e, S_NEXT_FILTER);
            }
            t.set(V_PIN, pin);
            cpu.bus.guest_write_32(s + C_DIR, 0xFFFF_FFFF);
            call_method(cpu, pin, PIN_QUERY_DIRECTION, &[s + C_DIR], S_DIRECTION)
        }
        S_NEXT_FILTER => {
            t.set(V_ENUM, 0);
            t.set(V_FILTER, t.var(V_FILTER) + 1);
            goto(S_FILTER)
        }
        S_DIRECTION => {
            let dir = cpu.bus.guest_read_32(s + C_DIR);
            if failed(value) || dir != PINDIR_INPUT {
                return goto(S_SKIP_PIN);
            }
            clear_out(cpu, t);
            let pin = t.var(V_PIN);
            call_method(cpu, pin, PIN_CONNECTED_TO, &[s + C_OUT], S_CONNECTED)
        }
        S_CONNECTED => {
            let other = out(cpu, t);
            if other != 0 {
                // Connected already.
                return release_then(t, other, S_SKIP_PIN);
            }
            clear_out(cpu, t);
            let pin = t.var(V_PIN);
            call_method(cpu, pin, PIN_ENUM_MEDIA_TYPES, &[s + C_OUT], S_TYPES)
        }
        S_SKIP_PIN => {
            let pin = t.var(V_PIN);
            t.set(V_PIN, 0);
            release_then(t, pin, S_NEXT_PIN)
        }
        S_TYPES => {
            let e = out(cpu, t);
            if failed(value) || e == 0 {
                return goto(S_OWN_START);
            }
            t.set(V_TYPES, e);
            goto(S_NEXT_TYPE)
        }
        S_NEXT_TYPE => {
            clear_out(cpu, t);
            let e = t.var(V_TYPES);
            call_method(cpu, e, ENUM_NEXT, &[1, s + C_OUT, s + C_FETCHED], S_TYPE)
        }
        S_TYPE => {
            let at = out(cpu, t);
            if value != S_OK || at == 0 {
                let e = t.var(V_TYPES);
                t.set(V_TYPES, 0);
                return release_then(t, e, S_OWN_START);
            }
            // The type is the caller's to free (DeleteMediaType), its
            // format block too when it can't be read.
            let mt = read_media_type(cpu, at);
            sample::free_media_type(cpu, at);
            let media = connecting_media(cpu, id);
            let audio = t.var(V_AUDIO) != 0;
            let found = mt.as_ref().zip(media).and_then(|(mt, m)| {
                if audio {
                    accepts_sound(&m, mt).then_some((mt, (0, false)))
                } else {
                    accepts(&m, mt).map(|k| (mt, k))
                }
            });
            let Some((mt, (k, bottom_up))) = found else {
                return goto(S_NEXT_TYPE);
            };
            put_type(cpu, s, &mt.head, &mt.format);
            t.set(V_PEER, 1);
            t.set(V_KIND, k as u32);
            t.set(V_BOTTOM_UP, bottom_up as u32);
            goto(S_ATTEMPT)
        }
        S_OWN_START => {
            t.set(V_OWN, 0);
            goto(S_OWN)
        }
        S_OWN => {
            let k = t.var(V_OWN) as usize;
            let media = connecting_media(cpu, id);
            let audio = t.var(V_AUDIO) != 0;
            let types = if audio { 1 } else { FORMATS.len() };
            let (Some(m), true) = (media, k < types) else {
                return goto(S_SKIP_PIN);
            };
            let (head, format) = if audio {
                own_sound_type(&m)
            } else {
                own_type(&m, k)
            };
            put_type(cpu, s, &head, &format);
            t.set(V_PEER, 0);
            t.set(V_KIND, k as u32);
            t.set(V_BOTTOM_UP, (!audio && k >= RGB24) as u32);
            goto(S_ATTEMPT)
        }
        S_ATTEMPT => {
            let out_pin = graph(cpu, id)
                .and_then(|g| g.connecting.as_ref())
                .map_or(0, |l| l.out_pin);
            let Some(me) = super::block(cpu, out_pin, super::PIN) else {
                return goto(S_SKIP_PIN);
            };
            let pin = t.var(V_PIN);
            // A missing method fails the attempt (`call_method` goes on
            // with 0, which E_FAIL stands for here).
            if method(cpu, pin, PIN_RECEIVE_CONNECTION).is_none() {
                return goto(S_NEXT_CANDIDATE);
            }
            call_method(
                cpu,
                pin,
                PIN_RECEIVE_CONNECTION,
                &[me, s + C_TYPE],
                S_RECEIVED,
            )
        }
        S_RECEIVED => {
            if failed(value) {
                return goto(S_NEXT_CANDIDATE);
            }
            clear_out(cpu, t);
            let pin = t.var(V_PIN);
            call_method(cpu, pin, QUERY_INTERFACE, &[s + C_IID, s + C_OUT], S_INPUT)
        }
        S_NEXT_CANDIDATE => {
            if t.var(V_PEER) != 0 {
                goto(S_NEXT_TYPE)
            } else {
                t.set(V_OWN, t.var(V_OWN) + 1);
                goto(S_OWN)
            }
        }
        S_INPUT => {
            let input = out(cpu, t);
            if failed(value) || input == 0 {
                return goto(S_DISCONNECT);
            }
            t.set(V_INPUT, input);
            // The base classes' DecideAllocator: the pin's requirements
            // first, then its allocator.
            cpu.bus.guest_fill(s + C_PROPS, 16, 0);
            if method(cpu, input, INPUT_GET_ALLOCATOR_REQUIREMENTS).is_none() {
                return goto(S_REQUIRED);
            }
            call_method(
                cpu,
                input,
                INPUT_GET_ALLOCATOR_REQUIREMENTS,
                &[s + C_PROPS],
                S_REQUIRED,
            )
        }
        S_REQUIRED => {
            if failed(value) {
                cpu.bus.guest_fill(s + C_PROPS, 16, 0);
            }
            clear_out(cpu, t);
            let input = t.var(V_INPUT);
            call_method(cpu, input, INPUT_GET_ALLOCATOR, &[s + C_OUT], S_ALLOCATOR)
        }
        S_ALLOCATOR => {
            let given = out(cpu, t);
            let ours = if failed(value) || given == 0 {
                None
            } else {
                sample::allocator_of(cpu, given)
            };
            match ours {
                // The pin's allocator is the memory allocator the
                // program made (CLSID_MemoryAllocator): the reference
                // GetAllocator gave is the link's.
                Some(a) => {
                    t.set(V_ALLOC, a);
                    goto(S_DECIDE)
                }
                None => {
                    // None, or one of the program's own: the decoder's
                    // own allocator, as the base classes' output pins
                    // use when the input pin's isn't taken.
                    let a = sample::new_allocator(cpu);
                    t.set(V_ALLOC, a);
                    if !failed(value) && given != 0 {
                        return release_then(t, given, S_DECIDE);
                    }
                    goto(S_DECIDE)
                }
            }
        }
        S_DECIDE => {
            // The requirements, zero when the pin gave none (S_REQUIRED).
            let req = sample::read_props(cpu, s + C_PROPS);
            let k = t.var(V_KIND) as usize;
            let audio = t.var(V_AUDIO) != 0;
            let size = connecting_media(cpu, id).map_or(0, |m| {
                if audio {
                    sound_sample_bytes(&m)
                } else {
                    image_size(k, m.width, m.height)
                }
            });
            // DecideBufferSize: a buffer at least, a picture's or a
            // sample of the sound's size at least, an alignment of 1 at
            // least (quartz_transform.c).
            let props = [req[0].max(1), req[1].max(size), req[2].max(1), req[3]];
            let a = t.var(V_ALLOC);
            if sample::set_props(cpu, a, props).is_err() {
                return goto(S_FAILED);
            }
            let Some(at) = super::block(cpu, a, super::ALLOCATOR) else {
                return goto(S_FAILED);
            };
            let input = t.var(V_INPUT);
            call_method(cpu, input, INPUT_NOTIFY_ALLOCATOR, &[at, 0], S_NOTIFIED)
        }
        S_NOTIFIED => {
            if failed(value) {
                return goto(S_FAILED);
            }
            let mut head = vec![0u8; MEDIA_TYPE];
            cpu.bus.guest_read_bytes(s + C_TYPE, &mut head);
            let n = (word(&head, 64) as usize).min(MAX_FORMAT);
            let mut format = vec![0u8; n];
            cpu.bus.guest_read_bytes(s + C_FORMAT, &mut format);
            head[68..72].fill(0);
            let k = graph(cpu, id).and_then(|g| g.filters.get(t.var(V_FILTER) as usize).cloned());
            let Some(g) = graph(cpu, id) else {
                return goto(S_FINISH);
            };
            let Some(mut l) = g.connecting.take() else {
                return goto(S_FINISH);
            };
            l.filter = k.map_or(0, |f| f.iface);
            l.pin = t.var(V_PIN);
            l.input = t.var(V_INPUT);
            l.allocator = t.var(V_ALLOC);
            l.head = head;
            l.format = format;
            l.kind = t.var(V_KIND);
            l.bottom_up = t.var(V_BOTTOM_UP) != 0;
            if l.audio {
                g.sound = Some(l);
            } else {
                g.link = Some(l);
            }
            t.set(V_PIN, 0);
            let e = t.var(V_TYPES);
            t.set(V_TYPES, 0);
            release_then(t, e, S_DONE)
        }
        S_FAILED => {
            let a = t.var(V_ALLOC);
            t.set(V_ALLOC, 0);
            if a != 0 {
                super::drop_ref(cpu, a);
            }
            let input = t.var(V_INPUT);
            t.set(V_INPUT, 0);
            release_then(t, input, S_DISCONNECT)
        }
        S_DISCONNECT => {
            let pin = t.var(V_PIN);
            call_method(cpu, pin, PIN_DISCONNECT, &[], S_NEXT_CANDIDATE)
        }
        S_DONE => {
            let e = t.var(V_ENUM);
            t.set(V_ENUM, 0);
            release_then(t, e, S_FINISH)
        }
        S_RELEASE => {
            let this = t.var(V_RELEASE);
            let then = t.var(V_THEN) as u16;
            call_method(cpu, this, RELEASE, &[], then)
        }
        _ => {
            // S_FINISH: without a connection, the stream's filter and its
            // pin go. The sound connects after the pictures.
            let left = graph(cpu, id).and_then(|g| g.connecting.take());
            let media = left.as_ref().and_then(|l| l.media.clone()).or_else(|| {
                graph(cpu, id)
                    .and_then(|g| g.link.as_ref())
                    .and_then(|l| l.media.clone())
            });
            if let Some(l) = left {
                for o in [l.out_pin, l.decoder] {
                    super::drop_ref(cpu, o);
                }
            }
            if t.var(V_AUDIO) == 0
                && let Some(m) = media.filter(|m| m.audio.is_some())
            {
                let path = crate::win32::dmusic::read_wide(cpu, t.var(V_FILE), 260);
                return begin(cpu, t, path, m, Stream::Audio);
            }
            if let Some(g) = graph(cpu, id) {
                g.connect_tried = true;
            }
            t.retry(cpu);
            Next::Stop
        }
    }
}

/// Start connecting the stream `stream` of the file `path`, `m`: its
/// filter and output pin are made, and the program's filters are tried
/// from the first.
fn begin(cpu: &mut Cpu, t: &mut Task, path: String, m: Arc<Media>, stream: Stream) -> Next {
    let id = t.var(V_GRAPH);
    let decoder = super::add(
        cpu,
        Kind::Decoder {
            d: Decoder { graph: id },
        },
    );
    let out_pin = super::add(
        cpu,
        Kind::Pin {
            p: Pin { graph: id },
        },
    );
    let Some(g) = graph(cpu, id) else {
        return goto(S_FINISH);
    };
    g.connecting = Some(Link {
        audio: stream == Stream::Audio,
        decoder,
        out_pin,
        file: path,
        media: Some(m),
        ..Link::default()
    });
    for v in [V_FILTER, V_ENUM, V_PIN, V_TYPES, V_OWN, V_INPUT, V_ALLOC, V_PEER] {
        t.set(v, 0);
    }
    t.set(V_AUDIO, (stream == Stream::Audio) as u32);
    goto(S_FILTER)
}

/// Put the media type `head` with the format block `format` in the
/// connection's scratch space, pointing at its block there.
fn put_type(cpu: &mut Cpu, s: u32, head: &[u8], format: &[u8]) {
    let mut head = head.to_vec();
    let n = format.len().min(MAX_FORMAT);
    head[64..68].copy_from_slice(&(n as u32).to_le_bytes());
    head[68..72].copy_from_slice(&(s + C_FORMAT).to_le_bytes());
    cpu.bus.guest_write_bytes(s + C_TYPE, &head);
    cpu.bus.guest_write_bytes(s + C_FORMAT, &format[..n]);
}

// The IBaseFilter of the filter that gives a stream.

/// The names of the filter that gives the stream of the link `l` and of
/// its output pin, and the filter's class.
fn names(l: &Link) -> (&'static str, &'static str, Guid) {
    if !l.audio {
        return (DECODER_NAME, PIN_NAME, super::CLSID_MPEG_VIDEO_CODEC);
    }
    let mpeg = l
        .media
        .as_ref()
        .is_some_and(|m| matches!(m.video, media::Kind::Mpeg(_)));
    if mpeg {
        (MPEG_AUDIO.0, MPEG_AUDIO.1, super::CLSID_MPEG_AUDIO_CODEC)
    } else {
        (AVI_SPLITTER.0, AVI_SPLITTER.1, super::CLSID_AVI_SPLITTER)
    }
}

/// The graph of the filter or pin `this`, and the names and class of
/// its link's stream (`names`), the pictures' when it has none.
fn identity(cpu: &mut Cpu, this: u32) -> Result<(u32, (&'static str, &'static str, Guid)), u32> {
    let id = owner_of(cpu, this)?;
    let object = super::object_of(cpu, this).ok_or(E_POINTER)?;
    let found = link_of(cpu, id, object).map(|l| names(l));
    Ok((id, found.unwrap_or((DECODER_NAME, PIN_NAME, super::CLSID_MPEG_VIDEO_CODEC))))
}

/// IPersist::GetClassID(this, out): CLSID_CMpegVideoCodec, or the
/// sound's filter's.
pub fn decoder_get_class_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, (_, _, class)) = hr!(identity(cpu, args[0]));
    if args[1] == 0 || !class.write(cpu, args[1]) {
        return Some(E_POINTER);
    }
    Some(S_OK)
}

/// IMediaFilter::GetState(this, timeout, out): the graph's.
pub fn decoder_get_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(owner_of(cpu, args[0]));
    let state = graph(cpu, id).map_or(STATE_STOPPED, |g| g.state);
    Some(put32(cpu, args[2], state).map_or_else(|e| e, |_| S_OK))
}

/// IMediaFilter::GetSyncSource(this, out): the graph's clock once it
/// gave it, else NULL.
pub fn decoder_get_sync_source(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(owner_of(cpu, args[0]));
    let clock = graph(cpu, id)
        .filter(|g| g.clock_given)
        .map_or(0, |g| g.clock);
    let at = if clock != 0 {
        super::pointer(cpu, clock)
    } else {
        0
    };
    Some(put32(cpu, args[1], at).map_or_else(|e| e, |_| S_OK))
}

/// IBaseFilter::FindPin(this, id, out): its output pin, the one the
/// program sees, by its name. Its input pin is the graph's own.
pub fn decoder_find_pin(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(owner_of(cpu, args[0]));
    hr!(put32(cpu, args[2], 0));
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    let name = crate::win32::dmusic::read_wide(cpu, args[1], 128);
    let object = hr!(super::object_of(cpu, args[0]).ok_or(E_POINTER));
    let found = STREAMS.into_iter().find_map(|s| {
        link(cpu, id, s)
            .filter(|l| l.decoder == object && names(l).1 == name)
            .map(|l| l.out_pin)
    });
    let Some(pin) = found else {
        return Some(VFW_E_NOT_FOUND);
    };
    let at = super::pointer(cpu, pin);
    cpu.bus.guest_write_32(args[2], at);
    Some(S_OK)
}

/// IBaseFilter::QueryFilterInfo(this, info): its name and its graph,
/// with a reference taken.
pub fn decoder_query_filter_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, (name, _, _)) = hr!(identity(cpu, args[0]));
    let info = args[1];
    if info == 0 || !cpu.bus.guest_probe(info, 260, true) {
        return Some(E_POINTER);
    }
    crate::win32::dmusic::write_wide(cpu, info, name, 128);
    let at = super::pointer(cpu, id);
    cpu.bus.guest_write_32(info + 256, at);
    Some(S_OK)
}

// The output pins.

/// The graph of the pin `this`, and its link if it is connected.
fn pin_link(cpu: &mut Cpu, this: u32) -> Result<(u32, Option<Link>), u32> {
    let id = super::object_of(cpu, this).ok_or(E_POINTER)?;
    let Some(Kind::Pin { p }) = super::kind(cpu, id) else {
        return Err(E_POINTER);
    };
    let graph_id = p.graph;
    let l = STREAMS.into_iter().find_map(|s| {
        link(cpu, graph_id, s)
            .filter(|l| l.out_pin == id)
            .map(|l| Link {
                head: l.head.clone(),
                format: l.format.clone(),
                ..l.clone_shallow()
            })
    });
    Ok((graph_id, l))
}

/// IPin::Connect(this, peer, type): the pin is connected when the graph
/// renders its file, and only then.
pub fn pin_connect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, l) = hr!(pin_link(cpu, args[0]));
    Some(if l.is_some() {
        VFW_E_ALREADY_CONNECTED
    } else {
        E_NOTIMPL
    })
}

/// IPin::Disconnect(this): the program's pin is told, and the link let
/// go of. The graph must be stopped. S_FALSE when not connected.
pub fn pin_disconnect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, l) = hr!(pin_link(cpu, args[0]));
    let Some(l) = l else {
        return Some(S_FALSE);
    };
    if graph(cpu, id).is_some_and(|g| g.state != STATE_STOPPED) {
        return Some(VFW_E_NOT_STOPPED);
    }
    let stream = l.stream();
    let mut list = List::new(S_OK, THEN_UNLINKED, id, stream.bit(), 0);
    list.release(cpu, l.input);
    list.release(cpu, l.pin);
    if let Some(l) = link(cpu, id, stream) {
        // Its pin, which has let go of this one, isn't told again.
        l.pin = 0;
        l.input = 0;
    }
    list.start(cpu, 0)
}

/// IPin::ConnectedTo(this, out): the program's pin, with a reference
/// taken.
pub fn pin_connected_to(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, l) = hr!(pin_link(cpu, args[0]));
    hr!(put32(cpu, args[1], 0));
    let Some(l) = l else {
        return Some(VFW_E_NOT_CONNECTED);
    };
    cpu.bus.guest_write_32(args[1], l.pin);
    let mut list = List::new(S_OK, THEN_DONE, id, 0, 0);
    list.call(cpu, l.pin, ADD_REF, &[], 0);
    list.start(cpu, 0)
}

/// IPin::ConnectionMediaType(this, type): a copy of the connection's.
pub fn pin_connection_media_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, l) = hr!(pin_link(cpu, args[0]));
    let at = args[1];
    if at == 0 || !cpu.bus.guest_probe(at, MEDIA_TYPE, true) {
        return Some(E_POINTER);
    }
    let Some(l) = l else {
        cpu.bus.guest_fill(at, MEDIA_TYPE, 0);
        return Some(VFW_E_NOT_CONNECTED);
    };
    if !write_media_type(cpu, at, &l.head, &l.format) {
        return Some(E_OUTOFMEMORY);
    }
    Some(S_OK)
}

/// IPin::QueryPinInfo(this, info): its filter, with a reference taken,
/// its direction and its name.
pub fn pin_query_pin_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(super::object_of(cpu, args[0]).ok_or(E_POINTER));
    let info = args[1];
    if info == 0 || !cpu.bus.guest_probe(info, 264, true) {
        return Some(E_POINTER);
    }
    let (graph_id, (_, name, _)) = hr!(identity(cpu, args[0]));
    let decoder = link_of(cpu, graph_id, id).map_or(0, |l| l.decoder);
    let at = if decoder != 0 {
        super::pointer(cpu, decoder)
    } else {
        0
    };
    cpu.bus.guest_write_32(info, at);
    cpu.bus.guest_write_32(info + 4, PINDIR_OUTPUT);
    crate::win32::dmusic::write_wide(cpu, info + 8, name, 128);
    Some(S_OK)
}

/// IPin::QueryDirection(this, out): an output pin.
pub fn pin_query_direction(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(owner_of(cpu, args[0]));
    Some(put32(cpu, args[1], PINDIR_OUTPUT).map_or_else(|e| e, |_| S_OK))
}

/// IPin::QueryId(this, out): its name, in a block of the task heap.
pub fn pin_query_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, (_, name, _)) = hr!(identity(cpu, args[0]));
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    let n = (name.len() as u32 + 1) * 2;
    let at = crate::win32::ole32::task_alloc(cpu, n);
    if at == 0 {
        return Some(E_OUTOFMEMORY);
    }
    crate::win32::dmusic::write_wide(cpu, at, name, name.len() + 1);
    Some(put32(cpu, args[1], at).map_or_else(|e| e, |_| S_OK))
}

/// IPin::QueryAccept(this, type): whether the pin can give it.
pub fn pin_query_accept(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(owner_of(cpu, args[0]));
    let object = hr!(super::object_of(cpu, args[0]).ok_or(E_POINTER));
    let Some(mt) = read_media_type(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    let found = [Some(Stream::Video), Some(Stream::Audio), None]
        .into_iter()
        .find(|&s| slot(cpu, id, s).is_some_and(|l| l.out_pin == object));
    let Some(which) = found else {
        return Some(S_FALSE);
    };
    let audio = slot(cpu, id, which).is_some_and(|l| l.audio);
    let ok = media_of(cpu, id, which).is_some_and(|m| {
        if audio {
            accepts_sound(&m, &mt)
        } else {
            accepts(&m, &mt).is_some()
        }
    });
    Some(if ok { S_OK } else { S_FALSE })
}

crate::state_fields!(Filter { iface, name });
crate::state_fields!(Link {
    audio,
    filter,
    pin,
    input,
    allocator,
    decoder,
    out_pin,
    head,
    format,
    kind,
    bottom_up,
    file,
    tid,
    wake,
    next,
    from_ms,
    to_ms,
    active,
    ended,
    refused,
    segment,
    discontinuity,
    generation
} skip {
    media,
    pictures
});
crate::state_fields!(Decoder { graph });
crate::state_fields!(Pin { graph });
