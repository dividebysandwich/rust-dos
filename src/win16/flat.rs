//! Flat thunks: how a Win32 DLL and a 16-bit DLL made with Windows 95's
//! thunk compiler call each other. Each side has a block of thunk data
//! with the same magic and checksum. The 32-bit DLL's entry point calls
//! KERNEL32's ThunkConnect32 with its block and the 16-bit DLL's name;
//! that loads the 16-bit DLL, whose DllEntryPoint calls KERNEL's
//! ThunkConnect16 with its own block, and joins the two. A 16-bit DLL
//! another process loaded already only has its DllEntryPoint run again,
//! as Windows 95 runs it for each process that attaches.
//!
//! A 32-bit to 16-bit thunk ("LS01") pushes the arguments as the 16-bit
//! function takes them, its pointers mapped to 16:16 ones (SMapLS and
//! the others), and calls the code ThunkConnect32 wrote into its block,
//! which takes the 16-bit function's address from the 16-bit DLL's
//! table of targets and goes on to QT_Thunk. That copies the arguments
//! to a 16-bit stack and calls the function.
//!
//! The 16-bit code runs in the Win16 process, where the 16-bit modules
//! and their selectors are, on a task of the Win32 process's own (its
//! client), as Windows 95 gives each Win32 process a task database.
//! The Win32 thread that called waits while the client runs the call,
//! and goes on with what the call returned in its registers. Pointers
//! go across as aliases: a 16:16 pointer MapLS makes for a Win32
//! process's memory has a selector whose base is a window of the Win16
//! process's address space that maps the same physical pages, and the
//! flat pointer MapSL makes for a 16-bit segment is a window of the
//! Win32 process's that maps the segment's pages. Both sides see each
//! other's writes, as in Windows 95, where the two are one address
//! space.
//!
//! The other direction ("SL01", 16-bit code calling 32-bit code) is
//! joined as Windows 95 joins it, but its calls fail as Windows 95's
//! fail when the Win32 side isn't there: the 16-bit function returns
//! the error value its thunk data has for it.

use super::thunk::{self, EAX, EBX, ECX, EDI, EDX, ESI, ESP};
use super::{ldt, module};
use crate::cpu::{Cpu, Seg};
use crate::dpmi::Context;
use crate::win32::memory::{PAGE, PRESENT, SHARED, SYSTEM, USER, WRITABLE};
use crate::win32::thread::Wait;

/// What a request of a Win32 thread to its client is.
pub const CALL: u8 = 0;
pub const CONNECT: u8 = 1;
/// LoadLibrary16's: load a 16-bit DLL for the process.
pub const LOAD: u8 = 2;

/// The bytes of a thunk block's header: magic and checksum.
const HEADER: usize = 8;
/// What ThunkConnect32 reads of the 32-bit thunk data.
const DATA32: usize = 0x24;
/// The bytes the thunk compiler's 32-bit thunks keep between their
/// frame pointer and the arguments they push for QT_Thunk.
const QT_FRAME: u32 = 0x40;

/// A Win32 process that calls 16-bit code, and its task in the Win16
/// process, which runs the calls.
#[derive(Clone, Debug, Default)]
pub struct Client {
    pub pid: u32,
    /// The task's thread.
    pub tid: u32,
    /// The 16-bit DLLs its ThunkConnect32s and LoadLibrary16s loaded, a
    /// reference to each, which go when it ends.
    pub modules: Vec<u16>,
    /// The requests waiting, and the one the task runs.
    pub queue: Vec<Request>,
    pub current: Option<Request>,
}

/// A Win32 thread's request: a call of a 16-bit function (`CALL`), a
/// ThunkConnect32 (`CONNECT`) or a LoadLibrary16 (`LOAD`). The thread
/// waits until it is done.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub kind: u8,
    pub tid: u32,
    /// CALL: the function, the registers it gets, the arguments' bytes,
    /// and where the Win32 thread goes on (`regs.eip` and its stack
    /// above the arguments); the 16-bit SP below the arguments, once the
    /// call is made.
    pub target: u32,
    pub regs: Context,
    pub args: Vec<u8>,
    pub sp: u32,
    /// CONNECT: the 32-bit thunk data, the names of the 16-bit DLL and
    /// its thunk data and the directory to look for it in, and the
    /// module loaded. LOAD: the DLL's name, an empty one and the
    /// directory, and the instance loaded.
    pub data32: u32,
    pub names: Vec<String>,
    pub module: u16,
}

/// A window of the process `owner`'s address space mapping the pages
/// at `address` of the process `source`: `pages` of them from `window`.
/// A MapLS alias has the Win16 selector `selector`.
#[derive(Clone, Debug, Default)]
pub struct Alias {
    pub owner: u32,
    pub source: u32,
    pub window: u32,
    pub pages: u32,
    pub address: u32,
    pub selector: u16,
}

/// A 16-bit to 32-bit thunk's data ThunkConnect16 joined: the 16-bit
/// block's address.
#[derive(Clone, Debug, Default)]
pub struct Link {
    pub data16: u32,
}

/// Run `f` with the process `pid`'s memory and the bus.
fn with_memory<T>(
    cpu: &mut Cpu,
    pid: u32,
    f: impl FnOnce(&mut crate::win32::memory::Memory, &mut crate::bus::Bus) -> T,
) -> Option<T> {
    super::with_win16(cpu, pid, |p, bus| f(&mut p.memory, bus))
}

/// `len` bytes at `at` of the process `pid`, if they are there.
fn read(cpu: &mut Cpu, pid: u32, at: u32, len: usize) -> Option<Vec<u8>> {
    with_memory(cpu, pid, |m, bus| {
        crate::win32::loader::peek(bus, m, at, len)
    })
    .flatten()
}

