//! DSOUND.DLL: DirectSound: sound buffers in the program's memory, mixed at
//! the emulated rate into the host's sound.
//!
//! A DirectSound object (IDirectSound, sound.rs) makes buffers
//! (IDirectSoundBuffer, buffer.rs): the primary buffer, which is the
//! device's output and its format, and secondary buffers, which hold
//! sounds and play mixed into it, each at its own rate, volume and pan,
//! once or in a loop. A buffer's samples are in pages of the process's
//! memory, which Lock hands the program; the device (device.rs) reads
//! them from there as it plays. A buffer has other interfaces too, which
//! QueryInterface gives: IDirectSound3DBuffer, and IDirectSound3DListener
//! for the primary buffer (ds3d.rs), IDirectSoundNotify and
//! IKsPropertySet (buffer.rs). Each interface has its own reference
//! count, as on Windows, and the buffer lives while one of them is above
//! 0. Releasing the DirectSound object releases its buffers.
//!
//! The device is as on Windows 95 with DirectX 5 or 6 and a sound card
//! without hardware mixing: buffers are mixed in software, at the primary
//! buffer's format. It has no capture device (capture.rs).
//!
//! Notifications, and the end of a buffer that doesn't loop, are seen
//! when time passes (`tick`, from thread::wake) or the program asks where
//! a buffer is: the events are set then, by the emulated time at which
//! the play cursor passed their positions.
//!
//! (Phase 3 of docs/native-windows.md.)

mod buffer;
mod capture;
pub mod device;
mod ds3d;
mod sound;

use super::com::{self, Guid};
use super::stubs::{Api, Conv, Dll, Vtable};
use crate::cpu::Cpu;
use device::Format;
pub use sound::enum_step;

