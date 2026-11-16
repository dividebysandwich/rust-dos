//! IDirectPlay8LobbiedApplication, of a program no lobby launched. It
//! registers programs for lobbies to launch, under
//! HKLM\Software\Microsoft\DirectPlay8\Applications, a key for each
//! program named by its application name, with the values of its
//! DPN_PROGRAM_DESC; RegisterProgram of a GUID registered already
//! replaces its key. It has no lobby connection, so what takes one
//! fails with DPNERR_INVALIDHANDLE.

use super::{
    DPN_OK, DPNERR_ALREADYINITIALIZED, DPNERR_DOESNOTEXIST, DPNERR_INVALIDFLAGS,
    DPNERR_INVALIDHANDLE, DPNERR_INVALIDPARAM, DPNERR_INVALIDPOINTER, DPNERR_UNINITIALIZED, object,
    object_id, read_u32, read_wide,
};
use crate::cpu::Cpu;
use crate::win32::com::Guid;
use crate::win32::registry::{self, HKEY_LOCAL_MACHINE, Open, REG_SZ, Registry};

/// Initialize's one flag.
const DPLINITIALIZE_DISABLEPARAMVAL: u32 = 1;
/// SetAppAvailable's one flag.
const DPLAVAILABLE_ALLOWMULTIPLECONNECT: u32 = 1;

/// The key the programs are registered under, in HKEY_LOCAL_MACHINE.
pub const APPLICATIONS: &str = "Software\\Microsoft\\DirectPlay8\\Applications";

/// DPN_PROGRAM_DESC: its size, flags, the application's GUID, then the
/// strings, by their offsets and the names of the values they go to.
const PROGRAM_DESC_SIZE: u32 = 0x38;
const GUID_AT: u32 = 8;
const APPLICATION_NAME_AT: u32 = 0x18;
const EXECUTABLE_FILENAME_AT: u32 = 0x28;
const STRINGS: [(u32, &str); 8] = [
    (0x18, "ApplicationName"),
    (0x1C, "CommandLine"),
    (0x20, "CurrentDirectory"),
    (0x24, "Description"),
    (0x28, "ExecutableFilename"),
    (0x2C, "ExecutablePath"),
    (0x30, "LauncherFilename"),
    (0x34, "LauncherPath"),
];

/// The lobbied application `this` is, or the method returns
/// DPNERR_INVALIDOBJECT; and DPNERR_UNINITIALIZED until its Initialize.
macro_rules! initialized {
    ($cpu:expr, $this:expr) => {{
        let id = object_id!($cpu, $this);
        if !object($cpu, id).initialized {
            return Some(DPNERR_UNINITIALIZED);
        }
        id
    }};
}

/// Initialize(this, context, handler, connection out, flags): no lobby
/// launched the program, so there is no connection to give it, and its
/// handle is left as it was.
pub(super) fn initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, handler, _, flags] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    let id = object_id!(cpu, this);
    if handler == 0 {
        return Some(DPNERR_INVALIDPOINTER);
    }
    if flags & !DPLINITIALIZE_DISABLEPARAMVAL != 0 {
        return Some(DPNERR_INVALIDFLAGS);
    }
    let o = object(cpu, id);
    if o.initialized {
        return Some(DPNERR_ALREADYINITIALIZED);
    }
    o.initialized = true;
    Some(DPN_OK)
}

/// Close(this, flags).
pub(super) fn close(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let id = initialized!(cpu, args[0]);
    object(cpu, id).initialized = false;
    Some(DPN_OK)
}

/// Send, UpdateStatus, GetConnectionSettings and SetConnectionSettings,
/// each first given a connection's handle: there is none.
pub(super) fn no_connection(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    initialized!(cpu, args[0]);
    Some(DPNERR_INVALIDHANDLE)
}

/// SetAppAvailable(this, available, flags): whether a lobby may connect
/// the running program to a session, which no lobby asks.
pub(super) fn set_app_available(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, _, flags] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    initialized!(cpu, this);
    if flags & !DPLAVAILABLE_ALLOWMULTIPLECONNECT != 0 {
        return Some(DPNERR_INVALIDFLAGS);
    }
    Some(DPN_OK)
}

