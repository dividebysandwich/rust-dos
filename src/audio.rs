use crate::bus::Bus;

const SAMPLE_RATE: f32 = 44100.0;
const VOLUME: i16 = 3000;
const BASE_FREQ: f32 = 1_193_182.0;

// Helper for System Beep (INT 10,07)
pub fn play_sdl_beep(bus: &mut Bus) {
    if let Some(device) = &mut bus.audio_device {
        if device.size() > 0 { return; }

        let frequency = 880.0;
        let duration_ms = 200;
        let samples_count = (SAMPLE_RATE as u32 * duration_ms) / 1000;
        
        let mut buffer = Vec::with_capacity(samples_count as usize);
        let mut phase = 0.0;
        let step = frequency / SAMPLE_RATE;

        for _ in 0..samples_count {
            phase += step;
            if phase >= 1.0 { phase -= 1.0; }
            let sample = if phase < 0.5 { VOLUME } else { -VOLUME };
            buffer.push(sample);
        }

        if let Err(e) = device.queue_audio(&buffer) {
            eprintln!("[AUDIO] Beep queue error: {}", e);
        }
        device.resume();
    }
}

pub fn pump_audio(bus: &mut Bus) {
    if let Some(device) = &mut bus.audio_device {
        let current_bytes = device.size();
        
        // WBuffer Underrun Detection
        if current_bytes == 0 && bus.speaker_on {
            println!("[AUDIO] Buffer Underrun detected!");
        }

        // Maintain about 50ms of audio (approx 2048 samples).
        let target_samples = 1024*10; 
        let current_samples = current_bytes / 2; // i16 = 2 bytes

        // If we are mostly full, don't add latency.
        if current_samples >= target_samples {
            return;
        }

        let needed = target_samples - current_samples;
        let mut buffer = Vec::with_capacity(needed as usize);
        let divisor = if bus.pit_divisor == 0 { 65536 } else { bus.pit_divisor as u32 };
        let frequency = BASE_FREQ / divisor as f32;
        let phase_step = frequency / SAMPLE_RATE;

        // Generate Audio
        for _ in 0..needed {
            // Filter out low frequencies (< 20Hz)
            let sample = if bus.speaker_on && frequency > 20.0 {
                
                // Advance Phase
                bus.audio_phase += phase_step;
                
                // Wrap Phase (Normalized 0.0 to 1.0)
                if bus.audio_phase >= 1.0 {
                    bus.audio_phase -= 1.0;
                }

                // Square Wave
                if bus.audio_phase < 0.5 { VOLUME } else { -VOLUME }
            } else {
                0 // Silence
            };
            
            buffer.push(sample);
        }

        if let Err(e) = device.queue_audio(&buffer) {
            eprintln!("[AUDIO] Queue error: {}", e);
        }
    }
}