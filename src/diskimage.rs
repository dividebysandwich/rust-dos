//! Raw floppy and hard disk images: the sectors of a disk, one after the
//! other, as the BIOS reads them by cylinder, head and sector (INT 13h) and
//! the FAT driver reads them by number.
//!
//! A floppy's geometry comes from the image size, as DOSBox has it; a hard
//! disk's from the mount options, its partition table or boot sector.

use std::cell::{Cell, Ref, RefCell};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::disk::DriveKind;

pub const SECTOR_SIZE: usize = 512;

/// INT 13h status codes.
pub const STATUS_BAD_COMMAND: u8 = 0x01;
pub const STATUS_WRITE_PROTECTED: u8 = 0x03;
pub const STATUS_SECTOR_NOT_FOUND: u8 = 0x04;
pub const STATUS_CONTROLLER_FAILURE: u8 = 0x20;

/// Cylinders, heads and sectors per track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chs {
    pub cylinders: u32,
    pub heads: u32,
    pub sectors: u32,
}

impl Chs {
    pub fn total(&self) -> u64 {
        self.cylinders as u64 * self.heads as u64 * self.sectors as u64
    }
}

/// A floppy format: size in KB, geometry and the BIOS drive type INT 13h
/// AH=08h reports for it (1 = 360K, 2 = 1.2M, 3 = 720K, 4 = 1.44M,
/// 6 = 2.88M).
struct FloppyFormat {
    kb: u64,
    sectors: u32,
    heads: u32,
    cylinders: u32,
    bios_type: u8,
}

const fn format(kb: u64, sectors: u32, heads: u32, cylinders: u32, bios_type: u8) -> FloppyFormat {
    FloppyFormat { kb, sectors, heads, cylinders, bios_type }
}

/// DOSBox's table of floppy image sizes.
const FLOPPY_FORMATS: [FloppyFormat; 14] = [
    format(160, 8, 1, 40, 0),   // SS/DD 5.25"
    format(180, 9, 1, 40, 0),   // SS/DD 5.25"
    format(200, 10, 1, 40, 0),  // SS/DD 5.25" (booters)
    format(320, 8, 2, 40, 1),   // DS/DD 5.25"
    format(360, 9, 2, 40, 1),   // DS/DD 5.25"
    format(400, 10, 2, 40, 1),  // DS/DD 5.25" (booters)
    format(720, 9, 2, 80, 3),   // DS/DD 3.5"
    format(1200, 15, 2, 80, 2), // DS/HD 5.25"
    format(1440, 18, 2, 80, 4), // DS/HD 3.5"
    format(1520, 19, 2, 80, 2), // DS/HD 5.25" (XDF)
    format(1680, 21, 2, 80, 4), // DS/HD 3.5" (DMF)
    format(1720, 21, 2, 82, 4), // DS/HD 3.5" (DMF)
    format(1840, 23, 2, 80, 4), // DS/HD 3.5" (XDF)
    format(2880, 36, 2, 80, 6), // DS/ED 3.5"
];

/// The geometry and BIOS drive type of a floppy image of `bytes` bytes, if
/// it is one of the known sizes (or 1 KB over, as images with a little
/// extra data are).
pub fn floppy_geometry(bytes: u64) -> Option<(Chs, u8)> {
    let kb = bytes / 1024;
    FLOPPY_FORMATS.iter().find(|f| kb == f.kb || kb == f.kb + 1).map(|f| {
        (Chs { cylinders: f.cylinders, heads: f.heads, sectors: f.sectors }, f.bios_type)
    })
}

/// The BIOS drive type of a floppy with a geometry that isn't in the table.
fn floppy_type_of(chs: Chs) -> u8 {
    match chs.sectors {
        s if s >= 36 => 6,
        s if s >= 18 => 4,
        s if s >= 15 => 2,
        _ if chs.cylinders >= 80 => 3,
        _ => 1,
    }
}

/// What an image file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    Cd,
    Floppy,
    HardDisk,
}

