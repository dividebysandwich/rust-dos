//! Drawing the settings window: a grid of CP437 character cells, rendered
//! with the VGA font onto the picture over a semi-transparent panel.

use crate::video::{self, CP437, Frame};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

pub const TEXT: Rgb = Rgb(0xD8, 0xDC, 0xE4);
pub const BRIGHT: Rgb = Rgb(0xFF, 0xFF, 0xFF);
pub const DIM: Rgb = Rgb(0x88, 0x92, 0xA8);
pub const BORDER: Rgb = Rgb(0x70, 0x9C, 0xE0);
pub const KEY: Rgb = Rgb(0xFF, 0xD8, 0x60);
pub const NOTE: Rgb = Rgb(0xC8, 0xB0, 0x68);
pub const ERROR: Rgb = Rgb(0xFF, 0x70, 0x70);
pub const GOOD: Rgb = Rgb(0x80, 0xE0, 0x90);
/// Background of the selected row and the focused control.
pub const SELECT: Rgb = Rgb(0x1C, 0x78, 0xA8);
/// Background of text fields.
pub const FIELD: Rgb = Rgb(0x08, 0x10, 0x2C);
/// The panel, blended over the picture at `PANEL_ALPHA`/256.
const PANEL: Rgb = Rgb(0x10, 0x20, 0x50);
pub const PANEL_ALPHA: u32 = 216;
/// How opaque something drawn over the picture is, of 256: wholly.
pub const OPAQUE: u32 = 256;

/// `over` over `under`, `alpha`/256 opaque.
fn blend(under: [u8; 3], over: Rgb, alpha: u32) -> [u8; 3] {
    let mix = |d: u8, c: u8| ((d as u32 * (256 - alpha) + c as u32 * alpha) >> 8) as u8;
    [mix(under[0], over.0), mix(under[1], over.1), mix(under[2], over.2)]
}

/// Most cells the panel takes, and the margin it leaves around it.
const MAX_COLS: usize = 96;
const MAX_ROWS: usize = 26;
const MARGIN_COLS: usize = 2;
const MARGIN_ROWS: usize = 1;

/// The character code of `c` in code page 437, '?' if it has none.
pub fn cp437(c: char) -> u8 {
    if (' '..='~').contains(&c) {
        return c as u8;
    }
    CP437.iter().position(|&x| x == c).map_or(b'?', |i| i as u8)
}

#[derive(Clone, Copy)]
struct Cell {
    ch: u8,
    fg: Rgb,
    /// An opaque background; None shows the panel.
    bg: Option<Rgb>,
}

/// The window's text, cell by cell.
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    cells: Vec<Cell>,
}

impl Grid {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self { cols, rows, cells: vec![Cell { ch: b' ', fg: TEXT, bg: None }; cols * rows] }
    }

    /// Write `text` from (`col`, `row`), clipped at `end` (a column) or the
    /// grid's edge. Returns the column after it.
    pub fn text_to(&mut self, col: usize, row: usize, text: &str, fg: Rgb, end: usize) -> usize {
        let end = end.min(self.cols);
        let mut x = col;
        for c in text.chars() {
            if x >= end || row >= self.rows {
                break;
            }
            let cell = &mut self.cells[row * self.cols + x];
            cell.ch = cp437(c);
            cell.fg = fg;
            x += 1;
        }
        x
    }

    pub fn text(&mut self, col: usize, row: usize, text: &str, fg: Rgb) -> usize {
        self.text_to(col, row, text, fg, self.cols)
    }

    pub fn char(&mut self, col: usize, row: usize, ch: u8, fg: Rgb) {
        if col < self.cols && row < self.rows {
            let cell = &mut self.cells[row * self.cols + col];
            cell.ch = ch;
            cell.fg = fg;
        }
    }

    /// The character in a cell.
    #[cfg(test)]
    pub fn cell(&self, col: usize, row: usize) -> u8 {
        self.cells[row * self.cols + col].ch
    }

    /// Give `width` cells from (`col`, `row`) an opaque background.
    pub fn background(&mut self, col: usize, row: usize, width: usize, bg: Rgb) {
        if row < self.rows {
            for x in col..(col + width).min(self.cols) {
                self.cells[row * self.cols + x].bg = Some(bg);
            }
        }
    }

    /// Draw a horizontal line of `ch` with `left` and `right` ends.
    pub fn line(&mut self, row: usize, left: u8, ch: u8, right: u8, fg: Rgb) {
        for x in 0..self.cols {
            let c = if x == 0 { left } else if x + 1 == self.cols { right } else { ch };
            self.char(x, row, c, fg);
        }
    }
}

