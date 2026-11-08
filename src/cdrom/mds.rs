//! Alcohol 120% images: a media descriptor (`.mds`) that lists the
//! disc's sessions and tracks and the file that holds their sectors
//! (`.mdf`), with the disc's DPM data when Alcohol measured it.
//!
//! Alcohol never published the layout. This reads version 1 descriptors
//! of CDs as libMirage's MDS parser does (CDEmu's `image-mds`, by Rok
//! Mandeljc, from Henrik Stokseth's reverse engineering, GPL-2.0 or
//! later; written from its description of the layout, not ported) and
//! Aaru's Alcohol 120% image (`Aaru.Images/Alcohol120`, by Natalia
//! Portillo), which agree on it. Everything is little-endian:
//!
//! ```text
//!   0  "MEDIA DESCRIPTOR"
//!  16  u8 major and minor version, u16 medium type (0 to 2 are CDs),
//!      u16 sessions
//!  80  u32 offset of the session blocks, u32 of the DPM table (0: none)
//! ```
//!
//! A session block (24 bytes) has its first and last sector (i32), its
//! number (u16), its count of track blocks (u8), and at 20 the offset of
//! those blocks. A track block (80 bytes) has the track's mode (low
//! nibble), its subchannel (0, or 8 for 96 interleaved bytes of P-W after
//! each sector), the ADR and control byte, the point of the TOC entry
//! (the track's number, or A0h to A2h), at 12 the offset of its extra
//! block (u32) and the bytes from one sector to the next (u16), at 36 the
//! sector of its INDEX 01 (u32) and that sector's offset in the data file
//! (u64), the number of data files (u32) and the offset of their footers
//! (u32). An extra block is the pregap's sectors and the track's (u32
//! each); a footer (16 bytes) is the offset of the file's name and
//! whether the name is UTF-16 (u32 each). Alcohol names the file `*.mdf`:
//! the descriptor's own name with that extension.
//!
//! The DPM table is a u32 count of blocks and a u32 offset for each. A
//! block starts with its type, u32: 1 is DPM data (`dpm::parse_block`
//! after the type); Aaru also writes 2, a list of the sectors it couldn't
//! read. libMirage reads only the table's first block, whatever its type,
//! and Aaru keeps the first valid DPM block; this keeps the first valid
//! block of type 1, so it reads what Alcohol, Aaru and the
//! refurbished-dinosaurs-toolkit disc archiver write (one DPM block, or a
//! DPM block among others).

use super::cue::TrackMode;
use super::dpm::{self, Dpm};

/// A descriptor starts with this.
pub const SIGNATURE: &[u8; 16] = b"MEDIA DESCRIPTOR";
/// The type of the DPM table's blocks of DPM data.
const DPM_BLOCK: u32 = 1;
const SESSION_BLOCK: usize = 24;
const TRACK_BLOCK: usize = 80;
/// Bytes of P-W subchannel a track of subchannel mode 8 stores after
/// each sector.
const SUBCHANNEL: u64 = 96;

/// What a descriptor says of the disc.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Descriptor {
    pub sessions: Vec<Session>,
    /// The first valid block of DPM data (type 1) of the descriptor's
    /// DPM table.
    pub dpm: Option<Dpm>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub number: u16,
    /// The session's tracks, in the order of their blocks.
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub number: u8,
    pub mode: TrackMode,
    /// The control bits of its TOC entry and Q subchannel: 4 for data,
    /// 2 for digital copy permitted, 1 for pre-emphasis.
    pub control: u8,
    /// Bytes from one sector to the next in the data file: the sector,
    /// then 96 bytes of interleaved P-W if `subchannel`.
    pub stride: u64,
    pub subchannel: bool,
    /// The sector of its INDEX 01.
    pub start: u32,
    /// Where sector `start` is in the data file.
    pub offset: u64,
    /// Sectors of the pregap before `start`.
    pub pregap: u32,
    /// Sectors from `start` on.
    pub sectors: u32,
    /// The data file's name, as the descriptor has it (`data_file`).
    pub file: String,
}

impl Descriptor {
    /// The data files' names, as the descriptor has them, each once.
    pub fn files(&self) -> Vec<&str> {
        let mut files: Vec<&str> = Vec::new();
        for track in self.sessions.iter().flat_map(|s| &s.tracks) {
            if !files.contains(&track.file.as_str()) {
                files.push(&track.file);
            }
        }
        files
    }
}

