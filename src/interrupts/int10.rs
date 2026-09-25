use crate::audio::play_sdl_beep;
use crate::cpu::Cpu;
use crate::video::adapter::Adapter;
use crate::video::bios::{self as video_bios, rom_pointer};
use crate::video::{BDA_CURSOR_MODE, BDA_CURSOR_POS, MAX_COLS, VideoMode, pixels};
use iced_x86::Register;

/// Current number of text rows on the screen, read from BDA 0x0484.
/// Programs that load an 8x8 or 8x14 font via INT 10h AH=11h AL=12h change
/// this value to 43 or 50 rows; hard-coding 25 would make the renderer and
/// scroll logic ignore everything below the first 25 rows.
fn active_rows(cpu: &Cpu) -> u8 {
    cpu.bus.text_rows() as u8
}

/// The bytes of video memory a page of `mode` takes (BDA 044Ch).
fn page_size(mode: u8) -> u16 {
    match mode {
        0x00 | 0x01 => 0x0800,
        0x02 | 0x03 | 0x07 => 0x1000,
        0x04..=0x06 | 0x0E => 0x4000,
        // The Tandy's and PCjr's 16-colour and 640x200x4 modes, as DOSBox
        // has their BIOS keep them.
        0x08..=0x0A | 0x0D => 0x2000,
        0x0F | 0x10 => 0x8000,
        0x11 | 0x12 => 0xA000,
        _ => 0xFA00,
    }
}

/// The values of the CGA's Mode Control register (3D8h) the IBM BIOS sets
/// for modes 0-7, which it keeps in BDA 0465h.
const MODE_CONTROL: [u8; 8] = [0x2C, 0x28, 0x2D, 0x29, 0x2A, 0x2E, 0x1E, 0x29];

/// Whether the current mode is a text mode.
fn text_mode(cpu: &Cpu) -> bool {
    matches!(
        cpu.bus.video_mode,
        VideoMode::Text80x25
            | VideoMode::Text80x25Color
            | VideoMode::Text40x25
            | VideoMode::Text40x25Color
            | VideoMode::Mono80x25
    )
}

/// The characters in a row of the current mode.
fn text_cols(cpu: &Cpu) -> usize {
    match cpu.bus.video_mode {
        VideoMode::Text40x25 | VideoMode::Text40x25Color => 40,
        _ if pixels::graphics_mode(&cpu.bus) => pixels::cells(&cpu.bus).0,
        _ => 80,
    }
}

/// The VGA's display combination code (INT 10h AH=1Ah): 08h with a colour
/// monitor, 07h with a monochrome one.
fn display_combination(cpu: &Cpu) -> u8 {
    if cpu.bus.vga.mono_monitor { 0x07 } else { 0x08 }
}

/// Point an interrupt vector (INT 1Fh, 43h) at the far pointer `pointer`.
fn set_vector(cpu: &mut Cpu, vector: usize, pointer: u32) {
    cpu.bus.write_16(vector * 4, pointer as u16);
    cpu.bus.write_16(vector * 4 + 2, (pointer >> 16) as u16);
}

fn get_vector(cpu: &Cpu, vector: usize) -> (u16, u16) {
    (cpu.bus.read_16(vector * 4 + 2), cpu.bus.read_16(vector * 4))
}

/// The scanlines the text modes have, over which a font's rows go: the
/// VGA's 400, the EGA's 350.
fn text_scanlines(cpu: &Cpu) -> u16 {
    match cpu.bus.vga.adapter {
        Adapter::Ega => 350,
        _ => 400,
    }
}

/// Take a font of `height` for the text mode: as many rows as fit in the
/// scanlines, and a cursor at the bottom of the cell.
fn text_font(cpu: &mut Cpu, height: u16) {
    let rows = text_scanlines(cpu) / height;
    cpu.bus.write_8(0x0484, (rows - 1) as u8);
    cpu.bus.write_16(0x0485, height);
    let cursor = match height {
        8 => 0x0607,
        14 => 0x0B0C,
        _ => 0x0D0E,
    };
    cpu.bus.write_16(0x0460, cursor);
    cpu.bus.vga.crtc_regs[0x09] = (cpu.bus.vga.crtc_regs[0x09] & 0xE0) | (height - 1) as u8;
    cpu.bus.vga.mark_dirty_full();
}

/// Take a graphics font (INT 43h) of `height`, with the rows BL says: 0
/// DL of them, 1 14, 2 25, 3 43.
fn graphics_font(cpu: &mut Cpu, pointer: u32, height: u16) {
    set_vector(cpu, 0x43, pointer);
    cpu.bus.write_16(0x0485, height);
    let rows = match cpu.get_reg8(Register::BL) {
        0 => cpu.get_reg8(Register::DL),
        1 => 14,
        2 => 25,
        3 => 43,
        _ => return,
    };
    cpu.bus.write_8(0x0484, rows.saturating_sub(1));
}

