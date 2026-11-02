//! DPLAYX.DLL: DirectPlay and its lobby, as on a machine where no
//! service provider can be used. DirectPlay objects are made, by
//! DirectPlayCreate and by CoCreateInstance, as Windows 98 makes them
//! with no network card: games of DirectX 5 and 6 make one at startup
//! even to play alone, and quit when they can't. What such an object
//! lacks is a connection. EnumConnections lists none,
//! InitializeConnection can't make one, and the methods that need one
//! say DPERR_UNINITIALIZED, as DirectPlay does before
//! InitializeConnection.
//!
//! Windows 98 with DirectX 5 or 6 lists the Serial and Modem providers
//! even without a network card, and IPX and TCP/IP where those
//! protocols are installed. Listing none is a deliberate departure from
//! that, until a provider can be initialized: a program offered a
//! connection goes on to initialize it and to look for sessions on it,
//! and there is nothing here to find them with yet. A TCP/IP provider
//! over WSOCK32, which reaches the other instances of a LAN room
//! (CONFIGURATION.md), is the natural next step. DirectPlayEnumerate
//! lists no providers for the same reason.
//!
//! The lobby (lobby.rs) is there as well. No lobby launched the program
//! and no application is registered to be launched, but the lobby
//! builds and reads DirectPlay addresses (address.rs) as Windows does.
//!
//! Each IID is an interface of its own, with its own vtable, pointer
//! and reference count, as in Windows' DPLAYX: IDirectPlay, 2, 2A, 3,
//! 3A, 4 and 4A, and IDirectPlayLobby, A, 2, 2A, 3 and 3A. The later
//! vtables begin with the methods of the earlier ones, so each is a
//! part of one list. IUnknown is the IDirectPlay interface, or the
//! IDirectPlayLobby one, whichever interface it is asked of. An object
//! goes when no interface of it has a reference left, or when its
//! process ends.

mod address;
mod lobby;

pub use address::step as enum_step;

use super::com::{self, CLASS_E_NOAGGREGATION, E_NOINTERFACE, E_OUTOFMEMORY, GUID_NULL, Guid};
use super::stubs::{Api, Conv, Dll, Vtable};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "DPLAYX.DLL",
    base: 0xBFAE_0000,
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

/// A method: DPLAYX's own code, not exported. `args` counts `this`.
const fn method(name: &'static str, args: u8, handler: super::stubs::Handler) -> Api {
    stdcall(name, 0, args, handler)
}

