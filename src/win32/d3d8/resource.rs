//! Resources: surfaces (IDirect3DSurface8), textures (IDirect3DTexture8)
//! and vertex and index buffers, and the methods of IDirect3DResource8
//! they share.
//!
//! A surface is a DirectDraw surface of its device's DirectDraw object:
//! a render target and a depth buffer in video memory, an image surface
//! in system memory. A texture is a DirectDraw texture with its mipmap
//! levels, in video memory for D3DPOOL_DEFAULT, managed for
//! D3DPOOL_MANAGED (in system memory, which the HAL copies to the card),
//! and in system memory for the other pools; each level is a surface of
//! Direct3D 8's whose count is the texture's. The HAL draws from 565,
//! 1555 and 4444 textures where they are and copies the others,
//! converted, to its heap on the card, so textures of the 32-bit formats
//! work too though CheckDeviceFormat leaves them out, as the Voodoo3's
//! driver did. Cube and volume textures fail, as the card has none.
//!
//! Vertex and index buffers are blocks of the process's memory.

use super::super::ddraw::surface::{
    self as dd_surface, DDPF_ALPHAPIXELS, DDPF_RGB, DDPF_ZBUFFER, DDSCAPS_3DDEVICE,
    DDSCAPS_COMPLEX, DDSCAPS_FLIP, DDSCAPS_MIPMAP, DDSCAPS_OFFSCREENPLAIN, DDSCAPS_PRIMARYSURFACE,
    DDSCAPS_SYSTEMMEMORY, DDSCAPS_TEXTURE, DDSCAPS_VIDEOMEMORY, DDSCAPS_ZBUFFER,
    DDSD_BACKBUFFERCOUNT, DDSD_CAPS, DDSD_HEIGHT, DDSD_MIPMAPCOUNT, DDSD_PIXELFORMAT, DDSD_WIDTH,
    Desc,
};
use super::super::ddraw::{self, Format, V_SURFACE7};
use super::{
    D3D_OK, D3DERR_INVALIDCALL, D3DERR_NOTFOUND, D3DERR_OUTOFVIDEOMEMORY, Object, V_INDEXBUFFER8,
    V_SURFACE8, V_TEXTURE8, V_VERTEXBUFFER8, add, object, of, state,
};
use crate::cpu::Cpu;
use crate::win32::com::{self, Guid};

// D3DFORMATs.
pub const FMT_UNKNOWN: u32 = 0;
pub const FMT_R8G8B8: u32 = 20;
pub const FMT_A8R8G8B8: u32 = 21;
pub const FMT_X8R8G8B8: u32 = 22;
pub const FMT_R5G6B5: u32 = 23;
pub const FMT_X1R5G5B5: u32 = 24;
pub const FMT_A1R5G5B5: u32 = 25;
pub const FMT_A4R4G4B4: u32 = 26;
pub const FMT_X4R4G4B4: u32 = 30;
pub const FMT_D16_LOCKABLE: u32 = 70;
pub const FMT_D24X8: u32 = 77;
pub const FMT_D16: u32 = 80;
pub const FMT_VERTEXDATA: u32 = 100;
pub const FMT_INDEX16: u32 = 101;
pub const FMT_INDEX32: u32 = 102;

// D3DPOOL.
pub const POOL_DEFAULT: u32 = 0;
pub const POOL_MANAGED: u32 = 1;
pub const POOL_SYSTEMMEM: u32 = 2;
pub const POOL_SCRATCH: u32 = 3;

// D3DUSAGE.
pub const USAGE_RENDERTARGET: u32 = 0x1;
pub const USAGE_DEPTHSTENCIL: u32 = 0x2;

// D3DRESOURCETYPE.
const RTYPE_SURFACE: u32 = 1;
const RTYPE_TEXTURE: u32 = 3;
const RTYPE_VERTEXBUFFER: u32 = 6;
const RTYPE_INDEXBUFFER: u32 = 7;

/// Data SetPrivateData stored.
#[derive(Clone, Debug, Default)]
pub struct Private {
    pub guid: [u8; 16],
    pub flags: u32,
    pub data: Vec<u8>,
}

/// A surface: the DirectDraw surface it is, and the interface of it it
/// holds a reference on (none for a texture's level or a back buffer
/// of a flip chain, which go with their root).
#[derive(Clone, Debug, Default)]
pub struct Surface {
    pub dd: u32,
    pub dd_iface: u32,
    pub owns: bool,
    pub format: u32,
    pub usage: u32,
    pub pool: u32,
    pub width: u32,
    pub height: u32,
    pub multisample: u32,
    pub lockable: bool,
    pub locked: bool,
    /// Its level in its texture.
    pub level: u32,
}

