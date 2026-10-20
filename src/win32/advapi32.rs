//! ADVAPI32.DLL: of what Windows 95 has, the registry's functions
//! (registry.rs has the registry itself), GetUserNameA, and the security
//! and service functions, which it has only as stubs that fail.
//!
//! The registry's functions return an error code, which they don't make
//! the last error. Their W forms are stubs that return 0 and make the
//! last error ERROR_CALL_NOT_IMPLEMENTED, as Windows 95's do.

use super::registry::{self, HKEY_CLASSES_ROOT, HKEY_DYN_DATA, HKEY_LOCAL_MACHINE, Open, REG_SZ};
use super::stubs::{Api, Conv, Dll};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "ADVAPI32.DLL",
    base: 0xBFEC_0000,
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
    stdcall("InitializeSecurityDescriptor", 1, 2, not_implemented),
    stdcall("SetSecurityDescriptorDacl", 2, 4, not_implemented),
    stdcall("InitializeAcl", 3, 3, not_implemented),
    stdcall("RegOpenKeyA", 4, 3, reg_open_key),
    stdcall("RegOpenKeyExA", 5, 5, reg_open_key_ex),
    stdcall("RegCreateKeyA", 6, 3, reg_create_key),
    stdcall("RegCreateKeyExA", 7, 9, reg_create_key_ex),
    stdcall("RegCloseKey", 8, 1, reg_close_key),
    stdcall("RegQueryValueA", 9, 4, reg_query_value),
    stdcall("RegQueryValueExA", 10, 6, reg_query_value_ex),
    stdcall("RegSetValueA", 11, 5, reg_set_value),
    stdcall("RegSetValueExA", 12, 6, reg_set_value_ex),
    stdcall("RegDeleteKeyA", 13, 2, reg_delete_key),
    stdcall("RegDeleteValueA", 14, 2, reg_delete_value),
    stdcall("RegEnumKeyA", 15, 4, reg_enum_key),
    stdcall("RegEnumKeyExA", 16, 8, reg_enum_key_ex),
    stdcall("RegEnumValueA", 17, 8, reg_enum_value),
    stdcall("RegQueryInfoKeyA", 18, 12, reg_query_info_key),
    stdcall("RegFlushKey", 19, 1, reg_flush_key),
    stdcall("RegConnectRegistryA", 20, 3, reg_connect_registry),
    stdcall("GetUserNameA", 21, 2, get_user_name),
    stdcall("RegOpenKeyW", 22, 3, not_implemented),
    stdcall("RegOpenKeyExW", 23, 5, not_implemented),
    stdcall("RegCreateKeyW", 24, 3, not_implemented),
    stdcall("RegCreateKeyExW", 25, 9, not_implemented),
    stdcall("RegQueryValueW", 26, 4, not_implemented),
    stdcall("RegQueryValueExW", 27, 6, not_implemented),
    stdcall("RegSetValueW", 28, 5, not_implemented),
    stdcall("RegSetValueExW", 29, 6, not_implemented),
    stdcall("RegDeleteKeyW", 30, 2, not_implemented),
    stdcall("RegDeleteValueW", 31, 2, not_implemented),
    stdcall("RegEnumKeyW", 32, 4, not_implemented),
    stdcall("RegEnumKeyExW", 33, 8, not_implemented),
    stdcall("RegEnumValueW", 34, 8, not_implemented),
    stdcall("RegQueryInfoKeyW", 35, 12, not_implemented),
    stdcall("RegConnectRegistryW", 36, 3, not_implemented),
    stdcall("GetUserNameW", 37, 2, not_implemented),
    stdcall("OpenProcessToken", 38, 3, not_implemented),
    stdcall("LookupPrivilegeValueA", 39, 3, not_implemented),
    stdcall("AdjustTokenPrivileges", 40, 6, not_implemented),
    stdcall("OpenSCManagerA", 41, 3, not_implemented),
];

const FALSE: u32 = 0;
const TRUE: u32 = 1;

