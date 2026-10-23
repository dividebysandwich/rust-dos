//! DMIME.DLL's performance and segment states: playing segments.
//!
//! The performance keeps music time (768 ticks a beat, `files::PPQ`)
//! against the master clock's reference time (100 ns units, the emulated
//! clock): an anchor (music time, reference time, tempo) that each tempo
//! the music plays moves on. A segment played becomes a segment state
//! with the events of its tracks; the process's multimedia thread
//! (winmm/callback.rs) sends each at its time (`poll`), on the MIDI
//! channel its performance channel is assigned to. Notifications are
//! queued with their times, and the program's event is set when one
//! comes.
//!
//! Segments start at once (the latency time, 100 ms on) or at a time
//! the program gives, aligned to a grid, beat or measure as its flags
//! say. A primary segment ends the one before it; secondary ones play
//! along.

use super::core::{self, LATENCY, now_rt};
use super::files::{Ev, PPQ, Timed};
use super::guids::*;
use super::{
    E_INVALIDARG, E_POINTER, Kind, MUSIC, PERFORMANCE, PORT, S_FALSE, S_OK, SEGMENT, SEGMENT_STATE,
    State, add, alloc, arg64, dmus_e, drop_ref, free, give_new, hold, object_of, pointer, state,
    this_of, write_u64,
};
use crate::cpu::Cpu;
use crate::win32::com::{self, Guid};
use crate::win32::winmm;

pub const DMUS_E_CANNOT_FREE: u32 = dmus_e(0x205);
pub const DMUS_E_ALREADY_INITED: u32 = dmus_e(0x151);
pub const DMUS_E_NOT_INIT: u32 = dmus_e(0x162);
pub const DMUS_E_NO_MASTER_CLOCK: u32 = dmus_e(0x170);
use super::DMUS_E_NOT_FOUND;

/// DMUS_SEGF_ flags of PlaySegment and Stop.
const SEGF_REFTIME: u32 = 1 << 6;
const SEGF_SECONDARY: u32 = 1 << 7;
const SEGF_QUEUE: u32 = 1 << 8;
const SEGF_CONTROL: u32 = 1 << 9;
const SEGF_GRID: u32 = 1 << 11;
const SEGF_BEAT: u32 = 1 << 12;
const SEGF_MEASURE: u32 = 1 << 13;
const SEGF_DEFAULT: u32 = 1 << 14;
/// DMUS_SEG_REPEAT_INFINITE.
const REPEAT_INFINITE: u32 = 0xFFFF_FFFF;

/// DMUS_PMSG_PART's size, a notification's (DMUS_NOTIFICATION_PMSG), and
/// the places of its fields.
const PMSG: u32 = 56;
const NOTIFICATION_PMSG: u32 = 88;
const PM_RTIME: u32 = 8;
const PM_MTIME: u32 = 16;
const PM_FLAGS: u32 = 20;
const PM_PCHANNEL: u32 = 24;
const PM_TYPE: u32 = 40;
const PM_GROUP: u32 = 48;
const PM_USER: u32 = 52;
/// DMUS_PMSGF_ flags: the times it has.
const PMSGF_REFTIME: u32 = 1;
const PMSGF_MUSICTIME: u32 = 2;
/// DMUS_PMSGT_ types.
const PMSGT_MIDI: u32 = 0;
const PMSGT_NOTE: u32 = 1;
const PMSGT_SYSEX: u32 = 2;
const PMSGT_NOTIFICATION: u32 = 3;
const PMSGT_TEMPO: u32 = 4;
const PMSGT_CURVE: u32 = 5;
const PMSGT_TIMESIG: u32 = 6;
const PMSGT_PATCH: u32 = 7;

/// DMUS_NOTIFICATION_ options: of the performance, of a segment, and a
/// measure's beat.
const MUSICSTARTED: u32 = 0;
const MUSICSTOPPED: u32 = 1;
const SEGSTART: u32 = 0;
const SEGEND: u32 = 1;
const SEGALMOSTEND: u32 = 2;
const SEGLOOP: u32 = 3;
const SEGABORT: u32 = 4;
const MEASUREBEAT: u32 = 0;

/// A performance channel's MIDI channel: on a port, in a channel group.
#[derive(Clone, Debug, Default)]
pub struct Assign {
    pub pchannel: u32,
    pub port: u32,
    pub group: u32,
    pub channel: u32,
}

/// A notification queued for the program, with the segment state it is
/// of (held), or 0.
#[derive(Clone, Debug, Default)]
pub struct Notice {
    pub mt: i64,
    pub rt: i64,
    pub guid: [u8; 16],
    pub option: u32,
    pub state: u32,
    pub field1: u32,
    pub field2: u32,
}

/// A performance.
#[derive(Clone, Debug, Default)]
pub struct Perf {
    /// Its DirectMusic (held), 0 before Init.
    pub music: u32,
    pub inited: bool,
    /// The ports added (held), and the one AddPort(NULL) made.
    pub ports: Vec<u32>,
    pub default_port: u32,
    pub assigned: Vec<Assign>,
    /// Prepare time and bumper length, in milliseconds.
    pub prepare: u32,
    pub bumper: u32,
    /// Music time, reference time and tempo, by which one gives the
    /// other.
    pub anchor: (i64, i64, f64),
    /// The time signature the music plays in.
    pub sig: (u8, u8, u16),
    /// The segment states playing or to play (held).
    pub states: Vec<u32>,
    /// Notes to end: music time, channel, note.
    pub offs: Vec<(i64, u8, u8)>,
    /// What SendPMsg queued, by music time.
    pub sent: Vec<(i64, Timed)>,
    /// The notification types asked for, the program's event (by its
    /// object, held), the notifications for the program, and the
    /// messages with it (AllocPMsg's and notifications).
    pub types: Vec<[u8; 16]>,
    pub event: u32,
    pub ready: Vec<Notice>,
    pub pmsgs: Vec<u32>,
    /// SetGlobalParam's parameters.
    pub params: Vec<([u8; 16], Vec<u8>)>,
    /// Its graph (held) and the tools of its own graph (not held).
    pub graph: u32,
    pub tools: Vec<u32>,
}

impl Perf {
    pub fn held(&self) -> Vec<u32> {
        let mut out = Vec::new();
        out.extend((self.music != 0).then_some(self.music));
        out.extend(&self.ports);
        out.extend(&self.states);
        out.extend(self.ready.iter().filter(|n| n.state != 0).map(|n| n.state));
        out.extend((self.graph != 0).then_some(self.graph));
        out
    }

    /// The master tempo's scale (GUID_PerfMasterTempo).
    fn master_tempo(&self) -> f64 {
        self.param(&GUID_PERF_MASTER_TEMPO)
            .and_then(|b| Some(f32::from_le_bytes(b.get(..4)?.try_into().ok()?)))
            .map_or(1.0, |t| (t as f64).clamp(0.01, 100.0))
    }

    fn param(&self, g: &Guid) -> Option<&[u8]> {
        self.params
            .iter()
            .find(|p| p.0 == g.0)
            .map(|p| p.1.as_slice())
    }

    /// Ticks of music time a reference time unit, at the tempo.
    fn rate(&self) -> f64 {
        let tempo = (self.anchor.2 * self.master_tempo()).max(0.001);
        tempo * PPQ as f64 / 600_000_000.0
    }

    /// The reference time of the music time `mt`.
    pub fn rt_of(&self, mt: i64) -> i64 {
        let (amt, art, _) = self.anchor;
        art.saturating_add(((mt - amt) as f64 / self.rate()).round() as i64)
    }

    /// The music time of the reference time `rt`.
    pub fn mt_of(&self, rt: i64) -> i64 {
        let (amt, art, _) = self.anchor;
        amt.saturating_add(((rt - art) as f64 * self.rate()).floor() as i64)
    }

