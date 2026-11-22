//! RPCRT4's side of COM between processes: the proxy/stub DLLs MIDL
//! makes for interfaces no type library describes (InstallShield's
//! OBJECTPS.DLL, for one). Such a DLL's DllGetClassObject gives
//! NdrDllGetClassObject its tables (`ProxyFileInfo`): for each interface
//! a proxy vtable whose methods are code of the DLL's own, and a stub
//! vtable with a stub function for each method. The code marshals the
//! arguments into a buffer in NDR, the Network Data Representation,
//! calling RPCRT4's functions for the types it doesn't copy itself
//! (NdrPointerMarshall and the rest), and sends the buffer with
//! NdrProxySendReceive. The object's side gives the buffer to the stub
//! function, which unmarshals the arguments, calls the object, and
//! marshals what it gave back in a buffer of NdrStubGetBuffer's.
//!
//! Here the buffers go between the processes as rpc.rs's calls do: the
//! proxy's NdrProxySendReceive sends the bytes to the apartment window
//! of the object's thread, whose stub task (`stub_step`) calls the stub
//! vtable's Invoke, and the reply's bytes come back into a buffer of the
//! caller's. The interface pointers in a buffer go as rpc.rs's do, as
//! the ids of their exports: in NDR a conformant array of bytes, its
//! length and then the bytes. Both ends are this RPCRT4, so only the
//! sizes the code reserves have to hold, not Windows' OBJREF.
//!
//! The DLLs MIDL made with -Os, whose proxies and stubs are compiled
//! code, are what this runs: the types are those such a DLL hands
//! RPCRT4, base types, pointers to them, structures and fixed arrays
//! without pointers, and interface pointers. A DLL made with -Oicf
//! (NdrClientCall2, NdrStubCall2) has interpreted proxies, which this
//! doesn't run.
//!
//! The DLL a process needs is loaded the first time one of its
//! interfaces comes into the process or is called there: COM gets its
//! class object (CoGetClassObject of the interface's ProxyStubClsid32,
//! CLSCTX_INPROC_SERVER, IPSFactoryBuffer), as Windows does, and keeps
//! it while the process lives. A function that needs one first loads it
//! (`load`), then runs again from its start.

use super::{
    E_NOTIMPL, Pending, RPC_E_DISCONNECTED, RPC_E_SERVER_DIED, Reader, Side, WM_OLE_CALL, Wire,
    drop_iface, export, get_iface, guid_of, next_id, proxy_at, put_iface, read_bytes, read32,
    reply, rpc, write_bytes, write32,
};
use crate::cpu::Cpu;
use crate::win32::com::{
    self, CLASS_E_CLASSNOTAVAILABLE, E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, Guid,
    IID_IUNKNOWN, S_FALSE, S_OK,
};
use crate::win32::ole32::{calls, compobj, task_alloc, task_free};
use crate::win32::registry::{self, HKEY_CLASSES_ROOT, REG_SZ};
use crate::win32::task::{Next, Task};
use crate::win32::user32::{self, current_pid};

/// A process's proxy/stub class object, once COM got it: (process,
/// class, the object or 0 if there was none to be had).
#[derive(Clone, Debug, Default)]
pub struct PsFactory {
    pid: u32,
    clsid: Vec<u8>,
    object: u32,
}

/// A buffer a stub task gave a stub function: the arguments it
/// unmarshalled may point into it, which NdrPointerFree mustn't free.
#[derive(Clone, Debug, Default)]
pub struct StubBuffer {
    pid: u32,
    start: u32,
    end: u32,
}

/// The bit of a call's method index that says its arguments are NDR.
pub const NDR_CALL: u32 = 0x8000_0000;

// MIDL_STUB_MESSAGE's fields.
const SM_RPC_MSG: u32 = 0x00;
const SM_BUFFER: u32 = 0x04;
const SM_BUFFER_START: u32 = 0x08;
const SM_BUFFER_END: u32 = 0x0C;
const SM_BUFFER_LENGTH: u32 = 0x14;
const SM_IS_CLIENT: u32 = 0x20;
const SM_MAX_COUNT: u32 = 0x3C;
const SM_ALLOCATE: u32 = 0x48;
const SM_FREE: u32 = 0x4C;
const SM_STACK_TOP: u32 = 0x50;
const SM_STUB_DESC: u32 = 0x60;
const SM_CHANNEL: u32 = 0x84;
/// The fields NdrProxyInitialize and NdrStubInitialize set: those every
/// MIDL's MIDL_STUB_MESSAGE has, to pArrayInfo.
const SM_INITIALIZED: u32 = 0x8C;

// RPC_MESSAGE's fields (and RPCOLEMESSAGE's, which has the same).
const RM_DATA_REPRESENTATION: u32 = 0x04;
const RM_BUFFER: u32 = 0x08;
const RM_BUFFER_LENGTH: u32 = 0x0C;
const RM_PROC_NUM: u32 = 0x10;
const RM_INTERFACE: u32 = 0x18;
const RPC_MESSAGE_SIZE: u32 = 0x2C;
/// Little-endian integers, ASCII, IEEE floats: what every caller has.
const NDR_LOCAL_DATA_REPRESENTATION: u32 = 0x10;

// MIDL_STUB_DESC's fields.
const SD_INTERFACE: u32 = 0x00;
const SD_ALLOCATE: u32 = 0x04;
const SD_FREE: u32 = 0x08;

// The proxy/stub tables. ProxyFileInfo: the proxy vtables, the stub
// vtables, the names, then the count and the tables' version.
const PF_PROXIES: u32 = 0x00;
const PF_STUBS: u32 = 0x04;
const PF_NAMES: u32 = 0x08;
const PF_TABLE_SIZE: u32 = 0x14;
const PF_TABLE_VERSION: u32 = 0x16;
/// CInterfaceStubVtbl: the IID, the server information, the count of
/// methods and the stub functions (from IUnknown's places on), then
/// IRpcStubBuffer's vtable.
const SH_IID: u32 = 0x00;
const SH_METHODS: u32 = 0x08;
const SH_DISPATCH: u32 = 0x0C;
const SH_VTABLE: u32 = 0x10;
/// CStdStubBuffer: the vtable, the count of references, the object.
const SB_REFS: u32 = 0x04;
const SB_OBJECT: u32 = 0x08;
const STUB_BUFFER_SIZE: u32 = 0x0C;
/// CStdPSFactoryBuffer: the vtable, the count of references, the
/// tables.
const PSF_REFS: u32 = 0x04;
const PSF_FILES: u32 = 0x08;
/// IRpcStubBuffer::Invoke's place in its vtable.
const INVOKE: u32 = 5;
/// The most tables and interfaces a DLL's tables are read for.
const MAX_FILES: u32 = 256;
const MAX_INTERFACES: u32 = 1024;