/// The name of the data file `name` names, for the descriptor named
/// `descriptor`: `*.mdf` (or another extension) is the descriptor's own
/// name with that extension, as libMirage and Aaru read it; any other
/// name is the file's.
pub fn data_file(descriptor: &str, name: &str) -> String {
    match name.strip_prefix("*.") {
        Some(extension) => {
            let file = descriptor.rsplit(['/', '\\']).next().unwrap_or(descriptor);
            let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
            format!("{}.{}", stem, extension)
        }
        None => name.to_string(),
    }
}

/// Little-endian reads at offsets of the descriptor, which fail saying
/// what ran past its end.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn bytes(&self, at: usize, n: usize, what: &str) -> Result<&'a [u8], String> {
        at.checked_add(n)
            .and_then(|end| self.0.get(at..end))
            .ok_or_else(|| {
                format!(
                    "the descriptor ends at byte {}, before the {} at byte {}",
                    self.0.len(),
                    what,
                    at
                )
            })
    }

    fn u32(&self, at: usize, what: &str) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            self.bytes(at, 4, what)?.try_into().unwrap(),
        ))
    }

    fn u16(&self, at: usize, what: &str) -> Result<u16, String> {
        Ok(u16::from_le_bytes(
            self.bytes(at, 2, what)?.try_into().unwrap(),
        ))
    }

    /// The name at byte `at`, up to its terminating zero: UTF-16 if
    /// `wide`, else one byte a character.
    fn name(&self, at: usize, wide: bool) -> Result<String, String> {
        let rest = self.0.get(at..).unwrap_or_default();
        let unterminated = || format!("the data file's name at byte {} has no end", at);
        if wide {
            let units: Vec<u16> = rest
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .take_while(|&u| u != 0)
                .collect();
            if units.len() * 2 + 2 > rest.len() {
                return Err(unterminated());
            }
            Ok(String::from_utf16_lossy(&units))
        } else {
            let end = rest.iter().position(|&b| b == 0).ok_or_else(unterminated)?;
            Ok(rest[..end].iter().map(|&b| b as char).collect())
        }
    }
}

/// The descriptor in `data`.
pub fn parse(data: &[u8]) -> Result<Descriptor, String> {
    let r = Reader(data);
    if r.bytes(0, 16, "signature")? != SIGNATURE {
        return Err("it is not an Alcohol 120% media descriptor".into());
    }
    let version = r.bytes(16, 2, "header")?;
    if version[0] != 1 {
        // Version 2 descriptors (DAEMON Tools' MDX) can be encrypted,
        // and neither libMirage nor Aaru reads them.
        return Err(format!(
            "it is a version {}.{} descriptor; only version 1, which Alcohol 120% writes, is read",
            version[0], version[1]
        ));
    }
    let medium = r.u16(18, "header")?;
    if medium > 2 {
        return Err(format!(
            "it describes a medium of type {:#04x}, which isn't a CD",
            medium
        ));
    }
    let count = r.u16(20, "header")?;
    let sessions_at = r.u32(80, "header")? as usize;
    let dpm_at = r.u32(84, "header")? as usize;

    // The offsets add with saturation. On a 32-bit host (the browser
    // build), an offset near 4 GiB would otherwise wrap round to the
    // descriptor's start and read other fields; saturated, the read fails
    // as one past the end.
    let mut sessions = Vec::new();
    for s in 0..count as usize {
        let at = sessions_at.saturating_add(s * SESSION_BLOCK);
        let block = r.bytes(at, SESSION_BLOCK, "session block")?;
        let number = u16::from_le_bytes([block[8], block[9]]);
        let blocks = block[10] as usize;
        let blocks_at = u32::from_le_bytes(block[20..24].try_into().unwrap()) as usize;
        let mut tracks = Vec::new();
        for b in 0..blocks {
            if let Some(track) = parse_track(&r, blocks_at.saturating_add(b * TRACK_BLOCK))? {
                tracks.push(track);
            }
        }
        sessions.push(Session { number, tracks });
    }
    let dpm = match dpm_at {
        0 => None,
        at => parse_dpm_table(&r, at),
    };
    Ok(Descriptor { sessions, dpm })
}

