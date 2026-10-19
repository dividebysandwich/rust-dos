//! Threads and the process: making and ending threads, their state, and
//! the handles of objects.

use super::{ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, TRUE, set_error};
use crate::cpu::Cpu;
use crate::instructions::fpu::control;
use crate::win32::loader::{TLS_ARRAY, TLS_SLOTS};
use crate::win32::memory::{GRANULARITY, PAGE, PRIVATE, SHARED, SYSTEM, USER, WRITABLE};
use crate::win32::object::{self, CURRENT_PROCESS, Kind};
use crate::win32::seh::{self, CONTEXT_SIZE};
use crate::win32::thread::{self, Thread};
use crate::win32::{Context, process, with_process};

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

/// The thread a handle names (its index), or None with the last error
/// set.
fn thread_of(cpu: &mut Cpu, handle: u32) -> Option<usize> {
    match object::kind_of(cpu, handle) {
        Some(Kind::Thread { index }) => Some(index as usize),
        _ => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            None
        }
    }
}

/// CreateThread(attributes, stack, start, parameter, flags, id): a thread
/// with a stack of its own, its TIB and TLS, which starts at `start` with
/// the parameter, suspended with CREATE_SUSPENDED.
pub(super) fn create_thread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[_, stack_size, start, param, flags, id_at] = args else {
        unreachable!()
    };
    let index = process(cpu).threads.len();
    let selector = thread::selector_of(index);
    let reserve = stack_size
        .max(process(cpu).stack_reserve)
        .clamp(GRANULARITY, 0x100_0000)
        .next_multiple_of(GRANULARITY);
    let made = with_process(cpu, |p, bus| {
        let memory = &mut p.memory;
        let stack = memory.reserve(None, reserve, PRIVATE, SHARED)?;
        let top = stack + reserve;
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
        // The return address that ends it, and its argument.
        let esp = top - 8;
        memory.write_32(bus, esp, super::thread_return_address())?;
        memory.write_32(bus, esp + 4, param)?;
        Some((stack, top, tib, esp))
    });
    let Some((stack, top, tib, esp)) = made else {
        set_error(cpu, ERROR_NOT_ENOUGH_MEMORY);
        return Some(0);
    };
    let system = process(cpu).system;
    crate::win32::system::set_tib(cpu, system, selector, tib);
    let process = process(cpu);
    let object = process.objects.add(
        Kind::Thread {
            index: index as u32,
        },
        None,
    );
    process.objects.hold(object);
    let handle = process.objects.handle(object);
    let id = thread::id_of(index);
    process.threads.push(Thread {
        id,
        object,
        tib,
        selector,
        stack: (stack, top),
        ctx: thread::initial_context(index, start, param, esp),
        suspended: (flags & CREATE_SUSPENDED != 0) as u32,
        ..Thread::default()
    });
    if id_at != 0 {
        cpu.bus.guest_write_32(id_at, id);
    }
    Some(handle)
}

/// ExitThread(code), and where a thread's function returns to.
pub(super) fn exit_thread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    thread::exit_current(cpu, args[0]);
    None
}

pub(super) fn thread_return(cpu: &mut Cpu, _: &[u32]) -> Option<u32> {
    let code = cpu.eax();
    thread::exit_current(cpu, code);
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
pub(super) fn get_current_process_id(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(PROCESS_ID)
}

pub const PROCESS_ID: u32 = 0xFFF4_0F1D;

/// GetExitCodeThread(handle, code): STILL_ACTIVE while it runs.
pub(super) fn get_exit_code_thread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = thread_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    let code = process(cpu).threads[index]
        .exit_code
        .unwrap_or(STILL_ACTIVE);
    cpu.bus.guest_write_32(args[1], code);
    Some(TRUE)
}

/// GetExitCodeProcess(handle, code): the process runs, as it is the only
/// one.
pub(super) fn get_exit_code_process(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !matches!(object::kind_of(cpu, args[0]), Some(Kind::Process {})) {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    cpu.bus.guest_write_32(args[1], STILL_ACTIVE);
    Some(TRUE)
}

/// TerminateThread(handle, code): it ends where it is.
pub(super) fn terminate_thread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = thread_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    let process = process(cpu);
    if process.current == Some(index) {
        thread::exit_current(cpu, args[1]);
        return None;
    }
    let others = process
        .threads
        .iter()
        .enumerate()
        .any(|(i, t)| i != index && t.exit_code.is_none());
    if !others {
        crate::win32::exit(cpu, args[1]);
        return None;
    }
    thread::end(process, index, args[1]);
    thread::wake(cpu);
    Some(TRUE)
}