/// The functions, by DPLAYX.DLL's ordinals, then the methods. Append
/// only (see `stubs`). IDirectPlayX's methods are those of IDirectPlay2
/// to 4, A and W; IDirectPlayLobbyX's those of the lobby's interfaces.
static APIS: &[Api] = &[
    stdcall("DirectPlayCreate", 1, 3, direct_play_create),
    stdcall("DirectPlayEnumerateA", 2, 2, direct_play_enumerate),
    stdcall("DirectPlayEnumerateW", 3, 2, direct_play_enumerate),
    stdcall("DirectPlayLobbyCreateA", 4, 5, lobby::create_a),
    stdcall("DirectPlayLobbyCreateW", 5, 5, lobby::create_w),
    stdcall("DirectPlayEnumerate", 9, 2, direct_play_enumerate),
    // IUnknown, of every interface.
    method("IUnknown_QueryInterface", 3, query_interface),
    method("IUnknown_AddRef", 1, add_ref),
    method("IUnknown_Release", 1, release),
    // IDirectPlay2.
    method("IDirectPlayX_AddPlayerToGroup", 3, uninitialized),
    method("IDirectPlayX_Close", 1, close),
    method("IDirectPlayX_CreateGroup", 6, uninitialized),
    method("IDirectPlayX_CreatePlayer", 7, uninitialized),
    method("IDirectPlayX_DeletePlayerFromGroup", 3, uninitialized),
    method("IDirectPlayX_DestroyGroup", 2, uninitialized),
    method("IDirectPlayX_DestroyPlayer", 2, uninitialized),
    method("IDirectPlayX_EnumGroupPlayers", 6, uninitialized),
    method("IDirectPlayX_EnumGroups", 5, uninitialized),
    method("IDirectPlayX_EnumPlayers", 5, uninitialized),
    method("IDirectPlayX_EnumSessions", 6, needs_session),
    method("IDirectPlayX_GetCaps", 3, get_caps),
    method("IDirectPlayX_GetGroupData", 5, uninitialized),
    method("IDirectPlayX_GetGroupName", 4, uninitialized),
    method("IDirectPlayX_GetMessageCount", 3, uninitialized),
    method("IDirectPlayX_GetPlayerAddress", 4, uninitialized),
    method("IDirectPlayX_GetPlayerCaps", 4, uninitialized),
    method("IDirectPlayX_GetPlayerData", 5, uninitialized),
    method("IDirectPlayX_GetPlayerName", 4, uninitialized),
    method("IDirectPlayX_GetSessionDesc", 3, uninitialized),
    method("IDirectPlayX_Initialize", 2, initialize),
    method("IDirectPlayX_Open", 3, needs_session),
    method("IDirectPlayX_Receive", 6, receive),
    method("IDirectPlayX_Send", 6, uninitialized),
    method("IDirectPlayX_SetGroupData", 5, uninitialized),
    method("IDirectPlayX_SetGroupName", 4, uninitialized),
    method("IDirectPlayX_SetPlayerData", 5, uninitialized),
    method("IDirectPlayX_SetPlayerName", 4, uninitialized),
    method("IDirectPlayX_SetSessionDesc", 3, uninitialized),
    // IDirectPlay3.
    method("IDirectPlayX_AddGroupToGroup", 3, uninitialized),
    method("IDirectPlayX_CreateGroupInGroup", 7, uninitialized),
    method("IDirectPlayX_DeleteGroupFromGroup", 3, uninitialized),
    method("IDirectPlayX_EnumConnections", 5, enum_connections),
    method("IDirectPlayX_EnumGroupsInGroup", 6, uninitialized),
    method("IDirectPlayX_GetGroupConnectionSettings", 5, uninitialized),
    method(
        "IDirectPlayX_InitializeConnection",
        3,
        initialize_connection,
    ),
    method("IDirectPlayX_SecureOpen", 5, needs_session),
    method("IDirectPlayX_SendChatMessage", 5, uninitialized),
    method("IDirectPlayX_SetGroupConnectionSettings", 4, uninitialized),
    method("IDirectPlayX_StartSession", 3, uninitialized),
    method("IDirectPlayX_GetGroupFlags", 3, uninitialized),
    method("IDirectPlayX_GetGroupParent", 3, uninitialized),
    method("IDirectPlayX_GetPlayerAccount", 5, uninitialized),
    method("IDirectPlayX_GetPlayerFlags", 3, uninitialized),
    // IDirectPlay4.
    method("IDirectPlayX_GetGroupOwner", 3, uninitialized),
    method("IDirectPlayX_SetGroupOwner", 3, uninitialized),
    method("IDirectPlayX_SendEx", 10, uninitialized),
    method("IDirectPlayX_GetMessageQueue", 6, uninitialized),
    method("IDirectPlayX_CancelMessage", 3, uninitialized),
    method("IDirectPlayX_CancelPriority", 4, uninitialized),
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
    // IDirectPlayLobby.
    method("IDirectPlayLobbyX_Connect", 4, lobby::connect),
    method(
        "IDirectPlayLobbyX_CreateAddress",
        7,
        address::create_address,
    ),
    method("IDirectPlayLobbyX_EnumAddress", 5, address::enum_address),
    method("IDirectPlayLobbyX_EnumAddressTypes", 5, address::enum_types),
    method(
        "IDirectPlayLobbyX_EnumLocalApplications",
        4,
        lobby::enum_local_applications,
    ),
    method(
        "IDirectPlayLobbyX_GetConnectionSettings",
        4,
        lobby::not_lobbied,
    ),
    method(
        "IDirectPlayLobbyX_ReceiveLobbyMessage",
        6,
        lobby::not_lobbied,
    ),
    method(
        "IDirectPlayLobbyX_RunApplication",
        5,
        lobby::run_application,
    ),
    method("IDirectPlayLobbyX_SendLobbyMessage", 5, lobby::not_lobbied),
    method(
        "IDirectPlayLobbyX_SetConnectionSettings",
        4,
        lobby::set_connection_settings,
    ),
    method(
        "IDirectPlayLobbyX_SetLobbyMessageEvent",
        4,
        lobby::not_lobbied,
    ),
    // IDirectPlayLobby2.
    method(
        "IDirectPlayLobbyX_CreateCompoundAddress",
        5,
        address::create_compound_address,
    ),
    // IDirectPlayLobby3.
    method("IDirectPlayLobbyX_ConnectEx", 5, lobby::connect_ex),
    method(
        "IDirectPlayLobbyX_RegisterApplication",
        3,
        lobby::register_application,
    ),
    method(
        "IDirectPlayLobbyX_UnregisterApplication",
        3,
        lobby::unregister_application,
    ),
    method(
        "IDirectPlayLobbyX_WaitForConnectionSettings",
        2,
        lobby::wait_for_connection_settings,
    ),
];

