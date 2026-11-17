//! IDirectPlay8Address: a list of named components, each a string, a
//! number, a GUID or bytes, and user data, which a program builds and
//! reads one component at a time or as a URL:
//! "x-directplay:/provider=%7BEBFE7BA0-628D-11D2-AE0F-006097B01411%7D;hostname=host;port=2302".
//! In a URL a value's characters other than letters, digits and "-._"
//! are %XX, a number is in decimal, a GUID is in braces, and user data
//! follows a "#". The service provider is the component "provider" and
//! the adapter "device", both GUIDs.

use super::{
    Class, DPN_OK, DPNERR_BUFFERTOOSMALL, DPNERR_DOESNOTEXIST, DPNERR_INVALIDADDRESSFORMAT,
    DPNERR_INVALIDPARAM, DPNERR_INVALIDPOINTER, DPNERR_INVALIDURL, DPNSUCCESS_EQUAL,
    DPNSUCCESS_NOTEQUAL, new_object, object, object_id, object_of, read_u32, read_wide, writable,
};
use crate::cpu::Cpu;
use crate::win32::com::{self, E_OUTOFMEMORY, Guid};
use crate::win32::dplayx::address::{
    DPAID_INET, DPAID_INET_PORT, DPAID_INET_W, DPAID_SERVICE_PROVIDER,
};
use crate::win32::dplayx::{CHUNK_HEADER, DPSPGUID_IPX, DPSPGUID_TCPIP};

/// A component's types of data.
pub const DPNA_DATATYPE_STRING: u32 = 1;
pub const DPNA_DATATYPE_DWORD: u32 = 2;
pub const DPNA_DATATYPE_GUID: u32 = 3;
pub const DPNA_DATATYPE_BINARY: u32 = 4;
/// Taken by AddComponent, and kept as a Unicode string.
const DPNA_DATATYPE_STRING_ANSI: u32 = 5;

const PROVIDER: &str = "provider";
const DEVICE: &str = "device";
const HOSTNAME: &str = "hostname";
const PORT: &str = "port";
const SCHEME: &str = "x-directplay:/";

/// DirectPlay 8's service providers.
const fn dp8sp(d1: u32) -> Guid {
    Guid::new(
        d1,
        0x628D,
        0x11D2,
        [0xAE, 0x0F, 0x00, 0x60, 0x97, 0xB0, 0x14, 0x11],
    )
}
pub const CLSID_DP8SP_TCPIP: Guid = dp8sp(0xEBFE_7BA0);
pub const CLSID_DP8SP_IPX: Guid = dp8sp(0x5393_4290);
pub const CLSID_DP8SP_MODEM: Guid = dp8sp(0x6D4A_3650);
pub const CLSID_DP8SP_SERIAL: Guid = dp8sp(0x743B_5D60);

/// DirectPlay's (DPLAYX's) modem and serial service providers.
const DPSPGUID_MODEM: Guid = Guid::new(
    0x44EA_A760,
    0xCB68,
    0x11CF,
    [0x9C, 0x4E, 0x00, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
);
const DPSPGUID_SERIAL: Guid = Guid::new(
    0x0F1D_6860,
    0x88D9,
    0x11CF,
    [0x9C, 0x4E, 0x00, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
);

/// A component of an address. A string is UTF-16 with its zero.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    pub kind: u32,
    pub data: Vec<u8>,
}

crate::state_fields!(Component { name, kind, data });

impl Component {
    fn guid(name: &str, guid: &Guid) -> Component {
        Component {
            name: name.to_string(),
            kind: DPNA_DATATYPE_GUID,
            data: guid.0.to_vec(),
        }
    }

    fn string(name: &str, text: &[u16]) -> Component {
        let mut data: Vec<u8> = text.iter().flat_map(|c| c.to_le_bytes()).collect();
        data.extend([0, 0]);
        Component {
            name: name.to_string(),
            kind: DPNA_DATATYPE_STRING,
            data,
        }
    }

    fn dword(name: &str, value: u32) -> Component {
        Component {
            name: name.to_string(),
            kind: DPNA_DATATYPE_DWORD,
            data: value.to_le_bytes().to_vec(),
        }
    }
}

/// The component `name` of `components`, its name in any case.
pub fn find<'a>(components: &'a [Component], name: &str) -> Option<&'a Component> {
    components
        .iter()
        .find(|c| c.name.eq_ignore_ascii_case(name))
}

