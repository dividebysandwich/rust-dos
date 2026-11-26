//! File and item monikers and the Running Object Table: CreateFileMoniker,
//! CreateItemMoniker and GetRunningObjectTable, as Windows 98's OLE32
//! has them.
//!
//! A moniker made here keeps what it names in its own block, after
//! com.rs's header: its reference count, the length of an item
//! moniker's delimiter, and its display name (the path of a file
//! moniker, the delimiter and the item of an item moniker) as UTF-16.
//! Two monikers are equal when they are of one class and their display
//! names are equal but for case, as Windows compares paths and items.
//!
//! The Running Object Table is the machine's, as on Windows: a process
//! registers an object under a moniker and another finds it there. An
//! entry holds the object as an export (rpc.rs), with a reference COM
//! holds for it, so GetObject in another process gets a proxy of it, and
//! in the process that registered it the object itself. An entry goes
//! with Revoke or when its process ends. Entries are found by the
//! moniker's class and its display name in capitals, which is what
//! Windows' comparison data of these monikers holds.
//!
//! Only monikers made here can be registered or looked up: Windows asks
//! a program's own moniker for its comparison data (IROTData) or
//! marshals it, which isn't done here. Binding (BindToObject,
//! BindToStorage), composition (ComposeWith, Inverse, CommonPrefixWith,
//! RelativePathTo), ParseDisplayName and the persistence methods of the
//! monikers, and EnumRunning of the table, give E_NOTIMPL (#2175).
//!
//! The hash and the comparisons follow Wine's ole32/filemoniker.c,
//! itemmoniker.c and moniker.c (LGPL 2.1 or later):
//!
//! Copyright 1999 Noomen Hamza
//! Copyright 2007 Robert Shearman

use super::compobj::{ADD_REF, RELEASE, call_each};
use super::{dll_index, rpc, state, task_alloc, vtable};
use crate::cpu::Cpu;
use crate::win32::com::{
    self, E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, Guid, IID_IUNKNOWN, S_FALSE, S_OK,
};
use crate::win32::user32::current_pid;

const E_NOTIMPL: u32 = 0x8000_4001;
pub const MK_E_UNAVAILABLE: u32 = 0x8004_01E3;
pub const MK_S_REDUCED_TO_SELF: u32 = 0x0004_01E2;
pub const MK_S_MONIKERALREADYREGISTERED: u32 = 0x0004_01E7;

