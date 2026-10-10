//! A CD made from a folder of the host: an ISO 9660 image with Joliet
//! names, for a system booted from a disk image, which sees CDs only
//! through its CD-ROM drive. The directories and volume descriptors are
//! built in memory; the files' sectors are read from the host files when
//! the drive reads them, so a big folder takes no more memory than its
//! directories. It is a snapshot: files the host adds later need the disc
//! made again.
//!
//! The primary directory tree has the DOS names the built-in DOS shows
//! for the folder, for DOS and MSCDEX; the Joliet tree has the host's
//! names, up to 64 characters, for Windows 95 and later.

use super::DATA_SECTOR;
use crate::diskimage::MemoryImage;
use crate::hostfs;
use chrono::{DateTime, Datelike, Local, Offset, Timelike};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A file whose sectors are those of a host file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostExtent {
    pub lba: u32,
    pub len: u64,
    pub path: PathBuf,
}

/// The disc made from a folder.
pub struct FolderImage {
    /// The sectors before the first file's: volume descriptors, path
    /// tables and directories.
    pub meta: MemoryImage,
    /// The files, by their first sector.
    pub files: Vec<HostExtent>,
    /// Sectors on the disc.
    pub sectors: u32,
    /// What was left out, and why.
    pub skipped: Vec<String>,
}

/// Sector of the Primary Volume Descriptor.
const PVD: u32 = 16;
/// Joliet names are at most 64 characters, as Windows reads them.
const JOLIET_MAX: usize = 64;
/// How deep the folder is read.
const MAX_DEPTH: usize = 32;
/// Sectors of zeros at the end of the disc.
const PADDING: u32 = 150;
/// A file of an extent: its size must fit 32 bits.
const MAX_FILE: u64 = u32::MAX as u64;

struct File {
    primary: String,
    joliet: Vec<u16>,
    len: u64,
    time: [u8; 7],
    path: PathBuf,
    lba: u32,
}

struct Dir {
    primary: String,
    joliet: Vec<u16>,
    time: [u8; 7],
    parent: usize,
    dirs: Vec<usize>,
    files: Vec<File>,
    /// Where each tree has the directory: (sector, bytes), and its number
    /// in that tree's path table.
    at: [(u32, u32); 2],
    number: [u16; 2],
}

/// Which directory tree: the primary one or Joliet's.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tree {
    Primary = 0,
    Joliet = 1,
}

