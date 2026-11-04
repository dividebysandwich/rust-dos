//! The machine-wide run lock as the emulator takes it (crates/runlock has
//! the protocol). Every run takes it shared unless told not to, so a
//! measurement that holds it exclusive runs alone. The default take is
//! best-effort: where the directory can't be written, rust-dos says so in
//! one line and runs without the lock. A run that asks for the lock with
//! `--run-lock`, `--run-lock-exclusive` or `--run-lock-wait` fails
//! instead.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use rust_dos::config::RunLockSetting;
use rust_dos_runlock::{self as runlock, Error, Mode, Request, Taken};

use crate::args::Args;

/// Takes the lock as `args` and the configuration file's `setting` say,
/// before any window or device opens. Ok(None) when the run goes without
/// it; Err(code) when rust-dos is to exit with `code`, having said why.
pub(crate) fn take(args: &Args, setting: Option<&RunLockSetting>) -> Result<Option<Taken>, i32> {
    let explicit =
        args.run_lock.is_some() || args.run_lock_exclusive || args.run_lock_wait.is_some();
    if args.no_run_lock || (!explicit && setting == Some(&RunLockSetting::Off)) {
        return Ok(None);
    }
    let Some(dir) = dir(args, setting) else {
        if explicit {
            eprintln!(
                "run-lock: no profile directory to keep the run lock in: give --run-lock DIR"
            );
            return Err(1);
        }
        eprintln!("run-lock: no profile directory to keep the run lock in; running without it");
        return Ok(None);
    };
    let mode = if args.run_lock_exclusive {
        Mode::Exclusive
    } else {
        Mode::Shared
    };
    let request = Request {
        wait: args
            .run_lock_wait
            .filter(|&secs| secs > 0)
            .map(Duration::from_secs),
        ..Request::new(mode, "rust-dos")
    };
    let start = Instant::now();
    let mut waited = false;
    let taken = runlock::take(&dir, &request, |holders| {
        waited = true;
        eprintln!(
            "run-lock: waiting for the {} lock in {}{}",
            mode.as_str(),
            dir.display(),
            if holders.is_empty() {
                String::new()
            } else {
                format!(": {}", runlock::list(holders))
            }
        );
    });
    match taken {
        Ok(taken) => {
            if waited {
                eprintln!(
                    "run-lock: got the {} lock after {}s",
                    mode.as_str(),
                    start.elapsed().as_secs()
                );
            }
            if let Taken::Held(hold) = &taken {
                export(hold.env());
            }
            Ok(Some(taken))
        }
        Err(e @ Error::Io { .. }) if !explicit => {
            eprintln!("run-lock: running without the run lock: {}", e);
            Ok(None)
        }
        Err(e) => {
            eprintln!("run-lock: {}", e);
            Err(e.exit_code())
        }
    }
}

/// Passes the hold on to the programs rust-dos starts (the printer's
/// commands, say), so that one of them that takes the lock inherits it.
fn export(vars: [(&'static str, std::ffi::OsString); 2]) {
    for (key, value) in vars {
        // SAFETY: called from the main thread at startup, before the
        // threads that could read the environment start (the debug
        // server's, the sound's, the printer's).
        unsafe { std::env::set_var(key, value) };
    }
}

/// Records the debug server in the hold's record, for status and for
/// the scripts that find a run by it.
pub(crate) fn record_server(
    taken: &mut Option<Taken>,
    server: Option<(String, std::net::SocketAddr)>,
) {
    if let (Some(Taken::Held(hold)), Some((instance, addr))) = (taken.as_mut(), server)
        && let Err(e) = hold.set_debug_server(Some(instance), Some(addr.to_string()))
    {
        eprintln!("run-lock: can't record the debug server: {}", e);
    }
}

/// The directory to lock: `--run-lock`'s, the configuration file's,
/// RUST_DOS_RUN_LOCK_DIR, or the profile directory's `run-lock`.
fn dir(args: &Args, setting: Option<&RunLockSetting>) -> Option<PathBuf> {
    args.run_lock
        .clone()
        .or_else(|| match setting {
            Some(RunLockSetting::Dir(dir)) => Some(dir.clone()),
            _ => None,
        })
        .or_else(runlock::default_dir)
}
