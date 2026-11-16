//! The cache of host folder entries and FindNext listings.

use super::*;
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::time::Duration;
use web_time::Instant;

/// The entries of host folders as last read, with their DOS names. Each
/// component of a DOS path is looked up among the entries of its folder,
/// so without this every open, attribute call and FindNext reads and sorts
/// every folder on its path.
///
/// A folder's entries are read again when the guest changed anything on a
/// host drive since (`changed`), and when the folder's modification time
/// moved (another program added, removed or renamed something in it). A
/// time under `RECENT` old is too close to tell a change made in the same
/// tick of a coarse clock (2 seconds on FAT) from the read, so the entries
/// of such a folder are kept for `TRUST` alone and then read again. An
/// installer copying into a folder keeps its time recent for minutes, and
/// polls for each file before it writes it: Fields of Fire's setup looked
/// up 2,000 missing names a second in a folder of 500 files, each of them
/// read and sorted three times. Folders that only a backend reaches (the
/// libretro frontend's, a write overlay's layer) aren't kept: their times
/// aren't known.
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
/// than the call should. A missing name isn't statted or opened on the
/// host either (`DiskController::locate_found`): Sub Culture's open asked
/// the host five times about the missing file, and now asks once (whether
/// its folder is a folder). A name the guest is about to create is looked
/// for again (`creating`), so that a file another program made in the
/// meantime is the one the guest gets.
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
    /// reads every file's size, so a folder's count is kept as its entries
    /// are: until its time moves, checked once in `TRUST`, and for `TRUST`
    /// alone while that time is `RECENT`. The files the guest changes in
    /// it (`touched`) are counted again one by one on the next look, so
    /// free space drops as a file is written without the whole folder
    /// being counted again: an installer asks for it before each block it
    /// writes. A file another program resizes shows when its folder is
    /// counted again, as a change by the guest in it does once the count
    /// is `TRUST` old.
    pub(super) usage: RefCell<HashMap<(PathBuf, u64, u64), DirUsage>>,
    /// The time `TRUST` is measured by in tests: it stands still from the
    /// first look and moves on only by `expire`, so how long a test takes
    /// doesn't change what it sees.
    #[cfg(test)]
    clock: Cell<Option<Instant>>,
    /// Folders read for their entries, and counted for their usage: what
    /// tests check a lookup's cost by.
    #[cfg(test)]
    pub(super) reads: Cell<u32>,
    #[cfg(test)]
    pub(super) counts: Cell<u32>,
}

/// The clusters a host folder's own files take, and its folders.
pub(super) type Usage = (u64, Rc<[PathBuf]>);

/// What the files of a host folder take, as counted.
pub(super) struct DirCount {
    /// The clusters its files take.
    pub(super) clusters: u64,
    /// Its folders.
    pub(super) dirs: Rc<[PathBuf]>,
    /// The clusters each file takes, by its name; None where counting
    /// stopped at the most it counts, before every file.
    pub(super) files: Option<HashMap<OsString, u64>>,
}

impl DirCount {
    /// Count the entry `name` of the folder `dir` again, in clusters of
    /// `cluster_bytes`: false where the folder has to be counted again
    /// instead, as one of its folders came or went, or not every file was
    /// counted.
    fn recount(&mut self, dir: &Path, name: &OsStr, cluster_bytes: u64) -> bool {
        if host_private(&name.to_string_lossy()) {
            return true;
        }
        let Some(files) = self.files.as_mut() else {
            return false;
        };
        let path = dir.join(name);
        let meta = hostfs::metadata(&path).ok();
        if self.dirs.contains(&path) || meta.is_some_and(|m| m.is_dir) {
            return false;
        }
        let was = match meta {
            Some(meta) => files.insert(name.to_owned(), meta.len.div_ceil(cluster_bytes)),
            None => files.remove(name),
        };
        let now = files.get(name).copied().unwrap_or(0);
        self.clusters = self.clusters - was.unwrap_or(0) + now;
        true
    }
}

/// A folder's count, its modification time then (None for a `RECENT`
/// one), and the entries the guest changed since.
pub(super) struct DirUsage {
    modified: Option<std::time::SystemTime>,
    /// When the folder was counted.
    counted: Instant,
    /// When its time was last found to be `modified`.
    checked: Instant,
    count: DirCount,
    /// The names of its entries the guest changed since (`touched`).
    pending: Vec<OsString>,
}

/// What a host folder's modification time says about keeping what was
/// read of it (`HostDirCache::settled`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Settled {
    /// `RECENT` or more in the past: kept until the time moves.
    At(std::time::SystemTime),
    /// Under `RECENT` old: kept for `TRUST`, then read again.
    Recent,
    /// A folder `std::fs` doesn't reach, or no folder: not kept.
    Unknown,
}

