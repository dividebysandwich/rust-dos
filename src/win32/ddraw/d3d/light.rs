//! Lights: IDirect3DLight, Direct3D 3 to 6's lights, which shine on
//! what a device draws when they are in its current viewport. Direct3D
//! 3's D3DLIGHT is always on; Direct3D 5's D3DLIGHT2 has a flag for it.

use super::super::{DDERR_ALREADYINITIALIZED, DDERR_INVALIDOBJECT, DDERR_INVALIDPARAMS};
use super::pipeline::{LIGHT_GLSPOT, LIGHT_SPOT, Light};
use super::{D3D_OK, probe, read_dwords, write_dwords};
use crate::cpu::Cpu;

/// A light.
#[derive(Clone, Debug, Default)]
pub struct LightObject {
    /// The version of IDirect3D it was made through.
    pub version: u32,
    pub set: bool,
    /// D3DLIGHT2's words after its size: the type, color, position,
    /// direction, range, falloff, attenuations, theta, phi and flags.
    pub data: [u32; 19],
    /// The viewport it is in.
    pub viewport: u32,
}

crate::state_fields!(LightObject {
    version,
    set,
    data,
    viewport
});

const D3DLIGHT_ACTIVE: u32 = 0x1;
const D3DLIGHT_NO_SPECULAR: u32 = 0x2;

/// The sizes of D3DLIGHT and D3DLIGHT2.
const SIZE1: u32 = 76;
const SIZE2: u32 = 80;

/// The light of the interface `this`: its id.
pub fn light_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    let (_, id) = super::of(cpu, this).ok_or(DDERR_INVALIDOBJECT)?;
    get(cpu, id).ok_or(DDERR_INVALIDOBJECT)?;
    Ok(id)
}

fn get(cpu: &mut Cpu, id: u32) -> Option<&mut LightObject> {
    super::object(cpu, id)?.light.as_mut()
}

/// The light `id` goes: its viewport, which held it, had let it go.
pub fn destroyed(_: &mut Cpu, _id: u32, _l: LightObject) {}

/// The lights that are on in the viewport `id`, as the pipeline has
/// them. A light's color is its diffuse and specular color.
pub fn lights_of(cpu: &mut Cpu, viewport: u32) -> Vec<Light> {
    let list = super::object(cpu, viewport)
        .and_then(|o| o.viewport.as_ref())
        .map(|v| v.lights.clone())
        .unwrap_or_default();
    let mut out = Vec::new();
    for (lid, _) in list {
        let Some(l) = get(cpu, lid).cloned() else {
            continue;
        };
        let w = l.data;
        if !l.set || w[18] & D3DLIGHT_ACTIVE == 0 {
            continue;
        }
        let f = |i: usize| f32::from_bits(w[i]);
        let color = [f(1), f(2), f(3), f(4)];
        out.push(Light {
            kind: if w[0] == LIGHT_GLSPOT {
                LIGHT_SPOT
            } else {
                w[0]
            },
            diffuse: color,
            specular: if w[18] & D3DLIGHT_NO_SPECULAR != 0 {
                [0.0; 4]
            } else {
                color
            },
            ambient: [0.0; 4],
            position: [f(5), f(6), f(7)],
            direction: [f(8), f(9), f(10)],
            range: f(11),
            falloff: f(12),
            attenuation: [f(13), f(14), f(15)],
            theta: f(16),
            phi: f(17),
        });
    }
    out
}

/// Initialize(this, d3d): it was.
pub fn initialize(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DDERR_ALREADYINITIALIZED)
}

/// SetLight(this, light): D3DLIGHT or D3DLIGHT2, of a type there is.
pub fn set_light(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(light_of(cpu, this));
    if !probe(cpu, at, 4, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let size = cpu.bus.guest_read_32(at);
    if !(size == SIZE1 || size == SIZE2) || !probe(cpu, at, size, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let mut data = read_dwords::<19>(cpu, at + 4);
    if size == SIZE1 {
        data[18] = D3DLIGHT_ACTIVE;
    }
    if !(1..=5).contains(&data[0]) {
        return Some(DDERR_INVALIDPARAMS);
    }
    if let Some(l) = get(cpu, id) {
        l.data = data;
        l.set = true;
    }
    Some(D3D_OK)
}

/// GetLight(this, light).
pub fn get_light(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(light_of(cpu, this));
    if !probe(cpu, at, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let size = cpu.bus.guest_read_32(at);
    if !(size == SIZE1 || size == SIZE2) || !probe(cpu, at, size, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let data = get(cpu, id).map_or([0; 19], |l| l.data);
    let n = (size as usize - 4) / 4;
    write_dwords(cpu, at + 4, &data[..n]);
    Some(D3D_OK)
}
