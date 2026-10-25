use chrono::{DateTime, Datelike, Local, Timelike};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::cdrom::image::CdImage;
use crate::cdrom::Extent;
use crate::diskimage::{self, Chs, DiskImage, ImageKind, MemoryImage};
use crate::fat::{self, EntryRef, FatVolume};
use crate::hostfs::{self, File, OpenOptions};
use crate::memfs::{Bytes, MemFs, Node};
use crate::mount::MountSpec;

mod drives;
mod files;
mod host_dir;
mod layout;
mod mount;
mod paths;
mod search;
mod state;

use host_dir::{HostDirCache, SearchKey, Usage};
pub use layout::{DriveKind, Fat32Layout, FatLayout, old_space};

/// The open files are numbered as DOS numbers the entries of its System
/// File Table (see dos_files.rs), which the handles of each process refer
/// to: the first three are the standard devices, AUX, CON and PRN, which
/// are always open; the files opened come after them.
pub const SFT_AUX: u16 = 0;
pub const SFT_CON: u16 = 1;
pub const SFT_PRN: u16 = 2;
pub const FIRST_FILE: u16 = 3;
/// The number of entries in the System File Table (FILES=), as DOSBox
/// has them. At most 128, for `DiskController::sft_dirty`.
pub const FILES: u16 = 127;

// Drive numbers are 0-based (0=A:, 2=C:, 25=Z:).
pub const DRIVE_C: u8 = 2;
pub const DRIVE_Z: u8 = 25;
/// A: and B: are the BIOS floppy units: whatever is mounted there is a
/// floppy drive.
pub const FLOPPY_DRIVES: u8 = 2;
/// Number of drive letters reported to programs (LASTDRIVE=Z).
pub const LASTDRIVE: u8 = 26;
/// Disks mounted by number instead of letter (`MOUNT 2 disk.img`, as in
/// DOSBox): images the BIOS has as units without a DOS drive, 0 and 1 the
/// floppy units 00h and 01h, 2 and 3 the hard disks 80h and 81h. The drive
/// table has them after Z:.
pub const NUMBERED_DRIVES: u8 = 4;
/// The drive table's size: the lettered drives, then the numbered ones.
pub const DRIVE_SLOTS: u8 = LASTDRIVE + NUMBERED_DRIVES;

/// Volume label used when a mount doesn't specify one.
pub const DEFAULT_LABEL: &str = "RUSTDOS";

/// Usable data clusters on a 1.44 MB floppy with 512-byte clusters.
const FLOPPY_CLUSTERS: u16 = 2847;

/// DOS time and date of the files held in memory: 1 January 2020.
const MEMORY_TIME: u16 = 0x0000;
const MEMORY_DATE: u16 = 0x5021;

/// A list of owned path components as the FAT driver takes them.
fn refs(parts: &[String]) -> Vec<&str> {
    parts.iter().map(String::as_str).collect()
}

/// A path on a disk image the way DOS shows it: its components as they are
/// in the directory entries, "GAMES\DOOM".
fn canonical_path(parts: &[&str]) -> String {
    parts.iter().map(|p| fat::canonical_name(p).unwrap_or_else(|| p.to_ascii_uppercase())).collect::<Vec<_>>().join("\\")
}

pub fn drive_letter(drive: u8) -> char {
    (b'A' + drive) as char
}

/// The drive table's place of the disk mounted as `number` (0 to 3).
pub const fn numbered_drive(number: u8) -> u8 {
    LASTDRIVE + number
}

/// The BIOS unit of the disk mounted as `number`: 00h and 01h for the
/// floppies 0 and 1, 80h and 81h for the hard disks 2 and 3.
pub const fn numbered_unit(number: u8) -> u8 {
    if number < FLOPPY_DRIVES { number } else { 0x80 + number - FLOPPY_DRIVES }
}

/// The number of a disk mounted by number, None for a lettered drive.
pub fn drive_number(drive: u8) -> Option<u8> {
    (LASTDRIVE..DRIVE_SLOTS).contains(&drive).then(|| drive - LASTDRIVE)
}

