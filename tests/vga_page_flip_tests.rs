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
    cpu.ax = 0x0013;
    cpu.set_reg8(Register::AH, 0x00);
    int10::handle(&mut cpu);
    cpu
}

/// RGB of 320x200 pixel (x, y) in a fresh render.
fn pixel(cpu: &Cpu, x: usize, y: usize) -> (u8, u8, u8) {
    let mut frame = vec![0u8; (SCREEN_WIDTH * SCREEN_HEIGHT * 3) as usize];
    video::render_screen(&mut frame, &cpu.bus);
    let i = (y * 2 * SCREEN_WIDTH as usize + x * 2) * 3;
    (frame[i], frame[i + 1], frame[i + 2])
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
    cpu.ax = 0x0013;
    int10::handle(&mut cpu);
    assert_eq!(cpu.bus.vga.latched_start_addr, 0);
}
