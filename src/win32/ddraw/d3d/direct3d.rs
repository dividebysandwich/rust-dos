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
use super::hal::{self, Kind, Offer, software};
use super::{
    D3D_OK, D3dObject, IID_HAL, IID_RAMP, IID_RGB, IID_TNLHAL, caps, is_device_guid, probe,
};
use crate::cpu::Cpu;
use crate::win32::com::{self, CLASS_E_NOAGGREGATION, Guid};
use crate::win32::task::{self, Next, Task};
use crate::win32::trace3d;

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
/// The card whose HAL is enumerated (`hal::Kind::code`), `NO_CARD`
/// when a list of devices has none.
const CARD: usize = 6;
/// The version of IDirect3D that lists the devices.
const VERSION: usize = 7;

const NO_CARD: u32 = u32::MAX;

/// A device IDirect3D lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Listed {
    Ramp,
    Rgb,
    Hal,
    TnlHal,
}

impl Listed {
    fn guid(self) -> Guid {
        match self {
            Listed::Ramp => IID_RAMP,
            Listed::Rgb => IID_RGB,
            Listed::Hal => IID_HAL,
            Listed::TnlHal => IID_TNLHAL,
        }
    }

    /// The color model of a software device, 0 for the card's.
    fn model(self) -> u32 {
        match self {
            Listed::Ramp => software::MODEL_MONO,
            Listed::Rgb => software::MODEL_RGB,
            _ => 0,
        }
    }

    /// The color model its description reports: the software device's,
    /// or D3DCOLOR_RGB for the card's.
    fn color_model(self) -> u32 {
        match self.model() {
            0 => software::MODEL_RGB,
            model => model,
        }
    }

    /// What it offers, with the card `card`.
    fn offer(self, card: Option<Offer>) -> Option<Offer> {
        match self {
            Listed::Ramp | Listed::Rgb => Some(hal::software()),
            _ => card,
        }
    }
}

/// The devices IDirect3D of `version` lists, in its order, with the
/// card `card`, as Windows 98's DirectX 6.1 has them: the software
/// devices first (the Ramp device up to Direct3D 5's IDirect3D2, then
/// the RGB one) and the card's HAL device last, after which Direct3D 7
/// lists the T&L HAL device on a card that transforms and lights.
fn listed(card: Option<Kind>, version: u32) -> Vec<Listed> {
    let mut list = Vec::new();
    if version <= 2 {
        list.push(Listed::Ramp);
    }
    list.push(Listed::Rgb);
    if let Some(kind) = card {
        list.push(Listed::Hal);
        if version >= 7 && hal::transforms(kind) {
            list.push(Listed::TnlHal);
        }
    }
    list
}

/// The device of `guid` that IDirect3D of `version` has with the card
/// `card`, if it lists one.
fn device_of_guid(card: Option<Kind>, guid: &Guid, version: u32) -> Option<Listed> {
    listed(card, version)
        .into_iter()
        .find(|l| l.guid() == *guid)
}

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
    offer: Offer,
) -> Option<u32> {
    let card = offer.kind.code();
    start_with(cpu, [kind, callback, context, param, offer.units, card, 0])
}

/// Start a list with the variables kind, callback, context, parameter,
/// texture units, card and version.
fn start_with(cpu: &mut Cpu, vars: [u32; 7]) -> Option<u32> {
    let [kind, callback, context, param, units, card, version] = vars;
    if !callable(cpu, callback) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let vars = vec![kind, callback, context, 0, param, units, card, version];
    task::start(cpu, task::D3D_ENUM, vars, SCRATCH)
}

/// Start listing the devices of IDirect3D of `version` (`listed`).
fn start_devices(cpu: &mut Cpu, dd: u32, vars: [u32; 4], version: u32) -> Option<u32> {
    let [kind, callback, context, param] = vars;
    let card = super::hal::card(cpu, dd);
    let (units, code) = card.map_or((0, NO_CARD), |c| (c.units, c.kind.code()));
    start_with(cpu, [kind, callback, context, param, units, code, version])
}