/// A texture: its root DirectDraw surface's interface, which it holds,
/// and its levels (surfaces of Direct3D 8's).
#[derive(Clone, Debug, Default)]
pub struct Texture {
    pub dd: u32,
    pub dd_iface: u32,
    pub levels: Vec<u32>,
    pub format: u32,
    pub usage: u32,
    pub pool: u32,
    pub lod: u32,
}

/// A vertex or index buffer: its memory in the process.
#[derive(Clone, Debug, Default)]
pub struct Buffer {
    pub index: bool,
    pub memory: u32,
    pub size: u32,
    pub usage: u32,
    pub pool: u32,
    /// The FVF code of a vertex buffer, the D3DFORMAT of an index
    /// buffer's indices.
    pub format: u32,
    pub locks: u32,
}

/// An additional swap chain: its back buffers and presentation
/// parameters.
#[derive(Clone, Debug, Default)]
pub struct SwapChain {
    pub backbuffers: Vec<u32>,
    pub params: Vec<u32>,
}

crate::state_fields!(Private { guid, flags, data });
crate::state_fields!(Surface {
    dd,
    dd_iface,
    owns,
    format,
    usage,
    pool,
    width,
    height,
    multisample,
    lockable,
    locked,
    level
});
crate::state_fields!(Texture {
    dd,
    dd_iface,
    levels,
    format,
    usage,
    pool,
    lod
});
crate::state_fields!(Buffer {
    index,
    memory,
    size,
    usage,
    pool,
    format,
    locks
});
crate::state_fields!(SwapChain {
    backbuffers,
    params
});

/// DirectDraw's pixel format of a D3DFORMAT, if surfaces can have it.
pub fn dd_format(format: u32) -> Option<Format> {
    let rgb = |bits: u32, r: u32, g: u32, b: u32, a: u32| Format {
        flags: DDPF_RGB | if a != 0 { DDPF_ALPHAPIXELS } else { 0 },
        fourcc: 0,
        bits,
        r,
        g,
        b,
        a,
    };
    Some(match format {
        FMT_R5G6B5 => rgb(16, 0xF800, 0x07E0, 0x001F, 0),
        FMT_X1R5G5B5 => rgb(16, 0x7C00, 0x03E0, 0x001F, 0),
        FMT_A1R5G5B5 => rgb(16, 0x7C00, 0x03E0, 0x001F, 0x8000),
        FMT_A4R4G4B4 => rgb(16, 0x0F00, 0x00F0, 0x000F, 0xF000),
        FMT_X4R4G4B4 => rgb(16, 0x0F00, 0x00F0, 0x000F, 0),
        FMT_R8G8B8 => rgb(24, 0xFF_0000, 0xFF00, 0xFF, 0),
        FMT_X8R8G8B8 => rgb(32, 0xFF_0000, 0xFF00, 0xFF, 0),
        FMT_A8R8G8B8 => rgb(32, 0xFF_0000, 0xFF00, 0xFF, 0xFF00_0000),
        FMT_D16 | FMT_D16_LOCKABLE => Format {
            flags: DDPF_ZBUFFER,
            bits: 16,
            g: 0xFFFF,
            ..Format::default()
        },
        FMT_D24X8 => Format {
            flags: DDPF_ZBUFFER,
            bits: 32,
            g: 0xFF_FFFF,
            ..Format::default()
        },
        _ => return None,
    })
}

/// The D3DFORMAT of a display depth.
pub fn display_format(bpp: u32) -> u32 {
    match bpp {
        16 => FMT_R5G6B5,
        24 => FMT_R8G8B8,
        32 => FMT_X8R8G8B8,
        _ => FMT_UNKNOWN,
    }
}

/// Whether `format` is a depth buffer's.
pub fn is_depth(format: u32) -> bool {
    matches!(format, 70..=80)
}

/// What a DirectDraw surface is made as.
pub enum Role {
    Target,
    Depth,
    Image,
    Texture(u32),
    /// The primary surface and its back buffers, for full screen.
    Primary(u32),
}

