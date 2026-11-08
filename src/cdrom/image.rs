//! A CD image: its tracks laid out on the disc the way the CUE sheet,
//! CHD file or BlindWrite descriptor describes them, and sector and
//! subchannel reads from the files behind them.

use super::cue::{parse_cue, CueSheet, FileFormat, TrackMode};
use super::{DATA_SECTOR, Extent, RAW_SECTOR, SYNC};
use crate::diskimage::MemoryImage;
use std::cell::RefCell;
use crate::hostfs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// A track as the disc has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub number: u8,
    pub mode: TrackMode,
    /// The session the track is in, from 1.
    pub session: u8,
    /// The control bits of its TOC entry and Q subchannel: 4 for data,
    /// 2 for digital copy permitted, 1 for pre-emphasis.
    pub control: u8,
    /// First sector of the pregap before the track (as `start` if none).
    pub pregap_start: u32,
    /// First sector of the track proper (INDEX 01).
    pub start: u32,
    /// First sector after the track, and after its postgap.
    pub end: u32,
    /// Where its sectors are stored, in order; those in none of them are
    /// silence.
    spans: Vec<Span>,
}

/// Sectors of a track stored one after the other in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Span {
    /// The first sector, and the first after them.
    start: u32,
    end: u32,
    file: usize,
    /// Byte offset of sector `start` in the file.
    offset: u64,
    /// Bytes from one sector to the next.
    stride: u64,
    /// The subchannel stored after each sector's 2352 bytes.
    subcode: Subcode,
}

impl Span {
    /// Sectors from `start` on, `stride` bytes apart, from byte `offset`
    /// of `file`, without subchannel.
    fn new(start: u32, end: u32, file: usize, offset: u64, stride: u64) -> Self {
        Span {
            start,
            end,
            file,
            offset,
            stride,
            subcode: Subcode::None,
        }
    }
}

/// What subchannel an image keeps with each sector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Subcode {
    None,
    /// 96 bytes of P-W, each channel's 12 after the one before.
    Linear,
    /// 16 bytes: the 12 of Q, then 4 more.
    Q,
}

impl Subcode {
    fn bytes(self) -> u64 {
        match self {
            Subcode::None => 0,
            Subcode::Linear => 96,
            Subcode::Q => 16,
        }
    }
}

/// Where the Q subchannel of a sector says it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QPosition {
    /// The control bits (`Track::control`).
    pub control: u8,
    pub track: u8,
    /// 0 in a pregap, then 1 and up.
    pub index: u8,
    /// Sectors into the track, or in a pregap, left after this one until
    /// it starts: the pregap's last sector has 0, as its first sector
    /// after the track's start does (ECMA-130, 22.3.3.3).
    pub relative: u32,
    /// The sector.
    pub absolute: u32,
}

impl Track {
    /// A track of session 1 with the usual control bits, its sectors in
    /// `data`.
    fn stored(
        number: u8,
        mode: TrackMode,
        pregap_start: u32,
        start: u32,
        end: u32,
        data: Span,
    ) -> Self {
        let control = if mode.is_audio() { 0 } else { 4 };
        Track {
            number,
            mode,
            session: 1,
            control,
            pregap_start,
            start,
            end,
            spans: vec![data],
        }
    }

    pub fn is_audio(&self) -> bool {
        self.mode.is_audio()
    }

    /// Where sector `lba` is stored, if it is.
    fn span(&self, lba: u32) -> Option<&Span> {
        self.spans.iter().find(|s| (s.start..s.end).contains(&lba))
    }
}

/// A file that holds track data.
struct Backing {
    source: Source,
    /// Where the sector data starts (after a WAVE header).
    data_offset: u64,
    /// Bytes of sector data.
    len: u64,
    /// Audio samples are big-endian.
    swap: bool,
}

/// Where a track file's bytes are.
enum Source {
    File(RefCell<File>),
    /// In memory, as the browser build has its CD images.
    Memory(MemoryImage),
    /// Decoded from a compressed audio file as they are read.
    #[cfg(feature = "cdaudio")]
    Decoded(RefCell<super::decoded::DecodedTrack>),
    /// A disc made from a host folder, its files read from the host.
    Folder(super::folder::FolderImage, RefCell<super::folder::OpenFiles>),
    /// The frames of a CHD file, every track's.
    #[cfg(feature = "chd")]
    Chd(RefCell<super::chd::ChdDisc>),
}

pub struct CdImage {
    path: PathBuf,
    files: Vec<Backing>,
    tracks: Vec<Track>,
    /// The media catalog number, where the image keeps it.
    mcn: Option<String>,
    /// The data position measurement the image keeps.
    dpm: Option<super::b6t::Dpm>,
    /// The subchannel file beside the image, for the sectors whose own
    /// files keep none.
    sub_file: Option<SubFile>,
}

/// The error of reading a sector a drive can't read: one whose EDC
/// doesn't match its data (`super::edc_fails`).
#[derive(Debug)]
pub struct UnreadableSector(pub u32);

impl std::fmt::Display for UnreadableSector {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "sector {} can't be read: its EDC doesn't match its data",
            self.0
        )
    }
}

impl std::error::Error for UnreadableSector {}

/// Whether a read failed on a sector a drive can't read, rather than on
/// the image file or an address past the disc.
pub fn is_unreadable(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|e| e.is::<UnreadableSector>())
}

/// Fail reading `raw`, sector `lba`, if a drive couldn't read it.
fn check_readable(lba: u32, raw: &[u8; RAW_SECTOR]) -> io::Result<()> {
    match super::edc_fails(raw) {
        true => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            UnreadableSector(lba),
        )),
        false => Ok(()),
    }
}

impl CdImage {
    /// Open a CD image: a CUE sheet, a CHD file, a BlindWrite or CloneCD
    /// descriptor, or a bare image of one data track (.iso, .bin, .img),
    /// whose sector format is found from where the ISO 9660 or High Sierra
    /// volume descriptor is.
    pub fn open(path: &Path) -> Result<Self, String> {
        if is_b6t(path) {
            Self::open_b6t(path)
        } else if let Some(sheet) = sheet(path) {
            Self::open_cue(path, sheet?)
        } else if is_chd(path) {
            Self::open_chd(path)
        } else {
            Self::open_bare(path)
        }
    }

    fn open_cue(path: &Path, sheet: CueSheet) -> Result<Self, String> {
        let dir = dir_of(path);
        let mut image = CdImage::new(path, Vec::new(), Vec::new());
        // Sector of the disc where the next file starts.
        let mut next = 0u32;
        for cue_file in &sheet.files {
            let file_path = find_file(dir, &cue_file.name)?;
            let backing = Backing::open(&file_path, cue_file.format)?;
            let file = image.files.len();
            // Sector of the disc where this file's sector 0 is, moved on by
            // the gaps that aren't in the file.
            let mut base = next;
            // The first two seconds of track 1's pregap are before the
            // disc's sector 0, where no read reaches. A data track 1 starts
            // at sector 0, so an image that keeps its pregap (INDEX 00
            // 00:00:00, INDEX 01 00:02:00, as BlindWrite's cue sheets give
            // it) has the disc start that far into the file. An audio
            // track 1 can start later, after hidden audio in its pregap:
            // sheets for those (INDEX 00 00:00:00, INDEX 01 03:22:70) count
            // the file from sector 0, so its sectors before INDEX 01 stay
            // on the disc.
            let first_file = image.tracks.is_empty();
            let (lead, mut offset) = match cue_file.tracks.first() {
                Some(t) if first_file && !t.mode.is_audio() => {
                    (t.index1, t.index1 as u64 * t.mode.sector_size())
                }
                _ => (0, 0),
            };
            let at = |index: u32| index.saturating_sub(lead);
            for (i, t) in cue_file.tracks.iter().enumerate() {
                // Track 1's PREGAP is silence before INDEX 01, of which
                // the first two seconds are before sector 0.
                let pregap = if first_file && i == 0 {
                    t.pregap.saturating_sub(super::LBA_OFFSET)
                } else {
                    t.pregap
                };
                base += pregap;
                let size = t.mode.sector_size();
                let first = t.index0.map_or(at(t.index1), at).min(at(t.index1));
                let last = match cue_file.tracks.get(i + 1) {
                    Some(n) => n.index0.map_or(at(n.index1), at),
                    None => first + ((backing.len.saturating_sub(offset)) / size) as u32,
                };
                if last < first {
                    return Err(format!("track {} ends before it starts", t.number));
                }
                let data_start = base + first;
                let data_end = base + last;
                let data = Span::new(data_start, data_end, file, offset, size);
                let pregap_start = data_start - pregap;
                let (start, end) = (base + at(t.index1), data_end + t.postgap);
                let mut track = Track::stored(t.number, t.mode, pregap_start, start, end, data);
                track.control |= t.flags;
                image.tracks.push(track);
                offset += (last - first) as u64 * size;
                base += t.postgap;
                next = data_end + t.postgap;
            }
            image.files.push(backing);
        }
        image.mcn = sheet.catalog;
        let names = sheet.files.iter().map(|f| f.name.as_str());
        image.sub_file = sub_file(path, names).map(|(_, sub)| sub);
        Ok(image)
    }

