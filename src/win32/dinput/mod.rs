//! DINPUT.DLL: DirectInput: the keyboard, the mouse and the joystick of the
//! emulated PC as DirectInput devices, for programs of DirectX 3 to 7.
//!
//! DirectInputCreateA makes a DirectInput object, whose interface is
//! IDirectInputA, IDirectInput2A and IDirectInput7A in one: their methods
//! are a prefix of IDirectInput7A's, so one table serves all three, as in
//! Wine. Its CreateDevice makes a device: the system keyboard, the system
//! mouse or a joystick of the game port, with IDirectInputDeviceA, 2A and
//! 7A in one interface in the same way. Each object is a block in the
//! process heap (com.rs) whose id is its place in `State::objects`, where
//! its state is. A device holds a reference to the DirectInput object
//! that made it.
//!
//! The devices are read where USER32 reads them (poll.rs): the keys that
//! go down and up come from USER32's reading of the 8042 keyboard, the
//! mouse's motion and buttons and the game port are read when time
//! passes. Buffered data has the emulated clock's milliseconds and a
//! sequence number of DirectInput's own, so runs are deterministic.
//!
//! (Phase 3 of docs/native-windows.md.)

mod device;
mod enumerate;
mod objects;
mod poll;

pub use enumerate::step;
pub use poll::{key, mouse_taken, poll};

use super::com::{
    self, CLASS_E_NOAGGREGATION, E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, Guid,
    IID_IUNKNOWN, REGDB_E_CLASSNOTREG, S_FALSE, S_OK,
};
use super::stubs::{Api, Conv, Dll, Vtable};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "DINPUT.DLL",
    base: 0xBFA8_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
    vtables: VTABLES,
};

const fn stdcall(
    name: &'static str,
    ordinal: u16,
    args: u8,
    handler: super::stubs::Handler,
) -> Api {
    Api {
        name,
        ordinal,
        args,
        conv: Conv::Stdcall,
        handler,
    }
}

/// A method of an interface: not exported.
const fn method(name: &'static str, args: u8, handler: super::stubs::Handler) -> Api {
    stdcall(name, 0, args, handler)
}

/// The functions. Append only (see `stubs`). The methods have ordinal
/// 0: they aren't exported.
static APIS: &[Api] = &[
    stdcall("DllCanUnloadNow", 1, 0, dll_can_unload_now),
    stdcall("DllGetClassObject", 2, 3, dll_get_class_object),
    stdcall("DirectInputCreateA", 3, 4, direct_input_create),
    stdcall("DirectInputCreateEx", 4, 5, direct_input_create_ex),
    method("DI_QueryInterface", 3, di_query_interface),
    method("DI_AddRef", 1, di_add_ref),
    method("DI_Release", 1, di_release),
    method("DI_CreateDevice", 4, di_create_device),
    method("DI_EnumDevices", 5, enumerate::enum_devices),
    method("DI_GetDeviceStatus", 2, di_get_device_status),
    method("DI_RunControlPanel", 3, di_run_control_panel),
    method("DI_Initialize", 3, di_initialize),
    method("DI_FindDevice", 4, di_find_device),
    method("DI_CreateDeviceEx", 5, di_create_device_ex),
    method("DID_QueryInterface", 3, device::query_interface),
    method("DID_AddRef", 1, device::add_ref),
    method("DID_Release", 1, device::release),
    method("DID_GetCapabilities", 2, device::get_capabilities),
    method("DID_EnumObjects", 4, enumerate::enum_objects),
    method("DID_GetProperty", 3, device::get_property),
    method("DID_SetProperty", 3, device::set_property),
    method("DID_Acquire", 1, device::acquire),
    method("DID_Unacquire", 1, device::unacquire),
    method("DID_GetDeviceState", 3, device::get_device_state),
    method("DID_GetDeviceData", 5, device::get_device_data),
    method("DID_SetDataFormat", 2, device::set_data_format),
    method(
        "DID_SetEventNotification",
        2,
        device::set_event_notification,
    ),
    method("DID_SetCooperativeLevel", 3, device::set_cooperative_level),
    method("DID_GetObjectInfo", 4, device::get_object_info),
    method("DID_GetDeviceInfo", 2, device::get_device_info),
    method("DID_RunControlPanel", 3, device::run_control_panel),
    method("DID_Initialize", 4, device::initialize),
    method("DID_CreateEffect", 5, device::create_effect),
    method("DID_EnumEffects", 4, device::enum_effects),
    method("DID_GetEffectInfo", 3, device::get_effect_info),
    method("DID_GetForceFeedbackState", 2, device::no_force_feedback),
    method("DID_SendForceFeedbackCommand", 2, device::no_force_feedback),
    method("DID_EnumCreatedEffectObjects", 4, device::enum_effects),
    method("DID_Escape", 2, device::no_force_feedback),
    method("DID_Poll", 1, device::poll_device),
    method("DID_SendDeviceData", 5, device::send_device_data),
    method("DID_EnumEffectsInFile", 5, device::effects_in_file),
    method("DID_WriteEffectToFile", 5, device::effects_in_file),
];

