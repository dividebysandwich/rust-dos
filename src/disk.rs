use chrono::{DateTime, Datelike, Local, Timelike};
use std::cell::Cell;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::cdrom::image::CdImage;
use crate::cdrom::Extent;
use crate::memfs::{Bytes, MemFs, Node};
use crate::mount::MountSpec;

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

/// DOS time and date of the files held in memory: 1 January 2020.
const MEMORY_TIME: u16 = 0x0000;
const MEMORY_DATE: u16 = 0x5021;

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

/// Whether DOS allows `c` in a file name.
fn dos_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "!#$%&'()-@^_`{}~".contains(c)
}

/// Whether `name` already is a DOS 8.3 name, in any case.
fn is_short_name(name: &str) -> bool {
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    !stem.is_empty()
        && stem.len() <= 8
        && ext.len() <= 3
        && !name.ends_with('.')
        && stem.chars().chain(ext.chars()).all(dos_name_char)
}

/// The DOS names of the entries of a directory, given in the order that
/// numbers them, as DOSBox and Windows make them: names that are 8.3
/// already are only uppercased; the others lose their spaces and other
/// characters DOS doesn't allow, and become the start of the name with a
/// number, `~N`, counted per prefix: "Day Of The Tentacle.BIN" and ".cue"
/// are DAYOFT~1.BIN and DAYOFT~2.CUE.
pub(crate) fn short_names<S: AsRef<str>>(names: &[S]) -> Vec<String> {
    let mut used = std::collections::HashSet::new();
    let mut result = vec![String::new(); names.len()];
    // 8.3 names come first, so that no generated name takes one.
    let mut long = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let upper = name.as_ref().to_ascii_uppercase();
        if is_short_name(&upper) && used.insert(upper.clone()) {
            result[i] = upper;
        } else {
            long.push(i);
        }
    }
    let mut counters: HashMap<String, u32> = HashMap::new();
    for i in long {
        let upper = names[i].as_ref().to_ascii_uppercase();
        let (stem, ext) = match upper.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => (stem, ext),
            _ => (upper.as_str(), ""),
        };
        let mut stem: String = stem.chars().filter(|&c| dos_name_char(c)).collect();
        if stem.is_empty() {
            stem = "NONAME".to_string();
        }
        let ext: String = ext.chars().filter(|&c| dos_name_char(c)).take(3).collect();
        let counter = counters.entry(stem.chars().take(6).collect()).or_insert(0);
        loop {
            *counter += 1;
            let suffix = format!("~{}", counter);
            let keep = stem.len().min(8 - suffix.len());
            let mut name = format!("{}{}", &stem[..keep], suffix);
            if !ext.is_empty() {
                name = format!("{}.{}", name, ext);
            }
            if used.insert(name.clone()) {
                result[i] = name;
                break;
            }
        }
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Floppy,
    HardDisk,
    CdRom,
    /// A read-only drive held in memory: Z:, and the built-in Ultrasound
    /// software.
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
    /// Host directory; `None` for the drives held in memory and CD images.
    pub root: Option<PathBuf>,
    /// The CD image the drive shows.
    pub image: Option<PathBuf>,
    pub label: String,
    /// True for CD-ROMs, `-ro` mounts and the drives held in memory.
    pub read_only: bool,
    pub current_dir: String,
    /// The mount as it was asked for (the path as given, the options before
    /// the label defaults and CD images' type), as the configuration file
    /// has it. None for the drives held in memory.
    pub mount: Option<MountSpec>,
}

impl DriveInfo {
    pub fn letter(&self) -> char {
        drive_letter(self.drive)
    }
}

/// What holds a drive's files.
enum Storage {
    /// A host directory, acting as the drive's root.
    Host(PathBuf),
    /// A tree held in memory, whose files are either in memory too or on
    /// the CD image.
    Tree { files: MemFs, image: Option<Rc<CdImage>> },
}

struct Drive {
    kind: DriveKind,
    storage: Storage,
    current_dir: String, // DOS directory relative to root (e.g., "GAMES\DOOM")
    label: String,
    read_only: bool,
    /// The mount as it was asked for.
    mount: Option<MountSpec>,
}

impl Drive {
    fn writable(&self) -> bool {
        !self.read_only
    }

    /// The host directory behind the drive, if there is one.
    fn host_root(&self) -> Option<&Path> {
        match &self.storage {
            Storage::Host(root) => Some(root),
            Storage::Tree { .. } => None,
        }
    }

