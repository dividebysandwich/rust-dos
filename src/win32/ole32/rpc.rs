//! COM between processes: local servers (CLSCTX_LOCAL_SERVER) and the
//! proxies and stubs that carry calls of their objects from one process
//! to another, as Windows 98's OLE32 and OLE Automation's marshaler do.
//!
//! An interface pointer that goes to another process is exported: COM
//! keeps the pointer, its IID and the thread whose apartment it lives in
//! (`Export`), with the references other processes hold to it, and holds
//! one reference of its own while any do. The receiving process gets a
//! proxy (`Proxy`): an object whose vtable is made for the interface,
//! one entry for each method, each of which pushes its place and calls
//! `ProxyMethod`. A method of a proxy copies its arguments out of the
//! caller's memory as the interface's type library describes them
//! (oleaut32's signature.rs), and sends them to a hidden window of the
//! object's thread (OleMainThreadWndClass, made for each apartment that
//! exports, as Windows makes it). That thread gets the call when it
//! next gets messages, as on Windows, where calls to a single-threaded
//! apartment come with its messages; its window procedure (the stub)
//! makes the arguments again in its own process's memory, calls the
//! object, and copies what the method gave back, which the proxy writes
//! into the caller's memory. The caller waits for the reply as
//! SendMessageA waits, getting the calls other processes make to it
//! meanwhile, so an object may call back into its caller's objects.
//!
//! What goes between the processes are bytes (`Wire`): integers, BSTRs,
//! VARIANTs, SAFEARRAYs, structures without pointers and interfaces,
//! which go as the ids of their exports. AddRef and Release of a proxy
//! stay in its process; when the last reference of the last proxy of an
//! export goes, the object's thread is told to release it with a posted
//! message.
//!
//! A class whose registry key has LocalServer32 and no class object
//! registered for it in another process starts its server: the command
//! line with " -Embedding" after it, then a wait of up to 30 seconds for
//! the server's CoRegisterClassObject. CoCreateInstance then calls the
//! class object's CreateInstance through a proxy, as Windows does.

use super::{address_of, calls, state};
use crate::cpu::Cpu;
use crate::win32::com::{self, E_INVALIDARG, E_NOINTERFACE, E_OUTOFMEMORY, Guid, S_OK};
use crate::win32::oleaut32::signature::{self, Kind, Method};
use crate::win32::task::{Next, Task};
use crate::win32::user32::{self, current_pid, current_tid};
use std::collections::HashMap;
use std::sync::Arc;

pub mod ndr;

pub const E_NOTIMPL: u32 = 0x8000_4001;
const CO_E_SERVER_EXEC_FAILURE: u32 = 0x8008_0005;
const RPC_E_DISCONNECTED: u32 = 0x8001_0108;
const RPC_E_SERVER_DIED: u32 = 0x8001_0007;
const CLASS_E_NOAGGREGATION: u32 = 0x8004_0110;
const DISP_E_BADVARTYPE: u32 = 0x8002_0008;

/// The messages the apartment window gets: a call (wParam its id) and a
/// reference to let go of (wParam the pointer).
const WM_OLE_CALL: u32 = 0x0401;
const WM_OLE_RELEASE: u32 = 0x0402;

const WINDOW_CLASS: &str = "OleMainThreadWndClass";
const WINDOW_TITLE: &[u8] = b"OleMainThreadWndName";

/// IUnknown's methods by their places.
const QUERY_INTERFACE: u32 = 0;
const ADD_REF: u32 = 1;
const RELEASE: u32 = 2;
/// IDispatch::GetTypeInfo's place.
const GET_TYPE_INFO: u32 = 4;

/// How long a client waits for a server it started to register its
/// class object, in microseconds.
const SERVER_WAIT: u64 = 30_000_000;

/// VARIANT types.
const VT_EMPTY: u16 = 0;
const VT_NULL: u16 = 1;
const VT_BSTR: u16 = 8;
const VT_DISPATCH: u16 = 9;
const VT_VARIANT: u16 = 12;
const VT_UNKNOWN: u16 = 13;
const VT_DECIMAL: u16 = 14;
const VT_ARRAY: u16 = 0x2000;
const VT_BYREF: u16 = 0x4000;

/// An interface pointer of a process that other processes hold.
#[derive(Clone, Debug, Default)]
pub struct Export {
    id: u32,
    pid: u32,
    tid: u32,
    /// The apartment window of its thread, which its calls go to.
    hwnd: u32,
    object: u32,
    iid: Vec<u8>,
    /// The references proxies and data on the way hold.
    refs: u32,
    /// Whether COM holds a reference to the object for them, released
    /// when the last goes: all but a class object registered with
    /// CoRegisterClassObject, whose registration holds one.
    owned: bool,
}

/// A proxy: an object of a process standing for an export of another.
#[derive(Clone, Debug, Default)]
pub struct Proxy {
    this: u32,
    pid: u32,
    export: u32,
    iid: Vec<u8>,
    refs: u32,
}

/// The vtable of a process's proxies of an interface.
#[derive(Clone, Debug, Default)]
pub struct ProxyVtable {
    pid: u32,
    iid: Vec<u8>,
    address: u32,
}

/// A thread's apartment window.
#[derive(Clone, Debug, Default)]
pub struct Window {
    tid: u32,
    pid: u32,
    hwnd: u32,
}

/// A call on its way: the request, the reply when it has come, and what
/// the side running it keeps between the steps of its task.
#[derive(Clone, Debug, Default)]
pub struct Pending {
    id: u32,
    /// The interface of the proxy that made it.
    iid: Vec<u8>,
    request: Vec<u8>,
    reply: Vec<u8>,
    replied: bool,
    /// The calls of AddRef (1) and Release (2) left to make: method and
    /// interface, one after the other.
    guest: Vec<u32>,
    /// The object's side: the block with the arguments as the method
    /// gets them, and the blocks made for what they point to.
    image: u32,
    blocks: Vec<u32>,
}

/// COM's state between processes.
#[derive(Clone, Debug, Default)]
pub struct Rpc {
    exports: Vec<Export>,
    proxies: Vec<Proxy>,
    vtables: Vec<ProxyVtable>,
    windows: Vec<Window>,
    calls: Vec<Pending>,
    next_id: u32,
    /// The proxy/stub DLLs' class objects each process got.
    ps: Vec<ndr::PsFactory>,
    /// The buffers of the NDR calls the processes' stubs are running.
    stub_buffers: Vec<ndr::StubBuffer>,
    /// The methods of the interfaces met, by IID: not saved, as they are
    /// read again from the type libraries.
    methods: Methods,
}

/// The methods of interfaces, read from their type libraries.
#[derive(Clone, Debug, Default)]
pub struct Methods(HashMap<Guid, Arc<Vec<Option<Method>>>>);

impl crate::savestate::State for Methods {
    fn save(&self, _: &mut crate::savestate::Writer) {}

    fn load(&mut self, _: &mut crate::savestate::Reader) -> crate::savestate::Result<()> {
        self.0.clear();
        Ok(())
    }
}

fn rpc(cpu: &mut Cpu) -> &mut Rpc {
    &mut state(cpu).rpc.0
}

fn next_id(cpu: &mut Cpu) -> u32 {
    let r = rpc(cpu);
    r.next_id = r.next_id.wrapping_add(1).max(1);
    r.next_id
}

fn guid_of(bytes: &[u8]) -> Guid {
    Guid(<[u8; 16]>::try_from(bytes).unwrap_or_default())
}

/// The methods of the interface `iid`, if calls of it can be copied.
fn methods(cpu: &mut Cpu, iid: &Guid) -> Option<Arc<Vec<Option<Method>>>> {
    if let Some(m) = rpc(cpu).methods.0.get(iid) {
        return Some(m.clone());
    }
    let m = Arc::new(signature::methods(cpu, iid)?);
    rpc(cpu).methods.0.insert(*iid, m.clone());
    Some(m)
}

/// The method at `index` of the interface `iid`: IUnknown's for the
/// first three, which every interface has, whatever marshals it.
fn method_of(cpu: &mut Cpu, iid: &Guid, index: u32) -> Option<Method> {
    if index < 3 {
        return signature::unknown().swap_remove(index as usize);
    }
    methods(cpu, iid)?.get(index as usize).cloned().flatten()
}