/// Error codes.
const ERROR_SUCCESS: u32 = 0;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_BAD_NETPATH: u32 = 53;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_CALL_NOT_IMPLEMENTED: u32 = 120;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_NO_MORE_ITEMS: u32 = 259;
const ERROR_BADKEY: u32 = 1010;

/// RegCreateKeyEx's dispositions.
const REG_CREATED_NEW_KEY: u32 = 1;
const REG_OPENED_EXISTING_KEY: u32 = 2;

/// The most data a value takes.
const MAX_DATA: u32 = 0x10_0000;

/// A function Windows 95 has only as a stub that fails.
fn not_implemented(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    super::process(cpu).last_error = ERROR_CALL_NOT_IMPLEMENTED;
    Some(FALSE)
}

/// The string at `at`, one char per byte, or "" for NULL. None if a page
/// isn't there.
fn string_at(cpu: &mut Cpu, at: u32) -> Option<String> {
    if at == 0 {
        return Some(String::new());
    }
    let bytes = cpu.bus.guest_read_asciiz(at, 0x1_0000);
    (!cpu.bus.guest_faulted()).then(|| crate::dosstr::from_bytes(&bytes))
}

/// The 32-bit value at `at`, or 0 for NULL. None if a page isn't there.
fn read_32(cpu: &mut Cpu, at: u32) -> Option<u32> {
    if at == 0 {
        return Some(0);
    }
    let value = cpu.bus.guest_read_32(at);
    (!cpu.bus.guest_faulted()).then_some(value)
}

/// Write `value` at `at` unless it is NULL.
fn write_32(cpu: &mut Cpu, at: u32, value: u32) {
    if at != 0 {
        cpu.bus.guest_write_32(at, value);
    }
}

/// Whether the 32-bit values at the addresses `at` that aren't NULL can
/// be written: false after a page fault.
fn writable(cpu: &mut Cpu, at: &[u32]) -> bool {
    at.iter()
        .all(|&a| a == 0 || cpu.bus.guest_probe(a, 4, true))
}

/// The running process's id, which its keys' handles carry.
fn pid(cpu: &mut Cpu) -> u32 {
    super::process(cpu).id
}

/// The key the handle `hkey` names, or the error for it.
fn open(cpu: &mut Cpu, hkey: u32) -> Result<u32, u32> {
    let id = pid(cpu);
    match registry::get(cpu).open(hkey, id) {
        Open::Key(key) => Ok(key),
        Open::Error(code) => Err(code),
    }
}

/// The subkey at `path` of `key`. A path that starts with a backslash
/// names none, as on Windows 95.
fn find(cpu: &mut Cpu, key: u32, path: &str) -> Option<u32> {
    if path.starts_with('\\') {
        return None;
    }
    registry::get(cpu).find(key, path)
}

/// A new handle of `key` for the running process, written at `phk`.
fn give_handle(cpu: &mut Cpu, key: u32, phk: u32) {
    let id = pid(cpu);
    let handle = registry::get(cpu).new_handle(key, id);
    cpu.bus.guest_write_32(phk, handle);
}

/// Unwrap a key or return its error from the function.
macro_rules! key {
    ($cpu:expr, $hkey:expr) => {
        match open($cpu, $hkey) {
            Ok(key) => key,
            Err(code) => return Some(code),
        }
    };
}

/// RegOpenKeyA: as RegOpenKeyExA, but with no subkey the handle is the
/// one given.
fn reg_open_key(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hkey, sub, phk] = args else {
        unreachable!()
    };
    if phk == 0 {
        return Some(ERROR_INVALID_PARAMETER);
    }
    let path = string_at(cpu, sub)?;
    let key = key!(cpu, hkey);
    if path.is_empty() {
        cpu.bus.guest_write_32(phk, hkey);
        return Some(ERROR_SUCCESS);
    }
    open_subkey(cpu, key, &path, phk)
}

/// RegOpenKeyExA: a new handle of the subkey `args[1]` of the key
/// `args[0]`, written at `args[4]`.
fn reg_open_key_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hkey, sub, _, _, phk] = args else {
        unreachable!()
    };
    if phk == 0 {
        return Some(ERROR_INVALID_PARAMETER);
    }
    let path = string_at(cpu, sub)?;
    let key = key!(cpu, hkey);
    open_subkey(cpu, key, &path, phk)
}

