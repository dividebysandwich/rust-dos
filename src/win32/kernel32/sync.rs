//! Synchronization: events, mutexes and semaphores, the wait functions,
//! Sleep, and critical sections. The objects are kernel objects
//! (object.rs); the waits are the threads' (thread.rs).

use super::{ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, TRUE, set_error, string_at};
use crate::cpu::Cpu;
use crate::win32::object::{self, Kind};
use crate::win32::process;
use crate::win32::thread::{self, WAIT_TIMEOUT, Wait};

const INFINITE: u32 = 0xFFFF_FFFF;
const WAIT_FAILED: u32 = 0xFFFF_FFFF;
/// The most objects a wait takes.
const MAXIMUM_WAIT_OBJECTS: u32 = 64;

const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_NOT_OWNER: u32 = 288;
const ERROR_TOO_MANY_POSTS: u32 = 298;

/// The name at `at`, or None for NULL: Some(None) for no name, None if a
/// page isn't there.
fn name_at(cpu: &mut Cpu, at: u32) -> Option<Option<String>> {
    match at {
        0 => Some(None),
        at => string_at(cpu, at).map(|name| (!name.is_empty()).then_some(name)),
    }
}

/// Make an object of `kind` named `name`, or with a name an object of the
/// same kind has already, a handle of that one (the last error then says
/// so). Fails with ERROR_INVALID_HANDLE if another kind has the name.
fn create(cpu: &mut Cpu, kind: Kind, name: Option<String>) -> u32 {
    let objects = object::objects(cpu);
    if let Some(name) = &name
        && let Some(id) = objects.named(name)
    {
        let same = objects
            .get(id)
            .is_some_and(|o| std::mem::discriminant(&o.kind) == std::mem::discriminant(&kind));
        if !same {
            set_error(cpu, ERROR_INVALID_HANDLE);
            return 0;
        }
        objects.hold(id);
        set_error(cpu, ERROR_ALREADY_EXISTS);
        return object::new_handle(cpu, id);
    }
    set_error(cpu, 0);
    object::create(cpu, kind, name)
}

/// A handle of the object named at `at`, if it is of the kind `same`
/// says.
fn open(cpu: &mut Cpu, at: u32, same: fn(&Kind) -> bool) -> Option<u32> {
    let Some(name) = name_at(cpu, at)? else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    };
    let objects = object::objects(cpu);
    match objects.named(&name) {
        Some(id) if objects.get(id).is_some_and(|o| same(&o.kind)) => {
            objects.hold(id);
            Some(object::new_handle(cpu, id))
        }
        Some(_) => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            Some(0)
        }
        None => {
            set_error(cpu, ERROR_FILE_NOT_FOUND);
            Some(0)
        }
    }
}

/// CreateEventA(attributes, manual, initial, name).
pub(super) fn create_event(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = name_at(cpu, args[3])?;
    let kind = Kind::Event {
        manual: args[1] != 0,
        signaled: args[2] != 0,
    };
    Some(create(cpu, kind, name))
}

/// OpenEventA(access, inherit, name).
pub(super) fn open_event(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    open(cpu, args[2], |k| matches!(k, Kind::Event { .. }))
}

/// CreateMutexA(attributes, owned, name).
pub(super) fn create_mutex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let name = name_at(cpu, args[2])?;
    let tid = process(cpu).current_thread().map_or(0, |t| t.id);
    let owned = args[1] != 0;
    let kind = Kind::Mutex {
        owner: if owned { tid } else { 0 },
        count: owned as u32,
        abandoned: false,
    };
    let handle = create(cpu, kind, name);
    // An existing mutex isn't taken.
    Some(handle)
}

/// OpenMutexA(access, inherit, name).
pub(super) fn open_mutex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    open(cpu, args[2], |k| matches!(k, Kind::Mutex { .. }))
}

/// CreateSemaphoreA(attributes, initial, maximum, name).
pub(super) fn create_semaphore(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (initial, max) = (args[1] as i32, args[2] as i32);
    if max <= 0 || initial < 0 || initial > max {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(0);
    }
    let name = name_at(cpu, args[3])?;
    let kind = Kind::Semaphore {
        count: initial as u32,
        max: max as u32,
    };
    Some(create(cpu, kind, name))
}

