//! Devices: IDirect3DDevice to IDirect3DDevice7. A device draws into
//! its render target, a 16-bit surface in video memory with
//! DDSCAPS_3DDEVICE, and holds a reference on it. It has the state that
//! says how: render states, texture stages, transforms, lights and the
//! material, and the viewport (`states.rs`); and the handles Direct3D
//! 5's interfaces name matrices, materials and textures by.
//!
//! IDirect3DDevice (Direct3D 3 and 5's, with execute buffers) is made by
//! asking a surface for the HAL device's GUID; IDirect3DDevice2, 3 and 7
//! by CreateDevice. A device made through IDirect3D2 or 3 has
//! IDirect3DDevice, 2 and 3; one made through IDirect3D7 only
//! IDirect3DDevice7.

use super::super::surface::{
    self, DDPF_RGB, DDSCAPS_SYSTEMMEMORY, DDSCAPS_ZBUFFER, MEM_CARD, MEM_VIDEO,
};
use super::super::{
    DDERR_ALREADYINITIALIZED, DDERR_INVALIDCAPS, DDERR_INVALIDOBJECT, DDERR_INVALIDPARAMS,
    DDERR_INVALIDPIXELFORMAT, V_D3D1, V_D3D2, V_D3D3, V_D3D7, V_DEVICE1, V_DEVICE2, V_DEVICE3,
    V_SURFACE1, V_SURFACE4, V_SURFACE7, V_VIEWPORT1, V_VIEWPORT2, V_VIEWPORT3, state,
};
use super::pipeline::{IDENTITY, Matrix};
use super::{
    D3D_OK, D3DERR_NOCURRENTVIEWPORT, D3DERR_NOVIEWPORTS, D3DERR_SCENE_IN_SCENE,
    D3DERR_SCENE_NOT_IN_SCENE, D3DERR_STENCILBUFFER_NOTPRESENT, D3DERR_SURFACENOTINVIDMEM,
    D3DERR_TOOMANYOPERATIONS, D3DERR_ZBUFFER_NOTPRESENT, D3dObject, caps, probe, read_matrix,
    write_dwords, write_matrix,
};
use crate::cpu::Cpu;
use crate::win32::com;

/// What a handle names.
pub const HANDLE_MATRIX: u32 = 1;
pub const HANDLE_MATERIAL: u32 = 2;
pub const HANDLE_TEXTURE: u32 = 3;

/// A handle: what it names (`HANDLE_`, 0 for a free one), the object
/// (a material or a surface), or the matrix.
#[derive(Clone, Debug, Default)]
pub struct Handle {
    pub kind: u32,
    pub object: u32,
    pub matrix: Matrix,
}

/// Direct3D 7's viewport: the rectangle and the depth range.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Viewport7 {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub min_z: f32,
    pub max_z: f32,
}

/// A light Direct3D 7 has: its index, D3DLIGHT7's words, and whether it
/// is on.
#[derive(Clone, Debug, Default)]
pub struct LightSlot {
    pub index: u32,
    pub light: [u32; 26],
    pub enabled: bool,
}

/// A change a state block makes: what it is (`states::CHANGE_`), which
/// one, and the value.
#[derive(Clone, Debug, Default)]
pub struct Change {
    pub kind: u32,
    pub a: u32,
    pub b: u32,
    pub data: Vec<u32>,
}

/// Begin's primitive under way: its type, vertex format, flags, the
/// vertices given so far and, for BeginIndexed, the vertex array.
#[derive(Clone, Debug, Default)]
pub struct Begin {
    pub kind: u32,
    pub fvf: u32,
    pub flags: u32,
    pub vertices: Vec<u8>,
    pub indexed: bool,
    pub array: u32,
    pub count: u32,
    pub indices: Vec<u32>,
}

