//! FAT12, FAT16 and FAT32 file systems on floppy and hard disk images, for
//! the drives mounted from them.
//!
//! A volume keeps only its boot sector's parameters and the parts of its
//! FAT it has read in memory and reads directories and files from the
//! image every time. Any
//! write to the image that the volume didn't make itself (INT 13h, INT 26h)
//! makes it read the boot sector and the FAT again, so DOS always sees what
//! is on the disk.
//!
//! Paths are the components of a DOS path from the root ("GAMES", "DOOM"),
//! with "." and ".." already folded away.

use std::cell::{RefCell, RefMut};
use std::collections::BTreeSet;
use std::rc::Rc;

use chrono::{Datelike, Timelike};

use crate::disk::FatLayout;
use crate::diskimage::{Bpb, DiskImage, SECTOR_SIZE, STATUS_SECTOR_NOT_FOUND, STATUS_WRITE_PROTECTED};

pub const ATTR_READ_ONLY: u8 = 0x01;
pub const ATTR_HIDDEN: u8 = 0x02;
pub const ATTR_SYSTEM: u8 = 0x04;
pub const ATTR_VOLUME: u8 = 0x08;
pub const ATTR_DIRECTORY: u8 = 0x10;
pub const ATTR_ARCHIVE: u8 = 0x20;
/// The attributes of a long file name entry.
const ATTR_LONG_NAME: u8 = 0x0F;
/// The attributes a program can set (INT 21h AX=4301h).
const ATTR_CHANGEABLE: u8 = ATTR_READ_ONLY | ATTR_HIDDEN | ATTR_SYSTEM | ATTR_ARCHIVE;

const DELETED: u8 = 0xE5;
const ENTRY_SIZE: usize = 32;
const ENTRIES_PER_SECTOR: usize = SECTOR_SIZE / ENTRY_SIZE;

// DOS error codes.
const FILE_NOT_FOUND: u8 = 0x02;
const PATH_NOT_FOUND: u8 = 0x03;
const ACCESS_DENIED: u8 = 0x05;
const WRITE_FAULT: u8 = 0x1D;
const DISK_FULL: u8 = 0x27;
const READ_FAULT: u8 = 0x1E;

/// Volumes with fewer clusters than this have 12-bit FATs, and with fewer
/// than `FAT16_MAX_CLUSTERS` 16-bit ones.
const FAT12_MAX_CLUSTERS: u64 = 4085;
const FAT16_MAX_CLUSTERS: u64 = 65525;

/// The current local time as a directory entry has it: (time, date).
pub fn dos_now() -> (u16, u16) {
    let t = crate::hosttime::now();
    let time = (t.hour() << 11 | t.minute() << 5 | (t.second() / 2)) as u16;
    let date = (((t.year().clamp(1980, 2107) - 1980) as u32) << 9 | t.month() << 5 | t.day()) as u16;
    (time, date)
}

/// Where a directory entry is: a sector of the volume and the entry's place
/// in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntryRef {
    sector: u64,
    index: usize,
}

/// A file or directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// "NAME.EXT", or "NAME" without an extension.
    pub name: String,
    pub attr: u8,
    pub time: u16,
    pub date: u16,
    pub cluster: u32,
    pub size: u32,
    /// Where the entry is; None for the root directory.
    pub at: Option<EntryRef>,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.attr & ATTR_DIRECTORY != 0
    }

    fn root() -> Self {
        Entry { name: String::new(), attr: ATTR_DIRECTORY, time: 0, date: 0, cluster: 0, size: 0, at: None }
    }

    fn parse(raw: &[u8], at: EntryRef) -> Self {
        let word = |i: usize| u16::from_le_bytes([raw[i], raw[i + 1]]);
        Entry {
            name: display_name(&raw[..11]),
            attr: raw[11],
            time: word(22),
            date: word(24),
            cluster: word(26) as u32,
            size: u32::from_le_bytes([raw[28], raw[29], raw[30], raw[31]]),
            at: Some(at),
        }
    }
}

/// The name of an 11-byte directory entry name, as DOS shows it.
fn display_name(raw: &[u8]) -> String {
    let text = |bytes: &[u8]| -> String {
        let mut s: String = bytes.iter().map(|&b| b as char).collect();
        s.truncate(s.trim_end_matches(' ').len());
        s
    };
    let mut raw: [u8; 11] = raw.try_into().unwrap();
    if raw[0] == 0x05 {
        raw[0] = DELETED;
    }
    let (stem, ext) = (text(&raw[..8]), text(&raw[8..]));
    if ext.is_empty() { stem } else { format!("{}.{}", stem, ext) }
}

/// A name the way a directory entry holds it: 8 and 3 characters in upper
/// case, padded with spaces. Longer parts are cut, as DOS does. None for a
/// name with characters that can't be in a DOS file name.
fn short_name(name: &str) -> Option<[u8; 11]> {
    match name {
        "." => return Some(*b".          "),
        ".." => return Some(*b"..         "),
        _ => {}
    }
    let (stem, ext) = name.split_once('.').unwrap_or((name, ""));
    if stem.is_empty() || ext.contains('.') {
        return None;
    }
    let byte = |c: char| {
        let c = c.to_ascii_uppercase();
        let allowed = c.is_ascii_alphanumeric() || "!#$%&'()-@^_`{}~".contains(c) || ('\u{80}'..='\u{FF}').contains(&c);
        allowed.then_some(c as u8)
    };
    let mut raw = [b' '; 11];
    for (i, c) in stem.chars().take(8).enumerate() {
        raw[i] = byte(c)?;
    }
    for (i, c) in ext.chars().take(3).enumerate() {
        raw[8 + i] = byte(c)?;
    }
    if raw[0] == DELETED {
        raw[0] = 0x05;
    }
    Some(raw)
}

/// A name the way DOS shows the file it names: upper case, and cut to 8.3.
pub fn canonical_name(name: &str) -> Option<String> {
    short_name(name).map(|raw| display_name(&raw))
}

/// Whether a directory entry's name is `want` (a `short_name`), in any
/// case.
fn same_name(raw: &[u8], want: &[u8; 11]) -> bool {
    raw[..11].iter().zip(want).all(|(a, b)| a.to_ascii_uppercase() == *b)
}

/// The width of a volume's FAT entries, which its cluster count decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FatType {
    Fat12,
    Fat16,
    Fat32,
}

/// Where the parts of a volume are, from its BPB.
#[derive(Clone, Copy, Debug)]
struct Params {
    bpb: Bpb,
    kind: FatType,
    /// The fixed root directory of FAT12 and FAT16; FAT32's is a chain of
    /// clusters from `root_cluster` on.
    root_start: u64,
    root_sectors: u64,
    root_cluster: u32,
    data_start: u64,
    /// Data clusters on the volume: cluster numbers 2 to `clusters + 1`.
    clusters: u32,
    cluster_bytes: u64,
    /// The FAT in use, and whether the others are kept the same (FAT32's
    /// ExtFlags can turn the mirroring off).
    active_fat: u8,
    mirrored: bool,
}

impl Params {
    fn new(bpb: Bpb, volume_sectors: u64) -> Result<Self, String> {
        let per_cluster = bpb.sectors_per_cluster as u64;
        let root_sectors = (bpb.root_entries as u64 * ENTRY_SIZE as u64).div_ceil(SECTOR_SIZE as u64);
        let root_start = bpb.reserved_sectors as u64 + bpb.fats as u64 * bpb.sectors_per_fat as u64;
        let data_start = root_start + root_sectors;
        let total = bpb.total_sectors as u64;
        if data_start >= total {
            return Err("Invalid FAT file system".to_string());
        }
        // The cluster count by the BPB decides the FAT type; those that
        // lie past the end of an image cut short can't be used. A FAT32
        // BPB is FAT32 whatever the count.
        let kind = match (total - data_start) / per_cluster {
            _ if bpb.fat32.is_some() => FatType::Fat32,
            n if n < FAT12_MAX_CLUSTERS => FatType::Fat12,
            n if n < FAT16_MAX_CLUSTERS => FatType::Fat16,
            _ => return Err("Invalid FAT file system".to_string()),
        };
        let fat_bytes = bpb.sectors_per_fat as u64 * SECTOR_SIZE as u64;
        let fat_entries = match kind {
            FatType::Fat12 => fat_bytes * 2 / 3,
            FatType::Fat16 => fat_bytes / 2,
            FatType::Fat32 => (fat_bytes / 4).min(FAT32_MAX_CLUSTERS + 2),
        };
        let clusters = ((total.min(volume_sectors).saturating_sub(data_start)) / per_cluster)
            .min(fat_entries.saturating_sub(2)) as u32;
        let (active_fat, mirrored) = match bpb.fat32 {
            Some(f) if f.ext_flags & 0x80 != 0 && ((f.ext_flags & 0x0F) as u8) < bpb.fats => {
                ((f.ext_flags & 0x0F) as u8, false)
            }
            _ => (0, true),
        };
        let root_cluster = bpb.fat32.map_or(0, |f| f.root_cluster);
        if kind == FatType::Fat32 && !(2..=clusters as u64 + 1).contains(&(root_cluster as u64)) {
            return Err("Invalid FAT file system".to_string());
        }
        Ok(Params {
            bpb,
            kind,
            root_start,
            root_sectors,
            root_cluster,
            data_start,
            clusters,
            cluster_bytes: per_cluster * SECTOR_SIZE as u64,
            active_fat,
            mirrored,
        })
    }

