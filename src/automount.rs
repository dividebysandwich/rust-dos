//! A game package's `automount` folder: images and folders at its top
//! that are drives by their names (GAME-PACKAGES.md). The name is the
//! drive's letter, then a number for the order of several discs or
//! floppies in one drive, then a label in brackets: `d.cue`, `d1.iso` (or
//! `d.iso1`), `d2[DISC3]/` are D:'s three discs, Ctrl+F4 changing them;
//! `c.vhd` is C:, a hard disk image; `a.img` and `a1.img` are floppies in
//! A:. A folder is a CD (`e/`), or a hard drive with `.hd` (`e.hd/`).
//! Other images, archives and folders there are systems that `os=` can
//! name, as the OS images folder's are (`os_images`).

use crate::disk::{DRIVE_C, DriveKind, FLOPPY_DRIVES, MountOptions};
use crate::mount::MountSpec;
use std::path::{Path, PathBuf};

/// The folder's name.
pub const FOLDER: &str = "automount";

const CD_EXTENSIONS: [&str; 5] = ["cue", "iso", "bin", "ins", "chd"];
const DISK_EXTENSIONS: [&str; 7] = ["vhd", "img", "ima", "dsk", "vfd", "flp", "86f"];
/// A system's, in the order `os_images` looks for a name.
const SYSTEM_EXTENSIONS: [&str; 6] = ["img", "vhd", "ima", "dosz", "zip", "7z"];

/// What an entry of the folder is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A hard disk image.
    HardDisk,
    /// CD images and folders.
    Cd,
    /// A folder as a hard drive.
    Folder,
    /// Floppy images, in A: or B:.
    Floppy,
}

/// A drive the folder has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub drive: u8,
    pub kind: Kind,
    /// Its images in order, or the one folder.
    pub paths: Vec<PathBuf>,
    pub label: Option<String>,
}

impl Mount {
    /// The drive as a lettered one of DOS's.
    pub fn spec(&self) -> MountSpec {
        let kind = match self.kind {
            Kind::Cd => DriveKind::CdRom,
            Kind::Floppy => DriveKind::Floppy,
            Kind::HardDisk | Kind::Folder => DriveKind::HardDisk,
        };
        let opts = MountOptions { kind, label: self.label.clone(), more_images: self.paths[1..].to_vec(), ..Default::default() };
        MountSpec { drive: self.drive, path: self.paths[0].clone(), opts }
    }
}

/// What a package's folder holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    pub mounts: Vec<Mount>,
    /// The entries that aren't drives: (name in lower case without its
    /// extension, path, whether it is a folder).
    pub systems: Vec<(String, PathBuf, bool)>,
    /// What was left out, and why.
    pub warnings: Vec<String>,
}

impl Scan {
    /// The drive the folder has as `drive`.
    pub fn mount(&self, drive: u8) -> Option<&Mount> {
        self.mounts.iter().find(|m| m.drive == drive)
    }

    /// The system called `name` (with or without its extension) the
    /// folder has, as `os_images::find` looks in the OS images folder.
    pub fn system(&self, name: &str) -> Option<PathBuf> {
        let lower = name.to_ascii_lowercase();
        let file_name = |p: &Path| p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name));
        let extension = |p: &Path| p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
        let named = |ext: Option<&str>| self.systems.iter().find(|(stem, p, _)| *stem == lower && extension(p).as_deref() == ext);
        self.systems
            .iter()
            .find(|(_, p, _)| file_name(p))
            .or_else(|| SYSTEM_EXTENSIONS.iter().find_map(|ext| named(Some(ext))))
            .or_else(|| self.systems.iter().find(|(stem, _, dir)| *dir && *stem == lower))
            .map(|(_, p, _)| p.clone())
    }

    /// Whether the system at `path` is files to run rather than a disk to
    /// boot, as `os_images::holds_files` tells: a folder or an archive.
    pub fn holds_files(&self, path: &Path) -> bool {
        crate::os_images::holds_files(path) || self.systems.iter().any(|(_, p, dir)| *dir && p == path)
    }
}

