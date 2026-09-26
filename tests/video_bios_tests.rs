//! The video BIOS and the VGA as programs see them: the BIOS data area's
//! video fields, display pages, which CRTC answers, and colours going
//! through the palette registers and the DAC in every mode.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video::{self, Frame};
use std::path::PathBuf;

/// INT 10h with the given registers.
fn int10(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16, dx: u16) {
    cpu.set_ax(ax);
    cpu.set_reg16(Register::BX, bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
    int10::handle(cpu);
}

fn machine(mode: u8) -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    int10(&mut cpu, mode as u16, 0, 0, 0);
    cpu
}

/// The picture, as the CRTC shows it after the next vertical retrace.
fn picture(cpu: &mut Cpu) -> Frame {
    cpu.bus.vga.latch_start_address();
    let (width, height) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    cpu.bus.vga.mark_dirty_full();
    video::render_screen(&mut frame, &cpu.bus);
    frame
}

fn pixel(frame: &Frame, x: usize, y: usize) -> (u8, u8, u8) {
    let i = (y * frame.width as usize + x) * 3;
    (frame.rgb[i], frame.rgb[i + 1], frame.rgb[i + 2])
}

/// The colours in a text cell (8x16 pixels at `col`, `row`).
fn cell_colors(frame: &Frame, col: usize, row: usize) -> Vec<(u8, u8, u8)> {
    let mut colors = Vec::new();
    for y in row * 16..(row + 1) * 16 {
        for x in col * 8..(col + 1) * 8 {
            let c = pixel(frame, x, y);
            if !colors.contains(&c) {
                colors.push(c);
            }
        }
    }
    colors
}

/// A DAC entry as 8-bit RGB.
fn dac(cpu: &Cpu, index: u8) -> (u8, u8, u8) {
    cpu.bus.vga.get_rgb(index)
}

#[test]
fn the_bda_has_the_page_size_and_offset() {
    let mut cpu = machine(0x03);
    assert_eq!(cpu.bus.read_16(0x044C), 0x1000);
    assert_eq!(cpu.bus.read_16(0x044E), 0);
    assert_eq!(cpu.bus.read_8(0x0465), 0x29);
    assert_eq!(cpu.bus.read_8(0x0466), 0x30);
    int10(&mut cpu, 0x0013, 0, 0, 0);
    assert_eq!(cpu.bus.read_16(0x044C), 0xFA00);
    int10(&mut cpu, 0x0006, 0, 0, 0);
    assert_eq!((cpu.bus.read_16(0x044C), cpu.bus.read_8(0x0466)), (0x4000, 0x3F));
}

#[test]
fn text_pages_show_from_the_start_address() {
    let mut cpu = machine(0x03);
    // 'A' on page 0 and 'B' on page 1, through the BIOS.
    int10(&mut cpu, 0x0941, 0x0007, 1, 0);
    int10(&mut cpu, 0x0200, 0x0100, 0, 0);
    int10(&mut cpu, 0x0942, 0x0107, 1, 0);
    assert_eq!(cpu.bus.read_8(0xB8000), b'A');
    assert_eq!(cpu.bus.read_8(0xB9000), b'B');

    int10(&mut cpu, 0x0501, 0, 0, 0);
    assert_eq!(cpu.bus.read_8(0x0462), 1);
    assert_eq!(cpu.bus.read_16(0x044E), 0x1000);
    assert_eq!((cpu.bus.vga.crtc_regs[0x0C], cpu.bus.vga.crtc_regs[0x0D]), (0x08, 0x00));
    let frame = picture(&mut cpu);
    assert_eq!(video::text::geometry(&cpu.bus).unwrap().start, 0x1000);
    assert!(cell_colors(&frame, 0, 0).contains(&(0xA8, 0xA8, 0xA8)), "page 1's B shows");

    // Teletype output goes to the active page.
    let mut cpu = machine(0x03);
    int10(&mut cpu, 0x0501, 0, 0, 0);
    int10(&mut cpu, 0x0E43, 0, 0, 0);
    assert_eq!(cpu.bus.read_8(0xB9000), b'C');
    assert_eq!(cpu.bus.read_8(0xB8000), 0x20);
}

