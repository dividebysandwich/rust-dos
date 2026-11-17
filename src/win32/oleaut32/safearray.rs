//! SAFEARRAYs, as Wine's oleaut32/safearray.c lays them out and Windows
//! does: a descriptor (the dimensions' count, features, the size of an
//! element, a lock count, the data's address and the bounds of each
//! dimension, the last dimension's first), with 16 bytes before it for
//! the IID of an array of interfaces or, in the last 4 of them, the
//! VARTYPE of the elements; the elements in a block of their own. The
//! first index of SafeArrayGetElement's list varies fastest. Arrays of
//! BSTRs, VARIANTs and interfaces own what their elements hold: the
//! functions copy BSTRs, add references to interfaces and let go of them
//! as the array does.
//!
//! The layout follows Wine's safearray.c (LGPL 2.1 or later):
//!
//! Copyright 1999 Sylvain St-Germain
//! Copyright 2002-2003 Marcus Meissner
//! Copyright 2003 Jon Griffiths

use crate::cpu::Cpu;
use crate::win32::com::{self, E_INVALIDARG, E_OUTOFMEMORY, Guid, S_OK};

const DISP_E_ARRAYISLOCKED: u32 = 0x8002_000D;
const DISP_E_BADINDEX: u32 = 0x8002_000B;
const E_UNEXPECTED: u32 = 0x8000_FFFF;

/// fFeatures.
const FADF_HAVEIID: u16 = 0x40;
const FADF_HAVEVARTYPE: u16 = 0x80;
const FADF_BSTR: u16 = 0x100;
const FADF_UNKNOWN: u16 = 0x200;
const FADF_DISPATCH: u16 = 0x400;
const FADF_VARIANT: u16 = 0x800;
const FADF_CREATEVECTOR: u16 = 0x2000;

const VT_BSTR: u16 = 8;
const VT_DISPATCH: u16 = 9;
const VT_VARIANT: u16 = 12;
const VT_UNKNOWN: u16 = 13;
const VT_ARRAY: u16 = 0x2000;
const VT_BYREF: u16 = 0x4000;

/// The descriptor's fields.
const DIMS: u32 = 0;
const FEATURES: u32 = 2;
const ELEMENT: u32 = 4;
const LOCKS: u32 = 8;
const DATA: u32 = 12;
const BOUNDS: u32 = 16;
/// The bytes before the descriptor.
const PREFIX: u32 = 16;

/// The most dimensions and elements an array is given.
const MAX_DIMS: u32 = 64;
const MAX_BYTES: u64 = 0x1000_0000;

