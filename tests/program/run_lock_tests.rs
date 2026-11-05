//! The machine-wide run lock as rust-dos and rust-dos-lock take it:
//! emulator runs (`rust-dos run`, which needs no window) hold it shared
//! by default, wait behind a measurement, inherit a wrapper's hold, and
//! go without it where it can't be had and nobody asked for it.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::time::{Duration, Instant};

const EMULATOR: &str = env!("CARGO_BIN_EXE_rust-dos");
const LOCK: &str = env!("CARGO_BIN_EXE_rust-dos-lock");

#[rustfmt::skip]
/// Prints "OK" with INT 29h.
const FAST: &[u8] = &[
    0xB0, b'O', 0xCD, 0x29, // MOV AL, 'O' ; INT 29h
    0xB0, b'K', 0xCD, 0x29, // MOV AL, 'K' ; INT 29h
    0xCD, 0x20,             // INT 20h
];

#[rustfmt::skip]
/// Waits for a key with INT 16h, again and again: it runs until its
/// standard input closes.
const WAIT_KEY: &[u8] = &[
    0xB4, 0x00,             // MOV AH, 00h
    0xCD, 0x16,             // INT 16h
    0xEB, 0xFA,             // JMP $-4
];

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::path::absolute(PathBuf::from("target/test_run_lock").join(name)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("c")).unwrap();
        std::fs::write(dir.join("c/FAST.COM"), FAST).unwrap();
        std::fs::write(dir.join("c/WAITKEY.COM"), WAIT_KEY).unwrap();
        Scratch(dir)
    }

    fn lock(&self) -> PathBuf {
        self.0.join("lock")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `rust-dos [flags] run -d C PROGRAM`, with the lock's directory in the
/// environment, as a machine that moved it has.
fn emulator(s: &Scratch, flags: &[&str], program: &str) -> Command {
    let mut command = Command::new(EMULATOR);
    command
        .args(flags)
        .arg("run")
        .arg("-d")
        .arg(s.0.join("c"))
        .arg(program)
        .env("RUST_DOS_RUN_LOCK_DIR", s.lock())
        .env_remove("RUST_DOS_RUN_LOCK_HELD")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// `rust-dos-lock MODE --dir LOCK -- ARGS...`.
fn wrapper(s: &Scratch, mode: &str, args: &[&Path]) -> Command {
    let mut command = Command::new(LOCK);
    command
        .arg(mode)
        .arg("--dir")
        .arg(s.lock())
        .arg("--")
        .args(args);
    command
        .env_remove("RUST_DOS_RUN_LOCK_HELD")
        .env_remove("RUST_DOS_RUN_LOCK_DIR");
    command
}

fn run(mut command: Command) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdin.take());
    child.wait_with_output().unwrap()
}