/// The IIDs of the interfaces the method `m` gives back, its arguments
/// at `places`.
fn out_iids(cpu: &mut Cpu, m: &Method, places: &[u32]) -> Vec<Guid> {
    let mut iids = Vec::new();
    for param in m.params.iter().filter(|p| p.is_out()) {
        if let Kind::Ptr(inner) = &param.kind {
            match **inner {
                Kind::Iface(iid) => iids.push(iid),
                Kind::IidIs(i) => iids.push(iid_at(cpu, places, i)),
                _ => {}
            }
        }
    }
    iids
}

/// The IIDs of the interfaces the method `m` gets from its caller.
fn in_iids(m: &Method) -> Vec<Guid> {
    m.params
        .iter()
        .filter(|p| p.is_in())
        .filter_map(|p| match &p.kind {
            Kind::Iface(iid) => Some(*iid),
            Kind::Ptr(inner) => match **inner {
                Kind::Iface(iid) => Some(iid),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// A process ended: its proxies let go of their exports, and its
/// exports, windows and vtables go with its memory.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let gone: Vec<u32> = rpc(cpu)
        .proxies
        .iter()
        .filter(|p| p.pid == pid)
        .map(|p| p.export)
        .collect();
    let r = rpc(cpu);
    r.proxies.retain(|p| p.pid != pid);
    r.exports.retain(|e| e.pid != pid);
    r.windows.retain(|w| w.pid != pid);
    r.vtables.retain(|v| v.pid != pid);
    ndr::process_ended(cpu, pid);
    for export in gone {
        release_export(cpu, export, 1);
    }
}

// The apartment windows.

/// The apartment window of the running thread, made if it has none.
fn window(cpu: &mut Cpu) -> u32 {
    let tid = current_tid(cpu);
    if let Some(w) = rpc(cpu).windows.iter().find(|w| w.tid == tid) {
        return w.hwnd;
    }
    let proc = address_of("OleMainWndProc");
    let pid = current_pid(cpu);
    user32::init(cpu);
    let window = user32::window::Window {
        text: WINDOW_TITLE.to_vec(),
        tid,
        pid,
        ..Default::default()
    };
    let hwnd = user32::window::new_hidden(user32::user(cpu), WINDOW_CLASS, proc, window);
    rpc(cpu).windows.push(Window { tid, pid, hwnd });
    hwnd
}

/// The running thread registers a class object for other processes:
/// its apartment window, which their calls come to, is made now.
pub fn class_registered(cpu: &mut Cpu) {
    window(cpu);
}

/// The window of the thread `tid`, if it has one.
fn window_of(cpu: &mut Cpu, tid: u32) -> Option<u32> {
    rpc(cpu)
        .windows
        .iter()
        .find(|w| w.tid == tid)
        .map(|w| w.hwnd)
}

// Exports and proxies.

fn export(cpu: &mut Cpu, id: u32) -> Option<&mut Export> {
    rpc(cpu).exports.iter_mut().find(|e| e.id == id)
}

/// Let go of `n` references to the export `id`: with the last, the
/// object's thread releases COM's reference to it.
fn release_export(cpu: &mut Cpu, id: u32, n: u32) {
    let Some(e) = export(cpu, id) else {
        return;
    };
    e.refs = e.refs.saturating_sub(n);
    if e.refs > 0 {
        return;
    }
    let (owned, tid, hwnd, object) = (e.owned, e.tid, e.hwnd, e.object);
    rpc(cpu).exports.retain(|e| e.id != id);
    if owned {
        let msg = user32::Msg {
            hwnd,
            message: WM_OLE_RELEASE,
            wparam: object,
            ..user32::Msg::default()
        };
        user32::message::post(cpu, tid, msg);
    }
}

/// The export of the running process's `object` as `iid`, made if
/// there is none, with a reference more: its id, and whether it is new.
/// A new one is of the running thread, or of `tid` with `hwnd`.
fn export_object(cpu: &mut Cpu, object: u32, iid: &Guid, owned: bool) -> (u32, bool) {
    let pid = current_pid(cpu);
    if let Some(e) = rpc(cpu)
        .exports
        .iter_mut()
        .find(|e| e.pid == pid && e.object == object && e.iid == iid.0)
    {
        e.refs += 1;
        return (e.id, false);
    }
    let tid = current_tid(cpu);
    let hwnd = window(cpu);
    let id = next_id(cpu);
    rpc(cpu).exports.push(Export {
        id,
        pid,
        tid,
        hwnd,
        object,
        iid: iid.0.to_vec(),
        refs: 1,
        owned,
    });
    (id, true)
}

/// The proxy at `this` of the running process.
fn proxy_at(cpu: &mut Cpu, this: u32) -> Option<usize> {
    let pid = current_pid(cpu);
    rpc(cpu)
        .proxies
        .iter()
        .position(|p| p.pid == pid && p.this == this)
}

/// A proxy of the export `id` in the running process, which takes a
/// reference to it: the one there is, with a reference more, or a new
/// one. 0 when the export is gone or its interface's calls can't be
/// copied.
fn proxy_of(cpu: &mut Cpu, id: u32) -> u32 {
    let pid = current_pid(cpu);
    if let Some(p) = rpc(cpu)
        .proxies
        .iter_mut()
        .find(|p| p.pid == pid && p.export == id)
    {
        p.refs += 1;
        let this = p.this;
        release_export(cpu, id, 1);
        return this;
    }
    let Some(iid) = export(cpu, id).map(|e| guid_of(&e.iid)) else {
        return 0;
    };
    let vtable = match proxy_vtable(cpu, &iid) {
        Some(v) => v,
        None => {
            cpu.bus.log_string(&format!(
                "[W32] OLE32: no proxy for the interface {} (no type library or proxy/stub DLL)",
                iid
            ));
            release_export(cpu, id, 1);
            return 0;
        }
    };
    let this = super::task_alloc(cpu, 8);
    if this == 0 {
        release_export(cpu, id, 1);
        return 0;
    }
    cpu.bus.guest_write_32(this, vtable);
    cpu.bus.guest_write_32(this + 4, id);
    rpc(cpu).proxies.push(Proxy {
        this,
        pid,
        export: id,
        iid: iid.0.to_vec(),
        refs: 1,
    });
    this
}

/// The vtable of the running process's proxies of `iid`, made if it
/// isn't there yet. Each entry is code of its own:
///
/// ```text
/// push <place>
/// call ProxyMethod
/// add esp, 4
/// ret <bytes of the arguments>
/// ```
fn proxy_vtable(cpu: &mut Cpu, iid: &Guid) -> Option<u32> {
    // A proxy/stub DLL's interface has the DLL's proxies.
    if let Some(interface) = ndr::interface(cpu, iid) {
        return Some(interface.proxy_vtable);
    }
    let pid = current_pid(cpu);
    if let Some(v) = rpc(cpu)
        .vtables
        .iter()
        .find(|v| v.pid == pid && v.iid == iid.0)
    {
        return Some(v.address);
    }
    let methods = methods(cpu, iid)?;
    let n = methods.len() as u32;
    let table = super::task_alloc(cpu, n * 4 + n * THUNK);
    if table == 0 {
        return None;
    }
    let target = address_of("ProxyMethod");
    for (i, m) in methods.iter().enumerate() {
        let i = i as u32;
        let code = table + n * 4 + i * THUNK;
        let stack = m.as_ref().map_or(4, Method::stack) as u16;
        let mut bytes = vec![0x68];
        bytes.extend_from_slice(&i.to_le_bytes());
        bytes.push(0xE8);
        bytes.extend_from_slice(&target.wrapping_sub(code + 10).to_le_bytes());
        bytes.extend_from_slice(&[0x83, 0xC4, 0x04, 0xC2]);
        bytes.extend_from_slice(&stack.to_le_bytes());
        cpu.bus.guest_write_bytes(code, &bytes);
        cpu.bus.guest_write_32(table + i * 4, code);
    }
    rpc(cpu).vtables.push(ProxyVtable {
        pid,
        iid: iid.0.to_vec(),
        address: table,
    });
    Some(table)
}

/// The bytes of each entry's code in a proxy vtable.
const THUNK: u32 = 16;

/// A reference to the proxy at `index` goes: with the last, the proxy
/// lets go of its export.
fn release_proxy(cpu: &mut Cpu, index: usize) -> u32 {
    let p = &mut rpc(cpu).proxies[index];
    p.refs = p.refs.saturating_sub(1);
    let refs = p.refs;
    if refs == 0 {
        let p = rpc(cpu).proxies.remove(index);
        if cpu.bus.guest_probe(p.this, 4, true) {
            cpu.bus.guest_write_32(p.this, 0);
        }
        super::task_free(cpu, p.this);
        release_export(cpu, p.export, 1);
    }
    refs
}

// The bytes between the processes.

#[derive(Default)]
struct Wire(Vec<u8>);

impl Wire {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.0.extend_from_slice(b);
    }
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Reader<'a> {
        Reader { data, at: 0 }
    }
    fn take(&mut self, n: usize) -> &'a [u8] {
        let start = self.at.min(self.data.len());
        let end = (start + n).min(self.data.len());
        self.at = start + n;
        &self.data[start..end]
    }
    fn u8(&mut self) -> u8 {
        self.take(1).first().copied().unwrap_or(0)
    }
    fn u16(&mut self) -> u16 {
        let b = self.take(2);
        if b.len() < 2 {
            0
        } else {
            u16::from_le_bytes([b[0], b[1]])
        }
    }
    fn u32(&mut self) -> u32 {
        let b = self.take(4);
        if b.len() < 4 {
            0
        } else {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        }
    }
    fn bytes(&mut self) -> Vec<u8> {
        let n = self.u32() as usize;
        self.take(n.min(self.data.len())).to_vec()
    }
}

