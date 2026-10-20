use super::*;

fn texture(screen: &GlScreen) -> Vec<u8> {
    let (width, height) = screen.texture_size;
    let mut pixels = vec![0; (width * height * 4) as usize];
    // SAFETY: the test owns the current context and the texture.
    unsafe {
        screen
            .gl
            .bind_texture(glow::TEXTURE_2D, Some(screen.texture));
        screen.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
        screen.gl.get_tex_image(
            glow::TEXTURE_2D,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixels)),
        );
        assert_eq!(screen.gl.get_error(), glow::NO_ERROR);
    }
    pixels
}

fn double(frame: &Frame) -> Frame {
    let mut out = Frame::new(frame.width * 2, frame.height * 2);
    for y in 0..frame.height as usize {
        for x in 0..frame.width as usize {
            let from = (y * frame.width as usize + x) * 4;
            for dy in 0..2 {
                for dx in 0..2 {
                    let to = ((y * 2 + dy) * out.width as usize + x * 2 + dx) * 4;
                    out.rgba[to..to + 4].copy_from_slice(&frame.rgba[from..from + 4]);
                }
            }
        }
    }
    out.doubled = true;
    out
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn rgba_uploads_preserve_full_partial_and_subsampled_rows() {
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    let mut screen = GlScreen::open(&video, "RGBA presentation regression", (8, 8))
        .unwrap_or_else(|e| panic!("{}", e.reason));
    let mut frame = Frame::from_rgb(5, 4, (0..60).collect());
    screen.present(&frame, 0..4, (5, 4), None);
    assert_eq!(texture(&screen), frame.rgba);
    frame.rgba[40..60]
        .as_chunks_mut::<4>()
        .0
        .fill([99, 50, 20, 255]);
    screen.present(&frame, 2..3, (5, 4), None);
    assert_eq!(texture(&screen), frame.rgba);

    let mut small = Frame::from_rgb(4, 3, (40..76).collect());
    let doubled = double(&small);
    screen.present(&doubled, 0..6, (8, 6), None);
    assert_eq!(screen.texture_size, (4, 3));
    assert_eq!(texture(&screen), small.rgba);
    small.rgba[16..32]
        .as_chunks_mut::<4>()
        .0
        .fill([11, 22, 33, 255]);
    let doubled = double(&small);
    screen.present(&doubled, 2..4, (8, 6), None);
    assert_eq!(texture(&screen), small.rgba);

    // Filtering changes the upload from the native half-size picture to
    // the complete frame, even when no CPU rows need repainting.
    screen.select(Shader::None, Filter::Linear).unwrap();
    screen.present(&doubled, 0..0, (8, 6), None);
    assert_eq!(screen.texture_size, (8, 6));
    assert_eq!(texture(&screen), doubled.rgba);
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn persistent_compositions_keep_their_pixels_after_partial_presentation() {
    use crate::display::{Display, Output};
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    let textures = std::cell::OnceCell::new();
    let mut display = Display::open(
        &video,
        "Persistent composition regression",
        &Default::default(),
        &textures,
    )
    .unwrap();
    let mut frame = Frame::from_rgb(5, 4, (0..60).collect());
    display.set_frame_size(5, 4).unwrap();
    display.present_rows(&frame, None, None, 0..0).unwrap();
    let check = |display: &Display<'_>, frame: &Frame| {
        assert_eq!(display.shown, frame.rgba);
        let Output::Gl(gl) = &display.out else { panic!("OpenGL is required") };
        assert_eq!(texture(gl), frame.rgba);
    };
    check(&display, &frame);
    for step in 0..30 {
        let y = step % 4;
        frame.rgba[y * 20..(y + 1) * 20]
            .as_chunks_mut::<4>()
            .0
            .fill([step as u8, 99, 37, 255]);
        let before = frame.rgba.clone();
        display.present_rows(&frame, None, None, y..y + 1).unwrap();
        assert_eq!(frame.rgba, before, "presenting must retain the composed pixels");
        check(&display, &frame);
    }
    // A redraw and a mode change cannot rely on decoder damage alone.
    display.redraw();
    display.present_rows(&frame, None, None, 0..0).unwrap();
    check(&display, &frame);
    frame = Frame::from_rgb(4, 5, (0..60).rev().collect());
    display.set_frame_size(4, 5).unwrap();
    display.present_rows(&frame, None, None, 0..0).unwrap();
    check(&display, &frame);
}
