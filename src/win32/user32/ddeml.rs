//! The DDE Management Library (DDEML) and the helpers of raw DDE
//! messages (PackDDElParam and the rest).
//!
//! DDEML's string handles, data handles, instances and conversations
//! are the system's, kept here. A data handle's bytes are kept here too,
//! and DdeAccessData gives the process that asks a copy in its own
//! memory, which DdeUnaccessData puts back: a handle passes from one
//! process to another as Windows passes it.
//!
//! The servers a client can talk to are the shell's own: Program
//! Manager's ("PROGMAN"), which setup programs ask to make Start Menu
//! groups and items (SHELL32's progman.rs); programs that talk to it in
//! raw DDE messages reach it through its window (user32/progman.rs).
//! DdeNameService registers a program's services, but conversations
//! with servers that are programs aren't made: DdeConnect fails for
//! them with DMLERR_NO_CONV_ESTABLISHED.
//!
//! A transaction with TIMEOUT_ASYNC is done at once too, but the client
//! hears of it as Windows tells it: DdeClientTransaction gives TRUE and
//! the transaction's id, and the instance's callback gets
//! XTYP_XACT_COMPLETE later, from the client's message loop. A hidden
//! window of the client's thread gets a message for it, which its
//! procedure (`wnd_proc`) turns into the call.
//!
//! The 16-bit DDEML (win16/ddeml.rs) goes on to these functions, so a
//! 16-bit program's instances, handles and conversations are kept here
//! with the 32-bit ones'.

use super::{current_tid, string_at};
use crate::cpu::Cpu;
use crate::savestate::Since;
use crate::win32::task::{self, Next, Task};

const DMLERR_NO_ERROR: u32 = 0;
const DMLERR_DLL_NOT_INITIALIZED: u32 = 0x4003;
const DMLERR_INVALIDPARAMETER: u32 = 0x4006;
const DMLERR_MEMORY_ERROR: u32 = 0x4008;
const DMLERR_NOTPROCESSED: u32 = 0x4009;
const DMLERR_NO_CONV_ESTABLISHED: u32 = 0x400A;
const DMLERR_UNFOUND_QUEUE_ID: u32 = 0x4011;

const XTYP_EXECUTE: u32 = 0x4050;
const XTYP_REQUEST: u32 = 0x20B0;
pub const XTYP_XACT_COMPLETE: u32 = 0x8080;
/// The timeout of an asynchronous transaction.
const TIMEOUT_ASYNC: u32 = 0xFFFF_FFFF;
/// DdeSetUserHandle's and DdeQueryConvInfo's id for the conversation
/// itself, not one of its transactions.
const QID_SYNC: u32 = 0xFFFF_FFFF;
/// The flags of a transaction type (XTYPF_NOBLOCK and the rest).
const XTYPF_MASK: u32 = 0x000F;

const HDATA_APPOWNED: u32 = 0x0001;
pub(super) const DDE_FACK: u32 = 0x8000;
const CP_WINUNICODE: u32 = 1200;
const CP_WINANSI: u32 = 1004;
/// The size of Win32's CONVCONTEXT.
const CONVCONTEXT_SIZE: u32 = 36;
pub(super) const CF_TEXT: u32 = 1;

pub(super) const WM_DDE_ACK: u32 = 0x3E4;
pub(super) const WM_DDE_ADVISE: u32 = 0x3E2;
pub(super) const WM_DDE_DATA: u32 = 0x3E5;
pub(super) const WM_DDE_POKE: u32 = 0x3E7;
pub(super) const WM_DDE_EXECUTE: u32 = 0x3E8;

/// The first string handle; they are numbers as atoms are.
const FIRST_STRING: u32 = 0xC000;
/// Data handles and conversations are numbers past these, four apart.
const FIRST_DATA: u32 = 0x00D0_0000;
const FIRST_CONV: u32 = 0x00C0_0000;

/// A DDEML instance.
#[derive(Clone, Debug, Default)]
pub struct Instance {
    pub pid: u32,
    pub tid: u32,
    pub callback: u32,
    pub flags: u32,
    pub error: u32,
    /// The services DdeNameService registered.
    pub services: Vec<u32>,
}

/// A string handle: its string and how many keep it.
#[derive(Clone, Debug, Default)]
pub struct Str {
    pub text: String,
    pub refs: u32,
}

/// A data handle.
#[derive(Clone, Debug, Default)]
pub struct Data {
    pub instance: u32,
    pub bytes: Vec<u8>,
    pub format: u32,
    pub item: u32,
    pub app_owned: bool,
    /// The copy DdeAccessData gave: its process, block and address.
    pub access: Option<(u32, u32, u32)>,
    /// The 16-bit global block the 16-bit DdeAccessData gave, or 0.
    pub access16: Since<DDE_SAVED, u32>,
}

/// A conversation, with the shell's Program Manager.
#[derive(Clone, Debug, Default)]
pub struct Conv {
    pub instance: u32,
    pub service: u32,
    pub topic: u32,
    /// DdeSetUserHandle's value for QID_SYNC.
    pub user: Since<DDE_SAVED, u32>,
}

/// An asynchronous transaction whose XTYP_XACT_COMPLETE hasn't been
/// given yet.
#[derive(Clone, Debug, Default)]
pub struct Xact {
    pub instance: u32,
    pub conv: u32,
    pub id: u32,
    pub kind: u32,
    pub format: u32,
    pub item: u32,
    /// What the callback gets: the data, TRUE for a transaction that
    /// gives none, 0 for one that failed.
    pub data: u32,
    /// The DDE_ status flags.
    pub status: u32,
    /// DdeSetUserHandle's value for the transaction.
    pub user: u32,
}

