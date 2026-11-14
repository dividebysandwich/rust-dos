//! A device's state: render states, Direct3D 5's light states, the
//! texture stages and their textures, the transforms, and Direct3D 7's
//! viewport, material, lights, clip planes and state blocks.
//!
//! Each change goes through `set`, which makes it, or records it in the
//! state block being recorded (between BeginStateBlock and
//! EndStateBlock the device's state stays as it is). Direct3D 5's render
//! states for the texture (TEXTUREHANDLE, TEXTUREMAPBLEND, TEXTUREMAG
//! and the others) set the first texture stage, as Direct3D 6 did;
//! IDirect3DDevice7 has no TEXTUREHANDLE and TEXTUREMAPBLEND.

use super::super::{DDERR_INVALIDOBJECT, DDERR_INVALIDPARAMS, V_TEXTURE2};
use super::device::{
    self, Change, Device, LightSlot, RENDER_STATES, RS_AMBIENT, RS_COLORVERTEX, RS_FOGDENSITY,
    RS_FOGEND, RS_FOGSTART, RS_FOGVERTEXMODE, RS_TEXTUREADDRESS, RS_TEXTUREADDRESSU,
    RS_TEXTUREADDRESSV, RS_TEXTUREHANDLE, RS_TEXTUREMAG, RS_TEXTUREMAPBLEND, RS_TEXTUREMIN, STAGES,
    TA_DIFFUSE, TA_TEXTURE, TOP_ADD, TOP_BLENDTEXTUREALPHA, TOP_MODULATE, TOP_SELECTARG1,
    TOP_SELECTARG2, TSS_ADDRESS, TSS_ADDRESSU, TSS_ADDRESSV, TSS_ALPHAARG1, TSS_ALPHAARG2,
    TSS_ALPHAOP, TSS_BORDERCOLOR, TSS_COLORARG1, TSS_COLORARG2, TSS_COLOROP, TSS_LAST,
    TSS_MAGFILTER, TSS_MAXANISOTROPY, TSS_MINFILTER, TSS_MIPFILTER, TSS_MIPMAPLODBIAS,
    TSS_TEXCOORDINDEX, Viewport7, device_of, get, version,
};
use super::pipeline::multiply;
use super::{
    D3D_OK, D3DERR_INBEGINSTATEBLOCK, D3DERR_INVALIDSTATEBLOCK, D3DERR_NOTINBEGINSTATEBLOCK, probe,
    read_dwords, read_f32, read_matrix, write_dwords, write_matrix,
};
use crate::cpu::Cpu;
use crate::win32::com;

// What a change is.
pub const CHANGE_RENDER: u32 = 1;
pub const CHANGE_STAGE: u32 = 2;
pub const CHANGE_TRANSFORM: u32 = 3;
pub const CHANGE_VIEWPORT: u32 = 4;
pub const CHANGE_MATERIAL: u32 = 5;
pub const CHANGE_LIGHT: u32 = 6;
pub const CHANGE_LIGHT_ENABLE: u32 = 7;
pub const CHANGE_TEXTURE: u32 = 8;
pub const CHANGE_PLANE: u32 = 9;

fn change(kind: u32, a: u32, b: u32, data: Vec<u32>) -> Change {
    Change { kind, a, b, data }
}

/// Make the change, or record it if a state block is being recorded.
pub fn set(cpu: &mut Cpu, id: u32, c: Change) {
    let Some(d) = get(cpu, id) else {
        return;
    };
    if let Some(block) = d.recording.as_mut() {
        let key = |c: &Change| (c.kind, c.a, if c.kind == CHANGE_STAGE { c.b } else { 0 });
        block.retain(|o| key(o) != key(&c));
        block.push(c);
        return;
    }
    apply(cpu, id, &c);
}

