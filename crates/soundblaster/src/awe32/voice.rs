//! One of the EMU8000's 32 voices: its registers, the oscillator reading
//! sample memory, the resonant low-pass filter, the volume and modulation
//! envelopes and the two LFOs, run a sample at a time at 44.1 kHz.

use super::Memory;
use super::tables::{self, ENV_FULL};

/// Stages of an envelope. There is no decay or release of its own: after
/// the hold, and on a release, the envelope ramps towards its sustain
/// target, down or up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub enum Stage {
    #[default]
    Stopped,
    Delay,
    Attack,
    Hold,
    Sustain,
    RampDown,
    RampUp,
}

rust_dos_savestate::state_enum!(Stage { Stage::Stopped, Stage::Delay, Stage::Attack, Stage::Hold, Stage::Sustain, Stage::RampDown, Stage::RampUp });

/// An envelope. `value_amp` is the attack's linear phase, `value_db` the
/// level after it: attenuation for the volume envelope (0 = full,
/// `ENV_FULL` = 96 dB down), the modulation amount for the other.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct Envelope {
    pub stage: Stage,
    delay_samples: i32,
    hold_samples: i32,
    attack_samples: i32,
    pub value_amp: i32,
    pub value_db: i32,
    sustain: i32,
    attack_step: i32,
    /// Units a sample, Q16, and the fraction carried between samples.
    ramp_step: i32,
    ramp_frac: i32,
}

impl Envelope {
    /// A new note: delay, or attack, or nothing when it never attacks.
    fn trigger(&mut self) {
        self.value_amp = 0;
        self.stage = if self.delay_samples != 0 {
            Stage::Delay
        } else if self.attack_step == 0 {
            Stage::Stopped
        } else {
            Stage::Attack
        };
    }

    fn set_attack_hold(&mut self, value: u16) {
        self.attack_samples = tables::get().attack_samples[(value & 0x7F) as usize];
        self.attack_step = if self.attack_samples == 0 { 0 } else { ENV_FULL / self.attack_samples };
        self.hold_samples = 4096 * (0x7F - ((value >> 8) & 0x7F) as i32);
    }

    fn ramp(&mut self) -> i32 {
        let s = self.ramp_step as i64 + self.ramp_frac as i64;
        self.ramp_frac = (s & 0xFFFF) as i32;
        (s >> 16) as i32
    }

    /// Start the ramp from where the envelope is towards its target.
    fn release(&mut self) {
        self.stage = if self.value_db >= self.sustain { Stage::RampDown } else { Stage::RampUp };
    }

