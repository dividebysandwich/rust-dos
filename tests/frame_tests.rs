//! The picture's size follows the video mode: 640x400 for text, the mode's
//! own size for graphics, small modes doubled.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video::{self, Frame};
use std::path::PathBuf;

fn machine(mode: u8) -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.set_ax(mode as u16);
    cpu.set_reg8(Register::AH, 0x00);
    int10::handle(&mut cpu);
    cpu
}

fn out(cpu: &mut Cpu, port: u16, index: u8, value: u8) {
    cpu.bus.io_write(port, index);
    cpu.bus.io_write(port + 1, value);
}

#[test]
fn frame_sizes_follow_the_mode() {
    for (mode, size) in [(0x03, (640, 400)), (0x13, (640, 400)), (0x0D, (640, 400)), (0x10, (640, 350)), (0x12, (640, 480))] {
        assert_eq!(video::frame_size(&machine(mode).bus), size, "mode {:02X}", mode);
    }

    // Unchained 320x240 programmed over mode 13h.
    let mut cpu = machine(0x13);
    out(&mut cpu, 0x3D4, 0x11, 0x0E);
    for (index, value) in [(0x06, 0x0D), (0x07, 0x3E), (0x10, 0xEA), (0x11, 0xAC), (0x12, 0xDF), (0x15, 0xE7), (0x16, 0x06)] {
        out(&mut cpu, 0x3D4, index, value);
    }
    cpu.bus.io_write(0x3C2, 0xE3);
    assert_eq!(video::frame_size(&cpu.bus), (640, 480));
}

#[test]
fn mode_12h_shows_all_480_lines() {
    let mut cpu = machine(0x12);
    // Map mask: all planes, so the pixel is color 15. Row 470, pixel 0.
    out(&mut cpu, 0x3C4, 0x02, 0x0F);
    cpu.bus.write_8(0xA0000 + 470 * 80, 0x80);
    let (width, height) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    video::render_screen(&mut frame, &cpu.bus);
    let at = |x: usize, y: usize| {
        let i = (y * width as usize + x) * 3;
        (frame.rgb[i], frame.rgb[i + 1], frame.rgb[i + 2])
    };
    assert_eq!(at(0, 470), cpu.bus.vga.get_rgb(0x3F));
    assert_eq!(at(1, 470), (0, 0, 0));
    assert_eq!(at(0, 469), (0, 0, 0));
}
