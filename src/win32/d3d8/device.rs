//! IDirect3DDevice8: the device, its swap chains, and the methods that
//! aren't about its state (`states.rs`), drawing (`draw.rs`), shaders
//! (`shader.rs`) or making resources (`resource.rs`).
//!
//! A device has a DirectDraw object of its own and a Direct3D device of
//! version 8 of that object (`inner`), which draws with the HAL. Full
//! screen, the DirectDraw object has exclusive mode and the display mode
//! of the presentation parameters, and the swap chain is the primary
//! surface's flip chain: Present flips it. In a window the back buffers
//! are render targets in video memory, and Present copies the first to
//! the window's client area on the desktop, converted to the desktop's
//! format and stretched to the size it has there, where the window can
//! be seen. The cursor SetCursorProperties gives isn't drawn.

use super::super::ddraw::d3d::device::{self as inner_device, Device as Inner, Viewport7};
use super::super::ddraw::d3d::{D3dObject, hal, shadow};
use super::super::ddraw::directdraw::{
    self, DDSCL_EXCLUSIVE, DDSCL_FULLSCREEN, DDSCL_NORMAL, DDSCL_NOWINDOWCHANGES,
};
use super::super::ddraw::surface as dd_surface;
use super::super::ddraw::{self, V_DD7, V_DEVICE7};
use super::resource::{
    self, FMT_A8R8G8B8, FMT_R5G6B5, FMT_X1R5G5B5, FMT_X8R8G8B8, POOL_DEFAULT, Role, SwapChain,
    USAGE_DEPTHSTENCIL, USAGE_RENDERTARGET,
};
use super::{
    D3D_OK, D3DERR_INVALIDCALL, D3DERR_NOTAVAILABLE, D3DERR_NOTFOUND, D3DERR_TOOMANYOPERATIONS,
    D3DERR_UNSUPPORTEDALPHAOPERATION, D3DERR_UNSUPPORTEDCOLOROPERATION, Object, V_DEVICE8,
    V_SWAPCHAIN8, add, object, of, state,
};
use crate::cpu::Cpu;
use crate::win32::com;
use crate::win32::gdi32::Rect;

/// D3DPRESENT_PARAMETERS' fields, by their places.
pub const PP_WIDTH: usize = 0;
pub const PP_HEIGHT: usize = 1;
pub const PP_FORMAT: usize = 2;
pub const PP_COUNT: usize = 3;
pub const PP_MULTISAMPLE: usize = 4;
pub const PP_SWAPEFFECT: usize = 5;
pub const PP_WINDOW: usize = 6;
pub const PP_WINDOWED: usize = 7;
pub const PP_AUTODEPTH: usize = 8;
pub const PP_DEPTHFORMAT: usize = 9;
pub const PP_INTERVAL: usize = 12;
pub const PARAMS: usize = 13;

const SWAPEFFECT_DISCARD: u32 = 1;
const SWAPEFFECT_COPY: u32 = 3;
const SWAPEFFECT_COPY_VSYNC: u32 = 4;
const PRESENT_INTERVAL_IMMEDIATE: u32 = 0x8000_0000;

// D3DCREATE_ flags.
const CREATE_SOFTWARE_VERTEXPROCESSING: u32 = 0x20;
const CREATE_HARDWARE_VERTEXPROCESSING: u32 = 0x40;
const CREATE_MIXED_VERTEXPROCESSING: u32 = 0x80;

/// D3DDEVTYPE_HAL.
pub const DEVTYPE_HAL: u32 = 1;
pub const DEVTYPE_REF: u32 = 2;

/// A device of Direct3D 8's.
#[derive(Clone, Debug, Default)]
pub struct Device {
    /// The IDirect3D8 it was made by, which it holds.
    pub d3d: u32,
    /// Its DirectDraw object and the IDirectDraw7 it holds of it.
    pub dd: u32,
    pub dd_iface: u32,
    /// The Direct3D device that draws, and the interface it holds of it.
    pub inner: u32,
    pub inner_iface: u32,
    /// CreateDevice's focus window, the device window, its D3DCREATE_
    /// flags and device type.
    pub focus: u32,
    pub window: u32,
    pub behavior: u32,
    pub devtype: u32,
    /// D3DPRESENT_PARAMETERS, as the device has them.
    pub params: Vec<u32>,
    /// Full screen: the primary surface (DirectDraw's) and the interface
    /// of it the device holds.
    pub front: u32,
    pub front_iface: u32,
    /// The swap chain's back buffers and the automatic depth buffer
    /// (surfaces of Direct3D 8's, implicit).
    pub backbuffers: Vec<u32>,
    pub auto_depth: u32,
    /// The render target and the depth buffer (0 for none).
    pub target: u32,
    pub depth: u32,
    pub in_scene: bool,
    /// The texture stages' states, by Direct3D 8's numbers.
    pub stages: Vec<u32>,
    /// Each stage's texture (Direct3D 8's object), 0 for none.
    pub textures: Vec<u32>,
    /// The streams' vertex buffers and strides.
    pub streams: Vec<(u32, u32)>,
    /// The index buffer and the base vertex index.
    pub indices: (u32, u32),
    /// The vertex shader (an FVF code or a shader's handle) and the
    /// pixel shader (0 for none).
    pub vertex_shader: u32,
    pub pixel_shader: u32,
    pub shaders: Vec<Option<super::shader::Shader>>,
    pub pixel_shaders: Vec<Option<Vec<u32>>>,
    pub constants: Vec<[u32; 4]>,
    pub ps_constants: Vec<[u32; 4]>,
    /// The world matrices past the fourth, which the inner device
    /// hasn't: D3DTS_WORLDMATRIX(n) and the matrix.
    pub worlds: Vec<(u32, [u32; 16])>,
    /// The state blocks, by their handles less 1, and the one being
    /// recorded.
    pub blocks: Vec<Option<Vec<inner_device::Change>>>,
    pub recording: Option<Vec<inner_device::Change>>,
    /// Whether the cursor is shown, and whether it has an image.
    pub cursor_shown: bool,
    pub cursor_set: bool,
    /// The gamma ramp: red, green and blue, 256 each.
    pub gamma: Vec<u16>,
    /// D3DCLIPSTATUS8.
    pub clip_status: [u32; 2],
    /// The palettes, 256 PALETTEENTRYs each, and the current one.
    pub palettes: Vec<Vec<u8>>,
    pub palette: u32,
}

