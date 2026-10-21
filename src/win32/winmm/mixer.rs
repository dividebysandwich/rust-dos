//! The mixer (mixer*) and the auxiliary audio device (aux*): the volumes
//! and mutes of the sound card's mixer, which scale what the emulated
//! devices play.
//!
//! The one mixer device has one destination, the speakers, with a master
//! volume and mute, and three sources into it, wave audio, the MIDI
//! synthesizer and CD audio, each with a volume and a mute of its own. A
//! source plays at its volume times the master volume (`gain`), and not
//! at all when it or the master is muted. The auxiliary device is the CD
//! audio input, whose volume is the CD line's.
//!
//! The ANSI functions are there; the Unicode ones fail with
//! MMSYSERR_NOTSUPPORTED, as Windows 95's do.

use super::callback::{self, Callback};
use super::*;
use crate::cpu::Cpu;

/// mixerGetLineInfoA's, mixerGetLineControlsA's and the details' flags.
const MIXER_GETLINEINFOF_DESTINATION: u32 = 0;
const MIXER_GETLINEINFOF_SOURCE: u32 = 1;
const MIXER_GETLINEINFOF_LINEID: u32 = 2;
const MIXER_GETLINEINFOF_COMPONENTTYPE: u32 = 3;
const MIXER_GETLINEINFOF_TARGETTYPE: u32 = 4;
const MIXER_GETLINECONTROLSF_ALL: u32 = 0;
const MIXER_GETLINECONTROLSF_ONEBYID: u32 = 1;
const MIXER_GETLINECONTROLSF_ONEBYTYPE: u32 = 2;
const MIXER_GETCONTROLDETAILSF_VALUE: u32 = 0;
const MIXER_QUERYMASK: u32 = 0xF;

/// The kinds of object mixerOpen and the others take, in the high bits
/// of their flags.
const MIXER_OBJECTF_HANDLE: u32 = 0x8000_0000;
const MIXER_OBJECTF_TYPEMASK: u32 = 0x7000_0000;
const MIXER_OBJECTF_MIXER: u32 = 0x0000_0000;
const MIXER_OBJECTF_WAVEOUT: u32 = 0x1000_0000;
const MIXER_OBJECTF_WAVEIN: u32 = 0x2000_0000;
const MIXER_OBJECTF_MIDIOUT: u32 = 0x3000_0000;
const MIXER_OBJECTF_MIDIIN: u32 = 0x4000_0000;
const MIXER_OBJECTF_AUX: u32 = 0x5000_0000;

/// The errors of the mixer.
const MIXERR_INVALLINE: u32 = 1024;
const MIXERR_INVALCONTROL: u32 = 1025;

/// The window message of a control's change.
const MM_MIXM_LINE_CHANGE: u32 = 0x3D0;
const MM_MIXM_CONTROL_CHANGE: u32 = 0x3D1;

/// MIXERLINE's flags, component types and target types.
const MIXERLINE_LINEF_ACTIVE: u32 = 0x0000_0001;
const MIXERLINE_LINEF_SOURCE: u32 = 0x8000_0000;
const COMPONENTTYPE_DST_SPEAKERS: u32 = 4;
const COMPONENTTYPE_SRC_SYNTHESIZER: u32 = 0x1004;
const COMPONENTTYPE_SRC_COMPACTDISC: u32 = 0x1005;
const COMPONENTTYPE_SRC_WAVEOUT: u32 = 0x1008;
const TARGETTYPE_UNDEFINED: u32 = 0;
const TARGETTYPE_WAVEOUT: u32 = 1;
const TARGETTYPE_MIDIOUT: u32 = 3;
const TARGETTYPE_AUX: u32 = 5;

/// The controls' types.
const CONTROLTYPE_VOLUME: u32 = 0x5003_0001;
const CONTROLTYPE_MUTE: u32 = 0x2001_0002;
const MIXERCONTROL_CONTROLF_UNIFORM: u32 = 1;

/// The structures' sizes.
const MIXERCAPSA_SIZE: u32 = 48;
const MIXERLINEA_SIZE: u32 = 168;
const MIXERLINECONTROLSA_SIZE: u32 = 24;
const MIXERCONTROLA_SIZE: u32 = 148;
const MIXERCONTROLDETAILS_SIZE: u32 = 24;

