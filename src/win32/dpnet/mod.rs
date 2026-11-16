//! DPNET.DLL: DirectPlay 8, which DirectX 8 brought, on a machine whose
//! network protocols can't open a connection. Its classes are the peer
//! (IDirectPlay8Peer), the lobbied application
//! (IDirectPlay8LobbiedApplication) and the address
//! (IDirectPlay8Address), which CoCreateInstance, DllGetClassObject and
//! DirectPlay8Create make.
//!
//! The built-in WinSock opens no sockets (wsock32.rs), so no service
//! provider of DirectPlay 8 can start: EnumServiceProviders lists the
//! four DirectX 8.1 registers (TCP/IP, IPX, modem, serial) only when the
//! program asks for every registered one, and a peer can't host, join or
//! look for a session. What needs a session fails as DirectPlay 8 does
//! with none, so a game that offers network play next to playing alone
//! falls back to playing alone. DPLAYX's sessions over the LAN room
//! (dplayx/) don't reach DirectPlay 8 yet.
//!
//! The lobbied application registers a program for lobbies under
//! HKLM\Software\Microsoft\DirectPlay8\Applications, as DirectPlay 8
//! does. No lobby launched the program, so it has no lobby connection.
//!
//! peer.rs has the peer's methods, lobbied.rs the lobbied application's
//! and address.rs the address's. Each object has one interface: IUnknown
//! is that interface, and its block (com.rs) holds the object's id.

mod address;
mod lobbied;
mod peer;

use super::com::{self, CLASS_E_NOAGGREGATION, E_NOINTERFACE, E_OUTOFMEMORY, Guid};
use super::stubs::{Api, Conv, Dll, Vtable};
use crate::cpu::Cpu;

pub use address::Component;

