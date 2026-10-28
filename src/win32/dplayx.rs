//! DPLAYX.DLL: DirectPlay, as on a machine with no service provider it
//! can use. DirectPlay objects are made, by DirectPlayCreate and by
//! CoCreateInstance, as Windows 98 makes them with no network card;
//! games of DirectX 5 and 6 make one at startup even to play alone, and
//! quit when they can't. Such an object lists no connections and can't
//! be given one, so it has no session to find, open or play in: the
//! methods that need a connection say DPERR_UNINITIALIZED, as DirectPlay
//! does before InitializeConnection. There are no service providers to
//! enumerate, and no lobby launched the program (no lobby object).
//!
//! An object has three interfaces: IDirectPlay, the first one, which
//! DirectPlayCreate gives, and IDirectPlay4A and IDirectPlay4, whose
//! tables of methods begin with those of IDirectPlay2(A) and
//! IDirectPlay3(A), so they serve those too. Each interface has its own
//! reference count, and the object goes when all three are at 0.

use super::com::{self, CLASS_E_NOAGGREGATION, E_NOINTERFACE, E_OUTOFMEMORY, GUID_NULL, Guid};
use super::stubs::{Api, Conv, Dll, Vtable};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "DPLAYX.DLL",
    base: 0xBFAE_0000,
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

/// A method: DPLAYX's own code, not exported. `args` counts `this`.
const fn method(name: &'static str, args: u8, handler: super::stubs::Handler) -> Api {
    stdcall(name, 0, args, handler)
}