crate::state_fields!(Device {
    d3d,
    dd,
    dd_iface,
    inner,
    inner_iface,
    focus,
    window,
    behavior,
    devtype,
    params,
    front,
    front_iface,
    backbuffers,
    auto_depth,
    target,
    depth,
    in_scene,
    stages,
    textures,
    streams,
    indices,
    vertex_shader,
    pixel_shader,
    shaders,
    pixel_shaders,
    constants,
    ps_constants,
    worlds,
    blocks,
    recording,
    cursor_shown,
    cursor_set,
    gamma,
    clip_status,
    palettes,
    palette
});

pub const STAGES: usize = 8;
pub const STREAMS: usize = 16;
pub const VS_CONSTANTS: usize = 96;
pub const PS_CONSTANTS: usize = 8;

/// The device `id`.
pub fn get(cpu: &mut Cpu, id: u32) -> Option<&mut Device> {
    object(cpu, id)?.dev.as_mut()
}

/// The device of the interface `this`: a device's, or a resource's or
/// swap chain's device. An error if it is none.
pub fn device_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    let (_, id) = of(cpu, this).ok_or(D3DERR_INVALIDCALL)?;
    let o = state(cpu).object(id).ok_or(D3DERR_INVALIDCALL)?;
    let device = if o.dev.is_some() { id } else { o.device };
    state(cpu)
        .object(device)
        .and_then(|o| o.dev.as_ref())
        .ok_or(D3DERR_INVALIDCALL)?;
    Ok(device)
}

/// The device `this` is, with what it has.
pub fn this_device(cpu: &mut Cpu, this: u32) -> Result<(u32, Device), u32> {
    let (vtable, id) = of(cpu, this).ok_or(D3DERR_INVALIDCALL)?;
    if vtable != V_DEVICE8 {
        return Err(D3DERR_INVALIDCALL);
    }
    let d = get(cpu, id).cloned().ok_or(D3DERR_INVALIDCALL)?;
    Ok((id, d))
}

/// The inner device of the device `id`.
pub fn inner(cpu: &mut Cpu, id: u32) -> Option<&mut Inner> {
    let inner = get(cpu, id)?.inner;
    inner_device::get(cpu, inner)
}

/// The DirectDraw surface of the surface `id` of Direct3D 8's.
pub fn dd_surface_of(cpu: &mut Cpu, id: u32) -> u32 {
    state(cpu)
        .object(id)
        .and_then(|o| o.surface.as_ref())
        .map_or(0, |s| s.dd)
}

/// Whether the device `device` draws with the object `id`, or has it as
/// a part of itself: it stays while the device does then.
pub fn uses(cpu: &mut Cpu, device: u32, id: u32) -> bool {
    let Some(d) = state(cpu).object(device).and_then(|o| o.dev.as_ref()) else {
        return false;
    };
    d.target == id
        || d.depth == id
        || d.auto_depth == id
        || d.backbuffers.contains(&id)
        || d.textures.contains(&id)
        || d.streams.iter().any(|s| s.0 == id)
        || d.indices.0 == id
}

/// The object `id` of the device `device` goes: the device forgets it.
pub fn forget(cpu: &mut Cpu, device: u32, id: u32) {
    let Some(d) = get(cpu, device) else {
        return;
    };
    for t in d.textures.iter_mut() {
        if *t == id {
            *t = 0;
        }
    }
    for s in d.streams.iter_mut() {
        if s.0 == id {
            s.0 = 0;
        }
    }
    if d.indices.0 == id {
        d.indices.0 = 0;
    }
    if d.target == id {
        d.target = 0;
    }
    if d.depth == id {
        d.depth = 0;
    }
}

/// The device `id` goes: its objects go with it, and it lets go of its
/// Direct3D device, DirectDraw object and IDirect3D8.
pub fn destroyed(cpu: &mut Cpu, id: u32, o: &Object) {
    let Some(d) = o.dev.clone() else {
        return;
    };
    // What has no references left goes; what the program still has is
    // the device's no more.
    for other in state(cpu).ids(|o| o.device == id && o.container == 0) {
        let Some(x) = object(cpu, other) else {
            continue;
        };
        if x.refs == 0 || x.implicit {
            super::destroy(cpu, other);
        } else {
            x.device = 0;
            x.holds = false;
        }
    }
    if d.inner_iface != 0 {
        ddraw::release(cpu, d.inner_iface);
    }
    if d.front_iface != 0 {
        ddraw::release(cpu, d.front_iface);
    }
    if d.dd_iface != 0 {
        if d.params.get(PP_WINDOWED) == Some(&0) {
            directdraw::restore_display_mode(cpu, &[d.dd_iface]);
            directdraw::set_cooperative_level(cpu, &[d.dd_iface, d.focus, DDSCL_NORMAL]);
        }
        ddraw::release(cpu, d.dd_iface);
    }
    if d.d3d != 0 {
        super::release(cpu, d.d3d);
    }
}

