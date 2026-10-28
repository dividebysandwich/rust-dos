//! The ISO 9660 file system of a CD's data track, read into a directory
//! tree of DOS names, as MSCDEX shows it to programs, with the long names
//! of the disc's Joliet tree, which Windows 95 and later show.
//!
//! A Joliet disc has two directory trees over the same files: the primary
//! one, whose names DOS reads, and one with names of up to 64 UCS-2
//! characters, which Windows reads instead. The drive's tree is the
//! primary one, so that DOS programs see the names they see under MSCDEX.
//! Each of its entries that Joliet has too gets the Joliet name as its
//! long name, for Windows programs, and what only Joliet has is added with
//! a DOS name made from the long one.

use super::image::CdImage;
use super::{Extent, DATA_SECTOR};
use crate::memfs::{MemFs, Node};
use std::collections::{HashMap, HashSet};

/// A disc's file system.
pub struct IsoVolume {
    /// The volume identifier, the drive's label.
    pub label: String,
    pub files: MemFs,
    /// The Primary Volume Descriptor.
    pub descriptor: Box<[u8; DATA_SECTOR]>,
}

/// Sector of the first volume descriptor.
const FIRST_DESCRIPTOR: u32 = 16;
/// ISO 9660 allows eight levels of directories.
const MAX_DEPTH: usize = 8;

/// Which directory tree a record is from: names are d-characters in the
/// primary tree and big-endian UCS-2 in Joliet's.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tree {
    Primary,
    Joliet,
}

/// A directory record.
struct Record {
    /// The name as the disc has it, without ";1": upper case in the
    /// primary tree.
    name: String,
    extent: Extent,
    is_dir: bool,
    /// The record's bytes, for MSCDEX's directory entry call.
    raw: Vec<u8>,
}

/// An entry of a directory as the drive shows it.
struct Entry {
    /// The DOS name.
    name: String,
    /// The Joliet name, where it isn't the DOS name.
    long: Option<String>,
    /// The record in the primary tree, or in Joliet's for an entry only
    /// Joliet has.
    record: Record,
    /// Whether the primary tree has the entry.
    in_primary: bool,
    /// For a directory, its extent in the Joliet tree, if Joliet has it.
    joliet_dir: Option<Extent>,
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The volume descriptors of the disc, from sector 16 to the terminator.
fn volume_descriptors(image: &CdImage) -> Result<Vec<Box<[u8; DATA_SECTOR]>>, String> {
    let track = image.data_track().ok_or("no data track")?;
    let mut descriptors = Vec::new();
    for lba in track.start + FIRST_DESCRIPTOR..track.end {
        let mut sector = Box::new([0u8; DATA_SECTOR]);
        image.read_data(lba, &mut sector).map_err(|e| e.to_string())?;
        if &sector[1..6] != b"CD001" {
            // A disc without a terminator ends its descriptors with
            // whatever follows them.
            if descriptors.is_empty() {
                return Err("not an ISO 9660 disc".to_string());
            }
            break;
        }
        if sector[0] == 0xFF {
            break;
        }
        descriptors.push(sector);
    }
    Ok(descriptors)
}

/// The Primary Volume Descriptor of the disc.
pub fn primary_descriptor(image: &CdImage) -> Result<Box<[u8; DATA_SECTOR]>, String> {
    volume_descriptors(image)?
        .into_iter()
        .find(|d| d[0] == 1)
        .ok_or_else(|| "no primary volume descriptor".to_string())
}

/// The root directory of the Joliet tree, if the disc has one: a
/// Supplementary Volume Descriptor whose escape sequences select UCS-2
/// level 1, 2 or 3 (%/@, %/C or %/E).
fn joliet_root(descriptors: &[Box<[u8; DATA_SECTOR]>]) -> Option<Extent> {
    descriptors
        .iter()
        .find(|d| d[0] == 2 && &d[88..90] == b"%/" && matches!(d[90], b'@' | b'C' | b'E'))
        .map(|d| root(d))
}

/// The DOS time and date of a directory record's 7-byte date (years since
/// 1900, month, day, hour, minute, second; the GMT offset is left out).
/// Dates before 1980 show as 1 January 1980 00:00 and dates after 2107 as
/// 31 December 2107 23:59:58, the ends of the range DOS can hold.
fn dos_time(date: &[u8]) -> (u16, u16) {
    let year = 1900 + date[0] as u16;
    let (year, month, day, hour, minute, second) = match year {
        ..1980 => (1980, 1, 1, 0, 0, 0),
        2108.. => (2107, 12, 31, 23, 59, 58),
        _ => (
            year,
            date[1].clamp(1, 12) as u16,
            date[2].clamp(1, 31) as u16,
            date[3].min(23) as u16,
            date[4].min(59) as u16,
            date[5].min(59) as u16,
        ),
    };
    let dos_date = (year - 1980) << 9 | month << 5 | day;
    let dos_time = hour << 11 | minute << 5 | (second / 2);
    (dos_time, dos_date)
}

/// The name a record's file identifier gives, without its version (";1")
/// and the "." of a name without an extension.
fn record_name(id: &[u8], tree: Tree) -> String {
    let name = match tree {
        Tree::Primary => String::from_utf8_lossy(id).to_ascii_uppercase(),
        Tree::Joliet => {
            let units: Vec<u16> = id
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&units)
        }
    };
    name.split(';')
        .next()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_string()
}

