//! A mark in rust-dos's directory while a host renderer is set up, so
//! that a run after one the graphics driver crashed in that setup draws
//! in software instead of crashing the same way at every start.
//!
//! The setup compiles shaders and draws a test on the driver: a driver
//! that crashes there takes the whole program with it, and nothing in
//! the program can catch that. So `begin` writes a key, the rust-dos
//! version and the OpenGL driver, to a file, and the guard it returns
//! removes the file once the setup and the first frame after it are
//! done. A file left with the same key means the run that wrote it ended
//! in between. A new driver or a new rust-dos has another key, and is
//! tried again.
//!
//! The file is locked while it is in use, and a lock ends with its
//! process: a file another run holds locked is that run's setup, which
//! is left alone.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// What `begin` found.
pub enum Begin {
    /// Set the renderer up, with the guard to drop when it is done
    /// (None where there is no file to keep the mark in).
    Go(Option<SetupGuard>),
    /// A run with the same key ended in the setup.
    Crashed,
}

/// The mark of a setup going on: dropped, it goes.
pub struct SetupGuard {
    file: File,
    path: PathBuf,
}

/// Start a setup of `key` marked in the file `name` of `dir`. With
/// `force` it goes ahead after a crash too, marked again.
pub fn begin(dir: Option<&Path>, name: &str, key: &str, force: bool) -> Begin {
    let Some(dir) = dir else {
        return Begin::Go(None);
    };
    let path = dir.join(name);
    let opened = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path);
    let Ok(mut file) = opened else {
        return Begin::Go(None);
    };
    match file.try_lock() {
        Ok(()) => {}
        // Another run is setting its renderer up, or the file can't be
        // locked here: go on without the mark.
        Err(TryLockError::WouldBlock | TryLockError::Error(_)) => return Begin::Go(None),
    }
    let mut left = String::new();
    let _ = file.read_to_string(&mut left);
    if left.trim_end() == key && !force {
        return Begin::Crashed;
    }
    let marked = file.set_len(0).is_ok()
        && file.seek(SeekFrom::Start(0)).is_ok()
        && file.write_all(key.as_bytes()).is_ok()
        && file.sync_data().is_ok();
    let guard = SetupGuard { file, path };
    Begin::Go(marked.then_some(guard))
}

impl Drop for SetupGuard {
    fn drop(&mut self) {
        // Removed while it is still locked, so no other run marks the
        // file in between. (Windows lets an open file go: Rust opens
        // files sharing their deletion.)
        let _ = self.file.set_len(0);
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rust-dos-setup-guard-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const NAME: &str = "renderer-setup";

    #[test]
    fn a_setup_that_ends_leaves_nothing() {
        let dir = dir("ends");
        let Begin::Go(Some(guard)) = begin(Some(&dir), NAME, "1.9.0 driver A", false) else {
            panic!("no guard");
        };
        assert!(dir.join(NAME).is_file());
        drop(guard);
        assert!(!dir.join(NAME).exists());
        assert!(matches!(
            begin(Some(&dir), NAME, "1.9.0 driver A", false),
            Begin::Go(Some(_))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_setup_another_run_has_going_is_left_alone() {
        let dir = dir("busy");
        let Begin::Go(Some(guard)) = begin(Some(&dir), NAME, "1.9.0 driver A", false) else {
            panic!("no guard");
        };
        // The file is locked: its key is the other run's, which hasn't
        // crashed.
        assert!(matches!(
            begin(Some(&dir), NAME, "1.9.0 driver A", false),
            Begin::Go(None)
        ));
        drop(guard);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_setup_that_crashed_is_not_tried_again_on_the_same_driver() {
        let dir = dir("crashed");
        // What a run that crashed in the setup leaves: the key, with the
        // lock gone with the process.
        std::fs::write(dir.join(NAME), "1.9.0 driver A").unwrap();
        assert!(matches!(
            begin(Some(&dir), NAME, "1.9.0 driver A", false),
            Begin::Crashed
        ));
        // The mark stays for the next run.
        assert!(matches!(
            begin(Some(&dir), NAME, "1.9.0 driver A", false),
            Begin::Crashed
        ));
        // Asked for, the renderer is tried on the driver that crashed.
        let Begin::Go(Some(guard)) = begin(Some(&dir), NAME, "1.9.0 driver A", true) else {
            panic!("no guard when forced");
        };
        drop(guard);
        // Another driver, or another rust-dos, is tried.
        std::fs::write(dir.join(NAME), "1.9.0 driver A").unwrap();
        let Begin::Go(Some(guard)) = begin(Some(&dir), NAME, "1.9.0 driver B", false) else {
            panic!("no guard for another driver");
        };
        drop(guard);
        assert!(!dir.join(NAME).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_directory_means_no_mark() {
        assert!(matches!(begin(None, NAME, "key", false), Begin::Go(None)));
    }
}