/// The W32 section's version that first keeps the 16-bit DdeAccessData
/// copies, the user handles and the asynchronous transactions.
const DDE_SAVED: u16 = 75;

#[derive(Clone, Debug, Default)]
pub struct State {
    instances: Vec<Option<Instance>>,
    strings: Vec<Option<Str>>,
    data: Vec<Option<Data>>,
    convs: Vec<Option<Conv>>,
    xacts: Since<DDE_SAVED, Vec<Option<Xact>>>,
    /// The id the last asynchronous transaction got.
    last_xact: Since<DDE_SAVED, u32>,
}

crate::state_fields!(Instance {
    pid,
    tid,
    callback,
    flags,
    error,
    services
});
crate::state_fields!(Str { text, refs });
crate::state_fields!(Data {
    instance,
    bytes,
    format,
    item,
    app_owned,
    access,
    access16
});
crate::state_fields!(Conv {
    instance,
    service,
    topic,
    user
});
crate::state_fields!(Xact {
    instance,
    conv,
    id,
    kind,
    format,
    item,
    data,
    status,
    user
});
crate::state_fields!(State {
    instances,
    strings,
    data,
    convs,
    xacts,
    last_xact
});

fn state(cpu: &mut Cpu) -> &mut State {
    &mut cpu.bus.win32.dde
}

/// Put `value` in the first free slot of `list`: its place.
fn insert<T>(list: &mut Vec<Option<T>>, value: T) -> usize {
    match list.iter().position(Option::is_none) {
        Some(i) => {
            list[i] = Some(value);
            i
        }
        None => {
            list.push(Some(value));
            list.len() - 1
        }
    }
}

fn instance(cpu: &mut Cpu, id: u32) -> Option<&mut Instance> {
    let i = (id as usize).checked_sub(1)?;
    state(cpu).instances.get_mut(i)?.as_mut()
}

fn set_dde_error(cpu: &mut Cpu, id: u32, error: u32) {
    if let Some(inst) = instance(cpu, id) {
        inst.error = error;
    }
}

/// The process `pid` ended: its instances, and their handles, go.
pub fn process_ended(cpu: &mut Cpu, pid: u32) {
    let ids: Vec<u32> = state(cpu)
        .instances
        .iter()
        .enumerate()
        .filter(|(_, i)| i.as_ref().is_some_and(|i| i.pid == pid))
        .map(|(n, _)| n as u32 + 1)
        .collect();
    for id in ids {
        uninitialize(cpu, id);
    }
}

/// The thread `tid` ended, a 16-bit task's, whose instances are its own
/// as a process's are on Windows 3.1: they go.
pub fn thread_ended(cpu: &mut Cpu, tid: u32) {
    let ids: Vec<u32> = state(cpu)
        .instances
        .iter()
        .enumerate()
        .filter(|(_, i)| i.as_ref().is_some_and(|i| i.tid == tid))
        .map(|(n, _)| n as u32 + 1)
        .collect();
    for id in ids {
        uninitialize(cpu, id);
    }
}

fn uninitialize(cpu: &mut Cpu, id: u32) {
    let s = state(cpu);
    if let Some(slot) = (id as usize)
        .checked_sub(1)
        .and_then(|i| s.instances.get_mut(i))
    {
        *slot = None;
    }
    for c in s.convs.iter_mut() {
        if c.as_ref().is_some_and(|c| c.instance == id) {
            *c = None;
        }
    }
    for d in s.data.iter_mut() {
        if d.as_ref().is_some_and(|d| d.instance == id) {
            *d = None;
        }
    }
    for x in s.xacts.iter_mut() {
        if x.as_ref().is_some_and(|x| x.instance == id) {
            *x = None;
        }
    }
}

/// DdeInitializeA(id, callback, flags, reserved): a new instance, or
/// the instance `*id` names given new flags.
pub fn dde_initialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [out, callback, flags, _] = args[..] else {
        return Some(DMLERR_INVALIDPARAMETER);
    };
    if out == 0 || !cpu.bus.guest_probe(out, 4, true) || callback == 0 {
        return Some(DMLERR_INVALIDPARAMETER);
    }
    let id = cpu.bus.guest_read_32(out);
    if id != 0 {
        return Some(match instance(cpu, id) {
            Some(inst) => {
                inst.flags = flags;
                DMLERR_NO_ERROR
            }
            None => DMLERR_INVALIDPARAMETER,
        });
    }
    let inst = Instance {
        pid: super::current_pid(cpu),
        tid: current_tid(cpu),
        callback,
        flags,
        ..Instance::default()
    };
    let i = insert(&mut state(cpu).instances, inst);
    cpu.bus.guest_write_32(out, i as u32 + 1);
    Some(DMLERR_NO_ERROR)
}

/// DdeUninitialize(id).
pub fn dde_uninitialize(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if instance(cpu, args[0]).is_none() {
        return Some(0);
    }
    uninitialize(cpu, args[0]);
    Some(1)
}

/// DdeGetLastError(id): the instance's last error, which is cleared.
pub fn dde_get_last_error(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    match instance(cpu, args[0]) {
        Some(inst) => Some(std::mem::take(&mut inst.error)),
        None => Some(DMLERR_DLL_NOT_INITIALIZED),
    }
}

/// The string of the handle `hsz`.
pub fn string_of(cpu: &mut Cpu, hsz: u32) -> Option<String> {
    let i = hsz.checked_sub(FIRST_STRING)? as usize;
    Some(state(cpu).strings.get(i)?.as_ref()?.text.clone())
}

