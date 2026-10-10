//! The Roland Sound Canvas SC-55 family on the MPU-401
//! (`midisynth=sc55`): a Rust port of Nuked-SC55, which emulates the
//! module's chips (the H8/532 running Roland's firmware, the sub-MCU and
//! the PCM chip) from their dies, as kept up by J.C. Moyer after nukeykt
//! archived the original. It needs the module's ROMs, which rust-dos
//! doesn't come with: `rom` finds them by their hashes, and the settings
//! window can download them once the user agrees to.
//!
//! Ported from <https://github.com/jcmoyer/Nuked-SC55> at 0.7.0, commit
//! 02f6e3d7bad89af33514bd48211bb950f8ad0e6b (GPL-2.0-or-later).

mod h8;
mod machine;
mod pcm;
mod resample;
pub mod rom;
mod submcu;
mod timer;

pub mod download;

use std::collections::VecDeque;
use std::path::Path;

use machine::Machine;
use resample::Resampler;

/// How long the firmware is let run when the module is switched on, so it
/// is ready for the first notes: a module whose main processor takes the
/// MIDI itself plays notes after 2.2 s, the mkII's sub-MCU at once.
fn boot_seconds(family: rom::Family) -> f64 {
    match family {
        rom::Family::Mk2 | rom::Family::St | rom::Family::Sc155Mk2 => 0.5,
        _ => 2.5,
    }
}

/// The module, and the MIDI on its way in.
struct Module {
    machine: Box<Machine>,
    /// MIDI bytes the serial port's buffer has no room for yet.
    queue: VecDeque<u8>,
    /// Frames the PCM chip made that haven't been taken.
    taken: usize,
}

impl Module {
    /// Move what the serial port's buffer has room for into it.
    fn feed(&mut self) {
        while self.machine.uart_pending() < 8000
            && let Some(byte) = self.queue.pop_front()
        {
            self.machine.post_uart(byte);
        }
    }

    /// One frame at the chip's own rate, as 32-bit samples.
    fn frame(&mut self) -> [i32; 2] {
        if self.taken >= self.machine.samples.len() {
            self.machine.samples.clear();
            self.taken = 0;
            self.feed();
            while self.machine.samples.is_empty() {
                self.machine.step();
            }
        }
        let frame = self.machine.samples[self.taken];
        self.taken += 1;
        frame
    }
}

/// The module's output has no steady level: the capacitors on its outputs
/// keep it from the amplifier. A one-pole high-pass filter at about 10 Hz
/// does the same here.
struct DcBlock {
    pole: f32,
    last_in: [f32; 2],
    last_out: [f32; 2],
}

impl DcBlock {
    fn new(rate: u32) -> DcBlock {
        DcBlock { pole: 1.0 - 2.0 * std::f32::consts::PI * 10.0 / rate as f32, last_in: [0.0; 2], last_out: [0.0; 2] }
    }

    #[inline]
    fn filter(&mut self, frame: [f32; 2]) -> [f32; 2] {
        for ((x, last_in), last_out) in frame.into_iter().zip(&mut self.last_in).zip(&mut self.last_out) {
            *last_out = x - *last_in + self.pole * *last_out;
            *last_in = x;
        }
        self.last_out
    }
}

/// A running Sound Canvas.
pub struct Sc55 {
    module: Module,
    resampler: Resampler,
    dc: DcBlock,
    /// Text a program put on the display, until it is taken.
    lcd: Option<String>,
    /// What plays, for the log.
    description: String,
}

impl Sc55 {
    /// Start a module of `model` (`auto`, a family or a `family-version`)
    /// with the ROMs in `rom_dir` or the usual places, at `rate` frames a
    /// second.
    pub fn open(rom_dir: Option<&Path>, model: &str, rate: u32) -> Result<Sc55, String> {
        let found = rom::find(rom_dir, model).ok_or_else(|| {
            let what = if model.eq_ignore_ascii_case("auto") { String::new() } else { format!(" for {}", model) };
            match rom_dir {
                Some(dir) => format!("no complete set of Sound Canvas ROMs{} in {}", what, dir.display()),
                None => format!(
                    "no Sound Canvas ROMs{} found (the settings window's Sound page can download them)",
                    what
                ),
            }
        })?;
        let loaded = rom::load(&found)?;
        let mut synth = Sc55::new(&loaded, rate);
        synth.description = format!("{} from {}", found.romset.display_name(), found.place());
        Ok(synth)
    }

