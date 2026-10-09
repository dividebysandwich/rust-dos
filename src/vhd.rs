//! Virtual PC hard disk images (.vhd), fixed and dynamic, as the sectors
//! of the disk they hold.
//!
//! A fixed image is the disk's sectors followed by a 512-byte footer
//! ("conectix", big-endian fields: the disk's size, its geometry, its
//! type). A dynamic one starts with a copy of the footer, then a header
//! ("cxsparse") that says where the block allocation table is and how big
//! the blocks are. The table holds, for each block of the disk, the
//! sector of the file it starts at, or FFFFFFFFh for a block never
//! written, which reads as zeros. A block is a bitmap of the sectors in it
//! that were written, then their data. A block written for the first time
//! goes where the footer was, and the footer after it.
//!
//! A differencing image is a dynamic one that holds the changes to
//! another image, its parent: what its blocks and bitmaps don't have is
//! the parent's. Its header names the parent and has the id from the
//! parent's footer, which is how the parent is found and checked
//! (`find_parent`).

use crate::diskimage::Chs;
use crate::hostfs::File;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const FOOTER: u64 = 512;
const COOKIE: &[u8; 8] = b"conectix";
const DYNAMIC_COOKIE: &[u8; 8] = b"cxsparse";
const FIXED: u32 = 2;
const DYNAMIC: u32 = 3;
const DIFFERENCING: u32 = 4;
const UNUSED: u32 = 0xFFFF_FFFF;
const SECTOR: u64 = 512;

fn read_exact_at(mut file: &File, at: u64, buf: &mut [u8]) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(at))?;
    file.read_exact(buf)
}

fn write_all_at(mut file: &File, at: u64, data: &[u8]) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(at))?;
    file.write_all(data)
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}

fn be64(b: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(b[at..at + 8].try_into().unwrap())
}

/// The footer of a file of `len` bytes: its last sector's, or the copy
/// a dynamic image starts with.
fn footer(file: &File, len: u64) -> Option<[u8; FOOTER as usize]> {
    let mut sector = [0u8; FOOTER as usize];
    [len.checked_sub(FOOTER)?, 0]
        .into_iter()
        .any(|at| read_exact_at(file, at, &mut sector).is_ok() && &sector[..8] == COOKIE)
        .then_some(sector)
}

/// Whether the file is a VHD image.
pub fn is_vhd(file: &File) -> bool {
    file.len().is_ok_and(|len| len >= FOOTER && footer(file, len).is_some())
}

/// The id in the footer of the VHD image in `file`, and for a
/// differencing image the id of its parent's, which it holds the changes
/// to.
pub fn ids(file: &File) -> Option<([u8; 16], Option<[u8; 16]>)> {
    let footer = footer(file, file.len().ok()?)?;
    let id = footer[68..84].try_into().unwrap();
    if be32(&footer, 60) != DIFFERENCING {
        return Some((id, None));
    }
    let mut header = [0u8; 56];
    read_exact_at(file, be64(&footer, 16), &mut header).ok()?;
    Some((id, Some(header[40..56].try_into().unwrap())))
}

struct Dynamic {
    /// Where the table is in the file.
    table_at: u64,
    block_size: u64,
    table: RefCell<Vec<u32>>,
    /// Each written block's bitmap, as it was read or written.
    bitmaps: RefCell<HashMap<usize, Vec<u8>>>,
    /// Where the footer is: where the next block goes.
    end: Cell<u64>,
    footer: [u8; FOOTER as usize],
}

/// The image a differencing image holds the changes to.
struct Parent {
    image: Box<crate::diskimage::ImageFile>,
    /// What serves the parent's file while it is open, when it is in an
    /// archive.
    _layer: Option<crate::hostfs::Layer>,
}

pub struct Vhd {
    file: File,
    len: u64,
    geometry: Option<Chs>,
    dynamic: Option<Dynamic>,
    /// The footer's unique id, which a differencing image over this one
    /// has in its header.
    id: [u8; 16],
    parent: Option<Parent>,
}

