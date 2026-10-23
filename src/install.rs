//! A game profile's `[install]` section: the files of InstallShield
//! installer archives on the game's drives, unpacked onto its drives the
//! first time it starts (unpack.rs reads and places them).
//!
//! ```ini
//! [install]
//! from=D:\SETUP\DATA1.HDR
//! to=C:\GAMES\ONI
//! group=Files (Program)
//! group=Files (Data)|DATA
//! ```
//!
//! Each `from=` starts a step: a cabinet set's header or an InstallShield
//! 3 archive, by its DOS path. The lines after it, up to the next
//! `from=`, say where its files go: `to=` is the DOS folder, `group=` a
//! file group to unpack, with a folder under `to=` after a `|`, and
//! `compression=markers` reads a set whose chunks end with 00 00 FF FF
//! (Unshield's `-O`). Without `group=` lines every file goes to its own
//! path. A step that unpacked every file writes `RUSTDOS.INS` in its
//! folder, and isn't run again while that file is there.

use crate::disk::{DiskController, FileData};
use crate::unpack::{Source, missing_group, placements};
use rust_dos_installshield::{Compression, Folder, Lookup, ReadSeek};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::rc::Rc;

/// The file a finished step leaves in its folder.
pub const MARKER: &str = "RUSTDOS.INS";

/// One archive to unpack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    /// The cabinet set's header or the InstallShield 3 archive, a DOS
    /// path with its drive.
    pub from: String,
    pub compression: Compression,
    /// The DOS folder its files go to, with its drive.
    pub to: String,
    /// The file groups to unpack, each with its folder under `to` (empty
    /// for `to` itself). None unpacks every file.
    pub groups: Vec<(String, String)>,
}

/// The steps of an `[install]` section's lines, in order, and what is
/// wrong with them. A step without `from=` or `to=` is left out.
pub fn parse(lines: &[(String, String)]) -> (Vec<Step>, Vec<String>) {
    let mut steps: Vec<Step> = Vec::new();
    let mut warnings = Vec::new();
    for (key, value) in lines {
        let value = value.trim();
        if key == "from" {
            steps.push(Step {
                from: dos_path(value),
                compression: Compression::Chunks,
                to: String::new(),
                groups: Vec::new(),
            });
            continue;
        }
        let Some(step) = steps.last_mut() else {
            warnings.push(format!("install: {}= comes before from=", key));
            continue;
        };
        match key.as_str() {
            "to" => step.to = dos_path(value),
            "compression" => match Compression::parse(value) {
                Some(c) => step.compression = c,
                None => warnings.push(format!(
                    "install: invalid compression '{}' (chunks or markers)",
                    value
                )),
            },
            "group" => {
                let (name, folder) = value.split_once('|').unwrap_or((value, ""));
                step.groups.push((
                    name.trim().to_string(),
                    dos_path(folder).trim_matches('\\').to_string(),
                ));
            }
            _ => warnings.push(format!("install: unknown setting '{}'", key)),
        }
    }
    steps.retain(|step| {
        let usable = has_drive(&step.from) && has_drive(&step.to);
        if !usable {
            warnings.push(format!(
                "install: from={} needs a to= folder, and both a drive letter",
                step.from
            ));
        }
        usable
    });
    (steps, warnings)
}

fn dos_path(text: &str) -> String {
    text.trim().replace('/', "\\")
}

fn has_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

/// What running the steps did: the steps that finished, and what went
/// wrong.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub installed: Vec<String>,
    pub warnings: Vec<String>,
}

/// Run each step whose folder has no `MARKER`.
pub fn run(disk: &mut DiskController, steps: &[Step]) -> Outcome {
    let mut outcome = Outcome::default();
    for step in steps {
        let marker = format!("{}\\{}", step.to.trim_end_matches('\\'), MARKER);
        if disk.exists(&marker) {
            continue;
        }
        if let Err(e) = run_step(disk, step, &mut outcome) {
            outcome
                .warnings
                .push(format!("install from {}: {}", step.from, e));
        }
    }
    outcome
}

