//! The sampler thread: it wakes a few thousand times a second, reads the
//! emulation thread's word (`WORD`) and counts it. Each read is one sample
//! of where the emulation thread's time went.
//!
//! The waits between reads vary at random around the interval asked for,
//! so that sampling can't fall into step with something periodic in the
//! machine, such as its frames. How long the waits really took is
//! measured and reported: a sleep's length depends on the host's timer
//! resolution.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{Loc, WORD, ZONES};

/// How the sampler waits between reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timer {
    /// Sleep: cheap. A sleep takes longer than asked (on Windows about
    /// a third of a millisecond longer), so the sampler asks for less by
    /// what its sleeps have overslept lately, which keeps the rate.
    Sleep,
    /// Spin on the clock: exact, but keeps a host core busy.
    Spin,
}

impl Timer {
    pub fn name(self) -> &'static str {
        match self {
            Timer::Sleep => "sleep",
            Timer::Spin => "spin",
        }
    }
}

/// The longest a profile's timeline goes, in seconds.
const TIMELINE_MAX: usize = 3600;

/// The waits' lengths are counted in buckets of this many microseconds,
/// the last one for all the longer waits.
const WAIT_BUCKET_US: u64 = 25;
const WAIT_BUCKETS: usize = 800;

/// What the sampler counted.
#[derive(Clone, Debug, Default)]
pub struct Samples {
    /// Samples by the word read.
    pub counts: HashMap<u64, u64>,
    pub total: u64,
    /// When the sampler took its first and last sample.
    pub first: Option<Instant>,
    pub last: Option<Instant>,
    /// The waits between samples: how many, their sum, shortest and longest
    /// (µs), and a histogram of their lengths.
    pub waits: u64,
    pub wait_sum_us: u64,
    pub wait_min_us: u64,
    pub wait_max_us: u64,
    pub wait_hist: Vec<u64>,
    /// Samples by zone in each second since the first.
    pub timeline: Vec<[u32; ZONES]>,
}

impl Samples {
    fn add(&mut self, loc: Loc, now: Instant) {
        *self.counts.entry(loc.0).or_insert(0) += 1;
        self.total += 1;
        let first = *self.first.get_or_insert(now);
        if let Some(last) = self.last {
            let us = now.duration_since(last).as_micros() as u64;
            self.waits += 1;
            self.wait_sum_us += us;
            self.wait_min_us = if self.waits == 1 { us } else { self.wait_min_us.min(us) };
            self.wait_max_us = self.wait_max_us.max(us);
            if self.wait_hist.is_empty() {
                self.wait_hist = vec![0; WAIT_BUCKETS];
            }
            self.wait_hist[((us / WAIT_BUCKET_US) as usize).min(WAIT_BUCKETS - 1)] += 1;
        }
        self.last = Some(now);
        let second = now.duration_since(first).as_secs() as usize;
        if second < TIMELINE_MAX {
            if self.timeline.len() <= second {
                self.timeline.resize(second + 1, [0; ZONES]);
            }
            self.timeline[second][loc.zone() as usize] += 1;
        }
    }

    /// The samples a second the sampler took.
    pub fn rate(&self) -> f64 {
        match (self.first, self.last) {
            (Some(first), Some(last)) if last > first => self.waits as f64 / last.duration_since(first).as_secs_f64(),
            _ => 0.0,
        }
    }

    /// The wait at percentile `p` (0 to 100), in µs, to the upper end of
    /// its bucket of 25 µs.
    pub fn wait_percentile_us(&self, p: f64) -> u64 {
        let goal = (self.waits as f64 * p / 100.0).ceil() as u64;
        let mut seen = 0;
        for (k, &n) in self.wait_hist.iter().enumerate() {
            seen += n;
            if seen >= goal.max(1) {
                return if k == WAIT_BUCKETS - 1 { self.wait_max_us } else { ((k as u64 + 1) * WAIT_BUCKET_US).min(self.wait_max_us) };
            }
        }
        self.wait_max_us
    }
}

/// A random number generator for the waits (xorshift).
struct Jitter(u64);