// Making a device.

/// The D3DFORMAT a back buffer of full screen has the display depth of:
/// its bits a pixel.
fn display_bpp(format: u32) -> Option<u32> {
    match format {
        FMT_R5G6B5 | FMT_X1R5G5B5 => Some(16),
        FMT_X8R8G8B8 | FMT_A8R8G8B8 => Some(32),
        _ => None,
    }
}

/// Read D3DPRESENT_PARAMETERS at `at`.
fn read_params(cpu: &mut Cpu, at: u32) -> Option<Vec<u32>> {
    if at == 0 || !cpu.bus.guest_probe(at, PARAMS * 4, true) {
        return None;
    }
    Some(
        (0..PARAMS as u32)
            .map(|i| cpu.bus.guest_read_32(at + 4 * i))
            .collect(),
    )
}

fn write_params(cpu: &mut Cpu, at: u32, params: &[u32]) {
    for (i, w) in params.iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, *w);
    }
}

/// Check presentation parameters, and fill in what they leave to the
/// device: the back buffers' count and, in a window, their size.
fn check_params(cpu: &mut Cpu, p: &mut [u32], window: u32) -> Result<(), u32> {
    let windowed = p[PP_WINDOWED] != 0;
    if p[PP_COUNT] > 3 || p[PP_MULTISAMPLE] > 1 {
        return Err(D3DERR_INVALIDCALL);
    }
    if !(SWAPEFFECT_DISCARD..=SWAPEFFECT_COPY_VSYNC).contains(&p[PP_SWAPEFFECT]) {
        return Err(D3DERR_INVALIDCALL);
    }
    if matches!(p[PP_SWAPEFFECT], SWAPEFFECT_COPY | SWAPEFFECT_COPY_VSYNC) && p[PP_COUNT] > 1 {
        return Err(D3DERR_INVALIDCALL);
    }
    if p[PP_COUNT] == 0 {
        p[PP_COUNT] = 1;
    }
    if windowed {
        if !resource::target_format(p[PP_FORMAT]) {
            return Err(D3DERR_INVALIDCALL);
        }
        if p[PP_WIDTH] == 0 || p[PP_HEIGHT] == 0 {
            let r = cpu.bus.win32.user.screen_client(window);
            if p[PP_WIDTH] == 0 {
                p[PP_WIDTH] = (r.right - r.left).max(1) as u32;
            }
            if p[PP_HEIGHT] == 0 {
                p[PP_HEIGHT] = (r.bottom - r.top).max(1) as u32;
            }
        }
    } else {
        let bpp = display_bpp(p[PP_FORMAT]).ok_or(D3DERR_INVALIDCALL)?;
        let modes = ddraw::display::modes(cpu);
        if !modes.contains(&(p[PP_WIDTH], p[PP_HEIGHT], bpp)) {
            return Err(D3DERR_INVALIDCALL);
        }
    }
    if p[PP_AUTODEPTH] != 0 && !super::direct3d::depth_format(p[PP_DEPTHFORMAT]) {
        return Err(D3DERR_INVALIDCALL);
    }
    Ok(())
}

/// What a swap chain is made of: the primary surface for full screen
/// and its interface, the back buffers and the automatic depth buffer.
struct Chain {
    front: u32,
    front_iface: u32,
    backbuffers: Vec<u32>,
    depth: u32,
}

/// Make the device `id`'s display and swap chain for the parameters `p`:
/// the cooperative level and mode, and the surfaces.
fn build(cpu: &mut Cpu, id: u32, p: &[u32]) -> Result<Chain, u32> {
    let d = get(cpu, id).cloned().ok_or(D3DERR_INVALIDCALL)?;
    let windowed = p[PP_WINDOWED] != 0;
    let mut chain = Chain {
        front: 0,
        front_iface: 0,
        backbuffers: Vec::new(),
        depth: 0,
    };
    let size = (p[PP_WIDTH], p[PP_HEIGHT]);
    let ok = |hr: Option<u32>| match hr {
        Some(0) => Ok(()),
        _ => Err(D3DERR_INVALIDCALL),
    };
    if windowed {
        directdraw::restore_display_mode(cpu, &[d.dd_iface]);
        let window = if p[PP_WINDOW] != 0 {
            p[PP_WINDOW]
        } else {
            d.focus
        };
        ok(directdraw::set_cooperative_level(
            cpu,
            &[d.dd_iface, window, DDSCL_NORMAL],
        ))?;
        for _ in 0..p[PP_COUNT] {
            let (ids, root) = resource::make_dd(cpu, id, Role::Target, size, p[PP_FORMAT], 0)?;
            let s = resource::new_surface(
                cpu,
                id,
                ids[0],
                root,
                p[PP_FORMAT],
                USAGE_RENDERTARGET,
                POOL_DEFAULT,
                true,
            );
            chain.backbuffers.push(s);
        }
    } else {
        let flags = DDSCL_EXCLUSIVE | DDSCL_FULLSCREEN | DDSCL_NOWINDOWCHANGES;
        ok(directdraw::set_cooperative_level(
            cpu,
            &[d.dd_iface, d.focus, flags],
        ))?;
        let bpp = display_bpp(p[PP_FORMAT]).ok_or(D3DERR_INVALIDCALL)?;
        ok(directdraw::set_display_mode(
            cpu,
            &[d.dd_iface, size.0, size.1, bpp],
        ))?;
        let (ids, root) =
            resource::make_dd(cpu, id, Role::Primary(p[PP_COUNT]), size, p[PP_FORMAT], 0)?;
        chain.front = ids[0];
        chain.front_iface = root;
        for &back in &ids[1..] {
            let s = resource::new_surface(
                cpu,
                id,
                back,
                0,
                p[PP_FORMAT],
                USAGE_RENDERTARGET,
                POOL_DEFAULT,
                true,
            );
            chain.backbuffers.push(s);
        }
    }
    if p[PP_AUTODEPTH] != 0 {
        let format = p[PP_DEPTHFORMAT];
        let (ids, root) = resource::make_dd(cpu, id, Role::Depth, size, format, 0)?;
        chain.depth = resource::new_surface(
            cpu,
            id,
            ids[0],
            root,
            format,
            USAGE_DEPTHSTENCIL,
            POOL_DEFAULT,
            true,
        );
    }
    Ok(chain)
}

