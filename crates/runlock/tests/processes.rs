//! The run lock between processes: this test program starts copies of
//! itself that take the lock (the `helper` test, which does nothing
//! unless RUNLOCK_HELPER is set), and wrappers that run them.

use rust_dos_runlock::{self as runlock, Mode, Request, State, Taken};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// What a helper does: `take MODE WAIT_MS SIGNAL` takes the lock, writes
/// SIGNAL.held (with "inherited" if it inherited the hold) and waits for
/// SIGNAL.release; `cli ARGS...` runs the wrapper with ARGS; `exit N`
/// exits with N; `touch FILE` writes FILE. A helper that a wrapper runs
/// (one with RUST_DOS_RUN_LOCK_HELD) does RUNLOCK_INNER's job instead.
const HELPER: &str = "RUNLOCK_HELPER";
const INNER: &str = "RUNLOCK_INNER";
const HELPER_LOCK_DIR: &str = "RUNLOCK_HELPER_DIR";
const HELPER_ARGS: [&str; 4] = ["--exact", "helper", "--nocapture", "--test-threads=1"];
const SEP: &str = "\u{1f}";

#[test]
fn helper() {
    let job = match (std::env::var(INNER), std::env::var_os(runlock::ENV_HELD)) {
        (Ok(inner), Some(_)) => inner,
        _ => match std::env::var(HELPER) {
            Ok(job) => job,
            Err(_) => return,
        },
    };
    let dir = PathBuf::from(std::env::var(HELPER_LOCK_DIR).unwrap());
    let words: Vec<&str> = job.split(SEP).collect();
    match words[0] {
        "take" => {
            let mode = Mode::parse(words[1]).unwrap();
            let wait = words[2].parse::<u64>().unwrap();
            let signal = PathBuf::from(words[3]);
            let request = Request {
                wait: (wait > 0).then(|| Duration::from_millis(wait)),
                poll: Duration::from_millis(20),
                ..Request::new(mode, "helper")
            };
            let waiting = signal.with_extension("waiting");
            let taken = runlock::take(&dir, &request, |_| {
                let _ = std::fs::write(&waiting, "");
            });
            match taken {
                Ok(taken) => {
                    let what = if let Taken::Inherited(_) = taken {
                        "inherited"
                    } else {
                        "held"
                    };
                    std::fs::write(signal.with_extension("held"), what).unwrap();
                    let release = signal.with_extension("release");
                    while !release.exists() {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                Err(e) => std::process::exit(e.exit_code()),
            }
        }
        "cli" => {
            let args = words[1..].iter().map(Into::into).collect();
            std::process::exit(runlock::cli::main("rust-dos-lock", args));
        }
        "exit" => std::process::exit(words[1].parse().unwrap()),
        "touch" => std::fs::write(words[1], "").unwrap(),
        other => panic!("unknown helper job {}", other),
    }
}

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!(
            "rust-dos-runlock-proc-{}-{}",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Scratch { root }
    }

    fn lock(&self) -> PathBuf {
        self.root.join("lock")
    }

    fn signal(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn helper_command(lock: &Path, job: &[&str]) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(HELPER_ARGS);
    command.env(HELPER, job.join(SEP));
    command.env(HELPER_LOCK_DIR, lock);
    command.env_remove(runlock::ENV_HELD);
    command.env_remove(runlock::ENV_DIR);
    command.env_remove(INNER);
    command.stdout(std::process::Stdio::null());
    command
}

/// The job words that make a helper take the lock.
fn take_job(mode: &str, wait_ms: u64, signal: &Path) -> Vec<String> {
    vec![
        "take".into(),
        mode.into(),
        wait_ms.to_string(),
        signal.to_string_lossy().into_owned(),
    ]
}

fn spawn_take(s: &Scratch, mode: &str, name: &str) -> Child {
    let job = take_job(mode, 0, &s.signal(name));
    let job: Vec<&str> = job.iter().map(String::as_str).collect();
    helper_command(&s.lock(), &job).spawn().unwrap()
}

/// A helper that runs the wrapper in `mode` with `options`, around a
/// helper doing `inner`.
fn wrapper(s: &Scratch, mode: &str, options: &[&str], inner: &[&str]) -> Command {
    let lock = s.lock().to_string_lossy().into_owned();
    let exe = std::env::current_exe()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut words = vec!["cli", mode, "--dir", &lock];
    words.extend(options);
    words.push("--");
    words.push(&exe);
    words.extend(HELPER_ARGS);
    let mut command = helper_command(&s.lock(), &words);
    command.env(INNER, inner.join(SEP));
    command
}

fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !cond() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting until {}",
            what
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn held(s: &Scratch, name: &str) -> bool {
    s.signal(name).with_extension("held").exists()
}

fn waits(s: &Scratch, name: &str) -> bool {
    s.signal(name).with_extension("waiting").exists()
}

fn release(s: &Scratch, name: &str, child: &mut Child) {
    std::fs::write(s.signal(name).with_extension("release"), "").unwrap();
    assert!(child.wait().unwrap().success());
}

/// For a moment, that `name` hasn't got the lock.
fn stays_out(s: &Scratch, name: &str) {
    std::thread::sleep(Duration::from_millis(500));
    assert!(!held(s, name), "{} got the lock", name);
}

#[test]
fn shared_holders_run_at_once() {
    let s = Scratch::new("shared");
    let mut a = spawn_take(&s, "shared", "a");
    let mut b = spawn_take(&s, "shared", "b");
    eventually("both hold", || held(&s, "a") && held(&s, "b"));
    let listed = runlock::holders(&s.lock()).unwrap();
    assert_eq!(listed.len(), 2);
    assert!(
        listed
            .iter()
            .all(|h| h.record.state == State::Holding && h.record.mode == Mode::Shared)
    );
    let pids: Vec<u32> = listed.iter().map(|h| h.record.pid).collect();
    assert!(pids.contains(&a.id()) && pids.contains(&b.id()));
    release(&s, "a", &mut a);
    release(&s, "b", &mut b);
    assert!(runlock::holders(&s.lock()).unwrap().is_empty());
}

#[test]
fn exclusive_waits_and_keeps_later_shared_out() {
    let s = Scratch::new("gate");
    let mut a = spawn_take(&s, "shared", "a");
    eventually("a holds", || held(&s, "a"));
    let mut m = spawn_take(&s, "exclusive", "m");
    eventually("the measurement waits", || waits(&s, "m"));
    stays_out(&s, "m");
    let listed = runlock::holders(&s.lock()).unwrap();
    assert!(
        listed
            .iter()
            .any(|h| h.record.pid == m.id() && h.record.state == State::Waiting)
    );
    // A shared run that asks after the measurement waits behind it.
    let mut b = spawn_take(&s, "shared", "b");
    eventually("b waits", || waits(&s, "b"));
    stays_out(&s, "b");
    release(&s, "a", &mut a);
    eventually("the measurement holds", || held(&s, "m"));
    stays_out(&s, "b");
    release(&s, "m", &mut m);
    eventually("b holds", || held(&s, "b"));
    release(&s, "b", &mut b);
}

#[test]
fn a_killed_holder_lets_go() {
    let s = Scratch::new("killed");
    let mut a = spawn_take(&s, "exclusive", "a");
    eventually("a holds", || held(&s, "a"));
    let pid = a.id();
    a.kill().unwrap();
    a.wait().unwrap();
    // Windows may let go of a dead process's locks a moment later; the
    // take polls until then.
    let mut b = spawn_take(&s, "shared", "b");
    eventually("b holds", || held(&s, "b"));
    let listed = runlock::holders(&s.lock()).unwrap();
    assert!(
        listed.iter().all(|h| h.record.pid != pid),
        "the dead holder is listed: {:?}",
        listed
    );
    let files: Vec<String> = std::fs::read_dir(s.lock().join(runlock::HOLDERS_DIR))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        files.iter().all(|f| !f.starts_with(&format!("{}.", pid))),
        "files left: {:?}",
        files
    );
    release(&s, "b", &mut b);
}

#[test]
fn a_wait_with_a_limit_exits_3() {
    let s = Scratch::new("timeout");
    let mut a = spawn_take(&s, "shared", "a");
    eventually("a holds", || held(&s, "a"));
    let job = take_job("exclusive", 300, &s.signal("m"));
    let job: Vec<&str> = job.iter().map(String::as_str).collect();
    let status = helper_command(&s.lock(), &job).status().unwrap();
    assert_eq!(status.code(), Some(runlock::EXIT_TIMED_OUT));
    assert!(!held(&s, "m"));
    // The wrapper doesn't run the command when it gives up.
    let marker = s.signal("ran");
    let marker_arg = marker.to_string_lossy().into_owned();
    let out = wrapper(&s, "exclusive", &["--wait", "1"], &["touch", &marker_arg])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(runlock::EXIT_TIMED_OUT));
    assert!(!marker.exists());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("run-lock: waiting for the exclusive lock"),
        "{}",
        stderr
    );
    assert!(stderr.contains("did not run"), "{}", stderr);
    assert!(
        stderr.contains(&format!("pid {}", a.id())),
        "the holder is named: {}",
        stderr
    );
    // Nothing of the timed-out takes is left.
    assert_eq!(runlock::holders(&s.lock()).unwrap().len(), 1);
    release(&s, "a", &mut a);
}

