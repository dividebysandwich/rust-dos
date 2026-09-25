//! One of the GF1's 32 voices: a position moving through DRAM at the
//! frequency control's rate, looping or stopping at its end points, and a
//! volume ramping between two levels.

use super::tables::{self, RAMP_FRAC, VOLUME_MAX};

/// Fraction bits of an address: positions are 20.9 fixed point.
pub const WAVE_FRAC: u32 = 9;
/// Addresses wrap at the 1 MB of DRAM.
const POS_MASK: u32 = (1 << (20 + WAVE_FRAC)) - 1;
const DRAM_MASK: u32 = 0xF_FFFF;

// Voice control and volume control bits.
pub const STOPPED: u8 = 0x01;
pub const STOP: u8 = 0x02;
pub const BITS16: u8 = 0x04;
pub const LOOP: u8 = 0x08;
pub const BIDIRECTIONAL: u8 = 0x10;
pub const IRQ_ENABLE: u8 = 0x20;
pub const DECREASING: u8 = 0x40;
/// Volume control bit 2: the voice passes its end address, raising its
/// IRQ, without looping or stopping ("rollover").
pub const ROLLOVER: u8 = 0x04;

/// Events of a frame.
pub const WAVE_IRQ: u8 = 1;
pub const RAMP_IRQ: u8 = 2;

#[derive(Clone, Copy, Debug)]
pub struct Voice {
    /// Voice control (00h), without the IRQ pending bit.
    pub wave_ctrl: u8,
    /// Frequency control (01h): bits 15-1 are the position increment in
    /// 1/512 samples per frame.
    pub freq: u16,
    pub start: u32,
    pub end: u32,
    pub pos: u32,
    /// Volume control (0Dh), without the IRQ pending bit.
    pub ramp_ctrl: u8,
    pub ramp_rate: u8,
    pub ramp_start: u8,
    pub ramp_end: u8,
    /// Current volume, 12 bits with `RAMP_FRAC` fraction bits.
    pub vol: u32,
    pub pan: u8,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            wave_ctrl: STOPPED,
            freq: 0,
            start: 0,
            end: 0,
            pos: 0,
            ramp_ctrl: STOPPED,
            ramp_rate: 0,
            ramp_start: 0,
            ramp_end: 0,
            vol: 0,
            pan: 7,
        }
    }
}

impl Voice {
    fn inc(&self) -> u32 {
        (self.freq >> 1) as u32
    }

    fn ramp_inc(&self) -> u32 {
        tables::ramp_increment(self.ramp_rate)
    }

    fn ramp_bounds(&self) -> (u32, u32) {
        ((self.ramp_start as u32) << (4 + RAMP_FRAC), (self.ramp_end as u32) << (4 + RAMP_FRAC))
    }

    pub fn wave_running(&self) -> bool {
        self.wave_ctrl & (STOPPED | STOP) == 0
    }

    pub fn ramp_running(&self) -> bool {
        self.ramp_ctrl & (STOPPED | STOP) == 0
    }

    /// Whether the voice contributes nothing: wave and ramp both stopped.
    pub fn silent(&self) -> bool {
        !self.wave_running() && !self.ramp_running()
    }

    /// The sample at the current position, interpolated between the two
    /// samples around it.
    #[inline]
    pub fn sample(&self, dram: &[u8]) -> f32 {
        let addr = self.pos >> WAVE_FRAC;
        let frac = (self.pos & ((1 << WAVE_FRAC) - 1)) as f32 / (1 << WAVE_FRAC) as f32;
        let (a, b) = if self.wave_ctrl & BITS16 != 0 {
            (sample16(dram, addr), sample16(dram, (addr + 1) & DRAM_MASK))
        } else {
            let a = dram[addr as usize] as i8 as f32 * 256.0;
            let b = dram[((addr + 1) & DRAM_MASK) as usize] as i8 as f32 * 256.0;
            (a, b)
        };
        a + (b - a) * frac
    }

    /// Advance one frame: the position, then the volume. Returns the IRQs
    /// the voice raised (`WAVE_IRQ`, `RAMP_IRQ`).
    #[inline]
    pub fn step(&mut self) -> u8 {
        let mut events = 0;
        if self.wave_running() && self.step_wave() {
            events |= WAVE_IRQ;
        }
        if self.ramp_running() && self.step_ramp() {
            events |= RAMP_IRQ;
        }
        events
    }