    fn step_ramps(&mut self) {
        match self.stage {
            Stage::RampDown => {
                self.value_db -= self.ramp();
                if self.value_db <= self.sustain {
                    self.value_db = self.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::RampUp => {
                self.value_db += self.ramp();
                if self.value_db >= self.sustain {
                    self.value_db = self.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            _ => {}
        }
    }
}

/// Delay of an envelope or LFO register: none with bit 15, else 32 samples
/// a step below 8000h.
fn delay_samples(value: u16) -> i32 {
    if value & 0x8000 != 0 { 0 } else { (0x8000 - (value & 0x7FFF) as i32) << 5 }
}

/// A signed modulation amount of a register byte, scaled to +-4000h.
fn fixed(byte: u8) -> i16 {
    let v = byte as i8 as i32;
    (v * 0x4000 / if v < 0 { 0x80 } else { 0x7F }) as i16
}

/// The volume attack's amplitude, as measured on a card, at 21 points of
/// its phase.
const ATTACK_SHAPE: [f64; 21] = [
    0.000, 0.000, 0.005, 0.058, 0.116, 0.203, 0.280, 0.326, 0.372, 0.433, 0.493, 0.537, 0.580, 0.621, 0.662, 0.703,
    0.744, 0.807, 0.899, 0.947, 1.000,
];

/// The attack's amplitude at `phase` (`ENV_FULL` = its end), 0 to 65536.
fn attack_amplitude(phase: i32) -> usize {
    let pos = (phase as f64 / ENV_FULL as f64 * 20.0).clamp(0.0, 20.0);
    let idx = (pos as usize).min(19);
    let amp = ATTACK_SHAPE[idx] + (ATTACK_SHAPE[idx + 1] - ATTACK_SHAPE[idx]) * (pos - idx as f64);
    (amp * 65536.0) as usize
}

/// The modulation envelope's attack: strongly convex.
fn mod_attack_value(phase: i32) -> i32 {
    let x = (phase as f64 / ENV_FULL as f64).clamp(0.0, 1.0);
    ((1.0 - (1.0 - x).powf(13.5)) * ENV_FULL as f64) as i32
}

/// The card overshoots the level after the volume attack by a few percent
/// for 0.4 s: the excess every 50 ms.
const OVERSHOOT: [f64; 9] = [0.075, 0.081, 0.067, 0.053, 0.040, 0.029, 0.018, 0.007, 0.0];
const OVERSHOOT_STEP: i32 = 2205;

/// The gain `samples` after the attack, Q12.
fn overshoot_q12(samples: i32) -> i32 {
    if !(0..8 * OVERSHOOT_STEP).contains(&samples) {
        return 4096;
    }
    let i = (samples / OVERSHOOT_STEP) as usize;
    let f = (samples % OVERSHOOT_STEP) as f64 / OVERSHOOT_STEP as f64;
    (4096.0 * (1.0 + OVERSHOOT[i] + (OVERSHOOT[i + 1] - OVERSHOOT[i]) * f)) as i32
}

/// The filter's DC attenuation for each Q, about half the resonance.
const FILTER_ATTEN: [i32; 16] =
    [65536, 61869, 57079, 53269, 49145, 44820, 40877, 37690, 32845, 30653, 28607, 26392, 24630, 22463, 20487, 18470];

// The filter as measured on a card: cutoff 101.81 Hz at register value 0,
// 29.3843 cents a step, Q 0.931 at Q 0 and 1.5 dB more a step; towards
// the top of the range the resonance is lower (`CHAM_TOP_DB`).
const CHAM_BASE_HZ: f64 = 101.81;
const CHAM_CENTS: f64 = 29.3843;
const CHAM_Q0: f64 = 0.931;
const CHAM_DB_PER_Q: f64 = 1.5;
const CHAM_TOP_DB: [f64; 16] = [0.0, 0.5, 1.25, 1.75, 2.5, 3.25, 4.25, 5.0, 6.25, 7.25, 9.0, 10.75, 12.5, 13.75, 15.25, 16.75];
const CHAM_TOP_LO_HZ: f64 = 2653.0;
const CHAM_TOP_HI_HZ: f64 = 6870.0;

fn cham_q_db(q: usize, fc: f64) -> f64 {
    let t = ((fc / CHAM_TOP_LO_HZ).log2() / (CHAM_TOP_HI_HZ / CHAM_TOP_LO_HZ).log2()).clamp(0.0, 1.0);
    (1.0 - t) * q as f64 * CHAM_DB_PER_Q + t * CHAM_TOP_DB[q]
}

/// What a voice sends to the output and the effects this sample.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sums {
    pub left: i32,
    pub right: i32,
    pub reverb: i32,
    pub chorus: i32,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Voice {
    // The registers as programs see them.
    /// Current pitch (high word) and address fraction (low word).
    pub cpf: u32,
    /// Pitch target (high word), reverb send (bits 8-15), pan aux.
    pub ptrx: u32,
    /// Current volume (high word) and filter cutoff (low word).
    pub cvcf: u32,
    /// Volume target (high word) and filter target (low word).
    pub vtft: u32,
    /// Two registers the Windows drivers write, of unknown use.
    pub z2: u32,
    pub z1: u32,
    /// Pan (top byte) and loop start.
    pub psst: u32,
    /// Chorus send (top byte) and loop end.
    pub csl: u32,
    /// Filter Q (top nibble), DMA bits and current address.
    pub ccca: u32,
    pub envvol: u16,
    pub dcysusv: u16,
    pub envval: u16,
    pub dcysus: u16,
    pub atkhldv: u16,
    pub lfo1val: u16,
    pub lfo2val: u16,
    pub atkhld: u16,
    pub ip: u16,
    pub ifatn: u16,
    pub pefe: u16,
    pub fmmod: u16,
    pub tremfrq: u16,
    pub fm2frq2: u16,

    pub engine_on: bool,
    /// Play position, 32.32 words.
    pub addr: u64,
    pub loop_start: u32,
    pub loop_end: u32,
    initial_att: i32,
    initial_filter: i32,
    pub vol_env: Envelope,
    pub mod_env: Envelope,
    lfo1_speed: u64,
    lfo2_speed: u64,
    /// LFO phases, 16.32.
    lfo1_count: u64,
    lfo2_count: u64,
    lfo1_delay: i32,
    lfo2_delay: i32,
    vol_l: i32,
    vol_r: i32,
    fixed_modenv_filter: i16,
    fixed_modenv_pitch: i16,
    fixed_lfo1_filter: i16,
    fixed_lfo1_vibrato: i16,
    fixed_lfo1_tremolo: i16,
    fixed_lfo2_vibrato: i16,
    filter_q: usize,
    filter_att: i32,
    lp: f64,
    bp: f64,
    /// Samples since the volume attack ended, -1 when no overshoot runs.
    overshoot: i32,
    /// Filter modulation in octaves: the target the envelopes set and
    /// the value the filter uses, a sample behind.
    filter_oct_target: f64,
    filter_oct: f64,
    /// The current volume, sliding towards its target.
    slide: i32,
    /// The filter's coefficients for (cutoff, octaves, Q), kept while
    /// those don't change.
    #[serde(skip)]
    coeffs: Option<(u8, f64, usize, f64, f64)>,
}

impl Voice {
    /// The voice at power-on: all registers zero, silent.
    pub fn new() -> Self {
        Self { overshoot: -1, filter_att: FILTER_ATTEN[0], ..Default::default() }
    }

    pub fn cur_volume(&self) -> u16 {
        (self.cvcf >> 16) as u16
    }

    fn cur_pitch(&self) -> u16 {
        (self.cpf >> 16) as u16
    }

    fn set_cur_pitch(&mut self, pitch: u16) {
        self.cpf = (self.cpf & 0xFFFF) | (pitch as u32) << 16;
    }

    fn pitch_target(&self) -> u16 {
        (self.ptrx >> 16) as u16
    }

    pub fn set_pitch_target(&mut self, pitch: u16) {
        self.ptrx = (self.ptrx & 0xFFFF) | (pitch as u32) << 16;
    }

    fn set_volume_target(&mut self, volume: i32) {
        self.vtft = (self.vtft & 0xFFFF) | (volume as u32 & 0xFFFF) << 16;
    }

    fn set_filter_target(&mut self, cutoff: i32) {
        self.vtft = (self.vtft & 0xFFFF_0000) | (cutoff as u32 & 0xFFFF);
    }

    fn reverb_send(&self) -> i32 {
        ((self.ptrx >> 8) & 0xFF) as i32
    }

    fn chorus_send(&self) -> i32 {
        (self.csl >> 24) as i32
    }

    fn init_filter(&self) -> u8 {
        (self.ifatn >> 8) as u8
    }

    /// PSST: loop start, and the pan when the high word is written.
    pub fn write_psst(&mut self, high: bool) {
        self.loop_start = self.psst & 0xFF_FFFF;
        if high {
            let pan = (self.psst >> 24) as i32;
            self.vol_l = pan;
            self.vol_r = 255 - pan;
        }
    }

    pub fn write_csl(&mut self) {
        self.loop_end = self.csl & 0xFF_FFFF;
    }

    /// CCCA: the play position, and with the high word the filter's Q.
    pub fn write_ccca(&mut self, high: bool) {
        self.addr = (self.addr & 0xFFFF_FFFF) | ((self.ccca & 0xFF_FFFF) as u64) << 32;
        if high {
            self.filter_q = (self.ccca >> 28) as usize;
            self.filter_att = FILTER_ATTEN[self.filter_q];
        }
    }

    pub fn write_envvol(&mut self, value: u16) {
        self.envvol = value;
        self.vol_env.delay_samples = delay_samples(value);
    }

    pub fn write_envval(&mut self, value: u16) {
        self.envval = value;
        self.mod_env.delay_samples = delay_samples(value);
    }

    /// DCYSUSV: the volume envelope's sustain and ramp rate, the release,
    /// and the envelope engine, whose start is a new note. Returns whether
    /// the engine started.
    pub fn write_dcysusv(&mut self, value: u16) -> bool {
        let t = tables::get();
        self.dcysusv = value;
        let was_on = self.engine_on;
        self.engine_on = value & 0x80 == 0;
        let started = self.engine_on && !was_on;
        if started {
            // A new note starts with a clean filter and the LFOs at 0.
            self.lp = 0.0;
            self.bp = 0.0;
            self.lfo1_count = 0;
            self.lfo2_count = 0;
            if self.atkhldv & 0x8000 == 0 {
                self.vol_env.trigger();
                self.overshoot = -1;
            }
            if self.atkhld & 0x8000 == 0 {
                self.mod_env.trigger();
                self.mod_env.value_db = 0;
            }
        }
        let sustain = ((value >> 8) & 0x7F) as i32;
        let env = &mut self.vol_env;
        env.sustain = if sustain == 0 { ENV_FULL } else { (0x7F - sustain) << 14 };
        env.ramp_step = t.decay_step[(value & 0x7F) as usize];
        if value & 0x8000 != 0 {
            if matches!(env.stage, Stage::Delay | Stage::Attack | Stage::Hold) {
                env.value_db = (t.amplitude_to_db[attack_amplitude(env.value_amp)] << 5).min(ENV_FULL);
            }
            env.release();
        }
        started
    }

    pub fn write_dcysus(&mut self, value: u16) {
        self.dcysus = value;
        let sustain = ((value >> 8) & 0x7F) as i32;
        let env = &mut self.mod_env;
        env.sustain = if sustain == 0 { 0 } else { ENV_FULL - ((0x7F - sustain) << 14) };
        env.ramp_step = tables::get().decay_step[(value & 0x7F) as usize];
        if value & 0x8000 != 0 {
            if matches!(env.stage, Stage::Delay | Stage::Attack | Stage::Hold) {
                env.value_db = mod_attack_value(env.value_amp).min(ENV_FULL - 1);
            }
            env.release();
        }
    }

    /// ATKHLDV: the volume envelope's attack and hold; unless bit 15 is
    /// set, a new attack of a running voice, with its LFOs from 0.
    pub fn write_atkhldv(&mut self, value: u16) {
        self.atkhldv = value;
        self.vol_env.set_attack_hold(value);
        if value & 0x8000 == 0 && self.engine_on {
            self.lfo1_count = 0;
            self.lfo2_count = 0;
            self.vol_env.trigger();
            self.overshoot = -1;
        }
    }

    pub fn write_atkhld(&mut self, value: u16) {
        self.atkhld = value;
        self.mod_env.set_attack_hold(value);
        if value & 0x8000 == 0 && self.engine_on {
            self.mod_env.trigger();
            self.mod_env.value_db = 0;
        }
    }

    pub fn write_lfo1val(&mut self, value: u16) {
        self.lfo1val = value;
        self.lfo1_delay = delay_samples(value);
    }

    pub fn write_lfo2val(&mut self, value: u16) {
        self.lfo2val = value;
        self.lfo2_delay = delay_samples(value);
    }

    pub fn write_ip(&mut self, value: u16) {
        self.ip = value;
        self.set_pitch_target(tables::get().pitch[value as usize]);
    }

    /// IFATN: initial cutoff and attenuation, which set the targets at once.
    pub fn write_ifatn(&mut self, value: u16) {
        self.ifatn = value;
        let att = (value & 0xFF) as i32;
        let filter = (value >> 8) as i32;
        self.initial_att = (att << 21) / 0xFF;
        self.set_volume_target(tables::get().atten[att as usize]);
        self.initial_filter = (filter << 21) / 0xFF;
        self.set_filter_target(if filter == 0xFF { 0xFFFF } else { self.initial_filter >> 5 });
    }

    pub fn write_pefe(&mut self, value: u16) {
        self.pefe = value;
        self.fixed_modenv_filter = fixed(value as u8);
        self.fixed_modenv_pitch = fixed((value >> 8) as u8);
    }

    pub fn write_fmmod(&mut self, value: u16) {
        self.fmmod = value;
        self.fixed_lfo1_filter = fixed(value as u8);
        self.fixed_lfo1_vibrato = fixed((value >> 8) as u8);
    }

    pub fn write_tremfrq(&mut self, value: u16) {
        self.tremfrq = value;
        self.lfo1_speed = tables::get().lfo_speed[(value & 0xFF) as usize];
        self.fixed_lfo1_tremolo = fixed((value >> 8) as u8);
    }

    pub fn write_fm2frq2(&mut self, value: u16) {
        self.fm2frq2 = value;
        self.lfo2_speed = tables::get().lfo_speed[(value & 0xFF) as usize];
        self.fixed_lfo2_vibrato = fixed((value >> 8) as u8);
    }

    /// Stop the voice at once: engine off, no volume.
    pub fn silence(&mut self) {
        self.engine_on = false;
        self.dcysusv |= 0x80;
        self.vol_env.stage = Stage::Stopped;
        self.vtft &= 0xFFFF;
        self.cvcf &= 0xFFFF;
        self.slide = 0;
    }

    /// The sample at the play position: a cubic B-spline over four words,
    /// which the card's output matches. The position lies between the
    /// second and third word ("the actual audio location is the point 1
    /// word higher").
    fn oscillator(&self, mem: &Memory) -> i32 {
        let a = (self.addr >> 32) as u32;
        let g = ((self.addr >> 16) & 0xFFFF) as f32 / 65536.0;
        let (g2, g3, u) = (g * g, g * g * g, 1.0 - g);
        let p0 = mem.word(a) as f32;
        let p1 = mem.word(a.wrapping_add(1)) as f32;
        let p2 = mem.word(a.wrapping_add(2)) as f32;
        let p3 = mem.word(a.wrapping_add(3)) as f32;
        ((u * u * u * p0 + (3.0 * g3 - 6.0 * g2 + 4.0) * p1 + (-3.0 * g3 + 3.0 * g2 + 3.0 * g + 1.0) * p2 + g3 * p3) / 6.0)
            as i32
    }

    /// The resonant low-pass, a Chamberlin state-variable filter, which
    /// leaves the signal alone at Q 0 with the cutoff open.
    fn filter(&mut self, dat: i32) -> i32 {
        let cutoff = self.init_filter();
        if self.filter_q == 0 && cutoff == 0xFF && self.filter_oct >= 0.0 {
            return dat;
        }
        let (f, qd) = match self.coeffs {
            Some((c, oct, q, f, qd)) if c == cutoff && oct == self.filter_oct && q == self.filter_q => (f, qd),
            _ => {
                let fc = (CHAM_BASE_HZ * (cutoff as f64 * CHAM_CENTS / 1200.0 + self.filter_oct).exp2())
                    .clamp(CHAM_BASE_HZ, 44100.0 / 6.0);
                let f = 2.0 * (std::f64::consts::PI * fc / 44100.0).sin();
                let qd = 1.0 / (CHAM_Q0 * 10f64.powf(cham_q_db(self.filter_q, fc) / 20.0));
                self.coeffs = Some((cutoff, self.filter_oct, self.filter_q, f, qd));
                (f, qd)
            }
        };
        let input = dat as f64 * self.filter_att as f64 / 65536.0;
        self.lp += f * self.bp;
        let hp = input - self.lp - qd * self.bp;
        self.bp += f * hp;
        self.lp.clamp(-32768.0, 32767.0) as i32
    }

    /// Play one sample into `sums`, then run the envelopes and LFOs and
    /// move on. `unmuted` is the chip's audio enable (HWCF3 bit 2).
    pub fn step(&mut self, mem: &Memory, unmuted: bool, sums: &mut Sums) {
        let volume = self.cur_volume() as i32;
        if volume != 0 {
            let dat = self.oscillator(mem);
            let mut dat = self.filter(dat);
            // DMA channels stream sample memory and are not heard.
            if unmuted && self.ccca & 0x0400_0000 == 0 {
                dat = (dat * volume) >> 16;
                if self.overshoot >= 0 {
                    dat = ((dat as i64 * overshoot_q12(self.overshoot) as i64) >> 12) as i32;
                    self.overshoot += 1;
                    if self.overshoot >= 8 * OVERSHOOT_STEP {
                        self.overshoot = -1;
                    }
                }
                sums.left += (dat * self.vol_l) >> 8;
                sums.right += (dat * self.vol_r) >> 8;
                sums.reverb += (dat * self.reverb_send()) >> 8;
                sums.chorus += (dat * self.chorus_send()) >> 8;
            }
        }

        if self.engine_on {
            self.run_envelopes();
        }

        self.addr = self.addr.wrapping_add((self.cur_pitch() as u64) << 18);
        if self.addr >= (self.loop_end as u64) << 32 {
            let int = ((self.addr >> 32) as u32).wrapping_sub(self.loop_end.wrapping_sub(self.loop_start)) & 0xFF_FFFF;
            self.addr = (self.addr & 0xFFFF_FFFF) | (int as u64) << 32;
        }

        self.set_cur_pitch(self.pitch_target());
        let target = (self.vtft >> 16) as i32;
        let d = target - self.slide;
        if d > 0 {
            self.slide += (d + 63) >> 6;
        } else if d < 0 {
            self.slide += d >> 6;
        }
        self.cvcf = (self.slide as u32 & 0xFFFF) << 16 | (self.vtft & 0xFFFF);
        self.filter_oct = self.filter_oct_target;
        self.ccca = (self.ccca & 0xFF00_0000) | ((self.addr >> 32) as u32 & 0xFF_FFFF);
        self.cpf = (self.cpf & 0xFFFF_0000) | ((self.addr >> 16) & 0xFFFF) as u32;
    }

    /// The envelopes and LFOs, setting the volume, filter and pitch
    /// targets.
    fn run_envelopes(&mut self) {
        let t = tables::get();
        let mut attenuation = self.initial_att;
        let mut filtercut = self.initial_filter;
        let mut pitch = self.ip as i32;

        let env = &mut self.vol_env;
        match env.stage {
            Stage::Delay => {
                env.delay_samples -= 1;
                if env.delay_samples <= 0 {
                    env.stage = Stage::Attack;
                    env.delay_samples = 0;
                }
                attenuation = 0x1F_FFFF;
            }
            Stage::Attack => {
                env.value_amp += env.attack_step;
                if env.value_amp >= ENV_FULL {
                    env.value_amp = ENV_FULL;
                    self.overshoot = 0;
                    env.value_db = 0;
                    env.stage = if env.hold_samples != 0 { Stage::Hold } else { Stage::RampUp };
                }
                attenuation += t.amplitude_to_db[attack_amplitude(env.value_amp)] << 5;
            }
            Stage::Hold => {
                env.hold_samples -= 1;
                if env.hold_samples <= 0 {
                    env.stage = Stage::RampUp;
                }
                attenuation += env.value_db;
            }
            Stage::RampDown | Stage::RampUp => {
                env.step_ramps();
                attenuation += env.value_db;
            }
            Stage::Sustain => attenuation += env.value_db,
            Stage::Stopped => attenuation = 0x1F_FFFF,
        }

        let env = &mut self.mod_env;
        match env.stage {
            Stage::Delay => {
                env.delay_samples -= 1;
                if env.delay_samples <= 0 {
                    env.stage = Stage::Attack;
                    env.delay_samples = 0;
                }
            }
            Stage::Attack => {
                env.value_amp += env.attack_step;
                env.value_db = mod_attack_value(env.value_amp);
                if env.value_amp >= ENV_FULL {
                    env.value_amp = ENV_FULL;
                    env.value_db = ENV_FULL;
                    env.stage = if env.hold_samples != 0 { Stage::Hold } else { Stage::RampDown };
                }
            }
            Stage::Hold => {
                env.hold_samples -= 1;
                if env.hold_samples <= 0 {
                    env.stage = Stage::RampDown;
                }
            }
            Stage::RampDown | Stage::RampUp => env.step_ramps(),
            Stage::Sustain | Stage::Stopped => {}
        }
        let mod_value = self.mod_env.value_db;

        const PHASE: u64 = 0xFFFF_FFFF_FFFF;
        if self.lfo1_delay != 0 {
            self.lfo1_delay -= 1;
        } else {
            self.lfo1_count = self.lfo1_count.wrapping_add(self.lfo1_speed) & PHASE;
        }
        if self.lfo2_delay != 0 {
            self.lfo2_delay -= 1;
        } else {
            self.lfo2_count = self.lfo2_count.wrapping_add(self.lfo2_speed) & PHASE;
        }
        let lfo1 = tables::lfo((self.lfo1_count >> 32) as u32);
        let lfo2 = tables::lfo((self.lfo2_count >> 32) as u32);

        pitch += ((mod_value >> 9) * self.fixed_modenv_pitch as i32) >> 14;
        pitch += (lfo1 * self.fixed_lfo1_vibrato as i32) >> 17;
        pitch += (lfo2 * self.fixed_lfo2_vibrato as i32) >> 17;
        filtercut += ((mod_value >> 9) * self.fixed_modenv_filter as i32) >> 5;
        filtercut += (lfo1 * self.fixed_lfo1_filter as i32) >> 9;
        // Tremolo only ever attenuates.
        let tremolo = (lfo1 * self.fixed_lfo1_tremolo as i32) >> 11;
        if tremolo < 0 {
            attenuation -= tremolo;
        }

        self.filter_oct_target = mod_value as f64 / ENV_FULL as f64 * (self.pefe as u8 as i8) as f64 / 127.0 * 6.0
            + lfo1 as f64 / 32768.0 * (self.fmmod as u8 as i8) as f64 / 127.0 * 3.0;

        let pitch = pitch.clamp(0, 0xFFFF);
        let attenuation = attenuation.clamp(0, 0x1F_FFFF);
        let filtercut = filtercut.clamp(0, 0x1F_FFFF);
        self.set_volume_target(t.db_to_volume[(attenuation >> 5) as usize]);
        self.set_filter_target(filtercut >> 5);
        self.set_pitch_target(t.pitch[pitch as usize]);
    }
}

rust_dos_savestate::state_fields!(Envelope {
    stage, delay_samples, hold_samples, attack_samples, value_amp, value_db, sustain, attack_step, ramp_step, ramp_frac,
});
rust_dos_savestate::state_fields!(Voice {
    cpf, ptrx, cvcf, vtft, z2, z1, psst, csl, ccca, envvol, dcysusv, envval, dcysus, atkhldv, lfo1val, lfo2val, atkhld,
    ip, ifatn, pefe, fmmod, tremfrq, fm2frq2, engine_on, addr, loop_start, loop_end, initial_att, initial_filter,
    vol_env, mod_env, lfo1_speed, lfo2_speed, lfo1_count, lfo2_count, lfo1_delay, lfo2_delay, vol_l, vol_r,
    fixed_modenv_filter, fixed_modenv_pitch, fixed_lfo1_filter, fixed_lfo1_vibrato, fixed_lfo1_tremolo,
    fixed_lfo2_vibrato, filter_q, filter_att, lp, bp, overshoot, filter_oct_target, filter_oct, slide,
} skip {
    // Worked out again from the registers.
    coeffs,
});
