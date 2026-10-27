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

use super::{current_tid, string_at};
use crate::cpu::Cpu;
use crate::win32::task::{self, Next, Task};

const DMLERR_NO_ERROR: u32 = 0;
const DMLERR_DLL_NOT_INITIALIZED: u32 = 0x4003;
const DMLERR_INVALIDPARAMETER: u32 = 0x4006;
const DMLERR_NOTPROCESSED: u32 = 0x4009;
const DMLERR_NO_CONV_ESTABLISHED: u32 = 0x400A;

const XTYP_EXECUTE: u32 = 0x4050;
const XTYP_REQUEST: u32 = 0x20B0;
/// The flags of a transaction type (XTYPF_NOBLOCK and the rest).
const XTYPF_MASK: u32 = 0x000F;

const HDATA_APPOWNED: u32 = 0x0001;
pub(super) const DDE_FACK: u32 = 0x8000;
const CP_WINUNICODE: u32 = 1200;
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
}

/// A conversation, with the shell's Program Manager.
#[derive(Clone, Debug, Default)]
pub struct Conv {
    pub instance: u32,
    pub service: u32,
    pub topic: u32,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    instances: Vec<Option<Instance>>,
    strings: Vec<Option<Str>>,
    data: Vec<Option<Data>>,
    convs: Vec<Option<Conv>>,
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
    access
});
crate::state_fields!(Conv {
    instance,
    service,
    topic
});
crate::state_fields!(State {
    instances,
    strings,
    data,
    convs
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
    if let Some((_, block, _)) = d.access.take() {
        crate::win32::kernel32::global_free(cpu, block);
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
    Some(1)
}

/// DdeClientTransaction(data, size, conversation, item, format, type,
/// timeout, result): an execute or a request to Program Manager. Group
/// windows a command opens are made in the calling thread, as the
/// client waits for Explorer's reply on Windows.
pub fn dde_client_transaction(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let [source, size, hconv, item, format, kind, _, result] = args[..] else {
        return Some(0);
    };
    let Some(id) = conv(cpu, hconv).map(|c| c.instance) else {
        return Some(0);
    };
    if result != 0 && cpu.bus.guest_probe(result, 4, true) {
        cpu.bus.guest_write_32(result, 0);
    }
    match kind & !XTYPF_MASK {
        XTYP_EXECUTE => {
            let bytes = if size == u32::MAX {
                let Some(d) = data(cpu, source) else {
                    set_dde_error(cpu, id, DMLERR_INVALIDPARAMETER);
                    return Some(0);
                };
                d.bytes.clone()
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
                Some(windows) => {
                    if result != 0 && cpu.bus.guest_probe(result, 4, true) {
                        cpu.bus.guest_write_32(result, DDE_FACK);
                    }
                    open_windows(cpu, windows)
                }
                None => {
                    set_dde_error(cpu, id, DMLERR_NOTPROCESSED);
                    Some(0)
                }
            }
        }
        XTYP_REQUEST => {
            let name = string_of(cpu, item).unwrap_or_default();
            match crate::win32::shell32::progman_request(cpu, &name) {
                Some(text) if format == CF_TEXT => {
                    let mut bytes = crate::dosstr::to_bytes(&text);
                    bytes.push(0);
                    if result != 0 && cpu.bus.guest_probe(result, 4, true) {
                        cpu.bus.guest_write_32(result, DDE_FACK);
                    }
                    Some(new_data(cpu, id, bytes, item, format, false))
                }
                _ => {
                    set_dde_error(cpu, id, DMLERR_NOTPROCESSED);
                    Some(0)
                }
            }
        }
        _ => {
            set_dde_error(cpu, id, DMLERR_NOTPROCESSED);
            Some(0)
        }
    }
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
    let create = super::address_of("CreateWindowExA");
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
