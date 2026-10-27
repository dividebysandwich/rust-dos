//! Drawing: lines and shapes with the DC's pen and brush, blits with
//! their raster operations, pixels, flood fills and paths.
//!
//! Each function maps its logical coordinates to the DC's device
//! coordinates and those to its surface's, loads the rows it draws on
//! (`Canvas`), draws the pixels its clip region lets through and stores
//! the rows back. Raster operations work on pixel values, as Windows
//! does: on a DIB of 8 bits a pixel they combine the indices.
//!
//! On a palette device (the screen of a desktop of 256 colours, and
//! GDI's 8-bit bitmaps) colors become system palette indices through
//! the DC's logical palette (`palette::DeviceMap`): PALETTEINDEX and
//! PALETTERGB take its entries, RGB colors of pens, brushes and text
//! the nearest static color, and the pixels of blits from DIBs and
//! other bitmaps the nearest color of the palette's and the static
//! ones.

use super::dc::{self, Dc, OPAQUE};
use super::surface::Surface;
use super::{
    BS_HATCHED, BS_NULL, BS_SOLID, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER, FALSE,
    PS_ALTERNATE, PS_DASH, PS_DASHDOT, PS_DASHDOTDOT, PS_DOT, PS_INSIDEFRAME, PS_NULL,
    PS_STYLE_MASK, PS_USERSTYLE, Pattern, Rect, Region, TRUE, gdi, set_error,
};
use crate::cpu::Cpu;
use crate::win32::user32::{read_words, write_words};

/// What a drawing function draws on: the rows of the surface it loaded
/// (or, from `open_columns`, some of their columns), and the rectangles
/// of the clip region within them.
pub struct Canvas {
    pub surface: Surface,
    pub rects: Vec<Rect>,
    /// The DC, as it was when the drawing started.
    pub dc: Dc,
    /// Where its device origin is on the surface.
    pub origin: (i32, i32),
    pub hdc: u32,
    /// What it drew on, in surface coordinates, for the DC's bounds.
    drawn: Rect,
}

impl Canvas {
    /// Load what the DC `hdc` draws on within `bounds`, a rectangle in
    /// device coordinates. None if it can't draw or nothing of `bounds`
    /// may be drawn on.
    pub fn open(cpu: &mut Cpu, hdc: u32, bounds: Rect) -> Option<Canvas> {
        Canvas::open_area(cpu, hdc, bounds, false)
    }

    /// `open` for a function that reads and draws only the pixels the
    /// clip region lets it within `bounds`: on a surface of 8 bits a
    /// pixel or more it loads only their columns.
    pub fn open_columns(cpu: &mut Cpu, hdc: u32, bounds: Rect) -> Option<Canvas> {
        Canvas::open_area(cpu, hdc, bounds, true)
    }

    fn open_area(cpu: &mut Cpu, hdc: u32, bounds: Rect, columns: bool) -> Option<Canvas> {
        let dc = gdi(cpu).dc(hdc)?.clone();
        let place = dc::place(cpu, hdc)?;
        let (ox, oy) = place.origin;
        let b = bounds.offset(ox, oy);
        let rects: Vec<Rect> = place
            .clip
            .rects
            .iter()
            .map(|r| r.intersect(&b))
            .filter(|r| !r.is_empty())
            .collect();
        let area = rects.iter().fold(Rect::default(), |a, r| a.union(r));
        if area.is_empty() {
            // Nothing to draw, but the bounds still count.
            dc::accumulate(cpu, hdc, bounds);
            return None;
        }
        let surface = if columns {
            Surface::load_area(cpu, place.target, area)?
        } else {
            Surface::load(cpu, place.target, area.top, area.bottom)?
        };
        let rects = rects
            .into_iter()
            .map(|r| r.intersect(&surface.loaded()))
            .filter(|r| !r.is_empty())
            .collect();
        Some(Canvas {
            surface,
            rects,
            dc,
            origin: place.origin,
            hdc,
            drawn: Rect::default(),
        })
    }

    /// Store what was drawn.
    pub fn close(self, cpu: &mut Cpu) {
        self.surface.store(cpu);
        let d = self.drawn.offset(-self.origin.0, -self.origin.1);
        dc::accumulate(cpu, self.hdc, d);
    }

    /// Whether the surface pixel (`x`, `y`) may be drawn.
    #[inline]
    pub fn visible(&self, x: i32, y: i32) -> bool {
        self.rects.iter().any(|r| r.contains(x, y))
    }

    /// The device point (`x`, `y`) on the surface.
    #[inline]
    pub fn at(&self, x: i32, y: i32) -> (i32, i32) {
        (x + self.origin.0, y + self.origin.1)
    }

    /// Draw the surface pixel (`x`, `y`) with `value` through the binary
    /// raster operation `rop2`, if it may be drawn.
    #[inline]
    pub fn plot(&mut self, x: i32, y: i32, value: u32, rop2: u32) {
        if !self.visible(x, y) {
            return;
        }
        self.plot_unclipped(x, y, value, rop2);
    }

    #[inline]
    fn plot_unclipped(&mut self, x: i32, y: i32, value: u32, rop2: u32) {
        let mask = self.surface.value_mask();
        let new = if rop2 == dc::R2_COPYPEN {
            value
        } else {
            let d = self.surface.get(x, y);
            apply_rop2(rop2, value, d, mask, &self.surface)
        };
        self.surface.put(x, y, new & mask);
        self.mark(x, y);
    }

    /// Count the surface rectangle `r` as drawn on, for a function that
    /// puts the surface's pixels itself.
    pub fn drew(&mut self, r: Rect) {
        if !r.is_empty() {
            self.drawn = self.drawn.union(&r);
        }
    }

    #[inline]
    fn mark(&mut self, x: i32, y: i32) {
        if self.drawn.is_empty() {
            self.drawn = Rect::new(x, y, x + 1, y + 1);
        } else {
            let d = &mut self.drawn;
            d.left = d.left.min(x);
            d.top = d.top.min(y);
            d.right = d.right.max(x + 1);
            d.bottom = d.bottom.max(y + 1);
        }
    }

    /// Fill the surface rectangle `r` with `fill` through `rop2`.
    pub fn fill(&mut self, r: Rect, fill: &Fill, rop2: u32) {
        if matches!(fill, Fill::None) {
            return;
        }
        let rects: Vec<Rect> = self
            .rects
            .iter()
            .map(|c| c.intersect(&r))
            .filter(|c| !c.is_empty())
            .collect();
        for c in rects {
            for y in c.top..c.bottom {
                for x in c.left..c.right {
                    if let Some(v) = fill.value_at(x, y) {
                        self.plot_unclipped(x, y, v, rop2);
                    }
                }
            }
        }
    }

    /// Fill the horizontal run of surface pixels from `x0` to `x1`
    /// (excluded) on row `y`.
    pub fn span(&mut self, x0: i32, x1: i32, y: i32, fill: &Fill, rop2: u32) {
        self.fill(Rect::new(x0, y, x1, y + 1), fill, rop2);
    }
}

/// The colors black and white, which BLACKNESS and R2_WHITE draw, as a
/// surface's values.
fn black_white(s: &Surface) -> (u32, u32) {
    if s.mono {
        (0, 1)
    } else {
        (s.value_of(0), s.value_of(0xFFFFFF))
    }
}

/// A binary raster operation (R2_*) of the pen's value `p` and the
/// pixel's `d`.
pub fn apply_rop2(rop2: u32, p: u32, d: u32, mask: u32, s: &Surface) -> u32 {
    match rop2 {
        1 => black_white(s).0,
        16 => black_white(s).1,
        13 => p,
        11 => d,
        _ => {
            let code = rop2.wrapping_sub(1) & 15;
            let mut r = 0;
            if code & 8 != 0 {
                r |= p & d;
            }
            if code & 4 != 0 {
                r |= p & !d;
            }
            if code & 2 != 0 {
                r |= !p & d;
            }
            if code & 1 != 0 {
                r |= !p & !d;
            }
            r & mask
        }
    }
}

/// A ternary raster operation's result for the pattern `p`, source `s`
/// and destination `d`.
#[inline]
pub fn apply_rop3(rop: u8, p: u32, s: u32, d: u32) -> u32 {
    match rop {
        0xCC => s,
        0xF0 => p,
        0xAA => d,
        0x66 => s ^ d,
        0x88 => s & d,
        0xEE => s | d,
        0x55 => !d,
        0x5A => p ^ d,
        0x33 => !s,
        0xBB => !s | d,
        0x22 => !s & d,
        _ => {
            let mut r = 0;
            for i in 0..8 {
                if rop >> i & 1 != 0 {
                    let a = if i & 4 != 0 { p } else { !p };
                    let b = if i & 2 != 0 { s } else { !s };
                    let c = if i & 1 != 0 { d } else { !d };
                    r |= a & b & c;
                }
            }
            r
        }
    }
}

/// Whether a raster operation uses the pattern, and the source.
pub fn uses_pattern(rop: u8) -> bool {
    ((rop >> 4) ^ rop) & 0x0F != 0
}

pub fn uses_source(rop: u8) -> bool {
    ((rop >> 2) ^ rop) & 0x33 != 0
}

/// How a brush fills, as a surface's values.
pub enum Fill {
    None,
    Solid(u32),
    /// A pattern, its origin on the surface, and its values; None draws
    /// nothing (a hatch's gaps on a transparent background).
    Pattern {
        width: i32,
        height: i32,
        org: (i32, i32),
        values: Vec<Option<u32>>,
    },
}

