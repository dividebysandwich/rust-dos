//! The Philips SAA1099: six square-wave channels, two noise generators
//! and two envelope generators, as MAME's saa1099.cpp has it
//! (BSD-3-Clause, by Juergen Buchmueller and Manuel Abadia, with the
//! noise and mixing that later research on the real chip found).
//!
//! The chip has two ports: a write to the address port picks a
//! register, a write to the data port sets it. Its registers:
//!
//! - 00h-05h: each channel's amplitude, left in bits 0-3, right in 4-7.
//! - 08h-0Dh: each channel's frequency, 0-255 within its octave.
//! - 10h-12h: the octaves, two channels a register (bits 0-2 and 4-6).
//! - 14h: the channels' tone enables, a bit each.
//! - 15h: the channels' noise enables, a bit each.
//! - 16h: the noise generators' clocks: bits 0-1 for channels 0-2,
//!   4-5 for 3-5. 0 to 2 are the chip's clock / 256, 512 and 1024; 3
//!   follows the tone of channel 0 (or 3).
//! - 18h, 19h: the envelope generators of channels 0-2 and 3-5. Bit 0
//!   mirrors the right side's envelope, bits 1-3 the shape, bit 4 cuts
//!   it to 3 bits, bit 5 clocks it with writes to the address port of
//!   register 18h or 19h (else with channel 1's or 4's tone), bit 7
//!   turns it on.
//! - 1Ch: bit 0 turns the sound on, bit 1 resets the tone generators.
//!
//! The chip makes a sample every 256 cycles of its clock (`DIVIDER`),
//! which `step` gives.

// This file is derived from MAME's src/devices/sound/saa1099.cpp, whose
// notice follows.
//
// license:BSD-3-Clause
// copyright-holders:Juergen Buchmueller, Manuel Abadia
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
//
// 1. Redistributions of source code must retain the above copyright
//    notice, this list of conditions and the following disclaimer.
// 2. Redistributions in binary form must reproduce the above copyright
//    notice, this list of conditions and the following disclaimer in
//    the documentation and/or other materials provided with the
//    distribution.
// 3. Neither the name of the copyright holder nor the names of its
//    contributors may be used to endorse or promote products derived
//    from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
// "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
// LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
// A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
// HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
// LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
// DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
// THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
// (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
// OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

/// The clock cycles in each of the chip's samples.
pub const DIVIDER: i32 = 256;

const LEFT: usize = 0;
const RIGHT: usize = 1;

/// A 4-bit amplitude as the level it gives: sixteenths of 16-bit full
/// scale.
fn amplitude(nibble: u8) -> u16 {
    (nibble as u32 * 32767 / 16) as u16
}

/// The envelope generators' shapes, 64 steps each; after the last a
/// shape repeats steps 32-63.
const ENVELOPE: [[u8; 64]; 8] = {
    let mut shapes = [[0u8; 64]; 8];
    let mut i = 0;
    while i < 64 {
        let up = (i % 16) as u8;
        let down = 15 - up;
        // Zero amplitude.
        shapes[0][i] = 0;
        // Maximum amplitude.
        shapes[1][i] = 15;
        // Single decay.
        shapes[2][i] = if i < 16 { down } else { 0 };
        // Repetitive decay.
        shapes[3][i] = down;
        // Single triangular.
        shapes[4][i] = if i < 16 {
            up
        } else if i < 32 {
            down
        } else {
            0
        };
        // Repetitive triangular.
        shapes[5][i] = if (i / 16) % 2 == 0 { up } else { down };
        // Single attack.
        shapes[6][i] = if i < 16 { up } else { 0 };
        // Repetitive attack.
        shapes[7][i] = up;
        i += 1;
    }
    shapes
};