/// AUXCAPS's technology and support flags.
const AUXCAPS_CDAUDIO: u16 = 1;
const AUXCAPS_VOLUME: u32 = 1;
const AUXCAPS_LRVOLUME: u32 = 2;

/// The volume steps a volume control has, as a sound card's mixer of
/// the time had.
const VOLUME_STEPS: u32 = 64;

/// The lines, by their ids: the destination and the three sources.
pub const SPEAKERS: usize = 0;
pub const WAVE: usize = 1;
pub const SYNTH: usize = 2;
pub const CD: usize = 3;
const LINES: usize = 4;

/// A line's volume (0 to 65535 on each side) and mute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub left: u32,
    pub right: u32,
    pub mute: bool,
}

impl Default for Line {
    fn default() -> Line {
        Line {
            left: 0xFFFF,
            right: 0xFFFF,
            mute: false,
        }
    }
}

/// A mixer mixerOpen opened.
#[derive(Clone, Debug, Default)]
pub struct Open {
    pub handle: u32,
    pub pid: u32,
    pub callback: Callback,
}

/// The mixer's state.
#[derive(Clone, Debug, Default)]
pub struct Mixer {
    pub lines: [Line; LINES],
    pub opens: Vec<Open>,
}

impl Mixer {
    /// The scale (left, right) the source `line` plays at.
    pub fn gain(&self, line: usize) -> (f32, f32) {
        let (m, s) = (&self.lines[SPEAKERS], &self.lines[line]);
        if m.mute || s.mute {
            return (0.0, 0.0);
        }
        let scale = |a: u32, b: u32| (a as f32 / 65535.0) * (b as f32 / 65535.0);
        (scale(m.left, s.left), scale(m.right, s.right))
    }

    pub fn midi_gain(&self) -> (f32, f32) {
        self.gain(SYNTH)
    }

    pub fn cd_gain(&self) -> (f32, f32) {
        self.gain(CD)
    }
}

fn mixer(cpu: &mut Cpu) -> &mut Mixer {
    &mut cpu.bus.win32.winmm.mixer
}

fn find(cpu: &Cpu, handle: u32) -> Option<usize> {
    cpu.bus
        .win32
        .winmm
        .mixer
        .opens
        .iter()
        .position(|o| o.handle == handle && handle != 0)
}

/// The mixer device an object of mixerOpen's or the others' `flags`
/// names: the mixer (0), or the error.
fn device_of(cpu: &Cpu, object: u32, flags: u32) -> Result<(), u32> {
    let handle = flags & MIXER_OBJECTF_HANDLE != 0;
    match flags & MIXER_OBJECTF_TYPEMASK {
        MIXER_OBJECTF_MIXER if handle => {
            find(cpu, object).ok_or(MMSYSERR_INVALHANDLE)?;
        }
        // A mixer's handle does for its id.
        MIXER_OBJECTF_MIXER if object != 0 && find(cpu, object).is_none() => {
            return Err(MMSYSERR_BADDEVICEID);
        }
        MIXER_OBJECTF_AUX if object != 0 => return Err(MMSYSERR_BADDEVICEID),
        MIXER_OBJECTF_MIDIOUT if handle => {
            if !super::midi::is_handle(cpu, object) {
                return Err(MMSYSERR_INVALHANDLE);
            }
        }
        MIXER_OBJECTF_MIDIOUT if object != 0 && object != super::midi::MIDI_MAPPER => {
            return Err(MMSYSERR_BADDEVICEID);
        }
        MIXER_OBJECTF_WAVEOUT if handle => {
            if object == 0 {
                return Err(MMSYSERR_INVALHANDLE);
            }
        }
        MIXER_OBJECTF_WAVEOUT if object != 0 && object != u32::MAX => {
            return Err(MMSYSERR_BADDEVICEID);
        }
        // Nothing records: no mixer goes with an input.
        MIXER_OBJECTF_WAVEIN | MIXER_OBJECTF_MIDIIN => {
            return Err(if handle {
                MMSYSERR_INVALHANDLE
            } else {
                MMSYSERR_BADDEVICEID
            });
        }
        MIXER_OBJECTF_MIXER | MIXER_OBJECTF_AUX | MIXER_OBJECTF_MIDIOUT | MIXER_OBJECTF_WAVEOUT => {
        }
        _ => return Err(MMSYSERR_INVALFLAG),
    }
    Ok(())
}

