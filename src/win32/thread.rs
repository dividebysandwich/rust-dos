//! Threads: saved processor contexts, switched on the one host thread.
//! The running thread's context is in the processor; the others' are in
//! their entries. Threads of all processes take turns: a thread that waits
//! (`block`) is switched out at once, and one that runs gives way to
//! another that is ready after `SLICE`, switched when the timer's
//! interrupt comes (`after_interrupt`), since programs spin on flags that
//! another thread sets. A wait that times out, such as a Sleep, ends when
//! its time comes (`schedule_waits`), and its thread runs at once, as the
//! boost Windows gives a thread whose wait ends lets it: a game that
//! sleeps out the rest of its frame while another of its threads spins
//! gets its next frame on time, not at the next interrupt. When no thread
//! can run, the processor runs the idle loop, a trap in KERNEL32 that
//! lets time pass to the next interrupt or timeout.
//!
//! Waits are kept with the threads (`Wait`). Whatever may end one (an
//! object signaled, a thread ended, time passing) calls `wake`, which
//! ends the waits it can, in the threads' order, taking what they waited
//! for (an auto-reset event's signal, a semaphore's count, a mutex).

use super::call::{self, THREAD_ATTACHED};
use super::memory::{GRANULARITY, Memory, PRIVATE, SHARED, USER, WRITABLE};
use super::object::{Kind, Objects};
use super::{Context, ESP, Win32, process};
use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::instructions::fpu::control;
use crate::instructions::sse::float::MXCSR_DEFAULT;

/// How long a thread runs before another that is ready gets its turn, in
/// microseconds. The timer's interrupt, which switches, comes every 55 ms.
const SLICE: u64 = 20_000;

/// The index of EAX and EBX in `Context::gpr`.
const EAX: usize = 0;
const EBX: usize = 3;

/// What wait functions return.
pub const WAIT_OBJECT_0: u32 = 0;
pub const WAIT_ABANDONED_0: u32 = 0x80;
pub const WAIT_TIMEOUT: u32 = 0x102;

/// DllMain's reason for a thread that starts.
const DLL_THREAD_ATTACH: u32 = 2;

/// The bottom of every thread's stack reservation, which the stack never
/// grows into: Windows 95 and 98 reserve 64 KB more than the stack's
/// size, below it, so VirtualQuery's AllocationBase for an address on
/// the stack is that much lower than the stack can reach. Watcom's C
/// library counts on it: its stack check takes the allocation base plus
/// 76 KB as the lowest the stack may go on Windows 95 and 98, and a
/// program with a 64 KB stack and nothing below it fails at its first
/// checked call with "Stack Overflow!". A page fault in it is a stack
/// overflow.
const STACK_FLOOR: u32 = 0x1_0000;

/// Reserve a thread's stack in `memory`: `reserve` rounded up to 64 KB,
/// on top of `STACK_FLOOR`, and the top of it committed, at most
/// `max_commit` bytes. The base of the reservation and the top (the
/// thread's `stack`), and the lowest committed address (the TIB's stack
/// limit).
pub fn reserve_stack(
    memory: &mut Memory,
    bus: &mut Bus,
    reserve: u32,
    max_commit: u32,
) -> Option<((u32, u32), u32)> {
    let reserve = reserve.next_multiple_of(GRANULARITY);
    let base = memory.reserve(None, STACK_FLOOR + reserve, PRIVATE, SHARED)?;
    let top = base + STACK_FLOOR + reserve;
    let limit = top - reserve.min(max_commit);
    memory.commit(bus, limit, top - limit, USER | WRITABLE)?;
    Some(((base, top), limit))
}