#[test]
fn teletype_keeps_the_cells_attribute() {
    let mut cpu = machine(0x03);
    int10(&mut cpu, 0x0958, 0x001E, 1, 0); // 'X', yellow on blue
    int10(&mut cpu, 0x0E59, 0, 0, 0); // 'Y' over it
    assert_eq!((cpu.bus.read_8(0xB8000), cpu.bus.read_8(0xB8001)), (b'Y', 0x1E));
}

#[test]
fn a_colour_vga_has_no_crtc_at_3b4() {
    let mut cpu = machine(0x03);
    cpu.bus.io_write(0x3D4, 0x0E);
    assert_eq!(cpu.bus.io_read(0x3D5), cpu.bus.vga.crtc_regs[0x0E]);
    cpu.bus.io_write(0x3B4, 0x0F);
    assert_eq!(cpu.bus.io_read(0x3B5), 0xFF);
    assert_eq!(cpu.bus.io_read(0x3BA), 0xFF);
    // A monochrome mode's CRTC moves there (Miscellaneous Output bit 0).
    let misc = cpu.bus.vga.misc_output_reg;
    cpu.bus.io_write(0x3C2, misc & !1);
    cpu.bus.io_write(0x3B4, 0x0E);
    assert_eq!(cpu.bus.io_read(0x3B5), cpu.bus.vga.crtc_regs[0x0E]);
    assert_eq!(cpu.bus.io_read(0x3DA), 0xFF);
    assert_ne!(cpu.bus.io_read(0x3BA), 0xFF);
}

#[test]
fn the_index_registers_read_back() {
    let mut cpu = machine(0x12);
    for (port, index) in [(0x3C4, 0x02), (0x3CE, 0x08), (0x3D4, 0x0E)] {
        cpu.bus.io_write(port, index);
        assert_eq!(cpu.bus.io_read(port), index, "port {port:03X}");
    }
    // The attribute controller's, with the palette address source bit.
    cpu.bus.io_read(0x3DA);
    cpu.bus.io_write(0x3C0, 0x31);
    assert_eq!(cpu.bus.io_read(0x3C0), 0x31);
    // Saving the sequencer's index around a read of its registers, as
    // Windows' VDD does, leaves the next data write where it was going.
    cpu.bus.io_write(0x3C4, 0x02);
    let saved = cpu.bus.io_read(0x3C4);
    cpu.bus.io_write(0x3C4, 0x04);
    cpu.bus.io_read(0x3C5);
    cpu.bus.io_write(0x3C4, saved);
    cpu.bus.io_write(0x3C5, 0x0F);
    assert_eq!(cpu.bus.vga.sequencer_regs[2], 0x0F);
}

#[test]
fn text_and_graphics_switched_through_the_registers_show_at_the_retrace() {
    let mut cpu = machine(0x12);
    let attribute = |cpu: &mut Cpu, index: u8, value: u8| {
        cpu.bus.io_read(0x3DA);
        cpu.bus.io_write(0x3C0, index);
        cpu.bus.io_write(0x3C0, value);
    };
    // Text through the attribute controller, as Windows' VDD shows its
    // messages: the rest of the registers first, the mode at the retrace.
    attribute(&mut cpu, 0x10, 0x0C);
    cpu.bus.io_write(0x3D4, 0x01);
    cpu.bus.io_write(0x3D5, 0x4F);
    assert_eq!(cpu.bus.video_mode, video::VideoMode::Vga640x480);
    cpu.bus.settle_register_mode();
    assert_eq!(cpu.bus.video_mode, video::VideoMode::Text80x25Color);
    // And back to its 640x480.
    attribute(&mut cpu, 0x10, 0x01);
    cpu.bus.settle_register_mode();
    assert_eq!(cpu.bus.video_mode, video::VideoMode::Vga640x480);
    // Nothing switched, nothing changes: a program's own tweak of a
    // graphics mode stays what the BIOS set.
    let mut cpu = machine(0x10);
    cpu.bus.io_write(0x3D4, 0x12);
    cpu.bus.io_write(0x3D5, 0xDF);
    cpu.bus.settle_register_mode();
    assert_eq!(cpu.bus.video_mode, video::VideoMode::Ega640x350);
}

