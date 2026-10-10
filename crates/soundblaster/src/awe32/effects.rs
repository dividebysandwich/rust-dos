//! The EMU8000's effects: the chorus and reverb the voices send to, and
//! the bass and treble equalizer on the output. The chip has no preset
//! registers: drivers program its effects processor through the INIT
//! arrays, and the presets are recognised from the words they write.
//!
//! The chorus is the chip's structure (one delay line, the right tap
//! earlier than the left, feedback from the left); the reverb is a
//! comb/allpass network fitted to a card per preset, with early
//! reflections, and echo trains for the two delay presets; the equalizer
//! is a pair of shelving filters fitted to the card's 12 positions.

use std::f64::consts::PI;

const CHORUS_SIZE: usize = 0x4000;

/// Where the drivers write the 28 reverb words: (INIT array 1-4, slot).
const REVERB_SLOTS: [(u8, u8); 28] = [
    (1, 0x03), (1, 0x05), (4, 0x1F), (1, 0x07), (2, 0x14), (2, 0x16), (1, 0x0F),
    (1, 0x17), (1, 0x1F), (2, 0x07), (2, 0x0F), (2, 0x17), (2, 0x1D), (2, 0x1F),
    (3, 0x01), (3, 0x03), (1, 0x09), (1, 0x0B), (1, 0x11), (1, 0x13), (1, 0x19),
    (1, 0x1B), (2, 0x01), (2, 0x03), (2, 0x09), (2, 0x0B), (2, 0x11), (2, 0x13),
];

/// The words of Creative's reverb presets: rooms 1-3, halls 1-2, plate,
/// delay and panning delay.
pub const REVERB_PRESETS: [[u16; 28]; 8] = [
    [0xB488, 0xA450, 0x9550, 0x84B5, 0x383A, 0x3EB5, 0x72F4, 0x72A4, 0x7254, 0x7204, 0x7204, 0x7204, 0x4416, 0x4516,
     0xA490, 0xA590, 0x842A, 0x852A, 0x842A, 0x852A, 0x8429, 0x8529, 0x8429, 0x8529, 0x8428, 0x8528, 0x8428, 0x8528],
    [0xB488, 0xA458, 0x9558, 0x84B5, 0x383A, 0x3EB5, 0x7284, 0x7254, 0x7224, 0x7224, 0x7254, 0x7284, 0x4448, 0x4548,
     0xA440, 0xA540, 0x842A, 0x852A, 0x842A, 0x852A, 0x8429, 0x8529, 0x8429, 0x8529, 0x8428, 0x8528, 0x8428, 0x8528],
    [0xB488, 0xA460, 0x9560, 0x84B5, 0x383A, 0x3EB5, 0x7284, 0x7254, 0x7224, 0x7224, 0x7254, 0x7284, 0x4416, 0x4516,
     0xA490, 0xA590, 0x842C, 0x852C, 0x842C, 0x852C, 0x842B, 0x852B, 0x842B, 0x852B, 0x842A, 0x852A, 0x842A, 0x852A],
    [0xB488, 0xA470, 0x9570, 0x84B5, 0x383A, 0x3EB5, 0x7284, 0x7254, 0x7224, 0x7224, 0x7254, 0x7284, 0x4448, 0x4548,
     0xA440, 0xA540, 0x842B, 0x852B, 0x842B, 0x852B, 0x842A, 0x852A, 0x842A, 0x852A, 0x8429, 0x8529, 0x8429, 0x8529],
    [0xB488, 0xA470, 0x9570, 0x84B5, 0x383A, 0x3EB5, 0x7254, 0x7234, 0x7224, 0x7254, 0x7264, 0x7294, 0x44C3, 0x45C3,
     0xA404, 0xA504, 0x842A, 0x852A, 0x842A, 0x852A, 0x8429, 0x8529, 0x8429, 0x8529, 0x8428, 0x8528, 0x8428, 0x8528],
    [0xB4FF, 0xA470, 0x9570, 0x84B5, 0x383A, 0x3EB5, 0x7234, 0x7234, 0x7234, 0x7234, 0x7234, 0x7234, 0x4448, 0x4548,
     0xA440, 0xA540, 0x842A, 0x852A, 0x842A, 0x852A, 0x8429, 0x8529, 0x8429, 0x8529, 0x8428, 0x8528, 0x8428, 0x8528],
    [0xB4FF, 0xA470, 0x9500, 0x84B5, 0x333A, 0x39B5, 0x7204, 0x7204, 0x7204, 0x7204, 0x7204, 0x72F4, 0x4400, 0x4500,
     0xA4FF, 0xA5FF, 0x8420, 0x8520, 0x8420, 0x8520, 0x8420, 0x8520, 0x8420, 0x8520, 0x8420, 0x8520, 0x8420, 0x8520],
    [0xB4FF, 0xA490, 0x9590, 0x8474, 0x333A, 0x39B5, 0x7204, 0x7204, 0x7204, 0x7204, 0x7204, 0x72F4, 0x4400, 0x4500,
     0xA4FF, 0xA5FF, 0x8420, 0x8520, 0x8420, 0x8520, 0x8420, 0x8520, 0x8420, 0x8520, 0x8420, 0x8520, 0x8420, 0x8520],
];