fn open_subkey(cpu: &mut Cpu, key: u32, path: &str, phk: u32) -> Option<u32> {
    if !writable(cpu, &[phk]) {
        return None;
    }
    match find(cpu, key, path) {
        Some(found) => {
            give_handle(cpu, found, phk);
            Some(ERROR_SUCCESS)
        }
        None => {
            cpu.bus.guest_write_32(phk, 0);
            Some(ERROR_FILE_NOT_FOUND)
        }
    }
}

/// RegCreateKeyA: RegCreateKeyExA's work.
fn reg_create_key(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create_key(cpu, args[0], args[1], args[2], 0)
}

/// RegCreateKeyExA: the subkey `args[1]` of the key `args[0]`, made if
/// it isn't there, and a new handle of it at `args[7]`. Windows 95 has
/// no classes, security or volatile keys: those arguments are ignored.
fn reg_create_key_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create_key(cpu, args[0], args[1], args[7], args[8])
}

/// The subkey `sub` of `hkey`, made if it isn't there: its new handle
/// at `phk`, and whether it was made at `disposition`.
fn create_key(cpu: &mut Cpu, hkey: u32, sub: u32, phk: u32, disposition: u32) -> Option<u32> {
    if phk == 0 {
        return Some(ERROR_INVALID_PARAMETER);
    }
    let path = string_at(cpu, sub)?;
    let key = key!(cpu, hkey);
    if !writable(cpu, &[phk, disposition]) {
        return None;
    }
    let (key, created) = registry::get(cpu).create(key, &path);
    give_handle(cpu, key, phk);
    let made = if created {
        REG_CREATED_NEW_KEY
    } else {
        REG_OPENED_EXISTING_KEY
    };
    write_32(cpu, disposition, made);
    Some(ERROR_SUCCESS)
}

/// RegCloseKey. The predefined keys don't close. The registry is saved
/// if it changed.
fn reg_close_key(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hkey = args[0];
    if hkey == 0 {
        return Some(ERROR_BADKEY);
    }
    if (HKEY_CLASSES_ROOT..=HKEY_DYN_DATA).contains(&hkey) {
        return Some(ERROR_SUCCESS);
    }
    let id = pid(cpu);
    if !registry::get(cpu).close(hkey, id) {
        return Some(ERROR_INVALID_HANDLE);
    }
    registry::save(cpu);
    Some(ERROR_SUCCESS)
}

/// The data of a value, at `data` (unless it is NULL) if it fits in the
/// size at `size`; its size at `size` and type at `kind` (unless they are
/// NULL). ERROR_MORE_DATA, with the size it needs, if it doesn't fit.
fn give_data(cpu: &mut Cpu, value: &[u8], kind: u32, at: (u32, u32, u32)) -> Option<u32> {
    let (ptype, data, size) = at;
    let room = read_32(cpu, size)?;
    let len = value.len() as u32;
    if !writable(cpu, &[ptype, size]) {
        return None;
    }
    if data != 0 && room < len {
        write_32(cpu, ptype, kind);
        write_32(cpu, size, len);
        return Some(ERROR_MORE_DATA);
    }
    if data != 0 {
        if !cpu.bus.guest_probe(data, value.len(), true) {
            return None;
        }
        cpu.bus.guest_write_bytes(data, value);
    }
    write_32(cpu, ptype, kind);
    write_32(cpu, size, len);
    Some(ERROR_SUCCESS)
}

/// RegQueryValueA: the default value of the subkey `args[1]` of the key
/// `args[0]` (or of that key, with no subkey), as RegQueryValueExA gives
/// it, an empty string when it has none.
fn reg_query_value(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hkey, sub, data, size] = args else {
        unreachable!()
    };
    let path = string_at(cpu, sub)?;
    let key = key!(cpu, hkey);
    let Some(key) = find(cpu, key, &path) else {
        return Some(ERROR_FILE_NOT_FOUND);
    };
    if data != 0 && size == 0 {
        return Some(ERROR_INVALID_PARAMETER);
    }
    let value = registry::get(cpu).value(key, "").map(|v| v.data.clone());
    let value = value.unwrap_or_else(|| vec![0]);
    give_data(cpu, &value, REG_SZ, (0, data, size))
}

