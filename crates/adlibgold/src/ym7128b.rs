//! The Yamaha YM7128B surround processor on the AdLib Gold's surround
//! module: a delay line of 100 ms that takes in the card's mono mix, with
//! a feedback tap through a first-order filter and eight output taps
//! weighed into the left and the right output.
//!
//! The chip has 31 registers of up to 6 bits, which it takes serially: a
//! program writes bits to the control chip's register 18h, where bit 0 is
//! the data (DIN), bit 1 the bit clock (SCI) and bit 2 the word clock
//! (A0). A bit is taken on each rising edge of SCI, into the address while
//! A0 is low and into the data while it is high, and the falling edge of
//! A0 writes the data to the register addressed.
//!
//! Registers, in the datasheet's order:
//!
//! - 00h-07h (GL1-GL8) and 08h-0Fh (GR1-GR8): the gains of output taps 1
//!   to 8 into the left and the right output;
//! - 10h-13h (VM, VC, VL, VR): the gains of the input, of the feedback
//!   and of the left and right outputs;
//! - 14h-15h (C0, C1): the feedback filter's two coefficients;
//! - 16h-1Eh (T0-T8): where taps 0 (the feedback's) to 8 sit in the delay
//!   line.
//!
//! A gain is 6 bits: bit 5 the sign (set for the signal as it is, clear
//! for it inverted) and bits 0-4 the level, 0 silent and 1 to 31 from -60
//! dB to 0 dB in 2 dB steps. A coefficient is a 6-bit two's complement
//! fraction, and a tap position 5 bits, 0 to 100 ms in 31 steps.
//!
//! The model follows the "ideal" engine of Andrea Zoppi's YM7128B_emu
//! (the one DOSBox Staging uses): it runs at the mixer's rate rather than
//! the chip's 23.55 kHz, which keeps the delays exact and needs no
//! resampling. YM7128B_emu is under this licence:
//!
//! Copyright (c) 2020-2023, Andrea Zoppi. All rights reserved.
//!
//! Redistribution and use in source and binary forms, with or without
//! modification, are permitted provided that the following conditions
//! are met:
//!
//! 1. Redistributions of source code must retain the above copyright
//!    notice, this list of conditions and the following disclaimer.
//!
//! 2. Redistributions in binary form must reproduce the above copyright
//!    notice, this list of conditions and the following disclaimer in the
//!    documentation and/or other materials provided with the
//!    distribution.
//!
//! THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
//! IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED
//! TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A
//! PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
//! HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
//! SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
//! LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
//! DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
//! THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
//! (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
//! OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

use rust_dos_audio_core::RATE;

/// The registers, 00h-1Eh.
pub const REGISTERS: usize = 0x1F;
const VM: usize = 0x10;
const VC: usize = 0x11;
const VL: usize = 0x12;
const C0: usize = 0x14;
const C1: usize = 0x15;
const T0: usize = 0x16;
const TAPS: usize = 9;

/// The delay line's length: 100 ms and the sample at the input.
const LINE: usize = RATE as usize / 10 + 1;

/// Below this, a sample is silence (the mixer's samples are 16-bit
/// values): the delay line's tail ends in zeros, not denormal numbers.
const TINY: f32 = 1e-6;

/// The value of a 6-bit gain register.
pub fn gain(value: u8) -> f32 {
    let level = value & 0x1F;
    let magnitude = if level == 0 {
        0.0
    } else {
        10f32.powf((-60.0 + 2.0 * (level as f32 - 1.0)) / 20.0)
    };
    if value & 0x20 != 0 {
        magnitude
    } else {
        -magnitude
    }
}

/// The value of a 6-bit coefficient register: a two's complement
/// fraction, -1 to 31/32.
pub fn coefficient(value: u8) -> f32 {
    (((value & 0x3F) << 2) as i8) as f32 / 128.0
}

/// Where a 5-bit tap register puts its tap, in frames behind the input.
pub fn tap(value: u8) -> usize {
    (value & 0x1F) as usize * (RATE as usize / 10) / 31
}

