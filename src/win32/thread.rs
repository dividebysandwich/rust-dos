//! Threads: saved processor contexts, switched on the one host thread. The
//! running thread's context is in the processor; the others' are in their
//! entries. Threads of all processes take turns: a thread that waits
//! (`block`) is switched out at once, and one that runs gives way to
//! another that is ready when its turn ends, at the fourth of Windows 98's
//! timer ticks after it began (`turn_over`), since programs spin on flags
//! that another thread sets. A wait that times out, such as a Sleep, ends
//! when its time comes (`schedule_waits`), and its thread runs at once, as
//! the boost Windows gives a thread whose wait ends lets it: a game that
//! sleeps out the rest of its frame while another of its threads spins gets
//! its next frame on time, not at the next interrupt. A thread that one of
//! a higher priority took the processor from goes on with the rest of its
//! turn when that one stops or its turn ends (`take_preempted`). When no
//! thread can run, the processor runs the idle loop, a trap in KERNEL32
//! that lets time pass to the next interrupt or timeout.
//!
//! Waits are kept with the threads (`Wait`). An object signaled ends the
//! waits for it at once (`object_changed`): SetEvent, ReleaseMutex and
//! ReleaseSemaphore, and the events the system sets
//! (`winmm::callback::set_event`). Whatever else may end one (a thread
//! ended, time passing, input, a device's work) calls `wake`, which reads
//! the devices and ends every wait that can end. Both end waits in the
//! threads' order, taking what they waited for (an auto-reset event's
//! signal, a semaphore's count, a mutex).

use super::call::{self, THREAD_ATTACHED};
use super::memory::{GRANULARITY, Memory, PRIVATE, SHARED, USER, WRITABLE};
use super::object::{Kind, Objects};
use super::seh;
use super::{Context, EAX, EBX, ESP, Win32, process};
use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::instructions::fpu::control;
use crate::instructions::sse::float::MXCSR_DEFAULT;

/// How long a thread runs before another that is ready gets its turn, in
/// microseconds: four of Windows 98's ticks (`TICK`).
const SLICE: u64 = 20_000;

/// Windows 98's timer tick, in microseconds: its VTD runs the PIT at
/// 200 Hz (a count of 5965), and a thread's turn ends at a tick, the
/// fourth after it began (`turn_end`). On Windows 98 a thread that gives
/// up its turn with Sleep(0) while another spins gets the processor back
/// 5 to 20 ms later, in steps of 5 ms. The built-in Windows leaves the
/// PIT at the BIOS's 18.2 Hz, so a turn's end is an event of its own
/// (`turn_due`). Ending turns at the BIOS's interrupt instead kept such
/// a thread waiting 55 ms: Dark Omen's movie decoder gives up its turn
/// once a picture, and showed two pictures a second of fifteen while its
/// main thread polled DirectInput (#1802).
const TICK: u64 = 5_000;

/// The W32 section's version from which a thread's TIB has its last
/// error at 60h and its TLS slots at 88h, as on Windows 98 SE. Before it
/// the last error was in the thread's entry (the running thread's in its
/// process's), and the slots were at 100h (`OLDER_TLS_ARRAY`).
pub const TIB_98: u16 = 114;
const OLDER_TLS_ARRAY: u32 = 0x100;

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
    /// Its debug registers while it doesn't run, as a CONTEXT has them:
    /// each thread has its own breakpoints.
    pub dr: seh::DebugRegs,
    /// Its last error in a state saved before its TIB held it (`TIB_98`),
    /// which the load puts there.
    pub older_last_error: crate::savestate::Lost<TIB_98, u32>,
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

    /// Its FPU's state while it doesn't run, as FSAVE stores it: for a
    /// thread that hasn't run yet, the state it starts with.
    pub fn fpu_image(&self) -> [u8; 108] {
        <[u8; 108]>::try_from(self.fpu.as_slice()).unwrap_or(INITIAL_FPU)
    }
}

