//! A DirectInput device and the methods of IDirectInputDevice7A, which
//! IDirectInputDeviceA's and IDirectInputDevice2A's are the start of.
//!
//! A device is acquired by Acquire, after SetDataFormat, as
//! SetCooperativeLevel allows: in the foreground only while its window
//! is the foreground window, and exclusively only while no other device
//! object has the same device exclusively. A foreground device loses
//! acquisition when its window stops being the foreground window
//! (poll.rs). GetDeviceState fills the program's data format with the
//! device's state as it is now; GetDeviceData takes the buffered data
//! (DIPROP_BUFFERSIZE), each event with the emulated clock's
//! milliseconds and a sequence number, which events that happened
//! together share.
//!
//! The devices have no force feedback: CreateEffect fails with
//! DIERR_UNSUPPORTED and EnumEffects finds nothing, as for such a device
//! on Windows.

use super::objects::{
    self, DIDF_ABSAXIS, DIDF_RELAXIS, Format, JOYSTICK, KEYBOARD, MOUSE, NONE, Obj,
};
use super::poll;
use super::{
    DEVICE, DI_NOEFFECT, DI_OK, DIERR_ACQUIRED, DIERR_DEVICENOTREG, DIERR_INPUTLOST,
    DIERR_INVALIDPARAM, DIERR_NOTACQUIRED, DIERR_NOTBUFFERED, DIERR_NOTFOUND, DIERR_NOTINITIALIZED,
    DIERR_OTHERAPPHASPRIO, DIERR_READONLY, DIERR_UNSUPPORTED, E_HANDLE, check_version, is_eight,
    new_object, object_of, release_object, result, state, version_of,
};
use crate::cpu::Cpu;
use crate::win32::com::{self, E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, Guid};
use crate::win32::object::{self, Kind};

const fn device_iid(d1: u32) -> Guid {
    Guid::new(
        d1,
        0xC92E,
        0x11CF,
        [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0],
    )
}

pub const IID_IDIRECT_INPUT_DEVICE_A: Guid = device_iid(0x5944_E680);
const IID_IDIRECT_INPUT_DEVICE_2A: Guid = device_iid(0x5944_E682);
const IID_IDIRECT_INPUT_DEVICE_7A: Guid = Guid::new(
    0x57D7_C6BC,
    0x2356,
    0x11D3,
    [0x8E, 0x9D, 0x00, 0xC0, 0x4F, 0x68, 0x44, 0xAE],
);
pub const IID_IDIRECT_INPUT_DEVICE_8A: Guid = Guid::new(
    0x54D4_1080,
    0xDC15,
    0x4833,
    [0xA4, 0x1B, 0x74, 0x8F, 0x73, 0xA3, 0x81, 0x79],
);

/// Whether `iid` is one of a device's interfaces: DirectInput 8's
/// devices (`eight`) have IDirectInputDevice8A alone, DINPUT's the
/// interfaces before it.
pub fn is_device_iid(iid: &Guid, eight: bool) -> bool {
    if eight {
        return [com::IID_IUNKNOWN, IID_IDIRECT_INPUT_DEVICE_8A].contains(iid);
    }
    [
        com::IID_IUNKNOWN,
        IID_IDIRECT_INPUT_DEVICE_A,
        IID_IDIRECT_INPUT_DEVICE_2A,
        IID_IDIRECT_INPUT_DEVICE_7A,
    ]
    .contains(iid)
}

/// SetCooperativeLevel's flags.
pub const DISCL_EXCLUSIVE: u32 = 0x01;
pub const DISCL_NONEXCLUSIVE: u32 = 0x02;
pub const DISCL_FOREGROUND: u32 = 0x04;
pub const DISCL_BACKGROUND: u32 = 0x08;
const DISCL_NOWINKEY: u32 = 0x10;

/// The properties, by their numbers (MAKEDIPROP).
const DIPROP_BUFFERSIZE: u32 = 1;
const DIPROP_AXISMODE: u32 = 2;
const DIPROP_GRANULARITY: u32 = 3;
const DIPROP_RANGE: u32 = 4;
const DIPROP_DEADZONE: u32 = 5;
const DIPROP_SATURATION: u32 = 6;
const DIPROP_FFGAIN: u32 = 7;
const DIPROP_FFLOAD: u32 = 8;
const DIPROP_AUTOCENTER: u32 = 9;
const DIPROP_CALIBRATIONMODE: u32 = 10;
const DIPROP_INSTANCENAME: u32 = 13;
const DIPROP_PRODUCTNAME: u32 = 14;
const DIPROP_JOYSTICKID: u32 = 15;
/// DirectInput 8's, which DirectInput 7 doesn't know.
const DIPROP_KEYNAME: u32 = 20;
const DIPROP_APPDATA: u32 = 22;
const DIPROP_SCANCODE: u32 = 23;
const DIPROP_VIDPID: u32 = 24;
const DIPROP_USERNAME: u32 = 25;
const DIPROP_TYPENAME: u32 = 26;

const DIPH_DEVICE: u32 = 0;
const DIPH_BYOFFSET: u32 = 1;
const DIPH_BYID: u32 = 2;
const DIPH_BYUSAGE: u32 = 3;

const DIPROPRANGE_NOMIN: u32 = 0x8000_0000;
const DIPROPRANGE_NOMAX: u32 = 0x7FFF_FFFF;

/// The sizes of the property structures: DIPROPDWORD, DIPROPRANGE,
/// DIPROPSTRING and DIPROPPOINTER.
const PROP_DWORD: u32 = 20;
const PROP_RANGE: u32 = 24;
const PROP_STRING: u32 = 16 + 2 * 260;
const PROP_POINTER: u32 = 20;

/// An object's DIPROP_APPDATA until the program sets one, which its
/// buffered data has as uAppData.
const NO_APP_DATA: u32 = u32::MAX;

