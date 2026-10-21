//! Viewports: IDirect3DViewport to IDirect3DViewport3, Direct3D 3 to
//! 6's way of saying where on the render target a device draws, and
//! with which lights. A viewport is added to a device, which holds it,
//! and the device's current viewport is the one it draws with.
//!
//! D3DVIEWPORT (Direct3D 3's) scales x/w and y/w from the rectangle's
//! middle; D3DVIEWPORT2 maps a clip volume onto the rectangle. Whichever
//! was set last is the one used; the other reads back with what the two
//! have in common, as Wine's ddraw has it.

use super::super::{
    DDERR_ALREADYINITIALIZED, DDERR_INVALIDOBJECT, DDERR_INVALIDPARAMS, DDERR_UNSUPPORTED, V_LIGHT,
};
use super::device::{self, HANDLE_MATERIAL};
use super::pipeline::{ViewMap, multiply, transform};
use super::{D3D_OK, D3DERR_VIEWPORTHASNODEVICE, probe, read_dwords, write_dwords};
use crate::cpu::Cpu;
use crate::win32::com;

/// A viewport.
#[derive(Clone, Debug, Default)]
pub struct Viewport {
    /// The version of IDirect3D it was made through.
    pub version: u32,
    /// The device it was added to.
    pub device: u32,
    /// Whether D3DVIEWPORT2 was set last, and whether either was.
    pub v2: bool,
    pub set: bool,
    /// The words of D3DVIEWPORT or D3DVIEWPORT2 after the size.
    pub data: [u32; 10],
    /// Its lights, and the interfaces of them it holds.
    pub lights: Vec<(u32, u32)>,
    /// The background material's handle, and the background depth
    /// surface's interface.
    pub background: u32,
    pub depth: u32,
}

crate::state_fields!(Viewport {
    version,
    device,
    v2,
    set,
    data,
    lights,
    background,
    depth
});

/// The viewport of the interface `this`: its id.
pub fn viewport_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    let (_, id) = super::of(cpu, this).ok_or(DDERR_INVALIDOBJECT)?;
    get(cpu, id).ok_or(DDERR_INVALIDOBJECT)?;
    Ok(id)
}

fn get(cpu: &mut Cpu, id: u32) -> Option<&mut Viewport> {
    super::object(cpu, id)?.viewport.as_mut()
}

/// How the viewport `id` maps clip coordinates to the screen, if it was
/// set.
pub fn view_map(cpu: &mut Cpu, id: u32) -> Option<ViewMap> {
    let v = get(cpu, id)?.clone();
    if !v.set {
        return None;
    }
    let d = v.data;
    let f = |i: usize| f32::from_bits(d[i]);
    Some(if v.v2 {
        ViewMap::v2(
            [d[0], d[1], d[2], d[3]],
            [f(4), f(5), f(6), f(7)],
            f(8),
            f(9),
        )
    } else {
        ViewMap::v1(d[0], d[1], d[2], d[3], [f(4), f(5)], [f(6), f(7)])
    })
}

/// The viewport `id` goes: it lets go of its lights.
pub fn destroyed(cpu: &mut Cpu, _id: u32, v: Viewport) {
    for (light, iface) in v.lights {
        if let Some(l) = super::object(cpu, light).and_then(|o| o.light.as_mut()) {
            l.viewport = 0;
        }
        super::super::release(cpu, iface);
    }
}

/// Initialize(this, d3d): it was.
pub fn initialize(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DDERR_ALREADYINITIALIZED)
}

/// D3DVIEWPORT and D3DVIEWPORT2's size.
const SIZE: u32 = 44;

/// Read a viewport structure the program gave.
fn read(cpu: &mut Cpu, at: u32) -> Result<[u32; 10], u32> {
    if !probe(cpu, at, SIZE, false) || cpu.bus.guest_read_32(at) != SIZE {
        return Err(DDERR_INVALIDPARAMS);
    }
    Ok(read_dwords::<10>(cpu, at + 4))
}

/// Where a viewport structure is to be written: an error if it can't.
fn check_out(cpu: &mut Cpu, at: u32) -> Result<(), u32> {
    if !probe(cpu, at, SIZE, true) || cpu.bus.guest_read_32(at) != SIZE {
        return Err(DDERR_INVALIDPARAMS);
    }
    Ok(())
}