/// The FPU's state a thread starts with, as FSAVE stores it: FNINIT's,
/// with the precision at 53 bits (control word 27Fh), the status word 0
/// and every register empty (tag word FFFFh).
pub const INITIAL_FPU: [u8; 108] = {
    let mut image = [0u8; 108];
    let words = [0xFFFF_027Fu32, 0xFFFF_0000, 0xFFFF_FFFF];
    let mut i = 0;
    while i < words.len() {
        let bytes = words[i].to_le_bytes();
        let mut b = 0;
        while b < 4 {
            image[4 * i + b] = bytes[b];
            b += 1;
        }
        i += 1;
    }
    image
};

/// A thread by its process's place and its own in that process's list.
type At = (usize, usize);

/// The emulated time and the instructions each thread has had
/// (`Win32::times`), for the debug server: nothing the machine does reads
/// them.
#[derive(Clone, Debug, Default)]
pub struct Times {
    /// The clock's microseconds and `Cpu::executed` when the running
    /// thread's (or the idle loop's) share began; None until the first
    /// thread runs, and again after a load.
    mark: Option<(u64, u64)>,
    /// Microseconds and instructions by thread id, 0 for the idle loop.
    by_thread: std::collections::BTreeMap<u32, (u64, u64)>,
}

/// Charge the time and instructions since the mark to the running thread,
/// or to the idle loop when none can run, and start a new mark. It runs
/// at every switch (`run`, `idle`) and as a process ends (`end_process`),
/// so a thread's share is charged while it is still the running one.
/// Time with no Win32 program running (DOS before and after it) goes to
/// nobody: the last process's end drops the mark. The instructions are
/// `Cpu::executed`'s, so a busy-wait loop the idle skip skips counts its
/// skipped passes too.
pub(super) fn charge(cpu: &mut Cpu) {
    let now = (cpu.bus.clock.now_micros(), cpu.executed);
    let win32 = &mut cpu.bus.win32;
    let mark = if win32.active() { Some(now) } else { None };
    if let Some((t0, i0)) = std::mem::replace(&mut win32.times.mark, mark)
        && mark.is_some()
    {
        let id = current(win32).map_or(0, |(p, t)| win32.processes[p].threads[t].id);
        let e = win32.times.by_thread.entry(id).or_default();
        e.0 += now.0.saturating_sub(t0);
        e.1 += now.1.saturating_sub(i0);
    }
}

/// The emulated microseconds and instructions each thread has had since
/// the first ran or the last load, by thread id, 0 for the idle loop:
/// `thread_times` in the debug server's `/api/win32`.
pub fn times(cpu: &mut Cpu) -> Vec<(u32, u64, u64)> {
    charge(cpu);
    let t = &cpu.bus.win32.times.by_thread;
    t.iter().map(|(&id, &(us, n))| (id, us, n)).collect()
}

/// The running thread, if one runs.
fn current(win32: &Win32) -> Option<At> {
    let process = win32.processes.get(win32.running)?;
    Some((win32.running, process.current?))
}

/// Whether a thread runs and another can take the processor from it when
/// its turn ends: the idle skip then skips a busy-wait loop of the
/// running one only as far as Windows would have spun it, to the end of
/// its turn (`Clock::handed_over`).
pub fn others_can_run(win32: &Win32) -> bool {
    let Some(running) = current(win32) else {
        return false;
    };
    next_where(win32, Some(running), |at| {
        at != running && can_run(win32, at)
    })
    .is_some()
}

/// The thread with the id `id`.
pub fn find(win32: &Win32, id: u32) -> Option<At> {
    win32.processes.iter().enumerate().find_map(|(p, process)| {
        let t = process.threads.iter().position(|t| t.id == id)?;
        Some((p, t))
    })
}