    /// The tempo is `tempo` from the music time `mt`.
    fn set_tempo(&mut self, mt: i64, tempo: f64) {
        let rt = self.rt_of(mt);
        self.anchor = (mt, rt, tempo.max(0.001));
    }

    /// The MIDI channel the performance channel `pchannel` plays on.
    fn channel(&self, pchannel: u32) -> Option<u8> {
        self.assigned
            .iter()
            .find(|a| a.pchannel == pchannel)
            .map(|a| a.channel as u8)
    }

    fn wants(&self, g: &Guid) -> bool {
        self.types.contains(&g.0)
    }

    /// Ticks of a beat, of a measure, and of a grid, by the time
    /// signature.
    fn lengths(&self) -> (i64, i64, i64) {
        let (beats, beat, grids) = self.sig;
        let beat_len = (PPQ as i64 * 4 / beat.max(1) as i64).max(1);
        (
            beat_len,
            beat_len * beats.max(1) as i64,
            (beat_len / grids.max(1) as i64).max(1),
        )
    }
}

/// A segment playing (a segment state): its performance and segment,
/// flags, times, repeats, the events of its tracks and where it is.
#[derive(Clone, Debug, Default)]
pub struct Playing {
    pub perf: u32,
    pub segment: u32,
    pub flags: u32,
    /// The music time it starts at, and the music time of the segment's
    /// time 0 in the repeat it plays.
    pub start: i64,
    pub offset: i64,
    pub start_point: i32,
    pub length: i32,
    pub loop_start: i32,
    pub loop_end: i32,
    pub repeats: u32,
    pub left: u32,
    pub events: Vec<Timed>,
    pub index: u32,
    /// 0 before it starts, 1 playing, 2 after its almost-end
    /// notification, 3 ended.
    pub stage: u8,
    pub next_beat: i64,
}

impl Playing {
    fn primary(&self) -> bool {
        self.flags & SEGF_SECONDARY == 0
    }

    /// Where the repeat it plays ends, in the segment's time.
    fn pass_end(&self) -> i32 {
        if self.left > 0 {
            if self.loop_end > 0 {
                self.loop_end
            } else {
                self.length
            }
        } else {
            self.length
        }
    }

    /// The music time it ends at, if its repeats end.
    fn end(&self) -> Option<i64> {
        if self.left == REPEAT_INFINITE {
            return None;
        }
        let (ls, le) = self.loop_span();
        Some(self.offset + self.length as i64 + self.left as i64 * (le - ls) as i64)
    }

    fn loop_span(&self) -> (i32, i32) {
        let end = if self.loop_end > 0 {
            self.loop_end
        } else {
            self.length
        };
        (self.loop_start, end)
    }
}

/// What a performance does next.
#[derive(Clone, Copy, Debug)]
enum Item {
    Start(u32),
    Event(u32),
    AlmostEnd(u32),
    Boundary(u32),
    Beat(u32),
    Off(usize),
    Sent(usize),
}

fn perf_of(s: &State, id: u32) -> Option<&Perf> {
    match &s.get(id)?.kind {
        Kind::Performance { p } => Some(p),
        _ => None,
    }
}

fn perf_mut(cpu: &mut Cpu, id: u32) -> Option<&mut Perf> {
    match &mut state(cpu).get_mut(id)?.kind {
        Kind::Performance { p } => Some(p),
        _ => None,
    }
}

fn playing_of(s: &State, id: u32) -> Option<&Playing> {
    match &s.get(id)?.kind {
        Kind::SegmentState { s } => Some(s),
        _ => None,
    }
}

fn playing_mut(cpu: &mut Cpu, id: u32) -> Option<&mut Playing> {
    match &mut state(cpu).get_mut(id)?.kind {
        Kind::SegmentState { s } => Some(s),
        _ => None,
    }
}

/// The next thing the performance `p` does, and its reference time.
fn next_item(s: &State, p: &Perf) -> Option<(i64, Item)> {
    let mut best: Option<(i64, Item)> = None;
    let mut consider = |rt: i64, item: Item| {
        if best.is_none_or(|b| rt < b.0) {
            best = Some((rt, item));
        }
    };
    for (i, off) in p.offs.iter().enumerate() {
        consider(p.rt_of(off.0), Item::Off(i));
    }
    for (i, e) in p.sent.iter().enumerate() {
        consider(p.rt_of(e.0), Item::Sent(i));
    }
    let (beat_len, _, _) = p.lengths();
    let _ = beat_len;
    for &id in &p.states {
        let Some(pl) = playing_of(s, id) else {
            continue;
        };
        match pl.stage {
            0 => consider(p.rt_of(pl.start), Item::Start(id)),
            1 | 2 => {
                let pass_end = pl.pass_end();
                if let Some(e) = pl.events.get(pl.index as usize).filter(|e| e.mt < pass_end) {
                    consider(p.rt_of(pl.offset + e.mt as i64), Item::Event(id));
                }
                let end_rt = p.rt_of(pl.offset + pass_end as i64);
                if pl.stage == 1 && pl.left == 0 {
                    let ahead = (p.prepare as i64 * 10_000 - LATENCY).max(0);
                    consider((end_rt - ahead).max(p.rt_of(pl.start)), Item::AlmostEnd(id));
                }
                consider(end_rt, Item::Boundary(id));
                if pl.primary() && p.wants(&GUID_NOTIFICATION_MEASURE_AND_BEAT) {
                    consider(p.rt_of(pl.next_beat), Item::Beat(id));
                }
            }
            _ => {}
        }
    }
    best
}

/// Queue a notification of the performance `perf` and set its event.
fn notify(cpu: &mut Cpu, perf: u32, guid: Guid, option: u32, st: u32, mt: i64, fields: (u32, u32)) {
    if st != 0 {
        hold(cpu, st);
    }
    let Some(p) = perf_mut(cpu, perf) else {
        return;
    };
    let rt = p.rt_of(mt);
    p.ready.push(Notice {
        mt,
        rt,
        guid: guid.0,
        option,
        state: st,
        field1: fields.0,
        field2: fields.1,
    });
    let event = p.event;
    if event != 0 {
        winmm::callback::set_event(cpu, event);
    }
}

/// Whether notifications of segments are asked for the state `st`: of
/// the performance, or of its segment.
fn segment_notes(cpu: &mut Cpu, perf: u32, st: u32) -> bool {
    let s = state(cpu);
    let of_perf = perf_of(s, perf).is_some_and(|p| p.wants(&GUID_NOTIFICATION_SEGMENT));
    let seg = playing_of(s, st).map_or(0, |pl| pl.segment);
    let of_seg = match s.get(seg).map(|o| &o.kind) {
        Some(Kind::Segment { s }) => s.notifications.contains(&GUID_NOTIFICATION_SEGMENT.0),
        _ => false,
    };
    of_perf || of_seg
}

fn perf_notes(cpu: &mut Cpu, perf: u32) -> bool {
    perf_of(state(cpu), perf).is_some_and(|p| p.wants(&GUID_NOTIFICATION_PERFORMANCE))
}

/// Send a MIDI message for the performance channel `pchannel`.
fn send(cpu: &mut Cpu, channel: Option<u8>, bytes: &[u8]) {
    let Some(&status) = bytes.first() else {
        return;
    };
    if status >= 0xF0 {
        winmm::midi::send(cpu, bytes);
        return;
    }
    let Some(c) = channel else {
        return;
    };
    let mut msg = bytes.to_vec();
    msg[0] = (status & 0xF0) | (c & 0x0F);
    winmm::midi::send(cpu, &msg);
}

