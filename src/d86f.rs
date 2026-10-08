//! 86Box's 86F floppy images (.86f), as the sectors of the disk they hold.
//!
//! An 86F image keeps each track as the flux transitions of its FM or MFM
//! encoding, as 86Box's floppy controller reads them (fdd_86f.c, which this
//! follows). The header is "86BF", the version (2.12) and the disk flags;
//! then a table of where each track is, 512 entries (track * 2 + side) on a
//! two-sided disk and 256 on a one-sided one, 0 for a track not there.
//! A track is its flags (data rate, encoding, RPM), the extra bit cells it
//! has if the disk says tracks have them, where the index hole is, then the
//! bit cells, MSB first, and as many bytes again of surface bits if the
//! disk has them: a cell with its surface bit set is weak where it reads 1
//! and a hole where it reads 0.
//!
//! There is no floppy controller to hand the bits to here: the disk is
//! decoded into its 512-byte sectors, numbered 1 on in each track, and a
//! sector written is encoded back into the track where its data field was
//! and written to the file there, so the image stays an 86F image. Weak
//! bits read as they are stored; sectors of other sizes, out of the 1..N
//! numbering or duplicated aren't seen.

use crate::diskimage::{Chs, STATUS_CRC_ERROR, STATUS_SECTOR_NOT_FOUND};
use crate::hostfs::File;
use std::cell::RefCell;
use std::io::{Read, Seek, SeekFrom, Write};

const MAGIC: &[u8; 4] = b"86BF";
const MAGIC_COMPRESSED: &[u8; 4] = b"86bf";
const VERSION: u16 = 0x020C;
const HEADER: usize = 8;
const SECTOR: usize = 512;

/// Disk flags.
const SURFACE: u16 = 0x0001;
const TWO_SIDES: u16 = 0x0008;
const WRITE_PROTECT: u16 = 0x0010;
const EXTRA_CELLS: u16 = 0x0080;
const ZONED: u16 = 0x0100;
const ZONE_TYPE: u16 = 0x0600;
const REVERSE: u16 = 0x0800;
const SPEED_UP: u16 = 0x1000;
/// Arrays sized in bytes rather than words (86Box's "mpc").
const BYTE_ARRAYS: u16 = 0x2000;

/// The raw MFM words of an A1 and a C2 without their clock bit, and the FM
/// words of the address marks with their C7 (D7 for the index mark) clock.
const MFM_A1: u16 = 0x4489;
const MFM_C2: u16 = 0x5224;
const FM_IDAM: u16 = 0xF57E;
const FM_DAM: u16 = 0xF56F;
const FM_DELETED: u16 = 0xF56A;
const FM_IAM: u16 = 0xF77A;

