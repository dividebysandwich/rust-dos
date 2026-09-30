//! A host folder as a hard disk of a system booted from a disk image,
//! which sees only disks: a FAT16 disk made in memory at the boot with
//! the folder's files on it, long names and all, and what the system
//! changed on it copied back into the folder when it shuts down, or when
//! asked (`sync`).
//!
//! A manifest remembers each file as it was on both sides at the last
//! copy, so a copy back only touches what the system changed. A file
//! changed on both sides keeps the host's version, and the system's goes
//! beside it as "name (from guest).ext". What the host changes while the
//! system runs reaches the disk at the next boot.

use crate::diskimage::{Chs, DiskImage};
use crate::fat::{Entry, FatVolume};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

/// The disk: 519 cylinders of 128 heads of 63 sectors, just under 2 GB,
/// the most FAT16 with 32 KB clusters holds, which DOS, Windows 3.1 and
/// every Windows 95 read. Memory is only taken for what is written.
pub const GEOMETRY: Chs = Chs { cylinders: 519, heads: 128, sectors: 63 };
/// The most the folder may hold: what the disk holds with room left.
pub const MAX_CONTENTS: u64 = 1536 << 20;

/// A file or directory as the host has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HostStamp {
    size: u64,
    /// Nanoseconds since 1970.
    modified: i128,
}

/// A file or directory as the disk has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FatStamp {
    size: u32,
    time: u16,
    date: u16,
    cluster: u32,
}

/// What the last copy left on both sides.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Known {
    dir: bool,
    host: Option<HostStamp>,
    fat: Option<FatStamp>,
    /// The DOS name on the disk.
    alias: String,
}

/// The files of the folder and the disk, by path from the folder with
/// "/" between the names: the host's names, which are the disk's long
/// names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    entries: BTreeMap<String, Known>,
}

/// A host folder made a disk.
pub struct SharedDisk {
    pub root: PathBuf,
    pub disk: Rc<DiskImage>,
    pub manifest: Manifest,
}

/// What a copy back did.
#[derive(Debug, Default)]
pub struct SyncReport {
    pub written: usize,
    pub deleted: usize,
    pub conflicts: Vec<String>,
    pub errors: Vec<String>,
}

impl SyncReport {
    /// A line for the log and the screen.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.written > 0 {
            parts.push(format!("{} written", self.written));
        }
        if self.deleted > 0 {
            parts.push(format!("{} deleted", self.deleted));
        }
        if !self.conflicts.is_empty() {
            parts.push(format!("{} changed on both sides, kept as \"(from guest)\"", self.conflicts.len()));
        }
        if !self.errors.is_empty() {
            parts.push(format!("{} failed", self.errors.len()));
        }
        if parts.is_empty() { "nothing changed".to_string() } else { parts.join(", ") }
    }
}

fn host_stamp(meta: &fs::Metadata) -> HostStamp {
    let modified = meta.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as i128);
    HostStamp { size: if meta.is_dir() { 0 } else { meta.len() }, modified }
}

fn fat_stamp(entry: &Entry) -> FatStamp {
    FatStamp { size: entry.size, time: entry.time, date: entry.date, cluster: entry.cluster }
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() { name.to_string() } else { format!("{}/{}", parent, name) }
}

