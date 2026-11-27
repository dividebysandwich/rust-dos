//! Device monikers (`@device:sw:{category}\{instance}`) and their
//! property bags, as the filter mapper hands them out: RegisterFilter
//! with `ppMoniker` gives the moniker of the filter's registration, and
//! a codec's DllRegisterServer binds it to its IPropertyBag to add values
//! of its own to the filter's key. The Indeo 5 codec registers its
//! decoder in the video compressors' category this way (#2182).
//!
//! A moniker names a key under `HKCR\CLSID\{category}\Instance`, as
//! DirectShow's device enumerator (DEVENUM.DLL) keeps the software
//! devices. Its property bag reads and writes that key's values: a BSTR
//! as a string, a 4-byte integer as a DWORD, and an array of bytes as
//! binary data, as Wine's devenum/mediacatenum.c does. Binding to the
//! filter itself (BindToObject), composition, parsing and persistence
//! give E_NOTIMPL: no program seen asks for them.

use super::{E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, E_POINTER, Kind, S_OK};
use crate::cpu::Cpu;
use crate::win32::com::{self, Guid};
use crate::win32::registry::{self, HKEY_CLASSES_ROOT, Open, REG_BINARY, REG_DWORD, REG_SZ};

const S_FALSE: u32 = 1;
const E_FAIL: u32 = super::E_FAIL;

/// IID_IPropertyBag.
pub const IID_IPROPERTY_BAG: Guid = Guid::new(
    0x5527_2A00,
    0x42CB,
    0x11CE,
    [0x81, 0x35, 0x00, 0xAA, 0x00, 0x4B, 0xB8, 0x51],
);

/// CLSID_CDeviceMoniker, the device moniker's class.
const CLSID_CDEVICE_MONIKER: Guid = Guid::new(
    0x4315_D437,
    0x5B8C,
    0x11D0,
    [0xBD, 0x3B, 0x00, 0xA0, 0xC9, 0x11, 0xCE, 0x86],
);

/// VARIANT types the property bag keeps.
const VT_I4: u16 = 3;
const VT_UI1: u16 = 17;
const VT_BSTR: u16 = 8;
const VT_UI4: u16 = 19;
const VT_INT: u16 = 22;
const VT_UINT: u16 = 23;
const VT_UI1_ARRAY: u16 = 0x2011;

/// A new device moniker of the instance `instance` of `category`, with
/// one reference: its IMoniker, or 0 when the heap is full.
pub fn new_moniker(cpu: &mut Cpu, category: &Guid, instance: &str) -> u32 {
    let id = super::add(
        cpu,
        Kind::Device {
            category: category.to_string(),
            instance: instance.to_string(),
        },
    );
    let at = super::pointer(cpu, id);
    super::drop_ref(cpu, id);
    at
}

/// The category and instance of the moniker or bag `this` is, if it is
/// one of `bag`'s kind.
fn names(cpu: &mut Cpu, this: u32, bag: bool) -> Option<(String, String)> {
    let id = super::object_of(cpu, this)?;
    match super::kind(cpu, id)? {
        Kind::Device { category, instance } if !bag => Some((category.clone(), instance.clone())),
        Kind::Bag { category, instance } if bag => Some((category.clone(), instance.clone())),
        _ => None,
    }
}

/// The path under HKEY_CLASSES_ROOT of the instance `instance` of the
/// category `category`: the key a moniker names, and the one the filter
/// mapper writes a filter or a category to.
pub(super) fn key_path(category: &str, instance: &str) -> String {
    format!("CLSID\\{}\\Instance\\{}", category, instance)
}

/// The moniker's display name, as Windows' device moniker gives it.
fn display_name(category: &str, instance: &str) -> String {
    format!("@device:sw:{}\\{}", category, instance)
}

/// The NUL-terminated UTF-16 string at `at` as UTF-16 units, or None.
fn units_at(cpu: &mut Cpu, at: u32) -> Option<Vec<u16>> {
    if at == 0 {
        return None;
    }
    let mut units = Vec::new();
    for i in 0..0x8000u32 {
        let p = at.wrapping_add(2 * i);
        if !cpu.bus.guest_probe(p, 2, false) {
            return None;
        }
        match cpu.bus.guest_read_16(p) {
            0 => return Some(units),
            c => units.push(c),
        }
    }
    None
}