/// The handle of `text`, a reference more to it.
fn string_handle(cpu: &mut Cpu, text: &str) -> u32 {
    let s = state(cpu);
    let found = s.strings.iter().position(|x| {
        x.as_ref()
            .is_some_and(|x| x.text.eq_ignore_ascii_case(text))
    });
    let i = match found {
        Some(i) => {
            s.strings[i].as_mut().expect("found").refs += 1;
            i
        }
        None => insert(
            &mut s.strings,
            Str {
                text: text.to_string(),
                refs: 1,
            },
        ),
    };
    FIRST_STRING + i as u32
}

/// The Unicode string at `at` as ANSI.
fn wide_at(cpu: &mut Cpu, at: u32) -> Option<String> {
    let mut bytes = Vec::new();
    loop {
        let p = at + 2 * bytes.len() as u32;
        if bytes.len() > 0x8000 || !cpu.bus.guest_probe(p, 2, false) {
            return None;
        }
        match cpu.bus.guest_read_16(p) {
            0 => break,
            c => bytes.push(if c < 0x100 { c as u8 } else { b'?' }),
        }
    }
    Some(crate::dosstr::from_bytes(&bytes))
}

/// DdeCreateStringHandleA(id, string, code page) and the W function,
/// which takes the string as Unicode for CP_WINUNICODE.
pub fn dde_create_string_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create_string(cpu, args, false)
}

pub fn dde_create_string_handle_w(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    create_string(cpu, args, true)
}

fn create_string(cpu: &mut Cpu, args: &[u32], wide: bool) -> Option<u32> {
    let [id, at, code_page] = args[..] else {
        return Some(0);
    };
    if instance(cpu, id).is_none() {
        return Some(0);
    }
    if at == 0 {
        set_dde_error(cpu, id, DMLERR_INVALIDPARAMETER);
        return Some(0);
    }
    let text = if wide || code_page == CP_WINUNICODE {
        wide_at(cpu, at)
    } else {
        string_at(cpu, at)
    };
    let Some(text) = text.filter(|t| !t.is_empty()) else {
        return Some(0);
    };
    Some(string_handle(cpu, &text))
}

/// DdeFreeStringHandle(id, hsz).
pub fn dde_free_string_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = args[1].checked_sub(FIRST_STRING) else {
        return Some(0);
    };
    let s = state(cpu);
    let Some(slot) = s.strings.get_mut(i as usize) else {
        return Some(0);
    };
    let Some(str) = slot.as_mut() else {
        return Some(0);
    };
    str.refs -= 1;
    if str.refs == 0 {
        *slot = None;
    }
    Some(1)
}

/// DdeKeepStringHandle(id, hsz).
pub fn dde_keep_string_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(i) = args[1].checked_sub(FIRST_STRING) else {
        return Some(0);
    };
    match state(cpu)
        .strings
        .get_mut(i as usize)
        .and_then(|s| s.as_mut())
    {
        Some(s) => {
            s.refs += 1;
            Some(1)
        }
        None => Some(0),
    }
}

/// DdeQueryStringA(id, hsz, buffer, size, code page): the string's
/// length, copied to the buffer if there is one.
pub fn dde_query_string(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [_, hsz, buffer, size, code_page] = args[..] else {
        return Some(0);
    };
    let Some(text) = string_of(cpu, hsz) else {
        return Some(0);
    };
    let bytes = crate::dosstr::to_bytes(&text);
    if buffer == 0 {
        return Some(bytes.len() as u32);
    }
    if size == 0 {
        return Some(0);
    }
    let n = bytes.len().min(size as usize - 1);
    if code_page == CP_WINUNICODE {
        if !cpu.bus.guest_probe(buffer, 2 * (n + 1), true) {
            return Some(0);
        }
        for (i, &b) in bytes[..n].iter().chain([&0]).enumerate() {
            cpu.bus.guest_write_16(buffer + 2 * i as u32, b as u16);
        }
    } else {
        if !cpu.bus.guest_probe(buffer, n + 1, true) {
            return Some(0);
        }
        cpu.bus.guest_write_bytes(buffer, &bytes[..n]);
        cpu.bus.guest_write_8(buffer + n as u32, 0);
    }
    Some(n as u32)
}

/// DdeCmpStringHandles(a, b): -1, 0 or 1, as the handles compare.
pub fn dde_cmp_string_handles(_: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(match args[0].cmp(&args[1]) {
        std::cmp::Ordering::Less => -1i32 as u32,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    })
}

/// The data handle `h`.
pub fn data_mut(cpu: &mut Cpu, h: u32) -> Option<&mut Data> {
    data(cpu, h)
}

fn data(cpu: &mut Cpu, h: u32) -> Option<&mut Data> {
    let off = h.checked_sub(FIRST_DATA)?;
    if !off.is_multiple_of(4) {
        return None;
    }
    state(cpu).data.get_mut(off as usize / 4)?.as_mut()
}

/// A new data handle of `bytes`.
fn new_data(cpu: &mut Cpu, id: u32, bytes: Vec<u8>, item: u32, format: u32, owned: bool) -> u32 {
    let d = Data {
        instance: id,
        bytes,
        format,
        item,
        app_owned: owned,
        access: None,
        access16: Since(0),
    };
    let i = insert(&mut state(cpu).data, d);
    FIRST_DATA + 4 * i as u32
}

fn read_bytes(cpu: &mut Cpu, at: u32, n: u32) -> Option<Vec<u8>> {
    let mut bytes = vec![0u8; n as usize];
    if n > 0 {
        if !cpu.bus.guest_probe(at, n as usize, false) {
            return None;
        }
        cpu.bus.guest_read_bytes(at, &mut bytes);
    }
    Some(bytes)
}

