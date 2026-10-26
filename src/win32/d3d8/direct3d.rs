//! IDirect3D8: Direct3DCreate8's object, which tells of the display
//! adapter, its modes and formats and the HAL device's caps, and makes
//! devices. There is one adapter, and its only device type is the HAL.
//!
//! Render targets and back buffers are 565, X8R8G8B8 or A8R8G8B8 (the
//! 32-bit ones drawn through 16-bit shadows), textures 565, 1555, 4444
//! and X1R5G5B5 as the card samples them, and depth buffers D16, D16
//! lockable or D24X8 (through a 16-bit shadow). The display modes are
//! the adapter's of 16 and 32 bits a pixel.

use super::device::{self, DEVTYPE_HAL, DEVTYPE_REF};
use super::resource::{
    self, FMT_A1R5G5B5, FMT_A4R4G4B4, FMT_A8R8G8B8, FMT_D16, FMT_D16_LOCKABLE, FMT_D24X8,
    FMT_R5G6B5, FMT_X1R5G5B5, FMT_X4R4G4B4, FMT_X8R8G8B8, USAGE_DEPTHSTENCIL, USAGE_RENDERTARGET,
};
use super::{D3D_OK, D3DERR_INVALIDCALL, D3DERR_NOTAVAILABLE, Object, V_DIRECT3D8, add, of};
use crate::cpu::Cpu;

/// The IDirect3D8 `this` is.
fn d3d_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    match of(cpu, this) {
        Some((V_DIRECT3D8, id)) => Ok(id),
        _ => Err(D3DERR_INVALIDCALL),
    }
}

/// Direct3DCreate8(sdk version): the object, or NULL.
pub fn direct3d_create8(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    // The device is DirectDraw's underneath, and DirectDraw's window
    // procedure, in front of the focus window's, is in its image.
    if crate::win32::loader::map_builtin_named(cpu, "DDRAW.DLL").is_none() {
        return Some(0);
    }
    let o = Object {
        d3d: true,
        ..Object::default()
    };
    let id = add(cpu, o, V_DIRECT3D8, 1);
    Some(super::iface(cpu, id))
}

/// RegisterSoftwareDevice(this, function).
pub fn register_software_device(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    hr!(d3d_of(cpu, args[0]));
    Some(D3D_OK)
}

/// GetAdapterCount(this).
pub fn get_adapter_count(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(d3d_of(cpu, args[0]).map_or(0, |_| 1))
}

/// GetAdapterIdentifier(this, adapter, flags, identifier):
/// D3DADAPTER_IDENTIFIER8, as DirectDraw's DDDEVICEIDENTIFIER2 has the
/// adapter.
pub fn get_adapter_identifier(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, adapter, flags, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    hr!(d3d_of(cpu, this));
    // D3DENUM_NO_WHQL_LEVEL is the only flag.
    if adapter != 0 || flags & !2 != 0 || at == 0 || !cpu.bus.guest_probe(at, 1068, true) {
        return Some(D3DERR_INVALIDCALL);
    }
    cpu.bus.guest_fill(at, 1068, 0);
    cpu.bus.guest_write_bytes(at, b"display.drv");
    let adapter = cpu.bus.vga.adapter;
    let name = adapter.describe();
    let name = name.strip_suffix(" (3D)").unwrap_or(name).as_bytes();
    cpu.bus
        .guest_write_bytes(at + 512, &name[..name.len().min(511)]);
    // The driver's version: 4.10.0.1998, Windows 98's.
    cpu.bus.guest_write_32(at + 1024, 1998);
    cpu.bus.guest_write_32(at + 1028, 4 << 16 | 10);
    // The PCI vendor and device.
    let ids = match cpu.bus.voodoo.as_ref() {
        Some(v) if crate::win32::screen::is_card(cpu) => {
            Some(u32::from_le_bytes(std::array::from_fn(|i| {
                v.config_read(i as u8)
            })))
        }
        _ => None,
    };
    if let Some(ids) = ids {
        cpu.bus.guest_write_32(at + 1032, ids & 0xFFFF);
        cpu.bus.guest_write_32(at + 1036, ids >> 16);
    }
    // The device identifier, which is the driver's and the chip's.
    let guid = [
        0xD7B7_1E3E,
        0x11CF_4A29 ^ ids.unwrap_or(0) >> 16,
        0xA000_9A8E,
        0x35C9_FB0B,
    ];
    for (i, w) in guid.iter().enumerate() {
        cpu.bus.guest_write_32(at + 1048 + 4 * i as u32, *w);
    }
    // WHQL: certified.
    if flags & 2 == 0 {
        cpu.bus.guest_write_32(at + 1064, 1);
    }
    Some(D3D_OK)
}

/// The modes Direct3D offers: the adapter's of 16 and 32 bits, by size.
fn modes(cpu: &Cpu) -> Vec<(u32, u32, u32)> {
    super::super::ddraw::display::modes(cpu)
        .into_iter()
        .filter(|m| m.2 == 16 || m.2 == 32)
        .collect()
}

/// GetAdapterModeCount(this, adapter).
pub fn get_adapter_mode_count(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if d3d_of(cpu, args[0]).is_err() || args[1] != 0 {
        return Some(0);
    }
    Some(modes(cpu).len() as u32)
}

/// EnumAdapterModes(this, adapter, mode, out): D3DDISPLAYMODE.
pub fn enum_adapter_modes(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, adapter, mode, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    hr!(d3d_of(cpu, this));
    let list = modes(cpu);
    let Some(&(w, h, bpp)) = list.get(mode as usize) else {
        return Some(D3DERR_INVALIDCALL);
    };
    if adapter != 0 || at == 0 || !cpu.bus.guest_probe(at, 16, true) {
        return Some(D3DERR_INVALIDCALL);
    }
    let words = [w, h, 60, resource::display_format(bpp)];
    for (i, v) in words.iter().enumerate() {
        cpu.bus.guest_write_32(at + 4 * i as u32, *v);
    }
    Some(D3D_OK)
}

