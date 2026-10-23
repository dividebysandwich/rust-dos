//! OLE32.DLL: what programs of Windows 95's time call COM's library for.
//! CoCreateInstance and CoGetClassObject make the built-in DLLs' classes
//! (com.rs) through a class factory of OLE32's own; other classes aren't
//! registered. There are the task allocator (CoTaskMemAlloc, CoGetMalloc)
//! and the functions between GUIDs and their strings.
//!
//! A thread has to call CoInitialize (or OleInitialize) before it makes
//! objects, as on Windows. Each process's task allocator is one object,
//! made the first time CoGetMalloc is called, which Release doesn't free.

use super::com::{
    self, CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, CO_E_NOTINITIALIZED, E_INVALIDARG,
    E_NOINTERFACE, E_OUTOFMEMORY, E_POINTER, Guid, IID_ICLASSFACTORY, IID_IUNKNOWN,
    REGDB_E_CLASSNOTREG, S_FALSE, S_OK,
};
use super::stubs::{Api, Conv, Dll, Vtable};
use crate::cpu::Cpu;

pub static DLL: Dll = Dll {
    name: "OLE32.DLL",
    base: 0xBF90_0000,
    apis: APIS,
    vars: &[],
    init: None,
    vtables: VTABLES,
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

/// The functions. Append only (see `stubs`). The methods of OLE32's own
/// objects have ordinal 0: they aren't exported.
static APIS: &[Api] = &[
    stdcall("CoInitialize", 1, 1, co_initialize),
    stdcall("CoInitializeEx", 2, 2, co_initialize_ex),
    stdcall("CoUninitialize", 3, 0, co_uninitialize),
    stdcall("OleInitialize", 4, 1, co_initialize),
    stdcall("OleUninitialize", 5, 0, co_uninitialize),
    stdcall("CoCreateInstance", 6, 5, co_create_instance),
    stdcall("CoGetClassObject", 7, 5, co_get_class_object),
    stdcall("CoTaskMemAlloc", 8, 1, co_task_mem_alloc),
    stdcall("CoTaskMemRealloc", 9, 2, co_task_mem_realloc),
    stdcall("CoTaskMemFree", 10, 1, co_task_mem_free),
    stdcall("CoGetMalloc", 11, 2, co_get_malloc),
    stdcall("StringFromGUID2", 12, 3, string_from_guid2),
    stdcall("StringFromCLSID", 13, 2, string_from_clsid),
    stdcall("StringFromIID", 14, 2, string_from_clsid),
    stdcall("CLSIDFromString", 15, 2, clsid_from_string),
    stdcall("IIDFromString", 16, 2, clsid_from_string),
    stdcall("IsEqualGUID", 17, 2, is_equal_guid),
    stdcall("CoCreateGuid", 18, 1, co_create_guid),
    stdcall("CoFreeUnusedLibraries", 19, 0, nothing),
    stdcall("CoGetCurrentProcess", 20, 0, co_get_current_process),
    stdcall("CLSIDFromProgID", 21, 2, clsid_from_prog_id),
    stdcall(
        "IClassFactory_QueryInterface",
        0,
        3,
        factory_query_interface,
    ),
    stdcall("IClassFactory_AddRef", 0, 1, factory_add_ref),
    stdcall("IClassFactory_Release", 0, 1, factory_release),
    stdcall(
        "IClassFactory_CreateInstance",
        0,
        4,
        factory_create_instance,
    ),
    stdcall("IClassFactory_LockServer", 0, 2, factory_lock_server),
    stdcall("IMalloc_QueryInterface", 0, 3, malloc_query_interface),
    stdcall("IMalloc_AddRef", 0, 1, malloc_add_ref),
    stdcall("IMalloc_Release", 0, 1, malloc_add_ref),
    stdcall("IMalloc_Alloc", 0, 2, malloc_alloc),
    stdcall("IMalloc_Realloc", 0, 3, malloc_realloc),
    stdcall("IMalloc_Free", 0, 2, malloc_free),
    stdcall("IMalloc_GetSize", 0, 2, malloc_get_size),
    stdcall("IMalloc_DidAlloc", 0, 2, malloc_did_alloc),
    stdcall("IMalloc_HeapMinimize", 0, 1, malloc_heap_minimize),
];

/// The vtables, by their places in this list.
static VTABLES: &[Vtable] = &[
    Vtable {
        name: "IClassFactory",
        methods: &[
            "IClassFactory_QueryInterface",
            "IClassFactory_AddRef",
            "IClassFactory_Release",
            "IClassFactory_CreateInstance",
            "IClassFactory_LockServer",
        ],
    },
    Vtable {
        name: "IMalloc",
        methods: &[
            "IMalloc_QueryInterface",
            "IMalloc_AddRef",
            "IMalloc_Release",
            "IMalloc_Alloc",
            "IMalloc_Realloc",
            "IMalloc_Free",
            "IMalloc_GetSize",
            "IMalloc_DidAlloc",
            "IMalloc_HeapMinimize",
        ],
    },
];
const CLASS_FACTORY: usize = 0;
const MALLOC: usize = 1;

const IID_IMALLOC: Guid = Guid::new(2, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]);

