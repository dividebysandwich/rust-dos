//! The virtual printer's typefaces: TrueType or OpenType fonts drawn at
//! the printer's resolution, from the `fontpath` folder under the names
//! DOSBox-X gives them (roman.ttf, sansserif.ttf, courier.ttf, script.ttf,
//! ocra.ttf), or from the host's own fonts (Liberation, DejaVu, the
//! Windows and macOS ones). Without any, the VGA's 8x16 ROM font, scaled
//! up and smoothed, prints instead, and it prints the characters a font
//! lacks too.

use ab_glyph_rasterizer::{Point, Rasterizer, point};
use skrifa::{
    FontRef, MetadataProvider,
    instance::{LocationRef, Size},
    outline::{DrawSettings, OutlinePen},
    raw::TableProvider,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The typefaces the printer has (ESC k picks one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    Roman,
    Sans,
    Courier,
    Script,
    Ocr,
}

impl Face {
    /// The file DOSBox-X loads from its font folder, then the host's
    /// fonts that look most like it.
    fn files(self) -> &'static [&'static str] {
        match self {
            Face::Roman => &[
                "roman.ttf",
                "LiberationSerif-Regular.ttf",
                "times.ttf",
                "Times New Roman.ttf",
                "Times.ttc",
                "DejaVuSerif.ttf",
                "NotoSerif-Regular.ttf",
            ],
            Face::Sans => &[
                "sansserif.ttf",
                "LiberationSans-Regular.ttf",
                "arial.ttf",
                "Arial.ttf",
                "Helvetica.ttc",
                "DejaVuSans.ttf",
                "NotoSans-Regular.ttf",
            ],
            Face::Courier => &[
                "courier.ttf",
                "LiberationMono-Regular.ttf",
                "cour.ttf",
                "Courier New.ttf",
                "Courier.ttc",
                "DejaVuSansMono.ttf",
                "NotoSansMono-Regular.ttf",
            ],
            Face::Script => &["script.ttf", "freescpt.ttf", "Brush Script.ttf"],
            Face::Ocr => &["ocra.ttf", "ocraext.ttf", "OCRA.otf", "OCRAStd.otf"],
        }
    }

    /// The face to print with when there is no font for this one.
    fn fallback(self) -> Option<Face> {
        match self {
            Face::Script => Some(Face::Roman),
            Face::Ocr => Some(Face::Courier),
            Face::Roman | Face::Sans => Some(Face::Courier),
            Face::Courier => None,
        }
    }
}

/// What a character prints in: the typeface, its size across and down in
/// points, and whether it slants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub face: Face,
    pub h_points: f64,
    pub v_points: f64,
    pub italic: bool,
}

impl Style {
    fn key(&self) -> (Face, u64, u64, bool) {
        (self.face, self.h_points.to_bits(), self.v_points.to_bits(), self.italic)
    }
}

/// A character drawn: its ink (0-255 a dot), and where it goes from the
/// print head, which is at the top of the line.
#[derive(Debug, Default)]
pub struct Glyph {
    pub left: i64,
    pub top: i64,
    pub width: usize,
    pub coverage: Vec<u8>,
    /// How far a proportional font moves on, in dots.
    pub advance: f64,
}

/// A character in a style (or a cell's size), as drawn characters are
/// kept by.
type GlyphKey = ((Face, u64, u64, bool), char);

/// How far italics slant: 0.2 across for each dot up, as DOSBox-X has it.
const SLANT: f32 = 0.2;
/// How many characters are kept drawn.
const CACHE: usize = 4096;

/// A validated font file and its unscaled line metrics.
pub struct FontFile {
    data: Vec<u8>,
    metrics: skrifa::metrics::Metrics,
}

pub struct Fonts {
    fontpath: Option<PathBuf>,
    dpi: u32,
    faces: HashMap<Face, Option<Arc<FontFile>>>,
    /// The host's font files by their names in lower case, looked for once.
    index: Option<HashMap<String, PathBuf>>,
    cache: HashMap<GlyphKey, Arc<Glyph>>,
    cell_cache: HashMap<GlyphKey, Arc<Glyph>>,
}