/// Make the DirectDraw surfaces of `role` for the device `device`: their
/// ids, the root first, with a reference on the root's interface.
pub fn make_dd(
    cpu: &mut Cpu,
    device: u32,
    role: Role,
    size: (u32, u32),
    format: u32,
    pool: u32,
) -> Result<(Vec<u32>, u32), u32> {
    let Some(dd) = super::device::get(cpu, device).map(|d| d.dd) else {
        return Err(D3DERR_INVALIDCALL);
    };
    let level = ddraw::directdraw::ddraw(cpu, dd).map_or(0, |d| d.level);
    let mut desc = Desc {
        flags: DDSD_CAPS | DDSD_WIDTH | DDSD_HEIGHT | DDSD_PIXELFORMAT,
        width: size.0,
        height: size.1,
        ..Desc::default()
    };
    if !matches!(role, Role::Primary(_)) {
        desc.format = dd_format(format).ok_or(D3DERR_INVALIDCALL)?;
    }
    let memory = match pool {
        POOL_DEFAULT => DDSCAPS_VIDEOMEMORY,
        POOL_MANAGED => 0,
        _ => DDSCAPS_SYSTEMMEMORY,
    };
    match role {
        Role::Target => desc.caps[0] = DDSCAPS_3DDEVICE | DDSCAPS_OFFSCREENPLAIN | memory,
        Role::Depth => desc.caps[0] = DDSCAPS_ZBUFFER | memory,
        Role::Image => desc.caps[0] = DDSCAPS_OFFSCREENPLAIN | DDSCAPS_SYSTEMMEMORY,
        Role::Texture(levels) => {
            desc.caps[0] = DDSCAPS_TEXTURE | memory;
            if pool == POOL_MANAGED {
                // DDSCAPS2_TEXTUREMANAGE.
                desc.caps[1] = 0x10;
            }
            if levels > 1 {
                desc.caps[0] |= DDSCAPS_MIPMAP | DDSCAPS_COMPLEX;
                desc.flags |= DDSD_MIPMAPCOUNT;
                desc.mipmaps = levels;
            }
        }
        Role::Primary(backbuffers) => {
            desc.flags = DDSD_CAPS | DDSD_BACKBUFFERCOUNT;
            desc.caps[0] =
                DDSCAPS_PRIMARYSURFACE | DDSCAPS_FLIP | DDSCAPS_COMPLEX | DDSCAPS_3DDEVICE;
            desc.backbuffers = backbuffers;
        }
    }
    let ids = dd_surface::make(cpu, dd, 7, level, &desc).map_err(|e| {
        if e == ddraw::DDERR_OUTOFVIDEOMEMORY {
            D3DERR_OUTOFVIDEOMEMORY
        } else {
            D3DERR_INVALIDCALL
        }
    })?;
    let root = ddraw::iface(cpu, ids[0], V_SURFACE7);
    if root == 0 {
        ddraw::destroy(cpu, ids[0]);
        return Err(com::E_OUTOFMEMORY);
    }
    ddraw::add_ref(cpu, root);
    Ok((ids, root))
}

/// A new surface of Direct3D 8's for the DirectDraw surface `dd` of the
/// device `device`: with a reference for the program, or with none for
/// an implicit one. Its id, or 0.
#[allow(clippy::too_many_arguments)]
pub fn new_surface(
    cpu: &mut Cpu,
    device: u32,
    dd: u32,
    dd_iface: u32,
    format: u32,
    usage: u32,
    pool: u32,
    implicit: bool,
) -> u32 {
    let (width, height) = ddraw::state(cpu)
        .surface(dd)
        .map_or((0, 0), |s| (s.width, s.height));
    let s = Surface {
        dd,
        dd_iface: if dd_iface != 0 {
            dd_iface
        } else {
            ddraw::iface(cpu, dd, V_SURFACE7)
        },
        owns: dd_iface != 0,
        format,
        usage,
        pool,
        width,
        height,
        lockable: !is_depth(format) || format == FMT_D16_LOCKABLE,
        ..Surface::default()
    };
    let o = Object {
        device,
        implicit,
        surface: Some(s),
        ..Object::default()
    };
    let id = add(cpu, o, V_SURFACE8, if implicit { 0 } else { 1 });
    if id == 0 && dd_iface != 0 {
        ddraw::release(cpu, dd_iface);
    }
    id
}

/// The object `id` goes: what it is of DirectDraw goes with it. A
/// texture's levels go with it, and devices forget it.
pub fn destroyed(cpu: &mut Cpu, id: u32, o: &Object) {
    if o.device != 0 {
        super::device::forget(cpu, o.device, id);
    }
    if let Some(s) = &o.surface
        && s.owns
    {
        ddraw::release(cpu, s.dd_iface);
    }
    if let Some(t) = &o.texture {
        for &level in &t.levels {
            super::destroy(cpu, level);
        }
        ddraw::release(cpu, t.dd_iface);
    }
    if let Some(b) = &o.buffer
        && b.memory != 0
    {
        crate::win32::kernel32::free_pages(cpu, b.memory);
    }
    if let Some(c) = &o.chain {
        for &b in &c.backbuffers {
            super::destroy(cpu, b);
        }
    }
}

const D3DSPD_IUNKNOWN: u32 = 1;

