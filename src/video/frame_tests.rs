use super::*;

#[test]
fn new_frames_are_opaque_black_rgba() {
    let frame = Frame::new(3, 2);
    assert_eq!(frame.rgba.len(), 24);
    assert!(
        frame
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [0, 0, 0, 255])
    );
    assert!(Frame::new(0, 0).rgba.is_empty());
}

#[test]
fn rgb_import_and_clones_keep_four_bytes_per_pixel() {
    let mut frame = Frame::from_rgb(2, 1, vec![1, 2, 3, 4, 5, 6]);
    frame.doubled = true;
    assert_eq!(frame.rgba, [1, 2, 3, 255, 4, 5, 6, 255]);
    let copied = frame.clone();
    assert_eq!(copied, frame);
    assert!(copied.doubled);
    let mut resized = Frame::new(1, 2);
    resized.clone_from(&frame);
    assert_eq!(resized, frame);
    assert!(resized.doubled);
    assert!(!resized.resize(2, 1));
    assert!(resized.resize(1, 1));
    assert_eq!(resized.rgba, [0, 0, 0, 255]);
}

#[test]
fn clearing_invalidates_the_decoders_source_cache() {
    let mut frame = Frame::new(2, 2);
    assert!(!frame.drawn_from.reuse(&[17], 4));
    assert!(frame.drawn_from.reuse(&[17], 4));
    frame.clear([11, 22, 33]);
    assert!(
        frame
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| *p == [11, 22, 33, 255])
    );
    assert!(!frame.drawn_from.reuse(&[17], 4));
}

#[test]
fn overlay_coverage_survives_clones_and_clears_when_base_rows_are_restored() {
    let mut frame = Frame::new(2, 2);
    frame.track_overlays(true);
    frame.mark_overlay(1, 0);
    let mut copy = frame.clone();
    assert_eq!(copy.overlay_mask().unwrap(), &[0, 1, 0, 0]);
    let mut clone_from = Frame::new(1, 1);
    clone_from.clone_from(&frame);
    assert_eq!(clone_from.overlay_mask().unwrap(), &[0, 1, 0, 0]);
    clone_from.mark_overlay(1, 0);
    clone_from.clone_from(&Frame::new(2, 2));
    assert_eq!(clone_from.overlay_mask().unwrap(), &[0, 0, 0, 0]);
    copy.copy_rows_from(&Frame::new(2, 2), 0..1);
    assert_eq!(copy.overlay_mask().unwrap(), &[0, 0, 0, 0]);
    assert!(copy.resize(1, 2));
    assert_eq!(copy.overlay_mask().unwrap(), &[0, 0]);
}

#[test]
fn mode13_sprite_writes_render_only_the_affected_rows() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    crate::interrupts::int10::set_mode(&mut cpu, 0x13);
    let (width, height) = frame_size(&cpu.bus);
    let mut frame = Frame::new(width, height);
    render_screen_rows(&mut frame, &cpu.bus);
    cpu.bus.vga.clear_dirty();

    cpu.bus.write_8(0xA0000 + 40 * 320 + 17, 0x3F);
    assert_eq!((cpu.bus.vga.dirty_y_min, cpu.bus.vga.dirty_y_max), (80, 82));

    let changed = render_screen_rows(&mut frame, &cpu.bus);
    let mut reference = frame.clone();
    let reference_changed = render_screen_rows(&mut reference, &cpu.bus);
    assert_eq!(changed, 80..82);
    assert_eq!(reference_changed, 0..height as usize);
    assert_eq!(frame, reference);
}

#[test]
fn mode13_with_a_scrolled_start_keeps_full_screen_dirty_tracking() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    crate::interrupts::int10::set_mode(&mut cpu, 0x13);
    let mut frame = Frame::new(frame_size(&cpu.bus).0, frame_size(&cpu.bus).1);
    render_screen_rows(&mut frame, &cpu.bus);
    cpu.bus.vga.clear_dirty();

    cpu.bus.vga.latched_start_addr = 80;
    cpu.bus.write_8(0xA0000 + 40 * 320 + 17, 0x3F);

    assert_eq!((cpu.bus.vga.dirty_y_min, cpu.bus.vga.dirty_y_max), (0, u32::MAX));
}

/// The pixel at (`x`, `y`) of `frame`, RGB.
fn pixel(frame: &Frame, x: usize, y: usize) -> [u8; 3] {
    let at = (y * frame.width as usize + x) * 4;
    [frame.rgba[at], frame.rgba[at + 1], frame.rgba[at + 2]]
}

