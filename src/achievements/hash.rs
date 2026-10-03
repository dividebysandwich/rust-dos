//! A game's hash, which RetroAchievements knows it by. MS-DOS games are
//! zip archives (.zip or .dosz), hashed as rcheevos does:
//! the MD5 of each file's name (in lower case, with forward slashes), CRC
//! and size from the central directory, sorted, so repacking the same
//! files keeps the hash. A .dosz may name a parent archive it goes over
//! (`<parent>.parent`, hashed first), and have a .dosc beside it, hashed
//! after.

use crate::archive::zip::{ZipEntry, central_directory};
use crate::hostfs::{self, File};
use md5::{Digest, Md5};
use std::path::{Path, PathBuf};

/// The hash of the archive at `path`, in lower-case hex.
pub fn hash_archive(path: &Path) -> Result<String, String> {
    let mut md5 = Md5::new();
    hash_dosz(path, &mut md5, &mut Vec::new())?;
    Ok(hex(&md5.finalize()))
}

/// Whether `text` is a hash as the site has them: 32 hex digits.
pub fn is_hash(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// `path` and its parents, the parents first; `children` are the archives
/// that went over it, which it can't have as a parent.
fn hash_dosz(path: &Path, md5: &mut Md5, children: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut file = File::open(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    let entries = files(&mut file).map_err(|e| format!("{}: {}", path.display(), e))?;
    let mut records = Vec::new();
    let mut parent = None;
    for entry in entries {
        // A .dosz's empty <name>.parent in its root names the archive it
        // goes over; it isn't hashed, so both can be renamed.
        let name = &entry.raw_name;
        let is_parent_marker = entry.size == 0
            && name.len() > 7
            && name[name.len() - 7..].eq_ignore_ascii_case(b".parent")
            && !name.contains(&b'/')
            && !name.contains(&b'\\');
        if is_parent_marker {
            if parent.is_some() {
                return Err(format!("{}: more than one parent archive", path.display()));
            }
            let name = String::from_utf8_lossy(&name[..name.len() - 7]).into_owned();
            parent = Some(path.with_file_name(name));
            continue;
        }
        records.push(record(&entry));
    }
    if let Some(parent) = parent {
        children.push(path.to_path_buf());
        if children.contains(&parent) {
            return Err(format!(
                "{}: the parent archives go round in a circle",
                path.display()
            ));
        }
        if !hostfs::is_file(&parent) {
            return Err(format!(
                "the parent archive {} isn't there",
                parent.display()
            ));
        }
        hash_dosz(&parent, md5, children)?;
    }
    hash_records(md5, records);
    // A .dosc beside a .dosz is hashed after it.
    let name = path.to_string_lossy();
    if let Some(last) = name.chars().last().filter(|c| c.eq_ignore_ascii_case(&'z')) {
        let dosc = PathBuf::from(format!(
            "{}{}",
            &name[..name.len() - 1],
            if last == 'z' { 'c' } else { 'C' }
        ));
        if let Ok(mut file) = File::open(&dosc) {
            let entries = files(&mut file).map_err(|e| format!("{}: {}", dosc.display(), e))?;
            hash_records(md5, entries.iter().map(record).collect());
        }
    }
    Ok(())
}

fn hash_records(md5: &mut Md5, mut records: Vec<Vec<u8>>) {
    // As rcheevos sorts them: by their bytes, as far as the shorter goes.
    records.sort_by(|a, b| {
        let n = a.len().min(b.len());
        a[..n].cmp(&b[..n])
    });
    for record in records {
        md5.update(&record);
    }
}

/// The files (not folders) of the archive's central directory.
fn files(file: &mut File) -> Result<Vec<ZipEntry>, String> {
    Ok(central_directory(file)?.into_iter().filter(|e| !e.is_dir()).collect())
}

/// What is hashed of a file: its name in lower case with forward slashes,
/// a zero, its CRC and size.
fn record(entry: &ZipEntry) -> Vec<u8> {
    let mut record: Vec<u8> =
        entry.raw_name.iter().map(|&b| if b == b'\\' { b'/' } else { b.to_ascii_lowercase() }).collect();
    record.push(0);
    record.extend_from_slice(&entry.crc.to_le_bytes());
    record.extend_from_slice(&entry.size.to_le_bytes());
    record
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stored zip of `files`, as `archive::zip`'s tests make them.
    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        crate::archive::zip::tests::zip(
            &files
                .iter()
                .map(|(n, d)| (*n, *d, false))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn archives_hash_by_their_files_whatever_their_order_and_case() {
        let dir = PathBuf::from("target/test_achievement_hash");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.zip");
        let b = dir.join("b.zip");
        std::fs::write(
            &a,
            zip(&[("GAME.EXE", b"MZ game"), ("DATA\\LEVEL1.DAT", b"level")]),
        )
        .unwrap();
        std::fs::write(
            &b,
            zip(&[("data/level1.dat", b"level"), ("game.exe", b"MZ game")]),
        )
        .unwrap();
        let hash = hash_archive(&a).unwrap();
        assert!(is_hash(&hash));
        assert_eq!(hash_archive(&b).unwrap(), hash);
        // The MD5 of the sorted records, worked out by hand (the test
        // archives' CRCs are zero).
        let record = |name: &str, data: &[u8]| {
            let mut r = name.as_bytes().to_vec();
            r.push(0);
            r.extend_from_slice(&0u32.to_le_bytes());
            r.extend_from_slice(&(data.len() as u64).to_le_bytes());
            r
        };
        let mut md5 = Md5::new();
        md5.update(record("data/level1.dat", b"level"));
        md5.update(record("game.exe", b"MZ game"));
        assert_eq!(hash, hex(&md5.finalize()));

        // A .dosz over a parent, with a .dosc beside it.
        std::fs::write(dir.join("base.dosz"), zip(&[("GAME.EXE", b"MZ game")])).unwrap();
        std::fs::write(
            dir.join("mod.dosz"),
            zip(&[("base.dosz.parent", b""), ("MOD.DAT", b"mod")]),
        )
        .unwrap();
        let alone = hash_archive(&dir.join("mod.dosz")).unwrap();
        std::fs::write(dir.join("mod.dosc"), zip(&[("SAVE.DAT", b"s")])).unwrap();
        assert_ne!(hash_archive(&dir.join("mod.dosz")).unwrap(), alone);
        let mut md5 = Md5::new();
        md5.update(record("game.exe", b"MZ game"));
        md5.update(record("mod.dat", b"mod"));
        assert_eq!(alone, hex(&md5.finalize()));
        assert!(hash_archive(&dir.join("missing.zip")).is_err());
        std::fs::write(dir.join("junk.zip"), b"not a zip at all, not at all").unwrap();
        assert!(hash_archive(&dir.join("junk.zip")).is_err());
        assert!(!is_hash("xyz") && is_hash("0123456789abcdef0123456789ABCDEF"));
    }
}