/// Keep the running thread's state in its entry: the context `ctx` (the
/// processor's, or the one an interrupt left) and the FPU's. Its last
/// error stays in its TIB.
fn save(cpu: &mut Cpu, ctx: Context) {
    let fpu = control::save_image(cpu).to_vec();
    let sse = cpu.model.has_sse().then_some((cpu.sse.xmm, cpu.sse.mxcsr));
    let dr = seh::debug_registers(cpu);
    let win32 = &mut cpu.bus.win32;
    let Some(process) = win32.processes.get_mut(win32.running) else {
        return;
    };
    if let Some(thread) = process.current_thread_mut() {
        thread.ctx = ctx;
        thread.fpu = fpu;
        thread.sse = sse;
        thread.dr = dr;
    }
}

/// Run the thread `at`, from its saved state, with a new turn.
fn enter(cpu: &mut Cpu, at: At) {
    run(cpu, at, 0);
}

/// Run the thread `at`, from its saved state, with a turn of which it
/// has used `used` microseconds: a preempted thread goes on with the rest
/// of its turn.
fn run(cpu: &mut Cpu, (p, t): At, used: u64) {
    charge(cpu);
    let now = cpu.bus.clock.now_micros();
    let win32 = &mut cpu.bus.win32;
    if let Some(process) = win32.processes.get_mut(win32.running) {
        process.current = None;
    }
    win32.running = p;
    win32.slice_start = now.saturating_sub(used);
    let tid = win32.processes[p].threads[t].id;
    win32.preempted.retain(|&(id, _)| id != tid);
    crate::win16::task::entered(win32, tid);
    let owner = super::directories::owner_of(win32, p, t);
    let process = &mut win32.processes[p];
    process.current = Some(t);
    let thread = &process.threads[t];
    let (ctx, fpu, sse, dr, tib) = (
        thread.ctx,
        thread.fpu_image(),
        thread.sse,
        thread.dr,
        thread.tib,
    );
    process.tib = tib;
    control::load_image(cpu, &fpu);
    if cpu.model.has_sse() {
        // A thread starts with its XMM registers clear.
        let (xmm, mxcsr) = sse.unwrap_or(([0; 8], MXCSR_DEFAULT));
        cpu.sse.xmm = xmm;
        cpu.sse.mxcsr = mxcsr;
    }
    seh::set_debug_registers(cpu, &dr);
    super::directories::switch(cpu, owner);
    super::system::enter_user(cpu, &ctx);
    arm_turn(cpu);
}

/// When the turn that began at `start` ends, by the clock's microseconds:
/// at the fourth tick after it began, `SLICE` after the one before it (or
/// at its start).
fn turn_end(start: u64) -> u64 {
    start - start % TICK + SLICE
}

/// The start of the running thread's turn at `now`, of a turn that began
/// at `start`: one that ended with no other thread ready to take over was
/// followed by another from its end, as Windows gives the thread a new
/// turn then.
fn renewed(start: u64, now: u64) -> u64 {
    let end = turn_end(start);
    if now < end {
        start
    } else {
        end + (now - end) / SLICE * SLICE
    }
}

// A renewed turn starts at a tick, so it ends `SLICE` later.
const _: () = assert!(SLICE.is_multiple_of(TICK));

/// When the running thread's turn ends (`turn_end` of its renewed turn at
/// `now`), for the timer event that ends it (`Win32::next_event`). None
/// while no thread runs or every other thread has ended: no other could
/// take the processor, and a program with one thread gets no event of
/// its own. It depends only on what a save state keeps and on `now`, so
/// a loaded state works it out again (`Bus::after_reload`).
pub(crate) fn turn_due(win32: &Win32, now: u64) -> Option<u64> {
    let running = current(win32)?;
    let others = win32.processes.iter().enumerate().any(|(p, process)| {
        let mut threads = process.threads.iter().enumerate();
        threads.any(|(t, thread)| (p, t) != running && thread.exit_code.is_none())
    });
    others.then(|| turn_end(renewed(win32.slice_start, now)))
}

