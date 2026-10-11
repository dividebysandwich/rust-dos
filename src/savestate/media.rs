//! The media a state was saved with: each mounted drive's host folder or
//! disk and CD images, as the slot file's header records them. A state
//! keeps the drives by their paths, not what is in them (disk/state.rs),
//! so a load through the debug server checks that the same media are
//! there before it puts the state in: the same drives, mounted from the
//! same paths, and images that are there. A read-only image (a CD's, a
//! `-ro` mount's) is also told apart by its size and a hash of its start;
//! a writable one changes as the machine writes it (a dynamic VHD grows),
//! so its size is recorded but not compared.
//!
//! A strict-match save also records what is on the writable drives
//! (`add_contents`): each file of a folder drive with its size and
//! SHA-256, and the SHA-256 of each writable image and delta file. A load
//! of such a state compares them with the files as they are, and names
//! the files a program (or anything else) changed since the save. The
//! state doesn't keep the files themselves, so a load can't put them back.

use crate::cpu::Cpu;
use crate::disk::{DriveInfo, DriveKind, drive_key};
use crate::hostfs;
use crate::mount::display_host_path;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// How much of the start of a read-only image is hashed.
pub const HEAD: u64 = 64 << 10;

/// A mounted drive's media.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Medium {
    /// `C`, or `2` for a disk mounted by number.
    pub drive: String,
    /// floppy, hdd, cdrom or virtual.
    pub kind: String,
    pub read_only: bool,
    /// The host folder the drive shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Where the drive's changes go: an overlay folder or a disk image's
    /// delta file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay: Option<String>,
    /// The disk or CD images, all of a drive's list, each CUE sheet
    /// followed by the files it keeps its tracks in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,
    /// What is on a writable folder drive, as the drive shows it (with
    /// its overlay's changes), sorted by path: recorded by a strict-match
    /// save (`add_contents`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<FileEntry>>,
    /// SHA-256 of a writable image's delta file (`overlay`), recorded by a
    /// strict-match save; `none` while there is no such file (it is made at
    /// the first write).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_sha256: Option<String>,
}

/// A disk or CD image file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Image {
    pub path: String,
    /// Its size in bytes; none if it can't be read.
    pub size: Option<u64>,
    /// SHA-256 of its first `HEAD` bytes, for a read-only image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha256: Option<String>,
    /// SHA-256 of the whole file, for a writable image, recorded by a
    /// strict-match save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// A file or folder on a writable folder drive.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Its path from the drive's root, with `/` between names; a folder's
    /// ends with `/`.
    pub path: String,
    /// A file's size in bytes; none for a folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// SHA-256 of a file's contents; none for a folder, or a file that
    /// couldn't be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// The media of the machine's mounted drives now. The drives the machine
/// has of itself (Z:, held in memory) have none and aren't listed.
///
/// The record is made when the mounts change and kept with them, as the
/// libretro core takes a state every frame: while every drive has the
/// same folder, overlay and images as when it was made, no CUE sheet is
/// read again and no file looked at. A strict-match save or load `forget`s
/// it first, so it is checked against the files as they are.
pub fn of(cpu: &Cpu) -> Vec<Medium> {
    let drives = cpu.bus.disk.all_drives();
    let mounts: Vec<Mount> = drives.iter().map(Mount::of).collect();
    let kept = &cpu.bus.disk.media;
    let media = match kept.take() {
        Some(record) if record.mounts == mounts => record.media,
        _ => drives.iter().filter_map(medium).collect(),
    };
    kept.set(Some(Kept { mounts, media: media.clone() }));
    media
}

/// Forget the record `of` keeps for `cpu` and the head hashes, so the
/// next record reads every CUE sheet and read-only image again. A
/// strict-match save or load does this: a track file can appear after
/// the sheet was mounted, and a file replaced by another of the same size with its
/// time kept (`cp -p`, `rsync -t`, an unpacked archive) has the cached
/// hash of the old one.
pub fn forget(cpu: &Cpu) {
    cpu.bus.disk.media.set(None);
    forget_hashes();
}