/// DdeCreateDataHandle(id, source, size, offset, item, format, flags).
pub fn dde_create_data_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [id, source, size, offset, item, format, flags] = args[..] else {
        return Some(0);
    };
    if instance(cpu, id).is_none() {
        return Some(0);
    }
    let mut bytes = vec![0u8; offset.min(0x100_0000) as usize];
    if source != 0 {
        let Some(read) = read_bytes(cpu, source, size.min(0x100_0000)) else {
            set_dde_error(cpu, id, DMLERR_INVALIDPARAMETER);
            return Some(0);
        };
        bytes.extend(read);
    } else {
        bytes.resize(offset.saturating_add(size).min(0x100_0000) as usize, 0);
    }
    Some(new_data(
        cpu,
        id,
        bytes,
        item,
        format,
        flags & HDATA_APPOWNED != 0,
    ))
}

/// DdeAddData(handle, source, size, offset): the bytes put at the
/// offset, the data made larger if they don't fit.
pub fn dde_add_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [h, source, size, offset] = args[..] else {
        return Some(0);
    };
    let Some(read) = read_bytes(cpu, source, size.min(0x100_0000)) else {
        return Some(0);
    };
    let Some(d) = data(cpu, h) else {
        return Some(0);
    };
    let end = offset as usize + read.len();
    // No larger than DdeCreateDataHandle makes data.
    if end > 0x100_0000 {
        return Some(0);
    }
    if d.bytes.len() < end {
        d.bytes.resize(end, 0);
    }
    d.bytes[offset as usize..end].copy_from_slice(&read);
    Some(h)
}

/// DdeGetData(handle, buffer, size, offset): the bytes copied, or the
/// data's size for no buffer.
pub fn dde_get_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [h, buffer, size, offset] = args[..] else {
        return Some(0);
    };
    let Some(d) = data(cpu, h) else {
        return Some(0);
    };
    let bytes = d.bytes.get(offset as usize..).unwrap_or(&[]).to_vec();
    if buffer == 0 {
        return Some(bytes.len() as u32);
    }
    let n = bytes.len().min(size as usize);
    if !cpu.bus.guest_probe(buffer, n, true) {
        return Some(0);
    }
    cpu.bus.guest_write_bytes(buffer, &bytes[..n]);
    Some(n as u32)
}

/// DdeAccessData(handle, size): a copy of the data in the process's
/// memory, and its size.
pub fn dde_access_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [h, size_out] = args[..] else {
        return Some(0);
    };
    let pid = super::current_pid(cpu);
    let Some(d) = data(cpu, h) else {
        return Some(0);
    };
    if let Some((p, _, at)) = d.access
        && p == pid
    {
        let n = d.bytes.len() as u32;
        if size_out != 0 && cpu.bus.guest_probe(size_out, 4, true) {
            cpu.bus.guest_write_32(size_out, n);
        }
        return Some(at);
    }
    let bytes = d.bytes.clone();
    let block = crate::win32::kernel32::global_alloc(cpu, 0, (bytes.len() as u32).max(1));
    let at = crate::win32::kernel32::global_address(cpu, block);
    if at == 0 {
        return Some(0);
    }
    cpu.bus.guest_write_bytes(at, &bytes);
    if size_out != 0 && cpu.bus.guest_probe(size_out, 4, true) {
        cpu.bus.guest_write_32(size_out, bytes.len() as u32);
    }
    data(cpu, h)?.access = Some((pid, block, at));
    Some(at)
}

/// DdeUnaccessData(handle): the copy's bytes taken back.
pub fn dde_unaccess_data(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(d) = data(cpu, args[0]) else {
        return Some(0);
    };
    let Some((_, block, at)) = d.access.take() else {
        return Some(1);
    };
    let n = d.bytes.len();
    let mut bytes = vec![0u8; n];
    if cpu.bus.guest_probe(at, n, false) {
        cpu.bus.guest_read_bytes(at, &mut bytes);
        data(cpu, args[0])?.bytes = bytes;
    }
    crate::win32::kernel32::global_free(cpu, block);
    Some(1)
}

/// DdeFreeDataHandle(handle).
pub fn dde_free_data_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(d) = data(cpu, args[0]) else {
        return Some(0);
    };
    let block16 = std::mem::take(&mut *d.access16);
    if let Some((_, block, _)) = d.access.take() {
        crate::win32::kernel32::global_free(cpu, block);
    }
    // The copy a 16-bit program's DdeAccessData made goes with the data,
    // whoever frees it: the program, or the system after a transaction.
    if block16 != 0 {
        crate::win16::free_dde_copy(cpu, block16 as u16);
    }
    let i = (args[0] - FIRST_DATA) as usize / 4;
    state(cpu).data[i] = None;
    Some(1)
}

/// DdeNameService(id, service, reserved, flags): the services the
/// instance offers, which conversations aren't made with (see the top).
pub fn dde_name_service(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    const DNS_REGISTER: u32 = 1;
    const DNS_UNREGISTER: u32 = 2;
    let [id, hsz, _, flags] = args[..] else {
        return Some(0);
    };
    let Some(inst) = instance(cpu, id) else {
        return Some(0);
    };
    if flags & DNS_REGISTER != 0 {
        inst.services.push(hsz);
    } else if flags & DNS_UNREGISTER != 0 {
        if hsz == 0 {
            inst.services.clear();
        } else {
            inst.services.retain(|&s| s != hsz);
        }
    }
    Some(1)
}

/// Whether a service or topic of a connection is Program Manager's:
/// "PROGMAN", or any for a handle of 0.
fn is_progman(cpu: &mut Cpu, hsz: u32) -> bool {
    hsz == 0 || string_of(cpu, hsz).is_some_and(|s| s.eq_ignore_ascii_case("PROGMAN"))
}