/// An entry of the folder: its name, and whether it is a folder.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    /// Its size, for a file.
    pub len: u64,
}

/// The folder of the package at `package` (an archive or a folder), as
/// it is named there, and its entries. None without one.
pub fn entries(package: &Path) -> Option<(String, Vec<Entry>)> {
    use crate::overlay::Lower;
    if crate::hostfs::is_dir(package) {
        let dir = crate::hostfs::read_dir(package)
            .ok()?
            .into_iter()
            .find(|e| e.is_dir && e.name.to_string_lossy().eq_ignore_ascii_case(FOLDER))?;
        let entries = crate::hostfs::read_dir(&dir.path)
            .ok()?
            .into_iter()
            .map(|e| Entry {
                name: e.name.to_string_lossy().into_owned(),
                is_dir: e.is_dir,
                len: if e.is_dir { 0 } else { e.metadata().map_or(0, |m| m.len) },
            })
            .collect();
        return Some((dir.name.to_string_lossy().into_owned(), entries));
    }
    let stack = crate::archive::open(package).ok()?;
    let mut folder = None;
    let mut entries: Vec<Entry> = Vec::new();
    for file in stack.files() {
        let Some((top, rest)) = file.split_once('/') else { continue };
        if !top.eq_ignore_ascii_case(FOLDER) {
            continue;
        }
        folder.get_or_insert_with(|| top.to_string());
        let (name, is_dir) = match rest.split_once('/') {
            Some((dir, _)) => (dir, true),
            None => (rest, false),
        };
        if name.is_empty() || entries.iter().any(|e| e.name.eq_ignore_ascii_case(name)) {
            continue;
        }
        let len = if is_dir { 0 } else { stack.metadata(&file).map_or(0, |m| m.len) };
        entries.push(Entry { name: name.to_string(), is_dir, len });
    }
    Some((folder?, entries))
}

/// The package's folder (`entries`), what is in it by name and size, to
/// tell when it changed.
pub fn listing(package: &Path) -> String {
    let Some((_, mut entries)) = entries(package) else { return String::new() };
    entries.sort_by_key(|e| e.name.to_ascii_lowercase());
    entries.iter().map(|e| format!("{}{}:{}\n", e.name, if e.is_dir { "/" } else { "" }, e.len)).collect()
}

/// The drives and systems in the package at `package`.
pub fn scan(package: &Path) -> Scan {
    match entries(package) {
        Some((folder, entries)) => scan_entries(&package.join(folder), &entries),
        None => Scan::default(),
    }
}

/// A name of the folder as a drive's: its letter (0 for A:), its number,
/// its label, and its kind; None for a name that isn't one.
fn parse(name: &str, is_dir: bool) -> Option<(u8, u32, Option<String>, Kind)> {
    let lower = name.to_ascii_lowercase();
    let letter = lower.chars().next().filter(char::is_ascii_lowercase)?;
    let drive = letter as u8 - b'a';
    let rest = &lower[1..];
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let mut number = rest[..digits].parse::<u32>().ok();
    let mut rest = &name[1 + digits..];
    let mut label = None;
    if let Some(inner) = rest.strip_prefix('[') {
        let (text, after) = inner.split_once(']')?;
        label = Some(text.to_string()).filter(|l| !l.trim().is_empty());
        rest = after;
    }
    let rest = rest.to_ascii_lowercase();
    let kind = if is_dir {
        match rest.as_str() {
            "" => Kind::Cd,
            ".hd" => Kind::Folder,
            _ => return None,
        }
    } else {
        let ext = rest.strip_prefix('.')?;
        // `d.iso1`: the number after the extension.
        let digits = ext.len() - ext.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        let (ext, after) = ext.split_at(ext.len() - digits);
        if !after.is_empty() {
            if number.is_some() {
                return None;
            }
            number = after.parse().ok();
        }
        match ext {
            e if CD_EXTENSIONS.contains(&e) => Kind::Cd,
            e if DISK_EXTENSIONS.contains(&e) && drive < FLOPPY_DRIVES => Kind::Floppy,
            e if DISK_EXTENSIONS.contains(&e) => Kind::HardDisk,
            _ => return None,
        }
    };
    Some((drive, number.unwrap_or(0), label, kind))
}

