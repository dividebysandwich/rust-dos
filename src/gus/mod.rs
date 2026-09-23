//! The Gravis Ultrasound: the GF1 chip's 32 wavetable voices playing
//! samples from 1 MB of on-board DRAM, the DMA that uploads them, the two
//! AdLib-style timers and the interrupts, at the ports 2X0h-2XFh and
//! 3X0h-3X7h (X = 4 at the usual base 240h). A classic card: no GUS MAX
//! codec.
//!
//! Like the Sound Blaster, the card runs on emulated time. The bus advances
//! it (`advance`) before every port access and when its next event
//! (`next_event`) is due: the voices render up to the present at the GF1's
//! playback rate, which depends on the number of active voices, DMA moves
//! the bytes that have come due, and the timers fire. The output waits in
//! `out` for the mixer, which resamples it (`pop_frame`).
//!
//! The card starts in the state ULTRINIT leaves it in, with its IRQ and
//! DMA latches set from the configuration; drivers that program the
//! latches themselves get what they ask for.
//!
//! `patch` and `synth` play Ultrasound patches (`.PAT`) as a General MIDI
//! synthesizer for the MPU-401, sharing the GF1's volume scale. `builtin`
//! holds the Gravis patch set, for the drive programs find it on.

pub mod builtin;
pub mod patch;
pub mod synth;
pub mod tables;
pub mod voice;

use std::collections::VecDeque;
use std::ops::Range;

use crate::dma::Dma;
use crate::timer::PIT_HZ;
use voice::Voice;

/// DRAM on the card.
pub const DRAM_SIZE: usize = 1 << 20;
const DRAM_MASK: u32 = DRAM_SIZE as u32 - 1;

/// IRQ and DMA channel of each latch value.
const IRQ_LATCH: [u8; 8] = [0, 2, 5, 3, 7, 11, 12, 15];
const DMA_LATCH: [u8; 8] = [0, 1, 3, 5, 6, 7, 0, 0];

/// DMA speed at the fastest rate setting, in bytes a second.
const DMA_RATE: u64 = 650_000;

// IRQ status bits (port 2X6h).
const IRQ_TIMER1: u8 = 0x04;
const IRQ_WAVE: u8 = 0x20;
const IRQ_RAMP: u8 = 0x40;
const IRQ_DMA: u8 = 0x80;

/// Resources of the card, from the configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GusConfig {
    pub enabled: bool,
    pub base: u16,
    pub irq: u8,
    pub dma: u8,
    /// The drive with the built-in Ultrasound software (see `builtin`),
    /// if there is one.
    pub drive: Option<u8>,
    /// The DOS directory of the Ultrasound software (ULTRADIR), when it is
    /// not the built-in one.
    pub ultradir: Option<String>,
}

impl Default for GusConfig {
    /// A card at 240h, IRQ 5, DMA 3, as DOSBox has it: the Sound Blaster
    /// has IRQ 7, and some games only accept Ultrasound IRQs up to 7. The
    /// built-in software is on X:.
    fn default() -> Self {
        Self { enabled: true, base: 0x240, irq: 5, dma: 3, drive: Some(b'X' - b'A'), ultradir: None }
    }
}

impl GusConfig {
    /// Whether ULTRADIR is the built-in Ultrasound software.
    pub fn builtin(&self) -> bool {
        self.ultradir.is_none() && self.drive.is_some()
    }

    /// ULTRADIR: the configured directory, else the built-in software's,
    /// else C:\ULTRASND, where the Gravis installer puts it.
    pub fn ultradir(&self) -> String {
        match (&self.ultradir, self.drive) {
            (Some(dir), _) => dir.clone(),
            (None, Some(drive)) => format!("{}:\\{}", crate::disk::drive_letter(drive), builtin::DIR),
            (None, None) => "C:\\ULTRASND".to_string(),
        }
    }

    /// The ULTRASND environment variable: base port, playback and record
    /// DMA, GF1 and MIDI IRQ.
    pub fn ultrasnd(&self) -> String {
        format!("{:X},{},{},{},{}", self.base, self.dma, self.dma, self.irq, self.irq)
    }
}

/// Frames a second the voices play at with `voices` active, times 1000.
fn frame_rate_milli(voices: u8) -> u64 {
    (1e9 / (1.619_695_497 * voices as f64)) as u64
}