/// The preset drivers set by default, and the chip is left with.
pub const DEFAULT_REVERB: usize = 4;

/// Room size, damping, pre-delay (ms) and output gain of each preset, as
/// fitted to the card's line out.
const REVERB_ROOMS: [[f32; 4]; 8] = [
    [0.613, 0.045, 0.0, 0.347], [0.583, 0.08, 0.0, 0.587], [0.647, 0.03, 0.0, 0.620],
    [0.692, 0.09, 0.0, 0.849], [0.672, 0.06, 0.0, 0.988], [0.694, 0.10, 0.0, 0.912],
    [0.0, 0.0, 200.0, 0.38], [0.7, 0.0, 150.0, 0.54],
];

/// The echo trains of the delay presets: period, first echo (L, R) in
/// samples, feedback a period, and gains.
struct Echo {
    period: usize,
    first: [usize; 2],
    feedback: f32,
    gain: [f32; 2],
}

const ECHOES: [Echo; 2] = [
    Echo { period: 5149, first: [5060, 4906], feedback: 0.432, gain: [0.945, 0.945] },
    Echo { period: 10408, first: [5094, 10187], feedback: 0.313, gain: [0.915, 0.496] },
];

/// Early reflections of the room presets: taps (R, L) and their weights.
const ER_TAPS: [[usize; 17]; 2] = [
    [57, 250, 327, 432, 596, 702, 778, 1030, 1135, 1388, 1493, 1570, 1646, 1927, 2003, 2197, 2273],
    [44, 220, 309, 397, 573, 662, 750, 1014, 1102, 1367, 1455, 1544, 1632, 1896, 1984, 2161, 2249],
];
const ER_WEIGHTS: [f32; 17] = [
    0.4528, 0.3256, 0.3362, 0.1924, 0.1673, 0.4171, 0.1225, 0.2145, 0.3240, 0.1304, 0.1673, 0.1871, 0.1265, 0.1897,
    0.1049, 0.0949, 0.0837,
];
const ER_GAINS: [f32; 8] = [0.639, 0.417, 0.409, 0.408, 0.324, 0.435, 0.0, 0.0];
const ER_SIZE: usize = 2400;

const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
/// The right channel's lines are this much longer.
const SPREAD: usize = 23;
const ECHO_SIZE: usize = 16384;

/// Bass and treble shelves (gain dB, corner Hz) of the 12 positions.
const TREBLE: [(f64, f64); 12] = [
    (-11.9, 1811.0), (-8.5, 1542.0), (-5.9, 1313.0), (-4.1, 1211.0), (-1.2, 1031.0), (0.0, 1000.0),
    (1.9, 1261.0), (3.5, 1671.0), (5.97, 2231.0), (7.90, 2457.0), (9.69, 2188.0), (11.16, 2231.0),
];
const BASS: [(f64, f64); 12] = [
    (-11.9, 224.0), (-8.4, 273.0), (-5.9, 321.0), (-4.0, 362.0), (-1.1, 426.0), (0.0, 100.0),
    (2.0, 500.0), (3.5, 564.0), (6.0, 636.0), (8.0, 718.0), (9.6, 778.0), (12.0, 914.0),
];