/// The records of a directory, without "." and "..".
fn read_directory(image: &CdImage, dir: &Extent, tree: Tree) -> Vec<Record> {
    let mut records = Vec::new();
    let mut sector = [0u8; DATA_SECTOR];
    let sectors = dir.size.div_ceil(DATA_SECTOR as u32);
    for i in 0..sectors {
        if image.read_data(dir.lba + i, &mut sector).is_err() {
            break;
        }
        let mut at = 0;
        // Records don't cross sectors; a length of 0 pads to the next.
        while at < DATA_SECTOR && sector[at] != 0 {
            let len = sector[at] as usize;
            let raw = &sector[at..(at + len).min(DATA_SECTOR)];
            at += len;
            if raw.len() < 34 || raw.len() < 33 + raw[32] as usize {
                break;
            }
            let id = &raw[33..33 + raw[32] as usize];
            // 00h is ".", 01h "..". Associated files (bit 2) hold Apple
            // resource forks and the like.
            if id == [0] || id == [1] || raw[25] & 0x04 != 0 {
                continue;
            }
            let name = record_name(id, tree);
            // Joliet doesn't allow a backslash or a slash in a name, and
            // a path can't have one.
            if tree == Tree::Joliet && (name.is_empty() || name.contains(['\\', '/'])) {
                continue;
            }
            let (time, date) = dos_time(&raw[18..25]);
            records.push(Record {
                name,
                extent: Extent {
                    // Data follows an extended attribute record, if any.
                    lba: le32(raw, 2) + raw[1] as u32,
                    size: le32(raw, 10),
                    time,
                    date,
                    hidden: raw[25] & 0x01 != 0,
                },
                is_dir: raw[25] & 0x02 != 0,
                raw: raw.to_vec(),
            });
        }
    }
    // Files of more than one extent have a record for each; programs see
    // the first. Windows doesn't tell Joliet names apart by case.
    let mut seen = HashSet::new();
    records.retain(|r| seen.insert(r.name.to_lowercase()));
    records
}

/// The files with data directly in a directory, by their extents.
fn data_extents(image: &CdImage, dir: &Extent, tree: Tree) -> HashSet<(u32, u32)> {
    read_directory(image, dir, tree)
        .into_iter()
        .filter(|r| !r.is_dir && r.extent.size > 0)
        .map(|r| (r.extent.lba, r.extent.size))
        .collect()
}