impl Fonts {
    pub fn new(fontpath: Option<PathBuf>, dpi: u32) -> Self {
        Self { fontpath, dpi, faces: HashMap::new(), index: None, cache: HashMap::new(), cell_cache: HashMap::new() }
    }

    /// The font file behind `face`, or none for the ROM font.
    pub fn file(&mut self, face: Face) -> Option<Arc<FontFile>> {
        if let Some(font) = self.faces.get(&face) {
            return font.clone();
        }
        let font = self.load(face).or_else(|| face.fallback().and_then(|f| self.file(f)));
        self.faces.insert(face, font.clone());
        font
    }

    fn load(&mut self, face: Face) -> Option<Arc<FontFile>> {
        let files = face.files();
        // The printer's own fonts first, then the host's.
        if let Some(dir) = &self.fontpath {
            for name in files {
                if let Some(font) = find_in(dir, name).and_then(|path| read_font(&path)) {
                    return Some(font);
                }
            }
        }
        let index = self.index.get_or_insert_with(system_fonts);
        files.iter().skip(1).find_map(|name| index.get(&name.to_ascii_lowercase()).and_then(|path| read_font(path)))
    }

    /// The height of the font's letters above the line they stand on, in
    /// dots.
    pub fn ascent(&mut self, style: &Style) -> f64 {
        let em = style.v_points / 72.0 * self.dpi as f64;
        match self.file(style.face) {
            Some(font) => font.metrics.ascent as f64 / font.metrics.units_per_em as f64 * em,
            None => em * BITMAP_ASCENT,
        }
    }

    /// The font's line height, in dots.
    pub fn height(&mut self, style: &Style) -> f64 {
        let em = style.v_points / 72.0 * self.dpi as f64;
        match self.file(style.face) {
            Some(font) => (font.metrics.ascent - font.metrics.descent) as f64 / font.metrics.units_per_em as f64 * em,
            None => em * 1.15,
        }
    }

    /// `c` drawn in `style`.
    pub fn glyph(&mut self, style: &Style, c: char) -> Arc<Glyph> {
        let key = (style.key(), c);
        if let Some(glyph) = self.cache.get(&key) {
            return glyph.clone();
        }
        let glyph = Arc::new(match self.file(style.face).and_then(|font| self.outline(&font, style, c)) {
            Some(glyph) => glyph,
            None => {
                let ascent = self.ascent(style);
                self.bitmap(style, c, ascent)
            }
        });
        if self.cache.len() >= CACHE {
            self.cache.clear();
        }
        self.cache.insert(key, glyph.clone());
        glyph
    }

    /// `c` from the font file, if the font has it.
    fn outline(&self, file: &FontFile, style: &Style, c: char) -> Option<Glyph> {
        let font = FontRef::from_index(&file.data, 0).ok()?;
        let metrics = &file.metrics;
        let upem = metrics.units_per_em as f32;
        let px_x = (style.h_points / 72.0 * self.dpi as f64) as f32;
        let px_y = (style.v_points / 72.0 * self.dpi as f64) as f32;
        let scale = point(px_x / upem, px_y / upem);
        let id = font.charmap().map(c).unwrap_or_default();
        if id.to_u32() == 0 && !c.is_whitespace() {
            return None;
        }
        let glyph_metrics = font.glyph_metrics(Size::unscaled(), LocationRef::default());
        let advance = (glyph_metrics.advance_width(id)? * scale.x) as f64;
        let ascent = metrics.ascent * scale.y;
        let outlines = font.outline_glyphs();
        let Some(outline) = outlines.get(id) else {
            return Some(Glyph { advance, ..Default::default() });
        };
        let bounds = glyph_metrics.bounds(id)?;
        let slant = if style.italic { SLANT } else { 0.0 };
        let left = ((bounds.x_min + slant * bounds.y_min) * scale.x).floor();
        let right = ((bounds.x_max + slant * bounds.y_max) * scale.x).ceil();
        let top = (-bounds.y_max * scale.y).floor();
        let bottom = (-bounds.y_min * scale.y).ceil();
        let width = (right - left).max(0.0) as usize;
        let height = (bottom - top).max(0.0) as usize;
        if width == 0 || height == 0 {
            return Some(Glyph { advance, ..Default::default() });
        }
        let mut rasterizer = Rasterizer::new(width, height);
        let mut pen = RasterPen::new(&mut rasterizer, scale, point(-left, -top), slant);
        outline.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen).ok()?;
        let mut coverage = vec![0u8; width * height];
        rasterizer.for_each_pixel_2d(|x, y, c| {
            if let Some(dot) = coverage.get_mut(y as usize * width + x as usize) {
                *dot = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        });
        Some(Glyph {
            left: left as i64,
            top: (ascent.round() + top) as i64,
            width,
            coverage,
            advance,
        })
    }

