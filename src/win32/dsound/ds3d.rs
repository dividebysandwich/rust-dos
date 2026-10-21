//! IDirectSound3DBuffer and IDirectSound3DListener: where a 3D buffer's
//! sound is, and where the listener is, which make how loud it is and
//! from which side: its distance from the listener over its minimum
//! distance, to the power of the listener's rolloff factor, up to its
//! maximum distance, and a pan by which side of the listener it is on.
//! Cones, velocities and Doppler shift are kept for the program to read
//! and don't change the sound.
//!
//! Settings made with DS3D_DEFERRED wait for the listener's
//! CommitDeferredSettings.

use super::buffer::{DSBCAPS_MUTE3DATMAXDISTANCE, index_of};
use super::*;

/// A listener: DS3DLISTENER's fields.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub front: [f32; 3],
    pub top: [f32; 3],
    pub distance_factor: f32,
    pub rolloff_factor: f32,
    pub doppler_factor: f32,
}

impl Default for Listener {
    fn default() -> Listener {
        Listener {
            position: [0.0; 3],
            velocity: [0.0; 3],
            front: [0.0, 0.0, 1.0],
            top: [0.0, 1.0, 0.0],
            distance_factor: 1.0,
            rolloff_factor: 1.0,
            doppler_factor: 1.0,
        }
    }
}

/// A 3D buffer's settings: DS3DBUFFER's fields.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub inside_cone: u32,
    pub outside_cone: u32,
    pub orientation: [f32; 3],
    pub outside_volume: i32,
    pub min_distance: f32,
    pub max_distance: f32,
    pub mode: u32,
}

impl Default for Params {
    fn default() -> Params {
        Params {
            position: [0.0; 3],
            velocity: [0.0; 3],
            inside_cone: 360,
            outside_cone: 360,
            orientation: [0.0, 0.0, 1.0],
            outside_volume: 0,
            min_distance: 1.0,
            max_distance: 1e9,
            mode: DS3DMODE_NORMAL,
        }
    }
}

const DS3DMODE_NORMAL: u32 = 0;
const DS3DMODE_HEADRELATIVE: u32 = 1;
const DS3DMODE_DISABLE: u32 = 2;
const DS3D_DEFERRED: u32 = 1;

/// The bytes of DS3DBUFFER and DS3DLISTENER.
const STRUCT_SIZE: u32 = 64;

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The gains, left and right, of a 3D buffer's sound with `params` for
/// `listener`.
pub fn gains(params: &Params, listener: &Listener, flags: u32) -> (f32, f32) {
    if params.mode == DS3DMODE_DISABLE {
        return (1.0, 1.0);
    }
    let (relative, right) = if params.mode == DS3DMODE_HEADRELATIVE {
        (params.position, [1.0, 0.0, 0.0])
    } else {
        let right = cross(listener.top, listener.front);
        (sub(params.position, listener.position), right)
    };
    let distance = dot(relative, relative).sqrt();
    let (min, max) = (params.min_distance.max(f32::MIN_POSITIVE), params.max_distance);
    if flags & DSBCAPS_MUTE3DATMAXDISTANCE != 0 && distance >= max {
        return (0.0, 0.0);
    }
    let gain = if distance <= min {
        1.0
    } else {
        (min / distance.min(max)).powf(listener.rolloff_factor)
    };
    let length = dot(right, right).sqrt();
    let side = if distance > 0.0 && length > 0.0 {
        (dot(relative, right) / (distance * length)).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    // A sound on the right is softer on the left, and the other way.
    let left = 1.0 - 0.6 * side.max(0.0);
    let right = 1.0 + 0.6 * side.min(0.0);
    (gain * left, gain * right)
}

/// The buffer `this` is a 3D interface of.
fn buffer(cpu: &mut Cpu, this: u32) -> Option<usize> {
    index_of(cpu, this)
}

/// The DirectSound object whose listener `this` is.
fn sound_of(cpu: &mut Cpu, this: u32) -> Option<usize> {
    let i = index_of(cpu, this)?;
    let ds = state(cpu);
    let sound = ds.buffers[i].sound;
    ds.sound(sound)
}

fn read_vector(cpu: &mut Cpu, at: u32) -> [f32; 3] {
    [0, 4, 8].map(|o| f32::from_bits(cpu.bus.guest_read_32(at + o)))
}

fn write_vector(cpu: &mut Cpu, at: u32, v: [f32; 3]) {
    for (n, value) in v.into_iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * n as u32, value.to_bits());
    }
}