/// The tables of methods: IDirectInput7A's, which IDirectInputA's and
/// IDirectInput2A's are the start of, and IDirectInputDevice7A's, which
/// IDirectInputDeviceA's and IDirectInputDevice2A's are.
static VTABLES: &[Vtable] = &[
    Vtable {
        name: "IDirectInput7A",
        methods: &[
            "DI_QueryInterface",
            "DI_AddRef",
            "DI_Release",
            "DI_CreateDevice",
            "DI_EnumDevices",
            "DI_GetDeviceStatus",
            "DI_RunControlPanel",
            "DI_Initialize",
            "DI_FindDevice",
            "DI_CreateDeviceEx",
        ],
    },
    Vtable {
        name: "IDirectInputDevice7A",
        methods: &[
            "DID_QueryInterface",
            "DID_AddRef",
            "DID_Release",
            "DID_GetCapabilities",
            "DID_EnumObjects",
            "DID_GetProperty",
            "DID_SetProperty",
            "DID_Acquire",
            "DID_Unacquire",
            "DID_GetDeviceState",
            "DID_GetDeviceData",
            "DID_SetDataFormat",
            "DID_SetEventNotification",
            "DID_SetCooperativeLevel",
            "DID_GetObjectInfo",
            "DID_GetDeviceInfo",
            "DID_RunControlPanel",
            "DID_Initialize",
            "DID_CreateEffect",
            "DID_EnumEffects",
            "DID_GetEffectInfo",
            "DID_GetForceFeedbackState",
            "DID_SendForceFeedbackCommand",
            "DID_EnumCreatedEffectObjects",
            "DID_Escape",
            "DID_Poll",
            "DID_SendDeviceData",
            "DID_EnumEffectsInFile",
            "DID_WriteEffectToFile",
        ],
    },
];

/// The places of the vtables in `VTABLES`.
const DIRECT_INPUT: usize = 0;
const DEVICE: usize = 1;

/// DirectInput's results.
pub const DI_OK: u32 = S_OK;
pub const DI_NOEFFECT: u32 = S_FALSE;
pub const DIERR_OLDDIRECTINPUTVERSION: u32 = 0x8007_047E;
pub const DIERR_BETADIRECTINPUTVERSION: u32 = 0x8007_0481;
pub const DIERR_DEVICENOTREG: u32 = REGDB_E_CLASSNOTREG;
pub const DIERR_NOTFOUND: u32 = 0x8007_0002;
pub const DIERR_INVALIDPARAM: u32 = E_INVALIDARG;
pub const DIERR_UNSUPPORTED: u32 = com::E_NOTIMPL;
pub const DIERR_NOTINITIALIZED: u32 = 0x8007_0015;
pub const DIERR_OTHERAPPHASPRIO: u32 = 0x8007_0005;
pub const DIERR_READONLY: u32 = 0x8007_0005;
pub const DIERR_INPUTLOST: u32 = 0x8007_001E;
pub const DIERR_ACQUIRED: u32 = 0x8007_00AA;
pub const DIERR_NOTACQUIRED: u32 = 0x8007_000C;
pub const DIERR_NOTBUFFERED: u32 = 0x8004_0207;
pub const E_HANDLE: u32 = 0x8007_0006;

/// The versions of DirectInput a program may ask for, as DirectX 7's
/// DINPUT.DLL takes them: DirectX 3, 5, 5a, 5.2, 6, 6.1 and 7.
const VERSIONS: &[u32] = &[0x300, 0x500, 0x50A, 0x5B2, 0x602, 0x61A, 0x700];

const CLSID_DIRECT_INPUT: Guid = Guid::new(
    0x25E6_09E0,
    0xB259,
    0x11CF,
    [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0],
);
const CLSID_DIRECT_INPUT_DEVICE: Guid = Guid::new(
    0x25E6_09E1,
    0xB259,
    0x11CF,
    [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0],
);
const IID_IDIRECT_INPUT_A: Guid = Guid::new(
    0x8952_1360,
    0xAA8A,
    0x11CF,
    [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0],
);
const IID_IDIRECT_INPUT_2A: Guid = Guid::new(
    0x5944_E662,
    0xAA8A,
    0x11CF,
    [0xBF, 0xC7, 0x44, 0x45, 0x53, 0x54, 0, 0],
);
const IID_IDIRECT_INPUT_7A: Guid = Guid::new(
    0x9A4C_B684,
    0x236D,
    0x11D3,
    [0x8E, 0x9D, 0x00, 0xC0, 0x4F, 0x68, 0x44, 0xAE],
);

