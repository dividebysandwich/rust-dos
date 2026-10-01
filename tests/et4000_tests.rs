//! The Tseng ET4000 (`machine=svga_et4000`): its registers as programs
//! detect and program them, its banked memory, the Sierra HiColor DAC,
//! Tseng's BIOS modes and its VBE 1.2.

use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::{self, bios, Frame};
use std::path::PathBuf;

fn machine() -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    bios::install(&mut cpu.bus, VideoSetup { adapter: Adapter::Et4000, ..Default::default() });
    int10::set_mode(&mut cpu, 0x03);
    cpu.set_es(0x3000);
    cpu.set_di(0);
    cpu
}

fn crtc(cpu: &mut Cpu, index: u8) -> u8 {
    cpu.bus.io_write(0x3D4, index);
    cpu.bus.io_read(0x3D5)
}

fn set_crtc(cpu: &mut Cpu, index: u8, value: u8) {
    cpu.bus.io_write(0x3D4, index);
    cpu.bus.io_write(0x3D5, value);
}

/// Read 3C6h four times, to reach the Sierra DAC's command register.
fn sierra_command(cpu: &mut Cpu) {
    cpu.bus.io_write(0x3C8, 0);
    for _ in 0..4 {
        cpu.bus.io_read(0x3C6);
    }
}

#[test]
fn programs_find_an_et4000_with_1_mb() {
    let mut cpu = machine();
    // The KEY, as Tseng's own utilities set it.
    cpu.bus.io_write(0x3BF, 0x03);
    cpu.bus.io_write(0x3D8, 0xA0);
    // VGADOC's test: Segment Select keeps what it is given, CR33 four
    // bits of it.
    cpu.bus.io_write(0x3CD, 0x5A);
    assert_eq!(cpu.bus.io_read(0x3CD), 0x5A);
    cpu.bus.io_write(0x3CD, 0x00);
    set_crtc(&mut cpu, 0x33, 0xFF);
    assert_eq!(crtc(&mut cpu, 0x33), 0x0F);
    set_crtc(&mut cpu, 0x33, 0x00);
    // CR37: a 32-bit bus to 256K-deep chips, 1 MB.
    assert_eq!(crtc(&mut cpu, 0x37) & 0x0B, 0x0B);
    // The attribute controller's register 16h takes a value.
    cpu.bus.io_read(0x3DA);
    cpu.bus.io_write(0x3C0, 0x36);
    cpu.bus.io_write(0x3C0, 0x10);
    cpu.bus.io_read(0x3DA);
    cpu.bus.io_write(0x3C0, 0x36);
    assert_eq!(cpu.bus.io_read(0x3C1), 0x10);
    cpu.bus.io_read(0x3DA);
    cpu.bus.io_write(0x3C0, 0x36);
    cpu.bus.io_write(0x3C0, 0x00);
    cpu.bus.io_write(0x3C0, 0x20);
    // Without the KEY the extended registers are gone, CR33 not.
    cpu.bus.io_write(0x3BF, 0x01);
    cpu.bus.io_write(0x3D8, 0x29);
    assert_eq!(crtc(&mut cpu, 0x37), 0);
    set_crtc(&mut cpu, 0x33, 0x02);
    assert_eq!(crtc(&mut cpu, 0x33), 0x02);
    // Tseng's name in the video BIOS.
    let rom: Vec<u8> = (0xC0000..0xC0100).map(|a| cpu.bus.read_8(a)).collect();
    assert!(rom.windows(5).any(|w| w == b"Tseng"));
}