#[test]
fn text_colours_come_through_the_palette_registers_and_the_dac() {
    let mut cpu = machine(0x03);
    // Dark grey on black: palette register 8 holds 38h.
    int10(&mut cpu, 0x0941, 0x0008, 1, 0);
    let frame = picture(&mut cpu);
    assert!(cell_colors(&frame, 0, 0).contains(&dac(&cpu, 0x38)));
    assert_eq!(dac(&cpu, 0x38), (0x54, 0x54, 0x54));

    // A program changing DAC entry 38h changes the dark grey.
    cpu.bus.io_write(0x3C8, 0x38);
    for value in [0x3F, 0, 0] {
        cpu.bus.io_write(0x3C9, value);
    }
    let frame = picture(&mut cpu);
    assert!(cell_colors(&frame, 0, 0).contains(&(0xFC, 0, 0)));

    // Brown is palette register 6: DAC entry 14h.
    int10(&mut cpu, 0x0941, 0x0006, 1, 0);
    let frame = picture(&mut cpu);
    assert!(cell_colors(&frame, 0, 0).contains(&(0xA8, 0x54, 0)));
}

#[test]
fn blinking_characters_and_bright_backgrounds() {
    let mut cpu = machine(0x03);
    // Blinking light grey on blue.
    int10(&mut cpu, 0x0941, 0x0097, 1, 0);
    let blue = (0, 0, 0xA8);
    let grey = (0xA8, 0xA8, 0xA8);
    let frame = picture(&mut cpu);
    let colors = cell_colors(&frame, 0, 0);
    assert!(colors.contains(&grey) && colors.contains(&blue), "{:?}", colors);
    cpu.bus.vga.set_blink(false);
    assert_eq!(cell_colors(&picture(&mut cpu), 0, 0), [blue]);

    // With blinking off, bit 7 picks the bright background instead.
    int10(&mut cpu, 0x1003, 0x0000, 0, 0);
    let colors = cell_colors(&picture(&mut cpu), 0, 0);
    assert!(colors.contains(&grey) && colors.contains(&(0x54, 0x54, 0xFC)), "{:?}", colors);
}

#[test]
fn mode_4_takes_its_palette_from_int_10h_0bh() {
    let mut cpu = machine(0x04);
    // The BIOS starts with palette 1 at high intensity.
    assert_eq!(cpu.bus.vga.attribute_regs[1..4], [0x13, 0x15, 0x17]);
    // Pixels 0-3 in the first byte.
    cpu.bus.write_8(0xB8000, 0b00_01_10_11);
    let frame = picture(&mut cpu);
    assert_eq!(pixel(&frame, 2, 0), (0x54, 0xFC, 0xFC)); // bright cyan
    assert_eq!(pixel(&frame, 6, 0), (0xFC, 0xFC, 0xFC)); // bright white

    // Palette 0, a blue background at low intensity.
    int10(&mut cpu, 0x0B00, 0x0100, 0, 0);
    int10(&mut cpu, 0x0B00, 0x0001, 0, 0);
    assert_eq!(cpu.bus.vga.attribute_regs[0..4], [0x01, 0x02, 0x04, 0x06]);
    assert_eq!(cpu.bus.read_8(0x0466), 0x01);
    let frame = picture(&mut cpu);
    assert_eq!(pixel(&frame, 0, 0), (0, 0, 0xA8)); // background
    assert_eq!(pixel(&frame, 2, 0), (0, 0xA8, 0)); // green
    assert_eq!(pixel(&frame, 4, 0), (0xA8, 0, 0)); // red
    assert_eq!(pixel(&frame, 6, 0), (0xA8, 0x54, 0)); // brown

    // Bit 4 of the background brightens the three colors.
    int10(&mut cpu, 0x0B00, 0x0011, 0, 0);
    assert_eq!(cpu.bus.vga.attribute_regs[1..4], [0x12, 0x14, 0x16]);
}

