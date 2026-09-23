//! Sound Blaster digital audio: the DSP and the mixer of the SB 2.0 (DSP
//! 2.01), SB Pro 2 (DSP 3.02, stereo, CT1345 mixer) and SB16 (DSP 4.05,
//! 16-bit transfers, CT1745 mixer). The FM chip is `opl.rs`.
//!
//! The DSP runs on emulated time: while a DMA transfer is active it pulls
//! samples from the DMA controller at its sample rate, and raises its IRQ
//! at the end of each block at the emulated time a real card would. The
//! bus advances it (`advance`) before every port access and when its next
//! event (`next_event`) is due. The samples go to `out`, which the audio
//! mixer drains.

use std::collections::VecDeque;

use crate::dma::Dma;
use crate::timer::PIT_HZ;

/// Which card is emulated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SbModel {
    Sb2,
    SbPro2,
    Sb16,
}

impl SbModel {
    /// DSP version: major, minor.
    fn dsp_version(self) -> (u8, u8) {
        match self {
            SbModel::Sb2 => (2, 1),
            SbModel::SbPro2 => (3, 2),
            SbModel::Sb16 => (4, 5),
        }
    }

    /// The T value of the BLASTER variable.
    pub fn blaster_type(self) -> u8 {
        match self {
            SbModel::Sb2 => 3,
            SbModel::SbPro2 => 4,
            SbModel::Sb16 => 6,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "sb2" | "sb20" => Some(SbModel::Sb2),
            "sbpro2" | "sbpro" => Some(SbModel::SbPro2),
            "sb16" => Some(SbModel::Sb16),
            _ => None,
        }
    }
}

/// Resources of the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SbConfig {
    pub model: SbModel,
    pub base: u16,
    pub irq: u8,
    /// 8-bit DMA channel.
    pub dma8: u8,
    /// 16-bit DMA channel (SB16).
    pub dma16: u8,
}

impl Default for SbConfig {
    /// An SB16 at 220h, IRQ 7, DMA 1 and 5. IRQ 7 keeps the card's
    /// interrupt off vector 0Dh, which DOS extenders that leave the PIC
    /// at its BIOS setting share with #GP.
    fn default() -> Self {
        Self { model: SbModel::Sb16, base: 0x220, irq: 7, dma8: 1, dma16: 5 }
    }
}

impl SbConfig {
    /// The BLASTER environment variable describing the card.
    pub fn blaster(&self) -> String {
        let mut s = format!("A{:X} I{} D{}", self.base, self.irq, self.dma8);
        if self.model == SbModel::Sb16 {
            s.push_str(&format!(" H{} P330", self.dma16));
        }
        s.push_str(&format!(" T{}", self.model.blaster_type()));
        s
    }
}

/// A DMA transfer the DSP runs.
#[derive(Clone, Copy, Debug)]
struct Transfer {
    bits16: bool,
    stereo: bool,
    signed: bool,
    auto_init: bool,
    /// Recording: DMA advances, nothing is played.
    input: bool,
    /// DMA units (bytes, or words for 16-bit) per block.
    block: u32,
    /// Units left in the current block.
    remaining: u32,
    /// DMA units per second.
    rate: u32,
    paused: bool,
    /// Stop at the end of the current block (exit auto-init).
    last_block: bool,
}

/// Mixer registers of the SB16 (CT1745) used for volumes.
const MASTER_L: u8 = 0x30;
const MASTER_R: u8 = 0x31;
const VOICE_L: u8 = 0x32;
const VOICE_R: u8 = 0x33;
const FM_L: u8 = 0x34;
const FM_R: u8 = 0x35;
const CD_L: u8 = 0x36;
const CD_R: u8 = 0x37;
/// The SB Pro's CD volume, a nibble per side.
const PRO_CD: u8 = 0x28;

