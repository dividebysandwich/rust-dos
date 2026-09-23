//! The Roland MPU-401 MIDI interface at 330h/331h, in UART mode: MIDI bytes
//! a program writes go to a General MIDI synthesizer (`rustysynth`) playing
//! a SoundFont, when the `midi` feature is built and a SoundFont is
//! configured. Without one the interface is still there, so programs that
//! detect it work, but it plays nothing.

use std::collections::VecDeque;

/// Acknowledge byte for commands.
const ACK: u8 = 0xFE;
/// Frames the synthesizer renders at a time.
#[cfg(feature = "midi")]
const BLOCK: usize = 64;

pub struct Mpu401 {
    read_buf: VecDeque<u8>,
    /// Running status and the data bytes of the message being received.
    status: u8,
    data: [u8; 2],
    have: usize,
    in_sysex: bool,
    #[cfg(feature = "midi")]
    synth: Option<rustysynth::Synthesizer>,
    #[cfg(feature = "midi")]
    block: (Vec<f32>, Vec<f32>, usize),
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
            #[cfg(feature = "midi")]
            synth: None,
            #[cfg(feature = "midi")]
            block: (vec![0.0; BLOCK], vec![0.0; BLOCK], BLOCK),
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
        self.synth = Some(synth);
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
        #[cfg(feature = "midi")]
        if let Some(synth) = &mut self.synth {
            synth.reset();
        }
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
                return;
            }
            0xF7 => {
                self.in_sysex = false;
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
        if self.in_sysex || self.status == 0 {
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

    #[cfg(feature = "midi")]
    fn message(&mut self, status: u8, d1: u8, d2: u8) {
        if let Some(synth) = &mut self.synth {
            synth.process_midi_message((status & 0x0F) as i32, (status & 0xF0) as i32, d1 as i32, d2 as i32);
        }
    }

    #[cfg(not(feature = "midi"))]
    fn message(&mut self, _status: u8, _d1: u8, _d2: u8) {}

    /// One stereo frame of synthesizer output at the mixer's rate.
    #[inline]
    pub fn render(&mut self) -> (f32, f32) {
        #[cfg(feature = "midi")]
        if let Some(synth) = &mut self.synth {
            let (left, right, at) = &mut self.block;
            if *at == BLOCK {
                synth.render(left, right);
                *at = 0;
            }
            let frame = (left[*at] * 32767.0, right[*at] * 32767.0);
            *at += 1;
            return frame;
        }
        (0.0, 0.0)
    }
}