pub static DLL: Dll = Dll {
    name: "DSOUND.DLL",
    base: 0xBFA4_0000,
    apis: APIS,
    vars: &[],
    init: None,
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

/// A method of an interface (not exported).
const fn method(name: &'static str, args: u8, handler: super::stubs::Handler) -> Api {
    stdcall(name, 0, args, handler)
}

/// The functions, by DSOUND.DLL's ordinals. Append only (see `stubs`).
static APIS: &[Api] = &[
    stdcall("DllCanUnloadNow", 4, 0, dll_can_unload_now),
    stdcall("DllGetClassObject", 5, 3, dll_get_class_object),
    stdcall("DirectSoundCreate", 1, 3, sound::direct_sound_create),
    stdcall("DirectSoundEnumerateA", 2, 2, sound::enumerate_a),
    stdcall("DirectSoundEnumerateW", 3, 2, sound::enumerate_w),
    stdcall("DirectSoundCaptureCreate", 6, 3, capture::create),
    stdcall("DirectSoundCaptureEnumerateA", 7, 2, capture::enumerate),
    stdcall("DirectSoundCaptureEnumerateW", 8, 2, capture::enumerate),
    method("DS_QueryInterface", 3, query_interface),
    method("DS_AddRef", 1, add_ref),
    method("DS_Release", 1, release),
    method("IDirectSound_CreateSoundBuffer", 4, sound::create_sound_buffer),
    method("IDirectSound_GetCaps", 2, sound::get_caps),
    method("IDirectSound_DuplicateSoundBuffer", 3, sound::duplicate),
    method("IDirectSound_SetCooperativeLevel", 3, sound::set_cooperative_level),
    method("IDirectSound_Compact", 1, sound::compact),
    method("IDirectSound_GetSpeakerConfig", 2, sound::get_speaker_config),
    method("IDirectSound_SetSpeakerConfig", 2, sound::set_speaker_config),
    method("IDirectSound_Initialize", 2, sound::initialize),
    method("IDirectSoundBuffer_GetCaps", 2, buffer::get_caps),
    method("IDirectSoundBuffer_GetCurrentPosition", 3, buffer::get_position),
    method("IDirectSoundBuffer_GetFormat", 4, buffer::get_format),
    method("IDirectSoundBuffer_GetVolume", 2, buffer::get_volume),
    method("IDirectSoundBuffer_GetPan", 2, buffer::get_pan),
    method("IDirectSoundBuffer_GetFrequency", 2, buffer::get_frequency),
    method("IDirectSoundBuffer_GetStatus", 2, buffer::get_status),
    method("IDirectSoundBuffer_Initialize", 3, buffer::initialize),
    method("IDirectSoundBuffer_Lock", 8, buffer::lock),
    method("IDirectSoundBuffer_Play", 4, buffer::play),
    method("IDirectSoundBuffer_SetCurrentPosition", 2, buffer::set_position),
    method("IDirectSoundBuffer_SetFormat", 2, buffer::set_format),
    method("IDirectSoundBuffer_SetVolume", 2, buffer::set_volume),
    method("IDirectSoundBuffer_SetPan", 2, buffer::set_pan),
    method("IDirectSoundBuffer_SetFrequency", 2, buffer::set_frequency),
    method("IDirectSoundBuffer_Stop", 1, buffer::stop),
    method("IDirectSoundBuffer_Unlock", 5, buffer::unlock),
    method("IDirectSoundBuffer_Restore", 1, buffer::restore),
    method("IDirectSound3DBuffer_GetAllParameters", 2, ds3d::buffer_get_all),
    method("IDirectSound3DBuffer_GetConeAngles", 3, ds3d::get_cone_angles),
    method("IDirectSound3DBuffer_GetConeOrientation", 2, ds3d::get_cone_orientation),
    method("IDirectSound3DBuffer_GetConeOutsideVolume", 2, ds3d::get_cone_outside_volume),
    method("IDirectSound3DBuffer_GetMaxDistance", 2, ds3d::get_max_distance),
    method("IDirectSound3DBuffer_GetMinDistance", 2, ds3d::get_min_distance),
    method("IDirectSound3DBuffer_GetMode", 2, ds3d::get_mode),
    method("IDirectSound3DBuffer_GetPosition", 2, ds3d::buffer_get_position),
    method("IDirectSound3DBuffer_GetVelocity", 2, ds3d::buffer_get_velocity),
    method("IDirectSound3DBuffer_SetAllParameters", 3, ds3d::buffer_set_all),
    method("IDirectSound3DBuffer_SetConeAngles", 4, ds3d::set_cone_angles),
    method("IDirectSound3DBuffer_SetConeOrientation", 5, ds3d::set_cone_orientation),
    method("IDirectSound3DBuffer_SetConeOutsideVolume", 3, ds3d::set_cone_outside_volume),
    method("IDirectSound3DBuffer_SetMaxDistance", 3, ds3d::set_max_distance),
    method("IDirectSound3DBuffer_SetMinDistance", 3, ds3d::set_min_distance),
    method("IDirectSound3DBuffer_SetMode", 3, ds3d::set_mode),
    method("IDirectSound3DBuffer_SetPosition", 5, ds3d::buffer_set_position),
    method("IDirectSound3DBuffer_SetVelocity", 5, ds3d::buffer_set_velocity),
    method("IDirectSound3DListener_GetAllParameters", 2, ds3d::listener_get_all),
    method("IDirectSound3DListener_GetDistanceFactor", 2, ds3d::get_distance_factor),
    method("IDirectSound3DListener_GetDopplerFactor", 2, ds3d::get_doppler_factor),
    method("IDirectSound3DListener_GetOrientation", 3, ds3d::get_orientation),
    method("IDirectSound3DListener_GetPosition", 2, ds3d::listener_get_position),
    method("IDirectSound3DListener_GetRolloffFactor", 2, ds3d::get_rolloff_factor),
    method("IDirectSound3DListener_GetVelocity", 2, ds3d::listener_get_velocity),
    method("IDirectSound3DListener_SetAllParameters", 3, ds3d::listener_set_all),
    method("IDirectSound3DListener_SetDistanceFactor", 3, ds3d::set_distance_factor),
    method("IDirectSound3DListener_SetDopplerFactor", 3, ds3d::set_doppler_factor),
    method("IDirectSound3DListener_SetOrientation", 8, ds3d::set_orientation),
    method("IDirectSound3DListener_SetPosition", 5, ds3d::listener_set_position),
    method("IDirectSound3DListener_SetRolloffFactor", 3, ds3d::set_rolloff_factor),
    method("IDirectSound3DListener_SetVelocity", 5, ds3d::listener_set_velocity),
    method("IDirectSound3DListener_CommitDeferredSettings", 1, ds3d::commit),
    method("IDirectSoundNotify_SetNotificationPositions", 3, buffer::set_notifications),
    method("IKsPropertySet_Get", 8, buffer::property_get),
    method("IKsPropertySet_Set", 7, buffer::property_set),
    method("IKsPropertySet_QuerySupport", 4, buffer::property_query),
    method("IDirectSoundCapture_CreateCaptureBuffer", 4, capture::create_buffer),
    method("IDirectSoundCapture_GetCaps", 2, capture::get_caps),
    method("IDirectSoundCapture_Initialize", 2, capture::initialize),
];

const QUERY: &str = "DS_QueryInterface";
const ADD_REF: &str = "DS_AddRef";
const RELEASE: &str = "DS_Release";

/// The interfaces' vtables, by the places `SOUND` and the others name.
static VTABLES: &[Vtable] = &[
    Vtable {
        name: "IDirectSound",
        methods: &[
            QUERY,
            ADD_REF,
            RELEASE,
            "IDirectSound_CreateSoundBuffer",
            "IDirectSound_GetCaps",
            "IDirectSound_DuplicateSoundBuffer",
            "IDirectSound_SetCooperativeLevel",
            "IDirectSound_Compact",
            "IDirectSound_GetSpeakerConfig",
            "IDirectSound_SetSpeakerConfig",
            "IDirectSound_Initialize",
        ],
    },
    Vtable {
        name: "IDirectSoundBuffer",
        methods: &[
            QUERY,
            ADD_REF,
            RELEASE,
            "IDirectSoundBuffer_GetCaps",
            "IDirectSoundBuffer_GetCurrentPosition",
            "IDirectSoundBuffer_GetFormat",
            "IDirectSoundBuffer_GetVolume",
            "IDirectSoundBuffer_GetPan",
            "IDirectSoundBuffer_GetFrequency",
            "IDirectSoundBuffer_GetStatus",
            "IDirectSoundBuffer_Initialize",
            "IDirectSoundBuffer_Lock",
            "IDirectSoundBuffer_Play",
            "IDirectSoundBuffer_SetCurrentPosition",
            "IDirectSoundBuffer_SetFormat",
            "IDirectSoundBuffer_SetVolume",
            "IDirectSoundBuffer_SetPan",
            "IDirectSoundBuffer_SetFrequency",
            "IDirectSoundBuffer_Stop",
            "IDirectSoundBuffer_Unlock",
            "IDirectSoundBuffer_Restore",
        ],
    },
    Vtable {
        name: "IDirectSound3DBuffer",
        methods: &[
            QUERY,
            ADD_REF,
            RELEASE,
            "IDirectSound3DBuffer_GetAllParameters",
            "IDirectSound3DBuffer_GetConeAngles",
            "IDirectSound3DBuffer_GetConeOrientation",
            "IDirectSound3DBuffer_GetConeOutsideVolume",
            "IDirectSound3DBuffer_GetMaxDistance",
            "IDirectSound3DBuffer_GetMinDistance",
            "IDirectSound3DBuffer_GetMode",
            "IDirectSound3DBuffer_GetPosition",
            "IDirectSound3DBuffer_GetVelocity",
            "IDirectSound3DBuffer_SetAllParameters",
            "IDirectSound3DBuffer_SetConeAngles",
            "IDirectSound3DBuffer_SetConeOrientation",
            "IDirectSound3DBuffer_SetConeOutsideVolume",
            "IDirectSound3DBuffer_SetMaxDistance",
            "IDirectSound3DBuffer_SetMinDistance",
            "IDirectSound3DBuffer_SetMode",
            "IDirectSound3DBuffer_SetPosition",
            "IDirectSound3DBuffer_SetVelocity",
        ],
    },
    Vtable {
        name: "IDirectSound3DListener",
        methods: &[
            QUERY,
            ADD_REF,
            RELEASE,
            "IDirectSound3DListener_GetAllParameters",
            "IDirectSound3DListener_GetDistanceFactor",
            "IDirectSound3DListener_GetDopplerFactor",
            "IDirectSound3DListener_GetOrientation",
            "IDirectSound3DListener_GetPosition",
            "IDirectSound3DListener_GetRolloffFactor",
            "IDirectSound3DListener_GetVelocity",
            "IDirectSound3DListener_SetAllParameters",
            "IDirectSound3DListener_SetDistanceFactor",
            "IDirectSound3DListener_SetDopplerFactor",
            "IDirectSound3DListener_SetOrientation",
            "IDirectSound3DListener_SetPosition",
            "IDirectSound3DListener_SetRolloffFactor",
            "IDirectSound3DListener_SetVelocity",
            "IDirectSound3DListener_CommitDeferredSettings",
        ],
    },
    Vtable {
        name: "IDirectSoundNotify",
        methods: &[
            QUERY,
            ADD_REF,
            RELEASE,
            "IDirectSoundNotify_SetNotificationPositions",
        ],
    },
    Vtable {
        name: "IKsPropertySet",
        methods: &[
            QUERY,
            ADD_REF,
            RELEASE,
            "IKsPropertySet_Get",
            "IKsPropertySet_Set",
            "IKsPropertySet_QuerySupport",
        ],
    },
    Vtable {
        name: "IDirectSoundCapture",
        methods: &[
            QUERY,
            ADD_REF,
            RELEASE,
            "IDirectSoundCapture_CreateCaptureBuffer",
            "IDirectSoundCapture_GetCaps",
            "IDirectSoundCapture_Initialize",
        ],
    },
];

/// The places of the vtables in `VTABLES`. A buffer's interfaces are at
/// `BUFFER` and after, in the order of `Buffer::ifaces`.
const SOUND: usize = 0;
const BUFFER: usize = 1;
const CAPTURE: usize = 6;

/// A buffer's interfaces, by their place in `Buffer::ifaces`.
const IFACE_BUFFER: usize = 0;
const IFACE_3D: usize = 1;
const IFACE_LISTENER: usize = 2;
const IFACE_NOTIFY: usize = 3;
const IFACE_PROPERTIES: usize = 4;
const IFACES: usize = 5;

/// DirectSound's HRESULTs.
const fn dserr(code: u32) -> u32 {
    0x8878_0000 | code
}
pub const DS_OK: u32 = 0;
pub const DSERR_CONTROLUNAVAIL: u32 = dserr(30);
pub const DSERR_INVALIDPARAM: u32 = com::E_INVALIDARG;
pub const DSERR_INVALIDCALL: u32 = dserr(50);
pub const DSERR_PRIOLEVELNEEDED: u32 = dserr(70);
pub const DSERR_BADFORMAT: u32 = dserr(100);
pub const DSERR_NODRIVER: u32 = dserr(120);
pub const DSERR_ALREADYINITIALIZED: u32 = dserr(130);
pub const DSERR_UNINITIALIZED: u32 = dserr(170);
pub const DSERR_NOAGGREGATION: u32 = com::CLASS_E_NOAGGREGATION;

/// The interfaces' IIDs.
const fn dx_guid(d1: u32) -> Guid {
    Guid::new(
        d1,
        0x4981,
        0x11CE,
        [0xA5, 0x21, 0x00, 0x20, 0xAF, 0x0B, 0xE5, 0x60],
    )
}
pub const IID_IDIRECTSOUND: Guid = dx_guid(0x279A_FA83);
pub const IID_IDIRECTSOUND3DLISTENER: Guid = dx_guid(0x279A_FA84);
pub const IID_IDIRECTSOUNDBUFFER: Guid = dx_guid(0x279A_FA85);
pub const IID_IDIRECTSOUND3DBUFFER: Guid = dx_guid(0x279A_FA86);
const fn dx5_guid(d1: u32) -> Guid {
    Guid::new(
        d1,
        0x89CD,
        0x11D0,
        [0xAF, 0x08, 0x00, 0xA0, 0xC9, 0x25, 0xCD, 0x16],
    )
}
pub const CLSID_DIRECTSOUNDCAPTURE: Guid = dx5_guid(0xB021_0780);
pub const IID_IDIRECTSOUNDCAPTURE: Guid = dx5_guid(0xB021_0781);
pub const IID_IDIRECTSOUNDNOTIFY: Guid = dx5_guid(0xB021_0783);
pub const IID_IKSPROPERTYSET: Guid = Guid::new(
    0x31EF_AC30,
    0x515C,
    0x11D0,
    [0xA9, 0xAA, 0x00, 0xAA, 0x00, 0x61, 0xBE, 0x93],
);
pub const CLSID_DIRECTSOUND: Guid = Guid::new(
    0x47D4_D946,
    0x62E8,
    0x11CF,
    [0x93, 0xBC, 0x44, 0x45, 0x53, 0x54, 0x00, 0x00],
);

/// The classes CoCreateInstance makes of DSOUND's (com.rs).
pub static CLASSES: &[com::Class] = &[
    com::Class {
        clsid: CLSID_DIRECTSOUND,
        dll: "DSOUND.DLL",
        name: "DirectSound",
        create: sound::create_instance,
    },
    com::Class {
        clsid: CLSID_DIRECTSOUNDCAPTURE,
        dll: "DSOUND.DLL",
        name: "DirectSoundCapture",
        create: capture::create_instance,
    },
];

/// A DirectSound object.
#[derive(Clone, Debug, Default)]
pub struct Sound {
    pub id: u32,
    pub pid: u32,
    /// Its IDirectSound, and that interface's reference count.
    pub iface: u32,
    pub refs: u32,
    /// Whether it has a device: DirectSoundCreate's have, CoCreateInstance's
    /// have once Initialize gave them one.
    pub initialized: bool,
    /// Its cooperative level (DSSCL_), 0 until SetCooperativeLevel, and its
    /// window.
    pub level: u32,
    pub hwnd: u32,
    /// Its primary buffer, by its id, or 0.
    pub primary: u32,
    /// The listener of its 3D sound, and the settings deferred for it.
    pub listener: ds3d::Listener,
    pub deferred: Option<ds3d::Listener>,
}

/// A sound buffer: primary or secondary.
#[derive(Clone, Debug, Default)]
pub struct Buffer {
    pub id: u32,
    pub pid: u32,
    /// Its DirectSound object, by its id.
    pub sound: u32,
    pub primary: bool,
    /// Its DSBCAPS_ flags.
    pub flags: u32,
    /// A secondary buffer's format (the primary has the device's).
    pub format: Format,
    /// Its bytes, and its samples' memory: by its id in `State::samples`,
    /// and its address.
    pub size: u32,
    pub samples: u32,
    pub addr: u32,
    /// Its interfaces' blocks (0 where it has none yet) and their
    /// reference counts, by the `IFACE_` places.
    pub ifaces: [u32; IFACES],
    pub refs: [u32; IFACES],
    /// Its rate in frames a second, its volume (hundredths of a decibel)
    /// and pan.
    pub frequency: u32,
    pub volume: i32,
    pub pan: i32,
    pub playing: bool,
    pub looping: bool,
    /// Where it was at the PIT tick `tick`, in frames (32.32), counted on
    /// past its end as it loops; and how far it has been seen to have
    /// played, for its notifications.
    pub tick: u64,
    pub pos: u64,
    pub seen: u64,
    /// Its notifications: offsets and their events' objects, which the
    /// buffer holds.
    pub notes: Vec<(u32, u32)>,
    /// Its 3D settings, and those deferred.
    pub params: ds3d::Params,
    pub deferred: Option<ds3d::Params>,
}

/// The memory of a buffer's samples, which a buffer and its duplicates
/// share: in the process `pid`, at `addr`, `refs` buffers using it.
#[derive(Clone, Debug, Default)]
pub struct Samples {
    pub id: u32,
    pub pid: u32,
    pub addr: u32,
    pub refs: u32,
}

/// DirectSound's state: the system's.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub sounds: Vec<Sound>,
    pub buffers: Vec<Buffer>,
    pub samples: Vec<Samples>,
    pub captures: Vec<capture::Capture>,
    /// The device's primary format, 0 for the default; its speaker
    /// configuration, 0 for the default; and the primary buffer's volume
    /// and pan.
    pub format: Format,
    pub speakers: u32,
    pub volume: i32,
    pub pan: i32,
    /// The last id given to an object.
    pub last_id: u32,
    /// When a buffer is next to need `tick` (a notification, an end), in
    /// PIT ticks.
    pub due: Option<u64>,
    /// Whether `tick` runs, so that what it does doesn't run it again.
    ticking: bool,
}

