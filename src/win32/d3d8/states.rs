//! A device's state: render states, texture stages and their textures,
//! transforms, the viewport, the material, lights and clip planes (on
//! the inner device, which draws with them), and the streams, the index
//! buffer, the shaders and their constants (the device's own), with the
//! state blocks that record and set them.
//!
//! Render states have the numbers Direct3D 7 has, and go to the inner
//! device as they are. The texture stages' states are kept by Direct3D
//! 8's numbers and go to the inner device with its filters' numbers:
//! D3DTEXF_NONE, POINT and LINEAR as D3DTFP_NONE, POINT and LINEAR for
//! the mipmap filter. The world matrices past the fourth have no effect:
//! vertex blending is left out.

use super::super::ddraw::d3d::device::{Change, Viewport7};
use super::super::ddraw::d3d::pipeline::{IDENTITY, multiply};
use super::super::ddraw::d3d::states as inner_states;
use super::device::{
    self, Device, PS_CONSTANTS, STAGES, STREAMS, VS_CONSTANTS, get, inner, this_device,
};
use super::resource::{FMT_INDEX16, FMT_INDEX32};
use super::{D3D_OK, D3DERR_INVALIDCALL, V_INDEXBUFFER8, V_TEXTURE8, V_VERTEXBUFFER8, of, state};
use crate::cpu::Cpu;

// What a change is.
const RENDER: u32 = 1;
const STAGE: u32 = 2;
const TRANSFORM: u32 = 3;
const VIEWPORT: u32 = 4;
const MATERIAL: u32 = 5;
const LIGHT: u32 = 6;
const LIGHT_ENABLE: u32 = 7;
const TEXTURE: u32 = 8;
const PLANE: u32 = 9;
const STREAM: u32 = 10;
const INDICES: u32 = 11;
const VSHADER: u32 = 12;
const PSHADER: u32 = 13;
const VCONST: u32 = 14;
const PCONST: u32 = 15;

fn change(kind: u32, a: u32, b: u32, data: Vec<u32>) -> Change {
    Change { kind, a, b, data }
}

/// Make the change, or record it if a state block is being recorded.
fn set(cpu: &mut Cpu, id: u32, c: Change) {
    let Some(d) = get(cpu, id) else {
        return;
    };
    if let Some(block) = d.recording.as_mut() {
        let key = |c: &Change| (c.kind, c.a, if c.kind == STAGE { c.b } else { 0 });
        block.retain(|o| key(o) != key(&c));
        block.push(c);
        return;
    }
    apply(cpu, id, &c);
}

/// The inner device's place of a D3DTRANSFORMSTATETYPE, if it has one.
fn inner_transform(t: u32) -> Option<usize> {
    match t {
        2 | 3 | 16..=23 => Some(t as usize),
        256 => Some(1),
        257..=259 => Some((t - 253) as usize),
        _ => None,
    }
}

fn valid_transform(t: u32) -> bool {
    matches!(t, 2 | 3 | 16..=23 | 256..=511)
}

/// A texture stage's state as the inner device has it.
fn inner_stage(kind: u32, value: u32) -> u32 {
    match kind {
        // D3DTSS_MIPFILTER.
        18 => value + 1,
        _ => value,
    }
}

/// The interface of the root surface of the texture `tex`, for the
/// inner device.
fn texture_root(cpu: &mut Cpu, tex: u32) -> u32 {
    state(cpu)
        .object(tex)
        .and_then(|o| o.texture.as_ref())
        .map_or(0, |t| t.dd_iface)
}

/// Whether `id` is an object of the kind `vtable` (or 0).
fn is_kind(cpu: &mut Cpu, id: u32, vtable: usize) -> bool {
    id == 0
        || state(cpu)
            .object(id)
            .is_some_and(|o| o.vtable as usize == vtable)
}