/// The words drivers write for each position: bass to INIT4 1 and 11h,
/// treble to INIT3 11h, 13h, 1Bh and INIT4 7, 0Bh, 0Dh, 17h, 19h.
pub const BASS_WORDS: [[u16; 2]; 12] = [
    [0xD26A, 0xD36A], [0xD25B, 0xD35B], [0xD24C, 0xD34C], [0xD23D, 0xD33D], [0xD21F, 0xD31F], [0xC208, 0xC308],
    [0xC219, 0xC319], [0xC22A, 0xC32A], [0xC24C, 0xC34C], [0xC26E, 0xC36E], [0xC248, 0xC384], [0xC26A, 0xC36A],
];
pub const TREBLE_WORDS: [[u16; 8]; 12] = [
    [0x821E, 0xC26A, 0x031E, 0xC36A, 0x021E, 0xD208, 0x831E, 0xD308],
    [0x821E, 0xC25B, 0x031E, 0xC35B, 0x021E, 0xD208, 0x831E, 0xD308],
    [0x821E, 0xC24C, 0x031E, 0xC34C, 0x021E, 0xD208, 0x831E, 0xD308],
    [0x821E, 0xC23D, 0x031E, 0xC33D, 0x021E, 0xD208, 0x831E, 0xD308],
    [0x821E, 0xC21F, 0x031E, 0xC31F, 0x021E, 0xD208, 0x831E, 0xD308],
    [0x821E, 0xD208, 0x031E, 0xD308, 0x021E, 0xD208, 0x831E, 0xD308],
    [0x821E, 0xD208, 0x031E, 0xD308, 0x021D, 0xD219, 0x831D, 0xD319],
    [0x821E, 0xD208, 0x031E, 0xD308, 0x021C, 0xD22A, 0x831C, 0xD32A],
    [0x821E, 0xD208, 0x031E, 0xD308, 0x021A, 0xD24C, 0x831A, 0xD34C],
    [0x821E, 0xD208, 0x031E, 0xD308, 0x0219, 0xD26E, 0x8319, 0xD36E],
    [0x821D, 0xD219, 0x031D, 0xD319, 0x0219, 0xD26E, 0x8319, 0xD36E],
    [0x821C, 0xD22A, 0x031C, 0xD32A, 0x0219, 0xD26E, 0x8319, 0xD36E],
];
pub const DEFAULT_BASS: usize = 5;
pub const DEFAULT_TREBLE: usize = 9;

/// The INIT arrays, as the effects read them.
pub struct Init<'a> {
    pub init: [&'a [u16; 32]; 4],
}

impl Init<'_> {
    fn get(&self, array: u8, slot: u8) -> u16 {
        self.init[array as usize - 1][slot as usize]
    }
}

/// The chorus: its parameters from the INIT arrays and HWCF4/5, and its
/// delay line.
#[derive(Clone, Debug)]
pub struct Chorus {
    pub feedback: i32,
    pub delay: i32,
    pub depth: f64,
    /// How much earlier the right channel's tap reads, in samples.
    pub right_offset: f64,
    /// LFO step and phase, 16.32 (65536 a period).
    pub lfo_inc: u64,
    pub lfo_pos: u64,
    write: usize,
    line: Box<[i32]>,
}

impl Default for Chorus {
    fn default() -> Self {
        Self {
            feedback: 0,
            delay: 0,
            depth: 0.0,
            right_offset: 0.0,
            lfo_inc: 0,
            lfo_pos: 0,
            write: 0,
            line: vec![0; CHORUS_SIZE].into_boxed_slice(),
        }
    }
}

impl Chorus {
    fn tap(&self, delay: f64) -> i32 {
        let pos = self.write as f64 - delay;
        let read = pos.floor();
        let fraction = ((pos - read) * 65536.0) as i32;
        let read = (read as i64).rem_euclid(CHORUS_SIZE as i64) as usize;
        let next = (read + 1) % CHORUS_SIZE;
        self.line[read] + (((self.line[next] - self.line[read]) as i64 * fraction as i64) >> 16) as i32
    }