impl Fill {
    #[inline]
    pub fn value_at(&self, x: i32, y: i32) -> Option<u32> {
        match self {
            Fill::None => None,
            Fill::Solid(v) => Some(*v),
            Fill::Pattern {
                width,
                height,
                org,
                values,
            } => {
                let px = (x - org.0).rem_euclid(*width);
                let py = (y - org.1).rem_euclid(*height);
                values[(py * width + px) as usize]
            }
        }
    }
}

/// The value of `color` (a COLORREF of the DC) on the surface: a
/// monochrome bitmap's 1 for the DC's background color and 0 for the
/// rest, as Windows draws colors on it.
pub fn value_for(cpu: &mut Cpu, canvas_dc: &Dc, s: &Surface, color: u32) -> u32 {
    value_through(cpu, canvas_dc, s, color, None)
}

/// `value_for`, through `map` on a palette device if the caller has the
/// DC's map already.
fn value_through(
    cpu: &mut Cpu,
    canvas_dc: &Dc,
    s: &Surface,
    color: u32,
    map: Option<&super::palette::DeviceMap>,
) -> u32 {
    if color >> 16 == 0x10FF {
        // DIBINDEX: the index itself on a surface with a color table,
        // black on others.
        return match s.bpp {
            1 | 4 | 8 if !s.mono => (color & 0xFFFF) & ((1 << s.bpp) - 1),
            _ => s.value_of(0),
        };
    }
    if s.device {
        return match map {
            Some(map) => device_value(map, color),
            None => device_value(
                &super::palette::DeviceMap::new(cpu, canvas_dc.attrs.palette),
                color,
            ),
        };
    }
    let color = dc::resolve_color(cpu, canvas_dc, color);
    if s.mono {
        let sum = (color & 0xFF) + ((color >> 8) & 0xFF) + ((color >> 16) & 0xFF);
        return (sum >= 384) as u32;
    }
    s.value_of(color)
}

/// The value of the COLORREF `color` on a palette device.
pub fn device_value(map: &super::palette::DeviceMap, color: u32) -> u32 {
    match color >> 24 {
        1 => map.index((color & 0xFFFF) as usize),
        2 => map.palette_rgb(color & 0xFF_FFFF),
        _ => map.solid(color & 0xFF_FFFF),
    }
}

/// How the brush `handle` fills on the canvas.
pub fn brush_fill(cpu: &mut Cpu, c: &Canvas, handle: u32) -> Fill {
    brush_fill_through(cpu, c, handle, None)
}

/// `brush_fill`, through `map` on a palette device if the caller has the
/// DC's map already: otherwise it builds the map once for all the
/// brush's colors.
fn brush_fill_through(
    cpu: &mut Cpu,
    c: &Canvas,
    handle: u32,
    map: Option<&super::palette::DeviceMap>,
) -> Fill {
    let Some(brush) = gdi(cpu).brush(handle).cloned() else {
        return Fill::None;
    };
    if brush.style == BS_NULL {
        return Fill::None;
    }
    let dc = &c.dc;
    let built;
    let map = match map {
        None if c.surface.device => {
            built = super::palette::DeviceMap::new(cpu, dc.attrs.palette);
            Some(&built)
        }
        _ => map,
    };
    let org = (
        c.origin.0 + dc.attrs.brush_org.0,
        c.origin.1 + dc.attrs.brush_org.1,
    );
    match brush.style {
        BS_SOLID => Fill::Solid(value_through(cpu, dc, &c.surface, brush.color, map)),
        _ => {
            let Some(pattern) = brush.pattern else {
                return Fill::Solid(value_through(cpu, dc, &c.surface, brush.color, map));
            };
            let fg = value_through(cpu, dc, &c.surface, dc.attrs.text_color, map);
            let bg = value_through(cpu, dc, &c.surface, dc.attrs.bk_color, map);
            let line = value_through(cpu, dc, &c.surface, brush.color, map);
            let opaque = dc.attrs.bk_mode == OPAQUE;
            let values = pattern
                .pixels
                .iter()
                .map(|&p| {
                    if brush.style == BS_HATCHED {
                        if p != 0 {
                            Some(line)
                        } else if opaque {
                            Some(bg)
                        } else {
                            None
                        }
                    } else if pattern.mono {
                        Some(if p != 0 { bg } else { fg })
                    } else if c.surface.mono {
                        // A color pattern on a monochrome bitmap: its
                        // pixels' brightness.
                        let sum = (p & 0xFF) + ((p >> 8) & 0xFF) + ((p >> 16) & 0xFF);
                        Some((sum >= 384) as u32)
                    } else if let Some(map) = map {
                        Some(map.blit_color(p, None))
                    } else {
                        Some(c.surface.value_of(p))
                    }
                })
                .collect();
            Fill::Pattern {
                width: pattern.width,
                height: pattern.height,
                org,
                values,
            }
        }
    }
}

/// The 8x8 pattern of a hatch style: 1 where the hatch's lines are.
pub fn hatch_pattern(hatch: u32) -> Pattern {
    let mut pixels = vec![0u32; 64];
    for y in 0..8 {
        for x in 0..8 {
            let on = match hatch {
                0 => y == 7,
                1 => x == 7,
                2 => x == y,
                3 => x + y == 7,
                4 => x == 7 || y == 7,
                _ => x == y || x + y == 7,
            };
            pixels[y * 8 + x] = on as u32;
        }
    }
    Pattern {
        width: 8,
        height: 8,
        pixels,
        mono: true,
    }
}

/// A pen as the canvas draws it.
pub struct Stroke {
    pub value: u32,
    /// Its width in device units; 0 for no line.
    pub width: i32,
    /// The lengths of its dashes and gaps, for a styled line; empty for
    /// a solid one.
    pub dashes: Vec<i32>,
    /// What the gaps are drawn with, on an opaque background.
    pub gap: Option<u32>,
    pub inside: bool,
}

/// How the DC's pen draws on the canvas.
pub fn pen_stroke(cpu: &mut Cpu, c: &Canvas) -> Stroke {
    let dc = &c.dc;
    let pen = gdi(cpu).pen(dc.attrs.pen).cloned().unwrap_or_default();
    let style = pen.style & PS_STYLE_MASK;
    if style == PS_NULL {
        return Stroke {
            value: 0,
            width: 0,
            dashes: Vec::new(),
            gap: None,
            inside: false,
        };
    }
    let width = dc.width_to_device(pen.width).max(1);
    let value = value_for(cpu, dc, &c.surface, pen.color);
    let dashes: Vec<i32> = if width > 1 {
        Vec::new()
    } else {
        match style {
            PS_DASH => vec![18, 6],
            PS_DOT => vec![3, 3],
            PS_DASHDOT => vec![9, 6, 3, 6],
            PS_DASHDOTDOT => vec![9, 3, 3, 3, 3, 3],
            PS_ALTERNATE => vec![1, 1],
            PS_USERSTYLE => pen.dashes.iter().map(|&d| (d as i32).max(1)).collect(),
            _ => Vec::new(),
        }
    };
    let gap = (dc.attrs.bk_mode == OPAQUE && !dashes.is_empty())
        .then(|| value_for(cpu, dc, &c.surface, dc.attrs.bk_color));
    Stroke {
        value,
        width,
        dashes,
        gap,
        inside: style == PS_INSIDEFRAME,
    }
}

/// The points of the line from (`x0`, `y0`) to (`x1`, `y1`), without the
/// last, as Bresenham's algorithm steps. They come one at a time: a line
/// with far coordinates has billions.
pub fn line_points(x0: i32, y0: i32, x1: i32, y1: i32) -> impl Iterator<Item = (i32, i32)> {
    let dx = (x1 as i64 - x0 as i64).abs();
    let dy = -(y1 as i64 - y0 as i64).abs();
    let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
    let mut err = dx + dy;
    let (mut x, mut y) = (x0, y0);
    std::iter::from_fn(move || {
        if (x, y) == (x1, y1) {
            return None;
        }
        let point = (x, y);
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
        Some(point)
    })
}

impl Canvas {
    /// Draw the line between the surface points `a` and `b`, without the
    /// last point, with the stroke; `phase` carries a styled line's
    /// place in its dashes from one segment to the next.
    pub fn line(&mut self, a: (i32, i32), b: (i32, i32), s: &Stroke, rop2: u32, phase: &mut i32) {
        if s.width == 0 {
            return;
        }
        let points = line_points(a.0, a.1, b.0, b.1);
        let period: i32 = s.dashes.iter().sum();
        for (x, y) in points {
            let mut on = true;
            if period > 0 {
                let mut at = phase.rem_euclid(period);
                on = false;
                for (i, &d) in s.dashes.iter().enumerate() {
                    if at < d {
                        on = i % 2 == 0;
                        break;
                    }
                    at -= d;
                }
                *phase += 1;
            }
            let value = if on { Some(s.value) } else { s.gap };
            if let Some(v) = value {
                self.dot(x, y, s.width, v, rop2);
            }
        }
    }

    /// A pen's dot of width `w` centered on (`x`, `y`).
    fn dot(&mut self, x: i32, y: i32, w: i32, value: u32, rop2: u32) {
        if w <= 1 {
            self.plot(x, y, value, rop2);
            return;
        }
        let r = w / 2;
        let r2 = (w * w) / 4;
        for dy in -r..w - r {
            for dx in -r..w - r {
                if dx * dx + dy * dy <= r2 {
                    self.plot(x + dx, y + dy, value, rop2);
                }
            }
        }
    }

