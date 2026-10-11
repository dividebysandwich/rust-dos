//! Host-side work on the emulation thread: what recording an animation
//! costs the thread that runs the machine, a captured frame at a time.

mod common;

use common::scratch;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use rust_dos::recorder::ScreenRecorder;
use rust_dos::video::Frame;

/// A 256-colour game picture as the renderer hands it over: 320x200 drawn
/// doubled to 640x400, 200 colours in bands, with a 32x32 sprite at
/// `sprite`, so that consecutive frames differ as a game's do.
fn vga_frame(sprite: usize) -> Frame {
    let (w, h) = (640usize, 400usize);
    let mut rgb = vec![0u8; w * h * 3];
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            let c = ((x / 8 + y / 5) % 200) as u8;
            let mut px = [c, c.wrapping_mul(3), 255 - c];
            if (sprite..sprite + 32).contains(&x) && (80..112).contains(&y) {
                px = [255, 255, 0];
            }
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let at = ((2 * y + dy) * w + 2 * x + dx) * 3;
                rgb[at..at + 3].copy_from_slice(&px);
            }
        }
    }
    Frame::from_rgb(w as u32, h as u32, rgb)
}

/// A high-colour (VESA 16-bit) picture at 640x480: thousands of colours,
/// which the GIF encoder has to quantise to 256.
fn hicolor_frame(shift: usize) -> Frame {
    let (w, h) = (640usize, 480usize);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let x = x + shift;
            rgb.extend_from_slice(&[(x * 255 / w) as u8, (y * 255 / h) as u8, ((x + y) % 256) as u8 & 0xF8]);
        }
    }
    Frame::from_rgb(w as u32, h as u32, rgb)
}

/// One captured animation frame: the time `ScreenRecorder::capture` takes
/// on the emulation thread for a frame it records. The recorder runs at
/// a rate that records every frame it is given.
fn gif_capture(c: &mut Criterion) {
    let mut group = c.benchmark_group("host/gif_capture");
    group.sample_size(30);
    let scenarios: [(&str, fn(usize) -> Frame); 2] = [("vga_640x400", vga_frame), ("hicolor_640x480", hicolor_frame)];
    for (name, make) in scenarios {
        let frames: Vec<Frame> = (0..8).map(|i| make(i * 24)).collect();
        let dir = scratch(&format!("gif_capture_{name}"));
        let mut recorder = ScreenRecorder::new(2000);
        recorder.toggle(&dir).unwrap();
        let mut next = 0;
        group.bench_with_input(BenchmarkId::from_parameter(name), &name, |b, _| {
            b.iter(|| {
                recorder.capture(&frames[next % frames.len()]);
                next += 1;
            })
        });
        recorder.toggle(&dir).unwrap();
    }
    group.finish();
}

criterion_group!(benches, gif_capture);
criterion_main!(benches);
