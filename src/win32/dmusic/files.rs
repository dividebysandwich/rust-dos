//! DirectMusic's files: segments (.sgt), MIDI files, which the loader
//! makes segments of, DLS collections (.dls), bands, styles and chord
//! maps. What they hold is read into plain data, which the objects are
//! made from (loader.rs).

use super::guids::*;
use super::riff::{self, Chunk, f64_at, i32_at, items, u16_at, u32_at};
use super::{DMUS_OBJ_CATEGORY, DMUS_OBJ_NAME, DMUS_OBJ_OBJECT, DMUS_OBJ_VERSION, Desc};

/// Ticks of a quarter note in music time (DMUS_PPQ).
pub const PPQ: i32 = 768;

/// An event of a track at its music time.
#[derive(Clone, Debug, Default)]
pub struct Timed {
    pub mt: i32,
    pub ev: Ev,
}

/// What a track plays.
#[derive(Clone, Debug)]
pub enum Ev {
    /// A MIDI message for a performance channel; the status's channel is
    /// the one the channel is assigned to. System messages go as they
    /// are.
    Midi {
        pchannel: u32,
        bytes: Vec<u8>,
    },
    /// A note, its note-off `duration` later.
    Note {
        pchannel: u32,
        note: u8,
        velocity: u8,
        duration: i32,
    },
    /// The tempo from here, in beats a minute.
    Tempo {
        tempo: f64,
    },
    TimeSig {
        beats: u8,
        beat: u8,
        grids: u16,
    },
    /// The instruments a band sets.
    Band {
        inst: Vec<Inst>,
    },
}

impl Default for Ev {
    fn default() -> Ev {
        Ev::Tempo { tempo: 120.0 }
    }
}

impl Ev {
    /// Its place among events at one time: settings before notes.
    pub fn order(&self) -> u8 {
        match self {
            Ev::Tempo { .. } | Ev::TimeSig { .. } | Ev::Band { .. } => 0,
            Ev::Midi { .. } | Ev::Note { .. } => 1,
        }
    }
}

/// An instrument of a band (DMUS_IO_INSTRUMENT).
#[derive(Clone, Debug, Default)]
pub struct Inst {
    pub pchannel: u32,
    pub patch: u32,
    pub flags: u32,
    pub pan: u8,
    pub volume: u8,
}

/// DMUS_IO_INST_ flags of an instrument: what it sets.
pub const INST_PATCH: u32 = 1;
pub const INST_BANKSELECT: u32 = 2;
pub const INST_PAN: u32 = 0x20;
pub const INST_VOLUME: u32 = 0x40;

impl Inst {
    /// The MIDI messages that set it up on `channel`.
    pub fn messages(&self, channel: u8) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let c = channel & 0x0F;
        if self.flags & INST_PATCH != 0 {
            if self.flags & INST_BANKSELECT != 0 {
                out.push(vec![0xB0 | c, 0, (self.patch >> 16) as u8 & 0x7F]);
                out.push(vec![0xB0 | c, 32, (self.patch >> 8) as u8 & 0x7F]);
            }
            out.push(vec![0xC0 | c, self.patch as u8 & 0x7F]);
        }
        if self.flags & INST_VOLUME != 0 {
            out.push(vec![0xB0 | c, 7, self.volume & 0x7F]);
        }
        if self.flags & INST_PAN != 0 {
            out.push(vec![0xB0 | c, 10, self.pan & 0x7F]);
        }
        out
    }
}

/// A track as a file has it.
#[derive(Clone, Debug, Default)]
pub struct TrackData {
    pub class: [u8; 16],
    pub group: u32,
    pub events: Vec<Timed>,
}

/// A segment as a file has it.
#[derive(Clone, Debug, Default)]
pub struct SegmentData {
    pub length: i32,
    pub repeats: u32,
    pub resolution: u32,
    pub start: i32,
    pub loop_start: i32,
    pub loop_end: i32,
    pub tracks: Vec<TrackData>,
}