    fn step_wave(&mut self) -> bool {
        let inc = self.inc() as i64;
        let (pos, start, end) = (self.pos as i64, self.start as i64, self.end as i64);
        let decreasing = self.wave_ctrl & DECREASING != 0;
        let (next, left, crossed) = if decreasing {
            let next = pos - inc;
            (next, start - next, pos > start)
        } else {
            let next = pos + inc;
            (next, next - end, pos < end)
        };
        if left < 0 {
            self.pos = next as u32 & POS_MASK;
            return false;
        }
        let irq = self.wave_ctrl & IRQ_ENABLE != 0;
        if self.ramp_ctrl & ROLLOVER != 0 {
            // Play on past the end, raising the IRQ when crossing it.
            self.pos = next.rem_euclid(POS_MASK as i64 + 1) as u32;
            return irq && crossed;
        }
        if self.wave_ctrl & LOOP != 0 {
            if self.wave_ctrl & BIDIRECTIONAL != 0 {
                self.wave_ctrl ^= DECREASING;
            }
            let looped = if self.wave_ctrl & DECREASING != 0 { end - left } else { start + left };
            self.pos = looped.rem_euclid(POS_MASK as i64 + 1) as u32;
        } else {
            self.wave_ctrl |= STOPPED;
            self.pos = if decreasing { self.start } else { self.end };
        }
        irq
    }

    fn step_ramp(&mut self) -> bool {
        let inc = self.ramp_inc() as i64;
        let (start, end) = self.ramp_bounds();
        let (start, end) = (start as i64, end as i64);
        let vol = self.vol as i64;
        let decreasing = self.ramp_ctrl & DECREASING != 0;
        let (next, left) = if decreasing {
            let next = vol - inc;
            (next, start - next)
        } else {
            let next = vol + inc;
            (next, next - end)
        };
        let max = ((VOLUME_MAX << RAMP_FRAC) | ((1 << RAMP_FRAC) - 1)) as i64;
        if left < 0 {
            self.vol = next.clamp(0, max) as u32;
            return false;
        }
        let irq = self.ramp_ctrl & IRQ_ENABLE != 0;
        let v = if self.ramp_ctrl & LOOP != 0 {
            if self.ramp_ctrl & BIDIRECTIONAL != 0 {
                self.ramp_ctrl ^= DECREASING;
            }
            if self.ramp_ctrl & DECREASING != 0 { end - left } else { start + left }
        } else {
            self.ramp_ctrl |= STOPPED;
            if decreasing { start } else { end }
        };
        self.vol = v.clamp(0, max) as u32;
        irq
    }

    /// Frames until the voice next raises an IRQ, if it will on its own.
    pub fn frames_to_irq(&self) -> Option<u64> {
        let mut best: Option<u64> = None;
        let mut consider = |k: u64| best = Some(best.map_or(k, |b| b.min(k)));
        if self.wave_running() && self.wave_ctrl & IRQ_ENABLE != 0 && self.inc() > 0 {
            let inc = self.inc() as u64;
            let distance = if self.wave_ctrl & DECREASING != 0 {
                (self.pos > self.start).then(|| (self.pos - self.start) as u64)
            } else {
                (self.pos < self.end).then(|| (self.end - self.pos) as u64)
            };
            match distance {
                Some(d) => consider(d.div_ceil(inc)),
                // Already at or past the boundary: the next frame loops or
                // stops, unless the voice rolls over, which never comes
                // back to it.
                None if self.ramp_ctrl & ROLLOVER == 0 => consider(1),
                None => {}
            }
        }
        if self.ramp_running() && self.ramp_ctrl & IRQ_ENABLE != 0 {
            let inc = self.ramp_inc() as u64;
            let (start, end) = self.ramp_bounds();
            let distance = if self.ramp_ctrl & DECREASING != 0 {
                self.vol.saturating_sub(start) as u64
            } else {
                end.saturating_sub(self.vol) as u64
            };
            if distance == 0 {
                consider(1);
            } else if inc > 0 {
                consider(distance.div_ceil(inc));
            }
        }
        best
    }