/// What copying values does besides: the AddRef and Release calls to
/// make in the process (method and interface), and, on the object's
/// side, the blocks made for the call.
#[derive(Default)]
struct Side {
    guest: Vec<u32>,
    blocks: Vec<u32>,
    /// An interface came that this process can't have: its export is
    /// gone, or nothing marshals its interface here.
    lost: bool,
}

impl Side {
    fn call(&mut self, method: u32, object: u32) {
        if object != 0 {
            self.guest.extend_from_slice(&[method, object]);
        }
    }

    /// A block of `size` zeroed bytes for the call, or 0.
    fn block(&mut self, cpu: &mut Cpu, size: u32) -> u32 {
        let at = super::task_alloc(cpu, size.max(4));
        if at != 0 {
            cpu.bus.guest_fill(at, size.max(4) as usize, 0);
            self.blocks.push(at);
        }
        at
    }
}

fn read32(cpu: &mut Cpu, at: u32) -> u32 {
    if at != 0 && cpu.bus.guest_probe(at, 4, false) {
        cpu.bus.guest_read_32(at)
    } else {
        0
    }
}

fn write32(cpu: &mut Cpu, at: u32, value: u32) {
    if at != 0 && cpu.bus.guest_probe(at, 4, true) {
        cpu.bus.guest_write_32(at, value);
    }
}

fn read_bytes(cpu: &mut Cpu, at: u32, n: u32) -> Vec<u8> {
    let mut b = vec![0; n as usize];
    if at != 0 && cpu.bus.guest_probe(at, n as usize, false) {
        cpu.bus.guest_read_bytes(at, &mut b);
    }
    b
}

fn write_bytes(cpu: &mut Cpu, at: u32, b: &[u8]) {
    if at != 0 && cpu.bus.guest_probe(at, b.len(), true) {
        cpu.bus.guest_write_bytes(at, b);
    }
}

/// The zero-ended string of `unit`-byte characters at `at`, with its
/// zero.
fn string_bytes(cpu: &mut Cpu, at: u32, unit: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < 0x10000 && cpu.bus.guest_probe(at + i, unit as usize, false) {
        let c = read_bytes(cpu, at + i, unit);
        out.extend_from_slice(&c);
        if c.iter().all(|&b| b == 0) {
            break;
        }
        i += unit;
    }
    out
}

// BSTRs.

fn put_bstr(cpu: &mut Cpu, s: u32, w: &mut Wire) {
    if s == 0 {
        w.u8(0);
        return;
    }
    let len = read32(cpu, s.wrapping_sub(4));
    w.u8(1);
    let b = read_bytes(cpu, s, len.min(0x100_0000));
    w.bytes(&b);
}

fn get_bstr(cpu: &mut Cpu, r: &mut Reader) -> u32 {
    if r.u8() == 0 {
        return 0;
    }
    let b = r.bytes();
    crate::win32::oleaut32::bstr_of_bytes(cpu, &b)
}

// Interfaces.

/// Copy the interface pointer `p` as `iid`. With `take` the reference
/// the holder of `p` has goes with it; without, it keeps it.
fn put_iface(cpu: &mut Cpu, p: u32, iid: &Guid, w: &mut Wire, side: &mut Side, take: bool) {
    if p == 0 {
        w.u8(0);
        return;
    }
    let id = if let Some(i) = proxy_at(cpu, p) {
        let id = rpc(cpu).proxies[i].export;
        if let Some(e) = export(cpu, id) {
            e.refs += 1;
        }
        if take {
            release_proxy(cpu, i);
        }
        id
    } else {
        let (id, new) = export_object(cpu, p, iid, true);
        match (new, take) {
            // COM's reference is the one that came with it.
            (true, true) => {}
            (true, false) => side.call(ADD_REF, p),
            (false, true) => side.call(RELEASE, p),
            (false, false) => {}
        }
        id
    };
    w.u8(1);
    w.u32(id);
}

/// An interface pointer copied by `put_iface`, with a reference for its
/// holder.
fn get_iface(cpu: &mut Cpu, r: &mut Reader, side: &mut Side) -> u32 {
    if r.u8() == 0 {
        return 0;
    }
    let id = r.u32();
    let pid = current_pid(cpu);
    let Some(e) = export(cpu, id) else {
        side.lost = true;
        return 0;
    };
    if e.pid != pid {
        let proxy = proxy_of(cpu, id);
        side.lost |= proxy == 0;
        return proxy;
    }
    // The object is of this process: the pointer itself.
    let object = e.object;
    if e.refs == 1 && e.owned {
        // COM's reference goes to the holder.
        rpc(cpu).exports.retain(|e| e.id != id);
    } else {
        side.call(ADD_REF, object);
        release_export(cpu, id, 1);
    }
    object
}

/// Let go of the interface pointer `p` its holder has.
fn drop_iface(cpu: &mut Cpu, p: u32, side: &mut Side) {
    if p == 0 {
        return;
    }
    match proxy_at(cpu, p) {
        Some(i) => {
            release_proxy(cpu, i);
        }
        None => side.call(RELEASE, p),
    }
}

// VARIANTs: the type at 0, the value at 8, or with VT_BYREF a pointer to
// it there.

/// The bytes a value of the type `vt` takes where a pointer of VT_BYREF
/// points.
fn storage(vt: u16) -> u32 {
    match vt & !VT_BYREF {
        16 | 17 => 1,
        2 | 11 | 18 => 2,
        5 | 6 | 7 | 20 | 21 => 8,
        VT_DECIMAL | VT_VARIANT => 16,
        _ => 4,
    }
}

/// Copy the value of the type `vt` at `at` (VT_VARIANT: a VARIANT). For
/// a VARIANT's own value, `at` is its place 8; scalars there take 8
/// bytes.
fn put_typed(cpu: &mut Cpu, vt: u16, at: u32, w: &mut Wire, side: &mut Side, take: bool) {
    if vt & VT_BYREF != 0 {
        let p = read32(cpu, at);
        w.u8((p != 0) as u8);
        if p != 0 {
            put_typed(cpu, vt & !VT_BYREF, p, w, side, take);
        }
        return;
    }
    if vt & VT_ARRAY != 0 {
        let p = read32(cpu, at);
        put_array(cpu, p, w, side, take);
        if take {
            write32(cpu, at, 0);
        }
        return;
    }
    match vt {
        VT_VARIANT => put_variant(cpu, at, w, side, take),
        VT_BSTR => {
            let s = read32(cpu, at);
            put_bstr(cpu, s, w);
            if take {
                crate::win32::oleaut32::free_bstr(cpu, s);
                write32(cpu, at, 0);
            }
        }
        VT_DISPATCH | VT_UNKNOWN => {
            let p = read32(cpu, at);
            let iid = iface_iid(vt);
            put_iface(cpu, p, &iid, w, side, take);
            if take {
                write32(cpu, at, 0);
            }
        }
        _ => {
            let b = read_bytes(cpu, at, storage(vt).max(4));
            w.bytes(&b);
        }
    }
}

fn iface_iid(vt: u16) -> Guid {
    if vt == VT_DISPATCH {
        signature::IID_IDISPATCH
    } else {
        com::IID_IUNKNOWN
    }
}