/// IDirectPlay4's methods in their order: IUnknown's 3, IDirectPlay2's
/// 29, IDirectPlay3's 15, then its own 6. IDirectPlay2's and 3's
/// vtables are the first PLAY2_END and PLAY3_END of them.
const PLAY4_METHODS: &[&str] = &[
    "IUnknown_QueryInterface",
    "IUnknown_AddRef",
    "IUnknown_Release",
    "IDirectPlayX_AddPlayerToGroup",
    "IDirectPlayX_Close",
    "IDirectPlayX_CreateGroup",
    "IDirectPlayX_CreatePlayer",
    "IDirectPlayX_DeletePlayerFromGroup",
    "IDirectPlayX_DestroyGroup",
    "IDirectPlayX_DestroyPlayer",
    "IDirectPlayX_EnumGroupPlayers",
    "IDirectPlayX_EnumGroups",
    "IDirectPlayX_EnumPlayers",
    "IDirectPlayX_EnumSessions",
    "IDirectPlayX_GetCaps",
    "IDirectPlayX_GetGroupData",
    "IDirectPlayX_GetGroupName",
    "IDirectPlayX_GetMessageCount",
    "IDirectPlayX_GetPlayerAddress",
    "IDirectPlayX_GetPlayerCaps",
    "IDirectPlayX_GetPlayerData",
    "IDirectPlayX_GetPlayerName",
    "IDirectPlayX_GetSessionDesc",
    "IDirectPlayX_Initialize",
    "IDirectPlayX_Open",
    "IDirectPlayX_Receive",
    "IDirectPlayX_Send",
    "IDirectPlayX_SetGroupData",
    "IDirectPlayX_SetGroupName",
    "IDirectPlayX_SetPlayerData",
    "IDirectPlayX_SetPlayerName",
    "IDirectPlayX_SetSessionDesc",
    "IDirectPlayX_AddGroupToGroup",
    "IDirectPlayX_CreateGroupInGroup",
    "IDirectPlayX_DeleteGroupFromGroup",
    "IDirectPlayX_EnumConnections",
    "IDirectPlayX_EnumGroupsInGroup",
    "IDirectPlayX_GetGroupConnectionSettings",
    "IDirectPlayX_InitializeConnection",
    "IDirectPlayX_SecureOpen",
    "IDirectPlayX_SendChatMessage",
    "IDirectPlayX_SetGroupConnectionSettings",
    "IDirectPlayX_StartSession",
    "IDirectPlayX_GetGroupFlags",
    "IDirectPlayX_GetGroupParent",
    "IDirectPlayX_GetPlayerAccount",
    "IDirectPlayX_GetPlayerFlags",
    "IDirectPlayX_GetGroupOwner",
    "IDirectPlayX_SetGroupOwner",
    "IDirectPlayX_SendEx",
    "IDirectPlayX_GetMessageQueue",
    "IDirectPlayX_CancelMessage",
    "IDirectPlayX_CancelPriority",
];
const PLAY2_END: usize = 32;
const PLAY3_END: usize = 47;
const PLAY2_METHODS: &[&str] = PLAY4_METHODS.split_at(PLAY2_END).0;
const PLAY3_METHODS: &[&str] = PLAY4_METHODS.split_at(PLAY3_END).0;