/// RegQueryValueExA: the value `args[1]` of the key `args[0]` (its
/// default value with NULL or ""): its type at `args[3]`, its data at
/// `args[4]` and their size at `args[5]`, which says how much room there
/// is. Without the value, Windows 95 sets the type to 0 and leaves the
/// size as it was.
fn reg_query_value_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hkey, name, _, ptype, data, size] = args else {
        unreachable!()
    };
    let name = string_at(cpu, name)?;
    let key = key!(cpu, hkey);
    if data != 0 && size == 0 {
        return Some(ERROR_INVALID_PARAMETER);
    }
    let value = registry::get(cpu).value(key, &name).cloned();
    let Some(value) = value else {
        write_32(cpu, ptype, 0);
        return Some(ERROR_FILE_NOT_FOUND);
    };
    give_data(cpu, &value.data, value.kind, (ptype, data, size))
}

/// RegSetValueA: set the default value of the subkey `args[1]` of the
/// key `args[0]`, made if it isn't there, to the string at `args[3]`.
/// Windows 95 takes any type and ignores the size: the string is stored
/// up to its zero.
fn reg_set_value(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hkey, sub, _, data, _] = args else {
        unreachable!()
    };
    if data == 0 {
        return Some(ERROR_INVALID_PARAMETER);
    }
    let path = string_at(cpu, sub)?;
    let mut text = cpu.bus.guest_read_asciiz(data, MAX_DATA as usize);
    if cpu.bus.guest_faulted() {
        return None;
    }
    text.push(0);
    let key = key!(cpu, hkey);
    let registry = registry::get(cpu);
    let (key, _) = registry.create(key, &path);
    registry.set_value(key, "", REG_SZ, text);
    Some(ERROR_SUCCESS)
}

/// RegSetValueExA: set the value `args[1]` of the key `args[0]` to the
/// `args[5]` bytes at `args[4]`, of the type `args[3]`. A REG_SZ string is
/// stored up to its zero, whatever the size says, as Windows 95 does.
fn reg_set_value_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hkey, name, _, kind, data, len] = args else {
        unreachable!()
    };
    let name = string_at(cpu, name)?;
    let key = key!(cpu, hkey);
    let bytes = if kind == REG_SZ {
        if data == 0 {
            return Some(ERROR_INVALID_PARAMETER);
        }
        let mut text = cpu.bus.guest_read_asciiz(data, MAX_DATA as usize);
        text.push(0);
        text
    } else if data == 0 {
        if len != 0 {
            return Some(ERROR_INVALID_PARAMETER);
        }
        Vec::new()
    } else {
        if len > MAX_DATA {
            return Some(ERROR_INVALID_PARAMETER);
        }
        let mut bytes = vec![0; len as usize];
        cpu.bus.guest_read_bytes(data, &mut bytes);
        bytes
    };
    if cpu.bus.guest_faulted() {
        return None;
    }
    registry::get(cpu).set_value(key, &name, kind, bytes);
    Some(ERROR_SUCCESS)
}

/// RegDeleteKeyA: delete the subkey `args[1]` of the key `args[0]` with
/// the keys under it, as Windows 95 does; with "", the key itself.
fn reg_delete_key(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hkey, sub] = args else { unreachable!() };
    if sub == 0 {
        return Some(ERROR_BADKEY);
    }
    let path = string_at(cpu, sub)?;
    let key = key!(cpu, hkey);
    let Some(key) = find(cpu, key, &path) else {
        return Some(ERROR_FILE_NOT_FOUND);
    };
    let predefined = (HKEY_CLASSES_ROOT..=HKEY_DYN_DATA).contains(&hkey) && path.is_empty();
    let registry = registry::get(cpu);
    if predefined || registry.is_root(key) {
        return Some(ERROR_ACCESS_DENIED);
    }
    registry.delete(key);
    Some(ERROR_SUCCESS)
}

