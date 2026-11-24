//! The current drive and directories of each program.
//!
//! On Windows 3.1, 95 and 98 every Win16 task keeps a current drive and
//! directory of its own (in its task database), and every Win32 process
//! keeps one current directory, which its threads share. A program
//! starts with those of the program that started it, or the folder
//! CreateProcessA's lpCurrentDirectory names, and what it changes later
//! is its own: a setup that moves to C: while it runs leaves the program
//! that started it on the drive it was on.
//!
//! The built-in DOS has one current drive and one current directory for
//! each drive (`DiskController`), which DOS's services and the file
//! functions of both kinds of programs go by. So the system keeps the
//! directories of each program that isn't running in its entry
//! (`Process::directories`, `Task16::directories`) and puts them in DOS
//! when one of its threads runs (`switch`), as Windows 3.1's KERNEL does
//! with the task database at a task switch. `State::owner` says whose
//! are in DOS now.
//!
//! The directories DOS had when the session started are DOS's own: the
//! prompt has them back when the session's last process ends, as it
//! does when a Windows program started from a DOS box on Windows 98
//! ends, whatever that program changed.

use super::{Win32, kernel32};
use crate::cpu::Cpu;

/// The W32 section's version that added each program's directories.
pub const DIRECTORIES_SAVED: u16 = 88;

/// DOS's current drive and each drive's current directory, and the long
/// names SetCurrentDirectoryA was given for them (`Win32::long_dirs`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Directories {
    current: (u8, Vec<(u8, String)>),
    long: Vec<(String, String)>,
}

impl Directories {
    /// DOS's current drive and directories as they are now.
    pub fn now(cpu: &Cpu) -> Directories {
        Directories {
            current: cpu.bus.disk.current_directories(),
            long: cpu.bus.win32.long_dirs.clone(),
        }
    }

    /// Those DOS has now, with the directory `path` and its drive the
    /// current ones, as SetCurrentDirectoryA makes them; or the error
    /// code when `path` isn't a directory. DOS's stay as they are.
    pub fn with_directory(cpu: &mut Cpu, path: &str) -> Result<Directories, u32> {
        let before = Directories::now(cpu);
        let changed = kernel32::change_directory(cpu, path).map(|()| Directories::now(cpu));
        put_back(cpu, before);
        changed
    }
}

/// Make `directories` DOS's current ones, in its tables too.
pub fn put_back(cpu: &mut Cpu, directories: Directories) {
    cpu.bus.disk.set_current_directories(&directories.current);
    cpu.bus.win32.long_dirs = directories.long;
    super::dos(cpu, |cpu| {
        for &(drive, _) in &directories.current.1 {
            crate::dos_data::write_cds(&mut cpu.bus, drive);
        }
    });
}

/// Whose directories DOS has, and DOS's own while the session runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// The id of the Win32 process or the thread of the Win16 task whose
    /// directories DOS has (`owner`), or 0 for DOS's own.
    owner: u32,
    /// DOS's own, kept while a program's are in DOS.
    dos: Option<Directories>,
}

/// Whose directories the thread `t` of the process `p` works in: a Win16
/// task's own, by its thread's id; for a task that runs a Win32
/// process's 16-bit calls (win16/flat.rs), that process's; for any other
/// thread, its process's, by the process's id.
pub fn owner_of(win32: &Win32, p: usize, t: usize) -> u32 {
    let process = &win32.processes[p];
    let tid = process.threads[t].id;
    if process.id != win32.win16.pid {
        return process.id;
    }
    if let Some(client) = win32.win16.clients.iter().find(|c| c.tid == tid) {
        return client.pid;
    }
    if crate::win16::is_task(win32, tid) {
        return tid;
    }
    process.id
}

/// The place an owner (`State::owner`) keeps its directories in while
/// they aren't DOS's: None for one that has ended.
fn kept(win32: &mut Win32, owner: u32) -> Option<&mut Option<Directories>> {
    if owner == 0 {
        return Some(&mut win32.directories.dos);
    }
    if let Some(process) = win32.processes.iter_mut().find(|p| p.id == owner) {
        return Some(&mut process.directories.0);
    }
    let task = win32.win16.tasks.iter_mut().find(|k| k.tid == owner)?;
    Some(&mut task.directories.0)
}

/// A thread of `owner` (`owner_of`) is to run: DOS's directories become
/// its, and the ones DOS had go back to their owner. An owner with none
/// kept (one from a state saved before programs had their own) takes
/// those DOS has.
pub fn switch(cpu: &mut Cpu, owner: u32) {
    let from = cpu.bus.win32.directories.owner;
    if from == owner {
        return;
    }
    let now = Directories::now(cpu);
    let win32 = &mut cpu.bus.win32;
    win32.directories.owner = owner;
    if let Some(place) = kept(win32, from) {
        *place = Some(now.clone());
    }
    let Some(next) = kept(win32, owner).and_then(Option::take) else {
        return;
    };
    if next != now {
        put_back(cpu, next);
    }
}

/// A session starts: DOS's directories are DOS's own.
pub fn session_starts(win32: &mut Win32) {
    *win32.directories = State::default();
}

/// The session's last process ended: DOS has its own directories back.
pub fn session_ended(cpu: &mut Cpu) {
    let state = std::mem::take(&mut *cpu.bus.win32.directories);
    if state.owner == 0 {
        return;
    }
    if let Some(dos) = state.dos {
        put_back(cpu, dos);
    }
}

crate::state_fields!(Directories { current, long });
crate::state_fields!(State { owner, dos });