/// What a thread waits for.
#[derive(Clone, Debug, Default)]
pub struct Wait {
    /// The objects, which the wait holds while it lasts.
    pub objects: Vec<u32>,
    /// Whether all of them must be signaled at once, or one does.
    pub all: bool,
    /// When it times out, by the clock's microseconds.
    pub deadline: Option<u64>,
    /// What it returns when it times out: WAIT_TIMEOUT, or 0 for Sleep.
    pub timeout_result: u32,
    /// The address of the critical section it waits to enter, or 0. The
    /// thread that leaves the section hands it over.
    pub section: u32,
    /// What its message queue must have to end it (MsgWaitForMultipleObjects,
    /// GetMessageA): it then returns the number of objects.
    pub queue: Option<super::user32::message::QueueWait>,
}

#[derive(Clone, Debug, Default)]
pub struct Thread {
    pub id: u32,
    /// Its object, which it holds while it runs.
    pub object: u32,
    pub tib: u32,
    /// Its TIB's selector, which FS has.
    pub selector: u16,
    /// Its stack: the lowest address of its reserve, `STACK_FLOOR` below
    /// the lowest it can grow to, and its top.
    pub stack: (u32, u32),
    /// Its context while it doesn't run.
    pub ctx: Context,
    /// Its FPU's state while it doesn't run, as FSAVE stores it; empty for
    /// a thread that hasn't run yet.
    pub fpu: Vec<u8>,
    /// Its XMM registers and MXCSR while it doesn't run, on a processor
    /// with SSE; None for a thread that hasn't run yet.
    pub sse: Option<([u128; 8], u32)>,
    pub last_error: u32,
    /// How many times it is suspended; it runs at 0.
    pub suspended: u32,
    pub priority: i32,
    /// Its exit code, once it ended.
    pub exit_code: Option<u32>,
    pub wait: Option<Wait>,
    /// The calls into the program it is in (`call`).
    pub calls: Vec<call::Call>,
    /// What the fault its exception came from was, for the message if
    /// nothing handles it.
    pub fault: String,
}

impl Thread {
    /// Whether it can run.
    pub fn ready(&self) -> bool {
        self.exit_code.is_none() && self.wait.is_none() && self.suspended == 0
    }

    /// The lowest address its stack can grow down to, `STACK_FLOOR` above
    /// the base of its reservation.
    pub fn stack_bottom(&self) -> u32 {
        self.stack.0 + STACK_FLOOR
    }
}

/// A thread by its process's place and its own in that process's list.
type At = (usize, usize);

/// The running thread, if one runs.
fn current(win32: &Win32) -> Option<At> {
    let process = win32.processes.get(win32.running)?;
    Some((win32.running, process.current?))
}

/// The thread with the id `id`.
pub fn find(win32: &Win32, id: u32) -> Option<At> {
    win32.processes.iter().enumerate().find_map(|(p, process)| {
        let t = process.threads.iter().position(|t| t.id == id)?;
        Some((p, t))
    })
}

/// Keep the running thread's state in its entry: the context `ctx` (the
/// processor's, or the one an interrupt left), the FPU's, its last error.
fn save(cpu: &mut Cpu, ctx: Context) {
    let fpu = control::save_image(cpu).to_vec();
    let sse = cpu.model.has_sse().then_some((cpu.sse.xmm, cpu.sse.mxcsr));
    let win32 = &mut cpu.bus.win32;
    let Some(process) = win32.processes.get_mut(win32.running) else {
        return;
    };
    let last_error = process.last_error;
    if let Some(thread) = process.current_thread_mut() {
        thread.ctx = ctx;
        thread.fpu = fpu;
        thread.sse = sse;
        thread.last_error = last_error;
    }
}

