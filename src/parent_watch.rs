//! `--exit-with-parent`: rust-dos quits when the process that started it,
//! or another one it names, ends.
//!
//! A script that drives rust-dos through the debug server and dies before
//! it sends `POST /api/control/quit` would otherwise leave the emulator
//! running with nobody driving it. The watch takes a handle on the process
//! at start (a process handle on Windows, a pidfd on Linux, a kqueue event
//! on macOS), so a process that later gets the same ID doesn't keep
//! rust-dos alive. A thread waits on the handle and raises a flag, and the
//! main loop quits as `/api/control/quit` does when it sees the flag: the
//! log, the journal and the recordings get finished, which a kill from
//! outside (a job object, `PR_SET_PDEATHSIG`) doesn't allow.
//!
//! The flag goes up only when the process has ended. A wait that fails is
//! tried again, or the process is looked at every so often instead, so a
//! failure of the watch doesn't end the emulator under a process that
//! still runs.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The exit code of a rust-dos that quit because the watched process
/// ended.
pub const EXIT_CODE: u8 = 4;

/// The process to watch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    /// The process that started rust-dos.
    Parent,
    /// A process named by its ID.
    Pid(u32),
}

/// Why the watch didn't start.
#[derive(Debug, PartialEq)]
pub enum StartError {
    /// The process had ended already: rust-dos quits at once.
    Gone(u32),
    /// The process can't be watched (no such process is not this: that is
    /// `Gone`). rust-dos doesn't start without the watch it was asked for.
    Failed(String),
}

/// A process being watched.
pub struct ParentWatch {
    pid: u32,
    gone: Arc<AtomicBool>,
}

impl ParentWatch {
    /// Starts watching `target`.
    pub fn start(target: Target) -> Result<ParentWatch, StartError> {
        let gone = Arc::new(AtomicBool::new(false));
        let pid = imp::watch(target, gone.clone())?;
        Ok(ParentWatch { pid, gone })
    }

    /// The ID of the process watched.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Whether the process has ended.
    pub fn gone(&self) -> bool {
        self.gone.load(Ordering::Acquire)
    }

    /// The flag that goes up when the process ends, for a wait elsewhere
    /// (the run lock's) to stop on.
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.gone.clone()
    }
}

/// Starts the thread that waits for the process, and raises `gone` when
/// it ends. `wait` returns only once the process has ended.
fn spawn_waiter(
    pid: u32,
    gone: Arc<AtomicBool>,
    wait: impl FnOnce() + Send + 'static,
) -> Result<(), StartError> {
    std::thread::Builder::new()
        .name(format!("exit-with-parent {pid}"))
        .spawn(move || {
            wait();
            gone.store(true, Ordering::Release);
        })
        .map(drop)
        .map_err(|e| {
            StartError::Failed(format!(
                "can't start the thread that watches process {pid}: {e}"
            ))
        })
}

/// The ID of the process that started this one, or of the one that took
/// it on when that ended.
#[cfg(unix)]
fn parent_id() -> u32 {
    // SAFETY: getppid takes nothing and can't fail.
    unsafe { libc::getppid() as u32 }
}

