//! Sound Blaster digital audio: the DSP and the mixer of the SB 2.0 (DSP
//! 2.01), SB Pro 2 (DSP 3.02, stereo, CT1345 mixer) and SB16 (DSP 4.05,
//! 16-bit transfers, CT1745 mixer). The FM chip is the `rust-dos-opl` crate's.
//!
//! The DSP runs on emulated time: while a DMA transfer is active it pulls
//! samples from the DMA controller at its sample rate, and raises its IRQ
//! at the end of each block at the emulated time a real card would. The
//! bus advances it (`advance`) before every port access and when its next
//! event (`next_event`) is due.
//!
//! Each sample sets the DAC's level (`dac`), which the card holds until the
//! next one: through the gap between two single-cycle blocks, and after a
//! transfer ends, as a real card's DAC does. The output is that level
//! sampled at the mixer's rate: `advance` makes one frame of `out` for
//! every frame of emulated time, numbered as the mixer numbers them, and
//! the mixer takes one for each frame it mixes. Card and mixer count
//! frames from the same clock, so neither can get ahead of the other, and
//! a sample's time decides which frames carry it: no frame is guessed
//! while a sample is late, and none is dropped or doubled to catch up.

use std::collections::VecDeque;

use rust_dos_dma::Dma;
use rust_dos_audio_core::{PIT_HZ, RATE, mul_div, mul_div_ceil};

/// Which card is emulated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SbModel {
    Sb2,
    SbPro2,
    Sb16,
    /// Creative's ViBRA16C (CTL0070), an SB16 on one chip that is an ISA
    /// Plug and Play card: its resources are set through the PnP
    /// registers, and its mixer's 80h and 81h only read them.
    Sb16Vibra,
    /// An SB16 with the EMU8000 wavetable synthesizer (`crate::awe32`).
    Awe32,
}

impl SbModel {
    /// DSP version: major, minor.
    fn dsp_version(self) -> (u8, u8) {
        match self {
            SbModel::Sb2 => (2, 1),
            SbModel::SbPro2 => (3, 2),
            SbModel::Sb16 => (4, 5),
            // The ViBRA16C's, as DOSBox-X gives it.
            SbModel::Sb16Vibra => (4, 13),
            // A CT3990's.
            SbModel::Awe32 => (4, 13),
        }
    }

    /// Whether the card is an SB16, as the AWE32 is too.
    #[inline]
    pub fn is_sb16(self) -> bool {
        matches!(self, SbModel::Sb16 | SbModel::Sb16Vibra | SbModel::Awe32)
    }

    /// The T value of the BLASTER variable.
    pub fn blaster_type(self) -> u8 {
        match self {
            SbModel::Sb2 => 3,
            SbModel::SbPro2 => 4,
            SbModel::Sb16 | SbModel::Sb16Vibra | SbModel::Awe32 => 6,
        }
    }

    /// The `sbtype` setting that selects the card.
    pub fn name(self) -> &'static str {
        match self {
            SbModel::Sb2 => "sb2",
            SbModel::SbPro2 => "sbpro2",
            SbModel::Sb16 => "sb16",
            SbModel::Sb16Vibra => "sb16vibra",
            SbModel::Awe32 => "awe32",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "sb2" | "sb20" => Some(SbModel::Sb2),
            "sbpro2" | "sbpro" => Some(SbModel::SbPro2),
            "sb16" => Some(SbModel::Sb16),
            "sb16vibra" | "vibra" => Some(SbModel::Sb16Vibra),
            "awe32" | "sbawe" | "awe" => Some(SbModel::Awe32),
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
        if self.model.is_sb16() {
            s.push_str(&format!(" H{} P330", self.dma16));
        }
        // The EMU8000's base, before T as Creative's installer writes it.
        if self.model == SbModel::Awe32 {
            s.push_str(&format!(" E{:X}", self.base + 0x400));
        }
        s.push_str(&format!(" T{}", self.model.blaster_type()));
        s
    }
}

/// A DMA transfer the DSP runs.
#[derive(Clone, Copy, Debug, Default)]
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
    /// Started by a high-speed command (90h, 91h, 98h, 99h) on an SB 2.0
    /// or SB Pro: the DSP takes no command while it runs.
    high_speed: bool,
    /// A byte the DSP ignored in high-speed mode has been logged; the
    /// later ones of the transfer go unlogged.
    ignored_logged: bool,
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
/// The SB Pro's mixer registers: voice, mic, input, output, master, FM,
/// CD and line.
/// What an empty ASP socket's registers read (DOSBox-X's start values, from
/// an SB16): their own number, but 0 for 00h-1Fh and 80h-83h, 01h for 05h
/// and F8h for 09h. Windows 98's SB16.VXD fails to start (Code 10) on a
/// card whose 09h isn't F8h.
const EMPTY_ASP_REGS: [u8; 256] = {
    let mut regs = [0; 256];
    let mut i = 0x20;
    while i < 256 {
        if i < 0x80 || i > 0x83 {
            regs[i] = i as u8;
        }
        i += 1;
    }
    regs[0x05] = 0x01;
    regs[0x09] = 0xF8;
    regs
};