pub const IID_IMONIKER: Guid = Guid::new(0xF, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IPERSIST: Guid = Guid::new(0x10C, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
const IID_IPERSISTSTREAM: Guid = Guid::new(0x109, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub const IID_IRUNNINGOBJECTTABLE: Guid = Guid::new(0x10, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub const CLSID_FILEMONIKER: Guid = Guid::new(0x303, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);
pub const CLSID_ITEMMONIKER: Guid = Guid::new(0x304, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

/// IsSystemMoniker's kinds.
const MKSYS_FILEMONIKER: u32 = 2;
const MKSYS_ITEMMONIKER: u32 = 4;

/// The classes of the monikers, as the id in their blocks.
pub const FILE: u32 = 1;
pub const ITEM: u32 = 2;

/// The places in a moniker's block after com.rs's header: the
/// reference count, the delimiter's and the display name's lengths in
/// UTF-16 units, and the display name with its 0.
const REFS: u32 = com::HEADER;
const DELIMITER: u32 = com::HEADER + 4;
const LENGTH: u32 = com::HEADER + 8;
const NAME: u32 = com::HEADER + 12;

/// An object registered in the Running Object Table.
#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub cookie: u32,
    pub pid: u32,
    /// The moniker's class (`FILE`, `ITEM`) and its display name in
    /// capitals.
    pub class: u32,
    pub name: String,
    /// The object, and its export that holds it.
    pub object: u32,
    pub export: u32,
    /// The time of its last change, a FILETIME: of Register, as on
    /// Windows, or the one NoteChangeTime gave since.
    pub changed: u64,
}

crate::state_fields!(Entry {
    cookie,
    pid,
    class,
    name,
    object,
    export,
    changed
});

/// The Running Object Table, and each process's interface of it.
#[derive(Clone, Debug, Default)]
pub struct Rot {
    pub entries: Vec<Entry>,
    pub next_cookie: u32,
    /// (process id, the IRunningObjectTable GetRunningObjectTable gives
    /// it).
    pub tables: Vec<(u32, u32)>,
}

crate::state_fields!(Rot {
    entries,
    next_cookie,
    tables
});

fn rot(cpu: &mut Cpu) -> &mut Rot {
    &mut state(cpu).rot
}

/// A process ended: what it registered goes, and its interface of the
/// table with its memory. The exports holding the objects go in rpc.rs.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let rot = rot(cpu);
    rot.entries.retain(|e| e.pid != pid);
    rot.tables.retain(|&(p, _)| p != pid);
}

/// UTF-16 units of the 0-terminated string at `at`, without the 0; None
/// if it can't be read.
fn units_at(cpu: &mut Cpu, at: u32) -> Option<Vec<u16>> {
    let mut units = Vec::new();
    loop {
        let p = at.checked_add(2 * units.len() as u32)?;
        if !cpu.bus.guest_probe(p, 2, false) {
            return None;
        }
        match cpu.bus.guest_read_16(p) {
            0 => return Some(units),
            c => units.push(c),
        }
    }
}

/// The capital of the UTF-16 unit `c`, where it has one of one unit.
fn upper(c: u16) -> u16 {
    let Some(ch) = char::from_u32(c as u32) else {
        return c;
    };
    let mut up = ch.to_uppercase();
    match (up.next(), up.next()) {
        (Some(u), None) if (u as u32) < 0x1_0000 => u as u16,
        _ => c,
    }
}

/// A new moniker of `class` named `name`, `delimiter` units of which
/// are an item moniker's delimiter: its interface, with one reference.
fn new_moniker(cpu: &mut Cpu, class: u32, name: &[u16], delimiter: u32) -> Option<u32> {
    let extra = NAME - com::HEADER + 2 * (name.len() as u32 + 1);
    let at = com::new_interface(cpu, dll_index(), vtable("IMoniker"), class, extra)?;
    cpu.bus.guest_write_32(at + REFS, 1);
    cpu.bus.guest_write_32(at + DELIMITER, delimiter);
    cpu.bus.guest_write_32(at + LENGTH, name.len() as u32);
    for (i, &c) in name.iter().enumerate() {
        cpu.bus.guest_write_16(at + NAME + 2 * i as u32, c);
    }
    Some(at)
}

/// The class and the display name of the moniker `this`, if it is one
/// made here.
fn moniker(cpu: &mut Cpu, this: u32) -> Option<(u32, Vec<u16>)> {
    let (v, class) = com::interface_of(cpu, dll_index(), this)?;
    if v != vtable("IMoniker") {
        return None;
    }
    let len = cpu.bus.guest_read_32(this + LENGTH);
    let name = (0..len)
        .map(|i| cpu.bus.guest_read_16(this + NAME + 2 * i))
        .collect();
    Some((class, name))
}

/// What the table finds the moniker `this` by: its class and its
/// display name in capitals.
fn key(cpu: &mut Cpu, this: u32) -> Option<(u32, String)> {
    let (class, name) = moniker(cpu, this)?;
    let upper: Vec<u16> = name.into_iter().map(upper).collect();
    Some((class, String::from_utf16_lossy(&upper)))
}

/// CreateFileMoniker(path, out).
pub fn create_file_moniker(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[path, out] = args else { unreachable!() };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    com::write_out(cpu, out, 0);
    let Some(name) = units_at(cpu, path) else {
        return Some(E_INVALIDARG);
    };
    let at = new_moniker(cpu, FILE, &name, 0);
    made(cpu, out, at)
}

/// CreateItemMoniker(delimiter, item, out). A null delimiter is none.
pub fn create_item_moniker(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[delimiter, item, out] = args else {
        unreachable!()
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    com::write_out(cpu, out, 0);
    let delimiter = match delimiter {
        0 => Vec::new(),
        at => match units_at(cpu, at) {
            Some(units) => units,
            None => return Some(E_INVALIDARG),
        },
    };
    let Some(item) = units_at(cpu, item) else {
        return Some(E_INVALIDARG);
    };
    let n = delimiter.len() as u32;
    let name = [delimiter, item].concat();
    let at = new_moniker(cpu, ITEM, &name, n);
    made(cpu, out, at)
}

fn made(cpu: &mut Cpu, out: u32, at: Option<u32>) -> Option<u32> {
    match at {
        Some(at) if com::write_out(cpu, out, at) => Some(S_OK),
        Some(at) => {
            com::free_interface(cpu, at);
            Some(E_POINTER)
        }
        None => Some(E_OUTOFMEMORY),
    }
}

/// QueryInterface of an object of `iids`: itself, with a reference more.
fn query(cpu: &mut Cpu, this: u32, iid: u32, out: u32, iids: &[Guid]) -> u32 {
    if out == 0 {
        return E_POINTER;
    }
    com::write_out(cpu, out, 0);
    let Some(iid) = Guid::read(cpu, iid) else {
        return E_INVALIDARG;
    };
    if iid != IID_IUNKNOWN && !iids.contains(&iid) {
        return E_NOINTERFACE;
    }
    add_ref_moniker(cpu, this);
    com::write_out(cpu, out, this);
    S_OK
}

fn add_ref_moniker(cpu: &mut Cpu, this: u32) -> u32 {
    let refs = cpu.bus.guest_read_32(this + REFS).wrapping_add(1);
    cpu.bus.guest_write_32(this + REFS, refs);
    refs
}

pub fn moniker_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, iid, out] = args else {
        unreachable!()
    };
    if moniker(cpu, this).is_none() {
        return Some(E_INVALIDARG);
    }
    let iids = [IID_IMONIKER, IID_IPERSIST, IID_IPERSISTSTREAM];
    Some(query(cpu, this, iid, out, &iids))
}

pub fn moniker_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let this = args[0];
    if moniker(cpu, this).is_none() {
        return Some(0);
    }
    Some(add_ref_moniker(cpu, this))
}

pub fn moniker_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let this = args[0];
    if moniker(cpu, this).is_none() {
        return Some(0);
    }
    let refs = cpu.bus.guest_read_32(this + REFS).saturating_sub(1);
    cpu.bus.guest_write_32(this + REFS, refs);
    if refs == 0 {
        com::free_interface(cpu, this);
    }
    Some(refs)
}

