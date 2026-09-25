//! Zip archives, as DOS games are often passed around: read with their
//! central directory, stored or deflated, and unpacked into a folder. Zip64
//! and encrypted archives aren't read.

use std::fs;
use std::path::{Component, Path, PathBuf};

/// A file or directory in an archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipEntry {
    /// Its path in the archive, with '/' between the parts.
    pub name: String,
    pub is_dir: bool,
    method: u16,
    compressed: usize,
    size: usize,
    /// Where its local header is.
    offset: usize,
    /// Its DOS time and date.
    pub time: u16,
    pub date: u16,
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    data.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The entries of the archive `data`.
pub fn entries(data: &[u8]) -> Result<Vec<ZipEntry>, String> {
    // The end of central directory record: its signature, in the last
    // 64 KB and 22 bytes (it can have a comment of up to 64 KB).
    let bad = || "not a zip archive".to_string();
    let from = data.len().saturating_sub(22 + 0xFFFF);
    let end = (from..=data.len().saturating_sub(22)).rev().find(|&i| u32_at(data, i) == Some(0x0605_4B50)).ok_or_else(bad)?;
    let count = u16_at(data, end + 10).ok_or_else(bad)? as usize;
    let mut at = u32_at(data, end + 16).ok_or_else(bad)? as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        if u32_at(data, at) != Some(0x0201_4B50) {
            return Err("a damaged zip archive".to_string());
        }
        let field = |offset: usize| u16_at(data, at + offset).ok_or_else(bad);
        let flags = field(8)?;
        if flags & 0x0001 != 0 {
            return Err("encrypted zip archives can't be read".to_string());
        }
        let (name_len, extra_len, comment_len) = (field(28)? as usize, field(30)? as usize, field(32)? as usize);
        let raw_name = data.get(at + 46..at + 46 + name_len).ok_or_else(bad)?;
        // Names are code page 437 unless flagged as UTF-8.
        let name = if flags & 0x0800 != 0 {
            String::from_utf8_lossy(raw_name).into_owned()
        } else {
            raw_name.iter().map(|&b| crate::video::CP437[b as usize]).collect()
        };
        let name = name.replace('\\', "/");
        let compressed = u32_at(data, at + 20).ok_or_else(bad)?;
        let size = u32_at(data, at + 24).ok_or_else(bad)?;
        let offset = u32_at(data, at + 42).ok_or_else(bad)?;
        if [compressed, size, offset].contains(&0xFFFF_FFFF) {
            return Err("zip64 archives can't be read".to_string());
        }
        entries.push(ZipEntry {
            is_dir: name.ends_with('/'),
            name,
            method: field(10)?,
            compressed: compressed as usize,
            size: size as usize,
            offset: offset as usize,
            time: field(12)?,
            date: field(14)?,
        });
        at += 46 + name_len + extra_len + comment_len;
    }
    Ok(entries)
}

/// The contents of a file in the archive `data`.
pub fn read(data: &[u8], entry: &ZipEntry) -> Result<Vec<u8>, String> {
    let bad = || format!("{}: damaged", entry.name);
    if u32_at(data, entry.offset) != Some(0x0403_4B50) {
        return Err(bad());
    }
    let start = entry.offset + 30 + u16_at(data, entry.offset + 26).ok_or_else(bad)? as usize + u16_at(data, entry.offset + 28).ok_or_else(bad)? as usize;
    let stored = data.get(start..start + entry.compressed).ok_or_else(bad)?;
    match entry.method {
        0 => Ok(stored.to_vec()),
        8 => {
            let mut out = Vec::with_capacity(entry.size);
            let mut inflate = flate2::Decompress::new(false);
            inflate.decompress_vec(stored, &mut out, flate2::FlushDecompress::Finish).map_err(|e| format!("{}: {}", entry.name, e))?;
            Ok(out)
        }
        method => Err(format!("{}: compression method {} isn't supported", entry.name, method)),
    }
}

/// The part of an archive's paths all of its entries are in, if they are
/// all in one folder (archives often hold the game's folder).
fn common_folder(entries: &[ZipEntry]) -> Option<String> {
    let first = entries.first()?.name.split('/').next()?.to_string();
    let all_in_it = entries.iter().all(|e| e.name.split_once('/').is_some_and(|(top, _)| top == first));
    all_in_it.then_some(first)
}

/// A safe path in `dest` for an entry's name: none that climbs out of it.
fn target(dest: &Path, name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(dest.join(path))
}