// The format characters.
const FC_BYTE: u8 = 0x01;
const FC_CHAR: u8 = 0x02;
const FC_SMALL: u8 = 0x03;
const FC_USMALL: u8 = 0x04;
const FC_WCHAR: u8 = 0x05;
const FC_SHORT: u8 = 0x06;
const FC_USHORT: u8 = 0x07;
const FC_LONG: u8 = 0x08;
const FC_ULONG: u8 = 0x09;
const FC_FLOAT: u8 = 0x0A;
const FC_HYPER: u8 = 0x0B;
const FC_DOUBLE: u8 = 0x0C;
const FC_ENUM16: u8 = 0x0D;
const FC_ENUM32: u8 = 0x0E;
const FC_ERROR_STATUS_T: u8 = 0x10;
const FC_RP: u8 = 0x11;
const FC_UP: u8 = 0x12;
const FC_OP: u8 = 0x13;
const FC_STRUCT: u8 = 0x15;
const FC_SMFARRAY: u8 = 0x1D;
const FC_LGFARRAY: u8 = 0x1E;
const FC_IP: u8 = 0x2F;
const FC_PP: u8 = 0x4B;
const FC_CONSTANT_IID: u8 = 0x5A;
const FC_INT3264: u8 = 0xB7;
const FC_UINT3264: u8 = 0xB8;
// A pointer's attributes.
const FC_DONT_FREE: u8 = 0x02;
const FC_ALLOCED_ON_STACK: u8 = 0x04;
const FC_SIMPLE_POINTER: u8 = 0x08;
const FC_POINTER_DEREF: u8 = 0x10;
/// A correlation descriptor's kind: a parameter of the method.
const FC_TOP_LEVEL_CONFORMANCE: u8 = 0x20;

/// RpcRaiseException's codes.
const RPC_X_NULL_REF_POINTER: u32 = 1780;
const RPC_X_BAD_STUB_DATA: u32 = 1783;
const EXCEPTION_NONCONTINUABLE: u32 = 1;

/// IRpcStubBuffer's IID.
const IID_IRPCSTUBBUFFER: Guid = Guid::new(
    0xD5F5_6AFC,
    0x593B,
    0x101A,
    [0xB5, 0x69, 0x08, 0x00, 0x2B, 0x2D, 0xBF, 0x7A],
);

/// An interface a proxy/stub DLL of the running process marshals.
#[derive(Clone, Copy, Debug)]
pub struct Interface {
    /// Its proxies' vtable: IUnknown's methods, then the interface's.
    pub proxy_vtable: u32,
    /// Its stub vtable (CInterfaceStubVtbl).
    stub: u32,
}

impl Interface {
    /// IRpcStubBuffer's vtable of its stubs.
    fn stub_vtable(&self) -> u32 {
        self.stub + SH_VTABLE
    }
}

/// The interfaces in the tables at `files` (a ProxyFileInfo list), each
/// with its proxy vtable, stub vtable and name.
fn interfaces(cpu: &mut Cpu, files: u32) -> Vec<(Guid, Interface, u32)> {
    let mut found = Vec::new();
    for f in 0..MAX_FILES {
        let info = read32(cpu, files + 4 * f);
        if info == 0 {
            break;
        }
        let proxies = read32(cpu, info + PF_PROXIES);
        let stubs = read32(cpu, info + PF_STUBS);
        let names = read32(cpu, info + PF_NAMES);
        let count = cpu.bus.guest_read_16(info + PF_TABLE_SIZE) as u32;
        // From version 2 a proxy vtable's header has the stubless
        // proxies' information before the IID.
        let version = cpu.bus.guest_read_16(info + PF_TABLE_VERSION);
        let header = if version > 1 { 8 } else { 4 };
        for i in 0..count.min(MAX_INTERFACES) {
            let proxy = read32(cpu, proxies + 4 * i);
            let stub = read32(cpu, stubs + 4 * i);
            let name = if names == 0 {
                0
            } else {
                read32(cpu, names + 4 * i)
            };
            let piid = read32(cpu, proxy + header - 4);
            if let Some(iid) = Guid::read(cpu, piid) {
                let proxy_vtable = proxy + header;
                found.push((iid, Interface { proxy_vtable, stub }, name));
            }
        }
    }
    found
}

/// The proxy/stub class whose DLL marshals `iid`, if it is a DLL of the
/// program's: the built-in DLLs' interfaces (OLE32's, OLE Automation's
/// type library marshaler) go as rpc.rs copies them.
fn program_ps_class(cpu: &mut Cpu, iid: &Guid) -> Option<Guid> {
    let clsid = compobj::ps_clsid(cpu, iid)?;
    let path = format!("CLSID\\{}\\InprocServer32", clsid);
    let dll = compobj::class_value(cpu, &path)?;
    let name = dll.rsplit(['\\', '/', ':']).next().unwrap_or(&dll);
    if crate::win32::stubs::find_dll(name).is_some() {
        return None;
    }
    Some(clsid)
}

/// The running process's interface `iid`, if a proxy/stub DLL of the
/// program's it loaded marshals it.
pub fn interface(cpu: &mut Cpu, iid: &Guid) -> Option<Interface> {
    let clsid = program_ps_class(cpu, iid)?;
    let pid = current_pid(cpu);
    let object = rpc(cpu)
        .ps
        .iter()
        .find(|f| f.pid == pid && f.clsid == clsid.0)
        .map(|f| f.object)
        .filter(|&o| o != 0)?;
    let files = read32(cpu, object + PSF_FILES);
    interfaces(cpu, files)
        .into_iter()
        .find(|(i, _, _)| i == iid)
        .map(|(_, interface, _)| interface)
}

/// The proxy/stub classes the running process has yet to load for
/// `iids`, each once.
fn to_load(cpu: &mut Cpu, iids: &[Guid]) -> Vec<Guid> {
    let pid = current_pid(cpu);
    let mut classes: Vec<Guid> = Vec::new();
    for iid in iids {
        let Some(clsid) = program_ps_class(cpu, iid) else {
            continue;
        };
        let loaded = rpc(cpu)
            .ps
            .iter()
            .any(|f| f.pid == pid && f.clsid == clsid.0);
        if !loaded && !classes.contains(&clsid) {
            classes.push(clsid);
        }
    }
    classes
}

/// Load the proxy/stub DLLs `iids` need that the running process hasn't
/// loaded, then run the function the running stub is of again from its
/// start. None when there is nothing to load; otherwise what the
/// function's handler returns.
pub fn load(cpu: &mut Cpu, iids: &[Guid]) -> Option<Option<u32>> {
    let classes = to_load(cpu, iids);
    if classes.is_empty() {
        return None;
    }
    let mut vars = vec![calls::NDR_LOAD, 0];
    for clsid in classes {
        vars.extend(
            clsid
                .0
                .chunks(4)
                .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])),
        );
    }
    Some(calls::start(cpu, vars, LOAD_SCRATCH))
}

/// The loading task's variables: the place of the class it is at, then
/// the classes, four each.
const L_NEXT: usize = 1;
const L_CLASSES: usize = 2;
/// Its scratch space: the class, IPSFactoryBuffer's IID, the object.
const L_CLSID: u32 = 0;
const L_IID: u32 = 16;
const L_OUT: u32 = 32;
const LOAD_SCRATCH: u32 = 36;

const CLSCTX_INPROC_SERVER: u32 = 1;
const QUERY_INTERFACE: u32 = 0;
const RELEASE: u32 = 2;

/// The loading task. Step 0 gets the class object of the next class,
/// step 1 keeps it, step 2 goes on to the next after the Release of an
/// object that isn't one of RPCRT4's.
pub fn load_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let s = t.scratch;
    loop {
        let i = t.var(L_NEXT) as usize;
        match t.step {
            0 => {
                if L_CLASSES + 4 * i + 4 > t.vars.len() {
                    // All loaded: the function runs again.
                    t.retry(cpu);
                    return Next::Stop;
                }
                class_var(t, i).write(cpu, s + L_CLSID);
                compobj::IID_IPSFACTORYBUFFER.write(cpu, s + L_IID);
                write32(cpu, s + L_OUT, 0);
                let get = super::address_of("CoGetClassObject");
                let args = [s + L_CLSID, CLSCTX_INPROC_SERVER, 0, s + L_IID, s + L_OUT];
                return Next::call(get, &args, 1);
            }
            1 => {
                let clsid = class_var(t, i);
                let object = read32(cpu, s + L_OUT);
                let ours = object != 0 && read32(cpu, object) == factory_vtable();
                let pid = current_pid(cpu);
                rpc(cpu).ps.push(PsFactory {
                    pid,
                    clsid: clsid.0.to_vec(),
                    object: if ours { object } else { 0 },
                });
                t.step = 2;
                if !ours {
                    cpu.bus.log_string(&format!(
                        "[W32] OLE32: the proxy/stub class {} has no tables of RPCRT4's ({:08X})",
                        clsid, value
                    ));
                    if let Some(release) = super::super::method(cpu, object, RELEASE) {
                        return Next::call(release, &[object], 2);
                    }
                }
            }
            _ => {
                t.set(L_NEXT, i as u32 + 1);
                t.step = 0;
            }
        }
    }
}

