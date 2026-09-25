//! CD-ROM images: CUE sheets with their BIN and WAV tracks, and bare ISO,
//! BIN or IMG images of a data track; the ISO 9660 file system on them.

pub mod audio;
pub mod cue;
#[cfg(feature = "cdaudio")]
pub mod decoded;
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
}