/// The address of the character at (`col`, `row`) of text page `page`,
/// each page `page_size` (BDA 044Ch) bytes on from the last.
fn cell_addr(cpu: &Cpu, page: u8, col: usize, row: usize) -> usize {
    let (base, _, wrap) = cpu.bus.vga.text_window();
    let page_offset = page as usize * cpu.bus.read_16(0x044C) as usize;
    base + ((page_offset + (row * text_cols(cpu) + col) * 2) & wrap)
}

/// The active display page (BDA 0462h).
fn active_page(cpu: &Cpu) -> u8 {
    cpu.bus.read_8(0x0462)
}

/// INT 10h AH=00h: set the standard video mode AL (bit 7: keep the
/// contents of video memory). This also leaves a VESA mode.
pub fn set_mode(cpu: &mut Cpu, al: u8) {
    let keep = al & 0x80 != 0;
    // A monochrome adapter only has mode 7, whatever a program asks for.
    let mode = if cpu.bus.vga.adapter.mono_only() { 0x07 } else { al & 0x7F };
    // A mode the adapter doesn't have leaves the one it is in.
    if !cpu.bus.vga.setup().supports_mode(mode) {
        cpu.bus.log_string(&format!("[BIOS] Video mode {:02X} isn't on this adapter", mode));
        return;
    }
    cpu.bus.vbe.reset();
    let adapter = cpu.bus.vga.adapter;

    // Reset Cursor
    set_cursor(cpu, 0, 0, 0);

    let new_mode = match mode {
        0x00 => Some((VideoMode::Text40x25, "Text Mode (40x25)")),
        0x01 => Some((VideoMode::Text40x25Color, "Text Mode (40x25 Color)")),
        0x02 => Some((VideoMode::Text80x25, "Text Mode (80x25)")),
        0x03 => Some((VideoMode::Text80x25Color, "Text Mode (80x25 Color)")),
        0x04 => Some((VideoMode::Cga320x200Color, "CGA Graphics Mode (320x200 Color)")),
        0x05 => Some((VideoMode::Cga320x200, "CGA Graphics Mode (320x200)")),
        0x06 => Some((VideoMode::Cga640x200, "CGA Graphics Mode (640x200)")),
        0x07 => Some((VideoMode::Mono80x25, "Monochrome Text Mode (80x25)")),
        0x08 => Some((VideoMode::Tandy160x200x16, "Tandy/PCjr Graphics Mode (160x200 16-color)")),
        0x09 => Some((VideoMode::Tandy320x200x16, "Tandy/PCjr Graphics Mode (320x200 16-color)")),
        0x0A => Some((VideoMode::Tandy640x200x4, "Tandy/PCjr Graphics Mode (640x200 4-color)")),
        0x0D => Some((VideoMode::Ega320x200, "EGA Graphics Mode (320x200 16-color)")),
        0x0E => Some((VideoMode::Ega640x200, "EGA Graphics Mode (640x200 16-color)")),
        0x0F => Some((VideoMode::Ega640x350Mono, "EGA Graphics Mode (640x350 monochrome)")),
        0x10 => Some((VideoMode::Ega640x350, "EGA Graphics Mode (640x350 16-color)")),
        0x11 => Some((VideoMode::Vga640x480Mono, "VGA Graphics Mode (640x480 2-color)")),
        0x12 => Some((VideoMode::Vga640x480, "VGA Graphics Mode (640x480 16-color)")),
        0x13 => Some((VideoMode::Graphics320x200, "Graphics Mode (320x200)")),
        _ => None,
    };
    match new_mode {
        Some((new_mode, name)) => {
            cpu.bus.log_string(&format!("[BIOS] Switch to {}", name));
            cpu.bus.video_mode = new_mode;
            cpu.bus.vga.set_video_mode(new_mode);
        }
        None => {
            cpu.bus.log_string(&format!("[BIOS] Unsupported Video Mode {:02X}", mode));
            // Out of a VESA mode, at least into one there is.
            if cpu.bus.video_mode == VideoMode::Vesa {
                cpu.bus.video_mode = VideoMode::Text80x25Color;
                cpu.bus.vga.set_video_mode(VideoMode::Text80x25Color);
            }
        }
    }

    // Clear the mode's memory, unless AL bit 7 asks to keep it: spaces in
    // light grey in the text modes, 0 in the graphics modes. The Tandy's
    // and PCjr's is the system memory the picture comes from.
    if !keep && adapter.gate_array() {
        let (base, size) = cpu.bus.vga.crt_range();
        if mode <= 0x03 {
            for addr in (base..base + size).step_by(2) {
                cpu.bus.write_16(addr, 0x0720);
            }
        } else {
            cpu.bus.fill_ram(base..base + size, 0);
        }
    } else if !keep {
        let vga = &mut cpu.bus.vga;
        match mode {
            0x00..=0x03 | 0x07 => {
                for cell in vga.vram_text[..0x8000].chunks_mut(2) {
                    cell.copy_from_slice(&[0x20, 0x07]);
                }
            }
            0x04..=0x06 => vga.vram_text[..0x4000].fill(0),
            _ => vga.vram_graphics.fill(0),
        }
    }

    cpu.bus.vga.mark_dirty_full();
    cpu.bus.write_8(0x0449, cpu.bus.video_mode as u8); // Update BDA Current Video Mode
    cpu.bus.write_8(0x0462, 0); // Update BDA Active Page to 0
    cpu.bus.write_16(0x044C, page_size(mode));
    cpu.bus.write_16(0x044E, 0);
    // The CGA's mode and colour registers as its BIOS sets them: palette 1
    // at high intensity, and in mode 6 white on black.
    if let Some(&control) = MODE_CONTROL.get(mode as usize) {
        cpu.bus.write_8(0x0465, control);
    }
    cpu.bus.write_8(0x0466, if mode == 0x06 { 0x3F } else { 0x30 });
    if adapter.gate_array() {
        // The Tandy's and PCjr's registers as the mode set them: Mode
        // Control, Color Select and the CRT/processor page register.
        cpu.bus.write_8(0x0465, cpu.bus.vga.cga_mode);
        cpu.bus.write_8(0x0466, cpu.bus.vga.cga_color);
        cpu.bus.write_8(0x048A, cpu.bus.vga.tandy.page);
    }
    let cols: u16 = match mode {
        0x08 => 20,
        0x00 | 0x01 | 0x04 | 0x05 | 0x09 => 40,
        0x13 => 40, // Mode 13h uses 40 columns text
        _ => 80,
    };
    cpu.bus.write_16(0x044A, cols);

    // Update BDA 0x0484 (Rows on Screen minus 1) and 0x0485 (char height).
    // Mode set always resets the cell size to the mode's default.
    let (rows, char_height): (u8, u16) = match mode {
        // Text modes: 25 rows of the 8x14 font on an EGA, 8x16 on a VGA.
        0x00..=0x03 | 0x07 if cpu.bus.vga.adapter == Adapter::Ega => (24, 14),
        0x00..=0x03 | 0x07 => (24, 16),
        // CGA 40-col graphics counts as 25 rows.
        0x04 | 0x05 => (24, 8),
        // CGA 640x200 2-color.
        0x06 => (24, 8),
        // EGA/VGA planar modes and 13h: treat as 25-row equivalents.
        0x0D | 0x0E => (24, 8),
        0x0F | 0x10 => (24, 14),
        0x11 | 0x12 => (29, 16), // 30 rows at 640x480
        0x13 => (24, 8),
        _ => (24, 16),
    };
    if mode <= 0x03 || mode == 0x07 {
        let height = if adapter.mono_only() { 14 } else { char_height };
        cpu.bus.write_16(0x0460, video_bios::cursor_shape(adapter, height));
    }
    // A monochrome monitor's VGA sums the colours it loads to grey.
    if video_bios::gray_summing(&cpu.bus) {
        cpu.bus.vga.sum_to_gray(0..256);
    }
    // The CGA's BIOS keeps neither these nor the graphics font.
    if adapter.ega_bios() {
        cpu.bus.write_8(0x0484, rows);
        cpu.bus.write_16(0x0485, char_height);
        // The graphics modes draw characters with the font INT 43h points to.
        let (font, _) = video_bios::graphics_font(mode);
        set_vector(cpu, 0x43, rom_pointer(font));
    }
}

