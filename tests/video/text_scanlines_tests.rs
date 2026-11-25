//! A VGA's text modes at the scanlines INT 10h AH=12h BL=30h selects for
//! the next mode set: 200, 350 or 400, and the rows a font gives in them.

use iced_x86::Register;
use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::bios;
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

#[test]
fn the_selection_sets_the_ega_switches_too() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    let high = cpu.bus.read_8(0x0488) & 0xF0;
    // 200 lines are the EGA's CGA emulation (switches 8), which AH=12h
    // BL=10h returns in CL.
    int10(&mut cpu, 0x1200, 0x0030);
    assert_eq!(cpu.bus.read_8(0x0488), high | 0x08);
    int10(&mut cpu, 0x1200, 0x0010);
    assert_eq!(cpu.get_reg8(Register::CL), 0x08);
    // 350 and 400 lines the enhanced display (9).
    int10(&mut cpu, 0x1201, 0x0030);
    assert_eq!(cpu.bus.read_8(0x0488), high | 0x09);
    int10(&mut cpu, 0x1200, 0x0030);
    int10(&mut cpu, 0x1202, 0x0030);
    assert_eq!(cpu.bus.read_8(0x0488), high | 0x09);
}

#[test]
fn a_font_taller_than_the_lines_shown_leaves_one_row() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    int10(&mut cpu, 0x0003, 0);
    // A program's CRTC showing 8 lines, with no overflow bits.
    cpu.bus.vga.crtc_regs[0x07] = 0;
    cpu.bus.vga.crtc_regs[0x12] = 7;
    int10(&mut cpu, 0x1114, 0);
    assert_eq!(rows_and_height(&cpu), (1, 16));
}

#[test]
fn the_200_line_text_modes_take_the_cga_monitors_colors() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    int10(&mut cpu, 0x1200, 0x0030);
    int10(&mut cpu, 0x0003, 0);
    // Like the other 200-line modes: brown is palette register 06h and the
    // bright colors 10h-17h, in the DAC as a CGA monitor shows them.
    let attr = &cpu.bus.vga.attribute_regs;
    assert_eq!(
        (attr[0x06], attr[0x08], attr[0x0F], attr[0x10]),
        (0x06, 0x10, 0x17, 0x08)
    );
    let dac = |index: usize| cpu.bus.vga.palette[index * 3..index * 3 + 3].to_vec();
    assert_eq!(dac(0x06), [0x2A, 0x15, 0x00]);
    assert_eq!(dac(0x17), [0x3F, 0x3F, 0x3F]);
    // 350 lines keep the EGA's 64 colors, brown at 14h.
    int10(&mut cpu, 0x1201, 0x0030);
    int10(&mut cpu, 0x0003, 0);
    assert_eq!(cpu.bus.vga.attribute_regs[0x06], 0x14);
    assert_eq!(
        cpu.bus.vga.palette[0x14 * 3..0x14 * 3 + 3],
        [0x2A, 0x15, 0x00]
    );
}

#[test]
fn the_state_information_tells_the_rows_and_scanlines() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    let state = |cpu: &mut Cpu| {
        cpu.set_es(0x3000);
        cpu.set_reg16(Register::DI, 0);
        int10(cpu, 0x1B00, 0);
        assert_eq!(cpu.get_al(), 0x1B);
        let at = |offset: usize| cpu.bus.read_8(0x30000 + offset);
        (at(0x04), at(0x05), at(0x22), at(0x23), at(0x2A))
    };
    // MODE CON LINES=43: 43 rows of the 8x8 font on 350 lines.
    int10(&mut cpu, 0x1201, 0x0030);
    int10(&mut cpu, 0x0003, 0);
    int10(&mut cpu, 0x1112, 0);
    assert_eq!(state(&mut cpu), (0x03, 80, 43, 8, 1));
    int10(&mut cpu, 0x1200, 0x0030);
    int10(&mut cpu, 0x0001, 0);
    assert_eq!(state(&mut cpu), (0x01, 40, 25, 8, 0));
    int10(&mut cpu, 0x1202, 0x0030);
    int10(&mut cpu, 0x0003, 0);
    assert_eq!(state(&mut cpu), (0x03, 80, 25, 16, 2));
    int10(&mut cpu, 0x0012, 0);
    assert_eq!(state(&mut cpu).4, 3);
    int10(&mut cpu, 0x0013, 0);
    assert_eq!(state(&mut cpu).4, 0);
}

#[test]
fn the_ega_has_no_double_scan_bit() {
    let mut cpu = Cpu::new(PathBuf::from("."));
    bios::install(
        &mut cpu.bus,
        VideoSetup {
            adapter: Adapter::Ega,
            ..Default::default()
        },
    );
    int10::set_mode(&mut cpu, 0x03);
    assert_eq!(cpu.bus.vga.text_scanlines(), 350);
    // Maximum Scan Line bit 7, set by a program written for a VGA, scans
    // nothing twice on an EGA.
    cpu.bus.vga.crtc_regs[0x09] |= 0x80;
    assert_eq!(cpu.bus.vga.text_scanlines(), 350);
}