    /// `c` from the VGA's ROM font, scaled to the size and smoothed.
    fn bitmap(&self, style: &Style, c: char, ascent: f64) -> Glyph {
        let em_x = style.h_points / 72.0 * self.dpi as f64;
        let em_y = style.v_points / 72.0 * self.dpi as f64;
        let cell_w = (em_x * BITMAP_WIDTH).round().max(1.0);
        let cell_h = (em_y * BITMAP_HEIGHT).round().max(1.0);
        let Some(code) = (c != ' ').then(|| crate::keylayout::cp437(c)).flatten() else {
            return Glyph { advance: cell_w, ..Default::default() };
        };
        let slant = if style.italic { SLANT as f64 } else { 0.0 };
        let base = cell_h * BITMAP_BASELINE / 16.0;
        let (width, coverage) = rom_glyph(code, cell_w, cell_h, slant, false);
        Glyph {
            left: 0,
            top: (ascent - base).round() as i64,
            width,
            coverage,
            advance: cell_w,
        }
    }

    /// A box drawing or block character from the ROM font, filling a
    /// cell `width` by `height` dots from the top of the line, so that
    /// lines and blocks join those next to them and on the lines above
    /// and below, as a dot matrix printer prints them.
    pub fn cell(&mut self, c: char, width: f64, height: f64, italic: bool) -> Option<Arc<Glyph>> {
        let code = crate::keylayout::cp437(c)?;
        let key = ((Face::Courier, width.to_bits(), height.to_bits(), italic), c);
        if let Some(glyph) = self.cell_cache.get(&key) {
            return Some(glyph.clone());
        }
        let (cell_w, cell_h) = (width.round().max(1.0), height.round().max(1.0));
        let (width, coverage) = rom_glyph(code, cell_w, cell_h, if italic { SLANT as f64 } else { 0.0 }, true);
        let glyph = Arc::new(Glyph { left: 0, top: 0, width, coverage, advance: cell_w });
        if self.cell_cache.len() >= CACHE {
            self.cell_cache.clear();
        }
        self.cell_cache.insert(key, glyph.clone());
        Some(glyph)
    }
}