/// Make the change.
fn apply(cpu: &mut Cpu, id: u32, c: &Change) {
    let value = c.data.first().copied().unwrap_or(0);
    let words = |n: usize| (c.data.len() == n).then_some(&c.data[..]);
    match c.kind {
        RENDER => {
            if let Some(i) = inner(cpu, id)
                && let Some(r) = i.render.get_mut(c.a as usize)
            {
                *r = value;
            }
        }
        STAGE => {
            let at = (c.a * 32 + c.b) as usize;
            if let Some(d) = get(cpu, id)
                && let Some(s) = d.stages.get_mut(at)
            {
                *s = value;
            }
            if let Some(i) = inner(cpu, id)
                && let Some(s) = i.stages.get_mut(at)
            {
                *s = inner_stage(c.b, value);
            }
        }
        TRANSFORM => {
            let Some(m) = words(16) else {
                return;
            };
            let m: [u32; 16] = m.try_into().unwrap_or([0; 16]);
            if let Some(t) = inner_transform(c.a) {
                if let Some(i) = inner(cpu, id) {
                    i.transforms[t] = m.map(f32::from_bits);
                }
            } else if let Some(d) = get(cpu, id) {
                // Only the matrices that aren't the identity are kept.
                d.worlds.retain(|w| w.0 != c.a);
                if m != IDENTITY.map(f32::to_bits) {
                    d.worlds.push((c.a, m));
                }
            }
        }
        VIEWPORT => {
            if let (Some(v), Some(i)) = (words(6).map(|v| v.to_vec()), inner(cpu, id)) {
                i.viewport = Viewport7 {
                    x: v[0],
                    y: v[1],
                    width: v[2],
                    height: v[3],
                    min_z: f32::from_bits(v[4]),
                    max_z: f32::from_bits(v[5]),
                };
            }
        }
        MATERIAL => {
            if let (Some(m), Some(i)) = (words(17).map(|v| v.to_vec()), inner(cpu, id)) {
                i.material.copy_from_slice(&m);
            }
        }
        LIGHT => {
            if let (Some(l), Some(i)) = (words(26).map(|v| v.to_vec()), inner(cpu, id)) {
                inner_states::light_slot(i, c.a).light.copy_from_slice(&l);
            }
        }
        LIGHT_ENABLE => {
            if let Some(i) = inner(cpu, id) {
                inner_states::light_slot(i, c.a).enabled = c.b != 0;
            }
        }
        TEXTURE => set_texture_now(cpu, id, c.a, value),
        PLANE => {
            if let (Some(p), Some(i)) = (words(4).map(|v| v.to_vec()), inner(cpu, id))
                && let Some(plane) = i.planes.get_mut(c.a as usize)
            {
                *plane = std::array::from_fn(|k| f32::from_bits(p[k]));
            }
        }
        STREAM => {
            let stride = c.data.get(1).copied().unwrap_or(0);
            if !is_kind(cpu, value, V_VERTEXBUFFER8) {
                return;
            }
            let Some(d) = get(cpu, id) else {
                return;
            };
            let Some(s) = d.streams.get_mut(c.a as usize) else {
                return;
            };
            let old = s.0;
            *s = (value, if value != 0 { stride } else { s.1 });
            if old != 0 && old != value {
                super::let_go(cpu, old);
            }
        }
        INDICES => {
            let base = c.data.get(1).copied().unwrap_or(0);
            if !is_kind(cpu, value, V_INDEXBUFFER8) {
                return;
            }
            let Some(d) = get(cpu, id) else {
                return;
            };
            let old = d.indices.0;
            d.indices = (value, base);
            if old != 0 && old != value {
                super::let_go(cpu, old);
            }
        }
        VSHADER => {
            if let Some(d) = get(cpu, id) {
                d.vertex_shader = value;
                super::shader::load_constants(d, value);
            }
        }
        PSHADER => {
            if let Some(d) = get(cpu, id) {
                d.pixel_shader = value;
            }
        }
        VCONST | PCONST => {
            let Some(v) = words(4) else {
                return;
            };
            let v: [u32; 4] = v.try_into().unwrap_or([0; 4]);
            if let Some(d) = get(cpu, id) {
                let list = if c.kind == VCONST {
                    &mut d.constants
                } else {
                    &mut d.ps_constants
                };
                if let Some(r) = list.get_mut(c.a as usize) {
                    *r = v;
                }
            }
        }
        _ => {}
    }
}

/// Set the texture of a stage: the texture `tex` of Direct3D 8's, or 0.
fn set_texture_now(cpu: &mut Cpu, id: u32, stage: u32, tex: u32) {
    if !is_kind(cpu, tex, V_TEXTURE8) {
        return;
    }
    let Some(d) = get(cpu, id) else {
        return;
    };
    let inner_id = d.inner;
    let Some(slot) = d.textures.get_mut(stage as usize) else {
        return;
    };
    let old = std::mem::replace(slot, tex);
    let root = texture_root(cpu, tex);
    inner_states::set_stage_texture(cpu, inner_id, stage, root);
    if old != 0 && old != tex {
        super::let_go(cpu, old);
    }
}