/// The device `id`'s swap chain goes.
fn drop_chain(cpu: &mut Cpu, id: u32) {
    let Some(d) = get(cpu, id).cloned() else {
        return;
    };
    if let Some(d) = get(cpu, id) {
        d.backbuffers.clear();
        d.auto_depth = 0;
        d.front = 0;
        d.front_iface = 0;
    }
    let mut parts = d.backbuffers.clone();
    if d.auto_depth != 0 {
        parts.push(d.auto_depth);
    }
    for s in parts {
        if let Some(o) = object(cpu, s) {
            o.implicit = false;
        }
        super::let_go(cpu, s);
    }
    if d.front_iface != 0 {
        ddraw::release(cpu, d.front_iface);
    }
}

/// Put the swap chain on the device, its first back buffer the render
/// target and its depth buffer the depth buffer.
fn use_chain(cpu: &mut Cpu, id: u32, chain: Chain, p: Vec<u32>) {
    let target = chain.backbuffers[0];
    let depth = chain.depth;
    if let Some(d) = get(cpu, id) {
        d.front = chain.front;
        d.front_iface = chain.front_iface;
        d.backbuffers = chain.backbuffers;
        d.auto_depth = chain.depth;
        d.params = p;
    }
    set_targets(cpu, id, target, depth);
}

/// Set the device's render target and depth buffer (surfaces of
/// Direct3D 8's), on the inner device too, with the viewport all of the
/// target. The ones before go if nothing has them.
pub fn set_targets(cpu: &mut Cpu, id: u32, target: u32, depth: u32) {
    let Some(d) = get(cpu, id) else {
        return;
    };
    let (old_target, old_depth) = (d.target, d.depth);
    d.target = target;
    d.depth = depth;
    let inner_id = d.inner;
    let target_dd = dd_surface_of(cpu, target);
    let depth_dd = dd_surface_of(cpu, depth);
    let iface = ddraw::iface(cpu, target_dd, ddraw::V_SURFACE7);
    let (w, h) = ddraw::state(cpu)
        .surface(target_dd)
        .map_or((0, 0), |s| (s.width, s.height));
    let old_iface = inner_device::get(cpu, inner_id).map_or(0, |i| i.target_iface);
    if iface != old_iface {
        ddraw::add_ref(cpu, iface);
    }
    if let Some(i) = inner_device::get(cpu, inner_id) {
        i.target = target_dd;
        i.target_iface = iface;
        i.depth = depth_dd;
        i.viewport = Viewport7 {
            x: 0,
            y: 0,
            width: w,
            height: h,
            min_z: 0.0,
            max_z: 1.0,
        };
    }
    if iface != old_iface && old_iface != 0 {
        ddraw::release(cpu, old_iface);
    }
    for old in [old_target, old_depth] {
        if old != 0 && old != target && old != depth {
            super::let_go(cpu, old);
        }
    }
}