impl Settled {
    /// The time a kept read is checked against; None for one that is
    /// read again after `TRUST`.
    fn time(self) -> Option<std::time::SystemTime> {
        match self {
            Settled::At(at) => Some(at),
            _ => None,
        }
    }
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
    /// The folder's time when read; None for a `RECENT` one, whose
    /// entries are read again after `TRUST`.
    modified: Option<std::time::SystemTime>,
    changes: u64,
    /// When the folder's time was last found to be `modified`.
    checked: Instant,
    /// When `entries` were read.
    read: Instant,
    entries: Rc<[(String, String)]>,
    /// Names, uppercased, that were looked up and aren't in `entries`, the
    /// oldest first.
    missing: VecDeque<String>,
}

impl HostDirCache {
    /// How recent a folder's modification time can be for its entries to
    /// be kept.
    const RECENT: std::time::Duration = std::time::Duration::from_secs(2);
    /// How long a folder's entries are used without checking its time, and
    /// how long a name a read didn't have counts as missing without
    /// reading the folder again.
    const TRUST: Duration = Duration::from_secs(1);
    /// Folders kept at most; one is dropped to make room for another.
    const MAX_DIRS: usize = 64;
    /// Searches kept at most: a program walking a tree has one open per
    /// level.
    const MAX_SEARCHES: usize = 8;
    /// Missing names noted per folder at most; the oldest goes first.
    const MAX_MISSING: usize = 32;

    /// Now, as `TRUST` is measured.
    fn now(&self) -> Instant {
        #[cfg(test)]
        {
            let now = self.clock.get().unwrap_or_else(Instant::now);
            self.clock.set(Some(now));
            now
        }
        #[cfg(not(test))]
        Instant::now()
    }

    /// How long ago `then` was, as `TRUST` is measured.
    fn since(&self, then: Instant) -> Duration {
        self.now().saturating_duration_since(then)
    }

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
    /// it finds into another folder doesn't list its source each time.
    /// `path` is counted again in its folder's count on the next look, and
    /// its own count, as a folder, is dropped.
    pub(super) fn touched(&self, path: &Path) {
        let parent = path.parent();
        let name = path.file_name();
        self.usage.borrow_mut().retain(|(dir, ..), kept| {
            if dir == path {
                return false;
            }
            if Some(dir.as_path()) != parent {
                return true;
            }
            match name {
                Some(name) if kept.count.files.is_some() => {
                    if !kept.pending.iter().any(|p| p == name) {
                        kept.pending.push(name.to_owned());
                    }
                    true
                }
                _ => false,
            }
        });
        self.searches.borrow_mut().retain(|s| Some(s.key.dir.as_path()) != parent && !s.key.dir.starts_with(path));
    }

    /// What the files in the host folder `dir` take with clusters of
    /// `cluster_bytes`, counted up to `cap`, kept or from `count`.
    pub(super) fn usage(
        &self,
        dir: &Path,
        cluster_bytes: u64,
        cap: u64,
        count: impl FnOnce() -> DirCount,
    ) -> Usage {
        let key = (dir.to_path_buf(), cluster_bytes, cap);
        let mut kept = self.usage.borrow_mut();
        if let Some(found) = kept.get_mut(&key)
            && self.usage_holds(dir, found, cluster_bytes)
        {
            return (found.count.clusters, found.count.dirs.clone());
        }
        // The time is taken before the count, so that a change made in
        // between shows as a newer time on a later look.
        let settled = Self::settled(dir);
        #[cfg(test)]
        self.counts.set(self.counts.get() + 1);
        let count = count();
        let usage = (count.clusters, count.dirs.clone());
        if settled == Settled::Unknown {
            kept.remove(&key);
        } else {
            let now = self.now();
            let modified = settled.time();
            let pending = Vec::new();
            kept.insert(
                key,
                DirUsage {
                    modified,
                    counted: now,
                    checked: now,
                    count,
                    pending,
                },
            );
        }
        usage
    }

    /// Whether the kept count `found` of the host folder `dir`, in
    /// clusters of `cluster_bytes`, still holds, with the entries the
    /// guest changed since counted again. Its time is checked once in
    /// `TRUST`, and a `RECENT` folder is counted again after `TRUST`.
    /// Changed entries are counted one by one within `TRUST` of the count,
    /// and the whole folder again after that, so that a file another
    /// program resized shows too.
    fn usage_holds(&self, dir: &Path, found: &mut DirUsage, cluster_bytes: u64) -> bool {
        if self.since(found.checked) >= Self::TRUST {
            match (Self::settled(dir), found.modified) {
                (Settled::At(at), Some(kept)) if at == kept => found.checked = self.now(),
                _ => return false,
            }
        }
        if found.pending.is_empty() {
            return true;
        }
        if self.since(found.counted) >= Self::TRUST {
            return false;
        }
        let pending = std::mem::take(&mut found.pending);
        pending
            .iter()
            .all(|name| found.count.recount(dir, name, cluster_bytes))
    }