/// The files a CUE sheet `text` names.
fn cue_files(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line.get(..5).filter(|k| k.eq_ignore_ascii_case("FILE ")).map(|_| line[5..].trim())?;
            let name = match rest.strip_prefix('"') {
                Some(quoted) => quoted.split('"').next()?,
                None => rest.split_whitespace().next()?,
            };
            Some(name.rsplit(['/', '\\']).next().unwrap_or(name).to_string())
        })
        .collect()
}

/// `scan` of the folder at `folder` with `entries`.
fn scan_entries(folder: &Path, entries: &[Entry]) -> Scan {
    let mut scan = Scan::default();
    let mut entries: Vec<&Entry> = entries.iter().collect();
    entries.sort_by_key(|e| e.name.to_ascii_lowercase());
    // The tracks a CUE sheet names are its disc's, not discs of their own.
    let mut tracks: Vec<String> = Vec::new();
    for entry in entries.iter().filter(|e| !e.is_dir && e.name.to_ascii_lowercase().ends_with(".cue")) {
        let text = crate::archive::read_member(&folder.join(&entry.name))
            .unwrap_or_else(|| crate::hostfs::read(folder.join(&entry.name)).map_err(|e| e.to_string()))
            .map(|data| String::from_utf8_lossy(&data).into_owned())
            .unwrap_or_default();
        tracks.extend(cue_files(&text).into_iter().map(|t| t.to_ascii_lowercase()));
    }
    let mut found: Vec<(u8, u32, Option<String>, Kind, &Entry)> = Vec::new();
    for entry in entries {
        if tracks.contains(&entry.name.to_ascii_lowercase()) {
            continue;
        }
        match parse(&entry.name, entry.is_dir) {
            Some((drive, _, _, kind)) if drive == DRIVE_C && kind == Kind::Cd => {
                scan.warnings.push(format!("{}/{}: C: can't be a CD-ROM drive", FOLDER, entry.name));
            }
            Some((drive, number, label, kind)) => found.push((drive, number, label, kind, entry)),
            None => {
                let name = entry.name.to_ascii_lowercase();
                let stem = match entry.is_dir {
                    true => name,
                    false => name.rsplit_once('.').map_or(name.clone(), |(stem, _)| stem.to_string()),
                };
                scan.systems.push((stem, folder.join(&entry.name), entry.is_dir));
            }
        }
    }
    for drive in 0..crate::disk::DRIVE_Z {
        let mut here: Vec<_> = found.iter().filter(|f| f.0 == drive).collect();
        let Some(kind) = here.iter().map(|f| f.3).min() else { continue };
        let name = |f: &&(u8, u32, Option<String>, Kind, &Entry)| format!("{}/{}", FOLDER, f.4.name);
        for other in here.iter().filter(|f| f.3 != kind) {
            scan.warnings.push(format!("{}: {}: is a {} already", name(other), letter(drive), kind_name(kind)));
        }
        here.retain(|f| f.3 == kind);
        here.sort_by_key(|f| f.1);
        let mut kept: Vec<&&(u8, u32, Option<String>, Kind, &Entry)> = Vec::new();
        for f in &here {
            match kept.iter().find(|k| k.1 == f.1) {
                Some(first) => scan.warnings.push(format!("{}: {} is {}'s disc {} already", name(f), name(first), letter(drive), f.1)),
                None => kept.push(f),
            }
        }
        // Several discs or floppies, but a folder only as the one disc;
        // one hard disk or folder.
        let why = match kind {
            Kind::Cd if kept.iter().any(|f| !f.4.is_dir) => "a folder can't be one of a drive's several discs",
            Kind::Floppy => "",
            _ => "a drive has one",
        };
        let images = kind == Kind::Cd && kept.iter().any(|f| !f.4.is_dir);
        let mut left: Vec<_> = Vec::new();
        for (i, f) in kept.into_iter().enumerate() {
            let stays = match kind {
                Kind::Floppy => true,
                Kind::Cd if images => !f.4.is_dir,
                _ => i == 0,
            };
            match stays {
                true => left.push(f),
                false => scan.warnings.push(format!("{} is left out: {}", name(f), why)),
            }
        }
        let kept = left;
        let label = kept.iter().find_map(|f| f.2.clone());
        let paths = kept.iter().map(|f| folder.join(&f.4.name)).collect();
        scan.mounts.push(Mount { drive, kind, paths, label });
    }
    for mount in &scan.mounts {
        if mount.paths.iter().any(|p| p.extension().is_some_and(|e| e.to_string_lossy().to_ascii_lowercase().starts_with("chd"))) {
            scan.warnings.push(format!("{}: CHD images can't be read yet", letter(mount.drive)));
        }
    }
    scan
}