/// What a file holds.
#[derive(Clone, Debug)]
pub enum Content {
    Segment(SegmentData),
    /// The instruments (patch, name) of a DLS collection.
    Collection(Vec<(u32, String)>),
    Band(Vec<Inst>),
    /// A style's tempo, time signature, and bands with their names.
    Style {
        tempo: f64,
        sig: (u8, u8, u16),
        bands: Vec<(Desc, Vec<Inst>)>,
    },
    ChordMap(u32),
}

/// What the file `data` holds, with its description, if DirectMusic
/// reads it.
pub fn read(data: &[u8]) -> Option<(Desc, Content)> {
    if data.starts_with(b"MThd") {
        return Some((Desc::default(), Content::Segment(midi_file(data)?)));
    }
    let riff = riff::parse(data)?;
    let desc = description(&riff);
    let content = match &riff.form {
        b"DMSG" => Content::Segment(segment(&riff)),
        b"DLS " => Content::Collection(collection(&riff)),
        b"DMBD" => Content::Band(band(&riff)),
        b"DMST" => style(&riff),
        b"DMPR" => Content::ChordMap(
            riff.find(b"perh")
                .and_then(|c| u32_at(c.data, 0))
                .unwrap_or(0),
        ),
        b"RMID" => {
            let midi = riff.find(b"data")?;
            Content::Segment(midi_file(midi.data)?)
        }
        _ => return None,
    };
    Some((desc, content))
}

/// The class of object the file `data` holds, by its first bytes.
pub fn class_of(data: &[u8]) -> Option<[u8; 16]> {
    if data.starts_with(b"MThd") {
        return Some(CLSID_SEGMENT.0);
    }
    let form = data.get(8..12)?;
    if !data.starts_with(b"RIFF") {
        return None;
    }
    Some(
        match form {
            b"DMSG" | b"RMID" => CLSID_SEGMENT,
            b"DLS " => CLSID_COLLECTION,
            b"DMBD" => CLSID_BAND,
            b"DMST" => CLSID_STYLE,
            b"DMPR" => CLSID_CHORD_MAP,
            _ => return None,
        }
        .0,
    )
}

/// The description a DirectMusic file keeps: its GUID ("guid", "dlid"
/// in a DLS file), version ("vers") and name and category (an "UNFO"
/// list, an "INFO" list in a DLS file).
fn description(riff: &Chunk) -> Desc {
    let mut d = Desc::default();
    if let Some(g) = riff.find(b"guid").or_else(|| riff.find(b"dlid"))
        && let Some(bytes) = g.data.get(..16)
    {
        d.object.copy_from_slice(bytes);
        d.valid |= DMUS_OBJ_OBJECT;
    }
    if let Some(v) = riff.find(b"vers")
        && let (Some(ms), Some(ls)) = (u32_at(v.data, 0), u32_at(v.data, 4))
    {
        d.version = (ms, ls);
        d.valid |= DMUS_OBJ_VERSION;
    }
    if let Some(unfo) = riff.find_list(b"UNFO") {
        if let Some(n) = unfo.find(b"UNAM") {
            d.name = riff::wide_string(n.data);
            d.valid |= DMUS_OBJ_NAME;
        }
        if let Some(c) = unfo.find(b"UCAT") {
            d.category = riff::wide_string(c.data);
            d.valid |= DMUS_OBJ_CATEGORY;
        }
    } else if let Some(info) = riff.find_list(b"INFO")
        && let Some(n) = info.find(b"INAM")
    {
        d.name = riff::narrow_string(n.data);
        d.valid |= DMUS_OBJ_NAME;
    }
    d
}

/// A segment file's segment ("DMSG"): its header ("segh") and its
/// tracks ("trkl").
fn segment(riff: &Chunk) -> SegmentData {
    let mut s = SegmentData {
        length: 1,
        ..SegmentData::default()
    };
    if let Some(h) = riff.find(b"segh") {
        let d = h.data;
        s.repeats = u32_at(d, 0).unwrap_or(0);
        s.length = i32_at(d, 4).unwrap_or(1).max(1);
        s.start = i32_at(d, 8).unwrap_or(0);
        s.loop_start = i32_at(d, 12).unwrap_or(0);
        s.loop_end = i32_at(d, 16).unwrap_or(0);
        s.resolution = u32_at(d, 20).unwrap_or(0);
    }
    if let Some(list) = riff.find_list(b"trkl") {
        for t in list.children().filter(|c| c.is(b"DMTK")) {
            if let Some(track) = track(&t) {
                s.tracks.push(track);
            }
        }
    }
    s
}