/// mixerGetNumDevs: one.
pub fn mixer_get_num_devs(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

/// mixerGetDevCapsA(id, caps, size): MIXERCAPSA, as much as `size` has
/// room for. The id may be a mixer's handle.
pub fn mixer_get_dev_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, at, size] = args else {
        unreachable!()
    };
    if id != 0 && find(cpu, id).is_none() {
        return Some(MMSYSERR_BADDEVICEID);
    }
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    let mut caps = Vec::with_capacity(MIXERCAPSA_SIZE as usize);
    caps.extend_from_slice(&MM_MICROSOFT.to_le_bytes());
    caps.extend_from_slice(&MM_MIXER_PID.to_le_bytes());
    caps.extend_from_slice(&0x0400u32.to_le_bytes());
    caps.extend_from_slice(&name_field("Sound Card Mixer", 32));
    caps.extend_from_slice(&0u32.to_le_bytes());
    caps.extend_from_slice(&1u32.to_le_bytes());
    write_struct(cpu, at, size, &caps)
}

/// The product id the mixer has in its caps.
const MM_MIXER_PID: u16 = 0x0064;

/// mixerOpen(handle, id, callback, instance, flags): a window callback
/// gets MM_MIXM_LINE_CHANGE and MM_MIXM_CONTROL_CHANGE; other callbacks
/// Windows doesn't take.
pub fn mixer_open(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[out, id, cb, instance, flags] = args else {
        unreachable!()
    };
    let _ = instance;
    if let Err(e) = device_of(cpu, id, flags & !callback::CALLBACK_TYPEMASK) {
        return Some(e);
    }
    let callback = match flags & callback::CALLBACK_TYPEMASK {
        callback::CALLBACK_NULL => Callback::None,
        callback::CALLBACK_WINDOW => {
            if cb != 0 && !cpu.bus.win32.user.is_window(cb) {
                return Some(MMSYSERR_INVALPARAM);
            }
            Callback::Window { hwnd: cb }
        }
        _ => return Some(MMSYSERR_INVALFLAG),
    };
    if out == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(out, 4, true) {
        return None;
    }
    let handle = new_handle(cpu);
    let pid = crate::win32::user32::current_pid(cpu);
    mixer(cpu).opens.push(Open {
        handle,
        pid,
        callback,
    });
    cpu.bus.guest_write_32(out, handle);
    Some(MMSYSERR_NOERROR)
}

/// mixerClose(handle).
pub fn mixer_close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = find(cpu, args[0]) else {
        return Some(MMSYSERR_INVALHANDLE);
    };
    mixer(cpu).opens.remove(i);
    Some(MMSYSERR_NOERROR)
}

/// mixerGetID(object, id, flags): the mixer of the object, 0.
pub fn mixer_get_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[object, at, flags] = args else {
        unreachable!()
    };
    if let Err(e) = device_of(cpu, object, flags) {
        return Some(e);
    }
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(at, 4, true) {
        return None;
    }
    cpu.bus.guest_write_32(at, 0);
    Some(MMSYSERR_NOERROR)
}

/// What a line is: its component and target type, names, and the device
/// it goes with.
fn line_info(line: usize) -> (u32, u32, &'static str, &'static str) {
    match line {
        SPEAKERS => (
            COMPONENTTYPE_DST_SPEAKERS,
            TARGETTYPE_WAVEOUT,
            "Volume",
            "Volume Control",
        ),
        WAVE => (
            COMPONENTTYPE_SRC_WAVEOUT,
            TARGETTYPE_WAVEOUT,
            "Wave",
            "Wave",
        ),
        SYNTH => (
            COMPONENTTYPE_SRC_SYNTHESIZER,
            TARGETTYPE_MIDIOUT,
            "MIDI",
            "MIDI Synthesizer",
        ),
        _ => (
            COMPONENTTYPE_SRC_COMPACTDISC,
            TARGETTYPE_AUX,
            "CD Audio",
            "CD Audio",
        ),
    }
}