/// The change as it is on the device now.
fn current(cpu: &mut Cpu, id: u32, c: &Change) -> Change {
    let d = get(cpu, id).cloned().unwrap_or_default();
    let i = inner(cpu, id).cloned().unwrap_or_default();
    let mut b = c.b;
    let data = match c.kind {
        RENDER => vec![i.state(c.a)],
        STAGE => vec![
            d.stages
                .get((c.a * 32 + c.b) as usize)
                .copied()
                .unwrap_or(0),
        ],
        TRANSFORM => transform_words(&d, &i, c.a).to_vec(),
        VIEWPORT => {
            let v = i.viewport;
            vec![
                v.x,
                v.y,
                v.width,
                v.height,
                v.min_z.to_bits(),
                v.max_z.to_bits(),
            ]
        }
        MATERIAL => i.material.to_vec(),
        LIGHT => i
            .lights
            .iter()
            .find(|l| l.index == c.a)
            .map_or_else(|| c.data.clone(), |l| l.light.to_vec()),
        LIGHT_ENABLE => {
            if let Some(l) = i.lights.iter().find(|l| l.index == c.a) {
                b = l.enabled as u32;
            }
            Vec::new()
        }
        TEXTURE => vec![d.textures.get(c.a as usize).copied().unwrap_or(0)],
        PLANE => i
            .planes
            .get(c.a as usize)
            .map(|p| p.map(f32::to_bits).to_vec())
            .unwrap_or_default(),
        STREAM => {
            let s = d.streams.get(c.a as usize).copied().unwrap_or_default();
            vec![s.0, s.1]
        }
        INDICES => vec![d.indices.0, d.indices.1],
        VSHADER => vec![d.vertex_shader],
        PSHADER => vec![d.pixel_shader],
        VCONST => d
            .constants
            .get(c.a as usize)
            .map_or_else(|| c.data.clone(), |v| v.to_vec()),
        PCONST => d
            .ps_constants
            .get(c.a as usize)
            .map_or_else(|| c.data.clone(), |v| v.to_vec()),
        _ => c.data.clone(),
    };
    change(c.kind, c.a, b, data)
}

/// The matrix of the transform `t`.
fn transform_words(d: &Device, i: &super::super::ddraw::d3d::device::Device, t: u32) -> [u32; 16] {
    if let Some(at) = inner_transform(t) {
        return i
            .transforms
            .get(at)
            .copied()
            .unwrap_or(IDENTITY)
            .map(f32::to_bits);
    }
    d.worlds
        .iter()
        .find(|w| w.0 == t)
        .map_or(IDENTITY.map(f32::to_bits), |w| w.1)
}

// The state a device starts with.

/// The render states Direct3D 8 has.
fn render_state_valid(s: u32) -> bool {
    matches!(
        s,
        7..=10
            | 14..=16
            | 19..=20
            | 22..=30
            | 34..=38
            | 40
            | 47..=48
            | 52..=60
            | 128..=137
            | 139..=143
            | 145..=148
            | 151..=168
            | 170..=173
    )
}

/// The render states a device starts with (those not 0).
fn render_defaults(depth: bool) -> Vec<(u32, u32)> {
    let one = 1.0f32.to_bits();
    vec![
        (7, depth as u32),
        (8, 3),
        (9, 2),
        (14, 1),
        (16, 1),
        (19, 2),
        (20, 1),
        (22, 3),
        (23, 4),
        (25, 8),
        (37, one),
        (38, one),
        (53, 1),
        (54, 1),
        (55, 1),
        (56, 8),
        (58, 0xFFFF_FFFF),
        (59, 0xFFFF_FFFF),
        (60, 0xFFFF_FFFF),
        (136, 1),
        (137, 1),
        (141, 1),
        (142, 1),
        (145, 1),
        (146, 2),
        (154, one),
        (158, one),
        (161, 1),
        (162, 0xFFFF_FFFF),
        (164, one),
        (165, 0xBAAD_CAFE),
        (166, super::caps::MAX_POINT_SIZE.to_bits()),
        (168, 0xF),
        (171, 1),
        (172, 3),
        (173, 1),
    ]
}

/// A texture stage's states as a device starts with them.
fn stage_defaults(stage: u32) -> Vec<(u32, u32)> {
    let first = stage == 0;
    vec![
        (1, if first { 4 } else { 1 }),
        (2, 2),
        (3, 1),
        (4, if first { 2 } else { 1 }),
        (5, 2),
        (6, 1),
        (11, stage),
        (13, 1),
        (14, 1),
        (16, 1),
        (17, 1),
        (21, 1),
        (25, 1),
        (26, 1),
        (27, 1),
        (28, 1),
    ]
}