/// A device.
#[derive(Clone, Debug, Default)]
pub struct Device {
    /// The version of IDirect3D it was made through: 1 (a surface's), 2,
    /// 3 or 7.
    pub version: u32,
    /// The DirectDraw object it is of.
    pub ddraw: u32,
    /// The render target, and the interface of it the device holds.
    pub target: u32,
    pub target_iface: u32,
    /// The card's texture units.
    pub tmus: u32,
    pub in_scene: bool,
    /// The render states, by their number; the light states.
    pub render: Vec<u32>,
    pub light_states: Vec<u32>,
    /// The texture stages' states: 32 a stage.
    pub stages: Vec<u32>,
    /// The texture interface of each stage, which the device holds.
    pub textures: Vec<u32>,
    /// The transforms, by D3DTRANSFORMSTATETYPE.
    pub transforms: Vec<Matrix>,
    /// The matrix handles execute buffers made the world, view and
    /// projection transform.
    pub bound: [u32; 3],
    pub viewport: Viewport7,
    /// D3DMATERIAL7's words.
    pub material: [u32; 17],
    pub lights: Vec<LightSlot>,
    /// The user's clip planes.
    pub planes: Vec<[f32; 4]>,
    /// D3DCLIPSTATUS's words.
    pub clip_status: [u32; 8],
    /// The viewports added to it (objects and the interfaces it holds),
    /// and the current one.
    pub viewports: Vec<(u32, u32)>,
    pub current: (u32, u32),
    pub handles: Vec<Handle>,
    /// D3DSTATS's counts: triangles, lines, points, spans, vertices.
    pub stats: [u32; 5],
    pub begin: Option<Begin>,
    /// The state blocks, by their handles less 1, and the one being
    /// recorded.
    pub blocks: Vec<Option<Vec<Change>>>,
    pub recording: Option<Vec<Change>>,
}

crate::state_fields!(Handle {
    kind,
    object,
    matrix
});
crate::state_fields!(Viewport7 {
    x,
    y,
    width,
    height,
    min_z,
    max_z
});
crate::state_fields!(LightSlot {
    index,
    light,
    enabled
});
crate::state_fields!(Change { kind, a, b, data });
crate::state_fields!(Begin {
    kind,
    fvf,
    flags,
    vertices,
    indexed,
    array,
    count,
    indices
});
crate::state_fields!(Device {
    version,
    ddraw,
    target,
    target_iface,
    tmus,
    in_scene,
    render,
    light_states,
    stages,
    textures,
    transforms,
    bound,
    viewport,
    material,
    lights,
    planes,
    clip_status,
    viewports,
    current,
    handles,
    stats,
    begin,
    blocks,
    recording
});

/// The version of IDirect3DDevice a vtable is.
pub fn version(vtable: usize) -> u32 {
    match vtable {
        V_DEVICE1 => 1,
        V_DEVICE2 => 2,
        V_DEVICE3 => 3,
        _ => 7,
    }
}

/// The device of the interface `this`: its vtable and id.
pub fn device_of(cpu: &mut Cpu, this: u32) -> Result<(usize, u32), u32> {
    let (vtable, id) = super::of(cpu, this).ok_or(DDERR_INVALIDOBJECT)?;
    get(cpu, id).ok_or(DDERR_INVALIDOBJECT)?;
    Ok((vtable, id))
}

pub fn get(cpu: &mut Cpu, id: u32) -> Option<&mut Device> {
    super::object(cpu, id)?.device.as_mut()
}

// Render states the device starts with.
pub const RS_TEXTUREHANDLE: u32 = 1;
pub const RS_TEXTUREADDRESS: u32 = 3;
pub const RS_TEXTUREPERSPECTIVE: u32 = 4;
pub const RS_ZENABLE: u32 = 7;
pub const RS_FILLMODE: u32 = 8;
pub const RS_SHADEMODE: u32 = 9;
pub const RS_ZWRITEENABLE: u32 = 14;
pub const RS_LASTPIXEL: u32 = 16;
pub const RS_TEXTUREMAG: u32 = 17;
pub const RS_TEXTUREMIN: u32 = 18;
pub const RS_SRCBLEND: u32 = 19;
pub const RS_DESTBLEND: u32 = 20;
pub const RS_TEXTUREMAPBLEND: u32 = 21;
pub const RS_CULLMODE: u32 = 22;
pub const RS_ZFUNC: u32 = 23;
pub const RS_ALPHAFUNC: u32 = 25;
pub const RS_FOGENABLE: u32 = 28;
pub const RS_SPECULARENABLE: u32 = 29;
pub const RS_FOGTABLEMODE: u32 = 35;
pub const RS_FOGSTART: u32 = 36;
pub const RS_FOGEND: u32 = 37;
pub const RS_FOGDENSITY: u32 = 38;
pub const RS_TEXTUREADDRESSU: u32 = 44;
pub const RS_TEXTUREADDRESSV: u32 = 45;
pub const RS_STENCILFUNC: u32 = 56;
pub const RS_STENCILMASK: u32 = 58;
pub const RS_STENCILWRITEMASK: u32 = 59;
pub const RS_TEXTUREFACTOR: u32 = 60;
pub const RS_CLIPPING: u32 = 136;
pub const RS_LIGHTING: u32 = 137;
pub const RS_AMBIENT: u32 = 139;
pub const RS_FOGVERTEXMODE: u32 = 140;
pub const RS_COLORVERTEX: u32 = 141;
pub const RS_LOCALVIEWER: u32 = 142;
pub const RS_NORMALIZENORMALS: u32 = 143;
pub const RS_DIFFUSEMATERIALSOURCE: u32 = 145;
pub const RS_SPECULARMATERIALSOURCE: u32 = 146;
pub const RS_AMBIENTMATERIALSOURCE: u32 = 147;
pub const RS_EMISSIVEMATERIALSOURCE: u32 = 148;
pub const RS_CLIPPLANEENABLE: u32 = 152;
pub const RENDER_STATES: usize = 256;