/// The primary buffer's format when none is set: 22050 Hz, 8 bits,
/// stereo, as Windows 95 has it.
const DEFAULT_FORMAT: Format = Format::pcm(22050, 8, 2);

impl State {
    fn next_id(&mut self) -> u32 {
        self.last_id += 1;
        self.last_id
    }

    /// The device's format: the primary buffer's.
    pub fn primary_format(&self) -> Format {
        if self.format.rate == 0 {
            DEFAULT_FORMAT
        } else {
            self.format
        }
    }

    fn sound(&self, id: u32) -> Option<usize> {
        self.sounds.iter().position(|s| s.id == id)
    }

    fn buffer(&self, id: u32) -> Option<usize> {
        self.buffers.iter().position(|b| b.id == id)
    }
}

fn state(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.dsound
}

fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("DSOUND is a built-in DLL")
}

/// A process ended: what it had goes. Its memory went with it.
pub fn process_ended(cpu: &mut Cpu, pid: u32, _tids: &[u32]) {
    let ds = state(cpu);
    let held: Vec<u32> = ds
        .buffers
        .iter()
        .filter(|b| b.pid == pid)
        .flat_map(|b| b.notes.iter().map(|&(_, e)| e))
        .collect();
    ds.sounds.retain(|s| s.pid != pid);
    ds.buffers.retain(|b| b.pid != pid);
    ds.samples.retain(|s| s.pid != pid);
    ds.captures.retain(|c| c.pid != pid);
    for id in held {
        super::object::objects(cpu).release(id);
    }
}