const IID_IDISPATCH: Guid = Guid::new(0x0002_0400, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

/// An array as its descriptor says.
pub struct Array {
    pub at: u32,
    pub vt: u16,
    pub element: u32,
    pub data: u32,
    /// Each dimension's count of elements and lower bound, as the
    /// descriptor has them: the last dimension first.
    pub bounds: Vec<(u32, i32)>,
}

impl Array {
    /// The count of elements.
    pub fn count(&self) -> u32 {
        self.bounds
            .iter()
            .fold(1u64, |n, &(c, _)| n.saturating_mul(c as u64))
            .min(u32::MAX as u64) as u32
    }
}

/// The size of an element of the type `vt`, or None for a type an array
/// can't hold.
fn element_size(vt: u16) -> Option<u32> {
    Some(match vt {
        16 | 17 => 1,
        2 | 11 | 18 => 2,
        3 | 4 | 10 | 19 | 22 | 23 | VT_BSTR | VT_DISPATCH | VT_UNKNOWN => 4,
        5 | 6 | 7 | 20 | 21 => 8,
        VT_VARIANT | 14 => 16,
        _ => return None,
    })
}

fn features_of(vt: u16) -> u16 {
    match vt {
        VT_BSTR => FADF_BSTR | FADF_HAVEVARTYPE,
        VT_UNKNOWN => FADF_UNKNOWN | FADF_HAVEIID,
        VT_DISPATCH => FADF_DISPATCH | FADF_HAVEIID,
        VT_VARIANT => FADF_VARIANT | FADF_HAVEVARTYPE,
        _ => FADF_HAVEVARTYPE,
    }
}

/// The array at `at`, if a descriptor can be read there.
pub fn describe(cpu: &mut Cpu, at: u32) -> Option<Array> {
    if at < PREFIX || !cpu.bus.guest_probe(at, BOUNDS as usize, false) {
        return None;
    }
    let dims = cpu.bus.guest_read_16(at + DIMS) as u32;
    if dims == 0 || dims > MAX_DIMS || !cpu.bus.guest_probe(at, (BOUNDS + 8 * dims) as usize, false)
    {
        return None;
    }
    let features = cpu.bus.guest_read_16(at + FEATURES);
    let element = cpu.bus.guest_read_32(at + ELEMENT);
    let data = cpu.bus.guest_read_32(at + DATA);
    let bounds = (0..dims)
        .map(|i| {
            let b = at + BOUNDS + 8 * i;
            (
                cpu.bus.guest_read_32(b),
                cpu.bus.guest_read_32(b + 4) as i32,
            )
        })
        .collect();
    let vt = if features & FADF_BSTR != 0 {
        VT_BSTR
    } else if features & FADF_VARIANT != 0 {
        VT_VARIANT
    } else if features & FADF_DISPATCH != 0 {
        VT_DISPATCH
    } else if features & FADF_UNKNOWN != 0 {
        VT_UNKNOWN
    } else if features & FADF_HAVEVARTYPE != 0 && cpu.bus.guest_probe(at - 4, 4, false) {
        cpu.bus.guest_read_32(at - 4) as u16
    } else {
        match element {
            1 => 17,
            2 => 2,
            8 => 5,
            16 => VT_VARIANT,
            _ => 3,
        }
    };
    let a = Array {
        at,
        vt,
        element,
        data,
        bounds,
    };
    let bytes = a.count() as u64 * element as u64;
    if bytes > MAX_BYTES {
        return None;
    }
    Some(a)
}

/// A descriptor for `dims` dimensions of elements of the type `vt`, with
/// no data: its address, or 0.
fn new_descriptor(cpu: &mut Cpu, vt: u16, dims: u32) -> u32 {
    let size = PREFIX + BOUNDS + 8 * dims;
    let block = crate::win32::ole32::task_alloc(cpu, size);
    if block == 0 {
        return 0;
    }
    cpu.bus.guest_fill(block, size as usize, 0);
    let at = block + PREFIX;
    let features = features_of(vt);
    cpu.bus.guest_write_16(at + DIMS, dims as u16);
    cpu.bus.guest_write_16(at + FEATURES, features);
    cpu.bus
        .guest_write_32(at + ELEMENT, element_size(vt).unwrap_or(0));
    if features & FADF_HAVEIID != 0 {
        let iid = if vt == VT_DISPATCH {
            IID_IDISPATCH
        } else {
            com::IID_IUNKNOWN
        };
        cpu.bus.guest_write_bytes(block, &iid.0);
    } else {
        cpu.bus.guest_write_32(at - 4, vt as u32);
    }
    at
}

/// A new array of elements of the type `vt`, each `element` bytes (0 for
/// the type's size), with `bounds` as the descriptor has them, its
/// elements zeroed. None when `vt` can't be in an array or memory is
/// out.
pub fn create(cpu: &mut Cpu, vt: u16, element: u32, bounds: &[(u32, i32)]) -> Option<Array> {
    let size = element_size(vt)?;
    if element != 0 && element != size {
        return None;
    }
    if bounds.is_empty() || bounds.len() as u32 > MAX_DIMS {
        return None;
    }
    let at = new_descriptor(cpu, vt, bounds.len() as u32);
    if at == 0 {
        return None;
    }
    for (i, &(n, low)) in bounds.iter().enumerate() {
        let b = at + BOUNDS + 8 * i as u32;
        cpu.bus.guest_write_32(b, n);
        cpu.bus.guest_write_32(b + 4, low as u32);
    }
    if !alloc_data(cpu, at) {
        free_descriptor(cpu, at);
        return None;
    }
    describe(cpu, at)
}

/// Give the array at `at` a zeroed block for its elements.
fn alloc_data(cpu: &mut Cpu, at: u32) -> bool {
    let Some(a) = describe(cpu, at) else {
        return false;
    };
    let bytes = a.count() as u64 * a.element as u64;
    if bytes > MAX_BYTES {
        return false;
    }
    let data = crate::win32::ole32::task_alloc(cpu, (bytes as u32).max(1));
    if data == 0 {
        return false;
    }
    cpu.bus.guest_fill(data, bytes as usize, 0);
    cpu.bus.guest_write_32(at + DATA, data);
    true
}

fn free_descriptor(cpu: &mut Cpu, at: u32) {
    crate::win32::ole32::task_free(cpu, at - PREFIX);
}

/// Free the array at `at`, its data and descriptor, without letting go
/// of what its elements hold (the caller has).
pub fn free(cpu: &mut Cpu, at: u32) {
    let Some(a) = describe(cpu, at) else {
        return;
    };
    crate::win32::ole32::task_free(cpu, a.data);
    free_descriptor(cpu, at);
}

/// Let go of what the element at `at` of the type `vt` holds: the
/// Release calls it needs go into `calls`.
fn clear_element(cpu: &mut Cpu, vt: u16, at: u32, calls: &mut Vec<(u32, u32)>) {
    match vt {
        VT_BSTR => {
            let s = cpu.bus.guest_read_32(at);
            super::free_bstr(cpu, s);
            cpu.bus.guest_write_32(at, 0);
        }
        VT_DISPATCH | VT_UNKNOWN => {
            let p = cpu.bus.guest_read_32(at);
            if let Some(m) = crate::win32::ole32::method(cpu, p, 2) {
                calls.push((m, p));
            }
            cpu.bus.guest_write_32(at, 0);
        }
        VT_VARIANT => clear_variant(cpu, at, calls),
        _ => {}
    }
}

/// VariantClear's work on the VARIANT at `at`: the Release calls it
/// needs go into `calls`.
pub fn clear_variant(cpu: &mut Cpu, at: u32, calls: &mut Vec<(u32, u32)>) {
    let vt = cpu.bus.guest_read_16(at);
    if vt & VT_BYREF == 0 {
        if vt & VT_ARRAY != 0 {
            let p = cpu.bus.guest_read_32(at + 8);
            destroy(cpu, p, calls);
        } else if matches!(vt, VT_BSTR | VT_DISPATCH | VT_UNKNOWN) {
            clear_element(cpu, vt, at + 8, calls);
        }
    }
    cpu.bus.guest_write_16(at, 0);
}

/// Copy the element of the type `vt` at `from` to `to`, which holds
/// nothing: the AddRef calls it needs go into `calls`. False when memory
/// is out.
fn copy_element(cpu: &mut Cpu, vt: u16, to: u32, from: u32, calls: &mut Vec<(u32, u32)>) -> bool {
    match vt {
        VT_BSTR => {
            let s = cpu.bus.guest_read_32(from);
            let copy = super::copy_bstr(cpu, s);
            cpu.bus.guest_write_32(to, copy);
            s == 0 || copy != 0
        }
        VT_DISPATCH | VT_UNKNOWN => {
            let p = cpu.bus.guest_read_32(from);
            if let Some(m) = crate::win32::ole32::method(cpu, p, 1) {
                calls.push((m, p));
            }
            cpu.bus.guest_write_32(to, p);
            true
        }
        VT_VARIANT => copy_variant(cpu, to, from, calls),
        _ => {
            let n = element_size(vt).unwrap_or(4) as usize;
            let mut b = vec![0; n];
            cpu.bus.guest_read_bytes(from, &mut b);
            cpu.bus.guest_write_bytes(to, &b);
            true
        }
    }
}

/// VariantCopy's work, from `from` to the VARIANT `to`, which holds
/// nothing: a BSTR copied, an interface with a reference added, an array
/// copied, a VT_BYREF pointer as it is.
pub fn copy_variant(cpu: &mut Cpu, to: u32, from: u32, calls: &mut Vec<(u32, u32)>) -> bool {
    let vt = cpu.bus.guest_read_16(from);
    let mut b = [0; 16];
    cpu.bus.guest_read_bytes(from, &mut b);
    cpu.bus.guest_write_bytes(to, &b);
    if vt & VT_BYREF != 0 {
        return true;
    }
    if vt & VT_ARRAY != 0 {
        let p = cpu.bus.guest_read_32(from + 8);
        let copy = copy(cpu, p, calls);
        cpu.bus.guest_write_32(to + 8, copy);
        return p == 0 || copy != 0;
    }
    if matches!(vt, VT_BSTR | VT_DISPATCH | VT_UNKNOWN) {
        return copy_element(cpu, vt, to + 8, from + 8, calls);
    }
    true
}

/// A copy of the array at `at` and its elements, or 0.
fn copy(cpu: &mut Cpu, at: u32, calls: &mut Vec<(u32, u32)>) -> u32 {
    let Some(a) = describe(cpu, at) else {
        return 0;
    };
    let Some(b) = create(cpu, a.vt, a.element, &a.bounds) else {
        return 0;
    };
    for i in 0..a.count() {
        let (to, from) = (b.data + i * a.element, a.data + i * a.element);
        copy_element(cpu, a.vt, to, from, calls);
    }
    b.at
}

/// SafeArrayDestroy's work: the elements let go of, the array freed.
fn destroy(cpu: &mut Cpu, at: u32, calls: &mut Vec<(u32, u32)>) {
    let Some(a) = describe(cpu, at) else {
        return;
    };
    for i in 0..a.count() {
        clear_element(cpu, a.vt, a.data + i * a.element, calls);
    }
    free(cpu, at);
}

/// The place of the element at the indices at `indices` (the first
/// dimension's first), if they are in the bounds.
fn element_at(cpu: &mut Cpu, a: &Array, indices: u32) -> Option<u32> {
    let dims = a.bounds.len() as u32;
    if !cpu.bus.guest_probe(indices, 4 * dims as usize, false) {
        return None;
    }
    let mut offset = 0u64;
    let mut stride = 1u64;
    for d in 0..dims {
        let (n, low) = a.bounds[(dims - 1 - d) as usize];
        let i = cpu.bus.guest_read_32(indices + 4 * d) as i32;
        let k = i.wrapping_sub(low);
        if k < 0 || k as u32 >= n {
            return None;
        }
        offset += k as u64 * stride;
        stride *= n as u64;
    }
    Some(a.data + (offset * a.element as u64) as u32)
}

fn run(cpu: &mut Cpu, result: u32, calls: Vec<(u32, u32)>) -> Option<u32> {
    crate::win32::ole32::call_all(cpu, result, &calls)
}

/// SafeArrayCreate(vt, dims, bounds): the bounds the first dimension's
/// first, which the descriptor keeps the other way round.
pub fn safe_array_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[vt, dims, bounds] = args else {
        unreachable!()
    };
    if dims == 0 || dims > MAX_DIMS || !cpu.bus.guest_probe(bounds, 8 * dims as usize, false) {
        return Some(0);
    }
    let list: Vec<(u32, i32)> = (0..dims)
        .rev()
        .map(|i| {
            let b = bounds + 8 * i;
            (
                cpu.bus.guest_read_32(b),
                cpu.bus.guest_read_32(b + 4) as i32,
            )
        })
        .collect();
    Some(create(cpu, vt as u16, 0, &list).map_or(0, |a| a.at))
}

