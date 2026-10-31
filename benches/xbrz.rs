//! Isolated xBRZ preparation timings, without other video fixtures.

#[path = "video/xbrz.rs"]
mod xbrz;

criterion::criterion_group!(benches, xbrz::prepare);
criterion::criterion_main!(benches);