// Texture stage states.
pub const TSS_COLOROP: u32 = 1;
pub const TSS_COLORARG1: u32 = 2;
pub const TSS_COLORARG2: u32 = 3;
pub const TSS_ALPHAOP: u32 = 4;
pub const TSS_ALPHAARG1: u32 = 5;
pub const TSS_ALPHAARG2: u32 = 6;
pub const TSS_TEXCOORDINDEX: u32 = 11;
pub const TSS_ADDRESS: u32 = 12;
pub const TSS_ADDRESSU: u32 = 13;
pub const TSS_ADDRESSV: u32 = 14;
pub const TSS_BORDERCOLOR: u32 = 15;
pub const TSS_MAGFILTER: u32 = 16;
pub const TSS_MINFILTER: u32 = 17;
pub const TSS_MIPFILTER: u32 = 18;
pub const TSS_MIPMAPLODBIAS: u32 = 19;
pub const TSS_MAXANISOTROPY: u32 = 21;
pub const TSS_LAST: u32 = 24;
pub const STAGES: u32 = 8;

pub const TOP_DISABLE: u32 = 1;
pub const TOP_SELECTARG1: u32 = 2;
pub const TOP_SELECTARG2: u32 = 3;
pub const TOP_MODULATE: u32 = 4;
pub const TOP_ADD: u32 = 7;
pub const TOP_BLENDTEXTUREALPHA: u32 = 13;
pub const TA_DIFFUSE: u32 = 0;
pub const TA_CURRENT: u32 = 1;
pub const TA_TEXTURE: u32 = 2;

pub const TRANSFORMS: usize = 24;

impl Device {
    fn new(version: u32, ddraw: u32, target: u32, target_iface: u32, tmus: u32) -> Device {
        let mut d = Device {
            version,
            ddraw,
            target,
            target_iface,
            tmus,
            render: vec![0; RENDER_STATES],
            light_states: vec![0; 9],
            stages: vec![0; (STAGES * 32) as usize],
            textures: vec![0; STAGES as usize],
            transforms: vec![IDENTITY; TRANSFORMS],
            planes: vec![[0.0; 4]; 6],
            ..Device::default()
        };
        let one = 1.0f32.to_bits();
        let defaults = [
            (RS_TEXTUREADDRESS, 1),
            (RS_TEXTUREPERSPECTIVE, 1),
            (RS_FILLMODE, 3),
            (RS_SHADEMODE, 2),
            (RS_ZWRITEENABLE, 1),
            (RS_LASTPIXEL, 1),
            (RS_TEXTUREMAG, 1),
            (RS_TEXTUREMIN, 1),
            (RS_SRCBLEND, 2),
            (RS_DESTBLEND, 1),
            (RS_TEXTUREMAPBLEND, 2),
            (RS_CULLMODE, 3),
            (RS_ZFUNC, 4),
            (RS_ALPHAFUNC, 8),
            (RS_FOGEND, one),
            (RS_FOGDENSITY, one),
            (RS_TEXTUREADDRESSU, 1),
            (RS_TEXTUREADDRESSV, 1),
            (RS_STENCILFUNC, 8),
            (RS_STENCILMASK, 0xFFFF_FFFF),
            (RS_STENCILWRITEMASK, 0xFFFF_FFFF),
            (RS_TEXTUREFACTOR, 0xFFFF_FFFF),
            (RS_CLIPPING, 1),
            (RS_LIGHTING, 1),
            (RS_COLORVERTEX, 1),
            (RS_LOCALVIEWER, 1),
            (RS_DIFFUSEMATERIALSOURCE, 1),
            (RS_SPECULARMATERIALSOURCE, 2),
        ];
        for (s, v) in defaults {
            d.render[s as usize] = v;
        }
        // A surface's device keys textures from the start, as Wine has
        // it (D3DRENDERSTATE_COLORKEYENABLE).
        if version == 1 {
            d.render[41] = 1;
        }
        // D3DCOLOR_RGB.
        d.light_states[3] = 2;
        for s in 0..STAGES {
            let first = s == 0;
            let values = [
                (TSS_COLOROP, if first { TOP_MODULATE } else { TOP_DISABLE }),
                (TSS_COLORARG1, TA_TEXTURE),
                (TSS_COLORARG2, TA_CURRENT),
                (
                    TSS_ALPHAOP,
                    if first { TOP_SELECTARG1 } else { TOP_DISABLE },
                ),
                (TSS_ALPHAARG1, TA_TEXTURE),
                (TSS_ALPHAARG2, TA_CURRENT),
                (TSS_TEXCOORDINDEX, s),
                (TSS_ADDRESS, 1),
                (TSS_ADDRESSU, 1),
                (TSS_ADDRESSV, 1),
                (TSS_MAGFILTER, 1),
                (TSS_MINFILTER, 1),
                (TSS_MIPFILTER, 1),
                (TSS_MAXANISOTROPY, 1),
            ];
            for (t, v) in values {
                d.stages[(s * 32 + t) as usize] = v;
            }
        }
        d
    }