pub static DLL: Dll = Dll {
    name: "DPNET.DLL",
    base: 0xBF52_0000,
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

/// A method: DPNET's own code, not exported. `args` counts `this`.
const fn method(name: &'static str, args: u8, handler: super::stubs::Handler) -> Api {
    stdcall(name, 0, args, handler)
}

/// The functions, by DPNET.DLL's ordinals, then the methods. Append only
/// (see `stubs`).
static APIS: &[Api] = &[
    stdcall("DirectPlay8Create", 1, 3, direct_play8_create),
    stdcall("DllCanUnloadNow", 2, 0, dll_can_unload_now),
    stdcall("DllGetClassObject", 3, 3, dll_get_class_object),
    stdcall("DllRegisterServer", 4, 0, dll_register_server),
    stdcall("DllUnregisterServer", 5, 0, dll_register_server),
    // IUnknown, of every interface.
    method("IUnknown_QueryInterface", 3, query_interface),
    method("IUnknown_AddRef", 1, add_ref),
    method("IUnknown_Release", 1, release),
    // IDirectPlay8Peer.
    method("Peer_Initialize", 4, peer::initialize),
    method("Peer_EnumServiceProviders", 7, peer::enum_service_providers),
    method("Peer_CancelAsyncOperation", 3, peer::cancel_async_operation),
    method("Peer_Connect", 12, peer::connect),
    method("Peer_SendTo", 8, peer::no_connection),
    method("Peer_GetSendQueueInfo", 5, peer::no_connection),
    method("Peer_Host", 8, peer::host),
    method("Peer_GetApplicationDesc", 4, peer::no_connection),
    method("Peer_SetApplicationDesc", 3, peer::no_connection),
    method("Peer_CreateGroup", 6, peer::no_connection),
    method("Peer_DestroyGroup", 5, peer::no_connection),
    method("Peer_AddPlayerToGroup", 6, peer::no_connection),
    method("Peer_RemovePlayerFromGroup", 6, peer::no_connection),
    method("Peer_SetGroupInfo", 6, peer::no_connection),
    method("Peer_GetGroupInfo", 5, peer::no_connection),
    method("Peer_EnumPlayersAndGroups", 4, peer::no_connection),
    method("Peer_EnumGroupMembers", 5, peer::no_connection),
    method("Peer_SetPeerInfo", 5, peer::set_peer_info),
    method("Peer_GetPeerInfo", 5, peer::no_connection),
    method("Peer_GetPeerAddress", 4, peer::no_connection),
    method("Peer_GetLocalHostAddresses", 4, peer::no_connection),
    method("Peer_Close", 2, peer::close),
    method("Peer_EnumHosts", 12, peer::enum_hosts),
    method("Peer_DestroyPeer", 5, peer::no_connection),
    method("Peer_ReturnBuffer", 3, peer::return_buffer),
    method("Peer_GetPlayerContext", 4, peer::no_connection),
    method("Peer_GetGroupContext", 4, peer::no_connection),
    method("Peer_GetCaps", 3, peer::get_caps),
    method("Peer_SetCaps", 3, peer::set_caps),
    method("Peer_SetSPCaps", 4, peer::sp_caps),
    method("Peer_GetSPCaps", 4, peer::sp_caps),
    method("Peer_GetConnectionInfo", 4, peer::no_connection),
    method("Peer_RegisterLobby", 4, peer::register_lobby),
    method("Peer_TerminateSession", 4, peer::no_connection),
    // IDirectPlay8LobbiedApplication.
    method("Lobbied_Initialize", 5, lobbied::initialize),
    method("Lobbied_RegisterProgram", 3, lobbied::register_program),
    method("Lobbied_UnRegisterProgram", 3, lobbied::unregister_program),
    method("Lobbied_Send", 5, lobbied::no_connection),
    method("Lobbied_SetAppAvailable", 3, lobbied::set_app_available),
    method("Lobbied_UpdateStatus", 4, lobbied::no_connection),
    method("Lobbied_Close", 2, lobbied::close),
    method("Lobbied_GetConnectionSettings", 5, lobbied::no_connection),
    method("Lobbied_SetConnectionSettings", 4, lobbied::no_connection),
    // IDirectPlay8Address.
    method("Address_BuildFromURLW", 2, address::build_from_url_w),
    method("Address_BuildFromURLA", 2, address::build_from_url_a),
    method("Address_Duplicate", 2, address::duplicate),
    method("Address_SetEqual", 2, address::set_equal),
    method("Address_IsEqual", 2, address::is_equal),
    method("Address_Clear", 1, address::clear),
    method("Address_GetURLW", 3, address::get_url_w),
    method("Address_GetURLA", 3, address::get_url_a),
    method("Address_GetSP", 2, address::get_sp),
    method("Address_GetUserData", 3, address::get_user_data),
    method("Address_SetSP", 2, address::set_sp),
    method("Address_SetUserData", 3, address::set_user_data),
    method("Address_GetNumComponents", 2, address::get_num_components),
    method(
        "Address_GetComponentByName",
        5,
        address::get_component_by_name,
    ),
    method(
        "Address_GetComponentByIndex",
        7,
        address::get_component_by_index,
    ),
    method("Address_AddComponent", 5, address::add_component),
    method("Address_GetDevice", 2, address::get_device),
    method("Address_SetDevice", 2, address::set_device),
    method(
        "Address_BuildFromDPADDRESS",
        3,
        address::build_from_dpaddress,
    ),
];

/// IDirectPlay8Peer's methods in their order.
const PEER_METHODS: &[&str] = &[
    "IUnknown_QueryInterface",
    "IUnknown_AddRef",
    "IUnknown_Release",
    "Peer_Initialize",
    "Peer_EnumServiceProviders",
    "Peer_CancelAsyncOperation",
    "Peer_Connect",
    "Peer_SendTo",
    "Peer_GetSendQueueInfo",
    "Peer_Host",
    "Peer_GetApplicationDesc",
    "Peer_SetApplicationDesc",
    "Peer_CreateGroup",
    "Peer_DestroyGroup",
    "Peer_AddPlayerToGroup",
    "Peer_RemovePlayerFromGroup",
    "Peer_SetGroupInfo",
    "Peer_GetGroupInfo",
    "Peer_EnumPlayersAndGroups",
    "Peer_EnumGroupMembers",
    "Peer_SetPeerInfo",
    "Peer_GetPeerInfo",
    "Peer_GetPeerAddress",
    "Peer_GetLocalHostAddresses",
    "Peer_Close",
    "Peer_EnumHosts",
    "Peer_DestroyPeer",
    "Peer_ReturnBuffer",
    "Peer_GetPlayerContext",
    "Peer_GetGroupContext",
    "Peer_GetCaps",
    "Peer_SetCaps",
    "Peer_SetSPCaps",
    "Peer_GetSPCaps",
    "Peer_GetConnectionInfo",
    "Peer_RegisterLobby",
    "Peer_TerminateSession",
];

/// IDirectPlay8LobbiedApplication's methods in their order.
const LOBBIED_METHODS: &[&str] = &[
    "IUnknown_QueryInterface",
    "IUnknown_AddRef",
    "IUnknown_Release",
    "Lobbied_Initialize",
    "Lobbied_RegisterProgram",
    "Lobbied_UnRegisterProgram",
    "Lobbied_Send",
    "Lobbied_SetAppAvailable",
    "Lobbied_UpdateStatus",
    "Lobbied_Close",
    "Lobbied_GetConnectionSettings",
    "Lobbied_SetConnectionSettings",
];

/// IDirectPlay8Address's methods in their order.
const ADDRESS_METHODS: &[&str] = &[
    "IUnknown_QueryInterface",
    "IUnknown_AddRef",
    "IUnknown_Release",
    "Address_BuildFromURLW",
    "Address_BuildFromURLA",
    "Address_Duplicate",
    "Address_SetEqual",
    "Address_IsEqual",
    "Address_Clear",
    "Address_GetURLW",
    "Address_GetURLA",
    "Address_GetSP",
    "Address_GetUserData",
    "Address_SetSP",
    "Address_SetUserData",
    "Address_GetNumComponents",
    "Address_GetComponentByName",
    "Address_GetComponentByIndex",
    "Address_AddComponent",
    "Address_GetDevice",
    "Address_SetDevice",
    "Address_BuildFromDPADDRESS",
];

/// The vtables, by their places in this list, which `Class` numbers.
static VTABLES: &[Vtable] = &[
    Vtable {
        name: "IDirectPlay8Peer",
        methods: PEER_METHODS,
    },
    Vtable {
        name: "IDirectPlay8LobbiedApplication",
        methods: LOBBIED_METHODS,
    },
    Vtable {
        name: "IDirectPlay8Address",
        methods: ADDRESS_METHODS,
    },
];

/// The kinds of object, by their vtables' places in VTABLES.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Class {
    Peer = 0,
    Lobbied = 1,
    Address = 2,
}