/// Whether `header`, the start of a file, is an 86F image's.
pub fn is_86f(header: &[u8]) -> bool {
    header.len() >= 4 && (&header[..4] == MAGIC || &header[..4] == MAGIC_COMPRESSED)
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// CRC-16/CCITT, from FFFFh, as the controller checks IDs and data.
fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &b in bytes {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

/// How many bit cells a revolution of a track with flags `track_flags`
/// has on a disk with `disk_flags` (86Box's common_get_raw_size).
fn raw_cells(disk_flags: u16, track_flags: u16, extra: i32) -> u32 {
    let rm = (disk_flags >> 5) & 3;
    let speed_up = disk_flags & SPEED_UP != 0;
    if rm == 0 && speed_up && extra != 0 {
        return extra as u32;
    }
    let mut diff = [1.0, 1.01, 1.015, 1.02][rm as usize];
    if speed_up {
        diff = 1.0 / diff;
    }
    let mut rate = match track_flags & 7 {
        0 => 500.0,
        1 => 300.0,
        3 => 1000.0,
        5 => 2000.0,
        _ => 250.0,
    };
    if !is_mfm(track_flags) {
        rate /= 2.0;
    }
    let rpm = if track_flags & 0xE0 == 0x20 { 360.0 } else { 300.0 };
    let size = 100000.0 / 250.0 * rate * 300.0 / rpm * diff;
    ((((size as u32) >> 4) << 4) as i64 + extra as i64).max(0) as u32
}

fn is_mfm(track_flags: u16) -> bool {
    track_flags & 0x18 == 0x08
}

/// How many bytes of cells (and of surface bits) a track keeps in the file
/// (86Box's d86f_get_array_size).
fn array_bytes(disk_flags: u16, extra: i32) -> usize {
    let rm = ((disk_flags >> 5) & 3) as usize;
    let speed_up = disk_flags & SPEED_UP != 0;
    let words: i64 = if rm == 0 && speed_up {
        0
    } else {
        let (base, slow, fast) = match (disk_flags >> 1) & 3 {
            0 | 1 => (12500, [12625, 12687, 12750], [12376, 12315, 12254]),
            2 => (25000, [25250, 25375, 25500], [24752, 24630, 24509]),
            _ => (50000, [50500, 50750, 51000], [49504, 49261, 49019]),
        };
        match rm {
            0 => base,
            _ if speed_up => fast[rm - 1],
            _ => slow[rm - 1],
        }
    };
    let cells = ((words << 4) + extra as i64).max(0) as usize;
    if disk_flags & BYTE_ARRAYS != 0 { cells.div_ceil(8) } else { cells.div_ceil(16) * 2 }
}

/// Bytes in the order of their cells, from the file's (or back): words
/// swapped on a disk stored the other way round.
fn file_order(disk_flags: u16, bytes: &mut [u8]) {
    if disk_flags & REVERSE != 0 {
        for pair in bytes.as_chunks_mut::<2>().0 {
            pair.swap(0, 1);
        }
    }
}

/// Where a 512-byte sector's data field is on its track.
#[derive(Clone, Copy, Debug)]
struct SectorLoc {
    /// The cell its first data byte starts at.
    data_cell: u32,
    /// The data byte of the address mark before it.
    mark: u8,
    crc_ok: bool,
}

/// A track of one side: its cells and where its sectors are.
struct Track {
    /// Where its header is in the file.
    at: u64,
    header_len: usize,
    flags: u16,
    /// Cells per revolution.
    cells: u32,
    /// The cells, MSB first, in their order on the track.
    bits: Vec<u8>,
    surface: Option<Vec<u8>>,
    /// Sector R at index R - 1.
    sectors: Vec<Option<SectorLoc>>,
    /// Whether each one's ID names this cylinder and head.
    exact: Vec<bool>,
    /// The cylinders the good IDs on the track name.
    ids: Vec<u32>,
}

impl Track {
    fn bit(&self, cell: u32) -> u8 {
        let cell = (cell % self.cells) as usize;
        (self.bits[cell / 8] >> (7 - cell % 8)) & 1
    }

    fn set_bit(&mut self, cell: u32, bit: u8) {
        let cell = (cell % self.cells) as usize;
        let mask = 0x80u8 >> (cell % 8);
        self.bits[cell / 8] = (self.bits[cell / 8] & !mask) | if bit != 0 { mask } else { 0 };
        if let Some(surface) = &mut self.surface {
            surface[cell / 8] &= !mask;
        }
    }

    /// The 16 cells from `cell` on.
    fn word(&self, cell: u32) -> u16 {
        (0..16).fold(0, |w, i| (w << 1) | self.bit(cell + i) as u16)
    }

    /// The byte whose cells start at `cell`: the second cell of each pair
    /// is the data bit, in FM and MFM alike.
    fn byte(&self, cell: u32) -> u8 {
        (0..8).fold(0, |b, i| (b << 1) | self.bit(cell + 2 * i + 1))
    }

    fn bytes(&self, cell: u32, n: usize) -> Vec<u8> {
        (0..n).map(|i| self.byte(cell + 16 * i as u32)).collect()
    }

    /// Find the address marks in a revolution, and the sectors.
    fn decode(&mut self, cylinder: u32, head: u32) {
        let mfm = is_mfm(self.flags);
        // (mark byte, the cell after it, the bytes the CRC starts with)
        let mut marks: Vec<(u8, u32, Vec<u8>)> = Vec::new();
        let mut window = 0u16;
        let mut cell = 0u32;
        // Past the index, for a sector across it.
        let end = self.cells + 1100 * 16;
        while cell < end {
            window = (window << 1) | self.bit(cell) as u16;
            cell += 1;
            if mfm && window == MFM_A1 {
                let mut next = cell;
                let mut syncs = 1;
                while self.word(next) == MFM_A1 {
                    next += 16;
                    syncs += 1;
                }
                let mark = self.byte(next);
                if syncs >= 3 && matches!(mark, 0xFE | 0xFB | 0xF8 | 0xF9 | 0xFA) {
                    marks.push((mark, next + 16, vec![0xA1, 0xA1, 0xA1, mark]));
                }
                cell = next;
                window = 0;
            } else if !mfm && matches!(window, FM_IDAM | FM_DAM | FM_DELETED) {
                let mark = (0..8).fold(0u8, |b, i| (b << 1) | ((window >> (14 - 2 * i)) & 1) as u8);
                marks.push((mark, cell, vec![mark]));
                window = 0;
            }
        }
        // Within the gap a controller waits for the data mark.
        let reach = if mfm { 43 * 16 } else { 30 * 16 };
        for (i, (mark, at, start)) in marks.iter().enumerate() {
            if *mark != 0xFE || *at >= self.cells + 16 * 16 {
                continue;
            }
            let id = self.bytes(*at, 6);
            let mut covered = start.clone();
            covered.extend_from_slice(&id[..4]);
            if crc16(&covered) != u16::from_be_bytes([id[4], id[5]]) {
                continue;
            }
            let (c, h, r, n) = (id[0] as u32, id[1] as u32, id[2] as usize, id[3]);
            self.ids.push(c);
            if n != 2 || r == 0 || r > self.sectors.len() {
                continue;
            }
            let Some((dam, data_at, dstart)) = marks.get(i + 1).filter(|m| m.0 != 0xFE && m.1 - at <= reach + 6 * 16) else {
                continue;
            };
            let field = self.bytes(*data_at, SECTOR + 2);
            let mut covered = dstart.clone();
            covered.extend_from_slice(&field[..SECTOR]);
            let crc_ok = crc16(&covered) == u16::from_be_bytes([field[SECTOR], field[SECTOR + 1]]);
            let loc = SectorLoc { data_cell: *data_at % self.cells, mark: *dam, crc_ok };
            // The first copy of a sector, one whose ID names this
            // cylinder and head over one that doesn't.
            let exact = c == cylinder && h == head;
            if self.sectors[r - 1].is_none() || (exact && !self.exact[r - 1]) {
                self.sectors[r - 1] = Some(loc);
                self.exact[r - 1] = exact;
            }
        }
    }

    /// How many sectors from 1 on are all there.
    fn run(&self) -> usize {
        self.sectors.iter().take_while(|s| s.is_some()).count()
    }

    fn read(&self, r: usize, buf: &mut [u8]) {
        match self.sectors.get(r.wrapping_sub(1)).copied().flatten() {
            Some(loc) => buf.copy_from_slice(&self.bytes(loc.data_cell, buf.len())),
            None => buf.fill(0),
        }
    }

    /// Encode `data` and its CRC into sector `r`'s data field; the range
    /// of bytes of `bits` changed, or all of them past the end for a field
    /// across the index.
    fn write(&mut self, r: usize, data: &[u8]) -> Option<(usize, usize)> {
        let loc = self.sectors.get(r.wrapping_sub(1)).copied().flatten()?;
        let mfm = is_mfm(self.flags);
        let mut covered = if mfm { vec![0xA1, 0xA1, 0xA1, loc.mark] } else { vec![loc.mark] };
        covered.extend_from_slice(data);
        let mut field = data.to_vec();
        field.extend_from_slice(&crc16(&covered).to_be_bytes());
        let mut cell = loc.data_cell;
        let mut prev = loc.mark & 1;
        for byte in field {
            for i in (0..8).rev() {
                let bit = (byte >> i) & 1;
                let clock = if mfm { (prev | bit) ^ 1 } else { 1 };
                self.set_bit(cell, clock);
                self.set_bit(cell + 1, bit);
                prev = bit;
                cell += 2;
            }
        }
        if mfm {
            // The clock of the next cell pair follows the last bit written.
            let next = self.bit(cell + 1);
            self.set_bit(cell, (prev | next) ^ 1);
            cell += 1;
        }
        if let Some(s) = &mut self.sectors[r - 1] {
            s.crc_ok = true;
        }
        match cell > self.cells {
            true => Some((0, usize::MAX)),
            false => Some((loc.data_cell as usize / 8, (cell as usize).div_ceil(8))),
        }
    }
}

/// An 86F image, decoded.
pub struct D86f {
    file: Option<File>,
    flags: u16,
    geometry: Chs,
    /// Indexed by cylinder * heads + head.
    tracks: Vec<Option<RefCell<Track>>>,
}

#[allow(clippy::len_without_is_empty)]
impl D86f {
    /// The image in `file`, which is only written to by `write_at`.
    pub fn open(mut file: File) -> Result<D86f, String> {
        let mut bytes = Vec::new();
        file.seek(SeekFrom::Start(0)).and_then(|_| file.read_to_end(&mut bytes)).map_err(|e| e.to_string())?;
        let mut image = Self::parse(&bytes)?;
        image.file = Some(file);
        Ok(image)
    }

    /// The image in `bytes`.
    pub fn parse(bytes: &[u8]) -> Result<D86f, String> {
        if bytes.len() < 16 || !is_86f(bytes) {
            return Err("not an 86F image".into());
        }
        if &bytes[..4] == MAGIC_COMPRESSED {
            return Err("compressed 86F images aren't supported".into());
        }
        let version = le16(bytes, 4);
        if version != VERSION {
            return Err(format!("86F version {}.{:02} isn't supported (only 2.12)", version >> 8, version & 0xFF));
        }
        let flags = le16(bytes, 6);
        if flags & (ZONED | ZONE_TYPE) != 0 {
            return Err("zoned (Apple or Commodore) 86F disks aren't supported".into());
        }
        let heads = if flags & TWO_SIDES != 0 { 2 } else { 1 };
        let entries = 256 * heads;
        if bytes.len() < HEADER + entries * 4 {
            return Err("the 86F image is cut short".into());
        }
        let offsets: Vec<u32> = (0..entries).map(|i| le32(bytes, HEADER + i * 4)).collect();
        if offsets[0] == 0 || (heads == 2 && offsets[1] == 0) {
            return Err("the 86F image has no track 0".into());
        }
        let load = |track: usize, head: usize| -> Result<Option<Track>, String> {
            let Some(&at) = offsets.get(track * heads + head).filter(|&&at| at != 0) else {
                return Ok(None);
            };
            let at = at as usize;
            let short = || "the 86F image is cut short".to_string();
            let header_len = if flags & EXTRA_CELLS != 0 { 10 } else { 6 };
            let header = bytes.get(at..at + header_len).ok_or_else(short)?;
            let track_flags = le16(header, 0);
            let mut extra = if flags & EXTRA_CELLS != 0 { le32(header, 2) as i32 } else { 0 };
            if flags & (SPEED_UP | 0x60) != SPEED_UP {
                extra = extra.clamp(-32768, 32768);
            }
            let len = array_bytes(flags, extra);
            let data_at = at + header_len;
            let mut bits = bytes.get(data_at..data_at + len).ok_or_else(short)?.to_vec();
            file_order(flags, &mut bits);
            let surface = match flags & SURFACE {
                0 => None,
                _ => {
                    let mut s = bytes.get(data_at + len..data_at + 2 * len).ok_or_else(short)?.to_vec();
                    file_order(flags, &mut s);
                    Some(s)
                }
            };
            let cells = raw_cells(flags, track_flags, extra).min(len as u32 * 8);
            if cells < 16 {
                return Ok(None);
            }
            Ok(Some(Track { at: at as u64, header_len, flags: track_flags, cells, bits, surface, sectors: vec![None; 255], exact: vec![false; 255], ids: Vec::new() }))
        };
        // The tracks by where the head was: by cylinder, or every other
        // one for a 40-track disk taken in thin tracks.
        let mut decoded: Vec<Vec<Option<Track>>> = Vec::new();
        for track in 0..256 {
            let mut sides = Vec::new();
            for head in 0..heads {
                let mut t = load(track, head)?;
                if let Some(t) = &mut t {
                    t.decode(track as u32, head as u32);
                }
                sides.push(t);
            }
            decoded.push(sides);
        }
        let cylinder_of = |t: &Option<Track>| -> Option<u32> {
            let t = t.as_ref()?;
            t.ids.iter().copied().max_by_key(|&c| t.ids.iter().filter(|&&d| d == c).count())
        };
        let thin = cylinder_of(&decoded[2][0]) == Some(1) && cylinder_of(&decoded[4][0]) == Some(2);
        let step = if thin { 2 } else { 1 };
        let mut by_cylinder: Vec<Vec<Option<Track>>> = decoded.into_iter().step_by(step).collect();
        if thin {
            // The IDs were matched against the thin track number.
            for (cylinder, sides) in by_cylinder.iter_mut().enumerate() {
                for (head, t) in sides.iter_mut().enumerate() {
                    if let Some(t) = t {
                        t.sectors.fill(None);
                        t.exact.fill(false);
                        t.decode(cylinder as u32, head as u32);
                    }
                }
            }
        }
        // The usual number of sectors, and the cylinders that have them.
        let mut counts = [0usize; 256];
        for t in by_cylinder.iter().flatten().flatten() {
            counts[t.run()] += 1;
        }
        let sectors = (1..256).max_by_key(|&n| (counts[n], n)).filter(|&n| counts[n] > 0).ok_or("the 86F image has no 512-byte sectors")?;
        let cylinders = by_cylinder.iter().rposition(|sides| sides.iter().flatten().any(|t| t.run() > 0)).map_or(0, |c| c + 1);
        by_cylinder.truncate(cylinders);
        let tracks = by_cylinder.into_iter().flatten().map(|t| t.map(RefCell::new)).collect();
        let geometry = Chs { cylinders: cylinders as u32, heads: heads as u32, sectors: sectors as u32 };
        Ok(D86f { file: None, flags, geometry, tracks })
    }

    pub fn geometry(&self) -> Chs {
        self.geometry
    }

    pub fn len(&self) -> u64 {
        self.geometry.total() * SECTOR as u64
    }

    /// Whether the disk's write-protect tab is set.
    pub fn write_protected(&self) -> bool {
        self.flags & WRITE_PROTECT != 0
    }

    /// Where in the file sector `lba`'s data field starts.
    #[cfg(test)]
    pub(crate) fn data_field_at(&self, lba: u64) -> Option<usize> {
        let (track, r) = self.place(lba);
        let track = track?.borrow();
        let loc = track.sectors.get(r - 1).copied().flatten()?;
        Some(track.at as usize + track.header_len + loc.data_cell as usize / 8)
    }

    /// The track and sector number of sector `lba`.
    fn place(&self, lba: u64) -> (Option<&RefCell<Track>>, usize) {
        let spt = self.geometry.sectors as u64;
        let track = (lba / spt) as usize;
        (self.tracks.get(track).and_then(Option::as_ref), (lba % spt) as usize + 1)
    }

    /// What a BIOS reports reading sector `lba`, if it isn't a clean read:
    /// not found, or a data CRC error.
    pub fn sector_status(&self, lba: u64) -> Option<u8> {
        let (track, r) = self.place(lba);
        match track.and_then(|t| t.borrow().sectors.get(r - 1).copied().flatten()) {
            None => Some(STATUS_SECTOR_NOT_FOUND),
            Some(loc) if !loc.crc_ok => Some(STATUS_CRC_ERROR),
            Some(_) => None,
        }
    }

    pub fn read_at(&self, at: u64, buf: &mut [u8]) -> std::io::Result<()> {
        if !at.is_multiple_of(SECTOR as u64) || !buf.len().is_multiple_of(SECTOR) {
            let start = at / SECTOR as u64;
            let end = (at + buf.len() as u64).div_ceil(SECTOR as u64);
            let mut whole = vec![0u8; ((end - start) as usize) * SECTOR];
            self.read_at(start * SECTOR as u64, &mut whole)?;
            let skip = (at % SECTOR as u64) as usize;
            buf.copy_from_slice(&whole[skip..skip + buf.len()]);
            return Ok(());
        }
        if at + buf.len() as u64 > self.len() {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        for (i, sector) in buf.as_chunks_mut::<SECTOR>().0.iter_mut().enumerate() {
            match self.place(at / SECTOR as u64 + i as u64) {
                (Some(track), r) => track.borrow().read(r, sector),
                (None, _) => sector.fill(0),
            }
        }
        Ok(())
    }

    pub fn write_at(&self, at: u64, data: &[u8]) -> std::io::Result<()> {
        if !at.is_multiple_of(SECTOR as u64) || !data.len().is_multiple_of(SECTOR) {
            let start = at / SECTOR as u64;
            let end = (at + data.len() as u64).div_ceil(SECTOR as u64);
            let mut whole = vec![0u8; ((end - start) as usize) * SECTOR];
            self.read_at(start * SECTOR as u64, &mut whole)?;
            let skip = (at % SECTOR as u64) as usize;
            whole[skip..skip + data.len()].copy_from_slice(data);
            return self.write_at(start * SECTOR as u64, &whole);
        }
        if self.write_protected() {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        if at + data.len() as u64 > self.len() {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        for (i, sector) in data.as_chunks::<SECTOR>().0.iter().enumerate() {
            let (Some(track), r) = self.place(at / SECTOR as u64 + i as u64) else {
                return Err(std::io::Error::other("the sector isn't on the disk"));
            };
            let mut track = track.borrow_mut();
            let (from, to) = track.write(r, sector).ok_or_else(|| std::io::Error::other("the sector isn't on the disk"))?;
            self.save(&track, from, to)?;
        }
        Ok(())
    }

    /// Write bytes `from..to` of a track's cells (and surface bits) back to
    /// the file, in whole words.
    fn save(&self, track: &Track, from: usize, to: usize) -> std::io::Result<()> {
        let Some(file) = &self.file else { return Ok(()) };
        let len = track.bits.len();
        let (from, to) = if to > len { (0, len) } else { (from & !1, (to.next_multiple_of(2)).min(len)) };
        let data_at = track.at + track.header_len as u64;
        let out = |at: u64, bytes: &[u8]| -> std::io::Result<()> {
            let mut bytes = bytes[from..to].to_vec();
            file_order(self.flags, &mut bytes);
            let mut file = file;
            file.seek(SeekFrom::Start(at + from as u64))?;
            file.write_all(&bytes)
        };
        out(data_at, &track.bits)?;
        if let Some(surface) = &track.surface {
            out(data_at + len as u64, surface)?;
        }
        Ok(())
    }
}

/// How `encode` lays an image out.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default)]
pub struct EncodeOptions {
    pub fm: bool,
    pub surface: bool,
    pub reverse: bool,
    pub extra_cells: i32,
    /// Each cylinder twice, as 86Box keeps a 40-track drive's disk.
    pub thin: bool,
    pub write_protect: bool,
}

/// An 86F image of the floppy whose sectors are `flat`, laid out as 86Box
/// lays out a sector image's tracks.
#[doc(hidden)]
pub fn encode(flat: &[u8], chs: Chs, opts: EncodeOptions) -> Vec<u8> {
    let (hole, rate, rpm360) = match chs.sectors {
        s if s >= 36 => (2u16, 3u16, false),
        15 if chs.cylinders >= 80 => (1, 0, true),
        s if s >= 15 => (1, 0, false),
        _ => (0, 2, false),
    };
    let mut disk_flags = hole << 1;
    if chs.heads == 2 {
        disk_flags |= TWO_SIDES;
    }
    if opts.surface {
        disk_flags |= SURFACE;
    }
    if opts.reverse {
        disk_flags |= REVERSE;
    }
    if opts.extra_cells != 0 {
        disk_flags |= EXTRA_CELLS;
    }
    if opts.write_protect {
        disk_flags |= WRITE_PROTECT;
    }
    let track_flags = rate | if opts.fm { 0 } else { 0x08 } | if rpm360 { 0x20 } else { 0 };
    let heads = chs.heads as usize;
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&disk_flags.to_le_bytes());
    out.resize(HEADER + 256 * heads * 4, 0);
    let cells = raw_cells(disk_flags, track_flags, opts.extra_cells);
    let len = array_bytes(disk_flags, opts.extra_cells);
    let copies = if opts.thin { 2 } else { 1 };
    for c in 0..chs.cylinders as usize {
        for copy in 0..copies {
            for h in 0..heads {
                let entry = (c * copies + copy) * heads + h;
                let at = out.len() as u32;
                out[HEADER + entry * 4..HEADER + entry * 4 + 4].copy_from_slice(&at.to_le_bytes());
                out.extend_from_slice(&track_flags.to_le_bytes());
                if opts.extra_cells != 0 {
                    out.extend_from_slice(&opts.extra_cells.to_le_bytes());
                }
                out.extend_from_slice(&0u32.to_le_bytes());
                let first = ((c * heads + h) * chs.sectors as usize) * SECTOR;
                let data = &flat[first..first + chs.sectors as usize * SECTOR];
                let mut bits = encode_track(data, c as u8, h as u8, chs.sectors as usize, cells, opts.fm);
                bits.resize(len, 0);
                file_order(disk_flags, &mut bits);
                out.extend_from_slice(&bits);
                if opts.surface {
                    out.extend(std::iter::repeat_n(0, len));
                }
            }
        }
    }
    out
}

/// The cells of a track, as 86Box's d86f_prepare_pretrack and
/// d86f_prepare_sector lay a sector image's tracks out.
fn encode_track(data: &[u8], c: u8, h: u8, spt: usize, cells: u32, fm: bool) -> Vec<u8> {
    struct Cells {
        bits: Vec<u8>,
        n: usize,
        prev: u8,
        fm: bool,
    }
    impl Cells {
        fn raw(&mut self, word: u16) {
            for i in (0..16).rev() {
                let bit = ((word >> i) & 1) as u8;
                if self.n / 8 < self.bits.len() && bit != 0 {
                    self.bits[self.n / 8] |= 0x80 >> (self.n % 8);
                }
                self.n += 1;
            }
            self.prev = (word & 1) as u8;
        }
        fn byte(&mut self, b: u8) {
            let mut word = 0u16;
            for i in (0..8).rev() {
                let bit = (b >> i) & 1;
                let clock = if self.fm { 1 } else { (self.prev | bit) ^ 1 };
                word = (word << 2) | ((clock as u16) << 1) | bit as u16;
                self.prev = bit;
            }
            self.raw(word);
        }
        fn fill(&mut self, b: u8, n: usize) {
            (0..n).for_each(|_| self.byte(b));
        }
    }
    let total = cells as usize / 16;
    let mut t = Cells { bits: vec![0; (cells as usize).div_ceil(8)], n: 0, prev: 0, fm };
    let (gap_byte, sync) = if fm { (0xFF, 6) } else { (0x4E, 12) };
    let (gap0, gap1, gap2) = if fm { (40, 26, 11) } else { (80, 50, 22) };
    let pretrack = gap0 + sync + if fm { 1 } else { 4 } + gap1;
    let am = if fm { 1 } else { 4 };
    let fixed = sync + am + 4 + 2 + gap2 + sync + am + SECTOR + 2;
    let gap3 = (total.saturating_sub(pretrack) / spt).saturating_sub(fixed).clamp(1, 84);
    t.fill(gap_byte, gap0);
    t.fill(0, sync);
    if fm {
        t.raw(FM_IAM);
    } else {
        (0..3).for_each(|_| t.raw(MFM_C2));
        t.byte(0xFC);
    }
    t.fill(gap_byte, gap1);
    for r in 1..=spt {
        let mark = |t: &mut Cells, fm_word: u16, byte: u8| {
            if t.fm {
                t.raw(fm_word);
            } else {
                (0..3).for_each(|_| t.raw(MFM_A1));
                t.byte(byte);
            }
        };
        let prefix: &[u8] = if fm { &[] } else { &[0xA1, 0xA1, 0xA1] };
        t.fill(0, sync);
        mark(&mut t, FM_IDAM, 0xFE);
        let id = [c, h, r as u8, 2];
        let crc = crc16(&[prefix, &[0xFE], &id].concat());
        id.iter().chain(&crc.to_be_bytes()).for_each(|&b| t.byte(b));
        t.fill(gap_byte, gap2);
        t.fill(0, sync);
        mark(&mut t, FM_DAM, 0xFB);
        let sector = &data[(r - 1) * SECTOR..r * SECTOR];
        let crc = crc16(&[prefix, &[0xFB], sector].concat());
        sector.iter().chain(&crc.to_be_bytes()).for_each(|&b| t.byte(b));
        t.fill(gap_byte, gap3);
    }
    while t.n + 16 <= cells as usize {
        t.byte(gap_byte);
    }
    t.bits
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str, data: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rust-dos-86f-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("disk.86f");
        std::fs::write(&path, data).unwrap();
        path
    }

    fn open(path: &std::path::Path) -> D86f {
        let file = crate::hostfs::OpenOptions::new().read(true).write(true).open(path).unwrap();
        D86f::open(file).unwrap()
    }

    fn flat(chs: Chs) -> Vec<u8> {
        (0..chs.total() as usize * SECTOR).map(|i| ((i / SECTOR) as u8) ^ (i as u8).wrapping_mul(7)).collect()
    }

    const DD: Chs = Chs { cylinders: 40, heads: 2, sectors: 9 };
    const HD: Chs = Chs { cylinders: 80, heads: 2, sectors: 18 };

    fn round_trip(chs: Chs, opts: EncodeOptions) {
        let source = flat(chs);
        let image = D86f::parse(&encode(&source, chs, opts)).unwrap();
        assert_eq!(image.geometry(), chs, "{:?}", opts);
        let mut back = vec![0u8; source.len()];
        image.read_at(0, &mut back).unwrap();
        assert!(back == source, "{:?}", opts);
        assert_eq!((0..chs.total()).find_map(|s| image.sector_status(s)), None);
    }

    #[test]
    fn images_decode_to_their_sectors() {
        round_trip(HD, EncodeOptions::default());
        round_trip(DD, EncodeOptions::default());
        round_trip(Chs { cylinders: 80, heads: 2, sectors: 15 }, EncodeOptions::default());
        round_trip(Chs { cylinders: 80, heads: 2, sectors: 36 }, EncodeOptions::default());
        round_trip(Chs { cylinders: 40, heads: 1, sectors: 8 }, EncodeOptions::default());
        round_trip(Chs { cylinders: 40, heads: 1, sectors: 5 }, EncodeOptions { fm: true, ..Default::default() });
        round_trip(HD, EncodeOptions { reverse: true, surface: true, ..Default::default() });
        round_trip(HD, EncodeOptions { extra_cells: 64, ..Default::default() });
        round_trip(DD, EncodeOptions { thin: true, ..Default::default() });
    }

    #[test]
    fn writes_go_back_into_the_bitstream() {
        for opts in [
            EncodeOptions { surface: true, reverse: true, ..Default::default() },
            EncodeOptions { fm: true, ..Default::default() },
            EncodeOptions { thin: true, ..Default::default() },
        ] {
            let chs = if opts.fm { Chs { cylinders: 40, heads: 2, sectors: 5 } } else { DD };
            let mut bytes = encode(&flat(chs), chs, opts);
            // Weak bits over the start of cylinder 3, head 1's track.
            let weak = opts.surface.then(|| {
                let image = D86f::parse(&bytes).unwrap();
                let track = image.tracks[3 * 2 + 1].as_ref().unwrap().borrow();
                let at = track.at as usize + track.header_len + track.bits.len();
                bytes[at..at + track.bits.len()].fill(0xFF);
                (at, track.bits.len())
            });
            let path = scratch(&format!("write{}", opts.fm as u8 + 2 * opts.thin as u8), &bytes);
            let image = open(&path);
            let lba = (3 * 2 + 1) * chs.sectors as u64 + 2;
            let mut sector = [0x5Au8; 1024];
            sector[700] = 0xC3;
            image.write_at(lba * 512, &sector).unwrap();

            let again = open(&path);
            assert_eq!(std::fs::metadata(&path).unwrap().len(), bytes.len() as u64);
            let mut back = [0u8; 1024];
            again.read_at(lba * 512, &mut back).unwrap();
            assert_eq!(back, sector);
            assert_eq!(again.sector_status(lba), None);
            assert_eq!(again.sector_status(lba + 1), None);
            let mut before = [0u8; 512];
            again.read_at((lba - 1) * 512, &mut before).unwrap();
            assert_eq!(before[..], flat(chs)[(lba as usize - 1) * 512..lba as usize * 512]);
            if let Some((at, len)) = weak {
                let file = std::fs::read(&path).unwrap();
                assert!(file[at..at + len].contains(&0), "the written cells aren't weak any more");
                assert!(file[at..at + len].contains(&0xFF));
            }
        }
    }

    #[test]
    fn a_damaged_sector_is_a_crc_error() {
        let mut bytes = encode(&flat(HD), HD, EncodeOptions::default());
        let field = D86f::parse(&bytes).unwrap().data_field_at(4).unwrap();
        bytes[field + 100] ^= 0x10;
        let image = D86f::parse(&bytes).unwrap();
        assert_eq!(image.sector_status(4), Some(STATUS_CRC_ERROR));
        assert_eq!(image.sector_status(3), None);
    }

    #[test]
    fn bad_images_are_refused() {
        let good = encode(&flat(DD), DD, EncodeOptions::default());
        assert!(D86f::parse(&good[..100]).is_err());
        let mut old = good.clone();
        old[4] = 0x0B;
        assert!(D86f::parse(&old).err().unwrap().contains("version"));
        let mut zoned = good.clone();
        zoned[7] |= 0x01;
        assert!(D86f::parse(&zoned).err().unwrap().contains("zoned"));
        let mut empty = good.clone();
        empty[8..12].fill(0);
        assert!(D86f::parse(&empty).err().unwrap().contains("track 0"));
        let protected = encode(&flat(DD), DD, EncodeOptions { write_protect: true, ..Default::default() });
        let image = D86f::parse(&protected).unwrap();
        assert!(image.write_protected());
        assert!(image.write_at(0, &[0; 512]).is_err());
    }
}