fn dll_can_unload_now(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let pid = super::user32::current_pid(cpu);
    let ds = state(cpu);
    let used = ds.sounds.iter().any(|s| s.pid == pid)
        || ds.buffers.iter().any(|b| b.pid == pid)
        || ds.captures.iter().any(|c| c.pid == pid);
    Some(if used { com::S_FALSE } else { com::S_OK })
}

/// DllGetClassObject(clsid, iid, out): DSOUND's, which fails with
/// DSERR_INVALIDPARAM where OLE32's would with E_POINTER.
fn dll_get_class_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[2] == 0 {
        return Some(DSERR_INVALIDPARAM);
    }
    Some(super::ole32::dll_get_class_object(cpu, DLL.name, args))
}

/// Which interface of DSOUND's `this` is: its vtable's place and its
/// object's id.
fn interface(cpu: &mut Cpu, this: u32) -> Option<(usize, u32)> {
    com::interface_of(cpu, dll_index(), this)
}

/// A new interface block of the object `id`, with the vtable `vtable`.
fn new_interface(cpu: &mut Cpu, vtable: usize, id: u32) -> Option<u32> {
    com::new_interface(cpu, dll_index(), vtable, id, 0)
}

/// IUnknown::QueryInterface(this, iid, out), for every interface here.
fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        return Some(DSERR_INVALIDPARAM);
    };
    let Some((vtable, id)) = interface(cpu, this) else {
        return Some(DSERR_INVALIDPARAM);
    };
    if out == 0 || !com::write_out(cpu, out, 0) {
        return Some(DSERR_INVALIDPARAM);
    }
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(DSERR_INVALIDPARAM);
    };
    Some(match vtable {
        SOUND => sound::query(cpu, id, &iid, out),
        CAPTURE => capture::query(cpu, id, &iid, out),
        _ => buffer::query(cpu, id, &iid, out),
    })
}

