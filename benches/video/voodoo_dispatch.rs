//! Variant selection for full, combine-only and unmatched pipelines.

use criterion::{BenchmarkId, Criterion};
use rust_dos::voodoo::mem::Vram;
use rust_dos::voodoo::raster::{self, RasterState, Stride, TmuRaster};
use std::hint::black_box;
use std::sync::Arc;

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
    for i in 0..fb.len() {
        fb.set(i, (i as u16).wrapping_mul(25173));
    }
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

pub fn lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("voodoo_variant_lookup");
    for (name, count, regs) in [
        (
            "first_full",
            2,
            [0x90110, 0x0D420009, 0x40400, 0, 0x8C261A0F, 0x84221C0F],
        ),
        (
            "quake_full",
            2,
            [0x90110, 0x0D420009, 0x6010, 0, 0x8C26130F, 0x8422130F],
        ),
        ("untextured", 0, [0x90110, 0x0542610A, 0x45110, 0, 0, 0]),
        (
            "combine",
            2,
            [0x843E1, 0x0D422409, 0x4110, 0xC0, 0x80000006, 0x8C261ACF],
        ),
        (
            "last_combine",
            2,
            [0x843E1, 0x0C482405, 0x4110, 0xC0, 0x80000006, 0x8C261C0F],
        ),
        (
            "menu_alpha",
            2,
            [0x843E1, 0x00482405, 0x5110, 0xC0, 0x80000006, 0x8C261C0F],
        ),
        (
            "menu_rgb",
            2,
            [0x843E1, 0x01422409, 0x5110, 0xC0, 0x80000006, 0x8C261ACF],
        ),
        ("generic", 2, [0; 6]),
        ("one_unit", 1, [0; 6]),
    ] {
        let state = two_unit_state(regs);
        group.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| black_box(raster::variant(black_box(&state), black_box(count))));
        });
    }
    group.finish();
}
