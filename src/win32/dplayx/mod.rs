//! DPLAYX.DLL: DirectPlay and its lobby. DirectPlay objects are made by
//! DirectPlayCreate and by CoCreateInstance, and have the interfaces
//! IDirectPlay (DirectX 3) to IDirectPlay4A.
//!
//! Two service providers are listed, IPX and TCP/IP, as on a Windows 98
//! machine with both protocols. Both are the same connection underneath:
//! the LAN room of rust-dos, which DirectPlay reaches through the
//! built-in IPX driver (net.rs). Its packets go out as IPX frames, as a
//! DOS game's do, so a session hosted on one member of a room is found by
//! every other member, whichever of the two providers each program
//! picked. A TCP/IP address a program gives is not used: a room is one
//! network, where a broadcast finds every session. The protocol between
//! the objects is rust-dos's own (wire.rs), not Windows'; a program on a
//! Windows 95 or 98 image in another instance doesn't hear it.
//!
//! session.rs keeps an object's session: its members, players and groups,
//! and the messages waiting for its players, which follow what DirectPlay
//! does on Windows 98 as Wine's tests recorded it. link.rs carries the
//! messages that must arrive, in order. play.rs has IDirectPlay2 to 4's
//! methods and play1.rs IDirectPlay's own; guest.rs reads and writes
//! their structures, ANSI or Unicode as the interface has them, and
//! calls.rs calls the program's callbacks and makes a method wait.
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
//! process ends; in a session, it leaves it first.

mod address;
mod calls;
mod guest;
mod link;
mod lobby;
mod net;
mod play;
mod play1;
mod session;
mod wire;