    /// What the modification time of the host folder `dir` says about
    /// keeping what was read of it: whether it is a folder `std::fs`
    /// reaches, and its time if that is `RECENT` or more in the past.
    fn settled(dir: &Path) -> Settled {
        if hostfs::has_scheme(dir) {
            return Settled::Unknown;
        }
        let meta = hostfs::metadata(dir).ok().filter(|m| m.is_dir);
        let Some(at) = meta.and_then(|m| m.modified) else {
            return Settled::Unknown;
        };
        if at.elapsed().is_ok_and(|age| age >= Self::RECENT) {
            Settled::At(at)
        } else {
            Settled::Recent
        }
    }

    /// The entries of `dir` from `read`, kept in place of the ones kept:
    /// what was kept lacked a name the guest looked for. The folder's time
    /// is taken again first: a folder changed within `RECENT` is kept for
    /// `TRUST` from now. The missing names noted stay noted if the entries
    /// came out the same.
    pub(super) fn reread(&self, dir: &Path, read: impl Fn() -> Vec<(String, String)>) -> Rc<[(String, String)]> {
        let settled = Self::settled(dir);
        #[cfg(test)]
        self.reads.set(self.reads.get() + 1);
        let entries: Rc<[(String, String)]> = read().into();
        let mut dirs = self.dirs.borrow_mut();
        if settled == Settled::Unknown {
            dirs.remove(dir);
        } else if let Some(kept) = dirs.get_mut(dir) {
            let now = self.now();
            if kept.entries != entries {
                kept.missing.clear();
            }
            kept.modified = settled.time();
            kept.checked = now;
            kept.read = now;
            kept.entries = entries.clone();
        }
        entries
    }

    /// Whether the name `upper` (uppercased) was looked up in the kept
    /// folder `dir` and not found in a read less than `TRUST` ago, with no
    /// change by the guest since.
    pub(super) fn known_missing(&self, dir: &Path, upper: &str) -> bool {
        self.dirs.borrow().get(dir).is_some_and(|kept| {
            kept.changes == self.changes.get()
                && self.since(kept.read) < Self::TRUST
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
                kept.missing.pop_front();
            }
            kept.missing.push_back(upper.to_string());
        }
    }

    /// The guest is about to create a file: the names noted missing are
    /// looked for again, so that a file another program made since isn't
    /// made a second time beside it (in another case, on a host that
    /// tells cases apart).
    pub(super) fn creating(&self) {
        for kept in self.dirs.borrow_mut().values_mut() {
            kept.missing.clear();
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
            .filter(|k| k.changes == changes && self.since(k.checked) < Self::TRUST)
            .map(|k| k.entries.clone());
        if let Some(entries) = trusted {
            return (entries, true);
        }
        let settled = Self::settled(dir);
        if settled == Settled::Unknown {
            self.dirs.borrow_mut().remove(dir);
            #[cfg(test)]
            self.reads.set(self.reads.get() + 1);
            return (read().into(), false);
        }
        // A recent folder's time doesn't tell whether it changed since.
        let modified = settled.time();
        let mut dirs = self.dirs.borrow_mut();
        if let Some(kept) = dirs
            .get_mut(dir)
            .filter(|k| modified.is_some() && k.modified == modified && k.changes == changes)
        {
            kept.checked = self.now();
            return (kept.entries.clone(), true);
        }
        drop(dirs);
        // Read after the time was taken, so that a change made in between
        // shows as a newer time on the next call.
        #[cfg(test)]
        self.reads.set(self.reads.get() + 1);
        let entries: Rc<[(String, String)]> = read().into();
        let mut dirs = self.dirs.borrow_mut();
        if dirs.len() >= Self::MAX_DIRS
            && !dirs.contains_key(dir)
            && let Some(dropped) = dirs.keys().next().cloned()
        {
            dirs.remove(&dropped);
        }
        let now = self.now();
        let kept = HostDir {
            modified,
            changes,
            checked: now,
            read: now,
            entries: entries.clone(),
            missing: VecDeque::new(),
        };
        dirs.insert(dir.to_path_buf(), kept);
        (entries, false)
    }

    /// Moves the test clock on by `TRUST`: the next look at each folder
    /// checks its time again, and a name noted missing is looked for again.
    #[cfg(test)]
    pub(super) fn expire(&self) {
        self.clock.set(Some(self.now() + Self::TRUST));
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
        let Settled::At(modified) = Self::settled(&key.dir) else {
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