/// The sizes of DIDEVCAPS, DirectX 5's and 3's.
const CAPS_SIZE: u32 = 44;
const CAPS_SIZE_DX3: u32 = 24;

/// DIDEVCAPS's flags.
const DIDC_ATTACHED: u32 = 0x01;
const DIDC_POLLEDDEVICE: u32 = 0x02;

/// GetDeviceData's flag that leaves the data in the buffer.
const DIGDD_PEEK: u32 = 1;

/// The most events a buffer holds, whatever DIPROP_BUFFERSIZE says.
const MAX_BUFFER: u32 = 1 << 16;

/// A joystick axis: its range, dead zone and saturation (DIPROP_RANGE,
/// DIPROP_DEADZONE, DIPROP_SATURATION), the value it last reported as
/// buffered data, and in relative mode the value GetDeviceState last
/// gave.
#[derive(Clone, Debug, Default)]
pub struct Axis {
    pub min: i32,
    pub max: i32,
    pub deadzone: u32,
    pub saturation: u32,
    pub last: i32,
    pub base: i32,
}

/// A buffered event: DIDEVICEOBJECTDATA, with the object's
/// DIPROP_APPDATA when the event happened.
#[derive(Clone, Debug, Default)]
pub struct Event {
    pub ofs: u32,
    pub data: u32,
    pub time: u32,
    pub sequence: u32,
    pub app_data: u32,
}

/// A device's state.
#[derive(Clone, Debug, Default)]
pub struct Device {
    /// The DirectInput object that made it, by its id, or 0.
    pub parent: u32,
    /// What it is (objects.rs), 0 before Initialize.
    pub kind: u32,
    /// A joystick's axes and buttons.
    pub shape: (u32, u32),
    /// SetCooperativeLevel's flags and window.
    pub coop: u32,
    pub hwnd: u32,
    pub acquired: bool,
    /// Whether it lost acquisition in exclusive mode since the program
    /// last asked for its data: DIERR_INPUTLOST then.
    pub lost: bool,
    pub format: Option<Format>,
    /// The buffer: its size and the events in it, and whether events
    /// were lost since GetDeviceData last took some.
    pub buffer_size: u32,
    pub queue: Vec<Event>,
    pub overflow: bool,
    /// SetEventNotification's event, by its object, or 0.
    pub event: u32,
    /// Whether its axes report relative motion (DIPROP_AXISMODE).
    pub relative: bool,
    pub axes: Vec<Axis>,
    /// The mouse's motion since GetDeviceState last read it, and where
    /// its motion took it since the device was made, in absolute mode.
    pub moved: [i32; 2],
    pub position: [i32; 2],
    /// A joystick's buttons as last reported, bit n for button n.
    pub buttons: u32,
    pub autocenter: bool,
    pub calibration: u32,
    /// The objects' DIPROP_APPDATA, by their places, that the program
    /// set since its last SetDataFormat; those past the end have none.
    pub app_data: Vec<u32>,
}

impl Device {
    /// A device of `kind` with `shape`, made by the DirectInput object
    /// `parent`: non-exclusive in the background, as without
    /// SetCooperativeLevel.
    pub fn new(parent: u32, kind: u32, shape: (u32, u32)) -> Device {
        let axes = if kind >= JOYSTICK { shape.0 } else { 0 };
        Device {
            parent,
            kind,
            shape,
            coop: DISCL_NONEXCLUSIVE | DISCL_BACKGROUND,
            hwnd: crate::win32::user32::DESKTOP,
            relative: kind == MOUSE,
            axes: (0..axes)
                .map(|_| Axis {
                    min: 0,
                    max: 0xFFFF,
                    deadzone: 0,
                    saturation: 10000,
                    ..Axis::default()
                })
                .collect(),
            autocenter: true,
            ..Device::default()
        }
    }

    pub fn objects(&self) -> Vec<Obj> {
        objects::objects(self.kind, self.shape)
    }

    /// Where the data format has the object `obj`, by its place.
    fn offset(&self, obj: usize) -> Option<u32> {
        let o = *self.format.as_ref()?.offsets.get(obj)?;
        (o != NONE).then_some(o)
    }

    /// The DIPROP_APPDATA of the object `obj`, by its place.
    fn app_data(&self, obj: usize) -> u32 {
        self.app_data.get(obj).copied().unwrap_or(NO_APP_DATA)
    }

    fn set_app_data(&mut self, obj: usize, value: u32) {
        if self.app_data.len() <= obj {
            self.app_data.resize(obj + 1, NO_APP_DATA);
        }
        self.app_data[obj] = value;
    }

    /// The object `obj` (by its place) changed to `data`: buffered data
    /// for it, if the data format has it and the device has a buffer.
    pub fn report(&mut self, obj: usize, data: u32, time: u32, sequence: u32) {
        let Some(ofs) = self.offset(obj) else {
            return;
        };
        if self.buffer_size == 0 {
            return;
        }
        if self.queue.len() as u32 >= self.buffer_size.min(MAX_BUFFER) {
            // The new event is lost.
            self.overflow = true;
            return;
        }
        let app_data = self.app_data(obj);
        self.queue.push(Event {
            ofs,
            data,
            time,
            sequence,
            app_data,
        });
    }

    /// The mouse moved by (`dx`, `dy`).
    pub fn mouse_moved(&mut self, d: [i32; 2], time: u32, sequence: u32) {
        for (i, &delta) in d.iter().enumerate() {
            if delta == 0 {
                continue;
            }
            self.moved[i] = self.moved[i].wrapping_add(delta);
            self.position[i] = self.position[i].wrapping_add(delta);
            let data = if self.relative {
                delta
            } else {
                self.position[i]
            };
            self.report(i, data as u32, time, sequence);
        }
    }

    /// A joystick's axis values from the game port's counts `counts`,
    /// scaled to each axis's range past its dead zone and saturation.
    pub fn joystick_values(&self, counts: &[u32]) -> Vec<i32> {
        self.axes
            .iter()
            .zip(counts)
            .map(|(a, &c)| scale(a, c))
            .collect()
    }
}

