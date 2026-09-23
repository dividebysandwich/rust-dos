//! A CD image: its tracks laid out on the disc the way the CUE sheet
//! describes them, and sector reads from the files behind them.

use super::cue::{parse_cue, FileFormat, TrackMode};
use super::{Extent, DATA_SECTOR, RAW_SECTOR};
use std::cell::RefCell;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// A track as the disc has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub number: u8,
    pub mode: TrackMode,
    /// First sector of the pregap before the track (as `start` if none).
    pub pregap_start: u32,
    /// First sector of the track proper (INDEX 01).
    pub start: u32,
    /// First sector after the track, and after its postgap.
    pub end: u32,
    /// First sector that is in the file; those before it are silence.
    data_start: u32,
    /// First sector after those in the file.
    data_end: u32,
    file: usize,
    /// Byte offset of sector `data_start` in the file.
    offset: u64,
}

impl Track {
    pub fn is_audio(&self) -> bool {
        self.mode.is_audio()
    }
}

/// A file that holds track data.
struct Backing {
    file: RefCell<File>,
    /// Where the sector data starts (after a WAVE header).
    data_offset: u64,
    /// Bytes of sector data.
    len: u64,
    /// Audio samples are big-endian.
    swap: bool,
}

pub struct CdImage {
    path: PathBuf,
    files: Vec<Backing>,
    tracks: Vec<Track>,
}

/// The sync pattern that starts every raw data sector.
const SYNC: [u8; 12] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

impl CdImage {
    /// Open a CD image: a CUE sheet, or a bare image of one data track
    /// (.iso, .bin, .img), whose sector format is found from where the
    /// ISO 9660 volume descriptor is.
    pub fn open(path: &Path) -> Result<Self, String> {
        let is_cue = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cue"));
        if is_cue {
            Self::open_cue(path)
        } else {
            Self::open_bare(path)
        }
    }