/// One of the six channels.
#[derive(Clone, Copy, Debug, Default)]
pub struct Channel {
    /// The frequency within the octave (register 08h-0Dh).
    pub frequency: u8,
    pub freq_enable: bool,
    pub noise_enable: bool,
    /// The octave, 0-7 (registers 10h-12h).
    pub octave: u8,
    /// The left and right levels, from the amplitude register.
    amplitude: [u16; 2],
    /// The envelope's factor on each side, 0-15, or 16 without one.
    envelope: [u8; 2],
    /// Clock cycles to the square wave's next edge, and its level.
    counter: i32,
    level: u8,
}

impl Channel {
    /// The clock cycles of half the square wave's period.
    #[inline]
    fn period(&self) -> i32 {
        (511 - self.frequency as i32) << (8 - self.octave as i32)
    }

    /// The left and right amplitudes, 0-15.
    pub fn amplitudes(&self) -> (u8, u8) {
        let nibble = |a: u16| (a as u32 * 16).div_ceil(32767) as u8;
        (nibble(self.amplitude[LEFT]), nibble(self.amplitude[RIGHT]))
    }

    /// The tone's frequency in hertz at a chip clock of `clock` Hz.
    pub fn hz(&self, clock: f64) -> f64 {
        clock / (2.0 * self.period() as f64)
    }
}

rust_dos_savestate::state_fields!(Channel {
    frequency,
    freq_enable,
    noise_enable,
    octave,
    amplitude,
    envelope,
    counter,
    level
});

/// One of the two noise generators: an 18-bit shift register.
#[derive(Clone, Copy, Debug)]
struct Noise {
    counter: i32,
    /// The clock cycles between shifts.
    period: i32,
    level: u32,
}

impl Default for Noise {
    fn default() -> Self {
        Self {
            counter: 0,
            period: 0,
            level: 0xFFFF_FFFF,
        }
    }
}

rust_dos_savestate::state_fields!(Noise {
    counter,
    period,
    level
});

/// The chip.
#[derive(Clone, Debug, Default)]
pub struct Saa1099 {
    channels: [Channel; 6],
    noise: [Noise; 2],
    noise_params: [u8; 2],
    env_enable: [bool; 2],
    env_reverse_right: [bool; 2],
    env_mode: [u8; 2],
    /// 3-bit envelope resolution.
    env_bits: [bool; 2],
    /// Clocked by writes to the address port, not by channel 1 or 4.
    env_clock: [bool; 2],
    env_step: [u8; 2],
    all_ch_enable: bool,
    sync_state: bool,
    selected_reg: u8,
}

rust_dos_savestate::state_fields!(Saa1099 {
    channels,
    noise,
    noise_params,
    env_enable,
    env_reverse_right,
    env_mode,
    env_bits,
    env_clock,
    env_step,
    all_ch_enable,
    sync_state,
    selected_reg
});

impl Saa1099 {
    pub fn new() -> Self {
        Self::default()
    }

    /// A write to the address port: the register the data port sets
    /// next. Picking 18h or 19h clocks the envelope generators that take
    /// their clock from these writes.
    pub fn write_address(&mut self, value: u8) {
        self.selected_reg = value & 0x1F;
        if self.selected_reg == 0x18 || self.selected_reg == 0x19 {
            for ch in 0..2 {
                if self.env_clock[ch] {
                    self.envelope_step(ch);
                }
            }
        }
    }