/// RegisterProgram(this, description, flags): the program's key, made
/// or replaced. The application name and the executable's file name
/// are needed; the other strings may be left out.
pub(super) fn register_program(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, desc, flags] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    initialized!(cpu, this);
    if desc == 0 || !cpu.bus.guest_probe(desc, PROGRAM_DESC_SIZE as usize, false) {
        return Some(DPNERR_INVALIDPOINTER);
    }
    if flags != 0 || cpu.bus.guest_read_32(desc + 4) != 0 {
        return Some(DPNERR_INVALIDFLAGS);
    }
    if cpu.bus.guest_read_32(desc) != PROGRAM_DESC_SIZE {
        return Some(DPNERR_INVALIDPARAM);
    }
    let Some(guid) = Guid::read(cpu, desc + GUID_AT) else {
        return Some(DPNERR_INVALIDPOINTER);
    };
    let mut values = Vec::new();
    for (at, name) in STRINGS {
        let pointer = read_u32(cpu, desc + at).unwrap_or(0);
        if pointer == 0 {
            if at == APPLICATION_NAME_AT || at == EXECUTABLE_FILENAME_AT {
                return Some(DPNERR_INVALIDPARAM);
            }
            continue;
        }
        let Some(text) = read_wide(cpu, pointer) else {
            return Some(DPNERR_INVALIDPOINTER);
        };
        values.push((name, text));
    }
    let ansi = |cpu: &Cpu, units: &[u16]| {
        let mut bytes = crate::win32::kernel32::wide_to_ansi(cpu, units);
        bytes.push(0);
        bytes
    };
    let values: Vec<(&str, Vec<u8>)> = values
        .iter()
        .map(|(name, text)| (*name, ansi(cpu, text)))
        .collect();
    // The key's name: the application name, which STRINGS lists first.
    let name = &values[0].1;
    let key_name = crate::dosstr::from_bytes(&name[..name.len() - 1]);
    if key_name.is_empty() || key_name.contains('\\') {
        return Some(DPNERR_INVALIDPARAM);
    }
    let registry = registry::get(cpu);
    let Open::Key(root) = registry.open(HKEY_LOCAL_MACHINE, 0) else {
        return Some(DPNERR_INVALIDPARAM);
    };
    let (applications, _) = registry.create(root, APPLICATIONS);
    // The program's earlier key, and any other program's of its name.
    while let Some(old) = registered(registry, applications, &guid)
        .or_else(|| registry.child(applications, &key_name))
    {
        registry.delete(old);
    }
    let (key, _) = registry.create(applications, &key_name);
    for (name, data) in values {
        registry.set_value(key, name, REG_SZ, data);
    }
    let mut guid_text = guid.to_string().into_bytes();
    guid_text.push(0);
    registry.set_value(key, "GUID", REG_SZ, guid_text);
    Some(DPN_OK)
}

/// UnRegisterProgram(this, guid, flags): the key of the program `guid`
/// goes.
pub(super) fn unregister_program(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, guid, flags] = args[..] else {
        return Some(DPNERR_INVALIDPARAM);
    };
    initialized!(cpu, this);
    if flags != 0 {
        return Some(DPNERR_INVALIDFLAGS);
    }
    let Some(guid) = Guid::read(cpu, guid) else {
        return Some(DPNERR_INVALIDPOINTER);
    };
    let registry = registry::get(cpu);
    let Open::Key(root) = registry.open(HKEY_LOCAL_MACHINE, 0) else {
        return Some(DPNERR_DOESNOTEXIST);
    };
    let found = registry
        .find(root, APPLICATIONS)
        .and_then(|applications| registered(registry, applications, &guid));
    match found {
        Some(key) => {
            registry.delete(key);
            Some(DPN_OK)
        }
        None => Some(DPNERR_DOESNOTEXIST),
    }
}

/// The key under `applications` of the program registered as `guid`.
fn registered(registry: &Registry, applications: u32, guid: &Guid) -> Option<u32> {
    let wanted = guid.to_string();
    registry
        .key(applications)
        .children
        .iter()
        .copied()
        .find(|&key| {
            registry
                .value(key, "GUID")
                .is_some_and(|v| v.text().eq_ignore_ascii_case(&wanted))
        })
}
