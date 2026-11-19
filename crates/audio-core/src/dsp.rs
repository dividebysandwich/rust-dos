//! The mixer's filters and effects: low- and high-pass filters that give
//! the PC speaker and the Sound Blaster the sound of the real thing, and a
//! reverb and a chorus for the synthesizers. Everything works one stereo
//! frame at a time, as the mixer renders (a program's change to the sound
//! can come after any frame), keeping its state between frames.

use std::f32::consts::PI;

/// The mixer's sample rate.
const RATE: f32 = crate::RATE as f32;

/// The resonance of a second-order Butterworth filter, the flattest.
pub const BUTTERWORTH_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Below this, a filter's state is silence (samples are 16-bit values, so
/// it is far below the smallest step): tails end in zeros, not in denormal
/// numbers, which are slow, and silence stays exactly silent.
const TINY: f32 = 1e-10;

fn flush(x: f32) -> f32 {
    if x.abs() < TINY { 0.0 } else { x }
}

/// A second-order filter section (the Audio EQ Cookbook's), in transposed
/// direct form II.
#[derive(Clone, Copy, Debug, Default)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    /// A low-pass at `cutoff` Hz with resonance `q` (0.707 for
    /// Butterworth).
    pub fn lowpass(cutoff: f32, q: f32) -> Self {
        let (cos, alpha) = Self::angle(cutoff, q);
        let b1 = 1.0 - cos;
        Self::normalized(b1 / 2.0, b1, b1 / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    /// A high-pass at `cutoff` Hz with resonance `q`.
    pub fn highpass(cutoff: f32, q: f32) -> Self {
        let (cos, alpha) = Self::angle(cutoff, q);
        let b0 = (1.0 + cos) / 2.0;
        Self::normalized(b0, -(1.0 + cos), b0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    fn angle(cutoff: f32, q: f32) -> (f32, f32) {
        let w = 2.0 * PI * cutoff.clamp(1.0, RATE * 0.49) / RATE;
        (w.cos(), w.sin() / (2.0 * q))
    }

    fn normalized(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Self { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, z1: 0.0, z2: 0.0 }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        // Both at once: clearing one alone would make the filter ring.
        if self.z1.abs() < TINY && self.z2.abs() < TINY {
            self.z1 = 0.0;
            self.z2 = 0.0;
        }
        y
    }

    /// Whether silence in gives silence out and leaves it as it is.
    pub fn at_rest(&self) -> bool {
        self.z1 == 0.0 && self.z2 == 0.0
    }
}

/// A first-order high-pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePoleHighpass {
    a: f32,
    x1: f32,
    y1: f32,
}

impl OnePoleHighpass {
    pub fn new(cutoff: f32) -> Self {
        let rc = 1.0 / (2.0 * PI * cutoff);
        Self { a: rc / (rc + 1.0 / RATE), x1: 0.0, y1: 0.0 }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        // The input's change first: on a level held, exactly 0. Adding the
        // small output to the large input first rounds it to the input's
        // precision, and the decay stalls at a few units off silence.
        let y = flush(self.a * (self.y1 + (x - self.x1)));
        self.x1 = x;
        self.y1 = y;
        y
    }

    /// Whether `x` would give exactly 0 and leave the filter as it is: it
    /// got that level last, and what it passed of it has faded out.
    pub fn rests_on(&self, x: f32) -> bool {
        self.x1 == x && self.y1 == 0.0
    }
}

/// A first-order high-pass at `COUPLING_HZ` on one channel, which takes a
/// steady level out of a source and passes its sound as it was. It starts
/// settled on the first sample it gets after `Default`, as
/// `OutputCoupling` does.
#[derive(Clone, Copy, Debug)]
pub struct DcBlocker {
    filter: OnePoleHighpass,
    settled: bool,
}

impl Default for DcBlocker {
    fn default() -> Self {
        Self { filter: OnePoleHighpass::new(COUPLING_HZ), settled: false }
    }
}

impl DcBlocker {
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        if !self.settled {
            self.settled = true;
            self.filter.x1 = x;
        }
        self.filter.process(x)
    }

    /// Whether `x` would give exactly 0 and leave the filter as it is.
    pub fn rests_on(&self, x: f32) -> bool {
        !self.settled && x == 0.0 || self.settled && self.filter.rests_on(x)
    }
}

/// Where the capacitor in series with a sound card's output starts to cut,
/// in hertz. With the input behind it, it is a first-order high-pass at
/// 1/(2 pi R C): 4.7 uF into a 10 kOhm line input (TI's application note
/// SDAA293 on output coupling capacitors) makes about 3 Hz, far below
/// anything heard.
pub const COUPLING_HZ: f32 = 3.0;

/// The capacitor in series with each of a sound card's outputs, left and
/// right: a first-order high-pass at `COUPLING_HZ`. A sound card's
/// output reaches the line input or the speakers through a capacitor,
/// which keeps the bias voltage of its amplifier off them (the SB 2.0's
/// TEA2025B, a single-supply amplifier, drives its speakers that way). A
/// DAC holds its last sample until the next, however long; the capacitor
/// lets none of that steady level through, and a level the DAC steps to
/// fades within a fraction of a second (the time constant is 53 ms),
/// heard as a click at most.
///
/// It starts settled on the first frame it gets after `Default` (a reset
/// of the mixer's filters, as after a state was loaded): a level the DAC
/// already held then passes as it would after a long while, as silence,
/// without a click of its own.
#[derive(Clone, Copy, Debug)]
pub struct OutputCoupling {
    filters: [OnePoleHighpass; 2],
    settled: bool,
}

impl Default for OutputCoupling {
    fn default() -> Self {
        Self {
            filters: [OnePoleHighpass::new(COUPLING_HZ); 2],
            settled: false,
        }
    }
}

impl OutputCoupling {
    #[inline]
    pub fn process(&mut self, (l, r): (f32, f32)) -> (f32, f32) {
        if !self.settled {
            self.settled = true;
            self.filters[0].x1 = l;
            self.filters[1].x1 = r;
        }
        (self.filters[0].process(l), self.filters[1].process(r))
    }

    /// Whether a frame of `(l, r)` would give silence and leave the
    /// capacitor as it is: it got that level last, and what it passed of
    /// it has faded to exactly 0.
    pub fn rests_on(&self, (l, r): (f32, f32)) -> bool {
        let [left, right] = &self.filters;
        self.settled && left.x1 == l && right.x1 == r && left.y1 == 0.0 && right.y1 == 0.0
    }
}

/// The small speaker of a PC, as DOSBox Staging has it: a third-order
/// high-pass at 120 Hz, as it can't move much air, and a second-order
/// low-pass at 4.8 kHz. The high-pass takes out the steady level the
/// speaker's cone rests at, so it starts settled on the first sample it
/// gets after `Default` (a state loaded with the speaker's output high
/// gives no click).
#[derive(Clone, Copy, Debug)]
pub struct SpeakerFilter {
    first: OnePoleHighpass,
    high: Biquad,
    low: Biquad,
    settled: bool,
}

impl Default for SpeakerFilter {
    fn default() -> Self {
        Self {
            first: OnePoleHighpass::new(120.0),
            high: Biquad::highpass(120.0, 1.0),
            low: Biquad::lowpass(4800.0, BUTTERWORTH_Q),
            settled: false,
        }
    }
}

impl SpeakerFilter {
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        if !self.settled {
            self.settled = true;
            self.first.x1 = x;
        }
        self.low.process(self.high.process(self.first.process(x)))
    }

    /// Whether `x` would give exactly 0 and leave the filter as it is: it
    /// got that level last and has rung out.
    pub fn rests_on(&self, x: f32) -> bool {
        let rests = !self.settled && x == 0.0 || self.settled && self.first.rests_on(x);
        rests && self.high.at_rest() && self.low.at_rest()
    }
}