#[test]
fn the_wrapper_passes_on_the_exit_code_and_the_hold() {
    let s = Scratch::new("wrapper");
    let run = |mode: &str, inner: &[&str]| wrapper(&s, mode, &[], inner).output().unwrap();
    // Exit codes pass through.
    let out = run("shared", &["exit", "7"]);
    assert_eq!(
        out.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Shared under exclusive, and exclusive under exclusive, inherit: the
    // inner take would otherwise wait for the gate its parent holds.
    for (outer, inner) in [
        ("exclusive", "shared"),
        ("exclusive", "exclusive"),
        ("shared", "shared"),
    ] {
        let signal = s.signal(&format!("{}-{}", outer, inner));
        std::fs::write(signal.with_extension("release"), "").unwrap();
        let job = take_job(inner, 5000, &signal);
        let job: Vec<&str> = job.iter().map(String::as_str).collect();
        let out = run(outer, &job);
        assert!(
            out.status.success(),
            "{} under {}: {}",
            inner,
            outer,
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(signal.with_extension("held")).unwrap(),
            "inherited"
        );
    }
    // Exclusive under shared is a conflict.
    let signal = s.signal("shared-exclusive");
    let job = take_job("exclusive", 5000, &signal);
    let job: Vec<&str> = job.iter().map(String::as_str).collect();
    let out = run("shared", &job);
    assert_eq!(out.status.code(), Some(runlock::EXIT_TIMED_OUT));
    assert!(!held(&s, "shared-exclusive"));
    assert!(runlock::holders(&s.lock()).unwrap().is_empty());
}