#[test]
fn the_window_is_banked_for_writes_and_reads_apart() {
    let mut cpu = machine();
    int10::set_mode(&mut cpu, 0x13);
    // Chained: bank 3 is the linear bytes from 30000h.
    cpu.bus.io_write(0x3CD, 0x03);
    cpu.bus.write_8(0xA0000 + 0x1235, 0x77);
    let linear = 0x3_1235;
    let plane_size = cpu.bus.vga.plane_size();
    assert_eq!(plane_size, 0x40000);
    assert_eq!(cpu.bus.vga.vram_graphics[(linear & 3) * plane_size + (linear >> 2)], 0x77);
    // Reads still come from bank 0, until its read half says 3.
    assert_eq!(cpu.bus.read_8(0xA0000 + 0x1235), 0x00);
    cpu.bus.io_write(0x3CD, 0x30);
    assert_eq!(cpu.bus.read_8(0xA0000 + 0x1235), 0x77);

    // Planar: bank 1 is the second 64 KB of each plane.
    int10::set_mode(&mut cpu, 0x12);
    cpu.bus.io_write(0x3CD, 0x11);
    cpu.bus.write_8(0xA0000 + 0x10, 0xFF);
    assert_eq!(cpu.bus.vga.vram_graphics[0x1_0010], 0xFF);
    assert_eq!(cpu.bus.vga.vram_graphics[3 * plane_size + 0x1_0010], 0xFF);
    assert_eq!(cpu.bus.vga.vram_graphics[0x10], 0x00);
    // A mode set takes the bank back to 0.
    int10::set_mode(&mut cpu, 0x12);
    assert_eq!(cpu.bus.io_read(0x3CD), 0x00);
}

#[test]
fn the_sierra_dac_s_command_register_turns_on_hicolor() {
    let mut cpu = machine();
    int10::set_mode(&mut cpu, 0x13);
    sierra_command(&mut cpu);
    assert_eq!(cpu.bus.io_read(0x3C6), 0x00);
    cpu.bus.io_write(0x3C6, 0xA0);
    assert_eq!(cpu.bus.vga.et4000.hicolor(), Some(15));
    // The pixel mask is still the pixel mask.
    assert_eq!(cpu.bus.vga.dac_mask, 0xFF);
    cpu.bus.io_write(0x3C6, 0xFF);
    assert_eq!(cpu.bus.vga.et4000.hicolor(), Some(15));
    // Mode 13h's registers at two bytes a pixel: 160 pixels across.
    assert_eq!(cpu.bus.vga.graphics_size(), (160, 200));
    // A pixel of pure red in 5:5:5 at the top left.
    cpu.bus.write_8(0xA0000, 0x00);
    cpu.bus.write_8(0xA0001, 0x7C);
    let (width, height) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    video::render_screen(&mut frame, &cpu.bus);
    assert_eq!(&frame.rgb[..3], &[255, 0, 0]);
    // A BIOS mode set turns it off.
    int10::set_mode(&mut cpu, 0x13);
    assert_eq!(cpu.bus.vga.et4000.hicolor(), None);
}