    fn cluster_sector(&self, cluster: u32) -> u64 {
        self.data_start + (cluster as u64 - 2) * self.bpb.sectors_per_cluster as u64
    }

    fn fat32(&self) -> bool {
        self.kind == FatType::Fat32
    }

    /// The directory entry `raw` at `at`: FAT32's has the high word of its
    /// first cluster at 14h, where FAT12 and FAT16 have OS/2's extended
    /// attributes.
    fn entry(&self, raw: &[u8], at: EntryRef) -> Entry {
        let mut entry = Entry::parse(raw, at);
        if self.fat32() {
            entry.cluster |= (u16::from_le_bytes([raw[20], raw[21]]) as u32) << 16;
        }
        entry
    }

    /// Put `cluster` into the directory entry `raw` as the file's first.
    fn set_cluster(&self, raw: &mut [u8], cluster: u32) {
        raw[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
        if self.fat32() {
            raw[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
        }
    }

    /// Where the boot sector's extended BPB (signature 29h, then the
    /// serial number and the label) is.
    fn extended_bpb(&self) -> usize {
        if self.fat32() { 0x42 } else { 0x26 }
    }
}

/// Bytes of the FAT read at a time.
const FAT_CHUNK: usize = 4096;
/// The most clusters a FAT32 volume has: 28-bit cluster numbers up to
/// 0FFFFFF6h, below the bad cluster and end of chain marks.
const FAT32_MAX_CLUSTERS: u64 = 0x0FFF_FFF5;
/// FAT32's entries have 28 bits; the top 4 are reserved and kept.
const FAT32_MASK: u32 = 0x0FFF_FFFF;

/// The FSInfo sector's signatures, and where its free cluster count and
/// the next free cluster hint are.
const FSINFO_LEAD: u32 = 0x4161_5252;
const FSINFO_STRUCT: u32 = 0x6141_7272;
const FSINFO_TRAIL: u32 = 0xAA55_0000;
const FSINFO_FREE: usize = 488;
const FSINFO_NEXT: usize = 492;

/// What the volume keeps in memory.
struct State {
    params: Params,
    disk: Rc<DiskImage>,
    /// The FAT in use's first sector on the disk, and its size.
    fat_at: u64,
    fat_len: usize,
    /// The FAT in use, `FAT_CHUNK` bytes at a time as they're needed: a
    /// big FAT32 volume's FAT is many megabytes.
    chunks: RefCell<Vec<Option<Box<[u8]>>>>,
    /// FAT sectors changed in memory and not yet written to the image.
    dirty: BTreeSet<u64>,
    /// The image's write count the FAT was read at, or last written at.
    generation: u64,
    /// Where to look for a free cluster first.
    next_free: u32,
    /// The free clusters, once counted.
    free: Option<u32>,
    /// FAT32's FSInfo sector, if it has a valid one.
    fsinfo: Option<u64>,
}

impl State {
    fn max_cluster(&self) -> u32 {
        self.params.clusters + 1
    }

    /// The FAT's `N` bytes from `at`, read from the disk as needed; None
    /// if it can't be read.
    fn fat_bytes<const N: usize>(&self, at: usize) -> Option<[u8; N]> {
        if at + N > self.fat_len {
            return Some([0; N]);
        }
        let mut chunks = self.chunks.borrow_mut();
        let mut out = [0u8; N];
        for (i, byte) in out.iter_mut().enumerate() {
            let pos = at + i;
            let chunk = &mut chunks[pos / FAT_CHUNK];
            if chunk.is_none() {
                let first = pos / FAT_CHUNK * FAT_CHUNK;
                let mut data = vec![0u8; FAT_CHUNK.min(self.fat_len - first)];
                self.disk.read(self.fat_at + (first / SECTOR_SIZE) as u64, &mut data).ok()?;
                *chunk = Some(data.into_boxed_slice());
            }
            *byte = chunk.as_ref().unwrap()[pos % FAT_CHUNK];
        }
        Some(out)
    }

    /// Change the FAT's bytes from `at`, which `fat_bytes` has read.
    fn put_fat_bytes(&mut self, at: usize, data: &[u8]) {
        let chunks = self.chunks.get_mut();
        for (i, &byte) in data.iter().enumerate() {
            let pos = at + i;
            if let Some(chunk) = &mut chunks[pos / FAT_CHUNK] {
                chunk[pos % FAT_CHUNK] = byte;
            }
        }
        self.dirty.insert((at / SECTOR_SIZE) as u64);
        self.dirty.insert(((at + data.len() - 1) / SECTOR_SIZE) as u64);
    }

    /// A bad cluster's mark, which is what a FAT that can't be read has.
    fn bad(&self) -> u32 {
        self.end_of_chain() - 8
    }

    fn get(&self, n: u32) -> u32 {
        let n = n as usize;
        match self.params.kind {
            FatType::Fat12 => self.fat_bytes::<2>(n * 3 / 2).map_or(self.bad(), |b| {
                let pair = u16::from_le_bytes(b) as u32;
                if n & 1 == 1 { pair >> 4 } else { pair & 0xFFF }
            }),
            FatType::Fat16 => self.fat_bytes::<2>(n * 2).map_or(self.bad(), |b| u16::from_le_bytes(b) as u32),
            FatType::Fat32 => self.fat_bytes::<4>(n * 4).map_or(self.bad(), |b| u32::from_le_bytes(b) & FAT32_MASK),
        }
    }

    fn set(&mut self, n: u32, value: u32) {
        let old = self.get(n);
        let at = n as usize;
        match self.params.kind {
            FatType::Fat16 => {
                if at * 2 + 2 > self.fat_len {
                    return;
                }
                self.put_fat_bytes(at * 2, &(value as u16).to_le_bytes());
            }
            FatType::Fat32 => {
                let Some(raw) = self.fat_bytes::<4>(at * 4).filter(|_| at * 4 + 4 <= self.fat_len) else { return };
                let kept = u32::from_le_bytes(raw) & !FAT32_MASK;
                self.put_fat_bytes(at * 4, &(kept | (value & FAT32_MASK)).to_le_bytes());
            }
            FatType::Fat12 => {
                let at = at * 3 / 2;
                let Some([lo, hi]) = self.fat_bytes::<2>(at).filter(|_| at + 2 <= self.fat_len) else { return };
                let pair = if n & 1 == 1 {
                    [(lo & 0x0F) | (value << 4) as u8, (value >> 4) as u8]
                } else {
                    [value as u8, (hi & 0xF0) | ((value >> 8) & 0x0F) as u8]
                };
                self.put_fat_bytes(at, &pair);
            }
        }
        if let Some(free) = &mut self.free {
            match (old == 0, value == 0) {
                (true, false) => *free = free.saturating_sub(1),
                (false, true) => *free += 1,
                _ => {}
            }
        }
    }

    fn end_of_chain(&self) -> u32 {
        match self.params.kind {
            FatType::Fat12 => 0xFFF,
            FatType::Fat16 => 0xFFFF,
            FatType::Fat32 => FAT32_MASK,
        }
    }

    /// The cluster after `cluster` in its chain: None at the end of the
    /// chain, or where the FAT is damaged.
    fn next(&self, cluster: u32) -> Option<u32> {
        let next = self.get(cluster);
        (2..=self.max_cluster()).contains(&next).then_some(next)
    }

    /// The clusters of the chain that starts at `first`.
    fn chain(&self, first: u32) -> Vec<u32> {
        let mut chain = Vec::new();
        let mut cluster = Some(first).filter(|c| (2..=self.max_cluster()).contains(c));
        while let Some(c) = cluster {
            // A FAT that loops back gives out after every cluster once.
            if chain.len() > self.params.clusters as usize {
                break;
            }
            chain.push(c);
            cluster = self.next(c);
        }
        chain
    }

    /// The first cluster of the directory `dir`: FAT32's root directory's
    /// for the root.
    fn dir_cluster(&self, dir: &Entry) -> u32 {
        if dir.cluster == 0 { self.params.root_cluster } else { dir.cluster }
    }

    /// Take a free cluster and end a chain with it.
    fn allocate(&mut self) -> Option<u32> {
        if self.free == Some(0) {
            return None;
        }
        let max = self.max_cluster();
        let start = self.next_free.clamp(2, max.max(2));
        let found = (start..=max).chain(2..start).find(|&c| self.get(c) == 0)?;
        self.set(found, self.end_of_chain());
        self.next_free = if found == max { 2 } else { found + 1 };
        Some(found)
    }

    fn free_chain(&mut self, first: u32) {
        for c in self.chain(first) {
            self.set(c, 0);
        }
    }

    fn free_clusters(&mut self) -> u32 {
        if let Some(free) = self.free {
            return free;
        }
        let free = (2..=self.max_cluster()).filter(|&c| self.get(c) == 0).count() as u32;
        self.free = Some(free);
        free
    }
}

/// A FAT12, FAT16 or FAT32 file system on a disk image.
pub struct FatVolume {
    disk: Rc<DiskImage>,
    /// The volume's first sector on the disk.
    start: u64,
    sectors: u64,
    state: RefCell<State>,
}

impl std::fmt::Debug for FatVolume {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FatVolume({}, {})", self.disk.path().display(), self.start)
    }
}

impl FatVolume {
    /// The file system of the `sectors` sectors of `disk` from `start` on.
    pub fn open(disk: Rc<DiskImage>, start: u64, sectors: u64) -> Result<Self, String> {
        let state = Self::load(&disk, start, sectors)?;
        Ok(FatVolume { disk, start, sectors, state: RefCell::new(state) })
    }