/// The functions, by DPLAYX.DLL's ordinals, then the methods. Append
/// only (see `stubs`). IDirectPlay4A's and IDirectPlay4's methods are
/// the same functions, as no method here reads or writes a string.
static APIS: &[Api] = &[
    stdcall("DirectPlayCreate", 1, 3, direct_play_create),
    stdcall("DirectPlayEnumerateA", 2, 2, direct_play_enumerate),
    stdcall("DirectPlayEnumerateW", 3, 2, direct_play_enumerate),
    stdcall("DirectPlayLobbyCreateA", 4, 5, direct_play_lobby_create),
    stdcall("DirectPlayLobbyCreateW", 5, 5, direct_play_lobby_create),
    stdcall("DirectPlayEnumerate", 9, 2, direct_play_enumerate),
    // IUnknown, of all three interfaces.
    method("IDirectPlay_QueryInterface", 3, query_interface),
    method("IDirectPlay_AddRef", 1, add_ref),
    method("IDirectPlay_Release", 1, release),
    // IDirectPlay2.
    method("IDirectPlay4_AddPlayerToGroup", 3, uninitialized),
    method("IDirectPlay4_Close", 1, close),
    method("IDirectPlay4_CreateGroup", 6, uninitialized),
    method("IDirectPlay4_CreatePlayer", 7, uninitialized),
    method("IDirectPlay4_DeletePlayerFromGroup", 3, uninitialized),
    method("IDirectPlay4_DestroyGroup", 2, uninitialized),
    method("IDirectPlay4_DestroyPlayer", 2, uninitialized),
    method("IDirectPlay4_EnumGroupPlayers", 6, uninitialized),
    method("IDirectPlay4_EnumGroups", 5, uninitialized),
    method("IDirectPlay4_EnumPlayers", 5, uninitialized),
    method("IDirectPlay4_EnumSessions", 6, needs_session),
    method("IDirectPlay4_GetCaps", 3, get_caps),
    method("IDirectPlay4_GetGroupData", 5, uninitialized),
    method("IDirectPlay4_GetGroupName", 4, uninitialized),
    method("IDirectPlay4_GetMessageCount", 3, uninitialized),
    method("IDirectPlay4_GetPlayerAddress", 4, uninitialized),
    method("IDirectPlay4_GetPlayerCaps", 4, uninitialized),
    method("IDirectPlay4_GetPlayerData", 5, uninitialized),
    method("IDirectPlay4_GetPlayerName", 4, uninitialized),
    method("IDirectPlay4_GetSessionDesc", 3, uninitialized),
    method("IDirectPlay4_Initialize", 2, initialize),
    method("IDirectPlay4_Open", 3, needs_session),
    method("IDirectPlay4_Receive", 6, receive),
    method("IDirectPlay4_Send", 6, uninitialized),
    method("IDirectPlay4_SetGroupData", 5, uninitialized),
    method("IDirectPlay4_SetGroupName", 4, uninitialized),
    method("IDirectPlay4_SetPlayerData", 5, uninitialized),
    method("IDirectPlay4_SetPlayerName", 4, uninitialized),
    method("IDirectPlay4_SetSessionDesc", 3, uninitialized),
    // IDirectPlay3.
    method("IDirectPlay4_AddGroupToGroup", 3, uninitialized),
    method("IDirectPlay4_CreateGroupInGroup", 7, uninitialized),
    method("IDirectPlay4_DeleteGroupFromGroup", 3, uninitialized),
    method("IDirectPlay4_EnumConnections", 5, enum_connections),
    method("IDirectPlay4_EnumGroupsInGroup", 6, uninitialized),
    method("IDirectPlay4_GetGroupConnectionSettings", 5, uninitialized),
    method(
        "IDirectPlay4_InitializeConnection",
        3,
        initialize_connection,
    ),
    method("IDirectPlay4_SecureOpen", 5, needs_session),
    method("IDirectPlay4_SendChatMessage", 5, uninitialized),
    method("IDirectPlay4_SetGroupConnectionSettings", 4, uninitialized),
    method("IDirectPlay4_StartSession", 3, uninitialized),
    method("IDirectPlay4_GetGroupFlags", 3, uninitialized),
    method("IDirectPlay4_GetGroupParent", 3, uninitialized),
    method("IDirectPlay4_GetPlayerAccount", 5, uninitialized),
    method("IDirectPlay4_GetPlayerFlags", 3, uninitialized),
    // IDirectPlay4.
    method("IDirectPlay4_GetGroupOwner", 3, uninitialized),
    method("IDirectPlay4_SetGroupOwner", 3, uninitialized),
    method("IDirectPlay4_SendEx", 10, uninitialized),
    method("IDirectPlay4_GetMessageQueue", 6, uninitialized),
    method("IDirectPlay4_CancelMessage", 3, uninitialized),
    method("IDirectPlay4_CancelPriority", 4, uninitialized),
    // IDirectPlay, the first interface.
    method("IDirectPlay_AddPlayerToGroup", 3, uninitialized),
    method("IDirectPlay_Close", 1, close),
    method("IDirectPlay_CreatePlayer", 5, uninitialized),
    method("IDirectPlay_CreateGroup", 4, uninitialized),
    method("IDirectPlay_DeletePlayerFromGroup", 3, uninitialized),
    method("IDirectPlay_DestroyPlayer", 2, uninitialized),
    method("IDirectPlay_DestroyGroup", 2, uninitialized),
    method("IDirectPlay_EnableNewPlayers", 2, uninitialized),
    method("IDirectPlay_EnumGroupPlayers", 5, uninitialized),
    method("IDirectPlay_EnumGroups", 5, uninitialized),
    method("IDirectPlay_EnumPlayers", 5, uninitialized),
    method("IDirectPlay_EnumSessions", 6, needs_session),
    method("IDirectPlay_GetCaps", 2, get_caps),
    method("IDirectPlay_GetMessageCount", 3, uninitialized),
    method("IDirectPlay_GetPlayerCaps", 3, uninitialized),
    method("IDirectPlay_GetPlayerName", 6, uninitialized),
    method("IDirectPlay_Initialize", 2, initialize),
    method("IDirectPlay_Open", 2, needs_session),
    method("IDirectPlay_Receive", 6, receive),
    method("IDirectPlay_SaveSession", 2, uninitialized),
    method("IDirectPlay_Send", 6, uninitialized),
    method("IDirectPlay_SetPlayerName", 4, uninitialized),
];

