use crate::bus::Bus;
use crate::cpu::Cpu;

pub mod vga;

pub const SCREEN_WIDTH: u32 = 640;
pub const SCREEN_HEIGHT: u32 = 400;

// Memory Map Addresses
pub const ADDR_VGA_GRAPHICS: usize = 0xA0000;
pub const ADDR_VGA_TEXT: usize = 0xB8000;
pub const SIZE_GRAPHICS: usize = 0x10000; // 64KB A0000..AFFFF window (covers modes 13h, 12h, etc.)
pub const SIZE_TEXT: usize = 32 * 1024; // 32kB to cover CGA modes too
pub const BDA_CURSOR_POS: usize = 0x0450; // Base for Page 0. Page n = 0x450 + n*2
pub const BDA_CURSOR_MODE: usize = 0x0460;
pub const MAX_COLS: u8 = 80;
pub const MAX_ROWS: u8 = 25;

static FONT_8X16: &[u8] = include_bytes!("assets/IBM_VGA_8x16.bin");
static FONT_8X8: &[u8] = include_bytes!("assets/IBM_VGA_8x8.bin");

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum VideoMode {
    Text40x25 = 0x00,
    Text40x25Color = 0x01,
    Text80x25 = 0x02,
    Text80x25Color = 0x03,
    Cga320x200Color = 0x04,
    #[allow(dead_code)]
    Cga320x200 = 0x05, // I can't be bothered and just treat it as Color too
    Cga640x200 = 0x06,
    Ega320x200 = 0x0D,  // EGA planar, 16 colors
    Ega640x200 = 0x0E,  // EGA planar, 16 colors
    Ega640x350 = 0x10,  // EGA planar, 16 colors
    Vga640x480 = 0x12,  // VGA planar, 16 colors
    Graphics320x200 = 0x13,
}

impl VideoMode {
    pub fn is_planar(self) -> bool {
        matches!(
            self,
            VideoMode::Ega320x200
                | VideoMode::Ega640x200
                | VideoMode::Ega640x350
                | VideoMode::Vga640x480
        )
    }

    /// Dimensions for each mode in pixels (width, height).
    pub fn dimensions(self) -> (usize, usize) {
        match self {
            VideoMode::Text40x25 | VideoMode::Text40x25Color => (320, 200),
            VideoMode::Text80x25 | VideoMode::Text80x25Color => (640, 400),
            VideoMode::Cga320x200Color | VideoMode::Cga320x200 => (320, 200),
            VideoMode::Cga640x200 => (640, 200),
            VideoMode::Ega320x200 => (320, 200),
            VideoMode::Ega640x200 => (640, 200),
            VideoMode::Ega640x350 => (640, 350),
            VideoMode::Vga640x480 => (640, 480),
            VideoMode::Graphics320x200 => (320, 200),
        }
    }
}

pub fn render_screen(canvas: &mut [u8], bus: &Bus) {
    // Modes shorter than the canvas leave a black bar below the rendered
    // image; clear everything first so the bar is black rather than showing
    // the previous frame.
    for b in canvas.iter_mut() {
        *b = 0;
    }
    match bus.video_mode {
        VideoMode::Graphics320x200 => render_graphics_mode(canvas, &bus.vga.vram_graphics, bus),
        VideoMode::Cga320x200Color | VideoMode::Cga320x200 => {
            render_cga_mode4(canvas, &bus.vga.vram_text, bus)
        }
        VideoMode::Cga640x200 => render_cga_mode6(canvas, &bus.vga.vram_text),
        VideoMode::Text80x25 | VideoMode::Text80x25Color => {
            render_text_mode_80x25(canvas, &bus.vga.vram_text, bus)
        }
        VideoMode::Text40x25 | VideoMode::Text40x25Color => {
            render_text_mode_40x25(canvas, &bus.vga.vram_text, bus)
        }
        // Mode 0Dh (320x200): 2x horizontal, 2x vertical -> 640x400 exactly.
        VideoMode::Ega320x200 => render_planar(canvas, &bus.vga.vram_graphics, bus, 320, 200, 2, 2),
        // Mode 0Eh (640x200): 2x vertical -> 640x400 exactly.
        VideoMode::Ega640x200 => render_planar(canvas, &bus.vga.vram_graphics, bus, 640, 200, 1, 2),
        // Mode 10h (640x350): native. Leaves a 50-line black band at the bottom.
        VideoMode::Ega640x350 => render_planar(canvas, &bus.vga.vram_graphics, bus, 640, 350, 1, 1),
        // Mode 12h (640x480): we only have 400 lines of canvas. Compress via
        // nearest-neighbour sampling to fit, keeping the full 640 width.
        VideoMode::Vga640x480 => render_planar_fit(canvas, &bus.vga.vram_graphics, bus, 640, 480),
    }
}

