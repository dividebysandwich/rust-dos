//! Threads and the process: making and ending threads, their state, and
//! the handles of objects.

use super::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, TRUE, set_error,
};
use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::instructions::fpu::control;
use crate::win32::loader::{TLS_ARRAY, TLS_SLOTS};
use crate::win32::memory::{GRANULARITY, PAGE, PRIVATE, SHARED, SYSTEM, USER, WRITABLE};
use crate::win32::object::{self, CURRENT_PROCESS, Kind};
use crate::win32::seh::{self, CONTEXT_SIZE};
use crate::win32::thread::{self, STACK_FLOOR, Thread};
use crate::win32::{Context, Process, process, with_process};

const STILL_ACTIVE: u32 = 0x103;
const CREATE_SUSPENDED: u32 = 4;
const DUPLICATE_CLOSE_SOURCE: u32 = 1;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;

/// The most of a thread's stack committed when it starts; the rest is
/// committed as it grows.
const STACK_COMMIT: u32 = 0x1_0000;

/// The TIB's fields.
const TIB_EXCEPTION_LIST: u32 = 0x00;
const TIB_STACK_TOP: u32 = 0x04;
const TIB_STACK_LIMIT: u32 = 0x08;
const TIB_SELF: u32 = 0x18;
const TIB_TLS_ARRAY: u32 = 0x2C;

/// The thread a handle names (its process's place and its own), or None
/// with the last error set. A thread that ended with its process is
/// gone.
pub(super) fn thread_of(cpu: &mut Cpu, handle: u32) -> Option<(usize, usize)> {
    let found = match object::kind_of(cpu, handle) {
        Some(Kind::Thread { id, .. }) => thread::find(&cpu.bus.win32, id),
        _ => None,
    };
    if found.is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
    }
    found
}

/// The thread `(p, t)`.
fn thread_at(cpu: &mut Cpu, (p, t): (usize, usize)) -> &mut Thread {
    &mut cpu.bus.win32.processes[p].threads[t]
}

/// Whether `(p, t)` is the running thread.
fn is_running(cpu: &Cpu, (p, t): (usize, usize)) -> bool {
    let win32 = &cpu.bus.win32;
    win32.running == p && win32.processes[p].current == Some(t)
}

/// The exit code of the thread or process a handle names: None if it
/// names neither.
fn exit_code_of(cpu: &mut Cpu, handle: u32, process: bool) -> Option<Option<u32>> {
    match object::kind_of(cpu, handle)? {
        Kind::Thread { exit_code, .. } if !process => Some(exit_code),
        Kind::Process { exit_code, .. } if process => Some(exit_code),
        _ => None,
    }
}

/// The place of the process a handle names, if it runs.
pub(super) fn process_of(cpu: &mut Cpu, handle: u32) -> Option<usize> {
    let Some(Kind::Process { id, .. }) = object::kind_of(cpu, handle) else {
        return None;
    };
    cpu.bus.win32.processes.iter().position(|p| p.id == id)
}

