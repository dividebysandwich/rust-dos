//! `machine=cga`: what programs detecting the adapter find, the 6845's
//! timing, the Mode Control and Color Select registers, and the picture.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::composite::{CompositeEra, CompositeMode, CompositeSettings};
use rust_dos::video::{self, Frame, VideoMode, bios};
use std::path::PathBuf;

fn int10(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16, dx: u16) {
    cpu.set_ax(ax);
    cpu.set_reg16(Register::BX, bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
    int10::handle(cpu);
}

/// A machine with a CGA in `mode`, at 1000 instructions an emulated ms.
fn cga(mode: u8) -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.bus.set_cycles_per_ms(1000);
    bios::install(&mut cpu.bus, VideoSetup { adapter: Adapter::Cga, ..Default::default() });
    int10::set_mode(&mut cpu, mode);
    cpu
}

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

#[test]
fn programs_find_a_cga_and_nothing_better() {
    let mut cpu = cga(0x03);
    // No VGA: AH=1Ah returns with AL as it was.
    int10(&mut cpu, 0x1A00, 0, 0, 0);
    assert_eq!(cpu.get_al(), 0x00);
    // No EGA: AH=12h BL=10h leaves BL 10h.
    int10(&mut cpu, 0x1200, 0x0010, 0, 0);
    assert_eq!(cpu.get_reg8(Register::BL), 0x10);
    // No video BIOS ROM, no EGA information in the BIOS data area.
    assert_ne!((cpu.bus.read_8(0xC0000), cpu.bus.read_8(0xC0001)), (0x55, 0xAA));
    assert_eq!(cpu.bus.read_8(0x0487), 0);
    assert_eq!(cpu.bus.read_16(0x0410) & 0x30, 0x20, "80x25 colour");
    // The VGA's registers aren't there, nor a monochrome adapter's.
    for port in [0x3C2, 0x3C5, 0x3C9, 0x3CC, 0x3CF, 0x3B5, 0x3BA] {
        assert_eq!(cpu.bus.io_read(port), 0xFF, "port {:03X}", port);
    }
    // The 6845 answers: its cursor address reads back.
    cpu.bus.io_write(0x3D4, 0x0F);
    cpu.bus.io_write(0x3D5, 0x5A);
    assert_eq!(cpu.bus.io_read(0x3D5), 0x5A);
    cpu.bus.io_write(0x3D4, 0x01);
    assert_eq!(cpu.bus.io_read(0x3D5), 0x00, "other registers are write-only");
    // Only modes 0-6.
    int10(&mut cpu, 0x000D, 0, 0, 0);
    assert_eq!(cpu.bus.video_mode, VideoMode::Text80x25Color);
    int10(&mut cpu, 0x0013, 0, 0, 0);
    assert_eq!(cpu.bus.video_mode, VideoMode::Text80x25Color);
}

#[test]
fn the_picture_is_60_hz_with_262_lines() {
    let mut cpu = cga(0x04);
    let timing = cpu.bus.vga.timing();
    assert_eq!((timing.total, timing.display), (262, 200));
    assert!((timing.frame_ns() as i64 - 16_688_000).abs() < 20_000, "{}", timing.frame_ns());
    // Port 3DAh shows the vertical retrace once a frame.
    let mut retraces = 0;
    let mut was = false;
    for _ in 0..2000 {
        cpu.bus.clock.icount += 20; // 20 us
        let now = cpu.bus.io_read(0x3DA) & 0x08 != 0;
        if now && !was {
            retraces += 1;
        }
        was = now;
    }
    assert_eq!(retraces, 2, "two retraces in 40 ms");
}

#[test]
fn mode_4_colours_come_from_the_color_select_register() {
    let mut cpu = cga(0x04);
    cpu.bus.write_8(0xB8000, 0b00_01_10_11);
    // The BIOS sets palette 1, bright: cyan, magenta and white.
    let frame = picture(&mut cpu);
    assert_eq!(pixel(&frame, 2, 0), (0x55, 0xFF, 0xFF));
    assert_eq!(pixel(&frame, 6, 0), (0xFF, 0xFF, 0xFF));
    // Palette 0 at low intensity, on blue: green, red, brown.
    cpu.bus.io_write(0x3D9, 0x01);
    let frame = picture(&mut cpu);
    let colors: Vec<_> = [0, 2, 4, 6].iter().map(|&x| pixel(&frame, x, 0)).collect();
    assert_eq!(colors, [(0, 0, 0xAA), (0, 0xAA, 0), (0xAA, 0, 0), (0xAA, 0x55, 0)]);
    // INT 10h AH=0Bh sets the register too, from the BIOS's copy in BDA
    // 0466h: a blue background without intensity, then palette 1.
    int10(&mut cpu, 0x0B00, 0x0001, 0, 0);
    int10(&mut cpu, 0x0B00, 0x0101, 0, 0);
    assert_eq!(cpu.bus.read_8(0x0466), 0x21);
    let frame = picture(&mut cpu);
    assert_eq!(pixel(&frame, 2, 0), (0, 0xAA, 0xAA));
    // Mode 5 (no colour burst) shows cyan, red and white.
    int10(&mut cpu, 0x0005, 0, 0, 0);
    cpu.bus.write_8(0xB8000, 0b00_01_10_11);
    let frame = picture(&mut cpu);
    let colors: Vec<_> = [2, 4, 6].iter().map(|&x| pixel(&frame, x, 0)).collect();
    assert_eq!(colors, [(0x55, 0xFF, 0xFF), (0xFF, 0x55, 0x55), (0xFF, 0xFF, 0xFF)]);
}