/// DdeConnect(id, service, topic, context): a conversation with the
/// shell's Program Manager.
pub fn dde_connect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [id, service, topic, _] = args[..] else {
        return Some(0);
    };
    if instance(cpu, id).is_none() {
        return Some(0);
    }
    if !is_progman(cpu, service) || !is_progman(cpu, topic) {
        set_dde_error(cpu, id, DMLERR_NO_CONV_ESTABLISHED);
        return Some(0);
    }
    let conv = Conv {
        instance: id,
        service,
        topic,
        user: Since(0),
    };
    let i = insert(&mut state(cpu).convs, conv);
    Some(FIRST_CONV + 4 * i as u32)
}

fn conv(cpu: &mut Cpu, h: u32) -> Option<&mut Conv> {
    let off = h.checked_sub(FIRST_CONV)?;
    if !off.is_multiple_of(4) {
        return None;
    }
    state(cpu).convs.get_mut(off as usize / 4)?.as_mut()
}

/// DdeDisconnect(conversation).
pub fn dde_disconnect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if conv(cpu, args[0]).is_none() {
        return Some(0);
    }
    let i = (args[0] - FIRST_CONV) as usize / 4;
    state(cpu).convs[i] = None;
    drop_xacts(cpu, |x| x.conv == args[0]);
    Some(1)
}

/// Forget the asynchronous transactions `goes` picks, whose
/// XTYP_XACT_COMPLETE then isn't given, and free the data of the
/// requests among them, which the system would have freed after the
/// callback: whether there was one.
fn drop_xacts(cpu: &mut Cpu, goes: impl Fn(&Xact) -> bool) -> bool {
    let mut found = false;
    let mut freed = Vec::new();
    for x in state(cpu).xacts.iter_mut() {
        if !x.as_ref().is_some_and(&goes) {
            continue;
        }
        if let Some(t) = x.take()
            && t.kind == XTYP_REQUEST
            && t.data != 0
        {
            freed.push(t.data);
        }
        found = true;
    }
    for h in freed {
        if data(cpu, h).is_some_and(|d| !d.app_owned) {
            dde_free_data_handle(cpu, &[h]);
        }
    }
    found
}

/// DdeClientTransaction(data, size, conversation, item, format, type,
/// timeout, result): an execute or a request to Program Manager. Group
/// windows a command opens are made in the calling thread, as the
/// client waits for Explorer's reply on Windows. A data handle given
/// for the data is the system's afterwards, freed unless the program
/// made it HDATA_APPOWNED. With TIMEOUT_ASYNC the result is the
/// transaction's id and the function gives TRUE: the outcome goes to
/// the callback (see the top).
pub fn dde_client_transaction(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [source, size, hconv, item, format, kind, timeout, result] = args[..] else {
        return Some(0);
    };
    let Some(id) = conv(cpu, hconv).map(|c| c.instance) else {
        return Some(0);
    };
    if result != 0 && cpu.bus.guest_probe(result, 4, true) {
        cpu.bus.guest_write_32(result, 0);
    }
    let asynchronous = timeout == TIMEOUT_ASYNC;
    let (reply, windows) = match kind & !XTYPF_MASK {
        XTYP_EXECUTE => {
            let bytes = if size == u32::MAX {
                let Some(d) = data(cpu, source) else {
                    set_dde_error(cpu, id, DMLERR_INVALIDPARAMETER);
                    return Some(0);
                };
                let (bytes, owned) = (d.bytes.clone(), d.app_owned);
                if !owned {
                    dde_free_data_handle(cpu, &[source]);
                }
                bytes
            } else {
                match read_bytes(cpu, source, size.min(0x10_0000)) {
                    Some(b) => b,
                    None => {
                        set_dde_error(cpu, id, DMLERR_INVALIDPARAMETER);
                        return Some(0);
                    }
                }
            };
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            let command = crate::dosstr::from_bytes(&bytes[..end]);
            match crate::win32::shell32::progman_execute(cpu, &command) {
                Some(windows) => (1, windows),
                None => (0, Vec::new()),
            }
        }
        XTYP_REQUEST => {
            let name = string_of(cpu, item).unwrap_or_default();
            match crate::win32::shell32::progman_request(cpu, &name) {
                Some(text) if format == CF_TEXT => {
                    let mut bytes = crate::dosstr::to_bytes(&text);
                    bytes.push(0);
                    (new_data(cpu, id, bytes, item, format, false), Vec::new())
                }
                _ => (0, Vec::new()),
            }
        }
        _ => {
            set_dde_error(cpu, id, DMLERR_NOTPROCESSED);
            return Some(0);
        }
    };
    let status = if reply != 0 { DDE_FACK } else { 0 };
    if asynchronous {
        let xact = begin_async(cpu, hconv, kind, format, item, reply, status);
        if result != 0 && cpu.bus.guest_probe(result, 4, true) {
            cpu.bus.guest_write_32(result, xact);
        }
        if xact == 0 {
            // No window to give the outcome by: the transaction fails,
            // and a request's data, which no one gets, goes.
            if kind & !XTYPF_MASK == XTYP_REQUEST && reply != 0 {
                dde_free_data_handle(cpu, &[reply]);
            }
            set_dde_error(cpu, id, DMLERR_MEMORY_ERROR);
            return Some(0);
        }
        return open_windows(cpu, windows);
    }
    if reply == 0 {
        set_dde_error(cpu, id, DMLERR_NOTPROCESSED);
        return Some(0);
    }
    if result != 0 && cpu.bus.guest_probe(result, 4, true) {
        cpu.bus.guest_write_32(result, status);
    }
    match kind & !XTYPF_MASK {
        XTYP_EXECUTE => open_windows(cpu, windows),
        _ => Some(reply),
    }
}