/// Make a value copied by `put_typed` at `at`.
fn get_typed(cpu: &mut Cpu, vt: u16, at: u32, r: &mut Reader, side: &mut Side) {
    if vt & VT_BYREF != 0 {
        if r.u8() == 0 {
            write32(cpu, at, 0);
            return;
        }
        let base = vt & !VT_BYREF;
        let p = side.block(cpu, storage(base).max(4));
        write32(cpu, at, p);
        get_typed(cpu, base, p, r, side);
        return;
    }
    if vt & VT_ARRAY != 0 {
        let p = get_array(cpu, r, side);
        write32(cpu, at, p);
        return;
    }
    match vt {
        VT_VARIANT => get_variant(cpu, at, r, side),
        VT_BSTR => {
            let s = get_bstr(cpu, r);
            write32(cpu, at, s);
        }
        VT_DISPATCH | VT_UNKNOWN => {
            let p = get_iface(cpu, r, side);
            write32(cpu, at, p);
        }
        _ => {
            let b = r.bytes();
            write_bytes(cpu, at, &b);
        }
    }
}

/// Let go of what the value of the type `vt` at `at` holds.
fn drop_typed(cpu: &mut Cpu, vt: u16, at: u32, side: &mut Side) {
    if vt & VT_BYREF != 0 {
        return;
    }
    if vt & VT_ARRAY != 0 {
        let p = read32(cpu, at);
        destroy_array(cpu, p, side);
        write32(cpu, at, 0);
        return;
    }
    match vt {
        VT_VARIANT => drop_variant(cpu, at, side),
        VT_BSTR => {
            let s = read32(cpu, at);
            crate::win32::oleaut32::free_bstr(cpu, s);
            write32(cpu, at, 0);
        }
        VT_DISPATCH | VT_UNKNOWN => {
            let p = read32(cpu, at);
            drop_iface(cpu, p, side);
            write32(cpu, at, 0);
        }
        _ => {}
    }
}

fn put_variant(cpu: &mut Cpu, at: u32, w: &mut Wire, side: &mut Side, take: bool) {
    let vt = if cpu.bus.guest_probe(at, 16, false) {
        cpu.bus.guest_read_16(at)
    } else {
        VT_EMPTY
    };
    w.u16(vt);
    if vt == VT_DECIMAL {
        let b = read_bytes(cpu, at + 2, 14);
        w.bytes(&b);
        return;
    }
    if vt != VT_EMPTY && vt != VT_NULL {
        put_typed(cpu, vt, at + 8, w, side, take);
    }
    if take {
        write_bytes(cpu, at, &[0; 2]);
    }
}

fn get_variant(cpu: &mut Cpu, at: u32, r: &mut Reader, side: &mut Side) {
    let vt = r.u16();
    write_bytes(cpu, at, &[0; 16]);
    if vt == VT_DECIMAL {
        let b = r.bytes();
        write_bytes(cpu, at + 2, &b);
    } else if vt != VT_EMPTY && vt != VT_NULL {
        get_typed(cpu, vt, at + 8, r, side);
    }
    write_bytes(cpu, at, &vt.to_le_bytes());
}

fn drop_variant(cpu: &mut Cpu, at: u32, side: &mut Side) {
    if !cpu.bus.guest_probe(at, 16, true) {
        return;
    }
    let vt = cpu.bus.guest_read_16(at);
    if vt != VT_DECIMAL {
        drop_typed(cpu, vt, at + 8, side);
    }
    cpu.bus.guest_write_16(at, VT_EMPTY);
}

// SAFEARRAYs.

fn put_array(cpu: &mut Cpu, p: u32, w: &mut Wire, side: &mut Side, take: bool) {
    use crate::win32::oleaut32::safearray;
    // An array the other side can't make again (of records, or of
    // elements whose size isn't their type's) goes as NULL.
    let Some(a) = safearray::describe(cpu, p).filter(|a| safearray::creatable(a.vt, a.element))
    else {
        if p != 0 {
            cpu.bus.log_string(&format!(
                "[W32] OLE32: the SAFEARRAY at {:08X} can't be copied to another process",
                p
            ));
        }
        w.u8(0);
        return;
    };
    w.u8(1);
    w.u16(a.vt);
    w.u32(a.element);
    w.u32(a.bounds.len() as u32);
    for &(n, low) in &a.bounds {
        w.u32(n);
        w.u32(low as u32);
    }
    for i in 0..a.count() {
        let at = a.data + i * a.element;
        match a.vt {
            VT_BSTR | VT_VARIANT | VT_DISPATCH | VT_UNKNOWN => {
                put_typed(cpu, a.vt, at, w, side, take)
            }
            _ => {
                let b = read_bytes(cpu, at, a.element);
                w.bytes(&b);
            }
        }
    }
    if take {
        safearray::free(cpu, p);
    }
}

fn get_array(cpu: &mut Cpu, r: &mut Reader, side: &mut Side) -> u32 {
    use crate::win32::oleaut32::safearray;
    if r.u8() == 0 {
        return 0;
    }
    let vt = r.u16();
    let element = r.u32();
    let dims = r.u32().min(64);
    let bounds: Vec<(u32, i32)> = (0..dims).map(|_| (r.u32(), r.u32() as i32)).collect();
    let Some(a) = safearray::create(cpu, vt, element, &bounds) else {
        // Memory is out: the elements are read all the same, so that
        // the values after the array are read where they are.
        let count = bounds
            .iter()
            .fold(1u64, |n, &(c, _)| n.saturating_mul(c as u64));
        let temp = side.block(cpu, 16);
        for _ in 0..count {
            match vt {
                VT_BSTR | VT_VARIANT | VT_DISPATCH | VT_UNKNOWN => {
                    get_typed(cpu, vt, temp, r, side);
                    drop_typed(cpu, vt, temp, side);
                }
                _ => {
                    r.bytes();
                }
            }
        }
        return 0;
    };
    for i in 0..a.count() {
        let at = a.data + i * a.element;
        match vt {
            VT_BSTR | VT_VARIANT | VT_DISPATCH | VT_UNKNOWN => get_typed(cpu, vt, at, r, side),
            _ => {
                let b = r.bytes();
                write_bytes(cpu, at, &b);
            }
        }
    }
    a.at
}

fn destroy_array(cpu: &mut Cpu, p: u32, side: &mut Side) {
    use crate::win32::oleaut32::safearray;
    let Some(a) = safearray::describe(cpu, p) else {
        return;
    };
    if matches!(a.vt, VT_BSTR | VT_VARIANT | VT_DISPATCH | VT_UNKNOWN) {
        for i in 0..a.count() {
            drop_typed(cpu, a.vt, a.data + i * a.element, side);
        }
    }
    safearray::free(cpu, p);
}

// Parameters.

/// Where each parameter of `m` is, the arguments starting at `args`.
fn places(m: &Method, args: u32) -> Vec<u32> {
    let mut at = args;
    m.params
        .iter()
        .map(|p| {
            let here = at;
            at += p.kind.size();
            here
        })
        .collect()
}

/// The IID the parameter at `index` points to.
fn iid_at(cpu: &mut Cpu, places: &[u32], index: usize) -> Guid {
    let p = places.get(index).map_or(0, |&at| read32(cpu, at));
    Guid::read(cpu, p).unwrap_or(com::IID_IUNKNOWN)
}

/// Copy the value of `kind` at `at`.
fn put_value(
    cpu: &mut Cpu,
    kind: &Kind,
    at: u32,
    places: &[u32],
    w: &mut Wire,
    side: &mut Side,
    take: bool,
) {
    match kind {
        Kind::Word | Kind::Unsupported => w.u32(read32(cpu, at)),
        Kind::Quad => {
            let b = read_bytes(cpu, at, 8);
            w.bytes(&b);
        }
        Kind::Record(n) => {
            let b = read_bytes(cpu, at, *n);
            w.bytes(&b);
        }
        Kind::Bstr => put_typed(cpu, VT_BSTR, at, w, side, take),
        Kind::Variant => put_variant(cpu, at, w, side, take),
        Kind::Array => {
            let p = read32(cpu, at);
            put_array(cpu, p, w, side, take);
        }
        Kind::Str | Kind::WStr => {
            let unit = if *kind == Kind::Str { 1 } else { 2 };
            let p = read32(cpu, at);
            w.u8((p != 0) as u8);
            if p != 0 {
                let b = string_bytes(cpu, p, unit);
                w.bytes(&b);
            }
        }
        Kind::Iface(iid) => {
            let p = read32(cpu, at);
            put_iface(cpu, p, iid, w, side, take);
        }
        Kind::IidIs(i) => {
            let iid = iid_at(cpu, places, *i);
            let p = read32(cpu, at);
            put_iface(cpu, p, &iid, w, side, take);
        }
        Kind::Ptr(inner) => {
            let p = read32(cpu, at);
            w.u8((p != 0) as u8);
            if p != 0 {
                put_value(cpu, inner, p, places, w, side, take);
            }
        }
        // Only at the top of a method's parameters.
        Kind::Names(_) | Kind::Ids(_) | Kind::DispParams | Kind::ExcepInfo => {}
    }
}

