//! Files the machine shows in directories of a drive that the drive
//! doesn't have: the built-in Windows's DLLs in C:\WINDOWS\SYSTEM and its
//! programs in C:\WINDOWS (win32/dll_files.rs). DOS and the programs
//! above it see them as any other file: they open for reading, and show
//! in searches and to the attribute calls as read-only files. Opening one
//! for writing, making, deleting or renaming one, or changing its
//! attributes is access denied, as for a read-only file.
//!
//! A file or directory the drive has of the same name wins: a game that
//! copies its own GLIDE2X.DLL there has that one. The directories on the
//! way to the files show while the drive hasn't them, and are made on the
//! drive when a file or directory is made in them.

use super::*;
use std::collections::BTreeMap;

/// The files and where they are.
#[derive(Default)]
pub(super) struct SystemFiles {
    /// The files of each directory by their names in capitals, the
    /// directories fully qualified in capitals ("C:\WINDOWS\SYSTEM"). No
    /// directory is here with no files.
    dirs: BTreeMap<String, BTreeMap<String, Bytes>>,
}

impl SystemFiles {
    /// Whether `path`, fully qualified in capitals with no backslash at
    /// its end, is a directory with files or one on the way to one.
    fn leads_to_files(&self, path: &str) -> bool {
        path.len() > 2
            && self
                .dirs
                .keys()
                .any(|dir| dir == path || dir.starts_with(&format!("{}\\", path)))
    }
}

/// What a DOS path names of the system files.
enum Shown<'a> {
    File(&'a Bytes),
    /// A directory with files, or one on its way to one.
    Dir,
}

/// The directory `path` (fully qualified) is in, and its name: "C:\" and
/// "WINDOWS" for "C:\WINDOWS".
fn split(path: &str) -> Option<(&str, &str)> {
    let i = path.rfind('\\')?;
    let dir = if i == 2 { &path[..3] } else { &path[..i] };
    Some((dir, &path[i + 1..]))
}

impl DiskController {
    /// Show `files` (name, contents) in the directory `dir`, fully
    /// qualified, in place of those shown there before; none takes them
    /// away. Other directories keep theirs. A file open already reads as
    /// its new contents: a save state opens its files before the built-in
    /// Windows says which version they are.
    pub fn set_system_files(&mut self, dir: &str, files: Vec<(String, Bytes)>) {
        let dir = dir.trim_end_matches('\\').to_ascii_uppercase();
        let files: BTreeMap<String, Bytes> = files
            .into_iter()
            .map(|(name, data)| (name.to_ascii_uppercase(), data))
            .collect();
        let keys: Vec<(u64, Bytes)> = files
            .iter()
            .map(|(name, data)| {
                let path = format!("{}\\{}", dir, name);
                (self.file_key(&path), data.clone())
            })
            .collect();
        if files.is_empty() {
            self.system_files.dirs.remove(&dir);
        } else {
            self.system_files.dirs.insert(dir, files);
        }
        let mut changed = Vec::new();
        for (&sft, open) in self.open_files.iter_mut() {
            if let OpenData::Memory(data, _) = &mut open.data
                && let Some((_, new)) = keys.iter().find(|(key, _)| *key == open.key)
                && data[..] != new[..]
            {
                *data = new.clone();
                changed.push(sft);
            }
        }
        for sft in changed {
            self.mark_dirty(sft);
        }
        // Searches kept from before listed the files as they were.
        self.host_dirs.forget();
    }

    /// Whether the directory `dos_path` names holds system files or a
    /// directory on the way to them, whether the drive has it or not: it
    /// isn't empty.
    pub(super) fn holds_system_files(&self, dos_path: &str) -> bool {
        if self.system_files.dirs.is_empty() {
            return false;
        }
        let Some(path) = self.qualify_path(dos_path) else {
            return false;
        };
        self.system_files
            .leads_to_files(path.trim_end_matches('\\'))
    }

