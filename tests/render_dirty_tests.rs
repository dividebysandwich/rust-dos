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