/// GetViewport(this, viewport): D3DVIEWPORT.
pub fn get_viewport(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(viewport_of(cpu, this));
    hr!(check_out(cpu, at));
    let v = get(cpu, id).cloned().unwrap_or_default();
    let mut d = v.data;
    if v.v2 {
        // The rectangle and depths; no scale or maximum.
        d[4..8].fill(0);
    }
    write_dwords(cpu, at + 4, &d);
    Some(D3D_OK)
}

/// SetViewport(this, viewport): D3DVIEWPORT, whose depths are 0 to 1
/// whatever it says.
pub fn set_viewport(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(viewport_of(cpu, this));
    let mut d = hr!(read(cpu, at));
    d[8] = 0.0f32.to_bits();
    d[9] = 1.0f32.to_bits();
    if let Some(v) = get(cpu, id) {
        v.data = d;
        v.v2 = false;
        v.set = true;
    }
    Some(D3D_OK)
}

/// GetViewport2(this, viewport): D3DVIEWPORT2.
pub fn get_viewport2(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(viewport_of(cpu, this));
    hr!(check_out(cpu, at));
    let v = get(cpu, id).cloned().unwrap_or_default();
    let mut d = v.data;
    if !v.v2 {
        // The rectangle and depths; no clip volume.
        d[4..8].fill(0);
    }
    write_dwords(cpu, at + 4, &d);
    Some(D3D_OK)
}

/// SetViewport2(this, viewport): D3DVIEWPORT2.
pub fn set_viewport2(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(viewport_of(cpu, this));
    let d = hr!(read(cpu, at));
    if let Some(v) = get(cpu, id) {
        v.data = d;
        v.v2 = true;
        v.set = true;
    }
    Some(D3D_OK)
}

// D3DTRANSFORM_ and D3DCLIP_ flags.
const D3DTRANSFORM_CLIPPED: u32 = 0x1;
const D3DTRANSFORM_UNCLIPPED: u32 = 0x2;
const D3DCLIP_LEFT: u32 = 0x1;
const D3DCLIP_RIGHT: u32 = 0x2;
const D3DCLIP_TOP: u32 = 0x4;
const D3DCLIP_BOTTOM: u32 = 0x8;
const D3DCLIP_FRONT: u32 = 0x10;
const D3DCLIP_BACK: u32 = 0x20;