/// The loading task's class at `i`.
fn class_var(t: &Task, i: usize) -> Guid {
    let mut bytes = [0; 16];
    for k in 0..4 {
        let word = t.var(L_CLASSES + 4 * i + k);
        bytes[4 * k..4 * k + 4].copy_from_slice(&word.to_le_bytes());
    }
    Guid(bytes)
}

/// A process ended: its class objects and buffers go with it.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let r = rpc(cpu);
    r.ps.retain(|f| f.pid != pid);
    r.stub_buffers.retain(|b| b.pid != pid);
}

// What the functions share.

/// Raise the exception `code` from the function the running stub is of,
/// which takes `args` arguments, as RpcRaiseException does.
fn raise(cpu: &mut Cpu, args: u32, code: u32) -> Option<u32> {
    crate::win32::seh::raise_from_call(cpu, args, code, EXCEPTION_NONCONTINUABLE, &[])
}

/// Return 0 once the AddRefs and Releases `side` has left are made; or
/// raise `result`'s error from the function, which takes `args`
/// arguments.
fn finish(cpu: &mut Cpu, args: u32, result: Result<(), u32>, side: Side) -> Option<u32> {
    if let Err(code) = result {
        return raise(cpu, args, code);
    }
    let mut calls = Vec::new();
    for pair in side.guest.chunks(2) {
        if let [method, object] = *pair
            && let Some(m) = super::super::method(cpu, object, method)
        {
            calls.push((m, object));
        }
    }
    super::super::call_all(cpu, 0, &calls)
}

/// A zeroed block of the task allocator of at least `size` bytes.
fn block(cpu: &mut Cpu, size: u32) -> Result<u32, u32> {
    let at = task_alloc(cpu, size.max(8));
    if at == 0 {
        return Err(E_OUTOFMEMORY);
    }
    cpu.bus.guest_fill(at, size.max(8) as usize, 0);
    Ok(at)
}

/// The HRESULT a proxy returns for the exception `code`: an HRESULT as
/// it is, a Win32 error as HRESULT_FROM_WIN32 makes it.
fn error_hresult(code: u32) -> u32 {
    if code == 0 || code & 0x8000_0000 != 0 {
        code
    } else {
        (code & 0xFFFF) | 0x8007_0000
    }
}

// Registration and the class object.

/// Set the value `name` of the key `path` under HKEY_CLASSES_ROOT,
/// making the key if it isn't there.
fn set_class_value(cpu: &mut Cpu, path: &str, name: &str, text: &str) {
    let r = registry::get(cpu);
    let registry::Open::Key(root) = r.open(HKEY_CLASSES_ROOT, 0) else {
        return;
    };
    let (key, _) = r.create(root, path);
    let mut data = crate::dosstr::to_bytes(text);
    data.push(0);
    r.set_value(key, name, REG_SZ, data);
}

/// Delete the key `path` under HKEY_CLASSES_ROOT with its subkeys.
fn delete_class_key(cpu: &mut Cpu, path: &str) {
    let r = registry::get(cpu);
    let registry::Open::Key(root) = r.open(HKEY_CLASSES_ROOT, 0) else {
        return;
    };
    if let Some(key) = r.find(root, path) {
        r.delete(key);
    }
}

/// The class of the tables at `files`: the one at `clsid`, or without
/// one the first interface's IID, as MIDL's dlldata.c has it.
fn tables_class(cpu: &mut Cpu, files: u32, clsid: u32) -> Option<Guid> {
    if clsid != 0 {
        return Guid::read(cpu, clsid);
    }
    interfaces(cpu, files).first().map(|f| f.0)
}

/// NdrDllRegisterProxy(module, files, clsid): each interface of the
/// tables gets Interface\{iid}, with its name, ProxyStubClsid32 and
/// NumMethods, and the class CLSID\{clsid}, with InprocServer32 naming
/// the module.
pub fn dll_register_proxy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [module, files, clsid] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(clsid) = tables_class(cpu, files, clsid) else {
        return Some(E_INVALIDARG);
    };
    let path = crate::win32::process(cpu)
        .modules
        .iter()
        .find(|m| m.base == module)
        .map(|m| m.path.clone());
    let Some(path) = path else {
        return Some(E_INVALIDARG);
    };
    let class = clsid.to_string();
    for (iid, interface, name) in interfaces(cpu, files) {
        let name = user32::string_at(cpu, name).unwrap_or_default();
        let methods = read32(cpu, interface.stub + SH_METHODS);
        let key = format!("Interface\\{}", iid);
        set_class_value(cpu, &key, "", &name);
        set_class_value(cpu, &format!("{key}\\ProxyStubClsid32"), "", &class);
        set_class_value(cpu, &format!("{key}\\NumMethods"), "", &methods.to_string());
    }
    let key = format!("CLSID\\{}", class);
    set_class_value(cpu, &key, "", "PSFactoryBuffer");
    let server = format!("{key}\\InprocServer32");
    set_class_value(cpu, &server, "", &path);
    set_class_value(cpu, &server, "ThreadingModel", "Both");
    Some(S_OK)
}

/// NdrDllUnregisterProxy(module, files, clsid): the keys
/// NdrDllRegisterProxy made go.
pub fn dll_unregister_proxy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [_, files, clsid] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let Some(clsid) = tables_class(cpu, files, clsid) else {
        return Some(E_INVALIDARG);
    };
    for (iid, _, _) in interfaces(cpu, files) {
        delete_class_key(cpu, &format!("Interface\\{}", iid));
    }
    delete_class_key(cpu, &format!("CLSID\\{}", clsid));
    Some(S_OK)
}

/// The address of the vtable of RPCRT4's class objects.
fn factory_vtable() -> u32 {
    crate::win32::rpcrt4::vtable_address("IPSFactoryBuffer")
}

/// NdrDllGetClassObject(clsid, iid, out, files, class, factory): the
/// DLL's class object, the CStdPSFactoryBuffer at `factory`, which gets
/// its vtable and tables the first time; for the tables' class only.
pub fn dll_get_class_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [clsid, iid, out, files, class, factory] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(E_INVALIDARG);
    }
    write32(cpu, out, 0);
    let (Some(clsid), Some(iid)) = (Guid::read(cpu, clsid), Guid::read(cpu, iid)) else {
        return Some(E_INVALIDARG);
    };
    if tables_class(cpu, files, class) != Some(clsid) {
        return Some(CLASS_E_CLASSNOTAVAILABLE);
    }
    if read32(cpu, factory) == 0 {
        write32(cpu, factory, factory_vtable());
        write32(cpu, factory + PSF_REFS, 0);
        write32(cpu, factory + PSF_FILES, files);
    }
    factory_query(cpu, factory, &iid, out)
}

/// NdrDllCanUnloadNow(factory): S_OK when nothing holds the class
/// object.
pub fn dll_can_unload_now(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let refs = read32(cpu, args[0] + PSF_REFS);
    Some(if refs == 0 { S_OK } else { S_FALSE })
}