/// Generic renderer for planar 16-color VGA/EGA modes (0Dh, 0Eh, 10h, 12h).
///
/// Reads pixels out of the 4-plane memory layout and runs each 4-bit pixel
/// through the Attribute Controller palette, then the DAC. scale_x/scale_y
/// let us upscale lower-res modes so they fill the standard 640x400 viewport.
fn render_planar(
    canvas: &mut [u8],
    vram: &[u8],
    bus: &Bus,
    width: usize,
    height: usize,
    scale_x: usize,
    scale_y: usize,
) {
    let high_bits = attribute_color_high_bits(bus);
    let bytes_per_row = width / 8;

    for y in 0..height {
        for x in 0..width {
            let rgb = planar_pixel_rgb(bus, vram, bytes_per_row, x, y, high_bits);

            for dy in 0..scale_y {
                for dx in 0..scale_x {
                    let tx = x * scale_x + dx;
                    let ty = y * scale_y + dy;
                    if tx < SCREEN_WIDTH as usize && ty < SCREEN_HEIGHT as usize {
                        let idx = (ty * SCREEN_WIDTH as usize + tx) * 3;
                        canvas[idx] = rgb.0;
                        canvas[idx + 1] = rgb.1;
                        canvas[idx + 2] = rgb.2;
                    }
                }
            }
        }
    }
}

/// Planar renderer for modes taller than SCREEN_HEIGHT (currently only
/// mode 12h, which is 640x480 vs our 640x400 viewport). Uses nearest-neighbour
/// sampling on the Y axis so all 480 source lines are represented.
fn render_planar_fit(canvas: &mut [u8], vram: &[u8], bus: &Bus, width: usize, height: usize) {
    let high_bits = attribute_color_high_bits(bus);
    let bytes_per_row = width / 8;

    for ty in 0..SCREEN_HEIGHT as usize {
        let sy = (ty as u64 * height as u64 / SCREEN_HEIGHT as u64) as usize;
        for tx in 0..SCREEN_WIDTH as usize {
            let sx = (tx as u64 * width as u64 / SCREEN_WIDTH as u64) as usize;
            let rgb = planar_pixel_rgb(bus, vram, bytes_per_row, sx, sy, high_bits);
            let idx = (ty * SCREEN_WIDTH as usize + tx) * 3;
            canvas[idx] = rgb.0;
            canvas[idx + 1] = rgb.1;
            canvas[idx + 2] = rgb.2;
        }
    }
}

/// Derive the upper 2 bits of the 8-bit DAC index from the attribute
/// controller's Mode Control (P54S) and Color Select registers.
fn attribute_color_high_bits(bus: &Bus) -> u8 {
    let mode_ctrl = bus.vga.attribute_regs[0x10];
    let color_select = bus.vga.attribute_regs[0x14];
    if (mode_ctrl & 0x80) != 0 {
        (color_select & 0x03) << 4
    } else {
        (color_select & 0x0C) << 4
    }
}

fn planar_pixel_rgb(
    bus: &Bus,
    vram: &[u8],
    bytes_per_row: usize,
    x: usize,
    y: usize,
    high_bits: u8,
) -> (u8, u8, u8) {
    let byte_offset = y * bytes_per_row + (x / 8);
    let bit_pos = 7 - (x % 8) as u8;
    let mut pixel_idx: u8 = 0;
    for plane in 0..4 {
        let idx = plane * 65536 + byte_offset;
        if idx < vram.len() {
            let bit = (vram[idx] >> bit_pos) & 1;
            pixel_idx |= bit << plane;
        }
    }
    let attr = bus.vga.attribute_regs[pixel_idx as usize & 0x0F] & 0x3F;
    let dac_idx = (attr & 0x0F) | high_bits;
    bus.vga.get_rgb(dac_idx & bus.vga.dac_mask)
}

