//! COM's library, after Wine's dlls/ole32/compobj.c: apartments
//! (CoInitializeEx and its rules), class objects registered with
//! CoRegisterClassObject, the in-process servers the registry names
//! (HKEY_CLASSES_ROOT\CLSID\{clsid}\InprocServer32), ProgIDs, proxy/stub
//! classes, and marshalling between threads of one process.
//!
//! A thread's apartment is single-threaded (COINIT_APARTMENTTHREADED,
//! and CoInitialize's) or the process's multithreaded one; a thread that
//! didn't initialize COM may use it while another thread of its process
//! is in the multithreaded apartment.
//!
//! Marshalling stays in the process: CoMarshalInterface writes the
//! interface pointer itself to the stream, holding a reference, and
//! CoUnmarshalInterface reads it back and queries it for the interface
//! asked for. The threads share the object directly, with no proxy, so
//! calls aren't moved to the object's apartment.

use super::{CLSCTX_INPROC, calls, state};
use crate::cpu::Cpu;
use crate::win32::com::{
    self, CO_E_NOTINITIALIZED, E_INVALIDARG, E_OUTOFMEMORY, Guid, IID_ICLASSFACTORY,
    REGDB_E_CLASSNOTREG, S_FALSE, S_OK,
};
use crate::win32::registry::{self, HKEY_CLASSES_ROOT};
use crate::win32::task::{Next, Task};
use crate::win32::user32::{current_pid, current_tid};

const RPC_E_CHANGED_MODE: u32 = 0x8001_0106;
const RPC_E_WRONG_THREAD: u32 = 0x8001_010E;
const CO_E_NOT_SUPPORTED: u32 = 0x8000_4021;
const CO_E_CLASSSTRING: u32 = 0x8004_01F3;
const CO_E_DLLNOTFOUND: u32 = 0x8004_01F8;
const CO_E_ERRORINDLL: u32 = 0x8004_01F9;
const REGDB_E_IIDNOTREG: u32 = 0x8004_0155;
const STG_E_READFAULT: u32 = 0x8003_001E;

const COINIT_APARTMENTTHREADED: u32 = 0x2;
const CLSCTX_INPROC_SERVER: u32 = 0x1;
const CLSCTX_LOCAL_SERVER: u32 = 0x4;

/// The methods of IUnknown and the others called here, by their places
/// in the vtables.
const QUERY_INTERFACE: u32 = 0;
const ADD_REF: u32 = 1;
const RELEASE: u32 = 2;
const CREATE_INSTANCE: u32 = 3;
const STREAM_READ: u32 = 3;
const STREAM_WRITE: u32 = 4;
const STREAM_SEEK: u32 = 5;

/// A thread's apartment.
#[derive(Clone, Debug, Default)]
pub struct Apartment {
    tid: u32,
    pid: u32,
    /// CoInitialize calls not yet undone.
    count: u32,
    /// OleInitialize calls not yet undone.
    ole: u32,
    multithreaded: bool,
    /// The message filter CoRegisterMessageFilter set.
    filter: u32,
}

/// A class object CoRegisterClassObject registered.
#[derive(Clone, Debug, Default)]
pub struct Registered {
    cookie: u32,
    clsid: Vec<u8>,
    object: u32,
    context: u32,
    tid: u32,
    pid: u32,
}

/// A proxy/stub class CoRegisterPSClsid set for a thread's apartment.
#[derive(Clone, Debug, Default)]
pub struct PsClass {
    tid: u32,
    iid: Vec<u8>,
    clsid: Vec<u8>,
}

/// COM's state of the threads and processes.
#[derive(Clone, Debug, Default)]
pub struct Com {
    apartments: Vec<Apartment>,
    registered: Vec<Registered>,
    next_cookie: u32,
    ps_classes: Vec<PsClass>,
}

fn com_state(cpu: &mut Cpu) -> &mut Com {
    &mut state(cpu).com
}

/// A process ended: its threads' apartments and registrations go.
pub fn process_ended(cpu: &mut Cpu, pid: u32, tids: &[u32]) {
    let com = com_state(cpu);
    com.apartments.retain(|a| a.pid != pid);
    com.registered.retain(|r| r.pid != pid);
    com.ps_classes.retain(|p| !tids.contains(&p.tid));
}