/// The ROM font's glyph for `code` scaled to `cell_w` by `cell_h` dots
/// and smoothed, slanted by `slant` about the bottom; `edges` carries the
/// glyph's edge dots on to the cell's edges. Its width, and its ink.
fn rom_glyph(code: u8, cell_w: f64, cell_h: f64, slant: f64, edges: bool) -> (usize, Vec<u8>) {
    let rows = &crate::video::font_8x16()[code as usize * 16..code as usize * 16 + 16];
    let bit = |x: i64, y: i64| -> f64 {
        let (x, y) = if edges { (x.clamp(0, 7), y.clamp(0, 15)) } else { (x, y) };
        if !(0..8).contains(&x) || !(0..16).contains(&y) {
            return 0.0;
        }
        (rows[y as usize] >> (7 - x) & 1) as f64
    };
    let extra = (slant * cell_h).ceil() as usize;
    let width = cell_w as usize + extra;
    let height = cell_h as usize;
    let mut coverage = vec![0u8; width * height];
    for dy in 0..height {
        let shift = slant * (cell_h - dy as f64);
        let sy = (dy as f64 + 0.5) * 16.0 / cell_h - 0.5;
        let (y0, fy) = (sy.floor() as i64, sy - sy.floor());
        for dx in 0..width {
            let sx = (dx as f64 - shift + 0.5) * 8.0 / cell_w - 0.5;
            if edges && (sx < -0.5 || sx > 7.5) {
                continue;
            }
            let (x0, fx) = (sx.floor() as i64, sx - sx.floor());
            let top = bit(x0, y0) * (1.0 - fx) + bit(x0 + 1, y0) * fx;
            let bottom = bit(x0, y0 + 1) * (1.0 - fx) + bit(x0 + 1, y0 + 1) * fx;
            let v = top * (1.0 - fy) + bottom * fy;
            coverage[dy * width + dx] = (v * 255.0).round() as u8;
        }
    }
    (width, coverage)
}

/// Feed unhinted font coordinates into the printer's existing rasterizer.
struct RasterPen<'a> {
    rasterizer: &'a mut Rasterizer,
    scale: Point,
    offset: Point,
    slant: f32,
    first: Point,
    last: Point,
}

impl<'a> RasterPen<'a> {
    fn new(rasterizer: &'a mut Rasterizer, scale: Point, offset: Point, slant: f32) -> Self {
        Self { rasterizer, scale, offset, slant, first: point(0.0, 0.0), last: point(0.0, 0.0) }
    }

    fn map(&self, x: f32, y: f32) -> Point {
        point((x + self.slant * y) * self.scale.x + self.offset.x, -y * self.scale.y + self.offset.y)
    }
}

impl OutlinePen for RasterPen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.first = self.map(x, y);
        self.last = self.first;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let end = self.map(x, y);
        self.rasterizer.draw_line(self.last, end);
        self.last = end;
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        let end = self.map(x, y);
        self.rasterizer.draw_quad(self.last, self.map(cx, cy), end);
        self.last = end;
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let end = self.map(x, y);
        self.rasterizer.draw_cubic(self.last, self.map(cx0, cy0), self.map(cx1, cy1), end);
        self.last = end;
    }

    fn close(&mut self) {
        self.rasterizer.draw_line(self.last, self.first);
        self.last = self.first;
    }
}

/// The ROM font's cell, in ems across and down, the row of its letters'
/// baseline, and the ascent it stands in for.
const BITMAP_WIDTH: f64 = 0.6;
const BITMAP_HEIGHT: f64 = 1.25;
const BITMAP_BASELINE: f64 = 12.5;
const BITMAP_ASCENT: f64 = 0.8;

/// A file named `name` in `dir`, whatever the case of its name.
fn find_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir.join(name);
    if path.is_file() {
        return Some(path);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
}

fn read_font(path: &Path) -> Option<Arc<FontFile>> {
    let data = std::fs::read(path).ok()?;
    let font = FontRef::from_index(&data, 0).ok()?;
    let upem = font.head().ok()?.units_per_em();
    if !(16..=16384).contains(&upem) {
        return None;
    }
    font.hhea().ok()?;
    font.maxp().ok()?;
    let metrics = font.metrics(Size::unscaled(), LocationRef::default());
    Some(Arc::new(FontFile { data, metrics }))
}