/// Run the thread `at`, from its saved state.
fn enter(cpu: &mut Cpu, (p, t): At) {
    let now = cpu.bus.clock.now_micros();
    let win32 = &mut cpu.bus.win32;
    if let Some(process) = win32.processes.get_mut(win32.running) {
        process.current = None;
    }
    win32.running = p;
    win32.slice_start = now;
    let tid = win32.processes[p].threads[t].id;
    crate::win16::task::entered(win32, tid);
    let process = &mut win32.processes[p];
    process.current = Some(t);
    let thread = &process.threads[t];
    let (ctx, fpu, sse, last_error, tib) = (
        thread.ctx,
        thread.fpu.clone(),
        thread.sse,
        thread.last_error,
        thread.tib,
    );
    process.last_error = last_error;
    process.tib = tib;
    match <&[u8; 108]>::try_from(fpu.as_slice()) {
        Ok(image) => control::load_image(cpu, image),
        Err(_) => {
            // A thread starts with the FPU's precision at 53 bits.
            control::fninit(cpu);
            cpu.fpu_control = 0x027F;
        }
    }
    if cpu.model.has_sse() {
        // A thread starts with its XMM registers clear.
        let (xmm, mxcsr) = sse.unwrap_or(([0; 8], MXCSR_DEFAULT));
        cpu.sse.xmm = xmm;
        cpu.sse.mxcsr = mxcsr;
    }
    super::system::enter_user(cpu, &ctx);
}

/// The next thread after `after` in turn that can run, if one can: the
/// threads of all processes, in order.
fn next_ready(win32: &Win32, after: Option<At>) -> Option<At> {
    let all: Vec<At> = win32
        .processes
        .iter()
        .enumerate()
        .flat_map(|(p, process)| (0..process.threads.len()).map(move |t| (p, t)))
        .collect();
    let start = after
        .and_then(|a| all.iter().position(|&x| x == a))
        .map_or(0, |i| i + 1);
    (0..all.len())
        .map(|k| all[(start + k) % all.len()])
        .find(|&(p, t)| {
            win32.processes[p].threads[t].ready() && crate::win16::task::may_run(win32, p, t)
        })
}

/// The running thread doesn't go on for now (it waits, was suspended or
/// gave up its turn): the next one that can run does, or else the idle
/// loop.
pub fn reschedule(cpu: &mut Cpu) {
    let ctx = Context::of(cpu);
    save(cpu, ctx);
    let win32 = &cpu.bus.win32;
    match next_ready(win32, current(win32)) {
        Some(at) => enter(cpu, at),
        None => idle(cpu),
    }
}

/// Run the first thread that can run, or else the idle loop, with nothing
/// running to save (its process ended).
pub fn run_next(cpu: &mut Cpu) {
    match next_ready(&cpu.bus.win32, None) {
        Some(at) => enter(cpu, at),
        None => idle(cpu),
    }
}

/// Run the idle loop, in the running process: no thread runs.
fn idle(cpu: &mut Cpu) {
    let process = process(cpu);
    process.current = None;
    let mut ctx = Context {
        eip: super::kernel32::idle_address(),
        eflags: 0x0202,
        seg: [
            super::system::DATA3,
            super::system::CODE3,
            super::system::DATA3,
            super::system::DATA3,
            0,
            0,
        ],
        ..Context::default()
    };
    ctx.gpr[ESP] = process.idle_stack;
    super::system::enter_user(cpu, &ctx);
}

/// The idle loop's trap: a thread that can run does, or time passes to
/// the next interrupt.
pub fn idle_trap(cpu: &mut Cpu) {
    wake(cpu);
    match next_ready(&cpu.bus.win32, None) {
        Some(at) => enter(cpu, at),
        None => cpu.idle = true,
    }
}

/// An interrupt's handler is done, and the program goes on at `ctx`:
/// waits that time passing ended end, and the running thread gives way to
/// another that is ready once its turn is over.
pub fn after_interrupt(cpu: &mut Cpu, ctx: Context) {
    wake(cpu);
    let now = cpu.bus.clock.now_micros();
    let win32 = &mut cpu.bus.win32;
    match current(win32) {
        None => match next_ready(win32, None) {
            Some(at) => enter(cpu, at),
            None => super::system::enter_user(cpu, &ctx),
        },
        Some(running) => {
            if now.saturating_sub(win32.slice_start) >= SLICE {
                if let Some(next) = next_ready(win32, Some(running)).filter(|&n| n != running) {
                    save(cpu, ctx);
                    enter(cpu, next);
                    return;
                }
                win32.slice_start = now;
            }
            super::system::enter_user(cpu, &ctx);
        }
    }
}

