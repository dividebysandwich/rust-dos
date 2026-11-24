//! The Philips SAA1099: six square-wave channels, two noise generators
//! and two envelope generators. The tone and noise generators are MAME's
//! saa1099.cpp (BSD-3-Clause, by Juergen Buchmueller and Manuel Abadia,
//! with the noise and mixing that later research on the real chip
//! found). The envelope generators and how they meet the amplitudes are
//! SAASound's (BSD-3-Clause, by Dave Hooper and Simon Owen, the SAA1099
//! of SimCoupe), which Dave Hooper measured on a real chip in 2018: see
//! `Envelope`.
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
//! - 18h, 19h: the envelope generators of channels 2 and 5. Bit 0
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
//    notice, this list of conditions and the following disclaimer in the
//    documentation and/or other materials provided with the
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
//
// The envelope generators and their output follow SAASound's
// SAAEnv.cpp, SAAAmp.cpp and SAADevice.cpp
// (https://github.com/stripwax/SAASound), whose notice follows.
//
// Copyright (c) 1998-2004, Dave Hooper <dave@beermex.com>
// Copyright (c) 2004-2025, Dave Hooper <dave@beermex.com> + Simon Owen
// <simon@simonowen.com>
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions
// are met:
//
// - Redistributions of source code must retain the above copyright
//   notice, this list of conditions and the following disclaimer.
// - Redistributions in binary form must reproduce the above copyright
//   notice, this list of conditions and the following disclaimer in the
//   documentation and/or other materials provided with the
//   distribution.
// - Neither the name Dave Hooper nor the names of its contributors may
//   be used to endorse or promote products derived from this software
//   without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
// "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
// LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
// A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
// OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
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

/// The level of a channel under an envelope, at full output: the pulses
/// of its amplitude's and its envelope's pulse-density patterns that
/// are both on, in 64ths of 16-bit full scale. Dave Hooper recorded
/// both patterns from a real SAA1099 (Vogons, "CMS/GameBlaster
/// emulation thread", 14 November 2018): the amplitude's pattern in
/// 64 clock cycles has 4 pulses for each step of the amplitude without
/// its lowest bit, which the envelope drops, and the envelope's spreads
/// its pulses so that their meeting comes to half the product, rounded
/// up. Amplitude 15 under envelope level 15 makes 53, where 15 alone
/// makes 60.
fn enveloped(amplitude: u8, envelope: u8) -> i32 {
    ((amplitude >> 1) as u32 * envelope as u32).div_ceil(2) as i32
}

/// One of the six channels.
#[derive(Clone, Copy, Debug, Default)]
pub struct Channel {
    /// The frequency within the octave (register 08h-0Dh).
    pub frequency: u8,
    pub freq_enable: bool,
    pub noise_enable: bool,
    /// The octave, 0-7 (registers 10h-12h).
    pub octave: u8,
    /// The left and right amplitudes, 0-15, from the amplitude register.
    amplitude: [u8; 2],
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
        (self.amplitude[LEFT], self.amplitude[RIGHT])
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

/// How the level of an envelope's phase moves with the position in it.
#[derive(Clone, Copy)]
enum Ramp {
    Zero,
    Max,
    Up,
    Down,
}

/// The eight shapes (register 18h or 19h bits 1-3): their phases of 16
/// positions each, and whether the shape starts over after the last.
/// A shape that doesn't ends at level 0.
const SHAPES: [(&[Ramp], bool); 8] = [
    (&[Ramp::Zero], false),
    (&[Ramp::Max], true),
    // Single and repetitive decay.
    (&[Ramp::Down], false),
    (&[Ramp::Down], true),
    // Single and repetitive triangle.
    (&[Ramp::Up, Ramp::Down], false),
    (&[Ramp::Up, Ramp::Down], true),
    // Single and repetitive attack.
    (&[Ramp::Up], false),
    (&[Ramp::Up], true),
];

/// One of the two envelope generators. Philips' data sheet (Mullard,
/// August 1984) draws generator 0 after the amplitude control of
/// channel 2 alone and generator 1 after channel 5's ("two of the six
/// tone generators are under envelope control"), clocked by channels 1
/// and 4; channels 0, 1, 3 and 4 keep their amplitude whatever 18h and
/// 19h hold. Dave Hooper found the same on a real chip, as did those
/// who played samples through channel 2's envelope on Game Blasters
/// (Vogons, "SAA1099 as 4-bit DAC", 2021).
///
/// Of a write to 18h or 19h, bit 7 (on or off) and bit 4 (3 or 4 bits)
/// act at once; the rest is buffered while a shape runs and taken when
/// it ends, or when a repetitive one comes back to its start (the data
/// sheet's points (3) and (4) of Fig. 5). An envelope that has just
/// started runs, so a second write right after the first waits too.
#[derive(Clone, Copy, Debug)]
struct Envelope {
    enabled: bool,
    invert_right: bool,
    /// Clocked by writes to the address port, not by channel 1 or 4.
    external_clock: bool,
    shape: u8,
    /// 3-bit resolution: the position moves 2 at a time, the level
    /// loses its lowest bit.
    three_bit: bool,
    phase: u8,
    /// 0-15 within the phase.
    position: u8,
    /// At the end of a single shape, or not started: new settings act
    /// at once.
    ended: bool,
    /// A write waiting for the shape's end.
    pending: Option<u8>,
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            enabled: false,
            invert_right: false,
            external_clock: false,
            shape: 0,
            three_bit: false,
            phase: 0,
            position: 0,
            ended: true,
            pending: None,
        }
    }
}

