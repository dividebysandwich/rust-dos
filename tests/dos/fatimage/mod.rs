//! Small FAT disk images for the tests, laid out byte by byte without the
//! emulator's FAT driver: floppies of the standard sizes and a hard disk
//! with a partition table, holding a few files and directories.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const SECTOR: usize = 512;

/// A fresh directory for one test's images.
pub fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from("target/test_fatimage").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(&dir).unwrap()
}

/// How a FAT volume is laid out.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub sectors_per_track: u16,
    pub heads: u16,
    pub cylinders: u16,
    pub sectors_per_cluster: u8,
    pub root_entries: u16,
    pub sectors_per_fat: u16,
    pub media: u8,
    /// The volume's first sector on the disk.
    pub start: u32,
    /// Sectors in the volume.
    pub sectors: u32,
}

impl Layout {
    pub fn disk_sectors(&self) -> usize {
        self.sectors_per_track as usize * self.heads as usize * self.cylinders as usize
    }

    pub fn root_start(&self) -> usize {
        1 + 2 * self.sectors_per_fat as usize
    }

    pub fn data_start(&self) -> usize {
        self.root_start() + self.root_entries as usize * 32 / SECTOR
    }

    pub fn clusters(&self) -> usize {
        (self.sectors as usize - self.data_start()) / self.sectors_per_cluster as usize
    }

    fn fat16(&self) -> bool {
        self.clusters() >= 4085
    }
}

pub const FLOPPY_1440: Layout = Layout {
    sectors_per_track: 18,
    heads: 2,
    cylinders: 80,
    sectors_per_cluster: 1,
    root_entries: 224,
    sectors_per_fat: 9,
    media: 0xF0,
    start: 0,
    sectors: 2880,
};

pub const FLOPPY_720: Layout = Layout {
    sectors_per_track: 9,
    heads: 2,
    cylinders: 80,
    sectors_per_cluster: 2,
    root_entries: 112,
    sectors_per_fat: 3,
    media: 0xF9,
    start: 0,
    sectors: 1440,
};

/// 5 cylinders of 16 heads x 63 sectors, one FAT16 partition from sector 63.
pub const HARD_DISK: Layout = Layout {
    sectors_per_track: 63,
    heads: 16,
    cylinders: 5,
    sectors_per_cluster: 1,
    root_entries: 512,
    sectors_per_fat: 20,
    media: 0xF8,
    start: 63,
    sectors: 5040 - 63,
};

/// The 11-byte directory entry name of "NAME.EXT".
fn entry_name(name: &str) -> [u8; 11] {
    let (stem, ext) = name.split_once('.').unwrap_or((name, ""));
    let mut raw = [b' '; 11];
    raw[..stem.len()].copy_from_slice(stem.as_bytes());
    raw[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    raw
}

fn entry(name: [u8; 11], attr: u8, cluster: u16, size: u32) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(&name);
    e[11] = attr;
    e[22..24].copy_from_slice(&0x6000u16.to_le_bytes()); // 12:00:00
    e[24..26].copy_from_slice(&0x5021u16.to_le_bytes()); // 1 January 2020
    e[26..28].copy_from_slice(&cluster.to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    e
}

/// Every directory of an image and its entries: names, and the contents of
/// the files (None for a subdirectory).
type Tree<'a> = BTreeMap<String, Vec<(String, Option<&'a [u8]>)>>;

