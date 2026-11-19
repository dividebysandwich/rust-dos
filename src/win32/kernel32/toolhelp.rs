//! Toolhelp32: CreateToolhelp32Snapshot and the functions that walk what
//! it took, the processes, their threads, modules and heaps, which
//! Windows 95 and 98 have in KERNEL32 (NT 4 hasn't, Windows 2000 has).
//! Memory managers such as SmartHeap walk the heaps with them at start,
//! looking for KERNEL32's exports without checking what they got.
//!
//! The snapshot keeps the lists as they were when it was taken, as
//! Windows does; Heap32First and Heap32Next walk a heap as it is when
//! they are called, as on Windows, where they take no snapshot handle.

use super::process::copy_process_memory;
use super::{ERROR_INVALID_PARAMETER, FALSE, INVALID_HANDLE_VALUE, TRUE, set_error};
use crate::cpu::Cpu;
use crate::win32::object::{self, Kind};
use crate::win32::{Process, process};

/// CreateToolhelp32Snapshot's flags: what it takes.
const TH32CS_SNAPHEAPLIST: u32 = 0x1;
const TH32CS_SNAPPROCESS: u32 = 0x2;
const TH32CS_SNAPTHREAD: u32 = 0x4;
const TH32CS_SNAPMODULE: u32 = 0x8;

/// The lists of a snapshot, in the order of their flags' bits.
const HEAP_LISTS: usize = 0;
const PROCESSES: usize = 1;
const THREADS: usize = 2;
const MODULES: usize = 3;

/// The sizes of the structures, dwSize included.
const HEAPLIST32_SIZE: u32 = 16;
const PROCESSENTRY32_SIZE: u32 = 296;
const THREADENTRY32_SIZE: u32 = 28;
const MODULEENTRY32_SIZE: u32 = 548;
const HEAPENTRY32_SIZE: u32 = 36;

/// HEAPLIST32's dwFlags of the process heap.
const HF32_DEFAULT: u32 = 1;
/// HEAPENTRY32's dwFlags.
const LF32_FIXED: u32 = 1;
const LF32_FREE: u32 = 2;
const LF32_MOVEABLE: u32 = 4;

/// The base priority of a thread of a process of normal priority, which
/// THREADENTRY32's tpBasePri has.
const NORMAL_BASE_PRIORITY: u32 = 8;

const ERROR_NO_MORE_FILES: u32 = 18;
const ERROR_BAD_LENGTH: u32 = 24;

/// The process with the id `id`, 0 for the caller's, as its index.
fn process_index(cpu: &mut Cpu, id: u32) -> Option<usize> {
    let id = match id {
        0 => process(cpu).id,
        id => id,
    };
    cpu.bus.win32.processes.iter().position(|p| p.id == id)
}

/// `text` as a zero-ended field of `len` bytes, cut to fit.
fn text_field(text: &str, len: usize) -> Vec<u8> {
    let mut bytes = crate::dosstr::to_bytes(text);
    bytes.truncate(len - 1);
    bytes.resize(len, 0);
    bytes
}

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// The 1-based number of `process`'s module at `base`, its th32ModuleID.
fn module_id(process: &Process, base: u32) -> u32 {
    process
        .modules
        .iter()
        .position(|m| m.base == base)
        .map_or(0, |i| i as u32 + 1)
}

/// The base priority of the threads of `process`, from its class, as
/// PROCESSENTRY32's pcPriClassBase has it.
fn class_base(process: &Process) -> u32 {
    match process.priority_class {
        0x40 => 4,
        0x80 => 13,
        0x100 => 24,
        _ => NORMAL_BASE_PRIORITY,
    }
}

/// HEAPLIST32 after dwSize, for each heap of `process`.
fn heap_lists(process: &Process) -> Vec<Vec<u8>> {
    std::iter::once((process.heap.base, HF32_DEFAULT))
        .chain(process.heaps.iter().map(|h| (h.base, 0)))
        .map(|(heap, flags)| words(&[process.id, heap, flags]))
        .collect()
}

/// PROCESSENTRY32 after dwSize, for each process.
fn processes(cpu: &Cpu) -> Vec<Vec<u8>> {
    cpu.bus
        .win32
        .processes
        .iter()
        .map(|p| {
            let threads = p.threads.iter().filter(|t| t.exit_code.is_none()).count();
            let path = p
                .modules
                .iter()
                .find(|m| m.base == p.image_base)
                .map_or(p.path.as_str(), |m| m.path.as_str());
            let mut bytes = words(&[
                1,
                p.id,
                p.heap.base,
                module_id(p, p.image_base),
                threads as u32,
                p.parent,
                class_base(p),
                0,
            ]);
            bytes.extend(text_field(path, 260));
            bytes
        })
        .collect()
}

