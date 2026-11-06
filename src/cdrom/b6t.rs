//! BlindWrite 5 and 6 images: a descriptor (`.b5t`, `.b6t`) that lists
//! the disc's sessions and tracks and the data files that hold their
//! sectors (`.b5i`, `.b6i`, and `.b00`, `.b01`... when BlindWrite split
//! them), and an optional `.bwa` file of DPM data beside it.
//!
//! BlindWrite never published the layout. This reads it as libMirage's
//! B6T parser does (CDEmu's `image-b6t`, by Rok Mandeljc, GPL-2.0 or
//! later, which this is ported from), checked against Aaru's BlindWrite
//! 5 reader (by Natalia Portillo). The descriptor is little-endian:
//!
//! ```text
//!   0  "BWT5 STREAM SIGN"
//!  48  u16 disc type (an MMC profile), u16 sessions
//!  64  u8 MCN valid, 13 bytes of MCN
//!  96  u16 lengths of PMA, ATIP, CD-Text, CD disc info; u32 of BCA
//! 120  u32 lengths of DVD structures and DVD disc info
//! 160  the drive's INQUIRY identification; 208 the volume ID
//! 240  u32 lengths of mode page 2Ah, an unknown block, the data
//!      blocks, the sessions and the DPM data
//! 260  the blocks those lengths give, in that order (the PMA, ATIP,
//!      CD-Text, BCA, DVD structures and disc info after the unknown
//!      block), then the data blocks, the sessions, the DPM data, a u32
//!      and "BWT5 STREAM FOOT"
//! ```
//!
//! A data block is a run of sectors in one file, all of one size: 2048,
//! 2336, 2352, or 2352 with 16 bytes of Q or 96 of P-W subchannel after
//! each sector. A track's sectors can be in several blocks, and a block
//! can hold several tracks.

/// A descriptor starts with this, and ends with `FOOTER`.
pub const SIGNATURE: &[u8; 16] = b"BWT5 STREAM SIGN";
const FOOTER: &[u8; 16] = b"BWT5 STREAM FOOT";

/// What a descriptor says of the disc.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Descriptor {
    /// The media catalog number, 13 digits, where the disc has one.
    pub mcn: Option<String>,
    pub blocks: Vec<DataBlock>,
    pub sessions: Vec<Session>,
    /// The DPM data the descriptor keeps (usually only the start of the
    /// disc's; a `.bwa` file has the whole disc's).
    pub dpm: Option<Dpm>,
}

/// A run of sectors in a data file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataBlock {
    /// The file's name, as BlindWrite wrote it.
    pub file: String,
    /// Byte offset of the first sector in the file.
    pub offset: u64,
    /// The first sector (negative in the first track's pregap).
    pub start: i64,
    pub sectors: u32,
    /// Bytes from one sector to the next.
    pub sector_size: u64,
}

impl DataBlock {
    /// Bytes of subchannel after each sector: 96 of P-W, deinterleaved
    /// (each channel's 12 bytes after the one before), or 16 of Q.
    pub fn subchannel(&self) -> u64 {
        match self.sector_size {
            2448 => 96,
            2368 => 16,
            _ => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub number: u16,
    pub tracks: Vec<Track>,
}

/// How a track's sectors are recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackType {
    Audio,
    Mode1,
    /// Mode 2 without XA forms.
    Mode2,
    Mode2Form1,
    Mode2Form2,
    /// A DVD's sectors, 2048 bytes of data each.
    Dvd,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub number: u8,
    pub kind: TrackType,
    /// The control bits of its TOC entry and Q subchannel: 4 for data,
    /// 2 for digital copy permitted, 1 for pre-emphasis.
    pub control: u8,
    /// The first sector of its pregap (negative for the first track's).
    pub start: i64,
    /// Sectors of the pregap, then of the track.
    pub pregap: u32,
    pub sectors: u32,
}

/// Data position measurement: the angle of every `resolution`th sector
/// from `start` on, as the drive that made the image measured them. Copy
/// protections such as StarForce and SecuROM time reads to check that
/// sectors are where the pressed disc has them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dpm {
    pub start: u32,
    pub resolution: u32,
    /// Angles of sectors `start + resolution`, `start + 2 * resolution`
    /// and on, in 256ths of a rotation from sector `start`.
    pub angles: Vec<u32>,
}

