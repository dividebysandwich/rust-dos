//! Criterion benchmarks for the Indeo 3 picture decoder over the two
//! staged `IV32` corpora under `tests/data/`:
//!
//! - `160x120_all_intra`: 8 intra frames (families A / E, VQ_NULL
//!   cells).
//! - `176x144_intra_period_4`: 2 intra + 6 inter frames (motion
//!   compensation with full- and half-pel vectors, families B / F).
//!
//! Each iteration decodes the whole 8-frame sequence through a fresh
//! [`Indeo3PictureDecoder`] (built outside the timed region), so the
//! time covers header parsing, the codebook arena, the plane decode,
//! the 8-bit output planes and the decoder's copy of the last
//! picture.
//!
//! Run with:
//!     cargo bench --bench indeo3_decode

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use oxideav_indeo::indeo3::Indeo3PictureDecoder;

const INTRA_BIN: &[u8] = include_bytes!("../tests/data/iv32-160x120-all-intra/samples.bin");
const INTRA_IDX: &str = include_str!("../tests/data/iv32-160x120-all-intra/samples-index.csv");
const INTER_BIN: &[u8] =
    include_bytes!("../tests/data/iv32-176x144-4frame-intra-period/samples.bin");
const INTER_IDX: &str =
    include_str!("../tests/data/iv32-176x144-4frame-intra-period/samples-index.csv");

/// The frames of a corpus, cut by its `frame,offset,size` index.
fn frames<'a>(bin: &'a [u8], idx: &str) -> Vec<&'a [u8]> {
    idx.lines()
        .skip(1)
        .map(|l| {
            let f: Vec<usize> = l.split(',').skip(1).map(|v| v.parse().unwrap()).collect();
            &bin[f[0]..f[0] + f[1]]
        })
        .collect()
}

fn decode_sequence(c: &mut Criterion) {
    let mut group = c.benchmark_group("indeo3_decode_sequence");
    for (name, bin, idx) in [
        ("160x120_all_intra", INTRA_BIN, INTRA_IDX),
        ("176x144_intra_period_4", INTER_BIN, INTER_IDX),
    ] {
        let frames = frames(bin, idx);
        group.throughput(Throughput::Elements(frames.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(name), &frames, |b, frames| {
            b.iter_batched(
                Indeo3PictureDecoder::new,
                |mut dec| {
                    for f in frames {
                        std::hint::black_box(dec.decode(f).unwrap());
                    }
                    dec
                },
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

criterion_group!(benches, decode_sequence);
criterion_main!(benches);
