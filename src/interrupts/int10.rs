use iced_x86::Register;
use crate::cpu::Cpu;
use crate::video::{VideoMode, ADDR_VGA_TEXT, BDA_CURSOR_POS, BDA_CURSOR_MODE, MAX_COLS, MAX_ROWS};
use crate::audio::play_sdl_beep;

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    match ah {
        // AH = 00h: Set Video Mode
        0x00 => {
            let mode = cpu.get_al();
            
            // Clear Screen
            match mode {
                // Text Modes: Clear with Spaces and Attribute 0x07
                0x00..=0x03 => {
                    scroll_area(cpu, true, 0, 0x07, 0, 0, MAX_ROWS - 1, MAX_COLS - 1);
                }
                // CGA Graphics Modes (4, 5, 6): Zero out 16KB of B8000 Memory
                0x04..=0x06 => {
                    for i in 0..16384 {
                        if i < cpu.bus.vram_text.len() {
                            cpu.bus.vram_text[i] = 0x00;
                        }
                    }
                }
                // VGA Graphics Mode (13h): Zero out 64KB of A0000 Memory
                0x13 => {
                    for i in 0..cpu.bus.vram_graphics.len() {
                        cpu.bus.vram_graphics[i] = 0x00;
                    }
                }
                // Fallback / Stubbed modes
                _ => {
                    // Optional: Clear text ram just in case
                     scroll_area(cpu, true, 0, 0x07, 0, 0, MAX_ROWS - 1, MAX_COLS - 1);
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
                    cpu.bus.log_string("[BIOS] Switch to Text Mode (40x25Color)");
                    cpu.bus.video_mode = VideoMode::Text40x25Color;
                }
                0x02 => {
                    cpu.bus.log_string("[BIOS] Switch to Text Mode (80x25)");
                    cpu.bus.video_mode = VideoMode::Text80x25;
                }
                0x03 => {
                    cpu.bus.log_string("[BIOS] Switch to Text Mode (80x25 Color)");
                    cpu.bus.video_mode = VideoMode::Text80x25Color;
                }
                0x04 => {
                    cpu.bus.log_string("[BIOS] Switch to CGA Graphics Mode (320x200 Color)");
                    cpu.bus.video_mode = VideoMode::Cga320x200Color;
                }
                0x06 => {
                    cpu.bus.log_string("[BIOS] Switch to CGA Graphics Mode (640x200)");
                    cpu.bus.video_mode = VideoMode::Cga640x200;
                }
                // TODO: EGA/VGA Modes 
                0x0D | 0x0E | 0x10 | 0x12 => {
                     cpu.bus.log_string(&format!("[BIOS] Switch to EGA/VGA Mode {:02X} (NOT IMPLEMENTED)", mode));
                     // We default to Text80x25 internally so the emulator doesn't crash.
                     // TODO: Proper EGA with Planar Memory emulation.
                     cpu.bus.video_mode = VideoMode::Text80x25; 
                }
                0x13 => {
                    cpu.bus.log_string("[BIOS] Switch to Graphics Mode (320x200)");
                    cpu.bus.video_mode = VideoMode::Graphics320x200;
                }
                _ => cpu.bus.log_string(&format!("[BIOS] Unsupported Video Mode {:02X}", mode)),
            }

            cpu.bus.write_8(0x0449, cpu.bus.video_mode as u8); // Update BDA Current Video Mode
            cpu.bus.write_8(0x0462, 0); // Update BDA Active Page to 0
            let cols: u16 = match mode {
                0x00 | 0x01 | 0x04 | 0x05 => 40,
                0x13 => 40, // Mode 13h uses 40 columns text
                _ => 80,
            };
            cpu.bus.write_16(0x044A, cols);
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
            cpu.bus.log_string(&format!("[BIOS] Set Active Page to {}", page));
        }

        // AH = 06h: Scroll Up
        0x06 => {
            let lines = cpu.get_reg8(Register::AL);
            let attr = cpu.get_reg8(Register::BH);
            let row_start = cpu.get_reg8(Register::CH);
            let col_start = cpu.get_reg8(Register::CL);
            let row_end = cpu.get_reg8(Register::DH);
            let col_end = cpu.get_reg8(Register::DL);
            
            scroll_area(cpu, true, lines, attr, row_start, col_start, row_end, col_end);
        }

        // AH = 07h: Scroll Down
        0x07 => {
            let lines = cpu.get_reg8(Register::AL);
            let attr = cpu.get_reg8(Register::BH);
            let row_start = cpu.get_reg8(Register::CH);
            let col_start = cpu.get_reg8(Register::CL);
            let row_end = cpu.get_reg8(Register::DH);
            let col_end = cpu.get_reg8(Register::DL);
            
            scroll_area(cpu, false, lines, attr, row_start, col_start, row_end, col_end);
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
                
                if temp_row < MAX_ROWS as usize {
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
                0x08 => { // Backspace
                    if col > 0 { 
                        col -= 1; 
                        // Visual erase
                        write_char_at(cpu, col, row, 0x20, 0x07);
                    }
                }
                0x0D => { // CR
                    col = 0;
                }
                0x0A => { // LF
                    row += 1;
                }
                _ => { // Printable
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
            if row >= MAX_ROWS {
                // Scroll entire screen up by 1 line
                scroll_area(cpu, true, 1, 0x07, 0, 0, MAX_ROWS - 1, MAX_COLS - 1);
                row = MAX_ROWS - 1;
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

        // AH = 11h: Character Generator
        0x11 => {
            // AL=00 (Load User Font), AL=30 (Get Font Info)
            // TODO: Implement
        }

        // AH = 12h: Alternate Function Select
        // BL = 10h: Get Configuration (EGA/VGA)
        0x12 => {
            let bl = cpu.get_reg8(Register::BL);
            if bl == 0x10 {
                cpu.set_reg8(Register::BH, 0); // Color Mode
                cpu.set_reg8(Register::BL, 3); // 256KB Video Memory
                cpu.cx = 0; // Feature bits
            }
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
                    let c = cpu.bus.read_8(cpu.get_physical_addr(es, bp.wrapping_add(offset)));
                    let a = cpu.bus.read_8(cpu.get_physical_addr(es, bp.wrapping_add(offset) + 1));
                    (c, a)
                } else {
                    let offset = i;
                    let c = cpu.bus.read_8(cpu.get_physical_addr(es, bp.wrapping_add(offset)));
                    (c, attr)
                };

                // BIOS AH=13h treats characters as Teletype (AH=0Eh), meaning
                // it processes CR, LF, BS, and Bell.
                match char_code {
                    0x0D => { // Carriage Return
                        curr_col = 0;
                    }
                    0x0A => { // Line Feed
                        curr_row += 1;
                    }
                    0x08 => { // Backspace
                        if curr_col > 0 { 
                            curr_col -= 1;
                            // Visual erase (Space + Light Gray)
                            // Note: We ignore Page for write_char_at in this simple impl
                            write_char_at(cpu, curr_col, curr_row, 0x20, 0x07);
                        }
                    }
                    0x07 => { // Bell
                        play_sdl_beep(&mut cpu.bus);
                    }
                    _ => { // Printable Character
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
                if curr_row >= MAX_ROWS {
                    // Scroll active area up
                    scroll_area(cpu, true, 1, 0x07, 0, 0, MAX_ROWS - 1, MAX_COLS - 1);
                    curr_row = MAX_ROWS - 1;
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
                cpu.bus.log_string(&format!("[BIOS] Unhandled INT 10h AH=1Ah with AL != 00h"));
            }
        }

        // AH = 1Bh: Get Video State Information
        // ES:DI points to 64-byte buffer
        0x1B => {
            let es = cpu.es;
            let di = cpu.di;
            let addr = cpu.get_physical_addr(es, di);

            // Write static table (Simulate VGA)
            // Offset 0: Static Functionality Table (Ptr) - 0:0 for now
            // TODO: Implement full table

            cpu.bus.write_8(addr, 0x00); 
            // Often AL=1B on return implies supported.
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

        _ => cpu.bus.log_string(&format!("[BIOS] Unhandled INT 10h AH={:02X}", cpu.get_ah())),
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
        VideoMode::Text80x25 | VideoMode::Text80x25Color | 
        VideoMode::Text40x25 | VideoMode::Text40x25Color => {
            let cols = if cpu.bus.video_mode == VideoMode::Text40x25 || 
                          cpu.bus.video_mode == VideoMode::Text40x25Color { 40 } else { 80 };
            
            let offset = (row as usize * cols + col as usize) * 2;
            if offset < cpu.bus.vram_text.len() {
                cpu.bus.write_8(ADDR_VGA_TEXT + offset, char_code);
                cpu.bus.write_8(ADDR_VGA_TEXT + offset + 1, attr);
            }
        }
        // TODO: Graphics Mode font rendering
        _ => { cpu.bus.log_string("[BIOS] write_char_at called in unsupported video mode"); }
    }
}

/// Generic Scroll Function (Handles AH=06, AH=07, AH=00, AH=0E)
/// lines=0 means "Clear Window"
fn scroll_area(cpu: &mut Cpu, up: bool, lines: u8, attr: u8, 
               row_start: u8, col_start: u8, row_end: u8, col_end: u8) {
    
    // Check for Graphics Mode Clearing
    let is_graphics = matches!(cpu.bus.video_mode, 
        VideoMode::Cga320x200 | VideoMode::Cga320x200Color | VideoMode::Cga640x200 | VideoMode::Graphics320x200
    );

    // If we are in graphics mode and asked to "Clear Screen" (lines = 0),
    // just zero out the VRAM.
    if is_graphics && lines == 0 {
        // Determine which VRAM buffer to clear
        if cpu.bus.video_mode == VideoMode::Graphics320x200 {
             for i in 0..cpu.bus.vram_graphics.len() { cpu.bus.vram_graphics[i] = 0; }
        } else {
             // CGA Modes use the text buffer range
             for i in 0..16384 { // 16KB CGA Memory
                 if i < cpu.bus.vram_text.len() { cpu.bus.vram_text[i] = 0; }
             }
        }
        return;
    }

    // Safety Clamps for Text Mode Logic
    let max_cols = if cpu.bus.video_mode == VideoMode::Text40x25 || 
                      cpu.bus.video_mode == VideoMode::Text40x25Color { 40 } else { 80 };
    
    // Safety Clamps
    let r_start = row_start as usize;
    let r_end = (row_end as usize).min(MAX_ROWS as usize - 1);
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