/// IDirectPlay4's methods in their order: IUnknown's 3, IDirectPlay2's
/// 29, IDirectPlay3's 15, then its own 6.
const PLAY4_METHODS: &[&str] = &[
    "IDirectPlay_QueryInterface",
    "IDirectPlay_AddRef",
    "IDirectPlay_Release",
    "IDirectPlay4_AddPlayerToGroup",
    "IDirectPlay4_Close",
    "IDirectPlay4_CreateGroup",
    "IDirectPlay4_CreatePlayer",
    "IDirectPlay4_DeletePlayerFromGroup",
    "IDirectPlay4_DestroyGroup",
    "IDirectPlay4_DestroyPlayer",
    "IDirectPlay4_EnumGroupPlayers",
    "IDirectPlay4_EnumGroups",
    "IDirectPlay4_EnumPlayers",
    "IDirectPlay4_EnumSessions",
    "IDirectPlay4_GetCaps",
    "IDirectPlay4_GetGroupData",
    "IDirectPlay4_GetGroupName",
    "IDirectPlay4_GetMessageCount",
    "IDirectPlay4_GetPlayerAddress",
    "IDirectPlay4_GetPlayerCaps",
    "IDirectPlay4_GetPlayerData",
    "IDirectPlay4_GetPlayerName",
    "IDirectPlay4_GetSessionDesc",
    "IDirectPlay4_Initialize",
    "IDirectPlay4_Open",
    "IDirectPlay4_Receive",
    "IDirectPlay4_Send",
    "IDirectPlay4_SetGroupData",
    "IDirectPlay4_SetGroupName",
    "IDirectPlay4_SetPlayerData",
    "IDirectPlay4_SetPlayerName",
    "IDirectPlay4_SetSessionDesc",
    "IDirectPlay4_AddGroupToGroup",
    "IDirectPlay4_CreateGroupInGroup",
    "IDirectPlay4_DeleteGroupFromGroup",
    "IDirectPlay4_EnumConnections",
    "IDirectPlay4_EnumGroupsInGroup",
    "IDirectPlay4_GetGroupConnectionSettings",
    "IDirectPlay4_InitializeConnection",
    "IDirectPlay4_SecureOpen",
    "IDirectPlay4_SendChatMessage",
    "IDirectPlay4_SetGroupConnectionSettings",
    "IDirectPlay4_StartSession",
    "IDirectPlay4_GetGroupFlags",
    "IDirectPlay4_GetGroupParent",
    "IDirectPlay4_GetPlayerAccount",
    "IDirectPlay4_GetPlayerFlags",
    "IDirectPlay4_GetGroupOwner",
    "IDirectPlay4_SetGroupOwner",
    "IDirectPlay4_SendEx",
    "IDirectPlay4_GetMessageQueue",
    "IDirectPlay4_CancelMessage",
    "IDirectPlay4_CancelPriority",
];

/// The vtables, by their places in this list (PLAY, PLAY_A, PLAY_W).
static VTABLES: &[Vtable] = &[
    Vtable {
        name: "IDirectPlay",
        methods: &[
            "IDirectPlay_QueryInterface",
            "IDirectPlay_AddRef",
            "IDirectPlay_Release",
            "IDirectPlay_AddPlayerToGroup",
            "IDirectPlay_Close",
            "IDirectPlay_CreatePlayer",
            "IDirectPlay_CreateGroup",
            "IDirectPlay_DeletePlayerFromGroup",
            "IDirectPlay_DestroyPlayer",
            "IDirectPlay_DestroyGroup",
            "IDirectPlay_EnableNewPlayers",
            "IDirectPlay_EnumGroupPlayers",
            "IDirectPlay_EnumGroups",
            "IDirectPlay_EnumPlayers",
            "IDirectPlay_EnumSessions",
            "IDirectPlay_GetCaps",
            "IDirectPlay_GetMessageCount",
            "IDirectPlay_GetPlayerCaps",
            "IDirectPlay_GetPlayerName",
            "IDirectPlay_Initialize",
            "IDirectPlay_Open",
            "IDirectPlay_Receive",
            "IDirectPlay_SaveSession",
            "IDirectPlay_Send",
            "IDirectPlay_SetPlayerName",
        ],
    },
    Vtable {
        name: "IDirectPlay4A",
        methods: PLAY4_METHODS,
    },
    Vtable {
        name: "IDirectPlay4",
        methods: PLAY4_METHODS,
    },
];

/// The vtables of an object's interfaces, by their places in VTABLES.
const PLAY: usize = 0;
const PLAY_A: usize = 1;
const PLAY_W: usize = 2;
const INTERFACES: usize = 3;