/// The classes of DINPUT's that CoCreateInstance makes: objects that
/// need their Initialize before anything else.
pub static CLASSES: &[com::Class] = &[
    com::Class {
        clsid: CLSID_DIRECT_INPUT,
        dll: "DINPUT.DLL",
        name: "DirectInput",
        create: create_direct_input,
    },
    com::Class {
        clsid: CLSID_DIRECT_INPUT_DEVICE,
        dll: "DINPUT.DLL",
        name: "DirectInputDevice",
        create: device::create_uninitialized,
    },
];

/// A DirectInput object or a device.
#[derive(Clone, Debug, Default)]
pub struct Object {
    /// The process it is in.
    pub pid: u32,
    /// Its interface's block, which all its interfaces are.
    pub this: u32,
    pub refs: u32,
    /// The DirectInput version it was made for, or 0 before Initialize.
    pub version: u32,
    /// A device's state; None for a DirectInput object.
    pub device: Option<device::Device>,
}

/// DirectInput's state: the system's.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// The objects of all processes, by their ids less 1.
    pub objects: Vec<Option<Object>>,
    /// The sequence number the last event had.
    pub sequence: u32,
    /// The mouse as DirectInput last read it.
    pub mouse: poll::MouseSeen,
}

impl State {
    fn object(&self, id: u32) -> Option<&Object> {
        self.objects.get(id.checked_sub(1)? as usize)?.as_ref()
    }

    fn object_mut(&mut self, id: u32) -> Option<&mut Object> {
        self.objects.get_mut(id.checked_sub(1)? as usize)?.as_mut()
    }

    /// Put `object` in a free place: its id.
    fn add(&mut self, object: Object) -> u32 {
        match self.objects.iter().position(Option::is_none) {
            Some(i) => {
                self.objects[i] = Some(object);
                i as u32 + 1
            }
            None => {
                self.objects.push(Some(object));
                self.objects.len() as u32
            }
        }
    }

    /// The next sequence number.
    fn next_sequence(&mut self) -> u32 {
        self.sequence = self.sequence.wrapping_add(1);
        self.sequence
    }

    /// Whether a device has the mouse exclusively: USER32 then leaves it
    /// alone, and Windows hides the cursor.
    pub fn mouse_taken(&self) -> bool {
        self.objects
            .iter()
            .filter_map(|o| o.as_ref()?.device.as_ref())
            .any(|d| {
                d.kind == objects::MOUSE && d.acquired && d.coop & device::DISCL_EXCLUSIVE != 0
            })
    }

    /// The devices, with their ids.
    fn devices_mut(&mut self) -> impl Iterator<Item = (u32, &mut device::Device)> {
        self.objects.iter_mut().enumerate().filter_map(|(i, o)| {
            let d = o.as_mut()?.device.as_mut()?;
            Some((i as u32 + 1, d))
        })
    }
}

fn state(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.dinput
}

fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("DINPUT is a built-in DLL")
}

/// The object whose interface `this` is, if it is one of DINPUT's with
/// the vtable `vtable`: its id.
fn object_of(cpu: &mut Cpu, this: u32, vtable: usize) -> Option<u32> {
    match com::interface_of(cpu, dll_index(), this)? {
        // A device's interface needs a device: the id in a forged one may
        // be that of the DirectInput object.
        (v, id)
            if v == vtable
                && state(cpu)
                    .object(id)
                    .is_some_and(|o| v != DEVICE || o.device.is_some()) =>
        {
            Some(id)
        }
        _ => None,
    }
}

/// A new object of the running process, with one reference: its
/// interface, or None when the heap is full.
fn new_object(
    cpu: &mut Cpu,
    vtable: usize,
    version: u32,
    device: Option<device::Device>,
) -> Option<u32> {
    let pid = super::user32::current_pid(cpu);
    let id = state(cpu).add(Object {
        pid,
        this: 0,
        refs: 1,
        version,
        device,
    });
    let Some(this) = com::new_interface(cpu, dll_index(), vtable, id, 0) else {
        state(cpu).objects[id as usize - 1] = None;
        return None;
    };
    if let Some(o) = state(cpu).object_mut(id) {
        o.this = this;
    }
    Some(this)
}

