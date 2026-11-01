//! The sound output of a run with no sound device. Nothing is played: the
//! queue empties on a monotonic clock, at exactly the output's 44,100
//! frames a second, so `pump_audio` and its underrun counts work as they
//! do on a real device. SDL's `dummy` audio driver runs fast (see
//! `docs/llm-debugging.md`), so it can't stand in for one.
//!
//! There is no thread and no timer: the queue's level is worked out from
//! the clock each time it is read.

use std::time::{Duration, Instant};

use crate::audio::{AudioBuffer, AudioOutput};

const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// A sound device that plays silently. It takes whole blocks, as SDL's
/// device takes its buffer from the queue at once, so the level a check
/// reads steps down as on a real device and `target_frames` means the same.
pub struct NullAudio {
    /// Time since an arbitrary start; it never goes back.
    clock: Box<dyn Fn() -> Duration>,
    /// When the device started, by `clock`.
    opened: Duration,
    blocksize: usize,
    /// The queue kept on top of the block, in frames.
    prebuffer: usize,
    /// The frames left after the first `taken` blocks.
    queued: usize,
    /// The blocks counted in `queued`.
    taken: u64,
}

impl NullAudio {
    pub fn new(buffer: AudioBuffer) -> Self {
        let start = Instant::now();
        Self::with_clock(buffer, Box::new(move || start.elapsed()))
    }

    /// A device on `clock`, for tests.
    fn with_clock(buffer: AudioBuffer, clock: Box<dyn Fn() -> Duration>) -> Self {
        let opened = clock();
        NullAudio {
            clock,
            opened,
            blocksize: buffer.blocksize as usize,
            prebuffer: prebuffer_frames(buffer),
            queued: 0,
            taken: 0,
        }
    }

    /// The blocks the device has taken by now since it started. The frames
    /// come from the clock's whole time, so no rounding adds up.
    fn blocks_due(&self) -> u64 {
        let elapsed = (self.clock)().saturating_sub(self.opened).as_nanos();
        let frames = elapsed * crate::opl::RATE as u128 / NANOS_PER_SECOND;
        (frames / self.blocksize as u128) as u64
    }

    /// Blocks taken while the queue was empty took nothing from it: the
    /// device played silence, as a real one does, and its clock went on.
    fn left(&self, due: u64) -> usize {
        let taken = (due - self.taken).saturating_mul(self.blocksize as u64);
        self.queued.saturating_sub(usize::try_from(taken).unwrap_or(usize::MAX))
    }
}

fn prebuffer_frames(buffer: AudioBuffer) -> usize {
    crate::opl::RATE as usize * buffer.prebuffer as usize / 1000
}

impl AudioOutput for NullAudio {
    fn name(&self) -> &str {
        "null"
    }

    fn queued_frames(&self) -> usize {
        self.left(self.blocks_due())
    }

    fn queue(&mut self, samples: &[i16]) -> Result<(), String> {
        let due = self.blocks_due();
        self.queued = self.left(due) + samples.len() / 2;
        self.taken = due;
        Ok(())
    }

    fn target_frames(&self) -> usize {
        self.blocksize + self.prebuffer
    }

