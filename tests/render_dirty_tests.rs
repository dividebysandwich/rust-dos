//! Text written straight into text VRAM (shell output, DOS character
//! output) must mark its rows dirty, or the dirty-rect renderer never
//! repaints it and the output stays black on screen.

use rust_dos::command::CommandDispatcher;
use rust_dos::cpu::Cpu;
use rust_dos::video::{self, Frame, SCREEN_HEIGHT, SCREEN_WIDTH};
use std::fs;
use std::path::PathBuf;

const CELL_H: usize = 16;

fn scratch(name: &str) -> PathBuf {
    let base = PathBuf::from("target/test_render_dirty").join(name);
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    base
}

/// A shell-ready CPU whose frame has been rendered once, so later renders
/// only repaint what gets marked dirty (like the main loop).
fn rendered_shell(name: &str) -> (Cpu, Frame) {
    let mut cpu = Cpu::new(scratch(name));
    cpu.load_shell();
    let mut frame = Frame::new(SCREEN_WIDTH, SCREEN_HEIGHT);
    video::render_screen(&mut frame, &cpu.bus);
    cpu.bus.vga.clear_dirty();
    (cpu, frame)
}

/// True if any pixel in text row `row` is lit.
fn row_has_pixels(frame: &Frame, row: usize) -> bool {
    let row_bytes = SCREEN_WIDTH as usize * 3;
    frame.rgb[row * CELL_H * row_bytes..(row + 1) * CELL_H * row_bytes]
        .iter()
        .any(|&b| b != 0)
}

#[test]
fn print_string_output_is_repainted() {
    let (mut cpu, mut frame) = rendered_shell("print_string");
    cpu.bus.cursor_y = 3;
    video::print_string(&mut cpu, "Bad command or file name.\r\nsecond line");
    video::render_screen(&mut frame, &cpu.bus);
    assert!(row_has_pixels(&frame, 3));
    assert!(row_has_pixels(&frame, 4));
    assert!(!row_has_pixels(&frame, 6));
}

#[test]
fn builtin_command_output_is_repainted() {
    let (mut cpu, mut frame) = rendered_shell("dir");
    fs::write(cpu.bus.disk.root_path().join("A.TXT"), b"x").unwrap();
    assert!(CommandDispatcher::new().dispatch(&mut cpu, "DIR", ""));
    video::render_screen(&mut frame, &cpu.bus);
    // Volume line, directory line, blank, entry, totals, free space
    for row in [0, 1, 3, 4, 5] {
        assert!(row_has_pixels(&frame, row), "row {} is blank", row);
    }
}

#[test]
fn print_char_and_scrolling_are_repainted() {
    let (mut cpu, mut frame) = rendered_shell("print_char");
    cpu.bus.cursor_y = 10;
    video::print_char(&mut cpu.bus, b'X');
    video::render_screen(&mut frame, &cpu.bus);
    assert!(row_has_pixels(&frame, 10));

    // Printing past the last row scrolls: everything moves up one row.
    cpu.bus.vga.clear_dirty();
    cpu.bus.cursor_x = 0;
    cpu.bus.cursor_y = 24;
    video::print_string(&mut cpu, "bottom\r\n");
    video::render_screen(&mut frame, &cpu.bus);
    assert!(row_has_pixels(&frame, 9), "X should have scrolled to row 9");
    assert!(!row_has_pixels(&frame, 10));
    assert!(row_has_pixels(&frame, 23));
}

#[test]
fn shell_reload_clears_the_old_screen_of_another_text_mode() {
    let (mut cpu, mut frame) = rendered_shell("reload");
    video::print_string(&mut cpu, "left over from a program");
    video::render_screen(&mut frame, &cpu.bus);
    assert!(row_has_pixels(&frame, 0));

    // It ended with 50 rows.
    cpu.bus.write_8(0x0484, 49);
    cpu.bus.vga.clear_dirty();
    cpu.load_shell();
    video::render_screen(&mut frame, &cpu.bus);
    assert!(!row_has_pixels(&frame, 0));
}