#[test]
fn the_200_line_modes_have_a_cga_monitors_colours() {
    let mut cpu = machine(0x0D);
    // Palette registers 8-15 are the bright colours: 10h-17h.
    assert_eq!(cpu.bus.vga.attribute_regs[8..16], [0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17]);
    // A pixel of each colour at x = colour: set/reset gives the colour,
    // the bit mask the pixel.
    let out = |cpu: &mut Cpu, port: u16, index: u8, value: u8| {
        cpu.bus.io_write(port, index);
        cpu.bus.io_write(port + 1, value);
    };
    out(&mut cpu, 0x3C4, 0x02, 0x0F);
    out(&mut cpu, 0x3CE, 0x01, 0x0F);
    for color in [6u8, 8, 14] {
        out(&mut cpu, 0x3CE, 0x00, color);
        out(&mut cpu, 0x3CE, 0x08, 0x80 >> (color % 8));
        let addr = 0xA0000 + color as usize / 8;
        cpu.bus.read_8(addr);
        cpu.bus.write_8(addr, 0xFF);
    }
    let frame = picture(&mut cpu);
    // 320 pixels drawn twice as wide.
    assert_eq!(pixel(&frame, 12, 0), (0xA8, 0x54, 0)); // brown
    assert_eq!(pixel(&frame, 16, 0), (0x54, 0x54, 0x54)); // dark grey
    assert_eq!(pixel(&frame, 28, 0), (0xFC, 0xFC, 0x54)); // yellow
}

#[test]
fn ega_configuration_reports_the_switches() {
    let mut cpu = machine(0x03);
    int10(&mut cpu, 0x1200, 0x0010, 0xFFFF, 0);
    assert_eq!(cpu.get_reg8(Register::BH), 0); // colour
    assert_eq!(cpu.get_reg8(Register::BL), 3); // 256 KB
    assert_eq!(cpu.get_reg8(Register::CL), cpu.bus.read_8(0x0488) & 0x0F);
    assert_eq!(cpu.get_reg8(Register::CH), cpu.bus.read_8(0x0488) >> 4);
}

/// The glyph of `ch` in a font of `height` at segment:offset.
fn glyph_at(cpu: &Cpu, segment: u16, offset: u16, ch: u8, height: usize) -> Vec<u8> {
    let base = ((segment as usize) << 4) + offset as usize + ch as usize * height;
    (0..height).map(|i| cpu.bus.read_8(base + i)).collect()
}

#[test]
fn the_rom_fonts_are_where_int_10h_says() {
    let mut cpu = machine(0x03);
    let fonts = [(2u16, 14usize, video::font_8x14()), (3, 8, video::font_8x8()), (6, 16, video::font_8x16())];
    for (bh, height, font) in fonts {
        int10(&mut cpu, 0x1130, bh << 8, 0, 0);
        let glyph = glyph_at(&cpu, cpu.es(), cpu.bp(), b'A', height);
        assert_eq!(glyph, font[b'A' as usize * height..(b'A' as usize + 1) * height], "BH={}", bh);
        assert_eq!(cpu.cx(), 16, "CX is the height of the font on the screen");
        assert_eq!(cpu.get_reg8(Register::DL), 24);
    }
    // The 8x8 font's second half, and INT 1Fh pointing at it.
    int10(&mut cpu, 0x1130, 0x0400, 0, 0);
    let high = (cpu.es(), cpu.bp());
    assert_eq!(glyph_at(&cpu, high.0, high.1, 0x00, 8), video::font_8x8()[0x80 * 8..0x81 * 8]);
    int10(&mut cpu, 0x1130, 0x0000, 0, 0);
    assert_eq!((cpu.es(), cpu.bp()), high);
    // The PC BIOS's 8x8 font at F000:FA6E.
    assert_eq!(glyph_at(&cpu, 0xF000, 0xFA6E, b'A', 8), [0x30, 0x78, 0xCC, 0xCC, 0xFC, 0xCC, 0xCC, 0x00]);
    // The 9-dot alternate glyphs: a character code, 14 bytes, ..., 0.
    int10(&mut cpu, 0x1130, 0x0500, 0, 0);
    assert_eq!(cpu.bus.read_8(((cpu.es() as usize) << 4) + cpu.bp() as usize), 0x1D);
}

