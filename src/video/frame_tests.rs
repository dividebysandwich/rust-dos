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

#[test]
#[should_panic]
fn rgb_import_rejects_a_pixel_count_that_does_not_fit() {
    Frame::from_rgb(1, 1, vec![1, 2, 3, 4]);
}
