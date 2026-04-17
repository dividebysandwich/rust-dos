//! AdLib (Yamaha YM3812 / OPL2) FM synthesizer.
//!
//! Implements the full OPL2 register file, 9 channels × 2 operators, all 4
//! OPL2 waveforms, ADSR envelopes, operator feedback, FM/additive algorithms,
//! rhythm mode, AM/VIB LFOs, and the timer-status byte used by games for
//! AdLib detection. Synthesis runs in f32 at the host output rate (44100 Hz);
//! phase increments are scaled from the native 49716 Hz OPL clock. The ADSR
//! curves and attenuation mapping are approximations, not bit-exact to the
//! real YM3812.

use std::time::Instant;

const NUM_OPERATORS: usize = 18;
const NUM_CHANNELS: usize = 9;

pub const OUTPUT_RATE: f32 = 44100.0;
const OPL_CLOCK: f32 = 49716.0;

const CH_OPS: [(usize, usize); NUM_CHANNELS] = [
    (0, 3),
    (1, 4),
    (2, 5),
    (6, 9),
    (7, 10),
    (8, 11),
    (12, 15),
    (13, 16),
    (14, 17),
];

const MULT_TABLE: [u32; 16] = [1, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 20, 24, 24, 30, 30];

const ATTACK_COEF: [f32; 16] = [
    0.0, 0.000009, 0.000018, 0.000036, 0.000072, 0.000144, 0.000288, 0.000576, 0.00115, 0.00230,
    0.00460, 0.00920, 0.01840, 0.03680, 0.07360, 1.0,
];

const DECAY_COEF: [f32; 16] = [
    0.0, 0.0000022, 0.0000044, 0.0000088, 0.0000176, 0.0000352, 0.0000704, 0.0001408, 0.0002816,
    0.0005632, 0.001126, 0.002253, 0.004505, 0.009010, 0.018020, 0.036040,
];

#[inline]
fn op_offset_to_idx(offset: u8) -> Option<usize> {
    match offset {
        0x00..=0x05 => Some(offset as usize),
        0x08..=0x0D => Some(offset as usize - 2),
        0x10..=0x15 => Some(offset as usize - 4),
        _ => None,
    }
}

#[inline]
fn wave_sample(phase: f32, ws: u8) -> f32 {
    // phase is in [0, 1); map into one period of sine and apply waveform mask.
    let p = phase - phase.floor();
    let s = (p * std::f32::consts::TAU).sin();
    match ws {
        0 => s,
        1 => s.max(0.0),
        2 => s.abs(),
        3 => {
            if p < 0.25 || (p >= 0.5 && p < 0.75) {
                s.abs()
            } else {
                0.0
            }
        }
        _ => s,
    }
}

#[inline]
fn tl_to_gain(tl: u8) -> f32 {
    // Total Level: 6-bit, -0.75 dB per unit.
    // gain = 10^(-tl * 0.75 / 20)
    const LN10_OVER_20: f32 = 0.115_129_25_f32;
    (-(tl as f32) * 0.75 * LN10_OVER_20).exp()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EnvState {
    Off,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone)]
struct Operator {
    am: bool,
    vib: bool,
    egt: bool,
    ksr: bool,
    mult: u8,
    ksl: u8,
    tl: u8,
    ar: u8,
    dr: u8,
    sl: u8,
    rr: u8,
    ws: u8,

    phase: f32,
    env: f32,
    state: EnvState,
    prev_out: f32,
    prev_out2: f32,
}

impl Default for Operator {
    fn default() -> Self {
        Self {
            am: false,
            vib: false,
            egt: false,
            ksr: false,
            mult: 0,
            ksl: 0,
            tl: 0,
            ar: 0,
            dr: 0,
            sl: 0,
            rr: 0,
            ws: 0,
            phase: 0.0,
            env: 0.0,
            state: EnvState::Off,
            prev_out: 0.0,
            prev_out2: 0.0,
        }
    }
}

impl Operator {
    fn key_on(&mut self) {
        self.phase = 0.0;
        self.prev_out = 0.0;
        self.prev_out2 = 0.0;
        self.state = EnvState::Attack;
    }

    fn key_off(&mut self) {
        if self.state != EnvState::Off {
            self.state = EnvState::Release;
        }
    }