/// CLSCTX_INPROC_SERVER and CLSCTX_INPROC_HANDLER: the contexts a built-in
/// class has.
const CLSCTX_INPROC: u32 = 0x3;

/// OLE32's state: the system's, kept per thread and process.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// The threads that called CoInitialize, with how many times.
    initialized: Vec<(u32, u32)>,
    /// Each process's task allocator, once it has one: (process id,
    /// address).
    mallocs: Vec<(u32, u32)>,
    /// The threads CoInitializeEx put in the multithreaded apartment.
    multithreaded: Vec<u32>,
}

fn state(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.ole
}

fn dll_index() -> usize {
    super::stubs::DLLS
        .iter()
        .position(|d| d.name == DLL.name)
        .expect("OLE32 is a built-in DLL")
}

/// A process ended: its threads' initializations and its allocator go.
pub fn process_ended(cpu: &mut Cpu, pid: u32, tids: &[u32]) {
    let ole = state(cpu);
    ole.initialized.retain(|(t, _)| !tids.contains(t));
    ole.mallocs.retain(|&(p, _)| p != pid);
    ole.multithreaded.retain(|t| !tids.contains(t));
}

fn nothing(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_OK)
}

/// CoInitializeEx(reserved, flags): with COINIT_APARTMENTTHREADED (2)
/// as CoInitialize; without it the thread is in the multithreaded
/// apartment.
fn co_initialize_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let result = co_initialize(cpu, args)?;
    let tid = super::user32::current_tid(cpu);
    let ole = state(cpu);
    if result == S_OK && args[1] & 2 == 0 && !ole.multithreaded.contains(&tid) {
        ole.multithreaded.push(tid);
    }
    Some(result)
}

/// CoInitialize, CoInitializeEx, OleInitialize: S_FALSE when the thread
/// had called it already.
fn co_initialize(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let tid = super::user32::current_tid(cpu);
    let ole = state(cpu);
    match ole.initialized.iter_mut().find(|(t, _)| *t == tid) {
        Some((_, n)) => {
            *n += 1;
            Some(S_FALSE)
        }
        None => {
            ole.initialized.push((tid, 1));
            Some(S_OK)
        }
    }
}

fn co_uninitialize(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let tid = super::user32::current_tid(cpu);
    let ole = state(cpu);
    if let Some(i) = ole.initialized.iter().position(|(t, _)| *t == tid) {
        ole.initialized[i].1 -= 1;
        if ole.initialized[i].1 == 0 {
            ole.initialized.remove(i);
            ole.multithreaded.retain(|&t| t != tid);
        }
    }
    Some(0)
}

/// Whether the running thread called CoInitialize.
fn initialized(cpu: &mut Cpu) -> bool {
    let tid = super::user32::current_tid(cpu);
    state(cpu).initialized.iter().any(|(t, _)| *t == tid)
}

/// The built-in class `clsid`, for the context `context`; or the HRESULT
/// to fail with.
fn class_for(cpu: &mut Cpu, clsid: u32, context: u32) -> Result<usize, u32> {
    let clsid = Guid::read(cpu, clsid).ok_or(E_INVALIDARG)?;
    if !initialized(cpu) {
        return Err(CO_E_NOTINITIALIZED);
    }
    let (index, _) = com::find_class(&clsid).ok_or(REGDB_E_CLASSNOTREG)?;
    if context & CLSCTX_INPROC == 0 {
        return Err(REGDB_E_CLASSNOTREG);
    }
    Ok(index)
}