fn letter(drive: u8) -> String {
    format!("{}:", crate::disk::drive_letter(drive))
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::HardDisk => "hard disk",
        Kind::Cd => "CD-ROM drive",
        Kind::Folder => "folder",
        Kind::Floppy => "floppy drive",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> Entry {
        Entry { name: name.to_string(), is_dir: false, len: 1 }
    }

    fn dir(name: &str) -> Entry {
        Entry { name: name.to_string(), is_dir: true, len: 0 }
    }

    fn names(mount: &Mount) -> Vec<String> {
        mount.paths.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn names_are_letters_numbers_and_labels() {
        assert_eq!(parse("d.cue", false), Some((3, 0, None, Kind::Cd)));
        assert_eq!(parse("D2.ISO", false), Some((3, 2, None, Kind::Cd)));
        assert_eq!(parse("d.iso1", false), Some((3, 1, None, Kind::Cd)));
        assert_eq!(parse("d[Game CD].iso", false), Some((3, 0, Some("Game CD".into()), Kind::Cd)));
        assert_eq!(parse("e3[DISC4]", true), Some((4, 3, Some("DISC4".into()), Kind::Cd)));
        assert_eq!(parse("e.hd", true), Some((4, 0, None, Kind::Folder)));
        assert_eq!(parse("c.vhd", false), Some((2, 0, None, Kind::HardDisk)));
        assert_eq!(parse("a1.img", false), Some((0, 1, None, Kind::Floppy)));
        assert_eq!(parse("win98se.vhd", false), None);
        assert_eq!(parse("d1.iso2", false), None);
        assert_eq!(parse("d.txt", false), None);
        assert_eq!(parse("dos", true), None);
    }

    #[test]
    fn discs_go_in_order_with_their_tracks_left_out() {
        let folder = Path::new("/nowhere/automount");
        let scan = scan_entries(folder, &[file("d.iso"), file("win98se.vhd"), dir("win31")]);
        assert!(scan.systems.iter().any(|(n, _, _)| n == "win98se"));
        assert_eq!(scan.system("WIN98SE"), Some(folder.join("win98se.vhd")));
        assert_eq!(scan.system("win31"), Some(folder.join("win31")));
        assert!(scan.holds_files(&folder.join("win31")) && !scan.holds_files(&folder.join("win98se.vhd")));
        assert_eq!(scan.system("win95"), None);
        let scan = scan_entries(folder, &[file("d.cue"), file("d.iso2"), file("d1[TWO].iso"), file("c.vhd")]);
        let d = scan.mount(3).unwrap();
        assert_eq!(names(d), ["d.cue", "d1[TWO].iso", "d.iso2"]);
        assert_eq!(d.label.as_deref(), Some("TWO"));
        let spec = d.spec();
        assert_eq!(spec.opts.kind, DriveKind::CdRom);
        assert_eq!(spec.opts.more_images.len(), 2);
        assert_eq!(scan.mount(2).unwrap().kind, Kind::HardDisk);
        assert!(scan.warnings.is_empty(), "{:?}", scan.warnings);
    }

    #[test]
    fn cue_sheets_name_their_tracks() {
        let cue = "FILE \"GAME (Track 1).bin\" BINARY\n  TRACK 01 MODE1/2352\nfile track2.wav WAVE\n";
        assert_eq!(cue_files(cue), ["GAME (Track 1).bin", "track2.wav"]);
    }

    #[test]
    fn a_drive_has_one_kind_and_one_disc_of_a_number() {
        let folder = Path::new("/p/automount");
        let scan = scan_entries(folder, &[file("d.vhd"), file("d1.iso"), file("e.iso"), file("e0.cue"), dir("c"), dir("f.hd"), dir("f1.hd")]);
        assert_eq!(scan.mount(3).unwrap().kind, Kind::HardDisk);
        assert_eq!(names(scan.mount(4).unwrap()), ["e.iso"]);
        assert_eq!(names(scan.mount(5).unwrap()), ["f.hd"]);
        assert!(scan.mount(2).is_none());
        assert_eq!(scan.warnings.len(), 4, "{:?}", scan.warnings);
    }

    #[test]
    fn a_folder_disc_is_on_its_own() {
        let folder = Path::new("/p/automount");
        let scan = scan_entries(folder, &[file("d.iso"), dir("d1"), file("d2.iso")]);
        assert_eq!(names(scan.mount(3).unwrap()), ["d.iso", "d2.iso"]);
        assert_eq!(scan.warnings.len(), 1, "{:?}", scan.warnings);
        let scan = scan_entries(folder, &[dir("e[DISC1]")]);
        let e = scan.mount(4).unwrap();
        assert_eq!((names(e), e.label.as_deref()), (vec!["e[DISC1]".to_string()], Some("DISC1")));
        assert!(scan.warnings.is_empty());
    }

    #[test]
    fn floppies_are_a_and_b() {
        let scan = scan_entries(Path::new("/p/automount"), &[file("a.img"), file("a1.img"), file("b.ima")]);
        assert_eq!(scan.mount(0).unwrap().spec().opts.kind, DriveKind::Floppy);
        assert_eq!(names(scan.mount(0).unwrap()), ["a.img", "a1.img"]);
        assert_eq!(scan.mount(1).unwrap().kind, Kind::Floppy);
    }

    #[test]
    fn a_folder_package_is_scanned_with_its_cue_sheets_tracks() {
        let package = std::env::temp_dir().join(format!("rust-dos-automount-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&package);
        let folder = package.join("AutoMount");
        std::fs::create_dir_all(folder.join("e1[SECOND]")).unwrap();
        std::fs::create_dir_all(folder.join("deeper/f.iso")).unwrap();
        std::fs::write(folder.join("d.cue"), "FILE \"d.bin\" BINARY\n").unwrap();
        std::fs::write(folder.join("d.bin"), [0u8; 2352]).unwrap();
        std::fs::write(folder.join("d1.iso"), [0u8; 2048]).unwrap();
        std::fs::write(package.join("GAME.EXE"), b"MZ").unwrap();
        let scan = scan(&package);
        assert_eq!(names(scan.mount(3).unwrap()), ["d.cue", "d1.iso"]);
        assert_eq!(scan.mount(4).unwrap().paths, [folder.join("e1[SECOND]")]);
        assert_eq!(scan.system("deeper"), Some(folder.join("deeper")));
        assert!(scan.warnings.is_empty(), "{:?}", scan.warnings);
        assert!(listing(&package).contains("d.bin:2352"));
        assert_eq!(listing(&package.join("nothing")), "");
        let _ = std::fs::remove_dir_all(&package);
    }
}