/// What kind of image `path` is. `requested` is the drive type the mount
/// asked for: CD-ROM and floppy force theirs, a hard disk (the default)
/// leaves it to the file. Like DOSBox, .iso, .cue and .bin are CDs, .vfd and
/// .flp floppies, and other images are floppies if their size is a floppy's.
/// The rest are hard disks when they start with a partition table or a boot
/// sector, and CDs when they hold an ISO 9660 volume.
pub fn detect(path: &Path, requested: DriveKind) -> Result<ImageKind, String> {
    if let Some(kind) = kind_by_name(path, requested) {
        return Ok(kind);
    }
    let mut file = File::open(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    let mut boot = [0u8; SECTOR_SIZE];
    let boot = file.read_exact(&mut boot).is_ok().then_some(&boot[..]);
    if let Some(kind) = kind_by_contents(len, boot) {
        return Ok(kind);
    }
    if crate::cdrom::image::CdImage::open(path).is_ok() {
        return Ok(ImageKind::Cd);
    }
    Err(format!("{} is not a floppy, hard disk or CD image", path.display()))
}

/// What kind of image `data` is, as `detect` finds for a file named `name`.
pub fn detect_memory(name: &str, data: &MemoryImage, requested: DriveKind) -> Result<ImageKind, String> {
    if let Some(kind) = kind_by_name(Path::new(name), requested) {
        return Ok(kind);
    }
    let mut boot = [0u8; SECTOR_SIZE];
    let boot = data.read_at(0, &mut boot).then_some(&boot[..]);
    if let Some(kind) = kind_by_contents(data.len(), boot) {
        return Ok(kind);
    }
    if crate::cdrom::image::CdImage::probe(data) {
        return Ok(ImageKind::Cd);
    }
    Err(format!("{} is not a floppy, hard disk or CD image", name))
}

/// The kind of image the drive type asked for or the file name's extension
/// says, if they do.
fn kind_by_name(path: &Path, requested: DriveKind) -> Option<ImageKind> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if requested == DriveKind::CdRom || matches!(ext.as_str(), "iso" | "cue" | "bin") {
        return Some(ImageKind::Cd);
    }
    if requested == DriveKind::Floppy || matches!(ext.as_str(), "vfd" | "flp") {
        return Some(ImageKind::Floppy);
    }
    None
}

/// The kind of an image of `len` bytes that starts with `boot`, if its size
/// is a floppy's or it starts with a partition table or boot sector.
fn kind_by_contents(len: u64, boot: Option<&[u8]>) -> Option<ImageKind> {
    if floppy_geometry(len).is_some() {
        return Some(ImageKind::Floppy);
    }
    let boot = boot?;
    (Bpb::parse(boot).is_some() || partitions(boot, len / 512).is_some()).then_some(ImageKind::HardDisk)
}

/// The fields of a FAT boot sector's BIOS parameter block that tell where
/// things are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bpb {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub fats: u8,
    pub root_entries: u16,
    pub total_sectors: u32,
    pub media: u8,
    pub sectors_per_fat: u16,
    pub sectors_per_track: u16,
    pub heads: u16,
    pub hidden_sectors: u32,
}

impl Bpb {
    /// The BPB of a boot sector, if it has a plausible one for a FAT12/16
    /// volume with 512-byte sectors.
    pub fn parse(boot: &[u8]) -> Option<Self> {
        if boot.len() < SECTOR_SIZE || !matches!(boot[0], 0xEB | 0xE9) {
            return None;
        }
        let word = |i: usize| u16::from_le_bytes([boot[i], boot[i + 1]]);
        let dword = |i: usize| u32::from_le_bytes([boot[i], boot[i + 1], boot[i + 2], boot[i + 3]]);
        let small_total = word(0x13);
        let bpb = Bpb {
            bytes_per_sector: word(0x0B),
            sectors_per_cluster: boot[0x0D],
            reserved_sectors: word(0x0E),
            fats: boot[0x10],
            root_entries: word(0x11),
            total_sectors: if small_total != 0 { small_total as u32 } else { dword(0x20) },
            media: boot[0x15],
            sectors_per_fat: word(0x16),
            sectors_per_track: word(0x18),
            heads: word(0x1A),
            hidden_sectors: dword(0x1C),
        };
        let valid = bpb.bytes_per_sector as usize == SECTOR_SIZE
            && bpb.sectors_per_cluster.is_power_of_two()
            && bpb.reserved_sectors >= 1
            && (1..=2).contains(&bpb.fats)
            && bpb.root_entries > 0
            && bpb.sectors_per_fat > 0
            && bpb.total_sectors > 0
            && bpb.media >= 0xF0;
        valid.then_some(bpb)
    }
}

