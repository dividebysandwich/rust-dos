//! Kernel objects and the handles of them: what CloseHandle,
//! DuplicateHandle and the wait functions work on. The objects are the
//! system's, shared by its processes (`Objects`); each process has its own
//! handles of them (`Handles`). An object lives while a handle or the
//! system holds it (`refs`). A handle is its index in the process's table,
//! times four from 4 on, as Windows 95's are small multiples of four.
//!
//! Events, mutexes, semaphores and file mappings may have a name, which
//! the Open functions and a second Create find them by, in one namespace
//! for all kinds and processes.

use crate::cpu::Cpu;

/// The pseudo-handles of the current process and thread, which only mean
/// something to the thread that uses them. Windows 95's process one is
/// 7FFFFFFFh, where NT's is -1.
pub const CURRENT_PROCESS: u32 = 0x7FFF_FFFF;
pub const CURRENT_THREAD: u32 = 0xFFFF_FFFE;

/// What an object is, and its state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A DOS file handle: the console's, or a file opened through DOS.
    Dos { handle: u16 },
    /// Signaled until a wait takes it (auto reset), or until ResetEvent
    /// (manual).
    Event { manual: bool, signaled: bool },
    /// Owned by the thread `owner` (0 when free) `count` times; abandoned
    /// when that thread ended without releasing it.
    Mutex {
        owner: u32,
        count: u32,
        abandoned: bool,
    },
    Semaphore { count: u32, max: u32 },
    /// The thread with the id `id`, and its exit code once it ended,
    /// which signals it.
    Thread { id: u32, exit_code: Option<u32> },
    /// The process with the id `id`, the same.
    Process { id: u32, exit_code: Option<u32> },
    /// A file mapping: its memory, `Win32::sections[section]`.
    Mapping { section: u32 },

    /// A file or device CreateFileA opened through DOS: its entry in DOS's
    /// file table, which it holds until the object goes, and the access it
    /// was opened for (GENERIC_READ, GENERIC_WRITE) and the sharing it
    /// allows (FILE_SHARE_READ, FILE_SHARE_WRITE); the ranges LockFile
    /// locked, as (offset, length).
    File {
        sft: u16,
        access: u32,
        share: u32,
        locks: Vec<(u32, u32)>,
    },
    /// A search of FindFirstFileA: the entries it has still to give.
    Find { entries: Vec<FindEntry> },
}

/// A file or directory a search found, as WIN32_FIND_DATA has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindEntry {
    pub attributes: u32,
    /// Its DOS date and time.
    pub date: u16,
    pub time: u16,
    pub size: u32,
    pub name: String,
    /// Its 8.3 name when that isn't `name`, or empty.
    pub short: String,
}

