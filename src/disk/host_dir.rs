//! The cache of host folder entries and FindNext listings.

use super::*;
use web_time::Instant;

/// The entries of host folders as last read, with their DOS names. Each
/// component of a DOS path is looked up among the entries of its folder,
/// so without this every open, attribute call and FindNext reads and sorts
/// every folder on its path.
///
/// A folder's entries are read again when the guest changed anything on a
/// host drive since (`changed`), when the folder's modification time moved
/// (another program added, removed or renamed something in it), and while
/// that time is under `RECENT` old, too close to tell a change made in the
/// same tick of a coarse clock (2 seconds on FAT) from the read. Folders
/// that only a backend reaches (the libretro frontend's, a write overlay's
/// layer) aren't kept: their times aren't known.
///
/// Checking a folder's time costs a stat, and a path names several
/// folders, so a folder whose time was checked less than `TRUST` ago is
/// used without checking it again. Another program's change shows within
/// `TRUST`; the guest's own changes show at once.
///
/// A folder's time doesn't always move when another program changes it:
/// Windows doesn't update it on FAT and exFAT, and unpackers and copy tools
/// set it back. So a name that isn't among a folder's kept entries is
/// looked for again in a fresh read (`reread`) before it counts as missing.
/// The names that a read less than `TRUST` ago didn't have are noted
/// (`missing`) and count as missing without another read: programs poll
/// for files that aren't there (Sub Culture opens a missing sound file
/// fifty times a second), and a read of a large folder takes far longer
/// than the call should.
///
/// The listings of the last few searches are kept the same way, for
/// FindNext (`DiskController::find_directory_entry`), whatever their size:
/// a listing stats each entry. FindFirst always lists afresh, so where a
/// folder's time doesn't move, a file another program adds mid-search
/// shows on the next FindFirst.
#[derive(Default)]
pub(super) struct HostDirCache {
    /// Counts the guest's changes to the names in host folders.
    pub(super) changes: Cell<u64>,
    pub(super) dirs: RefCell<HashMap<PathBuf, HostDir>>,
    /// The most recent search last. The guest's changes drop the searches
    /// of the folders they were made in (`touched`).
    pub(super) searches: RefCell<Vec<HostSearch>>,
    /// What the files in host folders take, as counted for the free space
    /// of their drives, by (folder, cluster size, most counted). Counting
    /// stats every file, so a folder's count is kept as its entries are:
    /// until the guest changes something in it (`touched`) or its time
    /// moves, and not while that time is `RECENT`. A count after a write
    /// then stats only the files of the folder written to. A file another
    /// program resizes shows after the guest's next change in its folder.
    pub(super) usage: RefCell<HashMap<(PathBuf, u64, u64), DirUsage>>,
}

/// The clusters a host folder's own files take, and its folders.
pub(super) type Usage = (u64, Rc<[PathBuf]>);

/// A folder's `Usage`, and its modification time when it was counted.
pub(super) struct DirUsage {
    modified: std::time::SystemTime,
    usage: Usage,
}

/// A search of a host folder, as FindFirst listed it.
pub(super) struct HostSearch {
    pub(super) key: SearchKey,
    modified: std::time::SystemTime,
    entries: Rc<[DosDirEntry]>,
}

/// What a search lists: the matches of `pattern` (in the 8.3 form the DTA
/// keeps for FindNext) among the entries of the host folder `dir` with
/// search attributes `attr`, on a drive that is writable or not, and that
/// has `dir` as its root (no "." and ".." entries) or not.
#[derive(PartialEq, Eq)]
pub(super) struct SearchKey {
    pub(super) dir: PathBuf,
    pub(super) pattern: [u8; 11],
    pub(super) attr: u16,
    pub(super) writable: bool,
    pub(super) root: bool,
}

pub(super) struct HostDir {
    modified: std::time::SystemTime,
    changes: u64,
    /// When the folder's time was last found to be `modified`.
    checked: Instant,
    /// When `entries` were read.
    read: Instant,
    entries: Rc<[(String, String)]>,
    /// Names, uppercased, that were looked up and aren't in `entries`.
    missing: Vec<String>,
}