    pub fn stage(&self, stage: u32, kind: u32) -> u32 {
        self.stages
            .get((stage * 32 + kind) as usize)
            .copied()
            .unwrap_or(0)
    }

    pub fn state(&self, s: u32) -> u32 {
        self.render.get(s as usize).copied().unwrap_or(0)
    }

    pub fn state_f(&self, s: u32) -> f32 {
        f32::from_bits(self.state(s))
    }

    /// The handle `h` of `kind`.
    pub fn handle(&self, h: u32, kind: u32) -> Option<&Handle> {
        let handle = self.handles.get(h.checked_sub(1)? as usize)?;
        (handle.kind == kind).then_some(handle)
    }

    pub fn handle_mut(&mut self, h: u32, kind: u32) -> Option<&mut Handle> {
        let handle = self.handles.get_mut(h.checked_sub(1)? as usize)?;
        (handle.kind == kind).then_some(handle)
    }

    /// A new handle for `kind` and `object`.
    pub fn new_handle(&mut self, kind: u32, object: u32) -> u32 {
        let handle = Handle {
            kind,
            object,
            matrix: [0.0; 16],
        };
        match self.handles.iter().position(|h| h.kind == 0) {
            Some(i) => {
                self.handles[i] = handle;
                i as u32 + 1
            }
            None => {
                self.handles.push(handle);
                self.handles.len() as u32
            }
        }
    }

    /// The handle of `object` as a `kind`, made if it has none.
    pub fn handle_of(&mut self, kind: u32, object: u32) -> u32 {
        match self
            .handles
            .iter()
            .position(|h| h.kind == kind && h.object == object)
        {
            Some(i) => i as u32 + 1,
            None => self.new_handle(kind, object),
        }
    }
}

/// Whether `id` can be a render target: an error if it can't, as the
/// HAL's driver had it. The surface is 16-bit 565 in video memory (or
/// an add-on card's buffer) with DDSCAPS_3DDEVICE, and its depth buffer,
/// if it has one, is 16-bit. A Banshee or Voodoo3 also draws into 24-
/// and 32-bit targets with 24- or 32-bit depth buffers, through 16-bit
/// shadows of them (`shadow.rs`).
pub fn check_target(cpu: &mut Cpu, id: u32) -> Result<(), u32> {
    let s = state(cpu).surface(id).cloned().ok_or(DDERR_INVALIDPARAMS)?;
    if s.caps[0] & surface::DDSCAPS_3DDEVICE == 0 {
        return Err(DDERR_INVALIDCAPS);
    }
    let video = s.memory == MEM_VIDEO || s.memory == MEM_CARD;
    if !video || s.caps[0] & DDSCAPS_SYSTEMMEMORY != 0 {
        return Err(D3DERR_SURFACENOTINVIDMEM);
    }
    let f = s.format;
    let wide = s.memory == MEM_VIDEO && crate::win32::glide::hw::banshee(cpu);
    let rgb565 = f.bits == 16 && (f.r, f.g, f.b) == (0xF800, 0x07E0, 0x001F);
    let rgb = matches!(f.bits, 24 | 32) && wide;
    if f.flags & DDPF_RGB == 0 || !(rgb565 || rgb) {
        return Err(DDERR_INVALIDPIXELFORMAT);
    }
    if let Some(z) = z_buffer(cpu, id) {
        let bits = state(cpu).surface(z).map_or(0, |z| z.format.bits);
        if bits != 16 && !(matches!(bits, 24 | 32) && wide) {
            return Err(DDERR_INVALIDPIXELFORMAT);
        }
    }
    Ok(())
}