    fn load(disk: &Rc<DiskImage>, start: u64, sectors: u64) -> Result<State, String> {
        let mut boot = [0u8; SECTOR_SIZE];
        disk.read(start, &mut boot).map_err(|_| "Can't read the boot sector".to_string())?;
        let bpb = match Bpb::parse(&boot) {
            Some(bpb) => bpb,
            None if disk.is_floppy() => dos1_bpb(disk)?,
            None => return Err("No FAT file system on the disk".to_string()),
        };
        let params = Params::new(bpb, sectors)?;
        let fat_at = start + bpb.reserved_sectors as u64 + params.active_fat as u64 * bpb.sectors_per_fat as u64;
        let fat_len = bpb.sectors_per_fat as usize * SECTOR_SIZE;
        let mut first = [0u8; SECTOR_SIZE];
        disk.read(fat_at, &mut first).map_err(|_| "Can't read the FAT".to_string())?;
        let mut state = State {
            params,
            disk: disk.clone(),
            fat_at,
            fat_len,
            chunks: RefCell::new(vec![None; fat_len.div_ceil(FAT_CHUNK)]),
            dirty: BTreeSet::new(),
            generation: disk.generation(),
            next_free: 2,
            free: None,
            fsinfo: None,
        };
        // FAT32's FSInfo sector hints where the free clusters start. Its
        // count isn't taken on trust: DOS counts them itself.
        if let Some(sector) = bpb.fat32.map(|f| f.fsinfo as u64).filter(|&s| s > 0 && s < bpb.reserved_sectors as u64) {
            let mut info = [0u8; SECTOR_SIZE];
            let dword = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
            if disk.read(start + sector, &mut info).is_ok()
                && dword(&info, 0) == FSINFO_LEAD
                && dword(&info, 484) == FSINFO_STRUCT
                && dword(&info, 508) == FSINFO_TRAIL
            {
                state.fsinfo = Some(sector);
                let next = dword(&info, FSINFO_NEXT);
                if (2..=state.max_cluster()).contains(&next) {
                    state.next_free = next;
                }
            }
        }
        Ok(state)
    }

    /// The volume's state, read again from the image if something else
    /// wrote to it.
    fn state(&self) -> RefMut<'_, State> {
        let mut state = self.state.borrow_mut();
        if state.generation != self.disk.generation() {
            match Self::load(&self.disk, self.start, self.sectors) {
                Ok(fresh) => *state = fresh,
                // A boot sector overwritten with junk: keep going with what
                // was there.
                Err(_) => state.generation = self.disk.generation(),
            }
        }
        state
    }

    /// Write the changed FAT sectors to every FAT (the one in use alone
    /// when FAT32's mirroring is off) and FAT32's free cluster count and
    /// hint to its FSInfo sector, and take the image's state as the
    /// volume's own.
    fn finish(&self, state: &mut State) -> Result<(), u8> {
        let p = state.params;
        let dirty = std::mem::take(&mut state.dirty);
        let changed = !dirty.is_empty();
        let copies: Vec<u64> = if p.mirrored { (0..p.bpb.fats as u64).collect() } else { vec![p.active_fat as u64] };
        let mut result = Ok(());
        for sector in dirty {
            let at = sector as usize * SECTOR_SIZE;
            let Some(data) = state.chunks.get_mut()[at / FAT_CHUNK].as_ref().map(|chunk| {
                let within = at % FAT_CHUNK;
                chunk[within..within + SECTOR_SIZE.min(chunk.len() - within)].to_vec()
            }) else {
                continue;
            };
            for &copy in &copies {
                let target = p.bpb.reserved_sectors as u64 + copy * p.bpb.sectors_per_fat as u64 + sector;
                if self.write_sector(target, &data).is_err() {
                    result = Err(WRITE_FAULT);
                }
            }
        }
        if let Some(sector) = state.fsinfo.filter(|_| changed) {
            let mut info = [0u8; SECTOR_SIZE];
            self.read_sector(sector, &mut info)?;
            info[FSINFO_FREE..FSINFO_FREE + 4].copy_from_slice(&state.free.unwrap_or(u32::MAX).to_le_bytes());
            info[FSINFO_NEXT..FSINFO_NEXT + 4].copy_from_slice(&state.next_free.to_le_bytes());
            if self.write_sector(sector, &info).is_err() {
                result = Err(WRITE_FAULT);
            }
        }
        state.generation = self.disk.generation();
        result
    }

    pub fn disk(&self) -> &Rc<DiskImage> {
        &self.disk
    }

    /// The volume's first sector on the disk.
    pub fn start(&self) -> u64 {
        self.start
    }

    fn read_sector(&self, sector: u64, buf: &mut [u8]) -> Result<(), u8> {
        self.disk.read(self.start + sector, buf).map_err(|_| READ_FAULT)
    }

    fn write_sector(&self, sector: u64, data: &[u8]) -> Result<(), u8> {
        self.disk.write(self.start + sector, data).map_err(|_| WRITE_FAULT)
    }

    /// Where the file system's parts are, as the DPB and BPB describe
    /// them.
    pub fn layout(&self) -> FatLayout {
        let state = self.state();
        let p = state.params;
        FatLayout {
            bytes_per_sector: p.bpb.bytes_per_sector,
            sectors_per_cluster: p.bpb.sectors_per_cluster as u16,
            clusters: p.clusters,
            reserved_sectors: p.bpb.reserved_sectors,
            fats: p.bpb.fats as u16,
            root_entries: p.bpb.root_entries,
            sectors_per_fat: p.bpb.sectors_per_fat,
            sectors_per_track: p.bpb.sectors_per_track,
            heads: p.bpb.heads,
            hidden_sectors: p.bpb.hidden_sectors,
            media: p.bpb.media,
            sectors: p.bpb.total_sectors,
            fat32: p.bpb.fat32.map(|f| crate::disk::Fat32Layout {
                root_cluster: f.root_cluster,
                fsinfo: f.fsinfo,
                backup_boot: f.backup_boot,
                ext_flags: f.ext_flags,
            }),
        }
    }

    pub fn fat_type(&self) -> FatType {
        self.state().params.kind
    }

    pub fn free_clusters(&self) -> u32 {
        self.state().free_clusters()
    }

    /// Where the search for a free cluster starts.
    pub fn next_free(&self) -> u32 {
        self.state().next_free
    }

    /// The boot sector and where its extended BPB is, if it has one
    /// (signature 29h at 26h, or at 42h on FAT32).
    fn extended_bpb(&self) -> Option<([u8; SECTOR_SIZE], usize)> {
        let at = self.state().params.extended_bpb();
        let mut boot = [0u8; SECTOR_SIZE];
        self.read_sector(0, &mut boot).ok()?;
        (boot[at] == 0x29).then_some((boot, at))
    }

    /// The volume label: the root directory's label entry, or else the
    /// boot sector's.
    pub fn label(&self) -> Option<String> {
        let state = self.state();
        let from_root = self.slots(&state, &Entry::root()).ok().and_then(|slots| {
            slots
                .iter()
                .find(|(_, raw)| raw[0] != DELETED && raw[11] != ATTR_LONG_NAME && raw[11] & ATTR_VOLUME != 0)
                .map(|(_, raw)| raw[..11].iter().map(|&b| b as char).collect::<String>())
        });
        drop(state);
        let label = from_root.or_else(|| {
            let (boot, at) = self.extended_bpb()?;
            let label: String = boot[at + 5..at + 16].iter().map(|&b| b as char).collect();
            (label.trim() != "NO NAME").then_some(label)
        })?;
        let label = label.trim_end().to_string();
        (!label.is_empty()).then_some(label)
    }

    /// The volume serial number from the boot sector.
    pub fn serial(&self) -> Option<u32> {
        let (boot, at) = self.extended_bpb()?;
        Some(u32::from_le_bytes(boot[at + 1..at + 5].try_into().unwrap()))
    }

    /// The sectors a directory's entries are in.
    fn dir_sectors(&self, state: &State, dir: &Entry) -> Vec<u64> {
        let p = &state.params;
        if dir.cluster == 0 && !p.fat32() {
            return (p.root_start..p.root_start + p.root_sectors).collect();
        }
        let per_cluster = p.bpb.sectors_per_cluster as u64;
        state
            .chain(state.dir_cluster(dir))
            .into_iter()
            .flat_map(|c| {
                let first = p.cluster_sector(c);
                first..first + per_cluster
            })
            .collect()
    }

    /// Every entry slot of a directory up to its end marker, with where it
    /// is.
    fn slots(&self, state: &State, dir: &Entry) -> Result<Vec<(EntryRef, [u8; ENTRY_SIZE])>, u8> {
        let mut slots = Vec::new();
        let mut buf = [0u8; SECTOR_SIZE];
        for sector in self.dir_sectors(state, dir) {
            self.read_sector(sector, &mut buf)?;
            for index in 0..ENTRIES_PER_SECTOR {
                let raw: [u8; ENTRY_SIZE] = buf[index * ENTRY_SIZE..(index + 1) * ENTRY_SIZE].try_into().unwrap();
                if raw[0] == 0 {
                    return Ok(slots);
                }
                slots.push((EntryRef { sector, index }, raw));
            }
        }
        Ok(slots)
    }