/// How many parts of each mix frame the PC speaker's level is taken in
/// over, for `SpeakerDecimator`: 8, a rate of 352.8 kHz.
pub const SPEAKER_OVERSAMPLE: usize = 8;

/// The taps of the speaker's decimation filter, 24 mix frames' worth.
const SPEAKER_TAPS: usize = 192;

/// The PC speaker's level, taken in as its mean over each eighth of a mix
/// frame, band-limited before one sample a frame is kept. The level is a
/// pulse train with edges at any of the PIT's 1.19 MHz ticks, and its
/// mean over a whole frame (a box filter, whose response falls off slowly)
/// lets what lies above half the mix rate fold into the audible range: a
/// pulse-width-modulated carrier's harmonics come out as tones. A
/// windowed-sinc low-pass at the eighths' rate passes the audible range
/// (within 0.004 dB to 16 kHz, 1.5 dB down at 18 kHz) and takes
/// everything that would fold below 20 kHz down by 70 dB or more (from
/// 24.1 kHz on), and the eighths' own means take out what lies near
/// their rate's multiples. The cost is the same in every frame: eight
/// means and a 192-tap filter.
///
/// It starts settled on the first part it gets after `Default` (a reset
/// of the mixer's filters, as after a state was loaded), as if the level
/// had held that long. A level held for the filter's length comes out
/// exactly, so a resting speaker stays exactly at its level.
#[derive(Clone, Debug)]
pub struct SpeakerDecimator {
    /// The parts taken in, each written twice, `SPEAKER_TAPS` apart, so
    /// the last `SPEAKER_TAPS` lie in one run.
    history: Box<[f32; 2 * SPEAKER_TAPS]>,
    /// Where the next part goes, below `SPEAKER_TAPS`.
    at: usize,
    /// The last part, and how many parts in a row had its value.
    last: f32,
    steady: usize,
    settled: bool,
}

