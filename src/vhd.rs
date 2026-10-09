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
//! Differencing images, which keep a parent's changes, aren't read.

use crate::diskimage::Chs;
use crate::hostfs::File;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};

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

pub struct Vhd {
    file: File,
    len: u64,
    geometry: Option<Chs>,
    dynamic: Option<Dynamic>,
}

#[allow(clippy::len_without_is_empty)]
impl Vhd {
    /// The image in `file`, which is only written to by `write_at`.
    pub fn open(file: File) -> Result<Vhd, String> {
        let file_len = file.len().map_err(|e| e.to_string())?;
        let footer = footer(&file, file_len).ok_or("not a VHD image")?;
        let len = be64(&footer, 48);
        let (c, h, s) = (u16::from_be_bytes([footer[56], footer[57]]), footer[58], footer[59]);
        let geometry = (c > 0 && (1..=16).contains(&h) && (1..=63).contains(&s))
            .then_some(Chs { cylinders: c as u32, heads: h as u32, sectors: s as u32 });
        let dynamic = match be32(&footer, 60) {
            FIXED => {
                if len > file_len - FOOTER.min(file_len) {
                    return Err("the VHD image is cut short".into());
                }
                None
            }
            DYNAMIC => Some(Self::dynamic(&file, &footer, file_len, len)?),
            DIFFERENCING => return Err("differencing VHD images (a parent's changes) aren't supported".into()),
            kind => return Err(format!("a VHD image of unknown type {}", kind)),
        };
        Ok(Vhd { file, len, geometry, dynamic })
    }

    fn dynamic(file: &File, footer: &[u8; FOOTER as usize], file_len: u64, len: u64) -> Result<Dynamic, String> {
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
        Ok(Dynamic { table_at, block_size, table: RefCell::new(table), bitmaps: Default::default(), end: Cell::new(end), footer: *footer })
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
                UNUSED => part.fill(0),
                block => {
                    read_exact_at(&self.file, self.data_at(d, block) + offset, part)?;
                    self.with_bitmap(d, index, block, |bitmap| {
                        // Sectors never written read as zeros.
                        for (i, chunk) in part.chunks_mut(SECTOR as usize).enumerate() {
                            let sector = (offset / SECTOR) as usize + i;
                            if bitmap[sector / 8] & (0x80 >> (sector % 8)) == 0 {
                                chunk.fill(0);
                            }
                        }
                    })?;
                }
            }
            done += n;
        }
        Ok(())
    }

    pub fn write_at(&self, at: u64, data: &[u8]) -> std::io::Result<()> {
        if at.checked_add(data.len() as u64).is_none_or(|end| end > self.len) {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        let Some(d) = &self.dynamic else {
            return write_all_at(&self.file, at, data);
        };
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
    let entries = len.div_ceil(block as u64) as u32;
    let footer = make_footer(len, DYNAMIC, 512);
    let mut out = footer.to_vec();
    let mut header = [0u8; 1024];
    header[..8].copy_from_slice(DYNAMIC_COOKIE);
    header[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
    header[16..24].copy_from_slice(&1536u64.to_be_bytes());
    header[24..28].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    header[28..32].copy_from_slice(&entries.to_be_bytes());
    header[32..36].copy_from_slice(&block.to_be_bytes());
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

    #[test]
    fn a_differencing_image_is_refused() {
        let mut data = vec![0u8; 4096];
        data.extend_from_slice(&make_footer(4096, DIFFERENCING, 0));
        let path = scratch("differencing", &data);
        let file = crate::hostfs::File::open(&path).unwrap();
        assert!(Vhd::open(file).err().unwrap().contains("differencing"));
    }
}