/// Make a value copied by `put_value` at `at`.
fn get_value(cpu: &mut Cpu, kind: &Kind, at: u32, r: &mut Reader, side: &mut Side) {
    match kind {
        Kind::Word | Kind::Unsupported => write32(cpu, at, r.u32()),
        Kind::Quad | Kind::Record(_) => {
            let b = r.bytes();
            write_bytes(cpu, at, &b);
        }
        Kind::Bstr => get_typed(cpu, VT_BSTR, at, r, side),
        Kind::Variant => get_variant(cpu, at, r, side),
        Kind::Array => {
            let p = get_array(cpu, r, side);
            write32(cpu, at, p);
        }
        Kind::Str | Kind::WStr => {
            let p = if r.u8() != 0 {
                let b = r.bytes();
                let p = side.block(cpu, b.len() as u32 + 2);
                write_bytes(cpu, p, &b);
                p
            } else {
                0
            };
            write32(cpu, at, p);
        }
        Kind::Iface(_) | Kind::IidIs(_) => {
            let p = get_iface(cpu, r, side);
            write32(cpu, at, p);
        }
        Kind::Ptr(inner) => {
            if r.u8() == 0 {
                write32(cpu, at, 0);
                return;
            }
            let p = side.block(cpu, inner.size());
            write32(cpu, at, p);
            get_value(cpu, inner, p, r, side);
        }
        Kind::Names(_) | Kind::Ids(_) | Kind::DispParams | Kind::ExcepInfo => {}
    }
}

/// Let go of what the value of `kind` at `at` holds.
fn drop_value(cpu: &mut Cpu, kind: &Kind, at: u32, side: &mut Side) {
    match kind {
        Kind::Bstr => drop_typed(cpu, VT_BSTR, at, side),
        Kind::Variant => drop_variant(cpu, at, side),
        Kind::Array => {
            let p = read32(cpu, at);
            destroy_array(cpu, p, side);
            write32(cpu, at, 0);
        }
        Kind::Iface(_) | Kind::IidIs(_) => {
            let p = read32(cpu, at);
            drop_iface(cpu, p, side);
            write32(cpu, at, 0);
        }
        Kind::Ptr(inner) => {
            let p = read32(cpu, at);
            if p != 0 {
                drop_value(cpu, inner, p, side);
            }
        }
        _ => {}
    }
}

// DISPPARAMS: the arguments (VARIANTs, the last first), the DISPIDs of
// the named ones, and their counts.

const DP_ARGS: u32 = 0;
const DP_NAMED: u32 = 4;
const DP_COUNT: u32 = 8;
const DP_NAMED_COUNT: u32 = 12;

/// The most arguments of an Invoke that are copied.
const MAX_ARGS: u32 = 1024;

fn put_disp_params(cpu: &mut Cpu, p: u32, w: &mut Wire, side: &mut Side) {
    w.u8((p != 0) as u8);
    if p == 0 {
        return;
    }
    let args = read32(cpu, p + DP_ARGS);
    let named = read32(cpu, p + DP_NAMED);
    let count = read32(cpu, p + DP_COUNT).min(MAX_ARGS);
    let named_count = read32(cpu, p + DP_NAMED_COUNT).min(MAX_ARGS);
    w.u32(count);
    for i in 0..count {
        put_variant(cpu, args + 16 * i, w, side, false);
    }
    w.u32(named_count);
    for i in 0..named_count {
        w.u32(read32(cpu, named + 4 * i));
    }
}

fn get_disp_params(cpu: &mut Cpu, r: &mut Reader, side: &mut Side) -> u32 {
    if r.u8() == 0 {
        return 0;
    }
    let p = side.block(cpu, 16);
    let count = r.u32().min(MAX_ARGS);
    let args = side.block(cpu, 16 * count);
    for i in 0..count {
        get_variant(cpu, args + 16 * i, r, side);
    }
    let named_count = r.u32().min(MAX_ARGS);
    let named = side.block(cpu, 4 * named_count);
    for i in 0..named_count {
        write32(cpu, named + 4 * i, r.u32());
    }
    write32(cpu, p + DP_ARGS, args);
    write32(cpu, p + DP_NAMED, named);
    write32(cpu, p + DP_COUNT, count);
    write32(cpu, p + DP_NAMED_COUNT, named_count);
    p
}

/// After the call, on the object's side: what the arguments passed by
/// reference hold now goes back, and the arguments go.
fn put_disp_params_back(cpu: &mut Cpu, p: u32, w: &mut Wire, side: &mut Side) {
    if p == 0 {
        return;
    }
    let args = read32(cpu, p + DP_ARGS);
    let count = read32(cpu, p + DP_COUNT).min(MAX_ARGS);
    for i in 0..count {
        let at = args + 16 * i;
        let vt = cpu.bus.guest_read_16(at);
        if vt & VT_BYREF != 0 {
            let target = read32(cpu, at + 8);
            if target != 0 {
                put_typed(cpu, vt & !VT_BYREF, target, w, side, true);
            }
        } else {
            drop_variant(cpu, at, side);
        }
    }
}

/// On the caller's side: the values of its arguments passed by
/// reference become those the method left in them.
fn get_disp_params_back(cpu: &mut Cpu, p: u32, r: &mut Reader, side: &mut Side) {
    if p == 0 {
        return;
    }
    let args = read32(cpu, p + DP_ARGS);
    let count = read32(cpu, p + DP_COUNT).min(MAX_ARGS);
    for i in 0..count {
        let at = args + 16 * i;
        let vt = cpu.bus.guest_read_16(at);
        if vt & VT_BYREF == 0 {
            continue;
        }
        let target = read32(cpu, at + 8);
        if target != 0 {
            let base = vt & !VT_BYREF;
            drop_typed(cpu, base, target, side);
            get_typed(cpu, base, target, r, side);
        }
    }
}

// EXCEPINFO: wCode, wReserved, three BSTRs, the help context, two
// pointers and the SCODE.

const EXCEPINFO: u32 = 32;

fn put_excep_info(cpu: &mut Cpu, p: u32, w: &mut Wire) {
    w.u8((p != 0) as u8);
    if p == 0 {
        return;
    }
    let head = read_bytes(cpu, p, 4);
    w.bytes(&head);
    for at in [4, 8, 12] {
        let s = read32(cpu, p + at);
        put_bstr(cpu, s, w);
        crate::win32::oleaut32::free_bstr(cpu, s);
    }
    w.u32(read32(cpu, p + 16));
    w.u32(read32(cpu, p + 28));
}

fn get_excep_info(cpu: &mut Cpu, p: u32, r: &mut Reader) {
    if r.u8() == 0 || p == 0 {
        return;
    }
    let head = r.bytes();
    write_bytes(cpu, p, &[0; EXCEPINFO as usize]);
    write_bytes(cpu, p, &head);
    for at in [4, 8, 12] {
        let s = get_bstr(cpu, r);
        write32(cpu, p + at, s);
    }
    let context = r.u32();
    write32(cpu, p + 16, context);
    let scode = r.u32();
    write32(cpu, p + 28, scode);
}

// The caller's side: a method of a proxy.

/// ProxyMethod: a method of a proxy, called from its vtable's code with
/// the method's place above the return address, then the caller's
/// return address, `this` and the arguments.
pub fn proxy_method(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let esp = cpu.esp();
    let index = read32(cpu, esp + 4);
    let this = read32(cpu, esp + 12);
    proxy_call(cpu, this, index, esp + 16)
}