/// Time a built-in device or a thread's timeout waited for came while the
/// running thread ran (win32::timer): waits that ended end, and a thread
/// that is ready and of a higher priority than the running one, as the
/// multimedia thread is, or one whose wait timed out and of the running
/// one's priority or higher, runs at once instead of at the end of its
/// turn.
pub fn preempt(cpu: &mut Cpu) {
    let timed_out = wake(cpu);
    let win32 = &cpu.bus.win32;
    let Some((p, t)) = current(win32) else {
        return;
    };
    let mine = win32.processes[p].threads[t].priority;
    let higher = win32.processes.iter().enumerate().find_map(|(q, process)| {
        let u = process.threads.iter().enumerate().position(|(u, th)| {
            th.ready() && th.priority > mine && crate::win16::task::may_run(win32, q, u)
        })?;
        Some((q, u))
    });
    // A thread whose wait timed out is boosted over the others of its
    // priority, as Windows boosts a thread whose wait ends.
    let boosted = || {
        timed_out.iter().copied().find(|&(q, u)| {
            let th = &win32.processes[q].threads[u];
            th.ready() && th.priority >= mine && crate::win16::task::may_run(win32, q, u)
        })
    };
    if let Some(next) = higher.or_else(boosted).filter(|&n| n != (p, t)) {
        save(cpu, Context::of(cpu));
        enter(cpu, next);
    }
}

/// Whether the object `id` is signaled for the thread `tid`.
fn signaled(objects: &Objects, id: u32, tid: u32) -> bool {
    match objects.get(id).map(|o| &o.kind) {
        Some(Kind::Event { signaled, .. }) => *signaled,
        Some(Kind::Mutex { owner, .. }) => *owner == 0 || *owner == tid,
        Some(Kind::Semaphore { count, .. }) => *count > 0,
        Some(Kind::Thread { exit_code, .. } | Kind::Process { exit_code, .. }) => {
            exit_code.is_some()
        }
        Some(Kind::Dos { .. } | Kind::File { .. }) => true,
        Some(Kind::Mapping { .. } | Kind::Find { .. }) | None => false,
    }
}

/// Take what a wait of the thread `tid` was satisfied by: an auto-reset
/// event's signal, a semaphore's count, a mutex. Whether a mutex was
/// abandoned.
fn take(objects: &mut Objects, id: u32, tid: u32) -> bool {
    let Some(object) = objects.get_mut(id) else {
        return false;
    };
    match &mut object.kind {
        Kind::Event { manual, signaled } => {
            if !*manual {
                *signaled = false;
            }
            false
        }
        Kind::Mutex {
            owner,
            count,
            abandoned,
        } => {
            *owner = tid;
            *count += 1;
            std::mem::take(abandoned)
        }
        Kind::Semaphore { count, .. } => {
            *count -= 1;
            false
        }
        _ => false,
    }
}

/// What a wait of the thread `tid` for `wait` returns if it ends now
/// (having taken what it waited for), or None if it goes on.
fn try_wait(objects: &mut Objects, tid: u32, wait: &Wait) -> Option<u32> {
    if wait.section != 0 {
        return None;
    }
    if wait.all {
        if wait.objects.is_empty() || !wait.objects.iter().all(|&o| signaled(objects, o, tid)) {
            return None;
        }
        let abandoned = wait
            .objects
            .iter()
            .fold(false, |a, &o| take(objects, o, tid) | a);
        return Some(if abandoned {
            WAIT_ABANDONED_0
        } else {
            WAIT_OBJECT_0
        });
    }
    let i = wait
        .objects
        .iter()
        .position(|&o| signaled(objects, o, tid))?;
    let abandoned = take(objects, wait.objects[i], tid);
    Some(
        i as u32
            + if abandoned {
                WAIT_ABANDONED_0
            } else {
                WAIT_OBJECT_0
            },
    )
}

