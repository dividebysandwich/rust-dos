//! The sheet of paper the virtual printer prints on: a byte a dot at the
//! printer's resolution, packed as in DOSBox-X's printer.cpp. The top
//! three bits are the inks on the dot, cyan, magenta and yellow, which
//! print over each other (black is all three), and the low five bits how
//! much ink, from 0 (none) to 31.

/// The ink bits of a dot: magenta, cyan and yellow.
pub const MAGENTA: u8 = 1 << 5;
pub const CYAN: u8 = 2 << 5;
pub const YELLOW: u8 = 4 << 5;
pub const BLACK: u8 = MAGENTA | CYAN | YELLOW;
/// The most ink a dot takes.
pub const FULL: u8 = 0x1F;

#[derive(Clone)]
pub struct Page {
    pub width: usize,
    pub height: usize,
    /// Dots per inch, across and down.
    pub dpi: u32,
    pub dots: Vec<u8>,
}

impl Page {
    /// A blank sheet `width` by `height` inches.
    pub fn new(width: f64, height: f64, dpi: u32) -> Self {
        let w = (width * dpi as f64).round().max(1.0) as usize;
        let h = (height * dpi as f64).round().max(1.0) as usize;
        Self { width: w, height: h, dpi, dots: vec![0; w * h] }
    }

    /// The sheet's size in inches.
    pub fn inches(&self) -> (f64, f64) {
        (self.width as f64 / self.dpi as f64, self.height as f64 / self.dpi as f64)
    }

    pub fn is_blank(&self) -> bool {
        self.dots.iter().all(|&d| d & FULL == 0)
    }

    /// Put `amount` (0-31) more of the inks in `color` on the dot at
    /// (`x`, `y`), which takes no more than it can.
    #[inline]
    pub fn ink(&mut self, x: i64, y: i64, amount: u8, color: u8) {
        if amount == 0 || x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return;
        }
        let dot = &mut self.dots[y as usize * self.width + x as usize];
        let level = (*dot & FULL) + amount;
        *dot = (*dot & !FULL) | level.min(FULL) | color;
    }

    /// Ink a glyph's coverage (0-255 a pixel, `width` a row) with its top
    /// left corner at (`x`, `y`).
    pub fn blit(&mut self, x: i64, y: i64, width: usize, coverage: &[u8], color: u8) {
        if width == 0 {
            return;
        }
        for (row, line) in coverage.chunks(width).enumerate() {
            for (col, &c) in line.iter().enumerate() {
                if c != 0 {
                    self.ink(x + col as i64, y + row as i64, c >> 3, color);
                }
            }
        }
    }

    /// A line three dots thick from `from` to `to` across at `y`, with
    /// gaps in it if `broken`, for underlines and strikethrough.
    pub fn line(&mut self, from: i64, to: i64, y: i64, broken: bool, color: u8) {
        let period = (self.dpi / 15).max(1) as i64;
        let gap = period * 4 / 5;
        for x in from..=to {
            if broken && x.rem_euclid(period) > gap {
                continue;
            }
            self.ink(x, y - 1, 16, color);
            self.ink(x, y, if broken { 16 } else { FULL }, color);
            self.ink(x, y + 1, 16, color);
        }
    }

    /// Whether the page has only black ink on it.
    pub fn is_gray(&self) -> bool {
        self.dots.iter().all(|&d| d & FULL == 0 || d & BLACK == BLACK)
    }

    /// The page as 8-bit gray, white 255.
    pub fn to_gray(&self) -> Vec<u8> {
        self.dots.iter().map(|&d| 255 - scale(d & FULL)).collect()
    }

    /// The page as RGB bytes.
    pub fn to_rgb(&self) -> Vec<u8> {
        let mut rgb = Vec::with_capacity(self.dots.len() * 3);
        for &d in &self.dots {
            rgb.extend_from_slice(&color_of(d));
        }
        rgb
    }
}

/// An amount of ink (0-31) as 0-255.
#[inline]
fn scale(level: u8) -> u8 {
    (level as u32 * 255 / FULL as u32) as u8
}

/// The colour of a dot: each ink takes its part of the white away (cyan
/// the red, magenta the green, yellow the blue).
pub fn color_of(dot: u8) -> [u8; 3] {
    let level = scale(dot & FULL);
    let take = |ink: u8| if dot & ink != 0 { 255 - level } else { 255 };
    [take(CYAN), take(MAGENTA), take(YELLOW)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_adds_up_to_full() {
        let mut page = Page::new(1.0, 1.0, 10);
        assert!(page.is_blank());
        page.ink(3, 4, 20, BLACK);
        page.ink(3, 4, 20, BLACK);
        assert_eq!(page.dots[4 * 10 + 3], BLACK | FULL);
        assert!(!page.is_blank());
        assert!(page.is_gray());
        assert_eq!(page.to_gray()[4 * 10 + 3], 0);
        // Off the sheet: nothing.
        page.ink(-1, 0, 31, BLACK);
        page.ink(10, 0, 31, BLACK);
    }

    #[test]
    fn inks_mix() {
        assert_eq!(color_of(0), [255, 255, 255]);
        assert_eq!(color_of(BLACK | FULL), [0, 0, 0]);
        assert_eq!(color_of(MAGENTA | YELLOW | FULL), [255, 0, 0]);
        let mut page = Page::new(1.0, 1.0, 10);
        page.ink(0, 0, 31, CYAN);
        assert!(!page.is_gray());
        assert_eq!(&page.to_rgb()[..3], &[0, 255, 255]);
    }
}
