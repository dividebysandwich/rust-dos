//! CPU preparation only; browser shader timings live in web/bench/.

use criterion::{BenchmarkId, Criterion, Throughput};
use rust_dos::video::Frame;
use rust_dos::video::mono::Monochrome;
use rust_dos::video::shader::XbrzFrame;
use std::hint::black_box;

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
        group.throughput(Throughput::Elements((width * height) as u64));
        for scenario in [
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
                "unchanged" | "overlay_only" | "overlay_static" | "overlay_move"
                | "dense_static" => 0..0,
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
                "one_row" => &row,
                "full" => &full,
                _ => &base,
            };
            let changed_screen = match scenario {
                "overlay_only" | "overlay_static" => &overlay,
                "overlay_pixels" => &pixels,
                "overlay_move" => &moved,
                "dense_static" => &dense,
                "dense_pixels" => &dense_pixels,
                _ => &screen,
            };
            let result =
                prepared.prepare(changed_screen, Some(update), Monochrome::Off, rows.clone());
            assert_eq!(
                result.1,
                !matches!(scenario, "unchanged" | "overlay_static" | "dense_static")
            );
            group.bench_function(
                BenchmarkId::new(scenario, format!("{width}x{height}")),
                |b| {
                    b.iter(|| {
                        step = !step;
                        black_box(prepared.prepare(
                            black_box(if step { changed_screen } else { initial }),
                            Some(black_box(if step { update } else { &base })),
                            Monochrome::Off,
                            rows.clone(),
                        ))
                    });
                },
            );
        }
    }
    group.finish();
}