/// Make the change. Programs set states to the values they have
/// already, many times a frame: a state is written only when the
/// value differs, since a write gives the device a new stamp and the
/// next draw then binds the card's state anew (`device::Device`).
fn apply(cpu: &mut Cpu, id: u32, c: &Change) {
    let value = c.data.first().copied().unwrap_or(0);
    match c.kind {
        CHANGE_RENDER => render(cpu, id, c.a, value),
        CHANGE_STAGE => {
            let at = (c.a * 32 + c.b) as usize;
            if let Some(d) = get(cpu, id)
                && d.stages.get(at).is_some_and(|s| *s != value)
            {
                d.stages[at] = value;
            }
        }
        CHANGE_TRANSFORM => {
            let at = c.a as usize;
            if let (Some(d), Ok(m)) = (get(cpu, id), <[u32; 16]>::try_from(&c.data[..]))
                && d.transforms.get(at).is_some_and(|t| t.map(f32::to_bits) != m)
            {
                d.transforms[at] = m.map(f32::from_bits);
            }
        }
        CHANGE_VIEWPORT => {
            if let (Some(d), Ok(v)) = (get(cpu, id), <[u32; 6]>::try_from(&c.data[..]))
                && viewport_words(&d.viewport) != v
            {
                d.viewport = Viewport7 {
                    x: v[0],
                    y: v[1],
                    width: v[2],
                    height: v[3],
                    min_z: f32::from_bits(v[4]),
                    max_z: f32::from_bits(v[5]),
                };
            }
        }
        CHANGE_MATERIAL => {
            if let (Some(d), Ok(m)) = (get(cpu, id), <[u32; 17]>::try_from(&c.data[..]))
                && d.material != m
            {
                d.material = m;
            }
        }
        CHANGE_LIGHT => {
            if let (Some(d), Ok(l)) = (get(cpu, id), <[u32; 26]>::try_from(&c.data[..]))
                && d.lights.iter().find(|s| s.index == c.a).is_none_or(|s| s.light != l)
            {
                light_slot(d, c.a).light = l;
            }
        }
        CHANGE_LIGHT_ENABLE => {
            let on = c.b != 0;
            if let Some(d) = get(cpu, id)
                && d.lights.iter().find(|s| s.index == c.a).is_none_or(|s| s.enabled != on)
            {
                light_slot(d, c.a).enabled = on;
            }
        }
        CHANGE_TEXTURE => set_stage_texture(cpu, id, c.a, value),
        CHANGE_PLANE => {
            let at = c.a as usize;
            if let (Some(d), Ok(p)) = (get(cpu, id), <[u32; 4]>::try_from(&c.data[..]))
                && d.planes.get(at).is_some_and(|plane| plane.map(f32::to_bits) != p)
            {
                d.planes[at] = p.map(f32::from_bits);
            }
        }
        _ => {}
    }
}

/// The change as it is on the device now.
fn current(d: &Device, c: &Change) -> Change {
    let data = match c.kind {
        CHANGE_RENDER => vec![d.state(c.a)],
        CHANGE_STAGE => vec![d.stage(c.a, c.b)],
        CHANGE_TRANSFORM => d
            .transforms
            .get(c.a as usize)
            .map(|m| m.map(f32::to_bits).to_vec())
            .unwrap_or_default(),
        CHANGE_VIEWPORT => viewport_words(&d.viewport).to_vec(),
        CHANGE_MATERIAL => d.material.to_vec(),
        CHANGE_LIGHT => d
            .lights
            .iter()
            .find(|l| l.index == c.a)
            .map(|l| l.light.to_vec())
            .unwrap_or_else(|| c.data.clone()),
        CHANGE_TEXTURE => vec![d.textures.get(c.a as usize).copied().unwrap_or(0)],
        CHANGE_PLANE => d
            .planes
            .get(c.a as usize)
            .map(|p| p.map(f32::to_bits).to_vec())
            .unwrap_or_default(),
        _ => c.data.clone(),
    };
    let b = match c.kind {
        CHANGE_LIGHT_ENABLE => d
            .lights
            .iter()
            .find(|l| l.index == c.a)
            .map_or(c.b, |l| l.enabled as u32),
        _ => c.b,
    };
    change(c.kind, c.a, b, data)
}

/// A viewport as the words a change has: the depths' to the last bit,
/// so that two viewports are the same when their words are.
pub fn viewport_words(v: &Viewport7) -> [u32; 6] {
    [
        v.x,
        v.y,
        v.width,
        v.height,
        v.min_z.to_bits(),
        v.max_z.to_bits(),
    ]
}

/// The light `index`, made as Direct3D 7's default light if there is
/// none: white and directional, shining along z.
pub fn light_slot(d: &mut Device, index: u32) -> &mut LightSlot {
    let i = match d.lights.iter().position(|l| l.index == index) {
        Some(i) => i,
        None => {
            let mut light = [0; 26];
            light[0] = super::pipeline::LIGHT_DIRECTIONAL;
            for w in &mut light[1..4] {
                *w = 1.0f32.to_bits();
            }
            light[18] = 1.0f32.to_bits();
            d.lights.push(LightSlot {
                index,
                light,
                enabled: false,
            });
            d.lights.len() - 1
        }
    };
    &mut d.lights[i]
}