/// A primary partition table entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Partition {
    kind: u8,
    start: u64,
    sectors: u64,
    /// Heads and sectors per track as the partition's ending CHS tells them.
    end_heads: u32,
    end_sectors: u32,
}

/// The partitions of an MBR on a disk of `disk_sectors` sectors, if the
/// sector is one: the 55AAh signature and entries that fit on the disk.
fn partitions(mbr: &[u8], disk_sectors: u64) -> Option<Vec<Partition>> {
    if mbr.len() < SECTOR_SIZE || mbr[510] != 0x55 || mbr[511] != 0xAA {
        return None;
    }
    let mut found = Vec::new();
    for i in 0..4 {
        let e = &mbr[0x1BE + i * 16..0x1BE + (i + 1) * 16];
        let start = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as u64;
        let sectors = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as u64;
        if e[4] == 0 || sectors == 0 {
            continue;
        }
        // The boot flag is 00h or 80h, and the partition lies on the disk
        // (images are often cut a cylinder short).
        if e[0] & 0x7F != 0 || start == 0 || start >= disk_sectors {
            return None;
        }
        found.push(Partition {
            kind: e[4],
            start,
            sectors: sectors.min(disk_sectors - start),
            end_heads: e[5] as u32 + 1,
            end_sectors: (e[6] & 0x3F) as u32,
        });
    }
    (!found.is_empty()).then_some(found)
}

/// Bytes in each piece of an image held in memory.
pub const CHUNK: usize = 64 << 10;

/// The bytes of a disk or CD image held in memory, as the browser build
/// has its drives, in `CHUNK`-sized pieces. Only pieces with something
/// other than zeros take memory, so an empty hard disk takes next to none.
#[derive(Clone, Debug, Default)]
pub struct MemoryImage {
    len: u64,
    chunks: Vec<Option<Box<[u8]>>>,
}

impl MemoryImage {
    /// `len` bytes of zeros.
    pub fn new(len: u64) -> Self {
        Self { len, chunks: vec![None; len.div_ceil(CHUNK as u64) as usize] }
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Pieces in the image, the last one maybe shorter than `CHUNK`.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// The piece `index`, or None if it is zeros.
    pub fn chunk(&self, index: usize) -> Option<&[u8]> {
        self.chunks.get(index)?.as_deref()
    }

    /// Fill `buf` from byte `at` on. False if that runs past the end.
    pub fn read_at(&self, at: u64, buf: &mut [u8]) -> bool {
        if at.checked_add(buf.len() as u64).is_none_or(|end| end > self.len) {
            return false;
        }
        let mut done = 0;
        while done < buf.len() {
            let pos = at as usize + done;
            let (index, offset) = (pos / CHUNK, pos % CHUNK);
            let n = (CHUNK - offset).min(buf.len() - done);
            match &self.chunks[index] {
                Some(chunk) => buf[done..done + n].copy_from_slice(&chunk[offset..offset + n]),
                None => buf[done..done + n].fill(0),
            }
            done += n;
        }
        true
    }

    /// Put `data` at byte `at` on. False if that runs past the end.
    pub fn write_at(&mut self, at: u64, data: &[u8]) -> bool {
        if at.checked_add(data.len() as u64).is_none_or(|end| end > self.len) {
            return false;
        }
        let mut done = 0;
        while done < data.len() {
            let pos = at as usize + done;
            let (index, offset) = (pos / CHUNK, pos % CHUNK);
            let n = (CHUNK - offset).min(data.len() - done);
            let part = &data[done..done + n];
            let chunk_len = (self.len as usize - index * CHUNK).min(CHUNK);
            let chunk = &mut self.chunks[index];
            // Zeros where there are zeros already need no memory.
            if chunk.is_some() || part.iter().any(|&b| b != 0) {
                chunk.get_or_insert_with(|| vec![0; chunk_len].into_boxed_slice())[offset..offset + n].copy_from_slice(part);
            }
            done += n;
        }
        true
    }
}

impl From<Vec<u8>> for MemoryImage {
    fn from(data: Vec<u8>) -> Self {
        let mut image = Self::new(data.len() as u64);
        image.write_at(0, &data);
        image
    }
}

/// Where the sectors of a disk image are.
enum Backing {
    File(File),
    /// In memory, and which `CHUNK`s were written since `take_written`
    /// last looked.
    Memory { data: RefCell<MemoryImage>, written: RefCell<Vec<bool>> },
}

/// A disk image: a file, or held in memory.
pub struct DiskImage {
    path: PathBuf,
    backing: Backing,
    sectors: u64,
    geometry: Chs,
    floppy: bool,
    bios_type: u8,
    writable: bool,
    /// Counts writes, so that what caches the disk's contents can tell
    /// when they changed underneath it.
    generation: Cell<u64>,
}

impl std::fmt::Debug for DiskImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DiskImage({})", self.path.display())
    }
}

