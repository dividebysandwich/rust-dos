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
    /// None only after the device couldn't be opened again.
    stream: Option<Stream>,
    /// To open the device again with another buffer.
    subsystem: sdl3::AudioSubsystem,
    /// The queue kept on top of the device's buffer, in frames.
    prebuffer: usize,
    mix: std::rc::Rc<std::cell::Cell<display::audio_mix::Mix>>,
    mixed: display::audio_mix::Mix,
    buffer: Vec<i16>,
    /// The 3D scene's sound stream, on the same device: closed with the
    /// machine's when the block size changes, or the device would stay
    /// open at the old size. RoomDevice opens it again when it needs it.
    room: RoomSlot,
}

/// The machine's sound on its way to the device: SDL's stream, which the
/// device takes its buffer from, and that buffer's size.
pub(crate) struct Stream {
    stream: sdl3::audio::AudioStreamOwner,
    /// The block size asked for, which the device may round.
    blocksize: u16,
    /// The device's buffer as SDL says it opened it, in frames of the
    /// machine's rate, as the stream's queue counts them.
    device_frames: usize,
}

/// The rate the machine mixes at, in stereo 16-bit samples.
const FREQ: i32 = 44100;

fn spec() -> sdl3::audio::AudioSpec {
    sdl3::audio::AudioSpec::new(Some(FREQ), Some(2), Some(sdl3::audio::AudioFormat::s16_sys()))
}

/// The sound device, playing, with a buffer of `blocksize` frames (SDL's
/// default is about 46 ms at 44.1 kHz), through a stream of its own. SDL
/// takes the size from a hint it reads when it opens the device, which
/// may round it; the device's buffer is what SDL says it opened.
fn open_audio(subsystem: &sdl3::AudioSubsystem, blocksize: u16) -> Result<Stream, String> {
    sdl3::hint::set("SDL_AUDIO_DEVICE_SAMPLE_FRAMES", &blocksize.to_string());
    let stream = subsystem
        .default_playback_device()
        .open_device_stream(Some(&spec()))
        .map_err(|e| e.to_string())?;
    stream.resume().map_err(|e| e.to_string())?;
    let mut format = spec().into();
    let mut frames = 0;
    // SAFETY: the stream is open, and SDL only fills in the two values.
    let known = unsafe {
        let device = sdl3::sys::audio::SDL_GetAudioStreamDevice(stream.raw());
        sdl3::sys::audio::SDL_GetAudioDeviceFormat(device, &mut format, &mut frames)
    };
    // The device may run at another rate (48 kHz): its buffer drains as
    // fast as that many of the machine's frames.
    let device_frames = if known && frames > 0 && format.freq > 0 {
        (frames as i64 * FREQ as i64 / format.freq as i64) as usize
    } else {
        blocksize as usize
    };
    Ok(Stream { stream, blocksize, device_frames })
}

impl audio::AudioOutput for SdlAudio {
    fn name(&self) -> &str {
        self.subsystem.current_audio_driver()
    }

    /// The frames waiting in the stream, in the machine's format: SDL
    /// counts them before it converts them for the device.
    fn queued_frames(&self) -> usize {
        let Some(stream) = &self.stream else { return 0 };
        stream.stream.queued_bytes().unwrap_or(0).max(0) as usize / 4
    }

    fn queue(&mut self, samples: &[i16]) -> Result<(), String> {
        use display::audio_mix::{IDENTITY, apply};
        // The sound is dropped: set_buffer said why there is no device,
        // and an error here would be printed for every video frame.
        let Some(stream) = &self.stream else { return Ok(()) };
        let mix = self.mix.get();
        if mix == IDENTITY && self.mixed == IDENTITY {
            return stream.stream.put_data_i16(samples).map_err(|e| e.to_string());
        }
        apply(samples, self.mixed, mix, &mut self.buffer);
        self.mixed = mix;
        stream.stream.put_data_i16(&self.buffer).map_err(|e| e.to_string())
    }

    /// The device takes its buffer from the stream at once; the prebuffer
    /// is for a video frame that comes late.
    fn target_frames(&self) -> usize {
        self.stream.as_ref().map_or(0, |s| s.device_frames) + self.prebuffer
    }

    /// A new block size opens the device again, which drops the little
    /// sound waiting in it. The old stream closes first: SDL shares one
    /// device among its streams, and it would keep the old size while one
    /// is open (the 3D scene's too, which closes as well). If the new size
    /// won't open, the old one is tried again.
    fn set_buffer(&mut self, buffer: rust_dos::audio::AudioBuffer) -> Result<(), String> {
        let old = self.stream.as_ref().map(|s| s.blocksize);
        if old != Some(buffer.blocksize) {
            self.room.borrow_mut().take();
            self.stream = None;
            match open_audio(&self.subsystem, buffer.blocksize) {
                Ok(stream) => self.stream = Some(stream),
                Err(e) => {
                    self.stream = old.and_then(|size| open_audio(&self.subsystem, size).ok());
                    return Err(e);
                }
            }
        }
        self.prebuffer = buffer.prebuffer_frames(FREQ as usize);
        Ok(())
    }
}

/// What `open_output` chose.
pub(crate) enum Output {
    Sdl(Stream, sdl3::AudioSubsystem),
    /// The null output; the error is why SDL's device couldn't be opened,
    /// if it was tried.
    Null(Option<String>),
}

impl Output {
    /// SDL's audio, for the devices of the 3D scene's own sounds.
    pub(crate) fn subsystem(&self) -> Option<sdl3::AudioSubsystem> {
        match self {
            Output::Sdl(_, subsystem) => Some(subsystem.clone()),
            Output::Null(_) => None,
        }
    }
}