/// The interface pointer private data holds, if it holds one.
fn held_pointer(p: &Private) -> Option<u32> {
    (p.flags & D3DSPD_IUNKNOWN != 0 && p.data.len() == 4)
        .then(|| u32::from_le_bytes([p.data[0], p.data[1], p.data[2], p.data[3]]))
}

/// Add or take a reference on an interface private data holds: one of
/// Direct3D 8's or DirectDraw's. Another's count stays as it is.
fn reference(cpu: &mut Cpu, iface: u32, add: bool) {
    if let Some((_, id)) = of(cpu, iface) {
        if add {
            super::add_ref(cpu, id);
        } else {
            super::release(cpu, id);
        }
    } else if ddraw::this(cpu, iface).is_some() {
        if add {
            ddraw::add_ref(cpu, iface);
        } else {
            ddraw::release(cpu, iface);
        }
    }
}

/// An object went with private data: the interfaces it held go.
pub fn release_private(cpu: &mut Cpu, list: &[Private]) {
    for p in list {
        if let Some(iface) = held_pointer(p) {
            reference(cpu, iface, false);
        }
    }
}

// Making them.

/// The device of the interface `this`, an error if it is none.
fn device_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    super::device::device_of(cpu, this)
}

/// The number of levels a texture of `w` x `h` has: all of them for 0.
fn level_count(w: u32, h: u32, asked: u32) -> u32 {
    let all = 32 - w.max(h).leading_zeros();
    if asked == 0 || asked > all {
        all
    } else {
        asked
    }
}

/// CreateTexture(this, width, height, levels, usage, format, pool,
/// out).
pub fn create_texture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, w, h, levels, usage, format, pool, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let device = hr!(device_of(cpu, this));
    if !com::write_out(cpu, out, 0) {
        return Some(D3DERR_INVALIDCALL);
    }
    if w == 0 || h == 0 || w > 4096 || h > 4096 || pool > POOL_SCRATCH {
        return Some(D3DERR_INVALIDCALL);
    }
    if dd_format(format).is_none() || is_depth(format) {
        return Some(D3DERR_INVALIDCALL);
    }
    if usage & USAGE_RENDERTARGET != 0 && pool != POOL_DEFAULT {
        return Some(D3DERR_INVALIDCALL);
    }
    let levels = level_count(w, h, levels);
    let (ids, root) = hr!(make_dd(
        cpu,
        device,
        Role::Texture(levels),
        (w, h),
        format,
        pool
    ));
    if usage & USAGE_RENDERTARGET != 0 {
        // A texture the device can draw into.
        for &id in &ids {
            if let Some(s) = ddraw::state(cpu).surface_mut(id) {
                s.caps[0] |= DDSCAPS_3DDEVICE;
            }
        }
    }
    let t = Texture {
        dd: ids[0],
        dd_iface: root,
        format,
        usage,
        pool,
        ..Texture::default()
    };
    let o = Object {
        device,
        texture: Some(t),
        ..Object::default()
    };
    let id = add(cpu, o, V_TEXTURE8, 1);
    if id == 0 {
        ddraw::release(cpu, root);
        return Some(com::E_OUTOFMEMORY);
    }
    let mut list = Vec::new();
    for (i, &dd) in ids.iter().enumerate() {
        let level = new_surface(cpu, device, dd, 0, format, usage, pool, false);
        if let Some(o) = object(cpu, level) {
            // Its count is the texture's.
            o.refs = 0;
            o.container = id;
            if o.holds {
                o.holds = false;
            }
            if let Some(s) = o.surface.as_mut() {
                s.level = i as u32;
            }
        }
        // `new_surface` took a reference on the device for it, unless it
        // made none.
        if level != 0 {
            super::release(cpu, device);
        }
        list.push(level);
    }
    if let Some(t) = object(cpu, id).and_then(|o| o.texture.as_mut()) {
        t.levels = list;
    }
    let iface = super::iface(cpu, id);
    com::write_out(cpu, out, iface);
    Some(D3D_OK)
}

/// CreateVolumeTexture(this, width, height, depth, levels, usage,
/// format, pool, out): the card has no volume textures.
pub fn create_volume_texture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(device_of(cpu, args[0]));
    com::write_out(cpu, args[8], 0);
    Some(D3DERR_INVALIDCALL)
}

/// CreateCubeTexture(this, edge, levels, usage, format, pool, out): nor
/// cube textures.
pub fn create_cube_texture(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(device_of(cpu, args[0]));
    com::write_out(cpu, args[6], 0);
    Some(D3DERR_INVALIDCALL)
}

