//! The BIOS's pixels in the standard graphics modes: reading and writing
//! one (INT 10h AH=0Ch and 0Dh), drawing characters from the graphics font
//! (INT 43h) for AH=09h, 0Ah and 0Eh, and scrolling character rows (AH=06h
//! and 07h). They work on video memory directly, as the BIOS programs the
//! card for itself whatever state a program left it in.

use super::VideoMode;
use crate::bus::Bus;

/// How a graphics mode keeps its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    /// CGA modes 4 and 5: two bits a pixel, even rows at B8000h and odd
    /// rows at BA000h.
    Cga4,
    /// CGA mode 6: a bit a pixel, interleaved the same way.
    Cga2,
    /// The EGA and VGA 16-colour modes: a bit a pixel in each of 4 planes.
    Planar,
    /// Mode 13h: a byte a pixel, spread over the planes four at a time.
    Linear,
}

/// The layout and size in pixels of the current mode, if it is a standard
/// graphics mode.
fn layout(bus: &Bus) -> Option<(Layout, usize, usize)> {
    let mode = bus.video_mode;
    let (width, height) = mode.dimensions();
    let layout = match mode {
        VideoMode::Cga320x200Color | VideoMode::Cga320x200 => Layout::Cga4,
        VideoMode::Cga640x200 => Layout::Cga2,
        VideoMode::Ega320x200
        | VideoMode::Ega640x200
        | VideoMode::Ega640x350
        | VideoMode::Ega640x350Mono
        | VideoMode::Vga640x480
        | VideoMode::Vga640x480Mono => Layout::Planar,
        VideoMode::Graphics320x200 => Layout::Linear,
        _ => return None,
    };
    Some((layout, width, height))
}

/// Whether the current mode is one the BIOS draws pixels in.
pub fn graphics_mode(bus: &Bus) -> bool {
    layout(bus).is_some()
}

/// The characters across and the rows of character cells down the screen
/// (cells are 8 pixels wide and BDA 0485h high).
pub fn cells(bus: &Bus) -> (usize, usize) {
    match layout(bus) {
        Some((_, width, height)) => (width / 8, height / char_height(bus)),
        None => (80, 25),
    }
}

fn char_height(bus: &Bus) -> usize {
    match bus.read_16(0x0485) {
        0 => 8,
        h => h as usize,
    }
}

/// Where the byte with pixel (`x`, `y`) of a CGA mode is, `per_byte`
/// pixels a byte.
fn cga_offset(x: usize, y: usize, per_byte: usize) -> usize {
    (y & 1) * 0x2000 + (y >> 1) * 80 + x / per_byte
}

/// The colour of pixel (`x`, `y`), or 0 off the screen.
pub fn get_pixel(bus: &Bus, x: usize, y: usize) -> u8 {
    let Some((layout, width, height)) = layout(bus) else { return 0 };
    if x >= width || y >= height {
        return 0;
    }
    let vga = &bus.vga;
    match layout {
        Layout::Cga4 => vga.vram_text[cga_offset(x, y, 4)] >> (6 - (x % 4) * 2) & 3,
        Layout::Cga2 => vga.vram_text[cga_offset(x, y, 8)] >> (7 - x % 8) & 1,
        Layout::Planar => {
            let offset = planar_offset(bus, x, y, width);
            (0..4).map(|p| (vga.vram_graphics[p * 0x10000 + offset] >> (7 - x % 8) & 1) << p).sum()
        }
        Layout::Linear => {
            let offset = y * width + x;
            vga.vram_graphics[(offset & 3) * 0x10000 + (offset >> 2)]
        }
    }
}

fn planar_offset(bus: &Bus, x: usize, y: usize, width: usize) -> usize {
    // The active page's (AH=05h) offset.
    (bus.read_16(0x044E) as usize + y * width / 8 + x / 8) & 0xFFFF
}

/// Whether `color` with bit 7 set is XORed onto the screen: in all but
/// the 256-colour mode, where bit 7 is part of the colour.
fn xors(bus: &Bus, color: u8) -> bool {
    color & 0x80 != 0 && !matches!(layout(bus), Some((Layout::Linear, ..)))
}

/// Bit 7 where it asks for XOR, for scrolling, which never does.
fn xor_bit(bus: &Bus) -> u8 {
    if xors(bus, 0x80) { 0x80 } else { 0 }
}