pub struct SoundBlaster {
    pub config: SbConfig,
    reset_stage: u8,
    in_command: Option<u8>,
    params: Vec<u8>,
    params_needed: usize,
    read_buf: VecDeque<u8>,
    test_reg: u8,
    pub speaker_on: bool,
    /// Units per second from the last time constant (40h).
    tc_rate: u32,
    /// Sample frames per second from 41h/42h (SB16).
    sb16_rate: Option<u32>,
    /// Block size from 48h, in units.
    block_size: u32,
    transfer: Option<Transfer>,
    /// Command 80h: units of silence left before its IRQ.
    silence: Option<u32>,
    /// IRQ requests of 8-bit and 16-bit transfers.
    pub irq8: bool,
    pub irq16: bool,
    /// Emulated time (PIT ticks) the DSP has run up to, and the remainder
    /// of units not yet due (in units * PIT ticks).
    last_ticks: u64,
    frac: u64,
    /// The level held by direct DAC output (command 10h), signed.
    pub dac: i16,
    /// Stereo frames produced and the rate they were produced at.
    pub out: VecDeque<(i16, i16)>,
    pub out_rate: u32,
    /// A left sample waiting for its right one.
    pending_left: Option<i16>,
    mixer_index: u8,
    mixer: [u8; 256],
}

/// Frames `out` keeps at most (about a second), for when nobody drains it.
const OUT_MAX: usize = 48_000;

impl SoundBlaster {
    pub fn new(config: SbConfig) -> Self {
        let mut sb = Self {
            config,
            reset_stage: 0,
            in_command: None,
            params: Vec::with_capacity(4),
            params_needed: 0,
            read_buf: VecDeque::new(),
            test_reg: 0,
            speaker_on: config.model == SbModel::Sb16,
            tc_rate: 11025,
            sb16_rate: None,
            block_size: 0x800,
            transfer: None,
            silence: None,
            irq8: false,
            irq16: false,
            last_ticks: 0,
            frac: 0,
            dac: 0,
            out: VecDeque::new(),
            out_rate: 11025,
            pending_left: None,
            mixer_index: 0,
            mixer: [0; 256],
        };
        sb.reset_mixer();
        sb
    }

    /// Whether the card's interrupt line is up.
    pub fn irq_pending(&self) -> bool {
        self.irq8 || self.irq16
    }

    fn reset_dsp(&mut self) {
        self.in_command = None;
        self.params.clear();
        self.read_buf.clear();
        self.transfer = None;
        self.silence = None;
        self.irq8 = false;
        self.irq16 = false;
        self.speaker_on = self.config.model == SbModel::Sb16;
        self.sb16_rate = None;
        self.pending_left = None;
        self.dac = 0;
    }

    fn reset_mixer(&mut self) {
        self.mixer = [0; 256];
        for reg in [MASTER_L, MASTER_R, VOICE_L, VOICE_R, FM_L, FM_R, CD_L, CD_R] {
            self.mixer[reg as usize] = 0xC0;
        }
        // SB Pro view: master, voice, FM and CD at the same levels.
        self.mixer[0x22] = 0xCC;
        self.mixer[0x04] = 0xCC;
        self.mixer[0x26] = 0xCC;
        self.mixer[PRO_CD as usize] = 0xCC;
    }

    /// Volume of the digital audio and of FM, as (left, right) gains
    /// including the master volume.
    pub fn volumes(&self) -> ((f32, f32), (f32, f32)) {
        if self.config.model == SbModel::Sb2 {
            return ((1.0, 1.0), (1.0, 1.0));
        }
        let level = |reg: u8| (self.mixer[reg as usize] >> 3) as f32 / 31.0;
        let (ml, mr) = (level(MASTER_L), level(MASTER_R));
        (
            (level(VOICE_L) * ml, level(VOICE_R) * mr),
            (level(FM_L) * ml, level(FM_R) * mr),
        )
    }

    /// Volume of CD audio as (left, right) gains including the master
    /// volume.
    pub fn cd_volume(&self) -> (f32, f32) {
        if self.config.model == SbModel::Sb2 {
            return (1.0, 1.0);
        }
        let level = |reg: u8| (self.mixer[reg as usize] >> 3) as f32 / 31.0;
        (level(CD_L) * level(MASTER_L), level(CD_R) * level(MASTER_R))
    }

    /// DMA channel of a transfer.
    fn channel(&self, bits16: bool) -> usize {
        if bits16 && self.config.model == SbModel::Sb16 { self.config.dma16 as usize } else { self.config.dma8 as usize }
    }