/// A buffer of `size` bytes.
#[allow(clippy::too_many_arguments)]
fn create_buffer(
    cpu: &mut Cpu,
    this: u32,
    size: u32,
    usage: u32,
    format: u32,
    pool: u32,
    out: u32,
    index: bool,
) -> u32 {
    let device = match device_of(cpu, this) {
        Ok(d) => d,
        Err(e) => return e,
    };
    if !com::write_out(cpu, out, 0) || pool > POOL_SCRATCH {
        return D3DERR_INVALIDCALL;
    }
    if size == 0 || size > 0x400_0000 {
        return D3DERR_INVALIDCALL;
    }
    if index && format != FMT_INDEX16 && format != FMT_INDEX32 {
        return D3DERR_INVALIDCALL;
    }
    let Some(memory) = crate::win32::kernel32::alloc_pages(cpu, size) else {
        return D3DERR_OUTOFVIDEOMEMORY;
    };
    let b = Buffer {
        index,
        memory,
        size,
        usage,
        pool,
        format,
        locks: 0,
    };
    let o = Object {
        device,
        buffer: Some(b),
        ..Object::default()
    };
    let vtable = if index {
        V_INDEXBUFFER8
    } else {
        V_VERTEXBUFFER8
    };
    let id = add(cpu, o, vtable, 1);
    if id == 0 {
        crate::win32::kernel32::free_pages(cpu, memory);
        return com::E_OUTOFMEMORY;
    }
    let iface = super::iface(cpu, id);
    com::write_out(cpu, out, iface);
    D3D_OK
}

/// CreateVertexBuffer(this, length, usage, fvf, pool, out).
pub fn create_vertex_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, size, usage, fvf, pool, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    Some(create_buffer(cpu, this, size, usage, fvf, pool, out, false))
}

/// CreateIndexBuffer(this, length, usage, format, pool, out).
pub fn create_index_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, size, usage, format, pool, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    Some(create_buffer(
        cpu, this, size, usage, format, pool, out, true,
    ))
}

/// A standalone surface of `role`, given at `out`.
#[allow(clippy::too_many_arguments)]
fn create_surface(
    cpu: &mut Cpu,
    this: u32,
    role: Role,
    w: u32,
    h: u32,
    format: u32,
    usage: u32,
    pool: u32,
    out: u32,
) -> u32 {
    let device = match device_of(cpu, this) {
        Ok(d) => d,
        Err(e) => return e,
    };
    if !com::write_out(cpu, out, 0) || w == 0 || h == 0 || w > 4096 || h > 4096 {
        return D3DERR_INVALIDCALL;
    }
    let (ids, root) = match make_dd(cpu, device, role, (w, h), format, pool) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let id = new_surface(cpu, device, ids[0], root, format, usage, pool, false);
    if id == 0 {
        return com::E_OUTOFMEMORY;
    }
    let iface = super::iface(cpu, id);
    com::write_out(cpu, out, iface);
    D3D_OK
}

/// Whether `format` can be a render target: 565 and the 32-bit formats,
/// those through 16-bit shadows.
pub fn target_format(format: u32) -> bool {
    matches!(format, FMT_R5G6B5 | FMT_X8R8G8B8 | FMT_A8R8G8B8)
}

/// CreateRenderTarget(this, width, height, format, multisample,
/// lockable, out).
pub fn create_render_target(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, w, h, format, ms, _, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    if !target_format(format) || ms > 1 {
        com::write_out(cpu, out, 0);
        return Some(D3DERR_INVALIDCALL);
    }
    let hr = create_surface(
        cpu,
        this,
        Role::Target,
        w,
        h,
        format,
        USAGE_RENDERTARGET,
        POOL_DEFAULT,
        out,
    );
    Some(hr)
}

/// CreateDepthStencilSurface(this, width, height, format, multisample,
/// out): of the formats `direct3d::depth_format` has.
pub fn create_depth_stencil(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, w, h, format, ms, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    if !super::direct3d::depth_format(format) || ms > 1 {
        com::write_out(cpu, out, 0);
        return Some(D3DERR_INVALIDCALL);
    }
    let hr = create_surface(
        cpu,
        this,
        Role::Depth,
        w,
        h,
        format,
        USAGE_DEPTHSTENCIL,
        POOL_DEFAULT,
        out,
    );
    Some(hr)
}

/// CreateImageSurface(this, width, height, format, out): in system
/// memory.
pub fn create_image_surface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, w, h, format, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    if is_depth(format) {
        com::write_out(cpu, out, 0);
        return Some(D3DERR_INVALIDCALL);
    }
    let hr = create_surface(cpu, this, Role::Image, w, h, format, 0, POOL_SYSTEMMEM, out);
    Some(hr)
}

// IDirect3DResource8.

/// The object `this` is, with an error if it is none.
fn this_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    of(cpu, this).map(|(_, id)| id).ok_or(D3DERR_INVALIDCALL)
}