/// A machine's record of its media, with the mounts it was made of.
pub struct Kept {
    mounts: Vec<Mount>,
    media: Vec<Medium>,
}

/// What of a mounted drive `medium` makes its record of.
#[derive(PartialEq)]
struct Mount {
    drive: u8,
    kind: DriveKind,
    read_only: bool,
    root: Option<PathBuf>,
    overlay: Option<PathBuf>,
    image: Option<PathBuf>,
    images: Vec<PathBuf>,
}

impl Mount {
    fn of(info: &DriveInfo) -> Self {
        Mount {
            drive: info.drive,
            kind: info.kind,
            read_only: info.read_only,
            root: info.root.clone(),
            overlay: info.overlay.clone(),
            image: info.image.clone(),
            images: info.images.clone(),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// How many images `medium` has looked for a CUE sheet's files in.
    static SHEET_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn medium(info: &DriveInfo) -> Option<Medium> {
    let mut listed: Vec<PathBuf> = info.images.clone();
    if listed.is_empty() {
        listed.extend(info.image.clone());
    }
    // A CUE sheet's tracks are in the files it names.
    let paths: Vec<PathBuf> = listed
        .into_iter()
        .flat_map(|p| {
            #[cfg(test)]
            SHEET_READS.with(|n| n.set(n.get() + 1));
            std::iter::once(p.clone()).chain(crate::cdrom::image::cue_files(&p))
        })
        .collect();
    if info.root.is_none() && paths.is_empty() {
        return None;
    }
    // A frontend's path (`saf://...`, see hostfs.rs) is kept as it is.
    let shown = |p: &Path| {
        if hostfs::has_scheme(p) {
            p.display().to_string()
        } else {
            display_host_path(&std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()))
        }
    };
    Some(Medium {
        drive: drive_key(info.drive),
        kind: info.kind.name().to_string(),
        read_only: info.read_only,
        folder: info.root.as_deref().map(shown),
        overlay: info.overlay.as_deref().map(shown),
        images: paths.iter().map(|p| image(p, info.read_only, shown(p))).collect(),
        ..Default::default()
    })
}

fn image(path: &Path, read_only: bool, shown: String) -> Image {
    let meta = hostfs::metadata(path).ok();
    let size = meta.as_ref().map(|m| m.len);
    let head_sha256 = match (read_only, &meta) {
        (true, Some(meta)) => head_hash(path, meta.len, meta.modified),
        _ => None,
    };
    Image { path: shown, size, head_sha256, sha256: None }
}

type Known = HashMap<PathBuf, (u64, Option<SystemTime>, String)>;

/// The head hashes `head_hash` keeps, by path.
static KNOWN: Mutex<Option<Known>> = Mutex::new(None);

/// Forget the head hashes kept, so the next record reads every read-only
/// image again (`forget`).
fn forget_hashes() {
    *KNOWN.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The hash of the start of the file at `path`, kept while its size and
/// time stay the same: a drive mounted again needn't read it again.
fn head_hash(path: &Path, size: u64, modified: Option<SystemTime>) -> Option<String> {
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    let known = known.get_or_insert_with(HashMap::new);
    if let Some((s, m, hash)) = known.get(path)
        && (*s, *m) == (size, modified)
    {
        return Some(hash.clone());
    }
    let mut head = Vec::new();
    hostfs::File::open(path).ok()?.take(HEAD).read_to_end(&mut head).ok()?;
    let hash = hex(&Sha256::digest(&head));
    known.insert(path.to_path_buf(), (size, modified, hash.clone()));
    Some(hash)
}

/// Record what is on the writable drives of `media` (the machine's now,
/// from `of`) that `wanted` picks: each file and folder of a host folder
/// drive with its size and SHA-256 (`files`), and the SHA-256 of each
/// writable image and delta file. Every file is read through, so a
/// strict-match save does it and a plain one doesn't. A booted system's
/// disks are left out: its state keeps a copy of them (disks.rs). So are
/// the files of the state at `state` itself, if it is on a drive: the
/// state file and the copies of disks beside it.
pub fn add_contents(cpu: &Cpu, media: &mut [Medium], state: &Path, wanted: impl Fn(&Medium) -> bool) {
    let own = StateFiles::of(state);
    for info in cpu.bus.disk.all_drives() {
        if info.read_only || cpu.bus.disk.bios_image(info.drive).is_some_and(|disk| disk.journaling()) {
            continue;
        }
        let key = drive_key(info.drive);
        let Some(medium) = media.iter_mut().find(|m| m.drive == key) else { continue };
        if !wanted(medium) {
            continue;
        }
        if let Some(root) = cpu.bus.disk.host_root(info.drive) {
            let mut files = Vec::new();
            walk(&root, "", &own, &mut files);
            files.sort_by(|a, b| a.path.cmp(&b.path));
            medium.files = Some(files);
        }
        let mut listed = info.images.clone();
        if listed.is_empty() {
            listed.extend(info.image.clone());
        }
        if listed.len() == medium.images.len() {
            for (path, image) in listed.iter().zip(&mut medium.images) {
                image.sha256 = file_hash(path);
            }
        }
        if medium.folder.is_none()
            && let Some(delta) = &info.overlay
        {
            // The file is made at the first write.
            medium.overlay_sha256 = Some(file_hash(delta).unwrap_or_else(|| NO_FILE.to_string()));
        }
    }
}

/// Whether a strict-match save recorded what is on the drive of `medium`.
pub fn has_contents(medium: &Medium) -> bool {
    medium.files.is_some() || medium.overlay_sha256.is_some() || medium.images.iter().any(|i| i.sha256.is_some())
}

/// The writable drives of the media a state was saved with whose contents
/// it has no record of (`add_contents`), each as a sentence: a state saved
/// without a strict match, or by an older rust-dos. `copied` says whether
/// a copy of a drive's disk is kept beside the state (a booted system's,
/// disks.rs), which the load puts back.
pub fn unrecorded(saved: &[Medium], copied: impl Fn(&str) -> bool) -> Vec<String> {
    saved
        .iter()
        .filter(|m| !m.read_only && (m.folder.is_some() || !m.images.is_empty()))
        .filter(|m| !has_contents(m) && !copied(&m.drive))
        .map(|m| {
            format!(
                "{} wasn't checked: the state doesn't record what is on it (saved without strict_match, or by an older rust-dos)",
                drive_name(&m.drive)
            )
        })
        .collect()
}

/// The files and folders under `dir`, `prefix` their path from the root.
/// A link to a folder is listed as a folder without going into it.
fn walk(dir: &Path, prefix: &str, own: &StateFiles, out: &mut Vec<FileEntry>) {
    let Ok(entries) = hostfs::read_dir(dir) else { return };
    let beside_state = own.dir.is_some() && std::fs::canonicalize(dir).ok() == own.dir;
    for entry in entries {
        let name = entry.name.to_string_lossy();
        if beside_state && own.has(&name) {
            continue;
        }
        let path = format!("{}{}", prefix, name);
        let is_dir = entry.is_dir || hostfs::metadata(&entry.path).is_ok_and(|m| m.is_dir);
        if is_dir {
            out.push(FileEntry { path: format!("{}/", path), size: None, sha256: None });
            if entry.is_dir {
                walk(&entry.path, &format!("{}/", path), own, out);
            }
        } else {
            let size = hostfs::metadata(&entry.path).map(|m| m.len).ok();
            out.push(FileEntry { path, size, sha256: file_hash(&entry.path) });
        }
    }
}

/// A state file's own files, which a record of the drive it is on leaves
/// out: `a.state`, `a.partial` while it is written, and `a.C.img` and the
/// like (disks.rs).
struct StateFiles {
    /// The folder they are in, if it is there.
    dir: Option<PathBuf>,
    name: String,
    stem: String,
}

impl StateFiles {
    fn of(state: &Path) -> Self {
        let parent = state.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let text = |s: Option<&std::ffi::OsStr>| s.map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        StateFiles { dir: std::fs::canonicalize(parent).ok(), name: text(state.file_name()), stem: text(state.file_stem()) }
    }

    fn has(&self, name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        let copy = name.strip_prefix(&self.stem).and_then(|rest| rest.strip_prefix('.')).is_some_and(|rest| rest.ends_with(".img"));
        name == self.name || name == format!("{}.partial", self.stem) || copy
    }
}

/// SHA-256 of the whole file at `path`, if it can be read.
fn file_hash(path: &Path) -> Option<String> {
    let mut file = hostfs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// `overlay_sha256` of a delta file that isn't there.
const NO_FILE: &str = "none";

/// How many changed files of a drive `differences` names; the rest are
/// counted.
const NAMED_FILES: usize = 20;

/// The files and folders that differ between two records of a drive's
/// (`files`), as sentences starting with `name`.
fn file_differences(name: &str, was: &[FileEntry], is: &[FileEntry]) -> Vec<String> {
    let index = |list: &[FileEntry]| list.iter().map(|f| (f.path.clone(), f.clone())).collect::<std::collections::BTreeMap<_, _>>();
    let (was, is) = (index(was), index(is));
    let mut paths: Vec<&String> = was.keys().chain(is.keys()).collect();
    paths.sort_unstable();
    paths.dedup();
    let size = |f: &FileEntry| f.size.map_or("unreadable".to_string(), |n| format!("{} bytes", n));
    let mut out: Vec<String> = paths
        .into_iter()
        .filter_map(|path| {
            let what = if path.ends_with('/') { "folder" } else { "file" };
            match (was.get(path), is.get(path)) {
                (Some(_), None) => Some(format!("{} {} {}: there when saved, gone now", name, what, path)),
                (None, Some(_)) => Some(format!("{} {} {}: new since the save", name, what, path)),
                (Some(a), Some(b)) if a.size != b.size => {
                    Some(format!("{} file {}: {} when saved, {} now", name, path, size(a), size(b)))
                }
                (Some(a), Some(b)) if a.sha256 != b.sha256 => {
                    Some(format!("{} file {}: its contents differ from when it was saved", name, path))
                }
                _ => None,
            }
        })
        .collect();
    if out.len() > NAMED_FILES {
        let more = out.len() - NAMED_FILES;
        out.truncate(NAMED_FILES);
        out.push(format!("{} and {} more files or folders differ from when it was saved", name, more));
    }
    out
}

/// "drive C:", or "disk 2" for a disk mounted by number, from a drive's
/// key (`Medium::drive`).
fn drive_name(drive: &str) -> String {
    if drive.chars().all(|c| c.is_ascii_digit()) { format!("disk {}", drive) } else { format!("drive {}:", drive) }
}

/// What differs between the media a state was saved with and the
/// machine's now, as sentences; empty if they are the same.
pub fn differences(saved: &[Medium], now: &[Medium]) -> Vec<String> {
    let mut out = Vec::new();
    let find = |list: &'_ [Medium], drive: &str| list.iter().find(|m| m.drive == drive).cloned();
    let mut drives: Vec<&str> = saved.iter().chain(now).map(|m| m.drive.as_str()).collect();
    drives.sort_unstable();
    drives.dedup();
    for drive in drives {
        let name = drive_name(drive);
        let (was, is) = match (find(saved, drive), find(now, drive)) {
            (Some(was), Some(is)) => (was, is),
            (Some(was), None) => {
                out.push(format!("{} is not mounted here; the state has it ({})", name, describe(&was)));
                continue;
            }
            (None, Some(is)) => {
                out.push(format!("{} is mounted here ({}); the state hasn't it", name, describe(&is)));
                continue;
            }
            (None, None) => unreachable!("a drive of one of the lists"),
        };
        let field = |out: &mut Vec<String>, what: &str, a: String, b: String| {
            if a != b {
                out.push(format!("{} {}: the state has {}, this machine has {}", name, what, a, b));
            }
        };
        let or_none = |v: &Option<String>| v.clone().unwrap_or_else(|| "none".to_string());
        field(&mut out, "type", was.kind.clone(), is.kind.clone());
        field(&mut out, "read-only", was.read_only.to_string(), is.read_only.to_string());
        field(&mut out, "folder", or_none(&was.folder), or_none(&is.folder));
        field(&mut out, "overlay", or_none(&was.overlay), or_none(&is.overlay));
        let paths = |m: &Medium| m.images.iter().map(|i| i.path.clone()).collect::<Vec<_>>().join(", ");
        if paths(&was) != paths(&is) {
            field(&mut out, "images", paths(&was), paths(&is));
            continue;
        }
        for (a, b) in was.images.iter().zip(&is.images) {
            let size = |s: Option<u64>| s.map_or("missing".to_string(), |n| format!("{} bytes", n));
            if (a.size.is_some() != b.size.is_some() || was.read_only) && a.size != b.size {
                out.push(format!("{} image {}: {} when saved, {} now", name, a.path, size(a.size), size(b.size)));
            } else if a.head_sha256.is_some() && b.head_sha256.is_some() && a.head_sha256 != b.head_sha256 {
                out.push(format!("{} image {}: its first {} KiB differ from when it was saved", name, a.path, HEAD >> 10));
            } else if a.sha256.is_some() && b.sha256.is_some() && a.sha256 != b.sha256 {
                out.push(format!("{} image {}: its contents differ from when it was saved", name, a.path));
            }
        }
        if let (Some(a), Some(b)) = (&was.overlay_sha256, &is.overlay_sha256)
            && a != b
        {
            out.push(format!("{} delta file {}: its contents differ from when it was saved", name, or_none(&was.overlay)));
        }
        if let (Some(a), Some(b)) = (&was.files, &is.files) {
            out.extend(file_differences(&name, a, b));
        }
    }
    out
}

fn describe(m: &Medium) -> String {
    match (&m.folder, m.images.first()) {
        (Some(folder), _) => format!("{} {}", m.kind, folder),
        (None, Some(image)) => format!("{} {}", m.kind, image.path),
        (None, None) => m.kind.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cd(path: &str, size: u64, hash: &str) -> Medium {
        Medium {
            drive: "D".into(),
            kind: "cdrom".into(),
            read_only: true,
            images: vec![Image { path: path.into(), size: Some(size), head_sha256: Some(hash.into()), sha256: None }],
            ..Default::default()
        }
    }

    fn folder(drive: &str, path: &str) -> Medium {
        Medium { drive: drive.into(), kind: "hdd".into(), folder: Some(path.into()), ..Default::default() }
    }

    #[test]
    fn the_same_media_have_no_differences() {
        let media = [folder("C", "/games/keen"), cd("/cds/game.cue", 1000, "ab")];
        assert!(differences(&media, &media).is_empty());
    }

    #[test]
    fn another_image_or_folder_or_a_missing_drive_differs() {
        let saved = [folder("C", "/games/keen"), cd("/cds/game.cue", 1000, "ab")];
        let other_cd = [folder("C", "/games/keen"), cd("/cds/game.cue", 1000, "cd")];
        let d = differences(&saved, &other_cd);
        assert_eq!(d.len(), 1, "{:?}", d);
        assert!(d[0].contains("drive D:") && d[0].contains("first 64 KiB"), "{}", d[0]);

        let resized = [folder("C", "/games/keen"), cd("/cds/game.cue", 2000, "ab")];
        assert!(differences(&saved, &resized)[0].contains("1000 bytes when saved, 2000 bytes now"));
        // A writable image grows as the machine writes it.
        let writable = |size| Medium { read_only: false, ..cd("/disks/c.vhd", size, "ab") };
        assert!(differences(&[writable(1000)], &[writable(2000)]).is_empty());
        let gone = Medium { images: vec![Image { path: "/disks/c.vhd".into(), size: None, head_sha256: None, sha256: None }], ..writable(0) };
        assert!(differences(&[writable(1000)], &[gone])[0].contains("missing now"));

        let moved = [folder("C", "/games/other"), cd("/cds/game.cue", 1000, "ab")];
        let d = differences(&saved, &moved);
        assert_eq!(d, ["drive C: folder: the state has /games/keen, this machine has /games/other"]);

        let no_cd = [folder("C", "/games/keen")];
        assert!(differences(&saved, &no_cd)[0].starts_with("drive D: is not mounted here"));
        assert!(differences(&no_cd, &saved)[0].starts_with("drive D: is mounted here"));
    }

    /// Held by the tests that read or forget the kept head hashes, which
    /// all tests share.
    static HASHES: Mutex<()> = Mutex::new(());

    #[test]
    fn a_folder_and_a_read_only_image_are_recorded() {
        let _hashes = HASHES.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("rust-dos-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("disk.img");
        std::fs::write(&path, vec![0xF6; 4096]).unwrap();
        let cpu = Cpu::new(dir.clone());
        let media = of(&cpu);
        let c = media.iter().find(|m| m.drive == "C").expect("C: is listed");
        assert!(c.folder.is_some() && c.images.is_empty());
        assert!(media.iter().all(|m| m.drive != "Z"), "Z: is the machine's own");

        let info = |read_only| DriveInfo {
            drive: 3,
            kind: crate::disk::DriveKind::CdRom,
            root: None,
            overlay: None,
            image: Some(path.clone()),
            images: Vec::new(),
            image_index: 0,
            label: String::new(),
            read_only,
            current_dir: String::new(),
            mount: None,
        };
        let d = medium(&info(true)).unwrap();
        assert_eq!((d.drive.as_str(), d.images.len()), ("D", 1));
        assert_eq!(d.images[0].size, Some(4096));
        assert_eq!(d.images[0].head_sha256.as_ref().map(String::len), Some(64));
        assert_eq!(medium(&info(false)).unwrap().images[0].head_sha256, None, "a writable image has its size only");

        // Another image of the same size at the same path.
        std::fs::write(&path, vec![0xE5; 4096]).unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(later).unwrap();
        let now = medium(&info(true)).unwrap();
        let d = differences(std::slice::from_ref(&d), std::slice::from_ref(&now));
        assert_eq!(d.len(), 1, "{:?}", d);

        // Replaced again with its time kept: the cached hash stays until
        // a strict-match save or load forgets it.
        std::fs::write(&path, vec![0x4D; 4096]).unwrap();
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(later).unwrap();
        assert_eq!(medium(&info(true)).unwrap(), now, "the hash kept for the same size and time");
        forget_hashes();
        assert_ne!(medium(&info(true)).unwrap(), now, "read again");

        // A CUE sheet, with the file its track is in.
        let cue = dir.join("game.ins");
        std::fs::write(&cue, "FILE \"disk.img\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n").unwrap();
        let d = medium(&DriveInfo { image: Some(cue), ..info(true) }).unwrap();
        let names: Vec<&str> = d.images.iter().map(|i| i.path.rsplit(['/', '\\']).next().unwrap()).collect();
        assert_eq!(names, ["game.ins", "disk.img"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn sheet_reads() -> usize {
        SHEET_READS.with(|n| n.get())
    }

    #[test]
    fn the_record_is_kept_until_the_mounts_change_or_a_strict_match_save() {
        let _hashes = HASHES.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("rust-dos-media-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("c")).unwrap();
        std::fs::create_dir_all(dir.join("disc")).unwrap();
        std::fs::write(dir.join("disc/DATA.DAT"), b"on the disc").unwrap();
        let folder = crate::cdrom::folder::build(&dir.join("disc"), "GAMECD").unwrap();
        let folder = crate::cdrom::image::CdImage::from_folder(folder, &dir.join("disc")).unwrap();
        let mut iso = Vec::new();
        for lba in 0..folder.leadout() {
            let mut sector = [0u8; crate::cdrom::DATA_SECTOR];
            folder.read_data(lba, &mut sector).unwrap();
            iso.extend_from_slice(&sector);
        }
        let track = dir.join("GAME.ISO");
        std::fs::write(&track, &iso).unwrap();
        let cue = dir.join("GAME.CUE");
        std::fs::write(&cue, "FILE \"GAME.ISO\" BINARY\n  TRACK 01 MODE1/2048\n    INDEX 01 00:00:00\n").unwrap();
        let cdrom = || crate::disk::MountOptions { kind: DriveKind::CdRom, ..Default::default() };

        let mut cpu = Cpu::new(dir.join("c"));
        cpu.bus.disk.mount(3, &cue, cdrom(), false).unwrap();
        let reads = sheet_reads();
        let first = of(&cpu);
        assert_eq!(sheet_reads(), reads + 1, "the sheet is read once");
        let d = first.iter().find(|m| m.drive == "D").expect("D: is listed");
        assert_eq!(d.images.len(), 2, "the sheet and its track: {:?}", d.images);
        for _ in 0..10 {
            assert_eq!(of(&cpu), first);
        }
        assert_eq!(sheet_reads(), reads + 1, "the record is kept while the mounts stay");

        // The track replaced, with the same size and time: the kept record
        // stays until a strict-match save or load forgets it.
        let modified = std::fs::metadata(&track).unwrap().modified().unwrap();
        let mut other = iso.clone();
        other[0] ^= 0xFF;
        std::fs::write(&track, &other).unwrap();
        std::fs::File::options().write(true).open(&track).unwrap().set_modified(modified).unwrap();
        assert_eq!(of(&cpu), first);
        forget(&cpu);
        let fresh = of(&cpu);
        assert_eq!(sheet_reads(), reads + 2, "a strict-match save reads the sheet again");
        assert_eq!(differences(&first, &fresh).len(), 1, "the track's start differs");
        std::fs::write(&track, &iso).unwrap();

        // Unmounted, then mounted again.
        cpu.bus.disk.unmount(3).unwrap();
        let unmounted = of(&cpu);
        assert!(unmounted.iter().all(|m| m.drive != "D"), "D: is gone");
        assert_eq!(of(&cpu), unmounted);
        cpu.bus.disk.mount(3, &cue, cdrom(), false).unwrap();
        let reads = sheet_reads();
        let again = of(&cpu);
        assert_eq!(sheet_reads(), reads + 1, "a mount reads the sheet");
        assert_eq!(again.iter().find(|m| m.drive == "D").map(|m| m.images.len()), Some(2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn file(path: &str, size: u64, hash: &str) -> FileEntry {
        FileEntry { path: path.into(), size: Some(size), sha256: Some(hash.into()) }
    }

    fn with_files(files: Vec<FileEntry>) -> Medium {
        Medium { files: Some(files), ..folder("C", "/games/keen") }
    }

    #[test]
    fn files_new_gone_resized_or_rewritten_differ() {
        let saved = with_files(vec![
            file("CONFIG.DAT", 7, "aa"),
            file("GAME.EXE", 100, "bb"),
            file("OLD.SAV", 10, "cc"),
            FileEntry { path: "SAVES/".into(), size: None, sha256: None },
        ]);
        assert!(differences(std::slice::from_ref(&saved), std::slice::from_ref(&saved)).is_empty());
        let now = with_files(vec![
            file("CONFIG.DAT", 7, "ab"),
            file("GAME.EXE", 100, "bb"),
            file("NEW.SAV", 12, "dd"),
            file("SAVES/", 3, "ee"),
        ]);
        assert_eq!(
            differences(&[saved.clone()], &[now]),
            [
                "drive C: file CONFIG.DAT: its contents differ from when it was saved",
                "drive C: file NEW.SAV: new since the save",
                "drive C: file OLD.SAV: there when saved, gone now",
                "drive C: file SAVES/: unreadable when saved, 3 bytes now",
            ]
        );
        // A state without the record, or a machine whose drive wasn't
        // read, has nothing to compare.
        let plain = folder("C", "/games/keen");
        assert!(differences(&[plain.clone()], &[with_files(Vec::new())]).is_empty());
        assert!(differences(&[saved], &[plain]).is_empty());
    }

    #[test]
    fn many_changed_files_are_named_up_to_a_limit() {
        let now = with_files((0..30).map(|i| file(&format!("F{:02}.DAT", i), 1, "aa")).collect());
        let d = differences(&[with_files(Vec::new())], &[now]);
        assert_eq!(d.len(), NAMED_FILES + 1);
        assert_eq!(d[0], "drive C: file F00.DAT: new since the save");
        assert_eq!(d[NAMED_FILES], "drive C: and 10 more files or folders differ from when it was saved");
    }

    #[test]
    fn a_writable_image_and_its_delta_file_are_hashed_whole() {
        let dir = std::env::temp_dir().join(format!("rust-dos-media-whole-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("c")).unwrap();
        let image = dir.join("floppy.img");
        let spec = crate::makeimg::ImageSpec { preset: crate::makeimg::preset("fd_360kb"), ..Default::default() };
        crate::makeimg::write(&image, &crate::makeimg::plan(&spec).unwrap(), false).unwrap();
        let mut cpu = Cpu::new(dir.join("c"));
        let floppy = || crate::disk::MountOptions { kind: DriveKind::Floppy, ..Default::default() };
        cpu.bus.disk.mount(0, &image, floppy(), false).unwrap();
        let record = |cpu: &Cpu| {
            let mut media = of(cpu);
            add_contents(cpu, &mut media, &dir.join("a.state"), |m| m.drive == "A");
            media
        };
        let saved = record(&cpu);
        let a = saved.iter().find(|m| m.drive == "A").unwrap();
        assert_eq!(a.images[0].sha256.as_ref().map(String::len), Some(64));
        assert!(has_contents(a));
        let c = saved.iter().find(|m| m.drive == "C").unwrap();
        assert!(!has_contents(c), "C: wasn't wanted");

        // A sector past the first 64 KiB, written by the machine.
        cpu.bus.disk.bios_image(0).unwrap().write(400, &[0x5A; 512]).unwrap();
        let d = differences(&saved, &record(&cpu));
        assert_eq!(d.len(), 1, "{:?}", d);
        assert!(d[0].starts_with("drive A: image ") && d[0].ends_with("its contents differ from when it was saved"), "{}", d[0]);

        // With a delta file the image stays as it is and the delta changes.
        cpu.bus.disk.unmount(0).unwrap();
        let delta = crate::disk::MountOptions { overlay: Some(dir.join("delta")), ..floppy() };
        cpu.bus.disk.mount(0, &image, delta, false).unwrap();
        let saved = record(&cpu);
        let a = saved.iter().find(|m| m.drive == "A").unwrap();
        assert!(a.overlay.is_some() && a.overlay_sha256.is_some(), "{:?}", a);
        cpu.bus.disk.bios_image(0).unwrap().write(401, &[0xA5; 512]).unwrap();
        let d = differences(&saved, &record(&cpu));
        assert_eq!(d.len(), 1, "{:?}", d);
        assert!(d[0].starts_with("drive A: delta file "), "{}", d[0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_state_without_a_record_of_a_writable_drive_names_it_unless_its_disk_is_copied() {
        let read_only_cd = cd("/cds/game.cue", 1000, "ab");
        let disk = Medium {
            drive: "2".into(),
            images: vec![Image { path: "/disks/c.img".into(), size: Some(10), ..Default::default() }],
            ..Default::default()
        };
        let saved = [folder("C", "/games/keen"), read_only_cd, disk];
        assert_eq!(
            unrecorded(&saved, |_| false),
            [
                "drive C: wasn't checked: the state doesn't record what is on it (saved without strict_match, or by an older rust-dos)",
                "disk 2 wasn't checked: the state doesn't record what is on it (saved without strict_match, or by an older rust-dos)",
            ]
        );
        assert_eq!(unrecorded(&saved, |key| key == "2").len(), 1, "a booted system's disk comes back from its copy");
        assert!(unrecorded(&[with_files(Vec::new())], |_| false).is_empty());
    }

    #[test]
    fn a_record_from_before_contents_were_recorded_reads() {
        let json = r#"{"drive":"C","kind":"hdd","read_only":false,"folder":"/games/keen","images":[{"path":"/x.img","size":10}]}"#;
        let medium: Medium = serde_json::from_str(json).unwrap();
        assert_eq!((medium.files, medium.overlay_sha256, medium.images[0].sha256.clone()), (None, None, None));
    }
}
