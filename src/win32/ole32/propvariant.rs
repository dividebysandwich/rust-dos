//! PROPVARIANTs: PropVariantClear and PropVariantCopy, for the types
//! Windows 95's OLE32 (that of NT 4) takes. A PROPVARIANT is 16 bytes:
//! the type (VT_), 6 bytes reserved, then the value, which for strings,
//! blobs and vectors points at memory of the task allocator.
//!
//! What they free and copy follows Wine's ole32/ole2.c (LGPL 2.1 or
//! later):
//!
//! Copyright 1995 Martin von Loewis
//! Copyright 1999 Francis Beaudet
//! Copyright 1999 Noel Borthwick
//! Copyright 1999, 2000 Marcus Meissner
//! Copyright 2005 Juan Lang

use super::{task_alloc, task_free};
use crate::cpu::Cpu;
use crate::win32::com::{E_OUTOFMEMORY, S_OK};

pub const STG_E_INVALIDPARAMETER: u32 = 0x8003_0057;
const DISP_E_BADVARTYPE: u32 = 0x8002_0008;

const VT_EMPTY: u16 = 0;
const VT_BSTR: u16 = 8;
const VT_VARIANT: u16 = 12;
const VT_LPSTR: u16 = 30;
const VT_LPWSTR: u16 = 31;
const VT_BLOB: u16 = 65;
const VT_STREAM: u16 = 66;
const VT_STORAGE: u16 = 67;
const VT_STREAMED_OBJECT: u16 = 68;
const VT_STORED_OBJECT: u16 = 69;
const VT_BLOB_OBJECT: u16 = 70;
const VT_CF: u16 = 71;
const VT_CLSID: u16 = 72;
const VT_VECTOR: u16 = 0x1000;

/// The size of a PROPVARIANT, and of a CLIPDATA.
const SIZE: u32 = 16;
const CLIPDATA: u32 = 12;

/// The size of an element of a vector of `vt`, if such a vector is
/// valid.
fn element_size(vt: u16) -> Option<u32> {
    Some(match vt {
        2 | 11 | 18 => 2,          // I2, BOOL, UI2
        3 | 4 | 10 | 19 => 4,      // I4, R4, ERROR, UI4
        5..=7 | 20 | 21 | 64 => 8, // R8, CY, DATE, I8, UI8, FILETIME
        17 => 1,                   // UI1
        VT_BSTR | VT_LPSTR | VT_LPWSTR => 4,
        VT_VARIANT => SIZE,
        VT_CF => CLIPDATA,
        VT_CLSID => 16,
        _ => return None,
    })
}

/// Whether a PROPVARIANT may have the type `vt`. Windows 95 has no
/// VT_DECIMAL, VT_I1, VT_INT, VT_UINT, VT_DISPATCH, VT_UNKNOWN or
/// VT_RECORD in one, and no arrays or references.
fn valid(vt: u16) -> bool {
    if vt & VT_VECTOR != 0 {
        return element_size(vt & !VT_VECTOR).is_some();
    }
    matches!(vt, 0..=8 | 10 | 11 | 17..=21 | VT_LPSTR | VT_LPWSTR | 64..=VT_CLSID)
}

/// Interfaces to release: (Release's address, interface).
pub type Releases = Vec<(u32, u32)>;

/// Clear the PROPVARIANT at `at`: free what it points at, note the
/// interfaces it holds in `releases`, and zero it. An HRESULT.
fn clear(cpu: &mut Cpu, at: u32, releases: &mut Releases, depth: u32) -> u32 {
    if !cpu.bus.guest_probe(at, SIZE as usize, true) {
        return STG_E_INVALIDPARAMETER;
    }
    let vt = cpu.bus.guest_read_16(at);
    if !valid(vt) {
        return STG_E_INVALIDPARAMETER;
    }
    let value = cpu.bus.guest_read_32(at + 8);
    let second = cpu.bus.guest_read_32(at + 12);
    match vt {
        VT_STREAM | VT_STORAGE | VT_STREAMED_OBJECT | VT_STORED_OBJECT => {
            if let Some(release) = super::stream::method(cpu, value, 2) {
                releases.push((release, value));
            }
        }
        VT_CLSID | VT_LPSTR | VT_LPWSTR => task_free(cpu, value),
        VT_BLOB | VT_BLOB_OBJECT => task_free(cpu, second),
        VT_BSTR => free_bstr(cpu, value),
        VT_CF => free_clipdata(cpu, value, true),
        _ if vt & VT_VECTOR != 0 => {
            let kind = vt & !VT_VECTOR;
            let (count, elements) = (value, second);
            let size = element_size(kind).unwrap_or(0);
            if elements != 0 {
                for i in 0..count.min(0x0100_0000) {
                    let e = elements.wrapping_add(i * size);
                    if !cpu.bus.guest_probe(e, size as usize, false) {
                        break;
                    }
                    match kind {
                        VT_BSTR => {
                            let s = cpu.bus.guest_read_32(e);
                            free_bstr(cpu, s);
                        }
                        VT_LPSTR | VT_LPWSTR => {
                            let s = cpu.bus.guest_read_32(e);
                            task_free(cpu, s);
                        }
                        VT_CF => free_clipdata(cpu, e, false),
                        VT_VARIANT if depth < 8 => {
                            clear(cpu, e, releases, depth + 1);
                        }
                        _ => {}
                    }
                }
                task_free(cpu, elements);
            }
        }
        _ => {}
    }
    cpu.bus.guest_fill(at, SIZE as usize, 0);
    S_OK
}