/// Mode 0Dh with Horizontal Display End lowered by one character, as
/// Prehistorik 2's map has it, and a picture of colour 1 in plane 0.
fn narrowed_mode_0d() -> Cpu {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    crate::interrupts::int10::set_mode(&mut cpu, 0x0D);
    cpu.bus.vga.crtc_regs[0x01] = 0x26;
    let plane_size = cpu.bus.vga.vram_graphics.len() / 4;
    cpu.bus.vga.vram_graphics[..plane_size].fill(0xFF);
    cpu.bus.vga.mark_dirty_full();
    cpu
}

#[test]
fn a_lowered_display_end_shows_the_overscan_inside_the_same_frame() {
    let mut cpu = narrowed_mode_0d();
    cpu.bus.vga.palette[27..30].copy_from_slice(&[0x3F, 0, 0]);
    cpu.bus.vga.attribute_regs[0x11] = 9;
    assert_eq!(cpu.bus.vga.graphics_size().0, 312);
    assert_eq!(frame_size(&cpu.bus), (640, 400));

    let mut frame = Frame::new(640, 400);
    render_screen_rows(&mut frame, &cpu.bus);
    let (r, g, b) = cpu.bus.vga.attribute_rgb(1);
    assert_eq!(pixel(&frame, 623, 200), [r, g, b]);
    let (r, g, b) = cpu.bus.vga.get_rgb(9);
    assert_eq!(pixel(&frame, 624, 200), [r, g, b]);
    assert_eq!(pixel(&frame, 639, 399), [r, g, b]);
    assert!(frame.doubled);
}

#[test]
fn a_new_overscan_colour_repaints_rows_whose_picture_did_not_change() {
    let mut cpu = narrowed_mode_0d();
    let mut frame = Frame::new(640, 400);
    render_screen_rows(&mut frame, &cpu.bus);
    cpu.bus.vga.clear_dirty();

    cpu.bus.vga.palette[27..30].copy_from_slice(&[0, 0x3F, 0]);
    cpu.bus.vga.attribute_regs[0x11] = 9;
    cpu.bus.vga.mark_dirty_full();
    render_screen_rows(&mut frame, &cpu.bus);
    let (r, g, b) = cpu.bus.vga.get_rgb(9);
    assert_eq!(pixel(&frame, 639, 0), [r, g, b]);
}

#[test]
fn mode13_counts_the_overscan_in_its_own_character_clocks() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    crate::interrupts::int10::set_mode(&mut cpu, 0x13);
    cpu.bus.vga.crtc_regs[0x01] = 0x4B;
    cpu.bus.vga.palette[27..30].copy_from_slice(&[0x3F, 0, 0]);
    cpu.bus.vga.attribute_regs[0x11] = 9;
    cpu.bus.vga.mark_dirty_full();
    assert_eq!(cpu.bus.vga.graphics_size().0, 304);
    assert_eq!(cpu.bus.vga.graphics_border(), 16);
    assert_eq!(frame_size(&cpu.bus), (640, 400));

    let mut frame = Frame::new(640, 400);
    render_screen_rows(&mut frame, &cpu.bus);
    let (r, g, b) = cpu.bus.vga.get_rgb(9);
    assert_eq!(pixel(&frame, 607, 10), [0, 0, 0]);
    assert_eq!(pixel(&frame, 608, 10), [r, g, b]);
    assert!(frame.doubled);
}

#[test]
fn the_bios_modes_have_no_overscan_beside_the_picture() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    for mode in [0x0D, 0x0E, 0x10, 0x12, 0x13] {
        crate::interrupts::int10::set_mode(&mut cpu, mode);
        assert_eq!(cpu.bus.vga.graphics_border(), 0, "mode {mode:02X}h");
    }
}

#[test]
fn a_mode_that_moves_the_blanking_with_the_picture_keeps_its_width() {
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    crate::interrupts::int10::set_mode(&mut cpu, 0x13);
    cpu.bus.vga.crtc_regs[0x01] = 0x3F;
    cpu.bus.vga.crtc_regs[0x02] = 0x40;
    assert_eq!(cpu.bus.vga.graphics_border(), 0);
    assert_eq!(frame_size(&cpu.bus), (512, 400));
}

#[test]
fn blanking_past_the_line_is_no_border() {
    let mut cpu = narrowed_mode_0d();
    cpu.bus.vga.crtc_regs[0x02] = 0xF0;
    assert_eq!(cpu.bus.vga.graphics_border(), 0);
}

#[test]
fn an_ega_keeps_frames_the_size_of_the_picture() {
    let mut cpu = narrowed_mode_0d();
    cpu.bus.vga.adapter = adapter::Adapter::Ega;
    assert_eq!(cpu.bus.vga.graphics_border(), 0);
    assert_eq!(frame_size(&cpu.bus), (624, 400));
}

#[test]
#[should_panic]
fn rgb_import_rejects_a_pixel_count_that_does_not_fit() {
    Frame::from_rgb(1, 1, vec![1, 2, 3, 4]);
}
