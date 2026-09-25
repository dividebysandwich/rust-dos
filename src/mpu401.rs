//! The Roland MPU-401 MIDI interface at 330h/331h, in UART mode: MIDI bytes
//! a program writes go to a synthesizer. That is `rustysynth` playing a
//! SoundFont (with the `midi` feature), the Gravis Ultrasound patch set
//! played by `gus::synth`, a Roland MT-32 played by munt (`mt32`), or a MIDI
//! port of the host (`midiout`, with the `hostmidi` feature). Without one
//! the interface is still there, so programs that detect it work, but it
//! plays nothing.

use std::collections::VecDeque;

use crate::gus::patch::PatchBank;
use crate::midi_shadow::{Midi, MidiShadow};
use crate::gus::synth::GusSynth;

/// Acknowledge byte for commands.
const ACK: u8 = 0xFE;
/// Frames the synthesizer renders at a time.
#[cfg(feature = "midi")]
const BLOCK: usize = 64;
/// Longest System Exclusive message kept for the synthesizer: more than
/// the MT-32's largest (a bank of timbres goes in messages of up to 256
/// bytes of data).
const SYSEX_MAX: usize = 8192;

enum Synth {
    None,
    #[cfg(feature = "midi")]
    SoundFont {
        synth: Box<rustysynth::Synthesizer>,
        /// A block of rendered frames and how many were used.
        block: (Vec<f32>, Vec<f32>, usize),
    },
    Gus(Box<GusSynth>),
    #[cfg(not(target_arch = "wasm32"))]
    Mt32(Box<crate::mt32::Mt32>),
    #[cfg(all(feature = "hostmidi", not(target_arch = "wasm32")))]
    Host(Box<crate::midiout::HostMidi>),
}

impl Synth {
    /// Silence everything playing, and set a SoundFont or Ultrasound
    /// synthesizer back to its defaults.
    fn silence(&mut self) {
        match self {
            Synth::None => {}
            #[cfg(feature = "midi")]
            Synth::SoundFont { synth, block } => {
                synth.reset();
                block.2 = BLOCK;
            }
            Synth::Gus(synth) => synth.reset(),
            #[cfg(not(target_arch = "wasm32"))]
            Synth::Mt32(synth) => synth.notes_off(),
            #[cfg(all(feature = "hostmidi", not(target_arch = "wasm32")))]
            Synth::Host(port) => port.notes_off(),
        }
    }

    fn message(&mut self, status: u8, d1: u8, d2: u8) {
        match self {
            Synth::None => {}
            #[cfg(feature = "midi")]
            Synth::SoundFont { synth, .. } => {
                synth.process_midi_message((status & 0x0F) as i32, (status & 0xF0) as i32, d1 as i32, d2 as i32)
            }
            Synth::Gus(synth) => synth.message(status, d1, d2),
            #[cfg(not(target_arch = "wasm32"))]
            Synth::Mt32(synth) => synth.message(status, d1, d2),
            #[cfg(all(feature = "hostmidi", not(target_arch = "wasm32")))]
            Synth::Host(port) => port.message(status, d1, d2),
        }
    }

    /// A System Exclusive message; the SoundFont synthesizer takes none.
    fn sysex(&mut self, body: &[u8]) {
        match self {
            Synth::Gus(synth) => synth.sysex(body),
            #[cfg(not(target_arch = "wasm32"))]
            Synth::Mt32(synth) => synth.sysex(body),
            #[cfg(all(feature = "hostmidi", not(target_arch = "wasm32")))]
            Synth::Host(port) => port.sysex(body),
            _ => {}
        }
    }
}

pub struct Mpu401 {
    read_buf: VecDeque<u8>,
    /// Running status and the data bytes of the message being received.
    status: u8,
    data: [u8; 2],
    have: usize,
    in_sysex: bool,
    sysex: Vec<u8>,
    /// What the synthesizer was told, for after a save state is loaded.
    shadow: MidiShadow,
    synth: Synth,
}

impl Default for Mpu401 {
    fn default() -> Self {
        Self::new()
    }
}

