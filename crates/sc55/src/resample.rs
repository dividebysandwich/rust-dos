//! From the Sound Canvas's rate to the mixer's: a windowed-sinc filter
//! that cuts what the slower rate can't hold, rather than folding it back
//! as interpolating alone would.

/// Taps on each side of the point.
const HALF: usize = 16;
const TAPS: usize = HALF * 2;
/// Positions between two input frames the kernel is tabled for.
const PHASES: usize = 256;

pub(super) struct Resampler {
    /// Input frames per output frame.
    step: f64,
    /// Where the next output frame is, in input frames from `history[0]`.
    pos: f64,
    history: Vec<[f32; 2]>,
    /// TAPS weights for each of PHASES + 1 positions.
    table: Vec<f32>,
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-9 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) }
}

/// A Blackman window over -1..1.
fn window(x: f64) -> f64 {
    if x.abs() >= 1.0 {
        return 0.0;
    }
    let t = std::f64::consts::PI * (x + 1.0);
    0.42 - 0.5 * t.cos() + 0.08 * (2.0 * t).cos()
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Resampler {
        let step = from as f64 / to as f64;
        // Cut a little below the slower rate's Nyquist frequency.
        let cutoff = 0.92 * (to as f64 / from as f64).min(1.0);
        let mut table = Vec::with_capacity((PHASES + 1) * TAPS);
        for phase in 0..=PHASES {
            let frac = phase as f64 / PHASES as f64;
            let weights: Vec<f64> = (0..TAPS)
                .map(|k| {
                    let x = k as f64 - (HALF as f64 - 1.0) - frac;
                    cutoff * sinc(cutoff * x) * window(x / HALF as f64)
                })
                .collect();
            // Each phase passes a steady level as it is.
            let sum: f64 = weights.iter().sum();
            table.extend(weights.iter().map(|w| (w / sum) as f32));
        }
        Resampler { step, pos: (HALF - 1) as f64, history: vec![[0.0; 2]; HALF - 1], table }
    }

    /// The next output frame, taking input frames from `input` as needed.
    #[inline]
    pub fn next(&mut self, mut input: impl FnMut() -> [f32; 2]) -> [f32; 2] {
        let base = self.pos as usize;
        while self.history.len() < base + HALF + 1 {
            self.history.push(input());
        }
        let frac = self.pos - base as f64;
        let phase = (frac * PHASES as f64).round() as usize;
        let weights = &self.table[phase * TAPS..(phase + 1) * TAPS];
        let frames = &self.history[base + 1 - HALF..base + 1 + HALF];
        let mut out = [0.0f32; 2];
        for (w, f) in weights.iter().zip(frames) {
            out[0] += w * f[0];
            out[1] += w * f[1];
        }
        self.pos += self.step;
        // Drop what no later frame needs.
        let keep_from = (self.pos as usize).saturating_sub(HALF - 1);
        if keep_from > 4096 {
            self.history.drain(..keep_from);
            self.pos -= keep_from as f64;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f64, rate: f64) -> impl FnMut() -> [f32; 2] {
        let mut n = 0u64;
        move || {
            let v = (2.0 * std::f64::consts::PI * freq * n as f64 / rate).sin() as f32;
            n += 1;
            [v, v]
        }
    }

    fn level(freq: f64) -> f32 {
        let mut r = Resampler::new(66207, 44100);
        let mut input = tone(freq, 66207.0);
        // Past the start, the loudest frame.
        (0..20000).map(|_| r.next(&mut input)[0].abs()).skip(1000).fold(0.0, f32::max)
    }

    #[test]
    fn steady_levels_pass() {
        let mut r = Resampler::new(64000, 44100);
        let out: Vec<f32> = (0..2000).map(|_| r.next(|| [0.5, -0.25])[1]).collect();
        assert!(out[100..].iter().all(|&v| (v + 0.25).abs() < 1e-4), "{:?}", &out[100..110]);
    }

    #[test]
    fn audible_tones_pass_and_ones_too_high_are_cut() {
        assert!((level(1000.0) - 1.0).abs() < 0.02, "{}", level(1000.0));
        assert!((level(15000.0) - 1.0).abs() < 0.05, "{}", level(15000.0));
        // Above 22.05 kHz it would fold back to an audible tone.
        assert!(level(28000.0) < 0.01, "{}", level(28000.0));
    }
}