pub use address::step as enum_step;
pub use calls::step as call_step;
pub use net::poll;

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
    stdcall("DirectPlayEnumerateW", 3, 2, direct_play_enumerate_w),
    stdcall("DirectPlayLobbyCreateA", 4, 5, lobby::create_a),
    stdcall("DirectPlayLobbyCreateW", 5, 5, lobby::create_w),
    stdcall("DirectPlayEnumerate", 9, 2, direct_play_enumerate),
    // IUnknown, of every interface.
    method("IUnknown_QueryInterface", 3, query_interface),
    method("IUnknown_AddRef", 1, add_ref),
    method("IUnknown_Release", 1, release),
    // IDirectPlay2.
    method(
        "IDirectPlayX_AddPlayerToGroup",
        3,
        play::add_player_to_group,
    ),
    method("IDirectPlayX_Close", 1, play::close),
    method("IDirectPlayX_CreateGroup", 6, play::create_group),
    method("IDirectPlayX_CreatePlayer", 7, play::create_player),
    method(
        "IDirectPlayX_DeletePlayerFromGroup",
        3,
        play::delete_player_from_group,
    ),
    method("IDirectPlayX_DestroyGroup", 2, play::destroy_group),
    method("IDirectPlayX_DestroyPlayer", 2, play::destroy_player),
    method("IDirectPlayX_EnumGroupPlayers", 6, play::enum_group_players),
    method("IDirectPlayX_EnumGroups", 5, play::enum_groups),
    method("IDirectPlayX_EnumPlayers", 5, play::enum_players),
    method("IDirectPlayX_EnumSessions", 6, play::enum_sessions),
    method("IDirectPlayX_GetCaps", 3, play::get_caps),
    method("IDirectPlayX_GetGroupData", 5, play::get_group_data),
    method("IDirectPlayX_GetGroupName", 4, play::get_group_name),
    method("IDirectPlayX_GetMessageCount", 3, play::get_message_count),
    method("IDirectPlayX_GetPlayerAddress", 4, play::get_player_address),
    method("IDirectPlayX_GetPlayerCaps", 4, play::get_player_caps),
    method("IDirectPlayX_GetPlayerData", 5, play::get_player_data),
    method("IDirectPlayX_GetPlayerName", 4, play::get_player_name),
    method("IDirectPlayX_GetSessionDesc", 3, play::get_session_desc),
    method("IDirectPlayX_Initialize", 2, initialize_method),
    method("IDirectPlayX_Open", 3, play::open),
    method("IDirectPlayX_Receive", 6, play::receive),
    method("IDirectPlayX_Send", 6, play::send),
    method("IDirectPlayX_SetGroupData", 5, play::set_group_data),
    method("IDirectPlayX_SetGroupName", 4, play::set_group_name),
    method("IDirectPlayX_SetPlayerData", 5, play::set_player_data),
    method("IDirectPlayX_SetPlayerName", 4, play::set_player_name),
    method("IDirectPlayX_SetSessionDesc", 3, play::set_session_desc),
    // IDirectPlay3.
    method("IDirectPlayX_AddGroupToGroup", 3, play::add_group_to_group),
    method(
        "IDirectPlayX_CreateGroupInGroup",
        7,
        play::create_group_in_group,
    ),
    method(
        "IDirectPlayX_DeleteGroupFromGroup",
        3,
        play::delete_group_from_group,
    ),
    method("IDirectPlayX_EnumConnections", 5, play::enum_connections),
    method(
        "IDirectPlayX_EnumGroupsInGroup",
        6,
        play::enum_groups_in_group,
    ),
    method(
        "IDirectPlayX_GetGroupConnectionSettings",
        5,
        play::unsupported,
    ),
    method(
        "IDirectPlayX_InitializeConnection",
        3,
        play::initialize_connection,
    ),
    method("IDirectPlayX_SecureOpen", 5, play::open),
    method("IDirectPlayX_SendChatMessage", 5, play::send_chat_message),
    method(
        "IDirectPlayX_SetGroupConnectionSettings",
        4,
        play::unsupported,
    ),
    method("IDirectPlayX_StartSession", 3, play::unsupported),
    method("IDirectPlayX_GetGroupFlags", 3, play::get_group_flags),
    method("IDirectPlayX_GetGroupParent", 3, play::get_group_parent),
    method("IDirectPlayX_GetPlayerAccount", 5, play::unsupported),
    method("IDirectPlayX_GetPlayerFlags", 3, play::get_player_flags),
    // IDirectPlay4.
    method("IDirectPlayX_GetGroupOwner", 3, play::get_group_owner),
    method("IDirectPlayX_SetGroupOwner", 3, play::set_group_owner),
    method("IDirectPlayX_SendEx", 10, play::send_ex),
    method("IDirectPlayX_GetMessageQueue", 6, play::get_message_queue),
    method("IDirectPlayX_CancelMessage", 3, play::cancel_message),
    method("IDirectPlayX_CancelPriority", 4, play::cancel_priority),
    // IDirectPlay, the first interface.
    method("IDirectPlay_AddPlayerToGroup", 3, play::add_player_to_group),
    method("IDirectPlay_Close", 1, play::close),
    method("IDirectPlay_CreatePlayer", 5, play1::create_player),
    method("IDirectPlay_CreateGroup", 4, play1::create_group),
    method(
        "IDirectPlay_DeletePlayerFromGroup",
        3,
        play::delete_player_from_group,
    ),
    method("IDirectPlay_DestroyPlayer", 2, play::destroy_player),
    method("IDirectPlay_DestroyGroup", 2, play::destroy_group),
    method("IDirectPlay_EnableNewPlayers", 2, play1::enable_new_players),
    method("IDirectPlay_EnumGroupPlayers", 5, play1::enum_group_players),
    method("IDirectPlay_EnumGroups", 5, play1::enum_groups),
    method("IDirectPlay_EnumPlayers", 5, play1::enum_players),
    method("IDirectPlay_EnumSessions", 6, play1::enum_sessions),
    method("IDirectPlay_GetCaps", 2, play1::get_caps),
    method("IDirectPlay_GetMessageCount", 3, play::get_message_count),
    method("IDirectPlay_GetPlayerCaps", 3, play1::get_player_caps),
    method("IDirectPlay_GetPlayerName", 6, play1::get_player_name),
    method("IDirectPlay_Initialize", 2, play1::initialize),
    method("IDirectPlay_Open", 2, play1::open),
    method("IDirectPlay_Receive", 6, play::receive),
    method("IDirectPlay_SaveSession", 2, play1::save_session),
    method("IDirectPlay_Send", 6, play::send),
    method("IDirectPlay_SetPlayerName", 4, play1::set_player_name),
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
const DPERR_UNAVAILABLE: u32 = dperr(250);
const DPERR_UNINITIALIZED: u32 = dperr(320);
const DPERR_UNKNOWNAPPLICATION: u32 = dperr(1050);
const DPERR_NOTLOBBIED: u32 = dperr(1070);
const DPERR_INVALIDPARAMS: u32 = com::E_INVALIDARG;

