//! Voodoo triangle pipelines and whole-frame software/backlog work.

use criterion::{BenchmarkId, Criterion, Throughput};
use rust_dos::voodoo::mem::Vram;
use rust_dos::voodoo::raster::{self, RasterState, Stats, Stride, TmuParams, TmuRaster, TriParams};
use rust_dos::voodoo::regs::*;
use rust_dos::voodoo::{Board, Now, Voodoo};
use std::hint::black_box;
use std::sync::Arc;

/// The colour and depth buffers' starting pattern.
fn fill(fb: &Vram) {
    for i in 0..fb.len() {
        fb.set(i, (i as u16).wrapping_mul(25173));
    }
}

fn two_unit_state(regs: [u32; 6]) -> RasterState {
    let lookup: Arc<[u32]> = (0..0x10000u32)
        .map(|i| i.wrapping_mul(0x9E37_79B9))
        .collect();
    let tmu = |mode: u32| {
        let ram = Vram::new(1 << 20);
        for i in 0..ram.len() {
            ram.set(i, (i as u16).wrapping_mul(40503));
        }
        TmuRaster {
            ram,
            mask: (1 << 20) - 1,
            mode,
            unfiltered: false,
            lodmin: 0,
            lodmax: 8 << 8,
            lodbias: 0,
            lodmask: 0x1FF,
            lodoffset: [
                0, 0x20000, 0x28000, 0x2A000, 0x2A800, 0x2AA00, 0x2AA80, 0x2AAA0, 0x2AAA8,
            ],
            detailmax: 0,
            detailbias: 0,
            detailscale: 0,
            wmask: 255,
            hmask: 255,
            lookup: lookup.clone(),
            bilinear_mask: 0xFF,
        }
    };
    let fb = Vram::new(4 << 20);
    fill(&fb);
    RasterState {
        fb,
        fbz_mode: regs[0],
        fbz_color_path: regs[1],
        alpha_mode: regs[2],
        fog_mode: regs[3],
        za_color: 0,
        chroma_key: 0,
        color0: 0x8040_2010,
        color1: 0x1020_4080,
        fog_color: 0x0030_3030,
        clip_left_right: 640,
        clip_low_y_high_y: 480,
        clip_mask: 0xFFF,
        chroma_range: 0,
        fogdelta_mask: 0xFC,
        yorigin: 479,
        ymask: 0x3FF,
        rowpixels: 1024,
        stride: Stride::linear(1024),
        aux_stride: Stride::linear(1024),
        dest: 0,
        aux: Some(1 << 20),
        fogblend: std::array::from_fn(|i| (i * 4) as u8),
        fogdelta: [4; 64],
        send_config: None,
        stipple: 0,
        tmu: [Some(tmu(regs[4])), Some(tmu(regs[5]))],
    }
}

/// The same textured triangle through a matching variant and Generic.
/// No register bit changes between the two measurements.
pub fn pipelines(c: &mut Criterion) {
    let st = two_unit_state([
        0x0009_4771,
        0x0D42_2409,
        0x0000_0410,
        0xC0,
        0x8000_0006,
        0x8C26_1A0F,
    ]);
    let tp = TmuParams {
        starts: 10 << 32,
        startt: 20 << 32,
        startw: 1 << 32,
        dsdx: 3 << 28,
        dtdx: 1 << 27,
        dwdx: 1 << 20,
        dsdy: 1 << 27,
        dtdy: 3 << 28,
        dwdy: 1 << 21,
        ..Default::default()
    };
    let p = TriParams {
        startr: 100 << 12,
        startg: 50 << 12,
        startb: 20 << 12,
        starta: 200 << 12,
        startz: 0x4000 << 12,
        startw: 1 << 32,
        tmu: [tp, tp],
        ..Default::default()
    };
    let variant = raster::variant(&st, 2).expect("the Oni colour path has a variant");
    let draw = |variant, stats: &mut Stats| {
        let mut stipple = 0;
        for y in 0..240 {
            raster::scanline_as(
                variant,
                2,
                &st,
                &p,
                y,
                0,
                320 - y * 4 / 3,
                &mut stipple,
                stats,
            );
        }
    };
    // Check that this fixture exercises writes, and both paths agree.
    let mut generic = Stats::default();
    draw(None, &mut generic);
    let expected: Vec<_> = (0..st.fb.len()).map(|i| st.fb.get(i)).collect();
    fill(&st.fb);
    let mut specialised = Stats::default();
    draw(Some(variant), &mut specialised);
    assert!(generic.pixels_out > 0);
    assert_eq!(generic, specialised);
    assert!((0..st.fb.len()).all(|i| st.fb.get(i) == expected[i]));
    let mut group = c.benchmark_group("video/voodoo/pipeline");
    group.throughput(Throughput::Elements(generic.pixels_in as u64));
    for (name, variant) in [("generic", None), ("variant", Some(variant))] {
        group.bench_function(name, |b| {
            b.iter(|| {
                let mut stats = Stats::default();
                draw(black_box(variant), &mut stats);
                black_box(stats);
            });
        });
    }
    group.finish();
}