/// TransformVertices(this, count, data, flags, offscreen): transform
/// D3DTRANSFORMDATA's vertices with the device's matrices onto the
/// viewport. With D3DTRANSFORM_CLIPPED each also gets its homogeneous
/// coordinates and clip flags, and a vertex outside is left as its clip
/// coordinates; the flags of a single vertex are given at `offscreen`
/// (ported from Wine's viewport.c).
pub fn transform_vertices(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, count, data, flags, offscreen] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(viewport_of(cpu, this));
    if flags & (D3DTRANSFORM_CLIPPED | D3DTRANSFORM_UNCLIPPED) == 0 {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(v) = get(cpu, id).cloned() else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let Some(t) = device::get(cpu, v.device).map(|d| d.transforms.clone()) else {
        return Some(D3DERR_VIEWPORTHASNODEVICE);
    };
    if !probe(cpu, data, 52, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let [_, input, in_size, output, out_size, h_out] = read_dwords::<6>(cpu, data);
    let m = multiply(&multiply(&t[1], &t[2]), &t[3]);
    let d = v.data;
    let f = |i: usize| f32::from_bits(d[i]);
    let (width, height) = (d[2] as f32, d[3] as f32);
    let (scale_x, scale_y) = if v.v2 {
        let s = |size: f32, clip: f32| if clip != 0.0 { size / clip } else { size };
        (s(width, f(6)), s(height, f(7)))
    } else {
        (f(4), f(5))
    };
    let clipped = flags & D3DTRANSFORM_CLIPPED != 0;
    let mut last_flags = 0;
    for i in 0..count {
        let src = input.wrapping_add(i.wrapping_mul(in_size));
        let dst = output.wrapping_add(i.wrapping_mul(out_size));
        let h = h_out.wrapping_add(16 * i);
        if !probe(cpu, src, 12, false)
            || !probe(cpu, dst, 16, true)
            || clipped && !probe(cpu, h, 16, true)
        {
            return Some(DDERR_INVALIDPARAMS);
        }
        let p = read_dwords::<3>(cpu, src).map(f32::from_bits);
        let [x, y, z, w] = transform([p[0], p[1], p[2], 1.0], &m);
        if clipped {
            let mut c = 0;
            if x * scale_x > width * 0.5 {
                c |= D3DCLIP_RIGHT;
            }
            if x * scale_x <= -width * 0.5 {
                c |= D3DCLIP_LEFT;
            }
            if y * scale_y > height * 0.5 {
                c |= D3DCLIP_TOP;
            }
            if y * scale_y <= -height * 0.5 {
                c |= D3DCLIP_BOTTOM;
            }
            if z < 0.0 {
                c |= D3DCLIP_FRONT;
            }
            if z > 1.0 {
                c |= D3DCLIP_BACK;
            }
            write_dwords(cpu, h, &[c, x.to_bits(), y.to_bits(), z.to_bits()]);
            last_flags = c;
            if c != 0 {
                write_dwords(cpu, dst, &[x, y, z, w].map(f32::to_bits));
                continue;
            }
        }
        let rhw = 1.0 / w;
        let out = [
            (d[2] / 2 + d[0]) as f32 + x * rhw * scale_x,
            (d[3] / 2 + d[1]) as f32 - y * rhw * scale_y,
            z * rhw,
            rhw,
        ];
        write_dwords(cpu, dst, &out.map(f32::to_bits));
    }
    if count == 1 && clipped {
        com::write_out(cpu, offscreen, last_flags);
    } else if probe(cpu, offscreen, 4, true) && cpu.bus.guest_read_32(offscreen) != 0 {
        cpu.bus.guest_write_32(offscreen, 0);
    }
    Some(D3D_OK)
}

/// LightElements(this, count, data): Direct3D never had it.
pub fn light_elements(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DDERR_UNSUPPORTED)
}

/// SetBackground(this, material): a material handle of a device of the
/// viewport's DirectDraw object, or 0.
pub fn set_background(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, h] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(viewport_of(cpu, this));
    if h != 0 {
        let owner = super::super::state(cpu).object(id).map_or(0, |o| o.owner);
        let known = super::super::state(cpu).objects.iter().flatten().any(|o| {
            o.owner == owner
                && o.d3d
                    .as_ref()
                    .and_then(|d| d.device.as_ref())
                    .is_some_and(|d| d.handle(h, HANDLE_MATERIAL).is_some())
        });
        if !known {
            return Some(DDERR_INVALIDPARAMS);
        }
    }
    if let Some(v) = get(cpu, id) {
        v.background = h;
    }
    Some(D3D_OK)
}

/// GetBackground(this, material, valid).
pub fn get_background(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, h, valid] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(viewport_of(cpu, this));
    let b = get(cpu, id).map_or(0, |v| v.background);
    if valid != 0 {
        com::write_out(cpu, valid, (b != 0) as u32);
    }
    if h != 0 {
        com::write_out(cpu, h, b);
    }
    Some(D3D_OK)
}

/// SetBackgroundDepth(this, surface), and SetBackgroundDepth2.
pub fn set_background_depth(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, surface] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(viewport_of(cpu, this));
    if let Some(v) = get(cpu, id) {
        v.depth = surface;
    }
    Some(D3D_OK)
}

/// GetBackgroundDepth(this, out, valid), and GetBackgroundDepth2.
pub fn get_background_depth(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, out, valid] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(viewport_of(cpu, this));
    let depth = get(cpu, id).map_or(0, |v| v.depth);
    if !com::write_out(cpu, out, depth) {
        return Some(DDERR_INVALIDPARAMS);
    }
    if depth != 0 {
        super::super::add_ref(cpu, depth);
    }
    if valid != 0 {
        com::write_out(cpu, valid, (depth != 0) as u32);
    }
    Some(D3D_OK)
}