/// INT 10h with AX, BX, CX, DX; returns AX.
fn int10(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16, dx: u16) -> u16 {
    cpu.set_ax(ax);
    cpu.set_bx(bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
    int10::handle(cpu);
    cpu.ax()
}

/// The rendered picture: its size, and its pixel at (x, y).
fn picture(cpu: &mut Cpu) -> (u32, u32, Vec<u8>) {
    let (width, height) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    cpu.bus.vga.mark_dirty_full();
    video::render_screen(&mut frame, &cpu.bus);
    (width, height, frame.rgb)
}

fn pixel(cpu: &mut Cpu, x: usize, y: usize) -> [u8; 3] {
    let (width, _, rgb) = picture(cpu);
    let i = (y * width as usize + x) * 3;
    [rgb[i], rgb[i + 1], rgb[i + 2]]
}

/// Write `value` to linear byte `at` of a chained mode, through the bank
/// it is in.
fn poke_linear(cpu: &mut Cpu, at: usize, value: u8) {
    let bank = (at >> 16) as u8;
    cpu.bus.io_write(0x3CD, bank | bank << 4);
    cpu.bus.write_8(0xA0000 + (at & 0xFFFF), value);
}

#[test]
fn tseng_s_256_colour_modes_show_banked_memory() {
    for (mode, width, height) in [(0x2Du8, 640u32, 350u32), (0x2E, 640, 480), (0x2F, 640, 400), (0x30, 800, 600), (0x38, 1024, 768)] {
        let mut cpu = machine();
        int10(&mut cpu, mode as u16, 0, 0, 0);
        assert_eq!(cpu.bus.read_8(0x0449), mode);
        assert_eq!(cpu.bus.read_16(0x044A), width as u16 / 8);
        let (w, h, _) = picture(&mut cpu);
        assert_eq!((w, h), (width, height), "mode {:02X}", mode);
        assert_eq!(cpu.bus.display_size(), (width as usize, height as usize));
        // A pixel near the bottom right, in a bank of its own: colour 15
        // of the default palette is white.
        let (x, y) = (width as usize - 3, height as usize - 2);
        poke_linear(&mut cpu, y * width as usize + x, 15);
        let (r, g, b) = cpu.bus.vga.get_rgb(15);
        assert_eq!(pixel(&mut cpu, x, y), [r, g, b], "mode {:02X}", mode);
        assert!(r > 200);
        assert_eq!(pixel(&mut cpu, x - 1, y), [0, 0, 0]);
        // The BIOS's own pixels there.
        int10(&mut cpu, 0x0C04, 0, 5, height as u16 - 1);
        assert_eq!(int10(&mut cpu, 0x0D00, 0, 5, height as u16 - 1) & 0xFF, 4);
        assert_eq!(int10(&mut cpu, 0x0D00, 0, x as u16, y as u16) & 0xFF, 15);
    }
}

#[test]
fn tseng_s_16_colour_modes_are_planar_past_64_kb() {
    for (mode, width, height) in [(0x29u8, 800u32, 600u32), (0x37, 1024, 768)] {
        let mut cpu = machine();
        int10(&mut cpu, mode as u16, 0, 0, 0);
        let (w, h, _) = picture(&mut cpu);
        assert_eq!((w, h), (width, height), "mode {:02X}", mode);
        // The last row's first byte: past 64 KB of each plane in 37h.
        let offset = (height as usize - 1) * width as usize / 8;
        let bank = (offset >> 16) as u8;
        cpu.bus.io_write(0x3CD, bank | bank << 4);
        cpu.bus.write_8(0xA0000 + (offset & 0xFFFF), 0x80);
        let (r, g, b) = cpu.bus.vga.attribute_rgb(15);
        assert_eq!(pixel(&mut cpu, 0, height as usize - 1), [r, g, b], "mode {:02X}", mode);
        assert!(r > 200);
        assert_eq!(pixel(&mut cpu, 1, height as usize - 1), [0, 0, 0]);
        // AH=0Ch draws there too, red (4).
        int10(&mut cpu, 0x0C04, 0, width as u16 - 1, height as u16 - 1);
        assert_eq!(int10(&mut cpu, 0x0D00, 0, width as u16 - 1, height as u16 - 1) & 0xFF, 4);
    }
}

#[test]
fn the_hicolor_bios_sets_32k_and_64k_colours() {
    let mut cpu = machine();
    // The DAC is a Sierra HiColor one.
    assert_eq!(int10(&mut cpu, 0x10F1, 0, 0, 0), 0x0010);
    assert_eq!(cpu.get_reg8(iced_x86::Register::BL), 1);
    assert_eq!(int10(&mut cpu, 0x10F0, 0x2E, 0, 0), 0x0010);
    assert_eq!(cpu.bus.read_8(0x0449), 0x2E);
    let (w, h, _) = picture(&mut cpu);
    assert_eq!((w, h), (640, 480));
    // Pixel (639, 479): two bytes, red in 5:5:5.
    let at = (479 * 640 + 639) * 2;
    poke_linear(&mut cpu, at, 0x00);
    poke_linear(&mut cpu, at + 1, 0x7C);
    assert_eq!(pixel(&mut cpu, 639, 479), [255, 0, 0]);
    int10(&mut cpu, 0x10F2, 0, 0, 0);
    assert_eq!(cpu.get_reg8(iced_x86::Register::BL), 1);
    // 64K colours: the same bytes are red and some green in 5:6:5.
    assert_eq!(int10(&mut cpu, 0x10F2, 2, 0, 0), 0x0010);
    assert_eq!(pixel(&mut cpu, 639, 479), [123, 130, 0]);
    // 800x600 and 320x200 too; not 1024x768.
    assert_eq!(int10(&mut cpu, 0x10F0, 0x30, 0, 0), 0x0010);
    assert_eq!(picture(&mut cpu).0, 800);
    assert_eq!(int10(&mut cpu, 0x10F0, 0x13, 0, 0), 0x0010);
    let (w, h, _) = picture(&mut cpu);
    assert_eq!((w, h, cpu.bus.vga.graphics_size()), (640, 400, (320, 200)));
    assert_eq!(int10(&mut cpu, 0x10F0, 0x38, 0, 0), 0x10F0);
    // The timings are a monitor's.
    let timing = cpu.bus.vga.peek_timing();
    assert!((60.0..75.0).contains(&timing.hz()), "{} Hz", timing.hz());
}

#[test]
fn a_standard_mode_after_a_tseng_mode_is_the_vga_s() {
    let mut cpu = machine();
    int10(&mut cpu, 0x0030, 0, 0, 0);
    cpu.bus.io_write(0x3CD, 0x55);
    int10(&mut cpu, 0x0013, 0, 0, 0);
    assert_eq!(cpu.bus.io_read(0x3CD), 0);
    assert_eq!(cpu.bus.vga.graphics_size(), (320, 200));
    assert_eq!(picture(&mut cpu).0, 640);
    int10(&mut cpu, 0x0003, 0, 0, 0);
    assert_eq!(video::frame_size(&cpu.bus).1, 400);
    assert_eq!(cpu.bus.vga.et4000.hicolor(), None);
}

#[test]
fn a_loaded_et4000_shows_the_same() {
    let mut a = machine();
    int10(&mut a, 0x10F0, 0x2E, 0, 0);
    for i in 0..640 * 480 * 2 {
        if i % 0x10000 == 0 {
            let bank = (i >> 16) as u8;
            a.bus.io_write(0x3CD, bank | bank << 4);
        }
        a.bus.write_8(0xA0000 + (i & 0xFFFF), (i * 7 / 3) as u8);
    }
    let before = picture(&mut a);
    let state = rust_dos::savestate::machine::save(&a);
    let mut b = machine();
    rust_dos::savestate::machine::load(&mut b, &state).unwrap();
    assert!(rust_dos::savestate::machine::save(&b) == state);
    assert!(picture(&mut b) == before);
}

#[test]
fn tseng_s_text_modes_are_wider_and_taller() {
    for (mode, cols, rows, cell) in [(0x22u8, 132u32, 44u32, 8u32), (0x23, 132, 25, 14), (0x24, 132, 28, 13), (0x26, 80, 60, 8), (0x2A, 100, 40, 15)] {
        let mut cpu = machine();
        int10(&mut cpu, mode as u16, 0, 0, 0);
        assert_eq!(cpu.bus.read_8(0x0449), mode);
        assert_eq!(cpu.bus.read_16(0x044A) as u32, cols, "mode {:02X}", mode);
        assert_eq!(cpu.bus.read_8(0x0484) as u32 + 1, rows, "mode {:02X}", mode);
        let geometry = video::text::geometry(&cpu.bus).unwrap();
        assert_eq!((geometry.cols as u32, geometry.rows as u32, geometry.cell_h() as u32), (cols, rows, cell), "mode {:02X}", mode);
        assert_eq!(video::frame_size(&cpu.bus), (cols * 8, rows * cell), "mode {:02X}", mode);
        // The teletype fills the last column of a row and goes on in the
        // next one, and scrolls at the bottom.
        int10(&mut cpu, 0x0200, 0, 0, (cols as u16 - 1) | (rows as u16 - 1) << 8);
        int10(&mut cpu, 0x0E41, 0, 0, 0);
        int10(&mut cpu, 0x0E42, 0, 0, 0);
        let at = |cpu: &Cpu, col: u32, row: u32| cpu.bus.read_8(0xB8000 + ((row * cols + col) * 2) as usize);
        assert_eq!(at(&cpu, cols - 1, rows - 2), b'A', "mode {:02X}", mode);
        assert_eq!(at(&cpu, 0, rows - 1), b'B', "mode {:02X}", mode);
        let timing = cpu.bus.vga.peek_timing();
        assert!((55.0..75.0).contains(&timing.hz()), "mode {:02X}: {} Hz", mode, timing.hz());
    }
    // Back to 80 columns.
    let mut cpu = machine();
    int10(&mut cpu, 0x0022, 0, 0, 0);
    int10(&mut cpu, 0x0003, 0, 0, 0);
    assert_eq!(video::text::geometry(&cpu.bus).unwrap().cols, 80);
}