    /// A write to the data port: the register picked last.
    pub fn write_data(&mut self, value: u8) {
        let reg = self.selected_reg;
        match reg {
            0x00..=0x05 => {
                let ch = &mut self.channels[reg as usize];
                ch.amplitude[LEFT] = amplitude(value & 0x0F);
                ch.amplitude[RIGHT] = amplitude(value >> 4);
            }
            0x08..=0x0D => self.channels[reg as usize - 8].frequency = value,
            0x10..=0x12 => {
                let ch = (reg as usize - 0x10) * 2;
                self.channels[ch].octave = value & 7;
                self.channels[ch + 1].octave = (value >> 4) & 7;
            }
            0x14 => {
                for (i, ch) in self.channels.iter_mut().enumerate() {
                    ch.freq_enable = value >> i & 1 != 0;
                }
            }
            0x15 => {
                for (i, ch) in self.channels.iter_mut().enumerate() {
                    ch.noise_enable = value >> i & 1 != 0;
                }
            }
            0x16 => {
                self.noise_params[0] = value & 3;
                self.noise_params[1] = (value >> 4) & 3;
            }
            0x18 | 0x19 => {
                let ch = reg as usize - 0x18;
                self.env_reverse_right[ch] = value & 0x01 != 0;
                self.env_mode[ch] = (value >> 1) & 7;
                self.env_bits[ch] = value & 0x10 != 0;
                self.env_clock[ch] = value & 0x20 != 0;
                self.env_enable[ch] = value & 0x80 != 0;
                // The envelope starts over.
                self.env_step[ch] = 0;
            }
            0x1C => {
                self.all_ch_enable = value & 0x01 != 0;
                self.sync_state = value & 0x02 != 0;
                if self.sync_state {
                    for ch in &mut self.channels {
                        ch.level = 0;
                        ch.counter = ch.period();
                    }
                }
            }
            _ => {}
        }
    }

    /// One step of an envelope generator: steps 0 to 63, then 32 to 63
    /// over again, the factor of its three channels from the shape.
    fn envelope_step(&mut self, ch: usize) {
        let group = &mut self.channels[ch * 3..ch * 3 + 3];
        if !self.env_enable[ch] {
            for c in group {
                c.envelope = [16, 16];
            }
            return;
        }
        let step = ((self.env_step[ch] + 1) & 0x3F) | (self.env_step[ch] & 0x20);
        self.env_step[ch] = step;
        let mask = if self.env_bits[ch] { 0x0E } else { 0x0F };
        let value = ENVELOPE[self.env_mode[ch] as usize][step as usize];
        let right = if self.env_reverse_right[ch] {
            15 - value
        } else {
            value
        };
        for c in group {
            c.envelope = [value & mask, right & mask];
        }
    }

    /// The chip's next sample, left and right: the sum of its channels,
    /// each up to 15/16 of 16-bit full scale. A chip with its sound off
    /// gives silence and its generators stand still.
    pub fn step(&mut self) -> (i32, i32) {
        if !self.all_ch_enable {
            return (0, 0);
        }
        for g in 0..2 {
            self.noise[g].period = match self.noise_params[g] {
                p @ 0..=2 => 256 << p,
                _ => self.channels[g * 3].period(),
            };
        }
        let (mut out_l, mut out_r) = (0i32, 0i32);
        for ch in 0..6 {
            while self.channels[ch].counter <= 0 {
                let c = &mut self.channels[ch];
                c.counter += c.period();
                c.level ^= 1;
                // Channels 1 and 4 clock the envelope generators that
                // don't take their clock from the address port.
                if ch == 1 && !self.env_clock[0] {
                    self.envelope_step(0);
                }
                if ch == 4 && !self.env_clock[1] {
                    self.envelope_step(1);
                }
            }
            let c = &mut self.channels[ch];
            c.counter -= DIVIDER;
            let noise_out = (self.noise[ch / 3].level & 1) as u8;
            let tone_out = c.level & 1;
            // 0 silent, 1 full, 2 half: with both tone and noise on, the
            // tone at half its amplitude while the noise is high.
            let level = match (c.noise_enable, c.freq_enable) {
                (true, true) => tone_out << noise_out,
                (true, false) => noise_out,
                (false, true) => tone_out,
                (false, false) => 0,
            };
            if level > 0 {
                let side =
                    |s: usize| c.amplitude[s] as i32 * c.envelope[s] as i32 / 16 / level as i32;
                out_l += side(LEFT);
                out_r += side(RIGHT);
            }
        }
        // The noise: an 18-bit shift register, x^18 + x^11 + 1, plain
        // XOR feedback.
        for n in &mut self.noise {
            while n.counter <= 0 {
                n.counter += n.period;
                let feedback = (n.level & 0x20000 == 0) != (n.level & 0x400 == 0);
                n.level = n.level << 1 | feedback as u32;
            }
            n.counter -= DIVIDER;
        }
        (out_l, out_r)
    }

