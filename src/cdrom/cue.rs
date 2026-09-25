//! CUE sheets: the text files that describe a CD image's tracks and the
//! files that hold them.
//!
//! ```text
//! FILE "SAMNMAX.img" BINARY
//!   TRACK 1 MODE1/2352
//!     INDEX 1 00:00:00
//!   TRACK 2 AUDIO
//!     INDEX 0 22:53:03
//!     INDEX 1 22:55:03
//! ```

use crate::mount::tokenize;

/// How a track's sectors are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackMode {
    /// Red Book audio: 2352 bytes of 16-bit stereo samples per sector.
    Audio,
    /// Data sectors of only their 2048 bytes of user data (ISO images).
    Mode1_2048,
    /// Whole Mode 1 sectors: sync, header, 2048 bytes of data, EDC/ECC.
    Mode1_2352,
    /// Mode 2 sectors without sync and header.
    Mode2_2336,
    /// Whole Mode 2 sectors (XA form 1 data after an 8-byte subheader).
    Mode2_2352,
}

impl TrackMode {
    /// Bytes per sector in the image file.
    pub fn sector_size(self) -> u64 {
        match self {
            TrackMode::Mode1_2048 => 2048,
            TrackMode::Mode2_2336 => 2336,
            TrackMode::Audio | TrackMode::Mode1_2352 | TrackMode::Mode2_2352 => 2352,
        }
    }

    /// Where the 2048 bytes of user data start in a stored sector.
    pub fn data_offset(self) -> u64 {
        match self {
            TrackMode::Mode1_2048 | TrackMode::Audio => 0,
            TrackMode::Mode1_2352 => 16,
            TrackMode::Mode2_2336 => 8,
            TrackMode::Mode2_2352 => 24,
        }
    }

    pub fn is_audio(self) -> bool {
        self == TrackMode::Audio
    }
}