impl Default for SpeakerDecimator {
    fn default() -> Self {
        Self {
            history: Box::new([0.0; 2 * SPEAKER_TAPS]),
            at: 0,
            last: 0.0,
            steady: 0,
            settled: false,
        }
    }
}

/// The decimation filter's taps: a sinc low-pass with its cutoff at
/// 19.5 kHz, in a Kaiser window (beta 6.76, for 70 dB), summing to 1.
fn speaker_taps() -> &'static [f32; SPEAKER_TAPS] {
    static TAPS: std::sync::OnceLock<[f32; SPEAKER_TAPS]> = std::sync::OnceLock::new();
    TAPS.get_or_init(|| {
        let rate = (crate::RATE as usize * SPEAKER_OVERSAMPLE) as f64;
        let cutoff = 19_550.0 / rate;
        let beta = 0.1102 * (70.0 - 8.7);
        let middle = (SPEAKER_TAPS - 1) as f64 / 2.0;
        let mut taps = [0.0f64; SPEAKER_TAPS];
        for (i, tap) in taps.iter_mut().enumerate() {
            let x = i as f64 - middle;
            let sinc = if x == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * x)
            };
            let r = x / middle;
            *tap = sinc * bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(beta);
        }
        let sum: f64 = taps.iter().sum();
        taps.map(|tap| (tap / sum) as f32)
    })
}

/// The modified Bessel function of the first kind, order 0, by its series.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..50 {
        term *= half / k as f64;
        sum += term * term;
        if term * term < sum * 1e-17 {
            break;
        }
    }
    sum
}

impl SpeakerDecimator {
    /// Take in the level's mean over the next eighth of a frame.
    #[inline]
    pub fn push(&mut self, x: f32) {
        if !self.settled {
            self.settled = true;
            self.history.fill(x);
            self.last = x;
            self.steady = SPEAKER_TAPS;
            return;
        }
        if x == self.last {
            self.steady = self.steady.saturating_add(1);
        } else {
            self.last = x;
            self.steady = 1;
        }
        self.history[self.at] = x;
        self.history[self.at + SPEAKER_TAPS] = x;
        self.at = if self.at + 1 == SPEAKER_TAPS {
            0
        } else {
            self.at + 1
        };
    }

    /// The sample for the frame whose last eighth came in last.
    #[inline]
    pub fn output(&self) -> f32 {
        if self.steady >= SPEAKER_TAPS {
            return self.last;
        }
        // The filter is symmetric, so the order the parts lie in, oldest
        // first from `at`, doesn't matter to it.
        let parts = &self.history[self.at..self.at + SPEAKER_TAPS];
        parts.iter().zip(speaker_taps()).map(|(x, h)| x * h).sum()
    }

    /// Whether parts of `x` from now on give `x` exactly and leave the
    /// filter as it is: it has had `x` for its whole length, or nothing
    /// yet.
    pub fn rests_on(&self, x: f32) -> bool {
        !self.settled || self.steady >= SPEAKER_TAPS && self.last == x
    }
}