rust_dos_savestate::state_fields!(Envelope {
    enabled,
    invert_right,
    external_clock,
    shape,
    three_bit,
    phase,
    position,
    ended,
    pending
});

impl Envelope {
    fn mask(&self) -> u8 {
        if self.three_bit { 0x0E } else { 0x0F }
    }

    /// The left and right levels, 0-15.
    fn levels(&self) -> (u8, u8) {
        let (phases, looping) = SHAPES[self.shape as usize];
        let left = if self.ended && !looping {
            0
        } else {
            let p = self.position & 0x0F;
            let level = match phases[self.phase as usize] {
                Ramp::Zero => 0,
                Ramp::Max => 15,
                Ramp::Up => p,
                Ramp::Down => 15 - p,
            };
            level & self.mask()
        };
        let right = if self.invert_right {
            (15 - left) & self.mask()
        } else {
            left
        };
        (left, right)
    }

    /// A write to register 18h or 19h.
    fn write(&mut self, value: u8) {
        let enabled = value & 0x80 != 0;
        if !enabled && !self.enabled {
            return;
        }
        self.enabled = enabled;
        if !enabled {
            self.ended = true;
            return;
        }
        // The resolution changes at once, the position with it: moving
        // to 3 bits drops its lowest bit, moving back sets it.
        let three_bit = value & 0x10 != 0;
        if three_bit && !self.three_bit {
            self.position &= 0x0E;
        } else if !three_bit && self.three_bit {
            self.position |= 0x01;
        }
        self.three_bit = three_bit;
        if self.ended {
            self.start(value);
            self.pending = None;
        } else {
            self.pending = Some(value);
        }
    }

    /// The settings of `value` taken, and the shape started.
    fn start(&mut self, value: u8) {
        self.invert_right = value & 0x01 != 0;
        self.shape = (value >> 1) & 7;
        self.three_bit = value & 0x10 != 0;
        self.external_clock = value & 0x20 != 0;
        self.enabled = value & 0x80 != 0;
        self.phase = 0;
        self.position = 0;
        self.ended = !self.enabled;
    }

    /// A clock from channel 1 or 4.
    fn internal_clock(&mut self) {
        if self.enabled && !self.external_clock {
            self.tick();
        }
    }

    /// A write of the address of register 18h or 19h.
    fn external_clock(&mut self) {
        if self.enabled && self.external_clock {
            self.tick();
        }
    }

    fn tick(&mut self) {
        if self.ended {
            return;
        }
        let (phases, looping) = SHAPES[self.shape as usize];
        self.position += if self.three_bit { 2 } else { 1 };
        if self.position < 16 {
            return;
        }
        self.position -= 16;
        self.phase += 1;
        if (self.phase as usize) < phases.len() {
            return;
        }
        // The end of the shape: a buffered write starts now.
        if looping {
            self.phase = 0;
        } else {
            self.ended = true;
        }
        if let Some(value) = self.pending.take() {
            self.start(value);
        }
    }
}

/// The chip.
#[derive(Clone, Debug, Default)]
pub struct Saa1099 {
    channels: [Channel; 6],
    noise: [Noise; 2],
    noise_params: [u8; 2],
    envelopes: [Envelope; 2],
    /// Registers 18h and 19h as last written.
    envelope_registers: [u8; 2],
    all_ch_enable: bool,
    sync_state: bool,
    selected_reg: u8,
}