    /// Run the DSP up to emulated time `now` (PIT ticks): play the DMA
    /// data that has come due, and raise the block-end IRQs.
    pub fn advance(&mut self, now: u64, dma: &mut Dma, ram: &[u8]) {
        let elapsed = now.saturating_sub(self.last_ticks);
        self.last_ticks = now;
        let Some(t) = self.transfer.filter(|t| !t.paused) else {
            if let Some(left) = self.silence {
                // Command 80h: count silent units at the time-constant rate.
                let units = (elapsed as u128 * self.tc_rate as u128 / PIT_HZ as u128) as u32;
                if units >= left {
                    self.silence = None;
                    self.irq8 = true;
                } else {
                    self.silence = Some(left - units);
                }
            }
            self.frac = 0;
            return;
        };
        let total = elapsed as u128 * t.rate as u128 + self.frac as u128;
        let mut units = (total / PIT_HZ as u128) as u64;
        self.frac = (total % PIT_HZ as u128) as u64;
        let ch = self.channel(t.bits16);
        let unit_bytes = if t.bits16 { 2 } else { 1 };
        let mut buf = [0u8; 4096];
        while units > 0 {
            let Some(mut t) = self.transfer else { break };
            let k = (units.min(t.remaining as u64) as usize).min(buf.len() / unit_bytes);
            if t.input {
                dma.transfer_skip(ch, k);
            } else {
                let bytes = k * unit_bytes;
                let (moved, _) = dma.transfer_read(ch, ram, &mut buf[..bytes]);
                // A channel masked mid-block plays silence for the rest.
                buf[moved..bytes].fill(if t.bits16 || t.signed { 0 } else { 0x80 });
                self.play(&t, &buf[..bytes]);
            }
            units -= k as u64;
            t.remaining -= k as u32;
            if t.remaining == 0 {
                if t.bits16 { self.irq16 = true } else { self.irq8 = true }
                if t.auto_init && !t.last_block {
                    t.remaining = t.block;
                } else {
                    self.transfer = None;
                    self.frac = 0;
                    break;
                }
            }
            self.transfer = Some(t);
        }
    }

    /// Queue samples of a transfer for output.
    fn play(&mut self, t: &Transfer, data: &[u8]) {
        let sample = |i: usize| -> i16 {
            if t.bits16 {
                let v = u16::from_le_bytes([data[i], data[i + 1]]);
                if t.signed { v as i16 } else { (v ^ 0x8000) as i16 }
            } else {
                let v = data[i];
                let s = if t.signed { v as i8 as i16 } else { v as i16 - 128 };
                s << 8
            }
        };
        let step = if t.bits16 { 2 } else { 1 };
        let mut i = 0;
        while i + step <= data.len() {
            let s = sample(i);
            i += step;
            if t.stereo {
                match self.pending_left.take() {
                    None => self.pending_left = Some(s),
                    Some(left) => self.out.push_back((left, s)),
                }
            } else {
                self.out.push_back((s, s));
            }
        }
        self.out_rate = if t.stereo { t.rate / 2 } else { t.rate };
        while self.out.len() > OUT_MAX {
            self.out.pop_front();
        }
    }

    /// When the DSP next needs attention: the end of the current block
    /// (or of a silence period), in PIT ticks.
    pub fn next_event(&self) -> Option<u64> {
        if let Some(t) = self.transfer.filter(|t| !t.paused && t.rate > 0) {
            let need = (t.remaining as u128 * PIT_HZ as u128).saturating_sub(self.frac as u128);
            return Some(self.last_ticks + need.div_ceil(t.rate as u128) as u64);
        }
        self.silence.map(|left| {
            self.last_ticks + (left as u128 * PIT_HZ as u128).div_ceil(self.tc_rate.max(1) as u128) as u64
        })
    }

    /// Port write at `offset` from the base (6 reset, 0Ch command, 4/5
    /// mixer).
    pub fn write(&mut self, offset: u16, value: u8, log: &mut Vec<String>) {
        match offset {
            0x4 => self.mixer_index = value,
            0x5 => self.mixer_write(value),
            0x6 => {
                if value & 1 != 0 {
                    self.reset_stage = 1;
                } else if self.reset_stage == 1 {
                    self.reset_stage = 0;
                    self.reset_dsp();
                    self.read_buf.push_back(0xAA);
                }
            }
            0xC => self.write_command(value, log),
            _ => {}
        }
    }

