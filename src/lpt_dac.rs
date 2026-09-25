//! A DAC on the parallel port LPT1 (378h), which many games of the late
//! 1980s and early 1990s play digital sound through (`lpt_dac`): the Covox
//! Speech Thing, a resistor ladder on the data lines that sounds each byte
//! written, and the Disney Sound Source, which takes bytes into a 16-byte
//! FIFO on each rising edge of the Select line (bit 3 of the control
//! port) and plays them at 7 kHz, and says so on the Acknowledge line
//! (bit 6 of the status port) while the FIFO is full. As in DOSBox
//! Staging (disney.cpp and covox.cpp), both play through filters that give
//! them the sound of the real devices.

use crate::dsp::{BUTTERWORTH_Q, Biquad, OnePoleHighpass};
use std::collections::VecDeque;

/// The parallel port's base, and its data, status and control ports.
pub const LPT1: u16 = 0x378;
pub const DATA: u16 = LPT1;
pub const STATUS: u16 = LPT1 + 1;
pub const CONTROL: u16 = LPT1 + 2;

/// The DAC on LPT1 (`lpt_dac`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LptDacType {
    None,
    Disney,
    Covox,
}

impl LptDacType {
    pub const ALL: [LptDacType; 3] = [LptDacType::None, LptDacType::Disney, LptDacType::Covox];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "false" => Some(LptDacType::None),
            "disney" => Some(LptDacType::Disney),
            "covox" => Some(LptDacType::Covox),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LptDacType::None => "none",
            LptDacType::Disney => "disney",
            LptDacType::Covox => "covox",
        }
    }

    /// As the settings window shows it.
    pub fn describe(self) -> &'static str {
        match self {
            LptDacType::None => "none",
            LptDacType::Disney => "Disney Sound Source",
            LptDacType::Covox => "Covox Speech Thing",
        }
    }
}

/// The bytes the Disney Sound Source's FIFO holds.
const FIFO: usize = 16;
/// The rate the Disney Sound Source plays them at.
const DISNEY_RATE: f32 = 7000.0;
/// Silence: the middle of the unsigned 8-bit range.
const SILENT: u8 = 0x80;
/// The status port's bits: Acknowledge (active low on the cable, the
/// Disney's FIFO full) and nothing wrong otherwise.
const STATUS_ACK: u8 = 0x40;
const STATUS_IDLE: u8 = 0x77;

/// The DAC on LPT1 as it plays.
#[derive(Clone, Debug)]
pub struct LptDac {
    kind: LptDacType,
    data: u8,
    control: u8,
    /// The Disney's FIFO; it keeps its last byte playing when it runs dry.
    fifo: VecDeque<u8>,
    /// How far into the Disney's current sample, in its samples.
    phase: f32,
    high: Option<Biquad>,
    low: Biquad,
    /// Keeps a Covox left at a byte other than silence from adding a
    /// constant to the mix.
    dc: OnePoleHighpass,
}

impl LptDac {
    pub fn new(kind: LptDacType) -> Self {
        let (high, low) = match kind {
            LptDacType::Disney => (Some(Biquad::highpass(100.0, BUTTERWORTH_Q)), Biquad::lowpass(2000.0, BUTTERWORTH_Q)),
            _ => (None, Biquad::lowpass(9000.0, BUTTERWORTH_Q)),
        };
        Self {
            kind,
            data: SILENT,
            control: 0,
            fifo: VecDeque::from([SILENT]),
            phase: 0.0,
            high,
            low,
            dc: OnePoleHighpass::new(10.0),
        }
    }

    pub fn kind(&self) -> LptDacType {
        self.kind
    }

    fn fifo_full(&self) -> bool {
        self.fifo.len() >= FIFO
    }

    pub fn write_data(&mut self, value: u8) {
        self.data = value;
    }

    /// A write to the control port: on the Disney, a rising edge of
    /// Select takes the data byte into the FIFO, if it has room.
    pub fn write_control(&mut self, value: u8) {
        if self.kind == LptDacType::Disney && self.control & 0x08 == 0 && value & 0x08 != 0 && !self.fifo_full() {
            self.fifo.push_back(self.data);
        }
        self.control = value;
    }

    pub fn read_data(&self) -> u8 {
        self.data
    }

    pub fn read_control(&self) -> u8 {
        self.control
    }

    /// The status port: on the Disney, Acknowledge while the FIFO is full.
    pub fn read_status(&self) -> u8 {
        match self.kind {
            LptDacType::Disney if self.fifo_full() => STATUS_IDLE | STATUS_ACK,
            LptDacType::Disney => STATUS_IDLE & !STATUS_ACK,
            _ => STATUS_IDLE,
        }
    }

    /// The next sample at the mixer's rate, on a 16-bit scale.
    pub fn render(&mut self) -> f32 {
        let byte = match self.kind {
            LptDacType::Disney => {
                self.phase += DISNEY_RATE / crate::opl::RATE as f32;
                if self.phase >= 1.0 {
                    self.phase -= 1.0;
                    if self.fifo.len() > 1 {
                        self.fifo.pop_front();
                    }
                }
                self.fifo.front().copied().unwrap_or(SILENT)
            }
            _ => self.data,
        };
        let sample = (byte as f32 - SILENT as f32) * 256.0;
        let sample = match &mut self.high {
            Some(high) => high.process(sample),
            None => self.dc.process(sample),
        };
        self.low.process(sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_parse_back() {
        for kind in LptDacType::ALL {
            assert_eq!(LptDacType::parse(kind.name()), Some(kind));
        }
        assert_eq!(LptDacType::parse("ston1"), None);
    }

    #[test]
    fn silence_is_silent() {
        for kind in [LptDacType::Disney, LptDacType::Covox] {
            let mut dac = LptDac::new(kind);
            assert!((0..1000).all(|_| dac.render() == 0.0));
        }
    }
}