/// RegDeleteValueA: delete the value `args[1]` of the key `args[0]`.
fn reg_delete_value(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = string_at(cpu, args[1])?;
    let key = key!(cpu, args[0]);
    if registry::get(cpu).delete_value(key, &name) {
        Some(ERROR_SUCCESS)
    } else {
        Some(ERROR_FILE_NOT_FOUND)
    }
}

/// The name of the subkey `index` of `key`, as bytes.
fn subkey_name(cpu: &mut Cpu, key: u32, index: u32) -> Option<Vec<u8>> {
    let registry = registry::get(cpu);
    let child = *registry.key(key).children.get(index as usize)?;
    Some(crate::dosstr::to_bytes(&registry.key(child).name))
}

/// Write `bytes` and a zero at `at`.
fn write_string(cpu: &mut Cpu, at: u32, bytes: &[u8]) {
    let mut bytes = bytes.to_vec();
    bytes.push(0);
    cpu.bus.guest_write_bytes(at, &bytes);
}

/// RegEnumKeyA: the name of the subkey `args[1]` of the key `args[0]`
/// into the buffer at `args[2]` of `args[3]` bytes.
fn reg_enum_key(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hkey, index, buffer, size] = args else {
        unreachable!()
    };
    let key = key!(cpu, hkey);
    let Some(name) = subkey_name(cpu, key, index) else {
        return Some(ERROR_NO_MORE_ITEMS);
    };
    if name.len() as u32 >= size {
        return Some(ERROR_MORE_DATA);
    }
    write_string(cpu, buffer, &name);
    Some(ERROR_SUCCESS)
}

/// RegEnumKeyExA: as RegEnumKeyA, the buffer's size at `args[3]`, where
/// the name's length goes. Keys have no class (an empty one at `args[5]`)
/// and no time they were written (0 at `args[7]`).
fn reg_enum_key_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hkey, index, buffer, size, _, class, class_size, time] = args else {
        unreachable!()
    };
    if size == 0 {
        return Some(ERROR_INVALID_PARAMETER);
    }
    let key = key!(cpu, hkey);
    let Some(name) = subkey_name(cpu, key, index) else {
        return Some(ERROR_NO_MORE_ITEMS);
    };
    let room = read_32(cpu, size)?;
    let class_room = read_32(cpu, class_size)?;
    if name.len() as u32 >= room {
        return Some(ERROR_MORE_DATA);
    }
    write_string(cpu, buffer, &name);
    write_32(cpu, size, name.len() as u32);
    if class != 0 && class_room > 0 {
        cpu.bus.guest_write_8(class, 0);
    }
    write_32(cpu, class_size, 0);
    if time != 0 {
        cpu.bus.guest_write_bytes(time, &[0; 8]);
    }
    Some(ERROR_SUCCESS)
}

/// RegEnumValueA: the value `args[1]` of the key `args[0]`: its name
/// into the buffer at `args[2]`, whose size is at `args[3]`, and its type
/// and data as RegQueryValueExA gives them. When the data doesn't fit,
/// only the type and the size it needs are given. When the name doesn't
/// fit, Windows 95 gives as much of it as fits, with its zero, and that
/// length; both are ERROR_MORE_DATA.
fn reg_enum_value(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[hkey, index, buffer, size, _, ptype, data, data_size] = args else {
        unreachable!()
    };
    if buffer == 0 || size == 0 {
        return Some(ERROR_INVALID_PARAMETER);
    }
    let key = key!(cpu, hkey);
    let value = registry::get(cpu)
        .key(key)
        .values
        .get(index as usize)
        .cloned();
    let Some(value) = value else {
        return Some(ERROR_NO_MORE_ITEMS);
    };
    let room = read_32(cpu, size)?;
    if data != 0 && data_size == 0 {
        return Some(ERROR_INVALID_PARAMETER);
    }
    let name = crate::dosstr::to_bytes(&value.name);
    if !writable(cpu, &[size]) {
        return None;
    }
    let result = give_data(cpu, &value.data, value.kind, (ptype, data, data_size))?;
    if result != ERROR_SUCCESS {
        return Some(result);
    }
    if name.len() as u32 >= room {
        if room > 0 {
            let part = room as usize - 1;
            write_string(cpu, buffer, &name[..part]);
            cpu.bus.guest_write_32(size, part as u32);
        }
        return Some(ERROR_MORE_DATA);
    }
    write_string(cpu, buffer, &name);
    cpu.bus.guest_write_32(size, name.len() as u32);
    Some(ERROR_SUCCESS)
}