/// CoCreateInstance(clsid, outer, context, iid, out).
fn co_create_instance(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [clsid, outer, context, iid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_POINTER);
    }
    com::write_out(cpu, out, 0);
    let index = match class_for(cpu, clsid, context) {
        Ok(index) => index,
        Err(e) => return Some(e),
    };
    if outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(E_INVALIDARG);
    };
    // DirectMusic's classes are apartment-threaded: a thread of the
    // multithreaded apartment gets one through a proxy, which Windows 98
    // has only for IUnknown (REGDB_E_IIDNOTREG for the others).
    let tid = super::user32::current_tid(cpu);
    if iid != com::IID_IUNKNOWN
        && state(cpu).multithreaded.contains(&tid)
        && com::class_at(index).is_some_and(|c| super::dmusic::is_apartment_class(&c.clsid))
    {
        return Some(0x8004_0155);
    }
    Some(create(cpu, index, &iid, out))
}

/// CoGetClassObject(clsid, context, server info, iid, out).
fn co_get_class_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [clsid, context, _, iid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_POINTER);
    }
    com::write_out(cpu, out, 0);
    let index = match class_for(cpu, clsid, context) {
        Ok(index) => index,
        Err(e) => return Some(e),
    };
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(E_INVALIDARG);
    };
    Some(new_factory(cpu, index, &iid, out))
}

/// A DLL's DllGetClassObject(clsid, iid, out), for its classes alone: the
/// DLL `dll`, as `stubs::DLLS` names it. Needs no CoInitialize.
pub fn dll_get_class_object(cpu: &mut Cpu, dll: &str, args: &[u32]) -> u32 {
    let [clsid, iid, out] = args[..] else {
        return E_INVALIDARG;
    };
    if out == 0 {
        return E_POINTER;
    }
    com::write_out(cpu, out, 0);
    let (Some(clsid), Some(iid)) = (Guid::read(cpu, clsid), Guid::read(cpu, iid)) else {
        return E_INVALIDARG;
    };
    match com::find_class(&clsid) {
        Some((index, class)) if class.dll == dll => new_factory(cpu, index, &iid, out),
        _ => CLASS_E_CLASSNOTAVAILABLE,
    }
}

/// A class factory of the class at `index` of `com::CLASSES`, given as
/// the interface `iid` at `out`. Its block keeps its reference count
/// after the header.
fn new_factory(cpu: &mut Cpu, index: usize, iid: &Guid, out: u32) -> u32 {
    if *iid != IID_ICLASSFACTORY && *iid != IID_IUNKNOWN {
        return E_NOINTERFACE;
    }
    let Some(this) = com::new_interface(cpu, dll_index(), CLASS_FACTORY, index as u32, 4) else {
        return E_OUTOFMEMORY;
    };
    cpu.bus.guest_write_32(this + com::HEADER, 1);
    if !com::write_out(cpu, out, this) {
        com::free_interface(cpu, this);
        return E_POINTER;
    }
    S_OK
}

/// The class a factory `this` makes, if `this` is one.
fn factory(cpu: &mut Cpu, this: u32) -> Option<usize> {
    match com::interface_of(cpu, dll_index(), this)? {
        (CLASS_FACTORY, index) => Some(index as usize),
        _ => None,
    }
}

fn factory_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if factory(cpu, this).is_none() {
        return Some(E_INVALIDARG);
    }
    match Guid::read(cpu, iid) {
        Some(g) if g == IID_ICLASSFACTORY || g == IID_IUNKNOWN => {
            factory_add_ref(cpu, &[this]);
            com::write_out(cpu, out, this);
            Some(S_OK)
        }
        _ => {
            com::write_out(cpu, out, 0);
            Some(E_NOINTERFACE)
        }
    }
}

fn factory_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let this = args[0];
    factory(cpu, this)?;
    let refs = cpu.bus.guest_read_32(this + com::HEADER) + 1;
    cpu.bus.guest_write_32(this + com::HEADER, refs);
    Some(refs)
}

