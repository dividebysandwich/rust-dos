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
#[should_panic]
fn rgb_import_rejects_a_pixel_count_that_does_not_fit() {
    Frame::from_rgb(1, 1, vec![1, 2, 3, 4]);
}
