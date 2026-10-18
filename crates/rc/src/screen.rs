//! The text screen RC draws: a grid of character and attribute cells,
//! and where the cursor shows.

use crate::cp437;

/// Box-drawing characters, in code page 437.
pub mod line {
    pub const H: u8 = 0xC4;
    pub const V: u8 = 0xB3;
    pub const TL: u8 = 0xDA;
    pub const TR: u8 = 0xBF;
    pub const BL: u8 = 0xC0;
    pub const BR: u8 = 0xD9;
    pub const UP: u8 = 0xC1;
    pub const DH: u8 = 0xCD;
    pub const DV: u8 = 0xBA;
    pub const DTL: u8 = 0xC9;
    pub const DTR: u8 = 0xBB;
    pub const DBL: u8 = 0xC8;
    pub const DBR: u8 = 0xBC;
    /// ╤ and ╧: a single line meeting a double one.
    pub const DDOWN: u8 = 0xD1;
    pub const DUP: u8 = 0xCF;
    /// ╟ and ╢: a single line from a double one, at the sides.
    pub const DLT: u8 = 0xC7;
    pub const DRT: u8 = 0xB6;
}

/// The screen: `cols` by `rows` cells of (character, attribute).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<(u8, u8)>,
    /// Where the cursor shows (column, row), None when it is hidden.
    pub cursor: Option<(usize, usize)>,
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Screen {
        Screen {
            cols,
            rows,
            cells: vec![(b' ', 0x07); cols * rows],
            cursor: None,
        }
    }

    pub fn set(&mut self, row: usize, col: usize, ch: u8, attr: u8) {
        if row < self.rows && col < self.cols {
            self.cells[row * self.cols + col] = (ch, attr);
        }
    }

    pub fn get(&self, row: usize, col: usize) -> (u8, u8) {
        if row < self.rows && col < self.cols {
            self.cells[row * self.cols + col]
        } else {
            (b' ', 0x07)
        }
    }

    /// The attribute of a cell changed, its character kept.
    pub fn paint(&mut self, row: usize, col: usize, attr: u8) {
        let (ch, _) = self.get(row, col);
        self.set(row, col, ch, attr);
    }

    /// Code page 437 text, cut at the screen's edge.
    pub fn text(&mut self, row: usize, col: usize, text: &[u8], attr: u8) {
        for (i, &b) in text.iter().enumerate() {
            self.set(row, col + i, b, attr);
        }
    }

    pub fn str(&mut self, row: usize, col: usize, text: &str, attr: u8) {
        self.text(row, col, &cp437::bytes(text), attr);
    }

    /// Text with its parts between '~' in the `hot` attribute.
    pub fn marked(&mut self, row: usize, col: usize, text: &str, attr: u8, hot: u8) {
        let mut at = col;
        let mut lit = false;
        for c in text.chars() {
            if c == '~' {
                lit = !lit;
                continue;
            }
            self.set(row, at, cp437::byte(c), if lit { hot } else { attr });
            at += 1;
        }
    }

    pub fn fill(&mut self, row: usize, col: usize, w: usize, h: usize, ch: u8, attr: u8) {
        for r in row..row + h {
            for c in col..col + w {
                self.set(r, c, ch, attr);
            }
        }
    }

    /// A box's frame, double or single lines.
    pub fn frame(&mut self, row: usize, col: usize, w: usize, h: usize, attr: u8, double: bool) {
        if w < 2 || h < 2 {
            return;
        }
        let (hl, vl, tl, tr, bl, br) = if double {
            (
                line::DH,
                line::DV,
                line::DTL,
                line::DTR,
                line::DBL,
                line::DBR,
            )
        } else {
            (line::H, line::V, line::TL, line::TR, line::BL, line::BR)
        };
        for c in col + 1..col + w - 1 {
            self.set(row, c, hl, attr);
            self.set(row + h - 1, c, hl, attr);
        }
        for r in row + 1..row + h - 1 {
            self.set(r, col, vl, attr);
            self.set(r, col + w - 1, vl, attr);
        }
        self.set(row, col, tl, attr);
        self.set(row, col + w - 1, tr, attr);
        self.set(row + h - 1, col, bl, attr);
        self.set(row + h - 1, col + w - 1, br, attr);
    }

    /// ╟───╢ across a double frame `w` wide.
    pub fn divider(&mut self, row: usize, col: usize, w: usize, attr: u8) {
        self.set(row, col, line::DLT, attr);
        for c in col + 1..col + w - 1 {
            self.set(row, c, line::H, attr);
        }
        self.set(row, col + w - 1, line::DRT, attr);
    }

    /// A box's shadow, as NC casts it: the cells two columns to the right
    /// and one row below go grey on black, their characters kept.
    pub fn shadow(&mut self, row: usize, col: usize, w: usize, h: usize) {
        for r in row + 1..row + h + 1 {
            for c in col + w..col + w + 2 {
                self.paint(r, c, 0x07);
            }
        }
        for c in col + 2..col + w {
            self.paint(row + h, c, 0x07);
        }
    }

    /// A title centred in a box's top edge, with a space either side.
    pub fn title(&mut self, row: usize, col: usize, w: usize, title: &str, attr: u8) {
        let text = format!(" {} ", title);
        let len = text.chars().count().min(w);
        self.str(row, col + (w - len) / 2, &text, attr);
    }
}

/// `text` cut or padded with spaces to `w` cells.
pub fn pad(text: &[u8], w: usize) -> Vec<u8> {
    let mut out: Vec<u8> = text.iter().copied().take(w).collect();
    out.resize(w, b' ');
    out
}

/// `text` right-aligned in `w` cells.
pub fn right(text: &str, w: usize) -> Vec<u8> {
    let b = cp437::bytes(text);
    if b.len() >= w {
        return b[b.len() - w..].to_vec();
    }
    let mut out = vec![b' '; w - b.len()];
    out.extend(b);
    out
}

/// `text` centred in `w` cells, an odd space going to the right.
pub fn centre(text: &[u8], w: usize) -> Vec<u8> {
    centred(text, w, false)
}

/// `text` centred in `w` cells, an odd space going to the left, as the
/// panels' column headings are.
pub fn centre_left(text: &[u8], w: usize) -> Vec<u8> {
    centred(text, w, true)
}

fn centred(text: &[u8], w: usize, up: bool) -> Vec<u8> {
    if text.len() >= w {
        return text[..w].to_vec();
    }
    let space = w - text.len();
    let left = if up { space.div_ceil(2) } else { space / 2 };
    let mut out = vec![b' '; left];
    out.extend_from_slice(text);
    out.resize(w, b' ');
    out
}
