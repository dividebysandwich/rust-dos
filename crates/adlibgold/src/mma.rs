//! The Yamaha YMZ263 (MMA) on the AdLib Gold: two channels of 12-bit
//! digital sound, each with a 128-byte FIFO that the CPU or a DMA channel
//! fills, and three timers. Its register map and behaviour are the AdLib
//! Gold SDTK's ("Digital Audio and MIDI").
//!
//! The chip sits at base + 4 to base + 7: a register number goes to
//! base + 4 (or + 6), and the register's value to base + 5 for channel 0
//! or base + 7 for channel 1. Reading base + 4 (or + 6) gives the status:
//! OV (7), the timers T2-T0 (6-4), the MIDI flags TRQ and RRQ (3-2) and
//! the FIFO flags FIF1 and FIF0 (1-0).
//!
//! Channel registers:
//!
//! - 09h, playback and recording: RST (7) resets the channel and empties
//!   its FIFO, R and L (6, 5) put it on the right and left outputs, FREQ
//!   (4-3) is its rate, PCM (2) chooses PCM over ADPCM, P/R (1) playback
//!   over recording, and GO (0) starts it.
//! - 0Ah, its output volume, 0 the quietest and FFh the loudest.
//! - 0Bh, the FIFO: a byte written goes in.
//! - 0Ch, its format: ILV (7, channel 0 only) interleaves both channels
//!   on channel 0's DMA and settings, DATA FMT (6-5) is the PCM format,
//!   FIFO INT (4-2) the FIFO level that raises the FIFO flag (112 bytes
//!   down to 16 in steps of 16), MSK (1) masks the flag's interrupt, and
//!   ENB (0) has the DMA fill the FIFO.
//!
//! Channel 0 alone has the timers (02h-08h) and MIDI (0Dh-0Eh).
//!
//! The chip runs on 16.9344 MHz / 32, a tick of 1.89 us: PCM plays at 44.1,
//! 22.05, 11.025 or 7.35 kHz (12, 24, 48 or 72 ticks a sample) and ADPCM
//! at half those down to 5.5125 kHz. Every rate is a whole number of the
//! mixer's 44.1 kHz frames, so the channels advance a mixer frame at a
//! time, and the DAC's level each frame is what the mixer plays.
//!
//! Recording and MIDI are not emulated: a channel set to record takes in
//! silence and gives the DMA nothing, and MIDI bytes are dropped.

use rust_dos_audio_core::{PIT_HZ, RATE, mul_div, mul_div_ceil};
use rust_dos_dma::Dma;
use std::collections::VecDeque;

/// The chip's clock: 16.9344 MHz / 32.
pub const CLOCK_HZ: u64 = 529_200;
/// The chip's ticks in a mixer frame.
const TICKS_PER_FRAME: u64 = CLOCK_HZ / RATE as u64;
/// A FIFO's size.
pub const FIFO: usize = 128;

pub const STATUS_FIF0: u8 = 0x01;
pub const STATUS_FIF1: u8 = 0x02;
pub const STATUS_T0: u8 = 0x10;
pub const STATUS_T1: u8 = 0x20;
pub const STATUS_T2: u8 = 0x40;
pub const STATUS_OV: u8 = 0x80;

const R9_GO: u8 = 0x01;
const R9_PLAY: u8 = 0x02;
const R9_PCM: u8 = 0x04;
const R9_LEFT: u8 = 0x20;
const R9_RIGHT: u8 = 0x40;
const R9_RST: u8 = 0x80;
const RC_ENB: u8 = 0x01;
const RC_MSK: u8 = 0x02;
const RC_ILV: u8 = 0x80;

/// A channel's sample period in mixer frames, by register 9.
fn period(r9: u8) -> u32 {
    let n = (r9 >> 3 & 3) as u32;
    let ticks = if r9 & R9_PCM != 0 {
        [12, 24, 48, 72][n as usize]
    } else {
        [24, 48, 72, 96][n as usize]
    };
    ticks / TICKS_PER_FRAME as u32
}

/// The FIFO level of register 0Ch's FIFO INT: the flag rises as the FIFO
/// falls below it in playback.
fn fifo_level(rc: u8) -> usize {
    (7 - (rc >> 2 & 7) as usize) * 16
}