/// THREADENTRY32 after dwSize, for each thread that hasn't ended, of
/// every process, as Windows 9x lists them whatever the process asked.
fn threads(cpu: &Cpu) -> Vec<Vec<u8>> {
    cpu.bus
        .win32
        .processes
        .iter()
        .flat_map(|p| {
            p.threads
                .iter()
                .filter(|t| t.exit_code.is_none())
                .map(move |t| words(&[1, t.id, p.id, class_base(p), t.priority as u32, 0]))
        })
        .collect()
}

/// MODULEENTRY32 after dwSize, for each module of `process`.
fn modules(process: &Process) -> Vec<Vec<u8>> {
    process
        .modules
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let uses = m.refs.max(1);
            let mut bytes = words(&[i as u32 + 1, process.id, uses, uses, m.base, m.size, m.base]);
            bytes.extend(text_field(&m.name, 256));
            bytes.extend(text_field(&m.path, 260));
            bytes
        })
        .collect()
}

/// CreateToolhelp32Snapshot(flags, process): a handle of what the flags
/// ask for as it is now: the heaps and modules of the process (0 for the
/// caller), and every process and thread. INVALID_HANDLE_VALUE for a
/// process there isn't.
pub(super) fn create_toolhelp32_snapshot(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[flags, id] = args else { unreachable!() };
    let mut lists = vec![Vec::new(); 4];
    if flags & (TH32CS_SNAPHEAPLIST | TH32CS_SNAPMODULE) != 0 {
        let Some(p) = process_index(cpu, id) else {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            return Some(INVALID_HANDLE_VALUE);
        };
        let process = &cpu.bus.win32.processes[p];
        if flags & TH32CS_SNAPHEAPLIST != 0 {
            lists[HEAP_LISTS] = heap_lists(process);
        }
        if flags & TH32CS_SNAPMODULE != 0 {
            lists[MODULES] = modules(process);
        }
    }
    if flags & TH32CS_SNAPPROCESS != 0 {
        lists[PROCESSES] = processes(cpu);
    }
    if flags & TH32CS_SNAPTHREAD != 0 {
        lists[THREADS] = threads(cpu);
    }
    let kind = Kind::Snapshot {
        lists,
        next: vec![0; 4],
    };
    Some(object::create(cpu, kind, None))
}