impl Mpu401 {
    pub fn new() -> Self {
        Self {
            read_buf: VecDeque::new(),
            status: 0,
            data: [0; 2],
            have: 0,
            in_sysex: false,
            sysex: Vec::new(),
            shadow: MidiShadow::default(),
            synth: Synth::None,
        }
    }

    /// Play General MIDI with Ultrasound patches.
    pub fn load_gus_patches(&mut self, bank: PatchBank) {
        self.synth = Synth::Gus(Box::new(GusSynth::new(bank)));
    }

    /// Play nothing.
    pub fn remove_synth(&mut self) {
        self.synth = Synth::None;
    }

    /// Which synthesizer plays: "soundfont", "gus", "mt32", "host" or
    /// "none".
    pub fn synth_name(&self) -> &'static str {
        match self.synth {
            Synth::None => "none",
            #[cfg(feature = "midi")]
            Synth::SoundFont { .. } => "soundfont",
            Synth::Gus(_) => "gus",
            #[cfg(not(target_arch = "wasm32"))]
            Synth::Mt32(_) => "mt32",
            #[cfg(all(feature = "hostmidi", not(target_arch = "wasm32")))]
            Synth::Host(_) => "host",
        }
    }

    /// Play the MT-32 with munt. Returns what plays, for the log.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_mt32(&mut self, synth: crate::mt32::Mt32) -> String {
        let description = synth.description().to_string();
        self.synth = Synth::Mt32(Box::new(synth));
        description
    }

    /// Send the MIDI out of a port of the host. Returns the port's name.
    #[cfg(all(feature = "hostmidi", not(target_arch = "wasm32")))]
    pub fn open_host(&mut self, port: crate::midiout::HostMidi) -> String {
        let name = port.name().to_string();
        self.synth = Synth::Host(Box::new(port));
        name
    }

    /// What the MT-32's display shows, once, when a program changed it.
    pub fn take_lcd_message(&mut self) -> Option<String> {
        match &mut self.synth {
            #[cfg(not(target_arch = "wasm32"))]
            Synth::Mt32(synth) => synth.take_lcd_message(),
            _ => None,
        }
    }

    /// The Ultrasound patch synthesizer, if it is the one playing.
    pub fn gus_synth(&self) -> Option<&GusSynth> {
        match &self.synth {
            Synth::Gus(synth) => Some(synth),
            _ => None,
        }
    }

    /// Load a SoundFont for the synthesizer.
    #[cfg(feature = "midi")]
    pub fn load_soundfont(&mut self, path: &std::path::Path) -> Result<(), String> {
        let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {}", path.display(), e))?;
        let font = rustysynth::SoundFont::new(&mut file).map_err(|e| format!("{}: {:?}", path.display(), e))?;
        let settings = rustysynth::SynthesizerSettings::new(crate::opl::RATE as i32);
        let synth = rustysynth::Synthesizer::new(&std::sync::Arc::new(font), &settings)
            .map_err(|e| format!("{:?}", e))?;
        self.synth = Synth::SoundFont {
            synth: Box::new(synth),
            block: (vec![0.0; BLOCK], vec![0.0; BLOCK], BLOCK),
        };
        Ok(())
    }

    #[cfg(not(feature = "midi"))]
    pub fn load_soundfont(&mut self, _path: &std::path::Path) -> Result<(), String> {
        Err("this build has no MIDI synthesizer (the `midi` feature)".to_string())
    }

    /// Silence everything and forget partial messages, when a program ends.
    pub fn reset(&mut self) {
        self.read_buf.clear();
        self.status = 0;
        self.have = 0;
        self.in_sysex = false;
        self.shadow.reset_channels();
        self.synth.silence();
    }

    /// Status port: bit 7 clear when a byte can be read, bit 6 clear when
    /// the interface accepts one (always).
    pub fn read_status(&self) -> u8 {
        if self.read_buf.is_empty() { 0x80 } else { 0x00 }
    }

    pub fn read_data(&mut self) -> u8 {
        self.read_buf.pop_front().unwrap_or(ACK)
    }

    /// Command port: reset (FFh) and UART mode (3Fh), and the intelligent
    /// mode commands, which are acknowledged and otherwise ignored.
    pub fn write_command(&mut self, value: u8) {
        if value == 0xFF {
            self.status = 0;
            self.have = 0;
            self.read_buf.clear();
        }
        self.read_buf.push_back(ACK);
    }

    /// A MIDI byte for the synthesizer.
    pub fn write_data(&mut self, byte: u8) {
        match byte {
            // Real-time messages may come between any bytes.
            0xF8..=0xFF => return,
            0xF0 => {
                self.in_sysex = true;
                self.sysex.clear();
                return;
            }
            0xF7 => {
                if std::mem::take(&mut self.in_sysex) {
                    self.sysex_done();
                }
                return;
            }
            0xF1..=0xF6 => {
                self.status = 0;
                return;
            }
            0x80..=0xEF => {
                self.in_sysex = false;
                self.status = byte;
                self.have = 0;
                return;
            }
            _ => {}
        }
        if self.in_sysex {
            if self.sysex.len() < SYSEX_MAX {
                self.sysex.push(byte);
            }
            return;
        }
        if self.status == 0 {
            return;
        }
        self.data[self.have] = byte;
        self.have += 1;
        let needed = if matches!(self.status & 0xF0, 0xC0 | 0xD0) { 1 } else { 2 };
        if self.have == needed {
            self.have = 0;
            self.message(self.status, self.data[0], if needed == 2 { self.data[1] } else { 0 });
        }
    }

    fn message(&mut self, status: u8, d1: u8, d2: u8) {
        self.shadow.message(status, d1, d2);
        self.synth.message(status, d1, d2);
    }

    /// A whole System Exclusive message came, in `self.sysex`.
    fn sysex_done(&mut self) {
        self.shadow.sysex(&self.sysex);
        self.synth.sysex(&self.sysex);
    }

    /// Tell the synthesizer, which a save state can't hold, what it was
    /// told before the state was saved, after the state is loaded.
    pub fn after_load(&mut self) {
        self.synth.silence();
        let Mpu401 { shadow, synth, .. } = self;
        shadow.replay(|midi| match midi {
            Midi::Message(status, d1, d2) => synth.message(status, d1, d2),
            Midi::Sysex(body) => synth.sysex(body),
        });
    }

    /// One stereo frame of synthesizer output at the mixer's rate.
    #[inline]
    pub fn render(&mut self) -> (f32, f32) {
        match &mut self.synth {
            Synth::None => (0.0, 0.0),
            #[cfg(feature = "midi")]
            Synth::SoundFont { synth, block: (left, right, at) } => {
                if *at == BLOCK {
                    synth.render(left, right);
                    *at = 0;
                }
                let frame = (left[*at] * 32767.0, right[*at] * 32767.0);
                *at += 1;
                frame
            }
            Synth::Gus(synth) => synth.render(),
            #[cfg(not(target_arch = "wasm32"))]
            Synth::Mt32(synth) => synth.render(),
            #[cfg(all(feature = "hostmidi", not(target_arch = "wasm32")))]
            Synth::Host(_) => (0.0, 0.0),
        }
    }
}

// The synthesizer is the host's; `after_load` tells it what it missed.
crate::state_fields!(Mpu401 { read_buf, status, data, have, in_sysex, sysex, shadow } skip { synth });

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_sysex_arrives_whole() {
        // An MT-32 timbre bank goes in messages longer than the 64 bytes
        // the interface used to keep.
        let mut mpu = Mpu401::new();
        mpu.write_data(0xF0);
        for i in 0..300u32 {
            mpu.write_data((i % 0x80) as u8);
            // Real-time bytes may come in between.
            if i == 100 {
                mpu.write_data(0xF8);
            }
        }
        mpu.write_data(0xF7);
        assert_eq!(mpu.sysex.len(), 300);
        assert!(!mpu.in_sysex);
        assert_eq!(mpu.sysex[299], (299 % 0x80) as u8);
    }

    #[test]
    fn running_status_after_sysex() {
        let mut mpu = Mpu401::new();
        for b in [0x90, 0x3C, 0x7F, 0xF0, 0x41, 0xF7, 0x90, 0x3C, 0x00, 0x40] {
            mpu.write_data(b);
        }
        assert_eq!((mpu.status, mpu.have, mpu.data[0]), (0x90, 1, 0x40));
    }
}