    fn step_envelope(&mut self) {
        match self.state {
            EnvState::Off => {}
            EnvState::Attack => {
                let c = ATTACK_COEF[self.ar as usize];
                if c >= 1.0 {
                    self.env = 1.0;
                    self.state = EnvState::Decay;
                } else {
                    self.env += (1.0 - self.env) * c;
                    if self.env >= 0.999 {
                        self.env = 1.0;
                        self.state = EnvState::Decay;
                    }
                }
            }
            EnvState::Decay => {
                let sustain_level = if self.sl == 15 {
                    0.0
                } else {
                    // sustain level: -3 dB per unit
                    (-(self.sl as f32) * 3.0 * 0.115_129_25_f32).exp()
                };
                let c = DECAY_COEF[self.dr as usize];
                self.env = sustain_level + (self.env - sustain_level) * (1.0 - c);
                if self.env <= sustain_level + 0.0005 {
                    self.env = sustain_level;
                    self.state = EnvState::Sustain;
                }
            }
            EnvState::Sustain => {
                if !self.egt {
                    let c = DECAY_COEF[self.rr as usize];
                    self.env *= 1.0 - c;
                    if self.env <= 0.0005 {
                        self.env = 0.0;
                        self.state = EnvState::Off;
                    }
                }
            }
            EnvState::Release => {
                let c = DECAY_COEF[self.rr as usize];
                self.env *= 1.0 - c;
                if self.env <= 0.0005 {
                    self.env = 0.0;
                    self.state = EnvState::Off;
                }
            }
        }
    }
}

#[derive(Clone, Default)]
struct Channel {
    f_num: u16,
    block: u8,
    key_on: bool,
    fb: u8,
    cnt: bool,
}

pub struct AdLib {
    registers: [u8; 256],
    current_reg: u8,
    operators: [Operator; NUM_OPERATORS],
    channels: [Channel; NUM_CHANNELS],

    waveform_select_enable: bool,
    rhythm_mode: bool,
    am_depth_high: bool,
    vib_depth_high: bool,

    // Rhythm-mode key-on bits (bits 0..4 of 0xBD: HH, CY, TOM, SD, BD)
    rhythm_bd: bool,
    rhythm_sd: bool,
    rhythm_tom: bool,
    rhythm_cy: bool,
    rhythm_hh: bool,

    // LFO phases in [0, 1)
    lfo_am_phase: f32,
    lfo_vib_phase: f32,

    // Pseudo-noise for rhythm percussion
    noise_state: u32,

    // Timer state (for AdLib detection)
    timer1_value: u8,
    timer2_value: u8,
    timer1_mask: bool,
    timer2_mask: bool,
    timer1_running: bool,
    timer2_running: bool,
    timer1_started: Option<Instant>,
    timer2_started: Option<Instant>,
    timer1_expired: bool,
    timer2_expired: bool,
}

impl AdLib {
    pub fn new() -> Self {
        Self {
            registers: [0; 256],
            current_reg: 0,
            operators: core::array::from_fn(|_| Operator::default()),
            channels: core::array::from_fn(|_| Channel::default()),
            waveform_select_enable: false,
            rhythm_mode: false,
            am_depth_high: false,
            vib_depth_high: false,
            rhythm_bd: false,
            rhythm_sd: false,
            rhythm_tom: false,
            rhythm_cy: false,
            rhythm_hh: false,
            lfo_am_phase: 0.0,
            lfo_vib_phase: 0.0,
            noise_state: 0x1234_5678,
            timer1_value: 0,
            timer2_value: 0,
            timer1_mask: false,
            timer2_mask: false,
            timer1_running: false,
            timer2_running: false,
            timer1_started: None,
            timer2_started: None,
            timer1_expired: false,
            timer2_expired: false,
        }
    }

    /// Writing to port 0x388 selects the target register.
    pub fn write_register_select(&mut self, value: u8) {
        self.current_reg = value;
    }

    /// Writing to port 0x389 stores the value in the selected register.
    pub fn write_register_data(&mut self, value: u8) {
        let reg = self.current_reg;
        self.apply_register(reg, value);
    }

    /// Status-byte read on port 0x388. Games poll this to detect AdLib.
    pub fn read_status(&mut self) -> u8 {
        self.update_timers();
        let mut status = 0u8;
        if self.timer1_expired || self.timer2_expired {
            status |= 0x80;
        }
        if self.timer1_expired {
            status |= 0x40;
        }
        if self.timer2_expired {
            status |= 0x20;
        }
        // Upper nibble bits set on real OPL2 silicon
        status | 0x06
    }