/// Put `c` in `components`, in place of the one of its name.
fn put(components: &mut Vec<Component>, c: Component) {
    match components
        .iter_mut()
        .find(|o| o.name.eq_ignore_ascii_case(&c.name))
    {
        Some(old) => *old = c,
        None => components.push(c),
    }
}

/// A value's characters as a URL has them.
fn escape(out: &mut Vec<u16>, units: impl IntoIterator<Item = u16>) {
    for c in units {
        let plain = c < 0x80 && ((c as u8).is_ascii_alphanumeric() || b"-._".contains(&(c as u8)));
        if plain || c >= 0x100 {
            out.push(c);
        } else {
            out.extend(format!("%{:02X}", c).encode_utf16());
        }
    }
}

/// The URL of an address.
pub fn url(components: &[Component], user_data: &[u8]) -> Vec<u16> {
    let mut out: Vec<u16> = SCHEME.encode_utf16().collect();
    // The provider and the device first, then the rest as added.
    let mut ordered: Vec<&Component> = Vec::new();
    for name in [PROVIDER, DEVICE] {
        ordered.extend(find(components, name));
    }
    ordered.extend(components.iter().filter(|c| {
        !c.name.eq_ignore_ascii_case(PROVIDER) && !c.name.eq_ignore_ascii_case(DEVICE)
    }));
    for (i, c) in ordered.into_iter().enumerate() {
        if i > 0 {
            out.push(b';' as u16);
        }
        escape(&mut out, c.name.encode_utf16());
        out.push(b'=' as u16);
        match c.kind {
            DPNA_DATATYPE_STRING => {
                let units = c
                    .data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&p| u16::from_le_bytes(p))
                    .take_while(|&u| u != 0);
                escape(&mut out, units);
            }
            DPNA_DATATYPE_DWORD => {
                let v = u32::from_le_bytes(c.data[..4].try_into().unwrap_or_default());
                out.extend(v.to_string().encode_utf16());
            }
            DPNA_DATATYPE_GUID => {
                let g = Guid(c.data.clone().try_into().unwrap_or_default());
                escape(&mut out, g.to_string().encode_utf16());
            }
            _ => escape(&mut out, c.data.iter().map(|&b| b as u16)),
        }
    }
    if !user_data.is_empty() {
        out.push(b'#' as u16);
        escape(&mut out, user_data.iter().map(|&b| b as u16));
    }
    out
}

/// `text` with its %XX turned into the characters they are, or None for
/// a "%" without two hexadecimal digits after it.
fn unescape(text: &[u16]) -> Option<Vec<u16>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if text[i] == b'%' as u16 {
            let digits = text.get(i + 1..i + 3)?;
            if !digits
                .iter()
                .all(|&d| d < 0x80 && (d as u8).is_ascii_hexdigit())
            {
                return None;
            }
            let hex = String::from_utf16_lossy(digits);
            out.push(u16::from_str_radix(&hex, 16).ok()?);
            i += 3;
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
    Some(out)
}

/// The components and the user data of the URL `text`, or None if it
/// isn't one.
pub fn parse(text: &[u16]) -> Option<(Vec<Component>, Vec<u8>)> {
    let scheme: Vec<u16> = SCHEME.encode_utf16().collect();
    let head = text.get(..scheme.len())?;
    if String::from_utf16_lossy(head).to_ascii_lowercase() != SCHEME {
        return None;
    }
    let rest = &text[scheme.len()..];
    let (body, data) = match rest.iter().position(|&c| c == b'#' as u16) {
        Some(at) => (&rest[..at], Some(&rest[at + 1..])),
        None => (rest, None),
    };
    let user_data = match data {
        Some(d) => unescape(d)?
            .into_iter()
            .map(|c| u8::try_from(c).ok())
            .collect::<Option<Vec<u8>>>()?,
        None => Vec::new(),
    };
    let mut components = Vec::new();
    for part in body.split(|&c| c == b';' as u16).filter(|p| !p.is_empty()) {
        let eq = part.iter().position(|&c| c == b'=' as u16)?;
        let name = String::from_utf16(&unescape(&part[..eq])?).ok()?;
        if name.is_empty() {
            return None;
        }
        let value = unescape(&part[eq + 1..])?;
        let text = String::from_utf16_lossy(&value);
        let component = if let Some(g) = Guid::parse(&text) {
            Component::guid(&name, &g)
        } else if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
            Component::dword(&name, text.parse().ok()?)
        } else {
            Component::string(&name, &value)
        };
        put(&mut components, component);
    }
    Some((components, user_data))
}