    /// Start a module with ROMs already loaded.
    pub fn new(roms: &rom::Loaded, rate: u32) -> Sc55 {
        let mut synth = Sc55::switched_on(roms, rate);
        let frames = (boot_seconds(roms.romset.family) * rate as f64) as usize;
        let mut settled = [0.0; 2];
        for _ in 0..frames {
            settled = synth.resampled();
        }
        // From the level it settled at, so it doesn't thump.
        synth.dc.last_in = settled;
        synth
    }

    /// A module just switched on, its firmware not yet started: for
    /// comparing with Nuked-SC55 from the same point.
    #[doc(hidden)]
    pub fn switched_on(roms: &rom::Loaded, rate: u32) -> Sc55 {
        let machine = Box::new(Machine::new(roms, true));
        let native = machine.pcm.output_frequency();
        Sc55 {
            module: Module { machine, queue: VecDeque::new(), taken: 0 },
            resampler: Resampler::new(native, rate),
            dc: DcBlock::new(rate),
            lcd: None,
            description: roms.romset.display_name(),
        }
    }

    /// MIDI bytes as they come.
    #[doc(hidden)]
    #[inline]
    pub fn midi_bytes(&mut self, bytes: &[u8]) {
        self.post(bytes);
    }

    /// The model and ROMs that play, for the log.
    pub fn description(&self) -> &str {
        &self.description
    }

    fn post(&mut self, bytes: &[u8]) {
        self.module.queue.extend(bytes);
        self.module.feed();
    }

    /// A channel message.
    #[inline]
    pub fn message(&mut self, status: u8, d1: u8, d2: u8) {
        if matches!(status & 0xF0, 0xC0 | 0xD0) {
            self.post(&[status, d1]);
        } else {
            self.post(&[status, d1, d2]);
        }
    }

    /// A System Exclusive message, without its F0h and F7h.
    pub fn sysex(&mut self, body: &[u8]) {
        // Roland's "display letters", to the GS address 10 00 00.
        if let [0x41, _, 0x45, 0x12, 0x10, 0x00, 0x00, text @ .., _checksum] = body {
            let text: String = text.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { ' ' }).collect();
            let text = text.trim().to_string();
            if !text.is_empty() {
                self.lcd = Some(text);
            }
        }
        let mut framed = Vec::with_capacity(body.len() + 2);
        framed.push(0xF0);
        framed.extend_from_slice(body);
        framed.push(0xF7);
        self.post(&framed);
    }

    /// Silence the notes, as when a program ends.
    pub fn notes_off(&mut self) {
        for channel in 0..16u8 {
            self.message(0xB0 | channel, 64, 0);
            self.message(0xB0 | channel, 123, 0);
        }
    }

    /// Text a program put on the display, once.
    pub fn take_lcd_message(&mut self) -> Option<String> {
        self.lcd.take()
    }

    /// One frame at the chip's own rate, as 32-bit samples.
    pub fn native_frame(&mut self) -> [i32; 2] {
        self.module.frame()
    }

    /// The chips' state, for finding where this and Nuked-SC55 part.
    #[doc(hidden)]
    pub fn debug_state(&self) -> Vec<i64> {
        self.module.machine.debug_state()
    }

    /// The chip's rate.
    pub fn native_rate(&self) -> u32 {
        self.module.machine.pcm.output_frequency()
    }

    /// One stereo frame at the rate the module was opened with, on the
    /// scale of 16-bit samples.
    #[inline]
    pub fn render(&mut self) -> (f32, f32) {
        let frame = self.resampled();
        let [l, r] = self.dc.filter(frame);
        (l, r)
    }

    /// A frame at the output rate, before the steady level is taken out.
    #[inline]
    fn resampled(&mut self) -> [f32; 2] {
        let module = &mut self.module;
        self.resampler.next(|| {
            let [l, r] = module.frame();
            [l as f32 / 65536.0, r as f32 / 65536.0]
        })
    }
}