    fn open_cue(path: &Path) -> Result<Self, String> {
        let text = fs::read(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let sheet = parse_cue(&String::from_utf8_lossy(&text))?;
        let dir = path.parent().unwrap_or(Path::new("."));

        let mut image = CdImage { path: path.to_path_buf(), files: Vec::new(), tracks: Vec::new() };
        // Sector of the disc where the next file starts.
        let mut next = 0u32;
        for cue_file in &sheet.files {
            let file_path = find_file(dir, &cue_file.name)?;
            let backing = Backing::open(&file_path, cue_file.format)?;
            let file = image.files.len();
            // Sector of the disc where this file's sector 0 is, moved on by
            // the gaps that aren't in the file.
            let mut base = next;
            let mut offset = 0u64;
            for (i, t) in cue_file.tracks.iter().enumerate() {
                base += t.pregap;
                let size = t.mode.sector_size();
                let first = t.index0.unwrap_or(t.index1).min(t.index1);
                let last = match cue_file.tracks.get(i + 1) {
                    Some(n) => n.index0.unwrap_or(n.index1),
                    None => first + ((backing.len.saturating_sub(offset)) / size) as u32,
                };
                if last < first {
                    return Err(format!("track {} ends before it starts", t.number));
                }
                let data_start = base + first;
                let data_end = base + last;
                image.tracks.push(Track {
                    number: t.number,
                    mode: t.mode,
                    pregap_start: data_start - t.pregap,
                    start: base + t.index1,
                    end: data_end + t.postgap,
                    data_start,
                    data_end,
                    file,
                    offset,
                });
                offset += (last - first) as u64 * size;
                base += t.postgap;
                next = data_end + t.postgap;
            }
            image.files.push(backing);
        }
        Ok(image)
    }

    fn open_bare(path: &Path) -> Result<Self, String> {
        let backing = Backing::open(path, FileFormat::Binary)?;
        // The Primary Volume Descriptor is sector 16: "CD001" at byte 1 of
        // its user data.
        let layouts = [TrackMode::Mode1_2048, TrackMode::Mode1_2352, TrackMode::Mode2_2352, TrackMode::Mode2_2336];
        let mut probe = [0u8; 5];
        let mode = layouts
            .into_iter()
            .find(|mode| {
                let at = 16 * mode.sector_size() + mode.data_offset() + 1;
                backing.read_at(at, &mut probe).is_ok() && &probe == b"CD001"
            })
            .ok_or_else(|| format!("{} is not an ISO 9660 CD image", path.display()))?;
        let sectors = (backing.len / mode.sector_size()) as u32;
        let track = Track {
            number: 1,
            mode,
            pregap_start: 0,
            start: 0,
            end: sectors,
            data_start: 0,
            data_end: sectors,
            file: 0,
            offset: 0,
        };
        Ok(CdImage { path: path.to_path_buf(), files: vec![backing], tracks: vec![track] })
    }

    /// The file the image was opened from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// First sector after the last track.
    pub fn leadout(&self) -> u32 {
        self.tracks.last().map_or(0, |t| t.end)
    }

    /// The track a sector belongs to, pregap included.
    pub fn track_at(&self, lba: u32) -> Option<&Track> {
        self.tracks.iter().find(|t| (t.pregap_start..t.end).contains(&lba))
    }

    /// The first data track, where the file system is.
    pub fn data_track(&self) -> Option<&Track> {
        self.tracks.iter().find(|t| !t.is_audio())
    }

    /// `len` bytes of sector `lba` from `skip` on, as the file has them;
    /// zeros for the parts of a gap that aren't in the file.
    fn read_stored(&self, track: &Track, lba: u32, skip: u64, buf: &mut [u8]) -> io::Result<()> {
        if !(track.data_start..track.data_end).contains(&lba) {
            buf.fill(0);
            return Ok(());
        }
        let at = track.offset + (lba - track.data_start) as u64 * track.mode.sector_size() + skip;
        self.files[track.file].read_at(at, buf)
    }

    /// The 2048 bytes of user data of data sector `lba`.
    pub fn read_data(&self, lba: u32, buf: &mut [u8; DATA_SECTOR]) -> io::Result<()> {
        let track = self.track_at(lba).filter(|t| !t.is_audio()).ok_or_else(out_of_range)?;
        self.read_stored(track, lba, track.mode.data_offset(), buf)
    }

    /// Up to `buf.len()` bytes of a file on the disc from byte `pos` on.
    /// Returns how many there were.
    pub fn read_extent(&self, extent: &Extent, pos: u64, buf: &mut [u8]) -> io::Result<usize> {
        let size = extent.size as u64;
        if pos >= size {
            return Ok(0);
        }
        let len = buf.len().min((size - pos) as usize);
        let mut sector = [0u8; DATA_SECTOR];
        let mut done = 0;
        while done < len {
            let at = pos + done as u64;
            let lba = extent.lba + (at / DATA_SECTOR as u64) as u32;
            let skip = (at % DATA_SECTOR as u64) as usize;
            let n = (DATA_SECTOR - skip).min(len - done);
            self.read_data(lba, &mut sector)?;
            buf[done..done + n].copy_from_slice(&sector[skip..skip + n]);
            done += n;
        }
        Ok(len)
    }

    /// The whole 2352 bytes of sector `lba`. Sectors the image stores
    /// without sync and header get them, as a drive reads them off the disc
    /// (the error correction codes stay zero).
    pub fn read_raw(&self, lba: u32, buf: &mut [u8; RAW_SECTOR]) -> io::Result<()> {
        let track = self.track_at(lba).ok_or_else(out_of_range)?;
        match track.mode {
            TrackMode::Audio | TrackMode::Mode1_2352 | TrackMode::Mode2_2352 => {
                self.read_stored(track, lba, 0, buf)?;
                if track.is_audio() && self.files[track.file].swap {
                    swap_samples(buf);
                }
                Ok(())
            }
            TrackMode::Mode1_2048 | TrackMode::Mode2_2336 => {
                buf.fill(0);
                buf[..12].copy_from_slice(&SYNC);
                let (m, s, f) = super::lba_to_msf(lba);
                let bcd = |v: u8| (v / 10) << 4 | v % 10;
                buf[12..16].copy_from_slice(&[bcd(m), bcd(s), bcd(f), 0]);
                if track.mode == TrackMode::Mode1_2048 {
                    buf[15] = 1;
                    self.read_stored(track, lba, 0, &mut buf[16..16 + DATA_SECTOR])
                } else {
                    buf[15] = 2;
                    self.read_stored(track, lba, 0, &mut buf[16..])
                }
            }
        }
    }

    /// Sector `lba` of an audio track as stereo samples: 588 frames. Data
    /// sectors play as silence.
    pub fn read_audio(&self, lba: u32, out: &mut Vec<(i16, i16)>) -> io::Result<()> {
        let mut raw = [0u8; RAW_SECTOR];
        match self.track_at(lba) {
            Some(t) if t.is_audio() => self.read_raw(lba, &mut raw)?,
            _ => {}
        }
        out.extend(raw.chunks_exact(4).map(|c| {
            (i16::from_le_bytes([c[0], c[1]]), i16::from_le_bytes([c[2], c[3]]))
        }));
        Ok(())
    }
}

impl Backing {
    fn open(path: &Path, format: FileFormat) -> Result<Self, String> {
        let error = |e: io::Error| format!("{}: {}", path.display(), e);
        let mut file = File::open(path).map_err(error)?;
        let total = file.metadata().map_err(error)?.len();
        let (data_offset, len) = match format {
            FileFormat::Wave => wave_data(&mut file).map_err(|e| format!("{}: {}", path.display(), e))?,
            _ => (0, total),
        };
        Ok(Backing { file: RefCell::new(file), data_offset, len, swap: format == FileFormat::Motorola })
    }