fn run_step(disk: &mut DiskController, step: &Step, outcome: &mut Outcome) -> Result<(), String> {
    let (dir, name) = step.from.rsplit_once('\\').ok_or("no file name")?;
    let folder = DosFolder::new(dir);
    folder.add(disk, name);
    // A set's volumes, data1.cab, data2.cab... beside its header.
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let prefix = stem.trim_end_matches(|c: char| c.is_ascii_digit());
    for n in 1..=99 {
        if !folder.add(disk, &format!("{}{}.cab", prefix, n)) {
            break;
        }
    }
    // Opening looks up files kept outside the cabinets too: those it
    // didn't find are looked up on the drives, and it is opened again.
    let mut source = Source::open_in(Box::new(folder.clone()), name, step.compression);
    for _ in 0..4 {
        if !folder.add_missed(disk) {
            break;
        }
        source = Source::open_in(Box::new(folder.clone()), name, step.compression);
    }
    let source = source?;
    let names: Vec<String> = step.groups.iter().map(|(name, _)| name.clone()).collect();
    if let Some(missing) = missing_group(&source, &names) {
        return Err(missing);
    }
    let (placed, warnings) = placements(&source, &step.groups);
    let messages_prefix = format!("install from {}: ", step.from);
    let messages = &mut outcome.warnings;
    messages.extend(
        warnings
            .into_iter()
            .map(|w| format!("{}{}", messages_prefix, w)),
    );
    let to = step.to.trim_end_matches('\\');
    make_folders(disk, to);
    let mut failed = 0;
    for p in &placed {
        let target = format!("{}\\{}", to, p.path.replace('/', "\\"));
        if let Some((parent, _)) = target.rsplit_once('\\') {
            make_folders(disk, parent);
        }
        let written = source.read_entry(p.index).and_then(|data| {
            disk.write_whole_file(&target, &data, None)
                .map_err(write_error)
        });
        if let Err(e) = written {
            messages.push(format!("{}{}: {}", messages_prefix, target, e));
            failed += 1;
        }
    }
    if failed > 0 {
        messages.push(format!(
            "install from {}: {} of {} files weren't unpacked; it runs again next time",
            step.from,
            failed,
            placed.len()
        ));
        return Ok(());
    }
    let marker = format!("{}\\{}", to, MARKER);
    let record = format!("from={}\r\nfiles={}\r\n", step.from, placed.len());
    disk.write_whole_file(&marker, record.as_bytes(), None)
        .map_err(|e| format!("{}: {}", marker, write_error(e)))?;
    outcome.installed.push(format!(
        "installed {} files from {} to {}",
        placed.len(),
        step.from,
        to
    ));
    Ok(())
}

fn write_error(code: u8) -> String {
    match code {
        0x03 => "the folder isn't there".to_string(),
        0x05 => "can't be written (a read-only drive, or a folder of that name)".to_string(),
        code => format!("DOS error {:#04x}", code),
    }
}

/// Create the folder `path` (with its drive) and the ones above it that
/// aren't there.
fn make_folders(disk: &DiskController, path: &str) {
    let mut at = String::new();
    for (i, part) in path.split('\\').enumerate() {
        if i > 0 {
            at.push('\\');
        }
        at.push_str(part);
        if i > 0 && !part.is_empty() && !disk.is_directory(&at) {
            let _ = disk.create_directory(&at);
        }
    }
}

/// A DOS folder for the readers. They hold it while they read, and the
/// drives can't be borrowed that long, so it holds where each file it
/// was given is (`FileData`) and notes the paths it was asked for that
/// it doesn't hold, to be looked up (`add_missed`).
#[derive(Clone)]
struct DosFolder {
    dir: String,
    files: Rc<RefCell<HashMap<String, FileData>>>,
    missed: Rc<RefCell<Vec<String>>>,
}

impl DosFolder {
    fn new(dir: &str) -> DosFolder {
        DosFolder {
            dir: dir.trim_end_matches('\\').to_string(),
            files: Rc::default(),
            missed: Rc::default(),
        }
    }

    fn full(&self, path: &str) -> String {
        format!("{}\\{}", self.dir, path.replace('/', "\\"))
    }

    /// Look up `path` (relative) on the drives: whether it is a file.
    fn add(&self, disk: &DiskController, path: &str) -> bool {
        let full = self.full(path);
        match disk.file_data(&full) {
            Some(data) => {
                self.files.borrow_mut().insert(full.to_lowercase(), data);
                true
            }
            None => false,
        }
    }

    /// Look up the paths asked for that it didn't hold: whether any was
    /// found.
    fn add_missed(&self, disk: &DiskController) -> bool {
        let missed = std::mem::take(&mut *self.missed.borrow_mut());
        let mut found = false;
        for path in missed {
            found |= self.add(disk, &path);
        }
        found
    }
}

