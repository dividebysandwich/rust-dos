//! Synchronization: events, mutexes and semaphores, the wait functions,
//! Sleep, and critical sections, which are mostly x86 code. The objects
//! are kernel objects (object.rs); the waits are the threads'
//! (thread.rs).

use super::{ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE, TRUE, set_error, string_at};
use crate::cpu::Cpu;
use crate::win32::heap::Heap;
use crate::win32::object::{self, Kind};
use crate::win32::thread::{self, WAIT_TIMEOUT, Wait};
use crate::win32::{TRAP, process, with_process};

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
    // The handles are arguments too where they are in the bytes above the
    // stack's top that a pass that counts takes a call to read, as an
    // array on the caller's stack often is. Elsewhere a counter may be
    // among them, and the loop isn't skipped.
    let arguments = crate::idle::above_stack_top(cpu, at, 4 * count);
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

// Critical sections, as Windows 98 SE's KERNEL32 has them. The program's
// CRITICAL_SECTION holds a type byte (4) and a pointer to the section's
// object in the system's heap; InitializeCriticalSection writes nothing
// else of its 24 bytes. The object has the section's state:
//
//   00h  byte   4, the object's type
//   04h  dword  how many times the owner is in it
//   08h  dword  the owner, its TIB here (Windows 98's TDB, a pointer too)
//   10h  dword  1 when it is free, less one for each Enter that hasn't
//               left: 0 with an owner alone, below 0 with threads waiting
//   18h  dword  Windows 98's event for the waits; here, where the waits
//               are the threads' (`Wait::section`), the hand-overs kept
//               for threads that counted themselves in but don't wait yet
//   1Ch  dword  the program's CRITICAL_SECTION
//
// Windows 95's CRITICAL_SECTION has a type dword of 4 and the pointer at
// 0Ch instead; the built-in Windows has Windows 98's for both. Programs
// can't read NT's LockCount, RecursionCount or OwningThread on either.
//
// EnterCriticalSection and LeaveCriticalSection are x86 code, which the
// recompiler runs with the program's code. They trap only to wait for a
// section another thread is in and to hand a section that was left to a
// thread waiting for it. A thread can be switched between any two of
// their instructions, so each claims or frees the section with one
// instruction on the counter at 10h: Enter decrements it before it writes
// the owner, and Leave clears the owner before it increments it. A
// hand-over can then come before the thread it is for waits
// (`critical_section_wake`).

/// The type byte of a CRITICAL_SECTION and of its object.
const SECTION_TYPE: u8 = 4;
/// Where a CRITICAL_SECTION points to its object.
const SECTION_OBJECT: u32 = 4;
/// The object's fields and size.
const RECURSION: u32 = 0x04;
const OWNER: u32 = 0x08;
const LOCK: u32 = 0x10;
const HANDOVERS: u32 = 0x18;
const SECTION: u32 = 0x1C;
const OBJECT_SIZE: u32 = 0x20;
/// The system heap's size: room for 32768 sections' objects.
const SYSTEM_HEAP_SIZE: u32 = 0x10_0000;

/// What Windows 98 raises for a CRITICAL_SECTION it didn't initialize.
const STATUS_ACCESS_VIOLATION: u32 = 0xC000_0005;

/// The ids of the functions the code below traps to: their places in
/// KERNEL32's table (KERNEL32 is the first DLL, so its ids are its
/// places). A test checks them.
pub(super) const RAISE_EXCEPTION_ID: u16 = 87;
pub(super) const CREATE_ID: u16 = 486;
pub(super) const DELETE_ID: u16 = 487;
pub(super) const WAIT_ID: u16 = 488;
pub(super) const WAKE_ID: u16 = 489;

const fn lo(id: u16) -> u8 {
    id as u8
}

const fn hi(id: u16) -> u8 {
    (id >> 8) as u8
}

/// InitializeCriticalSection(section): `critical_section_create`.
#[rustfmt::skip]
pub(super) const INITIALIZE_CRITICAL_SECTION: &[u8] = &[
    0xFE, TRAP, lo(CREATE_ID), hi(CREATE_ID), // trap CriticalSectionCreate
    0xC2, 0x04, 0x00, //                          ret 4
];

/// DeleteCriticalSection(section): `critical_section_delete`.
#[rustfmt::skip]
pub(super) const DELETE_CRITICAL_SECTION: &[u8] = &[
    0xFE, TRAP, lo(DELETE_ID), hi(DELETE_ID), // trap CriticalSectionDelete
    0xC2, 0x04, 0x00, //                          ret 4
];

