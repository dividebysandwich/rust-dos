//! The devices DirectInput has and the objects they are made of: the
//! keyboard's keys, the mouse's axes and buttons, a joystick's axes and
//! buttons; and data formats, which say where each object's value goes
//! in what GetDeviceState fills.
//!
//! The keyboard's objects are its keys, by their DIK codes, which are
//! the scan codes of the 8042 with 80h on for the extended ones (E0).
//! The mouse has two relative axes and three buttons, as the emulated
//! PS/2 mouse has no wheel. A joystick has the axes and buttons its
//! layout on the game port gives it (joystick.rs): X and Y, and Z and
//! Rz for the second stick of a four-axis one; it has no POV.

use super::DIERR_INVALIDPARAM;
use crate::cpu::Cpu;
use crate::win32::com::{E_POINTER, Guid};

/// What a device is: its `kind`.
pub const KEYBOARD: u32 = 1;
pub const MOUSE: u32 = 2;
/// A joystick, with its index on the game port added.
pub const JOYSTICK: u32 = 0x10;

/// The object types (DIDFT_).
pub const DIDFT_RELAXIS: u32 = 0x01;
pub const DIDFT_ABSAXIS: u32 = 0x02;
pub const DIDFT_AXIS: u32 = 0x03;
pub const DIDFT_PSHBUTTON: u32 = 0x04;
pub const DIDFT_BUTTON: u32 = 0x0C;
pub const DIDFT_POV: u32 = 0x10;
const DIDFT_ANYINSTANCE: u32 = 0x00FF_FF00;
const DIDFT_FFACTUATOR: u32 = 0x0100_0000;
const DIDFT_FFEFFECTTRIGGER: u32 = 0x0200_0000;
const DIDFT_OPTIONAL: u32 = 0x8000_0000;

/// DIDATAFORMAT's flags.
pub const DIDF_ABSAXIS: u32 = 1;
pub const DIDF_RELAXIS: u32 = 2;

const DIDOI_ASPECTPOSITION: u32 = 0x100;

const fn guid(d1: u32, d2: u16, d3: u16) -> Guid {
    Guid::new(d1, d2, d3, [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0])
}

pub const GUID_SYS_MOUSE: Guid = guid(0x6F1D_2B60, 0xD5A0, 0x11CF);
pub const GUID_SYS_KEYBOARD: Guid = guid(0x6F1D_2B61, 0xD5A0, 0x11CF);
const GUID_JOYSTICK: Guid = guid(0x6F1D_2B70, 0xD5A0, 0x11CF);
const GUID_X_AXIS: Guid = guid(0xA36D_02E0, 0xC9F3, 0x11CF);
const GUID_Y_AXIS: Guid = guid(0xA36D_02E1, 0xC9F3, 0x11CF);
const GUID_Z_AXIS: Guid = guid(0xA36D_02E2, 0xC9F3, 0x11CF);
const GUID_RZ_AXIS: Guid = guid(0xA36D_02E3, 0xC9F3, 0x11CF);
const GUID_BUTTON: Guid = guid(0xA36D_02F0, 0xC9F3, 0x11CF);
const GUID_POV: Guid = guid(0xA36D_02F2, 0xC9F3, 0x11CF);
const GUID_KEY: Guid = guid(0x5572_8220, 0xD33C, 0x11CF);

/// The instance GUID of the joystick `index`: rust-dos's own, as each
/// Windows installation makes its own.
fn joystick_guid(index: u32) -> Guid {
    guid(0x6F1D_2B71 + index, 0xD5A0, 0x11CF)
}

/// The keys of the keyboard, an enhanced 101-key one with the three
/// Windows keys, by their DIK codes.
static KEYS: &[u8] = &[
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10,
    0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x20,
    0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x2B, 0x2C, 0x2D, 0x2E, 0x2F, 0x30,
    0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E, 0x3F, 0x40,
    0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F, 0x50,
    0x51, 0x52, 0x53, 0x57, 0x58, 0x9C, 0x9D, 0xB5, 0xB7, 0xB8, 0xC5, 0xC7, 0xC8, 0xC9, 0xCB, 0xCD,
    0xCF, 0xD0, 0xD1, 0xD2, 0xD3, 0xDB, 0xDC, 0xDD,
];

