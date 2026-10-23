//! Materials: IDirect3DMaterial to 3, Direct3D 3 to 6's materials. A
//! device names one by a handle (GetHandle), which the light state
//! D3DLIGHTSTATE_MATERIAL and a viewport's background take.

use super::super::{
    DDERR_ALREADYINITIALIZED, DDERR_INVALIDOBJECT, DDERR_INVALIDPARAMS, DDERR_UNSUPPORTED,
};
use super::device::{self, Device, HANDLE_MATERIAL};
use super::pipeline::Material;
use super::{D3D_OK, probe, read_dwords, write_dwords};
use crate::cpu::Cpu;

/// A material: D3DMATERIAL's words after its size (the colors, the
/// power, the texture handle and the ramp size).
#[derive(Clone, Debug, Default)]
pub struct MaterialObject {
    pub data: [u32; 19],
}

crate::state_fields!(MaterialObject { data });

/// D3DMATERIAL's size.
const SIZE: u32 = 80;

fn material_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    let (_, id) = super::of(cpu, this).ok_or(DDERR_INVALIDOBJECT)?;
    super::object(cpu, id)
        .and_then(|o| o.material.as_ref())
        .ok_or(DDERR_INVALIDOBJECT)?;
    Ok(id)
}

/// The material a device's handle `h` names, as the pipeline has it.
pub fn of_handle(cpu: &mut Cpu, d: &Device, h: u32) -> Option<Material> {
    let id = d.handle(h, HANDLE_MATERIAL)?.object;
    let data = super::object(cpu, id)?.material.as_ref()?.data;
    Some(super::draw::material(&data[..17]))
}

/// The material `id` goes: the devices forget its handles.
pub fn destroyed(cpu: &mut Cpu, id: u32) {
    device::forget_handles(cpu, HANDLE_MATERIAL, id);
}

/// Initialize(this, d3d): it was.
pub fn initialize(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DDERR_ALREADYINITIALIZED)
}

/// SetMaterial(this, material).
pub fn set_material(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(material_of(cpu, this));
    if !probe(cpu, at, SIZE, false) || cpu.bus.guest_read_32(at) != SIZE {
        return Some(DDERR_INVALIDPARAMS);
    }
    let data = read_dwords::<19>(cpu, at + 4);
    if let Some(m) = super::object(cpu, id).and_then(|o| o.material.as_mut()) {
        m.data = data;
    }
    Some(D3D_OK)
}

/// GetMaterial(this, material).
pub fn get_material(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(material_of(cpu, this));
    if !probe(cpu, at, SIZE, true) || cpu.bus.guest_read_32(at) != SIZE {
        return Some(DDERR_INVALIDPARAMS);
    }
    let data = super::object(cpu, id)
        .and_then(|o| o.material.as_ref())
        .map_or([0; 19], |m| m.data);
    write_dwords(cpu, at + 4, &data);
    Some(D3D_OK)
}

/// GetHandle(this, device, out): the material's handle on the device.
pub fn get_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, device, out] = args[..] else {
        return Some(DDERR_INVALIDPARAMS);
    };
    let id = hr!(material_of(cpu, this));
    let Ok((_, did)) = device::device_of(cpu, device) else {
        return Some(DDERR_INVALIDPARAMS);
    };
    if !probe(cpu, out, 4, true) {
        return Some(DDERR_INVALIDPARAMS);
    }
    let h = device::get(cpu, did).map_or(0, |d| d.handle_of(HANDLE_MATERIAL, id));
    cpu.bus.guest_write_32(out, h);
    Some(D3D_OK)
}

/// Reserve(this) and Unreserve(this): for the ramp model, which isn't
/// there.
pub fn reserve(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(DDERR_UNSUPPORTED)
}
