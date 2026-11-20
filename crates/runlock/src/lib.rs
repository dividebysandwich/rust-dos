//! The machine-wide run lock of rust-dos: any number of ordinary runs at
//! once, and a measurement alone. This comment is the protocol's
//! specification. Every program that takes the lock follows it, whatever
//! it is written in, and a later change keeps it compatible with the
//! programs that implement this version (`protocol` 2). Version 1 had
//! no `door` and `queue`; "Older programs" says how the two mix.
//!
//! # Files
//!
//! The lock is a directory, `DIR`. By default it is `run-lock` in the
//! per-user profile directory (`%APPDATA%\rust-dos\run-lock`,
//! `~/.config/rust-dos/run-lock`); the environment variable
//! `RUST_DOS_RUN_LOCK_DIR` moves it. Two programs exclude each other only
//! when they use the same directory. It holds:
//!
//! - `lock`, the main lock. An ordinary run locks it shared, a
//!   measurement exclusive.
//! - `gate`, which puts measurements before the shared runs that ask
//!   after them.
//! - `queue`, which puts shared runs before the measurements that ask
//!   after them.
//! - `door`, through which every take joins the line.
//! - `holders/`, one pair of files for each program that holds the lock
//!   or waits for it.
//!
//! The locks are the OS's file locks on the whole file (`LockFileEx` on
//! Windows, `flock` on Unix; std's `File::lock` and `lock_shared`). The OS
//! releases them when the process ends, however it ends, so a hold is
//! never stale. The files are never deleted except as below, and their
//! contents mean nothing.
//!
//! # Taking it
//!
//! A program creates `DIR` and `DIR/holders` when they are missing,
//! writes its holder files (below) with `state` `waiting`, and then:
//!
//! - Shared: it takes `door` shared, takes `queue` shared and keeps it,
//!   and lets `door` go. Then it takes `gate` exclusive, takes `lock`
//!   shared, and lets `gate` go; it holds `gate` only for that moment. If
//!   `lock` can't be had (a measurement is letting go), it lets `gate` go
//!   and tries again. Once it holds `lock` it lets `queue` go.
//! - Exclusive: it takes `door` exclusive, then `queue` exclusive, then
//!   `gate` shared, and lets `queue` and then `door` go. It keeps `gate`
//!   while it waits for `lock` and while it runs; then it takes `lock`
//!   exclusive.
//!
//! While a measurement waits or runs it holds `gate` shared, so no new
//! shared run can take it exclusive and get in, and the running ones
//! drain. Every waiting measurement holds `gate` at once, so when one
//! lets go, the next takes `lock` while the shared runs that asked after
//! it still find `gate` held. Measurements thus come before every shared
//! run that asked after them. Among several waiting measurements there is
//! no order.
//!
//! A shared run that waits holds `queue` shared, so a measurement that
//! asks after it can't take `queue` exclusive, nor then `gate`, until
//! that shared run is in. When the measurements that asked before the
//! shared run let go, `gate` is free, and no later measurement can take
//! it first. Shared runs thus come before every measurement that asked
//! after them, however soon a measurement script asks again after
//! letting go.
//!
//! `door` keeps that order while a measurement waits for `queue`. The
//! measurement holds `door` exclusive meanwhile, so shared runs that ask
//! after it can't join `queue` ahead of it, and it lets `door` go only
//! once it has `gate`. A shared take holds `door` only while it joins
//! `queue`. With `door` held shared no measurement holds `queue`, so
//! that step doesn't wait.
//!
//! The line is kept in OS locks rather than in a ticket file because a
//! counter in a file has to be read and written, which Windows refuses
//! for a locked file, and because a process killed between taking a
//! ticket and being served leaves a number every later program would
//! have to recognise as dead. With locks, the line is whoever holds
//! `queue` and `gate`, and a killed process leaves it when the OS lets
//! its locks go.
//!
//! A take finds `door`, `queue` or `gate` held for a moment by another
//! take passing through, too. It tells that apart from a wait by the
//! holders' records (the holders it waits for are listed) and may try
//! again sooner when none is.
//!
//! A program that waits with a limit polls the `try_` forms, since a
//! blocked lock call can't be given up. One that waits without a limit
//! may poll too. When the wait runs out, or the program gives up for a
//! reason of its own (the process it serves has ended), it lets go of
//! everything, removes its holder files, and fails.
//!
//! Once it holds the lock, it rewrites its record with `state` `holding`.
//! It lets go by removing its holder files (the JSON first), then `lock`,
//! then `gate`.
//!
//! # Older programs
//!
//! A program of `protocol` 1 takes only `gate` and `lock`, in the steps
//! above, and never `door` or `queue`. Mixed with programs of this
//! version, measurements still run alone and no take waits for itself:
//! the older program waits on nothing it didn't wait on before, and holds
//! nothing the newer ones wait on in a new way. Only the order between
//! them is lost. An older shared take can get in ahead of newer
//! measurements that wait for `queue`, and an older measurement takes
//! `gate` ahead of newer shared takes that waited longer, as in
//! version 1.
//!
//! # Holders
//!
//! Windows' locks are mandatory: a locked file can't be read. So nothing
//! is written inside `lock`, and each holder has a pair of files of its
//! own in `holders/`, named after its process ID (`<pid>`; a second hold
//! in one process adds `-<n>`, as `<pid>-1`):
//!
//! 1. `<name>.lock`, created if missing and locked exclusive for as long
//!    as the program holds or waits for the lock. A program that finds the
//!    file of a dead process with its ID takes it over: the OS lock, not
//!    the name, says who is alive. If the file is gone once it is locked
//!    (a reader removed it), the program starts this step again.
//! 2. `<name>.json`, written to `<name>.json.tmp` and renamed into place,
//!    so a reader never sees half a record. It is one JSON object:
//!
//!    | Field | Value |
//!    |---|---|
//!    | `protocol` | 1 |
//!    | `pid` | the process ID |
//!    | `program` | the program's name, as `rust-dos` or `rust-dos-lock` |
//!    | `name` | a label (the wrapped command), or null |
//!    | `mode` | `shared` or `exclusive` |
//!    | `state` | `waiting`, then `holding` |
//!    | `asked` | when it asked, in seconds since 1970 (UTC) |
//!    | `instance` | the debug server's instance (`/api/version`), or null |
//!    | `debug_server` | the debug server's address, or null |
//!
//!    Readers ignore fields they don't know. The command line isn't
//!    recorded: it may carry a password.
//!
//! A reader lists the `.json` files. When it can lock the matching
//! `.lock` file, or there is none, that holder is gone: the reader removes
//! the JSON, then the `.lock` file, and lets the lock go. A `.lock` file
//! with no JSON that it can lock is removed the same way, and so is a
//! `<name>.json.tmp` with no `.lock` file left (a writer killed before its
//! rename). A reader takes no other lock, so listing the holders never
//! gets in a waiter's way.
//!
//! # Nested holds
//!
//! A holder gives the programs it starts `RUST_DOS_RUN_LOCK_HELD`
//! (`shared` or `exclusive`) and `RUST_DOS_RUN_LOCK_DIR` (`DIR`). A
//! program that finds both, for the directory it would lock, inherits the
//! hold and takes nothing: shared under either mode and exclusive under
//! exclusive. Exclusive under shared is a conflict, and the program fails
//! as a timed-out wait does: it would wait for its own parent.
//!
//! A program that fails a wait or meets a conflict exits with
//! [`EXIT_TIMED_OUT`] (3) and says so on stderr.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub mod cli;