/// CreateDevice(this, adapter, type, focus, flags, params, out) of the
/// IDirect3D8 `d3d`.
#[allow(clippy::too_many_arguments)]
pub fn create(
    cpu: &mut Cpu,
    d3d: u32,
    adapter: u32,
    devtype: u32,
    focus: u32,
    flags: u32,
    params_at: u32,
    out: u32,
) -> u32 {
    if !com::write_out(cpu, out, 0) {
        return D3DERR_INVALIDCALL;
    }
    if adapter != 0 {
        return D3DERR_INVALIDCALL;
    }
    let Some(mut p) = read_params(cpu, params_at) else {
        return D3DERR_INVALIDCALL;
    };
    let vp = flags
        & (CREATE_SOFTWARE_VERTEXPROCESSING
            | CREATE_HARDWARE_VERTEXPROCESSING
            | CREATE_MIXED_VERTEXPROCESSING);
    if vp.count_ones() != 1 {
        return D3DERR_INVALIDCALL;
    }
    match devtype {
        DEVTYPE_HAL => {}
        DEVTYPE_REF => return D3DERR_NOTAVAILABLE,
        _ => return D3DERR_INVALIDCALL,
    }
    // The card has no transform and lighting of its own.
    if vp != CREATE_SOFTWARE_VERTEXPROCESSING {
        return D3DERR_INVALIDCALL;
    }
    // A Voodoo Graphics or Voodoo2 add-on card has no Direct3D 8 driver.
    if cpu.bus.vga.adapter.voodoo_board().is_none() {
        return D3DERR_NOTAVAILABLE;
    }
    let window = if p[PP_WINDOW] != 0 {
        p[PP_WINDOW]
    } else {
        focus
    };
    if !cpu.bus.win32.user.is_window(window) || p[PP_WINDOWED] == 0 && focus == 0 {
        return D3DERR_INVALIDCALL;
    }
    if let Err(e) = check_params(cpu, &mut p, window) {
        return e;
    }

    // The DirectDraw object.
    let dd = directdraw::new_ddraw(cpu, true, false);
    let dd_iface = ddraw::iface(cpu, dd, V_DD7);
    if dd_iface == 0 {
        ddraw::destroy(cpu, dd);
        return com::E_OUTOFMEMORY;
    }
    ddraw::add_ref(cpu, dd_iface);
    let device = Device {
        d3d,
        dd,
        dd_iface,
        focus,
        window,
        behavior: flags,
        devtype,
        params: p.clone(),
        ..Device::default()
    };
    let o = Object {
        dev: Some(device),
        ..Object::default()
    };
    let id = add(cpu, o, V_DEVICE8, 1);
    if id == 0 {
        ddraw::release(cpu, dd_iface);
        return com::E_OUTOFMEMORY;
    }
    super::add_ref(cpu, d3d);
    let chain = match build(cpu, id, &p) {
        Ok(c) => c,
        Err(e) => {
            super::destroy(cpu, id);
            return e;
        }
    };

    // The Direct3D device that draws, on the first back buffer.
    let tmus = super::super::ddraw::d3d::hal(cpu, dd).unwrap_or(1);
    let inner = Inner::new(8, dd, 0, 0, tmus);
    let inner_id = ddraw::add_object(
        cpu,
        ddraw::Object {
            owner: dd,
            d3d: Some(D3dObject {
                device: Some(inner),
                ..D3dObject::default()
            }),
            ..ddraw::Object::default()
        },
    );
    let inner_iface = ddraw::iface(cpu, inner_id, V_DEVICE7);
    ddraw::add_ref(cpu, inner_iface);
    if let Some(d) = get(cpu, id) {
        d.inner = inner_id;
        d.inner_iface = inner_iface;
    }
    hal::created(cpu);
    use_chain(cpu, id, chain, p.clone());
    super::states::defaults(cpu, id);
    write_params(cpu, params_at, &p);
    let iface = super::iface(cpu, id);
    com::write_out(cpu, out, iface);
    D3D_OK
}

// The methods.

/// TestCooperativeLevel(this).
pub fn test_cooperative_level(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(this_device(cpu, args[0]));
    Some(D3D_OK)
}

/// GetAvailableTextureMem(this): the video memory free, as a driver
/// says it, rounded down to a megabyte.
pub fn get_available_texture_mem(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if this_device(cpu, args[0]).is_err() {
        return Some(0);
    }
    let (_, free) = ddraw::display::video_memory(cpu);
    Some(free & !0xF_FFFF)
}

/// ResourceManagerDiscardBytes(this, bytes): managed textures are in
/// system memory, copied to the card as they are drawn with.
pub fn discard_bytes(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(this_device(cpu, args[0]));
    Some(D3D_OK)
}

/// GetDirect3D(this, out).
pub fn get_direct3d(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    if !super::give(cpu, d.d3d, out) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// GetDeviceCaps(this, caps).
pub fn get_device_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    hr!(this_device(cpu, this));
    Some(super::caps::write(cpu, at))
}

/// Write D3DDISPLAYMODE at `at`: the display's mode now.
pub fn write_display_mode(cpu: &mut Cpu, at: u32) -> bool {
    if at == 0 || !cpu.bus.guest_probe(at, 16, true) {
        return false;
    }
    let (w, h, bpp, _) = ddraw::display::current(cpu);
    let (timing, _) = ddraw::display::timing(cpu);
    let hz = timing.hz().round() as u32;
    let words = [w, h, hz, resource::display_format(bpp)];
    for (i, v) in words.iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, *v);
    }
    true
}

/// GetDisplayMode(this, mode).
pub fn get_display_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    hr!(this_device(cpu, this));
    Some(if write_display_mode(cpu, at) {
        D3D_OK
    } else {
        D3DERR_INVALIDCALL
    })
}

/// GetCreationParameters(this, params): D3DDEVICE_CREATION_PARAMETERS.
pub fn get_creation_parameters(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    if at == 0 || !cpu.bus.guest_probe(at, 16, true) {
        return Some(D3DERR_INVALIDCALL);
    }
    for (i, v) in [0, d.devtype, d.focus, d.behavior].iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, *v);
    }
    Some(D3D_OK)
}

/// SetCursorProperties(this, x, y, surface): the cursor's image, which
/// isn't drawn.
pub fn set_cursor_properties(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, _, surface] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, _) = hr!(this_device(cpu, this));
    if surface == 0 || resource::surface_of(cpu, surface).is_err() {
        return Some(D3DERR_INVALIDCALL);
    }
    if let Some(d) = get(cpu, id) {
        d.cursor_set = true;
    }
    Some(D3D_OK)
}

/// SetCursorPosition(this, x, y, flags).
pub fn set_cursor_position(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// ShowCursor(this, show): whether it was shown. It is only with an
/// image.
pub fn show_cursor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok((id, d)) = this_device(cpu, args[0]) else {
        return Some(0);
    };
    if let Some(d) = get(cpu, id) {
        d.cursor_shown = args[1] != 0 && d.cursor_set;
    }
    Some(d.cursor_shown as u32)
}

