//! The metrics of Windows 98 SE's TrueType faces, Arial, Times New Roman
//! and Courier New, as its GDI measures them, without their outlines.
//! `truetype.bin` (tools/gen_truetype_metrics.py) has, for each of the
//! twelve fonts (each face regular, bold, italic and bold italic), the
//! numbers of its TrueType file that the metrics come from, the ascent
//! and descent in pixels at each pixel size (its VDMX table's), and the
//! widths Windows 98's rasterizer gives the characters of code pages
//! 1250 to 1254 and 1257 at the sizes up to 128 pixels where its hinting
//! moves them off the scaled advance. GDI's text then takes as much room
//! as on Windows 98, though `font` draws it with the ROM font's glyphs.

use std::sync::OnceLock;

static DATA: &[u8] = include_bytes!("truetype.bin");

/// The families, in `truetype.bin`'s order.
pub const ARIAL: usize = 0;
pub const TIMES_NEW_ROMAN: usize = 1;
pub const COURIER_NEW: usize = 2;

/// One of the twelve fonts.
struct Font {
    units_per_em: i32,
    win_ascent: i32,
    win_descent: i32,
    /// The line gap less what usWinAscent and usWinDescent take beyond
    /// the typographic ascent and descent, in font units.
    external: i32,
    /// xAvgCharWidth.
    average: i32,
    /// The width of the bounding box of all the glyphs.
    box_width: i32,
    /// The advance of the missing glyph, drawn for a character the font
    /// lacks.
    missing: i32,
    /// The advance of each of `Data::chars`.
    advances: Vec<u16>,
    /// The ascent and descent in pixels at the pixel sizes 1 to 255.
    heights: Vec<(u8, u8)>,
    /// For the pixel sizes from 1 up, the characters whose hinted width
    /// isn't their advance scaled, ascending: each its index in
    /// `Data::chars` (bits 0-8) and how much wider it is (bits 9-15,
    /// signed).
    hinted: Vec<Vec<u16>>,
}

struct Data {
    /// The characters the fonts have advances for, ascending.
    chars: Vec<u16>,
    fonts: Vec<Font>,
}

struct Reader {
    at: usize,
}

impl Reader {
    fn u8(&mut self) -> u8 {
        self.at += 1;
        DATA[self.at - 1]
    }

    fn u16(&mut self) -> u16 {
        self.at += 2;
        u16::from_le_bytes([DATA[self.at - 2], DATA[self.at - 1]])
    }

    fn i16(&mut self) -> i32 {
        self.u16() as i16 as i32
    }
}

fn data() -> &'static Data {
    static PARSED: OnceLock<Data> = OnceLock::new();
    PARSED.get_or_init(|| {
        assert_eq!(&DATA[..4], b"W98T");
        let mut r = Reader { at: 4 };
        let n = r.u16() as usize;
        let chars: Vec<u16> = (0..n).map(|_| r.u16()).collect();
        let fonts = (0..12)
            .map(|_| {
                let units_per_em = r.u16() as i32;
                let win_ascent = r.u16() as i32;
                let win_descent = r.u16() as i32;
                let external = r.i16();
                let average = r.u16() as i32;
                let box_width = r.u16() as i32;
                let missing = r.u16() as i32;
                let advances = (0..n).map(|_| r.u16()).collect();
                let heights = (0..255).map(|_| (r.u8(), r.u8())).collect();
                let sizes = r.u8() as usize;
                let hinted = (0..sizes)
                    .map(|_| {
                        let count = r.u16() as usize;
                        (0..count).map(|_| r.u16()).collect()
                    })
                    .collect();
                Font {
                    units_per_em,
                    win_ascent,
                    win_descent,
                    external,
                    average,
                    box_width,
                    missing,
                    advances,
                    heights,
                    hinted,
                }
            })
            .collect();
        assert_eq!(r.at, DATA.len());
        Data { chars, fonts }
    })
}

/// The font of the family `family` (`ARIAL`, `TIMES_NEW_ROMAN` or
/// `COURIER_NEW`) that Windows draws a font of this weight and slant
/// with: the bold one from 600 up.
pub fn style(family: usize, bold: bool, italic: bool) -> usize {
    family * 4 + bold as usize + 2 * italic as usize
}

/// `v` font units at `ppem` pixels to the em, rounded.
fn scale(f: &Font, v: i32, ppem: i32) -> i32 {
    (v * ppem + f.units_per_em / 2).div_euclid(f.units_per_em)
}

