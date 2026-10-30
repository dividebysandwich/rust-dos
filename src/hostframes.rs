//! How long the host's frames took and what the sound device went through
//! meanwhile, for the debug server and `RUST_DOS_AUTO_TRACE`.
//!
//! The frame loop calls `lap` where one kind of work ends, and `pump_audio`
//! calls `pump`, so the time between two pumps adds up to the sum of its
//! parts. The sound device waits from one pump to the next, and a pump
//! comes between the machine's run and the picture of a frame: what stands
//! in an underrun's record is the video work and presenting of the frame
//! before it and the machine's run of the frame it ended.
//!
//! All of this is the host's, and nothing in the machine reads it: it stays
//! out of save states and determinism checks. Each frame writes to memory
//! set aside at the start. Only reading it allocates.

use std::collections::VecDeque;
use std::time::Duration;
use web_time::Instant;

/// The frames that `summary` and `longest_since` look back over: about 17
/// seconds at 60 frames a second.
pub const FRAME_WINDOW: usize = 1024;
/// The underruns kept.
pub const UNDERRUN_LOG: usize = 32;

/// The kinds of work a frame's time goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Running the machine, with the devices that work as it runs.
    Cpu,
    /// Bringing the picture up to date and composing it.
    Video,
    /// Handing the picture to the window.
    Present,
    /// Waiting for the time of the next frame or the present.
    Wait,
    /// The rest: the host's events, the debug server's requests, pumping
    /// the sound.
    Other,
}

/// The time between two pumps by the work it went to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Split {
    pub cpu: Duration,
    pub video: Duration,
    pub present: Duration,
    pub wait: Duration,
    pub other: Duration,
}

impl Split {
    pub fn total(&self) -> Duration {
        self.cpu + self.video + self.present + self.wait + self.other
    }
}

/// What `pump_audio` found.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pump {
    /// The stereo frames in the device's queue when the pump came, and when
    /// it had finished.
    pub queued: usize,
    pub after: usize,
    /// The frames `pump_audio` keeps queued.
    pub target: usize,
    /// The emulated time.
    pub emulated_ns: u64,
}

/// An underrun: a stretch of pumps in a row that found the queue empty.
/// Everything but `dry_checks` and `stretch` is of the first. A dry pump
/// fills the queue up to its target, so each pump after the first in a
/// stretch found that the device had played all of that as well.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Underrun {
    /// Its number, from 1, as `HostFrames::underrun_events` counts.
    pub event: u64,
    /// How long after the frame loop's start it began.
    pub at: Duration,
    /// The time since the pump before, and what it went to.
    pub gap: Duration,
    pub split: Split,
    /// The frames in the queue when the pump before came and when it had
    /// finished; none if no pump came before.
    pub queued_before: Option<usize>,
    pub queued_after: Option<usize>,
    /// The frames `pump_audio` keeps queued.
    pub target: usize,
    /// Emulated seconds a wall second over the last quarter second, and
    /// over `gap`.
    pub speed: f32,
    pub gap_speed: Option<f32>,
    pub cycles_per_ms: u32,
    /// The pumps in a row that found the queue empty, and the time from
    /// the first to the last of them.
    pub dry_checks: u32,
    pub stretch: Duration,
}

/// `FRAME_WINDOW`'s frames as `summary` sums them up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSummary {
    /// How many there are, and how long they took together.
    pub frames: usize,
    pub span: Duration,
    pub longest: Duration,
    pub p50: Duration,
    pub p90: Duration,
    pub p99: Duration,
}

#[derive(Clone, Debug)]
pub struct HostFrames {
    /// The frames' wall time in microseconds and the emulated time at their
    /// ends, in rings, and how many came since the start.
    frames: Box<[u32]>,
    ends: Box<[u64]>,
    count: u64,
    /// The frame loop's start and its latest lap. Frontends without laps
    /// have neither, and only count underruns.
    start: Option<Instant>,
    last: Option<Instant>,
    /// The time since the last pump.
    split: Split,
    previous: Option<Pump>,
    /// The machine waits: its frames don't count, and the device running
    /// dry isn't an underrun.
    idle: bool,
    /// The last pump found the queue empty.
    dry: bool,
    events: u64,
    speed: f32,
    cycles_per_ms: u32,
    log: VecDeque<Underrun>,
}

impl Default for HostFrames {
    fn default() -> Self {
        Self::new()
    }
}