/// EnumConnections' flags: DirectPlay's service providers, the lobby's.
const DPCONNECTION_DIRECTPLAY: u32 = 1;
const DPCONNECTION_DIRECTPLAYLOBBY: u32 = 2;
/// A DPADDRESS chunk's header: its data's GUID and size.
const CHUNK_HEADER: u32 = 20;
/// The most data a message, a player's or a group's carries.
const MAX_DATA: u32 = 0x10_0000;

/// The service providers, as `Play::provider` numbers them.
const IPX: u8 = 1;
const TCPIP: u8 = 2;
const DPSPGUID_IPX: Guid = Guid::new(
    0x685B_C400,
    0x9D2C,
    0x11CF,
    [0xA9, 0xCD, 0x00, 0xAA, 0x00, 0x68, 0x86, 0xE3],
);
const DPSPGUID_TCPIP: Guid = Guid::new(
    0x36E9_5EE0,
    0x8577,
    0x11CF,
    [0x96, 0x0C, 0x00, 0x80, 0xC7, 0x53, 0x4E, 0x82],
);

/// The providers DirectPlayEnumerate and EnumConnections list, with the
/// names Windows 98 gives them.
const PROVIDERS: &[(Guid, &str)] = &[
    (DPSPGUID_IPX, "IPX Connection For DirectPlay"),
    (DPSPGUID_TCPIP, "Internet TCP/IP Connection For DirectPlay"),
];
/// The version DirectPlayEnumerate gives each provider: DirectX 6's.
const PROVIDER_VERSION: (u32, u32) = (6, 0);

/// A provider's number, if it is one there is.
fn provider_code(guid: &Guid) -> Option<u8> {
    match *guid {
        DPSPGUID_IPX => Some(IPX),
        DPSPGUID_TCPIP => Some(TCPIP),
        _ => None,
    }
}

fn provider_of(code: u8) -> Guid {
    if code == TCPIP {
        DPSPGUID_TCPIP
    } else {
        DPSPGUID_IPX
    }
}

/// Give the object `id` the provider `guid`: DP_OK, or
/// DPERR_UNAVAILABLE for one there isn't.
fn initialize(cpu: &mut Cpu, id: usize, guid: &Guid) -> u32 {
    let Some(code) = provider_code(guid) else {
        return DPERR_UNAVAILABLE;
    };
    net::connect(cpu);
    let p = play(cpu, id);
    p.provider = code;
    while p.endpoint == 0 {
        p.endpoint = random_u32();
    }
    net::prepare(cpu, id);
    DP_OK
}

/// A random number a program sees (an endpoint, a session's key), the
/// same in every run of deterministic mode.
fn random_u32() -> u32 {
    let mut bytes = [0; 4];
    crate::net::fill_machine_random(&mut bytes);
    u32::from_le_bytes(bytes)
}

/// Whether the interface `vtable` has Unicode strings: IDirectPlay2, 3
/// and 4, against their A forms and IDirectPlay.
fn wide(vtable: usize) -> bool {
    matches!(vtable, 1 | 3 | 5)
}

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
    /// A DirectPlay object's connection and session.
    pub play: session::Play,
}

crate::state_fields!(Object {
    pid,
    lobby,
    interfaces,
    refs,
    play
});

/// DPLAYX's state: the objects, by their ids, and the packets between
/// them when there is no IPX driver, which a state doesn't keep.
#[derive(Clone, Debug, Default)]
pub struct State {
    objects: Vec<Option<Object>>,
    local: net::Local,
}

crate::state_fields!(State { objects } skip { local });

fn objects(cpu: &mut Cpu) -> &mut Vec<Option<Object>> {
    &mut cpu.bus.win32.dplay.objects
}