#[test]
fn mode_6_draws_in_the_color_select_colour() {
    let mut cpu = cga(0x06);
    cpu.bus.write_8(0xB8000, 0x80);
    let frame = picture(&mut cpu);
    assert_eq!((pixel(&frame, 0, 0), pixel(&frame, 1, 0)), ((0xFF, 0xFF, 0xFF), (0, 0, 0)));
    cpu.bus.io_write(0x3D9, 0x0E);
    assert_eq!(pixel(&picture(&mut cpu), 0, 0), (0xFF, 0xFF, 0x55));
}

#[test]
fn programs_set_the_mode_through_the_mode_control_register() {
    let mut cpu = cga(0x03);
    cpu.bus.io_write(0x3D8, 0x0A);
    assert_eq!(cpu.bus.video_mode, VideoMode::Cga320x200Color);
    cpu.bus.io_write(0x3D8, 0x1E);
    assert_eq!(cpu.bus.video_mode, VideoMode::Cga640x200);
    cpu.bus.io_write(0x3D8, 0x29);
    assert_eq!(cpu.bus.video_mode, VideoMode::Text80x25Color);
    // Bit 3 off: no picture.
    int10(&mut cpu, 0x0941, 0x000F, 1, 0);
    cpu.bus.io_write(0x3D8, 0x21);
    assert!(picture(&mut cpu).rgb.iter().all(|&b| b == 0));
}

#[test]
fn text_is_the_8x8_font_scanned_twice() {
    let mut cpu = cga(0x03);
    int10(&mut cpu, 0x0941, 0x001E, 1, 0); // 'A', yellow on blue
    let geometry = video::text::geometry(&cpu.bus).unwrap();
    assert_eq!((geometry.cols, geometry.rows, geometry.cell_w(), geometry.cell_h()), (80, 25, 8, 16));
    let frame = picture(&mut cpu);
    // Row 0 of 'A' is 00110000, twice.
    for y in [0, 1] {
        assert_eq!(pixel(&frame, 2, y), (0xFF, 0xFF, 0x55));
        assert_eq!(pixel(&frame, 1, y), (0, 0, 0xAA));
    }
}

#[test]
fn the_160x100_tweak_has_100_rows_of_two_scanlines() {
    let mut cpu = cga(0x03);
    // As Paku Paku programs it: R4 7Fh, R6 64h, R7 70h, R9 1.
    for (reg, value) in [(0x04, 0x7F), (0x06, 0x64), (0x07, 0x70), (0x09, 0x01)] {
        cpu.bus.io_write(0x3D4, reg);
        cpu.bus.io_write(0x3D5, value);
    }
    cpu.bus.io_write(0x3D8, 0x09); // 80 columns, no blinking
    let geometry = video::text::geometry(&cpu.bus).unwrap();
    assert_eq!((geometry.rows, geometry.font_h, geometry.cell_h()), (100, 2, 4));
    assert_eq!(cpu.bus.vga.timing().total, 262);
    // Character DEh (the right half block) in red on green: two pixels.
    let cell = 0xB8000 + (50 * 80 + 10) * 2;
    cpu.bus.write_8(cell, 0xDE);
    cpu.bus.write_8(cell + 1, 0x24);
    let frame = picture(&mut cpu);
    assert_eq!(pixel(&frame, 10 * 8, 50 * 4), (0, 0xAA, 0));
    assert_eq!(pixel(&frame, 10 * 8 + 7, 50 * 4 + 3), (0xAA, 0, 0));
}

#[test]
fn the_bios_writes_graphics_text_from_the_rom_font() {
    let mut cpu = cga(0x04);
    int10(&mut cpu, 0x0E41, 0x0003, 0, 0);
    int10(&mut cpu, 0x0D00, 0, 2, 0);
    assert_eq!(cpu.get_al(), 3);
}

#[test]
fn the_shell_runs_on_a_cga() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    bios::install(&mut cpu.bus, VideoSetup { adapter: Adapter::Cga, ..Default::default() });
    cpu.load_shell();
    video::print_string(&mut cpu, "HELLO");
    let frame = picture(&mut cpu);
    assert!(frame.rgb[..640 * 16 * 3].iter().any(|&b| b != 0));
    assert_eq!(cpu.bus.read_16(0x0460), 0x0607);
    assert_eq!(cpu.bus.read_8(0x0484), 0);
}

