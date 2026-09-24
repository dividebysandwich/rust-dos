//! Text modes on the screen: how many characters a row has and how many
//! rows, how big a character cell is and with which font, where in text
//! memory the screen starts, and drawing the characters in their colours.

use super::adapter::Adapter;
use super::{FONT_8X8, FONT_8X14, FONT_8X16, VideoMode};
use crate::bus::Bus;

/// The shape of the text screen of the current mode.
#[derive(Clone, Copy, Debug)]
pub struct TextGeometry {
    pub cols: usize,
    pub rows: usize,
    /// The glyphs, `stride` bytes each, the leftmost pixel in bit 7, and
    /// the scanlines of a character row, `font_h`: fewer than a glyph has
    /// shows its top, more add blank ones.
    pub font: &'static [u8],
    pub stride: usize,
    pub font_h: usize,
    /// How many screen pixels a font pixel takes across and down: 2x2 in
    /// the 40-column modes.
    pub x_scale: usize,
    pub y_scale: usize,
    /// Bytes of text memory per row: a character and its attribute each.
    pub row_bytes: usize,
    /// Where the screen starts in text memory (the CRTC's Start Address, in
    /// bytes) and where text memory wraps (a mask).
    pub start: usize,
    pub wrap: usize,
}

impl TextGeometry {
    /// A character cell's width on the screen.
    pub fn cell_w(&self) -> usize {
        8 * self.x_scale
    }

    /// A character cell's height on the screen.
    pub fn cell_h(&self) -> usize {
        self.font_h * self.y_scale
    }

    /// The screen rows (first, past the last) that show the text memory
    /// bytes `start..end`, or None if none of them is on the screen.
    pub fn screen_rows(&self, start: usize, end: usize) -> Option<(u32, u32)> {
        if end <= start {
            return None;
        }
        let screen = self.rows * self.row_bytes;
        let first = start.wrapping_sub(self.start) & self.wrap;
        let last = (end - 1).wrapping_sub(self.start) & self.wrap;
        let cell_h = self.cell_h() as u32;
        if first > last {
            // Across the start of the screen: be generous.
            return Some((0, (self.rows as u32) * cell_h));
        }
        if first >= screen {
            return None;
        }
        let last = last.min(screen - 1);
        Some(((first / self.row_bytes) as u32 * cell_h, (last / self.row_bytes + 1) as u32 * cell_h))
    }
}

/// The text screen of the current mode, or None in a graphics mode.
pub fn geometry(bus: &Bus) -> Option<TextGeometry> {
    if bus.vga.adapter == Adapter::Cga {
        return cga_geometry(bus);
    }
    let (_, _, wrap) = bus.vga.text_window();
    // The CRTC counts the Start Address in characters: two bytes each.
    let start = (bus.vga.latched_start_addr * 2) & wrap;
    let rows = bus.text_rows();
    match bus.video_mode {
        VideoMode::Text80x25 | VideoMode::Text80x25Color => {
            let (font, font_h) = font_of_height(bus);
            Some(TextGeometry {
                cols: 80,
                rows,
                font,
                stride: font_h,
                font_h,
                x_scale: 1,
                y_scale: 1,
                row_bytes: 160,
                start,
                wrap,
            })
        }
        // The font of the 80-column modes, drawn twice as wide.
        VideoMode::Text40x25 | VideoMode::Text40x25Color => {
            let (font, font_h) = font_of_height(bus);
            Some(TextGeometry {
                cols: 40,
                rows,
                font,
                stride: font_h,
                font_h,
                x_scale: 2,
                y_scale: 1,
                row_bytes: 80,
                start,
                wrap,
            })
        }
        _ => None,
    }
}

/// The ROM font of the character height in BDA 0485h: programs like Norton
/// Commander switch to 80x50 by loading the 8x8 font (INT 10h AH=11h
/// AL=12h), and the EGA's text is in its 8x14.
fn font_of_height(bus: &Bus) -> (&'static [u8], usize) {
    match bus.read_16(0x0485) {
        1..=10 => (FONT_8X8, 8),
        11..=14 => (FONT_8X14, 14),
        _ => (FONT_8X16, 16),
    }
}