/// The class object as `iid`, IUnknown or IPSFactoryBuffer, into `out`.
fn factory_query(cpu: &mut Cpu, this: u32, iid: &Guid, out: u32) -> Option<u32> {
    if *iid != IID_IUNKNOWN && *iid != compobj::IID_IPSFACTORYBUFFER {
        write32(cpu, out, 0);
        return Some(E_NOINTERFACE);
    }
    let refs = read32(cpu, this + PSF_REFS);
    write32(cpu, this + PSF_REFS, refs.wrapping_add(1));
    write32(cpu, out, this);
    Some(S_OK)
}

/// IPSFactoryBuffer::QueryInterface(this, iid, out).
pub fn factory_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(com::E_POINTER);
    }
    let Some(iid) = Guid::read(cpu, iid) else {
        write32(cpu, out, 0);
        return Some(E_INVALIDARG);
    };
    factory_query(cpu, this, &iid, out)
}

/// IPSFactoryBuffer::AddRef(this).
pub fn factory_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let refs = read32(cpu, args[0] + PSF_REFS).wrapping_add(1);
    write32(cpu, args[0] + PSF_REFS, refs);
    Some(refs)
}

/// IPSFactoryBuffer::Release(this): the object is the DLL's own data,
/// which stays.
pub fn factory_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let refs = read32(cpu, args[0] + PSF_REFS).saturating_sub(1);
    write32(cpu, args[0] + PSF_REFS, refs);
    Some(refs)
}

/// IPSFactoryBuffer::CreateProxy and CreateStub. COM's proxies and
/// stubs of the DLL's interfaces are rpc.rs's, which use the tables
/// themselves (`interface`), so nothing calls these but a program that
/// makes a proxy or stub of its own, with a channel of its own, which
/// no program of the time does. They set the out pointers to NULL.
pub fn factory_create_proxy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    for &out in &args[3..5] {
        if out != 0 {
            write32(cpu, out, 0);
        }
    }
    Some(E_NOTIMPL)
}

pub fn factory_create_stub(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[3] != 0 {
        write32(cpu, args[3], 0);
    }
    Some(E_NOTIMPL)
}

// The proxy's side.

/// IUnknown_QueryInterface_Proxy(this, iid, out),
/// IUnknown_AddRef_Proxy(this) and IUnknown_Release_Proxy(this): the
/// proxy's IUnknown, as the proxies of type libraries have it.
pub fn unknown_query_interface_proxy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = cpu.esp() + 8;
    super::proxy_call(cpu, args[0], QUERY_INTERFACE, at)
}

pub fn unknown_add_ref_proxy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = cpu.esp() + 8;
    super::proxy_call(cpu, args[0], 1, at)
}

pub fn unknown_release_proxy(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = cpu.esp() + 8;
    super::proxy_call(cpu, args[0], RELEASE, at)
}

/// Set the fields of the MIDL_STUB_MESSAGE at `m` that the
/// Initialize functions set, for the RPC_MESSAGE at `rpc_msg` and the
/// MIDL_STUB_DESC at `desc`.
fn init_stub_message(cpu: &mut Cpu, m: u32, rpc_msg: u32, desc: u32, client: bool) {
    cpu.bus.guest_fill(m, SM_INITIALIZED as usize, 0);
    write32(cpu, m + SM_RPC_MSG, rpc_msg);
    write32(cpu, m + SM_IS_CLIENT, client as u32);
    let allocate = read32(cpu, desc + SD_ALLOCATE);
    let free = read32(cpu, desc + SD_FREE);
    write32(cpu, m + SM_ALLOCATE, allocate);
    write32(cpu, m + SM_FREE, free);
    write32(cpu, m + SM_STUB_DESC, desc);
}

/// NdrProxyInitialize(this, message, stub message, description, method):
/// the messages of a call of the method of the proxy `this`.
pub fn proxy_initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [_, rpc_msg, m, desc, method] = args[..] else {
        return Some(0);
    };
    cpu.bus.guest_fill(rpc_msg, RPC_MESSAGE_SIZE as usize, 0);
    write32(
        cpu,
        rpc_msg + RM_DATA_REPRESENTATION,
        NDR_LOCAL_DATA_REPRESENTATION,
    );
    write32(cpu, rpc_msg + RM_PROC_NUM, method);
    let interface = read32(cpu, desc + SD_INTERFACE);
    write32(cpu, rpc_msg + RM_INTERFACE, interface);
    init_stub_message(cpu, m, rpc_msg, desc, true);
    Some(0)
}

/// Give the stub message at `m` a buffer of its BufferLength.
fn new_buffer(cpu: &mut Cpu, m: u32) -> Result<(), u32> {
    let length = read32(cpu, m + SM_BUFFER_LENGTH);
    let buffer = block(cpu, length)?;
    let rpc_msg = read32(cpu, m + SM_RPC_MSG);
    write32(cpu, rpc_msg + RM_BUFFER, buffer);
    write32(cpu, rpc_msg + RM_BUFFER_LENGTH, length);
    set_buffer(cpu, m, buffer, length);
    Ok(())
}

/// The stub message at `m` reads or writes the `length` bytes at
/// `buffer`.
fn set_buffer(cpu: &mut Cpu, m: u32, buffer: u32, length: u32) {
    write32(cpu, m + SM_BUFFER, buffer);
    write32(cpu, m + SM_BUFFER_START, buffer);
    write32(cpu, m + SM_BUFFER_END, buffer.wrapping_add(length));
}

/// NdrProxyGetBuffer(this, stub message): the buffer the arguments are
/// marshalled into.
pub fn proxy_get_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let result = new_buffer(cpu, args[1]);
    finish(cpu, 2, result, Side::default())
}

/// NdrProxyFreeBuffer(this, stub message): the buffer goes.
pub fn proxy_free_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let rpc_msg = read32(cpu, args[1] + SM_RPC_MSG);
    let buffer = read32(cpu, rpc_msg + RM_BUFFER);
    task_free(cpu, buffer);
    write32(cpu, rpc_msg + RM_BUFFER, 0);
    Some(0)
}

/// NdrProxyErrorHandler(code): the HRESULT a proxy returns for the
/// exception it caught.
pub fn proxy_error_handler(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(error_hresult(args[0]))
}

/// NdrProxySendReceive(this, stub message): the buffer, up to where the
/// arguments were marshalled, goes to the object's thread; the reply
/// comes back in a new buffer.
pub fn proxy_send_receive(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, m] = args[..] else {
        return Some(0);
    };
    let Some(i) = proxy_at(cpu, this) else {
        return raise(cpu, 2, RPC_E_DISCONNECTED);
    };
    let p = rpc(cpu).proxies[i].clone();
    let Some(e) = export(cpu, p.export).cloned() else {
        return raise(cpu, 2, RPC_E_DISCONNECTED);
    };
    let rpc_msg = read32(cpu, m + SM_RPC_MSG);
    let buffer = read32(cpu, rpc_msg + RM_BUFFER);
    let end = read32(cpu, m + SM_BUFFER);
    let length = end
        .wrapping_sub(buffer)
        .min(read32(cpu, rpc_msg + RM_BUFFER_LENGTH));
    let method = read32(cpu, rpc_msg + RM_PROC_NUM) & 0xFFFF;
    let mut w = Wire::default();
    w.u32(e.id);
    w.u32(method | NDR_CALL);
    w.0.extend(read_bytes(cpu, buffer, length));
    task_free(cpu, buffer);
    write32(cpu, rpc_msg + RM_BUFFER, 0);
    let id = next_id(cpu);
    rpc(cpu).calls.push(Pending {
        id,
        iid: p.iid.clone(),
        request: w.0,
        ..Pending::default()
    });
    calls::start(cpu, vec![calls::NDR_SEND, id, e.hwnd, m], 0)
}