rust_dos_savestate::state_fields!(Saa1099 {
    channels,
    noise,
    noise_params,
    envelopes,
    envelope_registers,
    all_ch_enable,
    sync_state,
    selected_reg
});

impl Saa1099 {
    pub fn new() -> Self {
        Self::default()
    }

    /// A write to the address port: the register the data port sets
    /// next. Picking 18h or 19h clocks that envelope generator if it
    /// takes its clock from these writes.
    pub fn write_address(&mut self, value: u8) {
        self.selected_reg = value & 0x1F;
        if let 0x18 | 0x19 = self.selected_reg {
            self.envelopes[self.selected_reg as usize - 0x18].external_clock();
        }
    }

    /// A write to the data port: the register picked last.
    pub fn write_data(&mut self, value: u8) {
        let reg = self.selected_reg;
        match reg {
            0x00..=0x05 => {
                let ch = &mut self.channels[reg as usize];
                ch.amplitude[LEFT] = value & 0x0F;
                ch.amplitude[RIGHT] = value >> 4;
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
                let g = reg as usize - 0x18;
                self.envelope_registers[g] = value & 0xBF;
                self.envelopes[g].write(value);
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
                // Channels 1 and 4 clock the envelope generators at each
                // edge.
                if ch == 1 || ch == 4 {
                    self.envelopes[ch / 3].internal_clock();
                }
            }
            let c = &mut self.channels[ch];
            c.counter -= DIVIDER;
            let noise_out = (self.noise[ch / 3].level & 1) as i32;
            let tone_out = (c.level & 1) as i32;
            // 0 silent, 2 full, 1 half: with both tone and noise on, the
            // tone at half its amplitude while the noise is high.
            let mix = match (c.noise_enable, c.freq_enable) {
                (true, true) => tone_out * (2 - noise_out),
                (true, false) => noise_out * 2,
                (false, true) => tone_out * 2,
                (false, false) => 0,
            };
            let envelope = match ch {
                2 | 5 => Some(&self.envelopes[ch / 3]).filter(|e| e.enabled),
                _ => None,
            };
            if let Some(e) = envelope {
                // Under an envelope the mixer's output is inverted: a
                // channel with tone and noise off gives the envelope's
                // level, which is how samples are played through it.
                let (el, er) = e.levels();
                let side = |amp: u8, env: u8| enveloped(amp, env) * (2 - mix) * 32767 / 128;
                out_l += side(c.amplitude[LEFT], el);
                out_r += side(c.amplitude[RIGHT], er);
            } else if mix > 0 {
                let side = |amp: u8| amplitude(amp) as i32 * mix / 2;
                out_l += side(c.amplitude[LEFT]);
                out_r += side(c.amplitude[RIGHT]);
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

    /// The envelope generators' registers 18h and 19h as last written,
    /// but for bit 6, which the chip doesn't have.
    pub fn envelope_registers(&self) -> [u8; 2] {
        self.envelope_registers
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

    /// The envelope's levels, left and right, of generator `g`.
    fn env(chip: &Saa1099, g: usize) -> (u8, u8) {
        chip.envelopes[g].levels()
    }

    /// The single decay from 15 down to 0, a step at each edge of channel
    /// 1's tone, and the right side mirrored.
    #[test]
    fn envelope_decays_with_channel_1() {
        let mut chip = Saa1099::new();
        // Channel 1 a fast tone, the envelope's clock: an edge every 2
        // samples.
        write(&mut chip, 0x09, 255);
        write(&mut chip, 0x10, 0x70);
        write(&mut chip, 0x18, 0x80 | 2 << 1 | 1);
        // The shape starts at once at its first level.
        assert_eq!(env(&chip, 0), (15, 0));
        write(&mut chip, 0x1C, 0x01);
        let mut lefts = Vec::new();
        for _ in 0..40 {
            chip.step();
            let (l, r) = env(&chip, 0);
            assert_eq!(l + r, 15);
            lefts.push(l);
        }
        let mut seen = lefts.clone();
        seen.dedup();
        // The first clock comes with channel 1's first edge, at once.
        assert_eq!(&seen[..4], &[14, 13, 12, 11], "{:?}", lefts);
        for _ in 0..2000 {
            chip.step();
        }
        // A single shape ends at 0; the right side mirrors it.
        assert_eq!(env(&chip, 0), (0, 15));
    }

    /// With bit 5, the envelope steps at each write of register 18h's
    /// address, and not with channel 1.
    #[test]
    fn envelope_clocked_by_address_writes() {
        let mut chip = Saa1099::new();
        write(&mut chip, 0x09, 255);
        write(&mut chip, 0x10, 0x70);
        write(&mut chip, 0x18, 0x80 | 0x20 | 7 << 1);
        write(&mut chip, 0x1C, 0x01);
        // The write of 18h's address came before the envelope was on, so
        // the attack starts at 0.
        assert_eq!(env(&chip, 0).0, 0);
        for _ in 0..1000 {
            chip.step();
        }
        assert_eq!(env(&chip, 0).0, 0);
        for expected in 1..=5 {
            chip.write_address(0x18);
            assert_eq!(env(&chip, 0), (expected, expected));
        }
        // Register 19h's address leaves generator 0 alone.
        chip.write_address(0x19);
        assert_eq!(env(&chip, 0).0, 5);
    }

    /// Bit 4 acts at once, even while the rest of the write waits: in
    /// 3-bit mode the position drops its lowest bit and moves 2 a clock,
    /// and going back to 4 bits sets the bit.
    #[test]
    fn envelope_resolution_acts_at_once() {
        let mut chip = Saa1099::new();
        write(&mut chip, 0x18, 0x80 | 0x20 | 7 << 1);
        for _ in 0..3 {
            chip.write_address(0x18);
        }
        assert_eq!(env(&chip, 0).0, 3);
        chip.write_data(0x80 | 0x30 | 7 << 1);
        assert_eq!(env(&chip, 0), (2, 2));
        chip.write_address(0x18);
        assert_eq!(env(&chip, 0).0, 4);
        chip.write_data(0x80 | 0x20 | 7 << 1);
        assert_eq!(env(&chip, 0).0, 5);
        chip.write_address(0x18);
        assert_eq!(env(&chip, 0).0, 6);
    }

    /// A write while a shape runs waits for its end: the repetitive
    /// attack climbs on to 15 before the decay written halfway takes
    /// over. Turning the envelope off acts at once.
    #[test]
    fn envelope_writes_wait_for_the_end_of_the_shape() {
        let mut chip = Saa1099::new();
        write(&mut chip, 0x18, 0x80 | 0x20 | 7 << 1);
        // The write of 18h's address above clocked the generator before
        // the shape was on: it starts at 0.
        assert_eq!(env(&chip, 0).0, 0);
        for _ in 0..8 {
            chip.write_address(0x18);
        }
        assert_eq!(env(&chip, 0).0, 8);
        // Repetitive decay, written at position 8 (the address write
        // clocks once more first).
        write(&mut chip, 0x18, 0x80 | 0x20 | 3 << 1);
        assert_eq!(env(&chip, 0).0, 9);
        for expected in 10..=15 {
            chip.write_address(0x18);
            assert_eq!(env(&chip, 0).0, expected);
        }
        // The attack wraps to its start, where the decay begins.
        chip.write_address(0x18);
        assert_eq!(env(&chip, 0).0, 15);
        chip.write_address(0x18);
        assert_eq!(env(&chip, 0).0, 14);
        chip.write_address(0x18);
        chip.write_data(0x00);
        assert!(!chip.envelopes[0].enabled);
    }

    /// The single attack ends at 0, and a write after its end starts the
    /// new shape at once.
    #[test]
    fn single_shapes_end_at_zero() {
        let mut chip = Saa1099::new();
        write(&mut chip, 0x18, 0x80 | 0x20 | 6 << 1);
        for _ in 0..15 {
            chip.write_address(0x18);
        }
        assert_eq!(env(&chip, 0).0, 15);
        chip.write_address(0x18);
        assert_eq!(env(&chip, 0).0, 0);
        chip.write_address(0x18);
        assert_eq!(env(&chip, 0).0, 0);
        // Maximum amplitude, at once.
        chip.write_data(0x80 | 0x20 | 1 << 1);
        assert_eq!(env(&chip, 0).0, 15);
    }

    /// Generator 0 acts on channel 2 alone: channels 0 and 1 keep their
    /// amplitude with an envelope of level 0 on, while channel 2 falls
    /// silent.
    #[test]
    fn envelope_acts_on_channel_2_only() {
        let mut chip = Saa1099::new();
        for ch in 0..3 {
            write(&mut chip, ch, 0x0F);
        }
        write(&mut chip, 0x10, 0x00);
        write(&mut chip, 0x11, 0x00);
        // Zero amplitude, clocked by address writes so it stands still.
        write(&mut chip, 0x18, 0x80 | 0x20);
        write(&mut chip, 0x1C, 0x01);
        let full = amplitude(15) as i32;
        for (enables, expected) in [(0x01, full), (0x02, full), (0x04, 0)] {
            write(&mut chip, 0x14, enables);
            let samples = left(&mut chip, 2000);
            let peak = samples.iter().copied().max().unwrap();
            assert_eq!(peak, expected, "tone enables {:02X}", enables);
        }
        // Generator 1 on its own group: channel 5 only.
        write(&mut chip, 0x18, 0x00);
        write(&mut chip, 0x19, 0x80 | 0x20);
        for (ch, enables) in [(3, 0x08), (5, 0x20)] {
            write(&mut chip, ch, 0x0F);
            write(&mut chip, 0x14, enables);
            let peak = left(&mut chip, 2000).into_iter().max().unwrap();
            assert_eq!(peak, if ch == 5 { 0 } else { full }, "channel {}", ch);
        }
    }

    /// Under an envelope, amplitude and envelope meet as the chip's
    /// pulse patterns do, the amplitude without its lowest bit: 15 and
    /// 14 under level 15 both give 53/64 of full scale, 1 gives nothing.
    /// With tone and noise off the channel gives that level steadily,
    /// and with the tone on it alternates between it and 0.
    #[test]
    fn enveloped_amplitude_and_steady_output() {
        let mut chip = Saa1099::new();
        // Maximum amplitude.
        write(&mut chip, 0x18, 0x80 | 0x20 | 1 << 1);
        write(&mut chip, 0x1C, 0x01);
        let level = |a: i32| a * 32767 / 64;
        for (amp, expected) in [(15, level(53)), (14, level(53)), (3, level(8)), (1, 0)] {
            write(&mut chip, 0x02, amp);
            let samples = left(&mut chip, 100);
            assert!(samples.iter().all(|&s| s == expected), "amplitude {}", amp);
        }
        write(&mut chip, 0x02, 0x0F);
        write(&mut chip, 0x0A, 255);
        write(&mut chip, 0x11, 0x04);
        write(&mut chip, 0x14, 0x04);
        let samples = left(&mut chip, 400);
        assert!(samples.iter().all(|&s| s == 0 || s == level(53)));
        assert!(samples.contains(&0) && samples.contains(&level(53)));
        // The envelope off, the tone gives its full 60/64.
        write(&mut chip, 0x18, 0x00);
        let samples = left(&mut chip, 400);
        assert!(samples.contains(&amplitude(15).into()));
    }

    /// The pulse counts Dave Hooper recorded from the chip for amplitude
    /// 15 under each envelope level, and for amplitude 2.
    #[test]
    fn enveloped_matches_the_recorded_patterns() {
        let amp15 = [0, 4, 7, 11, 14, 18, 21, 25, 28, 32, 35, 39, 42, 46, 49, 53];
        let amp2 = [0, 1, 1, 2, 2, 3, 3, 4];
        for env in 0..16 {
            assert_eq!(enveloped(15, env), amp15[env as usize]);
        }
        for env in 0..8 {
            assert_eq!(enveloped(2, env), amp2[env as usize]);
        }
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
        write(&mut chip, 0x02, 0x9F);
        write(&mut chip, 0x14, 0x05);
        write(&mut chip, 0x15, 0x02);
        write(&mut chip, 0x18, 0x8A);
        write(&mut chip, 0x18, 0x86);
        write(&mut chip, 0x1C, 0x01);
        for _ in 0..777 {
            chip.step();
        }
        let mut w = rust_dos_savestate::Writer::new();
        chip.save(&mut w);
        let mut copy = Saa1099::new();
        copy.load(&mut rust_dos_savestate::Reader::new(&w.buf))
            .unwrap();
        assert_eq!(copy.envelope_registers(), [0x86, 0x00]);
        assert_eq!(
            (0..5000).map(|_| chip.step()).collect::<Vec<_>>(),
            (0..5000).map(|_| copy.step()).collect::<Vec<_>>()
        );
    }
}
