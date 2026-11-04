//! The wrapper and the status tool: `rust-dos-lock`, and the same as
//! `rust-dos lock`.
//!
//! ```text
//! rust-dos-lock shared|exclusive [--dir DIR] [--wait SECONDS] [--name NAME] -- CMD [ARGS...]
//! rust-dos-lock status [--dir DIR] [--json]
//! ```
//!
//! `shared` and `exclusive` take the lock, run CMD with the hold passed on
//! in its environment, let go when CMD ends, and exit with CMD's exit code
//! (128 + the signal's number for one a signal ended, on Unix). On Windows
//! the wrapper and CMD run in a job object that ends them all when the
//! wrapper ends, so a killed wrapper takes CMD and its children with it.

use crate::{EXIT_TIMED_OUT, Mode, Request, Taken};
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// The exit code of a bad command line.
pub const EXIT_USAGE: i32 = 2;

fn usage(program: &str) -> String {
    format!(
        "usage: {p} shared|exclusive [--dir DIR] [--wait SECONDS] [--name NAME] -- CMD [ARGS...]\n       \
         {p} status [--dir DIR] [--json]\n\n\
         shared     runs CMD holding the machine-wide run lock shared, beside other\n           \
         \x20shared holders (builds, tests, emulator runs)\n\
         exclusive  waits for every holder to end, keeps new shared holders out, and\n           \
         \x20runs CMD alone (a measurement)\n\
         status     lists the holders and the programs waiting for the lock\n\n\
         --dir DIR       the lock's directory [default: ${env}, else run-lock in\n                \
         \x20the rust-dos profile directory]\n\
         --wait SECONDS  give up after this long, exit {code}, and not run CMD\n                \
         \x20[default: 0, no limit]\n\
         --name NAME     a label for status [default: CMD's name]\n\n\
         The exit code is CMD's. A program inside the hold (rust-dos, another\n\
         {p}) inherits it through {held} and {env}.",
        p = program,
        env = crate::ENV_DIR,
        held = crate::ENV_HELD,
        code = EXIT_TIMED_OUT,
    )
}

struct Options {
    dir: Option<PathBuf>,
    wait: Option<Duration>,
    name: Option<String>,
    json: bool,
    command: Vec<OsString>,
}

fn parse(args: &[OsString], wrapping: bool) -> Result<Options, String> {
    let mut opts = Options {
        dir: None,
        wait: None,
        name: None,
        json: false,
        command: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].to_string_lossy();
        let mut value = |what: &str| -> Result<OsString, String> {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{} needs a value", what))
        };
        match arg.as_ref() {
            "--dir" => opts.dir = Some(PathBuf::from(value("--dir")?)),
            "--wait" => {
                let v = value("--wait")?;
                let secs: u64 = v.to_str().and_then(|s| s.parse().ok()).ok_or_else(|| {
                    format!("--wait takes whole seconds, not {}", v.to_string_lossy())
                })?;
                opts.wait = (secs > 0).then(|| Duration::from_secs(secs));
            }
            "--name" if wrapping => {
                opts.name = Some(value("--name")?.to_string_lossy().into_owned())
            }
            "--json" if !wrapping => opts.json = true,
            "--" if wrapping => {
                opts.command = args[i + 1..].to_vec();
                break;
            }
            s if wrapping && !s.starts_with('-') => {
                opts.command = args[i..].to_vec();
                break;
            }
            s => return Err(format!("unknown option {}", s)),
        }
        i += 1;
    }
    if wrapping && opts.command.is_empty() {
        return Err("no command to run".into());
    }
    Ok(opts)
}

/// Runs the tool with `args` (without the program's own name), as
/// `program` (the name its messages give). Returns the exit code.
pub fn main(program: &str, args: Vec<OsString>) -> i32 {
    let Some(first) = args.first().map(|a| a.to_string_lossy().into_owned()) else {
        eprintln!("{}", usage(program));
        return EXIT_USAGE;
    };
    let rest = &args[1..];
    let mode = match first.as_str() {
        "shared" => Mode::Shared,
        "exclusive" => Mode::Exclusive,
        "status" => {
            return match parse(rest, false) {
                Ok(opts) => status(opts),
                Err(e) => {
                    eprintln!("{}: {}\n{}", program, e, usage(program));
                    EXIT_USAGE
                }
            };
        }
        "-h" | "--help" | "help" => {
            println!("{}", usage(program));
            return 0;
        }
        "-V" | "--version" => {
            println!(
                "{} {} (run lock protocol {})",
                program,
                env!("CARGO_PKG_VERSION"),
                crate::PROTOCOL
            );
            return 0;
        }
        other => {
            eprintln!("{}: unknown mode {}\n{}", program, other, usage(program));
            return EXIT_USAGE;
        }
    };
    let opts = match parse(rest, true) {
        Ok(opts) => opts,
        Err(e) => {
            eprintln!("{}: {}\n{}", program, e, usage(program));
            return EXIT_USAGE;
        }
    };
    wrap(program, mode, opts)
}