/// The MIXERLINEA of the line `line`.
fn line_struct(line: usize) -> Vec<u8> {
    let (component, target, short, name) = line_info(line);
    let source = line != SPEAKERS;
    let mut s = Vec::with_capacity(MIXERLINEA_SIZE as usize);
    let put = |s: &mut Vec<u8>, v: u32| s.extend_from_slice(&v.to_le_bytes());
    put(&mut s, MIXERLINEA_SIZE);
    put(&mut s, 0);
    put(&mut s, if source { line as u32 - 1 } else { 0 });
    put(&mut s, line_id(line));
    put(
        &mut s,
        if source {
            MIXERLINE_LINEF_SOURCE | MIXERLINE_LINEF_ACTIVE
        } else {
            MIXERLINE_LINEF_ACTIVE
        },
    );
    put(&mut s, 0);
    put(&mut s, component);
    put(&mut s, 2);
    put(&mut s, if source { 0 } else { LINES as u32 - 1 });
    put(&mut s, 2);
    s.extend_from_slice(&name_field(short, 16));
    s.extend_from_slice(&name_field(name, 64));
    s.extend_from_slice(&target.to_le_bytes());
    s.extend_from_slice(&0u32.to_le_bytes());
    let (mid, pid) = if target == TARGETTYPE_UNDEFINED {
        (0, 0)
    } else {
        (MM_MICROSOFT, MM_MIXER_PID)
    };
    s.extend_from_slice(&mid.to_le_bytes());
    s.extend_from_slice(&pid.to_le_bytes());
    s.extend_from_slice(&0x0400u32.to_le_bytes());
    let device = match target {
        TARGETTYPE_MIDIOUT => "MPU-401 Synthesizer",
        TARGETTYPE_AUX => "CD Audio",
        _ => "Sound Card Wave Out",
    };
    s.extend_from_slice(&name_field(device, 32));
    s
}

/// A line's id: the destination 0xFFFF0000, as Windows numbers it, the
/// sources their index in the low word.
fn line_id(line: usize) -> u32 {
    if line == SPEAKERS {
        0xFFFF_0000
    } else {
        line as u32 - 1
    }
}

fn line_of_id(id: u32) -> Option<usize> {
    match id {
        0xFFFF_0000 => Some(SPEAKERS),
        0..=2 => Some(id as usize + 1),
        _ => None,
    }
}

/// mixerGetLineInfoA(object, line, flags): the line the query of
/// `flags` asks for, in the MIXERLINEA.
pub fn mixer_get_line_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[object, at, flags] = args else {
        unreachable!()
    };
    if flags & !(MIXER_QUERYMASK | MIXER_OBJECTF_HANDLE | MIXER_OBJECTF_TYPEMASK) != 0
        || flags & MIXER_QUERYMASK > MIXER_GETLINEINFOF_TARGETTYPE
    {
        return Some(MMSYSERR_INVALFLAG);
    }
    if let Err(e) = device_of(cpu, object, flags & !MIXER_QUERYMASK) {
        return Some(e);
    }
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(at, MIXERLINEA_SIZE as usize, true) {
        return None;
    }
    if cpu.bus.guest_read_32(at) < MIXERLINEA_SIZE {
        return Some(MMSYSERR_INVALPARAM);
    }
    let read = |cpu: &mut Cpu, off: u32| cpu.bus.guest_read_32(at + off);
    let line = match flags & MIXER_QUERYMASK {
        MIXER_GETLINEINFOF_DESTINATION => (read(cpu, 4) == 0).then_some(SPEAKERS),
        MIXER_GETLINEINFOF_SOURCE => {
            let source = read(cpu, 8) as usize;
            (read(cpu, 4) == 0 && source < LINES - 1).then_some(source + 1)
        }
        MIXER_GETLINEINFOF_LINEID => line_of_id(read(cpu, 12)),
        MIXER_GETLINEINFOF_COMPONENTTYPE => {
            let component = read(cpu, 24);
            (0..LINES).find(|&l| line_info(l).0 == component)
        }
        _ => {
            let target = read(cpu, 120);
            [WAVE, SYNTH, CD]
                .into_iter()
                .find(|&l| line_info(l).1 == target)
        }
    };
    let Some(line) = line else {
        return Some(MIXERR_INVALLINE);
    };
    let s = line_struct(line);
    cpu.bus.guest_write_bytes(at, &s);
    Some(MMSYSERR_NOERROR)
}