/// EnterCriticalSection(section): in at once when the section is free or
/// the thread's own, else wait. A CRITICAL_SECTION that wasn't
/// initialized raises an access violation; a handler that continues
/// from it goes on as if it was, as on Windows 98.
#[rustfmt::skip]
pub(super) const ENTER_CRITICAL_SECTION: &[u8] = &[
    // The section in ECX, its object in EDX, the thread's TIB in EAX.
    0x8B, 0x4C, 0x24, 0x04, //       00: mov ecx, [esp+4]
    0x80, 0x39, SECTION_TYPE, //     04: cmp byte [ecx], 4
    0x75, 0x2B, //                   07: jne 34h (uninitialized)
    0x8B, 0x51, 0x04, //             09: mov edx, [ecx+4]
    0x64, 0xA1, 0x18, 0x00, 0x00, 0x00, // 0C: mov eax, fs:[18h]
    0xFF, 0x4A, 0x10, //             12: dec dword [edx+10h]
    0x75, 0x09, //                   15: jne 20h (taken)
    // Free: the thread owns it now.
    0x89, 0x42, 0x08, //             17: mov [edx+8], eax
    0xFF, 0x42, 0x04, //             1A: inc dword [edx+4]
    0xC2, 0x04, 0x00, //             1D: ret 4
    // Taken: by this thread, or wait.
    0x39, 0x42, 0x08, //             20: cmp [edx+8], eax
    0x75, 0x06, //                   23: jne 2Bh
    0xFF, 0x42, 0x04, //             25: inc dword [edx+4]
    0xC2, 0x04, 0x00, //             28: ret 4
    0x52, //                         2B: push edx
    0xE8, 0x17, 0x00, 0x00, 0x00, // 2C: call 48h
    0xC2, 0x04, 0x00, //             31: ret 4
    // Uninitialized: RaiseException(STATUS_ACCESS_VIOLATION, 0, 0, NULL),
    // then on from 09h if a handler continues.
    0x51, //                         34: push ecx
    0x6A, 0x00, //                   35: push 0
    0x6A, 0x00, //                   37: push 0
    0x6A, 0x00, //                   39: push 0
    0x68, 0x05, 0x00, 0x00, 0xC0, // 3B: push C0000005h
    0xE8, 0x0A, 0x00, 0x00, 0x00, // 40: call 4Fh
    0x59, //                         45: pop ecx
    0xEB, 0xC1, //                   46: jmp 09h
    0xFE, TRAP, lo(WAIT_ID), hi(WAIT_ID), // 48: trap CriticalSectionWait
    0xC2, 0x04, 0x00, //             4C: ret 4
    0xFE, TRAP, lo(RAISE_EXCEPTION_ID), hi(RAISE_EXCEPTION_ID), // 4F: trap RaiseException
    0xC2, 0x10, 0x00, //             53: ret 10h
];

/// LeaveCriticalSection(section): out once, and when the thread is out
/// for good with others waiting, the first of them gets in. It doesn't
/// look at who the owner is, and a CRITICAL_SECTION that wasn't
/// initialized is left alone, as on Windows 98.
#[rustfmt::skip]
pub(super) const LEAVE_CRITICAL_SECTION: &[u8] = &[
    // The section in ECX, its object in EDX.
    0x8B, 0x4C, 0x24, 0x04, //       00: mov ecx, [esp+4]
    0x80, 0x39, SECTION_TYPE, //     04: cmp byte [ecx], 4
    0x75, 0x0B, //                   07: jne 14h
    0x8B, 0x51, 0x04, //             09: mov edx, [ecx+4]
    0xFF, 0x4A, 0x04, //             0C: dec dword [edx+4]
    0x74, 0x06, //                   0F: je 17h (out for good)
    0xFF, 0x42, 0x10, //             11: inc dword [edx+10h]
    0xC2, 0x04, 0x00, //             14: ret 4
    0xC7, 0x42, 0x08, 0x00, 0x00, 0x00, 0x00, // 17: mov dword [edx+8], 0
    0xFF, 0x42, 0x10, //             1E: inc dword [edx+10h]
    0x7F, 0xF1, //                   21: jg 14h (nobody else counted in)
    0x52, //                         23: push edx
    0xE8, 0x03, 0x00, 0x00, 0x00, // 24: call 2Ch
    0xC2, 0x04, 0x00, //             29: ret 4
    0xFE, TRAP, lo(WAKE_ID), hi(WAKE_ID), // 2C: trap CriticalSectionWake
    0xC2, 0x04, 0x00, //             30: ret 4
];

/// The running thread's TIB, which a section's object names its owner by
/// (fs:[18h] to the code above).
fn current_tib(cpu: &mut Cpu) -> u32 {
    process(cpu).current_thread().map_or(0, |t| t.tib)
}

