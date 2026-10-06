use chrono::{DateTime, Datelike, Local, Timelike};
use std::cell::Cell;
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

mod state;

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
    one(&|f| matches!(extension(f).as_str(), "cue" | "ins" | "inst"))
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

    /// The FAT file system behind `geometry`: a 1.44 MB diskette's for
    /// floppies, and a plausible one around the cluster count for the rest.
    pub fn layout(self) -> FatLayout {
        let (sectors_per_cluster, bytes_per_sector, clusters) = self.geometry();
        let (root_entries, sectors_per_track, heads, hidden_sectors) = match self {
            DriveKind::Floppy => (224, 18, 2, 0),
            _ => (512, 63, 16, 63),
        };
        let fat_bytes = if clusters < FAT12_MAX_CLUSTERS {
            ((clusters as u32 + 2) * 3).div_ceil(2)
        } else {
            (clusters as u32 + 2) * 2
        };
        let mut layout = FatLayout {
            bytes_per_sector,
            sectors_per_cluster,
            clusters: clusters as u32,
            reserved_sectors: 1,
            fats: 2,
            root_entries,
            sectors_per_fat: fat_bytes.div_ceil(bytes_per_sector as u32),
            sectors_per_track,
            heads,
            hidden_sectors,
            media: self.media_descriptor(),
            sectors: 0,
            fat32: None,
        };
        layout.sectors = layout.first_data_sector() + clusters as u32 * sectors_per_cluster as u32;
        layout
    }
}

/// The free space of a drive as the functions of before FAT32 (INT 21h
/// AH=1Bh, 1Ch and 36h) can tell it: (sectors per cluster, free clusters,
/// bytes per sector, total clusters) in 16 bits and never more than just
/// under 2 GB, which programs keep in signed 32-bit numbers. Bigger
/// clusters make up for fewer of them, as long as a cluster stays under
/// 32 KB (FORMAT divides by it), as MS-DOS 7.1 and DOSBox-X report them.
pub fn old_space(spc: u32, free: u32, bps: u32, total: u32) -> (u16, u16, u16, u16) {
    let mut factor = 1;
    while (total > 0xFFFF || free > 0xFFFF) && spc * factor <= 64 && bps * spc * factor < 0x8000 {
        factor *= 2;
    }
    let spc = spc * factor;
    let under_2gb = 0x7FFF_8000 / bps.max(1) / spc.max(1);
    let fit = |n: u32| (n / factor).min(under_2gb).min(0xFFFF) as u16;
    (spc as u16, fit(free), bps as u16, fit(total))
}

/// Volumes with fewer clusters than this have 12-bit FATs.
const FAT12_MAX_CLUSTERS: u16 = 4085;

/// Where a drive's FAT, root directory and data would lie, as DOS reports
/// it in the drive parameter block and the BIOS parameter block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FatLayout {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u16,
    pub clusters: u32,
    pub reserved_sectors: u16,
    pub fats: u16,
    pub root_entries: u16,
    pub sectors_per_fat: u32,
    pub sectors_per_track: u16,
    pub heads: u16,
    pub hidden_sectors: u32,
    pub media: u8,
    /// Sectors in the volume, which may run on past the last cluster.
    pub sectors: u32,
    /// A FAT32 volume's own fields.
    pub fat32: Option<Fat32Layout>,
}

/// The fields of a FAT32 volume's BPB that FAT12 and FAT16 don't have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fat32Layout {
    pub root_cluster: u32,
    pub fsinfo: u16,
    pub backup_boot: u16,
    pub ext_flags: u16,
}

impl FatLayout {
    /// The first sector of the root directory: FAT32's first data sector,
    /// its root directory being a chain of clusters.
    pub fn first_dir_sector(&self) -> u32 {
        self.reserved_sectors as u32 + self.fats as u32 * self.sectors_per_fat
    }

    pub fn first_data_sector(&self) -> u32 {
        let root_sectors = (self.root_entries as u32 * 32).div_ceil(self.bytes_per_sector as u32);
        self.first_dir_sector() + root_sectors
    }

    pub fn total_sectors(&self) -> u32 {
        self.sectors
    }

    pub fn cylinders(&self) -> u16 {
        let per_cylinder = self.sectors_per_track as u32 * self.heads as u32;
        (self.hidden_sectors as u64 + self.total_sectors() as u64).div_ceil(per_cylinder.max(1) as u64).min(u16::MAX as u64) as u16
    }

    /// "FAT12   ", "FAT16   " or "FAT32   ", as the boot sector names it.
    pub fn fs_type(&self) -> &'static [u8; 8] {
        match (self.fat32, self.clusters) {
            (Some(_), _) => b"FAT32   ",
            (None, n) if n < FAT12_MAX_CLUSTERS as u32 => b"FAT12   ",
            _ => b"FAT16   ",
        }
    }

    /// The DOS 4 BIOS parameter block, as a boot sector has it from offset
    /// 0Bh and IOCTL 440Dh/0860h returns it. FAT32's has no FAT size here.
    pub fn bpb(&self) -> [u8; 31] {
        let mut bpb = [0u8; 31];
        let total = self.total_sectors();
        let small_total = if total > 0xFFFF || self.fat32.is_some() { 0 } else { total as u16 };
        let small_fat = if self.fat32.is_some() { 0 } else { self.sectors_per_fat.min(0xFFFF) as u16 };
        bpb[0x00..0x02].copy_from_slice(&self.bytes_per_sector.to_le_bytes());
        bpb[0x02] = self.sectors_per_cluster as u8;
        bpb[0x03..0x05].copy_from_slice(&self.reserved_sectors.to_le_bytes());
        bpb[0x05] = self.fats as u8;
        bpb[0x06..0x08].copy_from_slice(&self.root_entries.to_le_bytes());
        bpb[0x08..0x0A].copy_from_slice(&small_total.to_le_bytes());
        bpb[0x0A] = self.media;
        bpb[0x0B..0x0D].copy_from_slice(&small_fat.to_le_bytes());
        bpb[0x0D..0x0F].copy_from_slice(&self.sectors_per_track.to_le_bytes());
        bpb[0x0F..0x11].copy_from_slice(&self.heads.to_le_bytes());
        bpb[0x11..0x15].copy_from_slice(&self.hidden_sectors.to_le_bytes());
        if small_total == 0 {
            bpb[0x15..0x19].copy_from_slice(&total.to_le_bytes());
        }
        bpb
    }
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
}

/// What an open handle reads and writes.
enum OpenData {
    Host(File),
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
    /// The disks made of shared host directories that a state being loaded
    /// has, with their manifests, for once the drives are mounted as it
    /// has them (`restore_shared`).
    pub(crate) shared_from_state: Option<Vec<(u8, String)>>,
    /// What mounting found to tell the user (a .dosc's patch that can't
    /// be used), for the bus's log (`Bus::mount_drive`).
    pub notes: Vec<String>,
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
            shared_from_state: None,
            notes: Vec::new(),
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

    /// Mount a host directory, or a disk or CD image, as `drive`. A list of
    /// images (the path and `opts.more_images`) puts the first in, and
    /// Ctrl+F4 the next (`swap_image`). Unless `replace` is set, the drive
    /// must not already be mounted. Replacing closes the files open on it.
    /// On A: and B: the drive is a floppy whatever `opts.kind` says. A
    /// numbered drive (`numbered_drive`) takes floppy or hard disk images
    /// whatever is on them.
    pub fn mount(
        &mut self,
        drive: u8,
        path: &Path,
        opts: MountOptions,
        replace: bool,
    ) -> Result<PathBuf, String> {
        if drive >= DRIVE_SLOTS {
            return Err("Invalid drive letter".to_string());
        }
        let name = drive_name(drive);
        if drive == DRIVE_Z || opts.kind == DriveKind::Virtual {
            return Err(format!("Drive {} is reserved", name));
        }
        if self.is_mounted(drive) && !replace {
            return Err(format!("Drive {} is already mounted", name));
        }
        // A: and B: are floppies whatever the type asked for, but a CD
        // can't be one.
        let floppy_drive = drive < FLOPPY_DRIVES;
        if floppy_drive && opts.kind == DriveKind::CdRom {
            return Err(format!("Drive {} is a floppy drive and can't be a CD-ROM", name));
        }
        let spec = MountSpec { drive, path: path.to_path_buf(), opts: opts.clone() };
        // An archive: its files, or the disk or CD image in it (or the one
        // the path names in it), through an overlay; the changes go to
        // `-overlay`'s folder.
        let mut overlaid = None;
        let archive_path;
        let mut path = path;
        // A path into an archive (`game.zip/CD/GAME.CUE`): what is there.
        let inside = if hostfs::exists(path) { None } else { crate::archive::split(path) };
        let whole = hostfs::is_file(path) && crate::archive::is_archive_name(path);
        let mut more_images = opts.more_images.clone();
        if (whole && more_images.is_empty()) || inside.is_some() {
            let whole_archive = inside.is_none();
            let (archive, inner) = inside.unwrap_or_else(|| (path.to_path_buf(), String::new()));
            let canonical = hostfs::canonicalize(&archive).map_err(|e| e.to_string())?;
            let variant = opts.variant.as_deref().filter(|_| whole_archive);
            let stack = crate::archive::open_variant(&canonical, variant)?;
            self.notes.extend(stack.notes.take());
            let image = if inner.is_empty() { archive_image(&stack.files()) } else { Some(inner) };
            let upper = opts.overlay.clone().filter(|_| !opts.read_only && (image.is_some() || opts.kind != DriveKind::CdRom));
            // A list of images in it: all of them in it.
            for image in &mut more_images {
                match crate::archive::split(image) {
                    Some((other, inner)) if hostfs::canonicalize(&other).ok().as_ref() == Some(&canonical) => {
                        *image = PathBuf::from(inner);
                    }
                    _ => return Err(format!("{} isn't in {} with the first image", image.display(), canonical.display())),
                }
            }
            let o = Self::overlaid(Box::new(stack), canonical, upper)?;
            for image in &mut more_images {
                *image = o.layer.root().join(&*image);
            }
            archive_path = match image {
                Some(image) => o.layer.root().join(image),
                None => o.layer.root().to_path_buf(),
            };
            path = &archive_path;
            overlaid = Some(o);
        }
        if hostfs::is_file(path) {
            let mut images = Vec::new();
            for image in std::iter::once(path).chain(more_images.iter().map(PathBuf::as_path)) {
                if !hostfs::is_file(image) {
                    return Err(format!("{} is not a disk or CD image", image.display()));
                }
                let canonical = hostfs::canonicalize(image).map_err(|e| e.to_string())?;
                // Two drives on one image would each think they know
                // what's on it.
                let elsewhere = (0..DRIVE_SLOTS)
                    .find(|&d| d != drive && self.drive(d).is_some_and(|other| other.images.contains(&canonical)));
                if let Some(other) = elsewhere {
                    return Err(format!("{} is already mounted as {}", image.display(), drive_name(other)));
                }
                images.push(canonical);
            }
            let (kind, storage, volume_label, writable) = Self::open_image(drive, &images[0], &opts)?;
            self.close_drive_files(drive);
            let shown = overlaid.as_ref().and_then(|o| o.shown(&images[0])).unwrap_or_else(|| images[0].clone());
            self.drives[drive as usize] = Some(Drive {
                kind,
                storage,
                current_dir: String::new(),
                label: Self::label_for(&opts, &volume_label),
                read_only: opts.read_only || !writable,
                mount: Some(spec),
                images,
                image: 0,
                media_changed: true,
                boot_cd: None,
                shared: None,
                overlay: overlaid,
            });
            return Ok(shown);
        }
        if !opts.more_images.is_empty() {
            return Err("Only disk and CD images can be mounted as a list".to_string());
        }
        if drive_number(drive).is_some() {
            return Err(format!("{} is not a disk image", path.display()));
        }
        if !hostfs::is_dir(path) {
            return Err(format!("{} is not a directory or a disk or CD image", path.display()));
        }
        let kind = if floppy_drive { DriveKind::Floppy } else { opts.kind };
        // In an archive, the folder of it the path names.
        let in_archive = overlaid.as_ref().map(|_| path.to_path_buf());
        let canonical = match &overlaid {
            Some(o) => o.lower.clone(),
            None => hostfs::canonicalize(path).map_err(|e| e.to_string())?,
        };
        if overlaid.is_none()
            && let Some(upper) = opts.overlay.as_ref().filter(|_| kind != DriveKind::CdRom && !opts.read_only)
        {
            let lower = Box::new(crate::overlay::Folder(canonical.clone()));
            overlaid = Some(Self::overlaid(lower, canonical.clone(), Some(upper.clone()))?);
        }
        let root = in_archive.unwrap_or_else(|| overlaid.as_ref().map_or_else(|| canonical.clone(), |o| o.layer.root().to_path_buf()));
        // An archive without the folder for its changes can't be written.
        let unwritable = overlaid.as_ref().is_some_and(|o| o.upper.is_none());

        self.close_drive_files(drive);
        self.drives[drive as usize] = Some(Drive {
            kind,
            storage: Storage::Host(root),
            current_dir: String::new(),
            label: Self::label_for(&opts, DEFAULT_LABEL),
            read_only: opts.read_only || kind == DriveKind::CdRom || unwritable,
            mount: Some(spec),
            images: Vec::new(),
            image: 0,
            media_changed: floppy_drive,
            boot_cd: None,
            shared: None,
            overlay: overlaid,
        });
        Ok(canonical)
    }