/// The track of the block at byte `at`, None for a block of another
/// point of the TOC (A0h to A2h, and B0h and C0h of a disc of several
/// sessions).
fn parse_track(r: &Reader, at: usize) -> Result<Option<Track>, String> {
    let block = r.bytes(at, TRACK_BLOCK, "track block")?;
    let number = block[4];
    if !(1..=99).contains(&number) {
        return Ok(None);
    }
    let u32_at = |i: usize| u32::from_le_bytes(block[i..i + 4].try_into().unwrap());
    let stride = u16::from_le_bytes([block[16], block[17]]) as u64;
    let subchannel = match block[1] {
        0 => false,
        8 => true,
        other => {
            return Err(format!(
                "track {} has subchannel of mode {:#04x}, which isn't read",
                number, other
            ));
        }
    };
    let size = stride.saturating_sub(if subchannel { SUBCHANNEL } else { 0 });
    // The low nibble of the mode, as libMirage reads it, the values 8
    // and up as those 8 below: 1 audio, 2 Mode 1, 0, 3 and 7 Mode 2, 4
    // Mode 2 Form 1, 5 Mode 2 Form 2. Alcohol writes A9h for audio, AAh
    // for Mode 1, ECh for Mode 2 Form 1.
    let kind = block[0] & 0x07;
    let mode = match (kind, size) {
        (1, 2352) => TrackMode::Audio,
        (2, 2352) => TrackMode::Mode1_2352,
        (2 | 4, 2048) => TrackMode::Mode1_2048,
        (0 | 3 | 4 | 5 | 7, 2352) => TrackMode::Mode2_2352,
        (0 | 3 | 4 | 5 | 7, 2336) => TrackMode::Mode2_2336,
        _ => {
            return Err(format!(
                "track {} is of mode {:#04x} in sectors of {} bytes, which isn't read",
                number, block[0], size
            ));
        }
    };
    let extra = u32_at(12) as usize;
    if extra == 0 {
        return Err(format!("track {} has no pregap and length", number));
    }
    let (pregap, sectors) = (
        r.u32(extra, "track's extra block")?,
        r.u32(extra.saturating_add(4), "track's extra block")?,
    );
    let files = u32_at(48);
    let footer = u32_at(52) as usize;
    // Aaru takes a track without a footer to be in `*.mdf`.
    let file = match (files, footer) {
        (_, 0) | (0, _) => "*.mdf".to_string(),
        (1, at) => {
            let name = r.u32(at, "track's footer")? as usize;
            let wide = r.u32(at.saturating_add(4), "track's footer")? != 0;
            r.name(name, wide)?
        }
        (n, _) => {
            return Err(format!(
                "track {} is split into {} data files, which isn't read",
                number, n
            ));
        }
    };
    Ok(Some(Track {
        number,
        mode,
        control: block[2] & 0x0F,
        stride,
        subchannel,
        start: u32_at(36),
        offset: u64::from_le_bytes(block[40..48].try_into().unwrap()),
        pregap,
        sectors,
        file,
    }))
}

/// The first valid DPM block of the table at byte `at`. A table or
/// block that runs past the descriptor's end, or a block `Dpm::new`
/// doesn't take, is passed over, as Aaru passes it over; the disc then
/// reads without it.
fn parse_dpm_table(r: &Reader, at: usize) -> Option<Dpm> {
    let word = |at: usize| r.u32(at, "DPM table").ok();
    let count = word(at)? as usize;
    // Aaru reads no block of a table whose offsets run past the end.
    let offsets = at.checked_add(4)?;
    r.bytes(offsets, count.checked_mul(4)?, "DPM table").ok()?;
    (0..count)
        .filter_map(|i| word(offsets + i * 4))
        .filter(|&block| word(block as usize) == Some(DPM_BLOCK))
        .find_map(|block| dpm::parse_block(r.0, block as usize + 4))
}

/// A track for `build`: mode byte, subchannel mode, number, INDEX 01
/// sector, offset in the data file, pregap and sectors. Its sectors are
/// 2352 bytes, and 96 more with subchannel.
#[cfg(test)]
pub(crate) type TestTrack = (u8, u8, u8, u32, u64, u32, u32);