/// The bytes of one PCM sample in format `rc`.
fn sample_bytes(rc: u8) -> usize {
    match rc >> 5 & 3 {
        1 | 2 => 2,
        _ => 1,
    }
}

/// The 16-bit level of a PCM sample in format `rc` (the DAC's 12 bits at
/// the top).
fn decode_pcm(rc: u8, b0: u8, b1: u8) -> i16 {
    match rc >> 5 & 3 {
        // The 8 LSBs, then the 4 MSBs (the sign repeated above them).
        1 => (((b1 as u16) << 8 | b0 as u16) << 4) as i16,
        // The 4 LSBs in the high nibble, then the 8 MSBs.
        2 => ((b1 as u16) << 8 | (b0 & 0xF0) as u16) as i16,
        // The 8 MSBs (3 is invalid; the chip takes it as 0).
        _ => ((b0 as i8) as i16) << 8,
    }
}

/// Yamaha's 4-bit ADPCM, the high nibble of a byte first.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Adpcm {
    predictor: i32,
    step: i32,
}

impl Adpcm {
    const STEP_MIN: i32 = 127;

    fn reset(&mut self) {
        *self = Self {
            predictor: 0,
            step: Self::STEP_MIN,
        };
    }

    fn decode(&mut self, code: u8) -> i16 {
        const ADJUST: [i32; 8] = [57, 57, 57, 57, 77, 102, 128, 153];
        let magnitude = (code & 7) as i32;
        let delta = ((magnitude * 2 + 1) * self.step) >> 3;
        self.predictor = if code & 8 != 0 {
            self.predictor - delta
        } else {
            self.predictor + delta
        }
        .clamp(-32768, 32767);
        self.step = ((self.step * ADJUST[magnitude as usize]) >> 6).clamp(Self::STEP_MIN, 24576);
        self.predictor as i16
    }
}

rust_dos_savestate::state_fields!(Adpcm { predictor, step });

#[derive(Clone, Debug, Default)]
pub struct Channel {
    regs: [u8; 16],
    fifo: VecDeque<u8>,
    /// Frames to the next sample while it plays.
    countdown: u32,
    /// The DAC's level.
    level: i16,
    adpcm: Adpcm,
    /// The second ADPCM code of the byte at the FIFO's head is next.
    low_nibble: bool,
}

rust_dos_savestate::state_fields!(Channel {
    regs,
    fifo,
    countdown,
    level,
    adpcm,
    low_nibble
});

impl Channel {
    /// The bytes waiting in its FIFO.
    pub fn fifo_len(&self) -> usize {
        self.fifo.len()
    }

    fn playing(&self) -> bool {
        self.regs[9] & (R9_GO | R9_PLAY | R9_RST) == R9_GO | R9_PLAY
    }

    fn reset(&mut self) {
        self.fifo.clear();
        self.level = 0;
        self.adpcm.reset();
        self.low_nibble = false;
    }

    /// The bytes the FIFO must hold for the next sample, with the settings
    /// of register 9 `r9` (channel 0's when interleaved).
    fn need(&self, r9: u8) -> usize {
        if r9 & R9_PCM == 0 {
            1
        } else {
            sample_bytes(self.regs[0xC])
        }
    }

    /// Play the next sample: false if the FIFO hasn't got it.
    fn take_sample(&mut self, r9: u8) -> bool {
        let need = self.need(r9);
        if self.fifo.len() < need {
            return false;
        }
        if r9 & R9_PCM != 0 {
            let b0 = self.fifo.pop_front().unwrap_or(0);
            let b1 = if need == 2 {
                self.fifo.pop_front().unwrap_or(0)
            } else {
                0
            };
            self.level = decode_pcm(self.regs[0xC], b0, b1);
        } else {
            let byte = self.fifo[0];
            let code = if self.low_nibble {
                byte & 0xF
            } else {
                byte >> 4
            };
            self.level = self.adpcm.decode(code);
            if self.low_nibble {
                self.fifo.pop_front();
            }
            self.low_nibble = !self.low_nibble;
        }
        true
    }
}

/// A timer: running from tick `next - period`, next due at tick `next`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Timer {
    running: bool,
    next: u64,
    period: u64,
}

rust_dos_savestate::state_fields!(Timer {
    running,
    next,
    period
});

