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
    let row_bytes = SCREEN_WIDTH as usize * 4;
    frame.rgba[row * CELL_H * row_bytes..(row + 1) * CELL_H * row_bytes]
        .as_chunks::<4>().0.iter()
        .any(|p| p[..3].iter().any(|&b| b != 0))
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
/// palette change, a scroll, panning, a row that wraps around the end of
/// the planes and a split screen moving up and down, unpanned or not, the
/// frame drawn bit by bit is the frame drawn from scratch. Over noise, and
/// over a pattern every row of which has the same bytes, where only the
/// panning tells rows apart.
#[test]
fn planar_frames_drawn_row_by_row_match_full_renders() {
    for patterned in [false, true] {
        for mode in [0x0Du8, 0x10, 0x12] {
            planar_steps(mode, patterned);
        }
    }
}

fn planar_steps(mode: u8, patterned: bool) {
    use rust_dos::interrupts::int10;
    let what = if patterned { "pattern" } else { "noise" };
    let mut cpu = Cpu::new(scratch(&format!("planar_{mode:02X}_{what}")));
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
            cpu.bus.write_8(0xA0000 + at, if patterned { 0x0F } else { seed as u8 });
        }
    }
    let (w, h) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(w, h);
    let mut check = |cpu: &mut Cpu, step: &str| {
        let previous = frame.rgba.clone();
        let rows = video::render_screen_rows(&mut frame, &cpu.bus);
        check_untouched_rows(&previous, &frame, rows);
        cpu.bus.vga.clear_dirty();
        let mut fresh = Frame::new(w, h);
        cpu.bus.vga.mark_dirty_full();
        video::render_screen(&mut fresh, &cpu.bus);
        cpu.bus.vga.clear_dirty();
        assert!(frame == fresh, "mode {mode:02X} over {what}: {step}");
        // Mode 0Dh's 320x200 is shown at 640x400, every pixel a block of
        // 2x2, which the window may show at half the size.
        assert_eq!(frame.doubled, mode == 0x0D, "mode {mode:02X} over {what}: {step}");
        if frame.doubled {
            let row = 4 * w as usize;
            for (y, line) in frame.rgba.chunks_exact(row).enumerate() {
                let above = &frame.rgba[(y & !1) * row..][..row];
                let pairs = line.as_chunks::<8>().0.iter().all(|p| p[..4] == p[4..]);
                assert!(pairs && line == above, "mode {mode:02X} over {what}: {step}, row {y} not doubled");
            }
        }
    };
    let attribute = |cpu: &mut Cpu, index: u8, value: u8| {
        cpu.bus.io_read(0x3DA);
        cpu.bus.io_write(0x3C0, index | 0x20);
        cpu.bus.io_write(0x3C0, value);
    };
    let crtc = |cpu: &mut Cpu, index: u8, value: u8| {
        cpu.bus.io_write(0x3D4, index);
        cpu.bus.io_write(0x3D5, value);
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
    attribute(&mut cpu, 0x01, 0x3F);
    check(&mut cpu, "palette");
    // Scrolled down a row and three bytes.
    let start = bytes_per_row + 3;
    crtc(&mut cpu, 0x0C, (start >> 8) as u8);
    crtc(&mut cpu, 0x0D, start as u8);
    cpu.bus.vga.latch_start_address();
    check(&mut cpu, "scroll");
    // Panned by three pixels, then five.
    attribute(&mut cpu, 0x13, 0x03);
    check(&mut cpu, "pan");
    attribute(&mut cpu, 0x13, 0x05);
    check(&mut cpu, "more pan");
    // Scrolled so that the third row runs past the end of the planes and
    // goes on at their start, then a byte changed in the part that
    // wrapped.
    let start = 0x10000 - 2 * bytes_per_row - 3;
    crtc(&mut cpu, 0x0C, (start >> 8) as u8);
    crtc(&mut cpu, 0x0D, start as u8);
    cpu.bus.vga.latch_start_address();
    check(&mut cpu, "scroll across the end of the planes");
    cpu.bus.write_8(0xA0000 + 10, 0x3C);
    check(&mut cpu, "a byte past the end of the planes");
    // A split screen from scanline `line` on: Line Compare's bits 0-7 in
    // CR18h, bit 8 in CR07h bit 4, bit 9 in CR09h bit 6.
    let split = |cpu: &mut Cpu, line: u16| {
        let regs = cpu.bus.vga.crtc_regs;
        crtc(cpu, 0x18, line as u8);
        crtc(cpu, 0x07, (regs[0x07] & !0x10) | ((line >> 4) as u8 & 0x10));
        crtc(cpu, 0x09, (regs[0x09] & !0x40) | ((line >> 3) as u8 & 0x40));
    };
    split(&mut cpu, 100);
    assert!(cpu.bus.vga.split_row() < cpu.bus.vga.graphics_size().1, "no split screen");
    check(&mut cpu, "split");
    // The split screen unpanned (Attribute Mode Control bit 5), moving.
    let mode_control = cpu.bus.vga.attribute_regs[0x10];
    attribute(&mut cpu, 0x10, mode_control | 0x20);
    check(&mut cpu, "split unpanned");
    for line in [300u16, 60, 150, 0] {
        split(&mut cpu, line);
        check(&mut cpu, &format!("split unpanned moved to {line}"));
    }
    attribute(&mut cpu, 0x10, mode_control);
    check(&mut cpu, "split panned again");
}