/// The depth buffer attached to the surface `id`.
pub fn z_buffer(cpu: &mut Cpu, id: u32) -> Option<u32> {
    let s = state(cpu).surface(id)?;
    let list: Vec<u32> = s.attached.iter().map(|a| a.0).collect();
    list.into_iter().find(|&a| {
        state(cpu)
            .surface(a)
            .is_some_and(|z| z.caps[0] & DDSCAPS_ZBUFFER != 0)
    })
}

/// Make a device of the DirectDraw object `dd` on the render target
/// `target` (the program's interface `target_iface` of it), and give the
/// program its interface `vtable` at `out`.
pub fn create(
    cpu: &mut Cpu,
    version: u32,
    dd: u32,
    target_iface: u32,
    vtable: usize,
    out: u32,
) -> u32 {
    let Some((_, target)) = super::super::this(cpu, target_iface) else {
        return DDERR_INVALIDPARAMS;
    };
    let Some(tmus) = super::hal(cpu, dd) else {
        return com::E_OUTOFMEMORY;
    };
    if let Err(e) = check_target(cpu, target) {
        return e;
    }
    let mut device = Device::new(version, dd, target, target_iface, tmus);
    let (w, h, z) = {
        let z = z_buffer(cpu, target).is_some();
        let s = state(cpu).surface(target);
        (s.map_or(0, |s| s.width), s.map_or(0, |s| s.height), z)
    };
    device.render[RS_ZENABLE as usize] = z as u32;
    device.viewport = Viewport7 {
        x: 0,
        y: 0,
        width: w,
        height: h,
        min_z: 0.0,
        max_z: 1.0,
    };
    let object = D3dObject {
        device: Some(device),
        ..D3dObject::default()
    };
    match super::add(cpu, dd, object, vtable, out) {
        Ok(_) => {
            super::super::add_ref(cpu, target_iface);
            super::hal::created(cpu);
            D3D_OK
        }
        Err(e) => e,
    }
}

/// The device `id` goes: it lets go of its render target, textures and
/// viewports.
pub fn destroyed(cpu: &mut Cpu, id: u32, d: Device) {
    let mut held: Vec<u32> = d.textures.iter().copied().filter(|&t| t != 0).collect();
    held.push(d.target_iface);
    held.extend(d.viewports.iter().map(|v| v.1));
    if d.current.1 != 0 {
        held.push(d.current.1);
    }
    for (vp, _) in &d.viewports {
        if let Some(v) = super::object(cpu, *vp).and_then(|o| o.viewport.as_mut())
            && v.device == id
        {
            v.device = 0;
        }
    }
    for iface in held {
        super::super::release(cpu, iface);
    }
    super::hal::release(cpu, id);
}

/// Every device forgets the handles of `kind` of `object`, which went.
pub fn forget_handles(cpu: &mut Cpu, kind: u32, object: u32) {
    for o in state(cpu).objects.iter_mut().flatten() {
        let Some(d) = o.d3d.as_mut().and_then(|d| d.device.as_mut()) else {
            continue;
        };
        for h in d.handles.iter_mut() {
            if h.kind == kind && h.object == object {
                *h = Handle::default();
            }
        }
    }
}

// The methods.

/// Initialize(this, d3d, guid, desc): it was.
pub fn initialize(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DDERR_ALREADYINITIALIZED)
}

/// GetCaps(this, hal, hel) of IDirect3DDevice to 3.
pub fn get_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, hal, hel] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    let tmus = get(cpu, id).map_or(1, |d| d.tmus);
    let mut sizes = [0; 2];
    for (i, at) in [hal, hel].into_iter().enumerate() {
        if !probe(cpu, at, 4, true) {
            return Some(DDERR_INVALIDPARAMS);
        }
        let size = cpu.bus.guest_read_32(at);
        if ![caps::DESC_V1, caps::DESC_V2, caps::DESC_V3].contains(&size)
            || !probe(cpu, at, size, true)
        {
            return Some(DDERR_INVALIDPARAMS);
        }
        sizes[i] = size;
    }
    caps::write_desc(cpu, hal, sizes[0], true, tmus);
    caps::write_desc(cpu, hel, sizes[1], false, tmus);
    Some(D3D_OK)
}