/// Play the event `e` at the music time `mt`; `tempo` if its tempo
/// counts.
fn play(cpu: &mut Cpu, perf: u32, mt: i64, e: &Ev, tempo: bool) {
    let Some(p) = perf_mut(cpu, perf) else {
        return;
    };
    match e {
        Ev::Midi { pchannel, bytes } => {
            let c = p.channel(*pchannel);
            send(cpu, c, bytes);
        }
        Ev::Note {
            pchannel,
            note,
            velocity,
            duration,
        } => {
            let Some(c) = p.channel(*pchannel) else {
                return;
            };
            let off = (mt + *duration as i64, c, *note);
            let i = p.offs.partition_point(|o| o.0 <= off.0);
            p.offs.insert(i, off);
            send(cpu, Some(c), &[0x90, *note, *velocity]);
        }
        Ev::Tempo { tempo: t } => {
            if tempo {
                p.set_tempo(mt, *t);
            }
        }
        Ev::TimeSig { beats, beat, grids } => {
            if tempo {
                p.sig = (*beats, *beat, *grids);
            }
        }
        Ev::Band { inst } => {
            let p = p.clone();
            for msg in core::band_messages(inst, |pc| p.channel(pc)) {
                winmm::midi::send(cpu, &msg);
            }
        }
    }
}

/// Stop the state `st` of the performance `perf`: out of its list, and
/// a SEGABORT notification if `abort`.
fn end_state(cpu: &mut Cpu, perf: u32, st: u32, option: Option<u32>) {
    let now = now_rt(cpu);
    let mt = perf_of(state(cpu), perf).map_or(0, |p| p.mt_of(now));
    let at = playing_of(state(cpu), st)
        .and_then(|pl| match option {
            Some(SEGEND) => pl.end(),
            _ => None,
        })
        .unwrap_or(mt);
    if let Some(pl) = playing_mut(cpu, st) {
        pl.stage = 3;
    }
    if let Some(o) = option
        && segment_notes(cpu, perf, st)
    {
        notify(cpu, perf, GUID_NOTIFICATION_SEGMENT, o, st, at, (0, 0));
    }
    if let Some(p) = perf_mut(cpu, perf)
        && let Some(i) = p.states.iter().position(|&s| s == st)
    {
        p.states.remove(i);
        drop_ref(cpu, st);
    }
}

/// Whether a state of the performance other than `but` has started.
fn others_playing(cpu: &mut Cpu, perf: u32, but: u32) -> bool {
    let s = state(cpu);
    perf_of(s, perf).is_some_and(|p| {
        p.states
            .iter()
            .any(|&id| id != but && playing_of(s, id).is_some_and(|pl| (1..=2).contains(&pl.stage)))
    })
}

/// Do what the performance `perf` has due by the reference time `rt`.
fn run(cpu: &mut Cpu, perf: u32, rt: i64) {
    // Not more than this many at once, so a busy segment can't hold up
    // the thread for good.
    for _ in 0..100_000 {
        let s = state(cpu);
        let Some(p) = perf_of(s, perf).filter(|p| p.inited) else {
            return;
        };
        let Some((due, item)) = next_item(s, p) else {
            return;
        };
        if due > rt {
            return;
        }
        step(cpu, perf, item);
    }
}

/// Do the item `item` of the performance `perf`.
fn step(cpu: &mut Cpu, perf: u32, item: Item) {
    match item {
        Item::Off(i) => {
            let Some(p) = perf_mut(cpu, perf) else {
                return;
            };
            let (_, c, note) = p.offs.remove(i);
            send(cpu, Some(c), &[0x80, note, 0]);
        }
        Item::Sent(i) => {
            let Some(p) = perf_mut(cpu, perf) else {
                return;
            };
            let (mt, e) = p.sent.remove(i);
            play(cpu, perf, mt, &e.ev, true);
        }
        Item::Start(st) => start_state(cpu, perf, st),
        Item::Event(st) => {
            let Some(pl) = playing_mut(cpu, st) else {
                return;
            };
            let Some(e) = pl.events.get(pl.index as usize).cloned() else {
                return;
            };
            pl.index += 1;
            let mt = pl.offset + e.mt as i64;
            let tempo = pl.primary() || pl.flags & SEGF_CONTROL != 0;
            play(cpu, perf, mt, &e.ev, tempo);
        }
        Item::AlmostEnd(st) => {
            let Some(pl) = playing_mut(cpu, st) else {
                return;
            };
            pl.stage = 2;
            let mt = pl.offset + pl.length as i64;
            let at = perf_of(state(cpu), perf).map_or(mt, |p| {
                let ahead = (p.prepare as i64 * 10_000 - LATENCY).max(0);
                p.mt_of(p.rt_of(mt) - ahead)
            });
            if segment_notes(cpu, perf, st) {
                notify(
                    cpu,
                    perf,
                    GUID_NOTIFICATION_SEGMENT,
                    SEGALMOSTEND,
                    st,
                    at,
                    (0, 0),
                );
            }
        }
        Item::Boundary(st) => {
            let Some(pl) = playing_mut(cpu, st) else {
                return;
            };
            if pl.left > 0 {
                let (ls, le) = pl.loop_span();
                pl.offset += (le - ls) as i64;
                pl.index = pl.events.partition_point(|e| e.mt < ls) as u32;
                if pl.left != REPEAT_INFINITE {
                    pl.left -= 1;
                }
                let mt = pl.offset + ls as i64;
                if segment_notes(cpu, perf, st) {
                    notify(
                        cpu,
                        perf,
                        GUID_NOTIFICATION_SEGMENT,
                        SEGLOOP,
                        st,
                        mt,
                        (0, 0),
                    );
                }
                return;
            }
            let end = pl.offset + pl.length as i64;
            if !others_playing(cpu, perf, st) && perf_notes(cpu, perf) {
                let guid = GUID_NOTIFICATION_PERFORMANCE;
                notify(cpu, perf, guid, MUSICSTOPPED, 0, end, (0, 0));
            }
            end_state(cpu, perf, st, Some(SEGEND));
        }
        Item::Beat(st) => {
            let Some(p) = perf_of(state(cpu), perf) else {
                return;
            };
            let (beat_len, measure_len, _) = p.lengths();
            let Some(pl) = playing_mut(cpu, st) else {
                return;
            };
            let at = pl.next_beat;
            let from = at - pl.start;
            pl.next_beat += beat_len;
            let beat = (from % measure_len / beat_len) as u32;
            let measure = (from / measure_len) as u32;
            let guid = GUID_NOTIFICATION_MEASURE_AND_BEAT;
            notify(cpu, perf, guid, MEASUREBEAT, 0, at, (beat, measure));
        }
    }
}

/// The state `st` starts: a primary segment ends the one before it.
fn start_state(cpu: &mut Cpu, perf: u32, st: u32) {
    let Some(pl) = playing_mut(cpu, st) else {
        return;
    };
    pl.stage = 1;
    pl.next_beat = pl.start;
    let (start, primary) = (pl.start, pl.primary());
    if primary {
        let s = state(cpu);
        let before: Vec<u32> = perf_of(s, perf)
            .map(|p| {
                p.states
                    .iter()
                    .copied()
                    .filter(|&id| {
                        id != st && playing_of(s, id).is_some_and(|o| o.primary() && o.stage >= 1)
                    })
                    .collect()
            })
            .unwrap_or_default();
        for id in before {
            end_state(cpu, perf, id, Some(SEGABORT));
        }
    }
    if !others_playing(cpu, perf, st) && perf_notes(cpu, perf) {
        let guid = GUID_NOTIFICATION_PERFORMANCE;
        notify(cpu, perf, guid, MUSICSTARTED, 0, start, (0, 0));
    }
    if segment_notes(cpu, perf, st) {
        notify(
            cpu,
            perf,
            GUID_NOTIFICATION_SEGMENT,
            SEGSTART,
            st,
            start,
            (0, 0),
        );
    }
}