/// The place among the keyboard's objects of the key `dik`.
pub fn key_index(dik: u8) -> Option<usize> {
    KEYS.iter().position(|&k| k == dik)
}

/// An object of a device.
#[derive(Clone, Copy, Debug)]
pub struct Obj {
    pub guid: Guid,
    /// Where its value is in the device's own data format (DIMOUSESTATE,
    /// DIJOYSTATE, the 256 keys).
    pub ofs: u32,
    /// Its type and instance (dwType).
    pub ty: u32,
    pub flags: u32,
}

impl Obj {
    pub fn is_axis(&self) -> bool {
        self.ty & DIDFT_AXIS != 0
    }

    pub fn is_button(&self) -> bool {
        self.ty & DIDFT_BUTTON != 0
    }

    pub fn instance(&self) -> u32 {
        self.ty >> 8 & 0xFFFF
    }
}

const fn make(guid: Guid, ofs: u32, ty: u32, instance: u32, flags: u32) -> Obj {
    Obj {
        guid,
        ofs,
        ty: ty | instance << 8,
        flags,
    }
}

/// A joystick's axes, in the order it has them, and where DIJOYSTATE
/// has each.
const JOYSTICK_AXES: [(Guid, u32, &str); 4] = [
    (GUID_X_AXIS, 0, "X Axis"),
    (GUID_Y_AXIS, 4, "Y Axis"),
    (GUID_Z_AXIS, 8, "Z Axis"),
    (GUID_RZ_AXIS, 20, "Z Rotation"),
];

/// The objects of a device of `kind` with `shape` (a joystick's axes and
/// buttons).
pub fn objects(kind: u32, shape: (u32, u32)) -> Vec<Obj> {
    match kind {
        KEYBOARD => KEYS
            .iter()
            .map(|&k| make(GUID_KEY, k as u32, DIDFT_PSHBUTTON, k as u32, 0))
            .collect(),
        MOUSE => {
            let mut list = vec![
                make(GUID_X_AXIS, 0, DIDFT_RELAXIS, 0, 0),
                make(GUID_Y_AXIS, 4, DIDFT_RELAXIS, 1, 0),
            ];
            for b in 0..3 {
                list.push(make(GUID_BUTTON, 12 + b, DIDFT_PSHBUTTON, 3 + b, 0));
            }
            list
        }
        _ => {
            let (axes, buttons) = shape;
            let mut list: Vec<Obj> = JOYSTICK_AXES[..axes.min(4) as usize]
                .iter()
                .enumerate()
                .map(|(i, &(g, ofs, _))| {
                    make(g, ofs, DIDFT_ABSAXIS, i as u32, DIDOI_ASPECTPOSITION)
                })
                .collect();
            for b in 0..buttons.min(32) {
                list.push(make(GUID_BUTTON, 48 + b, DIDFT_PSHBUTTON, b, 0));
            }
            list
        }
    }
}

/// The name of the object `obj` of a device of `kind`.
pub fn object_name(kind: u32, obj: &Obj) -> String {
    match kind {
        KEYBOARD => {
            let dik = obj.ofs as u8;
            let name = match dik {
                0xB7 => Some("Prnt Scrn".to_string()),
                0xC5 => Some("Pause".to_string()),
                0xDB => Some("Left Windows".to_string()),
                0xDC => Some("Right Windows".to_string()),
                0xDD => Some("Application".to_string()),
                _ => crate::win32::user32::input::key_name(dik & 0x7F, dik & 0x80 != 0),
            };
            name.unwrap_or_else(|| format!("Key {}", dik))
        }
        MOUSE => match obj.ofs {
            0 => "X Axis".to_string(),
            4 => "Y Axis".to_string(),
            o => format!("Button {}", o - 12),
        },
        _ if obj.is_axis() => JOYSTICK_AXES
            .iter()
            .find(|a| a.1 == obj.ofs)
            .map_or("Axis", |a| a.2)
            .to_string(),
        _ => format!("Button {}", obj.instance() + 1),
    }
}

/// The devices there are, in the order EnumDevices gives them; for
/// DirectInput 3, which has no joysticks, the keyboard and mouse alone.
pub fn present(cpu: &Cpu, version: u32) -> Vec<u32> {
    let mut kinds = vec![MOUSE, KEYBOARD];
    if version >= 0x500 {
        let joysticks = cpu.bus.joystick.joysticks().len() as u32;
        kinds.extend((0..joysticks).map(|i| JOYSTICK + i));
    }
    kinds
}