/// Take a reference of the object `id` away: it goes with the last one.
/// The count left.
fn release_object(cpu: &mut Cpu, id: u32) -> u32 {
    let Some(o) = state(cpu).object_mut(id) else {
        return 0;
    };
    o.refs = o.refs.saturating_sub(1);
    if o.refs > 0 {
        return o.refs;
    }
    let this = o.this;
    if let Some(d) = state(cpu).objects[id as usize - 1]
        .take()
        .and_then(|o| o.device)
    {
        device::destroyed(cpu, d);
    }
    com::free_interface(cpu, this);
    0
}

/// A process ended: its objects go, with what they held. Their blocks
/// went with its memory.
pub fn process_ended(cpu: &mut Cpu, pid: u32, _tids: &[u32]) {
    for i in 0..state(cpu).objects.len() {
        let mine = state(cpu).objects[i].as_ref().is_some_and(|o| o.pid == pid);
        if !mine {
            continue;
        }
        if let Some(mut d) = state(cpu).objects[i].take().and_then(|o| o.device) {
            // Its DirectInput object is the process's too.
            d.parent = 0;
            device::destroyed(cpu, d);
        }
    }
}

/// Whether DirectInput takes `version` from the instance `hinst`, or
/// the error it gives.
fn check_version(hinst: u32, version: u32) -> Result<(), u32> {
    if hinst == 0 {
        Err(DIERR_INVALIDPARAM)
    } else if version == 0 {
        Err(DIERR_NOTINITIALIZED)
    } else if version > 0x700 {
        Err(DIERR_OLDDIRECTINPUTVERSION)
    } else if !VERSIONS.contains(&version) {
        Err(DIERR_BETADIRECTINPUTVERSION)
    } else {
        Ok(())
    }
}

/// Whether `iid` is one of the DirectInput object's interfaces.
fn is_direct_input_iid(iid: &Guid) -> bool {
    [
        IID_IUNKNOWN,
        IID_IDIRECT_INPUT_A,
        IID_IDIRECT_INPUT_2A,
        IID_IDIRECT_INPUT_7A,
    ]
    .contains(iid)
}

fn result(r: Result<u32, u32>) -> Option<u32> {
    Some(r.unwrap_or_else(|e| e))
}

fn dll_can_unload_now(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_FALSE)
}

fn dll_get_class_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(super::ole32::dll_get_class_object(cpu, DLL.name, args))
}

/// CoCreateInstance's DirectInput object, which needs its Initialize.
fn create_direct_input(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    if !is_direct_input_iid(iid) {
        return E_NOINTERFACE;
    }
    let Some(this) = new_object(cpu, DIRECT_INPUT, 0, None) else {
        return E_OUTOFMEMORY;
    };
    if !com::write_out(cpu, out, this) {
        di_release(cpu, &[this]);
        return E_POINTER;
    }
    S_OK
}

/// DirectInputCreateA(hinst, version, out, outer).
fn direct_input_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hinst, version, out, outer) = (args[0], args[1], args[2], args[3]);
    Some(create(
        cpu,
        hinst,
        version,
        &IID_IDIRECT_INPUT_A,
        out,
        outer,
    ))
}

/// DirectInputCreateEx(hinst, version, iid, out, outer), DirectX 7's.
fn direct_input_create_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hinst, version, iid, out, outer) = (args[0], args[1], args[2], args[3], args[4]);
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(E_POINTER);
    };
    Some(create(cpu, hinst, version, &iid, out, outer))
}

fn create(cpu: &mut Cpu, hinst: u32, version: u32, iid: &Guid, out: u32, outer: u32) -> u32 {
    if !com::write_out(cpu, out, 0) {
        return E_POINTER;
    }
    if outer != 0 {
        return CLASS_E_NOAGGREGATION;
    }
    if !is_direct_input_iid(iid) {
        return E_NOINTERFACE;
    }
    if let Err(e) = check_version(hinst, version) {
        return e;
    }
    match new_object(cpu, DIRECT_INPUT, version, None) {
        Some(this) => {
            com::write_out(cpu, out, this);
            DI_OK
        }
        None => E_OUTOFMEMORY,
    }
}

/// The DirectInput object `this`, and its version (0 before
/// Initialize); or the error a method of it gives.
fn direct_input(cpu: &mut Cpu, this: u32) -> Result<(u32, u32), u32> {
    let id = object_of(cpu, this, DIRECT_INPUT).ok_or(E_POINTER)?;
    let version = state(cpu).object(id).map_or(0, |o| o.version);
    Ok((id, version))
}