/// A track ("DMTK"): its header ("trkh") and its data, the chunk or
/// list the header names.
fn track(dmtk: &Chunk) -> Option<TrackData> {
    let h = dmtk.find(b"trkh")?.data;
    let class: [u8; 16] = h.get(..16)?.try_into().ok()?;
    let group = u32_at(h, 20).unwrap_or(1);
    let ckid: [u8; 4] = h.get(24..28)?.try_into().ok()?;
    let fcc: [u8; 4] = h.get(28..32)?.try_into().ok()?;
    let body = dmtk.children().find(|c| {
        if ckid != [0; 4] {
            !c.is_list() && c.id == ckid
        } else {
            c.is_list() && c.form == fcc
        }
    });
    let events = match body {
        Some(b) if class == CLSID_SEQ_TRACK.0 => seq_track(&b),
        Some(b) if class == CLSID_TEMPO_TRACK.0 => items(b.data)
            .filter_map(|i| {
                Some(Timed {
                    mt: i32_at(i, 0)?,
                    ev: Ev::Tempo {
                        tempo: f64_at(i, 8)?,
                    },
                })
            })
            .collect(),
        Some(b) if class == CLSID_TIME_SIG_TRACK.0 => items(b.data)
            .filter_map(|i| {
                Some(Timed {
                    mt: i32_at(i, 0)?,
                    ev: Ev::TimeSig {
                        beats: *i.get(4)?,
                        beat: *i.get(5)?,
                        grids: u16_at(i, 6)?,
                    },
                })
            })
            .collect(),
        Some(b) if class == CLSID_SYSEX_TRACK.0 => sysex_track(b.data),
        Some(b) if class == CLSID_BAND_TRACK.0 => band_track(&b),
        _ => Vec::new(),
    };
    Some(TrackData {
        class,
        group: if group == 0 { 1 } else { group },
        events,
    })
}

/// How many data bytes a MIDI message with `status` has.
fn data_bytes(status: u8) -> usize {
    match status & 0xF0 {
        0xC0 | 0xD0 => 1,
        _ => 2,
    }
}

/// A sequence track ("seqt"): its events ("evtl", DMUS_IO_SEQ_ITEM) and
/// curves ("curl", DMUS_IO_CURVE_ITEM).
fn seq_track(seqt: &Chunk) -> Vec<Timed> {
    let mut out = Vec::new();
    if let Some(evtl) = seqt.find(b"evtl") {
        for i in items(evtl.data) {
            let (Some(mt), Some(duration), Some(pchannel), Some(offset)) =
                (i32_at(i, 0), i32_at(i, 4), u32_at(i, 8), u16_at(i, 12))
            else {
                continue;
            };
            let (Some(&status), Some(&b1), Some(&b2)) = (i.get(14), i.get(15), i.get(16)) else {
                continue;
            };
            let mt = mt.saturating_add(offset as i16 as i32);
            let ev = if status & 0xF0 == 0x90 {
                Ev::Note {
                    pchannel,
                    note: b1 & 0x7F,
                    velocity: b2 & 0x7F,
                    duration: duration.max(1),
                }
            } else if status & 0x80 != 0 && status < 0xF0 {
                let mut bytes = vec![status, b1 & 0x7F, b2 & 0x7F];
                bytes.truncate(1 + data_bytes(status));
                Ev::Midi { pchannel, bytes }
            } else {
                continue;
            };
            out.push(Timed { mt, ev });
        }
    }
    if let Some(curl) = seqt.find(b"curl") {
        for i in items(curl.data) {
            curve(i, &mut out);
        }
    }
    out
}