    /// The file or directory named `want` in `dir`, with its raw entry.
    fn lookup(&self, state: &State, dir: &Entry, want: &[u8; 11]) -> Result<Option<(EntryRef, [u8; ENTRY_SIZE])>, u8> {
        Ok(self.slots(state, dir)?.into_iter().find(|(_, raw)| {
            raw[0] != DELETED && raw[11] != ATTR_LONG_NAME && raw[11] & ATTR_VOLUME == 0 && same_name(raw, want)
        }))
    }

    fn find_in(&self, state: &State, path: &[&str]) -> Result<Entry, u8> {
        let mut current = Entry::root();
        for (i, part) in path.iter().enumerate() {
            let missing = if i + 1 == path.len() { FILE_NOT_FOUND } else { PATH_NOT_FOUND };
            if !current.is_dir() {
                return Err(PATH_NOT_FOUND);
            }
            let want = short_name(part).ok_or(missing)?;
            let (at, raw) = self.lookup(state, &current, &want)?.ok_or(missing)?;
            current = state.params.entry(&raw, at);
            // ".." of a directory in the root points at cluster 0, or on
            // FAT32 sometimes at the root's own.
            if current.is_dir() && (current.cluster == 0 || current.cluster == state.params.root_cluster) {
                current = Entry::root();
            }
        }
        Ok(current)
    }

    /// The directory `path` names: 03h if it isn't one.
    fn dir_in(&self, state: &State, path: &[&str]) -> Result<Entry, u8> {
        match self.find_in(state, path) {
            Ok(dir) if dir.is_dir() => Ok(dir),
            _ => Err(PATH_NOT_FOUND),
        }
    }

    /// The file or directory at `path`: 02h if it's missing, 03h if a
    /// directory on the way is.
    pub fn find(&self, path: &[&str]) -> Result<Entry, u8> {
        let state = self.state();
        self.find_in(&state, path)
    }

    /// The files and directories in the directory at `path`, in the order
    /// they're on the disk. Volume labels, long names and deleted entries
    /// are left out.
    pub fn list(&self, path: &[&str]) -> Result<Vec<Entry>, u8> {
        let state = self.state();
        let dir = self.dir_in(&state, path)?;
        Ok(self
            .slots(&state, &dir)?
            .into_iter()
            .filter(|(_, raw)| raw[0] != DELETED && raw[11] != ATTR_LONG_NAME && raw[11] & ATTR_VOLUME == 0)
            .map(|(at, raw)| state.params.entry(&raw, at))
            .collect())
    }

    fn raw_entry(&self, at: EntryRef) -> Result<[u8; SECTOR_SIZE], u8> {
        let mut buf = [0u8; SECTOR_SIZE];
        self.read_sector(at.sector, &mut buf)?;
        Ok(buf)
    }

    /// The entry at `at` as it is on the disk now.
    pub fn reload(&self, at: EntryRef) -> Result<Entry, u8> {
        let state = self.state();
        let buf = self.raw_entry(at)?;
        let raw = &buf[at.index * ENTRY_SIZE..(at.index + 1) * ENTRY_SIZE];
        if raw[0] == 0 || raw[0] == DELETED {
            return Err(FILE_NOT_FOUND);
        }
        Ok(state.params.entry(raw, at))
    }

    /// Change the entry at `at` in place.
    fn update_entry(&self, at: EntryRef, change: impl FnOnce(&mut [u8])) -> Result<(), u8> {
        let mut buf = self.raw_entry(at)?;
        change(&mut buf[at.index * ENTRY_SIZE..(at.index + 1) * ENTRY_SIZE]);
        self.write_sector(at.sector, &buf)
    }

    fn write_raw(&self, at: EntryRef, raw: &[u8; ENTRY_SIZE]) -> Result<(), u8> {
        self.update_entry(at, |slot| slot.copy_from_slice(raw))
    }

    /// Mark the entry at `at` in `dir` deleted, with the long name entries
    /// in front of it.
    fn delete_entry(&self, state: &State, dir: &Entry, at: EntryRef) -> Result<(), u8> {
        let slots = self.slots(state, dir)?;
        let mut targets = vec![at];
        if let Some(i) = slots.iter().position(|(r, _)| *r == at) {
            targets.extend(
                slots[..i]
                    .iter()
                    .rev()
                    .take_while(|(_, raw)| raw[11] == ATTR_LONG_NAME && raw[0] != DELETED)
                    .map(|(r, _)| *r),
            );
        }
        for target in targets {
            self.update_entry(target, |slot| slot[0] = DELETED)?;
        }
        Ok(())
    }

    /// Read from `offset` of `file` into `buf`: the number of bytes read,
    /// fewer at the end of the file.
    pub fn read(&self, file: &Entry, offset: u64, buf: &mut [u8]) -> Result<usize, u8> {
        let state = self.state();
        let size = file.size as u64;
        if offset >= size || buf.is_empty() {
            return Ok(0);
        }
        let len = (buf.len() as u64).min(size - offset) as usize;
        let cluster_bytes = state.params.cluster_bytes;
        let mut cluster = file.cluster;
        if !(2..=state.max_cluster()).contains(&cluster) {
            return Ok(0);
        }
        for _ in 0..offset / cluster_bytes {
            match state.next(cluster) {
                Some(next) => cluster = next,
                None => return Ok(0),
            }
        }
        let mut sector_buf = [0u8; SECTOR_SIZE];
        let mut done = 0;
        let mut pos = offset;
        while done < len {
            let within = pos % cluster_bytes;
            let sector = state.params.cluster_sector(cluster) + within / SECTOR_SIZE as u64;
            let skip = (within % SECTOR_SIZE as u64) as usize;
            let n = (SECTOR_SIZE - skip).min(len - done);
            self.read_sector(sector, &mut sector_buf)?;
            buf[done..done + n].copy_from_slice(&sector_buf[skip..skip + n]);
            done += n;
            pos += n as u64;
            if pos.is_multiple_of(cluster_bytes) && done < len {
                match state.next(cluster) {
                    Some(next) => cluster = next,
                    None => break,
                }
            }
        }
        Ok(done)
    }

    /// Write `data` at `offset` of the clusters `chain`.
    fn write_chain(&self, state: &State, chain: &[u32], offset: u64, data: &[u8]) -> Result<(), u8> {
        let cluster_bytes = state.params.cluster_bytes;
        let mut sector_buf = [0u8; SECTOR_SIZE];
        let mut done = 0;
        let mut pos = offset;
        while done < data.len() {
            let within = pos % cluster_bytes;
            let sector = state.params.cluster_sector(chain[(pos / cluster_bytes) as usize])
                + within / SECTOR_SIZE as u64;
            let skip = (within % SECTOR_SIZE as u64) as usize;
            let n = (SECTOR_SIZE - skip).min(data.len() - done);
            if n < SECTOR_SIZE {
                self.read_sector(sector, &mut sector_buf)?;
            }
            sector_buf[skip..skip + n].copy_from_slice(&data[done..done + n]);
            self.write_sector(sector, &sector_buf)?;
            done += n;
            pos += n as u64;
        }
        Ok(())
    }

    /// Write `data` at `offset` of the file whose entry is at `at`,
    /// growing it as needed. Returns the bytes written: fewer when the disk
    /// fills up. Writing past the end fills the gap with zeros.
    pub fn write(&self, at: EntryRef, offset: u64, data: &[u8]) -> Result<usize, u8> {
        if data.is_empty() {
            return Ok(0);
        }
        let mut state = self.state();
        let buf = self.raw_entry(at)?;
        let mut entry = state.params.entry(&buf[at.index * ENTRY_SIZE..(at.index + 1) * ENTRY_SIZE], at);
        let old_size = entry.size as u64;
        let end = (offset + data.len() as u64).min(u32::MAX as u64);
        let cluster_bytes = state.params.cluster_bytes;

        let mut chain = if entry.cluster == 0 { Vec::new() } else { state.chain(entry.cluster) };
        let needed = end.div_ceil(cluster_bytes) as usize;
        while chain.len() < needed {
            let Some(cluster) = state.allocate() else { break };
            match chain.last() {
                Some(&last) => state.set(last, cluster),
                None => entry.cluster = cluster,
            }
            chain.push(cluster);
        }
        let end = end.min(chain.len() as u64 * cluster_bytes);
        let written = end.saturating_sub(offset) as usize;
        let result = (|| {
            if offset > old_size && written > 0 {
                let gap = vec![0u8; (offset - old_size) as usize];
                self.write_chain(&state, &chain, old_size, &gap)?;
            }
            self.write_chain(&state, &chain, offset, &data[..written])?;
            let (time, date) = dos_now();
            let size = if written > 0 { old_size.max(end) } else { old_size } as u32;
            let params = state.params;
            self.update_entry(at, |raw| {
                raw[11] |= ATTR_ARCHIVE;
                raw[22..24].copy_from_slice(&time.to_le_bytes());
                raw[24..26].copy_from_slice(&date.to_le_bytes());
                params.set_cluster(raw, entry.cluster);
                raw[28..32].copy_from_slice(&size.to_le_bytes());
            })
        })();
        let finished = self.finish(&mut state);
        result.and(finished).map(|()| written)
    }

