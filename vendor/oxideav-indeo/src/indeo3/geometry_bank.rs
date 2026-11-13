//! Indeo 3 cell-geometry banks (`spec/04 §1.1` / `§5.3`, `spec/03
//! §4.2`; Extractor round 18, Validator round 19).
//!
//! The binary-tree walker never carries a cell's extent in the
//! bitstream: each leaf's rectangle is looked up from a per-plane
//! **cell-geometry bank** — five sub-tables indexed by the walker's
//! vertical (`cl`) and horizontal (`ch`) heap indices — that the
//! vendor populates once per `ICDecompressBegin` as a pure function
//! of the picture size. This module re-executes that populator
//! (`IR32_32.DLL!0x100038f0`, the rule of `spec/04 §5.3`) and is
//! cross-checked entry for entry against the staged
//! `tables/04-cell-geometry-banks.csv` (four geometries × four banks
//! × indices 1..255) in `tests/indeo3_geometry_banks.rs`.
//!
//! Heap indices: both trees start at 1; a split doubles the index
//! (first child `2k`, visited first) and the sibling is `2k + 1`. An
//! `H_SPLIT` (`spec/03 §2.2` code `00`) descends the vertical tree
//! (`cl`), a `V_SPLIT` (`01`) the horizontal tree (`ch`).
//!
//! Each plane has two banks: the **full-strip** bank and the
//! **last-strip** bank (built for the residual width of the last
//! strip), selected per cell by whether the cell's strip is the
//! picture's last one (`spec/03 §4.2`).

/// Row stride of the vendor's strip pixel buffers (`0xb0`); the
/// vertical offset table is expressed in these units.
pub const STRIP_ROW_STRIDE: u32 = 0xb0;

/// Luma strip width in pixels (`spec/02 §4.1`).
pub const LUMA_STRIP_WIDTH: u32 = 160;

/// Chroma strip width in pixels (`spec/02 §4.1`).
pub const CHROMA_STRIP_WIDTH: u32 = 40;

/// The "node under 4 rows / pixels" code in the `h4` / `w4` tables.
pub const UNDER_FOUR_CODE: u8 = 0x63;

/// The "node under 4 rows" vertical offset (`99 999`), which faults
/// the walker (`spec/03 §4.2`).
pub const UNDER_FOUR_YPOS: u32 = 0xf423f;

/// One `0xb00`-byte cell-geometry bank (`spec/04 §1.1`), indexed by
/// heap index `1..=255` (index 0 is never written by the populator).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeometryBank {
    /// `bank + 0x000[cl]` — cell height / 4, or [`UNDER_FOUR_CODE`].
    pub h4: [u8; 256],
    /// `bank + 0x100[ch]` — cell width / 4, or [`UNDER_FOUR_CODE`].
    pub w4: [u8; 256],
    /// `bank + 0x200[ch]` — strip-context slot of the horizontal node.
    pub strip: [u8; 256],
    /// `bank + 0x300[cl]` — byte offset of the node's top row
    /// (`row × 0xb0`), or [`UNDER_FOUR_YPOS`].
    pub ypos: [u32; 256],
    /// `bank + 0x700[ch]` — byte offset of the node's left column
    /// within its strip, or 0 for a node under 4 pixels.
    pub xpos: [u32; 256],
}

/// The two banks of one plane plus the strip geometry they encode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneBanks {
    /// The full-strip bank.
    pub full: GeometryBank,
    /// The last-strip bank (residual width).
    pub last: GeometryBank,
    /// Number of strips across the plane.
    pub nstrips: u32,
    /// Full strip width (160 luma / 40 chroma).
    pub full_strip: u32,
    /// Width of the last strip (`((width − 1) mod full) + 1`).
    pub last_strip: u32,
    /// Plane width / height the banks were built for.
    pub width: u32,
    /// Plane height.
    pub height: u32,
}

/// Smallest power of two `>= n` (`n >= 1`).
fn pow2ceil(n: u32) -> u32 {
    n.max(1).next_power_of_two()
}

/// `spec/04 §5.3` split rule: the two children of a node of extent `p`.
/// Returns `(first, second)`; the first child is the top / left one.
pub fn split_extent(p: u32) -> (u32, u32) {
    if p > 8 {
        // The horizontal loop keeps the first child as 8 bits.
        let first = (8 * ((p + 8) / 16)) & 0xff;
        (first, p.wrapping_sub(first))
    } else {
        let second = p / 2;
        (p - second, second)
    }
}

