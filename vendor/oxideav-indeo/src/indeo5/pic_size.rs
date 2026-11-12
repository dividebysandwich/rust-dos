//! Indeo 5 standard picture-size tables.
//!
//! Spec source: `docs/video/indeo/indeo5/spec/02-gop-and-band-layer.md`
//! §1.6, with the numeric values taken from the Extractor dumps
//! `tables/region_10088c90_u32.csv` (widths, `.rdata 0x10088c90`) and
//! `tables/region_10088cd0_u32.csv` (heights, `.rdata 0x10088cd0`).
//!
//! The GOP header's 4-bit `pic_size_id` (`spec/02 §1.6`) selects an
//! entry in two parallel 16-entry `u32` tables. Values `0..=14` look
//! up a preset dimension; value `15` triggers a custom 13+13-bit read
//! handled by the GOP parser, not this table. Indices `12..=14` are
//! zero on disk (unused slots); the parser does not special-case them,
//! so a stream selecting one yields a `0x0` picture, surfaced as a
//! distinct error by the GOP layer.

/// Spec/02 §1.6 — preset picture widths, indexed by `pic_size_id`
/// `0..=15`. Index 15 is `0` (custom-dimension sentinel; the GOP
/// parser reads explicit dimensions instead of consulting this table).
/// Values verbatim from `tables/region_10088c90_u32.csv`.
pub const PIC_WIDTHS: [u32; 16] = [
    640, 320, 160, 704, 352, 352, 176, 240, 640, 704, 80, 88, 0, 0, 0, 0,
];

/// Spec/02 §1.6 — preset picture heights, indexed by `pic_size_id`
/// `0..=15`. Values verbatim from `tables/region_10088cd0_u32.csv`.
/// The audit confirmed index 3 = 480 against the wiki's erroneous 224
/// (`spec/02 §1.6` audit note).
pub const PIC_HEIGHTS: [u32; 16] = [
    480, 240, 120, 480, 240, 288, 144, 180, 240, 240, 60, 72, 0, 0, 0, 0,
];

/// Spec/02 §1.6 — the `pic_size_id` value that triggers a custom
/// 13+13-bit dimension read instead of a table lookup.
pub const PIC_SIZE_ID_CUSTOM: u32 = 15;

/// Look up the `(width, height)` for a preset `pic_size_id`.
///
/// Returns `None` for the custom sentinel (`15`) — the caller must
/// read explicit dimensions — and for the unused zero slots
/// (`12..=14`), where both table entries are `0`.
pub fn lookup(pic_size_id: u32) -> Option<(u32, u32)> {
    if pic_size_id >= 16 || pic_size_id == PIC_SIZE_ID_CUSTOM {
        return None;
    }
    let w = PIC_WIDTHS[pic_size_id as usize];
    let h = PIC_HEIGHTS[pic_size_id as usize];
    if w == 0 || h == 0 {
        return None;
    }
    Some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cif_and_qcif() {
        // 352x288 (CIF) is pic_size_id 5; 176x144 (QCIF) is 6.
        assert_eq!(lookup(5), Some((352, 288)));
        assert_eq!(lookup(6), Some((176, 144)));
    }

    #[test]
    fn audit_corrected_index_3() {
        // spec/02 §1.6 audit: pic_size_id 3 = 704x480 (not the wiki's 224).
        assert_eq!(lookup(3), Some((704, 480)));
    }

    #[test]
    fn custom_sentinel_is_none() {
        assert_eq!(lookup(PIC_SIZE_ID_CUSTOM), None);
    }

    #[test]
    fn unused_zero_slots_are_none() {
        assert_eq!(lookup(12), None);
        assert_eq!(lookup(13), None);
        assert_eq!(lookup(14), None);
    }

    #[test]
    fn all_presets_aligned_to_four() {
        // Every nonzero preset is a multiple of 4 (matches the §2.2
        // alignment the format descriptor enforces).
        for id in 0..16 {
            if let Some((w, h)) = lookup(id) {
                assert_eq!(w & 3, 0, "width for id {id} not /4");
                assert_eq!(h & 3, 0, "height for id {id} not /4");
            }
        }
    }
}