/// GetCaps(this, desc) of IDirect3DDevice7.
pub fn get_caps7(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, desc] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !probe(cpu, desc, caps::DESC7, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let tmus = get(cpu, id).map_or(1, |d| d.tmus);
    caps::write_desc7(cpu, desc, tmus);
    Some(D3D_OK)
}

/// SwapTextureHandles(this, texture1, texture2): the two textures swap
/// their handles.
pub fn swap_texture_handles(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, a, b] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    let (Some((_, ta)), Some((_, tb))) = (super::super::this(cpu, a), super::super::this(cpu, b))
    else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let Some(d) = get(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let ha = d.handle_of(HANDLE_TEXTURE, ta);
    let hb = d.handle_of(HANDLE_TEXTURE, tb);
    d.handles[ha as usize - 1].object = tb;
    d.handles[hb as usize - 1].object = ta;
    Some(D3D_OK)
}

/// GetStats(this, stats).
pub fn get_stats(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !probe(cpu, at, 24, true) || cpu.bus.guest_read_32(at) != 24 {
        return Some(DDERR_INVALIDPARAMS);
    }
    let stats = get(cpu, id).map_or([0; 5], |d| d.stats);
    write_dwords(cpu, at + 4, &stats);
    Some(D3D_OK)
}

/// The viewport interface of a device's version.
fn viewport_vtable(version: u32) -> usize {
    match version {
        1 => V_VIEWPORT1,
        2 => V_VIEWPORT2,
        _ => V_VIEWPORT3,
    }
}

/// AddViewport(this, viewport).
pub fn add_viewport(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, vp] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    let Ok(vid) = super::viewport::viewport_of(cpu, vp) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if get(cpu, id).is_some_and(|d| d.viewports.iter().any(|v| v.0 == vid)) {
        return Some(DDERR_INVALIDPARAMS);
    }
    super::super::add_ref(cpu, vp);
    if let Some(d) = get(cpu, id) {
        d.viewports.push((vid, vp));
    }
    if let Some(v) = super::object(cpu, vid).and_then(|o| o.viewport.as_mut()) {
        v.device = id;
    }
    Some(D3D_OK)
}

/// DeleteViewport(this, viewport).
pub fn delete_viewport(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, vp] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    let Ok(vid) = super::viewport::viewport_of(cpu, vp) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let Some(d) = get(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let Some(i) = d.viewports.iter().position(|v| v.0 == vid) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, held) = d.viewports.remove(i);
    let current = (d.current.0 == vid).then_some(d.current.1);
    if current.is_some() {
        d.current = (0, 0);
    }
    if let Some(v) = super::object(cpu, vid).and_then(|o| o.viewport.as_mut()) {
        v.device = 0;
    }
    if let Some(c) = current {
        super::super::release(cpu, c);
    }
    super::super::release(cpu, held);
    Some(D3D_OK)
}

/// NextViewport(this, viewport, out, flags): D3DNEXT_NEXT (1) the one
/// after `viewport`, D3DNEXT_HEAD (2) the first, D3DNEXT_TAIL (4) the
/// last.
pub fn next_viewport(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, vp, out, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    if !com::write_out(cpu, out, 0) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let list = get(cpu, id)
        .map(|d| d.viewports.clone())
        .unwrap_or_default();
    if list.is_empty() {
        return Some(D3DERR_NOVIEWPORTS);
    }
    let next = match flags {
        2 => list.first().copied(),
        4 => list.last().copied(),
        1 => {
            let Ok(vid) = super::viewport::viewport_of(cpu, vp) else {
                return Some(DDERR_INVALIDPARAMS);
            };
            let Some(i) = list.iter().position(|v| v.0 == vid) else {
                return Some(DDERR_INVALIDPARAMS);
            };
            list.get(i + 1).copied()
        }
        _ => return Some(DDERR_INVALIDPARAMS),
    };
    if let Some((vid, _)) = next {
        let address = super::super::iface(cpu, vid, viewport_vtable(version(vtable)));
        com::write_out(cpu, out, address);
    }
    Some(D3D_OK)
}

/// Pick(this, buffer, viewport, flags, rect): nothing is picked.
pub fn pick(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(device_of(cpu, args[0]));
    Some(D3D_OK)
}