/// A Butterworth low-pass on both channels, second or fourth order, at a
/// cutoff that can change, as a Sound Blaster's output filter follows its
/// model and the sample rate.
#[derive(Clone, Copy, Debug, Default)]
pub struct StereoLowpass {
    cutoff: f32,
    order: usize,
    left: [Biquad; 2],
    right: [Biquad; 2],
}

impl StereoLowpass {
    /// Filter a frame at `cutoff` Hz, `order` 2 or 4.
    #[inline]
    pub fn process(&mut self, (l, r): (f32, f32), cutoff: f32, order: usize) -> (f32, f32) {
        if cutoff != self.cutoff || order != self.order {
            self.design(cutoff, order);
        }
        let mut out = (l, r);
        for i in 0..order / 2 {
            out = (self.left[i].process(out.0), self.right[i].process(out.1));
        }
        out
    }

    /// New coefficients, keeping the state, so nothing clicks.
    fn design(&mut self, cutoff: f32, order: usize) {
        let qs: &[f32] = if order >= 4 { &[0.5412, 1.3066] } else { &[BUTTERWORTH_Q] };
        for (i, &q) in qs.iter().enumerate() {
            let fresh = Biquad::lowpass(cutoff, q);
            for section in [&mut self.left[i], &mut self.right[i]] {
                *section = Biquad { z1: section.z1, z2: section.z2, ..fresh };
            }
        }
        self.cutoff = cutoff;
        self.order = order;
    }
}

/// A delay line of a comb filter with damping, as in Freeverb.
#[derive(Clone, Debug)]
struct Comb {
    buffer: Vec<f32>,
    at: usize,
    store: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Self { buffer: vec![0.0; len], at: 0, store: 0.0 }
    }

    #[inline]
    fn process(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let out = self.buffer[self.at];
        self.store = flush(out * (1.0 - damp) + self.store * damp);
        self.buffer[self.at] = flush(input + self.store * feedback);
        self.at = if self.at + 1 == self.buffer.len() { 0 } else { self.at + 1 };
        out
    }
}

/// An all-pass section of Freeverb.
#[derive(Clone, Debug)]
struct Allpass {
    buffer: Vec<f32>,
    at: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self { buffer: vec![0.0; len], at: 0 }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.at];
        self.buffer[self.at] = flush(input + delayed * 0.5);
        self.at = if self.at + 1 == self.buffer.len() { 0 } else { self.at + 1 };
        delayed - input
    }
}

/// The reverb presets (`reverb`), from a small room to a hall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReverbPreset {
    Off,
    Tiny,
    Small,
    Medium,
    Large,
    Huge,
}

impl ReverbPreset {
    pub const ALL: [ReverbPreset; 6] =
        [ReverbPreset::Off, ReverbPreset::Tiny, ReverbPreset::Small, ReverbPreset::Medium, ReverbPreset::Large, ReverbPreset::Huge];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name().eq_ignore_ascii_case(s.trim()))
    }

    pub fn name(self) -> &'static str {
        match self {
            ReverbPreset::Off => "off",
            ReverbPreset::Tiny => "tiny",
            ReverbPreset::Small => "small",
            ReverbPreset::Medium => "medium",
            ReverbPreset::Large => "large",
            ReverbPreset::Huge => "huge",
        }
    }

    /// How much of the FM synthesizer, the Gravis Ultrasound and MIDI goes
    /// into the reverb, as DOSBox Staging's presets send.
    pub fn synth_send(self) -> f32 {
        match self {
            ReverbPreset::Off => 0.0,
            ReverbPreset::Tiny => 0.65,
            ReverbPreset::Small => 0.40,
            ReverbPreset::Medium => 0.54,
            ReverbPreset::Large => 0.70,
            ReverbPreset::Huge => 0.85,
        }
    }

    /// The room size and damping, and the high-pass that keeps the rumble
    /// out, of the preset.
    fn room(self) -> (f32, f32, f32) {
        match self {
            ReverbPreset::Off | ReverbPreset::Tiny => (0.25, 0.7, 200.0),
            ReverbPreset::Small => (0.45, 0.6, 200.0),
            ReverbPreset::Medium => (0.65, 0.5, 170.0),
            ReverbPreset::Large => (0.8, 0.4, 140.0),
            ReverbPreset::Huge => (0.9, 0.3, 140.0),
        }
    }
}