    /// Fill the polygons `figures` (surface points) with `fill`: by the
    /// alternate rule, or winding with `winding`.
    pub fn fill_polygons(
        &mut self,
        figures: &[Vec<(i32, i32)>],
        winding: bool,
        fill: &Fill,
        rop2: u32,
    ) {
        for (y, x0, x1) in polygon_spans(figures, winding) {
            self.span(x0, x1, y, fill, rop2);
        }
    }
}

/// The runs of pixels the polygons `figures` cover, as (row, first,
/// after last): a pixel is in when its center is.
pub fn polygon_spans(figures: &[Vec<(i32, i32)>], winding: bool) -> Vec<(i32, i32, i32)> {
    let mut edges: Vec<((i32, i32), (i32, i32))> = Vec::new();
    for f in figures {
        if f.len() < 2 {
            continue;
        }
        for i in 0..f.len() {
            let a = f[i];
            let b = f[(i + 1) % f.len()];
            if a.1 != b.1 {
                edges.push((a, b));
            }
        }
    }
    if edges.is_empty() {
        return Vec::new();
    }
    let top = edges.iter().map(|e| e.0.1.min(e.1.1)).min().unwrap();
    let bottom = edges.iter().map(|e| e.0.1.max(e.1.1)).max().unwrap();
    let mut out = Vec::new();
    let mut xs: Vec<(f64, i32)> = Vec::new();
    for y in top..bottom {
        let cy = y as f64 + 0.5;
        xs.clear();
        for &(a, b) in &edges {
            let (lo, hi, dir) = if a.1 < b.1 { (a, b, 1) } else { (b, a, -1) };
            if cy < lo.1 as f64 || cy >= hi.1 as f64 {
                continue;
            }
            let t = (cy - lo.1 as f64) / (hi.1 - lo.1) as f64;
            xs.push((lo.0 as f64 + t * (hi.0 - lo.0) as f64, dir));
        }
        xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        if winding {
            let mut count = 0;
            for i in 0..xs.len() {
                let before = count;
                count += xs[i].1;
                if before == 0 && count != 0 {
                    // Starts here; ends where the count is 0 again.
                    let start = xs[i].0;
                    let mut j = i + 1;
                    let mut c = count;
                    while j < xs.len() {
                        c += xs[j].1;
                        if c == 0 {
                            break;
                        }
                        j += 1;
                    }
                    let end = xs.get(j).map_or(start, |e| e.0);
                    let (x0, x1) = ((start - 0.5).ceil() as i32, (end - 0.5).ceil() as i32);
                    if x1 > x0 {
                        out.push((y, x0, x1));
                    }
                }
            }
        } else {
            for pair in xs.chunks(2) {
                if let [a, b] = pair {
                    let (x0, x1) = ((a.0 - 0.5).ceil() as i32, (b.0 - 0.5).ceil() as i32);
                    if x1 > x0 {
                        out.push((y, x0, x1));
                    }
                }
            }
        }
    }
    out
}

/// The region the polygons cover, as polygon_spans makes it.
pub fn polygon_region(figures: &[Vec<(i32, i32)>], winding: bool) -> Region {
    let mut spans = polygon_spans(figures, winding);
    spans.sort_unstable();
    Region::from_spans(&spans)
}

/// The DC `hdc`, or None with the error set.
fn dc_of(cpu: &mut Cpu, hdc: u32) -> Option<Dc> {
    match gdi(cpu).dc(hdc) {
        Some(d) => Some(d.clone()),
        None => {
            set_error(cpu, ERROR_INVALID_HANDLE);
            None
        }
    }
}

/// The points at `at` of the program's memory.
fn read_points(cpu: &mut Cpu, at: u32, count: u32) -> Vec<(i32, i32)> {
    let w = read_words(cpu, at, 2 * count as usize);
    w.chunks(2).map(|p| (p[0] as i32, p[1] as i32)).collect()
}

/// The path states.
const PATH_OPEN: u32 = 1;
const PATH_CLOSED: u32 = 2;
const PT_CLOSEFIGURE: u8 = 1;
const PT_LINETO: u8 = 2;
const PT_BEZIERTO: u8 = 4;
const PT_MOVETO: u8 = 6;

/// Whether the DC is making a path: then what it draws goes there.
fn in_path(dc: &Dc) -> bool {
    dc.path_state == PATH_OPEN
}

/// Add device points to the DC's path.
fn path_add(cpu: &mut Cpu, hdc: u32, points: &[(i32, i32)], kind: u8) {
    dc::with(cpu, hdc, |d| {
        for &(x, y) in points {
            d.path.push((x, y, kind));
        }
    });
}

/// Add closed figures (device points) to the DC's path.
fn path_figures(cpu: &mut Cpu, hdc: u32, figures: &[Vec<(i32, i32)>]) {
    for f in figures {
        if f.is_empty() {
            continue;
        }
        path_add(cpu, hdc, &f[..1], PT_MOVETO);
        path_add(cpu, hdc, &f[1..], PT_LINETO);
        dc::with(cpu, hdc, |d| {
            if let Some(last) = d.path.last_mut() {
                last.2 |= PT_CLOSEFIGURE;
            }
        });
    }
}

/// The bounds of device points, a pen's width around them.
fn bounds_of(points: &[(i32, i32)], extra: i32) -> Rect {
    let mut r = Rect::default();
    for &(x, y) in points {
        r = r.union(&Rect::new(x, y, x + 1, y + 1));
    }
    r.inflate(extra, extra)
}

/// Draw outlines and fills: `figures` are device points; closed figures
/// are filled with the brush, if `fill` says so, and all are stroked
/// with the pen.
fn draw_figures(
    cpu: &mut Cpu,
    hdc: u32,
    figures: &[Vec<(i32, i32)>],
    closed: bool,
    fill: bool,
) -> bool {
    let Some(dc) = dc_of(cpu, hdc) else {
        return false;
    };
    let all: Vec<(i32, i32)> = figures.iter().flatten().copied().collect();
    let pen_width = gdi(cpu)
        .pen(dc.attrs.pen)
        .map_or(1, |p| dc.width_to_device(p.width));
    let bounds = bounds_of(&all, pen_width.max(1));
    let Some(mut c) = Canvas::open(cpu, hdc, bounds) else {
        return true;
    };
    let (ox, oy) = c.origin;
    let surface_figures: Vec<Vec<(i32, i32)>> = figures
        .iter()
        .map(|f| f.iter().map(|&(x, y)| (x + ox, y + oy)).collect())
        .collect();
    let rop2 = dc.attrs.rop2;
    if fill && closed {
        let f = brush_fill(cpu, &c, dc.attrs.brush);
        let winding = dc.attrs.poly_fill == dc::WINDING;
        c.fill_polygons(&surface_figures, winding, &f, rop2);
    }
    let stroke = pen_stroke(cpu, &c);
    for f in &surface_figures {
        let mut phase = 0;
        for w in f.windows(2) {
            c.line(w[0], w[1], &stroke, rop2, &mut phase);
        }
        if closed && f.len() > 1 {
            c.line(f[f.len() - 1], f[0], &stroke, rop2, &mut phase);
        } else if !closed && f.len() > 1 && stroke.width > 1 {
            // A wide line's end is drawn too.
            let last = f[f.len() - 1];
            c.line(last, (last.0 + 1, last.1), &stroke, rop2, &mut phase);
        }
    }
    c.close(cpu);
    true
}

/// MoveToEx(hdc, x, y, old).
pub fn move_to_ex(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, x, y, old) = (args[0], args[1] as i32, args[2] as i32, args[3]);
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(FALSE);
    };
    if old != 0 {
        write_words(cpu, old, &[dc.attrs.pos.0 as u32, dc.attrs.pos.1 as u32]);
    }
    dc::with(cpu, hdc, |d| d.attrs.pos = (x, y));
    if in_path(&dc) {
        path_add(cpu, hdc, &[dc.to_device(x, y)], PT_MOVETO);
    }
    Some(TRUE)
}

/// LineTo(hdc, x, y): from the current position, which moves there.
pub fn line_to(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, x, y) = (args[0], args[1] as i32, args[2] as i32);
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(FALSE);
    };
    let from = dc.to_device(dc.attrs.pos.0, dc.attrs.pos.1);
    let to = dc.to_device(x, y);
    dc::with(cpu, hdc, |d| d.attrs.pos = (x, y));
    if in_path(&dc) {
        path_line(cpu, hdc, from, &[to]);
        return Some(TRUE);
    }
    draw_figures(cpu, hdc, &[vec![from, to]], false, false);
    Some(TRUE)
}

/// Lines in a path go from where it is, a move to `from` first if the
/// figure has just started.
fn path_line(cpu: &mut Cpu, hdc: u32, from: (i32, i32), to: &[(i32, i32)]) {
    let starts = gdi(cpu)
        .dc(hdc)
        .is_some_and(|d| d.path.last().is_none_or(|p| p.2 & PT_CLOSEFIGURE != 0));
    if starts {
        path_add(cpu, hdc, &[from], PT_MOVETO);
    }
    path_add(cpu, hdc, to, PT_LINETO);
}

