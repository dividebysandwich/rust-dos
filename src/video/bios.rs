//! What the video card and its BIOS show programs looking for them: the
//! BIOS data area's fields about the adapter and the monitor, and the video
//! BIOS ROM at C000h.

use super::VideoMode;
use super::adapter::{Adapter, VideoSetup};
use crate::bus::Bus;

/// The video BIOS ROM's segment, and where its fonts are in it: the 8x8
/// font (its second half on its own for INT 1Fh), the 8x16 and the 8x14
/// fonts, and the alternate glyphs of the 9-dot wide 14 and 16-line cells.
pub const ROM_SEGMENT: u16 = 0xC000;
pub const FONT_8X8: u16 = 0x1000;
pub const FONT_8X8_HIGH: u16 = 0x1400;
pub const FONT_8X16: u16 = 0x2000;
pub const FONT_8X14: u16 = 0x3000;
pub const FONT_9X14: u16 = 0x3E00;
pub const FONT_9X16: u16 = 0x3F40;
/// Where the PC BIOS keeps the first half of its 8x8 font, for the CGA
/// graphics modes: F000:FA6E.
pub const PC_FONT_8X8: usize = 0xFFA6E;

/// A far pointer (segment:offset) into the video BIOS ROM.
pub fn rom_pointer(offset: u16) -> u32 {
    (ROM_SEGMENT as u32) << 16 | offset as u32
}

/// The graphics font (INT 43h) and its height for a mode: 8x8 in the
/// 200-line modes, 8x14 in the 350-line and 8x16 in the 480-line ones.
pub fn graphics_font(mode: u8) -> (u16, u16) {
    match mode {
        0x0F | 0x10 => (FONT_8X14, 14),
        0x11 | 0x12 => (FONT_8X16, 16),
        _ => (FONT_8X8, 8),
    }
}

/// Put another adapter in at the prompt: its BIOS data and the text mode
/// it starts in, keeping what the screen shows and where the cursor is, so
/// the prompt carries on where it was.
pub fn switch(cpu: &mut crate::cpu::Cpu, setup: VideoSetup) {
    let (col, row) = (cpu.bus.read_8(0x0450), cpu.bus.read_8(0x0451));
    install(&mut cpu.bus, setup);
    crate::interrupts::int10::set_mode(cpu, 0x80 | setup.prompt_mode());
    let row = row.min(cpu.bus.text_rows() as u8 - 1);
    cpu.bus.write_8(0x0450, col);
    cpu.bus.write_8(0x0451, row);
    cpu.bus.cursor_x = col as usize;
    cpu.bus.cursor_y = row as usize;
}

/// The text cursor's scanlines for a text mode with `height` scanlines to
/// a character: the two above the bottom one (the CGA's 8: 6 and 7).
pub fn cursor_shape(adapter: Adapter, height: u16) -> u16 {
    match (adapter, height) {
        (Adapter::Cga, _) | (_, 8) => 0x0607,
        (_, 14) => 0x0B0C,
        _ => 0x0D0E,
    }
}

/// Put the machine in the text mode the DOS prompt runs in (3, or 7 on a
/// monochrome adapter), with the registers, palette and BIOS data a mode
/// set leaves, so a program that exited in another mode, with its own
/// palette or rows, doesn't leave the prompt in it.
pub fn reset_for_shell(bus: &mut Bus) {
    let adapter = bus.vga.adapter;
    let mono = bus.vga.setup().mono();
    let mode = if mono { VideoMode::Mono80x25 } else { VideoMode::Text80x25Color };
    bus.write_8(0x0449, mode as u8); // Mode 3 (80x25 color text) or 7
    bus.write_16(0x044A, 80); // 80 columns
    bus.write_16(0x044C, 0x1000); // page size
    bus.write_16(0x044E, 0); // page 0's offset
    bus.write_8(0x0462, 0); // Active page 0
    bus.write_8(0x0450, 0); // Cursor col
    bus.write_8(0x0451, 0); // Cursor row
    bus.write_8(0x0465, 0x29);
    bus.write_8(0x0466, 0x30);
    let height = match adapter {
        Adapter::Cga => 0,
        Adapter::Ega | Adapter::Hercules => 14,
        _ => 16,
    };
    if adapter.ega_bios() {
        bus.write_8(0x0484, 24); // 25 rows
        bus.write_16(0x0485, height); // 8x16 font cell
    }
    bus.write_16(0x0460, cursor_shape(adapter, if adapter.mono_only() { 14 } else { height }));
    bus.video_mode = mode;
    bus.vga.set_video_mode(mode);
    if gray_summing(bus) {
        bus.vga.sum_to_gray(0..256);
    }
    bus.vbe.reset();
}