/// Freeverb (Jezar at Dreampoint's public domain reverb): eight parallel
/// damped combs and four all-passes in series for each channel, the right
/// channel's a little longer for width. The output is the reverb alone.
#[derive(Clone, Debug)]
pub struct Reverb {
    combs: [Vec<Comb>; 2],
    allpasses: [Vec<Allpass>; 2],
    feedback: f32,
    damp: f32,
    input_filter: [Biquad; 2],
}

impl Reverb {
    const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
    const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
    const SPREAD: usize = 23;
    const INPUT_GAIN: f32 = 0.015;

    pub fn new(preset: ReverbPreset) -> Self {
        let (room, damp, highpass) = preset.room();
        let side = |spread: usize| -> (Vec<Comb>, Vec<Allpass>) {
            (
                Self::COMBS.iter().map(|&len| Comb::new(len + spread)).collect(),
                Self::ALLPASSES.iter().map(|&len| Allpass::new(len + spread)).collect(),
            )
        };
        let (left_combs, left_allpasses) = side(0);
        let (right_combs, right_allpasses) = side(Self::SPREAD);
        Self {
            combs: [left_combs, right_combs],
            allpasses: [left_allpasses, right_allpasses],
            feedback: room * 0.28 + 0.7,
            damp: damp * 0.4,
            input_filter: [Biquad::highpass(highpass, BUTTERWORTH_Q); 2],
        }
    }

    /// The reverb of a frame sent into it.
    #[inline]
    pub fn process(&mut self, (l, r): (f32, f32)) -> (f32, f32) {
        let l = self.input_filter[0].process(l);
        let r = self.input_filter[1].process(r);
        let input = (l + r) * Self::INPUT_GAIN;
        let mut out = [0.0f32; 2];
        for (side, wet) in out.iter_mut().enumerate() {
            let mut sum = 0.0;
            for comb in &mut self.combs[side] {
                sum += comb.process(input, self.feedback, self.damp);
            }
            for allpass in &mut self.allpasses[side] {
                sum = allpass.process(sum);
            }
            *wet = sum;
        }
        (out[0], out[1])
    }
}

/// The chorus presets (`chorus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChorusPreset {
    Off,
    Light,
    Normal,
    Strong,
}

impl ChorusPreset {
    pub const ALL: [ChorusPreset; 4] = [ChorusPreset::Off, ChorusPreset::Light, ChorusPreset::Normal, ChorusPreset::Strong];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.name().eq_ignore_ascii_case(s.trim()))
    }

    pub fn name(self) -> &'static str {
        match self {
            ChorusPreset::Off => "off",
            ChorusPreset::Light => "light",
            ChorusPreset::Normal => "normal",
            ChorusPreset::Strong => "strong",
        }
    }

    /// How much of the FM synthesizer, the Gravis Ultrasound and MIDI goes
    /// into the chorus, as DOSBox Staging's presets send.
    pub fn synth_send(self) -> f32 {
        match self {
            ChorusPreset::Off => 0.0,
            ChorusPreset::Light => 0.33,
            ChorusPreset::Normal => 0.54,
            ChorusPreset::Strong => 0.75,
        }
    }
}

/// A stereo chorus: each channel delayed by about 12 ms, the delay swept
/// slowly up and down by a few ms, the right channel's a quarter turn
/// behind. The output is the chorus alone.
#[derive(Clone, Debug)]
pub struct Chorus {
    buffer: [Vec<f32>; 2],
    at: usize,
    phase: f32,
}

impl Default for Chorus {
    fn default() -> Self {
        Self::new()
    }
}

impl Chorus {
    const DELAY: f32 = 0.012 * RATE;
    const DEPTH: f32 = 0.003 * RATE;
    const SPEED: f32 = 0.4 / RATE;
    const LEN: usize = 1024;

    pub fn new() -> Self {
        Self { buffer: [vec![0.0; Self::LEN], vec![0.0; Self::LEN]], at: 0, phase: 0.0 }
    }