/// The protocol's version, the records' `protocol`.
pub const PROTOCOL: u32 = 2;
/// The main lock's file.
pub const LOCK_FILE: &str = "lock";
/// The gate's file.
pub const GATE_FILE: &str = "gate";
/// The file waiting shared takes hold.
pub const QUEUE_FILE: &str = "queue";
/// The file every take passes to join the line.
pub const DOOR_FILE: &str = "door";
/// The directory of the holder files.
pub const HOLDERS_DIR: &str = "holders";
/// The default directory's name in the profile directory.
pub const DIR_NAME: &str = "run-lock";
/// The mode a parent holds the lock in, for its children.
pub const ENV_HELD: &str = "RUST_DOS_RUN_LOCK_HELD";
/// The lock's directory: the parent's, and where the default one moves.
pub const ENV_DIR: &str = "RUST_DOS_RUN_LOCK_DIR";
/// The exit code of a wait that ran out, or of a conflict.
pub const EXIT_TIMED_OUT: i32 = 3;
/// How often a wait tries again.
pub const POLL: Duration = Duration::from_millis(100);
/// How long a wait with nobody listed in the way counts as a moment's
/// contention, and how often it tries again meanwhile.
const BRIEF: Duration = Duration::from_secs(1);
const BRIEF_POLL: Duration = Duration::from_millis(5);

/// How the lock is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// With other shared holders: an ordinary run.
    Shared,
    /// Alone: a measurement.
    Exclusive,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Shared => "shared",
            Mode::Exclusive => "exclusive",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "shared" => Some(Mode::Shared),
            "exclusive" => Some(Mode::Exclusive),
            _ => None,
        }
    }
}

/// Where a holder is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Waiting,
    Holding,
}

/// A holder's record, `holders/<name>.json`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Record {
    pub protocol: u32,
    pub pid: u32,
    pub program: String,
    #[serde(default)]
    pub name: Option<String>,
    pub mode: Mode,
    pub state: State,
    pub asked: u64,
    #[serde(default)]
    pub instance: Option<String>,
    #[serde(default)]
    pub debug_server: Option<String>,
}

/// A holder as a reader finds it: its file name without `.json`, and its
/// record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    pub file: String,
    pub record: Record,
}

impl Holder {
    /// One line about the holder, for the messages about waiting.
    pub fn describe(&self) -> String {
        let r = &self.record;
        let mut s = format!("pid {} {}", r.pid, r.program);
        if let Some(name) = &r.name {
            s += &format!(" ({})", name);
        }
        s += &format!(", {} {}", r.mode.as_str(), state_word(r.state));
        if let Some(addr) = &r.debug_server {
            s += &format!(", debug server {}", addr);
        }
        s += &format!(", asked {} ago", age(r.asked));
        s
    }
}

fn state_word(state: State) -> &'static str {
    match state {
        State::Waiting => "waiting",
        State::Holding => "holding",
    }
}