/// SafeArrayCreateVector(vt, lower bound, count).
pub fn safe_array_create_vector(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[vt, low, n] = args else { unreachable!() };
    let Some(a) = create(cpu, vt as u16, 0, &[(n, low as i32)]) else {
        return Some(0);
    };
    let features = cpu.bus.guest_read_16(a.at + FEATURES);
    cpu.bus
        .guest_write_16(a.at + FEATURES, features | FADF_CREATEVECTOR);
    Some(a.at)
}

/// SafeArrayDestroy(array).
pub fn safe_array_destroy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if at == 0 {
        return Some(S_OK);
    }
    if describe(cpu, at).is_none() {
        return Some(E_INVALIDARG);
    }
    if cpu.bus.guest_read_32(at + LOCKS) != 0 {
        return Some(DISP_E_ARRAYISLOCKED);
    }
    let mut calls = Vec::new();
    destroy(cpu, at, &mut calls);
    run(cpu, S_OK, calls)
}

/// SafeArrayGetDim(array).
pub fn safe_array_get_dim(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(describe(cpu, args[0]).map_or(0, |a| a.bounds.len() as u32))
}

/// SafeArrayGetElemsize(array).
pub fn safe_array_get_elemsize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(describe(cpu, args[0]).map_or(0, |a| a.element))
}