/// The message the hidden window of a client's thread gets for a
/// finished asynchronous transaction: wParam its place in the list,
/// lParam its id.
const WM_XACT_DONE: u32 = 0x0400;

/// The class of the hidden windows that tell clients of their
/// transactions, as Windows 98's DDEML has a hidden window of its own.
pub const EVENT_CLASS: &str = "DDEMLEvent";

/// Keep the asynchronous transaction on the conversation `hconv` that
/// had the outcome `data` and `status` for XTYP_XACT_COMPLETE, and post
/// the message that gives it to the callback: its id, or 0 when there
/// is no window to post it to.
fn begin_async(
    cpu: &mut Cpu,
    hconv: u32,
    kind: u32,
    format: u32,
    item: u32,
    data: u32,
    status: u32,
) -> u32 {
    let Some(c) = conv(cpu, hconv).cloned() else {
        return 0;
    };
    let Some(inst) = instance(cpu, c.instance).cloned() else {
        return 0;
    };
    let hwnd = event_window(cpu, inst.pid, inst.tid);
    if hwnd == 0 {
        return 0;
    }
    let s = state(cpu);
    *s.last_xact = s.last_xact.wrapping_add(1).max(1);
    let id = *s.last_xact;
    let xact = Xact {
        instance: c.instance,
        conv: hconv,
        id,
        kind: kind & !XTYPF_MASK,
        format,
        item,
        data,
        status,
        user: 0,
    };
    let slot = insert(&mut s.xacts, xact) as u32;
    let msg = super::message::Msg {
        hwnd,
        message: WM_XACT_DONE,
        wparam: slot,
        lparam: id,
        ..Default::default()
    };
    let msg = super::message::stamp(cpu, msg);
    super::message::post(cpu, inst.tid, msg);
    id
}

/// The hidden window of the thread `tid` of the process `pid`, made if
/// it isn't there yet: its handle, or 0 when there is no room for it.
fn event_window(cpu: &mut Cpu, pid: u32, tid: u32) -> u32 {
    super::init(cpu);
    let proc = super::address_of(cpu, "DdemlWndProc");
    let state = super::user(cpu);
    let found = state.top_levels().into_iter().find(|&h| {
        state
            .window(h)
            .is_some_and(|w| w.tid == tid && w.proc == proc)
    });
    if let Some(hwnd) = found {
        return hwnd;
    }
    let window = super::window::Window {
        tid,
        pid,
        ..Default::default()
    };
    super::window::new_hidden(state, EVENT_CLASS, proc, window)
}

/// DdemlWndProc(hwnd, message, wParam, lParam): the procedure of the
/// hidden windows, which calls a client's callback with
/// XTYP_XACT_COMPLETE for the transaction a message names.
pub fn wnd_proc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[_, msg, wp, lp] = args else {
        return Some(0);
    };
    if msg != WM_XACT_DONE {
        return super::defwnd::def_window_proc(cpu, args);
    }
    // A transaction abandoned, or of a conversation ended, is gone. One
    // that isn't stays while the callback runs, which may ask
    // DdeQueryConvInfo of it.
    let Some(x) = state(cpu)
        .xacts
        .get(wp as usize)
        .and_then(Option::as_ref)
        .filter(|x| x.id == lp)
        .cloned()
    else {
        return Some(0);
    };
    let topic = conv(cpu, x.conv).map_or(0, |c| c.topic);
    let Some(callback) = instance(cpu, x.instance).map(|i| i.callback) else {
        return Some(0);
    };
    // The data is the client's while the callback runs, and goes after
    // it.
    let free = if x.kind == XTYP_REQUEST { x.data } else { 0 };
    let vars = vec![
        callback,
        XTYP_XACT_COMPLETE,
        x.format,
        x.conv,
        topic,
        x.item,
        x.data,
        x.id,
        x.status,
        free,
        wp,
    ];
    task::start(cpu, task::DDEML_XACT, vars, 0)
}

/// The task of `wnd_proc`: variable 0 the callback, 1 to 8 its
/// arguments (7 the transaction's id), 9 a data handle to free when it
/// returns and 10 the transaction's place.
pub fn xact_step(cpu: &mut Cpu, t: &mut Task, _value: u32) -> Next {
    if t.step == 0 {
        return Next::call(t.var(0), &t.vars[1..9], 1);
    }
    let (id, slot) = (t.var(7), t.var(10) as usize);
    if let Some(x) = state(cpu).xacts.get_mut(slot)
        && x.as_ref().is_some_and(|x| x.id == id)
    {
        *x = None;
    }
    let free = t.var(9);
    if free != 0 && data(cpu, free).is_some_and(|d| !d.app_owned) {
        dde_free_data_handle(cpu, &[free]);
    }
    Next::Done(0)
}

/// DdeAbandonTransaction(id, conversation, transaction): the
/// transaction's XTYP_XACT_COMPLETE isn't given, nor any of the
/// conversation's for a transaction of 0, nor any of the instance's for
/// a conversation of 0.
pub fn dde_abandon_transaction(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [id, hconv, xact] = args[..] else {
        return Some(0);
    };
    if instance(cpu, id).is_none() {
        return Some(0);
    }
    if hconv != 0 && conv(cpu, hconv).is_none() {
        set_dde_error(cpu, id, DMLERR_INVALIDPARAMETER);
        return Some(0);
    }
    let found = drop_xacts(cpu, |x| {
        x.instance == id && (hconv == 0 || x.conv == hconv) && (xact == 0 || x.id == xact)
    });
    if xact != 0 && !found {
        set_dde_error(cpu, id, DMLERR_UNFOUND_QUEUE_ID);
        return Some(0);
    }
    Some(1)
}