#[allow(clippy::len_without_is_empty)]
impl Vhd {
    /// The image in `file`, which is only written to by `write_at`. A
    /// differencing image needs to know where it is (`open_at`).
    pub fn open(file: File) -> Result<Vhd, String> {
        Self::open_at(file, None)
    }

    /// The image in `file`, which is at `path`: a differencing image's
    /// parent is looked for beside it (`find_parent`).
    pub fn open_at(file: File, path: Option<&Path>) -> Result<Vhd, String> {
        let file_len = file.len().map_err(|e| e.to_string())?;
        let footer = footer(&file, file_len).ok_or("not a VHD image")?;
        let len = be64(&footer, 48);
        let (c, h, s) = (u16::from_be_bytes([footer[56], footer[57]]), footer[58], footer[59]);
        let geometry = (c > 0 && (1..=16).contains(&h) && (1..=63).contains(&s))
            .then_some(Chs { cylinders: c as u32, heads: h as u32, sectors: s as u32 });
        let id = footer[68..84].try_into().unwrap();
        let (dynamic, parent) = match be32(&footer, 60) {
            FIXED => {
                if len > file_len - FOOTER.min(file_len) {
                    return Err("the VHD image is cut short".into());
                }
                (None, None)
            }
            DYNAMIC => (Some(Self::dynamic(&file, &footer, file_len, len)?.0), None),
            DIFFERENCING => {
                let (dynamic, header) = Self::dynamic(&file, &footer, file_len, len)?;
                let parent = find_parent(&file, &header, path)?;
                let parent_len = parent.image.len().map_err(|e| e.to_string())?;
                if parent_len != len {
                    return Err(format!("its parent is a disk of {} bytes, not {}", parent_len, len));
                }
                (Some(dynamic), Some(parent))
            }
            kind => return Err(format!("a VHD image of unknown type {}", kind)),
        };
        Ok(Vhd { file, len, geometry, dynamic, id, parent })
    }

    fn dynamic(file: &File, footer: &[u8; FOOTER as usize], file_len: u64, len: u64) -> Result<(Dynamic, [u8; 1024]), String> {
        let error = |e: std::io::Error| e.to_string();
        let mut header = [0u8; 1024];
        read_exact_at(file, be64(footer, 16), &mut header).map_err(error)?;
        if &header[..8] != DYNAMIC_COOKIE {
            return Err("the VHD image's dynamic disk header is missing".into());
        }
        let (table_at, entries, block_size) = (be64(&header, 16), be32(&header, 28) as usize, be32(&header, 32) as u64);
        if block_size == 0 || block_size % SECTOR != 0 || (entries as u64) < len.div_ceil(block_size) {
            return Err("the VHD image's dynamic disk header is damaged".into());
        }
        let mut raw = vec![0u8; entries * 4];
        read_exact_at(file, table_at, &mut raw).map_err(error)?;
        let table = raw.as_chunks::<4>().0.iter().map(|b| u32::from_be_bytes(*b)).collect();
        // Where the footer is, or would be on a file whose footer was cut
        // off.
        let mut cookie = [0u8; 8];
        let end = match read_exact_at(file, file_len - FOOTER, &mut cookie) {
            Ok(()) if &cookie == COOKIE && file_len.is_multiple_of(SECTOR) => file_len - FOOTER,
            _ => file_len.next_multiple_of(SECTOR),
        };
        let dynamic = Dynamic { table_at, block_size, table: RefCell::new(table), bitmaps: Default::default(), end: Cell::new(end), footer: *footer };
        Ok((dynamic, header))
    }

    /// The disk's size.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// The geometry in the footer, if it is one the BIOS can have.
    pub fn geometry(&self) -> Option<Chs> {
        self.geometry
    }