/// `spec/02 §6` chroma plane geometry: the vendor pads the 4:1
/// subsampled dimensions up to a multiple of 4.
pub fn chroma_plane_dims(width: u32, height: u32) -> (u32, u32) {
    (((width >> 2) + 3) & !3, ((height >> 2) + 3) & !3)
}

/// Run the populator once (`spec/04 §5.3`) for one bank.
///
/// * `height` — plane height in rows.
/// * `this_strip` — width of the strip this bank describes.
/// * `width` — plane width (for the strip count).
/// * `full_strip` — full strip width of the plane.
/// * `wide` — the three-strip flag passed to the last-strip banks
///   (`320 < luma width <= 480`).
/// * `post_step` — the full-strip banks' `3 → 2` slot rewrite for
///   wide pictures.
fn populate(
    height: u32,
    this_strip: u32,
    width: u32,
    full_strip: u32,
    wide: bool,
    post_step: bool,
) -> GeometryBank {
    let nstrips = width.div_ceil(full_strip).max(1);
    let p_level = pow2ceil(nstrips);
    let l_level = if wide { 2 } else { p_level };

    // Extents, heap-indexed.
    let mut vx = [0u32; 256];
    let mut vy = [0u32; 256];
    // Horizontal seeding at the strip level.
    for k in 1..(2 * l_level).min(256) {
        vx[k as usize] = if k >= l_level { this_strip } else { 0 };
    }
    for k in l_level..128 {
        let (a, b) = split_extent(vx[k as usize]);
        vx[(2 * k) as usize] = a;
        vx[(2 * k + 1) as usize] = b;
    }
    // Vertical seeding with the plane height.
    vy[1] = height;
    for k in 1..128 {
        let (a, b) = split_extent(vy[k as usize]);
        vy[(2 * k) as usize] = a;
        vy[(2 * k + 1) as usize] = b;
    }

    // Positions: running sums in heap order over the *stored* table
    // (the populator fills the table in place, so an under-four node's
    // override — 0 for x, 0xf423f for y — feeds the next index's sum),
    // reset to 0 exactly when the sum reaches the strip width (x) or
    // the plane's byte height (y). Codes: the seeds are written as
    // extent / 4; every node from 2 up carries the under-four code.
    let mut h4 = [0u8; 256];
    let mut w4 = [0u8; 256];
    let mut xpos = [0u32; 256];
    let mut ypos = [0u32; 256];
    let y_limit = STRIP_ROW_STRIDE * height;
    for k in 1..256usize {
        if k >= 2 {
            let sx = xpos[k - 1] + vx[k - 1];
            xpos[k] = if sx == this_strip { 0 } else { sx };
            let sy = ypos[k - 1] + vy[k - 1] * STRIP_ROW_STRIDE;
            ypos[k] = if sy == y_limit { 0 } else { sy };
        }
        let under_x = k >= 2 && vx[k] < 4;
        let under_y = k >= 2 && vy[k] < 4;
        w4[k] = if under_x {
            UNDER_FOUR_CODE
        } else {
            (vx[k] / 4) as u8
        };
        h4[k] = if under_y {
            UNDER_FOUR_CODE
        } else {
            (vy[k] / 4) as u8
        };
        if under_x {
            xpos[k] = 0;
        }
        if under_y {
            ypos[k] = UNDER_FOUR_YPOS;
        }
    }

    // Strip slots: 2 above the strip level, the strip index at the
    // strip level (the wide flag clamps the fourth leaf to strip 2),
    // inherited below it.
    let mut strip = [0u8; 256];
    for k in 1..256usize {
        let kk = k as u32;
        strip[k] = if kk < p_level {
            2
        } else if kk < 2 * p_level {
            if wide && kk == p_level + 3 {
                2
            } else {
                (kk - p_level) as u8
            }
        } else {
            strip[k >> 1]
        };
    }
    if post_step {
        for s in strip.iter_mut() {
            if *s == 3 {
                *s = 2;
            }
        }
    }

    GeometryBank {
        h4,
        w4,
        strip,
        ypos,
        xpos,
    }
}

impl PlaneBanks {
    /// Build the two banks of a plane of `width × height` samples
    /// (`spec/04 §1.1`'s four populator calls). `luma_width` is the
    /// picture's luma width, which sets the wide-picture flag for
    /// both planes.
    pub fn build(width: u32, height: u32, full_strip: u32, luma_width: u32) -> Self {
        let nstrips = width.div_ceil(full_strip).max(1);
        let last_strip = ((width.max(1) - 1) % full_strip) + 1;
        let wide = luma_width > 320 && luma_width <= 480;
        let full = populate(height, full_strip, width, full_strip, false, wide);
        let last = populate(height, last_strip, width, full_strip, wide, false);
        PlaneBanks {
            full,
            last,
            nstrips,
            full_strip,
            last_strip,
            width,
            height,
        }
    }