/// Set pixel (`x`, `y`) to `color`, or with bit 7 set XOR it with the
/// colour it has, as INT 10h AH=0Ch does.
pub fn put_pixel(bus: &mut Bus, x: usize, y: usize, color: u8) {
    let Some((layout, width, height)) = layout(bus) else { return };
    if x >= width || y >= height {
        return;
    }
    let xor = xors(bus, color);
    let merge = |old: u8, mask: u8, bits: u8| if xor { old ^ (bits & mask) } else { (old & !mask) | (bits & mask) };
    match layout {
        Layout::Cga4 | Layout::Cga2 => {
            let (per_byte, depth) = if layout == Layout::Cga4 { (4, 2) } else { (8, 1) };
            let offset = cga_offset(x, y, per_byte);
            let shift = (per_byte - 1 - x % per_byte) * depth;
            let mask = ((1u8 << depth) - 1) << shift;
            let byte = &mut bus.vga.vram_text[offset];
            *byte = merge(*byte, mask, color << shift);
        }
        Layout::Planar => {
            let offset = planar_offset(bus, x, y, width);
            let mask = 0x80u8 >> (x % 8);
            for plane in 0..4 {
                let bits = if color >> plane & 1 != 0 { 0xFF } else { 0 };
                let byte = &mut bus.vga.vram_graphics[plane * 0x10000 + offset];
                *byte = merge(*byte, mask, bits);
            }
        }
        Layout::Linear => {
            let offset = y * width + x;
            bus.vga.vram_graphics[(offset & 3) * 0x10000 + (offset >> 2)] = color;
        }
    }
    bus.vga.mark_dirty_full();
}

/// The physical address an interrupt vector points to.
fn vector_address(bus: &Bus, vector: usize) -> usize {
    bus.read_16(vector * 4) as usize + ((bus.read_16(vector * 4 + 2) as usize) << 4)
}

/// Row `row` of the glyph of `ch`: from the graphics font INT 43h points
/// to, `char_height` bytes a glyph. A CGA's BIOS has the first 128 glyphs
/// of its 8x8 font at F000:FA6E, and INT 1Fh points to the rest.
fn glyph_row(bus: &Bus, ch: u8, row: usize) -> u8 {
    if !bus.vga.adapter.ega_bios() {
        return match ch {
            0..=0x7F => bus.read_8(super::bios::PC_FONT_8X8 + ch as usize * 8 + row),
            _ => bus.read_8(vector_address(bus, 0x1F) + (ch as usize - 0x80) * 8 + row),
        };
    }
    bus.read_8(vector_address(bus, 0x43) + ch as usize * char_height(bus) + row)
}

/// Draw `ch` in the character cell at (`col`, `row`): the glyph's pixels
/// in `color` and the rest in colour 0, or with bit 7 of `color` set the
/// glyph's pixels XORed and the rest left alone.
pub fn draw_char(bus: &mut Bus, col: usize, row: usize, ch: u8, color: u8) {
    let height = char_height(bus);
    let (x0, y0) = (col * 8, row * height);
    let xor = xors(bus, color);
    for y in 0..height {
        let bits = glyph_row(bus, ch, y);
        for x in 0..8 {
            if bits & (0x80 >> x) != 0 {
                put_pixel(bus, x0 + x, y0 + y, color);
            } else if !xor {
                put_pixel(bus, x0 + x, y0 + y, 0);
            }
        }
    }
}

/// The character in the cell at (`col`, `row`), found by matching its
/// pixels against the font (INT 10h AH=08h in graphics modes), or 0.
pub fn read_char(bus: &Bus, col: usize, row: usize) -> u8 {
    let height = char_height(bus);
    let (x0, y0) = (col * 8, row * height);
    let rows: Vec<u8> = (0..height)
        .map(|y| (0..8).fold(0u8, |bits, x| bits << 1 | (get_pixel(bus, x0 + x, y0 + y) != 0) as u8))
        .collect();
    (0..=255u8).find(|&ch| (0..height).all(|y| glyph_row(bus, ch, y) == rows[y])).unwrap_or(0)
}

/// Scroll the character cells from (`left`, `top`) to (`right`, `bottom`)
/// up (or down) by `lines` rows, filling the rows that come in with
/// `fill`; `lines` 0 clears the window.
#[allow(clippy::too_many_arguments)]
pub fn scroll(bus: &mut Bus, up: bool, lines: usize, fill: u8, top: usize, left: usize, bottom: usize, right: usize) {
    let (cols, rows) = cells(bus);
    let height = char_height(bus);
    if top > bottom || left > right || top >= rows || left >= cols {
        return;
    }
    let (bottom, right) = (bottom.min(rows - 1), right.min(cols - 1));
    let window = bottom - top + 1;
    let lines = if lines == 0 || lines > window { window } else { lines };
    let (x0, x1) = (left * 8, (right + 1) * 8);
    let (y0, y1) = (top * height, (bottom + 1) * height);
    let shift = lines * height;
    if up {
        for y in y0..y1 {
            for x in x0..x1 {
                let color = if y + shift < y1 { get_pixel(bus, x, y + shift) } else { fill };
                put_pixel(bus, x, y, color & !xor_bit(bus));
            }
        }
    } else {
        for y in (y0..y1).rev() {
            for x in x0..x1 {
                let color = if y >= y0 + shift { get_pixel(bus, x, y - shift) } else { fill };
                put_pixel(bus, x, y, color & !xor_bit(bus));
            }
        }
    }
}