    /// The drive's files, if they are held in memory.
    fn tree(&self) -> Option<&MemFs> {
        match &self.storage {
            Storage::Tree { files, .. } => Some(files),
            Storage::Host(_) => None,
        }
    }

    fn image(&self) -> Option<&Rc<CdImage>> {
        match &self.storage {
            Storage::Tree { image, .. } => image.as_ref(),
            Storage::Host(_) => None,
        }
    }
}

struct OpenFile {
    data: OpenData,
    drive: u8,
    /// PSP of the process that opened the file. DOS closes a process's files
    /// when it terminates.
    owner: u16,
}

/// What an open handle reads and writes.
enum OpenData {
    Host(File),
    /// A file held in memory, and the position, which the handles
    /// duplicated from this one share.
    Memory(Bytes, Rc<Cell<u64>>),
    /// A file on a CD image, and the shared position.
    Image(Rc<CdImage>, Extent, Rc<Cell<u64>>),
    /// A character device opened by name (NUL, CON, PRN...).
    Device(CharDevice),
}

/// Where the contents of a file are.
#[derive(Clone, Debug)]
pub enum FileData {
    Host(PathBuf),
    Memory(Bytes),
    Image(Rc<CdImage>, Extent),
}

impl std::fmt::Debug for CdImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CdImage({})", self.path().display())
    }
}

impl FileData {
    /// The whole file.
    pub fn read(&self) -> std::io::Result<Bytes> {
        match self {
            FileData::Host(path) => fs::read(path).map(Bytes::Owned),
            FileData::Memory(data) => Ok(data.clone()),
            FileData::Image(image, extent) => {
                let mut data = vec![0u8; extent.size as usize];
                image.read_extent(extent, 0, &mut data)?;
                Ok(Bytes::Owned(data))
            }
        }
    }
}

impl FileData {
    fn of(node: &Node, image: Option<&Rc<CdImage>>) -> Option<Self> {
        match node {
            Node::Bytes(data) => Some(FileData::Memory(data.clone())),
            Node::Extent(extent) => Some(FileData::Image(image?.clone(), *extent)),
        }
    }
}

/// A DOS character device a program opened by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharDevice {
    /// NUL, and the printer and serial ports, which nothing is attached
    /// to: reads find nothing, writes vanish.
    Nul,
    /// CON: writes go to the screen.
    Con,
}

