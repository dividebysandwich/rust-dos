use chrono::{DateTime, Datelike, Local, Timelike};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

// DOS defines standard handles: 0=Stdin, 1=Stdout, 2=Stderr, 3=Aux, 4=Printer
pub const FIRST_USER_HANDLE: u16 = 5;
/// A job file table has at most 255 slots (0xFF marks an unused one).
const HANDLE_LIMIT: u16 = 0xFF;

// Drive numbers are 0-based (0=A:, 2=C:, 25=Z:).
pub const DRIVE_C: u8 = 2;
pub const DRIVE_Z: u8 = 25;
/// Number of drive letters reported to programs (LASTDRIVE=Z).
pub const LASTDRIVE: u8 = 26;

/// Volume label used when a mount doesn't specify one.
pub const DEFAULT_LABEL: &str = "RUSTDOS";

/// Usable data clusters on a 1.44 MB floppy with 512-byte clusters.
const FLOPPY_CLUSTERS: u16 = 2847;

pub fn drive_letter(drive: u8) -> char {
    (b'A' + drive) as char
}

/// Split an optional leading "X:" off a DOS path. Works on bytes so that a
/// lossily-decoded non-ASCII first character can never cause a panic.
pub fn parse_drive_prefix(path: &str) -> (Option<u8>, &str) {
    let b = path.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        (Some(b[0].to_ascii_uppercase() - b'A'), &path[2..])
    } else {
        (None, path)
    }
}

/// DOS volume labels are at most 11 uppercase characters.
pub fn normalize_label(label: &str) -> String {
    label.trim().to_ascii_uppercase().chars().take(11).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Floppy,
    HardDisk,
    CdRom,
    /// The built-in Z: drive holding in-memory files.
    Virtual,
}

impl DriveKind {
    pub fn is_removable(self) -> bool {
        matches!(self, DriveKind::Floppy | DriveKind::CdRom)
    }

    /// Media descriptor byte as found in the FAT / DPB.
    pub fn media_descriptor(self) -> u8 {
        match self {
            DriveKind::Floppy => 0xF0, // 3.5" 1.44 MB
            _ => 0xF8,                 // fixed disk
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DriveKind::Floppy => "floppy",
            DriveKind::HardDisk => "hdd",
            DriveKind::CdRom => "cdrom",
            DriveKind::Virtual => "virtual",
        }
    }

    /// (sectors per cluster, bytes per sector, total clusters) as reported by
    /// INT 21h AH=1Ch/36h. Hard disks keep the long-standing fake 80 MB and
    /// CD-ROMs report what MSCDEX-style redirectors typically do.
    pub fn geometry(self) -> (u16, u16, u16) {
        match self {
            DriveKind::Floppy => (1, 512, FLOPPY_CLUSTERS),
            DriveKind::HardDisk => (8, 512, 20000),
            DriveKind::CdRom => (1, 2048, 0xFFFF),
            DriveKind::Virtual => (1, 512, 2000),
        }
    }
}

/// How a host directory is presented to DOS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountOptions {
    pub kind: DriveKind,
    pub label: Option<String>,
    pub read_only: bool,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self {
            kind: DriveKind::HardDisk,
            label: None,
            read_only: false,
        }
    }
}

/// Public snapshot of a mounted drive.
#[derive(Clone, Debug)]
pub struct DriveInfo {
    pub drive: u8,
    pub kind: DriveKind,
    /// Host directory; `None` for the virtual Z: drive.
    pub root: Option<PathBuf>,
    pub label: String,
    /// True for CD-ROMs, `-ro` mounts and Z:.
    pub read_only: bool,
    pub current_dir: String,
}

impl DriveInfo {
    pub fn letter(&self) -> char {
        drive_letter(self.drive)
    }
}

struct Drive {
    kind: DriveKind,
    root: PathBuf,       // Host directory acting as X:\ (empty for Z:)
    current_dir: String, // DOS directory relative to root (e.g., "GAMES\DOOM")
    label: String,
    read_only: bool,
}

impl Drive {
    fn writable(&self) -> bool {
        !self.read_only
    }
}

struct OpenFile {
    /// None for Z: virtual files, which have no backing file.
    file: Option<File>,
    drive: u8,
    /// PSP of the process that opened the file. DOS closes a process's files
    /// when it terminates.
    owner: u16,
}

/// Helper struct to transfer directory search results back to the CPU
#[allow(dead_code)]
pub struct DosDirEntry {
    pub filename: String,
    pub size: u32,
    pub is_dir: bool,
    pub is_readonly: bool,
    pub dos_time: u16,
    pub dos_date: u16,
    /// DOS attribute byte (0x01 R/O, 0x08 label, 0x10 dir, 0x20 archive).
    pub attr: u8,
}

pub struct DiskController {
    // Map DOS Handle (u16) -> Rust File Object
    open_files: HashMap<u16, OpenFile>,

    // File System State
    drives: [Option<Drive>; 26],
    current_drive: u8,                       // 0=A, ... 2=C, ... 25=Z
    virtual_files: HashMap<String, Vec<u8>>, // In-memory files for Z: drive
}

impl DiskController {
    /// Creates the controller with C: backed by `root_path` and the virtual
    /// Z: drive. C: and Z: are always present; everything else is mounted.
    pub fn new(root_path: PathBuf) -> Self {
        // Ensure root path exists
        if !root_path.exists() {
            println!(
                "[DISK] Warning: Root path {:?} does not exist. Creating it.",
                root_path
            );
            let _ = fs::create_dir_all(&root_path);
        }

        let canonical = fs::canonicalize(&root_path).unwrap_or(root_path);

        let mut virtual_files = HashMap::new();
        // Create a dummy COMMAND.COM on Z:
        virtual_files.insert("COMMAND.COM".to_string(), vec![0x90; 5000]);

        let mut drives: [Option<Drive>; 26] = std::array::from_fn(|_| None);
        drives[DRIVE_C as usize] = Some(Drive {
            kind: DriveKind::HardDisk,
            root: canonical,
            current_dir: String::new(),
            label: DEFAULT_LABEL.to_string(),
            read_only: false,
        });
        drives[DRIVE_Z as usize] = Some(Drive {
            kind: DriveKind::Virtual,
            root: PathBuf::new(),
            current_dir: String::new(),
            label: DEFAULT_LABEL.to_string(),
            read_only: true,
        });

        Self {
            open_files: HashMap::new(),
            drives,
            current_drive: DRIVE_C, // Default to C:
            virtual_files,
        }
    }