/// Reads the descriptor's fields in order.
struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.data.len());
        let end = end.ok_or_else(|| {
            format!(
                "the descriptor ends at byte {}, inside a block",
                self.data.len()
            )
        })?;
        let bytes = &self.data[self.at..end];
        self.at = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i32(&mut self) -> Result<i32, String> {
        Ok(self.u32()? as i32)
    }

    fn skip(&mut self, n: u64) -> Result<(), String> {
        self.take(usize::try_from(n).map_err(|_| "a block is too long".to_string())?)
            .map(|_| ())
    }
}

/// A name the descriptor stores as UTF-16.
fn utf16(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    String::from_utf16_lossy(&units)
        .trim_end_matches('\0')
        .to_string()
}

/// Whether a descriptor's disc type, an MMC profile, is a CD's: the
/// disc info block is then the CD's.
fn is_cd(profile: u16) -> bool {
    matches!(profile, 0x08..=0x0A | 0x20..=0x22 | 0x80..=0x82)
}

/// The descriptor in `data`.
pub fn parse(data: &[u8]) -> Result<Descriptor, String> {
    let mut c = Cursor { data, at: 0 };
    if c.take(16)? != SIGNATURE {
        return Err("it is not a BlindWrite 5 or 6 descriptor".into());
    }
    c.skip(32)?;
    let profile = c.u16()?;
    let sessions = c.u16()?;
    c.skip(12)?;
    let mcn_valid = c.u8()? != 0;
    let mcn = c.take(13)?;
    let mcn = (mcn_valid && mcn.iter().all(u8::is_ascii_digit))
        .then(|| String::from_utf8_lossy(mcn).into_owned());
    c.skip(2 + 16)?;
    let (pma, atip, cdtext, cd_info) = (c.u16()?, c.u16()?, c.u16()?, c.u16()?);
    let bca = c.u32()?;
    c.skip(12)?;
    let (dvd_structures, dvd_info) = (c.u32()?, c.u32()?);
    // Undeciphered bytes, the drive's identification and the volume ID.
    c.skip(32 + 48 + 32)?;
    // The lengths of the data blocks and the sessions follow from their
    // contents, as libMirage reads them.
    let (mode_page, unknown) = (c.u32()?, c.u32()?);
    c.skip(8)?;
    let dpm_len = c.u32()?;
    for len in [
        mode_page,
        unknown,
        pma as u32,
        atip as u32,
        cdtext as u32,
        bca,
        dvd_structures,
    ] {
        c.skip(len as u64)?;
    }
    c.skip(if is_cd(profile) {
        cd_info as u32
    } else {
        dvd_info
    } as u64)?;

    let count = c.u32()?;
    // The folder BlindWrite wrote the files to, on its machine.
    let path_len = c.u32()?;
    c.skip(path_len as u64)?;
    let mut blocks = Vec::new();
    for _ in 0..count {
        // Bit 15 of the first word marks blocks of audio; the tracks
        // say that too.
        c.skip(4)?;
        let bytes = c.u32()?;
        c.skip(16)?;
        let offset = c.u32()? as u64;
        c.skip(12)?;
        let start = c.i32()? as i64;
        let sectors = c.i32()?;
        let name_len = c.u32()?;
        let file = utf16(c.take(name_len as usize)?);
        c.skip(4)?;
        let sectors = u32::try_from(sectors).ok().filter(|&s| s > 0);
        let Some(sectors) = sectors else {
            return Err(format!("data block {} has no sectors", file));
        };
        let sector_size = bytes as u64 / sectors as u64;
        if !matches!(sector_size, 2048 | 2336 | 2352 | 2368 | 2448) {
            return Err(format!(
                "data block {} has sectors of {} bytes, which aren't read",
                file, sector_size
            ));
        }
        blocks.push(DataBlock {
            file,
            offset,
            start,
            sectors,
            sector_size,
        });
    }
    blocks.sort_by_key(|b| b.start);

    let mut list = Vec::new();
    for _ in 0..sessions {
        let number = c.u16()?;
        let entries = c.u8()?;
        c.skip(1 + 4 + 4 + 2 + 2)?;
        let mut tracks = Vec::new();
        for _ in 0..entries {
            if let Some(track) = parse_track(&mut c)? {
                tracks.push(track);
            }
        }
        list.push(Session { number, tracks });
    }

    let dpm = parse_dpm(c.take(dpm_len as usize)?, 32);
    c.skip(4)?;
    if c.take(16)? != FOOTER {
        return Err("the descriptor doesn't end where its blocks say".into());
    }
    Ok(Descriptor {
        mcn,
        blocks,
        sessions: list,
        dpm,
    })
}