impl Class {
    const ALL: [Class; 3] = [Class::Peer, Class::Lobbied, Class::Address];

    fn iid(self) -> Guid {
        match self {
            Class::Peer => IID_PEER,
            Class::Lobbied => IID_LOBBIED,
            Class::Address => IID_ADDRESS,
        }
    }

    fn of(code: u8) -> Option<Class> {
        Class::ALL.into_iter().find(|&c| c as u8 == code)
    }
}

const CLSID_PEER: Guid = Guid::new(
    0x286F_484D,
    0x375E,
    0x4458,
    [0xA2, 0x72, 0xB1, 0x38, 0xE2, 0xF8, 0x0A, 0x6A],
);
const IID_PEER: Guid = Guid::new(
    0x5102_DACF,
    0x241B,
    0x11D3,
    [0xAE, 0xA7, 0x00, 0x60, 0x97, 0xB0, 0x14, 0x11],
);
const CLSID_LOBBIED: Guid = Guid::new(
    0x6679_55AD,
    0x6B3B,
    0x43CA,
    [0xB9, 0x49, 0xBC, 0x69, 0xB5, 0xBA, 0xFF, 0x7F],
);
const IID_LOBBIED: Guid = Guid::new(
    0x8190_74A3,
    0x016C,
    0x11D3,
    [0xAE, 0x14, 0x00, 0x60, 0x97, 0xB0, 0x14, 0x11],
);
const CLSID_ADDRESS: Guid = Guid::new(
    0x934A_9523,
    0xA3CA,
    0x4BC5,
    [0xAD, 0xA0, 0xD6, 0xD9, 0x5D, 0x97, 0x94, 0x21],
);
const IID_ADDRESS: Guid = Guid::new(
    0x8378_3300,
    0x4063,
    0x4C8A,
    [0x9D, 0xB3, 0x82, 0x83, 0x0A, 0x7F, 0xEB, 0x31],
);