impl HostFrames {
    pub fn new() -> Self {
        Self {
            frames: vec![0; FRAME_WINDOW].into_boxed_slice(),
            ends: vec![0; FRAME_WINDOW].into_boxed_slice(),
            count: 0,
            start: None,
            last: None,
            split: Split::default(),
            previous: None,
            idle: false,
            dry: false,
            events: 0,
            speed: 0.0,
            cycles_per_ms: 0,
            log: VecDeque::with_capacity(UNDERRUN_LOG),
        }
    }

    /// The time since the last lap went to `phase`.
    #[inline]
    pub fn lap(&mut self, phase: Phase) {
        self.lap_at(phase, Instant::now());
    }

    fn lap_at(&mut self, phase: Phase, now: Instant) {
        self.start.get_or_insert(now);
        let Some(last) = self.last.replace(now) else {
            return;
        };
        let part = match phase {
            Phase::Cpu => &mut self.split.cpu,
            Phase::Video => &mut self.split.video,
            Phase::Present => &mut self.split.present,
            Phase::Wait => &mut self.split.wait,
            Phase::Other => &mut self.split.other,
        };
        *part += now - last;
    }

    /// The speeds to put in the next underrun's record: emulated seconds a
    /// wall second over the last quarter second, and instructions per ms.
    #[inline]
    pub fn note_speed(&mut self, speed: f32, cycles_per_ms: u32) {
        self.speed = speed;
        self.cycles_per_ms = cycles_per_ms;
    }

    /// A frame ended, from one start of the frame loop to the next, at
    /// the emulated time `emulated_ns`. Frames of a machine that waits
    /// don't count.
    #[inline]
    pub fn frame(&mut self, wall: Duration, emulated_ns: u64) {
        if self.idle {
            return;
        }
        let micros = wall.as_micros().min(u32::MAX as u128) as u32;
        let at = (self.count % FRAME_WINDOW as u64) as usize;
        self.frames[at] = micros;
        self.ends[at] = emulated_ns;
        self.count += 1;
    }

    /// Whether the machine waits, which `pump_audio` says each frame, with
    /// a sound device or without one.
    #[inline]
    pub fn set_idle(&mut self, idle: bool) {
        self.idle = idle;
    }

    /// `pump_audio` looked at the queue.
    pub fn pump(&mut self, pump: Pump) {
        let split = std::mem::take(&mut self.split);
        let previous = self.previous.replace(pump);
        if pump.queued != 0 || self.idle {
            self.dry = false;
            return;
        }
        let gap = split.total();
        if self.dry {
            if let Some(underrun) = self.log.back_mut() {
                underrun.dry_checks += 1;
                underrun.stretch += gap;
            }
            return;
        }
        self.dry = true;
        self.events += 1;
        let (Some(start), Some(last)) = (self.start, self.last) else {
            return;
        };
        let gap_speed = previous.filter(|_| !gap.is_zero()).map(|p| {
            pump.emulated_ns.saturating_sub(p.emulated_ns) as f32 / 1e9 / gap.as_secs_f32()
        });
        if self.log.len() == UNDERRUN_LOG {
            self.log.pop_front();
        }
        self.log.push_back(Underrun {
            event: self.events,
            at: last - start,
            gap,
            split,
            queued_before: previous.map(|p| p.queued),
            queued_after: previous.map(|p| p.after),
            target: pump.target,
            speed: self.speed,
            gap_speed,
            cycles_per_ms: self.cycles_per_ms,
            dry_checks: 1,
            stretch: Duration::ZERO,
        });
    }

    /// The underruns so far: stretches of dry pumps, which
    /// `Bus::audio_underruns` counts each pump of.
    pub fn underrun_events(&self) -> u64 {
        self.events
    }

    /// The latest underruns, oldest first.
    pub fn underruns(&self) -> impl Iterator<Item = &Underrun> {
        self.log.iter()
    }

    /// The frames counted so far.
    pub fn frame_count(&self) -> u64 {
        self.count
    }

    /// The longest frame that ended after the emulated time `ns`, of the
    /// last `FRAME_WINDOW`. It looks back as far as emulated time ran
    /// forward: a loaded save state can take the clock back, and the frames
    /// before that are of another time.
    pub fn longest_since(&self, ns: u64) -> Option<Duration> {
        let mut longest = None;
        let mut later = u64::MAX;
        for n in (self.count.saturating_sub(FRAME_WINDOW as u64)..self.count).rev() {
            let at = (n % FRAME_WINDOW as u64) as usize;
            if self.ends[at] <= ns || self.ends[at] > later {
                break;
            }
            later = self.ends[at];
            longest = longest.max(Some(self.frames[at]));
        }
        longest.map(|micros| Duration::from_micros(micros as u64))
    }

