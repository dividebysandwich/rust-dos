//! IDirectPlay8Peer, with no service provider that can start. It
//! initializes, keeps its caps, and lists the service providers
//! DirectX 8.1 registers; hosting, joining and looking for a session
//! fail on the device's service provider, and what needs a session
//! fails with DPNERR_NOCONNECTION.

use super::address::{self, DPNA_DATATYPE_GUID};
use super::{
    Class, DPN_OK, DPNERR_BUFFERTOOSMALL, DPNERR_DOESNOTEXIST, DPNERR_INVALIDDEVICEADDRESS,
    DPNERR_INVALIDFLAGS, DPNERR_INVALIDHANDLE, DPNERR_INVALIDHOSTADDRESS, DPNERR_INVALIDPARAM,
    DPNERR_INVALIDPOINTER, DPNERR_NOCONNECTION, DPNERR_NOTREGISTERED, DPNERR_UNSUPPORTED,
    initialized, object, object_of, read_u32, writable,
};
use crate::cpu::Cpu;
use crate::win32::com::Guid;

/// EnumServiceProviders' one flag: every registered provider, whether
/// it can start or not.
const DPNENUMSERVICEPROVIDERS_ALL: u32 = 1;
/// CancelAsyncOperation's flags for every operation of a kind.
const DPNCANCEL_ALL: u32 = 0x0000_0001 | 0x0000_0002 | 0x0000_0004 | 0x0000_8000;
/// RegisterLobby's flags.
const DPNLOBBY_REGISTER: u32 = 1;
const DPNLOBBY_UNREGISTER: u32 = 2;

/// DPN_CAPS: its size, then flags, connect timeout (ms), connect
/// retries and time until keep-alive (ms), as a new object has them.
const DPN_CAPS_SIZE: u32 = 20;
pub const DEFAULT_CAPS: [u32; 4] = [0, 200, 14, 60_000];

/// DPN_SERVICE_PROVIDER_INFO: flags, GUID, name, two reserved words.
const SP_INFO_SIZE: u32 = 32;

/// The service providers DirectX 8.1 registers, in the order its
/// registry lists them, with their friendly names.
pub const PROVIDERS: [(Guid, &str); 4] = [
    (
        address::CLSID_DP8SP_TCPIP,
        "DirectPlay8 TCP/IP Service Provider",
    ),
    (address::CLSID_DP8SP_IPX, "DirectPlay8 IPX Service Provider"),
    (
        address::CLSID_DP8SP_MODEM,
        "DirectPlay8 Modem Service Provider",
    ),
    (
        address::CLSID_DP8SP_SERIAL,
        "DirectPlay8 Serial Service Provider",
    ),
];

/// Whether the provider `guid` can start: none can, as the built-in
/// WinSock opens no sockets, TAPI has no modem, and DirectPlay 8 has no
/// way yet to the serial ports.
fn available(_: &Guid) -> bool {
    false
}

/// What a method gets for the provider `guid`: DPNERR_DOESNOTEXIST for
/// one that isn't registered, DPNERR_UNSUPPORTED for one that can't
/// start.
fn provider_error(guid: &Guid) -> u32 {
    if PROVIDERS.iter().any(|(g, _)| g == guid) {
        DPNERR_UNSUPPORTED
    } else {
        DPNERR_DOESNOTEXIST
    }
}

/// Initialize(this, context, handler, flags).
pub(super) fn initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, handler, flags] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    Some(super::initialize(cpu, this, handler, flags))
}

/// Close(this, flags): the peer is as before its Initialize.
pub(super) fn close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = initialized!(cpu, args[0]);
    let o = object(cpu, id);
    o.initialized = false;
    o.caps = DEFAULT_CAPS;
    Some(DPN_OK)
}