/// DMUS_CURVET_ types of curves, and the instant shape.
const CURVE_PB: u8 = 3;
const CURVE_CC: u8 = 4;
const CURVE_MAT: u8 = 5;
const CURVE_PAT: u8 = 6;
const SHAPE_INSTANT: u8 = 1;
/// DMUS_CURVE_RESET: the reset value is sent after the curve.
const CURVE_RESET: u8 = 1;

/// A curve (DMUS_IO_CURVE_ITEM) as the messages of its steps: the value
/// goes from its start to its end in steps of a sixteenth of a beat.
fn curve(i: &[u8], out: &mut Vec<Timed>) {
    let (Some(start), Some(duration), Some(reset_after), Some(pchannel)) =
        (i32_at(i, 0), i32_at(i, 4), i32_at(i, 8), u32_at(i, 12))
    else {
        return;
    };
    let (Some(offset), Some(from), Some(to), Some(reset)) =
        (u16_at(i, 16), u16_at(i, 18), u16_at(i, 20), u16_at(i, 22))
    else {
        return;
    };
    let (Some(&kind), Some(&shape), Some(&cc), Some(&flags)) =
        (i.get(24), i.get(25), i.get(26), i.get(27))
    else {
        return;
    };
    let (from, to, reset) = (from as i16 as i32, to as i16 as i32, reset as i16 as i32);
    let message = |v: i32| -> Option<Vec<u8>> {
        Some(match kind {
            CURVE_PB => {
                let v = v.clamp(0, 0x3FFF);
                vec![0xE0, (v & 0x7F) as u8, (v >> 7) as u8]
            }
            CURVE_CC => vec![0xB0, cc & 0x7F, v.clamp(0, 127) as u8],
            CURVE_MAT => vec![0xD0, v.clamp(0, 127) as u8],
            CURVE_PAT => vec![0xA0, cc & 0x7F, v.clamp(0, 127) as u8],
            _ => return None,
        })
    };
    let start = start.saturating_add(offset as i16 as i32);
    let duration = duration.max(0);
    let steps = if shape == SHAPE_INSTANT {
        0
    } else {
        (duration / (PPQ / 16)).clamp(0, 64)
    };
    let mut push = |mt: i32, v: i32| {
        if let Some(bytes) = message(v) {
            out.push(Timed {
                mt,
                ev: Ev::Midi { pchannel, bytes },
            });
        }
    };
    if steps == 0 {
        push(start, to);
    } else {
        for s in 0..=steps {
            let v = from + (to - from) * s / steps;
            push(start + duration * s / steps, v);
        }
    }
    if flags & CURVE_RESET != 0 {
        push(start + duration + reset_after.max(0), reset);
    }
}

/// A system exclusive track ("syex"): items of a time, a performance
/// channel (DirectX 7's) or not, a length and the bytes.
fn sysex_track(data: &[u8]) -> Vec<Timed> {
    // With the channel if the bytes after a 12-byte header start the
    // message, else DirectX 6's 8-byte header.
    let header = if data.get(12) == Some(&0xF0) { 12 } else { 8 };
    let mut out = Vec::new();
    let mut at = 0;
    while let (Some(mt), Some(len)) = (i32_at(data, at), u32_at(data, at + header - 4)) {
        let start = at + header;
        let Some(bytes) = data.get(start..start.saturating_add(len as usize)) else {
            break;
        };
        out.push(Timed {
            mt,
            ev: Ev::Midi {
                pchannel: 0,
                bytes: bytes.to_vec(),
            },
        });
        at = start + len as usize;
    }
    out
}

/// A band track ("DMBT"): its bands ("lbdl"), each at its time ("bdih",
/// DirectX 7's "bd2h").
fn band_track(dmbt: &Chunk) -> Vec<Timed> {
    let mut out = Vec::new();
    let Some(list) = dmbt.find_list(b"lbdl") else {
        return out;
    };
    for lbnd in list.children().filter(|c| c.is(b"lbnd")) {
        let mt = lbnd
            .find(b"bdih")
            .or_else(|| lbnd.find(b"bd2h"))
            .and_then(|h| i32_at(h.data, 0))
            .unwrap_or(0);
        if let Some(dmbd) = lbnd.find_list(b"DMBD") {
            out.push(Timed {
                mt,
                ev: Ev::Band { inst: band(&dmbd) },
            });
        }
    }
    out
}

