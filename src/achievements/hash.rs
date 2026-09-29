//! A game's hash, which RetroAchievements knows it by. MS-DOS games are
//! zip archives (DOSBox Pure's .zip or .dosz), hashed as rcheevos does:
//! the MD5 of each file's name (in lower case, with forward slashes), CRC
//! and size from the central directory, sorted, so repacking the same
//! files keeps the hash. A .dosz may name a parent archive it goes over
//! (`<parent>.parent`, hashed first), and have a .dosc beside it, hashed
//! after.

use md5::{Digest, Md5};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
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
    let entries = central_directory(&mut file).map_err(|e| format!("{}: {}", path.display(), e))?;
    let mut records = Vec::new();
    let mut parent = None;
    for entry in entries {
        // A .dosz's empty <name>.parent in its root names the archive it
        // goes over; it isn't hashed, so both can be renamed.
        let is_parent_marker = entry.size == 0
            && entry.name.len() > 7
            && entry.name[entry.name.len() - 7..].eq_ignore_ascii_case(b".parent")
            && !entry.name.contains(&b'/')
            && !entry.name.contains(&b'\\');
        if is_parent_marker {
            if parent.is_some() {
                return Err(format!("{}: more than one parent archive", path.display()));
            }
            let name = String::from_utf8_lossy(&entry.name[..entry.name.len() - 7]).into_owned();
            parent = Some(path.with_file_name(name));
            continue;
        }
        records.push(entry.record());
    }
    if let Some(parent) = parent {
        children.push(path.to_path_buf());
        if children.contains(&parent) {
            return Err(format!(
                "{}: the parent archives go round in a circle",
                path.display()
            ));
        }
        if !parent.is_file() {
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
            let entries =
                central_directory(&mut file).map_err(|e| format!("{}: {}", dosc.display(), e))?;
            hash_records(md5, entries.into_iter().map(|e| e.record()).collect());
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

/// A file in an archive's central directory.
struct Entry {
    name: Vec<u8>,
    crc: u32,
    size: u64,
}

impl Entry {
    /// What is hashed of it: its name in lower case with forward slashes,
    /// a zero, its CRC and size.
    fn record(&self) -> Vec<u8> {
        let mut record: Vec<u8> = self
            .name
            .iter()
            .map(|&b| {
                if b == b'\\' {
                    b'/'
                } else {
                    b.to_ascii_lowercase()
                }
            })
            .collect();
        record.push(0);
        record.extend_from_slice(&self.crc.to_le_bytes());
        record.extend_from_slice(&self.size.to_le_bytes());
        record
    }
}

fn le16(b: &[u8]) -> u64 {
    u16::from_le_bytes([b[0], b[1]]) as u64
}

fn le32(b: &[u8]) -> u64 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64
}

fn le64(b: &[u8]) -> u64 {
    u64::from_le_bytes(b[..8].try_into().unwrap())
}

/// The files (not directories) of the archive's central directory.
fn central_directory<F: Read + Seek>(file: &mut F) -> Result<Vec<Entry>, String> {
    let archive_size = file.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    if archive_size < 22 {
        return Err("the ZIP is too small".to_string());
    }
    let read_at = |file: &mut F, at: u64, len: usize| -> Result<Vec<u8>, String> {
        let mut buf = vec![0; len];
        file.seek(SeekFrom::Start(at))
            .and_then(|_| file.read_exact(&mut buf))
            .map_err(|_| "a ZIP read error".to_string())?;
        Ok(buf)
    };
    // The end of central directory record, searched for from the end.
    let tail_len = archive_size.min(0xFFFF + 22 + 2048);
    let tail = read_at(file, archive_size - tail_len, tail_len as usize)?;
    let eocd = (0..=tail.len() - 4)
        .rev()
        .find(|&i| le32(&tail[i..]) == 0x0605_4b50 && i + 22 <= tail.len())
        .ok_or("no ZIP central directory")?;
    let eocd_at = archive_size - tail_len + eocd as u64;
    let record = &tail[eocd..eocd + 22];
    let (mut total, mut cdir_size, mut cdir_at) = (
        le16(&record[0x0A..]),
        le32(&record[0x0C..]),
        le32(&record[0x10..]),
    );
    if (cdir_at == 0xFFFF_FFFF || cdir_size == 0xFFFF_FFFF || total == 0xFFFF) && eocd_at >= 20 + 56
    {
        // Zip64: its locator, then its own end record.
        let locator = read_at(file, eocd_at - 20, 20)?;
        if le32(&locator) == 0x0706_4b50 {
            let at = le64(&locator[8..]);
            if at <= archive_size - 56 {
                let record = read_at(file, at, 56)?;
                if le32(&record) == 0x0606_4b50 {
                    total = le64(&record[0x20..]);
                    cdir_size = le64(&record[0x28..]);
                    cdir_at = le64(&record[0x30..]);
                }
            }
        }
    }
    if cdir_size >= 0x1000_0000 || cdir_size < total * 46 || cdir_at + cdir_size > archive_size {
        return Err("the ZIP's central directory is invalid".to_string());
    }
    let cdir = read_at(file, cdir_at, cdir_size as usize)?;
    let mut entries = Vec::new();
    let mut at = 0usize;
    for _ in 0..total {
        if at + 46 > cdir.len() {
            break;
        }
        let h = &cdir[at..];
        if le32(h) != 0x0201_4b50 {
            break;
        }
        let method = le16(&h[0x0A..]);
        let crc = le32(&h[0x10..]) as u32;
        let mut comp_size = le32(&h[0x14..]);
        let mut size = le32(&h[0x18..]);
        let name_len = le16(&h[0x1C..]) as usize;
        let extra_len = le16(&h[0x1E..]) as usize;
        let comment_len = le16(&h[0x20..]) as usize;
        let external = le16(&h[0x26..]);
        let mut local_at = le32(&h[0x2A..]);
        let entry_len = 46 + name_len + extra_len + comment_len;
        if at + 46 + name_len + extra_len > cdir.len() {
            return Err("the ZIP's central directory is invalid".to_string());
        }
        let name = h[46..46 + name_len].to_vec();
        at += entry_len;
        // Directories aren't hashed.
        if name_len == 0 || matches!(name[name_len - 1], b'/' | b'\\') || external & 0x10 != 0 {
            continue;
        }
        if size == 0xFFFF_FFFF || comp_size == 0xFFFF_FFFF || local_at == 0xFFFF_FFFF {
            let mut x = &h[46 + name_len..46 + name_len + extra_len];
            while x.len() > 4 {
                let (id, len) = (le16(x), le16(&x[2..]) as usize);
                let Some(field) = x.get(4..4 + len) else {
                    break;
                };
                if id == 0x0001 {
                    let mut f = field;
                    for value in [&mut size, &mut comp_size, &mut local_at] {
                        if *value == 0xFFFF_FFFF {
                            if f.len() < 8 {
                                return Err("an invalid Zip64 file".to_string());
                            }
                            *value = le64(f);
                            f = &f[8..];
                        }
                    }
                    break;
                }
                x = &x[4 + len..];
            }
        }
        if (method == 0 && size != comp_size)
            || (size != 0 && comp_size == 0)
            || local_at + 30 + comp_size > archive_size
        {
            return Err("an invalid entry in the ZIP's central directory".to_string());
        }
        entries.push(Entry { name, crc, size });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stored zip of `files`, as `import::zip`'s tests make them.
    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        crate::import::zip::tests::zip(
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
