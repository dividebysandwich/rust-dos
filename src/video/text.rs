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
    /// How many screen pixels a font pixel takes across and down: twice as
    /// wide in the 40-column modes, twice as high on the CGA.
    pub x_scale: usize,
    pub y_scale: usize,
    /// Pixels a character cell has across: 8, or 9 in the monochrome mode,
    /// whose line-drawing characters (C0h-DFh) repeat their eighth column
    /// in the ninth and whose `alternate` glyphs replace some of the font's.
    pub dots: usize,
    pub alternate: &'static [u8],
    /// The monochrome mode's attributes (see `mda_colors`) rather than
    /// colours, with the underline on scanline `underline`.
    pub mono: bool,
    pub underline: usize,
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
        self.dots * self.x_scale
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
                dots: 8,
                alternate: &[],
                mono: false,
                underline: 0,
                row_bytes: 160,
                start,
                wrap,
            })
        }
        // The monochrome mode 7: 9-dot cells, the 14-line font on an MDA,
        // Hercules card or EGA and the 16-line one on a VGA, with the
        // alternate glyphs of 9-dot cells.
        VideoMode::Mono80x25 => {
            let (font, font_h) = match bus.vga.adapter {
                Adapter::Hercules => (FONT_8X14, 14),
                _ => font_of_height(bus),
            };
            let (alternate, underline) = match (bus.vga.adapter, font_h) {
                (Adapter::Hercules, _) => (super::FONT_9X14_ALTERNATE, 13),
                (_, 16) => (super::FONT_9X16_ALTERNATE, (bus.vga.crtc_regs[0x14] & 0x1F) as usize),
                _ => (super::FONT_9X14_ALTERNATE, (bus.vga.crtc_regs[0x14] & 0x1F) as usize),
            };
            Some(TextGeometry {
                cols: 80,
                rows,
                font,
                stride: font_h,
                font_h,
                x_scale: 1,
                y_scale: 1,
                dots: 9,
                alternate,
                mono: true,
                underline,
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
                dots: 8,
                alternate: &[],
                mono: false,
                underline: 0,
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
        dots: 8,
        alternate: &[],
        mono: false,
        underline: 0,
        row_bytes: cols * 2,
        start: (bus.vga.latched_start_addr * 2) & wrap,
        wrap,
    })
}

/// The colours of a monochrome attribute, as IBM's Monochrome Display
/// Adapter shows them: foreground and background, and whether it is
/// underlined. Background 7 with foreground 0 is reverse video, foreground
/// 0 otherwise nothing at all, 1 underlined; bit 3 brightens the
/// foreground, and without blinking bit 7 the background.
pub fn mda_colors(attr: u8, blinks: bool) -> ((u8, u8, u8), (u8, u8, u8), bool) {
    use super::hercules::{BLACK, BRIGHT, NORMAL};
    let (fg_bits, bg_bits) = (attr & 0x07, (attr >> 4) & 0x07);
    let fg = if attr & 0x08 != 0 { BRIGHT } else { NORMAL };
    let reverse_bg = if !blinks && attr & 0x80 != 0 { BRIGHT } else { NORMAL };
    match (bg_bits, fg_bits) {
        (7, 0) => (BLACK, reverse_bg, false),
        (_, 0) => (BLACK, BLACK, false),
        (_, 1) => (fg, BLACK, true),
        _ => (fg, BLACK, false),
    }
}

/// The glyph `alternate` has for `ch`, if it has one: a table of character
/// codes, each followed by its glyph of `height` bytes, ending with a 0.
fn alternate_glyph(alternate: &'static [u8], ch: u8, height: usize) -> Option<&'static [u8]> {
    alternate
        .chunks(height + 1)
        .take_while(|entry| entry[0] != 0 && entry.len() == height + 1)
        .find(|entry| entry[0] == ch)
        .map(|entry| &entry[1..])
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
        Adapter::Hercules if !vga.herc_video_enabled() => return,
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
            let char_code = vram[offset];
            let attr = vram[offset + 1];
            let hidden = blinks && attr & 0x80 != 0 && !blink_on;
            let (fg, bg, underline) = if g.mono {
                mda_colors(attr, blinks)
            } else if blinks {
                (colors[(attr & 0x0F) as usize], colors[((attr >> 4) & 0x07) as usize], false)
            } else {
                (colors[(attr & 0x0F) as usize], colors[(attr >> 4) as usize], false)
            };
            let glyph = alternate_glyph(g.alternate, char_code, g.font_h);
            // The line-drawing characters reach into a 9-dot cell's ninth
            // column; the others leave it blank.
            let line_drawing = (0xC0..=0xDF).contains(&char_code);

            for y in 0..g.font_h {
                let bits = match glyph {
                    Some(glyph) => glyph[y],
                    None if y < g.stride => g.font[char_code as usize * g.stride + y],
                    None => 0,
                };
                let underlined = underline && y == g.underline;
                for x in 0..g.dots {
                    let lit = match x {
                        8 => line_drawing && bits & 1 != 0,
                        _ => (bits >> (7 - x)) & 1 == 1,
                    };
                    let on = !hidden && (lit || underlined);
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