/// A band's instruments ("lbil", each "lbin" with its "bins").
fn band(dmbd: &Chunk) -> Vec<Inst> {
    let mut out = Vec::new();
    let Some(list) = dmbd.find_list(b"lbil") else {
        return out;
    };
    for lbin in list.children().filter(|c| c.is(b"lbin")) {
        let Some(b) = lbin.find(b"bins") else {
            continue;
        };
        let d = b.data;
        let (Some(patch), Some(pchannel), Some(flags)) =
            (u32_at(d, 0), u32_at(d, 24), u32_at(d, 28))
        else {
            continue;
        };
        out.push(Inst {
            pchannel,
            patch,
            flags,
            pan: d.get(32).copied().unwrap_or(64),
            volume: d.get(33).copied().unwrap_or(100),
        });
    }
    out
}

/// A DLS collection's instruments ("lins", each "ins " with its header
/// "insh" and name).
fn collection(riff: &Chunk) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    let Some(lins) = riff.find_list(b"lins") else {
        return out;
    };
    for ins in lins.children().filter(|c| c.is(b"ins ")) {
        let Some(h) = ins.find(b"insh") else {
            continue;
        };
        let (Some(bank), Some(program)) = (u32_at(h.data, 4), u32_at(h.data, 8)) else {
            continue;
        };
        let name = ins
            .find_list(b"INFO")
            .and_then(|i| i.find(b"INAM"))
            .map(|n| riff::narrow_string(n.data))
            .unwrap_or_default();
        out.push((patch_of_locale(bank, program), name));
    }
    out
}

/// A DLS instrument's patch (MIDILOCALE's bank and program) as
/// DirectMusic's patch number: bank select MSB in bits 16-22, LSB in
/// bits 8-14, the program in bits 0-6, and the drum flag.
pub fn patch_of_locale(bank: u32, program: u32) -> u32 {
    (bank & 0x8000_0000) | (bank & 0x7F00) << 8 | (bank & 0x7F) << 8 | (program & 0x7F)
}

/// A style ("DMST"): its header ("styh": time signature and tempo) and
/// bands.
fn style(riff: &Chunk) -> Content {
    let (mut tempo, mut sig) = (120.0, (4, 4, 4));
    if let Some(h) = riff.find(b"styh") {
        let d = h.data;
        if let (Some(&beats), Some(&beat), Some(grids)) = (d.first(), d.get(1), u16_at(d, 2)) {
            sig = (beats, beat, grids);
        }
        tempo = f64_at(d, 8).unwrap_or(120.0);
    }
    let bands = riff
        .children()
        .filter(|c| c.is_list() && c.form == *b"DMBD")
        .map(|c| (description(&c), band(&c)))
        .collect();
    Content::Style { tempo, sig, bands }
}