/// CreateAdditionalSwapChain(this, params, out): in a window.
pub fn create_additional_swap_chain(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    if !com::write_out(cpu, out, 0) {
        return Some(D3DERR_INVALIDCALL);
    }
    let Some(mut p) = read_params(cpu, at) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if p[PP_WINDOWED] == 0 || d.params.get(PP_WINDOWED) == Some(&0) {
        return Some(D3DERR_INVALIDCALL);
    }
    let window = if p[PP_WINDOW] != 0 {
        p[PP_WINDOW]
    } else {
        d.focus
    };
    hr!(check_params(cpu, &mut p, window));
    let chain = Object {
        device: id,
        chain: Some(SwapChain {
            backbuffers: Vec::new(),
            params: p.clone(),
        }),
        ..Object::default()
    };
    let chain_id = add(cpu, chain, V_SWAPCHAIN8, 1);
    if chain_id == 0 {
        return Some(com::E_OUTOFMEMORY);
    }
    let size = (p[PP_WIDTH], p[PP_HEIGHT]);
    for _ in 0..p[PP_COUNT] {
        let made = resource::make_dd(cpu, id, Role::Target, size, p[PP_FORMAT], 0);
        let (ids, root) = match made {
            Ok(m) => m,
            Err(e) => {
                super::release(cpu, chain_id);
                return Some(e);
            }
        };
        let s = resource::new_surface(
            cpu,
            id,
            ids[0],
            root,
            p[PP_FORMAT],
            USAGE_RENDERTARGET,
            POOL_DEFAULT,
            true,
        );
        if let Some(o) = object(cpu, s) {
            o.swapchain = chain_id;
        }
        if let Some(c) = object(cpu, chain_id).and_then(|o| o.chain.as_mut()) {
            c.backbuffers.push(s);
        }
    }
    write_params(cpu, at, &p);
    let iface = super::iface(cpu, chain_id);
    com::write_out(cpu, out, iface);
    Some(D3D_OK)
}

/// Reset(this, params): the swap chain made again, and the state as a
/// new device's. The program has to have let go of every resource in
/// D3DPOOL_DEFAULT first.
pub fn reset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    let Some(mut p) = read_params(cpu, at) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let window = if p[PP_WINDOW] != 0 {
        p[PP_WINDOW]
    } else {
        d.focus
    };
    if p[PP_WINDOWED] == 0 && d.focus == 0 {
        return Some(D3DERR_INVALIDCALL);
    }
    hr!(check_params(cpu, &mut p, window));
    let in_default = state(cpu).ids(|o| {
        o.device == id
            && o.refs > 0
            && o.container == 0
            && !o.implicit
            && (o.surface.as_ref().is_some_and(|s| s.pool == POOL_DEFAULT)
                || o.texture.as_ref().is_some_and(|t| t.pool == POOL_DEFAULT)
                || o.buffer.as_ref().is_some_and(|b| b.pool == POOL_DEFAULT))
    });
    if !in_default.is_empty() {
        return Some(D3DERR_INVALIDCALL);
    }
    super::states::unbind(cpu, id);
    set_targets(cpu, id, 0, 0);
    drop_chain(cpu, id);
    let chain = match build(cpu, id, &p) {
        Ok(c) => c,
        Err(_) => {
            // Back as it was, if it can be.
            if let Ok(c) = build(cpu, id, &d.params) {
                use_chain(cpu, id, c, d.params.clone());
                super::states::defaults(cpu, id);
            }
            return Some(D3DERR_INVALIDCALL);
        }
    };
    if let Some(dev) = get(cpu, id) {
        dev.window = window;
        dev.in_scene = false;
    }
    use_chain(cpu, id, chain, p.clone());
    super::states::defaults(cpu, id);
    write_params(cpu, at, &p);
    Some(D3D_OK)
}

/// The rectangle (left, top, right, bottom) at `at`, if there is one.
fn read_rect(cpu: &mut Cpu, at: u32) -> Option<Rect> {
    if at == 0 || !cpu.bus.guest_probe(at, 16, false) {
        return None;
    }
    Some(Rect::read(cpu, at))
}

/// Present (or a swap chain's Present) of the back buffer `back` (a
/// surface of Direct3D 8's) with the parameters `p`.
#[allow(clippy::too_many_arguments)]
fn present_buffer(
    cpu: &mut Cpu,
    d: &Device,
    back: u32,
    p: &[u32],
    src: u32,
    dst: u32,
    override_window: u32,
) -> Option<u32> {
    let back_dd = dd_surface_of(cpu, back);
    if p[PP_WINDOWED] == 0 {
        // Full screen: the flip chain goes round, at the next frame but
        // for D3DPRESENT_INTERVAL_IMMEDIATE.
        let flags = if p[PP_INTERVAL] == PRESENT_INTERVAL_IMMEDIATE {
            8
        } else {
            1
        };
        let copy = matches!(p[PP_SWAPEFFECT], SWAPEFFECT_COPY | SWAPEFFECT_COPY_VSYNC);
        let hr = dd_surface::flip(cpu, &[d.front_iface, 0, flags])?;
        if hr != 0 {
            return Some(if hr == ddraw::DDERR_WASSTILLDRAWING {
                D3D_OK
            } else {
                D3DERR_INVALIDCALL
            });
        }
        if copy {
            // The back buffer keeps what it had: the front's pixels now.
            super::copy::copy_all(cpu, d.front, back_dd);
        }
        return Some(D3D_OK);
    }
    shadow::expose(cpu, back_dd);
    let window = if override_window != 0 {
        override_window
    } else if p[PP_WINDOW] != 0 {
        p[PP_WINDOW]
    } else {
        d.focus
    };
    let src = read_rect(cpu, src);
    let dst = read_rect(cpu, dst);
    super::copy::present_window(cpu, back_dd, src, dst, window);
    Some(D3D_OK)
}