    /// Port read at `offset` from the base.
    pub fn read(&mut self, offset: u16) -> u8 {
        match offset {
            0x5 => self.mixer_read(),
            0xA => self.read_buf.pop_front().unwrap_or(0xFF),
            // Write status: bit 7 clear, ready for a byte.
            0xC => 0x7F,
            // Read status: bit 7 when data waits. Acknowledges the 8-bit
            // (and on older cards the only) interrupt.
            0xE => {
                self.irq8 = false;
                if self.read_buf.is_empty() { 0x7F } else { 0xFF }
            }
            // SB16: acknowledges the 16-bit interrupt.
            0xF if self.config.model == SbModel::Sb16 => {
                self.irq16 = false;
                0xFF
            }
            _ => 0xFF,
        }
    }

    fn mixer_write(&mut self, value: u8) {
        let reg = self.mixer_index;
        if self.config.model == SbModel::Sb2 {
            return;
        }
        match reg {
            0x00 => self.reset_mixer(),
            // SB Pro registers: a nibble per side, mirrored into the SB16's.
            0x04 | 0x22 | 0x26 | PRO_CD => {
                self.mixer[reg as usize] = value;
                let (l, r) = match reg {
                    0x04 => (VOICE_L, VOICE_R),
                    0x22 => (MASTER_L, MASTER_R),
                    0x26 => (FM_L, FM_R),
                    _ => (CD_L, CD_R),
                };
                self.mixer[l as usize] = (value & 0xF0) | 0x08;
                self.mixer[r as usize] = (value << 4) | 0x08;
            }
            // SB16 interrupt and DMA setup: read-only here; the card's
            // resources come from the configuration.
            0x80..=0x82 => {}
            _ => self.mixer[reg as usize] = value,
        }
    }

    fn mixer_read(&self) -> u8 {
        let reg = self.mixer_index;
        match (self.config.model, reg) {
            (SbModel::Sb2, _) => 0xFF,
            (SbModel::Sb16, 0x80) => match self.config.irq {
                2 | 9 => 1,
                5 => 2,
                7 => 4,
                10 => 8,
                _ => 0,
            },
            (SbModel::Sb16, 0x81) => {
                let low = match self.config.dma8 {
                    0 => 1,
                    1 => 2,
                    3 => 8,
                    _ => 0,
                };
                let high = match self.config.dma16 {
                    5 => 0x20,
                    6 => 0x40,
                    7 => 0x80,
                    _ => 0,
                };
                low | high
            }
            (SbModel::Sb16, 0x82) => (self.irq8 as u8) | (self.irq16 as u8) << 1 | 0x20,
            _ => self.mixer[reg as usize],
        }
    }

    /// SB Pro stereo output (mixer register 0Eh bit 1).
    fn sbpro_stereo(&self) -> bool {
        self.config.model != SbModel::Sb2 && self.mixer[0x0E] & 0x02 != 0
    }

    fn write_command(&mut self, value: u8, log: &mut Vec<String>) {
        if let Some(cmd) = self.in_command {
            self.params.push(value);
            if self.params.len() >= self.params_needed {
                self.in_command = None;
                self.execute(cmd, log);
                self.params.clear();
            }
            return;
        }
        let sb16 = self.config.model == SbModel::Sb16;
        let needs = match value {
            0x10 | 0x38 | 0x40 | 0xE0 | 0xE4 => 1,
            0x14 | 0x16 | 0x17 | 0x24 | 0x48 | 0x74..=0x77 | 0x80 => 2,
            0x41 | 0x42 if sb16 => 2,
            0xB0..=0xCF if sb16 => 3,
            _ => 0,
        };
        if needs == 0 {
            self.execute(value, log);
        } else {
            self.in_command = Some(value);
            self.params_needed = needs;
        }
    }

    fn param16(&self, i: usize) -> u32 {
        u16::from_le_bytes([self.params[i], self.params[i + 1]]) as u32
    }

    /// Start a transfer, 8-bit with the time-constant rate unless given.
    fn start(&mut self, bits16: bool, stereo: bool, signed: bool, auto_init: bool, input: bool, units: u32) {
        let channels = if stereo { 2 } else { 1 };
        let rate = match self.sb16_rate {
            Some(r) if self.config.model == SbModel::Sb16 => r * channels,
            _ => self.tc_rate,
        };
        self.pending_left = None;
        self.frac = 0;
        self.transfer = Some(Transfer {
            bits16,
            stereo,
            signed,
            auto_init,
            input,
            block: units.max(1),
            remaining: units.max(1),
            rate: rate.max(1),
            paused: false,
            last_block: false,
        });
    }

