//! Zip archives: their directory as the `zip` crate reads it (Zip64
//! included), and their stored or deflated files, read where they are.
//! Encrypted files can't be read.

use std::io::{Read, Seek, SeekFrom};

/// A file or folder in a zip archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipEntry {
    /// Its name's bytes as the archive has them, which achievement hashes
    /// are made of.
    pub raw_name: Vec<u8>,
    /// Its path, with '/' between the parts and none at the end.
    pub path: String,
    pub dir: bool,
    pub encrypted: bool,
    /// 0 stored, 8 deflated.
    pub method: u16,
    pub crc: u32,
    pub compressed: u64,
    pub size: u64,
    /// Where its data starts, after its local header.
    pub data_at: u64,
    /// Its DOS time and date.
    pub time: u16,
    pub date: u16,
}

impl ZipEntry {
    pub fn is_dir(&self) -> bool {
        self.dir
    }

    pub fn encrypted(&self) -> bool {
        self.encrypted
    }

    pub fn name(&self) -> String {
        self.path.clone()
    }
}

/// A folder's file type in a Unix mode.
const FILE_TYPE: u32 = 0o170000;
const FOLDER: u32 = 0o040000;

/// Whether an entry is a folder, by its central directory header: DOS's
/// folder attribute, or the Unix mode in the attributes' high word when the
/// archive was made on Unix. DOS zip programs left junk in that high word,
/// which the crate's `unix_mode` takes for a mode (1rap12.zip's INSTALL.EXE
/// has 4978h there: a folder's type).
fn folder_header(header: &[u8; 46]) -> bool {
    let external = u32::from_le_bytes([header[38], header[39], header[40], header[41]]);
    // Made by: 3 Unix, 19 OS X.
    let unix = matches!(header[5], 3 | 19);
    external & 0x10 != 0 || (unix && (external >> 16) & FILE_TYPE == FOLDER)
}

/// The files and folders of the archive.
pub fn central_directory<F: Read + Seek>(file: &mut F) -> Result<Vec<ZipEntry>, String> {
    let mut headers = Vec::new();
    let mut entries = {
        let mut archive = ::zip::ZipArchive::new(&mut *file).map_err(|e| format!("not a readable ZIP: {}", e))?;
        (0..archive.len())
            .map(|i| {
                let file = archive.by_index_raw(i).map_err(|e| format!("a damaged ZIP entry: {}", e))?;
                let raw_name = file.name_raw().to_vec();
                let path = file.name().replace('\\', "/").trim_end_matches('/').to_string();
                let dir = file.is_dir() || raw_name.last() == Some(&b'\\');
                headers.push(file.central_header_start());
                let (date, time) = file.last_modified().map_or((0, 0), |t| (t.datepart(), t.timepart()));
                // The method's number, whichever of the crate's own features
                // are on: flate2 here inflates what it can't.
                #[allow(deprecated)]
                let method = file.compression().to_u16();
                Ok(ZipEntry {
                    path,
                    dir,
                    encrypted: file.encrypted(),
                    method,
                    crc: file.crc32(),
                    compressed: file.compressed_size(),
                    size: file.size(),
                    data_at: file.data_start().ok_or_else(|| format!("{}: damaged", file.name()))?,
                    time,
                    date,
                    raw_name,
                })
            })
            .collect::<Result<Vec<_>, String>>()?
    };
    for (entry, at) in entries.iter_mut().zip(headers) {
        let mut header = [0; 46];
        file.seek(SeekFrom::Start(at))
            .and_then(|_| file.read_exact(&mut header))
            .map_err(|_| format!("{}: damaged", entry.path))?;
        entry.dir |= folder_header(&header);
    }
    Ok(entries)
}

/// The contents of a file in the archive.
pub fn read<F: Read + Seek>(file: &mut F, entry: &ZipEntry) -> Result<Vec<u8>, String> {
    if entry.encrypted {
        return Err(format!("{}: encrypted files can't be read", entry.path));
    }
    let mut packed = vec![0; entry.compressed as usize];
    file.seek(SeekFrom::Start(entry.data_at))
        .and_then(|_| file.read_exact(&mut packed))
        .map_err(|_| format!("{}: damaged", entry.path))?;
    match entry.method {
        0 => Ok(packed),
        8 => {
            let mut out = Vec::with_capacity(entry.size as usize);
            flate2::read::DeflateDecoder::new(packed.as_slice())
                .read_to_end(&mut out)
                .map_err(|e| format!("{}: {}", entry.path, e))?;
            Ok(out)
        }
        method => Err(format!("{}: compression method {} isn't supported", entry.path, method)),
    }
}

#[cfg(any(test, feature = "test-util"))]
pub mod tests {
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
    fn junk_in_the_attributes_is_no_folder() {
        let mut data = zip(&[("INSTALL.EXE", b"MZ", false), ("SUB/", b"", false), ("WOLF.DAT", b"", false)]);
        // The central directory's external attributes: DOS's archive bit,
        // with junk over it (keen1.zip's D440h, 1rap12.zip's 4978h, which
        // looks like a Unix folder), and the folder bit alone.
        let central = data.windows(4).position(|w| w == [0x50, 0x4B, 0x01, 0x02]).unwrap();
        data[central + 38..central + 42].copy_from_slice(&0x00D4_4020u32.to_le_bytes());
        let second = central + 46 + "INSTALL.EXE".len();
        data[second + 38..second + 42].copy_from_slice(&0x10u32.to_le_bytes());
        let third = second + 46 + "SUB/".len();
        data[third + 38..third + 42].copy_from_slice(&0x4978_C920u32.to_le_bytes());
        let entries = central_directory(&mut std::io::Cursor::new(data)).unwrap();
        assert!(!entries[0].is_dir());
        assert!(entries[1].is_dir());
        assert!(!entries[2].is_dir());
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