/// Let go of the textures, vertex buffers and index buffer the device
/// `id` has.
pub fn unbind(cpu: &mut Cpu, id: u32) {
    for s in 0..STAGES as u32 {
        set_texture_now(cpu, id, s, 0);
    }
    let Some(d) = get(cpu, id) else {
        return;
    };
    let mut gone: Vec<u32> = d.streams.iter().map(|s| s.0).filter(|&b| b != 0).collect();
    gone.push(d.indices.0);
    d.streams = vec![(0, 0); STREAMS];
    d.indices = (0, 0);
    for b in gone {
        if b != 0 {
            super::let_go(cpu, b);
        }
    }
}

/// Give the device `id` the state a new device has.
pub fn defaults(cpu: &mut Cpu, id: u32) {
    unbind(cpu, id);
    let Some(d) = get(cpu, id) else {
        return;
    };
    let depth = d.depth != 0;
    d.stages = vec![0; STAGES * 32];
    for s in 0..STAGES as u32 {
        for (k, v) in stage_defaults(s) {
            d.stages[(s * 32 + k) as usize] = v;
        }
    }
    d.textures = vec![0; STAGES];
    d.streams = vec![(0, 0); STREAMS];
    d.vertex_shader = 0;
    d.pixel_shader = 0;
    d.constants = vec![[0; 4]; VS_CONSTANTS];
    d.ps_constants = vec![[0; 4]; PS_CONSTANTS];
    d.worlds.clear();
    d.recording = None;
    d.clip_status = [0, 0xFFFF_FFFF];
    d.palette = 0xFFFF;
    let stages = d.stages.clone();
    let Some(i) = inner(cpu, id) else {
        return;
    };
    i.render.iter_mut().for_each(|r| *r = 0);
    for (s, v) in render_defaults(depth) {
        i.render[s as usize] = v;
    }
    for (at, v) in stages.iter().enumerate() {
        i.stages[at] = inner_stage(at as u32 % 32, *v);
    }
    i.transforms.iter_mut().for_each(|t| *t = IDENTITY);
    i.material = [0; 17];
    i.lights.clear();
    i.planes = vec![[0.0; 4]; 6];
    i.in_scene = false;
}

// The methods.

/// SetRenderState(this, state, value).
pub fn set_render_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, s, value] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    if s == 0 || s >= 256 {
        return Some(D3DERR_INVALIDCALL);
    }
    set(cpu, id, change(RENDER, s, 0, vec![value]));
    Some(D3D_OK)
}

/// GetRenderState(this, state, out).
pub fn get_render_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, s, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    if s == 0 || s >= 256 {
        return Some(D3DERR_INVALIDCALL);
    }
    let value = inner(cpu, id).map_or(0, |i| i.state(s));
    if !super::com::write_out(cpu, out, value) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// SetTextureStageState(this, stage, type, value).
pub fn set_texture_stage_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, stage, kind, value] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    if stage as usize >= STAGES || !(1..=28).contains(&kind) {
        return Some(D3DERR_INVALIDCALL);
    }
    set(cpu, id, change(STAGE, stage, kind, vec![value]));
    Some(D3D_OK)
}

/// GetTextureStageState(this, stage, type, out).
pub fn get_texture_stage_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, stage, kind, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    if stage as usize >= STAGES || !(1..=28).contains(&kind) {
        return Some(D3DERR_INVALIDCALL);
    }
    let value = d.stages[(stage * 32 + kind) as usize];
    if !super::com::write_out(cpu, out, value) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// SetTexture(this, stage, texture): the device keeps the texture while
/// it has it, the program's count on it as it is.
pub fn set_texture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, stage, tex] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    if stage as usize >= STAGES {
        return Some(D3DERR_INVALIDCALL);
    }
    let t = if tex != 0 {
        match of(cpu, tex) {
            Some((V_TEXTURE8, t)) => t,
            _ => return Some(D3DERR_INVALIDCALL),
        }
    } else {
        0
    };
    set(cpu, id, change(TEXTURE, stage, 0, vec![t]));
    Some(D3D_OK)
}

/// GetTexture(this, stage, out).
pub fn get_texture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, stage, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    if stage as usize >= STAGES || out == 0 {
        return Some(D3DERR_INVALIDCALL);
    }
    let t = d.textures[stage as usize];
    if t == 0 {
        super::com::write_out(cpu, out, 0);
    } else if !super::give(cpu, t, out) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// The matrix at `at`, as words.
fn read_words<const N: usize>(cpu: &mut Cpu, at: u32) -> Option<[u32; N]> {
    if at == 0 || !cpu.bus.guest_probe(at, N * 4, false) {
        return None;
    }
    Some(std::array::from_fn(|i| {
        cpu.bus.guest_read_32(at + 4 * i as u32)
    }))
}

fn write_words(cpu: &mut Cpu, at: u32, words: &[u32]) -> bool {
    if at == 0 || !cpu.bus.guest_probe(at, words.len() * 4, true) {
        return false;
    }
    for (i, w) in words.iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, *w);
    }
    true
}