/// SafeArrayGetUBound(array, dimension, out) and SafeArrayGetLBound: the
/// dimensions count from 1.
fn bound(cpu: &mut Cpu, args: &[u32], upper: bool) -> Option<u32> {
    let &[at, dim, out] = args else {
        unreachable!()
    };
    let Some(a) = describe(cpu, at) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let dims = a.bounds.len() as u32;
    if dim == 0 || dim > dims {
        return Some(DISP_E_BADINDEX);
    }
    let (n, low) = a.bounds[(dims - dim) as usize];
    let value = if upper {
        low.wrapping_add(n as i32).wrapping_sub(1)
    } else {
        low
    };
    com::write_out(cpu, out, value as u32);
    Some(S_OK)
}

pub fn safe_array_get_ubound(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    bound(cpu, args, true)
}

pub fn safe_array_get_lbound(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    bound(cpu, args, false)
}

/// SafeArrayLock(array).
pub fn safe_array_lock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if describe(cpu, at).is_none() {
        return Some(E_INVALIDARG);
    }
    let locks = cpu.bus.guest_read_32(at + LOCKS);
    if locks >= 0xFFFF {
        return Some(E_UNEXPECTED);
    }
    cpu.bus.guest_write_32(at + LOCKS, locks + 1);
    Some(S_OK)
}