const CLSID_DIRECT_PLAY: Guid = Guid::new(
    0xD1EB_6D20,
    0x8923,
    0x11D0,
    [0x9D, 0x97, 0x00, 0xA0, 0xC9, 0x0A, 0x43, 0xCB],
);
const IID_IDIRECTPLAY: Guid = Guid::new(
    0x5454_E9A0,
    0xDB65,
    0x11CE,
    [0x92, 0x1C, 0x00, 0xAA, 0x00, 0x6C, 0x49, 0x72],
);
const IID_IDIRECTPLAY2: Guid = Guid::new(
    0x2B74_F7C0,
    0x9154,
    0x11CF,
    [0xA9, 0xCD, 0x00, 0xAA, 0x00, 0x68, 0x86, 0xE3],
);
const IID_IDIRECTPLAY2A: Guid = Guid::new(
    0x9D46_0580,
    0xA822,
    0x11CF,
    [0x96, 0x0C, 0x00, 0x80, 0xC7, 0x53, 0x4E, 0x82],
);
const IID_IDIRECTPLAY3: Guid = Guid::new(
    0x133E_FE40,
    0x32DC,
    0x11D0,
    [0x9C, 0xFB, 0x00, 0xA0, 0xC9, 0x0A, 0x43, 0xCB],
);
const IID_IDIRECTPLAY3A: Guid = Guid::new(
    0x133E_FE41,
    0x32DC,
    0x11D0,
    [0x9C, 0xFB, 0x00, 0xA0, 0xC9, 0x0A, 0x43, 0xCB],
);
const IID_IDIRECTPLAY4: Guid = Guid::new(
    0x0AB1_C530,
    0x4745,
    0x11D1,
    [0xA7, 0xA1, 0x00, 0x00, 0xF8, 0x03, 0xAB, 0xFC],
);
const IID_IDIRECTPLAY4A: Guid = Guid::new(
    0x0AB1_C531,
    0x4745,
    0x11D1,
    [0xA7, 0xA1, 0x00, 0x00, 0xF8, 0x03, 0xAB, 0xFC],
);

/// The classes CoCreateInstance makes of DPLAYX's (com.rs): DirectPlay,
/// and its lobby, which fails as DirectPlayLobbyCreate does.
pub static CLASSES: &[com::Class] = &[
    com::Class {
        clsid: CLSID_DIRECT_PLAY,
        dll: "DPLAYX.DLL",
        name: "DirectPlay",
        create: create_instance,
    },
    com::Class {
        clsid: Guid::new(
            0x2FE8_F810,
            0xB2A5,
            0x11D0,
            [0xA7, 0x87, 0x00, 0x00, 0xF8, 0x03, 0xAB, 0xFC],
        ),
        dll: "DPLAYX.DLL",
        name: "DirectPlayLobby",
        create: create_lobby,
    },
];

const DP_OK: u32 = 0;
const fn dperr(code: u32) -> u32 {
    0x8877_0000 | code
}
const DPERR_ALREADYINITIALIZED: u32 = dperr(5);
const DPERR_INVALIDFLAGS: u32 = dperr(120);
const DPERR_NOMESSAGES: u32 = dperr(190);
const DPERR_UNAVAILABLE: u32 = dperr(250);
const DPERR_UNINITIALIZED: u32 = dperr(900);
const DPERR_INVALIDPARAMS: u32 = com::E_INVALIDARG;

/// EnumConnections' flags: DirectPlay's service providers, the lobby's.
const DPCONNECTION_DIRECTPLAY: u32 = 1;
const DPCONNECTION_DIRECTPLAYLOBBY: u32 = 2;
/// The size of a DPCAPS, which GetCaps checks.
const DPCAPS_SIZE: u32 = 40;

/// A DirectPlay object.
#[derive(Clone, Debug, Default)]
pub struct Play {
    pub pid: u32,
    /// Its interfaces' blocks, by their vtables, 0 before the program
    /// asks for one, and their reference counts.
    pub interfaces: Vec<u32>,
    pub refs: Vec<u32>,
}

crate::state_fields!(Play {
    pid,
    interfaces,
    refs
});

/// DPLAYX's state: the DirectPlay objects, by their ids.
#[derive(Clone, Debug, Default)]
pub struct State {
    objects: Vec<Option<Play>>,
}

crate::state_fields!(State { objects });

fn objects(cpu: &mut Cpu) -> &mut Vec<Option<Play>> {
    &mut cpu.bus.win32.dplay.objects
}

fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("DPLAYX is a built-in DLL")
}

/// The vtable of the interface `iid`, if a DirectPlay object has it.
fn interface_for(iid: &Guid) -> Option<usize> {
    if *iid == com::IID_IUNKNOWN || *iid == IID_IDIRECTPLAY {
        Some(PLAY)
    } else if [IID_IDIRECTPLAY2A, IID_IDIRECTPLAY3A, IID_IDIRECTPLAY4A].contains(iid) {
        Some(PLAY_A)
    } else if [IID_IDIRECTPLAY2, IID_IDIRECTPLAY3, IID_IDIRECTPLAY4].contains(iid) {
        Some(PLAY_W)
    } else {
        None
    }
}