fn lock_dir(opts: &Options) -> Option<PathBuf> {
    opts.dir.clone().or_else(crate::default_dir)
}

fn wrap(program: &str, mode: Mode, opts: Options) -> i32 {
    let Some(dir) = lock_dir(&opts) else {
        eprintln!(
            "run-lock: no profile directory to keep the lock in: give --dir or {}",
            crate::ENV_DIR
        );
        return 1;
    };
    let name = opts.name.clone().or_else(|| {
        std::path::Path::new(&opts.command[0])
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
    });
    let request = Request {
        wait: opts.wait,
        name,
        ..Request::new(mode, program)
    };
    let taken = crate::take(&dir, &request, |holders| {
        eprintln!(
            "run-lock: waiting for the {} lock in {}{}",
            mode.as_str(),
            dir.display(),
            if holders.is_empty() {
                String::new()
            } else {
                format!(": {}", crate::list(holders))
            }
        );
    });
    let taken = match taken {
        Ok(taken) => taken,
        Err(e) => {
            eprintln!(
                "run-lock: {}; {} did not run",
                e,
                opts.command[0].to_string_lossy()
            );
            return e.exit_code();
        }
    };
    let mut command = Command::new(platform::resolve(&opts.command[0]));
    command.args(&opts.command[1..]);
    if let Taken::Held(hold) = &taken {
        for (key, value) in hold.env() {
            command.env(key, value);
        }
    }
    let code = run(command, &opts.command[0]);
    drop(taken);
    code
}

/// Runs `command` and returns its exit code.
fn run(mut command: Command, name: &OsStr) -> i32 {
    platform::before_spawn();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("run-lock: can't start {}: {}", name.to_string_lossy(), e);
            return if e.kind() == std::io::ErrorKind::NotFound {
                127
            } else {
                126
            };
        }
    };
    platform::spawned(&child);
    let status = loop {
        match child.wait() {
            Ok(status) => break status,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                eprintln!("run-lock: lost {}: {}", name.to_string_lossy(), e);
                return 1;
            }
        }
    };
    platform::exit_code(status)
}

fn status(opts: Options) -> i32 {
    let Some(dir) = lock_dir(&opts) else {
        eprintln!(
            "run-lock: no profile directory to keep the lock in: give --dir or {}",
            crate::ENV_DIR
        );
        return 1;
    };
    let holders = match crate::holders(&dir) {
        Ok(holders) => holders,
        Err(e) => {
            eprintln!("run-lock: {}: {}", dir.display(), e);
            return 1;
        }
    };
    if opts.json {
        let records: Vec<_> = holders.iter().map(|h| &h.record).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&records).expect("records are JSON")
        );
        return 0;
    }
    println!("run lock in {}", dir.display());
    if holders.is_empty() {
        println!("  free: no holders, nobody waiting");
    }
    for h in &holders {
        let r = &h.record;
        let mut line = format!(
            "  pid {:<7} {:<9} {:<7} {:>6}  {}",
            r.pid,
            r.mode.as_str(),
            match r.state {
                crate::State::Waiting => "waiting",
                crate::State::Holding => "holding",
            },
            crate::age(r.asked),
            r.program
        );
        if let Some(name) = &r.name {
            line += &format!(" ({})", name);
        }
        if let Some(addr) = &r.debug_server {
            line += &format!("  debug server {}", addr);
        }
        if let Some(instance) = &r.instance {
            line += &format!("  instance {}", instance);
        }
        println!("{}", line);
    }
    0
}

#[cfg(windows)]
mod platform {
    use std::ffi::{OsStr, OsString};
    use std::path::{Path, PathBuf};
    use std::process::{Child, ExitStatus};
    use windows_sys::Win32::Foundation::{FALSE, HANDLE, TRUE};
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    /// Ctrl+C and Ctrl+Break reach CMD as well, which decides; the wrapper
    /// stays to let go of the lock when CMD ends.
    unsafe extern "system" fn ignore(_ctrl: u32) -> i32 {
        TRUE
    }

    /// Puts the wrapper in a job object that kills its processes when its
    /// last handle closes, which is when the wrapper ends. CMD and its
    /// children are born in it, so nothing slips out before it is in.
    /// Without one (a job that refuses nesting, before Windows 8) the
    /// wrapper runs as it is.
    pub(super) fn before_spawn() {
        unsafe {
            SetConsoleCtrlHandler(Some(ignore), TRUE);
            let job: HANDLE = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if set == FALSE || AssignProcessToJobObject(job, GetCurrentProcess()) == FALSE {
                windows_sys::Win32::Foundation::CloseHandle(job);
            }
            // The handle stays open until the wrapper ends.
        }
    }