/// The value of the axis `a` with the game port's count `count`: 10 at
/// one end, 450 at the other, 0 for an axis that isn't there, which is
/// in the middle.
fn scale(a: &Axis, count: u32) -> i32 {
    let count = if count == 0 {
        230
    } else {
        count.clamp(10, 450)
    };
    let d = count as i64 - 230;
    // Out from the middle, in ten thousandths of the way to an end.
    let out = d.abs() * 10000 / 220;
    let (dead, sat) = (a.deadzone as i64, (a.saturation as i64).max(1));
    let out = if out <= dead {
        0
    } else if out >= sat || sat <= dead {
        10000
    } else {
        (out - dead) * 10000 / (sat - dead)
    };
    let s = if d < 0 { -out } else { out };
    let (min, max) = (a.min as i64, a.max as i64);
    (min + (s + 10000) * (max - min) / 20000) as i32
}

/// The device `id`.
pub fn dev(cpu: &mut Cpu, id: u32) -> &mut Device {
    state(cpu)
        .object_mut(id)
        .and_then(|o| o.device.as_mut())
        .expect("a device")
}

/// The device whose interface `this` is: its id.
fn device_of(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    object_of(cpu, this, DEVICE).ok_or(E_POINTER)
}

/// The same, once initialized, its acquisition checked against the
/// foreground window.
fn ready(cpu: &mut Cpu, this: u32) -> Result<u32, u32> {
    let id = device_of(cpu, this)?;
    if dev(cpu, id).kind == 0 {
        return Err(DIERR_NOTINITIALIZED);
    }
    poll::check_foreground(cpu);
    Ok(id)
}

/// CoCreateInstance's device, which needs its Initialize.
pub fn create_uninitialized(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    uninitialized(cpu, iid, out, false)
}

/// The same of DINPUT8's.
pub fn create_uninitialized_8(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    uninitialized(cpu, iid, out, true)
}

fn uninitialized(cpu: &mut Cpu, iid: &Guid, out: u32, eight: bool) -> u32 {
    if !is_device_iid(iid, eight) {
        return E_NOINTERFACE;
    }
    let Some(this) = new_object(cpu, DEVICE, eight, 0, Some(Device::default())) else {
        return E_OUTOFMEMORY;
    };
    if !com::write_out(cpu, out, this) {
        release(cpu, &[this]);
        return E_POINTER;
    }
    S_OK
}

const S_OK: u32 = com::S_OK;

/// The device `d` went (its last reference, or its process): what it
/// held goes.
pub fn destroyed(cpu: &mut Cpu, d: Device) {
    if d.event != 0 {
        object::objects(cpu).release(d.event);
    }
    if d.parent != 0 {
        release_object(cpu, d.parent);
    }
}

/// QueryInterface(iid, out).
pub fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, iid, out) = (args[0], args[1], args[2]);
    let r = (|| {
        let id = device_of(cpu, this)?;
        let iid = Guid::read(cpu, iid).ok_or(E_POINTER)?;
        if !com::write_out(cpu, out, 0) {
            return Err(E_POINTER);
        }
        if !is_device_iid(&iid, is_eight(cpu, id)) {
            return Err(E_NOINTERFACE);
        }
        if let Some(o) = state(cpu).object_mut(id) {
            o.refs += 1;
        }
        com::write_out(cpu, out, this);
        Ok(S_OK)
    })();
    result(r)
}

pub fn add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok(id) = device_of(cpu, args[0]) else {
        return Some(0);
    };
    let o = state(cpu).object_mut(id)?;
    o.refs += 1;
    Some(o.refs)
}

pub fn release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok(id) = device_of(cpu, args[0]) else {
        return Some(0);
    };
    Some(release_object(cpu, id))
}

/// GetCapabilities(caps): DIDEVCAPS.
pub fn get_capabilities(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        let id = ready(cpu, args[0])?;
        let at = args[1];
        if at == 0 || !cpu.bus.guest_probe(at, 4, false) {
            return Err(E_POINTER);
        }
        let size = cpu.bus.guest_read_32(at);
        if (size != CAPS_SIZE && size != CAPS_SIZE_DX3)
            || !cpu.bus.guest_probe(at, size as usize, true)
        {
            return Err(DIERR_INVALIDPARAM);
        }
        let version = version_of(cpu, id);
        let d = dev(cpu, id);
        let objs = d.objects();
        let axes = objs.iter().filter(|o| o.is_axis()).count() as u32;
        let buttons = objs.iter().filter(|o| o.is_button()).count() as u32;
        let mut flags = DIDC_ATTACHED;
        if d.kind >= JOYSTICK {
            flags |= DIDC_POLLEDDEVICE;
        }
        let dev_type = objects::device_type(d.kind, version);
        let mut words = vec![size, flags, dev_type, axes, buttons, 0];
        words.resize(size as usize / 4, 0);
        for (i, w) in words.iter().enumerate() {
            cpu.bus.guest_write_32(at + 4 * i as u32, *w);
        }
        Ok(DI_OK)
    })();
    result(r)
}

/// What a property's header names: the device or one of its objects,
/// by its place.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    Device,
    Object(usize),
}