    /// The overlay of `lower` (`display` to show, the directory or archive)
    /// with the changes in `upper`, as a layer. Layers are the thread's
    /// own (`hostfs`), so theirs needn't be Send.
    #[allow(clippy::arc_with_non_send_sync)]
    fn overlaid(lower: Box<dyn crate::overlay::Lower>, display: PathBuf, upper: Option<PathBuf>) -> Result<Overlaid, String> {
        let overlay = crate::overlay::Overlay::new(lower, upper.clone())
            .map_err(|e| format!("{}: {}", upper.as_deref().unwrap_or(Path::new("")).display(), e))?;
        Ok(Overlaid { layer: hostfs::add_layer(std::sync::Arc::new(overlay)), lower: display, upper })
    }

    /// The label of a drive: the one the mount asks for, or else `default`.
    fn label_for(opts: &MountOptions, default: &str) -> String {
        opts.label
            .as_deref()
            .map(normalize_label)
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| normalize_label(default))
    }

    /// Open the disk or CD image at `path` for `drive`: the drive's type,
    /// its storage, the volume label and whether the image can be written.
    fn open_image(drive: u8, path: &Path, opts: &MountOptions) -> Result<(DriveKind, Storage, String, bool), String> {
        if let Some(number) = drive_number(drive) {
            return Self::raw_storage(number, path, opts);
        }
        let found = diskimage::detect(path, opts.kind)?;
        if found == ImageKind::Cd {
            Self::check_cd_drive(drive)?;
            return Self::cd_storage(CdImage::open(path)?);
        }
        // A: and B: take a hard disk image without a partition table as a
        // floppy of its size.
        let floppy = found == ImageKind::Floppy || drive < FLOPPY_DRIVES;
        let name = path.display().to_string();
        Self::fat_storage(drive, found, &name, || Self::open_disk(path, floppy, opts))
    }

    /// The floppy or hard disk image at `path`, its changes in a delta
    /// file in the mount's overlay folder if it has one (`diskdelta`).
    fn open_disk(path: &Path, floppy: bool, opts: &MountOptions) -> Result<DiskImage, String> {
        match Self::delta_for(path, opts) {
            Some(delta) => DiskImage::open_delta(path, &delta, floppy, opts.geometry, opts.read_only),
            None => DiskImage::open(path, floppy, opts.geometry, opts.read_only),
        }
    }

    /// Where the changes of the disk image at `path` go, under the mount's
    /// overlay folder: `<image>.rdelta`, beside where the image would be
    /// copied up to. An image in an archive that was copied up whole
    /// before deltas is written to as it is.
    fn delta_for(path: &Path, opts: &MountOptions) -> Option<PathBuf> {
        let upper = opts.overlay.as_ref()?;
        let name = match hostfs::is_layer(path) {
            true => hostfs::layer_path(path),
            false => path.file_name()?.to_string_lossy().into_owned(),
        };
        if hostfs::is_layer(path) && hostfs::is_file(upper.join(&name)) {
            return None;
        }
        Some(upper.join(format!("{}.rdelta", name)))
    }

    /// The type, storage, volume label and writability of the disk mounted
    /// as `number`: the image's sectors for the BIOS, a floppy's for 0 and 1
    /// and a hard disk's for 2 and 3, whatever file system they hold, or
    /// none.
    fn raw_storage(number: u8, path: &Path, opts: &MountOptions) -> Result<(DriveKind, Storage, String, bool), String> {
        if diskimage::detect(path, opts.kind) == Ok(ImageKind::Cd) {
            return Err(format!("{} is a CD image, which can't be mounted by number", path.display()));
        }
        let floppy = number < FLOPPY_DRIVES;
        let disk = Self::open_disk(path, floppy, opts)?;
        let kind = if floppy { DriveKind::Floppy } else { DriveKind::HardDisk };
        let writable = disk.writable();
        Ok((kind, Storage::Raw(Rc::new(disk)), String::new(), writable))
    }

    /// Whether a CD image can go in `drive`: not in a floppy drive, and not
    /// as C:.
    fn check_cd_drive(drive: u8) -> Result<(), String> {
        if drive < FLOPPY_DRIVES {
            return Err(format!("Drive {}: is a floppy drive and can't be a CD-ROM", drive_letter(drive)));
        }
        if drive == DRIVE_C {
            return Err("Drive C: can't be a CD-ROM".to_string());
        }
        Ok(())
    }

    /// The type, storage, volume label and writability of a CD drive with
    /// `image` in it.
    fn cd_storage(image: CdImage) -> Result<(DriveKind, Storage, String, bool), String> {
        // A disc of only audio tracks has no file system.
        let (files, volume_label) = match image.data_track() {
            Some(_) => {
                let volume = crate::cdrom::iso9660::read_volume(&image)?;
                (volume.files, volume.label)
            }
            None => (MemFs::new(), "AUDIO_CD".to_string()),
        };
        let volume_label = if volume_label.is_empty() { "CDROM".to_string() } else { volume_label };
        Ok((DriveKind::CdRom, Storage::Tree { files, image: Some(Rc::new(image)) }, volume_label, false))
    }

    /// The type, storage, volume label and writability of `drive` with the
    /// floppy or hard disk image `open` opens, which was `found` to be of
    /// that kind and `name` names in messages.
    fn fat_storage(
        drive: u8,
        found: ImageKind,
        name: &str,
        open: impl FnOnce() -> Result<DiskImage, String>,
    ) -> Result<(DriveKind, Storage, String, bool), String> {
        let open = || -> Result<(Rc<DiskImage>, FatVolume), String> {
            let disk = Rc::new(open()?);
            let (start, sectors) = disk.fat_volume()?;
            let volume = FatVolume::open(disk.clone(), start, sectors)?;
            Ok((disk, volume))
        };
        let (disk, volume) = open().map_err(|e| match found {
            ImageKind::HardDisk if drive < FLOPPY_DRIVES => {
                format!("Drive {}: is a floppy drive and can't hold a hard disk image", drive_letter(drive))
            }
            _ => format!("{}: {}", name, e),
        })?;
        let volume_label = volume.label().unwrap_or_default();
        let kind = if disk.is_floppy() { DriveKind::Floppy } else { DriveKind::HardDisk };
        Ok((kind, Storage::Fat(Rc::new(volume)), volume_label, disk.writable()))
    }

    /// Mount a disk or CD image held in memory as `drive`, replacing what is
    /// there, as `mount` mounts an image file named `name`: which kind of
    /// image it is comes from `opts.kind`, the name and the contents (see
    /// `diskimage::detect_memory`).
    pub fn mount_memory_image(
        &mut self,
        drive: u8,
        name: &str,
        data: MemoryImage,
        opts: MountOptions,
    ) -> Result<(), String> {
        Self::check_image_drive(drive, &opts)?;
        let found = diskimage::detect_memory(name, &data, opts.kind)?;
        let (kind, storage, volume_label, writable) = if found == ImageKind::Cd {
            Self::check_cd_drive(drive)?;
            Self::cd_storage(CdImage::from_memory(name, data)?)?
        } else {
            let floppy = found == ImageKind::Floppy || drive < FLOPPY_DRIVES;
            let open = || DiskImage::from_memory(name, data, floppy, opts.geometry, opts.read_only);
            Self::fat_storage(drive, found, name, open)?
        };
        self.insert_image_drive(drive, kind, storage, &volume_label, writable, &opts);
        Ok(())
    }

    /// Mount the floppy or hard disk image `disk` as `drive`, replacing what
    /// is there: one made in memory (`DiskImage::blank_hard_disk`).
    pub fn mount_disk_image(&mut self, drive: u8, disk: DiskImage, opts: MountOptions) -> Result<(), String> {
        Self::check_image_drive(drive, &opts)?;
        let found = if disk.is_floppy() { ImageKind::Floppy } else { ImageKind::HardDisk };
        let name = disk.path().display().to_string();
        let (kind, storage, volume_label, writable) = Self::fat_storage(drive, found, &name, || Ok(disk))?;
        self.insert_image_drive(drive, kind, storage, &volume_label, writable, &opts);
        Ok(())
    }

    /// Whether an image held in memory can be mounted as `drive`.
    fn check_image_drive(drive: u8, opts: &MountOptions) -> Result<(), String> {
        if drive >= LASTDRIVE {
            return Err("Invalid drive letter".to_string());
        }
        if drive == DRIVE_Z || opts.kind == DriveKind::Virtual {
            return Err(format!("Drive {}: is reserved", drive_letter(drive)));
        }
        Ok(())
    }

    /// Put an image held in memory in `drive`, closing the files open on
    /// what was there.
    fn insert_image_drive(
        &mut self,
        drive: u8,
        kind: DriveKind,
        storage: Storage,
        volume_label: &str,
        writable: bool,
        opts: &MountOptions,
    ) {
        self.close_drive_files(drive);
        self.drives[drive as usize] = Some(Drive {
            kind,
            storage,
            current_dir: String::new(),
            label: Self::label_for(opts, volume_label),
            read_only: opts.read_only || !writable,
            mount: None,
            images: Vec::new(),
            image: 0,
            media_changed: true,
            boot_cd: None,
            shared: None,
            overlay: None,
        });
    }

    /// Put the next image in a drive mounted from a list of them, as
    /// Ctrl+F4 does. Files open on the drive keep reading the disk they were
    /// opened on, and the current directory stays if the new disk has it.
    /// Returns what changed, or None for a drive with one image or none.
    pub fn swap_image(&mut self, drive: u8) -> Result<Option<String>, String> {
        let Some(d) = self.drive(drive).filter(|d| d.images.len() > 1) else {
            return Ok(None);
        };
        self.select_image(drive, (d.image + 1) % d.images.len())
    }

    /// The images of a drive mounted from images, and which one is in.
    pub fn images(&self, drive: u8) -> Option<(&[PathBuf], usize)> {
        self.drive(drive).filter(|d| !d.images.is_empty()).map(|d| (d.images.as_slice(), d.image))
    }

    /// Put image `index` of a drive mounted from images in, as `swap_image`
    /// does the next. Returns what changed, or None if it is in already.
    pub fn select_image(&mut self, drive: u8, index: usize) -> Result<Option<String>, String> {
        let name = drive_name(drive);
        let Some(d) = self.drive(drive).filter(|d| index < d.images.len()) else {
            return Err(format!("Drive {} has no disk {}", name, index + 1));
        };
        if d.image == index {
            return Ok(None);
        }
        let path = d.images[index].clone();
        let opts = d.mount.as_ref().map(|m| m.opts.clone()).unwrap_or_default();
        let (kind, storage, volume_label, writable) = Self::open_image(drive, &path, &opts)?;
        let d = self.drives[drive as usize].as_mut().unwrap();
        if kind != d.kind {
            return Err(format!("{} can't go in drive {}, which is a {}", path.display(), name, d.kind.name()));
        }
        d.storage = storage;
        d.image = index;
        d.label = Self::label_for(&opts, &volume_label);
        d.read_only = opts.read_only || !writable;
        d.media_changed = true;
        let count = d.images.len();
        let current = format!("{}\\{}", name, d.current_dir);
        if drive < LASTDRIVE
            && !self.is_directory(&current)
            && let Some(d) = self.drives[drive as usize].as_mut()
        {
            d.current_dir.clear();
        }
        let file = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        Ok(Some(format!("Drive {} disk {} of {}: {}", name, index + 1, count, file)))
    }

    /// Add `path` to the end of the images of a drive mounted from images,
    /// for `select_image` or Ctrl+F4 to put in later.
    pub fn add_image(&mut self, drive: u8, path: &Path) -> Result<(), String> {
        if !hostfs::is_file(path) {
            return Err(format!("{} is not a disk or CD image", path.display()));
        }
        let canonical = hostfs::canonicalize(path).map_err(|e| e.to_string())?;
        let elsewhere = (0..DRIVE_SLOTS).find(|&d| self.drive(d).is_some_and(|other| other.images.contains(&canonical)));
        if let Some(other) = elsewhere {
            return Err(format!("{} is already mounted as {}", path.display(), drive_name(other)));
        }
        let Some(d) = self.drives.get_mut(drive as usize).and_then(Option::as_mut).filter(|d| !d.images.is_empty())
        else {
            return Err(format!("Drive {} isn't mounted from disk images", drive_name(drive)));
        };
        // The mount as saved states have it lists them all.
        if let Some(mount) = d.mount.as_mut() {
            mount.opts.more_images.push(canonical.clone());
        }
        d.images.push(canonical);
        Ok(())
    }

    /// Take image `index`, which must not be the one in, out of the images
    /// of a drive mounted from images.
    pub fn remove_image(&mut self, drive: u8, index: usize) -> Result<(), String> {
        let name = drive_name(drive);
        let Some(d) = self.drives.get_mut(drive as usize).and_then(Option::as_mut).filter(|d| index < d.images.len())
        else {
            return Err(format!("Drive {} has no disk {}", name, index + 1));
        };
        if d.image == index {
            return Err(format!("Disk {} is in drive {}", index + 1, name));
        }
        d.images.remove(index);
        if d.image > index {
            d.image -= 1;
        }
        if let Some(mount) = d.mount.as_mut() {
            mount.path = d.images[0].clone();
            mount.opts.more_images = d.images[1..].to_vec();
        }
        Ok(())
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

    /// C: empty and read-only in place of what was there, which is mounted
    /// elsewhere now (REMOUNT): C: is always there.
    pub fn empty_drive_c(&mut self) {
        self.close_drive_files(DRIVE_C);
        self.drives[DRIVE_C as usize] = Some(Self::memory_drive(MemFs::new(), ""));
    }

    /// Unmount `drive`, closing its open files. C: and Z: cannot be removed.
    /// If it was the current drive, C: becomes current.
    pub fn unmount(&mut self, drive: u8) -> Result<(), String> {
        if drive >= DRIVE_SLOTS || !self.is_mounted(drive) {
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
        let gone: Vec<u16> = self
            .open_files
            .iter()
            .filter(|&(&sft, f)| sft >= FIRST_FILE && f.drive == drive)
            .map(|(&sft, _)| sft)
            .collect();
        for sft in gone {
            self.open_files.remove(&sft);
            self.mark_dirty(sft);
        }
    }

    fn drive(&self, drive: u8) -> Option<&Drive> {
        self.drives.get(drive as usize).and_then(|d| d.as_ref())
    }

    /// The drive that boots when Rust-DOS starts (`MountOptions::boot`),
    /// if one does.
    pub fn boot_drive(&self) -> Option<u8> {
        (0..DRIVE_SLOTS).find(|&d| self.drive(d).and_then(|d| d.mount.as_ref()).is_some_and(|spec| spec.opts.boot))
    }

    /// Make `drive` the one that boots when Rust-DOS starts, or none.
    pub fn set_boot_drive(&mut self, drive: Option<u8>) {
        for (d, slot) in self.drives.iter_mut().enumerate() {
            if let Some(spec) = slot.as_mut().and_then(|d| d.mount.as_mut()) {
                spec.opts.boot = drive == Some(d as u8);
            }
        }
    }

    /// Have `drive` boot when Rust-DOS starts, in place of any other, or
    /// not.
    pub fn set_boots(&mut self, drive: u8, boots: bool) {
        if boots {
            self.set_boot_drive(Some(drive));
        } else if let Some(spec) = self.drives.get_mut(drive as usize).and_then(|d| d.as_mut()?.mount.as_mut()) {
            spec.opts.boot = false;
        }
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

    /// The disc a booted system's CD-ROM drive reads for `drive`: its CD
    /// image, or the disc made from the host folder mounted as a CD.
    pub fn boot_cd_image(&self, drive: u8) -> Option<Rc<CdImage>> {
        let d = self.drive(drive)?;
        d.image().or(d.boot_cd.as_ref()).cloned()
    }

    /// Whether `drive` is a host folder mounted as a CD, which a booted
    /// system reads as a disc made from it.
    pub fn is_folder_cd(&self, drive: u8) -> bool {
        self.drive(drive).is_some_and(|d| d.kind == DriveKind::CdRom && d.host_root().is_some())
    }

    /// Make the discs of the host folders mounted as CDs that have none,
    /// for a booted system. Returns what the log should say.
    pub fn prepare_boot_cds(&mut self) -> Vec<String> {
        (0..LASTDRIVE)
            .filter(|&d| self.is_folder_cd(d) && self.drive(d).is_some_and(|d| d.boot_cd.is_none()))
            .collect::<Vec<_>>()
            .into_iter()
            .flat_map(|d| match self.make_boot_cd(d) {
                Ok(lines) => lines,
                Err(e) => vec![e],
            })
            .collect()
    }

    /// Make the disc of the host folder mounted as CD drive `drive` again,
    /// with what the folder holds now, as a new disc in the drive. Returns
    /// what the log should say.
    pub fn make_boot_cd(&mut self, drive: u8) -> Result<Vec<String>, String> {
        let letter = drive_letter(drive);
        let Some(d) = self.drives.get_mut(drive as usize).and_then(Option::as_mut) else {
            return Err(format!("Drive {}: is not mounted", letter));
        };
        let Some(root) = d.host_root().filter(|_| d.kind == DriveKind::CdRom).map(Path::to_path_buf) else {
            return Err(format!("Drive {}: is not a folder mounted as a CD", letter));
        };
        // The label the mount asks for, or the folder's name.
        let label = d.mount.as_ref().and_then(|m| m.opts.label.clone()).unwrap_or_else(|| {
            root.file_name().map_or_else(|| d.label.clone(), |n| n.to_string_lossy().into_owned())
        });
        let disc = crate::cdrom::folder::build(&root, &label)
            .map_err(|e| format!("Drive {}: can't be made a CD: {}", letter, e))?;
        let mut lines = vec![format!(
            "Drive {}: is a CD made from {} ({} MB)",
            letter,
            root.display(),
            (disc.sectors as u64 * crate::cdrom::DATA_SECTOR as u64).div_ceil(1 << 20)
        )];
        lines.extend(disc.skipped.iter().map(|s| format!("Drive {}: left off the CD: {}", letter, s)));
        d.boot_cd = Some(Rc::new(CdImage::from_folder(disc, &root)?));
        d.media_changed = true;
        Ok(lines)
    }

    /// Whether `drive` is a host directory a booted system gets as a hard
    /// disk: D: to Y: unless its mount says `-noshare`, C: with `-share`.
    pub fn is_shareable(&self, drive: u8) -> bool {
        let Some(d) = self.drive(drive).filter(|d| d.kind == DriveKind::HardDisk && d.host_root().is_some()) else {
            return false;
        };
        let share = d.mount.as_ref().and_then(|m| m.opts.share);
        share.unwrap_or((DRIVE_C + 1..DRIVE_Z).contains(&drive))
    }

    /// Whether `drive` has a disk made of its host directory for a booted
    /// system.
    pub fn is_shared(&self, drive: u8) -> bool {
        self.drive(drive).is_some_and(|d| d.shared.is_some())
    }

    /// The drives with disks made of their host directories, in
    /// drive-letter order.
    pub fn shared_drives(&self) -> Vec<u8> {
        (0..LASTDRIVE).filter(|&d| self.is_shared(d)).collect()
    }

    /// Make the disks of the host directories shared with a booted system
    /// that have none yet (a restart keeps what the system wrote). Returns
    /// what the log should say.
    pub fn prepare_shared_disks(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        for drive in 0..LASTDRIVE {
            if !self.is_shareable(drive) || self.is_shared(drive) {
                continue;
            }
            let d = self.drives[drive as usize].as_mut().expect("a shareable drive");
            let root = d.host_root().expect("a host directory").to_path_buf();
            let shown = d.shown_root().expect("a host directory").to_path_buf();
            match crate::shared_disk::SharedDisk::build_named(&root, &shown) {
                Ok((shared, skipped)) => {
                    lines.push(format!("Drive {}: is a hard disk made from {}", drive_letter(drive), shown.display()));
                    lines.extend(skipped.into_iter().map(|s| format!("Drive {}: left off the disk: {}", drive_letter(drive), s)));
                    d.shared = Some(shared);
                }
                Err(e) => lines.push(format!("Drive {}: can't be shared: {}", drive_letter(drive), e)),
            }
        }
        lines
    }

    /// Copy what a booted system changed on the shared disk of `drive`, or
    /// of every drive, into the host directories. `last` is the copy at
    /// its shutdown, which deletes files too. Returns a line for each.
    pub fn sync_shared(&mut self, drive: Option<u8>, last: bool) -> Vec<String> {
        let mut lines = Vec::new();
        for d in 0..LASTDRIVE {
            if drive.is_some_and(|only| only != d) {
                continue;
            }
            let Some(drive) = self.drives[d as usize].as_mut().filter(|d| d.shared.is_some()) else { continue };
            if drive.read_only {
                lines.push(format!("Drive {}: is read-only: what the system wrote stays on its disk", drive_letter(d)));
                continue;
            }
            // Under an overlay, the changes go to its folder.
            let target = drive.overlay.as_ref().and_then(|o| o.upper.clone());
            let shared = drive.shared.as_mut().expect("a shared disk");
            let report = shared.sync(last);
            let target = target.unwrap_or_else(|| shared.root.clone());
            lines.push(format!("Drive {}: {} copied to {}: {}", drive_letter(d), if last { "was" } else { "is" }, target.display(), report.summary()));
            lines.extend(report.conflicts.iter().map(|c| format!("Drive {}: {} changed on both sides", drive_letter(d), c)));
            lines.extend(report.errors.iter().map(|e| format!("Drive {}: {}", drive_letter(d), e)));
        }
        lines
    }

    /// The drives with disks made of shared host directories and their
    /// manifests, as a state keeps them.
    pub(crate) fn shared_manifests(&self) -> Vec<(u8, String)> {
        self.shared_drives()
            .into_iter()
            .filter_map(|d| Some((d, self.drive(d)?.shared.as_ref()?.manifest.to_text())))
            .collect()
    }

    /// The disks of shared host directories a state being loaded has: an
    /// empty one for each drive that has none, which the state's
    /// checkpoint fills from the copy beside the state file, and the
    /// state's manifest.
    pub(crate) fn restore_shared(&mut self) -> Result<(), String> {
        let Some(list) = self.shared_from_state.take() else { return Ok(()) };
        for (drive, text) in list {
            let manifest = crate::shared_disk::Manifest::from_text(&text).ok_or("a broken manifest")?;
            let d = self.drives.get_mut(drive as usize).and_then(Option::as_mut);
            let Some(d) = d.filter(|d| d.kind == DriveKind::HardDisk && d.host_root().is_some()) else {
                return Err(format!("drive {} isn't a host directory, which the system had as a disk", drive_name(drive)));
            };
            if d.shared.is_none() {
                let root = d.host_root().expect("a host directory").to_path_buf();
                d.shared = Some(crate::shared_disk::SharedDisk::empty(&root)?);
            }
            d.shared.as_mut().expect("a shared disk").manifest = manifest;
        }
        Ok(())
    }

    /// At a booted system's shutdown (`last`): copy what it changed on the
    /// shared disks into the host directories, and let go of the disks.
    /// Otherwise, as it goes away without one (a state of the built-in DOS
    /// loaded over it), without deleting. Returns what the log should say.
    pub fn finish_shared_disks(&mut self, last: bool) -> Vec<String> {
        let lines = self.sync_shared(None, last);
        for d in self.drives.iter_mut().flatten() {
            d.shared = None;
        }
        lines
    }

    /// Let go of the discs made from host folders: the built-in DOS reads
    /// the folders themselves.
    pub fn drop_boot_cds(&mut self) {
        for d in self.drives.iter_mut().flatten() {
            d.boot_cd = None;
        }
    }

    /// The file system of a drive mounted from a disk image.
    pub fn fat_volume(&self, drive: u8) -> Option<Rc<FatVolume>> {
        self.drive(drive)?.fat().cloned()
    }

    /// The disk image of a drive mounted from one, as the BIOS reads it.
    pub fn bios_image(&self, drive: u8) -> Option<Rc<DiskImage>> {
        self.drive(drive)?.disk().cloned()
    }

    /// The IDE slot `drive` was mounted for (`-ide`), if any.
    pub fn ide_slot(&self, drive: u8) -> Option<crate::ide::IdeSlot> {
        self.drive(drive)?.mount.as_ref()?.opts.ide
    }

    /// Keep journals of the writes to the disk images in the drives, or
    /// stop: a booted system's states and rewind take its disks back with
    /// its memory (`DiskImage::revert_to`).
    pub fn keep_journals(&self, on: bool) {
        for drive in 0..DRIVE_SLOTS {
            if let Some(disk) = self.bios_image(drive) {
                disk.keep_journal(on);
            }
        }
    }

    /// Take the disks back to the checkpoints of the state just loaded.
    pub(crate) fn revert_disks(&mut self) -> Vec<String> {
        let mut failed = Vec::new();
        for (drive, id) in std::mem::take(&mut self.reverts) {
            let reverted = self.bios_image(drive).ok_or_else(|| "it's gone".to_string()).and_then(|disk| disk.revert_to(id));
            if let Err(e) = reverted {
                failed.push(format!("drive {}: {}", drive_name(drive), e));
            }
        }
        failed
    }

    /// Whether another disk went in `drive` since the last time this was
    /// asked (INT 13h AH=16h's disk change line).
    pub fn take_media_changed(&mut self, drive: u8) -> bool {
        self.drives
            .get_mut(drive as usize)
            .and_then(Option::as_mut)
            .is_some_and(|d| std::mem::take(&mut d.media_changed))
    }

    pub fn drive_info(&self, drive: u8) -> Option<DriveInfo> {
        self.drive(drive).map(|d| DriveInfo {
            drive,
            kind: d.kind,
            root: d.shown_root().map(Path::to_path_buf),
            overlay: d
                .disk()
                .and_then(|disk| disk.delta_path().map(Path::to_path_buf))
                .or_else(|| d.overlay.as_ref().and_then(|o| o.upper.clone())),
            image: d
                .image()
                .map(|image| image.path().to_path_buf())
                .or_else(|| d.mounted_disk().map(|disk| disk.path().to_path_buf()))
                .map(|path| d.shown(path)),
            images: d.images.iter().map(|path| d.shown(path.clone())).collect(),
            image_index: d.image,
            label: d.label.clone(),
            read_only: !d.writable(),
            current_dir: d.current_dir.to_ascii_uppercase(),
            mount: d.mount.clone(),
        })
    }

    /// The lettered drives: the ones DOS has.
    pub fn mounted_drives(&self) -> Vec<DriveInfo> {
        (0..LASTDRIVE).filter_map(|d| self.drive_info(d)).collect()
    }

    /// The disks mounted by number, which only the BIOS has.
    pub fn numbered_drives(&self) -> Vec<DriveInfo> {
        (LASTDRIVE..DRIVE_SLOTS).filter_map(|d| self.drive_info(d)).collect()
    }

    /// Floppy drives the BIOS reports: one for A:, two for B: even with A:
    /// empty, as a machine with two drives and one disk.
    pub fn floppy_units(&self) -> u8 {
        (0..FLOPPY_DRIVES)
            .rev()
            .find(|&unit| self.drive_kind(self.floppy_unit(unit)) == Some(DriveKind::Floppy))
            .map_or(0, |unit| unit + 1)
    }

    /// The drive behind the BIOS's floppy unit `unit` (0 or 1): the disk
    /// mounted as that number, or else A: or B:.
    pub fn floppy_unit(&self, unit: u8) -> u8 {
        let numbered = numbered_drive(unit);
        if self.is_mounted(numbered) { numbered } else { unit }
    }

    /// The drives behind the BIOS's hard disk units, 80h up: the disks
    /// mounted as 2 and 3 are 80h and 81h, and the hard disk drives that
    /// `lettered` takes fill the units they leave, in drive-letter order.
    pub fn hard_disk_units(&self, lettered: impl Fn(u8) -> bool) -> Vec<u8> {
        let mut drives = self.drives_of_kind(DriveKind::HardDisk).into_iter().filter(|&d| lettered(d));
        let mut units: Vec<u8> = (FLOPPY_DRIVES..NUMBERED_DRIVES)
            .filter_map(|number| {
                let numbered = numbered_drive(number);
                if self.is_mounted(numbered) { Some(numbered) } else { drives.next() }
            })
            .collect();
        units.extend(drives);
        units
    }

    /// Mounted lettered drives of the given kind, in drive-letter order.
    pub fn drives_of_kind(&self, kind: DriveKind) -> Vec<u8> {
        (0..LASTDRIVE)
            .filter(|&d| self.drive_kind(d) == Some(kind))
            .collect()
    }

    /// Drive a file handle was opened on (Z: for virtual handles).
    pub fn handle_drive(&self, handle: u16) -> Option<u8> {
        self.open_files.get(&handle).map(|f| f.drive)
    }

    /// What tells an open file apart from others, for telling sequential
    /// disk access from random (the disk noises).
    pub fn handle_key(&self, handle: u16) -> Option<u64> {
        self.open_files.get(&handle).map(|f| f.key)
    }

    /// Which drive a DOS path is on.
    pub fn drive_of(&self, dos_path: &str) -> Option<u8> {
        self.split_drive(&dos_path.replace('/', "\\")).map(|(drive, _)| drive)
    }

    /// A number that tells the file a DOS path names apart from others.
    pub fn file_key(&self, dos_path: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.qualify_path(dos_path).unwrap_or_else(|| dos_path.to_ascii_uppercase()).hash(&mut hasher);
        hasher.finish()
    }

    /// The character device an open handle refers to, if any.
    pub fn handle_device(&self, handle: u16) -> Option<CharDevice> {
        match self.open_files.get(&handle)?.data {
            OpenData::Device(device) => Some(device),
            _ => None,
        }
    }

    /// What DOS keeps in the file table entry `sft` about the file open
    /// there, but its position (`position`).
    pub fn sft_entry(&self, sft: u16) -> Option<SftEntry> {
        let open = self.open_files.get(&sft)?;
        let size = match &open.data {
            OpenData::Host(file) => file.len().unwrap_or(0),
            OpenData::Memory(data, _) => data.len() as u64,
            OpenData::Image(_, extent, _) => extent.size as u64,
            OpenData::Fat { volume, at, .. } => volume.reload(*at).map_or(0, |entry| entry.size as u64),
            OpenData::Device(_) => 0,
        };
        let (time, date) = self.file_time(sft).unwrap_or((0, 0));
        let leaf = open.path.rsplit(['\\', '/', ':']).next().unwrap_or("");
        let (stem, ext) = match open.data {
            OpenData::Device(_) => (leaf.split('.').next().unwrap_or(""), ""),
            _ => leaf.rsplit_once('.').unwrap_or((leaf, "")),
        };
        let mut name = [b' '; 11];
        for (slot, b) in name[..8].iter_mut().zip(stem.trim().bytes()) {
            *slot = b.to_ascii_uppercase();
        }
        for (slot, b) in name[8..].iter_mut().zip(ext.bytes()) {
            *slot = b.to_ascii_uppercase();
        }
        Some(SftEntry {
            refs: open.refs,
            mode: open.mode as u16 | if open.fcb { 0x8000 } else { 0 },
            device: self.handle_device(sft),
            drive: open.drive,
            owner: open.owner,
            name,
            size: size.min(u32::MAX as u64) as u32,
            time,
            date,
        })
    }

    /// Where in the open file `sft` the next read or write goes.
    pub fn position(&self, sft: u16) -> Option<u64> {
        match &self.open_files.get(&sft)?.data {
            OpenData::Host(file) => (&*file).stream_position().ok(),
            OpenData::Memory(_, pos) | OpenData::Image(_, _, pos) | OpenData::Fat { pos, .. } => Some(pos.get()),
            OpenData::Device(_) => Some(0),
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
    /// Trailing spaces are the padding of a name DOS packs into 8 and 3
    /// characters, and go: Comanche opens "LH66.RLE    ".
    fn logical_components<'a>(drive: &'a Drive, rest: &'a str) -> Vec<&'a str> {
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
    pub(crate) fn locate(&self, dos_path: &str) -> Option<(u8, PathBuf)> {
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

    /// The drive a DOS path is on and the path there, if that drive is
    /// mounted from a disk image.
    fn locate_fat(&self, dos_path: &str) -> Option<(u8, Rc<FatVolume>, Vec<String>)> {
        let normalized = dos_path.replace('/', "\\");
        let (drive_num, rest) = self.split_drive(&normalized)?;
        let drive = self.drive(drive_num)?;
        let volume = drive.fat()?.clone();
        let parts = Self::logical_components(drive, rest).into_iter().map(str::to_string).collect();
        Some((drive_num, volume, parts))
    }

    /// The file or directory a DOS path names on a drive mounted from a
    /// disk image: None if the path is on another drive.
    fn find_fat(&self, dos_path: &str) -> Option<Result<fat::Entry, u8>> {
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
    fn host_entries(dir: &Path) -> Vec<(String, String)> {
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

    /// A file just opened as `filename` with access `mode`, with the one
    /// reference of whoever opened it.
    fn opened(&mut self, data: OpenData, drive: u8, owner: u16, key: u64, filename: &str, mode: u8) -> OpenFile {
        let path = self.qualify_path(filename).unwrap_or_else(|| filename.to_ascii_uppercase());
        OpenFile { data, drive, owner, key, path, mode, refs: 1, fcb: false }
    }

    /// The lowest unused entry of the file table.
    fn free_handle(&self) -> Result<u16, u8> {
        (FIRST_FILE..FILES)
            .find(|h| !self.open_files.contains_key(h))
            .ok_or(0x04) // Too many open files
    }

    /// Put an open file in the table at `sft`.
    fn insert(&mut self, sft: u16, file: OpenFile) {
        self.open_files.insert(sft, file);
        self.mark_dirty(sft);
    }

    fn mark_dirty(&mut self, sft: u16) {
        self.sft_dirty |= 1u128.checked_shl(sft as u32).unwrap_or(0);
    }

    fn mark_moved(&mut self, sft: u16) {
        self.position_dirty |= 1u128.checked_shl(sft as u32).unwrap_or(0);
    }

    /// The entries whose copy in DOS memory is out of date since the last
    /// call, and those of them whose position only.
    pub fn take_dirty(&mut self) -> (u128, u128) {
        let whole = std::mem::take(&mut self.sft_dirty);
        (whole, std::mem::take(&mut self.position_dirty) & !whole)
    }

    /// The character device `filename` names, EMMXXXX0 among them while
    /// there is EMS.
    pub fn device(&self, filename: &str) -> Option<CharDevice> {
        let device = char_device(filename);
        if device.is_some() || !self.emm_device {
            return device;
        }
        let last = filename.rsplit(['\\', '/', ':']).next()?;
        let stem = last.split('.').next()?.trim();
        stem.eq_ignore_ascii_case("EMMXXXX0").then_some(CharDevice::Emm)
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
        if let Some(device) = self.device(filename) {
            let handle = self.free_handle()?;
            let file = self.opened(OpenData::Device(device), self.current_drive, owner, 0, filename, mode);
            self.insert(handle, file);
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
            let key = self.file_key(filename);
            let file = self.opened(data, drive, owner, key, filename, mode);
            self.insert(handle, file);
            return Ok(handle);
        }

        if let Some((drive, volume, parts)) = self.locate_fat(filename) {
            let writable = self.is_writable(drive);
            let access = mode & 0x03;
            match access {
                3 => return Err(0x0C),
                1 if !writable => return Err(0x05),
                _ => {}
            }
            let parts = refs(&parts);
            let handle = self.free_handle()?;
            let entry = match volume.find(&parts) {
                Ok(entry) if entry.is_dir() => return Err(0x05),
                Ok(entry) => entry,
                Err(0x02) if create && writable => volume.create(&parts, 0)?,
                Err(0x02) if create => return Err(0x05),
                Err(e) => return Err(e),
            };
            // Read/write opens on write-protected disks are downgraded, as
            // on CD-ROMs; read-only files can't be written.
            let write = access != 0 && writable;
            if write && entry.attr & fat::ATTR_READ_ONLY != 0 {
                return Err(0x05);
            }
            let at = entry.at.ok_or(0x05u8)?;
            let data = OpenData::Fat { volume, at, pos: Rc::new(Cell::new(0)), write };
            let key = self.file_key(filename);
            let file = self.opened(data, drive, owner, key, filename, mode);
            self.insert(handle, file);
            return Ok(handle);
        }

        let (drive, path) = self.locate(filename).ok_or(0x03)?; // Path not found
        // A directory (or a bare "D:") isn't a file: access denied.
        if hostfs::is_dir(&path) {
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
                let key = self.file_key(filename);
                let file = self.opened(OpenData::Host(file), drive, owner, key, filename, mode);
                self.insert(handle, file);
                Ok(handle)
            }
            Err(_) => Err(0x02),
        }
    }

    // INT 21h, AH=3Ch: Create File. Opens read/write, creating the file if
    // missing but never truncating (see int21.rs for why).
    pub fn create_file(&mut self, filename: &str, owner: u16) -> Result<u16, u8> {
        if self.device(filename).is_some() {
            return self.open_file(filename, 0x02, owner);
        }
        let normalized = filename.replace('/', "\\");
        let (drive, _) = self.split_drive(&normalized).ok_or(0x03)?;
        self.check_writable(drive)?;
        self.open_or_create(filename, 0x02, owner, true)
    }

    /// INT 21h, AH=5Bh: create a file that must not exist yet.
    pub fn create_new_file(&mut self, filename: &str, owner: u16) -> Result<u16, u8> {
        if self.device(filename).is_none() && self.exists(filename) {
            return Err(0x50); // File exists
        }
        self.create_file(filename, owner)
    }

    /// INT 21h, AH=5Ah: create a file with a unique name in `directory`
    /// (which ends in a backslash). Returns the handle and the name.
    pub fn create_temp_file(&mut self, directory: &str, owner: u16) -> Result<(u16, String), u8> {
        let stamp = crate::hosttime::now().timestamp_subsec_nanos();
        for i in 0..1000u32 {
            let name = format!("{}{:08X}", directory, stamp.wrapping_add(i) & 0x0FFF_FFFF);
            if let Ok(handle) = self.create_new_file(&name, owner) {
                return Ok((handle, name));
            }
        }
        Err(0x05)
    }

    /// Write a whole file at once, as COPY does: `data` at `dos_path`,
    /// replacing a file that is there, dated `stamp` (the packed DOS time
    /// and date) or now.
    pub fn write_whole_file(&self, dos_path: &str, data: &[u8], stamp: Option<(u16, u16)>) -> Result<(), u8> {
        if let Some((drive, volume, parts)) = self.locate_fat(dos_path) {
            self.check_writable(drive)?;
            let (time, date) = stamp.unwrap_or_else(fat::dos_now);
            return volume.put_file(&refs(&parts), data, time, date);
        }
        if self.locate_in_memory(dos_path).is_some() {
            return Err(0x05);
        }
        let (drive, path) = self.locate(dos_path).ok_or(0x03)?;
        self.check_writable(drive)?;
        if hostfs::is_dir(&path) {
            return Err(0x05);
        }
        if !path.parent().is_some_and(hostfs::is_dir) {
            return Err(0x03);
        }
        hostfs::write(&path, data).map_err(|_| 0x05)?;
        if let Some(when) = stamp.and_then(|(time, date)| dos_to_system_time(time, date)) {
            let _ = OpenOptions::new().write(true).open(&path).and_then(|f| f.set_modified(when));
        }
        Ok(())
    }

    /// INT 21h, AH=41h: delete a file.
    pub fn delete_file(&self, filename: &str) -> Result<(), u8> {
        if let Some((drive, volume, parts)) = self.locate_fat(filename) {
            let parts = refs(&parts);
            let entry = volume.find(&parts)?;
            if entry.is_dir() {
                return Err(0x02);
            }
            self.check_writable(drive)?;
            if entry.attr & fat::ATTR_READ_ONLY != 0 {
                return Err(0x05);
            }
            return volume.remove(&parts);
        }
        let (drive, path) = self.locate(filename).ok_or(0x03)?;
        if !hostfs::is_file(&path) {
            return Err(0x02); // File not found
        }
        self.check_writable(drive)?;
        hostfs::remove_file(path).map_err(|_| 0x05)
    }

    /// INT 21h, AH=56h: rename or move a file within a drive.
    pub fn rename_file(&self, from: &str, to: &str) -> Result<(), u8> {
        if let Some((drive, volume, from_parts)) = self.locate_fat(from) {
            let from_parts = refs(&from_parts);
            if from_parts.is_empty() {
                return Err(0x05);
            }
            volume.find(&from_parts)?;
            self.check_writable(drive)?;
            let normalized = to.replace('/', "\\");
            let (to_drive, rest) = self.split_drive(&normalized).ok_or(0x03)?;
            if to_drive != drive {
                return Err(0x11); // Not same device
            }
            let to_parts = Self::logical_components(self.drive(drive).ok_or(0x03u8)?, rest);
            if to_parts.is_empty() {
                return Err(0x03);
            }
            return volume.rename(&from_parts, &to_parts);
        }
        let (drive, source) = self.locate(from).ok_or(0x03)?;
        if !hostfs::exists(&source) {
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
        if leaf.is_empty() || !hostfs::is_dir(&parent) {
            return Err(0x03);
        }
        if self.find_existing_child(&parent, leaf).is_some() {
            return Err(0x05); // Destination exists
        }
        hostfs::rename(source, parent.join(leaf.to_uppercase())).map_err(|_| 0x05)
    }

    /// INT 21h, AX=5700h: the DOS time and date of a file's last change.
    pub fn file_time(&self, handle: u16) -> Result<(u16, u16), u8> {
        let open = self.open_files.get(&handle).ok_or(0x06)?;
        let modified = match &open.data {
            OpenData::Host(f) => f.modified(),
            OpenData::Memory(..) => return Ok((MEMORY_TIME, MEMORY_DATE)),
            OpenData::Image(_, extent, _) => return Ok((extent.time, extent.date)),
            OpenData::Fat { volume, at, .. } => {
                let entry = volume.reload(*at)?;
                return Ok((entry.time, entry.date));
            }
            OpenData::Device(_) => None,
        };
        let t: DateTime<Local> = modified.map_or_else(crate::hosttime::now, DateTime::from);
        let time = (t.hour() << 11 | t.minute() << 5 | t.second() / 2) as u16;
        let year = (t.year().max(1980) - 1980) as u32;
        let date = (year << 9 | t.month() << 5 | t.day()) as u16;
        Ok((time, date))
    }

    /// INT 21h AX=5701h: set the time and date of an open file. Only
    /// files on disk images keep them; host files keep their own.
    pub fn set_file_time(&self, handle: u16, time: u16, date: u16) -> Result<(), u8> {
        match &self.open_files.get(&handle).ok_or(0x06u8)?.data {
            OpenData::Fat { volume, at, .. } => volume.set_time(*at, time, date),
            _ => Ok(()),
        }
    }

    /// True if the file table entry `sft` is an open file.
    pub fn is_open(&self, sft: u16) -> bool {
        self.open_files.contains_key(&sft)
    }

    /// One more handle (or FCB) refers to the open file `sft`.
    pub fn add_ref(&mut self, sft: u16) -> bool {
        let Some(open) = self.open_files.get_mut(&sft) else { return false };
        open.refs = open.refs.saturating_add(1);
        self.mark_dirty(sft);
        true
    }

    /// A handle (or FCB) that referred to the open file `sft` is closed
    /// (INT 21h AH=3Eh): the file closes with the last of them, but the
    /// standard devices stay. False if it isn't open.
    pub fn close_file(&mut self, sft: u16) -> bool {
        let Some(open) = self.open_files.get_mut(&sft) else { return false };
        open.refs = open.refs.saturating_sub(1);
        if open.refs == 0 && sft >= FIRST_FILE {
            self.open_files.remove(&sft);
        }
        self.mark_dirty(sft);
        true
    }

    /// Whether a process started now gets a handle for the open file
    /// `sft`: unless it was opened with the no-inherit bit (80h).
    pub fn inheritable(&self, sft: u16) -> bool {
        self.open_files.get(&sft).is_some_and(|f| f.mode & 0x80 == 0 && !f.fcb)
    }

    /// The open file `sft` is an FCB's.
    pub fn set_fcb(&mut self, sft: u16) {
        if let Some(open) = self.open_files.get_mut(&sft) {
            open.fcb = true;
            self.mark_dirty(sft);
        }
    }

    /// Close the files a terminating process opened for FCBs, which no
    /// handle refers to.
    pub fn close_fcb_files(&mut self, owner: u16) {
        let gone: Vec<u16> =
            self.open_files.iter().filter(|(_, f)| f.fcb && f.owner == owner).map(|(&sft, _)| sft).collect();
        for sft in gone {
            self.open_files.remove(&sft);
            self.mark_dirty(sft);
        }
    }

    /// Close every open file, for when the shell is reloaded.
    pub fn close_all_files(&mut self) {
        self.open_files.clear();
        self.open_standard_devices();
    }

    // INT 21h, AH=3Fh: Read from File
    pub fn read_file(&mut self, handle: u16, count: usize) -> Result<Vec<u8>, u16> {
        self.mark_moved(handle);
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
                OpenData::Fat { volume, at, pos, .. } => {
                    let entry = volume.reload(*at)?;
                    let mut buffer = vec![0u8; count];
                    let n = volume.read(&entry, pos.get(), &mut buffer)?;
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
        self.mark_dirty(handle);
        if let Some(open) = self.open_files.get_mut(&handle) {
            let file = match &mut open.data {
                OpenData::Host(file) => file,
                OpenData::Memory(..) | OpenData::Image(..) => return Err(0x05),
                OpenData::Fat { write: false, .. } => return Err(0x05),
                OpenData::Fat { volume, at, pos, .. } => {
                    let n = volume.write(*at, pos.get(), data)?;
                    pos.set(pos.get() + n as u64);
                    return Ok(n as u16);
                }
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
        self.mark_moved(handle);
        if let Some(open) = self.open_files.get_mut(&handle) {
            let file = match &mut open.data {
                OpenData::Host(file) => file,
                OpenData::Memory(data, pos) => return Self::seek_in(data.len() as u64, pos, offset, origin),
                OpenData::Image(_, extent, pos) => return Self::seek_in(extent.size as u64, pos, offset, origin),
                OpenData::Fat { volume, at, pos, .. } => {
                    let size = volume.reload(*at)?.size;
                    return Self::seek_in(size as u64, pos, offset, origin);
                }
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

    /// Make an open file `size` bytes long, cutting it or adding zeros
    /// (INT 21h AH=28h with no records). Only host files can be cut;
    /// the others grow with zeros written at their end.
    pub fn set_file_size(&mut self, handle: u16, size: u64) -> Result<(), u8> {
        let open = self.open_files.get(&handle).ok_or(0x06u8)?;
        if let OpenData::Host(file) = &open.data {
            let result = file.set_len(size).map_err(|_| 0x05);
            self.mark_dirty(handle);
            return result;
        }
        let end = self.seek_file(handle, 0, 2).map_err(|_| 0x05u8)?;
        if end > size {
            return Err(0x05);
        }
        let zeros = vec![0u8; (size - end) as usize];
        self.write_file(handle, &zeros).map(|_| ())
    }

    // ========================================================================
    // FILESYSTEM METADATA & SEARCH
    // ========================================================================

    /// Allocation geometry of `drive` (0-based) for INT 21h AH=1Bh/1Ch:
    /// (sectors per cluster, bytes per sector, total clusters), as the old
    /// functions can tell it (`old_space`).
    pub fn drive_geometry(&self, drive: u8) -> Option<(u16, u16, u16)> {
        self.layout(drive).map(|l| {
            let (spc, _, bps, total) =
                old_space(l.sectors_per_cluster as u32, 0, l.bytes_per_sector as u32, l.clusters);
            (spc, bps, total)
        })
    }

    /// Where `drive`'s FAT, directory and data are: a disk image's own, or
    /// a plausible one for the others (see `DriveKind::layout`).
    pub fn layout(&self, drive: u8) -> Option<FatLayout> {
        let d = self.drive(drive)?;
        Some(d.fat().map_or_else(|| d.kind.layout(), |volume| volume.layout()))
    }

    /// The media descriptor byte of `drive` (0 if it isn't mounted).
    pub fn media_descriptor(&self, drive: u8) -> u8 {
        self.layout(drive).map_or(0, |l| l.media)
    }

    /// The volume serial number of a drive mounted from a disk image.
    pub fn volume_serial(&self, drive: u8) -> Option<u32> {
        self.fat_volume(drive)?.serial()
    }

    /// INT 21h AH=36h: (sectors per cluster, free clusters, bytes per
    /// sector, total clusters) of `drive` (0 the current one, 1 A:), as
    /// the old function can tell them (`old_space`).
    pub fn get_disk_free_space(&self, drive: u8) -> Result<(u16, u16, u16, u16), u16> {
        let (spc, free, bps, total) = self.get_disk_free_space32(drive)?;
        Ok(old_space(spc, free, bps, total))
    }

    /// The free space of `drive` (0 the current one, 1 A:) as AX=7303h
    /// tells it: (sectors per cluster, free clusters, bytes per sector,
    /// total clusters), whatever their size.
    pub fn get_disk_free_space32(&self, drive: u8) -> Result<(u32, u32, u32, u32), u16> {
        let target_drive = if drive == 0 {
            self.current_drive
        } else {
            drive - 1
        };

        let d = self.drive(target_drive).ok_or(0x0Fu16)?; // Invalid Drive
        if let Some(volume) = d.fat() {
            let layout = volume.layout();
            let free = volume.free_clusters().min(layout.clusters);
            return Ok((layout.sectors_per_cluster as u32, free, layout.bytes_per_sector as u32, layout.clusters));
        }
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
        Ok((spc as u32, free as u32, bps as u32, total as u32))
    }

    /// Clusters occupied by the tree under `root` (one per directory plus
    /// each file rounded up). Stops counting once `cap` is reached so that
    /// mounting a huge directory as a floppy stays cheap. Symlinks are not
    /// followed.
    fn used_clusters(root: &Path, cluster_bytes: u64, cap: u64) -> u64 {
        let mut used = 0u64;
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            let Ok(read_dir) = hostfs::read_dir(&dir) else {
                continue;
            };
            for entry in read_dir {
                if entry.name.to_string_lossy().starts_with('.') {
                    continue;
                }
                if entry.is_dir {
                    used += 1;
                    pending.push(entry.path);
                } else if let Some(meta) = entry.metadata().ok().filter(|m| m.is_file()) {
                    used += meta.len.div_ceil(cluster_bytes);
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
        if let Some(found) = self.find_fat(filename) {
            return found.map(|e| e.attr as u16);
        }
        let (drive, path) = self.locate(filename).ok_or(0x03)?;
        if !hostfs::exists(&path) {
            return Err(0x02); // File Not Found
        }

        let mut attr: u16 = 0;
        if hostfs::is_dir(&path) {
            attr |= 0x10; // Directory
        } else {
            attr |= 0x20; // Archive (standard file)
        }
        // Reflect host read-only state into DOS R/O bit. On Unix, read-only means
        // no user-write permission. On Windows, the readonly flag. Everything
        // on read-only media is R/O too.
        let host_ro = hostfs::metadata(&path).is_ok_and(|m| m.readonly);
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
        if let Some((drive, volume, parts)) = self.locate_fat(filename) {
            let parts = refs(&parts);
            volume.find(&parts)?;
            self.check_writable(drive)?;
            return volume.set_attr(&parts, attr as u8);
        }
        let (drive, path) = self.locate(filename).ok_or(0x03)?;
        if !hostfs::exists(&path) {
            return Err(0x02);
        }
        self.check_writable(drive)?;
        if let Ok(meta) = hostfs::metadata(&path) {
            let want_ro = (attr & 0x01) != 0;
            if meta.readonly != want_ro {
                // Ignore permission-set errors on systems where it's not supported.
                let _ = hostfs::set_readonly(&path, want_ro);
            }
        }
        Ok(())
    }

    /// DOS AH=39h: Create a directory at the given DOS path.
    pub fn create_directory(&self, path: &str) -> Result<(), u8> {
        let normalized = path.replace('/', "\\");
        let (drive, rest) = self.split_drive(&normalized).ok_or(0x03)?;
        self.check_writable(drive)?;
        if let Some((_, volume, parts)) = self.locate_fat(path) {
            if parts.is_empty() {
                return Err(0x05);
            }
            return volume.mkdir(&refs(&parts));
        }

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
        if !hostfs::is_dir(&parent_path) {
            return Err(0x03);
        }
        // Case-insensitive: MKDIR "Foo" should collide with existing "FOO".
        if let Some(existing) = self.find_existing_child(&parent_path, leaf) {
            let full = parent_path.join(existing);
            if hostfs::exists(&full) {
                return Err(0x05); // Access denied / already exists
            }
        }
        let target = parent_path.join(leaf.to_uppercase());
        hostfs::create_dir(&target).map_err(|_| 0x05)
    }

    /// DOS AH=3Ah: Remove an empty directory.
    pub fn remove_directory(&self, path: &str) -> Result<(), u8> {
        let normalized = path.replace('/', "\\");
        let (drive_num, rest) = self.split_drive(&normalized).ok_or(0x03)?;
        if let Some((_, volume, parts)) = self.locate_fat(path) {
            let parts = refs(&parts);
            if parts.is_empty() || !volume.find(&parts).is_ok_and(|e| e.is_dir()) {
                return Err(0x03);
            }
            self.check_writable(drive_num)?;
            if self.drive(drive_num).is_some_and(|d| canonical_path(&parts) == d.current_dir) {
                return Err(0x10);
            }
            return volume.rmdir(&parts);
        }
        let (host_path, dos_form) = self.resolve_names_on(drive_num, rest).ok_or(0x03)?;
        if !hostfs::is_dir(&host_path) {
            return Err(0x03);
        }
        self.check_writable(drive_num)?;
        // DOS error 0x10 = "attempt to remove current directory".
        if self.drive(drive_num).is_some_and(|d| dos_form.eq_ignore_ascii_case(&d.current_dir)) {
            return Err(0x10);
        }
        hostfs::remove_dir(&host_path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => 0x03,
            _ => 0x05, // Access denied (e.g. not empty)
        })
    }

    /// Case-insensitive lookup of a child by name in a host directory.
    fn find_existing_child(&self, parent: &Path, name: &str) -> Option<std::ffi::OsString> {
        let upper = name.to_uppercase();
        if let Ok(entries) = hostfs::read_dir(parent) {
            for e in entries {
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
        // DOS packs the pattern into a name of 8 and an extension of 3,
        // padded with spaces, so what goes past them is dropped and
        // trailing spaces are padding: Comanche looks for "LH66.RLE    ".
        let field = |part: &'_ str, len: usize| {
            let end = part.char_indices().nth(len).map_or(part.len(), |(i, _)| i);
            part[..end].trim_end_matches(' ').to_string()
        };
        let (p_name, p_ext) = (field(p_name, 8), field(p_ext, 3));

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

        match_part(f_name, &p_name) && match_part(f_ext, &p_ext)
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

    /// The files a path names, with wildcards in its last part or not,
    /// each with its fully qualified DOS path, as DEL, COPY, REN, FOR and
    /// IF EXIST take them: directories, hidden and system files are left
    /// out.
    pub fn matching_files(&self, spec: &str) -> Result<Vec<(String, DosDirEntry)>, u8> {
        let entries = self.list_directory(spec, 0)?;
        let dir = self.qualify_directory(spec).ok_or(0x03u8)?;
        let dir = dir.trim_end_matches('\\');
        Ok(entries
            .into_iter()
            .filter(|e| !e.is_dir)
            .map(|e| (format!("{}\\{}", dir, e.filename), e))
            .collect())
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

        if let Some(volume) = drive.fat() {
            let dir = Self::logical_components(drive, search_dir_str);
            for entry in volume.list(&dir).map_err(|_| 0x03u8)? {
                let attr = entry.attr;
                if (attr as u16 & restricted_bits) & !search_attr != 0 || !Self::matches_pattern(&entry.name, pattern) {
                    continue;
                }
                valid_entries.push(DosDirEntry {
                    is_dir: entry.is_dir(),
                    is_readonly: attr & fat::ATTR_READ_ONLY != 0,
                    filename: entry.name,
                    size: entry.size,
                    dos_time: entry.time,
                    dos_date: entry.date,
                    attr,
                });
            }
            return Ok(valid_entries);
        }

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

        if !hostfs::is_dir(&host_dir) {
            return Err(0x03);
        }
        let is_host_root = drive.host_root() == Some(host_dir.as_path());
        let media_ro = !drive.writable();

        if !is_host_root {
            valid_entries.extend(Self::dot_entries(pattern));
        }

        for (original_name, final_name) in Self::host_entries(&host_dir) {
            let Ok(metadata) = hostfs::metadata(host_dir.join(&original_name)) else {
                continue;
            };

            let is_dir = metadata.is_dir;
            let is_readonly = media_ro || metadata.readonly;
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

            let (dos_time, dos_date) = system_time_to_dos(metadata.modified.unwrap_or(std::time::SystemTime::now()));

            valid_entries.push(DosDirEntry {
                filename: final_name,
                size: metadata.len as u32,
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
mod tests {
    use super::*;
    use std::fs;

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
    fn an_overlay_keeps_the_directory_as_it_was() {
        let base = scratch("overlay");
        fs::create_dir_all(base.join("c")).unwrap();
        fs::create_dir_all(base.join("game/saves")).unwrap();
        fs::write(base.join("game/GAME.CFG"), b"old").unwrap();
        fs::write(base.join("game/saves/slot1.sav"), b"one").unwrap();
        let mut disk = DiskController::new(base.join("c"));
        let opts = MountOptions { overlay: Some(base.join("upper")), ..MountOptions::default() };
        disk.mount(3, &base.join("game"), opts, false).unwrap();
        let info = disk.drive_info(3).unwrap();
        assert_eq!(info.root.as_deref(), Some(fs::canonicalize(base.join("game")).unwrap().as_path()));
        assert_eq!(info.overlay.as_deref(), Some(base.join("upper").as_path()));

        let h = disk.open_file(r"D:\GAME.CFG", 2, PSP).unwrap();
        assert_eq!(disk.write_file(h, b"new"), Ok(3));
        disk.close_file(h);
        let h = disk.create_file(r"D:\SAVES\SLOT2.SAV", PSP).unwrap();
        disk.write_file(h, b"two").unwrap();
        disk.close_file(h);
        disk.delete_file(r"D:\SAVES\SLOT1.SAV").unwrap();
        disk.create_directory(r"D:\NEW").unwrap();
        disk.rename_file(r"D:\SAVES\SLOT2.SAV", r"D:\NEW\SLOT2.SAV").unwrap();

        let h = disk.open_file(r"D:\GAME.CFG", 0, PSP).unwrap();
        assert_eq!(disk.read_file(h, 3).unwrap(), b"new");
        let names = |disk: &DiskController, spec: &str| -> Vec<String> {
            disk.list_directory(spec, 0x10).unwrap().into_iter().map(|e| e.filename).collect()
        };
        assert!(names(&disk, r"D:\SAVES\*.*").iter().all(|n| n.starts_with('.')));
        assert_eq!(names(&disk, r"D:\NEW\SLOT*.*"), ["SLOT2.SAV"]);
        // The directory as it was.
        assert_eq!(fs::read(base.join("game/GAME.CFG")).unwrap(), b"old");
        assert_eq!(fs::read(base.join("game/saves/slot1.sav")).unwrap(), b"one");
        assert_eq!(fs::read_dir(base.join("game")).unwrap().count(), 2);
        assert_eq!(fs::read(base.join("upper/GAME.CFG")).unwrap(), b"new");
        assert_eq!(fs::read(base.join("upper/NEW/SLOT2.SAV")).unwrap(), b"two");
    }

    #[test]
    fn an_archive_is_a_drive_with_its_changes_beside_it() {
        let base = scratch("archive");
        fs::create_dir_all(base.join("c")).unwrap();
        let zip = crate::archive::zip::tests::zip(&[
            ("Game/GAME.EXE", b"MZ game", true),
            ("Game/SAVES/SLOT1.SAV", b"one", false),
        ]);
        fs::write(base.join("game.zip"), &zip).unwrap();
        let mut disk = DiskController::new(base.join("c"));

        // Without a folder for its changes, it is read-only.
        disk.mount(3, &base.join("game.zip"), MountOptions::default(), false).unwrap();
        assert!(disk.drive_info(3).unwrap().read_only);
        assert_eq!(disk.create_file(r"D:\NEW.TXT", PSP), Err(0x05));

        let opts = MountOptions { overlay: Some(base.join("saves")), ..MountOptions::default() };
        disk.mount(3, &base.join("game.zip"), opts, true).unwrap();
        let info = disk.drive_info(3).unwrap();
        assert!(!info.read_only);
        assert_eq!(info.root.as_deref(), Some(fs::canonicalize(base.join("game.zip")).unwrap().as_path()));
        let h = disk.open_file(r"D:\GAME.EXE", 0, PSP).unwrap();
        assert_eq!(disk.read_file(h, 16).unwrap(), b"MZ game");
        disk.close_file(h);
        let h = disk.open_file(r"D:\SAVES\SLOT1.SAV", 2, PSP).unwrap();
        disk.write_file(h, b"ONE").unwrap();
        disk.close_file(h);
        disk.delete_file(r"D:\GAME.EXE").unwrap();
        let names: Vec<String> = disk.list_directory(r"D:\*.*", 0x10).unwrap().into_iter().map(|e| e.filename).collect();
        assert_eq!(names, ["SAVES"]);
        assert_eq!(fs::read(base.join("saves/SAVES/SLOT1.SAV")).unwrap(), b"ONE");
        assert_eq!(fs::read(base.join("game.zip")).unwrap(), zip, "the archive as it was");
    }

    #[test]
    fn a_patched_file_is_what_dos_reads_and_writes() {
        let base = scratch("archive_patch");
        fs::create_dir_all(base.join("c")).unwrap();
        fs::write(base.join("game.dosz"), crate::archive::zip::tests::zip(&[("GAME.EXE", b"MZ protected", false)])).unwrap();
        let mut patch = b"PATCH".to_vec();
        patch.extend([0, 0, 3, 0, 9]);
        patch.extend(b"cracked!!");
        patch.extend(b"EOF");
        fs::write(base.join("game.dosc"), crate::archive::zip::tests::zip(&[("GAME.EXE", &patch, false)])).unwrap();
        let mut disk = DiskController::new(base.join("c"));
        let opts = MountOptions { overlay: Some(base.join("saves")), ..MountOptions::default() };
        disk.mount(3, &base.join("game.dosz"), opts, false).unwrap();
        let h = disk.open_file(r"D:\GAME.EXE", 0, PSP).unwrap();
        assert_eq!(disk.read_file(h, 32).unwrap(), b"MZ cracked!!");
        disk.close_file(h);
        // Written to, it goes to the saves as it reads.
        let h = disk.open_file(r"D:\GAME.EXE", 2, PSP).unwrap();
        disk.write_file(h, b"MZ").unwrap();
        disk.close_file(h);
        assert_eq!(fs::read(base.join("saves/GAME.EXE")).unwrap(), b"MZ cracked!!");
    }

    #[test]
    fn a_path_into_an_archive_mounts_what_is_there() {
        let base = scratch("archive_path");
        fs::create_dir_all(base.join("c")).unwrap();
        let zip = crate::archive::zip::tests::zip(&[
            ("Game/GAME.EXE", b"MZ game", false),
            ("Game/CD/DATA.DAT", b"cd data", false),
            ("Game/DISKS/disk1.img", &vec![0u8; 368_640], false),
            ("Game/DISKS/disk2.img", &vec![0u8; 368_640], false),
        ]);
        fs::write(base.join("game.zip"), &zip).unwrap();
        let mut disk = DiskController::new(base.join("c"));
        disk.mount(3, &base.join("game.zip/CD"), cdrom(), false).unwrap();
        let h = disk.open_file(r"D:\DATA.DAT", 0, PSP).unwrap();
        assert_eq!(disk.read_file(h, 16).unwrap(), b"cd data");
        assert!(disk.list_directory(r"D:\GAME.EXE", 0).unwrap().is_empty(), "only the folder");
        // In any case, as DOSBox configurations name them.
        disk.mount(5, &base.join("game.zip/cd"), MountOptions::default(), false).unwrap();
        let h = disk.open_file(r"F:\DATA.DAT", 0, PSP).unwrap();
        assert_eq!(disk.read_file(h, 16).unwrap(), b"cd data");
        let list = MountOptions { more_images: vec![base.join("game.zip/DISKS/disk2.img")], ..MountOptions::default() };
        disk.mount(numbered_drive(0), &base.join("game.zip/DISKS/DISK1.IMG"), list, false).unwrap();
        let archive = fs::canonicalize(base.join("game.zip")).unwrap();
        let info = disk.drive_info(numbered_drive(0)).unwrap();
        assert_eq!(info.image, Some(archive.join("DISKS/DISK1.IMG")));
        assert_eq!(info.images, [archive.join("DISKS/DISK1.IMG"), archive.join("DISKS/disk2.img")]);
        let elsewhere = MountOptions { more_images: vec![base.join("c/disk2.img")], ..MountOptions::default() };
        assert!(disk.mount(numbered_drive(1), &base.join("game.zip/DISKS/disk2.img"), elsewhere, false).is_err());
        assert!(disk.mount(4, &base.join("game.zip/NONE"), MountOptions::default(), false).is_err());
    }

    #[test]
    fn a_cue_sheet_in_an_archive_mounts_its_disc() {
        let base = scratch("archive_cue");
        fs::create_dir_all(base.join("c")).unwrap();
        fs::create_dir_all(base.join("disc")).unwrap();
        fs::write(base.join("disc/DATA.DAT"), b"on the disc").unwrap();
        let folder = CdImage::from_folder(crate::cdrom::folder::build(&base.join("disc"), "GAMECD").unwrap(), &base.join("disc")).unwrap();
        let mut iso = Vec::new();
        for lba in 0..folder.leadout() {
            let mut sector = [0u8; crate::cdrom::DATA_SECTOR];
            folder.read_data(lba, &mut sector).unwrap();
            iso.extend_from_slice(&sector);
        }
        let cue = b"FILE \"GAME.ISO\" BINARY\r\n  TRACK 01 MODE1/2048\r\n    INDEX 01 00:00:00\r\n";
        let zip = crate::archive::zip::tests::zip(&[("Game/CD/GAME.CUE", cue, false), ("Game/CD/GAME.ISO", &iso, false)]);
        fs::write(base.join("game.zip"), &zip).unwrap();
        let mut disk = DiskController::new(base.join("c"));
        disk.mount(3, &base.join("game.zip/CD/GAME.CUE"), MountOptions::default(), false).unwrap();
        let info = disk.drive_info(3).unwrap();
        assert_eq!((info.kind, info.label.as_str()), (DriveKind::CdRom, "GAMECD"));
        let h = disk.open_file(r"D:\DATA.DAT", 0, PSP).unwrap();
        assert_eq!(disk.read_file(h, 32).unwrap(), b"on the disc");
    }

    #[test]
    fn an_archive_of_a_disk_image_mounts_the_image() {
        let base = scratch("archive_image");
        fs::create_dir_all(base.join("c")).unwrap();
        let zip = crate::archive::zip::tests::zip(&[("booter.img", &vec![0u8; 368_640], true), ("README.TXT", b"hi", false)]);
        fs::write(base.join("booter.zip"), &zip).unwrap();
        let mut disk = DiskController::new(base.join("c"));
        let opts = MountOptions { overlay: Some(base.join("saves")), ..MountOptions::default() };
        disk.mount(numbered_drive(0), &base.join("booter.zip"), opts, false).unwrap();
        let info = disk.drive_info(numbered_drive(0)).unwrap();
        let archive = fs::canonicalize(base.join("booter.zip")).unwrap();
        assert_eq!(info.image, Some(archive.join("booter.img")));
        assert!(!info.read_only);
        // Its changes go to a delta file in the folder for them, made at
        // the first write; the image isn't copied out.
        assert_eq!(info.overlay, Some(base.join("saves/booter.img.rdelta")));
        assert!(!base.join("saves/booter.img.rdelta").exists());
        let image = disk.bios_image(numbered_drive(0)).unwrap();
        image.write(10, &[0xAB; 512]).unwrap();
        assert!(base.join("saves/booter.img.rdelta").exists());
        assert!(!base.join("saves/booter.img").exists());
        assert_eq!(fs::read(base.join("booter.zip")).unwrap(), zip);
        // Mounted again, the change is there.
        let opts = MountOptions { overlay: Some(base.join("saves")), ..MountOptions::default() };
        disk.mount(numbered_drive(0), &base.join("booter.zip"), opts, true).unwrap();
        let mut sector = [0u8; 512];
        disk.bios_image(numbered_drive(0)).unwrap().read(10, &mut sector).unwrap();
        assert_eq!(sector, [0xAB; 512]);
    }

    #[test]
    fn an_archive_of_a_vhd_mounts_the_vhd() {
        let base = scratch("archive_vhd");
        fs::create_dir_all(base.join("c")).unwrap();
        let vhd = crate::vhd::make_dynamic(8 << 20, 2 << 20);
        let zip = crate::archive::zip::tests::zip(&[("STORE.VHD", &vhd, true)]);
        fs::write(base.join("store.dosz"), &zip).unwrap();
        assert_eq!(archive_hard_disk(&base.join("store.dosz")), Some("STORE.VHD".to_string()));
        let mut disk = DiskController::new(base.join("c"));
        let opts = MountOptions { overlay: Some(base.join("saves")), ..MountOptions::default() };
        disk.mount(numbered_drive(3), &base.join("store.dosz"), opts, false).unwrap();
        let image = disk.bios_image(numbered_drive(3)).unwrap();
        assert_eq!(image.sectors(), (8 << 20) / 512);
        image.write(5000, &[0xCD; 512]).unwrap();
        assert!(base.join("saves/STORE.VHD.rdelta").exists());
        assert_eq!(fs::read(base.join("store.dosz")).unwrap(), zip);
        let mut sector = [0u8; 512];
        image.read(5000, &mut sector).unwrap();
        assert_eq!(sector, [0xCD; 512]);
    }

    /// An image copied out whole into the folder for an archive's changes,
    /// as before delta files, is the one written to.
    #[test]
    fn an_image_copied_out_before_stays_the_one() {
        let base = scratch("archive_image_copied");
        fs::create_dir_all(base.join("c")).unwrap();
        fs::create_dir_all(base.join("saves")).unwrap();
        let zip = crate::archive::zip::tests::zip(&[("booter.img", &vec![0u8; 368_640], true)]);
        fs::write(base.join("booter.zip"), &zip).unwrap();
        fs::write(base.join("saves/booter.img"), vec![0x11u8; 368_640]).unwrap();
        let mut disk = DiskController::new(base.join("c"));
        let opts = MountOptions { overlay: Some(base.join("saves")), ..MountOptions::default() };
        disk.mount(numbered_drive(0), &base.join("booter.zip"), opts, false).unwrap();
        let image = disk.bios_image(numbered_drive(0)).unwrap();
        assert_eq!(image.delta_path(), None);
        image.write(0, &[0x22; 512]).unwrap();
        assert_eq!(fs::read(base.join("saves/booter.img")).unwrap()[..2], [0x22, 0x22]);
    }

    /// With -overlay, a hard disk image is left as it is: one image is
    /// under each game's changes.
    #[test]
    fn an_image_under_an_overlay_keeps_as_it_is() {
        let base = scratch("image_overlay");
        fs::create_dir_all(base.join("c")).unwrap();
        let blank = DiskImage::blank_hard_disk("base.img", 8 << 20, Some("BASE")).unwrap();
        blank.copy_to(&base.join("base.img")).unwrap();
        let before = fs::read(base.join("base.img")).unwrap();
        let mut disk = DiskController::new(base.join("c"));
        for (game, text) in [("one", b"first game".as_slice()), ("two", b"second game")] {
            let opts = MountOptions { overlay: Some(base.join(game)), ..MountOptions::default() };
            disk.mount(3, &base.join("base.img"), opts, true).unwrap();
            let h = disk.create_file(r"D:\GAME.TXT", PSP).unwrap();
            disk.write_file(h, text).unwrap();
            disk.close_file(h);
            assert!(base.join(game).join("base.img.rdelta").exists());
        }
        assert_eq!(fs::read(base.join("base.img")).unwrap(), before, "the image as it was");
        for (game, text) in [("one", b"first game".as_slice()), ("two", b"second game")] {
            let opts = MountOptions { overlay: Some(base.join(game)), ..MountOptions::default() };
            disk.mount(3, &base.join("base.img"), opts, true).unwrap();
            let h = disk.open_file(r"D:\GAME.TXT", 0, PSP).unwrap();
            assert_eq!(disk.read_file(h, 32).unwrap(), text);
            disk.close_file(h);
        }
        disk.mount(3, &base.join("base.img"), MountOptions::default(), true).unwrap();
        assert!(disk.open_file(r"D:\GAME.TXT", 0, PSP).is_err());
    }

    #[test]
    fn names_padded_with_spaces_are_found_and_opened() {
        let base = scratch("pattern_padding");
        fs::write(base.join("LH66.RLE"), b"x").unwrap();
        let mut disk = DiskController::new(base);
        for spec in ["lh66.rle    ", "LH66    .RLE", "LH66.RLEX", "C:\\LH66.RLE  "] {
            let found = disk.list_directory(spec, 0).unwrap();
            assert_eq!(found.len(), 1, "{spec:?}");
            assert_eq!(found[0].filename, "LH66.RLE");
        }
        assert!(disk.list_directory("LH66.R  ", 0).unwrap().is_empty());
        assert!(disk.is_file("lh66.rle    "));
        let h = disk.open_file("C:\\LH66.RLE  ", 0, PSP).unwrap();
        assert_eq!(disk.read_file(h, 1).unwrap(), b"x");
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
    fn file_table_entries_are_reused_and_closed_with_their_last_reference() {
        let base = scratch("handle_reuse");
        fs::write(base.join("F.TXT"), b"1").unwrap();
        let mut disk = DiskController::new(base);
        let first = disk.open_file("F.TXT", 0, PSP).unwrap();
        assert_eq!(first, FIRST_FILE);
        let a = disk.open_file("F.TXT", 0, PSP).unwrap();
        // A second handle for it: the file stays open until both close.
        assert!(disk.add_ref(a));
        assert!(disk.close_file(a));
        assert_eq!(disk.read_file(a, 1).unwrap(), b"1");
        assert!(disk.close_file(a));
        assert!(!disk.close_file(a));
        assert_eq!(disk.open_file("Z:\\COMMAND.COM", 0, PSP), Ok(a));
        assert_eq!(disk.handle_drive(a), Some(DRIVE_Z));

        // The standard devices stay open, whatever closes them.
        assert!(disk.close_file(SFT_CON));
        assert_eq!(disk.handle_device(SFT_CON), Some(CharDevice::Con));

        // A process's FCB files close when it ends; its other files are
        // closed through its handles.
        let child = 0x2000;
        let f = disk.open_file("F.TXT", 0, child).unwrap();
        disk.set_fcb(f);
        assert!(!disk.inheritable(f));
        disk.close_fcb_files(child);
        assert!(!disk.is_open(f));
        assert_eq!(disk.read_file(first, 1).unwrap(), b"1");
        let entry = disk.sft_entry(first).unwrap();
        assert_eq!((&entry.name, entry.refs, entry.size, entry.owner), (b"F       TXT", 1, 1, PSP));
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
    fn a_and_b_are_always_floppies() {
        let base = scratch("floppy_letters");
        for d in ["c", "a", "b", "e"] {
            fs::create_dir_all(base.join(d)).unwrap();
        }
        fs::write(base.join("game.iso"), b"").unwrap();
        let mut disk = DiskController::new(base.join("c"));
        assert_eq!(disk.floppy_units(), 0);

        // B: alone makes two units, the first one empty.
        disk.mount(1, &base.join("b"), MountOptions::default(), false).unwrap();
        assert_eq!(disk.drive_kind(1), Some(DriveKind::Floppy));
        assert_eq!(disk.floppy_units(), 2);
        let hdd = MountOptions { kind: DriveKind::HardDisk, read_only: true, ..Default::default() };
        disk.mount(0, &base.join("a"), hdd.clone(), false).unwrap();
        let info = disk.drive_info(0).unwrap();
        assert_eq!((info.kind, info.read_only), (DriveKind::Floppy, true));
        // The mount stays as asked for, for the configuration file.
        assert_eq!(info.mount.unwrap().opts, hdd);
        disk.unmount(1).unwrap();
        assert_eq!(disk.floppy_units(), 1);

        assert!(disk.mount(1, &base.join("b"), cdrom(), false).unwrap_err().contains("floppy"));
        assert!(disk.mount(1, &base.join("game.iso"), MountOptions::default(), false).is_err());
        assert!(!disk.is_mounted(1));

        // Other letters keep the type they are given.
        disk.mount(4, &base.join("e"), MountOptions::default(), false).unwrap();
        assert_eq!(disk.drive_kind(4), Some(DriveKind::HardDisk));
    }

    #[test]
    fn floppies_have_a_1_44_mb_diskette_layout() {
        let floppy = DriveKind::Floppy.layout();
        assert_eq!((floppy.sectors_per_fat, floppy.first_dir_sector(), floppy.first_data_sector()), (9, 19, 33));
        assert_eq!((floppy.total_sectors(), floppy.cylinders(), floppy.fs_type()), (2880, 80, b"FAT12   "));
        // The BPB of a DOS-formatted 1.44 MB diskette.
        assert_eq!(
            floppy.bpb()[..0x15],
            [0x00, 0x02, 0x01, 0x01, 0x00, 0x02, 0xE0, 0x00, 0x40, 0x0B, 0xF0, 0x09, 0x00, 0x12, 0x00, 0x02, 0x00, 0, 0, 0, 0]
        );
        assert_eq!(floppy.bpb()[0x15..], [0; 10]);

        let hdd = DriveKind::HardDisk.layout();
        assert_eq!((hdd.sectors_per_fat, hdd.fs_type()), (79, b"FAT16   "));
        assert_eq!(hdd.total_sectors(), 191 + 8 * 20000);
        let bpb = hdd.bpb();
        assert_eq!((bpb[0x08], bpb[0x09], bpb[0x0A]), (0, 0, 0xF8));
        assert_eq!(u32::from_le_bytes(bpb[0x15..0x19].try_into().unwrap()), hdd.total_sectors());
    }

    #[test]
    fn disks_mounted_by_number_are_the_bioses_alone() {
        let base = scratch("numbered");
        // A blank hard disk, and floppies without a file system.
        fs::write(base.join("blank.img"), vec![0u8; 16 * 63 * 512 * 4]).unwrap();
        fs::write(base.join("booter1.img"), vec![0u8; 368_640]).unwrap();
        fs::write(base.join("booter2.img"), vec![0u8; 368_640]).unwrap();
        fs::create_dir_all(base.join("dir")).unwrap();
        let mut disk = DiskController::new(base.join("c"));
        let image = |name: &str| base.join(name);
        let list = MountOptions { more_images: vec![image("booter2.img")], ..MountOptions::default() };

        // Lettered drives need a file system; numbered ones take the sectors.
        assert!(disk.mount(3, &image("blank.img"), MountOptions::default(), false).is_err());
        disk.mount(numbered_drive(2), &image("blank.img"), MountOptions::default(), false).unwrap();
        disk.mount(numbered_drive(0), &image("booter1.img"), list, false).unwrap();
        let hdd = disk.bios_image(numbered_drive(2)).unwrap();
        assert_eq!((hdd.is_floppy(), hdd.geometry().heads, hdd.geometry().cylinders), (false, 16, 4));
        assert_eq!(disk.drive_kind(numbered_drive(0)), Some(DriveKind::Floppy));
        assert!(disk.bios_image(numbered_drive(0)).unwrap().is_floppy());

        // DOS doesn't have them; they take images only, and each once.
        assert_eq!(disk.mounted_drives().len(), 2, "C: and Z:");
        assert_eq!(disk.numbered_drives().iter().map(DriveInfo::name).collect::<Vec<_>>(), ["0", "2"]);
        assert!(disk.mount(numbered_drive(3), &base.join("dir"), MountOptions::default(), false).is_err());
        let twice = disk.mount(numbered_drive(3), &image("blank.img"), MountOptions::default(), false);
        assert!(twice.unwrap_err().contains("already mounted as 2"));
        assert!(disk.swap_image(numbered_drive(0)).unwrap().unwrap().starts_with("Drive 0 disk 2 of 2"));
        // Disk control picks any of them, and adds to and takes from them.
        fs::write(base.join("booter3.img"), vec![0u8; 368_640]).unwrap();
        disk.add_image(numbered_drive(0), &image("booter3.img")).unwrap();
        assert!(disk.add_image(numbered_drive(0), &image("booter3.img")).is_err(), "each image once");
        assert_eq!(disk.images(numbered_drive(0)).map(|(list, i)| (list.len(), i)), Some((3, 1)));
        assert!(disk.select_image(numbered_drive(0), 2).unwrap().unwrap().starts_with("Drive 0 disk 3 of 3"));
        assert_eq!(disk.select_image(numbered_drive(0), 2), Ok(None));
        assert!(disk.select_image(numbered_drive(0), 3).is_err());
        assert!(disk.remove_image(numbered_drive(0), 2).is_err(), "it is in");
        disk.remove_image(numbered_drive(0), 0).unwrap();
        assert_eq!(disk.images(numbered_drive(0)).map(|(list, i)| (list.len(), i)), Some((2, 1)));
        assert!(disk.images(DRIVE_C).is_none());

        // Floppy unit 00h is disk 0 before A:; hard disk 80h is disk 2,
        // and the hard disk drives follow in the units left.
        assert_eq!((disk.floppy_units(), disk.floppy_unit(0), disk.floppy_unit(1)), (1, numbered_drive(0), 1));
        assert_eq!(disk.hard_disk_units(|_| true), [numbered_drive(2), DRIVE_C]);
        assert_eq!(disk.hard_disk_units(|d| disk.bios_image(d).is_some()), [numbered_drive(2)]);
        disk.unmount(numbered_drive(2)).unwrap();
        disk.mount(numbered_drive(3), &image("blank.img"), MountOptions::default(), false).unwrap();
        assert_eq!(disk.hard_disk_units(|_| true), [DRIVE_C, numbered_drive(3)]);
        assert_eq!(disk.hard_disk_units(|d| disk.bios_image(d).is_some()), [numbered_drive(3)]);
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
            ..Default::default()
        };
        disk.mount(3, &base.join("d"), opts, false).unwrap();

        let c = disk.find_directory_entry("*.*", 0, 0x08).unwrap();
        assert_eq!((c.filename.as_str(), c.attr), ("RUSTDOS", 0x08));
        let d = disk.find_directory_entry("D:\\*.*", 0, 0x08).unwrap();
        assert_eq!(d.filename, "GAMECD_D.ISK");
        assert_eq!(disk.volume_label(3).unwrap(), "GAMECD_DISK");
    }

    #[test]
    fn images_held_in_memory() {
        let mut disk = DiskController::new(scratch("memimage"));
        // A hard disk made for C:, and files put on it.
        let c = DiskImage::blank_hard_disk("C.IMG", 8 << 20, Some("web")).unwrap();
        disk.mount_disk_image(DRIVE_C, c, MountOptions::default()).unwrap();
        disk.fat_volume(DRIVE_C).unwrap().put_file(&["GAMES", "KEEN", "KEEN1.EXE"], b"MZ", 0, 0x5021).unwrap();
        assert!(disk.is_file("C:\\GAMES\\KEEN\\KEEN1.EXE") && disk.is_directory("C:\\GAMES"));
        let info = disk.drive_info(DRIVE_C).unwrap();
        assert_eq!((info.kind, info.label.as_str(), info.read_only), (DriveKind::HardDisk, "WEB", false));
        assert!(info.mount.is_none() && info.root.is_none());

        // A floppy image's bytes as A:, written through DOS.
        let blank = DiskImage::from_memory("F.IMG", MemoryImage::new(1_474_560), true, None, false).unwrap();
        fat::format(&blank, 0, 2880, None).unwrap();
        let bytes = blank.memory().unwrap().clone();
        disk.mount_memory_image(0, "DISK1.IMA", bytes.clone(), MountOptions::default()).unwrap();
        assert_eq!(disk.drive_kind(0), Some(DriveKind::Floppy));
        disk.bios_image(0).unwrap().take_written();
        let h = disk.create_file("A:\\SAVE.DAT", PSP).unwrap();
        assert_eq!(disk.write_file(h, b"saved"), Ok(5));
        disk.close_file(h);
        assert!(!disk.bios_image(0).unwrap().take_written().is_empty());
        assert!(matches!(disk.file_data("A:\\SAVE.DAT").map(|f| f.read()), Some(Ok(d)) if &d[..] == b"saved"));

        // Not a CD in a floppy drive or as C:, nor junk anywhere.
        let cd = MountOptions { kind: DriveKind::CdRom, ..MountOptions::default() };
        assert!(disk.mount_memory_image(1, "B.ISO", bytes.clone(), cd.clone()).is_err());
        assert!(disk.mount_memory_image(DRIVE_C, "C.ISO", bytes, cd).is_err());
        assert!(disk.mount_memory_image(3, "junk.dat", vec![1; 5000].into(), MountOptions::default()).is_err());
        assert!(!disk.is_mounted(3) && disk.is_file("C:\\GAMES\\KEEN\\KEEN1.EXE"));
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

        // Read-only files, read from where the last read ended.
        let h = disk.open_file("X:PIANO.PAT", 2, PSP).unwrap();
        assert_eq!(disk.handle_drive(h), Some(23));
        assert_eq!(disk.read_file(h, 4).unwrap(), b"0123");
        assert_eq!(disk.read_file(h, 2).unwrap(), b"45");
        assert_eq!(disk.read_file(h, 100).unwrap(), b"6789");
        assert_eq!(disk.read_file(h, 1).unwrap(), b"");
        assert_eq!(disk.seek_file(h, -3, 2), Ok(7));
        assert_eq!(disk.position(h), Some(7));
        assert_eq!(disk.read_file(h, 5).unwrap(), b"789");
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
