//! CPU preparation only; browser shader timings live in web/bench/.

use criterion::{BenchmarkId, Criterion, Throughput};
use rust_dos::video::Frame;
use rust_dos::video::mono::Monochrome;
use rust_dos::video::shader::XbrzFrame;
use std::hint::black_box;

fn prepare_frame(
    prepared: &mut XbrzFrame,
    screen: &Frame,
    base: &Frame,
    base_rows: std::ops::Range<usize>,
    screen_rows: std::ops::Range<usize>,
) -> (bool, bool, bool) {
    prepared.prepare_rows(screen, Some(base), Monochrome::Off, base_rows, screen_rows)
}

pub fn prepare(c: &mut Criterion) {
    let mut group = c.benchmark_group("video/xbrz_prepare");
    for (width, height, scale) in [(320, 200, 2), (640, 480, 1)] {
        let mut base = Frame::new(width * scale, height * scale);
        base.doubled = scale == 2;
        for y in 0..base.height as usize {
            for x in 0..base.width as usize {
                let colour = ((x / scale as usize / 7 + y / scale as usize / 5) % 16) as u8;
                let at = (y * base.width as usize + x) * 4;
                base.rgba[at..at + 3].copy_from_slice(&[
                    colour.wrapping_mul(17),
                    colour.wrapping_mul(43),
                    colour.wrapping_mul(71),
                ]);
            }
        }
        let mut row = base.clone();
        let top = row.height as usize / 2;
        let row_bytes = row.width as usize * 4;
        for pixel in row.rgba[top * row_bytes..(top + scale as usize) * row_bytes]
            .as_chunks_mut::<4>()
            .0
        {
            pixel[0] ^= 127;
        }
        let mut full = base.clone();
        for pixel in full.rgba.as_chunks_mut::<4>().0 {
            pixel[0] ^= 127;
        }
        let mut screen = base.clone();
        screen.track_overlays(true);
        let mut overlay = screen.clone();
        overlay.mark_overlay_rect(20, top, 32, 16);
        let mut moved = screen.clone();
        moved.mark_overlay_rect(60, top, 32, 16);
        let mut pixels = overlay.clone();
        for y in top..top + 16 {
            for x in 20..52 {
                pixels.rgba[y * row_bytes + x * 4] ^= 127;
            }
        }
        let mut dense = screen.clone();
        dense.mark_overlay_rect(0, 0, dense.width as usize, dense.height as usize);
        let mut dense_pixels = dense.clone();
        for pixel in dense_pixels.rgba.as_chunks_mut::<4>().0 {
            pixel[0] ^= 127;
        }
        let mut single = base.clone();
        let mut sprite = base.clone();
        let mut distant = base.clone();
        for (frame, rects) in [
            (&mut single, vec![(20, height / 2, 1, 1)]),
            (&mut sprite, vec![(20, height / 2, 32, 16)]),
            (
                &mut distant,
                vec![(20, 8, 16, 8), (width - 40, height - 24, 16, 8)],
            ),
        ] {
            for (x, y, w, h) in rects {
                for row in y * scale..(y + h) * scale {
                    for col in x * scale..(x + w) * scale {
                        frame.rgba[(row * frame.width + col) as usize * 4] ^= 127;
                    }
                }
            }
        }
        for frame in [&mut row, &mut full, &mut single, &mut sprite, &mut distant] {
            frame.track_overlays(true);
        }
        group.throughput(Throughput::Elements((width * height) as u64));
        for scenario in [
            "single",
            "sprite",
            "distant",
            "unchanged",
            "one_row",
            "full",
            "overlay_only",
            "overlay_static",
            "overlay_pixels",
            "overlay_move",
            "dense_static",
            "dense_pixels",
        ] {
            let mut prepared = XbrzFrame::default();
            let rows = match scenario {
                "unchanged" | "overlay_static" | "dense_static" => 0..0,
                "overlay_only" | "overlay_move" => top..top + 16,
                "single" => top..top + scale as usize,
                "sprite" => top..top + 16 * scale as usize,
                "distant" => 8 * scale as usize..(height as usize - 16) * scale as usize,
                "overlay_pixels" => top..top + 16,
                "one_row" => top..top + scale as usize,
                _ => 0..base.height as usize,
            };
            let initial = match scenario {
                "overlay_static" | "overlay_pixels" | "overlay_move" => &overlay,
                "dense_static" | "dense_pixels" => &dense,
                _ => &screen,
            };
            prepared.prepare(
                initial,
                Some(&base),
                Monochrome::Off,
                0..base.height as usize,
            );
            let mut step = false;
            let update = match scenario {
                "single" => &single,
                "sprite" => &sprite,
                "distant" => &distant,
                "one_row" => &row,
                "full" => &full,
                _ => &base,
            };
            let base_rows = match scenario {
                "one_row" | "single" | "sprite" | "distant" | "full" => rows.clone(),
                _ => 0..0,
            };
            let changed_screen = match scenario {
                "single" | "sprite" | "distant" | "one_row" | "full" => update,
                "overlay_only" | "overlay_static" => &overlay,
                "overlay_pixels" => &pixels,
                "overlay_move" => &moved,
                "dense_static" => &dense,
                "dense_pixels" => &dense_pixels,
                _ => &screen,
            };
            let result = prepare_frame(
                &mut prepared,
                changed_screen,
                update,
                base_rows.clone(),
                rows.clone(),
            );
            assert_eq!(
                result.1,
                !matches!(scenario, "unchanged" | "overlay_static" | "dense_static")
            );
            group.bench_function(
                BenchmarkId::new(scenario, format!("{width}x{height}")),
                |b| {
                    b.iter(|| {
                        step = !step;
                        black_box(prepare_frame(
                            &mut prepared,
                            black_box(if step { changed_screen } else { initial }),
                            black_box(if step { update } else { &base }),
                            base_rows.clone(),
                            rows.clone(),
                        ))
                    });
                },
            );
        }
    }
    group.finish();
}