impl Jitter {
    /// A factor between 0.5 and 1.5.
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        0.5 + (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// The longest sleep between checks of `stop`: what `Sampler::stop` waits
/// at most at rates under about 150 Hz. Faster rates sleep in one piece.
const STOP_SLICE: Duration = Duration::from_millis(10);

/// The sampler thread, while it runs.
pub struct Sampler {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Sampler {
    /// Start sampling `hz` times a second, into `samples`, until `stop`
    /// is called or `until` has come.
    pub fn start(hz: u32, timer: Timer, until: Option<Instant>, samples: Arc<Mutex<Samples>>) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let interval = Duration::from_secs_f64(1.0 / hz.max(1) as f64);
        let thread = {
            let stop = stop.clone();
            std::thread::Builder::new().name("rust-dos profiler".into()).spawn(move || {
                raise_priority();
                let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
                let mut jitter = Jitter(seed | 1);
                // How much longer than asked the sleeps took, on average.
                let mut oversleep = Duration::ZERO;
                while !stop.load(Ordering::Relaxed) {
                    let wait = interval.mul_f64(jitter.next());
                    match timer {
                        Timer::Sleep => {
                            let ask = wait.saturating_sub(oversleep);
                            let asked_at = Instant::now();
                            // In slices, so that a stop at a low rate
                            // doesn't wait out a long interval.
                            let end = asked_at + ask;
                            loop {
                                let left = end.saturating_duration_since(Instant::now());
                                if left.is_zero() || stop.load(Ordering::Relaxed) {
                                    break;
                                }
                                std::thread::sleep(left.min(STOP_SLICE));
                            }
                            let over = asked_at.elapsed().saturating_sub(ask);
                            oversleep = (oversleep * 7 + over) / 8;
                        }
                        Timer::Spin => {
                            let end = Instant::now() + wait;
                            while Instant::now() < end && !stop.load(Ordering::Relaxed) {
                                std::hint::spin_loop();
                            }
                        }
                    }
                    let now = Instant::now();
                    if until.is_some_and(|u| now >= u) || stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let loc = Loc(WORD.load(Ordering::Relaxed));
                    samples.lock().unwrap_or_else(|e| e.into_inner()).add(loc, now);
                }
            })?
        };
        Ok(Sampler { stop, thread: Some(thread) })
    }

    /// Whether the thread still samples (it ends at its `until`).
    pub fn running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Stop the thread.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Wake up on time: the sampler's thread above the others.
fn raise_priority() {
    #[cfg(windows)]
    // SAFETY: changes the calling thread's own priority.
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Zone;

    #[test]
    fn samples_count_words_waits_and_seconds() {
        let mut s = Samples::default();
        let t0 = Instant::now();
        s.add(Loc::host(Zone::FrameWait), t0);
        s.add(Loc::host(Zone::FrameWait), t0 + Duration::from_micros(500));
        s.add(Loc::host(Zone::VgaRender), t0 + Duration::from_micros(1500));
        s.add(Loc::host(Zone::VgaRender), t0 + Duration::from_millis(1200));
        assert_eq!(s.total, 4);
        assert_eq!(s.counts[&Loc::host(Zone::FrameWait).0], 2);
        assert_eq!((s.waits, s.wait_min_us, s.wait_max_us), (3, 500, 1_198_500));
        assert_eq!(s.timeline.len(), 2);
        assert_eq!(s.timeline[0][Zone::FrameWait as usize], 2);
        assert_eq!(s.timeline[1][Zone::VgaRender as usize], 1);
        assert!((s.rate() - 3.0 / 1.2).abs() < 0.01);
        assert_eq!(s.wait_percentile_us(50.0), 1025, "the bucket of 1000-1025 us");
        assert_eq!(s.wait_percentile_us(100.0), 1_198_500, "the longest, past the last bucket");
    }

    #[test]
    fn jitter_stays_within_half_and_one_and_a_half() {
        let mut j = Jitter(12345);
        for _ in 0..10_000 {
            let f = j.next();
            assert!((0.5..1.5).contains(&f), "{}", f);
        }
    }

    #[test]
    fn a_slow_profile_stops_without_waiting_out_its_interval() {
        let samples = Arc::new(Mutex::new(Samples::default()));
        let mut s = Sampler::start(1, Timer::Sleep, None, samples).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let asked = Instant::now();
        s.stop();
        assert!(asked.elapsed() < Duration::from_millis(250), "stop took {:?}", asked.elapsed());
    }

    #[test]
    fn the_thread_samples_and_stops() {
        let samples = Arc::new(Mutex::new(Samples::default()));
        let mut s = Sampler::start(2000, Timer::Sleep, None, samples.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        s.stop();
        let got = samples.lock().unwrap().clone();
        assert!(got.total > 10, "{} samples in 100 ms at 2000 Hz", got.total);
        assert!(!s.running());
    }
}
