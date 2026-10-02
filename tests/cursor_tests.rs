//! The text cursor as the screen shows it: on an EGA or VGA, a shape set
//! in a CGA's 8 lines (INT 10h AH=01h, CX=0607h) is the same shape in the
//! font's lines, at the bottom of the cell, not lines 6 and 7 of 16.

use rust_dos::cpu::Cpu;
use rust_dos::interrupts::int10;
use rust_dos::video::adapter::{Adapter, VideoSetup};
use rust_dos::video::{self, Frame};
use std::path::PathBuf;

fn machine(adapter: Adapter) -> Cpu {
    let mut cpu = Cpu::new(PathBuf::from("target/test_cursor"));
    video::bios::install(&mut cpu.bus, VideoSetup { adapter, ..Default::default() });
    cpu.load_shell();
    cpu
}

fn int10(cpu: &mut Cpu, ax: u16, bx: u16, cx: u16) {
    cpu.set_ax(ax);
    cpu.set_reg16(iced_x86::Register::BX, bx);
    cpu.set_cx(cx);
    int10::handle(cpu);
}

/// The scanlines of the cell at the cursor (0, 0) the cursor lights.
fn cursor_lines(cpu: &Cpu) -> Vec<usize> {
    let geometry = video::text::geometry(&cpu.bus).unwrap();
    let mut frame = Frame::new(video::SCREEN_WIDTH, video::SCREEN_HEIGHT);
    video::overlay::draw_cursors(&mut frame, &cpu.bus, true);
    let width = frame.width as usize;
    let mut lines: Vec<usize> = (0..geometry.cell_h())
        .filter(|&y| frame.rgb[y * width * 3..y * width * 3 + 3] == [0xDD; 3])
        .map(|y| y / geometry.y_scale)
        .collect();
    lines.dedup();
    lines
}

#[test]
fn a_cga_shape_is_at_the_bottom_of_a_vga_cell() {
    let mut cpu = machine(Adapter::Vga);
    int10(&mut cpu, 0x0003, 0, 0);
    assert_eq!(cursor_lines(&cpu), vec![13, 14], "after a mode set");
    int10(&mut cpu, 0x0100, 0, 0x0607);
    assert_eq!(cursor_lines(&cpu), vec![13, 14], "the CGA's underline");
    int10(&mut cpu, 0x0100, 0, 0x0007);
    assert_eq!(cursor_lines(&cpu), (0..=15).collect::<Vec<_>>(), "the CGA's block");
    int10(&mut cpu, 0x0100, 0, 0x2000);
    assert_eq!(cursor_lines(&cpu), Vec::<usize>::new(), "hidden");
    // BDA 0460h keeps what was asked for.
    int10(&mut cpu, 0x0100, 0, 0x0607);
    assert_eq!(cpu.bus.read_16(0x0460), 0x0607);
}

#[test]
fn an_8_line_font_keeps_its_cursor_at_the_bottom() {
    let mut cpu = machine(Adapter::Vga);
    int10(&mut cpu, 0x0003, 0, 0);
    // AX=1112h: the 8x8 font, 50 rows.
    int10(&mut cpu, 0x1112, 0, 0);
    assert_eq!(cursor_lines(&cpu), vec![6, 7]);
    int10(&mut cpu, 0x0100, 0, 0x0607);
    assert_eq!(cursor_lines(&cpu), vec![6, 7]);
}

#[test]
fn the_ega_cell_is_14_lines() {
    let mut cpu = machine(Adapter::Ega);
    int10(&mut cpu, 0x0003, 0, 0);
    int10(&mut cpu, 0x0100, 0, 0x0607);
    assert_eq!(cursor_lines(&cpu), vec![11, 12]);
}

#[test]
fn the_cga_takes_the_shape_as_it_is() {
    let mut cpu = machine(Adapter::Cga);
    int10(&mut cpu, 0x0003, 0, 0);
    int10(&mut cpu, 0x0100, 0, 0x0607);
    assert_eq!(cursor_lines(&cpu), vec![6, 7]);
}

#[test]
fn the_prompt_starts_with_the_bottom_cursor() {
    let cpu = machine(Adapter::Vga);
    assert_eq!(cursor_lines(&cpu), vec![13, 14]);
}