#[cfg(windows)]
mod imp {
    use super::{StartError, Target, spawn_waiter};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{
        ERROR_INVALID_PARAMETER, FILETIME, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
        WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, INFINITE, OpenProcess,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    pub(super) fn watch(target: Target, gone: Arc<AtomicBool>) -> Result<u32, StartError> {
        let pid = match target {
            Target::Pid(pid) => pid,
            Target::Parent => parent_pid()?,
        };
        // SAFETY: OpenProcess takes no pointers; a null handle is a failure.
        let handle = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        if handle.is_null() {
            // SAFETY: reads the calling thread's last error.
            let error = unsafe { GetLastError() };
            // No process has the ID (any more).
            if error == ERROR_INVALID_PARAMETER {
                return Err(StartError::Gone(pid));
            }
            return Err(StartError::Failed(format!(
                "can't watch process {pid}: {}",
                std::io::Error::from_raw_os_error(error as i32)
            )));
        }
        // SAFETY: the handle is open, and nothing else owns it.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        // The process ended, and something still holds a handle on it.
        // SAFETY: the handle is open and has SYNCHRONIZE.
        if unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) } == WAIT_OBJECT_0 {
            return Err(StartError::Gone(pid));
        }
        // Windows keeps a process's parent ID after the parent ends, and
        // may give the ID to a new process: one started after rust-dos
        // isn't its parent.
        if target == Target::Parent {
            // SAFETY: GetCurrentProcess returns a pseudo handle that needs
            // no closing.
            let ours = created(unsafe { GetCurrentProcess() });
            if let (Some(theirs), Some(ours)) = (created(handle.as_raw_handle()), ours)
                && theirs > ours
            {
                return Err(StartError::Gone(pid));
            }
        }
        spawn_waiter(pid, gone, move || {
            // A wait that fails is tried again rather than taken for the
            // process's end.
            // SAFETY: `handle` keeps the handle open.
            while unsafe { WaitForSingleObject(handle.as_raw_handle(), INFINITE) } != WAIT_OBJECT_0
            {
                std::thread::sleep(Duration::from_millis(200));
            }
        })?;
        Ok(pid)
    }

    /// When the process was created, in 100 ns units.
    fn created(process: HANDLE) -> Option<u64> {
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: the handle has PROCESS_QUERY_LIMITED_INFORMATION (or is
        // the current process's), and the pointers are to locals.
        let ok =
            unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) };
        (ok != 0).then_some((creation.dwHighDateTime as u64) << 32 | creation.dwLowDateTime as u64)
    }

    /// The ID of the process that started this one, from a snapshot of the
    /// processes.
    fn parent_pid() -> Result<u32, StartError> {
        // SAFETY: CreateToolhelp32Snapshot takes no pointers.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(StartError::Failed(format!(
                "can't find the process that started rust-dos: {}",
                std::io::Error::last_os_error()
            )));
        }
        // SAFETY: the snapshot is open, and nothing else owns it.
        let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
        // SAFETY: takes no pointers.
        let me = unsafe { GetCurrentProcessId() };
        // SAFETY: PROCESSENTRY32W is plain data, for which zero is valid.
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        // SAFETY: the snapshot is open and the entry's size is set.
        let mut more = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } != 0;
        while more {
            if entry.th32ProcessID == me {
                return Ok(entry.th32ParentProcessID);
            }
            // SAFETY: as for Process32FirstW.
            more = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } != 0;
        }
        Err(StartError::Failed(
            "can't find the process that started rust-dos".into(),
        ))
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{StartError, Target, spawn_waiter};
    use std::io::{Error, ErrorKind};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    pub(super) fn watch(target: Target, gone: Arc<AtomicBool>) -> Result<u32, StartError> {
        let pid = match target {
            Target::Pid(pid) => pid,
            Target::Parent => super::parent_id(),
        };
        // SAFETY: pidfd_open takes a PID and flags; it returns a new file
        // descriptor or -1.
        let fd =
            unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) } as libc::c_int;
        if fd < 0 {
            let error = Error::last_os_error();
            return match error.raw_os_error() {
                Some(libc::ESRCH) => Err(StartError::Gone(pid)),
                // Linux before 5.3 has no pidfd_open (ENOSYS), and a
                // seccomp filter that doesn't know the call, as older
                // container runtimes have, refuses it with EPERM, which
                // pidfd_open itself never returns: the process is looked
                // at every so often instead.
                Some(libc::ENOSYS | libc::EPERM) => super::poll::watch(target, pid, gone),
                _ => Err(StartError::Failed(format!(
                    "can't watch process {pid}: {error}"
                ))),
            };
        }
        // SAFETY: the descriptor is new, and nothing else owns it.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // The parent ended before the pidfd was taken, and the ID went to
        // another process: rust-dos has a new parent by now.
        if target == Target::Parent && super::parent_id() != pid {
            return Err(StartError::Gone(pid));
        }
        spawn_waiter(pid, gone, move || {
            let mut poll = libc::pollfd {
                fd: fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // The pidfd becomes readable when the process ends.
            // SAFETY: one pollfd, which lives on this stack, for a
            // descriptor `fd` keeps open.
            while unsafe { libc::poll(&mut poll, 1, -1) } < 0 {
                if Error::last_os_error().kind() != ErrorKind::Interrupted {
                    // The wait failed, which isn't the process's end.
                    super::poll::wait(target, pid);
                    break;
                }
            }
        })?;
        Ok(pid)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{StartError, Target, spawn_waiter};
    use std::io::{Error, ErrorKind};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    pub(super) fn watch(target: Target, gone: Arc<AtomicBool>) -> Result<u32, StartError> {
        let pid = match target {
            Target::Pid(pid) => pid,
            Target::Parent => super::parent_id(),
        };
        // SAFETY: kqueue takes nothing and returns a descriptor or -1.
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return Err(StartError::Failed(format!(
                "can't watch process {pid}: {}",
                Error::last_os_error()
            )));
        }
        // SAFETY: the descriptor is new, and nothing else owns it.
        let kq = unsafe { OwnedFd::from_raw_fd(kq) };
        // SAFETY: kevent is plain data, for which zero is valid.
        let mut change: libc::kevent = unsafe { std::mem::zeroed() };
        change.ident = pid as usize;
        change.filter = libc::EVFILT_PROC;
        change.flags = libc::EV_ADD | libc::EV_ONESHOT;
        change.fflags = libc::NOTE_EXIT;
        // SAFETY: one change, no events asked for, no timeout.
        let added = unsafe {
            libc::kevent(
                kq.as_raw_fd(),
                &change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        if added < 0 {
            let error = Error::last_os_error();
            return match error.raw_os_error() {
                Some(libc::ESRCH) => Err(StartError::Gone(pid)),
                _ => Err(StartError::Failed(format!(
                    "can't watch process {pid}: {error}"
                ))),
            };
        }
        // The parent ended before the event was added, and the ID went to
        // another process: rust-dos has a new parent by now.
        if target == Target::Parent && super::parent_id() != pid {
            return Err(StartError::Gone(pid));
        }
        spawn_waiter(pid, gone, move || {
            // SAFETY: as above.
            let mut event: libc::kevent = unsafe { std::mem::zeroed() };
            loop {
                // SAFETY: no changes, room for one event, no timeout, on a
                // descriptor `kq` keeps open.
                let n = unsafe {
                    libc::kevent(
                        kq.as_raw_fd(),
                        std::ptr::null(),
                        0,
                        &mut event,
                        1,
                        std::ptr::null(),
                    )
                };
                if n > 0 {
                    break;
                }
                if Error::last_os_error().kind() != ErrorKind::Interrupted {
                    // The wait failed, which isn't the process's end.
                    super::poll::wait(target, pid);
                    break;
                }
            }
        })?;
        Ok(pid)
    }
}

/// Looking at the process every so often. Unix systems other than Linux
/// and macOS have no handle on a process here, and Linux has none before
/// 5.3; the others fall back on it when their wait fails.
#[cfg(unix)]
mod poll {
    use std::io::Error;
    use std::time::Duration;

    use super::Target;
    #[cfg(not(target_os = "macos"))]
    use super::{StartError, spawn_waiter};
    #[cfg(not(target_os = "macos"))]
    use std::sync::Arc;
    #[cfg(not(target_os = "macos"))]
    use std::sync::atomic::AtomicBool;

    /// How often the process is looked at. rust-dos may quit up to this
    /// long after the process ends, and it seldom wakes up while it waits.
    const INTERVAL: Duration = Duration::from_secs(3);

    /// Whether the process is still there. For the parent, rust-dos gets a
    /// new one when it ends, which a reused ID doesn't hide; a named
    /// process's ID may be reused unnoticed.
    fn alive(target: Target, pid: u32) -> bool {
        match target {
            Target::Parent => super::parent_id() == pid,
            Target::Pid(_) => {
                // SAFETY: signal 0 sends nothing, it only checks the
                // process.
                let sent = unsafe { libc::kill(pid as libc::pid_t, 0) };
                sent == 0 || Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
            }
        }
    }

    /// Returns once the process has ended.
    pub(super) fn wait(target: Target, pid: u32) {
        while alive(target, pid) {
            std::thread::sleep(INTERVAL);
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn watch(
        target: Target,
        pid: u32,
        gone: Arc<AtomicBool>,
    ) -> Result<u32, StartError> {
        if !alive(target, pid) {
            return Err(StartError::Gone(pid));
        }
        spawn_waiter(pid, gone, move || wait(target, pid))?;
        Ok(pid)
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
mod imp {
    use super::{StartError, Target};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    pub(super) fn watch(target: Target, gone: Arc<AtomicBool>) -> Result<u32, StartError> {
        let pid = match target {
            Target::Pid(pid) => pid,
            Target::Parent => super::parent_id(),
        };
        super::poll::watch(target, pid, gone)
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    use super::{StartError, Target};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    pub(super) fn watch(_target: Target, _gone: Arc<AtomicBool>) -> Result<u32, StartError> {
        Err(StartError::Failed(
            "--exit-with-parent isn't supported on this system".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};

    /// A process that runs for half a minute unless killed.
    fn sleeper() -> Child {
        #[cfg(windows)]
        let mut command = {
            let mut c = Command::new("ping");
            c.args(["-n", "30", "127.0.0.1"]);
            c
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut c = Command::new("sleep");
            c.arg("30");
            c
        };
        command
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("start a process to watch")
    }

    #[test]
    fn a_named_process_that_ends_raises_the_flag() {
        let mut child = sleeper();
        let watch = ParentWatch::start(Target::Pid(child.id())).expect("watch");
        assert_eq!(watch.pid(), child.id());
        std::thread::sleep(Duration::from_millis(100));
        assert!(!watch.gone(), "the process still runs");
        child.kill().expect("kill");
        child.wait().expect("wait");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !watch.gone() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(watch.gone(), "the flag went up when the process ended");
    }

    #[test]
    fn a_process_already_gone_is_reported_at_start() {
        let mut child = sleeper();
        child.kill().expect("kill");
        child.wait().expect("wait");
        match ParentWatch::start(Target::Pid(child.id())) {
            Err(StartError::Gone(pid)) => assert_eq!(pid, child.id()),
            Err(StartError::Failed(e)) => panic!("expected Gone, got Failed({e})"),
            Ok(_) => panic!("expected Gone, the watch started"),
        }
    }

    #[test]
    fn the_parent_is_watched_while_it_runs() {
        let watch = ParentWatch::start(Target::Parent).expect("watch the test runner's parent");
        assert_ne!(watch.pid(), std::process::id());
        std::thread::sleep(Duration::from_millis(100));
        assert!(!watch.gone());
    }
}