impl DiskImage {
    /// Open `path` as a floppy or hard disk image. `geometry` overrides the
    /// detected one. The image is written to unless `read_only` is set or
    /// the file can't be written, which makes the disk write-protected.
    pub fn open(path: &Path, floppy: bool, geometry: Option<Chs>, read_only: bool) -> Result<Self, String> {
        let error = |e: std::io::Error| format!("{}: {}", path.display(), e);
        let (file, writable) = if read_only {
            (File::open(path).map_err(error)?, false)
        } else {
            match OpenOptions::new().read(true).write(true).open(path) {
                Ok(file) => (file, true),
                Err(_) => (File::open(path).map_err(error)?, false),
            }
        };
        let len = file.metadata().map_err(error)?.len();
        Self::new(path, Backing::File(file), len, floppy, geometry, writable)
    }

    /// A disk image held in memory, which `name` names in messages, as
    /// `open` makes of a file with those bytes.
    pub fn from_memory(
        name: &str,
        data: MemoryImage,
        floppy: bool,
        geometry: Option<Chs>,
        read_only: bool,
    ) -> Result<Self, String> {
        let len = data.len();
        let written = RefCell::new(vec![false; data.chunk_count()]);
        let backing = Backing::Memory { data: RefCell::new(data), written };
        Self::new(Path::new(name), backing, len, floppy, geometry, !read_only)
    }

    /// An empty hard disk of `bytes` bytes held in memory: a partition
    /// table with one FAT16 partition over the whole cylinders, formatted
    /// with the volume label `label`.
    pub fn blank_hard_disk(name: &str, bytes: u64, label: Option<&str>) -> Result<Self, String> {
        // Big enough for FAT16 with single-sector clusters.
        if bytes < 4 << 20 {
            return Err("The disk is too small".to_string());
        }
        let disk = Self::from_memory(name, MemoryImage::new(bytes), false, None, false)?;
        let g = disk.geometry;
        let per_cylinder = g.heads as u64 * g.sectors as u64;
        let start = g.sectors as u64;
        let sectors = (g.cylinders as u64 * per_cylinder).min(disk.sectors) - start;
        // One partition from the second track to the end of the last whole
        // cylinder, FAT16 (06h), or FAT16 under 32 MB (04h).
        let chs = |lba: u64| {
            let (c, rest) = (lba / per_cylinder, lba % per_cylinder);
            let (h, s) = (rest / g.sectors as u64, rest % g.sectors as u64 + 1);
            let c = c.min(1023);
            [h as u8, (s as u8) | ((c >> 8) as u8) << 6, c as u8]
        };
        let mut mbr = [0u8; SECTOR_SIZE];
        let entry = &mut mbr[0x1BE..0x1CE];
        entry[0] = 0x80;
        entry[1..4].copy_from_slice(&chs(start));
        entry[4] = if sectors < 0x10000 { 0x04 } else { 0x06 };
        entry[5..8].copy_from_slice(&chs(start + sectors - 1));
        entry[8..12].copy_from_slice(&(start as u32).to_le_bytes());
        entry[12..16].copy_from_slice(&(sectors as u32).to_le_bytes());
        mbr[510] = 0x55;
        mbr[511] = 0xAA;
        disk.write(0, &mbr).map_err(|_| "Can't write the disk".to_string())?;
        crate::fat::format(&disk, start, sectors, label)?;
        Ok(disk)
    }