/// IPersist::GetClassID(this, out).
pub fn moniker_get_class_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, out] = args else { unreachable!() };
    let Some((class, _)) = moniker(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 || !cpu.bus.guest_probe(out, 16, true) {
        return Some(E_POINTER);
    }
    let clsid = if class == FILE {
        CLSID_FILEMONIKER
    } else {
        CLSID_ITEMMONIKER
    };
    cpu.bus.guest_write_bytes(out, &clsid.0);
    Some(S_OK)
}

/// IPersistStream::IsDirty: these monikers never change.
pub fn moniker_is_dirty(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_FALSE)
}

/// What isn't done here: see the module's comment.
pub fn moniker_not_implemented(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(E_NOTIMPL)
}

/// IMoniker::BindToObject and BindToStorage(this, bc, left, iid, out):
/// not done here, so `out` gets null.
pub fn moniker_bind(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    com::write_out(cpu, args[4], 0);
    Some(E_NOTIMPL)
}

/// IMoniker::Reduce(this, bc, how far, left, out): these monikers reduce
/// to themselves.
pub fn moniker_reduce(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, _, _, _, out] = args else {
        unreachable!()
    };
    if moniker(cpu, this).is_none() {
        return Some(E_INVALIDARG);
    }
    if out == 0 {
        return Some(E_POINTER);
    }
    add_ref_moniker(cpu, this);
    com::write_out(cpu, out, this);
    Some(MK_S_REDUCED_TO_SELF)
}