/// IDirectPlay's methods, the first interface's, which IDirectPlay2's
/// don't begin with.
const PLAY1_METHODS: &[&str] = &[
    "IUnknown_QueryInterface",
    "IUnknown_AddRef",
    "IUnknown_Release",
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
];

/// IDirectPlayLobby3's methods in their order: IUnknown's 3,
/// IDirectPlayLobby's 11, IDirectPlayLobby2's 1, then its own 4.
const LOBBY3_METHODS: &[&str] = &[
    "IUnknown_QueryInterface",
    "IUnknown_AddRef",
    "IUnknown_Release",
    "IDirectPlayLobbyX_Connect",
    "IDirectPlayLobbyX_CreateAddress",
    "IDirectPlayLobbyX_EnumAddress",
    "IDirectPlayLobbyX_EnumAddressTypes",
    "IDirectPlayLobbyX_EnumLocalApplications",
    "IDirectPlayLobbyX_GetConnectionSettings",
    "IDirectPlayLobbyX_ReceiveLobbyMessage",
    "IDirectPlayLobbyX_RunApplication",
    "IDirectPlayLobbyX_SendLobbyMessage",
    "IDirectPlayLobbyX_SetConnectionSettings",
    "IDirectPlayLobbyX_SetLobbyMessageEvent",
    "IDirectPlayLobbyX_CreateCompoundAddress",
    "IDirectPlayLobbyX_ConnectEx",
    "IDirectPlayLobbyX_RegisterApplication",
    "IDirectPlayLobbyX_UnregisterApplication",
    "IDirectPlayLobbyX_WaitForConnectionSettings",
];
const LOBBY1_END: usize = 14;
const LOBBY2_END: usize = 15;
const LOBBY1_METHODS: &[&str] = LOBBY3_METHODS.split_at(LOBBY1_END).0;
const LOBBY2_METHODS: &[&str] = LOBBY3_METHODS.split_at(LOBBY2_END).0;

/// The vtables, by their places in this list, which IIDS follows. An A
/// interface and its W one point at the same functions, which holds
/// only while no method reads or writes a string: one that does needs
/// an A function and a W one.
static VTABLES: &[Vtable] = &[
    Vtable {
        name: "IDirectPlay",
        methods: PLAY1_METHODS,
    },
    Vtable {
        name: "IDirectPlay2",
        methods: PLAY2_METHODS,
    },
    Vtable {
        name: "IDirectPlay2A",
        methods: PLAY2_METHODS,
    },
    Vtable {
        name: "IDirectPlay3",
        methods: PLAY3_METHODS,
    },
    Vtable {
        name: "IDirectPlay3A",
        methods: PLAY3_METHODS,
    },
    Vtable {
        name: "IDirectPlay4",
        methods: PLAY4_METHODS,
    },
    Vtable {
        name: "IDirectPlay4A",
        methods: PLAY4_METHODS,
    },
    Vtable {
        name: "IDirectPlayLobby",
        methods: LOBBY1_METHODS,
    },
    Vtable {
        name: "IDirectPlayLobbyA",
        methods: LOBBY1_METHODS,
    },
    Vtable {
        name: "IDirectPlayLobby2",
        methods: LOBBY2_METHODS,
    },
    Vtable {
        name: "IDirectPlayLobby2A",
        methods: LOBBY2_METHODS,
    },
    Vtable {
        name: "IDirectPlayLobby3",
        methods: LOBBY3_METHODS,
    },
    Vtable {
        name: "IDirectPlayLobby3A",
        methods: LOBBY3_METHODS,
    },
];