    fn new(path: &Path, backing: Backing, len: u64, floppy: bool, geometry: Option<Chs>, writable: bool) -> Result<Self, String> {
        let sectors = len / SECTOR_SIZE as u64;
        if sectors == 0 {
            return Err(format!("{} is empty", path.display()));
        }
        let mut disk = Self {
            path: path.to_path_buf(),
            backing,
            sectors,
            geometry: Chs { cylinders: 0, heads: 0, sectors: 0 },
            floppy,
            bios_type: 0,
            writable,
            generation: Cell::new(0),
        };
        let (geometry, bios_type) = if floppy {
            match geometry {
                Some(chs) => (chs, floppy_type_of(chs)),
                None => floppy_geometry(len)
                    .ok_or_else(|| format!("{} is not the size of a floppy disk image", path.display()))?,
            }
        } else {
            let mut boot = [0u8; SECTOR_SIZE];
            disk.read_at(0, &mut boot).map_err(|e| format!("{}: {}", path.display(), e))?;
            (geometry.unwrap_or_else(|| hard_disk_geometry(&boot, sectors)), 0)
        };
        if geometry.heads == 0 || geometry.sectors == 0 || geometry.cylinders == 0 {
            return Err("Invalid disk geometry".to_string());
        }
        disk.geometry = geometry;
        disk.bios_type = bios_type;
        Ok(disk)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_floppy(&self) -> bool {
        self.floppy
    }

    pub fn geometry(&self) -> Chs {
        self.geometry
    }

    /// Sectors in the file.
    pub fn sectors(&self) -> u64 {
        self.sectors
    }

    /// The BIOS drive type of a floppy (0 for a hard disk).
    pub fn bios_type(&self) -> u8 {
        self.bios_type
    }

    pub fn writable(&self) -> bool {
        self.writable
    }

    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// The bytes of an image held in memory.
    pub fn memory(&self) -> Option<Ref<'_, MemoryImage>> {
        match &self.backing {
            Backing::Memory { data, .. } => Some(data.borrow()),
            Backing::File(_) => None,
        }
    }

    /// The `CHUNK`s of an image held in memory written since the last
    /// call, by number, for keeping a copy of the image up to date.
    pub fn take_written(&self) -> Vec<usize> {
        match &self.backing {
            Backing::Memory { written, .. } => written
                .borrow_mut()
                .iter_mut()
                .enumerate()
                .filter_map(|(i, w)| std::mem::take(w).then_some(i))
                .collect(),
            Backing::File(_) => Vec::new(),
        }
    }

    /// The sector number of cylinder `c`, head `h`, sector `s` (from 1), if
    /// that is a sector of the geometry. Multi-sector transfers run on from
    /// there across heads and cylinders.
    pub fn chs_to_lba(&self, c: u32, h: u32, s: u32) -> Option<u64> {
        let g = self.geometry;
        if s == 0 || s > g.sectors || h >= g.heads {
            return None;
        }
        Some((c as u64 * g.heads as u64 + h as u64) * g.sectors as u64 + s as u64 - 1)
    }

    fn check(&self, lba: u64, len: usize) -> Result<(), u8> {
        let count = len.div_ceil(SECTOR_SIZE) as u64;
        if lba.checked_add(count).is_none_or(|end| end > self.sectors) {
            return Err(STATUS_SECTOR_NOT_FOUND);
        }
        Ok(())
    }