/// The property `prop` and its header at `at`, which must be `size`
/// bytes in all: what it names.
fn header(cpu: &mut Cpu, id: u32, at: u32, size: u32) -> Result<Target, u32> {
    if at == 0 || !cpu.bus.guest_probe(at, 16, false) {
        return Err(DIERR_INVALIDPARAM);
    }
    let w: Vec<u32> = (0..4).map(|i| cpu.bus.guest_read_32(at + 4 * i)).collect();
    let (total, header_size, obj, how) = (w[0], w[1], w[2], w[3]);
    if header_size != 16 || total != size || !cpu.bus.guest_probe(at, size as usize, true) {
        return Err(DIERR_INVALIDPARAM);
    }
    let d = dev(cpu, id);
    match how {
        DIPH_DEVICE if obj == 0 => Ok(Target::Device),
        DIPH_BYOFFSET => d
            .format
            .as_ref()
            .and_then(|f| f.object_at(obj))
            .map(Target::Object)
            .ok_or(DIERR_NOTFOUND),
        DIPH_BYID => find_by_id(&d.objects(), obj)
            .map(Target::Object)
            .ok_or(DIERR_NOTFOUND),
        // Usages are HID's, and the emulated devices aren't HID devices.
        DIPH_BYUSAGE => Err(DIERR_UNSUPPORTED),
        _ => Err(DIERR_INVALIDPARAM),
    }
}

/// The object whose dwType is `ty`, by its place.
fn find_by_id(objs: &[Obj], ty: u32) -> Option<usize> {
    objs.iter()
        .position(|o| o.instance() == ty >> 8 & 0xFFFF && o.ty & ty & 0xFF != 0)
}

/// The property a GUID pointer names: MAKEDIPROP's numbers are
/// pointers below 64 KB.
fn property(guid: u32) -> Option<u32> {
    (guid < 0x10000).then_some(guid)
}

/// The joystick axes `target` names: all, or the one.
fn joystick_axes(d: &Device, target: Target) -> Result<Vec<usize>, u32> {
    if d.kind < JOYSTICK {
        return Err(DIERR_UNSUPPORTED);
    }
    match target {
        Target::Device => Ok((0..d.axes.len()).collect()),
        Target::Object(k) if k < d.axes.len() => Ok(vec![k]),
        Target::Object(_) => Err(DIERR_UNSUPPORTED),
    }
}

/// UTF-16 for DIPROPSTRING.
fn wide(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .take(259)
        .flat_map(|c| c.to_le_bytes())
        .collect()
}

/// Put `text` in the DIPROPSTRING at `at`.
fn put_string(cpu: &mut Cpu, at: u32, text: &str) {
    cpu.bus.guest_fill(at + 16, 520, 0);
    cpu.bus.guest_write_bytes(at + 16, &wide(text));
}

/// The size of the structure of the property `prop` if it is one that
/// DirectInput 8 added. Earlier versions don't know them, and say so
/// with DIERR_UNSUPPORTED, as DirectInput 7 does.
fn size_8(prop: u32) -> Option<u32> {
    match prop {
        DIPROP_KEYNAME | DIPROP_USERNAME | DIPROP_TYPENAME => Some(PROP_STRING),
        DIPROP_SCANCODE | DIPROP_VIDPID => Some(PROP_DWORD),
        DIPROP_APPDATA => Some(PROP_POINTER),
        _ => None,
    }
}

/// GetProperty(guid, header).
pub fn get_property(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, guid, at) = (args[0], args[1], args[2]);
    let r = (|| {
        let id = ready(cpu, this)?;
        let prop = property(guid).ok_or(DIERR_UNSUPPORTED)?;
        let size = match prop {
            DIPROP_RANGE => PROP_RANGE,
            DIPROP_INSTANCENAME | DIPROP_PRODUCTNAME => PROP_STRING,
            DIPROP_BUFFERSIZE..=DIPROP_CALIBRATIONMODE | DIPROP_JOYSTICKID => PROP_DWORD,
            _ if is_eight(cpu, id) => size_8(prop).ok_or(DIERR_UNSUPPORTED)?,
            _ => return Err(DIERR_UNSUPPORTED),
        };
        let target = header(cpu, id, at, size)?;
        let d = dev(cpu, id);
        let value = |v: u32| vec![v];
        let words = match prop {
            DIPROP_BUFFERSIZE if target == Target::Device => value(d.buffer_size),
            DIPROP_AXISMODE if d.kind == KEYBOARD => return Err(DIERR_UNSUPPORTED),
            DIPROP_AXISMODE if target == Target::Device => value(d.relative as u32),
            DIPROP_BUFFERSIZE | DIPROP_AXISMODE => return Err(DIERR_INVALIDPARAM),
            DIPROP_GRANULARITY | DIPROP_RANGE | DIPROP_DEADZONE | DIPROP_SATURATION => {
                let Target::Object(k) = target else {
                    return Err(DIERR_NOTFOUND);
                };
                if !d.objects()[k].is_axis() {
                    return Err(DIERR_UNSUPPORTED);
                }
                match (prop, d.axes.get(k)) {
                    (DIPROP_GRANULARITY, _) => value(1),
                    (DIPROP_RANGE, None) => vec![DIPROPRANGE_NOMIN, DIPROPRANGE_NOMAX],
                    (DIPROP_RANGE, Some(a)) => vec![a.min as u32, a.max as u32],
                    (DIPROP_DEADZONE, Some(a)) => value(a.deadzone),
                    (DIPROP_SATURATION, Some(a)) => value(a.saturation),
                    _ => return Err(DIERR_UNSUPPORTED),
                }
            }
            DIPROP_AUTOCENTER if d.kind >= JOYSTICK => value(d.autocenter as u32),
            DIPROP_CALIBRATIONMODE if d.kind >= JOYSTICK => value(d.calibration),
            DIPROP_JOYSTICKID if d.kind >= JOYSTICK => value(d.kind - JOYSTICK),
            DIPROP_INSTANCENAME | DIPROP_PRODUCTNAME => {
                // The names, which are the same for every version.
                let info = objects::instance(d.kind, d.shape, objects::INSTANCE_SIZE, 0x700);
                let from = if prop == DIPROP_INSTANCENAME { 40 } else { 300 };
                let name: Vec<u8> = info[from..from + 260]
                    .iter()
                    .take_while(|&&b| b != 0)
                    .copied()
                    .collect();
                put_string(cpu, at, &String::from_utf8_lossy(&name));
                return Ok(DI_OK);
            }
            // A key's name, as GetObjectInfo gives it, and its scan code:
            // the 8042's, with E0 in the low byte before an extended
            // key's. The mouse's and joystick's objects have neither.
            DIPROP_KEYNAME | DIPROP_SCANCODE => {
                let Target::Object(k) = target else {
                    return Err(DIERR_INVALIDPARAM);
                };
                if d.kind != KEYBOARD {
                    return Err(DIERR_UNSUPPORTED);
                }
                let obj = d.objects()[k];
                if prop == DIPROP_SCANCODE {
                    value(objects::scan_code(obj.instance() as u8))
                } else {
                    put_string(cpu, at, &objects::object_name(KEYBOARD, &obj));
                    return Ok(DI_OK);
                }
            }
            DIPROP_APPDATA => {
                let Target::Object(k) = target else {
                    return Err(DIERR_UNSUPPORTED);
                };
                value(d.app_data(k))
            }
            // The user an action map gave the device to, and no action
            // map is there: none, with an empty name.
            DIPROP_USERNAME if target == Target::Device => {
                put_string(cpu, at, "");
                return Ok(DI_NOEFFECT);
            }
            // A HID device's vendor and product, which the keyboard, the
            // mouse and a joystick of the game port don't have, and the
            // joystick's type (#378).
            DIPROP_USERNAME | DIPROP_VIDPID | DIPROP_TYPENAME => {
                return Err(DIERR_UNSUPPORTED);
            }
            _ => return Err(DIERR_UNSUPPORTED),
        };
        for (i, w) in words.iter().enumerate() {
            cpu.bus.guest_write_32(at + 16 + 4 * i as u32, *w);
        }
        Ok(DI_OK)
    })();
    result(r)
}

