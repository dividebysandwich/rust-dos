//! `machine=hercules`: the Hercules Graphics Card as programs detecting it
//! find it, the monochrome text mode 7 with the MDA's attributes, and the
//! 720x348 graphics programs set up themselves.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::hercules::{BLACK, BRIGHT, CRTC_GRAPHICS, NORMAL};
use rust_dos::video::{self, Frame, VideoMode, bios};
use std::path::PathBuf;

fn int10(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16, dx: u16) {
    cpu.set_ax(ax);
    cpu.set_reg16(Register::BX, bx);
    cpu.set_cx(cx);
    cpu.set_dx(dx);
    int10::handle(cpu);
}

fn hercules() -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("."));
    cpu.bus.set_cycles_per_ms(1000);
    bios::install(&mut cpu.bus, VideoSetup { adapter: Adapter::Hercules, ..Default::default() });
    int10::set_mode(&mut cpu, 0x07);
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
fn programs_find_a_monochrome_adapter() {
    let mut cpu = hercules();
    assert_eq!(cpu.bus.read_16(0x0410) & 0x30, 0x30, "monochrome in the equipment word");
    assert_eq!(cpu.bus.read_16(0x0463), 0x3B4);
    int10(&mut cpu, 0x0F00, 0, 0, 0);
    assert_eq!(cpu.get_al(), 0x07);
    // No EGA, no VGA, no video BIOS ROM.
    int10(&mut cpu, 0x1200, 0x0010, 0, 0);
    assert_eq!(cpu.get_reg8(Register::BL), 0x10);
    int10(&mut cpu, 0x1A00, 0, 0, 0);
    assert_eq!(cpu.get_al(), 0x00);
    assert_ne!((cpu.bus.read_8(0xC0000), cpu.bus.read_8(0xC0001)), (0x55, 0xAA));
    // A 6845 at 3B4h, nothing at 3D4h or the VGA's ports.
    cpu.bus.io_write(0x3B4, 0x0E);
    cpu.bus.io_write(0x3B5, 0x07);
    assert_eq!(cpu.bus.io_read(0x3B5), 0x07);
    for port in [0x3D5, 0x3DA, 0x3C5, 0x3CC, 0x3C9] {
        assert_eq!(cpu.bus.io_read(port), 0xFF, "port {:03X}", port);
    }
}

#[test]
fn status_bit_7_tells_a_hercules_card_from_an_mda() {
    let mut cpu = hercules();
    let (mut high, mut low) = (false, false);
    for _ in 0..1000 {
        cpu.bus.clock.icount += 20; // 20 us: 20 ms in all, a frame at 50 Hz
        let status = cpu.bus.io_read(0x3BA);
        assert_eq!(status & 0x70, 0, "bits 4-6: an HGC");
        if status & 0x80 != 0 { high = true } else { low = true }
    }
    assert!(high && low, "bit 7 changes within a frame");
    let timing = cpu.bus.vga.timing();
    assert_eq!((timing.total, timing.display), (370, 350));
    assert!((timing.hz() - 49.8).abs() < 0.5);
}

#[test]
fn text_goes_to_b0000_and_every_mode_is_7() {
    let mut cpu = hercules();
    int10(&mut cpu, 0x0E41, 0, 0, 0);
    assert_eq!(cpu.bus.read_8(0xB0000), b'A');
    assert_eq!(cpu.bus.vga.vram_text[0], b'A');
    int10(&mut cpu, 0x0013, 0, 0, 0);
    assert_eq!(cpu.bus.video_mode, VideoMode::Mono80x25);
    assert_eq!(cpu.bus.read_8(0x0449), 0x07);
    assert_eq!(video::frame_size(&cpu.bus), (720, 350));
}

#[test]
fn page_1_needs_the_configuration_switch() {
    let mut cpu = hercules();
    cpu.bus.write_8(0xB8000, 0x5A);
    assert_ne!(cpu.bus.vga.vram_text[0x8000], 0x5A, "B8000h isn't the card's without 3BFh bit 1");
    cpu.bus.io_write(0x3BF, 0x03);
    cpu.bus.write_8(0xB8000, 0x5A);
    assert_eq!(cpu.bus.vga.vram_text[0x8000], 0x5A);
    assert_eq!(cpu.bus.read_8(0xB8000), 0x5A);
}