/// INT 10h AH=05h AL=80h-83h on a Tandy or PCjr: 80h reads the page the
/// picture comes from into BH and the one the processor sees into BL, 81h
/// sets the processor's, 82h the picture's, 83h both. The PCjr returns
/// them all the same.
fn gate_array_pages(cpu: &mut Cpu) {
    let (al, bh, bl) = (cpu.get_al(), cpu.get_reg8(Register::BH), cpu.get_reg8(Register::BL));
    let mut page = cpu.bus.read_8(0x048A);
    match al {
        0x80 => {
            cpu.set_reg8(Register::BH, page & 7);
            cpu.set_reg8(Register::BL, (page >> 3) & 7);
        }
        0x81 => page = (page & 0xC7) | (bl & 7) << 3,
        0x82 => page = (page & 0xF8) | (bh & 7),
        0x83 => page = (page & 0xC0) | (bh & 7) | (bl & 7) << 3,
        _ => {}
    }
    if cpu.bus.vga.adapter == Adapter::Pcjr {
        cpu.set_reg8(Register::BH, page & 7);
        cpu.set_reg8(Register::BL, (page >> 3) & 7);
    }
    cpu.bus.io_write(0x3DF, page);
    cpu.bus.write_8(0x048A, page);
}

/// INT 10h AH=0Bh on a Tandy or PCjr: the Tandy has the CGA's Color Select
/// register, and in its 16-colour modes also takes the background into
/// palette register 0; the PCjr sets its palette registers: the background
/// (and the border), and mode 4's three colours or mode 6's one.
fn gate_array_color_select(cpu: &mut Cpu, bh: u8, bl: u8, select: u8) {
    let mode = cpu.bus.read_8(0x0449);
    let graphics = !matches!(mode, 0x00..=0x03);
    let tandy = cpu.bus.vga.adapter == Adapter::Tandy;
    if tandy {
        cpu.bus.io_write(0x3D9, select);
    }
    let ga = &mut cpu.bus.vga.tandy;
    match bh {
        0x00 => {
            if !tandy || matches!(mode, 0x08 | 0x09) {
                ga.border = select & 0x0F;
                if graphics {
                    ga.palette[0] = select & 0x0F;
                }
            }
        }
        _ if tandy => {}
        _ => match mode {
            0x04 | 0x05 => {
                let colors = if bl & 1 != 0 { [0x03, 0x05, 0x0F] } else { [0x02, 0x04, 0x06] };
                ga.palette[1..4].copy_from_slice(&colors);
            }
            0x06 => ga.palette[1] = if bl & 1 != 0 { 0x0F } else { 0 },
            _ => {
                for (i, p) in ga.palette.iter_mut().enumerate().skip(1) {
                    *p = i as u8;
                }
            }
        },
    }
    cpu.bus.vga.mark_dirty_full();
}