fn apartment(cpu: &mut Cpu) -> Option<&mut Apartment> {
    let tid = current_tid(cpu);
    com_state(cpu).apartments.iter_mut().find(|a| a.tid == tid)
}

/// Whether the running thread may use COM: it initialized it, or its
/// process has a multithreaded apartment it belongs to.
pub fn initialized(cpu: &mut Cpu) -> bool {
    let (tid, pid) = (current_tid(cpu), current_pid(cpu));
    let apartments = &com_state(cpu).apartments;
    apartments.iter().any(|a| a.tid == tid)
        || apartments.iter().any(|a| a.pid == pid && a.multithreaded)
}

/// Whether the running thread is in its process's multithreaded
/// apartment: it joined it with CoInitializeEx, or it never initialized
/// COM and the process has one.
pub fn in_multithreaded_apartment(cpu: &mut Cpu) -> bool {
    let (tid, pid) = (current_tid(cpu), current_pid(cpu));
    let apartments = &com_state(cpu).apartments;
    match apartments.iter().find(|a| a.tid == tid) {
        Some(a) => a.multithreaded,
        None => apartments.iter().any(|a| a.pid == pid && a.multithreaded),
    }
}

/// CoInitializeEx for `model`: S_FALSE when the thread had initialized
/// COM already, RPC_E_CHANGED_MODE when it did for the other model.
fn initialize(cpu: &mut Cpu, model: u32) -> u32 {
    let multithreaded = model & COINIT_APARTMENTTHREADED == 0;
    let (tid, pid) = (current_tid(cpu), current_pid(cpu));
    if let Some(a) = apartment(cpu) {
        if a.multithreaded != multithreaded {
            return RPC_E_CHANGED_MODE;
        }
        a.count += 1;
        return S_FALSE;
    }
    com_state(cpu).apartments.push(Apartment {
        tid,
        pid,
        count: 1,
        multithreaded,
        ..Apartment::default()
    });
    S_OK
}

/// CoInitialize(reserved).
pub fn co_initialize(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(initialize(cpu, COINIT_APARTMENTTHREADED))
}

/// CoInitializeEx(reserved, model).
pub fn co_initialize_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(initialize(cpu, args[1]))
}

/// OleInitialize(reserved): COM for a single-threaded apartment, then
/// S_OK for the thread's first OleInitialize, S_FALSE after.
pub fn ole_initialize(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let result = initialize(cpu, COINIT_APARTMENTTHREADED);
    if result & 0x8000_0000 != 0 {
        return Some(result);
    }
    let Some(a) = apartment(cpu) else {
        return Some(result);
    };
    a.ole += 1;
    Some(if a.ole == 1 { S_OK } else { S_FALSE })
}

/// CoUninitialize: the last one ends the thread's apartment, revoking
/// the class objects registered in it.
pub fn co_uninitialize(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let tid = current_tid(cpu);
    let com = com_state(cpu);
    let Some(i) = com.apartments.iter().position(|a| a.tid == tid) else {
        return Some(0);
    };
    com.apartments[i].count -= 1;
    if com.apartments[i].count > 0 {
        return Some(0);
    }
    com.apartments.remove(i);
    com.ps_classes.retain(|p| p.tid != tid);
    let (revoked, kept) = std::mem::take(&mut com.registered)
        .into_iter()
        .partition(|r| r.tid == tid);
    com.registered = kept;
    release_all(cpu, 0, revoked.iter().map(|r| r.object))
}

/// OleUninitialize.
pub fn ole_uninitialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if let Some(a) = apartment(cpu) {
        a.ole = a.ole.saturating_sub(1);
    }
    co_uninitialize(cpu, args)
}

/// Call Release on each of `objects`, then return `value`.
fn release_all(cpu: &mut Cpu, value: u32, objects: impl Iterator<Item = u32>) -> Option<u32> {
    call_each(cpu, value, RELEASE, objects)
}

