//! DMIME.DLL's segments, their tracks, and graphs.
//!
//! A segment is a list of tracks, each in some groups. Tracks keep their
//! events (files.rs), which a performance plays (performance.rs), and
//! answer for the parameters of their kind: tempo, time signature, mute,
//! and the band track's commands. Tools put in a graph are kept, and not
//! called.

use super::files::{Ev, SegmentData, Timed, TrackData};
use super::guids::*;
use super::{
    E_INVALIDARG, E_POINTER, GRAPH, Kind, S_FALSE, S_OK, SEGMENT, TRACK, add, dmus_e, drop_ref,
    give_new, hold, interface, object_of, pointer, state, this_of,
};
use crate::cpu::Cpu;
use crate::win32::com::{self, Guid};

pub const DMUS_E_SET_UNSUPPORTED: u32 = dmus_e(0x123);
pub const DMUS_E_GET_UNSUPPORTED: u32 = dmus_e(0x124);
pub const DMUS_E_TYPE_UNSUPPORTED: u32 = dmus_e(0x164);
pub const DMUS_E_TRACK_NOT_FOUND: u32 = dmus_e(0x166);
pub const DMUS_E_OUT_OF_RANGE: u32 = dmus_e(0x202);
use super::DMUS_E_NOT_FOUND;

/// A segment: its times, its tracks (id, groups), graph and the
/// notifications asked of it.
#[derive(Clone, Debug, Default)]
pub struct Segment {
    pub length: i32,
    pub repeats: u32,
    pub resolution: u32,
    pub start: i32,
    pub loop_start: i32,
    pub loop_end: i32,
    pub tracks: Vec<(u32, u32)>,
    pub graph: u32,
    pub notifications: Vec<[u8; 16]>,
}

impl Segment {
    pub fn held(&self) -> Vec<u32> {
        let mut out: Vec<u32> = self.tracks.iter().map(|t| t.0).collect();
        if self.graph != 0 {
            out.push(self.graph);
        }
        out
    }
}

/// A track: its class and events.
#[derive(Clone, Debug, Default)]
pub struct Track {
    pub class: [u8; 16],
    pub events: Vec<Timed>,
}

impl Track {
    pub fn held(&self) -> Vec<u32> {
        Vec::new()
    }
}

/// CoCreateInstance of a segment: empty.
pub fn create_segment(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = new_segment(
        cpu,
        SegmentData {
            length: 1,
            ..SegmentData::default()
        },
    );
    give_new(cpu, id, iid, out)
}

/// CoCreateInstance of a graph.
pub fn create_graph(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = add(cpu, Kind::Graph { tools: Vec::new() });
    give_new(cpu, id, iid, out)
}

fn new_track_object(cpu: &mut Cpu, class: Guid, iid: &Guid, out: u32) -> u32 {
    let id = new_track(
        cpu,
        Track {
            class: class.0,
            events: Vec::new(),
        },
    );
    give_new(cpu, id, iid, out)
}

/// The `create` of each track class (com.rs), which know their class.
macro_rules! track_classes {
    ($($name:ident: $clsid:ident,)*) => {
        $(
            pub fn $name(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
                new_track_object(cpu, $clsid, iid, out)
            }
        )*
    };
}

track_classes! {
    create_tempo_track: CLSID_TEMPO_TRACK,
    create_seq_track: CLSID_SEQ_TRACK,
    create_sysex_track: CLSID_SYSEX_TRACK,
    create_time_sig_track: CLSID_TIME_SIG_TRACK,
    create_chord_track: CLSID_CHORD_TRACK,
    create_command_track: CLSID_COMMAND_TRACK,
    create_style_track: CLSID_STYLE_TRACK,
    create_motif_track: CLSID_MOTIF_TRACK,
    create_mute_track: CLSID_MUTE_TRACK,
    create_band_track: CLSID_BAND_TRACK,
    create_chord_map_track: CLSID_CHORD_MAP_TRACK,
    create_sign_post_track: CLSID_SIGN_POST_TRACK,
}

/// A track object: its id, with one reference.
pub fn new_track(cpu: &mut Cpu, t: Track) -> u32 {
    add(cpu, Kind::Track { t })
}