/// NdrProxySendReceive's task: variables 1 to 3 the call's id, the
/// window it goes to and the stub message.
const X_ID: usize = 1;
const X_HWND: usize = 2;
const X_MESSAGE: usize = 3;

pub fn proxy_step(cpu: &mut Cpu, t: &mut Task, _: u32) -> Next {
    let id = t.var(X_ID);
    match t.step {
        0 => {
            let send = user32::address_of(cpu, "SendMessageA");
            Next::call(send, &[t.var(X_HWND), WM_OLE_CALL, id, 0], 1)
        }
        1 => {
            let calls = &mut rpc(cpu).calls;
            let Some(i) = calls.iter().position(|c| c.id == id) else {
                return raise_in_task(RPC_E_SERVER_DIED);
            };
            let call = calls.remove(i);
            if !call.replied || call.reply.len() < 4 {
                return raise_in_task(RPC_E_SERVER_DIED);
            }
            let mut r = Reader::new(&call.request);
            r.u32();
            let method = r.u32() & !NDR_CALL;
            let result = Reader::new(&call.reply).u32();
            cpu.bus.log_string(&format!(
                "[W32] OLE32: method {} of {} in another process = {:08X}, {} bytes back",
                method,
                guid_of(&call.iid),
                result,
                call.reply.len() - 4
            ));
            if result != S_OK {
                return raise_in_task(result);
            }
            let bytes = &call.reply[4..];
            let m = t.var(X_MESSAGE);
            write32(cpu, m + SM_BUFFER_LENGTH, bytes.len() as u32);
            if let Err(code) = new_buffer(cpu, m) {
                return raise_in_task(code);
            }
            let buffer = read32(cpu, m + SM_BUFFER);
            write_bytes(cpu, buffer, bytes);
            Next::Done(0)
        }
        _ => Next::Done(0),
    }
}

/// A step that raises the exception `code` where the task's function was
/// called, calling RpcRaiseException as the program would. It doesn't
/// return: the program's handler takes over.
fn raise_in_task(code: u32) -> Next {
    let raise = crate::win32::rpcrt4::address_of("RpcRaiseException");
    Next::call(raise, &[code], 2)
}

/// RpcRaiseException(code): the exception, which can't be continued.
pub fn rpc_raise_exception(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    raise(cpu, 1, args[0])
}

/// NdrConvert(stub message, format): the data in the buffer to the
/// local representation. Both sides are this machine's, so there is
/// nothing to convert.
pub fn convert(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// NdrOleAllocate(size) and NdrOleFree(block): the task allocator.
pub fn ole_allocate(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(task_alloc(cpu, args[0]))
}

pub fn ole_free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    task_free(cpu, args[0]);
    Some(0)
}

// The object's side.

/// Whether the request `request` is a call of a proxy/stub DLL's proxy,
/// and so for `stub_step`.
pub fn is_ndr_call(request: &[u8]) -> bool {
    let mut r = Reader::new(request);
    r.u32();
    r.u32() & NDR_CALL != 0
}

/// The interface of the export a request is for, if it is of the
/// running process.
pub fn request_iid(cpu: &mut Cpu, request: &[u8]) -> Option<Guid> {
    let id = Reader::new(request).u32();
    export(cpu, id).map(|e| guid_of(&e.iid))
}

/// The stub task: variables 1 to 3 the call's id, the block with the
/// stub and the message, and the buffer with the arguments.
const T_ID: usize = 1;
const T_BLOCK: usize = 2;
const T_BUFFER: usize = 3;
/// The block: the CStdStubBuffer, then the RPC_MESSAGE.
const B_STUB: u32 = 0;
const B_MESSAGE: u32 = STUB_BUFFER_SIZE;
const B_SIZE: u32 = STUB_BUFFER_SIZE + RPC_MESSAGE_SIZE;

/// Start the stub task of the call `id`.
pub fn start_stub(cpu: &mut Cpu, id: u32) -> Option<u32> {
    calls::start(cpu, vec![calls::NDR_STUB, id], 0)
}

/// The stub task: step 0 calls the stub's Invoke with the arguments'
/// buffer, step 1 replies with the buffer the stub function filled.
pub fn stub_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let id = t.var(T_ID);
    if t.step == 0 {
        return match invoke(cpu, t, id) {
            Ok(next) => next,
            Err(code) => {
                reply(cpu, id, code, Wire::default());
                Next::Done(0)
            }
        };
    }
    let block = t.var(T_BLOCK);
    let buffer = t.var(T_BUFFER);
    let message = block + B_MESSAGE;
    let out = read32(cpu, message + RM_BUFFER);
    let length = read32(cpu, message + RM_BUFFER_LENGTH);
    let w = if value == S_OK {
        Wire(read_bytes(cpu, out, length))
    } else {
        Wire::default()
    };
    reply(cpu, id, value, w);
    rpc(cpu).stub_buffers.retain(|b| b.start != buffer);
    if out != buffer {
        task_free(cpu, out);
    }
    task_free(cpu, buffer);
    task_free(cpu, block);
    Next::Done(0)
}

/// Make the stub and the message of the call `id` and call the stub's
/// Invoke; or the HRESULT the call fails with.
fn invoke(cpu: &mut Cpu, t: &mut Task, id: u32) -> Result<Next, u32> {
    let request = rpc(cpu)
        .calls
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.request.clone())
        .ok_or(RPC_E_DISCONNECTED)?;
    let mut r = Reader::new(&request);
    let export_id = r.u32();
    let method = r.u32() & !NDR_CALL;
    let e = export(cpu, export_id).cloned().ok_or(RPC_E_DISCONNECTED)?;
    if e.pid != current_pid(cpu) {
        return Err(RPC_E_DISCONNECTED);
    }
    let interface = interface(cpu, &guid_of(&e.iid)).ok_or(E_NOINTERFACE)?;
    let bytes = request.get(8..).unwrap_or_default();
    let length = bytes.len() as u32;
    let block = block(cpu, B_SIZE)?;
    let buffer = match self::block(cpu, length) {
        Ok(buffer) => buffer,
        Err(code) => {
            task_free(cpu, block);
            return Err(code);
        }
    };
    write_bytes(cpu, buffer, bytes);
    let stub = block + B_STUB;
    write32(cpu, stub, interface.stub_vtable());
    write32(cpu, stub + SB_REFS, 1);
    write32(cpu, stub + SB_OBJECT, e.object);
    let message = block + B_MESSAGE;
    write32(
        cpu,
        message + RM_DATA_REPRESENTATION,
        NDR_LOCAL_DATA_REPRESENTATION,
    );
    write32(cpu, message + RM_BUFFER, buffer);
    write32(cpu, message + RM_BUFFER_LENGTH, length);
    write32(cpu, message + RM_PROC_NUM, method);
    t.set(T_BLOCK, block);
    t.set(T_BUFFER, buffer);
    let pid = current_pid(cpu);
    rpc(cpu).stub_buffers.push(StubBuffer {
        pid,
        start: buffer,
        end: buffer + length,
    });
    let invoke = read32(cpu, interface.stub_vtable() + 4 * INVOKE);
    Ok(Next::call(invoke, &[stub, message, 0], 1))
}