/// Make the disc for the folder `root`, labelled `label`.
pub fn build(root: &Path, label: &str) -> Result<FolderImage, String> {
    let meta = hostfs::metadata(root).map_err(|e| format!("{}: {}", root.display(), e))?;
    if !meta.is_dir {
        return Err(format!("{} is not a folder", root.display()));
    }
    let mut dirs = vec![Dir {
        primary: String::new(),
        joliet: Vec::new(),
        time: record_time(meta.modified),
        parent: 0,
        dirs: Vec::new(),
        files: Vec::new(),
        at: [(0, 0); 2],
        number: [1; 2],
    }];
    let mut skipped = Vec::new();
    let mut seen = HashSet::new();
    if let Ok(canonical) = hostfs::canonicalize(root) {
        seen.insert(canonical);
    }
    read_folder(&mut dirs, 0, root, 0, &mut seen, &mut skipped);

    // Each tree's directories in path table order: by level, then by
    // parent, then by name, which is breadth first with sorted children.
    let orders = [Tree::Primary, Tree::Joliet].map(|tree| {
        let mut order = vec![0usize];
        let mut i = 0;
        while i < order.len() {
            let mut children = dirs[order[i]].dirs.clone();
            children.sort_by(|&a, &b| dir_key(&dirs[a], tree).cmp(&dir_key(&dirs[b], tree)));
            order.extend(children);
            i += 1;
        }
        order
    });
    for tree in [Tree::Primary, Tree::Joliet] {
        for (n, &d) in orders[tree as usize].iter().enumerate() {
            dirs[d].number[tree as usize] = (n + 1).min(u16::MAX as usize) as u16;
        }
    }

    // Path tables from sector 19 on: the primary tree's L and M, then
    // Joliet's.
    let table_len = [Tree::Primary, Tree::Joliet]
        .map(|tree| orders[tree as usize].iter().map(|&d| path_record_len(&dirs[d], tree) as u32).sum::<u32>());
    let mut next = PVD + 3;
    let mut tables = [[0u32; 2]; 2];
    for tree in [Tree::Primary, Tree::Joliet] {
        for table in &mut tables[tree as usize] {
            *table = next;
            next += table_len[tree as usize].div_ceil(DATA_SECTOR as u32).max(1);
        }
    }
    // Then each tree's directories, in path table order.
    for tree in [Tree::Primary, Tree::Joliet] {
        for &d in &orders[tree as usize] {
            let bytes = directory_len(&dirs, d, tree);
            dirs[d].at[tree as usize] = (next, bytes);
            next += bytes / DATA_SECTOR as u32;
        }
    }
    let meta_sectors = next;
    // Then the files, in the primary tree's order.
    let mut files = Vec::new();
    for &d in &orders[Tree::Primary as usize] {
        for file in &mut dirs[d].files {
            file.lba = next;
            let sectors = file.len.div_ceil(DATA_SECTOR as u64);
            if sectors > 0 {
                files.push(HostExtent { lba: next, len: file.len, path: file.path.clone() });
            }
            next = u32::try_from(next as u64 + sectors).map_err(|_| "The folder is too big for a CD".to_string())?;
        }
    }
    // Padding after the last file, as mastering programs leave it: some
    // readers read ahead past the end.
    let sectors = next + PADDING;

    let mut meta = MemoryImage::new(meta_sectors as u64 * DATA_SECTOR as u64);
    // The disc is made now: by the machine's clock, which deterministic
    // mode sets.
    let now = record_time(Some(crate::hosttime::now().into()));
    for tree in [Tree::Primary, Tree::Joliet] {
        let sector = PVD + tree as u32;
        let descriptor = volume_descriptor(&dirs, tree, label, sectors, table_len[tree as usize], tables[tree as usize], now);
        meta.write_at(sector as u64 * DATA_SECTOR as u64, &descriptor);
        let (little, big) = path_tables(&dirs, &orders[tree as usize], tree);
        meta.write_at(tables[tree as usize][0] as u64 * DATA_SECTOR as u64, &little);
        meta.write_at(tables[tree as usize][1] as u64 * DATA_SECTOR as u64, &big);
        for d in 0..dirs.len() {
            let (lba, _) = dirs[d].at[tree as usize];
            meta.write_at(lba as u64 * DATA_SECTOR as u64, &directory(&dirs, d, tree));
        }
    }
    let mut terminator = [0u8; DATA_SECTOR];
    terminator[0] = 0xFF;
    terminator[1..7].copy_from_slice(b"CD001\x01");
    meta.write_at((PVD + 2) as u64 * DATA_SECTOR as u64, &terminator);
    Ok(FolderImage { meta, files, sectors, skipped })
}

