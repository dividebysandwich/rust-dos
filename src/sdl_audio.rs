//! The SDL sound device the mixed output plays on.

use std::cell::Cell;
use std::rc::Rc;

use crate::audio;
use crate::config::Settings;
use crate::cpu::Cpu;
use crate::display;
use rust_dos::null_audio::NullAudio;

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
    fn name(&self) -> &str {
        self.subsystem.current_audio_driver()
    }

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

/// What `open_output` chose.
pub(crate) enum Output {
    Sdl(sdl2::audio::AudioQueue<i16>, sdl2::AudioSubsystem),
    /// The null output; the error is why SDL's device couldn't be opened,
    /// if it was tried.
    Null(Option<String>),
}

/// Open the sound output `settings` choose. `headless` is a run with no
/// window to go with the sound.
pub(crate) fn open_output(sdl: &sdl2::Sdl, settings: &Settings, headless: bool) -> Output {
    let driver_named = std::env::var_os("SDL_AUDIODRIVER").is_some();
    if !settings.audio_output.wants_sdl(headless, driver_named) {
        return Output::Null(None);
    }
    let device = sdl
        .audio()
        .and_then(|subsystem| Ok((open_audio(&subsystem, settings.audio_buffer.blocksize)?, subsystem)));
    match device {
        Ok((queue, subsystem)) => Output::Sdl(queue, subsystem),
        Err(e) => Output::Null(Some(e)),
    }
}

/// Give the machine its sound output. Without a sound device (as in a
/// virtual machine without a sound card) the emulator runs silent on the
/// null output, so the queue and its underruns still count.
pub(crate) fn attach(
    cpu: &mut Cpu,
    output: Output,
    settings: &Settings,
    audio_mix: &Rc<Cell<display::audio_mix::Mix>>,
) {
    match output {
        Output::Sdl(queue, subsystem) => {
            let driver = subsystem.current_audio_driver();
            if driver.eq_ignore_ascii_case("dummy") {
                warn_dummy(cpu);
            }
            cpu.bus.log_string(&format!("[AUDIO] Sound output: SDL's {} driver", driver));
            let mix = audio_mix.clone();
            let prebuffer = prebuffer_frames(&queue, settings.audio_buffer.prebuffer);
            let output = SdlAudio {
                queue,
                subsystem,
                prebuffer,
                mix,
                mixed: display::audio_mix::IDENTITY,
                buffer: Vec::new(),
            };
            cpu.bus.audio_device = Some(Box::new(output));
        }
        Output::Null(error) => {
            if let Some(e) = error {
                eprintln!("[AUDIO] No sound device, so no sound: {}", e);
                cpu.bus.log_string(&format!("[AUDIO] No sound device, so no sound: {}", e));
            }
            cpu.bus.log_string("[AUDIO] Sound output: the null output");
            cpu.bus.audio_device = Some(Box::new(NullAudio::new(settings.audio_buffer)));
        }
    }
}

/// SDL's dummy driver runs faster than real time, so the queue runs dry
/// however well the host keeps up. Said on stderr too, where a measuring
/// script sees it.
fn warn_dummy(cpu: &mut Cpu) {
    let warning = "[AUDIO] WARNING: SDL's dummy audio driver is active. It runs faster than \
                   real time, so underrun counts are wrong, and no measurement may use it. \
                   Use a real driver (SDL_AUDIODRIVER=wasapi on Windows) with --mute, or \
                   --audio-output null without a sound device.";
    eprintln!("{}", warning);
    cpu.bus.log_string(warning);
}

/// The 3D scene's own sounds (the rain, thunder, switches) on a sound
/// device of their own, which SDL's thread pulls from as it plays: never
/// waiting on the machine's frames, so a slow frame leaves no gap in the
/// rain, and it goes on while the machine is paused. Opened the first time
/// there is something to hear, paused while there is nothing.
pub(crate) struct RoomDevice {
    pub sound: std::sync::Arc<std::sync::Mutex<display::room_sound::RoomSound>>,
    device: Option<sdl2::audio::AudioDevice<RoomCallback>>,
    /// It couldn't be opened: not tried again.
    failed: bool,
}

pub(crate) struct RoomCallback(std::sync::Arc<std::sync::Mutex<display::room_sound::RoomSound>>);

impl sdl2::audio::AudioCallback for RoomCallback {
    type Channel = i16;

    fn callback(&mut self, out: &mut [i16]) {
        out.fill(0);
        self.0.lock().unwrap_or_else(|e| e.into_inner()).mix_into(out);
    }
}

impl RoomDevice {
    pub fn new() -> Self {
        RoomDevice { sound: Default::default(), device: None, failed: false }
    }

    /// Play while there is something to hear, on `subsystem`'s device.
    pub fn update(&mut self, subsystem: Option<&sdl2::AudioSubsystem>) {
        let active = self.sound.lock().unwrap_or_else(|e| e.into_inner()).active();
        if active && self.device.is_none() && !self.failed {
            let Some(subsystem) = subsystem else { return };
            // (SDL converts to the device's own rate and format.)
            let spec = sdl2::audio::AudioSpecDesired { freq: Some(44100), channels: Some(2), samples: Some(1024) };
            match subsystem.open_playback(None, &spec, |_| RoomCallback(self.sound.clone())) {
                Ok(device) => self.device = Some(device),
                Err(e) => {
                    eprintln!("[AUDIO] No sound device for the 3D scene's sounds: {}", e);
                    self.failed = true;
                }
            }
        }
        if let Some(device) = &self.device {
            match (active, device.status()) {
                (true, sdl2::audio::AudioStatus::Playing) | (false, sdl2::audio::AudioStatus::Paused) => {}
                (true, _) => device.resume(),
                (false, _) => device.pause(),
            }
        }
    }
}