/// NdrStubInitialize(message, stub message, description, channel): the
/// stub message reads the arguments in the message's buffer.
pub fn stub_initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [rpc_msg, m, desc, channel] = args[..] else {
        return Some(0);
    };
    init_stub_message(cpu, m, rpc_msg, desc, false);
    let buffer = read32(cpu, rpc_msg + RM_BUFFER);
    let length = read32(cpu, rpc_msg + RM_BUFFER_LENGTH);
    set_buffer(cpu, m, buffer, length);
    write32(cpu, m + SM_BUFFER_LENGTH, length);
    write32(cpu, m + SM_CHANNEL, channel);
    Some(0)
}

/// NdrStubGetBuffer(stub, channel, stub message): the buffer what the
/// method gave back is marshalled into, which becomes the message's.
pub fn stub_get_buffer(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let result = new_buffer(cpu, args[2]);
    finish(cpu, 3, result, Side::default())
}

/// The CInterfaceStubVtbl of the stub at `this`: its vtable is the
/// IRpcStubBuffer vtable at the end of it.
fn stub_header(cpu: &mut Cpu, this: u32) -> u32 {
    read32(cpu, this).wrapping_sub(SH_VTABLE)
}

/// CStdStubBuffer_QueryInterface(this, iid, out): IUnknown and
/// IRpcStubBuffer are the stub itself.
pub fn stub_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, iid, out] = args[..] else {
        return Some(E_INVALIDARG);
    };
    if out == 0 {
        return Some(com::E_POINTER);
    }
    match Guid::read(cpu, iid) {
        Some(iid) if iid == IID_IUNKNOWN || iid == IID_IRPCSTUBBUFFER => {
            stub_add_ref(cpu, &[this]);
            write32(cpu, out, this);
            Some(S_OK)
        }
        _ => {
            write32(cpu, out, 0);
            Some(E_NOINTERFACE)
        }
    }
}

/// CStdStubBuffer_AddRef(this).
pub fn stub_add_ref(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let refs = read32(cpu, args[0] + SB_REFS).wrapping_add(1);
    write32(cpu, args[0] + SB_REFS, refs);
    Some(refs)
}

/// NdrCStdStubBuffer_Release(this, factory): with the last reference
/// the stub lets go of its object and goes.
pub fn stub_release(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let this = args[0];
    let refs = read32(cpu, this + SB_REFS).saturating_sub(1);
    write32(cpu, this + SB_REFS, refs);
    if refs > 0 {
        return Some(refs);
    }
    let object = read32(cpu, this + SB_OBJECT);
    task_free(cpu, this);
    release_object(cpu, object, 0)
}

/// Release `object`, if there is one, then return `value`.
fn release_object(cpu: &mut Cpu, object: u32, value: u32) -> Option<u32> {
    match super::super::method(cpu, object, RELEASE) {
        Some(release) => super::super::call_all(cpu, value, &[(release, object)]),
        None => Some(value),
    }
}

/// CStdStubBuffer_Connect(this, object): the stub calls the object's
/// interface, which its QueryInterface gives.
pub fn stub_connect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, object] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let header = stub_header(cpu, this);
    let piid = read32(cpu, header + SH_IID);
    let Some(query) = super::super::method(cpu, object, QUERY_INTERFACE) else {
        return Some(E_INVALIDARG);
    };
    let vars = vec![calls::NDR_INVOKE, 0, query, object, piid, this + SB_OBJECT];
    calls::start(cpu, vars, 0)
}

/// CStdStubBuffer_Disconnect(this): the stub lets go of its object.
pub fn stub_disconnect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let object = read32(cpu, args[0] + SB_OBJECT);
    write32(cpu, args[0] + SB_OBJECT, 0);
    release_object(cpu, object, 0)
}

/// CStdStubBuffer_Invoke(this, message, channel): the stub function of
/// the message's method, with a stub phase of its own, then S_OK.
pub fn stub_invoke(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [this, message, channel] = args[..] else {
        return Some(E_INVALIDARG);
    };
    let header = stub_header(cpu, this);
    let methods = read32(cpu, header + SH_METHODS);
    let method = read32(cpu, message + RM_PROC_NUM) & 0xFFFF;
    if !(3..methods).contains(&method) {
        return Some(error_hresult(RPC_X_BAD_STUB_DATA));
    }
    let table = read32(cpu, header + SH_DISPATCH);
    let dispatch = read32(cpu, table + 4 * method);
    let vars = vec![calls::NDR_INVOKE, 1, dispatch, this, channel, message];
    calls::start(cpu, vars, 4)
}

/// The task of Connect and Invoke: variable 1 says which (Invoke's
/// call gets the scratch space as its last argument, the stub phase,
/// and the task returns S_OK), variable 2 the function, then its
/// arguments.
pub fn call_step(_: &mut Cpu, t: &mut Task, value: u32) -> Next {
    if t.step > 0 {
        return Next::Done(if t.var(1) == 1 { S_OK } else { value });
    }
    let mut args = t.vars[3..].to_vec();
    if t.var(1) == 1 {
        args.push(t.scratch);
    }
    Next::call(t.var(2), &args, 1)
}

/// CStdStubBuffer_IsIIDSupported(this, iid): the stub, if it is of the
/// interface `iid`; NULL if not.
pub fn stub_is_iid_supported(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let header = stub_header(cpu, args[0]);
    let piid = read32(cpu, header + SH_IID);
    let same = Guid::read(cpu, piid).is_some() && Guid::read(cpu, piid) == Guid::read(cpu, args[1]);
    Some(if same { args[0] } else { 0 })
}

/// CStdStubBuffer_CountRefs(this): the stub's references.
pub fn stub_count_refs(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(read32(cpu, args[0] + SB_REFS))
}

/// CStdStubBuffer_DebugServerQueryInterface(this, out): the object.
pub fn stub_debug_server_query_interface(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let object = read32(cpu, args[0] + SB_OBJECT);
    write32(cpu, args[1], object);
    Some(S_OK)
}