/// The classes CoCreateInstance makes of DPNET's (com.rs).
pub static CLASSES: &[com::Class] = &[
    com::Class {
        clsid: CLSID_PEER,
        dll: "DPNET.DLL",
        name: "DirectPlay8Peer",
        create: create_peer,
    },
    com::Class {
        clsid: CLSID_LOBBIED,
        dll: "DPNET.DLL",
        name: "DirectPlay8LobbiedApplication",
        create: create_lobbied,
    },
    com::Class {
        clsid: CLSID_ADDRESS,
        dll: "DPNET.DLL",
        name: "DirectPlay8Address",
        create: create_address,
    },
];

/// DirectPlay 8's HRESULTs: MAKE_DPNHRESULT, facility 15h, the codes
/// from 8000h on.
const DPN_OK: u32 = 0;
const fn dpnerr(code: u32) -> u32 {
    0x8015_8000 + code
}
const DPNSUCCESS_EQUAL: u32 = 0x0015_8005;
const DPNSUCCESS_NOTEQUAL: u32 = 0x0015_800A;
const DPNERR_ALREADYINITIALIZED: u32 = dpnerr(0x80);
const DPNERR_BUFFERTOOSMALL: u32 = dpnerr(0x100);
const DPNERR_DOESNOTEXIST: u32 = dpnerr(0x180);
const DPNERR_INVALIDADDRESSFORMAT: u32 = dpnerr(0x290);
const DPNERR_INVALIDDEVICEADDRESS: u32 = dpnerr(0x320);
const DPNERR_INVALIDFLAGS: u32 = dpnerr(0x340);
const DPNERR_INVALIDHANDLE: u32 = dpnerr(0x360);
const DPNERR_INVALIDHOSTADDRESS: u32 = dpnerr(0x370);
const DPNERR_INVALIDOBJECT: u32 = dpnerr(0x400);
const DPNERR_INVALIDURL: u32 = dpnerr(0x450);
const DPNERR_NOCONNECTION: u32 = dpnerr(0x480);
const DPNERR_NOTREGISTERED: u32 = dpnerr(0x550);
const DPNERR_UNINITIALIZED: u32 = dpnerr(0x640);
const DPNERR_INVALIDPARAM: u32 = com::E_INVALIDARG;
const DPNERR_INVALIDPOINTER: u32 = com::E_POINTER;
const DPNERR_UNSUPPORTED: u32 = com::E_NOTIMPL;

/// A DirectPlay 8 object.
#[derive(Clone, Debug, Default)]
pub struct Object {
    pub pid: u32,
    /// Its interface's block, and the references to it.
    pub this: u32,
    pub refs: u32,
    /// Its `Class`.
    pub class: u8,
    /// Whether a peer's or a lobbied application's Initialize ran, and
    /// no Close since.
    pub initialized: bool,
    /// A peer's DPN_CAPS after its size: flags, connect timeout, connect
    /// retries, time until keep-alive.
    pub caps: [u32; 4],
    /// An address's components, in the order they were added, and its
    /// user data.
    pub components: Vec<Component>,
    pub user_data: Vec<u8>,
}

crate::state_fields!(Object {
    pid,
    this,
    refs,
    class,
    initialized,
    caps,
    components,
    user_data
});

/// DPNET's state: the objects, by their ids.
#[derive(Clone, Debug, Default)]
pub struct State {
    objects: Vec<Option<Object>>,
}

/// The W32 section's version that first keeps DPNET's objects: an older
/// one had none.
pub const OBJECTS_SAVED: u16 = 74;

impl crate::savestate::State for State {
    fn save(&self, w: &mut crate::savestate::Writer) {
        crate::savestate::State::save(&self.objects, w);
    }

    fn load(&mut self, r: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        if r.version().is_some_and(|v| v < OBJECTS_SAVED) {
            *self = State::default();
            return Ok(());
        }
        crate::savestate::State::load(&mut self.objects, r)
    }
}

fn objects(cpu: &mut Cpu) -> &mut Vec<Option<Object>> {
    &mut cpu.bus.win32.dpnet.objects
}

/// The object `id`, which the method's `object!` found.
fn object(cpu: &mut Cpu, id: usize) -> &mut Object {
    objects(cpu)[id].as_mut().expect("a live object")
}

fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("DPNET is a built-in DLL")
}