/// Write `data` at `at` of the process `pid`. False where a page isn't
/// there.
fn write(cpu: &mut Cpu, pid: u32, at: u32, data: &[u8]) -> bool {
    with_memory(cpu, pid, |m, bus| m.write(bus, at, data))
        .flatten()
        .is_some()
}

/// The running process's id.
fn running_pid(cpu: &Cpu) -> u32 {
    crate::win32::running(cpu).map_or(0, |p| p.id)
}

/// The Win16 process, made if there is none, for the running Win32
/// process's calls: its id, or None when it can't be made.
fn ensure_win16(cpu: &mut Cpu) -> Option<u32> {
    let environment = crate::win32::running(cpu)?.environment.clone();
    if let Err(message) = super::task::ensure_process(cpu, environment) {
        cpu.bus.log_string(&format!("[W16] {}", message));
        return None;
    }
    Some(cpu.bus.win32.win16.pid)
}

// Aliases.

/// Map the pages that hold `len` bytes at `address` of the process
/// `source` into a new window of the process `owner`: where `address`
/// is in it. Pages that aren't there stay out of the window.
fn alias(
    cpu: &mut Cpu,
    source: u32,
    address: u32,
    len: u32,
    owner: u32,
    selector: u16,
) -> Option<u32> {
    let first = address & !(PAGE - 1);
    let end = (address as u64 + len.max(1) as u64).min(1 << 32);
    let pages = (end - first as u64).div_ceil(PAGE as u64) as u32;
    let entries = with_memory(cpu, source, |m, bus| {
        (0..pages)
            .map(|i| {
                m.entry(bus, first.wrapping_add(i * PAGE))
                    .filter(|e| e & PRESENT != 0)
            })
            .collect::<Vec<_>>()
    })?;
    let window = with_memory(cpu, owner, |m, bus| {
        let window = m.reserve(None, pages * PAGE, SHARED, SYSTEM)?;
        for (i, entry) in entries.iter().enumerate() {
            if let Some(e) = entry {
                m.map(bus, window + i as u32 * PAGE, *e, e & WRITABLE | USER)?;
            }
        }
        Some(window)
    })
    .flatten()?;
    cpu.tlb.flush();
    cpu.bus.win32.win16.aliases.push(Alias {
        owner,
        source,
        window,
        pages,
        address,
        selector,
    });
    Some(window + (address & (PAGE - 1)))
}

/// Take the alias `i` away: its window's pages go out of its owner's page
/// tables, the pages staying their source's, and its selector, if it
/// has one, is free again.
fn unalias(cpu: &mut Cpu, i: usize) {
    let a = cpu.bus.win32.win16.aliases.remove(i);
    with_memory(cpu, a.owner, |m, bus| {
        for page in 0..a.pages {
            m.set_entry(bus, a.window + page * PAGE, 0);
        }
        m.unreserve(a.window);
    });
    cpu.tlb.flush();
    if a.selector != 0 {
        free_selector(cpu, a.selector);
    }
}

/// Free the Win16 selector `sel`, its descriptor no longer present.
fn free_selector(cpu: &mut Cpu, sel: u16) {
    if let Some(system) = super::win16_ref(cpu).map(|p| p.system) {
        ldt::write(
            cpu,
            system,
            sel,
            ldt::segment(0, 0, ldt::ACCESS_DATA & !0x80),
        );
    }
    cpu.bus.win32.win16.ldt.free(sel, 1);
}

/// The address in the process `owner` of `address` of the process
/// `source`, if an alias without a selector has the whole of `len`
/// bytes there.
fn aliased(cpu: &Cpu, owner: u32, source: u32, address: u32, len: u32) -> Option<u32> {
    cpu.bus.win32.win16.aliases.iter().find_map(|a| {
        let first = a.address & !(PAGE - 1);
        let end = first as u64 + (a.pages * PAGE) as u64;
        (a.owner == owner
            && a.source == source
            && a.selector == 0
            && address >= first
            && address as u64 + len as u64 <= end)
            .then(|| a.window + (address - first))
    })
}

/// MapLS: a 16:16 pointer to `ptr` of the running process, its selector
/// a new one of the Win16 process's LDT covering 64 KB from there. A
/// value below 64 KB is no pointer and comes back as it is.
pub fn map_ls(cpu: &mut Cpu, ptr: u32) -> u32 {
    if ptr >> 16 == 0 {
        return ptr;
    }
    let pid = running_pid(cpu);
    let Some(win16) = ensure_win16(cpu) else {
        return 0;
    };
    let Some(sel) = cpu.bus.win32.win16.ldt.alloc(1) else {
        return 0;
    };
    let base = if pid == win16 {
        Some(ptr)
    } else {
        alias(cpu, pid, ptr, 0x1_0000, win16, sel)
    };
    let Some(base) = base else {
        cpu.bus.win32.win16.ldt.free(sel, 1);
        return 0;
    };
    let system = super::win16(cpu).system;
    ldt::write(
        cpu,
        system,
        sel,
        ldt::segment(base, 0xFFFF, ldt::ACCESS_DATA),
    );
    (sel as u32) << 16
}

/// UnMapLS: the selector of a 16:16 pointer MapLS made is free again.
pub fn unmap_ls(cpu: &mut Cpu, segptr: u32) {
    let sel = (segptr >> 16) as u16;
    if sel == 0 {
        return;
    }
    let aliases = &cpu.bus.win32.win16.aliases;
    if let Some(i) = aliases
        .iter()
        .position(|a| a.selector != 0 && a.selector | 3 == sel | 3)
    {
        unalias(cpu, i);
    }
}