/// IUnknown::AddRef(this): the count of the interface `this`.
fn add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((vtable, id)) = interface(cpu, args[0]) else {
        return Some(0);
    };
    let ds = state(cpu);
    Some(match vtable {
        SOUND => ds.sound(id).map_or(0, |i| {
            ds.sounds[i].refs += 1;
            ds.sounds[i].refs
        }),
        CAPTURE => capture::add_ref(ds, id),
        _ => ds.buffer(id).map_or(0, |i| {
            let kind = vtable - BUFFER;
            ds.buffers[i].refs[kind] += 1;
            ds.buffers[i].refs[kind]
        }),
    })
}

/// IUnknown::Release(this): the count of the interface `this` left; the
/// object goes when it has none left.
fn release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((vtable, id)) = interface(cpu, args[0]) else {
        return Some(0);
    };
    Some(match vtable {
        SOUND => sound::release(cpu, id),
        CAPTURE => capture::release(cpu, id),
        _ => buffer::release(cpu, id, vtable - BUFFER),
    })
}

/// Whether a buffer plays, so the device is to mix.
pub fn playing(ds: &State) -> bool {
    ds.buffers.iter().any(|b| b.playing)
}

/// Time passed (winmm::tick, from thread::wake): the buffers that
/// passed their notifications' positions set their events, and those
/// that played to their ends stop.
pub fn tick(cpu: &mut Cpu) {
    if state(cpu).ticking || !playing(state(cpu)) {
        return;
    }
    state(cpu).ticking = true;
    device::catch_up(cpu);
    let ids: Vec<u32> = state(cpu).buffers.iter().map(|b| b.id).collect();
    for id in ids {
        buffer::settle(cpu, id);
    }
    reschedule(cpu);
    state(cpu).ticking = false;
}