/// A Standard MIDI File as DirectMusic's loader makes a segment of it:
/// chord, tempo, band, time signature and sequence tracks, in that
/// order, all in group 1, times in music time, and its length its last
/// event's time rounded up to a whole measure.
pub fn midi_file(data: &[u8]) -> Option<SegmentData> {
    let be32 = |at: usize| Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?));
    let be16 = |at: usize| Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?));
    let header = be32(4)? as usize;
    let tracks = be16(10)?;
    let division = (be16(12)? & 0x7FFF).max(1) as i64;
    let to_mt = |tick: u64| (tick as i64 * PPQ as i64 / division).min(i32::MAX as i64) as i32;
    let (mut tempos, mut sigs, mut bands, mut seq) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut last = 0i32;
    let mut at = 8usize.checked_add(header)?;
    for _ in 0..tracks {
        if data.get(at..at + 4) != Some(b"MTrk") {
            break;
        }
        let len = be32(at + 4)? as usize;
        let end = (at + 8).saturating_add(len).min(data.len());
        let body = &data[at + 8..end];
        let mut p = 0;
        let mut tick = 0u64;
        let mut status = 0u8;
        while p < body.len() {
            tick += var_len(body, &mut p)? as u64;
            let mt = to_mt(tick);
            last = last.max(mt);
            let mut b = *body.get(p)?;
            if b & 0x80 != 0 {
                p += 1;
            } else {
                b = status;
            }
            match b {
                0xFF => {
                    let kind = *body.get(p)?;
                    p += 1;
                    let n = var_len(body, &mut p)? as usize;
                    let meta = body.get(p..p + n)?;
                    p += n;
                    match kind {
                        0x51 if n == 3 => {
                            let us =
                                (meta[0] as u32) << 16 | (meta[1] as u32) << 8 | meta[2] as u32;
                            let tempo = 60_000_000.0 / us.max(1) as f64;
                            tempos.push(Timed {
                                mt,
                                ev: Ev::Tempo { tempo },
                            });
                        }
                        0x58 if n >= 2 => sigs.push(Timed {
                            mt,
                            ev: Ev::TimeSig {
                                beats: meta[0].max(1),
                                beat: 1u8.checked_shl(meta[1] as u32).unwrap_or(4),
                                grids: 4,
                            },
                        }),
                        0x2F => break,
                        _ => {}
                    }
                }
                0xF0 | 0xF7 => {
                    let n = var_len(body, &mut p)? as usize;
                    let msg = body.get(p..p + n)?;
                    p += n;
                    let mut bytes = Vec::with_capacity(n + 1);
                    if b == 0xF0 {
                        bytes.push(0xF0);
                    }
                    bytes.extend_from_slice(msg);
                    seq.push(Timed {
                        mt,
                        ev: Ev::Midi { pchannel: 0, bytes },
                    });
                }
                0x80..=0xEF => {
                    status = b;
                    let n = data_bytes(b);
                    let mut bytes = vec![b & 0xF0];
                    bytes.extend_from_slice(body.get(p..p + n)?);
                    p += n;
                    let ev = Ev::Midi {
                        pchannel: (b & 0x0F) as u32,
                        bytes,
                    };
                    // Program changes go to the band track.
                    if b & 0xF0 == 0xC0 {
                        bands.push(Timed { mt, ev });
                    } else {
                        seq.push(Timed { mt, ev });
                    }
                }
                _ => return None,
            }
        }
        at = (at + 8).saturating_add(len);
    }
    for list in [&mut tempos, &mut sigs, &mut bands, &mut seq] {
        list.sort_by_key(|t| t.mt);
    }
    let measure = match sigs.first().map(|s| &s.ev) {
        Some(&Ev::TimeSig { beats, beat, .. }) => {
            beats as i32 * (PPQ * 4 / beat.max(1) as i32).max(1)
        }
        _ => PPQ * 4,
    };
    let length = (last.saturating_add(measure - 1) / measure).max(1) * measure;
    let track = |class: &crate::win32::com::Guid, events| TrackData {
        class: class.0,
        group: 1,
        events,
    };
    Some(SegmentData {
        length,
        tracks: vec![
            track(&CLSID_CHORD_TRACK, Vec::new()),
            track(&CLSID_TEMPO_TRACK, tempos),
            track(&CLSID_BAND_TRACK, bands),
            track(&CLSID_TIME_SIG_TRACK, sigs),
            track(&CLSID_SEQ_TRACK, seq),
        ],
        ..SegmentData::default()
    })
}

/// A variable-length number at `at`.
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

/// The General MIDI instruments of the default collection (gm.dls): the
/// melodic ones by program, and the drum kits.
pub fn gm_collection() -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = GM_NAMES
        .iter()
        .enumerate()
        .map(|(i, n)| (i as u32, n.to_string()))
        .collect();
    for (program, name) in GM_KITS {
        out.push((0x8000_0000 | program, name.to_string()));
    }
    out
}

