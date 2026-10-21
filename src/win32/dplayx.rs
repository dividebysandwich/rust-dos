//! DPLAYX.DLL: DirectPlay, as on a machine with no network: there are no
//! service providers to list, DirectPlay objects can't be made, and no
//! lobby launched the program. Games of DirectX 5 and 6 import it for
//! their multiplayer menus and call it at startup to see whether a lobby
//! started them; with these answers they play alone.

use super::com::{self, Guid};
use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "DPLAYX.DLL",
    base: 0xBFAE_0000,
    apis: APIS,
    vars: &[],
    init: None,
    vtables: &[],
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

/// The functions, by DPLAYX.DLL's ordinals. Append only (see `stubs`).
static APIS: &[Api] = &[
    stdcall("DirectPlayCreate", 1, 3, direct_play_create),
    stdcall("DirectPlayEnumerateA", 2, 2, direct_play_enumerate),
    stdcall("DirectPlayEnumerateW", 3, 2, direct_play_enumerate),
    stdcall("DirectPlayLobbyCreateA", 4, 5, direct_play_lobby_create),
    stdcall("DirectPlayLobbyCreateW", 5, 5, direct_play_lobby_create),
    stdcall("DirectPlayEnumerate", 9, 2, direct_play_enumerate),
];

/// The classes CoCreateInstance makes of DPLAYX's (com.rs): DirectPlay
/// and its lobby, which fail as DirectPlayCreate does.
pub static CLASSES: &[com::Class] = &[
    com::Class {
        clsid: Guid::new(
            0xD1EB_6D20,
            0x8923,
            0x11D0,
            [0x9D, 0x97, 0x00, 0xA0, 0xC9, 0x0A, 0x43, 0xCB],
        ),
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
        create: create_instance,
    },
];

/// Make a DirectPlay or lobby object for CoCreateInstance: there is no
/// service provider to make it with.
fn create_instance(_: &mut Cpu, _: &Guid, _: u32) -> u32 {
    DPERR_UNAVAILABLE
}

const DP_OK: u32 = 0;
const fn dperr(code: u32) -> u32 {
    0x8877_0000 | code
}
const DPERR_UNAVAILABLE: u32 = dperr(250);
const DPERR_INVALIDPARAMS: u32 = super::com::E_INVALIDARG;

/// DirectPlayCreate(provider, out, outer): no service provider is there.
fn direct_play_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[1] == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    super::com::write_out(cpu, args[1], 0);
    Some(DPERR_UNAVAILABLE)
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
    super::com::write_out(cpu, args[1], 0);
    Some(DPERR_UNAVAILABLE)
}