/// SetProperty(guid, header).
pub fn set_property(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, guid, at) = (args[0], args[1], args[2]);
    let r = (|| {
        let id = ready(cpu, this)?;
        let prop = property(guid).ok_or(DIERR_UNSUPPORTED)?;
        let size = match prop {
            DIPROP_RANGE => PROP_RANGE,
            DIPROP_BUFFERSIZE..=DIPROP_CALIBRATIONMODE => PROP_DWORD,
            _ if is_eight(cpu, id) => size_8(prop).ok_or(DIERR_UNSUPPORTED)?,
            _ => return Err(DIERR_UNSUPPORTED),
        };
        let target = header(cpu, id, at, size)?;
        let data = cpu.bus.guest_read_32(at + 16);
        let data2 = cpu.bus.guest_read_32(at + 20);
        let d = dev(cpu, id);
        match prop {
            DIPROP_BUFFERSIZE => {
                if target != Target::Device {
                    return Err(DIERR_INVALIDPARAM);
                }
                if d.acquired {
                    return Err(DIERR_ACQUIRED);
                }
                d.buffer_size = data;
                d.queue.truncate(data.min(MAX_BUFFER) as usize);
            }
            DIPROP_AXISMODE => {
                if d.kind == KEYBOARD || target != Target::Device {
                    // No device has the mode of each axis apart.
                    return Err(DIERR_UNSUPPORTED);
                }
                if data > 1 {
                    return Err(DIERR_INVALIDPARAM);
                }
                if d.acquired {
                    return Err(DIERR_ACQUIRED);
                }
                d.relative = data == 1;
            }
            DIPROP_RANGE => {
                let axes = joystick_axes(d, target)?;
                let (min, max) = (data as i32, data2 as i32);
                if min >= max {
                    return Err(DIERR_INVALIDPARAM);
                }
                for k in axes {
                    d.axes[k].min = min;
                    d.axes[k].max = max;
                }
            }
            DIPROP_DEADZONE | DIPROP_SATURATION => {
                let axes = joystick_axes(d, target)?;
                if data > 10000 {
                    return Err(DIERR_INVALIDPARAM);
                }
                for k in axes {
                    if prop == DIPROP_DEADZONE {
                        d.axes[k].deadzone = data;
                    } else {
                        d.axes[k].saturation = data;
                    }
                }
            }
            DIPROP_AUTOCENTER | DIPROP_CALIBRATIONMODE => {
                if d.kind < JOYSTICK {
                    return Err(DIERR_UNSUPPORTED);
                }
                if data > 1 {
                    return Err(DIERR_INVALIDPARAM);
                }
                if prop == DIPROP_AUTOCENTER {
                    if d.acquired {
                        return Err(DIERR_ACQUIRED);
                    }
                    d.autocenter = data == 1;
                } else {
                    d.calibration = data;
                }
            }
            // An object's value for the program's own use, which its
            // buffered data carries, until the next SetDataFormat.
            DIPROP_APPDATA => {
                let Target::Object(k) = target else {
                    return Err(DIERR_UNSUPPORTED);
                };
                d.set_app_data(k, data);
            }
            DIPROP_KEYNAME | DIPROP_SCANCODE if target == Target::Device => {
                return Err(DIERR_INVALIDPARAM);
            }
            DIPROP_USERNAME | DIPROP_VIDPID | DIPROP_TYPENAME => {
                if target != Target::Device {
                    return Err(DIERR_UNSUPPORTED);
                }
                return Err(DIERR_READONLY);
            }
            DIPROP_GRANULARITY => return Err(DIERR_READONLY),
            DIPROP_FFGAIN | DIPROP_FFLOAD => return Err(DIERR_UNSUPPORTED),
            _ => return Err(DIERR_UNSUPPORTED),
        }
        Ok(DI_OK)
    })();
    result(r)
}

/// Whether the device `id`'s objects are the device `kind`'s, held
/// exclusively by a device object other than `id`.
fn held_exclusively(cpu: &mut Cpu, id: u32, kind: u32) -> bool {
    state(cpu).devices_mut().any(|(other, d)| {
        other != id && d.kind == kind && d.acquired && d.coop & DISCL_EXCLUSIVE != 0
    })
}