    /// Read `buf.len()` bytes from sector `lba` on.
    pub fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), u8> {
        self.check(lba, buf.len())?;
        self.read_at(lba * SECTOR_SIZE as u64, buf).map_err(|_| STATUS_CONTROLLER_FAILURE)
    }

    fn read_at(&self, at: u64, buf: &mut [u8]) -> std::io::Result<()> {
        match &self.backing {
            Backing::File(file) => {
                let mut file = file;
                file.seek(SeekFrom::Start(at)).and_then(|_| file.read_exact(buf))
            }
            Backing::Memory { data, .. } => match data.borrow().read_at(at, buf) {
                true => Ok(()),
                false => Err(std::io::ErrorKind::UnexpectedEof.into()),
            },
        }
    }

    /// Write `data` from sector `lba` on.
    pub fn write(&self, lba: u64, data: &[u8]) -> Result<(), u8> {
        if !self.writable {
            return Err(STATUS_WRITE_PROTECTED);
        }
        self.check(lba, data.len())?;
        self.generation.set(self.generation.get() + 1);
        let at = lba * SECTOR_SIZE as u64;
        match &self.backing {
            Backing::File(file) => {
                let mut file = file;
                file.seek(SeekFrom::Start(at))
                    .and_then(|_| file.write_all(data))
                    .map_err(|_| STATUS_CONTROLLER_FAILURE)
            }
            Backing::Memory { data: image, written } => {
                if !image.borrow_mut().write_at(at, data) {
                    return Err(STATUS_SECTOR_NOT_FOUND);
                }
                if !data.is_empty() {
                    let at = at as usize;
                    written.borrow_mut()[at / CHUNK..=(at + data.len() - 1) / CHUNK].fill(true);
                }
                Ok(())
            }
        }
    }

    /// Where the FAT volume on the disk is: (first sector, sectors). A
    /// floppy is one volume; a hard disk is a volume from its boot sector
    /// on, or has one in its first FAT12/16 primary partition.
    pub fn fat_volume(&self) -> Result<(u64, u64), String> {
        let mut boot = [0u8; SECTOR_SIZE];
        self.read(0, &mut boot).map_err(|_| "Can't read the boot sector".to_string())?;
        if self.floppy || Bpb::parse(&boot).is_some() {
            return Ok((0, self.sectors));
        }
        let parts = partitions(&boot, self.sectors).ok_or("No partition table or boot sector")?;
        if let Some(p) = parts.iter().find(|p| matches!(p.kind, 0x01 | 0x04 | 0x06 | 0x0E)) {
            return Ok((p.start, p.sectors));
        }
        if parts.iter().any(|p| matches!(p.kind, 0x0B | 0x0C)) {
            return Err("FAT32 partitions aren't supported".to_string());
        }
        Err("No FAT12 or FAT16 partition".to_string())
    }
}