/// A drive as MOUNT and `[drives]` name it: "C", or "2" for a disk
/// mounted by number.
/// The hard disk image the archive `path` holds, to mount instead of its
/// files (`archive_image`): a .vhd, or an image bigger than a floppy and
/// not a CD's. None for an archive of files.
pub fn archive_hard_disk(path: &Path) -> Option<String> {
    use crate::overlay::Lower;
    if !hostfs::is_file(path) || !crate::archive::is_archive_name(path) {
        return None;
    }
    let stack = crate::archive::open_variant(path, None).ok()?;
    let image = archive_image(&stack.files())?;
    let ext = image.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase()).unwrap_or_default();
    let disk = match ext.as_str() {
        "vhd" => true,
        "img" | "ima" | "dsk" => stack.metadata(&image).is_ok_and(|m| m.len > 2_949_120),
        _ => false,
    };
    disk.then_some(image)
}

/// The disk or CD image an archive holds, of its files (paths from its
/// root), to mount instead of the files: its one CUE sheet, or its one
/// image. None if there are programs beside it.
fn archive_image(files: &[String]) -> Option<String> {
    let extension = |f: &str| f.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase()).unwrap_or_default();
    if files.iter().any(|f| matches!(extension(f).as_str(), "exe" | "com" | "bat")) {
        return None;
    }
    let one = |wanted: &dyn Fn(&str) -> bool| -> Option<String> {
        let found: Vec<&String> = files.iter().filter(|f| wanted(f)).collect();
        match found.as_slice() {
            [one] => Some((*one).clone()),
            _ => None,
        }
    };
    one(&|f| matches!(extension(f).as_str(), "cue" | "ins" | "inst" | "bdn"))
        .or_else(|| one(&|f| crate::mount::is_image_name(Path::new(f))))
}

pub fn drive_key(drive: u8) -> String {
    match drive_number(drive) {
        Some(number) => number.to_string(),
        None => drive_letter(drive).to_string(),
    }
}

/// A drive as messages name it: "C:", or "2" for a disk mounted by number.
pub fn drive_name(drive: u8) -> String {
    match drive_number(drive) {
        Some(number) => number.to_string(),
        None => format!("{}:", drive_letter(drive)),
    }
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
pub fn short_names<S: AsRef<str>>(names: &[S]) -> Vec<String> {
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

/// How a host directory or disk image is presented to DOS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountOptions {
    pub kind: DriveKind,
    pub label: Option<String>,
    pub read_only: bool,
    /// The images after the first of a drive mounted from a list of them,
    /// which Ctrl+F4 steps through.
    pub more_images: Vec<PathBuf>,
    /// A hard disk image's geometry, where it can't be found from the
    /// image.
    pub geometry: Option<Chs>,
    /// Where a booted system finds the disk or CD-ROM drive on the IDE
    /// channels (`-ide`); None where it goes by default.
    pub ide: Option<crate::ide::IdeSlot>,
    /// The disk image boots when Rust-DOS starts (`-boot`). One drive at a
    /// time has it (`DiskController::set_boot_drive`).
    pub boot: bool,
    /// A host directory on D: to Y: is a hard disk of a booted system
    /// (`shared_disk`) unless this says no (`-noshare`); C: only with yes
    /// (`-share`). None goes by the letter.
    pub share: Option<bool>,
    /// The folder a host directory's changes go to, which leave the
    /// directory as it is (`-overlay`, `overlay`); a disk image's go to a
    /// delta file in it (`diskdelta`).
    pub overlay: Option<PathBuf>,
    /// The launch configuration of an archive's .dosc (its `[variant]`
    /// folder) over the archive (`-variant`).
    pub variant: Option<String>,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self {
            kind: DriveKind::HardDisk,
            label: None,
            read_only: false,
            more_images: Vec::new(),
            geometry: None,
            ide: None,
            boot: false,
            share: None,
            overlay: None,
            variant: None,
        }
    }
}