    pub(super) fn spawned(_child: &Child) {}

    pub(super) fn exit_code(status: ExitStatus) -> i32 {
        status.code().unwrap_or(1)
    }

    /// CMD as a shell finds it: a bare name in the folders of PATH, in
    /// their order, with the extensions of PATHEXT. Windows itself looks
    /// in System32 first, where `bash` is WSL's, whatever shell the
    /// wrapper was started from.
    pub(super) fn resolve(name: &OsStr) -> OsString {
        let bare = Path::new(name);
        if name.to_string_lossy().contains(['/', '\\', ':']) {
            return name.to_os_string();
        }
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        let Some(path) = std::env::var_os("PATH") else {
            return name.to_os_string();
        };
        for dir in std::env::split_paths(&path) {
            let candidates: Vec<PathBuf> = if bare.extension().is_some() {
                vec![dir.join(bare)]
            } else {
                let with = |ext: &str| {
                    let mut file = name.to_os_string();
                    file.push(ext);
                    dir.join(file)
                };
                exts.split(';')
                    .filter(|e| !e.is_empty())
                    .map(with)
                    .collect()
            };
            if let Some(found) = candidates.into_iter().find(|c| c.is_file()) {
                return found.into_os_string();
            }
        }
        name.to_os_string()
    }
}

#[cfg(unix)]
mod platform {
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Child, ExitStatus};
    use std::sync::atomic::{AtomicI32, Ordering};

    static CHILD: AtomicI32 = AtomicI32::new(0);

    /// TERM and HUP go on to CMD, and the wrapper waits for it to end;
    /// INT and QUIT, which the terminal sends CMD too, are left to it.
    /// Handlers, unlike ignored signals, don't pass on to CMD.
    extern "C" fn forward(signal: libc::c_int) {
        let pid = CHILD.load(Ordering::SeqCst);
        if pid > 0 && (signal == libc::SIGTERM || signal == libc::SIGHUP) {
            unsafe { libc::kill(pid, signal) };
        }
    }

    pub(super) fn before_spawn() {
        for signal in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT, libc::SIGQUIT] {
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = forward as *const () as libc::sighandler_t;
                action.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut action.sa_mask);
                libc::sigaction(signal, &action, std::ptr::null_mut());
            }
        }
    }

    pub(super) fn spawned(child: &Child) {
        CHILD.store(child.id() as i32, Ordering::SeqCst);
    }

    pub(super) fn exit_code(status: ExitStatus) -> i32 {
        status
            .code()
            .or_else(|| status.signal().map(|s| 128 + s))
            .unwrap_or(1)
    }

    /// CMD as given: exec looks in PATH as a shell does.
    pub(super) fn resolve(name: &std::ffi::OsStr) -> std::ffi::OsString {
        name.to_os_string()
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    use std::process::{Child, ExitStatus};

    pub(super) fn before_spawn() {}

    pub(super) fn spawned(_child: &Child) {}

    pub(super) fn exit_code(status: ExitStatus) -> i32 {
        status.code().unwrap_or(1)
    }

    pub(super) fn resolve(name: &std::ffi::OsStr) -> std::ffi::OsString {
        name.to_os_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn options_parse() {
        let o = parse(
            &os(&[
                "--dir", "d", "--wait", "5", "--name", "n", "--", "cargo", "--x",
            ]),
            true,
        )
        .unwrap();
        assert_eq!(o.dir, Some(PathBuf::from("d")));
        assert_eq!(o.wait, Some(Duration::from_secs(5)));
        assert_eq!(o.name.as_deref(), Some("n"));
        assert_eq!(o.command, os(&["cargo", "--x"]));
        // The command may come without `--`, and --wait 0 has no limit.
        let o = parse(&os(&["--wait", "0", "cargo", "--", "x"]), true).unwrap();
        assert_eq!(o.wait, None);
        assert_eq!(o.command, os(&["cargo", "--", "x"]));
        assert!(parse(&os(&["--wait", "x", "cargo"]), true).is_err());
        assert!(parse(&os(&["--"]), true).is_err());
        assert!(parse(&os(&["--json"]), false).unwrap().json);
        assert!(parse(&os(&["cargo"]), false).is_err());
    }

    #[test]
    fn usage_errors_exit_2() {
        assert_eq!(main("t", Vec::new()), EXIT_USAGE);
        assert_eq!(main("t", os(&["both"])), EXIT_USAGE);
        assert_eq!(main("t", os(&["shared", "--wait"])), EXIT_USAGE);
    }
}