/// The asynchronous transaction `xact` of the conversation `hconv`.
fn xact(cpu: &mut Cpu, hconv: u32, xact: u32) -> Option<&mut Xact> {
    state(cpu)
        .xacts
        .iter_mut()
        .flatten()
        .find(|x| x.conv == hconv && x.id == xact)
}

/// DdeSetUserHandle(conversation, id, value): the value DdeQueryConvInfo
/// gives for the conversation (id QID_SYNC) or one of its asynchronous
/// transactions.
pub fn dde_set_user_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hconv, id, user] = args[..] else {
        return Some(0);
    };
    let Some(c) = conv(cpu, hconv) else {
        return Some(0);
    };
    if id == QID_SYNC {
        *c.user = user;
        return Some(1);
    }
    let instance = c.instance;
    match xact(cpu, hconv, id) {
        Some(x) => {
            x.user = user;
            Some(1)
        }
        None => {
            set_dde_error(cpu, instance, DMLERR_UNFOUND_QUEUE_ID);
            Some(0)
        }
    }
}

/// What DdeQueryConvInfo tells of a conversation, the 32-bit and the
/// 16-bit CONVINFO alike.
#[derive(Clone, Debug, Default)]
pub struct ConvInfo {
    pub user: u32,
    pub service: u32,
    pub topic: u32,
    pub item: u32,
    pub format: u32,
    pub kind: u32,
    pub status: u16,
    pub state: u16,
}

/// The CONVINFO of the conversation `hconv`, for the transaction `id`
/// or QID_SYNC. A transaction that isn't there sets the instance's
/// DMLERR_UNFOUND_QUEUE_ID, for the 32-bit and the 16-bit DDEML alike.
pub fn conv_info(cpu: &mut Cpu, hconv: u32, id: u32) -> Option<ConvInfo> {
    /// ST_CONNECTED and ST_CLIENT.
    const ST_CLIENT_CONNECTED: u16 = 0x0011;
    /// The conversation's states: XST_CONNECTED, XST_REQSENT and
    /// XST_EXECSENT.
    const XST_CONNECTED: u16 = 2;
    const XST_REQSENT: u16 = 5;
    const XST_EXECSENT: u16 = 9;
    let c = conv(cpu, hconv)?.clone();
    let mut info = ConvInfo {
        user: *c.user,
        service: c.service,
        topic: c.topic,
        status: ST_CLIENT_CONNECTED,
        state: XST_CONNECTED,
        ..ConvInfo::default()
    };
    if id != QID_SYNC {
        let Some(x) = xact(cpu, hconv, id).cloned() else {
            set_dde_error(cpu, c.instance, DMLERR_UNFOUND_QUEUE_ID);
            return None;
        };
        info.user = x.user;
        info.item = x.item;
        info.format = x.format;
        info.kind = x.kind;
        info.state = if x.kind == XTYP_EXECUTE {
            XST_EXECSENT
        } else {
            XST_REQSENT
        };
    }
    Some(info)
}

/// DdeQueryConvInfo(conversation, id, info): the CONVINFO, as much of
/// it as its cb asks for; the bytes written.
pub fn dde_query_conv_info(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [hconv, id, out] = args[..] else {
        return Some(0);
    };
    if out == 0 || !cpu.bus.guest_probe(out, 4, false) {
        return Some(0);
    }
    let Some(info) = conv_info(cpu, hconv, id) else {
        return Some(0);
    };
    // cb, hUser, hConvPartner, hszSvcPartner, hszServiceReq, hszTopic,
    // hszItem, wFmt, wType, wStatus, wConvst, wLastError, hConvList,
    // ConvCtxt (cb, wFlags, wCountryID, iCodePage, dwLangID,
    // dwSecurity, then SECURITY_QUALITY_OF_SERVICE's 12 bytes), hwnd and
    // hwndPartner.
    let fields = [
        0,
        info.user,
        0,
        info.service,
        info.service,
        info.topic,
        info.item,
        info.format,
        info.kind,
        info.status as u32,
        info.state as u32,
        0,
        0,
    ];
    let mut bytes: Vec<u8> = fields.iter().flat_map(|v| v.to_le_bytes()).collect();
    let context = [CONVCONTEXT_SIZE, 0, 0, CP_WINANSI, 0, 0, 0, 0, 0];
    bytes.extend(context.iter().flat_map(|v| v.to_le_bytes()));
    bytes.extend([0u8; 8]);
    let cb = cpu.bus.guest_read_32(out) as usize;
    let n = cb.min(bytes.len());
    if n < 4 || !cpu.bus.guest_probe(out, n, true) {
        return Some(0);
    }
    // cb stays as the program set it.
    cpu.bus.guest_write_bytes(out + 4, &bytes[4..n]);
    Some(n as u32)
}

/// The class of Explorer's folder windows, which group windows are.
pub const FOLDER_CLASS: &str = "CabinetWClass";

/// Open a window titled with each of `titles`, through CreateWindowExA
/// called as the program would call it, and return TRUE. A group whose
/// window is open already keeps that one, as Explorer's does.
pub fn open_windows(cpu: &mut Cpu, titles: Vec<String>) -> Option<u32> {
    let mut titles = titles;
    let mut seen: Vec<Vec<u8>> = Vec::new();
    let state = &cpu.bus.win32.user;
    for h in state.top_levels() {
        if super::class::is_class(state, h, FOLDER_CLASS)
            && let Some(w) = state.window(h)
        {
            seen.push(w.text.clone());
        }
    }
    titles.retain(|t| {
        let text = crate::dosstr::to_bytes(t);
        if seen.iter().any(|s| s.eq_ignore_ascii_case(&text)) {
            return false;
        }
        seen.push(text);
        true
    });
    if titles.is_empty() {
        return Some(1);
    }
    let mut text = Vec::new();
    for t in &titles {
        text.extend(crate::dosstr::to_bytes(t));
        text.push(0);
    }
    text.extend(crate::dosstr::to_bytes(FOLDER_CLASS));
    text.push(0);
    let scratch = text.len().div_ceil(4) as u32 * 4;
    // The strings ride in the variables until the task has its scratch
    // space.
    let mut vars = vec![titles.len() as u32, text.len() as u32];
    vars.extend(text.chunks(4).map(|c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        u32::from_le_bytes(w)
    }));
    task::start(cpu, task::DDEML, vars, scratch)
}