// Emulate Mode 13h (320x200) -> Scaled to 640x400
pub fn render_graphics_mode(canvas: &mut [u8], vram: &[u8], bus: &Bus) {
    for y in 0..200 {
        for x in 0..320 {
            let linear_addr = y * 320 + x;
            // In Planar Mode 13h (Chain 4), pixels are interleaved across planes.
            // Plane = Addr % 4
            // Offset = Addr / 4
            let plane = linear_addr & 3;
            let offset = linear_addr >> 2;
            let final_index = (plane * 65536) + offset;

            let color_idx = if final_index < vram.len() {
                vram[final_index]
            } else {
                0
            };
            // VGA hardware ANDs pixel through the PEL mask register before DAC lookup.
            let rgb = bus.vga.get_rgb(color_idx & bus.vga.dac_mask);

            // Scale 2x horizontally and 2x vertically
            for dy in 0..2 {
                for dx in 0..2 {
                    let target_x = x * 2 + dx;
                    let target_y = y * 2 + dy;
                    let idx = (target_y * SCREEN_WIDTH as usize + target_x) * 3;

                    canvas[idx] = rgb.0;
                    canvas[idx + 1] = rgb.1;
                    canvas[idx + 2] = rgb.2;
                }
            }
        }
    }
}

// CGA Mode 4/5 (320x200 4 color)
// Memory is interleaved: Even rows at 0x0000, Odd rows at 0x2000
fn render_cga_mode4(canvas: &mut [u8], vram: &[u8], bus: &Bus) {
    // Read Palette from BDA (0x0466)
    // Bit 5 = Palette ID (0=Red/Green/Brown, 1=Cyan/Magenta/White)
    // Bit 0-3 = Background Color (Index in VGA Palette)
    let cga_reg = bus.read_8(0x0466);
    let bg_color_idx = cga_reg & 0x0F;
    let palette_id = (cga_reg & 0x20) != 0;
    // Get RGB values using the bus
    let bg_rgb_val = bus.vga.get_rgb(bg_color_idx);

    // Hardcoded Indices
    let p0 = [
        bg_rgb_val,
        bus.vga.get_rgb(2),
        bus.vga.get_rgb(4),
        bus.vga.get_rgb(6),
    ];
    let p1 = [
        bg_rgb_val,
        bus.vga.get_rgb(3),
        bus.vga.get_rgb(5),
        bus.vga.get_rgb(7),
    ];

    let current_pal = if palette_id { p1 } else { p0 };

    for y in 0..200 {
        // Determine memory offset based on interleave
        let bank_offset = if y % 2 == 0 { 0 } else { 0x2000 };
        let line_offset = bank_offset + ((y / 2) * 80);

        for byte_idx in 0..80 {
            let offset = line_offset + byte_idx;
            if offset >= vram.len() {
                continue;
            }

            let byte = vram[offset];

            // 4 pixels per byte (2 bits each)
            for p in 0..4 {
                // High bits are leftmost pixel
                let shift = 6 - (p * 2);
                let color_idx = (byte >> shift) & 0x03;
                let rgb = current_pal[color_idx as usize];

                let x = (byte_idx * 4) + p;

                // Scale 2x2
                for dy in 0..2 {
                    for dx in 0..2 {
                        let target_x = x * 2 + dx;
                        let target_y = y * 2 + dy;
                        let idx = (target_y * SCREEN_WIDTH as usize + target_x) * 3;
                        if idx + 2 < canvas.len() {
                            canvas[idx] = rgb.0;
                            canvas[idx + 1] = rgb.1;
                            canvas[idx + 2] = rgb.2;
                        }
                    }
                }
            }
        }
    }
}