/// The height in pixels of the font `f` at `ppem` pixels to the em, and
/// its ascent.
fn height_at(f: &Font, ppem: i32) -> (i32, i32) {
    match f.heights.get(ppem as usize - 1) {
        Some(&(a, d)) => (a as i32 + d as i32, a as i32),
        None => {
            let a = scale(f, f.win_ascent, ppem);
            (a + scale(f, f.win_descent, ppem), a)
        }
    }
}

/// The pixel size a font `height` pixels high gets (a cell height, as a
/// positive lfHeight asks for). As Windows 98 finds it: among the sizes
/// VDMX has, from 8 pixels up, the first exactly as high, or else the
/// last lower; below them, the size the height scales to, made smaller
/// while it is higher than asked and one larger when that one is exactly
/// as high.
fn ppem_for_height(f: &Font, height: i32) -> i32 {
    const VDMX_FIRST: i32 = 8;
    if height >= height_at(f, VDMX_FIRST).0 {
        let mut last = VDMX_FIRST;
        for p in VDMX_FIRST..=255 {
            let h = height_at(f, p).0;
            if h == height {
                return p;
            }
            if h > height {
                return last;
            }
            last = p;
        }
        // Higher than the table goes: scaled.
        let cell = (f.win_ascent + f.win_descent) as i64;
        return (height as i64 * f.units_per_em as i64 / cell).clamp(255, 1024) as i32;
    }
    let cell = f.win_ascent + f.win_descent;
    let mut p = ((2 * height * f.units_per_em + cell) / (2 * cell)).max(1);
    while p > 1 && height_at(f, p).0 > height {
        p -= 1;
    }
    if height_at(f, p).0 < height && height_at(f, p + 1).0 == height {
        p += 1;
    }
    p
}

/// A font of one of the faces at one size, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    /// The font (`style`).
    pub style: usize,
    /// Pixels to the em, upward and across: across it is scaled by the
    /// width asked for.
    pub ppem: i32,
    pub x_ppem: i32,
    pub height: i32,
    pub ascent: i32,
    pub internal: i32,
    pub external: i32,
    /// tmAveCharWidth and tmMaxCharWidth.
    pub average: i32,
    pub max: i32,
}

/// The font `style` as a logical font `device_height` high (0 its
/// default 12 points, negative its em) and `device_width` wide (0 its
/// own width) gets it in pixels. A width asked for makes the font that
/// many times its average width wider than its own and is its average
/// width, as on Windows 98.
pub fn size(style: usize, device_height: i32, device_width: i32) -> Size {
    let f = &data().fonts[style];
    let ppem = match device_height {
        0 => 16,
        h if h < 0 => h.saturating_neg(),
        h => ppem_for_height(f, h),
    }
    .clamp(1, 1024);
    let own = scale(f, f.average, ppem);
    let (x_ppem, average, max) = if device_width > 0 && own > 0 {
        let w = device_width.min(1024);
        let (em, own64) = (f.units_per_em as i64, own as i64);
        let max = (f.box_width as i64 * (ppem * w) as i64 * 2 + em * own64) / (2 * em * own64);
        ((ppem * w / own).max(1), w, max as i32)
    } else {
        (ppem, own, scale(f, f.box_width, ppem))
    };
    // VDMX's heights are those of its 1:1 records. A font scaled across
    // has its ascent and descent scaled and rounded: Windows 98 takes
    // them from the records for 2:1 and 5:3 where the scale is near
    // those, which `truetype.bin` leaves out.
    let (height, ascent) = if x_ppem == ppem {
        height_at(f, ppem)
    } else {
        let a = scale(f, f.win_ascent, ppem);
        (a + scale(f, f.win_descent, ppem), a)
    };
    Size {
        style,
        ppem,
        x_ppem,
        height,
        ascent,
        internal: height - ppem,
        external: scale(f, f.external, ppem).max(0),
        average,
        max,
    }
}

/// The font `style` at `x_ppem` pixels to the em across, as far as
/// `advance` needs it.
pub fn across(style: usize, x_ppem: i32) -> Size {
    Size {
        x_ppem,
        ..size(style, -x_ppem, 0)
    }
}