/// Public snapshot of a mounted drive.
#[derive(Clone, Debug)]
pub struct DriveInfo {
    pub drive: u8,
    pub kind: DriveKind,
    /// Host directory; `None` for the drives held in memory and images.
    /// Under an overlay, the directory below it.
    pub root: Option<PathBuf>,
    /// The folder a host directory's changes go to, under an overlay, or
    /// the delta file a disk image's do.
    pub overlay: Option<PathBuf>,
    /// The disk or CD image the drive shows.
    pub image: Option<PathBuf>,
    /// All the images of a drive mounted from a list of them, and which one
    /// it shows.
    pub images: Vec<PathBuf>,
    pub image_index: usize,
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

    /// "C:", or "2" for a disk mounted by number.
    pub fn name(&self) -> String {
        drive_name(self.drive)
    }
}

/// What holds a drive's files.
enum Storage {
    /// A host directory, acting as the drive's root: the directory itself,
    /// or the root of the drive's overlay (`layerN:/`).
    Host(PathBuf),
    /// A tree held in memory, whose files are either in memory too or on
    /// the CD image.
    Tree { files: MemFs, image: Option<Rc<CdImage>> },
    /// The FAT file system of a floppy or hard disk image.
    Fat(Rc<FatVolume>),
    /// A disk mounted by number: its sectors for the BIOS, and no files.
    Raw(Rc<DiskImage>),
}

/// A drive's write overlay (`overlay::Overlay`), as long as the drive has
/// it: over a host directory, or an archive.
struct Overlaid {
    layer: hostfs::Layer,
    /// The directory or archive below, and where the changes go.
    lower: PathBuf,
    upper: Option<PathBuf>,
}

impl Overlaid {
    /// A path under the overlay's root as people know it: under the
    /// directory or archive below.
    fn shown(&self, path: &Path) -> Option<PathBuf> {
        path.starts_with(self.layer.root()).then(|| self.lower.join(hostfs::layer_path(path)))
    }
}

struct Drive {
    kind: DriveKind,
    storage: Storage,
    current_dir: String, // DOS directory relative to root (e.g., "GAMES\DOOM")
    label: String,
    read_only: bool,
    /// The mount as it was asked for.
    mount: Option<MountSpec>,
    /// The images of a drive mounted from images, and which one is in.
    images: Vec<PathBuf>,
    image: usize,
    /// Another disk went in since INT 13h last looked (AH=16h).
    media_changed: bool,
    /// For a host folder mounted as a CD, the disc made from it that a
    /// booted system's CD-ROM drive reads (`prepare_boot_cds`).
    boot_cd: Option<Rc<CdImage>>,
    /// For a host directory shared with a booted system, the disk made of
    /// it (`prepare_shared_disks`).
    shared: Option<crate::shared_disk::SharedDisk>,
    /// The overlay the drive's files are in: a host directory's with
    /// `-overlay`, or an archive's.
    overlay: Option<Overlaid>,
}

impl Drive {
    fn writable(&self) -> bool {
        !self.read_only
    }

    /// The host directory behind the drive, if there is one.
    fn host_root(&self) -> Option<&Path> {
        match &self.storage {
            Storage::Host(root) => Some(root),
            _ => None,
        }
    }

    /// The host directory as people know it: under an overlay, the one
    /// below.
    fn shown_root(&self) -> Option<&Path> {
        match &self.overlay {
            Some(overlaid) if self.host_root().is_some() => Some(&overlaid.lower),
            _ => self.host_root(),
        }
    }

    /// A path under the drive's overlay as people know it, in the
    /// directory or archive below; others as they are.
    fn shown(&self, path: PathBuf) -> PathBuf {
        self.overlay.as_ref().and_then(|o| o.shown(&path)).unwrap_or(path)
    }

    /// The drive's files, if they are held in memory.
    fn tree(&self) -> Option<&MemFs> {
        match &self.storage {
            Storage::Tree { files, .. } => Some(files),
            _ => None,
        }
    }

    fn image(&self) -> Option<&Rc<CdImage>> {
        match &self.storage {
            Storage::Tree { image, .. } => image.as_ref(),
            _ => None,
        }
    }

    /// The file system of a drive mounted from a disk image.
    fn fat(&self) -> Option<&Rc<FatVolume>> {
        match &self.storage {
            Storage::Fat(volume) => Some(volume),
            _ => None,
        }
    }