/// Do what the performance `perf` has due now, for a call of the
/// program that looks at what plays.
fn catch_up(cpu: &mut Cpu, perf: u32) {
    let now = now_rt(cpu);
    run(cpu, perf, now);
}

/// Send what the performances and ports of the process `pid` have due at
/// `now` (microseconds).
pub fn poll(cpu: &mut Cpu, pid: u32, now: u64) {
    let rt = (now as i64).saturating_mul(10);
    let perfs: Vec<u32> = state(cpu)
        .objects
        .iter()
        .filter(|o| o.pid == pid && matches!(o.kind, Kind::Performance { .. }))
        .map(|o| o.id)
        .collect();
    for perf in perfs {
        run(cpu, perf, rt);
    }
    core::poll_ports(cpu, pid, rt);
}

/// When the performances and ports of the process `pid` (any, for None)
/// next have something due, in microseconds.
pub fn next_due(s: &State, pid: Option<u32>) -> Option<u64> {
    let perfs = s
        .objects
        .iter()
        .filter(|o| pid.is_none_or(|p| p == o.pid))
        .filter_map(|o| match &o.kind {
            Kind::Performance { p } if p.inited => next_item(s, p).map(|n| n.0),
            _ => None,
        });
    perfs
        .chain(core::ports_due(s, pid))
        .min()
        .map(|rt| (rt.max(0) as u64).div_ceil(10))
}

/// The scale the performances' master volume puts on the synthesizer:
/// GUID_PerfMasterVolume in hundredths of a decibel.
pub fn gain(s: &State) -> f32 {
    s.objects
        .iter()
        .filter_map(|o| match &o.kind {
            Kind::Performance { p } => p.param(&GUID_PERF_MASTER_VOLUME),
            _ => None,
        })
        .filter_map(|b| Some(i32::from_le_bytes(b.get(..4)?.try_into().ok()?)))
        .map(|v| 10f32.powf(v.clamp(-10_000, 0) as f32 / 2000.0))
        .product()
}

/// CoCreateInstance of a performance.
pub fn create(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let p = Perf {
        prepare: 1000,
        bumper: 50,
        anchor: (0, 0, 120.0),
        sig: (4, 4, 4),
        // AutoDownload is there, off, before it is set.
        params: vec![(GUID_PERF_AUTO_DOWNLOAD.0, vec![0; 4])],
        ..Perf::default()
    };
    let id = add(cpu, Kind::Performance { p });
    give_new(cpu, id, iid, out)
}

/// CoCreateInstance of a segment state: one that plays nothing.
pub fn create_state(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = add(
        cpu,
        Kind::SegmentState {
            s: Playing {
                stage: 3,
                ..Playing::default()
            },
        },
    );
    give_new(cpu, id, iid, out)
}

/// The performance `this` is.
fn this_perf(cpu: &mut Cpu, this: u32) -> Option<u32> {
    this_of(cpu, this, PERFORMANCE)
}

/// The performance `this` is, if Init was called on it: Err with the
/// HRESULT if not.
fn inited(cpu: &mut Cpu, this: u32, not: u32) -> Result<u32, u32> {
    let id = this_perf(cpu, this).ok_or(E_INVALIDARG)?;
    if !perf_mut(cpu, id).is_some_and(|p| p.inited) {
        return Err(not);
    }
    Ok(id)
}

/// Stop the music and the notes of the performance `perf`, as it closes
/// or goes: its states let go, its event too.
pub fn close(cpu: &mut Cpu, perf: u32) {
    let Some(p) = perf_mut(cpu, perf) else {
        return;
    };
    let playing = !p.states.is_empty() || !p.offs.is_empty();
    let event = std::mem::take(&mut p.event);
    p.offs.clear();
    p.sent.clear();
    if event != 0 {
        crate::win32::object::objects(cpu).release(event);
    }
    if playing {
        winmm::midi::notes_off(cpu);
    }
}

/// IDirectMusicPerformance::Init(this, music, dsound, window).
pub fn init(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, music, _, _] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(id) = this_perf(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if perf_mut(cpu, id)?.inited {
        return Some(DMUS_E_ALREADY_INITED);
    }
    let given = if music != 0 && cpu.bus.guest_probe(music, 4, true) {
        cpu.bus.guest_read_32(music)
    } else {
        0
    };
    let dm = if given != 0 {
        let Some(dm) = this_of(cpu, given, MUSIC) else {
            return Some(E_INVALIDARG);
        };
        hold(cpu, dm);
        dm
    } else {
        let dm = core::new_music(cpu);
        if music != 0 {
            let at = pointer(cpu, dm);
            com::write_out(cpu, music, at);
        }
        dm
    };
    core::music_has_dsound(cpu, dm);
    winmm::callback::ensure_thread(cpu);
    let now = now_rt(cpu);
    let p = perf_mut(cpu, id)?;
    p.music = dm;
    p.inited = true;
    p.anchor = (0, now, 120.0);
    Some(S_OK)
}

/// The music time a segment played at `start` (0: as soon as it can)
/// with `flags` starts at.
fn start_time(cpu: &mut Cpu, perf: u32, start: i64, flags: u32, resolution: u32) -> i64 {
    let now = now_rt(cpu);
    let s = state(cpu);
    let Some(p) = perf_of(s, perf) else {
        return 0;
    };
    let queue = p.mt_of(now + LATENCY);
    let primary = flags & SEGF_SECONDARY == 0;
    let primaries: Vec<&Playing> = p
        .states
        .iter()
        .filter_map(|&id| playing_of(s, id))
        .filter(|pl| pl.primary() && pl.stage < 3)
        .collect();
    let mut mt = if start == 0 {
        queue
    } else if flags & SEGF_REFTIME != 0 {
        p.mt_of(start)
    } else {
        start
    }
    .max(queue);
    if primary
        && flags & SEGF_QUEUE != 0
        && let Some(end) = primaries.iter().filter_map(|pl| pl.end()).max()
    {
        return end.max(queue);
    }
    let res = if flags & SEGF_DEFAULT != 0 {
        resolution
    } else {
        flags
    };
    let (beat, measure, grid) = p.lengths();
    let unit = if res & SEGF_MEASURE != 0 {
        measure
    } else if res & SEGF_BEAT != 0 {
        beat
    } else if res & SEGF_GRID != 0 {
        grid
    } else {
        1
    };
    if unit > 1 {
        let origin = primaries.first().map_or(0, |pl| pl.start);
        let from = mt - origin;
        mt = origin + (from + unit - 1).div_euclid(unit) * unit;
    }
    mt
}

/// IDirectMusicPerformance::PlaySegment(this, segment, flags, start (2
/// words), state).
pub fn play_segment(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, seg, flags, lo, hi, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let perf = match inited(cpu, this, DMUS_E_NOT_INIT) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let Some(seg) = this_of(cpu, seg, SEGMENT) else {
        return Some(E_POINTER);
    };
    if out != 0 {
        com::write_out(cpu, out, 0);
    }
    catch_up(cpu, perf);
    let s = super::segment::segment_of(cpu, seg)?.clone();
    let start = start_time(cpu, perf, arg64(lo, hi), flags, s.resolution);
    let events = super::segment::events(cpu, seg);
    let index = events.partition_point(|e| e.mt < s.start) as u32;
    let pl = Playing {
        perf,
        segment: seg,
        flags,
        start,
        offset: start - s.start as i64,
        start_point: s.start,
        length: s.length,
        loop_start: s.loop_start,
        loop_end: s.loop_end,
        repeats: s.repeats,
        left: s.repeats,
        events,
        index,
        stage: 0,
        next_beat: start,
    };
    hold(cpu, seg);
    let st = add(cpu, Kind::SegmentState { s: pl });
    perf_mut(cpu, perf)?.states.push(st);
    if out != 0 {
        let at = pointer(cpu, st);
        com::write_out(cpu, out, at);
    }
    winmm::schedule(cpu);
    Some(S_OK)
}