    /// A new block size starts the device over, which drops what waits in
    /// its queue, as opening SDL's device again does.
    fn set_buffer(&mut self, buffer: AudioBuffer) -> Result<(), String> {
        if self.blocksize != buffer.blocksize as usize {
            self.blocksize = buffer.blocksize as usize;
            self.opened = (self.clock)();
            self.queued = 0;
            self.taken = 0;
        }
        self.prebuffer = prebuffer_frames(buffer);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::pump_audio;
    use crate::bus::Bus;
    use std::cell::Cell;
    use std::rc::Rc;

    const BLOCK: u16 = 512;
    const TARGET: usize = 512 + 882;

    fn buffer() -> AudioBuffer {
        AudioBuffer { blocksize: BLOCK, prebuffer: 20 }
    }

    fn device() -> (NullAudio, Rc<Cell<Duration>>) {
        let time = Rc::new(Cell::new(Duration::ZERO));
        let clock = time.clone();
        (NullAudio::with_clock(buffer(), Box::new(move || clock.get())), time)
    }

    /// The time `frames` take at `rate` frames a second.
    fn frames_at(frames: u64, rate: u64) -> Duration {
        Duration::from_nanos((frames as u128 * NANOS_PER_SECOND / rate as u128) as u64)
    }

    #[test]
    fn it_is_named_and_keeps_the_target_of_a_real_device() {
        let (mut output, _) = device();
        assert_eq!(output.name(), "null");
        assert_eq!(output.target_frames(), TARGET);
        output.set_buffer(AudioBuffer { blocksize: BLOCK, prebuffer: 10 }).unwrap();
        assert_eq!(output.target_frames(), 512 + 441);
    }

    #[test]
    fn the_queue_empties_a_block_at_a_time_at_the_output_rate() {
        let (mut output, time) = device();
        output.queue(&[0; 4096]).unwrap();
        assert_eq!(output.queued_frames(), 2048);
        // Just short of a block's time, and then just at it.
        time.set(frames_at(511, 44_100));
        assert_eq!(output.queued_frames(), 2048);
        time.set(frames_at(512, 44_100) + Duration::from_nanos(1));
        assert_eq!(output.queued_frames(), 1536);
        time.set(frames_at(1024, 44_100) + Duration::from_nanos(1));
        assert_eq!(output.queued_frames(), 1024);
    }

    #[test]
    fn an_hour_plays_exactly_the_output_rate() {
        let (mut output, time) = device();
        let hour = 44_100 * 3600usize;
        output.queue(&vec![0; hour * 2]).unwrap();
        // The queue is read and added to often, which must not add up to
        // any rounding error. (Each second's sound comes in 1000 steps.)
        for ms in 1..=3_600_000u64 {
            time.set(Duration::from_millis(ms));
            if ms % 997 == 0 {
                output.queue(&[]).unwrap();
            }
        }
        // The last whole block of the hour.
        assert_eq!(output.queued_frames(), hour % 512);
    }

    #[test]
    fn a_block_taken_while_the_queue_is_empty_takes_nothing_from_later_sound() {
        let (mut output, time) = device();
        output.queue(&[0; 200]).unwrap();
        time.set(Duration::from_secs(1));
        assert_eq!(output.queued_frames(), 0);
        output.queue(&[0; 200]).unwrap();
        assert_eq!(output.queued_frames(), 100);
    }

    #[test]
    fn a_new_block_size_starts_over_empty() {
        let (mut output, time) = device();
        time.set(Duration::from_secs(1));
        output.queue(&[0; 4096]).unwrap();
        output.set_buffer(AudioBuffer { blocksize: 1024, prebuffer: 20 }).unwrap();
        assert_eq!(output.queued_frames(), 0);
        output.queue(&[0; 4096]).unwrap();
        time.set(Duration::from_secs(1) + frames_at(1025, 44_100));
        assert_eq!(output.queued_frames(), 1024);
    }

    /// The dry checks of `seconds` of `pump_audio` at 60 frames a second,
    /// with the sound of 60 frames a second of emulated time, on a device
    /// whose clock runs at `rate` for the output's 44,100.
    fn dry_checks(rate: u64, seconds: u64) -> u64 {
        let mut bus = Bus::new(std::path::PathBuf::from("."));
        let (output, time) = device();
        bus.audio_device = Some(Box::new(output));
        let start = Duration::from_millis(5);
        for frame in 0..seconds * 60 {
            let wall = frames_at(frame * 735, 44_100) + start;
            time.set(wall.mul_f64(rate as f64 / 44_100.0));
            bus.audio_out.extend(std::iter::repeat_n(100i16, 735 * 2));
            pump_audio(&mut bus, false);
        }
        bus.audio_underruns
    }

    #[test]
    fn a_queue_fed_at_the_right_rate_never_runs_dry() {
        // The one dry check is the first, which finds the queue empty on
        // any device.
        assert_eq!(dry_checks(44_100, 600), 1);
    }

    /// What the null output is for: at SDL's `dummy` driver's speed, the
    /// same feed runs dry again and again.
    #[test]
    fn the_same_feed_runs_dry_on_a_device_as_fast_as_sdl_s_dummy_driver() {
        assert!(dry_checks(44_940, 600) > 50);
    }
}