/// The 8051 microcontroller's memory as an SB16 starts (DOSBox-X's values,
/// from a card's): zeros but for 0Eh, 0Fh and 37h. Windows 98's SB16.VXD
/// reads 0Eh, and fails to start (Code 10) where it finds 00h.
const MEM8051_START: [u8; 256] = {
    let mut mem = [0; 256];
    mem[0x0E] = 0xFF;
    mem[0x0F] = 0x07;
    mem[0x37] = 0x38;
    mem
};

const PRO_REGS: [u8; 9] = [0x00, 0x04, 0x0A, 0x0C, 0x0E, 0x22, 0x26, PRO_CD, 0x2E];

/// The SB16 registers (left, right) that an SB Pro volume register (04h,
/// 22h, 26h or 28h) reads and writes.
fn pro_volume_regs(reg: u8) -> (u8, u8) {
    match reg {
        0x04 => (VOICE_L, VOICE_R),
        0x22 => (MASTER_L, MASTER_R),
        0x26 => (FM_L, FM_R),
        _ => (CD_L, CD_R),
    }
}

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
    /// The DAC's level, left and right, signed: the last sample it was
    /// given, by a transfer or by command 10h. The DAC holds it until the
    /// next sample, however long that takes.
    pub dac: (i16, i16),
    /// Frames of output at `RATE`, each the DAC's level at the frame's
    /// time, up to `last_ticks`; the mixer takes one per frame it mixes.
    pub out: VecDeque<(i16, i16)>,
    /// Frames of output made since power-on, in the mixer's numbering:
    /// frame n is the level at the first tick t with t * RATE >= n * PIT_HZ.
    frames: u64,
    /// Sample frames per second of the last transfer: the rate `dac`
    /// changes at, for the output filter.
    pub out_rate: u32,
    /// A left sample waiting for its right one.
    pending_left: Option<i16>,
    mixer_index: u8,
    mixer: [u8; 256],
    /// The SB16's Advanced Signal Processor socket, empty: its mode and
    /// registers (commands 04h, 0Eh and 0Fh), which drivers probe for it.
    asp_mode: u8,
    asp_regs: [u8; 256],
    /// The SB16's 8051 microcontroller's memory (commands F9h and FAh).
    mem8051: [u8; 256],
    /// The IRQ and DMA channels the card uses now: the configured ones
    /// until a driver selects others in the SB16's mixer (80h and 81h), as
    /// Windows 95's driver does with those Device Manager gives it. None
    /// when it selects none.
    pub irq: Option<u8>,
    dma8: Option<u8>,
    dma16: Option<u8>,
    /// Command E2h (DMA identification): the running value its answers
    /// come from, the number of times it ran, and the answer waiting for
    /// the 8-bit DMA channel to take it.
    e2_value: u8,
    e2_count: u8,
    e2_pending: Option<u8>,
}

/// What each parameter bit of command E2h adds to its running value, for
/// the four calls in turn, and what each call adds besides (the last
/// column), as DOSBox and DOSBox-X have it from real cards.
const E2_ADD: [[i16; 9]; 4] = [
    [0x01, -0x02, -0x04, 0x08, -0x10, 0x20, 0x40, -0x80, -106],
    [-0x01, 0x02, -0x04, 0x08, 0x10, -0x20, 0x40, -0x80, 165],
    [-0x01, 0x02, 0x04, -0x08, 0x10, -0x20, -0x40, 0x80, -151],
    [0x01, -0x02, 0x04, -0x08, -0x10, 0x20, -0x40, 0x80, 90],
];

/// While a transfer runs, emulated time is cut into slots of `BUSY_TICKS`
/// PIT ticks (about 3.4 us), and the write status reads busy in about one
/// slot in five, never in two slots in a row.
const BUSY_TICKS: u64 = 4;

/// Whether the DSP is busy in time slot `slot` while a transfer runs. A
/// hash of the slot number picks the busy slots instead of a fixed period,
/// so a poll loop whose period happens to be a multiple of a fixed
/// pattern's (at some cycles setting) does not sample the same phase on
/// every read: it sees both states within a few reads.
fn busy_slot(slot: u64) -> bool {
    let picked = |n: u64| {
        let mut x = n.wrapping_add(0x9E37_79B9_7F4A_7C15);
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (x ^ (x >> 31)) & 3 == 0
    };
    picked(slot) && !picked(slot.wrapping_sub(1))
}

