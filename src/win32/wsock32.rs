//! WSOCK32.DLL: Windows Sockets 1.1 without a network. WSAStartup
//! succeeds, so that programs that start Winsock with the rest of the
//! game go on; a socket can't be made (WSAENETDOWN), the local host's
//! name is the machine's and no other host's address is found. The byte
//! order functions and inet_addr work. Programs import the functions by
//! their ordinals, which are Windows 95's.

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "WSOCK32.DLL",
    base: 0xBFCC_0000,
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
    stdcall("accept", 1, 3, not_socket),
    stdcall("bind", 2, 3, not_socket),
    stdcall("closesocket", 3, 1, not_socket),
    stdcall("connect", 4, 3, not_socket),
    stdcall("getpeername", 5, 3, not_socket),
    stdcall("getsockname", 6, 3, not_socket),
    stdcall("getsockopt", 7, 5, not_socket),
    stdcall("htonl", 8, 1, swap32),
    stdcall("htons", 9, 1, swap16),
    stdcall("inet_addr", 10, 1, inet_addr),
    stdcall("inet_ntoa", 11, 1, null),
    stdcall("ioctlsocket", 12, 3, not_socket),
    stdcall("listen", 13, 2, not_socket),
    stdcall("ntohl", 14, 1, swap32),
    stdcall("ntohs", 15, 1, swap16),
    stdcall("recv", 16, 4, not_socket),
    stdcall("recvfrom", 17, 6, not_socket),
    stdcall("select", 18, 5, not_socket),
    stdcall("send", 19, 4, not_socket),
    stdcall("sendto", 20, 6, not_socket),
    stdcall("setsockopt", 21, 5, not_socket),
    stdcall("shutdown", 22, 2, not_socket),
    stdcall("socket", 23, 3, socket),
    stdcall("gethostbyaddr", 51, 3, no_host),
    stdcall("gethostbyname", 52, 1, no_host),
    stdcall("getprotobyname", 53, 1, no_host),
    stdcall("getprotobynumber", 54, 1, no_host),
    stdcall("getservbyname", 55, 2, no_host),
    stdcall("getservbyport", 56, 2, no_host),
    stdcall("gethostname", 57, 2, gethostname),
    stdcall("WSAAsyncSelect", 101, 4, not_socket),
    stdcall("WSAGetLastError", 111, 0, wsa_get_last_error),
    stdcall("WSASetLastError", 112, 1, wsa_set_last_error),
    stdcall("WSAStartup", 115, 2, wsa_startup),
    stdcall("WSACleanup", 116, 0, zero),
    stdcall("__WSAFDIsSet", 151, 2, zero),
    stdcall("WSAIsBlocking", 114, 0, zero),
    stdcall("WSACancelBlockingCall", 113, 0, zero),
];

const SOCKET_ERROR: u32 = u32::MAX;
const WSAENOTSOCK: u32 = 10038;
const WSAENETDOWN: u32 = 10050;
const WSAVERNOTSUPPORTED: u32 = 10092;
const WSAHOST_NOT_FOUND: u32 = 11001;
const WSAEFAULT: u32 = 10014;
/// The host's name, as gethostname gives it.
const HOST_NAME: &[u8] = b"RUSTDOS\0";

fn set_error(cpu: &mut Cpu, code: u32) {
    super::process(cpu).last_error = code;
}

pub(super) fn zero(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

pub(super) fn null(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// What there is no socket for: SOCKET_ERROR, WSAENOTSOCK.
pub(super) fn not_socket(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    set_error(cpu, WSAENOTSOCK);
    Some(SOCKET_ERROR)
}

/// socket: INVALID_SOCKET, the network down.
pub(super) fn socket(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    set_error(cpu, WSAENETDOWN);
    Some(SOCKET_ERROR)
}

/// The lookups of hosts, protocols and services: NULL.
pub(super) fn no_host(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    set_error(cpu, WSAHOST_NOT_FOUND);
    Some(0)
}

pub(super) fn swap32(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(args[0].swap_bytes())
}

pub(super) fn swap16(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some((args[0] as u16).swap_bytes() as u32)
}

/// inet_addr: a dotted address in network order, or INADDR_NONE.
pub(super) fn inet_addr(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] == 0 {
        return Some(u32::MAX);
    }
    let bytes = cpu.bus.guest_read_asciiz(args[0], 64);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let text = String::from_utf8_lossy(&bytes);
    let parts: Vec<Option<u8>> = text.trim().split('.').map(|p| p.parse().ok()).collect();
    match parts[..] {
        [Some(a), Some(b), Some(c), Some(d)] => Some(u32::from_le_bytes([a, b, c, d])),
        _ => Some(u32::MAX),
    }
}

/// gethostname(name, len).
pub(super) fn gethostname(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [at, len] = args[..] else {
        return Some(SOCKET_ERROR);
    };
    if at == 0 || (len as usize) < HOST_NAME.len() {
        set_error(cpu, WSAEFAULT);
        return Some(SOCKET_ERROR);
    }
    if !cpu.bus.guest_probe(at, HOST_NAME.len(), true) {
        return None;
    }
    cpu.bus.guest_write_bytes(at, HOST_NAME);
    Some(0)
}

pub(super) fn wsa_get_last_error(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(super::process(cpu).last_error)
}

pub(super) fn wsa_set_last_error(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_error(cpu, args[0]);
    Some(0)
}

/// WSAStartup(version, data): Winsock 1.1, in WSADATA (400 bytes).
fn wsa_startup(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    startup(cpu, args, 0x0101, b"WinSock 1.1 (no network)")
}

/// WSAStartup(version, data) of a Winsock whose highest version is
/// `highest` (minor version in the high byte, as WSADATA has it): the
/// version asked for, or the highest if it asks for a later one.
pub(super) fn startup(cpu: &mut Cpu, args: &[u32], highest: u16, text: &[u8]) -> Option<u32> {
    let [version, at] = args[..] else {
        return Some(WSAEFAULT);
    };
    let (major, minor) = (version & 0xFF, (version >> 8) & 0xFF);
    if major < 1 || (major == 1 && minor < 1) {
        return Some(WSAVERNOTSUPPORTED);
    }
    let asked = (minor << 8 | major) as u16;
    let swap = |v: u16| v.rotate_left(8);
    let used = if swap(asked) < swap(highest) { asked } else { highest };
    if at == 0 {
        return Some(WSAEFAULT);
    }
    if !cpu.bus.guest_probe(at, 400, true) {
        return None;
    }
    let mut data = vec![0u8; 400];
    data[0..2].copy_from_slice(&used.to_le_bytes());
    data[2..4].copy_from_slice(&highest.to_le_bytes());
    data[4..4 + text.len()].copy_from_slice(text);
    let status = b"Running";
    data[261..261 + status.len()].copy_from_slice(status);
    // iMaxSockets and iMaxUdpDg.
    data[390..392].copy_from_slice(&64u16.to_le_bytes());
    data[392..394].copy_from_slice(&512u16.to_le_bytes());
    cpu.bus.guest_write_bytes(at, &data);
    Some(0)
}