/// A control's id: two to a line, the volume then the mute.
fn control_id(line: usize, mute: bool) -> u32 {
    line as u32 * 2 + 1 + mute as u32
}

/// The line and whether it is the mute, of the control `id`.
fn control_of_id(id: u32) -> Option<(usize, bool)> {
    let k = id.checked_sub(1)? as usize;
    (k < LINES * 2).then_some((k / 2, k % 2 == 1))
}

/// The MIXERCONTROLA of a control.
fn control_struct(line: usize, mute: bool) -> Vec<u8> {
    let mut s = Vec::with_capacity(MIXERCONTROLA_SIZE as usize);
    let put = |s: &mut Vec<u8>, v: u32| s.extend_from_slice(&v.to_le_bytes());
    put(&mut s, MIXERCONTROLA_SIZE);
    put(&mut s, control_id(line, mute));
    if mute {
        put(&mut s, CONTROLTYPE_MUTE);
        put(&mut s, MIXERCONTROL_CONTROLF_UNIFORM);
    } else {
        put(&mut s, CONTROLTYPE_VOLUME);
        put(&mut s, 0);
    }
    put(&mut s, 0);
    let (short, name) = if mute {
        ("Mute", "Mute")
    } else {
        ("Volume", "Volume")
    };
    s.extend_from_slice(&name_field(short, 16));
    s.extend_from_slice(&name_field(name, 64));
    // Bounds: the least and most, and four reserved.
    put(&mut s, 0);
    put(&mut s, if mute { 1 } else { 0xFFFF });
    s.extend_from_slice(&[0; 16]);
    // Metrics: the steps, and five reserved.
    put(&mut s, if mute { 0 } else { VOLUME_STEPS });
    s.extend_from_slice(&[0; 20]);
    s
}

/// mixerGetLineControlsA(object, controls, flags): the line's controls,
/// all of them, or the one of an id or type.
pub fn mixer_get_line_controls(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[object, at, flags] = args else {
        unreachable!()
    };
    if flags & !(MIXER_QUERYMASK | MIXER_OBJECTF_HANDLE | MIXER_OBJECTF_TYPEMASK) != 0
        || flags & MIXER_QUERYMASK > MIXER_GETLINECONTROLSF_ONEBYTYPE
    {
        return Some(MMSYSERR_INVALFLAG);
    }
    if let Err(e) = device_of(cpu, object, flags & !MIXER_QUERYMASK) {
        return Some(e);
    }
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu
        .bus
        .guest_probe(at, MIXERLINECONTROLSA_SIZE as usize, true)
    {
        return None;
    }
    let w: [u32; 6] = std::array::from_fn(|k| cpu.bus.guest_read_32(at + 4 * k as u32));
    let [cb, line_id, which, count, cbmxctrl, array] = w;
    if cb < MIXERLINECONTROLSA_SIZE || cbmxctrl < MIXERCONTROLA_SIZE || array == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    let controls: Vec<(usize, bool)> = match flags & MIXER_QUERYMASK {
        MIXER_GETLINECONTROLSF_ALL => {
            let Some(line) = line_of_id(line_id) else {
                return Some(MIXERR_INVALLINE);
            };
            if count != 2 {
                return Some(MMSYSERR_INVALPARAM);
            }
            vec![(line, false), (line, true)]
        }
        MIXER_GETLINECONTROLSF_ONEBYID => {
            let Some(control) = control_of_id(which) else {
                return Some(MIXERR_INVALCONTROL);
            };
            // The line the control is on goes in dwLineID.
            cpu.bus.guest_write_32(at + 4, self::line_id(control.0));
            vec![control]
        }
        _ => {
            let Some(line) = line_of_id(line_id) else {
                return Some(MIXERR_INVALLINE);
            };
            match which {
                CONTROLTYPE_VOLUME => vec![(line, false)],
                CONTROLTYPE_MUTE => vec![(line, true)],
                _ => return Some(MIXERR_INVALCONTROL),
            }
        }
    };
    if !cpu
        .bus
        .guest_probe(array, (cbmxctrl as usize) * controls.len(), true)
    {
        return None;
    }
    for (k, &(line, mute)) in controls.iter().enumerate() {
        let s = control_struct(line, mute);
        cpu.bus.guest_write_bytes(array + cbmxctrl * k as u32, &s);
    }
    Some(MMSYSERR_NOERROR)
}