/// The UTF-16 string at `at` in the ANSI code page, as the registry
/// keeps names and strings (a char for each byte), or None if it can't
/// be read there.
pub(super) fn ansi_at(cpu: &mut Cpu, at: u32) -> Option<String> {
    let units = units_at(cpu, at)?;
    let bytes = crate::win32::kernel32::wide_to_ansi(cpu, &units);
    Some(crate::dosstr::from_bytes(&bytes))
}

// IMoniker.

/// IPersist::GetClassID(this, pClassID): CLSID_CDeviceMoniker.
pub fn get_class_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if names(cpu, args[0], false).is_none() || args[1] == 0 {
        return Some(E_POINTER);
    }
    CLSID_CDEVICE_MONIKER.write(cpu, args[1]);
    Some(S_OK)
}

/// IPersistStream::IsDirty(this): S_FALSE, nothing to save.
pub fn is_dirty(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_FALSE)
}

/// A method that isn't there: E_NOTIMPL.
pub fn not_implemented(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(E_NOTIMPL)
}

/// IMoniker::BindToObject(this, pbc, pmkToLeft, riid, ppvResult):
/// E_NOTIMPL, with the out-pointer cleared.
pub fn bind_to_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    com::write_out(cpu, args[4], 0);
    Some(E_NOTIMPL)
}

/// IMoniker::BindToStorage(this, pbc, pmkToLeft, riid, ppvObj): the
/// property bag of the moniker's key for IID_IPropertyBag.
pub fn bind_to_storage(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((category, instance)) = names(cpu, args[0], false) else {
        return Some(E_POINTER);
    };
    let Some(iid) = Guid::read(cpu, args[3]) else {
        return Some(E_POINTER);
    };
    // An out-pointer that can't take the bag fails before one is made,
    // or the bag would be left with a reference nobody holds.
    if !com::write_out(cpu, args[4], 0) {
        return Some(E_POINTER);
    }
    if iid != IID_IPROPERTY_BAG {
        return Some(E_NOINTERFACE);
    }
    let id = super::add(cpu, Kind::Bag { category, instance });
    Some(super::give_new(cpu, id, &iid, args[4]))
}

/// IMoniker::IsEqual(this, pmkOtherMoniker): S_OK for a device moniker
/// of the same key.
pub fn is_equal(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(mine) = names(cpu, args[0], false) else {
        return Some(E_POINTER);
    };
    let other = names(cpu, args[1], false);
    let same = other
        .is_some_and(|o| o.0.eq_ignore_ascii_case(&mine.0) && o.1.eq_ignore_ascii_case(&mine.1));
    Some(if same { S_OK } else { S_FALSE })
}

/// IMoniker::Hash(this, pdwHash): a hash of the display name in
/// capitals.
pub fn hash(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((category, instance)) = names(cpu, args[0], false) else {
        return Some(E_POINTER);
    };
    let name = display_name(&category, &instance).to_ascii_uppercase();
    let h = name
        .bytes()
        .fold(0u32, |h, b| h.rotate_left(3) ^ u32::from(b));
    Some(super::put32(cpu, args[1], h).map_or_else(|e| e, |()| S_OK))
}

/// IMoniker::GetDisplayName(this, pbc, pmkToLeft, ppszDisplayName):
/// `@device:sw:{category}\{instance}`, in memory of the task allocator.
pub fn get_display_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((category, instance)) = names(cpu, args[0], false) else {
        return Some(E_POINTER);
    };
    if args[3] == 0 {
        return Some(E_POINTER);
    }
    let name = display_name(&category, &instance);
    let units: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let at = crate::win32::ole32::task_alloc(cpu, 2 * units.len() as u32);
    if at == 0 {
        com::write_out(cpu, args[3], 0);
        return Some(super::E_OUTOFMEMORY);
    }
    for (i, u) in units.iter().enumerate() {
        cpu.bus.guest_write_16(at + 2 * i as u32, *u);
    }
    com::write_out(cpu, args[3], at);
    Some(S_OK)
}

/// IMoniker::IsSystemMoniker(this, pdwMksys): MKSYS_NONE, S_FALSE.
pub fn is_system_moniker(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(super::put32(cpu, args[1], 0).map_or_else(|e| e, |()| S_FALSE))
}

// IPropertyBag.

