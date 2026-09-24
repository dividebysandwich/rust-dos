//! Drawing the settings window: a grid of CP437 character cells, rendered
//! with the VGA font onto the picture over a semi-transparent panel.

use crate::debug::keys::CP437;
use crate::video::{self, Frame};

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
const PANEL_ALPHA: u32 = 216;

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
    let font = if layout.cell_h == 16 { video::font_8x16() } else { video::font_8x8() };
    let width = frame.width as usize;
    let blend = |d: u8, c: u8| ((d as u32 * (256 - PANEL_ALPHA) + c as u32 * PANEL_ALPHA) >> 8) as u8;
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
                        [blend(px[0], PANEL.0), blend(px[1], PANEL.1), blend(px[2], PANEL.2)]
                    };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