    fn update_timers(&mut self) {
        if self.timer1_running {
            if let Some(start) = self.timer1_started {
                let elapsed_us = start.elapsed().as_micros() as u64;
                let period_us = (256 - self.timer1_value as u64) * 80;
                if elapsed_us >= period_us && !self.timer1_mask {
                    self.timer1_expired = true;
                }
            }
        }
        if self.timer2_running {
            if let Some(start) = self.timer2_started {
                let elapsed_us = start.elapsed().as_micros() as u64;
                let period_us = (256 - self.timer2_value as u64) * 320;
                if elapsed_us >= period_us && !self.timer2_mask {
                    self.timer2_expired = true;
                }
            }
        }
    }

    fn apply_register(&mut self, reg: u8, value: u8) {
        self.registers[reg as usize] = value;
        match reg {
            0x01 => {
                self.waveform_select_enable = (value & 0x20) != 0;
            }
            0x02 => {
                self.timer1_value = value;
            }
            0x03 => {
                self.timer2_value = value;
            }
            0x04 => {
                if (value & 0x80) != 0 {
                    self.timer1_expired = false;
                    self.timer2_expired = false;
                } else {
                    self.timer1_mask = (value & 0x40) != 0;
                    self.timer2_mask = (value & 0x20) != 0;
                    let t1_start = (value & 0x01) != 0;
                    let t2_start = (value & 0x02) != 0;
                    if t1_start && !self.timer1_running {
                        self.timer1_started = Some(Instant::now());
                        self.timer1_expired = false;
                    }
                    if t2_start && !self.timer2_running {
                        self.timer2_started = Some(Instant::now());
                        self.timer2_expired = false;
                    }
                    self.timer1_running = t1_start;
                    self.timer2_running = t2_start;
                }
            }
            0x08 => {
                // CSM / Note Select — does not affect sound in this model.
            }
            0x20..=0x35 => {
                if let Some(idx) = op_offset_to_idx(reg - 0x20) {
                    let op = &mut self.operators[idx];
                    op.am = (value & 0x80) != 0;
                    op.vib = (value & 0x40) != 0;
                    op.egt = (value & 0x20) != 0;
                    op.ksr = (value & 0x10) != 0;
                    op.mult = value & 0x0F;
                }
            }
            0x40..=0x55 => {
                if let Some(idx) = op_offset_to_idx(reg - 0x40) {
                    let op = &mut self.operators[idx];
                    op.ksl = value >> 6;
                    op.tl = value & 0x3F;
                }
            }
            0x60..=0x75 => {
                if let Some(idx) = op_offset_to_idx(reg - 0x60) {
                    let op = &mut self.operators[idx];
                    op.ar = value >> 4;
                    op.dr = value & 0x0F;
                }
            }
            0x80..=0x95 => {
                if let Some(idx) = op_offset_to_idx(reg - 0x80) {
                    let op = &mut self.operators[idx];
                    op.sl = value >> 4;
                    op.rr = value & 0x0F;
                }
            }
            0xA0..=0xA8 => {
                let ch = (reg - 0xA0) as usize;
                self.channels[ch].f_num = (self.channels[ch].f_num & 0x300) | value as u16;
            }
            0xB0..=0xB8 => {
                let ch = (reg - 0xB0) as usize;
                let f_high = (value & 0x03) as u16;
                self.channels[ch].f_num = (self.channels[ch].f_num & 0x00FF) | (f_high << 8);
                self.channels[ch].block = (value >> 2) & 0x07;
                let new_key = (value & 0x20) != 0;
                let old_key = self.channels[ch].key_on;
                self.channels[ch].key_on = new_key;
                if new_key && !old_key {
                    let (m, c) = CH_OPS[ch];
                    self.operators[m].key_on();
                    self.operators[c].key_on();
                } else if !new_key && old_key {
                    let (m, c) = CH_OPS[ch];
                    self.operators[m].key_off();
                    self.operators[c].key_off();
                }
            }
            0xBD => {
                self.am_depth_high = (value & 0x80) != 0;
                self.vib_depth_high = (value & 0x40) != 0;
                let new_rhythm = (value & 0x20) != 0;
                let new_bd = (value & 0x10) != 0;
                let new_sd = (value & 0x08) != 0;
                let new_tom = (value & 0x04) != 0;
                let new_cy = (value & 0x02) != 0;
                let new_hh = (value & 0x01) != 0;
                // Bass Drum uses both ops of channel 6
                if new_rhythm {
                    self.update_rhythm_key(new_bd, self.rhythm_bd, 12, 15);
                    self.update_rhythm_key_single(new_hh, self.rhythm_hh, 13);
                    self.update_rhythm_key_single(new_sd, self.rhythm_sd, 16);
                    self.update_rhythm_key_single(new_tom, self.rhythm_tom, 14);
                    self.update_rhythm_key_single(new_cy, self.rhythm_cy, 17);
                }
                self.rhythm_mode = new_rhythm;
                self.rhythm_bd = new_bd;
                self.rhythm_sd = new_sd;
                self.rhythm_tom = new_tom;
                self.rhythm_cy = new_cy;
                self.rhythm_hh = new_hh;
            }
            0xC0..=0xC8 => {
                let ch = (reg - 0xC0) as usize;
                self.channels[ch].fb = (value >> 1) & 0x07;
                self.channels[ch].cnt = (value & 0x01) != 0;
            }
            0xE0..=0xF5 => {
                if let Some(idx) = op_offset_to_idx(reg - 0xE0) {
                    let op = &mut self.operators[idx];
                    op.ws = if self.waveform_select_enable {
                        value & 0x03
                    } else {
                        0
                    };
                }
            }
            _ => {}
        }
    }