fn vector_arg(args: &[u32]) -> [f32; 3] {
    [args[0], args[1], args[2]].map(f32::from_bits)
}

/// Write `value` at the out pointer `out`: DS_OK, or DSERR_INVALIDPARAM
/// if it can't be written there.
fn out(cpu: &mut Cpu, out: u32, value: u32) -> Option<u32> {
    Some(if com::write_out(cpu, out, value) {
        DS_OK
    } else {
        DSERR_INVALIDPARAM
    })
}

fn out_vector(cpu: &mut Cpu, out: u32, v: [f32; 3]) -> Option<u32> {
    if out == 0 || !cpu.bus.guest_probe(out, 12, true) {
        return Some(DSERR_INVALIDPARAM);
    }
    write_vector(cpu, out, v);
    Some(DS_OK)
}

/// A buffer's settings as they are now (deferred ones not yet).
fn params(cpu: &mut Cpu, this: u32) -> Option<Params> {
    let i = buffer(cpu, this)?;
    Some(state(cpu).buffers[i].params)
}

/// Change a buffer's settings, now or when committed, as `apply` says.
fn change(cpu: &mut Cpu, this: u32, apply: u32, f: impl FnOnce(&mut Params)) -> Option<u32> {
    let Some(i) = buffer(cpu, this) else {
        return Some(DSERR_INVALIDPARAM);
    };
    device::catch_up(cpu);
    let b = &mut state(cpu).buffers[i];
    if apply & DS3D_DEFERRED != 0 {
        let mut p = b.deferred.unwrap_or(b.params);
        f(&mut p);
        b.deferred = Some(p);
    } else {
        f(&mut b.params);
    }
    Some(DS_OK)
}

/// The listener as it is now.
fn listener(cpu: &mut Cpu, this: u32) -> Option<Listener> {
    let s = sound_of(cpu, this)?;
    Some(state(cpu).sounds[s].listener)
}

fn change_listener(
    cpu: &mut Cpu,
    this: u32,
    apply: u32,
    f: impl FnOnce(&mut Listener),
) -> Option<u32> {
    let Some(s) = sound_of(cpu, this) else {
        return Some(DSERR_INVALIDPARAM);
    };
    device::catch_up(cpu);
    let sound = &mut state(cpu).sounds[s];
    if apply & DS3D_DEFERRED != 0 {
        let mut l = sound.deferred.unwrap_or(sound.listener);
        f(&mut l);
        sound.deferred = Some(l);
    } else {
        f(&mut sound.listener);
    }
    Some(DS_OK)
}