    #[inline]
    pub fn process(&mut self, (l, r): (f32, f32)) -> (f32, f32) {
        self.buffer[0][self.at] = l;
        self.buffer[1][self.at] = r;
        let mut out = [0.0f32; 2];
        for (side, turn) in [(0usize, 0.0f32), (1, 0.25)] {
            let lfo = (2.0 * PI * (self.phase + turn)).sin();
            let delay = Self::DELAY + Self::DEPTH * lfo;
            let back = delay.floor();
            let frac = delay - back;
            let a = (self.at + Self::LEN - back as usize) % Self::LEN;
            let b = (a + Self::LEN - 1) % Self::LEN;
            out[side] = self.buffer[side][a] * (1.0 - frac) + self.buffer[side][b] * frac;
        }
        self.at = (self.at + 1) % Self::LEN;
        self.phase += Self::SPEED;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        (out[0], out[1])
    }
}

// A filter's memory; its coefficients come with the device it is in.
rust_dos_savestate::state_fields!(Biquad { z1, z2 } skip { b0, b1, b2, a1, a2 });
rust_dos_savestate::state_fields!(OnePoleHighpass { x1, y1 } skip { a });


#[cfg(test)]
mod tests {
    use super::*;

    /// The speaker's decimation's gain at `hz`: a sine at the eighths'
    /// rate in, one output a frame, its amplitude after the filter filled.
    fn decimated_gain(hz: f64) -> f64 {
        let rate = (crate::RATE as usize * SPEAKER_OVERSAMPLE) as f64;
        let mut decimator = SpeakerDecimator::default();
        let mut peak = 0.0f64;
        for n in 0..SPEAKER_OVERSAMPLE * 4000 {
            let x = (2.0 * std::f64::consts::PI * hz * n as f64 / rate).sin();
            decimator.push(x as f32);
            if (n + 1) % SPEAKER_OVERSAMPLE == 0 && n > 4 * SPEAKER_TAPS {
                peak = peak.max(decimator.output().abs() as f64);
            }
        }
        peak
    }

    #[test]
    fn the_speakers_decimation_passes_the_audible_range_and_stops_what_folds() {
        for hz in [100.0, 1000.0, 5000.0, 12_000.0] {
            let gain = decimated_gain(hz);
            assert!((gain - 1.0).abs() < 0.01, "{hz} Hz at {gain}");
        }
        // What would fold below 20 kHz at the mix's rate: 70 dB down.
        for hz in [24_100.0, 27_500.0, 60_000.0, 82_500.0, 170_000.0] {
            let gain = decimated_gain(hz);
            assert!(gain < 0.000_35, "{hz} Hz at {gain}");
        }
    }

    #[test]
    fn a_held_level_comes_out_exactly() {
        let mut decimator = SpeakerDecimator::default();
        decimator.push(0.25);
        assert_eq!(decimator.output(), 0.25);
        assert!(decimator.rests_on(0.25));
        decimator.push(1.0);
        assert!(!decimator.rests_on(1.0));
        for _ in 2..SPEAKER_TAPS {
            decimator.push(1.0);
            assert!(!decimator.rests_on(1.0));
        }
        decimator.push(1.0);
        assert!(decimator.rests_on(1.0));
        assert_eq!(decimator.output(), 1.0);
    }