    fn update_rhythm_key(&mut self, new_on: bool, old_on: bool, m: usize, c: usize) {
        if new_on && !old_on {
            self.operators[m].key_on();
            self.operators[c].key_on();
        } else if !new_on && old_on {
            self.operators[m].key_off();
            self.operators[c].key_off();
        }
    }

    fn update_rhythm_key_single(&mut self, new_on: bool, old_on: bool, op_idx: usize) {
        if new_on && !old_on {
            self.operators[op_idx].key_on();
        } else if !new_on && old_on {
            self.operators[op_idx].key_off();
        }
    }

    /// Advance pseudo-noise LFSR, return 0 or 1.
    fn next_noise_bit(&mut self) -> u32 {
        // 32-bit Galois LFSR (taps 32, 22, 2, 1)
        let lsb = self.noise_state & 1;
        self.noise_state >>= 1;
        if lsb != 0 {
            self.noise_state ^= 0x8020_0003;
        }
        lsb
    }

    /// Render one output sample at OUTPUT_RATE. Returns f32 in roughly [-9, 9]
    /// (sum of 9 channels, each in [-1, 1]). The caller scales to i16.
    pub fn render_sample(&mut self) -> f32 {
        // Advance LFOs. AM runs at 3.7 Hz, VIB at 6.1 Hz on real OPL2.
        self.lfo_am_phase = (self.lfo_am_phase + 3.7 / OUTPUT_RATE).fract();
        self.lfo_vib_phase = (self.lfo_vib_phase + 6.1 / OUTPUT_RATE).fract();
        let am_depth = if self.am_depth_high { 4.8 } else { 1.0 }; // dB
        let vib_depth = if self.vib_depth_high { 14.0 } else { 7.0 }; // cents
        let am_lfo_db = am_depth * 0.5 * (1.0 - (self.lfo_am_phase * std::f32::consts::TAU).cos());
        let vib_lfo_semitones = vib_depth
            * (self.lfo_vib_phase * std::f32::consts::TAU).sin()
            / 100.0;
        let vib_factor = 2f32.powf(vib_lfo_semitones / 12.0);
        let am_gain = 10f32.powf(-am_lfo_db / 20.0);

        let freq_scale = OPL_CLOCK / OUTPUT_RATE / (1u32 << 21) as f32;
        let mut total = 0.0f32;

        for ch_idx in 0..NUM_CHANNELS {
            let ch = self.channels[ch_idx].clone();

            // In rhythm mode, channels 6-8 are handled specially below.
            if self.rhythm_mode && ch_idx >= 6 {
                continue;
            }

            let base = (ch.f_num as u32) * (1u32 << ch.block) as u32;
            let (m_idx, c_idx) = CH_OPS[ch_idx];

            // Skip silent channels early to save work
            if !ch.key_on
                && self.operators[m_idx].state == EnvState::Off
                && self.operators[c_idx].state == EnvState::Off
            {
                continue;
            }

            // Modulator
            let (mod_out, _) = self.step_operator(m_idx, base, freq_scale, 0.0, ch.fb, vib_factor, am_gain);

            // Carrier, FM-modulated by modulator output (unless additive)
            let phase_mod = if ch.cnt { 0.0 } else { mod_out * 2.0 };
            let (car_out, _) =
                self.step_operator(c_idx, base, freq_scale, phase_mod, 0, vib_factor, am_gain);

            total += if ch.cnt { mod_out + car_out } else { car_out };
        }

        if self.rhythm_mode {
            total += self.render_rhythm(freq_scale, vib_factor, am_gain);
        }

        total
    }

