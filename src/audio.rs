use crate::bus::Bus;

// A standard PC Speaker beep is often around 880Hz for 0.25 seconds
pub fn play_sdl_beep(bus: &mut Bus) {
    if let Some(device) = &mut bus.audio_device {
        // Check if we are already playing (prevent stacking beeps indefinitely)
        if device.size() > 0 {
            return;
        }

        // Generate Square Wave Samples
        let sample_rate = 44100;
        let frequency = 880.0;
        let duration_ms = 200;
        let volume = 3000; // Max is 32767

        let samples_count = (sample_rate * duration_ms) / 1000;
        let period = sample_rate as f32 / frequency;

        let mut buffer = Vec::with_capacity(samples_count as usize);

        for i in 0..samples_count {
            // Square wave logic: High for first half of period, Low for second half
            let t = i as f32 % period;
            let sample = if t < period / 2.0 { volume } else { -volume };
            buffer.push(sample);
        }

        // Queue the audio
        // The device will play this buffer and then go silent automatically
        device.queue_audio(&buffer).unwrap();
        device.resume(); // Ensure playback is active
    }
}

pub fn pump_audio(bus: &mut Bus) {
    if let Some(device) = &mut bus.audio_device {
        // Maintain a small buffer target (e.g., 4096 samples ~90ms)
        // If we let it drain to 0, audio stutters. If we fill it too much, audio lags.
        if device.size() >= 4096 {
            return;
        }

        let sample_rate = 44100.0;
        let volume = 3000;
        
        // Generate ~1 frame worth of audio (plus a bit to be safe)
        // 16ms @ 44100Hz is ~700 samples. We generate 1024.
        let samples_to_generate = 1024;
        let mut buffer = Vec::with_capacity(samples_to_generate);

        for _ in 0..samples_to_generate {
            let sample = if bus.speaker_on {
                // Calculate Frequency: Clock / Divisor
                // The PC Clock is 1.193182 MHz
                let divisor = if bus.pit_divisor == 0 { 65536 } else { bus.pit_divisor as u32 };
                let freq = 1193182.0 / divisor as f32;

                // Simple Square Wave Generation
                let period = sample_rate / freq;
                
                // Update Phase
                bus.audio_phase = (bus.audio_phase + 1.0) % period;

                if bus.audio_phase < (period / 2.0) { volume } else { -volume }
            } else {
                0 // Silence
            };
            buffer.push(sample);
        }

        device.queue_audio(&buffer).unwrap();
    }
}