/// A segment object of `data`, its tracks made: its id.
pub fn new_segment(cpu: &mut Cpu, data: SegmentData) -> u32 {
    let tracks = data
        .tracks
        .into_iter()
        .map(
            |TrackData {
                 class,
                 group,
                 events,
             }| {
                let mut events = events;
                events.sort_by_key(|t| (t.mt, t.ev.order()));
                (new_track(cpu, Track { class, events }), group)
            },
        )
        .collect();
    add(
        cpu,
        Kind::Segment {
            s: Segment {
                length: data.length.max(1),
                repeats: data.repeats,
                resolution: data.resolution,
                start: data.start,
                loop_start: data.loop_start,
                loop_end: data.loop_end,
                tracks,
                graph: 0,
                notifications: Vec::new(),
            },
        },
    )
}

pub fn segment_of(cpu: &mut Cpu, id: u32) -> Option<&mut Segment> {
    match &mut state(cpu).get_mut(id)?.kind {
        Kind::Segment { s } => Some(s),
        _ => None,
    }
}

pub fn track_of(cpu: &mut Cpu, id: u32) -> Option<&mut Track> {
    match &mut state(cpu).get_mut(id)?.kind {
        Kind::Track { t } => Some(t),
        _ => None,
    }
}

/// The segment `this` is, if it is one.
fn this_segment(cpu: &mut Cpu, this: u32) -> Option<u32> {
    this_of(cpu, this, SEGMENT)
}

/// The events of the segment `id`'s tracks, in time order: what a
/// performance plays of it.
pub fn events(cpu: &mut Cpu, id: u32) -> Vec<Timed> {
    let Some(s) = segment_of(cpu, id) else {
        return Vec::new();
    };
    let tracks: Vec<u32> = s.tracks.iter().map(|t| t.0).collect();
    let mut out = Vec::new();
    for t in tracks {
        if let Some(t) = track_of(cpu, t) {
            out.extend(t.events.iter().cloned());
        }
    }
    out.sort_by_key(|t| (t.mt, t.ev.order()));
    out
}

/// Write a number the program asked for at `out`.
fn answer(cpu: &mut Cpu, out: u32, value: u32) -> Option<u32> {
    Some(if com::write_out(cpu, out, value) {
        S_OK
    } else {
        E_POINTER
    })
}