    /// The sessions and tracks a BlindWrite descriptor lists, their
    /// sectors in its data files, with the subchannel those keep and the
    /// DPM data of the `.bwa` file beside it or of the descriptor.
    fn open_b6t(path: &Path) -> Result<Self, String> {
        use super::b6t::{self, TrackType};
        let error = |e: String| format!("{}: {}", path.display(), e);
        let data = hostfs::read(path).map_err(|e| error(e.to_string()))?;
        let disc = b6t::parse(&data).map_err(error)?;
        let dir = dir_of(path);
        let mut image = CdImage::new(path, Vec::new(), Vec::new());
        image.mcn = disc.mcn.clone();
        // The file of each data block, each file opened once.
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut files = Vec::new();
        for block in &disc.blocks {
            let found = find_file(dir, &block.file)?;
            let file = match paths.iter().position(|p| *p == found) {
                Some(file) => file,
                None => {
                    image.files.push(Backing::open(&found, FileFormat::Binary)?);
                    paths.push(found);
                    paths.len() - 1
                }
            };
            files.push(file);
        }
        for session in &disc.sessions {
            for t in &session.tracks {
                let sector = |at: i64| {
                    u32::try_from(at).map_err(|_| {
                        error(format!("track {} isn't where a CD's sectors are", t.number))
                    })
                };
                // The first track's pregap starts at -150, where no read
                // reaches.
                let (first, end) = (t.start.max(0), t.start + t.sectors as i64);
                let mut spans = Vec::new();
                let mut sizes = Vec::new();
                for (block, &file) in disc.blocks.iter().zip(&files) {
                    let (from, to) = (
                        first.max(block.start),
                        end.min(block.start + block.sectors as i64),
                    );
                    if from < to {
                        let offset = block.offset + (from - block.start) as u64 * block.sector_size;
                        let mut span =
                            Span::new(sector(from)?, sector(to)?, file, offset, block.sector_size);
                        span.subcode = match block.subchannel() {
                            96 => Subcode::Linear,
                            16 => Subcode::Q,
                            _ => Subcode::None,
                        };
                        sizes.push(block.sector_size - block.subchannel());
                        spans.push(span);
                    }
                }
                let size = match sizes[..] {
                    [] => {
                        return Err(error(format!(
                            "track {} is in none of its data files",
                            t.number
                        )));
                    }
                    [size, ..] if sizes.iter().all(|&s| s == size) => size,
                    _ => {
                        return Err(error(format!(
                            "track {} has sectors of different sizes",
                            t.number
                        )));
                    }
                };
                let mode = match (t.kind, size) {
                    (TrackType::Audio, 2352) => TrackMode::Audio,
                    (TrackType::Mode1 | TrackType::Dvd, 2352) => TrackMode::Mode1_2352,
                    (TrackType::Mode1 | TrackType::Dvd | TrackType::Mode2Form1, 2048) => {
                        TrackMode::Mode1_2048
                    }
                    (TrackType::Mode2 | TrackType::Mode2Form1 | TrackType::Mode2Form2, 2336) => {
                        TrackMode::Mode2_2336
                    }
                    (TrackType::Mode2 | TrackType::Mode2Form1 | TrackType::Mode2Form2, 2352) => {
                        TrackMode::Mode2_2352
                    }
                    (kind, size) => {
                        return Err(error(format!(
                            "track {} is {:?} in sectors of {} bytes, which isn't read",
                            t.number, kind, size
                        )));
                    }
                };
                image.tracks.push(Track {
                    number: t.number,
                    mode,
                    session: session.number as u8,
                    control: t.control,
                    pregap_start: sector(first)?,
                    start: sector(t.start + t.pregap as i64)?,
                    end: sector(end)?,
                    spans,
                });
            }
        }
        image.tracks.sort_by_key(|t| t.number);
        if image.tracks.is_empty() {
            return Err(error("it has no tracks".into()));
        }
        image.dpm = bwa_file(path)
            .and_then(|p| hostfs::read(p).ok())
            .and_then(|d| b6t::parse_bwa(&d))
            .or(disc.dpm);
        Ok(image)
    }

    fn open_bare(path: &Path) -> Result<Self, String> {
        let mut image = Self::bare(path, Backing::open(path, FileFormat::Binary)?)?;
        image.sub_file = sub_file(path, std::iter::empty()).map(|(_, sub)| sub);
        Ok(image)
    }

    #[cfg(feature = "chd")]
    fn open_chd(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        Self::chd(path, Box::new(file))
    }

    #[cfg(not(feature = "chd"))]
    fn open_chd(path: &Path) -> Result<Self, String> {
        Err(format!("{}: this rust-dos can't read CHD images", path.display()))
    }

    /// The tracks of a CHD file one after the other, each with its pregap
    /// and postgap, as a CUE sheet would lay them out.
    #[cfg(feature = "chd")]
    fn chd(path: &Path, stream: Box<dyn super::chd::Stream>) -> Result<Self, String> {
        use super::chd::{ChdDisc, FRAME};
        let disc = ChdDisc::open(stream).map_err(|e| format!("{}: {}", path.display(), e))?;
        let mut tracks = Vec::new();
        let (mut next, mut frame) = (0u32, 0u64);
        for t in disc.tracks() {
            let start = next + t.pregap;
            let data_start = if t.pregap_stored { next } else { start };
            let data_end = data_start + t.frames;
            let data = Span::new(data_start, data_end, 0, frame * FRAME, FRAME);
            tracks.push(Track::stored(t.number, t.mode, next, start, data_end + t.postgap, data));
            frame += t.frames.next_multiple_of(4) as u64;
            next = data_end + t.postgap;
        }
        let len = disc.byte_len();
        let backing = Backing { source: Source::Chd(RefCell::new(disc)), data_offset: 0, len, swap: true };
        Ok(CdImage::new(path, vec![backing], tracks))
    }

    fn new(path: &Path, files: Vec<Backing>, tracks: Vec<Track>) -> Self {
        CdImage {
            path: path.to_path_buf(),
            files,
            tracks,
            mcn: None,
            dpm: None,
            sub_file: None,
        }
    }

    /// An image held in memory, as `open` makes of a file with those bytes
    /// (a bare image of one data track, or a CHD file); `name` names it in
    /// messages.
    pub fn from_memory(name: &str, data: MemoryImage) -> Result<Self, String> {
        #[cfg(feature = "chd")]
        if is_chd_memory(&data) {
            return Self::chd(Path::new(name), Box::new(MemoryReader { data, pos: 0 }));
        }
        let len = data.len();
        Self::bare(Path::new(name), Backing { source: Source::Memory(data), data_offset: 0, len, swap: false })
    }

    /// The disc made from the host folder `root` (`folder::build`).
    pub fn from_folder(disc: super::folder::FolderImage, root: &Path) -> Result<Self, String> {
        let len = disc.sectors as u64 * DATA_SECTOR as u64;
        let source = Source::Folder(disc, RefCell::new(Default::default()));
        Self::bare(root, Backing { source, data_offset: 0, len, swap: false })
    }

    /// Whether this disc was made from a host folder.
    pub fn is_folder(&self) -> bool {
        self.files.iter().any(|f| matches!(f.source, Source::Folder(..)))
    }

    /// Whether `data` is a bare image of one data track, or a CHD file.
    pub fn probe(data: &MemoryImage) -> bool {
        is_chd_memory(data) || Self::bare_mode(data.len(), |at, buf| data.read_at(at, buf)).is_some()
    }

    /// The sector format of a bare image of `len` bytes, found from where
    /// the first volume descriptor is, which `read` fills a buffer from.
    fn bare_mode(len: u64, mut read: impl FnMut(u64, &mut [u8]) -> bool) -> Option<TrackMode> {
        // The first volume descriptor is sector 16: "CD001" at byte 1 of
        // its user data, or High Sierra's "CDROM" at byte 9.
        let layouts = [TrackMode::Mode1_2048, TrackMode::Mode1_2352, TrackMode::Mode2_2352, TrackMode::Mode2_2336];
        let mut probe = [0u8; 14];
        layouts.into_iter().find(|mode| {
            let at = 16 * mode.sector_size() + mode.data_offset();
            at + probe.len() as u64 <= len
                && read(at, &mut probe)
                && super::iso9660::Format::of(&probe).is_some()
        })
    }