impl HostDirCache {
    /// How recent a folder's modification time can be for its entries to
    /// be kept.
    const RECENT: std::time::Duration = std::time::Duration::from_secs(2);
    /// How long a folder's entries are used without checking its time, and
    /// how long a name a read didn't have counts as missing without
    /// reading the folder again.
    const TRUST: std::time::Duration = std::time::Duration::from_secs(1);
    /// Folders kept at most; one is dropped to make room for another.
    const MAX_DIRS: usize = 64;
    /// Searches kept at most: a program walking a tree has one open per
    /// level.
    const MAX_SEARCHES: usize = 8;
    /// Missing names noted per folder at most; the oldest goes first.
    const MAX_MISSING: usize = 32;

    /// The guest created, wrote, deleted or renamed something on a host
    /// drive: every folder is read again.
    pub(super) fn changed(&self) {
        self.changes.set(self.changes.get().wrapping_add(1));
    }

    /// Host folders changed outside the guest's file calls (a state was
    /// loaded, a shared disk copied back): nothing kept is used again.
    pub(super) fn forget(&self) {
        self.changed();
        self.searches.borrow_mut().clear();
        self.usage.borrow_mut().clear();
    }

    /// The guest created, wrote, resized, deleted or renamed the host file
    /// or folder `path`, or changed its attributes: the searches of its
    /// folder, and of `path` and the folders under it, list them again.
    /// Searches of other folders are kept, so a program copying the files
    /// it finds into another folder doesn't list its source each time. The
    /// files of its folder, and of `path`, are counted again.
    pub(super) fn touched(&self, path: &Path) {
        let parent = path.parent();
        self.usage
            .borrow_mut()
            .retain(|(dir, ..), _| Some(dir.as_path()) != parent && dir != path);
        self.searches.borrow_mut().retain(|s| Some(s.key.dir.as_path()) != parent && !s.key.dir.starts_with(path));
    }

    /// What the files in the host folder `dir` take with clusters of
    /// `cluster_bytes`, counted up to `cap`, kept or from `count`.
    pub(super) fn usage(
        &self,
        dir: &Path,
        cluster_bytes: u64,
        cap: u64,
        count: impl FnOnce() -> Usage,
    ) -> Usage {
        let modified = Self::settled(dir);
        let key = (dir.to_path_buf(), cluster_bytes, cap);
        let mut kept = self.usage.borrow_mut();
        if let Some(found) = kept.get(&key)
            && Some(found.modified) == modified
        {
            return found.usage.clone();
        }
        let usage = count();
        match modified {
            Some(modified) => {
                let usage = usage.clone();
                kept.insert(key, DirUsage { modified, usage });
            }
            None => {
                kept.remove(&key);
            }
        }
        usage
    }

    /// The modification time of the host folder `dir`, if what was read of
    /// it can be kept: it is a folder `std::fs` reaches, and the time is
    /// `RECENT` or more in the past.
    fn settled(dir: &Path) -> Option<std::time::SystemTime> {
        if hostfs::has_scheme(dir) {
            return None;
        }
        hostfs::metadata(dir)
            .ok()
            .filter(|m| m.is_dir)
            .and_then(|m| m.modified)
            .filter(|&at| at.elapsed().is_ok_and(|age| age >= Self::RECENT))
    }

    /// The entries of `dir` from `read`, kept in place of the ones kept:
    /// what was kept lacked a name the guest looked for. The folder's time
    /// is taken again first, and a folder changed within `RECENT` isn't
    /// kept any more. The missing names noted stay noted if the entries
    /// came out the same.
    pub(super) fn reread(&self, dir: &Path, read: impl Fn() -> Vec<(String, String)>) -> Rc<[(String, String)]> {
        let modified = Self::settled(dir);
        let entries: Rc<[(String, String)]> = read().into();
        let mut dirs = self.dirs.borrow_mut();
        match modified {
            Some(modified) => {
                if let Some(kept) = dirs.get_mut(dir) {
                    let now = Instant::now();
                    if kept.entries != entries {
                        kept.missing.clear();
                    }
                    kept.modified = modified;
                    kept.checked = now;
                    kept.read = now;
                    kept.entries = entries.clone();
                }
            }
            None => {
                dirs.remove(dir);
            }
        }
        entries
    }