/// The 16-color renderer redraws only the rows whose plane bytes changed,
/// and what it leaves must be what a full render draws: after a sprite, a
/// palette change, a scroll, panning and a split screen, the frame drawn
/// bit by bit is the frame drawn from scratch.
#[test]
fn planar_frames_drawn_row_by_row_match_full_renders() {
    use rust_dos::interrupts::int10;
    for mode in [0x0Du8, 0x10, 0x12] {
        let mut cpu = Cpu::new(scratch(&format!("planar_{mode:02X}")));
        int10::set_mode(&mut cpu, mode);
        let bytes_per_row = cpu.bus.vga.graphics_size().0 / 8;
        let planes = |cpu: &mut Cpu, mask: u8| {
            cpu.bus.io_write(0x3C4, 0x02);
            cpu.bus.io_write(0x3C5, mask);
        };
        let mut seed = 0x2545_F491u32;
        for plane in 0..4 {
            planes(&mut cpu, 1 << plane);
            for at in 0..0x10000 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                cpu.bus.write_8(0xA0000 + at, seed as u8);
            }
        }
        let (w, h) = video::frame_size(&cpu.bus);
        let mut frame = Frame::new(w, h);
        let mut check = |cpu: &mut Cpu, what: &str| {
            video::render_screen(&mut frame, &cpu.bus);
            cpu.bus.vga.clear_dirty();
            let mut fresh = Frame::new(w, h);
            cpu.bus.vga.mark_dirty_full();
            video::render_screen(&mut fresh, &cpu.bus);
            cpu.bus.vga.clear_dirty();
            assert!(frame == fresh, "mode {mode:02X}: {what}");
        };
        check(&mut cpu, "first frame");
        // A sprite: four bytes wide, a plane at a time.
        for plane in 0..4u8 {
            planes(&mut cpu, 1 << plane);
            for y in 40..72 {
                for x in 10..14 {
                    cpu.bus.write_8(0xA0000 + y * bytes_per_row + x, 0xA5 ^ plane);
                }
            }
        }
        check(&mut cpu, "sprite");
        planes(&mut cpu, 0x0F);
        cpu.bus.write_8(0xA0000 + 3 * bytes_per_row, 0x55);
        check(&mut cpu, "one byte in all planes");
        // Palette register 1 to another colour.
        cpu.bus.io_read(0x3DA);
        cpu.bus.io_write(0x3C0, 0x01);
        cpu.bus.io_write(0x3C0, 0x3F);
        cpu.bus.io_write(0x3C0, 0x20);
        check(&mut cpu, "palette");
        // Scrolled down a row and three bytes.
        let start = bytes_per_row + 3;
        cpu.bus.io_write(0x3D4, 0x0C);
        cpu.bus.io_write(0x3D5, (start >> 8) as u8);
        cpu.bus.io_write(0x3D4, 0x0D);
        cpu.bus.io_write(0x3D5, start as u8);
        cpu.bus.vga.latch_start_address();
        check(&mut cpu, "scroll");
        // Panned by three pixels.
        cpu.bus.io_read(0x3DA);
        cpu.bus.io_write(0x3C0, 0x33);
        cpu.bus.io_write(0x3C0, 0x03);
        cpu.bus.io_write(0x3C0, 0x20);
        check(&mut cpu, "pan");
        cpu.bus.io_read(0x3DA);
        cpu.bus.io_write(0x3C0, 0x33);
        cpu.bus.io_write(0x3C0, 0x05);
        cpu.bus.io_write(0x3C0, 0x20);
        check(&mut cpu, "more pan");
        // A split screen from scanline 100 on.
        cpu.bus.io_write(0x3D4, 0x18);
        cpu.bus.io_write(0x3D5, 100);
        check(&mut cpu, "split");
    }
}

/// The CGA's composite picture is redrawn only where its lines changed,
/// and what it leaves must be what a full render draws: after a box, a
/// colour and palette change, the burst off and on, and a scroll.
#[test]
fn cga_composite_frames_drawn_line_by_line_match_full_renders() {
    use rust_dos::interrupts::int10;
    use rust_dos::video::adapter::{Adapter, VideoSetup};
    use rust_dos::video::bios;
    use rust_dos::video::composite::{CompositeEra, CompositeMode, CompositeSettings};
    for (mode, control) in [(0x04u8, 0x0Au8), (0x06, 0x1A)] {
        let mut cpu = Cpu::new(scratch(&format!("composite_{mode:02X}")));
        bios::install(&mut cpu.bus, VideoSetup { adapter: Adapter::Cga, ..Default::default() });
        int10::set_mode(&mut cpu, mode);
        cpu.bus.io_write(0x3D8, control);
        cpu.bus.vga.set_composite(CompositeSettings { mode: CompositeMode::On, era: CompositeEra::Old });
        assert!(cpu.bus.vga.composite_active());
        for offset in 0..0x4000 {
            cpu.bus.write_8(0xB8000 + offset, (offset * 13 + offset / 80) as u8);
        }
        let mut frame = Frame::new(SCREEN_WIDTH, SCREEN_HEIGHT);
        let mut check = |cpu: &mut Cpu, what: &str| {
            video::render_screen(&mut frame, &cpu.bus);
            cpu.bus.vga.clear_dirty();
            let mut fresh = Frame::new(SCREEN_WIDTH, SCREEN_HEIGHT);
            cpu.bus.vga.mark_dirty_full();
            video::render_screen(&mut fresh, &cpu.bus);
            cpu.bus.vga.clear_dirty();
            assert!(frame == fresh, "mode {mode:02X}: {what}");
        };
        check(&mut cpu, "first frame");
        for bank in [0, 0x2000] {
            for y in 20..36 {
                for x in 30..34 {
                    cpu.bus.write_8(0xB8000 + bank + y * 80 + x, 0x5A);
                }
            }
        }
        check(&mut cpu, "box");
        // Another background and palette (mode 4) or foreground (mode 6).
        cpu.bus.io_write(0x3D9, 0x21);
        check(&mut cpu, "colour select");
        // The burst off: a grey picture.
        cpu.bus.io_write(0x3D8, control | 0x04);
        check(&mut cpu, "burst off");
        cpu.bus.io_write(0x3D8, control);
        check(&mut cpu, "burst on");
        // Started a line further on.
        cpu.bus.io_write(0x3D4, 0x0C);
        cpu.bus.io_write(0x3D5, 0);
        cpu.bus.io_write(0x3D4, 0x0D);
        cpu.bus.io_write(0x3D5, 40);
        cpu.bus.vga.latch_start_address();
        check(&mut cpu, "scroll");
    }
}