/// Where the window sits on the picture and how big its cells are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Top left corner, in frame pixels.
    pub x: usize,
    pub y: usize,
    /// Cell height: the 8x16 font, or 8x8 on small pictures.
    pub cell_h: usize,
    pub cols: usize,
    pub rows: usize,
}

impl Layout {
    /// A panel centred on a `width` x `height` picture.
    pub fn for_frame(width: usize, height: usize) -> Self {
        let cell_h = if width / 8 >= 64 && height / 16 >= 20 { 16 } else { 8 };
        let cols = (width / 8).saturating_sub(2 * MARGIN_COLS).min(MAX_COLS);
        let rows = (height / cell_h).saturating_sub(2 * MARGIN_ROWS).min(MAX_ROWS);
        Self { x: (width - cols * 8) / 2, y: (height - rows * cell_h) / 2, cell_h, cols, rows }
    }

    /// The cell at frame pixel (`x`, `y`), if it is on the panel.
    pub fn cell_at(&self, x: i32, y: i32) -> Option<(usize, usize)> {
        let col = (x - self.x as i32).div_euclid(8);
        let row = (y - self.y as i32).div_euclid(self.cell_h as i32);
        ((0..self.cols as i32).contains(&col) && (0..self.rows as i32).contains(&row))
            .then_some((col as usize, row as usize))
    }
}

/// Draw `grid` onto `frame` at `layout`: the panel blended over the
/// picture, then the cells' backgrounds and characters.
pub fn render(grid: &Grid, layout: &Layout, frame: &mut Frame) {
    render_blended(grid, layout, frame, PANEL_ALPHA);
}

/// `render` with the panel `alpha`/256 opaque.
pub fn render_blended(grid: &Grid, layout: &Layout, frame: &mut Frame, alpha: u32) {
    let font = if layout.cell_h == 16 { video::font_8x16() } else { video::font_8x8() };
    let width = frame.width as usize;
    for row in 0..grid.rows.min(layout.rows) {
        for col in 0..grid.cols.min(layout.cols) {
            let cell = grid.cells[row * grid.cols + col];
            let glyph = &font[cell.ch as usize * layout.cell_h..][..layout.cell_h];
            for (gy, bits) in glyph.iter().enumerate() {
                let y = layout.y + row * layout.cell_h + gy;
                let start = (y * width + layout.x + col * 8) * 3;
                let Some(pixels) = frame.rgb.get_mut(start..start + 24) else { continue };
                for (gx, px) in pixels.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                    *px = if bits & (0x80 >> gx) != 0 {
                        [cell.fg.0, cell.fg.1, cell.fg.2]
                    } else if let Some(bg) = cell.bg {
                        [bg.0, bg.1, bg.2]
                    } else {
                        blend(*px, PANEL, alpha)
                    };
                }
            }
        }
    }
}

/// A graph of `values`, oldest first, the newest at the right edge and
/// the stats' `HISTORY` of them across the whole width, on a scale from
/// 0 to `max`, in the cells from (`col`, `row`), `cols` wide and `rows`
/// high, drawn onto `frame` where `layout` put the panel. Its background,
/// its lines at each quarter of the scale and what is below the values
/// are `alpha`/256 opaque, the values' line wholly.
pub fn plot(frame: &mut Frame, layout: &Layout, cells: (usize, usize, usize, usize), values: &[f32], max: f32, color: Rgb, alpha: u32) {
    let (col, row, cols, rows) = cells;
    let (x0, y0) = (layout.x + col * 8, layout.y + row * layout.cell_h);
    let (w, h) = (cols * 8, rows * layout.cell_h);
    let (width, height) = (frame.width as usize, frame.height as usize);
    let history = crate::stats::HISTORY;
    let faint = Rgb(color.0 / 3, color.1 / 3, color.2 / 3);
    for i in 0..w {
        // The sample this column shows, counting from the newest, and the
        // height of its bar.
        let back = (w - 1 - i) * history / w;
        let value = values.len().checked_sub(back + 1).and_then(|at| values.get(at)).filter(|_| max > 0.0);
        let bar = value.map_or(0, |&v| ((v / max).clamp(0.0, 1.0) * h as f32).round() as usize);
        let x = x0 + i;
        for dy in 0..h {
            let y = y0 + h - 1 - dy;
            if x >= width || y >= height {
                continue;
            }
            // A dotted line at each quarter of the scale.
            let quarter = (1..4).any(|q| y == y0 + h - h * q / 4) && x % 2 == 0;
            let (c, a) = match dy + 1 {
                top if top == bar => (color, OPAQUE),
                top if top < bar => (faint, alpha),
                _ if quarter => (DIM, alpha),
                _ => (FIELD, alpha),
            };
            let at = (y * width + x) * 3;
            let px = &mut frame.rgb[at..at + 3];
            px.copy_from_slice(&blend([px[0], px[1], px[2]], c, a));
        }
    }
}