/// IDirectMusicPerformance::Stop(this, segment, state, time, flags):
/// both NULL stop all.
pub fn stop(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, seg, st, _, _] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(perf) = this_perf(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    catch_up(cpu, perf);
    let seg = if seg != 0 {
        this_of(cpu, seg, SEGMENT)
    } else {
        None
    };
    let st = if st != 0 {
        this_of(cpu, st, SEGMENT_STATE)
    } else {
        None
    };
    let now = now_rt(cpu);
    let s = state(cpu);
    let Some(p) = perf_of(s, perf) else {
        return Some(E_INVALIDARG);
    };
    let all = args[1] == 0 && args[2] == 0;
    let stopping: Vec<u32> = p
        .states
        .iter()
        .copied()
        .filter(|&id| {
            all || Some(id) == st || playing_of(s, id).is_some_and(|pl| Some(pl.segment) == seg)
        })
        .collect();
    if stopping.is_empty() {
        return Some(S_OK);
    }
    let now = p.mt_of(now);
    // Those that hadn't started yet start and stop.
    for &id in &stopping {
        if playing_of(state(cpu), id).is_some_and(|pl| pl.stage == 0) {
            start_state(cpu, perf, id);
        }
    }
    let remaining = perf_of(state(cpu), perf).map_or(0, |p| {
        p.states.iter().filter(|id| !stopping.contains(id)).count()
    });
    if remaining == 0 && perf_notes(cpu, perf) {
        let guid = GUID_NOTIFICATION_PERFORMANCE;
        notify(cpu, perf, guid, MUSICSTOPPED, 0, now, (0, 0));
    }
    for id in stopping {
        end_state(cpu, perf, id, Some(SEGABORT));
    }
    let offs = std::mem::take(&mut perf_mut(cpu, perf)?.offs);
    for (_, c, note) in offs {
        send(cpu, Some(c), &[0x80, note, 0]);
    }
    if remaining == 0 {
        winmm::midi::notes_off(cpu);
    }
    winmm::schedule(cpu);
    Some(S_OK)
}

/// The primary (or control) state playing at the music time `mt`, the
/// latest started.
fn state_at(s: &State, p: &Perf, mt: i64) -> Option<u32> {
    p.states.iter().rev().copied().find(|&id| {
        playing_of(s, id).is_some_and(|pl| {
            (pl.primary() || pl.flags & SEGF_CONTROL != 0)
                && pl.start <= mt
                && pl.end().is_none_or(|e| mt < e)
        })
    })
}

/// IDirectMusicPerformance::GetSegmentState(this, state, time).
pub fn get_segment_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    catch_up(cpu, perf);
    let s = state(cpu);
    let found = perf_of(s, perf).and_then(|p| state_at(s, p, args[2] as i32 as i64));
    let Some(st) = found else {
        com::write_out(cpu, args[1], 0);
        return Some(DMUS_E_NOT_FOUND);
    };
    let at = pointer(cpu, st);
    com::write_out(cpu, args[1], at);
    Some(S_OK)
}

/// Write a number the program asked for at `out`.
fn answer(cpu: &mut Cpu, out: u32, value: u32) -> Option<u32> {
    Some(if com::write_out(cpu, out, value) {
        S_OK
    } else {
        E_POINTER
    })
}

/// IDirectMusicPerformance::SetPrepareTime(this, milliseconds).
pub fn set_prepare_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    perf_mut(cpu, perf)?.prepare = args[1];
    Some(S_OK)
}

/// IDirectMusicPerformance::GetPrepareTime(this, milliseconds).
pub fn get_prepare_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let v = perf_mut(cpu, perf)?.prepare;
    answer(cpu, args[1], v)
}

/// IDirectMusicPerformance::SetBumperLength(this, milliseconds).
pub fn set_bumper_length(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    perf_mut(cpu, perf)?.bumper = args[1];
    Some(S_OK)
}

/// IDirectMusicPerformance::GetBumperLength(this, milliseconds).
pub fn get_bumper_length(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let v = perf_mut(cpu, perf)?.bumper;
    answer(cpu, args[1], v)
}

/// The event of the message at `at` (DMUS_PMSG and what follows by its
/// type), if it plays.
fn pmsg_event(cpu: &mut Cpu, at: u32, kind: u32, pchannel: u32) -> Option<Ev> {
    let b = |cpu: &mut Cpu, off: u32| -> u8 {
        let mut x = [0];
        cpu.bus.guest_read_bytes(at + off, &mut x);
        x[0]
    };
    Some(match kind {
        PMSGT_MIDI => {
            let status = b(cpu, 56);
            let mut bytes = vec![status, b(cpu, 57) & 0x7F, b(cpu, 58) & 0x7F];
            bytes.truncate(if matches!(status & 0xF0, 0xC0 | 0xD0) {
                2
            } else {
                3
            });
            Ev::Midi { pchannel, bytes }
        }
        PMSGT_NOTE => Ev::Note {
            pchannel,
            note: b(cpu, 75) & 0x7F,
            velocity: b(cpu, 68) & 0x7F,
            duration: (cpu.bus.guest_read_32(at + 56) as i32).max(1),
        },
        PMSGT_SYSEX => {
            let len = cpu.bus.guest_read_32(at + 56).min(0x10000);
            if !cpu.bus.guest_probe(at + 60, len as usize, false) {
                return None;
            }
            let mut bytes = vec![0; len as usize];
            cpu.bus.guest_read_bytes(at + 60, &mut bytes);
            Ev::Midi { pchannel, bytes }
        }
        PMSGT_TEMPO => {
            let mut t = [0; 8];
            cpu.bus.guest_read_bytes(at + 56, &mut t);
            Ev::Tempo {
                tempo: f64::from_le_bytes(t),
            }
        }
        PMSGT_CURVE => {
            // The curve's end value, at its start.
            let end = cpu.bus.guest_read_16(at + 70) as i16 as i32;
            let (kind, cc) = (b(cpu, 80), b(cpu, 82));
            let bytes = match kind {
                3 => {
                    let v = end.clamp(0, 0x3FFF);
                    vec![0xE0, (v & 0x7F) as u8, (v >> 7) as u8]
                }
                4 => vec![0xB0, cc & 0x7F, end.clamp(0, 127) as u8],
                5 => vec![0xD0, end.clamp(0, 127) as u8],
                6 => vec![0xA0, cc & 0x7F, end.clamp(0, 127) as u8],
                _ => return None,
            };
            Ev::Midi { pchannel, bytes }
        }
        PMSGT_TIMESIG => Ev::TimeSig {
            beats: b(cpu, 56).max(1),
            beat: b(cpu, 57).max(1),
            grids: cpu.bus.guest_read_16(at + 58).max(1),
        },
        PMSGT_PATCH => Ev::Band {
            inst: vec![super::files::Inst {
                pchannel,
                patch: (b(cpu, 57) as u32) << 16 | (b(cpu, 58) as u32) << 8 | b(cpu, 56) as u32,
                flags: super::files::INST_PATCH | super::files::INST_BANKSELECT,
                pan: 64,
                volume: 100,
            }],
        },
        _ => return None,
    })
}

/// Give back a message of the program's: what it holds released, its
/// memory freed.
fn free_pmsg_memory(cpu: &mut Cpu, perf: u32, at: u32) {
    let user = cpu.bus.guest_read_32(at + PM_USER);
    if user != 0
        && let Some(id) = object_of(cpu, user)
    {
        drop_ref(cpu, id);
    }
    if let Some(p) = perf_mut(cpu, perf) {
        p.pmsgs.retain(|&m| m != at);
    }
    free(cpu, at);
}