pub struct Ym7128b {
    /// The registers as written.
    regs: [u8; REGISTERS],
    /// The serial port: the last level of SCI and A0, and the bits taken
    /// into the address and the data.
    sci: bool,
    a0: bool,
    address: u8,
    data: u8,
    /// The gains of registers 00h-13h and the coefficients, worked out
    /// from them.
    gains: [f32; C1 + 1],
    taps: [usize; TAPS],
    /// The delay line, newest sample at `head`.
    line: Box<[f32]>,
    head: usize,
    /// Tap 0 at the frame before, the feedback filter's delayed input.
    t0_before: f32,
}

impl Default for Ym7128b {
    fn default() -> Self {
        Self::new()
    }
}

impl Ym7128b {
    /// The chip at power-on: every register 0, so it gives silence.
    pub fn new() -> Self {
        let mut chip = Self {
            regs: [0; REGISTERS],
            sci: false,
            a0: false,
            address: 0,
            data: 0,
            gains: [0.0; C1 + 1],
            taps: [0; TAPS],
            line: vec![0.0; LINE].into_boxed_slice(),
            head: 0,
            t0_before: 0.0,
        };
        chip.refresh();
        chip
    }

    /// Take a write to the control chip's register 18h: one level of the
    /// serial port's three lines.
    pub fn write_serial(&mut self, value: u8) {
        let din = value & 1;
        let sci = value & 2 != 0;
        let a0 = value & 4 != 0;
        if self.a0 && !a0 {
            // The shift registers hold the last 5 address bits and 6 data
            // bits clocked in.
            self.write(self.address & 0x1F, self.data & 0x3F);
        } else if !self.sci && sci {
            if a0 {
                self.data = self.data << 1 | din;
            } else {
                self.address = self.address << 1 | din;
            }
        }
        self.sci = sci;
        self.a0 = a0;
    }

    /// Write a register.
    pub fn write(&mut self, address: u8, value: u8) {
        let address = address as usize;
        if address >= REGISTERS {
            return;
        }
        self.regs[address] = value & if address >= T0 { 0x1F } else { 0x3F };
        self.refresh_one(address);
    }

    /// The registers as written.
    pub fn registers(&self) -> &[u8; REGISTERS] {
        &self.regs
    }

    fn refresh(&mut self) {
        for address in 0..REGISTERS {
            self.refresh_one(address);
        }
    }

    fn refresh_one(&mut self, address: usize) {
        let value = self.regs[address];
        match address {
            0..C0 => self.gains[address] = gain(value),
            C0 | C1 => self.gains[address] = coefficient(value),
            T0.. => self.taps[address - T0] = tap(value),
        }
    }

    /// Whether the chip gives silence: no gain reaches an output.
    pub fn is_silent(&self) -> bool {
        self.gains[VL] == 0.0 && self.gains[VL + 1] == 0.0
            || self.gains[..VM].iter().all(|&g| g == 0.0)
    }

    /// Whether the delay line holds silence.
    pub fn at_rest(&self) -> bool {
        self.t0_before == 0.0 && self.line.iter().all(|&s| s == 0.0)
    }

    /// Empty the delay line (after a state was loaded).
    pub fn clear(&mut self) {
        self.line.fill(0.0);
        self.t0_before = 0.0;
    }

    fn at(&self, delay: usize) -> f32 {
        self.line[(self.head + delay) % LINE]
    }

    /// One frame: the mono input in, the left and right outputs out.
    pub fn process(&mut self, input: f32) -> (f32, f32) {
        let g = &self.gains;
        // The feedback: tap 0 through the filter, whose input a frame
        // before is the other term.
        let t0 = self.at(self.taps[0]);
        let feedback = (t0 * g[C0] + self.t0_before * g[C1]) * g[VC];
        self.t0_before = t0;
        let mut sample = input * g[VM] + feedback;
        if sample.abs() < TINY {
            sample = 0.0;
        }
        self.head = if self.head == 0 {
            LINE - 1
        } else {
            self.head - 1
        };
        self.line[self.head] = sample;
        let mut out = [0.0f32; 2];
        for (channel, out) in out.iter_mut().enumerate() {
            let mut sum = 0.0;
            for n in 1..TAPS {
                sum += self.at(self.taps[n]) * self.gains[channel * 8 + n - 1];
            }
            // The chip's output stage halves what it sums (its output
            // is oversampled by two, at the same level per sample).
            *out = sum * self.gains[VL + channel] / 2.0;
        }
        (out[0], out[1])
    }
}

