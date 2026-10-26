//! Files the machine shows in a directory of a drive that the drive
//! doesn't have: the built-in Windows's DLLs in C:\WINDOWS\SYSTEM
//! (win32/dll_files.rs). DOS and the programs above it see them as any
//! other file: they open for reading, and show in searches and to the
//! attribute calls as read-only files. Opening one for writing, making,
//! deleting or renaming one, or changing its attributes is access
//! denied, as for a read-only file.
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
    /// The directory, fully qualified in capitals ("C:\WINDOWS\SYSTEM").
    dir: String,
    /// The files by their names in capitals.
    files: BTreeMap<String, Bytes>,
}

/// What a DOS path names of the system files.
enum Shown<'a> {
    File(&'a Bytes),
    /// The directory, or one on its way to it.
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
    /// qualified, in place of those shown before; none takes them away.
    /// A file open already reads as its new contents: a save state opens
    /// its files before the built-in Windows says which version they are.
    pub fn set_system_files(&mut self, dir: &str, files: Vec<(String, Bytes)>) {
        self.system_files = SystemFiles {
            dir: dir.trim_end_matches('\\').to_ascii_uppercase(),
            files: files
                .into_iter()
                .map(|(name, data)| (name.to_ascii_uppercase(), data))
                .collect(),
        };
        let keys: Vec<(u64, Bytes)> = self
            .system_files
            .files
            .iter()
            .map(|(name, data)| {
                let path = format!("{}\\{}", self.system_files.dir, name);
                (self.file_key(&path), data.clone())
            })
            .collect();
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

    /// Whether the directory `dos_path` names holds the system files or a
    /// directory on the way to them, whether the drive has it or not: it
    /// isn't empty.
    pub(super) fn holds_system_files(&self, dos_path: &str) -> bool {
        let shown = &self.system_files;
        if shown.files.is_empty() {
            return false;
        }
        let Some(path) = self.qualify_path(dos_path) else {
            return false;
        };
        let path = path.trim_end_matches('\\');
        path.len() > 2 && (shown.dir == path || shown.dir.starts_with(&format!("{}\\", path)))
    }

    /// What the DOS path `dos_path` names of the system files, the drive
    /// not having it.
    fn shown(&self, dos_path: &str) -> Option<Shown<'_>> {
        let shown = &self.system_files;
        if shown.files.is_empty() {
            return None;
        }
        let path = self.qualify_path(dos_path)?;
        let path = path.trim_end_matches('\\');
        let found = if let Some((dir, name)) = split(path)
            && dir == shown.dir
            && let Some(data) = shown.files.get(name)
        {
            Shown::File(data)
        } else if path.len() > 2
            && (shown.dir == path || shown.dir.starts_with(&format!("{}\\", path)))
        {
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

    /// Whether `dos_path` names the system files' directory, or one on its
    /// way, which the drive hasn't.
    pub(super) fn is_system_dir(&self, dos_path: &str) -> bool {
        matches!(self.shown(dos_path), Some(Shown::Dir))
    }

    /// Whether the directory `dos_path` is in is the system files' or one
    /// on its way, which the drive hasn't.
    pub(super) fn in_system_dir(&self, dos_path: &str) -> bool {
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
        if self.system_files.files.is_empty() {
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
    /// drive's own entries. A system directory the drive hasn't lists as
    /// empty but for them.
    pub(super) fn with_system_entries(
        &self,
        spec: &str,
        search_attr: u16,
        listed: Result<Vec<DosDirEntry>, u8>,
    ) -> Result<Vec<DosDirEntry>, u8> {
        let shown = &self.system_files;
        if shown.files.is_empty() || search_attr & 0x08 != 0 {
            return listed;
        }
        let Some(dir) = self.qualify_directory(spec) else {
            return listed;
        };
        let dir = dir.trim_end_matches('\\');
        let pattern = spec.rsplit(['\\', '/', ':']).next().unwrap_or("");
        let is_shown = dir == shown.dir;
        let child = shown
            .dir
            .strip_prefix(dir)
            .and_then(|rest| rest.strip_prefix('\\').or((dir.len() == 2).then_some(rest)))
            .map(|rest| rest.split('\\').next().unwrap_or(rest));
        if !is_shown && child.is_none() {
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
        if is_shown {
            for (name, data) in &shown.files {
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
        } else if let Some(child) = child
            && search_attr & 0x10 != 0
            && Self::matches_pattern(child, pattern)
            && !has(&entries, child)
        {
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
        Ok(entries)
    }
}
