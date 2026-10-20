//! Where unit tests write their files: under one folder in the temp
//! directory for the whole test process, so that a test that fails
//! before its own clean-up, or has none, leaves nothing behind.
//!
//! The process deletes its folder when it exits through the C library's
//! `exit`: always on Unix, and on Windows when all tests passed (libtest
//! ends a failed run with `ExitProcess`, which skips `atexit`). For the
//! runs that end otherwise, failed or killed, the folder holds a lock
//! file the process keeps locked while it lives, and the next test
//! process deletes every folder whose lock it can take.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const PREFIX: &str = "rust-dos-tests-";

static ROOT: OnceLock<(PathBuf, File)> = OnceLock::new();

unsafe extern "C" {
    fn atexit(callback: extern "C" fn()) -> std::ffi::c_int;
}

extern "C" fn remove_root() {
    if let Some((root, _)) = ROOT.get() {
        let _ = std::fs::remove_dir_all(root);
    }
}

/// Deletes the folders of test processes that are gone.
fn remove_stale() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(PREFIX) {
            continue;
        }
        let dir = entry.path();
        // A folder without its lock, or with one made less than a minute
        // ago, may be one whose process hasn't taken the lock yet.
        let Ok(lock) = File::open(dir.join("lock")) else {
            continue;
        };
        let age = lock
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok());
        if age.is_some_and(|age| age.as_secs() >= 60) && lock.try_lock().is_ok() {
            drop(lock);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

fn root() -> &'static Path {
    &ROOT
        .get_or_init(|| {
            remove_stale();
            let root = std::env::temp_dir().join(format!("{PREFIX}{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let lock = File::create(root.join("lock")).unwrap();
            lock.lock().unwrap();
            // SAFETY: `remove_root` is an `extern "C"` function that takes
            // nothing, as `atexit` wants; it only deletes ROOT's folder.
            unsafe { atexit(remove_root) };
            (root, lock)
        })
        .0
}

/// `name` in the process's folder, which exists by then. The test makes
/// the file or folder itself.
pub fn path(name: &str) -> PathBuf {
    root().join(name)
}