/// The id of the address `other` is, if it is one.
fn other_address(cpu: &mut Cpu, other: u32) -> Option<usize> {
    match object_of(cpu, other) {
        Ok((id, Class::Address)) => Some(id),
        _ => None,
    }
}

fn set(cpu: &mut Cpu, id: usize, components: Vec<Component>, user_data: Vec<u8>) {
    let o = object(cpu, id);
    o.components = components;
    o.user_data = user_data;
}

/// BuildFromURLW(this, url): the address the URL is, in place of the
/// one there was.
pub(super) fn build_from_url_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = object_id!(cpu, args[0]);
    let Some(text) = read_wide(cpu, args[1]) else {
        return Some(DPNERR_INVALIDPOINTER);
    };
    build(cpu, id, &text)
}

/// BuildFromURLA(this, url).
pub(super) fn build_from_url_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = object_id!(cpu, args[0]);
    if args[1] == 0 {
        return Some(DPNERR_INVALIDPOINTER);
    }
    let bytes = cpu.bus.guest_read_asciiz(args[1], 0x1_0000);
    if cpu.bus.guest_faulted() {
        return Some(DPNERR_INVALIDPOINTER);
    }
    let text = crate::win32::kernel32::ansi_to_wide(cpu, &bytes);
    build(cpu, id, &text)
}

fn build(cpu: &mut Cpu, id: usize, text: &[u16]) -> Option<u32> {
    match parse(text) {
        Some((components, user_data)) => {
            set(cpu, id, components, user_data);
            Some(DPN_OK)
        }
        None => Some(DPNERR_INVALIDURL),
    }
}

/// Duplicate(this, out): a new address the same as this one.
pub(super) fn duplicate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = object_id!(cpu, args[0]);
    if !com::write_out(cpu, args[1], 0) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    let (components, user_data) = {
        let o = object(cpu, id);
        (o.components.clone(), o.user_data.clone())
    };
    let Some((copy_id, copy)) = new_object(cpu, Class::Address) else {
        return Some(E_OUTOFMEMORY);
    };
    set(cpu, copy_id, components, user_data);
    com::write_out(cpu, args[1], copy);
    Some(DPN_OK)
}

/// SetEqual(this, other): this address made the same as `other`.
pub(super) fn set_equal(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = object_id!(cpu, args[0]);
    let Some(other) = other_address(cpu, args[1]) else {
        return Some(DPNERR_INVALIDPOINTER);
    };
    let o = object(cpu, other);
    let (components, user_data) = (o.components.clone(), o.user_data.clone());
    set(cpu, id, components, user_data);
    Some(DPN_OK)
}

/// IsEqual(this, other): DPNSUCCESS_EQUAL when their URLs are the same
/// but for case, DPNSUCCESS_NOTEQUAL when they aren't.
pub(super) fn is_equal(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = object_id!(cpu, args[0]);
    let Some(other) = other_address(cpu, args[1]) else {
        return Some(DPNERR_INVALIDPOINTER);
    };
    let url_of = |cpu: &mut Cpu, id| {
        let o = object(cpu, id);
        String::from_utf16_lossy(&url(&o.components, &o.user_data)).to_ascii_lowercase()
    };
    let same = url_of(cpu, id) == url_of(cpu, other);
    Some(if same {
        DPNSUCCESS_EQUAL
    } else {
        DPNSUCCESS_NOTEQUAL
    })
}

/// Clear(this): no components and no user data.
pub(super) fn clear(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = object_id!(cpu, args[0]);
    set(cpu, id, Vec::new(), Vec::new());
    Some(DPN_OK)
}

/// GetURLW(this, buffer, chars): the URL with its zero, and its length
/// in characters at `chars`; DPNERR_BUFFERTOOSMALL with the length
/// needed if `chars` says the buffer is shorter.
pub(super) fn get_url_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_url(cpu, args, true)
}

/// GetURLA(this, buffer, chars): GetURLW's URL in the ANSI code page.
pub(super) fn get_url_a(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_url(cpu, args, false)
}