impl Folder for DosFolder {
    fn lookup(&self, path: &str) -> std::io::Result<Lookup> {
        let data = self
            .files
            .borrow()
            .get(&self.full(path).to_lowercase())
            .cloned();
        let Some(data) = data else {
            let mut missed = self.missed.borrow_mut();
            if missed.len() < 10_000 {
                missed.push(path.to_string());
            }
            return Ok(Lookup::Missing);
        };
        let (stream, len): (Box<dyn ReadSeek>, u64) = match data {
            FileData::Host(path) => {
                let file = crate::hostfs::File::open(&path)?;
                let len = file.len()?;
                (Box::new(file), len)
            }
            FileData::Memory(bytes) => {
                let len = bytes.len() as u64;
                (Box::new(std::io::Cursor::new(bytes)), len)
            }
            FileData::Image(image, extent) => {
                let len = extent.size as u64;
                let read = move |pos: u64, buf: &mut [u8]| image.read_extent(&extent, pos, buf);
                (Box::new(Reader { read, len, pos: 0 }), len)
            }
            FileData::Fat(volume, entry) => {
                let len = entry.size as u64;
                let read = move |pos: u64, buf: &mut [u8]| {
                    volume
                        .read(&entry, pos, buf)
                        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))
                };
                (Box::new(Reader { read, len, pos: 0 }), len)
            }
        };
        Ok(Lookup::Found(stream, len))
    }
}

/// A file on a disc or disk image, read from a position at a time.
struct Reader<F> {
    read: F,
    len: u64,
    pos: u64,
}