/// A hard disk's geometry from its first sector: the heads and sectors
/// per track its partitions end on, or its boot sector's, or 16 heads of
/// 63 sectors. The cylinders are what the image holds.
fn hard_disk_geometry(boot: &[u8], sectors: u64) -> Chs {
    let (heads, per_track) = match (Bpb::parse(boot), partitions(boot, sectors)) {
        (Some(bpb), _) if bpb.heads > 0 && (1..=63).contains(&bpb.sectors_per_track) => {
            (bpb.heads as u32, bpb.sectors_per_track as u32)
        }
        (_, Some(parts)) => parts
            .iter()
            .find(|p| (1..=63).contains(&p.end_sectors) && p.end_heads > 1)
            .map_or((16, 63), |p| (p.end_heads, p.end_sectors)),
        _ => (16, 63),
    };
    let cylinders = (sectors / (heads as u64 * per_track as u64)).clamp(1, u32::MAX as u64) as u32;
    Chs { cylinders, heads, sectors: per_track }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rust-dos-diskimage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn boot_sector(total: u16, spt: u16, heads: u16) -> Vec<u8> {
        let mut boot = vec![0u8; SECTOR_SIZE];
        boot[0] = 0xEB;
        boot[0x0B..0x0D].copy_from_slice(&512u16.to_le_bytes());
        boot[0x0D] = 1;
        boot[0x0E] = 1;
        boot[0x10] = 2;
        boot[0x11..0x13].copy_from_slice(&224u16.to_le_bytes());
        boot[0x13..0x15].copy_from_slice(&total.to_le_bytes());
        boot[0x15] = 0xF0;
        boot[0x16] = 9;
        boot[0x18..0x1A].copy_from_slice(&spt.to_le_bytes());
        boot[0x1A..0x1C].copy_from_slice(&heads.to_le_bytes());
        boot[510] = 0x55;
        boot[511] = 0xAA;
        boot
    }

    #[test]
    fn floppy_sizes() {
        let (chs, kind) = floppy_geometry(1_474_560).unwrap();
        assert_eq!((chs.cylinders, chs.heads, chs.sectors, kind), (80, 2, 18, 4));
        assert_eq!(floppy_geometry(737_280).unwrap().1, 3);
        assert_eq!(floppy_geometry(368_640).unwrap().0.sectors, 9);
        // 1 KB of extra data is fine, more isn't.
        assert!(floppy_geometry(1_474_560 + 1024).is_some());
        assert!(floppy_geometry(1_474_560 + 4096).is_none());
        assert!(floppy_geometry(10 * 1024 * 1024).is_none());
    }

    #[test]
    fn chs_numbering() {
        let path = scratch("chs.img", &vec![0u8; 737_280]);
        let disk = DiskImage::open(&path, true, None, false).unwrap();
        assert_eq!(disk.geometry(), Chs { cylinders: 80, heads: 2, sectors: 9 });
        assert_eq!(disk.chs_to_lba(0, 0, 1), Some(0));
        assert_eq!(disk.chs_to_lba(0, 1, 1), Some(9));
        assert_eq!(disk.chs_to_lba(1, 0, 3), Some(20));
        assert_eq!(disk.chs_to_lba(0, 0, 0), None);
        assert_eq!(disk.chs_to_lba(0, 0, 10), None);
        assert_eq!(disk.chs_to_lba(0, 2, 1), None);
    }

    #[test]
    fn reads_and_writes_sectors() {
        let path = scratch("rw.img", &vec![0u8; 368_640]);
        let disk = DiskImage::open(&path, true, None, false).unwrap();
        let generation = disk.generation();
        disk.write(3, &[0xAB; 1024]).unwrap();
        assert!(disk.generation() > generation);
        let mut buf = [0u8; 512];
        disk.read(4, &mut buf).unwrap();
        assert_eq!(buf, [0xAB; 512]);
        assert_eq!(disk.read(720, &mut buf), Err(STATUS_SECTOR_NOT_FOUND));
        assert_eq!(disk.write(719, &[0; 1024]), Err(STATUS_SECTOR_NOT_FOUND));
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[3 * 512..5 * 512], &[0xAB; 1024][..]);

        let ro = DiskImage::open(&path, true, None, true).unwrap();
        assert!(!ro.writable());
        assert_eq!(ro.write(0, &[0; 512]), Err(STATUS_WRITE_PROTECTED));
    }

    #[test]
    fn disks_held_in_memory() {
        let disk = DiskImage::blank_hard_disk("C.IMG", 8 << 20, Some("RUSTDOS")).unwrap();
        assert!(!disk.is_floppy() && disk.writable());
        // Only the partition table, the boot sector, the FATs and the root
        // directory's label take memory: the first two pieces.
        let used = |disk: &DiskImage| {
            let memory = disk.memory().unwrap();
            (0..memory.chunk_count()).filter(|&i| memory.chunk(i).is_some()).count()
        };
        assert_eq!(used(&disk), 2);
        let g = disk.geometry();
        assert_eq!((g.heads, g.sectors), (16, 63));
        // One partition from the second track to the last whole cylinder.
        let (start, sectors) = disk.fat_volume().unwrap();
        assert_eq!((start, start + sectors), (63, g.cylinders as u64 * 16 * 63));
        let kind = detect_memory("C.IMG", &disk.memory().unwrap(), DriveKind::HardDisk);
        assert_eq!(kind, Ok(ImageKind::HardDisk));
        assert!(DiskImage::blank_hard_disk("C.IMG", 1 << 20, None).is_err());

        // Formatting wrote the partition table; writes are counted once.
        assert_eq!(disk.take_written().first(), Some(&0));
        assert!(disk.take_written().is_empty());
        disk.write(300, &[0xAB; 1024]).unwrap();
        assert_eq!(disk.take_written(), [300 * SECTOR_SIZE / CHUNK]);
        assert_eq!(used(&disk), 3);
        // Zeros where there are zeros take no memory.
        disk.write(disk.sectors() - 2, &[0; 1024]).unwrap();
        disk.write(299, &[0; 1024]).unwrap();
        assert_eq!(used(&disk), 3);
        let mut buf = [0u8; 512];
        disk.read(301, &mut buf).unwrap();
        assert_eq!(buf, [0xAB; 512]);
        assert_eq!(disk.read(disk.sectors(), &mut buf), Err(STATUS_SECTOR_NOT_FOUND));

        let floppy = DiskImage::from_memory("A.IMG", MemoryImage::new(737_280), true, None, true).unwrap();
        assert_eq!(floppy.geometry(), Chs { cylinders: 80, heads: 2, sectors: 9 });
        assert_eq!(floppy.write(0, &[0; 512]), Err(STATUS_WRITE_PROTECTED));
        assert_eq!(detect_memory("A.IMG", &floppy.memory().unwrap(), DriveKind::HardDisk), Ok(ImageKind::Floppy));
        assert!(detect_memory("junk.dat", &vec![1; 5000].into(), DriveKind::HardDisk).is_err());

        // Pieces cut across, and a short last one.
        let mut image = MemoryImage::new(CHUNK as u64 * 2 + 100);
        assert!(image.write_at(CHUNK as u64 - 2, &[1, 2, 3, 4]));
        assert!(image.write_at(CHUNK as u64 * 2 + 98, &[5, 6]));
        assert!(!image.write_at(CHUNK as u64 * 2 + 99, &[7, 8]));
        let mut buf = [9u8; 6];
        assert!(image.read_at(CHUNK as u64 - 3, &mut buf));
        assert_eq!(buf, [0, 1, 2, 3, 4, 0]);
        assert_eq!(image.chunk(2).map(<[u8]>::len), Some(100));
        assert!(!image.read_at(CHUNK as u64 * 2 + 99, &mut buf));
    }

    #[test]
    fn detects_the_kind_of_image() {
        let floppy = scratch("f.img", &vec![0u8; 1_474_560]);
        assert_eq!(detect(&floppy, DriveKind::HardDisk), Ok(ImageKind::Floppy));
        assert_eq!(detect(&floppy, DriveKind::CdRom), Ok(ImageKind::Cd));
        assert_eq!(detect(Path::new("x.iso"), DriveKind::HardDisk), Ok(ImageKind::Cd));
        assert_eq!(detect(Path::new("x.flp"), DriveKind::HardDisk), Ok(ImageKind::Floppy));

        let mut disk = vec![0u8; 16 * 63 * 512 * 4];
        disk[..512].copy_from_slice(&boot_sector(4032, 63, 16));
        let bare = scratch("bare.img", &disk);
        assert_eq!(detect(&bare, DriveKind::HardDisk), Ok(ImageKind::HardDisk));

        let junk = scratch("junk.img", &vec![0x11u8; 100_000]);
        assert!(detect(&junk, DriveKind::HardDisk).is_err());
    }

    #[test]
    fn hard_disks_find_their_fat_partition() {
        // 4 cylinders of 4 heads x 17 sectors, one FAT16 partition from
        // sector 17 ending on cylinder 3, head 3, sector 17.
        let sectors = 4 * 4 * 17;
        let mut disk = vec![0u8; sectors * 512];
        let e = 0x1BE;
        disk[e] = 0x80;
        disk[e + 4] = 0x06;
        disk[e + 5] = 3;
        disk[e + 6] = 17;
        disk[e + 7] = 3;
        disk[e + 8..e + 12].copy_from_slice(&17u32.to_le_bytes());
        disk[e + 12..e + 16].copy_from_slice(&((sectors - 17) as u32).to_le_bytes());
        disk[510] = 0x55;
        disk[511] = 0xAA;
        let path = scratch("mbr.img", &disk);
        assert_eq!(detect(&path, DriveKind::HardDisk), Ok(ImageKind::HardDisk));
        let image = DiskImage::open(&path, false, None, false).unwrap();
        assert_eq!(image.geometry(), Chs { cylinders: 4, heads: 4, sectors: 17 });
        assert_eq!(image.fat_volume(), Ok((17, sectors as u64 - 17)));

        let forced = Chs { cylinders: 2, heads: 8, sectors: 17 };
        assert_eq!(DiskImage::open(&path, false, Some(forced), false).unwrap().geometry(), forced);

        disk[e + 4] = 0x0C;
        let fat32 = scratch("fat32.img", &disk);
        assert!(DiskImage::open(&fat32, false, None, false).unwrap().fat_volume().unwrap_err().contains("FAT32"));
    }
}