// CGA Mode 6 (640x200 2 color - Black & White)
fn render_cga_mode6(canvas: &mut [u8], vram: &[u8]) {
    let fg = (255, 255, 255);
    let bg = (0, 0, 0);

    for y in 0..200 {
        let bank_offset = if y % 2 == 0 { 0 } else { 0x2000 };
        let line_offset = bank_offset + ((y / 2) * 80);

        for byte_idx in 0..80 {
            let offset = line_offset + byte_idx;
            if offset >= vram.len() {
                continue;
            }
            let byte = vram[offset];

            // 8 pixels per byte (1 bit each)
            for p in 0..8 {
                let shift = 7 - p;
                let on = (byte >> shift) & 0x01 == 1;
                let rgb = if on { fg } else { bg };

                let x = (byte_idx * 8) + p;

                // Scale 1x horizontal, 2x vertical (to get 640x400)
                for dy in 0..2 {
                    let target_y = y * 2 + dy;
                    let idx = (target_y * SCREEN_WIDTH as usize + x) * 3;
                    if idx + 2 < canvas.len() {
                        canvas[idx] = rgb.0;
                        canvas[idx + 1] = rgb.1;
                        canvas[idx + 2] = rgb.2;
                    }
                }
            }
        }
    }
}

// Emulate Text Mode (80x25) using authentic 8x16 Font
// No scaling needed for height (16px * 25 rows = 400px)
pub fn render_text_mode_80x25(canvas: &mut [u8], vram: &[u8], bus: &Bus) {
    // Programs like Norton Commander switch to 80x50 by loading the 8x8 font
    // (INT 10h AH=11h AL=12h). The row count and character cell height live in
    // BDA 0x0484 / 0x0485; honour them so all rows the program wrote are drawn.
    let rows = bus.read_8(0x0484) as usize + 1;
    let char_height = bus.read_16(0x0485) as usize;
    let (font, font_height): (&[u8], usize) = match char_height {
        0..=10 => (FONT_8X8, 8),
        _ => (FONT_8X16, 16),
    };

    for row in 0..rows {
        for col in 0..80 {
            let offset = (row * 80 + col) * 2;
            if offset + 1 >= vram.len() {
                continue;
            }
            let char_code = vram[offset] as usize;
            let attr = vram[offset + 1];

            let fg = bus.vga.get_rgb(attr & 0x0F);
            let bg = bus.vga.get_rgb((attr >> 4) & 0x0F);

            let glyph_start = char_code * font_height;
            if glyph_start + font_height > font.len() {
                continue;
            }

            for y in 0..font_height {
                let glyph_row = font[glyph_start + y];
                for x in 0..8 {
                    let on = (glyph_row >> (7 - x)) & 1 == 1;
                    let color = if on { fg } else { bg };

                    let screen_x = (col * 8) + x;
                    let screen_y = (row * font_height) + y;
                    if screen_y >= SCREEN_HEIGHT as usize {
                        continue;
                    }
                    let idx = (screen_y * SCREEN_WIDTH as usize + screen_x) * 3;
                    if idx + 2 >= canvas.len() {
                        continue;
                    }
                    canvas[idx] = color.0;
                    canvas[idx + 1] = color.1;
                    canvas[idx + 2] = color.2;
                }
            }
        }
    }
}

// Emulate Text Mode (40x25) using authentic 8x8 Font
// Scaled 2x width, 2x height
fn render_text_mode_40x25(canvas: &mut [u8], vram: &[u8], bus: &Bus) {
    for row in 0..25 {
        for col in 0..40 {
            let offset = (row * 40 + col) * 2;
            if offset + 1 >= vram.len() {
                continue;
            }

            let char_code = vram[offset] as usize;
            let attr = vram[offset + 1];

            let fg = bus.vga.get_rgb(attr & 0x0F);
            let bg = bus.vga.get_rgb((attr >> 4) & 0x0F);

            // Each character is 8 bytes long in the 8x8 font
            let glyph_start = char_code * 8;

            for y in 0..8 {
                let glyph_row = FONT_8X8[glyph_start + y];

                for x in 0..8 {
                    let on = (glyph_row >> (7 - x)) & 1 == 1;
                    let color = if on { fg } else { bg };

                    // Calculate Base Position (40 cols * 16px wide)
                    let start_x = (col * 16) + (x * 2);
                    let start_y = (row * 16) + (y * 2);

                    // Draw 2x2 pixel block for every 1 font pixel
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let idx = ((start_y + dy) * SCREEN_WIDTH as usize + (start_x + dx)) * 3;
                            if idx + 2 < canvas.len() {
                                canvas[idx] = color.0;
                                canvas[idx + 1] = color.1;
                                canvas[idx + 2] = color.2;
                            }
                        }
                    }
                }
            }
        }
    }
}