/// GetDevice(this, out).
pub fn get_device(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let id = hr!(this_of(cpu, this));
    let device = state(cpu).object(id).map_or(0, |o| o.device);
    if !super::give(cpu, device, out) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// SetPrivateData(this, guid, data, size, flags): with D3DSPD_IUNKNOWN
/// the data is an interface, which it holds a reference on.
pub fn set_private_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, guid, data, size, flags] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let id = hr!(this_of(cpu, this));
    let Some(g) = Guid::read(cpu, guid) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if flags & D3DSPD_IUNKNOWN != 0 && size != 4 {
        return Some(D3DERR_INVALIDCALL);
    }
    // An IUnknown's data is the interface pointer itself.
    let bytes = if flags & D3DSPD_IUNKNOWN != 0 {
        data.to_le_bytes().to_vec()
    } else {
        if size > 0x10_0000 || size > 0 && !cpu.bus.guest_probe(data, size as usize, false) {
            return Some(D3DERR_INVALIDCALL);
        }
        let mut bytes = vec![0; size as usize];
        cpu.bus.guest_read_bytes(data, &mut bytes);
        bytes
    };
    free(cpu, id, &g.0);
    let p = Private {
        guid: g.0,
        flags,
        data: bytes,
    };
    if let Some(iface) = held_pointer(&p) {
        reference(cpu, iface, true);
    }
    if let Some(o) = object(cpu, id) {
        o.private.push(p);
    }
    Some(D3D_OK)
}

/// Take the data `guid` off the object `id`: whether it had it.
fn free(cpu: &mut Cpu, id: u32, guid: &[u8; 16]) -> bool {
    let Some(o) = object(cpu, id) else {
        return false;
    };
    let Some(i) = o.private.iter().position(|p| p.guid == *guid) else {
        return false;
    };
    let p = o.private.remove(i);
    release_private(cpu, &[p]);
    true
}

/// GetPrivateData(this, guid, data, size): the size it needs when
/// `data` is NULL or too small.
pub fn get_private_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, guid, data, size_at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let id = hr!(this_of(cpu, this));
    let Some(g) = Guid::read(cpu, guid) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if !cpu.bus.guest_probe(size_at, 4, true) {
        return Some(D3DERR_INVALIDCALL);
    }
    let Some(p) = state(cpu)
        .object(id)
        .and_then(|o| o.private.iter().find(|p| p.guid == g.0).cloned())
    else {
        return Some(D3DERR_NOTFOUND);
    };
    let room = cpu.bus.guest_read_32(size_at);
    cpu.bus.guest_write_32(size_at, p.data.len() as u32);
    if data == 0 {
        return Some(D3D_OK);
    }
    if (room as usize) < p.data.len() {
        return Some(super::D3DERR_MOREDATA);
    }
    if !cpu.bus.guest_probe(data, p.data.len(), true) {
        return Some(D3DERR_INVALIDCALL);
    }
    cpu.bus.guest_write_bytes(data, &p.data);
    // The interface given back has a reference of the caller's.
    if let Some(iface) = held_pointer(&p) {
        reference(cpu, iface, true);
    }
    Some(D3D_OK)
}

/// FreePrivateData(this, guid).
pub fn free_private_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, guid] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let id = hr!(this_of(cpu, this));
    let Some(g) = Guid::read(cpu, guid) else {
        return Some(D3DERR_INVALIDCALL);
    };
    Some(if free(cpu, id, &g.0) {
        D3D_OK
    } else {
        D3DERR_NOTFOUND
    })
}

/// SetPriority(this, priority): the one before.
pub fn set_priority(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok(id) = this_of(cpu, args[0]) else {
        return Some(0);
    };
    let o = object(cpu, id)?;
    Some(std::mem::replace(&mut o.priority, args[1]))
}

/// GetPriority(this).
pub fn get_priority(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok(id) = this_of(cpu, args[0]) else {
        return Some(0);
    };
    Some(state(cpu).object(id).map_or(0, |o| o.priority))
}

/// PreLoad(this): textures go to the card when they are drawn with.
pub fn preload(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// GetType(this): its D3DRESOURCETYPE.
pub fn get_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((vtable, _)) = of(cpu, args[0]) else {
        return Some(0);
    };
    Some(match vtable {
        V_SURFACE8 => RTYPE_SURFACE,
        V_TEXTURE8 => RTYPE_TEXTURE,
        V_VERTEXBUFFER8 => RTYPE_VERTEXBUFFER,
        V_INDEXBUFFER8 => RTYPE_INDEXBUFFER,
        _ => 0,
    })
}

// IDirect3DSurface8.

