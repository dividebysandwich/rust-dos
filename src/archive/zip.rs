//! Zip archives, read from their central directory without loading the
//! whole archive: Zip64 included, stored or deflated files. Encrypted
//! files can't be read.

use std::io::{Read, Seek, SeekFrom};

/// A file or folder in a zip archive's central directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipEntry {
    /// Its name as the archive has it: code page 437, or UTF-8 when
    /// `flags` says so (`name` decodes it).
    pub raw_name: Vec<u8>,
    pub flags: u16,
    pub method: u16,
    pub crc: u32,
    pub compressed: u64,
    pub size: u64,
    /// Where its local header is.
    pub local_at: u64,
    /// Its DOS time and date.
    pub time: u16,
    pub date: u16,
    /// The low word of its external attributes: 10h for a folder.
    pub external: u16,
}

impl ZipEntry {
    pub fn is_dir(&self) -> bool {
        matches!(self.raw_name.last(), None | Some(b'/' | b'\\')) || self.external & 0x10 != 0
    }

    pub fn encrypted(&self) -> bool {
        self.flags & 0x0001 != 0
    }

    /// Its path, with '/' between the parts and none at the end.
    pub fn name(&self) -> String {
        let name = if self.flags & 0x0800 != 0 {
            String::from_utf8_lossy(&self.raw_name).into_owned()
        } else {
            self.raw_name.iter().map(|&b| crate::video::CP437[b as usize]).collect()
        };
        name.replace('\\', "/").trim_end_matches('/').to_string()
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

fn read_at<F: Read + Seek>(file: &mut F, at: u64, len: usize) -> Result<Vec<u8>, String> {
    let mut buf = vec![0; len];
    file.seek(SeekFrom::Start(at))
        .and_then(|_| file.read_exact(&mut buf))
        .map_err(|_| "a ZIP read error".to_string())?;
    Ok(buf)
}

/// The files and folders of the archive's central directory.
pub fn central_directory<F: Read + Seek>(file: &mut F) -> Result<Vec<ZipEntry>, String> {
    let archive_size = file.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    if archive_size < 22 {
        return Err("the ZIP is too small".to_string());
    }
    // The end of central directory record, searched for from the end.
    let tail_len = archive_size.min(0xFFFF + 22 + 2048);
    let tail = read_at(file, archive_size - tail_len, tail_len as usize)?;
    let eocd = (0..=tail.len() - 4)
        .rev()
        .find(|&i| le32(&tail[i..]) == 0x0605_4b50 && i + 22 <= tail.len())
        .ok_or("no ZIP central directory")?;
    let eocd_at = archive_size - tail_len + eocd as u64;
    let record = &tail[eocd..eocd + 22];
    let (mut total, mut cdir_size, mut cdir_at) = (le16(&record[0x0A..]), le32(&record[0x0C..]), le32(&record[0x10..]));
    if (cdir_at == 0xFFFF_FFFF || cdir_size == 0xFFFF_FFFF || total == 0xFFFF) && eocd_at >= 20 + 56 {
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
        let flags = le16(&h[0x08..]) as u16;
        let method = le16(&h[0x0A..]) as u16;
        let (time, date) = (le16(&h[0x0C..]) as u16, le16(&h[0x0E..]) as u16);
        let crc = le32(&h[0x10..]) as u32;
        let mut compressed = le32(&h[0x14..]);
        let mut size = le32(&h[0x18..]);
        let name_len = le16(&h[0x1C..]) as usize;
        let extra_len = le16(&h[0x1E..]) as usize;
        let comment_len = le16(&h[0x20..]) as usize;
        let external = le16(&h[0x26..]) as u16;
        let mut local_at = le32(&h[0x2A..]);
        if at + 46 + name_len + extra_len > cdir.len() {
            return Err("the ZIP's central directory is invalid".to_string());
        }
        let raw_name = h[46..46 + name_len].to_vec();
        let extra = &h[46 + name_len..46 + name_len + extra_len];
        at += 46 + name_len + extra_len + comment_len;
        if size == 0xFFFF_FFFF || compressed == 0xFFFF_FFFF || local_at == 0xFFFF_FFFF {
            let mut x = extra;
            while x.len() > 4 {
                let (id, len) = (le16(x), le16(&x[2..]) as usize);
                let Some(field) = x.get(4..4 + len) else {
                    break;
                };
                if id == 0x0001 {
                    let mut f = field;
                    for value in [&mut size, &mut compressed, &mut local_at] {
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
        let entry = ZipEntry { raw_name, flags, method, crc, compressed, size, local_at, time, date, external };
        if !entry.is_dir()
            && ((method == 0 && size != compressed) || (size != 0 && compressed == 0) || local_at + 30 + compressed > archive_size)
        {
            return Err("an invalid entry in the ZIP's central directory".to_string());
        }
        entries.push(entry);
    }
    Ok(entries)
}

/// Where the file's data starts, after its local header.
pub fn data_at<F: Read + Seek>(file: &mut F, entry: &ZipEntry) -> Result<u64, String> {
    let bad = || format!("{}: damaged", entry.name());
    let header = read_at(file, entry.local_at, 30).map_err(|_| bad())?;
    if le32(&header) != 0x0403_4b50 {
        return Err(bad());
    }
    Ok(entry.local_at + 30 + le16(&header[26..]) + le16(&header[28..]))
}

/// The contents of a file in the archive.
pub fn read<F: Read + Seek>(file: &mut F, entry: &ZipEntry) -> Result<Vec<u8>, String> {
    if entry.encrypted() {
        return Err(format!("{}: encrypted files can't be read", entry.name()));
    }
    let start = data_at(file, entry)?;
    let stored = read_at(file, start, entry.compressed as usize).map_err(|_| format!("{}: damaged", entry.name()))?;
    match entry.method {
        0 => Ok(stored),
        8 => {
            let mut out = Vec::with_capacity(entry.size as usize);
            flate2::read::DeflateDecoder::new(stored.as_slice())
                .read_to_end(&mut out)
                .map_err(|e| format!("{}: {}", entry.name(), e))?;
            Ok(out)
        }
        method => Err(format!("{}: compression method {} isn't supported", entry.name(), method)),
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
    fn stored_and_deflated_files_are_read() {
        let data = zip(&[("GAME/", b"", false), ("GAME/A.TXT", b"stored", false), ("GAME/B.TXT", &[7; 5000], true)]);
        let mut file = std::io::Cursor::new(data);
        let entries = central_directory(&mut file).unwrap();
        let names: Vec<(String, bool)> = entries.iter().map(|e| (e.name(), e.is_dir())).collect();
        assert_eq!(names, [("GAME".into(), true), ("GAME/A.TXT".into(), false), ("GAME/B.TXT".into(), false)]);
        assert_eq!(read(&mut file, &entries[1]).unwrap(), b"stored");
        assert_eq!(read(&mut file, &entries[2]).unwrap(), vec![7; 5000]);
        assert_eq!((entries[1].time, entries[1].date), (0x6000, 0x2A21));
    }
}