    /// What the DOS path `dos_path` names of the system files, the drive
    /// not having it.
    fn shown(&self, dos_path: &str) -> Option<Shown<'_>> {
        let shown = &self.system_files;
        if shown.dirs.is_empty() {
            return None;
        }
        let path = self.qualify_path(dos_path)?;
        let path = path.trim_end_matches('\\');
        let found = if let Some((dir, name)) = split(path)
            && let Some(data) = shown.dirs.get(dir).and_then(|files| files.get(name))
        {
            Shown::File(data)
        } else if shown.leads_to_files(path) {
            Shown::Dir
        } else {
            return None;
        };
        (!self.real_is_file(path) && !self.real_is_directory(path)).then_some(found)
    }

    /// The contents of the system file `dos_path` names, if it names one
    /// the drive hasn't.
    pub(super) fn system_file(&self, dos_path: &str) -> Option<Bytes> {
        match self.shown(dos_path)? {
            Shown::File(data) => Some(data.clone()),
            Shown::Dir => None,
        }
    }

    /// Whether `dos_path` names a directory of system files, or one on
    /// the way to one, which the drive hasn't.
    pub(super) fn is_system_dir(&self, dos_path: &str) -> bool {
        matches!(self.shown(dos_path), Some(Shown::Dir))
    }

    /// Whether the directory `dos_path` is in is one of system files or
    /// one on the way to one, which the drive hasn't.
    pub(super) fn in_system_dir(&self, dos_path: &str) -> bool {
        if self.system_files.dirs.is_empty() {
            return false;
        }
        let Some(path) = self.qualify_path(dos_path) else {
            return false;
        };
        split(path.trim_end_matches('\\')).is_some_and(|(dir, _)| self.is_system_dir(dir))
    }

    /// Whether `dos_path` names a system file or directory the drive
    /// hasn't.
    pub(super) fn is_system_entry(&self, dos_path: &str) -> bool {
        self.shown(dos_path).is_some()
    }

    /// The attributes of the system file or directory `dos_path` names:
    /// read-only and archive, or directory.
    pub(super) fn system_attribute(&self, dos_path: &str) -> Option<u16> {
        match self.shown(dos_path)? {
            Shown::File(_) => Some(0x21),
            Shown::Dir => Some(0x10),
        }
    }

    /// Make on the drive the system directories on the way to `dos_path`
    /// that it hasn't, before a file or directory is made there.
    pub(super) fn make_system_dirs(&self, dos_path: &str) {
        if self.system_files.dirs.is_empty() {
            return;
        }
        let Some(path) = self.qualify_path(dos_path) else {
            return;
        };
        let mut at = 3;
        while let Some(i) = path[at..].find('\\') {
            let dir = &path[..at + i];
            if self.is_system_dir(dir) && self.create_drive_directory(dir).is_err() {
                return;
            }
            at += i + 1;
        }
    }

    /// Open the system file `filename` names, which has `data`, with
    /// access `mode`: for reading alone.
    pub(super) fn open_system_file(
        &mut self,
        filename: &str,
        data: Bytes,
        mode: u8,
        owner: u16,
    ) -> Result<u16, u8> {
        match mode & 0x03 {
            0 => {}
            1 | 2 => return Err(0x05),
            _ => return Err(0x0C),
        }
        let drive = self.drive_of(filename).ok_or(0x03u8)?;
        let handle = self.free_handle()?;
        let key = self.file_key(filename);
        let file = self.opened(
            OpenData::Memory(data, Rc::new(Cell::new(0))),
            drive,
            owner,
            key,
            filename,
            mode,
        );
        self.insert(handle, file);
        Ok(handle)
    }

    /// The listing `listed` of the directory of the search `spec` with the
    /// system files and directories that match it added, after the
    /// drive's own entries: the directory's own files, then the
    /// directories in it on the way to others. A system directory the
    /// drive hasn't lists as empty but for them.
    pub(super) fn with_system_entries(
        &self,
        spec: &str,
        search_attr: u16,
        listed: Result<Vec<DosDirEntry>, u8>,
    ) -> Result<Vec<DosDirEntry>, u8> {
        let shown = &self.system_files;
        if shown.dirs.is_empty() || Self::label_only(search_attr) {
            return listed;
        }
        let Some(dir) = self.qualify_directory(spec) else {
            return listed;
        };
        let dir = dir.trim_end_matches('\\');
        let pattern = spec.rsplit(['\\', '/', ':']).next().unwrap_or("");
        let files = shown.dirs.get(dir);
        let mut children: Vec<&str> = Vec::new();
        for shown_dir in shown.dirs.keys() {
            let child = shown_dir
                .strip_prefix(dir)
                .and_then(|rest| rest.strip_prefix('\\').or((dir.len() == 2).then_some(rest)))
                .filter(|rest| !rest.is_empty())
                .map(|rest| rest.split('\\').next().unwrap_or(rest));
            if let Some(child) = child
                && !children.contains(&child)
            {
                children.push(child);
            }
        }
        if files.is_none() && children.is_empty() {
            return listed;
        }
        let mut entries = match listed {
            Ok(entries) => entries,
            Err(0x03) if self.is_system_dir(dir) => {
                Self::dot_entries(pattern, search_attr, [(MEMORY_TIME, MEMORY_DATE); 2]).collect()
            }
            Err(e) => return Err(e),
        };
        let has = |entries: &[DosDirEntry], name: &str| {
            entries
                .iter()
                .any(|e| e.filename.eq_ignore_ascii_case(name))
        };
        for (name, data) in files.into_iter().flatten() {
            if Self::matches_pattern(name, pattern) && !has(&entries, name) {
                entries.push(DosDirEntry {
                    filename: name.clone(),
                    size: data.len() as u32,
                    is_dir: false,
                    is_readonly: true,
                    dos_time: MEMORY_TIME,
                    dos_date: MEMORY_DATE,
                    attr: 0x21,
                });
            }
        }
        if search_attr & 0x10 != 0 {
            for child in children {
                if Self::matches_pattern(child, pattern) && !has(&entries, child) {
                    entries.push(DosDirEntry {
                        filename: child.to_string(),
                        size: 0,
                        is_dir: true,
                        is_readonly: false,
                        dos_time: MEMORY_TIME,
                        dos_date: MEMORY_DATE,
                        attr: 0x10,
                    });
                }
            }
        }
        Ok(entries)
    }
}
