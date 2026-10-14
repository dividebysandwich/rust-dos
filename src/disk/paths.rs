//! Resolving DOS paths to host paths, and the current directories.

use super::*;

impl DiskController {
    // ========================================================================
    // PATH RESOLUTION
    // ========================================================================

    /// Split a DOS path into (drive, remainder), defaulting to the current
    /// drive. Returns None for a malformed drive specifier such as "@:".
    pub(super) fn split_drive<'a>(&self, path: &'a str) -> Option<(u8, &'a str)> {
        match parse_drive_prefix(path) {
            (Some(d), rest) => Some((d, rest)),
            (None, rest) => {
                if rest.as_bytes().get(1) == Some(&b':') {
                    None
                } else {
                    Some((self.current_drive, rest))
                }
            }
        }
    }

    /// Logical path components of `rest` on `drive`, applying the drive's
    /// current directory for relative paths and folding "." / "..".
    /// Trailing spaces are the padding of a name DOS packs into 8 and 3
    /// characters, and go: Comanche opens "LH66.RLE    ".
    pub(super) fn logical_components<'a>(drive: &'a Drive, rest: &'a str) -> Vec<&'a str> {
        let mut components: Vec<&str> = Vec::new();
        if !rest.starts_with('\\') {
            components.extend(drive.current_dir.split('\\').filter(|p| !p.is_empty()));
        }
        for part in rest.split('\\') {
            let part = match part.trim_end_matches(' ') {
                "" => part,
                trimmed => trimmed,
            };
            match part {
                "" | "." => {}
                ".." => {
                    components.pop();
                }
                _ => components.push(part),
            }
        }
        components
    }

    /// The path `rest` names on a drive held in memory, as `MemFs` keeps
    /// paths.
    pub(super) fn memory_path(drive: &Drive, rest: &str) -> String {
        Self::logical_components(drive, rest).join("\\").to_ascii_uppercase()
    }

    /// Resolves a DOS path (e.g., "GAMES\DOOM.EXE", "..\FILE.TXT" or
    /// "D:\DATA") to a Host Path, ensuring it stays within the drive's root.
    /// Handles case-insensitivity and short filenames (8.3).
    pub fn resolve_path(&self, dos_path: &str) -> Option<PathBuf> {
        self.locate(dos_path).map(|(_, path)| path)
    }

    /// Like `resolve_path`, but also returns the drive the path is on.
    pub(crate) fn locate(&self, dos_path: &str) -> Option<(u8, PathBuf)> {
        let path_str = dos_path.replace('/', "\\");
        let (drive, rest) = self.split_drive(&path_str)?;
        self.resolve_on(drive, rest).map(|p| (drive, p))
    }

    pub(super) fn resolve_on(&self, drive_num: u8, rest: &str) -> Option<PathBuf> {
        self.resolve_names_on(drive_num, rest).map(|(path, _)| path)
    }

    /// The host path of `rest` on a host drive, and its DOS path from the
    /// root in short names (e.g. "GAMES\DAYOFT~1").
    pub(super) fn resolve_names_on(&self, drive_num: u8, rest: &str) -> Option<(PathBuf, String)> {
        let drive = self.drive(drive_num)?;
        // Drives held in memory have no host paths; callers look there
        // first (`locate_in_memory`).
        let root = drive.host_root()?;

        // Traverse and Resolve to Host Paths. ".." handling in
        // logical_components can't climb above the root.
        let mut full_path = root.to_path_buf();
        let mut dos_path: Vec<String> = Vec::new();
        for part in Self::logical_components(drive, rest) {
            let (host_name, dos_name) = self.find_host_child(&full_path, part);
            full_path.push(host_name);
            dos_path.push(dos_name);
        }

        // Final Security Check
        if full_path.starts_with(root) {
            Some((full_path, dos_path.join("\\")))
        } else {
            None
        }
    }

    /// Fully qualified DOS directory for the directory part of a search spec,
    /// e.g. "*.EXE" on C: in GAMES -> "C:\GAMES", "D:SUB\*.*" -> "D:\SUB".
    /// Purely logical; the directory need not exist.
    pub fn qualify_directory(&self, spec: &str) -> Option<String> {
        let normalized = spec.replace('/', "\\");
        let (drive_num, rest) = self.split_drive(&normalized)?;
        let drive = self.drive(drive_num)?;
        let dir_part = match rest.rfind('\\') {
            Some(0) => "\\",
            Some(i) => &rest[..i],
            None => "",
        };
        let components = Self::logical_components(drive, dir_part);
        Some(format!(
            "{}:\\{}",
            drive_letter(drive_num),
            components.join("\\").to_ascii_uppercase()
        ))
    }

    /// Fully qualified DOS path of a file name, e.g. "DESCENTR.EXE" in
    /// C:\DESCENT -> "C:\DESCENT\DESCENTR.EXE" (INT 21h AH=60h, and the
    /// program path DOS puts after a program's environment). Purely
    /// logical; the file need not exist.
    pub fn qualify_path(&self, spec: &str) -> Option<String> {
        let normalized = spec.replace('/', "\\");
        let (drive_num, rest) = self.split_drive(&normalized)?;
        let drive = self.drive(drive_num)?;
        let components = Self::logical_components(drive, rest);
        Some(format!(
            "{}:\\{}",
            drive_letter(drive_num),
            components.join("\\").to_ascii_uppercase()
        ))
    }

    /// The drive a DOS path is on and the path there, if that drive is
    /// held in memory: "X:MIDI\ACPIANO.PAT" in X:\ULTRASND is
    /// "ULTRASND\MIDI\ACPIANO.PAT" on X:.
    pub(super) fn locate_in_memory(&self, dos_path: &str) -> Option<(u8, &Drive, String)> {
        let normalized = dos_path.replace('/', "\\");
        let (drive_num, rest) = self.split_drive(&normalized)?;
        let drive = self.drive(drive_num).filter(|d| d.tree().is_some())?;
        let path = Self::memory_path(drive, rest);
        Some((drive_num, drive, path))
    }

    /// The drive a DOS path is on and the path there, if that drive is
    /// mounted from a disk image.
    pub(super) fn locate_fat(&self, dos_path: &str) -> Option<(u8, Rc<FatVolume>, Vec<String>)> {
        let normalized = dos_path.replace('/', "\\");
        let (drive_num, rest) = self.split_drive(&normalized)?;
        let drive = self.drive(drive_num)?;
        let volume = drive.fat()?.clone();
        let parts = Self::logical_components(drive, rest).into_iter().map(str::to_string).collect();
        Some((drive_num, volume, parts))
    }

    /// The file or directory a DOS path names on a drive mounted from a
    /// disk image: None if the path is on another drive.
    pub(super) fn find_fat(&self, dos_path: &str) -> Option<Result<fat::Entry, u8>> {
        let (_, volume, parts) = self.locate_fat(dos_path)?;
        Some(volume.find(&refs(&parts)))
    }

    /// A file on a drive held in memory.
    fn virtual_file(&self, filename: &str) -> Option<&Node> {
        let (_, drive, path) = self.locate_in_memory(filename)?;
        drive.tree()?.file(&path)
    }

    /// Whether `filename` is a file on a drive held in memory.
    pub fn is_virtual_file(&self, filename: &str) -> bool {
        self.virtual_file(filename).is_some()
    }

    /// Whether a DOS path names an existing file, on any drive.
    pub fn is_file(&self, dos_path: &str) -> bool {
        if let Some(found) = self.find_fat(dos_path) {
            return found.is_ok_and(|e| !e.is_dir());
        }
        self.is_virtual_file(dos_path) || self.resolve_path(dos_path).is_some_and(|p| hostfs::is_file(&p))
    }

    /// Whether a DOS path names an existing directory, on any drive.
    pub fn is_directory(&self, dos_path: &str) -> bool {
        if let Some(found) = self.find_fat(dos_path) {
            return found.is_ok_and(|e| e.is_dir());
        }
        match self.locate_in_memory(dos_path) {
            Some((_, drive, path)) => drive.tree().is_some_and(|files| files.is_dir(&path)),
            None => self.resolve_path(dos_path).is_some_and(|p| hostfs::is_dir(&p)),
        }
    }

    /// Whether a DOS path names an existing file or directory, on any
    /// drive.
    pub fn exists(&self, dos_path: &str) -> bool {
        self.is_file(dos_path) || self.is_directory(dos_path)
    }

    /// Where the contents of the file a DOS path names are, on any drive.
    pub fn file_data(&self, dos_path: &str) -> Option<FileData> {
        if let Some((_, volume, parts)) = self.locate_fat(dos_path) {
            let entry = volume.find(&refs(&parts)).ok().filter(|e| !e.is_dir())?;
            return Some(FileData::Fat(volume, entry));
        }
        match self.locate_in_memory(dos_path) {
            Some((_, drive, path)) => FileData::of(drive.tree()?.file(&path)?, drive.image()),
            None => self.resolve_path(dos_path).filter(|p| hostfs::is_file(p)).map(FileData::Host),
        }
    }

    /// The entries of a host directory with their DOS names (see
    /// `short_names`), in sorted order. Hidden (dot) files are left out.
    /// Every component of a path is looked up here, so the entries are
    /// kept while the folder doesn't change (`HostDirCache`).
    /// The flag says whether they are the kept ones.
    pub(super) fn host_entries(&self, dir: &Path) -> (Rc<[(String, String)]>, bool) {
        self.host_dirs.entries(dir, || Self::read_host_entries(dir))
    }

    pub(super) fn read_host_entries(dir: &Path) -> Vec<(String, String)> {
        let mut names: Vec<String> = hostfs::read_dir(dir)
            .into_iter()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with('.'))
            .collect();
        names.sort_by_key(|name| name.to_ascii_uppercase());
        let short = short_names(&names);
        names.into_iter().zip(short).collect()
    }

    /// The host name and the DOS name of the entry of `dir` that a DOS path
    /// component names: its long name in any case, or its short name. A
    /// name that matches nothing comes back uppercased, for creating it.
    fn find_host_child(&self, dir: &Path, target: &str) -> (String, String) {
        let target_upper = target.to_ascii_uppercase();
        let find = |entries: &[(String, String)]| {
            entries
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(target))
                .or_else(|| entries.iter().find(|(_, short)| *short == target_upper))
                .cloned()
        };
        let (entries, kept) = self.host_entries(dir);
        find(&entries)
            .or_else(|| kept.then(|| find(&self.host_dirs.reread(dir, || Self::read_host_entries(dir)))).flatten())
            .unwrap_or((target_upper.clone(), target_upper))
    }

    // ========================================================================
    // DIR OPERATIONS
    // ========================================================================

    /// DOS CHDIR: changes the current directory of the drive named in `path`
    /// (or of the current drive) without switching drives.
    pub fn set_current_directory(&mut self, path: &str) -> bool {
        let normalized = path.replace('/', "\\");
        let Some((drive_num, rest)) = self.split_drive(&normalized) else {
            return false;
        };
        let Some(drive) = self.drive(drive_num) else {
            return false;
        };
        if let Some(volume) = drive.fat() {
            let parts = Self::logical_components(drive, rest);
            if !volume.find(&parts).is_ok_and(|e| e.is_dir()) {
                return false;
            }
            let dir = canonical_path(&parts);
            if let Some(d) = self.drives[drive_num as usize].as_mut() {
                d.current_dir = dir;
            }
            return true;
        }
        if let Some(files) = drive.tree() {
            let dir = Self::memory_path(drive, rest);
            if !files.is_dir(&dir) {
                return false;
            }
            if let Some(d) = self.drives[drive_num as usize].as_mut() {
                d.current_dir = dir;
            }
            return true;
        }

        // Resolve the new path to check existence, and keep it in short
        // names, as programs see it.
        match self.resolve_names_on(drive_num, rest) {
            Some((host_path, dos_dir)) if hostfs::is_dir(&host_path) => {
                if let Some(d) = self.drives[drive_num as usize].as_mut() {
                    d.current_dir = dos_dir;
                }
                true
            }
            _ => false,
        }
    }

    /// Current directory of the current drive, without drive or leading "\".
    pub fn get_current_directory(&self) -> String {
        self.get_current_directory_of(self.current_drive)
            .unwrap_or_default()
    }

    /// Current directory of `drive`, or None if it isn't mounted.
    pub fn get_current_directory_of(&self, drive: u8) -> Option<String> {
        self.drive(drive)
            .map(|d| d.current_dir.to_ascii_uppercase())
    }
}
