//! TrueType fonts from files, as AddFontResourceA adds them: a program's
//! own .TTF (or each font of a .TTC). GDI draws them as Windows 98 does
//! without font smoothing: the outlines hinted by the font's own
//! instructions where it has them, at the size the logical font asks
//! for, and filled one bit a pixel. Each character moves on by its width
//! in the font's `hdmx` table at that size, else by its hinted advance,
//! else by its advance scaled.
//!
//! The metrics are the file's as Windows 98 scales them: usWinAscent and
//! usWinDescent make the cell, the em is the cell less the internal
//! leading, and xAvgCharWidth is tmAveCharWidth.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ab_glyph_rasterizer::{Point, Rasterizer, point};
use skrifa::outline::{DrawSettings, Engine, HintingInstance, HintingOptions, OutlinePen, Target};
use skrifa::raw::TableProvider;
use skrifa::raw::tables::os2::SelectionFlags;
use skrifa::string::StringId;
use skrifa::{FontRef, GlyphId, MetadataProvider, instance::LocationRef, instance::Size};

/// The fonts of a collection read at most.
const MOST_FONTS: u32 = 16;
/// The widest a glyph is drawn, in pixels: a row is a u64.
const MOST_WIDTH: i32 = 64;
/// The largest size drawn, in pixels to the em.
const MOST_PPEM: i32 = 255;

/// A TrueType font of a file: the file's bytes, which a save state
/// keeps, the font's index in them, and what GDI reads of it.
pub struct Ttf {
    pub data: Vec<u8>,
    pub index: u32,
    /// The family's name, which a logical font asks for.
    pub face: String,
    pub weight: i32,
    pub italic: bool,
    /// The font has a symbol cmap (3,0): its characters are bytes, in
    /// SYMBOL_CHARSET.
    pub symbol: bool,
    /// The family (FF_ROMAN, FF_SWISS...) and VARIABLE_PITCH or
    /// FIXED_PITCH, as a LOGFONT has them.
    pub pitch_family: u8,
    /// ulCodePageRange1: the character sets the font has, as a
    /// FONTSIGNATURE's code page bits (0 when the table has none).
    pub code_pages: u32,
    units_per_em: i32,
    win_ascent: i32,
    win_descent: i32,
    /// The line gap less what usWinAscent and usWinDescent take beyond
    /// the typographic ascent and descent, in font units.
    external: i32,
    average: i32,
    box_width: i32,
    /// The glyphs drawn, by `Size::key` and the code page of the bytes.
    pub(super) drawn: RefCell<HashMap<(SizeKey, u16), Rc<super::font::Glyphs>>>,
}

impl std::fmt::Debug for Ttf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ttf({:?}, font {})", self.face, self.index)
    }
}

/// A size of a font, in pixels: what a glyph's drawing depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SizeKey {
    pub ppem: i32,
    pub x_ppem: i32,
    pub bold: bool,
}

/// A font at a size, as TEXTMETRIC has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub key: SizeKey,
    pub height: i32,
    pub ascent: i32,
    pub internal: i32,
    pub external: i32,
    pub average: i32,
    pub max: i32,
}

/// A glyph drawn: its rows from the cell's top, the leftmost pixel in bit
/// 0 at `offset` from the pen, and how far it moves on.
pub struct Drawn {
    pub rows: Vec<u64>,
    pub advance: i32,
    pub offset: i32,
}

/// `v` font units at `ppem` pixels to the em, rounded.
fn scale(v: i32, ppem: i32, upem: i32) -> i32 {
    (v * ppem + upem / 2).div_euclid(upem)
}

impl Ttf {
    /// The fonts of a TrueType file or collection: none if the bytes are
    /// neither.
    pub fn fonts_of_file(data: &[u8]) -> Vec<Ttf> {
        (0..MOST_FONTS)
            .map_while(|i| FontRef::from_index(data, i).ok().map(|_| i))
            .filter_map(|i| Ttf::parse(data.to_vec(), i))
            .collect()
    }