/// How a FILE holds its data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileFormat {
    /// Raw sectors; audio samples little-endian.
    Binary,
    /// Raw sectors; audio samples big-endian.
    Motorola,
    /// A RIFF WAVE file: of 16-bit stereo 44.1 kHz audio, or decoded to
    /// it (and anything that turns out to be a compressed file).
    Wave,
    /// A compressed audio file (Ogg Vorbis, FLAC, MP3), decoded to CD audio.
    Compressed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueTrack {
    pub number: u8,
    pub mode: TrackMode,
    /// INDEX 00 (the start of the pregap in the file) and INDEX 01 (the
    /// start of the track), in sectors from the start of the file.
    pub index0: Option<u32>,
    pub index1: u32,
    /// Silent sectors before and after the track that aren't in the file.
    pub pregap: u32,
    pub postgap: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueFile {
    pub name: String,
    pub format: FileFormat,
    pub tracks: Vec<CueTrack>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CueSheet {
    pub files: Vec<CueFile>,
}

/// "mm:ss:ff" in sectors.
fn parse_msf(s: &str) -> Result<u32, String> {
    let parts: Vec<u32> = s
        .split(':')
        .map(|p| p.parse::<u32>())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("bad time {}", s))?;
    match parts[..] {
        [m, s, f] if s < 60 && f < 75 => Ok((m * 60 + s) * 75 + f),
        _ => Err(format!("bad time {}", s)),
    }
}

pub fn parse_cue(text: &str) -> Result<CueSheet, String> {
    let mut sheet = CueSheet::default();
    for (number, line) in text.lines().enumerate() {
        let error = |msg: String| format!("line {}: {}", number + 1, msg);
        let tokens = tokenize(line).map_err(error)?;
        let Some(keyword) = tokens.first() else {
            continue;
        };
        let arg = |i: usize| tokens.get(i).map(String::as_str).ok_or_else(|| error(format!("{} needs more", keyword)));
        match keyword.to_ascii_uppercase().as_str() {
            "FILE" => {
                let format = match arg(2)?.to_ascii_uppercase().as_str() {
                    "BINARY" => FileFormat::Binary,
                    "MOTOROLA" => FileFormat::Motorola,
                    "WAVE" => FileFormat::Wave,
                    "MP3" | "OGG" | "VORBIS" | "FLAC" => FileFormat::Compressed,
                    other => return Err(error(format!("{} files are not supported", other))),
                };
                sheet.files.push(CueFile { name: arg(1)?.to_string(), format, tracks: Vec::new() });
            }
            "TRACK" => {
                let file = sheet.files.last_mut().ok_or_else(|| error("TRACK before FILE".into()))?;
                let number = arg(1)?.parse::<u8>().map_err(|_| error("bad track number".into()))?;
                let mode = match arg(2)?.to_ascii_uppercase().as_str() {
                    "AUDIO" => TrackMode::Audio,
                    "MODE1/2048" => TrackMode::Mode1_2048,
                    "MODE1/2352" => TrackMode::Mode1_2352,
                    "MODE2/2336" => TrackMode::Mode2_2336,
                    "MODE2/2352" => TrackMode::Mode2_2352,
                    other => return Err(error(format!("{} tracks are not supported", other))),
                };
                file.tracks.push(CueTrack { number, mode, index0: None, index1: 0, pregap: 0, postgap: 0 });
            }
            keyword @ ("INDEX" | "PREGAP" | "POSTGAP") => {
                let track = sheet
                    .files
                    .last_mut()
                    .and_then(|f| f.tracks.last_mut())
                    .ok_or_else(|| error(format!("{} before TRACK", keyword)))?;
                match keyword {
                    "INDEX" => {
                        let index = arg(1)?.parse::<u8>().map_err(|_| error("bad index".into()))?;
                        let at = parse_msf(arg(2)?).map_err(error)?;
                        match index {
                            0 => track.index0 = Some(at),
                            1 => track.index1 = at,
                            _ => {} // Subindexes don't matter to MSCDEX.
                        }
                    }
                    "PREGAP" => track.pregap = parse_msf(arg(1)?).map_err(error)?,
                    _ => track.postgap = parse_msf(arg(1)?).map_err(error)?,
                }
            }
            // CD-Text, catalog numbers, flags and comments.
            _ => {}
        }
    }
    if !sheet.files.iter().any(|f| !f.tracks.is_empty()) {
        return Err("no tracks".to_string());
    }
    Ok(sheet)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sam_and_max() {
        let sheet = parse_cue(
            "FILE \"SAMNMAX.img\" BINARY\r\n   TRACK 1 MODE1/2352\r\n   INDEX 1 00:00:00\r\n   \
             TRACK 2 AUDIO\r\n   INDEX 0 22:53:03\r\n   INDEX 1 22:55:03\r\n",
        )
        .unwrap();
        assert_eq!(sheet.files.len(), 1);
        let file = &sheet.files[0];
        assert_eq!((file.name.as_str(), file.format), ("SAMNMAX.img", FileFormat::Binary));
        assert_eq!(file.tracks[0].mode, TrackMode::Mode1_2352);
        assert_eq!(
            file.tracks[1],
            CueTrack {
                number: 2,
                mode: TrackMode::Audio,
                index0: Some(102_978),
                index1: 103_128,
                pregap: 0,
                postgap: 0
            }
        );
    }

    #[test]
    fn several_files_gaps_and_other_keywords() {
        let sheet = parse_cue(
            "REM GENRE Game\nCATALOG 0000000000000\nFILE \"Game (Track 1).bin\" BINARY\n  TRACK 01 MODE2/2352\n    \
             INDEX 01 00:00:00\nFILE \"Game (Track 2).wav\" WAVE\n  TRACK 02 AUDIO\n    FLAGS DCP\n    \
             PREGAP 00:02:00\n    INDEX 01 00:00:00\n    POSTGAP 00:01:00\n",
        )
        .unwrap();
        assert_eq!(sheet.files.len(), 2);
        assert_eq!(sheet.files[1].format, FileFormat::Wave);
        let track = &sheet.files[1].tracks[0];
        assert_eq!((track.pregap, track.postgap), (150, 75));
    }

    #[test]
    fn errors() {
        assert!(parse_cue("TRACK 01 AUDIO").is_err());
        assert_eq!(parse_cue("FILE \"x.mp3\" MP3\n TRACK 01 AUDIO").unwrap().files[0].format, FileFormat::Compressed);
        assert!(parse_cue("FILE \"x.opus\" OPUS\n TRACK 01 AUDIO").is_err());
        assert!(parse_cue("FILE \"x.bin\" BINARY\n TRACK 01 CDG").is_err());
        assert!(parse_cue("FILE \"x.bin\" BINARY\n TRACK 01 AUDIO\n INDEX 01 00:61:00").is_err());
        assert!(parse_cue("").is_err());
    }
}