/// What a MIXERCONTROLDETAILS asks for: the control (its line and
/// whether it is the mute), the channels, and where the values go, one
/// every `stride` bytes.
struct Details {
    line: usize,
    mute: bool,
    channels: u32,
    stride: u32,
    place: u32,
}

/// The MIXERCONTROLDETAILS at `at`, or the error; None when its page
/// isn't there.
fn details(cpu: &mut Cpu, at: u32) -> Option<Result<Details, u32>> {
    if at == 0 {
        return Some(Err(MMSYSERR_INVALPARAM));
    }
    if !cpu
        .bus
        .guest_probe(at, MIXERCONTROLDETAILS_SIZE as usize, false)
    {
        return None;
    }
    let w: [u32; 6] = std::array::from_fn(|k| cpu.bus.guest_read_32(at + 4 * k as u32));
    let [cb, id, channels, multiple, cb_details, place] = w;
    if cb < MIXERCONTROLDETAILS_SIZE || multiple != 0 || cb_details < 4 || place == 0 {
        return Some(Err(MMSYSERR_INVALPARAM));
    }
    let Some((line, mute)) = control_of_id(id) else {
        return Some(Err(MIXERR_INVALCONTROL));
    };
    if !(channels == 1 || channels == 2 && !mute) {
        return Some(Err(MMSYSERR_INVALPARAM));
    }
    if !cpu
        .bus
        .guest_probe(place, (cb_details * channels) as usize, true)
    {
        return None;
    }
    Some(Ok(Details {
        line,
        mute,
        channels,
        stride: cb_details,
        place,
    }))
}

/// mixerGetControlDetailsA(object, details, flags): a control's value,
/// for one channel or each.
pub fn mixer_get_control_details(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[object, at, flags] = args else {
        unreachable!()
    };
    if flags & MIXER_QUERYMASK != MIXER_GETCONTROLDETAILSF_VALUE {
        return Some(MMSYSERR_INVALPARAM);
    }
    if let Err(e) = device_of(cpu, object, flags & !MIXER_QUERYMASK) {
        return Some(e);
    }
    let Details {
        line,
        mute,
        channels,
        stride,
        place,
    } = match details(cpu, at)? {
        Ok(d) => d,
        Err(e) => return Some(e),
    };
    let l = mixer(cpu).lines[line].clone();
    let values = if mute {
        vec![l.mute as u32]
    } else if channels == 1 {
        vec![l.left.max(l.right)]
    } else {
        vec![l.left, l.right]
    };
    for (k, v) in values.into_iter().enumerate() {
        cpu.bus.guest_write_32(place + stride * k as u32, v);
    }
    Some(MMSYSERR_NOERROR)
}

/// mixerSetControlDetails(object, details, flags): set a control, and
/// tell the open mixers' windows.
pub fn mixer_set_control_details(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[object, at, flags] = args else {
        unreachable!()
    };
    if flags & MIXER_QUERYMASK != MIXER_GETCONTROLDETAILSF_VALUE {
        return Some(MMSYSERR_INVALPARAM);
    }
    if let Err(e) = device_of(cpu, object, flags & !MIXER_QUERYMASK) {
        return Some(e);
    }
    let Details {
        line,
        mute,
        channels,
        stride,
        place,
    } = match details(cpu, at)? {
        Ok(d) => d,
        Err(e) => return Some(e),
    };
    let first = cpu.bus.guest_read_32(place);
    let second = if channels == 2 {
        cpu.bus.guest_read_32(place + stride)
    } else {
        first
    };
    cpu.bus.audio_catch_up();
    let l = &mut mixer(cpu).lines[line];
    if mute {
        l.mute = first != 0;
    } else {
        l.left = first.min(0xFFFF);
        l.right = second.min(0xFFFF);
    }
    changed(cpu, MM_MIXM_CONTROL_CHANGE, control_id(line, mute));
    Some(MMSYSERR_NOERROR)
}