    /// Whether the sound is on (register 1Ch bit 0).
    pub fn enabled(&self) -> bool {
        self.all_ch_enable
    }

    pub fn channels(&self) -> &[Channel; 6] {
        &self.channels
    }

    /// The noise generators' clock settings (register 16h).
    pub fn noise_params(&self) -> [u8; 2] {
        self.noise_params
    }

    /// The envelope generators' registers 18h and 19h as written, but
    /// for the bits that aren't kept.
    pub fn envelope_registers(&self) -> [u8; 2] {
        std::array::from_fn(|g| {
            self.env_reverse_right[g] as u8
                | self.env_mode[g] << 1
                | (self.env_bits[g] as u8) << 4
                | (self.env_clock[g] as u8) << 5
                | (self.env_enable[g] as u8) << 7
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_dos_savestate::State;

    fn write(chip: &mut Saa1099, reg: u8, value: u8) {
        chip.write_address(reg);
        chip.write_data(value);
    }

    /// The left side's samples of `n` steps.
    fn left(chip: &mut Saa1099, n: usize) -> Vec<i32> {
        (0..n).map(|_| chip.step().0).collect()
    }

    #[test]
    fn silent_until_the_sound_is_on() {
        let mut chip = Saa1099::new();
        write(&mut chip, 0x00, 0xFF);
        write(&mut chip, 0x14, 0x01);
        assert!(left(&mut chip, 1000).iter().all(|&s| s == 0));
        write(&mut chip, 0x1C, 0x01);
        assert!(left(&mut chip, 1000).iter().any(|&s| s != 0));
    }

    /// A tone's half period is (511 - frequency) << (8 - octave) clock
    /// cycles: frequency 255 in octave 4 is 4096 cycles, 16 samples high
    /// and 16 low, and full amplitude is 15/16 of full scale.
    #[test]
    fn tone_period_and_amplitude() {
        let mut chip = Saa1099::new();
        write(&mut chip, 0x00, 0x0F);
        write(&mut chip, 0x08, 255);
        write(&mut chip, 0x10, 0x04);
        write(&mut chip, 0x14, 0x01);
        write(&mut chip, 0x1C, 0x01);
        let samples = left(&mut chip, 64 * 32);
        let high = amplitude(15) as i32;
        assert!(
            samples.iter().all(|&s| s == 0 || s == high),
            "{:?}",
            &samples[..40]
        );
        let edges: Vec<usize> = (1..samples.len())
            .filter(|&i| samples[i] != samples[i - 1])
            .collect();
        assert!(edges.len() > 100);
        // The first edge comes at once, as the counter starts at 0.
        assert!(
            edges[1..].windows(2).all(|w| w[1] - w[0] == 16),
            "{:?}",
            &edges[..10]
        );
        assert_eq!(chip.channels()[0].hz(8_000_000.0), 8_000_000.0 / 8192.0);
        // The right side's amplitude is 0.
        assert!((0..100).all(|_| chip.step().1 == 0));
    }

    #[test]
    fn amplitude_nibbles() {
        let mut chip = Saa1099::new();
        write(&mut chip, 0x02, 0x3C);
        assert_eq!(chip.channels()[2].amplitudes(), (12, 3));
    }

    /// The noise at the clock / 256: a new bit every sample, the output
    /// high about half the time, and with the tone on as well the tone at
    /// half its level while the noise is high.
    #[test]
    fn noise_is_random_and_halves_a_tone() {
        let mut chip = Saa1099::new();
        write(&mut chip, 0x00, 0x0F);
        write(&mut chip, 0x15, 0x01);
        write(&mut chip, 0x16, 0x00);
        write(&mut chip, 0x1C, 0x01);
        let samples = left(&mut chip, 4000);
        let high = samples.iter().filter(|&&s| s != 0).count();
        assert!((1600..2400).contains(&high), "{}", high);
        let changes = samples.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(changes > 1000, "{}", changes);
        write(&mut chip, 0x14, 0x01);
        let full = amplitude(15) as i32;
        let samples = left(&mut chip, 4000);
        assert!(samples.contains(&full) && samples.contains(&(full / 2)));
        assert!(
            samples
                .iter()
                .all(|&s| s == 0 || s == full || s == full / 2)
        );
    }

    /// The single decay down to 0, a step at each edge of channel 1's
    /// tone, and the right side mirrored.
    #[test]
    fn envelope_decays_with_channel_1() {
        let mut chip = Saa1099::new();
        // Channel 2 a fast tone at full amplitude, channel 1 the
        // envelope's clock.
        write(&mut chip, 0x02, 0xFF);
        write(&mut chip, 0x0A, 255);
        write(&mut chip, 0x11, 0x07);
        write(&mut chip, 0x09, 255);
        write(&mut chip, 0x10, 0x70);
        write(&mut chip, 0x14, 0x04);
        write(&mut chip, 0x18, 0x80 | 2 << 1 | 1);
        write(&mut chip, 0x1C, 0x01);
        let mut lefts = Vec::new();
        let mut rights = Vec::new();
        for _ in 0..40 {
            chip.step();
            lefts.push(chip.channels()[2].envelope[LEFT]);
            rights.push(chip.channels()[2].envelope[RIGHT]);
        }
        let mut seen: Vec<u8> = lefts.clone();
        seen.dedup();
        // The first clock comes with channel 1's first edge, at once, and
        // takes the envelope from step 0 to step 1.
        assert_eq!(&seen[..3], &[14, 13, 12], "{:?}", lefts);
        assert!(lefts.iter().zip(&rights).all(|(l, r)| l + r == 15));
        for _ in 0..2000 {
            chip.step();
        }
        assert_eq!(chip.channels()[2].envelope, [0, 15]);
    }

    /// With bit 5, the envelope steps at each write of register 18h's
    /// address, and not with channel 1.
    #[test]
    fn envelope_clocked_by_address_writes() {
        let mut chip = Saa1099::new();
        write(&mut chip, 0x18, 0x80 | 0x20 | 7 << 1);
        write(&mut chip, 0x1C, 0x01);
        for _ in 0..1000 {
            chip.step();
        }
        assert_eq!(chip.channels()[0].envelope, [0, 0]);
        for expected in 1..=5 {
            chip.write_address(0x18);
            assert_eq!(chip.channels()[0].envelope, [expected, expected]);
        }
        // 3-bit resolution drops bit 0.
        write(&mut chip, 0x18, 0x80 | 0x30 | 7 << 1);
        chip.write_address(0x18);
        chip.write_address(0x18);
        chip.write_address(0x18);
        assert_eq!(chip.channels()[0].envelope, [2, 2]);
    }

    #[test]
    fn sync_resets_the_tones() {
        let mut chip = Saa1099::new();
        write(&mut chip, 0x00, 0x0F);
        write(&mut chip, 0x14, 0x01);
        write(&mut chip, 0x1C, 0x01);
        for _ in 0..1234 {
            chip.step();
        }
        write(&mut chip, 0x1C, 0x03);
        assert_eq!(chip.channels()[0].level, 0);
        assert_eq!(chip.channels()[0].counter, chip.channels()[0].period());
    }

    #[test]
    fn state_round_trip() {
        let mut chip = Saa1099::new();
        write(&mut chip, 0x00, 0x9F);
        write(&mut chip, 0x14, 0x01);
        write(&mut chip, 0x15, 0x02);
        write(&mut chip, 0x18, 0x8A);
        write(&mut chip, 0x1C, 0x01);
        for _ in 0..777 {
            chip.step();
        }
        let mut w = rust_dos_savestate::Writer::new();
        chip.save(&mut w);
        let mut copy = Saa1099::new();
        copy.load(&mut rust_dos_savestate::Reader::new(&w.buf))
            .unwrap();
        assert_eq!(
            (0..500).map(|_| chip.step()).collect::<Vec<_>>(),
            (0..500).map(|_| copy.step()).collect::<Vec<_>>()
        );
    }
}