impl Timer {
    fn start(&mut self, now: u64, period: u64) {
        *self = Timer {
            running: true,
            next: now + period.max(1),
            period: period.max(1),
        };
    }

    /// Whether it ran out since the last call, up to tick `now`.
    fn expired(&mut self, now: u64) -> bool {
        if !self.running || self.next > now {
            return false;
        }
        self.next += ((now - self.next) / self.period + 1) * self.period;
        true
    }
}

/// Where a channel's DMA comes from: the control chip's DEN and DMA SEL
/// bits for it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DmaLine {
    pub enabled: bool,
    pub channel: u8,
}

#[derive(Clone, Debug, Default)]
pub struct Mma {
    index: u8,
    pub channels: [Channel; 2],
    status: u8,
    /// The timers' reload values and the counters (0, the base counter,
    /// 1 and 2).
    timer0_latch: u16,
    base_latch: u16,
    timer1_latch: u8,
    timer2_latch: u16,
    timers: [Timer; 4],
    /// Timer 2's count as latched by a read of its low byte.
    timer2_read: u16,
    /// Frames made since power-on, in the mixer's numbering (frame n
    /// starts at the first tick t with t * RATE >= n * PIT_HZ).
    frames: u64,
    /// The interleaved DMA's next unit goes to channel 1.
    ilv_next: u8,
    /// The DAC levels of the frames made, as the channels' volumes and
    /// outputs put them on the left and right, for the mixer.
    pub out: VecDeque<(i16, i16)>,
}

rust_dos_savestate::state_fields!(Mma {
    index, channels, status, timer0_latch, base_latch, timer1_latch, timer2_latch, timers, timer2_read,
    frames, ilv_next
} skip { out });

/// Frames of output kept for a mixer that doesn't come for them.
const OUT_MAX: usize = RATE as usize;

impl Mma {
    pub fn new(now: u64) -> Self {
        let mut mma = Self {
            frames: mul_div(now, RATE as u64, PIT_HZ),
            ..Self::default()
        };
        for ch in &mut mma.channels {
            ch.reset();
        }
        mma
    }

    fn ticks(now: u64) -> u64 {
        mul_div(now, CLOCK_HZ, PIT_HZ)
    }

    /// The status, as a read of base + 4 gives it: the read clears the
    /// timer, overrun and FIFO flags, which latch until it.
    pub fn read_status(&mut self) -> u8 {
        let status = self.status;
        self.status &= !(STATUS_OV | STATUS_T0 | STATUS_T1 | STATUS_T2 | STATUS_FIF0 | STATUS_FIF1);
        status
    }

    pub fn status(&self) -> u8 {
        self.status
    }

    pub fn write_index(&mut self, value: u8) {
        self.index = value;
    }

    /// Whether the chip asks for an interrupt: a flag up whose mask is
    /// clear.
    pub fn irq_pending(&self) -> bool {
        let r = &self.channels[0].regs;
        let s = self.status;
        (s & STATUS_FIF0 != 0 && r[0xC] & RC_MSK == 0)
            || (s & STATUS_FIF1 != 0 && self.channels[1].regs[0xC] & RC_MSK == 0)
            || (s & STATUS_T0 != 0 && r[8] & 0x10 == 0)
            || (s & STATUS_T1 != 0 && r[8] & 0x20 == 0)
            || (s & STATUS_T2 != 0 && r[8] & 0x40 == 0)
            || (s & STATUS_OV != 0 && r[0xD] & 0x20 == 0)
    }