/// For each record of a directory in the Joliet tree, the record of the
/// same directory in the primary tree that is the same file or directory,
/// if there is one. A file is the one with the same data, a directory the
/// one of the same name or else the one with the same files in it. Names
/// decide files only where the data doesn't, as for empty files: the
/// trees often spell them differently ("NSENGL~1.DLL" and
/// "nsenglishres.dll").
fn pair(image: &CdImage, primary: &[Record], joliet: &[Record]) -> Vec<Option<usize>> {
    let primary_names: Vec<&str> = primary.iter().map(|r| r.name.as_str()).collect();
    let primary_short = crate::disk::short_names(&primary_names);
    let joliet_names: Vec<&str> = joliet.iter().map(|r| r.name.as_str()).collect();
    let joliet_short = crate::disk::short_names(&joliet_names);
    // The primary records by their data, and by their names and DOS
    // names, so that a directory of thousands of files isn't compared
    // record by record.
    let mut by_data: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (p, record) in primary.iter().enumerate() {
        if !record.is_dir && record.extent.size > 0 {
            let key = (record.extent.lba, record.extent.size);
            by_data.entry(key).or_default().push(p);
        }
        by_name.entry(&record.name).or_default().push(p);
        if primary_short[p] != record.name {
            by_name.entry(&primary_short[p]).or_default().push(p);
        }
    }
    let joliet_upper: Vec<String> = joliet.iter().map(|r| r.name.to_uppercase()).collect();
    let mut pairs = vec![None; joliet.len()];
    let mut taken = vec![false; primary.len()];
    // First what both the data and a name say, then the data alone, then
    // a name alone.
    for pass in 0..3 {
        for (j, record) in joliet.iter().enumerate() {
            if pairs[j].is_some() {
                continue;
            }
            let free = |p: &usize| !taken[*p] && primary[*p].is_dir == record.is_dir;
            let data: &[usize] = if record.is_dir || record.extent.size == 0 {
                &[]
            } else {
                let key = (record.extent.lba, record.extent.size);
                by_data.get(&key).map_or(&[], Vec::as_slice)
            };
            let named: Vec<usize> = [joliet_upper[j].as_str(), joliet_short[j].as_str()]
                .iter()
                .flat_map(|name| by_name.get(name).into_iter().flatten().copied())
                .collect();
            let found = match pass {
                0 => data.iter().copied().find(|p| free(p) && named.contains(p)),
                1 => data.iter().copied().find(|p| free(p)),
                _ => named.iter().copied().find(|p| free(p)),
            };
            if let Some(p) = found {
                taken[p] = true;
                pairs[j] = Some(p);
            }
        }
    }

    // Directories the trees name too differently to tell ("PROGRAM_" and
    // "Program Files"), by what they hold.
    let paired: HashSet<usize> = pairs.iter().flatten().copied().collect();
    let primary_dirs: Vec<usize> = (0..primary.len())
        .filter(|&p| primary[p].is_dir && !paired.contains(&p))
        .collect();
    let joliet_dirs: Vec<usize> = (0..joliet.len())
        .filter(|&j| joliet[j].is_dir && pairs[j].is_none())
        .collect();
    if primary_dirs.is_empty() || joliet_dirs.is_empty() {
        return pairs;
    }
    let mut primary_files: Vec<Option<HashSet<(u32, u32)>>> = primary_dirs
        .iter()
        .map(|&p| Some(data_extents(image, &primary[p].extent, Tree::Primary)))
        .collect();
    for j in joliet_dirs {
        let files = data_extents(image, &joliet[j].extent, Tree::Joliet);
        let found = primary_files
            .iter()
            .position(|p| p.as_ref().is_some_and(|p| !p.is_disjoint(&files)));
        if let Some(i) = found {
            primary_files[i] = None;
            pairs[j] = Some(primary_dirs[i]);
        }
    }
    pairs
}

/// The entries of a directory, from its records in the primary tree and
/// in Joliet's, either of which may be missing: the primary tree's with
/// the DOS names programs see them by and the Joliet names of those
/// Joliet has, then those only Joliet has, with DOS names made from
/// theirs.
fn entries(image: &CdImage, primary: Option<&Extent>, joliet: Option<&Extent>) -> Vec<Entry> {
    let primary = primary.map_or_else(Vec::new, |dir| read_directory(image, dir, Tree::Primary));
    let joliet = joliet.map_or_else(Vec::new, |dir| read_directory(image, dir, Tree::Joliet));
    let pairs = pair(image, &primary, &joliet);
    let mut joliet_of: Vec<Option<Record>> = (0..primary.len()).map(|_| None).collect();
    let mut only_joliet = Vec::new();
    for (record, paired) in joliet.into_iter().zip(pairs) {
        match paired {
            Some(p) => joliet_of[p] = Some(record),
            None => only_joliet.push(record),
        }
    }
    // The names made for what only Joliet has come after the primary
    // tree's own, and don't take them.
    let names: Vec<&str> = primary
        .iter()
        .chain(&only_joliet)
        .map(|r| r.name.as_str())
        .collect();
    let dos_names = crate::disk::short_names(&names);
    let in_primary = primary.into_iter().zip(joliet_of).map(|(record, joliet)| {
        let joliet_dir = joliet.as_ref().filter(|j| j.is_dir).map(|j| j.extent);
        (record, joliet.map(|j| j.name), true, joliet_dir)
    });
    let in_joliet = only_joliet.into_iter().map(|record| {
        let joliet_dir = record.is_dir.then_some(record.extent);
        let name = record.name.clone();
        (record, Some(name), false, joliet_dir)
    });
    in_primary
        .chain(in_joliet)
        .zip(dos_names)
        .map(|((record, long, in_primary, joliet_dir), name)| Entry {
            long: long.filter(|long| *long != name),
            name,
            record,
            in_primary,
            joliet_dir,
        })
        .collect()
}