/// Open the sound output `settings` choose. `headless` is a run with no
/// window to go with the sound.
pub(crate) fn open_output(sdl: &sdl3::Sdl, settings: &Settings, headless: bool) -> Output {
    // (SDL takes an empty value as no value, and picks a device itself.
    // SDL3 reads SDL_AUDIO_DRIVER, or the older SDL_AUDIODRIVER when it is
    // unset.)
    let driver_named = ["SDL_AUDIO_DRIVER", "SDL_AUDIODRIVER"]
        .iter()
        .any(|var| std::env::var_os(var).is_some_and(|name| !name.is_empty()));
    if !settings.audio_output.wants_sdl(headless, driver_named) {
        return Output::Null(None);
    }
    let device = sdl.audio().map_err(|e| e.to_string()).and_then(|subsystem| {
        let stream = open_audio(&subsystem, settings.audio_buffer.blocksize)?;
        Ok((stream, subsystem))
    });
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
    room: &RoomDevice,
) {
    match output {
        Output::Sdl(stream, subsystem) => {
            let driver = subsystem.current_audio_driver();
            if driver.eq_ignore_ascii_case("dummy") {
                warn_dummy(cpu);
            }
            cpu.bus
                .log_string(&format!("[AUDIO] Sound output: SDL's {} driver", driver));
            let mix = audio_mix.clone();
            let prebuffer = settings.audio_buffer.prebuffer_frames(FREQ as usize);
            let output = SdlAudio {
                stream: Some(stream),
                subsystem,
                prebuffer,
                mix,
                mixed: display::audio_mix::IDENTITY,
                buffer: Vec::new(),
                room: room.device.clone(),
            };
            cpu.bus.audio_device = Some(Box::new(output));
        }
        Output::Null(error) => {
            if let Some(e) = error {
                eprintln!("[AUDIO] No sound device, so no sound: {}", e);
                cpu.bus
                    .log_string(&format!("[AUDIO] No sound device, so no sound: {}", e));
            }
            cpu.bus.log_string("[AUDIO] Sound output: the null output");
            cpu.bus.audio_device = Some(Box::new(NullAudio::new(settings.audio_buffer)));
        }
    }
}

/// SDL's dummy driver doesn't keep real time, so the queue may run dry
/// however well the host keeps up. Said on stderr too, where a measuring
/// script sees it.
fn warn_dummy(cpu: &mut Cpu) {
    let warning = "[AUDIO] WARNING: SDL's dummy audio driver is active. It doesn't keep real \
                   time, so underrun counts mean nothing, and no measurement may use it. \
                   Use a real driver (SDL_AUDIO_DRIVER=wasapi on Windows) with --mute, or \
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
    /// Shared with SdlAudio, which closes it to change the block size.
    device: RoomSlot,
    /// Whether the device plays (it opens paused).
    playing: bool,
    /// It couldn't be opened: not tried again.
    failed: bool,
}

pub(crate) struct RoomCallback {
    sound: std::sync::Arc<std::sync::Mutex<display::room_sound::RoomSound>>,
    buffer: Vec<i16>,
}

impl sdl3::audio::AudioCallback<i16> for RoomCallback {
    /// SDL asks for `requested` samples more.
    fn callback(&mut self, stream: &mut sdl3::audio::AudioStream, requested: i32) {
        self.buffer.clear();
        self.buffer.resize(requested.max(0) as usize, 0);
        self.sound.lock().unwrap_or_else(|e| e.into_inner()).mix_into(&mut self.buffer);
        let _ = stream.put_data_i16(&self.buffer);
    }
}

/// Where the 3D scene's sound stream is, while it is open.
type RoomSlot = Rc<std::cell::RefCell<Option<RoomStream>>>;

/// The 3D scene's sound stream.
struct RoomStream(sdl3::audio::AudioStreamWithCallback<RoomCallback>);

impl Drop for RoomStream {
    /// The sdl3 crate frees the callback before it destroys the stream,
    /// and SDL's audio thread may call it in between. Taking the callback
    /// off first waits for one running to finish (SDL does it under the
    /// stream's lock), and none starts after.
    fn drop(&mut self) {
        // SAFETY: the stream is alive until the field drops, after this.
        unsafe {
            sdl3::sys::audio::SDL_SetAudioStreamGetCallback(
                self.0.raw(),
                None,
                std::ptr::null_mut(),
            )
        };
    }
}

impl RoomDevice {
    pub fn new() -> Self {
        let device = Default::default();
        RoomDevice { sound: Default::default(), device, playing: false, failed: false }
    }

    /// Play while there is something to hear, on `subsystem`'s device.
    pub fn update(&mut self, subsystem: Option<&sdl3::AudioSubsystem>) {
        let active = self.sound.lock().unwrap_or_else(|e| e.into_inner()).active();
        let mut slot = self.device.borrow_mut();
        if active
            && slot.is_none()
            && !self.failed
            && let Some(subsystem) = subsystem
        {
            // (SDL converts to the device's own rate and format.)
            let callback = RoomCallback { sound: self.sound.clone(), buffer: Vec::new() };
            let device = subsystem.default_playback_device();
            match device.open_playback_stream_with_callback(&spec(), callback) {
                Ok(device) => {
                    *slot = Some(RoomStream(device));
                    self.playing = false;
                }
                Err(e) => {
                    eprintln!("[AUDIO] No sound device for the 3D scene's sounds: {}", e);
                    self.failed = true;
                }
            }
        }
        let Some(RoomStream(device)) = &*slot else {
            self.sound.lock().unwrap_or_else(|e| e.into_inner()).clear();
            return;
        };
        if active != self.playing {
            let done = if active { device.resume() } else { device.pause() };
            self.playing = if done.is_ok() { active } else { self.playing };
        }
    }
}