/// The device whose instance GUID is `guid`, if it is there.
pub fn kind_of(cpu: &Cpu, guid: &Guid, version: u32) -> Option<u32> {
    present(cpu, version)
        .into_iter()
        .find(|&k| instance_guid(k) == *guid)
}

/// The axes and buttons of a device of `kind`, as it is now.
pub fn shape(cpu: &Cpu, kind: u32) -> (u32, u32) {
    match kind {
        KEYBOARD => (0, KEYS.len() as u32),
        MOUSE => (2, 3),
        _ => cpu
            .bus
            .joystick
            .joysticks()
            .get((kind - JOYSTICK) as usize)
            .copied()
            .unwrap_or((2, 2)),
    }
}

pub fn instance_guid(kind: u32) -> Guid {
    match kind {
        KEYBOARD => GUID_SYS_KEYBOARD,
        MOUSE => GUID_SYS_MOUSE,
        _ => joystick_guid(kind - JOYSTICK),
    }
}

/// DIDEVTYPE_ with its subtype: an enhanced keyboard, a traditional
/// mouse and joystick.
pub fn device_type(kind: u32) -> u32 {
    match kind {
        KEYBOARD => 3 | 4 << 8,
        MOUSE => 2 | 2 << 8,
        _ => 4 | 2 << 8,
    }
}

/// The size of DIDEVICEINSTANCEA, DirectX 5's and 3's.
pub const INSTANCE_SIZE: u32 = 580;
pub const INSTANCE_SIZE_DX3: u32 = 560;

/// The DIDEVICEINSTANCEA of a device of `kind` with `shape`, `size`
/// bytes of it.
pub fn instance(kind: u32, shape: (u32, u32), size: u32) -> Vec<u8> {
    let mut b = vec![0u8; size as usize];
    b[0..4].copy_from_slice(&size.to_le_bytes());
    b[4..20].copy_from_slice(&instance_guid(kind).0);
    let product = match kind {
        KEYBOARD | MOUSE => instance_guid(kind),
        _ => GUID_JOYSTICK,
    };
    b[20..36].copy_from_slice(&product.0);
    b[36..40].copy_from_slice(&device_type(kind).to_le_bytes());
    let (name, product_name) = match kind {
        KEYBOARD => ("Keyboard".to_string(), "Keyboard".to_string()),
        MOUSE => ("Mouse".to_string(), "Mouse".to_string()),
        _ => (
            format!("Joystick {}", kind - JOYSTICK + 1),
            format!("{}-axis, {}-button joystick", shape.0, shape.1),
        ),
    };
    b[40..40 + name.len()].copy_from_slice(name.as_bytes());
    b[300..300 + product_name.len()].copy_from_slice(product_name.as_bytes());
    if size >= INSTANCE_SIZE {
        // No force feedback driver; the HID usage page and usage of the
        // kind of device.
        let usage: u16 = match kind {
            KEYBOARD => 6,
            MOUSE => 2,
            _ => 4,
        };
        b[576..578].copy_from_slice(&1u16.to_le_bytes());
        b[578..580].copy_from_slice(&usage.to_le_bytes());
    }
    b
}

/// The size of DIDEVICEOBJECTINSTANCEA, DirectX 5's and 3's.
pub const OBJECT_SIZE: u32 = 316;
pub const OBJECT_SIZE_DX3: u32 = 292;

/// The DIDEVICEOBJECTINSTANCEA of `obj` of a device of `kind`, where the
/// data format has it at `ofs`, `size` bytes of it.
pub fn object_instance(kind: u32, obj: &Obj, ofs: u32, size: u32) -> Vec<u8> {
    let mut b = vec![0u8; size as usize];
    b[0..4].copy_from_slice(&size.to_le_bytes());
    b[4..20].copy_from_slice(&obj.guid.0);
    b[20..24].copy_from_slice(&ofs.to_le_bytes());
    b[24..28].copy_from_slice(&obj.ty.to_le_bytes());
    b[28..32].copy_from_slice(&obj.flags.to_le_bytes());
    let name = object_name(kind, obj);
    let name = &name.as_bytes()[..name.len().min(259)];
    b[32..32 + name.len()].copy_from_slice(name);
    b
}