/// Work out when `tick` is next due, and have the clock stop then.
pub fn reschedule(cpu: &mut Cpu) {
    let now = cpu.bus.clock.now_ticks();
    let due = state(cpu)
        .buffers
        .iter()
        .filter_map(buffer::next_event)
        .filter(|&t| t > now)
        .min();
    state(cpu).due = due;
    cpu.bus.reschedule();
}

/// When the device is next to need `tick`, in PIT ticks, for the clock.
pub fn next_event(ds: &State, now: u64) -> Option<u64> {
    ds.due.filter(|&t| t > now)
}

/// DirectSound's sound for the host's frames `first` to `first +
/// frames`. None if no buffer plays.
fn render(bus: &crate::bus::Bus, first: u64, frames: usize) -> Option<Vec<(f32, f32)>> {
    buffer::render(bus, first, frames)
}

crate::state_fields!(Sound {
    id,
    pid,
    iface,
    refs,
    initialized,
    level,
    hwnd,
    primary,
    listener,
    deferred
});
crate::state_fields!(Buffer {
    id,
    pid,
    sound,
    primary,
    flags,
    format,
    size,
    samples,
    addr,
    ifaces,
    refs,
    frequency,
    volume,
    pan,
    playing,
    looping,
    tick,
    pos,
    seen,
    notes,
    params,
    deferred
});
crate::state_fields!(Samples {
    id,
    pid,
    addr,
    refs
});
crate::state_fields!(Format {
    tag,
    channels,
    rate,
    bits
});
crate::state_fields!(State {
    sounds,
    buffers,
    samples,
    captures,
    format,
    speakers,
    volume,
    pan,
    last_id,
    due
} skip {
    ticking
});