/// The CGA's composite picture is redrawn only where its lines changed,
/// and what it leaves must be what a full render draws: after a box, a
/// colour and palette change, the burst off and on, a scroll, the other
/// revision of the card, and the picture off and on.
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
            let previous = frame.rgba.clone();
            let rows = video::render_screen_rows(&mut frame, &cpu.bus);
            check_untouched_rows(&previous, &frame, rows);
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
        // The other revision of the card.
        cpu.bus.vga.set_composite(CompositeSettings { mode: CompositeMode::On, era: CompositeEra::New });
        check(&mut cpu, "new era");
        // The picture off (drawn black by the RGB path) and on again.
        cpu.bus.io_write(0x3D8, control & !0x08);
        check(&mut cpu, "video off");
        cpu.bus.io_write(0x3D8, control);
        check(&mut cpu, "video on");
    }
}

fn check_untouched_rows(previous: &[u8], frame: &Frame, rows: std::ops::Range<usize>) {
    let bytes = frame.width as usize * 4;
    assert!(rows.start <= rows.end && rows.end <= frame.height as usize);
    assert_eq!(&previous[..rows.start * bytes], &frame.rgba[..rows.start * bytes]);
    assert_eq!(&previous[rows.end * bytes..], &frame.rgba[rows.end * bytes..]);
}

#[test]
fn indexed_dirty_rows_include_scaled_copies_and_skip_unchanged_sources() {
    use rust_dos::interrupts::int10;
    let mut cpu = Cpu::new(scratch("indexed_rows"));
    int10::set_mode(&mut cpu, 0x13);
    let (w, h) = video::frame_size(&cpu.bus);
    let mut frame = Frame::new(w, h);
    assert_eq!(video::render_screen_rows(&mut frame, &cpu.bus), 0..h as usize);
    cpu.bus.vga.mark_dirty_full();
    assert!(video::render_screen_rows(&mut frame, &cpu.bus).is_empty());

    for row in [0, 47, 199] {
        let previous = frame.rgba.clone();
        cpu.bus.write_8(0xA0000 + row * 320 + 3, 15);
        let rows = video::render_screen_rows(&mut frame, &cpu.bus);
        assert_eq!(rows, row * 2..row * 2 + 2);
        check_untouched_rows(&previous, &frame, rows);
        let mut fresh = Frame::new(w, h);
        video::render_screen(&mut fresh, &cpu.bus);
        assert_eq!(frame, fresh);
        cpu.bus.vga.mark_dirty_full();
        assert!(video::render_screen_rows(&mut frame, &cpu.bus).is_empty());
    }
    // A palette change invalidates the cached interpretation of every row.
    cpu.bus.io_write(0x3C8, 15);
    for component in [63, 0, 0] {
        cpu.bus.io_write(0x3C9, component);
    }
    assert_eq!(video::render_screen_rows(&mut frame, &cpu.bus), 0..h as usize);
}

#[test]
fn text_dirty_rows_leave_the_rest_of_the_composed_picture_alone() {
    let (mut cpu, mut frame) = rendered_shell("text_rows");
    assert!(video::render_screen_rows(&mut frame, &cpu.bus).is_empty());
    cpu.bus.cursor_y = 10;
    video::print_char(&mut cpu.bus, b'X');
    let previous = frame.rgba.clone();
    let rows = video::render_screen_rows(&mut frame, &cpu.bus);
    assert_eq!(rows, 10 * CELL_H..11 * CELL_H);
    check_untouched_rows(&previous, &frame, rows);
}

#[test]
fn moving_and_blinking_cursors_match_full_composition_in_text_and_graphics() {
    use rust_dos::interrupts::int10;
    use rust_dos::video::{compose::Composition, mono, overlay};
    for mode in [0x03, 0x13, 0x06] {
        let mut cpu = Cpu::new(scratch(&format!("cursor_rows_{mode:02x}")));
        int10::set_mode(&mut cpu, mode);
        let (w, h) = video::frame_size(&cpu.bus);
        let mut base = Frame::new(w, h);
        let mut output = base.clone();
        let mut composition = Composition::default();
        for step in 0..60 {
            cpu.bus.write_8(0x0450, (step % 80) as u8);
            cpu.bus.write_8(0x0451, (step % 25) as u8);
            cpu.bus.mouse.installed = step > 5;
            cpu.bus.mouse.hide_counter = i32::from(step % 4 == 0);
            let (vw, vh) = cpu.bus.mouse.virtual_screen(&cpu.bus);
            cpu.bus.mouse.x = if step % 7 == 0 { -3 } else { step * 29 % vw };
            cpu.bus.mouse.y = if step % 9 == 0 { vh - 1 } else { step * 13 % vh };
            let changed = if cpu.bus.vga.dirty {
                let rows = video::render_screen_rows(&mut base, &cpu.bus);
                cpu.bus.vga.clear_dirty();
                rows
            } else {
                0..0
            };
            let visible = step % 3 != 0;
            let cursor = overlay::cursor_rows(&base, &cpu.bus, visible);
            let monitor = mono::Monochrome::ALL[(step / 8) as usize % 4];
            let rows = composition.restore(&mut output, &base, changed, cursor, monitor);
            overlay::draw_cursors(&mut output, &cpu.bus, visible);
            mono::apply_rows(&mut output, monitor, rows);
            let mut full = base.clone();
            overlay::draw_cursors(&mut full, &cpu.bus, visible);
            mono::apply(&mut full, monitor);
            assert!(output == full, "mode {mode:02x}, step {step}");
        }
    }
}