fn free_bstr(cpu: &mut Cpu, s: u32) {
    if s != 0 {
        task_free(cpu, s - 4);
    }
}

/// Free the data of the CLIPDATA at `at`, and the CLIPDATA itself when
/// `own` (a VT_CF's value points at one of its own).
fn free_clipdata(cpu: &mut Cpu, at: u32, own: bool) {
    if at == 0 || !cpu.bus.guest_probe(at, CLIPDATA as usize, false) {
        return;
    }
    let data = cpu.bus.guest_read_32(at + 8);
    task_free(cpu, data);
    if own {
        task_free(cpu, at);
    }
}

/// PropVariantClear(variant): the interfaces it holds are released after.
pub fn prop_variant_clear(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if at == 0 {
        return Some(S_OK);
    }
    let mut releases = Vec::new();
    let hr = clear(cpu, at, &mut releases, 0);
    call_all(cpu, hr, &releases)
}

/// Call each method of `calls` with its interface, then return `hr`.
pub fn call_all(cpu: &mut Cpu, hr: u32, calls: &[(u32, u32)]) -> Option<u32> {
    if calls.is_empty() {
        return Some(hr);
    }
    let mut vars = vec![super::calls::CALL_ALL, hr];
    for &(method, this) in calls {
        vars.extend([method, this]);
    }
    super::calls::start(cpu, vars, 0)
}

/// A copy of `n` bytes at `from` in a block of the task allocator, or
/// Err(()) when memory is out. 0 for a null pointer.
fn copy_block(cpu: &mut Cpu, from: u32, n: u32) -> Result<u32, ()> {
    if from == 0 {
        return Ok(0);
    }
    if !cpu.bus.guest_probe(from, n as usize, false) {
        return Err(());
    }
    let to = task_alloc(cpu, n);
    if to == 0 {
        return Err(());
    }
    let mut bytes = vec![0; n as usize];
    cpu.bus.guest_read_bytes(from, &mut bytes);
    cpu.bus.guest_write_bytes(to, &bytes);
    Ok(to)
}

/// The length in bytes of the string at `at` with its 0, of `unit`-byte
/// characters.
fn string_size(cpu: &mut Cpu, at: u32, unit: u32) -> u32 {
    let mut n = 0;
    loop {
        let p = at.wrapping_add(n);
        if !cpu.bus.guest_probe(p, unit as usize, false) {
            return n;
        }
        let c = if unit == 2 {
            cpu.bus.guest_read_16(p) as u32
        } else {
            cpu.bus.guest_read_8(p) as u32
        };
        n += unit;
        if c == 0 {
            return n;
        }
    }
}

/// A copy of the BSTR `s`, with its length.
fn copy_bstr(cpu: &mut Cpu, s: u32) -> Result<u32, ()> {
    if s == 0 {
        return Ok(0);
    }
    if !cpu.bus.guest_probe(s - 4, 4, false) {
        return Err(());
    }
    let len = cpu.bus.guest_read_32(s - 4);
    let to = copy_block(cpu, s - 4, len.saturating_add(6))?;
    Ok(to + 4)
}

/// Copy the CLIPDATA at `from` to `to`, its data copied too.
fn copy_clipdata(cpu: &mut Cpu, from: u32, to: u32) -> Result<(), ()> {
    if !cpu.bus.guest_probe(from, CLIPDATA as usize, false)
        || !cpu.bus.guest_probe(to, CLIPDATA as usize, true)
    {
        return Err(());
    }
    let size = cpu.bus.guest_read_32(from);
    let format = cpu.bus.guest_read_32(from + 4);
    let data = cpu.bus.guest_read_32(from + 8);
    // cbSize counts the format's 4 bytes.
    let data = copy_block(cpu, data, size.saturating_sub(4))?;
    cpu.bus.guest_write_32(to, size);
    cpu.bus.guest_write_32(to + 4, format);
    cpu.bus.guest_write_32(to + 8, data);
    Ok(())
}

