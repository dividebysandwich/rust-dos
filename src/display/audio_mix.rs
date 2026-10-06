//! The sound's two channels mixed by a 2x2 matrix, for the 3D scene's
//! sound from where its speakers are (stage/spatial.rs). The matrix goes
//! from one block's to the next over the block's frames, so that it
//! doesn't click, and adds no delay.

/// Left from left, left from right, right from left, right from right.
pub type Mix = [f32; 4];

/// The sound as it is.
pub const IDENTITY: Mix = [1.0, 0.0, 0.0, 1.0];

/// The samples (interleaved stereo) mixed, going from the mix `from` at
/// the first frame to `to` at the last.
pub fn apply(samples: &[i16], from: Mix, to: Mix, out: &mut Vec<i16>) {
    out.clear();
    let frames = samples.len() / 2;
    out.reserve(frames * 2);
    for (n, frame) in samples.chunks_exact(2).enumerate() {
        let t = if frames > 1 { n as f32 / (frames - 1) as f32 } else { 1.0 };
        let m: [f32; 4] = std::array::from_fn(|i| from[i] + (to[i] - from[i]) * t);
        let (l, r) = (frame[0] as f32, frame[1] as f32);
        let clamp = |x: f32| x.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        out.push(clamp(l * m[0] + r * m[1]));
        out.push(clamp(l * m[2] + r * m[3]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixing_glides_from_one_mix_to_the_next() {
        let mut out = Vec::new();
        apply(&[1000, 0, 1000, 0, 1000, 0], IDENTITY, [0.0, 0.0, 1.0, 0.0], &mut out);
        assert_eq!(out, [1000, 0, 500, 500, 0, 1000]);
        apply(&[30000, 30000], IDENTITY, [1.0, 1.0, 1.0, 1.0], &mut out);
        assert_eq!(out, [32767, 32767], "clipped, not wrapped");
    }
}