/// Unpack the archive `data` into `dest`, leaving out a folder that holds
/// all of it. Returns the files written, relative to `dest`.
pub fn extract(data: &[u8], dest: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = entries(data)?;
    let strip = common_folder(&entries).map(|f| format!("{}/", f));
    let mut written = Vec::new();
    for entry in &entries {
        let name = strip.as_deref().and_then(|s| entry.name.strip_prefix(s)).unwrap_or(&entry.name);
        if name.is_empty() {
            continue;
        }
        let Some(path) = target(dest, name.trim_end_matches('/')) else { continue };
        if entry.is_dir {
            fs::create_dir_all(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
            continue;
        }
        let contents = read(data, entry)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {}", parent.display(), e))?;
        }
        fs::write(&path, contents).map_err(|e| format!("{}: {}", path.display(), e))?;
        if let Some(when) = crate::disk::dos_to_system_time(entry.time, entry.date) {
            let _ = fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(when));
        }
        written.push(PathBuf::from(name));
    }
    Ok(written)
}

/// The program that starts the game among the files unpacked: the one
/// program or batch file in the folder's top, leaving out those that set
/// the game up or install it. None if there isn't exactly one.
pub fn start_program(files: &[PathBuf]) -> Option<String> {
    const NOT_GAMES: &[&str] = &["SETUP", "INSTALL", "INSTALLER", "CONFIG", "SETSOUND", "SOUND", "SETMAIN", "UNINST", "UNINSTALL", "README", "CATALOG"];
    let programs: Vec<String> = files
        .iter()
        .filter(|f| f.components().count() == 1)
        .filter_map(|f| f.to_str())
        .filter(|name| {
            let upper = name.to_ascii_uppercase();
            let (stem, ext) = upper.rsplit_once('.').unwrap_or((&upper, ""));
            matches!(ext, "EXE" | "COM" | "BAT") && !NOT_GAMES.contains(&stem)
        })
        .map(str::to_string)
        .collect();
    match programs.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::DeflateEncoder;
    use std::io::Write;

    /// A zip archive of `files` (name, contents, deflated), written the way
    /// zip programs write them.
    pub fn zip(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let (mut data, mut central) = (Vec::new(), Vec::new());
        for &(name, contents, deflate) in files {
            let stored = if deflate {
                let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
                encoder.write_all(contents).unwrap();
                encoder.finish().unwrap()
            } else {
                contents.to_vec()
            };
            let method: u16 = if deflate { 8 } else { 0 };
            let offset = data.len() as u32;
            let header = |sig: u32| {
                let mut h = sig.to_le_bytes().to_vec();
                h.extend([20, 0, 0, 0]);
                h.extend(method.to_le_bytes());
                h.extend([0x00, 0x60, 0x21, 0x2A]); // 12:00, 1st January 2001
                h.extend([0; 4]); // CRC, which isn't checked
                h.extend((stored.len() as u32).to_le_bytes());
                h.extend((contents.len() as u32).to_le_bytes());
                h.extend((name.len() as u16).to_le_bytes());
                h.extend([0, 0]);
                h
            };
            data.extend(header(0x0403_4B50));
            data.extend(name.as_bytes());
            data.extend(&stored);
            let mut entry = vec![0x50, 0x4B, 0x01, 0x02, 20, 0];
            entry.extend(&header(0)[4..]);
            entry.extend([0; 10]); // comment length, disk, attributes
            entry.extend(offset.to_le_bytes());
            entry.extend(name.as_bytes());
            central.extend(entry);
        }
        let at = data.len() as u32;
        data.extend(&central);
        data.extend([0x50, 0x4B, 0x05, 0x06, 0, 0, 0, 0]);
        data.extend((files.len() as u16).to_le_bytes());
        data.extend((files.len() as u16).to_le_bytes());
        data.extend((central.len() as u32).to_le_bytes());
        data.extend(at.to_le_bytes());
        data.extend([0, 0]);
        data
    }

    #[test]
    fn archives_unpack_without_their_common_folder() {
        let data = zip(&[
            ("KEEN/", b"", false),
            ("KEEN/KEEN4E.EXE", b"MZ the game", true),
            ("KEEN/SETUP.EXE", b"MZ setup", false),
            ("KEEN/DATA/LEVEL1.DAT", &[7u8; 5000], true),
            ("KEEN/../../evil", b"no", false),
        ]);
        let dir = PathBuf::from("target/test_zip");
        let _ = fs::remove_dir_all(&dir);
        let files = extract(&data, &dir).unwrap();
        assert_eq!(fs::read(dir.join("KEEN4E.EXE")).unwrap(), b"MZ the game");
        assert_eq!(fs::read(dir.join("DATA/LEVEL1.DAT")).unwrap(), vec![7u8; 5000]);
        assert!(!dir.join("../evil").exists() && !PathBuf::from("target/evil").exists());
        assert_eq!(start_program(&files).as_deref(), Some("KEEN4E.EXE"), "not SETUP");
        assert!(entries(b"not a zip").is_err());
    }
}