/// Polyline(hdc, points, count).
pub fn polyline(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, at, count) = (args[0], args[1], args[2]);
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(FALSE);
    };
    if count < 2 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let points: Vec<(i32, i32)> = read_points(cpu, at, count)
        .into_iter()
        .map(|(x, y)| dc.to_device(x, y))
        .collect();
    if in_path(&dc) {
        path_add(cpu, hdc, &points[..1], PT_MOVETO);
        path_add(cpu, hdc, &points[1..], PT_LINETO);
        return Some(TRUE);
    }
    draw_figures(cpu, hdc, &[points], false, false);
    Some(TRUE)
}

/// PolylineTo(hdc, points, count): from the current position, which moves
/// to the last point.
pub fn polyline_to(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, at, count) = (args[0], args[1], args[2]);
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(FALSE);
    };
    if count == 0 {
        return Some(TRUE);
    }
    let logical = read_points(cpu, at, count);
    let last = *logical.last().unwrap();
    let mut points = vec![dc.to_device(dc.attrs.pos.0, dc.attrs.pos.1)];
    points.extend(logical.iter().map(|&(x, y)| dc.to_device(x, y)));
    dc::with(cpu, hdc, |d| d.attrs.pos = last);
    if in_path(&dc) {
        path_line(cpu, hdc, points[0], &points[1..]);
        return Some(TRUE);
    }
    draw_figures(cpu, hdc, &[points], false, false);
    Some(TRUE)
}

/// PolyPolyline(hdc, points, counts, polylines).
pub fn poly_polyline(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, at, counts, n) = (args[0], args[1], args[2], args[3]);
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(FALSE);
    };
    let counts = read_words(cpu, counts, n as usize);
    let total: u32 = counts.iter().sum();
    let all = read_points(cpu, at, total);
    let mut figures = Vec::new();
    let mut i = 0;
    for &c in &counts {
        let f: Vec<(i32, i32)> = all[i..i + c as usize]
            .iter()
            .map(|&(x, y)| dc.to_device(x, y))
            .collect();
        i += c as usize;
        figures.push(f);
    }
    if in_path(&dc) {
        for f in &figures {
            if !f.is_empty() {
                path_add(cpu, hdc, &f[..1], PT_MOVETO);
                path_add(cpu, hdc, &f[1..], PT_LINETO);
            }
        }
        return Some(TRUE);
    }
    draw_figures(cpu, hdc, &figures, false, false);
    Some(TRUE)
}

/// Polygon(hdc, points, count).
pub fn polygon(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, at, count) = (args[0], args[1], args[2]);
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(FALSE);
    };
    if count < 2 {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    let points: Vec<(i32, i32)> = read_points(cpu, at, count)
        .into_iter()
        .map(|(x, y)| dc.to_device(x, y))
        .collect();
    if in_path(&dc) {
        path_figures(cpu, hdc, &[points]);
        return Some(TRUE);
    }
    draw_figures(cpu, hdc, &[points], true, true);
    Some(TRUE)
}

/// PolyPolygon(hdc, points, counts, polygons).
pub fn poly_polygon(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, at, counts, n) = (args[0], args[1], args[2], args[3]);
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(FALSE);
    };
    let counts = read_words(cpu, counts, n as usize);
    let total: u32 = counts.iter().sum();
    let all = read_points(cpu, at, total);
    let mut figures = Vec::new();
    let mut i = 0;
    for &c in &counts {
        figures.push(
            all[i..i + c as usize]
                .iter()
                .map(|&(x, y)| dc.to_device(x, y))
                .collect::<Vec<_>>(),
        );
        i += c as usize;
    }
    if in_path(&dc) {
        path_figures(cpu, hdc, &figures);
        return Some(TRUE);
    }
    draw_figures(cpu, hdc, &figures, true, true);
    Some(TRUE)
}

/// The device rectangle a shape function's logical one is: its right and
/// bottom edges are outside it.
fn shape_rect(dc: &Dc, args: &[u32]) -> Rect {
    let r = Rect::new(
        args[1] as i32,
        args[2] as i32,
        args[3] as i32,
        args[4] as i32,
    );
    dc.rect_to_device(&r)
}

/// Rectangle(hdc, left, top, right, bottom): the pen's outline inside
/// the rectangle, the brush within it.
pub fn rectangle(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hdc = args[0];
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(FALSE);
    };
    let r = shape_rect(&dc, args);
    if in_path(&dc) {
        let f = vec![
            (r.left, r.top),
            (r.right - 1, r.top),
            (r.right - 1, r.bottom - 1),
            (r.left, r.bottom - 1),
        ];
        path_figures(cpu, hdc, &[f]);
        return Some(TRUE);
    }
    if r.width() < 1 || r.height() < 1 {
        return Some(TRUE);
    }
    let Some(mut c) = Canvas::open(cpu, hdc, r.inflate(16, 16)) else {
        return Some(TRUE);
    };
    let rop2 = dc.attrs.rop2;
    let stroke = pen_stroke(cpu, &c);
    let s = r.offset(c.origin.0, c.origin.1);
    let f = brush_fill(cpu, &c, dc.attrs.brush);
    if stroke.width == 0 {
        // Without a pen the brush fills one pixel less on the right and
        // at the bottom.
        c.fill(
            Rect::new(s.left, s.top, s.right - 1, s.bottom - 1),
            &f,
            rop2,
        );
    } else {
        let w = stroke.width;
        let (inner, outline) = if stroke.inside || w == 1 {
            let o = Rect::new(
                s.left + w / 2,
                s.top + w / 2,
                s.right - 1 - (w - 1) / 2,
                s.bottom - 1 - (w - 1) / 2,
            );
            (s.inflate(-w, -w), o)
        } else {
            let o = Rect::new(s.left, s.top, s.right - 1, s.bottom - 1);
            (s.inflate(-((w + 1) / 2), -((w + 1) / 2)), o)
        };
        c.fill(inner, &f, rop2);
        let corners = [
            (outline.left, outline.top),
            (outline.right, outline.top),
            (outline.right, outline.bottom),
            (outline.left, outline.bottom),
        ];
        let mut phase = 0;
        for i in 0..4 {
            c.line(corners[i], corners[(i + 1) % 4], &stroke, rop2, &mut phase);
        }
    }
    c.close(cpu);
    Some(TRUE)
}

/// The points of the ellipse in the device rectangle `r`, as a polygon.
fn ellipse_points(r: &Rect) -> Vec<(i32, i32)> {
    let cx = (r.left + r.right - 1) as f64 / 2.0;
    let cy = (r.top + r.bottom - 1) as f64 / 2.0;
    let rx = (r.width() - 1).max(0) as f64 / 2.0;
    let ry = (r.height() - 1).max(0) as f64 / 2.0;
    let n = ((rx + ry) * 2.0).clamp(8.0, 720.0) as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let a = i as f64 * std::f64::consts::TAU / n as f64;
        let p = (
            (cx + rx * a.cos()).round() as i32,
            (cy - ry * a.sin()).round() as i32,
        );
        if out.last() != Some(&p) {
            out.push(p);
        }
    }
    out
}

/// The angle of the point (`x`, `y`) seen from the center of `r`,
/// counterclockwise from the right, in radians, as the ellipse scales it.
fn angle_of(r: &Rect, x: i32, y: i32) -> f64 {
    let cx = (r.left + r.right) as f64 / 2.0;
    let cy = (r.top + r.bottom) as f64 / 2.0;
    let rx = (r.width() as f64 / 2.0).max(1.0);
    let ry = (r.height() as f64 / 2.0).max(1.0);
    (-(y as f64 - cy) / ry).atan2((x as f64 - cx) / rx)
}

/// The points of the arc of the ellipse in `r` from the angle `a0` to
/// `a1`, counterclockwise, or clockwise with `clockwise`.
fn arc_points(r: &Rect, a0: f64, a1: f64, clockwise: bool) -> Vec<(i32, i32)> {
    let tau = std::f64::consts::TAU;
    let cx = (r.left + r.right - 1) as f64 / 2.0;
    let cy = (r.top + r.bottom - 1) as f64 / 2.0;
    let rx = (r.width() - 1).max(0) as f64 / 2.0;
    let ry = (r.height() - 1).max(0) as f64 / 2.0;
    let mut sweep = if clockwise { a0 - a1 } else { a1 - a0 };
    sweep = sweep.rem_euclid(tau);
    if sweep == 0.0 {
        sweep = tau;
    }
    let n = (((rx + ry) * sweep).clamp(4.0, 720.0)) as usize;
    let mut out = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = sweep * i as f64 / n as f64;
        let a = if clockwise { a0 - t } else { a0 + t };
        let p = (
            (cx + rx * a.cos()).round() as i32,
            (cy - ry * a.sin()).round() as i32,
        );
        if out.last() != Some(&p) {
            out.push(p);
        }
    }
    out
}

/// Ellipse(hdc, left, top, right, bottom).
pub fn ellipse(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hdc = args[0];
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(FALSE);
    };
    let r = shape_rect(&dc, args);
    let points = ellipse_points(&r);
    if in_path(&dc) {
        path_figures(cpu, hdc, &[points]);
        return Some(TRUE);
    }
    if r.width() < 2 || r.height() < 2 {
        return Some(TRUE);
    }
    draw_figures(cpu, hdc, &[points], true, true);
    Some(TRUE)
}

/// The kinds of arcs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ArcKind {
    Arc,
    Pie,
    Chord,
}