/// The method at `index` of the proxy `this`, its arguments after
/// `this` at `args`. A proxy/stub DLL's proxies come here for
/// IUnknown's methods (ndr.rs).
fn proxy_call(cpu: &mut Cpu, this: u32, index: u32, args: u32) -> Option<u32> {
    let Some(i) = proxy_at(cpu, this) else {
        return Some(RPC_E_DISCONNECTED);
    };
    let p = rpc(cpu).proxies[i].clone();
    let iid = guid_of(&p.iid);
    match index {
        ADD_REF => {
            let p = &mut rpc(cpu).proxies[i];
            p.refs += 1;
            return Some(p.refs);
        }
        RELEASE => return Some(release_proxy(cpu, i)),
        QUERY_INTERFACE => {
            let out = read32(cpu, args + 4);
            let wanted_at = read32(cpu, args);
            let wanted = Guid::read(cpu, wanted_at);
            if out == 0 {
                return Some(com::E_POINTER);
            }
            write32(cpu, out, 0);
            let Some(wanted) = wanted else {
                return Some(E_INVALIDARG);
            };
            if wanted == iid {
                rpc(cpu).proxies[i].refs += 1;
                write32(cpu, out, this);
                return Some(S_OK);
            }
        }
        _ => {}
    }
    let Some(m) = method_of(cpu, &iid, index) else {
        return Some(E_NOTIMPL);
    };
    let m = &m;
    // The type information isn't copied: the caller reads the type
    // library itself.
    if index == GET_TYPE_INFO && m.params.last().map(|p| &p.kind) == Some(&unsupported_out()) {
        let out = read32(cpu, args + 8);
        if out != 0 {
            write32(cpu, out, 0);
        }
        return Some(E_NOTIMPL);
    }
    let Some(e) = export(cpu, p.export).cloned() else {
        return Some(RPC_E_DISCONNECTED);
    };
    let places = places(m, args);
    // A parameter of a type the marshaler doesn't copy (a structure with
    // pointers, a void*) fails the call when it holds anything: a
    // pointer of this process means nothing in the object's, as Windows'
    // marshaler fails on a type it can't copy.
    let uncopied = m
        .params
        .iter()
        .zip(&places)
        .position(|(p, &at)| p.kind == Kind::Unsupported && read32(cpu, at) != 0);
    if let Some(i) = uncopied {
        cpu.bus.log_string(&format!(
            "[W32] OLE32: method {} of {}: its parameter {} is of a type that isn't copied",
            index, iid, i
        ));
        return Some(DISP_E_BADVARTYPE);
    }
    // The proxy/stub DLLs of the interfaces that come back are loaded
    // before the call, which runs again.
    let iids = out_iids(cpu, m, &places);
    if let Some(loading) = ndr::load(cpu, &iids) {
        return loading;
    }
    let mut w = Wire::default();
    let mut side = Side::default();
    w.u32(e.id);
    w.u32(index);
    for (param, &at) in m.params.iter().zip(&places) {
        put_in(cpu, param, at, &places, &mut w, &mut side);
    }
    let id = next_id(cpu);
    rpc(cpu).calls.push(Pending {
        id,
        iid: iid.0.to_vec(),
        request: w.0,
        guest: side.guest,
        ..Pending::default()
    });
    let vars = vec![calls::PROXY, id, index, args, e.hwnd];
    calls::start(cpu, vars, 0)
}

fn unsupported_out() -> Kind {
    Kind::Ptr(Box::new(Kind::Unsupported))
}

/// Copy the parameter at `at` on the caller's side, before the call.
fn put_in(
    cpu: &mut Cpu,
    param: &signature::Param,
    at: u32,
    places: &[u32],
    w: &mut Wire,
    side: &mut Side,
) {
    match &param.kind {
        Kind::Ptr(inner) => {
            let p = read32(cpu, at);
            w.u8((p != 0) as u8);
            if p != 0 && param.is_in() {
                put_value(cpu, inner, p, places, w, side, false);
            }
        }
        Kind::Names(count) => {
            let p = read32(cpu, at);
            let n = places
                .get(*count)
                .map_or(0, |&c| read32(cpu, c))
                .min(MAX_ARGS);
            w.u32(n);
            for i in 0..n {
                let name = read32(cpu, p + 4 * i);
                let b = string_bytes(cpu, name, 2);
                w.bytes(&b);
            }
        }
        Kind::Ids(_) | Kind::ExcepInfo => w.u8((read32(cpu, at) != 0) as u8),
        Kind::DispParams => {
            let p = read32(cpu, at);
            put_disp_params(cpu, p, w, side);
        }
        kind if param.is_in() => put_value(cpu, kind, at, places, w, side, false),
        _ => {}
    }
}

/// Write the values the method gave back for the parameter at `at` on
/// the caller's side.
fn get_out(cpu: &mut Cpu, param: &signature::Param, at: u32, r: &mut Reader, side: &mut Side) {
    match &param.kind {
        Kind::Ptr(inner) if param.is_out() => {
            let p = read32(cpu, at);
            if r.u8() == 0 || p == 0 {
                return;
            }
            if param.is_in() {
                drop_value(cpu, inner, p, side);
            }
            get_value(cpu, inner, p, r, side);
        }
        Kind::Ids(_) => {
            let p = read32(cpu, at);
            let n = r.u32().min(MAX_ARGS);
            for i in 0..n {
                let id = r.u32();
                if p != 0 {
                    write32(cpu, p + 4 * i, id);
                }
            }
        }
        Kind::DispParams => {
            let p = read32(cpu, at);
            get_disp_params_back(cpu, p, r, side);
        }
        Kind::ExcepInfo => {
            let p = read32(cpu, at);
            get_excep_info(cpu, p, r);
        }
        _ => {}
    }
}

/// The proxy method's task: variables 1 to 4 the call's id, the
/// method's place, where its arguments are and the window the call goes
/// to.
const P_ID: usize = 1;
const P_INDEX: usize = 2;
const P_ARGS: usize = 3;
const P_HWND: usize = 4;
const P_RESULT: usize = 5;

pub fn proxy_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let id = t.var(P_ID);
    loop {
        match t.step {
            // AddRef of the interfaces passed, then the call.
            0 => {
                if let Some(next) = guest_call(cpu, id, 0) {
                    return next;
                }
                let send = user32::address_of("SendMessageA");
                return Next::call(send, &[t.var(P_HWND), WM_OLE_CALL, id, 0], 1);
            }
            1 => {
                let Some(i) = rpc(cpu).calls.iter().position(|c| c.id == id) else {
                    return Next::Done(RPC_E_SERVER_DIED);
                };
                let call = &rpc(cpu).calls[i];
                if !call.replied {
                    rpc(cpu).calls.remove(i);
                    return Next::Done(RPC_E_SERVER_DIED);
                }
                let reply = call.reply.clone();
                let mut r = Reader::new(&reply);
                let result = r.u32();
                t.set(P_RESULT, result);
                let iid = guid_of(&call.iid);
                let index = t.var(P_INDEX);
                cpu.bus.log_string(&format!(
                    "[W32] OLE32: method {} of {} in another process = {:08X}",
                    index, iid, result
                ));
                let mut side = Side::default();
                if let Some(m) = method_of(cpu, &iid, index) {
                    let places = places(&m, t.var(P_ARGS));
                    for (param, &at) in m.params.iter().zip(&places) {
                        get_out(cpu, param, at, &mut r, &mut side);
                    }
                }
                // An interface that came back and can't be had here
                // fails the call, as its unmarshalling fails it on
                // Windows; the caller gets NULL for it.
                if side.lost && result & 0x8000_0000 == 0 {
                    t.set(P_RESULT, E_NOINTERFACE);
                }
                if let Some(c) = rpc(cpu).calls.iter_mut().find(|c| c.id == id) {
                    c.guest = side.guest;
                }
                t.step = 2;
            }
            // Release of what the caller's [in, out] values held.
            2 => {
                if let Some(next) = guest_call(cpu, id, 2) {
                    return next;
                }
                rpc(cpu).calls.retain(|c| c.id != id);
                return Next::Done(t.var(P_RESULT));
            }
            _ => {
                let _ = value;
                return Next::Done(t.var(P_RESULT));
            }
        }
    }
}

/// The next AddRef or Release the call `id` has left to make, as a call
/// that goes on at `step`; None when there are none left.
fn guest_call(cpu: &mut Cpu, id: u32, step: u16) -> Option<Next> {
    let call = rpc(cpu).calls.iter_mut().find(|c| c.id == id)?;
    if call.guest.len() < 2 {
        return None;
    }
    let (method, object) = (call.guest[0], call.guest[1]);
    call.guest.drain(..2);
    match super::method(cpu, object, method) {
        Some(m) => Some(Next::call(m, &[object], step)),
        None => Some(Next::call(0, &[], step)),
    }
}

// The object's side: the apartment window.