/// The CGA's text screen, as its 6845 has it: R1 characters a row, R6
/// rows of R9 + 1 scanlines, each drawn twice for the 400 lines of the
/// picture. Programs make other shapes of it, such as 80x100 rows of two
/// scanlines for 16 colours at 160x100.
fn cga_geometry(bus: &Bus) -> Option<TextGeometry> {
    if !matches!(
        bus.video_mode,
        VideoMode::Text80x25 | VideoMode::Text80x25Color | VideoMode::Text40x25 | VideoMode::Text40x25Color
    ) {
        return None;
    }
    let regs = &bus.vga.crtc_regs;
    let cols = (regs[1] as usize).clamp(1, 80);
    let rows = ((regs[6] & 0x7F) as usize).clamp(1, 100);
    let (_, _, wrap) = bus.vga.text_window();
    Some(TextGeometry {
        cols,
        rows,
        font: FONT_8X8,
        stride: 8,
        font_h: (regs[9] & 0x1F) as usize + 1,
        x_scale: if cols > 40 { 1 } else { 2 },
        y_scale: 2,
        row_bytes: cols * 2,
        start: (bus.vga.latched_start_addr * 2) & wrap,
        wrap,
    })
}

/// Draw the text rows that fall in the screen rows `y_min..y_max` of
/// `canvas`, `canvas_w` pixels wide. The colours go through the attribute
/// controller's palette registers and the DAC, as a VGA's do. With blinking
/// on, attribute bit 7 blinks the character and the background has only
/// the eight dark colours.
pub fn render(canvas: &mut [u8], canvas_w: usize, bus: &Bus, g: &TextGeometry, y_min: usize, y_max: usize) {
    let vga = &bus.vga;
    let vram = &vga.vram_text;
    let blinks = vga.blinks();
    let blink_on = vga.blink_on();
    let colors: [(u8, u8, u8); 16] = match vga.adapter {
        // Mode Control bit 3 turns the CGA's picture off.
        Adapter::Cga if !vga.cga_video_enabled() => return,
        Adapter::Cga => vga.cga_text_colors(),
        _ => std::array::from_fn(|attr| vga.attribute_rgb(attr as u8)),
    };
    let (cell_w, cell_h) = (g.cell_w(), g.cell_h());
    let canvas_h = canvas.len() / (canvas_w * 3);
    let row_lo = (y_min / cell_h).min(g.rows);
    let row_hi = y_max.div_ceil(cell_h).min(g.rows);

    for row in row_lo..row_hi {
        for col in 0..g.cols {
            let offset = (g.start + row * g.row_bytes + col * 2) & g.wrap;
            let char_code = vram[offset] as usize;
            let attr = vram[offset + 1];
            let (fg, bg, hidden) = if blinks {
                (attr & 0x0F, (attr >> 4) & 0x07, attr & 0x80 != 0 && !blink_on)
            } else {
                (attr & 0x0F, attr >> 4, false)
            };
            let (fg, bg) = (colors[fg as usize], colors[bg as usize]);

            for y in 0..g.font_h {
                let bits = if y < g.stride { g.font[char_code * g.stride + y] } else { 0 };
                for x in 0..8 {
                    let on = !hidden && (bits >> (7 - x)) & 1 == 1;
                    let (r, gr, b) = if on { fg } else { bg };
                    for dy in 0..g.y_scale {
                        let py = row * cell_h + y * g.y_scale + dy;
                        if py >= canvas_h {
                            continue;
                        }
                        for dx in 0..g.x_scale {
                            let px = col * cell_w + x * g.x_scale + dx;
                            if px >= canvas_w {
                                continue;
                            }
                            let i = (py * canvas_w + px) * 3;
                            canvas[i] = r;
                            canvas[i + 1] = gr;
                            canvas[i + 2] = b;
                        }
                    }
                }
            }
        }
    }
}
