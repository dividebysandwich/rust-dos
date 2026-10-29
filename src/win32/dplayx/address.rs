//! DirectPlay addresses, which the lobby makes and reads: a list of
//! chunks, each a DPADDRESS header (the GUID of the data's type and its
//! size) and the data, the first a DPAID_TotalSize chunk with the whole
//! address's size. CreateAddress and CreateCompoundAddress make one from
//! the program's data, EnumAddress calls the program back with each
//! chunk of one, DPAID_TotalSize's too as Wine does, and
//! EnumAddressTypes with the types of data a service provider's
//! addresses have, as the providers Windows 98 comes with register
//! them. Both are a task (task.rs) that calls the callback for one item
//! after another, until the list ends or the callback says to stop.

use super::{CHUNK_HEADER, DP_OK, DPERR_BUFFERTOOSMALL, DPERR_INVALIDPARAMS, object, object_of};
use crate::cpu::Cpu;
use crate::win32::com::Guid;
use crate::win32::task::{self, Next, Task};

const DPAID_TOTAL_SIZE: Guid = Guid::new(
    0x1318_F560,
    0x912C,
    0x11D0,
    [0x9D, 0xAA, 0x00, 0xA0, 0xC9, 0x0A, 0x43, 0xCB],
);
const DPAID_SERVICE_PROVIDER: Guid = Guid::new(
    0x07D9_16C0,
    0xE0AF,
    0x11CF,
    [0x9C, 0x4E, 0x00, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
);
const DPAID_INET: Guid = Guid::new(
    0xC4A5_4DA0,
    0xE0AF,
    0x11CF,
    [0x9C, 0x4E, 0x00, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
);
const DPAID_INET_W: Guid = Guid::new(
    0xE632_32A0,
    0x9DBF,
    0x11D0,
    [0x9C, 0xC1, 0x00, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
);
const DPAID_INET_PORT: Guid = Guid::new(
    0xE452_4541,
    0x8EA5,
    0x11D1,
    [0x8A, 0x96, 0x00, 0x60, 0x97, 0xB0, 0x14, 0x11],
);
const DPAID_PHONE: Guid = Guid::new(
    0x78EC_89A0,
    0xE0AF,
    0x11CF,
    [0x9C, 0x4E, 0x00, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
);
const DPAID_PHONE_W: Guid = Guid::new(
    0xBA5A_7A70,
    0x9DBF,
    0x11D0,
    [0x9C, 0xC1, 0x00, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
);
const DPAID_MODEM: Guid = Guid::new(
    0xF6DC_C200,
    0xA2FE,
    0x11D0,
    [0x9C, 0x4F, 0x00, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
);
const DPAID_MODEM_W: Guid = Guid::new(
    0x01FD_92E0,
    0xA2FF,
    0x11D0,
    [0x9C, 0x4F, 0x00, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
);
const DPAID_COM_PORT: Guid = Guid::new(
    0xF2F0_CE00,
    0xE0AF,
    0x11CF,
    [0x9C, 0x4E, 0x00, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
);

/// The service providers of DirectX 6 and the types of data their
/// addresses have: IPX's have none, TCP/IP's a host and a port, the
/// modem's a phone number and a modem, the serial link's a port's
/// settings.
const ADDRESS_TYPES: &[(Guid, &[Guid])] = &[
    (
        Guid::new(
            0x685B_C400,
            0x9D2C,
            0x11CF,
            [0xA9, 0xCD, 0x00, 0xAA, 0x00, 0x68, 0x86, 0xE3],
        ),
        &[],
    ),
    (
        Guid::new(
            0x36E9_5EE0,
            0x8577,
            0x11CF,
            [0x96, 0x0C, 0x00, 0x80, 0xC7, 0x53, 0x4E, 0x82],
        ),
        &[DPAID_INET, DPAID_INET_W, DPAID_INET_PORT],
    ),
    (
        Guid::new(
            0x44EA_A760,
            0xCB68,
            0x11CF,
            [0x9C, 0x4E, 0x00, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
        ),
        &[DPAID_PHONE, DPAID_PHONE_W, DPAID_MODEM, DPAID_MODEM_W],
    ),
    (
        Guid::new(
            0x0F1D_6860,
            0x88D9,
            0x11CF,
            [0x9C, 0x4E, 0x00, 0xA0, 0xC9, 0x05, 0x42, 0x5E],
        ),
        &[DPAID_COM_PORT],
    ),
];

/// More data than this in one chunk, or this many elements, is taken
/// for a bad size.
const MAX_DATA: u32 = 0x10_0000;
const MAX_ELEMENTS: u32 = 256;
/// A DPCOMPOUNDADDRESSELEMENT: the type's GUID, the size, the data.
const ELEMENT: u32 = 24;

/// The kinds of lists (the task's variable 0).
const CHUNKS: u32 = 0;
const TYPES: u32 = 1;

/// The task's variables: the kind, the callback, its context, where
/// the list is at, then the chunks' addresses or the provider's place
/// in ADDRESS_TYPES.
const KIND: usize = 0;
const CALLBACK: usize = 1;
const CONTEXT: usize = 2;
const NEXT: usize = 3;
const ITEMS: usize = 4;

/// The `size` bytes at `at`, or None if they can't be read.
fn read_data(cpu: &mut Cpu, at: u32, size: u32) -> Option<Vec<u8>> {
    if size == 0 {
        return Some(Vec::new());
    }
    if at == 0 || size > MAX_DATA || !cpu.bus.guest_probe(at, size as usize, false) {
        return None;
    }
    let mut data = vec![0; size as usize];
    cpu.bus.guest_read_bytes(at, &mut data);
    Some(data)
}

/// Whether `callback` is code the program may call.
fn callable(cpu: &mut Cpu, callback: u32) -> bool {
    callback != 0 && crate::win32::kernel32::virtual_memory::allows(cpu, callback, 1, false)
}

/// CreateAddress(this, provider, type, data, size, address, address
/// size): an address of the provider's chunk and one of the data. Each
/// chunk is as long as the program says its data is; Wine recomputes
/// the size of the string types (DPAID_INet, Phone, Modem and their W
/// forms) from the strings instead.
pub(super) fn create_address(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, provider, kind, data, size, address, address_size] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    object!(cpu, this);
    let (Some(provider), Some(kind), Some(data)) = (
        Guid::read(cpu, provider),
        Guid::read(cpu, kind),
        read_data(cpu, data, size),
    ) else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let chunks = [(DPAID_SERVICE_PROVIDER, provider.0.to_vec()), (kind, data)];
    Some(build(cpu, &chunks, address, address_size))
}

/// CreateCompoundAddress(this, elements, count, address, address size):
/// an address of a chunk for each element.
pub(super) fn create_compound_address(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, elements, count, address, address_size] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    object!(cpu, this);
    if elements == 0 || count == 0 || count > MAX_ELEMENTS {
        return Some(DPERR_INVALIDPARAMS);
    }
    if !cpu
        .bus
        .guest_probe(elements, (count * ELEMENT) as usize, false)
    {
        return Some(DPERR_INVALIDPARAMS);
    }
    let mut chunks = Vec::new();
    for i in 0..count {
        let at = elements + i * ELEMENT;
        let kind = Guid::read(cpu, at);
        let size = cpu.bus.guest_read_32(at + 16);
        let data_at = cpu.bus.guest_read_32(at + 20);
        let (Some(kind), Some(data)) = (kind, read_data(cpu, data_at, size)) else {
            return Some(DPERR_INVALIDPARAMS);
        };
        chunks.push((kind, data));
    }
    Some(build(cpu, &chunks, address, address_size))
}

/// Write the address of `chunks`, after its DPAID_TotalSize chunk, at
/// `address`, whose size is at `size_at`: the size it needs there
/// instead, and DPERR_BUFFERTOOSMALL, when there is no room or no
/// buffer.
fn build(cpu: &mut Cpu, chunks: &[(Guid, Vec<u8>)], address: u32, size_at: u32) -> u32 {
    if size_at == 0 || !cpu.bus.guest_probe(size_at, 4, true) {
        return DPERR_INVALIDPARAMS;
    }
    let data: u32 = chunks.iter().map(|(_, d)| d.len() as u32).sum();
    let required = CHUNK_HEADER * (1 + chunks.len() as u32) + 4 + data;
    let room = cpu.bus.guest_read_32(size_at);
    if address == 0 || room < required {
        cpu.bus.guest_write_32(size_at, required);
        return DPERR_BUFFERTOOSMALL;
    }
    if !cpu.bus.guest_probe(address, required as usize, true) {
        return DPERR_INVALIDPARAMS;
    }
    let mut bytes = Vec::with_capacity(required as usize);
    let total = (DPAID_TOTAL_SIZE, required.to_le_bytes().to_vec());
    for (kind, data) in std::iter::once(&total).chain(chunks) {
        bytes.extend_from_slice(&kind.0);
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(data);
    }
    cpu.bus.guest_write_bytes(address, &bytes);
    cpu.bus.guest_write_32(size_at, required);
    DP_OK
}

/// EnumAddress(this, callback, address, size, context): each chunk of
/// the address to the callback (type, size, data, context). An address
/// whose chunks run past its size is refused before any is.
pub(super) fn enum_address(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, callback, address, size, context] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    object!(cpu, this);
    if !callable(cpu, callback) || address == 0 {
        return Some(DPERR_INVALIDPARAMS);
    }
    if !cpu.bus.guest_probe(address, size as usize, false) {
        return Some(DPERR_INVALIDPARAMS);
    }
    let mut vars = vec![CHUNKS, callback, context, 0];
    let mut at = 0u64;
    while at < size as u64 {
        let chunk = address + at as u32;
        if at + CHUNK_HEADER as u64 > size as u64 {
            return Some(DPERR_INVALIDPARAMS);
        }
        let data = cpu.bus.guest_read_32(chunk + 16) as u64;
        at += CHUNK_HEADER as u64 + data;
        if at > size as u64 {
            return Some(DPERR_INVALIDPARAMS);
        }
        vars.push(chunk);
    }
    task::start(cpu, task::DPLAY_ENUM, vars, 0)
}

/// EnumAddressTypes(this, callback, provider, context, flags): the
/// types of the provider's addresses to the callback (type, context,
/// flags); none for a provider Windows 98 doesn't have.
pub(super) fn enum_types(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, callback, provider, context, flags] = args[..] else {
        return Some(DPERR_INVALIDPARAMS);
    };
    object!(cpu, this);
    if flags != 0 || !callable(cpu, callback) {
        return Some(DPERR_INVALIDPARAMS);
    }
    let Some(provider) = Guid::read(cpu, provider) else {
        return Some(DPERR_INVALIDPARAMS);
    };
    let Some(place) = ADDRESS_TYPES.iter().position(|(p, _)| *p == provider) else {
        return Some(DP_OK);
    };
    let vars = vec![TYPES, callback, context, 0, place as u32];
    task::start(cpu, task::DPLAY_ENUM, vars, 16)
}

/// The task's step: the next item to the callback, until there are no
/// more or the callback returned FALSE.
pub fn step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step == 1 && value == 0 {
        return Next::Done(DP_OK);
    }
    let next = t.var(NEXT);
    t.set(NEXT, next + 1);
    let (callback, context) = (t.var(CALLBACK), t.var(CONTEXT));
    if t.var(KIND) == TYPES {
        let types = ADDRESS_TYPES
            .get(t.var(ITEMS) as usize)
            .map_or(&[][..], |&(_, types)| types);
        let Some(kind) = types.get(next as usize) else {
            return Next::Done(DP_OK);
        };
        kind.write(cpu, t.scratch);
        return Next::call(callback, &[t.scratch, context, 0], 1);
    }
    let Some(&chunk) = t.vars.get(ITEMS + next as usize) else {
        return Next::Done(DP_OK);
    };
    let size = cpu.bus.guest_read_32(chunk + 16);
    let args = [chunk, size, chunk + CHUNK_HEADER, context];
    Next::call(callback, &args, 1)
}