/// Make the end of the running thread's turn a timer event (`turn_due`):
/// after a thread runs, its turn was renewed, or another thread or
/// process was made.
pub fn arm_turn(cpu: &mut Cpu) {
    let due = turn_due(&cpu.bus.win32, cpu.bus.clock.now_micros());
    if due != cpu.bus.win32.turn_due {
        cpu.bus.win32.turn_due = due;
        cpu.bus.reschedule_timers();
    }
}

/// The running thread's turn ended (`turn_due`) while it ran the program's
/// code (`in_program`): the thread a higher one preempted goes on, or else
/// the next one in turn that can run, or with none the running thread
/// starts a new turn. In the system's code or real-mode code it can't be
/// switched, and changes nothing: its turn ends at the next interrupt
/// back in the program (`after_interrupt`) or the next event. A thread
/// that ended since the event was armed leaves the event behind, which
/// changes nothing either, as a loaded state has no such event.
pub fn turn_over(cpu: &mut Cpu, in_program: bool) {
    let now = cpu.bus.clock.now_micros();
    if turn_due(&cpu.bus.win32, now).is_none() {
        cpu.bus.win32.turn_due = None;
        return;
    }
    let win32 = &cpu.bus.win32;
    if let Some(running) = current(win32).filter(|_| in_program)
        && now >= turn_end(win32.slice_start)
    {
        if next_turn(cpu, running, Context::of(cpu)) {
            return;
        }
        let win32 = &mut cpu.bus.win32;
        win32.slice_start = renewed(win32.slice_start, now);
    }
    arm_turn(cpu);
}

/// The running thread `running`, with the context `ctx`, has had its
/// turn: the thread a higher one preempted goes on, or else the next one
/// in turn that can run, and true; false when none can.
fn next_turn(cpu: &mut Cpu, running: At, ctx: Context) -> bool {
    let win32 = &mut cpu.bus.win32;
    // A thread preempted while another ran is at the head of the queue:
    // it goes on before the next one in turn.
    if let Some((at, used)) = take_preempted(win32, |_, _| true) {
        save(cpu, ctx);
        run(cpu, at, used);
        return true;
    }
    if let Some(next) = next_ready(win32, Some(running)).filter(|&n| n != running) {
        save(cpu, ctx);
        enter(cpu, next);
        return true;
    }
    false
}

/// The next thread after `after` in turn that can run, if one can: the
/// threads of all processes, in order.
fn next_ready(win32: &Win32, after: Option<At>) -> Option<At> {
    next_where(win32, after, |at| can_run(win32, at))
}

/// Whether the thread `at` can run: it is ready, and the Win16 lock lets
/// it.
fn can_run(win32: &Win32, (p, t): At) -> bool {
    win32.processes[p].threads[t].ready() && crate::win16::task::may_run(win32, p, t)
}

/// The next thread after `after` in turn for which `pick` holds. Sleep(0)
/// asks for it at every call, so the threads aren't collected first.
fn next_where(win32: &Win32, after: Option<At>, pick: impl Fn(At) -> bool) -> Option<At> {
    let all = || {
        win32
            .processes
            .iter()
            .enumerate()
            .flat_map(|(p, process)| (0..process.threads.len()).map(move |t| (p, t)))
    };
    let start = after
        .and_then(|a| all().position(|x| x == a))
        .map_or(0, |i| i + 1);
    all().skip(start).chain(all().take(start)).find(|&at| pick(at))
}

/// The running thread doesn't go on for now (it waits, was suspended or
/// gave up its turn): the next one that can run does, or else the idle
/// loop.
pub fn reschedule(cpu: &mut Cpu) {
    let ctx = Context::of(cpu);
    save(cpu, ctx);
    if let Some((at, used)) = take_preempted(&mut cpu.bus.win32, |_, _| true) {
        run(cpu, at, used);
        return;
    }
    let win32 = &cpu.bus.win32;
    match next_ready(win32, current(win32)) {
        Some(at) => enter(cpu, at),
        None => idle(cpu),
    }
}