/// SetTransform(this, state, matrix).
pub fn set_transform(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, t, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    let Some(m) = read_words::<16>(cpu, at) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if !valid_transform(t) {
        return Some(D3DERR_INVALIDCALL);
    }
    set(cpu, id, change(TRANSFORM, t, 0, m.to_vec()));
    Some(D3D_OK)
}

/// GetTransform(this, state, matrix).
pub fn get_transform(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, t, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    if !valid_transform(t) {
        return Some(D3DERR_INVALIDCALL);
    }
    let i = inner(cpu, id).cloned().unwrap_or_default();
    let m = transform_words(&d, &i, t);
    if !write_words(cpu, at, &m) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// MultiplyTransform(this, state, matrix): the matrix times the one
/// there is, put there.
pub fn multiply_transform(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, t, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    let Some(m) = read_words::<16>(cpu, at) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if !valid_transform(t) {
        return Some(D3DERR_INVALIDCALL);
    }
    let i = inner(cpu, id).cloned().unwrap_or_default();
    let old = transform_words(&d, &i, t).map(f32::from_bits);
    let product = multiply(&m.map(f32::from_bits), &old);
    set(
        cpu,
        id,
        change(TRANSFORM, t, 0, product.map(f32::to_bits).to_vec()),
    );
    Some(D3D_OK)
}

/// SetViewport(this, viewport): D3DVIEWPORT8, inside the render target.
pub fn set_viewport(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    let Some(v) = read_words::<6>(cpu, at) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let target = device::dd_surface_of(cpu, d.target);
    let (w, h) = super::super::ddraw::state(cpu)
        .surface(target)
        .map_or((0, 0), |s| (s.width, s.height));
    let right = v[0] as u64 + v[2] as u64;
    let bottom = v[1] as u64 + v[3] as u64;
    if right > w as u64 || bottom > h as u64 {
        return Some(D3DERR_INVALIDCALL);
    }
    set(cpu, id, change(VIEWPORT, 0, 0, v.to_vec()));
    Some(D3D_OK)
}

/// GetViewport(this, viewport).
pub fn get_viewport(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    let c = current(cpu, id, &change(VIEWPORT, 0, 0, Vec::new()));
    if !write_words(cpu, at, &c.data) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// SetMaterial(this, material): D3DMATERIAL8.
pub fn set_material(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    let Some(m) = read_words::<17>(cpu, at) else {
        return Some(D3DERR_INVALIDCALL);
    };
    set(cpu, id, change(MATERIAL, 0, 0, m.to_vec()));
    Some(D3D_OK)
}

/// GetMaterial(this, material).
pub fn get_material(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    let m = inner(cpu, id).map_or([0; 17], |i| i.material);
    if !write_words(cpu, at, &m) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// SetLight(this, index, light): D3DLIGHT8, of a point, spot or
/// directional light.
pub fn set_light(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    let Some(l) = read_words::<26>(cpu, at) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if !(1..=3).contains(&l[0]) {
        return Some(D3DERR_INVALIDCALL);
    }
    set(cpu, id, change(LIGHT, index, 0, l.to_vec()));
    Some(D3D_OK)
}

/// GetLight(this, index, light).
pub fn get_light(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    let light = inner(cpu, id).and_then(|i| i.lights.iter().find(|l| l.index == index).cloned());
    let Some(l) = light else {
        return Some(D3DERR_INVALIDCALL);
    };
    if !write_words(cpu, at, &l.light) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// LightEnable(this, index, enable): a light there is none of yet is
/// made, white and directional.
pub fn light_enable(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, on] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    set(
        cpu,
        id,
        change(LIGHT_ENABLE, index, (on != 0) as u32, Vec::new()),
    );
    Some(D3D_OK)
}

/// GetLightEnable(this, index, out): 128 for one that is on.
pub fn get_light_enable(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    let light = inner(cpu, id).and_then(|i| i.lights.iter().find(|l| l.index == index).cloned());
    let Some(l) = light else {
        return Some(D3DERR_INVALIDCALL);
    };
    if !super::com::write_out(cpu, out, if l.enabled { 128 } else { 0 }) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// SetClipPlane(this, index, plane).
pub fn set_clip_plane(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    let Some(p) = read_words::<4>(cpu, at) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if index >= super::caps::MAX_CLIP_PLANES {
        return Some(D3DERR_INVALIDCALL);
    }
    set(cpu, id, change(PLANE, index, 0, p.to_vec()));
    Some(D3D_OK)
}

/// GetClipPlane(this, index, plane).
pub fn get_clip_plane(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    if index >= super::caps::MAX_CLIP_PLANES {
        return Some(D3DERR_INVALIDCALL);
    }
    let c = current(cpu, id, &change(PLANE, index, 0, Vec::new()));
    if !write_words(cpu, at, &c.data) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// SetClipStatus(this, status): D3DCLIPSTATUS8.
pub fn set_clip_status(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    let Some(s) = read_words::<2>(cpu, at) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if let Some(d) = get(cpu, id) {
        d.clip_status = s;
    }
    Some(D3D_OK)
}

/// GetClipStatus(this, status).
pub fn get_clip_status(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    if !write_words(cpu, at, &d.clip_status) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

// Palettes, which textures of the formats the device has don't use.

/// SetPaletteEntries(this, number, entries).
pub fn set_palette_entries(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, number, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    if number > 0xFFFF || at == 0 || !cpu.bus.guest_probe(at, 1024, false) {
        return Some(D3DERR_INVALIDCALL);
    }
    let mut entries = vec![0; 1024];
    cpu.bus.guest_read_bytes(at, &mut entries);
    if let Some(d) = get(cpu, id) {
        let n = number as usize;
        if d.palettes.len() <= n {
            d.palettes.resize(n + 1, Vec::new());
        }
        d.palettes[n] = entries;
    }
    Some(D3D_OK)
}

/// GetPaletteEntries(this, number, entries).
pub fn get_palette_entries(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, number, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    let Some(p) = d.palettes.get(number as usize).filter(|p| !p.is_empty()) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if at == 0 || !cpu.bus.guest_probe(at, 1024, true) {
        return Some(D3DERR_INVALIDCALL);
    }
    cpu.bus.guest_write_bytes(at, p);
    Some(D3D_OK)
}

/// SetCurrentTexturePalette(this, number).
pub fn set_current_texture_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, number] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    if d.palettes.get(number as usize).is_none_or(|p| p.is_empty()) {
        return Some(D3DERR_INVALIDCALL);
    }
    if let Some(d) = get(cpu, id) {
        d.palette = number;
    }
    Some(D3D_OK)
}

/// GetCurrentTexturePalette(this, out).
pub fn get_current_texture_palette(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    if !super::com::write_out(cpu, out, d.palette) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

// Streams, indices and shaders.

/// SetStreamSource(this, stream, buffer, stride): a NULL buffer keeps
/// the stride there was.
pub fn set_stream_source(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, stream, vb, stride] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    if stream as usize >= STREAMS {
        return Some(D3DERR_INVALIDCALL);
    }
    let b = if vb != 0 {
        match of(cpu, vb) {
            Some((V_VERTEXBUFFER8, b)) => b,
            _ => return Some(D3DERR_INVALIDCALL),
        }
    } else {
        0
    };
    set(cpu, id, change(STREAM, stream, 0, vec![b, stride]));
    Some(D3D_OK)
}

/// GetStreamSource(this, stream, buffer, stride).
pub fn get_stream_source(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, stream, out, stride_at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    if stream as usize >= STREAMS {
        return Some(D3DERR_INVALIDCALL);
    }
    let (b, stride) = d.streams[stream as usize];
    if !super::com::write_out(cpu, stride_at, stride) {
        return Some(D3DERR_INVALIDCALL);
    }
    if b == 0 {
        super::com::write_out(cpu, out, 0);
    } else if !super::give(cpu, b, out) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// SetIndices(this, buffer, base vertex index).
pub fn set_indices(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, ib, base] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    let b = if ib != 0 {
        match of(cpu, ib) {
            Some((V_INDEXBUFFER8, b)) => b,
            _ => return Some(D3DERR_INVALIDCALL),
        }
    } else {
        0
    };
    set(cpu, id, change(INDICES, 0, 0, vec![b, base]));
    Some(D3D_OK)
}

/// GetIndices(this, buffer, base vertex index).
pub fn get_indices(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out, base_at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    let (b, base) = d.indices;
    if !super::com::write_out(cpu, base_at, base) {
        return Some(D3DERR_INVALIDCALL);
    }
    if b == 0 {
        super::com::write_out(cpu, out, 0);
    } else if !super::give(cpu, b, out) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// The size of an index of an index buffer's format.
pub fn index_size(format: u32) -> u32 {
    match format {
        FMT_INDEX32 => 4,
        FMT_INDEX16 => 2,
        _ => 2,
    }
}

/// SetVertexShader(this, handle): an FVF code, or a shader's handle.
pub fn set_vertex_shader(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, handle] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    if super::shader::is_handle(handle) {
        if super::shader::get_shader(&d, handle).is_none() {
            return Some(D3DERR_INVALIDCALL);
        }
    } else if handle != 0 && super::super::ddraw::d3d::pipeline::Layout::of(handle).is_none() {
        return Some(D3DERR_INVALIDCALL);
    }
    set(cpu, id, change(VSHADER, 0, 0, vec![handle]));
    Some(D3D_OK)
}

/// GetVertexShader(this, out).
pub fn get_vertex_shader(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    if !super::com::write_out(cpu, out, d.vertex_shader) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// Set or get shader constants: `count` registers from `first` at `at`.
fn constants(cpu: &mut Cpu, args: &[u32], pixel: bool, write: bool) -> Option<u32> {
    let [this, first, at, count] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    let most = if pixel { PS_CONSTANTS } else { VS_CONSTANTS } as u64;
    if first as u64 + count as u64 > most {
        return Some(D3DERR_INVALIDCALL);
    }
    if count == 0 {
        return Some(D3D_OK);
    }
    if at == 0 || !cpu.bus.guest_probe(at, count as usize * 16, !write) {
        return Some(D3DERR_INVALIDCALL);
    }
    for r in 0..count {
        let reg = first + r;
        if write {
            let v: Vec<u32> = (0..4)
                .map(|k| cpu.bus.guest_read_32(at + 16 * r + 4 * k))
                .collect();
            let kind = if pixel { PCONST } else { VCONST };
            set(cpu, id, change(kind, reg, 0, v));
        } else {
            let list = if pixel { &d.ps_constants } else { &d.constants };
            let v = list[reg as usize];
            for (k, w) in v.iter().enumerate() {
                cpu.bus.guest_write_32(at + 16 * r + 4 * k as u32, *w);
            }
        }
    }
    Some(D3D_OK)
}

/// SetVertexShaderConstant(this, register, data, count).
pub fn set_vertex_shader_constant(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    constants(cpu, args, false, true)
}

/// GetVertexShaderConstant(this, register, data, count).
pub fn get_vertex_shader_constant(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    constants(cpu, args, false, false)
}

/// SetPixelShaderConstant(this, register, data, count).
pub fn set_pixel_shader_constant(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    constants(cpu, args, true, true)
}

/// GetPixelShaderConstant(this, register, data, count).
pub fn get_pixel_shader_constant(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    constants(cpu, args, true, false)
}

/// SetPixelShader(this, handle): only none, as the card has no pixel
/// shaders.
pub fn set_pixel_shader(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, handle] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    if handle != 0 && super::shader::get_pixel(&d, handle).is_none() {
        return Some(D3DERR_INVALIDCALL);
    }
    set(cpu, id, change(PSHADER, 0, 0, vec![handle]));
    Some(D3D_OK)
}

/// GetPixelShader(this, out).
pub fn get_pixel_shader(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    if !super::com::write_out(cpu, out, d.pixel_shader) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

// State blocks.

/// BeginStateBlock(this).
pub fn begin_state_block(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, d) = hr!(this_device(cpu, args[0]));
    if d.recording.is_some() {
        return Some(D3DERR_INVALIDCALL);
    }
    if let Some(d) = get(cpu, id) {
        d.recording = Some(Vec::new());
    }
    Some(D3D_OK)
}

/// Put a block in the device's list: its handle.
fn new_block(d: &mut Device, block: Vec<Change>) -> u32 {
    match d.blocks.iter().position(Option::is_none) {
        Some(i) => {
            d.blocks[i] = Some(block);
            i as u32 + 1
        }
        None => {
            d.blocks.push(Some(block));
            d.blocks.len() as u32
        }
    }
}

/// EndStateBlock(this, out).
pub fn end_state_block(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    if d.recording.is_none() || out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(D3DERR_INVALIDCALL);
    }
    let Some(d) = get(cpu, id) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let block = d.recording.take().unwrap_or_default();
    let handle = new_block(d, block);
    cpu.bus.guest_write_32(out, handle);
    Some(D3D_OK)
}

/// The block `handle` of the device `d`.
fn block(d: &Device, handle: u32) -> Option<Vec<Change>> {
    d.blocks.get(handle.checked_sub(1)? as usize)?.clone()
}

/// ApplyStateBlock(this, handle).
pub fn apply_state_block(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, handle] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    if d.recording.is_some() {
        return Some(D3DERR_INVALIDCALL);
    }
    let Some(b) = block(&d, handle) else {
        return Some(D3DERR_INVALIDCALL);
    };
    for c in &b {
        apply(cpu, id, c);
    }
    Some(D3D_OK)
}

/// CaptureStateBlock(this, handle): the block takes the device's state
/// for what it has.
pub fn capture_state_block(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, handle] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    if d.recording.is_some() {
        return Some(D3DERR_INVALIDCALL);
    }
    let Some(b) = block(&d, handle) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let captured: Vec<Change> = b.iter().map(|c| current(cpu, id, c)).collect();
    if let Some(d) = get(cpu, id) {
        d.blocks[handle as usize - 1] = Some(captured);
    }
    Some(D3D_OK)
}

/// DeleteStateBlock(this, handle).
pub fn delete_state_block(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, handle] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    if block(&d, handle).is_none() {
        return Some(D3DERR_INVALIDCALL);
    }
    if let Some(d) = get(cpu, id) {
        d.blocks[handle as usize - 1] = None;
    }
    Some(D3D_OK)
}