/// MapSL: the running process's flat address of the 16:16 pointer
/// `segptr`, 0 if its selector isn't there.
pub fn map_sl(cpu: &mut Cpu, segptr: u32) -> u32 {
    let sel = (segptr >> 16) as u16;
    let offset = segptr & 0xFFFF;
    if sel == 0 {
        return 0;
    }
    let pid = running_pid(cpu);
    // A pointer MapLS made for the process's own memory.
    if let Some(a) = cpu
        .bus
        .win32
        .win16
        .aliases
        .iter()
        .find(|a| a.selector != 0 && a.selector | 3 == sel | 3 && a.source == pid)
    {
        return a.address.wrapping_add(offset);
    }
    let win16 = cpu.bus.win32.win16.pid;
    let Some((base, limit, _)) = super::selector_base(cpu, sel) else {
        return 0;
    };
    if pid == win16 {
        return base.wrapping_add(offset);
    }
    let len = limit.saturating_add(1);
    let at = match aliased(cpu, pid, win16, base, len) {
        Some(at) => at,
        None => match alias(cpu, win16, base, len, pid, 0) {
            Some(at) => at,
            None => return 0,
        },
    };
    at.wrapping_add(offset)
}

// KERNEL32's functions.

/// ThunkConnect32(thunk data, 16-bit thunk data's name, 16-bit DLL,
/// 32-bit DLL, module, reason): at DLL_PROCESS_ATTACH, load the 16-bit
/// DLL and join the two thunk data blocks. The calling thread waits
/// while the Win16 side loads; it gets 1, or 0 when the two don't join.
pub fn thunk_connect32(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (data32, name_at, dll16_at, reason) = (args[0], args[1], args[2], args[5]);
    if reason != 1 {
        return Some(1);
    }
    let magic = guest_bytes(cpu, data32, 4);
    if magic != b"LS01" && magic != b"SL01" {
        cpu.bus
            .log_string("[W16] ThunkConnect32: thunk data with an unknown magic");
        return Some(0);
    }
    let dll16 = guest_string(cpu, dll16_at);
    let name16 = guest_string(cpu, name_at);
    let pid = running_pid(cpu);
    // The 16-bit DLL is looked for beside the 32-bit DLL that has the
    // thunk data, as well as where LoadLibrary looks.
    let dir = crate::win32::running(cpu)
        .and_then(|p| {
            p.modules
                .iter()
                .find(|m| data32.wrapping_sub(m.base) < m.size)
                .map(|m| crate::win32::loader::directory_of(&m.path))
        })
        .unwrap_or_default();
    let Some(tid) = crate::win32::running(cpu)
        .and_then(|p| p.current_thread())
        .map(|t| t.id)
    else {
        return Some(0);
    };
    if ensure_client(cpu, pid).is_none() {
        return Some(0);
    }
    post(
        cpu,
        pid,
        Request {
            kind: CONNECT,
            tid,
            data32,
            names: vec![dll16, name16, dir],
            ..Request::default()
        },
    );
    None
}

/// QT_Thunk: call the 16-bit function at EDX with the arguments the
/// caller pushed below its frame's thunk buffer (from ESP above the
/// return address to EBP - 40h). It gets the caller's registers; the
/// caller gets EAX, ECX and EDX back, and its stack without the
/// arguments the function took off.
pub fn qt_thunk(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let esp = cpu.esp();
    let ebp = cpu.ebp();
    let ret = cpu.bus.guest_read_32(esp);
    let from = esp.wrapping_add(4);
    let to = ebp.wrapping_sub(QT_FRAME);
    let len = to.saturating_sub(from).min(0x1000) & !1;
    let args = guest_bytes(cpu, from, len as usize);
    let pid = running_pid(cpu);
    let tid = crate::win32::running(cpu)
        .and_then(|p| p.current_thread())
        .map(|t| t.id)?;
    if ensure_client(cpu, pid).is_none() {
        crate::win32::fatal(cpu, "a flat thunk's call with no Win16 side", None);
        return None;
    }
    let mut regs = Context::of(cpu);
    regs.eip = ret;
    regs.gpr[ESP] = from;
    post(
        cpu,
        pid,
        Request {
            kind: CALL,
            tid,
            target: cpu.edx(),
            regs,
            args,
            ..Request::default()
        },
    );
    None
}

/// FT_Prolog, the thunk compiler's other way of calling 16-bit code,
/// which the built-in Windows doesn't have.
pub fn ft_prolog(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    crate::win32::fatal(
        cpu,
        "a flat thunk through FT_Prolog, which the built-in Windows doesn't have yet",
        None,
    );
    None
}

/// The Win16 loader's errors LoadLibrary16 passes on.
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_BAD_FORMAT: u32 = 11;

/// LoadLibrary16(name), KERNEL32's ordinal 35 on Windows 95 and 98: the
/// Win16 LoadLibrary, for a Win32 program. The process's task in the
/// Win16 process loads the DLL, where its thunks' calls run, and keeps
/// a reference to it until the process ends or frees it. The calling
/// thread waits, and gets the DLL's instance or an error below 32.
pub fn load_library16(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = guest_string(cpu, args[0]);
    let pid = running_pid(cpu);
    let dir = crate::win32::running(cpu)
        .and_then(|p| p.modules.first())
        .map(|m| crate::win32::loader::directory_of(&m.path))
        .unwrap_or_default();
    let Some(tid) = crate::win32::running(cpu)
        .and_then(|p| p.current_thread())
        .map(|t| t.id)
    else {
        return Some(ERROR_FILE_NOT_FOUND);
    };
    if ensure_client(cpu, pid).is_none() {
        return Some(ERROR_FILE_NOT_FOUND);
    }
    post(
        cpu,
        pid,
        Request {
            kind: LOAD,
            tid,
            names: vec![name, String::new(), dir],
            ..Request::default()
        },
    );
    None
}