    /// The loudest output of `filter` after it settled on a sine at `hz`.
    fn gain(mut filter: impl FnMut(f32) -> f32, hz: f32) -> f32 {
        let mut peak = 0.0f32;
        for n in 0..8820 {
            let y = filter((2.0 * PI * hz * n as f32 / RATE).sin());
            if n > 4410 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn a_lowpass_passes_the_bass_and_stops_the_treble() {
        let mut low = Biquad::lowpass(1000.0, BUTTERWORTH_Q);
        assert!((gain(|x| low.process(x), 100.0) - 1.0).abs() < 0.02);
        let mut low = Biquad::lowpass(1000.0, BUTTERWORTH_Q);
        assert!((gain(|x| low.process(x), 1000.0) - BUTTERWORTH_Q).abs() < 0.02);
        let mut low = Biquad::lowpass(1000.0, BUTTERWORTH_Q);
        assert!(gain(|x| low.process(x), 10000.0) < 0.02);
    }

    #[test]
    fn a_highpass_removes_dc() {
        let mut high = Biquad::highpass(120.0, BUTTERWORTH_Q);
        let mut last = 1.0;
        for _ in 0..44100 {
            last = high.process(1.0);
        }
        assert!(last.abs() < 1e-3, "{}", last);
        let mut one = OnePoleHighpass::new(120.0);
        for _ in 0..44100 {
            last = one.process(1.0);
        }
        assert!(last.abs() < 1e-3, "{}", last);
    }

    #[test]
    fn silence_stays_exactly_silent() {
        let mut speaker = SpeakerFilter::default();
        let mut reverb = Reverb::new(ReverbPreset::Huge);
        let mut chorus = Chorus::new();
        let mut sb = StereoLowpass::default();
        for _ in 0..10000 {
            assert_eq!(speaker.process(0.0), 0.0);
            assert_eq!(reverb.process((0.0, 0.0)), (0.0, 0.0));
            assert_eq!(chorus.process((0.0, 0.0)), (0.0, 0.0));
            assert_eq!(sb.process((0.0, 0.0), 3200.0, 4), (0.0, 0.0));
        }
        // And a tail ends in zeros.
        speaker.process(3000.0);
        for _ in 0..200_000 {
            speaker.process(0.0);
        }
        assert_eq!(speaker.process(0.0), 0.0);
    }

    /// A level the DAC steps to and holds comes through at once and
    /// fades: under a tenth within 150 ms and out within a second. A tone
    /// passes as it was.
    #[test]
    fn the_output_coupling_lets_a_held_level_fade() {
        let mut coupling = OutputCoupling::default();
        assert_eq!(coupling.process((0.0, 0.0)), (0.0, 0.0));
        let held = (16384.0, -8192.0);
        let out: Vec<(f32, f32)> = (0..44100).map(|_| coupling.process(held)).collect();
        assert!((out[0].0 - 16384.0).abs() < 16.0, "{:?}", out[0]);
        assert!((out[0].1 + 8192.0).abs() < 8.0, "{:?}", out[0]);
        let tenth = out[6615];
        assert!(tenth.0 > 0.0 && tenth.0 < 1638.0, "{:?}", tenth);
        assert!(tenth.1 < 0.0 && tenth.1 > -819.0, "{:?}", tenth);
        let second = out[44099];
        assert!(second.0.abs() < 1.0 && second.1.abs() < 1.0, "{:?}", second);
        let mut coupling = OutputCoupling::default();
        let gain = gain(|x| coupling.process((x, x)).0, 100.0);
        assert!((gain - 1.0).abs() < 0.01, "{}", gain);
    }

    /// After a reset of the mixer's filters the level the DAC holds is
    /// silence from the first frame, as it would be after a long while,
    /// and what the DAC plays next passes.
    #[test]
    fn the_output_coupling_starts_settled() {
        let mut coupling = OutputCoupling::default();
        for _ in 0..1000 {
            assert_eq!(coupling.process((16384.0, -16384.0)), (0.0, 0.0));
        }
        let step = coupling.process((16384.0 + 256.0, -16384.0));
        assert!((step.0 - 256.0).abs() < 1.0 && step.1 == 0.0, "{:?}", step);
    }

    #[test]
    fn the_reverb_rings_on_and_the_chorus_is_the_input_later() {
        let mut reverb = Reverb::new(ReverbPreset::Medium);
        reverb.process((10000.0, 10000.0));
        let tail: f32 = (0..44100).map(|_| reverb.process((0.0, 0.0)).0.abs()).sum();
        assert!(tail > 100.0, "{}", tail);
        let mut chorus = Chorus::new();
        let out: Vec<f32> = (0..2000).map(|n| chorus.process((if n == 0 { 1.0 } else { 0.0 }, 0.0)).0).collect();
        let first = out.iter().position(|&y| y != 0.0).unwrap();
        assert!((400..=700).contains(&first), "{}", first);
    }

    #[test]
    fn presets_parse_back() {
        for preset in ReverbPreset::ALL {
            assert_eq!(ReverbPreset::parse(preset.name()), Some(preset));
        }
        for preset in ChorusPreset::ALL {
            assert_eq!(ChorusPreset::parse(preset.name()), Some(preset));
        }
        assert_eq!(ReverbPreset::parse("hall"), None);
    }
}