rust_dos_savestate::state_fields!(Ym7128b { regs, sci, a0, address, data } skip { gains, taps, line, head, t0_before });

impl Ym7128b {
    /// Work the gains and taps out again from the registers loaded, with
    /// an empty delay line.
    pub fn after_load(&mut self) {
        self.refresh();
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Send a register write over the serial port, as a driver does:
    /// 5 address bits with A0 low, then 6 data bits with A0 high, each
    /// bit clocked by SCI, and A0 dropped to write.
    fn send(chip: &mut Ym7128b, address: u8, value: u8) {
        for bit in (0..5).rev() {
            let d = address >> bit & 1;
            chip.write_serial(d);
            chip.write_serial(d | 2);
        }
        for bit in (0..6).rev() {
            let d = value >> bit & 1;
            chip.write_serial(4 | d);
            chip.write_serial(4 | d | 2);
        }
        chip.write_serial(0);
    }

    #[test]
    fn serial_writes_reach_the_registers() {
        let mut chip = Ym7128b::new();
        send(&mut chip, 0x12, 0x3F);
        send(&mut chip, 0x1E, 0x1F);
        send(&mut chip, 0x00, 0x21);
        assert_eq!(chip.registers()[0x12], 0x3F);
        assert_eq!(chip.registers()[0x1E], 0x1F);
        assert_eq!(chip.registers()[0x00], 0x21);
    }

    #[test]
    fn gains_and_coefficients_follow_the_datasheet() {
        assert_eq!(gain(0x3F), 1.0);
        assert_eq!(gain(0x1F), -1.0);
        assert_eq!(gain(0x20), 0.0);
        assert!((gain(0x21) - 0.001).abs() < 1e-6);
        assert!((gain(0x3E) - 10f32.powf(-0.1)).abs() < 1e-6);
        assert_eq!(coefficient(0x1F), 31.0 / 32.0);
        assert_eq!(coefficient(0x20), -1.0);
        assert_eq!(coefficient(0x3F), -1.0 / 32.0);
        assert_eq!(tap(31), RATE as usize / 10);
        assert_eq!(tap(0), 0);
    }

    /// An impulse comes out of a tap the tap's delay later, at the tap's
    /// gain and the output's.
    #[test]
    fn an_impulse_comes_out_of_a_tap_its_delay_later() {
        let mut chip = Ym7128b::new();
        send(&mut chip, VM as u8, 0x3F);
        send(&mut chip, VL as u8, 0x3F);
        send(&mut chip, VL as u8 + 1, 0x3F);
        // Tap 1 at 31/31 of 100 ms into the left output, inverted into
        // the right one at -6 dB.
        send(&mut chip, T0 as u8 + 1, 31);
        send(&mut chip, 0x00, 0x3F);
        send(&mut chip, 0x08, 0x1C);
        let delay = tap(31);
        let mut out = Vec::new();
        out.push(chip.process(1000.0));
        for _ in 0..delay + 2 {
            out.push(chip.process(0.0));
        }
        let hits: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, o)| o.0 != 0.0)
            .map(|(n, _)| n)
            .collect();
        assert_eq!(hits, vec![delay]);
        let (l, r) = out[delay];
        assert_eq!(l, 500.0);
        assert!((r - -500.0 * gain(0x3C)).abs() < 1e-3, "{r}");
    }

    /// With every register 0 the chip gives silence.
    #[test]
    fn power_on_is_silent() {
        let mut chip = Ym7128b::new();
        assert!(chip.is_silent());
        for _ in 0..100 {
            assert_eq!(chip.process(1234.0), (0.0, 0.0));
        }
    }
}