/// The surface `this` is: its id and what it has.
pub fn surface_of(cpu: &mut Cpu, this: u32) -> Result<(u32, Surface), u32> {
    let id = this_of(cpu, this)?;
    let s = state(cpu)
        .object(id)
        .and_then(|o| o.surface.clone())
        .ok_or(D3DERR_INVALIDCALL)?;
    Ok((id, s))
}

/// GetContainer(this, iid, out): a level's texture, or the device for
/// the others.
pub fn get_container(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(surface_of(cpu, this));
    let Some(g) = Guid::read(cpu, iid) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let o = state(cpu).object(id).cloned().unwrap_or_default();
    let container = if o.container != 0 {
        o.container
    } else {
        o.device
    };
    Some(super::query(cpu, container, &g, out))
}

/// Write D3DSURFACE_DESC at `at`.
fn write_surface_desc(cpu: &mut Cpu, at: u32, s: &Surface) -> bool {
    if !cpu.bus.guest_probe(at, 32, true) {
        return false;
    }
    let size = ddraw::state(cpu).surface(s.dd).map_or(0, |d| d.size());
    let words = [
        s.format,
        RTYPE_SURFACE,
        s.usage,
        s.pool,
        size,
        s.multisample,
        s.width,
        s.height,
    ];
    for (i, w) in words.iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, *w);
    }
    true
}

/// GetDesc(this, desc).
pub fn surface_get_desc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, s) = hr!(surface_of(cpu, this));
    if !write_surface_desc(cpu, at, &s) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// Lock the surface `id`: D3DLOCKED_RECT's pitch and bits at `at`, for
/// the rectangle at `rect` or all of it.
fn lock_surface(cpu: &mut Cpu, id: u32, at: u32, rect: u32) -> u32 {
    let Some(s) = state(cpu).object(id).and_then(|o| o.surface.clone()) else {
        return D3DERR_INVALIDCALL;
    };
    if s.locked || !s.lockable || !cpu.bus.guest_probe(at, 8, true) {
        return D3DERR_INVALIDCALL;
    }
    let Some(dd) = ddraw::state(cpu).surface(s.dd).cloned() else {
        return D3DERR_INVALIDCALL;
    };
    let (mut x, mut y) = (0, 0);
    if rect != 0 {
        if !cpu.bus.guest_probe(rect, 16, false) {
            return D3DERR_INVALIDCALL;
        }
        let r: [i32; 4] =
            std::array::from_fn(|i| cpu.bus.guest_read_32(rect + 4 * i as u32) as i32);
        let (w, h) = (dd.width as i32, dd.height as i32);
        if r[0] < 0 || r[1] < 0 || r[0] >= r[2] || r[1] >= r[3] || r[2] > w || r[3] > h {
            return D3DERR_INVALIDCALL;
        }
        (x, y) = (r[0] as u32, r[1] as u32);
    }
    // What the card drew into a 32-bit target's shadow is there.
    ddraw::d3d::shadow::expose(cpu, s.dd);
    let Some(base) = dd_surface::address(cpu, &dd) else {
        return D3DERR_INVALIDCALL;
    };
    let bits = base + y * dd.pitch + x * dd.bytes_per_pixel();
    cpu.bus.guest_write_32(at, dd.pitch);
    cpu.bus.guest_write_32(at + 4, bits);
    if let Some(s) = object(cpu, id).and_then(|o| o.surface.as_mut()) {
        s.locked = true;
    }
    D3D_OK
}

/// Unlock the surface `id`: its pixels changed, and its texture's.
fn unlock_surface(cpu: &mut Cpu, id: u32) -> u32 {
    let Some(o) = state(cpu).object(id).cloned() else {
        return D3DERR_INVALIDCALL;
    };
    let Some(s) = o.surface else {
        return D3DERR_INVALIDCALL;
    };
    if !s.locked {
        return D3DERR_INVALIDCALL;
    }
    if let Some(s) = object(cpu, id).and_then(|o| o.surface.as_mut()) {
        s.locked = false;
    }
    dd_surface::changed(cpu, s.dd);
    if let Some(t) = state(cpu)
        .object(o.container)
        .and_then(|c| c.texture.clone())
    {
        dd_surface::changed(cpu, t.dd);
    }
    D3D_OK
}

/// LockRect(this, locked, rect, flags).
pub fn surface_lock_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at, rect, _] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(surface_of(cpu, this));
    Some(lock_surface(cpu, id, at, rect))
}

/// UnlockRect(this).
pub fn surface_unlock_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = hr!(surface_of(cpu, args[0]));
    Some(unlock_surface(cpu, id))
}

// IDirect3DBaseTexture8 and IDirect3DTexture8.

