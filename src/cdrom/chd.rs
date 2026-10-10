//! CD images in CHD files, MAME's "compressed hunks of data": the tracks its
//! metadata lists, and their sectors decompressed a hunk at a time.
//!
//! Every sector is kept as a frame of 2448 bytes, its data (as much as the
//! track's mode stores) and 96 bytes of subcode, which nothing here reads.
//! Each track starts on a multiple of four frames. Audio samples are
//! big-endian.

use super::cue::TrackMode;
use chd::metadata::{KnownMetadata, Metadata, MetadataTag};
use chd::Chd;
use std::io::{self, Read, Seek};

/// Bytes of a frame: a sector's 2352 and 96 of subcode.
pub const FRAME: u64 = 2448;

/// What a CHD can be read from: a file, or an image held in memory.
pub trait Stream: Read + Seek + Send {}
impl<T: Read + Seek + Send> Stream for T {}

/// A track as the metadata describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChdTrack {
    pub number: u8,
    pub mode: TrackMode,
    /// Frames in the file, those of the pregap among them if it is stored.
    pub frames: u32,
    pub pregap: u32,
    /// The pregap's sectors are in the file, before the track's.
    pub pregap_stored: bool,
    /// Silent sectors after the track that aren't in the file.
    pub postgap: u32,
}

pub struct ChdDisc {
    chd: Chd<Box<dyn Stream>>,
    tracks: Vec<ChdTrack>,
    hunk_size: u64,
    /// The hunk last decompressed, which `hunk` holds.
    cached: Option<u32>,
    hunk: Vec<u8>,
    compressed: Vec<u8>,
}

impl ChdDisc {
    pub fn open(stream: Box<dyn Stream>) -> Result<Self, String> {
        let mut chd = Chd::open(stream, None).map_err(|e| e.to_string())?;
        if chd.header().has_parent() {
            return Err("it is a CHD of changes to another, which isn't read".into());
        }
        let metadata: Vec<Metadata> = chd.metadata_refs().try_into().map_err(|e: chd::Error| e.to_string())?;
        let mut tracks = metadata
            .iter()
            .filter(|m| m.metatag() == KnownMetadata::CdRomTrack2 as u32 || m.metatag() == KnownMetadata::CdRomTrack as u32)
            .map(|m| parse_track(&m.value))
            .collect::<Result<Vec<_>, _>>()?;
        if tracks.is_empty() {
            return Err("it is not a CD image".into());
        }
        tracks.sort_by_key(|t| t.number);
        let hunk_size = chd.header().hunk_size() as u64;
        let hunk = chd.get_hunksized_buffer();
        Ok(ChdDisc { chd, tracks, hunk_size, cached: None, hunk, compressed: Vec::new() })
    }

    pub fn tracks(&self) -> &[ChdTrack] {
        &self.tracks
    }

    /// Bytes of frames in the file.
    pub fn byte_len(&self) -> u64 {
        self.chd.header().logical_bytes()
    }

    /// Fill `buf` from byte `at` of the frames on.
    pub fn read_at(&mut self, mut at: u64, buf: &mut [u8]) -> io::Result<()> {
        let mut done = 0;
        while done < buf.len() {
            let hunk = (at / self.hunk_size) as u32;
            if self.cached != Some(hunk) {
                self.cached = None;
                self.chd.hunk(hunk)?.read_hunk_in(&mut self.compressed, &mut self.hunk)?;
                self.cached = Some(hunk);
            }
            let skip = (at % self.hunk_size) as usize;
            let n = (self.hunk.len() - skip).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&self.hunk[skip..skip + n]);
            done += n;
            at += n as u64;
        }
        Ok(())
    }
}

/// A track from its metadata, "TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:1234
/// PREGAP:150 PGTYPE:VAUDIO PGSUB:RW POSTGAP:0" (the older form stops after
/// FRAMES).
fn parse_track(value: &[u8]) -> Result<ChdTrack, String> {
    let text = String::from_utf8_lossy(value);
    let text = text.trim_end_matches('\0');
    let field = |key: &str| {
        text.split_whitespace().find_map(|f| f.split_once(':').filter(|(k, _)| *k == key).map(|(_, v)| v))
    };
    let number = |key: &str| field(key).map_or(Ok(0), |v| v.parse::<u32>().map_err(|_| format!("bad {} in \"{}\"", key, text)));
    let track = number("TRACK")?;
    let kind = field("TYPE").ok_or_else(|| format!("no track type in \"{}\"", text))?;
    let mode = match kind {
        "AUDIO" => TrackMode::Audio,
        "MODE1" | "MODE1/2048" => TrackMode::Mode1_2048,
        "MODE1_RAW" | "MODE1/2352" => TrackMode::Mode1_2352,
        "MODE2" | "MODE2/2336" | "MODE2_FORM_MIX" => TrackMode::Mode2_2336,
        "MODE2_RAW" | "MODE2/2352" | "CDI/2352" => TrackMode::Mode2_2352,
        // The 2048 bytes of user data of XA form 1 sectors, as an ISO
        // image has its sectors.
        "MODE2_FORM1" | "MODE2/2048" => TrackMode::Mode1_2048,
        _ => return Err(format!("track {} is of type {}, which isn't read", track, kind)),
    };
    Ok(ChdTrack {
        number: u8::try_from(track).map_err(|_| format!("bad track number {}", track))?,
        mode,
        frames: number("FRAMES")?,
        pregap: number("PREGAP")?,
        pregap_stored: field("PGTYPE").is_some_and(|t| t.starts_with('V')),
        postgap: number("POSTGAP")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_metadata() {
        let track = parse_track(b"TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:1234 PREGAP:150 PGTYPE:VAUDIO PGSUB:RW POSTGAP:0\0").unwrap();
        assert_eq!(
            track,
            ChdTrack { number: 2, mode: TrackMode::Audio, frames: 1234, pregap: 150, pregap_stored: true, postgap: 0 }
        );
        let old = parse_track(b"TRACK:1 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:300").unwrap();
        assert_eq!((old.mode, old.frames, old.pregap, old.pregap_stored), (TrackMode::Mode1_2352, 300, 0, false));
        let unstored = parse_track(b"TRACK:3 TYPE:MODE1 SUBTYPE:NONE FRAMES:10 PREGAP:150 PGTYPE:MODE1 PGSUB:NONE POSTGAP:150").unwrap();
        assert_eq!((unstored.pregap_stored, unstored.postgap), (false, 150));
        assert!(parse_track(b"TRACK:1 TYPE:MODE2_FORM2 SUBTYPE:NONE FRAMES:1").is_err());
    }
}