/// Frames `out` keeps at most (about a second), for when nobody drains it.
const OUT_MAX: usize = 48_000;

impl SoundBlaster {
    /// A card at emulated time `now` (PIT ticks), from which its output
    /// frames count.
    pub fn new(config: SbConfig, now: u64) -> Self {
        let mut sb = Self {
            config,
            reset_stage: 0,
            in_command: None,
            params: Vec::with_capacity(4),
            params_needed: 0,
            read_buf: VecDeque::new(),
            test_reg: 0,
            speaker_on: config.model.is_sb16(),
            tc_rate: 11025,
            sb16_rate: None,
            block_size: 0x800,
            transfer: None,
            silence: None,
            irq8: false,
            irq16: false,
            last_ticks: now,
            frac: 0,
            dac: (0, 0),
            out: VecDeque::new(),
            frames: mul_div(now, RATE as u64, PIT_HZ),
            out_rate: 11025,
            pending_left: None,
            mixer_index: 0,
            mixer: [0; 256],
            asp_mode: 0,
            asp_regs: EMPTY_ASP_REGS,
            mem8051: MEM8051_START,
            irq: Some(config.irq),
            dma8: Some(config.dma8),
            dma16: Some(config.dma16),
            e2_value: 0xAA,
            e2_count: 0,
            e2_pending: None,
        };
        sb.reset_mixer();
        sb
    }

    /// Route the card to an IRQ and DMA channels, as an ISA Plug and Play
    /// configuration does (None for none). A 16-bit channel below 4 is
    /// none: 16-bit transfers then use the 8-bit channel.
    pub fn set_lines(&mut self, irq: Option<u8>, dma8: Option<u8>, dma16: Option<u8>) {
        self.irq = irq;
        self.dma8 = dma8;
        self.dma16 = dma16.filter(|&ch| ch >= 4);
    }

    /// The 8-bit and 16-bit DMA channels the card uses now (see `irq`).
    pub fn dma(&self) -> (Option<u8>, Option<u8>) {
        (self.dma8, self.dma16)
    }

    /// Whether the card's interrupt line is up.
    #[inline]
    pub fn irq_pending(&self) -> bool {
        self.irq8 || self.irq16
    }

    fn reset_dsp(&mut self) {
        // A reset in high-speed mode is how a driver leaves it. Creative's
        // hardware programming guide says it restores the DSP's parameters
        // to what they were before the mode began, and the DSP took no
        // command since, so the speaker keeps its setting here (the time
        // constant and block size outlive any reset). The transfer, the
        // pending IRQ and the read buffer go as on any reset, and the DSP
        // answers AAh, as DOSBox-X does.
        if !self.high_speed() {
            self.speaker_on = self.config.model.is_sb16();
        }
        self.in_command = None;
        self.params.clear();
        self.read_buf.clear();
        self.transfer = None;
        self.silence = None;
        self.irq8 = false;
        self.irq16 = false;
        self.sb16_rate = None;
        self.pending_left = None;
        self.dac = (0, 0);
        self.e2_pending = None;
    }

    /// Write the answer of command E2h to memory once the 8-bit DMA
    /// channel is unmasked. Returns the memory written. A channel set to
    /// read memory or to verify takes the byte as one transfer and leaves
    /// memory alone, as the 8237 does in those modes.
    pub fn dma_write(&mut self, dma: &mut Dma, ram: &mut [u8]) -> Option<std::ops::Range<usize>> {
        let byte = self.e2_pending?;
        let ch = self.dma8.map(usize::from).filter(|&ch| dma.ready(ch))?;
        if dma.channel(ch).mode & 0x0C != 0x04 {
            dma.transfer_skip(ch, 1);
            self.e2_pending = None;
            return None;
        }
        let (moved, _, written) = dma.transfer_write(ch, ram, &[byte]);
        if moved == 1 {
            self.e2_pending = None;
        }
        written
    }

    /// Start command E2h's sequence over, as after a power-on. A loaded
    /// state does this: the sequence isn't saved, and keeping the running
    /// machine's would make the answers depend on what ran before the
    /// load.
    pub fn restart_dma_identification(&mut self) {
        self.e2_value = 0xAA;
        self.e2_count = 0;
        self.e2_pending = None;
    }

    /// Whether the DSP is in high-speed mode: a transfer that a high-speed
    /// command started on an SB 2.0 or SB Pro runs. Until its block ends
    /// (91h, 99h) or a reset ends it (any of them), the DSP ignores the
    /// bytes written to its command port and the write status reads busy.
    pub fn high_speed(&self) -> bool {
        self.transfer.is_some_and(|t| t.high_speed)
    }