/// Copy the PROPVARIANT at `src` to `dest`, deep; the interfaces it holds
/// are noted in `add_refs`. Err with the HRESULT.
fn copy(
    cpu: &mut Cpu,
    dest: u32,
    src: u32,
    add_refs: &mut Releases,
    depth: u32,
) -> Result<(), u32> {
    if !cpu.bus.guest_probe(src, SIZE as usize, false)
        || !cpu.bus.guest_probe(dest, SIZE as usize, true)
    {
        return Err(STG_E_INVALIDPARAMETER);
    }
    let vt = cpu.bus.guest_read_16(src);
    if !valid(vt) {
        return Err(DISP_E_BADVARTYPE);
    }
    let mut bytes = [0u8; SIZE as usize];
    cpu.bus.guest_read_bytes(src, &mut bytes);
    cpu.bus.guest_write_bytes(dest, &bytes);
    let value = cpu.bus.guest_read_32(src + 8);
    let second = cpu.bus.guest_read_32(src + 12);
    let oom = |_| E_OUTOFMEMORY;
    match vt {
        VT_STREAM | VT_STORAGE | VT_STREAMED_OBJECT | VT_STORED_OBJECT => {
            if let Some(add_ref) = super::stream::method(cpu, value, 1) {
                add_refs.push((add_ref, value));
            }
        }
        VT_CLSID => {
            let to = copy_block(cpu, value, 16).map_err(oom)?;
            cpu.bus.guest_write_32(dest + 8, to);
        }
        VT_LPSTR | VT_LPWSTR => {
            let unit = if vt == VT_LPWSTR { 2 } else { 1 };
            let n = if value == 0 {
                0
            } else {
                string_size(cpu, value, unit)
            };
            let to = copy_block(cpu, value, n).map_err(oom)?;
            cpu.bus.guest_write_32(dest + 8, to);
        }
        VT_BLOB | VT_BLOB_OBJECT => {
            let to = copy_block(cpu, second, value).map_err(oom)?;
            cpu.bus.guest_write_32(dest + 12, to);
        }
        VT_BSTR => {
            let to = copy_bstr(cpu, value).map_err(oom)?;
            cpu.bus.guest_write_32(dest + 8, to);
        }
        VT_CF => {
            if value != 0 {
                let to = task_alloc(cpu, CLIPDATA);
                if to == 0 {
                    return Err(E_OUTOFMEMORY);
                }
                copy_clipdata(cpu, value, to).map_err(oom)?;
                cpu.bus.guest_write_32(dest + 8, to);
            }
        }
        _ if vt & VT_VECTOR != 0 => {
            let kind = vt & !VT_VECTOR;
            let size = element_size(kind).unwrap_or(0);
            let (count, elements) = (value, second);
            let total = count.checked_mul(size).ok_or(E_OUTOFMEMORY)?;
            let to = copy_block(cpu, elements, total).map_err(oom)?;
            cpu.bus.guest_write_32(dest + 12, to);
            if to != 0 {
                for i in 0..count {
                    let (from, into) = (elements + i * size, to + i * size);
                    let s = cpu.bus.guest_read_32(from);
                    match kind {
                        VT_BSTR => {
                            let c = copy_bstr(cpu, s).map_err(oom)?;
                            cpu.bus.guest_write_32(into, c);
                        }
                        VT_LPSTR | VT_LPWSTR => {
                            let unit = if kind == VT_LPWSTR { 2 } else { 1 };
                            let n = if s == 0 { 0 } else { string_size(cpu, s, unit) };
                            let c = copy_block(cpu, s, n).map_err(oom)?;
                            cpu.bus.guest_write_32(into, c);
                        }
                        VT_CF => copy_clipdata(cpu, from, into).map_err(oom)?,
                        VT_VARIANT if depth < 8 => copy(cpu, into, from, add_refs, depth + 1)?,
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// PropVariantCopy(dest, src).
pub fn prop_variant_copy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[dest, src] = args else { unreachable!() };
    let mut add_refs = Vec::new();
    match copy(cpu, dest, src, &mut add_refs, 0) {
        Ok(()) => call_all(cpu, S_OK, &add_refs),
        Err(hr) => {
            if cpu.bus.guest_probe(dest, 2, true) {
                cpu.bus.guest_write_16(dest, VT_EMPTY);
            }
            Some(hr)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_windows_95_takes() {
        assert!(valid(VT_BSTR));
        assert!(valid(VT_CLSID));
        assert!(valid(VT_VARIANT | VT_VECTOR));
        assert!(!valid(VT_VARIANT));
        assert!(!valid(14)); // VT_DECIMAL
        assert!(!valid(VT_BSTR | 0x2000));
        assert!(!valid(VT_BLOB | VT_VECTOR));
    }
}