/// CreateThread(attributes, stack, start, parameter, flags, id): a thread
/// with a stack of its own, its TIB and TLS, which starts at `start` with
/// the parameter, suspended with CREATE_SUSPENDED.
pub(super) fn create_thread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[_, stack_size, start, param, flags, id_at] = args else {
        unreachable!()
    };
    // The new thread takes the place of one that ended, and its TIB's
    // selector, giving back what the ended one had. The first thread's
    // place stays, as the process's TIB is its.
    let ended = process(cpu)
        .threads
        .iter()
        .skip(1)
        .position(|t| t.exit_code.is_some())
        .map(|i| i + 1);
    let index = ended.unwrap_or(process(cpu).threads.len());
    if index >= thread::MAX_THREADS {
        set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
        return Some(0);
    }
    let selector = thread::selector_of(index);
    let reserve = stack_size
        .max(process(cpu).stack_reserve)
        .clamp(GRANULARITY, 0x100_0000)
        .next_multiple_of(GRANULARITY);
    let made = with_process(cpu, |p, bus| {
        if let Some(t) = ended {
            release(bus, p, t);
        }
        let memory = &mut p.memory;
        let stack = memory.reserve(None, STACK_FLOOR + reserve, PRIVATE, SHARED)?;
        let top = stack + STACK_FLOOR + reserve;
        let commit = reserve.min(STACK_COMMIT);
        memory.commit(bus, top - commit, commit, USER | WRITABLE)?;
        let tib = memory.reserve(None, GRANULARITY, SHARED, SYSTEM)?;
        memory.commit(bus, tib, PAGE, USER | WRITABLE)?;
        for (field, value) in [
            (TIB_EXCEPTION_LIST, 0xFFFF_FFFF),
            (TIB_STACK_TOP, top),
            (TIB_STACK_LIMIT, top - commit),
            (TIB_SELF, tib),
            (TIB_TLS_ARRAY, tib + TLS_ARRAY),
            (
                crate::win32::loader::TIB_PROCESS,
                crate::win32::loader::process_block(p.command_line),
            ),
        ] {
            memory.write_32(bus, tib + field, value)?;
        }
        // Its copy of the program's static TLS, in slot 0.
        if !p.tls_template.is_empty() {
            let len = (p.tls_template.len() as u32).next_multiple_of(PAGE);
            let block = memory.reserve(None, len, PRIVATE, SHARED)?;
            memory.commit(bus, block, len, USER | WRITABLE)?;
            memory.write(bus, block, &p.tls_template)?;
            memory.write_32(bus, tib + TLS_ARRAY, block)?;
        }
        // And of its DLLs'.
        crate::win32::loader::tls_blocks(bus, p, tib)?;
        let memory = &mut p.memory;
        // The return address that ends it, and its argument.
        let esp = top - 8;
        memory.write_32(bus, esp, super::thread_return_address())?;
        memory.write_32(bus, esp + 4, param)?;
        Some((stack, top, tib, esp))
    });
    if ended.is_some() {
        cpu.tlb.flush();
    }
    let Some((stack, top, tib, esp)) = made else {
        set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
        return Some(0);
    };
    let system = process(cpu).system;
    crate::win32::system::set_tib(cpu, system, selector, tib);
    let id = cpu.bus.win32.new_id();
    let objects = object::objects(cpu);
    let object = objects.add(
        Kind::Thread {
            id,
            exit_code: None,
        },
        None,
    );
    objects.hold(object);
    let handle = object::new_handle(cpu, object);
    let thread = Thread {
        id,
        object,
        tib,
        selector,
        stack: (stack, top),
        ctx: thread::initial_context(index, start, param, esp),
        suspended: (flags & CREATE_SUSPENDED != 0) as u32,
        ..Thread::default()
    };
    let threads = &mut process(cpu).threads;
    match threads.get_mut(index) {
        Some(place) => *place = thread,
        None => threads.push(thread),
    }
    if id_at != 0 {
        cpu.bus.guest_write_32(id_at, id);
    }
    Some(handle)
}

/// Give back what the ended thread `t` of `p` had: its stack, its TIB
/// and its copies of the static TLS. Its place keeps no addresses, so
/// nothing is given back twice.
fn release(bus: &mut Bus, p: &mut Process, t: usize) {
    let thread = &mut p.threads[t];
    let (stack, tib) = (thread.stack.0, thread.tib);
    thread.stack = (0, 0);
    thread.tib = 0;
    if tib != 0 {
        let mut slots: Vec<u32> = p.dll_tls.iter().map(|&(slot, _)| slot).collect();
        if !p.tls_template.is_empty() {
            slots.push(0);
        }
        for slot in slots {
            if let Some(block) = p.memory.read_32(bus, tib + TLS_ARRAY + 4 * slot)
                && block != 0
            {
                p.memory.release_region(bus, block);
            }
        }
        p.memory.release_region(bus, tib);
    }
    if stack != 0 {
        p.memory.release_region(bus, stack);
    }
}