/// IDirectMusicSegment::GetLength(this, length).
pub fn get_length(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_segment(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let v = segment_of(cpu, id)?.length as u32;
    answer(cpu, args[1], v)
}

/// IDirectMusicSegment::SetLength(this, length).
pub fn set_length(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_segment(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let s = segment_of(cpu, id)?;
    let length = args[1] as i32;
    if length <= s.start {
        return Some(DMUS_E_OUT_OF_RANGE);
    }
    s.length = length;
    Some(S_OK)
}

/// IDirectMusicSegment::GetRepeats(this, repeats).
pub fn get_repeats(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_segment(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let v = segment_of(cpu, id)?.repeats;
    answer(cpu, args[1], v)
}

/// IDirectMusicSegment::SetRepeats(this, repeats).
pub fn set_repeats(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_segment(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    segment_of(cpu, id)?.repeats = args[1];
    Some(S_OK)
}

/// IDirectMusicSegment::GetDefaultResolution(this, flags).
pub fn get_default_resolution(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_segment(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let v = segment_of(cpu, id)?.resolution;
    answer(cpu, args[1], v)
}

/// IDirectMusicSegment::SetDefaultResolution(this, flags).
pub fn set_default_resolution(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_segment(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    segment_of(cpu, id)?.resolution = args[1];
    Some(S_OK)
}

/// Whether a class GUID the program passed stands for any track.
fn any_class(g: &Guid) -> bool {
    *g == com::GUID_NULL || *g == GUID_ALL_TYPES
}

/// IDirectMusicSegment::GetTrack(this, class, groups, index, track).
pub fn get_track(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, class, groups, index, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(id) = this_segment(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_POINTER);
    }
    let class = if class == 0 {
        com::GUID_NULL
    } else {
        Guid::read(cpu, class)?
    };
    let tracks = segment_of(cpu, id)?.tracks.clone();
    let mut found = tracks.iter().filter(|&&(t, g)| {
        g & groups != 0
            && (any_class(&class) || track_of(cpu, t).is_some_and(|t| t.class == class.0))
    });
    let Some(&(track, _)) = found.nth(index as usize) else {
        return Some(DMUS_E_NOT_FOUND);
    };
    Some(super::give(cpu, track, &IID_IDIRECTMUSIC_TRACK, out))
}

/// IDirectMusicSegment::GetTrackGroup(this, track, groups).
pub fn get_track_group(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_segment(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(track) = this_of(cpu, args[1], TRACK) else {
        return Some(E_POINTER);
    };
    let found = segment_of(cpu, id)?
        .tracks
        .iter()
        .find(|t| t.0 == track)
        .map(|t| t.1);
    match found {
        Some(g) => answer(cpu, args[2], g),
        None => Some(DMUS_E_NOT_FOUND),
    }
}

/// IDirectMusicSegment::InsertTrack(this, track, groups).
pub fn insert_track(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_segment(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(track) = this_of(cpu, args[1], TRACK) else {
        return Some(E_POINTER);
    };
    if args[2] == 0 {
        return Some(E_INVALIDARG);
    }
    let s = segment_of(cpu, id)?;
    if s.tracks.iter().any(|t| t.0 == track) {
        return Some(S_FALSE);
    }
    s.tracks.push((track, args[2]));
    hold(cpu, track);
    Some(S_OK)
}

/// IDirectMusicSegment::RemoveTrack(this, track).
pub fn remove_track(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_segment(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(track) = this_of(cpu, args[1], TRACK) else {
        return Some(E_POINTER);
    };
    let s = segment_of(cpu, id)?;
    let Some(i) = s.tracks.iter().position(|t| t.0 == track) else {
        return Some(S_FALSE);
    };
    s.tracks.remove(i);
    drop_ref(cpu, track);
    Some(S_OK)
}

/// The graph field of the segment or performance `id`.
fn graph_field(cpu: &mut Cpu, id: u32) -> Option<&mut u32> {
    match &mut state(cpu).get_mut(id)?.kind {
        Kind::Segment { s } => Some(&mut s.graph),
        Kind::Performance { p } => Some(&mut p.graph),
        _ => None,
    }
}

/// GetGraph(this, graph) of a segment or a performance.
pub fn get_graph(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object_of(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    let graph = *graph_field(cpu, id)?;
    if graph == 0 {
        com::write_out(cpu, args[1], 0);
        return Some(DMUS_E_NOT_FOUND);
    }
    let at = pointer(cpu, graph);
    answer(cpu, args[1], at)
}

/// SetGraph(this, graph) of a segment or a performance: NULL takes it
/// away.
pub fn set_graph(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object_of(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let graph = if args[1] == 0 {
        0
    } else {
        match this_of(cpu, args[1], GRAPH) {
            Some(g) => g,
            None => return Some(E_POINTER),
        }
    };
    if graph != 0 {
        hold(cpu, graph);
    }
    let old = std::mem::replace(graph_field(cpu, id)?, graph);
    if old != 0 {
        drop_ref(cpu, old);
    }
    Some(S_OK)
}

/// Add or take away a notification of a list: S_FALSE when it already
/// is or isn't there.
pub fn notification(list: &mut Vec<[u8; 16]>, guid: Guid, on: bool) -> u32 {
    let at = list.iter().position(|g| *g == guid.0);
    match (on, at) {
        (true, None) => list.push(guid.0),
        (false, Some(i)) => {
            list.remove(i);
        }
        _ => return S_FALSE,
    }
    S_OK
}

/// IDirectMusicSegment::AddNotificationType(this, guid).
pub fn add_notification_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    segment_notification(cpu, args, true)
}

/// IDirectMusicSegment::RemoveNotificationType(this, guid).
pub fn remove_notification_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    segment_notification(cpu, args, false)
}

fn segment_notification(cpu: &mut Cpu, args: &[u32], on: bool) -> Option<u32> {
    let Some(id) = this_segment(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(guid) = Guid::read(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    Some(notification(
        &mut segment_of(cpu, id)?.notifications,
        guid,
        on,
    ))
}

/// What a track answers for a parameter.
enum Value {
    /// DMUS_TEMPO_PARAM: the change's time from the one asked, and the
    /// tempo.
    Tempo(i32, f64),
    /// DMUS_TIMESIGNATURE.
    TimeSig(i32, u8, u8, u16),
    /// DMUS_MUTE_PARAM: not muted.
    Mute,
}

/// Whether a track of `class` gets (`get`) or sets the parameter `g`.
fn supports(class: &[u8; 16], g: &Guid, get: bool) -> bool {
    let class = Guid(*class);
    if class == CLSID_TEMPO_TRACK {
        *g == GUID_TEMPO_PARAM || (!get && (*g == GUID_ENABLE_TEMPO || *g == GUID_DISABLE_TEMPO))
    } else if class == CLSID_TIME_SIG_TRACK {
        *g == GUID_TIME_SIGNATURE
            || (!get && (*g == GUID_ENABLE_TIME_SIG || *g == GUID_DISABLE_TIME_SIG))
    } else if class == CLSID_MUTE_TRACK {
        *g == GUID_MUTE_PARAM
    } else if class == CLSID_BAND_TRACK {
        !get && [
            GUID_DOWNLOAD,
            GUID_UNLOAD,
            GUID_ENABLE_AUTO_DOWNLOAD,
            GUID_DISABLE_AUTO_DOWNLOAD,
            GUID_CLEAR_ALL_BANDS,
            GUID_IDIRECTMUSIC_BAND,
            GUID_CONNECT_TO_DLS_COLLECTION,
            GUID_STANDARD_MIDI_FILE,
        ]
        .contains(g)
    } else {
        false
    }
}

/// The parameter `g` of the track at `mt`, and when the next change is
/// (0 for none).
fn get_value(t: &Track, g: &Guid, mt: i32) -> Result<(Value, i32), u32> {
    if !supports(&t.class, g, true) {
        return Err(DMUS_E_GET_UNSUPPORTED);
    }
    if *g == GUID_MUTE_PARAM {
        return Ok((Value::Mute, 0));
    }
    let wanted = |e: &Timed| match e.ev {
        Ev::Tempo { .. } => *g == GUID_TEMPO_PARAM,
        Ev::TimeSig { .. } => *g == GUID_TIME_SIGNATURE,
        _ => false,
    };
    let mut current = None;
    let mut next = 0;
    for e in t.events.iter().filter(|e| wanted(e)) {
        if e.mt <= mt {
            current = Some(e);
        } else {
            next = e.mt - mt;
            break;
        }
    }
    let Some(e) = current else {
        return Err(DMUS_E_NOT_FOUND);
    };
    let value = match e.ev {
        Ev::Tempo { tempo } => Value::Tempo(e.mt - mt, tempo),
        Ev::TimeSig { beats, beat, grids } => Value::TimeSig(e.mt - mt, beats, beat, grids),
        _ => return Err(DMUS_E_NOT_FOUND),
    };
    Ok((value, next))
}

/// Write a parameter's value at `at`.
fn write_value(cpu: &mut Cpu, at: u32, v: &Value) -> bool {
    match *v {
        Value::Tempo(rel, tempo) => {
            if !cpu.bus.guest_probe(at, 16, true) {
                return false;
            }
            cpu.bus.guest_write_32(at, rel as u32);
            cpu.bus.guest_write_bytes(at + 8, &tempo.to_le_bytes());
        }
        Value::TimeSig(rel, beats, beat, grids) => {
            if !cpu.bus.guest_probe(at, 8, true) {
                return false;
            }
            cpu.bus.guest_write_32(at, rel as u32);
            cpu.bus.guest_write_bytes(at + 4, &[beats, beat]);
            cpu.bus.guest_write_16(at + 6, grids);
        }
        Value::Mute => {
            // dwPChannel in, dwPChannelMap and fMute out.
            if !cpu.bus.guest_probe(at, 12, true) {
                return false;
            }
            let pchannel = cpu.bus.guest_read_32(at);
            cpu.bus.guest_write_32(at + 4, pchannel);
            cpu.bus.guest_write_32(at + 8, 0);
        }
    }
    true
}

/// Set the parameter `g` of the track `id` at `mt` from `at`.
fn set_value(cpu: &mut Cpu, id: u32, g: &Guid, mt: i32, at: u32) -> u32 {
    let Some(class) = track_of(cpu, id).map(|t| t.class) else {
        return E_INVALIDARG;
    };
    if !supports(&class, g, false) {
        return DMUS_E_SET_UNSUPPORTED;
    }
    let ev = if *g == GUID_TEMPO_PARAM {
        if at == 0 || !cpu.bus.guest_probe(at, 16, false) {
            return E_POINTER;
        }
        let mut b = [0; 8];
        cpu.bus.guest_read_bytes(at + 8, &mut b);
        Ev::Tempo {
            tempo: f64::from_le_bytes(b),
        }
    } else if *g == GUID_TIME_SIGNATURE {
        if at == 0 || !cpu.bus.guest_probe(at, 8, false) {
            return E_POINTER;
        }
        let mut b = [0; 2];
        cpu.bus.guest_read_bytes(at + 4, &mut b);
        Ev::TimeSig {
            beats: b[0].max(1),
            beat: b[1].max(1),
            grids: cpu.bus.guest_read_16(at + 6).max(1),
        }
    } else if *g == GUID_IDIRECTMUSIC_BAND {
        let Some(inst) = super::content::band_instruments(cpu, at) else {
            return E_POINTER;
        };
        Ev::Band { inst }
    } else if *g == GUID_CLEAR_ALL_BANDS {
        if let Some(t) = track_of(cpu, id) {
            t.events.retain(|e| !matches!(e.ev, Ev::Band { .. }));
        }
        return S_OK;
    } else {
        // The band track's downloads and the like, and turning tempo
        // and time signatures on and off: nothing to keep.
        return S_OK;
    };
    if let Some(t) = track_of(cpu, id) {
        let same = |e: &Timed| std::mem::discriminant(&e.ev) == std::mem::discriminant(&ev);
        t.events
            .retain(|e| !(e.mt == mt && same(e) && !matches!(e.ev, Ev::Band { .. })));
        let i = t.events.partition_point(|e| e.mt <= mt);
        t.events.insert(i, Timed { mt, ev });
    }
    S_OK
}

/// DMUS_SEG_ANYTRACK and DMUS_SEG_ALLTRACKS: any track that answers, or
/// all.
const ANY_TRACK: u32 = 0x8000_0000;

/// The tracks of the segment `id` in `groups` that do the parameter `g`.
fn param_tracks(cpu: &mut Cpu, id: u32, groups: u32, g: &Guid, get: bool) -> Vec<u32> {
    let Some(s) = segment_of(cpu, id) else {
        return Vec::new();
    };
    let tracks = s.tracks.clone();
    tracks
        .into_iter()
        .filter(|&(t, gr)| {
            gr & groups != 0 && track_of(cpu, t).is_some_and(|t| supports(&t.class, g, get))
        })
        .map(|(t, _)| t)
        .collect()
}

/// The parameter `g` of the segment `id` at `mt` to `param`, and the
/// time to its next change to `next`: of the track `which` gives (the
/// groups, and the place among those that have it).
pub fn segment_param(
    cpu: &mut Cpu,
    id: u32,
    g: &Guid,
    (groups, index): (u32, u32),
    mt: i32,
    next: u32,
    param: u32,
) -> u32 {
    let tracks = param_tracks(cpu, id, groups, g, true);
    let list: Vec<u32> = if index & ANY_TRACK != 0 {
        tracks
    } else {
        tracks.into_iter().nth(index as usize).into_iter().collect()
    };
    if list.is_empty() {
        return DMUS_E_TRACK_NOT_FOUND;
    }
    let length = segment_of(cpu, id).map_or(0, |s| s.length);
    let mut result = DMUS_E_NOT_FOUND;
    for t in list {
        let Some(track) = track_of(cpu, t) else {
            continue;
        };
        match get_value(track, g, mt) {
            Ok((value, n)) => {
                if param == 0 || !write_value(cpu, param, &value) {
                    return E_POINTER;
                }
                // With no change after, the time to the segment's end.
                let n = if n == 0 { (length - mt).max(0) } else { n };
                if next != 0 {
                    com::write_out(cpu, next, n as u32);
                }
                return S_OK;
            }
            Err(e) => result = e,
        }
    }
    result
}

/// IDirectMusicSegment::GetParam(this, guid, groups, index, time, next,
/// param).
pub fn get_param(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, guid, groups, index, mt, next, param] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(id) = this_segment(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let Some(g) = Guid::read(cpu, guid) else {
        return Some(E_POINTER);
    };
    Some(segment_param(
        cpu,
        id,
        &g,
        (groups, index),
        mt as i32,
        next,
        param,
    ))
}

/// Set the parameter `g` on the tracks of the segment `id` that `which`
/// gives.
pub fn set_segment_param(
    cpu: &mut Cpu,
    id: u32,
    g: &Guid,
    (groups, index): (u32, u32),
    mt: i32,
    param: u32,
) -> u32 {
    let tracks = param_tracks(cpu, id, groups, g, false);
    let list: Vec<u32> = if index & ANY_TRACK != 0 {
        tracks
    } else {
        tracks.into_iter().nth(index as usize).into_iter().collect()
    };
    if list.is_empty() {
        return DMUS_E_TRACK_NOT_FOUND;
    }
    let mut result = S_OK;
    for t in list {
        let hr = set_value(cpu, t, g, mt, param);
        if hr != S_OK {
            result = hr;
        }
    }
    result
}

/// IDirectMusicSegment::SetParam(this, guid, groups, index, time, param).
pub fn set_param(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, guid, groups, index, mt, param] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(id) = this_segment(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let Some(g) = Guid::read(cpu, guid) else {
        return Some(E_POINTER);
    };
    Some(set_segment_param(
        cpu,
        id,
        &g,
        (groups, index),
        mt as i32,
        param,
    ))
}

/// The events of a track from `start` to `end` (0: to its end), from
/// `start` on.
fn clip(events: &[Timed], start: i32, end: i32) -> Vec<Timed> {
    events
        .iter()
        .filter(|e| e.mt >= start && (end == 0 || e.mt < end))
        .map(|e| Timed {
            mt: e.mt - start,
            ev: e.ev.clone(),
        })
        .collect()
}

/// IDirectMusicSegment::Clone(this, start, end, segment): a segment of
/// the part from `start` to `end` (0: to the end).
pub fn clone(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, start, end, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(id) = this_segment(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_POINTER);
    }
    let (start, end) = (start as i32, end as i32);
    let s = segment_of(cpu, id)?.clone();
    if start < 0 || (end != 0 && end <= start) {
        return Some(E_INVALIDARG);
    }
    let mut tracks = Vec::new();
    for &(t, group) in &s.tracks {
        let Some(track) = track_of(cpu, t) else {
            continue;
        };
        tracks.push(TrackData {
            class: track.class,
            group,
            events: clip(&track.events, start, end),
        });
    }
    let length = if end == 0 {
        s.length - start
    } else {
        end - start
    };
    let new = new_segment(
        cpu,
        SegmentData {
            length: length.max(1),
            repeats: s.repeats,
            resolution: s.resolution,
            start: 0,
            loop_start: 0,
            loop_end: 0,
            tracks,
        },
    );
    Some(give_new(cpu, new, &IID_IDIRECTMUSIC_SEGMENT, out))
}

/// IDirectMusicSegment::SetStartPoint(this, time).
pub fn set_start_point(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_segment(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let s = segment_of(cpu, id)?;
    let start = args[1] as i32;
    if start < 0 || start >= s.length {
        return Some(DMUS_E_OUT_OF_RANGE);
    }
    s.start = start;
    Some(S_OK)
}

/// IDirectMusicSegment::GetStartPoint(this, time).
pub fn get_start_point(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_segment(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let v = segment_of(cpu, id)?.start as u32;
    answer(cpu, args[1], v)
}

/// IDirectMusicSegment::SetLoopPoints(this, start, end): 0 and 0 loop
/// the whole segment.
pub fn set_loop_points(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_segment(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let s = segment_of(cpu, id)?;
    let (start, end) = (args[1] as i32, args[2] as i32);
    if (start, end) != (0, 0) && (start < 0 || end > s.length || start >= end) {
        return Some(DMUS_E_OUT_OF_RANGE);
    }
    s.loop_start = start;
    s.loop_end = end;
    Some(S_OK)
}

/// IDirectMusicSegment::GetLoopPoints(this, start, end).
pub fn get_loop_points(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_segment(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let (start, end) = {
        let s = segment_of(cpu, id)?;
        (s.loop_start, s.loop_end)
    };
    if args[1] == 0 || args[2] == 0 {
        return Some(E_POINTER);
    }
    com::write_out(cpu, args[1], start as u32);
    com::write_out(cpu, args[2], end as u32);
    Some(S_OK)
}

/// The tools of the graph `this` (a graph, or a performance's own).
fn tools(cpu: &mut Cpu, this: u32) -> Option<&mut Vec<u32>> {
    let (id, iface) = interface(cpu, this)?;
    if iface != GRAPH {
        return None;
    }
    match &mut state(cpu).get_mut(id)?.kind {
        Kind::Graph { tools } => Some(tools),
        Kind::Performance { p } => Some(&mut p.tools),
        _ => None,
    }
}

/// IDirectMusicGraph::InsertTool(this, tool, channels, count, index):
/// the tool is kept at its place (negative from the end), not held.
pub fn insert_tool(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let tool = args[1];
    if tool == 0 {
        return Some(E_POINTER);
    }
    let Some(list) = tools(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if list.contains(&tool) {
        return Some(DMUS_E_ALREADY_EXISTS);
    }
    let index = args[4] as i32;
    let at = if index < 0 {
        (list.len() as i32 + 1 + index).max(0) as usize
    } else {
        (index as usize).min(list.len())
    };
    list.insert(at, tool);
    Some(S_OK)
}

const DMUS_E_ALREADY_EXISTS: u32 = dmus_e(0x201);

/// IDirectMusicGraph::GetTool(this, index, tool): the pointer the
/// program gave, without a reference taken.
pub fn get_tool(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(list) = tools(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(&tool) = list.get(args[1] as usize) else {
        return Some(DMUS_E_NOT_FOUND);
    };
    answer(cpu, args[2], tool)
}

/// IDirectMusicGraph::RemoveTool(this, tool).
pub fn remove_tool(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(list) = tools(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(i) = list.iter().position(|&t| t == args[1]) else {
        return Some(DMUS_E_NOT_FOUND);
    };
    list.remove(i);
    Some(S_OK)
}

/// IDirectMusicTrack::InitPlay(this, state, performance, data, track id,
/// flags): no state data.
pub fn track_init_play(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if this_of(cpu, args[0], TRACK).is_none() {
        return Some(E_INVALIDARG);
    }
    com::write_out(cpu, args[3], 0);
    Some(S_OK)
}

/// IDirectMusicTrack::GetParam(this, guid, time, next, param).
pub fn track_get_param(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, guid, mt, next, param] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(id) = this_of(cpu, this, TRACK) else {
        return Some(E_INVALIDARG);
    };
    let Some(g) = Guid::read(cpu, guid) else {
        return Some(E_POINTER);
    };
    let track = track_of(cpu, id)?;
    match get_value(track, &g, mt as i32) {
        Ok((value, n)) => {
            if param == 0 || !write_value(cpu, param, &value) {
                return Some(E_POINTER);
            }
            if next != 0 {
                com::write_out(cpu, next, n as u32);
            }
            Some(S_OK)
        }
        Err(e) => Some(e),
    }
}

/// IDirectMusicTrack::SetParam(this, guid, time, param).
pub fn track_set_param(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_of(cpu, args[0], TRACK) else {
        return Some(E_INVALIDARG);
    };
    let Some(g) = Guid::read(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    Some(set_value(cpu, id, &g, args[2] as i32, args[3]))
}

/// IDirectMusicTrack::IsParamSupported(this, guid).
pub fn track_is_param_supported(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_of(cpu, args[0], TRACK) else {
        return Some(E_INVALIDARG);
    };
    let Some(g) = Guid::read(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    let class = track_of(cpu, id)?.class;
    Some(
        if supports(&class, &g, true) || supports(&class, &g, false) {
            S_OK
        } else {
            DMUS_E_TYPE_UNSUPPORTED
        },
    )
}

/// IDirectMusicTrack::Clone(this, start, end, track).
pub fn track_clone(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, start, end, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(id) = this_of(cpu, this, TRACK) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_POINTER);
    }
    let (start, end) = (start as i32, end as i32);
    if start < 0 || (end != 0 && end <= start) {
        return Some(E_INVALIDARG);
    }
    let t = track_of(cpu, id)?.clone();
    let new = new_track(
        cpu,
        Track {
            class: t.class,
            events: clip(&t.events, start, end),
        },
    );
    Some(give_new(cpu, new, &IID_IDIRECTMUSIC_TRACK, out))
}

crate::state_fields!(Segment {
    length,
    repeats,
    resolution,
    start,
    loop_start,
    loop_end,
    tracks,
    graph,
    notifications
});
crate::state_fields!(Track { class, events });