/// OpenSemaphoreA(access, inherit, name).
pub(super) fn open_semaphore(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    open(cpu, args[2], |k| matches!(k, Kind::Semaphore { .. }))
}

/// The object of `handle`'s kind, changed by `f`, which says whether it
/// may be; then the waits it may end, end (`thread::object_changed`).
/// False, with the last error set, if `handle` isn't a handle of the kind
/// `f` takes.
fn change(cpu: &mut Cpu, handle: u32, f: impl FnOnce(&mut Kind, u32) -> Result<(), u32>) -> u32 {
    let Some(id) = object::resolve(cpu, handle) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return FALSE;
    };
    let tid = process(cpu).current_thread().map_or(0, |t| t.id);
    let Some(object) = object::objects(cpu).get_mut(id) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return FALSE;
    };
    if let Err(error) = f(&mut object.kind, tid) {
        set_error(cpu, error);
        return FALSE;
    }
    thread::object_changed(&mut cpu.bus.win32, id);
    TRUE
}

fn set_signal(kind: &mut Kind, to: bool) -> Result<(), u32> {
    match kind {
        Kind::Event { signaled, .. } => {
            *signaled = to;
            Ok(())
        }
        _ => Err(ERROR_INVALID_HANDLE),
    }
}

pub(super) fn set_event(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(change(cpu, args[0], |k, _| set_signal(k, true)))
}

pub(super) fn reset_event(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(change(cpu, args[0], |k, _| set_signal(k, false)))
}

/// PulseEvent: the waiting threads it can release go (all for a manual
/// event, one for an auto-reset one), and the event is reset.
pub(super) fn pulse_event(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let done = change(cpu, args[0], |k, _| set_signal(k, true));
    if done == TRUE {
        change(cpu, args[0], |k, _| set_signal(k, false));
    }
    Some(done)
}

/// ReleaseMutex: the owner gives it up once.
pub(super) fn release_mutex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(change(cpu, args[0], |kind, tid| match kind {
        Kind::Mutex { owner, count, .. } if *owner == tid && *count > 0 => {
            *count -= 1;
            if *count == 0 {
                *owner = 0;
            }
            Ok(())
        }
        Kind::Mutex { .. } => Err(ERROR_NOT_OWNER),
        _ => Err(ERROR_INVALID_HANDLE),
    }))
}

/// ReleaseSemaphore(handle, count, previous).
pub(super) fn release_semaphore(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[handle, add, previous] = args else {
        unreachable!()
    };
    let mut before = None;
    let done = change(cpu, handle, |kind, _| match kind {
        Kind::Semaphore { count, max } => {
            if add as i32 <= 0 {
                return Err(ERROR_INVALID_PARAMETER);
            }
            if count.checked_add(add).is_none_or(|n| n > *max) {
                return Err(ERROR_TOO_MANY_POSTS);
            }
            before = Some(*count);
            *count += add;
            Ok(())
        }
        _ => Err(ERROR_INVALID_HANDLE),
    });
    if let Some(before) = before
        && previous != 0
    {
        cpu.bus.guest_write_32(previous, before);
    }
    Some(done)
}

/// Wait for the objects of `handles` (all, or one), for at most `ms`. A
/// wait that ends at once, and one with no time to wait that times out at
/// once, as the polls of a loop that waits do, allocate nothing.
/// `arguments`: the call read no RAM but its arguments (`found_none`).
fn wait(cpu: &mut Cpu, handles: &[u32], all: bool, ms: u32, arguments: bool) -> Option<u32> {
    debug_assert!(handles.len() <= MAXIMUM_WAIT_OBJECTS as usize);
    let mut ids = [0; MAXIMUM_WAIT_OBJECTS as usize];
    for (id, &handle) in ids.iter_mut().zip(handles) {
        match object::resolve(cpu, handle) {
            Some(found) => *id = found,
            None => {
                set_error(cpu, ERROR_INVALID_HANDLE);
                return Some(WAIT_FAILED);
            }
        }
    }
    let objects = &ids[..handles.len()];
    if let Some(result) = thread::poll_objects(cpu, objects, all) {
        return Some(result);
    }
    if ms == 0 {
        found_none(cpu, objects, arguments);
        return Some(WAIT_TIMEOUT);
    }
    let deadline = (ms != INFINITE).then(|| cpu.bus.clock.now_micros() + ms as u64 * 1000);
    let wait = Wait {
        objects: objects.to_vec(),
        all,
        deadline,
        timeout_result: WAIT_TIMEOUT,
        section: 0,
        queue: None,
    };
    thread::block(cpu, wait);
    None
}