/// Set the texture of a stage: the interface `iface` (0 for none), on
/// which the device holds a reference.
pub fn set_stage_texture(cpu: &mut Cpu, id: u32, stage: u32, iface: u32) {
    if iface != 0 && super::super::this(cpu, iface).is_none() {
        return;
    }
    let Some(d) = get(cpu, id) else {
        return;
    };
    // The texture the stage has already: nothing changes, the
    // references neither.
    if d.textures.get(stage as usize) == Some(&iface) {
        return;
    }
    let Some(slot) = d.textures.get_mut(stage as usize) else {
        return;
    };
    let old = std::mem::replace(slot, iface);
    if iface != 0 {
        super::super::add_ref(cpu, iface);
    }
    if old != 0 {
        super::super::release(cpu, old);
    }
}

/// Set a render state, and what it stands for.
fn render(cpu: &mut Cpu, id: u32, s: u32, value: u32) {
    let Some(d) = get(cpu, id) else {
        return;
    };
    if d.render.get(s as usize).is_some_and(|r| *r != value) {
        d.render[s as usize] = value;
    }
    let stage0: Vec<(u32, u32)> = match s {
        RS_TEXTUREADDRESS => ADDRESS_STATES.map(|t| (t, value)).to_vec(),
        RS_TEXTUREADDRESSU => vec![(TSS_ADDRESSU, value)],
        RS_TEXTUREADDRESSV => vec![(TSS_ADDRESSV, value)],
        // BORDERCOLOR, MIPMAPLODBIAS and ANISOTROPY.
        43 => vec![(TSS_BORDERCOLOR, value)],
        46 => vec![(TSS_MIPMAPLODBIAS, value)],
        49 => vec![(TSS_MAXANISOTROPY, value)],
        // D3DFILTER_NEAREST and LINEAR, as D3DTFG_POINT and LINEAR.
        RS_TEXTUREMAG => vec![(TSS_MAGFILTER, if value == 2 { 2 } else { 1 })],
        RS_TEXTUREMIN => {
            // D3DFILTER_ to D3DTFN_ and D3DTFP_: (min, mip).
            let (min, mip) = match value {
                2 => (2, 1),
                3 => (1, 2),
                4 => (2, 2),
                5 => (1, 3),
                6 => (2, 3),
                _ => (1, 1),
            };
            vec![(TSS_MINFILTER, min), (TSS_MIPFILTER, mip)]
        }
        RS_TEXTUREMAPBLEND => {
            // D3DTBLEND_ as the first stage's operations: the color's and
            // the alpha's, each with the texture and the diffuse color.
            let (color, alpha) = match value {
                // DECAL, DECALMASK, COPY.
                1 | 5 | 7 => (TOP_SELECTARG1, TOP_SELECTARG1),
                // DECALALPHA.
                3 => (TOP_BLENDTEXTUREALPHA, TOP_SELECTARG2),
                // MODULATEALPHA.
                4 => (TOP_MODULATE, TOP_MODULATE),
                // ADD.
                8 => (TOP_ADD, TOP_SELECTARG2),
                // MODULATE and MODULATEMASK: the texture's alpha, or the
                // diffuse alpha for a texture without one, which the HAL
                // picks when it draws (hal::bind).
                _ => (TOP_MODULATE, TOP_SELECTARG1),
            };

            vec![
                (TSS_COLOROP, color),
                (TSS_COLORARG1, TA_TEXTURE),
                (TSS_COLORARG2, TA_DIFFUSE),
                (TSS_ALPHAOP, alpha),
                (TSS_ALPHAARG1, TA_TEXTURE),
                (TSS_ALPHAARG2, TA_DIFFUSE),
            ]
        }
        _ => vec![],
    };
    for (kind, v) in stage0 {
        if d.stages[kind as usize] != v {
            d.stages[kind as usize] = v;
        }
    }
    if s == RS_TEXTUREHANDLE {
        let surface = d
            .handle(value, device::HANDLE_TEXTURE)
            .map_or(0, |h| h.object);
        let iface = if surface != 0 {
            super::super::iface(cpu, surface, V_TEXTURE2)
        } else {
            0
        };
        set_stage_texture(cpu, id, 0, iface);
    }
}

/// Whether `s` is a render state the device's version has.
fn render_state_valid(vtable: usize, s: u32) -> bool {
    if s == 0 || s as usize >= RENDER_STATES {
        return false;
    }
    !(version(vtable) == 7 && matches!(s, RS_TEXTUREHANDLE | RS_TEXTUREMAPBLEND))
}

/// GetRenderState(this, state, out).
pub fn get_render_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, s, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    if !render_state_valid(vtable, s) || !probe(cpu, out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let value = get(cpu, id).map_or(0, |d| d.state(s));
    cpu.bus.guest_write_32(out, value);
    Some(D3D_OK)
}

