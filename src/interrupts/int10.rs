use iced_x86::Register;
use crate::cpu::Cpu;
use crate::video::VideoMode;
use crate::audio::play_sdl_beep;

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    match ah {
        // AH = 00h: Set Video Mode
        0x00 => {
            let mode = cpu.get_al();
            // Clear Screen
            for i in (0..4000).step_by(2) {
                cpu.bus.write_8(0xB8000 + i, 0x20);
                cpu.bus.write_8(0xB8000 + i + 1, 0x07);
            }

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
                0x13 => {
                    cpu.bus.log_string("[BIOS] Switch to Graphics Mode (320x200)");
                    cpu.bus.video_mode = VideoMode::Graphics320x200;
                }
                _ => cpu.bus.log_string(&format!("[BIOS] Unsupported Video Mode {:02X}", mode)),
            }
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
                cpu.cx = cpu.bus.read_16(0x0460);
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
            cpu.bus.log_string(&format!("[BIOS] Set Active Page to {}", page));
        }

        // AH = 06h: Scroll Up
        0x06 => scroll_window(cpu, true),

        // AH = 07h: Scroll Down
        0x07 => scroll_window(cpu, false),

        // AH = 0Eh: Teletype Output
        0x0E => {
            let char_code = cpu.get_reg8(Register::AL);
            let cursor_addr = 0x0450;
            let mut col = cpu.bus.read_8(cursor_addr);
            let mut row = cpu.bus.read_8(cursor_addr + 1);

            let max_cols = 80;
            let max_rows = 25;

            match char_code {
                0x07 => play_sdl_beep(&mut cpu.bus),
                0x0D => col = 0,
                0x0A => row += 1,
                0x08 => {
                    if col > 0 { col -= 1; }
                    let vram_offset = (row as usize * max_cols + col as usize) * 2;
                    cpu.bus.write_8(0xB8000 + vram_offset, 0x20);
                    cpu.bus.write_8(0xB8000 + vram_offset + 1, 0x07);
                }
                _ => {
                    let vram_offset = (row as usize * max_cols + col as usize) * 2;
                    if vram_offset < 4000 {
                        cpu.bus.write_8(0xB8000 + vram_offset, char_code);
                        cpu.bus.write_8(0xB8000 + vram_offset + 1, 0x07);
                    }
                    col += 1;
                    if col >= max_cols as u8 {
                        col = 0;
                        row += 1;
                    }
                }
            }

            if row >= max_rows as u8 {
                // Scroll up one line
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
                // Clear bottom row
                let last_row_start = ((max_rows - 1) * max_cols) * 2;
                for i in (0..160).step_by(2) {
                    cpu.bus.write_8(0xB8000 + last_row_start + i, 0x20);
                    cpu.bus.write_8(0xB8000 + last_row_start + i + 1, 0x07);
                }
                row = (max_rows - 1) as u8;
            }

            cpu.bus.write_8(cursor_addr, col);
            cpu.bus.write_8(cursor_addr + 1, row);
        }

        // AH = 0Fh: Get Video Mode
        0x0F => {
             match cpu.bus.video_mode {
                VideoMode::Text40x25 | VideoMode::Text40x25Color => {
                    cpu.set_reg8(Register::AL, 0x01); // Mode 1
                    cpu.set_reg8(Register::AH, 40);
                }
                VideoMode::Text80x25 | VideoMode::Text80x25Color => {
                    cpu.set_reg8(Register::AL, 0x03); // Mode 3
                    cpu.set_reg8(Register::AH, 80);
                }
                VideoMode::Graphics320x200 => {
                    cpu.set_reg8(Register::AL, 0x13); // Mode 13h
                    cpu.set_reg8(Register::AH, 40);
                }
            }
            cpu.set_reg8(Register::BH, 0); // Page 0
        }

        _ => cpu.bus.log_string(&format!("[BIOS] Unhandled INT 10h AH={:02X}", cpu.get_ah())),
    }
}

// Helper for AH=06/07
fn scroll_window(cpu: &mut Cpu, up: bool) {
    let lines = cpu.get_reg8(Register::AL);
    let attr = cpu.get_reg8(Register::BH);
    let row_start = cpu.get_reg8(Register::CH) as usize;
    let col_start = cpu.get_reg8(Register::CL) as usize;
    let row_end = cpu.get_reg8(Register::DH) as usize;
    let col_end = cpu.get_reg8(Register::DL) as usize;
    let max_cols = 80;

    if lines == 0 {
        // Clear Window
        for r in row_start..=row_end {
            for c in col_start..=col_end {
                let offset = (r * max_cols + c) * 2;
                let phys_addr = 0xB8000 + offset;
                cpu.bus.write_8(phys_addr, 0x20);
                cpu.bus.write_8(phys_addr + 1, attr);
            }
        }
        return;
    }

    if up {
        // Scroll Up (Copy Top -> Bottom)
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
        // Clear bottom
        for r in (row_end - lines as usize + 1)..=row_end {
            for c in col_start..=col_end {
                let offset = (r * max_cols + c) * 2;
                cpu.bus.write_8(0xB8000 + offset, 0x20);
                cpu.bus.write_8(0xB8000 + offset + 1, attr);
            }
        }
    } else {
        // Scroll Down (Copy Bottom -> Top)
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
        // Clear top
        let clear_limit = std::cmp::min(row_start + lines as usize, row_end + 1);
        for r in row_start..clear_limit {
            for c in col_start..=col_end {
                let offset = (r * max_cols + c) * 2;
                cpu.bus.write_8(0xB8000 + offset, 0x20);
                cpu.bus.write_8(0xB8000 + offset + 1, attr);
            }
        }
    }
}