/// SafeArrayUnlock(array).
pub fn safe_array_unlock(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if describe(cpu, at).is_none() {
        return Some(E_INVALIDARG);
    }
    let locks = cpu.bus.guest_read_32(at + LOCKS);
    if locks == 0 {
        return Some(E_UNEXPECTED);
    }
    cpu.bus.guest_write_32(at + LOCKS, locks - 1);
    Some(S_OK)
}

/// SafeArrayAccessData(array, out): locked, and its data's address.
pub fn safe_array_access_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, out] = args else { unreachable!() };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let result = safe_array_lock(cpu, &[at])?;
    let data = if result == S_OK {
        cpu.bus.guest_read_32(at + DATA)
    } else {
        0
    };
    com::write_out(cpu, out, data);
    Some(result)
}

/// SafeArrayUnaccessData(array).
pub fn safe_array_unaccess_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    safe_array_unlock(cpu, args)
}

/// SafeArrayPtrOfIndex(array, indices, out).
pub fn safe_array_ptr_of_index(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, indices, out] = args else {
        unreachable!()
    };
    let Some(a) = describe(cpu, at) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    match element_at(cpu, &a, indices) {
        Some(p) => {
            com::write_out(cpu, out, p);
            Some(S_OK)
        }
        None => Some(DISP_E_BADINDEX),
    }
}

/// SafeArrayGetElement(array, indices, out): a copy of the element.
pub fn safe_array_get_element(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, indices, out] = args else {
        unreachable!()
    };
    let Some(a) = describe(cpu, at) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 || !cpu.bus.guest_probe(out, a.element as usize, true) {
        return Some(E_INVALIDARG);
    }
    let Some(p) = element_at(cpu, &a, indices) else {
        return Some(DISP_E_BADINDEX);
    };
    let mut calls = Vec::new();
    if !copy_element(cpu, a.vt, out, p, &mut calls) {
        return run(cpu, E_OUTOFMEMORY, calls);
    }
    run(cpu, S_OK, calls)
}

