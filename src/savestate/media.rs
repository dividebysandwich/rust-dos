//! The media a state was saved with: each mounted drive's host folder or
//! disk and CD images, as the slot file's header records them. A state
//! keeps the drives by their paths, not what is in them (disk/state.rs),
//! so a load through the debug server checks that the same media are
//! there before it puts the state in: the same drives, mounted from the
//! same paths, and images of the same size. A read-only image (a CD's, a
//! `-ro` mount's) is also told apart by a hash of its start; a writable
//! one changes as the machine writes it, so only its size is recorded.

use crate::cpu::Cpu;
use crate::disk::{DriveInfo, drive_key};
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
}

/// The media of the machine's mounted drives now. The drives the machine
/// has of itself (Z:, held in memory) have none and aren't listed.
pub fn of(cpu: &Cpu) -> Vec<Medium> {
    cpu.bus.disk.all_drives().iter().filter_map(medium).collect()
}

fn medium(info: &DriveInfo) -> Option<Medium> {
    let mut listed: Vec<PathBuf> = info.images.clone();
    if listed.is_empty() {
        listed.extend(info.image.clone());
    }
    // A CUE sheet's tracks are in the files it names.
    let paths: Vec<PathBuf> =
        listed.into_iter().flat_map(|p| std::iter::once(p.clone()).chain(crate::cdrom::image::cue_files(&p))).collect();
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
    })
}

fn image(path: &Path, read_only: bool, shown: String) -> Image {
    let meta = hostfs::metadata(path).ok();
    let size = meta.as_ref().map(|m| m.len);
    let head_sha256 = match (read_only, &meta) {
        (true, Some(meta)) => head_hash(path, meta.len, meta.modified),
        _ => None,
    };
    Image { path: shown, size, head_sha256 }
}

type Known = HashMap<PathBuf, (u64, Option<SystemTime>, String)>;

/// The head hashes `head_hash` keeps, by path.
static KNOWN: Mutex<Option<Known>> = Mutex::new(None);

/// Forget the head hashes kept, so the next record reads every read-only
/// image again. A checkpoint does this: a file replaced by another of the
/// same size with its time kept (`cp -p`, `rsync -t`, an unpacked
/// archive) has the cached hash of the old one.
pub fn forget_hashes() {
    *KNOWN.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The hash of the start of the file at `path`, kept while its size and
/// time stay the same: the libretro core takes a state every frame.
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
    let hash: String = Sha256::digest(&head).iter().map(|b| format!("{:02x}", b)).collect();
    known.insert(path.to_path_buf(), (size, modified, hash.clone()));
    Some(hash)
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
        let name = if drive.chars().all(|c| c.is_ascii_digit()) { format!("disk {}", drive) } else { format!("drive {}:", drive) };
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
            if a.size != b.size {
                out.push(format!("{} image {}: {} when saved, {} now", name, a.path, size(a.size), size(b.size)));
            } else if a.head_sha256.is_some() && b.head_sha256.is_some() && a.head_sha256 != b.head_sha256 {
                out.push(format!("{} image {}: its first {} KiB differ from when it was saved", name, a.path, HEAD >> 10));
            }
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
            images: vec![Image { path: path.into(), size: Some(size), head_sha256: Some(hash.into()) }],
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

        let moved = [folder("C", "/games/other"), cd("/cds/game.cue", 1000, "ab")];
        let d = differences(&saved, &moved);
        assert_eq!(d, ["drive C: folder: the state has /games/keen, this machine has /games/other"]);

        let no_cd = [folder("C", "/games/keen")];
        assert!(differences(&saved, &no_cd)[0].starts_with("drive D: is not mounted here"));
        assert!(differences(&no_cd, &saved)[0].starts_with("drive D: is mounted here"));
    }

    #[test]
    fn a_folder_and_a_read_only_image_are_recorded() {
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
        // a checkpoint forgets it.
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
}
