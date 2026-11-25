//! A VGA's text modes at the scanlines INT 10h AH=12h BL=30h selects for
//! the next mode set: 200, 350 or 400, and the rows a font gives in them.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video;
use std::path::PathBuf;

/// INT 10h with the given registers.
fn int10(cpu: &mut Cpu, ax: u16, bx: u16) {
    cpu.set_ax(ax);
    cpu.set_reg16(Register::BX, bx);
    int10::handle(cpu);
}

/// The scanlines the CRTC shows: Vertical Display End and its overflow
/// bits, plus one.
fn display_end(cpu: &Cpu) -> u16 {
    let crtc = &cpu.bus.vga.crtc_regs;
    let overflow = crtc[0x07] as u16;
    (crtc[0x12] as u16 | (overflow & 0x02) << 7 | (overflow & 0x40) << 3) + 1
}

/// The rows (BDA 0484h plus one) and the font height (0485h).
fn rows_and_height(cpu: &Cpu) -> (u8, u16) {
    (cpu.bus.read_8(0x0484) + 1, cpu.bus.read_16(0x0485))
}

#[test]
fn the_8x8_font_at_350_lines_gives_43_rows() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    // What MODE CON LINES=43 and ANSI.SYS do on a VGA.
    int10(&mut cpu, 0x1201, 0x0030);
    assert_eq!(cpu.get_al(), 0x12);
    assert_eq!(cpu.bus.read_8(0x0489) & 0x90, 0x00);
    int10(&mut cpu, 0x0003, 0);
    // 25 rows of the 8x14 font, on 350 lines at 31.5 kHz and 70 Hz with
    // the 350-line sync polarity.
    assert_eq!(rows_and_height(&cpu), (25, 14));
    assert_eq!(display_end(&cpu), 350);
    assert_eq!(cpu.bus.vga.misc_output_reg, 0xA3);
    assert_eq!(cpu.bus.vga.crtc_regs[0x09] & 0x9F, 0x0D);
    assert_eq!(cpu.bus.read_16(0x0460), 0x0B0C);
    assert_eq!(video::frame_size(&cpu.bus), (640, 350));
    int10(&mut cpu, 0x1112, 0);
    assert_eq!(rows_and_height(&cpu), (43, 8));
    assert_eq!(cpu.bus.vga.crtc_regs[0x09] & 0x1F, 7);
    assert_eq!(display_end(&cpu), 350);
    let geometry = video::text::geometry(&cpu.bus).unwrap();
    assert_eq!((geometry.rows, geometry.cell_h()), (43, 8));
    assert_eq!(video::frame_size(&cpu.bus), (640, 350));
}

#[test]
fn the_selection_holds_for_later_mode_sets_until_changed() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    int10(&mut cpu, 0x1201, 0x0030);
    int10(&mut cpu, 0x0013, 0);
    // The graphics modes keep their own timing.
    assert_eq!(display_end(&cpu), 400);
    int10(&mut cpu, 0x0001, 0);
    assert_eq!(rows_and_height(&cpu), (25, 14));
    assert_eq!(display_end(&cpu), 350);
    int10(&mut cpu, 0x1202, 0x0030);
    // Only the next mode set takes it.
    assert_eq!(display_end(&cpu), 350);
    int10(&mut cpu, 0x0003, 0);
    assert_eq!(rows_and_height(&cpu), (25, 16));
    assert_eq!(display_end(&cpu), 400);
    assert_eq!(cpu.bus.read_8(0x0489) & 0x90, 0x10);
}

#[test]
fn text_at_200_lines_is_the_8x8_font_scanned_twice() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    int10(&mut cpu, 0x1200, 0x0030);
    assert_eq!(cpu.get_al(), 0x12);
    assert_eq!(cpu.bus.read_8(0x0489) & 0x90, 0x80);
    int10(&mut cpu, 0x0003, 0);
    assert_eq!(rows_and_height(&cpu), (25, 8));
    assert_eq!(cpu.bus.vga.crtc_regs[0x09], 0xC7);
    assert_eq!(cpu.bus.read_16(0x0460), 0x0607);
    // The picture is 400 lines, each character row 16 of them.
    assert_eq!(video::frame_size(&cpu.bus), (640, 400));
    let geometry = video::text::geometry(&cpu.bus).unwrap();
    assert_eq!((geometry.rows, geometry.cell_h()), (25, 16));
    // The 8x8 font loaded again leaves 25 rows of 200 lines.
    int10(&mut cpu, 0x1112, 0);
    assert_eq!(rows_and_height(&cpu), (25, 8));
}

#[test]
fn the_default_is_400_lines_and_50_rows_of_the_8x8_font() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    assert_eq!(cpu.bus.read_8(0x0489) & 0x90, 0x10);
    int10(&mut cpu, 0x0003, 0);
    assert_eq!(rows_and_height(&cpu), (25, 16));
    assert_eq!(display_end(&cpu), 400);
    assert_eq!(cpu.bus.vga.misc_output_reg, 0x67);
    int10(&mut cpu, 0x1112, 0);
    assert_eq!(rows_and_height(&cpu), (50, 8));
    assert_eq!(video::frame_size(&cpu.bus), (640, 400));
}

#[test]
fn the_monochrome_mode_has_no_200_lines() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    int10(&mut cpu, 0x1200, 0x0030);
    int10(&mut cpu, 0x0007, 0);
    assert_eq!(rows_and_height(&cpu), (25, 14));
    assert_eq!(display_end(&cpu), 350);
    assert_eq!(video::frame_size(&cpu.bus), (720, 350));
}

#[test]
fn an_unknown_selection_changes_nothing() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    int10(&mut cpu, 0x1203, 0x0030);
    assert_eq!(cpu.get_al(), 0x03);
    assert_eq!(cpu.bus.read_8(0x0489) & 0x90, 0x10);
}