/// The root directory's extent, from a volume descriptor.
fn root(descriptor: &[u8; DATA_SECTOR]) -> Extent {
    let record = &descriptor[156..190];
    let (time, date) = dos_time(&record[18..25]);
    Extent { lba: le32(record, 2) + record[1] as u32, size: le32(record, 10), time, date, hidden: false }
}

/// Read the file system of a disc's data track.
pub fn read_volume(image: &CdImage) -> Result<IsoVolume, String> {
    let descriptors = volume_descriptors(image)?;
    let descriptor = descriptors
        .iter()
        .find(|d| d[0] == 1)
        .cloned()
        .ok_or("no primary volume descriptor")?;
    // Padded with spaces, or on some discs with NULs.
    let label = String::from_utf8_lossy(&descriptor[40..72]).trim_matches([' ', '\0']).to_string();
    let mut files = MemFs::new();
    files.insert_dir_record("", root(&descriptor));
    let mut visited = HashSet::new();
    let mut pending = vec![(
        String::new(),
        Some(root(&descriptor)),
        joliet_root(&descriptors),
        0,
    )];
    while let Some((path, primary, joliet, depth)) = pending.pop() {
        // A directory loop on a broken disc.
        if !visited.insert((primary.map(|d| d.lba), joliet.map(|d| d.lba))) {
            continue;
        }
        for entry in entries(image, primary.as_ref(), joliet.as_ref()) {
            let child = if path.is_empty() {
                entry.name
            } else {
                format!("{}\\{}", path, entry.name)
            };
            let extent = entry.record.extent;
            if entry.record.is_dir {
                files.insert_dir_record(&child, extent);
            } else {
                files.insert_node(&child, Node::Extent(extent));
            }
            if let Some(long) = &entry.long {
                files.set_long_name(&child, long);
            }
            if entry.record.is_dir && depth < MAX_DEPTH {
                let primary = entry.in_primary.then_some(extent);
                pending.push((child, primary, entry.joliet_dir, depth + 1));
            }
        }
    }
    Ok(IsoVolume { label, files, descriptor })
}

/// The directory record of the file or directory at a DOS path on the disc
/// ("SAMNMAX\\SAMNMAX.EXE"), for MSCDEX's Get Directory Entry. MSCDEX
/// reads the primary tree alone, so what only Joliet has isn't found.
pub fn find_record(image: &CdImage, path: &str) -> Option<Vec<u8>> {
    let descriptors = volume_descriptors(image).ok()?;
    let mut dir = root(descriptors.iter().find(|d| d[0] == 1)?);
    let mut joliet = joliet_root(&descriptors);
    let mut components = path.split('\\').filter(|c| !c.is_empty()).peekable();
    while let Some(component) = components.next() {
        let entry = entries(image, Some(&dir), joliet.as_ref())
            .into_iter()
            .find(|e| e.in_primary && e.name.eq_ignore_ascii_case(component))?;
        if components.peek().is_none() {
            return Some(entry.record.raw);
        }
        if !entry.record.is_dir {
            return None;
        }
        dir = entry.record.extent;
        joliet = entry.joliet_dir;
    }
    None
}