fn parent_of(key: &str) -> &str {
    key.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn host_path(root: &Path, key: &str) -> PathBuf {
    key.split('/').fold(root.to_path_buf(), |path, name| path.join(name))
}

/// The folder's files and directories, by key.
fn host_tree(root: &Path) -> BTreeMap<String, (bool, HostStamp)> {
    fn walk(dir: &Path, key: &str, depth: usize, out: &mut BTreeMap<String, (bool, HostStamp)>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
            // Follows symbolic links.
            let Ok(meta) = fs::metadata(entry.path()) else { continue };
            let child = join(key, &name);
            if meta.is_dir() {
                out.insert(child.clone(), (true, host_stamp(&meta)));
                if depth < MAX_DEPTH {
                    walk(&entry.path(), &child, depth + 1, out);
                }
            } else if meta.is_file() {
                out.insert(child, (false, host_stamp(&meta)));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", 0, &mut out);
    out
}

/// How deep directories are followed.
const MAX_DEPTH: usize = 32;

/// A file or directory on the disk.
struct FatFile {
    dir: bool,
    stamp: FatStamp,
    entry: Entry,
}

/// The disk's files and directories, by key. An entry with no long name
/// that has the DOS name a file with one had in the same directory, which
/// is gone, is that file: a DOS program that saves a file by making it
/// anew loses its long name.
fn fat_tree(volume: &FatVolume, manifest: &Manifest) -> BTreeMap<String, FatFile> {
    fn walk(volume: &FatVolume, manifest: &Manifest, path: &mut Vec<String>, key: &str, out: &mut BTreeMap<String, FatFile>) {
        let parts: Vec<&str> = path.iter().map(String::as_str).collect();
        let Ok(entries) = volume.list(&parts) else { return };
        // Windows' Recycle Bin stays on the disk.
        let recycled = |e: &Entry| {
            path.is_empty() && e.name == "RECYCLED" && e.attr & (crate::fat::ATTR_HIDDEN | crate::fat::ATTR_SYSTEM) != 0
        };
        let entries: Vec<Entry> =
            entries.into_iter().filter(|e| e.name != "." && e.name != ".." && !recycled(e)).collect();
        let present: BTreeSet<String> = entries.iter().filter_map(|e| e.long_name.clone()).collect();
        for entry in entries {
            let name = match &entry.long_name {
                Some(long) => long.clone(),
                None => manifest
                    .entries
                    .iter()
                    .find(|(k, known)| {
                        parent_of(k) == key && known.alias == entry.name && !present.contains(k.rsplit('/').next().unwrap_or(k))
                    })
                    .map_or_else(|| entry.name.clone(), |(k, _)| k.rsplit('/').next().unwrap_or(k).to_string()),
            };
            let child = join(key, &name);
            path.push(entry.name.clone());
            if entry.is_dir() && path.len() <= MAX_DEPTH {
                walk(volume, manifest, path, &child, out);
            }
            out.insert(child, FatFile { dir: entry.is_dir(), stamp: fat_stamp(&entry), entry });
            path.pop();
        }
    }
    let mut out = BTreeMap::new();
    walk(volume, manifest, &mut Vec::new(), "", &mut out);
    out
}

/// The volume on the disk.
fn open_volume(disk: &Rc<DiskImage>) -> Result<FatVolume, String> {
    let (start, sectors) = disk.fat_volume()?;
    FatVolume::open(disk.clone(), start, sectors)
}

/// The volume label for a folder: its name in DOS's characters.
fn label_for(root: &Path) -> String {
    let name = root.file_name().map(|n| n.to_string_lossy().to_ascii_uppercase()).unwrap_or_default();
    let label: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || "_-".contains(*c)).take(11).collect();
    if label.is_empty() { "SHARED".to_string() } else { label }
}

impl SharedDisk {
    /// Make the disk of the folder `root`. Returns it with what the log
    /// should say about files left off it.
    pub fn build(root: &Path) -> Result<(SharedDisk, Vec<String>), String> {
        let tree = host_tree(root);
        let contents: u64 = tree.values().map(|(_, stamp)| stamp.size).sum();
        if contents > MAX_CONTENTS {
            return Err(format!(
                "{} holds {} MB, more than the {} MB a shared disk takes",
                root.display(),
                contents >> 20,
                MAX_CONTENTS >> 20
            ));
        }
        let name = root.display().to_string();
        let disk = Rc::new(DiskImage::blank_hard_disk_chs(&name, GEOMETRY, Some(&label_for(root)))?);
        let volume = open_volume(&disk)?;
        let mut manifest = Manifest::default();
        let mut skipped = Vec::new();
        // The DOS path of each directory put on the disk.
        let mut dirs: BTreeMap<String, Vec<String>> = BTreeMap::from([(String::new(), Vec::new())]);
        for (key, (dir, stamp)) in &tree {
            let Some(parent) = dirs.get(parent_of(key)).cloned() else { continue };
            let leaf = key.rsplit('/').next().unwrap_or(key);
            let parts: Vec<&str> = parent.iter().map(String::as_str).collect();
            let made = if *dir {
                volume.mkdir_long(&parts, leaf)
            } else {
                volume.create_long(&parts, leaf, 0)
            };
            let entry = match made {
                Ok(entry) => entry,
                Err(0x03) => {
                    skipped.push(format!("{}: the name can't be on a FAT disk", key));
                    continue;
                }
                Err(0x05) => {
                    skipped.push(format!("{}: another file has the name, or the directory is full", key));
                    continue;
                }
                Err(e) => return Err(format!("{}: can't be put on the disk (error {:02X}h)", key, e)),
            };
            let at = entry.at.ok_or("no directory entry")?;
            if *dir {
                let mut path = parent.clone();
                path.push(entry.name.clone());
                dirs.insert(key.clone(), path);
            } else {
                copy_in(&volume, at, &host_path(root, key)).map_err(|e| format!("{}: {}", key, e))?;
            }
            let modified = SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(stamp.modified.max(0) as u64);
            let (time, date) = crate::disk::system_time_to_dos(modified);
            volume.set_time(at, time, date).map_err(|e| format!("{}: error {:02X}h", key, e))?;
            let entry = volume.reload(at).map_err(|e| format!("{}: error {:02X}h", key, e))?;
            manifest.entries.insert(
                key.clone(),
                Known { dir: *dir, host: Some(*stamp), fat: Some(fat_stamp(&entry)), alias: entry.name.clone() },
            );
        }
        Ok((SharedDisk { root: root.to_path_buf(), disk, manifest }, skipped))
    }

    /// Copy what the system changed on the disk into the folder. `last`
    /// is the copy at its shutdown; others (while it runs) delete nothing,
    /// as its caches may not have written everything yet.
    pub fn sync(&mut self, last: bool) -> SyncReport {
        let mut report = SyncReport::default();
        let volume = match open_volume(&self.disk) {
            Ok(volume) => volume,
            Err(e) => {
                report.errors.push(e);
                return report;
            }
        };
        let fat = fat_tree(&volume, &self.manifest);
        let host = host_tree(&self.root);
        let keys: BTreeSet<&String> = fat.keys().chain(host.keys()).chain(self.manifest.entries.keys()).collect();
        // Deletions wait, deepest first, so directories are empty.
        let mut deletions = Vec::new();
        // What a copy that deleted nothing still has to delete.
        let mut kept = Vec::new();
        for key in keys {
            let known = self.manifest.entries.get(key);
            let f = fat.get(key);
            let h = host.get(key);
            let guest_changed = match (known.and_then(|k| k.fat.map(|s| (k.dir, s))), f) {
                (Some((dir, _)), Some(f)) if dir && f.dir => false,
                (Some((_, stamp)), Some(f)) => stamp != f.stamp,
                (None, None) => false,
                _ => true,
            };
            if !guest_changed {
                continue;
            }
            let host_changed = match (known.and_then(|k| k.host.map(|s| (k.dir, s))), h) {
                (Some((true, _)), Some((true, _))) => false,
                (Some((_, stamp)), Some((_, now))) => stamp != *now,
                (None, None) => false,
                _ => true,
            };
            let path = host_path(&self.root, key);
            match f {
                Some(f) if f.dir => {
                    if h.is_some_and(|(dir, _)| !dir) {
                        report.errors.push(format!("{}: a file on the host, a directory on the disk", key));
                    } else if let Err(e) = fs::create_dir_all(&path) {
                        report.errors.push(format!("{}: {}", key, e));
                    }
                }
                Some(f) => {
                    let target = if host_changed && h.is_some() {
                        report.conflicts.push(key.clone());
                        conflict_path(&path)
                    } else {
                        path
                    };
                    match copy_out(&volume, &f.entry, &target) {
                        Ok(()) => report.written += 1,
                        Err(e) => report.errors.push(format!("{}: {}", key, e)),
                    }
                }
                None if h.is_none() => {}
                None if !last => kept.push(key.clone()),
                // Gone from the disk: gone from the folder, unless the host
                // changed it since.
                None if host_changed => {}
                None => deletions.push((key.clone(), h.is_some_and(|(dir, _)| *dir))),
            }
        }
        deletions.sort_by_key(|(key, _)| std::cmp::Reverse(key.matches('/').count()));
        for (key, dir) in deletions {
            let path = host_path(&self.root, &key);
            let done = if dir { fs::remove_dir(&path) } else { fs::remove_file(&path) };
            match done {
                Ok(()) => report.deleted += 1,
                // A directory the host put files in stays.
                Err(_) if dir => {}
                Err(e) => report.errors.push(format!("{}: {}", key, e)),
            }
        }

        // The manifest of the disk and folder as they are now.
        let host = host_tree(&self.root);
        let mut manifest = Manifest::default();
        for (key, f) in &fat {
            manifest.entries.insert(
                key.clone(),
                Known { dir: f.dir, host: host.get(key).map(|(_, s)| *s), fat: Some(f.stamp), alias: f.entry.name.clone() },
            );
        }
        for key in kept {
            if let Some(known) = self.manifest.entries.get(&key) {
                manifest.entries.insert(key, known.clone());
            }
        }
        self.manifest = manifest;
        report
    }
}

/// Where the system's version of a file the host changed too goes:
/// "name (from guest).ext" beside it.
fn conflict_path(path: &Path) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name = match path.extension() {
        Some(ext) => format!("{} (from guest).{}", stem, ext.to_string_lossy()),
        None => format!("{} (from guest)", stem),
    };
    path.with_file_name(name)
}

/// Bytes copied at a time.
const COPY_CHUNK: usize = 1 << 20;

/// Put the host file `path` in the file whose entry is at `at`.
fn copy_in(volume: &FatVolume, at: crate::fat::EntryRef, path: &Path) -> Result<(), String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; COPY_CHUNK];
    let mut offset = 0u64;
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(());
        }
        let written = volume.write(at, offset, &buf[..n]).map_err(|e| format!("error {:02X}h", e))?;
        if written < n {
            return Err("the disk is full".to_string());
        }
        offset += n as u64;
    }
}