/// OleMainWndProc(hwnd, message, wParam, lParam): the apartment
/// window's procedure. A call runs the object's method; a reference to
/// let go of is released.
pub fn wnd_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[_, msg, wp, _] = args else {
        return Some(0);
    };
    match msg {
        WM_OLE_CALL => {
            let request = rpc(cpu)
                .calls
                .iter()
                .find(|c| c.id == wp)
                .map(|c| c.request.clone())
                .unwrap_or_default();
            let ndr = ndr::is_ndr_call(&request);
            // The proxy/stub DLLs of the interface called and of those
            // passed in are loaded first, and the call comes again.
            let iids = if ndr {
                ndr::request_iid(cpu, &request).into_iter().collect()
            } else {
                call_method(cpu, &request).map_or(Vec::new(), |(_, _, m)| in_iids(&m))
            };
            if let Some(loading) = ndr::load(cpu, &iids) {
                return loading;
            }
            if ndr {
                return ndr::start_stub(cpu, wp);
            }
            let vars = vec![calls::STUB, wp];
            calls::start(cpu, vars, 0)
        }
        WM_OLE_RELEASE => match super::method(cpu, wp, RELEASE) {
            Some(release) => super::call_all(cpu, 0, &[(release, wp)]),
            None => Some(0),
        },
        _ => user32::defwnd::def_window_proc(cpu, args),
    }
}

/// The stub's task: variable 1 the call's id.
const S_ID: usize = 1;

pub fn stub_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let id = t.var(S_ID);
    loop {
        match t.step {
            0 => match unpack_call(cpu, id) {
                Ok(()) => t.step = 1,
                Err(result) => {
                    reply(cpu, id, result, Wire::default());
                    return Next::Done(0);
                }
            },
            // AddRef of the caller's own objects it passed, then the
            // method.
            1 => {
                if let Some(next) = guest_call(cpu, id, 1) {
                    return next;
                }
                let Some(call) = rpc(cpu).calls.iter().find(|c| c.id == id) else {
                    return Next::Done(0);
                };
                let (image, request) = (call.image, call.request.clone());
                let mut r = Reader::new(&request);
                let export_id = r.u32();
                let index = r.u32();
                let Some(e) = export(cpu, export_id).cloned() else {
                    reply(cpu, id, RPC_E_DISCONNECTED, Wire::default());
                    return Next::Done(0);
                };
                let iid = guid_of(&e.iid);
                let stack = method_of(cpu, &iid, index).map_or(4, |m| m.stack());
                let mut args = vec![e.object];
                for i in 0..(stack - 4) / 4 {
                    args.push(read32(cpu, image + 4 * i));
                }
                let vtable = read32(cpu, e.object);
                let method = read32(cpu, vtable + 4 * index);
                return Next::call(method, &args, 2);
            }
            2 => {
                pack_reply(cpu, id, value);
                t.step = 3;
            }
            // Release of what the arguments held.
            3 => {
                if let Some(next) = guest_call(cpu, id, 3) {
                    return next;
                }
                if let Some(call) = rpc(cpu).calls.iter_mut().find(|c| c.id == id) {
                    let blocks = std::mem::take(&mut call.blocks);
                    let image = std::mem::take(&mut call.image);
                    super::task_free(cpu, image);
                    for b in blocks {
                        super::task_free(cpu, b);
                    }
                }
                return Next::Done(0);
            }
            _ => return Next::Done(0),
        }
    }
}

/// The method of the call `id`, and its export's interface.
fn call_method(cpu: &mut Cpu, request: &[u8]) -> Result<(Export, u32, Method), u32> {
    let mut r = Reader::new(request);
    let export_id = r.u32();
    let index = r.u32();
    let e = export(cpu, export_id).cloned().ok_or(RPC_E_DISCONNECTED)?;
    if e.pid != current_pid(cpu) {
        return Err(RPC_E_DISCONNECTED);
    }
    let iid = guid_of(&e.iid);
    match method_of(cpu, &iid, index) {
        Some(m) => Ok((e, index, m)),
        None if methods(cpu, &iid).is_none() => Err(E_NOINTERFACE),
        None => Err(E_NOTIMPL),
    }
}

/// Make the arguments of the call `id` in this process's memory.
fn unpack_call(cpu: &mut Cpu, id: u32) -> Result<(), u32> {
    let request = rpc(cpu)
        .calls
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.request.clone())
        .ok_or(RPC_E_DISCONNECTED)?;
    let (_, _, m) = call_method(cpu, &request)?;
    let mut r = Reader::new(&request);
    r.u32();
    r.u32();
    let size = m.stack() - 4;
    let image = super::task_alloc(cpu, size.max(4));
    if image == 0 {
        return Err(E_OUTOFMEMORY);
    }
    cpu.bus.guest_fill(image, size.max(4) as usize, 0);
    let places = places(&m, image);
    let mut side = Side::default();
    for (param, &at) in m.params.iter().zip(&places) {
        get_in(cpu, param, at, &places, &mut r, &mut side);
    }
    let call = rpc(cpu)
        .calls
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or(RPC_E_DISCONNECTED)?;
    call.image = image;
    call.blocks = side.blocks;
    call.guest = side.guest;
    Ok(())
}

/// Make the parameter at `at` on the object's side.
fn get_in(
    cpu: &mut Cpu,
    param: &signature::Param,
    at: u32,
    places: &[u32],
    r: &mut Reader,
    side: &mut Side,
) {
    match &param.kind {
        Kind::Ptr(inner) => {
            if r.u8() == 0 {
                write32(cpu, at, 0);
                return;
            }
            let p = side.block(cpu, inner.size());
            write32(cpu, at, p);
            if param.is_in() {
                get_value(cpu, inner, p, r, side);
            }
        }
        Kind::Names(_) => {
            let n = r.u32().min(MAX_ARGS);
            let list = side.block(cpu, 4 * n);
            for i in 0..n {
                let b = r.bytes();
                let name = side.block(cpu, b.len() as u32 + 2);
                write_bytes(cpu, name, &b);
                write32(cpu, list + 4 * i, name);
            }
            write32(cpu, at, list);
        }
        Kind::Ids(count) => {
            let n = places
                .get(*count)
                .map_or(0, |&c| read32(cpu, c))
                .min(MAX_ARGS);
            let p = if r.u8() != 0 {
                side.block(cpu, 4 * n)
            } else {
                0
            };
            write32(cpu, at, p);
        }
        Kind::ExcepInfo => {
            let p = if r.u8() != 0 {
                side.block(cpu, EXCEPINFO)
            } else {
                0
            };
            write32(cpu, at, p);
        }
        Kind::DispParams => {
            let p = get_disp_params(cpu, r, side);
            write32(cpu, at, p);
        }
        kind if param.is_in() => get_value(cpu, kind, at, r, side),
        _ => {}
    }
}

/// The method of the call `id` returned `result`: what it gave back
/// goes into the reply, and the arguments are let go of.
fn pack_reply(cpu: &mut Cpu, id: u32, result: u32) {
    let Some((request, image)) = rpc(cpu)
        .calls
        .iter()
        .find(|c| c.id == id)
        .map(|c| (c.request.clone(), c.image))
    else {
        return;
    };
    let mut w = Wire::default();
    let mut side = Side::default();
    if let Ok((_, _, m)) = call_method(cpu, &request) {
        let places = places(&m, image);
        for (param, &at) in m.params.iter().zip(&places) {
            put_out(cpu, param, at, &places, &mut w, &mut side);
        }
    }
    if let Some(call) = rpc(cpu).calls.iter_mut().find(|c| c.id == id) {
        call.guest = side.guest;
        call.blocks.extend(side.blocks);
    }
    reply(cpu, id, result, w);
}

/// Copy what the method left in the parameter at `at` on the object's
/// side, and let go of it.
fn put_out(
    cpu: &mut Cpu,
    param: &signature::Param,
    at: u32,
    places: &[u32],
    w: &mut Wire,
    side: &mut Side,
) {
    match &param.kind {
        Kind::Ptr(inner) => {
            let p = read32(cpu, at);
            if param.is_out() {
                w.u8((p != 0) as u8);
                if p != 0 {
                    put_value(cpu, inner, p, places, w, side, true);
                }
            } else if p != 0 {
                drop_value(cpu, inner, p, side);
            }
        }
        Kind::Ids(count) => {
            let p = read32(cpu, at);
            let n = if p == 0 {
                0
            } else {
                places
                    .get(*count)
                    .map_or(0, |&c| read32(cpu, c))
                    .min(MAX_ARGS)
            };
            w.u32(n);
            for i in 0..n {
                w.u32(read32(cpu, p + 4 * i));
            }
        }
        Kind::DispParams => {
            let p = read32(cpu, at);
            put_disp_params_back(cpu, p, w, side);
        }
        Kind::ExcepInfo => {
            let p = read32(cpu, at);
            put_excep_info(cpu, p, w);
        }
        Kind::Names(_) => {}
        kind if param.is_in() => drop_value(cpu, kind, at, side),
        _ => {}
    }
}