/// EnumDevices(this, callback, context) of IDirect3D to 3: the devices
/// it lists (`listed`), with D3DDEVICEDESC of the interface's version.
pub fn enum_devices(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, callback, context] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, dd) = hr!(d3d_of(cpu, this));
    let version = d3d_version(vtable);
    let size = match version {
        1 => caps::DESC_V1,
        2 => caps::DESC_V2,
        _ => caps::DESC_V3,
    };
    start_devices(cpu, dd, [DEVICES, callback, context, size], version)
}

/// EnumDevices(this, callback, context) of IDirect3D7.
pub fn enum_devices7(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, callback, context] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (_, dd) = hr!(d3d_of(cpu, this));
    start_devices(cpu, dd, [DEVICES7, callback, context, 0], 7)
}

/// EnumTextureFormats(this, callback, context) of a device: the texture
/// formats, as DDSURFACEDESC for IDirect3DDevice and 2, as
/// DDPIXELFORMAT from 3 on.
pub fn enum_texture_formats(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, callback, context] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, id) = hr!(super::device::device_of(cpu, this));
    let version = super::device::version(vtable);
    let kind = if version < 3 { TEXTURES_DESC } else { TEXTURES };
    let offer = super::device::offer_of(cpu, id);
    start(cpu, kind, callback, context, 0, offer)
}

/// EnumZBufferFormats(this, device, callback, context).
pub fn enum_z_buffer_formats(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, device, callback, context] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, dd) = hr!(d3d_of(cpu, this));
    let Some(guid) = Guid::read(cpu, device) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let version = d3d_version(vtable);
    let card = super::hal::card(cpu, dd);
    let Some(offer) =
        device_of_guid(card.map(|c| c.kind), &guid, version).and_then(|device| device.offer(card))
    else {
        return Some(DDERR_INVALIDPARAMS);
    };
    start(cpu, Z_FORMATS, callback, context, 0, offer)
}

/// The task's step: the next item to the callback, until the list ends
/// or the callback returns D3DENUMRET_CANCEL (0).
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let kind = t.var(KIND);
    let (scratch, step) = (t.scratch, t.step);
    let next = items(cpu, t, value);
    if trace3d::on(cpu) {
        let list = match kind {
            DEVICES | DEVICES7 => "DDRAW.EnumDevices",
            Z_FORMATS => "DDRAW.EnumZBufferFormats",
            _ => "DDRAW.EnumTextureFormats",
        };
        trace3d::enumerated(cpu, list, scratch, step, value, &next, |cpu, args| {
            match kind {
                DEVICES => trace3d::d3d_device_item(cpu, args),
                DEVICES7 => trace3d::d3d_device7_item(cpu, args),
                TEXTURES_DESC => trace3d::surface_desc_item(cpu, args, 0),
                _ => trace3d::pixel_format_item(cpu, args),
            }
        });
    }
    next
}

