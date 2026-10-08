//! What the headset's session did, in `vr.log` in Rust-DOS's folder as
//! well as on the console: for finding out, with a headset nobody watches
//! the console of, what its runtime takes and how it keeps up. The log of
//! the run before is kept as `vr.log.1`.

use std::fs::File;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

struct Log {
    file: Option<File>,
    start: Instant,
}

static LOG: OnceLock<Mutex<Log>> = OnceLock::new();

fn log() -> &'static Mutex<Log> {
    LOG.get_or_init(|| {
        let file = rust_dos::config::user_dir().and_then(|dir| {
            let path = dir.join("vr.log");
            let _ = std::fs::rename(&path, dir.join("vr.log.1"));
            File::create(path).ok()
        });
        Mutex::new(Log { file, start: Instant::now() })
    })
}

/// Say `line`, with `[VR]` before it on the console, and the seconds since
/// the first line in the file.
pub fn line(line: impl AsRef<str>) {
    let line = line.as_ref();
    eprintln!("[VR] {}", line);
    let mut log = log().lock().unwrap_or_else(|e| e.into_inner());
    let seconds = log.start.elapsed().as_secs_f32();
    if let Some(file) = &mut log.file {
        let _ = writeln!(file, "{:9.3} {}", seconds, line);
        let _ = file.flush();
    }
}

/// How the headset's frames keep up, said every `PERIOD`.
pub struct FrameStats {
    since: Instant,
    /// When the last wait for a frame ended.
    last: Option<Instant>,
    frames: u32,
    intervals: Duration,
    longest: Duration,
    late: u32,
    skipped: u32,
    /// From beginning the frame to ending it.
    work: Duration,
    /// The headset's frame period, as the runtime predicts it.
    period: Duration,
}

impl Default for FrameStats {
    fn default() -> Self {
        FrameStats {
            since: Instant::now(),
            last: None,
            frames: 0,
            intervals: Duration::ZERO,
            longest: Duration::ZERO,
            late: 0,
            skipped: 0,
            work: Duration::ZERO,
            period: Duration::ZERO,
        }
    }
}

impl FrameStats {
    pub const PERIOD: Duration = Duration::from_secs(10);

    /// A wait for a frame ended at `now`, the headset showing a frame every
    /// `period`; it is to be drawn or not (`render`).
    pub fn waited(&mut self, now: Instant, period: Duration, render: bool) {
        if let Some(last) = self.last {
            let interval = now.saturating_duration_since(last);
            self.intervals += interval;
            self.longest = self.longest.max(interval);
            // Half a frame more than one: one was missed.
            if !period.is_zero() && interval > period * 3 / 2 {
                self.late += 1;
            }
        }
        self.last = Some(now);
        self.frames += 1;
        self.period = period;
        if !render {
            self.skipped += 1;
        }
    }

    /// The frame took `work` from beginning to ending it.
    pub fn worked(&mut self, work: Duration) {
        self.work += work;
    }

    /// What to say about the frames since the last, at `now`, every
    /// `PERIOD`; the counts start again.
    pub fn report(&mut self, now: Instant) -> Option<String> {
        if now.saturating_duration_since(self.since) < Self::PERIOD || self.frames < 2 {
            return None;
        }
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let frames = self.frames;
        let line = format!(
            "{} frames: {:.2} ms apart on average (the headset's {:.2}), {:.2} longest, {} late, {} not drawn, {:.2} ms drawing each",
            frames,
            ms(self.intervals) / (frames - 1) as f64,
            ms(self.period),
            ms(self.longest),
            self.late,
            self.skipped,
            ms(self.work) / frames as f64,
        );
        *self = FrameStats { since: now, last: self.last, ..FrameStats::default() };
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_frames_are_counted() {
        let start = Instant::now();
        let mut stats = FrameStats { since: start, ..FrameStats::default() };
        let period = Duration::from_micros(11_111);
        let mut at = start;
        for i in 0..900 {
            // Every hundredth frame, one is missed.
            at += if i % 100 == 99 { period * 2 } else { period };
            stats.waited(at, period, i != 0);
            stats.worked(Duration::from_millis(4));
        }
        let line = stats.report(at).expect("ten seconds");
        assert!(line.starts_with("900 frames: "), "{}", line);
        // 908 periods over 899 intervals.
        assert!(line.contains("11.22 ms apart"), "{}", line);
        assert!(line.contains("(the headset's 11.11), 22.22 longest, 9 late, 1 not drawn, 4.00 ms"), "{}", line);
        // Counted again from there.
        assert_eq!(stats.report(at), None);
        assert_eq!(stats.frames, 0);
    }
}