/// IMoniker::Enum(this, forward, out): a simple moniker has no parts to
/// enumerate, and gives null.
pub fn moniker_enum(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let out = args[2];
    if out == 0 {
        return Some(E_POINTER);
    }
    com::write_out(cpu, out, 0);
    Some(S_OK)
}

/// IMoniker::IsEqual(this, other): S_OK for a moniker of the same class
/// whose display name is the same but for case.
pub fn moniker_is_equal(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, other] = args else { unreachable!() };
    let (Some(a), Some(b)) = (key(cpu, this), key(cpu, other)) else {
        return Some(S_FALSE);
    };
    Some(if a == b { S_OK } else { S_FALSE })
}

/// IMoniker::Hash(this, out): from the name in capitals, so that equal
/// monikers hash alike. An item moniker's is Windows' and Wine's, of
/// its item without the delimiter (Wine's tests have 0x73C for "Test").
/// A file moniker's is sampled as Wine's is for a long path.
pub fn moniker_hash(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, out] = args else { unreachable!() };
    let Some((class, name)) = moniker(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_POINTER);
    }
    let units: Vec<u32> = name.into_iter().map(|c| upper(c) as u32).collect();
    let hash = if class == ITEM {
        let delimiter = cpu.bus.guest_read_32(this + DELIMITER) as usize;
        units[delimiter.min(units.len())..]
            .iter()
            .fold(0u32, |h, &c| h.wrapping_mul(3) ^ c)
    } else if units.len() < 16 {
        units
            .iter()
            .fold(0u32, |h, &c| h.wrapping_mul(37).wrapping_add(c))
    } else {
        let skip = units.len() / 8;
        units
            .iter()
            .step_by(skip)
            .fold(0u32, |h, &c| h.wrapping_mul(39).wrapping_add(c))
    };
    com::write_out(cpu, out, hash);
    Some(S_OK)
}

/// IMoniker::IsRunning(this, bc, left, newly running): whether the
/// moniker is newly running, or in the Running Object Table.
pub fn moniker_is_running(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, _, _, newly] = args else {
        unreachable!()
    };
    let Some(key) = key(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if newly != 0 && self::key(cpu, newly).as_ref() == Some(&key) {
        return Some(S_OK);
    }
    Some(if find(cpu, &key).is_some() {
        S_OK
    } else {
        S_FALSE
    })
}

/// IMoniker::GetTimeOfLastChange(this, bc, left, out): the time of the
/// last change the table has for it.
pub fn moniker_get_time_of_last_change(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, _, _, out] = args else {
        unreachable!()
    };
    let Some(key) = key(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    Some(time_of_last_change(cpu, &key, out))
}

/// IMoniker::GetDisplayName(this, bc, left, out): the display name, in
/// memory of the task allocator.
pub fn moniker_get_display_name(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, _, _, out] = args else {
        unreachable!()
    };
    let Some((_, name)) = moniker(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_POINTER);
    }
    let at = task_alloc(cpu, 2 * (name.len() as u32 + 1));
    if at == 0 {
        com::write_out(cpu, out, 0);
        return Some(E_OUTOFMEMORY);
    }
    for (i, c) in name.iter().copied().chain([0]).enumerate() {
        cpu.bus.guest_write_16(at + 2 * i as u32, c);
    }
    com::write_out(cpu, out, at);
    Some(S_OK)
}

/// IMoniker::IsSystemMoniker(this, out).
pub fn moniker_is_system_moniker(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, out] = args else { unreachable!() };
    let Some((class, _)) = moniker(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_POINTER);
    }
    let kind = if class == FILE {
        MKSYS_FILEMONIKER
    } else {
        MKSYS_ITEMMONIKER
    };
    com::write_out(cpu, out, kind);
    Some(S_OK)
}

// The Running Object Table.

/// The entry registered under `key`, the first registered of several.
fn find(cpu: &mut Cpu, key: &(u32, String)) -> Option<Entry> {
    rot(cpu)
        .entries
        .iter()
        .find(|e| e.class == key.0 && e.name == key.1)
        .cloned()
}