/// How long ago `asked` (seconds since 1970) was, as "12s" or "3m04s".
pub fn age(asked: u64) -> String {
    let secs = unix_now().saturating_sub(asked);
    match secs {
        0..60 => format!("{}s", secs),
        60..3600 => format!("{}m{:02}s", secs / 60, secs % 60),
        _ => format!("{}h{:02}m", secs / 3600, secs / 60 % 60),
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The lock's directory when nothing names one: `RUST_DOS_RUN_LOCK_DIR`
/// if it is set, else `run-lock` in the per-user profile directory.
pub fn default_dir() -> Option<PathBuf> {
    match std::env::var_os(ENV_DIR) {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => rust_dos_hostdirs::profile_dir().map(|d| d.join(DIR_NAME)),
    }
}

/// The hold the parent passed on, from the environment: its mode and
/// directory.
pub fn inherited() -> Option<(Mode, PathBuf)> {
    let mode = Mode::parse(std::env::var(ENV_HELD).ok()?.trim())?;
    let dir = std::env::var_os(ENV_DIR).filter(|d| !d.is_empty())?;
    Some((mode, PathBuf::from(dir)))
}

/// Whether two names of directories are the same directory.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => absolute(a) == absolute(b),
    }
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// What a take asks for.
#[derive(Clone, Debug)]
pub struct Request {
    pub mode: Mode,
    /// The longest wait; None waits without limit.
    pub wait: Option<Duration>,
    /// The record's `program`.
    pub program: String,
    /// The record's `name`.
    pub name: Option<String>,
    /// How often the wait tries again.
    pub poll: Duration,
    /// A flag that ends the wait when it goes up (another thread raises
    /// it), with [`Error::Cancelled`].
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Request {
    pub fn new(mode: Mode, program: &str) -> Request {
        Request {
            mode,
            wait: None,
            program: program.to_string(),
            name: None,
            poll: POLL,
            cancel: None,
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
    }
}

/// Why a take failed.
#[derive(Debug)]
pub enum Error {
    /// The directory or a file in it can't be made or locked.
    Io { path: PathBuf, error: io::Error },
    /// The wait ran out; the holders it waited for.
    TimedOut {
        mode: Mode,
        dir: PathBuf,
        waited: Duration,
        holders: Vec<Holder>,
    },
    /// An exclusive hold asked for inside a shared one.
    Conflict { dir: PathBuf },
    /// The request's `cancel` flag went up during the wait.
    Cancelled { mode: Mode, dir: PathBuf },
}

impl Error {
    /// The exit code a program that gives up on the lock exits with.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Io { .. } => 1,
            Error::TimedOut { .. } | Error::Conflict { .. } | Error::Cancelled { .. } => {
                EXIT_TIMED_OUT
            }
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io { path, error } => write!(f, "{}: {}", path.display(), error),
            Error::TimedOut {
                mode,
                dir,
                waited,
                holders,
            } => {
                write!(
                    f,
                    "gave up after {}s waiting for the {} lock in {}",
                    waited.as_secs(),
                    mode.as_str(),
                    dir.display()
                )?;
                if !holders.is_empty() {
                    write!(f, "; it is held or asked for by {}", list(holders))?;
                }
                Ok(())
            }
            Error::Conflict { dir } => write!(
                f,
                "an exclusive lock was asked for inside a shared hold of {} ({}=shared): \
                 it would wait for its own parent",
                dir.display(),
                ENV_HELD
            ),
            Error::Cancelled { mode, dir } => write!(
                f,
                "stopped waiting for the {} lock in {}",
                mode.as_str(),
                dir.display()
            ),
        }
    }
}

impl std::error::Error for Error {}

/// The holders, described, separated by "; ".
pub fn list(holders: &[Holder]) -> String {
    holders
        .iter()
        .map(Holder::describe)
        .collect::<Vec<_>>()
        .join("; ")
}

/// A take that succeeded.
#[derive(Debug)]
pub enum Taken {
    /// This program holds the lock until the hold drops.
    Held(Box<Hold>),
    /// The parent holds it, in this mode, for this program too.
    Inherited(Mode),
}

impl Taken {
    /// The mode held, by this program or its parent.
    pub fn mode(&self) -> Mode {
        match self {
            Taken::Held(hold) => hold.mode,
            Taken::Inherited(mode) => *mode,
        }
    }
}

/// Takes the lock in `dir`, or inherits the parent's hold (see the
/// environment's [`inherited`]). `waiting` is called with the holders
/// in the way when the wait starts and whenever they change.
pub fn take(dir: &Path, request: &Request, waiting: impl FnMut(&[Holder])) -> Result<Taken, Error> {
    take_with(dir, request, inherited(), waiting)
}

/// [`take`], with the parent's hold given.
pub fn take_with(
    dir: &Path,
    request: &Request,
    parent: Option<(Mode, PathBuf)>,
    waiting: impl FnMut(&[Holder]),
) -> Result<Taken, Error> {
    let dir = absolute(dir);
    if let Some((held, parent_dir)) = parent
        && same_dir(&parent_dir, &dir)
    {
        return match (held, request.mode) {
            (Mode::Shared, Mode::Exclusive) => Err(Error::Conflict { dir }),
            (held, _) => Ok(Taken::Inherited(held)),
        };
    }
    Hold::take(dir, request, waiting).map(|hold| Taken::Held(Box::new(hold)))
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |error| Error::Io {
        path: path.to_path_buf(),
        error,
    }
}

fn open_lock_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

/// Whether `try_lock` (or `try_lock_shared`) got the lock.
fn got(result: Result<(), TryLockError>) -> io::Result<bool> {
    match result {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

/// This program's holder files.
#[derive(Debug)]
struct Entry {
    /// `<name>.lock`, locked while it is open.
    _lock: File,
    lock_path: PathBuf,
    json_path: PathBuf,
    file: String,
}

impl Entry {
    /// Creates and locks `<pid>.lock` (or `<pid>-<n>.lock`).
    fn create(holders: &Path) -> Result<Entry, Error> {
        let pid = std::process::id();
        let mut n = 0u32;
        let mut attempts = 0u32;
        loop {
            let file = if n == 0 {
                pid.to_string()
            } else {
                format!("{}-{}", pid, n)
            };
            let lock_path = holders.join(format!("{}.lock", file));
            let lock = match open_lock_file(&lock_path) {
                Ok(lock) => lock,
                // Windows: a file a reader deleted while it was open stays
                // until the reader closes it.
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied && attempts < 100 => {
                    attempts += 1;
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(e) => return Err(io_err(&lock_path)(e)),
            };
            if !got(lock.try_lock()).map_err(io_err(&lock_path))? {
                // Another hold of this process.
                n += 1;
                continue;
            }
            // A reader that found the file before it was locked removed
            // it: this lock is on a file nobody else sees.
            if !lock_path.exists() {
                attempts += 1;
                if attempts > 100 {
                    return Err(io_err(&lock_path)(io::Error::other(
                        "the holder file keeps vanishing",
                    )));
                }
                continue;
            }
            let json_path = holders.join(format!("{}.json", file));
            return Ok(Entry {
                _lock: lock,
                lock_path,
                json_path,
                file,
            });
        }
    }

    fn write(&self, record: &Record) -> Result<(), Error> {
        let tmp = self.json_path.with_extension("json.tmp");
        let text = serde_json::to_string(record).expect("a record is JSON");
        std::fs::write(&tmp, text).map_err(io_err(&tmp))?;
        // Windows refuses the rename while a reader has the record open
        // without sharing it for deletion, as Python's open() does.
        let mut attempts = 0;
        loop {
            match std::fs::rename(&tmp, &self.json_path) {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied && attempts < 100 => {
                    attempts += 1;
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(io_err(&self.json_path)(e));
                }
            }
        }
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.json_path);
        let _ = std::fs::remove_file(&self.lock_path);
        // The lock goes with the handle.
    }
}

/// A hold of the lock, given up when it drops.
#[derive(Debug)]
pub struct Hold {
    dir: PathBuf,
    mode: Mode,
    record: Record,
    // Dropped in this order: the holder files, `lock`, `gate`.
    entry: Entry,
    lock: File,
    /// A measurement's `gate`, held shared.
    gate: Option<File>,
}

/// The files a take uses while it waits, and how far it has got.
struct Line {
    door: File,
    queue: File,
    gate: File,
    door_path: PathBuf,
    queue_path: PathBuf,
    gate_path: PathBuf,
    lock_path: PathBuf,
    step: Step,
}

/// How far a take has got in the line.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Outside: it holds nothing yet.
    Outside,
    /// A measurement holding `door`, waiting for `queue`.
    AtDoor,
    /// A shared take holding `queue`, or a measurement holding `gate`.
    InLine,
}

impl Hold {
    fn take(
        dir: PathBuf,
        request: &Request,
        mut waiting: impl FnMut(&[Holder]),
    ) -> Result<Hold, Error> {
        let holders = dir.join(HOLDERS_DIR);
        std::fs::create_dir_all(&holders).map_err(io_err(&holders))?;
        let lock_path = dir.join(LOCK_FILE);
        let gate_path = dir.join(GATE_FILE);
        let queue_path = dir.join(QUEUE_FILE);
        let door_path = dir.join(DOOR_FILE);
        let lock = open_lock_file(&lock_path).map_err(io_err(&lock_path))?;
        let mut line = Line {
            door: open_lock_file(&door_path).map_err(io_err(&door_path))?,
            queue: open_lock_file(&queue_path).map_err(io_err(&queue_path))?,
            gate: open_lock_file(&gate_path).map_err(io_err(&gate_path))?,
            door_path,
            queue_path,
            gate_path,
            lock_path,
            step: Step::Outside,
        };
        let entry = Entry::create(&holders)?;
        let record = Record {
            protocol: PROTOCOL,
            pid: std::process::id(),
            program: request.program.clone(),
            name: request.name.clone(),
            mode: request.mode,
            state: State::Waiting,
            asked: unix_now(),
            instance: None,
            debug_server: None,
        };
        entry.write(&record)?;
        let mut hold = Hold {
            dir,
            mode: request.mode,
            record,
            entry,
            lock,
            gate: None,
        };

        let start = Instant::now();
        let mut told: Option<Vec<String>> = None;
        loop {
            if hold.try_once(&mut line)? {
                break;
            }
            let in_way = hold.in_the_way(line.step);
            // Nobody listed in the way, early in the wait: another take
            // has `door`, `queue` or `gate` for a moment, or a holder's
            // record isn't written yet. Not worth a line about waiting,
            // and soon over.
            let brief = in_way.is_empty() && told.is_none() && start.elapsed() < BRIEF;
            let files: Vec<String> = in_way.iter().map(|h| h.file.clone()).collect();
            if !brief && told.as_ref() != Some(&files) {
                waiting(&in_way);
                told = Some(files);
            }
            if let Some(limit) = request.wait
                && start.elapsed() >= limit
            {
                return Err(Error::TimedOut {
                    mode: hold.mode,
                    dir: hold.dir.clone(),
                    waited: start.elapsed(),
                    holders: in_way,
                });
            }
            if request.cancelled() {
                return Err(Error::Cancelled {
                    mode: hold.mode,
                    dir: hold.dir.clone(),
                });
            }
            std::thread::sleep(if brief {
                request.poll.min(BRIEF_POLL)
            } else {
                request.poll
            });
        }
        // A measurement keeps `gate`; the other files' handles close,
        // with no lock left on them.
        if hold.mode == Mode::Exclusive {
            hold.gate = Some(line.gate);
        }
        hold.record.state = State::Holding;
        hold.entry.write(&hold.record)?;
        Ok(hold)
    }

    /// One try at the lock, from where `line` has got to.
    fn try_once(&self, line: &mut Line) -> Result<bool, Error> {
        match self.mode {
            Mode::Shared => {
                if line.step == Step::Outside {
                    if !got(line.door.try_lock_shared()).map_err(io_err(&line.door_path))? {
                        return Ok(false);
                    }
                    let joined =
                        got(line.queue.try_lock_shared()).map_err(io_err(&line.queue_path));
                    line.door.unlock().map_err(io_err(&line.door_path))?;
                    if !joined? {
                        return Ok(false);
                    }
                    line.step = Step::InLine;
                }
                if !got(line.gate.try_lock()).map_err(io_err(&line.gate_path))? {
                    return Ok(false);
                }
                let taken = got(self.lock.try_lock_shared()).map_err(io_err(&line.lock_path));
                line.gate.unlock().map_err(io_err(&line.gate_path))?;
                if !taken? {
                    return Ok(false);
                }
                line.queue.unlock().map_err(io_err(&line.queue_path))?;
                Ok(true)
            }
            Mode::Exclusive => {
                if line.step == Step::Outside {
                    if !got(line.door.try_lock()).map_err(io_err(&line.door_path))? {
                        return Ok(false);
                    }
                    line.step = Step::AtDoor;
                }
                if line.step == Step::AtDoor {
                    // The shared takes waiting in `queue` asked first.
                    if !got(line.queue.try_lock()).map_err(io_err(&line.queue_path))? {
                        return Ok(false);
                    }
                    // Only a take of protocol 1 has `gate` exclusive now.
                    let gated = got(line.gate.try_lock_shared()).map_err(io_err(&line.gate_path));
                    line.queue.unlock().map_err(io_err(&line.queue_path))?;
                    if !gated? {
                        return Ok(false);
                    }
                    line.door.unlock().map_err(io_err(&line.door_path))?;
                    line.step = Step::InLine;
                }
                got(self.lock.try_lock()).map_err(io_err(&line.lock_path))
            }
        }
    }

    /// The other holders this take waits for. A shared take waits for the
    /// exclusive ones, which hold `door` or `gate` while they wait and
    /// run. An exclusive take waits for those that hold the lock, in
    /// either mode, and until it has `gate`, for the shared takes that
    /// asked before it and wait in `queue`.
    fn in_the_way(&self, step: Step) -> Vec<Holder> {
        let mut found = holders(&self.dir).unwrap_or_default();
        found.retain(|h| h.file != self.entry.file);
        found.retain(|h| match self.mode {
            Mode::Shared => h.record.mode == Mode::Exclusive,
            Mode::Exclusive => {
                h.record.state == State::Holding
                    || (step != Step::InLine
                        && h.record.mode == Mode::Shared
                        && h.record.asked <= self.record.asked)
            }
        });
        found
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The record as written.
    pub fn record(&self) -> &Record {
        &self.record
    }

    /// Records the program's debug server: its instance and address.
    pub fn set_debug_server(
        &mut self,
        instance: Option<String>,
        address: Option<String>,
    ) -> Result<(), Error> {
        self.record.instance = instance;
        self.record.debug_server = address;
        self.entry.write(&self.record)
    }

    /// The variables that pass the hold on to the programs this one
    /// starts.
    pub fn env(&self) -> [(&'static str, std::ffi::OsString); 2] {
        [
            (ENV_HELD, self.mode.as_str().into()),
            (ENV_DIR, self.dir.clone().into_os_string()),
        ]
    }
}

/// The holders of the lock in `dir` and those waiting for it, oldest
/// first. Removes the files of holders that are gone.
pub fn holders(dir: &Path) -> io::Result<Vec<Holder>> {
    let holders_dir = dir.join(HOLDERS_DIR);
    let entries = match std::fs::read_dir(&holders_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut jsons = Vec::new();
    let mut locks = Vec::new();
    let mut tmps = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(stem) = name.strip_suffix(".json") {
            jsons.push(stem.to_string());
        } else if let Some(stem) = name.strip_suffix(".lock") {
            locks.push(stem.to_string());
        } else if let Some(stem) = name.strip_suffix(".json.tmp") {
            tmps.push(stem.to_string());
        }
    }
    let mut found = Vec::new();
    for stem in &jsons {
        let json_path = holders_dir.join(format!("{}.json", stem));
        let lock_path = holders_dir.join(format!("{}.lock", stem));
        if !alive(&lock_path, Some(&json_path)) {
            continue;
        }
        // A record that can't be read (a newer protocol's, say) belongs to
        // a live holder all the same; it isn't listed.
        let Ok(text) = std::fs::read_to_string(&json_path) else {
            continue;
        };
        if let Ok(record) = serde_json::from_str::<Record>(&text) {
            found.push(Holder {
                file: stem.clone(),
                record,
            });
        }
    }
    for stem in locks.iter().filter(|stem| !jsons.contains(stem)) {
        alive(&holders_dir.join(format!("{}.lock", stem)), None);
    }
    // A writer killed between its write and its rename left this; a live
    // one keeps its `.lock` file, which the loops above left in place.
    for stem in &tmps {
        if !holders_dir.join(format!("{}.lock", stem)).exists() {
            let _ = std::fs::remove_file(holders_dir.join(format!("{}.json.tmp", stem)));
        }
    }
    found.sort_by(|a, b| (a.record.asked, &a.file).cmp(&(b.record.asked, &b.file)));
    Ok(found)
}

/// Whether the holder whose lock file is `lock_path` is alive. A dead
/// one's files go: `json_path` first, then the lock file.
fn alive(lock_path: &Path, json_path: Option<&Path>) -> bool {
    let lock = match OpenOptions::new().read(true).write(true).open(lock_path) {
        Ok(lock) => lock,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if let Some(json) = json_path {
                let _ = std::fs::remove_file(json);
            }
            return false;
        }
        // Can't tell (no permission, say): count it as alive.
        Err(_) => return true,
    };
    match lock.try_lock() {
        Ok(()) => {
            if let Some(json) = json_path {
                let _ = std::fs::remove_file(json);
            }
            let _ = std::fs::remove_file(lock_path);
            false
        }
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rust-dos-runlock-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn request(mode: Mode, wait_ms: u64) -> Request {
        Request {
            wait: Some(Duration::from_millis(wait_ms)),
            poll: Duration::from_millis(10),
            ..Request::new(mode, "test")
        }
    }

    fn held(taken: Result<Taken, Error>) -> Hold {
        match taken {
            Ok(Taken::Held(hold)) => *hold,
            other => panic!("not held: {:?}", other),
        }
    }

    #[test]
    fn shared_holds_go_together() {
        let dir = scratch("shared");
        let a = held(take_with(&dir, &request(Mode::Shared, 0), None, |_| {}));
        let b = held(take_with(&dir, &request(Mode::Shared, 0), None, |_| {}));
        let listed = holders(&dir).unwrap();
        assert_eq!(listed.len(), 2);
        assert!(
            listed
                .iter()
                .all(|h| h.record.state == State::Holding && h.record.mode == Mode::Shared)
        );
        // The second hold of one process gets a name of its own.
        let pid = std::process::id().to_string();
        assert!(listed.iter().any(|h| h.file == pid));
        assert!(listed.iter().any(|h| h.file == format!("{}-1", pid)));
        drop((a, b));
        assert!(holders(&dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exclusive_waits_for_shared_and_times_out() {
        let dir = scratch("excl");
        let shared = held(take_with(&dir, &request(Mode::Shared, 0), None, |_| {}));
        let mut told = Vec::new();
        let result = take_with(&dir, &request(Mode::Exclusive, 100), None, |h| {
            told.push(h.to_vec())
        });
        match result {
            Err(Error::TimedOut {
                mode: Mode::Exclusive,
                holders,
                ..
            }) => {
                assert_eq!(holders.len(), 1);
                assert_eq!(holders[0].record.mode, Mode::Shared);
            }
            other => panic!("expected a timeout: {:?}", other),
        }
        assert_eq!(
            told.len(),
            1,
            "one waiting line while the holders stay the same"
        );
        assert_eq!(told[0].len(), 1);
        // The timed-out take left nothing behind, the gate included.
        assert_eq!(holders(&dir).unwrap().len(), 1);
        let again = held(take_with(&dir, &request(Mode::Shared, 0), None, |_| {}));
        drop((shared, again));
        let excl = held(take_with(&dir, &request(Mode::Exclusive, 0), None, |_| {}));
        assert_eq!(excl.record().state, State::Holding);
        // A shared take waits for it, and names it.
        match take_with(&dir, &request(Mode::Shared, 50), None, |_| {}) {
            Err(Error::TimedOut { holders, .. }) => {
                assert_eq!(holders.len(), 1);
                assert_eq!(holders[0].record.mode, Mode::Exclusive);
            }
            other => panic!("expected a timeout: {:?}", other),
        }
        drop(excl);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_gate_keeps_new_shared_holds_out() {
        let dir = scratch("gate");
        let shared = held(take_with(&dir, &request(Mode::Shared, 0), None, |_| {}));
        let dir2 = dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let measurement = std::thread::spawn(move || {
            let req = Request {
                poll: Duration::from_millis(10),
                ..Request::new(Mode::Exclusive, "measure")
            };
            let hold = held(take_with(&dir2, &req, None, |_| {}));
            tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            drop(hold);
        });
        // Wait until the measurement waits.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !holders(&dir)
            .unwrap()
            .iter()
            .any(|h| h.record.mode == Mode::Exclusive)
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert!(matches!(
            take_with(&dir, &request(Mode::Shared, 100), None, |_| {}),
            Err(Error::TimedOut { .. })
        ));
        drop(shared);
        rx.recv_timeout(Duration::from_secs(10))
            .expect("the measurement got the lock");
        let after = held(take_with(
            &dir,
            &request(Mode::Shared, 10_000),
            None,
            |_| {},
        ));
        measurement.join().unwrap();
        drop(after);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_waiting_measurement_comes_before_later_shared_holds_at_the_hand_over() {
        let dir = scratch("handover");
        let first = held(take_with(&dir, &request(Mode::Exclusive, 0), None, |_| {}));
        let dir2 = dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let second = std::thread::spawn(move || {
            // Slow to try again, so that the shared take below tries
            // first once the first measurement has let go.
            let req = Request {
                poll: Duration::from_millis(500),
                ..Request::new(Mode::Exclusive, "second")
            };
            let hold = held(take_with(&dir2, &req, None, |_| {}));
            tx.send(()).unwrap();
            drop(hold);
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !holders(&dir)
            .unwrap()
            .iter()
            .any(|h| h.record.state == State::Waiting)
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(first);
        // The second measurement asked first: the gate it holds keeps
        // this shared take out although the lock is free.
        assert!(matches!(
            take_with(&dir, &request(Mode::Shared, 0), None, |_| {}),
            Err(Error::TimedOut { .. })
        ));
        rx.recv_timeout(Duration::from_secs(10))
            .expect("the second measurement got the lock");
        second.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Waits until `dir` lists `n` holders in `state` with `mode`.
    fn until_listed(dir: &Path, mode: Mode, state: State, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while holders(dir)
            .unwrap()
            .iter()
            .filter(|h| h.record.mode == mode && h.record.state == state)
            .count()
            < n
        {
            assert!(
                Instant::now() < deadline,
                "{:?} {:?} not listed",
                mode,
                state
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A take on a thread of its own, which holds the lock for `hold_ms`.
    /// `turns` gets `name` when it gets in.
    fn take_on_thread(
        dir: &Path,
        mode: Mode,
        name: &'static str,
        poll_ms: u64,
        hold_ms: u64,
        turns: std::sync::mpsc::Sender<&'static str>,
    ) -> std::thread::JoinHandle<()> {
        let dir = dir.to_path_buf();
        std::thread::spawn(move || {
            let req = Request {
                poll: Duration::from_millis(poll_ms),
                wait: Some(Duration::from_secs(20)),
                ..Request::new(mode, name)
            };
            let hold = held(take_with(&dir, &req, None, |_| {}));
            turns.send(name).unwrap();
            std::thread::sleep(Duration::from_millis(hold_ms));
            drop(hold);
        })
    }

    #[test]
    fn a_waiting_shared_take_comes_before_a_later_measurement() {
        let dir = scratch("fair");
        let first = held(take_with(&dir, &request(Mode::Exclusive, 0), None, |_| {}));
        let (tx, rx) = std::sync::mpsc::channel();
        // The shared take tries again far less often than the second
        // measurement, which in protocol 1 took `gate` at once and kept
        // it, so that the shared take never found it free.
        let shared = take_on_thread(&dir, Mode::Shared, "shared", 200, 300, tx.clone());
        until_listed(&dir, Mode::Shared, State::Waiting, 1);
        let second = take_on_thread(&dir, Mode::Exclusive, "second", 5, 0, tx);
        until_listed(&dir, Mode::Exclusive, State::Waiting, 1);
        std::thread::sleep(Duration::from_millis(100));
        drop(first);
        let order: Vec<&str> = rx.iter().take(2).collect();
        assert_eq!(order, ["shared", "second"]);
        shared.join().unwrap();
        second.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_measurement_at_the_door_comes_before_later_shared_takes() {
        let dir = scratch("door");
        let first = held(take_with(&dir, &request(Mode::Exclusive, 0), None, |_| {}));
        let (tx, rx) = std::sync::mpsc::channel();
        // A shared take waits behind the first measurement, and a second
        // measurement waits behind it, holding `door`.
        let early = take_on_thread(&dir, Mode::Shared, "early", 10, 200, tx.clone());
        until_listed(&dir, Mode::Shared, State::Waiting, 1);
        let second = take_on_thread(&dir, Mode::Exclusive, "second", 10, 200, tx.clone());
        until_listed(&dir, Mode::Exclusive, State::Waiting, 1);
        std::thread::sleep(Duration::from_millis(100));
        // This one asked after the second measurement, and tries again
        // sooner than it.
        let late = take_on_thread(&dir, Mode::Shared, "late", 2, 0, tx);
        until_listed(&dir, Mode::Shared, State::Waiting, 2);
        drop(first);
        let order: Vec<&str> = rx.iter().take(3).collect();
        assert_eq!(order, ["early", "second", "late"]);
        for t in [early, second, late] {
            t.join().unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_raised_cancel_flag_ends_the_wait() {
        let dir = scratch("cancel");
        let excl = held(take_with(&dir, &request(Mode::Exclusive, 0), None, |_| {}));
        let flag = Arc::new(AtomicBool::new(true));
        let req = Request {
            cancel: Some(flag),
            poll: Duration::from_millis(10),
            ..Request::new(Mode::Shared, "test")
        };
        let err = take_with(&dir, &req, None, |_| {}).unwrap_err();
        assert!(matches!(err, Error::Cancelled { .. }), "{:?}", err);
        assert_eq!(holders(&dir).unwrap().len(), 1);
        drop(excl);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nested_holds_inherit() {
        let dir = scratch("nested");
        std::fs::create_dir_all(&dir).unwrap();
        let shared = Some((Mode::Shared, dir.clone()));
        let exclusive = Some((Mode::Exclusive, dir.clone()));
        let r = |mode| request(mode, 0);
        assert!(matches!(
            take_with(&dir, &r(Mode::Shared), shared.clone(), |_| {}),
            Ok(Taken::Inherited(Mode::Shared))
        ));
        assert!(matches!(
            take_with(&dir, &r(Mode::Shared), exclusive.clone(), |_| {}),
            Ok(Taken::Inherited(Mode::Exclusive))
        ));
        assert!(matches!(
            take_with(&dir, &r(Mode::Exclusive), exclusive, |_| {}),
            Ok(Taken::Inherited(Mode::Exclusive))
        ));
        let conflict = take_with(&dir, &r(Mode::Exclusive), shared, |_| {}).unwrap_err();
        assert!(matches!(conflict, Error::Conflict { .. }));
        assert_eq!(conflict.exit_code(), EXIT_TIMED_OUT);
        // A parent's hold of another directory is no hold of this one.
        let other = Some((Mode::Exclusive, dir.join("elsewhere")));
        let hold = held(take_with(&dir, &r(Mode::Shared), other, |_| {}));
        let env = hold.env();
        assert_eq!(env[0], (ENV_HELD, "shared".into()));
        assert_eq!(env[1].0, ENV_DIR);
        drop(hold);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dead_holders_files_go() {
        let dir = scratch("dead");
        let holders_dir = dir.join(HOLDERS_DIR);
        std::fs::create_dir_all(&holders_dir).unwrap();
        // A holder that died: its files, and no lock on them.
        let record = Record {
            protocol: PROTOCOL,
            pid: 999_999,
            program: "gone".into(),
            name: None,
            mode: Mode::Exclusive,
            state: State::Holding,
            asked: 1,
            instance: None,
            debug_server: None,
        };
        std::fs::write(
            holders_dir.join("999999.json"),
            serde_json::to_string(&record).unwrap(),
        )
        .unwrap();
        std::fs::write(holders_dir.join("999999.lock"), "").unwrap();
        // One that died between its two files, and a record without a
        // lock file.
        std::fs::write(holders_dir.join("999998.lock"), "").unwrap();
        std::fs::write(
            holders_dir.join("999997.json"),
            serde_json::to_string(&record).unwrap(),
        )
        .unwrap();
        // One killed between writing its record and renaming it.
        std::fs::write(holders_dir.join("999996.json.tmp"), "{").unwrap();
        assert!(holders(&dir).unwrap().is_empty());
        let left: Vec<_> = std::fs::read_dir(&holders_dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert!(left.is_empty(), "left: {:?}", left);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn records_read_back_and_ignore_unknown_fields() {
        let text = r#"{"protocol":1,"pid":5,"program":"x","mode":"exclusive","state":"waiting",
            "asked":7,"future":true}"#;
        let record: Record = serde_json::from_str(text).unwrap();
        assert_eq!(record.mode, Mode::Exclusive);
        assert_eq!(record.state, State::Waiting);
        assert_eq!(record.name, None);
        assert_eq!(record.debug_server, None);
    }

    #[test]
    fn debug_server_goes_into_the_record() {
        let dir = scratch("debug");
        let mut hold = held(take_with(&dir, &request(Mode::Shared, 0), None, |_| {}));
        hold.set_debug_server(Some("00ff".into()), Some("127.0.0.1:8086".into()))
            .unwrap();
        let listed = holders(&dir).unwrap();
        assert_eq!(listed[0].record.instance.as_deref(), Some("00ff"));
        assert_eq!(
            listed[0].record.debug_server.as_deref(),
            Some("127.0.0.1:8086")
        );
        assert!(listed[0].describe().contains("debug server 127.0.0.1:8086"));
        drop(hold);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unwritable_directory_is_an_io_error() {
        let dir = scratch("unwritable");
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        // A file where the directory should be.
        std::fs::write(&dir, "").unwrap();
        let err = take_with(&dir, &request(Mode::Shared, 0), None, |_| {}).unwrap_err();
        assert!(matches!(err, Error::Io { .. }));
        assert_eq!(err.exit_code(), 1);
        let _ = std::fs::remove_file(&dir);
    }
}