/// Present(this, source rect, dest rect, window override, dirty region).
pub fn present(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, src, dst, window, _] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    let Some(&back) = d.backbuffers.first() else {
        return Some(D3DERR_INVALIDCALL);
    };
    let p = d.params.clone();
    present_buffer(cpu, &d, back, &p, src, dst, window)
}

/// The back buffer `index` of the device's swap chain or of an
/// additional one, given at `out` with a reference.
fn give_back_buffer(cpu: &mut Cpu, list: &[u32], index: u32, kind: u32, out: u32) -> u32 {
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return D3DERR_INVALIDCALL;
    }
    // D3DBACKBUFFER_TYPE_MONO.
    if kind != 0 {
        return D3DERR_INVALIDCALL;
    }
    let Some(&b) = list.get(index as usize) else {
        return D3DERR_INVALIDCALL;
    };
    super::give(cpu, b, out);
    D3D_OK
}

/// GetBackBuffer(this, index, type, out).
pub fn get_back_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, kind, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    Some(give_back_buffer(cpu, &d.backbuffers, index, kind, out))
}

/// GetRasterStatus(this, status): D3DRASTER_STATUS.
pub fn get_raster_status(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    if at == 0 || !cpu.bus.guest_probe(at, 8, true) {
        return Some(D3DERR_INVALIDCALL);
    }
    let hr = directdraw::get_scan_line(cpu, &[d.dd_iface, at + 4]).unwrap_or(0);
    let blank = hr == ddraw::DDERR_VERTICALBLANKINPROGRESS;
    cpu.bus.guest_write_32(at, blank as u32);
    if blank {
        cpu.bus.guest_write_32(at + 4, 0);
    }
    Some(D3D_OK)
}

/// The identity ramp.
pub fn identity_gamma() -> Vec<u16> {
    (0..768).map(|i| ((i % 256) * 257) as u16).collect()
}

/// SetGammaRamp(this, flags, ramp): kept, not applied.
pub fn set_gamma_ramp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, at] = args[..] else {
        return Some(0);
    };
    let Ok((id, _)) = this_device(cpu, this) else {
        return Some(0);
    };
    if at == 0 || !cpu.bus.guest_probe(at, 1536, false) {
        return Some(0);
    }
    let ramp: Vec<u16> = (0..768)
        .map(|i| cpu.bus.guest_read_16(at + 2 * i))
        .collect();
    if let Some(d) = get(cpu, id) {
        d.gamma = ramp;
    }
    Some(0)
}

/// GetGammaRamp(this, ramp).
pub fn get_gamma_ramp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(0);
    };
    let Ok((_, d)) = this_device(cpu, this) else {
        return Some(0);
    };
    if at == 0 || !cpu.bus.guest_probe(at, 1536, true) {
        return Some(0);
    }
    let ramp = if d.gamma.len() == 768 {
        d.gamma
    } else {
        identity_gamma()
    };
    for (i, v) in ramp.iter().enumerate() {
        cpu.bus.guest_write_16(at + 2 * i as u32, *v);
    }
    Some(0)
}

/// SetRenderTarget(this, target, depth): a NULL target keeps the one
/// there is; a NULL depth buffer draws without one.
pub fn set_render_target(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, target, depth] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    let new_target = if target != 0 {
        let (t, s) = hr!(resource::surface_of(cpu, target));
        if s.usage & USAGE_RENDERTARGET == 0 {
            return Some(D3DERR_INVALIDCALL);
        }
        t
    } else {
        d.target
    };
    let new_depth = if depth != 0 {
        let (z, s) = hr!(resource::surface_of(cpu, depth));
        if s.usage & USAGE_DEPTHSTENCIL == 0 {
            return Some(D3DERR_INVALIDCALL);
        }
        z
    } else {
        0
    };
    let size = |cpu: &mut Cpu, id: u32| {
        state(cpu)
            .object(id)
            .and_then(|o| o.surface.as_ref())
            .map_or((0, 0), |s| (s.width, s.height))
    };
    if new_depth != 0 {
        let (tw, th) = size(cpu, new_target);
        let (zw, zh) = size(cpu, new_depth);
        if zw < tw || zh < th {
            return Some(D3DERR_INVALIDCALL);
        }
    }
    set_targets(cpu, id, new_target, new_depth);
    Some(D3D_OK)
}

/// GetRenderTarget(this, out).
pub fn get_render_target(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    if d.target == 0 {
        com::write_out(cpu, out, 0);
        return Some(D3DERR_NOTFOUND);
    }
    if !super::give(cpu, d.target, out) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// GetDepthStencilSurface(this, out).
pub fn get_depth_stencil_surface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(D3DERR_INVALIDCALL);
    }
    if d.depth == 0 {
        com::write_out(cpu, out, 0);
        return Some(D3DERR_NOTFOUND);
    }
    super::give(cpu, d.depth, out);
    Some(D3D_OK)
}

/// BeginScene(this).
pub fn begin_scene(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, d) = hr!(this_device(cpu, args[0]));
    if d.in_scene {
        return Some(D3DERR_INVALIDCALL);
    }
    if let Some(d) = get(cpu, id) {
        d.in_scene = true;
    }
    Some(D3D_OK)
}