/// Acquire.
pub fn acquire(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        let id = ready(cpu, args[0])?;
        let d = dev(cpu, id);
        if d.acquired {
            return Ok(DI_NOEFFECT);
        }
        if d.format.is_none() {
            return Err(DIERR_INVALIDPARAM);
        }
        let (coop, hwnd, kind) = (d.coop, d.hwnd, d.kind);
        if coop & DISCL_FOREGROUND != 0 && !poll::is_foreground(cpu, hwnd) {
            return Err(DIERR_OTHERAPPHASPRIO);
        }
        if coop & DISCL_EXCLUSIVE != 0 && held_exclusively(cpu, id, kind) {
            return Err(DIERR_OTHERAPPHASPRIO);
        }
        let joystick = (kind >= JOYSTICK).then(|| poll::joystick_raw(cpu, kind - JOYSTICK));
        let d = dev(cpu, id);
        d.acquired = true;
        d.lost = false;
        d.queue.clear();
        d.overflow = false;
        d.moved = [0; 2];
        if let Some((counts, buttons)) = joystick {
            let values = d.joystick_values(&counts);
            for (a, v) in d.axes.iter_mut().zip(values) {
                a.last = v;
                a.base = v;
            }
            d.buttons = buttons;
        }
        Ok(DI_OK)
    })();
    result(r)
}

/// Unacquire.
pub fn unacquire(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        let id = ready(cpu, args[0])?;
        let d = dev(cpu, id);
        if !d.acquired {
            return Ok(DI_NOEFFECT);
        }
        d.acquired = false;
        Ok(DI_OK)
    })();
    result(r)
}

/// The error for reading a device that isn't acquired.
fn not_acquired(d: &mut Device) -> u32 {
    if std::mem::take(&mut d.lost) {
        DIERR_INPUTLOST
    } else {
        DIERR_NOTACQUIRED
    }
}

/// GetDeviceState(size, data): the data format filled with the device's
/// state.
pub fn get_device_state(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, size, at) = (args[0], args[1], args[2]);
    let r = (|| {
        let id = ready(cpu, this)?;
        poll::refresh(cpu);
        let d = dev(cpu, id);
        if !d.acquired {
            return Err(not_acquired(d));
        }
        let Some(format) = d.format.clone() else {
            return Err(DIERR_NOTACQUIRED);
        };
        if size != format.size {
            return Err(DIERR_INVALIDPARAM);
        }
        if at == 0 || !cpu.bus.guest_probe(at, size as usize, true) {
            return Err(E_POINTER);
        }
        let mut data = vec![0u8; size as usize];
        for &pov in &format.povs {
            data[pov as usize..pov as usize + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        let values = state_values(cpu, id);
        for (k, &ofs) in format.offsets.iter().enumerate() {
            if ofs == NONE {
                continue;
            }
            let (value, wide) = values[k];
            let ofs = ofs as usize;
            if wide {
                data[ofs..ofs + 4].copy_from_slice(&value.to_le_bytes());
            } else {
                data[ofs] = value as u8;
            }
        }
        cpu.bus.guest_write_bytes(at, &data);
        Ok(DI_OK)
    })();
    result(r)
}

/// The value of each object of the device `id` as GetDeviceState gives
/// it, and whether it takes four bytes; the relative axes' motion is
/// taken.
fn state_values(cpu: &mut Cpu, id: u32) -> Vec<(u32, bool)> {
    let kind = dev(cpu, id).kind;
    match kind {
        KEYBOARD => {
            let held = cpu.bus.kbd.held_keys();
            dev(cpu, id)
                .objects()
                .iter()
                .map(|o| {
                    let key = (o.ofs & 0x7F) as usize | ((o.ofs & 0x80) as usize) << 1;
                    let down = held[key / 64] >> (key % 64) & 1 != 0;
                    (if down { 0x80 } else { 0 }, false)
                })
                .collect()
        }
        MOUSE => {
            let buttons = cpu.bus.mouse.buttons;
            let d = dev(cpu, id);
            let axes = if d.relative {
                std::mem::take(&mut d.moved)
            } else {
                d.position
            };
            let mut values: Vec<(u32, bool)> = axes.iter().map(|&v| (v as u32, true)).collect();
            values.extend((0..3).map(|b| ((buttons >> b & 1) as u32 * 0x80, false)));
            values
        }
        _ => {
            let (counts, buttons) = poll::joystick_raw(cpu, kind - JOYSTICK);
            let d = dev(cpu, id);
            let now = d.joystick_values(&counts);
            let mut values = Vec::new();
            for (a, v) in d.axes.iter_mut().zip(now) {
                let value = if d.relative {
                    v.wrapping_sub(std::mem::replace(&mut a.base, v))
                } else {
                    v
                };
                values.push((value as u32, true));
            }
            values.extend((0..d.shape.1).map(|b| ((buttons >> b & 1) * 0x80, false)));
            values
        }
    }
}

/// GetDeviceData(object size, data, count, flags): the buffered events,
/// as many as fit, taken from the buffer unless DIGDD_PEEK. No buffer
/// (`data` 0) takes them without returning them.
pub fn get_device_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, object_size, at, count_at, flags) = (args[0], args[1], args[2], args[3], args[4]);
    let r = (|| {
        let id = ready(cpu, this)?;
        // DIDEVICEOBJECTDATA, with DirectX 8's uAppData (the object's
        // DIPROP_APPDATA) or without.
        if object_size != 16 && object_size != 20 {
            return Err(DIERR_INVALIDPARAM);
        }
        if flags & !DIGDD_PEEK != 0 {
            return Err(DIERR_INVALIDPARAM);
        }
        poll::refresh(cpu);
        let d = dev(cpu, id);
        if !d.acquired {
            return Err(not_acquired(d));
        }
        if d.buffer_size == 0 {
            return Err(DIERR_NOTBUFFERED);
        }
        if count_at == 0 || !cpu.bus.guest_probe(count_at, 4, true) {
            return Err(DIERR_INVALIDPARAM);
        }
        let wanted = cpu.bus.guest_read_32(count_at);
        let d = dev(cpu, id);
        let n = (wanted as usize).min(d.queue.len());
        let events: Vec<Event> = d.queue[..n].to_vec();
        if at != 0 {
            if !cpu.bus.guest_probe(at, n * object_size as usize, true) {
                return Err(DIERR_INVALIDPARAM);
            }
            for (i, e) in events.iter().enumerate() {
                let base = at + i as u32 * object_size;
                let mut words = vec![e.ofs, e.data, e.time, e.sequence];
                if object_size == 20 {
                    words.push(e.app_data);
                }
                for (k, w) in words.iter().enumerate() {
                    cpu.bus.guest_write_32(base + 4 * k as u32, *w);
                }
            }
        }
        let d = dev(cpu, id);
        let overflow = d.overflow;
        if flags & DIGDD_PEEK == 0 {
            d.queue.drain(..n);
            d.overflow = false;
        }
        cpu.bus.guest_write_32(count_at, n as u32);
        // DI_BUFFEROVERFLOW.
        Ok(if overflow { DI_NOEFFECT } else { DI_OK })
    })();
    result(r)
}