/// The character device a file name names. DOS finds devices by name in
/// any directory, with any extension: "C:\GAME\NUL.TXT" is NUL.
pub fn char_device(filename: &str) -> Option<CharDevice> {
    let last = filename.rsplit(['\\', '/', ':']).next()?;
    let stem = last.split('.').next()?.trim().to_ascii_uppercase();
    match stem.as_str() {
        "CON" => Some(CharDevice::Con),
        "NUL" | "PRN" | "AUX" | "LPT1" | "LPT2" | "LPT3" | "COM1" | "COM2" | "COM3" | "COM4" | "CLOCK$" => {
            Some(CharDevice::Nul)
        }
        _ => None,
    }
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
    current_drive: u8, // 0=A, ... 2=C, ... 25=Z
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

        let canonical = fs::canonicalize(&root_path).unwrap_or_else(|_| root_path.clone());

        // Create a dummy COMMAND.COM on Z:
        let mut z_files = MemFs::new();
        z_files.insert("COMMAND.COM", vec![0x90; 5000]);

        let mut drives: [Option<Drive>; 26] = std::array::from_fn(|_| None);
        drives[DRIVE_C as usize] = Some(Drive {
            kind: DriveKind::HardDisk,
            storage: Storage::Host(canonical),
            current_dir: String::new(),
            label: DEFAULT_LABEL.to_string(),
            read_only: false,
            mount: Some(MountSpec { drive: DRIVE_C, path: root_path.clone(), opts: MountOptions::default() }),
        });
        drives[DRIVE_Z as usize] = Some(Self::memory_drive(z_files, DEFAULT_LABEL));

        Self {
            open_files: HashMap::new(),
            drives,
            current_drive: DRIVE_C, // Default to C:
        }
    }

    fn memory_drive(files: MemFs, label: &str) -> Drive {
        Drive {
            kind: DriveKind::Virtual,
            storage: Storage::Tree { files, image: None },
            current_dir: String::new(),
            label: normalize_label(label),
            read_only: true,
            mount: None,
        }
    }

    // ========================================================================
    // MOUNTS
    // ========================================================================

    /// Host directory currently backing drive C:.
    pub fn root_path(&self) -> &Path {
        self.drives[DRIVE_C as usize]
            .as_ref()
            .and_then(Drive::host_root)
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
        let label = |default: &str| {
            opts.label
                .as_deref()
                .map(normalize_label)
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| normalize_label(default))
        };
        if path.is_file() {
            // A CD image.
            if drive == DRIVE_C {
                return Err("Drive C: must be a host directory".to_string());
            }
            if !matches!(opts.kind, DriveKind::CdRom | DriveKind::HardDisk) {
                return Err("Only CD-ROM images can be mounted".to_string());
            }
            let canonical = fs::canonicalize(path).map_err(|e| e.to_string())?;
            let image = CdImage::open(&canonical)?;
            // A disc of only audio tracks has no file system.
            let (files, volume_label) = match image.data_track() {
                Some(_) => {
                    let volume = crate::cdrom::iso9660::read_volume(&image)?;
                    (volume.files, volume.label)
                }
                None => (MemFs::new(), "AUDIO_CD".to_string()),
            };
            let volume_label = if volume_label.is_empty() { "CDROM".to_string() } else { volume_label };
            self.close_drive_files(drive);
            self.drives[drive as usize] = Some(Drive {
                kind: DriveKind::CdRom,
                storage: Storage::Tree { files, image: Some(Rc::new(image)) },
                current_dir: String::new(),
                label: label(&volume_label),
                read_only: true,
                mount: Some(MountSpec { drive, path: path.to_path_buf(), opts: opts.clone() }),
            });
            return Ok(canonical);
        }
        if !path.is_dir() {
            return Err(format!("{} is not a directory or a CD image", path.display()));
        }
        let canonical = fs::canonicalize(path).map_err(|e| e.to_string())?;

        self.close_drive_files(drive);
        self.drives[drive as usize] = Some(Drive {
            kind: opts.kind,
            storage: Storage::Host(canonical.clone()),
            current_dir: String::new(),
            label: label(DEFAULT_LABEL),
            read_only: opts.read_only || opts.kind == DriveKind::CdRom,
            mount: Some(MountSpec { drive, path: path.to_path_buf(), opts: opts.clone() }),
        });
        Ok(canonical)
    }

    /// Mount `files` as the read-only drive `drive`, which must not be
    /// mounted yet. C: and Z: are taken.
    pub fn mount_memory(&mut self, drive: u8, files: MemFs, label: &str) -> Result<(), String> {
        if drive >= LASTDRIVE || drive == DRIVE_C || drive == DRIVE_Z {
            return Err(format!("Drive {}: is reserved", drive_letter(drive.min(LASTDRIVE - 1))));
        }
        if self.is_mounted(drive) {
            return Err(format!("Drive {}: is already mounted", drive_letter(drive)));
        }
        self.drives[drive as usize] = Some(Self::memory_drive(files, label));
        Ok(())
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

    /// The CD image a drive shows.
    pub fn cd_image(&self, drive: u8) -> Option<Rc<CdImage>> {
        self.drive(drive)?.image().cloned()
    }

    pub fn drive_info(&self, drive: u8) -> Option<DriveInfo> {
        self.drive(drive).map(|d| DriveInfo {
            drive,
            kind: d.kind,
            root: d.host_root().map(Path::to_path_buf),
            image: d.image().map(|image| image.path().to_path_buf()),
            label: d.label.clone(),
            read_only: !d.writable(),
            current_dir: d.current_dir.to_ascii_uppercase(),
            mount: d.mount.clone(),
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

    /// The character device an open handle refers to, if any.
    pub fn handle_device(&self, handle: u16) -> Option<CharDevice> {
        match self.open_files.get(&handle)?.data {
            OpenData::Device(device) => Some(device),
            _ => None,
        }
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

    /// The path `rest` names on a drive held in memory, as `MemFs` keeps
    /// paths.
    fn memory_path(drive: &Drive, rest: &str) -> String {
        Self::logical_components(drive, rest).join("\\").to_ascii_uppercase()
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
        self.resolve_names_on(drive_num, rest).map(|(path, _)| path)
    }

    /// The host path of `rest` on a host drive, and its DOS path from the
    /// root in short names (e.g. "GAMES\DAYOFT~1").
    fn resolve_names_on(&self, drive_num: u8, rest: &str) -> Option<(PathBuf, String)> {
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
    fn locate_in_memory(&self, dos_path: &str) -> Option<(u8, &Drive, String)> {
        let normalized = dos_path.replace('/', "\\");
        let (drive_num, rest) = self.split_drive(&normalized)?;
        let drive = self.drive(drive_num).filter(|d| d.tree().is_some())?;
        let path = Self::memory_path(drive, rest);
        Some((drive_num, drive, path))
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
        self.is_virtual_file(dos_path) || self.resolve_path(dos_path).is_some_and(|p| p.is_file())
    }

    /// Whether a DOS path names an existing directory, on any drive.
    pub fn is_directory(&self, dos_path: &str) -> bool {
        match self.locate_in_memory(dos_path) {
            Some((_, drive, path)) => drive.tree().is_some_and(|files| files.is_dir(&path)),
            None => self.resolve_path(dos_path).is_some_and(|p| p.is_dir()),
        }
    }

    /// Where the contents of the file a DOS path names are, on any drive.
    pub fn file_data(&self, dos_path: &str) -> Option<FileData> {
        match self.locate_in_memory(dos_path) {
            Some((_, drive, path)) => FileData::of(drive.tree()?.file(&path)?, drive.image()),
            None => self.resolve_path(dos_path).filter(|p| p.is_file()).map(FileData::Host),
        }
    }

    /// The entries of a host directory with their DOS names (see
    /// `short_names`), in sorted order. Hidden (dot) files are left out.
    fn host_entries(dir: &Path) -> Vec<(String, String)> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .into_iter()
            .flatten()
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
        let entries = Self::host_entries(dir);
        entries
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(target))
            .or_else(|| entries.iter().find(|(_, short)| *short == target_upper))
            .cloned()
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
            Some((host_path, dos_dir)) if host_path.is_dir() => {
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
        self.open_or_create(filename, mode, owner, false)
    }

    /// Open `filename` with access `mode`; `create` makes a missing file,
    /// as the create calls do. Opening alone never creates one: programs
    /// probe for files by opening them read/write.
    fn open_or_create(&mut self, filename: &str, mode: u8, owner: u16, create: bool) -> Result<u16, u8> {
        // Devices open by name, whatever the directory; there's no file to
        // create.
        if let Some(device) = char_device(filename) {
            let handle = self.free_handle()?;
            self.open_files.insert(
                handle,
                OpenFile { data: OpenData::Device(device), drive: self.current_drive, owner },
            );
            return Ok(handle);
        }
        // Files held in memory are read-only: read/write opens are
        // downgraded as on a CD-ROM.
        if let Some((drive, d, path)) = self.locate_in_memory(filename) {
            let files = d.tree().ok_or(0x03u8)?;
            let data = match files.file(&path).and_then(|node| FileData::of(node, d.image())) {
                Some(FileData::Memory(data)) => OpenData::Memory(data, Rc::new(Cell::new(0))),
                Some(FileData::Image(image, extent)) => OpenData::Image(image, extent, Rc::new(Cell::new(0))),
                _ if files.is_dir(&path) => return Err(0x05),
                _ => {
                    let parent = path.rsplit_once('\\').map_or("", |(p, _)| p);
                    return Err(if files.is_dir(parent) { 0x02 } else { 0x03 });
                }
            };
            match mode & 0x03 {
                0 | 2 => {}
                1 => return Err(0x05),
                _ => return Err(0x0C),
            }
            let handle = self.free_handle()?;
            self.open_files.insert(handle, OpenFile { data, drive, owner });
            return Ok(handle);
        }

        let (drive, path) = self.locate(filename).ok_or(0x03)?; // Path not found
        // A directory (or a bare "D:") isn't a file: access denied.
        if path.is_dir() {
            return Err(0x05);
        }
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
                options.write(true).create(create).truncate(false);
            }
            2 => {
                // Read/write opens on read-only media (CD-ROM) are quietly
                // downgraded to read-only, as MSCDEX does; lots of CD games
                // open their data files R/W without ever writing.
                if writable {
                    options.read(true).write(true).create(create);
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
                        data: OpenData::Host(file),
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
        if char_device(filename).is_some() {
            return self.open_file(filename, 0x02, owner);
        }
        let normalized = filename.replace('/', "\\");
        let (drive, _) = self.split_drive(&normalized).ok_or(0x03)?;
        self.check_writable(drive)?;
        self.open_or_create(filename, 0x02, owner, true)
    }

    /// INT 21h, AH=5Bh: create a file that must not exist yet.
    pub fn create_new_file(&mut self, filename: &str, owner: u16) -> Result<u16, u8> {
        if char_device(filename).is_none() && self.resolve_path(filename).is_some_and(|p| p.exists()) {
            return Err(0x50); // File exists
        }
        self.create_file(filename, owner)
    }

    /// INT 21h, AH=5Ah: create a file with a unique name in `directory`
    /// (which ends in a backslash). Returns the handle and the name.
    pub fn create_temp_file(&mut self, directory: &str, owner: u16) -> Result<(u16, String), u8> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        for i in 0..1000u32 {
            let name = format!("{}{:08X}", directory, stamp.wrapping_add(i) & 0x0FFF_FFFF);
            if let Ok(handle) = self.create_new_file(&name, owner) {
                return Ok((handle, name));
            }
        }
        Err(0x05)
    }

    /// INT 21h, AH=41h: delete a file.
    pub fn delete_file(&self, filename: &str) -> Result<(), u8> {
        let (drive, path) = self.locate(filename).ok_or(0x03)?;
        if !path.is_file() {
            return Err(0x02); // File not found
        }
        self.check_writable(drive)?;
        fs::remove_file(path).map_err(|_| 0x05)
    }

    /// INT 21h, AH=56h: rename or move a file within a drive.
    pub fn rename_file(&self, from: &str, to: &str) -> Result<(), u8> {
        let (drive, source) = self.locate(from).ok_or(0x03)?;
        if !source.exists() {
            return Err(0x02);
        }
        self.check_writable(drive)?;
        let normalized = to.replace('/', "\\");
        let (to_drive, rest) = self.split_drive(&normalized).ok_or(0x03)?;
        if to_drive != drive {
            return Err(0x11); // Not same device
        }
        let (parent_dos, leaf) = match rest.rsplit_once('\\') {
            Some(("", l)) => ("\\", l),
            Some((p, l)) => (p, l),
            None => (".", rest),
        };
        let parent = self.resolve_on(to_drive, parent_dos).ok_or(0x03)?;
        if leaf.is_empty() || !parent.is_dir() {
            return Err(0x03);
        }
        if self.find_existing_child(&parent, leaf).is_some() {
            return Err(0x05); // Destination exists
        }
        fs::rename(source, parent.join(leaf.to_uppercase())).map_err(|_| 0x05)
    }

    /// INT 21h, AH=45h/46h: a second handle for the file behind `handle`,
    /// sharing its position. `new_handle` picks the number (AH=46h, which
    /// closes a file already open there).
    pub fn duplicate_handle(&mut self, handle: u16, new_handle: Option<u16>) -> Result<u16, u8> {
        let open = self.open_files.get(&handle).ok_or(0x06)?;
        let data = match &open.data {
            OpenData::Host(f) => OpenData::Host(f.try_clone().map_err(|_| 0x04)?),
            OpenData::Memory(data, pos) => OpenData::Memory(data.clone(), pos.clone()),
            OpenData::Image(image, extent, pos) => OpenData::Image(image.clone(), *extent, pos.clone()),
            OpenData::Device(device) => OpenData::Device(*device),
        };
        let copy = OpenFile {
            data,
            drive: open.drive,
            owner: open.owner,
        };
        let target = match new_handle {
            Some(h) if h < HANDLE_LIMIT => h,
            Some(_) => return Err(0x06),
            None => self.free_handle()?,
        };
        self.open_files.insert(target, copy);
        Ok(target)
    }

    /// INT 21h, AX=5700h: the DOS time and date of a file's last change.
    pub fn file_time(&self, handle: u16) -> Result<(u16, u16), u8> {
        let open = self.open_files.get(&handle).ok_or(0x06)?;
        let modified = match &open.data {
            OpenData::Host(f) => f.metadata().ok().and_then(|m| m.modified().ok()),
            OpenData::Memory(..) => return Ok((MEMORY_TIME, MEMORY_DATE)),
            OpenData::Image(_, extent, _) => return Ok((extent.time, extent.date)),
            OpenData::Device(_) => None,
        };
        let t: DateTime<Local> = modified.map_or_else(Local::now, DateTime::from);
        let time = (t.hour() << 11 | t.minute() << 5 | t.second() / 2) as u16;
        let year = (t.year().max(1980) - 1980) as u32;
        let date = (year << 9 | t.month() << 5 | t.day()) as u16;
        Ok((time, date))
    }

    /// True if `handle` is open.
    pub fn is_open(&self, handle: u16) -> bool {
        self.open_files.contains_key(&handle)
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
            let file = match &mut open.data {
                OpenData::Host(file) => file,
                OpenData::Memory(data, pos) => {
                    let start = (pos.get() as usize).min(data.len());
                    let end = start.saturating_add(count).min(data.len());
                    pos.set(pos.get() + (end - start) as u64);
                    return Ok(data[start..end].to_vec());
                }
                OpenData::Image(image, extent, pos) => {
                    let mut buffer = vec![0u8; count];
                    let n = image.read_extent(extent, pos.get(), &mut buffer).map_err(|_| 0x1Eu16)?;
                    buffer.truncate(n);
                    pos.set(pos.get() + n as u64);
                    return Ok(buffer);
                }
                OpenData::Device(_) => return Ok(Vec::new()),
            };
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
            let file = match &mut open.data {
                OpenData::Host(file) => file,
                OpenData::Memory(..) | OpenData::Image(..) => return Err(0x05),
                OpenData::Device(_) => return Ok(data.len() as u16),
            };
            match file.write(data) {
                Ok(bytes_written) => Ok(bytes_written as u16),
                Err(_) => Err(0x05),
            }
        } else {
            Err(0x06)
        }
    }

    /// Seek in a file of `len` bytes that isn't a host file: past the end
    /// is fine, before the start is not.
    fn seek_in(len: u64, pos: &Cell<u64>, offset: i64, origin: u8) -> Result<u64, u16> {
        let base = match origin {
            0 => 0,
            1 => pos.get() as i64,
            2 => len as i64,
            _ => return Err(0x01),
        };
        let at = base.checked_add(offset).filter(|&at| at >= 0).ok_or(0x19u16)? as u64;
        pos.set(at);
        Ok(at)
    }

    // INT 21h, AH=42h: Seek
    pub fn seek_file(&mut self, handle: u16, offset: i64, origin: u8) -> Result<u64, u16> {
        if let Some(open) = self.open_files.get_mut(&handle) {
            let file = match &mut open.data {
                OpenData::Host(file) => file,
                OpenData::Memory(data, pos) => return Self::seek_in(data.len() as u64, pos, offset, origin),
                OpenData::Image(_, extent, pos) => return Self::seek_in(extent.size as u64, pos, offset, origin),
                OpenData::Device(_) => return Ok(0),
            };
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
                let used = d.host_root().map_or(0, |root| Self::used_clusters(root, cluster_bytes, total as u64));
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
        if let Some((_, drive, path)) = self.locate_in_memory(filename) {
            let files = drive.tree().ok_or(0x03u8)?;
            return match files.file(&path) {
                Some(node) => Ok(Self::tree_attr(Some(node)) as u16),
                None if files.is_dir(&path) => Ok(0x11),
                None => Err(0x02),
            };
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
        let normalized = path.replace('/', "\\");
        let (drive_num, rest) = self.split_drive(&normalized).ok_or(0x03)?;
        let (host_path, dos_form) = self.resolve_names_on(drive_num, rest).ok_or(0x03)?;
        if !host_path.is_dir() {
            return Err(0x03);
        }
        self.check_writable(drive_num)?;
        // DOS error 0x10 = "attempt to remove current directory".
        if self.drive(drive_num).is_some_and(|d| dos_form.eq_ignore_ascii_case(&d.current_dir)) {
            return Err(0x10);
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

    /// The attributes of an entry of a tree held in memory (None for a
    /// directory): read-only, and hidden where the CD says so.
    fn tree_attr(node: Option<&Node>) -> u8 {
        match node {
            None => 0x11,
            Some(Node::Extent(extent)) if extent.hidden => 0x23,
            Some(_) => 0x21,
        }
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

    /// The ".." and "." entries of a directory below the root that match
    /// `pattern`.
    fn dot_entries(pattern: &str) -> impl Iterator<Item = DosDirEntry> + '_ {
        ["..", "."]
            .into_iter()
            .filter(|dot| Self::matches_pattern(dot, pattern))
            .map(|dot| DosDirEntry {
                filename: dot.to_string(),
                size: 0,
                is_dir: true,
                is_readonly: false,
                dos_time: 0,
                dos_date: 0,
                attr: 0x10,
            })
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

        // Search attribute bits an entry needs to be listed.
        let restricted_bits = 0x02 | 0x04 | 0x10;
        let mut valid_entries: Vec<DosDirEntry> = Vec::new();

        if let Some(files) = drive.tree() {
            let dir = Self::memory_path(drive, search_dir_str);
            if !files.is_dir(&dir) {
                return Err(0x03);
            }
            if !dir.is_empty() {
                valid_entries.extend(Self::dot_entries(pattern));
            }
            for (name, node) in files.list(&dir) {
                let attr = Self::tree_attr(node);
                if (attr as u16 & restricted_bits) & !search_attr != 0 || !Self::matches_pattern(name, pattern) {
                    continue;
                }
                let (dos_time, dos_date) = match node {
                    Some(Node::Extent(extent)) => (extent.time, extent.date),
                    _ => (MEMORY_TIME, MEMORY_DATE),
                };
                valid_entries.push(DosDirEntry {
                    filename: name.to_string(),
                    size: node.map_or(0, |n| n.len() as u32),
                    is_dir: node.is_none(),
                    is_readonly: true,
                    dos_time,
                    dos_date,
                    attr,
                });
            }
            return Ok(valid_entries);
        }

        // Host Filesystem Listing
        let host_dir = self.resolve_on(drive_num, search_dir_str).ok_or(0x03)?;

        if !host_dir.is_dir() {
            return Err(0x03);
        }
        let is_host_root = drive.host_root() == Some(host_dir.as_path());
        let media_ro = !drive.writable();

        if !is_host_root {
            valid_entries.extend(Self::dot_entries(pattern));
        }

        for (original_name, final_name) in Self::host_entries(&host_dir) {
            let Ok(metadata) = fs::metadata(host_dir.join(&original_name)) else {
                continue;
            };

            let is_dir = metadata.is_dir();
            let is_readonly = media_ro || metadata.permissions().readonly();
            let mut file_attr: u8 = if is_dir { 0x10 } else { 0x20 };
            if is_readonly {
                file_attr |= 0x01;
            }

            if (file_attr as u16 & restricted_bits) & !search_attr != 0 {
                continue;
            }

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
    fn long_names_get_dosbox_short_names() {
        let names = [
            "Day Of The Tentacle.BIN",
            "Day Of The Tentacle.cue",
            "readme.txt",
            "Cargo.toml",
            "two.dots.txt",
            "ABCDEF~1.TXT",
            "abcdefghij.txt",
        ];
        assert_eq!(
            short_names(&names),
            [
                "DAYOFT~1.BIN",
                "DAYOFT~2.CUE",
                "README.TXT",
                "CARGO~1.TOM",
                "TWODOT~1.TXT",
                "ABCDEF~1.TXT",
                "ABCDEF~2.TXT",
            ]
        );
    }

    #[test]
    fn host_files_are_found_by_long_and_short_name() {
        let base = scratch("short_names");
        fs::create_dir_all(base.join("CD")).unwrap();
        fs::write(base.join("CD/Day Of The Tentacle.BIN"), b"bin").unwrap();
        fs::write(base.join("CD/Day Of The Tentacle.cue"), b"cue").unwrap();
        let disk = DiskController::new(base.clone());

        let cue = disk.resolve_path(r"C:\CD\DAYOFT~2.CUE").unwrap();
        assert!(cue.ends_with("CD/Day Of The Tentacle.cue"));
        let bin = disk.resolve_path(r"\cd\day of the tentacle.bin").unwrap();
        assert!(bin.ends_with("CD/Day Of The Tentacle.BIN"));
        let names: Vec<String> = disk
            .list_directory(r"C:\CD\*.*", 0)
            .unwrap()
            .into_iter()
            .map(|e| e.filename)
            .collect();
        assert_eq!(names, ["..", ".", "DAYOFT~1.BIN", "DAYOFT~2.CUE"]);
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
    fn opening_a_missing_file_does_not_create_it() {
        let base = scratch("open_missing");
        fs::create_dir_all(base.join("c")).unwrap();
        let mut disk = DiskController::new(base.join("c"));
        for mode in [0, 1, 2] {
            assert_eq!(disk.open_file("C:\\PROBE.PAT", mode, PSP), Err(0x02));
        }
        assert!(!base.join("c/PROBE.PAT").exists());
        assert!(disk.create_file("C:\\PROBE.PAT", PSP).is_ok());
        assert!(base.join("c/PROBE.PAT").exists());
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
    fn drives_held_in_memory() {
        let base = scratch("memory");
        let mut disk = DiskController::new(base);
        let mut files = MemFs::new();
        files.insert("GUS\\MIDI\\PIANO.PAT", &b"0123456789"[..]);
        files.insert("GUS\\README.TXT", &b"hi"[..]);
        disk.mount_memory(23, files.clone(), "patches").unwrap();
        assert!(disk.mount_memory(23, files.clone(), "again").is_err());
        assert!(disk.mount_memory(DRIVE_C, files.clone(), "c").is_err());
        assert!(disk.mount_memory(DRIVE_Z, files, "z").is_err());
        let info = disk.drive_info(23).unwrap();
        assert_eq!((info.kind, info.root, info.label.as_str(), info.read_only), (DriveKind::Virtual, None, "PATCHES", true));

        // Directories, with their dot entries and only when asked for.
        let names = |disk: &DiskController, spec: &str, attr: u16| -> Vec<(String, u8)> {
            disk.list_directory(spec, attr).unwrap().into_iter().map(|e| (e.filename, e.attr)).collect()
        };
        assert_eq!(names(&disk, "X:\\*.*", 0x10), [("GUS".to_string(), 0x11)]);
        assert!(names(&disk, "X:\\*.*", 0).is_empty());
        assert_eq!(
            names(&disk, "X:\\GUS\\*.*", 0x10),
            [("..".to_string(), 0x10), (".".to_string(), 0x10), ("MIDI".to_string(), 0x11), ("README.TXT".to_string(), 0x21)]
        );
        assert_eq!(names(&disk, "X:\\GUS\\MIDI\\*.PAT", 0), [("PIANO.PAT".to_string(), 0x21)]);
        assert_eq!(disk.list_directory("X:\\NOPE\\*.*", 0).err(), Some(0x03));
        assert_eq!(disk.get_file_attribute("X:\\GUS"), Ok(0x11));
        assert_eq!(disk.get_file_attribute("X:\\GUS\\README.TXT"), Ok(0x21));
        assert_eq!(disk.get_file_attribute("X:\\GUS\\NOPE.TXT"), Err(0x02));
        assert!(disk.is_directory("X:\\GUS\\MIDI") && !disk.is_directory("X:\\GUS\\README.TXT"));
        assert!(disk.is_file("x:/gus/readme.txt") && !disk.is_file("X:\\GUS"));

        // Relative to the drive's current directory.
        assert!(disk.set_current_directory("X:\\GUS\\MIDI"));
        assert!(!disk.set_current_directory("X:\\GUS\\README.TXT"));
        assert_eq!(disk.get_current_directory_of(23).as_deref(), Some("GUS\\MIDI"));
        assert_eq!(disk.qualify_path("X:PIANO.PAT").as_deref(), Some("X:\\GUS\\MIDI\\PIANO.PAT"));
        assert!(matches!(disk.file_data("X:..\\README.TXT"), Some(FileData::Memory(d)) if &d[..] == b"hi"));

        // Read-only files, read with a position duplicated handles share.
        let h = disk.open_file("X:PIANO.PAT", 2, PSP).unwrap();
        assert_eq!(disk.handle_drive(h), Some(23));
        assert_eq!(disk.read_file(h, 4).unwrap(), b"0123");
        let dup = disk.duplicate_handle(h, None).unwrap();
        assert_eq!(disk.read_file(dup, 2).unwrap(), b"45");
        assert_eq!(disk.read_file(h, 100).unwrap(), b"6789");
        assert_eq!(disk.read_file(h, 1).unwrap(), b"");
        assert_eq!(disk.seek_file(h, -3, 2), Ok(7));
        assert_eq!(disk.read_file(dup, 5).unwrap(), b"789");
        assert_eq!(disk.seek_file(h, 20, 0), Ok(20));
        assert_eq!(disk.read_file(h, 1).unwrap(), b"");
        assert_eq!(disk.seek_file(h, -21, 1), Err(0x19));
        assert_eq!(disk.write_file(h, b"x"), Err(0x05));
        assert_eq!(disk.file_time(h), Ok((MEMORY_TIME, MEMORY_DATE)));
        assert_eq!(disk.open_file("X:PIANO.PAT", 1, PSP), Err(0x05));
        assert_eq!(disk.open_file("X:NOPE.PAT", 0, PSP), Err(0x02));
        assert_eq!(disk.open_file("X:\\NOPE\\PIANO.PAT", 0, PSP), Err(0x03));
        assert_eq!(disk.open_file("X:\\GUS", 0, PSP), Err(0x05));
        assert_eq!(disk.create_file("X:NEW.PAT", PSP), Err(0x05));
        assert_eq!(disk.create_directory("X:\\NEW"), Err(0x05));

        disk.unmount(23).unwrap();
        assert!(disk.read_file(h, 1).is_err());
        assert!(!disk.is_file("X:\\GUS\\README.TXT"));
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
