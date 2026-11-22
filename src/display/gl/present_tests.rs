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
    let result = screen.capture(frame, base, mono, display, false).unwrap();
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
fn xbrz_overlay_uploads_preserve_other_rows_and_recover_missed_revisions() {
    let sdl = sdl3::init().unwrap();
    let video = sdl.video().unwrap();
    let mut screen = GlScreen::open(&video, "xBRZ overlay bands", (4, 6), true)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    screen.select(Shader::Xbrz, Filter::Sharp).unwrap();
    let base = Frame::from_rgb(4, 6, vec![80; 4 * 6 * 3]);
    let mut composed = base.clone();
    composed.track_overlays(true);
    composed.mark_overlay(1, 1);
    screen
        .present(&composed, Some(&base), 0..6, (4, 6), None, Monochrome::Off)
        .unwrap();
    let read = |screen: &GlScreen| {
        let mut pixels = vec![0; 4 * 6 * 4];
        // SAFETY: this test owns the context, texture and sized readback.
        unsafe {
            screen
                .gl
                .bind_texture(glow::TEXTURE_2D, screen.xbrz_overlay);
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
    };
    // A sentinel outside the dirty band detects an accidental full upload.
    let sentinel = [88, 77, 66, 255];
    // SAFETY: the test owns the context and writes one texel in its texture.
    unsafe {
        screen
            .gl
            .bind_texture(glow::TEXTURE_2D, screen.xbrz_overlay);
        screen.gl.tex_sub_image_2d(
            glow::TEXTURE_2D,
            0,
            0,
            0,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&sentinel)),
        );
    }
    composed.rgba[(4 + 1) * 4] ^= 127;
    screen
        .present(&composed, Some(&base), 1..2, (4, 6), None, Monochrome::Off)
        .unwrap();
    let uploaded = read(&screen);
    assert_eq!(&uploaded[..4], &sentinel);
    assert_eq!(&uploaded[16..32], &screen.xbrz_frame.overlay.rgba[16..32]);
    composed.rgba[(4 + 1) * 4] ^= 64;
    screen
        .xbrz_frame
        .prepare(&composed, Some(&base), Monochrome::Off, 1..2);
    composed.mark_overlay(1, 4);
    screen
        .xbrz_frame
        .prepare(&composed, Some(&base), Monochrome::Off, 0..0);
    screen
        .present(&composed, Some(&base), 0..0, (4, 6), None, Monochrome::Off)
        .unwrap();
    assert_eq!(read(&screen), screen.xbrz_frame.overlay.rgba);
    let source = screen.xbrz_frame.source_revision();
    screen.select(Shader::None, Filter::Sharp).unwrap();
    composed.rgba[(4 + 1) * 4] ^= 32;
    screen
        .present(&composed, None, 0..6, (4, 6), None, Monochrome::Off)
        .unwrap();
    screen.select(Shader::Xbrz, Filter::Sharp).unwrap();
    screen
        .present(&composed, Some(&base), 0..0, (4, 6), None, Monochrome::Off)
        .unwrap();
    assert_eq!(read(&screen), screen.xbrz_frame.overlay.rgba);
    assert_eq!(read(&screen)[(4 + 1) * 4], composed.rgba[(4 + 1) * 4]);
    assert_eq!(screen.xbrz_frame.source_revision(), source);
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn failed_xbrz_preparation_reuploads_the_complete_composed_frame() {
    use crate::config::Settings;
    use crate::display::{Display, Output};
    let sdl = sdl3::init().unwrap();
    let video = sdl.video().unwrap();
    let textures = std::cell::OnceCell::new();
    let settings = Settings {
        shader: Shader::Xbrz,
        ..Default::default()
    };
    let mut display =
        Display::open(&video, "xBRZ failure recovery", &settings, &textures, true).unwrap();
    display.set_frame_size(4, 6).unwrap();
    let mut base = Frame::from_rgb(4, 6, vec![80; 4 * 6 * 3]);
    let mut composed = base.clone();
    composed.track_overlays(true);
    composed.mark_overlay_rect(0, 4, 4, 1);
    composed.rgba[4 * 4 * 4..5 * 4 * 4].fill(127);
    display
        .present_rows(&composed, None, None, 0..6, Some(&base))
        .unwrap();
    base.rgba[4 * 4] ^= 127;
    let Output::Gl(gl) = &mut display.out else {
        panic!("OpenGL is required")
    };
    gl.fail_overlay_upload = true;
    display
        .present_rows(&composed, None, None, 1..2, Some(&base))
        .unwrap();
    let Output::Gl(gl) = &display.out else {
        unreachable!()
    };
    assert_eq!(gl.active(), Shader::None);
    assert_eq!(
        texture(gl),
        composed.rgba,
        "rows outside the source damage retain overlays"
    );
    assert!(
        display
            .shader_warning()
            .unwrap()
            .contains("injected overlay")
    );
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn xbrz_clean_and_flattened_presentations_share_preparation() {
    use crate::video::{mono, power};
    let sdl = sdl3::init().unwrap();
    let video = sdl.video().unwrap();
    let mut screen = GlScreen::open(&video, "xBRZ input regression", (16, 12), true)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    screen.select(Shader::Xbrz, Filter::Sharp).unwrap();
    let native = Frame::from_rgb(8, 6, (0..144).map(|i| (i * 37) as u8).collect());
    let base = double(&native);
    let mut composed = base.clone();
    mono::apply(&mut composed, Monochrome::Amber);
    composed.track_overlays(true);
    composed.mark_overlay_rect(2, 2, 3, 3);
    for y in 2..5 {
        for x in 2..5 {
            composed.rgba[(y * 16 + x) * 4..][..4].copy_from_slice(&[11, 222, 33, 255]);
        }
    }
    for effect in [false, true, false] {
        let clean = reference_capture(&mut screen, &composed, &base, Monochrome::Amber, (16, 12));
        let mut flattened = composed.clone();
        if effect {
            power::apply(
                &mut flattened,
                &power::Look {
                    brightness: 0.7,
                    scale: (0.6, 0.4),
                    offset_y: 0.0,
                    wobble: 0.0,
                    phase: 0.0,
                },
            );
            assert!(!flattened.doubled);
        }
        // The effect's coverage predates its geometric transformation.
        let mut expected_input = flattened.clone();
        expected_input.track_overlays(false);
        let expected = reference_capture(
            &mut screen,
            &expected_input,
            &expected_input,
            Monochrome::Off,
            (16, 12),
        );
        assert_ne!(
            clean, expected,
            "flattened effects filter the whole picture"
        );
        screen
            .present(
                &composed,
                Some(&base),
                0..12,
                (16, 12),
                None,
                Monochrome::Amber,
            )
            .unwrap();
        assert_eq!(screen.texture_scale, 2);
        assert_eq!(
            screen.xbrz_frame.overlay.rgba[(2 * 16 + 2) * 4..][..4],
            [11, 222, 33, 255]
        );
        screen
            .present(&flattened, None, 0..12, (16, 12), None, Monochrome::Green)
            .unwrap();
        assert_eq!(screen.texture_size, if effect { (16, 12) } else { (8, 6) });
        assert_eq!(screen.texture_scale, if effect { 1 } else { 2 });
        assert!(screen.xbrz_frame.overlay.rgba.iter().all(|&byte| byte == 0));
        let mut pixels = vec![0; 16 * 12 * 4];
        // SAFETY: this test owns the current context and sized readback.
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
        assert_eq!(top_first, expected.rgba, "effect={effect}");
        println!(
            "flattened effect={effect}: {} bytes match reference; scale={}",
            top_first.len(),
            screen.texture_scale
        );
    }
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn xbrz_cached_passes_match_reference_and_reuse_source_work() {
    let sdl = sdl3::init().unwrap();
    let video = sdl.video().unwrap();
    let mut screen = GlScreen::open(&video, "xBRZ cache regression", (32, 24), true)
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
    screen.select(Shader::Xbrz, Filter::Sharp).unwrap();
    let native = Frame::from_rgb(5, 3, (0..45).map(|i| (i * 37) as u8).collect());
    let mut base = double(&native);
    let mut composed = base.clone();
    composed.track_overlays(true);
    let draws = |screen: &GlScreen| screen.xbrz_gpu.as_ref().unwrap().as_ref().unwrap().draws;
    let compare = |screen: &mut GlScreen, frame: &Frame, base: &Frame, mono, size| {
        let actual = screen.capture(frame, base, mono, size, false).unwrap();
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
        let mut scissor = [0; 4];
        screen
            .gl
            .get_parameter_i32_slice(glow::SCISSOR_BOX, &mut scissor);
        assert_eq!(scissor, [0, 0, 32, 24]);
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
    let sdl = sdl3::init().unwrap();
    let video = sdl.video().unwrap();
    let mut screen = GlScreen::open(&video, "RGBA presentation regression", (8, 8), true)
        .unwrap_or_else(|e| panic!("{}", e.reason));
    let mut frame = Frame::from_rgb(5, 4, (0..60).collect());
    screen
        .present(&frame, None, 0..4, (5, 4), None, Monochrome::Off)
        .unwrap();
    assert_eq!(texture(&screen), frame.rgba);
    frame.rgba[40..60]
        .as_chunks_mut::<4>()
        .0
        .fill([99, 50, 20, 255]);
    screen
        .present(&frame, None, 2..3, (5, 4), None, Monochrome::Off)
        .unwrap();
    assert_eq!(texture(&screen), frame.rgba);

    let mut small = Frame::from_rgb(4, 3, (40..76).collect());
    let doubled = double(&small);
    screen
        .present(&doubled, None, 0..6, (8, 6), None, Monochrome::Off)
        .unwrap();
    assert_eq!(screen.texture_size, (4, 3));
    assert_eq!(texture(&screen), small.rgba);
    small.rgba[16..32]
        .as_chunks_mut::<4>()
        .0
        .fill([11, 22, 33, 255]);
    let doubled = double(&small);
    screen
        .present(&doubled, None, 2..4, (8, 6), None, Monochrome::Off)
        .unwrap();
    assert_eq!(texture(&screen), small.rgba);

    // Filtering changes the upload from the native half-size picture to
    // the complete frame, even when no CPU rows need repainting.
    screen.select(Shader::None, Filter::Linear).unwrap();
    screen
        .present(&doubled, None, 0..0, (8, 6), None, Monochrome::Off)
        .unwrap();
    assert_eq!(screen.texture_size, (8, 6));
    assert_eq!(texture(&screen), doubled.rgba);

    // xBRZ needs native pixels even when the plain filter is linear.
    screen.select(Shader::Xbrz, Filter::Linear).unwrap();
    screen
        .present(&doubled, None, 0..0, (8, 6), None, Monochrome::Off)
        .unwrap();
    assert_eq!(screen.texture_size, (4, 3));
    assert_eq!(texture(&screen), small.rgba);
    small.rgba[16..32]
        .as_chunks_mut::<4>()
        .0
        .fill([55, 66, 77, 255]);
    let doubled = double(&small);
    screen
        .present(&doubled, None, 3..4, (8, 6), None, Monochrome::Off)
        .unwrap();
    assert_eq!(texture(&screen), small.rgba);
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn xbrz_overlays_preserve_native_input_and_filtered_background() {
    let sdl = sdl3::init().unwrap();
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
    let mut screen = GlScreen::open(&video, "xBRZ overlay regression", (16, 12), true)
        .unwrap_or_else(|e| panic!("{}", e.reason));
    screen.select(Shader::Xbrz, Filter::Sharp).unwrap();
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
        let mut expected = screen.capture(&composed, &base, mono, (16, 12), false).unwrap().rgba;
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
            .present(
                &composed,
                Some(&base),
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
        let captured = screen.capture(&composed, &base, mono, (16, 12), false).unwrap();
        assert_eq!(captured.rgba, expected, "capture, mono: {mono:?}");
    }
    let doubled = screen.capture(&base, &base, Monochrome::Off, (16, 12), false).unwrap();
    let undoubled = screen.capture(&native, &native, Monochrome::Off, (8, 6), false).unwrap();
    assert_eq!(doubled.rgba, undoubled.rgba);
    screen
        .present(&native, Some(&native), 0..6, (8, 6), None, Monochrome::Off)
        .unwrap();
    assert_eq!(texture(&screen), native.rgba);
    screen.select(Shader::Scanlines, Filter::Sharp).unwrap();
    let captured = screen.capture(&base, &base, Monochrome::Off, (16, 12), false).unwrap();
    assert_eq!((captured.width, captured.height), (16, 12));
    assert!(captured.rgba.as_chunks::<4>().0.iter().any(|p| p[..3] != [0; 3]));
    screen.select(Shader::None, Filter::Sharp).unwrap();
    assert!(screen.capture(&base, &base, Monochrome::Off, (16, 12), false).is_none());
}

#[test]
#[ignore = "requires an OpenGL display; run in CI under Xvfb"]
fn xbrz_near_colour_edges_use_exact_colour_inequality() {
    use crate::video::mono::Monochrome;
    let sdl = sdl3::init().unwrap();
    let video = sdl.video().unwrap();
    let values = [
        160, 32, 160, 0, 32, 32, 16, 16, 160, 0, 32, 16, 0, 0, 16, 255, 16, 0, 16, 255, 16,
        32, 160, 255, 160,
    ];
    let native = Frame::from_rgb(5, 5, values.into_iter().flat_map(|v| [v, v, v]).collect());
    let base = double(&native);
    let mut screen = GlScreen::open(&video, "xBRZ near colour regression", (10, 10), true)
        .unwrap_or_else(|e| panic!("{}", e.reason));
    screen.select(Shader::Xbrz, Filter::Sharp).unwrap();
    let output = screen.capture(&base, &base, Monochrome::Off, (10, 10), false).unwrap();
    assert_eq!(&output.rgba[(2 * 10 + 7) * 4..][..3], &[40, 40, 40]);
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn xbrz_two_colour_pictures_filter_the_same_with_colours_swapped() {
    use crate::video::mono::Monochrome;
    // With a symmetric distance, swapping the two colours of a picture
    // swaps them in every blend: each output pixel of the swapped picture
    // is the sum of the two colours minus the original's, give or take
    // the rounding of the blend. The texels near the edge also see the
    // transparent black outside the frame, so only the interior counts.
    let (low, high) = (100u8, 134u8);
    let (width, height) = (24usize, 24usize);
    let mut state = 0x2545_f491u32;
    let picture: Vec<bool> = (0..width * height)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state >> 31 == 1
        })
        .collect();
    let sdl = sdl3::init().unwrap();
    let video = sdl.video().unwrap();
    let display = (width as u32 * 2, height as u32 * 2);
    let mut screen = GlScreen::open(&video, "xBRZ swapped colours", display, true)
        .unwrap_or_else(|e| panic!("{}", e.reason));
    screen.select(Shader::Xbrz, Filter::Sharp).unwrap();
    let mut draw = |swapped: bool| {
        let pixels = picture
            .iter()
            .flat_map(|&bit| [if bit != swapped { high } else { low }; 3])
            .collect();
        let native = Frame::from_rgb(width as u32, height as u32, pixels);
        let base = double(&native);
        screen
            .capture(&base, &base, Monochrome::Off, display, false)
            .unwrap()
    };
    let (plain, swapped) = (draw(false), draw(true));
    let mut blended = 0;
    for y in 8..height * 2 - 8 {
        for x in 8..width * 2 - 8 {
            let at = (y * width * 2 + x) * 4;
            let (a, b) = (plain.rgba[at] as i32, swapped.rgba[at] as i32);
            if a != low as i32 && a != high as i32 {
                blended += 1;
            }
            assert!(
                (a + b - low as i32 - high as i32).abs() <= 1,
                "({x}, {y}): {a} and {b} with the colours swapped"
            );
        }
    }
    assert!(blended > 0, "the picture must have blended edges");
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn persistent_compositions_keep_their_pixels_after_partial_presentation() {
    use crate::display::{Display, Output};
    let sdl = sdl3::init().unwrap();
    let video = sdl.video().unwrap();
    let textures = std::cell::OnceCell::new();
    let mut display = Display::open(
        &video,
        "Persistent composition regression",
        &Default::default(),
        &textures,
        true,
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
        assert!(display.shaded(&composed, &base, false).is_some());
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

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn xbrz_incremental_regions_match_full_kernels_after_deferred_updates() {
    let sdl = sdl3::init().unwrap();
    let video = sdl.video().unwrap();
    let mut screen = GlScreen::open(&video, "xBRZ damage regression", (96, 64), true)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    screen.select(Shader::Xbrz, Filter::Sharp).unwrap();
    let mut native = Frame::from_rgba(
        24,
        16,
        (0..24 * 16)
            .flat_map(|i| {
                [
                    (i * 37) as u8,
                    (i * 71) as u8,
                    (i * 113) as u8,
                    (i * 53) as u8,
                ]
            })
            .collect(),
    );
    for step in 0..72 {
        if step % 13 == 1 {
            // Prepare one source revision without uploading it. The next
            // upload must include this pixel outside its latest row band.
            native.rgba[0] ^= 127;
            let skipped = if step % 24 < 12 {
                native.clone()
            } else {
                double(&native)
            };
            screen.xbrz_frame.prepare(
                &skipped,
                Some(&skipped),
                Monochrome::ALL[(step - 1) / 9 % 4],
                0..skipped.height as usize,
            );
        }
        for (x, y) in [
            (step % 24, step * 7 % 16),
            (23 - step % 24, 15 - step * 7 % 16),
        ] {
            native.rgba[(y * 24 + x) * 4] ^= 127;
        }
        let base = if step % 24 < 12 {
            native.clone()
        } else {
            double(&native)
        };
        let mono = Monochrome::ALL[step / 9 % 4];
        let mut frame = base.clone();
        crate::video::mono::apply(&mut frame, mono);
        frame.track_overlays(true);
        frame.mark_overlay_rect(0, 0, 3, 2);
        frame.rgba[..12].fill(step as u8);
        let size = match step % 6 {
            0 => (24, 16),
            1 | 2 => (8, 8),
            3 => (73, 49),
            _ => (96, 64),
        };
        if step % 13 == 0 {
            screen.xbrz_uploaded_revision = Some(u32::MAX);
        }
        let actual = screen.capture(&frame, &base, mono, size, false).unwrap();
        assert_eq!(
            texture(&screen)
                .iter()
                .zip(&screen.xbrz_frame.source.rgba)
                .position(|(a, b)| a != b),
            None,
            "source at step {step}"
        );
        let expected = reference_capture(&mut screen, &frame, &base, mono, size);
        assert_eq!(
            actual
                .rgba
                .iter()
                .zip(&expected.rgba)
                .position(|(a, b)| a != b),
            None,
            "step {step}, size {size:?}",
        );
        // SAFETY: the test owns this current context and its source texture.
        unsafe {
            assert_eq!(screen.gl.get_parameter_i32(glow::UNPACK_ROW_LENGTH), 0);
            assert_eq!(screen.gl.get_error(), glow::NO_ERROR);
        }
    }
}

/// A ToadForce in a machine draws through the window's host renderer,
/// by the calls `Display::run_toadforce` makes: the pixels are on the
/// GPU until card memory is asked for, the window's own drawing is as
/// it was afterwards, and a card that outlives the window draws in
/// software.
#[test]
#[ignore = "requires a desktop OpenGL context"]
fn a_toadforce_draws_through_the_windows_host_renderer() {
    use rust_dos::cpu::Cpu;
    use rust_dos::toadforce::format::{Format, Surface};
    use rust_dos::toadforce::level1::states::{
        ARG_DIFFUSE, OP_SELECT_ARG1, RS_CULLMODE, TSS_ALPHAARG1, TSS_ALPHAOP, TSS_COLORARG1,
        TSS_COLOROP,
    };
    use rust_dos::toadforce::level1::{Target, Viewport, encode};
    use rust_dos::toadforce::regs;
    use rust_dos::video::adapter::{Adapter, VideoSetup};

    let sdl = sdl3::init().unwrap();
    let video = sdl.video().unwrap();
    let mut screen = GlScreen::open(&video, "ToadForce host", (8, 8), true)
        .unwrap_or_else(|e| panic!("{}", e.reason));
    let mut cpu = Cpu::new(std::path::PathBuf::from("."));
    let setup = VideoSetup {
        adapter: Adapter::ToadForce,
        ..Default::default()
    };
    rust_dos::video::bios::install(&mut cpu.bus, setup);
    cpu.bus.toadforce_register_write(regs::LEVEL, 1);

    assert!(!screen.toadforce_linked());
    let link = screen.toadforce_link().expect("a host renderer");
    cpu.bus.toadforce_set_host(Some(link));
    assert!(cpu.bus.toadforce_hosted() && screen.toadforce_linked());
    let renderer = |cpu: &Cpu| cpu.bus.toadforce_status().unwrap()["renderer"].clone();
    assert_eq!(renderer(&cpu), "OpenGL");
    let made = cpu.bus.toadforce_status().unwrap();

    let colour = Surface {
        offset: 8 << 20,
        pitch: 16 * 4,
        width: 16,
        height: 16,
        format: Format::X8R8G8B8,
    };
    let target = Target {
        colour,
        depth: None,
    };
    let viewport = Viewport {
        x: 0,
        y: 0,
        width: 16,
        height: 16,
        min_z: 0.0,
        max_z: 1.0,
    };
    // Two triangles over the target, of transformed vertices with a
    // diffuse colour (D3DFVF_XYZRHW | D3DFVF_DIFFUSE).
    let square = |cpu: &mut Cpu, diffuse: u32| {
        let corners = [(0.0, 0.0), (16.0, 0.0), (0.0, 16.0)];
        let other = [(16.0, 0.0), (16.0, 16.0), (0.0, 16.0)];
        let mut vertices = Vec::new();
        for (x, y) in corners.into_iter().chain(other) {
            vertices.extend([x, y, 0.5f32, 1.0].map(f32::to_bits));
            vertices.push(diffuse);
        }
        let mut words = Vec::new();
        encode::draw(&mut words, 4, 0x004 | 0x040, 6, &vertices);
        cpu.bus.toadforce_submit(&words);
        cpu.bus.toadforce_flush();
    };
    let mut words = Vec::new();
    encode::render_target(&mut words, Some(&target));
    encode::viewport(&mut words, &viewport);
    encode::render_state(&mut words, &[(RS_CULLMODE, 1)]);
    // The diffuse colour alone: no texture is set.
    let diffuse = [
        (TSS_COLOROP, OP_SELECT_ARG1),
        (TSS_COLORARG1, ARG_DIFFUSE),
        (TSS_ALPHAOP, OP_SELECT_ARG1),
        (TSS_ALPHAARG1, ARG_DIFFUSE),
    ];
    encode::stage_state(&mut words, 0, &diffuse);
    cpu.bus.toadforce_submit(&words);
    let pixel = |cpu: &Cpu| {
        let at = (8 << 20) + 8 * 64 + 8 * 4;
        u32::from_le_bytes(cpu.bus.vbe.vram[at..at + 4].try_into().unwrap()) & 0xFF_FFFF
    };

    square(&mut cpu, 0xFFFF_0000);
    assert_eq!(cpu.bus.toadforce_status().unwrap()["error"], 0);
    // The GPU has the square, and card memory gets it when asked.
    assert!(cpu.bus.toadforce_ahead());
    assert_eq!(pixel(&cpu), 0);
    cpu.bus.toadforce_touch(8 << 20, 16 * 64);
    assert!(!cpu.bus.toadforce_ahead());
    assert_eq!(pixel(&cpu), 0xFF_0000);
    let status = cpu.bus.toadforce_status().unwrap();
    assert_eq!(status["host"]["read_backs"], 1, "{}", status["host"]);
    // One batch more than the renderer's test of the host's formats
    // drew when it was made.
    let batches = |status: &serde_json::Value| status["host"]["counts"]["batches"].as_u64();
    assert_eq!(batches(&status), batches(&made).map(|n| n + 1));

    // The window draws its own picture as before.
    let frame = Frame::from_rgb(5, 4, (0..60).collect());
    screen
        .present(&frame, None, 0..4, (5, 4), None, Monochrome::Off)
        .unwrap();
    assert_eq!(texture(&screen), frame.rgba);
    // SAFETY: the test's context is current.
    unsafe { assert_eq!(screen.gl.get_error(), glow::NO_ERROR) };

    // Another machine's card gets the same renderer, without the
    // first card's surfaces.
    let link = screen.toadforce_link().expect("a host renderer");
    cpu.bus.toadforce_set_host(Some(link));
    square(&mut cpu, 0xFF00_FF00);
    assert_eq!(pixel(&cpu), 0xFF_0000);

    // The window ends with the square still on the GPU: that picture
    // is lost, and the card draws in software from then on.
    drop(screen);
    square(&mut cpu, 0xFF00_00FF);
    assert_eq!(pixel(&cpu), 0x00_00FF);
    cpu.bus.toadforce_set_host(None);
    assert_eq!(renderer(&cpu), "software");
}

/// The window's picture of a ToadForce surface comes from the host
/// renderer's copy: the top row first, 16-bit pixels as card memory
/// would have them, the window's own drawing over it, and nothing read
/// back.
#[test]
#[ignore = "requires a desktop OpenGL context"]
fn a_toadforce_surface_is_shown_from_the_hosts_copy() {
    use rust_dos::cpu::Cpu;
    use rust_dos::toadforce::format::{Format, Surface};
    use rust_dos::toadforce::level1::states::{
        ARG_DIFFUSE, OP_SELECT_ARG1, RS_CULLMODE, TSS_ALPHAARG1, TSS_ALPHAOP, TSS_COLORARG1,
        TSS_COLOROP,
    };
    use rust_dos::toadforce::level1::{Target, Viewport, encode};
    use rust_dos::toadforce::regs;
    use rust_dos::video::adapter::{Adapter, VideoSetup};

    let sdl = sdl3::init().unwrap();
    let video = sdl.video().unwrap();
    let mut screen = GlScreen::open(&video, "ToadForce picture", (8, 8), true)
        .unwrap_or_else(|e| panic!("{}", e.reason));
    for format in [Format::X8R8G8B8, Format::R5G6B5] {
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let setup = VideoSetup {
            adapter: Adapter::ToadForce,
            ..Default::default()
        };
        rust_dos::video::bios::install(&mut cpu.bus, setup);
        cpu.bus.toadforce_register_write(regs::LEVEL, 1);
        let link = screen.toadforce_link().expect("a host renderer");
        cpu.bus.toadforce_set_host(Some(link));
        let colour = Surface {
            offset: 8 << 20,
            pitch: 16 * format.bytes(),
            width: 16,
            height: 16,
            format,
        };
        fn card(cpu: &mut Cpu) -> &mut rust_dos::toadforce::ToadForce {
            cpu.bus.toadforce.as_mut().expect("a ToadForce")
        }
        fn shows(cpu: &mut Cpu, surface: &Surface) -> bool {
            let bus = &mut cpu.bus;
            let card = bus.toadforce.as_mut().expect("a ToadForce");
            card.host_shows(surface, &mut bus.vbe.vram)
        }
        // Nothing is drawn yet: the picture is card memory's.
        assert!(!shows(&mut cpu, &colour), "{format:?}");
        assert!(!card(&mut cpu).host_presents(), "{format:?}");

        let target = Target {
            colour,
            depth: None,
        };
        let viewport = Viewport {
            x: 0,
            y: 0,
            width: 16,
            height: 16,
            min_z: 0.0,
            max_z: 1.0,
        };
        let diffuse = [
            (TSS_COLOROP, OP_SELECT_ARG1),
            (TSS_COLORARG1, ARG_DIFFUSE),
            (TSS_ALPHAOP, OP_SELECT_ARG1),
            (TSS_ALPHAARG1, ARG_DIFFUSE),
        ];
        let mut words = Vec::new();
        encode::render_target(&mut words, Some(&target));
        encode::viewport(&mut words, &viewport);
        encode::render_state(&mut words, &[(RS_CULLMODE, 1)]);
        encode::stage_state(&mut words, 0, &diffuse);
        // Rows 0 to `bottom` of the target in one colour.
        let mut rows = |bottom: f32, diffuse: u32| {
            let corners = [(0.0, 0.0), (16.0, 0.0), (0.0, bottom)];
            let other = [(16.0, 0.0), (16.0, bottom), (0.0, bottom)];
            let mut vertices = Vec::new();
            for (x, y) in corners.into_iter().chain(other) {
                vertices.extend([x, y, 0.5f32, 1.0].map(f32::to_bits));
                vertices.push(diffuse);
            }
            encode::draw(&mut words, 4, 0x004 | 0x040, 6, &vertices);
        };
        // Red, with green on the top four rows.
        rows(16.0, 0xFFFF_0000);
        rows(4.0, 0xFF00_FF00);
        cpu.bus.toadforce_submit(&words);
        cpu.bus.toadforce_flush();
        assert_eq!(cpu.bus.toadforce_status().unwrap()["error"], 0);
        assert!(cpu.bus.toadforce_ahead());

        // The host has the surface, and the frame's read-back is left
        // out once.
        assert!(shows(&mut cpu, &colour), "{format:?}");
        assert!(card(&mut cpu).host_presents(), "{format:?}");
        assert!(!card(&mut cpu).host_presents(), "{format:?}");

        // The window drew a white pixel at (3, 9) over the machine's
        // picture, which it has as black: card memory's stale bytes.
        let base = Frame::from_rgb(16, 16, vec![0; 16 * 16 * 3]);
        let mut over = base.clone();
        let at = (9 * 16 + 3) * 4;
        over.rgba[at..at + 3].fill(255);
        let composed = screen
            .toadforce
            .0
            .borrow_mut()
            .as_mut()
            .expect("the host renderer")
            .composite(&colour, None, &over, &base);
        let (texture, size) = composed.expect("a picture");
        assert_eq!(size, (16, 16));
        let read = |texture: glow::Texture| {
            let mut pixels = vec![0; 16 * 16 * 4];
            // SAFETY: the test owns the current context and the texture.
            unsafe {
                screen.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
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
            move |x: usize, y: usize| {
                let at = (y * 16 + x) * 4;
                [pixels[at], pixels[at + 1], pixels[at + 2]]
            }
        };
        let pixel = read(texture);
        assert_eq!(pixel(8, 1), [0, 255, 0], "{format:?}");
        assert_eq!(pixel(8, 12), [255, 0, 0], "{format:?}");
        assert_eq!(pixel(3, 9), [255, 255, 255], "{format:?}");
        assert_eq!(pixel(4, 9), [255, 0, 0], "{format:?}");

        // Through a gamma ramp, the picture's colours change, and what
        // the window drew over it doesn't.
        let mut ramp = rust_dos::toadforce::display::IDENTITY;
        ramp[0] = 0x01_0203;
        ramp[255] = 0x80_C040;
        let composed = screen
            .toadforce
            .0
            .borrow_mut()
            .as_mut()
            .expect("the host renderer")
            .composite(&colour, Some(&ramp), &over, &base);
        let pixel = read(composed.expect("a picture").0);
        assert_eq!(pixel(8, 1), [0x01, 0xC0, 0x03], "{format:?}");
        assert_eq!(pixel(8, 12), [0x80, 0x02, 0x03], "{format:?}");
        assert_eq!(pixel(3, 9), [255, 255, 255], "{format:?}");

        // Nothing came back to card memory, which a screenshot still
        // gets when it asks.
        let status = cpu.bus.toadforce_status().unwrap();
        assert_eq!(status["host"]["read_backs"], 0, "{}", status["host"]);
        assert!(cpu.bus.toadforce_ahead());
        cpu.bus.toadforce_touch(8 << 20, 16 * colour.pitch as usize);
        assert!(!cpu.bus.toadforce_ahead());
        let first = cpu.bus.vbe.vram[8 << 20..][..2].to_vec();
        let green = if format == Format::R5G6B5 {
            [0xE0, 0x07]
        } else {
            [0, 255]
        };
        assert_eq!(first, green, "{format:?}");
        // The copy is what card memory holds now, and still shown.
        assert!(shows(&mut cpu, &colour), "{format:?}");
        cpu.bus.toadforce_set_host(None);
    }
}

/// What the plain look draws of the screen's texture into a `w` by `h`
/// framebuffer, as the window draws it into its letterbox, rows from the
/// top, four bytes a pixel.
fn drawn(screen: &mut GlScreen, (w, h): (u32, u32)) -> Vec<u8> {
    let gl = screen.gl.clone();
    let mut pixels = vec![0; (w * h * 4) as usize];
    // SAFETY: the test owns the current context and what it makes in it.
    unsafe {
        let target = gl.create_texture().unwrap();
        gl.bind_texture(glow::TEXTURE_2D, Some(target));
        let none = glow::PixelUnpackData::Slice(None);
        let rgba8 = glow::RGBA8 as i32;
        let (tw, th) = (w as i32, h as i32);
        gl.tex_image_2d(glow::TEXTURE_2D, 0, rgba8, tw, th, 0, glow::RGBA, glow::UNSIGNED_BYTE, none);
        let framebuffer = gl.create_framebuffer().unwrap();
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
        let attach = glow::COLOR_ATTACHMENT0;
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, attach, glow::TEXTURE_2D, Some(target), 0);
        screen.draw_look(screen.texture, screen.texture_size, (0, 0, w, h), 1.0);
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
        let out = glow::PixelPackData::Slice(Some(&mut pixels));
        gl.read_pixels(0, 0, tw, th, glow::RGBA, glow::UNSIGNED_BYTE, out);
        assert_eq!(gl.get_error(), glow::NO_ERROR);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.delete_framebuffer(framebuffer);
        gl.delete_texture(target);
    }
    pixels.chunks_exact(w as usize * 4).rev().flatten().copied().collect()
}

/// `frame` scaled `sx` by `sy` times with nearest sampling.
fn nearest(frame: &Frame, (sx, sy): (u32, u32)) -> Vec<u8> {
    let (w, h) = (frame.width * sx, frame.height * sy);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let at = (((y / sy) * frame.width + x / sx) * 4) as usize;
            out.extend_from_slice(&frame.rgba[at..at + 4]);
        }
    }
    out
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn sharp_and_hard_scaling_are_nearest_sampling_at_whole_number_scales() {
    let sdl = sdl3::init().unwrap();
    let video = sdl.video().unwrap();
    let mut screen = GlScreen::open(&video, "Sharp scaling", (64, 48), true)
        .unwrap_or_else(|e| panic!("{}", e.reason));
    let frame = Frame::from_rgb(16, 12, (0..16 * 12 * 3).map(|i| (i * 89 % 251) as u8).collect());
    for filter in [Filter::Sharp, Filter::Hard] {
        screen.select(Shader::None, filter).unwrap();
        screen
            .present(&frame, None, 0..12, (16, 12), None, Monochrome::Off)
            .unwrap();
        // 1x, 2x, 3x, and the two axes apart, as with a fixed aspect ratio.
        for scale in [(1, 1), (2, 2), (3, 3), (2, 3), (5, 4)] {
            let size = (16 * scale.0, 12 * scale.1);
            assert_eq!(drawn(&mut screen, size), nearest(&frame, scale), "{filter:?} {scale:?}");
        }
        // A doubled picture is uploaded at its native size, and drawn as
        // the doubled one would be.
        let doubled = double(&frame);
        screen
            .present(&doubled, None, 0..24, (32, 24), None, Monochrome::Off)
            .unwrap();
        assert_eq!(screen.texture_size, (16, 12));
        for scale in [(1, 1), (2, 2), (3, 3)] {
            let size = (32 * scale.0, 24 * scale.1);
            assert_eq!(drawn(&mut screen, size), nearest(&doubled, scale), "{filter:?} {scale:?}");
        }
    }
}

/// `filter=hard` is nearest sampling at any scale: each screen pixel is
/// the texel under its centre. 12 texels over 28 or 20 screen pixels put
/// no centre on a texel's edge, so the texel is never in doubt.
#[test]
#[ignore = "requires a desktop OpenGL context"]
fn hard_scaling_is_nearest_sampling_at_any_scale() {
    let sdl = sdl3::init().unwrap();
    let video = sdl.video().unwrap();
    let mut screen = GlScreen::open(&video, "Hard scaling", (64, 48), true)
        .unwrap_or_else(|e| panic!("{}", e.reason));
    screen.select(Shader::None, Filter::Hard).unwrap();
    let frame = Frame::from_rgb(12, 12, (0..12 * 12 * 3).map(|i| (i * 89 % 251) as u8).collect());
    screen
        .present(&frame, None, 0..12, (12, 12), None, Monochrome::Off)
        .unwrap();
    for (w, h) in [(28u32, 28u32), (20, 28)] {
        let texel = |at: u32, size: u32| (2 * at + 1) * 12 / (2 * size);
        let expected: Vec<u8> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let at = ((texel(y, h) * 12 + texel(x, w)) * 4) as usize;
                frame.rgba[at..at + 4].to_vec()
            })
            .collect();
        assert_eq!(drawn(&mut screen, (w, h)), expected, "{w}x{h}");
    }
}

/// How much of the screen each texel of one-pixel stripes covers along
/// one axis: `values` are the screen pixels' whiteness (0 to 1) across
/// the stripes, `texels` many, white first. A pixel's white goes to the
/// white texel it overlaps and its black to the black one.
fn stripe_coverage(values: &[f64], texels: usize) -> Vec<f64> {
    let scale = values.len() as f64 / texels as f64;
    let mut cover = vec![0.0; texels];
    for (i, &white) in values.iter().enumerate() {
        let (from, to) = (i as f64 / scale, (i + 1) as f64 / scale);
        let first = from.floor() as usize;
        let last = ((to - 1e-9).floor() as usize).min(texels - 1);
        for (texel, share) in cover.iter_mut().enumerate().take(last + 1).skip(first) {
            *share += if texel % 2 == 0 { white } else { 1.0 - white };
        }
    }
    cover
}

#[test]
#[ignore = "requires a desktop OpenGL context"]
fn sharp_scaling_gives_every_texel_the_same_width_at_two_and_a_half_times() {
    let sdl = sdl3::init().unwrap();
    let video = sdl.video().unwrap();
    let mut screen = GlScreen::open(&video, "Sharp scaling", (64, 48), true)
        .unwrap_or_else(|e| panic!("{}", e.reason));
    screen.select(Shader::None, Filter::Sharp).unwrap();
    // White and black columns one texel wide, and rows the same.
    let (w, h) = (16u32, 12u32);
    let stripes = |across: bool| {
        let rgb = (0..w * h)
            .flat_map(|i| {
                let on = if across { i % w } else { i / w } % 2 == 0;
                [if on { 255 } else { 0 }; 3]
            })
            .collect();
        Frame::from_rgb(w, h, rgb)
    };
    // 2.5 times, and 320x200 at 4:3 in a 800x600 window: 2.5 across,
    // 3 down.
    for (scale, size) in [(2.5, (40, 30)), (2.5, (40, 36))] {
        for across in [true, false] {
            let frame = stripes(across);
            screen
                .present(&frame, None, 0..h as usize, (w, h), None, Monochrome::Off)
                .unwrap();
            let pixels = drawn(&mut screen, size);
            let at = |x: u32, y: u32| pixels[((y * size.0 + x) * 4) as usize] as f64 / 255.0;
            // Along the middle row or column.
            let (values, texels, scale): (Vec<f64>, usize, f64) = if across {
                ((0..size.0).map(|x| at(x, size.1 / 2)).collect(), w as usize, scale)
            } else {
                let down = size.1 as f64 / h as f64;
                ((0..size.1).map(|y| at(size.0 / 2, y)).collect(), h as usize, down)
            };
            let cover = stripe_coverage(&values, texels);
            for (texel, &width) in cover.iter().enumerate() {
                // An 8-bit pixel's rounding on each of the two edges.
                assert!(
                    (width - scale).abs() <= 2.0 / 255.0,
                    "across={across} {size:?}: texel {texel} covers {width}, not {scale}: {cover:?}"
                );
            }
            // A blend is less than one screen pixel wide: no two pixels
            // in a row are between black and white.
            let blended: Vec<bool> = values.iter().map(|&v| v > 0.01 && v < 0.99).collect();
            assert!(
                !blended.windows(2).any(|pair| pair[0] && pair[1]),
                "across={across} {size:?}: {values:?}"
            );
        }
    }
}