fn factory_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let this = args[0];
    if factory(cpu, this).is_none() {
        return Some(0);
    }
    let refs = cpu.bus.guest_read_32(this + com::HEADER).saturating_sub(1);
    cpu.bus.guest_write_32(this + com::HEADER, refs);
    if refs == 0 {
        com::free_interface(cpu, this);
    }
    Some(refs)
}

/// IClassFactory::CreateInstance(this, outer, iid, out).
fn factory_create_instance(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, outer, iid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(index) = factory(cpu, this) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_POINTER);
    }
    com::write_out(cpu, out, 0);
    if outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(E_INVALIDARG);
    };
    Some(create(cpu, index, &iid, out))
}

/// Make an object of the class at `index` of `com::CLASSES`, given as
/// the interface `iid` at `out`: the class's HRESULT.
fn create(cpu: &mut Cpu, index: usize, iid: &Guid, out: u32) -> u32 {
    // The index comes from the factory's interface, which the program
    // may have made up.
    let Some(class) = com::class_at(index) else {
        return CLASS_E_CLASSNOTAVAILABLE;
    };
    // The class's DLL, which COM loads for its objects' methods.
    if super::loader::map_builtin_named(cpu, class.dll).is_none() {
        return E_OUTOFMEMORY;
    }
    let result = (class.create)(cpu, iid, out);
    cpu.bus.log_string(&format!(
        "[W32] OLE32 makes a {} ({}): {:08X}",
        class.name, iid, result
    ));
    result
}

fn factory_lock_server(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_OK)
}

/// A block of the process heap for the task allocator, or 0.
pub(super) fn task_alloc(cpu: &mut Cpu, size: u32) -> u32 {
    super::with_process(cpu, |p, bus| p.heap.alloc(&mut p.memory, bus, size.max(1))).unwrap_or(0)
}

fn task_free(cpu: &mut Cpu, at: u32) {
    if at != 0 {
        super::with_process(cpu, |p, _| p.heap.free(at));
    }
}

fn task_size(cpu: &mut Cpu, at: u32) -> Option<u32> {
    super::with_process(cpu, |p, _| p.heap.size(at))
}

/// The block `at` made `size` bytes long, moved if it has to be: its
/// address, or 0 with `at` left as it was.
fn task_realloc(cpu: &mut Cpu, at: u32, size: u32) -> u32 {
    if at == 0 {
        return task_alloc(cpu, size);
    }
    if size == 0 {
        task_free(cpu, at);
        return 0;
    }
    let Some(old) = task_size(cpu, at) else {
        return 0;
    };
    let to = task_alloc(cpu, size);
    if to != 0 {
        let mut bytes = vec![0; old.min(size) as usize];
        cpu.bus.guest_read_bytes(at, &mut bytes);
        cpu.bus.guest_write_bytes(to, &bytes);
        task_free(cpu, at);
    }
    to
}

fn co_task_mem_alloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(task_alloc(cpu, args[0]))
}

fn co_task_mem_realloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(task_realloc(cpu, args[0], args[1]))
}

fn co_task_mem_free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    task_free(cpu, args[0]);
    Some(0)
}

/// CoGetMalloc(context, out): the process's task allocator.
pub(super) fn co_get_malloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [_, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let pid = super::user32::current_pid(cpu);
    let known = state(cpu)
        .mallocs
        .iter()
        .find(|&&(p, _)| p == pid)
        .map(|&(_, at)| at);
    let at = match known {
        Some(at) => at,
        None => {
            let Some(at) = com::new_interface(cpu, dll_index(), MALLOC, 0, 0) else {
                return Some(E_OUTOFMEMORY);
            };
            state(cpu).mallocs.push((pid, at));
            at
        }
    };
    if !com::write_out(cpu, out, at) {
        return Some(E_POINTER);
    }
    Some(S_OK)
}

fn malloc_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    match Guid::read(cpu, iid) {
        Some(g) if g == IID_IMALLOC || g == IID_IUNKNOWN => {
            com::write_out(cpu, out, this);
            Some(S_OK)
        }
        _ => {
            com::write_out(cpu, out, 0);
            Some(E_NOINTERFACE)
        }
    }
}