/// Places in VTABLES: IDirectPlay, and the lobby's first two. Those
/// from LOBBY on are a lobby's.
const PLAY: usize = 0;
const LOBBY: usize = 7;
const LOBBY_A: usize = 8;

const CLSID_DIRECT_PLAY: Guid = Guid::new(
    0xD1EB_6D20,
    0x8923,
    0x11D0,
    [0x9D, 0x97, 0x00, 0xA0, 0xC9, 0x0A, 0x43, 0xCB],
);
const CLSID_DIRECT_PLAY_LOBBY: Guid = Guid::new(
    0x2FE8_F810,
    0xB2A5,
    0x11D0,
    [0xA7, 0x87, 0x00, 0x00, 0xF8, 0x03, 0xAB, 0xFC],
);

/// The interfaces' IIDs, by their vtables' places in VTABLES:
/// IDirectPlay to IDirectPlay4A, then IDirectPlayLobby to 3A.
const IIDS: [Guid; 13] = [
    Guid::new(
        0x5454_E9A0,
        0xDB65,
        0x11CE,
        [0x92, 0x1C, 0x00, 0xAA, 0x00, 0x6C, 0x49, 0x72],
    ),
    Guid::new(
        0x2B74_F7C0,
        0x9154,
        0x11CF,
        [0xA9, 0xCD, 0x00, 0xAA, 0x00, 0x68, 0x86, 0xE3],
    ),
    Guid::new(
        0x9D46_0580,
        0xA822,
        0x11CF,
        [0x96, 0x0C, 0x00, 0x80, 0xC7, 0x53, 0x4E, 0x82],
    ),
    Guid::new(
        0x133E_FE40,
        0x32DC,
        0x11D0,
        [0x9C, 0xFB, 0x00, 0xA0, 0xC9, 0x0A, 0x43, 0xCB],
    ),
    Guid::new(
        0x133E_FE41,
        0x32DC,
        0x11D0,
        [0x9C, 0xFB, 0x00, 0xA0, 0xC9, 0x0A, 0x43, 0xCB],
    ),
    Guid::new(
        0x0AB1_C530,
        0x4745,
        0x11D1,
        [0xA7, 0xA1, 0x00, 0x00, 0xF8, 0x03, 0xAB, 0xFC],
    ),
    Guid::new(
        0x0AB1_C531,
        0x4745,
        0x11D1,
        [0xA7, 0xA1, 0x00, 0x00, 0xF8, 0x03, 0xAB, 0xFC],
    ),
    Guid::new(
        0xAF46_5C71,
        0x9588,
        0x11CF,
        [0xA0, 0x20, 0x00, 0xAA, 0x00, 0x61, 0x57, 0xAC],
    ),
    Guid::new(
        0x26C6_6A70,
        0xB367,
        0x11CF,
        [0xA0, 0x24, 0x00, 0xAA, 0x00, 0x61, 0x57, 0xAC],
    ),
    Guid::new(
        0x0194_C220,
        0xA303,
        0x11D0,
        [0x9C, 0x4F, 0x00, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
    ),
    Guid::new(
        0x1BB4_AF80,
        0xA303,
        0x11D0,
        [0x9C, 0x4F, 0x00, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
    ),
    Guid::new(
        0x2DB7_2490,
        0x652C,
        0x11D1,
        [0xA7, 0xA8, 0x00, 0x00, 0xF8, 0x03, 0xAB, 0xFC],
    ),
    Guid::new(
        0x2DB7_2491,
        0x652C,
        0x11D1,
        [0xA7, 0xA8, 0x00, 0x00, 0xF8, 0x03, 0xAB, 0xFC],
    ),
];

/// The classes CoCreateInstance makes of DPLAYX's (com.rs).
pub static CLASSES: &[com::Class] = &[
    com::Class {
        clsid: CLSID_DIRECT_PLAY,
        dll: "DPLAYX.DLL",
        name: "DirectPlay",
        create: create_play,
    },
    com::Class {
        clsid: CLSID_DIRECT_PLAY_LOBBY,
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
const DPERR_BUFFERTOOSMALL: u32 = dperr(30);
const DPERR_INVALIDFLAGS: u32 = dperr(120);
const DPERR_INVALIDOBJECT: u32 = dperr(130);
const DPERR_NOMESSAGES: u32 = dperr(190);
const DPERR_UNAVAILABLE: u32 = dperr(250);
const DPERR_UNINITIALIZED: u32 = dperr(320);
const DPERR_UNKNOWNAPPLICATION: u32 = dperr(1050);
const DPERR_NOTLOBBIED: u32 = dperr(1070);
const DPERR_INVALIDPARAMS: u32 = com::E_INVALIDARG;

/// EnumConnections' flags: DirectPlay's service providers, the lobby's.
const DPCONNECTION_DIRECTPLAY: u32 = 1;
const DPCONNECTION_DIRECTPLAYLOBBY: u32 = 2;
/// Receive's flags: DPRECEIVE_ALL, TOPLAYER, FROMPLAYER and PEEK.
const DPRECEIVE_FLAGS: u32 = 0xF;
/// A DPADDRESS chunk's header: its data's GUID and size.
const CHUNK_HEADER: u32 = 20;

/// A DirectPlay or lobby object.
#[derive(Clone, Debug, Default)]
pub struct Object {
    pub pid: u32,
    pub lobby: bool,
    /// Its interfaces' blocks, by their vtables' places in VTABLES, 0
    /// for those the program hasn't asked for, and their reference
    /// counts.
    pub interfaces: Vec<u32>,
    pub refs: Vec<u32>,
}

crate::state_fields!(Object {
    pid,
    lobby,
    interfaces,
    refs
});

/// DPLAYX's state: the objects, by their ids.
#[derive(Clone, Debug, Default)]
pub struct State {
    objects: Vec<Option<Object>>,
}

crate::state_fields!(State { objects });

fn objects(cpu: &mut Cpu) -> &mut Vec<Option<Object>> {
    &mut cpu.bus.win32.dplay.objects
}

fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("DPLAYX is a built-in DLL")
}

/// The vtable of the interface `iid` of a lobby or a DirectPlay object,
/// if it has that interface.
fn interface_for(lobby: bool, iid: &Guid) -> Option<usize> {
    if *iid == com::IID_IUNKNOWN {
        return Some(if lobby { LOBBY } else { PLAY });
    }
    let vtable = IIDS.iter().position(|i| i == iid)?;
    ((vtable >= LOBBY) == lobby).then_some(vtable)
}

/// A new object, as its interface of the vtable `vtable` with one
/// reference: its block, or None when the heap is full.
fn new_object(cpu: &mut Cpu, vtable: usize) -> Option<u32> {
    let object = Object {
        pid: super::user32::current_pid(cpu),
        lobby: vtable >= LOBBY,
        interfaces: vec![0; VTABLES.len()],
        refs: vec![0; VTABLES.len()],
    };
    let list = objects(cpu);
    let id = match list.iter().position(Option::is_none) {
        Some(i) => {
            list[i] = Some(object);
            i
        }
        None => {
            list.push(Some(object));
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

/// The object the interface `this` is of and the interface's vtable,
/// or DPERR_INVALIDOBJECT when `this` isn't an interface of a live
/// object: every method's answer to such a pointer.
fn object_of(cpu: &mut Cpu, this: u32) -> Result<(usize, usize), u32> {
    let (vtable, id) = com::interface_of(cpu, dll_index(), this).ok_or(DPERR_INVALIDOBJECT)?;
    let id = id as usize;
    let object = objects(cpu).get(id).and_then(Option::as_ref);
    match object {
        Some(o) if o.interfaces.get(vtable) == Some(&this) => Ok((id, vtable)),
        _ => Err(DPERR_INVALIDOBJECT),
    }
}

/// The object of `this`, or the method returns DPERR_INVALIDOBJECT.
macro_rules! object {
    ($cpu:expr, $this:expr) => {
        match object_of($cpu, $this) {
            Ok(v) => v,
            Err(e) => return Some(e),
        }
    };
}
use object;

/// The process `pid` ended: its objects go with its heap.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    for slot in objects(cpu).iter_mut() {
        if slot.as_ref().is_some_and(|o| o.pid == pid) {
            *slot = None;
        }
    }
}

/// Make an object of the vtable `iid` asks for, of a lobby or of
/// DirectPlay, at `out` for CoCreateInstance.
fn create(cpu: &mut Cpu, lobby: bool, iid: &Guid, out: u32) -> u32 {
    let Some(vtable) = interface_for(lobby, iid) else {
        com::write_out(cpu, out, 0);
        return E_NOINTERFACE;
    };
    let this = new_object(cpu, vtable);
    com::write_out(cpu, out, this.unwrap_or(0));
    if this.is_some() { DP_OK } else { E_OUTOFMEMORY }
}

/// CoCreateInstance's DirectPlay object. It has no connection, as one
/// CoCreateInstance makes on Windows hasn't until the program calls
/// InitializeConnection.
fn create_play(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    create(cpu, false, iid, out)
}

/// CoCreateInstance's lobby object.
fn create_lobby(cpu: &mut Cpu, iid: &Guid, out: u32) -> u32 {
    create(cpu, true, iid, out)
}

/// DirectPlayCreate(provider, out, outer): an IDirectPlay. GUID_NULL
/// asks for an object with no service provider, which is made; there is
/// no other provider to make one with. The outer object is looked at
/// first, as Wine does.
fn direct_play_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [provider, out, outer] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    if outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    let provider = Guid::read(cpu, provider);
    if provider.is_none() || !com::write_out(cpu, out, 0) {
        return Some(DPERR_INVALIDPARAMS);
    }
    if provider != Some(GUID_NULL) {
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
/// of which none is listed (see the module's notes).
fn direct_play_enumerate(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    Some(DP_OK)
}

/// QueryInterface(this, iid, out), of every interface.
fn query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let (id, vtable) = object!(cpu, this);
    if !com::write_out(cpu, out, 0) {
        return Some(DPERR_INVALIDPARAMS);
    }
    let lobby = vtable >= LOBBY;
    let Some(wanted) = Guid::read(cpu, iid).and_then(|g| interface_for(lobby, &g)) else {
        return Some(E_NOINTERFACE);
    };
    match interface(cpu, id, wanted) {
        Some(it) => {
            com::write_out(cpu, out, it);
            Some(DP_OK)
        }
        None => Some(E_OUTOFMEMORY),
    }
}

fn add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok((id, vtable)) = object_of(cpu, args[0]) else {
        return Some(0);
    };
    let refs = &mut objects(cpu)[id].as_mut()?.refs[vtable];
    *refs += 1;
    Some(*refs)
}

/// Release(this): the interface's count, and the object goes when no
/// interface of it has a reference left.
fn release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Ok((id, vtable)) = object_of(cpu, args[0]) else {
        return Some(0);
    };
    let object = objects(cpu)[id].as_mut()?;
    object.refs[vtable] = object.refs[vtable].saturating_sub(1);
    let refs = object.refs[vtable];
    if object.refs.iter().all(|&r| r == 0) {
        let gone = objects(cpu)[id].take()?;
        for this in gone.interfaces.into_iter().filter(|&i| i != 0) {
            com::free_interface(cpu, this);
        }
    }
    Some(refs)
}

/// A method that needs a connection, which the object hasn't.
fn uninitialized(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    object!(cpu, args[0]);
    Some(DPERR_UNINITIALIZED)
}

/// Close(this): no session is open, and that is closed already.
fn close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    object!(cpu, args[0]);
    Some(DP_OK)
}

/// Initialize(this, guid): DirectPlay's objects are initialized as
/// they are made, and the method says so.
fn initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    object!(cpu, args[0]);
    Some(DPERR_ALREADYINITIALIZED)
}

