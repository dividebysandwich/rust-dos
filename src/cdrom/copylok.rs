//! CopyLok, also called CodeLok: PAN Technology's (Panlok's) ring
//! protection of about 1999 to 2001. The pressed disc has a band of
//! sectors a drive can't read, a few hundred to a few thousand of them,
//! and the game reads some of them and runs only when the reads fail.
//!
//! The emulator needs nothing of its own to pass the check: an image of
//! whole 2352-byte sectors keeps those sectors as Redump's dumps have them
//! (redumper writes sync and header and fills the rest with 55h, so the
//! EDC doesn't match), and such a sector reads as an error
//! (`super::edc_fails`). This module only recognizes the disc, so that
//! opening an image can say whether it can pass the check: an image of
//! 2048-byte sectors can't keep the band, and the game then refuses the
//! disc.
//!
//! The protection marks the primary volume descriptor. Its application
//! use field (bytes 883 to 1394) holds 29 bytes that start with the dword
//! 4ED38AE1h and zeros after them, and the 653 reserved bytes after the
//! field are zero. The variants differ in the 29 bytes; which is which
//! follows BinaryObjectScanner's `Protection/CopyLok.cs` (MIT), from the
//! Redump entries it lists.

use super::DATA_SECTOR;

/// Where the application use field of a primary volume descriptor starts.
const APPLICATION_USE: usize = 883;
/// Where the reserved bytes after it start.
const RESERVED: usize = 1395;
/// The bytes of the application use field CopyLok writes.
const MARK_LEN: usize = 29;
/// The dword CopyLok's mark starts with.
const MARK: u32 = 0x4ED3_8AE1;

/// The variants of CopyLok, by how many sectors their band has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// The early one: about 1,850 sectors that fail.
    Early,
    /// About 255 sectors.
    Fewer,
    /// About 1,500, in pairs.
    Pairs,
    /// About 775, one at a time.
    Solo,
    /// The mark, with fields no known variant has.
    Unknown,
}

impl Variant {
    pub fn describe(self) -> &'static str {
        match self {
            Variant::Early => "early variant, about 1,850 unreadable sectors",
            Variant::Fewer => "about 255 unreadable sectors",
            Variant::Pairs => "about 1,500 unreadable sectors, in pairs",
            Variant::Solo => "about 775 unreadable sectors",
            Variant::Unknown => "unknown variant",
        }
    }
}

/// The CopyLok variant whose mark `pvd`, the disc's primary volume
/// descriptor, carries, if it carries one.
pub fn variant(pvd: &[u8; DATA_SECTOR]) -> Option<Variant> {
    if pvd[0] != 1 || &pvd[1..6] != b"CD001" {
        return None;
    }
    let field = &pvd[APPLICATION_USE..RESERVED];
    if field[MARK_LEN..].iter().any(|&b| b != 0) || pvd[RESERVED..].iter().any(|&b| b != 0) {
        return None;
    }
    let u16_at = |at: usize| u16::from_le_bytes([field[at], field[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes(field[at..at + 4].try_into().unwrap());
    if u32_at(0) != MARK {
        return None;
    }
    let size = u16_at(4);
    let second = u16_at(6);
    let zero = field[12];
    let early = u16_at(13);
    let pair = u16_at(15);
    let one = u32_at(17);
    let early_two = u32_at(21);
    let pair_two = u32_at(25);
    if early == 0 {
        return Some(match (pair, one, early_two, pair_two) {
            (0, 0, 0, 0) => Variant::Early,
            _ => Variant::Unknown,
        });
    }
    if second != 0x4ED3 || zero != 0 || early != 0x0C76 || one != 1 {
        return Some(Variant::Unknown);
    }
    // BinaryObjectScanner has one sample of these, with 6999h here; the
    // others have D1ADh.
    if size < 0xADD1 {
        return Some(Variant::Fewer);
    }
    Some(match (pair, pair_two) {
        (0x9425, p) if p != 0 => Variant::Pairs,
        (0xF3ED, 0) => Variant::Solo,
        _ => Variant::Unknown,
    })
}

/// A primary volume descriptor with CopyLok's mark: `mark`, the first 29
/// bytes of its application use field.
#[cfg(test)]
pub(crate) fn marked_pvd(mark: &[u8; MARK_LEN]) -> [u8; DATA_SECTOR] {
    let mut pvd = [0u8; DATA_SECTOR];
    pvd[0] = 1;
    pvd[1..6].copy_from_slice(b"CD001");
    pvd[6] = 1;
    pvd[APPLICATION_USE..APPLICATION_USE + MARK_LEN].copy_from_slice(mark);
    pvd
}

/// The 29 bytes of the early variant's mark.
#[cfg(test)]
pub(crate) fn early_mark() -> [u8; MARK_LEN] {
    let mut mark = [0u8; MARK_LEN];
    mark[..4].copy_from_slice(&MARK.to_le_bytes());
    mark[4..6].copy_from_slice(&0xD1ADu16.to_le_bytes());
    mark
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mark of the variants after the early one, with `size` and the
    /// two pair fields.
    fn later_mark(size: u16, pair: u16, pair_two: u32) -> [u8; MARK_LEN] {
        let mut mark = early_mark();
        mark[4..6].copy_from_slice(&size.to_le_bytes());
        mark[6..8].copy_from_slice(&0x4ED3u16.to_le_bytes());
        mark[13..15].copy_from_slice(&0x0C76u16.to_le_bytes());
        mark[15..17].copy_from_slice(&pair.to_le_bytes());
        mark[17..21].copy_from_slice(&1u32.to_le_bytes());
        mark[25..29].copy_from_slice(&pair_two.to_le_bytes());
        mark
    }

    #[test]
    fn the_variants() {
        let of = |mark: [u8; MARK_LEN]| variant(&marked_pvd(&mark));
        assert_eq!(of(early_mark()), Some(Variant::Early));
        assert_eq!(of(later_mark(0x6999, 0, 0)), Some(Variant::Fewer));
        assert_eq!(of(later_mark(0xD1AD, 0x9425, 0x1234)), Some(Variant::Pairs));
        assert_eq!(of(later_mark(0xD1AD, 0xF3ED, 0)), Some(Variant::Solo));
        assert_eq!(of(later_mark(0xD1AD, 0x9425, 0)), Some(Variant::Unknown));
        let mut odd = early_mark();
        odd[15] = 1;
        assert_eq!(of(odd), Some(Variant::Unknown));
    }

    #[test]
    fn discs_without_the_mark() {
        // A plain descriptor.
        assert_eq!(variant(&marked_pvd(&[0; MARK_LEN])), None);
        // Another first dword.
        let mut mark = early_mark();
        mark[0] ^= 1;
        assert_eq!(variant(&marked_pvd(&mark)), None);
        // More in the application use field, or in the reserved bytes.
        let mut pvd = marked_pvd(&early_mark());
        pvd[APPLICATION_USE + MARK_LEN] = 1;
        assert_eq!(variant(&pvd), None);
        let mut pvd = marked_pvd(&early_mark());
        pvd[DATA_SECTOR - 1] = 1;
        assert_eq!(variant(&pvd), None);
        // Not a primary volume descriptor.
        let mut pvd = marked_pvd(&early_mark());
        pvd[0] = 2;
        assert_eq!(variant(&pvd), None);
    }
}