/// The holders' records, as `rust-dos-lock status --json` lists them.
fn status(s: &Scratch) -> Vec<serde_json::Value> {
    let out = Command::new(LOCK)
        .arg("status")
        .arg("--dir")
        .arg(s.lock())
        .arg("--json")
        .output()
        .unwrap();
    assert!(out.status.success());
    serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !cond() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting until {}",
            what
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A run of WAITKEY that holds what it holds until `stop`.
struct Running {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Running {
    fn start(mut command: Command) -> Running {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        Running { child, stdin }
    }

    fn pid(&self) -> u64 {
        u64::from(self.child.id())
    }

    /// Closes its standard input, which ends WAITKEY, and waits for it.
    fn stop(mut self) -> String {
        drop(self.stdin.take());
        let _ = self.child.wait();
        let mut stderr = String::new();
        if let Some(mut e) = self.child.stderr.take() {
            let _ = e.read_to_string(&mut stderr);
        }
        stderr
    }
}

fn holding(records: &[serde_json::Value], pid: u64) -> Option<&serde_json::Value> {
    records
        .iter()
        .find(|r| r["pid"] == pid && r["state"] == "holding")
}

#[test]
fn emulator_runs_hold_the_lock_shared_together() {
    let s = Scratch::new("shared");
    let a = Running::start(emulator(&s, &["--no-config"], "WAITKEY"));
    let b = Running::start(emulator(&s, &[], "WAITKEY"));
    let (pa, pb) = (a.pid(), b.pid());
    eventually("both runs hold the lock", || {
        let records = status(&s);
        holding(&records, pa).is_some() && holding(&records, pb).is_some()
    });
    let records = status(&s);
    let record = holding(&records, pa).unwrap();
    assert_eq!(record["mode"], "shared");
    assert_eq!(record["program"], "rust-dos");
    a.stop();
    b.stop();
    eventually("the holders are gone", || status(&s).is_empty());
}

#[test]
fn a_measurement_waits_for_runs_and_runs_wait_for_it() {
    let s = Scratch::new("measure");
    let shared = Running::start(emulator(&s, &[], "WAITKEY"));
    let sp = shared.pid();
    eventually("the run holds", || holding(&status(&s), sp).is_some());
    // An exclusive run with a limit gives up before it starts the
    // machine, with exit code 3.
    let out = run(emulator(
        &s,
        &["--run-lock-exclusive", "--run-lock-wait", "1"],
        "FAST",
    ));
    assert_eq!(out.status.code(), Some(3));
    assert!(
        out.stdout.is_empty(),
        "the program ran: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("run-lock: waiting for the exclusive lock"),
        "{}",
        stderr
    );
    assert!(
        stderr.contains(&format!("pid {} rust-dos", sp)),
        "{}",
        stderr
    );
    // A measurement waits for the run, and an emulator run inside it
    // inherits its hold.
    let emulator_exe = PathBuf::from(EMULATOR);
    let c = s.0.join("c");
    let inner: [&Path; 5] = [
        &emulator_exe,
        Path::new("run"),
        Path::new("-d"),
        &c,
        Path::new("WAITKEY"),
    ];
    let measurement = Running::start(wrapper(&s, "exclusive", &inner));
    let mp = measurement.pid();
    eventually("the measurement waits", || {
        status(&s)
            .iter()
            .any(|r| r["pid"] == mp && r["state"] == "waiting")
    });
    // A run that asks after it waits behind it, on the default wait.
    let later = Running::start(emulator(&s, &[], "WAITKEY"));
    let lp = later.pid();
    eventually("the later run waits", || {
        status(&s)
            .iter()
            .any(|r| r["pid"] == lp && r["state"] == "waiting")
    });
    shared.stop();
    eventually("the measurement holds", || {
        holding(&status(&s), mp).is_some()
    });
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        holding(&status(&s), lp).is_none(),
        "the later run got in beside the measurement"
    );
    // --no-run-lock runs whatever holds the lock.
    let out = run(emulator(&s, &["--no-run-lock"], "FAST"));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "OK");
    measurement.stop();
    eventually("the later run holds", || holding(&status(&s), lp).is_some());
    let stderr = later.stop();
    assert!(
        stderr.contains("run-lock: waiting for the shared lock"),
        "{}",
        stderr
    );
    assert!(
        stderr.contains(&format!("pid {} rust-dos-lock (rust-dos)", mp)),
        "{}",
        stderr
    );
}

#[test]
fn a_lock_that_cant_be_had_is_skipped_unless_asked_for() {
    let s = Scratch::new("unwritable");
    // A file where the directory would be.
    let not_a_dir = s.0.join("file");
    std::fs::write(&not_a_dir, "").unwrap();
    let mut command = emulator(&s, &[], "FAST");
    command.env("RUST_DOS_RUN_LOCK_DIR", &not_a_dir);
    let out = run(command);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "OK");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.matches("run-lock:").count(), 1, "{}", stderr);
    assert!(
        stderr.contains("running without the run lock"),
        "{}",
        stderr
    );
    let flag = not_a_dir.to_string_lossy().into_owned();
    let out = run(emulator(&s, &["--run-lock", &flag], "FAST"));
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
}

#[test]
fn the_wrapper_passes_on_the_exit_code() {
    let s = Scratch::new("exit");
    let emulator_exe = PathBuf::from(EMULATOR);
    let c = s.0.join("c");
    let inner: [&Path; 5] = [
        &emulator_exe,
        Path::new("run"),
        Path::new("-d"),
        &c,
        Path::new("WAITKEY"),
    ];
    let out = run(wrapper(&s, "shared", &inner));
    // WAITKEY, its input closed, is stopped with 255.
    assert_eq!(out.status.code(), Some(255));
    assert!(status(&s).is_empty());
}