/// The network side of the object `id`, which the method's `object!`
/// found.
fn play(cpu: &mut Cpu, id: usize) -> &mut session::Play {
    &mut objects(cpu)[id].as_mut().expect("a live object").play
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
        play: session::Play::default(),
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
    let ids: Vec<usize> = objects(cpu)
        .iter()
        .enumerate()
        .filter(|(_, o)| o.as_ref().is_some_and(|o| o.pid == pid))
        .map(|(i, _)| i)
        .collect();
    for id in ids {
        leave(cpu, id);
        objects(cpu)[id] = None;
    }
}

/// The Win32 session ended at once, with the DOS process it ran in: every
/// object leaves its session, and DirectPlay starts again with none, as
/// the other DLLs do. Their processes, and the handles of their players'
/// events, are gone already.
pub fn session_ended(cpu: &mut Cpu) {
    for id in 0..objects(cpu).len() {
        if objects(cpu)[id].is_some() {
            leave(cpu, id);
        }
    }
    cpu.bus.win32.dplay = State::default();
}

/// The object `id` goes: it leaves its session, telling the others once
/// (they take it for gone in time if they don't hear it), and lets go of
/// its players' events.
fn leave(cpu: &mut Cpu, id: usize) {
    let now = cpu.bus.clock.now_micros();
    net::prepare(cpu, id);
    let p = play(cpu, id);
    p.close(now);
    p.flush(now);
    net::settle(cpu);
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

/// DirectPlayCreate(provider, out, outer): an IDirectPlay with the
/// provider, or with none for GUID_NULL (Initialize gives it one). The
/// outer object is looked at first, as Wine does.
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
    let provider = provider.unwrap_or(GUID_NULL);
    if provider != GUID_NULL && provider_code(&provider).is_none() {
        return Some(DPERR_UNAVAILABLE);
    }
    let Some(this) = new_object(cpu, PLAY) else {
        return Some(E_OUTOFMEMORY);
    };
    if provider != GUID_NULL {
        let (id, _) = object!(cpu, this);
        initialize(cpu, id, &provider);
    }
    com::write_out(cpu, out, this);
    Some(DP_OK)
}

/// DirectPlayEnumerateA and DirectPlayEnumerate(callback, context): the
/// service providers, with ANSI names; DirectPlayEnumerateW's are
/// Unicode.
fn direct_play_enumerate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    enumerate_providers(cpu, args, false)
}

fn direct_play_enumerate_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    enumerate_providers(cpu, args, true)
}

fn enumerate_providers(cpu: &mut Cpu, args: &[u32], wide: bool) -> Option<u32> {
    let [callback, context] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    if callback == 0 || !super::kernel32::virtual_memory::allows(cpu, callback, 1, false) {
        return Some(DPERR_INVALIDPARAMS);
    }
    let list = calls::List {
        kind: calls::PROVIDER_LIST,
        callback,
        context,
        wide,
        object: 0,
        version: 0,
    };
    calls::providers(cpu, list)
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
        leave(cpu, id);
        let gone = objects(cpu)[id].take()?;
        for this in gone.interfaces.into_iter().filter(|&i| i != 0) {
            com::free_interface(cpu, this);
        }
    }
    Some(refs)
}

/// Initialize(this, guid) of IDirectPlay2 and later: their objects are
/// initialized as they are made, and the method says so.
fn initialize_method(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    object!(cpu, args[0]);
    Some(DPERR_ALREADYINITIALIZED)
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

    /// An object whose process ended leaves its session after the
    /// process went (`remove_process`): the handle DirectPlay made for a
    /// player's event went with the process, and isn't closed in another.
    #[test]
    fn an_ended_process_leaves_its_session_without_its_handles() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let mut play = session::Play {
            provider: IPX,
            endpoint: 7,
            ..session::Play::default()
        };
        play.create(session::Info::default(), Vec::new(), 1);
        let player = play.create_entity(false, 0, None, None, vec![], 0, 5, 0);
        assert!(player.is_ok());
        // IDirectPlay's CreatePlayer made the event and its handle.
        play.session.as_mut().unwrap().entities[0].own_event = 8;
        *objects(&mut cpu) = vec![Some(Object {
            pid: 1,
            play,
            ..Object::default()
        })];
        process_ended(&mut cpu, 1);
        assert!(objects(&mut cpu)[0].is_none());
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