/// CStdStubBuffer_DebugServerRelease(this, object): nothing to do.
pub fn stub_debug_server_release(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

// The types.

fn read8(cpu: &mut Cpu, at: u32) -> u8 {
    cpu.bus.guest_read_8(at)
}

fn read16(cpu: &mut Cpu, at: u32) -> u16 {
    cpu.bus.guest_read_16(at)
}

fn is_client(cpu: &mut Cpu, m: u32) -> bool {
    read8(cpu, m + SM_IS_CLIENT) != 0
}

/// A base type's bytes in the buffer and in memory.
fn base_type(fc: u8) -> Option<(u32, u32)> {
    match fc {
        FC_BYTE | FC_CHAR | FC_SMALL | FC_USMALL => Some((1, 1)),
        FC_WCHAR | FC_SHORT | FC_USHORT => Some((2, 2)),
        FC_ENUM16 => Some((2, 4)),
        FC_LONG | FC_ULONG | FC_FLOAT | FC_ENUM32 | FC_ERROR_STATUS_T | FC_INT3264
        | FC_UINT3264 => Some((4, 4)),
        FC_HYPER | FC_DOUBLE => Some((8, 8)),
        _ => None,
    }
}

/// A structure or fixed array without pointers at `fmt`: its alignment
/// and size, the same in the buffer and in memory.
fn flat_type(cpu: &mut Cpu, fmt: u32) -> Option<(u32, u32)> {
    let align = read8(cpu, fmt + 1) as u32 + 1;
    let (size, layout) = match read8(cpu, fmt) {
        FC_STRUCT => return Some((align, read16(cpu, fmt + 2) as u32)),
        FC_SMFARRAY => (read16(cpu, fmt + 2) as u32, fmt + 4),
        FC_LGFARRAY => (read32(cpu, fmt + 2), fmt + 6),
        _ => return None,
    };
    // An array of pointers has their layout before its elements' type.
    (read8(cpu, layout) != FC_PP).then_some((align, size))
}

/// A pointer's type, attributes and the description of what it points
/// to.
struct Pointer {
    kind: u8,
    attr: u8,
    target: u32,
}

fn pointer(cpu: &mut Cpu, fmt: u32) -> Option<Pointer> {
    let kind = read8(cpu, fmt);
    if !matches!(kind, FC_RP | FC_UP | FC_OP) {
        return None;
    }
    let attr = read8(cpu, fmt + 1);
    let target = if attr & FC_SIMPLE_POINTER != 0 {
        fmt + 2
    } else {
        let offset = read16(cpu, fmt + 2) as i16 as i32;
        (fmt + 2).wrapping_add(offset as u32)
    };
    Some(Pointer { kind, attr, target })
}

/// What an unsupported format character does: the buffer is taken as
/// bad, as RPCRT4 takes a type it doesn't know.
fn unsupported(cpu: &mut Cpu, fmt: u32) -> u32 {
    let fc = read8(cpu, fmt);
    cpu.bus.log_string(&format!(
        "[W32] RPCRT4: the format character {:02X} at {:08X} isn't supported",
        fc, fmt
    ));
    RPC_X_BAD_STUB_DATA
}

/// The memory a value of the type at `fmt` takes.
fn memory_size(cpu: &mut Cpu, fmt: u32) -> Option<u32> {
    let fc = read8(cpu, fmt);
    if let Some((_, memory)) = base_type(fc) {
        return Some(memory);
    }
    if fc == FC_IP || pointer(cpu, fmt).is_some() {
        return Some(4);
    }
    flat_type(cpu, fmt).map(|(_, size)| size)
}

/// The IID of the interface pointer at `fmt`: a constant, or the
/// parameter iid_is names, whose value the proxy or stub put in
/// MaxCount (code of MIDL's -Os) or which is on the stack at StackTop.
fn ip_iid(cpu: &mut Cpu, m: u32, fmt: u32) -> Result<Guid, u32> {
    if read8(cpu, fmt + 1) == FC_CONSTANT_IID {
        return Guid::read(cpu, fmt + 2).ok_or(RPC_X_BAD_STUB_DATA);
    }
    if read8(cpu, fmt + 2) & 0xF0 != FC_TOP_LEVEL_CONFORMANCE {
        return Err(unsupported(cpu, fmt));
    }
    let top = read32(cpu, m + SM_STACK_TOP);
    let at = if top == 0 {
        read32(cpu, m + SM_MAX_COUNT)
    } else {
        let offset = read16(cpu, fmt + 4) as i16 as i32;
        read32(cpu, top.wrapping_add(offset as u32))
    };
    Guid::read(cpu, at).ok_or(RPC_X_BAD_STUB_DATA)
}

/// The IIDs of the interface pointers the type at `fmt` unmarshals.
fn ip_iids(cpu: &mut Cpu, m: u32, fmt: u32, depth: u32) -> Vec<Guid> {
    if read8(cpu, fmt) == FC_IP {
        return ip_iid(cpu, m, fmt).into_iter().collect();
    }
    match pointer(cpu, fmt) {
        Some(p) if depth < 8 => ip_iids(cpu, m, p.target, depth + 1),
        _ => Vec::new(),
    }
}

// The buffer.

/// The sizing pass: `n` more bytes at a multiple of `align`.
fn grow(cpu: &mut Cpu, m: u32, align: u32, n: u32) {
    let length = read32(cpu, m + SM_BUFFER_LENGTH);
    let length = length.wrapping_add(align - 1) & !(align - 1);
    write32(cpu, m + SM_BUFFER_LENGTH, length.wrapping_add(n));
}

/// The `n` bytes of the buffer at its next multiple of `align`, which it
/// moves past: their address.
fn take(cpu: &mut Cpu, m: u32, align: u32, n: u32) -> Result<u32, u32> {
    let buffer = read32(cpu, m + SM_BUFFER);
    let at = buffer.wrapping_add(align - 1) & !(align - 1);
    let end = at.checked_add(n).ok_or(RPC_X_BAD_STUB_DATA)?;
    if at < buffer || end > read32(cpu, m + SM_BUFFER_END) {
        return Err(RPC_X_BAD_STUB_DATA);
    }
    write32(cpu, m + SM_BUFFER, end);
    Ok(at)
}

/// Write `bytes` into the buffer at its next multiple of `align`.
fn put(cpu: &mut Cpu, m: u32, align: u32, bytes: &[u8]) -> Result<(), u32> {
    let at = take(cpu, m, align, bytes.len() as u32)?;
    write_bytes(cpu, at, bytes);
    Ok(())
}

/// Whether `p` is in a buffer, where nothing is to be freed.
fn in_buffer(cpu: &mut Cpu, m: u32, p: u32) -> bool {
    let start = read32(cpu, m + SM_BUFFER_START);
    let end = read32(cpu, m + SM_BUFFER_END);
    if (start..end).contains(&p) {
        return true;
    }
    let pid = current_pid(cpu);
    rpc(cpu)
        .stub_buffers
        .iter()
        .any(|b| b.pid == pid && (b.start..b.end).contains(&p))
}

// Sizing, marshalling, unmarshalling and freeing. As RPCRT4's own
// functions, `mem` is where the data is, an interface pointer itself or
// a pointer itself; `to` is where the pointer to the data goes, or the
// interface pointer, or the pointer.

/// The bytes of an interface pointer at most: its length, then its
/// export as `put_iface` writes it.
const IP_BYTES: u32 = 4 + 5;

fn size(cpu: &mut Cpu, m: u32, mem: u32, fmt: u32) -> Result<(), u32> {
    let fc = read8(cpu, fmt);
    if let Some((wire, _)) = base_type(fc) {
        grow(cpu, m, wire, wire);
    } else if let Some((align, n)) = flat_type(cpu, fmt) {
        grow(cpu, m, align, n);
    } else if fc == FC_IP {
        grow(cpu, m, 4, IP_BYTES);
    } else if let Some(p) = pointer(cpu, fmt) {
        if p.kind != FC_RP {
            grow(cpu, m, 4, 4);
        }
        if mem != 0 {
            let target = deref(cpu, &p, mem);
            size(cpu, m, target, p.target)?;
        }
    } else {
        return Err(unsupported(cpu, fmt));
    }
    Ok(())
}

/// What the pointer `p` described by `ptr` points to, as the type it
/// points to takes it: the pointer it points to with FC_POINTER_DEREF.
fn deref(cpu: &mut Cpu, ptr: &Pointer, p: u32) -> u32 {
    if ptr.attr & FC_POINTER_DEREF != 0 {
        read32(cpu, p)
    } else {
        p
    }
}

fn marshal(cpu: &mut Cpu, m: u32, mem: u32, fmt: u32, side: &mut Side) -> Result<(), u32> {
    let fc = read8(cpu, fmt);
    if let Some((wire, _)) = base_type(fc) {
        let bytes = read_bytes(cpu, mem, wire);
        put(cpu, m, wire, &bytes)
    } else if let Some((align, n)) = flat_type(cpu, fmt) {
        let bytes = read_bytes(cpu, mem, n);
        put(cpu, m, align, &bytes)
    } else if fc == FC_IP {
        let iid = ip_iid(cpu, m, fmt)?;
        let mut w = Wire::default();
        put_iface(cpu, mem, &iid, &mut w, side, false);
        let mut bytes = (w.0.len() as u32).to_le_bytes().to_vec();
        bytes.extend(w.0);
        put(cpu, m, 4, &bytes)
    } else if let Some(p) = pointer(cpu, fmt) {
        if p.kind != FC_RP {
            // A unique pointer's id: 0 for NULL.
            put(cpu, m, 4, &mem.to_le_bytes())?;
        } else if mem == 0 {
            return Err(RPC_X_NULL_REF_POINTER);
        }
        if mem == 0 {
            return Ok(());
        }
        let target = deref(cpu, &p, mem);
        marshal(cpu, m, target, p.target, side)
    } else {
        Err(unsupported(cpu, fmt))
    }
}

fn unmarshal(
    cpu: &mut Cpu,
    m: u32,
    to: u32,
    fmt: u32,
    must_alloc: bool,
    side: &mut Side,
) -> Result<(), u32> {
    let fc = read8(cpu, fmt);
    if fc == FC_IP {
        return unmarshal_ip(cpu, m, to, side);
    }
    if let Some(p) = pointer(cpu, fmt) {
        return unmarshal_pointer(cpu, m, to, &p, must_alloc, side);
    }
    let (align, wire, memory) = match (base_type(fc), flat_type(cpu, fmt)) {
        (Some((wire, memory)), _) => (wire, wire, memory),
        (None, Some((align, n))) => (align, n, n),
        _ => return Err(unsupported(cpu, fmt)),
    };
    let at = take(cpu, m, align, wire)?;
    if must_alloc || read32(cpu, to) == 0 {
        // The object's side reads the data where it is in the buffer.
        if !must_alloc && !is_client(cpu, m) && wire == memory {
            write32(cpu, to, at);
            return Ok(());
        }
        let block = block(cpu, memory)?;
        write32(cpu, to, block);
    }
    let data = read32(cpu, to);
    let mut bytes = read_bytes(cpu, at, wire);
    bytes.resize(memory as usize, 0);
    write_bytes(cpu, data, &bytes);
    Ok(())
}

/// Unmarshal the pointer described by `p` into `to`.
fn unmarshal_pointer(
    cpu: &mut Cpu,
    m: u32,
    to: u32,
    p: &Pointer,
    must_alloc: bool,
    side: &mut Side,
) -> Result<(), u32> {
    if p.kind != FC_RP {
        let at = take(cpu, m, 4, 4)?;
        if read32(cpu, at) == 0 {
            write32(cpu, to, 0);
            return Ok(());
        }
    } else if is_client(cpu, m) && read32(cpu, to) == 0 {
        return Err(RPC_X_NULL_REF_POINTER);
    }
    if p.attr & FC_POINTER_DEREF == 0 {
        return unmarshal(cpu, m, to, p.target, must_alloc, side);
    }
    // It points to a pointer, which goes where it points.
    if must_alloc || read32(cpu, to) == 0 {
        let slot = block(cpu, 4)?;
        write32(cpu, to, slot);
    }
    let slot = read32(cpu, to);
    unmarshal(cpu, m, slot, p.target, must_alloc, side)
}

/// Unmarshal an interface pointer into `to`. A proxy's [in, out]
/// pointer lets go of the interface it held.
fn unmarshal_ip(cpu: &mut Cpu, m: u32, to: u32, side: &mut Side) -> Result<(), u32> {
    let at = take(cpu, m, 4, 4)?;
    let n = read32(cpu, at);
    if n > IP_BYTES {
        return Err(RPC_X_BAD_STUB_DATA);
    }
    let at = take(cpu, m, 1, n)?;
    let bytes = read_bytes(cpu, at, n);
    if is_client(cpu, m) {
        let old = read32(cpu, to);
        drop_iface(cpu, old, side);
    }
    let p = get_iface(cpu, &mut Reader::new(&bytes), side);
    write32(cpu, to, p);
    if p == 0 && bytes.first().is_some_and(|&b| b != 0) {
        // The export is gone, or the interface has no proxy here.
        return Err(E_NOINTERFACE);
    }
    Ok(())
}

fn free(cpu: &mut Cpu, m: u32, mem: u32, fmt: u32, side: &mut Side) -> Result<(), u32> {
    let fc = read8(cpu, fmt);
    if fc == FC_IP {
        drop_iface(cpu, mem, side);
        return Ok(());
    }
    let Some(p) = pointer(cpu, fmt) else {
        // Data without pointers has nothing of its own to free.
        return Ok(());
    };
    if p.attr & FC_DONT_FREE != 0 || mem == 0 {
        return Ok(());
    }
    let target = deref(cpu, &p, mem);
    free(cpu, m, target, p.target, side)?;
    if !in_buffer(cpu, m, mem) && p.attr & FC_ALLOCED_ON_STACK == 0 {
        task_free(cpu, mem);
    }
    Ok(())
}

// The functions MIDL's code calls.

pub fn simple_struct_buffer_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let result = size(cpu, args[0], args[1], args[2]);
    finish(cpu, 3, result, Side::default())
}