    /// A write to channel `ch`'s data port, at tick `now`.
    pub fn write_data(
        &mut self,
        ch: usize,
        value: u8,
        now: u64,
        dma: &mut Dma,
        ram: &[u8],
        lines: [DmaLine; 2],
    ) {
        self.advance(now, dma, ram, lines);
        let reg = self.index as usize;
        if reg >= 0xF {
            return;
        }
        let t = Self::ticks(now);
        if ch == 0 {
            match reg {
                // The timers' reload values take effect at the next start.
                2 => self.timer0_latch = self.timer0_latch & 0xFF00 | value as u16,
                3 => self.timer0_latch = self.timer0_latch & 0xFF | (value as u16) << 8,
                4 => self.base_latch = self.base_latch & 0xF00 | value as u16,
                5 => {
                    self.base_latch = self.base_latch & 0xFF | ((value & 0xF) as u16) << 8;
                    self.timer1_latch = value >> 4;
                }
                6 => self.timer2_latch = self.timer2_latch & 0xFF00 | value as u16,
                7 => self.timer2_latch = self.timer2_latch & 0xFF | (value as u16) << 8,
                8 => self.write_timer_control(value, t),
                _ => {}
            }
        }
        let old = self.channels[ch].regs[reg];
        self.channels[ch].regs[reg] = value;
        match reg {
            9 => {
                let c = &mut self.channels[ch];
                if value & R9_RST != 0 {
                    c.reset();
                    self.status &= !(1 << ch);
                } else if value & (R9_GO | R9_PLAY) == R9_GO | R9_PLAY
                    && old & (R9_GO | R9_PLAY) != R9_GO | R9_PLAY
                {
                    c.countdown = period(value);
                }
                if value & (R9_PCM | R9_PLAY) != old & (R9_PCM | R9_PLAY) {
                    c.adpcm.reset();
                    c.low_nibble = false;
                }
            }
            0xB => {
                let c = &mut self.channels[ch];
                if c.fifo.len() < FIFO {
                    c.fifo.push_back(value);
                }
            }
            _ => {}
        }
        self.fill(dma, ram, lines);
    }

    fn write_timer_control(&mut self, value: u8, t: u64) {
        let old = self.channels[0].regs[8];
        let base_period = if self.base_latch == 0 {
            4096
        } else {
            self.base_latch as u64
        };
        let rising = value & !old;
        if rising & 1 != 0 {
            let p = if self.timer0_latch == 0 {
                65536
            } else {
                self.timer0_latch as u64
            };
            self.timers[0].start(t, p);
        }
        if rising & 8 != 0 {
            self.timers[1].start(t, base_period);
        }
        if rising & 2 != 0 {
            let n = if self.timer1_latch == 0 {
                16
            } else {
                self.timer1_latch as u64
            };
            self.timers[2].start(t, n * base_period);
        }
        if rising & 4 != 0 {
            let n = if self.timer2_latch == 0 {
                65536
            } else {
                self.timer2_latch as u64
            };
            self.timers[3].start(t, n * base_period);
        }
        for (bit, timer) in [(1, 0), (8, 1), (2, 2), (4, 3)] {
            if value & bit == 0 {
                self.timers[timer].running = false;
            }
        }
    }

    /// A read of channel `ch`'s data port.
    pub fn read_data(
        &mut self,
        ch: usize,
        now: u64,
        dma: &mut Dma,
        ram: &[u8],
        lines: [DmaLine; 2],
    ) -> u8 {
        self.advance(now, dma, ram, lines);
        let reg = self.index as usize;
        if reg >= 0xF {
            return 0xFF;
        }
        if ch == 0 && reg == 6 {
            // The low byte latches the count, the high byte reads the
            // latch.
            let t = &self.timers[3];
            let base = if self.base_latch == 0 {
                4096
            } else {
                self.base_latch as u64
            };
            self.timer2_read = if t.running {
                (t.next - Self::ticks(now)).div_ceil(base) as u16
            } else {
                self.timer2_latch
            };
            return self.timer2_read as u8;
        }
        if ch == 0 && reg == 7 {
            return (self.timer2_read >> 8) as u8;
        }
        // Register 0Bh reads back the last byte through it: the SDTK's
        // detection writes 5Ah and reads it back.
        self.channels[ch].regs[reg]
    }

    /// The DMA channel feeding MMA channel `ch`, if its DMA is on.
    fn dma_channel(&self, ch: usize, lines: [DmaLine; 2]) -> Option<usize> {
        let line = lines[ch];
        (self.channels[ch].regs[0xC] & RC_ENB != 0 && line.enabled).then_some(line.channel as usize)
    }

    fn interleaved(&self) -> bool {
        self.channels[0].regs[0xC] & RC_ILV != 0
    }