/// The First (`first`) or Next function of the list `list`: the
/// snapshot's next entry of it into the structure at `at`, whose dwSize
/// must be at least `size`. ERROR_NO_MORE_FILES at the list's end.
fn walk(cpu: &mut Cpu, args: &[u32], list: usize, size: u32, first: bool) -> Option<u32> {
    let &[handle, at] = args else { unreachable!() };
    let dw_size = cpu.bus.guest_read_32(at);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let id = object::resolve(cpu, handle);
    let Some(object::Object {
        kind: Kind::Snapshot { lists, next },
        ..
    }) = id.and_then(|id| object::objects(cpu).get_mut(id))
    else {
        set_error(cpu, super::ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    if dw_size < size {
        set_error(cpu, ERROR_BAD_LENGTH);
        return Some(FALSE);
    }
    let index = if first { 0 } else { next[list] as usize };
    let entry = lists[list].get(index).cloned();
    if entry.is_some() {
        next[list] = index as u32 + 1;
    }
    let Some(entry) = entry else {
        set_error(cpu, ERROR_NO_MORE_FILES);
        return Some(FALSE);
    };
    cpu.bus.guest_write_bytes(at + 4, &entry);
    if cpu.bus.guest_faulted() {
        return None;
    }
    Some(TRUE)
}

/// Heap32ListFirst(snapshot, list) and Heap32ListNext: the heaps.
pub(super) fn heap32_list_first(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    walk(cpu, args, HEAP_LISTS, HEAPLIST32_SIZE, true)
}

pub(super) fn heap32_list_next(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    walk(cpu, args, HEAP_LISTS, HEAPLIST32_SIZE, false)
}

/// Process32First(snapshot, entry) and Process32Next: the processes.
pub(super) fn process32_first(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    walk(cpu, args, PROCESSES, PROCESSENTRY32_SIZE, true)
}

pub(super) fn process32_next(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    walk(cpu, args, PROCESSES, PROCESSENTRY32_SIZE, false)
}

/// Thread32First(snapshot, entry) and Thread32Next: the threads.
pub(super) fn thread32_first(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    walk(cpu, args, THREADS, THREADENTRY32_SIZE, true)
}

pub(super) fn thread32_next(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    walk(cpu, args, THREADS, THREADENTRY32_SIZE, false)
}

/// Module32First(snapshot, entry) and Module32Next: the modules.
pub(super) fn module32_first(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    walk(cpu, args, MODULES, MODULEENTRY32_SIZE, true)
}

pub(super) fn module32_next(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    walk(cpu, args, MODULES, MODULEENTRY32_SIZE, false)
}

/// The piece of the heap `heap` of the process `id` after `after` (the
/// first one for None), as HEAPENTRY32 after dwSize; None at the heap's
/// end, Err(()) when there is no such process or heap.
fn heap_entry(
    cpu: &mut Cpu,
    id: u32,
    heap: u32,
    after: Option<u32>,
) -> Result<Option<Vec<u8>>, ()> {
    let p = process_index(cpu, id).ok_or(())?;
    let process = &cpu.bus.win32.processes[p];
    let h = if process.heap.base == heap {
        &process.heap
    } else {
        process.heaps.iter().find(|h| h.base == heap).ok_or(())?
    };
    let Some((address, size, free)) = h.piece_after(after) else {
        return Ok(None);
    };
    let moveable = (!free && heap == process.heap.base)
        .then(|| process.moveables.iter().find(|m| m.block == address))
        .flatten();
    let (handle, flags, locks) = match (free, moveable) {
        (true, _) => (address, LF32_FREE, 0),
        (false, Some(m)) => (m.handle, LF32_MOVEABLE, m.locks),
        (false, None) => (address, LF32_FIXED, 0),
    };
    // dwResvd, Windows' own place in the walk, stays 0: Heap32Next goes
    // on from dwAddress.
    let entry = words(&[handle, address, size, flags, locks, 0, process.id, heap]);
    Ok(Some(entry))
}

/// Write the piece `entry` of a heap walk into the HEAPENTRY32 at `at`,
/// whose dwSize was checked, or fail with `ERROR_NO_MORE_FILES` at the
/// end and ERROR_INVALID_PARAMETER for no such heap.
fn put_heap_entry(cpu: &mut Cpu, at: u32, entry: Result<Option<Vec<u8>>, ()>) -> Option<u32> {
    match entry {
        Ok(Some(entry)) => {
            cpu.bus.guest_write_bytes(at + 4, &entry);
            if cpu.bus.guest_faulted() {
                return None;
            }
            Some(TRUE)
        }
        Ok(None) => {
            set_error(cpu, ERROR_NO_MORE_FILES);
            Some(FALSE)
        }
        Err(()) => {
            set_error(cpu, ERROR_INVALID_PARAMETER);
            Some(FALSE)
        }
    }
}

/// HEAPENTRY32's dwSize at `at`, if it is large enough; else the error
/// is set.
fn heap_entry_size_ok(cpu: &mut Cpu, at: u32) -> Option<bool> {
    let size = cpu.bus.guest_read_32(at);
    if cpu.bus.guest_faulted() {
        return None;
    }
    if size < HEAPENTRY32_SIZE {
        set_error(cpu, ERROR_BAD_LENGTH);
        return Some(false);
    }
    Some(true)
}

/// Heap32First(entry, process, heap): the first piece of the heap, a
/// block given out or a free gap, as the heap is now.
pub(super) fn heap32_first(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[at, id, heap] = args else {
        unreachable!()
    };
    if !heap_entry_size_ok(cpu, at)? {
        return Some(FALSE);
    }
    let entry = heap_entry(cpu, id, heap, None);
    put_heap_entry(cpu, at, entry)
}

/// Heap32Next(entry): the piece after the one in `entry`, of the process
/// and heap it names, as the heap is now.
pub(super) fn heap32_next(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let at = args[0];
    if !heap_entry_size_ok(cpu, at)? {
        return Some(FALSE);
    }
    let mut fields = [0u8; HEAPENTRY32_SIZE as usize];
    cpu.bus.guest_read_bytes(at, &mut fields);
    if cpu.bus.guest_faulted() {
        return None;
    }
    let field = |i: usize| u32::from_le_bytes(fields[4 * i..4 * i + 4].try_into().unwrap());
    let (address, id, heap) = (field(2), field(7), field(8));
    let entry = heap_entry(cpu, id, heap, Some(address));
    put_heap_entry(cpu, at, entry)
}

/// Toolhelp32ReadProcessMemory(process id, address, buffer, size, read):
/// ReadProcessMemory with the process's id (0 for the caller's) in place
/// of a handle.
pub(super) fn toolhelp32_read_process_memory(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let &[id, address, buffer, size, done_at] = args else {
        unreachable!()
    };
    let Some(p) = process_index(cpu, id) else {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    };
    copy_process_memory(cpu, p, [address, buffer, size, done_at], false)
}