/// SafeArrayPutElement(array, indices, value): the element becomes a
/// copy of the value (for BSTRs and interfaces, of what `value` points
/// to... as Windows takes them: the BSTR or interface pointer itself).
pub fn safe_array_put_element(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, indices, value] = args else {
        unreachable!()
    };
    let Some(a) = describe(cpu, at) else {
        return Some(E_INVALIDARG);
    };
    let Some(p) = element_at(cpu, &a, indices) else {
        return Some(DISP_E_BADINDEX);
    };
    let mut calls = Vec::new();
    // AddRef the new before the old is released.
    let mut releases = Vec::new();
    let ok = match a.vt {
        // A BSTR or an interface pointer is passed as itself.
        VT_BSTR | VT_DISPATCH | VT_UNKNOWN => {
            let scratch = crate::win32::ole32::task_alloc(cpu, 4);
            if scratch == 0 {
                return Some(E_OUTOFMEMORY);
            }
            cpu.bus.guest_write_32(scratch, value);
            clear_element(cpu, a.vt, p, &mut releases);
            let ok = copy_element(cpu, a.vt, p, scratch, &mut calls);
            crate::win32::ole32::task_free(cpu, scratch);
            ok
        }
        _ => {
            if value == 0 || !cpu.bus.guest_probe(value, a.element as usize, false) {
                return Some(E_INVALIDARG);
            }
            clear_element(cpu, a.vt, p, &mut releases);
            copy_element(cpu, a.vt, p, value, &mut calls)
        }
    };
    calls.extend(releases);
    run(cpu, if ok { S_OK } else { E_OUTOFMEMORY }, calls)
}

/// SafeArrayCopy(array, out): a copy with copies of its elements.
pub fn safe_array_copy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, out] = args else { unreachable!() };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    com::write_out(cpu, out, 0);
    if at == 0 {
        return Some(S_OK);
    }
    let mut calls = Vec::new();
    let copy = copy(cpu, at, &mut calls);
    if copy == 0 {
        return run(cpu, E_OUTOFMEMORY, calls);
    }
    com::write_out(cpu, out, copy);
    run(cpu, S_OK, calls)
}

/// SafeArrayGetVartype(array, out).
pub fn safe_array_get_vartype(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, out] = args else { unreachable!() };
    let Some(a) = describe(cpu, at) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 || !cpu.bus.guest_probe(out, 2, true) {
        return Some(E_INVALIDARG);
    }
    cpu.bus.guest_write_16(out, a.vt);
    Some(S_OK)
}

/// SafeArrayAllocDescriptor(dims, out): a descriptor with no type and no
/// data, which the program fills in.
pub fn safe_array_alloc_descriptor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[dims, out] = args else { unreachable!() };
    if dims == 0 || dims > MAX_DIMS || out == 0 {
        return Some(E_INVALIDARG);
    }
    let at = new_descriptor(cpu, 0, dims);
    if at == 0 {
        return Some(E_OUTOFMEMORY);
    }
    // No type: the program sets the features and element size.
    cpu.bus.guest_write_16(at + FEATURES, 0);
    cpu.bus.guest_write_32(at - 4, 0);
    com::write_out(cpu, out, at);
    Some(S_OK)
}

/// SafeArrayAllocData(array).
pub fn safe_array_alloc_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if alloc_data(cpu, args[0]) {
        Some(S_OK)
    } else {
        Some(E_INVALIDARG)
    }
}

/// SafeArrayDestroyData(array): the elements let go of and their block
/// freed.
pub fn safe_array_destroy_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    let Some(a) = describe(cpu, at) else {
        return Some(E_INVALIDARG);
    };
    if cpu.bus.guest_read_32(at + LOCKS) != 0 {
        return Some(DISP_E_ARRAYISLOCKED);
    }
    let mut calls = Vec::new();
    for i in 0..a.count() {
        clear_element(cpu, a.vt, a.data + i * a.element, &mut calls);
    }
    crate::win32::ole32::task_free(cpu, a.data);
    cpu.bus.guest_write_32(at + DATA, 0);
    run(cpu, S_OK, calls)
}

/// SafeArrayDestroyDescriptor(array).
pub fn safe_array_destroy_descriptor(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if describe(cpu, at).is_none() {
        return Some(E_INVALIDARG);
    }
    if cpu.bus.guest_read_32(at + LOCKS) != 0 {
        return Some(DISP_E_ARRAYISLOCKED);
    }
    free_descriptor(cpu, at);
    Some(S_OK)
}