/// A descriptor laid out as Alcohol 120% writes one, for tests: of
/// `sessions` of tracks, all in the data file `file`, with a DPM table
/// of `blocks` (each a type and the words after it) if there are any.
#[cfg(test)]
pub(crate) fn build(sessions: &[&[TestTrack]], file: &str, blocks: &[(u32, &[u32])]) -> Vec<u8> {
    let le16 = |v: u16| v.to_le_bytes();
    let le32 = |v: u32| v.to_le_bytes();
    let tracks: usize = sessions.iter().map(|s| s.len() + 3).sum();
    let sessions_at = 88;
    let blocks_at = sessions_at + sessions.len() * SESSION_BLOCK;
    let extras_at = blocks_at + tracks * TRACK_BLOCK;
    let footer_at = extras_at + tracks * 8;
    let name_at = footer_at + 16;
    let name: Vec<u8> = file
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    let table_at = name_at + name.len();

    let mut out = SIGNATURE.to_vec();
    out.extend([1, 5]);
    out.extend(le16(0));
    out.extend(le16(sessions.len() as u16));
    out.extend([2, 0]);
    out.resize(80, 0);
    out.extend(le32(sessions_at as u32));
    out.extend(le32(if blocks.is_empty() {
        0
    } else {
        table_at as u32
    }));

    let mut block_list = Vec::new();
    let mut extras = Vec::new();
    let mut index = 0;
    for (s, tracks) in sessions.iter().enumerate() {
        out.extend(le32(-150i32 as u32));
        out.extend(le32(0));
        out.extend(le16(s as u16 + 1));
        out.push(tracks.len() as u8 + 3);
        out.push(3);
        out.extend(le16(tracks[0].2 as u16));
        out.extend(le16(tracks[tracks.len() - 1].2 as u16));
        out.extend(le32(0));
        out.extend(le32((blocks_at + index * TRACK_BLOCK) as u32));
        for point in [0xA0u8, 0xA1, 0xA2] {
            let mut block = [0u8; TRACK_BLOCK];
            block[2] = 0x14;
            block[4] = point;
            block[12..16].copy_from_slice(&le32((extras_at + index * 8) as u32));
            block_list.push(block);
            extras.extend([0; 8]);
            index += 1;
        }
        for &(mode, sub, number, start, offset, pregap, sectors) in *tracks {
            let mut block = [0u8; TRACK_BLOCK];
            block[0] = mode;
            block[1] = sub;
            block[2] = if mode & 7 == 1 { 0x10 } else { 0x14 };
            block[4] = number;
            block[12..16].copy_from_slice(&le32((extras_at + index * 8) as u32));
            let stride = 2352 + if sub == 8 { 96 } else { 0 };
            block[16..18].copy_from_slice(&le16(stride));
            block[18] = 2;
            block[36..40].copy_from_slice(&le32(start));
            block[40..48].copy_from_slice(&offset.to_le_bytes());
            block[48..52].copy_from_slice(&le32(1));
            block[52..56].copy_from_slice(&le32(footer_at as u32));
            block_list.push(block);
            extras.extend(le32(pregap));
            extras.extend(le32(sectors));
            index += 1;
        }
    }
    for block in block_list {
        out.extend(block);
    }
    out.extend(extras);
    out.extend(le32(name_at as u32));
    out.extend(le32(1));
    out.extend([0; 8]);
    out.extend(name);

    if !blocks.is_empty() {
        out.extend(le32(blocks.len() as u32));
        let mut at = table_at + 4 + blocks.len() * 4;
        for (_, words) in blocks {
            out.extend(le32(at as u32));
            at += 4 + words.len() * 4;
        }
        for (kind, words) in blocks {
            out.extend(le32(*kind));
            for word in *words {
                out.extend(le32(*word));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A data track and an audio track after its 150-sector pregap, as
    /// Alcohol stores them: the pregap in the data file.
    const DISC: [TestTrack; 2] = [
        (0xAA, 0, 1, 0, 0, 150, 1000),
        (0xA9, 0, 2, 1150, 1150 * 2352, 150, 300),
    ];

    #[test]
    fn tracks_without_dpm() {
        let d = parse(&build(&[&DISC], "*.mdf", &[])).unwrap();
        assert_eq!(d.dpm, None);
        assert_eq!(d.sessions.len(), 1);
        let tracks = &d.sessions[0].tracks;
        assert_eq!(tracks.len(), 2, "the A0h-A2h points aren't tracks");
        assert_eq!(
            tracks[1],
            Track {
                number: 2,
                mode: TrackMode::Audio,
                control: 0,
                stride: 2352,
                subchannel: false,
                start: 1150,
                offset: 1150 * 2352,
                pregap: 150,
                sectors: 300,
                file: "*.mdf".into(),
            }
        );
        assert_eq!(
            (tracks[0].mode, tracks[0].control),
            (TrackMode::Mode1_2352, 4)
        );
        assert_eq!(d.files(), ["*.mdf"]);
        assert_eq!(data_file("D:\\Games\\Grom.MDS", "*.mdf"), "Grom.mdf");
        assert_eq!(data_file("disc", "*.MDF"), "disc.MDF");
        assert_eq!(data_file("Grom.mds", "Other.mdf"), "Other.mdf");
    }

    #[test]
    fn modes_and_subchannel() {
        let tracks = [
            (0xEC, 8, 1, 0, 0, 150, 10),
            (0xAB, 0, 2, 160, 10 * 2448, 0, 10),
        ];
        let d = parse(&build(&[&tracks], "game.mdf", &[])).unwrap();
        let t = &d.sessions[0].tracks;
        assert_eq!(
            (t[0].mode, t[0].stride, t[0].subchannel),
            (TrackMode::Mode2_2352, 2448, true)
        );
        assert_eq!(t[1].mode, TrackMode::Mode2_2352);
        assert_eq!(t[0].file, "game.mdf");

        let bad = [(0xAA, 4, 1, 0, 0, 150, 10)];
        let error = parse(&build(&[&bad], "*.mdf", &[])).unwrap_err();
        assert!(error.contains("subchannel of mode 0x04"), "{}", error);
        let bad = [(0xA6, 0, 1, 0, 0, 150, 10)];
        let error = parse(&build(&[&bad], "*.mdf", &[])).unwrap_err();
        assert!(error.contains("mode 0xa6"), "{}", error);
    }

    #[test]
    fn a_dpm_block() {
        let words: &[u32] = &[0, 256, 3, 700, 1390, 2070];
        let d = parse(&build(&[&DISC], "*.mdf", &[(1, words)])).unwrap();
        assert_eq!(
            d.dpm,
            Some(Dpm {
                start: 0,
                resolution: 256,
                angles: vec![700, 1390, 2070]
            })
        );
        // The table is the descriptor's last part; DPM data doesn't move
        // the tracks.
        assert_eq!(
            d.sessions,
            parse(&build(&[&DISC], "*.mdf", &[])).unwrap().sessions
        );
    }

    #[test]
    fn the_first_valid_dpm_block_of_several() {
        let errors: &[u32] = &[4, 1, 2, 7, 9];
        let first: &[u32] = &[150, 50, 2, 600, 1200];
        let second: &[u32] = &[0, 10, 1, 99];
        let going_back: &[u32] = &[0, 10, 2, 600, 300];
        let d = parse(&build(
            &[&DISC],
            "*.mdf",
            &[(2, errors), (1, first), (1, second)],
        ))
        .unwrap();
        assert_eq!(
            d.dpm.map(|d| (d.start, d.angles)),
            Some((150, vec![600, 1200]))
        );
        let d = parse(&build(&[&DISC], "*.mdf", &[(1, going_back), (1, second)])).unwrap();
        assert_eq!(
            d.dpm.map(|d| d.angles),
            Some(vec![99]),
            "the invalid block is passed over"
        );
        let d = parse(&build(&[&DISC], "*.mdf", &[(2, errors)])).unwrap();
        assert_eq!(d.dpm, None, "only a list of read errors");
    }

    #[test]
    fn truncated_data() {
        let words: &[u32] = &[0, 256, 3, 700, 1390, 2070];
        let whole = build(&[&DISC], "*.mdf", &[(1, words)]);
        // A DPM table cut short reads as none; the tracks still read.
        for cut in [4, 12, 30] {
            let d = parse(&whole[..whole.len() - cut]).unwrap();
            assert_eq!(d.dpm, None, "cut by {}", cut);
            assert_eq!(d.sessions[0].tracks.len(), 2);
        }
        // A descriptor cut short before its tracks doesn't read.
        for cut in [10, 50, 100, 300, 500] {
            assert!(parse(&whole[..cut]).is_err(), "cut at {}", cut);
        }
        let mut huge = whole.clone();
        let table = u32::from_le_bytes(whole[84..88].try_into().unwrap()) as usize;
        huge[table..table + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            parse(&huge).unwrap().dpm,
            None,
            "a count of blocks past the end"
        );
    }

    #[test]
    fn descriptors_that_are_refused() {
        let good = build(&[&DISC], "*.mdf", &[]);
        assert!(parse(&good).is_ok());
        assert!(parse(b"BWT5 STREAM SIGN and more").is_err());
        let mut v2 = good.clone();
        v2[16] = 2;
        assert!(parse(&v2).unwrap_err().contains("version 2"));
        let mut dvd = good.clone();
        dvd[18] = 0x10;
        assert!(parse(&dvd).unwrap_err().contains("isn't a CD"));
        // Session blocks at the last offset a descriptor can name run
        // past its end, on a 32-bit host too.
        let mut far = good.clone();
        far[80..84].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&far).unwrap_err().contains("session block"));
    }
}