#[test]
fn text_fonts_set_the_rows_that_fit() {
    let mut cpu = machine(0x03);
    for (al, rows, height) in [(0x12u8, 50u8, 8u16), (0x11, 28, 14), (0x14, 25, 16)] {
        int10(&mut cpu, 0x1100 | al as u16, 0, 0, 0);
        assert_eq!(cpu.bus.read_8(0x0484) + 1, rows, "AL={:02X}", al);
        assert_eq!(cpu.bus.read_16(0x0485), height);
        let geometry = video::text::geometry(&cpu.bus).unwrap();
        assert_eq!((geometry.rows, geometry.font_h), (rows as usize, height as usize));
    }
}

#[test]
fn the_bios_writes_text_in_graphics_modes() {
    // Mode 13h: teletype in colour BL, from the 8x8 graphics font.
    let mut cpu = machine(0x13);
    int10(&mut cpu, 0x0E41, 0x0004, 0, 0);
    let read = |cpu: &mut Cpu, x: u16, y: u16| {
        int10(cpu, 0x0D00, 0, x, y);
        cpu.get_al()
    };
    // The top row of 'A' is 00110000.
    assert_eq!((read(&mut cpu, 2, 0), read(&mut cpu, 3, 0), read(&mut cpu, 1, 0)), (4, 4, 0));
    assert_eq!(cpu.bus.read_8(0x0450), 1, "the cursor moved on");

    // Mode 4: AH=09h, and AH=08h reads the character back.
    let mut cpu = machine(0x04);
    int10(&mut cpu, 0x0941, 0x0003, 1, 0);
    assert_eq!(read(&mut cpu, 2, 0), 3);
    int10(&mut cpu, 0x0800, 0, 0, 0);
    assert_eq!(cpu.get_al(), b'A');

    // Mode 12h: the 8x16 font, in a plane colour. Row 2 of 'A' is 00010000.
    let mut cpu = machine(0x12);
    int10(&mut cpu, 0x0E41, 0x000F, 0, 0);
    assert_eq!((read(&mut cpu, 3, 2), read(&mut cpu, 2, 2)), (15, 0));
}

#[test]
fn graphics_pixels_xor_and_scroll() {
    let mut cpu = machine(0x12);
    int10(&mut cpu, 0x0C05, 0, 10, 20);
    int10(&mut cpu, 0x0C83, 0, 10, 20);
    int10(&mut cpu, 0x0D00, 0, 10, 20);
    assert_eq!(cpu.get_al(), 5 ^ 3);
    // In 256 colours bit 7 is part of the colour.
    let mut cpu = machine(0x13);
    int10(&mut cpu, 0x0C05, 0, 10, 20);
    int10(&mut cpu, 0x0C83, 0, 10, 20);
    int10(&mut cpu, 0x0D00, 0, 10, 20);
    assert_eq!(cpu.get_al(), 0x83);

    // A line feed on the last row scrolls the screen up a character row.
    int10(&mut cpu, 0x0C07, 0, 0, 8);
    int10(&mut cpu, 0x0200, 0, 0, 24 << 8);
    int10(&mut cpu, 0x0E0A, 0, 0, 0);
    int10(&mut cpu, 0x0D00, 0, 0, 0);
    assert_eq!(cpu.get_al(), 7);
    int10(&mut cpu, 0x0D00, 0, 0, 8);
    assert_eq!(cpu.get_al(), 0);

    // AH=06h clears a window in the fill colour.
    let mut cpu = machine(0x0D);
    int10(&mut cpu, 0x0600, 0x0200, 0, (24 << 8) | 39);
    int10(&mut cpu, 0x0D00, 0, 100, 100);
    assert_eq!(cpu.get_al(), 2);
}