impl<F: FnMut(u64, &mut [u8]) -> std::io::Result<usize>> Read for Reader<F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.len {
            return Ok(0);
        }
        let n = (self.read)(self.pos, buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl<F> Seek for Reader<F> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or(std::io::ErrorKind::InvalidInput)?;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::MountOptions;
    use rust_dos_installshield::write::{
        self, ArchiveFile, CabinetFile, CabinetGroup, CabinetOptions,
    };

    fn lines(text: &str) -> Vec<(String, String)> {
        text.lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.to_string()))
            .collect()
    }

    #[test]
    fn steps_parse_in_order() {
        let (steps, warnings) = parse(&lines(
            r"from=D:\SETUP\DATA1.HDR
to=C:\GAMES\X\
compression=markers
group=Program
group=Data Files | DATA
from=d:/_setup.1
to=C:\OLD",
        ));
        assert!(warnings.is_empty(), "{:?}", warnings);
        assert_eq!(
            steps,
            [
                Step {
                    from: r"D:\SETUP\DATA1.HDR".to_string(),
                    compression: Compression::Markers,
                    to: r"C:\GAMES\X\".to_string(),
                    groups: vec![
                        ("Program".to_string(), String::new()),
                        ("Data Files".to_string(), "DATA".to_string())
                    ],
                },
                Step {
                    from: r"d:\_setup.1".to_string(),
                    compression: Compression::Chunks,
                    to: r"C:\OLD".to_string(),
                    groups: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn bad_lines_are_warned_about() {
        let (steps, warnings) = parse(&lines(
            r"to=C:\X
from=D:\A.HDR
compression=zip
size=3
from=B.HDR
to=C:\Y",
        ));
        assert!(steps.is_empty());
        assert_eq!(
            warnings,
            [
                "install: to= comes before from=",
                "install: invalid compression 'zip' (chunks or markers)",
                "install: unknown setting 'size'",
                r"install: from=D:\A.HDR needs a to= folder, and both a drive letter",
                "install: from=B.HDR needs a to= folder, and both a drive letter",
            ]
        );
    }

    /// C: and D: host folders under `name`, with `files` in D:\SETUP.
    fn drives(name: &str, files: &[(String, Vec<u8>)]) -> (DiskController, std::path::PathBuf) {
        let base = crate::test_dir::path(name);
        std::fs::create_dir_all(base.join("c")).unwrap();
        std::fs::create_dir_all(base.join("d/setup")).unwrap();
        for (file, data) in files {
            std::fs::write(base.join("d/setup").join(file), data).unwrap();
        }
        let mut disk = DiskController::new(base.join("c"));
        disk.mount(3, &base.join("d"), MountOptions::default(), false)
            .unwrap();
        (disk, base)
    }

    #[test]
    fn a_split_set_unpacks_by_group_once() {
        let map: Vec<u8> = (0..5000u32).map(|i| (i * 7 % 251) as u8).collect();
        let files = [
            // Stored as they are, so they take the volumes' room.
            CabinetFile {
                compressed: false,
                ..CabinetFile::new("", "GAME.EXE", &[7u8; 3000])
            },
            CabinetFile {
                compressed: false,
                ..CabinetFile::new("maps", "level1.map", &map)
            },
            CabinetFile::new("", "unused.txt", b"not in a group we want"),
        ];
        let options = CabinetOptions {
            volume_capacity: 2000,
            groups: vec![
                CabinetGroup::new("Program", 0, 0),
                CabinetGroup::new("Maps", 1, 1),
                CabinetGroup::new("Extras", 2, 2),
            ],
            ..CabinetOptions::default()
        };
        let set = write::cabinet(5, &files, &options);
        assert!(set.len() > 2, "the set spans volumes");
        let (mut disk, base) = drives("install-split", &set);
        let (steps, _) = parse(&lines(
            r"from=D:\SETUP\DATA1.HDR
to=C:\GAMES\X
group=program
group=Maps|DATA",
        ));
        let outcome = run(&mut disk, &steps);
        assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
        assert_eq!(
            outcome.installed,
            [r"installed 2 files from D:\SETUP\DATA1.HDR to C:\GAMES\X"]
        );
        let read = |disk: &DiskController, path: &str| {
            disk.file_data(path).unwrap().read().unwrap().into_owned()
        };
        assert_eq!(read(&disk, r"C:\GAMES\X\GAME.EXE"), files[0].data);
        assert_eq!(read(&disk, r"C:\GAMES\X\DATA\MAPS\LEVEL1.MAP"), map);
        assert!(!disk.exists(r"C:\GAMES\X\UNUSED.TXT"));
        assert!(disk.exists(r"C:\GAMES\X\RUSTDOS.INS"));
        // Once the marker is there, the step doesn't run again.
        std::fs::remove_file(base.join("c/GAMES/X/GAME.EXE")).unwrap();
        assert_eq!(run(&mut disk, &steps), Outcome::default());
        assert!(!disk.exists(r"C:\GAMES\X\GAME.EXE"));
    }

    #[test]
    fn a_failed_step_leaves_no_marker() {
        let set = write::cabinet(
            5,
            &[CabinetFile::new("", "A.TXT", b"a")],
            &CabinetOptions::default(),
        );
        let (mut disk, _base) = drives("install-fail", &set);
        let (steps, _) = parse(&lines(
            r"from=D:\SETUP\DATA1.HDR
to=C:\X
group=Nothing",
        ));
        let outcome = run(&mut disk, &steps);
        assert!(outcome.installed.is_empty());
        assert_eq!(
            outcome.warnings,
            [r"install from D:\SETUP\DATA1.HDR: the cabinet has no file group 'Nothing'"]
        );
        assert!(!disk.exists(r"C:\X\RUSTDOS.INS"));
        let (steps, _) = parse(&lines(
            r"from=D:\SETUP\MISSING.HDR
to=C:\X",
        ));
        let outcome = run(&mut disk, &steps);
        assert_eq!(outcome.warnings.len(), 1);
        assert!(
            outcome.warnings[0].starts_with(r"install from D:\SETUP\MISSING.HDR: "),
            "{:?}",
            outcome.warnings
        );
    }

    #[test]
    fn an_installshield_3_archive_unpacks_whole() {
        let archive = write::archive(
            &[
                ArchiveFile::new("", "SETUP.INI", b"[x]"),
                ArchiveFile::new("BIN", "RUN.EXE", &[1u8; 900]),
            ],
            0,
            1,
            1,
        );
        let (mut disk, _base) = drives("install-is3", &[("_SETUP.1".to_string(), archive)]);
        let (steps, _) = parse(&lines(
            r"from=D:\SETUP\_SETUP.1
to=C:\APP",
        ));
        let outcome = run(&mut disk, &steps);
        assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
        let run_exe = disk
            .file_data(r"C:\APP\BIN\RUN.EXE")
            .unwrap()
            .read()
            .unwrap();
        assert_eq!(run_exe.len(), 900);
        assert!(disk.exists(r"C:\APP\SETUP.INI"));
    }

    #[test]
    fn a_reader_reads_and_seeks_within_its_file() {
        let data: Vec<u8> = (0..100u8).collect();
        let read = |pos: u64, buf: &mut [u8]| {
            let from = (pos as usize).min(data.len());
            let n = buf.len().min(data.len() - from);
            buf[..n].copy_from_slice(&data[from..from + n]);
            Ok(n)
        };
        let mut reader = Reader {
            read,
            len: 100,
            pos: 0,
        };
        let mut buf = [0u8; 10];
        assert_eq!(reader.seek(SeekFrom::End(-5)).unwrap(), 95);
        assert_eq!(reader.read(&mut buf).unwrap(), 5);
        assert_eq!(&buf[..5], &[95, 96, 97, 98, 99]);
        assert_eq!(reader.read(&mut buf).unwrap(), 0);
        assert_eq!(reader.seek(SeekFrom::Current(-50)).unwrap(), 50);
        assert!(reader.seek(SeekFrom::Current(-51)).is_err());
    }
}