/// What a wait for `wait` returns at once, if the running thread doesn't
/// have to wait.
pub fn wait_now(cpu: &mut Cpu, wait: &Wait) -> Option<u32> {
    let tid = process(cpu).current_thread()?.id;
    try_wait(&mut cpu.bus.win32.objects, tid, wait)
}

/// The running thread waits for `wait`, and another runs meanwhile. What
/// the wait returns is in EAX when it goes on.
pub fn block(cpu: &mut Cpu, wait: Wait) {
    for &id in &wait.objects {
        cpu.bus.win32.objects.hold(id);
    }
    if let Some(thread) = process(cpu).current_thread_mut() {
        thread.wait = Some(wait);
    }
    schedule_waits(&mut cpu.bus);
    reschedule(cpu);
}

/// When the first of the threads' timed waits that are still to come
/// ends, by the clock's microseconds. A wait whose time has passed but
/// that hasn't ended, while the system runs code of its own, ends at the
/// next interrupt and gets no event of its own.
pub(crate) fn wait_due(win32: &Win32, now: u64) -> Option<u64> {
    win32
        .processes
        .iter()
        .flat_map(|p| &p.threads)
        .filter_map(|t| t.wait.as_ref()?.deadline)
        .filter(|&d| d > now)
        .min()
}

/// Make the end of the first timed wait a timer event
/// (`Win32::next_event`).
pub(crate) fn schedule_waits(bus: &mut Bus) {
    bus.win32.wait_due = wait_due(&bus.win32, bus.clock.now_micros());
    bus.reschedule_timers();
}

/// End the wait of the thread `at`, which returns `result`.
pub fn end_wait(win32: &mut Win32, (p, t): At, result: u32) {
    let thread = &mut win32.processes[p].threads[t];
    let Some(wait) = thread.wait.take() else {
        return;
    };
    thread.ctx.gpr[EAX] = result;
    for id in wait.objects {
        win32.objects.release(id);
    }
}

/// End the waits that can end now. The threads whose waits timed out.
pub fn wake(cpu: &mut Cpu) -> Vec<At> {
    super::user32::input::poll(cpu);
    super::dinput::poll(cpu);
    super::winmm::tick(cpu);
    super::dsound::tick(cpu);
    let now = cpu.bus.clock.now_micros();
    let win32 = &mut cpu.bus.win32;
    let running = current(win32);
    let mut timed_out = Vec::new();
    for p in 0..win32.processes.len() {
        for t in 0..win32.processes[p].threads.len() {
            let thread = &win32.processes[p].threads[t];
            let Some(wait) = thread.wait.clone() else {
                continue;
            };
            if running == Some((p, t)) {
                continue;
            }
            let tid = thread.id;
            let result = try_wait(&mut win32.objects, tid, &wait)
                .or_else(|| {
                    let queue = wait.queue.as_ref()?;
                    super::user32::message::wait_ready(win32, tid, queue, now)
                        .then_some(wait.objects.len() as u32)
                })
                .or_else(|| {
                    wait.deadline.filter(|&d| now >= d)?;
                    timed_out.push((p, t));
                    Some(wait.timeout_result)
                });
            if let Some(result) = result {
                end_wait(win32, (p, t), result);
            }
        }
    }
    timed_out
}

/// How many threads a process's list has room for: one TIB selector for
/// each in the LDT from TIB3 on.
pub const MAX_THREADS: usize =
    ((8 * super::system::LDT_ENTRIES - (super::system::TIB3 & !7) as u32) / 8) as usize;

/// The TIB selector of the thread at `index` in its process's list.
pub fn selector_of(index: usize) -> u16 {
    super::system::TIB3 + 8 * index as u16
}

