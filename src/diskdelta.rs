//! A disk image's changes kept in a file of their own (`.rdelta`), which
//! leaves the image as it is: what the machine writes goes to the delta
//! file, a `BLOCK` at a time, and the blocks it never wrote are read from
//! the image below. One image can so be under many deltas, a Windows 95
//! install under each game's, and an image in an archive is written to
//! without copying all of it out first.
//!
//! The file: a header sector (`MAGIC`, the version, the block size, the
//! image's size and what it begins and ends with, hashed), the block map
//! (for each block of the image, a little-endian u32: 0 for the image's,
//! or the number from 1 of the block in the file), then the blocks in the
//! order they were first written. A block goes in before its entry in the
//! map, so a file cut short where it was being written loses that write
//! and nothing else.

use crate::diskimage::{Chs, ImageFile};
use crate::hostfs::{self, File, OpenOptions};
use sha2::{Digest, Sha256};
use std::cell::{Cell, RefCell};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const BLOCK: usize = 64 << 10;
const MAGIC: &[u8; 8] = b"RDOSDLTA";
const VERSION: u32 = 1;
const HEADER: u64 = 512;
/// How much of the image's start and end the hash in the header covers.
const HASHED_START: u64 = 1 << 20;
const HASHED_END: u64 = 64 << 10;

/// Why a delta file can't be opened: it is another disk's changes (or no
/// changes at all), or something else went wrong.
enum Mismatch {
    Changes(String),
    Other(String),
}

pub struct Delta {
    base: ImageFile,
    len: u64,
    path: PathBuf,
    /// What tells the image the delta was made over from another: its
    /// size, start and end, hashed.
    id: [u8; 32],
    /// The file, once there is one: it is made at the first write.
    file: RefCell<Option<File>>,
    writable: bool,
    map: RefCell<Vec<u32>>,
    /// The blocks in the file.
    blocks: Cell<u32>,
}

thread_local! {
    /// What `Delta::open` did that the user should hear of, for the
    /// drive's mount to tell (`take_notes`).
    static NOTES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// What opening deltas did since this was last called: the deltas set
/// aside, as they were of another disk.
pub fn take_notes() -> Vec<String> {
    NOTES.with(|notes| std::mem::take(&mut *notes.borrow_mut()))
}

fn error(path: &Path, e: std::io::Error) -> String {
    format!("{}: {}", path.display(), e)
}

fn read_exact_at(mut file: &File, at: u64, buf: &mut [u8]) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(at))?;
    file.read_exact(buf)
}

fn write_all_at(mut file: &File, at: u64, data: &[u8]) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(at))?;
    file.write_all(data)
}

#[allow(clippy::len_without_is_empty)]
impl Delta {
    /// The image `base` under the delta file `path`, which is made at the
    /// first write if it isn't there. A delta made over another image, or
    /// over this one since changed (a game's package, or the system it
    /// runs in, put in anew), is set aside beside it as
    /// `<name>.stale-<time>`, and the disk starts as the image has it; read
    /// only, it is refused.
    pub fn open(base: &Path, path: &Path, read_only: bool) -> Result<Delta, String> {
        match Self::open_as_is(base, path, read_only) {
            Err(Mismatch::Other(e)) => Err(e),
            Err(Mismatch::Changes(e)) if read_only => Err(e),
            Err(Mismatch::Changes(e)) => {
                let time = crate::hosttime::now().format("%Y%m%d-%H%M%S");
                let mut aside = path.as_os_str().to_owned();
                aside.push(format!(".stale-{}", time));
                let aside = PathBuf::from(aside);
                hostfs::rename(path, &aside).map_err(|io| format!("{} (and it can't be set aside: {})", e, io))?;
                NOTES.with(|notes| notes.borrow_mut().push(format!("{}; it is set aside as {}", e, aside.display())));
                Self::open_as_is(base, path, read_only).map_err(|m| match m {
                    Mismatch::Changes(e) | Mismatch::Other(e) => e,
                })
            }
            Ok(delta) => Ok(delta),
        }
    }