    fn execute(&mut self, cmd: u8, log: &mut Vec<String>) {
        let stereo8 = self.sbpro_stereo();
        match cmd {
            // Direct DAC: one unsigned 8-bit sample.
            0x10 => self.dac = (self.params[0] as i16 - 128) << 8,
            // 8-bit single-cycle output; 16h/17h are 2-bit ADPCM, played
            // as plain 8-bit data.
            0x14 | 0x16 | 0x17 => {
                let len = self.param16(0) + 1;
                self.start(false, stereo8, false, false, false, len);
            }
            // 8-bit auto-init output (1Fh: ADPCM), and high-speed modes.
            0x1C | 0x1F | 0x90 => self.start(false, stereo8, false, true, false, self.block_size),
            0x91 => self.start(false, stereo8, false, false, false, self.block_size),
            // Direct ADC: silence.
            0x20 => self.read_buf.push_back(0x80),
            // 8-bit input.
            0x24 => {
                let len = self.param16(0) + 1;
                self.start(false, false, false, false, true, len);
            }
            0x2C | 0x98 => self.start(false, false, false, true, true, self.block_size),
            0x99 => self.start(false, false, false, false, true, self.block_size),
            // MIDI output byte (the SB's own UART): nothing attached.
            0x38 => {}
            0x40 => {
                let tc = self.params[0] as u32;
                self.tc_rate = 1_000_000 / (256 - tc).max(1);
                self.sb16_rate = None;
            }
            0x41 | 0x42 => self.sb16_rate = Some(u16::from_be_bytes([self.params[0], self.params[1]]) as u32),
            0x48 => self.block_size = self.param16(0) + 1,
            // ADPCM single-cycle output, as 8-bit.
            0x74..=0x77 => {
                let len = self.param16(0) + 1;
                self.start(false, false, false, false, false, len);
            }
            0x7D | 0x7F => self.start(false, false, false, true, false, self.block_size),
            // Silence for n+1 samples, then an IRQ.
            0x80 => self.silence = Some(self.param16(0) + 1),
            // SB Pro input mode (mono / stereo): nothing to record.
            0xA0 | 0xA8 => {}
            // SB16: B0h-BFh 16-bit and C0h-CFh 8-bit transfers. Bit 3 input,
            // bit 2 auto-init; the mode byte has bit 4 signed, bit 5 stereo.
            0xB0..=0xCF => {
                let mode = self.params[0];
                let len = u16::from_le_bytes([self.params[1], self.params[2]]) as u32 + 1;
                self.start(cmd < 0xC0, mode & 0x20 != 0, mode & 0x10 != 0, cmd & 0x04 != 0, cmd & 0x08 != 0, len);
            }
            // Pause and continue the 8-bit (D0h/D4h) or 16-bit (D5h/D6h)
            // transfer.
            0xD0 | 0xD5 => {
                if let Some(t) = &mut self.transfer {
                    t.paused = true;
                }
            }
            0xD4 | 0xD6 => {
                if let Some(t) = &mut self.transfer {
                    t.paused = false;
                }
            }
            0xD1 => self.speaker_on = true,
            0xD3 => self.speaker_on = false,
            0xD8 => self.read_buf.push_back(if self.speaker_on { 0xFF } else { 0x00 }),
            // Exit auto-init after the current block.
            0xD9 | 0xDA => {
                if let Some(t) = &mut self.transfer {
                    t.last_block = true;
                }
            }
            0xE0 => self.read_buf.push_back(!self.params[0]),
            0xE1 => {
                let (major, minor) = self.config.model.dsp_version();
                self.read_buf.push_back(major);
                self.read_buf.push_back(minor);
            }
            0xE3 => self.read_buf.extend(b"COPYRIGHT (C) CREATIVE TECHNOLOGY LTD, 1992.\0"),
            0xE4 => self.test_reg = self.params[0],
            0xE8 => self.read_buf.push_back(self.test_reg),
            // Raise the 8-bit (F2h) or 16-bit (F3h) interrupt.
            0xF2 => self.irq8 = true,
            0xF3 if self.config.model == SbModel::Sb16 => self.irq16 = true,
            0xF8 => self.read_buf.push_back(0),
            _ => log.push(format!("[SB] Unhandled DSP command {:02X}h", cmd)),
        }
    }
}