/// IPropertyBag::Read(this, pszPropName, pVar, pErrorLog): the value
/// of the key as a VARIANT: a string as a BSTR, a DWORD as VT_I4, and
/// binary data as a SAFEARRAY of bytes (VT_ARRAY | VT_UI1), the types
/// Write keeps. E_FAIL when the key or value isn't there.
pub fn bag_read(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((category, instance)) = names(cpu, args[0], true) else {
        return Some(E_POINTER);
    };
    let Some(name) = ansi_at(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    if args[2] == 0 || !cpu.bus.guest_probe(args[2], 16, true) {
        return Some(E_POINTER);
    }
    let registry = registry::get(cpu);
    let Open::Key(root) = registry.open(HKEY_CLASSES_ROOT, 0) else {
        return Some(E_FAIL);
    };
    let Some(key) = registry.find(root, &key_path(&category, &instance)) else {
        return Some(E_FAIL);
    };
    let Some(value) = registry.value(key, &name).cloned() else {
        return Some(E_FAIL);
    };
    let var = args[2];
    match value.kind {
        REG_SZ => {
            let units = crate::win32::kernel32::ansi_to_wide(
                cpu,
                value.data.split(|&b| b == 0).next().unwrap_or(&[]),
            );
            let text = String::from_utf16_lossy(&units);
            let bstr = crate::win32::oleaut32::new_bstr(cpu, &text);
            if bstr == 0 {
                return Some(super::E_OUTOFMEMORY);
            }
            cpu.bus.guest_write_16(var, VT_BSTR);
            cpu.bus.guest_write_32(var + 8, bstr);
        }
        REG_DWORD if value.data.len() >= 4 => {
            let d =
                u32::from_le_bytes([value.data[0], value.data[1], value.data[2], value.data[3]]);
            cpu.bus.guest_write_16(var, VT_I4);
            cpu.bus.guest_write_32(var + 8, d);
        }
        REG_BINARY => {
            let len = value.data.len() as u32;
            let bounds = [(len, 0)];
            let create = crate::win32::oleaut32::safearray::create;
            let Some(array) = create(cpu, VT_UI1, 0, &bounds) else {
                return Some(super::E_OUTOFMEMORY);
            };
            cpu.bus.guest_write_bytes(array.data, &value.data);
            cpu.bus.guest_write_16(var, VT_UI1_ARRAY);
            cpu.bus.guest_write_32(var + 8, array.at);
        }
        _ => return Some(E_FAIL),
    }
    Some(S_OK)
}

/// IPropertyBag::Write(this, pszPropName, pVar): the VARIANT as the
/// key's value of that name, the key made if it isn't there: a BSTR as
/// a string, an integer of 4 bytes as a DWORD, an array of bytes as
/// binary data. E_INVALIDARG for another type.
pub fn bag_write(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((category, instance)) = names(cpu, args[0], true) else {
        return Some(E_POINTER);
    };
    let Some(name) = ansi_at(cpu, args[1]) else {
        return Some(E_POINTER);
    };
    let var = args[2];
    if var == 0 || !cpu.bus.guest_probe(var, 16, false) {
        return Some(E_POINTER);
    }
    let (kind, data) = match cpu.bus.guest_read_16(var) {
        VT_BSTR => {
            let bstr = cpu.bus.guest_read_32(var + 8);
            let units = units_at(cpu, bstr).unwrap_or_default();
            let mut bytes = crate::win32::kernel32::wide_to_ansi(cpu, &units);
            bytes.push(0);
            (REG_SZ, bytes)
        }
        VT_I4 | VT_UI4 | VT_INT | VT_UINT => (
            REG_DWORD,
            cpu.bus.guest_read_32(var + 8).to_le_bytes().to_vec(),
        ),
        VT_UI1_ARRAY => {
            // SAFEARRAY: cDims, fFeatures, cbElements, cLocks, pvData,
            // then rgsabound[0]'s cElements.
            let array = cpu.bus.guest_read_32(var + 8);
            if array == 0 || !cpu.bus.guest_probe(array, 24, false) {
                return Some(E_INVALIDARG);
            }
            let data = cpu.bus.guest_read_32(array + 12);
            let len = cpu.bus.guest_read_32(array + 16);
            if len > 0x10_0000 || (len > 0 && !cpu.bus.guest_probe(data, len as usize, false)) {
                return Some(E_INVALIDARG);
            }
            let mut bytes = vec![0; len as usize];
            cpu.bus.guest_read_bytes(data, &mut bytes);
            (REG_BINARY, bytes)
        }
        _ => return Some(E_INVALIDARG),
    };
    let registry = registry::get(cpu);
    let Open::Key(root) = registry.open(HKEY_CLASSES_ROOT, 0) else {
        return Some(E_FAIL);
    };
    let (key, _) = registry.create(root, &key_path(&category, &instance));
    registry.set_value(key, &name, kind, data);
    Some(S_OK)
}