    /// Put a file with the contents `data`, dated `time` and `date`, at
    /// `path`, making the directories on its way and replacing a file that
    /// is there. 27h (disk full) if it doesn't fit, which leaves no file.
    pub fn put_file(&self, path: &[&str], data: &[u8], time: u16, date: u16) -> Result<(), u8> {
        let (_, dirs) = path.split_last().ok_or(ACCESS_DENIED)?;
        for depth in 1..=dirs.len() {
            match self.find(&dirs[..depth]) {
                Ok(entry) if entry.is_dir() => {}
                Ok(_) => return Err(ACCESS_DENIED),
                Err(_) => self.mkdir(&dirs[..depth])?,
            }
        }
        match self.find(path) {
            Ok(entry) if entry.is_dir() => return Err(ACCESS_DENIED),
            Ok(_) => self.remove(path)?,
            Err(_) => {}
        }
        let at = self.create(path, 0)?.at.ok_or(ACCESS_DENIED)?;
        if self.write(at, 0, data)? < data.len() {
            let _ = self.remove(path);
            return Err(DISK_FULL);
        }
        self.set_time(at, time, date)
    }

    /// Set the time and date of the entry at `at`.
    pub fn set_time(&self, at: EntryRef, time: u16, date: u16) -> Result<(), u8> {
        let mut state = self.state();
        let result = self.update_entry(at, |raw| {
            raw[22..24].copy_from_slice(&time.to_le_bytes());
            raw[24..26].copy_from_slice(&date.to_le_bytes());
        });
        self.finish(&mut state).and(result)
    }

    /// A free entry slot in `dir`, growing a subdirectory (or FAT32's root
    /// directory) by a cluster when it's full. A FAT12 or FAT16 root
    /// directory can't grow: 05h.
    fn free_slot(&self, state: &mut State, dir: &Entry) -> Result<EntryRef, u8> {
        let mut buf = [0u8; SECTOR_SIZE];
        let sectors = self.dir_sectors(state, dir);
        for &sector in &sectors {
            self.read_sector(sector, &mut buf)?;
            if let Some(index) = (0..ENTRIES_PER_SECTOR).find(|i| matches!(buf[i * ENTRY_SIZE], 0 | DELETED)) {
                return Ok(EntryRef { sector, index });
            }
        }
        if dir.cluster == 0 && !state.params.fat32() {
            return Err(ACCESS_DENIED);
        }
        let last = *state.chain(state.dir_cluster(dir)).last().ok_or(ACCESS_DENIED)?;
        let cluster = state.allocate().ok_or(ACCESS_DENIED)?;
        state.set(last, cluster);
        let first = self.zero_cluster(state, cluster)?;
        Ok(EntryRef { sector: first, index: 0 })
    }

    /// Fill a cluster with zeros; returns its first sector.
    fn zero_cluster(&self, state: &State, cluster: u32) -> Result<u64, u8> {
        let first = state.params.cluster_sector(cluster);
        let zeros = vec![0u8; state.params.cluster_bytes as usize];
        self.write_sector(first, &zeros)?;
        Ok(first)
    }