/// IDirectMusicPerformance::SendPMsg(this, message): it plays at its
/// time, and its memory goes back.
pub fn send_pmsg(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let perf = match inited(cpu, args[0], DMUS_E_NOT_INIT) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let at = args[1];
    if at == 0 {
        return Some(E_POINTER);
    }
    if !perf_mut(cpu, perf)?.pmsgs.contains(&at) {
        return Some(E_INVALIDARG);
    }
    let flags = cpu.bus.guest_read_32(at + PM_FLAGS);
    let kind = cpu.bus.guest_read_32(at + PM_TYPE);
    let pchannel = cpu.bus.guest_read_32(at + PM_PCHANNEL);
    let now = now_rt(cpu);
    let p = perf_mut(cpu, perf)?;
    let mt = if flags & PMSGF_MUSICTIME != 0 {
        cpu.bus.guest_read_32(at + PM_MTIME) as i32 as i64
    } else if flags & PMSGF_REFTIME != 0 {
        let rt = super::read_u64(cpu, at + PM_RTIME) as i64;
        perf_mut(cpu, perf)?.mt_of(rt)
    } else {
        p.mt_of(now)
    };
    if let Some(ev) = pmsg_event(cpu, at, kind, pchannel) {
        let p = perf_mut(cpu, perf)?;
        let i = p.sent.partition_point(|e| e.0 <= mt);
        p.sent.insert(i, (mt, Timed { mt: 0, ev }));
    }
    free_pmsg_memory(cpu, perf, at);
    winmm::schedule(cpu);
    Some(S_OK)
}

/// IDirectMusicPerformance::MusicToReferenceTime(this, time, rt).
pub fn music_to_reference_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let perf = match inited(cpu, args[0], DMUS_E_NO_MASTER_CLOCK) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    if args[2] == 0 {
        return Some(E_POINTER);
    }
    let rt = perf_mut(cpu, perf)?.rt_of(args[1] as i32 as i64);
    write_u64(cpu, args[2], rt as u64);
    Some(S_OK)
}

/// IDirectMusicPerformance::ReferenceToMusicTime(this, rt (2 words),
/// time).
pub fn reference_to_music_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let perf = match inited(cpu, args[0], DMUS_E_NO_MASTER_CLOCK) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let mt = perf_mut(cpu, perf)?.mt_of(arg64(args[1], args[2]));
    answer(cpu, args[3], mt as i32 as u32)
}

/// IDirectMusicPerformance::IsPlaying(this, segment, state): S_OK while
/// it plays, S_FALSE before it starts and after it ends.
pub fn is_playing(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 && args[2] == 0 {
        return Some(E_POINTER);
    }
    catch_up(cpu, perf);
    let seg = this_of(cpu, args[1], SEGMENT);
    let st = this_of(cpu, args[2], SEGMENT_STATE);
    let s = state(cpu);
    let playing = perf_of(s, perf).is_some_and(|p| {
        p.states.iter().any(|&id| {
            (Some(id) == st || playing_of(s, id).is_some_and(|pl| Some(pl.segment) == seg))
                && playing_of(s, id).is_some_and(|pl| (1..=2).contains(&pl.stage))
        })
    });
    Some(if playing { S_OK } else { S_FALSE })
}

/// IDirectMusicPerformance::GetTime(this, rt, time).
pub fn get_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let perf = match inited(cpu, args[0], DMUS_E_NO_MASTER_CLOCK) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let now = now_rt(cpu);
    if args[1] != 0 {
        write_u64(cpu, args[1], now as u64);
    }
    if args[2] != 0 {
        let mt = perf_mut(cpu, perf)?.mt_of(now);
        com::write_out(cpu, args[2], mt as i32 as u32);
    }
    Some(S_OK)
}

/// IDirectMusicPerformance::AllocPMsg(this, size, message).
pub fn alloc_pmsg(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[2] == 0 {
        return Some(E_POINTER);
    }
    if args[1] < PMSG || args[1] > 0x10_0000 {
        return Some(E_INVALIDARG);
    }
    let Some(at) = alloc(cpu, args[1]) else {
        return Some(com::E_OUTOFMEMORY);
    };
    cpu.bus.guest_write_32(at, args[1]);
    perf_mut(cpu, perf)?.pmsgs.push(at);
    answer(cpu, args[2], at)
}

/// IDirectMusicPerformance::FreePMsg(this, message): what it holds is
/// released.
pub fn free_pmsg(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    if !perf_mut(cpu, perf)?.pmsgs.contains(&args[1]) {
        return Some(DMUS_E_CANNOT_FREE);
    }
    free_pmsg_memory(cpu, perf, args[1]);
    Some(S_OK)
}

/// IDirectMusicPerformance::SetNotificationHandle(this, event, minimum
/// time (2 words)): 0 takes it away.
pub fn set_notification_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let event = if args[1] == 0 {
        0
    } else {
        match crate::win32::object::resolve(cpu, args[1]) {
            Some(id) => id,
            None => return Some(E_INVALIDARG),
        }
    };
    if event != 0 {
        crate::win32::object::objects(cpu).hold(event);
    }
    let old = std::mem::replace(&mut perf_mut(cpu, perf)?.event, event);
    if old != 0 {
        crate::win32::object::objects(cpu).release(old);
    }
    Some(S_OK)
}

/// IDirectMusicPerformance::GetNotificationPMsg(this, message): the next
/// notification, or S_FALSE.
pub fn get_notification_pmsg(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let out = args[1];
    if out == 0 {
        return Some(E_POINTER);
    }
    catch_up(cpu, perf);
    let p = perf_mut(cpu, perf)?;
    if p.ready.is_empty() {
        com::write_out(cpu, out, 0);
        return Some(S_FALSE);
    }
    let Some(at) = alloc(cpu, NOTIFICATION_PMSG) else {
        return Some(com::E_OUTOFMEMORY);
    };
    let p = perf_mut(cpu, perf)?;
    let n = p.ready.remove(0);
    p.pmsgs.push(at);
    cpu.bus.guest_write_32(at, NOTIFICATION_PMSG);
    write_u64(cpu, at + PM_RTIME, n.rt as u64);
    cpu.bus.guest_write_32(at + PM_MTIME, n.mt as i32 as u32);
    // DMUS_PMSGF_REFTIME | DMUS_PMSGF_TOOL_IMMEDIATE | _QUEUE, as Windows
    // 98 SE has them.
    cpu.bus.guest_write_32(at + PM_FLAGS, 13);
    cpu.bus.guest_write_32(at + PM_TYPE, PMSGT_NOTIFICATION);
    cpu.bus.guest_write_32(at + PM_GROUP, 1);
    if n.state != 0 {
        let user = pointer(cpu, n.state);
        cpu.bus.guest_write_32(at + PM_USER, user);
        drop_ref(cpu, n.state);
    }
    Guid(n.guid).write(cpu, at + PMSG);
    cpu.bus.guest_write_32(at + 72, n.option);
    cpu.bus.guest_write_32(at + 76, n.field1);
    cpu.bus.guest_write_32(at + 80, n.field2);
    answer(cpu, out, at)
}

/// IDirectMusicPerformance::AddNotificationType(this, guid).
pub fn add_notification_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    perf_notification(cpu, args, true)
}

/// IDirectMusicPerformance::RemoveNotificationType(this, guid).
pub fn remove_notification_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    perf_notification(cpu, args, false)
}

fn perf_notification(cpu: &mut Cpu, args: &[u32], on: bool) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(guid) = Guid::read(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    let p = perf_mut(cpu, perf)?;
    Some(super::segment::notification(&mut p.types, guid, on))
}

