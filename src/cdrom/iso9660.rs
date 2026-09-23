//! The ISO 9660 file system of a CD's data track, read into a directory
//! tree of DOS names, as MSCDEX shows it to programs.

use super::image::CdImage;
use super::{Extent, DATA_SECTOR};
use crate::memfs::{MemFs, Node};
use std::collections::HashSet;

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

/// A directory record.
struct Record {
    /// The name as the disc has it, without ";1".
    name: String,
    extent: Extent,
    is_dir: bool,
    /// The record's bytes, for MSCDEX's directory entry call.
    raw: Vec<u8>,
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The Primary Volume Descriptor of the disc.
pub fn primary_descriptor(image: &CdImage) -> Result<Box<[u8; DATA_SECTOR]>, String> {
    let track = image.data_track().ok_or("no data track")?;
    let mut sector = Box::new([0u8; DATA_SECTOR]);
    for lba in track.start + FIRST_DESCRIPTOR..track.end {
        image.read_data(lba, &mut sector).map_err(|e| e.to_string())?;
        if &sector[1..6] != b"CD001" {
            return Err("not an ISO 9660 disc".to_string());
        }
        match sector[0] {
            1 => return Ok(sector),
            0xFF => break,
            _ => {}
        }
    }
    Err("no primary volume descriptor".to_string())
}

/// The DOS date and time of a directory record's 7-byte date.
fn dos_time(date: &[u8]) -> (u16, u16) {
    let year = 1900 + date[0] as u16;
    let (month, day) = (date[1].clamp(1, 12) as u16, date[2].clamp(1, 31) as u16);
    let dos_date = (year.max(1980) - 1980) << 9 | month << 5 | day;
    let dos_time = (date[3] as u16) << 11 | (date[4] as u16) << 5 | date[5] as u16 / 2;
    (dos_time, dos_date)
}

/// The records of a directory, without "." and "..".
fn read_directory(image: &CdImage, dir: &Extent) -> Vec<Record> {
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
            let name = String::from_utf8_lossy(id);
            let name = name.split(';').next().unwrap_or_default().trim_end_matches('.').to_ascii_uppercase();
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
    // the first.
    let mut seen = HashSet::new();
    records.retain(|r| seen.insert(r.name.clone()));
    records
}

/// The records of a directory with the DOS names programs see them by.
fn dos_entries(image: &CdImage, dir: &Extent) -> Vec<(String, Record)> {
    let records = read_directory(image, dir);
    let names: Vec<&str> = records.iter().map(|r| r.name.as_str()).collect();
    crate::disk::short_names(&names).into_iter().zip(records).collect()
}

/// The root directory's extent.
fn root(descriptor: &[u8; DATA_SECTOR]) -> Extent {
    let record = &descriptor[156..190];
    let (time, date) = dos_time(&record[18..25]);
    Extent { lba: le32(record, 2) + record[1] as u32, size: le32(record, 10), time, date, hidden: false }
}

/// Read the file system of a disc's data track.
pub fn read_volume(image: &CdImage) -> Result<IsoVolume, String> {
    let descriptor = primary_descriptor(image)?;
    // Padded with spaces, or on some discs with NULs.
    let label = String::from_utf8_lossy(&descriptor[40..72]).trim_matches([' ', '\0']).to_string();
    let mut files = MemFs::new();
    let mut visited = HashSet::new();
    let mut pending = vec![(String::new(), root(&descriptor), 0)];
    while let Some((path, dir, depth)) = pending.pop() {
        if !visited.insert(dir.lba) {
            continue; // A directory loop on a broken disc.
        }
        for (name, record) in dos_entries(image, &dir) {
            let child = if path.is_empty() { name } else { format!("{}\\{}", path, name) };
            if record.is_dir {
                files.insert_dir(&child);
                if depth < MAX_DEPTH {
                    pending.push((child, record.extent, depth + 1));
                }
            } else {
                files.insert_node(&child, Node::Extent(record.extent));
            }
        }
    }
    Ok(IsoVolume { label, files, descriptor })
}

/// The directory record of the file or directory at a DOS path on the disc
/// ("SAMNMAX\\SAMNMAX.EXE"), for MSCDEX's Get Directory Entry.
pub fn find_record(image: &CdImage, path: &str) -> Option<Vec<u8>> {
    let descriptor = primary_descriptor(image).ok()?;
    let mut dir = root(&descriptor);
    let mut components = path.split('\\').filter(|c| !c.is_empty()).peekable();
    while let Some(component) = components.next() {
        let (_, record) = dos_entries(image, &dir)
            .into_iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(component))?;
        if components.peek().is_none() {
            return Some(record.raw);
        }
        if !record.is_dir {
            return None;
        }
        dir = record.extent;
    }
    None
}