/// SafeArrayRedim(array, bound): the last dimension of the program's
/// list (the descriptor's first) gets the new bound; elements that no
/// longer fit are let go of, new ones are zeroed.
pub fn safe_array_redim(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, bound] = args else { unreachable!() };
    let Some(a) = describe(cpu, at) else {
        return Some(E_INVALIDARG);
    };
    if !cpu.bus.guest_probe(bound, 8, false) {
        return Some(E_INVALIDARG);
    }
    if cpu.bus.guest_read_32(at + LOCKS) != 0 {
        return Some(DISP_E_ARRAYISLOCKED);
    }
    let n = cpu.bus.guest_read_32(bound);
    let low = cpu.bus.guest_read_32(bound + 4);
    let old = a.count();
    // The elements of each step of the dimension that changes: the
    // other dimensions' counts, which an empty array has too.
    let inner = a.bounds[1..]
        .iter()
        .fold(1u64, |n, &(c, _)| n.saturating_mul(c as u64))
        .min(u32::MAX as u64) as u32;
    let new = (inner as u64 * n as u64).min(u32::MAX as u64) as u32;
    if new as u64 * a.element as u64 > MAX_BYTES {
        return Some(E_OUTOFMEMORY);
    }
    let mut calls = Vec::new();
    for i in new.min(old)..old {
        clear_element(cpu, a.vt, a.data + i * a.element, &mut calls);
    }
    let data = crate::win32::ole32::task_alloc(cpu, (new * a.element).max(1));
    if data == 0 {
        return run(cpu, E_OUTOFMEMORY, calls);
    }
    cpu.bus.guest_fill(data, (new * a.element) as usize, 0);
    let keep = (new.min(old) * a.element) as usize;
    let mut b = vec![0; keep];
    cpu.bus.guest_read_bytes(a.data, &mut b);
    cpu.bus.guest_write_bytes(data, &b);
    crate::win32::ole32::task_free(cpu, a.data);
    cpu.bus.guest_write_32(at + DATA, data);
    cpu.bus.guest_write_32(at + BOUNDS, n);
    cpu.bus.guest_write_32(at + BOUNDS + 4, low);
    run(cpu, S_OK, calls)
}

/// VariantCopyInd(dest, src): a copy of the value a VT_BYREF VARIANT
/// points to, or VariantCopy's of any other.
pub fn variant_copy_ind(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[dest, src] = args else { unreachable!() };
    if !cpu.bus.guest_probe(dest, 16, true) || !cpu.bus.guest_probe(src, 16, false) {
        return Some(E_INVALIDARG);
    }
    let vt = cpu.bus.guest_read_16(src);
    if vt & VT_BYREF == 0 {
        return super::variant::variant_copy(cpu, args);
    }
    let base = vt & !VT_BYREF;
    let p = cpu.bus.guest_read_32(src + 8);
    // The value it points to, as a VARIANT of its own.
    let mut value = [0u8; 16];
    if base == VT_VARIANT {
        if !cpu.bus.guest_probe(p, 16, false) {
            return Some(E_INVALIDARG);
        }
        cpu.bus.guest_read_bytes(p, &mut value);
    } else {
        let size = if base & VT_ARRAY != 0 {
            4
        } else {
            match element_size(base) {
                Some(n) => n as usize,
                None => return Some(super::variant::DISP_E_BADVARTYPE),
            }
        };
        if !cpu.bus.guest_probe(p, size, false) {
            return Some(E_INVALIDARG);
        }
        cpu.bus.guest_read_bytes(p, &mut value[8..8 + size]);
        value[..2].copy_from_slice(&base.to_le_bytes());
    }
    let scratch = crate::win32::ole32::task_alloc(cpu, 16);
    if scratch == 0 {
        return Some(E_OUTOFMEMORY);
    }
    cpu.bus.guest_write_bytes(scratch, &value);
    let mut releases = Vec::new();
    let mut calls = Vec::new();
    if dest != src {
        clear_variant(cpu, dest, &mut releases);
    }
    let ok = copy_variant(cpu, dest, scratch, &mut calls);
    crate::win32::ole32::task_free(cpu, scratch);
    calls.extend(releases);
    run(cpu, if ok { S_OK } else { E_OUTOFMEMORY }, calls)
}