/// SetRenderState(this, state, value).
pub fn set_render_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, s, value] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    if !render_state_valid(vtable, s) {
        return Some(DDERR_INVALIDPARAMS);
    }
    set(cpu, id, change(CHANGE_RENDER, s, 0, vec![value]));
    Some(D3D_OK)
}

/// The render state a light state is from Direct3D 6 on, if it is one.
fn light_state_render(s: u32) -> Option<u32> {
    match s {
        2 => Some(RS_AMBIENT),
        4 => Some(RS_FOGVERTEXMODE),
        5 => Some(RS_FOGSTART),
        6 => Some(RS_FOGEND),
        7 => Some(RS_FOGDENSITY),
        8 => Some(RS_COLORVERTEX),
        _ => None,
    }
}

/// GetLightState(this, state, out).
pub fn get_light_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, s, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !(1..=8).contains(&s) || !probe(cpu, out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(d) = get(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let value = match light_state_render(s) {
        Some(r) => d.state(r),
        None => d.light_states[s as usize],
    };
    cpu.bus.guest_write_32(out, value);
    Some(D3D_OK)
}

/// SetLightState(this, state, value).
pub fn set_light_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, s, value] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !(1..=8).contains(&s) {
        return Some(DDERR_INVALIDPARAMS);
    }
    match light_state_render(s) {
        Some(r) => set(cpu, id, change(CHANGE_RENDER, r, 0, vec![value])),
        None => {
            if let Some(d) = get(cpu, id) {
                d.light_states[s as usize] = value;
            }
        }
    }
    Some(D3D_OK)
}

/// Whether `t` is a transform the device's version has: the world, view
/// and projection, and from Direct3D 7 on the other world matrices and
/// the textures'.
fn transform_valid(vtable: usize, t: u32) -> bool {
    match version(vtable) {
        7 => matches!(t, 1..=6 | 16..=23),
        _ => matches!(t, 1..=3),
    }
}