/// A wait with no time to wait found none of `objects` signaled, and
/// returns WAIT_TIMEOUT: it changed nothing, not even the last error, and
/// the program polls them in a loop. For an idle proof (`idle::pure_call`)
/// the call is the same in every pass until one of them is signaled, and
/// nothing in a proof's passes can do that:
///
/// - another thread runs only after a switch, which comes at a timer
///   event or an interrupt, where a proof ends and up to which the passes
///   it proved are skipped, or at a call of the running thread that blocks
///   or yields, which isn't one an idle proof follows;
/// - what the system signals of its own (a sound buffer's notification, a
///   multimedia timer's event, input) it signals at those events and
///   interrupts too;
/// - the running thread changes objects only through calls (SetEvent,
///   ReleaseMutex, CloseHandle), none of which an idle proof follows;
/// - a mutex the running thread holds is signaled for it, and a wait for
///   it doesn't come here.
///
/// A change notification is the exception: the wait looks at its
/// directory on the host, which another program may change, and notes
/// when it looked. `arguments`: the call read no RAM but its arguments,
/// which a pass that counts needs (`idle::reads_arguments`). The polls
/// arm the loop (`idle::service_polled`), as Sleep(0)'s do: a worker
/// thread that polls a mutex another thread holds, with nothing else to
/// do, spins for the rest of its turn (#1222).
fn found_none(cpu: &mut Cpu, objects: &[u32], arguments: bool) {
    let objects_of = &cpu.bus.win32.objects;
    let looks = objects.iter().any(|&id| {
        matches!(
            objects_of.get(id).map(|o| &o.kind),
            Some(Kind::Change { .. })
        )
    });
    if looks {
        return;
    }
    crate::idle::pure_call(cpu, None);
    if arguments {
        crate::idle::reads_arguments(cpu);
    }
    cpu.bus.observe.polled = true;
}

/// WaitForSingleObject(handle, ms), and WaitForSingleObjectEx with its
/// alertable flag, which nothing alerts yet.
pub(super) fn wait_for_single_object(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    wait(cpu, &[args[0]], false, args[1], true)
}