/// GetAdapterDisplayMode(this, adapter, out).
pub fn get_adapter_display_mode(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, adapter, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    hr!(d3d_of(cpu, this));
    if adapter != 0 || !device::write_display_mode(cpu, at) {
        return Some(D3DERR_INVALIDCALL);
    }
    Some(D3D_OK)
}

/// Whether the adapter `adapter` has the device type `devtype`: an
/// error if it hasn't.
fn check_device(cpu: &mut Cpu, adapter: u32, devtype: u32) -> Result<(), u32> {
    if adapter != 0 {
        return Err(D3DERR_INVALIDCALL);
    }
    match devtype {
        DEVTYPE_HAL if cpu.bus.vga.adapter.voodoo_board().is_some() => Ok(()),
        DEVTYPE_HAL | DEVTYPE_REF => Err(D3DERR_NOTAVAILABLE),
        _ => Err(D3DERR_INVALIDCALL),
    }
}

/// A display mode's format's bits a pixel.
fn display_bits(format: u32) -> Option<u32> {
    match format {
        FMT_R5G6B5 => Some(16),
        FMT_X8R8G8B8 => Some(32),
        _ => None,
    }
}

/// CheckDeviceType(this, adapter, type, display format, back buffer
/// format, windowed).
pub fn check_device_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, adapter, devtype, display, back, windowed] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    hr!(d3d_of(cpu, this));
    hr!(check_device(cpu, adapter, devtype));
    let Some(bits) = display_bits(display) else {
        return Some(D3DERR_NOTAVAILABLE);
    };
    if !resource::target_format(back) {
        return Some(D3DERR_NOTAVAILABLE);
    }
    let back_bits = if back == FMT_R5G6B5 { 16 } else { 32 };
    if windowed == 0 && back_bits != bits {
        return Some(D3DERR_NOTAVAILABLE);
    }
    Some(D3D_OK)
}

// D3DRESOURCETYPE.
const RTYPE_SURFACE: u32 = 1;
const RTYPE_TEXTURE: u32 = 3;

/// Whether `format` is a depth buffer's the device has.
pub fn depth_format(format: u32) -> bool {
    matches!(format, FMT_D16 | FMT_D16_LOCKABLE | FMT_D24X8)
}

/// CheckDeviceFormat(this, adapter, type, adapter format, usage,
/// resource type, format).
pub fn check_device_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, adapter, devtype, display, usage, rtype, format] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    hr!(d3d_of(cpu, this));
    hr!(check_device(cpu, adapter, devtype));
    if display_bits(display).is_none() {
        return Some(D3DERR_NOTAVAILABLE);
    }
    let surface_or_texture = matches!(rtype, RTYPE_SURFACE | RTYPE_TEXTURE);
    let ok = if usage & USAGE_DEPTHSTENCIL != 0 {
        rtype == RTYPE_SURFACE && depth_format(format)
    } else if usage & USAGE_RENDERTARGET != 0 {
        surface_or_texture && resource::target_format(format)
    } else {
        let texture = matches!(
            format,
            FMT_R5G6B5 | FMT_X1R5G5B5 | FMT_A1R5G5B5 | FMT_A4R4G4B4 | FMT_X4R4G4B4
        );
        let surface = texture || matches!(format, FMT_X8R8G8B8 | FMT_A8R8G8B8);
        match rtype {
            RTYPE_TEXTURE => texture,
            RTYPE_SURFACE => surface,
            _ => false,
        }
    };
    Some(if ok { D3D_OK } else { D3DERR_NOTAVAILABLE })
}

/// CheckDeviceMultiSampleType(this, adapter, type, format, windowed,
/// multisample type): only none.
pub fn check_multi_sample_type(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, adapter, devtype, _, _, kind] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    hr!(d3d_of(cpu, this));
    hr!(check_device(cpu, adapter, devtype));
    Some(if kind == 0 {
        D3D_OK
    } else {
        D3DERR_NOTAVAILABLE
    })
}

/// CheckDepthStencilMatch(this, adapter, type, adapter format, render
/// target format, depth format): any depth buffer goes with any target.
pub fn check_depth_stencil_match(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, adapter, devtype, _, target, depth] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    hr!(d3d_of(cpu, this));
    hr!(check_device(cpu, adapter, devtype));
    Some(if resource::target_format(target) && depth_format(depth) {
        D3D_OK
    } else {
        D3DERR_NOTAVAILABLE
    })
}

/// GetDeviceCaps(this, adapter, type, caps).
pub fn get_device_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, adapter, devtype, at] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    hr!(d3d_of(cpu, this));
    hr!(check_device(cpu, adapter, devtype));
    Some(super::caps::write(cpu, at))
}

/// GetAdapterMonitor(this, adapter).
pub fn get_adapter_monitor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if d3d_of(cpu, args[0]).is_err() || args[1] != 0 {
        return Some(0);
    }
    Some(crate::win32::user32::monitor::MONITOR)
}

/// CreateDevice(this, adapter, type, focus, flags, params, out).
pub fn create_device(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, adapter, devtype, focus, flags, params, out] = args[..] else {
        return Some(D3DERR_INVALIDCALL);
    };
    let id = hr!(d3d_of(cpu, this));
    Some(device::create(
        cpu, id, adapter, devtype, focus, flags, params, out,
    ))
}
