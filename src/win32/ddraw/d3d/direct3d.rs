//! IDirect3D to IDirect3D7: the interfaces of a DirectDraw object that
//! list the devices and make Direct3D's objects. The lists (devices,
//! texture and depth buffer formats) call the program back for each
//! item, as a task (task.rs) of their own.

use super::super::surface::{self, Desc};
use super::super::{
    DDERR_ALREADYINITIALIZED, DDERR_INVALIDOBJECT, DDERR_INVALIDPARAMS, DDERR_NOTFOUND, V_D3D1,
    V_D3D2, V_D3D3, V_D3D7, V_DEVICE1, V_DEVICE2, V_DEVICE3, V_DEVICE7, V_LIGHT, V_MATERIAL1,
    V_MATERIAL2, V_MATERIAL3, V_VB1, V_VB7, V_VIEWPORT1, V_VIEWPORT2, V_VIEWPORT3, state,
};
use super::{D3D_OK, D3dObject, IID_HAL, caps, is_device_guid, probe};
use crate::cpu::Cpu;
use crate::win32::com::{self, CLASS_E_NOAGGREGATION, Guid};
use crate::win32::task::{self, Next, Task};

/// The DirectDraw object of the IDirect3D interface `this`: the
/// interface's vtable and the object's id.
fn d3d_of(cpu: &mut Cpu, this: u32) -> Result<(usize, u32), u32> {
    let (vtable, id) = super::super::this(cpu, this).ok_or(DDERR_INVALIDOBJECT)?;
    if state(cpu)
        .object(id)
        .and_then(|o| o.ddraw.as_ref())
        .is_none()
    {
        return Err(DDERR_INVALIDOBJECT);
    }
    Ok((vtable, id))
}

fn d3d_version(vtable: usize) -> u32 {
    match vtable {
        V_D3D1 => 1,
        V_D3D2 => 2,
        V_D3D3 => 3,
        _ => 7,
    }
}

/// Initialize(this, guid): it was.
pub fn initialize(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DDERR_ALREADYINITIALIZED)
}

fn callable(cpu: &mut Cpu, callback: u32) -> bool {
    callback != 0 && crate::win32::kernel32::virtual_memory::allows(cpu, callback, 1, false)
}

/// The kinds of lists (the task's variable 0).
const DEVICES: u32 = 0;
const DEVICES7: u32 = 1;
const TEXTURES_DESC: u32 = 2;
const TEXTURES: u32 = 3;
const Z_FORMATS: u32 = 4;

/// The task's variables: the kind, the callback, its context, where the
/// list is, and the kind's parameter (the size of the device
/// descriptions, or the texture units).
const KIND: usize = 0;
const CALLBACK: usize = 1;
const CONTEXT: usize = 2;
const NEXT: usize = 3;
const PARAM: usize = 4;
const TMUS: usize = 5;

/// The scratch space: the GUID, the strings, and the two descriptions.
const SCRATCH: u32 = 640;
const STRINGS: u32 = 16;
const NAME: u32 = 96;
const HAL_DESC: u32 = 128;
const HEL_DESC: u32 = 384;

fn start(
    cpu: &mut Cpu,
    kind: u32,
    callback: u32,
    context: u32,
    param: u32,
    tmus: u32,
) -> Option<u32> {
    if !callable(cpu, callback) {
        return Some(DDERR_INVALIDPARAMS);
    }
    task::start(
        cpu,
        task::D3D_ENUM,
        vec![kind, callback, context, 0, param, tmus],
        SCRATCH,
    )
}

/// EnumDevices(this, callback, context) of IDirect3D to 3: the HAL, if
/// there is one, with D3DDEVICEDESC of the interface's version.
pub fn enum_devices(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, callback, context] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, dd) = hr!(d3d_of(cpu, this));
    let size = match d3d_version(vtable) {
        1 => caps::DESC_V1,
        2 => caps::DESC_V2,
        _ => caps::DESC_V3,
    };
    let Some(tmus) = super::hal(cpu, dd) else {
        return Some(if callable(cpu, callback) {
            D3D_OK
        } else {
            DDERR_INVALIDPARAMS
        });
    };
    start(cpu, DEVICES, callback, context, size, tmus)
}

/// EnumDevices(this, callback, context) of IDirect3D7.
pub fn enum_devices7(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, callback, context] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, dd) = hr!(d3d_of(cpu, this));
    let Some(tmus) = super::hal(cpu, dd) else {
        return Some(if callable(cpu, callback) {
            D3D_OK
        } else {
            DDERR_INVALIDPARAMS
        });
    };
    start(cpu, DEVICES7, callback, context, 0, tmus)
}