    /// The luma banks of a `width × height` picture.
    pub fn luma(width: u32, height: u32) -> Self {
        Self::build(width, height, LUMA_STRIP_WIDTH, width)
    }

    /// The chroma banks of a `width × height` picture (chroma plane
    /// `chroma_plane_dims`, strip 40).
    pub fn chroma(width: u32, height: u32) -> Self {
        let (cw, ch) = chroma_plane_dims(width, height);
        Self::build(cw, ch, CHROMA_STRIP_WIDTH, width)
    }

    /// The bank a cell in strip slot `slot` reads (`spec/03 §4.2`):
    /// the last-strip bank when the slot is the picture's last strip.
    pub fn bank_for_slot(&self, slot: u8) -> &GeometryBank {
        if u32::from(slot) + 1 >= self.nstrips {
            &self.last
        } else {
            &self.full
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_rule() {
        assert_eq!(split_extent(160), (80, 80));
        assert_eq!(split_extent(120), (64, 56));
        assert_eq!(split_extent(56), (32, 24));
        assert_eq!(split_extent(24), (16, 8));
        assert_eq!(split_extent(16), (8, 8));
        assert_eq!(split_extent(8), (4, 4));
        assert_eq!(split_extent(4), (2, 2));
        assert_eq!(split_extent(5), (3, 2));
        assert_eq!(split_extent(0), (0, 0));
    }

    #[test]
    fn chroma_dims_pad_to_four() {
        assert_eq!(chroma_plane_dims(160, 120), (40, 32));
        assert_eq!(chroma_plane_dims(176, 144), (44, 36));
        assert_eq!(chroma_plane_dims(320, 240), (80, 60));
    }

    #[test]
    fn luma_160x120_head() {
        // tables/04-cell-geometry-banks.csv rows for 160x120 luma full.
        let b = PlaneBanks::luma(160, 120);
        assert_eq!(b.nstrips, 1);
        let f = &b.full;
        assert_eq!(
            (f.h4[1], f.w4[1], f.strip[1], f.ypos[1], f.xpos[1]),
            (30, 40, 0, 0, 0)
        );
        assert_eq!(
            (f.h4[3], f.w4[3], f.ypos[3], f.xpos[3]),
            (14, 20, 11264, 80)
        );
        assert_eq!(
            (f.h4[7], f.w4[7], f.ypos[7], f.xpos[7]),
            (6, 10, 16896, 120)
        );
        assert_eq!((f.h4[8], f.w4[8]), (4, 6));
        // Single strip: slot 0 is the last strip.
        assert!(std::ptr::eq(b.bank_for_slot(0), &b.last));
    }

    #[test]
    fn luma_176x144_last_strip_bank() {
        let b = PlaneBanks::luma(176, 144);
        assert_eq!(b.nstrips, 2);
        assert_eq!(b.last_strip, 16);
        let l = &b.last;
        assert_eq!((l.h4[1], l.w4[1], l.strip[1]), (36, 0, 2));
        assert_eq!((l.h4[2], l.w4[2], l.strip[2]), (18, 4, 0));
        assert_eq!((l.h4[3], l.w4[3], l.strip[3], l.ypos[3]), (18, 4, 1, 12672));
        assert_eq!(
            (l.h4[5], l.w4[5], l.strip[5], l.ypos[5], l.xpos[5]),
            (8, 2, 0, 7040, 8)
        );
        assert!(std::ptr::eq(b.bank_for_slot(1), &b.last));
        assert!(std::ptr::eq(b.bank_for_slot(0), &b.full));
    }

    #[test]
    fn wide_400x300_full_bank_slots() {
        let b = PlaneBanks::luma(400, 300);
        let f = &b.full;
        assert_eq!((f.h4[1], f.w4[1], f.strip[1]), (75, 0, 2));
        assert_eq!((f.h4[2], f.w4[2], f.strip[2]), (38, 99, 2));
        assert_eq!(
            (f.h4[3], f.w4[3], f.strip[3], f.ypos[3]),
            (37, 99, 2, 26752)
        );
        assert_eq!((f.w4[4], f.strip[4]), (40, 0));
        assert_eq!((f.w4[5], f.strip[5], f.ypos[5]), (40, 1, 14080));
        assert_eq!((f.w4[7], f.strip[7]), (40, 2));
    }
}