fn write(v: &mut Voodoo, reg: usize, data: u32) {
    v.write((reg * 4) as u32, data, u32::MAX, Now::default());
}

/// A 640x480 card with double buffering, as initialized by Glide.
fn card(workers: usize, mirror: bool) -> Voodoo {
    let mut v = Voodoo::with_workers(Board::Max, workers);
    v.config_write(0x40, 1);
    write(&mut v, FBI_INIT1, 10 << 4);
    write(&mut v, FBI_INIT2, 150 << 11);
    write(&mut v, FBI_INIT3, 0);
    // The size only takes with the sync registers set.
    write(&mut v, BACK_PORCH, 2 << 16 | 40);
    write(&mut v, H_SYNC, ((800 - 96 - 2) << 16) | (96 - 1));
    write(&mut v, V_SYNC, (525 - 2) << 16 | 2);
    write(&mut v, VIDEO_DIMENSIONS, 479 << 16 | 639);
    v.config_write(0xC0, 0);
    write(&mut v, FBI_INIT0, 1);
    v.config_write(0x40, 0);
    write(&mut v, CLIP_LEFT_RIGHT, 640);
    write(&mut v, CLIP_LOW_Y_HIGH_Y, 480);
    write(&mut v, FBZ_MODE, 1 << 9 | 1);
    write(&mut v, FBZ_COLOR_PATH, 0);
    write(&mut v, START_R, 180 << 12);
    write(&mut v, START_G, 90 << 12);
    write(&mut v, START_B, 40 << 12);
    write(&mut v, START_A, 255 << 12);
    v.set_mirror(mirror);
    v.set_software_picture(false);
    v.take_mirror();
    v
}

fn frame(v: &mut Voodoo) {
    // Full-screen opaque clears let the backlog discard older frames.
    write(v, COLOR1, 0);
    write(v, FASTFILL_CMD, 0);
    for i in 0..64 {
        let x = (i % 8) * 72;
        let y = (i / 8) * 52;
        for (reg, value) in [
            (VERTEX_AX, x),
            (VERTEX_AY, y),
            (VERTEX_BX, x + 64),
            (VERTEX_BY, y),
            (VERTEX_CX, x),
            (VERTEX_CY, y + 48),
        ] {
            write(v, reg, value * 16);
        }
        write(v, TRIANGLE_CMD, 0);
    }
    v.prepare_display();
    if v.mirror_attached() {
        black_box(v.take_mirror().expect("mirror attached"));
    } else {
        v.flush();
        black_box(v.frame_buffer().get(0));
    }
}

/// Includes command setup, worker dispatch and completion, or mirror
/// recording and backlog pruning. No host GL context is needed: this
/// measures the CPU work accompanying GL replay, not GPU rendering.
pub fn frames(c: &mut Criterion) {
    let mut group = c.benchmark_group("video/voodoo/frame");
    group.throughput(Throughput::Elements(64));
    for workers in [0, 4] {
        for (name, mirror) in [("software", false), ("gl_backlog", true)] {
            let mut v = card(workers, mirror);
            // Two frames, so the second's clear prunes the first.
            frame(&mut v);
            frame(&mut v);
            if mirror {
                let state = v.describe(Now::default());
                assert!(state["held_jobs"].as_u64().unwrap() > 0);
                assert!(state["pruned_jobs"].as_u64().unwrap() > 0);
            } else {
                assert_ne!(v.frame_buffer().get(0), 0);
            }
            group.bench_function(BenchmarkId::new(name, workers), |b| {
                b.iter(|| frame(&mut v));
            });
            v.flush();
        }
    }
    group.finish();
}