/// A disk image of `layout` holding `files` ("DIR\\NAME.EXT", contents)
/// and the empty directories `dirs`, with a volume label if given.
pub fn image(layout: Layout, label: Option<&str>, dirs: &[&str], files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut disk = vec![0u8; layout.disk_sectors() * SECTOR];
    let vol = layout.start as usize * SECTOR;
    let cluster_bytes = layout.sectors_per_cluster as usize * SECTOR;

    // The tree: every directory with its entries, parents first.
    let mut tree: Tree = BTreeMap::new();
    tree.insert(String::new(), Vec::new());
    let add_dir = |tree: &mut Tree, path: &str| {
        let mut parent = String::new();
        for part in path.split('\\') {
            let full = if parent.is_empty() { part.to_string() } else { format!("{}\\{}", parent, part) };
            if !tree.contains_key(&full) {
                tree.get_mut(&parent).unwrap().push((part.to_string(), None));
                tree.insert(full.clone(), Vec::new());
            }
            parent = full;
        }
    };
    for dir in dirs {
        add_dir(&mut tree, dir);
    }
    for (path, data) in files {
        let (parent, name) = path.rsplit_once('\\').unwrap_or(("", path));
        if !parent.is_empty() {
            add_dir(&mut tree, parent);
        }
        tree.get_mut(parent).unwrap().push((name.to_string(), Some(data)));
    }

    // Clusters, in order: each subdirectory, then each file.
    let mut next = 2usize;
    let mut fat_entries: Vec<(usize, usize)> = Vec::new(); // (cluster, next or 0xFFFF)
    let mut alloc = |count: usize| -> usize {
        if count == 0 {
            return 0;
        }
        let first = next;
        for i in 0..count {
            let link = if i + 1 == count { 0xFFFF } else { first + i + 1 };
            fat_entries.push((first + i, link));
        }
        next += count;
        first
    };
    let mut dir_cluster: BTreeMap<String, usize> = BTreeMap::new();
    dir_cluster.insert(String::new(), 0);
    for (path, entries) in &tree {
        if !path.is_empty() {
            let bytes = (entries.len() + 2) * 32;
            dir_cluster.insert(path.clone(), alloc(bytes.div_ceil(cluster_bytes)));
        }
    }
    let cluster_offset = |c: usize| vol + (layout.data_start() + (c - 2) * layout.sectors_per_cluster as usize) * SECTOR;

    for (path, entries) in &tree {
        let mut raw = Vec::new();
        let own = dir_cluster[path];
        if !path.is_empty() {
            let parent = path.rsplit_once('\\').map_or("", |(p, _)| p);
            raw.extend(entry(*b".          ", 0x10, own as u16, 0));
            raw.extend(entry(*b"..         ", 0x10, dir_cluster[parent] as u16, 0));
        } else if let Some(label) = label {
            let mut name = [b' '; 11];
            name[..label.len()].copy_from_slice(label.as_bytes());
            raw.extend(entry(name, 0x08, 0, 0));
        }
        for (name, data) in entries {
            match data {
                None => {
                    let full = if path.is_empty() { name.clone() } else { format!("{}\\{}", path, name) };
                    raw.extend(entry(entry_name(name), 0x10, dir_cluster[&full] as u16, 0));
                }
                Some(data) => {
                    let first = alloc(data.len().div_ceil(cluster_bytes));
                    if first != 0 {
                        let at = cluster_offset(first);
                        disk[at..at + data.len()].copy_from_slice(data);
                    }
                    raw.extend(entry(entry_name(name), 0x20, first as u16, data.len() as u32));
                }
            }
        }
        let at = if path.is_empty() { vol + layout.root_start() * SECTOR } else { cluster_offset(own) };
        disk[at..at + raw.len()].copy_from_slice(&raw);
    }

    // The FATs.
    let mut fat = vec![0u8; layout.sectors_per_fat as usize * SECTOR];
    let mut set = |n: usize, value: usize| {
        if layout.fat16() {
            fat[n * 2..n * 2 + 2].copy_from_slice(&(value as u16).to_le_bytes());
        } else {
            let value = value & 0xFFF;
            let at = n * 3 / 2;
            if n & 1 == 1 {
                fat[at] = (fat[at] & 0x0F) | (value << 4) as u8;
                fat[at + 1] = (value >> 4) as u8;
            } else {
                fat[at] = value as u8;
                fat[at + 1] = (fat[at + 1] & 0xF0) | (value >> 8) as u8;
            }
        }
    };
    set(0, 0xFF00 | layout.media as usize);
    set(1, 0xFFFF);
    for (cluster, link) in fat_entries {
        set(cluster, link);
    }
    for copy in 0..2 {
        let at = vol + (1 + copy * layout.sectors_per_fat as usize) * SECTOR;
        disk[at..at + fat.len()].copy_from_slice(&fat);
    }

    // The boot sector.
    let b = &mut disk[vol..vol + SECTOR];
    b[..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
    b[3..11].copy_from_slice(b"MSDOS5.0");
    b[0x0B..0x0D].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    b[0x0D] = layout.sectors_per_cluster;
    b[0x0E..0x10].copy_from_slice(&1u16.to_le_bytes());
    b[0x10] = 2;
    b[0x11..0x13].copy_from_slice(&layout.root_entries.to_le_bytes());
    b[0x13..0x15].copy_from_slice(&(layout.sectors as u16).to_le_bytes());
    b[0x15] = layout.media;
    b[0x16..0x18].copy_from_slice(&layout.sectors_per_fat.to_le_bytes());
    b[0x18..0x1A].copy_from_slice(&layout.sectors_per_track.to_le_bytes());
    b[0x1A..0x1C].copy_from_slice(&layout.heads.to_le_bytes());
    b[0x1C..0x20].copy_from_slice(&layout.start.to_le_bytes());
    b[0x26] = 0x29;
    b[0x27..0x2B].copy_from_slice(&0xCAFE_F00Du32.to_le_bytes());
    b[0x2B..0x36].copy_from_slice(b"NO NAME    ");
    b[0x36..0x3E].copy_from_slice(if layout.fat16() { b"FAT16   " } else { b"FAT12   " });
    b[510] = 0x55;
    b[511] = 0xAA;

    // A hard disk's partition table.
    if layout.start > 0 {
        let e = &mut disk[0x1BE..0x1CE];
        let last_cylinder = layout.cylinders - 1;
        e[0] = 0x80;
        e[1..4].copy_from_slice(&[1, 1, 0]); // head 1, sector 1, cylinder 0
        e[4] = 0x04; // FAT16 under 32 MB
        e[5] = (layout.heads - 1) as u8;
        e[6] = layout.sectors_per_track as u8 | ((last_cylinder >> 2) as u8 & 0xC0);
        e[7] = last_cylinder as u8;
        e[8..12].copy_from_slice(&layout.start.to_le_bytes());
        e[12..16].copy_from_slice(&layout.sectors.to_le_bytes());
        disk[510] = 0x55;
        disk[511] = 0xAA;
    }
    disk
}

/// Write an image to `dir/name` and return its path.
pub fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}