/// Sleep(0): the running thread gives the rest of its turn to another
/// that is ready and of its priority or a higher one, and true if one
/// takes it. With no such thread, Sleep(0) returns at once and the
/// thread goes on with its turn, which isn't started over: a ready thread
/// of a lower priority doesn't get the processor from it (Sleep's
/// documentation until Windows Server 2003, which changed that). Nothing
/// changes then, and an idle proof follows the call (`idle::pure_call`):
/// a thread that waits for a flag in a Sleep(0) loop while the others
/// wait is a busy-wait loop, armed at the call (`idle::service_polled`)
/// and skipped to the next event like any other. The call reads nothing
/// but its argument, so a loop that also counts its passes is skipped
/// too. Switching to itself instead saved and loaded the thread's whole
/// state at every call, which ended the proof, and took the host about a
/// microsecond: MDK2's worker thread spins so, 14 instructions a pass,
/// while the main thread waits for the retrace in SwapBuffers, and the
/// host ran the spin several times slower than the emulated time it
/// covered (#1007).
pub fn yield_turn(cpu: &mut Cpu) -> bool {
    let win32 = &mut cpu.bus.win32;
    let Some(running) = current(win32) else {
        return false;
    };
    let mine = win32.processes[running.0].threads[running.1].priority;
    let takes = |win32: &Win32, (p, t): At| win32.processes[p].threads[t].priority >= mine;
    // A thread a higher priority took the processor from is at the head
    // of its priority's queue, with the rest of its turn.
    if let Some((at, used)) = take_preempted(win32, takes) {
        save(cpu, Context::of(cpu));
        run(cpu, at, used);
        return true;
    }
    let win32 = &cpu.bus.win32;
    let next = next_where(win32, Some(running), |at| {
        at != running && can_run(win32, at) && takes(win32, at)
    });
    match next {
        Some(at) => {
            save(cpu, Context::of(cpu));
            enter(cpu, at);
            true
        }
        None => {
            crate::idle::pure_call(cpu, None);
            crate::idle::reads_arguments(cpu);
            cpu.bus.observe.polled = true;
            false
        }
    }
}

/// Run the first thread that can run, or else the idle loop, with nothing
/// running to save (its process ended).
pub fn run_next(cpu: &mut Cpu) {
    if let Some((at, used)) = take_preempted(&mut cpu.bus.win32, |_, _| true) {
        run(cpu, at, used);
        return;
    }
    match next_ready(&cpu.bus.win32, None) {
        Some(at) => enter(cpu, at),
        None => idle(cpu),
    }
}

/// The thread a thread of a higher priority took the processor from last
/// that can still run and for which `pick` holds, and how much of its
/// turn it had used, taken from `preempted`, as Windows puts a preempted
/// thread back at the head of its priority's queue with what is left of
/// its quantum. Without that, the thread after the preempting one in turn
/// ran with a new turn: a multimedia timer of 10 ms, whose thread
/// preempts every period, started the main thread's turn over each time,
/// and another thread of the main one's priority never ran (Bedlam's
/// Windows version waits on its loader thread for ever). The last one
/// taken from goes first: when a thread that preempted another is
/// preempted in turn, it is the one that goes on once the third stops.
/// Those that can't run any more (ended, suspended) are dropped and get a
/// new turn when they run again; those `pick` passes over stay.
fn take_preempted(win32: &mut Win32, pick: impl Fn(&Win32, At) -> bool) -> Option<(At, u64)> {
    let mut i = win32.preempted.len();
    while i > 0 {
        i -= 1;
        let (tid, used) = win32.preempted[i];
        let at = find(win32, tid).filter(|&at| Some(at) != current(win32) && can_run(win32, at));
        match at {
            None => {
                win32.preempted.remove(i);
            }
            Some(at) if pick(&*win32, at) => {
                win32.preempted.remove(i);
                return Some((at, used));
            }
            Some(_) => {}
        }
    }
    None
}