    /// Put the running transfer in or out of high-speed mode, as a save
    /// state has it (the state keeps the mode apart from the transfer).
    /// The SB16 has no such mode, so on one the transfer stays out of it.
    /// False when `on` asks for the mode and no transfer runs.
    pub fn set_high_speed(&mut self, on: bool) -> bool {
        let sb16 = self.config.model.is_sb16();
        match &mut self.transfer {
            Some(t) => {
                t.high_speed = on && !sb16;
                t.ignored_logged = false;
                true
            }
            None => !on,
        }
    }

    fn reset_mixer(&mut self) {
        self.mixer = [0; 256];
        for reg in [MASTER_L, MASTER_R, VOICE_L, VOICE_R, FM_L, FM_R, CD_L, CD_R] {
            self.mixer[reg as usize] = 0xC0;
        }
    }

    /// Volume of the digital audio and of FM, as (left, right) gains
    /// including the master volume.
    #[inline]
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
    #[inline]
    pub fn cd_volume(&self) -> (f32, f32) {
        if self.config.model == SbModel::Sb2 {
            return (1.0, 1.0);
        }
        let level = |reg: u8| (self.mixer[reg as usize] >> 3) as f32 / 31.0;
        (level(CD_L) * level(MASTER_L), level(CD_R) * level(MASTER_R))
    }

    /// DMA channel of a transfer. An SB16 with no 16-bit channel selected
    /// moves 16-bit data over its 8-bit one.
    fn channel(&self, bits16: bool) -> Option<usize> {
        let high = if bits16 && self.config.model.is_sb16() { self.dma16 } else { None };
        high.or(self.dma8).map(usize::from)
    }

    /// Run the DSP up to emulated time `now` (PIT ticks): play the DMA
    /// data that has come due, raise the block-end IRQs, and make the
    /// frames of output due by then, each the DAC's level at its time.
    pub fn advance(&mut self, now: u64, dma: &mut Dma, ram: &[u8]) {
        let end = mul_div(now, RATE as u64, PIT_HZ);
        while self.frames < end {
            self.frames += 1;
            let at = mul_div_ceil(self.frames, PIT_HZ, RATE as u64);
            self.run_to(at, dma, ram);
            self.out.push_back(self.dac);
        }
        if self.out.len() > OUT_MAX {
            let extra = self.out.len() - OUT_MAX;
            self.out.drain(..extra);
        }
        self.run_to(now, dma, ram);
    }

    /// Whether the card makes no sound: no transfer or silence period runs,
    /// and the DAC sits at the centre, as do the frames not yet taken.
    pub fn is_idle(&self) -> bool {
        self.transfer.is_none()
            && self.silence.is_none()
            && self.dac == (0, 0)
            && self.out.iter().all(|&f| f == (0, 0))
    }