    /// The floppy or hard disk image in the drive, as the BIOS reads it:
    /// the one mounted, or the disk made of a shared host directory.
    fn disk(&self) -> Option<&Rc<DiskImage>> {
        self.mounted_disk().or(self.shared.as_ref().map(|s| &s.disk))
    }

    /// The floppy or hard disk image mounted in the drive.
    fn mounted_disk(&self) -> Option<&Rc<DiskImage>> {
        match &self.storage {
            Storage::Fat(volume) => Some(volume.disk()),
            Storage::Raw(disk) => Some(disk),
            _ => None,
        }
    }
}

/// An open file as the System File Table has it (`DiskController::sft_entry`).
pub struct SftEntry {
    pub refs: u16,
    /// The access mode it was opened with, and 8000h for an FCB's.
    pub mode: u16,
    pub device: Option<CharDevice>,
    pub drive: u8,
    /// The PSP of the process that opened it.
    pub owner: u16,
    /// Its name as in a directory entry: 8 characters and 3 of extension.
    pub name: [u8; 11],
    pub size: u32,
    pub time: u16,
    pub date: u16,
    /// A device in raw mode (bit 5 of IOCTL 4400h's information word).
    pub raw: bool,
}

struct OpenFile {
    data: OpenData,
    drive: u8,
    /// PSP of the process that opened the file. DOS closes a process's files
    /// when it terminates.
    owner: u16,
    /// Tells the file apart from others (`DiskController::file_key`).
    key: u64,
    /// The file as it was opened, for a save state to open it again: its
    /// full DOS path and access mode.
    path: String,
    mode: u8,
    /// The handles and FCBs that refer to it: it is closed when the last
    /// of them is.
    refs: u16,
    /// Opened for an FCB rather than a handle.
    fcb: bool,
    /// Made by a create call over a file that had bytes: they go
    /// when the program first uses the handle (`DiskController::settle`).
    empty_on_use: bool,
    /// A device a program put in raw (binary) mode with IOCTL 4401h:
    /// its output goes out as it is, a tab without being expanded.
    raw: bool,
}

/// Empty a file a create call opened, which nothing has used yet.
fn empty_created(open: &OpenFile) {
    match &open.data {
        OpenData::Host(file, _) => {
            let _ = file.set_len(0);
        }
        OpenData::Fat { volume, at, write: true, .. } => {
            let _ = volume.truncate(*at, 0);
        }
        _ => {}
    }
}

/// What an open handle reads and writes.
enum OpenData {
    /// A host file, and its host path when it was opened for writing:
    /// writes drop the searches of its folder (`HostDirCache::touched`).
    Host(File, Option<PathBuf>),
    /// A file held in memory, and the position, which the handles
    /// duplicated from this one share.
    Memory(Bytes, Rc<Cell<u64>>),
    /// A file on a CD image, and the shared position.
    Image(Rc<CdImage>, Extent, Rc<Cell<u64>>),
    /// A file on a disk image: where its directory entry is, the shared
    /// position, and whether it was opened for writing.
    Fat { volume: Rc<FatVolume>, at: EntryRef, pos: Rc<Cell<u64>>, write: bool },
    /// A character device opened by name (NUL, CON, PRN...).
    Device(CharDevice),
}