/// Whether the VGA BIOS sums the palettes it loads to grey (BDA 0489h bit
/// 1, set for a monochrome monitor).
pub fn gray_summing(bus: &Bus) -> bool {
    bus.vga.adapter.vga_bios() && bus.read_8(0x0489) & 0x02 != 0
}

/// Write the fonts into the ROMs.
fn install_fonts(bus: &mut Bus) {
    let rom = |offset: u16| ((ROM_SEGMENT as usize) << 4) + offset as usize;
    bus.load_bytes(rom(FONT_8X8), super::font_8x8());
    bus.load_bytes(rom(FONT_8X16), super::font_8x16());
    bus.load_bytes(rom(FONT_8X14), super::font_8x14());
    bus.load_bytes(rom(FONT_9X14), super::FONT_9X14_ALTERNATE);
    bus.load_bytes(rom(FONT_9X16), super::FONT_9X16_ALTERNATE);
    bus.load_bytes(PC_FONT_8X8, &super::font_8x8()[..128 * 8]);
}

/// Put the adapter of `setup` in place: the card, and what the BIOS data
/// area and the ROM say about it. The video mode stays as it is; the
/// caller sets the one the adapter starts in.
pub fn install(bus: &mut Bus, setup: VideoSetup) {
    bus.vga.adapter = setup.adapter;
    bus.vga.mono_monitor = setup.mono_monitor;

    // Equipment word bits 4-5: the initial video mode, 10 for 80x25 in
    // colour, 11 for monochrome. The other bits are the floppies' and the
    // coprocessor's.
    let mono = setup.mono();
    let equipment = bus.read_16(0x0410) & !0x0030;
    bus.write_16(0x0410, equipment | if mono { 0x0030 } else { 0x0020 });
    // The CRTC's address.
    bus.write_16(0x0463, if mono { 0x03B4 } else { 0x03D4 });

    if matches!(setup.adapter, Adapter::Cga | Adapter::Hercules) {
        // The PC BIOS keeps neither the rows and the character height
        // (0484h-0486h) nor the EGA's and VGA's information (0487h-048Ah),
        // and there is no video BIOS ROM at C000h.
        for addr in 0x0484..=0x048A {
            bus.write_8(addr, 0);
        }
        bus.load_bytes(0xC0000, &[0xFF; 3]);
        bus.load_bytes(0xC001E, &[0xFF; 7]);
        install_fonts(bus);
        return;
    }
    // 0484h and 0485h: 25 rows of the text font (8x14 on the EGA's 350
    // lines, 8x16 on the VGA's 400).
    let ega = setup.adapter == Adapter::Ega;
    // An EGA's switches for an Enhanced Color Display, or for the IBM
    // Monochrome Display (1011); the VGA reads its monitor instead.
    bus.write_8(0x0484, 24);
    bus.write_16(0x0485, if ega { 14 } else { 16 });
    bus.write_16(0x0460, cursor_shape(setup.adapter, if ega { 14 } else { 16 }));
    bus.vga.switches = match (ega, mono) {
        (true, true) => 0b1011,
        (true, false) => 0b1001,
        _ => 0b0110,
    };
    if ega {
        // 0487h: bits 5-6 the memory (256 KB), bit 1 a monochrome monitor;
        // 0488h: the switches and the feature bits; the VGA's 0489h and
        // 048Ah are 0.
        bus.write_8(0x0487, if mono { 0x62 } else { 0x60 });
        bus.write_8(0x0488, 0xF0 | bus.vga.switches);
        bus.write_8(0x0489, 0x00);
        bus.write_8(0x048A, 0x00);
        // The EGA BIOS ROM: 16 KB (32 blocks), without the VGA's name.
        bus.load_bytes(0xC0000, &[0x55, 0xAA, 0x20]);
        bus.load_bytes(0xC001E, b"IBM EGA");
        install_fonts(bus);
        return;
    }

    // 0487h: bits 5-6 the memory (11: 256 KB).
    bus.write_8(0x0487, 0x60);
    // 0488h: the EGA's switch settings and feature bits; 9 is an enhanced
    // colour display.
    bus.write_8(0x0488, 0x09);
    // 0489h: bit 0 the VGA is active; bits 7 and 4 the text modes'
    // scanlines (01: 400); bit 1 gray-scale summing and bit 2 a monochrome
    // monitor, which the BIOS sums the palettes it loads for; bit 3 no
    // palette loading.
    bus.write_8(0x0489, if mono { 0x17 } else { 0x11 });
    // 048Ah: the index of the VGA's entry in the display combination code
    // table (INT 10h AH=1Ah returns the code itself).
    bus.write_8(0x048A, 0x0B);

    // The video BIOS ROM: its signature and size (64 blocks of 512 bytes),
    // and the name that programs look for.
    bus.load_bytes(0xC0000, &[0x55, 0xAA, 0x40]);
    bus.load_bytes(0xC001E, b"IBM VGA");
    install_fonts(bus);
}
