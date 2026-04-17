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

        // Generate Audio — mix PC speaker square wave + AdLib FM + Sound
        // Blaster 8-bit PCM. AdLib sums 9 channels each in roughly [-1, 1];
        // scale to roughly match PC speaker loudness and clamp to i16 after
        // mixing. The SB PCM path pulls one byte from DMA memory whenever
        // its phase accumulator overflows — see `SoundBlaster::advance_one`.
        const ADLIB_GAIN: f32 = 1400.0;
        const SB_PCM_GAIN: f32 = 0.75;

        // Split the borrows up front. The SB PCM path needs a &[u8] view of
        // ram plus &mut references to sb and dma_ch1 on every sample; the
        // AdLib path needs &mut adlib. Taking them here keeps the hot
        // inner loop free of repeated field-access borrow checks.
        let ram_ptr = bus.ram.as_ptr();
        let ram_len = bus.ram.len();
        // SAFETY: single-threaded emulator; ram is a fixed 1 MiB Vec that
        // outlives this function. The bus mutable borrows we take next
        // don't resize or drop it. Same guarantee as the decoder slice in
        // main.rs.
        let ram_view: &[u8] = unsafe { std::slice::from_raw_parts(ram_ptr, ram_len) };

        for _ in 0..needed {
            let speaker = if bus.speaker_on && frequency > 20.0 {
                bus.audio_phase += phase_step;
                if bus.audio_phase >= 1.0 {
                    bus.audio_phase -= 1.0;
                }
                if bus.audio_phase < 0.5 { VOLUME } else { -VOLUME }
            } else {
                0
            };

            let adlib = (bus.adlib.render_sample() * ADLIB_GAIN) as i32;
            let sb_sample = bus.sb.advance_one(ram_view, &mut bus.dma_ch1, SAMPLE_RATE as u32);
            let sb = if bus.sb.speaker_on {
                (sb_sample as f32 * SB_PCM_GAIN) as i32
            } else {
                0
            };
            let mixed =
                (speaker as i32 + adlib + sb).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            buffer.push(mixed);
        }

        if let Err(e) = device.queue_audio(&buffer) {
            eprintln!("[AUDIO] Queue error: {}", e);
        }
    }
}