pub fn simple_struct_marshall(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut side = Side::default();
    let result = marshal(cpu, args[0], args[1], args[2], &mut side);
    finish(cpu, 3, result, side)
}

pub fn simple_struct_unmarshall(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut side = Side::default();
    let result = unmarshal(
        cpu,
        args[0],
        args[1],
        args[2],
        args[3] & 0xFF != 0,
        &mut side,
    );
    finish(cpu, 4, result, side)
}

pub fn pointer_buffer_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let result = size(cpu, args[0], args[1], args[2]);
    finish(cpu, 3, result, Side::default())
}

pub fn pointer_marshall(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut side = Side::default();
    let result = marshal(cpu, args[0], args[1], args[2], &mut side);
    finish(cpu, 3, result, side)
}

/// NdrPointerUnmarshall(stub message, to, format, must allocate), and
/// NdrInterfacePointerUnmarshall: the proxy/stub DLLs of the interfaces
/// that come are loaded first.
pub fn pointer_unmarshall(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [m, to, fmt, must_alloc] = args[..] else {
        return Some(0);
    };
    let iids = ip_iids(cpu, m, fmt, 0);
    if let Some(loading) = load(cpu, &iids) {
        return loading;
    }
    let mut side = Side::default();
    let result = unmarshal(cpu, m, to, fmt, must_alloc & 0xFF != 0, &mut side);
    finish(cpu, 4, result, side)
}

pub fn pointer_free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let mut side = Side::default();
    let result = free(cpu, args[0], args[1], args[2], &mut side);
    finish(cpu, 3, result, side)
}

pub fn interface_pointer_buffer_size(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    pointer_buffer_size(cpu, args)
}

pub fn interface_pointer_marshall(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    pointer_marshall(cpu, args)
}

pub fn interface_pointer_unmarshall(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    pointer_unmarshall(cpu, args)
}

pub fn interface_pointer_free(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    pointer_free(cpu, args)
}

/// NdrClearOutParameters(stub message, format, out): what a proxy's out
/// parameter holds is let go of and the parameter zeroed, when the call
/// failed.
pub fn clear_out_parameters(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [m, fmt, out] = args[..] else {
        return Some(0);
    };
    let Some(p) = pointer(cpu, fmt) else {
        return Some(0);
    };
    if out == 0 {
        return Some(0);
    }
    let mut side = Side::default();
    let size = if p.attr & FC_POINTER_DEREF != 0 {
        let held = read32(cpu, out);
        if free(cpu, m, held, p.target, &mut side).is_err() {
            return Some(0);
        }
        4
    } else {
        memory_size(cpu, p.target).unwrap_or(0)
    };
    cpu.bus.guest_fill(out, size as usize, 0);
    finish(cpu, 3, Ok(()), side)
}

crate::state_fields!(PsFactory { pid, clsid, object });
crate::state_fields!(StubBuffer { pid, start, end });