fn get_url(cpu: &mut Cpu, args: &[u32], wide: bool) -> Option<u32> {
    let [this, buffer, chars_at] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    let id = object_id!(cpu, this);
    let Some(room) = read_u32(cpu, chars_at) else {
        return Some(DPNERR_INVALIDPOINTER);
    };
    let o = object(cpu, id);
    let text = url(&o.components, &o.user_data);
    let mut bytes = if wide {
        text.iter().flat_map(|c| c.to_le_bytes()).collect()
    } else {
        crate::win32::kernel32::wide_to_ansi(cpu, &text)
    };
    let unit = if wide { 2 } else { 1 };
    bytes.extend(std::iter::repeat_n(0, unit));
    let needed = (bytes.len() / unit) as u32;
    if !writable(cpu, chars_at, 4) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    cpu.bus.guest_write_32(chars_at, needed);
    if room < needed {
        return Some(DPNERR_BUFFERTOOSMALL);
    }
    if !writable(cpu, buffer, bytes.len() as u32) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    cpu.bus.guest_write_bytes(buffer, &bytes);
    Some(DPN_OK)
}

/// GetSP and GetDevice(this, out): the GUID of the component.
pub(super) fn get_sp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_guid(cpu, args, PROVIDER)
}

pub(super) fn get_device(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_guid(cpu, args, DEVICE)
}

fn get_guid(cpu: &mut Cpu, args: &[u32], name: &str) -> Option<u32> {
    let id = object_id!(cpu, args[0]);
    if !writable(cpu, args[1], 16) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    let found = find(&object(cpu, id).components, name)
        .filter(|c| c.kind == DPNA_DATATYPE_GUID)
        .map(|c| Guid(c.data.clone().try_into().unwrap_or_default()));
    match found {
        Some(g) => {
            g.write(cpu, args[1]);
            Some(DPN_OK)
        }
        None => Some(DPNERR_DOESNOTEXIST),
    }
}

/// SetSP and SetDevice(this, guid).
pub(super) fn set_sp(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_guid(cpu, args, PROVIDER)
}

pub(super) fn set_device(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    set_guid(cpu, args, DEVICE)
}

fn set_guid(cpu: &mut Cpu, args: &[u32], name: &str) -> Option<u32> {
    let id = object_id!(cpu, args[0]);
    let Some(g) = Guid::read(cpu, args[1]) else {
        return Some(DPNERR_INVALIDPOINTER);
    };
    put(&mut object(cpu, id).components, Component::guid(name, &g));
    Some(DPN_OK)
}

/// GetUserData(this, buffer, size).
pub(super) fn get_user_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, buffer, size_at] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    let id = object_id!(cpu, this);
    let Some(room) = read_u32(cpu, size_at) else {
        return Some(DPNERR_INVALIDPOINTER);
    };
    let data = object(cpu, id).user_data.clone();
    if data.is_empty() {
        return Some(DPNERR_DOESNOTEXIST);
    }
    copy_out(cpu, &data, buffer, size_at, room)
}

/// Write `data` at `buffer` if `room` bytes take it, and its size at
/// `size_at` either way.
fn copy_out(cpu: &mut Cpu, data: &[u8], buffer: u32, size_at: u32, room: u32) -> Option<u32> {
    if !writable(cpu, size_at, 4) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    cpu.bus.guest_write_32(size_at, data.len() as u32);
    if room < data.len() as u32 {
        return Some(DPNERR_BUFFERTOOSMALL);
    }
    if !writable(cpu, buffer, data.len() as u32) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    cpu.bus.guest_write_bytes(buffer, data);
    Some(DPN_OK)
}

/// SetUserData(this, data, size): none for a size of 0.
pub(super) fn set_user_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, data, size] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    let id = object_id!(cpu, this);
    let bytes = if size == 0 {
        Vec::new()
    } else {
        if data == 0 || !cpu.bus.guest_probe(data, size as usize, false) {
            return Some(DPNERR_INVALIDPOINTER);
        }
        let mut bytes = vec![0; size as usize];
        cpu.bus.guest_read_bytes(data, &mut bytes);
        bytes
    };
    object(cpu, id).user_data = bytes;
    Some(DPN_OK)
}

/// GetNumComponents(this, out).
pub(super) fn get_num_components(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = object_id!(cpu, args[0]);
    let count = object(cpu, id).components.len() as u32;
    if !com::write_out(cpu, args[1], count) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    Some(DPN_OK)
}