/// EnumTextureFormats(this, callback, context) of a device: the texture
/// formats, as DDSURFACEDESC for IDirect3DDevice and 2, as
/// DDPIXELFORMAT from 3 on.
pub fn enum_texture_formats(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, callback, context] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, _) = hr!(super::device::device_of(cpu, this));
    let version = super::device::version(vtable);
    let kind = if version < 3 { TEXTURES_DESC } else { TEXTURES };
    start(cpu, kind, callback, context, 0, 0)
}

/// EnumZBufferFormats(this, device, callback, context).
pub fn enum_z_buffer_formats(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, device, callback, context] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, dd) = hr!(d3d_of(cpu, this));
    let Some(guid) = Guid::read(cpu, device) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if guid != IID_HAL || super::hal(cpu, dd).is_none() {
        return Some(DDERR_INVALIDPARAMS);
    }
    start(cpu, Z_FORMATS, callback, context, 0, 0)
}

/// The task's step: the next item to the callback, until the list ends
/// or the callback returns D3DENUMRET_CANCEL (0).
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step == 1 && value == 0 {
        return Next::Done(D3D_OK);
    }
    let next = t.var(NEXT) as usize;
    t.set(NEXT, next as u32 + 1);
    let s = t.scratch;
    let (callback, context) = (t.var(CALLBACK), t.var(CONTEXT));
    match t.var(KIND) {
        DEVICES | DEVICES7 => {
            if next > 0 {
                return Next::Done(D3D_OK);
            }
            cpu.bus.guest_write_bytes(s + STRINGS, caps::DESCRIPTION);
            cpu.bus.guest_write_bytes(s + NAME, caps::NAME);
            let tmus = t.var(TMUS);
            if t.var(KIND) == DEVICES7 {
                caps::write_desc7(cpu, s + HAL_DESC, tmus);
                return Next::call(callback, &[s + STRINGS, s + NAME, s + HAL_DESC, context], 1);
            }
            IID_HAL.write(cpu, s);
            let size = t.var(PARAM);
            caps::write_desc(cpu, s + HAL_DESC, size, true, tmus);
            caps::write_desc(cpu, s + HEL_DESC, size, false, tmus);
            let args = [
                s,
                s + STRINGS,
                s + NAME,
                s + HAL_DESC,
                s + HEL_DESC,
                context,
            ];
            Next::call(callback, &args, 1)
        }
        kind => {
            let list = if kind == Z_FORMATS {
                caps::z_formats(cpu)
            } else {
                caps::texture_formats()
            };
            let Some(f) = list.get(next) else {
                return Next::Done(D3D_OK);
            };
            if kind == TEXTURES_DESC {
                let d = Desc {
                    flags: surface::DDSD_CAPS | surface::DDSD_PIXELFORMAT,
                    format: *f,
                    caps: [surface::DDSCAPS_TEXTURE, 0, 0, 0],
                    ..Desc::default()
                };
                d.write(cpu, s, 108);
            } else {
                surface::write_format(cpu, s, f);
            }
            Next::call(callback, &[s, context], 1)
        }
    }
}

/// CreateLight(this, out, outer).
pub fn create_light(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create_object(cpu, args, |_| {
        (
            D3dObject {
                light: Some(super::LightObject::default()),
                ..D3dObject::default()
            },
            V_LIGHT,
        )
    })
}

/// CreateMaterial(this, out, outer).
pub fn create_material(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create_object(cpu, args, |v| {
        (
            D3dObject {
                material: Some(super::MaterialObject::default()),
                ..D3dObject::default()
            },
            match v {
                1 => V_MATERIAL1,
                2 => V_MATERIAL2,
                _ => V_MATERIAL3,
            },
        )
    })
}

/// CreateViewport(this, out, outer).
pub fn create_viewport(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create_object(cpu, args, |v| {
        (
            D3dObject {
                viewport: Some(super::Viewport::default()),
                ..D3dObject::default()
            },
            match v {
                1 => V_VIEWPORT1,
                2 => V_VIEWPORT2,
                _ => V_VIEWPORT3,
            },
        )
    })
}

/// Make an object for (this, out, outer): `make` gives it and its
/// interface by the IDirect3D's version.
fn create_object(
    cpu: &mut Cpu,
    args: &[u32],
    make: impl Fn(u32) -> (D3dObject, usize),
) -> Option<u32> {
    let [this, out, outer] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, dd) = hr!(d3d_of(cpu, this));
    if !com::write_out(cpu, out, 0) {
        return Some(DDERR_INVALIDPARAMS);
    }
    if outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    let (mut object, v) = make(d3d_version(vtable));
    if let Some(l) = object.light.as_mut() {
        l.version = d3d_version(vtable);
    }
    if let Some(vp) = object.viewport.as_mut() {
        vp.version = d3d_version(vtable);
    }
    Some(match super::add(cpu, dd, object, v, out) {
        Ok(_) => D3D_OK,
        Err(e) => e,
    })
}