/// Write the disk's file `entry` to the host file `target`, through a
/// file beside it that takes its place when it's whole, dated as on the
/// disk.
fn copy_out(volume: &FatVolume, entry: &Entry, target: &Path) -> Result<(), String> {
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let partial = target.with_file_name(format!(
        ".{}.rust-dos-partial",
        target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    let result = (|| {
        let mut out = fs::File::create(&partial).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; COPY_CHUNK];
        let mut offset = 0u64;
        while offset < entry.size as u64 {
            let n = volume.read(entry, offset, &mut buf).map_err(|e| format!("error {:02X}h", e))?;
            if n == 0 {
                return Err("the file's clusters end early".to_string());
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            offset += n as u64;
        }
        if let Some(time) = crate::disk::dos_to_system_time(entry.time, entry.date) {
            let _ = out.set_modified(time);
        }
        drop(out);
        fs::rename(&partial, target).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

impl Manifest {
    /// The manifest as text, a line for each file, for the file beside a
    /// state file.
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        for (key, known) in &self.entries {
            let host = known.host.map_or("-".to_string(), |h| format!("{} {}", h.size, h.modified));
            let fat = known.fat.map_or("-".to_string(), |f| format!("{} {} {} {}", f.size, f.time, f.date, f.cluster));
            text.push_str(&format!("{}\t{}\t{}\t{}\t{}\n", if known.dir { "d" } else { "f" }, known.alias, host, fat, key));
        }
        text
    }

    /// The manifest `to_text` wrote.
    pub fn from_text(text: &str) -> Option<Manifest> {
        let mut entries = BTreeMap::new();
        for line in text.lines().filter(|l| !l.is_empty()) {
            let fields: Vec<&str> = line.splitn(5, '\t').collect();
            let [kind, alias, host, fat, key] = fields[..] else { return None };
            let numbers = |s: &str| -> Option<Vec<i128>> { s.split(' ').map(|n| n.parse().ok()).collect() };
            let host = match host {
                "-" => None,
                s => match numbers(s)?[..] {
                    [size, modified] => Some(HostStamp { size: size as u64, modified }),
                    _ => return None,
                },
            };
            let fat = match fat {
                "-" => None,
                s => match numbers(s)?[..] {
                    [size, time, date, cluster] => {
                        Some(FatStamp { size: size as u32, time: time as u16, date: date as u16, cluster: cluster as u32 })
                    }
                    _ => return None,
                },
            };
            entries.insert(key.to_string(), Known { dir: kind == "d", host, fat, alias: alias.to_string() });
        }
        Some(Manifest { entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::current_dir().unwrap().join("target/test_shared_disk").join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn contents(volume: &FatVolume, path: &[&str]) -> Vec<u8> {
        let entry = volume.find(path).unwrap();
        let mut buf = vec![0u8; entry.size as usize];
        volume.read(&entry, 0, &mut buf).unwrap();
        buf
    }

    /// A folder with long names, a subdirectory and an empty one.
    fn folder(name: &str) -> PathBuf {
        let root = scratch(name);
        fs::write(root.join("Read Me First.txt"), b"hello").unwrap();
        fs::write(root.join("NOTES.TXT"), b"notes").unwrap();
        fs::create_dir_all(root.join("Saved Games/Slot One")).unwrap();
        fs::write(root.join("Saved Games/Slot One/game.sav"), vec![7u8; 100_000]).unwrap();
        fs::create_dir(root.join("Empty")).unwrap();
        root
    }

    #[test]
    fn the_disk_has_the_folder() {
        let root = folder("build");
        let (shared, skipped) = SharedDisk::build(&root).unwrap();
        assert!(skipped.is_empty(), "{:?}", skipped);
        let volume = open_volume(&shared.disk).unwrap();
        assert_eq!(volume.fat_type(), crate::fat::FatType::Fat16);
        assert_eq!(volume.label().as_deref(), Some("BUILD"));
        let names: Vec<(String, Option<String>)> =
            volume.list(&[]).unwrap().into_iter().map(|e| (e.name, e.long_name)).collect();
        assert!(names.contains(&("README~1.TXT".to_string(), Some("Read Me First.txt".to_string()))), "{:?}", names);
        assert!(names.contains(&("NOTES.TXT".to_string(), None)));
        assert_eq!(contents(&volume, &["README~1.TXT"]), b"hello");
        assert_eq!(contents(&volume, &["SAVEDG~1", "SLOTON~1", "GAME.SAV"]).len(), 100_000);
        let modified = fs::metadata(root.join("NOTES.TXT")).unwrap().modified().unwrap();
        let entry = volume.find(&["NOTES.TXT"]).unwrap();
        assert_eq!((entry.time, entry.date), crate::disk::system_time_to_dos(modified));
        // Nothing changed: nothing to copy.
        let mut shared = shared;
        let report = shared.sync(true);
        assert_eq!((report.written, report.deleted, report.errors.len()), (0, 0, 0), "{:?}", report);
    }

    #[test]
    fn what_the_system_changes_reaches_the_folder() {
        let root = folder("changes");
        let (mut shared, _) = SharedDisk::build(&root).unwrap();
        let volume = open_volume(&shared.disk).unwrap();
        // A new file with a long name, one changed, one deleted, a new
        // directory, and the empty one gone.
        let new = volume.create_long(&["SAVEDG~1"], "Slot Two.sav", 0).unwrap();
        volume.write(new.at.unwrap(), 0, b"two").unwrap();
        let notes = volume.find(&["NOTES.TXT"]).unwrap();
        volume.write(notes.at.unwrap(), 0, b"NOTES, longer now").unwrap();
        volume.set_time(notes.at.unwrap(), 0x6000, 0x5A21).unwrap();
        volume.remove(&["SAVEDG~1", "SLOTON~1", "GAME.SAV"]).unwrap();
        volume.rmdir(&["SAVEDG~1", "SLOTON~1"]).unwrap();
        volume.mkdir_long(&[], "New Folder").unwrap();
        volume.rmdir(&["EMPTY"]).unwrap();
        // Windows' Recycle Bin stays on the disk.
        volume.mkdir(&["RECYCLED"]).unwrap();
        volume.set_attr(&["RECYCLED"], crate::fat::ATTR_HIDDEN | crate::fat::ATTR_SYSTEM).unwrap();
        volume.create(&["RECYCLED", "DC0.TXT"], 0).unwrap();

        // While it runs nothing goes.
        let report = shared.sync(false);
        assert_eq!((report.written, report.deleted), (2, 0), "{:?}", report);
        assert_eq!(fs::read(root.join("Saved Games/Slot Two.sav")).unwrap(), b"two");
        assert_eq!(fs::read(root.join("NOTES.TXT")).unwrap(), b"NOTES, longer now");
        assert!(root.join("New Folder").is_dir());
        assert!(root.join("Saved Games/Slot One/game.sav").is_file());
        assert!(!root.join("RECYCLED").exists());

        // At the shutdown the deletions go too, once.
        let report = shared.sync(true);
        assert_eq!((report.written, report.deleted), (0, 3), "{:?}", report);
        assert!(!root.join("Saved Games/Slot One").exists());
        assert!(!root.join("Empty").exists());
        let report = shared.sync(true);
        assert_eq!((report.written, report.deleted), (0, 0), "{:?}", report);
        let modified = fs::metadata(root.join("NOTES.TXT")).unwrap().modified().unwrap();
        assert_eq!(crate::disk::system_time_to_dos(modified), (0x6000, 0x5A21));
    }

    #[test]
    fn the_host_s_changes_win() {
        let root = folder("conflicts");
        let (mut shared, _) = SharedDisk::build(&root).unwrap();
        let volume = open_volume(&shared.disk).unwrap();
        let notes = volume.find(&["NOTES.TXT"]).unwrap();
        volume.write(notes.at.unwrap(), 0, b"guest").unwrap();
        volume.set_time(notes.at.unwrap(), 0x6000, 0x5A21).unwrap();
        volume.remove(&["README~1.TXT"]).unwrap();
        // The host changes both meanwhile.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(root.join("NOTES.TXT"), b"host notes").unwrap();
        fs::write(root.join("Read Me First.txt"), b"host readme").unwrap();

        let report = shared.sync(true);
        assert_eq!(report.conflicts, ["NOTES.TXT"]);
        assert_eq!(fs::read(root.join("NOTES.TXT")).unwrap(), b"host notes");
        assert_eq!(fs::read(root.join("NOTES (from guest).TXT")).unwrap(), b"guest");
        assert_eq!(fs::read(root.join("Read Me First.txt")).unwrap(), b"host readme");
        assert_eq!(report.deleted, 0);
    }

    #[test]
    fn a_dos_program_s_save_keeps_the_long_name() {
        let root = folder("dos_save");
        let (mut shared, _) = SharedDisk::build(&root).unwrap();
        let volume = open_volume(&shared.disk).unwrap();
        // EDIT deletes the file and makes it anew by its DOS name.
        volume.remove(&["README~1.TXT"]).unwrap();
        let again = volume.create(&["README~1.TXT"], 0).unwrap();
        volume.write(again.at.unwrap(), 0, b"edited in DOS").unwrap();
        let report = shared.sync(true);
        assert_eq!((report.written, report.deleted), (1, 0), "{:?}", report);
        assert_eq!(fs::read(root.join("Read Me First.txt")).unwrap(), b"edited in DOS");
        assert!(!root.join("README~1.TXT").exists());
    }

    #[test]
    fn manifests_as_text() {
        let root = folder("manifest");
        let (shared, _) = SharedDisk::build(&root).unwrap();
        let text = shared.manifest.to_text();
        assert_eq!(Manifest::from_text(&text), Some(shared.manifest.clone()));
        assert_eq!(Manifest::from_text("x"), None);
    }
}
