use iced_x86::Register;

use crate::bus::Bus;
use crate::cpu::Cpu;
use crate::interrupts;

pub const SCREEN_WIDTH: u32 = 640;
pub const SCREEN_HEIGHT: u32 = 400;

// Memory Map Addresses
pub const ADDR_VGA_GRAPHICS: usize = 0xA0000;
pub const ADDR_VGA_TEXT: usize = 0xB8000;
pub const SIZE_GRAPHICS: usize = 64000; // 320 * 200
pub const SIZE_TEXT: usize = 4000; // 80 * 25 * 2 bytes

static FONT_8X16: &[u8] = include_bytes!("assets/IBM_VGA_8x16.bin");
static FONT_8X8: &[u8] = include_bytes!("assets/IBM_VGA_8x8.bin");

#[derive(PartialEq, Clone, Copy)]
pub enum VideoMode {
    Text40x25 = 0x00,
    Text40x25Color = 0x01,
    Text80x25 = 0x02,
    Text80x25Color = 0x03,
    Graphics320x200 = 0x13,
}

pub fn render_screen(canvas: &mut [u8], bus: &Bus) {
    match bus.video_mode {
        VideoMode::Graphics320x200 => render_graphics_mode(canvas, &bus.vram_graphics),
        VideoMode::Text80x25 => render_text_mode_80x25(canvas, &bus.vram_text),
        VideoMode::Text80x25Color => render_text_mode_80x25(canvas, &bus.vram_text),
        VideoMode::Text40x25 => render_text_mode_40x25(canvas, &bus.vram_text),
        VideoMode::Text40x25Color => render_text_mode_40x25(canvas, &bus.vram_text),
    }
}

// Emulate Mode 13h (320x200) -> Scaled to 640x400
pub fn render_graphics_mode(canvas: &mut [u8], vram: &[u8]) {
    for y in 0..200 {
        for x in 0..320 {
            let color_idx = vram[y * 320 + x];
            let rgb = vga_palette(color_idx);

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

// Emulate Text Mode (80x25) using authentic 8x16 Font
// No scaling needed for height (16px * 25 rows = 400px)
pub fn render_text_mode_80x25(canvas: &mut [u8], vram: &[u8]) {
    for row in 0..25 {
        for col in 0..80 {
            let offset = (row * 80 + col) * 2;
            let char_code = vram[offset] as usize; // Direct index into CP437
            let attr = vram[offset + 1];

            let fg = vga_palette(attr & 0x0F);
            let bg = vga_palette((attr >> 4) & 0x0F);

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
fn render_text_mode_40x25(canvas: &mut [u8], vram: &[u8]) {
    for row in 0..25 {
        for col in 0..40 {
            let offset = (row * 40 + col) * 2;
            if offset + 1 >= vram.len() { continue; }

            let char_code = vram[offset] as usize;
            let attr = vram[offset + 1];

            let fg = vga_palette(attr & 0x0F);
            let bg = vga_palette((attr >> 4) & 0x0F);

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

// Simple Palette Generator
pub fn vga_palette(index: u8) -> (u8, u8, u8) {
    match index {
        0x00 => (0, 0, 0),       // Black
        0x01 => (0, 0, 170),     // Blue
        0x02 => (0, 170, 0),     // Green
        0x03 => (0, 170, 170),   // Cyan
        0x04 => (170, 0, 0),     // Red
        0x05 => (170, 0, 170),   // Magenta
        0x06 => (170, 85, 0),    // Brown
        0x07 => (170, 170, 170), // Light Gray
        0x08 => (85, 85, 85),    // Dark Gray
        0x09 => (85, 85, 255),   // Light Blue
        0x0A => (85, 255, 85),   // Light Green
        0x0B => (85, 255, 255),  // Light Cyan
        0x0C => (255, 85, 85),   // Light Red
        0x0D => (255, 85, 255),  // Light Magenta
        0x0E => (255, 255, 85),  // Yellow
        0x0F => (255, 255, 255), // White
        // Procedural fallback for 256-color mode
        _ => {
            let r = (index % 32) * 8;
            let g = (index % 64) * 4;
            let b = (index % 128) * 2;
            (r, g, b)
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
                bus.vram_text[offset] = 0x20; // Space
            }
        }
        _ => {
            // Print standard character
            let offset = (bus.cursor_y * 80 + bus.cursor_x) * 2;
            bus.vram_text[offset] = ascii;
            bus.vram_text[offset + 1] = 0x07; // Light Gray Attribute
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

// Use the BIOS Teletype function to print strings.
// This ensures the cursor position (BDA 0x0450) is updated automatically.
pub fn print_string(cpu: &mut Cpu, msg: &str) {
    // Save registers if calling during debugging
    // let saved_ax = cpu.get_reg16(Register::AX);
    
    for b in msg.bytes() {
        // Setup AH=0E (Teletype) and AL=Char
        cpu.set_reg8(Register::AH, 0x0E);
        cpu.set_reg8(Register::AL, b);
        cpu.set_reg8(Register::BH, 0x00); // Page 0
        cpu.set_reg8(Register::BL, 0x07); // Color (Gray)

        // Invoke the Interrupt Handler directly
        interrupts::handle_interrupt(cpu, 0x10);
    }
    
    // cpu.set_reg16(Register::AX, saved_ax);
}