    /// Whether the name `upper` (uppercased) was looked up in the kept
    /// folder `dir` and not found in a read less than `TRUST` ago, with no
    /// change by the guest since.
    pub(super) fn known_missing(&self, dir: &Path, upper: &str) -> bool {
        self.dirs.borrow().get(dir).is_some_and(|kept| {
            kept.changes == self.changes.get()
                && kept.read.elapsed() < Self::TRUST
                && kept.missing.iter().any(|name| name == upper)
        })
    }

    /// The name `upper` (uppercased) isn't among the entries of the kept
    /// folder `dir`, which were just read.
    pub(super) fn note_missing(&self, dir: &Path, upper: &str) {
        if let Some(kept) = self.dirs.borrow_mut().get_mut(dir)
            && !kept.missing.iter().any(|name| name == upper)
        {
            if kept.missing.len() >= Self::MAX_MISSING {
                kept.missing.remove(0);
            }
            kept.missing.push(upper.to_string());
        }
    }

    /// The entries of `dir`, kept or from `read`, and whether they are the
    /// kept ones.
    pub(super) fn entries(&self, dir: &Path, read: impl Fn() -> Vec<(String, String)>) -> (Rc<[(String, String)]>, bool) {
        let changes = self.changes.get();
        let trusted = self
            .dirs
            .borrow()
            .get(dir)
            .filter(|k| k.changes == changes && k.checked.elapsed() < Self::TRUST)
            .map(|k| k.entries.clone());
        if let Some(entries) = trusted {
            return (entries, true);
        }
        let Some(modified) = Self::settled(dir) else {
            self.dirs.borrow_mut().remove(dir);
            return (read().into(), false);
        };
        let mut dirs = self.dirs.borrow_mut();
        if let Some(kept) = dirs
            .get_mut(dir)
            .filter(|k| k.modified == modified && k.changes == changes)
        {
            kept.checked = Instant::now();
            return (kept.entries.clone(), true);
        }
        drop(dirs);
        // Read after the time was taken, so that a change made in between
        // shows as a newer time on the next call.
        let entries: Rc<[(String, String)]> = read().into();
        let mut dirs = self.dirs.borrow_mut();
        if dirs.len() >= Self::MAX_DIRS
            && !dirs.contains_key(dir)
            && let Some(dropped) = dirs.keys().next().cloned()
        {
            dirs.remove(&dropped);
        }
        let now = Instant::now();
        let kept = HostDir {
            modified,
            changes,
            checked: now,
            read: now,
            entries: entries.clone(),
            missing: Vec::new(),
        };
        dirs.insert(dir.to_path_buf(), kept);
        (entries, false)
    }

    /// Makes what is kept as old as `TRUST`, as if that time had passed:
    /// the next look at each folder checks its time again.
    #[cfg(test)]
    pub(super) fn expire(&self) {
        for kept in self.dirs.borrow_mut().values_mut() {
            if let Some(then) = kept.checked.checked_sub(Self::TRUST) {
                kept.checked = then;
            }
            if let Some(then) = kept.read.checked_sub(Self::TRUST) {
                kept.read = then;
            }
        }
    }

    /// The listing of the search `key`: the one kept, unless `first` (a
    /// new search lists the folder as it is now), or from `list`.
    pub(super) fn search(
        &self,
        key: SearchKey,
        first: bool,
        list: impl FnOnce() -> Result<Vec<DosDirEntry>, u8>,
    ) -> Result<Rc<[DosDirEntry]>, u8> {
        let mut searches = self.searches.borrow_mut();
        let at = searches.iter().position(|s| s.key == key);
        let kept = at.map(|i| searches.remove(i));
        let Some(modified) = Self::settled(&key.dir) else {
            return list().map(Into::into);
        };
        let entries = match kept {
            Some(kept) if !first && kept.modified == modified => kept.entries,
            _ => list()?.into(),
        };
        if searches.len() >= Self::MAX_SEARCHES {
            searches.remove(0);
        }
        searches.push(HostSearch { key, modified, entries: entries.clone() });
        Ok(entries)
    }
}