    /// One sample: the send in, the left and right return out. The LFO is
    /// a triangle: the card's delay moves linearly between its extremes.
    pub fn run(&mut self, input: i32) -> (i32, i32) {
        let int = (self.lfo_pos >> 32) & 0xFFFF;
        let fract = (self.lfo_pos >> 16) & 0xFFFF;
        let phase = (int as f64 + fract as f64 / 65536.0) / 65536.0;
        let tri = if phase < 0.5 { 4.0 * phase - 1.0 } else { 3.0 - 4.0 * phase };
        let delay_l = self.delay as f64 + tri * self.depth;
        let delay_r = delay_l - self.right_offset;
        let l = self.tap(delay_l);
        let r = self.tap(delay_r);
        self.line[self.write] = input.wrapping_add(((l as i64 * self.feedback as i64) >> 8) as i32);
        self.write = (self.write + 1) % CHORUS_SIZE;
        self.lfo_pos = self.lfo_pos.wrapping_add(self.lfo_inc) & 0xFFFF_FFFF_FFFF;
        (l, r)
    }

    pub fn clear(&mut self) {
        self.line.fill(0);
    }
}

/// The reverb, fitted to the card preset by preset.
#[derive(Clone, Debug)]
pub struct Reverb {
    pub preset: usize,
    feedback: f32,
    damp: f32,
    in_gain: f32,
    out_gain: f32,
    er_gain: f32,
    /// 0 for the comb network, 1 or 2 for the echo presets.
    echo_mode: usize,
    pre_len: usize,
    lines: ReverbLines,
}

/// The reverb's delay lines, which save states leave out.
#[derive(Clone, Debug)]
struct ReverbLines {
    comb: [[Box<[f32]>; 8]; 2],
    comb_pos: [[usize; 8]; 2],
    comb_store: [[f32; 8]; 2],
    allpass: [[Box<[f32]>; 4]; 2],
    allpass_pos: [[usize; 4]; 2],
    pre: Box<[f32]>,
    pre_pos: usize,
    echo_x: Box<[f32]>,
    echo_z: Box<[f32]>,
    echo_pos: usize,
    er: Box<[f32]>,
    er_pos: usize,
}

impl ReverbLines {
    fn new() -> Self {
        let line = |len: usize| vec![0.0; len].into_boxed_slice();
        Self {
            comb: std::array::from_fn(|ch| std::array::from_fn(|i| line(COMBS[i] + ch * SPREAD))),
            comb_pos: [[0; 8]; 2],
            comb_store: [[0.0; 8]; 2],
            allpass: std::array::from_fn(|ch| std::array::from_fn(|i| line(ALLPASSES[i] + ch * SPREAD))),
            allpass_pos: [[0; 4]; 2],
            pre: line(0),
            pre_pos: 0,
            echo_x: line(ECHO_SIZE),
            echo_z: line(ECHO_SIZE),
            echo_pos: 0,
            er: line(ER_SIZE),
            er_pos: 0,
        }
    }
}

impl Default for Reverb {
    fn default() -> Self {
        let mut reverb = Self {
            preset: usize::MAX,
            feedback: 0.0,
            damp: 0.0,
            in_gain: 0.0,
            out_gain: 0.0,
            er_gain: 0.0,
            echo_mode: 0,
            pre_len: 0,
            lines: ReverbLines::new(),
        };
        reverb.set_preset(DEFAULT_REVERB);
        reverb
    }
}

impl Reverb {
    pub fn set_preset(&mut self, preset: usize) {
        let room = REVERB_ROOMS[preset];
        self.preset = preset;
        self.er_gain = ER_GAINS[preset];
        self.echo_mode = match preset {
            6 => 1,
            7 => 2,
            _ => 0,
        };
        if self.echo_mode != 0 {
            self.lines.echo_pos = 0;
            self.lines.echo_x.fill(0.0);
            self.lines.echo_z.fill(0.0);
        }
        self.feedback = 0.7 + room[0] * 0.28;
        self.damp = room[1];
        self.in_gain = 1.0 - self.feedback;
        self.out_gain = room[3];
        // The echo presets have no pre-delay of their own.
        let len = if self.echo_mode != 0 { 0 } else { (room[2] * 44100.0 / 1000.0) as usize };
        if len != self.pre_len || self.lines.pre.len() != len {
            self.pre_len = len;
            self.lines.pre = vec![0.0; len].into_boxed_slice();
            self.lines.pre_pos = 0;
        }
    }