fn call_each(
    cpu: &mut Cpu,
    value: u32,
    method: u32,
    objects: impl Iterator<Item = u32>,
) -> Option<u32> {
    let mut vars = vec![calls::CALL_ALL, value];
    for object in objects {
        if let Some(m) = super::method(cpu, object, method) {
            vars.extend([m, object]);
        }
    }
    if vars.len() == 2 {
        return Some(value);
    }
    calls::start(cpu, vars, 0)
}

/// The string of the registry key `path` under HKEY_CLASSES_ROOT.
fn class_value(cpu: &mut Cpu, path: &str) -> Option<String> {
    registry::string_value(cpu, HKEY_CLASSES_ROOT, path, "")
}

fn wide_at(cpu: &mut Cpu, at: u32) -> String {
    let mut text = String::new();
    for i in 0..256u32 {
        let c = cpu.bus.guest_read_16(at + 2 * i);
        if c == 0 {
            break;
        }
        text.push(char::from_u32(c as u32).unwrap_or('?'));
    }
    text
}

/// The ProgID's CLSID, as HKEY_CLASSES_ROOT\<ProgID>\CLSID names it.
pub fn prog_id_class(cpu: &mut Cpu, prog_id: &str) -> Option<Guid> {
    Guid::parse(&class_value(cpu, &format!("{}\\CLSID", prog_id))?)
}

/// ProgIDFromCLSID(clsid, out): the class's ProgID in a block of the
/// task allocator.
pub fn prog_id_from_clsid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [clsid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    com::write_out(cpu, out, 0);
    let Some(clsid) = Guid::read(cpu, clsid) else {
        return Some(E_INVALIDARG);
    };
    let Some(prog_id) = class_value(cpu, &format!("CLSID\\{}\\ProgID", clsid)) else {
        return Some(REGDB_E_CLASSNOTREG);
    };
    let at = super::task_alloc(cpu, 2 * (prog_id.encode_utf16().count() as u32 + 1));
    if at == 0 {
        return Some(E_OUTOFMEMORY);
    }
    for (i, c) in prog_id.encode_utf16().chain([0]).enumerate() {
        cpu.bus.guest_write_16(at + 2 * i as u32, c);
    }
    com::write_out(cpu, out, at);
    Some(S_OK)
}