/// AddRef and Release: the allocator stays while the process runs.
fn malloc_add_ref(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(1)
}

fn malloc_alloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(task_alloc(cpu, args[1]))
}

fn malloc_realloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(task_realloc(cpu, args[1], args[2]))
}

fn malloc_free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    task_free(cpu, args[1]);
    Some(0)
}

/// GetSize: the block's size, or -1 for one that isn't the allocator's.
fn malloc_get_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(task_size(cpu, args[1]).unwrap_or(u32::MAX))
}

fn malloc_did_alloc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(task_size(cpu, args[1]).is_some() as u32)
}

fn malloc_heap_minimize(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// Write `text` with its 0 at `at` as UTF-16, its characters ASCII.
fn put_wide(cpu: &mut Cpu, at: u32, text: &str) {
    for (i, c) in text.bytes().chain([0]).enumerate() {
        cpu.bus.guest_write_16(at + 2 * i as u32, c as u16);
    }
}

/// StringFromGUID2(guid, buffer, characters): the characters written,
/// with the 0, or 0 if they don't fit.
fn string_from_guid2(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [guid, buffer, len] = args[..] else {
        return Some(0);
    };
    let Some(guid) = Guid::read(cpu, guid) else {
        return Some(0);
    };
    let text = guid.to_string();
    let n = text.len() as u32 + 1;
    if len < n || !cpu.bus.guest_probe(buffer, 2 * n as usize, true) {
        return Some(0);
    }
    put_wide(cpu, buffer, &text);
    Some(n)
}

/// StringFromCLSID and StringFromIID(guid, out): the string in a block
/// of the task allocator.
fn string_from_clsid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [guid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(guid) = Guid::read(cpu, guid) else {
        return Some(E_INVALIDARG);
    };
    let text = guid.to_string();
    let at = task_alloc(cpu, 2 * (text.len() as u32 + 1));
    if at == 0 {
        return Some(E_OUTOFMEMORY);
    }
    put_wide(cpu, at, &text);
    if !com::write_out(cpu, out, at) {
        task_free(cpu, at);
        return Some(E_POINTER);
    }
    Some(S_OK)
}

/// CO_E_CLASSSTRING: a string that isn't a GUID's.
const CO_E_CLASSSTRING: u32 = 0x8004_01F3;

/// CLSIDFromString and IIDFromString(string, out). A null string is
/// GUID_NULL.
fn clsid_from_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [string, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if string == 0 {
        return Some(if com::GUID_NULL.write(cpu, out) {
            S_OK
        } else {
            E_INVALIDARG
        });
    }
    let mut text = String::new();
    for i in 0..40u32 {
        let c = cpu.bus.guest_read_16(string + 2 * i);
        if c == 0 {
            break;
        }
        text.push(char::from_u32(c as u32).unwrap_or('?'));
    }
    let Some(guid) = Guid::parse(&text) else {
        return Some(CO_E_CLASSSTRING);
    };
    Some(if guid.write(cpu, out) {
        S_OK
    } else {
        E_INVALIDARG
    })
}

/// CLSIDFromProgID: no class has a ProgID.
fn clsid_from_prog_id(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(CO_E_CLASSSTRING)
}

fn is_equal_guid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let a = Guid::read(cpu, args[0]);
    let b = Guid::read(cpu, args[1]);
    Some((a.is_some() && a == b) as u32)
}

/// CoCreateGuid(out): a GUID of version 4, from the emulated clock, so
/// that runs are the same.
fn co_create_guid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let counter = &mut cpu.bus.win32.guids;
    *counter += 1;
    let seed = cpu.bus.clock.icount ^ (*counter as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut x = seed | 1;
    let mut bytes = [0; 16];
    for b in bytes.iter_mut() {
        // xorshift64*
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        *b = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 56) as u8;
    }
    bytes[7] = bytes[7] & 0x0F | 0x40;
    bytes[8] = bytes[8] & 0x3F | 0x80;
    Some(if Guid(bytes).write(cpu, args[0]) {
        S_OK
    } else {
        E_INVALIDARG
    })
}

fn co_get_current_process(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(super::user32::current_pid(cpu))
}

crate::state_fields!(State {
    initialized,
    mallocs,
    multithreaded
});