/// EnumConnections(this, application, callback, context, flags): the
/// connections of DirectPlay's service providers or of the lobby's, of
/// which none is listed (see the module's notes).
fn enum_connections(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, callback, _, flags] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    object!(cpu, this);
    if callback == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    if flags & !(DPCONNECTION_DIRECTPLAY | DPCONNECTION_DIRECTPLAYLOBBY) != 0 {
        return Some(DPERR_INVALIDFLAGS);
    }
    Some(DP_OK)
}

/// InitializeConnection(this, connection, flags): the connection is a
/// DirectPlay address, at least a chunk's header, and its service
/// provider, whichever it names, isn't one this machine can use.
fn initialize_connection(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, connection, flags] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    object!(cpu, this);
    if flags != 0 {
        return Some(DPERR_INVALIDFLAGS);
    }
    if connection == 0
        || !cpu
            .bus
            .guest_probe(connection, CHUNK_HEADER as usize, false)
    {
        return Some(DPERR_INVALIDPARAMS);
    }
    Some(DPERR_UNAVAILABLE)
}

/// EnumSessions(this, desc, timeout, callback, context, flags),
/// Open(this, desc, flags), SecureOpen(this, desc, ...) and
/// IDirectPlay's Open(this, desc): there is no connection to find or
/// open a session with. Only the description's pointer is checked; the
/// rest of what DirectPlay checks (a dwSize of 80, the flags, the
/// callback) waits for a provider to use them with.
fn needs_session(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    object!(cpu, args[0]);
    if args[1] == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    Some(DPERR_UNINITIALIZED)
}