/// A track's entry of a session, None for the entries of the TOC's
/// points A0h to A2h.
fn parse_track(c: &mut Cursor) -> Result<Option<Track>, String> {
    let entry = c.take(64)?;
    let kind = match entry[0] {
        0 => return Ok(None),
        1 => TrackType::Audio,
        2 => TrackType::Mode1,
        3 => TrackType::Mode2,
        4 => TrackType::Mode2Form1,
        5 => TrackType::Mode2Form2,
        6 => TrackType::Dvd,
        other => return Err(format!("a track is of type {}, which isn't known", other)),
    };
    // A CD's track entries have 8 more bytes.
    if kind != TrackType::Dvd {
        c.skip(8)?;
    }
    let u32_at = |at: usize| u32::from_le_bytes(entry[at..at + 4].try_into().unwrap());
    let number = entry[12];
    if !(1..=99).contains(&number) {
        return Ok(None);
    }
    let sectors = u32_at(46) as i32;
    let sectors =
        u32::try_from(sectors).map_err(|_| format!("track {} has a negative length", number))?;
    let pregap = u32_at(22);
    if pregap > sectors {
        return Err(format!(
            "track {}'s pregap is longer than the track",
            number
        ));
    }
    Ok(Some(Track {
        number,
        kind,
        control: entry[10] & 0x0F,
        start: u32_at(42) as i32 as i64,
        pregap,
        sectors,
    }))
}

/// DPM data from byte `at` of `data` on: the start sector, resolution,
/// count and angles.
fn parse_dpm(data: &[u8], at: usize) -> Option<Dpm> {
    let word = |i: usize| {
        data.get(at + i * 4..at + i * 4 + 4)
            .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
    };
    let (start, resolution, count) = (word(0)?, word(1)?, word(2)?);
    if resolution == 0 || count == 0 {
        return None;
    }
    let angles = (0..count as usize)
        .map(|i| word(3 + i))
        .collect::<Option<Vec<_>>>()?;
    Some(Dpm {
        start,
        resolution,
        angles,
    })
}

/// The DPM data of a `.bwa` file.
pub fn parse_bwa(data: &[u8]) -> Option<Dpm> {
    parse_dpm(data, 12)
}

/// A data block for `build`: file, offset, first sector, sectors, and
/// sector size.
#[cfg(test)]
pub(crate) type TestBlock<'a> = (&'a str, u32, i32, u32, u32);
/// A track for `build`: type, control, number, first sector, pregap and
/// sectors.
#[cfg(test)]
pub(crate) type TestTrack = (u8, u8, u8, i32, u32, u32);