fn time_of_last_change(cpu: &mut Cpu, key: &(u32, String), out: u32) -> u32 {
    if out == 0 || !cpu.bus.guest_probe(out, 8, true) {
        return E_POINTER;
    }
    match find(cpu, key) {
        Some(e) => {
            cpu.bus.guest_write_32(out, e.changed as u32);
            cpu.bus.guest_write_32(out + 4, (e.changed >> 32) as u32);
            S_OK
        }
        None => MK_E_UNAVAILABLE,
    }
}

/// GetRunningObjectTable(reserved, out): the process's interface of the
/// machine's table, the same each time.
pub fn get_running_object_table(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[_, out] = args else { unreachable!() };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    let pid = current_pid(cpu);
    let found = rot(cpu).tables.iter().find(|t| t.0 == pid).map(|t| t.1);
    let table = match found {
        Some(t) => t,
        None => {
            let Some(t) = com::new_interface(cpu, dll_index(), vtable("IRunningObjectTable"), 0, 0)
            else {
                com::write_out(cpu, out, 0);
                return Some(E_OUTOFMEMORY);
            };
            rot(cpu).tables.push((pid, t));
            t
        }
    };
    if !com::write_out(cpu, out, table) {
        return Some(E_INVALIDARG);
    }
    Some(S_OK)
}

fn is_table(cpu: &mut Cpu, this: u32) -> bool {
    com::interface_of(cpu, dll_index(), this)
        .is_some_and(|(v, _)| v == vtable("IRunningObjectTable"))
}

pub fn rot_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, iid, out] = args else {
        unreachable!()
    };
    if !is_table(cpu, this) {
        return Some(E_INVALIDARG);
    }
    if out == 0 {
        return Some(E_POINTER);
    }
    com::write_out(cpu, out, 0);
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(E_INVALIDARG);
    };
    if iid != IID_IUNKNOWN && iid != IID_IRUNNINGOBJECTTABLE {
        return Some(E_NOINTERFACE);
    }
    com::write_out(cpu, out, this);
    Some(S_OK)
}

/// AddRef and Release of the table, which lives as long as its process,
/// as Windows' does in OLE32.
pub fn rot_add_ref(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

/// IRunningObjectTable::Register(this, flags, object, moniker, out): the
/// object goes in under the moniker, held with a reference of the
/// table's, and `out` gets the cookie that revokes it.
/// MK_S_MONIKERALREADYREGISTERED when the moniker had an object, which
/// stays found first.
pub fn rot_register(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, _flags, object, moniker, out] = args else {
        unreachable!()
    };
    if !is_table(cpu, this) {
        return Some(E_INVALIDARG);
    }
    if object == 0 || moniker == 0 || out == 0 {
        return Some(E_INVALIDARG);
    }
    let Some(key) = key(cpu, moniker) else {
        cpu.bus.log_string(
            "[W32] OLE32: Register in the Running Object Table with a moniker not of OLE32's own",
        );
        return Some(E_NOTIMPL);
    };
    if !com::write_out(cpu, out, 0) {
        return Some(E_INVALIDARG);
    }
    let already = find(cpu, &key).is_some();
    let (export, new) = rpc::hold(cpu, object);
    let pid = current_pid(cpu);
    let changed = crate::win32::kernel32::time::file_time_now(cpu);
    let rot = rot(cpu);
    rot.next_cookie = rot.next_cookie.wrapping_add(1).max(1);
    let cookie = rot.next_cookie;
    rot.entries.push(Entry {
        cookie,
        pid,
        class: key.0,
        name: key.1,
        object,
        export,
        changed,
    });
    com::write_out(cpu, out, cookie);
    let hr = if already {
        MK_S_MONIKERALREADYREGISTERED
    } else {
        S_OK
    };
    // A new export holds a reference of COM's to the object, which the
    // last of its holders lets go.
    if new {
        return call_each(cpu, hr, ADD_REF, [object].into_iter());
    }
    Some(hr)
}