/// D3DFINDDEVICESEARCH's flags.
const D3DFDS_GUID: u32 = 0x2;
const D3DFDS_HARDWARE: u32 = 0x4;

/// FindDevice(this, search, result): the HAL, if the search asks for
/// nothing else, with its descriptions of the size the result has room
/// for. The color model is not looked at.
pub fn find_device(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, search, result] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, dd) = hr!(d3d_of(cpu, this));
    if !probe(cpu, search, 92, false) || !probe(cpu, result, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let size = cpu.bus.guest_read_32(result);
    let desc = match size {
        364 => caps::DESC_V1,
        428 => caps::DESC_V2,
        524 => caps::DESC_V3,
        _ => return Some(DDERR_INVALIDPARAMS),
    };
    if cpu.bus.guest_read_32(search) != 92 || !probe(cpu, result, size, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let flags = cpu.bus.guest_read_32(search + 4);
    let Some(tmus) = super::hal(cpu, dd) else {
        return Some(DDERR_NOTFOUND);
    };
    if flags & D3DFDS_GUID != 0 && Guid::read(cpu, search + 16) != Some(IID_HAL) {
        return Some(DDERR_NOTFOUND);
    }
    if flags & D3DFDS_HARDWARE != 0 && cpu.bus.guest_read_32(search + 8) == 0 {
        return Some(DDERR_NOTFOUND);
    }
    IID_HAL.write(cpu, result + 4);
    caps::write_desc(cpu, result + 20, desc, true, tmus);
    caps::write_desc(cpu, result + 20 + desc, desc, false, tmus);
    Some(D3D_OK)
}

/// CreateDevice(this, guid, surface, out[, outer]) of IDirect3D2, 3 and
/// 7. A device GUID that isn't the HAL's, or the HAL's with no card to
/// draw with, makes it fail as a device that can't be made.
pub fn create_device(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, guid, target, out) = (args[0], args[1], args[2], args[3]);
    let (vtable, dd) = hr!(d3d_of(cpu, this));
    let version = d3d_version(vtable);
    if !com::write_out(cpu, out, 0) {
        return Some(DDERR_INVALIDPARAMS);
    }
    if args.get(4).is_some_and(|&outer| outer != 0) {
        return Some(CLASS_E_NOAGGREGATION);
    }
    let Some(guid) = Guid::read(cpu, guid) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if !is_device_guid(&guid) {
        return Some(DDERR_INVALIDPARAMS);
    }
    if guid != IID_HAL {
        return Some(com::E_OUTOFMEMORY);
    }
    let v = match version {
        2 => V_DEVICE2,
        3 => V_DEVICE3,
        _ => V_DEVICE7,
    };
    Some(super::device::create(cpu, version, dd, target, v, out))
}

/// A surface asked for a device by its GUID (QueryInterface): Direct3D
/// 3 and 5's IDirect3DDevice, on the surface as its render target, given
/// at `out`.
pub fn surface_device(cpu: &mut Cpu, id: u32, vtable: usize, iid: &Guid, out: u32) -> u32 {
    if *iid != IID_HAL {
        return com::E_OUTOFMEMORY;
    }
    let dd = state(cpu).object(id).map_or(0, |o| o.owner);
    let target = super::super::iface(cpu, id, vtable);
    super::device::create(cpu, 1, dd, target, V_DEVICE1, out)
}

/// CreateVertexBuffer(this, desc, out, flags[, outer]) of IDirect3D3 and
/// 7.
pub fn create_vertex_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, desc, out) = (args[0], args[1], args[2]);
    let (vtable, dd) = hr!(d3d_of(cpu, this));
    if !com::write_out(cpu, out, 0) {
        return Some(DDERR_INVALIDPARAMS);
    }
    if args.get(4).is_some_and(|&outer| outer != 0) {
        return Some(CLASS_E_NOAGGREGATION);
    }
    if !probe(cpu, desc, 16, false) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let [_, caps, fvf, count] = super::read_dwords::<4>(cpu, desc);
    let v = if vtable == V_D3D7 { V_VB7 } else { V_VB1 };
    let buffer = hr!(super::vertexbuffer::make(cpu, caps, fvf, count));
    let object = D3dObject {
        buffer: Some(buffer),
        ..D3dObject::default()
    };
    Some(match super::add(cpu, dd, object, v, out) {
        Ok(_) => D3D_OK,
        Err(e) => e,
    })
}

/// EvictManagedTextures(this).
pub fn evict_managed_textures(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(d3d_of(cpu, args[0]));
    Some(D3D_OK)
}