/// A pixel font for big numbers, drawn with the half block characters: a
/// cell is two pixels, one above the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BigFont {
    /// 5x7 pixels, four cells high.
    Large,
    /// 3x5 pixels, three cells high.
    Small,
}

impl BigFont {
    /// A glyph's width and height in pixels.
    fn size(self) -> (usize, usize) {
        match self {
            BigFont::Large => (5, 7),
            BigFont::Small => (3, 5),
        }
    }

    /// The rows of `c`'s pixels, top first, the leftmost pixel in the
    /// highest of the glyph's bits: the digits, '%', '.' and '-'.
    fn glyph(self, c: char) -> Option<&'static [u8]> {
        const LARGE: [(char, [u8; 7]); 13] = [
            ('0', [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110]),
            ('1', [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110]),
            ('2', [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111]),
            ('3', [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110]),
            ('4', [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010]),
            ('5', [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110]),
            ('6', [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110]),
            ('7', [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000]),
            ('8', [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110]),
            ('9', [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100]),
            ('%', [0b11000, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b00011]),
            ('.', [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100]),
            ('-', [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000]),
        ];
        const SMALL: [(char, [u8; 5]); 13] = [
            ('0', [0b111, 0b101, 0b101, 0b101, 0b111]),
            ('1', [0b010, 0b110, 0b010, 0b010, 0b111]),
            ('2', [0b111, 0b001, 0b111, 0b100, 0b111]),
            ('3', [0b111, 0b001, 0b011, 0b001, 0b111]),
            ('4', [0b101, 0b101, 0b111, 0b001, 0b001]),
            ('5', [0b111, 0b100, 0b111, 0b001, 0b111]),
            ('6', [0b111, 0b100, 0b111, 0b101, 0b111]),
            ('7', [0b111, 0b001, 0b001, 0b010, 0b010]),
            ('8', [0b111, 0b101, 0b111, 0b101, 0b111]),
            ('9', [0b111, 0b101, 0b111, 0b001, 0b111]),
            ('%', [0b101, 0b001, 0b010, 0b100, 0b101]),
            ('.', [0b000, 0b000, 0b000, 0b000, 0b010]),
            ('-', [0b000, 0b000, 0b111, 0b000, 0b000]),
        ];
        match self {
            BigFont::Large => LARGE.iter().find(|(g, _)| *g == c).map(|(_, rows)| &rows[..]),
            BigFont::Small => SMALL.iter().find(|(g, _)| *g == c).map(|(_, rows)| &rows[..]),
        }
    }

    /// The cells `text` takes across, with a column between the glyphs.
    pub fn width(self, text: &str) -> usize {
        (text.chars().count() * (self.size().0 + 1)).saturating_sub(1)
    }

    /// The cells it takes down.
    pub fn height(self) -> usize {
        self.size().1.div_ceil(2)
    }
}