/// InitializeCriticalSection's trap: the section's object, made in the
/// system heap, free, and the CRITICAL_SECTION at `args[0]` pointing to
/// it. As Windows 98 does, it first writes the CRITICAL_SECTION's first
/// dword back, so that a pointer to memory the program can't write
/// faults before anything is made, and it raises an access violation when
/// the heap is full.
pub(super) fn critical_section_create(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    for field in [0, SECTION_OBJECT] {
        let value = cpu.bus.guest_read_32(at.wrapping_add(field));
        cpu.bus.guest_write_32(at.wrapping_add(field), value);
    }
    if cpu.bus.guest_faulted() {
        return None;
    }
    let object = with_process(cpu, |p, bus| {
        if p.system_heap.is_none() {
            *p.system_heap = Heap::create_shared(&mut p.memory, SYSTEM_HEAP_SIZE);
        }
        let object = p
            .system_heap
            .as_mut()?
            .alloc(&mut p.memory, bus, OBJECT_SIZE)?;
        let mut bytes = [0; OBJECT_SIZE as usize];
        bytes[0] = SECTION_TYPE;
        bytes[LOCK as usize..][..4].copy_from_slice(&1u32.to_le_bytes());
        bytes[SECTION as usize..][..4].copy_from_slice(&at.to_le_bytes());
        p.memory.write(bus, object, &bytes)?;
        Some(object)
    });
    let Some(object) = object else {
        return super::super::seh::raise_from_call(cpu, 1, STATUS_ACCESS_VIOLATION, 0, &[]);
    };
    cpu.bus.guest_write_8(at, SECTION_TYPE);
    cpu.bus
        .guest_write_32(at.wrapping_add(SECTION_OBJECT), object);
    Some(0)
}

/// DeleteCriticalSection's trap: the object of the CRITICAL_SECTION at
/// `args[0]` goes back to the system heap. The CRITICAL_SECTION stays as
/// it is, still pointing there, as on Windows 98. One that wasn't
/// initialized raises an access violation.
pub(super) fn critical_section_delete(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    let kind = cpu.bus.guest_read_8(at);
    let object = cpu.bus.guest_read_32(at.wrapping_add(SECTION_OBJECT));
    if cpu.bus.guest_faulted() {
        return None;
    }
    if kind != SECTION_TYPE {
        return super::super::seh::raise_from_call(cpu, 1, STATUS_ACCESS_VIOLATION, 0, &[]);
    }
    if let Some(heap) = process(cpu).system_heap.as_mut() {
        heap.free(object);
    }
    Some(0)
}

/// EnterCriticalSection's trap, after its code counted the thread in
/// (`LOCK`) and found the section with another owner, or none yet: the
/// thread waits until a thread leaving the section hands it over
/// (`critical_section_wake`). A hand-over may have come between the count
/// and the trap, with no thread waiting then (`HANDOVERS`): the section
/// is this thread's at once.
///
/// The owner can't tell: it is 0 both while a thread that left the
/// section is on its way to the wake and while one that just counted
/// itself in to a free section is about to write itself there.
pub(super) fn critical_section_wait(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let object = args[0];
    let handovers = cpu.bus.guest_read_32(object.wrapping_add(HANDOVERS));
    if cpu.bus.guest_faulted() {
        return None;
    }
    if handovers != 0 {
        let tib = current_tib(cpu);
        cpu.bus
            .guest_write_32(object.wrapping_add(HANDOVERS), handovers - 1);
        cpu.bus.guest_write_32(object.wrapping_add(OWNER), tib);
        cpu.bus.guest_write_32(object.wrapping_add(RECURSION), 1);
        return Some(0);
    }
    thread::block(
        cpu,
        Wait {
            section: object,
            ..Wait::default()
        },
    );
    None
}

/// LeaveCriticalSection's trap, after its code left the section and found
/// other threads counted in (`LOCK` at 0 or below): the first that waits
/// for the section, in the threads' order, gets it. With none waiting
/// yet, one was switched out between its count and its trap, and the
/// hand-over is kept for the first to reach its trap
/// (`critical_section_wait`).
pub(super) fn critical_section_wake(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let object = args[0];
    let handovers = cpu.bus.guest_read_32(object.wrapping_add(HANDOVERS));
    if cpu.bus.guest_faulted() {
        return None;
    }
    let win32 = &mut cpu.bus.win32;
    let p = win32.running;
    let waiting = win32.processes[p]
        .threads
        .iter()
        .position(|t| t.wait.as_ref().is_some_and(|w| w.section == object));
    let Some(t) = waiting else {
        cpu.bus
            .guest_write_32(object.wrapping_add(HANDOVERS), handovers.wrapping_add(1));
        return Some(0);
    };
    thread::end_wait(win32, (p, t), 0);
    let tib = win32.processes[p].threads[t].tib;
    cpu.bus.guest_write_32(object.wrapping_add(OWNER), tib);
    cpu.bus.guest_write_32(object.wrapping_add(RECURSION), 1);
    Some(0)
}

