//! Which file a path names, whatever name it is given by: what tells open
//! files apart for sharing modes and for the emptying a create call leaves
//! for a file's first use.

use super::*;

/// The file a DOS path names (`DiskController::file_id`). A file may have
/// more than one path: its long name and its 8.3 alias, a name in another
/// case. Windows 9x's VFAT finds the same directory entry through each,
/// and its sharing checks, and the file a create call empties, go by that
/// entry.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum FileId {
    /// A file on a drive mounted from a disk image: the drive, the image
    /// and the file's directory entry. A file open on one disk of a list
    /// stays open when another goes in (`swap_image`), and the image tells
    /// it from the file in the same slot of the other disk. A rename that
    /// moves the entry moves the open files' ids with it
    /// (`DiskController::moved_files`).
    Fat(u8, PathBuf, EntryRef),
    /// A file in a host folder: its host path, in the names the host's
    /// entries have. The drive's root is canonical, so two drives over one
    /// host folder see the same files.
    Host(PathBuf),
    /// A file on a drive held in memory, by its full path in DOS names,
    /// and a name that names no file, by its full DOS path.
    Named(String),
    /// A character device.
    Device,
}

impl FileId {
    /// The file whose directory entry is at `at` on `volume`, in `drive`.
    pub(super) fn fat(drive: u8, volume: &FatVolume, at: EntryRef) -> Self {
        FileId::Fat(drive, volume.disk().path().to_path_buf(), at)
    }

    /// A number for the id, for the disk noises and for record locks,
    /// which save states keep.
    pub(super) fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }
}

impl DiskController {
    /// The file `dos_path` names, found as the call running now finds it
    /// (`fat_find`): the same id for the file's long name, its alias and
    /// its name in any case. A name that names no file has its full DOS
    /// path for its id.
    pub(super) fn file_id(&self, dos_path: &str) -> FileId {
        if self.device(dos_path).is_some() {
            return FileId::Device;
        }
        let named = || {
            FileId::Named(
                self.qualify_path(dos_path)
                    .unwrap_or_else(|| dos_path.to_ascii_uppercase()),
            )
        };
        if let Some((drive, volume, parts)) = self.locate_fat(dos_path) {
            return match self.fat_find(&volume, &parts) {
                Ok(fat::Entry { at: Some(at), .. }) => FileId::fat(drive, &volume, at),
                _ => named(),
            };
        }
        if let Some((drive, _, path)) = self.locate_in_memory(dos_path) {
            return FileId::Named(format!("{}:\\{}", drive_letter(drive), path));
        }
        self.host_file_id(dos_path).map_or_else(named, FileId::Host)
    }

    /// The host path of the file `dos_path` names on a host drive, in the
    /// names the host has: None if it names none there. A path each of
    /// whose components matched an entry of its folder has them already
    /// (`walk_host`). A component the entries can't rule out (a short name
    /// of the Windows host's own, or letters beyond ASCII in another case)
    /// is asked of the host, which gives the file's own path.
    fn host_file_id(&self, dos_path: &str) -> Option<PathBuf> {
        let (drive, walk) = self.locate_walk(dos_path)?;
        if walk.matched {
            return Some(walk.path);
        }
        if walk.missing {
            return None;
        }
        let root = self.drive(drive)?.host_root()?;
        hostfs::canonicalize(&walk.path)
            .ok()
            .filter(|path| path.starts_with(root))
    }

    /// Give each open file the id `moved` gives for its id, if it gives
    /// one, after a rename moved the file or a directory above it. Their
    /// record locks go with them.
    pub(super) fn moved_files(&mut self, moved: impl Fn(&FileId) -> Option<FileId>) {
        let mut rekeyed = Vec::new();
        for (&sft, open) in self.open_files.iter_mut() {
            if let Some(id) = moved(&open.id) {
                rekeyed.push((sft, id.key()));
                open.id = id;
            }
        }
        for (sft, key) in rekeyed {
            self.rekey_locks(sft, key);
        }
    }

    /// The host paths of the files under the host path `from` that a
    /// rename of `from` to `to` moves: `from` itself and what a directory
    /// there holds.
    pub(super) fn host_files_moved(from: &Path, to: &Path) -> impl Fn(&FileId) -> Option<FileId> {
        let (from, to) = (from.to_path_buf(), to.to_path_buf());
        move |id| match id {
            FileId::Host(path) => {
                let below = path.strip_prefix(&from).ok()?;
                Some(FileId::Host(if below.as_os_str().is_empty() {
                    to.clone()
                } else {
                    to.join(below)
                }))
            }
            _ => None,
        }
    }
}
