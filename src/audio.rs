use crate::bus::Bus;

/// Where the mixed output goes: the SDL window's sound device, or the
/// browser's. Samples are 44.1 kHz stereo, interleaved.
pub trait AudioOutput {
    /// Stereo frames queued and not played yet.
    fn queued_frames(&self) -> usize;
    /// Queue samples behind those waiting.
    fn queue(&mut self, samples: &[i16]) -> Result<(), String>;
}

/// The BEL character's beep: 200 ms of 880 Hz, mixed into the output.
pub fn play_sdl_beep(bus: &mut Bus) {
    bus.audio_catch_up();
    bus.beep_frames = crate::opl::RATE / 5;
}

/// Hand the audio rendered since the last call to the host: the output
/// device, which gets silence while the mixer is muted, and the debug
/// server's audio stream. Called once per video frame; `idle` while the
/// machine waits (paused, or in the settings window), when the device
/// running dry isn't an underrun. Returns the samples, for recordings,
/// which the mute leaves alone.
pub fn pump_audio(bus: &mut Bus, idle: bool) -> Vec<i16> {
    bus.audio_catch_up();
    let samples: Vec<i16> = bus.audio_out.drain(..).collect();
    let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    bus.audio_peak = bus.audio_peak.max(peak);
    if let Some(device) = &mut bus.audio_device {
        // Emulated time runs close to the wall clock, so the queue stays
        // near its level; bound it both ways. Too little and the device
        // runs dry between frames: pad with silence. Too much is latency.
        let queued = device.queued_frames();
        let rate = crate::opl::RATE as usize;
        let mut out = Vec::with_capacity(samples.len() + rate / 10);
        if queued < rate / 50 {
            out.resize((rate * 3 / 40 - queued) * 2, 0);
            if !idle {
                bus.audio_underruns += 1;
            }
        }
        let room = (rate / 4).saturating_sub(queued) * 2;
        let take = samples.len().min(room);
        if bus.mixer.muted {
            out.resize(out.len() + take, 0);
        } else {
            out.extend_from_slice(&samples[..take]);
        }
        if !out.is_empty()
            && let Err(e) = device.queue(&out)
        {
            eprintln!("[AUDIO] Queue error: {}", e);
        }
    }
    if let Some(hook) = &mut bus.audio_hook {
        hook(&samples);
    }
    samples
}
