//! Win32s's Universal Thunks, which Windows 95 and 98 keep for programs
//! written for Win32s: a Win32 DLL registers a 16-bit DLL with KERNEL32's
//! UTRegister and gets a 32-bit function, its thunk, that calls the
//! 16-bit DLL's UTProc with a buffer and a value. The 16-bit code finds
//! the Win32 side's memory with WIN32S16's UTLinearToSelectorOffset.
//!
//! As on Windows 98, where WIN32S16.DLL does the 16-bit half, UTRegister
//! fails for a DLL that asks for a 32-bit callback, which only Win32s
//! itself has. Otherwise it loads the 16-bit DLL with the Win16
//! LoadLibrary, calls its UTInit, if it names one, with no callback and
//! the buffer, and writes the thunk's code into the system heap. A call
//! of the thunk turns the pointers its translation list points to into
//! 16:16 ones in place, and the buffer's address, with
//! UTLinearToSelectorOffset. That gives each 32 KB of a process's memory
//! one selector for 64 KB from there, which it keeps until the process
//! ends, so that an object of up to 32 KB fits under it wherever it
//! starts. The 16-bit code runs on the process's client (flat.rs), and
//! the thread gets UTProc's DX:AX. A process has ten at most.

use super::builtin::{Def, rust};
use super::flat::{self, Request, UT_REGISTER};
use super::module;
use super::thunk::EAX;
use crate::cpu::Cpu;
use crate::dpmi::Context;
use crate::savestate::Since;

/// The W32 state version that first saves the Universal Thunks
/// (`State::universal` and `Request::held`).
pub const UT_SAVED: u16 = 92;

/// The thunks a process can have, as WIN32S16.DLL's table has room for.
const MAX_THUNKS: usize = 10;
/// The block of a process's memory each of UTLinearToSelectorOffset's
/// selectors is for.
const BLOCK: u32 = 0x8000;
/// The translation list's entries read at most: Windows 98 reads until
/// the 0 that ends it, and faults at the end of the memory without one.
const MAX_LIST: u32 = 0x1000;
/// The bytes of a thunk's code (`thunk_code`): pop, push, push and jmp.
const THUNK_SIZE: u32 = 12;

/// What the Universal Thunks keep of the Win16 process.
#[derive(Clone, Debug, Default)]
pub struct Universal {
    /// The thunks registered.
    pub thunks: Vec<Thunk>,
    /// The selectors UTLinearToSelectorOffset made.
    pub selectors: Vec<Selector>,
}

/// A Win32 DLL's thunk: the process, the DLL's handle, the 16-bit DLL
/// loaded for it, and the thunk's code in the process's system heap.
#[derive(Clone, Debug, Default)]
pub struct Thunk {
    pub pid: u32,
    pub module32: u32,
    pub module16: u16,
    pub code: u32,
}

/// The selector of the 32 KB block at `base` of the process `source`'s
/// memory.
#[derive(Clone, Debug, Default)]
pub struct Selector {
    pub source: u32,
    pub base: u32,
    pub selector: u16,
}

// KERNEL32's functions.

