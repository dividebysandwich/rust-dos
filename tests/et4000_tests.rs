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