/// A new thread's start: the DLLs' entry points learn of it, then its
/// function runs, its argument and return address on the stack.
pub fn start(cpu: &mut Cpu) {
    let start = cpu.eax();
    let param = cpu.ebx();
    next_attach(cpu, start, param, 0);
}

/// Tell the DLL `from` on in the list that the thread started, or with
/// none left, run its function.
fn next_attach(cpu: &mut Cpu, start: u32, param: u32, from: usize) {
    let dlls = process(cpu).entry_points();
    match dlls.get(from) {
        Some(&(base, entry)) => {
            let resume = call::here(cpu);
            let data = vec![start, param, from as u32 + 1];
            if !call::call(
                cpu,
                entry,
                &[base, DLL_THREAD_ATTACH, 0],
                THREAD_ATTACHED,
                resume,
                data,
            ) {
                super::fatal(cpu, "a thread's stack isn't there", None);
            }
        }
        None => cpu.set_eip(start),
    }
}

/// A DLL's entry point learned of a new thread.
pub fn attached(cpu: &mut Cpu, _: u32, call: call::Call) {
    let &[start, param, next] = call.data.as_slice() else {
        return;
    };
    next_attach(cpu, start, param, next as usize);
}

/// The context a new thread starts with: at KERNEL32's thread start,
/// with its function in EAX and argument in EBX, as on Windows, and on
/// its stack the return address that ends it and the argument.
pub fn initial_context(index: usize, start: u32, param: u32, esp: u32) -> Context {
    let mut ctx = Context {
        eip: super::kernel32::thread_start_address(),
        eflags: 0x0202,
        seg: [
            super::system::DATA3,
            super::system::CODE3,
            super::system::DATA3,
            super::system::DATA3,
            selector_of(index),
            0,
        ],
        ..Context::default()
    };
    ctx.gpr[EAX] = start;
    ctx.gpr[EBX] = param;
    ctx.gpr[ESP] = esp;
    ctx
}

/// End the running thread with exit code `code`. The last thread of a
/// process to end ends the process.
pub fn exit_current(cpu: &mut Cpu, code: u32) {
    let Some((p, t)) = current(&cpu.bus.win32) else {
        return;
    };
    let others = cpu.bus.win32.processes[p]
        .threads
        .iter()
        .enumerate()
        .any(|(i, thread)| i != t && thread.exit_code.is_none());
    if !others {
        super::exit(cpu, code);
        return;
    }
    end(&mut cpu.bus.win32, p, t, code);
    wake(cpu);
    reschedule(cpu);
}

/// The thread `t` of the process `p` ends with `code`: its object is
/// signaled, its mutexes are abandoned, and it lets go of its object.
pub fn end(win32: &mut Win32, p: usize, t: usize, code: u32) {
    let thread = &mut win32.processes[p].threads[t];
    if thread.exit_code.is_some() {
        return;
    }
    thread.exit_code = Some(code);
    thread.calls.clear();
    let (tid, own) = (thread.id, thread.object);
    let wait = thread.wait.take();
    super::user32::message::thread_ended(win32, tid);
    let objects = &mut win32.objects;
    for id in wait.into_iter().flat_map(|w| w.objects) {
        objects.release(id);
    }
    for object in objects.objects.iter_mut().flatten() {
        match &mut object.kind {
            Kind::Mutex {
                owner,
                count,
                abandoned,
            } if *owner == tid => {
                *owner = 0;
                *count = 0;
                *abandoned = true;
            }
            _ => {}
        }
    }
    if let Some(object) = objects.get_mut(own)
        && let Kind::Thread { exit_code, .. } = &mut object.kind
    {
        *exit_code = Some(code);
    }
    objects.release(own);
}

crate::state_fields!(Wait {
    objects,
    all,
    deadline,
    timeout_result,
    section,
    queue
});
crate::state_fields!(Thread {
    id,
    object,
    tib,
    selector,
    stack,
    ctx,
    fpu,
    sse,
    last_error,
    suspended,
    priority,
    exit_code,
    wait,
    calls,
    fault
});