    fn bare(path: &Path, backing: Backing) -> Result<Self, String> {
        let mode = Self::bare_mode(backing.len, |at, buf| backing.read_at(at, buf).is_ok())
            .ok_or_else(|| {
                format!(
                    "{} is not an ISO 9660 or High Sierra CD image",
                    path.display()
                )
            })?;
        let sectors = (backing.len / mode.sector_size()) as u32;
        let data = Span::new(0, sectors, 0, 0, mode.sector_size());
        let track = Track::stored(1, mode, 0, 0, sectors, data);
        Ok(CdImage::new(path, vec![backing], vec![track]))
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

    /// The first data track of the last session that has one, where the
    /// file system is: a disc written in several sessions has its latest
    /// volume descriptors there, and a CD Extra its audio tracks before.
    pub fn data_track(&self) -> Option<&Track> {
        let session = self
            .tracks
            .iter()
            .filter(|t| !t.is_audio())
            .map(|t| t.session)
            .max()?;
        self.tracks
            .iter()
            .find(|t| !t.is_audio() && t.session == session)
    }

    /// The disc's media catalog number, 13 digits, if the image keeps it.
    pub fn mcn(&self) -> Option<&str> {
        self.mcn.as_deref()
    }

    /// The data position measurement of the disc, if the image keeps it:
    /// where reads find their sectors (`timing::Geometry`).
    pub fn dpm(&self) -> Option<&super::b6t::Dpm> {
        self.dpm.as_ref()
    }

    /// `len` bytes of sector `lba` from `skip` on, as the file has them;
    /// zeros for the parts of a gap that aren't in the file.
    fn read_stored(&self, track: &Track, lba: u32, skip: u64, buf: &mut [u8]) -> io::Result<()> {
        let Some(span) = track.span(lba) else {
            buf.fill(0);
            return Ok(());
        };
        let at = span.offset + (lba - span.start) as u64 * span.stride + skip;
        self.files[span.file].read_at(at, buf)
    }

    /// The 2048 bytes of user data of data sector `lba`. Fails with
    /// `UnreadableSector` on a sector a drive can't read.
    pub fn read_data(&self, lba: u32, buf: &mut [u8; DATA_SECTOR]) -> io::Result<()> {
        let track = self.track_at(lba).filter(|t| !t.is_audio()).ok_or_else(out_of_range)?;
        if let TrackMode::Mode1_2352 | TrackMode::Mode2_2352 = track.mode {
            let mut raw = [0u8; RAW_SECTOR];
            self.read_stored(track, lba, 0, &mut raw)?;
            check_readable(lba, &raw)?;
            let at = track.mode.data_offset() as usize;
            buf.copy_from_slice(&raw[at..at + DATA_SECTOR]);
            return Ok(());
        }
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
    /// (the error correction codes stay zero). Fails with `UnreadableSector`
    /// on a data sector a drive can't read.
    pub fn read_raw(&self, lba: u32, buf: &mut [u8; RAW_SECTOR]) -> io::Result<()> {
        let track = self.track_at(lba).ok_or_else(out_of_range)?;
        match track.mode {
            TrackMode::Audio => {
                self.read_stored(track, lba, 0, buf)?;
                if track.span(lba).is_some_and(|s| self.files[s.file].swap) {
                    swap_samples(buf);
                }
                Ok(())
            }
            TrackMode::Mode1_2352 | TrackMode::Mode2_2352 => {
                self.read_stored(track, lba, 0, buf)?;
                check_readable(lba, buf)
            }
            TrackMode::Mode1_2048 | TrackMode::Mode2_2336 => {
                buf.fill(0);
                buf[..12].copy_from_slice(&SYNC);
                let (m, s, f) = super::lba_to_msf(lba);
                let bcd = |v: u8| ((v / 10) << 4) | (v % 10);
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
        out.extend(
            raw.as_chunks::<4>().0.iter().map(|c| (i16::from_le_bytes([c[0], c[1]]), i16::from_le_bytes([c[2], c[3]]))),
        );
        Ok(())
    }

    /// The 96 bytes of P-W subchannel of sector `lba`, each channel's 12
    /// after the one before: as the image keeps them, in its files or in
    /// a subchannel file beside it (`SubFile`), and where it keeps none
    /// (or only Q), P and Q as the disc's layout makes them, with R-W
    /// zero.
    pub fn read_subchannel(&self, lba: u32, out: &mut [u8; 96]) -> io::Result<()> {
        let track = self.track_at(lba).ok_or_else(out_of_range)?;
        out.fill(0);
        // P marks the pause before a track.
        if lba < track.start {
            out[..12].fill(0xFF);
        }
        out[12..24].copy_from_slice(&made_q(track, lba));
        if let Some(span) = track.span(lba) {
            let at = span.offset + (lba - span.start) as u64 * span.stride + span.stride
                - span.subcode.bytes();
            match span.subcode {
                Subcode::None => {}
                Subcode::Linear => return self.files[span.file].read_at(at, out),
                Subcode::Q => return self.files[span.file].read_at(at, &mut out[12..24]),
            }
        }
        match &self.sub_file {
            Some(sub) => sub.read(lba, out),
            None => Ok(()),
        }
    }

    /// Where the Q subchannel of sector `lba` says it is: the image's Q
    /// where it keeps a position there with a good CRC; else, as a drive
    /// that reads a Q it can't use reports the last good one, where the
    /// disc's layout has the sector.
    pub fn q_position(&self, lba: u32) -> Option<QPosition> {
        let track = self.track_at(lba)?;
        let mut sub = [0u8; 96];
        let stored = self
            .read_subchannel(lba, &mut sub)
            .ok()
            .and_then(|()| decode_q(&sub[12..24]));
        stored.or_else(|| decode_q(&made_q(track, lba)))
    }
}

/// A file of 96 bytes of subchannel for every sector, which dumping
/// programs write beside the image: CloneCD's and DiscImageCreator's
/// `.sub` keep each of P to W's 12 bytes after the one before, from
/// sector 0; redumper's `.subcode` keeps them raw, interleaved as a drive
/// reads them, from the lead-in on.
struct SubFile {
    backing: Backing,
    /// The sector of the file's first 96 bytes (before 0 in the lead-in).
    first: i64,
    /// Its subchannel is raw, interleaved.
    interleaved: bool,
}

/// How many places of a subchannel file `SubFile::open` looks at.
const SUB_SAMPLES: u64 = 64;

impl SubFile {
    /// The subchannel file at `path`, with its layout found from the Q of
    /// sectors spread over it: of the two layouts, and the sector its
    /// first entry would be, the one most Qs with a good CRC and a place
    /// in a track agree on, when that is more than half of them. None if
    /// its size isn't whole entries or its Qs agree on no layout.
    fn open(path: &Path) -> Option<Self> {
        let backing = Backing::open(path, FileFormat::Binary).ok()?;
        if backing.len == 0 || backing.len % 96 != 0 {
            return None;
        }
        let entries = backing.len / 96;
        let mut votes: Vec<((i64, bool), u32)> = Vec::new();
        let mut decoded = 0;
        let samples = entries.min(SUB_SAMPLES);
        for k in 0..samples {
            let entry = k * entries / samples;
            let mut sub = [0u8; 96];
            backing.read_at(entry * 96, &mut sub).ok()?;
            for interleaved in [false, true] {
                let linear = match interleaved {
                    true => super::deinterleave_subchannel(&sub),
                    false => sub,
                };
                // The lead-in's Q has track 0, and its TOC in place of the
                // sector's address.
                let Some(q) = decode_q(&linear[12..24]).filter(|q| q.track != 0) else {
                    continue;
                };
                decoded += 1;
                let layout = (q.absolute as i64 - entry as i64, interleaved);
                match votes.iter_mut().find(|(l, _)| *l == layout) {
                    Some((_, n)) => *n += 1,
                    None => votes.push((layout, 1)),
                }
            }
        }
        let ((first, interleaved), n) = votes.into_iter().max_by_key(|&(_, n)| n)?;
        (n * 2 > decoded).then_some(SubFile {
            backing,
            first,
            interleaved,
        })
    }

    /// Where sector `lba`'s entry is in the file, if it has one.
    fn entry(&self, lba: u32) -> Option<u64> {
        let entry = u64::try_from(lba as i64 - self.first).ok()?;
        (entry < self.backing.len / 96).then_some(entry * 96)
    }

    /// Sector `lba`'s subchannel into `out`, each channel's 12 bytes after
    /// the one before; `out` stays as it is for a sector the file has no
    /// entry for.
    fn read(&self, lba: u32, out: &mut [u8; 96]) -> io::Result<()> {
        let Some(at) = self.entry(lba) else {
            return Ok(());
        };
        self.backing.read_at(at, out)?;
        if self.interleaved {
            *out = super::deinterleave_subchannel(out);
        }
        Ok(())
    }
}

/// The subchannel file beside the image at `path` whose data files are
/// named `files`, and where it is: of the files there, in any case, with
/// the name of the image or of one of its files and the extension `.sub`
/// or `.subcode`, in that order, the first that `SubFile::open` takes.
fn sub_file<'a>(path: &Path, files: impl Iterator<Item = &'a str>) -> Option<(PathBuf, SubFile)> {
    let stem = |name: &str| {
        let file = name.rsplit(['\\', '/']).next()?;
        Some(Path::new(file).file_stem()?.to_string_lossy().into_owned())
    };
    let stems = path
        .file_name()
        .and_then(|n| stem(&n.to_string_lossy()))
        .into_iter()
        .chain(files.filter_map(stem));
    let mut names: Vec<String> = Vec::new();
    for s in stems {
        for ext in ["sub", "subcode"] {
            let name = format!("{}.{}", s, ext);
            if !names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                names.push(name);
            }
        }
    }
    // A sheet of many files has many names to look for: the folder is
    // listed once, for those that aren't there as they are spelt.
    let dir = dir_of(path);
    let mut listing: Option<Vec<PathBuf>> = None;
    names.iter().find_map(|name| {
        let exact = dir.join(name);
        let found = if hostfs::is_file(&exact) {
            exact
        } else {
            let listing = listing.get_or_insert_with(|| {
                hostfs::read_dir(dir)
                    .into_iter()
                    .flatten()
                    .map(|entry| entry.path)
                    .collect()
            });
            listing
                .iter()
                .find(|p| {
                    p.file_name()
                        .is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(name))
                        && hostfs::is_file(p)
                })?
                .clone()
        };
        SubFile::open(&found).map(|sub| (found, sub))
    })
}

/// `v` in binary-coded decimal.
fn bcd(v: u32) -> u8 {
    (((v / 10 % 10) << 4) | (v % 10)) as u8
}

/// CCITT's CRC-16 of `data`: polynomial x^16 + x^12 + x^5 + 1, from 0.
fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0u16, |crc, &b| {
        (0..8).fold(crc ^ (b as u16) << 8, |c, _| {
            if c & 0x8000 != 0 {
                c << 1 ^ 0x1021
            } else {
                c << 1
            }
        })
    })
}