/// Write `text` in `font` from (`col`, `row`), clipped at the grid's edge;
/// characters the font doesn't have are left blank. Returns the column
/// after it.
pub fn big_text(g: &mut Grid, col: usize, row: usize, text: &str, font: BigFont, fg: Rgb) -> usize {
    let (width, height) = font.size();
    let mut x = col;
    for c in text.chars() {
        if let Some(rows) = font.glyph(c) {
            for cell in 0..font.height() {
                let pixel = |y: usize, bit: usize| y < height && rows[y] & (1 << (width - 1 - bit)) != 0;
                for bit in 0..width {
                    let ch = match (pixel(cell * 2, bit), pixel(cell * 2 + 1, bit)) {
                        (true, true) => 0xDB,
                        (true, false) => 0xDF,
                        (false, true) => 0xDC,
                        (false, false) => continue,
                    };
                    g.char(x + bit, row + cell, ch, fg);
                }
            }
        }
        x += width + 1;
    }
    x.saturating_sub(1).max(col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_text_draws_half_blocks() {
        let mut g = Grid::new(20, 5);
        let end = big_text(&mut g, 1, 0, "1%", BigFont::Large, GOOD);
        assert_eq!((end, BigFont::Large.width("1%"), BigFont::Large.height()), (12, 11, 4));
        let at = |g: &Grid, col: usize, row: usize| g.cells[row * g.cols + col].ch;
        // The 1's stem: its top pixel alone, then both halves, and its
        // foot's last pixel row in the fourth cell's upper half.
        assert_eq!((at(&g, 3, 0), at(&g, 3, 1), at(&g, 3, 3)), (0xDB, 0xDB, 0xDF));
        assert_eq!(at(&g, 2, 0), 0xDC, "the flag's lower pixel");
        assert_eq!(at(&g, 1, 0), b' ');
        assert_eq!(g.cells[3].fg, GOOD);
        // Small digits are three cells high, and wider text is clipped.
        assert_eq!(BigFont::Small.height(), 3);
        big_text(&mut g, 15, 3, "888", BigFont::Small, GOOD);
    }

    #[test]
    fn plots_draw_inside_their_cells() {
        let mut frame = Frame::new(640, 400);
        let layout = Layout::for_frame(640, 400);
        plot(&mut frame, &layout, (2, 10, 20, 4), &[0.0, 50.0, 100.0], 100.0, GOOD, OPAQUE);
        let px = |x: usize, y: usize| {
            let i = (y * 640 + x) * 3;
            Rgb(frame.rgb[i], frame.rgb[i + 1], frame.rgb[i + 2])
        };
        let (x0, y0) = (layout.x + 16, layout.y + 10 * layout.cell_h);
        let (w, h) = (160, 4 * layout.cell_h);
        // The newest, full scale, reaches the top at the right edge.
        assert_eq!(px(x0 + w - 1, y0), GOOD);
        // Nothing drawn outside the cells.
        assert_eq!(px(x0 - 1, y0), Rgb(0, 0, 0));
        assert_eq!(px(x0 + w, y0 + h - 1), Rgb(0, 0, 0));
        // The oldest columns are empty: there were only three samples.
        assert_eq!(px(x0, y0 + h - 1), FIELD);

        // Half opaque, over a white picture: the picture shows through
        // but for the values' line.
        let mut frame = Frame::new(640, 400);
        frame.rgb.fill(0xFF);
        plot(&mut frame, &layout, (2, 10, 20, 4), &[100.0], 100.0, GOOD, OPAQUE / 2);
        let px = |x: usize, y: usize| frame.rgb[(y * 640 + x) * 3..][..3].to_vec();
        assert_eq!(px(x0 + w - 1, y0), [GOOD.0, GOOD.1, GOOD.2]);
        assert_eq!(px(x0, y0 + h - 1), blend([0xFF; 3], FIELD, 128));
        assert_eq!(px(x0, y0 + h - 1), [0x83, 0x87, 0x95]);
    }

    #[test]
    fn text_maps_to_code_page_437() {
        assert_eq!(cp437('A'), b'A');
        assert_eq!(cp437('═'), 0xCD);
        assert_eq!(cp437('►'), 0x10);
        assert_eq!(cp437('€'), b'?');
    }

    #[test]
    fn the_panel_fits_every_picture() {
        let text = Layout::for_frame(640, 400);
        assert_eq!((text.cols, text.rows, text.cell_h), (76, 23, 16));
        assert_eq!((text.x, text.y), (16, 16));
        let ega = Layout::for_frame(640, 350);
        assert_eq!((ega.rows, ega.cell_h), (19, 16));
        // Small tweaked modes use the 8x8 font.
        let small = Layout::for_frame(400, 300);
        assert_eq!((small.cols, small.rows, small.cell_h), (46, MAX_ROWS, 8));
        let big = Layout::for_frame(1024, 768);
        assert_eq!((big.cols, big.rows), (MAX_COLS, MAX_ROWS));

        assert_eq!(text.cell_at(16, 16), Some((0, 0)));
        assert_eq!(text.cell_at(16 + 8 * 75 + 7, 16 + 16 * 22 + 15), Some((75, 22)));
        assert_eq!(text.cell_at(15, 100), None);
        assert_eq!(text.cell_at(-40, -40), None);
    }

    #[test]
    fn rendering_blends_the_panel_and_draws_glyphs() {
        let mut frame = Frame::new(640, 400);
        frame.rgb.fill(0xFF);
        let layout = Layout::for_frame(640, 400);
        let mut grid = Grid::new(layout.cols, layout.rows);
        grid.text(0, 0, "\u{2588}", BRIGHT); // a solid block
        render(&grid, &layout, &mut frame);
        let px = |x: usize, y: usize| &frame.rgb[(y * 640 + x) * 3..][..3];
        // Outside the panel: untouched.
        assert_eq!(px(0, 0), [0xFF, 0xFF, 0xFF]);
        // The block glyph: its colour.
        assert_eq!(px(layout.x, layout.y + 8), [0xFF, 0xFF, 0xFF]);
        // Panel: the picture shows through, darkened towards the panel colour.
        let under = px(layout.x + 8 * 5, layout.y + 8);
        assert!(under[2] > PANEL.2 && under[2] < 0xFF, "{:?}", under);
    }
}