fn arc_like(cpu: &mut Cpu, args: &[u32], kind: ArcKind) -> Option<u32> {
    let hdc = args[0];
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(FALSE);
    };
    let r = shape_rect(&dc, args);
    let (x0, y0) = dc.to_device(args[5] as i32, args[6] as i32);
    let (x1, y1) = dc.to_device(args[7] as i32, args[8] as i32);
    let clockwise = dc.attrs.arc_dir == 2;
    let a0 = angle_of(&r, x0, y0);
    let a1 = angle_of(&r, x1, y1);
    let mut points = arc_points(&r, a0, a1, clockwise);
    let closed = kind != ArcKind::Arc;
    if kind == ArcKind::Pie {
        let c = ((r.left + r.right - 1) / 2, (r.top + r.bottom - 1) / 2);
        points.push(c);
    }
    if in_path(&dc) {
        if closed {
            path_figures(cpu, hdc, &[points]);
        } else {
            path_add(cpu, hdc, &points[..1], PT_MOVETO);
            path_add(cpu, hdc, &points[1..], PT_LINETO);
        }
        return Some(TRUE);
    }
    draw_figures(cpu, hdc, &[points], closed, closed);
    Some(TRUE)
}

/// Arc(hdc, left, top, right, bottom, xstart, ystart, xend, yend).
pub fn arc(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    arc_like(cpu, args, ArcKind::Arc)
}

pub fn pie(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    arc_like(cpu, args, ArcKind::Pie)
}

pub fn chord(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    arc_like(cpu, args, ArcKind::Chord)
}

/// RoundRect(hdc, left, top, right, bottom, width, height): corners the
/// quarters of an ellipse `width` by `height`.
pub fn round_rect(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hdc = args[0];
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(FALSE);
    };
    let r = shape_rect(&dc, args);
    let ew = dc.width_to_device(args[5] as i32).min(r.width()).max(1);
    let eh = {
        let (_, y0) = dc.to_device(0, 0);
        let (_, y1) = dc.to_device(0, args[6] as i32);
        (y1 - y0).abs().min(r.height()).max(1)
    };
    let quarter =
        |cx: Rect, from: f64| arc_points(&cx, from, from + std::f64::consts::FRAC_PI_2, false);
    let mut points = Vec::new();
    points.extend(quarter(
        Rect::new(r.right - ew, r.top, r.right, r.top + eh),
        0.0,
    ));
    points.extend(quarter(
        Rect::new(r.left, r.top, r.left + ew, r.top + eh),
        std::f64::consts::FRAC_PI_2,
    ));
    points.extend(quarter(
        Rect::new(r.left, r.bottom - eh, r.left + ew, r.bottom),
        std::f64::consts::PI,
    ));
    points.extend(quarter(
        Rect::new(r.right - ew, r.bottom - eh, r.right, r.bottom),
        3.0 * std::f64::consts::FRAC_PI_2,
    ));
    if in_path(&dc) {
        path_figures(cpu, hdc, &[points]);
        return Some(TRUE);
    }
    draw_figures(cpu, hdc, &[points], true, true);
    Some(TRUE)
}

/// The points of the Bézier curves from `start` through `points`, three
/// a curve.
fn bezier_points(start: (i32, i32), points: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut out = vec![start];
    let mut p0 = start;
    for c in points.chunks(3) {
        if c.len() < 3 {
            break;
        }
        let (p1, p2, p3) = (c[0], c[1], c[2]);
        let steps = 16;
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let u = 1.0 - t;
            let f = |a: i32, b: i32, c: i32, d: i32| {
                (u * u * u * a as f64
                    + 3.0 * u * u * t * b as f64
                    + 3.0 * u * t * t * c as f64
                    + t * t * t * d as f64)
                    .round() as i32
            };
            out.push((f(p0.0, p1.0, p2.0, p3.0), f(p0.1, p1.1, p2.1, p3.1)));
        }
        p0 = p3;
    }
    out
}

/// PolyBezier(hdc, points, count).
pub fn poly_bezier(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, at, count) = (args[0], args[1], args[2]);
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(FALSE);
    };
    if count < 4 || (count - 1) % 3 != 0 {
        return Some(FALSE);
    }
    let points: Vec<(i32, i32)> = read_points(cpu, at, count)
        .into_iter()
        .map(|(x, y)| dc.to_device(x, y))
        .collect();
    let line = bezier_points(points[0], &points[1..]);
    if in_path(&dc) {
        path_add(cpu, hdc, &points[..1], PT_MOVETO);
        path_add(cpu, hdc, &points[1..], PT_BEZIERTO);
        return Some(TRUE);
    }
    draw_figures(cpu, hdc, &[line], false, false);
    Some(TRUE)
}

/// PolyBezierTo(hdc, points, count): from the current position.
pub fn poly_bezier_to(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, at, count) = (args[0], args[1], args[2]);
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(FALSE);
    };
    if count == 0 || count % 3 != 0 {
        return Some(FALSE);
    }
    let logical = read_points(cpu, at, count);
    let last = *logical.last().unwrap();
    let points: Vec<(i32, i32)> = logical.iter().map(|&(x, y)| dc.to_device(x, y)).collect();
    let start = dc.to_device(dc.attrs.pos.0, dc.attrs.pos.1);
    dc::with(cpu, hdc, |d| d.attrs.pos = last);
    if in_path(&dc) {
        let starts = dc.path.last().is_none_or(|p| p.2 & PT_CLOSEFIGURE != 0);
        if starts {
            path_add(cpu, hdc, &[start], PT_MOVETO);
        }
        path_add(cpu, hdc, &points, PT_BEZIERTO);
        return Some(TRUE);
    }
    let line = bezier_points(start, &points);
    draw_figures(cpu, hdc, &[line], false, false);
    Some(TRUE)
}

/// PatBlt(hdc, x, y, width, height, rop): the brush, the destination
/// or both.
pub fn pat_blt(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let rop = args[5];
    let r = (args[0], [args[1], args[2], args[3], args[4]]);
    if uses_source((rop >> 16) as u8) {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(FALSE);
    }
    Some(blit(cpu, r.0, r.1.map(|v| v as i32), None, rop) as u32)
}