/// EnumServiceProviders(this, provider, application, buffer, size,
/// count, flags): the providers that can start, or every registered
/// one with DPNENUMSERVICEPROVIDERS_ALL, as DPN_SERVICE_PROVIDER_INFO
/// with their names after them; or, given a provider, its adapters.
pub(super) fn enum_service_providers(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, provider, _, buffer, size_at, count_at, flags] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    initialized!(cpu, this);
    if flags & !DPNENUMSERVICEPROVIDERS_ALL != 0 {
        return Some(DPNERR_INVALIDFLAGS);
    }
    let Some(room) = read_u32(cpu, size_at) else {
        return Some(DPNERR_INVALIDPOINTER);
    };
    if !writable(cpu, size_at, 4) || !writable(cpu, count_at, 4) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    if provider != 0 {
        // A provider's adapters: it has to start to find them.
        let Some(guid) = Guid::read(cpu, provider) else {
            return Some(DPNERR_INVALIDPOINTER);
        };
        return Some(provider_error(&guid));
    }
    let listed: Vec<&(Guid, &str)> = PROVIDERS
        .iter()
        .filter(|(g, _)| flags & DPNENUMSERVICEPROVIDERS_ALL != 0 || available(g))
        .collect();
    let names: Vec<Vec<u8>> = listed
        .iter()
        .map(|(_, name)| {
            let mut bytes: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
            bytes.extend([0, 0]);
            bytes
        })
        .collect();
    let needed =
        SP_INFO_SIZE * listed.len() as u32 + names.iter().map(|n| n.len() as u32).sum::<u32>();
    cpu.bus.guest_write_32(size_at, needed);
    cpu.bus.guest_write_32(count_at, listed.len() as u32);
    if room < needed {
        return Some(DPNERR_BUFFERTOOSMALL);
    }
    if needed > 0 && !writable(cpu, buffer, needed) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    let mut name_at = buffer + SP_INFO_SIZE * listed.len() as u32;
    for (i, ((guid, _), name)) in listed.iter().zip(&names).enumerate() {
        let entry = buffer + SP_INFO_SIZE * i as u32;
        cpu.bus.guest_fill(entry, SP_INFO_SIZE as usize, 0);
        guid.write(cpu, entry + 4);
        cpu.bus.guest_write_32(entry + 20, name_at);
        cpu.bus.guest_write_bytes(name_at, name);
        name_at += name.len() as u32;
    }
    Some(DPN_OK)
}

/// CancelAsyncOperation(this, handle, flags): there are no operations,
/// so cancelling all of a kind does nothing, and a handle is none of
/// them.
pub(super) fn cancel_async_operation(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, handle, flags] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    initialized!(cpu, this);
    if handle != 0 {
        return Some(if flags != 0 {
            DPNERR_INVALIDFLAGS
        } else {
            DPNERR_INVALIDHANDLE
        });
    }
    if flags == 0 || flags & !DPNCANCEL_ALL != 0 {
        return Some(DPNERR_INVALIDFLAGS);
    }
    Some(DPN_OK)
}

/// The service provider of the address `this`, an IDirectPlay8Address:
/// Err(DPNERR_INVALIDPOINTER) if it isn't one, Ok(None) if it names no
/// provider.
fn provider_of(cpu: &mut Cpu, this: u32) -> Result<Option<Guid>, u32> {
    match object_of(cpu, this) {
        Ok((id, Class::Address)) => {
            let component = address::find(&object(cpu, id).components, "provider");
            Ok(component.and_then(|c| {
                (c.kind == DPNA_DATATYPE_GUID)
                    .then(|| Guid(c.data.clone().try_into().unwrap_or_default()))
            }))
        }
        _ => Err(DPNERR_INVALIDPOINTER),
    }
}

/// The error of an operation on the device `device` (or, without one,
/// the host address `host`), whose provider can't start: no provider
/// named is a bad device or host address, a registered one that can't
/// start a bad device address, as its adapter can't be opened.
fn no_provider(cpu: &mut Cpu, device: u32, host: u32) -> u32 {
    let (at, missing) = if device != 0 {
        (device, DPNERR_INVALIDDEVICEADDRESS)
    } else {
        (host, DPNERR_INVALIDHOSTADDRESS)
    };
    match provider_of(cpu, at) {
        Err(e) => e,
        Ok(None) => missing,
        Ok(Some(guid)) if PROVIDERS.iter().any(|(g, _)| *g == guid) => DPNERR_INVALIDDEVICEADDRESS,
        Ok(Some(_)) => DPNERR_DOESNOTEXIST,
    }
}

/// Host(this, application, devices, count, security, credentials,
/// context, flags): a session can't be hosted on a device whose
/// provider doesn't start.
pub(super) fn host(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, application, devices, count, ..] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    initialized!(cpu, this);
    if application == 0 || devices == 0 || count == 0 {
        return Some(DPNERR_INVALIDPARAM);
    }
    let Some(first) = read_u32(cpu, devices) else {
        return Some(DPNERR_INVALIDPOINTER);
    };
    Some(no_provider(cpu, first, 0))
}