/// Clear(this, count, rects, flags): the viewport's rectangles to its
/// background material's color and the depth buffer to its far end.
pub fn clear(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, count, rects, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    clear_with(cpu, this, count, rects, flags, None, 1.0)
}

/// Clear2(this, count, rects, flags, color, z, stencil).
pub fn clear2(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, count, rects, flags, color, z, _] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    clear_with(cpu, this, count, rects, flags, Some(color), f32::from_bits(z))
}

/// Clear the rectangles of the viewport's device's render target
/// (D3DCLEAR_TARGET) with `color`, or the background material's
/// diffuse color, and of its depth buffer (D3DCLEAR_ZBUFFER) with `z`.
fn clear_with(
    cpu: &mut Cpu,
    this: u32,
    count: u32,
    rects: u32,
    flags: u32,
    color: Option<u32>,
    z: f32,
) -> Option<u32> {
    let id = hr!(viewport_of(cpu, this));
    let Some(v) = get(cpu, id).cloned().filter(|v| v.device != 0) else {
        return Some(D3DERR_VIEWPORTHASNODEVICE);
    };
    if count != 0 && !probe(cpu, rects, count.saturating_mul(16), false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let Some(d) = super::device::get(cpu, v.device).cloned() else {
        return Some(D3D_OK);
    };
    let color = color.unwrap_or_else(|| {
        super::material::of_handle(cpu, &d, v.background)
            .map_or(0, |m| super::pipeline::color(m.diffuse))
    });
    let list = super::device::read_rects(cpu, rects, count);
    let target = (flags & super::device::CLEAR_TARGET != 0).then_some(color);
    let depth = (flags & super::device::CLEAR_ZBUFFER != 0).then_some(z);
    super::hal::clear(cpu, &d, &list, target, depth);
    Some(D3D_OK)
}

/// AddLight(this, light): up to 8 lights a viewport.
pub fn add_light(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, light] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(viewport_of(cpu, this));
    let Ok(lid) = super::light::light_of(cpu, light) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if super::object(cpu, lid)
        .and_then(|o| o.light.as_ref())
        .is_some_and(|l| l.viewport != 0)
    {
        return Some(super::D3DERR_LIGHTHASVIEWPORT);
    }
    if get(cpu, id).is_none_or(|v| v.lights.len() >= 8) {
        return Some(DDERR_INVALIDPARAMS);
    }
    super::super::add_ref(cpu, light);
    if let Some(v) = get(cpu, id) {
        v.lights.push((lid, light));
    }
    if let Some(l) = super::object(cpu, lid).and_then(|o| o.light.as_mut()) {
        l.viewport = id;
    }
    Some(D3D_OK)
}

/// DeleteLight(this, light).
pub fn delete_light(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, light] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(viewport_of(cpu, this));
    let Ok(lid) = super::light::light_of(cpu, light) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let Some(v) = get(cpu, id) else {
        return Some(DDERR_INVALIDOBJECT);
    };
    let Some(i) = v.lights.iter().position(|l| l.0 == lid) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, held) = v.lights.remove(i);
    if let Some(l) = super::object(cpu, lid).and_then(|o| o.light.as_mut()) {
        l.viewport = 0;
    }
    super::super::release(cpu, held);
    Some(D3D_OK)
}

/// NextLight(this, light, out, flags): D3DNEXT_NEXT (1), HEAD (2) or
/// TAIL (4), with a reference.
pub fn next_light(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, light, out, flags] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(viewport_of(cpu, this));
    if !com::write_out(cpu, out, 0) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let list = get(cpu, id).map(|v| v.lights.clone()).unwrap_or_default();
    let next = match flags {
        2 => list.first().copied(),
        4 => list.last().copied(),
        1 => {
            let Ok(lid) = super::light::light_of(cpu, light) else {
                return Some(DDERR_INVALIDPARAMS);
            };
            list.iter()
                .position(|l| l.0 == lid)
                .and_then(|i| list.get(i + 1).copied())
        }
        _ => None,
    };
    match next {
        Some((lid, _)) => Some(super::super::give(cpu, lid, V_LIGHT, out)),
        None => Some(DDERR_INVALIDPARAMS),
    }
}