/// A new DirectPlay object, as its interface of the vtable `vtable`
/// with one reference: its block, or None when the heap is full.
fn new_object(cpu: &mut Cpu, vtable: usize) -> Option<u32> {
    let play = Play {
        pid: super::user32::current_pid(cpu),
        interfaces: vec![0; INTERFACES],
        refs: vec![0; INTERFACES],
    };
    let list = objects(cpu);
    let id = match list.iter().position(Option::is_none) {
        Some(i) => {
            list[i] = Some(play);
            i
        }
        None => {
            list.push(Some(play));
            list.len() - 1
        }
    };
    let this = interface(cpu, id, vtable);
    if this.is_none() {
        objects(cpu)[id] = None;
    }
    this
}

/// The object `id`'s interface of the vtable `vtable`, made if it
/// wasn't, with a reference added.
fn interface(cpu: &mut Cpu, id: usize, vtable: usize) -> Option<u32> {
    let have = *objects(cpu).get(id)?.as_ref()?.interfaces.get(vtable)?;
    let this = if have != 0 {
        have
    } else {
        let this = com::new_interface(cpu, dll_index(), vtable, id as u32, 0)?;
        objects(cpu)[id].as_mut()?.interfaces[vtable] = this;
        this
    };
    objects(cpu)[id].as_mut()?.refs[vtable] += 1;
    Some(this)
}

/// The object the interface `this` is of, and the interface's vtable.
fn object_of(cpu: &mut Cpu, this: u32) -> Option<(usize, usize)> {
    let (vtable, id) = com::interface_of(cpu, dll_index(), this)?;
    let id = id as usize;
    let play = objects(cpu).get(id)?.as_ref()?;
    (play.interfaces.get(vtable) == Some(&this)).then_some((id, vtable))
}

/// The process `pid` ended: its objects go with its heap.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    for slot in objects(cpu).iter_mut() {
        if slot.as_ref().is_some_and(|p| p.pid == pid) {
            *slot = None;
        }
    }
}

/// Make a DirectPlay object for CoCreateInstance, as the interface
/// `iid` at `out`. It has no connection, as one CoCreateInstance makes
/// on Windows hasn't until the program calls InitializeConnection.
fn create_instance(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    let Some(vtable) = interface_for(iid) else {
        com::write_out(cpu, out, 0);
        return E_NOINTERFACE;
    };
    match new_object(cpu, vtable) {
        Some(this) => {
            com::write_out(cpu, out, this);
            DP_OK
        }
        None => {
            com::write_out(cpu, out, 0);
            E_OUTOFMEMORY
        }
    }
}

/// Make a lobby object for CoCreateInstance: no lobby, as
/// DirectPlayLobbyCreate says.
fn create_lobby(_: &mut Cpu, _: &Guid, _: u32) -> u32 {
    DPERR_UNAVAILABLE
}

/// DirectPlayCreate(provider, out, outer): an IDirectPlay. GUID_NULL
/// asks for an object with no service provider, which is made; there is
/// no other provider to make one with.
fn direct_play_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [provider, out, outer] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    if !com::write_out(cpu, out, 0) {
        return Some(DPERR_INVALIDPARAMS);
    }
    if outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    let Some(provider) = Guid::read(cpu, provider) else {
        return Some(DPERR_INVALIDPARAMS);
    };
    if provider != GUID_NULL {
        return Some(DPERR_UNAVAILABLE);
    }
    match new_object(cpu, PLAY) {
        Some(this) => {
            com::write_out(cpu, out, this);
            Some(DP_OK)
        }
        None => Some(E_OUTOFMEMORY),
    }
}

/// DirectPlayEnumerate(A, W)(callback, context): the service providers,
/// of which there are none.
fn direct_play_enumerate(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    Some(DP_OK)
}

/// DirectPlayLobbyCreate(A, W)(provider, out, outer, data, size): no
/// lobby; programs go on without one.
fn direct_play_lobby_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[1] == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    com::write_out(cpu, args[1], 0);
    Some(DPERR_UNAVAILABLE)
}