/// WaitForMultipleObjects(count, handles, all, ms), and the Ex one.
pub(super) fn wait_for_multiple_objects(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[count, at, all, ms, ..] = args else {
        unreachable!()
    };
    if count == 0 || count > MAXIMUM_WAIT_OBJECTS {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(WAIT_FAILED);
    }
    let mut handles = [0; MAXIMUM_WAIT_OBJECTS as usize];
    let handles = &mut handles[..count as usize];
    for (i, handle) in (0..).zip(handles.iter_mut()) {
        *handle = cpu.bus.guest_read_32(at.wrapping_add(4 * i));
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    // The handles are arguments too where they are in the 64 bytes above
    // the stack's top that a pass that counts takes a call to read, as an
    // array on the caller's stack often is.
    let top = cpu.esp() as u64;
    let (start, end) = (at as u64, at as u64 + 4 * count as u64);
    let arguments = start >= top && end <= top + 64;
    wait(cpu, handles, all != 0, ms, arguments)
}

/// Sleep(ms), and SleepEx(ms, alertable), which returns 0: other threads
/// run meanwhile. Sleep(0) gives the rest of the turn to another thread
/// of the same priority or a higher one that is ready, and with none,
/// returns at once (`thread::yield_turn`).
pub(super) fn sleep(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let ms = args[0];
    cpu.set_eax(0);
    if ms == 0 {
        // A call that switched threads returns when the thread runs again.
        let switched = thread::yield_turn(cpu);
        return (!switched).then_some(0);
    }
    let deadline = (ms != INFINITE).then(|| cpu.bus.clock.now_micros() + ms as u64 * 1000);
    thread::block(
        cpu,
        Wait {
            deadline,
            ..Wait::default()
        },
    );
    None
}

/// CRITICAL_SECTION's fields, as NT has them (Windows 95's are its own,
/// and programs don't read them): LockCount, -1 when free and then the
/// number of threads in it or waiting; RecursionCount; OwningThread.
const LOCK_COUNT: u32 = 4;
const RECURSION_COUNT: u32 = 8;
const OWNING_THREAD: u32 = 12;
const CRITICAL_SECTION: usize = 24;

pub(super) fn initialize_critical_section(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    cpu.bus.guest_fill(at, CRITICAL_SECTION, 0);
    cpu.bus.guest_write_32(at + LOCK_COUNT, 0xFFFF_FFFF);
    Some(0)
}

pub(super) fn delete_critical_section(_: &mut Cpu, _: &[u32]) -> Option<u32> {
    Some(0)
}

/// EnterCriticalSection: in at once if it is free or the thread's own,
/// else wait until the thread in it leaves it to this one.
pub(super) fn enter_critical_section(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    let tid = process(cpu).current_thread().map_or(0, |t| t.id);
    let owner = cpu.bus.guest_read_32(at + OWNING_THREAD);
    let count = cpu.bus.guest_read_32(at + RECURSION_COUNT);
    let lock = cpu.bus.guest_read_32(at + LOCK_COUNT);
    if cpu.bus.guest_faulted() {
        return None;
    }
    cpu.bus
        .guest_write_32(at + LOCK_COUNT, lock.wrapping_add(1));
    if owner == 0 || owner == tid || count == 0 {
        cpu.bus.guest_write_32(at + OWNING_THREAD, tid);
        cpu.bus.guest_write_32(
            at + RECURSION_COUNT,
            if owner == tid { count + 1 } else { 1 },
        );
        return Some(0);
    }
    thread::block(
        cpu,
        Wait {
            section: at,
            ..Wait::default()
        },
    );
    None
}

/// TryEnterCriticalSection: in if it can be at once.
pub(super) fn try_enter_critical_section(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    let tid = process(cpu).current_thread().map_or(0, |t| t.id);
    let owner = cpu.bus.guest_read_32(at + OWNING_THREAD);
    let count = cpu.bus.guest_read_32(at + RECURSION_COUNT);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if owner != 0 && owner != tid && count != 0 {
        return Some(FALSE);
    }
    enter_critical_section(cpu, args)?;
    Some(TRUE)
}

/// LeaveCriticalSection: out once; when the thread is out for good, the
/// first thread waiting gets in.
pub(super) fn leave_critical_section(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    let tid = process(cpu).current_thread().map_or(0, |t| t.id);
    let owner = cpu.bus.guest_read_32(at + OWNING_THREAD);
    let count = cpu.bus.guest_read_32(at + RECURSION_COUNT);
    let lock = cpu.bus.guest_read_32(at + LOCK_COUNT);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if owner != tid || count == 0 {
        return Some(0);
    }
    cpu.bus
        .guest_write_32(at + LOCK_COUNT, lock.wrapping_sub(1));
    if count > 1 {
        cpu.bus.guest_write_32(at + RECURSION_COUNT, count - 1);
        return Some(0);
    }
    let win32 = &mut cpu.bus.win32;
    let p = win32.running;
    let next = win32.processes[p]
        .threads
        .iter()
        .position(|t| t.wait.as_ref().is_some_and(|w| w.section == at))
        .map(|t| {
            thread::end_wait(win32, (p, t), 0);
            win32.processes[p].threads[t].id
        });
    cpu.bus
        .guest_write_32(at + OWNING_THREAD, next.unwrap_or(0));
    cpu.bus
        .guest_write_32(at + RECURSION_COUNT, next.is_some() as u32);
    Some(0)
}
