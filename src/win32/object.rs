//! Kernel objects and the handles of them: what CloseHandle,
//! DuplicateHandle and the wait functions work on. An object lives while
//! a handle or the system holds it (`refs`); a handle is its index in the
//! process's table, times four from 4 on, as Windows 95's are small
//! multiples of four.
//!
//! Events, mutexes and semaphores may have a name, which the Open
//! functions and a second Create find them by, in one namespace for all
//! kinds.

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
    /// The thread at `index` in the process's list.
    Thread { index: u32 },
    /// The process.
    Process {},
}

impl Default for Kind {
    fn default() -> Self {
        Kind::Process {}
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

/// The objects and the handles of them.
#[derive(Clone, Debug, Default)]
pub struct Objects {
    pub objects: Vec<Option<Object>>,
    /// Each handle's object, by its index.
    handles: Vec<Option<u32>>,
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
                *slot = None;
            }
        }
    }

    /// A new handle of the object `id`, which it holds (the caller has
    /// counted the hold).
    pub fn handle(&mut self, id: u32) -> u32 {
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

    /// The object of the handle `handle` (not a pseudo-handle).
    pub fn of(&self, handle: u32) -> Option<u32> {
        if !handle.is_multiple_of(4) || handle == 0 {
            return None;
        }
        self.handles.get((handle / 4 - 1) as usize).copied().flatten()
    }

    /// Close the handle: its object's hold goes. False if there is none.
    pub fn close(&mut self, handle: u32) -> bool {
        let Some(id) = self.of(handle) else {
            return false;
        };
        self.handles[(handle / 4 - 1) as usize] = None;
        self.release(id);
        true
    }

    /// The object with the name `name`, whatever its kind.
    pub fn named(&self, name: &str) -> Option<u32> {
        self.objects
            .iter()
            .position(|o| o.as_ref().is_some_and(|o| o.name.as_deref() == Some(name)))
            .map(|i| i as u32)
    }

    /// The handles there are, with their objects, for debuggers.
    pub fn handles(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.handles
            .iter()
            .enumerate()
            .filter_map(|(i, h)| h.map(|id| ((i as u32 + 1) * 4, id)))
    }
}

/// The object a handle of the current thread's names: its own handles,
/// and the pseudo-handles of itself and its process.
pub fn resolve(cpu: &mut Cpu, handle: u32) -> Option<u32> {
    let process = super::process(cpu);
    match handle {
        CURRENT_PROCESS => Some(process.object),
        CURRENT_THREAD => process.current_thread().map(|t| t.object),
        _ => process.objects.of(handle),
    }
}

/// The kind of the object `handle` names, if it is one.
pub fn kind_of(cpu: &mut Cpu, handle: u32) -> Option<Kind> {
    let id = resolve(cpu, handle)?;
    Some(super::process(cpu).objects.get(id)?.kind.clone())
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
    Thread { index },
    Process {},
});
crate::state_fields!(Object { kind, name, refs });
crate::state_fields!(Objects { objects, handles });
