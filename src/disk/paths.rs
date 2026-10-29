//! Resolving DOS paths to host paths, and the current directories.

use super::*;

/// The entries of a host folder, as (host name, DOS name).
type HostEntries = Rc<[(String, String)]>;

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
    /// paths: in DOS names, where `rest` may have long ones.
    pub(super) fn memory_path(drive: &Drive, rest: &str) -> String {
        let components = Self::logical_components(drive, rest);
        match drive.tree() {
            Some(files) => files.resolve(&components),
            None => components.join("\\").to_ascii_uppercase(),
        }
    }

    /// Resolves a DOS path (e.g., "GAMES\DOOM.EXE", "..\FILE.TXT" or
    /// "D:\DATA") to a Host Path, ensuring it stays within the drive's root.
    /// Handles case-insensitivity and short filenames (8.3).
    pub fn resolve_path(&self, dos_path: &str) -> Option<PathBuf> {
        self.locate(dos_path).map(|(_, path)| path)
    }

    /// Like `resolve_path`, but also returns the drive the path is on.
    pub(crate) fn locate(&self, dos_path: &str) -> Option<(u8, PathBuf)> {
        self.locate_found(dos_path).map(|(drive, path, _)| (drive, path))
    }

    pub(super) fn resolve_on(&self, drive_num: u8, rest: &str) -> Option<PathBuf> {
        self.resolve_names_on(drive_num, rest).map(|(path, _)| path)
    }

    /// The host path of `rest` on a host drive, and its DOS path from the
    /// root in short names (e.g. "GAMES\DAYOFT~1").
    pub(super) fn resolve_names_on(&self, drive_num: u8, rest: &str) -> Option<(PathBuf, String)> {
        self.walk_host(drive_num, rest).map(|(path, dos, _)| (path, dos))
    }

    /// `resolve_names_on`, and whether a component of the path isn't
    /// among its folder's entries (`find_host_entry`): the path names
    /// nothing on the host then, which needs no stat to tell. Each
    /// component is the entry's long name, or, where it matches nothing,
    /// the name uppercased, for creating it. A name the entries can't rule
    /// out (`entries_settle`) is never missing.
    fn walk_host(&self, drive_num: u8, rest: &str) -> Option<(PathBuf, String, bool)> {
        let drive = self.drive(drive_num)?;
        // Drives held in memory have no host paths; callers look there
        // first (`locate_in_memory`).
        let root = drive.host_root()?;

        // Traverse and Resolve to Host Paths. ".." handling in
        // logical_components can't climb above the root.
        let mut full_path = root.to_path_buf();
        let mut dos_path: Vec<String> = Vec::new();
        let mut missing = false;
        // Below a name the entries can't rule out, the folders read may
        // not be the ones the host has there, so they rule out nothing.
        let mut sure = true;
        for part in Self::logical_components(drive, rest) {
            let found = self.find_host_entry(&full_path, part).1;
            if found.is_none() && sure {
                if Self::entries_settle(part) {
                    missing = true;
                } else {
                    sure = false;
                }
            }
            let (host_name, dos_name) = found.unwrap_or_else(|| {
                let upper = part.to_ascii_uppercase();
                (upper.clone(), upper)
            });
            full_path.push(host_name);
            dos_path.push(dos_name);
        }

        // Final Security Check
        if full_path.starts_with(root) {
            Some((full_path, dos_path.join("\\"), missing))
        } else {
            None
        }
    }

    /// Whether a folder's entries settle that the path component `part`
    /// isn't in it when none matches it. They leave out names starting
    /// with a dot, and the host may know a name by a spelling they don't
    /// match: Windows drops a dot that ends it ("README." is "README"),
    /// has short names of its own (with a "~"), and Windows and macOS fold
    /// the case of letters beyond ASCII. Such names go to the host.
    fn entries_settle(part: &str) -> bool {
        !part.starts_with('.') && !part.ends_with('.') && !part.contains('~') && part.is_ascii()
    }

    /// `locate`, and whether a component of the path isn't among its
    /// folder's entries (`walk_host`), so that the path names nothing on
    /// the host. Where each is, a stat tells whether the path names a file
    /// or a folder.
    pub(super) fn locate_found(&self, dos_path: &str) -> Option<(u8, PathBuf, bool)> {
        let path_str = dos_path.replace('/', "\\");
        let (drive, rest) = self.split_drive(&path_str)?;
        let (path, _, missing) = self.walk_host(drive, rest)?;
        Some((drive, path, missing))
    }

    /// The host path of a DOS path, None where it names nothing on the
    /// host (`locate_found`).
    fn resolve_found(&self, dos_path: &str) -> Option<PathBuf> {
        self.locate_found(dos_path).filter(|(_, _, missing)| !missing).map(|(_, path, _)| path)
    }

    /// The names of the entries of the directory `dos_dir`, as (long
    /// name, DOS name), on a host drive and on a drive held in memory: the
    /// host's names are the long names Windows has, and so are a CD's
    /// Joliet names. None on disk images, where names are 8.3 alone, and
    /// for a directory that isn't there.
    pub fn long_names(&self, dos_dir: &str) -> Option<Vec<(String, String)>> {
        let normalized = dos_dir.replace('/', "\\");
        let (drive_num, rest) = self.split_drive(&normalized)?;
        let drive = self.drive(drive_num)?;
        if let Some(files) = drive.tree() {
            let dir = Self::memory_path(drive, rest);
            return files.is_dir(&dir).then(|| files.long_names(&dir));
        }
        drive.host_root()?;
        let rest = if rest.is_empty() { "." } else { rest };
        let dir = self.resolve_on(drive_num, rest)?;
        if !hostfs::is_dir(&dir) {
            return None;
        }
        let (entries, _) = self.host_entries(&dir);
        Some(entries.to_vec())
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

    /// Whether a DOS path names an existing file, on any drive, or a
    /// system file (system_files.rs).
    pub fn is_file(&self, dos_path: &str) -> bool {
        self.real_is_file(dos_path) || self.system_file(dos_path).is_some()
    }

    /// Whether a DOS path names an existing directory, on any drive, or a
    /// system directory.
    pub fn is_directory(&self, dos_path: &str) -> bool {
        self.real_is_directory(dos_path) || self.is_system_dir(dos_path)
    }

    /// Whether a DOS path names a file the drive has.
    pub(super) fn real_is_file(&self, dos_path: &str) -> bool {
        if let Some(found) = self.find_fat(dos_path) {
            return found.is_ok_and(|e| !e.is_dir());
        }
        self.is_virtual_file(dos_path)
            || self.resolve_found(dos_path).is_some_and(|p| hostfs::is_file(&p))
    }

    /// Whether a DOS path names a directory the drive has.
    pub(super) fn real_is_directory(&self, dos_path: &str) -> bool {
        if let Some(found) = self.find_fat(dos_path) {
            return found.is_ok_and(|e| e.is_dir());
        }
        match self.locate_in_memory(dos_path) {
            Some((_, drive, path)) => drive.tree().is_some_and(|files| files.is_dir(&path)),
            None => self.resolve_found(dos_path).is_some_and(|p| hostfs::is_dir(&p)),
        }
    }

    /// Whether a DOS path names an existing file or directory, on any
    /// drive.
    pub fn exists(&self, dos_path: &str) -> bool {
        self.is_file(dos_path) || self.is_directory(dos_path)
    }

    /// Where the contents of the file a DOS path names are, on any drive.
    pub fn file_data(&self, dos_path: &str) -> Option<FileData> {
        if let Some(data) = self.system_file(dos_path) {
            return Some(FileData::Memory(data));
        }
        if let Some((_, volume, parts)) = self.locate_fat(dos_path) {
            let entry = volume.find(&refs(&parts)).ok().filter(|e| !e.is_dir())?;
            return Some(FileData::Fat(volume, entry));
        }
        match self.locate_in_memory(dos_path) {
            Some((_, drive, path)) => FileData::of(drive.tree()?.file(&path)?, drive.image()),
            None => self.resolve_found(dos_path).filter(|p| hostfs::is_file(p)).map(FileData::Host),
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

    /// The entries of the host folder `dir`, and the one named `name`: by
    /// its long name in any case, or by its short name. A name the kept
    /// entries lack is looked for in a fresh read, unless a recent read
    /// lacked it too (`HostDirCache`).
    pub(super) fn find_host_entry(
        &self,
        dir: &Path,
        name: &str,
    ) -> (HostEntries, Option<(String, String)>) {
        let upper = name.to_ascii_uppercase();
        let find = |entries: &[(String, String)]| {
            entries
                .iter()
                .find(|(long, _)| long.eq_ignore_ascii_case(name))
                .or_else(|| entries.iter().find(|(_, short)| *short == upper))
                .cloned()
        };
        let (mut entries, kept) = self.host_entries(dir);
        if let Some(found) = find(&entries) {
            return (entries, Some(found));
        }
        if kept {
            if self.host_dirs.known_missing(dir, &upper) {
                return (entries, None);
            }
            entries = self.host_dirs.reread(dir, || Self::read_host_entries(dir));
            if let Some(found) = find(&entries) {
                return (entries, Some(found));
            }
        }
        // The entries were just read.
        self.host_dirs.note_missing(dir, &upper);
        (entries, None)
    }

    // ========================================================================
    // DIR OPERATIONS
    // ========================================================================

    /// DOS CHDIR: changes the current directory of the drive named in `path`
    /// (or of the current drive) without switching drives.
    pub fn set_current_directory(&mut self, path: &str) -> bool {
        if self.is_system_dir(path)
            && let Some(dir) = self.qualify_path(path)
            && let Some((drive, rest)) = self.split_drive(&dir).map(|(d, r)| (d, r.to_string()))
            && let Some(d) = self.drives[drive as usize].as_mut()
        {
            d.current_dir = rest.trim_matches('\\').to_string();
            return true;
        }
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