/// Connect(this, application, host, device, security, credentials,
/// data, size, player context, async context, handle, flags): as Host,
/// on the device or, without one, the host address's provider.
pub(super) fn connect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, application, host, device, ..] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    initialized!(cpu, this);
    if application == 0 || host == 0 {
        return Some(DPNERR_INVALIDPARAM);
    }
    Some(no_provider(cpu, device, host))
}

/// EnumHosts(this, application, host, device, data, size, count,
/// interval, timeout, context, handle, flags): as Connect, though the
/// host address may be left out.
pub(super) fn enum_hosts(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, application, host, device, ..] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    initialized!(cpu, this);
    if application == 0 || (host == 0 && device == 0) {
        return Some(DPNERR_INVALIDPARAM);
    }
    Some(no_provider(cpu, device, host))
}

/// The methods of a session (sending, groups, players, the
/// application's description): there is none.
pub(super) fn no_connection(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    initialized!(cpu, args[0]);
    Some(DPNERR_NOCONNECTION)
}

/// SetPeerInfo(this, info, context, handle, flags): taken before a
/// session for the one the peer makes or joins, which it doesn't.
pub(super) fn set_peer_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, info, ..] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    initialized!(cpu, this);
    if info == 0 {
        return Some(DPNERR_INVALIDPOINTER);
    }
    Some(DPN_OK)
}

/// ReturnBuffer(this, handle, flags): no message gave a buffer.
pub(super) fn return_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    initialized!(cpu, args[0]);
    Some(DPNERR_INVALIDHANDLE)
}

/// GetCaps(this, caps, flags): DPN_CAPS as SetCaps left it.
pub(super) fn get_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, caps, flags] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    let id = initialized!(cpu, this);
    if flags != 0 {
        return Some(DPNERR_INVALIDFLAGS);
    }
    if !writable(cpu, caps, DPN_CAPS_SIZE) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    if cpu.bus.guest_read_32(caps) != DPN_CAPS_SIZE {
        return Some(DPNERR_INVALIDPARAM);
    }
    let values = object(cpu, id).caps;
    for (i, v) in values.into_iter().enumerate() {
        cpu.bus.guest_write_32(caps + 4 + 4 * i as u32, v);
    }
    Some(DPN_OK)
}

/// SetCaps(this, caps, flags).
pub(super) fn set_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, caps, flags] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    let id = initialized!(cpu, this);
    if flags != 0 {
        return Some(DPNERR_INVALIDFLAGS);
    }
    if caps == 0 || !cpu.bus.guest_probe(caps, DPN_CAPS_SIZE as usize, false) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    if cpu.bus.guest_read_32(caps) != DPN_CAPS_SIZE {
        return Some(DPNERR_INVALIDPARAM);
    }
    let values: [u32; 4] = std::array::from_fn(|i| cpu.bus.guest_read_32(caps + 4 + 4 * i as u32));
    object(cpu, id).caps = values;
    Some(DPN_OK)
}

/// GetSPCaps and SetSPCaps(this, provider, caps, flags): the provider
/// has to start for its caps.
pub(super) fn sp_caps(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, provider, caps, flags] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    initialized!(cpu, this);
    if flags != 0 {
        return Some(DPNERR_INVALIDFLAGS);
    }
    if caps == 0 {
        return Some(DPNERR_INVALIDPOINTER);
    }
    match Guid::read(cpu, provider) {
        Some(guid) => Some(provider_error(&guid)),
        None => Some(DPNERR_INVALIDPOINTER),
    }
}

/// RegisterLobby(this, handle, lobbied application, flags): a peer
/// reports to the lobby that launched the program, which none did, so
/// no handle is a lobby connection.
pub(super) fn register_lobby(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, lobbied, flags] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    initialized!(cpu, this);
    match flags {
        DPNLOBBY_REGISTER if lobbied == 0 => Some(DPNERR_INVALIDPOINTER),
        DPNLOBBY_REGISTER => Some(DPNERR_INVALIDHANDLE),
        DPNLOBBY_UNREGISTER => Some(DPNERR_NOTREGISTERED),
        _ => Some(DPNERR_INVALIDFLAGS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_that_cant_start_is_told_from_one_that_isnt_there() {
        assert_eq!(
            provider_error(&address::CLSID_DP8SP_TCPIP),
            DPNERR_UNSUPPORTED
        );
        assert_eq!(provider_error(&Guid([7; 16])), DPNERR_DOESNOTEXIST);
    }
}