/// The CRC the Q subchannel keeps of its first 10 bytes, inverted.
fn q_crc(q: &[u8]) -> u16 {
    !crc16(&q[..10])
}

/// The Q subchannel of sector `lba` of `track` as a pressed disc has it:
/// the position (ADR 1) and its CRC. In a pregap the relative time counts
/// down to 0 at its last sector.
fn made_q(track: &Track, lba: u32) -> [u8; 12] {
    let (index, relative) = match lba < track.start {
        true => (0, track.start - lba - 1),
        false => (1, lba - track.start),
    };
    let msf = |frames: u32| {
        [
            bcd(frames / 75 / 60),
            bcd(frames / 75 % 60),
            bcd(frames % 75),
        ]
    };
    let mut q = [0u8; 12];
    q[0] = track.control << 4 | 1;
    q[1] = bcd(track.number as u32);
    q[2] = index;
    q[3..6].copy_from_slice(&msf(relative));
    q[7..10].copy_from_slice(&msf(lba + super::LBA_OFFSET));
    let crc = q_crc(&q);
    q[10..12].copy_from_slice(&crc.to_be_bytes());
    q
}

/// The position a Q subchannel of ADR 1 gives, if its CRC is good.
fn decode_q(q: &[u8]) -> Option<QPosition> {
    if q[0] & 0x0F != 1 || u16::from_be_bytes([q[10], q[11]]) != q_crc(q) {
        return None;
    }
    let from_bcd =
        |b: u8| (b >> 4 < 10 && b & 0x0F < 10).then_some((b >> 4) as u32 * 10 + (b & 0x0F) as u32);
    let frames = |m: &[u8]| Some((from_bcd(m[0])? * 60 + from_bcd(m[1])?) * 75 + from_bcd(m[2])?);
    Some(QPosition {
        control: q[0] >> 4,
        track: from_bcd(q[1])? as u8,
        index: from_bcd(q[2])? as u8,
        relative: frames(&q[3..6])?,
        absolute: frames(&q[7..10])?.checked_sub(super::LBA_OFFSET)?,
    })
}

impl Backing {
    fn open(path: &Path, format: FileFormat) -> Result<Self, String> {
        let error = |e: io::Error| format!("{}: {}", path.display(), e);
        let mut file = File::open(path).map_err(error)?;
        let total = file.len().map_err(error)?;
        let (data_offset, len) = match format {
            // A WAVE file of CD audio is read as it is; any other (another
            // rate, or a compressed file the sheet calls WAVE, as many do)
            // is decoded.
            FileFormat::Wave => match wave_data(&mut file) {
                Ok((WaveFormat { channels: 2, rate: 44_100, bits: 16 }, offset, size)) => (offset, size),
                _ => return Self::decoded(path),
            },
            FileFormat::Compressed => return Self::decoded(path),
            _ => (0, total),
        };
        Ok(Backing { source: Source::File(RefCell::new(file)), data_offset, len, swap: format == FileFormat::Motorola })
    }

    /// A compressed audio file, decoded to CD audio as it is read. Its
    /// length is rounded up to whole sectors, the last one padded with
    /// silence.
    #[cfg(feature = "cdaudio")]
    fn decoded(path: &Path) -> Result<Self, String> {
        let track = super::decoded::DecodedTrack::open(path)?;
        let len = track.byte_len().next_multiple_of(RAW_SECTOR as u64);
        Ok(Backing { source: Source::Decoded(RefCell::new(track)), data_offset: 0, len, swap: false })
    }

    #[cfg(not(feature = "cdaudio"))]
    fn decoded(path: &Path) -> Result<Self, String> {
        Err(format!("{}: this rust-dos can't play compressed audio files", path.display()))
    }

    /// Fill `buf` from byte `at` of the sector data; past the end of the
    /// file reads zeros.
    fn read_at(&self, at: u64, buf: &mut [u8]) -> io::Result<()> {
        buf.fill(0);
        if at >= self.len {
            return Ok(());
        }
        let n = buf.len().min((self.len - at) as usize);
        let at = self.data_offset + at;
        match &self.source {
            Source::File(file) => {
                let mut file = file.borrow_mut();
                file.seek(SeekFrom::Start(at))?;
                file.read_exact(&mut buf[..n])
            }
            Source::Memory(data) => match data.read_at(at, &mut buf[..n]) {
                true => Ok(()),
                false => Err(io::ErrorKind::UnexpectedEof.into()),
            },
            #[cfg(feature = "cdaudio")]
            Source::Decoded(track) => {
                track.borrow_mut().read_at(at, &mut buf[..n]);
                Ok(())
            }
            Source::Folder(disc, open) => {
                disc.read_at(at, &mut buf[..n], &mut open.borrow_mut());
                Ok(())
            }
            #[cfg(feature = "chd")]
            Source::Chd(disc) => disc.borrow_mut().read_at(at, &mut buf[..n]),
        }
    }
}

/// A CHD file starts with these bytes.
const CHD_MAGIC: &[u8; 8] = b"MComprHD";

/// Whether `path` is a CHD file: named .chd, or starting as one does.
fn is_chd(path: &Path) -> bool {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("chd")) {
        return true;
    }
    let mut magic = [0u8; 8];
    File::open(path).is_ok_and(|mut f| f.read_exact(&mut magic).is_ok()) && &magic == CHD_MAGIC
}

/// Whether `path` is a BlindWrite descriptor: named .b5t or .b6t, or
/// starting as one does.
fn is_b6t(path: &Path) -> bool {
    if path
        .extension()
        .is_some_and(|e| ["b5t", "b6t"].iter().any(|x| e.eq_ignore_ascii_case(x)))
    {
        return true;
    }
    let mut magic = [0u8; 16];
    File::open(path).is_ok_and(|mut f| f.read_exact(&mut magic).is_ok())
        && &magic == super::b6t::SIGNATURE
}

/// The `.bwa` file of DPM data beside the BlindWrite descriptor at `path`,
/// if there is one.
fn bwa_file(path: &Path) -> Option<PathBuf> {
    let name = path.with_extension("bwa");
    find_in(dir_of(path), &name.file_name()?.to_string_lossy(), |p| {
        hostfs::is_file(p)
    })
}

/// The files the BlindWrite descriptor at `path` keeps its sectors in,
/// each found or why not, and its `.bwa` file if there is one; none if it
/// can't be read.
fn b6t_files(path: &Path) -> Vec<Result<PathBuf, String>> {
    let Some(disc) = hostfs::read(path)
        .ok()
        .and_then(|d| super::b6t::parse(&d).ok())
    else {
        return Vec::new();
    };
    // Blocks name a file once each, in whatever case and with whatever
    // folder BlindWrite wrote; the file found is what counts once.
    let mut files: Vec<Result<PathBuf, String>> = Vec::new();
    for block in &disc.blocks {
        let file = find_file(dir_of(path), &block.file);
        if !files.contains(&file) {
            files.push(file);
        }
    }
    files.extend(bwa_file(path).map(Ok));
    files
}