/// TerminateProcess(handle, code): the process ends.
pub(super) fn terminate_process(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    if !matches!(object::kind_of(cpu, args[0]), Some(Kind::Process {})) {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    crate::win32::exit(cpu, args[1]);
    None
}

/// SuspendThread(handle): its count before. A thread that suspends
/// itself stops here.
pub(super) fn suspend_thread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = thread_of(cpu, args[0]) else {
        return Some(0xFFFF_FFFF);
    };
    let process = process(cpu);
    let thread = &mut process.threads[index];
    let before = thread.suspended;
    if thread.exit_code.is_none() {
        thread.suspended += 1;
    }
    if process.current == Some(index) {
        cpu.set_eax(before);
        thread::reschedule(cpu);
        return None;
    }
    Some(before)
}

/// ResumeThread(handle): its count before.
pub(super) fn resume_thread(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = thread_of(cpu, args[0]) else {
        return Some(0xFFFF_FFFF);
    };
    let thread = &mut process(cpu).threads[index];
    let before = thread.suspended;
    thread.suspended = before.saturating_sub(1);
    Some(before)
}

pub(super) fn get_thread_priority(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = thread_of(cpu, args[0]) else {
        return Some(0x7FFF_FFFF);
    };
    Some(process(cpu).threads[index].priority as u32)
}

/// SetThreadPriority(handle, priority): kept; threads take turns
/// whatever their priority.
pub(super) fn set_thread_priority(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = thread_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    let priority = args[1] as i32;
    if !(-2..=2).contains(&priority) && priority != -15 && priority != 15 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    process(cpu).threads[index].priority = priority;
    Some(TRUE)
}

/// GetThreadContext(handle, context): the thread's registers, as it was
/// when it last stopped, or for the running thread, as it calls.
pub(super) fn get_thread_context(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = thread_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    let at = args[1];
    let flags = cpu.bus.guest_read_32(at);
    if !cpu.bus.guest_probe(at, CONTEXT_SIZE as usize, true) {
        return None;
    }
    let (ctx, fpu) = if process(cpu).current == Some(index) {
        (Context::of(cpu), control::save_image(cpu))
    } else {
        let thread = &process(cpu).threads[index];
        let fpu = <[u8; 108]>::try_from(thread.fpu.as_slice()).unwrap_or([0; 108]);
        (thread.ctx, fpu)
    };
    seh::write_context(cpu, at, &ctx, &fpu, flags);
    Some(TRUE)
}

/// SetThreadContext(handle, context): the parts of the thread's registers
/// the context's flags name. Not for the running thread.
pub(super) fn set_thread_context(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(index) = thread_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    if process(cpu).current == Some(index) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let thread = &process(cpu).threads[index];
    let mut ctx = thread.ctx;
    let mut fpu = <[u8; 108]>::try_from(thread.fpu.as_slice()).unwrap_or([0; 108]);
    let had_fpu = !thread.fpu.is_empty();
    seh::read_context(cpu, args[1], &mut ctx, &mut fpu);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let thread = &mut process(cpu).threads[index];
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
    if process(cpu).objects.close(handle) {
        Some(TRUE)
    } else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        Some(FALSE)
    }
}

/// DuplicateHandle(from process, handle, to process, new, access, inherit,
/// options): another handle of the object, in the one process there is.
/// The pseudo-handles of the process and thread give real ones.
pub(super) fn duplicate_handle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[from, handle, to, new_at, _, _, options] = args else {
        unreachable!()
    };
    let is_process =
        |cpu: &mut Cpu, h: u32| matches!(object::kind_of(cpu, h), Some(Kind::Process {}));
    if !is_process(cpu, from) || !is_process(cpu, to) {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    let Some(id) = object::resolve(cpu, handle) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    let process = process(cpu);
    process.objects.hold(id);
    let new = process.objects.handle(id);
    if options & DUPLICATE_CLOSE_SOURCE != 0 {
        process.objects.close(handle);
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