    /// The font `index` of the file `data`, or None if it isn't a
    /// TrueType font GDI can draw.
    pub fn parse(data: Vec<u8>, index: u32) -> Option<Ttf> {
        let font = FontRef::from_index(&data, index).ok()?;
        let head = font.head().ok()?;
        let upem = head.units_per_em() as i32;
        if !(16..=16384).contains(&upem) {
            return None;
        }
        // Outlines GDI fills: TrueType's, not CFF's.
        font.glyf().ok()?;
        font.loca(None).ok()?;
        let hhea = font.hhea().ok()?;
        let face = font
            .localized_strings(StringId::FAMILY_NAME)
            .english_or_first()?
            .to_string();
        if face.is_empty() {
            return None;
        }
        let symbol = font.cmap().ok()?.encoding_records().iter().any(|r| {
            r.platform_id() == skrifa::raw::tables::cmap::PlatformId::Windows
                && r.encoding_id() == 0
        });
        let (typo_ascent, typo_descent) = (
            hhea.ascender().to_i16() as i32,
            hhea.descender().to_i16() as i32,
        );
        let line_gap = hhea.line_gap().to_i16() as i32;
        let fixed = font.post().is_ok_and(|p| p.is_fixed_pitch() != 0);
        let os2 = font.os2().ok();
        let (win_ascent, win_descent) = match &os2 {
            Some(o) => (o.us_win_ascent() as i32, o.us_win_descent() as i32),
            None => (typo_ascent, -typo_descent),
        };
        if win_ascent + win_descent <= 0 {
            return None;
        }
        let box_width = head.x_max() as i32 - head.x_min() as i32;
        let family = os2.as_ref().map_or(0, |o| family_of(o.panose_10(), fixed));
        let pitch = if fixed { 1 } else { 2 };
        let average = match &os2 {
            Some(o) if o.x_avg_char_width() > 0 => o.x_avg_char_width() as i32,
            _ => box_width / 2,
        };
        Some(Ttf {
            face,
            index,
            weight: os2.as_ref().map_or(400, |o| o.us_weight_class() as i32),
            italic: os2
                .as_ref()
                .is_some_and(|o| o.fs_selection().contains(SelectionFlags::ITALIC))
                || head.mac_style().bits() & 2 != 0,
            symbol,
            pitch_family: family | pitch,
            code_pages: os2
                .as_ref()
                .and_then(|o| o.ul_code_page_range_1())
                .unwrap_or(0),
            units_per_em: upem,
            win_ascent,
            win_descent,
            external: (line_gap - ((win_ascent + win_descent) - (typo_ascent - typo_descent)))
                .max(0),
            average,
            box_width,
            drawn: RefCell::new(HashMap::new()),
            data,
        })
    }

    /// The height in pixels of the font at `ppem` pixels to the em, and
    /// its ascent.
    fn height_at(&self, ppem: i32) -> (i32, i32) {
        let a = scale(self.win_ascent, ppem, self.units_per_em);
        (a + scale(self.win_descent, ppem, self.units_per_em), a)
    }

    /// The pixel size a cell `height` pixels high gets: the size it
    /// scales to, made smaller while the font is higher than asked and
    /// one larger when that one is exactly as high.
    fn ppem_for_height(&self, height: i32) -> i32 {
        let cell = self.win_ascent + self.win_descent;
        let mut p = ((2 * height * self.units_per_em + cell) / (2 * cell)).max(1);
        while p > 1 && self.height_at(p).0 > height {
            p -= 1;
        }
        if self.height_at(p).0 < height && self.height_at(p + 1).0 == height {
            p += 1;
        }
        p
    }

    /// The font as a logical font `device_height` high (0 its default 12
    /// points, negative its em) and `device_width` wide (0 its own width)
    /// gets it, made bold with `bold`. A width asked for makes the font
    /// that many times its average width wider than its own.
    pub fn metrics(&self, device_height: i32, device_width: i32, bold: bool) -> Metrics {
        let upem = self.units_per_em;
        let ppem = match device_height {
            0 => 16,
            h if h < 0 => h.saturating_neg(),
            h => self.ppem_for_height(h),
        }
        .clamp(1, MOST_PPEM);
        let own = scale(self.average, ppem, upem);
        let (x_ppem, average) = if device_width > 0 && own > 0 {
            let w = device_width.min(1024);
            ((ppem * w / own).clamp(1, MOST_PPEM), w)
        } else {
            (ppem, own)
        };
        let (height, ascent) = self.height_at(ppem);
        Metrics {
            key: SizeKey { ppem, x_ppem, bold },
            height,
            ascent,
            internal: height - ppem,
            external: scale(self.external, ppem, upem),
            average: average + bold as i32,
            max: scale(self.box_width, x_ppem, upem) + bold as i32,
        }
    }