/// The folder of the file at `path`. A bare file name's parent is "",
/// which can't be listed to match names in any case.
fn dir_of(path: &Path) -> &Path {
    path.parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

fn is_chd_memory(data: &MemoryImage) -> bool {
    let mut magic = [0u8; 8];
    data.read_at(0, &mut magic) && &magic == CHD_MAGIC
}

/// An image held in memory read as a file.
#[cfg(feature = "chd")]
struct MemoryReader {
    data: MemoryImage,
    pos: u64,
}

#[cfg(feature = "chd")]
impl Read for MemoryReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(self.data.len().saturating_sub(self.pos) as usize);
        if !self.data.read_at(self.pos, &mut buf[..n]) {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.pos += n as u64;
        Ok(n)
    }
}

#[cfg(feature = "chd")]
impl Seek for MemoryReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let pos = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.data.len().checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = pos.ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        Ok(self.pos)
    }
}

fn out_of_range() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "sector out of range")
}

fn swap_samples(buf: &mut [u8]) {
    for pair in buf.as_chunks_mut::<2>().0 {
        pair.swap(0, 1);
    }
}

/// The format of a PCM WAVE file's samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WaveFormat {
    pub channels: u16,
    pub rate: u32,
    pub bits: u16,
}

/// The format of a PCM WAVE file's samples, where they are and how many
/// bytes.
pub fn wave_data<R: Read + Seek>(file: &mut R) -> Result<(WaveFormat, u64, u64), String> {
    let mut header = [0u8; 12];
    file.read_exact(&mut header).map_err(|e| e.to_string())?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("not a WAVE file".to_string());
    }
    let mut at = 12u64;
    let mut format = None;
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
                format = (word(0) == 1).then_some(WaveFormat { channels: word(2), rate, bits: word(14) });
            }
            b"data" => {
                let format = format.ok_or("not a PCM WAVE file")?;
                return Ok((format, at + 8, size));
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
}

/// The largest file `sheet` reads to see whether it is a CUE sheet. A
/// sheet of 99 tracks is a few KiB; a disc image is far larger.
const MAX_SHEET: u64 = 64 * 1024;

/// The CUE sheet at `path`, read and parsed, if `CdImage::open` reads it
/// as one: a `.cue`, GOG's `.ins` (`.inst` in Steam's copies) or GOG's
/// `.bdn` for BloodNet, with the error if it can't be read or parsed, or a
/// small file of any other name that parses as a sheet with a track.
/// A CloneCD descriptor (`.ccd`, or a small file that starts as one) is
/// read as the sheet of its `.img` (`ccd::parse`). None for any other
/// file.
fn sheet(path: &Path) -> Option<Result<CueSheet, String>> {
    let has = |exts: &[&str]| {
        path.extension()
            .is_some_and(|e| exts.iter().any(|c| e.eq_ignore_ascii_case(c)))
    };
    let (named, ccd) = (has(&["cue", "ins", "inst", "bdn"]), has(&["ccd"]));
    if !named && !ccd && !hostfs::metadata(path).is_ok_and(|m| !m.is_dir && m.len <= MAX_SHEET) {
        return None;
    }
    let text = match hostfs::read(path) {
        Ok(data) => String::from_utf8_lossy(&data).into_owned(),
        Err(e) => return (named || ccd).then(|| Err(format!("{}: {}", path.display(), e))),
    };
    if ccd || super::ccd::is_ccd(&text) {
        // CloneCD and DiscImageCreator name the image as the descriptor.
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let sheet = super::ccd::parse(&text, &format!("{}.img", stem));
        return Some(sheet.map_err(|e| format!("{}: {}", path.display(), e)));
    }
    let sheet = parse_cue(&text);
    (named || sheet.is_ok()).then_some(sheet)
}

/// The files a CUE sheet or a BlindWrite or CloneCD descriptor at `path`
/// keeps its tracks in, those that are there, with the subchannel or DPM
/// file beside it; none if `path` isn't one that `CdImage::open` reads as
/// one.
pub fn track_files(path: &Path) -> Vec<PathBuf> {
    if is_b6t(path) {
        return b6t_files(path).into_iter().filter_map(Result::ok).collect();
    }
    let Some(Ok(sheet)) = sheet(path) else {
        return Vec::new();
    };
    let names = sheet.files.iter().map(|f| f.name.as_str());
    sheet
        .files
        .iter()
        .filter_map(|f| find_file(dir_of(path), &f.name).ok())
        .chain(sub_file(path, names).map(|(p, _)| p))
        .collect()
}

/// Whether `path` is a CUE sheet or a BlindWrite or CloneCD descriptor
/// `CdImage::open` reads as one that names a track file that isn't there.
pub fn lacks_a_track_file(path: &Path) -> bool {
    if is_b6t(path) {
        return b6t_files(path).iter().any(Result::is_err);
    }
    let Some(Ok(sheet)) = sheet(path) else {
        return false;
    };
    sheet
        .files
        .iter()
        .any(|f| find_file(dir_of(path), &f.name).is_err())
}

/// A file named in a CUE sheet: below the sheet's folder by its relative
/// path (some sheets keep their audio tracks in a folder), else next to the
/// sheet, each part by the name as written or by any case of it (sheets
/// made on Windows rarely match the case).
fn find_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let parts: Vec<&str> = name.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
    let file = parts.last().copied().unwrap_or(name);
    // An absolute path is whoever made the sheet's: only its file name
    // counts. So does a path that climbs out of the sheet's folder with `..`.
    let relative = !name.starts_with(['\\', '/']) && !name.contains(':') && !parts.contains(&"..");
    let is_file: fn(&Path) -> bool = |p| hostfs::is_file(p);
    let is_dir: fn(&Path) -> bool = |p| hostfs::is_dir(p);
    let folders = &parts[..parts.len().saturating_sub(1)];
    let below_sheet = relative && !folders.is_empty();
    if below_sheet {
        let below = folders.iter().try_fold(dir.to_path_buf(), |d, part| find_in(&d, part, is_dir));
        if let Some(path) = below.and_then(|d| find_in(&d, file, is_file)) {
            return Ok(path);
        }
    }
    find_in(dir, file, is_file).ok_or_else(|| {
        // The sheet's path is canonical, which on Windows starts with
        // `\\?\`; the message leaves that out.
        let shown = crate::mount::display_host_path;
        let beside = shown(&dir.join(file));
        if below_sheet {
            format!(
                "{} not found, nor {}",
                shown(&dir.join(parts.join(std::path::MAIN_SEPARATOR_STR))),
                beside
            )
        } else {
            format!("{} not found", beside)
        }
    })
}