/// IDirectMusicPerformance::AddPort(this, port): NULL makes the
/// Microsoft Synthesizer's, with performance channels 0-15 on its first
/// channel group.
pub fn add_port(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let perf = match inited(cpu, args[0], DMUS_E_NOT_INIT) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    if args[1] == 0 {
        if perf_mut(cpu, perf)?.default_port != 0 {
            return Some(S_OK);
        }
        let port = core::new_port(cpu, true, 1);
        let p = perf_mut(cpu, perf)?;
        p.ports.push(port);
        p.default_port = port;
        assign(p, 0..16, port, 1, |pc| pc);
        return Some(S_OK);
    }
    let Some(port) = this_of(cpu, args[1], PORT) else {
        return Some(E_POINTER);
    };
    if perf_mut(cpu, perf)?.ports.contains(&port) {
        return Some(S_OK);
    }
    hold(cpu, port);
    perf_mut(cpu, perf)?.ports.push(port);
    Some(S_OK)
}

/// Assign the performance channels `pchannels` to the port's group.
fn assign(
    p: &mut Perf,
    pchannels: std::ops::Range<u32>,
    port: u32,
    group: u32,
    channel: impl Fn(u32) -> u32,
) {
    for pc in pchannels {
        p.assigned.retain(|a| a.pchannel != pc);
        p.assigned.push(Assign {
            pchannel: pc,
            port,
            group,
            channel: channel(pc) & 0x0F,
        });
    }
}

/// IDirectMusicPerformance::RemovePort(this, port).
pub fn remove_port(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(port) = this_of(cpu, args[1], PORT) else {
        return Some(E_POINTER);
    };
    let p = perf_mut(cpu, perf)?;
    let Some(i) = p.ports.iter().position(|&x| x == port) else {
        return Some(E_INVALIDARG);
    };
    p.ports.remove(i);
    p.assigned.retain(|a| a.port != port);
    if p.default_port == port {
        p.default_port = 0;
    }
    drop_ref(cpu, port);
    Some(S_OK)
}

/// The port the program's pointer `port` is, if the performance has it,
/// and whether its channel groups take `group`.
fn port_for(cpu: &mut Cpu, perf: u32, port: u32, group: u32) -> Option<u32> {
    let id = this_of(cpu, port, PORT)?;
    if !perf_mut(cpu, perf)?.ports.contains(&id) {
        return None;
    }
    let groups = match &state(cpu).get(id)?.kind {
        Kind::Port { p } => p.groups,
        _ => return None,
    };
    (group >= 1 && group <= groups).then_some(id)
}

/// IDirectMusicPerformance::AssignPChannelBlock(this, block, port,
/// group).
pub fn assign_pchannel_block(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(port) = port_for(cpu, perf, args[2], args[3]) else {
        return Some(E_INVALIDARG);
    };
    let Some(first) = args[1].checked_mul(16).filter(|f| *f < 0xFFFF_FFF0) else {
        return Some(E_INVALIDARG);
    };
    assign(
        perf_mut(cpu, perf)?,
        first..first + 16,
        port,
        args[3],
        |pc| pc - first,
    );
    Some(S_OK)
}

/// IDirectMusicPerformance::AssignPChannel(this, pchannel, port, group,
/// channel).
pub fn assign_pchannel(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[4] > 15 {
        return Some(E_INVALIDARG);
    }
    let Some(port) = port_for(cpu, perf, args[2], args[3]) else {
        return Some(E_INVALIDARG);
    };
    let pc = args[1];
    assign(perf_mut(cpu, perf)?, pc..pc + 1, port, args[3], |_| args[4]);
    Some(S_OK)
}

/// IDirectMusicPerformance::PChannelInfo(this, pchannel, port, group,
/// channel).
pub fn pchannel_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let Some(a) = perf_mut(cpu, perf)?
        .assigned
        .iter()
        .find(|a| a.pchannel == args[1])
        .cloned()
    else {
        return Some(E_INVALIDARG);
    };
    if args[2] != 0 {
        let at = pointer(cpu, a.port);
        com::write_out(cpu, args[2], at);
    }
    if args[3] != 0 {
        com::write_out(cpu, args[3], a.group);
    }
    if args[4] != 0 {
        com::write_out(cpu, args[4], a.channel);
    }
    Some(S_OK)
}

/// IDirectMusicPerformance::DownloadInstrument(this, instrument,
/// pchannel, downloaded, ranges, count, port, group, channel).
pub fn download_instrument(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if this_of(cpu, args[1], super::INSTRUMENT).is_none() {
        return Some(E_POINTER);
    }
    let Some(a) = perf_mut(cpu, perf)?
        .assigned
        .iter()
        .find(|a| a.pchannel == args[2])
        .cloned()
    else {
        return Some(E_INVALIDARG);
    };
    let hr = core::give_downloaded(cpu, args[3]);
    if hr != S_OK {
        return Some(hr);
    }
    if args[6] != 0 {
        let at = pointer(cpu, a.port);
        com::write_out(cpu, args[6], at);
    }
    com::write_out(cpu, args[7], a.group);
    com::write_out(cpu, args[8], a.channel);
    Some(S_OK)
}

/// IDirectMusicPerformance::GetParam(this, guid, groups, index, time,
/// next, param): of the segment playing at the time.
pub fn get_param(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, guid, groups, index, mt, next, param] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let perf = match inited(cpu, this, DMUS_E_NOT_INIT) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    let Some(g) = Guid::read(cpu, guid) else {
        return Some(E_POINTER);
    };
    let mt = mt as i32 as i64;
    let s = state(cpu);
    let Some(st) = perf_of(s, perf).and_then(|p| state_at(s, p, mt)) else {
        return Some(DMUS_E_NOT_FOUND);
    };
    let (seg, offset) = playing_of(s, st).map(|pl| (pl.segment, pl.offset))?;
    let at = (mt - offset) as i32;
    let which = (groups, index);
    Some(super::segment::segment_param(
        cpu, seg, &g, which, at, next, param,
    ))
}

/// IDirectMusicPerformance::SetParam(this, guid, groups, index, time,
/// param): on the segment playing at the time, if one is.
pub fn set_param(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, guid, groups, index, mt, param] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(perf) = this_perf(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let Some(g) = Guid::read(cpu, guid) else {
        return Some(E_POINTER);
    };
    let mt = mt as i32 as i64;
    let s = state(cpu);
    let Some(st) = perf_of(s, perf).and_then(|p| state_at(s, p, mt)) else {
        return Some(S_OK);
    };
    let (seg, offset) = playing_of(s, st).map(|pl| (pl.segment, pl.offset))?;
    let at = (mt - offset) as i32;
    let which = (groups, index);
    Some(super::segment::set_segment_param(
        cpu, seg, &g, which, at, param,
    ))
}

/// IDirectMusicPerformance::GetGlobalParam(this, guid, data, size):
/// E_INVALIDARG for one never set.
pub fn get_global_param(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, guid, data, size] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(perf) = this_perf(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let Some(g) = Guid::read(cpu, guid) else {
        return Some(E_POINTER);
    };
    let Some(value) = perf_mut(cpu, perf)?.param(&g).map(<[u8]>::to_vec) else {
        return Some(E_INVALIDARG);
    };
    if data == 0 {
        return Some(E_POINTER);
    }
    let n = value.len().min(size as usize);
    if !cpu.bus.guest_probe(data, n, true) {
        return Some(E_POINTER);
    }
    cpu.bus.guest_write_bytes(data, &value[..n]);
    Some(S_OK)
}

