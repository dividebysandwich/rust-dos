//! Writing zip archives: a whole archive at once, from entries packed
//! anew or copied as they were from another archive.

use std::io::Write;

use flate2::Compression;
use flate2::write::DeflateEncoder;

use crate::ZipEntry;

/// An entry to write: its data as the archive keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packed {
    /// The name's bytes, '/' between the parts, none at the end.
    pub name: Vec<u8>,
    pub dir: bool,
    /// 0 stored, 8 deflated.
    pub method: u16,
    pub encrypted: bool,
    pub crc: u32,
    pub size: u64,
    /// The data, stored or deflated.
    pub data: Vec<u8>,
    /// The DOS time and date.
    pub time: u16,
    pub date: u16,
}

/// A file's contents packed, deflated unless that makes them no smaller.
pub fn pack(name: &[u8], contents: &[u8], time: u16, date: u16) -> Packed {
    let mut crc = flate2::Crc::new();
    crc.update(contents);
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    let deflated = encoder
        .write_all(contents)
        .and_then(|_| encoder.finish())
        .ok()
        .filter(|d| d.len() < contents.len());
    let (method, data) = match deflated {
        Some(d) => (8, d),
        None => (0, contents.to_vec()),
    };
    Packed {
        name: name.to_vec(),
        dir: false,
        method,
        encrypted: false,
        crc: crc.sum(),
        size: contents.len() as u64,
        data,
        time,
        date,
    }
}

/// A folder.
pub fn folder(name: &[u8], time: u16, date: u16) -> Packed {
    Packed {
        name: name.to_vec(),
        dir: true,
        method: 0,
        encrypted: false,
        crc: 0,
        size: 0,
        data: Vec::new(),
        time,
        date,
    }
}

/// An entry of the archive `archive` as it is there, its data not
/// unpacked, under the name `name`.
pub fn raw(archive: &[u8], entry: &ZipEntry, name: &[u8]) -> Result<Packed, String> {
    let start = entry.data_at as usize;
    let data = start
        .checked_add(entry.compressed as usize)
        .and_then(|end| archive.get(start..end))
        .ok_or_else(|| format!("{}: damaged", entry.path))?;
    Ok(Packed {
        name: name.to_vec(),
        dir: entry.dir,
        method: entry.method,
        encrypted: entry.encrypted,
        crc: entry.crc,
        size: entry.size,
        data: data.to_vec(),
        time: entry.time,
        date: entry.date,
    })
}

/// An archive of `entries`, as DOS zip programs write them: no Zip64, so
/// at most 65535 entries and 4 GB.
pub fn write(entries: &[Packed]) -> Result<Vec<u8>, String> {
    if entries.len() > 0xFFFF {
        return Err("too many files for a ZIP archive".to_string());
    }
    let mut out = Vec::new();
    let mut central = Vec::new();
    for e in entries {
        let offset = u32::try_from(out.len()).map_err(|_| too_big())?;
        let mut name = e.name.clone();
        if e.dir {
            name.push(b'/');
        }
        let size = u32::try_from(e.size).map_err(|_| too_big())?;
        let compressed = u32::try_from(e.data.len()).map_err(|_| too_big())?;
        let name_len = u16::try_from(name.len()).map_err(|_| "a name too long".to_string())?;
        // What the local and the central header have alike, from the
        // version needed on.
        let mut common = Vec::new();
        common.extend(20u16.to_le_bytes());
        common.extend(u16::from(e.encrypted).to_le_bytes());
        common.extend(e.method.to_le_bytes());
        common.extend(e.time.to_le_bytes());
        common.extend(e.date.to_le_bytes());
        common.extend(e.crc.to_le_bytes());
        common.extend(compressed.to_le_bytes());
        common.extend(size.to_le_bytes());
        common.extend(name_len.to_le_bytes());
        common.extend(0u16.to_le_bytes());
        out.extend(0x0403_4B50u32.to_le_bytes());
        out.extend(&common);
        out.extend(&name);
        out.extend(&e.data);
        central.extend(0x0201_4B50u32.to_le_bytes());
        // Made by version 2.0 on DOS.
        central.extend(20u16.to_le_bytes());
        central.extend(&common);
        // Comment, disk, internal attributes.
        central.extend([0; 6]);
        let attributes: u32 = if e.dir { 0x10 } else { 0x20 };
        central.extend(attributes.to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(&name);
    }
    let at = u32::try_from(out.len()).map_err(|_| too_big())?;
    let central_len = u32::try_from(central.len()).map_err(|_| too_big())?;
    out.extend(central);
    out.extend(0x0605_4B50u32.to_le_bytes());
    out.extend([0; 4]);
    let count = entries.len() as u16;
    out.extend(count.to_le_bytes());
    out.extend(count.to_le_bytes());
    out.extend(central_len.to_le_bytes());
    out.extend(at.to_le_bytes());
    out.extend([0, 0]);
    Ok(out)
}

fn too_big() -> String {
    "too big for a ZIP archive".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{central_directory, read};
    use std::io::Cursor;

    #[test]
    fn writes_what_it_reads_back() {
        let text = b"some text, some text, some text, some text".repeat(20);
        let entries = [
            folder(b"SUB", 0x6000, 0x2A21),
            pack(b"SUB/A.TXT", &text, 0x6000, 0x2A21),
            pack(b"B.BIN", &[1, 2, 3], 0x1234, 0x5678),
        ];
        let data = write(&entries).unwrap();
        assert_eq!(entries[1].method, 8);
        assert_eq!(entries[2].method, 0);
        let mut file = Cursor::new(data.clone());
        let read_back = central_directory(&mut file).unwrap();
        let names: Vec<(String, bool)> = read_back.iter().map(|e| (e.name(), e.is_dir())).collect();
        assert_eq!(
            names,
            [
                ("SUB".into(), true),
                ("SUB/A.TXT".into(), false),
                ("B.BIN".into(), false)
            ]
        );
        assert_eq!(read(&mut file, &read_back[1]).unwrap(), text);
        assert_eq!((read_back[2].time, read_back[2].date), (0x1234, 0x5678));
        // Copied as it is, renamed, into another archive.
        let copied = raw(&data, &read_back[1], b"C.TXT").unwrap();
        let again = write(&[copied]).unwrap();
        let mut file = Cursor::new(again);
        let entries = central_directory(&mut file).unwrap();
        assert_eq!(entries[0].name(), "C.TXT");
        assert_eq!(read(&mut file, &entries[0]).unwrap(), text);
        // An empty archive is one.
        assert!(
            central_directory(&mut Cursor::new(write(&[]).unwrap()))
                .unwrap()
                .is_empty()
        );
    }
}