/// The task of `open_windows`: variable 0 the number of windows, 1 the
/// size of the strings (the titles, then the class), the strings after.
pub fn step(cpu: &mut Cpu, t: &mut Task, _value: u32) -> Next {
    let count = t.var(0);
    let n = t.step as u32;
    if n >= count {
        return Next::Done(1);
    }
    if n == 0 {
        let len = t.var(1) as usize;
        let bytes: Vec<u8> = t.vars[2..].iter().flat_map(|w| w.to_le_bytes()).collect();
        cpu.bus
            .guest_write_bytes(t.scratch, &bytes[..len.min(bytes.len())]);
    }
    // The title of window `n`, and the class after the titles.
    let mut at = t.scratch;
    for _ in 0..n {
        at += cpu.bus.guest_read_asciiz(at, 0x1000).len() as u32 + 1;
    }
    let mut class = t.scratch;
    for _ in 0..count {
        class += cpu.bus.guest_read_asciiz(class, 0x1000).len() as u32 + 1;
    }
    const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const CW_USEDEFAULT: u32 = 0x8000_0000;
    let create = super::address_of(cpu, "CreateWindowExA");
    let args = [
        0,
        class,
        at,
        WS_OVERLAPPEDWINDOW | WS_VISIBLE,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        0,
        0,
        0,
        0,
    ];
    Next::call(create, &args, t.step + 1)
}

fn is_global(msg: u32) -> bool {
    matches!(msg, WM_DDE_ACK | WM_DDE_ADVISE | WM_DDE_DATA | WM_DDE_POKE)
}

/// PackDDElParam(message, low, high): for the messages whose lParam is
/// two 32-bit values, a global block holding them, as Windows NT and
/// Windows 98 pack them.
pub fn pack_dde_lparam(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [msg, lo, hi] = args[..] else {
        return Some(0);
    };
    if is_global(msg) {
        const GMEM_DDESHARE: u32 = 0x2000;
        let block = crate::win32::kernel32::global_alloc(cpu, GMEM_DDESHARE, 8);
        let at = crate::win32::kernel32::global_address(cpu, block);
        if at == 0 {
            return Some(0);
        }
        cpu.bus.guest_write_32(at, lo);
        cpu.bus.guest_write_32(at + 4, hi);
        return Some(block);
    }
    if msg == WM_DDE_EXECUTE {
        return Some(hi);
    }
    Some((lo & 0xFFFF) | hi << 16)
}

/// The two values in the global block `lparam` that PackDDElParam made,
/// if it is one.
pub(super) fn unpacked(cpu: &mut Cpu, lparam: u32) -> Option<(u32, u32)> {
    let at = if lparam == 0 {
        0
    } else {
        crate::win32::kernel32::global_address(cpu, lparam)
    };
    if at == 0 || !cpu.bus.guest_probe(at, 8, false) {
        return None;
    }
    Some((cpu.bus.guest_read_32(at), cpu.bus.guest_read_32(at + 4)))
}

/// UnpackDDElParam(message, lParam, low, high).
pub fn unpack_dde_lparam(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [msg, lparam, lo_out, hi_out] = args[..] else {
        return Some(0);
    };
    let (lo, hi, ok) = if is_global(msg) {
        match unpacked(cpu, lparam) {
            Some((lo, hi)) => (lo, hi, true),
            None => (0, 0, false),
        }
    } else if msg == WM_DDE_EXECUTE {
        (0, lparam, true)
    } else {
        (lparam & 0xFFFF, lparam >> 16, true)
    };
    for (out, v) in [(lo_out, lo), (hi_out, hi)] {
        if out != 0 && cpu.bus.guest_probe(out, 4, true) {
            cpu.bus.guest_write_32(out, v);
        }
    }
    Some(ok as u32)
}

/// FreeDDElParam(message, lParam): a packed block freed.
pub fn free_dde_lparam(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if is_global(args[0]) && args[1] != 0 {
        crate::win32::kernel32::global_free(cpu, args[1]);
    }
    Some(1)
}

/// ReuseDDElParam(lParam, message in, message out, low, high).
pub fn reuse_dde_lparam(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [lparam, msg_in, msg_out, lo, hi] = args[..] else {
        return Some(0);
    };
    if !is_global(msg_in) {
        return pack_dde_lparam(cpu, &[msg_out, lo, hi]);
    }
    if is_global(msg_out) {
        if lparam == 0 {
            return Some(0);
        }
        let at = crate::win32::kernel32::global_address(cpu, lparam);
        if at == 0 || !cpu.bus.guest_probe(at, 8, true) {
            return Some(0);
        }
        cpu.bus.guest_write_32(at, lo);
        cpu.bus.guest_write_32(at + 4, hi);
        return Some(lparam);
    }
    free_dde_lparam(cpu, &[msg_in, lparam]);
    if msg_out == WM_DDE_EXECUTE {
        Some(hi)
    } else {
        Some((lo & 0xFFFF) | hi << 16)
    }
}
