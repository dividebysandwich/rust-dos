use crate::bus::Bus;

/// Where the mixed output goes: the SDL window's sound device, or the
/// browser's. Samples are 44.1 kHz stereo, interleaved.
pub trait AudioOutput {
    /// Stereo frames queued and not played yet.
    fn queued_frames(&self) -> usize;
    /// Queue samples behind those waiting.
    fn queue(&mut self, samples: &[i16]) -> Result<(), String>;
    /// The frames to keep queued when the next video frame's sound comes:
    /// enough that the device doesn't run dry while it waits, and no more,
    /// as each is latency.
    fn target_frames(&self) -> usize;
}

/// The BEL character's beep: 200 ms of 880 Hz, mixed into the output.
pub fn play_sdl_beep(bus: &mut Bus) {
    bus.audio_catch_up();
    bus.beep_frames = crate::opl::RATE / 5;
}

/// How far the resampling in `Feed` stretches or squeezes the sound to
/// bring the queue back to its target: by half the queue's distance from
/// it a second, and at most half a percent, about a twelfth of a semitone.
const MAX_DRIFT: f64 = 0.005;
const DRIFT_PER_SECOND: f64 = 0.5;

/// What `pump_audio` keeps between video frames to hold the device's queue
/// at its target. Emulated time follows the wall clock, but not exactly
/// the sound device's clock, and a slow frame brings the sound of several.
/// Sound that comes too late for the target is dropped at once, and a
/// queue that drifts away from it is brought back by playing the sound a
/// little faster or slower. The sound handed on for recordings stays as
/// it was.
#[derive(Clone, Debug)]
pub struct Feed {
    /// The queue's level when the sound comes, averaged over a few frames;
    /// none after the queue was padded or sound dropped.
    level: Option<f64>,
    /// Frames of input per frame of output: 1 unless drifting back to the
    /// target.
    step: f64,
    /// Where in the new sound the next output frame is, in frames: -1 to 0
    /// is between the last frame of the previous sound and the first.
    pos: f64,
    last: (i16, i16),
}

impl Default for Feed {
    fn default() -> Self {
        Self { level: None, step: 1.0, pos: 0.0, last: (0, 0) }
    }
}

impl Feed {
    /// The resampling toward `target` for a queue of `queued` frames:
    /// starting when the average strays more than 5 ms from it, ending
    /// when it is back within about a millisecond.
    fn steer(&mut self, queued: usize, target: usize, rate: usize) {
        let level = self.level.map_or(queued as f64, |level| level + (queued as f64 - level) * 0.05);
        self.level = Some(level);
        let off = (level - target as f64) / rate as f64;
        if off.abs() > 0.005 || (self.step != 1.0 && off.abs() > 0.001) {
            self.step = 1.0 + (off * DRIFT_PER_SECOND).clamp(-MAX_DRIFT, MAX_DRIFT);
        } else {
            self.stop_drifting();
        }
    }

    /// Back to the sound as it is, on a whole frame.
    fn stop_drifting(&mut self) {
        self.step = 1.0;
        self.pos = self.pos.round();
    }