    /// Let the DMA fill the FIFOs of the channels that play from it.
    fn fill(&mut self, dma: &mut Dma, ram: &[u8], lines: [DmaLine; 2]) {
        if self.interleaved() {
            // Both channels on channel 0's DMA, a sample (or an ADPCM
            // byte) each in turn, channel 0 first; both need ENB.
            let Some(d) = self.dma_channel(0, lines) else {
                return;
            };
            if self.channels[1].regs[0xC] & RC_ENB == 0 || self.channels[0].regs[9] & R9_PLAY == 0 {
                return;
            }
            let r9 = self.channels[0].regs[9];
            loop {
                let c = &self.channels[self.ilv_next as usize];
                let unit = c.need(r9);
                if c.fifo.len() + unit > FIFO || !dma.ready(d) {
                    break;
                }
                let mut buf = [0u8; 2];
                let (moved, _) = dma.transfer_read(d, ram, &mut buf[..unit]);
                let c = &mut self.channels[self.ilv_next as usize];
                c.fifo.extend(&buf[..moved]);
                if moved < unit {
                    break;
                }
                self.ilv_next ^= 1;
            }
            return;
        }
        for ch in 0..2 {
            let Some(d) = self.dma_channel(ch, lines) else {
                continue;
            };
            let c = &mut self.channels[ch];
            if c.regs[9] & R9_PLAY == 0 || !dma.ready(d) {
                continue;
            }
            let room = FIFO - c.fifo.len();
            if room == 0 {
                continue;
            }
            let mut buf = [0u8; FIFO];
            let (moved, _) = dma.transfer_read(d, ram, &mut buf[..room]);
            c.fifo.extend(&buf[..moved]);
        }
    }

    /// Run the chip up to tick `now`.
    pub fn advance(&mut self, now: u64, dma: &mut Dma, ram: &[u8], lines: [DmaLine; 2]) {
        let end = mul_div(now, RATE as u64, PIT_HZ);
        while self.frames < end {
            self.frames += 1;
            self.frame(dma, ram, lines);
        }
        if self.out.len() > OUT_MAX {
            let extra = self.out.len() - OUT_MAX;
            self.out.drain(..extra);
        }
        self.run_timers(Self::ticks(now));
    }

    fn run_timers(&mut self, t: u64) {
        if self.timers[0].expired(t) {
            self.status |= STATUS_T0;
        }
        // Timers 1 and 2 count the base counter's periods: with it
        // stopped they stand still.
        let base = self.timers[1].running;
        self.timers[1].expired(t);
        for (n, flag) in [(2, STATUS_T1), (3, STATUS_T2)] {
            if !base {
                if self.timers[n].running {
                    self.timers[n].next = self.timers[n].next.max(t + 1);
                }
                continue;
            }
            if self.timers[n].expired(t) {
                self.status |= flag;
            }
        }
    }

    /// One mixer frame: the channels whose sample comes due play it, the
    /// DMA tops their FIFOs up, and the frame's levels go out.
    fn frame(&mut self, dma: &mut Dma, ram: &[u8], lines: [DmaLine; 2]) {
        let ilv = self.interleaved() && self.channels[0].playing();
        let r9_0 = self.channels[0].regs[9];
        for ch in 0..2 {
            // Interleaved, channel 1 follows channel 0's P/R, FREQ and GO.
            let (playing, r9) = if ilv {
                (true, r9_0)
            } else {
                (self.channels[ch].playing(), self.channels[ch].regs[9])
            };
            if !playing {
                continue;
            }
            let c = &mut self.channels[ch];
            c.countdown = c.countdown.saturating_sub(1);
            if c.countdown > 0 {
                continue;
            }
            c.countdown = period(r9);
            if !c.take_sample(r9) {
                self.status |= STATUS_OV;
            }
            self.fill(dma, ram, lines);
            let c = &self.channels[ch];
            if c.fifo.len() < fifo_level(c.regs[0xC]) {
                self.status |= 1 << ch;
            }
        }
        let mut out = (0i32, 0i32);
        for c in &self.channels {
            let r9 = c.regs[9];
            let v = c.level as i32 * c.regs[0xA] as i32 / 255;
            if r9 & R9_LEFT != 0 {
                out.0 += v;
            }
            if r9 & R9_RIGHT != 0 {
                out.1 += v;
            }
        }
        self.out.push_back((
            out.0.clamp(-32768, 32767) as i16,
            out.1.clamp(-32768, 32767) as i16,
        ));
    }