/// A new object of `class` with one reference: its id and its
/// interface's block, or None when the heap is full.
fn new_object(cpu: &mut Cpu, class: Class) -> Option<(usize, u32)> {
    let list = objects(cpu);
    let id = list.iter().position(Option::is_none).unwrap_or(list.len());
    let this = com::new_interface(cpu, dll_index(), class as usize, id as u32, 0)?;
    let object = Object {
        pid: super::user32::current_pid(cpu),
        this,
        refs: 1,
        class: class as u8,
        caps: peer::DEFAULT_CAPS,
        ..Object::default()
    };
    let list = objects(cpu);
    if id == list.len() {
        list.push(Some(object));
    } else {
        list[id] = Some(object);
    }
    Some((id, this))
}

/// The object `this` is the interface of, and its class, or
/// DPNERR_INVALIDOBJECT when `this` isn't the interface of a live
/// object.
fn object_of(cpu: &mut Cpu, this: u32) -> Result<(usize, Class), u32> {
    let (vtable, id) = com::interface_of(cpu, dll_index(), this).ok_or(DPNERR_INVALIDOBJECT)?;
    let id = id as usize;
    match objects(cpu).get(id).and_then(Option::as_ref) {
        Some(o) if o.this == this && o.class as usize == vtable => {
            Ok((id, Class::of(o.class).ok_or(DPNERR_INVALIDOBJECT)?))
        }
        _ => Err(DPNERR_INVALIDOBJECT),
    }
}

/// The id of the object `this` is the interface of, or the method
/// returns DPNERR_INVALIDOBJECT.
macro_rules! object_id {
    ($cpu:expr, $this:expr) => {
        match super::object_of($cpu, $this) {
            Ok((id, _)) => id,
            Err(e) => return Some(e),
        }
    };
}
use object_id;

/// The id of the peer or lobbied application `this` is, or the method
/// returns DPNERR_INVALIDOBJECT; and DPNERR_UNINITIALIZED until its
/// Initialize.
macro_rules! initialized {
    ($cpu:expr, $this:expr) => {{
        let id = super::object_id!($cpu, $this);
        if !super::object($cpu, id).initialized {
            return Some(super::DPNERR_UNINITIALIZED);
        }
        id
    }};
}
use initialized;

/// The Initialize of a peer and of a lobbied application, whose one
/// flag is the same: the object `this` takes the handler `handler`,
/// which the messages of a session or a lobby would go to. There are
/// none, so it isn't kept.
fn initialize(cpu: &mut Cpu, this: u32, handler: u32, flags: u32) -> u32 {
    /// DPNINITIALIZE_DISABLEPARAMVAL and DPLINITIALIZE_DISABLEPARAMVAL.
    const DISABLEPARAMVAL: u32 = 1;
    let id = match object_of(cpu, this) {
        Ok((id, _)) => id,
        Err(e) => return e,
    };
    if handler == 0 {
        return DPNERR_INVALIDPOINTER;
    }
    if flags & !DISABLEPARAMVAL != 0 {
        return DPNERR_INVALIDFLAGS;
    }
    let o = object(cpu, id);
    if o.initialized {
        return DPNERR_ALREADYINITIALIZED;
    }
    o.initialized = true;
    DPN_OK
}

/// The process `pid` ended: its objects go with its heap.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    for slot in objects(cpu).iter_mut() {
        if slot.as_ref().is_some_and(|o| o.pid == pid) {
            *slot = None;
        }
    }
}

/// The Win32 session ended: DirectPlay 8 starts again with no objects.
pub fn session_ended(cpu: &mut Cpu) {
    cpu.bus.win32.dpnet = State::default();
}

/// Make an object of `class` as the interface `iid` at `out`.
fn create(cpu: &mut Cpu, class: Class, iid: &Guid, out: u32) -> u32 {
    if *iid != com::IID_IUNKNOWN && *iid != class.iid() {
        com::write_out(cpu, out, 0);
        return E_NOINTERFACE;
    }
    let this = new_object(cpu, class).map(|(_, this)| this);
    com::write_out(cpu, out, this.unwrap_or(0));
    if this.is_some() {
        DPN_OK
    } else {
        E_OUTOFMEMORY
    }
}

fn create_peer(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    create(cpu, Class::Peer, iid, out)
}

fn create_lobbied(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    create(cpu, Class::Lobbied, iid, out)
}

