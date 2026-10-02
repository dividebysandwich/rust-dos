//! A text screen EDIT draws on before it goes to video memory: cells of a
//! character and an attribute, and the boxes, shadows and lines of the
//! interface.

pub struct Screen {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<(u8, u8)>,
}

/// Box drawing characters of code page 437.
pub mod line {
    pub const H: u8 = 0xC4;
    pub const V: u8 = 0xB3;
    pub const TL: u8 = 0xDA;
    pub const TR: u8 = 0xBF;
    pub const BL: u8 = 0xC0;
    pub const BR: u8 = 0xD9;
    pub const LT: u8 = 0xC3;
    pub const RT: u8 = 0xB4;
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self { cols, rows, cells: vec![(b' ', 0x07); cols * rows] }
    }

    pub fn set(&mut self, row: usize, col: usize, ch: u8, attr: u8) {
        if row < self.rows && col < self.cols {
            self.cells[row * self.cols + col] = (ch, attr);
        }
    }

    pub fn get(&self, row: usize, col: usize) -> (u8, u8) {
        if row < self.rows && col < self.cols { self.cells[row * self.cols + col] } else { (b' ', 0) }
    }

    /// `text` from (`row`, `col`), cut at the right edge.
    pub fn text(&mut self, row: usize, col: usize, text: &[u8], attr: u8) {
        for (i, &ch) in text.iter().enumerate() {
            self.set(row, col + i, ch, attr);
        }
    }

    pub fn str(&mut self, row: usize, col: usize, text: &str, attr: u8) {
        self.text(row, col, &crate::dosstr::to_bytes(text), attr);
    }

    /// A label with a hotkey: the character after '&' in `hot`'s colour.
    pub fn label(&mut self, row: usize, col: usize, text: &str, attr: u8, hot: u8) {
        let mut at = col;
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '&' {
                if let Some(c) = chars.next() {
                    self.set(row, at, c as u8, hot);
                    at += 1;
                }
                continue;
            }
            self.set(row, at, c as u8, attr);
            at += 1;
        }
    }

    pub fn fill(&mut self, row: usize, col: usize, width: usize, height: usize, ch: u8, attr: u8) {
        for r in row..row + height {
            for c in col..col + width {
                self.set(r, c, ch, attr);
            }
        }
    }

    /// A single line frame, filled with blanks in `attr`.
    pub fn frame(&mut self, row: usize, col: usize, width: usize, height: usize, attr: u8) {
        use line::*;
        self.fill(row, col, width, height, b' ', attr);
        for c in col + 1..col + width - 1 {
            self.set(row, c, H, attr);
            self.set(row + height - 1, c, H, attr);
        }
        for r in row + 1..row + height - 1 {
            self.set(r, col, V, attr);
            self.set(r, col + width - 1, V, attr);
        }
        self.set(row, col, TL, attr);
        self.set(row, col + width - 1, TR, attr);
        self.set(row + height - 1, col, BL, attr);
        self.set(row + height - 1, col + width - 1, BR, attr);
    }

    /// A line across a frame at `row`, joining its sides.
    pub fn divider(&mut self, row: usize, col: usize, width: usize, attr: u8) {
        self.set(row, col, line::LT, attr);
        for c in col + 1..col + width - 1 {
            self.set(row, c, line::H, attr);
        }
        self.set(row, col + width - 1, line::RT, attr);
    }

    /// The shadow of a box: the two columns right of it and the row under
    /// it go dark, their characters showing.
    pub fn shadow(&mut self, row: usize, col: usize, width: usize, height: usize) {
        let dark = |s: &mut Self, r: usize, c: usize| {
            let (ch, _) = s.get(r, c);
            s.set(r, c, ch, 0x08);
        };
        for r in row + 1..=row + height {
            dark(self, r, col + width);
            dark(self, r, col + width + 1);
        }
        for c in col + 2..col + width {
            dark(self, row + height, c);
        }
    }

    /// The cells as video memory holds them.
    pub fn bytes(&self) -> Vec<u8> {
        self.cells.iter().flat_map(|&(ch, attr)| [ch, attr]).collect()
    }
}
