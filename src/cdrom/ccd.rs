//! CloneCD's `.ccd` descriptors, which CloneCD and DiscImageCreator write
//! beside a raw image of the disc (`.img`, every sector's 2352 bytes from
//! sector 0 on) and its subchannel (`.sub`).
//!
//! ```text
//! [CloneCD]
//! Version=3
//! [Disc]
//! TocEntries=4
//! Sessions=1
//! DataTracksScrambled=0
//! CATALOG=0123456789012
//! [Entry 3]
//! Session=1
//! Point=0x01
//! Control=0x04
//! PLBA=0
//! [TRACK 1]
//! MODE=1
//! INDEX 1=0
//! ```
//!
//! The `[Entry]` sections are the disc's table of contents as the drive
//! read it: a point of 1 to 99 is a track, with its control bits, and
//! A2h is the lead-out. Each `[TRACK]` section gives the track's mode (0
//! audio, 1 or 2) and its index points as sectors of the disc. The image
//! keeps the sectors as the drive read them, so a sector the drive
//! couldn't read keeps the data and EDC it came with.

use super::cue::{CueFile, CueSheet, CueTrack, FileFormat, TrackMode, is_catalog};

/// An entry of the table of contents a descriptor lists.
struct TocEntry {
    session: i64,
    point: i64,
    control: i64,
}

/// A track a descriptor lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CcdTrack {
    number: u8,
    mode: Option<u8>,
    index0: Option<u32>,
    index1: Option<u32>,
}

/// Whether `text` starts as a CloneCD descriptor does: its first section
/// is `[CloneCD]`.
pub fn is_ccd(text: &str) -> bool {
    text.trim_start_matches('\u{FEFF}')
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .is_some_and(|l| l.eq_ignore_ascii_case("[CloneCD]"))
}

/// A number as the descriptors write them: decimal, or hexadecimal after
/// `0x`, with a sign or without.
fn number(s: &str) -> Option<i64> {
    let s = s.trim();
    let (negative, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let value = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => i64::from_str_radix(hex, 16).ok()?,
        None => s.parse().ok()?,
    };
    Some(if negative { -value } else { value })
}