    /// Fill `buf` from byte `at` of the sector data; past the end of the
    /// file reads zeros.
    fn read_at(&self, at: u64, buf: &mut [u8]) -> io::Result<()> {
        buf.fill(0);
        if at >= self.len {
            return Ok(());
        }
        let n = buf.len().min((self.len - at) as usize);
        let mut file = self.file.borrow_mut();
        file.seek(SeekFrom::Start(self.data_offset + at))?;
        file.read_exact(&mut buf[..n])
    }
}

fn out_of_range() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "sector out of range")
}

fn swap_samples(buf: &mut [u8]) {
    for pair in buf.chunks_exact_mut(2) {
        pair.swap(0, 1);
    }
}

/// Where the samples of a WAVE file are, and how many bytes. Only CD
/// audio fits a CD track: PCM, 2 channels, 16 bits, 44.1 kHz.
fn wave_data(file: &mut File) -> Result<(u64, u64), String> {
    let mut header = [0u8; 12];
    file.read_exact(&mut header).map_err(|e| e.to_string())?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("not a WAVE file".to_string());
    }
    let mut at = 12u64;
    let mut format_ok = false;
    loop {
        let mut chunk = [0u8; 8];
        file.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
        file.read_exact(&mut chunk).map_err(|_| "no data chunk".to_string())?;
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as u64;
        match &chunk[0..4] {
            b"fmt " => {
                let mut fmt = [0u8; 16];
                file.read_exact(&mut fmt).map_err(|e| e.to_string())?;
                let word = |i: usize| u16::from_le_bytes([fmt[i], fmt[i + 1]]);
                let rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
                format_ok = word(0) == 1 && word(2) == 2 && rate == 44_100 && word(14) == 16;
            }
            b"data" => {
                if !format_ok {
                    return Err("only 16-bit stereo 44.1 kHz PCM WAVE files can be CD audio".to_string());
                }
                return Ok((at + 8, size));
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
}

/// A file named in a CUE sheet: next to the sheet, by the name as written
/// or by any case of it (sheets made on Windows rarely match the case).
fn find_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    // Only the file name counts; sheets can carry the paths of whoever
    // made them.
    let name = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let path = dir.join(name);
    if path.is_file() {
        return Ok(path);
    }
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .find(|p| p.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(name)))
        .ok_or_else(|| format!("{} not found", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from("target/test_cdimage").join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Raw 2352-byte sectors whose bytes all hold `fill`.
    fn sectors(count: usize, fill: u8) -> Vec<u8> {
        vec![fill; count * RAW_SECTOR]
    }

    #[test]
    fn tracks_in_one_file_with_a_pregap_in_it() {
        let dir = scratch("one_file");
        let mut data = sectors(20, 1);
        data.extend(sectors(10, 2));
        fs::write(dir.join("disc.bin"), data).unwrap();
        fs::write(
            dir.join("disc.cue"),
            "FILE \"DISC.BIN\" BINARY\n TRACK 1 MODE1/2352\n  INDEX 1 00:00:00\n \
             TRACK 2 AUDIO\n  INDEX 0 00:00:18\n  INDEX 1 00:00:20\n",
        )
        .unwrap();
        let image = CdImage::open(&dir.join("disc.cue")).unwrap();
        let tracks = image.tracks();
        assert_eq!((tracks[0].start, tracks[0].end), (0, 18));
        assert_eq!((tracks[1].pregap_start, tracks[1].start, tracks[1].end), (18, 20, 30));
        assert_eq!(image.leadout(), 30);
        assert_eq!(image.track_at(19).unwrap().number, 2);

        let mut samples = Vec::new();
        image.read_audio(25, &mut samples).unwrap();
        assert_eq!(samples.len(), 588);
        assert_eq!(samples[0], (0x0202, 0x0202));
        let mut data = [0u8; DATA_SECTOR];
        image.read_data(3, &mut data).unwrap();
        assert!(data.iter().all(|&b| b == 1));
        assert!(image.read_data(25, &mut data).is_err(), "audio isn't data");
    }

    #[test]
    fn files_per_track_with_gaps_outside_them() {
        let dir = scratch("per_track");
        fs::write(dir.join("t1.bin"), vec![7u8; 10 * 2048]).unwrap();
        fs::write(dir.join("t2.bin"), sectors(5, 3)).unwrap();
        fs::write(
            dir.join("disc.cue"),
            "FILE \"t1.bin\" BINARY\n TRACK 01 MODE1/2048\n  INDEX 01 00:00:00\n\
             FILE \"T2.BIN\" MOTOROLA\n TRACK 02 AUDIO\n  PREGAP 00:02:00\n  INDEX 01 00:00:00\n",
        )
        .unwrap();
        let image = CdImage::open(&dir.join("disc.cue")).unwrap();
        let t2 = &image.tracks()[1];
        assert_eq!((t2.pregap_start, t2.start, t2.end), (10, 160, 165));
        let mut raw = [0u8; RAW_SECTOR];
        image.read_raw(100, &mut raw).unwrap();
        assert!(raw.iter().all(|&b| b == 0), "pregap silence");
        image.read_raw(4, &mut raw).unwrap();
        assert_eq!(&raw[..12], &SYNC);
        assert_eq!(&raw[12..16], &[0x00, 0x02, 0x04, 0x01]);
        assert_eq!(raw[16], 7);
    }

    #[test]
    fn bare_images_are_probed_for_their_sector_format() {
        let dir = scratch("bare");
        let mut raw = sectors(17, 0);
        raw[16 * RAW_SECTOR + 16 + 1..16 * RAW_SECTOR + 16 + 6].copy_from_slice(b"CD001");
        fs::write(dir.join("game.bin"), &raw).unwrap();
        let image = CdImage::open(&dir.join("game.bin")).unwrap();
        assert_eq!(image.tracks()[0].mode, TrackMode::Mode1_2352);
        assert_eq!(image.leadout(), 17);

        fs::write(dir.join("junk.iso"), vec![0u8; 40 * 2048]).unwrap();
        assert!(CdImage::open(&dir.join("junk.iso")).is_err());
    }
}