/// FreeLibrary16(instance), ordinal 36: a reference LoadLibrary16 took
/// goes, and the DLL with the last.
pub fn free_library16(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let pid = running_pid(cpu);
    let handle =
        super::module_index(cpu, args[0] as u16).map(|i| cpu.bus.win32.win16.modules[i].handle);
    let clients = &mut cpu.bus.win32.win16.clients;
    let held = clients.iter_mut().find(|c| c.pid == pid).and_then(|c| {
        let i = c.modules.iter().position(|&m| Some(m) == handle)?;
        Some(c.modules.remove(i))
    });
    if let Some(handle) = held {
        module::release(cpu, handle);
    }
    Some(0)
}

/// GetProcAddress16(module, name), ordinal 37: the 16:16 address of the
/// 16-bit DLL's function, named by a string or, below 10000h, by its
/// ordinal; 0 if it has none.
pub fn get_proc_address16(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let module = args[0] as u16;
    if super::module_index(cpu, module).is_none() {
        return Some(0);
    }
    let ordinal = if args[1] < 0x1_0000 {
        Some(args[1] as u16)
    } else {
        let name = guest_string(cpu, args[1]);
        module::ordinal_of(cpu, module, &name)
    };
    Some(ordinal.map_or(0, |o| module::entry_point(cpu, module, o)))
}

pub fn map_ls_api(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(map_ls(cpu, args[0]))
}

pub fn unmap_ls_api(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    unmap_ls(cpu, args[0]);
    Some(0)
}

pub fn map_sl_api(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(map_sl(cpu, args[0]))
}