/// BitBlt(dst, x, y, width, height, src, xsrc, ysrc, rop).
pub fn bit_blt(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let rop = args[8];
    let dst = [args[1], args[2], args[3], args[4]].map(|v| v as i32);
    let src = if uses_source((rop >> 16) as u8) {
        Some((args[5], [args[6] as i32, args[7] as i32, dst[2], dst[3]]))
    } else {
        None
    };
    if src.is_some() && args[5] == 0 {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    Some(blit(cpu, args[0], dst, src, rop) as u32)
}

/// StretchBlt(dst, x, y, width, height, src, xsrc, ysrc, wsrc, hsrc,
/// rop).
pub fn stretch_blt(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let rop = args[10];
    let dst = [args[1], args[2], args[3], args[4]].map(|v| v as i32);
    let src = if uses_source((rop >> 16) as u8) {
        Some((
            args[5],
            [args[6], args[7], args[8], args[9]].map(|v| v as i32),
        ))
    } else {
        None
    };
    if src.is_some() && args[5] == 0 {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    Some(blit(cpu, args[0], dst, src, rop) as u32)
}

/// A source of pixels for a blit: a surface and how its values become
/// the destination's.
pub struct Source {
    pub surface: Surface,
    /// The source DC's background color, for a blit from color to
    /// monochrome.
    pub bk_color: u32,
    /// Where its device origin is.
    pub origin: (i32, i32),
}

/// The blit's work: `dst` is x, y, width and height in the destination
/// DC's logical units; `src` the source DC and its x, y, width and
/// height. Whether it did it.
pub fn blit(
    cpu: &mut Cpu,
    hdst: u32,
    dst: [i32; 4],
    src: Option<(u32, [i32; 4])>,
    rop: u32,
) -> bool {
    let Some(ddc) = dc_of(cpu, hdst) else {
        return false;
    };
    let (x0, y0) = ddc.to_device(dst[0], dst[1]);
    let (x1, y1) = ddc.to_device(dst[0] + dst[2], dst[1] + dst[3]);
    let source = match src {
        Some((hsrc, s)) => {
            let Some(sdc) = dc_of(cpu, hsrc) else {
                return false;
            };
            let (sx0, sy0) = sdc.to_device(s[0], s[1]);
            let (sx1, sy1) = sdc.to_device(s[0] + s[2], s[1] + s[3]);
            let Some(place) = dc::place(cpu, hsrc) else {
                return false;
            };
            let (ox, oy) = place.origin;
            // The pixels it reads: a rectangle that runs backwards covers
            // its start pixel and those before it, and one of no width
            // reads its start column all the same.
            let (top, bottom) = span(sy0, sy1);
            let (left, right) = span(sx0, sx1);
            let (left, right) = (left.min(sx0), right.max(sx0 + 1));
            let area = Rect::new(left + ox, top + oy, right + ox, bottom + oy);
            let Some(surface) = Surface::load_area(cpu, place.target, area) else {
                return false;
            };
            let bk_color = dc::resolve_color(cpu, &sdc, sdc.attrs.bk_color);
            Some((
                Source {
                    surface,
                    bk_color,
                    origin: (ox, oy),
                },
                (sx0, sy0, sx1, sy1),
            ))
        }
        None => None,
    };
    blit_from(cpu, hdst, (x0, y0, x1, y1), source, rop)
}

/// The pixels from `a` to `b`, first and after last: a range that runs
/// backwards covers its start pixel and those before it.
fn span(a: i32, b: i32) -> (i32, i32) {
    if b >= a { (a, b) } else { (b + 1, a + 1) }
}

/// The source coordinate a stretched blit reads for the destination's
/// `p`, of a destination from `d0` to `d1` and a source from `s0` to
/// `s1`: the pixel's steps from the destination's start, scaled down,
/// and backwards where a range runs backwards.
fn scale(p: i32, d0: i32, d1: i32, s0: i32, s1: i32) -> i32 {
    let (d, s) = ((d1 - d0) as i64, (s1 - s0) as i64);
    if d == 0 {
        return s0;
    }
    let f = (p - d0) as i64 * d.signum();
    (s0 as i64 + f * s.abs() / d.abs() * s.signum()) as i32
}

/// A raster operation's work on one pixel.
struct Rop {
    rop3: u8,
    fill: Fill,
    mask: u32,
    /// BLACKNESS's and WHITENESS's values.
    black: u32,
    white: u32,
}

impl Rop {
    /// The destination pixel (`x`, `y`)'s new value from the source's
    /// `s` and its own `d`: None where the pattern leaves it.
    #[inline]
    fn apply(&self, x: i32, y: i32, s: u32, d: u32) -> Option<u32> {
        let p = match self.fill.value_at(x, y) {
            Some(p) => p,
            None if uses_pattern(self.rop3) => return None,
            None => 0,
        };
        Some(match self.rop3 {
            0x00 => self.black,
            0xFF => self.white,
            _ => apply_rop3(self.rop3, p, s, d) & self.mask,
        })
    }
}

/// A blit to the device rectangle from (`x0`, `y0`) to (`x1`, `y1`) of
/// `hdst` (mirrored where they run backwards) from `source`, whose
/// rectangle is in its device coordinates.
pub fn blit_from(
    cpu: &mut Cpu,
    hdst: u32,
    (x0, y0, x1, y1): (i32, i32, i32, i32),
    source: Option<(Source, (i32, i32, i32, i32))>,
    rop: u32,
) -> bool {
    let Some(ddc) = dc_of(cpu, hdst) else {
        return false;
    };
    let rop3 = (rop >> 16) as u8;
    let ((left, right), (top, bottom)) = (span(x0, x1), span(y0, y1));
    let r = Rect::new(left, top, right, bottom);
    if r.is_empty() {
        return true;
    }
    let Some(mut c) = Canvas::open_columns(cpu, hdst, r) else {
        return true;
    };
    let map = c
        .surface
        .device
        .then(|| super::palette::DeviceMap::new(cpu, ddc.attrs.palette));
    let fill = if uses_pattern(rop3) {
        brush_fill_through(cpu, &c, ddc.attrs.brush, map.as_ref())
    } else {
        Fill::Solid(0)
    };
    let text = value_through(cpu, &ddc, &c.surface, ddc.attrs.text_color, map.as_ref());
    let bk = value_through(cpu, &ddc, &c.surface, ddc.attrs.bk_color, map.as_ref());
    let mask = c.surface.value_mask();
    let (black, white) = match rop3 {
        0x00 | 0xFF => black_white(&c.surface),
        _ => (0, 0),
    };
    let op = Rop {
        rop3,
        fill,
        mask,
        black,
        // WHITENESS sets every bit of a pixel without a color table, as
        // the other operations do: a 32-bit pixel's top byte too.
        white: if c.surface.bpp > 8 { mask } else { white },
    };
    let mut xlat = source
        .as_ref()
        .map(|(src, _)| Translate::kept(cpu, &src.surface, map, src.bk_color, text, bk));
    let (ox, oy) = c.origin;
    let rects = std::mem::take(&mut c.rects);
    let dst_rect = r.offset(ox, oy);
    // A blit whose source and destination have the same size and run
    // the same way (one mirrored on both sides included) reads the
    // source pixel at the destination's place moved by this.
    let shift = source.as_ref().and_then(|(src, (sx0, sy0, sx1, sy1))| {
        (sx1 - sx0 == x1 - x0 && sy1 - sy0 == y1 - y0)
            .then_some((sx0 + src.origin.0 - x0 - ox, sy0 + src.origin.1 - y0 - oy))
    });
    let mut values = Vec::new();
    for clip in &rects {
        let area = clip.intersect(&dst_rect);
        if let (Some((src, _)), Some((dx, dy)), Some(t)) = (&source, shift, &mut xlat) {
            // A pixel whose source pixel isn't there stays as it is.
            let area = area.intersect(&src.surface.loaded().offset(-dx, -dy));
            if area.is_empty() {
                continue;
            }
            values.resize((area.right - area.left) as usize, 0);
            for y in area.top..area.bottom {
                src.surface.get_row(area.left + dx, y + dy, &mut values);
                for v in values.iter_mut() {
                    *v = t.get(&src.surface, *v, &c.surface);
                }
                if rop3 == 0xCC {
                    // SRCCOPY: the source's values, a row at a time.
                    for v in values.iter_mut() {
                        *v &= mask;
                    }
                    c.surface.put_row(area.left, y, &values);
                    continue;
                }
                for (x, &s) in (area.left..area.right).zip(&values) {
                    if let Some(v) = op.apply(x, y, s, c.surface.get(x, y)) {
                        c.surface.put(x, y, v);
                    }
                }
            }
            continue;
        }
        for y in area.top..area.bottom {
            let sy = source.as_ref().map_or(0, |(src, (_, sy0, _, sy1))| {
                scale(y - oy, y0, y1, *sy0, *sy1) + src.origin.1
            });
            for x in area.left..area.right {
                let s = match &source {
                    Some((src, (sx0, _, sx1, _))) => {
                        let sx = scale(x - ox, x0, x1, *sx0, *sx1) + src.origin.0;
                        if !src.surface.has(sx, sy) {
                            continue;
                        }
                        let v = src.surface.get(sx, sy);
                        match &mut xlat {
                            Some(t) => t.get(&src.surface, v, &c.surface),
                            None => 0,
                        }
                    }
                    None => 0,
                };
                if let Some(v) = op.apply(x, y, s, c.surface.get(x, y)) {
                    c.surface.put(x, y, v);
                }
            }
        }
    }
    if let Some(t) = xlat {
        t.keep(cpu);
    }
    c.drawn = dst_rect;
    c.close(cpu);
    true
}

/// How a blit's source values become the destination's: for a source of
/// 8 bits a pixel or fewer a table filled in as values come up, for a
/// deeper one the conversion of each value (remembered on a palette
/// device, where it is a search).
pub struct Translate {
    table: Vec<Option<u32>>,
    cache: Option<std::collections::HashMap<u32, u32>>,
    map: Option<super::palette::DeviceMap>,
    src_bk: u32,
    text: u32,
    bk: u32,
    /// What `table` is of, when it can be kept for the next blit.
    key: Option<TableKey>,
}

impl Translate {
    /// The translation from `src`, `map` the destination DC's if the
    /// destination is a palette device; the DCs' colors as `convert`
    /// takes them.
    pub fn new(
        src: &Surface,
        map: Option<super::palette::DeviceMap>,
        src_bk: u32,
        text: u32,
        bk: u32,
    ) -> Translate {
        let table = if src.bpp <= 8 {
            vec![None; 1 << src.bpp]
        } else {
            Vec::new()
        };
        let cache = (src.bpp > 8 && map.is_some()).then(std::collections::HashMap::new);
        Translate {
            table,
            cache,
            map,
            src_bk,
            text,
            bk,
            key: None,
        }
    }

    /// `new`, with the table the last blit kept if it was of the same
    /// translation: on a palette device each of a table's colors takes a
    /// search of the palette, and a program blits from the same colors
    /// many times.
    pub fn kept(
        cpu: &mut Cpu,
        src: &Surface,
        map: Option<super::palette::DeviceMap>,
        src_bk: u32,
        text: u32,
        bk: u32,
    ) -> Translate {
        let key = table_key(src, map.as_ref());
        let kept = key
            .as_ref()
            .and_then(|key| gdi(cpu).kept_table.take_if(|k| k.key == *key));
        let mut t = Translate::new(src, map, src_bk, text, bk);
        if let Some(kept) = kept {
            t.table = kept.table;
        }
        t.key = key;
        t
    }

    /// Keep the table for the next blit, as `kept` takes it.
    pub fn keep(self, cpu: &mut Cpu) {
        if let Some(key) = self.key {
            gdi(cpu).kept_table = Some(Box::new(KeptTable {
                key,
                table: self.table,
            }));
        }
    }

    fn one(&self, src: &Surface, v: u32, dst: &Surface) -> u32 {
        if let Some(map) = &self.map
            && !src.mono
        {
            if let Some(indices) = &src.pal_indices {
                return indices.get(v as usize).map_or(0, |&i| map.index(i as usize));
            }
            if src.device {
                return v;
            }
            let hint = (src.bpp <= 8).then_some(v);
            return map.blit_color(src.color_of(v), hint);
        }
        convert(src, v, dst, self.src_bk, self.text, self.bk)
    }

    /// The destination's value of the source's value `v`.
    #[inline]
    pub fn get(&mut self, src: &Surface, v: u32, dst: &Surface) -> u32 {
        if let Some(&slot) = self.table.get(v as usize) {
            if let Some(t) = slot {
                return t;
            }
            let t = self.one(src, v, dst);
            self.table[v as usize] = Some(t);
            return t;
        }
        if self.cache.is_some() {
            if let Some(&t) = self.cache.as_ref().and_then(|c| c.get(&v)) {
                return t;
            }
            let t = self.one(src, v, dst);
            if let Some(c) = &mut self.cache {
                c.insert(v, t);
            }
            return t;
        }
        self.one(src, v, dst)
    }
}

/// What a table of translations from `src` through `map` is of when it
/// can be kept: a source with a color table onto a palette device, where
/// a value's translation depends on these and nothing else
/// (`Translate::one`).
fn table_key(src: &Surface, map: Option<&super::palette::DeviceMap>) -> Option<TableKey> {
    let map = map?;
    (!src.mono && src.bpp <= 8).then(|| TableKey {
        bpp: src.bpp,
        colors: src.colors.clone(),
        pal_indices: src.pal_indices.clone(),
        device: src.device,
        map: map.clone(),
    })
}

/// What a kept translation table is of (`table_key`).
#[derive(Debug, PartialEq)]
pub struct TableKey {
    bpp: u32,
    colors: Vec<u32>,
    pal_indices: Option<Vec<u16>>,
    device: bool,
    map: super::palette::DeviceMap,
}

/// The table of the last blit from a color table onto a palette device.
#[derive(Debug)]
pub struct KeptTable {
    key: TableKey,
    table: Vec<Option<u32>>,
}

/// A source pixel's value `v` as the destination's: a monochrome
/// bitmap's 0 and 1 are the destination DC's text and background
/// colors; a color becomes 1 on a monochrome bitmap when it is the source
/// DC's background color.
#[inline]
pub fn convert(src: &Surface, v: u32, dst: &Surface, src_bk: u32, text: u32, bk: u32) -> u32 {
    if src.mono && dst.mono {
        return v;
    }
    if src.mono {
        return if v != 0 { bk } else { text };
    }
    if dst.mono {
        return (src.color_of(v) & 0xFFFFFF == src_bk & 0xFFFFFF) as u32;
    }
    if src.bpp == dst.bpp && src.bpp > 8 && src.masks == dst.masks {
        return v;
    }
    if src.bpp == dst.bpp && src.bpp <= 8 && src.colors == dst.colors {
        return v;
    }
    dst.value_of(src.color_of(v))
}

/// SetPixel(hdc, x, y, color): the color it set.
pub fn set_pixel(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hdc = args[0];
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(super::CLR_INVALID);
    };
    let (x, y) = dc.to_device(args[1] as i32, args[2] as i32);
    let Some(mut c) = Canvas::open(cpu, hdc, Rect::new(x, y, x + 1, y + 1)) else {
        return Some(super::CLR_INVALID);
    };
    let v = value_for(cpu, &dc, &c.surface, args[3]);
    let (sx, sy) = c.at(x, y);
    c.plot(sx, sy, v, dc::R2_COPYPEN);
    let color = if c.surface.mono {
        if v != 0 { 0xFFFFFF } else { 0 }
    } else {
        c.surface.color_of(v)
    };
    c.close(cpu);
    Some(color)
}

/// SetPixelV(hdc, x, y, color).
pub fn set_pixel_v(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let r = set_pixel(cpu, args)?;
    Some((r != super::CLR_INVALID) as u32)
}

/// GetPixel(hdc, x, y): CLR_INVALID outside the region it may draw on.
pub fn get_pixel(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hdc = args[0];
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(super::CLR_INVALID);
    };
    let (x, y) = dc.to_device(args[1] as i32, args[2] as i32);
    let Some(c) = Canvas::open(cpu, hdc, Rect::new(x, y, x + 1, y + 1)) else {
        return Some(super::CLR_INVALID);
    };
    let (sx, sy) = c.at(x, y);
    if !c.visible(sx, sy) {
        return Some(super::CLR_INVALID);
    }
    let v = c.surface.get(sx, sy);
    Some(if c.surface.mono {
        if v != 0 { 0xFFFFFF } else { 0 }
    } else {
        c.surface.color_of(v)
    })
}