fn create_address(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    create(cpu, Class::Address, iid, out)
}

/// DirectPlay8Create(iid, out, outer): an object of the class whose
/// interface `iid` is, as DirectX 8.1's DPNET.DLL exports it besides
/// its classes. An IID of no class of DPNET's fails with
/// DPNERR_INVALIDPARAM, as in Wine's.
fn direct_play8_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [iid, out, outer] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    if !com::write_out(cpu, out, 0) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    if outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(DPNERR_INVALIDPOINTER);
    };
    match Class::ALL.into_iter().find(|c| c.iid() == iid) {
        Some(class) => Some(create(cpu, class, &iid, out)),
        None => Some(DPNERR_INVALIDPARAM),
    }
}

fn dll_can_unload_now(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(com::S_FALSE)
}

fn dll_get_class_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(super::ole32::dll_get_class_object(cpu, DLL.name, args))
}

fn dll_register_server(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(com::S_OK)
}

/// QueryInterface(this, iid, out): the object's own interface, for its
/// IID or IUnknown's.
fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    let (id, class) = match object_of(cpu, this) {
        Ok(found) => found,
        Err(e) => return Some(e),
    };
    if !com::write_out(cpu, out, 0) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    match Guid::read(cpu, iid) {
        Some(g) if g == com::IID_IUNKNOWN || g == class.iid() => {
            object(cpu, id).refs += 1;
            com::write_out(cpu, out, this);
            Some(DPN_OK)
        }
        _ => Some(E_NOINTERFACE),
    }
}

fn add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok((id, _)) = object_of(cpu, args[0]) else {
        return Some(0);
    };
    let o = object(cpu, id);
    o.refs += 1;
    Some(o.refs)
}

/// Release(this): the count left; the object goes with the last.
fn release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok((id, _)) = object_of(cpu, args[0]) else {
        return Some(0);
    };
    let o = object(cpu, id);
    o.refs = o.refs.saturating_sub(1);
    let refs = o.refs;
    if refs == 0 {
        let this = o.this;
        objects(cpu)[id] = None;
        com::free_interface(cpu, this);
    }
    Some(refs)
}

/// Whether `size` bytes at `at` can be written.
fn writable(cpu: &mut Cpu, at: u32, size: u32) -> bool {
    at != 0 && cpu.bus.guest_probe(at, size as usize, true)
}

/// The 32-bit number at `at`, if it can be read.
fn read_u32(cpu: &mut Cpu, at: u32) -> Option<u32> {
    (at != 0 && cpu.bus.guest_probe(at, 4, false)).then(|| cpu.bus.guest_read_32(at))
}

/// The string of 16-bit characters at `at`, without its zero, or None
/// if it can't be read whole.
fn read_wide(cpu: &mut Cpu, at: u32) -> Option<Vec<u16>> {
    /// Longer strings than any URL or path DirectPlay 8 takes.
    const LONGEST: u32 = 0x1_0000;
    if at == 0 {
        return None;
    }
    let mut units = Vec::new();
    for i in 0..LONGEST {
        let p = at.wrapping_add(2 * i);
        if !cpu.bus.guest_probe(p, 2, false) {
            return None;
        }
        match cpu.bus.guest_read_16(p) {
            0 => return Some(units),
            c => units.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_method_lists_name_methods_there_are() {
        for vtable in VTABLES {
            for name in vtable.methods {
                assert!(
                    APIS.iter().any(|a| a.name == *name),
                    "{} has no {name}",
                    vtable.name
                );
            }
        }
        let lengths: Vec<usize> = VTABLES.iter().map(|v| v.methods.len()).collect();
        // IUnknown's 3, then the interfaces' own 34, 9 and 19.
        assert_eq!(lengths, [37, 12, 22]);
    }

    #[test]
    fn the_objects_of_an_ended_process_go() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let object = |pid| {
            Some(Object {
                pid,
                ..Object::default()
            })
        };
        *objects(&mut cpu) = vec![object(1), object(2), None, object(1)];
        process_ended(&mut cpu, 1);
        let left: Vec<Option<u32>> = objects(&mut cpu)
            .iter()
            .map(|o| o.as_ref().map(|o| o.pid))
            .collect();
        assert_eq!(left, [None, Some(2), None, None]);
    }
}
