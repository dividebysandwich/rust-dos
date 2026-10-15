//! The cache of host folder entries and FindNext listings.

use super::*;

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
/// layer) aren't kept: their times aren't known. Nor are folders of fewer
/// than `MIN_ENTRIES` entries, which take no longer to read than to check.
///
/// A folder's time doesn't always move when another program changes it:
/// Windows doesn't update it on FAT and exFAT, and unpackers and copy tools
/// set it back. So a name that isn't among a folder's kept entries is
/// looked for again in a fresh read (`reread`) before it counts as missing.
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
    entries: Rc<[(String, String)]>,
}

impl HostDirCache {
    /// How recent a folder's modification time can be for its entries to
    /// be kept.
    const RECENT: std::time::Duration = std::time::Duration::from_secs(2);
    /// Folders kept at most; one is dropped to make room for another.
    const MAX_DIRS: usize = 64;
    /// Searches kept at most: a program walking a tree has one open per
    /// level.
    const MAX_SEARCHES: usize = 8;
    /// The fewest entries a folder is kept with. Checking a folder's time
    /// costs a stat, which on Windows takes about as long as reading a
    /// folder of a dozen entries.
    pub(super) const MIN_ENTRIES: usize = 16;

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
    }

    /// The guest created, wrote, resized, deleted or renamed the host file
    /// or folder `path`, or changed its attributes: the searches of its
    /// folder, and of `path` and the folders under it, list them again.
    /// Searches of other folders are kept, so a program copying the files
    /// it finds into another folder doesn't list its source each time.
    pub(super) fn touched(&self, path: &Path) {
        let parent = path.parent();
        self.searches.borrow_mut().retain(|s| Some(s.key.dir.as_path()) != parent && !s.key.dir.starts_with(path));
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
    /// what was kept lacked a name the guest looked for.
    pub(super) fn reread(&self, dir: &Path, read: impl Fn() -> Vec<(String, String)>) -> Rc<[(String, String)]> {
        let entries: Rc<[(String, String)]> = read().into();
        if let Some(kept) = self.dirs.borrow_mut().get_mut(dir) {
            kept.entries = entries.clone();
        }
        entries
    }

    /// The entries of `dir`, kept or from `read`, and whether they are the
    /// kept ones.
    pub(super) fn entries(&self, dir: &Path, read: impl Fn() -> Vec<(String, String)>) -> (Rc<[(String, String)]>, bool) {
        // Small folders are read each time; a first read tells.
        let mut first = None;
        if !self.dirs.borrow().contains_key(dir) {
            let entries = read();
            if entries.len() < Self::MIN_ENTRIES {
                return (entries.into(), false);
            }
            first = Some(entries);
        }
        let Some(modified) = Self::settled(dir) else {
            self.dirs.borrow_mut().remove(dir);
            return (first.unwrap_or_else(read).into(), false);
        };
        let changes = self.changes.get();
        let kept = self.dirs.borrow().get(dir).filter(|k| k.modified == modified && k.changes == changes).map(|k| k.entries.clone());
        if let Some(entries) = kept {
            return (entries, true);
        }
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
        dirs.insert(dir.to_path_buf(), HostDir { modified, changes, entries: entries.clone() });
        (entries, false)
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
