use crate::bus::Bus;

/// The BEL character's beep: 200 ms of 880 Hz, mixed into the output.
pub fn play_sdl_beep(bus: &mut Bus) {
    bus.audio_catch_up();
    bus.beep_frames = crate::opl::RATE / 5;
}

/// Hand the audio rendered since the last call to the host: the output
/// device and the debug server's audio stream. Called once per video frame.
pub fn pump_audio(bus: &mut Bus) {
    bus.audio_catch_up();
    let samples: Vec<i16> = bus.audio_out.drain(..).collect();
    let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    bus.audio_peak = bus.audio_peak.max(peak);
    if let Some(device) = &mut bus.audio_device {
        // Emulated time runs close to the wall clock, so the queue stays
        // near its level; bound it both ways. Too little and the device
        // runs dry between frames: pad with silence. Too much is latency.
        let queued = device.size() as usize / 4;
        let rate = crate::opl::RATE as usize;
        let mut out = Vec::with_capacity(samples.len() + rate / 10);
        if queued < rate / 50 {
            out.resize((rate * 3 / 40 - queued) * 2, 0);
            bus.audio_underruns += 1;
        }
        let room = (rate / 4).saturating_sub(queued) * 2;
        out.extend_from_slice(&samples[..samples.len().min(room)]);
        if !out.is_empty()
            && let Err(e) = device.queue_audio(&out)
        {
            eprintln!("[AUDIO] Queue error: {}", e);
        }
    }
    if let Some(hook) = &mut bus.audio_hook {
        hook(&samples);
    }
}