/// Run the idle loop, in the running process: no thread runs.
fn idle(cpu: &mut Cpu) {
    charge(cpu);
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
    // No turn to end: the event left behind changes nothing.
    cpu.bus.win32.turn_due = None;
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
    // A program's level 0 code, as SafeDisc's, goes on without a switch:
    // its frame is on the level 0 stack, which every thread's entry from
    // level 3 uses. Windows 98 switches threads on the way back to level 3.
    if super::system::is_system(&ctx) {
        super::system::enter_user(cpu, &ctx);
        return;
    }
    let now = cpu.bus.clock.now_micros();
    let win32 = &mut cpu.bus.win32;
    match current(win32) {
        None => match next_ready(win32, None) {
            Some(at) => enter(cpu, at),
            None => super::system::enter_user(cpu, &ctx),
        },
        Some(running) => {
            // The turn ended while the thread ran the system's code or
            // real-mode code (`turn_over`), or, with no other thread, at
            // no event.
            if now >= turn_end(win32.slice_start) {
                if next_turn(cpu, running, ctx) {
                    return;
                }
                let win32 = &mut cpu.bus.win32;
                win32.slice_start = renewed(win32.slice_start, now);
                arm_turn(cpu);
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
        // The thread taken from goes on when the other stops, or before
        // the one taken from earlier if the other is preempted in turn.
        let now = cpu.bus.clock.now_micros();
        let win32 = &mut cpu.bus.win32;
        let used = now.saturating_sub(win32.slice_start);
        let id = win32.processes[p].threads[t].id;
        win32.preempted.push((id, used));
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
        Some(Kind::Change { signaled, .. }) => *signaled,
        Some(
            Kind::Mapping { .. } | Kind::Find { .. } | Kind::Snapshot { .. } | Kind::Vxd { .. },
        )
        | None => false,
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
    try_take(objects, tid, &wait.objects, wait.all)
}

/// What a wait of the thread `tid` for the objects `ids` (all of them, or
/// one) returns if it ends now, having taken what it waited for, or None
/// if it goes on.
fn try_take(objects: &mut Objects, tid: u32, ids: &[u32], all: bool) -> Option<u32> {
    if all {
        if ids.is_empty() || !ids.iter().all(|&o| signaled(objects, o, tid)) {
            return None;
        }
        let abandoned = ids.iter().fold(false, |a, &o| take(objects, o, tid) | a);
        return Some(if abandoned {
            WAIT_ABANDONED_0
        } else {
            WAIT_OBJECT_0
        });
    }
    let i = ids.iter().position(|&o| signaled(objects, o, tid))?;
    let abandoned = take(objects, ids[i], tid);
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
    if wait.section != 0 {
        return None;
    }
    poll_objects(cpu, &wait.objects, wait.all)
}

/// What a wait of the running thread for the objects `ids` (all of them,
/// or one) returns at once, having taken what it waited for, if it doesn't
/// have to wait.
pub fn poll_objects(cpu: &mut Cpu, ids: &[u32], all: bool) -> Option<u32> {
    super::kernel32::change::poll(cpu, ids);
    let tid = process(cpu).current_thread()?.id;
    try_take(&mut cpu.bus.win32.objects, tid, ids, all)
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
/// ends, or an ASPI request the drive is still working on is done, by
/// the clock's microseconds. A wait whose time has passed but that
/// hasn't ended, while the system runs code of its own, ends at the next
/// interrupt and gets no event of its own.
pub(crate) fn wait_due(win32: &Win32, now: u64) -> Option<u64> {
    win32
        .processes
        .iter()
        .flat_map(|p| &p.threads)
        .filter_map(|t| t.wait.as_ref()?.deadline)
        // An ASPI request ends a wait on its event when the drive is done.
        .chain(super::wnaspi32::due(&win32.wnaspi32))
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

/// The object `id` changed: the running thread set an event or released a
/// mutex or a semaphore, or the system set an event
/// (`winmm::callback::set_event`). The waits of the other threads that it
/// may end, end, in the threads' order, as `wake` ends them. Only a wait
/// for the object can. Whatever signals an object calls this or `wake`
/// then, so a wait for it doesn't wait for a later `wake`. What else ends
/// waits (time passing, input, the devices' notifications) `wake` looks
/// at where it comes: at the timer's events (`preempt`), after
/// interrupts, in the idle loop, and at the calls that post messages or
/// play sound. Calling `wake` here as well polled the devices and copied
/// every thread's wait at each SetEvent and ReleaseMutex, which made
/// ReleaseMutex cost the host as much as two other calls (#1222).
/// Whether it ended a wait.
pub fn object_changed(win32: &mut Win32, id: u32) -> bool {
    let running = current(win32);
    let mut ended = false;
    for p in 0..win32.processes.len() {
        for t in 0..win32.processes[p].threads.len() {
            if running == Some((p, t)) {
                continue;
            }
            let thread = &win32.processes[p].threads[t];
            let Some(wait) = thread.wait.as_ref().filter(|w| w.objects.contains(&id)) else {
                continue;
            };
            if let Some(result) = try_wait(&mut win32.objects, thread.id, wait) {
                end_wait(win32, (p, t), result);
                ended = true;
            }
        }
    }
    ended
}

/// End the waits that can end now. The threads whose waits timed out.
pub fn wake(cpu: &mut Cpu) -> Vec<At> {
    super::user32::input::poll(cpu);
    super::dinput::poll(cpu);
    super::winmm::tick(cpu);
    super::dsound::tick(cpu);
    super::wnaspi32::tick(cpu);
    super::dplayx::poll(cpu);
    // The change notifications the waiting threads wait for look at
    // their directories again.
    super::kernel32::change::poll_blocked(cpu);
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

/// Move the TIB fields of a state saved before `TIB_98` to where the TIB
/// has them now: each thread's TLS slots from 100h to 88h, with
/// `fs:[2Ch]` pointing at them there, and its last error from its entry
/// (the running thread's from its process's) into the TIB. Programs read
/// the slots through `fs:[2Ch]` and the last error through GetLastError,
/// so they find the same values after the load.
pub(crate) fn mend_tibs(bus: &mut Bus) {
    use super::loader::{TIB_LAST_ERROR, TIB_TLS_ARRAY, TLS_ARRAY, TLS_SLOTS};
    let size = 4 * TLS_SLOTS;
    for i in 0..bus.win32.processes.len() {
        let mut process = std::mem::take(&mut bus.win32.processes[i]);
        if let Some(running) = process.older_last_error.0.take() {
            let (memory, current) = (&process.memory, process.current);
            for (t, thread) in process.threads.iter_mut().enumerate() {
                let older = thread.older_last_error.0.take().unwrap_or(0);
                let tib = thread.tib;
                if tib == 0 {
                    continue;
                }
                let mut slots = vec![0; size as usize];
                if memory
                    .read(bus, tib + OLDER_TLS_ARRAY, &mut slots)
                    .is_some()
                {
                    // The old array's end, past the new one's.
                    let past = TLS_ARRAY + size;
                    let gone = vec![0; (OLDER_TLS_ARRAY + size - past) as usize];
                    memory.write(bus, tib + past, &gone);
                    memory.write(bus, tib + TLS_ARRAY, &slots);
                }
                memory.write_32(bus, tib + TIB_TLS_ARRAY, tib + TLS_ARRAY);
                let last_error = if current == Some(t) { running } else { older };
                memory.write_32(bus, tib + TIB_LAST_ERROR, last_error);
            }
        }
        bus.win32.processes[i] = process;
    }
}

crate::state_fields!(Thread {
    id,
    object,
    tib,
    selector,
    stack,
    ctx,
    fpu,
    sse,
    dr,
    older_last_error,
    suspended,
    priority,
    exit_code,
    wait,
    calls,
    fault
});

#[cfg(test)]
mod tests {
    use super::*;
    use crate::savestate::Lost;
    use crate::win32::loader::{TIB_LAST_ERROR, TIB_TLS_ARRAY, TLS_ARRAY, TLS_SLOTS};
    use crate::win32::memory::{PAGE, Pool, SYSTEM};

    /// What an older state has in slot `slot` of the TIB at `tib`.
    fn value(tib: u32, slot: u32) -> u32 {
        tib ^ (0x5A00 + slot)
    }

    /// A process of a state saved before `TIB_98`, with two threads, the
    /// second running: the load moves each thread's slots from 100h to
    /// 88h, clears the old array's end past the new one's, points
    /// `fs:[2Ch]` at them, and puts the running thread's last error,
    /// which the process kept, and the other's, which its entry kept, at
    /// TIB+60h. A process of a newer state, with nothing lost, is left
    /// as it is.
    #[test]
    fn an_older_states_tibs_get_their_slots_and_last_errors_moved() {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        cpu.bus.win32.pool = Pool::take(&mut cpu.bus, 0x40_0000);
        let mut processes = Vec::new();
        let mut tibs = Vec::new();
        for older in [true, false] {
            let mut memory = Memory::new(&mut cpu.bus).expect("a page directory");
            let mut threads = Vec::new();
            for t in 0..2 {
                let bus = &mut cpu.bus;
                let tib = memory.reserve(None, GRANULARITY, SHARED, SYSTEM).unwrap();
                memory.commit(bus, tib, PAGE, USER | WRITABLE).unwrap();
                memory
                    .write_32(bus, tib + TIB_TLS_ARRAY, tib + OLDER_TLS_ARRAY)
                    .unwrap();
                for slot in 0..TLS_SLOTS {
                    let at = tib + OLDER_TLS_ARRAY + 4 * slot;
                    memory.write_32(bus, at, value(tib, slot)).unwrap();
                }
                threads.push(Thread {
                    tib,
                    older_last_error: Lost(older.then_some(10 + t)),
                    ..Thread::default()
                });
                tibs.push((older, t, tib));
            }
            processes.push(super::super::Process {
                memory,
                threads,
                current: Some(1),
                older_last_error: Lost(older.then_some(99)),
                ..Default::default()
            });
        }
        cpu.bus.win32.processes = processes;
        mend_tibs(&mut cpu.bus);
        let bus = &cpu.bus;
        for (p, process) in bus.win32.processes.iter().enumerate() {
            assert_eq!(process.older_last_error.0, None);
            let memory = &process.memory;
            for thread in &process.threads {
                assert_eq!(thread.older_last_error.0, None);
            }
            for &(older, t, tib) in tibs.iter().skip(2 * p).take(2) {
                let read = |at: u32| memory.read_32(bus, tib + at).unwrap();
                if !older {
                    assert_eq!(read(TIB_TLS_ARRAY), tib + OLDER_TLS_ARRAY);
                    assert_eq!(read(TIB_LAST_ERROR), 0);
                    assert_eq!(read(OLDER_TLS_ARRAY), value(tib, 0));
                    continue;
                }
                assert_eq!(read(TIB_TLS_ARRAY), tib + TLS_ARRAY);
                for slot in 0..TLS_SLOTS {
                    assert_eq!(read(TLS_ARRAY + 4 * slot), value(tib, slot), "slot {slot}");
                }
                for at in (TLS_ARRAY + 4 * TLS_SLOTS..OLDER_TLS_ARRAY + 4 * TLS_SLOTS).step_by(4) {
                    assert_eq!(read(at), 0, "TIB+{at:X}h");
                }
                let error = if t == 1 { 99 } else { 10 };
                assert_eq!(read(TIB_LAST_ERROR), error, "thread {t}");
            }
        }
    }
}