/// Add the contents of the host folder `path` to directory `d`.
fn read_folder(dirs: &mut Vec<Dir>, d: usize, path: &Path, depth: usize, seen: &mut HashSet<PathBuf>, skipped: &mut Vec<String>) {
    let Ok(entries) = hostfs::read_dir(path) else {
        skipped.push(format!("{}: can't be read", path.display()));
        return;
    };
    let mut entries: Vec<(String, PathBuf, hostfs::Meta)> = entries
        .into_iter()
        .filter_map(|e| {
            let name = e.name.to_string_lossy().into_owned();
            // Follows symbolic links.
            let meta = e.metadata().ok()?;
            Some((name, e.path, meta))
        })
        .filter(|(name, _, _)| name != "." && name != "..")
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.retain(|(_, path, meta)| {
        if meta.is_file() && meta.len > MAX_FILE {
            skipped.push(format!("{}: 4 GB or more doesn't fit a CD file", path.display()));
            return false;
        }
        meta.is_file() || meta.is_dir
    });
    // The names are unique among the files and directories together.
    let hosts: Vec<&str> = entries.iter().map(|(name, _, _)| name.as_str()).collect();
    let primaries = crate::disk::short_names(&hosts);
    let joliets = joliet_names(&hosts);
    let mut subdirs = Vec::new();
    for (((_, path, meta), primary), joliet) in entries.into_iter().zip(primaries).zip(joliets) {
        let time = record_time(meta.modified);
        if meta.is_dir {
            if depth + 1 >= MAX_DEPTH {
                skipped.push(format!("{}: too deep", path.display()));
                continue;
            }
            // A link back up would go on forever.
            if let Ok(canonical) = hostfs::canonicalize(&path)
                && !seen.insert(canonical)
            {
                skipped.push(format!("{}: a folder already on the disc", path.display()));
                continue;
            }
            let child = dirs.len();
            dirs.push(Dir { primary, joliet, time, parent: d, dirs: Vec::new(), files: Vec::new(), at: [(0, 0); 2], number: [0; 2] });
            dirs[d].dirs.push(child);
            subdirs.push((child, path));
        } else {
            let primary = if primary.contains('.') { format!("{};1", primary) } else { format!("{}.;1", primary) };
            dirs[d].files.push(File { primary, joliet, len: meta.len, time, path, lba: 0 });
        }
    }
    for (child, path) in subdirs {
        read_folder(dirs, child, &path, depth + 1, seen, skipped);
    }
}

/// The Joliet names of a directory's entries: the host names, with the
/// characters Joliet doesn't allow as "_", cut to 64 characters with a
/// number to keep them apart.
fn joliet_names(names: &[&str]) -> Vec<Vec<u16>> {
    let clean: Vec<Vec<u16>> = names
        .iter()
        .map(|name| {
            let name: String = name.chars().map(|c| if "*/:;?\\".contains(c) || c < ' ' { '_' } else { c }).collect();
            name.encode_utf16().collect()
        })
        .collect();
    let mut used: HashSet<Vec<u16>> = clean.iter().filter(|n| n.len() <= JOLIET_MAX).cloned().collect();
    let mut counter = 0u32;
    clean
        .into_iter()
        .map(|name| {
            if name.len() <= JOLIET_MAX {
                return name;
            }
            // Keep the extension, and as much of the start as fits.
            let text = String::from_utf16_lossy(&name);
            let ext: Vec<u16> = match text.rsplit_once('.') {
                Some((_, ext)) if ext.encode_utf16().count() <= 8 => format!(".{}", ext).encode_utf16().collect(),
                _ => Vec::new(),
            };
            loop {
                counter += 1;
                let suffix: Vec<u16> = format!("~{}", counter).encode_utf16().collect();
                let keep = JOLIET_MAX - suffix.len() - ext.len();
                let mut short: Vec<u16> = name[..keep].to_vec();
                // Not half of a surrogate pair.
                if short.last().is_some_and(|&u| (0xD800..0xDC00).contains(&u)) {
                    short.pop();
                }
                short.extend(&suffix);
                short.extend(&ext);
                if used.insert(short.clone()) {
                    return short;
                }
            }
        })
        .collect()
}

/// A directory's identifier bytes in a tree.
fn dir_id(dir: &Dir, tree: Tree) -> Vec<u8> {
    match tree {
        Tree::Primary => dir.primary.as_bytes().to_vec(),
        Tree::Joliet => ucs2(&dir.joliet),
    }
}

fn file_id(file: &File, tree: Tree) -> Vec<u8> {
    match tree {
        Tree::Primary => file.primary.as_bytes().to_vec(),
        Tree::Joliet => ucs2(&file.joliet),
    }
}

/// What sorts directories in a tree.
fn dir_key(dir: &Dir, tree: Tree) -> Vec<u8> {
    dir_id(dir, tree)
}

fn ucs2(name: &[u16]) -> Vec<u8> {
    name.iter().flat_map(|u| u.to_be_bytes()).collect()
}

fn path_record_len(dir: &Dir, tree: Tree) -> usize {
    let id = dir_id(dir, tree).len().max(1);
    8 + id + (id & 1)
}