/// UTRegister(Win32 DLL, 16-bit DLL, UTInit's name, UTProc's name, where
/// the thunk goes, 32-bit callback, buffer): register the 16-bit DLL's
/// UTProc as the Win32 DLL's Universal Thunk. The calling thread waits
/// while its client loads the DLL and runs its UTInit, and gets TRUE, or
/// FALSE when the DLL has a thunk already, the process ten, the 16-bit
/// DLL doesn't load or lacks a function, UTInit returns 0, or there is a
/// callback.
pub fn ut_register(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (module32, dll_at, init_at, proc_at) = (args[0], args[1], args[2], args[3]);
    let (thunk_at, callback, buffer) = (args[4], args[5], args[6]);
    if callback != 0 {
        cpu.bus.log_string(
            "[W16] UTRegister with a 32-bit callback, which Windows 98's WIN32S16 \
             refuses: it fails",
        );
        return Some(0);
    }
    let pid = flat::running_pid(cpu);
    let thunks = &cpu.bus.win32.win16.universal.thunks;
    let mine = || thunks.iter().filter(|t| t.pid == pid);
    if mine().any(|t| t.module32 == module32) || mine().count() >= MAX_THUNKS {
        return Some(0);
    }
    let dll = flat::guest_string(cpu, dll_at);
    let init = function_name(cpu, init_at);
    let proc = function_name(cpu, proc_at);
    // The thunk's address is written from the Win16 side, where a page
    // that isn't there can't fault for the program: one that isn't
    // faults now.
    if thunk_at != 0 {
        cpu.bus.guest_probe(thunk_at, 4, true);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    let Some(tid) = crate::win32::running(cpu)
        .and_then(|p| p.current_thread())
        .map(|t| t.id)
    else {
        return Some(0);
    };
    // The Win16 LoadLibrary of the process's task, which looks in its
    // program's directory too, as LoadLibrary16's does.
    let dir = crate::win32::running(cpu)
        .and_then(|p| p.modules.first())
        .map(|m| crate::win32::loader::directory_of(&m.path))
        .unwrap_or_default();
    if flat::ensure_client(cpu, pid).is_none() {
        return Some(0);
    }
    let request = Request {
        kind: UT_REGISTER,
        tid,
        names: vec![dll, String::new(), dir, init, proc],
        data32: thunk_at,
        held: Since(vec![module32, buffer]),
        ..Request::default()
    };
    flat::post(cpu, pid, request);
    None
}

/// UTUnRegister(Win32 DLL): the DLL's thunk goes, and its reference to
/// the 16-bit DLL, which goes with the last.
pub fn ut_unregister(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let pid = flat::running_pid(cpu);
    let thunks = &mut cpu.bus.win32.win16.universal.thunks;
    let Some(i) = thunks
        .iter()
        .position(|t| t.pid == pid && t.module32 == args[0])
    else {
        return Some(0);
    };
    let thunk = thunks.remove(i);
    super::with_win16(cpu, pid, |p, _| p.system_free(thunk.code));
    flat::release_module(cpu, pid, thunk.module16);
    Some(1)
}

/// UTGlue32(UTProc, buffer, value, translation list), stdcall, where each
/// thunk's code goes with its UTProc's 16:16 address pushed under the
/// thunk's three arguments: the pointers the list's entries point to,
/// up to the 0 that ends it, become 16:16 ones in place, and UTProc is
/// called with the buffer's 16:16 address and the value, PASCAL, on the
/// process's client. The thread gets DX:AX.
pub fn ut_glue32(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (proc16, buffer, value, list) = (args[0], args[1], args[2], args[3]);
    let pid = flat::running_pid(cpu);
    // Every page the list's work reads or writes is there before any
    // pointer changes: a page that faults runs the call again, which
    // would turn a pointer turned already once more.
    let mut places = Vec::new();
    if list != 0 {
        for i in 0..MAX_LIST {
            let at = cpu.bus.guest_read_32(list.wrapping_add(4 * i));
            if at == 0 || cpu.bus.guest_faulted() {
                break;
            }
            cpu.bus.guest_probe(at, 4, true);
            places.push(at);
        }
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    for at in places {
        let pointer = cpu.bus.guest_read_32(at);
        let segptr = linear_to_segptr(cpu, pid, pointer);
        cpu.bus.guest_write_32(at, segptr);
    }
    let buffer16 = linear_to_segptr(cpu, pid, buffer);
    let mut bytes = value.to_le_bytes().to_vec();
    bytes.extend_from_slice(&buffer16.to_le_bytes());
    flat::callback16(cpu, flat::CALLBACK, proc16, bytes, 0)
}

/// A function's name in UTRegister's arguments: empty for none, "#n" for
/// the ordinal n, which a value below 64 KB is.
fn function_name(cpu: &mut Cpu, at: u32) -> String {
    match at {
        0 => String::new(),
        1..0x1_0000 => format!("#{}", at),
        _ => flat::guest_string(cpu, at),
    }
}

/// The 16:16 address of the function `name` (`function_name`) of the
/// 16-bit module `module`, 0 if it has none.
fn entry(cpu: &Cpu, module: u16, name: &str) -> u32 {
    let ordinal = match name.strip_prefix('#') {
        Some(number) => number.parse().ok(),
        None => module::ordinal_of(cpu, module, name),
    };
    ordinal.map_or(0, |o| module::entry_point(cpu, module, o))
}

// The client's side.

/// The 16-bit DLL of the client `c`'s UTRegister is loaded and its entry
/// points ran: UTInit is called with the buffer, if the DLL names one,
/// and goes on in `initialized`; otherwise the thunk is made at once.
/// None when UTInit runs.
pub(super) fn loaded(cpu: &mut Cpu, c: usize, ctx: &mut Context) -> Option<u32> {
    let client = &cpu.bus.win32.win16.clients[c];
    let pid = client.pid;
    let Some(request) = client.current.clone() else {
        return Some(ctx.gpr[EAX]);
    };
    let module16 = request.module;
    let (init, proc) = (&request.names[3], &request.names[4]);
    let why = if entry(cpu, module16, proc) == 0 {
        format!("it has no {}", proc)
    } else if init.is_empty() {
        let registered = register(cpu, pid, &request);
        flat::done(cpu, c, registered as u32);
        return Some(ctx.gpr[EAX]);
    } else {
        let init16 = entry(cpu, module16, init);
        let buffer = request.held.get(1).copied().unwrap_or(0);
        let buffer16 = linear_to_segptr(cpu, pid, buffer);
        if init16 == 0 {
            format!("it has no {}", init)
        } else if buffer != 0 && buffer16 == 0 {
            "its buffer has no selector".to_string()
        } else {
            // UTInit(callback, buffer), PASCAL: the buffer at the top of
            // the stack, and no callback, as Windows 98 has none.
            let mut args = buffer16.to_le_bytes().to_vec();
            args.extend_from_slice(&0u32.to_le_bytes());
            let call = Request {
                target: init16,
                args,
                ..request
            };
            flat::start_call(cpu, c, call, *ctx);
            return None;
        }
    };
    unregistered(cpu, pid, &request, &why);
    flat::done(cpu, c, 0);
    Some(ctx.gpr[EAX])
}

/// UTInit of the client `c`'s UTRegister `request` returned `value`: the
/// thunk is made, or with 0 the registration fails.
pub(super) fn initialized(cpu: &mut Cpu, c: usize, request: Request, value: u32) {
    let pid = cpu.bus.win32.win16.clients[c].pid;
    let registered = if value == 0 {
        unregistered(cpu, pid, &request, "its UTInit returned 0");
        false
    } else {
        register(cpu, pid, &request)
    };
    flat::finish(cpu, request.tid, registered as u32, |_| ());
}

/// Make the thunk of the process `pid`'s UTRegister `request`, whose
/// 16-bit DLL is loaded and initialized: its code in the system heap, its
/// address where the program asked. False, the 16-bit DLL let go, when
/// the heap is full.
fn register(cpu: &mut Cpu, pid: u32, request: &Request) -> bool {
    let module16 = request.module;
    let proc16 = entry(cpu, module16, &request.names[4]);
    let glue = crate::win32::kernel32::address_of(cpu, "UTGlue32");
    let code = super::with_win16(cpu, pid, |p, bus| {
        let at = p.system_alloc(bus, THUNK_SIZE)?;
        p.memory.write(bus, at, &thunk_code(at, proc16, glue))?;
        Some(at)
    })
    .flatten();
    let Some(code) = code else {
        unregistered(cpu, pid, request, "there is no room for its thunk");
        return false;
    };
    if request.data32 != 0 {
        flat::write(cpu, pid, request.data32, &code.to_le_bytes());
    }
    let module32 = request.held.first().copied().unwrap_or(0);
    cpu.bus.win32.win16.universal.thunks.push(Thunk {
        pid,
        module32,
        module16,
        code,
    });
    cpu.bus.log_string(&format!(
        "[W16] UTRegister: {} of {}, its thunk at {:08X}",
        request.names[4], request.names[0], code
    ));
    true
}

/// The UTRegister `request` of the process `pid` fails for `why`, and
/// the 16-bit DLL it loaded is let go.
fn unregistered(cpu: &mut Cpu, pid: u32, request: &Request, why: &str) {
    cpu.bus.log_string(&format!(
        "[W16] UTRegister with {}: {}",
        request.names[0], why
    ));
    flat::release_module(cpu, pid, request.module);
}

/// A thunk's code at `at`, as Windows 98 writes it: its return address
/// popped, UTProc's address `proc16` pushed under it, and on to `glue`.
fn thunk_code(at: u32, proc16: u32, glue: u32) -> Vec<u8> {
    let mut code = vec![0x5A, 0x68];
    code.extend_from_slice(&proc16.to_le_bytes());
    code.extend_from_slice(&[0x52, 0xE9]);
    let next = at.wrapping_add(THUNK_SIZE);
    code.extend_from_slice(&glue.wrapping_sub(next).to_le_bytes());
    code
}

/// The process `pid` ends: its thunks go, and its selectors, whose
/// aliases flat.rs takes away. With the Win16 process, all of them go.
pub(super) fn process_ending(cpu: &mut Cpu, pid: u32) {
    let all = pid == cpu.bus.win32.win16.pid;
    let universal = &mut cpu.bus.win32.win16.universal;
    universal.thunks.retain(|t| !all && t.pid != pid);
    universal.selectors.retain(|s| !all && s.source != pid);
}

// WIN32S16's functions.

/// UTLinearToSelectorOffset for the process `pid`: a 16:16 pointer to
/// `lin` of its memory, 0 for 0 or any address below 64 KB. Its selector
/// is the one of the 32 KB block `lin` is in, made the first time, for
/// 64 KB from the block's start.
pub(super) fn linear_to_segptr(cpu: &mut Cpu, pid: u32, lin: u32) -> u32 {
    let base = lin & !(BLOCK - 1);
    let cached = cpu
        .bus
        .win32
        .win16
        .universal
        .selectors
        .iter()
        .find(|s| s.source == pid && s.base == base)
        .map(|s| s.selector);
    let selector = match cached {
        Some(selector) => {
            flat::refresh(cpu, selector);
            selector
        }
        None => {
            let selector = (flat::map_ls_from(cpu, pid, base) >> 16) as u16;
            if selector == 0 {
                return 0;
            }
            cpu.bus.win32.win16.universal.selectors.push(Selector {
                source: pid,
                base,
                selector,
            });
            selector
        }
    };
    (selector as u32) << 16 | (lin - base)
}

/// UTSelectorOffsetToLinear(pointer): the flat address of a 16:16
/// pointer, as MapSL gives it to the process the 16-bit code runs for; 0
/// for a pointer whose selector is 0.
fn selector_offset_to_linear(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if args[0] >> 16 == 0 {
        return Some(0);
    }
    let pid = flat::served_pid(cpu);
    Some(flat::map_sl_for(cpu, pid, args[0]))
}

/// UTLinearToSelectorOffset(address): a 16:16 pointer to the flat
/// address of the process the 16-bit code runs for.
fn linear_to_selector_offset(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let pid = flat::served_pid(cpu);
    Some(linear_to_segptr(cpu, pid, args[0]))
}

/// Windows 98's WIN32S16.DLL, the 16-bit half of the Universal Thunks.
pub static WIN32S16: Def = Def {
    name: "WIN32S16",
    file: "WIN32S16.DLL",
    apis: &[
        rust(
            "UTSelectorOffsetToLinear",
            45,
            "l",
            selector_offset_to_linear,
        ),
        rust(
            "UTLinearToSelectorOffset",
            46,
            "l",
            linear_to_selector_offset,
        ),
    ],
    library: true,
};

crate::state_fields!(Universal { thunks, selectors });
crate::state_fields!(Thunk {
    pid,
    module32,
    module16,
    code
});
crate::state_fields!(Selector {
    source,
    base,
    selector
});