/// A descriptor as BlindWrite writes one, for tests: of `blocks` and
/// `sessions` of tracks.
#[cfg(test)]
pub(crate) fn build(
    mcn: Option<&str>,
    blocks: &[TestBlock],
    sessions: &[&[TestTrack]],
    dpm: Option<&Dpm>,
) -> Vec<u8> {
    let le16 = |v: u16| v.to_le_bytes();
    let le32 = |v: u32| v.to_le_bytes();
    let utf16 = |s: &str| {
        s.encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<u8>>()
    };

    let mut blocks_data = Vec::new();
    blocks_data.extend(le32(blocks.len() as u32));
    let path = utf16("D:\\");
    blocks_data.extend(le32(path.len() as u32));
    blocks_data.extend(&path);
    for &(file, offset, start, sectors, size) in blocks {
        blocks_data.extend(le32(0x0002_0000));
        blocks_data.extend(le32(sectors * size));
        blocks_data.extend([0; 16]);
        blocks_data.extend(le32(offset));
        blocks_data.extend([0; 12]);
        blocks_data.extend(le32(start as u32));
        blocks_data.extend(le32(sectors));
        let name = utf16(file);
        blocks_data.extend(le32(name.len() as u32));
        blocks_data.extend(&name);
        blocks_data.extend([0; 4]);
    }

    let mut sessions_data = Vec::new();
    for (i, tracks) in sessions.iter().enumerate() {
        // Each session lists points A0h to A2h before its tracks.
        sessions_data.extend(le16(i as u16 + 1));
        sessions_data.push(tracks.len() as u8 + 3);
        sessions_data.push(3);
        sessions_data.extend([0; 12]);
        for point in [0xA0u8, 0xA1, 0xA2] {
            let mut entry = [0u8; 64];
            entry[12] = point;
            sessions_data.extend(entry);
        }
        for &(kind, control, number, start, pregap, sectors) in *tracks {
            let mut entry = [0u8; 72];
            entry[0] = kind;
            entry[10] = control;
            entry[11] = 1;
            entry[12] = number;
            entry[22..26].copy_from_slice(&le32(pregap));
            entry[42..46].copy_from_slice(&le32(start as u32));
            entry[46..50].copy_from_slice(&le32(sectors));
            entry[58..62].copy_from_slice(&le32(i as u32 + 1));
            sessions_data.extend(entry);
        }
    }

    let mut dpm_data = Vec::new();
    if let Some(dpm) = dpm {
        dpm_data.extend([1, 0, 0, 0, 1, 0, 0, 0]);
        dpm_data.extend([0; 24]);
        for word in [dpm.start, dpm.resolution, dpm.angles.len() as u32]
            .iter()
            .chain(&dpm.angles)
        {
            dpm_data.extend(le32(*word));
        }
    }

    let mut out = SIGNATURE.to_vec();
    out.extend([0; 32]);
    out.extend(le16(0x08));
    out.extend(le16(sessions.len() as u16));
    out.extend([0; 12]);
    out.push(mcn.is_some() as u8);
    let mut code = [0u8; 13];
    code[..mcn.map_or(0, str::len)].copy_from_slice(mcn.unwrap_or("").as_bytes());
    out.extend(code);
    out.extend([0; 2 + 16]);
    // No PMA, ATIP or CD-Text; 56 bytes of disc info.
    out.extend([0, 0, 0, 0, 0, 0, 56, 0]);
    out.extend([0; 4 + 12 + 8]);
    out.extend([0; 32 + 48 + 32]);
    for len in [4, 4, blocks_data.len(), sessions_data.len(), dpm_data.len()] {
        out.extend(le32(len as u32));
    }
    // A mode page 2Ah of 4 bytes, the unknown block, the disc info.
    out.extend([0x2A, 2, 0, 0]);
    out.extend([0; 4]);
    out.extend([0; 56]);
    out.extend(blocks_data);
    out.extend(sessions_data);
    out.extend(dpm_data);
    let len = out.len() as u32 + 20;
    out.extend(le32(len));
    out.extend(FOOTER);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_data_track_and_audio_tracks_in_one_file() {
        let data = build(
            Some("0123456789012"),
            &[
                ("GAME.B6I", 0, -150, 1150, 2448),
                ("GAME.B6I", 1150 * 2448, 1000, 400, 2352),
            ],
            &[&[
                (2, 4, 1, -150, 150, 1150),
                (1, 0, 2, 1000, 150, 200),
                (1, 2, 3, 1200, 0, 200),
            ]],
            None,
        );
        let d = parse(&data).unwrap();
        assert_eq!(d.mcn.as_deref(), Some("0123456789012"));
        assert_eq!(d.blocks.len(), 2);
        assert_eq!(
            d.blocks[1],
            DataBlock {
                file: "GAME.B6I".into(),
                offset: 1150 * 2448,
                start: 1000,
                sectors: 400,
                sector_size: 2352
            }
        );
        assert_eq!(
            (d.blocks[0].subchannel(), d.blocks[1].subchannel()),
            (96, 0)
        );
        assert_eq!(d.sessions.len(), 1);
        let tracks = &d.sessions[0].tracks;
        assert_eq!(tracks.len(), 3, "the A0h-A2h points aren't tracks");
        assert_eq!(
            tracks[0],
            Track {
                number: 1,
                kind: TrackType::Mode1,
                control: 4,
                start: -150,
                pregap: 150,
                sectors: 1150
            }
        );
        assert_eq!(
            (tracks[2].kind, tracks[2].control, tracks[2].start),
            (TrackType::Audio, 2, 1200)
        );
        assert_eq!(d.dpm, None);
    }

    #[test]
    fn sessions_and_dpm_data() {
        let dpm = Dpm {
            start: 0,
            resolution: 256,
            angles: vec![700, 1390, 2070],
        };
        let data = build(
            None,
            &[("CD.B6I", 0, 0, 100, 2048), ("CD.B00", 0, 11500, 50, 2048)],
            &[&[(2, 4, 1, 0, 0, 100)], &[(4, 4, 2, 11400, 100, 150)]],
            Some(&dpm),
        );
        let d = parse(&data).unwrap();
        assert_eq!(d.mcn, None);
        assert_eq!(
            d.sessions.iter().map(|s| s.number).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(d.sessions[1].tracks[0].kind, TrackType::Mode2Form1);
        assert_eq!(d.dpm, Some(dpm.clone()));

        let mut bwa = vec![1, 0, 0, 0, 8, 0, 0, 0, 1, 0, 0, 0];
        for word in [0u32, 256, 3, 700, 1390, 2070] {
            bwa.extend(word.to_le_bytes());
        }
        assert_eq!(parse_bwa(&bwa), Some(dpm));
        assert_eq!(parse_bwa(&bwa[..30]), None, "fewer angles than it says");
    }

    #[test]
    fn errors() {
        let good = build(
            None,
            &[("A.B6I", 0, 0, 10, 2352)],
            &[&[(2, 4, 1, 0, 0, 10)]],
            None,
        );
        assert!(parse(&good).is_ok());
        assert!(parse(b"FILE \"x.bin\" BINARY").is_err());
        assert!(parse(&good[..good.len() - 1]).is_err(), "no footer");
        for cut in [20, 300, good.len() - 30] {
            assert!(parse(&good[..cut]).is_err(), "cut at {}", cut);
        }
        let odd = build(
            None,
            &[("A.B6I", 0, 0, 10, 2000)],
            &[&[(2, 4, 1, 0, 0, 10)]],
            None,
        );
        assert!(parse(&odd).is_err(), "sectors of 2000 bytes");
        let unknown = build(
            None,
            &[("A.B6I", 0, 0, 10, 2352)],
            &[&[(9, 4, 1, 0, 0, 10)]],
            None,
        );
        assert!(parse(&unknown).is_err(), "a track of type 9");
    }
}