    fn open_as_is(base: &Path, path: &Path, read_only: bool) -> Result<Delta, Mismatch> {
        let other = Mismatch::Other;
        let base_file = File::open(base).map_err(|e| other(error(base, e)))?;
        let base_file = ImageFile::new(base_file, base).map_err(|e| other(format!("{}: {}", base.display(), e)))?;
        let len = base_file.len().map_err(|e| other(error(base, e)))?;
        let id = identity(&base_file, len).map_err(|e| other(error(base, e)))?;
        let blocks_in_image = len.div_ceil(BLOCK as u64) as usize;
        let mut delta = Delta {
            base: base_file,
            len,
            path: path.to_path_buf(),
            id,
            file: RefCell::new(None),
            writable: !read_only,
            map: RefCell::new(vec![0; blocks_in_image]),
            blocks: Cell::new(0),
        };
        if hostfs::is_file(path) {
            let file = match read_only {
                true => File::open(path),
                false => OpenOptions::new().read(true).write(true).open(path),
            }
            .or_else(|_| {
                delta.writable = false;
                File::open(path)
            })
            .map_err(|e| other(error(path, e)))?;
            delta.load(&file, base)?;
            *delta.file.borrow_mut() = Some(file);
        }
        Ok(delta)
    }

    /// The header and map of `file`, if it is a delta over this image.
    fn load(&mut self, file: &File, base: &Path) -> Result<(), Mismatch> {
        let other = Mismatch::Other;
        let mut header = [0u8; HEADER as usize];
        read_exact_at(file, 0, &mut header).map_err(|e| Mismatch::Changes(error(&self.path, e)))?;
        let u32_at = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
        if &header[..8] != MAGIC || u32_at(8) != VERSION || u32_at(12) as usize != BLOCK {
            return Err(Mismatch::Changes(format!("{} is not a disk image's changes", self.path.display())));
        }
        let len = u64::from_le_bytes(header[16..24].try_into().unwrap());
        if len != self.len || header[24..56] != self.id {
            return Err(Mismatch::Changes(format!(
                "{} holds the changes of another disk than {}, or of the disk before it changed",
                self.path.display(),
                base.display()
            )));
        }
        let mut raw = vec![0u8; self.map.borrow().len() * 4];
        read_exact_at(file, HEADER, &mut raw).map_err(|e| other(error(&self.path, e)))?;
        let file_len = file.len().map_err(|e| other(error(&self.path, e)))?;
        let data = self.data_start();
        let mut map = self.map.borrow_mut();
        let mut blocks = 0;
        let mut lost = Vec::new();
        for (index, (entry, bytes)) in map.iter_mut().zip(raw.as_chunks::<4>().0).enumerate() {
            let n = u32::from_le_bytes(*bytes);
            // A block past the end of the file was being written when it
            // was cut short.
            if n != 0 && data + n as u64 * BLOCK as u64 <= file_len {
                *entry = n;
                blocks = blocks.max(n);
            } else if n != 0 {
                lost.push(index);
            }
        }
        // Out of the map, or the block's number would be another's too
        // once it is given out again.
        if self.writable {
            for index in lost {
                write_all_at(file, HEADER + index as u64 * 4, &[0; 4]).map_err(|e| other(error(&self.path, e)))?;
            }
        }
        self.blocks.set(blocks);
        Ok(())
    }

    /// Where the blocks start in the file: after the header and the map,
    /// on a sector.
    fn data_start(&self) -> u64 {
        HEADER + (self.map.borrow().len() as u64 * 4).next_multiple_of(512)
    }

    /// The geometry the image file states, if it states one.
    pub fn base_geometry(&self) -> Option<Chs> {
        self.base.geometry()
    }

    /// The floppy geometry the image file states, if it states one.
    pub fn base_floppy_geometry(&self) -> Option<Chs> {
        self.base.floppy_geometry()
    }

    /// The image's size.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether writes can go to the delta file.
    pub fn writable(&self) -> bool {
        self.writable
    }

    /// The blocks written so far.
    pub fn blocks(&self) -> u32 {
        self.blocks.get()
    }