/// IRunningObjectTable::Revoke(this, cookie): E_INVALIDARG for a cookie
/// another process registered, whose registrations Windows keeps in
/// that process's OLE32 (Wine's ole32/moniker.c does too). The table's
/// reference to the object goes before Revoke returns, when it is the
/// last and the object is of this thread, so a program that waits for
/// the object to go after revoking it isn't kept waiting for its
/// messages.
pub fn rot_revoke(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, cookie] = args else {
        unreachable!()
    };
    if !is_table(cpu, this) {
        return Some(E_INVALIDARG);
    }
    let pid = current_pid(cpu);
    let rot = rot(cpu);
    let Some(i) = rot
        .entries
        .iter()
        .position(|e| e.cookie == cookie && e.pid == pid)
    else {
        return Some(E_INVALIDARG);
    };
    let e = rot.entries.remove(i);
    match rpc::let_go(cpu, e.export) {
        Some(object) => call_each(cpu, S_OK, RELEASE, [object].into_iter()),
        None => Some(S_OK),
    }
}

/// IRunningObjectTable::IsRunning(this, moniker).
pub fn rot_is_running(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, moniker] = args else {
        unreachable!()
    };
    if !is_table(cpu, this) {
        return Some(E_INVALIDARG);
    }
    let Some(key) = key(cpu, moniker) else {
        return Some(S_FALSE);
    };
    Some(if find(cpu, &key).is_some() {
        S_OK
    } else {
        S_FALSE
    })
}

/// IRunningObjectTable::GetObject(this, moniker, out): the object
/// registered under the moniker, with a reference more; in another
/// process than the one that registered it, a proxy of it.
pub fn rot_get_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, moniker, out] = args else {
        unreachable!()
    };
    if !is_table(cpu, this) {
        return Some(E_INVALIDARG);
    }
    if out == 0 {
        return Some(E_POINTER);
    }
    com::write_out(cpu, out, 0);
    let Some(key) = key(cpu, moniker) else {
        return Some(MK_E_UNAVAILABLE);
    };
    let Some(e) = find(cpu, &key) else {
        return Some(MK_E_UNAVAILABLE);
    };
    if e.pid == current_pid(cpu) {
        com::write_out(cpu, out, e.object);
        return call_each(cpu, S_OK, ADD_REF, [e.object].into_iter());
    }
    match rpc::proxy_of_held(cpu, e.export) {
        0 => Some(MK_E_UNAVAILABLE),
        proxy => {
            com::write_out(cpu, out, proxy);
            Some(S_OK)
        }
    }
}

/// IRunningObjectTable::NoteChangeTime(this, cookie, time). As Revoke,
/// it takes only a cookie of the running process.
pub fn rot_note_change_time(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, cookie, time] = args else {
        unreachable!()
    };
    if !is_table(cpu, this) {
        return Some(E_INVALIDARG);
    }
    if time == 0 || !cpu.bus.guest_probe(time, 8, false) {
        return Some(E_INVALIDARG);
    }
    let value = cpu.bus.guest_read_32(time) as u64 | (cpu.bus.guest_read_32(time + 4) as u64) << 32;
    let pid = current_pid(cpu);
    let entries = &mut rot(cpu).entries;
    match entries.iter_mut().find(|e| e.cookie == cookie && e.pid == pid) {
        Some(e) => {
            e.changed = value;
            Some(S_OK)
        }
        None => Some(E_INVALIDARG),
    }
}

/// IRunningObjectTable::GetTimeOfLastChange(this, moniker, out).
pub fn rot_get_time_of_last_change(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[this, moniker, out] = args else {
        unreachable!()
    };
    if !is_table(cpu, this) {
        return Some(E_INVALIDARG);
    }
    let Some(key) = key(cpu, moniker) else {
        return Some(MK_E_UNAVAILABLE);
    };
    Some(time_of_last_change(cpu, &key, out))
}

/// IRunningObjectTable::EnumRunning(this, out): not done here.
pub fn rot_enum_running(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    com::write_out(cpu, args[1], 0);
    Some(E_NOTIMPL)
}