static GM_KITS: [(u32, &str); 9] = [
    (0, "Standard"),
    (8, "Room"),
    (16, "Power"),
    (24, "Electronic"),
    (25, "TR-808"),
    (32, "Jazz"),
    (40, "Brush"),
    (48, "Orchestra"),
    (56, "SFX"),
];

static GM_NAMES: [&str; 128] = [
    "Piano 1",
    "Piano 2",
    "Piano 3",
    "Honky-tonk",
    "E.Piano 1",
    "E.Piano 2",
    "Harpsichord",
    "Clav.",
    "Celesta",
    "Glockenspiel",
    "Music Box",
    "Vibraphone",
    "Marimba",
    "Xylophone",
    "Tubular-bell",
    "Santur",
    "Organ 1",
    "Organ 2",
    "Organ 3",
    "Church Org.1",
    "Reed Organ",
    "Accordion Fr",
    "Harmonica",
    "Bandoneon",
    "Nylon-str.Gt",
    "Steel-str.Gt",
    "Jazz Gt.",
    "Clean Gt.",
    "Muted Gt.",
    "Overdrive Gt",
    "DistortionGt",
    "Gt.Harmonics",
    "Acoustic Bs.",
    "Fingered Bs.",
    "Picked Bs.",
    "Fretless Bs.",
    "Slap Bass 1",
    "Slap Bass 2",
    "Synth Bass 1",
    "Synth Bass 2",
    "Violin",
    "Viola",
    "Cello",
    "Contrabass",
    "Tremolo Str",
    "PizzicatoStr",
    "Harp",
    "Timpani",
    "Strings",
    "Slow Strings",
    "Syn.Strings1",
    "Syn.Strings2",
    "Choir Aahs",
    "Voice Oohs",
    "SynVox",
    "OrchestraHit",
    "Trumpet",
    "Trombone",
    "Tuba",
    "MutedTrumpet",
    "French Horns",
    "Brass 1",
    "Synth Brass1",
    "Synth Brass2",
    "Soprano Sax",
    "Alto Sax",
    "Tenor Sax",
    "Baritone Sax",
    "Oboe",
    "English Horn",
    "Bassoon",
    "Clarinet",
    "Piccolo",
    "Flute",
    "Recorder",
    "Pan Flute",
    "Bottle Blow",
    "Shakuhachi",
    "Whistle",
    "Ocarina",
    "Square Wave",
    "Saw Wave",
    "Syn.Calliope",
    "Chiffer Lead",
    "Charang",
    "Solo Vox",
    "5th Saw Wave",
    "Bass & Lead",
    "Fantasia",
    "Warm Pad",
    "Polysynth",
    "Space Voice",
    "Bowed Glass",
    "Metal Pad",
    "Halo Pad",
    "Sweep Pad",
    "Ice Rain",
    "Soundtrack",
    "Crystal",
    "Atmosphere",
    "Brightness",
    "Goblin",
    "Echo Drops",
    "Star Theme",
    "Sitar",
    "Banjo",
    "Shamisen",
    "Koto",
    "Kalimba",
    "Bagpipe",
    "Fiddle",
    "Shanai",
    "Tinkle Bell",
    "Agogo",
    "Steel Drums",
    "Woodblock",
    "Taiko",
    "Melo. Tom 1",
    "Synth Drum",
    "Reverse Cym.",
    "Gt.FretNoise",
    "Breath Noise",
    "Seashore",
    "Bird",
    "Telephone 1",
    "Helicopter",
    "Applause",
    "Gun Shot",
];

crate::state_fields!(Timed { mt, ev });
crate::state_fields!(Inst {
    pchannel,
    patch,
    flags,
    pan,
    volume
});
crate::win32::state_variants!(Ev {
    Midi { pchannel, bytes },
    Note {
        pchannel,
        note,
        velocity,
        duration
    },
    Tempo { tempo },
    TimeSig { beats, beat, grids },
    Band { inst },
});

#[cfg(test)]
mod tests {
    use super::*;