/// Emulated nanoseconds at PIT tick `ticks`.
fn ticks_to_ns(ticks: u64) -> u64 {
    (ticks as u128 * 1_000_000_000 / PIT_HZ as u128) as u64
}

/// The first PIT tick at or after emulated nanosecond `ns`.
fn ns_to_ticks(ns: u64) -> u64 {
    (ns as u128 * PIT_HZ as u128).div_ceil(1_000_000_000) as u64
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
struct Timer {
    value: u8,
    running: bool,
    masked: bool,
    reached: bool,
    irq: bool,
    /// When it next expires, in emulated ns.
    next_ns: u64,
}

pub struct Gus {
    pub config: GusConfig,
    dram: Vec<u8>,
    voices: [Voice; 32],
    active: u8,
    voice_sel: u8,
    reg_sel: u8,
    /// The 16-bit register data written to 3X4h/3X5h.
    data: u16,
    /// What 3X3h reads: the last byte written to 3X3h, 3X4h or 3X5h
    /// (Jazz Jackrabbit's driver detects the card by it).
    select_readback: u8,
    dram_addr: u32,
    mix: u8,
    /// The next write to 2XBh sets a latch (it follows a write to 2X0h).
    latch_armed: bool,
    /// GF1 IRQ, MIDI IRQ and DMA channel from the latches.
    irq: u8,
    midi_irq: u8,
    dma_ch: u8,
    adlib_cmd: u8,
    reset_reg: u8,
    status: u8,
    /// Voices with a wave or volume IRQ pending.
    wave_irq: u32,
    ramp_irq: u32,
    /// The voice 8Fh reports next.
    irq_chan: u8,
    /// An event asked for an interrupt since the bus last looked.
    fresh: bool,
    timer_ctrl: u8,
    timers: [Timer; 2],
    dma_ctrl: u8,
    dma_addr: u16,
    dma_pos: u32,
    dma_active: bool,
    dma_frac: u64,
    sample_ctrl: u8,
    /// Emulated time (PIT ticks) the card has run up to, and the part of a
    /// frame (in frames * PIT ticks * 1000) not yet played.
    last_ticks: u64,
    frame_frac: u64,
    /// Stereo frames at the playback rate, waiting for the mixer.
    out: VecDeque<(f32, f32)>,
    mix_buf: Vec<(f32, f32)>,
    /// Resampling position between `prev` and `cur` for the mixer.
    phase: f64,
    prev: (f32, f32),
    cur: (f32, f32),
    /// Output gain: lowered at once when the voices' sum would clip, and
    /// recovering over a few seconds, as DOSBox does. Drivers that stream
    /// a software mix at full volume (HMI's) plus music otherwise clip.
    gain: f32,
}

/// Gain recovered per mixer frame: from half to full in about 2 s.
const GAIN_RECOVERY: f32 = 0.5 / 88_200.0;

impl Gus {
    pub fn new(config: GusConfig, now: u64) -> Self {
        let mut gus = Self {
            dram: vec![0; DRAM_SIZE],
            voices: [Voice::default(); 32],
            active: 14,
            voice_sel: 0,
            reg_sel: 0,
            data: 0,
            select_readback: 0,
            dram_addr: 0,
            mix: 0,
            latch_armed: false,
            irq: config.irq,
            midi_irq: config.irq,
            dma_ch: config.dma,
            adlib_cmd: 0,
            reset_reg: 0,
            status: 0,
            wave_irq: 0,
            ramp_irq: 0,
            irq_chan: 0,
            fresh: false,
            timer_ctrl: 0,
            timers: [Timer::default(); 2],
            dma_ctrl: 0,
            dma_addr: 0,
            dma_pos: 0,
            dma_active: false,
            dma_frac: 0,
            sample_ctrl: 0,
            last_ticks: now,
            frame_frac: 0,
            out: VecDeque::new(),
            mix_buf: Vec::new(),
            phase: 0.0,
            prev: (0.0, 0.0),
            cur: (0.0, 0.0),
            gain: 1.0,
            config,
        };
        gus.power_on();
        gus
    }

    /// The state ULTRINIT leaves: running with the DAC and interrupts on,
    /// the latches as configured, every voice stopped. DRAM keeps what it
    /// holds, so patches a resident driver loaded survive.
    pub fn power_on(&mut self) {
        self.gf1_reset();
        self.reset_reg = 0x07;
        self.mix = 0x08;
        self.irq = self.config.irq;
        self.midi_irq = self.config.irq;
        self.dma_ch = self.config.dma;
        self.voice_sel = 0;
        self.reg_sel = 0;
        self.data = 0;
        self.dram_addr = 0;
        self.out.clear();
        self.phase = 0.0;
        self.prev = (0.0, 0.0);
        self.cur = (0.0, 0.0);
    }

    /// Stop every voice, leaving the rest of the card alone: what a
    /// program exit does while a resident driver owns the card.
    pub fn silence(&mut self) {
        for v in &mut self.voices {
            v.wave_ctrl |= voice::STOPPED;
            v.ramp_ctrl |= voice::STOPPED;
            v.vol = 0;
        }
        self.wave_irq = 0;
        self.ramp_irq = 0;
        self.update_voice_irq();
    }

    /// Reset of the GF1 (register 4Ch bit 0 clear): voices, interrupts,
    /// timers and DMA, but not DRAM or the latches.
    fn gf1_reset(&mut self) {
        self.voices = [Voice::default(); 32];
        self.set_active(14);
        self.wave_irq = 0;
        self.ramp_irq = 0;
        self.irq_chan = 0;
        self.status = 0;
        self.fresh = false;
        self.timer_ctrl = 0;
        self.timers = [Timer { value: 0xFF, ..Timer::default() }; 2];
        self.dma_ctrl = 0;
        self.dma_active = false;
        self.sample_ctrl = 0;
        self.adlib_cmd = 85;
        self.mix = 0x0B;
        self.latch_armed = false;
    }

    /// Whether `port` is one of the card's.
    #[inline]
    pub fn claims(&self, port: u16) -> bool {
        let offset = port.wrapping_sub(self.config.base);
        offset < 0x10 || offset.wrapping_sub(0x100) < 8
    }

    /// Whether an access to `port` only latches a value: the voice and
    /// register selects (3X2h, 3X3h), and on a write the low data byte
    /// (3X4h). Nothing the card plays depends on them.
    #[inline]
    pub fn latch_only(&self, port: u16, write: bool) -> bool {
        let offset = port.wrapping_sub(self.config.base);
        offset == 0x102 || offset == 0x103 || (write && offset == 0x104)
    }

    /// The IRQ the card interrupts on, if it has one.
    pub fn irq(&self) -> Option<u8> {
        match self.irq {
            0 => None,
            2 => Some(9),
            n => Some(n),
        }
    }

    /// Whether the card's interrupt line is up.
    pub fn irq_line(&self) -> bool {
        let enabled = if self.reset_reg & 0x04 != 0 { 0xFF } else { !(IRQ_WAVE | IRQ_RAMP) };
        self.status & enabled != 0 && self.mix & 0x08 != 0
    }

    /// Whether something asked for an interrupt since the last call.
    pub fn take_fresh(&mut self) -> bool {
        std::mem::take(&mut self.fresh)
    }

    fn frame_rate_milli(&self) -> u64 {
        frame_rate_milli(self.active)
    }

    /// Frames a second the voices play at.
    pub fn frame_rate(&self) -> f64 {
        self.frame_rate_milli() as f64 / 1000.0
    }

    fn set_active(&mut self, voices: u8) {
        let voices = voices.clamp(14, 32);
        if voices != self.active {
            self.active = voices;
            self.frame_frac = 0;
        }
    }

    fn active_mask(&self) -> u32 {
        if self.active >= 32 { u32::MAX } else { (1 << self.active) - 1 }
    }

    /// Set the wave and volume IRQ bits of the status from the voices'
    /// pending IRQs, and point 8Fh at a voice that has one.
    fn update_voice_irq(&mut self) {
        self.status &= !(IRQ_WAVE | IRQ_RAMP);
        let mask = self.active_mask();
        let pending = (self.wave_irq | self.ramp_irq) & mask;
        if pending == 0 {
            return;
        }
        if self.wave_irq & mask != 0 {
            self.status |= IRQ_WAVE;
        }
        if self.ramp_irq & mask != 0 {
            self.status |= IRQ_RAMP;
        }
        self.fresh = true;
        for _ in 0..32 {
            if pending & (1 << self.irq_chan) != 0 {
                break;
            }
            self.irq_chan = (self.irq_chan + 1) % self.active;
        }
    }

    /// DRAM byte address of the DMA address register.
    fn dma_start(&self) -> u32 {
        let a = self.dma_addr as u32;
        let a = if self.dma_ctrl & 0x04 != 0 { (a & 0xC000) | ((a & 0x1FFF) << 1) } else { a };
        (a << 4) & DRAM_MASK
    }

    /// DMA bytes a second at the rate set in register 41h.
    fn dma_rate(&self) -> u64 {
        DMA_RATE / (1 + ((self.dma_ctrl >> 3) & 3) as u64)
    }

    // ---- Ports ----

    /// Port write. `now` is the emulated time (PIT ticks); the bus has
    /// advanced the card to it.
    pub fn write(&mut self, port: u16, value: u8, now: u64) {
        match port.wrapping_sub(self.config.base) {
            0x000 => {
                self.mix = value;
                self.latch_armed = true;
            }
            0x008 => self.adlib_cmd = value,
            0x009 => self.timer_command(value, now),
            0x00A => self.adlib_cmd = value,
            0x00B => {
                if std::mem::take(&mut self.latch_armed) {
                    self.set_latch(value);
                }
            }
            0x102 => self.voice_sel = value & 31,
            0x103 => {
                self.reg_sel = value;
                self.select_readback = value;
                self.data = 0;
            }
            0x104 => {
                self.data = (self.data & 0xFF00) | value as u16;
                self.select_readback = value;
            }
            0x105 => {
                self.data = (self.data & 0x00FF) | (value as u16) << 8;
                self.select_readback = value;
                self.write_register();
            }
            0x107 => self.dram[(self.dram_addr & DRAM_MASK) as usize] = value,
            // 3X0h/3X1h, the MIDI UART: nothing is connected.
            _ => {}
        }
    }

    /// A write to the AdLib address port 388h, which the card latches like
    /// one to 2X8h.
    pub fn write_adlib_address(&mut self, value: u8) {
        self.adlib_cmd = value;
    }

    pub fn read(&mut self, port: u16) -> u8 {
        match port.wrapping_sub(self.config.base) {
            0x006 => self.status,
            0x008 => {
                let mut v = 0;
                if self.timers[0].reached {
                    v |= 0x40;
                }
                if self.timers[1].reached {
                    v |= 0x20;
                }
                if v != 0 {
                    v |= 0x80;
                }
                if self.status & IRQ_TIMER1 != 0 {
                    v |= 0x04;
                }
                if self.status & (IRQ_TIMER1 << 1) != 0 {
                    v |= 0x02;
                }
                v
            }
            0x00A => self.adlib_cmd,
            // MIDI UART status: ready to send, nothing received.
            0x100 => 0x02,
            0x101 => 0x00,
            0x102 => self.voice_sel,
            0x103 => self.select_readback,
            0x104 => self.read_register(false) as u8,
            0x105 => (self.read_register(true) >> 8) as u8,
            0x107 => self.dram[(self.dram_addr & DRAM_MASK) as usize],
            _ => 0xFF,
        }
    }

    /// 2XBh after 2X0h: the IRQ latch (2X0h bit 6 set) or the DMA latch.
    /// A zero field leaves the setting alone.
    fn set_latch(&mut self, value: u8) {
        if self.mix & 0x40 != 0 {
            let gf1 = IRQ_LATCH[(value & 7) as usize];
            let midi = if value & 0x40 != 0 { gf1 } else { IRQ_LATCH[((value >> 3) & 7) as usize] };
            if gf1 != 0 {
                self.irq = gf1;
            }
            if midi != 0 {
                self.midi_irq = midi;
            }
        } else {
            let dma = DMA_LATCH[(value & 7) as usize];
            if dma != 0 {
                self.dma_ch = dma;
            }
        }
    }

    /// 2X9h: the AdLib-compatible timer command.
    fn timer_command(&mut self, value: u8, now: u64) {
        if value & 0x80 != 0 {
            self.timers[0].reached = false;
            self.timers[1].reached = false;
            return;
        }
        self.timers[0].masked = value & 0x40 != 0;
        self.timers[1].masked = value & 0x20 != 0;
        let now_ns = ticks_to_ns(now);
        for (t, step) in [(0usize, 80_000u64), (1, 320_000)] {
            let timer = &mut self.timers[t];
            if value & (1 << t) != 0 {
                if !timer.running {
                    timer.running = true;
                    timer.next_ns = now_ns + (256 - timer.value as u64) * step;
                }
            } else {
                timer.running = false;
            }
        }
    }

    fn write_register(&mut self) {
        let data = self.data;
        let hi = (data >> 8) as u8;
        let n = self.voice_sel as usize;
        let mask = 1u32 << n;
        let v = &mut self.voices[n];
        match self.reg_sel {
            0x00 => {
                v.wave_ctrl = hi & 0x7F;
                let old = self.wave_irq;
                if hi & 0xA0 == 0xA0 {
                    self.wave_irq |= mask;
                } else {
                    self.wave_irq &= !mask;
                }
                if self.wave_irq != old {
                    self.update_voice_irq();
                }
            }
            0x01 => v.freq = data,
            0x02 => v.start = (v.start & 0xFFFF) | ((data as u32 & 0x1FFF) << 16),
            0x03 => v.start = (v.start & !0xFFFF) | data as u32,
            0x04 => v.end = (v.end & 0xFFFF) | ((data as u32 & 0x1FFF) << 16),
            0x05 => v.end = (v.end & !0xFFFF) | data as u32,
            0x06 => v.ramp_rate = hi,
            0x07 => v.ramp_start = hi,
            0x08 => v.ramp_end = hi,
            0x09 => v.vol = ((data >> 4) as u32) << tables::RAMP_FRAC,
            0x0A => v.pos = (v.pos & 0xFFFF) | ((data as u32 & 0x1FFF) << 16),
            0x0B => v.pos = (v.pos & !0xFFFF) | data as u32,
            0x0C => v.pan = hi & 0x0F,
            0x0D => {
                v.ramp_ctrl = hi & 0x7F;
                let old = self.ramp_irq;
                if hi & 0xA0 == 0xA0 {
                    self.ramp_irq |= mask;
                } else {
                    self.ramp_irq &= !mask;
                }
                if self.ramp_irq != old {
                    self.update_voice_irq();
                }
            }
            0x0E => {
                self.set_active((hi & 31) + 1);
                self.update_voice_irq();
            }
            0x41 => {
                self.dma_ctrl = hi;
                if hi & 0x01 == 0 {
                    self.dma_active = false;
                } else if !self.dma_active {
                    self.dma_active = true;
                    self.dma_pos = self.dma_start();
                    self.dma_frac = 0;
                }
            }
            0x42 => self.dma_addr = data,
            0x43 => self.dram_addr = (self.dram_addr & 0xF_0000) | data as u32,
            0x44 => self.dram_addr = (self.dram_addr & 0xFFFF) | ((hi as u32 & 0x0F) << 16),
            0x45 => {
                self.timer_ctrl = hi;
                self.timers[0].irq = hi & 0x04 != 0;
                self.timers[1].irq = hi & 0x08 != 0;
                if !self.timers[0].irq {
                    self.status &= !IRQ_TIMER1;
                }
                if !self.timers[1].irq {
                    self.status &= !(IRQ_TIMER1 << 1);
                }
            }
            0x46 => self.timers[0].value = hi,
            0x47 => self.timers[1].value = hi,
            0x49 => self.sample_ctrl = hi,
            0x4C => {
                self.reset_reg = hi;
                if hi & 0x01 == 0 {
                    self.gf1_reset();
                }
            }
            _ => {}
        }
    }

    /// Value of the selected register. `ack` is set for the read of the
    /// high byte (3X5h), the one that acknowledges what it reports.
    fn read_register(&mut self, ack: bool) -> u16 {
        let n = self.voice_sel as usize;
        let mask = 1u32 << n;
        let v = &self.voices[n];
        let hi = |b: u8| (b as u16) << 8;
        match self.reg_sel {
            0x80 => hi(v.wave_ctrl | if self.wave_irq & mask != 0 { 0x80 } else { 0 }),
            0x81 => v.freq,
            0x82 => (v.start >> 16) as u16 & 0x1FFF,
            0x83 => v.start as u16,
            0x84 => (v.end >> 16) as u16 & 0x1FFF,
            0x85 => v.end as u16,
            0x86 => hi(v.ramp_rate),
            0x87 => hi(v.ramp_start),
            0x88 => hi(v.ramp_end),
            0x89 => ((v.vol >> tables::RAMP_FRAC) << 4) as u16,
            0x8A => (v.pos >> 16) as u16 & 0x1FFF,
            0x8B => v.pos as u16,
            0x8C => hi(v.pan),
            0x8D => hi(v.ramp_ctrl | if self.ramp_irq & mask != 0 { 0x80 } else { 0 }),
            0x8E => hi(0xC0 | (self.active - 1)),
            0x8F => {
                let ch = self.irq_chan;
                let mask = 1u32 << ch;
                let mut value = ch | 0x20;
                if self.ramp_irq & mask == 0 {
                    value |= 0x40;
                }
                if self.wave_irq & mask == 0 {
                    value |= 0x80;
                }
                if ack {
                    self.wave_irq &= !mask;
                    self.ramp_irq &= !mask;
                    self.update_voice_irq();
                }
                hi(value)
            }
            0x41 => {
                let value = (self.dma_ctrl & !0x40) | if self.status & IRQ_DMA != 0 { 0x40 } else { 0 };
                if ack {
                    self.status &= !IRQ_DMA;
                }
                hi(value)
            }
            0x42 => self.dma_addr,
            0x43 => self.dram_addr as u16,
            0x44 => hi((self.dram_addr >> 16) as u8),
            0x45 => hi(self.timer_ctrl),
            0x46 => hi(self.timers[0].value),
            0x47 => hi(self.timers[1].value),
            0x49 => hi((self.sample_ctrl & !0x40) | if self.status & IRQ_DMA != 0 { 0x40 } else { 0 }),
            0x4C => hi(self.reset_reg),
            _ => self.data,
        }
    }

    // ---- Emulated time ----

    /// Run the card up to emulated time `now` (PIT ticks): DMA, timers and
    /// the voices. Returns the system memory a DMA transfer from the card
    /// wrote, if any.
    pub fn advance(&mut self, now: u64, dma: &mut Dma, ram: &mut [u8]) -> Option<Range<usize>> {
        let elapsed = now.saturating_sub(self.last_ticks);
        self.last_ticks = self.last_ticks.max(now);
        let written = self.run_dma(elapsed, dma, ram);
        self.run_timers(now);

        let rate = self.frame_rate_milli() as u128;
        let unit = PIT_HZ as u128 * 1000;
        let total = elapsed as u128 * rate + self.frame_frac as u128;
        let mut frames = (total / unit) as u64;
        self.frame_frac = (total % unit) as u64;
        // After a long pause (a debugger stop), play on from here rather
        // than render the gap.
        let max = self.frame_rate_milli() / 2000;
        if frames > max {
            frames = max;
        }
        self.render(frames as usize);
        written
    }

    fn run_timers(&mut self, now: u64) {
        let now_ns = ticks_to_ns(now);
        for (t, step) in [(0usize, 80_000u64), (1, 320_000)] {
            let timer = &mut self.timers[t];
            if !timer.running || timer.next_ns > now_ns {
                continue;
            }
            let period = (256 - timer.value as u64) * step;
            let periods = (now_ns - timer.next_ns) / period + 1;
            timer.next_ns += periods * period;
            if !timer.masked {
                timer.reached = true;
            }
            if timer.irq {
                self.status |= IRQ_TIMER1 << t;
                self.fresh = true;
            }
        }
    }

    /// Move the DMA bytes that came due in `elapsed` PIT ticks.
    fn run_dma(&mut self, elapsed: u64, dma: &mut Dma, ram: &mut [u8]) -> Option<Range<usize>> {
        if !self.dma_active {
            return None;
        }
        let ch = self.dma_ch as usize;
        let unit = if ch >= 4 { 2u64 } else { 1 };
        let total = elapsed as u128 * self.dma_rate() as u128 + self.dma_frac as u128;
        let mut bytes = (total / PIT_HZ as u128) as u64;
        self.dma_frac = (total % PIT_HZ as u128) as u64;
        if dma.channel(ch).masked {
            // Nothing moves while the channel is masked.
            self.dma_frac = 0;
            return None;
        }
        // Whole transfers only; the odd byte waits for the next call.
        self.dma_frac += (bytes % unit) * PIT_HZ;
        bytes -= bytes % unit;
        let mut written: Option<Range<usize>> = None;
        let mut buf = [0u8; 4096];
        while bytes > 0 && self.dma_active {
            let left = (dma.channel(ch).cur_count as u64 + 1) * unit;
            let n = bytes.min(left).min(buf.len() as u64) as usize;
            let (moved, tc) = if self.dma_ctrl & 0x02 == 0 {
                let (moved, tc) = dma.transfer_read(ch, ram, &mut buf[..n]);
                for (i, &b) in buf[..moved].iter().enumerate() {
                    let addr = (self.dma_pos + i as u32) & DRAM_MASK;
                    let invert = self.dma_ctrl & 0x80 != 0 && (self.dma_ctrl & 0x40 == 0 || addr & 1 == 1);
                    self.dram[addr as usize] = if invert { b ^ 0x80 } else { b };
                }
                (moved, tc)
            } else {
                for (i, b) in buf[..n].iter_mut().enumerate() {
                    *b = self.dram[((self.dma_pos + i as u32) & DRAM_MASK) as usize];
                }
                let (moved, tc, span) = dma.transfer_write(ch, ram, &buf[..n]);
                if let Some(s) = span {
                    written = Some(match written {
                        Some(w) => w.start.min(s.start)..w.end.max(s.end),
                        None => s,
                    });
                }
                (moved, tc)
            };
            self.dma_pos = (self.dma_pos + moved as u32) & DRAM_MASK;
            bytes -= n as u64;
            if tc {
                self.dma_active = false;
                self.dma_ctrl &= !0x01;
                if self.dma_ctrl & 0x20 != 0 {
                    self.status |= IRQ_DMA;
                    self.fresh = true;
                }
            } else if moved < n {
                break;
            }
        }
        written
    }

    /// Play `frames` frames of the voices into `out`.
    fn render(&mut self, frames: usize) {
        if frames == 0 {
            return;
        }
        let running = self.reset_reg & 0x01 != 0;
        let audible = running && self.voices[..self.active as usize].iter().any(|v| !v.silent());
        if audible {
            self.mix_buf.clear();
            self.mix_buf.resize(frames, (0.0, 0.0));
            let dram = &self.dram;
            let mut raised = false;
            for (i, v) in self.voices[..self.active as usize].iter_mut().enumerate() {
                if v.silent() {
                    continue;
                }
                for frame in self.mix_buf.iter_mut() {
                    let s = v.sample(dram);
                    let (gl, gr) = v.gains();
                    frame.0 += s * gl;
                    frame.1 += s * gr;
                    let events = v.step();
                    if events != 0 {
                        if events & voice::WAVE_IRQ != 0 {
                            self.wave_irq |= 1 << i;
                        }
                        if events & voice::RAMP_IRQ != 0 {
                            self.ramp_irq |= 1 << i;
                        }
                        raised = true;
                    }
                }
            }
            if raised {
                self.update_voice_irq();
            }
            let dac = self.reset_reg & 0x02 != 0;
            for &frame in &self.mix_buf {
                self.out.push_back(if dac { frame } else { (0.0, 0.0) });
            }
        } else {
            self.out.extend(std::iter::repeat_n((0.0, 0.0), frames));
        }
        let max = (self.frame_rate_milli() / 2000) as usize;
        if self.out.len() > max {
            let extra = self.out.len() - max;
            self.out.drain(..extra);
        }
    }

    /// When the card next needs attention without a port access: a timer
    /// expiring, a DMA transfer ending, or a voice reaching the point where
    /// it raises an IRQ. In PIT ticks.
    pub fn next_event(&self, dma: &Dma) -> Option<u64> {
        let mut next: Option<u64> = None;
        let mut consider = |t: u64| next = Some(next.map_or(t, |n| n.min(t)));
        for timer in &self.timers {
            // Only an interrupt needs the time; a program polling 2X8h
            // brings the card up to date first.
            if timer.running && timer.irq {
                consider(ns_to_ticks(timer.next_ns));
            }
        }
        if self.dma_active && !dma.channel(self.dma_ch as usize).masked {
            let unit = if self.dma_ch >= 4 { 2 } else { 1 };
            let bytes = (dma.channel(self.dma_ch as usize).cur_count as u128 + 1) * unit;
            let need = (bytes * PIT_HZ as u128).saturating_sub(self.dma_frac as u128);
            consider(self.last_ticks + need.div_ceil(self.dma_rate() as u128) as u64);
        }
        if self.reset_reg & 0x01 != 0 {
            let frames = self.voices[..self.active as usize].iter().filter_map(|v| v.frames_to_irq()).min();
            if let Some(k) = frames {
                let need = (k as u128 * PIT_HZ as u128 * 1000).saturating_sub(self.frame_frac as u128);
                consider(self.last_ticks + need.div_ceil(self.frame_rate_milli() as u128).max(1) as u64);
            }
        }
        next
    }

    // ---- Output ----

    /// One stereo frame for a mixer running at `rate` frames a second,
    /// interpolated from the card's output.
    #[inline]
    pub fn pop_frame(&mut self, rate: u32) -> (f32, f32) {
        self.phase += self.frame_rate() / rate as f64;
        while self.phase >= 1.0 {
            match self.out.pop_front() {
                Some(frame) => {
                    self.prev = self.cur;
                    self.cur = frame;
                    self.phase -= 1.0;
                }
                None => {
                    // Behind by a frame of rounding: hold the last one.
                    self.prev = self.cur;
                    self.phase = 1.0;
                    break;
                }
            }
        }
        let t = self.phase.min(1.0) as f32;
        let l = self.prev.0 + (self.cur.0 - self.prev.0) * t;
        let r = self.prev.1 + (self.cur.1 - self.prev.1) * t;
        let peak = l.abs().max(r.abs());
        if peak * self.gain > 32767.0 {
            self.gain = 32767.0 / peak;
        }
        let out = (l * self.gain, r * self.gain);
        self.gain = (self.gain + GAIN_RECOVERY).min(1.0);
        out
    }

    /// Drop output beyond a tenth of a second that nobody took.
    pub fn trim_output(&mut self) {
        let keep = (self.frame_rate_milli() / 10_000) as usize;
        if self.out.len() > keep {
            let extra = self.out.len() - keep;
            self.out.drain(..extra);
        }
    }

    /// A DRAM byte, for tests and the debugger.
    pub fn peek(&self, addr: u32) -> u8 {
        self.dram[(addr & DRAM_MASK) as usize]
    }

    /// The card's state for the debugger.
    pub fn snapshot(&self) -> serde_json::Value {
        let voices: Vec<serde_json::Value> = self
            .voices
            .iter()
            .enumerate()
            .map(|(i, v)| {
                serde_json::json!({
                    "voice": i,
                    "active": i < self.active as usize,
                    "wave_ctrl": format!("{:02X}", v.wave_ctrl),
                    "ramp_ctrl": format!("{:02X}", v.ramp_ctrl),
                    "freq": format!("{:04X}", v.freq),
                    "start": format!("{:05X}", v.start >> voice::WAVE_FRAC),
                    "end": format!("{:05X}", v.end >> voice::WAVE_FRAC),
                    "pos": format!("{:05X}", v.pos >> voice::WAVE_FRAC),
                    "volume": format!("{:03X}", v.vol >> tables::RAMP_FRAC),
                    "ramp": format!("{:02X} {:02X}-{:02X}", v.ramp_rate, v.ramp_start, v.ramp_end),
                    "pan": v.pan,
                    "wave_irq": self.wave_irq & (1 << i) != 0,
                    "ramp_irq": self.ramp_irq & (1 << i) != 0,
                })
            })
            .collect();
        let playing = self.voices[..self.active as usize].iter().filter(|v| !v.silent()).count();
        serde_json::json!({
            "base": format!("{:X}", self.config.base),
            "irq": self.irq(),
            "midi_irq": self.midi_irq,
            "dma": self.dma_ch,
            "reset": format!("{:02X}", self.reset_reg),
            "mix": format!("{:02X}", self.mix),
            "irq_status": format!("{:02X}", self.status),
            "irq_line": self.irq_line(),
            "active_voices": self.active,
            "playing_voices": playing,
            "frame_rate": self.frame_rate(),
            "timer_ctrl": format!("{:02X}", self.timer_ctrl),
            "timers": self.timers,
            "dma_ctrl": format!("{:02X}", self.dma_ctrl),
            "dma_addr": format!("{:04X}", self.dma_addr),
            "dma_active": self.dma_active,
            "dma_pos": format!("{:05X}", self.dma_pos),
            "dram_addr": format!("{:05X}", self.dram_addr),
            "queued_frames": self.out.len(),
            "output_gain": self.gain,
            "voices": voices,
        })
    }
}
