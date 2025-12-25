use iced_x86::Register;

use crate::audio::play_sdl_beep;
use crate::bus::Bus;
use crate::command::{run_dir_command, run_type_command};
use crate::cpu::{Cpu, CpuState};
use crate::video::{print_char, print_string, VideoMode};

// Helper to read a string from memory (DS:DX) until 0x00 (ASCIIZ)
fn read_asciiz_string(bus: &Bus, addr: usize) -> String {
    let mut curr = addr;
    let mut chars = Vec::new();
    loop {
        let byte = bus.ram[curr];
        if byte == 0 {
            break;
        }
        chars.push(byte);
        curr += 1;
    }
    String::from_utf8_lossy(&chars).to_string()
}

/// Converts a filename pattern (e.g., "*.*", "FILE.TXT") to DOS FCB format (11 bytes).
fn pattern_to_fcb(pattern: &str) -> [u8; 11] {
    let mut fcb = [b' '; 11];
    let upper = pattern.to_uppercase();
    
    // Split into Name and Extension
    let (name, ext) = match upper.rsplit_once('.') {
        Some((n, e)) => (n, e),
        None => (upper.as_str(), ""),
    };

    // Process Name (first 8 bytes)
    for (i, byte) in name.bytes().enumerate() {
        if i >= 8 { break; }
        if byte == b'*' {
            // Fill remaining name chars with '?'
            for j in i..8 { fcb[j] = b'?'; }
            break;
        } else {
            fcb[i] = byte;
        }
    }

    // Process Extension (last 3 bytes)
    for (i, byte) in ext.bytes().enumerate() {
        if i >= 3 { break; }
        if byte == b'*' {
             // Fill remaining ext chars with '?'
            for j in i..3 { fcb[8 + j] = b'?'; }
            break;
        } else {
            fcb[8 + i] = byte;
        }
    }

    fcb
}

// Helper: Reconstruct "NAME.EXT" from the DTA's fixed-width 11-byte template
fn read_dta_template(bus: &Bus, dta_phys: usize) -> String {
    let mut name = String::new();
    let mut ext = String::new();

    // Read Name (Offsets 1-8)
    for i in 0..8 {
        let c = bus.read_8(dta_phys + 1 + i);
        // DOS uses 0x20 (Space) for padding. 0x3F is '?'.
        if c > 0x20 { 
            name.push(c as char); 
        } else if c == b'?' {
            name.push('?');
        }
    }

    // Read Extension (Offsets 9-11)
    for i in 0..3 {
        let c = bus.read_8(dta_phys + 9 + i);
        if c > 0x20 { 
            ext.push(c as char); 
        } else if c == b'?' {
            ext.push('?');
        }
    }

    // Handle "????????.???" case (Equivalent to *.*)
    if name.chars().all(|c| c == '?') && ext.chars().all(|c| c == '?') {
        return "*.*".to_string();
    }

    if ext.is_empty() {
        name
    } else {
        format!("{}.{}", name, ext)
    }
}