fn items(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step == 1 && value == 0 {
        return Next::Done(D3D_OK);
    }
    let next = t.var(NEXT) as usize;
    t.set(NEXT, next as u32 + 1);
    let s = t.scratch;
    let (callback, context) = (t.var(CALLBACK), t.var(CONTEXT));
    match t.var(KIND) {
        DEVICES | DEVICES7 => {
            let card = (t.var(CARD) != NO_CARD).then(|| Offer {
                kind: hal::Kind::from_code(t.var(CARD)),
                units: t.var(TMUS),
            });
            let list = listed(card.map(|c| c.kind), t.var(VERSION));
            let Some((device, offer)) = list
                .get(next)
                .and_then(|&device| Some((device, device.offer(card)?)))
            else {
                return Next::Done(D3D_OK);
            };
            let (description, name) = match device {
                Listed::Ramp => (caps::RAMP_DESCRIPTION, caps::RAMP_NAME),
                Listed::Rgb => (caps::RGB_DESCRIPTION, caps::RGB_NAME),
                Listed::Hal => (caps::DESCRIPTION, caps::NAME),
                Listed::TnlHal => (caps::TNL_DESCRIPTION, caps::TNL_NAME),
            };
            cpu.bus.guest_write_bytes(s + STRINGS, description);
            cpu.bus.guest_write_bytes(s + NAME, name);
            if t.var(KIND) == DEVICES7 {
                let tnl = device == Listed::TnlHal;
                caps::write_desc7(cpu, s + HAL_DESC, offer, tnl);
                return Next::call(callback, &[s + STRINGS, s + NAME, s + HAL_DESC, context], 1);
            }
            device.guid().write(cpu, s);
            let size = t.var(PARAM);
            match device.model() {
                0 => {
                    caps::write_desc(cpu, s + HAL_DESC, size, Some(offer));
                    caps::write_desc(cpu, s + HEL_DESC, size, None);
                }
                model => {
                    caps::write_empty_desc(cpu, s + HAL_DESC, size);
                    caps::write_software_desc(cpu, s + HEL_DESC, size, model);
                }
            }
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
            let description = hal::description(hal::Kind::from_code(t.var(CARD)));
            let list = if kind == Z_FORMATS {
                description.z_formats
            } else {
                description.textures
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
const D3DFDS_COLORMODEL: u32 = 0x1;
const D3DFDS_GUID: u32 = 0x2;
const D3DFDS_HARDWARE: u32 = 0x4;

/// FindDevice(this, search, result): the first device that matches the
/// search, the HAL before the software devices, with its descriptions
/// of the size the result has room for. The search may ask for a GUID,
/// for hardware or software, and for color models: D3DCOLOR_MONO (1)
/// and D3DCOLOR_RGB (2) are bits, and a device matches a search that
/// has its model's bit. Wine's d3d.c finds a device on Windows for the
/// model 0xDEADBEEF, which has both bits; a program that asks for RGB
/// gets the HAL or the RGB device, and one that asks for MONO the Ramp
/// device.
pub fn find_device(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, search, result] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let (vtable, dd) = hr!(d3d_of(cpu, this));
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
    let hardware = cpu.bus.guest_read_32(search + 8) != 0;
    let model = cpu.bus.guest_read_32(search + 12);
    let guid = Guid::read(cpu, search + 16);
    let card = super::hal::card(cpu, dd);
    let mut list = listed(card.map(|c| c.kind), d3d_version(vtable));
    list.retain(|&l| l != Listed::TnlHal);
    list.sort_by_key(|l| l.model() != 0);
    let found = list.into_iter().find(|&l| {
        (flags & D3DFDS_GUID == 0 || guid == Some(l.guid()))
            && (flags & D3DFDS_HARDWARE == 0 || hardware == (l.model() == 0))
            && (flags & D3DFDS_COLORMODEL == 0 || model & l.color_model() != 0)
    });
    let Some(device) = found else {
        return Some(DDERR_NOTFOUND);
    };
    device.guid().write(cpu, result + 4);
    match (device.model(), card) {
        (0, Some(offer)) => {
            caps::write_desc(cpu, result + 20, desc, Some(offer));
            caps::write_desc(cpu, result + 20 + desc, desc, None);
        }
        (model, _) => {
            caps::write_empty_desc(cpu, result + 20, desc);
            caps::write_software_desc(cpu, result + 20 + desc, desc, model);
        }
    }
    Some(D3D_OK)
}

/// CreateDevice(this, guid, surface, out[, outer]) of IDirect3D2, 3 and
/// 7. A device GUID the interface doesn't list with this card
/// (`listed`), the HAL's with no card among them, makes it fail as a
/// device that can't be made.
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
    let card = super::hal::card(cpu, dd).map(|o| o.kind);
    let Some(device) = device_of_guid(card, &guid, version) else {
        return Some(com::E_OUTOFMEMORY);
    };
    let tnl = device == Listed::TnlHal;
    let v = match version {
        2 => V_DEVICE2,
        3 => V_DEVICE3,
        _ => V_DEVICE7,
    };
    let model = device.model();
    Some(super::device::create(
        cpu, version, dd, target, v, out, tnl, model,
    ))
}

/// A surface asked for a device by its GUID (QueryInterface): Direct3D
/// 3 and 5's IDirect3DDevice, on the surface as its render target, given
/// at `out`.
pub fn surface_device(cpu: &mut Cpu, id: u32, vtable: usize, iid: &Guid, out: u32) -> u32 {
    let dd = state(cpu).object(id).map_or(0, |o| o.owner);
    let card = super::hal::card(cpu, dd).map(|o| o.kind);
    let Some(device) = device_of_guid(card, iid, 1) else {
        return com::E_OUTOFMEMORY;
    };
    let target = super::super::iface(cpu, id, vtable);
    super::device::create(cpu, 1, dd, target, V_DEVICE1, out, false, device.model())
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