    pub fn read_at(&self, at: u64, buf: &mut [u8]) -> std::io::Result<()> {
        if at.checked_add(buf.len() as u64).is_none_or(|end| end > self.len) {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        let Some(d) = &self.dynamic else {
            return read_exact_at(&self.file, at, buf);
        };
        let mut done = 0;
        while done < buf.len() {
            let pos = at + done as u64;
            let (index, offset) = ((pos / d.block_size) as usize, pos % d.block_size);
            let n = ((d.block_size - offset) as usize).min(buf.len() - done);
            let part = &mut buf[done..done + n];
            match d.table.borrow()[index] {
                UNUSED => self.unwritten(pos, part)?,
                block => {
                    read_exact_at(&self.file, self.data_at(d, block) + offset, part)?;
                    let bitmap = self.with_bitmap(d, index, block, |bitmap| bitmap.clone())?;
                    // The sectors never written: zeros, or the parent's.
                    let mut i = 0;
                    while i < n {
                        let sector = ((offset + i as u64) / SECTOR) as usize;
                        let end = (((sector as u64 + 1) * SECTOR - offset) as usize).min(n);
                        if bitmap[sector / 8] & (0x80 >> (sector % 8)) == 0 {
                            self.unwritten(pos + i as u64, &mut part[i..end])?;
                        }
                        i = end;
                    }
                }
            }
            done += n;
        }
        Ok(())
    }

    /// What the disk has at `at` where the image never wrote: zeros, or
    /// a differencing image's parent's.
    fn unwritten(&self, at: u64, buf: &mut [u8]) -> std::io::Result<()> {
        match &self.parent {
            Some(parent) => parent.image.read_at(at, buf),
            None => {
                buf.fill(0);
                Ok(())
            }
        }
    }

    pub fn write_at(&self, at: u64, data: &[u8]) -> std::io::Result<()> {
        if at.checked_add(data.len() as u64).is_none_or(|end| end > self.len) {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        let Some(d) = &self.dynamic else {
            return write_all_at(&self.file, at, data);
        };
        // A sector written in part is written whole, with the rest of it
        // what the parent has, as the sector is the image's once written.
        if self.parent.is_some() && (!at.is_multiple_of(SECTOR) || !(data.len() as u64).is_multiple_of(SECTOR)) {
            let start = at - at % SECTOR;
            let end = (at + data.len() as u64).next_multiple_of(SECTOR).min(self.len);
            let mut whole = vec![0u8; (end - start) as usize];
            self.read_at(start, &mut whole)?;
            whole[(at - start) as usize..][..data.len()].copy_from_slice(data);
            return self.write_at(start, &whole);
        }
        let mut done = 0;
        while done < data.len() {
            let pos = at + done as u64;
            let (index, offset) = ((pos / d.block_size) as usize, pos % d.block_size);
            let n = ((d.block_size - offset) as usize).min(data.len() - done);
            let block = match d.table.borrow()[index] {
                UNUSED => None,
                block => Some(block),
            };
            let block = match block {
                Some(block) => block,
                None => self.allocate(d, index)?,
            };
            write_all_at(&self.file, self.data_at(d, block) + offset, &data[done..done + n])?;
            let (first, last) = ((offset / SECTOR) as usize, ((offset + n as u64 - 1) / SECTOR) as usize);
            let changed = self.with_bitmap(d, index, block, |bitmap| {
                let mut changed = false;
                for sector in first..=last {
                    changed |= bitmap[sector / 8] & (0x80 >> (sector % 8)) == 0;
                    bitmap[sector / 8] |= 0x80 >> (sector % 8);
                }
                changed.then(|| bitmap.clone())
            })?;
            if let Some(bitmap) = changed {
                write_all_at(&self.file, block as u64 * SECTOR, &bitmap)?;
            }
            done += n;
        }
        Ok(())
    }

    /// The size of a block's bitmap in the file, whole sectors.
    fn bitmap_len(d: &Dynamic) -> u64 {
        (d.block_size / SECTOR).div_ceil(8).next_multiple_of(SECTOR)
    }

    fn data_at(&self, d: &Dynamic, block: u32) -> u64 {
        block as u64 * SECTOR + Self::bitmap_len(d)
    }

    fn with_bitmap<T>(&self, d: &Dynamic, index: usize, block: u32, f: impl FnOnce(&mut Vec<u8>) -> T) -> std::io::Result<T> {
        let mut bitmaps = d.bitmaps.borrow_mut();
        let bitmap = match bitmaps.entry(index) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let mut bitmap = vec![0u8; Self::bitmap_len(d) as usize];
                read_exact_at(&self.file, block as u64 * SECTOR, &mut bitmap)?;
                e.insert(bitmap)
            }
        };
        Ok(f(bitmap))
    }