/// `name` in `dir`, as written or in any case, if `kind` holds for it.
fn find_in(dir: &Path, name: &str, kind: fn(&Path) -> bool) -> Option<PathBuf> {
    let path = dir.join(name);
    if kind(&path) {
        return Some(path);
    }
    hostfs::read_dir(dir)
        .into_iter()
        .flatten()
        .map(|entry| entry.path)
        .find(|p| p.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(name)) && kind(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
    fn a_first_track_whose_pregap_is_in_the_file() {
        // BlindWrite's images keep track 1's pregap; the disc's sector 0
        // is a data track 1's INDEX 01, three sectors into the file.
        let dir = scratch("first_pregap");
        let mut data = sectors(3, 9);
        data.extend(sectors(5, 1));
        data.extend(sectors(4, 2));
        fs::write(dir.join("disc.bin"), data).unwrap();
        fs::write(
            dir.join("disc.cue"),
            "FILE \"disc.bin\" BINARY\n TRACK 01 MODE1/2352\n  INDEX 00 00:00:00\n  \
             INDEX 01 00:00:03\n TRACK 02 MODE1/2352\n  INDEX 01 00:00:08\n",
        )
        .unwrap();
        let image = CdImage::open(&dir.join("disc.cue")).unwrap();
        let tracks = image.tracks();
        assert_eq!((tracks[0].pregap_start, tracks[0].start, tracks[0].end), (0, 0, 5));
        assert_eq!((tracks[1].start, tracks[1].end), (5, 9));
        let mut data = [0u8; DATA_SECTOR];
        image.read_data(0, &mut data).unwrap();
        assert!(data.iter().all(|&b| b == 1), "sector 0 is INDEX 01's");
        image.read_data(5, &mut data).unwrap();
        assert!(data.iter().all(|&b| b == 2));

        // A PREGAP on track 1 is the same two seconds, before sector 0.
        fs::write(
            dir.join("gap.cue"),
            "FILE \"disc.bin\" BINARY\n TRACK 01 MODE1/2352\n  PREGAP 00:02:00\n  \
             INDEX 01 00:00:00\n",
        )
        .unwrap();
        let image = CdImage::open(&dir.join("gap.cue")).unwrap();
        assert_eq!((image.tracks()[0].start, image.tracks()[0].end), (0, 12));

        // A longer one leaves the rest of it on the disc, silent.
        fs::write(
            dir.join("long_gap.cue"),
            "FILE \"disc.bin\" BINARY\n TRACK 01 MODE1/2352\n  PREGAP 00:04:00\n  \
             INDEX 01 00:00:00\n",
        )
        .unwrap();
        let image = CdImage::open(&dir.join("long_gap.cue")).unwrap();
        let t = &image.tracks()[0];
        assert_eq!((t.pregap_start, t.start, t.end), (0, 150, 162));
        image.read_data(150, &mut data).unwrap();
        assert!(data.iter().all(|&b| b == 9));
    }

    #[test]
    fn an_audio_first_track_with_audio_in_its_pregap() {
        // Hidden audio before track 1's INDEX 01 is on the disc from
        // sector 0, as sheets of single-file rips have it.
        let dir = scratch("first_audio_pregap");
        let mut data = sectors(3, 9);
        data.extend(sectors(5, 1));
        data.extend(sectors(4, 2));
        fs::write(dir.join("disc.bin"), data).unwrap();
        fs::write(
            dir.join("disc.cue"),
            "FILE \"disc.bin\" BINARY\n TRACK 01 AUDIO\n  INDEX 00 00:00:00\n  \
             INDEX 01 00:00:03\n TRACK 02 MODE1/2352\n  INDEX 01 00:00:08\n",
        )
        .unwrap();
        let image = CdImage::open(&dir.join("disc.cue")).unwrap();
        let tracks = image.tracks();
        assert_eq!((tracks[0].pregap_start, tracks[0].start, tracks[0].end), (0, 3, 8));
        assert_eq!((tracks[1].start, tracks[1].end), (8, 12));
        let mut raw = [0u8; RAW_SECTOR];
        image.read_raw(0, &mut raw).unwrap();
        assert!(raw.iter().all(|&b| b == 9), "the hidden audio");
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
    fn files_in_a_folder_below_the_sheet() {
        // The sheet names `audio\Song2.bin`; the folder on disk is AUDIO.
        let dir = scratch("subfolder");
        fs::create_dir_all(dir.join("AUDIO")).unwrap();
        fs::write(dir.join("data.bin"), vec![7u8; 10 * 2048]).unwrap();
        fs::write(dir.join("AUDIO").join("Song2.bin"), sectors(5, 3)).unwrap();
        fs::write(
            dir.join("disc.cue"),
            "FILE \"data.bin\" BINARY\n TRACK 01 MODE1/2048\n  INDEX 01 00:00:00\n\
             FILE \"audio\\Song2.bin\" BINARY\n TRACK 02 AUDIO\n  INDEX 01 00:00:00\n",
        )
        .unwrap();
        let image = CdImage::open(&dir.join("disc.cue")).unwrap();
        let mut raw = [0u8; RAW_SECTOR];
        image.read_raw(image.tracks()[1].start, &mut raw).unwrap();
        assert_eq!(raw[0], 3);
    }

    #[test]
    fn files_are_not_looked_for_above_the_sheet() {
        // `..\Song2.bin` names a file above the sheet's folder; only the
        // copy next to the sheet counts.
        let root = scratch("above");
        let dir = root.join("disc");
        fs::create_dir_all(&dir).unwrap();
        fs::write(root.join("Song2.bin"), sectors(5, 9)).unwrap();
        fs::write(dir.join("Song2.bin"), sectors(5, 3)).unwrap();
        fs::write(dir.join("disc.cue"), "FILE \"..\\Song2.bin\" BINARY\n TRACK 01 AUDIO\n  INDEX 01 00:00:00\n").unwrap();
        let image = CdImage::open(&dir.join("disc.cue")).unwrap();
        let mut raw = [0u8; RAW_SECTOR];
        image.read_raw(image.tracks()[0].start, &mut raw).unwrap();
        assert_eq!(raw[0], 3);
    }

    #[test]
    fn sectors_whose_edc_fails_cant_be_read() {
        let dir = scratch("unreadable");
        let mut data: Vec<u8> = (0..10)
            .flat_map(|lba| crate::cdrom::mode1_sector(lba, lba as u8))
            .collect();
        // Sector 5 as copy protections master it: its data doesn't match
        // its EDC.
        data[5 * RAW_SECTOR + 16..5 * RAW_SECTOR + 16 + DATA_SECTOR].fill(0x55);
        fs::write(dir.join("disc.bin"), data).unwrap();
        fs::write(
            dir.join("disc.cue"),
            "FILE \"disc.bin\" BINARY\n TRACK 01 MODE1/2352\n  INDEX 01 00:00:00\n",
        )
        .unwrap();
        let image = CdImage::open(&dir.join("disc.cue")).unwrap();

        let mut data = [0u8; DATA_SECTOR];
        let mut raw = [0u8; RAW_SECTOR];
        let error = image.read_data(5, &mut data).unwrap_err();
        assert!(is_unreadable(&error), "{}", error);
        assert!(is_unreadable(&image.read_raw(5, &mut raw).unwrap_err()));
        for lba in [4, 6] {
            image.read_data(lba, &mut data).unwrap();
            assert!(data.iter().all(|&b| b == lba as u8));
            image.read_raw(lba, &mut raw).unwrap();
            assert_eq!(raw[16], lba as u8);
        }
        // A file over it fails there; one before it reads.
        let extent = |lba, size| Extent {
            lba,
            size,
            time: 0,
            date: 0,
            hidden: false,
        };
        let mut buf = vec![0u8; 3 * DATA_SECTOR];
        assert!(
            image
                .read_extent(&extent(4, 3 * DATA_SECTOR as u32), 0, &mut buf)
                .is_err()
        );
        assert_eq!(
            image
                .read_extent(&extent(2, 3 * DATA_SECTOR as u32), 0, &mut buf)
                .unwrap(),
            buf.len()
        );
        // Past the disc is another error.
        assert!(!is_unreadable(&image.read_data(10, &mut data).unwrap_err()));
    }

    /// Raw sector `lba` of a BlindWrite data file: a Mode 1 sector whose
    /// data is all `lba`, and 96 bytes of subchannel holding `q` as Q.
    fn with_subchannel(lba: u32, q: [u8; 12]) -> Vec<u8> {
        let mut sector = crate::cdrom::mode1_sector(lba, lba as u8).to_vec();
        let mut sub = [0u8; 96];
        sub[12..24].copy_from_slice(&q);
        sector.extend(sub);
        sector
    }

    #[test]
    fn blindwrite_images() {
        use crate::cdrom::b6t::{self, Dpm};
        let dir = scratch("blindwrite");
        // Track 1, Mode 1 with subchannel, its pregap stored from sector
        // -150 on: sector 5's Q names track 9 (as a protection might
        // master it), sector 6's has a bad CRC, the others none.
        let mut q5 = [0x41, 0x09, 0x01, 0, 0, 0, 0, 0x00, 0x02, 0x05, 0, 0];
        let crc = q_crc(&q5);
        q5[10..12].copy_from_slice(&crc.to_be_bytes());
        let mut q6 = q5;
        q6[11] ^= 1;
        let mut data = vec![0u8; 150 * 2448];
        for lba in 0..20 {
            let q = match lba {
                5 => q5,
                6 => q6,
                _ => [0; 12],
            };
            data.extend(with_subchannel(lba, q));
        }
        // Sector 7 as copy protections master it: its EDC fails.
        data[(157 * 2448 + 100)..(157 * 2448 + 104)].fill(0xEE);
        fs::write(dir.join("Game.b6i"), data).unwrap();
        // Track 2, audio with a pregap of 10, split into the next file.
        fs::write(dir.join("Game.b00"), sectors(30, 0x22)).unwrap();
        // Track 3 in session 2, Mode 2 Form 1 as 2048-byte sectors.
        fs::write(dir.join("Game.b01"), vec![0x33u8; 10 * 2048]).unwrap();
        let blocks = [
            ("GAME.B6I", 0, -150, 170, 2448),
            ("C:\\Images\\GAME.B00", 0, 20, 30, 2352),
            ("GAME.B01", 0, 60, 10, 2048),
        ];
        let dpm = Dpm {
            start: 0,
            resolution: 10,
            angles: vec![10, 20],
        };
        let descriptor = b6t::build(
            Some("1234567890123"),
            &blocks,
            &[
                &[(2, 4, 1, -150, 150, 170), (1, 2, 2, 20, 10, 30)],
                &[(4, 4, 3, 60, 0, 10)],
            ],
            Some(&dpm),
        );
        fs::write(dir.join("Game.b6t"), descriptor).unwrap();
        let image = CdImage::open(&dir.join("Game.b6t")).unwrap();

        let tracks = image.tracks();
        let layout: Vec<_> = tracks
            .iter()
            .map(|t| {
                (
                    t.number,
                    t.mode,
                    t.session,
                    t.control,
                    t.pregap_start,
                    t.start,
                    t.end,
                )
            })
            .collect();
        assert_eq!(
            layout,
            [
                (1, TrackMode::Mode1_2352, 1, 4, 0, 0, 20),
                (2, TrackMode::Audio, 1, 2, 20, 30, 50),
                (3, TrackMode::Mode1_2048, 2, 4, 60, 60, 70),
            ]
        );
        assert_eq!(image.leadout(), 70);
        assert_eq!(image.data_track().unwrap().number, 3, "the last session's");
        assert_eq!(image.mcn(), Some("1234567890123"));
        assert_eq!(image.dpm(), Some(&dpm));

        let mut sector = [0u8; DATA_SECTOR];
        image.read_data(4, &mut sector).unwrap();
        assert!(sector.iter().all(|&b| b == 4));
        assert!(is_unreadable(&image.read_data(7, &mut sector).unwrap_err()));
        image.read_data(65, &mut sector).unwrap();
        assert!(sector.iter().all(|&b| b == 0x33));
        let mut raw = [0u8; RAW_SECTOR];
        image.read_raw(40, &mut raw).unwrap();
        assert!(raw.iter().all(|&b| b == 0x22));
        assert!(
            image.read_raw(55, &mut raw).is_err(),
            "between the sessions"
        );

        // The image's subchannel, and for the sectors without, P and Q
        // as the layout has them.
        let mut sub = [0u8; 96];
        image.read_subchannel(5, &mut sub).unwrap();
        assert_eq!(sub[12..24], q5);
        let at = |lba| image.q_position(lba).unwrap();
        assert_eq!(
            at(5),
            QPosition {
                control: 4,
                track: 9,
                index: 1,
                relative: 0,
                absolute: 5
            }
        );
        assert_eq!(
            at(6),
            QPosition {
                control: 4,
                track: 1,
                index: 1,
                relative: 6,
                absolute: 6
            },
            "a bad CRC"
        );
        assert_eq!(
            at(25),
            QPosition {
                control: 2,
                track: 2,
                index: 0,
                relative: 4,
                absolute: 25
            }
        );
        assert_eq!(at(33).relative, 3);
        image.read_subchannel(25, &mut sub).unwrap();
        assert!(sub[..12].iter().all(|&b| b == 0xFF), "P in the pregap");
        assert_eq!(sub[12..15], [0x21, 0x02, 0x00]);
        image.read_subchannel(33, &mut sub).unwrap();
        assert!(sub[..12].iter().all(|&b| b == 0));

        // The files a state and an automount folder count as the disc's.
        let files = track_files(&dir.join("Game.b6t"));
        assert_eq!(files.len(), 3);
        assert!(!lacks_a_track_file(&dir.join("Game.b6t")));

        // A .bwa file beside the descriptor has the whole disc's DPM data.
        let mut bwa = vec![1, 0, 0, 0, 8, 0, 0, 0, 1, 0, 0, 0];
        for word in [0u32, 5, 3, 1, 2, 3] {
            bwa.extend(word.to_le_bytes());
        }
        fs::write(dir.join("GAME.BWA"), bwa).unwrap();
        let image = CdImage::open(&dir.join("Game.b6t")).unwrap();
        assert_eq!(image.dpm().unwrap().angles, [1, 2, 3]);
        assert_eq!(track_files(&dir.join("Game.b6t")).len(), 4);

        fs::remove_file(dir.join("Game.b01")).unwrap();
        assert!(lacks_a_track_file(&dir.join("Game.b6t")));
        assert!(CdImage::open(&dir.join("Game.b6t")).is_err());
    }

    #[test]
    fn blindwrite_descriptors_of_any_name() {
        // Found by their signature, as CHD files are.
        let dir = scratch("blindwrite_named");
        fs::write(dir.join("disc.b5i"), sectors(4, 0)).unwrap();
        let descriptor = crate::cdrom::b6t::build(
            None,
            &[("disc.b5i", 0, 0, 4, 2352)],
            &[&[(1, 0, 1, 0, 0, 4)]],
            None,
        );
        fs::write(dir.join("disc.dat"), descriptor).unwrap();
        let image = CdImage::open(&dir.join("disc.dat")).unwrap();
        assert!(image.tracks()[0].is_audio());
        assert_eq!(image.data_track(), None);
        // A data track can't be in sectors of a size its mode hasn't.
        let descriptor = crate::cdrom::b6t::build(
            None,
            &[("disc.b5i", 0, 0, 4, 2352)],
            &[&[(4, 4, 1, 0, 0, 4)]],
            None,
        );
        fs::write(dir.join("disc.b5t"), descriptor).unwrap();
        assert!(
            CdImage::open(&dir.join("disc.b5t")).is_ok(),
            "Mode 2 Form 1 as whole sectors"
        );
        let descriptor = crate::cdrom::b6t::build(
            None,
            &[("disc.b5i", 0, 0, 4, 2048)],
            &[&[(1, 0, 1, 0, 0, 4)]],
            None,
        );
        fs::write(dir.join("disc.b5t"), descriptor).unwrap();
        assert!(
            CdImage::open(&dir.join("disc.b5t")).is_err(),
            "audio in 2048-byte sectors"
        );
    }

    #[test]
    fn q_subchannel_crc() {
        // CRC-16/XMODEM's check value.
        assert_eq!(crc16(b"123456789"), 0x31C3);
        // A pressed disc's Q at 00:02:00, track 1 index 1, decodes back.
        let track = Track::stored(
            1,
            TrackMode::Mode1_2048,
            0,
            0,
            10,
            Span::new(0, 10, 0, 0, 2048),
        );
        let q = made_q(&track, 0);
        assert_eq!(q[..10], [0x41, 0x01, 0x01, 0, 0, 0, 0, 0x00, 0x02, 0x00]);
        assert_eq!(
            decode_q(&q),
            Some(QPosition {
                control: 4,
                track: 1,
                index: 1,
                relative: 0,
                absolute: 0
            })
        );
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

        // High Sierra has "CDROM" at byte 9.
        let mut old = vec![0u8; 17 * 2048];
        old[16 * 2048 + 9..16 * 2048 + 14].copy_from_slice(b"CDROM");
        fs::write(dir.join("old.iso"), &old).unwrap();
        let image = CdImage::open(&dir.join("old.iso")).unwrap();
        assert_eq!(image.tracks()[0].mode, TrackMode::Mode1_2048);

        fs::write(dir.join("junk.iso"), vec![0u8; 40 * 2048]).unwrap();
        assert!(CdImage::open(&dir.join("junk.iso")).is_err());
    }

    #[test]
    fn images_held_in_memory() {
        let mut iso = vec![0u8; 17 * 2048];
        iso[16 * 2048 + 1..16 * 2048 + 6].copy_from_slice(b"CD001");
        let iso = MemoryImage::from(iso);
        assert!(CdImage::probe(&iso));
        let image = CdImage::from_memory("GAME.ISO", iso).unwrap();
        assert_eq!(image.tracks()[0].mode, TrackMode::Mode1_2048);
        assert_eq!(image.leadout(), 17);
        let mut sector = [0u8; DATA_SECTOR];
        image.read_data(16, &mut sector).unwrap();
        assert_eq!(&sector[1..6], b"CD001");
        assert!(!CdImage::probe(&MemoryImage::new(40 * 2048)));
        assert!(CdImage::from_memory("junk.iso", MemoryImage::new(40 * 2048)).is_err());
    }

    #[test]
    #[allow(clippy::arc_with_non_send_sync)]
    fn a_sheet_at_the_root_of_an_overlaid_drive() {
        use crate::overlay::{Folder, Overlay};
        let dir = scratch("overlaid");
        fs::create_dir_all(dir.join("lower")).unwrap();
        fs::write(dir.join("lower/game.bin"), sectors(5, 4)).unwrap();
        fs::write(dir.join("lower/game.cue"), "FILE \"game.bin\" BINARY\n TRACK 01 MODE1/2352\n  INDEX 01 00:00:00\n")
            .unwrap();
        let overlay = Overlay::new(Box::new(Folder(dir.join("lower"))), Some(dir.join("upper"))).unwrap();
        let layer = hostfs::add_layer(std::sync::Arc::new(overlay));
        let image = CdImage::open(&layer.root().join("game.cue")).unwrap();
        assert_eq!(image.leadout(), 5);
        let mut data = [0u8; DATA_SECTOR];
        image.read_data(2, &mut data).unwrap();
        assert!(data.iter().all(|&b| b == 4));
    }

    /// The 96 bytes of linear subchannel of sector `lba` of a disc of one
    /// data track from 0, with R holding `lba`; its Q's CRC broken as
    /// SecuROM's mastering does (XOR 8001h) when `bad`.
    fn linear_sub(track: &Track, lba: u32, bad: bool) -> [u8; 96] {
        let mut sub = [0u8; 96];
        let mut q = made_q(track, lba);
        if bad {
            q[10] ^= 0x80;
            q[11] ^= 0x01;
        }
        sub[12..24].copy_from_slice(&q);
        sub[24] = lba as u8;
        sub
    }

    #[test]
    fn subchannel_files_beside_a_sheet() {
        let dir = scratch("sub_file");
        let count = 300u32;
        let track = Track::stored(
            1,
            TrackMode::Mode1_2352,
            0,
            0,
            count,
            Span::new(0, count, 0, 0, 2352),
        );
        let cue = "FILE \"disc.img\" BINARY\n TRACK 01 MODE1/2352\n  INDEX 01 00:00:00\n";
        fs::write(dir.join("disc.img"), sectors(count as usize, 0)).unwrap();
        fs::write(dir.join("game.cue"), cue).unwrap();
        // CloneCD's and DiscImageCreator's layout, named as the data file:
        // from sector 0, each channel's bytes after the one before.
        let clone: Vec<u8> = (0..count)
            .flat_map(|lba| linear_sub(&track, lba, lba == 100))
            .collect();
        fs::write(dir.join("DISC.SUB"), &clone).unwrap();
        let image = CdImage::open(&dir.join("game.cue")).unwrap();
        let mut sub = [0u8; 96];
        image.read_subchannel(100, &mut sub).unwrap();
        assert_eq!(sub, linear_sub(&track, 100, true), "the bad Q as the disc has it");
        image.read_subchannel(299, &mut sub).unwrap();
        assert_eq!(sub[24], 299u32 as u8);
        assert_eq!(image.q_position(100).unwrap().absolute, 100);
        let files = track_files(&dir.join("game.cue"));
        let named = |p: &PathBuf| p.file_name().unwrap().eq_ignore_ascii_case("disc.sub");
        assert!(files.iter().any(named), "{:?}", files);

        // redumper's: raw P-W, from 150 sectors before 0 in the lead-in,
        // whose Qs (track 0) say nothing of where they are.
        fs::remove_file(dir.join("DISC.SUB")).unwrap();
        let mut raw = Vec::new();
        for i in 0..150u32 {
            let mut lead_in = [0u8; 96];
            lead_in[12..24].copy_from_slice(&made_q(&track, i));
            lead_in[13] = 0;
            let crc = q_crc(&lead_in[12..24]);
            lead_in[22..24].copy_from_slice(&crc.to_be_bytes());
            raw.extend(super::super::interleave_subchannel(&lead_in));
        }
        for lba in 0..count {
            raw.extend(super::super::interleave_subchannel(&linear_sub(&track, lba, lba == 7)));
        }
        fs::write(dir.join("game.subcode"), &raw).unwrap();
        let image = CdImage::open(&dir.join("game.cue")).unwrap();
        image.read_subchannel(7, &mut sub).unwrap();
        assert_eq!(sub, linear_sub(&track, 7, true));
        image.read_subchannel(250, &mut sub).unwrap();
        assert_eq!(sub, linear_sub(&track, 250, false));

        // A file of another size, or whose Qs say nothing, is left alone:
        // the drive makes P and Q from the layout.
        fs::write(dir.join("game.subcode"), &raw[..raw.len() - 1]).unwrap();
        let image = CdImage::open(&dir.join("game.cue")).unwrap();
        image.read_subchannel(7, &mut sub).unwrap();
        assert_eq!(sub[12..24], made_q(&track, 7));
        assert_eq!(sub[24], 0);
        fs::write(dir.join("game.subcode"), vec![0u8; raw.len()]).unwrap();
        let image = CdImage::open(&dir.join("game.cue")).unwrap();
        image.read_subchannel(7, &mut sub).unwrap();
        assert_eq!(sub[12..24], made_q(&track, 7));

        // A file left alone doesn't hide one named as a data file.
        fs::write(dir.join("DISC.SUB"), &clone).unwrap();
        let image = CdImage::open(&dir.join("game.cue")).unwrap();
        image.read_subchannel(100, &mut sub).unwrap();
        assert_eq!(sub, linear_sub(&track, 100, true));
        let files = track_files(&dir.join("game.cue"));
        assert!(files.iter().any(named), "{:?}", files);
    }

    #[test]
    fn clonecd_images() {
        let dir = scratch("clonecd");
        // A data track of 200 sectors whose sector 50 a drive couldn't
        // read (its EDC doesn't match its data), then an audio track of
        // 100 with a pregap of 20 in the image, its copy flag set.
        let mut img: Vec<u8> = (0..200)
            .flat_map(|lba| crate::cdrom::mode1_sector(lba, lba as u8))
            .collect();
        img[50 * RAW_SECTOR + 16] ^= 0xFF;
        img.extend(sectors(100, 7));
        fs::write(dir.join("Game.IMG"), &img).unwrap();
        let ccd = "[CloneCD]\r\nVersion=3\r\n[Disc]\r\nTocEntries=5\r\nSessions=1\r\n\
            DataTracksScrambled=0\r\nCDTextLength=0\r\nCATALOG=0724384512345\r\n\
            [Session 1]\r\nPreGapMode=1\r\nPreGapSubC=0\r\n\
            [Entry 0]\r\nSession=1\r\nPoint=0xa2\r\nControl=0x04\r\nPLBA=300\r\n\
            [Entry 1]\r\nSession=1\r\nPoint=0x01\r\nControl=0x04\r\nPLBA=0\r\n\
            [Entry 2]\r\nSession=1\r\nPoint=0x02\r\nControl=0x02\r\nPLBA=220\r\n\
            [TRACK 1]\r\nMODE=1\r\nINDEX 1=0\r\n\
            [TRACK 2]\r\nMODE=0\r\nINDEX 0=200\r\nINDEX 1=220\r\n";
        fs::write(dir.join("game.ccd"), ccd).unwrap();
        let span = Span::new(0, 300, 0, 0, 2352);
        let data = Track::stored(1, TrackMode::Mode1_2352, 0, 0, 300, span);
        let sub: Vec<u8> = (0..300)
            .flat_map(|lba| linear_sub(&data, lba, lba == 120))
            .collect();
        fs::write(dir.join("game.sub"), &sub).unwrap();

        let image = CdImage::open(&dir.join("game.ccd")).unwrap();
        let tracks: Vec<_> = image
            .tracks()
            .iter()
            .map(|t| (t.number, t.mode, t.pregap_start, t.start, t.end, t.control))
            .collect();
        assert_eq!(
            tracks,
            [
                (1, TrackMode::Mode1_2352, 0, 0, 200, 4),
                (2, TrackMode::Audio, 200, 220, 300, 2)
            ]
        );
        assert_eq!(image.leadout(), 300);
        assert_eq!(image.mcn(), Some("0724384512345"));
        let mut buf = [0u8; DATA_SECTOR];
        let mut raw = [0u8; RAW_SECTOR];
        image.read_data(49, &mut buf).unwrap();
        assert!(buf.iter().all(|&b| b == 49));
        assert!(is_unreadable(&image.read_data(50, &mut buf).unwrap_err()));
        assert!(is_unreadable(&image.read_raw(50, &mut raw).unwrap_err()));
        image.read_raw(250, &mut raw).unwrap();
        assert!(raw.iter().all(|&b| b == 7), "audio as the image has it");
        // The subchannel file beside it, named as it: a bad Q as dumped.
        let mut got = [0u8; 96];
        image.read_subchannel(120, &mut got).unwrap();
        assert_eq!(got, linear_sub(&data, 120, true));
        let mut files: Vec<String> = track_files(&dir.join("game.ccd"))
            .iter()
            .map(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_ascii_lowercase()
            })
            .collect();
        files.sort();
        assert_eq!(files, ["game.img", "game.sub"]);
        assert!(!lacks_a_track_file(&dir.join("game.ccd")));

        // A descriptor of another name is read as one by how it starts,
        // and looks for the image named as it.
        fs::copy(dir.join("game.ccd"), dir.join("other.txt")).unwrap();
        let error = CdImage::open(&dir.join("other.txt")).unwrap_err();
        assert!(error.contains("other.img"), "{}", error);
        fs::remove_file(dir.join("Game.IMG")).unwrap();
        assert!(lacks_a_track_file(&dir.join("game.ccd")));
        fs::write(dir.join("two.ccd"), ccd.replace("Sessions=1", "Sessions=2")).unwrap();
        let error = CdImage::open(&dir.join("two.ccd")).unwrap_err();
        assert!(
            error.contains("two.ccd") && error.contains("several sessions"),
            "{}",
            error
        );
    }
}