/// GetComponentByName(this, name, buffer, size, type).
pub(super) fn get_component_by_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, name, buffer, size_at, type_at] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    let id = object_id!(cpu, this);
    let Some(name) = read_wide(cpu, name) else {
        return Some(DPNERR_INVALIDPOINTER);
    };
    let Some(room) = read_u32(cpu, size_at) else {
        return Some(DPNERR_INVALIDPOINTER);
    };
    if !writable(cpu, type_at, 4) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    let name = String::from_utf16_lossy(&name);
    let Some(c) = find(&object(cpu, id).components, &name).cloned() else {
        return Some(DPNERR_DOESNOTEXIST);
    };
    cpu.bus.guest_write_32(type_at, c.kind);
    copy_out(cpu, &c.data, buffer, size_at, room)
}

/// GetComponentByIndex(this, index, name, name chars, buffer, size,
/// type): the component's name with its zero, and its data, if both
/// buffers take them; their sizes either way.
pub(super) fn get_component_by_index(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, index, name, chars_at, buffer, size_at, type_at] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    let id = object_id!(cpu, this);
    let (Some(name_room), Some(room)) = (read_u32(cpu, chars_at), read_u32(cpu, size_at)) else {
        return Some(DPNERR_INVALIDPOINTER);
    };
    if !writable(cpu, chars_at, 4) || !writable(cpu, size_at, 4) || !writable(cpu, type_at, 4) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    let Some(c) = object(cpu, id).components.get(index as usize).cloned() else {
        return Some(DPNERR_DOESNOTEXIST);
    };
    let mut name_bytes: Vec<u8> = c.name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    name_bytes.extend([0, 0]);
    let chars = name_bytes.len() as u32 / 2;
    cpu.bus.guest_write_32(chars_at, chars);
    cpu.bus.guest_write_32(size_at, c.data.len() as u32);
    cpu.bus.guest_write_32(type_at, c.kind);
    if name_room < chars || room < c.data.len() as u32 {
        return Some(DPNERR_BUFFERTOOSMALL);
    }
    if !writable(cpu, name, name_bytes.len() as u32)
        || (!c.data.is_empty() && !writable(cpu, buffer, c.data.len() as u32))
    {
        return Some(DPNERR_INVALIDPOINTER);
    }
    cpu.bus.guest_write_bytes(name, &name_bytes);
    cpu.bus.guest_write_bytes(buffer, &c.data);
    Some(DPN_OK)
}

/// AddComponent(this, name, data, size, type): the component, in place
/// of one of its name. A number takes 4 bytes and a GUID 16; a string's
/// size counts its zero, and an ANSI string is kept as Unicode.
pub(super) fn add_component(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, name, data, size, kind] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    let id = object_id!(cpu, this);
    let Some(name) = read_wide(cpu, name) else {
        return Some(DPNERR_INVALIDPOINTER);
    };
    if data == 0 || !cpu.bus.guest_probe(data, size as usize, false) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    let mut bytes = vec![0; size as usize];
    cpu.bus.guest_read_bytes(data, &mut bytes);
    let name = String::from_utf16_lossy(&name);
    let component = match kind {
        DPNA_DATATYPE_DWORD if size == 4 => Component {
            name,
            kind,
            data: bytes,
        },
        DPNA_DATATYPE_GUID if size == 16 => Component {
            name,
            kind,
            data: bytes,
        },
        DPNA_DATATYPE_BINARY => Component {
            name,
            kind,
            data: bytes,
        },
        DPNA_DATATYPE_STRING => {
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&p| u16::from_le_bytes(p))
                .collect();
            match units.iter().position(|&u| u == 0) {
                Some(end) if size % 2 == 0 && end + 1 == units.len() => {
                    Component::string(&name, &units[..end])
                }
                _ => return Some(DPNERR_INVALIDPARAM),
            }
        }
        DPNA_DATATYPE_STRING_ANSI => match bytes.iter().position(|&b| b == 0) {
            Some(end) if end + 1 == bytes.len() => {
                let units = crate::win32::kernel32::ansi_to_wide(cpu, &bytes[..end]);
                Component::string(&name, &units)
            }
            _ => return Some(DPNERR_INVALIDPARAM),
        },
        _ => return Some(DPNERR_INVALIDPARAM),
    };
    put(&mut object(cpu, id).components, component);
    Some(DPN_OK)
}