    /// The glyphs of the 256 characters `chars` gives (a byte's Unicode
    /// character, or its byte in a symbol font; None for one the font
    /// draws with its missing glyph) at the size `m`.
    pub fn draw(&self, m: &Metrics, chars: impl Fn(u8) -> Option<u32>) -> Vec<Drawn> {
        let Ok(font) = FontRef::from_index(&self.data, self.index) else {
            return (0..256)
                .map(|_| Drawn {
                    rows: Vec::new(),
                    advance: 0,
                    offset: 0,
                })
                .collect();
        };
        let SizeKey { ppem, x_ppem, bold } = m.key;
        let size = Size::new(ppem as f32);
        let outlines = font.outline_glyphs();
        let charmap = font.charmap();
        let hinting = (x_ppem == ppem && outlines.prefer_interpreter())
            .then(|| {
                let options = HintingOptions {
                    engine: Engine::Interpreter,
                    target: Target::Mono,
                };
                HintingInstance::new(&outlines, size, LocationRef::default(), options).ok()
            })
            .flatten();
        let metrics = font.glyph_metrics(size, LocationRef::default());
        let hdmx = (x_ppem == ppem)
            .then(|| font.hdmx().ok()?.record_for_size(ppem as u8))
            .flatten();
        let stretch = x_ppem as f32 / ppem as f32;
        (0..=255u8)
            .map(|b| {
                let id = chars(b)
                    .and_then(|c| charmap.map(c))
                    .unwrap_or(GlyphId::NOTDEF);
                let mut pen = Path::default();
                let adjusted = outlines.get(id).and_then(|g| {
                    let settings = match &hinting {
                        Some(h) => DrawSettings::hinted(h, false),
                        None => DrawSettings::unhinted(size, LocationRef::default()),
                    };
                    g.draw(settings, &mut pen).ok()
                });
                let advance = hdmx
                    .as_ref()
                    .and_then(|r| r.widths().get(id.to_u32() as usize).map(|&w| w as i32))
                    .or_else(|| {
                        adjusted
                            .and_then(|a| a.advance_width)
                            .map(|w| w.round() as i32)
                    })
                    .unwrap_or_else(|| {
                        (metrics.advance_width(id).unwrap_or(0.0) * stretch).round() as i32
                    });
                let (rows, offset) = pen.fill(stretch, m.ascent, m.height);
                let rows = if bold {
                    rows.into_iter().map(|r| r | r << 1).collect()
                } else {
                    rows
                };
                Drawn {
                    rows,
                    advance: advance + bold as i32,
                    offset,
                }
            })
            .collect()
    }
}

/// The family of a font by its PANOSE digits, as a LOGFONT's
/// lfPitchAndFamily has it: Latin text by its serifs (FF_SWISS without,
/// FF_ROMAN with, FF_MODERN monospaced), script (FF_SCRIPT), decorative
/// (FF_DECORATIVE), else FF_DONTCARE.
fn family_of(panose: &[u8], fixed: bool) -> u8 {
    const FF_ROMAN: u8 = 0x10;
    const FF_SWISS: u8 = 0x20;
    const FF_MODERN: u8 = 0x30;
    const FF_SCRIPT: u8 = 0x40;
    const FF_DECORATIVE: u8 = 0x50;
    match panose.first() {
        Some(2) if fixed || panose.get(3) == Some(&9) => FF_MODERN,
        Some(2) if matches!(panose.get(1), Some(11..=13)) => FF_SWISS,
        Some(2) => FF_ROMAN,
        Some(3) => FF_SCRIPT,
        Some(4) => FF_DECORATIVE,
        _ => 0,
    }
}