/// The reply to the call `id`: `result`, then what `w` has.
fn reply(cpu: &mut Cpu, id: u32, result: u32, w: Wire) {
    let mut bytes = result.to_le_bytes().to_vec();
    bytes.extend(w.0);
    if let Some(call) = rpc(cpu).calls.iter_mut().find(|c| c.id == id) {
        call.reply = bytes;
        call.replied = true;
    }
}

// Local servers.

/// A class object another process registered for `clsid` that a client
/// of this one may use: its owner's (pid, tid) and the object, after
/// `compobj::local_class` found it.
pub struct Remote {
    pub pid: u32,
    pub tid: u32,
    pub object: u32,
}

/// The activation task's variables: whether to make an object, the
/// CLSID, IID and out pointer, the server's process, the time its wait
/// ends, and the class object's proxy.
const A_CREATE: usize = 1;
const A_CLSID: usize = 2;
const A_IID: usize = 3;
const A_OUT: usize = 4;
const A_PID: usize = 5;
const A_DEADLINE_LO: usize = 6;
const A_DEADLINE_HI: usize = 7;
const A_FACTORY: usize = 8;
/// Its scratch space: the command line, STARTUPINFO, PROCESS_INFORMATION.
const S_LINE: u32 = 0;
const LINE: u32 = 520;
const S_STARTUP: u32 = LINE;
const S_PROCESS: u32 = LINE + crate::win32::kernel32::STARTUPINFO_SIZE;
const ACTIVATE_SCRATCH: u32 = S_PROCESS + 16;

/// CoCreateInstance (with `create`) or CoGetClassObject of a class of a
/// local server: `clsid`, `iid` and `out` are the caller's pointers.
pub fn activate(
    cpu: &mut Cpu,
    create: bool,
    clsid: u32,
    iid: u32,
    out: u32,
    outer: u32,
) -> Option<u32> {
    if create && outer != 0 {
        return Some(CLASS_E_NOAGGREGATION);
    }
    // The proxy/stub DLL of the interface asked for is loaded first, and
    // the function runs again.
    if let Some(wanted) = Guid::read(cpu, iid)
        && let Some(loading) = ndr::load(cpu, &[wanted])
    {
        return loading;
    }
    let vars = vec![calls::ACTIVATE, create as u32, clsid, iid, out];
    calls::start(cpu, vars, ACTIVATE_SCRATCH)
}

pub fn activate_step(cpu: &mut Cpu, t: &mut Task, value: u32) -> Next {
    let s = t.scratch;
    let Some(clsid) = Guid::read(cpu, t.var(A_CLSID)) else {
        return Next::Done(E_INVALIDARG);
    };
    loop {
        match t.step {
            0 => {
                if super::compobj::local_class(cpu, &clsid).is_some() {
                    t.step = 10;
                    continue;
                }
                let path = format!("CLSID\\{}\\LocalServer32", clsid);
                let Some(command) = super::compobj::class_value(cpu, &path) else {
                    return Next::Done(com::REGDB_E_CLASSNOTREG);
                };
                let line = format!("{} -Embedding", command.trim_end());
                cpu.bus.log_string(&format!(
                    "[W32] OLE32: starting the local server of {}: {}",
                    clsid, line
                ));
                let mut bytes = crate::dosstr::to_bytes(&line);
                bytes.truncate(LINE as usize - 1);
                bytes.push(0);
                cpu.bus.guest_write_bytes(s + S_LINE, &bytes);
                cpu.bus
                    .guest_write_32(s + S_STARTUP, crate::win32::kernel32::STARTUPINFO_SIZE);
                let create = crate::win32::kernel32::address_of("CreateProcessA");
                let args = [
                    0,
                    s + S_LINE,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    s + S_STARTUP,
                    s + S_PROCESS,
                ];
                return Next::call(create, &args, 1);
            }
            1 => {
                if value == 0 {
                    return Next::Done(CO_E_SERVER_EXEC_FAILURE);
                }
                t.set(A_PID, read32(cpu, s + S_PROCESS + 8));
                let deadline = cpu.bus.clock.now_micros() + SERVER_WAIT;
                t.set(A_DEADLINE_LO, deadline as u32);
                t.set(A_DEADLINE_HI, (deadline >> 32) as u32);
                let close = crate::win32::kernel32::address_of("CloseHandle");
                return Next::call(close, &[read32(cpu, s + S_PROCESS)], 2);
            }
            2 => {
                let close = crate::win32::kernel32::address_of("CloseHandle");
                return Next::call(close, &[read32(cpu, s + S_PROCESS + 4)], 3);
            }
            // Wait for the server's class object.
            3 => {
                if super::compobj::local_class(cpu, &clsid).is_some() {
                    t.step = 10;
                    continue;
                }
                let pid = t.var(A_PID);
                let alive = cpu
                    .bus
                    .win32
                    .processes
                    .iter()
                    .any(|p| p.id == pid && p.threads.iter().any(|t| t.exit_code.is_none()));
                let deadline = t.var(A_DEADLINE_LO) as u64 | (t.var(A_DEADLINE_HI) as u64) << 32;
                if !alive || cpu.bus.clock.now_micros() >= deadline {
                    cpu.bus.log_string(&format!(
                        "[W32] OLE32: the local server of {} didn't register its class object",
                        clsid
                    ));
                    return Next::Done(CO_E_SERVER_EXEC_FAILURE);
                }
                let sleep = crate::win32::kernel32::address_of("Sleep");
                return Next::call(sleep, &[10], 3);
            }
            10 => {
                let Some(remote) = super::compobj::local_class(cpu, &clsid) else {
                    return Next::Done(CO_E_SERVER_EXEC_FAILURE);
                };
                super::compobj::class_used(cpu, &clsid, remote.pid);
                let factory = factory_proxy(cpu, &remote);
                if factory == 0 {
                    return Next::Done(CO_E_SERVER_EXEC_FAILURE);
                }
                t.set(A_FACTORY, factory);
                let vtable = read32(cpu, factory);
                let (method, args) = if t.var(A_CREATE) != 0 {
                    let m = read32(cpu, vtable + 12);
                    (m, vec![factory, 0, t.var(A_IID), t.var(A_OUT)])
                } else {
                    let m = read32(cpu, vtable);
                    (m, vec![factory, t.var(A_IID), t.var(A_OUT)])
                };
                return Next::call(method, &args, 11);
            }
            _ => {
                let factory = t.var(A_FACTORY);
                if let Some(i) = proxy_at(cpu, factory) {
                    release_proxy(cpu, i);
                }
                return Next::Done(value);
            }
        }
    }
}

/// A proxy in the running process of the class object `remote`, as
/// IClassFactory.
fn factory_proxy(cpu: &mut Cpu, remote: &Remote) -> u32 {
    let iid = com::IID_ICLASSFACTORY;
    let found = rpc(cpu)
        .exports
        .iter_mut()
        .find(|e| e.pid == remote.pid && e.object == remote.object && e.iid == iid.0);
    let id = match found {
        Some(e) => {
            e.refs += 1;
            e.id
        }
        None => {
            let Some(hwnd) = window_of(cpu, remote.tid) else {
                return 0;
            };
            let id = next_id(cpu);
            rpc(cpu).exports.push(Export {
                id,
                pid: remote.pid,
                tid: remote.tid,
                hwnd,
                object: remote.object,
                iid: iid.0.to_vec(),
                refs: 1,
                owned: false,
            });
            id
        }
    };
    proxy_of(cpu, id)
}

/// The class object `object` of the running process is revoked: an
/// export of it holds a reference of its own from now on, the one the
/// registration had, so the proxies of it go on working.
pub fn class_revoked(cpu: &mut Cpu, object: u32) -> bool {
    let pid = current_pid(cpu);
    match rpc(cpu)
        .exports
        .iter_mut()
        .find(|e| e.pid == pid && e.object == object && !e.owned)
    {
        Some(e) => {
            e.owned = true;
            true
        }
        None => false,
    }
}

crate::state_fields!(Export {
    id,
    pid,
    tid,
    hwnd,
    object,
    iid,
    refs,
    owned
});
crate::state_fields!(Proxy {
    this,
    pid,
    export,
    iid,
    refs
});
crate::state_fields!(ProxyVtable { pid, iid, address });
crate::state_fields!(Window { tid, pid, hwnd });
crate::state_fields!(Pending {
    id,
    iid,
    request,
    reply,
    replied,
    guest,
    image,
    blocks
});
crate::state_fields!(Rpc {
    exports,
    proxies,
    vtables,
    windows,
    calls,
    next_id,
    ps,
    stub_buffers,
    methods
});