/// SetDataFormat(format).
pub fn set_data_format(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        let id = ready(cpu, args[0])?;
        let objs = dev(cpu, id).objects();
        let format = objects::read_format(cpu, args[1], &objs)?;
        let d = dev(cpu, id);
        if d.acquired {
            return Err(DIERR_ACQUIRED);
        }
        if d.kind != KEYBOARD {
            if format.flags & DIDF_ABSAXIS != 0 {
                d.relative = false;
            } else if format.flags & DIDF_RELAXIS != 0 {
                d.relative = true;
            }
        }
        d.format = Some(format);
        d.queue.clear();
        d.app_data.clear();
        Ok(DI_OK)
    })();
    result(r)
}

/// SetEventNotification(event): the event is set when the device's
/// state changes or it loses acquisition; 0 takes it away.
pub fn set_event_notification(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, handle) = (args[0], args[1]);
    let r = (|| {
        let id = ready(cpu, this)?;
        if dev(cpu, id).acquired {
            return Err(DIERR_ACQUIRED);
        }
        let event = if handle == 0 {
            0
        } else {
            let id = object::resolve(cpu, handle).ok_or(DIERR_INVALIDPARAM)?;
            let is_event = matches!(
                object::objects(cpu).get(id).map(|o| &o.kind),
                Some(Kind::Event { .. })
            );
            if !is_event {
                return Err(DIERR_INVALIDPARAM);
            }
            id
        };
        let old = dev(cpu, id).event;
        if old != 0 && event != 0 && old != event {
            // DIERR_HANDLEEXISTS.
            return Err(DIERR_READONLY);
        }
        if old == event {
            return Ok(DI_OK);
        }
        if event != 0 {
            object::objects(cpu).hold(event);
        }
        if old != 0 {
            object::objects(cpu).release(old);
        }
        dev(cpu, id).event = event;
        Ok(DI_OK)
    })();
    result(r)
}

/// SetCooperativeLevel(hwnd, flags). Without a window, only
/// non-exclusive background access, with the desktop's. The keyboard
/// and the mouse aren't had exclusively in the background.
pub fn set_cooperative_level(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, hwnd, flags) = (args[0], args[1], args[2]);
    let r = (|| {
        let id = ready(cpu, this)?;
        let access = flags & (DISCL_EXCLUSIVE | DISCL_NONEXCLUSIVE);
        let place = flags & (DISCL_FOREGROUND | DISCL_BACKGROUND);
        if access == 0
            || access == DISCL_EXCLUSIVE | DISCL_NONEXCLUSIVE
            || place == 0
            || place == DISCL_FOREGROUND | DISCL_BACKGROUND
            || flags & !0x1F != 0
        {
            return Err(E_INVALIDARG);
        }
        let user = &cpu.bus.win32.user;
        if hwnd != 0
            && user
                .window(hwnd)
                .is_none_or(|w| w.style & crate::win32::user32::window::WS_CHILD != 0)
        {
            return Err(E_HANDLE);
        }
        let main = flags & !DISCL_NOWINKEY;
        let hwnd = match hwnd {
            0 if main == DISCL_NONEXCLUSIVE | DISCL_BACKGROUND => crate::win32::user32::DESKTOP,
            0 => return Err(E_HANDLE),
            h => h,
        };
        let d = dev(cpu, id);
        if main == DISCL_EXCLUSIVE | DISCL_BACKGROUND && (d.kind == KEYBOARD || d.kind == MOUSE) {
            return Err(DIERR_UNSUPPORTED);
        }
        if d.acquired {
            return Err(DIERR_ACQUIRED);
        }
        d.coop = flags;
        d.hwnd = hwnd;
        Ok(DI_OK)
    })();
    result(r)
}

/// GetObjectInfo(info, obj, how): DIDEVICEOBJECTINSTANCEA.
pub fn get_object_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, at, obj, how) = (args[0], args[1], args[2], args[3]);
    let r = (|| {
        let id = ready(cpu, this)?;
        if at == 0 || !cpu.bus.guest_probe(at, 4, false) {
            return Err(E_POINTER);
        }
        let size = cpu.bus.guest_read_32(at);
        if size != objects::OBJECT_SIZE && size != objects::OBJECT_SIZE_DX3 {
            return Err(DIERR_INVALIDPARAM);
        }
        let d = dev(cpu, id);
        let objs = d.objects();
        let k = match how {
            DIPH_BYOFFSET => d.format.as_ref().and_then(|f| f.object_at(obj)),
            DIPH_BYID => find_by_id(&objs, obj),
            DIPH_BYUSAGE => return Err(DIERR_UNSUPPORTED),
            _ => return Err(DIERR_INVALIDPARAM),
        }
        .ok_or(DIERR_NOTFOUND)?;
        let ofs = d.offset(k).unwrap_or(objs[k].ofs);
        let bytes = objects::object_instance(d.kind, &objs[k], ofs, size);
        if !cpu.bus.guest_probe(at, size as usize, true) {
            return Err(E_POINTER);
        }
        cpu.bus.guest_write_bytes(at, &bytes);
        Ok(DI_OK)
    })();
    result(r)
}

