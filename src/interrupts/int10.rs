use crate::audio::play_sdl_beep;
use crate::cpu::Cpu;
use crate::video::{ADDR_VGA_TEXT, BDA_CURSOR_MODE, BDA_CURSOR_POS, MAX_COLS, VideoMode};
use iced_x86::Register;

/// Current number of text rows on the screen, read from BDA 0x0484.
/// Programs that load an 8x8 or 8x14 font via INT 10h AH=11h AL=12h change
/// this value to 43 or 50 rows; hard-coding 25 would make the renderer and
/// scroll logic ignore everything below the first 25 rows.
fn active_rows(cpu: &Cpu) -> u8 {
    cpu.bus.read_8(0x0484).wrapping_add(1).max(1)
}

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    cpu.bus.log_string(&format!(
        "[BIOS] INT 10h Called. AH={:02X}, AL={:02X}, BX={:04X}, CX={:04X}, DX={:04X}",
        ah,
        cpu.get_al(),
        cpu.bx,
        cpu.cx,
        cpu.dx
    ));

    match ah {
        // AH = 00h: Set Video Mode
        0x00 => {
            let mode = cpu.get_al();

            // Clear Screen
            let rows_max = active_rows(cpu).saturating_sub(1);
            match mode {
                // Text Modes: Clear with Spaces and Attribute 0x07
                0x00..=0x03 => {
                    scroll_area(cpu, true, 0, 0x07, 0, 0, rows_max, MAX_COLS - 1);
                }
                // CGA Graphics Modes (4, 5, 6): Zero out 16KB of B8000 Memory
                0x04..=0x06 => {
                    for i in 0..16384 {
                        if i < cpu.bus.vga.vram_text.len() {
                            cpu.bus.vga.vram_text[i] = 0x00;
                        }
                    }
                    cpu.bus.vga.dirty = true;
                }
                // VGA Graphics Mode (13h) or planar EGA/VGA modes: clear the
                // entire 256KB planar VRAM. set_video_mode also zeros it but
                // we do it here so mode setting is consistent with other mode
                // clears above.
                0x0D | 0x0E | 0x10 | 0x12 | 0x13 => {
                    for i in 0..cpu.bus.vga.vram_graphics.len() {
                        cpu.bus.vga.vram_graphics[i] = 0x00;
                    }
                    cpu.bus.vga.dirty = true;
                }
                // Fallback / Stubbed modes
                _ => {
                    // Optional: Clear text ram just in case
                    scroll_area(cpu, true, 0, 0x07, 0, 0, rows_max, MAX_COLS - 1);
                }
            }

            // Reset Cursor
            set_cursor(cpu, 0, 0, 0);

            match mode {
                0x00 => {
                    cpu.bus.log_string("[BIOS] Switch to Text Mode (40x25)");
                    cpu.bus.video_mode = VideoMode::Text40x25;
                }
                0x01 => {
                    cpu.bus
                        .log_string("[BIOS] Switch to Text Mode (40x25Color)");
                    cpu.bus.video_mode = VideoMode::Text40x25Color;
                }
                0x02 => {
                    cpu.bus.log_string("[BIOS] Switch to Text Mode (80x25)");
                    cpu.bus.video_mode = VideoMode::Text80x25;
                }
                0x03 => {
                    cpu.bus
                        .log_string("[BIOS] Switch to Text Mode (80x25 Color)");
                    cpu.bus.video_mode = VideoMode::Text80x25Color;
                }
                0x04 => {
                    cpu.bus
                        .log_string("[BIOS] Switch to CGA Graphics Mode (320x200 Color)");
                    cpu.bus.video_mode = VideoMode::Cga320x200Color;
                }
                0x06 => {
                    cpu.bus
                        .log_string("[BIOS] Switch to CGA Graphics Mode (640x200)");
                    cpu.bus.video_mode = VideoMode::Cga640x200;
                }
                0x0D => {
                    cpu.bus
                        .log_string("[BIOS] Switch to EGA Graphics Mode (320x200 16-color)");
                    cpu.bus.video_mode = VideoMode::Ega320x200;
                    cpu.bus.vga.set_video_mode(VideoMode::Ega320x200);
                }
                0x0E => {
                    cpu.bus
                        .log_string("[BIOS] Switch to EGA Graphics Mode (640x200 16-color)");
                    cpu.bus.video_mode = VideoMode::Ega640x200;
                    cpu.bus.vga.set_video_mode(VideoMode::Ega640x200);
                }
                0x10 => {
                    cpu.bus
                        .log_string("[BIOS] Switch to EGA Graphics Mode (640x350 16-color)");
                    cpu.bus.video_mode = VideoMode::Ega640x350;
                    cpu.bus.vga.set_video_mode(VideoMode::Ega640x350);
                }
                0x12 => {
                    cpu.bus
                        .log_string("[BIOS] Switch to VGA Graphics Mode (640x480 16-color)");
                    cpu.bus.video_mode = VideoMode::Vga640x480;
                    cpu.bus.vga.set_video_mode(VideoMode::Vga640x480);
                }
                0x13 => {
                    cpu.bus
                        .log_string("[BIOS] Switch to Graphics Mode (320x200)");
                    cpu.bus.video_mode = VideoMode::Graphics320x200;
                    cpu.bus.vga.set_video_mode(VideoMode::Graphics320x200);
                }
                _ => cpu
                    .bus
                    .log_string(&format!("[BIOS] Unsupported Video Mode {:02X}", mode)),
            }

            cpu.bus.vga.dirty = true;
            cpu.bus.write_8(0x0449, cpu.bus.video_mode as u8); // Update BDA Current Video Mode
            cpu.bus.write_8(0x0462, 0); // Update BDA Active Page to 0
            let cols: u16 = match mode {
                0x00 | 0x01 | 0x04 | 0x05 => 40,
                0x13 => 40, // Mode 13h uses 40 columns text
                _ => 80,
            };
            cpu.bus.write_16(0x044A, cols);

            // Update BDA 0x0484 (Rows on Screen minus 1) and 0x0485 (char height).
            // Mode set always resets the cell size to the mode's default.
            let (rows, char_height): (u8, u16) = match mode {
                // Text modes: 25 rows, VGA 8x16 font is the default.
                0x00..=0x03 => (24, 16),
                // CGA 40-col graphics counts as 25 rows.
                0x04 | 0x05 => (24, 8),
                // CGA 640x200 2-color.
                0x06 => (24, 8),
                // EGA/VGA planar modes and 13h: treat as 25-row equivalents.
                0x0D | 0x0E => (24, 8),
                0x10 => (24, 14),
                0x12 => (29, 16), // 30 rows at 640x480
                0x13 => (24, 8),
                _ => (24, 16),
            };
            cpu.bus.write_8(0x0484, rows);
            cpu.bus.write_16(0x0485, char_height);
        }

        // AH = 01h: Set Cursor Type
        0x01 => {
            let cx = cpu.cx;
            cpu.bus.write_16(0x0460, cx);
        }

        // AH = 02h: Set Cursor Position
        0x02 => {
            let page = cpu.get_reg8(Register::BH) as usize;
            let row = cpu.get_reg8(Register::DH);
            let col = cpu.get_reg8(Register::DL);

            if page < 8 {
                let cursor_addr = 0x450 + (page * 2);
                cpu.bus.write_8(cursor_addr, col);
                cpu.bus.write_8(cursor_addr + 1, row);
            }
        }

        // AH = 03h: Get Cursor Position
        0x03 => {
            let page = cpu.get_reg8(Register::BH) as usize;
            if page < 8 {
                let cursor_addr = 0x450 + (page * 2);
                let col = cpu.bus.read_8(cursor_addr);
                let row = cpu.bus.read_8(cursor_addr + 1);
                cpu.set_reg8(Register::DL, col);
                cpu.set_reg8(Register::DH, row);
                // Also return Cursor Mode (Start/End Scanlines)
                let cursor_shape = cpu.bus.read_16(BDA_CURSOR_MODE);
                cpu.set_reg16(Register::CX, cursor_shape);
            }
        }

        // AH = 04h: Read Light Pen
        0x04 => {
            cpu.cx = 0;
            cpu.dx = 0;
        }

        // AH = 05h: Set Active Page
        0x05 => {
            let page = cpu.get_reg8(Register::AL);
            cpu.bus.write_8(0x0462, page); // Update BDA Active Page
            cpu.bus
                .log_string(&format!("[BIOS] Set Active Page to {}", page));
        }

        // AH = 06h: Scroll Up
        0x06 => {
            let lines = cpu.get_reg8(Register::AL);
            let attr = cpu.get_reg8(Register::BH);
            let row_start = cpu.get_reg8(Register::CH);
            let col_start = cpu.get_reg8(Register::CL);
            let row_end = cpu.get_reg8(Register::DH);
            let col_end = cpu.get_reg8(Register::DL);

            scroll_area(
                cpu, true, lines, attr, row_start, col_start, row_end, col_end,
            );
        }

        // AH = 07h: Scroll Down
        0x07 => {
            let lines = cpu.get_reg8(Register::AL);
            let attr = cpu.get_reg8(Register::BH);
            let row_start = cpu.get_reg8(Register::CH);
            let col_start = cpu.get_reg8(Register::CL);
            let row_end = cpu.get_reg8(Register::DH);
            let col_end = cpu.get_reg8(Register::DL);

            scroll_area(
                cpu, false, lines, attr, row_start, col_start, row_end, col_end,
            );
        }

        // AH = 08h: Read Character and Attribute at Cursor Position
        // BH = Page Number
        // Returns: AH = Attribute, AL = Character
        0x08 => {
            let page = cpu.get_reg8(Register::BH);
            let (col, row) = get_cursor(cpu, page);
            let (char_code, attr) = read_char_at(cpu, col, row, page);
            cpu.set_reg8(Register::AH, attr);
            cpu.set_reg8(Register::AL, char_code);
        }

        // AH = 09h: Write Character and Attribute at Cursor Position
        // AL = Char, BH = Page, BL = Attribute, CX = Count
        0x09 => {
            let char_code = cpu.get_al();
            let page = cpu.get_reg8(Register::BH);
            let attr = cpu.get_reg8(Register::BL);
            let count = cpu.cx as usize;

            let (col, row) = get_cursor(cpu, page);

            // Repeat char count times (without moving cursor)
            for i in 0..count {
                // Determine VRAM offset
                // Note: DOS wraps to next line visually for this function, but doesn't scroll
                let temp_col = (col as usize + i) % MAX_COLS as usize;
                let temp_row = (row as usize) + (col as usize + i) / MAX_COLS as usize;

                if temp_row < active_rows(cpu) as usize {
                    write_char_at(cpu, temp_col as u8, temp_row as u8, char_code, attr);
                }
            }
        }

        // AH = 0Bh: Set Color Palette / Background Color
        // BH = 00h: Set Background/Border Color
        //      BL = Color Value (0-15 for Border, 0-31 for CGA Background)
        // BH = 01h: Set Palette (CGA 320x200 Mode 4/5 only)
        //      BL = Palette ID (0 or 1)
        0x0B => {
            let bh = cpu.get_reg8(Register::BH);
            let bl = cpu.get_reg8(Register::BL);

            // Update BIOS Data Area (BDA) at 0x0466.
            // This byte mirrors the CGA Color Select Register (Port 0x3D9).
            let mut current_3d9 = cpu.bus.read_8(0x0466);

            if bh == 0x00 {
                // Set Background / Border Color
                // Bits 0-3 represent the border/background color.
                // Bit 4 is Intensity (sometimes part of background in some modes).

                // Clear lower 5 bits and set new color
                current_3d9 = (current_3d9 & 0xE0) | (bl & 0x1F);
                cpu.bus.write_8(0x0466, current_3d9);

                // TODO: Renderer needs to actually read 0x0466 to
                // draw the border or change the background color of transparent pixels.
            } else if bh == 0x01 {
                // Set CGA Palette
                // Bit 5 controls the active palette in Mode 4.
                // 0 = Palette 0 (Ugly Green/Red/Brown)
                // 1 = Palette 1 (Even uglier Cyan/Magenta/White)

                if (bl & 0x01) != 0 {
                    current_3d9 |= 0x20; // Set Bit 5
                } else {
                    current_3d9 &= !0x20; // Clear Bit 5
                }
                cpu.bus.write_8(0x0466, current_3d9);
            }
        }

        // AH = 0Eh: Teletype Output
        0x0E => {
            let char_code = cpu.get_reg8(Register::AL);
            // Always Page 0 for basic TTY
            let (mut col, mut row) = get_cursor(cpu, 0);

            match char_code {
                0x07 => play_sdl_beep(&mut cpu.bus), // Bell
                0x08 => {
                    // Backspace
                    if col > 0 {
                        col -= 1;
                        // Visual erase
                        write_char_at(cpu, col, row, 0x20, 0x07);
                    }
                }
                0x0D => {
                    // CR
                    col = 0;
                }
                0x0A => {
                    // LF
                    row += 1;
                }
                _ => {
                    // Printable
                    write_char_at(cpu, col, row, char_code, 0x07);
                    col += 1;
                }
            }

            // Handle Line Wrapping
            if col >= MAX_COLS {
                col = 0;
                row += 1;
            }

            // Handle Scrolling
            let rows = active_rows(cpu);
            if row >= rows {
                // Scroll entire screen up by 1 line
                scroll_area(cpu, true, 1, 0x07, 0, 0, rows - 1, MAX_COLS - 1);
                row = rows - 1;
            }

            // Update Cursor (Sync BDA and Internal)
            set_cursor(cpu, col, row, 0);
        }

        // AH = 0Fh: Get Video Mode
        0x0F => {
            // Probably safer to use current state from BDA
            let mode = cpu.bus.read_8(0x0449);
            let cols = cpu.bus.read_16(0x044A) as u8;
            let page = cpu.bus.read_8(0x0462);

            cpu.set_reg8(Register::AL, mode);
            cpu.set_reg8(Register::AH, cols);
            cpu.set_reg8(Register::BH, page);

            //  match cpu.bus.video_mode {
            //     VideoMode::Text40x25 | VideoMode::Text40x25Color => {
            //         cpu.set_reg8(Register::AL, 0x01); // Mode 1
            //         cpu.set_reg8(Register::AH, 40);
            //     }
            //     VideoMode::Text80x25 | VideoMode::Text80x25Color => {
            //         cpu.set_reg8(Register::AL, 0x03); // Mode 3
            //         cpu.set_reg8(Register::AH, 80);
            //     }
            //     VideoMode::Cga320x200 | VideoMode::Cga320x200Color => {
            //         cpu.set_reg8(Register::AL, 0x04); // Mode 4
            //         cpu.set_reg8(Register::AH, 40);
            //     }
            //     VideoMode::Cga640x200 => {
            //         cpu.set_reg8(Register::AL, 0x06); // Mode 6
            //         cpu.set_reg8(Register::AH, 80);
            //     }
            //     VideoMode::Graphics320x200 => {
            //         cpu.set_reg8(Register::AL, 0x13); // Mode 13h
            //         cpu.set_reg8(Register::AH, 40);
            //     }
            // }
            // cpu.set_reg8(Register::BH, 0); // Page 0
        }

        // AH = 10h: Palette / Color Registers
        0x10 => {
            let al = cpu.get_al();
            match al {
                0x00 => {
                    // Set Single Palette Register
                    // BL = Palette Register (0-15)
                    // BH = Color Value
                    let reg = (cpu.bx & 0xFF) as u8 & 0x0F;
                    let val = (cpu.bx >> 8) as u8;
                    cpu.bus.vga.attribute_regs[reg as usize] = val;
                }
                0x01 => {
                    // Set Overscan (Border) Color
                    let val = (cpu.bx >> 8) as u8; // BH
                    cpu.bus.vga.attribute_regs[0x11] = val;
                }
                0x02 => {
                    // Set All Palette Registers + Overscan
                    // ES:DX points to 17 byte table (0-15 + Overscan)
                    let es = cpu.es;
                    let dx = cpu.dx;
                    let addr = cpu.get_physical_addr(es, dx);

                    for i in 0..16 {
                        let val = cpu.bus.read_8(addr + i);
                        cpu.bus.vga.attribute_regs[i as usize] = val;
                    }
                    let border = cpu.bus.read_8(addr + 16);
                    cpu.bus.vga.attribute_regs[0x11] = border;
                }
                0x03 => {
                    // Toggle Blinking / Background Intensity
                    // BL = 0 -> Intensity (bit 7 of attribute = intense background)
                    // BL = 1 -> Blinking
                    let bl = cpu.get_reg8(Register::BL);
                    let mode = cpu.bus.vga.attribute_regs[0x10];
                    if bl == 0 {
                        cpu.bus.vga.attribute_regs[0x10] = mode & !0x08;
                    } else {
                        cpu.bus.vga.attribute_regs[0x10] = mode | 0x08;
                    }
                }
                0x07 => {
                    // Read Individual Palette Register
                    // BL = Register
                    // Return: BH = Value
                    let reg = (cpu.bx & 0xFF) as u8 & 0x0F;
                    let val = cpu.bus.vga.attribute_regs[reg as usize];
                    cpu.set_reg8(Register::BH, val);
                }
                0x08 => {
                    // Read Overscan Color -> BH
                    let val = cpu.bus.vga.attribute_regs[0x11];
                    cpu.set_reg8(Register::BH, val);
                }
                0x09 => {
                    // Read All Palette Registers + Overscan
                    // ES:DX -> 17-byte buffer (16 palette + overscan)
                    let es = cpu.es;
                    let dx = cpu.dx;
                    let addr = cpu.get_physical_addr(es, dx);
                    for i in 0..16 {
                        let val = cpu.bus.vga.attribute_regs[i];
                        cpu.bus.write_8(addr + i, val);
                    }
                    let border = cpu.bus.vga.attribute_regs[0x11];
                    cpu.bus.write_8(addr + 16, border);
                }
                0x10 => {
                    // Set Individual DAC Register
                    // BX = Register (0-255)
                    // DH = Red, CH = Green, CL = Blue (each 6-bit, 0-63)
                    let idx = (cpu.bx & 0xFF) as usize;
                    let r = (cpu.dx >> 8) as u8 & 0x3F; // DH
                    let g = (cpu.cx >> 8) as u8 & 0x3F; // CH
                    let b = (cpu.cx & 0xFF) as u8 & 0x3F; // CL

                    let base = idx * 3;
                    if base + 2 < cpu.bus.vga.palette.len() {
                        cpu.bus.vga.palette[base] = r;
                        cpu.bus.vga.palette[base + 1] = g;
                        cpu.bus.vga.palette[base + 2] = b;
                    }
                }
                0x12 => {
                    // Set Block of DAC Registers
                    // BX = Starting register, CX = Count
                    // ES:DX -> table of (R,G,B) triplets (6-bit values each)
                    let start = (cpu.bx & 0xFF) as usize;
                    let count = cpu.cx as usize;
                    let es = cpu.es;
                    let dx = cpu.dx;
                    let addr = cpu.get_physical_addr(es, dx);

                    for i in 0..count {
                        let base = (start + i) * 3;
                        if base + 2 >= cpu.bus.vga.palette.len() {
                            break;
                        }
                        let src = addr + i * 3;
                        cpu.bus.vga.palette[base] = cpu.bus.read_8(src) & 0x3F;
                        cpu.bus.vga.palette[base + 1] = cpu.bus.read_8(src + 1) & 0x3F;
                        cpu.bus.vga.palette[base + 2] = cpu.bus.read_8(src + 2) & 0x3F;
                    }
                }
                0x13 => {
                    // Select Color Page
                    // BL = 0: Set Paging Mode (BH bit 0 selects 16 pages of 16 / 4 pages of 64)
                    // BL = 1: Set Page Number (BH = page)
                    let bl = cpu.get_reg8(Register::BL);
                    let bh = cpu.get_reg8(Register::BH);
                    if bl == 0 {
                        // Bit 7 of Mode Control Register (attr 0x10) = paging mode
                        let mode = cpu.bus.vga.attribute_regs[0x10];
                        cpu.bus.vga.attribute_regs[0x10] = (mode & !0x80) | ((bh & 1) << 7);
                    } else {
                        cpu.bus.vga.attribute_regs[0x14] = bh;
                    }
                }
                0x15 => {
                    // Read Individual DAC Register
                    // BX = Register
                    // Return: DH=Red, CH=Green, CL=Blue
                    let idx = (cpu.bx & 0xFF) as usize;
                    let base = idx * 3;
                    if base + 2 < cpu.bus.vga.palette.len() {
                        let r = cpu.bus.vga.palette[base];
                        let g = cpu.bus.vga.palette[base + 1];
                        let b = cpu.bus.vga.palette[base + 2];
                        cpu.set_reg8(Register::DH, r);
                        cpu.set_reg8(Register::CH, g);
                        cpu.set_reg8(Register::CL, b);
                    }
                }
                0x17 => {
                    // Read Block of DAC Registers
                    // BX = Starting register, CX = Count
                    // ES:DX -> buffer to receive (R,G,B) triplets
                    let start = (cpu.bx & 0xFF) as usize;
                    let count = cpu.cx as usize;
                    let es = cpu.es;
                    let dx = cpu.dx;
                    let addr = cpu.get_physical_addr(es, dx);

                    for i in 0..count {
                        let base = (start + i) * 3;
                        if base + 2 >= cpu.bus.vga.palette.len() {
                            break;
                        }
                        let r = cpu.bus.vga.palette[base];
                        let g = cpu.bus.vga.palette[base + 1];
                        let b = cpu.bus.vga.palette[base + 2];
                        let dst = addr + i * 3;
                        cpu.bus.write_8(dst, r);
                        cpu.bus.write_8(dst + 1, g);
                        cpu.bus.write_8(dst + 2, b);
                    }
                }
                0x18 => {
                    // Set PEL Mask
                    // BL = Mask
                    cpu.bus.vga.dac_mask = cpu.get_reg8(Register::BL);
                }
                0x19 => {
                    // Read PEL Mask -> BL
                    let mask = cpu.bus.vga.dac_mask;
                    cpu.set_reg8(Register::BL, mask);
                }
                0x1A => {
                    // Read Color Page State
                    // Returns: BH = current page, BL = paging mode (0=4x64, 1=16x16)
                    let mode = cpu.bus.vga.attribute_regs[0x10];
                    let page = cpu.bus.vga.attribute_regs[0x14];
                    cpu.set_reg8(Register::BH, page);
                    cpu.set_reg8(Register::BL, (mode >> 7) & 1);
                }
                0x1B => {
                    // Perform Gray-Scale Summing
                    // BX = starting register, CX = count
                    let start = (cpu.bx & 0xFF) as usize;
                    let count = cpu.cx as usize;

                    for i in 0..count {
                        let base = (start + i) * 3;
                        if base + 2 >= cpu.bus.vga.palette.len() {
                            break;
                        }
                        let r = cpu.bus.vga.palette[base] as u32;
                        let g = cpu.bus.vga.palette[base + 1] as u32;
                        let b = cpu.bus.vga.palette[base + 2] as u32;
                        // NTSC-style luminance formula scaled into 6-bit range
                        let gray = ((r * 30 + g * 59 + b * 11) / 100).min(63) as u8;
                        cpu.bus.vga.palette[base] = gray;
                        cpu.bus.vga.palette[base + 1] = gray;
                        cpu.bus.vga.palette[base + 2] = gray;
                    }
                }
                _ => {
                    cpu.bus
                        .log_string(&format!("[BIOS] Unhandled INT 10h AH=10 AL={:02X}", al));
                }
            }
        }

        // AH = 11h: Character Generator
        0x11 => {
            let al = cpu.get_al();
            match al {
                // AL=10h/11h/12h/14h: Load ROM font and reprogram the CRTC /
                // BDA for the new character height. The "programmed" variants
                // (10-14h) change the displayed row count; the "not programmed"
                // variants (20-24h) just load the font glyphs. We don't maintain
                // a mutable font RAM so there's nothing to copy; we just update
                // the BDA fields the renderer reads.
                //
                // AL=11h: 8x14 font (EGA)  -> 25 rows on 350-line display
                // AL=12h: 8x8 font        -> 43 rows on EGA (350) or 50 rows on VGA (400)
                // AL=14h: 8x16 font (VGA) -> 25 rows on 400-line display
                0x11 | 0x12 | 0x14 => {
                    let (rows_minus_one, char_height) = match al {
                        0x11 => (24u8, 14u16),
                        0x12 => (49u8, 8u16), // assume VGA 400-line display
                        0x14 => (24u8, 16u16),
                        _ => unreachable!(),
                    };
                    cpu.bus.write_8(0x0484, rows_minus_one);
                    cpu.bus.write_16(0x0485, char_height);
                    cpu.bus.log_string(&format!(
                        "[BIOS] INT 10h AH=11h AL={:02X}: font loaded, rows={} height={}",
                        al,
                        rows_minus_one as u16 + 1,
                        char_height
                    ));
                }
                // AL=20h/22h/23h/24h: load font without reprogramming the CRTC.
                // We have nothing to do here (font glyphs come from constant tables)
                // but we need to acknowledge the call so software doesn't think
                // the BIOS is broken.
                0x20 | 0x22 | 0x23 | 0x24 => {}
                0x30 => {
                    // Get Font Information
                    // Returns:
                    //   ES:BP -> pointer to the requested font (selected by BH)
                    //   CX    = character height in scan lines (for that font)
                    //   DL    = CURRENT character rows on screen - 1
                    //           (NOT a property of the queried font — programs
                    //           like Norton Commander use DL as the authoritative
                    //           row count for the current mode. Returning the
                    //           queried font's implied row count here would make
                    //           NC draw its UI scaled to 50 rows even in 80x25.)
                    //
                    // BH = 0: Int 1Fh pointer (8x8)
                    //      1: Int 43h pointer (8x8 first half)
                    //      2: ROM 8x14 font
                    //      3: ROM 8x8 font (lo)
                    //      4: ROM 8x8 font (hi)
                    //      5: ROM 9x14 alternate font
                    //      6: ROM 8x16 font (VGA)
                    //      7: ROM 9x16 alternate font (VGA)
                    let val_bh = cpu.get_reg8(Register::BH);
                    let current_rows_minus_1 = cpu.bus.read_8(0x0484);
                    cpu.set_reg8(Register::DL, current_rows_minus_1);
                    match val_bh {
                        0x00 | 0x01 | 0x03 | 0x04 => {
                            cpu.cx = 8;
                            cpu.es = 0xF000;
                            cpu.bp = 0xFA6E;
                        }
                        0x02 | 0x05 => {
                            cpu.cx = 14;
                            cpu.es = 0xC000;
                            cpu.bp = 0x2000;
                        }
                        0x06 | 0x07 => {
                            cpu.cx = 16;
                            cpu.es = 0xC000;
                            cpu.bp = 0x2000;
                        }
                        _ => {
                            cpu.cx = 16;
                            cpu.es = 0xC000;
                            cpu.bp = 0x2000;
                        }
                    }
                }
                _ => {
                    cpu.bus
                        .log_string(&format!("[BIOS] Unhandled INT 10h AH=11h AL={:02X}", al));
                }
            }
        }

        // AH = 12h: Alternate Function Select
        // BL = 10h: Get Configuration (EGA/VGA)
        0x12 => {
            let bl = cpu.get_reg8(Register::BL);
            match bl {
                0x10 => {
                    // Get Configuration
                    cpu.set_reg8(Register::BH, 0); // Color Mode
                    cpu.set_reg8(Register::BL, 3); // 256KB Video Memory
                    cpu.cx = 0; // Feature bits
                }
                0x30 => {
                    // Select Scan Lines (AL = 0, 1, 2)
                    // We just acknowledge it
                    cpu.set_reg8(Register::AL, 0x12);
                }
                0x34 => {
                    // Cursor Emulation
                    cpu.set_reg8(Register::AL, 0x12); // Supported
                }
                _ => {
                    cpu.bus
                        .log_string(&format!("[BIOS] Unhandled INT 10h AH=12h BL={:02X}", bl));
                }
            }
            cpu.bus.log_string(&format!(
                "[BIOS] AH=12 Return: ax={:04X} bx={:04X} cx={:04X}",
                cpu.ax, cpu.bx, cpu.cx
            ));
        }

        // AH = 13h: Write String
        // AL = Write Mode (0-3)
        // BH = Page Number
        // BL = Attribute (only if AL bit 1 is 0)
        // CX = Length of string
        // DX = Row (DH) / Column (DL)
        // ES:BP = Pointer to string
        0x13 => {
            let mode = cpu.get_al();
            let count = cpu.cx; // CX is loop count
            let page = cpu.get_reg8(Register::BH);
            let attr = cpu.get_reg8(Register::BL);
            let start_row = cpu.get_reg8(Register::DH);
            let start_col = cpu.get_reg8(Register::DL);

            // Pointers
            let es = cpu.es;
            let bp = cpu.bp;

            // Decode Mode bits
            // Bit 0: Update cursor? (0=No, 1=Yes)
            // Bit 1: String contains attributes? (0=No, 1=Yes)
            let update_cursor = (mode & 0x01) != 0;
            let use_string_attr = (mode & 0x02) != 0;

            // Current simulation position (Start where user asked)
            let mut curr_col = start_col;
            let mut curr_row = start_row;

            for i in 0..count {
                // Fetch Data from Memory
                // If Mode 2/3, string is [Char, Attr, Char, Attr...]
                // If Mode 0/1, string is [Char, Char...] and we use BL for Attr
                let (char_code, char_attr) = if use_string_attr {
                    let offset = i.wrapping_mul(2);
                    let c = cpu
                        .bus
                        .read_8(cpu.get_physical_addr(es, bp.wrapping_add(offset)));
                    let a = cpu
                        .bus
                        .read_8(cpu.get_physical_addr(es, bp.wrapping_add(offset) + 1));
                    (c, a)
                } else {
                    let offset = i;
                    let c = cpu
                        .bus
                        .read_8(cpu.get_physical_addr(es, bp.wrapping_add(offset)));
                    (c, attr)
                };

                // BIOS AH=13h treats characters as Teletype (AH=0Eh), meaning
                // it processes CR, LF, BS, and Bell.
                match char_code {
                    0x0D => {
                        // Carriage Return
                        curr_col = 0;
                    }
                    0x0A => {
                        // Line Feed
                        curr_row += 1;
                    }
                    0x08 => {
                        // Backspace
                        if curr_col > 0 {
                            curr_col -= 1;
                            // Visual erase (Space + Light Gray)
                            // Note: We ignore Page for write_char_at in this simple impl
                            write_char_at(cpu, curr_col, curr_row, 0x20, 0x07);
                        }
                    }
                    0x07 => {
                        // Bell
                        play_sdl_beep(&mut cpu.bus);
                    }
                    _ => {
                        // Printable Character
                        write_char_at(cpu, curr_col, curr_row, char_code, char_attr);
                        curr_col += 1;
                    }
                }

                // Handle Line Wrapping
                if curr_col >= MAX_COLS {
                    curr_col = 0;
                    curr_row += 1;
                }

                // Handle Scrolling
                let rows = active_rows(cpu);
                if curr_row >= rows {
                    // Scroll active area up
                    scroll_area(cpu, true, 1, 0x07, 0, 0, rows - 1, MAX_COLS - 1);
                    curr_row = rows - 1;
                }
            }

            // If mode bit 0 is set, the actual BIOS cursor position has to be updated
            if update_cursor {
                set_cursor(cpu, curr_col, curr_row, page);
            }
        }

        // AH = 1Ah: Video Display Combination (VGA/MCGA) for detection
        0x1A => {
            let al = cpu.get_al();
            if al == 0x00 {
                // Get Display Combination Code
                // BL = Active Display (08 = VGA w/ Color Analog)
                // BH = Inactive Display (00 = None)
                cpu.set_reg8(Register::AL, 0x1A); // Function Supported
                cpu.set_reg8(Register::BL, 0x08);
                cpu.set_reg8(Register::BH, 0x00);
            } else {
                cpu.bus
                    .log_string(&format!("[BIOS] Unhandled INT 10h AH=1Ah with AL != 00h"));
            }
        }

        // AH = 1Bh: Get Video State Information
        // ES:DI points to 64-byte buffer
        0x1B => {
            let es = cpu.es;
            let di = cpu.di;
            let addr = cpu.get_physical_addr(es, di);

            // Clear buffer (64 bytes)
            for i in 0..64 {
                cpu.bus.write_8(addr + i, 0);
            }

            // Populate Fields

            // 00: Static Func Table. We point to F000:E000 (Dummy)
            // Storing Offset (E000) then Segment (F000)
            cpu.bus.write_16(addr, 0xE000);
            cpu.bus.write_16(addr + 2, 0xF000);

            // 04: Video Mode
            let mode = match cpu.bus.video_mode {
                VideoMode::Text80x25 => 3,
                VideoMode::Graphics320x200 => 0x13,
                _ => 3,
            };
            cpu.bus.write_8(addr + 4, mode);

            // 05: Columns (80)
            cpu.bus.write_16(addr + 5, 80);

            // 07: Regen Buffer Length (32KB for VGA Text? B8000-BFFFF)
            // In Mode 13h, this should technically be 64KB?
            // The caller is likely in Text Mode when querying.
            cpu.bus.write_16(addr + 7, 0x8000);

            // 09: Regen Buffer Start Offset (0)
            cpu.bus.write_16(addr + 9, 0);

            // 0B: Cursor Pos (Page 0)
            let (col, row) = get_cursor(cpu, 0);
            cpu.bus
                .write_16(addr + 0x0B, (row as u16) << 8 | (col as u16));

            // 1B: Cursor Type
            cpu.bus.write_16(addr + 0x1B, 0x0607);

            // 1D: Active Page
            cpu.bus.write_8(addr + 0x1D, 0);

            // 1E: CRT Port
            cpu.bus.write_16(addr + 0x1E, 0x3D4);

            // 22: Rows on Screen (25)
            cpu.bus.write_8(addr + 0x22, 25);

            // 23: Char Height (16)
            cpu.bus.write_16(addr + 0x23, 16);

            // 25: Active Display Combination Code (DCC)
            // 08 = VGA w/ Color Analog
            cpu.bus.write_8(addr + 0x25, 0x08);

            // 26: Alternate DCC (00 = None)
            cpu.bus.write_8(addr + 0x26, 0x00);

            // 27: Colors supported (Word)
            cpu.bus.write_16(addr + 0x27, 16);

            // 29: Max Pages
            cpu.bus.write_8(addr + 0x29, 8);

            // 2A: Scan Lines (0=200, 1=350, 2=400, 3=480)
            // VGA Text is 400. Mode 13h is 200.
            let scan_code = if mode == 0x13 { 0 } else { 2 };
            cpu.bus.write_8(addr + 0x2A, scan_code);

            // 31: Video Mem (3=256K)
            cpu.bus.write_8(addr + 0x31, 3);

            // Return Success
            cpu.set_reg8(Register::AL, 0x1B);
        }

        // TODO: Check if this makes sense here
        0x4F => {
            // AH=EFh: Extended Video Function (VESA BIOS Extensions)
            let al = cpu.get_reg8(Register::AL);
            match al {
                0x00 => {
                    // AL=00h: Return VBE Controller Info
                    let es = cpu.es;
                    let di = cpu.di;
                    let addr = cpu.get_physical_addr(es, di);
                    let vbe_signature = b"VESA";
                    for i in 0..4 {
                        cpu.bus.write_8(addr + i, vbe_signature[i]);
                    }
                    // TODO:Other fields zero for now
                    cpu.set_reg8(Register::AL, 0x4F); // Function supported
                    cpu.set_reg8(Register::AH, 0x00); // Function successful
                }
                0x01 => {
                    // AL=01h: Return VBE Mode Info
                    let es = cpu.es;
                    let di = cpu.di;
                    let addr = cpu.get_physical_addr(es, di);
                    // For simplicity, only implement mode 0x101 (640x480x256)
                    let mode_number: u16 = 0x101;
                    cpu.bus.write_16(addr, mode_number);
                    // TODO: Other fields zero for now
                    cpu.set_reg8(Register::AL, 0x4F); // Function supported
                    cpu.set_reg8(Register::AH, 0x00); // Function successful
                }
                _ => {
                    cpu.set_reg8(Register::AL, 0x4F); // Function supported
                    cpu.set_reg8(Register::AH, 0x01); // Function failed
                }
            }
        }

        // AH = 0Ch: Write Graphics Pixel
        // AL = Color Value
        // BH = Page Number (Ignored in Mode 13h)
        // CX = Column (X)
        // DX = Row (Y)
        0x0C => {
            let color = cpu.get_al();
            let x = cpu.get_reg16(Register::CX) as usize;
            let y = cpu.get_reg16(Register::DX) as usize;

            // Mode 13h Dimensions
            let width = 320;
            let height = 200;

            if x < width && y < height {
                // Calculate Linear Address for Mode 13h (0xA0000 base)
                let offset = 0xA0000 + (y * width + x);
                cpu.bus.write_8(offset, color);
            }
        }

        // AH = 0Dh: Read Graphics Pixel
        // BH = Page Number (Ignored in Mode 13h)
        // CX = Column (X)
        // DX = Row (Y)
        // Returns: AL = Color Value
        0x0D => {
            let x = cpu.get_reg16(Register::CX) as usize;
            let y = cpu.get_reg16(Register::DX) as usize;
            let width = 320;
            let height = 200;

            let color = if x < width && y < height {
                let offset = 0xA0000 + (y * width + x);
                cpu.bus.read_8(offset)
            } else {
                0 // Return black if out of bounds
            };

            cpu.set_reg8(Register::AL, color);
        }

        0xEF => {
            // Hercules Graphics Card Functions
        }
        0x5F => {
            // Not sure what this is used for
        }

        _ => cpu
            .bus
            .log_string(&format!("[BIOS] Unhandled INT 10h AH={:02X}", cpu.get_ah())),
    }
}

