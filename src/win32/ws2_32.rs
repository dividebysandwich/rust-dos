//! WS2_32.DLL: Windows Sockets 2 without a network, as WSOCK32.DLL's
//! Winsock 1.1 is (wsock32.rs): the same functions at the same
//! ordinals, and WSAStartup gives up to version 2.2, Windows 98's.
//! Games import it for their multiplayer code and start it with the
//! rest of the game (Zanzarah).

use super::stubs::{Api, Conv, Dll};
use super::wsock32 as w;
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "WS2_32.DLL",
    base: 0xBF7C_0000,
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

/// The functions. Append only (see `stubs`).
static APIS: &[Api] = &[
    stdcall("accept", 1, 3, w::not_socket),
    stdcall("bind", 2, 3, w::not_socket),
    stdcall("closesocket", 3, 1, w::not_socket),
    stdcall("connect", 4, 3, w::not_socket),
    stdcall("getpeername", 5, 3, w::not_socket),
    stdcall("getsockname", 6, 3, w::not_socket),
    stdcall("getsockopt", 7, 5, w::not_socket),
    stdcall("htonl", 8, 1, w::swap32),
    stdcall("htons", 9, 1, w::swap16),
    stdcall("ioctlsocket", 10, 3, w::not_socket),
    stdcall("inet_addr", 11, 1, w::inet_addr),
    stdcall("inet_ntoa", 12, 1, w::null),
    stdcall("listen", 13, 2, w::not_socket),
    stdcall("ntohl", 14, 1, w::swap32),
    stdcall("ntohs", 15, 1, w::swap16),
    stdcall("recv", 16, 4, w::not_socket),
    stdcall("recvfrom", 17, 6, w::not_socket),
    stdcall("select", 18, 5, w::not_socket),
    stdcall("send", 19, 4, w::not_socket),
    stdcall("sendto", 20, 6, w::not_socket),
    stdcall("setsockopt", 21, 5, w::not_socket),
    stdcall("shutdown", 22, 2, w::not_socket),
    stdcall("socket", 23, 3, w::socket),
    stdcall("gethostbyaddr", 51, 3, w::no_host),
    stdcall("gethostbyname", 52, 1, w::no_host),
    stdcall("getprotobyname", 53, 1, w::no_host),
    stdcall("getprotobynumber", 54, 1, w::no_host),
    stdcall("getservbyname", 55, 2, w::no_host),
    stdcall("getservbyport", 56, 2, w::no_host),
    stdcall("gethostname", 57, 2, w::gethostname),
    stdcall("WSAAsyncSelect", 101, 4, w::not_socket),
    stdcall("WSACancelBlockingCall", 113, 0, w::zero),
    stdcall("WSAIsBlocking", 114, 0, w::zero),
    stdcall("WSAGetLastError", 111, 0, w::wsa_get_last_error),
    stdcall("WSASetLastError", 112, 1, w::wsa_set_last_error),
    stdcall("WSAStartup", 115, 2, wsa_startup),
    stdcall("WSACleanup", 116, 0, w::zero),
    stdcall("__WSAFDIsSet", 151, 2, w::zero),
];

/// WSAStartup(version, data): Winsock 2.2 at most.
fn wsa_startup(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    w::startup(cpu, args, 0x0202, b"WinSock 2.0 (no network)")
}