/// INT 10h AH=10h AL=00h-02h on a Tandy or PCjr: a palette register, the
/// border, or all of them from ES:DX. The Tandy's 320x200 CGA modes take
/// their colours from the bright registers the Color Select register
/// picks, which AL=00h sets for colours 1-3; in 640x200x2 colour 1 is the
/// Color Select register's.
fn gate_array_palette_function(cpu: &mut Cpu) {
    let (al, bh, bl) = (cpu.get_al(), cpu.get_reg8(Register::BH), cpu.get_reg8(Register::BL));
    let tandy = cpu.bus.vga.adapter == Adapter::Tandy;
    let mode = cpu.bus.read_8(0x0449);
    match al {
        0x00 => {
            let reg = match (tandy, mode, bl & 0x0F) {
                (true, 0x04 | 0x05, reg @ 1..=3) => {
                    reg * 2 + 8 + (cpu.bus.read_8(0x0466) >> 5 & 1)
                }
                (true, 0x06, 1) => cpu.bus.vga.cga_color & 0x0F,
                (_, _, reg) => reg,
            };
            cpu.bus.vga.tandy.palette[reg as usize & 0x0F] = bh & 0x0F;
        }
        0x01 => cpu.bus.vga.tandy.border = bh,
        _ => {
            let addr = cpu.get_physical_addr(cpu.es(), cpu.dx());
            for i in 0..16 {
                cpu.bus.vga.tandy.palette[i] = cpu.bus.read_8(addr + i) & 0x0F;
            }
            cpu.bus.vga.tandy.border = cpu.bus.read_8(addr + 16);
        }
    }
    cpu.bus.vga.mark_dirty_full();
}