/// RegQueryInfoKeyA: of the key `args[0]`, its class (none), how many
/// subkeys and values it has, and the longest of their names and data,
/// at the addresses that aren't NULL.
fn reg_query_info_key(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[
        hkey,
        class,
        class_size,
        _,
        subkeys,
        max_subkey,
        max_class,
        values,
        max_name,
        max_data,
        security,
        time,
    ] = args
    else {
        unreachable!()
    };
    let key = key!(cpu, hkey);
    let class_room = read_32(cpu, class_size)?;
    let registry = registry::get(cpu);
    let k = registry.key(key);
    let longest = |names: &mut dyn Iterator<Item = usize>| names.max().unwrap_or(0) as u32;
    let counts = [
        (subkeys, k.children.len() as u32),
        (
            max_subkey,
            longest(&mut k.children.iter().map(|&c| registry.key(c).name.len())),
        ),
        (max_class, 0),
        (values, k.values.len() as u32),
        (
            max_name,
            longest(&mut k.values.iter().map(|v| v.name.len())),
        ),
        (
            max_data,
            longest(&mut k.values.iter().map(|v| v.data.len())),
        ),
        (security, 0),
        (class_size, 0),
    ];
    for (at, value) in counts {
        write_32(cpu, at, value);
    }
    if class != 0 && class_room > 0 {
        cpu.bus.guest_write_8(class, 0);
    }
    if time != 0 {
        cpu.bus.guest_write_bytes(time, &[0; 8]);
    }
    Some(ERROR_SUCCESS)
}

/// RegFlushKey: write the registry to its file, if it changed.
fn reg_flush_key(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    key!(cpu, args[0]);
    registry::save(cpu);
    Some(ERROR_SUCCESS)
}

/// RegConnectRegistryA: with no computer name, the predefined key
/// `args[1]` itself; there is no network to reach another computer's.
fn reg_connect_registry(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[machine, hkey, phk] = args else {
        unreachable!()
    };
    let machine = string_at(cpu, machine)?;
    if !machine.is_empty() {
        return Some(ERROR_BAD_NETPATH);
    }
    if phk == 0 {
        return Some(ERROR_INVALID_PARAMETER);
    }
    key!(cpu, hkey);
    cpu.bus.guest_write_32(phk, hkey);
    Some(ERROR_SUCCESS)
}

/// GetUserNameA: the name of the user who logged on, which Windows 95
/// keeps in HKLM\Network\Logon, into the buffer at `args[0]` whose size
/// is at `args[1]`, where the size it took or needs goes, with the zero.
fn get_user_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[buffer, size] = args else {
        unreachable!()
    };
    let room = read_32(cpu, size)?;
    let root = key!(cpu, HKEY_LOCAL_MACHINE);
    let registry = registry::get(cpu);
    let name = registry
        .find(root, "Network\\Logon")
        .and_then(|k| registry.value(k, "username"))
        .filter(|v| v.kind == REG_SZ)
        .map(|v| v.data.split(|&b| b == 0).next().unwrap_or(&[]).to_vec())
        .unwrap_or_else(|| b"User".to_vec());
    let need = name.len() as u32 + 1;
    if room < need {
        write_32(cpu, size, need);
        super::process(cpu).last_error = ERROR_INSUFFICIENT_BUFFER;
        return Some(FALSE);
    }
    write_string(cpu, buffer, &name);
    write_32(cpu, size, need);
    Some(TRUE)
}