    /// TEST.MID of the measurements on Windows 98 SE: format 1, 480
    /// ticks a beat, 120 beats a minute and 240 from beat 4, 4/4, eight
    /// beats of notes.
    pub fn test_mid() -> Vec<u8> {
        fn vlq(mut n: u32) -> Vec<u8> {
            let mut out = vec![(n & 0x7F) as u8];
            n >>= 7;
            while n != 0 {
                out.push(0x80 | (n & 0x7F) as u8);
                n >>= 7;
            }
            out.reverse();
            out
        }
        fn track(mut events: Vec<(u32, Vec<u8>)>) -> Vec<u8> {
            events.sort_by_key(|e| e.0);
            let mut data = Vec::new();
            let mut last = 0;
            for (tick, ev) in events {
                data.extend(vlq(tick - last));
                data.extend(ev);
                last = tick;
            }
            data.extend([0, 0xFF, 0x2F, 0]);
            let mut out = b"MTrk".to_vec();
            out.extend((data.len() as u32).to_be_bytes());
            out.extend(data);
            out
        }
        let meta = vec![
            (0, vec![0xFF, 0x51, 3, 0x07, 0xA1, 0x20]),
            (0, vec![0xFF, 0x58, 4, 4, 2, 0x18, 8]),
            (4 * 480, vec![0xFF, 0x51, 3, 0x03, 0xD0, 0x90]),
        ];
        let mut notes = vec![(0, vec![0xC0, 0x18]), (0, vec![0xB0, 7, 100])];
        for beat in 0..8u32 {
            notes.push((beat * 480, vec![0x90, 60 + beat as u8, 100]));
            notes.push((beat * 480 + 240, vec![0x80, 60 + beat as u8, 0]));
        }
        let mut out = b"MThd".to_vec();
        out.extend(6u32.to_be_bytes());
        out.extend([0, 1, 0, 2, 0x01, 0xE0]);
        out.extend(track(meta));
        out.extend(track(notes));
        out
    }

    #[test]
    fn a_midi_file_becomes_the_tracks_windows_makes() {
        let s = midi_file(&test_mid()).unwrap();
        // As Windows 98 SE has it: 6144 long, the five tracks.
        assert_eq!(s.length, 6144);
        let classes: Vec<[u8; 16]> = s.tracks.iter().map(|t| t.class).collect();
        assert_eq!(
            classes,
            [
                CLSID_CHORD_TRACK.0,
                CLSID_TEMPO_TRACK.0,
                CLSID_BAND_TRACK.0,
                CLSID_TIME_SIG_TRACK.0,
                CLSID_SEQ_TRACK.0
            ]
        );
        let tempos: Vec<(i32, f64)> = s.tracks[1]
            .events
            .iter()
            .filter_map(|t| match t.ev {
                Ev::Tempo { tempo } => Some((t.mt, tempo)),
                _ => None,
            })
            .collect();
        assert_eq!(tempos, [(0, 120.0), (3072, 240.0)]);
        assert_eq!(s.tracks[2].events.len(), 1);
        // A note at beat 1 is at 768.
        assert!(
            s.tracks[4].events.iter().any(|t| t.mt == 768
                && matches!(&t.ev, Ev::Midi { bytes, .. } if bytes[..2] == [0x90, 61]))
        );
    }

    #[test]
    fn dls_patches_are_directmusic_patches() {
        assert_eq!(patch_of_locale(0x8000_0000, 25), 0x8000_0019);
        assert_eq!(patch_of_locale(0x0102, 5), 0x0001_0205);
    }

    #[test]
    fn curves_step_from_start_to_end() {
        let mut item = Vec::new();
        for v in [0i32, 768, 0, 3] {
            item.extend(v.to_le_bytes());
        }
        for v in [0i16, 0, 127, 0] {
            item.extend(v.to_le_bytes());
        }
        item.extend([CURVE_CC, 0, 7, 0]);
        let mut out = Vec::new();
        curve(&item, &mut out);
        assert_eq!(out.len(), 17);
        assert!(matches!(&out[16].ev, Ev::Midi { pchannel: 3, bytes } if *bytes == [0xB0, 7, 127]));
    }
}