/// UnMapSLFixArray(count, array): MapSLFix's segments needn't stay
/// where they are any more, which they always do.
pub fn unmap_sl_fix_array(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// SMapLS, a register function: EAX mapped as MapLS maps it, EDX the
/// same, or 0 for a value that isn't a pointer.
pub fn smap_ls(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let value = cpu.eax();
    if value >> 16 == 0 {
        cpu.set_edx(0);
        return Some(value);
    }
    let segptr = map_ls(cpu, value);
    cpu.set_edx(segptr);
    Some(segptr)
}

/// SUnMapLS, a register function: the pointer in EAX, if SMapLS made
/// one, is let go. The registers stay as they are.
pub fn sunmap_ls(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let value = cpu.eax();
    if value >> 16 != 0 {
        unmap_ls(cpu, value);
    }
    None
}

/// SMapLS_IP_EBP_n: the pointer argument at EBP + n mapped as MapLS maps
/// it, in EAX and in its place on the stack, which a value that isn't a
/// pointer leaves 0 for SUnMapLS_IP_EBP_n.
pub fn smap_ls_ebp(cpu: &mut Cpu, offset: u32) -> Option<u32> {
    let at = cpu.ebp().wrapping_add(offset);
    let value = cpu.bus.guest_read_32(at);
    let segptr = if value >> 16 == 0 {
        cpu.bus.guest_write_32(at, 0);
        value
    } else {
        let segptr = map_ls(cpu, value);
        cpu.bus.guest_write_32(at, segptr);
        segptr
    };
    Some(segptr)
}

/// SUnMapLS_IP_EBP_n: the pointer SMapLS_IP_EBP_n put at EBP + n is let
/// go. The registers stay as they are.
pub fn sunmap_ls_ebp(cpu: &mut Cpu, offset: u32) -> Option<u32> {
    let at = cpu.ebp().wrapping_add(offset);
    let segptr = cpu.bus.guest_read_32(at);
    if segptr != 0 {
        unmap_ls(cpu, segptr);
        cpu.bus.guest_write_32(at, 0);
    }
    None
}

/// The SMapLS_IP_EBP_n and SUnMapLS_IP_EBP_n functions, for n from 8 to
/// 40, as KERNEL32 exports them.
macro_rules! ebp_functions {
    ($($n:literal: $map:ident, $unmap:ident;)*) => {
        $(
            pub fn $map(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
                smap_ls_ebp(cpu, $n)
            }
            pub fn $unmap(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
                sunmap_ls_ebp(cpu, $n)
            }
        )*
    };
}

ebp_functions! {
    8: smap_ls_ebp_8, sunmap_ls_ebp_8;
    12: smap_ls_ebp_12, sunmap_ls_ebp_12;
    16: smap_ls_ebp_16, sunmap_ls_ebp_16;
    20: smap_ls_ebp_20, sunmap_ls_ebp_20;
    24: smap_ls_ebp_24, sunmap_ls_ebp_24;
    28: smap_ls_ebp_28, sunmap_ls_ebp_28;
    32: smap_ls_ebp_32, sunmap_ls_ebp_32;
    36: smap_ls_ebp_36, sunmap_ls_ebp_36;
    40: smap_ls_ebp_40, sunmap_ls_ebp_40;
}

/// `len` bytes at `at` of the running process.
fn guest_bytes(cpu: &mut Cpu, at: u32, len: usize) -> Vec<u8> {
    let mut bytes = vec![0; len];
    cpu.bus.guest_read_bytes(at, &mut bytes);
    bytes
}

fn guest_string(cpu: &mut Cpu, at: u32) -> String {
    let bytes = cpu.bus.guest_read_asciiz(at, 0x200);
    crate::dosstr::from_bytes(&bytes)
}

// The client tasks.

/// The client of the Win32 process `pid`, made with its task if there is
/// none: its place in `State::clients`.
fn ensure_client(cpu: &mut Cpu, pid: u32) -> Option<usize> {
    if let Some(c) = cpu
        .bus
        .win32
        .win16
        .clients
        .iter()
        .position(|c| c.pid == pid)
    {
        return Some(c);
    }
    ensure_win16(cpu)?;
    let name = crate::win32::running(cpu)
        .and_then(|p| p.modules.first())
        .map_or("WIN32".to_string(), |m| {
            m.name.split('.').next().unwrap_or("WIN32").to_string()
        });
    let ctx = Context {
        eip: crate::win32::kernel32::address_of("Win16Serve"),
        eflags: 0x0202,
        seg: [
            crate::win32::system::DATA3,
            crate::win32::system::CODE3,
            crate::win32::system::DATA3,
            crate::win32::system::DATA3,
            0,
            0,
        ],
        ..Context::default()
    };
    match super::task::client_task(cpu, &name, ctx) {
        Ok((tid, _)) => {
            cpu.bus.win32.win16.clients.push(Client {
                pid,
                tid,
                ..Client::default()
            });
            Some(cpu.bus.win32.win16.clients.len() - 1)
        }
        Err(message) => {
            cpu.bus.log_string(&format!("[W16] {}", message));
            None
        }
    }
}

/// The running Win32 thread asks the client of the process `pid` for
/// `request`, and waits until it is done. The client is woken if it
/// waits for one.
fn post(cpu: &mut Cpu, pid: u32, request: Request) {
    let Some(c) = cpu
        .bus
        .win32
        .win16
        .clients
        .iter()
        .position(|c| c.pid == pid)
    else {
        return;
    };
    let client = &mut cpu.bus.win32.win16.clients[c];
    client.queue.push(request);
    let tid = client.tid;
    let idle = client.current.is_none();
    if idle && let Some(at) = crate::win32::thread::find(&cpu.bus.win32, tid) {
        crate::win32::thread::end_wait(&mut cpu.bus.win32, at, 0);
    }
    crate::win32::thread::block(cpu, Wait::default());
}

/// The place in `State::clients` of the client whose task's thread is
/// `tid`. The 16-bit code a client runs can give up its turn, and a
/// client that goes meanwhile moves the others down the list, so what
/// runs after it keeps the thread, which stays the same.
fn client_of(cpu: &Cpu, tid: u32) -> Option<usize> {
    cpu.bus
        .win32
        .win16
        .clients
        .iter()
        .position(|c| c.tid == tid)
}

/// KERNEL32's `Win16Serve`, where a client task's thread waits: it runs
/// the next request, or waits for one.
pub fn serve(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let tid = crate::win32::running(cpu)
        .and_then(|p| p.current_thread())
        .map_or(0, |t| t.id);
    let Some(c) = client_of(cpu, tid) else {
        crate::win32::fatal(
            cpu,
            "a call of Win16Serve from a thread that isn't a client's",
            None,
        );
        return None;
    };
    let client = &mut cpu.bus.win32.win16.clients[c];
    if client.current.is_none() && !client.queue.is_empty() {
        let request = client.queue.remove(0);
        client.current = Some(request.clone());
        match request.kind {
            CALL => start_call(cpu, c, request),
            LOAD => start_load(cpu, c, request),
            _ => start_connect(cpu, c, request),
        }
        return None;
    }
    crate::win32::thread::block(cpu, Wait::default());
    None
}

/// The Win32 thread `tid` goes on, `f` having set its registers, with
/// `eax` in EAX.
fn finish(cpu: &mut Cpu, tid: u32, eax: u32, f: impl FnOnce(&mut Context)) {
    let Some((p, t)) = crate::win32::thread::find(&cpu.bus.win32, tid) else {
        return;
    };
    f(&mut cpu.bus.win32.processes[p].threads[t].ctx);
    crate::win32::thread::end_wait(&mut cpu.bus.win32, (p, t), eax);
}

/// The client `c` is done with its request: the Win32 thread goes on
/// with `eax`.
fn done(cpu: &mut Cpu, c: usize, eax: u32) {
    if let Some(request) = cpu.bus.win32.win16.clients[c].current.take() {
        finish(cpu, request.tid, eax, |_| ());
    }
}

/// Run the call `request` on the client `c`'s task, from Win16Serve.
fn start_call(cpu: &mut Cpu, c: usize, request: Request) {
    let resume = Context::of(cpu);
    let (ss, sp) = thunk::stack16_for(cpu, &resume);
    let len = request.args.len() as u16;
    // The arguments as they were on the 32-bit stack, the lowest at the
    // top of the 16-bit one: pushed from the highest word down.
    let words: Vec<u16> = request
        .args
        .chunks(2)
        .rev()
        .map(|w| u16::from_le_bytes([w[0], w.get(1).copied().unwrap_or(0)]))
        .collect();
    if let Some(current) = cpu.bus.win32.win16.clients[c].current.as_mut() {
        current.sp = sp.wrapping_sub(len) as u32;
    }
    let mut regs = Context::default();
    for r in [EAX, EBX, ECX, EDX, ESI, EDI] {
        regs.gpr[r] = request.regs.gpr[r];
    }
    let tid = cpu.bus.win32.win16.clients[c].tid;
    thunk::call16_regs(
        cpu,
        request.target,
        &words,
        regs,
        resume,
        thunk::THEN_FLAT_CALL,
        vec![tid],
        (ss, sp),
    );
}

/// The 16-bit function a client called returned (`start_call`): the
/// Win32 thread goes on with its EAX, ECX and EDX, and its stack without
/// the arguments the function took off.
pub fn then_call(cpu: &mut Cpu, _: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    let tid = data.first().copied().unwrap_or(0);
    let request = client_of(cpu, tid).and_then(|c| cpu.bus.win32.win16.clients[c].current.take());
    if let Some(request) = request {
        let popped = (cpu.sp() as u32).wrapping_sub(request.sp) & 0xFFFF;
        let (eax, ecx, edx) = (cpu.eax(), cpu.ecx(), cpu.edx());
        finish(cpu, request.tid, eax, |t| {
            t.gpr[ECX] = ecx;
            t.gpr[EDX] = edx;
            t.eip = request.regs.eip;
            t.gpr[ESP] = request.regs.gpr[ESP].wrapping_add(popped);
        });
    }
    Some(ctx.gpr[EAX])
}

/// Load the 16-bit DLL of the ThunkConnect32 `request` on the client
/// `c`'s task, from Win16Serve, and run the entry points of the modules
/// that came with it.
fn start_connect(cpu: &mut Cpu, c: usize, request: Request) {
    let mut inits = Vec::new();
    let (dll, dir) = (&request.names[0], &request.names[2]);
    let index = match module::load(cpu, dll, dir, &mut inits) {
        Ok(index) => index,
        Err(message) => {
            cpu.bus
                .log_string(&format!("[W16] ThunkConnect32: {}", message));
            done(cpu, c, 0);
            return;
        }
    };
    let handle = cpu.bus.win32.win16.modules[index].handle;
    let client = &mut cpu.bus.win32.win16.clients[c];
    // One reference to it for each process that uses it.
    let attached = client.modules.contains(&handle);
    if !attached {
        client.modules.push(handle);
    }
    if let Some(current) = client.current.as_mut() {
        current.module = handle;
    }
    let tid = client.tid;
    if attached {
        module::release(cpu, handle);
    }
    let mut resume = Context::of(cpu);
    let joined = if inits.contains(&index) || attached {
        let mut data = vec![tid];
        data.extend(
            inits
                .iter()
                .map(|&i| cpu.bus.win32.win16.modules[i].handle as u32),
        );
        then_inits(cpu, 0, &data, &mut resume)
    } else {
        // Loaded already, for another process: as in Windows 95, its
        // DllEntryPoint runs for this one too.
        let data = [thunk::THEN_FLAT_INITS as u32, handle as u32, tid];
        super::task::then_dll_entry(cpu, 0, &data, &mut resume)
    };
    if joined.is_some() {
        crate::win32::system::load_context(cpu, &resume);
    }
}

/// The modules `data[1..]` are to have their entry points run, for the
/// ThunkConnect32 of the client whose thread is `data[0]`, which then
/// joins the thunk data. None when an entry point runs first.
pub fn then_inits(cpu: &mut Cpu, _: u32, data: &[u32], ctx: &mut Context) -> Option<u32> {
    let tid = data.first().copied().unwrap_or(0);
    if data.len() > 1 {
        let mut rest = vec![tid];
        rest.extend_from_slice(&data[2..]);
        super::task::call_lib_init(cpu, data[1] as u16, *ctx, thunk::THEN_FLAT_INITS, rest);
        return None;
    }
    if let Some(c) = client_of(cpu, tid) {
        let load = cpu.bus.win32.win16.clients[c].current.as_ref();
        if let Some(r) = load.filter(|r| r.kind == LOAD) {
            let instance = r.module as u32;
            done(cpu, c, instance);
        } else {
            let joined = connect(cpu, c);
            done(cpu, c, joined as u32);
        }
    }
    Some(ctx.gpr[EAX])
}

/// Load the 16-bit DLL of the LoadLibrary16 `request` on the client
/// `c`'s task, from Win16Serve, and run the entry points of the modules
/// that came with it, as the Win16 LoadLibrary does. The Win32 thread
/// gets the DLL's instance, or the Win16 loader's error below 32.
fn start_load(cpu: &mut Cpu, c: usize, request: Request) {
    let mut inits = Vec::new();
    let (dll, dir) = (&request.names[0], &request.names[2]);
    let index = match module::load(cpu, dll, dir, &mut inits) {
        Ok(index) => index,
        Err(message) => {
            cpu.bus
                .log_string(&format!("[W16] LoadLibrary16({}): {}", dll, message));
            let error = if message.contains("found") {
                ERROR_FILE_NOT_FOUND
            } else {
                ERROR_BAD_FORMAT
            };
            done(cpu, c, error);
            return;
        }
    };
    let m = &cpu.bus.win32.win16.modules[index];
    let (handle, instance) = (m.handle, if m.dgroup != 0 { m.dgroup } else { m.handle });
    let client = &mut cpu.bus.win32.win16.clients[c];
    client.modules.push(handle);
    if let Some(current) = client.current.as_mut() {
        current.module = instance;
    }
    let mut data = vec![client.tid];
    data.extend(
        inits
            .iter()
            .map(|&i| cpu.bus.win32.win16.modules[i].handle as u32),
    );
    let mut resume = Context::of(cpu);
    if then_inits(cpu, 0, &data, &mut resume).is_some() {
        crate::win32::system::load_context(cpu, &resume);
    }
}

/// Join the thunk data of the client `c`'s ThunkConnect32, whose 16-bit
/// DLL is loaded: whether they joined.
fn connect(cpu: &mut Cpu, c: usize) -> bool {
    let Some(client) = cpu.bus.win32.win16.clients.get(c) else {
        return false;
    };
    let pid = client.pid;
    let Some(request) = client.current.clone() else {
        return false;
    };
    let (dll, name) = (request.names[0].clone(), request.names[1].clone());
    let fail = |cpu: &mut Cpu, why: &str| {
        cpu.bus
            .log_string(&format!("[W16] ThunkConnect32 with {}: {}", dll, why));
        false
    };
    let Some(entry) = module::ordinal_of(cpu, request.module, &name)
        .map(|o| module::entry_point(cpu, request.module, o))
        .filter(|&e| e != 0)
    else {
        return fail(cpu, &format!("it has no {}", name));
    };
    let data16 = super::linear(cpu, entry);
    let win16 = cpu.bus.win32.win16.pid;
    let (Some(header16), Some(data32)) = (
        read(cpu, win16, data16, 0x2C),
        read(cpu, pid, request.data32, DATA32),
    ) else {
        return fail(cpu, "its thunk data isn't there");
    };
    if header16[..HEADER] != data32[..HEADER] {
        return fail(cpu, "its thunk data doesn't match the 32-bit DLL's");
    }
    let word = |b: &[u8], at: usize| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    if &data32[..4] == b"LS01" {
        // The 16-bit DLL's table of targets, which the code written here
        // reads.
        let table16 = word(&header16, 8);
        let Some((_, limit, _)) = super::selector_base(cpu, (table16 >> 16) as u16) else {
            return fail(cpu, "its table of targets isn't there");
        };
        let at = super::linear(cpu, table16);
        let len = limit.saturating_add(1).saturating_sub(table16 & 0xFFFF);
        let Some(table) =
            aliased(cpu, pid, win16, at, len).or_else(|| alias(cpu, win16, at, len, pid, 0))
        else {
            return fail(cpu, "there is no room for its table of targets");
        };
        let qt = request.data32.wrapping_add(word(&data32, 0x1C));
        let ft = request.data32.wrapping_add(word(&data32, 0x20));
        let ok = write(cpu, pid, request.data32 + 8, &table.to_le_bytes())
            && write(cpu, pid, qt, &qt_code(table))
            && write(cpu, pid, ft, &ft_code(table));
        if !ok {
            return fail(cpu, "its thunk data can't be written");
        }
    } else {
        let links = &cpu.bus.win32.win16.links;
        let Some(l) = links.iter().position(|l| l.data16 == data16) else {
            return fail(cpu, "its DllEntryPoint didn't call ThunkConnect16");
        };
        if !write(
            cpu,
            pid,
            request.data32 + 0x0C,
            &(l as u32 + 1).to_le_bytes(),
        ) {
            return fail(cpu, "its thunk data can't be written");
        }
    }
    cpu.bus.log_string(&format!(
        "[W16] ThunkConnect32: {} joined with {}",
        name, dll
    ));
    true
}

/// The code ThunkConnect32 writes for a 32-bit to 16-bit thunk's call,
/// as Windows 95 writes it: the target's 16:16 address from the 16-bit
/// table at `table`, by the number the thunk keeps at EBP - 4, into EDX,
/// then on to QT_Thunk.
fn qt_code(table: u32) -> Vec<u8> {
    let mut code = vec![0x33, 0xC9, 0x8A, 0x4D, 0xFC, 0x8B, 0x14, 0x8D];
    code.extend_from_slice(&table.to_le_bytes());
    code.push(0xB8);
    code.extend_from_slice(&crate::win32::kernel32::address_of("QT_Thunk").to_le_bytes());
    code.extend_from_slice(&[0xFF, 0xE0]);
    code
}

/// The code ThunkConnect32 writes for the FT_Prolog thunks: the target,
/// by the number in CL, into EDX, then on to FT_Prolog.
fn ft_code(table: u32) -> Vec<u8> {
    let mut code = vec![0x0F, 0xB6, 0xD1, 0x8B, 0x14, 0x95];
    code.extend_from_slice(&table.to_le_bytes());
    code.push(0x68);
    code.extend_from_slice(&crate::win32::kernel32::address_of("FT_Prolog").to_le_bytes());
    code.push(0xC3);
    code
}

// KERNEL's functions.

/// ThunkConnect16(16-bit DLL, 32-bit DLL, instance, reason, thunk data,
/// 32-bit thunk data's name, code segment), from the 16-bit DLL's
/// DllEntryPoint: for a 16-bit to 32-bit thunk, its data is joined for
/// the ThunkConnect32 to come. 1.
pub fn thunk_connect16(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let data16 = super::linear(cpu, args[4]);
    if data16 == 0 {
        return Some(0);
    }
    let magic = super::peek(cpu, data16, 4).unwrap_or_default();
    if magic == b"SL01" {
        let links = &mut cpu.bus.win32.win16.links;
        let l = match links.iter().position(|l| l.data16 == data16) {
            Some(l) => l,
            None => {
                links.push(Link { data16 });
                links.len() - 1
            }
        };
        // Its fpData, which says it is joined.
        super::write_32(cpu, data16 + 0x10, l as u32 + 1);
    } else if magic != b"LS01" {
        return Some(0);
    }
    Some(1)
}

/// C16ThkSL01, a register function, where the thunk compiler's 16-bit
/// to 32-bit thunks go. With EAX a 16:16 pointer, the first time: the
/// thunk's code there is made to come back here with EAX 0 and EDX its
/// thunk data, and runs. With EAX 0, a thunk's call of the 32-bit target
/// CX / 4: that fails, as when no Win32 process joined the thunk, with
/// the error value the thunk data has for the target, its arguments
/// taken off.
pub fn c16_thk_sl01(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let mut ctx = Context::of(cpu);
    let stub = ctx.gpr[EAX];
    if stub != 0 {
        let at = super::linear(cpu, stub);
        let back = super::builtin::stub_address(cpu, 0, "C16ThkSL01").unwrap_or(0);
        let mut code = vec![0x66, 0x33, 0xC0, 0x66, 0xBA];
        code.extend_from_slice(&ctx.gpr[EDX].to_le_bytes());
        code.push(0xEA);
        code.extend_from_slice(&back.to_le_bytes());
        super::poke(cpu, at, &code);
        ctx.seg[Seg::CS as usize] = (stub >> 16) as u16;
        ctx.eip = stub & 0xFFFF;
        crate::win32::system::load_context(cpu, &ctx);
        return None;
    }
    let data16 = super::linear(cpu, ctx.gpr[EDX]);
    let target = (ctx.gpr[ECX] & 0xFFFF) / 4;
    // The thunk data's API database: each target's bytes of arguments
    // and error value.
    let database = super::linear(cpu, super::read_32(cpu, data16 + 0x28));
    let entry = database.wrapping_add(8 * target);
    let (arg_bytes, error) = if database != 0 {
        (super::read_32(cpu, entry), super::read_32(cpu, entry + 4))
    } else {
        (0, 0)
    };
    cpu.bus.log_string(&format!(
        "[W16] 16-bit code called 32-bit code through a flat thunk (target {}), \
         which the built-in Windows doesn't have: it fails",
        target
    ));
    let sp = ctx.gpr[ESP] as u16;
    let base = cpu.seg_cache(Seg::SS).base;
    let ip = cpu.bus.guest_read_16(base.wrapping_add(sp as u32));
    let cs = cpu
        .bus
        .guest_read_16(base.wrapping_add(sp.wrapping_add(2) as u32));
    thunk::set_ax(&mut ctx, error as u16);
    ctx.gpr[EDX] = ctx.gpr[EDX] & 0xFFFF_0000 | error >> 16;
    ctx.seg[Seg::CS as usize] = cs;
    ctx.eip = ip as u32;
    thunk::set_sp(&mut ctx, sp.wrapping_add(4).wrapping_add(arg_bytes as u16));
    crate::win32::system::load_context(cpu, &ctx);
    None
}

// Processes ending.

/// The Win32 threads that wait for `client`'s requests go on, their
/// calls failed: EAX 0, and a call's caller back at its return address.
fn fail_requests(cpu: &mut Cpu, client: Client) {
    for request in client.current.into_iter().chain(client.queue) {
        finish(cpu, request.tid, 0, |t| {
            if request.kind == CALL {
                t.eip = request.regs.eip;
                t.gpr[ESP] = request.regs.gpr[ESP];
            }
        });
    }
}

/// The task whose thread is `tid` ends by itself (task::exit, after a
/// fault in its code too): if it is a client, it serves no more, and the
/// Win32 threads that wait for it go on, their calls failed.
pub fn task_ending(cpu: &mut Cpu, tid: u32) {
    if let Some(c) = client_of(cpu, tid) {
        let client = cpu.bus.win32.win16.clients.remove(c);
        release_modules(cpu, &client);
        fail_requests(cpu, client);
    }
}

/// The references to 16-bit DLLs `client`'s ThunkConnect32s took go.
fn release_modules(cpu: &mut Cpu, client: &Client) {
    for &handle in &client.modules {
        module::release(cpu, handle);
    }
}

/// The process `pid` ends: the aliases in it and of its memory go, and
/// a Win32 process's client task ends, and its references to 16-bit
/// DLLs with it.
pub fn process_ending(cpu: &mut Cpu, pid: u32) {
    let win16 = cpu.bus.win32.win16.pid;
    if win16 == 0 {
        return;
    }
    while let Some(i) = cpu
        .bus
        .win32
        .win16
        .aliases
        .iter()
        .position(|a| a.owner == pid || a.source == pid)
    {
        unalias(cpu, i);
    }
    if pid == win16 {
        for client in std::mem::take(&mut cpu.bus.win32.win16.clients) {
            fail_requests(cpu, client);
        }
        return;
    }
    let clients = &mut cpu.bus.win32.win16.clients;
    if let Some(c) = clients.iter().position(|c| c.pid == pid) {
        let client = clients.remove(c);
        super::task::end_client_task(cpu, client.tid);
        release_modules(cpu, &client);
    }
}

/// The Win16 process's place among the processes if it has nothing left
/// to run: no task, its last one a Win32 process's client that ended
/// with that process. Its place and the exit code it ends with, its
/// last program's.
pub fn unneeded(cpu: &Cpu) -> Option<(usize, u32)> {
    let win16 = &cpu.bus.win32.win16;
    if win16.pid == 0 || win16.tasks.iter().any(|t| t.exit_code.is_none()) {
        return None;
    }
    cpu.bus
        .win32
        .processes
        .iter()
        .position(|p| p.id == win16.pid)
        .map(|k| (k, win16.exit_code as u32))
}

crate::state_fields!(Client {
    pid,
    tid,
    modules,
    queue,
    current
});
crate::state_fields!(Request {
    kind,
    tid,
    target,
    regs,
    args,
    sp,
    data32,
    names,
    module
});
crate::state_fields!(Alias {
    owner,
    source,
    window,
    pages,
    address,
    selector
});
crate::state_fields!(Link { data16 });