/// The disc a CloneCD descriptor describes, as a CUE sheet of one file,
/// `image` (the `.img` beside it), would: its tracks, their modes, index
/// points and copy and pre-emphasis flags, and the catalog number.
///
/// Discs of more than one session, and images whose data tracks are
/// stored scrambled, are refused: the sectors of each session's lead-out
/// and lead-in, which a disc's addresses count, aren't in the image, and
/// where each session starts in it isn't known from the descriptor alone.
pub fn parse(text: &str, image: &str) -> Result<CueSheet, String> {
    if !is_ccd(text) {
        return Err("it isn't a CloneCD descriptor".into());
    }
    let mut section = String::new();
    let mut tracks: Vec<CcdTrack> = Vec::new();
    let mut entries: Vec<TocEntry> = Vec::new();
    let mut catalog = None;
    for (n, line) in text.lines().enumerate() {
        let error = |msg: String| format!("line {}: {}", n + 1, msg);
        let line = line.trim().trim_start_matches('\u{FEFF}');
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_ascii_uppercase();
            if let Some(t) = section.strip_prefix("TRACK ") {
                let number = number(t)
                    .and_then(|v| u8::try_from(v).ok())
                    .filter(|v| (1..=99).contains(v))
                    .ok_or_else(|| error(format!("bad track number {}", t)))?;
                tracks.push(CcdTrack {
                    number,
                    ..CcdTrack::default()
                });
            } else if section.starts_with("ENTRY ") {
                // An entry that names no session is of the first.
                entries.push(TocEntry {
                    session: 1,
                    point: -1,
                    control: 0,
                });
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(error(format!("expected KEY=VALUE, found {}", line)));
        };
        let key = key.trim().to_ascii_uppercase();
        let value = value.trim();
        let num = || number(value).ok_or_else(|| error(format!("bad number {}", value)));
        let sector =
            || num().and_then(|v| u32::try_from(v).map_err(|_| error(format!("bad sector {}", v))));
        match section.as_str() {
            "DISC" => match key.as_str() {
                "SESSIONS" if num()? > 1 => {
                    return Err("discs of several sessions aren't supported".into());
                }
                "DATATRACKSSCRAMBLED" if num()? != 0 => {
                    return Err("images of scrambled data tracks aren't supported".into());
                }
                // The drive and MSCDEX report 13 digits; any other value
                // is no catalog number.
                "CATALOG" if is_catalog(value) => catalog = Some(value.to_string()),
                _ => {}
            },
            s if s.starts_with("ENTRY ") => {
                let entry = entries.last_mut().expect("an entry section was opened");
                match key.as_str() {
                    "SESSION" => entry.session = num()?,
                    "POINT" => entry.point = num()?,
                    "CONTROL" => entry.control = num()?,
                    _ => {}
                }
            }
            s if s.starts_with("TRACK ") => {
                let track = tracks.last_mut().expect("a track section was opened");
                match key.as_str() {
                    "MODE" => {
                        let mode = num()?;
                        track.mode = Some(
                            u8::try_from(mode)
                                .ok()
                                .filter(|m| *m <= 2)
                                .ok_or_else(|| error(format!("unknown track mode {}", mode)))?,
                        );
                    }
                    "INDEX 0" => track.index0 = Some(sector()?),
                    "INDEX 1" => track.index1 = Some(sector()?),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if entries.iter().any(|e| e.session != 1) {
        return Err("discs of several sessions aren't supported".into());
    }
    if tracks.is_empty() {
        return Err("it lists no tracks".into());
    }
    tracks.sort_by_key(|t| t.number);
    let mut cue_tracks: Vec<CueTrack> = Vec::new();
    for t in &tracks {
        let mode = match t.mode {
            Some(0) => TrackMode::Audio,
            Some(1) => TrackMode::Mode1_2352,
            Some(2) => TrackMode::Mode2_2352,
            _ => return Err(format!("track {} has no mode", t.number)),
        };
        let index1 = t
            .index1
            .ok_or_else(|| format!("track {} has no INDEX 1", t.number))?;
        // An INDEX 0 at or after INDEX 1 marks no pregap.
        let index0 = t.index0.filter(|&i| i < index1);
        if cue_tracks
            .last()
            .is_some_and(|p| p.index1 >= index0.unwrap_or(index1))
        {
            return Err(format!(
                "track {} starts before the one before it",
                t.number
            ));
        }
        // The control bits the table of contents gives the track, of
        // which the CUE sheet's FLAGS keep four channels, digital copy and
        // pre-emphasis; the data bit comes from the mode.
        let control = entries
            .iter()
            .find(|e| e.point == t.number as i64)
            .map_or(0, |e| e.control as u8);
        cue_tracks.push(CueTrack {
            number: t.number,
            mode,
            index0,
            index1,
            pregap: 0,
            postgap: 0,
            flags: control & 0x0B,
        });
    }
    Ok(CueSheet {
        files: vec![CueFile {
            name: image.to_string(),
            format: FileFormat::Binary,
            tracks: cue_tracks,
        }],
        catalog,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_TRACK: &str = "[CloneCD]\r\nVersion=3\r\n[Disc]\r\nTocEntries=4\r\nSessions=1\r\n\
        DataTracksScrambled=0\r\nCDTextLength=0\r\n[Session 1]\r\nPreGapMode=1\r\nPreGapSubC=0\r\n\
        [Entry 0]\r\nSession=1\r\nPoint=0xa0\r\nADR=0x01\r\nControl=0x04\r\nTrackNo=0\r\nAMin=0\r\n\
        ASec=0\r\nAFrame=0\r\nALBA=-150\r\nZero=0\r\nPMin=1\r\nPSec=0\r\nPFrame=0\r\nPLBA=4350\r\n\
        [Entry 1]\r\nSession=1\r\nPoint=0xa2\r\nControl=0x04\r\nPLBA=300\r\n\
        [Entry 2]\r\nSession=1\r\nPoint=0x01\r\nADR=0x01\r\nControl=0x04\r\nPLBA=0\r\n\
        [TRACK 1]\r\nMODE=1\r\nINDEX 1=0\r\n";

    #[test]
    fn a_data_disc() {
        assert!(is_ccd(ONE_TRACK));
        let sheet = parse(ONE_TRACK, "game.img").unwrap();
        assert_eq!(sheet.catalog, None);
        assert_eq!(sheet.files.len(), 1);
        let file = &sheet.files[0];
        assert_eq!(
            (file.name.as_str(), file.format),
            ("game.img", FileFormat::Binary)
        );
        let track = &file.tracks[0];
        assert_eq!(
            (track.number, track.mode, track.index0, track.index1),
            (1, TrackMode::Mode1_2352, None, 0)
        );
        assert_eq!(track.flags, 0, "the data bit isn't a flag");
    }

    #[test]
    fn audio_tracks_with_pregaps_flags_and_a_catalog() {
        let text = "[CloneCD]\nVersion=3\n[Disc]\nSessions=1\nCATALOG=0724384512345\n\
            [Entry 0]\nSession=1\nPoint=0x01\nControl=0x04\nPLBA=0\n\
            [Entry 1]\nSession=1\nPoint=0x02\nControl=0x03\nPLBA=1150\n\
            [Entry 2]\nSession=1\nPoint=0x03\nControl=0x00\nPLBA=2000\n\
            [TRACK 1]\nMODE=1\nINDEX 1=0\n\
            [TRACK 2]\nMODE=0\nINDEX 0=1000\nINDEX 1=1150\n\
            [TRACK 3]\nMODE=0\nINDEX 1=2000\n";
        let sheet = parse(text, "music.img").unwrap();
        assert_eq!(sheet.catalog.as_deref(), Some("0724384512345"));
        for bad in ["0", "072438451234X", "07243845123456"] {
            let other = text.replace("0724384512345", bad);
            assert_eq!(parse(&other, "music.img").unwrap().catalog, None, "{}", bad);
        }
        let tracks = &sheet.files[0].tracks;
        let got: Vec<_> = tracks
            .iter()
            .map(|t| (t.number, t.mode, t.index0, t.index1, t.flags))
            .collect();
        assert_eq!(
            got,
            [
                (1, TrackMode::Mode1_2352, None, 0, 0),
                (2, TrackMode::Audio, Some(1000), 1150, 3),
                (3, TrackMode::Audio, None, 2000, 0),
            ]
        );
    }

    #[test]
    fn what_it_refuses() {
        let several = ONE_TRACK.replace("Sessions=1", "Sessions=2");
        assert!(
            parse(&several, "x.img")
                .unwrap_err()
                .contains("several sessions")
        );
        let scrambled = ONE_TRACK.replace("DataTracksScrambled=0", "DataTracksScrambled=1");
        assert!(
            parse(&scrambled, "x.img")
                .unwrap_err()
                .contains("scrambled")
        );
        let entry_of_session_2 =
            ONE_TRACK.replace("[Entry 2]\r\nSession=1", "[Entry 2]\r\nSession=2");
        assert!(
            parse(&entry_of_session_2, "x.img")
                .unwrap_err()
                .contains("several sessions")
        );
        assert!(
            parse(&ONE_TRACK.replace("MODE=1", "MODE=3"), "x.img")
                .unwrap_err()
                .contains("mode 3")
        );
        assert!(
            parse(&ONE_TRACK.replace("INDEX 1=0", ""), "x.img")
                .unwrap_err()
                .contains("INDEX 1")
        );
        let no_tracks = &ONE_TRACK[..ONE_TRACK.find("[TRACK 1]").unwrap()];
        assert!(parse(no_tracks, "x.img").unwrap_err().contains("no tracks"));
        assert!(!is_ccd("FILE \"x.bin\" BINARY\n"));
        assert!(parse("FILE \"x.bin\" BINARY\n", "x.img").is_err());
    }

    #[test]
    fn numbers_in_hex_or_decimal() {
        assert_eq!(number("0x1F"), Some(31));
        assert_eq!(number("-150"), Some(-150));
        assert_eq!(number(" 42 "), Some(42));
        assert_eq!(number("x"), None);
    }
}
