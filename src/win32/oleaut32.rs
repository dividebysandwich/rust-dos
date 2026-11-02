//! OLEAUT32.DLL: the BSTR functions and VARIANTs' simplest ones, which
//! setup programs and OLE32's PROPVARIANTs use. A BSTR is a string of
//! Unicode characters whose length in bytes is in the 4 bytes before it,
//! and a zero after it, in a block of the task allocator, as Windows
//! has it.

use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "OLEAUT32.DLL",
    base: 0xBFE6_0000,
    apis: APIS,
    vars: &[],
    init: None,
    term: None,
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

/// The functions, by OLEAUT32's ordinals. Append only (see `stubs`).
static APIS: &[Api] = &[
    stdcall("SysAllocString", 2, 1, sys_alloc_string),
    stdcall("SysReAllocString", 3, 2, sys_realloc_string),
    stdcall("SysAllocStringLen", 4, 2, sys_alloc_string_len),
    stdcall("SysReAllocStringLen", 5, 3, sys_realloc_string_len),
    stdcall("SysFreeString", 6, 1, sys_free_string),
    stdcall("SysStringLen", 7, 1, sys_string_len),
    stdcall("VariantInit", 8, 1, variant_init),
    stdcall("VariantClear", 9, 1, variant_clear),
    stdcall("SysStringByteLen", 149, 1, sys_string_byte_len),
    stdcall("SysAllocStringByteLen", 150, 2, sys_alloc_string_byte_len),
];

const S_OK: u32 = 0;
const DISP_E_BADVARTYPE: u32 = 0x8002_0008;

/// A new BSTR of `len` bytes, copied from `from` if it isn't 0: its
/// address, or 0 when memory is out.
fn alloc(cpu: &mut Cpu, from: u32, len: u32) -> u32 {
    let Some(size) = len.checked_add(6) else {
        return 0;
    };
    let block = super::ole32::task_alloc(cpu, size);
    if block == 0 {
        return 0;
    }
    let mut bytes = vec![0u8; size as usize];
    bytes[..4].copy_from_slice(&len.to_le_bytes());
    if from != 0 && cpu.bus.guest_probe(from, len as usize, false) {
        cpu.bus
            .guest_read_bytes(from, &mut bytes[4..4 + len as usize]);
    }
    cpu.bus.guest_write_bytes(block, &bytes);
    block + 4
}

/// The length in characters of the Unicode string at `at`.
fn wide_len(cpu: &mut Cpu, at: u32) -> u32 {
    let mut n = 0;
    while cpu.bus.guest_probe(at + 2 * n, 2, false) && cpu.bus.guest_read_16(at + 2 * n) != 0 {
        n += 1;
    }
    n
}

/// The length in bytes of the BSTR `s`.
fn byte_len(cpu: &mut Cpu, s: u32) -> u32 {
    if s < 4 || !cpu.bus.guest_probe(s - 4, 4, false) {
        return 0;
    }
    cpu.bus.guest_read_32(s - 4)
}

/// SysAllocString(text): NULL for NULL.
fn sys_alloc_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let text = args[0];
    if text == 0 {
        return Some(0);
    }
    let n = wide_len(cpu, text);
    Some(alloc(cpu, text, 2 * n))
}

/// SysAllocStringLen(text, characters): uninitialized when text is NULL.
fn sys_alloc_string_len(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[text, n] = args else { unreachable!() };
    if n >= 0x8000_0000 / 2 {
        return Some(0);
    }
    Some(alloc(cpu, text, 2 * n))
}

/// SysAllocStringByteLen(bytes, length).
fn sys_alloc_string_byte_len(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[text, n] = args else { unreachable!() };
    if n >= 0x8000_0000 {
        return Some(0);
    }
    Some(alloc(cpu, text, n))
}

/// SysReAllocString(place, text): a new BSTR at `place`, the old freed.
fn sys_realloc_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[place, text] = args else { unreachable!() };
    let n = if text == 0 { 0 } else { wide_len(cpu, text) };
    realloc(cpu, place, text, n)
}

/// SysReAllocStringLen(place, text, characters).
fn sys_realloc_string_len(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[place, text, n] = args else {
        unreachable!()
    };
    if n >= 0x8000_0000 / 2 {
        return Some(0);
    }
    realloc(cpu, place, text, n)
}

fn realloc(cpu: &mut Cpu, place: u32, text: u32, n: u32) -> Option<u32> {
    if place == 0 || !cpu.bus.guest_probe(place, 4, true) {
        return Some(0);
    }
    let new = alloc(cpu, text, 2 * n);
    if new == 0 {
        return Some(0);
    }
    let old = cpu.bus.guest_read_32(place);
    free(cpu, old);
    cpu.bus.guest_write_32(place, new);
    Some(1)
}

fn free(cpu: &mut Cpu, s: u32) {
    if s >= 4 {
        super::ole32::task_free(cpu, s - 4);
    }
}

fn sys_free_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    free(cpu, args[0]);
    Some(0)
}

fn sys_string_len(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(byte_len(cpu, args[0]) / 2)
}

fn sys_string_byte_len(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(byte_len(cpu, args[0]))
}

/// VariantInit(variant): VT_EMPTY.
fn variant_init(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if cpu.bus.guest_probe(args[0], 2, true) {
        cpu.bus.guest_write_16(args[0], 0);
    }
    Some(0)
}

/// VariantClear(variant): a BSTR is freed, an interface released.
/// Arrays (VT_ARRAY) aren't taken yet.
fn variant_clear(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if !cpu.bus.guest_probe(at, 16, true) {
        return Some(S_OK);
    }
    let vt = cpu.bus.guest_read_16(at);
    let value = cpu.bus.guest_read_32(at + 8);
    let mut release = None;
    if vt & 0x4000 == 0 {
        match vt {
            // VT_BSTR.
            8 => free(cpu, value),
            // VT_DISPATCH, VT_UNKNOWN.
            9 | 13 => release = super::ole32::method(cpu, value, 2).map(|m| (m, value)),
            _ if vt & 0x2000 != 0 => return Some(DISP_E_BADVARTYPE),
            _ => {}
        }
    }
    cpu.bus.guest_write_16(at, 0);
    match release {
        Some(call) => super::ole32::call_all(cpu, S_OK, &[call]),
        None => Some(S_OK),
    }
}