/// Tell the open mixers' windows of a change.
fn changed(cpu: &mut Cpu, msg: u32, id: u32) {
    let opens: Vec<Open> = mixer(cpu).opens.clone();
    for o in opens {
        callback::notify(cpu, &o.callback, o.handle, msg, 0, id, 0);
    }
}

/// Set a line's volume from elsewhere (auxSetVolume, midiOutSetVolume
/// on the mixer's MIDI line): the windows hear of it.
fn set_line_volume(cpu: &mut Cpu, line: usize, volume: u32) {
    cpu.bus.audio_catch_up();
    let l = &mut mixer(cpu).lines[line];
    l.left = volume & 0xFFFF;
    l.right = volume >> 16;
    changed(cpu, MM_MIXM_CONTROL_CHANGE, control_id(line, false));
    let _ = MM_MIXM_LINE_CHANGE;
}

/// mixerMessage(object, message, p1, p2): no driver messages.
pub fn mixer_message(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if find(cpu, args[0]).is_none() && args[0] != 0 {
        return Some(MMSYSERR_INVALHANDLE);
    }
    Some(MMSYSERR_NOTSUPPORTED)
}

/// auxGetNumDevs: one, the CD audio input.
pub fn aux_get_num_devs(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

/// auxGetDevCapsA(id, caps, size).
pub fn aux_get_dev_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, at, size] = args else {
        unreachable!()
    };
    if id != 0 && id != u32::MAX {
        return Some(MMSYSERR_BADDEVICEID);
    }
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    let mut caps = Vec::with_capacity(48);
    caps.extend_from_slice(&MM_MICROSOFT.to_le_bytes());
    caps.extend_from_slice(&MM_MIXER_PID.to_le_bytes());
    caps.extend_from_slice(&0x0400u32.to_le_bytes());
    caps.extend_from_slice(&name_field("CD Audio", 32));
    caps.extend_from_slice(&AUXCAPS_CDAUDIO.to_le_bytes());
    caps.extend_from_slice(&0u16.to_le_bytes());
    caps.extend_from_slice(&(AUXCAPS_VOLUME | AUXCAPS_LRVOLUME).to_le_bytes());
    write_struct(cpu, at, size, &caps)
}

/// auxGetVolume(id, volume): the CD line's volume.
pub fn aux_get_volume(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, at] = args else { unreachable!() };
    if id != 0 {
        return Some(MMSYSERR_BADDEVICEID);
    }
    if at == 0 {
        return Some(MMSYSERR_INVALPARAM);
    }
    if !cpu.bus.guest_probe(at, 4, true) {
        return None;
    }
    let l = &mixer(cpu).lines[CD];
    let volume = l.left | l.right << 16;
    cpu.bus.guest_write_32(at, volume);
    Some(MMSYSERR_NOERROR)
}

/// auxSetVolume(id, volume).
pub fn aux_set_volume(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, volume] = args else { unreachable!() };
    if id != 0 {
        return Some(MMSYSERR_BADDEVICEID);
    }
    set_line_volume(cpu, CD, volume);
    Some(MMSYSERR_NOERROR)
}

/// auxOutMessage(id, message, p1, p2).
pub fn aux_out_message(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] != 0 {
        return Some(MMSYSERR_BADDEVICEID);
    }
    Some(MMSYSERR_NOTSUPPORTED)
}

/// A process ended: its mixers close.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    mixer(cpu).opens.retain(|o| o.pid != pid);
}

crate::state_fields!(Line { left, right, mute });
crate::state_fields!(Open {
    handle,
    pid,
    callback
});
crate::state_fields!(Mixer { lines, opens });