// The handlers below are the critical section functions of states saved
// when they were traps, whose memory still has those stubs and the
// sections those made, in NT's layout: LockCount, -1 when free and then
// the number of threads in it or waiting; RecursionCount; OwningThread.
// They go on with those until the process ends.
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

#[cfg(test)]
mod tests {
    use super::*;
    use iced_x86::code_asm::*;

    #[test]
    fn the_traps_of_the_critical_sections_code_are_its_functions() {
        for (id, name) in [
            (RAISE_EXCEPTION_ID, "RaiseException"),
            (CREATE_ID, "CriticalSectionCreate"),
            (DELETE_ID, "CriticalSectionDelete"),
            (WAIT_ID, "CriticalSectionWait"),
            (WAKE_ID, "CriticalSectionWake"),
        ] {
            let found = crate::win32::stubs::function_name(id);
            assert_eq!(found, Some(format!("KERNEL32.{name}")), "{id}");
        }
    }

    fn trap(a: &mut CodeAssembler, id: u16) {
        let [lo, hi] = id.to_le_bytes();
        a.db(&[0xFE, TRAP, lo, hi]).unwrap();
    }

    /// The bytes of EnterCriticalSection and LeaveCriticalSection are the
    /// instructions their comments list, assembled.
    #[test]
    fn the_critical_sections_code_is_its_listing() {
        let mut a = CodeAssembler::new(32).unwrap();
        let mut uninitialized = a.create_label();
        let mut initialized = a.create_label();
        let mut taken = a.create_label();
        let mut wait = a.create_label();
        let mut wait_trap = a.create_label();
        let mut raise_trap = a.create_label();
        a.mov(ecx, dword_ptr(esp + 4)).unwrap();
        a.cmp(byte_ptr(ecx), 4).unwrap();
        a.jne(uninitialized).unwrap();
        a.set_label(&mut initialized).unwrap();
        a.mov(edx, dword_ptr(ecx + 4)).unwrap();
        a.mov(eax, dword_ptr(0x18u64).fs()).unwrap();
        a.dec(dword_ptr(edx + 0x10)).unwrap();
        a.jne(taken).unwrap();
        a.mov(dword_ptr(edx + 8), eax).unwrap();
        a.inc(dword_ptr(edx + 4)).unwrap();
        a.ret_1(4).unwrap();
        a.set_label(&mut taken).unwrap();
        a.cmp(dword_ptr(edx + 8), eax).unwrap();
        a.jne(wait).unwrap();
        a.inc(dword_ptr(edx + 4)).unwrap();
        a.ret_1(4).unwrap();
        a.set_label(&mut wait).unwrap();
        a.push(edx).unwrap();
        a.call(wait_trap).unwrap();
        a.ret_1(4).unwrap();
        a.set_label(&mut uninitialized).unwrap();
        a.push(ecx).unwrap();
        a.push(0).unwrap();
        a.push(0).unwrap();
        a.push(0).unwrap();
        a.push(STATUS_ACCESS_VIOLATION as i32).unwrap();
        a.call(raise_trap).unwrap();
        a.pop(ecx).unwrap();
        a.jmp(initialized).unwrap();
        a.set_label(&mut wait_trap).unwrap();
        trap(&mut a, WAIT_ID);
        a.ret_1(4).unwrap();
        a.set_label(&mut raise_trap).unwrap();
        trap(&mut a, RAISE_EXCEPTION_ID);
        a.ret_1(0x10).unwrap();
        assert_eq!(a.assemble(0).unwrap(), ENTER_CRITICAL_SECTION);

        let mut a = CodeAssembler::new(32).unwrap();
        let mut done = a.create_label();
        let mut released = a.create_label();
        let mut wake_trap = a.create_label();
        a.mov(ecx, dword_ptr(esp + 4)).unwrap();
        a.cmp(byte_ptr(ecx), 4).unwrap();
        a.jne(done).unwrap();
        a.mov(edx, dword_ptr(ecx + 4)).unwrap();
        a.dec(dword_ptr(edx + 4)).unwrap();
        a.je(released).unwrap();
        a.inc(dword_ptr(edx + 0x10)).unwrap();
        a.set_label(&mut done).unwrap();
        a.ret_1(4).unwrap();
        a.set_label(&mut released).unwrap();
        a.mov(dword_ptr(edx + 8), 0).unwrap();
        a.inc(dword_ptr(edx + 0x10)).unwrap();
        a.jg(done).unwrap();
        a.push(edx).unwrap();
        a.call(wake_trap).unwrap();
        a.ret_1(4).unwrap();
        a.set_label(&mut wake_trap).unwrap();
        trap(&mut a, WAKE_ID);
        a.ret_1(4).unwrap();
        assert_eq!(a.assemble(0).unwrap(), LEAVE_CRITICAL_SECTION);
    }
}
