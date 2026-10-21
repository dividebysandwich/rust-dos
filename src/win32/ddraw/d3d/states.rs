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

/// Make the change.
fn apply(cpu: &mut Cpu, id: u32, c: &Change) {
    let value = c.data.first().copied().unwrap_or(0);
    match c.kind {
        CHANGE_RENDER => render(cpu, id, c.a, value),
        CHANGE_STAGE => {
            if let Some(d) = get(cpu, id)
                && let Some(s) = d.stages.get_mut((c.a * 32 + c.b) as usize)
            {
                *s = value;
            }
        }
        CHANGE_TRANSFORM => {
            if let (Some(d), Ok(m)) = (get(cpu, id), <[u32; 16]>::try_from(&c.data[..]))
                && let Some(t) = d.transforms.get_mut(c.a as usize)
            {
                *t = m.map(f32::from_bits);
            }
        }
        CHANGE_VIEWPORT => {
            if let (Some(d), Ok(v)) = (get(cpu, id), <[u32; 6]>::try_from(&c.data[..])) {
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
            if let (Some(d), Ok(m)) = (get(cpu, id), <[u32; 17]>::try_from(&c.data[..])) {
                d.material = m;
            }
        }
        CHANGE_LIGHT => {
            if let (Some(d), Ok(l)) = (get(cpu, id), <[u32; 26]>::try_from(&c.data[..])) {
                light_slot(d, c.a).light = l;
            }
        }
        CHANGE_LIGHT_ENABLE => {
            if let Some(d) = get(cpu, id) {
                light_slot(d, c.a).enabled = c.b != 0;
            }
        }
        CHANGE_TEXTURE => set_stage_texture(cpu, id, c.a, value),
        CHANGE_PLANE => {
            if let (Some(d), Ok(p)) = (get(cpu, id), <[u32; 4]>::try_from(&c.data[..]))
                && let Some(plane) = d.planes.get_mut(c.a as usize)
            {
                *plane = p.map(f32::from_bits);
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

fn viewport_words(v: &Viewport7) -> [u32; 6] {
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
fn light_slot(d: &mut Device, index: u32) -> &mut LightSlot {
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
fn set_stage_texture(cpu: &mut Cpu, id: u32, stage: u32, iface: u32) {
    if iface != 0 && super::super::this(cpu, iface).is_none() {
        return;
    }
    let Some(d) = get(cpu, id) else {
        return;
    };
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
    if let Some(r) = d.render.get_mut(s as usize) {
        *r = value;
    }
    let stage0: Vec<(u32, u32)> = match s {
        RS_TEXTUREADDRESS => vec![
            (TSS_ADDRESS, value),
            (TSS_ADDRESSU, value),
            (TSS_ADDRESSV, value),
        ],
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
                // DECAL, COPY, DECALMASK.
                1 | 5 | 6 => (TOP_SELECTARG1, TOP_SELECTARG1),
                // DECALALPHA.
                3 => (TOP_BLENDTEXTUREALPHA, TOP_SELECTARG2),
                // MODULATEALPHA.
                4 => (TOP_MODULATE, TOP_MODULATE),
                // ADD.
                7 => (TOP_ADD, TOP_SELECTARG2),
                // MODULATE and MODULATEMASK: the texture's alpha.
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
        d.stages[kind as usize] = v;
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
    set(cpu, id, change(CHANGE_LIGHT, index, 0, l.to_vec()));
    Some(D3D_OK)
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
    let value = get(cpu, id).map_or(0, |d| d.stage(stage, t));
    cpu.bus.guest_write_32(out, value);
    Some(D3D_OK)
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
    set(cpu, id, change(CHANGE_STAGE, stage, t, vec![value]));
    Some(D3D_OK)
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
