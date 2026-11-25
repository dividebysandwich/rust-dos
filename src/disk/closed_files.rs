//! The host handles of files the guest wrote and closed, kept a moment
//! for an open that follows.

use super::*;
use std::collections::VecDeque;
use std::time::Duration;
use web_time::Instant;

/// The host handles of the files the guest wrote and closed last, kept
/// open for a moment so that opening such a file again takes no new host
/// open.
///
/// On Windows a real-time virus scanner looks at a file the first time it
/// is opened after it was written: Dark Omen's setup writes each file it
/// copies, closes it, and opens it again at once to set its time, and that
/// open took 9 to 12 ms of the host's time where any other open takes
/// well under one (#1804). Its copy spent more than half of the host's
/// time in those opens. Opening the file through the handle that wrote it
/// asks the host nothing.
///
/// Only a handle that wrote is kept: a file opened for writing and only
/// read has nothing for a scanner to look at again. `MAX` handles are
/// kept at most, each for `KEEP`: the frame loop closes the older ones
/// (`DiskController::close_idle_files`), as do `keep` and `take`. A
/// handle serves only an open of the same host path with the same access
/// (`take`), while the file there is still the one it wrote
/// (`hostfs::File::is_at`: the same size and time, and on Unix the same
/// inode) and isn't read-only. A call that deletes or renames a file or a
/// folder drops the handles kept under it first (`drop_under`), so the
/// host never sees a file still open that the guest has closed and goes
/// on to delete, rename or replace; so does anything that changes host
/// folders behind the guest's back (`HostDirCache::forget`), and closing
/// every file before a boot or a reload of the shell. Handles are opened
/// sharing reading, writing and deleting (`std::fs` on Windows), so
/// another program on the host can open, replace or delete the file
/// meanwhile, and then the handle isn't used. A program that opens the
/// file without sharing writing can't, for the second it is kept.
#[derive(Default)]
pub(super) struct ClosedFiles {
    /// The oldest first.
    kept: RefCell<VecDeque<Closed>>,
}

/// A host file the guest closed, open still.
struct Closed {
    path: PathBuf,
    file: File,
    /// Whether it reads as well as writes.
    read: bool,
    /// When the guest closed it.
    at: Instant,
}

impl ClosedFiles {
    /// How long a closed file's handle is kept.
    const KEEP: Duration = Duration::from_secs(1);
    /// The most handles kept; the oldest goes first.
    const MAX: usize = 8;

    /// The guest closed the host file `path`, which it opened for writing,
    /// and for reading too if `read`: keep `file` for an open that
    /// follows. Only files on folders `std::fs` reaches are kept.
    pub(super) fn keep(&self, path: PathBuf, file: File, read: bool) {
        if hostfs::is_foreign(&path) {
            return;
        }
        let mut kept = self.kept.borrow_mut();
        kept.retain(|c| c.path != path);
        if kept.len() >= Self::MAX {
            kept.pop_front();
        }
        kept.push_back(Closed {
            path,
            file,
            read,
            at: Instant::now(),
        });
        drop(kept);
        self.expire();
    }

    /// The kept handle of the host file `path` for an open with access
    /// `read` and `write`, if there is one and the file there, as `meta`
    /// describes it, is the one it wrote. A handle that doesn't serve is
    /// closed. Its position is at the start.
    pub(super) fn take(
        &self,
        path: &Path,
        read: bool,
        write: bool,
        meta: &hostfs::Meta,
    ) -> Option<File> {
        self.expire();
        let mut kept = self.kept.borrow_mut();
        let at = kept.iter().position(|c| c.path == path)?;
        let closed = kept.remove(at)?;
        drop(kept);
        let same = closed.read == read
            && write
            && !meta.is_dir
            && !meta.readonly
            && closed.file.is_at(path, meta);
        let file = closed.file;
        if !same || (&file).seek(SeekFrom::Start(0)).is_err() {
            return None;
        }
        Some(file)
    }

    /// Close the handles kept of `path` and of the files under it: the
    /// guest is about to delete, rename or replace it.
    pub(super) fn drop_under(&self, path: &Path) {
        self.kept.borrow_mut().retain(|c| !c.path.starts_with(path));
    }

    /// Close every handle kept.
    pub(super) fn clear(&self) {
        self.kept.borrow_mut().clear();
    }

    /// The host paths whose handles are kept.
    pub(super) fn paths(&self) -> Vec<PathBuf> {
        self.kept.borrow().iter().map(|c| c.path.clone()).collect()
    }

    /// Close the handles kept longer than `KEEP`: on every `keep` and
    /// `take`, and once a frame (`DiskController::close_idle_files`).
    pub(super) fn expire(&self) {
        let mut kept = self.kept.borrow_mut();
        while kept.front().is_some_and(|c| c.at.elapsed() >= Self::KEEP) {
            kept.pop_front();
        }
    }

    /// Make every handle kept look `KEEP` old, for tests.
    #[cfg(test)]
    pub(super) fn age(&self) {
        for closed in self.kept.borrow_mut().iter_mut() {
            closed.at = closed.at.checked_sub(Self::KEEP).unwrap_or(closed.at);
        }
    }

    /// How many handles are kept, for tests.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.kept.borrow().len()
    }
}