    /// Run the DSP up to `now`: the DMA units due by then, and the IRQ of a
    /// block that ends.
    fn run_to(&mut self, now: u64, dma: &mut Dma, ram: &[u8]) {
        let elapsed = now.saturating_sub(self.last_ticks);
        self.last_ticks = now;
        let Some(t) = self.transfer.filter(|t| !t.paused) else {
            if let Some(left) = self.silence {
                // Command 80h: count silent units at the time-constant rate,
                // keeping the part of a unit that has passed, as a driver
                // polling the DSP while it waits advances it a tick at a time.
                let total = elapsed as u128 * self.tc_rate as u128 + self.frac as u128;
                let units = total / PIT_HZ as u128;
                if units >= left as u128 {
                    self.silence = None;
                    self.irq8 = true;
                    self.frac = 0;
                } else {
                    self.silence = Some(left - units as u32);
                    self.frac = (total % PIT_HZ as u128) as u64;
                }
            } else {
                self.frac = 0;
            }
            return;
        };
        let total = elapsed as u128 * t.rate as u128 + self.frac as u128;
        let mut units = (total / PIT_HZ as u128) as u64;
        self.frac = (total % PIT_HZ as u128) as u64;
        let ch = self.channel(t.bits16);
        let unit_bytes = if t.bits16 { 2 } else { 1 };
        while units > 0 {
            let Some(mut t) = self.transfer else { break };
            // The DSP waits while its DMA channel is masked, as a real one
            // waits for its request to be answered: no data, no IRQ. HMI's
            // setup finds the card's channel by trying each one. So does one
            // with no channel selected.
            let Some(ch) = ch.filter(|&ch| dma.ready(ch)) else {
                self.frac = 0;
                break;
            };
            // Made here, where a unit is read: `advance` runs this once a
            // frame, and most frames at a low rate have no unit due.
            let mut buf = [0u8; 4096];
            let mut k = (units.min(t.remaining as u64) as usize).min(buf.len() / unit_bytes);
            if t.input {
                dma.transfer_skip(ch, k);
            } else {
                let (moved, _) = dma.transfer_read(ch, ram, &mut buf[..k * unit_bytes]);
                k = moved / unit_bytes;
                self.play(&t, &buf[..k * unit_bytes]);
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

    /// Give the DAC the samples of a transfer. Several due in one frame
    /// leave it at the last: the card changes level faster than the mixer
    /// samples it.
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
                    Some(left) => self.dac = (left, s),
                }
            } else {
                self.dac = (s, s);
            }
        }
        self.out_rate = if t.stereo { t.rate / 2 } else { t.rate };
    }

    /// When the DSP next needs attention: the end of the current block
    /// (or of a silence period), in PIT ticks.
    #[inline]
    pub fn next_event(&self) -> Option<u64> {
        if let Some(t) = self.transfer.filter(|t| !t.paused && t.rate > 0) {
            let need = (t.remaining as u128 * PIT_HZ as u128).saturating_sub(self.frac as u128);
            return Some(self.last_ticks + need.div_ceil(t.rate as u128) as u64);
        }
        self.silence.map(|left| {
            let need = (left as u128 * PIT_HZ as u128).saturating_sub(self.frac as u128);
            self.last_ticks + need.div_ceil(self.tc_rate.max(1) as u128) as u64
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
            // Write status: busy for good in high-speed mode, and now and
            // then while another transfer runs.
            0xC if self.high_speed() => 0xFF,
            0xC => self.write_status(),
            // Read status: bit 7 when data waits. Acknowledges the 8-bit
            // (and on older cards the only) interrupt.
            0xE => {
                self.irq8 = false;
                if self.read_buf.is_empty() { 0x7F } else { 0xFF }
            }
            // SB16: acknowledges the 16-bit interrupt.
            0xF if self.config.model.is_sb16() => {
                self.irq16 = false;
                0xFF
            }
            _ => 0xFF,
        }
    }

    /// The write status port (2xCh). An idle DSP takes a byte at once. One
    /// running a DMA transfer is busy for a moment now and then while it
    /// services the transfer (at most 3.4 us at a time, about a fifth of
    /// the time), so reads in a loop see bit 7 set every few reads. Miles'
    /// AIL 2 Sound Blaster drivers (SBPDIG.ADV) rely on that when they
    /// stop a block that is still playing: they poll for
    /// bit 7 with the IRQ vector already restored, then halt the DMA. A
    /// DSP that never reads busy keeps them polling until the block ends,
    /// its IRQ goes to the BIOS's handler, and the card's request stays
    /// up unacknowledged, so the next block's end raises no new interrupt.
    /// `last_ticks` is the present here: port reads bring the DSP up to
    /// date first.
    fn write_status(&self) -> u8 {
        let running = self.transfer.is_some_and(|t| !t.paused);
        if running && busy_slot(self.last_ticks / BUSY_TICKS) { 0xFF } else { 0x7F }
    }

    fn mixer_write(&mut self, value: u8) {
        let reg = self.mixer_index;
        if self.config.model == SbModel::Sb2 {
            return;
        }
        // The SB Pro's mixer has only its own registers; the SB16's
        // (30h-47h) are where this keeps the levels.
        if !self.config.model.is_sb16() && !PRO_REGS.contains(&reg) {
            return;
        }
        match reg {
            0x00 => self.reset_mixer(),
            // SB Pro volumes: a nibble per side, kept in the SB16's registers.
            0x04 | 0x22 | 0x26 | PRO_CD => {
                let (l, r) = pro_volume_regs(reg);
                self.mixer[l as usize] = (value & 0xF0) | 0x08;
                self.mixer[r as usize] = (value << 4) | 0x08;
            }
            // The ViBRA16C takes its interrupt and DMA channels from its
            // Plug and Play registers only (DOSBox-X, sblaster.cpp).
            0x80 | 0x81 if self.config.model == SbModel::Sb16Vibra => {}
            // SB16 interrupt and DMA setup, a bit per line or channel.
            0x80 => {
                self.irq = [(0x01, 9), (0x02, 5), (0x04, 7), (0x08, 10)]
                    .into_iter()
                    .find(|&(bit, _)| value & bit != 0)
                    // The bus's IRQ 2 is the AT's IRQ 9; keep a configured 2.
                    .map(|(_, irq)| if irq == 9 && self.config.irq == 2 { 2 } else { irq });
            }
            0x81 => {
                self.dma8 = [(0x01, 0), (0x02, 1), (0x08, 3)].into_iter().find(|&(bit, _)| value & bit != 0).map(|(_, ch)| ch);
                self.dma16 = [(0x20, 5), (0x40, 6), (0x80, 7)].into_iter().find(|&(bit, _)| value & bit != 0).map(|(_, ch)| ch);
            }
            0x82 => {}
            _ => self.mixer[reg as usize] = value,
        }
    }

    fn mixer_read(&self) -> u8 {
        let reg = self.mixer_index;
        match (self.config.model, reg) {
            (SbModel::Sb2, _) => 0xFF,
            // Registers an SB Pro lacks, as DOSBox answers them: programs
            // that write and read back 3Fh or read 80h/81h take a card that
            // keeps them for an SB16.
            (_, 0x00) => 0x00,
            (m, _) if !m.is_sb16() && !PRO_REGS.contains(&reg) => 0x0A,
            (m, 0x80) if m.is_sb16() => match self.irq {
                Some(2 | 9) => 1,
                Some(5) => 2,
                Some(7) => 4,
                Some(10) => 8,
                _ => 0,
            },
            (m, 0x81) if m.is_sb16() => {
                let low = match self.dma8 {
                    Some(0) => 1,
                    Some(1) => 2,
                    Some(3) => 8,
                    _ => 0,
                };
                let high = match self.dma16 {
                    Some(5) => 0x20,
                    Some(6) => 0x40,
                    Some(7) => 0x80,
                    _ => 0,
                };
                low | high
            }
            (m, 0x82) if m.is_sb16() => (self.irq8 as u8) | (self.irq16 as u8) << 1 | 0x20,
            // Nor does the SB16's keep registers past its own, as DOSBox-X
            // answers them: Windows 98's SB16 driver writes and reads back
            // 8Ch-90h and FFh, and fails to start on a card that keeps them.
            (_, 0x30..=0x47) => self.mixer[reg as usize],
            _ if !PRO_REGS.contains(&reg) => 0x0A,
            // The SB Pro's volume registers read the top four of each
            // side's five bits in the SB16's, so on an SB16 a level set
            // through either reads back through both. An SB Pro can write
            // only the nibbles, and reads back what it wrote.
            (_, 0x04 | 0x22 | 0x26 | PRO_CD) => {
                let (l, r) = pro_volume_regs(reg);
                (self.mixer[l as usize] & 0xF0) | self.mixer[r as usize] >> 4
            }
            _ => self.mixer[reg as usize],
        }
    }

    /// SB Pro stereo output (mixer register 0Eh bit 1).
    fn sbpro_stereo(&self) -> bool {
        self.config.model != SbModel::Sb2 && self.mixer[0x0E] & 0x02 != 0
    }

    fn write_command(&mut self, value: u8, log: &mut Vec<String>) {
        if let Some(t) = &mut self.transfer
            && t.high_speed
        {
            // One line per transfer: a driver that keeps writing during a
            // long transfer would otherwise add one per byte.
            if !t.ignored_logged {
                t.ignored_logged = true;
                log.push(format!(
                    "[SB] DSP byte {:02X}h ignored: high-speed mode runs until a reset (later bytes ignored without a log line)",
                    value
                ));
            }
            return;
        }
        if let Some(cmd) = self.in_command {
            self.params.push(value);
            if self.params.len() >= self.params_needed {
                self.in_command = None;
                self.execute(cmd, log);
                self.params.clear();
            }
            return;
        }
        let sb16 = self.config.model.is_sb16();
        let needs = match value {
            // The SB16's ASP and 8051 commands (DOSBox-X's
            // DSP_cmd_len_sb16).
            0x04 | 0x08 | 0x0F | 0xF9 if sb16 => 1,
            0x05 | 0x0E | 0xFA if sb16 => 2,
            0x10 | 0x38 | 0x40 | 0xE0 | 0xE2 | 0xE4 => 1,
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
            Some(r) if self.config.model.is_sb16() => r * channels,
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
            high_speed: false,
            ignored_logged: false,
        });
    }

    fn execute(&mut self, cmd: u8, log: &mut Vec<String>) {
        let stereo8 = self.sbpro_stereo();
        // Commands only the SB16 takes parameters for arrive here without
        // them on older cards, which don't know them either.
        let sb16 = self.config.model.is_sb16();
        match cmd {
            // Direct DAC: one unsigned 8-bit sample.
            0x10 => {
                let level = (self.params[0] as i16 - 128) << 8;
                self.dac = (level, level);
            }
            // 8-bit single-cycle output; 16h/17h are 2-bit ADPCM, played
            // as plain 8-bit data.
            0x14 | 0x16 | 0x17 => {
                let len = self.param16(0) + 1;
                self.start(false, stereo8, false, false, false, len);
            }
            // 8-bit auto-init output (1Fh: ADPCM).
            0x1C | 0x1F => self.start(false, stereo8, false, true, false, self.block_size),
            // High-speed output (90h auto-init, 91h single-cycle) and input
            // (98h, 99h). The SB16 takes them as plain 8-bit transfers.
            0x90 | 0x91 | 0x98 | 0x99 => {
                let input = cmd >= 0x98;
                self.start(false, stereo8 && !input, false, cmd & 1 == 0, input, self.block_size);
                self.set_high_speed(true);
            }
            // Direct ADC: silence.
            0x20 => self.read_buf.push_back(0x80),
            // 8-bit input.
            0x24 => {
                let len = self.param16(0) + 1;
                self.start(false, false, false, false, true, len);
            }
            0x2C => self.start(false, false, false, true, true, self.block_size),
            // MIDI output byte (the SB's own UART): nothing attached.
            0x38 => {}
            0x40 => {
                let tc = self.params[0] as u32;
                self.tc_rate = 1_000_000 / (256 - tc).max(1);
                self.sb16_rate = None;
            }
            0x41 | 0x42 if sb16 => self.sb16_rate = Some(u16::from_be_bytes([self.params[0], self.params[1]]) as u32),
            0x48 => self.block_size = self.param16(0) + 1,
            // ADPCM single-cycle output, as 8-bit.
            0x74..=0x77 => {
                let len = self.param16(0) + 1;
                self.start(false, false, false, false, false, len);
            }
            0x7D | 0x7F => self.start(false, false, false, true, false, self.block_size),
            // Silence for n+1 samples, then an IRQ.
            0x80 => {
                self.silence = Some(self.param16(0) + 1);
                self.frac = 0;
            }
            // SB Pro input mode (mono / stereo): nothing to record.
            0xA0 | 0xA8 => {}
            // SB16: B0h-BFh 16-bit and C0h-CFh 8-bit transfers. Bit 3 input,
            // bit 2 auto-init; the mode byte has bit 4 signed, bit 5 stereo.
            0xB0..=0xCF if sb16 => {
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
            // Continue auto-init (SB16): take back an exit asked for with
            // DAh (8-bit) or D9h (16-bit) while the block still runs.
            0x45 | 0x47 if sb16 => {
                if let Some(t) = &mut self.transfer
                    && t.auto_init
                    && t.bits16 == (cmd == 0x47)
                {
                    t.last_block = false;
                }
            }
            0xE0 => self.read_buf.push_back(!self.params[0]),
            // DMA identification: a byte made from the parameter and the
            // calls before goes to memory on the 8-bit DMA channel.
            // Creative's CT-VOICE.DRV has the answers overwrite an entry
            // of its own function table, so only a card that answers like
            // Creative's leaves it a working play routine.
            0xE2 => {
                let add = &E2_ADD[(self.e2_count % 4) as usize];
                let mut value = self.e2_value as i16;
                for (bit, step) in add[..8].iter().enumerate() {
                    if self.params[0] >> bit & 1 != 0 {
                        value += step;
                    }
                }
                self.e2_value = (value + add[8]) as u8;
                self.e2_count = self.e2_count.wrapping_add(1);
                self.e2_pending = Some(self.e2_value);
            }
            0xE1 => {
                let (major, minor) = self.config.model.dsp_version();
                self.read_buf.push_back(major);
                self.read_buf.push_back(minor);
            }
            0xE3 => self.read_buf.extend(b"COPYRIGHT (C) CREATIVE TECHNOLOGY LTD, 1992.\0"),
            0xE4 => self.test_reg = self.params[0],
            0xE8 => self.read_buf.push_back(self.test_reg),
            // ESS's "identify chip": Creative's cards don't answer it.
            // Setup programs (HMI's) send it to tell an ESS AudioDrive apart.
            0xE7 => {}
            // Raise the 8-bit (F2h) or 16-bit (F3h) interrupt.
            0xF2 => self.irq8 = true,
            0xF3 if sb16 => self.irq16 = true,
            0xF8 => self.read_buf.push_back(0),
            // The SB16's ASP socket, empty, as Windows' driver probes it:
            // the mode (04h), a codec parameter (05h), the chip's version
            // (08h 03h: none, FFh as a card without one says) and its
            // registers (0Eh, 0Fh), which keep nothing written to them.
            // DOSBox-X's sblaster.cpp answers the same.
            0x04 if sb16 => self.asp_mode = self.params[0],
            0x04 => self.read_buf.push_back(if self.config.model == SbModel::Sb2 { 0x88 } else { 0x7B }),
            0x05 if sb16 => {}
            0x08 if sb16 => {
                if self.params[0] == 0x03 {
                    self.read_buf.push_back(0xFF);
                }
            }
            0x0E if sb16 => {}
            0x0F if sb16 => self.read_buf.push_back(self.asp_regs[self.params[0] as usize]),
            // The 8051's memory.
            0xF9 if sb16 => self.read_buf.push_back(self.mem8051[self.params[0] as usize]),
            0xFA if sb16 => self.mem8051[self.params[0] as usize] = self.params[1],
            _ => log.push(format!("[SB] Unhandled DSP command {:02X}h", cmd)),
        }
    }
}