/// Where the contents of a file are.
#[derive(Clone, Debug)]
pub enum FileData {
    Host(PathBuf),
    Memory(Bytes),
    Image(Rc<CdImage>, Extent),
    Fat(Rc<FatVolume>, fat::Entry),
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
            FileData::Host(path) => hostfs::read(path).map(Bytes::Owned),
            FileData::Memory(data) => Ok(data.clone()),
            FileData::Image(image, extent) => {
                let mut data = vec![0u8; extent.size as usize];
                image.read_extent(extent, 0, &mut data)?;
                Ok(Bytes::Owned(data))
            }
            FileData::Fat(volume, entry) => {
                let mut data = vec![0u8; entry.size as usize];
                let n = volume.read(entry, 0, &mut data).map_err(|_| std::io::ErrorKind::InvalidData)?;
                data.truncate(n);
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CharDevice {
    /// NUL, and the printer and serial ports, which nothing is attached
    /// to: reads find nothing, writes vanish.
    #[default]
    Nul,
    /// CON: writes go to the screen.
    Con,
    /// EMMXXXX0, the expanded memory manager, which programs open to see
    /// whether there is EMS.
    Emm,
    /// A serial port: writes go out through it, reads take what came.
    /// Without the port, it is NUL.
    Com1,
    Com2,
    Com3,
    Com4,
    /// PRN and LPT1: writes go to the printer. Without one, it is NUL.
    Prn,
}

impl CharDevice {
    /// The serial port's index (0 for COM1).
    pub fn com_port(self) -> Option<usize> {
        match self {
            CharDevice::Com1 => Some(0),
            CharDevice::Com2 => Some(1),
            CharDevice::Com3 => Some(2),
            CharDevice::Com4 => Some(3),
            _ => None,
        }
    }
}

/// The character device a file name names. DOS finds devices by name in
/// any directory, with any extension: "C:\GAME\NUL.TXT" is NUL.
pub fn char_device(filename: &str) -> Option<CharDevice> {
    let last = filename.rsplit(['\\', '/', ':']).next()?;
    let stem = last.split('.').next()?.trim().to_ascii_uppercase();
    match stem.as_str() {
        "CON" => Some(CharDevice::Con),
        "COM1" => Some(CharDevice::Com1),
        "COM2" => Some(CharDevice::Com2),
        "COM3" => Some(CharDevice::Com3),
        "COM4" => Some(CharDevice::Com4),
        "PRN" | "LPT1" => Some(CharDevice::Prn),
        "NUL" | "AUX" | "LPT2" | "LPT3" | "CLOCK$" => Some(CharDevice::Nul),
        _ => None,
    }
}

/// Helper struct to transfer directory search results back to the CPU
#[allow(dead_code)]
#[derive(Clone)]
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
    /// The open files by their System File Table entry.
    open_files: HashMap<u16, OpenFile>,
    /// The entries whose copy in DOS memory is out of date: all of it, or
    /// only the position (`dos_files::flush`).
    sft_dirty: u128,
    position_dirty: u128,

    // File System State
    /// The lettered drives, then the disks mounted by number.
    drives: [Option<Drive>; DRIVE_SLOTS as usize],
    current_drive: u8, // 0=A, ... 2=C, ... 25=Z
    /// Whether the expanded memory manager's EMMXXXX0 device is there.
    pub emm_device: bool,
    /// The disks a state just loaded goes back to the checkpoints of, and
    /// those checkpoints, once all of the state is in (`revert_disks`).
    pub(crate) reverts: Vec<(u8, u64)>,
    /// The state file being loaded, whose copies of disks the drives are
    /// offered once they are mounted as it has them (savestate/disks.rs).
    pub(crate) copies_from: Option<PathBuf>,
    /// The folder drives the state being loaded takes as mounted from other
    /// host folders, by drive number (a debug server load's `folders`).
    pub folders_from_state: Vec<(u8, PathBuf)>,
    /// The disks made of shared host directories that a state being loaded
    /// has, with their manifests, for once the drives are mounted as it
    /// has them (`restore_shared`).
    pub(crate) shared_from_state: Option<Vec<(u8, String)>>,
    /// What mounting found to tell the user (a .dosc's patch that can't
    /// be used), for the bus's log (`Bus::mount_drive`).
    pub notes: Vec<String>,
    /// The record of the drives' media a state's header has, kept with
    /// the mounts it was made of (savestate/media.rs).
    pub(crate) media: Cell<Option<crate::savestate::media::Kept>>,
    /// The entries of host folders as last read (`host_entries`).
    host_dirs: HostDirCache,
}

impl DiskController {
    /// Creates the controller with C: backed by `root_path` and the virtual
    /// Z: drive. C: and Z: are always present; everything else is mounted.
    pub fn new(root_path: PathBuf) -> Self {
        // Ensure root path exists
        if !hostfs::exists(&root_path) {
            println!(
                "[DISK] Warning: Root path {:?} does not exist. Creating it.",
                root_path
            );
            let _ = hostfs::create_dir_all(&root_path);
        }

        let canonical = hostfs::canonicalize(&root_path).unwrap_or_else(|_| root_path.clone());

        // COMMAND.COM on Z:, which programs run to shell out.
        let mut z_files = MemFs::new();
        z_files.insert("COMMAND.COM", crate::command_com::stub_code());

        let mut drives: [Option<Drive>; DRIVE_SLOTS as usize] = std::array::from_fn(|_| None);
        drives[DRIVE_C as usize] = Some(Drive {
            kind: DriveKind::HardDisk,
            storage: Storage::Host(canonical),
            current_dir: String::new(),
            label: DEFAULT_LABEL.to_string(),
            read_only: false,
            mount: Some(MountSpec { drive: DRIVE_C, path: root_path.clone(), opts: MountOptions::default() }),
            images: Vec::new(),
            image: 0,
            media_changed: false,
            boot_cd: None,
            shared: None,
            overlay: None,
        });
        drives[DRIVE_Z as usize] = Some(Self::memory_drive(z_files, DEFAULT_LABEL));

        let mut disk = Self {
            open_files: HashMap::new(),
            sft_dirty: 0,
            position_dirty: 0,
            drives,
            current_drive: DRIVE_C, // Default to C:
            emm_device: false,
            reverts: Vec::new(),
            copies_from: None,
            folders_from_state: Vec::new(),
            shared_from_state: None,
            notes: Vec::new(),
            media: Cell::new(None),
            host_dirs: HostDirCache::default(),
        };
        disk.open_standard_devices();
        disk
    }