pub fn handle(cpu: &mut Cpu) {
    let ah = cpu.get_ah();
    let adapter = cpu.bus.vga.adapter;

    // What the adapter's BIOS doesn't have returns with the registers as
    // they were, which is how programs tell the adapters apart: the VESA
    // extensions (AH=4Fh), the VGA's display combination code and state
    // information (AH=1Ah-1Ch), its DAC functions (AH=10h AL=07h and up)
    // and its scanline and other options (AH=12h BL=30h-36h).
    let vga_only = match ah {
        0x1A..=0x1C => true,
        0x10 => cpu.get_al() >= 0x07,
        0x12 => (0x30..=0x36).contains(&cpu.get_reg8(Register::BL)),
        _ => false,
    };
    // Before them, the EGA's palette registers, character generator and
    // configuration (AH=10h-12h), which a CGA's BIOS hasn't either.
    let ega_only = matches!(ah, 0x10..=0x12);
    // The Tandy's and PCjr's BIOS has AH=10h's palette registers and border
    // (AL=00h-02h), for their gate array.
    let gate_array_palette = ah == 0x10 && adapter.gate_array() && cpu.get_al() <= 0x02;
    if (vga_only && !adapter.vga_bios())
        || (ega_only && !adapter.ega_bios() && !gate_array_palette)
        || (ah == 0x4F && !adapter.has_vbe())
    {
        return;
    }

    match ah {
        // AH = 00h: Set Video Mode
        0x00 => set_mode(cpu, cpu.get_al()),

        // AH = 01h: Set Cursor Type
        0x01 => {
            let cx = cpu.cx();
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
            cpu.set_cx(0);
            cpu.set_dx(0);
        }

        // AH = 05h: Set Active Page. The CRTC shows the page from its
        // Start Address on, which counts characters in the text modes and
        // bytes of each plane in the 16-color ones.
        // AL=80h-83h on a Tandy or PCjr: read or set the pages the picture
        // comes from (BH) and the processor sees at B8000h (BL).
        0x05 if adapter.gate_array() && cpu.get_al() & 0x80 != 0 => gate_array_pages(cpu),
        0x05 => {
            let page = cpu.get_reg8(Register::AL);
            let offset = page as usize * cpu.bus.read_16(0x044C) as usize;
            cpu.bus.write_8(0x0462, page);
            cpu.bus.write_16(0x044E, offset as u16);
            let start = if text_mode(cpu) { offset / 2 } else { offset };
            cpu.bus.vga.crtc_regs[0x0C] = (start >> 8) as u8;
            cpu.bus.vga.crtc_regs[0x0D] = start as u8;
            let (col, row) = get_cursor(cpu, page);
            set_cursor(cpu, col, row, page);
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
            let (char_code, attr) = if pixels::graphics_mode(&cpu.bus) {
                (pixels::read_char(&cpu.bus, col as usize, row as usize), 0)
            } else {
                read_char_at(cpu, col, row, page)
            };
            cpu.set_reg8(Register::AH, attr);
            cpu.set_reg8(Register::AL, char_code);
        }

        // AH = 0Ah: Write Character at Cursor Position, keeping the
        // attributes (in graphics modes in the colour BL).
        // AL = Char, BH = Page, BL = Color, CX = Count
        0x0A => {
            let char_code = cpu.get_al();
            let page = cpu.get_reg8(Register::BH);
            let color = cpu.get_reg8(Register::BL);
            let (col, row) = get_cursor(cpu, page);
            let cols = text_cols(cpu);
            for i in 0..cpu.cx() as usize {
                let (c, r) = ((col as usize + i) % cols, row as usize + (col as usize + i) / cols);
                if r < active_rows(cpu) as usize {
                    let attr = if pixels::graphics_mode(&cpu.bus) {
                        color
                    } else {
                        read_page_char(cpu, page, c as u8, r as u8).1
                    };
                    write_page_char(cpu, page, c as u8, r as u8, char_code, attr);
                }
            }
        }

        // AH = 09h: Write Character and Attribute at Cursor Position
        // AL = Char, BH = Page, BL = Attribute, CX = Count
        0x09 => {
            let char_code = cpu.get_al();
            let page = cpu.get_reg8(Register::BH);
            let attr = cpu.get_reg8(Register::BL);
            let count = cpu.cx() as usize;

            let (col, row) = get_cursor(cpu, page);

            // Repeat char count times (without moving cursor)
            let cols = text_cols(cpu);
            for i in 0..count {
                // Determine VRAM offset
                // Note: DOS wraps to next line visually for this function, but doesn't scroll
                let temp_col = (col as usize + i) % cols;
                let temp_row = (row as usize) + (col as usize + i) / cols;

                if temp_row < active_rows(cpu) as usize {
                    write_page_char(cpu, page, temp_col as u8, temp_row as u8, char_code, attr);
                }
            }
        }

        // AH = 0Bh: Set Color Palette / Background Color
        // BH = 00h: Set Background/Border Color
        //      BL = Color Value (0-15 for Border, 0-31 for CGA Background)
        // BH = 01h: Set Palette (CGA 320x200 Mode 4/5 only)
        //      BL = Palette ID (0 or 1)
        // BDA 0466h keeps the CGA's Color Select register (3D9h): the
        // background in bits 0-3, the intensity of mode 4's colors in bit 4
        // and its palette in bit 5. An EGA or VGA sets its palette registers
        // to match: the border (11h), and in the CGA graphics modes the
        // background (0) and mode 4's three colors (1-3).
        0x0B => {
            let bh = cpu.get_reg8(Register::BH);
            let bl = cpu.get_reg8(Register::BL);
            let mut select = cpu.bus.read_8(0x0466);
            match bh {
                0x00 => select = (select & 0xE0) | (bl & 0x1F),
                0x01 => select = (select & 0xDF) | if bl & 1 != 0 { 0x20 } else { 0 },
                _ => return,
            }
            cpu.bus.write_8(0x0466, select);
            if cpu.bus.vga.adapter.gate_array() {
                gate_array_color_select(cpu, bh, bl, select);
                return;
            }
            if !cpu.bus.vga.adapter.ega_bios() {
                // A CGA has the register itself.
                cpu.bus.io_write(0x3D9, select);
                return;
            }
            let graphics = cpu.bus.read_8(0x0449) > 3;
            let regs = &mut cpu.bus.vga.attribute_regs;
            if bh == 0x00 {
                // An RGBI color: intensity in bit 4 of a palette register.
                let color = (bl & 0x07) | (bl << 1 & 0x10);
                regs[0x11] = color;
                if graphics {
                    regs[0x00] = color;
                }
            }
            if graphics {
                let first = (select & 0x10) | 0x02 | (select >> 5 & 1);
                for (i, reg) in regs[1..=3].iter_mut().enumerate() {
                    *reg = first + 2 * i as u8;
                }
            }
            cpu.bus.vga.mark_dirty_full();
        }

        // AH = 0Eh: Teletype Output
        0x0E => {
            let char_code = cpu.get_reg8(Register::AL);
            // On the active page, as the IBM BIOS does whatever BH says.
            // In graphics modes BL is the character's colour.
            let page = active_page(cpu);
            let (mut col, mut row) = get_cursor(cpu, page);
            let cols = text_cols(cpu) as u8;
            let graphics = pixels::graphics_mode(&cpu.bus);
            let color = cpu.get_reg8(Register::BL);

            match char_code {
                0x07 => play_sdl_beep(&mut cpu.bus), // Bell
                0x08 => {
                    // Backspace
                    if col > 0 {
                        col -= 1;
                        // Visual erase
                        write_page_char(cpu, page, col, row, 0x20, 0x07);
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
                    // Printable, in the attribute the cell has.
                    let attr = match read_page_char(cpu, page, col, row).1 {
                        _ if graphics => color,
                        0 => 0x07,
                        attr => attr,
                    };
                    write_page_char(cpu, page, col, row, char_code, attr);
                    col += 1;
                }
            }

            // Handle Line Wrapping
            if col >= cols {
                col = 0;
                row += 1;
            }

            // Handle Scrolling
            let rows = active_rows(cpu);
            if row >= rows {
                // Scroll entire screen up by 1 line, keeping the bottom
                // line's attribute (colour 0 in graphics modes).
                let fill = if graphics { 0 } else { read_page_char(cpu, page, col.min(cols - 1), rows - 1).1 };
                let fill = if fill == 0 && !graphics { 0x07 } else { fill };
                scroll_area(cpu, true, 1, fill, 0, 0, rows - 1, cols - 1);
                row = rows - 1;
            }

            // Update Cursor (Sync BDA and Internal)
            set_cursor(cpu, col, row, page);
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
        0x10 if adapter.gate_array() => gate_array_palette_function(cpu),
        0x10 => {
            let al = cpu.get_al();
            match al {
                0x00 => {
                    // Set Single Palette Register
                    // BL = Palette Register (0-15)
                    // BH = Color Value
                    let reg = (cpu.bx() & 0xFF) as u8 & 0x0F;
                    let val = (cpu.bx() >> 8) as u8;
                    cpu.bus.vga.attribute_regs[reg as usize] = val;
                    cpu.bus.vga.mark_dirty_full();
                }
                0x01 => {
                    // Set Overscan (Border) Color
                    let val = (cpu.bx() >> 8) as u8; // BH
                    cpu.bus.vga.attribute_regs[0x11] = val;
                    cpu.bus.vga.mark_dirty_full();
                }
                0x02 => {
                    // Set All Palette Registers + Overscan
                    // ES:DX points to 17 byte table (0-15 + Overscan)
                    let es = cpu.es();
                    let dx = cpu.dx();
                    let addr = cpu.get_physical_addr(es, dx);

                    for i in 0..16 {
                        let val = cpu.bus.read_8(addr + i);
                        cpu.bus.vga.attribute_regs[i as usize] = val;
                    }
                    let border = cpu.bus.read_8(addr + 16);
                    cpu.bus.vga.attribute_regs[0x11] = border;
                    cpu.bus.vga.mark_dirty_full();
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
                    cpu.bus.vga.mark_dirty_full();
                }
                0x07 => {
                    // Read Individual Palette Register
                    // BL = Register
                    // Return: BH = Value
                    let reg = (cpu.bx() & 0xFF) as u8 & 0x0F;
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
                    let es = cpu.es();
                    let dx = cpu.dx();
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
                    let idx = (cpu.bx() & 0xFF) as usize;
                    let mask = cpu.bus.vga.dac_value_mask();
                    let r = (cpu.dx() >> 8) as u8 & mask; // DH
                    let g = (cpu.cx() >> 8) as u8 & mask; // CH
                    let b = (cpu.cx() & 0xFF) as u8 & mask; // CL

                    let base = idx * 3;
                    if base + 2 < cpu.bus.vga.palette.len() {
                        cpu.bus.vga.palette[base] = r;
                        cpu.bus.vga.palette[base + 1] = g;
                        cpu.bus.vga.palette[base + 2] = b;
                        cpu.bus.vga.mark_dirty_full();
                    }
                    if video_bios::gray_summing(&cpu.bus) {
                        cpu.bus.vga.sum_to_gray(idx..idx + 1);
                    }
                }
                0x12 => {
                    // Set Block of DAC Registers
                    // BX = Starting register, CX = Count
                    // ES:DX -> table of (R,G,B) triplets (6-bit values each)
                    let start = (cpu.bx() & 0xFF) as usize;
                    let count = cpu.cx() as usize;
                    let es = cpu.es();
                    let dx = cpu.dx();
                    let addr = cpu.get_physical_addr(es, dx);

                    let mask = cpu.bus.vga.dac_value_mask();
                    for i in 0..count {
                        let base = (start + i) * 3;
                        if base + 2 >= cpu.bus.vga.palette.len() {
                            break;
                        }
                        let src = addr + i * 3;
                        cpu.bus.vga.palette[base] = cpu.bus.read_8(src) & mask;
                        cpu.bus.vga.palette[base + 1] = cpu.bus.read_8(src + 1) & mask;
                        cpu.bus.vga.palette[base + 2] = cpu.bus.read_8(src + 2) & mask;
                    }
                    cpu.bus.vga.mark_dirty_full();
                    if video_bios::gray_summing(&cpu.bus) {
                        cpu.bus.vga.sum_to_gray(start..start + count);
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
                    cpu.bus.vga.mark_dirty_full();
                }
                0x15 => {
                    // Read Individual DAC Register
                    // BX = Register
                    // Return: DH=Red, CH=Green, CL=Blue
                    let idx = (cpu.bx() & 0xFF) as usize;
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
                    let start = (cpu.bx() & 0xFF) as usize;
                    let count = cpu.cx() as usize;
                    let es = cpu.es();
                    let dx = cpu.dx();
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
                    cpu.bus.vga.mark_dirty_full();
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
                    let start = (cpu.bx() & 0xFF) as usize;
                    let count = cpu.cx() as usize;
                    cpu.bus.vga.sum_to_gray(start..start + count);
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
                // AL=10h/11h/12h/14h: load a font and take its height: as
                // many rows as fit in the text mode's scanlines (on a VGA's
                // 400: 25 of 16, 28 of 14, 50 of 8). The text renderer draws
                // with the ROM font of that height; there is no font RAM
                // for a program's own glyphs (AL=10h: BH bytes a glyph).
                0x10 | 0x11 | 0x12 | 0x14 => {
                    let height = match al {
                        0x10 => (cpu.get_reg8(Register::BH) as u16).clamp(1, 32),
                        0x11 => 14,
                        0x12 => 8,
                        _ => 16,
                    };
                    if text_mode(cpu) {
                        text_font(cpu, height);
                    }
                }
                // AL=00h-04h load glyphs into the character generator
                // without changing the rows, and set its block specifier.
                0x00..=0x04 => {}
                // AL=20h: the second half of the 8x8 font for the CGA
                // graphics modes (INT 1Fh) is at ES:BP.
                0x20 => {
                    let pointer = (cpu.es() as u32) << 16 | cpu.bp() as u32;
                    set_vector(cpu, 0x1F, pointer);
                }
                // AL=21h-24h: the graphics font (INT 43h): a program's own at
                // ES:BP with CX bytes a glyph, or the ROM's 8x14, 8x8 and
                // 8x16; BL gives the rows (see `graphics_font`).
                0x21 => {
                    let pointer = (cpu.es() as u32) << 16 | cpu.bp() as u32;
                    graphics_font(cpu, pointer, cpu.cx());
                }
                0x22 => graphics_font(cpu, rom_pointer(video_bios::FONT_8X14), 14),
                0x23 => graphics_font(cpu, rom_pointer(video_bios::FONT_8X8), 8),
                0x24 => graphics_font(cpu, rom_pointer(video_bios::FONT_8X16), 16),
                0x30 => {
                    // Get Font Information
                    // Returns:
                    //   ES:BP -> pointer to the requested font (selected by BH)
                    //   CX    = the height of the font on the screen (BDA
                    //           0485h), whichever font was asked for
                    //   DL    = CURRENT character rows on screen - 1
                    //           (NOT a property of the queried font — programs
                    //           like Norton Commander use DL as the authoritative
                    //           row count for the current mode. Returning the
                    //           queried font's implied row count here would make
                    //           NC draw its UI scaled to 50 rows even in 80x25.)
                    //
                    // BH = 0: Int 1Fh pointer (8x8, second half)
                    //      1: Int 43h pointer (the graphics font)
                    //      2: ROM 8x14 font
                    //      3: ROM 8x8 font (lo)
                    //      4: ROM 8x8 font (hi)
                    //      5: ROM 9x14 alternate font
                    //      6: ROM 8x16 font (VGA)
                    //      7: ROM 9x16 alternate font (VGA)
                    let current_rows_minus_1 = cpu.bus.read_8(0x0484);
                    cpu.set_reg8(Register::DL, current_rows_minus_1);
                    cpu.set_cx(cpu.bus.read_16(0x0485));
                    let rom = |offset| (video_bios::ROM_SEGMENT, offset);
                    let (segment, offset) = match cpu.get_reg8(Register::BH) {
                        0x00 => get_vector(cpu, 0x1F),
                        0x01 => get_vector(cpu, 0x43),
                        0x02 => rom(video_bios::FONT_8X14),
                        0x03 => rom(video_bios::FONT_8X8),
                        0x04 => rom(video_bios::FONT_8X8_HIGH),
                        0x05 => rom(video_bios::FONT_9X14),
                        0x07 => rom(video_bios::FONT_9X16),
                        _ => rom(video_bios::FONT_8X16),
                    };
                    cpu.set_es(segment);
                    cpu.set_bp(offset);
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
                    // Get Configuration: BH 0 for a color CRTC at 3D4h, BL
                    // the memory (3: 256 KB), CL the switches and CH the
                    // feature bits, from BDA 0488h.
                    let switches = cpu.bus.read_8(0x0488);
                    let mono = cpu.bus.read_16(0x0463) == 0x3B4;
                    cpu.set_reg8(Register::BH, mono as u8);
                    cpu.set_reg8(Register::BL, 3);
                    cpu.set_reg8(Register::CL, switches & 0x0F);
                    cpu.set_reg8(Register::CH, switches >> 4);
                }
                0x30 => {
                    // Select Scan Lines (AL = 0, 1, 2)
                    // We just acknowledge it
                    cpu.set_reg8(Register::AL, 0x12);
                }
                0x33 => {
                    // Gray-Scale Summing: AL 0 on, 1 off, for the palettes
                    // the BIOS loads (BDA 0489h bit 1).
                    let flags = cpu.bus.read_8(0x0489);
                    let flags = if cpu.get_al() == 0 { flags | 0x02 } else { flags & !0x02 };
                    cpu.bus.write_8(0x0489, flags);
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
            let count = cpu.cx(); // CX is loop count
            let page = cpu.get_reg8(Register::BH);
            let attr = cpu.get_reg8(Register::BL);
            let start_row = cpu.get_reg8(Register::DH);
            let start_col = cpu.get_reg8(Register::DL);

            // Pointers
            let es = cpu.es();
            let bp = cpu.bp();

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
                // BL = Active Display (08 = VGA w/ Color Analog, 07 = VGA
                // w/ Monochrome Analog)
                // BH = Inactive Display (00 = None)
                cpu.set_reg8(Register::AL, 0x1A); // Function Supported
                cpu.set_reg8(Register::BL, display_combination(cpu));
                cpu.set_reg8(Register::BH, 0x00);
            } else {
                cpu.bus
                    .log_string(&format!("[BIOS] Unhandled INT 10h AH=1Ah with AL != 00h"));
            }
        }

        // AH = 1Bh: Get Video State Information
        // ES:DI points to 64-byte buffer
        0x1B => {
            let es = cpu.es();
            let di = cpu.di();
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
            cpu.bus.write_8(addr + 0x25, display_combination(cpu));

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

        // AH = 4Fh: VESA BIOS Extensions
        0x4F => super::vbe::handle(cpu),

        // AH = 0Ch: Write Graphics Pixel, in any standard graphics mode
        // AL = Color Value (bit 7: XOR it with the pixel's)
        // BH = Page Number
        // CX = Column (X)
        // DX = Row (Y)
        0x0C => {
            let (x, y, color) = (cpu.cx() as usize, cpu.dx() as usize, cpu.get_al());
            pixels::put_pixel(&mut cpu.bus, x, y, color);
        }

        // AH = 0Dh: Read Graphics Pixel
        // BH = Page Number
        // CX = Column (X)
        // DX = Row (Y)
        // Returns: AL = Color Value
        0x0D => {
            let color = pixels::get_pixel(&cpu.bus, cpu.cx() as usize, cpu.dx() as usize);
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

/// Writes a character and attribute to the active page (text modes).
fn write_char_at(cpu: &mut Cpu, col: u8, row: u8, char_code: u8, attr: u8) {
    let page = active_page(cpu);
    write_page_char(cpu, page, col, row, char_code, attr);
}

/// Writes a character and attribute to text page `page`.
fn write_page_char(cpu: &mut Cpu, page: u8, col: u8, row: u8, char_code: u8, attr: u8) {
    if pixels::graphics_mode(&cpu.bus) {
        // `attr` is the colour, XORed with bit 7.
        pixels::draw_char(&mut cpu.bus, col as usize, row as usize, char_code, attr);
        return;
    }
    if !text_mode(cpu) {
        return;
    }
    let addr = cell_addr(cpu, page, col as usize, row as usize);
    cpu.bus.write_8(addr, char_code);
    cpu.bus.write_8(addr + 1, attr);
}

/// Reads a character and attribute from text page `page`.
fn read_page_char(cpu: &Cpu, page: u8, col: u8, row: u8) -> (u8, u8) {
    if !text_mode(cpu) {
        return (0, 0);
    }
    let addr = cell_addr(cpu, page, col as usize, row as usize);
    (cpu.bus.read_8(addr), cpu.bus.read_8(addr + 1))
}

/// Reads a character and attribute from VRAM (Text Mode)
fn read_char_at(cpu: &Cpu, col: u8, row: u8, page: u8) -> (u8, u8) {
    read_page_char(cpu, page, col, row)
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
    // Graphics modes scroll pixels, a character cell at a time, and fill
    // with the colour in `attr`.
    if pixels::graphics_mode(&cpu.bus) {
        let (top, left, bottom, right) = (row_start as usize, col_start as usize, row_end as usize, col_end as usize);
        pixels::scroll(&mut cpu.bus, up, lines as usize, attr, top, left, bottom, right);
        return;
    }

    // Safety Clamps for Text Mode Logic
    let max_cols = text_cols(cpu);
    let page = active_page(cpu);

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
                let (val, at) = read_page_char(cpu, page, c as u8, (r + count) as u8);

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
                    let (val, at) = read_page_char(cpu, page, c as u8, (r - count) as u8);

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
