//! 256-color mode shows VRAM from the CRTC Start Address that was latched
//! at the last vertical retrace, so unchained ("mode X") games can draw one
//! page while another is on screen.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video::{self, SCREEN_HEIGHT, SCREEN_WIDTH};
use std::path::PathBuf;

fn mode_13h() -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.set_ax(0x0013);
    cpu.set_reg8(Register::AH, 0x00);
    int10::handle(&mut cpu);
    cpu
}

/// RGB of 320x200 pixel (x, y) in a fresh render.
fn pixel(cpu: &Cpu, x: usize, y: usize) -> (u8, u8, u8) {
    let (width, height) = video::frame_size(&cpu.bus);
    assert_eq!((width, height), (SCREEN_WIDTH, SCREEN_HEIGHT));
    let mut frame = video::Frame::new(width, height);
    video::render_screen(&mut frame, &cpu.bus);
    let i = (y * 2 * SCREEN_WIDTH as usize + x * 2) * 3;
    (frame.rgb[i], frame.rgb[i + 1], frame.rgb[i + 2])
}

fn out(cpu: &mut Cpu, port: u16, index: u8, value: u8) {
    cpu.bus.io_write(port, index);
    cpu.bus.io_write(port + 1, value);
}

#[test]
fn chained_mode_13h_is_linear() {
    let mut cpu = mode_13h();
    cpu.bus.write_8(0xA0000 + 320 + 1, 4);
    cpu.bus.vga.mark_dirty_full();
    assert_eq!(pixel(&cpu, 1, 1), cpu.bus.vga.get_rgb(4));
    assert_eq!(pixel(&cpu, 0, 1), cpu.bus.vga.get_rgb(0));
}

#[test]
fn unchained_pages_flip_at_retrace() {
    let mut cpu = mode_13h();
    out(&mut cpu, 0x3C4, 0x04, 0x06); // unchain the planes
    out(&mut cpu, 0x3C4, 0x02, 0x0F); // write all four planes
    cpu.bus.write_8(0xA0000, 9); // page 0: pixels 0..3 of row 0
    cpu.bus.write_8(0xA4000, 5); // page 1 at 4000h
    assert_eq!(pixel(&cpu, 3, 0), cpu.bus.vga.get_rgb(9));

    // Flipping takes effect at the next retrace, not on the write.
    out(&mut cpu, 0x3D4, 0x0C, 0x40);
    out(&mut cpu, 0x3D4, 0x0D, 0x00);
    assert_eq!(pixel(&cpu, 3, 0), cpu.bus.vga.get_rgb(9));
    cpu.bus.vga.latch_start_address();
    assert_eq!(pixel(&cpu, 3, 0), cpu.bus.vga.get_rgb(5));
    assert_eq!(pixel(&cpu, 4, 0), cpu.bus.vga.get_rgb(0));

    // A new mode starts displaying from the top of VRAM again.
    cpu.set_ax(0x0013);
    int10::handle(&mut cpu);
    assert_eq!(cpu.bus.vga.latched_start_addr, 0);
}

#[test]
fn split_screen_shows_address_0_below_the_line_compare() {
    let mut cpu = mode_13h();
    out(&mut cpu, 0x3C4, 0x04, 0x06); // unchain the planes
    out(&mut cpu, 0x3C4, 0x02, 0x0F); // write all four planes
    cpu.bus.write_8(0xA0000, 9); // status bar at address 0
    cpu.bus.write_8(0xA4000 + 150 * 80, 5); // playfield page at 4000h, row 150
    out(&mut cpu, 0x3D4, 0x0C, 0x40);
    out(&mut cpu, 0x3D4, 0x0D, 0x00);
    cpu.bus.vga.latch_start_address();
    assert_eq!(pixel(&cpu, 0, 150), cpu.bus.vga.get_rgb(5));

    // Split after scanline 299: rows of 2 scanlines from row 150 on show
    // address 0 onwards.
    out(&mut cpu, 0x3D4, 0x18, 299u16 as u8);
    let overflow = cpu.bus.vga.crtc_regs[0x07] | 0x10; // bit 8
    out(&mut cpu, 0x3D4, 0x07, overflow);
    let max_scan = cpu.bus.vga.crtc_regs[0x09] & !0x40; // bit 9 clear
    out(&mut cpu, 0x3D4, 0x09, max_scan);
    assert_eq!(pixel(&cpu, 0, 150), cpu.bus.vga.get_rgb(9));
    assert_eq!(pixel(&cpu, 0, 149), cpu.bus.vga.get_rgb(0));

    // Horizontal panning shifts the playfield by half the register value
    // in 256-color modes.
    cpu.bus.write_8(0xA4000 + 10 * 80 + 1, 7);
    cpu.bus.io_read(0x3DA);
    cpu.bus.io_write(0x3C0, 0x33); // index 13h, palette access off
    cpu.bus.io_write(0x3C0, 0x04);
    assert_eq!(pixel(&cpu, 2, 10), cpu.bus.vga.get_rgb(7));
}

/// Render into `frame` as the main loop does, and check it shows what a
/// fresh render of the whole screen does.
fn render_again(cpu: &mut Cpu, frame: &mut video::Frame) {
    cpu.bus.sync_display();
    let (width, height) = video::frame_size(&cpu.bus);
    if frame.resize(width, height) {
        cpu.bus.vga.mark_dirty_full();
    }
    if cpu.bus.vga.dirty {
        video::render_screen(frame, &cpu.bus);
    }
    let mut fresh = video::Frame::new(width, height);
    cpu.bus.vga.mark_dirty_full();
    video::render_screen(&mut fresh, &cpu.bus);
    cpu.bus.vga.clear_dirty();
    assert!(*frame == fresh, "a render over the last differs from a fresh one");
}

#[test]
fn rows_drawn_again_over_the_last_picture_are_the_new_picture() {
    let mut cpu = mode_13h();
    let mut frame = video::Frame::new(1, 1);
    for x in 0..320 {
        cpu.bus.write_8(0xA0000 + 50 * 320 + x, x as u8);
    }
    render_again(&mut cpu, &mut frame);

    // A picture of text in between doesn't stay.
    let pixels = cpu.bus.vga.vram_graphics.clone();
    cpu.set_ax(0x0003);
    int10::handle(&mut cpu);
    render_again(&mut cpu, &mut frame);
    cpu.set_ax(0x0013);
    int10::handle(&mut cpu);
    cpu.bus.vga.vram_graphics = pixels;
    cpu.bus.vga.mark_dirty_full();
    render_again(&mut cpu, &mut frame);

    // A pixel, a color, the PEL mask, a page flip, and a write that leaves
    // the pixel as it was.
    cpu.bus.write_8(0xA0000 + 199 * 320 + 319, 7);
    render_again(&mut cpu, &mut frame);
    cpu.bus.io_write(0x3C8, 7);
    for v in [63, 0, 32] {
        cpu.bus.io_write(0x3C9, v);
    }
    render_again(&mut cpu, &mut frame);
    cpu.bus.io_write(0x3C6, 0x0F);
    render_again(&mut cpu, &mut frame);
    out(&mut cpu, 0x3D4, 0x0C, 0x01);
    cpu.bus.vga.latch_start_address();
    render_again(&mut cpu, &mut frame);
    cpu.bus.write_8(0xA0000 + 50 * 320 + 3, 3);
    render_again(&mut cpu, &mut frame);
}