/// QueryInterface(this, iid, out), of every interface.
fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let Some((id, _)) = object_of(cpu, this) else {
        return Some(DPERR_INVALIDPARAMS);
    };
    if !com::write_out(cpu, out, 0) {
        return Some(DPERR_INVALIDPARAMS);
    }
    let vtable = Guid::read(cpu, iid).and_then(|g| interface_for(&g));
    match vtable.and_then(|v| interface(cpu, id, v)) {
        Some(it) => {
            com::write_out(cpu, out, it);
            Some(DP_OK)
        }
        None => Some(E_NOINTERFACE),
    }
}

fn add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((id, vtable)) = object_of(cpu, args[0]) else {
        return Some(0);
    };
    let refs = &mut objects(cpu)[id].as_mut()?.refs[vtable];
    *refs += 1;
    Some(*refs)
}

/// Release(this): the interface's count, and the object goes when no
/// interface of it has a reference left.
fn release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((id, vtable)) = object_of(cpu, args[0]) else {
        return Some(0);
    };
    let play = objects(cpu)[id].as_mut()?;
    play.refs[vtable] = play.refs[vtable].saturating_sub(1);
    let refs = play.refs[vtable];
    if play.refs.iter().all(|&r| r == 0) {
        let gone = objects(cpu)[id].take()?;
        for this in gone.interfaces.into_iter().filter(|&i| i != 0) {
            com::free_interface(cpu, this);
        }
    }
    Some(refs)
}

/// A method that needs a connection, which the object hasn't.
fn uninitialized(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(match object_of(cpu, args[0]) {
        Some(_) => DPERR_UNINITIALIZED,
        None => DPERR_INVALIDPARAMS,
    })
}

/// Close(this): no session is open, and that is closed already.
fn close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(match object_of(cpu, args[0]) {
        Some(_) => DP_OK,
        None => DPERR_INVALIDPARAMS,
    })
}

/// Initialize(this, guid): DirectPlay's objects are initialized as
/// they are made, and the method says so.
fn initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(match object_of(cpu, args[0]) {
        Some(_) => DPERR_ALREADYINITIALIZED,
        None => DPERR_INVALIDPARAMS,
    })
}

/// EnumConnections(this, application, callback, context, flags): the
/// connections of DirectPlay's service providers or of the lobby's, of
/// which there are none to call the callback with.
fn enum_connections(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, callback, _, flags] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    if object_of(cpu, this).is_none() || callback == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    if flags & !(DPCONNECTION_DIRECTPLAY | DPCONNECTION_DIRECTPLAYLOBBY) != 0 {
        return Some(DPERR_INVALIDFLAGS);
    }
    Some(DP_OK)
}

/// InitializeConnection(this, connection, flags): the connection's
/// service provider, whichever it names, isn't one this machine can
/// use.
fn initialize_connection(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, connection, flags] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    if object_of(cpu, this).is_none() {
        return Some(DPERR_INVALIDPARAMS);
    }
    if flags != 0 {
        return Some(DPERR_INVALIDFLAGS);
    }
    if connection == 0 || !cpu.bus.guest_probe(connection, 4, false) {
        return Some(DPERR_INVALIDPARAMS);
    }
    Some(DPERR_UNAVAILABLE)
}

/// EnumSessions(this, desc, timeout, callback, context, flags),
/// Open(this, desc, flags), SecureOpen(this, desc, ...) and IDirectPlay's
/// Open(this, desc): a session's description they can't do without,
/// and a connection to find or open it with, which there isn't.
fn needs_session(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if object_of(cpu, args[0]).is_none() || args[1] == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    Some(DPERR_UNINITIALIZED)
}

/// GetCaps(this, caps, flags), and IDirectPlay's GetCaps(this, caps):
/// the caps are the service provider's, and there is none.
fn get_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let caps = args[1];
    if object_of(cpu, args[0]).is_none() || caps == 0 || !cpu.bus.guest_probe(caps, 4, false) {
        return Some(DPERR_INVALIDPARAMS);
    }
    if cpu.bus.guest_read_32(caps) != DPCAPS_SIZE {
        return Some(DPERR_INVALIDPARAMS);
    }
    Some(DPERR_UNINITIALIZED)
}

/// Receive(this, from, to, flags, data, size): no player gets
/// messages.
fn receive(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, from, to, _, _, size] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    if object_of(cpu, this).is_none() || from == 0 || to == 0 || size == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    Some(DPERR_NOMESSAGES)
}
