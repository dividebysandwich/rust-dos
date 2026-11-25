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
/// A handle is kept for `KEEP` at most, and `MAX` of them, and is given to
/// an open only of the same host path with the same access (`take`), while
/// the file there is still the one it wrote: the same size and time, and
/// not read-only. A call that deletes or renames a file or a folder drops
/// the handles kept under it first (`drop_under`), so the host never sees
/// a file still open that the guest has closed and goes on to delete,
/// rename or replace; so does anything that changes host folders behind
/// the guest's back (`HostDirCache::forget`). Handles are opened sharing
/// reading, writing and deleting (`std::fs` on Windows), so another
/// program on the host can open, replace or delete the file meanwhile, and
/// then the size or time differs and the handle isn't used.
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
            && closed.file.len().is_ok_and(|len| len == meta.len)
            && meta.modified.is_some()
            && closed.file.modified() == meta.modified;
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

    /// Close the handles kept longer than `KEEP`.
    fn expire(&self) {
        let mut kept = self.kept.borrow_mut();
        while kept.front().is_some_and(|c| c.at.elapsed() >= Self::KEEP) {
            kept.pop_front();
        }
    }

    /// How many handles are kept, for tests.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.kept.borrow().len()
    }
}
