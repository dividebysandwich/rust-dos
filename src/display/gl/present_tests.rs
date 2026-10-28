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

fn reference_capture(
    screen: &mut GlScreen,
    frame: &Frame,
    base: &Frame,
    mono: Monochrome,
    display: (u32, u32),
) -> Frame {
    let (vertex, fragment) = shader::sources(Shader::Xbrz, screen.glsl);
    let preamble = screen.glsl.preamble();
    let fragment = fragment.replacen(preamble, &format!("{preamble}#define XBRZ_REFERENCE\n"), 1);
    let reference =
        compile_sources(&screen.gl, screen.glsl, "reference", (vertex, fragment)).unwrap();
    let program = screen.programs.insert(Shader::Xbrz, Ok(reference)).unwrap();
    let gpu = screen.xbrz_gpu.replace(Err("reference draw".into()));
    let result = screen.capture(frame, base, mono, display).unwrap();
    let reference = screen
        .programs
        .insert(Shader::Xbrz, program)
        .unwrap()
        .unwrap();
    screen.xbrz_gpu = gpu;
    // SAFETY: the reference program belongs to this test's current context.
    unsafe { screen.gl.delete_program(reference.program) };
    result
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn xbrz_cached_passes_match_reference_and_reuse_source_work() {
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    let mut screen = GlScreen::open(&video, "xBRZ cache regression", (32, 24))
        .unwrap_or_else(|error| panic!("{}", error.reason));
    for glsl in [Glsl::Gl130, Glsl::Gl150] {
        for pass in [
            shader::XbrzPass::Corners,
            shader::XbrzPass::Resolve,
            shader::XbrzPass::Display,
        ] {
            let sources = shader::xbrz_sources(pass, glsl);
            let program = compile_sources(&screen.gl, glsl, "xBRZ dialect", sources).unwrap();
            // SAFETY: this test owns the current context and program.
            unsafe { screen.gl.delete_program(program.program) };
        }
    }
    screen.select(Shader::Xbrz, Filter::Nearest).unwrap();
    let native = Frame::from_rgb(5, 3, (0..45).map(|i| (i * 37) as u8).collect());
    let mut base = double(&native);
    let mut composed = base.clone();
    composed.track_overlays(true);
    let draws = |screen: &GlScreen| screen.xbrz_gpu.as_ref().unwrap().as_ref().unwrap().draws;
    let compare = |screen: &mut GlScreen, frame: &Frame, base: &Frame, mono, size| {
        let actual = screen.capture(frame, base, mono, size).unwrap();
        let expected = reference_capture(screen, frame, base, mono, size);
        assert_eq!(actual, expected);
        // SAFETY: the test owns this context and checks all pass errors.
        assert_eq!(unsafe { screen.gl.get_error() }, glow::NO_ERROR);
    };
    compare(&mut screen, &composed, &base, Monochrome::Off, (32, 24));
    assert_eq!(draws(&screen), (1, 1));
    // SAFETY: this test owns the context and restores its temporary state.
    let read = unsafe {
        let read = screen.gl.create_framebuffer().unwrap();
        screen
            .gl
            .bind_framebuffer(glow::READ_FRAMEBUFFER, Some(read));
        screen.gl.enable(glow::SCISSOR_TEST);
        screen.gl.scissor(0, 0, 32, 24);
        screen.gl.enable(glow::BLEND);
        read
    };
    // Force offscreen preparation while the caller's raster state is set.
    screen
        .xbrz_gpu
        .as_mut()
        .unwrap()
        .as_mut()
        .unwrap()
        .invalidate();
    screen.draw_look(screen.texture, screen.texture_size, (0, 0, 32, 24), 1.0);
    assert_eq!(draws(&screen), (2, 2));
    // SAFETY: all queried objects belong to the test's current context.
    unsafe {
        assert!(screen.gl.is_enabled(glow::BLEND));
        assert!(screen.gl.is_enabled(glow::DITHER));
        assert!(screen.gl.is_enabled(glow::SCISSOR_TEST));
        assert_eq!(
            screen
                .gl
                .get_parameter_framebuffer(glow::READ_FRAMEBUFFER_BINDING),
            Some(read)
        );
        screen.gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
        screen.gl.delete_framebuffer(read);
        screen.gl.disable(glow::SCISSOR_TEST);
        screen.gl.disable(glow::BLEND);
    }
    composed.mark_overlay_rect(0, 0, 2, 2);
    compare(&mut screen, &composed, &base, Monochrome::Off, (32, 24));
    assert_eq!(
        draws(&screen),
        (2, 2),
        "overlay changes reuse both base passes"
    );
    for y in 0..2 {
        for x in 0..2 {
            base.rgba[(y * base.width as usize + x) * 4] ^= 127;
        }
    }
    compare(&mut screen, &composed, &base, Monochrome::Off, (32, 24));
    assert_eq!(
        draws(&screen),
        (3, 3),
        "base changes under overlays invalidate both passes"
    );
    compare(&mut screen, &composed, &base, Monochrome::Amber, (32, 24));
    assert_eq!(
        draws(&screen),
        (4, 4),
        "tint changes invalidate both passes"
    );
    screen.window.set_size(5, 3).unwrap();
    compare(&mut screen, &composed, &base, Monochrome::Amber, (5, 3));
    assert_eq!(
        draws(&screen),
        (4, 4),
        "small output reuses corners and resolves directly"
    );
    screen.window.set_size(2, 1).unwrap();
    compare(&mut screen, &composed, &base, Monochrome::Amber, (2, 1));
    assert_eq!(draws(&screen), (4, 4), "minification skips source-sized passes");
    screen.window.set_size(40, 30).unwrap();
    compare(&mut screen, &composed, &base, Monochrome::Amber, (40, 30));
    assert_eq!(
        draws(&screen),
        (4, 4),
        "larger output reuses the 2x texture"
    );
    for (width, height) in [(1, 7), (7, 1), (1, 1)] {
        let base = Frame::from_rgba(
            width,
            height,
            (0..width * height)
                .flat_map(|i| [(i * 37) as u8, 40, 80, (i * 53) as u8])
                .collect(),
        );
        compare(&mut screen, &base, &base, Monochrome::Off, (40, 30));
    }
    let texture = screen.texture;
    let gl = &screen.gl;
    let cache = screen.xbrz_gpu.as_mut().unwrap().as_mut().unwrap();
    assert!(
        cache
            .draw(gl, texture, (u32::MAX, 1), (0, 0, 1, 1), None)
            .is_err()
    );
    let before = draws(&screen);
    // SAFETY: the test owns the context and this temporary 1x1 texture.
    let external = unsafe {
        let texture = screen.gl.create_texture().unwrap();
        screen.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        screen.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::NEAREST as i32,
        );
        screen.gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            1,
            1,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&[33, 99, 170, 255])),
        );
        texture
    };
    screen.draw_look(external, (1, 1), (0, 0, 32, 24), 1.0);
    screen.draw_look(external, (1, 1), (0, 0, 32, 24), 1.0);
    screen.draw_look(screen.texture, (1, 1), (0, 0, 32, 24), 1.0);
    assert_eq!(draws(&screen), (before.0 + 3, before.1 + 3));
    // SAFETY: the external texture belongs to this test's context.
    unsafe { screen.gl.delete_texture(external) };
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

    // xBRZ needs native pixels even when the plain filter is linear.
    screen.select(Shader::Xbrz, Filter::Linear).unwrap();
    screen.present(&doubled, 0..0, (8, 6), None);
    assert_eq!(screen.texture_size, (4, 3));
    assert_eq!(texture(&screen), small.rgba);
    small.rgba[16..32]
        .as_chunks_mut::<4>()
        .0
        .fill([55, 66, 77, 255]);
    let doubled = double(&small);
    screen.present(&doubled, 3..4, (8, 6), None);
    assert_eq!(texture(&screen), small.rgba);
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn xbrz_overlays_preserve_native_input_and_filtered_background() {
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    let native = Frame::from_rgb(
        8,
        6,
        (0..48)
            .flat_map(|i| {
                if i % 9 < 4 {
                    [200, 40, 40]
                } else {
                    [30, 60, 200]
                }
            })
            .collect(),
    );
    let base = double(&native);
    let mut screen = GlScreen::open(&video, "xBRZ overlay regression", (16, 12))
        .unwrap_or_else(|e| panic!("{}", e.reason));
    screen.select(Shader::Xbrz, Filter::Nearest).unwrap();
    let layer = Layer {
        rect: (4.0, 4.0, 2.0, 2.0),
        picture: Frame::from_rgb(1, 1, vec![20, 220, 80]),
        generation: 1,
    };
    use crate::video::mono::{self, Monochrome};
    for (overlay, manual, mono) in [
        (false, false, Monochrome::Off),
        (true, false, Monochrome::Off),
        (false, false, Monochrome::Off),
        (true, true, Monochrome::Off),
        (false, false, Monochrome::White),
        (false, false, Monochrome::Amber),
        (true, false, Monochrome::Green),
        (true, true, Monochrome::White),
    ] {
        let mut composed = base.clone();
        composed.track_overlays(true);
        mono::apply(&mut composed, mono);
        let mut tinted = native.clone();
        mono::apply(&mut tinted, mono);
        let mut expected = screen.capture(&composed, &base, mono, (16, 12)).unwrap().rgba;
        assert_ne!(expected, composed.rgba, "xBRZ must smooth the diagonal edges");
        if overlay {
            composed.doubled = false;
            composed.rgba[68..72].copy_from_slice(&[11, 222, 33, 255]);
            composed.mark_overlay(1, 1);
            expected[68..72].copy_from_slice(&[11, 222, 33, 255]);
        }
        if manual {
            for y in 4..6 {
                for x in 4..6 {
                    expected[(y * 16 + x) * 4..(y * 16 + x + 1) * 4]
                        .copy_from_slice(&[20, 220, 80, 255]);
                }
            }
            composed.mark_overlay_rect(4, 4, 2, 2);
        }
        screen
            .present_xbrz(
                &composed,
                &base,
                0..12,
                (16, 12),
                manual.then_some(&layer),
                mono,
            )
            .unwrap();
        assert_eq!(screen.texture_size, (8, 6));
        assert_eq!(texture(&screen), tinted.rgba);
        let mut pixels = vec![0; expected.len()];
        // SAFETY: the test owns the current context and reads its window.
        unsafe {
            screen.gl.read_buffer(glow::BACK);
            screen.gl.read_pixels(
                0,
                0,
                16,
                12,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
            assert_eq!(screen.gl.get_error(), glow::NO_ERROR);
        }
        let top_first: Vec<_> = pixels
            .as_chunks::<64>()
            .0
            .iter()
            .rev()
            .flatten()
            .copied()
            .collect();
        let differences: Vec<_> = top_first
            .iter()
            .zip(&expected)
            .enumerate()
            .filter(|(_, (got, want))| got != want)
            .take(8)
            .collect();
        assert!(
            differences.is_empty(),
            "overlay: {overlay}, manual: {manual}, mono: {mono:?}: {differences:?}"
        );
        if manual {
            for y in 4..6 {
                for x in 4..6 {
                    composed.rgba[(y * 16 + x) * 4..(y * 16 + x + 1) * 4]
                        .copy_from_slice(&[20, 220, 80, 255]);
                }
            }
        }
        let captured = screen.capture(&composed, &base, mono, (16, 12)).unwrap();
        assert_eq!(captured.rgba, expected, "capture, mono: {mono:?}");
    }
    let doubled = screen.capture(&base, &base, Monochrome::Off, (16, 12)).unwrap();
    let undoubled = screen.capture(&native, &native, Monochrome::Off, (8, 6)).unwrap();
    assert_eq!(doubled.rgba, undoubled.rgba);
    screen
        .present_xbrz(&native, &native, 0..6, (8, 6), None, Monochrome::Off)
        .unwrap();
    assert_eq!(texture(&screen), native.rgba);
    screen.select(Shader::Scanlines, Filter::Nearest).unwrap();
    let captured = screen.capture(&base, &base, Monochrome::Off, (16, 12)).unwrap();
    assert_eq!((captured.width, captured.height), (16, 12));
    assert!(captured.rgba.as_chunks::<4>().0.iter().any(|p| p[..3] != [0; 3]));
    screen.select(Shader::None, Filter::Nearest).unwrap();
    assert!(screen.capture(&base, &base, Monochrome::Off, (16, 12)).is_none());
}

#[test]
#[ignore = "requires an OpenGL display; run in CI under Xvfb"]
fn xbrz_near_colour_edges_use_exact_colour_inequality() {
    use crate::video::mono::Monochrome;
    let sdl = sdl2::init().unwrap();
    let video = sdl.video().unwrap();
    let values = [
        160, 32, 160, 0, 32, 32, 16, 16, 160, 0, 32, 16, 0, 0, 16, 255, 16, 0, 16, 255, 16,
        32, 160, 255, 160,
    ];
    let native = Frame::from_rgb(5, 5, values.into_iter().flat_map(|v| [v, v, v]).collect());
    let base = double(&native);
    let mut screen = GlScreen::open(&video, "xBRZ near colour regression", (10, 10))
        .unwrap_or_else(|e| panic!("{}", e.reason));
    screen.select(Shader::Xbrz, Filter::Nearest).unwrap();
    let output = screen.capture(&base, &base, Monochrome::Off, (10, 10)).unwrap();
    assert_eq!(&output.rgba[(2 * 10 + 7) * 4..][..3], &[40, 40, 40]);
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
    display.present_rows(&frame, None, None, 0..0, None).unwrap();
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
        display.present_rows(&frame, None, None, y..y + 1, None).unwrap();
        assert_eq!(frame.rgba, before, "presenting must retain the composed pixels");
        check(&display, &frame);
    }
    // A redraw and a mode change cannot rely on decoder damage alone.
    display.redraw();
    display.present_rows(&frame, None, None, 0..0, None).unwrap();
    check(&display, &frame);
    frame = Frame::from_rgb(4, 5, (0..60).rev().collect());
    display.set_frame_size(4, 5).unwrap();
    display.present_rows(&frame, None, None, 0..0, None).unwrap();
    check(&display, &frame);
    // Exercise the frontend's settings and clean-base handoff, including
    // a partial native row update and a return to the plain renderer.
    use crate::config::Settings;
    use crate::video::mono::{self, Monochrome};
    let settings = Settings {
        shader: Shader::Xbrz,
        monochrome: Monochrome::Green,
        ..Default::default()
    };
    display.apply(&settings).unwrap();
    for (step, rows) in [0..frame.height as usize * 2, 2..4].into_iter().enumerate() {
        frame.rgba[16..32].as_chunks_mut::<4>().0.fill([40 + step as u8 * 40, 80, 120, 255]);
        let base = double(&frame);
        let mut composed = base.clone();
        mono::apply(&mut composed, settings.monochrome);
        display.set_frame_size(composed.width, composed.height).unwrap();
        display.present_rows(&composed, None, None, rows, Some(&base)).unwrap();
        let mut tinted = frame.clone();
        mono::apply(&mut tinted, settings.monochrome);
        let Output::Gl(gl) = &display.out else { panic!("OpenGL is required") };
        assert_eq!(texture(gl), tinted.rgba);
        assert!(display.shaded(&composed, &base).is_some());
    }
    let base = double(&frame);
    let mut composed = base.clone();
    composed.track_overlays(true);
    display.set_frame_size(composed.width, composed.height).unwrap();
    display.present_rows(&composed, None, None, 0..composed.height as usize, Some(&base)).unwrap();
    composed.mark_overlay(2, 2);
    display.present_rows(&composed, None, None, 2..3, Some(&base)).unwrap();
    let Output::Gl(gl) = &display.out else { unreachable!() };
    assert_eq!(gl.xbrz_frame.overlay.rgba[(2 * composed.width as usize + 2) * 4 + 3], 255);
    composed.track_overlays(false);
    composed.track_overlays(true);
    display.present_rows(&composed, None, None, 2..3, Some(&base)).unwrap();
    let Output::Gl(gl) = &display.out else { unreachable!() };
    assert_eq!(gl.xbrz_frame.overlay.rgba[(2 * composed.width as usize + 2) * 4 + 3], 0);
    composed.mark_overlay(2, 2);
    display.present_rows(&composed, None, None, 2..3, Some(&base)).unwrap();
    let source = match &display.out {
        Output::Gl(gl) => gl.xbrz_frame.source.clone(),
        _ => unreachable!(),
    };
    let mut changed_base = base.clone();
    changed_base.rgba[(2 * changed_base.width as usize + 2) * 4..][..3]
        .copy_from_slice(&[250, 1, 2]);
    display.present_rows(&composed, None, None, 2..3, Some(&changed_base)).unwrap();
    let Output::Gl(gl) = &display.out else { unreachable!() };
    assert_ne!(gl.xbrz_frame.source, source, "base updates under a stable overlay reach xBRZ");
    assert_eq!(&gl.xbrz_frame.source.rgba[(4 + 1) * 4..][..3], &[15, 76, 15]);
    display.apply(&Default::default()).unwrap();
    display.set_frame_size(frame.width, frame.height).unwrap();
    display.present(&mut frame, None, None).unwrap();
    check(&display, &frame);
}