/// IDirectSound3DBuffer::GetAllParameters(this, DS3DBUFFER).
pub fn buffer_get_all(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[1];
    if at == 0 || !cpu.bus.guest_probe(at, STRUCT_SIZE as usize, true) {
        return Some(DSERR_INVALIDPARAM);
    }
    if cpu.bus.guest_read_32(at) != STRUCT_SIZE {
        return Some(DSERR_INVALIDPARAM);
    }
    let Some(p) = params(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    write_vector(cpu, at + 4, p.position);
    write_vector(cpu, at + 16, p.velocity);
    cpu.bus.guest_write_32(at + 28, p.inside_cone);
    cpu.bus.guest_write_32(at + 32, p.outside_cone);
    write_vector(cpu, at + 36, p.orientation);
    cpu.bus.guest_write_32(at + 48, p.outside_volume as u32);
    cpu.bus.guest_write_32(at + 52, p.min_distance.to_bits());
    cpu.bus.guest_write_32(at + 56, p.max_distance.to_bits());
    cpu.bus.guest_write_32(at + 60, p.mode);
    Some(DS_OK)
}

/// IDirectSound3DBuffer::SetAllParameters(this, DS3DBUFFER, apply).
pub fn buffer_set_all(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[1];
    if at == 0 || !cpu.bus.guest_probe(at, STRUCT_SIZE as usize, false) {
        return Some(DSERR_INVALIDPARAM);
    }
    if cpu.bus.guest_read_32(at) != STRUCT_SIZE {
        return Some(DSERR_INVALIDPARAM);
    }
    let p = Params {
        position: read_vector(cpu, at + 4),
        velocity: read_vector(cpu, at + 16),
        inside_cone: cpu.bus.guest_read_32(at + 28),
        outside_cone: cpu.bus.guest_read_32(at + 32),
        orientation: read_vector(cpu, at + 36),
        outside_volume: cpu.bus.guest_read_32(at + 48) as i32,
        min_distance: f32::from_bits(cpu.bus.guest_read_32(at + 52)),
        max_distance: f32::from_bits(cpu.bus.guest_read_32(at + 56)),
        mode: cpu.bus.guest_read_32(at + 60),
    };
    if p.mode > DS3DMODE_DISABLE {
        return Some(DSERR_INVALIDPARAM);
    }
    change(cpu, args[0], args[2], |q| *q = p)
}

pub fn get_cone_angles(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(p) = params(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    if args[1] == 0 || args[2] == 0 {
        return Some(DSERR_INVALIDPARAM);
    }
    if out(cpu, args[1], p.inside_cone) != Some(DS_OK) {
        return Some(DSERR_INVALIDPARAM);
    }
    out(cpu, args[2], p.outside_cone)
}

pub fn set_cone_angles(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (inside, outside) = (args[1], args[2]);
    if inside > 360 || outside > 360 || inside > outside {
        return Some(DSERR_INVALIDPARAM);
    }
    change(cpu, args[0], args[3], |p| {
        p.inside_cone = inside;
        p.outside_cone = outside;
    })
}

pub fn get_cone_orientation(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(p) = params(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    out_vector(cpu, args[1], p.orientation)
}

pub fn set_cone_orientation(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = vector_arg(&args[1..]);
    change(cpu, args[0], args[4], |p| p.orientation = v)
}

pub fn get_cone_outside_volume(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(p) = params(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    out(cpu, args[1], p.outside_volume as u32)
}

pub fn set_cone_outside_volume(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let volume = args[1] as i32;
    if !(-10000..=0).contains(&volume) {
        return Some(DSERR_INVALIDPARAM);
    }
    change(cpu, args[0], args[2], |p| p.outside_volume = volume)
}

pub fn get_max_distance(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(p) = params(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    out(cpu, args[1], p.max_distance.to_bits())
}

pub fn set_max_distance(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let d = f32::from_bits(args[1]);
    if d.is_nan() || d <= 0.0 {
        return Some(DSERR_INVALIDPARAM);
    }
    change(cpu, args[0], args[2], |p| p.max_distance = d)
}

pub fn get_min_distance(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(p) = params(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    out(cpu, args[1], p.min_distance.to_bits())
}

pub fn set_min_distance(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let d = f32::from_bits(args[1]);
    if d.is_nan() || d <= 0.0 {
        return Some(DSERR_INVALIDPARAM);
    }
    change(cpu, args[0], args[2], |p| p.min_distance = d)
}

pub fn get_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(p) = params(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    out(cpu, args[1], p.mode)
}

pub fn set_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mode = args[1];
    if mode > DS3DMODE_DISABLE {
        return Some(DSERR_INVALIDPARAM);
    }
    change(cpu, args[0], args[2], |p| p.mode = mode)
}

pub fn buffer_get_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(p) = params(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    out_vector(cpu, args[1], p.position)
}

pub fn buffer_set_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = vector_arg(&args[1..]);
    change(cpu, args[0], args[4], |p| p.position = v)
}

pub fn buffer_get_velocity(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(p) = params(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    out_vector(cpu, args[1], p.velocity)
}

pub fn buffer_set_velocity(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = vector_arg(&args[1..]);
    change(cpu, args[0], args[4], |p| p.velocity = v)
}

/// IDirectSound3DListener::GetAllParameters(this, DS3DLISTENER).
pub fn listener_get_all(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[1];
    if at == 0 || !cpu.bus.guest_probe(at, STRUCT_SIZE as usize, true) {
        return Some(DSERR_INVALIDPARAM);
    }
    if cpu.bus.guest_read_32(at) != STRUCT_SIZE {
        return Some(DSERR_INVALIDPARAM);
    }
    let Some(l) = listener(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    write_vector(cpu, at + 4, l.position);
    write_vector(cpu, at + 16, l.velocity);
    write_vector(cpu, at + 28, l.front);
    write_vector(cpu, at + 40, l.top);
    cpu.bus.guest_write_32(at + 52, l.distance_factor.to_bits());
    cpu.bus.guest_write_32(at + 56, l.rolloff_factor.to_bits());
    cpu.bus.guest_write_32(at + 60, l.doppler_factor.to_bits());
    Some(DS_OK)
}

/// IDirectSound3DListener::SetAllParameters(this, DS3DLISTENER, apply).
pub fn listener_set_all(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[1];
    if at == 0 || !cpu.bus.guest_probe(at, STRUCT_SIZE as usize, false) {
        return Some(DSERR_INVALIDPARAM);
    }
    if cpu.bus.guest_read_32(at) != STRUCT_SIZE {
        return Some(DSERR_INVALIDPARAM);
    }
    let l = Listener {
        position: read_vector(cpu, at + 4),
        velocity: read_vector(cpu, at + 16),
        front: read_vector(cpu, at + 28),
        top: read_vector(cpu, at + 40),
        distance_factor: f32::from_bits(cpu.bus.guest_read_32(at + 52)),
        rolloff_factor: f32::from_bits(cpu.bus.guest_read_32(at + 56)),
        doppler_factor: f32::from_bits(cpu.bus.guest_read_32(at + 60)),
    };
    change_listener(cpu, args[0], args[2], |m| *m = l)
}

pub fn get_distance_factor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(l) = listener(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    out(cpu, args[1], l.distance_factor.to_bits())
}

pub fn set_distance_factor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let f = f32::from_bits(args[1]);
    if f.is_nan() || f <= 0.0 {
        return Some(DSERR_INVALIDPARAM);
    }
    change_listener(cpu, args[0], args[2], |l| l.distance_factor = f)
}

pub fn get_doppler_factor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(l) = listener(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    out(cpu, args[1], l.doppler_factor.to_bits())
}

pub fn set_doppler_factor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let f = f32::from_bits(args[1]);
    if !(0.0..=10.0).contains(&f) {
        return Some(DSERR_INVALIDPARAM);
    }
    change_listener(cpu, args[0], args[2], |l| l.doppler_factor = f)
}

pub fn get_rolloff_factor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(l) = listener(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    out(cpu, args[1], l.rolloff_factor.to_bits())
}

pub fn set_rolloff_factor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let f = f32::from_bits(args[1]);
    if !(0.0..=10.0).contains(&f) {
        return Some(DSERR_INVALIDPARAM);
    }
    change_listener(cpu, args[0], args[2], |l| l.rolloff_factor = f)
}

pub fn get_orientation(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(l) = listener(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    if out_vector(cpu, args[1], l.front) != Some(DS_OK) {
        return Some(DSERR_INVALIDPARAM);
    }
    out_vector(cpu, args[2], l.top)
}

pub fn set_orientation(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (front, top) = (vector_arg(&args[1..]), vector_arg(&args[4..]));
    change_listener(cpu, args[0], args[7], |l| {
        l.front = front;
        l.top = top;
    })
}

pub fn listener_get_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(l) = listener(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    out_vector(cpu, args[1], l.position)
}

pub fn listener_set_position(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = vector_arg(&args[1..]);
    change_listener(cpu, args[0], args[4], |l| l.position = v)
}

pub fn listener_get_velocity(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(l) = listener(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    out_vector(cpu, args[1], l.velocity)
}

pub fn listener_set_velocity(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let v = vector_arg(&args[1..]);
    change_listener(cpu, args[0], args[4], |l| l.velocity = v)
}

/// IDirectSound3DListener::CommitDeferredSettings(this): the deferred
/// settings of the listener and of its object's buffers take effect.
pub fn commit(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(s) = sound_of(cpu, args[0]) else {
        return Some(DSERR_INVALIDPARAM);
    };
    device::catch_up(cpu);
    let ds = state(cpu);
    let sound = &mut ds.sounds[s];
    if let Some(l) = sound.deferred.take() {
        sound.listener = l;
    }
    let id = sound.id;
    for b in ds.buffers.iter_mut().filter(|b| b.sound == id) {
        if let Some(p) = b.deferred.take() {
            b.params = p;
        }
    }
    Some(DS_OK)
}

crate::state_fields!(Listener {
    position,
    velocity,
    front,
    top,
    distance_factor,
    rolloff_factor,
    doppler_factor
});
crate::state_fields!(Params {
    position,
    velocity,
    inside_cone,
    outside_cone,
    orientation,
    outside_volume,
    min_distance,
    max_distance,
    mode
});