    /// The last `FRAME_WINDOW` frames; none before the first. The
    /// percentiles are the nearest rank's: the frame that this share of
    /// them took no longer than.
    pub fn summary(&self) -> Option<FrameSummary> {
        let frames = self.count.min(FRAME_WINDOW as u64) as usize;
        let mut sorted = self.frames[..frames].to_vec();
        sorted.sort_unstable();
        let rank = |percent: usize| {
            let at = (frames * percent).div_ceil(100).max(1) - 1;
            Duration::from_micros(sorted[at] as u64)
        };
        (frames > 0).then(|| FrameSummary {
            frames,
            span: Duration::from_micros(sorted.iter().map(|&m| m as u64).sum()),
            longest: rank(100),
            p50: rank(50),
            p90: rank(90),
            p99: rank(99),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    /// `HostFrames` with its laps at `base` plus the milliseconds given.
    struct Rig {
        frames: HostFrames,
        base: Instant,
        ms: u64,
    }

    impl Rig {
        fn new() -> Self {
            let mut frames = HostFrames::new();
            let base = Instant::now();
            frames.lap_at(Phase::Other, base);
            Self {
                frames,
                base,
                ms: 0,
            }
        }

        fn lap(&mut self, phase: Phase, ms: u64) {
            self.ms += ms;
            self.frames.lap_at(phase, self.base + MS * self.ms as u32);
        }

        fn pump(&mut self, queued: usize) {
            let emulated_ns = self.ms * 1_000_000;
            self.frames.pump(Pump {
                queued,
                after: queued + 700,
                target: 1400,
                emulated_ns,
                ..Pump::default()
            });
        }
    }

    #[test]
    fn an_underrun_has_the_time_since_the_last_pump_by_its_parts() {
        let mut rig = Rig::new();
        rig.lap(Phase::Cpu, 5);
        rig.pump(900);
        rig.frames.note_speed(0.75, 240_000);
        rig.lap(Phase::Other, 1);
        rig.lap(Phase::Video, 2);
        rig.lap(Phase::Wait, 3);
        rig.lap(Phase::Present, 4);
        rig.lap(Phase::Cpu, 50);
        rig.pump(0);
        let underrun = rig.frames.underruns().next().copied().unwrap();
        assert_eq!(underrun.event, 1);
        assert_eq!(underrun.at, MS * 65);
        assert_eq!(underrun.gap, MS * 60);
        assert_eq!(underrun.split.gap_parts(), [50, 2, 4, 3, 1]);
        assert_eq!(
            (underrun.queued_before, underrun.queued_after),
            (Some(900), Some(1600))
        );
        assert_eq!(underrun.target, 1400);
        assert_eq!((underrun.speed, underrun.cycles_per_ms), (0.75, 240_000));
        assert_eq!(
            underrun.gap_speed,
            Some(1.0),
            "as much emulated as wall time here"
        );
    }

    impl Split {
        fn gap_parts(&self) -> [u128; 5] {
            [self.cpu, self.video, self.present, self.wait, self.other].map(|d| d.as_millis())
        }
    }

    #[test]
    fn dry_pumps_in_a_row_are_one_underrun() {
        let mut rig = Rig::new();
        rig.lap(Phase::Cpu, 10);
        rig.pump(0);
        for _ in 0..2 {
            rig.lap(Phase::Cpu, 40);
            rig.pump(0);
        }
        assert_eq!(rig.frames.underrun_events(), 1);
        let first = rig.frames.underruns().next().copied().unwrap();
        assert_eq!((first.dry_checks, first.stretch), (3, MS * 80));
        assert_eq!(first.gap, MS * 10, "the record is of the first");
        // The queue has sound again, and the next dry pump starts another.
        rig.pump(500);
        rig.lap(Phase::Cpu, 40);
        rig.pump(0);
        assert_eq!(rig.frames.underrun_events(), 2);
        assert_eq!(rig.frames.underruns().count(), 2);
    }

    #[test]
    fn a_machine_that_waits_has_no_underruns_and_ends_a_stretch() {
        let mut rig = Rig::new();
        rig.pump(0);
        rig.frames.set_idle(true);
        rig.frames.pump(Pump::default());
        rig.frames.pump(Pump::default());
        assert_eq!(rig.frames.underrun_events(), 1);
        rig.frames.set_idle(false);
        rig.pump(0);
        assert_eq!(rig.frames.underrun_events(), 2);
    }

    #[test]
    fn frontends_without_laps_only_count() {
        let mut frames = HostFrames::new();
        frames.pump(Pump::default());
        assert_eq!(frames.underrun_events(), 1);
        assert_eq!(frames.underruns().count(), 0);
        frames.pump(Pump {
            queued: 100,
            ..Pump::default()
        });
        frames.pump(Pump::default());
        assert_eq!(frames.underrun_events(), 2);
    }

    #[test]
    fn the_log_keeps_the_latest_underruns() {
        let mut rig = Rig::new();
        let capacity = rig.frames.log.capacity();
        for _ in 0..UNDERRUN_LOG + 3 {
            rig.lap(Phase::Cpu, 1);
            rig.pump(0);
            rig.pump(100);
        }
        let events: Vec<u64> = rig.frames.underruns().map(|u| u.event).collect();
        assert_eq!(events.len(), UNDERRUN_LOG);
        assert_eq!(
            (events[0], *events.last().unwrap()),
            (4, UNDERRUN_LOG as u64 + 3)
        );
        assert_eq!(
            rig.frames.log.capacity(),
            capacity,
            "it allocates at the start only"
        );
    }

    #[test]
    fn the_summary_is_of_the_last_window_of_frames() {
        let mut frames = HostFrames::new();
        assert_eq!(frames.summary(), None);
        frames.frame(MS * 7, 0);
        let one = frames.summary().unwrap();
        assert_eq!(
            (one.frames, one.longest, one.p50, one.p99),
            (1, MS * 7, MS * 7, MS * 7)
        );

        // 1 to 100 ms in equal shares, the only ones left after the ring
        // went round.
        frames.frame(MS * 500, 0);
        let window = FRAME_WINDOW as u32;
        for n in 0..window * 2 {
            frames.frame(MS * ((n % window) * 100 / window + 1), 0);
        }
        let summary = frames.summary().unwrap();
        assert_eq!(summary.frames, FRAME_WINDOW);
        assert_eq!(summary.longest, MS * 100);
        assert_eq!(summary.p50, MS * 50);
        assert_eq!(summary.p90, MS * 90);
        assert_eq!(summary.p99, MS * 99);
    }

    #[test]
    fn longest_since_is_of_the_frames_after_the_mark() {
        let mut frames = HostFrames::new();
        frames.frame(MS * 90, 100);
        assert_eq!(frames.longest_since(100), None);
        frames.frame(MS * 5, 110);
        frames.frame(MS * 30, 120);
        frames.frame(MS * 7, 130);
        assert_eq!(frames.longest_since(100), Some(MS * 30));
        assert_eq!(frames.longest_since(120), Some(MS * 7));
        // Further back than the ring goes, it says what is left.
        for n in 0..FRAME_WINDOW as u64 {
            frames.frame(MS * 3, 200 + n);
        }
        assert_eq!(frames.longest_since(100), Some(MS * 3));
    }

    #[test]
    fn longest_since_stops_where_a_loaded_state_took_the_clock_back() {
        let mut frames = HostFrames::new();
        frames.frame(MS * 90, 5000);
        frames.frame(MS * 5, 110);
        frames.frame(MS * 6, 120);
        assert_eq!(frames.longest_since(100), Some(MS * 6));
    }

    #[test]
    fn the_frames_of_a_machine_that_waits_do_not_count() {
        let mut frames = HostFrames::new();
        frames.set_idle(true);
        frames.frame(MS * 900, 0);
        assert_eq!(frames.frame_count(), 0);
        frames.set_idle(false);
        frames.frame(MS * 9, 0);
        assert_eq!(frames.frame_count(), 1);
    }

    #[test]
    fn save_states_leave_it_out() {
        let mut cpu = crate::cpu::Cpu::new(std::path::PathBuf::from("."));
        let before = crate::savestate::machine::save(&cpu);
        cpu.bus.host_frames.lap(Phase::Cpu);
        cpu.bus.host_frames.lap(Phase::Video);
        cpu.bus.host_frames.frame(MS * 9, 0);
        cpu.bus.host_frames.pump(Pump::default());
        assert_eq!(crate::savestate::machine::save(&cpu), before);
    }
}
