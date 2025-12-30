use crate::bus::Bus;
use crate::cpu::Cpu;

pub mod vga;

pub const SCREEN_WIDTH: u32 = 640;
pub const SCREEN_HEIGHT: u32 = 400;

// Memory Map Addresses
pub const ADDR_VGA_GRAPHICS: usize = 0xA0000;
pub const ADDR_VGA_TEXT: usize = 0xB8000;
pub const SIZE_GRAPHICS: usize = 64000; // 320 * 200
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
    Graphics320x200 = 0x13,
}

pub fn render_screen(canvas: &mut [u8], bus: &Bus) {
    match bus.video_mode {
        VideoMode::Graphics320x200 => render_graphics_mode(canvas, &bus.vga.vram_graphics, bus),
        VideoMode::Cga320x200Color | VideoMode::Cga320x200 => {
            render_cga_mode4(canvas, &bus.vga.vram_text, &bus)
        }
        VideoMode::Cga640x200 => render_cga_mode6(canvas, &bus.vga.vram_text),
        VideoMode::Text80x25 => render_text_mode_80x25(canvas, &bus.vga.vram_text, bus),
        VideoMode::Text80x25Color => render_text_mode_80x25(canvas, &bus.vga.vram_text, bus),
        VideoMode::Text40x25 => render_text_mode_40x25(canvas, &bus.vga.vram_text, bus),
        VideoMode::Text40x25Color => render_text_mode_40x25(canvas, &bus.vga.vram_text, bus),
    }
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
            let rgb = bus.vga.get_rgb(color_idx);

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
    for row in 0..25 {
        for col in 0..80 {
            let offset = (row * 80 + col) * 2;
            let char_code = vram[offset] as usize; // Direct index into CP437
            let attr = vram[offset + 1];

            let fg = bus.vga.get_rgb(attr & 0x0F);
            let bg = bus.vga.get_rgb((attr >> 4) & 0x0F);

            // Calculate start index in the font array
            // Each character is 16 bytes long in the 8x16 font
            let glyph_start = char_code * 16;

            // Draw 8x16 Block
            for y in 0..16 {
                // Get the byte for this row of the character
                let glyph_row = FONT_8X16[glyph_start + y];

                for x in 0..8 {
                    // Check bit (most significant bit is left-most pixel)
                    let on = (glyph_row >> (7 - x)) & 1 == 1;
                    let color = if on { fg } else { bg };

                    let screen_x = (col * 8) + x;
                    let screen_y = (row * 16) + y;

                    let idx = (screen_y * SCREEN_WIDTH as usize + screen_x) * 3;

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
            }
        }
        _ => {
            // Print standard character
            let offset = (bus.cursor_y * 80 + bus.cursor_x) * 2;
            bus.vga.vram_text[offset] = ascii;
            bus.vga.vram_text[offset + 1] = 0x07; // Light Gray Attribute
            bus.cursor_x += 1;
        }
    }

    // Handle Line Wrap
    if bus.cursor_x >= 80 {
        bus.cursor_x = 0;
        bus.cursor_y += 1;
    }

    // Handle Scrolling
    if bus.cursor_y >= 25 {
        bus.scroll_up();
        bus.cursor_y = 24;
    }
}

pub fn print_string(cpu: &mut Cpu, s: &str) {
    let mut col = cpu.bus.cursor_x;
    let mut row = cpu.bus.cursor_y;
    let max_cols = 80;
    let max_rows = 25;

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
}