/// The host's font files by name (in lower case), from the folders the
/// systems keep them in.
fn system_fonts() -> HashMap<String, PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        dirs.push(windir.join("Fonts"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Microsoft\\Windows\\Fonts"));
        }
    } else if cfg!(target_os = "macos") {
        dirs.extend(["/System/Library/Fonts", "/Library/Fonts"].map(PathBuf::from));
        if let Some(home) = crate::hostdirs::home_dir() {
            dirs.push(home.join("Library/Fonts"));
        }
    } else {
        dirs.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(PathBuf::from));
        if let Some(home) = crate::hostdirs::home_dir() {
            dirs.push(home.join(".local/share/fonts"));
            dirs.push(home.join(".fonts"));
        }
    }
    let mut index = HashMap::new();
    for dir in dirs {
        walk(&dir, 4, &mut index);
    }
    index
}

fn walk(dir: &Path, depth: u32, index: &mut HashMap<String, PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                walk(&path, depth - 1, index);
            }
        } else {
            index.entry(entry.file_name().to_string_lossy().to_ascii_lowercase()).or_insert(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(face: Face) -> Style {
        Style { face, h_points: 10.5, v_points: 10.5, italic: false }
    }

    #[test]
    fn outlines_close_and_rasterize_line_quadratic_and_cubic_segments() {
        let mut rasterizer = Rasterizer::new(4, 4);
        let mut pen = RasterPen::new(&mut rasterizer, point(1.0, 1.0), point(0.0, 0.0), 0.0);
        pen.move_to(0.0, 0.0);
        pen.line_to(4.0, 0.0);
        pen.quad_to(4.0, -2.0, 4.0, -4.0);
        pen.curve_to(3.0, -4.0, 1.0, -4.0, 0.0, -4.0);
        pen.close();
        let mut pixels = Vec::new();
        rasterizer.for_each_pixel_2d(|_, _, coverage| pixels.push(coverage));
        assert_eq!(pixels, vec![1.0; 16]);
    }

    #[test]
    fn invalid_font_files_are_rejected() {
        let path = crate::test_dir::path("invalid-font.ttf");
        let mut head_only = vec![0u8; 82];
        head_only[..4].copy_from_slice(&[0, 1, 0, 0]);
        head_only[4..6].copy_from_slice(&1u16.to_be_bytes());
        head_only[12..16].copy_from_slice(b"head");
        head_only[20..24].copy_from_slice(&28u32.to_be_bytes());
        head_only[24..28].copy_from_slice(&54u32.to_be_bytes());
        for upem in [0u16, 16385] {
            head_only[46..48].copy_from_slice(&upem.to_be_bytes());
            assert_eq!(FontRef::from_index(&head_only, 0).unwrap().head().unwrap().units_per_em(), upem);
            std::fs::write(&path, &head_only).unwrap();
            let result = read_font(&path);
            std::fs::remove_file(&path).unwrap();
            assert!(result.is_none());
        }
        std::fs::write(&path, b"not an OpenType font").unwrap();
        let result = read_font(&path);
        std::fs::remove_file(path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn the_rom_font_prints_without_fonts() {
        let mut fonts = Fonts::new(None, 360);
        // No host fonts looked for: an empty index.
        fonts.index = Some(HashMap::new());
        let a = fonts.glyph(&style(Face::Courier), 'A');
        assert!(a.width > 20 && a.coverage.contains(&255), "{:?}", a.width);
        let space = fonts.glyph(&style(Face::Courier), ' ');
        assert!(space.coverage.is_empty() && space.advance > 0.0);
        // Slanted, it is wider.
        let italic = fonts.glyph(&Style { italic: true, ..style(Face::Courier) }, 'A');
        assert!(italic.width > a.width);
    }

    #[test]
    fn host_fonts_print_when_there_are_some() {
        let mut fonts = Fonts::new(None, 360);
        if fonts.file(Face::Courier).is_none() {
            return;
        }
        let a = fonts.glyph(&style(Face::Courier), 'A');
        assert!(a.width > 10 && a.coverage.iter().any(|&c| c > 200));
        assert!(fonts.ascent(&style(Face::Courier)) > 30.0);
        // Box drawing, which a font may lack, prints all the same.
        let corner = fonts.glyph(&style(Face::Courier), '╔');
        assert!(corner.coverage.iter().any(|&c| c > 0));
    }
}