/// The same, once initialized.
fn initialized(cpu: &mut Cpu, this: u32) -> Result<(u32, u32), u32> {
    match direct_input(cpu, this)? {
        (_, 0) => Err(DIERR_NOTINITIALIZED),
        found => Ok(found),
    }
}

/// IDirectInput::QueryInterface(iid, out).
fn di_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, iid, out) = (args[0], args[1], args[2]);
    let r = (|| {
        let (id, _) = direct_input(cpu, this)?;
        let iid = Guid::read(cpu, iid).ok_or(E_POINTER)?;
        if !com::write_out(cpu, out, 0) {
            return Err(E_POINTER);
        }
        if !is_direct_input_iid(&iid) {
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

fn di_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object_of(cpu, args[0], DIRECT_INPUT) else {
        return Some(0);
    };
    let o = state(cpu).object_mut(id)?;
    o.refs += 1;
    Some(o.refs)
}

fn di_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(id) = object_of(cpu, args[0], DIRECT_INPUT) else {
        return Some(0);
    };
    Some(release_object(cpu, id))
}

/// IDirectInput::CreateDevice(guid, out, outer).
fn di_create_device(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, guid, out, outer) = (args[0], args[1], args[2], args[3]);
    let iid = device::IID_IDIRECT_INPUT_DEVICE_A;
    result(create_device(cpu, this, guid, &iid, out, outer))
}

/// IDirectInput7::CreateDeviceEx(guid, iid, out, outer).
fn di_create_device_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (this, guid, iid, out, outer) = (args[0], args[1], args[2], args[3], args[4]);
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(E_POINTER);
    };
    result(create_device(cpu, this, guid, &iid, out, outer))
}

fn create_device(
    cpu: &mut Cpu,
    this: u32,
    guid: u32,
    iid: &Guid,
    out: u32,
    outer: u32,
) -> Result<u32, u32> {
    let (id, version) = direct_input(cpu, this)?;
    let guid = Guid::read(cpu, guid);
    if !com::write_out(cpu, out, 0) {
        return Err(E_POINTER);
    }
    let guid = guid.ok_or(E_POINTER)?;
    if version == 0 {
        return Err(DIERR_NOTINITIALIZED);
    }
    if outer != 0 {
        return Err(CLASS_E_NOAGGREGATION);
    }
    if !device::is_device_iid(iid) {
        return Err(E_NOINTERFACE);
    }
    let kind = objects::kind_of(cpu, &guid, version).ok_or(DIERR_DEVICENOTREG)?;
    let device = device::Device::new(id, kind, objects::shape(cpu, kind));
    let this = new_object(cpu, DEVICE, version, Some(device)).ok_or(E_OUTOFMEMORY)?;
    if let Some(o) = state(cpu).object_mut(id) {
        o.refs += 1;
    }
    com::write_out(cpu, out, this);
    Ok(DI_OK)
}

/// IDirectInput::GetDeviceStatus(guid): DI_OK for a device that is
/// there.
fn di_get_device_status(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        let (_, version) = initialized(cpu, args[0])?;
        let guid = Guid::read(cpu, args[1]).ok_or(E_POINTER)?;
        objects::kind_of(cpu, &guid, version).ok_or(DIERR_DEVICENOTREG)?;
        Ok(DI_OK)
    })();
    result(r)
}

/// IDirectInput::RunControlPanel(hwnd, flags): there is no control panel
/// to run.
fn di_run_control_panel(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        initialized(cpu, args[0])?;
        if args[2] != 0 {
            return Err(DIERR_INVALIDPARAM);
        }
        Ok(DI_OK)
    })();
    result(r)
}

/// IDirectInput::Initialize(hinst, version).
fn di_initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        let (id, _) = direct_input(cpu, args[0])?;
        check_version(args[1], args[2])?;
        if let Some(o) = state(cpu).object_mut(id) {
            o.version = args[2];
        }
        Ok(DI_OK)
    })();
    result(r)
}

/// IDirectInput2::FindDevice(class, name, out): it finds HID devices by
/// their names, and the emulated PC has none.
fn di_find_device(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = (|| {
        initialized(cpu, args[0])?;
        Guid::read(cpu, args[1]).ok_or(E_POINTER)?;
        if args[2] == 0 || args[3] == 0 {
            return Err(E_POINTER);
        }
        Err(DIERR_DEVICENOTREG)
    })();
    result(r)
}

crate::state_fields!(Object {
    pid,
    this,
    refs,
    version,
    device
});
crate::state_fields!(State {
    objects,
    sequence,
    mouse
});