    /// Append `input` (stereo, interleaved) to `out`, resampled by `step`
    /// with linear interpolation. The last input frame waits for the next
    /// call, which interpolates from it; at a step of 1 on a whole frame,
    /// the sound goes as it is.
    fn resample(&mut self, input: &[i16], out: &mut Vec<i16>) {
        let frames = input.len() / 2;
        if frames == 0 {
            return;
        }
        let frame = |i: isize| if i < 0 { self.last } else { (input[i as usize * 2], input[i as usize * 2 + 1]) };
        if self.step == 1.0 && self.pos.fract() == 0.0 {
            if self.pos < 0.0 {
                out.extend_from_slice(&[self.last.0, self.last.1]);
            }
            out.extend_from_slice(input);
            self.last = frame(frames as isize - 1);
            self.pos = 0.0;
            return;
        }
        let lerp = |a: i16, b: i16, t: f64| (a as f64 + (b as f64 - a as f64) * t).round() as i16;
        let mut pos = self.pos;
        while pos < (frames - 1) as f64 {
            let i = pos.floor();
            let (t, i) = (pos - i, i as isize);
            let (a, b) = (frame(i), frame(i + 1));
            out.push(lerp(a.0, b.0, t));
            out.push(lerp(a.1, b.1, t));
            pos += self.step;
        }
        self.last = frame(frames as isize - 1);
        self.pos = pos - frames as f64;
    }
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
        let feed = &mut bus.audio_feed;
        let queued = device.queued_frames();
        let target = device.target_frames();
        let rate = crate::opl::RATE as usize;
        let frames = samples.len() / 2;
        // Running dry: silence up to the target.
        let mut pad = 0;
        if queued < target / 4 {
            pad = target - queued;
            if !idle {
                bus.audio_underruns += 1;
            }
        }
        // More than two frames' sound beyond the target, as after a slow
        // frame: the oldest is too late, and goes, down to a frame's. The
        // sound rather than silence fills the queue back up.
        let mut drop = 0;
        if queued + pad + frames > target + rate / 30 {
            let keep = (target + frames.min(rate / 60)).saturating_sub(queued);
            drop = frames - frames.min(keep);
            pad = keep - (frames - drop);
        }
        if pad > 0 || drop > 0 {
            feed.level = None;
            feed.stop_drifting();
        } else {
            feed.steer(queued, target, rate);
        }
        let mut out = vec![0; pad * 2];
        feed.resample(&samples[drop * 2..], &mut out);
        if bus.mixer.muted || bus.mixer.fast_forward {
            out.fill(0);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A sound device playing `rate` frames a second of wall time, for
    /// `pump_audio` called at 60 frames a second.
    struct Device {
        queued: usize,
        target: usize,
        played: Vec<i16>,
    }

    impl AudioOutput for std::rc::Rc<std::cell::RefCell<Device>> {
        fn queued_frames(&self) -> usize {
            self.borrow().queued
        }

        fn queue(&mut self, samples: &[i16]) -> Result<(), String> {
            let mut device = self.borrow_mut();
            device.queued += samples.len() / 2;
            device.played.extend_from_slice(samples);
            Ok(())
        }

        fn target_frames(&self) -> usize {
            self.borrow().target
        }
    }

    #[test]
    fn resampling_at_one_passes_the_sound_on_as_it_is() {
        let mut feed = Feed::default();
        let mut out = Vec::new();
        feed.resample(&[1, -1, 2, -2, 3, -3], &mut out);
        feed.resample(&[4, -4, 5, -5], &mut out);
        assert_eq!(out, [1, -1, 2, -2, 3, -3, 4, -4, 5, -5]);

        // Back from drifting, the frame it waited with first.
        feed.step = 1.0025;
        feed.resample(&[6, -6, 7, -7], &mut out);
        feed.stop_drifting();
        feed.resample(&[8, -8], &mut out);
        assert_eq!(out[10..], [6, -6, 7, -7, 8, -8]);
    }

    #[test]
    fn resampling_faster_plays_fewer_frames() {
        let mut feed = Feed { step: 1.005, ..Feed::default() };
        let input: Vec<i16> = (0..2000).flat_map(|i| [i as i16, -(i as i16)]).collect();
        let mut out = Vec::new();
        feed.resample(&input, &mut out);
        assert_eq!(out.len() / 2, (1999.0f64 / 1.005).ceil() as usize);
        // Linear: the sound's ramp is still a ramp.
        assert_eq!(out[2 * 1000], (1000.0f64 * 1.005).round() as i16);
    }

    fn pump(bus: &mut Bus, device: &std::rc::Rc<std::cell::RefCell<Device>>, frames: usize, played: usize) {
        bus.audio_out.extend(std::iter::repeat_n(100i16, frames * 2));
        let mut d = device.borrow_mut();
        d.queued = d.queued.saturating_sub(played);
        drop(d);
        pump_audio(bus, false);
    }

    #[test]
    fn the_queue_holds_its_target_through_a_slow_frame_and_drift() {
        let mut bus = Bus::new(std::path::PathBuf::from("."));
        let device = std::rc::Rc::new(std::cell::RefCell::new(Device { queued: 0, target: 1400, played: Vec::new() }));
        bus.audio_device = Some(Box::new(device.clone()));
        // `audio_catch_up` renders nothing on its own: emulated time stands.
        pump(&mut bus, &device, 735, 0);
        assert_eq!(device.borrow().queued, 1400 + 735);
        assert_eq!(bus.audio_underruns, 1);
        for _ in 0..60 {
            pump(&mut bus, &device, 735, 735);
        }
        assert_eq!(device.borrow().queued, 1400 + 735);

        // A frame 100 ms late: the device plays what it had, then waits,
        // and the sound of those 100 ms comes at once.
        pump(&mut bus, &device, 735 + 4410, 4410 + 735);
        let queued = device.borrow().queued;
        assert!(queued <= 1400 + 735 + 1, "{queued}");
        assert_eq!(bus.audio_underruns, 2);

        // The device's clock 0.14% slower than emulated time: the queue
        // stays within 6 ms of its target, 5 and the average's lag.
        for _ in 0..3600 {
            pump(&mut bus, &device, 735, 734);
            let queued = device.borrow().queued - 735;
            assert!((1400 - 44..1400 + 265).contains(&queued), "{queued}");
        }
    }
}