/// Fill the graphics memory (both banks) with `byte`.
fn fill(cpu: &mut Cpu, byte: u8) {
    for offset in 0..0x4000 {
        cpu.bus.write_8(0xB8000 + offset, byte);
    }
}

fn composite(cpu: &mut Cpu, mode: CompositeMode, era: CompositeEra) {
    cpu.bus.vga.set_composite(CompositeSettings { mode, era });
}

/// The colour in the middle of the screen, away from the border.
fn middle(cpu: &mut Cpu) -> (u8, u8, u8) {
    pixel(&picture(cpu), 320, 200)
}

fn saturated((r, g, b): (u8, u8, u8)) -> bool {
    r.max(g).max(b) as i32 - r.min(g).min(b) as i32 > 60
}

#[test]
fn composite_games_get_artifact_colours_in_640x200() {
    // What King's Quest does for a composite monitor: mode 6, then the
    // colour burst on (Mode Control 1Ah), white on black.
    let mut cpu = cga(0x06);
    cpu.bus.io_write(0x3D8, 0x1A);
    assert_eq!(cpu.bus.video_mode, VideoMode::Cga640x200);
    fill(&mut cpu, 0x00);
    assert_eq!(middle(&mut cpu), (0, 0, 0));
    fill(&mut cpu, 0xFF);
    let (r, g, b) = middle(&mut cpu);
    assert!(r > 230 && g > 230 && b > 230, "white: {:?}", (r, g, b));
    // 0011 and 1100: two pixels on, two off, a cycle of the colour
    // carrier at opposite phases: blue and brown, say.
    fill(&mut cpu, 0x33);
    let blue = middle(&mut cpu);
    fill(&mut cpu, 0xCC);
    let brown = middle(&mut cpu);
    assert!(saturated(blue) && saturated(brown), "{:?} {:?}", blue, brown);
    assert_ne!(blue, brown);
    assert!(blue.2 > blue.0, "0011 is bluish: {:?}", blue);

    // The BIOS's own mode 6 has the burst off: an RGB picture, as on a
    // CGA's colour monitor.
    cpu.bus.io_write(0x3D8, 0x1E);
    fill(&mut cpu, 0x33);
    let frame = picture(&mut cpu);
    assert_eq!((pixel(&frame, 320, 200), pixel(&frame, 322, 200)), ((0, 0, 0), (0xFF, 0xFF, 0xFF)));
}

#[test]
fn composite_follows_the_setting() {
    let mut cpu = cga(0x06);
    cpu.bus.io_write(0x3D8, 0x1A);
    fill(&mut cpu, 0x33);
    // Off: the RGB monitor's two colours, whatever the program set.
    composite(&mut cpu, CompositeMode::Off, CompositeEra::Old);
    assert_eq!(pixel(&picture(&mut cpu), 322, 200), (0xFF, 0xFF, 0xFF));
    // On: composite with the BIOS's mode 6 too, where the missing burst
    // makes the patterns grey.
    composite(&mut cpu, CompositeMode::On, CompositeEra::Old);
    cpu.bus.io_write(0x3D8, 0x1E);
    let (r, g, b) = middle(&mut cpu);
    assert!(r == g && g == b && r > 40 && r < 220, "grey: {:?}", (r, g, b));
    // The two revisions of the card mix the colours differently.
    cpu.bus.io_write(0x3D8, 0x1A);
    let old = middle(&mut cpu);
    composite(&mut cpu, CompositeMode::On, CompositeEra::New);
    assert_ne!(old, middle(&mut cpu));
    // Only a CGA has the composite output.
    let mut vga = Cpu::new(PathBuf::from("."));
    int10::set_mode(&mut vga, 0x06);
    vga.bus.vga.set_composite(CompositeSettings { mode: CompositeMode::On, era: CompositeEra::Old });
    assert!(!vga.bus.vga.composite_active());
}

#[test]
fn composite_320x200_blends_the_palette() {
    let mut cpu = cga(0x04);
    // Palette 1 (cyan, magenta, white); 01 pixels: cyan on an RGB monitor.
    fill(&mut cpu, 0x55);
    let rgb = middle(&mut cpu);
    assert_eq!(rgb, (0x55, 0xFF, 0xFF));
    // Auto leaves 320x200 to the RGB monitor; on, it is composite.
    composite(&mut cpu, CompositeMode::On, CompositeEra::Old);
    let decoded = middle(&mut cpu);
    assert_ne!(decoded, rgb);
    assert!(saturated(decoded), "{:?}", decoded);
    // Mode 5 has no colour burst: grey.
    cpu.bus.io_write(0x3D8, 0x0E);
    let (r, g, b) = middle(&mut cpu);
    assert!(r == g && g == b, "{:?}", (r, g, b));
}
