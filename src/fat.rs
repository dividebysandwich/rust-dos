//! FAT12 and FAT16 file systems on floppy and hard disk images, for the
//! drives mounted from them.
//!
//! A volume keeps only its boot sector's parameters and the first FAT in
//! memory and reads directories and files from the image every time. Any
//! write to the image that the volume didn't make itself (INT 13h, INT 26h)
//! makes it read the boot sector and the FAT again, so DOS always sees what
//! is on the disk.
//!
//! Paths are the components of a DOS path from the root ("GAMES", "DOOM"),
//! with "." and ".." already folded away.

use std::cell::{RefCell, RefMut};
use std::collections::BTreeSet;
use std::rc::Rc;

use chrono::{Datelike, Local, Timelike};

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
const READ_FAULT: u8 = 0x1E;

/// Volumes with fewer clusters than this have 12-bit FATs, and with fewer
/// than `FAT16_MAX_CLUSTERS` 16-bit ones.
const FAT12_MAX_CLUSTERS: u64 = 4085;
const FAT16_MAX_CLUSTERS: u64 = 65525;

/// The current local time as a directory entry has it: (time, date).
pub fn dos_now() -> (u16, u16) {
    let t = Local::now();
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
    pub cluster: u16,
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
            cluster: word(26),
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

/// Where the parts of a volume are, from its BPB.
#[derive(Clone, Copy, Debug)]
struct Params {
    bpb: Bpb,
    fat16: bool,
    root_start: u64,
    root_sectors: u64,
    data_start: u64,
    /// Data clusters on the volume: cluster numbers 2 to `clusters + 1`.
    clusters: u32,
    cluster_bytes: u64,
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
        // lie past the end of an image cut short can't be used.
        let fat16 = match (total - data_start) / per_cluster {
            n if n < FAT12_MAX_CLUSTERS => false,
            n if n < FAT16_MAX_CLUSTERS => true,
            _ => return Err("FAT32 file systems aren't supported".to_string()),
        };
        let fat_bytes = bpb.sectors_per_fat as u64 * SECTOR_SIZE as u64;
        let fat_entries = if fat16 { fat_bytes / 2 } else { fat_bytes * 2 / 3 };
        let clusters = ((total.min(volume_sectors).saturating_sub(data_start)) / per_cluster)
            .min(fat_entries.saturating_sub(2)) as u32;
        Ok(Params {
            bpb,
            fat16,
            root_start,
            root_sectors,
            data_start,
            clusters,
            cluster_bytes: per_cluster * SECTOR_SIZE as u64,
        })
    }

    fn cluster_sector(&self, cluster: u32) -> u64 {
        self.data_start + (cluster as u64 - 2) * self.bpb.sectors_per_cluster as u64
    }
}

/// What the volume keeps in memory.
struct State {
    params: Params,
    /// The first FAT.
    fat: Vec<u8>,
    /// FAT sectors changed in memory and not yet written to the image.
    dirty: BTreeSet<u64>,
    /// The image's write count the FAT was read at, or last written at.
    generation: u64,
    /// Where to look for a free cluster first.
    next_free: u32,
}

impl State {
    fn max_cluster(&self) -> u32 {
        self.params.clusters + 1
    }

    fn get(&self, n: u32) -> u32 {
        let byte = |i: usize| self.fat.get(i).copied().unwrap_or(0) as u32;
        if self.params.fat16 {
            let at = n as usize * 2;
            byte(at) | byte(at + 1) << 8
        } else {
            let at = n as usize * 3 / 2;
            let pair = byte(at) | byte(at + 1) << 8;
            if n & 1 == 1 { pair >> 4 } else { pair & 0xFFF }
        }
    }

    fn set(&mut self, n: u32, value: u32) {
        let at = if self.params.fat16 { n as usize * 2 } else { n as usize * 3 / 2 };
        if at + 1 >= self.fat.len() {
            return;
        }
        if self.params.fat16 {
            self.fat[at..at + 2].copy_from_slice(&(value as u16).to_le_bytes());
        } else if n & 1 == 1 {
            self.fat[at] = (self.fat[at] & 0x0F) | (value << 4) as u8;
            self.fat[at + 1] = (value >> 4) as u8;
        } else {
            self.fat[at] = value as u8;
            self.fat[at + 1] = (self.fat[at + 1] & 0xF0) | ((value >> 8) & 0x0F) as u8;
        }
        self.dirty.insert((at / SECTOR_SIZE) as u64);
        self.dirty.insert(((at + 1) / SECTOR_SIZE) as u64);
    }