// Interrupt handler
pub fn handle_interrupt(cpu: &mut Cpu, vector: u8) {
    match vector {
        // INT 00h: Divide by Zero Exception
        0x00 => {
            cpu.bus
                .log_string("[CPU] EXCEPTION: Divide by Zero (INT 0). Terminating Program.");
            print_string(cpu, "Divide overflow\r\n");

            // In a real CPU, this jumps to the handler in the IVT. We just go back to the shell
            cpu.state = crate::cpu::CpuState::RebootShell;
        }

        // INT 10h: Video Services (BIOS)
        0x10 => {
            let ah = cpu.get_ah();
            match ah {
                // AH = 00h: Set Video Mode
                0x00 => {
                    let mode = cpu.get_al();

                    // Fill Text VRAM (B8000) with "Space" (0x20) + "Gray on Black" (0x07)
                    for i in (0..4000).step_by(2) {
                        cpu.bus.write_8(0xB8000 + i, 0x20);
                        cpu.bus.write_8(0xB8000 + i + 1, 0x07);
                    }

                    match mode {
                        // Mode 00 & 01: 40x25 Text (BW/Color)
                        // We map this to 80x25 to prevent crash, though it will look wide.
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
                        0x13 => {
                            cpu.bus
                                .log_string("[BIOS] Switch to Graphics Mode (320x200)");
                            cpu.bus.video_mode = VideoMode::Graphics320x200;
                        }
                        _ => cpu
                            .bus
                            .log_string(&format!("[BIOS] Unsupported Video Mode {:02X}", mode)),
                    }
                }

                //AH = 01h: Set Cursor Type (Shape/Visibility)
                // CH = Start Scanline (Bit 5 = 1 means invisible)
                // CL = End Scanline
                0x01 => {
                    let cx = cpu.cx;
                    // Store in BIOS Data Area 0x0460
                    cpu.bus.write_16(0x0460, cx);
                }

                // AH = 02h: Set Cursor Position
                // Entry: BH = Page Number (0-7)
                //        DH = Row
                //        DL = Column
                0x02 => {
                    let page = cpu.get_reg8(Register::BH) as usize;
                    let row = cpu.get_reg8(Register::DH);
                    let col = cpu.get_reg8(Register::DL);

                    // The BIOS stores cursor positions at physical address 0x0450.
                    // There are 8 pages supported, 2 bytes per page.
                    // Format: Byte 0 = Column, Byte 1 = Row.
                    if page < 8 {
                        let cursor_addr = 0x450 + (page * 2);
                        cpu.bus.write_8(cursor_addr, col);
                        cpu.bus.write_8(cursor_addr + 1, row);
                    }

                    // Note: If you implement a blinking hardware cursor in your
                    // render loop later, you should read coordinates from 0x0450.
                }

                // AH = 03h: Get Cursor Position (Complimentary to 02h)
                // Entry: BH = Page Number
                // Return: DH = Row, DL = Column, CX = Cursor Mode (Scanlines)
                0x03 => {
                    let page = cpu.get_reg8(Register::BH) as usize;

                    if page < 8 {
                        let cursor_addr = 0x450 + (page * 2);
                        let col = cpu.bus.read_8(cursor_addr);
                        let row = cpu.bus.read_8(cursor_addr + 1);

                        cpu.set_reg8(Register::DL, col);
                        cpu.set_reg8(Register::DH, row);

                        // Read cursor shape from BDA 0x0460
                        cpu.cx = cpu.bus.read_16(0x0460);
                    }
                }

                // TODO: Verify
                0x04 => {
                    // AH = 04h: Read Light Pen Position (Not Implemented)
                    // Return: CX = Column, DX = Row
                    cpu.cx = 0;
                    cpu.dx = 0;
                }

                // AH = 05h: Set active page
                // TODO: Verify
                0x05 => {
                    let page = cpu.get_reg8(Register::AL);
                    cpu.bus.log_string(&format!("[BIOS] Set Active Page to {}", page));
                }


                // AH = 06h: Scroll Up Window (or Clear)
                // AL = Lines to scroll (0 = Clear Window)
                // BH = Attribute for blank lines
                // CX = Upper Left (CH=Row, CL=Col)
                // DX = Lower Right (DH=Row, DL=Col)
                0x06 => {
                    let lines = cpu.get_reg8(Register::AL);
                    let attr = cpu.get_reg8(Register::BH);
                    let row_start = cpu.get_reg8(Register::CH) as usize;
                    let col_start = cpu.get_reg8(Register::CL) as usize;
                    let row_end = cpu.get_reg8(Register::DH) as usize;
                    let col_end = cpu.get_reg8(Register::DL) as usize;

                    // Basic bounds check (assuming 80 columns)
                    let max_cols = 80;

                    // Case 1: Clear Window (AL = 0)
                    // This is the most common usage.
                    if lines == 0 {
                        for r in row_start..=row_end {
                            for c in col_start..=col_end {
                                let offset = (r * max_cols + c) * 2;
                                let phys_addr = 0xB8000 + offset;
                                cpu.bus.write_8(phys_addr, 0x20); // Char: Space
                                cpu.bus.write_8(phys_addr + 1, attr); // Attr: BH
                            }
                        }
                    } else {
                        // Case 2: Scroll Up
                        // For every row from Top to (Bottom - Lines), copy data from (Row + Lines)
                        for r in row_start..=(row_end - lines as usize) {
                            for c in col_start..=col_end {
                                let dest_offset = (r * max_cols + c) * 2;
                                let src_offset = ((r + lines as usize) * max_cols + c) * 2;

                                let char_code = cpu.bus.read_8(0xB8000 + src_offset);
                                let char_attr = cpu.bus.read_8(0xB8000 + src_offset + 1);

                                cpu.bus.write_8(0xB8000 + dest_offset, char_code);
                                cpu.bus.write_8(0xB8000 + dest_offset + 1, char_attr);
                            }
                        }

                        // Clear the bottom 'lines' rows
                        for r in (row_end - lines as usize + 1)..=row_end {
                            for c in col_start..=col_end {
                                let offset = (r * max_cols + c) * 2;
                                let phys_addr = 0xB8000 + offset;
                                cpu.bus.write_8(phys_addr, 0x20);
                                cpu.bus.write_8(phys_addr + 1, attr);
                            }
                        }
                    }
                }

                // AH = 07h: Scroll Down Window (or Clear)
                // Inputs: Same as AH=06
                // AL = Lines to scroll (0 = Clear Window)
                // BH = Attribute for blank lines
                // CX = Upper Left (CH=Row, CL=Col)
                // DX = Lower Right (DH=Row, DL=Col)
                0x07 => {
                    let lines = cpu.get_reg8(Register::AL);
                    let attr = cpu.get_reg8(Register::BH);
                    let row_start = cpu.get_reg8(Register::CH) as usize;
                    let col_start = cpu.get_reg8(Register::CL) as usize;
                    let row_end = cpu.get_reg8(Register::DH) as usize;
                    let col_end = cpu.get_reg8(Register::DL) as usize;

                    let max_cols = 80;

                    // Case 1: Clear Window (AL = 0)
                    // Identical to Scroll Up logic
                    if lines == 0 {
                        for r in row_start..=row_end {
                            for c in col_start..=col_end {
                                let offset = (r * max_cols + c) * 2;
                                let phys_addr = 0xB8000 + offset;
                                cpu.bus.write_8(phys_addr, 0x20); // Space
                                cpu.bus.write_8(phys_addr + 1, attr);
                            }
                        }
                    } else {
                        // Case 2: Scroll Down
                        // We must copy BACKWARDS (Bottom -> Top)
                        // Dest: Row r
                        // Source: Row r - lines

                        // Safety check to prevent underflow if lines > window height
                        let effective_start = row_start + lines as usize;

                        if effective_start <= row_end {
                            for r in (effective_start..=row_end).rev() {
                                for c in col_start..=col_end {
                                    let dest_offset = (r * max_cols + c) * 2;
                                    let src_offset = ((r - lines as usize) * max_cols + c) * 2;

                                    let char_code = cpu.bus.read_8(0xB8000 + src_offset);
                                    let char_attr = cpu.bus.read_8(0xB8000 + src_offset + 1);

                                    cpu.bus.write_8(0xB8000 + dest_offset, char_code);
                                    cpu.bus.write_8(0xB8000 + dest_offset + 1, char_attr);
                                }
                            }
                        }

                        // Clear the Top 'lines' rows
                        // Limit loop to row_end to handle massive scrolls
                        let clear_limit = std::cmp::min(row_start + lines as usize, row_end + 1);

                        for r in row_start..clear_limit {
                            for c in col_start..=col_end {
                                let offset = (r * max_cols + c) * 2;
                                let phys_addr = 0xB8000 + offset;
                                cpu.bus.write_8(phys_addr, 0x20);
                                cpu.bus.write_8(phys_addr + 1, attr);
                            }
                        }
                    }
                }

                // AH = 0Eh: Teletype Output
                // AL = Character to write
                // BH = Page Number (Assumed 0)
                // BL = Color (Graphics Mode only, ignored in Text Mode)
                0x0E => {
                    let char_code = cpu.get_reg8(Register::AL);

                    // Get Current Cursor Position (BDA 0x0450)
                    // We assume Page 0 for simplicity.
                    let cursor_addr = 0x0450;
                    let mut col = cpu.bus.read_8(cursor_addr);
                    let mut row = cpu.bus.read_8(cursor_addr + 1);

                    let max_cols = 80;
                    let max_rows = 25;

                    match char_code {
                        0x07 => {
                            // BEL (Bell) -> Beep
                            play_sdl_beep(&mut cpu.bus);
                        }
                        0x0D => {
                            // CR (Carriage Return) -> Reset Column
                            col = 0;
                        }
                        0x0A => {
                            // LF (Line Feed) -> Next Row
                            row += 1;
                        }
                        0x08 => {
                            // BS (Backspace) -> Previous Column
                            if col > 0 {
                                col -= 1;
                            }
                            // Calculate the position we just moved back to
                            let vram_offset = (row as usize * max_cols + col as usize) * 2;

                            // Overwrite with Space (0x20) and default attribute (0x07)
                            cpu.bus.write_8(0xB8000 + vram_offset, 0x20);
                            cpu.bus.write_8(0xB8000 + vram_offset + 1, 0x07);
                        }
                        _ => {
                            // Visible Character
                            // Write Char + Attribute to VRAM
                            let vram_offset = (row as usize * max_cols + col as usize) * 2;
                            if vram_offset < 4000 {
                                cpu.bus.write_8(0xB8000 + vram_offset, char_code);
                                cpu.bus.write_8(0xB8000 + vram_offset + 1, 0x07);
                                // Light Gray
                            }

                            col += 1;

                            // Handle Line Wrap
                            if col >= max_cols as u8 {
                                col = 0;
                                row += 1;
                            }
                        }
                    }

                    // Handle Scrolling (If we hit the bottom)
                    if row >= max_rows as u8 {
                        // Move rows 1-24 UP to 0-23
                        // Total size: 80 cols * 2 bytes = 160 bytes per row
                        for r in 1..max_rows {
                            for c in 0..max_cols {
                                let src_offset = (r * max_cols + c) * 2;
                                let dest_offset = ((r - 1) * max_cols + c) * 2;

                                let val = cpu.bus.read_8(0xB8000 + src_offset);
                                let attr = cpu.bus.read_8(0xB8000 + src_offset + 1);

                                cpu.bus.write_8(0xB8000 + dest_offset, val);
                                cpu.bus.write_8(0xB8000 + dest_offset + 1, attr);
                            }
                        }

                        // Clear the Bottom Row (Row 24)
                        let last_row_start = ((max_rows - 1) * max_cols) * 2;
                        for i in (0..160).step_by(2) {
                            cpu.bus.write_8(0xB8000 + last_row_start + i, 0x20); // Space
                            cpu.bus.write_8(0xB8000 + last_row_start + i + 1, 0x07);
                            // Attribute
                        }

                        // Reset Row to the last line
                        row = (max_rows - 1) as u8;
                    }

                    // Update Cursor Position in BDA
                    cpu.bus.write_8(cursor_addr, col);
                    cpu.bus.write_8(cursor_addr + 1, row);
                }

                // AH = 0Fh: Get Video Mode
                // Returns: AL = Current Mode
                //          AH = Number of Columns (80 or 40)
                //          BH = Active Page (usually 0)
                0x0F => {
                    match cpu.bus.video_mode {
                        VideoMode::Text40x25 => {
                            cpu.bus.log_string("[BIOS] Reported Video Mode: 0x00");
                            cpu.set_reg8(Register::AL, 0x00); // Current Mode 0
                            cpu.set_reg8(Register::AH, 40); // 40 Columns
                            cpu.set_reg8(Register::BH, 0); // Page 0
                        }
                        VideoMode::Text40x25Color => {
                            cpu.bus.log_string("[BIOS] Reported Video Mode: 0x01");
                            cpu.set_reg8(Register::AL, 0x01); // Current Mode 1
                            cpu.set_reg8(Register::AH, 40); // 40 Columns
                            cpu.set_reg8(Register::BH, 0); // Page 0
                        }
                        VideoMode::Text80x25 => {
                            cpu.bus.log_string("[BIOS] Reported Video Mode: 0x02");
                            cpu.set_reg8(Register::AL, 0x02); // Current Mode 2
                            cpu.set_reg8(Register::AH, 80); // 80 Columns
                            cpu.set_reg8(Register::BH, 0); // Page 0
                        }
                        VideoMode::Text80x25Color => {
                            cpu.bus.log_string("[BIOS] Reported Video Mode: 0x03");
                            cpu.set_reg8(Register::AL, 0x03); // Current Mode 3
                            cpu.set_reg8(Register::AH, 80); // 80 Columns
                            cpu.set_reg8(Register::BH, 0); // Page 0
                        }
                        VideoMode::Graphics320x200 => {
                            cpu.bus.log_string("[BIOS] Reported Video Mode: 0x13");
                            cpu.set_reg8(Register::AL, 0x13); // Current Mode 13h
                            cpu.set_reg8(Register::AH, 40); // 40 Columns (technically)
                            cpu.set_reg8(Register::BH, 0); // Page 0
                        }
                    }
                }
                _ => cpu
                    .bus
                    .log_string(&format!("[BIOS] Unhandled INT 10h AH={:02X}", cpu.get_ah())),
            }
        }

        // INT 11h: Get Equipment List
        0x11 => {
            // Returns AX = Bitfield
            // Bit 0: Floppy installed
            // Bit 1: Math Coprocessor (FPU)
            // Bit 4-5: Video Mode (10 = 80x25 Color)
            // Bit 6-7: Floppy count (00 = 1 drive)
            // Bit 9-11: Serial Ports (000 = 0)
            // Bit 14-15: Parallel Ports (00 = 0)
            cpu.ax = 0b0000_0000_0010_0011; // 80x25 Color, FPU, Floppy
        }

        // INT 12h: Get Memory Size
        0x12 => {
            // Returns AX = Continuous memory size in KB
            cpu.ax = 640;
        }

        // INT 15h: System Services
        0x15 => {
            let ah = cpu.get_ah();
            match ah {
                // AH = 88h: Get Extended Memory Size
                // Returns AX = Contiguous KB starting at 100000h
                0x88 => {
                    // Report 16MB of XMS (plenty for most DOS games)
                    // 16MB - 1MB (Base) = 15MB = 15360 KB
                    cpu.ax = 15360;
                    cpu.set_flag(crate::cpu::FLAG_CF, false); // Success
                }

                // AH = 86h: Wait (Microseconds)
                // CX:DX = Interval in microseconds
                0x86 => {
                    let high = cpu.cx as u64;
                    let low = cpu.dx as u64;
                    let micros = (high << 16) | low;

                    // We can actually sleep here to throttle the emulator
                    std::thread::sleep(std::time::Duration::from_micros(micros));

                    cpu.set_flag(crate::cpu::FLAG_CF, false); // Success
                }

                _ => cpu
                    .bus
                    .log_string(&format!("[BIOS] Unhandled INT 15h AH={:02X}", ah)),
            }
        }

        // INT 16h: Keyboard Services (BIOS)
        0x16 => {
            let ah = cpu.get_ah();
            match ah {
                // AH = 00h: Read Key (Blocking)
                0x00 => {
                    if let Some(k) = cpu.bus.keyboard_buffer.pop_front() {
                        cpu.ax = k; // Return Scancode + ASCII
                    } else {
                        // BLOCKING: Rewind IP to retry this instruction next cycle
                        cpu.ip = cpu.ip.wrapping_sub(2);
                    }
                }
                _ => cpu
                    .bus
                    .log_string(&format!("[BIOS] Unhandled INT 16h AH={:02X}", ah)),
            }
        }

        // INT 1Ah: BIOS Time Services
        0x1A => {
            let ah = cpu.get_ah();
            match ah {
                // AH = 00h: Read System-Timer Time Counter
                // Returns: CX:DX = Ticks since midnight
                //          AL = Midnight flag (0 if midnight hasn't passed)
                0x00 => {
                    // Calculate elapsed time in milliseconds
                    let elapsed_ms = cpu.bus.start_time.elapsed().as_millis();

                    // Convert to DOS Ticks (18.2065 ticks per second)
                    // Formula: ms * 18.2 / 1000
                    // We use integer math: (ms * 182) / 10000 roughly equals ms * 0.0182
                    let ticks = (elapsed_ms as u64 * 182) / 10000;

                    // Set Registers
                    // DOS clock wraps at 24 hours (1,573,040 ticks).
                    // We just let it grow since games usually only care about the delta.

                    cpu.cx = (ticks >> 16) as u16; // High Word
                    cpu.dx = (ticks & 0xFFFF) as u16; // Low Word
                    cpu.set_reg8(Register::AL, 0); // Midnight Flag (Clear)
                }

                // AH = 02h: Get Real-Time Clock Time (CMOS)
                // Returns: CH=Hours, CL=Minutes, DH=Seconds (In BCD)
                0x02 => {
                    // Simple stub: Return 00:00:00 to prevent crashes
                    // If you want real time, you need to use chrono and convert to BCD
                    cpu.cx = 0;
                    cpu.dx = 0;
                    cpu.set_flag(crate::cpu::FLAG_CF, false); // Success
                }

                // AH = 04h: Get Real-Time Clock Date (CMOS)
                // Returns: CX=Century/Year, DX=Month/Day (In BCD)
                0x04 => {
                    // Simple stub: Return 2000-01-01
                    cpu.cx = 0x2000; // BCD for 2000
                    cpu.dx = 0x0101; // BCD for Jan 1st
                    cpu.set_flag(crate::cpu::FLAG_CF, false); // Success
                }

                _ => cpu
                    .bus
                    .log_string(&format!("[BIOS] Unhandled INT 1A AH={:02X}", ah)),
            }
        }

        // INT 20h: Custom Shell Hook (Emulator Specific)
        0x20 => {
            // Check who is calling INT 20h
            if cpu.cs != 0 {
                // CASE 1: USER PROGRAM (CS=1000, etc)
                // This is a standard DOS exit request.
                cpu.state = CpuState::RebootShell;
                return; // Stop processing immediately
            }

            // Reads command string from DS:DX
            let ds = cpu.ds;
            let dx = cpu.dx;
            let addr = cpu.get_physical_addr(ds, dx);
            let raw_cmd = read_asciiz_string(&cpu.bus, addr);

            // Process Backspaces and Control Characters in Rust
            // (Since our Assembly shell is too simple to handle editing, we do it here)
            let mut clean_chars = Vec::new();
            for c in raw_cmd.chars() {
                if c == '\x08' {
                    // Backspace
                    clean_chars.pop();
                } else if c.is_ascii_graphic() || c == ' ' {
                    clean_chars.push(c);
                }
            }
            let clean_cmd: String = clean_chars.into_iter().collect();

            cpu.bus.log_string(&format!(
                "[SHELL DEBUG] Raw: {:?} | Cleaned: {:?}",
                raw_cmd, clean_cmd
            ));

            print_string(cpu, "\r\n");

            // Split "TYPE FILE.TXT" into "TYPE" and "FILE.TXT"
            let (command, args) = match clean_cmd.split_once(' ') {
                Some((c, a)) => (c, a.trim()),
                None => (clean_cmd.as_str(), ""),
            };

            if command.eq_ignore_ascii_case("DIR") {
                run_dir_command(cpu);
            } else if command.eq_ignore_ascii_case("CLS") {
                // Clear Screen Command
                // Simply invoke BIOS Scroll Up with 0 lines to clear entire screen
                cpu.set_reg8(Register::AH, 0x06); // Scroll Up
                cpu.set_reg8(Register::AL, 0x00); // Clear Entire Window
                cpu.set_reg8(Register::BH, 0x07); // Attribute: Light Gray on Black
                cpu.set_reg8(Register::CH, 0x00); // Upper Left Row
                cpu.set_reg8(Register::CL, 0x00); // Upper Left Col
                cpu.set_reg8(Register::DH, 0x18); // Lower Right Row (24)
                cpu.set_reg8(Register::DL, 0x4F); // Lower Right Col (79)

                // Call INT 10h Handler Directly
                handle_interrupt(cpu, 0x10);

                // Reset Cursor Position to Top-Left (0,0)
                cpu.bus.write_8(0x450, 0x00); // Column
                cpu.bus.write_8(0x451, 0x00); // Row
            } else if command.eq_ignore_ascii_case("TYPE") {
                if args.is_empty() {
                    print_string(cpu, "Required parameter missing\r\n");
                } else {
                    run_type_command(cpu, args);
                }
            } else if command.eq_ignore_ascii_case("EXIT") {
                cpu.bus
                    .log_string("[SHELL] Exiting Emulator via command...");
                std::process::exit(0);
            } else if command.is_empty() {
                // No command entered, just ignore
            } else {
                // Try to run as executable
                let filename = command.to_string();

                // If no extension, try .COM, then .EXE
                if !filename.contains('.') {
                    // Try .com first (DOS convention)
                    let com_name = format!("{}.com", command);
                    if cpu.load_executable(&com_name) {
                        return;
                    }
                    // Try .exe
                    let exe_name = format!("{}.exe", command);
                    if cpu.load_executable(&exe_name) {
                        return;
                    }
                } else {
                    // User typed extension, load directly
                    if cpu.load_executable(&filename) {
                        return;
                    }
                }

                print_string(cpu, "Bad command or file name.\r\n");
            }

            // Always return to prompt
            print_string(cpu, "C:\\>");
        }

        // INT 21h: DOS Kernel API
        0x21 => {
            let ah = cpu.get_ah();
            match ah {
                // AH = 02h: Output Character (DL = Char)
                // This is frequently used for single-char output, including BEEP
                0x02 => {
                    let char_byte = cpu.get_dl();
                    if char_byte == 0x07 {
                        play_sdl_beep(&mut cpu.bus);
                    } else {
                        print_char(&mut cpu.bus, char_byte);
                    }
                    // Returns AL = Character (standard DOS behavior)
                    cpu.set_reg8(Register::AL, char_byte);
                }

                // AH = 09h: Print String (Ends in '$')
                0x09 => {
                    let mut offset = cpu.dx;
                    loop {
                        let char_byte = cpu.bus.read_8(cpu.get_physical_addr(cpu.ds, offset));
                        if char_byte == b'$' {
                            break;
                        }

                        if char_byte == 0x07 {
                            play_sdl_beep(&mut cpu.bus);
                        } else {
                            print_char(&mut cpu.bus, char_byte);
                        }

                        offset += 1;
                    }
                }

                // AH=19h: Get Current Default Drive
                0x19 => {
                    //let drive = cpu.bus.disk.get_current_drive();
                    //cpu.set_reg8(Register::AL, drive);
                    //TODO: Implement multiple drives
                    cpu.set_reg8(Register::AL, 2);
                }

                // AH=1Ah: Set Disk Transfer Area (DTA) Address
                0x1A => {
                    let ds = cpu.ds; // Segment
                    let dx = cpu.get_reg16(Register::DX); // Offset

                    cpu.bus.dta_segment = ds;
                    cpu.bus.dta_offset = dx;
                    // println!("[DOS] Set DTA to {:04X}:{:04X}", ds, dx);
                }

                // AH = 25h: Set Interrupt Vector
                // Entry: AL = Interrupt Number
                //        DS:DX = Address of new handler
                // Action: Updates the IVT at physical address 0000h + (AL * 4)
                0x25 => {
                    let int_num = cpu.get_al() as usize;
                    let new_off = cpu.dx;
                    let new_seg = cpu.ds;

                    // The IVT is located at 0x00000 in RAM.
                    // Each entry is 4 bytes: Offset (2 bytes), Segment (2 bytes).
                    let phys_addr = int_num * 4;

                    // Write Offset (Little Endian)
                    cpu.bus.write_8(phys_addr, (new_off & 0xFF) as u8);
                    cpu.bus.write_8(phys_addr + 1, (new_off >> 8) as u8);

                    // Write Segment (Little Endian)
                    cpu.bus.write_8(phys_addr + 2, (new_seg & 0xFF) as u8);
                    cpu.bus.write_8(phys_addr + 3, (new_seg >> 8) as u8);

                    cpu.bus.log_string(&format!(
                        "[DOS] Hooked Interrupt {:02X} to {:04X}:{:04X}",
                        int_num, new_seg, new_off
                    ));
                }

                // AH=2Fh: Get DTA Address
                0x2F => {
                    // Return the stored DTA address in ES:BX
                    cpu.es = cpu.bus.dta_segment;
                    cpu.set_reg16(Register::BX, cpu.bus.dta_offset);
                }

                // AH = 30h: Get DOS Version
                // Returns: AL = Major Version, AH = Minor Version
                0x30 => {
                    // Report DOS 5.0 (Standard for most games)
                    cpu.set_reg8(Register::AL, 5); // Major: 5
                    cpu.set_reg8(Register::AH, 0); // Minor: .00

                    // OEM ID Microsoft and Serial Number 0
                    cpu.bx = 0xFF00;
                    cpu.cx = 0x0000;

                    cpu.bus.log_string("[DOS] Reported DOS Version 5.0");
                }

                // AH = 35h: Get Interrupt Vector
                // Entry: AL = Interrupt Number
                // Return: ES:BX = Address of current handler
                0x35 => {
                    let int_num = cpu.get_al() as usize;
                    let phys_addr = int_num * 4;

                    // Read Offset
                    let off_low = cpu.bus.read_8(phys_addr) as u16;
                    let off_high = cpu.bus.read_8(phys_addr + 1) as u16;
                    cpu.bx = (off_high << 8) | off_low;

                    // Read Segment
                    let seg_low = cpu.bus.read_8(phys_addr + 2) as u16;
                    let seg_high = cpu.bus.read_8(phys_addr + 3) as u16;
                    cpu.es = (seg_high << 8) | seg_low;
                }

                // AH=36h: Get Disk Free Space
                0x36 => {
                    let dl = cpu.get_reg8(Register::DL);
                    match cpu.bus.disk.get_disk_free_space(dl) {
                        Ok((sectors, available, bytes_per_sec, total)) => {
                            cpu.set_reg16(Register::AX, sectors);

                            // Cap clusters to 20,000 (approx 80MB) to prevent overflow in old apps
                            // 0xFFFF clusters * 4KB = 256MB might break string formatting
                            cpu.set_reg16(Register::BX, std::cmp::min(available, 20000));

                            cpu.set_reg16(Register::CX, bytes_per_sec);
                            cpu.set_reg16(Register::DX, std::cmp::min(total, 20000));
                        }
                        Err(_) => {
                            cpu.set_reg16(Register::AX, 0xFFFF);
                        }
                    }
                }

                // AH = 3Dh: Open File
                0x3D => {
                    let addr = cpu.get_physical_addr(cpu.ds, cpu.dx);
                    let filename = read_asciiz_string(&cpu.bus, addr);
                    let mode = cpu.get_al();

                    match cpu.bus.disk.open_file(&filename, mode) {
                        Ok(handle) => {
                            cpu.ax = handle;
                            // In real CPU, clear CF (Carry Flag) here
                        }
                        Err(code) => {
                            cpu.ax = code as u16;
                            // In real CPU, set CF (Carry Flag) here
                        }
                    }
                }

                // AH = 3Eh: Close File
                0x3E => {
                    let handle = cpu.bx;
                    cpu.bus.disk.close_file(handle);
                }

                // AH = 3Fh: Read from File (or Stdin)
                // Entry: BX = Handle
                //        CX = Number of bytes to read
                //        DS:DX = Buffer address
                // Return: AX = Number of bytes read (or Error Code if CF set)
                0x3F => {
                    let handle = cpu.bx;
                    let count = cpu.cx as usize;
                    let mut buf_addr = cpu.get_physical_addr(cpu.ds, cpu.dx);

                    if handle == 0 {
                        // STDIN: Read from Keyboard Buffer
                        // DOS behavior: If buffer empty, wait? Or return 0?
                        // Simple implementation: Read whatever is in the buffer up to CX
                        let mut read_count = 0;
                        for _ in 0..count {
                            if let Some(key) = cpu.bus.keyboard_buffer.pop_front() {
                                // Only return ASCII part (low byte)
                                cpu.bus.write_8(buf_addr, (key & 0xFF) as u8);
                                buf_addr += 1;
                                read_count += 1;
                            } else {
                                break;
                            }
                        }
                        cpu.ax = read_count as u16;
                        cpu.set_flag(crate::cpu::FLAG_CF, false);
                    } else {
                        // Disk Read
                        // We assume bus.disk.read_file returns Result<Vec<u8>, u16>
                        match cpu.bus.disk.read_file(handle, count) {
                            Ok(bytes) => {
                                // Write the bytes from Host File -> Guest RAM
                                for b in &bytes {
                                    cpu.bus.write_8(buf_addr, *b);
                                    buf_addr += 1;
                                }
                                cpu.ax = bytes.len() as u16;
                                cpu.set_flag(crate::cpu::FLAG_CF, false);
                            }
                            Err(e) => {
                                cpu.ax = e; // Error code (e.g. 6 = Invalid Handle)
                                cpu.set_flag(crate::cpu::FLAG_CF, true);
                            }
                        }
                    }
                }

                // AH = 40h: Write to File (or Stdout)
                0x40 => {
                    let handle = cpu.bx;
                    let count = cpu.cx as usize;
                    let buf_addr = cpu.get_physical_addr(cpu.ds, cpu.dx);

                    // Extract data buffer from RAM
                    let mut data = Vec::with_capacity(count);
                    for i in 0..count {
                        data.push(cpu.bus.read_8(buf_addr + i));
                    }

                    if handle == 1 || handle == 2 {
                        // STDOUT / STDERR
                        // Note: Write File (40h) treats 0x07 as a raw byte usually,
                        // but DOS consoles often interpret it.
                        for &byte in &data {
                            if byte == 0x07 {
                                play_sdl_beep(&mut cpu.bus);
                            }
                        }

                        let s = String::from_utf8_lossy(&data);
                        // We filter out 0x07 for print_string to avoid visual garbage
                        let visual_s = s.replace('\x07', "");
                        print_string(cpu, &visual_s);
                        cpu.ax = count as u16;
                    } else {
                        match &mut cpu.bus.disk.write_file(handle, &data) {
                            Ok(written) => cpu.ax = *written,
                            Err(_) => cpu.ax = 0,
                        }
                    }
                }

                // AH = 42h: Move File Pointer (LSEEK)
                // Entry: BX = Handle
                //        CX:DX = Offset (Signed 32-bit)
                //        AL = Origin (0=Start, 1=Current, 2=End)
                // Return: DX:AX = New Position
                0x42 => {
                    let handle = cpu.bx;
                    let offset_high = cpu.cx as u32;
                    let offset_low = cpu.dx as u32;
                    let offset = ((offset_high << 16) | offset_low) as i32; // DOS offsets are signed
                    let whence = cpu.get_al();

                    // Map DOS Origin to Rust SeekFrom
                    // We assume bus.disk.seek_file takes (handle, offset, whence)
                    // and returns Result<u64, u16>
                    match cpu.bus.disk.seek_file(handle, offset as i64, whence) {
                        Ok(new_pos) => {
                            // Split 32-bit result into DX:AX
                            cpu.dx = ((new_pos >> 16) & 0xFFFF) as u16;
                            cpu.ax = (new_pos & 0xFFFF) as u16;
                            cpu.set_flag(crate::cpu::FLAG_CF, false);
                        }
                        Err(e) => {
                            cpu.ax = e; // Error code
                            cpu.set_flag(crate::cpu::FLAG_CF, true);
                        }
                    }
                }

                // AH=43h: Get/Set File Attributes
                0x43 => {
                    let al = cpu.get_reg8(Register::AL);
                    // AL=00 (Get), AL=01 (Set)
                    // DS:DX = Filename (ASCIIZ)

                    // TODO: Read filename from DS:DX and get/set attributes
                    // For now, assume success/archive to prevent crashing
                    if al == 0x00 {
                        cpu.set_reg16(Register::CX, 0x20); // Attribute: Archive
                        cpu.set_flag(crate::cpu::FLAG_CF, false); // Success
                    } else {
                        // Set attributes (Ignored)
                        cpu.set_flag(crate::cpu::FLAG_CF, false);
                    }
                }

                // AH=47h: Get Current Directory
                0x47 => {
                    let _dl = cpu.get_reg8(Register::DL);
                    // Drive 0=Default, 1=A, ... 3=C.
                    // Note: DOS AH=47 takes 0=Default, but we only simulate C: (drive 3 or default)
                    // We assume any query is valid for now.

                    let ds = cpu.ds;
                    let si = cpu.get_reg16(Register::SI);
                    let addr = cpu.get_physical_addr(ds, si);

                    // Clear 64-byte buffer first
                    for i in 0..64 {
                        cpu.bus.write_8(addr + i, 0x00);
                    }

                    // Return success (Root directory "")
                    cpu.set_reg16(Register::AX, 0x0100);
                    cpu.set_flag(crate::cpu::FLAG_CF, false);
                }

                // AH = 4Ah: Resize Memory Block (SETBLOCK)
                // Entry: ES = Segment of block to modify
                //        BX = New size in paragraphs (16 bytes)
                // Return: CF set on error, AX = error code
                0x4A => {
                    let requested_size = cpu.get_reg16(Register::BX);

                    // We simulate 640KB of Conventional Memory (0xA000 paragraphs)
                    // The program is loaded at 0x1000 (PSP).
                    // Available size = 0xA000 - 0x1000 = 0x9000 paragraphs.
                    let max_available = 0x9000;

                    cpu.bus.log_string(format!("[DEBUG] INT 21,4A Resize Mem: Requested {:04X} paras. Max Available: {:04X}", requested_size, max_available).as_str());

                    if requested_size > max_available {
                        // Protocol: If request is too big, FAIL and return max available in BX
                        cpu.bus.log_string("[DEBUG] -> Denied. Returning max size.");
                        cpu.set_reg16(Register::BX, max_available);
                        cpu.set_reg16(Register::AX, 0x0008); // Error: Insufficient Memory
                        cpu.set_flag(crate::cpu::FLAG_CF, true); // Set Carry Flag (Error)
                    } else {
                        // Success
                        cpu.bus.log_string("[DEBUG] -> Approved.");
                        cpu.set_flag(crate::cpu::FLAG_CF, false);
                    }
                }

                // AH = 4Ch: Terminate Program
                0x4C => {
                    cpu.bus
                        .log_string("[DOS] Program Terminated (INT 21h, 4Ch).");
                    cpu.state = CpuState::RebootShell;
                }

                // AH=4Eh: Find First File
                // AH=4Fh: Find Next File
                0x4E | 0x4F => {
                    let dta_seg = cpu.bus.dta_segment;
                    let dta_off = cpu.bus.dta_offset;
                    let dta_phys = cpu.get_physical_addr(dta_seg, dta_off);

                    cpu.bus.log_string(&format!("[DEBUG] INT 21h, {:02X} FindFirst/Next called. DTA at {:04X}:{:04X} (Phys {:#05X})", ah, dta_seg, dta_off, dta_phys));

                    // DTA Layout Constants
                    const OFFSET_ATTR_SEARCH: usize = 12; // Where DOS remembers what we are looking for
                    const OFFSET_INDEX: usize = 13; // Where DOS remembers how far we got

                    // Determine Index, Attribute, and Pattern
                    let (index, search_attr, raw_pattern) = if ah == 0x4E {
                        // FindFirst: Read Pattern from DS:DX
                        let name_addr = cpu.get_physical_addr(cpu.ds, cpu.dx);
                        let pattern = read_asciiz_string(&cpu.bus, name_addr);
                        println!("[DEBUG] FindFirst Pattern: {}", pattern);
                        (0, cpu.cx, pattern)
                    } else {
                        // FindNext: Read Index & Attr from DTA
                        let idx = cpu.bus.read_16(dta_phys + OFFSET_INDEX) as usize; 
                        let attr = cpu.bus.read_8(dta_phys + OFFSET_ATTR_SEARCH) as u16;
                        // Reconstruct the pattern from the DTA template (offsets 1-11) 
                        let pattern = read_dta_template(&cpu.bus, dta_phys);
                        println!("[DEBUG] FindNext Index: {}, Attr: {:02X}, Pattern: {}", idx, attr, pattern);
                        (idx, attr, pattern)
                        //(idx, attr, "*.*".to_string()) // DOS ignores pattern on FindNext
                    };

                    //Sanitize Pattern: Strip "C:\" or Path Separators
                    // If pattern is "C:\*.*", we want "*.*"
                    // If pattern is "\*.*", we want "*.*"
                    let search_pattern = if let Some(idx) = raw_pattern.rfind('\\') {
                        raw_pattern[idx+1..].to_string()
                    } else if let Some(idx) = raw_pattern.rfind(':') {
                        raw_pattern[idx+1..].to_string()
                    } else {
                        raw_pattern
                    };

                    match cpu.bus.disk.find_directory_entry(&search_pattern, index, search_attr) {
                        Ok(entry) => {
                            cpu.bus.log_string(&format!(
                                "[DEBUG] -> Disk returned: {}",
                                entry.filename
                            ));


                            // Setup DTA Header
                            // Write this for BOTH calls to ensure D.COM sees a valid drive byte.
                            cpu.bus.write_8(dta_phys + 0, 3); // Drive C:
                            // Persist Search Attribute (Offset 12)
                            cpu.bus.write_8(dta_phys + OFFSET_ATTR_SEARCH, search_attr as u8);

                            // Update Index
                            cpu.bus.write_16(dta_phys + OFFSET_INDEX, (index + 1) as u16);

                            // Write search_pattern into dta_phys + 1..11 for reference
                            let pattern_bytes = search_pattern.as_bytes();
                            let pattern_len = std::cmp::min(pattern_bytes.len(), 11);
//                            for i in 0..pattern_len {
//                                cpu.bus.write_8(dta_phys + 1 + i, pattern_bytes[i]);
//                            }
                            let fcb_bytes = pattern_to_fcb(&search_pattern);
                            for i in 0..11 {
                                cpu.bus.write_8(dta_phys + 1 + i, fcb_bytes[i]);
                            }
                            // Pad remaining bytes with 0
                            for i in pattern_len..11 {
                                cpu.bus.write_8(dta_phys + 1 + i, 0);
                            }


//                            for i in 0..6 { cpu.bus.write_8(dta_phys + 15 + i, 0); }
                            // Write Unique Position/Cluster ID to offsets 15-20 (6 bytes)
                            // We construct a 32-bit unique ID from the index to ensure it changes every time.
                            // DOS uses this to find the next entry.
                            let unique_id = (index as u32).wrapping_add(0x12345678);
                            // Write bytes 15-16
                            cpu.bus.write_16(dta_phys + 15, (unique_id & 0xFFFF) as u16);
                            // Write bytes 17-18
                            cpu.bus.write_16(dta_phys + 17, (unique_id >> 16) as u16);
                            // Write bytes 19-20 (Padding/Extra precision)
                            cpu.bus.write_16(dta_phys + 19, (index as u16).wrapping_mul(3));

                            // File Attributes (Offset 21)
                            let mut attr = if entry.is_dir { 0x10 } else { 0x20 };
                            if entry.filename == "RUSTDOS" {
                                attr = 0x08;
                            }
                            cpu.bus.write_8(dta_phys + 21, attr);

                            // Time/Date
                            cpu.bus.write_16(dta_phys + 22, entry.dos_time); 
                            cpu.bus.write_16(dta_phys + 24, entry.dos_date);

                            // File Size (Offset 26)
                            cpu.bus
                                .write_16(dta_phys + 26, (entry.size & 0xFFFF) as u16);
                            cpu.bus.write_16(dta_phys + 28, (entry.size >> 16) as u16);

                            // Filename (Offset 30)
                            let name_start = dta_phys + 30;
                            // Explicitly zero out the entire 13-byte buffer first.
                            for i in 0..13 {
                                cpu.bus.write_8(name_start + i, 0x00);
                            }

                            // Write the new filename
                            let name_bytes = entry.filename.as_bytes();
                            let len = std::cmp::min(name_bytes.len(), 12); // Max 12 chars (8.3)

                            for i in 0..len {
                                cpu.bus.write_8(name_start + i, name_bytes[i]);
                            }
                            // Null terminator is already there because we zeroed the buffer.

                            let verify_char = cpu.bus.read_8(name_start) as char;
                            cpu.bus.log_string(&format!("[DEBUG] Wrote '{}' to DTA. Verification read of first char: '{}'", entry.filename, verify_char));

                            // Success
                            cpu.set_reg16(Register::AX, 0);
                            cpu.set_flag(crate::cpu::FLAG_CF, false);
                        }
                        Err(code) => {
                            cpu.bus
                                .log_string("[DEBUG] -> Disk returned Error (End of List)");
                            cpu.set_reg16(Register::AX, code as u16);
                            cpu.set_flag(crate::cpu::FLAG_CF, true);
                        }
                    }
                }

                // AH = 00h: Terminate Program (Legacy Method)
                // Functionally identical to AH=4Ch for our purposes
                0x00 => {
                    cpu.bus
                        .log_string("[DOS] Program Terminated (Legacy INT 20h/21h AH=00).");
                    cpu.state = CpuState::RebootShell;
                }

                _ => cpu.bus.log_string(&format!(
                    "[DOS] Unhandled Call Int 0x21 AH={:02X}",
                    cpu.get_ah()
                )),
            }
        }

        0x28 => {
            // Idle interrupt
        }

        // INT 33h: Mouse Services
        0x33 => {
            let ax = cpu.ax;
            match ax {
                // AX = 0000h: Reset Mouse Driver
                0x0000 => {
                    // Return 0 in AX to say "No Mouse Installed"
                    // (0xFFFF = "Mouse Installed")
                    cpu.ax = 0x0000;
                    cpu.bx = 0; // Number of buttons
                }
                _ => cpu
                    .bus
                    .log_string(&format!("[MOUSE] Unhandled Call Int 0x33 AX={:04X}", ax)),
            }
        }

        0x4C => {
            cpu.bus
                .log_string("[DOS] Program Exited. Rebooting Shell...");
            cpu.state = CpuState::RebootShell;
        }

        // Catch-all
        _ => cpu
            .bus
            .log_string(&format!("[CPU] Unhandled Interrupt Vector {:02X}", vector)),
    }
}