    // ========================================================================
    // MOUNTS
    // ========================================================================

    /// Host directory currently backing drive C:.
    pub fn root_path(&self) -> &Path {
        self.drives[DRIVE_C as usize]
            .as_ref()
            .map(|d| d.root.as_path())
            .unwrap_or(Path::new(""))
    }

    /// Mount a host directory as `drive`. Unless `replace` is set, the drive
    /// must not already be mounted. Replacing closes the files open on it.
    pub fn mount(
        &mut self,
        drive: u8,
        path: &Path,
        opts: MountOptions,
        replace: bool,
    ) -> Result<PathBuf, String> {
        if drive >= LASTDRIVE {
            return Err("Invalid drive letter".to_string());
        }
        let letter = drive_letter(drive);
        if drive == DRIVE_Z || opts.kind == DriveKind::Virtual {
            return Err(format!("Drive {}: is reserved", letter));
        }
        if self.is_mounted(drive) && !replace {
            return Err(format!("Drive {}: is already mounted", letter));
        }
        if !path.is_dir() {
            return Err(format!("{} is not a directory", path.display()));
        }
        let canonical = fs::canonicalize(path).map_err(|e| e.to_string())?;

        self.close_drive_files(drive);
        self.drives[drive as usize] = Some(Drive {
            kind: opts.kind,
            root: canonical.clone(),
            current_dir: String::new(),
            label: opts
                .label
                .as_deref()
                .map(normalize_label)
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| DEFAULT_LABEL.to_string()),
            read_only: opts.read_only || opts.kind == DriveKind::CdRom,
        });
        Ok(canonical)
    }

    /// Unmount `drive`, closing its open files. C: and Z: cannot be removed.
    /// If it was the current drive, C: becomes current.
    pub fn unmount(&mut self, drive: u8) -> Result<(), String> {
        if drive >= LASTDRIVE || !self.is_mounted(drive) {
            return Err("Drive not mounted".to_string());
        }
        if drive == DRIVE_C || drive == DRIVE_Z {
            return Err(format!(
                "Drive {}: cannot be unmounted",
                drive_letter(drive)
            ));
        }
        self.close_drive_files(drive);
        self.drives[drive as usize] = None;
        if self.current_drive == drive {
            self.current_drive = DRIVE_C;
        }
        Ok(())
    }

    fn close_drive_files(&mut self, drive: u8) {
        self.open_files.retain(|_, f| f.drive != drive);
    }

    fn drive(&self, drive: u8) -> Option<&Drive> {
        self.drives.get(drive as usize).and_then(|d| d.as_ref())
    }

    pub fn is_mounted(&self, drive: u8) -> bool {
        self.drive(drive).is_some()
    }

    pub fn drive_kind(&self, drive: u8) -> Option<DriveKind> {
        self.drive(drive).map(|d| d.kind)
    }

    pub fn is_writable(&self, drive: u8) -> bool {
        self.drive(drive).is_some_and(|d| d.writable())
    }

    pub fn volume_label(&self, drive: u8) -> Option<String> {
        self.drive(drive).map(|d| d.label.clone())
    }

    pub fn drive_info(&self, drive: u8) -> Option<DriveInfo> {
        self.drive(drive).map(|d| DriveInfo {
            drive,
            kind: d.kind,
            root: (d.kind != DriveKind::Virtual).then(|| d.root.clone()),
            label: d.label.clone(),
            read_only: !d.writable(),
            current_dir: d.current_dir.to_ascii_uppercase(),
        })
    }

    pub fn mounted_drives(&self) -> Vec<DriveInfo> {
        (0..LASTDRIVE).filter_map(|d| self.drive_info(d)).collect()
    }

    /// Mounted drives of the given kind, in drive-letter order.
    pub fn drives_of_kind(&self, kind: DriveKind) -> Vec<u8> {
        (0..LASTDRIVE)
            .filter(|&d| self.drive_kind(d) == Some(kind))
            .collect()
    }

    /// Drive a file handle was opened on (Z: for virtual handles).
    pub fn handle_drive(&self, handle: u16) -> Option<u8> {
        self.open_files.get(&handle).map(|f| f.drive)
    }

    pub fn set_current_drive(&mut self, drive: u8) -> u8 {
        // Only switch to drives that exist; always report LASTDRIVE.
        if self.is_mounted(drive) {
            self.current_drive = drive;
        }
        LASTDRIVE
    }

    pub fn get_current_drive(&self) -> u8 {
        self.current_drive
    }

    // ========================================================================
    // PATH RESOLUTION
    // ========================================================================

    /// Split a DOS path into (drive, remainder), defaulting to the current
    /// drive. Returns None for a malformed drive specifier such as "@:".
    fn split_drive<'a>(&self, path: &'a str) -> Option<(u8, &'a str)> {
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
    fn logical_components<'a>(drive: &'a Drive, rest: &'a str) -> Vec<&'a str> {
        let mut components: Vec<&str> = Vec::new();
        if !rest.starts_with('\\') {
            components.extend(drive.current_dir.split('\\').filter(|p| !p.is_empty()));
        }
        for part in rest.split('\\') {
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

    /// Resolves a DOS path (e.g., "GAMES\DOOM.EXE", "..\FILE.TXT" or
    /// "D:\DATA") to a Host Path, ensuring it stays within the drive's root.
    /// Handles case-insensitivity and short filenames (8.3).
    pub fn resolve_path(&self, dos_path: &str) -> Option<PathBuf> {
        self.locate(dos_path).map(|(_, path)| path)
    }

    /// Like `resolve_path`, but also returns the drive the path is on.
    fn locate(&self, dos_path: &str) -> Option<(u8, PathBuf)> {
        let path_str = dos_path.replace('/', "\\");
        let (drive, rest) = self.split_drive(&path_str)?;
        self.resolve_on(drive, rest).map(|p| (drive, p))
    }

    fn resolve_on(&self, drive_num: u8, rest: &str) -> Option<PathBuf> {
        let drive = self.drive(drive_num)?;
        // Z: files are served from memory; callers check `is_virtual_file`.
        if drive.kind == DriveKind::Virtual {
            return None;
        }

        // Traverse and Resolve to Host Paths. ".." handling in
        // logical_components can't climb above the root.
        let mut full_path = drive.root.clone();
        for part in Self::logical_components(drive, rest) {
            let actual_name = self.find_host_child(&full_path, part);
            full_path.push(actual_name);
        }

        // Final Security Check
        if full_path.starts_with(&drive.root) {
            Some(full_path)
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

    // Helper to check if a file exists on Z:
    pub fn is_virtual_file(&self, filename: &str) -> bool {
        self.virtual_file_name(filename)
            .is_some_and(|name| self.virtual_files.contains_key(&name))
    }

    /// Z:-relative name of `filename` if it refers to the Z: drive.
    fn virtual_file_name(&self, filename: &str) -> Option<String> {
        let normalized = filename.replace('/', "\\");
        let (drive, rest) = self.split_drive(&normalized)?;
        (drive == DRIVE_Z).then(|| rest.trim_start_matches('\\').to_ascii_uppercase())
    }

    // Helper to get virtual file size
    pub fn get_virtual_file_size(&self, filename: &str) -> u32 {
        self.virtual_file_name(filename)
            .and_then(|name| self.virtual_files.get(&name))
            .map(|data| data.len() as u32)
            .unwrap_or(0)
    }

    /// Helper to find a child in a directory matching DOS semantics
    /// (Case-Insensitive OR Short Filename match)
    fn find_host_child(&self, dir: &Path, target: &str) -> String {
        // Read directory and sort for deterministic short names
        let mut entries: Vec<String> = Vec::new();
        if let Ok(read_dir) = fs::read_dir(dir) {
            for entry in read_dir.flatten() {
                entries.push(entry.file_name().to_string_lossy().to_string());
            }
        }
        entries.sort(); // Ensure ~1 order is consistent

        let target_upper = target.to_ascii_uppercase();
        let mut generated_counts: HashMap<String, usize> = HashMap::new();

        for name in entries {
            if name.starts_with('.') {
                continue;
            }

            // 1. Exact/Case-Insensitive Match
            if name.eq_ignore_ascii_case(target) {
                return name;
            }

            // 2. Short Name Match
            // Generate Short Name for this entry
            let (stem, ext) = Self::to_short_name(&name);
            let base_key = if ext.is_empty() {
                stem.clone()
            } else {
                format!("{}.{}", stem, ext)
            };

            let count = *generated_counts.get(&base_key).unwrap_or(&0);
            let final_short_name = if count == 0 {
                generated_counts.insert(base_key, 1);
                if ext.is_empty() {
                    stem
                } else {
                    format!("{}.{}", stem, ext)
                }
            } else {
                generated_counts.insert(base_key, count + 1);
                let suffix = format!("~{}", count);
                let available_len = 8usize.saturating_sub(suffix.len());
                let short_stem = if stem.len() > available_len {
                    &stem[0..available_len]
                } else {
                    &stem
                };

                if ext.is_empty() {
                    format!("{}{}", short_stem, suffix)
                } else {
                    format!("{}{}.{}", short_stem, suffix, ext)
                }
            };

            if final_short_name == target_upper {
                return name; // Found the host file corresponding to the short name
            }
        }

        // Not found? Return target as uppercase (default for creation)
        target.to_ascii_uppercase()
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
        if drive.kind == DriveKind::Virtual {
            // Z: is a flat directory: only its root exists.
            return Self::logical_components(drive, rest).is_empty();
        }

        // Resolve the new path to check existence
        if let Some(host_path) = self.resolve_on(drive_num, rest) {
            if host_path.is_dir() {
                // Store the DOS representation (relative to root)
                if let Ok(suffix) = host_path.strip_prefix(&drive.root) {
                    let dos_dir = suffix.to_string_lossy().replace('/', "\\");
                    if let Some(d) = self.drives[drive_num as usize].as_mut() {
                        d.current_dir = dos_dir;
                    }
                    return true;
                }
            }
        }
        false
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

    // ========================================================================
    // FILE I/O OPERATIONS
    // ========================================================================

    fn check_writable(&self, drive: u8) -> Result<(), u8> {
        match self.drive(drive) {
            None => Err(0x03),
            Some(d) if !d.writable() => Err(0x05), // Access denied
            Some(_) => Ok(()),
        }
    }

    /// Lowest unused handle, like DOS taking the first free job file table
    /// slot. Programs expect small numbers: the Microsoft C runtime rejects
    /// handles at or above its 20-entry file table.
    fn free_handle(&self) -> Result<u16, u8> {
        (FIRST_USER_HANDLE..HANDLE_LIMIT)
            .find(|h| !self.open_files.contains_key(h))
            .ok_or(0x04) // Too many open files
    }

    // INT 21h, AH=3Dh: Open File. `owner` is the PSP of the calling process.
    pub fn open_file(&mut self, filename: &str, mode: u8, owner: u16) -> Result<u16, u8> {
        // Handle Virtual Z: files
        if self.is_virtual_file(filename) {
            // Virtual files only need to "exist" so programs like NC find
            // COMMAND.COM; EXEC loads them itself. They have no backing
            // file, so reads on the handle fail, which is fine.
            let handle = self.free_handle()?;
            self.open_files.insert(
                handle,
                OpenFile {
                    file: None,
                    drive: DRIVE_Z,
                    owner,
                },
            );
            return Ok(handle);
        }

        let (drive, path) = self.locate(filename).ok_or(0x03)?; // Path not found
        let writable = self.is_writable(drive);

        let mut options = OpenOptions::new();
        match mode & 0x03 {
            0 => {
                options.read(true);
            }
            1 => {
                if !writable {
                    return Err(0x05);
                }
                options.write(true).create(true).truncate(false);
            } // logic tweak for safety
            2 => {
                // Read/write opens on read-only media (CD-ROM) are quietly
                // downgraded to read-only, as MSCDEX does; lots of CD games
                // open their data files R/W without ever writing.
                if writable {
                    options.read(true).write(true).create(true);
                } else {
                    options.read(true);
                }
            }
            _ => return Err(0x0C),
        }

        let handle = self.free_handle()?;
        match options.open(path) {
            Ok(file) => {
                self.open_files.insert(
                    handle,
                    OpenFile {
                        file: Some(file),
                        drive,
                        owner,
                    },
                );
                Ok(handle)
            }
            Err(_) => Err(0x02),
        }
    }

    // INT 21h, AH=3Ch: Create File. Opens read/write, creating the file if
    // missing but never truncating (see int21.rs for why).
    pub fn create_file(&mut self, filename: &str, owner: u16) -> Result<u16, u8> {
        let normalized = filename.replace('/', "\\");
        let (drive, _) = self.split_drive(&normalized).ok_or(0x03)?;
        self.check_writable(drive)?;
        self.open_file(filename, 0x02, owner)
    }

    // INT 21h, AH=3Eh: Close File
    pub fn close_file(&mut self, handle: u16) -> bool {
        self.open_files.remove(&handle).is_some()
    }

    /// Close the files a terminating process opened, as DOS does on exit.
    pub fn close_process_files(&mut self, owner: u16) {
        self.open_files.retain(|_, f| f.owner != owner);
    }

    /// Close every open file, for when the shell is reloaded.
    pub fn close_all_files(&mut self) {
        self.open_files.clear();
    }

    // INT 21h, AH=3Fh: Read from File
    pub fn read_file(&mut self, handle: u16, count: usize) -> Result<Vec<u8>, u16> {
        if let Some(open) = self.open_files.get_mut(&handle) {
            let file = open.file.as_mut().ok_or(0x05u16)?;
            let mut buffer = vec![0u8; count];
            match file.read(&mut buffer) {
                Ok(bytes_read) => {
                    buffer.truncate(bytes_read);
                    Ok(buffer)
                }
                Err(_) => Err(0x05),
            }
        } else {
            Err(0x06)
        }
    }

    // INT 21h, AH=40h: Write to File
    pub fn write_file(&mut self, handle: u16, data: &[u8]) -> Result<u16, u8> {
        if let Some(open) = self.open_files.get_mut(&handle) {
            let file = open.file.as_mut().ok_or(0x05u8)?;
            match file.write(data) {
                Ok(bytes_written) => Ok(bytes_written as u16),
                Err(_) => Err(0x05),
            }
        } else {
            Err(0x06)
        }
    }

    // INT 21h, AH=42h: Seek
    pub fn seek_file(&mut self, handle: u16, offset: i64, origin: u8) -> Result<u64, u16> {
        if let Some(open) = self.open_files.get_mut(&handle) {
            let file = open.file.as_mut().ok_or(0x05u16)?;
            let seek_from = match origin {
                0 => SeekFrom::Start(offset as u64),
                1 => SeekFrom::Current(offset),
                2 => SeekFrom::End(offset),
                _ => return Err(0x01),
            };
            match file.seek(seek_from) {
                Ok(new_pos) => Ok(new_pos),
                Err(_) => Err(0x19),
            }
        } else {
            Err(0x06)
        }
    }

    // ========================================================================
    // FILESYSTEM METADATA & SEARCH
    // ========================================================================

    /// Allocation geometry of `drive` (0-based): (sectors per cluster, bytes
    /// per sector, total clusters).
    pub fn drive_geometry(&self, drive: u8) -> Option<(u16, u16, u16)> {
        self.drive_kind(drive).map(DriveKind::geometry)
    }

    // INT 21h, AH=36h: Get Disk Free Space
    // Input DL: 0=Default, 1=A, 2=B, 3=C, ...
    // Returns (sectors per cluster, free clusters, bytes per sector, total clusters)
    pub fn get_disk_free_space(&self, drive: u8) -> Result<(u16, u16, u16, u16), u16> {
        let target_drive = if drive == 0 {
            self.current_drive
        } else {
            drive - 1
        };

        let d = self.drive(target_drive).ok_or(0x0Fu16)?; // Invalid Drive
        let (spc, bps, total) = d.kind.geometry();
        let free = match d.kind {
            // Floppies report real usage so programs can tell whether a save
            // will fit; hard disks keep reporting an empty fake 80 MB.
            DriveKind::Floppy => {
                let cluster_bytes = spc as u64 * bps as u64;
                let used = Self::used_clusters(&d.root, cluster_bytes, total as u64);
                total - used.min(total as u64) as u16
            }
            DriveKind::HardDisk => total,
            DriveKind::CdRom => 0,
            DriveKind::Virtual => 1000,
        };
        Ok((spc, free, bps, total))
    }

    /// Clusters occupied by the tree under `root` (one per directory plus
    /// each file rounded up). Stops counting once `cap` is reached so that
    /// mounting a huge directory as a floppy stays cheap. Symlinks are not
    /// followed.
    fn used_clusters(root: &Path, cluster_bytes: u64, cap: u64) -> u64 {
        let mut used = 0u64;
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            let Ok(read_dir) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in read_dir.flatten() {
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                    continue;
                };
                if meta.is_dir() {
                    used += 1;
                    pending.push(entry.path());
                } else if meta.is_file() {
                    used += meta.len().div_ceil(cluster_bytes);
                }
                if used >= cap {
                    return cap;
                }
            }
        }
        used
    }

    // INT 21h, AH=43h: Get File Attributes
    // Returns: Attribute Byte (0x20 = Archive, 0x10 = Subdir, etc.)
    #[allow(dead_code)]
    pub fn get_file_attribute(&self, filename: &str) -> Result<u16, u8> {
        if self.is_virtual_file(filename) {
            return Ok(0x21); // Archive + R/O
        }
        let (drive, path) = self.locate(filename).ok_or(0x03)?;
        if !path.exists() {
            return Err(0x02); // File Not Found
        }

        let mut attr: u16 = 0;
        if path.is_dir() {
            attr |= 0x10; // Directory
        } else {
            attr |= 0x20; // Archive (standard file)
        }
        // Reflect host read-only state into DOS R/O bit. On Unix, read-only means
        // no user-write permission. On Windows, the readonly flag. Everything
        // on read-only media is R/O too.
        let host_ro = fs::metadata(&path).is_ok_and(|m| m.permissions().readonly());
        if host_ro || !self.is_writable(drive) {
            attr |= 0x01;
        }
        Ok(attr)
    }

    /// DOS AH=43h AL=01: Set file attributes. We honor just the R/O bit because
    /// the host filesystem generally doesn't have direct analogs for DOS's
    /// Hidden/System bits. Directory and Volume Label bits cannot be set via
    /// this call on real DOS either.
    pub fn set_file_attribute(&self, filename: &str, attr: u16) -> Result<(), u8> {
        let (drive, path) = self.locate(filename).ok_or(0x03)?;
        if !path.exists() {
            return Err(0x02);
        }
        self.check_writable(drive)?;
        if let Ok(meta) = fs::metadata(&path) {
            let mut perms = meta.permissions();
            let want_ro = (attr & 0x01) != 0;
            if perms.readonly() != want_ro {
                perms.set_readonly(want_ro);
                // Ignore permission-set errors on systems where it's not supported.
                let _ = fs::set_permissions(&path, perms);
            }
        }
        Ok(())
    }

    /// DOS AH=39h: Create a directory at the given DOS path.
    pub fn create_directory(&self, path: &str) -> Result<(), u8> {
        let normalized = path.replace('/', "\\");
        let (drive, rest) = self.split_drive(&normalized).ok_or(0x03)?;
        self.check_writable(drive)?;

        // resolve_path walks any existing leaf, but MKDIR needs to create a new
        // leaf — so resolve the parent, then append the final component.
        let (parent_dos, leaf) = match rest.rsplit_once('\\') {
            Some(("", l)) => ("\\", l),
            Some((p, l)) => (p, l),
            None => (".", rest), // current directory of that drive
        };
        if leaf.is_empty() {
            return Err(0x03); // Path not found / invalid
        }
        let parent_path = self.resolve_on(drive, parent_dos).ok_or(0x03)?;
        if !parent_path.is_dir() {
            return Err(0x03);
        }
        // Case-insensitive: MKDIR "Foo" should collide with existing "FOO".
        if let Some(existing) = self.find_existing_child(&parent_path, leaf) {
            let full = parent_path.join(existing);
            if full.exists() {
                return Err(0x05); // Access denied / already exists
            }
        }
        let target = parent_path.join(leaf.to_uppercase());
        fs::create_dir(&target).map_err(|_| 0x05)
    }

    /// DOS AH=3Ah: Remove an empty directory.
    pub fn remove_directory(&self, path: &str) -> Result<(), u8> {
        let (drive_num, host_path) = self.locate(path).ok_or(0x03)?;
        if !host_path.exists() {
            return Err(0x03);
        }
        if !host_path.is_dir() {
            return Err(0x03);
        }
        self.check_writable(drive_num)?;
        // DOS error 0x10 = "attempt to remove current directory".
        if let Some(drive) = self.drive(drive_num) {
            if let Ok(rel) = host_path.strip_prefix(&drive.root) {
                let dos_form = rel.to_string_lossy().replace('/', "\\");
                if dos_form.eq_ignore_ascii_case(&drive.current_dir) {
                    return Err(0x10);
                }
            }
        }
        fs::remove_dir(&host_path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => 0x03,
            _ => 0x05, // Access denied (e.g. not empty)
        })
    }

    /// Case-insensitive lookup of a child by name in a host directory.
    fn find_existing_child(&self, parent: &Path, name: &str) -> Option<std::ffi::OsString> {
        let upper = name.to_uppercase();
        if let Ok(entries) = fs::read_dir(parent) {
            for e in entries.flatten() {
                let fname = e.file_name();
                if fname.to_string_lossy().to_uppercase() == upper {
                    return Some(fname);
                }
            }
        }
        None
    }

    // Returns the path string relative to root, e.g., "GAMES\DOOM"
    fn to_short_name(filename: &str) -> (String, String) {
        let filename = filename.to_uppercase();

        let (stem, ext) = match filename.rsplit_once('.') {
            Some((s, e)) => (s, e),
            None => (filename.as_str(), ""),
        };

        // Filter invalid chars
        let mut clean_stem: String = stem
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || "!@#$%^&()-_'{}`~".contains(*c))
            .collect();

        let mut clean_ext: String = ext.chars().filter(|c| c.is_ascii_alphanumeric()).collect();

        if clean_ext.len() > 3 {
            clean_ext.truncate(3);
        }
        if clean_stem.len() > 8 {
            clean_stem.truncate(8);
        }

        if clean_stem.is_empty() {
            clean_stem = "NONAME".to_string();
        }

        (clean_stem, clean_ext)
    }

    /// Helper: Simple DOS wildcard matching (? and *)
    fn matches_pattern(filename: &str, pattern: &str) -> bool {
        if pattern == "*.*" {
            return true;
        }

        // Split filename and pattern by '.'
        let (f_name, f_ext) = filename.split_once('.').unwrap_or((filename, ""));
        let (p_name, p_ext) = pattern.split_once('.').unwrap_or((pattern, ""));

        let match_part = |f: &str, p: &str| -> bool {
            if p == "*" {
                return true;
            }
            let mut f_chars = f.chars();
            let mut p_chars = p.chars();
            loop {
                match (f_chars.next(), p_chars.next()) {
                    (None, None) => return true,
                    (Some(_), None) => return false, // Filename longer than pattern
                    (None, Some(pc)) => {
                        if pc == '*' {
                            return true;
                        }
                        if pc == '?' {
                            continue;
                        } // Treat ? as match for "empty" (padding)
                        return false;
                    }
                    (Some(fc), Some(pc)) => {
                        if pc == '*' {
                            return true;
                        }
                        if pc == '?' {
                            continue;
                        }
                        if pc.to_ascii_uppercase() != fc.to_ascii_uppercase() {
                            return false;
                        }
                    }
                }
            }
        };

        match_part(f_name, p_name) && match_part(f_ext, p_ext)
    }

    /// A volume label as FindFirst reports it: labels longer than 8
    /// characters get a dot after the 8th, like a filename.
    fn label_entry(label: &str) -> DosDirEntry {
        let filename = if label.len() > 8 {
            format!("{}.{}", &label[..8], &label[8..])
        } else {
            label.to_string()
        };
        DosDirEntry {
            filename,
            size: 0,
            is_dir: false,
            is_readonly: false,
            dos_time: 0x0000,
            dos_date: 0x5021,
            attr: 0x08,
        }
    }

    // INT 21h, AH=4E/4F: Find First / Find Next
    // search_spec contains the path AND the pattern e.g. "C:\GAMES\*.EXE" or "*.EXE"
    pub fn find_directory_entry(
        &self,
        search_spec: &str,
        search_index: usize,
        search_attr: u16,
    ) -> Result<DosDirEntry, u8> {
        self.list_directory(search_spec, search_attr)?
            .into_iter()
            .nth(search_index)
            .ok_or(0x12)
    }

    /// All entries matching a search spec, in the order FindFirst/FindNext
    /// return them. Directory listings (DIR) use this directly.
    pub fn list_directory(
        &self,
        search_spec: &str,
        search_attr: u16,
    ) -> Result<Vec<DosDirEntry>, u8> {
        let normalized = search_spec.replace('/', "\\");
        let (drive_num, rest) = self.split_drive(&normalized).ok_or(0x03)?;
        let drive = self.drive(drive_num).ok_or(0x03)?;

        // Handle Volume Label request
        if (search_attr & 0x08) != 0 {
            return Ok(vec![Self::label_entry(&drive.label)]);
        }

        // Split Spec into Directory and Pattern
        let (parent_dir, pattern) = match rest.rfind('\\') {
            Some(idx) => rest.split_at(idx + 1),
            None => ("", rest),
        };
        let search_dir_str = if parent_dir.is_empty() {
            "."
        } else {
            parent_dir
        };

        let mut valid_entries: Vec<DosDirEntry> = Vec::new();

        if drive.kind == DriveKind::Virtual {
            // Virtual Z: Drive Listing: a single flat directory
            if !Self::logical_components(drive, search_dir_str).is_empty() {
                return Err(0x03);
            }
            let mut names: Vec<&String> = self.virtual_files.keys().collect();
            names.sort();
            for fname in names {
                if Self::matches_pattern(fname, pattern) {
                    valid_entries.push(DosDirEntry {
                        filename: fname.clone(),
                        size: self.virtual_files[fname].len() as u32,
                        is_dir: false,
                        is_readonly: true,
                        dos_time: 0x0000,
                        dos_date: 0x5021,
                        attr: 0x21,
                    });
                }
            }
            return Ok(valid_entries);
        }

        // Host Filesystem Listing
        let host_dir = self.resolve_on(drive_num, search_dir_str).ok_or(0x03)?;

        let read_dir = fs::read_dir(&host_dir).map_err(|_| 0x03)?;
        let mut all_entries: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();
        all_entries.sort_by_key(|dir_entry| dir_entry.file_name());

        let mut generated_names: HashMap<String, usize> = HashMap::new();

        let is_host_root = host_dir == drive.root;
        let media_ro = !drive.writable();

        if !is_host_root {
            for dot in ["..", "."] {
                if Self::matches_pattern(dot, pattern) {
                    valid_entries.push(DosDirEntry {
                        filename: dot.to_string(),
                        size: 0,
                        is_dir: true,
                        is_readonly: false,
                        dos_time: 0,
                        dos_date: 0,
                        attr: 0x10,
                    });
                }
            }
        }

        for entry in all_entries {
            let original_name = entry.file_name().to_string_lossy().into_owned();

            if original_name.starts_with('.') {
                continue;
            }

            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            let is_dir = metadata.is_dir();
            let is_readonly = media_ro || metadata.permissions().readonly();
            let mut file_attr: u8 = if is_dir { 0x10 } else { 0x20 };
            if is_readonly {
                file_attr |= 0x01;
            }

            let restricted_bits = 0x02 | 0x04 | 0x10;
            if (file_attr as u16 & restricted_bits) & !search_attr != 0 {
                continue;
            }

            let (stem, ext) = Self::to_short_name(&original_name);
            let base_key = if ext.is_empty() {
                stem.clone()
            } else {
                format!("{}.{}", stem, ext)
            };

            let count = *generated_names.get(&base_key).unwrap_or(&0);

            let final_name = if count == 0 {
                generated_names.insert(base_key, 1);
                if ext.is_empty() {
                    stem
                } else {
                    format!("{}.{}", stem, ext)
                }
            } else {
                generated_names.insert(base_key.clone(), count + 1);
                let suffix = format!("~{}", count);
                let available_len = 8usize.saturating_sub(suffix.len());
                let short_stem = if stem.len() > available_len {
                    &stem[0..available_len]
                } else {
                    &stem
                };

                if ext.is_empty() {
                    format!("{}{}", short_stem, suffix)
                } else {
                    format!("{}{}.{}", short_stem, suffix, ext)
                }
            };

            if !Self::matches_pattern(&final_name, pattern) {
                continue;
            }

            let sys_time = metadata.modified().unwrap_or(std::time::SystemTime::now());
            let datetime: DateTime<Local> = sys_time.into();
            let dos_time = ((datetime.hour() as u16) << 11)
                | ((datetime.minute() as u16) << 5)
                | ((datetime.second() as u16) / 2);
            let year = datetime.year();
            let dos_date = if year < 1980 {
                0x0021
            } else {
                (((year - 1980) as u16) << 9)
                    | ((datetime.month() as u16) << 5)
                    | (datetime.day() as u16)
            };

            valid_entries.push(DosDirEntry {
                filename: final_name,
                size: metadata.len() as u32,
                is_dir,
                is_readonly,
                dos_time,
                dos_date,
                attr: file_attr,
            });
        }

        Ok(valid_entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PSP that owns the files the tests open.
    const PSP: u16 = 0x1000;

    /// A fresh scratch directory under target/ for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from("target/test_disk_unit").join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cdrom() -> MountOptions {
        MountOptions {
            kind: DriveKind::CdRom,
            ..Default::default()
        }
    }

    #[test]
    fn parse_drive_prefix_handles_letters_and_non_ascii() {
        assert_eq!(parse_drive_prefix("d:\\X"), (Some(3), "\\X"));
        assert_eq!(parse_drive_prefix("FILE.TXT"), (None, "FILE.TXT"));
        assert_eq!(parse_drive_prefix("\u{FFFD}:X"), (None, "\u{FFFD}:X"));
        assert_eq!(parse_drive_prefix("é"), (None, "é"));
        assert_eq!(parse_drive_prefix(""), (None, ""));
    }

    #[test]
    fn each_drive_has_its_own_current_directory() {
        let base = scratch("cwd");
        fs::create_dir_all(base.join("c/GAMES")).unwrap();
        fs::create_dir_all(base.join("d/DATA")).unwrap();
        let mut disk = DiskController::new(base.join("c"));
        disk.mount(3, &base.join("d"), MountOptions::default(), false)
            .unwrap();

        assert!(disk.set_current_directory("GAMES"));
        assert!(disk.set_current_directory("D:\\DATA"));
        assert_eq!(disk.get_current_drive(), DRIVE_C);
        assert_eq!(disk.get_current_directory(), "GAMES");
        assert_eq!(disk.get_current_directory_of(3).unwrap(), "DATA");

        fs::write(base.join("d/DATA/f.txt"), b"x").unwrap();
        let resolved = disk.resolve_path("D:F.TXT").unwrap();
        assert!(resolved.ends_with("DATA/f.txt"));
        assert_eq!(disk.qualify_directory("D:*.*").as_deref(), Some("D:\\DATA"));
        assert_eq!(disk.qualify_directory("\\*.*").as_deref(), Some("C:\\"));
    }

    #[test]
    fn unmounted_and_reserved_drives() {
        let base = scratch("reserved");
        let mut disk = DiskController::new(base.clone());
        assert!(disk.resolve_path("E:\\X").is_none());
        assert_eq!(disk.set_current_drive(4), LASTDRIVE);
        assert_eq!(disk.get_current_drive(), DRIVE_C);
        assert!(
            disk.mount(DRIVE_Z, &base, MountOptions::default(), true)
                .is_err()
        );
        assert!(disk.unmount(DRIVE_C).is_err());
        assert!(disk.unmount(DRIVE_Z).is_err());
        assert!(disk.unmount(4).is_err());
        assert!(
            disk.mount(3, &base.join("missing"), MountOptions::default(), false)
                .is_err()
        );
    }

    #[test]
    fn mkdir_with_drive_prefix_lands_on_that_drive() {
        let base = scratch("mkdir");
        fs::create_dir_all(base.join("c")).unwrap();
        fs::create_dir_all(base.join("d")).unwrap();
        let mut disk = DiskController::new(base.join("c"));
        disk.mount(3, &base.join("d"), MountOptions::default(), false)
            .unwrap();

        disk.create_directory("D:NEWDIR").unwrap();
        disk.create_directory("D:\\ROOTDIR").unwrap();
        assert!(base.join("d/NEWDIR").is_dir());
        assert!(base.join("d/ROOTDIR").is_dir());
        assert!(!base.join("c/D:NEWDIR").exists());

        // Error 0x10 only applies to the target drive's current directory.
        assert!(disk.set_current_directory("D:\\NEWDIR"));
        assert_eq!(disk.remove_directory("D:\\NEWDIR"), Err(0x10));
        fs::create_dir_all(base.join("c/NEWDIR")).unwrap();
        assert_eq!(disk.remove_directory("C:\\NEWDIR"), Ok(()));
    }

    #[test]
    fn read_only_media_rejects_writes() {
        let base = scratch("readonly");
        fs::create_dir_all(base.join("c")).unwrap();
        fs::create_dir_all(base.join("cd/SUB")).unwrap();
        fs::write(base.join("cd/DATA.DAT"), b"cd data").unwrap();
        let mut disk = DiskController::new(base.join("c"));
        disk.mount(3, &base.join("cd"), cdrom(), false).unwrap();

        assert_eq!(disk.create_file("D:\\NEW.TXT", PSP), Err(0x05));
        assert_eq!(disk.open_file("D:\\DATA.DAT", 1, PSP), Err(0x05));
        assert_eq!(disk.create_directory("D:\\X"), Err(0x05));
        assert_eq!(disk.remove_directory("D:\\SUB"), Err(0x05));
        assert_eq!(disk.set_file_attribute("D:\\DATA.DAT", 0), Err(0x05));
        assert_eq!(disk.get_file_attribute("D:\\DATA.DAT"), Ok(0x21));

        // Mode 2 is downgraded: reads work, writes fail.
        let h = disk.open_file("D:\\DATA.DAT", 2, PSP).unwrap();
        assert_eq!(disk.read_file(h, 7).unwrap(), b"cd data");
        assert_eq!(disk.write_file(h, b"x"), Err(0x05));
        assert!(!base.join("cd/NEW.TXT").exists());
        assert_eq!(fs::read(base.join("cd/DATA.DAT")).unwrap(), b"cd data");

        let entries = disk.list_directory("D:\\*.*", 0x10).unwrap();
        assert!(entries.iter().all(|e| e.attr & 0x01 != 0));
    }

    #[test]
    fn unmount_and_remount_close_only_their_own_files() {
        let base = scratch("handles");
        for d in ["c", "c2", "d"] {
            fs::create_dir_all(base.join(d)).unwrap();
            fs::write(base.join(d).join("F.TXT"), b"1").unwrap();
        }
        let mut disk = DiskController::new(base.join("c"));
        disk.mount(3, &base.join("d"), MountOptions::default(), false)
            .unwrap();
        let hc = disk.open_file("C:\\F.TXT", 0, PSP).unwrap();
        let hd = disk.open_file("D:\\F.TXT", 0, PSP).unwrap();
        assert_eq!(disk.handle_drive(hd), Some(3));

        disk.mount(DRIVE_C, &base.join("c2"), MountOptions::default(), true)
            .unwrap();
        assert!(disk.read_file(hc, 1).is_err());
        assert!(disk.read_file(hd, 1).is_ok());

        disk.set_current_drive(3);
        disk.unmount(3).unwrap();
        assert!(disk.read_file(hd, 1).is_err());
        assert_eq!(disk.get_current_drive(), DRIVE_C);
    }

    #[test]
    fn handles_are_reused_and_closed_with_their_process() {
        let base = scratch("handle_reuse");
        fs::write(base.join("F.TXT"), b"1").unwrap();
        let mut disk = DiskController::new(base);
        let child = 0x2000;
        let parent_h = disk.open_file("F.TXT", 0, PSP).unwrap();
        assert_eq!(parent_h, FIRST_USER_HANDLE);
        let a = disk.open_file("F.TXT", 0, child).unwrap();
        let b = disk.open_file("F.TXT", 0, child).unwrap();
        assert!(disk.close_file(a));
        assert!(!disk.close_file(a));
        assert_eq!(disk.open_file("F.TXT", 0, child), Ok(a));

        disk.close_process_files(child);
        assert!(disk.read_file(a, 1).is_err());
        assert!(disk.read_file(b, 1).is_err());
        assert_eq!(disk.read_file(parent_h, 1).unwrap(), b"1");
        assert_eq!(disk.open_file("Z:\\COMMAND.COM", 0, child), Ok(a));
        assert_eq!(disk.handle_drive(a), Some(DRIVE_Z));
    }

    #[test]
    fn floppy_free_space_tracks_usage() {
        let base = scratch("floppy");
        fs::create_dir_all(base.join("c")).unwrap();
        fs::create_dir_all(base.join("a")).unwrap();
        let mut disk = DiskController::new(base.join("c"));
        let floppy = MountOptions {
            kind: DriveKind::Floppy,
            ..Default::default()
        };
        disk.mount(0, &base.join("a"), floppy, false).unwrap();

        let (_, empty_free, _, total) = disk.get_disk_free_space(1).unwrap();
        assert_eq!(empty_free, total);
        fs::write(base.join("a/SAVE.DAT"), vec![0u8; 1024]).unwrap();
        let (spc, free, bps, _) = disk.get_disk_free_space(1).unwrap();
        assert_eq!((spc, bps), (1, 512));
        assert_eq!(free, total - 2);
        assert_eq!(disk.get_disk_free_space(5), Err(0x0F));
    }

    #[test]
    fn volume_label_comes_from_the_searched_drive() {
        let base = scratch("label");
        fs::create_dir_all(base.join("c")).unwrap();
        fs::create_dir_all(base.join("d")).unwrap();
        let mut disk = DiskController::new(base.join("c"));
        let opts = MountOptions {
            kind: DriveKind::CdRom,
            label: Some("gamecd_disk1".to_string()),
            read_only: false,
        };
        disk.mount(3, &base.join("d"), opts, false).unwrap();

        let c = disk.find_directory_entry("*.*", 0, 0x08).unwrap();
        assert_eq!((c.filename.as_str(), c.attr), ("RUSTDOS", 0x08));
        let d = disk.find_directory_entry("D:\\*.*", 0, 0x08).unwrap();
        assert_eq!(d.filename, "GAMECD_D.ISK");
        assert_eq!(disk.volume_label(3).unwrap(), "GAMECD_DISK");
    }

    #[test]
    fn z_drive_is_only_used_when_named() {
        let base = scratch("zdrive");
        fs::write(base.join("HOST.TXT"), b"x").unwrap();
        let mut disk = DiskController::new(base.clone());
        disk.set_current_drive(DRIVE_Z);
        assert!(disk.is_virtual_file("COMMAND.COM"));
        assert!(!disk.is_virtual_file("C:\\COMMAND.COM"));
        let host = disk.list_directory("C:\\*.*", 0x10).unwrap();
        assert!(host.iter().any(|e| e.filename == "HOST.TXT"));
        let z = disk.list_directory("*.*", 0x10).unwrap();
        assert_eq!(z.len(), 1);
        assert_eq!(z[0].filename, "COMMAND.COM");
    }
}