/// EndScene(this): what the card drew is in the render target.
pub fn end_scene(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (id, d) = hr!(this_device(cpu, args[0]));
    if !d.in_scene {
        return Some(D3DERR_INVALIDCALL);
    }
    if let Some(d) = get(cpu, id) {
        d.in_scene = false;
    }
    let target = dd_surface_of(cpu, d.target);
    shadow::expose(cpu, target);
    Some(D3D_OK)
}

// D3DCLEAR_ flags.
const CLEAR_TARGET: u32 = 1;
const CLEAR_ZBUFFER: u32 = 2;
const CLEAR_STENCIL: u32 = 4;

/// Clear(this, count, rects, flags, color, z, stencil): the rectangles
/// (D3DRECTs), or all of the viewport.
pub fn clear(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, count, rects_at, flags, color, z, _] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (id, d) = hr!(this_device(cpu, this));
    if flags & !(CLEAR_TARGET | CLEAR_ZBUFFER | CLEAR_STENCIL) != 0 || flags == 0 {
        return Some(D3DERR_INVALIDCALL);
    }
    if flags & (CLEAR_ZBUFFER | CLEAR_STENCIL) != 0 && d.depth == 0 {
        return Some(D3DERR_INVALIDCALL);
    }
    // The depth buffers have no stencil.
    if flags & CLEAR_STENCIL != 0 {
        return Some(D3DERR_INVALIDCALL);
    }
    let Some(dev) = inner(cpu, id).cloned() else {
        return Some(D3DERR_INVALIDCALL);
    };
    let v = dev.viewport;
    let view = [
        v.x as i32,
        v.y as i32,
        (v.x + v.width) as i32,
        (v.y + v.height) as i32,
    ];
    let mut rects = Vec::new();
    if count > 0 && rects_at != 0 {
        if count > 0x1_0000 || !cpu.bus.guest_probe(rects_at, count as usize * 16, false) {
            return Some(D3DERR_INVALIDCALL);
        }
        for i in 0..count {
            let r: [i32; 4] = std::array::from_fn(|k| {
                cpu.bus.guest_read_32(rects_at + 16 * i + 4 * k as u32) as i32
            });
            let c = [
                r[0].max(view[0]),
                r[1].max(view[1]),
                r[2].min(view[2]),
                r[3].min(view[3]),
            ];
            if c[0] < c[2] && c[1] < c[3] {
                rects.push(c);
            }
        }
        if rects.is_empty() {
            return Some(D3D_OK);
        }
    } else {
        rects.push(view);
    }
    let target = (flags & CLEAR_TARGET != 0).then_some(color);
    let depth = (flags & CLEAR_ZBUFFER != 0).then(|| f32::from_bits(z).clamp(0.0, 1.0));
    hal::clear(cpu, &dev, &rects, target, depth);
    Some(D3D_OK)
}

/// ValidateDevice(this, passes): whether the card draws with the
/// texture stages as they are, in one pass.
pub fn validate_device(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, d) = hr!(this_device(cpu, this));
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) {
        return Some(D3DERR_INVALIDCALL);
    }
    let tmus = super::super::ddraw::d3d::hal(cpu, d.dd).unwrap_or(1) as usize;
    let stage = |s: usize, t: usize| d.stages[s * 32 + t];
    let mut used = 0;
    for s in 0..STAGES {
        let (op, aop) = (stage(s, 1), stage(s, 4));
        if op == 1 {
            break;
        }
        if !hal::can_do(op) {
            return Some(D3DERR_UNSUPPORTEDCOLOROPERATION);
        }
        if aop != 1 && !hal::can_do(aop) {
            return Some(D3DERR_UNSUPPORTEDALPHAOPERATION);
        }
        used += 1;
    }
    if used > tmus {
        return Some(D3DERR_TOOMANYOPERATIONS);
    }
    cpu.bus.guest_write_32(out, 1);
    Some(D3D_OK)
}

/// GetInfo(this, id, info, size): the driver has none of the queries.
pub fn get_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(this_device(cpu, args[0]));
    Some(com::S_FALSE)
}

// IDirect3DSwapChain8.

/// The swap chain `this` is.
fn chain_of(cpu: &mut Cpu, this: u32) -> Result<(u32, SwapChain, u32), u32> {
    let (vtable, id) = of(cpu, this).ok_or(D3DERR_INVALIDCALL)?;
    if vtable != V_SWAPCHAIN8 {
        return Err(D3DERR_INVALIDCALL);
    }
    let o = state(cpu).object(id).cloned().ok_or(D3DERR_INVALIDCALL)?;
    let c = o.chain.ok_or(D3DERR_INVALIDCALL)?;
    Ok((id, c, o.device))
}

/// Present(this, source rect, dest rect, window override, dirty region).
pub fn swap_chain_present(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, src, dst, window, _] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, c, device) = hr!(chain_of(cpu, this));
    let Some(d) = get(cpu, device).cloned() else {
        return Some(D3DERR_INVALIDCALL);
    };
    let Some(&back) = c.backbuffers.first() else {
        return Some(D3DERR_INVALIDCALL);
    };
    present_buffer(cpu, &d, back, &c.params, src, dst, window)
}

/// GetBackBuffer(this, index, type, out).
pub fn swap_chain_get_back_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, kind, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let (_, c, _) = hr!(chain_of(cpu, this));
    Some(give_back_buffer(cpu, &c.backbuffers, index, kind, out))
}