/// GetDeviceInfo(info): DIDEVICEINSTANCEA.
pub fn get_device_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, at) = (args[0], args[1]);
    let r = (|| {
        let id = ready(cpu, this)?;
        if at == 0 || !cpu.bus.guest_probe(at, 4, false) {
            return Err(E_POINTER);
        }
        let size = cpu.bus.guest_read_32(at);
        if size != objects::INSTANCE_SIZE && size != objects::INSTANCE_SIZE_DX3 {
            return Err(DIERR_INVALIDPARAM);
        }
        if !cpu.bus.guest_probe(at, size as usize, true) {
            return Err(E_POINTER);
        }
        let version = version_of(cpu, id);
        let d = dev(cpu, id);
        let bytes = objects::instance(d.kind, d.shape, size, version);
        cpu.bus.guest_write_bytes(at, &bytes);
        Ok(DI_OK)
    })();
    result(r)
}

/// RunControlPanel(hwnd, flags): there is no control panel to run.
pub fn run_control_panel(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        ready(cpu, args[0])?;
        if args[2] != 0 {
            return Err(DIERR_INVALIDPARAM);
        }
        Ok(DI_OK)
    })();
    result(r)
}

/// Initialize(hinst, version, guid): the device becomes the one `guid`
/// names; S_FALSE if it is that one already.
pub fn initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, hinst, ver, guid) = (args[0], args[1], args[2], args[3]);
    let r = (|| {
        let id = device_of(cpu, this)?;
        check_version(hinst, ver, is_eight(cpu, id))?;
        let guid = Guid::read(cpu, guid).ok_or(E_POINTER)?;
        let kind = objects::kind_of(cpu, &guid, ver).ok_or(DIERR_DEVICENOTREG)?;
        let d = dev(cpu, id);
        if d.kind == kind {
            return Ok(DI_NOEFFECT);
        }
        if d.acquired {
            return Err(DIERR_ACQUIRED);
        }
        let (parent, event) = (d.parent, d.event);
        let shape = objects::shape(cpu, kind);
        *dev(cpu, id) = Device {
            event,
            ..Device::new(parent, kind, shape)
        };
        if let Some(o) = state(cpu).object_mut(id) {
            o.version = ver;
        }
        Ok(DI_OK)
    })();
    result(r)
}

/// CreateEffect(guid, effect, out, outer): no force feedback.
pub fn create_effect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        ready(cpu, args[0])?;
        if args[3] != 0 {
            com::write_out(cpu, args[3], 0);
        }
        Err(DIERR_UNSUPPORTED)
    })();
    result(r)
}

/// EnumEffects(callback, ref, type) and EnumCreatedEffectObjects: there
/// are none to give.
pub fn enum_effects(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        ready(cpu, args[0])?;
        if args[1] == 0 {
            return Err(DIERR_INVALIDPARAM);
        }
        Ok(DI_OK)
    })();
    result(r)
}

/// GetEffectInfo(info, guid): the device has no effects.
pub fn get_effect_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        ready(cpu, args[0])?;
        Err(DIERR_DEVICENOTREG)
    })();
    result(r)
}

/// GetForceFeedbackState, SendForceFeedbackCommand and Escape: nothing
/// to do them to.
pub fn no_force_feedback(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        ready(cpu, args[0])?;
        Err(DIERR_UNSUPPORTED)
    })();
    result(r)
}

/// EnumEffectsInFile and WriteEffectToFile.
pub fn effects_in_file(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    no_force_feedback(cpu, args)
}

/// IDirectInputDevice8's BuildActionMap(format, user, flags),
/// SetActionMap(format, user, flags) and GetImageInfo(header): action
/// maps and device images aren't there, and the methods say so with
/// DIERR_UNSUPPORTED (E_NOTIMPL). Programs of DirectX 8 that read their
/// devices through data formats, as those of DirectX 7 do, don't call
/// them.
pub fn action_map(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        ready(cpu, args[0])?;
        Err(DIERR_UNSUPPORTED)
    })();
    result(r)
}

/// Poll: the devices are read; DI_NOEFFECT for those that don't need
/// polling, the keyboard and the mouse.
pub fn poll_device(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        let id = ready(cpu, args[0])?;
        poll::refresh(cpu);
        let d = dev(cpu, id);
        if !d.acquired {
            return Err(not_acquired(d));
        }
        Ok(if d.kind >= JOYSTICK {
            DI_OK
        } else {
            DI_NOEFFECT
        })
    })();
    result(r)
}

/// SendDeviceData(size, data, count, flags): the devices have nothing
/// to send data to.
pub fn send_device_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        let id = ready(cpu, args[0])?;
        if !dev(cpu, id).acquired {
            return Err(DIERR_NOTACQUIRED);
        }
        Err(DIERR_UNSUPPORTED)
    })();
    result(r)
}

crate::state_fields!(Axis {
    min,
    max,
    deadzone,
    saturation,
    last,
    base
});
crate::state_fields!(Event {
    ofs,
    data,
    time,
    sequence,
    app_data
});
crate::state_fields!(Device {
    parent,
    kind,
    shape,
    coop,
    hwnd,
    acquired,
    lost,
    format,
    buffer_size,
    queue,
    overflow,
    event,
    relative,
    axes,
    moved,
    position,
    buttons,
    autocenter,
    calibration,
    app_data
});
crate::state_fields!(Format {
    size,
    flags,
    offsets,
    povs
});