    /// AUX, CON and PRN in the first entries of the file table, where DOS
    /// opens them at startup and the handles 0 to 4 of every process
    /// start out referring to.
    fn open_standard_devices(&mut self) {
        for (sft, name, device) in
            [(SFT_AUX, "AUX", CharDevice::Nul), (SFT_CON, "CON", CharDevice::Con), (SFT_PRN, "PRN", CharDevice::Prn)]
        {
            let file = OpenFile {
                data: OpenData::Device(device),
                drive: DRIVE_C,
                owner: 0,
                key: 0,
                path: name.to_string(),
                mode: 0x02,
                refs: 0,
                fcb: false,
                empty_on_use: false,
                raw: false,
            };
            self.open_files.insert(sft, file);
        }
        self.sft_dirty = u128::MAX;
    }

    /// Put a file on Z:, where programs find it on the PATH.
    pub fn add_virtual_file(&mut self, name: &str, bytes: Vec<u8>) {
        if let Some(Storage::Tree { files, .. }) = self.drives[DRIVE_Z as usize].as_mut().map(|d| &mut d.storage) {
            files.insert(name, bytes);
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
            images: Vec::new(),
            image: 0,
            media_changed: false,
            boot_cd: None,
            shared: None,
            overlay: None,
        }
    }
}

/// The packed DOS time and date of a host file time, in local time; 1 Jan
/// 1980 for times before it.
pub fn system_time_to_dos(at: std::time::SystemTime) -> (u16, u16) {
    let datetime: DateTime<Local> = at.into();
    let time = ((datetime.hour() as u16) << 11) | ((datetime.minute() as u16) << 5) | ((datetime.second() as u16) / 2);
    let year = datetime.year();
    let date = match year {
        ..1980 => return (0, 0x0021),
        2108.. => 127 << 9 | 12 << 5 | 31,
        _ => (((year - 1980) as u16) << 9) | ((datetime.month() as u16) << 5) | (datetime.day() as u16),
    };
    (time, date)
}

/// The local time a packed DOS time and date stand for.
pub fn dos_to_system_time(time: u16, date: u16) -> Option<std::time::SystemTime> {
    use chrono::TimeZone;
    let (year, month, day) = (1980 + (date >> 9) as i32, (date >> 5 & 0x0F) as u32, (date & 0x1F) as u32);
    let (hour, minute, second) = ((time >> 11) as u32, (time >> 5 & 0x3F) as u32, (time & 0x1F) as u32 * 2);
    let local = chrono::Local.with_ymd_and_hms(year, month, day, hour, minute, second).earliest()?;
    Some(local.into())
}

#[cfg(test)]
mod tests;