/// CLSIDFromProgID(ProgID, out): GUID_NULL and CO_E_CLASSSTRING for a
/// ProgID the registry doesn't have.
pub fn clsid_from_prog_id(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [prog_id, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if prog_id == 0 || out == 0 || !cpu.bus.guest_probe(out, 16, true) {
        return Some(E_INVALIDARG);
    }
    let prog_id = wide_at(cpu, prog_id);
    match prog_id_class(cpu, &prog_id) {
        Some(clsid) => {
            clsid.write(cpu, out);
            Some(S_OK)
        }
        None => {
            com::GUID_NULL.write(cpu, out);
            Some(CO_E_CLASSSTRING)
        }
    }
}

/// CoGetTreatAsClass(clsid, out): the class the registry's TreatAs
/// names, or S_FALSE and the class itself.
pub fn co_get_treat_as_class(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [clsid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(clsid) = Guid::read(cpu, clsid) else {
        return Some(E_INVALIDARG);
    };
    if !cpu.bus.guest_probe(out, 16, true) {
        return Some(E_INVALIDARG);
    }
    let treat_as = class_value(cpu, &format!("CLSID\\{}\\TreatAs", clsid));
    match treat_as.as_deref().and_then(Guid::parse) {
        Some(other) => {
            other.write(cpu, out);
            Some(S_OK)
        }
        None => {
            clsid.write(cpu, out);
            Some(S_FALSE)
        }
    }
}

/// CoRegisterPSClsid(iid, clsid): the thread's apartment uses `clsid`
/// as the proxy/stub class of `iid`.
pub fn co_register_ps_clsid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [iid, clsid] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let (Some(iid), Some(clsid)) = (Guid::read(cpu, iid), Guid::read(cpu, clsid)) else {
        return Some(E_INVALIDARG);
    };
    if apartment(cpu).is_none() {
        return Some(CO_E_NOTINITIALIZED);
    }
    let tid = current_tid(cpu);
    let classes = &mut com_state(cpu).ps_classes;
    classes.retain(|p| !(p.tid == tid && p.iid == iid.0));
    classes.push(PsClass {
        tid,
        iid: iid.0.to_vec(),
        clsid: clsid.0.to_vec(),
    });
    Some(S_OK)
}

/// CoGetPSClsid(iid, out): the proxy/stub class of `iid`: the one
/// CoRegisterPSClsid set, or the registry's
/// Interface\{iid}\ProxyStubClsid32.
pub fn co_get_ps_clsid(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [iid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if !initialized(cpu) {
        return Some(CO_E_NOTINITIALIZED);
    }
    let Some(iid) = Guid::read(cpu, iid) else {
        return Some(E_INVALIDARG);
    };
    if out == 0 || !cpu.bus.guest_probe(out, 16, true) {
        return Some(E_INVALIDARG);
    }
    let tid = current_tid(cpu);
    let set = com_state(cpu)
        .ps_classes
        .iter()
        .find(|p| p.tid == tid && p.iid == iid.0)
        .and_then(|p| <[u8; 16]>::try_from(p.clsid.as_slice()).ok());
    let clsid = match set {
        Some(bytes) => Guid(bytes),
        None => {
            let path = format!("Interface\\{}\\ProxyStubClsid32", iid);
            match class_value(cpu, &path).as_deref().and_then(Guid::parse) {
                Some(clsid) => clsid,
                None => return Some(REGDB_E_IIDNOTREG),
            }
        }
    };
    clsid.write(cpu, out);
    Some(S_OK)
}

/// CoRegisterMessageFilter(filter, previous): a single-threaded
/// apartment's filter. The filter isn't called: there are no calls
/// between apartments to filter.
pub fn co_register_message_filter(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [filter, previous] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(a) = apartment(cpu) else {
        return Some(CO_E_NOT_SUPPORTED);
    };
    if a.multithreaded {
        return Some(CO_E_NOT_SUPPORTED);
    }
    let old = std::mem::replace(&mut a.filter, filter);
    if previous != 0 {
        com::write_out(cpu, previous, old);
    }
    Some(S_OK)
}

/// CoRegisterClassObject(clsid, object, context, flags, cookie): COM
/// holds a reference to the object until CoRevokeClassObject or the
/// apartment's end.
pub fn co_register_class_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [clsid, object, context, _, cookie_at] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if object == 0 || cookie_at == 0 {
        return Some(E_INVALIDARG);
    }
    let Some(clsid) = Guid::read(cpu, clsid) else {
        return Some(E_INVALIDARG);
    };
    if !initialized(cpu) {
        return Some(CO_E_NOTINITIALIZED);
    }
    let (tid, pid) = (current_tid(cpu), current_pid(cpu));
    let com = com_state(cpu);
    com.next_cookie += 1;
    let cookie = com.next_cookie;
    com.registered.push(Registered {
        cookie,
        clsid: clsid.0.to_vec(),
        object,
        context,
        tid,
        pid,
    });
    com::write_out(cpu, cookie_at, cookie);
    call_each(cpu, S_OK, ADD_REF, [object].into_iter())
}

/// CoRevokeClassObject(cookie): only from the thread that registered
/// it.
pub fn co_revoke_class_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let cookie = args[0];
    let (tid, pid) = (current_tid(cpu), current_pid(cpu));
    let com = com_state(cpu);
    let Some(i) = com
        .registered
        .iter()
        .position(|r| r.cookie == cookie && r.pid == pid)
    else {
        return Some(E_INVALIDARG);
    };
    if com.registered[i].tid != tid {
        return Some(RPC_E_WRONG_THREAD);
    }
    let gone = com.registered.remove(i);
    release_all(cpu, S_OK, [gone.object].into_iter())
}

/// Where a class's objects come from.
pub enum Source {
    /// A class object registered with CoRegisterClassObject.
    Registered(u32),
    /// A class of a built-in DLL (com.rs), by its place.
    Builtin(usize),
    /// A DLL the registry names as the class's in-process server.
    Server,
}