/// BuildFromDPADDRESS(this, address, size): the address of a DirectPlay
/// address (DPLAYX's DPADDRESS chunks), as a lobby of DirectPlay 4 gives
/// one: its service provider as DirectPlay 8's, its host name and its
/// port. Other chunks are left out.
pub(super) fn build_from_dpaddress(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, at, size] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    let id = object_id!(cpu, this);
    if at == 0 || size == 0 || !cpu.bus.guest_probe(at, size as usize, false) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    let mut bytes = vec![0; size as usize];
    cpu.bus.guest_read_bytes(at, &mut bytes);
    match from_dpaddress(cpu, &bytes) {
        Some(components) => {
            set(cpu, id, components, Vec::new());
            Some(DPN_OK)
        }
        None => Some(DPNERR_INVALIDADDRESSFORMAT),
    }
}

/// The components of the DPADDRESS `bytes`, or None without a service
/// provider DirectPlay 8 has.
fn from_dpaddress(cpu: &Cpu, bytes: &[u8]) -> Option<Vec<Component>> {
    let mut components = Vec::new();
    let mut provider = None;
    let mut rest = bytes;
    while rest.len() >= CHUNK_HEADER as usize {
        let kind = Guid(rest[..16].try_into().ok()?);
        let len = u32::from_le_bytes(rest[16..20].try_into().ok()?) as usize;
        let data = rest.get(20..20 + len)?;
        rest = &rest[20 + len..];
        match kind {
            DPAID_SERVICE_PROVIDER => {
                let sp = Guid(data.get(..16)?.try_into().ok()?);
                provider = Some(match sp {
                    DPSPGUID_TCPIP => CLSID_DP8SP_TCPIP,
                    DPSPGUID_IPX => CLSID_DP8SP_IPX,
                    DPSPGUID_MODEM => CLSID_DP8SP_MODEM,
                    DPSPGUID_SERIAL => CLSID_DP8SP_SERIAL,
                    _ => return None,
                });
            }
            DPAID_INET => {
                let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
                let units = crate::win32::kernel32::ansi_to_wide(cpu, &data[..end]);
                put(&mut components, Component::string(HOSTNAME, &units));
            }
            DPAID_INET_W => {
                let units: Vec<u16> = data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&p| u16::from_le_bytes(p))
                    .take_while(|&u| u != 0)
                    .collect();
                put(&mut components, Component::string(HOSTNAME, &units));
            }
            DPAID_INET_PORT => {
                let port = u16::from_le_bytes(data.get(..2)?.try_into().ok()?);
                put(&mut components, Component::dword(PORT, port as u32));
            }
            _ => {}
        }
    }
    components.insert(0, Component::guid(PROVIDER, &provider?));
    Some(components)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn a_url_names_the_provider_first_and_escapes_its_values() {
        let components = vec![
            Component::string(HOSTNAME, &wide("my host;1")),
            Component::dword(PORT, 2302),
            Component::guid(PROVIDER, &CLSID_DP8SP_TCPIP),
        ];
        let text = String::from_utf16(&url(&components, b"#x")).unwrap();
        assert_eq!(
            text,
            "x-directplay:/provider=%7BEBFE7BA0-628D-11D2-AE0F-006097B01411%7D;\
             hostname=my%20host%3B1;port=2302#%23x"
        );
    }

    #[test]
    fn a_url_is_read_back_as_the_components_it_was_made_of() {
        let components = vec![
            Component::guid(PROVIDER, &CLSID_DP8SP_IPX),
            Component::string(HOSTNAME, &wide("a=b")),
            Component::dword(PORT, 47624),
        ];
        let text = url(&components, &[1, 2, 0xFF]);
        assert_eq!(parse(&text), Some((components, vec![1, 2, 0xFF])));
        assert_eq!(
            parse(&wide("X-DirectPlay:/")),
            Some((Vec::new(), Vec::new()))
        );
        assert_eq!(parse(&wide("http://x")), None);
        assert_eq!(parse(&wide("x-directplay:/port")), None);
        assert_eq!(parse(&wide("x-directplay:/port=%4")), None);
        assert_eq!(parse(&wide("x-directplay:/port=%+4")), None);
    }
}