/// Sets the cursor position in BOTH BDA and Internal State
fn set_cursor(cpu: &mut Cpu, col: u8, row: u8, page: u8) {
    if page < 8 {
        // Update BDA (The Source of Truth for BIOS)
        let addr = BDA_CURSOR_POS + (page as usize * 2);
        cpu.bus.write_8(addr, col);
        cpu.bus.write_8(addr + 1, row);

        // Update Internal State (If Active Page)
        // This fixes the desync where renderer looked at old internal state
        if page == 0 {
            cpu.bus.cursor_x = col as usize;
            cpu.bus.cursor_y = row as usize;
        }
    }
}

/// Reads the cursor position from BDA
fn get_cursor(cpu: &Cpu, page: u8) -> (u8, u8) {
    if page < 8 {
        let addr = BDA_CURSOR_POS + (page as usize * 2);
        let col = cpu.bus.read_8(addr);
        let row = cpu.bus.read_8(addr + 1);
        (col, row)
    } else {
        (0, 0)
    }
}

/// Writes a character and attribute to VRAM (Text Mode)
fn write_char_at(cpu: &mut Cpu, col: u8, row: u8, char_code: u8, attr: u8) {
    match cpu.bus.video_mode {
        // Standard Text Modes
        VideoMode::Text80x25
        | VideoMode::Text80x25Color
        | VideoMode::Text40x25
        | VideoMode::Text40x25Color => {
            let cols = if cpu.bus.video_mode == VideoMode::Text40x25
                || cpu.bus.video_mode == VideoMode::Text40x25Color
            {
                40
            } else {
                80
            };

            let offset = (row as usize * cols + col as usize) * 2;
            if offset < cpu.bus.vga.vram_text.len() {
                cpu.bus.write_8(ADDR_VGA_TEXT + offset, char_code);
                cpu.bus.write_8(ADDR_VGA_TEXT + offset + 1, attr);
            }
        }
        // TODO: Graphics Mode font rendering
        _ => {
            cpu.bus
                .log_string("[BIOS] write_char_at called in unsupported video mode");
        }
    }
}