    /// A block for the disk's block `index`, empty, where the footer was;
    /// the footer goes after it, then the table says where it is. A file
    /// cut short while this was written loses the block and nothing else.
    fn allocate(&self, d: &Dynamic, index: usize) -> std::io::Result<u32> {
        let at = d.end.get();
        let block = u32::try_from(at / SECTOR).map_err(|_| std::io::Error::other("the VHD image is full"))?;
        let size = Self::bitmap_len(d) + d.block_size;
        self.file.set_len(at + size)?;
        write_all_at(&self.file, at, &vec![0u8; Self::bitmap_len(d) as usize])?;
        write_all_at(&self.file, at + size, &d.footer)?;
        write_all_at(&self.file, d.table_at + index as u64 * 4, &block.to_be_bytes())?;
        // The copy at the start stays as it is.
        d.table.borrow_mut()[index] = block;
        d.bitmaps.borrow_mut().insert(index, vec![0u8; Self::bitmap_len(d) as usize]);
        d.end.set(at + size);
        Ok(block)
    }
}

/// The file names a differencing image's header gives its parent: its
/// name, then the last part of each path its locators hold.
fn parent_names(file: &File, header: &[u8; 1024]) -> Vec<String> {
    let utf16 = |bytes: &[u8], big: bool| -> String {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| if big { u16::from_be_bytes(*b) } else { u16::from_le_bytes(*b) })
            .take_while(|&u| u != 0)
            .collect();
        String::from_utf16_lossy(&units)
    };
    let mut names = vec![utf16(&header[64..576], true)];
    for entry in header[576..768].chunks(24) {
        let (code, len, at) = (&entry[..4], be32(entry, 8) as usize, be64(entry, 16));
        let mut data = vec![0u8; len.min(4096)];
        if len == 0 || read_exact_at(file, at, &mut data).is_err() {
            continue;
        }
        let path = match code {
            b"W2ku" | b"W2ru" => utf16(&data, false),
            b"MacX" => String::from_utf8_lossy(&data).trim_end_matches('\0').to_string(),
            _ => continue,
        };
        names.push(path.rsplit(['/', '\\']).next().unwrap_or("").to_string());
    }
    let mut unique: Vec<String> = Vec::new();
    for name in names {
        if !name.is_empty() && !unique.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            unique.push(name);
        }
    }
    unique
}

/// The parent of the differencing image in `file`, at `path`, whose
/// dynamic disk header is `header`: by the names it gives it, beside it,
/// then in the OS images folders (`os_images`), and any VHD there with
/// the id it has. A parent whose id isn't that one is another disk, or
/// the disk since changed, and is refused.
fn find_parent(file: &File, header: &[u8; 1024], path: Option<&Path>) -> Result<Parent, String> {
    let wanted: [u8; 16] = header[40..56].try_into().unwrap();
    let names = parent_names(file, header);
    let shown = names.first().cloned().unwrap_or_else(|| "its parent".to_string());
    let mut candidates: Vec<PathBuf> = Vec::new();
    for name in &names {
        if let Some(dir) = path.and_then(Path::parent) {
            candidates.push(dir.join(name));
        }
        let stem = name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem);
        candidates.extend(crate::os_images::find(name).or_else(|| crate::os_images::find(stem)));
    }
    // Those of its name, then any with its parent's id.
    let named = candidates.len();
    for dir in crate::os_images::dirs() {
        let vhds = crate::hostfs::read_dir(&dir).into_iter().flatten().filter(|e| {
            !e.is_dir && e.path.extension().is_some_and(|x| x.eq_ignore_ascii_case("vhd"))
        });
        candidates.extend(vhds.map(|e| e.path));
    }
    let mut other = None;
    for (i, candidate) in candidates.into_iter().enumerate() {
        if path.is_some_and(|p| p == candidate) || !(crate::hostfs::exists(&candidate) || crate::archive::split(&candidate).is_some()) {
            continue;
        }
        let Ok((image, layer)) = crate::diskimage::open_read_only(&candidate) else { continue };
        match &image {
            crate::diskimage::ImageFile::Vhd(vhd) if vhd.id == wanted => {
                return Ok(Parent { image: Box::new(image), _layer: layer });
            }
            _ if i < named => {
                other.get_or_insert(candidate);
            }
            _ => {}
        }
    }
    Err(match other {
        Some(found) => format!("it holds the changes to another {} than {}, or to that disk before it changed", shown, found.display()),
        None => format!("it holds the changes to {}, which isn't beside it or in the OS images folder", shown),
    })
}