/// Where the objects of `clsid` come from for `context`, for the
/// running thread.
pub fn source(cpu: &mut Cpu, clsid: &Guid, context: u32) -> Option<Source> {
    let (tid, pid) = (current_tid(cpu), current_pid(cpu));
    let found = com_state(cpu).registered.iter().find(|r| {
        r.clsid == clsid.0
            && r.pid == pid
            && ((r.context & context & CLSCTX_INPROC_SERVER != 0 && r.tid == tid)
                || r.context & context & CLSCTX_LOCAL_SERVER != 0)
    });
    if let Some(r) = found {
        return Some(Source::Registered(r.object));
    }
    if context & CLSCTX_INPROC != 0
        && let Some((index, _)) = com::find_class(clsid)
    {
        return Some(Source::Builtin(index));
    }
    if context & CLSCTX_INPROC_SERVER != 0 {
        let path = format!("CLSID\\{}\\InprocServer32", clsid);
        if class_value(cpu, &path).is_some_and(|d| !d.is_empty()) {
            return Some(Source::Server);
        }
    }
    None
}

// The task that gets a class object from a registered object or a DLL,
// and for CoCreateInstance makes an object with it.
const G_CREATE: usize = 1;
const G_CLSID: usize = 2;
const G_IID: usize = 3;
const G_OUT: usize = 4;
const G_OUTER: usize = 5;
const G_OBJECT: usize = 6;
const G_RESULT: usize = 7;
/// Its scratch space: the factory, IID_IClassFactory, then a string.
const S_FACTORY: u32 = 0;
const S_IID: u32 = 4;
const S_TEXT: u32 = 20;
pub const CLASS_SCRATCH: u32 = 20 + 264;

/// Start the task: the class object `object` (registered), or else the
/// DLL `server`'s, as `iid` at `out`; with `create`, an object of the
/// class made by it, aggregated by `outer`.
pub fn start_class(
    cpu: &mut Cpu,
    create: bool,
    clsid: u32,
    iid: u32,
    out: u32,
    outer: u32,
    object: u32,
) -> Option<u32> {
    let vars = vec![calls::CLASS, create as u32, clsid, iid, out, outer, object];
    calls::start(cpu, vars, CLASS_SCRATCH)
}

fn put_text(cpu: &mut Cpu, at: u32, text: &str) {
    let mut bytes = crate::dosstr::to_bytes(text);
    bytes.truncate(259);
    bytes.push(0);
    cpu.bus.guest_write_bytes(at, &bytes);
}

/// The class task's steps.
pub fn class_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let create = t.var(G_CREATE) != 0;
    let s = t.scratch;
    // What the class object comes as, and where it goes.
    let (iid, out) = if create {
        (s + S_IID, s + S_FACTORY)
    } else {
        (t.var(G_IID), t.var(G_OUT))
    };
    match t.step {
        0 => {
            IID_ICLASSFACTORY.write(cpu, s + S_IID);
            let object = t.var(G_OBJECT);
            if object != 0 {
                let Some(qi) = super::method(cpu, object, QUERY_INTERFACE) else {
                    return Next::Done(E_INVALIDARG);
                };
                return Next::call(qi, &[object, iid, out], 10);
            }
            let Some(clsid) = Guid::read(cpu, t.var(G_CLSID)) else {
                return Next::Done(E_INVALIDARG);
            };
            let path = format!("CLSID\\{}\\InprocServer32", clsid);
            let Some(dll) = class_value(cpu, &path) else {
                return Next::Done(REGDB_E_CLASSNOTREG);
            };
            put_text(cpu, s + S_TEXT, &dll);
            let load = crate::win32::kernel32::address_of("LoadLibraryA");
            Next::call(load, &[s + S_TEXT], 1)
        }
        1 => {
            if value == 0 {
                return Next::Done(CO_E_DLLNOTFOUND);
            }
            put_text(cpu, s + S_TEXT, "DllGetClassObject");
            let find = crate::win32::kernel32::address_of("GetProcAddress");
            Next::call(find, &[value, s + S_TEXT], 2)
        }
        2 => {
            if value == 0 {
                return Next::Done(CO_E_ERRORINDLL);
            }
            Next::call(value, &[t.var(G_CLSID), iid, out], 10)
        }
        10 => {
            if !create || value & 0x8000_0000 != 0 {
                return Next::Done(value);
            }
            let factory = cpu.bus.guest_read_32(s + S_FACTORY);
            let Some(method) = super::method(cpu, factory, CREATE_INSTANCE) else {
                return Next::Done(E_INVALIDARG);
            };
            let args = [factory, t.var(G_OUTER), t.var(G_IID), t.var(G_OUT)];
            Next::call(method, &args, 11)
        }
        11 => {
            t.set(G_RESULT, value);
            let factory = cpu.bus.guest_read_32(s + S_FACTORY);
            match super::method(cpu, factory, RELEASE) {
                Some(release) => Next::call(release, &[factory], 12),
                None => Next::Done(value),
            }
        }
        _ => Next::Done(t.var(G_RESULT)),
    }
}