    fn end_of_chain(&self) -> u32 {
        if self.params.fat16 { 0xFFFF } else { 0xFFF }
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

    /// Take a free cluster and end a chain with it.
    fn allocate(&mut self) -> Option<u32> {
        let max = self.max_cluster();
        let start = self.next_free.clamp(2, max.max(2));
        let found = (start..=max).chain(2..start).find(|&c| self.get(c) == 0)?;
        self.set(found, self.end_of_chain());
        self.next_free = found + 1;
        Some(found)
    }

    fn free_chain(&mut self, first: u32) {
        for c in self.chain(first) {
            self.set(c, 0);
        }
    }

    fn free_clusters(&self) -> u32 {
        (2..=self.max_cluster()).filter(|&c| self.get(c) == 0).count() as u32
    }
}

/// A FAT12 or FAT16 file system on a disk image.
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

    fn load(disk: &DiskImage, start: u64, sectors: u64) -> Result<State, String> {
        let mut boot = [0u8; SECTOR_SIZE];
        disk.read(start, &mut boot).map_err(|_| "Can't read the boot sector".to_string())?;
        let bpb = match Bpb::parse(&boot) {
            Some(bpb) => bpb,
            None if disk.is_floppy() => dos1_bpb(disk)?,
            None => return Err("No FAT file system on the disk".to_string()),
        };
        let params = Params::new(bpb, sectors)?;
        let mut fat = vec![0u8; bpb.sectors_per_fat as usize * SECTOR_SIZE];
        disk.read(start + bpb.reserved_sectors as u64, &mut fat).map_err(|_| "Can't read the FAT".to_string())?;
        Ok(State { params, fat, dirty: BTreeSet::new(), generation: disk.generation(), next_free: 2 })
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

    /// Write the changed FAT sectors to every FAT, and take the image's
    /// state as the volume's own.
    fn finish(&self, state: &mut State) -> Result<(), u8> {
        let bpb = state.params.bpb;
        let dirty = std::mem::take(&mut state.dirty);
        let mut result = Ok(());
        for sector in dirty {
            let at = sector as usize * SECTOR_SIZE;
            let data = &state.fat[at..at + SECTOR_SIZE];
            for copy in 0..bpb.fats as u64 {
                let target = bpb.reserved_sectors as u64 + copy * bpb.sectors_per_fat as u64 + sector;
                if self.write_sector(target, data).is_err() {
                    result = Err(WRITE_FAULT);
                }
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
            clusters: p.clusters.min(u16::MAX as u32) as u16,
            reserved_sectors: p.bpb.reserved_sectors,
            fats: p.bpb.fats as u16,
            root_entries: p.bpb.root_entries,
            sectors_per_fat: p.bpb.sectors_per_fat,
            sectors_per_track: p.bpb.sectors_per_track,
            heads: p.bpb.heads,
            hidden_sectors: p.bpb.hidden_sectors,
            media: p.bpb.media,
            sectors: p.bpb.total_sectors,
        }
    }

    pub fn free_clusters(&self) -> u32 {
        self.state().free_clusters()
    }

    /// The boot sector's extended BPB (signature 29h at 26h), if it has one.
    fn extended_bpb(&self) -> Option<[u8; SECTOR_SIZE]> {
        let mut boot = [0u8; SECTOR_SIZE];
        self.read_sector(0, &mut boot).ok()?;
        (boot[0x26] == 0x29).then_some(boot)
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
            let boot = self.extended_bpb()?;
            let label: String = boot[0x2B..0x36].iter().map(|&b| b as char).collect();
            (label.trim() != "NO NAME").then_some(label)
        })?;
        let label = label.trim_end().to_string();
        (!label.is_empty()).then_some(label)
    }

    /// The volume serial number from the boot sector.
    pub fn serial(&self) -> Option<u32> {
        let boot = self.extended_bpb()?;
        Some(u32::from_le_bytes([boot[0x27], boot[0x28], boot[0x29], boot[0x2A]]))
    }

    /// The sectors a directory's entries are in.
    fn dir_sectors(&self, state: &State, dir: &Entry) -> Vec<u64> {
        let p = &state.params;
        if dir.cluster == 0 {
            return (p.root_start..p.root_start + p.root_sectors).collect();
        }
        let per_cluster = p.bpb.sectors_per_cluster as u64;
        state
            .chain(dir.cluster as u32)
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
            current = Entry::parse(&raw, at);
            // ".." of a directory in the root points at cluster 0.
            if current.is_dir() && current.cluster == 0 {
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
            .map(|(at, raw)| Entry::parse(&raw, at))
            .collect())
    }