/// ExitThread(code), and where a thread's function returns to: the DLLs
/// learn that the thread ends, then it does.
pub(super) fn exit_thread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    super::module::exit_thread(cpu, args[0]);
    None
}

pub(super) fn thread_return(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let code = cpu.eax();
    super::module::exit_thread(cpu, code);
    None
}

pub(super) fn get_current_thread(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(object::CURRENT_THREAD)
}

pub(super) fn get_current_thread_id(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(process(cpu).current_thread().map_or(0, |t| t.id))
}

pub(super) fn get_current_process(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(CURRENT_PROCESS)
}

/// The process's id: as on Windows 95, a large number.
pub(super) fn get_current_process_id(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(process(cpu).id)
}

/// GetExitCodeThread(handle, code): STILL_ACTIVE while it runs.
pub(super) fn get_exit_code_thread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_exit_code(cpu, args, false)
}

/// GetExitCodeProcess(handle, code): STILL_ACTIVE while it runs.
pub(super) fn get_exit_code_process(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    get_exit_code(cpu, args, true)
}

fn get_exit_code(cpu: &mut Cpu, args: &[u32], process: bool) -> Option<u32> {
    let Some(code) = exit_code_of(cpu, args[0], process) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    cpu.bus
        .guest_write_32(args[1], code.unwrap_or(STILL_ACTIVE));
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(TRUE)
}

/// TerminateThread(handle, code): it ends where it is.
pub(super) fn terminate_thread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some((p, t)) = thread_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    if is_running(cpu, (p, t)) {
        thread::exit_current(cpu, args[1]);
        return None;
    }
    let others = cpu.bus.win32.processes[p]
        .threads
        .iter()
        .enumerate()
        .any(|(i, thread)| i != t && thread.exit_code.is_none());
    if !others {
        if !crate::win32::terminate(cpu, p, args[1]) {
            return None;
        }
        return Some(TRUE);
    }
    thread::end(&mut cpu.bus.win32, p, t, args[1]);
    thread::wake(cpu);
    Some(TRUE)
}

/// TerminateProcess(handle, code): the process ends.
pub(super) fn terminate_process(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(p) = process_of(cpu, args[0]) else {
        // A process that ended already ends again without fuss.
        if exit_code_of(cpu, args[0], true).is_some() {
            set_error(cpu, ERROR_ACCESS_DENIED);
        } else {
            set_error(cpu, ERROR_INVALID_HANDLE);
        }
        return Some(FALSE);
    };
    if !crate::win32::terminate(cpu, p, args[1]) {
        return None;
    }
    Some(TRUE)
}

/// SuspendThread(handle): its count before. A thread that suspends
/// itself stops here; one that ended gives -1 with ERROR_ACCESS_DENIED.
pub(super) fn suspend_thread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(at) = thread_of(cpu, args[0]) else {
        return Some(0xFFFF_FFFF);
    };
    let thread = thread_at(cpu, at);
    if thread.exit_code.is_some() {
        set_error(cpu, ERROR_ACCESS_DENIED);
        return Some(0xFFFF_FFFF);
    }
    let before = thread.suspended;
    thread.suspended += 1;
    if is_running(cpu, at) {
        cpu.set_eax(before);
        thread::reschedule(cpu);
        return None;
    }
    Some(before)
}

/// ResumeThread(handle): its count before. Of a thread that ended,
/// -1, as on Windows 9x.
pub(super) fn resume_thread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(at) = thread_of(cpu, args[0]) else {
        return Some(0xFFFF_FFFF);
    };
    if thread_at(cpu, at).exit_code.is_some() {
        set_error(cpu, ERROR_ACCESS_DENIED);
        return Some(0xFFFF_FFFF);
    }
    let thread = thread_at(cpu, at);
    let before = thread.suspended;
    thread.suspended = before.saturating_sub(1);
    Some(before)
}