/// FloodFill(hdc, x, y, color): fill out to the border of `color`.
pub fn flood_fill(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    ext_flood_fill(cpu, &[args[0], args[1], args[2], args[3], 0])
}

/// ExtFloodFill(hdc, x, y, color, type): FLOODFILLBORDER (0) fills out to
/// pixels of `color`; FLOODFILLSURFACE (1) fills the pixels of `color`.
pub fn ext_flood_fill(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hdc = args[0];
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(FALSE);
    };
    let (x, y) = dc.to_device(args[1] as i32, args[2] as i32);
    let (w, h) = dc::device_size(cpu, &dc);
    let Some(mut c) = Canvas::open(cpu, hdc, Rect::new(-w, -h, 2 * w, 2 * h)) else {
        return Some(FALSE);
    };
    let (sx, sy) = c.at(x, y);
    if !c.visible(sx, sy) {
        return Some(FALSE);
    }
    let target = value_for(cpu, &dc, &c.surface, args[3]);
    let surface_kind = args[4] == 1;
    let fill = brush_fill(cpu, &c, dc.attrs.brush);
    let inside = |c: &Canvas, x: i32, y: i32| {
        let v = c.surface.get(x, y);
        if surface_kind {
            v == target
        } else {
            v != target
        }
    };
    if !inside(&c, sx, sy) {
        return Some(FALSE);
    }
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![(sx, sy)];
    let mut spots = Vec::new();
    while let Some((px, py)) = stack.pop() {
        if !c.visible(px, py) || !seen.insert((px, py)) || !inside(&c, px, py) {
            continue;
        }
        spots.push((px, py));
        stack.extend([(px + 1, py), (px - 1, py), (px, py + 1), (px, py - 1)]);
    }
    for (px, py) in spots {
        if let Some(v) = fill.value_at(px, py) {
            c.plot(px, py, v, dc::R2_COPYPEN);
        }
    }
    c.close(cpu);
    Some(TRUE)
}

/// Fill the region `region` (device coordinates of `hdc`) with the brush
/// `brush` through `rop2`.
fn fill_region(cpu: &mut Cpu, hdc: u32, region: &Region, brush: u32, rop2: u32) -> bool {
    let Some(mut c) = Canvas::open(cpu, hdc, region.bounds()) else {
        return gdi(cpu).dc(hdc).is_some();
    };
    let f = brush_fill(cpu, &c, brush);
    let (ox, oy) = c.origin;
    for r in &region.rects {
        c.fill(r.offset(ox, oy), &f, rop2);
    }
    c.close(cpu);
    true
}

/// A region object in the DC's device coordinates: regions are in
/// logical units for these functions.
fn device_region(cpu: &mut Cpu, dc: &Dc, hrgn: u32) -> Option<Region> {
    let region = gdi(cpu).region(hrgn)?.clone();
    let mut out = Region::new();
    for r in &region.rects {
        out = out.or_rect(&dc.rect_to_device(r));
    }
    Some(out)
}

/// FillRgn(hdc, region, brush).
pub fn fill_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = dc_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    let Some(region) = device_region(cpu, &dc, args[1]) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    if gdi(cpu).brush(args[2]).is_none() {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    }
    Some(fill_region(cpu, args[0], &region, args[2], dc::R2_COPYPEN) as u32)
}

/// PaintRgn(hdc, region): with the DC's brush.
pub fn paint_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = dc_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    let Some(region) = device_region(cpu, &dc, args[1]) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    Some(fill_region(cpu, args[0], &region, dc.attrs.brush, dc.attrs.rop2) as u32)
}

/// InvertRgn(hdc, region).
pub fn invert_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = dc_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    let Some(region) = device_region(cpu, &dc, args[1]) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    let black = super::handle(super::BLACK_BRUSH as usize);
    // R2_NOT.
    Some(fill_region(cpu, args[0], &region, black, 6) as u32)
}

/// FrameRgn(hdc, region, brush, width, height): the brush along the
/// region's edges, inside it.
pub fn frame_rgn(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(dc) = dc_of(cpu, args[0]) else {
        return Some(FALSE);
    };
    let Some(region) = device_region(cpu, &dc, args[1]) else {
        set_error(cpu, ERROR_INVALID_HANDLE);
        return Some(FALSE);
    };
    let (w, h) = (args[3] as i32, args[4] as i32);
    // The frame: the region less the region moved in by the width and
    // height every way.
    let mut inner = region.clone();
    for (dx, dy) in [(w, 0), (-w, 0), (0, h), (0, -h)] {
        inner = inner.and(&region.offset_by(dx, dy));
    }
    let frame = region.diff(&inner);
    Some(fill_region(cpu, args[0], &frame, args[2], dc::R2_COPYPEN) as u32)
}

/// BeginPath(hdc).
pub fn begin_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(
        dc::with(cpu, args[0], |d| {
            d.path.clear();
            d.path_state = PATH_OPEN;
            TRUE
        })
        .unwrap_or(FALSE),
    )
}

/// EndPath(hdc).
pub fn end_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(
        dc::with(cpu, args[0], |d| {
            if d.path_state != PATH_OPEN {
                return FALSE;
            }
            d.path_state = PATH_CLOSED;
            TRUE
        })
        .unwrap_or(FALSE),
    )
}

/// AbortPath(hdc).
pub fn abort_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(
        dc::with(cpu, args[0], |d| {
            d.path.clear();
            d.path_state = 0;
            TRUE
        })
        .unwrap_or(FALSE),
    )
}