/// GetPickRecords(this, count, records): there are none.
pub fn get_pick_records(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, count, _] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    hr!(device_of(cpu, this));
    if !com::write_out(cpu, count, 0) {
        return Some(DDERR_INVALIDPARAMS);
    }
    Some(D3D_OK)
}

/// CreateMatrix(this, out): a handle of a matrix of zeros.
pub fn create_matrix(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !probe(cpu, out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let h = get(cpu, id).map_or(0, |d| d.new_handle(HANDLE_MATRIX, 0));
    cpu.bus.guest_write_32(out, h);
    Some(D3D_OK)
}

/// SetMatrix(this, handle, matrix). A matrix an execute buffer made a
/// transform is that transform again.
pub fn set_matrix(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, h, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !probe(cpu, at, 64, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let m = read_matrix(cpu, at);
    let Some(d) = get(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let Some(handle) = d.handle_mut(h, HANDLE_MATRIX) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    handle.matrix = m;
    for i in 0..3 {
        if d.bound[i] == h {
            d.transforms[i + 1] = m;
        }
    }
    Some(D3D_OK)
}

/// GetMatrix(this, handle, matrix).
pub fn get_matrix(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, h, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if !probe(cpu, at, 64, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(m) = get(cpu, id).and_then(|d| d.handle(h, HANDLE_MATRIX).map(|h| h.matrix)) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    write_matrix(cpu, at, &m);
    Some(D3D_OK)
}

/// DeleteMatrix(this, handle).
pub fn delete_matrix(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, h] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    let Some(d) = get(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let Some(handle) = d.handle_mut(h, HANDLE_MATRIX) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    *handle = Handle::default();
    for b in d.bound.iter_mut() {
        if *b == h {
            *b = 0;
        }
    }
    Some(D3D_OK)
}

/// BeginScene(this).
pub fn begin_scene(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(device_of(cpu, args[0]));
    let Some(d) = get(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if d.in_scene {
        return Some(D3DERR_SCENE_IN_SCENE);
    }
    d.in_scene = true;
    Some(D3D_OK)
}

/// EndScene(this).
pub fn end_scene(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, id) = hr!(device_of(cpu, args[0]));
    let Some(d) = get(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if !d.in_scene {
        return Some(D3DERR_SCENE_NOT_IN_SCENE);
    }
    d.in_scene = false;
    let target = d.target;
    super::shadow::expose(cpu, target);
    Some(D3D_OK)
}

/// GetDirect3D(this, out): the IDirect3D of the device's version.
pub fn get_direct3d(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    if !com::write_out(cpu, out, 0) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let dd = get(cpu, id).map_or(0, |d| d.ddraw);
    let v = match version(vtable) {
        1 => V_D3D1,
        2 => V_D3D2,
        3 => V_D3D3,
        _ => V_D3D7,
    };
    Some(super::super::give(cpu, dd, v, out))
}

/// SetCurrentViewport(this, viewport): one added to the device.
pub fn set_current_viewport(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, vp] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    let Ok(vid) = super::viewport::viewport_of(cpu, vp) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let Some(d) = get(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if !d.viewports.iter().any(|v| v.0 == vid) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let old = std::mem::replace(&mut d.current, (vid, vp));
    super::super::add_ref(cpu, vp);
    if old.1 != 0 {
        super::super::release(cpu, old.1);
    }
    Some(D3D_OK)
}

/// GetCurrentViewport(this, out).
pub fn get_current_viewport(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    if !com::write_out(cpu, out, 0) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let current = get(cpu, id).map_or(0, |d| d.current.0);
    if current == 0 {
        return Some(D3DERR_NOCURRENTVIEWPORT);
    }
    Some(super::super::give(
        cpu,
        current,
        viewport_vtable(version(vtable)),
        out,
    ))
}

/// SetRenderTarget(this, surface, flags). The viewport stays as it is.
pub fn set_render_target(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, target, _] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    let Some((_, sid)) = super::super::this(cpu, target) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if state(cpu).surface(sid).is_none() {
        return Some(DDERR_INVALIDPARAMS);
    }
    hr!(check_target(cpu, sid));
    super::super::add_ref(cpu, target);
    let z = z_buffer(cpu, sid).is_some();
    let Some(d) = get(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let old = d.target_iface;
    d.target = sid;
    d.target_iface = target;
    if !z {
        d.render[RS_ZENABLE as usize] = 0;
    }
    super::super::release(cpu, old);
    Some(D3D_OK)
}

/// GetRenderTarget(this, out): the surface interface of the device's
/// version.
pub fn get_render_target(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(device_of(cpu, this));
    if !com::write_out(cpu, out, 0) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let target = get(cpu, id).map_or(0, |d| d.target);
    let v = match version(vtable) {
        7 => V_SURFACE7,
        3 => V_SURFACE4,
        _ => V_SURFACE1,
    };
    Some(super::super::give(cpu, target, v, out))
}

// D3DCLEAR_ flags.
pub const CLEAR_TARGET: u32 = 1;
pub const CLEAR_ZBUFFER: u32 = 2;
pub const CLEAR_STENCIL: u32 = 4;

/// Clear(this, count, rects, flags, color, z, stencil). The depth
/// buffer is cleared only if there is one, and there is no stencil.
pub fn clear(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, count, rects, flags, color, z, _] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    if count != 0 && !probe(cpu, rects, count.saturating_mul(16), false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(d) = get(cpu, id).cloned() else {
        return Some(DDERR_INVALIDOBJECT);
    };
    if flags & CLEAR_ZBUFFER != 0 && z_buffer(cpu, d.target).is_none() {
        return Some(D3DERR_ZBUFFER_NOTPRESENT);
    }
    if flags & CLEAR_STENCIL != 0 {
        return Some(D3DERR_STENCILBUFFER_NOTPRESENT);
    }
    let v = &d.viewport;
    let list = clip_rects(read_rects(cpu, rects, count), [v.x, v.y, v.width, v.height]);
    if count != 0 && list.is_empty() {
        return Some(D3D_OK);
    }
    let target = (flags & CLEAR_TARGET != 0).then_some(color);
    let depth = (flags & CLEAR_ZBUFFER != 0).then_some(f32::from_bits(z));
    super::hal::clear(cpu, &d, &list, target, depth);
    Some(D3D_OK)
}

/// Clear keeps to the viewport (x, y, width, height): the rectangles
/// are cut to it, and no rectangles are the whole of it. A viewport
/// with no size leaves them as they are.
pub fn clip_rects(rects: Vec<[i32; 4]>, viewport: [u32; 4]) -> Vec<[i32; 4]> {
    let [x, y, w, h] = viewport.map(|v| v.min(0x7FFF) as i32);
    if w == 0 || h == 0 {
        return rects;
    }
    let area = [x, y, x + w, y + h];
    if rects.is_empty() {
        return vec![area];
    }
    rects
        .into_iter()
        .map(|r| {
            [
                r[0].max(area[0]),
                r[1].max(area[1]),
                r[2].min(area[2]),
                r[3].min(area[3]),
            ]
        })
        .filter(|r| r[0] < r[2] && r[1] < r[3])
        .collect()
}

/// `count` D3DRECTs at `at`, which the caller has probed.
pub fn read_rects(cpu: &mut Cpu, at: u32, count: u32) -> Vec<[i32; 4]> {
    (0..count.min(0x1000))
        .map(|i| super::read_dwords::<4>(cpu, at + 16 * i).map(|v| v as i32))
        .collect()
}

/// PreLoad(this, texture): textures are loaded when they are drawn
/// with.
pub fn preload(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(device_of(cpu, args[0]));
    Some(D3D_OK)
}

/// ValidateDevice(this, passes): one pass, if the stages in use are no
/// more than the card's texture units.
pub fn validate_device(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, passes] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, id) = hr!(device_of(cpu, this));
    let Some(d) = get(cpu, id).cloned() else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let used = (0..STAGES)
        .take_while(|&s| d.stage(s, TSS_COLOROP) != TOP_DISABLE)
        .count() as u32;
    if used > d.tmus {
        return Some(D3DERR_TOOMANYOPERATIONS);
    }
    for s in 0..used {
        if !super::hal::can_do(d.stage(s, TSS_COLOROP)) {
            return Some(super::D3DERR_UNSUPPORTEDCOLOROPERATION);
        }
        let alpha = d.stage(s, TSS_ALPHAOP);
        if alpha != TOP_DISABLE && !super::hal::can_do(alpha) {
            return Some(super::D3DERR_UNSUPPORTEDALPHAOPERATION);
        }
    }
    if !com::write_out(cpu, passes, 1) {
        return Some(DDERR_INVALIDPARAMS);
    }
    Some(D3D_OK)
}

/// GetInfo(this, id, data, size): there is no information.
pub fn get_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(device_of(cpu, args[0]));
    Some(com::S_FALSE)
}