    /// Recognise the preset from the INIT words: at least 24 of the 28
    /// have to match.
    pub fn decode(&mut self, init: &Init) {
        let mut best = None;
        let mut best_hits = 0;
        for (p, words) in REVERB_PRESETS.iter().enumerate() {
            let hits = REVERB_SLOTS.iter().zip(words).filter(|&(&(a, s), &w)| init.get(a, s) == w).count();
            if hits > best_hits {
                best_hits = hits;
                best = Some(p);
            }
        }
        if let Some(p) = best
            && best_hits >= 24
            && p != self.preset
        {
            self.set_preset(p);
        }
    }

    /// One sample: the send in, the return (L, R) out.
    pub fn run(&mut self, input: i32) -> (i32, i32) {
        let lines = &mut self.lines;
        let mut x = input as f32 / 32768.0;
        if self.pre_len > 0 {
            let delayed = lines.pre[lines.pre_pos];
            lines.pre[lines.pre_pos] = x;
            lines.pre_pos = (lines.pre_pos + 1) % self.pre_len;
            x = delayed;
        }
        let mut out = [0i32; 2];
        if self.echo_mode != 0 {
            // x is the input's history, z every echo after the first.
            let echo = &ECHOES[self.echo_mode - 1];
            let pos = lines.echo_pos;
            let old = (pos + ECHO_SIZE - echo.period) & (ECHO_SIZE - 1);
            lines.echo_z[pos] = echo.feedback * (lines.echo_x[old] + lines.echo_z[old]);
            lines.echo_x[pos] = x;
            for (ch, o) in out.iter_mut().enumerate() {
                let p = (pos + ECHO_SIZE - echo.first[ch]) & (ECHO_SIZE - 1);
                let v = lines.echo_x[p] + lines.echo_z[p];
                *o = (v * echo.gain[ch] * 32768.0) as i32;
            }
            lines.echo_pos = (pos + 1) & (ECHO_SIZE - 1);
            return (out[0], out[1]);
        }
        let scaled = x * self.in_gain;
        let mut er = [0.0f32; 2];
        if self.er_gain > 0.0 {
            lines.er[lines.er_pos] = x;
            for (ch, e) in er.iter_mut().enumerate() {
                let sum: f32 = ER_TAPS[ch]
                    .iter()
                    .zip(ER_WEIGHTS)
                    .map(|(&tap, w)| lines.er[(lines.er_pos + ER_SIZE - tap) % ER_SIZE] * w)
                    .sum();
                *e = sum * self.er_gain;
            }
            lines.er_pos = (lines.er_pos + 1) % ER_SIZE;
        }
        for (ch, o) in out.iter_mut().enumerate() {
            let mut acc = 0.0;
            for i in 0..8 {
                let line = &mut lines.comb[ch][i];
                let pos = lines.comb_pos[ch][i];
                let y = line[pos];
                let store = &mut lines.comb_store[ch][i];
                *store = y * (1.0 - self.damp) + *store * self.damp;
                line[pos] = scaled + *store * self.feedback;
                lines.comb_pos[ch][i] = (pos + 1) % line.len();
                acc += y;
            }
            acc *= 0.125;
            for i in 0..4 {
                let line = &mut lines.allpass[ch][i];
                let pos = lines.allpass_pos[ch][i];
                let buf = line[pos];
                line[pos] = acc + buf * 0.5;
                lines.allpass_pos[ch][i] = (pos + 1) % line.len();
                acc = buf - acc;
            }
            *o = ((acc * self.out_gain + er[ch]) * 32768.0) as i32;
        }
        (out[0], out[1])
    }

    pub fn clear(&mut self) {
        let preset = self.preset;
        self.lines = ReverbLines::new();
        self.pre_len = usize::MAX;
        self.set_preset(preset);
    }
}

/// The equalizer: a bass and a treble shelf on both channels.
#[derive(Clone, Debug)]
pub struct Eq {
    pub bass: usize,
    pub treble: usize,
    coef: [[f64; 5]; 2],
    z: [[[f64; 2]; 2]; 2],
}

impl Default for Eq {
    fn default() -> Self {
        let mut eq = Self { bass: DEFAULT_BASS, treble: DEFAULT_TREBLE, coef: [[0.0; 5]; 2], z: [[[0.0; 2]; 2]; 2] };
        eq.design();
        eq
    }
}