    fn step_operator(
        &mut self,
        idx: usize,
        base_step: u32,
        freq_scale: f32,
        phase_mod: f32,
        feedback: u8,
        vib_factor: f32,
        am_gain: f32,
    ) -> (f32, f32) {
        let op = &mut self.operators[idx];

        let mut step = base_step as f32 * MULT_TABLE[op.mult as usize] as f32 * freq_scale;
        if op.vib {
            step *= vib_factor;
        }
        op.phase = (op.phase + step).fract();

        let fb_phase = if feedback > 0 {
            // OPL feedback: scale = 2^(fb-1) / 256 roughly; tuned here by ear.
            let avg = (op.prev_out + op.prev_out2) * 0.5;
            avg * (1u32 << feedback) as f32 * 0.5
        } else {
            0.0
        };

        let p = op.phase + phase_mod + fb_phase;
        let raw = wave_sample(p, op.ws);
        let mut gain = op.env * tl_to_gain(op.tl);
        if op.am {
            gain *= am_gain;
        }
        let out = raw * gain;

        op.prev_out2 = op.prev_out;
        op.prev_out = out;

        op.step_envelope();

        (out, raw)
    }

    fn render_rhythm(&mut self, freq_scale: f32, vib_factor: f32, am_gain: f32) -> f32 {
        // Channel 6 — Bass Drum: normal 2-op FM on operators 12 (mod) and 15 (car).
        let mut total = 0.0f32;
        {
            let ch = self.channels[6].clone();
            let base = (ch.f_num as u32) * (1u32 << ch.block) as u32;
            let (mod_out, _) = self.step_operator(12, base, freq_scale, 0.0, ch.fb, vib_factor, am_gain);
            let phase_mod = if ch.cnt { 0.0 } else { mod_out * 2.0 };
            let (car_out, _) = self.step_operator(15, base, freq_scale, phase_mod, 0, vib_factor, am_gain);
            total += if ch.cnt { mod_out + car_out } else { car_out };
        }

        // Channels 7 & 8 use dedicated percussion generators. Operators:
        //   13 = Hi-Hat, 16 = Snare Drum, 14 = Tom-Tom, 17 = Cymbal.
        // Hi-Hat and Cymbal mix tonal phase with a pseudo-noise bit to get
        // the characteristic metallic/crash timbre.
        let ch7 = self.channels[7].clone();
        let ch8 = self.channels[8].clone();
        let base7 = (ch7.f_num as u32) * (1u32 << ch7.block) as u32;
        let base8 = (ch8.f_num as u32) * (1u32 << ch8.block) as u32;

        // Tom-Tom — a simple sine.
        {
            let (out, _) = self.step_operator(14, base8, freq_scale, 0.0, 0, vib_factor, am_gain);
            total += out;
        }

        // Snare Drum — tonal from op 16 phase + noise flip.
        {
            let noise = self.next_noise_bit();
            let noise_phase = if noise != 0 { 0.5 } else { 0.0 };
            let (out, _) = self.step_operator(16, base7, freq_scale, noise_phase, 0, vib_factor, am_gain);
            total += out;
        }

        // Hi-Hat and Cymbal — share a noise-biased phase expression.
        {
            let noise = self.next_noise_bit();
            let noise_phase = if noise != 0 { 0.25 } else { 0.0 };
            let (out, _) = self.step_operator(13, base7, freq_scale, noise_phase, 0, vib_factor, am_gain);
            total += out;
        }
        {
            let noise = self.next_noise_bit();
            let noise_phase = if noise != 0 { 0.125 } else { 0.375 };
            let (out, _) = self.step_operator(17, base8, freq_scale, noise_phase, 0, vib_factor, am_gain);
            total += out;
        }

        total
    }
}

impl Default for AdLib {
    fn default() -> Self {
        Self::new()
    }
}