#[test]
fn graphics_rows_interleave_in_four_banks() {
    let mut cpu = hercules();
    // As Hercules' own software sets graphics up: allow them, program the
    // 6845, then graphics with the video on.
    cpu.bus.io_write(0x3BF, 0x01);
    cpu.bus.io_write(0x3B8, 0x02);
    for (index, &value) in CRTC_GRAPHICS.iter().enumerate() {
        cpu.bus.io_write(0x3B4, index as u8);
        cpu.bus.io_write(0x3B5, value);
    }
    cpu.bus.io_write(0x3B8, 0x0A);
    assert_eq!(cpu.bus.video_mode, VideoMode::HercGraphics);
    assert_eq!(video::frame_size(&cpu.bus), (720, 348));
    // Row 1 is in the second bank, row 4 the second row of the first.
    cpu.bus.write_8(0xB0000 + 0x2000, 0x80);
    cpu.bus.write_8(0xB0000 + 90, 0x01);
    let frame = picture(&mut cpu);
    assert_eq!(pixel(&frame, 0, 1), NORMAL);
    assert_eq!(pixel(&frame, 0, 0), BLACK);
    assert_eq!(pixel(&frame, 7, 4), NORMAL);
    assert_eq!(cpu.bus.vga.timing().display, 348);
}

/// The colours in the text cell at (`col`, 0), 9x14 pixels.
fn cell(frame: &Frame, col: usize) -> Vec<Vec<(u8, u8, u8)>> {
    (0..14).map(|y| (0..9).map(|x| pixel(frame, col * 9 + x, y)).collect()).collect()
}

#[test]
fn mda_attributes() {
    let mut cpu = hercules();
    let put = |cpu: &mut Cpu, col: usize, ch: u8, attr: u8| {
        cpu.bus.write_8(0xB0000 + col * 2, ch);
        cpu.bus.write_8(0xB0000 + col * 2 + 1, attr);
    };
    put(&mut cpu, 0, b'_', 0x70); // reverse: black on light
    put(&mut cpu, 1, b' ', 0x01); // underline
    put(&mut cpu, 2, 0xDB, 0x00); // nothing shows
    put(&mut cpu, 3, 0xDB, 0x0F); // bright
    put(&mut cpu, 4, 0xC4, 0x07); // a line through the ninth column
    put(&mut cpu, 5, b'-', 0x07); // but not a hyphen
    let frame = picture(&mut cpu);
    assert_eq!(cell(&frame, 0)[5], vec![NORMAL; 9], "the reverse background");
    assert!(cell(&frame, 0).iter().any(|row| row[..8].contains(&BLACK)), "the character in black");
    assert_eq!(cell(&frame, 1)[13], vec![NORMAL; 9]);
    assert_eq!(cell(&frame, 1)[12], vec![BLACK; 9]);
    assert!(cell(&frame, 2).iter().flatten().all(|&c| c == BLACK));
    // The full block is a line-drawing character: nine columns.
    assert_eq!(cell(&frame, 3)[5], vec![BRIGHT; 9]);
    let line = cell(&frame, 4).into_iter().find(|row| row[0] != BLACK).unwrap();
    assert_eq!(line, vec![NORMAL; 9]);
    let hyphen = cell(&frame, 5).into_iter().find(|row| row.iter().any(|&c| c != BLACK)).unwrap();
    assert_eq!(hyphen[8], BLACK);
}

#[test]
fn the_shell_runs_on_a_hercules_card() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    bios::install(&mut cpu.bus, VideoSetup { adapter: Adapter::Hercules, ..Default::default() });
    cpu.load_shell();
    assert_eq!(cpu.bus.video_mode, VideoMode::Mono80x25);
    video::print_string(&mut cpu, "HELLO");
    assert_eq!(cpu.bus.read_8(0xB0000), b'H');
    let frame = picture(&mut cpu);
    assert!(frame.rgb[..720 * 14 * 3].iter().any(|&b| b != 0));
    assert_eq!(cpu.bus.read_16(0x0460), 0x0B0C);
}

#[test]
fn programs_set_up_other_graphics_shapes() {
    let mut cpu = hercules();
    cpu.bus.io_write(0x3BF, 0x01);
    cpu.bus.io_write(0x3B8, 0x02);
    // 640x300, as MicroProse's F-15 Strike Eagle II has it: 40 characters
    // of 16 pixels, 75 rows of 4 scanlines.
    let mut regs = CRTC_GRAPHICS;
    regs[1] = 0x28;
    regs[6] = 0x4B;
    for (index, &value) in regs.iter().enumerate() {
        cpu.bus.io_write(0x3B4, index as u8);
        cpu.bus.io_write(0x3B5, value);
    }
    cpu.bus.io_write(0x3B8, 0x0A);
    assert_eq!(video::frame_size(&cpu.bus), (640, 300));
    // Rows of 80 bytes: row 4 starts at byte 80 of the first bank.
    cpu.bus.write_8(0xB0000 + 80, 0x80);
    let frame = picture(&mut cpu);
    assert_eq!(pixel(&frame, 0, 4), NORMAL);
}