/// An outline's segments in pixels, y up from the baseline.
#[derive(Default)]
struct Path {
    segments: Vec<Segment>,
    first: (f32, f32),
    last: (f32, f32),
}

enum Segment {
    Line([(f32, f32); 2]),
    Quad([(f32, f32); 3]),
    Cubic([(f32, f32); 4]),
}

impl Segment {
    fn points(&self) -> &[(f32, f32)] {
        match self {
            Segment::Line(p) => p,
            Segment::Quad(p) => p,
            Segment::Cubic(p) => p,
        }
    }
}

impl OutlinePen for Path {
    fn move_to(&mut self, x: f32, y: f32) {
        self.first = (x, y);
        self.last = (x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.segments.push(Segment::Line([self.last, (x, y)]));
        self.last = (x, y);
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.segments
            .push(Segment::Quad([self.last, (cx, cy), (x, y)]));
        self.last = (x, y);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.segments
            .push(Segment::Cubic([self.last, (cx0, cy0), (cx1, cy1), (x, y)]));
        self.last = (x, y);
    }

    fn close(&mut self) {
        if self.last != self.first {
            self.segments.push(Segment::Line([self.last, self.first]));
        }
        self.last = self.first;
    }
}

impl Path {
    /// The outline filled one bit a pixel, stretched across by
    /// `stretch`, in a cell `height` rows high whose baseline is `ascent`
    /// rows down: each row's lit pixels from the column `offset` (the
    /// return's second), lit where the outline covers at least half of
    /// the pixel. A glyph wider than `MOST_WIDTH` loses its right part.
    fn fill(&self, stretch: f32, ascent: i32, height: i32) -> (Vec<u64>, i32) {
        let mut rows = vec![0u64; height.max(0) as usize];
        let all = self.segments.iter().flat_map(|s| s.points());
        let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &(x, y) in all {
            let (x, y) = (x * stretch, ascent as f32 - y);
            (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
        }
        if self.segments.is_empty() || x1 <= x0 || y1 <= y0 {
            return (rows, 0);
        }
        let (left, top) = (x0.floor(), y0.floor());
        let width = ((x1.ceil() - left) as i32).clamp(1, MOST_WIDTH) as usize;
        let depth = ((y1.ceil() - top) as i32).clamp(1, 4 * height.max(1)) as usize;
        let at =
            |(x, y): (f32, f32)| -> Point { point(x * stretch - left, ascent as f32 - y - top) };
        let mut r = Rasterizer::new(width, depth);
        for s in &self.segments {
            match s {
                Segment::Line([a, b]) => r.draw_line(at(*a), at(*b)),
                Segment::Quad([a, b, c]) => r.draw_quad(at(*a), at(*b), at(*c)),
                Segment::Cubic([a, b, c, d]) => r.draw_cubic(at(*a), at(*b), at(*c), at(*d)),
            }
        }
        let top = top as i32;
        r.for_each_pixel_2d(|x, y, coverage| {
            let row = top + y as i32;
            if coverage >= 0.5
                && let Some(bits) = usize::try_from(row).ok().and_then(|i| rows.get_mut(i))
            {
                *bits |= 1 << x;
            }
        });
        (rows, left as i32)
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A TrueType font of the family `family`: an em of 1000 units, an
    /// ascent of 800 and a descent of 200, a .notdef, a space 250 wide,
    /// "A" as a square 500 units a side 600 wide, and in a symbol font
    /// (cmap 3,0) the square at F041h instead. Its outlines have no
    /// instructions.
    pub(in super::super) fn ttf(family: &str, symbol: bool) -> Vec<u8> {
        build(family, symbol)
    }

    fn be16(v: &mut Vec<u8>, x: u16) {
        v.extend_from_slice(&x.to_be_bytes());
    }

    fn be32(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_be_bytes());
    }

    fn build(family: &str, symbol: bool) -> Vec<u8> {
        // glyf: .notdef and space empty, the square a contour of four
        // on-curve points.
        let mut glyf = Vec::new();
        be16(&mut glyf, 1); // contours
        for v in [100i16, 0, 600, 500] {
            be16(&mut glyf, v as u16);
        }
        be16(&mut glyf, 3); // end of contour 0
        be16(&mut glyf, 0); // instructions
        glyf.extend_from_slice(&[1, 1, 1, 1]); // on curve, long coordinates
        for x in [100i16, 500, 0, -500] {
            be16(&mut glyf, x as u16);
        }
        for y in [0i16, 0, 500, 0] {
            be16(&mut glyf, y as u16);
        }
        // The fourth point is (100, 500): x -500 from (600, 500).
        let square = glyf.len() as u16;
        let mut loca = Vec::new();
        for off in [0u16, 0, 0, square / 2] {
            be16(&mut loca, off);
        }
        let mut hmtx = Vec::new();
        for (adv, lsb) in [(500u16, 0i16), (250, 0), (600, 100)] {
            be16(&mut hmtx, adv);
            be16(&mut hmtx, lsb as u16);
        }
        let mut head = Vec::new();
        be32(&mut head, 0x0001_0000);
        be32(&mut head, 0x0001_0000);
        be32(&mut head, 0); // checksum adjustment
        be32(&mut head, 0x5F0F_3CF5);
        be16(&mut head, 0);
        be16(&mut head, 1000);
        head.extend_from_slice(&[0; 16]);
        for v in [0i16, -200, 600, 800] {
            be16(&mut head, v as u16);
        }
        be16(&mut head, 0); // macStyle
        be16(&mut head, 8);
        be16(&mut head, 2);
        be16(&mut head, 0); // short loca
        be16(&mut head, 0);
        let mut hhea = Vec::new();
        be32(&mut hhea, 0x0001_0000);
        for v in [800i16, -200, 0] {
            be16(&mut hhea, v as u16);
        }
        be16(&mut hhea, 600);
        hhea.extend_from_slice(&[0; 22]);
        be16(&mut hhea, 3);
        let mut maxp = Vec::new();
        be32(&mut maxp, 0x0001_0000);
        be16(&mut maxp, 3);
        maxp.extend_from_slice(&[0; 26]);
        // cmap format 4 with one segment for the square and the end one.
        let (first, space) = if symbol {
            (0xF041u16, 0xF020u16)
        } else {
            (0x41, 0x20)
        };
        let mut sub = Vec::new();
        be16(&mut sub, 4);
        be16(&mut sub, 0); // length, set below
        be16(&mut sub, 0);
        be16(&mut sub, 6); // 3 segments
        for v in [4u16, 1, 2] {
            be16(&mut sub, v);
        }
        for v in [space, first, 0xFFFF] {
            be16(&mut sub, v);
        }
        be16(&mut sub, 0);
        for v in [space, first, 0xFFFF] {
            be16(&mut sub, v);
        }
        for v in [1u16.wrapping_sub(space), 2u16.wrapping_sub(first), 1] {
            be16(&mut sub, v);
        }
        for _ in 0..3 {
            be16(&mut sub, 0);
        }
        let len = sub.len() as u16;
        sub[2..4].copy_from_slice(&len.to_be_bytes());
        let mut cmap = Vec::new();
        be16(&mut cmap, 0);
        be16(&mut cmap, 1);
        be16(&mut cmap, 3);
        be16(&mut cmap, if symbol { 0 } else { 1 });
        be32(&mut cmap, 12);
        cmap.extend(sub);
        // name: the family (1) in UTF-16BE for Windows.
        let utf16: Vec<u8> = family
            .encode_utf16()
            .flat_map(|c| c.to_be_bytes())
            .collect();
        let mut name = Vec::new();
        be16(&mut name, 0);
        be16(&mut name, 1);
        be16(&mut name, 18);
        for v in [
            3u16,
            if symbol { 0 } else { 1 },
            0x409,
            1,
            utf16.len() as u16,
            0,
        ] {
            be16(&mut name, v);
        }
        name.extend(utf16);
        let mut os2 = Vec::new();
        be16(&mut os2, 1);
        be16(&mut os2, 500); // xAvgCharWidth
        be16(&mut os2, 400); // weight
        os2.extend_from_slice(&[0; 26]);
        os2.extend_from_slice(&[2, 11, 0, 0, 0, 0, 0, 0, 0, 0]); // PANOSE: sans
        os2.extend_from_slice(&[0; 16]);
        os2.extend_from_slice(b"TEST");
        be16(&mut os2, 0x40); // fsSelection: regular
        os2.extend_from_slice(&[0; 4]);
        for v in [800i16, -200, 0, 800, 200] {
            be16(&mut os2, v as u16);
        }
        be32(&mut os2, 1); // ulCodePageRange1: Latin 1
        be32(&mut os2, 0);
        let mut post = Vec::new();
        be32(&mut post, 0x0003_0000);
        post.extend_from_slice(&[0; 28]);
        let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"OS/2", os2),
            (b"cmap", cmap),
            (b"glyf", glyf),
            (b"head", head),
            (b"hhea", hhea),
            (b"hmtx", hmtx),
            (b"loca", loca),
            (b"maxp", maxp),
            (b"name", name),
            (b"post", post),
        ];
        let mut out = Vec::new();
        be32(&mut out, 0x0001_0000);
        be16(&mut out, tables.len() as u16);
        out.extend_from_slice(&[0; 6]);
        let mut at = 12 + 16 * tables.len();
        let mut body = Vec::new();
        for (tag, data) in &mut tables {
            out.extend_from_slice(*tag);
            be32(&mut out, 0);
            be32(&mut out, at as u32);
            be32(&mut out, data.len() as u32);
            data.resize(data.len().next_multiple_of(4), 0);
            at += data.len();
            body.extend_from_slice(data);
        }
        out.extend(body);
        out
    }