// Marshalling. The data in the stream: "MEOW" (an OBJREF's signature),
// 1, the IID, the interface pointer and the marshalling flags.
const RECORD: u32 = 32;
const SIGNATURE: u32 = 0x574F_454D;
const R_IID: u32 = 8;
const R_POINTER: u32 = 24;
const R_FLAGS: u32 = 28;
const MSHLFLAGS_NORMAL: u32 = 0;

const M_STREAM: usize = 1;
const M_IID: usize = 2;
const M_OBJECT: usize = 3;
const M_FLAGS: usize = 4;
/// Whether the stream goes back to its start afterwards (for
/// CoMarshalInterThreadInterfaceInStream).
const M_REWIND: usize = 5;
const M_RESULT: usize = 6;
/// The scratch space: the record, the pointer QueryInterface gives, the
/// proxy/stub factory, the stub it makes and IID_IPSFactoryBuffer.
const S_POINTER: u32 = RECORD;
const S_PS_FACTORY: u32 = RECORD + 4;
const S_STUB: u32 = RECORD + 8;
const S_PS_IID: u32 = RECORD + 12;
pub const MARSHAL_SCRATCH: u32 = RECORD + 28;

const IID_IPSFACTORYBUFFER: Guid = Guid::new(
    0xD5F5_69D0,
    0x593B,
    0x101A,
    [0xB5, 0x69, 0x08, 0x00, 0x2B, 0x2D, 0xBF, 0x7A],
);
/// IPSFactoryBuffer::CreateStub's place in its vtable.
const CREATE_STUB: u32 = 4;

/// The object registered in this thread as the class CoRegisterPSClsid
/// set for `iid`, if one is.
fn ps_factory(cpu: &mut Cpu, iid: &Guid) -> Option<u32> {
    let (tid, pid) = (current_tid(cpu), current_pid(cpu));
    let com = com_state(cpu);
    let class = com
        .ps_classes
        .iter()
        .find(|p| p.tid == tid && p.iid == iid.0)?;
    let found = com
        .registered
        .iter()
        .find(|r| r.clsid == class.clsid && r.pid == pid && r.tid == tid)?;
    Some(found.object)
}

/// CoMarshalInterface(stream, iid, object, context, reserved, flags).
pub fn co_marshal_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [stream, iid, object, _, _, flags] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if stream == 0 || object == 0 || Guid::read(cpu, iid).is_none() {
        return Some(E_INVALIDARG);
    }
    let vars = vec![calls::MARSHAL, stream, iid, object, flags, 0];
    calls::start(cpu, vars, MARSHAL_SCRATCH)
}

/// CoMarshalInterThreadInterfaceInStream(iid, object, out): a new
/// stream on global memory with the interface marshalled in it, at its
/// start.
pub fn co_marshal_inter_thread_interface_in_stream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [iid, object, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if object == 0 || out == 0 || Guid::read(cpu, iid).is_none() {
        return Some(E_INVALIDARG);
    }
    let made = super::stream::create_stream_on_hglobal(cpu, &[0, 1, out])?;
    if made != S_OK {
        return Some(made);
    }
    let stream = cpu.bus.guest_read_32(out);
    let vars = vec![calls::MARSHAL, stream, iid, object, MSHLFLAGS_NORMAL, 1];
    calls::start(cpu, vars, MARSHAL_SCRATCH)
}