fn record_len(id: usize) -> usize {
    33 + id + (1 - (id & 1))
}

/// The records of directory `d` in a tree: ".", "..", then its entries
/// sorted by identifier.
fn records(dirs: &[Dir], d: usize, tree: Tree) -> Vec<Vec<u8>> {
    let t = tree as usize;
    let dir = &dirs[d];
    let parent = &dirs[dir.parent];
    let mut records = vec![
        record(&[0], dir.at[t].0, dir.at[t].1, dir.time, true),
        record(&[1], parent.at[t].0, parent.at[t].1, parent.time, true),
    ];
    let mut entries: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for &c in &dir.dirs {
        let child = &dirs[c];
        let id = dir_id(child, tree);
        entries.push((id.clone(), record(&id, child.at[t].0, child.at[t].1, child.time, true)));
    }
    for file in &dir.files {
        let id = file_id(file, tree);
        let lba = if file.len == 0 { 0 } else { file.lba };
        entries.push((id.clone(), record(&id, lba, file.len as u32, file.time, false)));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    records.extend(entries.into_iter().map(|(_, r)| r));
    records
}

/// Bytes of directory `d`'s extent: whole sectors, as no record crosses
/// from one to the next.
fn directory_len(dirs: &[Dir], d: usize, tree: Tree) -> u32 {
    let mut sectors = 1u32;
    let mut used = 0;
    for r in records(dirs, d, tree) {
        if used + r.len() > DATA_SECTOR {
            sectors += 1;
            used = 0;
        }
        used += r.len();
    }
    sectors * DATA_SECTOR as u32
}

fn directory(dirs: &[Dir], d: usize, tree: Tree) -> Vec<u8> {
    let mut out = Vec::new();
    let mut used = 0;
    for r in records(dirs, d, tree) {
        if used + r.len() > DATA_SECTOR {
            out.resize(out.len() + DATA_SECTOR - used, 0);
            used = 0;
        }
        used += r.len();
        out.extend(r);
    }
    out.resize(out.len() + DATA_SECTOR - used, 0);
    out
}

fn both16(v: u16) -> [u8; 4] {
    let (l, b) = (v.to_le_bytes(), v.to_be_bytes());
    [l[0], l[1], b[0], b[1]]
}

fn both32(v: u32) -> [u8; 8] {
    let (l, b) = (v.to_le_bytes(), v.to_be_bytes());
    [l[0], l[1], l[2], l[3], b[0], b[1], b[2], b[3]]
}

fn record(id: &[u8], lba: u32, len: u32, time: [u8; 7], is_dir: bool) -> Vec<u8> {
    let size = record_len(id.len());
    let mut r = vec![0u8; size];
    r[0] = size as u8;
    r[2..10].copy_from_slice(&both32(lba));
    r[10..18].copy_from_slice(&both32(len));
    r[18..25].copy_from_slice(&time);
    r[25] = if is_dir { 0x02 } else { 0 };
    r[28..32].copy_from_slice(&both16(1));
    r[32] = id.len() as u8;
    r[33..33 + id.len()].copy_from_slice(id);
    r
}

/// The L and M path tables of a tree.
fn path_tables(dirs: &[Dir], order: &[usize], tree: Tree) -> (Vec<u8>, Vec<u8>) {
    let t = tree as usize;
    let (mut little, mut big) = (Vec::new(), Vec::new());
    for &d in order {
        let dir = &dirs[d];
        let id = if d == 0 { vec![0] } else { dir_id(dir, tree) };
        let parent = dirs[dir.parent].number[t];
        for (out, lba, parent) in [
            (&mut little, dir.at[t].0.to_le_bytes(), parent.to_le_bytes()),
            (&mut big, dir.at[t].0.to_be_bytes(), parent.to_be_bytes()),
        ] {
            out.push(id.len() as u8);
            out.push(0);
            out.extend(lba);
            out.extend(parent);
            out.extend(&id);
            if id.len() & 1 == 1 {
                out.push(0);
            }
        }
    }
    (little, big)
}

/// The Primary Volume Descriptor, or Joliet's Supplementary one.
fn volume_descriptor(
    dirs: &[Dir],
    tree: Tree,
    label: &str,
    sectors: u32,
    table_len: u32,
    tables: [u32; 2],
    now: [u8; 7],
) -> [u8; DATA_SECTOR] {
    let mut d = [0u8; DATA_SECTOR];
    d[0] = if tree == Tree::Primary { 1 } else { 2 };
    d[1..6].copy_from_slice(b"CD001");
    d[6] = 1;
    let text = |d: &mut [u8], at: usize, len: usize, value: &str| match tree {
        Tree::Primary => {
            let bytes: Vec<u8> = value.bytes().chain(std::iter::repeat(b' ')).take(len).collect();
            d[at..at + len].copy_from_slice(&bytes);
        }
        Tree::Joliet => {
            // Odd lengths end in a byte of zero.
            let units: Vec<u16> = value.encode_utf16().take(len / 2).chain(std::iter::repeat(0x20)).take(len / 2).collect();
            d[at..at + len / 2 * 2].copy_from_slice(&ucs2(&units));
        }
    };
    let volume: String = match tree {
        Tree::Primary => label
            .to_ascii_uppercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .take(32)
            .collect(),
        Tree::Joliet => label.to_string(),
    };
    text(&mut d, 8, 32, "");
    text(&mut d, 40, 32, &volume);
    d[80..88].copy_from_slice(&both32(sectors));
    if tree == Tree::Joliet {
        // UCS-2 level 3.
        d[88..91].copy_from_slice(b"%/E");
    }
    d[120..124].copy_from_slice(&both16(1));
    d[124..128].copy_from_slice(&both16(1));
    d[128..132].copy_from_slice(&both16(DATA_SECTOR as u16));
    d[132..140].copy_from_slice(&both32(table_len));
    d[140..144].copy_from_slice(&tables[0].to_le_bytes());
    d[148..152].copy_from_slice(&tables[1].to_be_bytes());
    let root = &dirs[0];
    let (lba, len) = root.at[tree as usize];
    d[156..190].copy_from_slice(&record(&[0], lba, len, root.time, true));
    for (at, len) in [(190, 128), (318, 128), (446, 128), (702, 37), (739, 37), (776, 37)] {
        text(&mut d, at, len, "");
    }
    text(&mut d, 574, 128, "RUST-DOS");
    let stamp = volume_time(now);
    d[813..830].copy_from_slice(&stamp);
    d[830..847].copy_from_slice(&stamp);
    d[847..863].copy_from_slice(b"0000000000000000");
    d[864..880].copy_from_slice(b"0000000000000000");
    d[881] = 1;
    d
}

/// A directory record's date and time: the host time `at` in local time,
/// with its offset from UTC in quarter hours.
fn record_time(at: Option<SystemTime>) -> [u8; 7] {
    let local: DateTime<Local> = at.unwrap_or(SystemTime::UNIX_EPOCH).into();
    let offset = local.offset().fix().local_minus_utc() / 900;
    [
        (local.year() - 1900).clamp(0, 255) as u8,
        local.month() as u8,
        local.day() as u8,
        local.hour() as u8,
        local.minute() as u8,
        local.second().min(59) as u8,
        offset as i8 as u8,
    ]
}

/// A volume descriptor's date and time, in digits, from a record's.
fn volume_time(t: [u8; 7]) -> [u8; 17] {
    let text = format!("{:04}{:02}{:02}{:02}{:02}{:02}00", 1900 + t[0] as u32, t[1], t[2], t[3], t[4], t[5]);
    let mut out = [0u8; 17];
    out[..16].copy_from_slice(text.as_bytes());
    out[16] = t[6];
    out
}

impl FolderImage {
    /// Fill `buf` from byte `at` of the disc. Sectors of a host file that
    /// got shorter or went away read as zeros.
    pub fn read_at(&self, at: u64, buf: &mut [u8], open: &mut OpenFiles) {
        buf.fill(0);
        let meta_len = self.meta.len();
        let mut done = 0;
        while done < buf.len() {
            let pos = at + done as u64;
            if pos < meta_len {
                let n = ((meta_len - pos) as usize).min(buf.len() - done);
                self.meta.read_at(pos, &mut buf[done..done + n]);
                done += n;
                continue;
            }
            let lba = (pos / DATA_SECTOR as u64) as u32;
            // The file whose extent holds the byte, if any.
            let i = self.files.partition_point(|f| f.lba <= lba);
            let Some(file) = i.checked_sub(1).map(|i| &self.files[i]) else { break };
            let start = file.lba as u64 * DATA_SECTOR as u64;
            let end = start + file.len.div_ceil(DATA_SECTOR as u64) * DATA_SECTOR as u64;
            if pos >= end {
                // Past the last file: nothing but zeros.
                let next = self.files.get(i).map_or(u64::MAX, |f| f.lba as u64 * DATA_SECTOR as u64);
                done += (next.saturating_sub(pos)).min((buf.len() - done) as u64) as usize;
                continue;
            }
            let n = ((end - pos) as usize).min(buf.len() - done);
            let offset = pos - start;
            if offset < file.len {
                let want = ((file.len - offset) as usize).min(n);
                open.read(&file.path, offset, &mut buf[done..done + want]);
            }
            done += n;
        }
    }
}

/// The host files a folder's disc read last, kept open.
#[derive(Default)]
pub struct OpenFiles {
    files: Vec<(PathBuf, hostfs::File)>,
}

impl OpenFiles {
    const KEEP: usize = 4;

    fn read(&mut self, path: &Path, at: u64, buf: &mut [u8]) {
        use std::io::{Read, Seek, SeekFrom};
        let index = match self.files.iter().position(|(p, _)| p == path) {
            Some(i) => i,
            None => {
                let Ok(file) = hostfs::File::open(path) else { return };
                if self.files.len() >= Self::KEEP {
                    self.files.remove(0);
                }
                self.files.push((path.to_path_buf(), file));
                self.files.len() - 1
            }
        };
        let file = &mut self.files[index].1;
        if file.seek(SeekFrom::Start(at)).is_err() {
            return;
        }
        let mut done = 0;
        while done < buf.len() {
            match file.read(&mut buf[done..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => done += n,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdrom::image::CdImage;
    use crate::cdrom::iso9660;
    use crate::memfs::Node;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from("target/test_cdfolder").join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_contents(image: &CdImage, files: &crate::memfs::MemFs, path: &str) -> Vec<u8> {
        let Some(Node::Extent(extent)) = files.file(path) else { panic!("{} is not a file", path) };
        let mut data = vec![0u8; extent.size as usize];
        assert_eq!(image.read_extent(extent, 0, &mut data).unwrap(), data.len());
        data
    }

    #[test]
    fn a_folder_reads_back_through_iso9660() {
        let root = scratch("roundtrip");
        fs::write(root.join("readme.txt"), b"hello").unwrap();
        fs::write(root.join("Day Of The Tentacle.txt"), vec![7u8; 5000]).unwrap();
        fs::write(root.join("Day Of The Tentacle.dat"), b"").unwrap();
        fs::write(root.join("Day Of The Tentacle again.txt"), b"x").unwrap();
        let mut deep = root.clone();
        for level in 0..9 {
            deep = deep.join(format!("level{}", level));
        }
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("bottom.bin"), vec![3u8; 3 * 2048 + 1]).unwrap();
        fs::create_dir_all(root.join("Empty Folder")).unwrap();

        let disc = build(&root, "My Stuff").unwrap();
        assert!(disc.skipped.is_empty(), "{:?}", disc.skipped);
        let image = CdImage::from_folder(disc, &root).unwrap();
        let volume = iso9660::read_volume(&image).unwrap();
        assert_eq!(volume.label, "MY_STUFF");
        assert_eq!(file_contents(&image, &volume.files, "README.TXT"), b"hello");
        // Numbered in the order of the host names.
        assert_eq!(file_contents(&image, &volume.files, "DAYOFT~1.TXT"), b"x");
        assert_eq!(file_contents(&image, &volume.files, "DAYOFT~2.DAT"), b"");
        assert_eq!(file_contents(&image, &volume.files, "DAYOFT~3.TXT"), vec![7u8; 5000]);
        assert!(volume.files.is_dir("EMPTYF~1"));
        // ISO 9660 readers stop at eight levels; the sectors are there.
        assert!(volume.files.is_dir("LEVEL0\\LEVEL1\\LEVEL2\\LEVEL3\\LEVEL4\\LEVEL5\\LEVEL6\\LEVEL7"));
    }

    /// The names in a directory of the Joliet tree.
    fn joliet_listing(image: &CdImage) -> Vec<String> {
        let mut svd = [0u8; DATA_SECTOR];
        image.read_data(17, &mut svd).unwrap();
        assert_eq!((svd[0], &svd[1..6], &svd[88..91]), (2, &b"CD001"[..], &b"%/E"[..]));
        let lba = u32::from_le_bytes(svd[158..162].try_into().unwrap());
        let mut sector = [0u8; DATA_SECTOR];
        image.read_data(lba, &mut sector).unwrap();
        let mut names = Vec::new();
        let mut at = 0;
        while sector[at] != 0 {
            let len = sector[at] as usize;
            let id = &sector[at + 33..at + 33 + sector[at + 32] as usize];
            if id != [0] && id != [1] {
                let units: Vec<u16> = id.chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                names.push(String::from_utf16_lossy(&units));
            }
            at += len;
        }
        names
    }

    #[test]
    fn the_disc_is_made_at_the_machines_time() {
        let root = scratch("made");
        fs::write(root.join("a.txt"), b"1").unwrap();
        let at = chrono::NaiveDate::from_ymd_opt(1995, 4, 11).unwrap().and_hms_opt(12, 34, 56).unwrap();
        crate::hosttime::fix(Some(at));
        let image = CdImage::from_folder(build(&root, "x").unwrap(), &root).unwrap();
        crate::hosttime::fix(None);
        let mut pvd = [0u8; DATA_SECTOR];
        image.read_data(PVD, &mut pvd).unwrap();
        // Created and modified, as digits: 1995-04-11 12:34:56.00.
        assert_eq!(&pvd[813..829], b"1995041112345600");
        assert_eq!(&pvd[830..846], b"1995041112345600");
    }

    #[test]
    fn joliet_has_the_long_names() {
        let root = scratch("joliet");
        fs::write(root.join("Übersicht der Dateien.txt"), b"1").unwrap();
        fs::write(root.join("a:b?.txt"), b"2").unwrap();
        let long = format!("{}.document", "x".repeat(80));
        fs::write(root.join(&long), b"3").unwrap();
        fs::create_dir(root.join("Saved Games")).unwrap();
        let image = CdImage::from_folder(build(&root, "Stuff").unwrap(), &root).unwrap();
        let names = joliet_listing(&image);
        assert!(names.contains(&"Übersicht der Dateien.txt".to_string()), "{:?}", names);
        assert!(names.contains(&"a_b_.txt".to_string()));
        assert!(names.contains(&"Saved Games".to_string()));
        let cut = format!("{}~1.document", "x".repeat(64 - 2 - 9));
        assert!(names.contains(&cut), "{:?}", names);
        assert!(iso9660::primary_descriptor(&image).is_ok());
    }

    #[test]
    fn a_file_that_shrinks_reads_as_zeros() {
        let root = scratch("shrink");
        fs::write(root.join("a.bin"), vec![1u8; 4096]).unwrap();
        fs::write(root.join("b.bin"), vec![2u8; 100]).unwrap();
        let image = CdImage::from_folder(build(&root, "x").unwrap(), &root).unwrap();
        fs::write(root.join("a.bin"), vec![9u8; 10]).unwrap();
        let volume = iso9660::read_volume(&image).unwrap();
        let a = file_contents(&image, &volume.files, "A.BIN");
        assert_eq!(a.len(), 4096);
        assert_eq!(&a[..10], &[9u8; 10]);
        assert!(a[10..].iter().all(|&b| b == 0));
        assert_eq!(file_contents(&image, &volume.files, "B.BIN"), vec![2u8; 100]);
    }
}

