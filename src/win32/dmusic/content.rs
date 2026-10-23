//! Bands (DMBAND.DLL), styles (DMSTYLE.DLL), chord maps and the composer
//! (DMCOMPOS.DLL).
//!
//! A band sets up instruments: it plays as a band track's event of a
//! segment. A style keeps its tempo, time signature and bands; its
//! patterns and motifs aren't read, since the composition engine that
//! plays them isn't there. A chord map keeps its scale, and the composer
//! makes nothing.

use super::files::{Ev, Inst, SegmentData, Timed, TrackData};
use super::guids::*;
use super::{
    BAND, CHORD_MAP, DMUS_E_NOT_FOUND, E_INVALIDARG, E_POINTER, Kind, S_FALSE, S_OK, STYLE, add,
    give, give_new, read_wide, state, this_of, write_wide,
};
use crate::cpu::Cpu;
use crate::win32::com::{self, Guid};

/// A band's instruments.
#[derive(Clone, Debug, Default)]
pub struct Band {
    pub inst: Vec<Inst>,
}

/// A style: its tempo, time signature (beats a measure, the beat's
/// note, grids a beat), and bands (name, band object).
#[derive(Clone, Debug, Default)]
pub struct Style {
    pub tempo: f64,
    pub sig: (u8, u8, u16),
    pub bands: Vec<(String, u32)>,
}

/// CoCreateInstance of a band: no instruments.
pub fn create_band(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = new_band(cpu, Vec::new());
    give_new(cpu, id, iid, out)
}

/// CoCreateInstance of a style: 4/4 at 120.
pub fn create_style(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = add(
        cpu,
        Kind::Style {
            s: Style {
                tempo: 120.0,
                sig: (4, 4, 4),
                bands: Vec::new(),
            },
        },
    );
    give_new(cpu, id, iid, out)
}

/// CoCreateInstance of a chord map: the major scale.
pub fn create_chord_map(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = add(cpu, Kind::ChordMap { scale: MAJOR });
    give_new(cpu, id, iid, out)
}

/// The major scale's notes from C, two octaves (as DirectMusic's
/// default scale).
pub const MAJOR: u32 = 0x00AB_5AB5;

/// CoCreateInstance of the composer.
pub fn create_composer(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let id = add(cpu, Kind::Composer {});
    give_new(cpu, id, iid, out)
}

/// A band object of `inst`: its id.
pub fn new_band(cpu: &mut Cpu, inst: Vec<Inst>) -> u32 {
    add(cpu, Kind::Band { b: Band { inst } })
}

/// The instruments of the band the program's pointer `this` is.
pub fn band_instruments(cpu: &mut Cpu, this: u32) -> Option<Vec<Inst>> {
    let id = this_of(cpu, this, BAND)?;
    match &state(cpu).get(id)?.kind {
        Kind::Band { b } => Some(b.inst.clone()),
        _ => None,
    }
}

fn style_of(cpu: &mut Cpu, this: u32) -> Option<Style> {
    let id = this_of(cpu, this, STYLE)?;
    match &state(cpu).get(id)?.kind {
        Kind::Style { s } => Some(s.clone()),
        _ => None,
    }
}

/// What has no object of its kind here: DMUS_E_NOT_FOUND.
pub fn not_found(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DMUS_E_NOT_FOUND)
}

/// The characters of DirectMusic's names (DMUS_MAX_NAME).
const MAX_NAME: usize = 64;

/// IDirectMusicStyle::GetBand(this, name, band).
pub fn style_get_band(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(s) = style_of(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 || args[2] == 0 {
        return Some(E_POINTER);
    }
    let name = read_wide(cpu, args[1], MAX_NAME);
    let Some(&(_, band)) = s.bands.iter().find(|b| b.0.eq_ignore_ascii_case(&name)) else {
        com::write_out(cpu, args[2], 0);
        return Some(DMUS_E_NOT_FOUND);
    };
    Some(give(cpu, band, &IID_IDIRECTMUSIC_BAND, args[2]))
}

/// IDirectMusicStyle::EnumBand(this, index, name): S_FALSE past the
/// last.
pub fn style_enum_band(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(s) = style_of(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[2] == 0 {
        return Some(E_POINTER);
    }
    let Some((name, _)) = s.bands.get(args[1] as usize) else {
        return Some(S_FALSE);
    };
    write_wide(cpu, args[2], name, MAX_NAME);
    Some(S_OK)
}

/// IDirectMusicStyle::GetDefaultBand(this, band): the first; S_FALSE
/// for a style with none.
pub fn style_get_default_band(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(s) = style_of(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    let Some(&(_, band)) = s.bands.first() else {
        com::write_out(cpu, args[1], 0);
        return Some(S_FALSE);
    };
    Some(give(cpu, band, &IID_IDIRECTMUSIC_BAND, args[1]))
}

/// IDirectMusicStyle::GetTimeSignature(this, signature).
pub fn style_get_time_signature(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(s) = style_of(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    let at = args[1];
    if at == 0 || !cpu.bus.guest_probe(at, 8, true) {
        return Some(E_POINTER);
    }
    let (beats, beat, grids) = s.sig;
    cpu.bus.guest_write_32(at, 0);
    cpu.bus.guest_write_bytes(at + 4, &[beats, beat]);
    cpu.bus.guest_write_16(at + 6, grids);
    Some(S_OK)
}

/// IDirectMusicStyle::GetTempo(this, tempo).
pub fn style_get_tempo(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(s) = style_of(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 || !cpu.bus.guest_probe(args[1], 8, true) {
        return Some(E_POINTER);
    }
    cpu.bus.guest_write_bytes(args[1], &s.tempo.to_le_bytes());
    Some(S_OK)
}

/// IDirectMusicBand::CreateSegment(this, segment): a segment of one
/// band track with the band at its start.
pub fn band_create_segment(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(inst) = band_instruments(cpu, args[0]) else {
        return Some(E_INVALIDARG);
    };
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    let data = SegmentData {
        length: 1,
        tracks: vec![TrackData {
            class: CLSID_BAND_TRACK.0,
            group: 1,
            events: vec![Timed {
                mt: 0,
                ev: Ev::Band { inst },
            }],
        }],
        ..SegmentData::default()
    };
    let id = super::segment::new_segment(cpu, data);
    Some(give_new(cpu, id, &IID_IDIRECTMUSIC_SEGMENT, args[1]))
}

/// IDirectMusicBand::Download and Unload(this, performance): the
/// synthesizer has the General MIDI instruments the bands name.
pub fn band_download(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if band_instruments(cpu, args[0]).is_none() {
        return Some(E_INVALIDARG);
    }
    if args[1] == 0 {
        return Some(E_POINTER);
    }
    Some(S_OK)
}

/// IDirectMusicChordMap::GetScale(this, scale).
pub fn chord_map_get_scale(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = this_of(cpu, args[0], CHORD_MAP) else {
        return Some(E_INVALIDARG);
    };
    let Kind::ChordMap { scale } = state(cpu).get(id)?.kind else {
        return Some(E_INVALIDARG);
    };
    Some(if com::write_out(cpu, args[1], scale) {
        S_OK
    } else {
        E_POINTER
    })
}

crate::state_fields!(Band { inst });
crate::state_fields!(Style { tempo, sig, bands });