pub(super) fn get_thread_priority(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(at) = thread_of(cpu, args[0]) else {
        return Some(0x7FFF_FFFF);
    };
    Some(thread_at(cpu, at).priority as u32)
}

/// SetThreadPriority(handle, priority): kept; threads take turns
/// whatever their priority.
pub(super) fn set_thread_priority(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(at) = thread_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    let priority = args[1] as i32;
    if !(-2..=2).contains(&priority) && priority != -15 && priority != 15 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    thread_at(cpu, at).priority = priority;
    Some(TRUE)
}

/// GetThreadContext(handle, context): the thread's registers, as it was
/// when it last stopped, or for the running thread, as it calls.
pub(super) fn get_thread_context(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(which) = thread_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    let at = args[1];
    let flags = cpu.bus.guest_read_32(at);
    if !cpu.bus.guest_probe(at, CONTEXT_SIZE as usize, true) {
        return None;
    }
    let (ctx, fpu) = if is_running(cpu, which) {
        (Context::of(cpu), control::save_image(cpu))
    } else {
        let thread = thread_at(cpu, which);
        let fpu = <[u8; 108]>::try_from(thread.fpu.as_slice()).unwrap_or([0; 108]);
        (thread.ctx, fpu)
    };
    seh::write_context(cpu, at, &ctx, &fpu, flags);
    Some(TRUE)
}

/// SetThreadContext(handle, context): the parts of the thread's registers
/// the context's flags name. Not for the running thread.
pub(super) fn set_thread_context(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(which) = thread_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    if is_running(cpu, which) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let thread = thread_at(cpu, which);
    let mut ctx = thread.ctx;
    let mut fpu = <[u8; 108]>::try_from(thread.fpu.as_slice()).unwrap_or([0; 108]);
    let had_fpu = !thread.fpu.is_empty();
    seh::read_context(cpu, args[1], &mut ctx, &mut fpu);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let thread = thread_at(cpu, which);
    thread.ctx = ctx;
    if had_fpu || fpu != [0; 108] {
        thread.fpu = fpu.to_vec();
    }
    Some(TRUE)
}

/// CloseHandle(handle).
pub(super) fn close_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let handle = args[0];
    if handle == CURRENT_PROCESS || handle == object::CURRENT_THREAD {
        return Some(TRUE);
    }
    if object::close(cpu, handle) {
        Some(TRUE)
    } else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        Some(FALSE)
    }
}

/// DuplicateHandle(from process, handle, to process, new, access, inherit,
/// options): another handle of the object, in the process `to` names.
/// The pseudo-handles of the process and thread give real ones.
pub(super) fn duplicate_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[from, handle, to, new_at, _, _, options] = args else {
        unreachable!()
    };
    let (Some(source), Some(target)) = (process_of(cpu, from), process_of(cpu, to)) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    let win32 = &cpu.bus.win32;
    let id = if source == win32.running {
        object::resolve(cpu, handle)
    } else {
        win32.processes[source].handles.of(handle)
    };
    let Some(id) = id else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    let win32 = &mut cpu.bus.win32;
    win32.objects.hold(id);
    let new = win32.processes[target].handles.add(id);
    if options & DUPLICATE_CLOSE_SOURCE != 0
        && let Some(id) = win32.processes[source].handles.remove(handle)
    {
        win32.objects.release(id);
    }
    if new_at != 0 {
        cpu.bus.guest_write_32(new_at, new);
    }
    Some(TRUE)
}

/// The TLS slot `index` of every thread set to 0, as TlsAlloc and TlsFree
/// leave it.
pub(super) fn clear_tls_slot(cpu: &mut Cpu, index: u32) {
    if index >= TLS_SLOTS {
        return;
    }
    let tibs: Vec<u32> = process(cpu)
        .threads
        .iter()
        .filter(|t| t.exit_code.is_none())
        .map(|t| t.tib)
        .collect();
    for tib in tibs {
        cpu.bus.guest_write_32(tib + TLS_ARRAY + 4 * index, 0);
    }
}