    /// A new directory entry.
    pub(crate) fn new_entry(name: &[u8; 11], attr: u8, cluster: u32) -> [u8; ENTRY_SIZE] {
        let (time, date) = dos_now();
        let mut raw = [0u8; ENTRY_SIZE];
        raw[..11].copy_from_slice(name);
        raw[11] = attr;
        raw[14..16].copy_from_slice(&time.to_le_bytes());
        raw[16..18].copy_from_slice(&date.to_le_bytes());
        raw[18..20].copy_from_slice(&date.to_le_bytes());
        raw[22..24].copy_from_slice(&time.to_le_bytes());
        raw[24..26].copy_from_slice(&date.to_le_bytes());
        raw[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
        raw[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
        raw
    }

    /// The directory a new entry at `path` goes in and the entry's name:
    /// 03h for a missing directory or a name DOS doesn't allow, 05h if the
    /// name is taken.
    fn new_place(&self, state: &State, path: &[&str]) -> Result<(Entry, [u8; 11]), u8> {
        let (leaf, parent) = path.split_last().ok_or(ACCESS_DENIED)?;
        let dir = self.dir_in(state, parent)?;
        let name = short_name(leaf).filter(|n| n[0] != b'.').ok_or(PATH_NOT_FOUND)?;
        if self.lookup(state, &dir, &name)?.is_some() {
            return Err(ACCESS_DENIED);
        }
        Ok((dir, name))
    }

    /// Create the empty file `path`, which must not exist.
    pub fn create(&self, path: &[&str], attr: u8) -> Result<Entry, u8> {
        let mut state = self.state();
        let result = (|| -> Result<Entry, u8> {
            let (dir, name) = self.new_place(&state, path)?;
            let at = self.free_slot(&mut state, &dir)?;
            let raw = Self::new_entry(&name, (attr & ATTR_CHANGEABLE) | ATTR_ARCHIVE, 0);
            self.write_raw(at, &raw)?;
            Ok(state.params.entry(&raw, at))
        })();
        let finished = self.finish(&mut state);
        let entry = result?;
        finished.map(|()| entry)
    }

    /// Delete the file at `path`.
    pub fn remove(&self, path: &[&str]) -> Result<(), u8> {
        let mut state = self.state();
        let result = (|| {
            let (_, parent) = path.split_last().ok_or(FILE_NOT_FOUND)?;
            let dir = self.dir_in(&state, parent)?;
            let entry = self.find_in(&state, path)?;
            if entry.is_dir() {
                return Err(FILE_NOT_FOUND);
            }
            let at = entry.at.ok_or(FILE_NOT_FOUND)?;
            state.free_chain(entry.cluster);
            self.delete_entry(&state, &dir, at)
        })();
        self.finish(&mut state).and(result)
    }

    /// Rename or move the file at `from` to `to`. Directories can only be
    /// renamed in place, as in DOS.
    pub fn rename(&self, from: &[&str], to: &[&str]) -> Result<(), u8> {
        let mut state = self.state();
        let result = (|| {
            let (_, from_parent) = from.split_last().ok_or(FILE_NOT_FOUND)?;
            let from_dir = self.dir_in(&state, from_parent)?;
            let entry = self.find_in(&state, from)?;
            let at = entry.at.ok_or(ACCESS_DENIED)?;
            let (to_dir, name) = self.new_place(&state, to)?;
            let mut raw: [u8; ENTRY_SIZE] =
                self.raw_entry(at)?[at.index * ENTRY_SIZE..(at.index + 1) * ENTRY_SIZE].try_into().unwrap();
            raw[..11].copy_from_slice(&name);
            if to_dir.cluster == from_dir.cluster {
                // The long name, if any, is the old one.
                self.delete_entry(&state, &from_dir, at)?;
                return self.write_raw(at, &raw);
            }
            if entry.is_dir() {
                return Err(ACCESS_DENIED);
            }
            let slot = self.free_slot(&mut state, &to_dir)?;
            self.write_raw(slot, &raw)?;
            self.delete_entry(&state, &from_dir, at)
        })();
        self.finish(&mut state).and(result)
    }

    /// Create the directory `path`.
    pub fn mkdir(&self, path: &[&str]) -> Result<(), u8> {
        let mut state = self.state();
        let result = (|| {
            let (parent, name) = self.new_place(&state, path)?;
            let cluster = state.allocate().ok_or(ACCESS_DENIED)?;
            let slot = match self.free_slot(&mut state, &parent) {
                Ok(slot) => slot,
                Err(e) => {
                    state.set(cluster, 0);
                    return Err(e);
                }
            };
            let first = self.zero_cluster(&state, cluster)?;
            let mut dots = [0u8; SECTOR_SIZE];
            // ".." of a directory in the root is 0, FAT32's too.
            dots[..ENTRY_SIZE].copy_from_slice(&Self::new_entry(b".          ", ATTR_DIRECTORY, cluster));
            dots[ENTRY_SIZE..2 * ENTRY_SIZE]
                .copy_from_slice(&Self::new_entry(b"..         ", ATTR_DIRECTORY, parent.cluster));
            self.write_sector(first, &dots)?;
            self.write_raw(slot, &Self::new_entry(&name, ATTR_DIRECTORY, cluster))
        })();
        self.finish(&mut state).and(result)
    }

    /// Remove the empty directory `path`: 05h if it isn't empty.
    pub fn rmdir(&self, path: &[&str]) -> Result<(), u8> {
        let mut state = self.state();
        let result = (|| {
            let (_, parent) = path.split_last().ok_or(ACCESS_DENIED)?;
            let dir = self.dir_in(&state, parent)?;
            let entry = self.find_in(&state, path).map_err(|_| PATH_NOT_FOUND)?;
            let at = entry.at.filter(|_| entry.is_dir()).ok_or(PATH_NOT_FOUND)?;
            let occupied = self.slots(&state, &entry)?.iter().any(|(_, raw)| {
                raw[0] != DELETED && raw[11] != ATTR_LONG_NAME && raw[0] != b'.'
            });
            if occupied {
                return Err(ACCESS_DENIED);
            }
            state.free_chain(entry.cluster);
            self.delete_entry(&state, &dir, at)
        })();
        self.finish(&mut state).and(result)
    }

    /// Set the attributes of the file or directory at `path` that programs
    /// can change.
    pub fn set_attr(&self, path: &[&str], attr: u8) -> Result<(), u8> {
        let mut state = self.state();
        let result = (|| {
            let at = self.find_in(&state, path)?.at.ok_or(ACCESS_DENIED)?;
            self.update_entry(at, |raw| raw[11] = (raw[11] & !ATTR_CHANGEABLE) | (attr & ATTR_CHANGEABLE))
        })();
        self.finish(&mut state).and(result)
    }

    fn sector_range(&self, sector: u64, len: usize) -> Result<(), u16> {
        let count = len.div_ceil(SECTOR_SIZE) as u64;
        if sector.checked_add(count).is_none_or(|end| end > self.sectors) {
            return Err(0x0408); // sector not found
        }
        Ok(())
    }

    /// INT 25h: read sectors of the volume. Errors are INT 25h's AX.
    pub fn read_sectors(&self, sector: u64, buf: &mut [u8]) -> Result<(), u16> {
        self.sector_range(sector, buf.len())?;
        self.disk.read(self.start + sector, buf).map_err(disk_error)
    }

    /// INT 26h: write sectors of the volume.
    pub fn write_sectors(&self, sector: u64, data: &[u8]) -> Result<(), u16> {
        self.sector_range(sector, data.len())?;
        self.disk.write(self.start + sector, data).map_err(disk_error)
    }
}

/// INT 25h/26h's error code (AH = BIOS status, AL = DOS critical error)
/// for a disk image's status.
fn disk_error(status: u8) -> u16 {
    match status {
        STATUS_WRITE_PROTECTED => 0x0300,
        STATUS_SECTOR_NOT_FOUND => 0x0408,
        _ => 0x200C,
    }
}

/// The BPB of a DOS 1.x floppy, which has none: its format follows from
/// the media byte at the start of its FAT.
fn dos1_bpb(disk: &DiskImage) -> Result<Bpb, String> {
    let mut fat = [0u8; SECTOR_SIZE];
    disk.read(1, &mut fat).map_err(|_| "Can't read the FAT".to_string())?;
    let size = disk.sectors();
    let (per_cluster, root_entries, per_fat, total) = match fat[0] {
        0xFE => (1, 64, 1, 320),
        0xFC => (1, 64, 2, 360),
        0xFF => (2, 112, 1, 640),
        0xFD => (2, 112, 2, 720),
        0xF9 if size >= 2400 => (1, 224, 7, 2400),
        0xF9 => (2, 112, 3, 1440),
        0xF0 if size >= 5760 => (2, 240, 9, 5760),
        0xF0 => (1, 224, 9, 2880),
        _ => return Err("No FAT file system on the disk".to_string()),
    };
    let geometry = disk.geometry();
    Ok(Bpb {
        bytes_per_sector: SECTOR_SIZE as u16,
        sectors_per_cluster: per_cluster,
        reserved_sectors: 1,
        fats: 2,
        root_entries,
        total_sectors: total,
        media: fat[0],
        sectors_per_fat: per_fat,
        sectors_per_track: geometry.sectors as u16,
        heads: geometry.heads as u16,
        hidden_sectors: 0,
        fat32: None,
    })
}

/// Put an empty FAT file system on the `sectors` sectors of `disk` from
/// `start` on: a floppy's standard one for its size, otherwise FAT12 or
/// FAT16 by the size.
pub fn format(disk: &DiskImage, start: u64, sectors: u64, label: Option<&str>) -> Result<(), String> {
    let floppy = disk.is_floppy();
    let (per_cluster, root_entries, media) = match (floppy, sectors) {
        (true, 720) => (2u64, 112u64, 0xFDu8),
        (true, 1440) => (2, 112, 0xF9),
        (true, 2400) => (1, 224, 0xF9),
        (true, 2880) => (1, 224, 0xF0),
        (true, 5760) => (2, 240, 0xF0),
        (true, _) => (1, 224, 0xF0),
        (false, _) => {
            let mut per_cluster = 1;
            while sectors / per_cluster >= FAT16_MAX_CLUSTERS - 10 {
                per_cluster *= 2;
            }
            (per_cluster, 512, 0xF8)
        }
    };
    if per_cluster > 128 {
        return Err("The disk is too big for FAT16".to_string());
    }
    let root_sectors = root_entries * ENTRY_SIZE as u64 / SECTOR_SIZE as u64;
    let fat12 = sectors / per_cluster < FAT12_MAX_CLUSTERS;
    let mut per_fat = 1u64;
    for _ in 0..4 {
        let clusters = (sectors - 1 - root_sectors - 2 * per_fat) / per_cluster;
        let bytes = if fat12 { (clusters + 2) * 3 / 2 + 1 } else { (clusters + 2) * 2 };
        per_fat = bytes.div_ceil(SECTOR_SIZE as u64);
    }
    let geometry = disk.geometry();
    let mut boot = [0u8; SECTOR_SIZE];
    boot[..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
    boot[3..11].copy_from_slice(b"RUSTDOS ");
    boot[0x0B..0x0D].copy_from_slice(&(SECTOR_SIZE as u16).to_le_bytes());
    boot[0x0D] = per_cluster as u8;
    boot[0x0E..0x10].copy_from_slice(&1u16.to_le_bytes());
    boot[0x10] = 2;
    boot[0x11..0x13].copy_from_slice(&(root_entries as u16).to_le_bytes());
    if sectors <= 0xFFFF {
        boot[0x13..0x15].copy_from_slice(&(sectors as u16).to_le_bytes());
    } else {
        boot[0x20..0x24].copy_from_slice(&(sectors as u32).to_le_bytes());
    }
    boot[0x15] = media;
    boot[0x16..0x18].copy_from_slice(&(per_fat as u16).to_le_bytes());
    boot[0x18..0x1A].copy_from_slice(&(geometry.sectors as u16).to_le_bytes());
    boot[0x1A..0x1C].copy_from_slice(&(geometry.heads as u16).to_le_bytes());
    boot[0x1C..0x20].copy_from_slice(&(start as u32).to_le_bytes());
    boot[0x24] = if floppy { 0x00 } else { 0x80 };
    boot[0x26] = 0x29;
    boot[0x27..0x2B].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    let label_bytes = label.map_or(*b"NO NAME    ", |l| {
        let mut raw = [b' '; 11];
        for (i, c) in l.bytes().take(11).enumerate() {
            raw[i] = c.to_ascii_uppercase();
        }
        raw
    });
    boot[0x2B..0x36].copy_from_slice(&label_bytes);
    boot[0x36..0x3E].copy_from_slice(if fat12 { b"FAT12   " } else { b"FAT16   " });
    boot[510] = 0x55;
    boot[511] = 0xAA;

    let error = |_| "Can't write the disk".to_string();
    disk.write(start, &boot).map_err(error)?;
    let zeros = vec![0u8; ((2 * per_fat + root_sectors) as usize) * SECTOR_SIZE];
    disk.write(start + 1, &zeros).map_err(error)?;
    let head: &[u8] = if fat12 { &[media, 0xFF, 0xFF] } else { &[media, 0xFF, 0xFF, 0xFF] };
    let mut first = [0u8; SECTOR_SIZE];
    first[..head.len()].copy_from_slice(head);
    for copy in 0..2 {
        disk.write(start + 1 + copy * per_fat, &first).map_err(error)?;
    }
    if label.is_some() {
        let mut root = [0u8; SECTOR_SIZE];
        root[..ENTRY_SIZE].copy_from_slice(&FatVolume::new_entry(&label_bytes, ATTR_VOLUME, 0));
        disk.write(start + 1 + 2 * per_fat, &root).map_err(error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn image(name: &str, bytes: usize) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rust-dos-fat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, vec![0u8; bytes]).unwrap();
        path
    }

    fn floppy(name: &str) -> FatVolume {
        let disk = Rc::new(DiskImage::open(&image(name, 1_474_560), true, None, false).unwrap());
        format(&disk, 0, 2880, Some("Disk one")).unwrap();
        FatVolume::open(disk, 0, 2880).unwrap()
    }

    fn contents(volume: &FatVolume, path: &[&str]) -> Vec<u8> {
        let entry = volume.find(path).unwrap();
        let mut buf = vec![0u8; entry.size as usize + 10];
        let n = volume.read(&entry, 0, &mut buf).unwrap();
        buf.truncate(n);
        buf
    }

    #[test]
    fn names() {
        assert_eq!(&short_name("readme.txt").unwrap(), b"README  TXT");
        assert_eq!(&short_name("LONGFILENAME.TEXT").unwrap(), b"LONGFILETEX");
        assert_eq!(&short_name("FOO.").unwrap(), b"FOO        ");
        assert_eq!(short_name("A.B.C"), None);
        assert_eq!(short_name("BAD*.TXT"), None);
        assert_eq!(short_name(".TXT"), None);
        assert_eq!(display_name(b"README  TXT"), "README.TXT");
        assert_eq!(display_name(b"DIR        "), "DIR");
        assert_eq!(display_name(b"\x05BC     DAT"), "\u{E5}BC.DAT");
    }

    #[test]
    fn a_new_floppy() {
        let volume = floppy("new.img");
        let layout = volume.layout();
        assert_eq!((layout.sectors_per_cluster, layout.root_entries, layout.sectors_per_fat), (1, 224, 9));
        assert_eq!((layout.total_sectors(), layout.media, layout.clusters), (2880, 0xF0, 2847));
        assert_eq!(volume.free_clusters(), 2847);
        assert_eq!(volume.label().as_deref(), Some("DISK ONE"));
        assert_eq!(volume.list(&[]).unwrap(), vec![]);
        assert_eq!(volume.find(&["NOPE"]), Err(FILE_NOT_FOUND));
        assert_eq!(volume.find(&["NOPE", "X"]), Err(PATH_NOT_FOUND));
    }

    #[test]
    fn files_grow_across_clusters() {
        let volume = floppy("grow.img");
        let entry = volume.create(&["data.bin"], 0).unwrap();
        let at = entry.at.unwrap();
        let data: Vec<u8> = (0..5000u32).map(|i| (i * 7) as u8).collect();
        assert_eq!(volume.write(at, 0, &data[..700]), Ok(700));
        assert_eq!(volume.write(at, 700, &data[700..]), Ok(4300));
        assert_eq!(contents(&volume, &["DATA.BIN"]), data);
        assert_eq!(volume.free_clusters(), 2847 - 10);

        // Overwrite in the middle, across a sector boundary.
        assert_eq!(volume.write(at, 510, &[0xAA; 4]), Ok(4));
        let back = contents(&volume, &["data.bin"]);
        assert_eq!(&back[508..516], &[data[508], data[509], 0xAA, 0xAA, 0xAA, 0xAA, data[514], data[515]]);

        // Writing past the end fills the gap with zeros.
        assert_eq!(volume.write(at, 6000, b"END"), Ok(3));
        let back = contents(&volume, &["DATA.BIN"]);
        assert_eq!(back.len(), 6003);
        assert!(back[5000..6000].iter().all(|&b| b == 0));
        assert_eq!(&back[6000..], b"END");

        let reloaded = volume.reload(at).unwrap();
        assert_eq!(reloaded.size, 6003);
        assert_ne!(reloaded.attr & ATTR_ARCHIVE, 0);
    }

    #[test]
    fn both_fats_stay_the_same() {
        let volume = floppy("fats.img");
        let entry = volume.create(&["A"], 0).unwrap();
        volume.write(entry.at.unwrap(), 0, &vec![1u8; 20_000]).unwrap();
        let mut fat1 = vec![0u8; 9 * 512];
        let mut fat2 = vec![0u8; 9 * 512];
        volume.disk().read(1, &mut fat1).unwrap();
        volume.disk().read(10, &mut fat2).unwrap();
        assert_eq!(fat1, fat2);
        // FAT12: cluster 2 -> 3 -> ..., 40 clusters of 512 bytes, the
        // chain ending at 41.
        assert_eq!(&fat1[..6], &[0xF0, 0xFF, 0xFF, 0x03, 0x40, 0x00]);
    }

    #[test]
    fn a_full_disk_takes_what_fits() {
        let volume = floppy("full.img");
        let entry = volume.create(&["BIG"], 0).unwrap();
        let big = vec![0x55u8; 1_500_000];
        let written = volume.write(entry.at.unwrap(), 0, &big).unwrap();
        assert_eq!(written, 2847 * 512);
        assert_eq!(volume.free_clusters(), 0);
        let more = volume.create(&["MORE"], 0).unwrap();
        assert_eq!(volume.write(more.at.unwrap(), 0, b"x"), Ok(0));
    }

    #[test]
    fn directories() {
        let volume = floppy("dirs.img");
        volume.mkdir(&["GAMES"]).unwrap();
        volume.mkdir(&["GAMES", "DOOM"]).unwrap();
        assert_eq!(volume.mkdir(&["GAMES"]), Err(ACCESS_DENIED));
        assert_eq!(volume.mkdir(&["NONE", "X"]), Err(PATH_NOT_FOUND));
        let doom = volume.create(&["GAMES", "DOOM", "DOOM.EXE"], 0).unwrap();
        volume.write(doom.at.unwrap(), 0, b"MZ").unwrap();

        let names: Vec<String> = volume.list(&["GAMES"]).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, [".", "..", "DOOM"]);
        assert!(volume.find(&["games", "doom"]).unwrap().is_dir());
        assert_eq!(contents(&volume, &["GAMES", "DOOM", "DOOM.EXE"]), b"MZ");
        assert_eq!(volume.list(&["GAMES", "DOOM", "DOOM.EXE"]), Err(PATH_NOT_FOUND));

        // A directory fills its cluster and grows.
        for i in 0..40 {
            volume.create(&["GAMES", &format!("F{}", i)], 0).unwrap();
        }
        assert_eq!(volume.list(&["GAMES"]).unwrap().len(), 43);

        assert_eq!(volume.rmdir(&["GAMES", "DOOM"]), Err(ACCESS_DENIED));
        volume.remove(&["GAMES", "DOOM", "DOOM.EXE"]).unwrap();
        volume.rmdir(&["GAMES", "DOOM"]).unwrap();
        assert_eq!(volume.find(&["GAMES", "DOOM"]), Err(FILE_NOT_FOUND));
    }

    #[test]
    fn the_root_directory_fills_up() {
        let volume = floppy("root.img");
        // 224 entries, one of them the label.
        for i in 0..223 {
            volume.create(&[&format!("F{}", i)], 0).unwrap();
        }
        assert_eq!(volume.create(&["LAST"], 0), Err(ACCESS_DENIED));
        volume.remove(&["F5"]).unwrap();
        volume.create(&["LAST"], 0).unwrap();
    }

    #[test]
    fn rename_delete_and_attributes() {
        let volume = floppy("rename.img");
        volume.mkdir(&["SUB"]).unwrap();
        let file = volume.create(&["OLD.TXT"], 0).unwrap();
        let free = volume.free_clusters();
        volume.write(file.at.unwrap(), 0, b"hello").unwrap();

        volume.rename(&["OLD.TXT"], &["NEW.TXT"]).unwrap();
        assert_eq!(volume.find(&["OLD.TXT"]), Err(FILE_NOT_FOUND));
        volume.rename(&["NEW.TXT"], &["SUB", "MOVED.TXT"]).unwrap();
        assert_eq!(contents(&volume, &["SUB", "MOVED.TXT"]), b"hello");
        assert_eq!(volume.find(&["NEW.TXT"]), Err(FILE_NOT_FOUND));
        let other = volume.create(&["OTHER"], 0).unwrap();
        assert_eq!(volume.rename(&["OTHER"], &["SUB", "MOVED.TXT"]), Err(ACCESS_DENIED));
        assert_eq!(volume.rename(&["SUB"], &["X", "SUB"]), Err(PATH_NOT_FOUND));
        volume.rename(&["SUB"], &["DIR"]).unwrap();
        assert!(volume.find(&["DIR", "MOVED.TXT"]).is_ok());

        volume.set_attr(&["DIR", "MOVED.TXT"], ATTR_READ_ONLY | ATTR_HIDDEN | ATTR_DIRECTORY).unwrap();
        assert_eq!(volume.find(&["DIR", "MOVED.TXT"]).unwrap().attr, ATTR_READ_ONLY | ATTR_HIDDEN);

        volume.set_time(other.at.unwrap(), 0x1234, 0x5678).unwrap();
        let other = volume.find(&["OTHER"]).unwrap();
        assert_eq!((other.time, other.date), (0x1234, 0x5678));

        volume.remove(&["DIR", "MOVED.TXT"]).unwrap();
        assert_eq!(volume.free_clusters(), free);
        assert_eq!(volume.remove(&["DIR"]), Err(FILE_NOT_FOUND));
    }

    #[test]
    fn deleting_removes_long_names() {
        let volume = floppy("lfn.img");
        let at = volume.create(&["LONGNA~1.TXT"], 0).unwrap().at.unwrap();
        // Put a long name entry in front of it by hand.
        let root = at.sector;
        let mut buf = [0u8; SECTOR_SIZE];
        volume.disk().read(root, &mut buf).unwrap();
        buf.copy_within(ENTRY_SIZE..2 * ENTRY_SIZE, 2 * ENTRY_SIZE);
        buf[ENTRY_SIZE..2 * ENTRY_SIZE].fill(0x20);
        buf[ENTRY_SIZE] = 0x41;
        buf[ENTRY_SIZE + 11] = ATTR_LONG_NAME;
        volume.disk().write(root, &buf).unwrap();

        assert_eq!(volume.list(&[]).unwrap().len(), 1);
        volume.remove(&["LONGNA~1.TXT"]).unwrap();
        volume.disk().read(root, &mut buf).unwrap();
        assert_eq!((buf[ENTRY_SIZE], buf[2 * ENTRY_SIZE]), (DELETED, DELETED));
    }

    #[test]
    fn raw_writes_are_seen() {
        let volume = floppy("raw.img");
        let at = volume.create(&["F"], 0).unwrap().at.unwrap();
        volume.write(at, 0, &[1; 600]).unwrap();
        assert_eq!(volume.free_clusters(), 2847 - 2);
        // Another program clears the FAT and the root directory by sector.
        let mut fat = [0u8; SECTOR_SIZE];
        fat[..3].copy_from_slice(&[0xF0, 0xFF, 0xFF]);
        volume.write_sectors(1, &fat).unwrap();
        volume.write_sectors(19, &[0u8; SECTOR_SIZE]).unwrap();
        assert_eq!(volume.free_clusters(), 2847);
        assert_eq!(volume.find(&["F"]), Err(FILE_NOT_FOUND));
        let mut buf = [0u8; SECTOR_SIZE];
        assert_eq!(volume.read_sectors(2880, &mut buf), Err(0x0408));
    }

    /// A FAT32 hard disk from MAKEIMG, of `mb` MB with clusters of `spc`
    /// sectors, and its volume.
    fn fat32(name: &str, mb: u64, spc: u32) -> (Rc<DiskImage>, FatVolume) {
        let dir = std::env::temp_dir().join(format!("rust-dos-fat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let spec = crate::makeimg::ImageSpec {
            size_mb: Some(mb),
            fat: Some(32),
            sectors_per_cluster: Some(spc),
            label: Some("Big One".to_string()),
            ..Default::default()
        };
        crate::makeimg::write(&path, &crate::makeimg::plan(&spec).unwrap(), true).unwrap();
        let disk = Rc::new(DiskImage::open(&path, false, None, false).unwrap());
        let (start, sectors) = disk.fat_volume().unwrap();
        let volume = FatVolume::open(disk.clone(), start, sectors).unwrap();
        (disk, volume)
    }

    /// FAT entry `n` of the FAT `copy` as it is on the disk.
    fn fat_entry(volume: &FatVolume, copy: u64, n: u32) -> u32 {
        let p = volume.state().params;
        let mut buf = [0u8; SECTOR_SIZE];
        let sector = p.bpb.reserved_sectors as u64 + copy * p.bpb.sectors_per_fat as u64 + n as u64 * 4 / 512;
        volume.read_sector(sector, &mut buf).unwrap();
        let at = n as usize * 4 % 512;
        u32::from_le_bytes(buf[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn fat32_volumes() {
        let (_, volume) = fat32("fat32.img", 64, 1);
        assert_eq!(volume.fat_type(), FatType::Fat32);
        let layout = volume.layout();
        assert_eq!((layout.fs_type(), layout.root_entries, layout.reserved_sectors), (b"FAT32   ", 0, 32));
        assert!(layout.clusters > 65535, "{}", layout.clusters);
        assert_eq!(layout.fat32.map(|f| (f.root_cluster, f.fsinfo, f.backup_boot)), Some((2, 1, 6)));
        // The root's cluster is taken.
        assert_eq!(volume.free_clusters(), layout.clusters - 1);
        assert_eq!(volume.label().as_deref(), Some("BIG ONE"));
        assert!(volume.serial().is_some());

        volume.mkdir(&["GAMES"]).unwrap();
        let at = volume.create(&["GAMES", "DOOM.WAD"], 0).unwrap().at.unwrap();
        let data: Vec<u8> = (0..5000u32).map(|i| i as u8).collect();
        assert_eq!(volume.write(at, 0, &data), Ok(5000));
        assert_eq!(contents(&volume, &["GAMES", "DOOM.WAD"]), data);
        assert_eq!(volume.free_clusters(), layout.clusters - 1 - 1 - 10);
        // ".." of a directory in the root is cluster 0.
        let games = volume.find(&["GAMES"]).unwrap();
        let mut dots = [0u8; SECTOR_SIZE];
        volume.read_sector(volume.state().params.cluster_sector(games.cluster), &mut dots).unwrap();
        assert_eq!((&dots[32..34], &dots[32 + 20..32 + 22], &dots[32 + 26..32 + 28]), (&b".."[..], &[0, 0][..], &[0, 0][..]));
        assert_eq!(volume.list(&["GAMES", ".."]).map(|l| l.len()), Ok(1));
        volume.remove(&["GAMES", "DOOM.WAD"]).unwrap();
        volume.rmdir(&["GAMES"]).unwrap();
        assert_eq!(volume.free_clusters(), layout.clusters - 1);
    }

    #[test]
    fn a_fat32_root_directory_grows() {
        let (disk, volume) = fat32("root32.img", 40, 1);
        // 16 entries a cluster; the label takes one.
        for i in 0..40 {
            volume.create(&[&format!("FILE{}", i)], 0).unwrap();
        }
        let root = volume.state().params.root_cluster;
        assert_eq!(volume.state().chain(root).len(), 3);
        assert_eq!(volume.list(&[]).unwrap().len(), 40);
        let (start, sectors) = disk.fat_volume().unwrap();
        let again = FatVolume::open(disk, start, sectors).unwrap();
        assert!(again.find(&["FILE39"]).is_ok());
    }

    #[test]
    fn fat32_clusters_past_65535() {
        let (disk, volume) = fat32("high32.img", 64, 1);
        volume.state().next_free = 70_000;
        let at = volume.create(&["HIGH.DAT"], 0).unwrap().at.unwrap();
        volume.write(at, 0, &[7u8; 1500]).unwrap();
        let entry = volume.find(&["HIGH.DAT"]).unwrap();
        assert_eq!(entry.cluster, 70_000);
        assert_eq!((fat_entry(&volume, 0, 70_000), fat_entry(&volume, 1, 70_002)), (70_001, 0x0FFF_FFFF));
        let (start, sectors) = disk.fat_volume().unwrap();
        let again = FatVolume::open(disk, start, sectors).unwrap();
        assert_eq!(contents(&again, &["HIGH.DAT"]), vec![7u8; 1500]);
        // The FSInfo sector has the hint for the next.
        let mut info = [0u8; SECTOR_SIZE];
        again.read_sector(1, &mut info).unwrap();
        assert_eq!(u32::from_le_bytes(info[FSINFO_NEXT..FSINFO_NEXT + 4].try_into().unwrap()), 70_003);
        assert_eq!(again.next_free(), 70_003);
    }

    #[test]
    fn fat32_entries_keep_their_top_bits_and_fsinfo_its_count() {
        let (_, volume) = fat32("bits32.img", 40, 1);
        // A free cluster with the reserved bits set is free all the same.
        let mut fat = [0u8; SECTOR_SIZE];
        volume.read_sector(32, &mut fat).unwrap();
        fat[12..16].copy_from_slice(&0xF000_0000u32.to_le_bytes());
        volume.write_sectors(32, &fat).unwrap();
        let free = volume.free_clusters();
        let at = volume.create(&["F"], 0).unwrap().at.unwrap();
        volume.write(at, 0, &[1]).unwrap();
        assert_eq!(volume.find(&["F"]).unwrap().cluster, 3);
        assert_eq!(fat_entry(&volume, 0, 3), 0xFFFF_FFFF);
        let mut info = [0u8; SECTOR_SIZE];
        volume.read_sector(1, &mut info).unwrap();
        assert_eq!(u32::from_le_bytes(info[FSINFO_FREE..FSINFO_FREE + 4].try_into().unwrap()), free - 1);
    }

    #[test]
    fn fat32_without_mirroring_writes_its_active_fat() {
        let (disk, volume) = fat32("mirror32.img", 40, 1);
        let mut boot = [0u8; SECTOR_SIZE];
        volume.read_sector(0, &mut boot).unwrap();
        boot[0x28] = 0x81;
        volume.write_sectors(0, &boot).unwrap();
        let (start, sectors) = disk.fat_volume().unwrap();
        let volume = FatVolume::open(disk, start, sectors).unwrap();
        let at = volume.create(&["F"], 0).unwrap().at.unwrap();
        volume.write(at, 0, &[1]).unwrap();
        assert_eq!((fat_entry(&volume, 0, 3), fat_entry(&volume, 1, 3)), (0, 0x0FFF_FFFF));
        assert_eq!(contents(&volume, &["F"]), [1]);
    }

    #[test]
    fn hard_disk_volumes_are_fat16() {
        let path = image("hdd.img", 16 * 63 * 512 * 20);
        let disk = Rc::new(DiskImage::open(&path, false, None, false).unwrap());
        format(&disk, 63, disk.sectors() - 63, None).unwrap();
        let volume = FatVolume::open(disk.clone(), 63, disk.sectors() - 63).unwrap();
        let layout = volume.layout();
        assert_eq!((layout.media, layout.hidden_sectors, layout.fs_type()), (0xF8, 63, b"FAT16   "));
        assert!(layout.clusters as u64 >= FAT12_MAX_CLUSTERS);
        let at = volume.create(&["X"], 0).unwrap().at.unwrap();
        volume.write(at, 0, &vec![9u8; 100_000]).unwrap();
        assert_eq!(contents(&volume, &["X"]), vec![9u8; 100_000]);
        assert_eq!(volume.label(), None);
    }

    #[test]
    fn dos_1_floppies_have_no_bpb() {
        let path = image("dos1.img", 368_640);
        let disk = Rc::new(DiskImage::open(&path, true, None, false).unwrap());
        format(&disk, 0, 720, None).unwrap();
        disk.write(0, &[0u8; SECTOR_SIZE]).unwrap();
        let volume = FatVolume::open(disk, 0, 720).unwrap();
        let layout = volume.layout();
        assert_eq!((layout.media, layout.sectors_per_cluster, layout.root_entries), (0xFD, 2, 112));
        volume.create(&["WORKS"], 0).unwrap();
    }
}
