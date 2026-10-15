//! CD-ROM images: CUE sheets with their BIN and WAV tracks, CHD files, and
//! bare ISO, BIN or IMG images of a data track; the ISO 9660 file system on
//! them.

pub mod audio;
#[cfg(feature = "chd")]
pub mod chd;
pub mod cue;
#[cfg(feature = "cdaudio")]
pub mod decoded;
pub mod folder;
pub mod image;
pub mod iso9660;

/// Bytes of user data in a data sector.
pub const DATA_SECTOR: usize = 2048;
/// Bytes of a whole sector, as audio tracks and raw reads have it.
pub const RAW_SECTOR: usize = 2352;
/// Sectors per second, the "frames" of Red Book addresses.
pub const FRAMES_PER_SECOND: u32 = 75;
/// Sector 0 (LBA 0) is at 00:02:00: the two-second pregap of track 1.
pub const LBA_OFFSET: u32 = 150;

/// The sync pattern that starts every raw data sector.
pub const SYNC: [u8; 12] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];

/// The EDC's CRC table: polynomial x^32 + x^31 + x^16 + x^15 + x^4 + x^3
/// + x + 1, bit-reversed.
const EDC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut e = i as u32;
        let mut bit = 0;
        while bit < 8 {
            e = (e >> 1) ^ if e & 1 != 0 { 0xD801_8001 } else { 0 };
            bit += 1;
        }
        table[i] = e;
        i += 1;
    }
    table
};

/// The error detection code a data sector stores over `data`.
pub fn edc(data: &[u8]) -> u32 {
    data.iter().fold(0, |e, &b| (e >> 8) ^ EDC_TABLE[((e ^ b as u32) & 0xFF) as usize])
}

/// Whether a drive can't read this whole data sector: its sync and header
/// make it a Mode 1 or Mode 2 sector, and the EDC stored in it doesn't
/// match its data. Copy protections master such sectors on purpose and
/// check that reading them fails. Sectors without the sync (audio, gaps),
/// and those with an EDC of 0, which images made without EDC and ECC have,
/// read as they are.
pub fn edc_fails(raw: &[u8; RAW_SECTOR]) -> bool {
    if raw[..12] != SYNC {
        return false;
    }
    let check = |covered: std::ops::Range<usize>| {
        let at = covered.end;
        let stored = u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]);
        stored != 0 && edc(&raw[covered]) != stored
    };
    match raw[15] {
        1 => check(0..2064),
        // XA sectors repeat their subheader; the submode's bit 5 is Form 2.
        2 if raw[16..20] == raw[20..24] => check(if raw[18] & 0x20 == 0 { 16..2072 } else { 16..2348 }),
        _ => false,
    }
}

/// Minute, second and frame of a sector.
pub fn lba_to_msf(lba: u32) -> (u8, u8, u8) {
    let frames = lba + LBA_OFFSET;
    let seconds = frames / FRAMES_PER_SECOND;
    ((seconds / 60) as u8, (seconds % 60) as u8, (frames % FRAMES_PER_SECOND) as u8)
}

/// The sector at minute, second and frame (none before 00:02:00).
pub fn msf_to_lba(minute: u32, second: u32, frame: u32) -> u32 {
    ((minute * 60 + second) * FRAMES_PER_SECOND + frame).saturating_sub(LBA_OFFSET)
}

/// A sector's Red Book address as MSCDEX passes it in a dword: the frame
/// in the low byte, then the second and the minute.
pub fn redbook(lba: u32) -> u32 {
    let (m, s, f) = lba_to_msf(lba);
    f as u32 | (s as u32) << 8 | (m as u32) << 16
}

/// The sector a Red Book address dword names.
pub fn redbook_to_lba(address: u32) -> u32 {
    msf_to_lba(address >> 16 & 0xFF, address >> 8 & 0xFF, address & 0xFF)
}

/// Where a file is on a CD, with what its directory entry says about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    /// First sector of the data.
    pub lba: u32,
    /// Length in bytes.
    pub size: u32,
    /// DOS time and date.
    pub time: u16,
    pub date: u16,
    pub hidden: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(lba_to_msf(0), (0, 2, 0));
        assert_eq!(lba_to_msf(103_153), (22, 57, 28));
        assert_eq!(msf_to_lba(22, 57, 28), 103_153);
        assert_eq!(redbook(0), 0x00_02_00);
        assert_eq!(redbook_to_lba(redbook(4711)), 4711);
        assert_eq!(msf_to_lba(0, 0, 0), 0);
    }

    /// A Mode 1 sector at 00:02:16 (LBA 16) whose data is `fill`, with its
    /// EDC.
    fn mode1(fill: u8) -> [u8; RAW_SECTOR] {
        let mut raw = [0u8; RAW_SECTOR];
        raw[..12].copy_from_slice(&SYNC);
        raw[12..16].copy_from_slice(&[0x00, 0x02, 0x16, 1]);
        raw[16..16 + DATA_SECTOR].fill(fill);
        let edc = edc(&raw[..2064]);
        raw[2064..2068].copy_from_slice(&edc.to_le_bytes());
        raw
    }

    #[test]
    fn edc_of_known_data() {
        // The CRC's check value, and a sector worked out with another
        // implementation of the EDC.
        assert_eq!(edc(b"123456789"), 0x6EC2_EDC4);
        assert_eq!(edc(&[0u8; 2064]), 0);
        assert_eq!(edc(&mode1(0x55)[..2064]), 0x0B2F_7816);
    }

    #[test]
    fn sectors_whose_edc_fails() {
        let mut raw = mode1(0x55);
        assert!(!edc_fails(&raw));
        raw[100] ^= 1;
        assert!(edc_fails(&raw), "a byte of data changed");
        raw[2064..2068].fill(0);
        assert!(!edc_fails(&raw), "an image without EDC");
        raw[2064] = 1;
        raw[0] = 0x12;
        assert!(!edc_fails(&raw), "no sync: not a data sector");

        // Mode 2: Form 1 has its EDC after 2048 bytes of data, Form 2 at
        // the end of the sector.
        for (submode, covered) in [(0x08u8, 16..2072usize), (0x28, 16..2348)] {
            let mut raw = [0x33u8; RAW_SECTOR];
            raw[..12].copy_from_slice(&SYNC);
            raw[15] = 2;
            raw[16..24].copy_from_slice(&[0, 0, submode, 0, 0, 0, submode, 0]);
            let edc = edc(&raw[covered.clone()]);
            raw[covered.end..covered.end + 4].copy_from_slice(&edc.to_le_bytes());
            assert!(!edc_fails(&raw), "submode {:02X}", submode);
            raw[30] ^= 0x80;
            assert!(edc_fails(&raw), "submode {:02X}", submode);
        }
    }
}