impl Default for Kind {
    fn default() -> Self {
        Kind::Event {
            manual: false,
            signaled: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Object {
    pub kind: Kind,
    pub name: Option<String>,
    /// The handles of it, and the system's holds (a thread's while it
    /// runs, a wait's).
    pub refs: u32,
}

/// The system's objects.
#[derive(Clone, Debug, Default)]
pub struct Objects {
    pub objects: Vec<Option<Object>>,
    /// The DOS file table entries of files whose objects went, which
    /// `close_files` closes.
    pub closed: Vec<u16>,
}

impl Objects {
    /// A new object, held once (by the handle to come, usually).
    pub fn add(&mut self, kind: Kind, name: Option<String>) -> u32 {
        let object = Object {
            kind,
            name,
            refs: 1,
        };
        match self.objects.iter().position(Option::is_none) {
            Some(i) => {
                self.objects[i] = Some(object);
                i as u32
            }
            None => {
                self.objects.push(Some(object));
                self.objects.len() as u32 - 1
            }
        }
    }

    pub fn get(&self, id: u32) -> Option<&Object> {
        self.objects.get(id as usize)?.as_ref()
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Object> {
        self.objects.get_mut(id as usize)?.as_mut()
    }

    pub fn hold(&mut self, id: u32) {
        if let Some(object) = self.get_mut(id) {
            object.refs += 1;
        }
    }

    /// Let go of the object `id` once; it goes when nothing holds it.
    pub fn release(&mut self, id: u32) {
        if let Some(slot) = self.objects.get_mut(id as usize)
            && let Some(object) = slot
        {
            object.refs = object.refs.saturating_sub(1);
            if object.refs == 0 {
                if let Kind::File { sft, .. } = object.kind {
                    self.closed.push(sft);
                }
                *slot = None;
            }
        }
    }

    /// The object with the name `name`, whatever its kind.
    pub fn named(&self, name: &str) -> Option<u32> {
        self.objects
            .iter()
            .position(|o| o.as_ref().is_some_and(|o| o.name.as_deref() == Some(name)))
            .map(|i| i as u32)
    }
}

/// A process's handles.
#[derive(Clone, Debug, Default)]
pub struct Handles {
    /// Each handle's object, by its index.
    handles: Vec<Option<u32>>,
}

impl Handles {
    /// A new handle of the object `id`, which it holds (the caller has
    /// counted the hold).
    pub fn add(&mut self, id: u32) -> u32 {
        let i = match self.handles.iter().position(Option::is_none) {
            Some(i) => {
                self.handles[i] = Some(id);
                i
            }
            None => {
                self.handles.push(Some(id));
                self.handles.len() - 1
            }
        };
        (i as u32 + 1) * 4
    }

    /// The handle `handle` of the object `id`, as a process inherits it,
    /// if that handle is free (the caller has counted the hold).
    pub fn add_at(&mut self, handle: u32, id: u32) -> bool {
        if !handle.is_multiple_of(4) || handle == 0 {
            return false;
        }
        let i = (handle / 4 - 1) as usize;
        if self.handles.len() <= i {
            self.handles.resize(i + 1, None);
        }
        if self.handles[i].is_some() {
            return false;
        }
        self.handles[i] = Some(id);
        true
    }

    /// The object of the handle `handle` (not a pseudo-handle).
    pub fn of(&self, handle: u32) -> Option<u32> {
        if !handle.is_multiple_of(4) || handle == 0 {
            return None;
        }
        self.handles.get((handle / 4 - 1) as usize).copied().flatten()
    }

    /// Take the handle away: its object, whose hold the caller lets go
    /// of. None if there is no such handle.
    pub fn remove(&mut self, handle: u32) -> Option<u32> {
        let id = self.of(handle)?;
        self.handles[(handle / 4 - 1) as usize] = None;
        Some(id)
    }

    /// The handles there are, with their objects.
    pub fn iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.handles
            .iter()
            .enumerate()
            .filter_map(|(i, h)| h.map(|id| ((i as u32 + 1) * 4, id)))
    }
}

/// The system's objects.
pub fn objects(cpu: &mut Cpu) -> &mut Objects {
    &mut cpu.bus.win32.objects
}

/// A new handle in the running process of the object `id`, which it
/// holds.
pub fn new_handle(cpu: &mut Cpu, id: u32) -> u32 {
    super::process(cpu).handles.add(id)
}

/// A new object of `kind`, and a handle of it in the running process.
pub fn create(cpu: &mut Cpu, kind: Kind, name: Option<String>) -> u32 {
    let id = objects(cpu).add(kind, name);
    new_handle(cpu, id)
}

/// Close the running process's handle `handle`. False if there is none.
pub fn close(cpu: &mut Cpu, handle: u32) -> bool {
    match super::process(cpu).handles.remove(handle) {
        Some(id) => {
            objects(cpu).release(id);
            close_files(&mut cpu.bus);
            true
        }
        None => false,
    }
}

/// Close in DOS the files whose objects went.
pub fn close_files(bus: &mut crate::bus::Bus) {
    for sft in std::mem::take(&mut bus.win32.objects.closed) {
        bus.disk.close_file(sft);
    }
}

/// Close in DOS the files of all objects, which go with the last
/// process.
pub fn close_all_files(bus: &mut crate::bus::Bus) {
    let objects = &mut bus.win32.objects;
    for object in objects.objects.iter().flatten() {
        if let Kind::File { sft, .. } = object.kind {
            objects.closed.push(sft);
        }
    }
    close_files(bus);
}

/// The object a handle of the running thread's names: its process's
/// handles, and the pseudo-handles of itself and its process.
pub fn resolve(cpu: &mut Cpu, handle: u32) -> Option<u32> {
    let process = super::process(cpu);
    match handle {
        CURRENT_PROCESS => Some(process.object),
        CURRENT_THREAD => process.current_thread().map(|t| t.object),
        _ => process.handles.of(handle),
    }
}

/// The kind of the object `handle` names, if it is one.
pub fn kind_of(cpu: &mut Cpu, handle: u32) -> Option<Kind> {
    let id = resolve(cpu, handle)?;
    Some(objects(cpu).get(id)?.kind.clone())
}

crate::win32::state_variants!(Kind {
    Dos { handle },
    Event { manual, signaled },
    Mutex {
        owner,
        count,
        abandoned
    },
    Semaphore { count, max },
    Thread { id, exit_code },
    Process { id, exit_code },
    Mapping { section },

    File {
        sft,
        access,
        share,
        locks
    },
    Find { entries },
});
crate::state_fields!(FindEntry {
    attributes,
    date,
    time,
    size,
    name,
    short
});
crate::state_fields!(Object { kind, name, refs });
crate::state_fields!(Objects { objects, closed });
crate::state_fields!(Handles { handles });