// D3DSTATEBLOCKTYPE.
const SBT_ALL: u32 = 1;
const SBT_PIXELSTATE: u32 = 2;
const SBT_VERTEXSTATE: u32 = 3;

/// The render states of a pixel state block.
const PIXEL_RENDER: &[u32] = &[
    7, 8, 9, 10, 14, 15, 16, 19, 20, 23, 24, 25, 26, 27, 36, 37, 38, 40, 47, 52, 53, 54, 55, 56,
    57, 58, 59, 60, 128, 129, 130, 131, 132, 133, 134, 135, 168, 171,
];

/// The render states of a vertex state block.
const VERTEX_RENDER: &[u32] = &[
    9, 22, 28, 29, 34, 35, 36, 37, 38, 48, 136, 137, 139, 140, 141, 142, 143, 145, 146, 147, 148,
    151, 152, 154, 155, 156, 157, 158, 159, 160, 161, 162, 163, 166, 167, 170, 172, 173,
];

/// CreateStateBlock(this, type, out): a block of the device's state as
/// it is, all of it or the part `type` says.
pub fn create_state_block(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, kind, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    if !(SBT_ALL..=SBT_VERTEXSTATE).contains(&kind) || d.recording.is_some() {
        return Some(D3DERR_INVALIDCALL);
    }
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(D3DERR_INVALIDCALL);
    }
    let all = kind == SBT_ALL;
    let pixel = all || kind == SBT_PIXELSTATE;
    let vertex = all || kind == SBT_VERTEXSTATE;
    let mut list = Vec::new();
    let renders: Vec<u32> = if all {
        (1..256).filter(|&s| render_state_valid(s)).collect()
    } else if pixel {
        PIXEL_RENDER.to_vec()
    } else {
        VERTEX_RENDER.to_vec()
    };
    for s in renders {
        list.push(change(RENDER, s, 0, Vec::new()));
    }
    for stage in 0..STAGES as u32 {
        for k in 1..=28 {
            let vertex_stage = matches!(k, 11 | 24);
            if pixel || vertex_stage && vertex {
                list.push(change(STAGE, stage, k, Vec::new()));
            }
        }
    }
    if vertex {
        let i = inner(cpu, id).cloned().unwrap_or_default();
        for l in &i.lights {
            list.push(change(LIGHT, l.index, 0, l.light.to_vec()));
            list.push(change(LIGHT_ENABLE, l.index, 0, Vec::new()));
        }
        list.push(change(VSHADER, 0, 0, Vec::new()));
        for r in 0..VS_CONSTANTS as u32 {
            list.push(change(VCONST, r, 0, Vec::new()));
        }
    }
    if pixel {
        list.push(change(PSHADER, 0, 0, Vec::new()));
        for r in 0..PS_CONSTANTS as u32 {
            list.push(change(PCONST, r, 0, Vec::new()));
        }
    }
    if all {
        for t in [2, 3].into_iter().chain(16..=23).chain(256..512) {
            list.push(change(TRANSFORM, t, 0, Vec::new()));
        }
        list.push(change(VIEWPORT, 0, 0, Vec::new()));
        list.push(change(MATERIAL, 0, 0, Vec::new()));
        for stage in 0..STAGES as u32 {
            list.push(change(TEXTURE, stage, 0, Vec::new()));
        }
        for p in 0..super::caps::MAX_CLIP_PLANES {
            list.push(change(PLANE, p, 0, Vec::new()));
        }
        for s in 0..STREAMS as u32 {
            list.push(change(STREAM, s, 0, Vec::new()));
        }
        list.push(change(INDICES, 0, 0, Vec::new()));
    }
    let captured: Vec<Change> = list.iter().map(|c| current(cpu, id, c)).collect();
    let Some(d) = get(cpu, id) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let handle = new_block(d, captured);
    cpu.bus.guest_write_32(out, handle);
    Some(D3D_OK)
}