/// How far the character `c` moves on in a font of the size `s`; `None`
/// is a character the font lacks, drawn with its missing glyph.
pub fn advance(s: &Size, c: Option<u16>) -> i32 {
    let d = data();
    let f = &d.fonts[s.style];
    let Some(i) = c.and_then(|c| d.chars.binary_search(&c).ok()) else {
        return scale(f, f.missing, s.x_ppem);
    };
    let scaled = scale(f, f.advances[i] as i32, s.x_ppem);
    if let Some(hinted) = f.hinted.get(s.x_ppem as usize - 1)
        && let Ok(k) = hinted.binary_search_by_key(&(i as u16), |&e| e & 0x1FF)
    {
        // The 7 bits of the difference, sign extended.
        return scaled + ((hinted[k] as i16) >> 9) as i32;
    }
    scaled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widths(s: &Size, text: &str) -> Vec<i32> {
        text.chars().map(|c| advance(s, Some(c as u16))).collect()
    }

    fn extent(s: &Size, text: &str) -> i32 {
        widths(s, text).iter().sum()
    }

    /// What Windows 98 SE's GetTextMetricsA gives Arial at the sizes
    /// CyberStorm 2 asks for (lfHeight -13, -16 and -21) and at a
    /// positive height.
    #[test]
    fn arial_metrics_are_windows_98s() {
        let s = size(style(ARIAL, false, false), -13, 0);
        assert_eq!(
            (s.height, s.ascent, s.internal, s.external, s.average, s.max),
            (16, 13, 3, 0, 6, 35)
        );
        let s = size(style(ARIAL, false, false), -16, 0);
        assert_eq!(
            (s.height, s.ascent, s.internal, s.external, s.average, s.max),
            (18, 15, 2, 1, 7, 43)
        );
        let s = size(style(ARIAL, true, false), -21, 0);
        assert_eq!(
            (s.height, s.ascent, s.internal, s.external, s.average, s.max),
            (24, 19, 3, 1, 10, 56)
        );
        // A cell 16 high is 13 pixels to the em, the first size VDMX
        // has that high; 20 is the last lower than it, 17.
        assert_eq!(size(style(ARIAL, false, false), 16, 0).ppem, 13);
        assert_eq!(size(style(ARIAL, true, false), 20, 0).ppem, 17);
        assert_eq!(size(style(ARIAL, false, false), 0, 0).height, 18);
    }

    /// GetTextExtentPoint32A of CyberStorm 2's labels in Windows 98 SE.
    #[test]
    fn arial_extents_are_windows_98s() {
        let s = size(style(ARIAL, false, false), -13, 0);
        assert_eq!(extent(&s, "Sound Effects"), 81);
        assert_eq!(extent(&s, "Extreme Confirmation"), 125);
        assert_eq!(extent(&s, "Extreme Information"), 116);
        assert_eq!(extent(&s, "CUC (Celestine UltraCryo Inc.)"), 176);
        let s = size(style(ARIAL, true, false), -16, 0);
        assert_eq!(extent(&s, "Hercbase Ambient Animations"), 230);
        let s = size(style(ARIAL, true, false), -21, 0);
        assert_eq!(extent(&s, "The quick brown fox jumps over the lazy dog"), 453);
    }

    /// Hinting moves widths off the scaled advance: Arial's M is 9
    /// pixels at 10 to the em where the scaled advance rounds to 8.
    #[test]
    fn hinted_widths_are_windows_98s() {
        let s = size(style(ARIAL, false, false), -10, 0);
        assert_eq!(widths(&s, "MT"), [9, 7]);
        let s = size(style(COURIER_NEW, false, false), -13, 0);
        assert!(widths(&s, "iW@").iter().all(|&w| w == 8));
    }

    /// A width asked for scales the font across: Arial -13 at 12 wide
    /// is hinted at 26 pixels across.
    #[test]
    fn a_width_asked_for_scales_across() {
        let s = size(style(ARIAL, false, false), -13, 12);
        assert_eq!((s.x_ppem, s.average, s.max), (26, 12, 70));
        assert_eq!(widths(&s, "W"), [26]);
    }

    /// Sizes no program asks for, as large as the arguments go, stop at
    /// 1024 pixels without overflowing.
    #[test]
    fn huge_sizes_stop_at_1024_pixels() {
        for h in [i32::MAX, i32::MIN, 100_000] {
            let s = size(style(TIMES_NEW_ROMAN, true, true), h, i32::MAX);
            assert!(s.ppem <= 1024 && s.x_ppem >= 1 && s.max > 0, "{h}: {s:?}");
            assert!(advance(&s, Some(u16::from(b'W'))) > 0);
        }
    }
}