// `high_speed` goes in a section of its own (`SBHS`, bus/state.rs), written
// only while the mode runs, so states from before it was kept still load.
rust_dos_savestate::state_fields!(Transfer { bits16, stereo, signed, auto_init, input, block, remaining, rate, paused, last_block } skip { high_speed, ignored_logged });
rust_dos_savestate::state_fields!(SoundBlaster {
    reset_stage, in_command, params, params_needed, read_buf, test_reg, speaker_on, tc_rate, sb16_rate,
    block_size, transfer, silence, irq8, irq16, last_ticks, frac, dac, out, frames, out_rate,
    pending_left, mixer_index, mixer, asp_mode, asp_regs, mem8051, irq, dma8, dma16,
} skip {
    // E2h's state isn't kept, so states saved before it load: the bus
    // starts its sequence over after a load (restart_dma_identification).
    config, e2_value, e2_count, e2_pending,
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_speed_mode_logs_one_ignored_byte_per_transfer() {
        let config = SbConfig { model: SbModel::SbPro2, ..SbConfig::default() };
        let mut sb = SoundBlaster::new(config, 0);
        let mut log = Vec::new();
        sb.write(0xC, 0x90, &mut log);
        for byte in [0xD0, 0xD3, 0xE1, 0xDA] {
            sb.write(0xC, byte, &mut log);
        }
        assert_eq!(log.len(), 1, "one line for the transfer: {:?}", log);
        assert!(log[0].contains("D0h"), "{:?}", log);
        // A reset ends the transfer; the next one logs its first ignored byte.
        sb.write(0x6, 1, &mut log);
        sb.write(0x6, 0, &mut log);
        sb.write(0xC, 0x90, &mut log);
        sb.write(0xC, 0xD0, &mut log);
        sb.write(0xC, 0xD0, &mut log);
        assert_eq!(log.len(), 2, "{:?}", log);
    }

    #[test]
    fn sb16_answers_windows_98s_probes_as_a_card_without_an_asp() {
        let mut sb = SoundBlaster::new(SbConfig::default(), 0);
        let mut log = Vec::new();
        let mut asp_reg = |sb: &mut SoundBlaster, reg: u8| {
            for byte in [0x0F, reg] {
                sb.write(0xC, byte, &mut log);
            }
            sb.read(0xA)
        };
        // SB16.VXD writes ASP register 05h, reads it back, and wants F8h in 09h.
        for byte in [0x0E, 0x05, 0x03] {
            sb.write(0xC, byte, &mut Vec::new());
        }
        assert_eq!(asp_reg(&mut sb, 0x05), 0x01, "an empty socket keeps nothing");
        assert_eq!(asp_reg(&mut sb, 0x09), 0xF8);
        // ... and its 8051's memory at 0Eh.
        for byte in [0xF9, 0x0E] {
            sb.write(0xC, byte, &mut Vec::new());
        }
        assert_eq!(sb.read(0xA), 0xFF);
        // Mixer registers the CT1745 lacks read 0Ah, whatever was written.
        for (reg, kept) in [(0x90, false), (0xFF, false), (0x30, true)] {
            sb.write(0x4, reg, &mut Vec::new());
            sb.write(0x5, 0x18, &mut Vec::new());
            assert_eq!(sb.read(0x5) == 0x18, kept, "mixer register {:02X}h", reg);
        }
    }

    #[test]
    fn busy_slots_are_short_and_do_not_alias_with_a_poll_period() {
        let busy = (0..16_000u64).filter(|&s| busy_slot(s)).count();
        assert!((2_400..=3_600).contains(&busy), "busy in {} of 16000 slots", busy);
        assert!((0..16_000u64).all(|s| !(busy_slot(s) && busy_slot(s + 1))), "never busy two slots in a row");
        // A loop polling every `step` slots, from any phase, sees both
        // states within a few dozen reads.
        for step in 1..=64u64 {
            for start in 0..256u64 {
                let reads: Vec<bool> = (0..64).map(|k| busy_slot(start + k * step)).collect();
                assert!(reads.contains(&true) && reads.contains(&false), "step {} from {}", step, start);
            }
        }
    }
}