    pub fn read_at(&self, at: u64, buf: &mut [u8]) -> std::io::Result<()> {
        if at.checked_add(buf.len() as u64).is_none_or(|end| end > self.len) {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        let map = self.map.borrow();
        let file = self.file.borrow();
        let data = self.data_start();
        let mut done = 0;
        while done < buf.len() {
            let pos = at + done as u64;
            let (index, offset) = ((pos / BLOCK as u64) as usize, (pos % BLOCK as u64) as usize);
            let n = (BLOCK - offset).min(buf.len() - done);
            let part = &mut buf[done..done + n];
            match (map[index], file.as_ref()) {
                (0, _) | (_, None) => self.base.read_at(pos, part)?,
                (block, Some(file)) => read_exact_at(file, data + (block as u64 - 1) * BLOCK as u64 + offset as u64, part)?,
            }
            done += n;
        }
        Ok(())
    }

    pub fn write_at(&self, at: u64, data: &[u8]) -> std::io::Result<()> {
        if !self.writable {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        if at.checked_add(data.len() as u64).is_none_or(|end| end > self.len) {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        self.make_file()?;
        let start = self.data_start();
        let mut done = 0;
        while done < data.len() {
            let pos = at + done as u64;
            let (index, offset) = ((pos / BLOCK as u64) as usize, (pos % BLOCK as u64) as usize);
            let n = (BLOCK - offset).min(data.len() - done);
            let block = self.block_for(index, n == BLOCK)?;
            let file = self.file.borrow();
            let file = file.as_ref().expect("made");
            write_all_at(file, start + (block as u64 - 1) * BLOCK as u64 + offset as u64, &data[done..done + n])?;
            done += n;
        }
        Ok(())
    }

    /// The number of block `index` in the file, put there with what the
    /// image has unless all of it is about to be written over.
    fn block_for(&self, index: usize, whole: bool) -> std::io::Result<u32> {
        let block = self.map.borrow()[index];
        if block != 0 {
            return Ok(block);
        }
        let mut contents = vec![0u8; BLOCK];
        if !whole {
            let at = index as u64 * BLOCK as u64;
            let n = (self.len - at).min(BLOCK as u64) as usize;
            self.base.read_at(at, &mut contents[..n])?;
        }
        let block = self.blocks.get() + 1;
        let file = self.file.borrow();
        let file = file.as_ref().expect("made");
        write_all_at(file, self.data_start() + (block as u64 - 1) * BLOCK as u64, &contents)?;
        write_all_at(file, HEADER + index as u64 * 4, &block.to_le_bytes())?;
        self.map.borrow_mut()[index] = block;
        self.blocks.set(block);
        Ok(block)
    }

    /// The delta file, with its header and an empty map, if there is none
    /// yet.
    fn make_file(&self) -> std::io::Result<()> {
        if self.file.borrow().is_some() {
            return Ok(());
        }
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            hostfs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&self.path)?;
        let mut header = [0u8; HEADER as usize];
        header[..8].copy_from_slice(MAGIC);
        header[8..12].copy_from_slice(&VERSION.to_le_bytes());
        header[12..16].copy_from_slice(&(BLOCK as u32).to_le_bytes());
        header[16..24].copy_from_slice(&self.len.to_le_bytes());
        header[24..56].copy_from_slice(&self.id);
        write_all_at(&file, 0, &header)?;
        file.set_len(self.data_start())?;
        *self.file.borrow_mut() = Some(file);
        Ok(())
    }
}

/// What tells an image from another, or from itself after it changed where
/// it is most likely to: its size, its first MB (the partition table, boot
/// sector and FATs of most disks) and its last 64 KB.
fn identity(base: &ImageFile, len: u64) -> std::io::Result<[u8; 32]> {
    let mut hash = Sha256::new();
    hash.update(len.to_le_bytes());
    let mut start = vec![0u8; len.min(HASHED_START) as usize];
    base.read_at(0, &mut start)?;
    hash.update(&start);
    let end_at = len.saturating_sub(HASHED_END).max(start.len() as u64);
    let mut end = vec![0u8; (len - end_at) as usize];
    base.read_at(end_at, &mut end)?;
    hash.update(&end);
    Ok(hash.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/test-diskdelta").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An image whose every byte tells where it is.
    fn image(dir: &Path, len: usize) -> PathBuf {
        let path = dir.join("base.img");
        std::fs::write(&path, (0..len).map(|i| (i / 512) as u8 ^ i as u8).collect::<Vec<_>>()).unwrap();
        path
    }

    fn read(delta: &Delta, at: u64, len: usize) -> Vec<u8> {
        let mut buf = vec![0u8; len];
        delta.read_at(at, &mut buf).unwrap();
        buf
    }

    #[test]
    fn writes_go_to_the_delta_and_the_image_stays() {
        let dir = scratch("writes");
        let base = image(&dir, 3 * BLOCK + 1536);
        let before = std::fs::read(&base).unwrap();
        let path = dir.join("C.rdelta");
        let delta = Delta::open(&base, &path, false).unwrap();
        assert_eq!(read(&delta, 1000, 3000), before[1000..4000]);
        assert!(!path.exists(), "made at the first write");
        // Across two blocks, and the short last one.
        delta.write_at(BLOCK as u64 - 512, &[0xAA; 1024]).unwrap();
        delta.write_at(3 * BLOCK as u64 + 512, &[0xBB; 1024]).unwrap();
        assert_eq!(delta.blocks(), 3);
        let mut expected = before.clone();
        expected[BLOCK - 512..BLOCK + 512].fill(0xAA);
        expected[3 * BLOCK + 512..3 * BLOCK + 1536].fill(0xBB);
        assert_eq!(read(&delta, 0, expected.len()), expected);
        assert_eq!(std::fs::read(&base).unwrap(), before, "the image is as it was");
        assert!(delta.write_at(expected.len() as u64 - 512, &[0; 1024]).is_err());

        // Opened again, the changes are there.
        drop(delta);
        let again = Delta::open(&base, &path, true).unwrap();
        assert_eq!(read(&again, 0, expected.len()), expected);
        assert!(again.write_at(0, &[0; 512]).is_err(), "read-only");
    }

    #[test]
    fn a_delta_goes_over_a_dynamic_vhd() {
        let dir = scratch("vhd");
        let base = dir.join("disk.vhd");
        std::fs::write(&base, crate::vhd::make_dynamic(4 << 20, 2 << 20)).unwrap();
        let before = std::fs::read(&base).unwrap();
        let delta = Delta::open(&base, &dir.join("disk.vhd.rdelta"), false).unwrap();
        assert_eq!(delta.len(), 4 << 20);
        delta.write_at(3 << 20, &[0x77; 512]).unwrap();
        assert_eq!(read(&delta, (3 << 20) - 512, 1024), [[0u8; 512], [0x77; 512]].concat());
        assert_eq!(std::fs::read(&base).unwrap(), before, "the image is as it was");
    }

    #[test]
    fn a_delta_over_another_image_is_set_aside() {
        let dir = scratch("other");
        let base = image(&dir, 2 * BLOCK);
        let path = dir.join("C.rdelta");
        Delta::open(&base, &path, false).unwrap().write_at(0, &[1; 512]).unwrap();
        let mut changed = std::fs::read(&base).unwrap();
        changed[100] ^= 0xFF;
        std::fs::write(&base, changed).unwrap();
        assert!(Delta::open(&base, &path, true).is_err());
        // Set aside, never lost: the disk is the image's again.
        take_notes();
        let delta = Delta::open(&base, &path, false).unwrap();
        assert_eq!(delta.blocks(), 0);
        assert!(!path.exists());
        let notes = take_notes();
        assert!(notes.len() == 1 && notes[0].contains("set aside"), "{:?}", notes);
        let aside: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("C.rdelta.stale-")).collect();
        assert_eq!(aside.len(), 1);
        std::fs::write(&path, b"not a delta").unwrap();
        assert!(Delta::open(&base, &path, true).is_err());
    }

    #[test]
    fn a_block_cut_short_is_lost_alone() {
        let dir = scratch("cut");
        let base = image(&dir, 4 * BLOCK);
        let before = std::fs::read(&base).unwrap();
        let path = dir.join("C.rdelta");
        let delta = Delta::open(&base, &path, false).unwrap();
        delta.write_at(0, &[0xAA; 512]).unwrap();
        delta.write_at(2 * BLOCK as u64, &[0xBB; 512]).unwrap();
        drop(delta);
        let len = std::fs::metadata(&path).unwrap().len();
        std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(len - 100).unwrap();
        let delta = Delta::open(&base, &path, false).unwrap();
        assert_eq!(read(&delta, 0, 512), vec![0xAA; 512]);
        assert_eq!(read(&delta, 2 * BLOCK as u64, 512), before[2 * BLOCK..2 * BLOCK + 512]);
        // The lost block's number goes to the next one written, alone.
        delta.write_at(3 * BLOCK as u64, &[0xCC; 512]).unwrap();
        drop(delta);
        let delta = Delta::open(&base, &path, true).unwrap();
        assert_eq!(read(&delta, 2 * BLOCK as u64, 512), before[2 * BLOCK..2 * BLOCK + 512]);
        assert_eq!(read(&delta, 3 * BLOCK as u64, 512), vec![0xCC; 512]);
    }
}