/// Reads a character and attribute from VRAM (Text Mode)
fn read_char_at(cpu: &Cpu, col: u8, row: u8, _page: u8) -> (u8, u8) {
    match cpu.bus.video_mode {
        VideoMode::Text80x25
        | VideoMode::Text80x25Color
        | VideoMode::Text40x25
        | VideoMode::Text40x25Color => {
            let cols = if cpu.bus.video_mode == VideoMode::Text40x25
                || cpu.bus.video_mode == VideoMode::Text40x25Color
            {
                40
            } else {
                80
            };

            let offset = (row as usize * cols + col as usize) * 2;
            if offset < cpu.bus.vga.vram_text.len() {
                let char_code = cpu.bus.read_8(ADDR_VGA_TEXT + offset);
                let attr = cpu.bus.read_8(ADDR_VGA_TEXT + offset + 1);
                (char_code, attr)
            } else {
                (0, 0)
            }
        }
        _ => (0, 0),
    }
}

/// Generic Scroll Function (Handles AH=06, AH=07, AH=00, AH=0E)
/// lines=0 means "Clear Window"
fn scroll_area(
    cpu: &mut Cpu,
    up: bool,
    lines: u8,
    attr: u8,
    row_start: u8,
    col_start: u8,
    row_end: u8,
    col_end: u8,
) {
    // Check for Graphics Mode Clearing
    let is_graphics = matches!(
        cpu.bus.video_mode,
        VideoMode::Cga320x200
            | VideoMode::Cga320x200Color
            | VideoMode::Cga640x200
            | VideoMode::Graphics320x200
    );

    // If we are in graphics mode and asked to "Clear Screen" (lines = 0),
    // just zero out the VRAM.
    if is_graphics && lines == 0 {
        // Determine which VRAM buffer to clear
        if cpu.bus.video_mode == VideoMode::Graphics320x200 {
            for i in 0..cpu.bus.vga.vram_graphics.len() {
                cpu.bus.vga.vram_graphics[i] = 0;
            }
        } else {
            // CGA Modes use the text buffer range
            for i in 0..16384 {
                // 16KB CGA Memory
                if i < cpu.bus.vga.vram_text.len() {
                    cpu.bus.vga.vram_text[i] = 0;
                }
            }
        }
        cpu.bus.vga.dirty = true;
        return;
    }

    // Safety Clamps for Text Mode Logic
    let max_cols = if cpu.bus.video_mode == VideoMode::Text40x25
        || cpu.bus.video_mode == VideoMode::Text40x25Color
    {
        40
    } else {
        80
    };

    // Safety Clamps. Use the BDA row count so scrolling respects 80x43/50.
    let rows = active_rows(cpu) as usize;
    let r_start = row_start as usize;
    let r_end = (row_end as usize).min(rows.saturating_sub(1));
    let c_start = col_start as usize;
    let c_end = (col_end as usize).min(max_cols - 1);
    let count = lines as usize;

    // Standard Text Mode Clear/Scroll Logic
    if count == 0 {
        for r in r_start..=r_end {
            for c in c_start..=c_end {
                write_char_at(cpu, c as u8, r as u8, 0x20, attr);
            }
        }
        return;
    }

    if up {
        // Scroll Up (Copy Lower -> Upper)
        for r in r_start..=(r_end.saturating_sub(count)) {
            for c in c_start..=c_end {
                let src_r = r + count;
                // Read from Source
                let src_offset = (src_r * max_cols + c) * 2;

                // Read directly from bus to handle scrolling
                // Use read_8 directly because there's no read_char_at
                let val = cpu.bus.read_8(ADDR_VGA_TEXT + src_offset);
                let at = cpu.bus.read_8(ADDR_VGA_TEXT + src_offset + 1);

                // Write to Dest
                write_char_at(cpu, c as u8, r as u8, val, at);
            }
        }
        // Clear new bottom lines
        let clear_start = (r_end.saturating_sub(count)) + 1;
        for r in clear_start..=r_end {
            for c in c_start..=c_end {
                write_char_at(cpu, c as u8, r as u8, 0x20, attr);
            }
        }
    } else {
        // Scroll Down (Copy Upper -> Lower) - Iterate Reverse
        // Used by AH=07
        let effective_start = r_start + count;
        if effective_start <= r_end {
            for r in (effective_start..=r_end).rev() {
                for c in c_start..=c_end {
                    let src_r = r - count;
                    let src_offset = (src_r * max_cols + c) * 2;
                    let val = cpu.bus.read_8(ADDR_VGA_TEXT + src_offset);
                    let at = cpu.bus.read_8(ADDR_VGA_TEXT + src_offset + 1);

                    write_char_at(cpu, c as u8, r as u8, val, at);
                }
            }
        }
        // Clear top lines
        let clear_end = (r_start + count).min(r_end + 1);
        for r in r_start..clear_end {
            for c in c_start..=c_end {
                write_char_at(cpu, c as u8, r as u8, 0x20, attr);
            }
        }
    }
}