/// Whether a written word is a table's. The DOS game driver writes the
/// low byte with its nibbles swapped, which sounds the same on the card.
fn word_matches(reg: u16, table: u16) -> bool {
    let swapped = (reg & 0xFF00) | ((reg & 0x0F) << 4) | ((reg & 0xF0) >> 4);
    reg == table || swapped == table
}

/// An RBJ shelf with slope 0.5: b0 b1 b2 a1 a2.
fn shelf((gain_db, f0): (f64, f64), high: bool) -> [f64; 5] {
    let a = 10f64.powf(gain_db / 40.0);
    let w0 = 2.0 * PI * f0 / 44100.0;
    let cw = w0.cos();
    let alpha = w0.sin() / 2.0 * ((a + 1.0 / a) * (1.0 / 0.5 - 1.0) + 2.0).sqrt();
    let sq = 2.0 * a.sqrt() * alpha;
    let (b0, b1, b2, a0, a1, a2) = if high {
        (
            a * ((a + 1.0) + (a - 1.0) * cw + sq),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cw),
            a * ((a + 1.0) + (a - 1.0) * cw - sq),
            (a + 1.0) - (a - 1.0) * cw + sq,
            2.0 * ((a - 1.0) - (a + 1.0) * cw),
            (a + 1.0) - (a - 1.0) * cw - sq,
        )
    } else {
        (
            a * ((a + 1.0) - (a - 1.0) * cw + sq),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cw),
            a * ((a + 1.0) - (a - 1.0) * cw - sq),
            (a + 1.0) + (a - 1.0) * cw + sq,
            -2.0 * ((a - 1.0) + (a + 1.0) * cw),
            (a + 1.0) + (a - 1.0) * cw - sq,
        )
    };
    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

impl Eq {
    fn design(&mut self) {
        self.coef = [shelf(BASS[self.bass], false), shelf(TREBLE[self.treble], true)];
    }

    /// Recognise the positions from the INIT words; words that match no
    /// position leave it as it was.
    pub fn decode(&mut self, init: &Init) {
        let bass_words = [init.get(4, 0x01), init.get(4, 0x11)];
        let treble_words = [
            init.get(3, 0x11),
            init.get(3, 0x13),
            init.get(3, 0x1B),
            init.get(4, 0x07),
            init.get(4, 0x0B),
            init.get(4, 0x0D),
            init.get(4, 0x17),
            init.get(4, 0x19),
        ];
        let bass = BASS_WORDS
            .iter()
            .position(|w| w.iter().zip(bass_words).all(|(&t, r)| word_matches(r, t)))
            .unwrap_or(self.bass);
        let treble = TREBLE_WORDS
            .iter()
            .position(|w| w.iter().zip(treble_words).all(|(&t, r)| word_matches(r, t)))
            .unwrap_or(self.treble);
        if (bass, treble) != (self.bass, self.treble) {
            // The filters keep their state, so a change doesn't click.
            self.bass = bass;
            self.treble = treble;
            self.design();
        }
    }

    pub fn run(&mut self, frame: [i32; 2]) -> [i32; 2] {
        let mut out = [0; 2];
        for ch in 0..2 {
            let mut x = frame[ch] as f64;
            for s in 0..2 {
                let q = &self.coef[s];
                let z = &mut self.z[s][ch];
                let y = q[0] * x + z[0];
                z[0] = q[1] * x - q[3] * y + z[1];
                z[1] = q[2] * x - q[4] * y;
                x = y;
            }
            out[ch] = x.round() as i32;
        }
        out
    }

    pub fn clear(&mut self) {
        self.z = [[[0.0; 2]; 2]; 2];
    }
}

rust_dos_savestate::state_fields!(Chorus { feedback, delay, depth, right_offset, lfo_inc, lfo_pos } skip {
    // The sound ringing on in it.
    write, line,
});
rust_dos_savestate::state_fields!(Reverb { preset } skip {
    // Set again from the preset, and the sound ringing on.
    feedback, damp, in_gain, out_gain, er_gain, echo_mode, pre_len, lines,
});
rust_dos_savestate::state_fields!(Eq { bass, treble } skip {
    coef, z,
});

impl Reverb {
    /// After a state was loaded: the preset's settings, empty lines.
    pub fn after_load(&mut self) {
        self.clear();
    }
}

impl Eq {
    pub fn after_load(&mut self) {
        self.design();
        self.clear();
    }
}