/// A small Joliet disc in 2048-byte sectors, whose trees differ the ways
/// real discs' do. The primary tree has DOSONLY.BAT, EMPTY.TXT,
/// LONGNAME.DOC, NSENGLIS.DLL, README.TXT and PROGRAM_\SETUP.INI. Joliet
/// (UCS-2 level 1) has the same files as "empty.txt", a 71-character name,
/// "nsenglishres.dll", "ReadMe.txt" and "Program Files\setup.ini", has
/// "Only In Joliet.txt" besides, and hasn't DOSONLY.BAT.
#[cfg(test)]
pub(crate) fn joliet_disc() -> Vec<u8> {
    fn both(v: u32, bytes: usize) -> Vec<u8> {
        let le = v.to_le_bytes();
        let be = v.to_be_bytes();
        le[..bytes]
            .iter()
            .chain(&be[4 - bytes..])
            .copied()
            .collect()
    }
    fn record(id: &[u8], lba: u32, size: u32, dir: bool) -> Vec<u8> {
        let mut r = vec![0u8; 33];
        r[2..10].copy_from_slice(&both(lba, 4));
        r[10..18].copy_from_slice(&both(size, 4));
        r[18..25].copy_from_slice(&[96, 5, 17, 10, 30, 0, 0]);
        r[25] = if dir { 0x02 } else { 0 };
        r[28..32].copy_from_slice(&both(1, 2));
        r[32] = id.len() as u8;
        r.extend_from_slice(id);
        if r.len() % 2 == 1 {
            r.push(0);
        }
        r[0] = r.len() as u8;
        r
    }
    fn ucs2(name: &str) -> Vec<u8> {
        name.encode_utf16().flat_map(u16::to_be_bytes).collect()
    }
    let long_doc = "A document name longer than the sixty-four characters Joliet allows.doc";
    assert_eq!(long_doc.len(), 71);
    // (sector, parent sector, entries as (identifier, sector, size, is a
    // directory)).
    type Dir = (u32, u32, Vec<(Vec<u8>, u32, u32, bool)>);
    let file = |id: Vec<u8>, lba, size| (id, lba, size, false);
    let dirs: [Dir; 4] = [
        (
            20,
            20,
            vec![
                file(b"DOSONLY.BAT;1".to_vec(), 34, 3),
                file(b"EMPTY.TXT;1".to_vec(), 0, 0),
                file(b"LONGNAME.DOC;1".to_vec(), 35, 4),
                file(b"NSENGLIS.DLL;1".to_vec(), 30, 5),
                (b"PROGRAM_".to_vec(), 21, 2048, true),
                file(b"README.TXT;1".to_vec(), 31, 6),
            ],
        ),
        (21, 20, vec![file(b"SETUP.INI;1".to_vec(), 33, 7)]),
        (
            22,
            22,
            vec![
                file(ucs2(&format!("{};1", long_doc)), 35, 4),
                file(ucs2("empty.txt;1"), 0, 0),
                file(ucs2("nsenglishres.dll;1"), 30, 5),
                file(ucs2("Only In Joliet.txt;1"), 32, 8),
                (ucs2("Program Files"), 23, 2048, true),
                file(ucs2("ReadMe.txt;1"), 31, 6),
            ],
        ),
        (23, 22, vec![file(ucs2("setup.ini;1"), 33, 7)]),
    ];
    let mut disc = vec![0u8; 40 * DATA_SECTOR];
    for (lba, parent, records) in dirs {
        let mut sector = record(&[0], lba, 2048, true);
        sector.extend(record(&[1], parent, 2048, true));
        for (id, at, size, dir) in records {
            sector.extend(record(&id, at, size, dir));
        }
        let at = lba as usize * DATA_SECTOR;
        disc[at..at + sector.len()].copy_from_slice(&sector);
    }
    for lba in 30..36 {
        let at = lba * DATA_SECTOR;
        disc[at..at + 8].fill(lba as u8);
    }
    for (lba, kind, root) in [(16, 1, 20), (17, 2, 22), (18, 0xFF, 0)] {
        let d = &mut disc[lba * DATA_SECTOR..(lba + 1) * DATA_SECTOR];
        d[0] = kind;
        d[1..7].copy_from_slice(b"CD001\x01");
        if kind == 0xFF {
            continue;
        }
        d[40..72].fill(b' ');
        d[40..48].copy_from_slice(b"NETSTORM");
        d[80..88].copy_from_slice(&both(40, 4));
        if kind == 2 {
            d[88..91].copy_from_slice(b"%/@");
        }
        d[128..132].copy_from_slice(&both(DATA_SECTOR as u32, 2));
        d[156..190].copy_from_slice(&record(&[0], root, 2048, true));
    }
    disc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joliet_volume() -> (CdImage, IsoVolume) {
        let image = CdImage::from_memory("joliet.iso", joliet_disc().into()).unwrap();
        let volume = read_volume(&image).unwrap();
        (image, volume)
    }

    #[test]
    fn joliet_names_are_the_long_names() {
        let (_, volume) = joliet_volume();
        let files = &volume.files;
        let long = |path: &str| files.long_name(path);
        // The same file in both trees, by its data, however they name it.
        assert_eq!(long("NSENGLIS.DLL"), Some("nsenglishres.dll"));
        assert_eq!(long("README.TXT"), Some("ReadMe.txt"));
        // Joliet's 64 characters are a rule discs don't all keep.
        let long_doc = "A document name longer than the sixty-four characters Joliet allows.doc";
        assert_eq!(long("LONGNAME.DOC"), Some(long_doc));
        // An empty file has no data to go by, but its name.
        assert_eq!(long("EMPTY.TXT"), Some("empty.txt"));
        // A directory, by its name or else by its files.
        assert_eq!(long("PROGRAM_"), Some("Program Files"));
        assert_eq!(long("PROGRAM_\\SETUP.INI"), Some("setup.ini"));
        // What only one tree has.
        assert_eq!(long("DOSONLY.BAT"), None);
        assert_eq!(long("ONLYIN~1.TXT"), Some("Only In Joliet.txt"));
        let only = files.file("ONLYIN~1.TXT");
        assert!(matches!(only, Some(Node::Extent(e)) if (e.lba, e.size) == (32, 8)));
        let names: Vec<&str> = files.list("").into_iter().map(|(name, _)| name).collect();
        assert_eq!(
            names,
            [
                "DOSONLY.BAT",
                "EMPTY.TXT",
                "LONGNAME.DOC",
                "NSENGLIS.DLL",
                "ONLYIN~1.TXT",
                "PROGRAM_",
                "README.TXT"
            ]
        );

        // Paths in long names, in any case, are the DOS ones.
        assert_eq!(files.resolve(&["nsenglishres.dll"]), "NSENGLIS.DLL");
        assert_eq!(
            files.resolve(&["PROGRAM FILES", "Setup.ini"]),
            "PROGRAM_\\SETUP.INI"
        );
        assert_eq!(
            files.resolve(&["Program Files", "missing.txt"]),
            "PROGRAM_\\MISSING.TXT"
        );
    }

    #[test]
    fn mscdex_finds_the_primary_records() {
        let (image, _) = joliet_volume();
        let record = find_record(&image, "PROGRAM_\\SETUP.INI").unwrap();
        assert_eq!(&record[33..33 + record[32] as usize], b"SETUP.INI;1");
        assert!(find_record(&image, "NSENGLIS.DLL").is_some());
        // MSCDEX reads no Joliet names.
        assert!(find_record(&image, "ONLYIN~1.TXT").is_none());
        assert!(find_record(&image, "Program Files\\setup.ini").is_none());
    }

    #[test]
    fn record_dates_fit_the_dos_range() {
        // 31 October 1995, 12:34:56.
        assert_eq!(dos_time(&[95, 10, 31, 12, 34, 56, 0]), (12 << 11 | 34 << 5 | 28, 15 << 9 | 10 << 5 | 31));
        // Before 1980 and after 2107: the ends of the range.
        assert_eq!(dos_time(&[79, 12, 31, 23, 59, 59, 0]), (0, 1 << 5 | 1));
        assert_eq!(dos_time(&[208, 1, 1, 0, 0, 0, 0]), (23 << 11 | 59 << 5 | 29, 127 << 9 | 12 << 5 | 31));
        assert_eq!(dos_time(&[255, 1, 1, 0, 0, 0, 0]), (23 << 11 | 59 << 5 | 29, 127 << 9 | 12 << 5 | 31));
        // 2107 itself fits; fields out of range are cut to theirs.
        assert_eq!(dos_time(&[207, 13, 0, 24, 60, 61, 0]), (23 << 11 | 59 << 5 | 29, 127 << 9 | 12 << 5 | 1));
    }
}