/// The marshalling task's steps.
pub fn marshal_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let s = t.scratch;
    let stream = t.var(M_STREAM);
    match t.step {
        0 => {
            let object = t.var(M_OBJECT);
            let Some(qi) = super::method(cpu, object, QUERY_INTERFACE) else {
                return Next::Done(E_INVALIDARG);
            };
            Next::call(qi, &[object, t.var(M_IID), s + S_POINTER], 1)
        }
        1 => {
            if value & 0x8000_0000 != 0 {
                return Next::Done(value);
            }
            // A proxy/stub class the program registered for the
            // interface makes its stub, which may fail the marshalling.
            let iid = Guid::read(cpu, t.var(M_IID)).unwrap_or_default();
            let Some(factory) = ps_factory(cpu, &iid) else {
                return write_record(cpu, t);
            };
            let Some(qi) = super::method(cpu, factory, QUERY_INTERFACE) else {
                return write_record(cpu, t);
            };
            IID_IPSFACTORYBUFFER.write(cpu, s + S_PS_IID);
            Next::call(qi, &[factory, s + S_PS_IID, s + S_PS_FACTORY], 4)
        }
        4 => {
            let factory = cpu.bus.guest_read_32(s + S_PS_FACTORY);
            if value & 0x8000_0000 != 0 {
                return write_record(cpu, t);
            }
            let Some(create) = super::method(cpu, factory, CREATE_STUB) else {
                return write_record(cpu, t);
            };
            cpu.bus.guest_write_32(s + S_STUB, 0);
            let pointer = cpu.bus.guest_read_32(s + S_POINTER);
            Next::call(create, &[factory, t.var(M_IID), pointer, s + S_STUB], 5)
        }
        5 => {
            t.set(M_RESULT, value);
            let factory = cpu.bus.guest_read_32(s + S_PS_FACTORY);
            match super::method(cpu, factory, RELEASE) {
                Some(release) => Next::call(release, &[factory], 6),
                None => Next::call(0, &[], 6),
            }
        }
        6 => {
            let failed = t.var(M_RESULT) & 0x8000_0000 != 0;
            let object = cpu
                .bus
                .guest_read_32(s + if failed { S_POINTER } else { S_STUB });
            match super::method(cpu, object, RELEASE) {
                Some(release) if object != 0 => Next::call(release, &[object], 7),
                _ => Next::call(0, &[], 7),
            }
        }
        7 => match t.var(M_RESULT) {
            result if result & 0x8000_0000 != 0 => Next::Done(result),
            _ => write_record(cpu, t),
        },
        2 => {
            if value & 0x8000_0000 != 0 || t.var(M_REWIND) == 0 {
                return Next::Done(value);
            }
            let Some(seek) = super::method(cpu, stream, STREAM_SEEK) else {
                return Next::Done(S_OK);
            };
            // Seek(this, 0 as a LARGE_INTEGER, STREAM_SEEK_SET, NULL).
            Next::call(seek, &[stream, 0, 0, 0, 0], 3)
        }
        _ => Next::Done(S_OK),
    }
}

/// Write the marshalling record to the stream, the task's step 2 next.
fn write_record(cpu: &mut Cpu, t: &mut Task) -> Next {
    let s = t.scratch;
    let stream = t.var(M_STREAM);
    let iid = Guid::read(cpu, t.var(M_IID)).unwrap_or_default();
    let pointer = cpu.bus.guest_read_32(s + S_POINTER);
    cpu.bus.guest_write_32(s, SIGNATURE);
    cpu.bus.guest_write_32(s + 4, 1);
    cpu.bus.guest_write_bytes(s + R_IID, &iid.0);
    cpu.bus.guest_write_32(s + R_POINTER, pointer);
    cpu.bus.guest_write_32(s + R_FLAGS, t.var(M_FLAGS));
    let Some(write) = super::method(cpu, stream, STREAM_WRITE) else {
        return Next::Done(E_INVALIDARG);
    };
    Next::call(write, &[stream, s, RECORD, 0], 2)
}

const U_STREAM: usize = 1;
const U_IID: usize = 2;
const U_OUT: usize = 3;
/// What the task does with the data: 0 unmarshal it, 1 release it.
const U_MODE: usize = 4;
/// Whether the stream is released afterwards.
const U_RELEASE_STREAM: usize = 5;
const U_POINTER: usize = 6;
const U_RESULT: usize = 7;
const MODE_UNMARSHAL: u32 = 0;
const MODE_RELEASE: u32 = 1;
const S_READ: u32 = RECORD;

