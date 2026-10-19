//! Threads: saved processor contexts, switched on the one host thread.
//! The running thread's context is in the processor; the others' are in
//! their entries. A thread that waits (`block`) is switched out at once;
//! the others share the processor in turns of `SLICE`, switched when the
//! timer's interrupt comes (`after_interrupt`), since programs spin on
//! flags that another thread sets. When no thread can run, the processor
//! runs the idle loop, a trap in KERNEL32 that lets time pass to the next
//! interrupt.
//!
//! Waits are kept with the threads (`Wait`). Whatever may end one (an
//! object signaled, a thread ended, time passing) calls `wake`, which
//! ends the waits it can, in the threads' order, taking what they waited
//! for (an auto-reset event's signal, a semaphore's count, a mutex).

use super::call::{self, THREAD_ATTACHED};
use super::object::Kind;
use super::{Context, ESP, process};
use crate::cpu::Cpu;
use crate::instructions::fpu::control;

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
}

#[derive(Clone, Debug, Default)]
pub struct Thread {
    pub id: u32,
    /// Its object, which it holds while it runs.
    pub object: u32,
    pub tib: u32,
    /// Its TIB's selector, which FS has.
    pub selector: u16,
    /// Its stack: the lowest address of its reserve, and its top.
    pub stack: (u32, u32),
    /// Its context while it doesn't run.
    pub ctx: Context,
    /// Its FPU's state while it doesn't run, as FSAVE stores it; empty for
    /// a thread that hasn't run yet.
    pub fpu: Vec<u8>,
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
}

/// The id of the thread at `index`: as on Windows 95, a large number,
/// which programs only compare.
pub fn id_of(index: usize) -> u32 {
    0xFFF5_0000 + (index as u32 + 1) * 0x40
}

/// Keep the running thread's state in its entry: the context `ctx` (the
/// processor's, or the one an interrupt left), the FPU's, its last error.
fn save(cpu: &mut Cpu, ctx: Context) {
    let fpu = control::save_image(cpu).to_vec();
    let process = process(cpu);
    let last_error = process.last_error;
    if let Some(thread) = process.current_thread_mut() {
        thread.ctx = ctx;
        thread.fpu = fpu;
        thread.last_error = last_error;
    }
}

/// Run the thread at `index`, from its saved state.
fn enter(cpu: &mut Cpu, index: usize) {
    let now = cpu.bus.clock.now_micros();
    let process = process(cpu);
    process.current = Some(index);
    process.slice_start = now;
    let thread = &process.threads[index];
    let (ctx, fpu, last_error, tib) = (
        thread.ctx,
        thread.fpu.clone(),
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
    super::system::enter_user(cpu, &ctx);
}

/// The next thread after `after` in turn that can run, if one can.
fn next_ready(process: &super::Process, after: Option<usize>) -> Option<usize> {
    let n = process.threads.len();
    let start = after.map_or(0, |i| i + 1);
    (0..n)
        .map(|k| (start + k) % n)
        .find(|&i| process.threads[i].ready())
}

/// The running thread doesn't go on for now (it waits, was suspended or
/// ended, or gives up its turn): the next one that can run does, or else
/// the idle loop.
pub fn reschedule(cpu: &mut Cpu) {
    let ctx = Context::of(cpu);
    save(cpu, ctx);
    let process = process(cpu);
    match next_ready(process, process.current) {
        Some(index) => enter(cpu, index),
        None => idle(cpu),
    }
}

/// Run the idle loop: no thread runs.
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
    match next_ready(process(cpu), None) {
        Some(index) => enter(cpu, index),
        None => cpu.idle = true,
    }
}

/// An interrupt's handler is done, and the program goes on at `ctx`:
/// waits that time passing ended end, and the running thread gives way to
/// another that is ready once its turn is over.
pub fn after_interrupt(cpu: &mut Cpu, ctx: Context) {
    wake(cpu);
    let now = cpu.bus.clock.now_micros();
    let process = process(cpu);
    match process.current {
        None => match next_ready(process, None) {
            Some(index) => enter(cpu, index),
            None => super::system::enter_user(cpu, &ctx),
        },
        Some(current) => {
            if now.saturating_sub(process.slice_start) >= SLICE {
                if let Some(next) = next_ready(process, Some(current)).filter(|&n| n != current) {
                    save(cpu, ctx);
                    enter(cpu, next);
                    return;
                }
                process.slice_start = now;
            }
            super::system::enter_user(cpu, &ctx);
        }
    }
}

/// Whether the object `id` is signaled for the thread `tid`.
fn signaled(process: &super::Process, id: u32, tid: u32) -> bool {
    match process.objects.get(id).map(|o| &o.kind) {
        Some(Kind::Event { signaled, .. }) => *signaled,
        Some(Kind::Mutex { owner, .. }) => *owner == 0 || *owner == tid,
        Some(Kind::Semaphore { count, .. }) => *count > 0,
        Some(Kind::Thread { index }) => process
            .threads
            .get(*index as usize)
            .is_none_or(|t| t.exit_code.is_some()),
        Some(Kind::Dos { .. }) => true,
        Some(Kind::Process {}) | None => false,
    }
}