/// The texture `this` is: its id and what it has.
pub fn texture_of(cpu: &mut Cpu, this: u32) -> Result<(u32, Texture), u32> {
    let id = this_of(cpu, this)?;
    let t = state(cpu)
        .object(id)
        .and_then(|o| o.texture.clone())
        .ok_or(D3DERR_INVALIDCALL)?;
    Ok((id, t))
}

/// SetLOD(this, lod): the one before. Only managed textures have one.
pub fn set_lod(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok((id, t)) = texture_of(cpu, args[0]) else {
        return Some(0);
    };
    if t.pool != POOL_MANAGED {
        return Some(0);
    }
    let lod = args[1].min(t.levels.len().saturating_sub(1) as u32);
    let t = object(cpu, id)?.texture.as_mut()?;
    Some(std::mem::replace(&mut t.lod, lod))
}

/// GetLOD(this).
pub fn get_lod(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(texture_of(cpu, args[0]).map_or(0, |(_, t)| t.lod))
}

/// GetLevelCount(this).
pub fn get_level_count(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(texture_of(cpu, args[0]).map_or(0, |(_, t)| t.levels.len() as u32))
}

/// The level `level` of the texture `this`.
fn level_of(cpu: &mut Cpu, this: u32, level: u32) -> Result<u32, u32> {
    let (_, t) = texture_of(cpu, this)?;
    t.levels
        .get(level as usize)
        .copied()
        .ok_or(D3DERR_INVALIDCALL)
}

/// GetLevelDesc(this, level, desc).
pub fn get_level_desc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, level, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let id = hr!(level_of(cpu, this, level));
    let Some(s) = state(cpu).object(id).and_then(|o| o.surface.clone()) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if !write_surface_desc(cpu, at, &s) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// GetSurfaceLevel(this, level, out).
pub fn get_surface_level(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, level, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let id = hr!(level_of(cpu, this, level));
    if !super::give(cpu, id, out) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// LockRect(this, level, locked, rect, flags).
pub fn texture_lock_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, level, at, rect, _] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let id = hr!(level_of(cpu, this, level));
    Some(lock_surface(cpu, id, at, rect))
}

/// UnlockRect(this, level).
pub fn texture_unlock_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = hr!(level_of(cpu, args[0], args[1]));
    Some(unlock_surface(cpu, id))
}

/// AddDirtyRect(this, rect): the whole texture is copied again when it
/// changed.
pub fn add_dirty_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (_, t) = hr!(texture_of(cpu, args[0]));
    dd_surface::changed(cpu, t.dd);
    Some(D3D_OK)
}

// Vertex and index buffers.

/// The buffer `this` is: its id and what it has.
pub fn buffer_of(cpu: &mut Cpu, this: u32) -> Result<(u32, Buffer), u32> {
    let id = this_of(cpu, this)?;
    let b = state(cpu)
        .object(id)
        .and_then(|o| o.buffer.clone())
        .ok_or(D3DERR_INVALIDCALL)?;
    Ok((id, b))
}

/// Lock(this, offset, size, out, flags): any number of times, with any
/// flags.
pub fn buffer_lock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, offset, _, out, _] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, b) = hr!(buffer_of(cpu, this));
    if offset > b.size || !com::write_out(cpu, out, b.memory + offset) {
        return Some(D3DERR_INVALIDCALL);
    }
    if let Some(b) = object(cpu, id).and_then(|o| o.buffer.as_mut()) {
        b.locks += 1;
    }
    Some(D3D_OK)
}

/// Unlock(this).
pub fn buffer_unlock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, _) = hr!(buffer_of(cpu, args[0]));
    if let Some(b) = object(cpu, id).and_then(|o| o.buffer.as_mut()) {
        b.locks = b.locks.saturating_sub(1);
    }
    Some(D3D_OK)
}

/// GetDesc(this, desc) of a vertex buffer: D3DVERTEXBUFFER_DESC.
pub fn vertex_buffer_get_desc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, b) = hr!(buffer_of(cpu, this));
    if !cpu.bus.guest_probe(at, 24, true) {
        return Some(D3DERR_INVALIDCALL);
    }
    let words = [
        FMT_VERTEXDATA,
        RTYPE_VERTEXBUFFER,
        b.usage,
        b.pool,
        b.size,
        b.format,
    ];
    for (i, w) in words.iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, *w);
    }
    Some(D3D_OK)
}

/// GetDesc(this, desc) of an index buffer: D3DINDEXBUFFER_DESC.
pub fn index_buffer_get_desc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, b) = hr!(buffer_of(cpu, this));
    if !cpu.bus.guest_probe(at, 20, true) {
        return Some(D3DERR_INVALIDCALL);
    }
    let words = [b.format, RTYPE_INDEXBUFFER, b.usage, b.pool, b.size];
    for (i, w) in words.iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, *w);
    }
    Some(D3D_OK)
}