/// CoUnmarshalInterface(stream, iid, out).
pub fn co_unmarshal_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [stream, iid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if stream == 0 || out == 0 {
        return Some(E_INVALIDARG);
    }
    if !initialized(cpu) {
        return Some(CO_E_NOTINITIALIZED);
    }
    let vars = vec![calls::UNMARSHAL, stream, iid, out, MODE_UNMARSHAL, 0];
    calls::start(cpu, vars, MARSHAL_SCRATCH)
}

/// CoGetInterfaceAndReleaseStream(stream, iid, out).
pub fn co_get_interface_and_release_stream(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [stream, iid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if stream == 0 || out == 0 {
        return Some(E_INVALIDARG);
    }
    let vars = vec![calls::UNMARSHAL, stream, iid, out, MODE_UNMARSHAL, 1];
    calls::start(cpu, vars, MARSHAL_SCRATCH)
}

/// CoReleaseMarshalData(stream): the reference the data holds goes.
pub fn co_release_marshal_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let stream = args[0];
    if stream == 0 {
        return Some(E_INVALIDARG);
    }
    let vars = vec![calls::UNMARSHAL, stream, 0, 0, MODE_RELEASE, 0];
    calls::start(cpu, vars, MARSHAL_SCRATCH)
}

/// The unmarshalling task's steps.
pub fn unmarshal_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let s = t.scratch;
    let stream = t.var(U_STREAM);
    let release = |cpu: &mut Cpu, object: u32, step: u16| match super::method(cpu, object, RELEASE)
    {
        Some(method) => Next::call(method, &[object], step),
        None => Next::call(0, &[], step),
    };
    match t.step {
        0 => {
            if t.var(U_OUT) != 0 {
                com::write_out(cpu, t.var(U_OUT), 0);
            }
            let Some(read) = super::method(cpu, stream, STREAM_READ) else {
                return Next::Done(E_INVALIDARG);
            };
            cpu.bus.guest_write_32(s + S_READ, 0);
            Next::call(read, &[stream, s, RECORD, s + S_READ], 1)
        }
        1 => {
            let read = cpu.bus.guest_read_32(s + S_READ);
            if value & 0x8000_0000 != 0 || read < RECORD {
                return Next::Done(STG_E_READFAULT);
            }
            if cpu.bus.guest_read_32(s) != SIGNATURE {
                return Next::Done(E_INVALIDARG);
            }
            let pointer = cpu.bus.guest_read_32(s + R_POINTER);
            t.set(U_POINTER, pointer);
            if t.var(U_MODE) == MODE_RELEASE {
                t.set(U_RESULT, S_OK);
                return release(cpu, pointer, 4);
            }
            let Some(qi) = super::method(cpu, pointer, QUERY_INTERFACE) else {
                return Next::Done(E_INVALIDARG);
            };
            Next::call(qi, &[pointer, t.var(U_IID), t.var(U_OUT)], 2)
        }
        2 => {
            t.set(U_RESULT, value);
            // Normal marshalling is for one unmarshal, which takes the
            // data's reference.
            if cpu.bus.guest_read_32(s + R_FLAGS) == MSHLFLAGS_NORMAL {
                return release(cpu, t.var(U_POINTER), 3);
            }
            Next::call(0, &[], 3)
        }
        3 => {
            if t.var(U_RELEASE_STREAM) != 0 {
                return release(cpu, stream, 4);
            }
            Next::Done(t.var(U_RESULT))
        }
        _ => Next::Done(t.var(U_RESULT)),
    }
}

/// OleSetMenuDescriptor(menu, frame, active, frame object, active
/// object): S_OK; the menu's commands aren't sent to the object.
pub fn ole_set_menu_descriptor(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(S_OK)
}

crate::state_fields!(Apartment {
    tid,
    pid,
    count,
    ole,
    multithreaded,
    filter
});
crate::state_fields!(Registered {
    cookie,
    clsid,
    object,
    context,
    tid,
    pid
});
crate::state_fields!(PsClass { tid, iid, clsid });
crate::state_fields!(Com {
    apartments,
    registered,
    next_cookie,
    ps_classes
});
