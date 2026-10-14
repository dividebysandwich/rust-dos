//! The SDL sound device the mixed output plays on.

use std::cell::Cell;
use std::rc::Rc;

use crate::audio;
use crate::config::Settings;
use crate::cpu::Cpu;
use crate::display;

/// The SDL sound device, where the mixed output goes, through the 3D
/// scene's mix of the channels for where the viewer is (`mix`, which the
/// main loop sets), from the one of the last samples (`mixed`).
pub(crate) struct SdlAudio {
    queue: sdl2::audio::AudioQueue<i16>,
    /// To open the device again with another buffer.
    subsystem: sdl2::AudioSubsystem,
    /// The queue kept on top of the device's buffer, in frames.
    prebuffer: usize,
    mix: std::rc::Rc<std::cell::Cell<display::audio_mix::Mix>>,
    mixed: display::audio_mix::Mix,
    buffer: Vec<i16>,
}

/// The sound device, playing, with a buffer of `blocksize` frames (SDL's
/// default is 2048 frames, 46 ms of latency).
pub(crate) fn open_audio(subsystem: &sdl2::AudioSubsystem, blocksize: u16) -> Result<sdl2::audio::AudioQueue<i16>, String> {
    let desired_spec =
        sdl2::audio::AudioSpecDesired { freq: Some(44100), channels: Some(2), samples: Some(blocksize) };
    let device = subsystem.open_queue::<i16, _>(None, &desired_spec)?;
    device.resume();
    Ok(device)
}

/// The prebuffer setting's milliseconds in frames of the device.
fn prebuffer_frames(queue: &sdl2::audio::AudioQueue<i16>, ms: u16) -> usize {
    queue.spec().freq as usize * ms as usize / 1000
}

impl audio::AudioOutput for SdlAudio {
    fn queued_frames(&self) -> usize {
        self.queue.size() as usize / 4
    }

    fn queue(&mut self, samples: &[i16]) -> Result<(), String> {
        use display::audio_mix::{IDENTITY, apply};
        let mix = self.mix.get();
        if mix == IDENTITY && self.mixed == IDENTITY {
            return self.queue.queue_audio(samples);
        }
        apply(samples, self.mixed, mix, &mut self.buffer);
        self.mixed = mix;
        self.queue.queue_audio(&self.buffer)
    }

    /// The device takes its buffer from the queue at once; the prebuffer
    /// is for a video frame that comes late.
    fn target_frames(&self) -> usize {
        self.queue.spec().samples as usize + self.prebuffer
    }

    /// A new block size opens the device again, which drops the little
    /// sound waiting in it.
    fn set_buffer(&mut self, buffer: rust_dos::audio::AudioBuffer) -> Result<(), String> {
        if self.queue.spec().samples != buffer.blocksize {
            let queue = open_audio(&self.subsystem, buffer.blocksize)?;
            self.queue = queue;
        }
        self.prebuffer = prebuffer_frames(&self.queue, buffer.prebuffer);
        Ok(())
    }
}

/// Give the machine the sound device, or say there is none: without one
/// (as in a virtual machine without a sound card) the emulator runs silent.
pub(crate) fn attach(
    cpu: &mut Cpu,
    audio_device: Result<(sdl2::audio::AudioQueue<i16>, sdl2::AudioSubsystem), String>,
    settings: &Settings,
    audio_mix: &Rc<Cell<display::audio_mix::Mix>>,
) {
    match audio_device {
        Ok((queue, subsystem)) => {
            let mix = audio_mix.clone();
            let prebuffer = prebuffer_frames(&queue, settings.audio_buffer.prebuffer);
            let output =
                SdlAudio { queue, subsystem, prebuffer, mix, mixed: display::audio_mix::IDENTITY, buffer: Vec::new() };
            cpu.bus.audio_device = Some(Box::new(output));
        }
        Err(e) => {
            eprintln!("[AUDIO] No sound device, so no sound: {}", e);
            cpu.bus.log_string(&format!("[AUDIO] No sound device, so no sound: {}", e));
        }
    }
}