/// IDirectMusicPerformance::SetGlobalParam(this, guid, data, size).
pub fn set_global_param(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, guid, data, size] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(perf) = this_perf(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    let Some(g) = Guid::read(cpu, guid) else {
        return Some(E_POINTER);
    };
    let size = size.min(0x1000) as usize;
    if data == 0 || !cpu.bus.guest_probe(data, size, false) {
        return Some(E_POINTER);
    }
    let mut value = vec![0; size];
    cpu.bus.guest_read_bytes(data, &mut value);
    let now = now_rt(cpu);
    let p = perf_mut(cpu, perf)?;
    // A new master tempo counts from now.
    if g == GUID_PERF_MASTER_TEMPO && p.inited {
        let mt = p.mt_of(now);
        let tempo = p.anchor.2;
        p.set_tempo(mt, tempo);
    }
    p.params.retain(|x| x.0 != g.0);
    p.params.push((g.0, value));
    winmm::schedule(cpu);
    Some(S_OK)
}

/// IDirectMusicPerformance::GetLatencyTime and GetQueueTime(this, rt):
/// the ports' latency on from now.
pub fn get_latency_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Err(e) = inited(cpu, args[0], DMUS_E_NO_MASTER_CLOCK) {
        return Some(e);
    }
    if args[1] == 0 || !cpu.bus.guest_probe(args[1], 8, true) {
        return Some(E_POINTER);
    }
    let at = now_rt(cpu) + LATENCY;
    write_u64(cpu, args[1], at as u64);
    Some(S_OK)
}

/// IDirectMusicPerformance::CloseDown(this): the music stops, and what
/// it held goes.
pub fn close_down(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(perf) = this_perf(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    close(cpu, perf);
    let p = perf_mut(cpu, perf)?;
    let held = p.held();
    p.music = 0;
    p.ports.clear();
    p.default_port = 0;
    p.assigned.clear();
    p.states.clear();
    p.ready.clear();
    p.graph = 0;
    p.inited = false;
    for id in held {
        drop_ref(cpu, id);
    }
    winmm::schedule(cpu);
    Some(S_OK)
}

/// Align the music time `mt` to the next boundary `flags` name.
fn resolve(p: &Perf, mt: i64, flags: u32) -> i64 {
    let (beat, measure, grid) = p.lengths();
    let unit = if flags & SEGF_MEASURE != 0 {
        measure
    } else if flags & SEGF_BEAT != 0 {
        beat
    } else if flags & SEGF_GRID != 0 {
        grid
    } else {
        1
    };
    (mt + unit - 1).div_euclid(unit) * unit
}

/// IDirectMusicPerformance::GetResolvedTime(this, rt (2 words), out,
/// flags).
pub fn get_resolved_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let perf = match inited(cpu, args[0], DMUS_E_NO_MASTER_CLOCK) {
        Ok(id) => id,
        Err(e) => return Some(e),
    };
    if args[3] == 0 {
        return Some(E_POINTER);
    }
    let p = perf_mut(cpu, perf)?;
    let rt = arg64(args[1], args[2]);
    let mt = resolve(p, p.mt_of(rt), args[4]);
    let out = p.rt_of(mt).max(rt);
    write_u64(cpu, args[3], out as u64);
    Some(S_OK)
}

/// A DMUS_TIMESIGNATURE at `at`: its time, and the lengths of its beat
/// and grid and measure.
fn read_sig(cpu: &mut Cpu, at: u32) -> Option<(i64, i64, i64, i64)> {
    if at == 0 || !cpu.bus.guest_probe(at, 8, false) {
        return None;
    }
    let time = cpu.bus.guest_read_32(at) as i32 as i64;
    let mut b = [0; 2];
    cpu.bus.guest_read_bytes(at + 4, &mut b);
    let grids = cpu.bus.guest_read_16(at + 6).max(1) as i64;
    let beat = (PPQ as i64 * 4 / b[1].max(1) as i64).max(1);
    Some((time, beat, (beat / grids).max(1), beat * b[0].max(1) as i64))
}

/// IDirectMusicPerformance::TimeToRhythm(this, time, signature, measure,
/// beat, grid, offset).
pub fn time_to_rhythm(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if this_perf(cpu, args[0]).is_none() {
        return Some(E_INVALIDARG);
    }
    let Some((time, beat, grid, measure)) = read_sig(cpu, args[2]) else {
        return Some(E_POINTER);
    };
    let t = (args[1] as i32 as i64 - time).max(0);
    let m = t / measure;
    let in_measure = t % measure;
    let b = in_measure / beat;
    let g = in_measure % beat / grid;
    let o = in_measure % beat % grid;
    if args[3] != 0 {
        cpu.bus.guest_write_16(args[3], m as u16);
    }
    if args[4] != 0 {
        cpu.bus.guest_write_bytes(args[4], &[b as u8]);
    }
    if args[5] != 0 {
        cpu.bus.guest_write_bytes(args[5], &[g as u8]);
    }
    if args[6] != 0 {
        cpu.bus.guest_write_16(args[6], o as u16);
    }
    Some(S_OK)
}

/// IDirectMusicPerformance::RhythmToTime(this, measure, beat, grid,
/// offset, signature, time).
pub fn rhythm_to_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if this_perf(cpu, args[0]).is_none() {
        return Some(E_INVALIDARG);
    }
    let Some((time, beat, grid, measure)) = read_sig(cpu, args[5]) else {
        return Some(E_POINTER);
    };
    let mt = time
        + (args[1] & 0xFFFF) as i64 * measure
        + (args[2] & 0xFF) as i64 * beat
        + (args[3] & 0xFF) as i64 * grid
        + (args[4] & 0xFFFF) as u16 as i16 as i64;
    answer(cpu, args[6], mt as i32 as u32)
}

/// The segment state `this` is: what it plays.
fn this_playing(cpu: &mut Cpu, this: u32) -> Option<Playing> {
    let id = this_of(cpu, this, SEGMENT_STATE)?;
    playing_of(state(cpu), id).cloned()
}

/// IDirectMusicSegmentState::GetRepeats(this, repeats).
pub fn state_get_repeats(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(pl) = this_playing(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    answer(cpu, args[1], pl.repeats)
}

/// IDirectMusicSegmentState::GetSegment(this, segment).
pub fn state_get_segment(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(pl) = this_playing(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    if pl.segment == 0 {
        com::write_out(cpu, args[1], 0);
        return Some(DMUS_E_NOT_FOUND);
    }
    let at = pointer(cpu, pl.segment);
    answer(cpu, args[1], at)
}

/// IDirectMusicSegmentState::GetStartTime(this, time).
pub fn state_get_start_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(pl) = this_playing(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    answer(cpu, args[1], pl.start as i32 as u32)
}

/// IDirectMusicSegmentState::GetSeek(this, time): how far the segment
/// is prepared, its prepare time ahead of now.
pub fn state_get_seek(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(pl) = this_playing(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let now = now_rt(cpu);
    let seek = match perf_of(state(cpu), pl.perf) {
        Some(p) if pl.stage < 3 => {
            let ahead = p.mt_of(now + p.prepare as i64 * 10_000) - pl.offset;
            ahead.clamp(pl.start_point as i64, pl.pass_end() as i64)
        }
        _ => pl.length as i64,
    };
    answer(cpu, args[1], seek as i32 as u32)
}

/// IDirectMusicSegmentState::GetStartPoint(this, time).
pub fn state_get_start_point(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(pl) = this_playing(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    answer(cpu, args[1], pl.start_point as u32)
}

crate::state_fields!(Assign {
    pchannel,
    port,
    group,
    channel
});
crate::state_fields!(Notice {
    mt,
    rt,
    guid,
    option,
    state,
    field1,
    field2
});
crate::state_fields!(Perf {
    music,
    inited,
    ports,
    default_port,
    assigned,
    prepare,
    bumper,
    anchor,
    sig,
    states,
    offs,
    sent,
    types,
    event,
    ready,
    pmsgs,
    params,
    graph,
    tools
});
crate::state_fields!(Playing {
    perf,
    segment,
    flags,
    start,
    offset,
    start_point,
    length,
    loop_start,
    loop_end,
    repeats,
    left,
    events,
    index,
    stage,
    next_beat
});