    #[test]
    fn a_ttf_file_gives_its_family_and_metrics() {
        let f = &Ttf::fonts_of_file(&ttf("Test Face", false))[0];
        assert_eq!(f.face, "Test Face");
        assert!(!f.symbol);
        assert_eq!((f.weight, f.italic), (400, false));
        assert_eq!(f.pitch_family, 0x22, "FF_SWISS, variable pitch");
        // 20 pixels to the em: 16 above the baseline, 4 below.
        let m = f.metrics(-20, 0, false);
        assert_eq!((m.height, m.ascent, m.internal), (20, 16, 0));
        assert_eq!(m.average, 10);
        // A cell 30 high is 30 pixels to the em.
        assert_eq!(f.metrics(30, 0, false).key.ppem, 30);
    }

    #[test]
    fn a_glyph_is_its_outline_filled_at_the_size() {
        let f = &Ttf::fonts_of_file(&ttf("Test Face", false))[0];
        let m = f.metrics(-20, 0, false);
        let glyphs = f.draw(&m, |b| Some(b as u32));
        let a = &glyphs[b'A' as usize];
        // 600 units at 20 pixels to the em; the square from x 2 to 12,
        // and from the baseline (row 16) 10 rows up.
        assert_eq!(a.advance, 12);
        assert_eq!(a.offset, 2);
        let lit: Vec<usize> = (0..20).filter(|&r| a.rows[r] != 0).collect();
        assert_eq!(lit, (6..16).collect::<Vec<_>>());
        assert!(a.rows[6..16].iter().all(|&r| r == (1 << 10) - 1));
        assert_eq!(glyphs[b' ' as usize].advance, 5);
        assert!(glyphs[b' ' as usize].rows.iter().all(|&r| r == 0));
    }

    #[test]
    fn a_symbol_font_maps_bytes_to_its_private_use_characters() {
        let f = &Ttf::fonts_of_file(&ttf("Test Symbols", true))[0];
        assert!(f.symbol);
        let m = f.metrics(-20, 0, false);
        let glyphs = f.draw(&m, |b| Some(b as u32));
        assert!(glyphs[b'A' as usize].rows.iter().any(|&r| r != 0));
    }

    #[test]
    fn bytes_that_arent_ttf_give_no_fonts() {
        assert!(Ttf::fonts_of_file(b"not a font at all").is_empty());
        assert!(Ttf::fonts_of_file(&[]).is_empty());
    }
}