// Prints a character and advances cursor, handling scrolling
pub fn print_char(bus: &mut Bus, ascii: u8) {
    match ascii {
        0x0D => {
            // Carriage Return (\r)
            bus.cursor_x = 0;
        }
        0x0A => {
            // Line Feed (\n)
            bus.cursor_y += 1;
        }
        0x08 => {
            // Backspace
            if bus.cursor_x > 0 {
                bus.cursor_x -= 1;
                // Visually clear the character
                let offset = (bus.cursor_y * 80 + bus.cursor_x) * 2;
                bus.vga.vram_text[offset] = 0x20; // Space
                bus.vga.dirty = true;
            }
        }
        _ => {
            // Print standard character
            let offset = (bus.cursor_y * 80 + bus.cursor_x) * 2;
            bus.vga.vram_text[offset] = ascii;
            bus.vga.vram_text[offset + 1] = 0x07; // Light Gray Attribute
            bus.cursor_x += 1;
            bus.vga.dirty = true;
        }
    }

    // Handle Line Wrap
    if bus.cursor_x >= 80 {
        bus.cursor_x = 0;
        bus.cursor_y += 1;
    }

    // Handle Scrolling, using the row count from BDA so 80x43 / 80x50 modes
    // get proper scroll behaviour rather than being clamped to 25.
    let rows = bus.read_8(0x0484) as usize + 1;
    if bus.cursor_y >= rows {
        bus.scroll_up();
        bus.cursor_y = rows - 1;
    }
}

pub fn print_string(cpu: &mut Cpu, s: &str) {
    let mut col = cpu.bus.cursor_x;
    let mut row = cpu.bus.cursor_y;
    let max_cols = 80;
    let max_rows = cpu.bus.read_8(0x0484) as usize + 1;

    for c in s.chars() {
        match c {
            '\r' => {
                col = 0;
            }
            '\n' => {
                row += 1;
            }
            '\x08' => {
                // Backspace
                if col > 0 {
                    col -= 1;
                    // Visual Erase (Space + Light Gray)
                    let offset = (row * max_cols + col) * 2;
                    if offset < SIZE_TEXT {
                        cpu.bus.vga.vram_text[offset] = 0x20;
                        cpu.bus.vga.vram_text[offset + 1] = 0x07;
                    }
                }
            }
            _ => {
                // Printable Character
                let offset = (row * max_cols + col) * 2;
                if offset < SIZE_TEXT {
                    cpu.bus.vga.vram_text[offset] = c as u8;
                    cpu.bus.vga.vram_text[offset + 1] = 0x07; // Attribute: Light Gray
                }
                col += 1;
            }
        }

        // Handle Wrapping
        if col >= max_cols {
            col = 0;
            row += 1;
        }

        // Handle Scrolling
        if row >= max_rows {
            // Scroll Up Logic (Direct Memory Move)
            let row_size = max_cols * 2;
            let screen_size = max_rows * row_size;

            // Shift everything up by one row
            // We can't use `copy_within` easily on Vec<u8> across overlapping ranges in simple rust
            // without unsafe or a temp buffer, but a simple loop works fine for 4KB.
            for i in 0..(screen_size - row_size) {
                cpu.bus.vga.vram_text[i] = cpu.bus.vga.vram_text[i + row_size];
            }

            // Clear bottom row
            for i in (screen_size - row_size)..screen_size {
                if i % 2 == 0 {
                    cpu.bus.vga.vram_text[i] = 0x20; // Space
                } else {
                    cpu.bus.vga.vram_text[i] = 0x07; // Color
                }
            }

            row = max_rows - 1;
        }
    }

    // Update Internal Bus State
    cpu.bus.cursor_x = col;
    cpu.bus.cursor_y = row;

    // Update BIOS Data Area (BDA)
    // The Assembly Shell reads [0x0450] to know where to print the next prompt.
    // If we don't update this, the shell will print over our output.
    cpu.bus.write_8(0x0450, col as u8);
    cpu.bus.write_8(0x0451, row as u8);

    cpu.bus.vga.dirty = true;
}