    /// Output gains of the voice: volume and pan.
    #[inline]
    pub fn gains(&self) -> (f32, f32) {
        let v = tables::volumes()[(self.vol >> RAMP_FRAC) as usize & 0xFFF];
        let (l, r) = tables::pans()[self.pan as usize & 0xF];
        (v * l, v * r)
    }
}

/// A 16-bit sample: 16-bit voices address words, keeping the 256K bank
/// (bits 18-19) of the address.
#[inline]
fn sample16(dram: &[u8], addr: u32) -> f32 {
    let byte = ((addr & 0xC_0000) | ((addr & 0x1_FFFF) << 1)) as usize;
    i16::from_le_bytes([dram[byte], dram[(byte + 1) & DRAM_MASK as usize]]) as f32
}

crate::state_fields!(Voice { wave_ctrl, freq, start, end, pos, ramp_ctrl, ramp_rate, ramp_start, ramp_end, vol, pan });


#[cfg(test)]
mod tests {
    use super::*;

    fn voice(start: u32, end: u32, freq: u16, ctrl: u8) -> Voice {
        Voice {
            wave_ctrl: ctrl,
            freq,
            start: start << WAVE_FRAC,
            end: end << WAVE_FRAC,
            pos: start << WAVE_FRAC,
            ..Voice::default()
        }
    }

    #[test]
    fn forward_loop_carries_the_overshoot() {
        // 3 samples a frame, looping 10..20.
        let mut v = voice(10, 20, 3 << 10, LOOP);
        for _ in 0..4 {
            v.step();
        }
        assert_eq!(v.pos >> WAVE_FRAC, 12);
    }

    #[test]
    fn one_shot_stops_at_the_end() {
        let mut v = voice(0, 5, 2 << 10, IRQ_ENABLE);
        assert_eq!(v.frames_to_irq(), Some(3));
        assert_eq!(v.step(), 0);
        assert_eq!(v.step(), 0);
        assert_eq!(v.step(), WAVE_IRQ);
        assert!(!v.wave_running());
        assert_eq!(v.pos >> WAVE_FRAC, 5);
    }

    #[test]
    fn bidirectional_loop_turns_around() {
        let mut v = voice(0, 4, 1 << 10, LOOP | BIDIRECTIONAL);
        let mut path = Vec::new();
        for _ in 0..8 {
            v.step();
            path.push(v.pos >> WAVE_FRAC);
        }
        assert_eq!(path, [1, 2, 3, 4, 3, 2, 1, 0]);
    }

    #[test]
    fn rollover_raises_once_and_plays_on() {
        let mut v = voice(0, 2, 1 << 10, IRQ_ENABLE);
        v.ramp_ctrl = ROLLOVER | STOPPED;
        let events: Vec<u8> = (0..4).map(|_| v.step()).collect();
        assert_eq!(events, [0, WAVE_IRQ, 0, 0]);
        assert_eq!(v.pos >> WAVE_FRAC, 4);
        assert_eq!(v.frames_to_irq(), None);
    }

    #[test]
    fn ramp_stops_at_its_end() {
        let mut v = Voice { ramp_ctrl: IRQ_ENABLE, ramp_rate: 0x3F, ramp_start: 0, ramp_end: 0x10, ..Voice::default() };
        // 0x100 volume steps at 63 a frame.
        assert_eq!(v.frames_to_irq(), Some(5));
        let events: Vec<u8> = (0..5).map(|_| v.step()).collect();
        assert_eq!(events, [0, 0, 0, 0, RAMP_IRQ]);
        assert_eq!(v.vol >> RAMP_FRAC, 0x100);
        assert!(!v.ramp_running());
    }

    #[test]
    fn sixteen_bit_addresses_keep_the_bank() {
        let mut dram = vec![0u8; 1 << 20];
        // Word 0x40010 of the 256K bank at 0x40000 is at byte 0x40020.
        dram[0x40020..0x40022].copy_from_slice(&1234i16.to_le_bytes());
        let v = Voice { wave_ctrl: BITS16, pos: 0x40010 << WAVE_FRAC, ..Voice::default() };
        assert_eq!(v.sample(&dram), 1234.0);
    }
}