/// The geometry a VHD's footer gives a disk of `sectors` sectors, as the
/// specification works it out (Virtual PC's): at most 65535 cylinders, 16
/// heads and 255 sectors per track.
fn footer_geometry(sectors: u64) -> (u16, u8, u8) {
    let total = sectors.min(65535 * 16 * 255);
    let (mut per_track, mut heads, mut cylinders_times_heads);
    if total >= 65535 * 16 * 63 {
        (per_track, heads) = (255, 16);
        cylinders_times_heads = total / per_track;
    } else {
        per_track = 17;
        cylinders_times_heads = total / per_track;
        heads = cylinders_times_heads.div_ceil(1024).max(4);
        if cylinders_times_heads >= heads * 1024 || heads > 16 {
            (per_track, heads) = (31, 16);
            cylinders_times_heads = total / per_track;
        }
        if cylinders_times_heads >= heads * 1024 {
            (per_track, heads) = (63, 16);
            cylinders_times_heads = total / per_track;
        }
    }
    ((cylinders_times_heads / heads) as u16, heads as u8, per_track as u8)
}

/// The one's complement of the sum of `bytes`, which VHD checksums are.
fn checksum(bytes: &[u8]) -> [u8; 4] {
    (!bytes.iter().map(|&b| b as u32).fold(0u32, u32::wrapping_add)).to_be_bytes()
}

/// The footer of a VHD image of a disk of `len` bytes of `kind`, made
/// now by Rust-DOS, its checksum filled in.
pub(crate) fn make_footer(len: u64, kind: u32, header_at: u64) -> [u8; 512] {
    use std::hash::{BuildHasher, Hasher};
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    // Seconds since 2000-01-01.
    let time = now.as_secs().saturating_sub(946_684_800) as u32;
    let mut f = [0u8; 512];
    f[..8].copy_from_slice(COOKIE);
    f[8..12].copy_from_slice(&2u32.to_be_bytes());
    f[12..16].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    f[16..24].copy_from_slice(&header_at.to_be_bytes());
    f[24..28].copy_from_slice(&time.to_be_bytes());
    f[28..32].copy_from_slice(b"rdos");
    f[32..36].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    f[36..40].copy_from_slice(b"Wi2k");
    f[40..48].copy_from_slice(&len.to_be_bytes());
    f[48..56].copy_from_slice(&len.to_be_bytes());
    let (cylinders, heads, sectors) = footer_geometry(len / SECTOR);
    f[56..58].copy_from_slice(&cylinders.to_be_bytes());
    f[58] = heads;
    f[59] = sectors;
    f[60..64].copy_from_slice(&kind.to_be_bytes());
    // A unique id, from hashers seeded at random.
    for half in f[68..84].chunks_mut(8) {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(now.as_nanos());
        half.copy_from_slice(&hasher.finish().to_be_bytes());
    }
    let sum = checksum(&f);
    f[64..68].copy_from_slice(&sum);
    f
}

/// The footer of a fixed VHD image of a disk of `len` bytes, which goes
/// after its sectors.
pub(crate) fn fixed_footer(len: u64) -> [u8; 512] {
    make_footer(len, FIXED, u64::MAX)
}

/// A dynamic VHD image of a disk of `len` bytes in blocks of `block`
/// bytes, with nothing written.
pub(crate) fn make_dynamic(len: u64, block: u32) -> Vec<u8> {
    make_sparse(len, block, None)
}