/// SetTransform(this, type, matrix).
pub fn set_transform(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, t, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    if !transform_valid(vtable, t) || !probe(cpu, at, 64, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let m = read_dwords::<16>(cpu, at);
    set(cpu, id, change(CHANGE_TRANSFORM, t, 0, m.to_vec()));
    Some(D3D_OK)
}

/// GetTransform(this, type, matrix).
pub fn get_transform(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, t, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    if !transform_valid(vtable, t) || !probe(cpu, at, 64, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let m = get(cpu, id).map_or(super::pipeline::IDENTITY, |d| d.transforms[t as usize]);
    write_matrix(cpu, at, &m);
    Some(D3D_OK)
}

/// MultiplyTransform(this, type, matrix): the transform is the matrix
/// times what it was.
pub fn multiply_transform(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, t, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    if !transform_valid(vtable, t) || !probe(cpu, at, 64, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let m = read_matrix(cpu, at);
    let old = get(cpu, id).map_or(super::pipeline::IDENTITY, |d| d.transforms[t as usize]);
    let new = multiply(&m, &old).map(f32::to_bits);
    set(cpu, id, change(CHANGE_TRANSFORM, t, 0, new.to_vec()));
    Some(D3D_OK)
}

/// SetViewport(this, viewport) of IDirect3DDevice7.
pub fn set_viewport(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !probe(cpu, at, 24, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let v = read_dwords::<6>(cpu, at);
    set(cpu, id, change(CHANGE_VIEWPORT, 0, 0, v.to_vec()));
    Some(D3D_OK)
}

/// GetViewport(this, viewport) of IDirect3DDevice7.
pub fn get_viewport(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !probe(cpu, at, 24, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let v = get(cpu, id).map_or([0; 6], |d| viewport_words(&d.viewport));
    write_dwords(cpu, at, &v);
    Some(D3D_OK)
}

/// SetMaterial(this, material) of IDirect3DDevice7.
pub fn set_material(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !probe(cpu, at, 68, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let m = read_dwords::<17>(cpu, at);
    set(cpu, id, change(CHANGE_MATERIAL, 0, 0, m.to_vec()));
    Some(D3D_OK)
}

/// GetMaterial(this, material) of IDirect3DDevice7.
pub fn get_material(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !probe(cpu, at, 68, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let m = get(cpu, id).map_or([0; 17], |d| d.material);
    write_dwords(cpu, at, &m);
    Some(D3D_OK)
}

/// SetLight(this, index, light) of IDirect3DDevice7: a point, spot or
/// directional light; one with a position takes no negative
/// attenuation.
pub fn set_light(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !probe(cpu, at, 104, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let l = read_dwords::<26>(cpu, at);
    if !(1..=3).contains(&l[0]) {
        return Some(DDERR_INVALIDPARAMS);
    }
    if l[0] != super::pipeline::LIGHT_DIRECTIONAL {
        let negative = (0..3).any(|i| read_f32(cpu, at + 4 * (21 + i)) < 0.0);
        if negative {
            return Some(DDERR_INVALIDPARAMS);
        }
    }
    if !light_is_valid(&l) {
        return Some(DDERR_INVALIDPARAMS);
    }
    set(cpu, id, change(CHANGE_LIGHT, index, 0, l.to_vec()));
    Some(D3D_OK)
}

/// Whether Direct3D 7's SetLight takes the D3DLIGHT7 `l`, as measured
/// on Windows: a point or spot light's range is 0 to the square root
/// of the largest float, a spot's angles are 0 to pi with theta no
/// more than phi, and a spot or a directional light points somewhere
/// (a direction whose length a float takes for 0, such as 1e-25,
/// doesn't). Direct3D 8 takes them all.
fn light_is_valid(l: &[u32; 26]) -> bool {
    let f = |i: usize| f32::from_bits(l[i]);
    let (kind, range, theta, phi) = (l[0], f(19), f(24), f(25));
    let pointing = f(16) * f(16) + f(17) * f(17) + f(18) * f(18) > 0.0;
    let ranged = (0.0..=f32::MAX.sqrt()).contains(&range);
    match kind {
        super::pipeline::LIGHT_DIRECTIONAL => pointing,
        super::pipeline::LIGHT_SPOT => {
            ranged && pointing && theta >= 0.0 && theta <= phi && phi <= std::f32::consts::PI
        }
        _ => ranged,
    }
}

/// GetLight(this, index, light).
pub fn get_light(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !probe(cpu, at, 104, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let light =
        get(cpu, id).and_then(|d| d.lights.iter().find(|l| l.index == index).map(|l| l.light));
    let Some(light) = light else {
        return Some(DDERR_INVALIDPARAMS);
    };
    write_dwords(cpu, at, &light);
    Some(D3D_OK)
}

/// LightEnable(this, index, enable): a light not set yet is the
/// default one.
pub fn light_enable(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, enable] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if let Some(d) = get(cpu, id)
        && d.recording.is_none()
    {
        light_slot(d, index);
    }
    set(
        cpu,
        id,
        change(CHANGE_LIGHT_ENABLE, index, (enable != 0) as u32, vec![]),
    );
    Some(D3D_OK)
}

/// GetLightEnable(this, index, out).
pub fn get_light_enable(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    let enabled = get(cpu, id).and_then(|d| {
        d.lights
            .iter()
            .find(|l| l.index == index)
            .map(|l| l.enabled)
    });
    let Some(enabled) = enabled else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if !com::write_out(cpu, out, enabled as u32) {
        return Some(DDERR_INVALIDPARAMS);
    }
    Some(D3D_OK)
}

/// SetClipPlane(this, index, plane): one of six.
pub fn set_clip_plane(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if index >= 6 || !probe(cpu, at, 16, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let p = read_dwords::<4>(cpu, at);
    set(cpu, id, change(CHANGE_PLANE, index, 0, p.to_vec()));
    Some(D3D_OK)
}

/// GetClipPlane(this, index, plane).
pub fn get_clip_plane(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if index >= 6 || !probe(cpu, at, 16, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let p = get(cpu, id).map_or([0.0; 4], |d| d.planes[index as usize]);
    write_dwords(cpu, at, &p.map(f32::to_bits));
    Some(D3D_OK)
}

/// SetClipStatus(this, status).
pub fn set_clip_status(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !probe(cpu, at, 32, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let s = read_dwords::<8>(cpu, at);
    if let Some(d) = get(cpu, id) {
        d.clip_status = s;
    }
    Some(D3D_OK)
}

/// GetClipStatus(this, status).
pub fn get_clip_status(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !probe(cpu, at, 32, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let s = get(cpu, id).map_or([0; 8], |d| d.clip_status);
    write_dwords(cpu, at, &s);
    Some(D3D_OK)
}

/// GetTexture(this, stage, out): the stage's texture, with a reference.
pub fn get_texture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, stage, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if stage >= STAGES || !probe(cpu, out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let iface = get(cpu, id).map_or(0, |d| d.textures[stage as usize]);
    if iface != 0 {
        super::super::add_ref(cpu, iface);
    }
    cpu.bus.guest_write_32(out, iface);
    Some(D3D_OK)
}

/// SetTexture(this, stage, texture): a surface's interface (a
/// texture's, for IDirect3DDevice3), or NULL.
pub fn set_texture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, stage, texture] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if stage >= STAGES {
        return Some(DDERR_INVALIDPARAMS);
    }
    if texture != 0 {
        let surface = super::super::this(cpu, texture)
            .is_some_and(|(_, s)| super::super::state(cpu).surface(s).is_some());
        if !surface {
            return Some(DDERR_INVALIDPARAMS);
        }
    }
    set(cpu, id, change(CHANGE_TEXTURE, stage, 0, vec![texture]));
    Some(D3D_OK)
}

/// GetTextureStageState(this, stage, type, out).
pub fn get_texture_stage_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, stage, t, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if stage >= STAGES || !(1..=TSS_LAST).contains(&t) || !probe(cpu, out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let value = get(cpu, id).map_or(0, |d| stage_state(d, stage, t));
    cpu.bus.guest_write_32(out, value);
    Some(D3D_OK)
}

/// The state `t` of a stage as GetTextureStageState gives it.
/// D3DTSS_ADDRESS reads back as ADDRESSU, as Wine's ddraw has it, so it
/// follows ADDRESSU set on its own.
fn stage_state(d: &Device, stage: u32, t: u32) -> u32 {
    let t = if t == TSS_ADDRESS { TSS_ADDRESSU } else { t };
    d.stage(stage, t)
}

/// SetTextureStageState(this, stage, type, value).
pub fn set_texture_stage_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, stage, t, value] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if stage >= STAGES || !(1..=TSS_LAST).contains(&t) {
        return Some(DDERR_INVALIDPARAMS);
    }
    set_stage(cpu, id, stage, t, value);
    Some(D3D_OK)
}

/// D3DTSS_ADDRESS and the states it sets: ADDRESSU and ADDRESSV, the
/// directions the samplers read.
const ADDRESS_STATES: [u32; 3] = [TSS_ADDRESS, TSS_ADDRESSU, TSS_ADDRESSV];

/// Set the state `t` of a stage, and for D3DTSS_ADDRESS both directions
/// with it. Each is a change of its own, made or recorded, so a state
/// block that has D3DTSS_ADDRESS captures and restores ADDRESSU and
/// ADDRESSV as the device has them, which the program may since have
/// set apart.
fn set_stage(cpu: &mut Cpu, id: u32, stage: u32, t: u32, value: u32) {
    let states = match t {
        TSS_ADDRESS => &ADDRESS_STATES[..],
        _ => std::slice::from_ref(&t),
    };
    for &s in states {
        set(cpu, id, change(CHANGE_STAGE, stage, s, vec![value]));
    }
}

// State blocks.

/// The render states Direct3D 7 has, and those of them that are about
/// vertices.
const RENDER_LIST: &[u32] = &[
    2, 4, 7, 8, 9, 10, 14, 15, 16, 19, 20, 22, 23, 24, 25, 26, 27, 28, 29, 30, 33, 34, 35, 36, 37,
    38, 40, 41, 47, 48, 52, 53, 54, 55, 56, 57, 58, 59, 60, 128, 129, 130, 131, 132, 133, 134, 135,
    136, 137, 138, 139, 140, 141, 142, 143, 145, 146, 147, 148, 151, 152,
];
const VERTEX_RENDER: &[u32] = &[
    9, 22, 28, 29, 34, 35, 36, 37, 38, 48, 136, 137, 139, 140, 141, 142, 143, 145, 146, 147, 148,
    151, 152,
];

/// The changes a new state block of `kind` (D3DSBT_ALL 1, PIXELSTATE
/// 2, VERTEXSTATE 3) is made of, with the device's state now.
fn block_of(d: &Device, kind: u32) -> Vec<Change> {
    let pixel = kind != 3;
    let vertex = kind != 2;
    let mut list = Vec::new();
    for &s in RENDER_LIST {
        let v = VERTEX_RENDER.contains(&s);
        if v && vertex || !v && pixel {
            list.push(change(CHANGE_RENDER, s, 0, vec![]));
        }
    }
    for stage in 0..STAGES {
        for t in 1..=TSS_LAST {
            let v = t == TSS_TEXCOORDINDEX || t == TSS_LAST;
            if v && vertex || !v && pixel {
                list.push(change(CHANGE_STAGE, stage, t, vec![]));
            }
        }
    }
    if kind == 1 {
        for stage in 0..STAGES {
            list.push(change(CHANGE_TEXTURE, stage, 0, vec![]));
        }
        for t in [1, 2, 3, 4, 5, 6, 16, 17, 18, 19, 20, 21, 22, 23] {
            list.push(change(CHANGE_TRANSFORM, t, 0, vec![]));
        }
        list.push(change(CHANGE_VIEWPORT, 0, 0, vec![]));
        list.push(change(CHANGE_MATERIAL, 0, 0, vec![]));
        for p in 0..6 {
            list.push(change(CHANGE_PLANE, p, 0, vec![]));
        }
    }
    if vertex {
        for l in &d.lights {
            list.push(change(CHANGE_LIGHT, l.index, 0, l.light.to_vec()));
            list.push(change(CHANGE_LIGHT_ENABLE, l.index, 0, vec![]));
        }
    }
    list.iter().map(|c| current(d, c)).collect()
}

/// The state block `h` of the device `id`: an error if it is being
/// recorded or there is none.
fn block(cpu: &mut Cpu, id: u32, h: u32) -> Result<Vec<Change>, u32> {
    let d = get(cpu, id).ok_or(DDERR_INVALIDOBJECT)?;
    if d.recording.is_some() {
        return Err(D3DERR_INBEGINSTATEBLOCK);
    }
    let i = h.checked_sub(1).ok_or(D3DERR_INVALIDSTATEBLOCK)? as usize;
    d.blocks
        .get(i)
        .cloned()
        .flatten()
        .ok_or(D3DERR_INVALIDSTATEBLOCK)
}

/// BeginStateBlock(this).
pub fn begin_state_block(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(device_of(cpu, args[0]));
    let Some(d) = get(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if d.recording.is_some() {
        return Some(D3DERR_INBEGINSTATEBLOCK);
    }
    d.recording = Some(Vec::new());
    Some(D3D_OK)
}

/// Keep a state block: its handle.
fn new_block(d: &mut Device, list: Vec<Change>) -> u32 {
    match d.blocks.iter().position(Option::is_none) {
        Some(i) => {
            d.blocks[i] = Some(list);
            i as u32 + 1
        }
        None => {
            d.blocks.push(Some(list));
            d.blocks.len() as u32
        }
    }
}

/// EndStateBlock(this, out).
pub fn end_state_block(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !probe(cpu, out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(d) = get(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let Some(list) = d.recording.take() else {
        return Some(D3DERR_NOTINBEGINSTATEBLOCK);
    };
    let h = new_block(d, list);
    cpu.bus.guest_write_32(out, h);
    Some(D3D_OK)
}

/// ApplyStateBlock(this, handle).
pub fn apply_state_block(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, h] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    let list = hr!(block(cpu, id, h));
    for c in &list {
        apply(cpu, id, c);
    }
    Some(D3D_OK)
}

/// CaptureStateBlock(this, handle): the block takes the device's state
/// now, for what it has.
pub fn capture_state_block(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, h] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    let list = hr!(block(cpu, id, h));
    let Some(d) = get(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let captured = list.iter().map(|c| current(d, c)).collect();
    d.blocks[h as usize - 1] = Some(captured);
    Some(D3D_OK)
}

/// DeleteStateBlock(this, handle).
pub fn delete_state_block(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, h] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    hr!(block(cpu, id, h));
    if let Some(d) = get(cpu, id) {
        d.blocks[h as usize - 1] = None;
    }
    Some(D3D_OK)
}

/// CreateStateBlock(this, type, out).
pub fn create_state_block(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !(1..=3).contains(&kind) || !probe(cpu, out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(d) = get(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if d.recording.is_some() {
        return Some(D3DERR_INBEGINSTATEBLOCK);
    }
    let list = block_of(d, kind);
    let h = new_block(d, list);
    cpu.bus.guest_write_32(out, h);
    Some(D3D_OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win32::ddraw::{Object, add_object};

    const WRAP: u32 = 1;
    const MIRROR: u32 = 2;
    const CLAMP: u32 = 3;
    const BORDER: u32 = 4;

    /// A Direct3D 7 device: its id.
    fn device(cpu: &mut Cpu) -> u32 {
        let d3d = super::super::D3dObject {
            device: Some(Device::new(7, 0, 0, 0, 2)),
            ..Default::default()
        };
        add_object(
            cpu,
            Object {
                d3d: Some(d3d),
                ..Default::default()
            },
        )
    }

    fn address(cpu: &mut Cpu, id: u32) -> (u32, u32) {
        let d = get(cpu, id).unwrap();
        (d.stage(1, TSS_ADDRESSU), d.stage(1, TSS_ADDRESSV))
    }

    #[test]
    fn a_block_with_the_address_state_keeps_each_direction() {
        // A block recorded with D3DTSS_ADDRESS sets both directions, and
        // when it captures the device's state it takes ADDRESSU and
        // ADDRESSV as they are, though the program set them apart.
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let id = device(&mut cpu);
        get(&mut cpu, id).unwrap().recording = Some(Vec::new());
        set_stage(&mut cpu, id, 1, TSS_ADDRESS, CLAMP);
        let block = get(&mut cpu, id).unwrap().recording.take().unwrap();
        assert_eq!(address(&mut cpu, id), (WRAP, WRAP));
        for c in &block {
            apply(&mut cpu, id, c);
        }
        assert_eq!(address(&mut cpu, id), (CLAMP, CLAMP));

        set_stage(&mut cpu, id, 1, TSS_ADDRESSU, MIRROR);
        set_stage(&mut cpu, id, 1, TSS_ADDRESSV, BORDER);
        let d = get(&mut cpu, id).unwrap();
        let captured: Vec<Change> = block.iter().map(|c| current(d, c)).collect();
        set_stage(&mut cpu, id, 1, TSS_ADDRESS, WRAP);
        for c in &captured {
            apply(&mut cpu, id, c);
        }
        assert_eq!(address(&mut cpu, id), (MIRROR, BORDER));
    }

    #[test]
    fn the_address_state_reads_back_as_addressu() {
        // GetTextureStageState(D3DTSS_ADDRESS) gives ADDRESSU, also after
        // ADDRESSU is set on its own.
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let id = device(&mut cpu);
        let read = |cpu: &mut Cpu| stage_state(get(cpu, id).unwrap(), 1, TSS_ADDRESS);
        set_stage(&mut cpu, id, 1, TSS_ADDRESS, CLAMP);
        assert_eq!(read(&mut cpu), CLAMP);
        set_stage(&mut cpu, id, 1, TSS_ADDRESSU, MIRROR);
        assert_eq!(read(&mut cpu), MIRROR);
    }

    /// What Direct3D 7's SetLight was measured to refuse, each beside
    /// the nearest light it takes.
    #[test]
    fn set_light_refuses_what_direct3d_7_refuses() {
        use super::super::pipeline::{LIGHT_DIRECTIONAL, LIGHT_SPOT};
        // Kind, direction, range, theta and phi of a D3DLIGHT7.
        let light = |kind: u32, direction: [f32; 3], range: f32, theta: f32, phi: f32| {
            let mut l = [0u32; 26];
            l[0] = kind;
            for (i, d) in direction.iter().enumerate() {
                l[16 + i] = d.to_bits();
            }
            (l[19], l[24], l[25]) = (range.to_bits(), theta.to_bits(), phi.to_bits());
            light_is_valid(&l)
        };
        let (point, z) = (1, [0.0, 0.0, 1.0]);
        let pi = std::f32::consts::PI;
        // A point light: its range alone counts.
        assert!(light(point, [0.0; 3], 0.0, 9.0, -9.0));
        assert!(light(point, z, f32::MAX.sqrt(), 0.0, 0.0));
        assert!(!light(point, z, -0.5, 0.0, 0.0));
        assert!(!light(point, z, 2.0e19, 0.0, 0.0));
        assert!(!light(point, z, f32::NAN, 0.0, 0.0));
        // A spot: its range, its direction and its angles.
        assert!(light(LIGHT_SPOT, z, 40.0, 0.25, 0.75));
        assert!(light(LIGHT_SPOT, [0.0, 0.0, 7.0], 40.0, 0.75, 0.75));
        assert!(light(LIGHT_SPOT, z, 40.0, pi, pi));
        assert!(light(LIGHT_SPOT, z, 40.0, 0.0, 0.0));
        assert!(!light(LIGHT_SPOT, z, 40.0, 0.75, 0.25));
        assert!(!light(LIGHT_SPOT, z, 40.0, -0.25, 0.75));
        assert!(!light(LIGHT_SPOT, z, 40.0, 0.25, -0.75));
        assert!(!light(LIGHT_SPOT, z, 40.0, 0.25, 3.25));
        assert!(!light(LIGHT_SPOT, z, -1.0, 0.25, 0.75));
        assert!(!light(LIGHT_SPOT, [0.0; 3], 40.0, 0.25, 0.75));
        assert!(!light(LIGHT_SPOT, [0.0, 2e-25, 0.0], 40.0, 0.25, 0.75));
        // A directional light: its direction alone.
        assert!(light(LIGHT_DIRECTIONAL, z, -3.0, 5.0, 1.0));
        assert!(!light(LIGHT_DIRECTIONAL, [0.0; 3], 10.0, 0.0, 0.0));
        assert!(!light(LIGHT_DIRECTIONAL, [3e-25, 0.0, 0.0], 10.0, 0.0, 0.0));
    }
}