/// A data format the program set: how many bytes it has, its flags,
/// where each of the device's objects is in it (`NONE` for those it
/// doesn't have), and the places of the POVs it has that the device
/// doesn't, which are -1.
#[derive(Clone, Debug, Default)]
pub struct Format {
    pub size: u32,
    pub flags: u32,
    pub offsets: Vec<u32>,
    pub povs: Vec<u32>,
}

pub const NONE: u32 = u32::MAX;

impl Format {
    /// The place in `objs` of the object at `ofs`.
    pub fn object_at(&self, ofs: u32) -> Option<usize> {
        self.offsets.iter().position(|&o| o == ofs)
    }
}

/// Whether the format's object of `guid` (None for any) and `ty` names
/// `obj`.
fn names(guid: Option<Guid>, ty: u32, obj: &Obj) -> bool {
    if guid.is_some_and(|g| g != obj.guid) {
        return false;
    }
    let types = ty & 0xFF;
    if types != 0 && obj.ty & types == 0 {
        return false;
    }
    if ty & (DIDFT_FFACTUATOR | DIDFT_FFEFFECTTRIGGER) != 0 {
        return false;
    }
    let instance = ty & DIDFT_ANYINSTANCE;
    instance == DIDFT_ANYINSTANCE || instance >> 8 == obj.instance()
}

/// Read the DIDATAFORMAT at `at` for a device with the objects `objs`:
/// each of its objects takes the first of the device's that it names
/// and no other took. One the device lacks fails the format unless it
/// is optional.
pub fn read_format(cpu: &mut Cpu, at: u32, objs: &[Obj]) -> Result<Format, u32> {
    if at == 0 {
        return Err(E_POINTER);
    }
    if !cpu.bus.guest_probe(at, 24, false) {
        return Err(DIERR_INVALIDPARAM);
    }
    let w: Vec<u32> = (0..6).map(|i| cpu.bus.guest_read_32(at + 4 * i)).collect();
    let (size, obj_size, flags, data_size, count, list) = (w[0], w[1], w[2], w[3], w[4], w[5]);
    if size != 24 || obj_size != 16 || data_size % 4 != 0 || count > 1024 {
        return Err(DIERR_INVALIDPARAM);
    }
    if flags & !(DIDF_ABSAXIS | DIDF_RELAXIS) != 0 || flags == DIDF_ABSAXIS | DIDF_RELAXIS {
        return Err(DIERR_INVALIDPARAM);
    }
    if count > 0 && !cpu.bus.guest_probe(list, 16 * count as usize, false) {
        return Err(DIERR_INVALIDPARAM);
    }
    let mut format = Format {
        size: data_size,
        flags,
        offsets: vec![NONE; objs.len()],
        povs: Vec::new(),
    };
    for i in 0..count {
        let o: Vec<u32> = (0..4)
            .map(|k| cpu.bus.guest_read_32(list + 16 * i + 4 * k))
            .collect();
        let (guid_at, ofs, ty) = (o[0], o[1], o[2]);
        let guid = if guid_at == 0 {
            None
        } else {
            Some(Guid::read(cpu, guid_at).ok_or(DIERR_INVALIDPARAM)?)
        };
        let found =
            (0..objs.len()).find(|&k| format.offsets[k] == NONE && names(guid, ty, &objs[k]));
        let pov = ty & DIDFT_POV != 0 || guid == Some(GUID_POV);
        let wide = match found {
            Some(k) => !objs[k].is_button(),
            None => ty & (DIDFT_AXIS | DIDFT_POV) != 0 || pov,
        };
        let bytes = if wide { 4 } else { 1 };
        if ofs.checked_add(bytes).is_none_or(|end| end > data_size) || (wide && ofs % 4 != 0) {
            return Err(DIERR_INVALIDPARAM);
        }
        match found {
            Some(k) => format.offsets[k] = ofs,
            None if pov => format.povs.push(ofs),
            None if ty & DIDFT_OPTIONAL != 0 => {}
            None => return Err(DIERR_INVALIDPARAM),
        }
    }
    Ok(format)
}