/// CloseFigure(hdc).
pub fn close_figure(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    Some(
        dc::with(cpu, args[0], |d| {
            if d.path_state != PATH_OPEN {
                return FALSE;
            }
            if let Some(last) = d.path.last_mut() {
                last.2 |= PT_CLOSEFIGURE;
            }
            TRUE
        })
        .unwrap_or(FALSE),
    )
}

/// The path's figures as polygons (Bézier curves flattened), and
/// whether each is closed.
fn path_polygons(path: &[(i32, i32, u8)]) -> Vec<(Vec<(i32, i32)>, bool)> {
    let mut out: Vec<(Vec<(i32, i32)>, bool)> = Vec::new();
    let mut i = 0;
    while i < path.len() {
        let (x, y, kind) = path[i];
        if kind & !PT_CLOSEFIGURE == PT_MOVETO || out.is_empty() || out.last().unwrap().1 {
            out.push((vec![(x, y)], false));
            if kind & !PT_CLOSEFIGURE != PT_MOVETO {
                // A figure that went on after it was closed.
                i += 1;
                continue;
            }
        } else if kind & !PT_CLOSEFIGURE == PT_BEZIERTO {
            let start = *out.last().unwrap().0.last().unwrap();
            let ctrl: Vec<(i32, i32)> = path[i..(i + 3).min(path.len())]
                .iter()
                .map(|p| (p.0, p.1))
                .collect();
            let pts = bezier_points(start, &ctrl);
            let f = out.last_mut().unwrap();
            f.0.extend(&pts[1..]);
            let closed = path[(i + 2).min(path.len() - 1)].2 & PT_CLOSEFIGURE != 0;
            f.1 = closed;
            i += 3;
            continue;
        } else {
            out.last_mut().unwrap().0.push((x, y));
        }
        if kind & PT_CLOSEFIGURE != 0 {
            out.last_mut().unwrap().1 = true;
        }
        i += 1;
    }
    out
}

/// Take the DC's finished path; None (with the DC's path left) if it
/// has none.
fn take_path(cpu: &mut Cpu, hdc: u32) -> Option<Vec<(i32, i32, u8)>> {
    dc::with(cpu, hdc, |d| {
        if d.path_state != PATH_CLOSED {
            return None;
        }
        d.path_state = 0;
        Some(std::mem::take(&mut d.path))
    })
    .flatten()
}

fn draw_path(cpu: &mut Cpu, hdc: u32, stroke: bool, fill: bool) -> Option<u32> {
    let Some(path) = take_path(cpu, hdc) else {
        return Some(FALSE);
    };
    let polys = path_polygons(&path);
    if fill {
        // Every figure is closed for the fill.
        let figures: Vec<Vec<(i32, i32)>> = polys.iter().map(|p| p.0.clone()).collect();
        fill_figures(cpu, hdc, &figures);
    }
    if stroke {
        for (f, closed) in &polys {
            draw_figures(cpu, hdc, std::slice::from_ref(f), *closed, false);
        }
    }
    Some(TRUE)
}

/// Fill the device polygons with the DC's brush, without outlines.
fn fill_figures(cpu: &mut Cpu, hdc: u32, figures: &[Vec<(i32, i32)>]) {
    let Some(dc) = gdi(cpu).dc(hdc).cloned() else {
        return;
    };
    let all: Vec<(i32, i32)> = figures.iter().flatten().copied().collect();
    let Some(mut c) = Canvas::open(cpu, hdc, bounds_of(&all, 1)) else {
        return;
    };
    let (ox, oy) = c.origin;
    let figures: Vec<Vec<(i32, i32)>> = figures
        .iter()
        .map(|f| f.iter().map(|&(x, y)| (x + ox, y + oy)).collect())
        .collect();
    let f = brush_fill(cpu, &c, dc.attrs.brush);
    c.fill_polygons(
        &figures,
        dc.attrs.poly_fill == dc::WINDING,
        &f,
        dc.attrs.rop2,
    );
    c.close(cpu);
}

pub fn fill_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    draw_path(cpu, args[0], false, true)
}

pub fn stroke_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    draw_path(cpu, args[0], true, false)
}

pub fn stroke_and_fill_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    draw_path(cpu, args[0], true, true)
}

/// FlattenPath(hdc): its curves become lines.
pub fn flatten_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let hdc = args[0];
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(FALSE);
    };
    if dc.path_state != PATH_CLOSED {
        return Some(FALSE);
    }
    let polys = path_polygons(&dc.path);
    let mut path = Vec::new();
    for (f, closed) in polys {
        for (i, &(x, y)) in f.iter().enumerate() {
            let mut kind = if i == 0 { PT_MOVETO } else { PT_LINETO };
            if closed && i == f.len() - 1 {
                kind |= PT_CLOSEFIGURE;
            }
            path.push((x, y, kind));
        }
    }
    dc::with(cpu, hdc, |d| d.path = path);
    Some(TRUE)
}

/// The region of the DC's path, in device coordinates.
fn path_region(cpu: &mut Cpu, hdc: u32) -> Option<Region> {
    let dc = gdi(cpu).dc(hdc)?.clone();
    let path = take_path(cpu, hdc)?;
    let figures: Vec<Vec<(i32, i32)>> = path_polygons(&path).into_iter().map(|p| p.0).collect();
    Some(polygon_region(&figures, dc.attrs.poly_fill == dc::WINDING))
}

/// PathToRegion(hdc): a new region of the path.
pub fn path_to_region(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(region) = path_region(cpu, args[0]) else {
        return Some(0);
    };
    Some(super::add(cpu, super::Kind::Region { region }))
}

/// SelectClipPath(hdc, mode).
pub fn select_clip_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let Some(region) = path_region(cpu, args[0]) else {
        return Some(FALSE);
    };
    let r = dc::select_clip(cpu, args[0], Some(region), args[1]);
    Some((r != 0) as u32)
}

/// GetPath(hdc, points, kinds, size): the path's points in logical
/// units; with a size of 0, how many there are.
pub fn get_path(cpu: &mut Cpu, args: &[u32]) -> Option<u32> {
    let (hdc, points, kinds, size) = (args[0], args[1], args[2], args[3] as i32);
    let Some(dc) = dc_of(cpu, hdc) else {
        return Some(-1i32 as u32);
    };
    if dc.path_state != PATH_CLOSED {
        return Some(-1i32 as u32);
    }
    let n = dc.path.len() as i32;
    if size == 0 {
        return Some(n as u32);
    }
    if size < n {
        set_error(cpu, ERROR_INVALID_PARAMETER);
        return Some(-1i32 as u32);
    }
    for (i, &(x, y, kind)) in dc.path.iter().enumerate() {
        let (lx, ly) = dc.to_logical(x, y);
        write_words(cpu, points + 8 * i as u32, &[lx as u32, ly as u32]);
        cpu.bus.guest_write_8(kinds + i as u32, kind);
    }
    Some(n as u32)
}

/// Draw on the DC `hdc` without its mapping: fill the device rectangle
/// `r` with the brush `brush`, for USER's painting.
pub fn fill_device_rect(cpu: &mut Cpu, hdc: u32, r: Rect, brush: u32) -> bool {
    fill_region(cpu, hdc, &Region::from_rect(r), brush, dc::R2_COPYPEN)
}

/// Invert the device rectangle `r` of `hdc`.
pub fn invert_device_rect(cpu: &mut Cpu, hdc: u32, r: Rect) {
    let black = super::handle(super::BLACK_BRUSH as usize);
    fill_region(cpu, hdc, &Region::from_rect(r), black, 6);
}

/// Draw the device rectangle `r`'s dotted outline by XOR, as
/// DrawFocusRect does.
pub fn focus_device_rect(cpu: &mut Cpu, hdc: u32, r: Rect) {
    let Some(mut c) = Canvas::open(cpu, hdc, r) else {
        return;
    };
    let s = r.offset(c.origin.0, c.origin.1);
    let mask = c.surface.value_mask();
    let on = |i: i32| i % 2 == 0;
    for x in s.left..s.right {
        if on(x - s.left) {
            c.plot(x, s.top, mask, 7);
            if s.bottom - 1 > s.top {
                c.plot(x, s.bottom - 1, mask, 7);
            }
        }
    }
    for y in s.top + 1..s.bottom - 1 {
        if on(y - s.top) {
            c.plot(s.left, y, mask, 7);
            if s.right - 1 > s.left {
                c.plot(s.right - 1, y, mask, 7);
            }
        }
    }
    c.close(cpu);
}

/// Draw the device line from `a` to `b` (without its last point) in
/// `color` (a COLORREF) on `hdc`, for USER's frames.
pub fn device_line(cpu: &mut Cpu, hdc: u32, a: (i32, i32), b: (i32, i32), color: u32) {
    let bounds = Rect::new(
        a.0.min(b.0),
        a.1.min(b.1),
        a.0.max(b.0) + 1,
        a.1.max(b.1) + 1,
    );
    let Some(mut c) = Canvas::open(cpu, hdc, bounds) else {
        return;
    };
    let dc = c.dc.clone();
    let v = value_for(cpu, &dc, &c.surface, color);
    let (ox, oy) = c.origin;
    for (x, y) in line_points(a.0, a.1, b.0, b.1) {
        c.plot(x + ox, y + oy, v, dc::R2_COPYPEN);
    }
    c.close(cpu);
}