/// Take what a wait of the thread `tid` was satisfied by: an auto-reset
/// event's signal, a semaphore's count, a mutex. Whether a mutex was
/// abandoned.
fn take(process: &mut super::Process, id: u32, tid: u32) -> bool {
    let Some(object) = process.objects.get_mut(id) else {
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

/// What a wait of the thread at `index` for `wait` returns if it ends
/// now (having taken what it waited for), or None if it goes on.
fn try_wait(process: &mut super::Process, index: usize, wait: &Wait) -> Option<u32> {
    let tid = process.threads[index].id;
    if wait.section != 0 {
        return None;
    }
    if wait.all {
        if wait.objects.is_empty() || !wait.objects.iter().all(|&o| signaled(process, o, tid)) {
            return None;
        }
        let abandoned = wait
            .objects
            .iter()
            .fold(false, |a, &o| take(process, o, tid) | a);
        return Some(if abandoned {
            WAIT_ABANDONED_0
        } else {
            WAIT_OBJECT_0
        });
    }
    let i = wait
        .objects
        .iter()
        .position(|&o| signaled(process, o, tid))?;
    let abandoned = take(process, wait.objects[i], tid);
    Some(i as u32 + if abandoned {
        WAIT_ABANDONED_0
    } else {
        WAIT_OBJECT_0
    })
}

/// What a wait for `wait` returns at once, if it doesn't have to wait,
/// for the running thread.
pub fn wait_now(cpu: &mut Cpu, wait: &Wait) -> Option<u32> {
    let process = process(cpu);
    let index = process.current?;
    try_wait(process, index, wait)
}

/// The running thread waits for `wait`, and another runs meanwhile. What
/// the wait returns is in EAX when it goes on.
pub fn block(cpu: &mut Cpu, wait: Wait) {
    let process = process(cpu);
    for &id in &wait.objects {
        process.objects.hold(id);
    }
    if let Some(thread) = process.current_thread_mut() {
        thread.wait = Some(wait);
    }
    reschedule(cpu);
}

/// End the wait of the thread at `index`, which returns `result`.
pub fn end_wait(process: &mut super::Process, index: usize, result: u32) {
    let thread = &mut process.threads[index];
    let Some(wait) = thread.wait.take() else {
        return;
    };
    thread.ctx.gpr[EAX] = result;
    for id in wait.objects {
        process.objects.release(id);
    }
}

/// End the waits that can end now.
pub fn wake(cpu: &mut Cpu) {
    let now = cpu.bus.clock.now_micros();
    let process = process(cpu);
    for index in 0..process.threads.len() {
        let Some(wait) = process.threads[index].wait.clone() else {
            continue;
        };
        if process.current == Some(index) {
            continue;
        }
        let result = try_wait(process, index, &wait).or_else(|| {
            wait.deadline
                .filter(|&d| now >= d)
                .map(|_| wait.timeout_result)
        });
        if let Some(result) = result {
            end_wait(process, index, result);
        }
    }
}

/// The running thread's TIB selector for the thread at `index`.
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

/// End the running thread with exit code `code`. The last thread to end
/// ends the process.
pub fn exit_current(cpu: &mut Cpu, code: u32) {
    let process = process(cpu);
    let Some(index) = process.current else {
        return;
    };
    let others = process
        .threads
        .iter()
        .enumerate()
        .any(|(i, t)| i != index && t.exit_code.is_none());
    if !others {
        super::exit(cpu, code);
        return;
    }
    end(process, index, code);
    wake(cpu);
    reschedule(cpu);
}

/// The thread at `index` ends with `code`: its mutexes are abandoned, and
/// it lets go of its object.
pub fn end(process: &mut super::Process, index: usize, code: u32) {
    let thread = &mut process.threads[index];
    if thread.exit_code.is_some() {
        return;
    }
    thread.exit_code = Some(code);
    thread.calls.clear();
    let (tid, object) = (thread.id, thread.object);
    if let Some(wait) = thread.wait.take() {
        for id in wait.objects {
            process.objects.release(id);
        }
    }
    for object in process.objects.objects.iter_mut().flatten() {
        if let Kind::Mutex {
            owner,
            count,
            abandoned,
        } = &mut object.kind
            && *owner == tid
        {
            *owner = 0;
            *count = 0;
            *abandoned = true;
        }
    }
    process.objects.release(object);
}

crate::state_fields!(Wait {
    objects,
    all,
    deadline,
    timeout_result,
    section
});
crate::state_fields!(Thread {
    id,
    object,
    tib,
    selector,
    stack,
    ctx,
    fpu,
    last_error,
    suspended,
    priority,
    exit_code,
    wait,
    calls,
    fault
});