    /// Whether the chip plays nothing and holds both DACs at 0.
    pub fn is_idle(&self) -> bool {
        self.channels.iter().all(|c| !c.playing() && c.level == 0)
    }

    /// When the chip's interrupt flags may next rise (in ticks), for the
    /// machine's scheduler: a FIFO falling below its level, or a timer.
    pub fn next_event(&self, dma: &Dma, lines: [DmaLine; 2]) -> Option<u64> {
        let mut next: Option<u64> = None;
        let mut at = |t: u64| next = Some(next.map_or(t, |n| n.min(t)));
        let r = &self.channels[0].regs;
        let tick = |t: u64| mul_div_ceil(t, PIT_HZ, CLOCK_HZ);
        if r[8] & 0x10 == 0 && self.timers[0].running {
            at(tick(self.timers[0].next));
        }
        if self.timers[1].running {
            for (n, mask) in [(2, 0x20), (3, 0x40)] {
                if r[8] & mask == 0 && self.timers[n].running {
                    at(tick(self.timers[n].next));
                }
            }
        }
        let ilv = self.interleaved() && self.channels[0].playing();
        for ch in 0..2 {
            let c = &self.channels[ch];
            let r9 = if ilv {
                self.channels[0].regs[9]
            } else {
                c.regs[9]
            };
            if !(ilv || c.playing()) || c.regs[0xC] & RC_MSK != 0 || self.status & (1 << ch) != 0 {
                continue;
            }
            // The bytes it has to play: its FIFO, and what the DMA has
            // left for it (none of its own when interleaved: the flag
            // rises once the shared DMA ends, close enough).
            let d = self
                .dma_channel(if ilv { 0 } else { ch }, lines)
                .filter(|&d| dma.ready(d));
            let more = match d {
                Some(d) if dma.channel(d).auto_init() => continue,
                Some(d) => dma.channel(d).cur_count as usize + 1,
                None => 0,
            };
            let need = c.need(r9).max(1);
            let total = c.fifo.len() + if ilv { more / 2 } else { more };
            let level = fifo_level(c.regs[0xC]);
            let samples = if total < level {
                1
            } else {
                (total - level) / need + 1
            };
            let frames =
                self.frames + c.countdown.max(1) as u64 + (samples as u64 - 1) * period(r9) as u64;
            at(mul_div_ceil(frames, PIT_HZ, RATE as u64));
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_formats_follow_the_sdtk() {
        assert_eq!(decode_pcm(0x00, 0x7F, 0), 0x7F00);
        assert_eq!(decode_pcm(0x00, 0x80, 0), -0x8000);
        // Format 1: 8 LSBs, then the 4 MSBs with the sign above them.
        assert_eq!(decode_pcm(0x20, 0xFF, 0x07), 0x7FF0);
        assert_eq!(decode_pcm(0x20, 0x00, 0xF8), -0x8000);
        assert_eq!(decode_pcm(0x20, 0x01, 0x00), 0x10);
        // Format 2: 4 LSBs at the top of the first byte, then 8 MSBs.
        assert_eq!(decode_pcm(0x40, 0xF0, 0x7F), 0x7FF0);
        assert_eq!(decode_pcm(0x40, 0x10, 0x00), 0x10);
        assert_eq!(sample_bytes(0x20), 2);
        assert_eq!(sample_bytes(0x00), 1);
    }

    #[test]
    fn rates_and_levels() {
        // PCM 44.1 kHz is every frame, 11.025 kHz every 4th; ADPCM at
        // FREQ 3 is 5.5125 kHz.
        assert_eq!(period(R9_PCM), 1);
        assert_eq!(period(R9_PCM | 0x10), 4);
        assert_eq!(period(0x18), 8);
        assert_eq!(fifo_level(0), 112);
        assert_eq!(fifo_level(6 << 2), 16);
    }

    /// The ADPCM decoder rises by the step and adapts it.
    #[test]
    fn adpcm_steps() {
        let mut a = Adpcm::default();
        a.reset();
        assert_eq!(a.decode(0x7), (15 * 127) >> 3);
        assert_eq!(a.step, (127 * 153) >> 6);
        let before = a.predictor;
        a.decode(0x8);
        assert!(a.predictor < before);
        assert_eq!(a.step, Adpcm::STEP_MIN.max(((127 * 153) >> 6) * 57 >> 6));
    }
}
