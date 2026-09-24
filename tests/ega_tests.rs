//! `machine=ega`: an EGA with an Enhanced Color Display, as programs
//! detecting it find it, its timing, and its colours without a DAC.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::{self, Frame, VideoMode, bios};
use std::path::PathBuf;

fn int10(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16, dx: u16) {
    cpu.set_ax(ax);
    cpu.set_reg16(Register::BX, bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
    int10::handle(cpu);
}

fn ega(mode: u8) -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.bus.set_cycles_per_ms(1000);
    bios::install(&mut cpu.bus, VideoSetup { adapter: Adapter::Ega, ..Default::default() });
    int10::set_mode(&mut cpu, mode);
    cpu
}

fn picture(cpu: &mut Cpu) -> Frame {
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

/// Pixel 0 of the top row in colour `color`, through the planes.
fn first_pixel(cpu: &mut Cpu, color: u8) {
    cpu.bus.io_write(0x3C4, 0x02);
    cpu.bus.io_write(0x3C5, 0x0F);
    cpu.bus.write_8(0xA0000, 0x00);
    cpu.bus.io_write(0x3C5, color);
    cpu.bus.write_8(0xA0000, 0x80);
}

#[test]
fn programs_find_an_ega_and_no_vga() {
    let mut cpu = ega(0x03);
    // No VGA BIOS: AH=1Ah leaves AL.
    int10(&mut cpu, 0x1A00, 0, 0, 0);
    assert_eq!(cpu.get_al(), 0x00);
    // The EGA's configuration: colour, 256 KB, the switches of an
    // Enhanced Color Display.
    int10(&mut cpu, 0x1200, 0x0010, 0, 0);
    assert_eq!(cpu.get_reg8(Register::BH), 0);
    assert_eq!(cpu.get_reg8(Register::BL), 3);
    assert_eq!(cpu.get_reg8(Register::CL), 0x09);
    assert_eq!((cpu.bus.read_8(0x0487), cpu.bus.read_8(0x0488)), (0x60, 0xF9));
    assert_eq!((cpu.bus.read_8(0xC0000), cpu.bus.read_8(0xC0001)), (0x55, 0xAA));
    // No VESA, no VGA modes.
    int10(&mut cpu, 0x4F00, 0, 0, 0);
    assert_eq!(cpu.ax(), 0x4F00);
    for mode in [0x12, 0x13] {
        int10(&mut cpu, mode, 0, 0, 0);
        assert_eq!(cpu.bus.video_mode, VideoMode::Text80x25Color, "mode {:02X}", mode);
    }
}

#[test]
fn the_registers_are_write_only() {
    let mut cpu = ega(0x10);
    // No DAC, no read-back of the other registers.
    for port in [0x3C1, 0x3C6, 0x3C7, 0x3C8, 0x3C9, 0x3CC] {
        assert_eq!(cpu.bus.io_read(port), 0xFF, "port {:03X}", port);
    }
    cpu.bus.io_write(0x3C4, 0x02);
    assert_eq!(cpu.bus.io_read(0x3C5), 0xFF);
    cpu.bus.io_write(0x3CE, 0x05);
    assert_eq!(cpu.bus.io_read(0x3CF), 0xFF);
    cpu.bus.io_write(0x3D4, 0x01);
    assert_eq!(cpu.bus.io_read(0x3D5), 0xFF);
    // But the cursor address can be read.
    cpu.bus.io_write(0x3D4, 0x0F);
    cpu.bus.io_write(0x3D5, 0x42);
    assert_eq!(cpu.bus.io_read(0x3D5), 0x42);
}

#[test]
fn the_350_and_200_line_timings() {
    let mut cpu = ega(0x10);
    let timing = cpu.bus.vga.timing();
    assert_eq!(timing.display, 350);
    let line_khz = 1e6 / timing.line_ns as f64;
    assert!((line_khz - 21.85).abs() < 0.1, "{} kHz", line_khz);
    assert!((timing.hz() - 60.0).abs() < 1.0, "{} Hz", timing.hz());
    assert_eq!(video::frame_size(&cpu.bus), (640, 350));

    let mut cpu = ega(0x0D);
    let timing = cpu.bus.vga.timing();
    assert_eq!((timing.total, timing.display), (262, 200));
    assert!((timing.hz() - 60.0).abs() < 1.0, "{} Hz", timing.hz());
    assert_eq!(video::frame_size(&cpu.bus), (640, 400));
}

#[test]
fn colours_go_straight_from_the_palette_registers_to_the_monitor() {
    // At 350 lines the palette registers hold rgbRGB: 14h is brown.
    let mut cpu = ega(0x10);
    first_pixel(&mut cpu, 6);
    assert_eq!(pixel(&picture(&mut cpu), 0, 0), (0xAA, 0x55, 0x00));
    // At 200 lines the monitor takes RGB and intensity: 14h is light red,
    // 06h brown.
    let mut cpu = ega(0x0D);
    first_pixel(&mut cpu, 6);
    assert_eq!(pixel(&picture(&mut cpu), 0, 0), (0xAA, 0x55, 0x00));
    int10(&mut cpu, 0x1000, 0x1406, 0, 0);
    assert_eq!(pixel(&picture(&mut cpu), 0, 0), (0xFF, 0x55, 0x55));
}

#[test]
fn text_is_640x350_in_the_8x14_font() {
    let mut cpu = ega(0x03);
    assert_eq!(video::frame_size(&cpu.bus), (640, 350));
    let geometry = video::text::geometry(&cpu.bus).unwrap();
    assert_eq!((geometry.rows, geometry.font_h), (25, 14));
    assert_eq!(cpu.bus.read_16(0x0460), 0x0B0C);
    // The 8x8 font: 43 rows.
    int10(&mut cpu, 0x1112, 0, 0, 0);
    assert_eq!(cpu.bus.read_8(0x0484) + 1, 43);
    int10(&mut cpu, 0x0941, 0x001E, 1, 0);
    let frame = picture(&mut cpu);
    assert_eq!(pixel(&frame, 0, 0), (0x00, 0x00, 0xAA));
}

#[test]
fn the_shell_runs_on_an_ega() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    bios::install(&mut cpu.bus, VideoSetup { adapter: Adapter::Ega, ..Default::default() });
    cpu.load_shell();
    video::print_string(&mut cpu, "HELLO");
    assert_eq!(video::frame_size(&cpu.bus), (640, 350));
    let frame = picture(&mut cpu);
    assert!(frame.rgb[..640 * 14 * 3].iter().any(|&b| b != 0));
    assert_eq!(cpu.bus.read_16(0x0485), 14);
}
