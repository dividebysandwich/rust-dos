//! A profile from start to stop: the sampler, the bus's counters, and the
//! machine's running totals at the start, to report what changed in the
//! window.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::counters::Counters;
use super::sampler::{Sampler, Samples, Timer};
use crate::cpu::Cpu;

/// How a profile samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Samples a second asked for.
    pub hz: u32,
    pub timer: Timer,
    /// Stop after this long.
    pub duration: Option<Duration>,
}

impl Default for Options {
    fn default() -> Self {
        Options { hz: DEFAULT_HZ, timer: Timer::Sleep, duration: None }
    }
}

pub const DEFAULT_HZ: u32 = 2000;
/// The rates a profile can sample at.
pub const MIN_HZ: u32 = 10;
pub const MAX_HZ: u32 = 20_000;

/// The machine's running totals at a moment.
#[derive(Clone, Debug)]
pub struct Totals {
    pub at: Instant,
    /// Instructions executed, the instruction count (emulated time), and
    /// what of it the CPU sat halted or stalled on I/O.
    pub executed: u64,
    pub icount: u64,
    pub idle: u64,
    pub stalled: u64,
    pub emulated_ns: u64,
    /// Frames the program drew.
    pub frames_drawn: u64,
    pub decode_hits: u64,
    pub decode_misses: u64,
    pub dynrec: crate::dynrec::DynStats,
    pub calls: Option<crate::dynrec::Calls>,
    /// `autospeed::Activity`: bursts, copies, flips, polls, status reads,
    /// video bytes and I/O bytes.
    pub activity: [u64; 7],
}

impl Totals {
    pub fn take(cpu: &Cpu) -> Self {
        let a = &cpu.bus.activity;
        Totals {
            at: Instant::now(),
            executed: cpu.executed,
            icount: cpu.bus.clock.icount,
            idle: cpu.bus.clock.idle,
            stalled: cpu.bus.clock.stalled,
            emulated_ns: cpu.bus.clock.now_ns(),
            frames_drawn: cpu.bus.frames_drawn,
            decode_hits: cpu.decode_cache.hits,
            decode_misses: cpu.decode_cache.misses,
            dynrec: cpu.dynrec.counts(),
            calls: cpu.dynrec.calls().cloned(),
            activity: [a.bursts, a.copies, a.flips, a.polls, a.status_reads, a.video_bytes, a.io_bytes],
        }
    }
}

/// A profile.
pub struct Session {
    pub options: Options,
    pub started: Instant,
    sampler: Option<Sampler>,
    samples: Arc<Mutex<Samples>>,
    pub start: Totals,
    /// The totals and the counts when it stopped.
    pub end: Option<Totals>,
    counters: Option<Box<Counters>>,
}

impl Session {
    /// Start profiling the machine, on its thread: the bus counts from now
    /// on and translated code is translated again, noting the blocks it
    /// enters.
    pub fn start(cpu: &mut Cpu, options: Options) -> std::io::Result<Session> {
        let options = Options { hz: options.hz.clamp(MIN_HZ, MAX_HZ), ..options };
        let ram_len = cpu.bus.ram().len();
        let counters = Counters::new(ram_len, |port| cpu.bus.port_zone(port));
        cpu.bus.profile = Some(counters);
        cpu.dynrec.flush();
        super::set_active(true);
        super::set_host(super::Zone::Debug);
        let samples = Arc::new(Mutex::new(Samples::default()));
        let started = Instant::now();
        let sampler = match Sampler::start(options.hz, options.timer, options.duration.map(|d| started + d), samples.clone()) {
            Ok(s) => s,
            Err(e) => {
                cpu.bus.profile = None;
                cpu.dynrec.flush();
                super::set_active(false);
                return Err(e);
            }
        };
        Ok(Session {
            options,
            started,
            sampler: Some(sampler),
            samples,
            start: Totals::take(cpu),
            end: None,
            counters: None,
        })
    }

    /// Whether it still samples.
    pub fn running(&self) -> bool {
        self.end.is_none()
    }

    /// Stop once the sampler has, at the end of its duration. Call once a
    /// frame.
    pub fn poll(&mut self, cpu: &mut Cpu) {
        if self.end.is_none() && self.sampler.as_ref().is_some_and(|s| !s.running()) {
            self.stop(cpu);
        }
    }

    /// Stop sampling and counting; translated code goes back to blocks
    /// that don't note themselves.
    pub fn stop(&mut self, cpu: &mut Cpu) {
        if self.end.is_some() {
            return;
        }
        if let Some(mut sampler) = self.sampler.take() {
            sampler.stop();
        }
        self.end = Some(Totals::take(cpu));
        self.counters = cpu.bus.profile.take();
        cpu.dynrec.flush();
        super::set_active(false);
    }

    /// The samples so far.
    pub fn samples(&self) -> Samples {
        self.samples.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// How many samples there are so far, without copying them.
    pub fn total(&self) -> u64 {
        self.samples.lock().unwrap_or_else(|e| e.into_inner()).total
    }

    /// The counts: the bus's while it runs, the ones kept after.
    pub fn counters<'a>(&'a self, cpu: &'a Cpu) -> Option<&'a Counters> {
        self.counters.as_deref().or(cpu.bus.profile.as_deref())
    }

    /// The window so far: from the start to the stop, or to now.
    pub fn window(&self, cpu: &Cpu) -> (Totals, Totals) {
        (self.start.clone(), self.end.clone().unwrap_or_else(|| Totals::take(cpu)))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.end.is_none() {
            super::set_active(false);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One profile at a time among the tests: the word is the process's.
    pub static ONE_PROFILE: Mutex<()> = Mutex::new(());

    #[test]
    fn a_session_counts_samples_and_stops_at_its_duration() {
        let _one = ONE_PROFILE.lock().unwrap_or_else(|e| e.into_inner());
        let mut cpu = Cpu::new(std::path::PathBuf::from("."));
        let options = Options { hz: 4000, duration: Some(Duration::from_millis(150)), ..Options::default() };
        let mut session = Session::start(&mut cpu, options).unwrap();
        assert!(cpu.bus.profile.is_some());
        super::super::set_host(super::super::Zone::VgaRender);
        let t0 = Instant::now();
        while session.running() && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
            session.poll(&mut cpu);
        }
        assert!(!session.running(), "stopped at the end of its duration");
        assert!(cpu.bus.profile.is_none(), "the bus stopped counting");
        assert!(session.counters(&cpu).is_some(), "the counts are kept");
        let samples = session.samples();
        assert!(samples.total > 20, "{} samples", samples.total);
        let render = super::super::Loc::host(super::super::Zone::VgaRender).0;
        assert!(samples.counts.get(&render).copied().unwrap_or(0) * 2 > samples.total, "{:?}", samples.counts);
        assert!(!super::super::active());
    }
}