/// A differencing VHD image over the parent called `parent_name` whose
/// footer's id is `parent_id`, of a disk of `len` bytes in blocks of
/// `block` bytes, with nothing changed.
#[cfg(test)]
pub(crate) fn make_differencing(len: u64, block: u32, parent_id: [u8; 16], parent_name: &str) -> Vec<u8> {
    make_sparse(len, block, Some((parent_id, parent_name)))
}

/// The id in the footer of the VHD image `data`.
#[cfg(test)]
pub(crate) fn image_id(data: &[u8]) -> [u8; 16] {
    data[68..84].try_into().unwrap()
}

fn make_sparse(len: u64, block: u32, parent: Option<([u8; 16], &str)>) -> Vec<u8> {
    let entries = len.div_ceil(block as u64) as u32;
    let footer = make_footer(len, if parent.is_some() { DIFFERENCING } else { DYNAMIC }, 512);
    let mut out = footer.to_vec();
    let mut header = [0u8; 1024];
    header[..8].copy_from_slice(DYNAMIC_COOKIE);
    header[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
    header[16..24].copy_from_slice(&1536u64.to_be_bytes());
    header[24..28].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    header[28..32].copy_from_slice(&entries.to_be_bytes());
    header[32..36].copy_from_slice(&block.to_be_bytes());
    if let Some((id, name)) = parent {
        header[40..56].copy_from_slice(&id);
        for (i, unit) in name.encode_utf16().take(256).enumerate() {
            header[64 + i * 2..66 + i * 2].copy_from_slice(&unit.to_be_bytes());
        }
    }
    let sum = checksum(&header);
    header[36..40].copy_from_slice(&sum);
    out.extend_from_slice(&header);
    let table_len = (entries as usize * 4).next_multiple_of(512);
    out.extend(std::iter::repeat_n(0xFFu8, table_len));
    out.extend_from_slice(&footer);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str, data: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rust-dos-vhd-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("disk.vhd");
        std::fs::write(&path, data).unwrap();
        path
    }

    fn open(path: &std::path::Path) -> Vhd {
        let file = crate::hostfs::OpenOptions::new().read(true).write(true).open(path).unwrap();
        Vhd::open(file).unwrap()
    }

    #[test]
    fn a_fixed_image_is_its_sectors_without_the_footer() {
        let mut data: Vec<u8> = (0..64 * 1024u32).map(|i| (i / 512) as u8).collect();
        data.extend_from_slice(&make_footer(64 * 1024, FIXED, u64::MAX));
        let path = scratch("fixed", &data);
        let vhd = open(&path);
        assert_eq!(vhd.len(), 64 * 1024);
        let mut buf = [0u8; 512];
        vhd.read_at(5 * 512, &mut buf).unwrap();
        assert!(buf.iter().all(|&b| b == 5));
        assert!(vhd.read_at(64 * 1024, &mut buf).is_err());
    }

    #[test]
    fn a_dynamic_image_grows_where_it_is_written() {
        let path = scratch("dynamic", &make_dynamic(8 << 20, 2 << 20));
        let before = std::fs::metadata(&path).unwrap().len();
        let vhd = open(&path);
        assert_eq!(vhd.len(), 8 << 20);
        let mut buf = vec![0xAAu8; 1024];
        vhd.read_at(3 << 20, &mut buf).unwrap();
        assert!(buf.iter().all(|&b| b == 0));
        vhd.write_at((3 << 20) + 512, &[0x55; 700]).unwrap();
        drop(vhd);
        assert!(std::fs::metadata(&path).unwrap().len() > before);
        let vhd = open(&path);
        let mut buf = vec![0xAAu8; 2048];
        vhd.read_at(3 << 20, &mut buf).unwrap();
        assert!(buf[..512].iter().all(|&b| b == 0));
        assert!(buf[512..1212].iter().all(|&b| b == 0x55));
        assert!(buf[1212..].iter().all(|&b| b == 0));
        // The footer is at the end again.
        let data = std::fs::read(&path).unwrap();
        assert_eq!(&data[data.len() - 512..data.len() - 504], COOKIE);
    }

    #[test]
    fn footers_have_virtual_pcs_geometry_and_checksum() {
        // As qemu-img makes them, but for its rounding the size up to whole
        // cylinders.
        for (mb, chs) in [(20, (602, 4, 17)), (500, (1015, 16, 63)), (40 << 10, (20560, 16, 255)), (200 << 10, (65535, 16, 255))] {
            assert_eq!(footer_geometry(mb << 11), chs, "{} MB", mb);
        }
        let f = make_footer(500 << 20, FIXED, u64::MAX);
        let sum = !f.iter().enumerate().filter(|(i, _)| !(64..68).contains(i)).map(|(_, &b)| b as u32).sum::<u32>();
        assert_eq!(be32(&f, 64), sum);
        assert_ne!(f[68..84], make_footer(500 << 20, FIXED, u64::MAX)[68..84], "ids are unique");
    }

    /// A parent of 4 MiB whose sector n holds n, and a differencing image
    /// over it called `child.vhd` beside it.
    fn parent_and_child(name: &str) -> (PathBuf, PathBuf) {
        let parent = scratch(name, &make_dynamic(4 << 20, 1 << 20)).with_file_name("parent.vhd");
        std::fs::rename(parent.with_file_name("disk.vhd"), &parent).unwrap();
        let vhd = open(&parent);
        for sector in 0..(4 << 20) / 512u64 {
            vhd.write_at(sector * 512, &[sector as u8; 512]).unwrap();
        }
        drop(vhd);
        let id = image_id(&std::fs::read(&parent).unwrap());
        let child = parent.with_file_name("child.vhd");
        std::fs::write(&child, make_differencing(4 << 20, 1 << 20, id, "parent.vhd")).unwrap();
        (parent, child)
    }

    fn open_child(path: &std::path::Path) -> Result<Vhd, String> {
        let file = crate::hostfs::OpenOptions::new().read(true).write(true).open(path).unwrap();
        Vhd::open_at(file, Some(path))
    }

    #[test]
    fn a_differencing_image_reads_its_parent_where_it_has_nothing() {
        let (parent, child) = parent_and_child("differencing");
        let vhd = open_child(&child).unwrap();
        let mut buf = vec![0u8; 1024];
        vhd.read_at(7 * 512, &mut buf).unwrap();
        assert!(buf[..512].iter().all(|&b| b == 7) && buf[512..].iter().all(|&b| b == 8));
        // Part of a sector: the rest of it stays the parent's.
        vhd.write_at(9 * 512 + 100, &[0xEE; 10]).unwrap();
        vhd.write_at(20 * 512, &[0xDD; 512]).unwrap();
        drop(vhd);
        let parent_before = std::fs::read(&parent).unwrap();
        let vhd = open_child(&child).unwrap();
        let mut sector = [0u8; 512];
        vhd.read_at(9 * 512, &mut sector).unwrap();
        assert!(sector[..100].iter().all(|&b| b == 9) && sector[100..110].iter().all(|&b| b == 0xEE));
        assert!(sector[110..].iter().all(|&b| b == 9));
        vhd.read_at(20 * 512, &mut sector).unwrap();
        assert!(sector.iter().all(|&b| b == 0xDD));
        // In the same block, sectors it didn't write are the parent's.
        vhd.read_at(21 * 512, &mut sector).unwrap();
        assert!(sector.iter().all(|&b| b == 21));
        assert_eq!(std::fs::read(&parent).unwrap(), parent_before, "the parent isn't written");
    }

    #[test]
    fn a_differencing_image_needs_its_own_parent() {
        let (parent, child) = parent_and_child("differencing-other");
        // Another disk of the name.
        std::fs::write(&parent, make_dynamic(4 << 20, 1 << 20)).unwrap();
        assert!(open_child(&child).err().unwrap().contains("another parent.vhd"));
        std::fs::remove_file(&parent).unwrap();
        assert!(open_child(&child).err().unwrap().contains("isn't beside it"));
        // Without knowing where it is, it can't look beside it.
        let file = crate::hostfs::File::open(&child).unwrap();
        assert!(Vhd::open(file).is_err());
    }
}