    fn raw_entry(&self, at: EntryRef) -> Result<[u8; SECTOR_SIZE], u8> {
        let mut buf = [0u8; SECTOR_SIZE];
        self.read_sector(at.sector, &mut buf)?;
        Ok(buf)
    }

    /// The entry at `at` as it is on the disk now.
    pub fn reload(&self, at: EntryRef) -> Result<Entry, u8> {
        let _state = self.state();
        let buf = self.raw_entry(at)?;
        let raw = &buf[at.index * ENTRY_SIZE..(at.index + 1) * ENTRY_SIZE];
        if raw[0] == 0 || raw[0] == DELETED {
            return Err(FILE_NOT_FOUND);
        }
        Ok(Entry::parse(raw, at))
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
        let mut cluster = file.cluster as u32;
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
        let mut entry = Entry::parse(&buf[at.index * ENTRY_SIZE..(at.index + 1) * ENTRY_SIZE], at);
        let old_size = entry.size as u64;
        let end = (offset + data.len() as u64).min(u32::MAX as u64);
        let cluster_bytes = state.params.cluster_bytes;

        let mut chain = if entry.cluster == 0 { Vec::new() } else { state.chain(entry.cluster as u32) };
        let needed = end.div_ceil(cluster_bytes) as usize;
        while chain.len() < needed {
            let Some(cluster) = state.allocate() else { break };
            match chain.last() {
                Some(&last) => state.set(last, cluster),
                None => entry.cluster = cluster as u16,
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
            self.update_entry(at, |raw| {
                raw[11] |= ATTR_ARCHIVE;
                raw[22..24].copy_from_slice(&time.to_le_bytes());
                raw[24..26].copy_from_slice(&date.to_le_bytes());
                raw[26..28].copy_from_slice(&entry.cluster.to_le_bytes());
                raw[28..32].copy_from_slice(&size.to_le_bytes());
            })
        })();
        let finished = self.finish(&mut state);
        result.and(finished).map(|()| written)
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

    /// A free entry slot in `dir`, growing a subdirectory by a cluster when
    /// it's full. The root directory can't grow: 05h.
    fn free_slot(&self, state: &mut State, dir: &Entry) -> Result<EntryRef, u8> {
        let mut buf = [0u8; SECTOR_SIZE];
        let sectors = self.dir_sectors(state, dir);
        for &sector in &sectors {
            self.read_sector(sector, &mut buf)?;
            if let Some(index) = (0..ENTRIES_PER_SECTOR).find(|i| matches!(buf[i * ENTRY_SIZE], 0 | DELETED)) {
                return Ok(EntryRef { sector, index });
            }
        }
        if dir.cluster == 0 {
            return Err(ACCESS_DENIED);
        }
        let last = *state.chain(dir.cluster as u32).last().ok_or(ACCESS_DENIED)?;
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
    fn new_entry(name: &[u8; 11], attr: u8, cluster: u16) -> [u8; ENTRY_SIZE] {
        let (time, date) = dos_now();
        let mut raw = [0u8; ENTRY_SIZE];
        raw[..11].copy_from_slice(name);
        raw[11] = attr;
        raw[14..16].copy_from_slice(&time.to_le_bytes());
        raw[16..18].copy_from_slice(&date.to_le_bytes());
        raw[18..20].copy_from_slice(&date.to_le_bytes());
        raw[22..24].copy_from_slice(&time.to_le_bytes());
        raw[24..26].copy_from_slice(&date.to_le_bytes());
        raw[26..28].copy_from_slice(&cluster.to_le_bytes());
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
            Ok(Entry::parse(&raw, at))
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
            state.free_chain(entry.cluster as u32);
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
            dots[..ENTRY_SIZE].copy_from_slice(&Self::new_entry(b".          ", ATTR_DIRECTORY, cluster as u16));
            dots[ENTRY_SIZE..2 * ENTRY_SIZE]
                .copy_from_slice(&Self::new_entry(b"..         ", ATTR_DIRECTORY, parent.cluster));
            self.write_sector(first, &dots)?;
            self.write_raw(slot, &Self::new_entry(&name, ATTR_DIRECTORY, cluster as u16))
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
            state.free_chain(entry.cluster as u32);
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