/// GetCaps(this, caps, flags), and IDirectPlay's GetCaps(this, caps):
/// the caps are the connection's, and there is none, whatever the
/// arguments are. Their dwSize is checked once there is a provider.
fn get_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    object!(cpu, args[0]);
    Some(DPERR_UNINITIALIZED)
}

/// Receive(this, from, to, flags, data, size): no player gets messages.
/// It says DPERR_NOMESSAGES rather than DPERR_UNINITIALIZED, as Wine
/// does: games poll it in a loop that ends on that answer.
fn receive(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, from, to, flags, _, size] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    object!(cpu, this);
    if flags & !DPRECEIVE_FLAGS != 0 {
        return Some(DPERR_INVALIDFLAGS);
    }
    if from == 0 || to == 0 || size == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    Some(DPERR_NOMESSAGES)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn each_iid_has_its_own_vtable() {
        assert_eq!(VTABLES.len(), IIDS.len());
        let lengths: Vec<usize> = VTABLES.iter().map(|v| v.methods.len()).collect();
        assert_eq!(
            lengths,
            [25, 32, 32, 47, 47, 53, 53, 14, 14, 15, 15, 19, 19]
        );
        assert_eq!(interface_for(false, &com::IID_IUNKNOWN), Some(PLAY));
        assert_eq!(interface_for(true, &com::IID_IUNKNOWN), Some(LOBBY));
        assert_eq!(interface_for(true, &IIDS[LOBBY_A]), Some(LOBBY_A));
        assert_eq!(interface_for(false, &IIDS[LOBBY_A]), None);
        assert_eq!(interface_for(true, &IIDS[PLAY]), None);
    }
}
